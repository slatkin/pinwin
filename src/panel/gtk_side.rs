//! The GTK side of the `Panel` lifecycle (port-to-rust D4): the
//! process-lifetime `pinwin-gtk` thread, the per-start `GtkApplication` and
//! glue build, and the commands the host posts with
//! `MainContext::invoke` (applies and teardown). Ported from `src/pinwin_api.c`
//! and `glue.c`'s activation half.
//!
//! One thread, parked between panels: `gtk::init` records the first
//! initialising thread and panics on a second one, so a thread per start that
//! joins on drop cannot restart (D4's task 1.2 spike); this thread calls
//! `gtk::init` once through [`crate::surfaces::init`] and runs each panel's
//! own `GtkApplication` loop on it. All GTK objects, the terminal, the pty
//! state and the draw state live in the [`GLUE`] thread-local on this thread,
//! so nothing needs to be `Send` (the ghostty terminal types are `!Send`,
//! D2 item 8).
//!
//! The host never joins the thread (D4): a command that arrives when no loop
//! runs (the parked thread, or a host thread the context does not own) is
//! dispatched inline by `MainContext::invoke` on the *calling* thread, where
//! [`GLUE`] is `None` and the command reports not-live without touching GTK
//! state.
//!
//! Panics never cross back into GTK/glib (D5): every closure registered here
//! runs its body through the shared [`crate::guard`] helpers, latching the
//! panel's one shared poisoned flag.

use std::cell::{Cell, RefCell};
use std::os::fd::RawFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use crate::guard::{Poisoned, guard, guard_always};
use crate::input::InputLinks;
use crate::layout::Layout;
use crate::pty::Pty;
use crate::render::DrawState;
use crate::surfaces::{DrawFn, MeasureFn, PublishOutcome, SurfaceHooks, Surfaces};
use crate::term::Terminal;

use gtk4::cairo;
use gtk4::glib;
use gtk4::glib::ControlFlow;
use gtk4::prelude::*;

use super::Inner;
use super::handshake::{Handshake, StartOutcome};

/// The startup arguments one panel runs with (D6, D7): the host-owned pty
/// master fd, the full layout, the keyboard mode and the optional focus
/// accent. `Copy`, so a start command can carry it across threads.
#[derive(Clone, Copy, Debug)]
pub struct Startup {
    /// The host-owned pty master fd (D7). The library never closes it.
    pub fd: RawFd,
    /// The full startup layout.
    pub layout: Layout,
    /// The keyboard interactivity mode, fixed at start time.
    pub keyboard: crate::layout::Keyboard,
    /// The focus accent; `None` disables it.
    pub accent: Option<crate::layout::Accent>,
}

/// A start command delivered to the parked GTK thread. Everything in it is
/// `Send`: the layout and the fd are plain data, the latch is an atomic, the
/// handshake and the handle state are `Arc`/channel handles.
pub(crate) struct StartCommand {
    pub startup: Startup,
    pub id: u64,
    pub poisoned: Poisoned,
    pub handshake: Handshake,
    pub inner: Arc<Inner>,
}

/// The per-start application id prefix (D4: each start creates and runs its
/// own `GtkApplication` with a distinct id; `NON_UNIQUE` keeps the id off the
/// session bus).
fn app_id(id: u64) -> String {
    format!("dev.pinwin.panel{id}")
}

/// The live panel's GTK-side state, on the GTK thread (D4). The host's
/// thread-local instance is always `None`, so an invoked command that runs
/// inline on the host thread reports not-live instead of touching GTK state.
struct LivePanel {
    id: u64,
    poisoned: Poisoned,
    surfaces: Rc<Surfaces>,
}

thread_local! {
    static GLUE: RefCell<Option<LivePanel>> = const { RefCell::new(None) };
}

/// A late-bound weak handle to the surfaces: the hook closures are built
/// before [`Surfaces::build`] returns the `Rc`, so they hold one of these and
/// upgrade it per call (surfaces closures hold `Weak`s of the surfaces too,
/// so nothing keeps a closed panel's glue alive).
#[derive(Clone, Default)]
struct SurfacesLink(Rc<RefCell<Option<std::rc::Weak<Surfaces>>>>);

impl SurfacesLink {
    fn new() -> Self {
        SurfacesLink(Rc::new(RefCell::new(None)))
    }

    fn set(&self, weak: std::rc::Weak<Surfaces>) {
        *self.0.borrow_mut() = Some(weak);
    }

    /// Run `body` with the live surfaces, or nothing when the panel is gone.
    fn with<R>(&self, body: impl FnOnce(&Surfaces) -> R) -> Option<R> {
        // Clone the weak and drop the guard before the body runs.
        let weak = self.0.borrow().as_ref().map(std::rc::Weak::clone)?;
        weak.upgrade().map(|surfaces| body(&surfaces))
    }
}

/// The GTK thread's entry (D5 boundary): a panic anywhere in the loop leaves
/// the command channel closed, so a pending start reports `Internal` instead
/// of hanging, and the thread ends rather than run broken glue.
pub(crate) fn gtk_thread_main(receiver: mpsc::Receiver<StartCommand>) {
    let latch = Poisoned::new();
    let _ = guard_always(&latch, || {
        while let Ok(command) = receiver.recv() {
            run_one_panel(command);
        }
    });
}

/// One panel's lifetime on the GTK thread: init the application, build the
/// glue on activate, run the loop until teardown quits it, then clean up and
/// park. The start handshake is completed by the glue's start-result hook
/// (the first draw's monitor resolution), the startup watchdog or the
/// loop-returned cleanup — exactly one of them, via [`Handshake`].
fn run_one_panel(command: StartCommand) {
    let StartCommand {
        startup,
        id,
        poisoned,
        handshake,
        inner,
    } = command;

    // GTK and layer-shell init (glue_init's checks). A failure here is a
    // missing display or missing wlr-layer-shell: NoDisplay, nothing opened.
    let app = match guard(&poisoned, || crate::surfaces::init(Some(&app_id(id)))) {
        Ok(Ok(app)) => app,
        Ok(Err(failure)) => {
            handshake.report(StartOutcome::from(failure));
            return;
        }
        Err(_) => {
            handshake.report(StartOutcome::Internal);
            return;
        }
    };

    // The glue is built on activate, inside the application's own loop.
    let activate_poisoned = poisoned.clone();
    let activate_handshake = handshake.clone();
    let weak_app = glib::WeakRef::<gtk4::Application>::new();
    weak_app.set(Some(&app));
    app.connect_activate(move |_| {
        let Some(app) = weak_app.upgrade() else {
            return;
        };
        match guard(&activate_poisoned, || {
            build_glue(&app, &startup, &activate_poisoned, &activate_handshake)
        }) {
            Ok(surfaces) => {
                GLUE.with(|cell| {
                    *cell.borrow_mut() = Some(LivePanel {
                        id,
                        poisoned: activate_poisoned.clone(),
                        surfaces,
                    });
                });
            }
            // A caught panic latched the shared flag: fail a still-pending
            // start handshake and quit the loop (D5).
            Err(_) => {
                activate_handshake.report(StartOutcome::Internal);
                app.quit();
            }
        }
    });

    // The startup watchdog: while the handshake is still pending, a panic
    // anywhere in the startup path (activate, map, first draw) must not hang
    // the waiting host forever. It reports Internal, tears the half-built
    // panel down and quits the loop. Once the handshake is resolved the
    // watchdog removes itself, and a resolved handshake is never re-reported.
    let watchdog_handshake = handshake.clone();
    let watchdog_poisoned = poisoned.clone();
    let watchdog_app = glib::WeakRef::<gtk4::Application>::new();
    watchdog_app.set(Some(&app));
    glib::timeout_add_local(Duration::from_millis(200), move || {
        if watchdog_handshake.resolved() {
            return ControlFlow::Break;
        }
        if watchdog_poisoned.is_poisoned() {
            watchdog_handshake.report(StartOutcome::Internal);
            close_glue(id);
            if let Some(app) = watchdog_app.upgrade() {
                app.quit();
            }
            return ControlFlow::Break;
        }
        ControlFlow::Continue
    });

    // Run the panel's own application loop (g_application_run with no
    // arguments, as the C did). Blocks until a teardown quits it, or the
    // startup fails and quits it.
    let _exit = app.run_with_args(&[] as &[&str]);

    // The loop returned. Fail a still-pending handshake (the C's
    // loop-returned-before-live path); a completed handshake is unaffected.
    handshake.report(StartOutcome::NoDisplay);
    // Close anything a teardown did not (a failed start, a loop that ended on
    // its own), then mark the handle not live so applies stop posting.
    close_glue(id);
    inner.live.store(false, Ordering::Relaxed);
}

/// Take this panel's glue out of [`GLUE`] and close its surfaces, when the
/// entry is still this panel's (a queued teardown from an earlier panel must
/// not close the one that runs now). The close runs even on a latched flag:
/// it is the state reset the drop contract relies on (D5).
fn close_glue(id: u64) {
    GLUE.with(|cell| {
        let scratch = Poisoned::new();
        let _ = guard_always(&scratch, || {
            let mine = cell.borrow().as_ref().is_some_and(|live| live.id == id);
            if mine {
                let live = cell.borrow_mut().take().expect("panel checked present");
                let _ = guard_always(&live.poisoned, || live.surfaces.close());
            }
        });
    });
}

/// The apply command, posted from the host thread with
/// `MainContext::invoke` (D4): the layout is a `Copy` value, the reply is a
/// bounded `sync_channel(1)`. Answers `NotLive` when this command's panel is
/// no longer the one running — a queued command from a dropped panel must not
/// touch the panel that runs now — and `Terminal` when the publish panicked
/// or the shared latch was already set (D5: a panic reports Internal, never
/// NotRunning).
pub(crate) fn dispatch_apply(
    id: u64,
    layout: Layout,
    duration_ms: u32,
    reply: mpsc::SyncSender<PublishOutcome>,
) {
    GLUE.with(|cell| {
        // The plumbing around the publish is itself a boundary closure (D5):
        // a panic here still reports through the reply instead of unwinding
        // into glib's dispatch.
        let scratch = Poisoned::new();
        let outcome = guard_always(&scratch, || match cell.borrow().as_ref() {
            Some(live) if live.id == id => guard(&live.poisoned, || {
                live.surfaces.publish(layout, duration_ms)
            })
            .unwrap_or(PublishOutcome::Terminal),
            _ => PublishOutcome::NotLive,
        })
        .unwrap_or(PublishOutcome::Terminal);
        let _ = reply.send(outcome);
    });
}

/// The teardown command, posted from the host thread's `Drop` (D4/D5): close
/// the surfaces (cancelling any running animation first, as `Surfaces::close`
/// does), quit the loop so the thread returns to its park, and report. Runs
/// even on a latched flag — the drop contract closes the panel regardless —
/// and always replies.
pub(crate) fn dispatch_teardown(id: u64, reply: mpsc::SyncSender<()>) {
    GLUE.with(|cell| {
        let scratch = Poisoned::new();
        let _ = guard_always(&scratch, || {
            let mine = cell.borrow().as_ref().is_some_and(|live| live.id == id);
            if mine {
                let live = cell.borrow_mut().take().expect("panel checked present");
                let _ = guard_always(&live.poisoned, || {
                    live.surfaces.close();
                    live.surfaces.app.quit();
                });
            }
        });
        let _ = reply.send(());
    });
}

/// Build one panel's glue (`on_activate`): the terminal, the pty, the draw
/// state and the surfaces, wired together through the [`SurfaceHooks`] slots
/// with the panel's one shared poisoned latch. Returns the surfaces; the rest
/// of the glue lives in the hook closures and is torn down with them.
fn build_glue(
    app: &gtk4::Application,
    startup: &Startup,
    poisoned: &Poisoned,
    handshake: &Handshake,
) -> Rc<Surfaces> {
    let theme = crate::fontconfig::load_theme_colours();
    let draw = Rc::new(RefCell::new(DrawState::new(
        poisoned.clone(),
        startup.accent,
        theme,
    )));
    // The cell size is unknown until the first measurement; the measure hook
    // records it before any winsize or draw uses it.
    let pty = Rc::new(RefCell::new(Pty::new(poisoned.clone(), startup.fd, 0, 0)));
    let link = SurfacesLink::new();
    // Whether the panel holds keyboard focus (`g_focused`): the input
    // controllers set it, the draw hook copies it into the draw state.
    let focused = Rc::new(Cell::new(false));
    let writer = pty.borrow().writer();
    let terminal = Rc::new(RefCell::new(Terminal::new(
        poisoned.clone(),
        writer,
        crate::render::PixbufDecoder,
        {
            let link = link.clone();
            move || {
                link.with(|surfaces| surfaces.queue_draw());
            }
        },
    )));

    let hooks = SurfaceHooks {
        draw: Some(draw_hook(
            link.clone(),
            draw.clone(),
            terminal.clone(),
            focused.clone(),
            poisoned.clone(),
            handshake.clone(),
        )),
        apply_size: apply_size_hook(
            link.clone(),
            terminal.clone(),
            pty.clone(),
            poisoned.clone(),
        ),
        measure: measure_hook(draw.clone(), pty.clone(), poisoned.clone()),
        tween_cache_drop: {
            let draw = draw.clone();
            let poisoned = poisoned.clone();
            Rc::new(move || {
                let _ = guard(&poisoned, || draw.borrow_mut().drop_grid_cache());
            })
        },
        start_result: {
            let handshake = handshake.clone();
            Rc::new(move |ok| {
                handshake.report(if ok {
                    StartOutcome::Started
                } else {
                    StartOutcome::NoDisplay
                });
            })
        },
        set_tween_active: {
            let pty = pty.clone();
            let poisoned = poisoned.clone();
            Rc::new(move |active| {
                let _ = guard(&poisoned, || pty.borrow().set_tween_active(active));
            })
        },
    };
    let surfaces = Surfaces::build(
        app,
        startup.layout,
        startup.keyboard,
        startup.accent,
        hooks,
        poisoned.clone(),
    );
    surfaces.attach_input(InputLinks {
        terminal: terminal.clone(),
        draw_offset: {
            let link = link.clone();
            Rc::new(move || link.with(|surfaces| surfaces.draw_offset()).unwrap_or(0.0))
        },
        focused: focused.clone(),
        queue_draw: {
            let link = link.clone();
            Rc::new(move || {
                link.with(|surfaces| surfaces.queue_draw());
            })
        },
        poisoned: poisoned.clone(),
    });
    // The late-bound handles are live from here on.
    link.set(Rc::downgrade(&surfaces));
    surfaces
}

/// The drawing area's draw function (`render.c`'s `on_draw`): resolve the
/// layout monitor on the first draw (`g_layout_latch`), then render the
/// frame. A panic here latches the shared flag, fails a still-pending start
/// handshake and quits the loop (D5: stop drawing and quit).
fn draw_hook(
    link: SurfacesLink,
    draw: Rc<RefCell<DrawState>>,
    terminal: Rc<RefCell<Terminal>>,
    focused: Rc<Cell<bool>>,
    poisoned: Poisoned,
    handshake: Handshake,
) -> Rc<DrawFn> {
    Rc::new(move |cr: &cairo::Context, width: i32, height: i32| {
        let outcome = guard(&poisoned, || {
            // The first draw is the first point where the surface has entered
            // its output, so the monitor reported here is the panel's real
            // one (glue.c's resolve_layout_monitor via g_layout_latch).
            link.with(|surfaces| {
                if surfaces.latch.get() {
                    surfaces.resolve_monitor();
                }
            });
            let (offset, animating) = link
                .with(|surfaces| (surfaces.draw_offset() as i32, surfaces.anim.active()))
                .unwrap_or((0, false));
            draw.borrow_mut().set_focused(focused.get());
            draw.borrow_mut().draw(
                cr,
                &mut terminal.borrow_mut(),
                width,
                height,
                offset,
                animating,
            );
        });
        if outcome.is_err() {
            handshake.report(StartOutcome::Internal);
            link.with(|surfaces| surfaces.app.quit());
        }
    })
}

/// The cell measurement against a widget (`render.c`'s
/// `cell_metrics_update`), taken from the widget's pango context before the
/// first draw as glue.c's `on_activate` did, and recorded into the pty's
/// winsize pixel fields. Reports the cell size in pixels.
fn measure_hook(
    draw: Rc<RefCell<DrawState>>,
    pty: Rc<RefCell<Pty>>,
    poisoned: Poisoned,
) -> Rc<MeasureFn> {
    Rc::new(move |widget: &gtk4::Widget| {
        guard(&poisoned, || {
            let context = widget.create_pango_context();
            let (cell_w, cell_h) = {
                let mut draw = draw.borrow_mut();
                draw.cell_metrics_update(&context);
                (draw.cell_w(), draw.cell_h())
            };
            pty.borrow_mut().set_cell_size(cell_w as u32, cell_h as u32);
            (cell_w, cell_h)
        })
        .unwrap_or((0, 0))
    })
}

/// The grid resize behind `apply_size` (`src/pty.c`): recompute the rows from
/// the drawing area's allocation, push the grid through the terminal and the
/// pty when it changed (never while a tween runs — the tween draws the old
/// grid from its cache and the resize is deferred to its end), and attach the
/// pty read source on the first call. `false` reports a terminal that could
/// not be allocated; the previous grid stays (`glue_publish_layout`'s
/// `GLUE_ERR_TERMINAL` path).
fn apply_size_hook(
    link: SurfacesLink,
    terminal: Rc<RefCell<Terminal>>,
    pty: Rc<RefCell<Pty>>,
    poisoned: Poisoned,
) -> Rc<dyn Fn() -> bool> {
    // The last grid pushed to the terminal (`g_rows`/`g_grid_cols`): rows
    // follow the allocated height, columns stay the applied input and are
    // never re-derived from the allocated width (pty.c's design D4).
    let pushed = Cell::new((0i32, 0i32));
    Rc::new(move || {
        guard(&poisoned, || {
            link.with(|surfaces| apply_size_to(surfaces, &terminal, &pty, &pushed))
                // No surfaces (torn down, or the build has not returned yet):
                // nothing to size, and not a terminal failure.
                .unwrap_or(true)
        })
        .unwrap_or(false)
    })
}

/// The `apply_size` body against live surfaces.
fn apply_size_to(
    surfaces: &Surfaces,
    terminal: &Rc<RefCell<Terminal>>,
    pty: &Rc<RefCell<Pty>>,
    pushed: &Cell<(i32, i32)>,
) -> bool {
    let height = surfaces.area.height();
    let cell_h = surfaces.cell_h();
    if cell_h > 0 && height > 0 {
        let rows = (height / cell_h).max(1);
        let cols = i32::from(surfaces.cols());
        if !surfaces.anim.active() && (rows, cols) != pushed.get() {
            // Push the grid first: a terminal that cannot be allocated leaves
            // the previous grid and pty winsize in place (the library never
            // exits, port-to-rust D3).
            let ok = terminal
                .borrow_mut()
                .push_size(cols, rows, surfaces.cell_w(), cell_h);
            if !ok {
                return false;
            }
            pushed.set((rows, cols));
            pty.borrow().resize(cols, rows);
        }
    }
    if !pty.borrow().attached() {
        // Attach with the terminal's effective grid (the 40x24 default before
        // the first push, as `g_pty_cols`/`g_pty_rows` in the C). A failed
        // attach degrades to no terminal; it never exits (D3) and is not a
        // layout verdict.
        let (cols, rows) = {
            let terminal = terminal.borrow();
            (i32::from(terminal.cols()), i32::from(terminal.rows()))
        };
        let feed_terminal = terminal.clone();
        let _ = pty.borrow_mut().attach(cols, rows, move |data| {
            feed_terminal.borrow_mut().push_pty_data(data)
        });
    }
    true
}
