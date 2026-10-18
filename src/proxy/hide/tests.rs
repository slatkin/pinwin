//! The hide and show state machine tests (D6): display-free, so they run
//! without a compositor (`port-to-rust` D10).

use std::rc::Rc;
use std::time::Duration;

use wl_proxy::baseline::Baseline;
use wl_proxy::object::ObjectCoreApi;
use wl_proxy::protocols::linux_drm_syncobj_v1::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;
use wl_proxy::protocols::wayland::wl_buffer::WlBuffer;
use wl_proxy::state::State;

use super::{Attach, Hidden};
use crate::config::PanelSettings;
use crate::daemon::events::{Bus, Event};
use crate::daemon::panel::PanelReply;
use crate::ipc::Request;
use crate::name::PanelName;
use crate::proxy::buffers::{PanelBuffers, Release};
use crate::proxy::routing::Routing;

/// A display-free state: the test mints proxy objects without a
/// compositor (`port-to-rust` D10).
fn state() -> Rc<State> {
    State::builder(Baseline::V6)
        .without_server()
        .build()
        .expect("a client-side state needs no compositor")
}

/// One `wl_buffer` object the app could attach.
fn buffer(state: &Rc<State>) -> Rc<WlBuffer> {
    state.create_object::<WlBuffer>(1)
}

/// One imported timeline, which a held point names.
fn timeline(state: &Rc<State>) -> Rc<WpLinuxDrmSyncobjTimelineV1> {
    state.create_object::<WpLinuxDrmSyncobjTimelineV1>(1)
}

/// The panel's routing with the built-in settings.
fn routing() -> Rc<Routing> {
    Routing::new(PanelSettings::built_in())
}

/// A display-free buffer-ownership model for the panel surface.
fn buffers() -> Rc<PanelBuffers> {
    Rc::new(PanelBuffers::new())
}

/// The hidden state of a panel that starts hidden or shown.
fn hidden(shown: bool) -> Hidden {
    Hidden::new(shown, buffers())
}

/// A panel name the events name.
fn notes() -> PanelName {
    PanelName::parse("notes").expect("a panel name")
}

/// A toggle hides a shown panel and shows a hidden one, each real change
/// publishing one event (spec "Hide", "Show a hidden panel").
#[test]
fn a_toggle_hides_then_shows_with_one_event_each() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let panel = notes();
    assert_eq!(
        super::answer(
            &routing,
            &Request::Toggle {
                panel: panel.clone()
            },
            &panel,
            &bus
        ),
        Some(PanelReply::Done)
    );
    assert!(routing.hidden.is_hidden());
    assert_eq!(
        events.next(Duration::from_millis(100)),
        Ok(Event::Hidden {
            panel: panel.clone()
        }
        .line())
    );
    assert_eq!(
        super::answer(
            &routing,
            &Request::Toggle {
                panel: panel.clone()
            },
            &panel,
            &bus
        ),
        Some(PanelReply::Done)
    );
    assert!(!routing.hidden.is_hidden());
    assert_eq!(
        events.next(Duration::from_millis(100)),
        Ok(Event::Shown {
            panel: panel.clone()
        }
        .line())
    );
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
}

/// Showing a shown panel and hiding a hidden one change nothing and
/// publish nothing (spec "Show a shown panel").
#[test]
fn showing_a_shown_panel_and_hiding_a_hidden_one_are_no_ops() {
    let bus = Bus::new();
    let events = bus.subscribe();
    let routing = routing();
    let panel = notes();
    assert_eq!(
        super::answer(
            &routing,
            &Request::Show {
                panel: panel.clone()
            },
            &panel,
            &bus
        ),
        Some(PanelReply::Done)
    );
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    assert_eq!(
        super::answer(
            &routing,
            &Request::Hide {
                panel: panel.clone()
            },
            &panel,
            &bus
        ),
        Some(PanelReply::Done)
    );
    assert_eq!(
        events.next(Duration::from_millis(100)),
        Ok(Event::Hidden {
            panel: panel.clone()
        }
        .line())
    );
    assert_eq!(
        super::answer(
            &routing,
            &Request::Hide {
                panel: panel.clone()
            },
            &panel,
            &bus
        ),
        Some(PanelReply::Done)
    );
    assert_eq!(
        events.next(Duration::from_millis(50)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
}

/// A visible commit forwards its attach and becomes the buffer the next
/// show attaches; showing is a forward, not a drop (D6 amendments 2, 3).
#[test]
fn a_visible_buffer_becomes_the_one_show_attaches() {
    let state = state();
    let hidden = hidden(false);
    let first = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Forward);
    let plan = hidden.commit_plan();
    assert!(plan.release.is_empty(), "a visible attach is not released");
    let (kept, _points) = hidden
        .show_buffer()
        .expect("a visible commit keeps its buffer");
    assert_eq!(kept.unique_id(), first.unique_id());
}

/// While hidden the first buffer is kept for show and each later one is
/// dropped and released after the commit (D6, "Hide releases the
/// reservation").
#[test]
fn a_hidden_panel_keeps_the_first_buffer_and_releases_the_next() {
    let state = state();
    let hidden = hidden(true);
    let first = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
    let plan = hidden.commit_plan();
    assert!(plan.release.is_empty(), "the kept buffer is not released");
    let (kept, _points) = hidden.show_buffer().expect("the first buffer is kept");
    assert_eq!(kept.unique_id(), first.unique_id());

    let second = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&second)), Attach::Drop);
    let plan = hidden.commit_plan();
    assert_eq!(plan.release.len(), 1, "the dropped attach owes one release");
    assert_eq!(plan.release[0].unique_id(), second.unique_id());
    let (kept, _points) = hidden.show_buffer().expect("the kept buffer stays");
    assert_eq!(kept.unique_id(), first.unique_id());
}

/// Reattaching the kept buffer while hidden neither drops nor releases
/// it: the proxy holds it for show (D6 amendment 2).
#[test]
fn a_hidden_reattach_of_the_kept_buffer_is_held() {
    let state = state();
    let hidden = hidden(true);
    let first = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
    let _ = hidden.commit_plan();
    assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
    let plan = hidden.commit_plan();
    assert!(plan.release.is_empty());
}

/// A proxy-made attach of a buffer the app already owns is a borrow: the
/// compositor's release for it is swallowed, and only a later app attach
/// makes the next release the app's (D6 amendment 3).
#[test]
fn a_proxy_attach_of_an_app_held_buffer_swallows_its_release() {
    let state = state();
    let hidden = hidden(false);
    let buffer = buffer(&state);
    hidden.on_proxy_attach(&buffer);
    assert_eq!(
        hidden.on_release(buffer.unique_id()),
        Release::Swallow,
        "the app already owns the buffer"
    );
    assert_eq!(
        hidden.on_release(buffer.unique_id()),
        Release::Swallow,
        "no app attach is outstanding"
    );

    assert_eq!(hidden.on_app_attach(Some(&buffer)), Attach::Forward);
    let _ = hidden.commit_plan();
    assert_eq!(
        hidden.on_release(buffer.unique_id()),
        Release::Forward,
        "the app's own attach owns the release"
    );
}

/// Holding points across a hidden commit keeps them for the next
/// buffer-carrying commit; a visible buffer commit emits them (D6). A
/// hidden commit carries no buffer because the proxy dropped the attach.
#[test]
fn held_points_wait_for_a_buffer_carrying_commit() {
    let state = state();
    let hidden = hidden(true);
    let timeline = timeline(&state);
    hidden.remember_acquire(&timeline, 0, 1);
    hidden.remember_release(&timeline, 0, 2);
    let plan = hidden.commit_plan();
    assert!(
        plan.points.is_none(),
        "a commit without a buffer emits no points"
    );

    let buffer = buffer(&state);
    hidden.set_hidden(false);
    assert_eq!(hidden.on_app_attach(Some(&buffer)), Attach::Forward);
    let plan = hidden.commit_plan();
    let points = plan.points.expect("a buffer-carrying commit emits points");
    assert!(!points.is_empty());
    let (_kept, kept_points) = hidden.show_buffer().expect("the buffer is kept");
    assert!(!kept_points.is_empty(), "the kept buffer keeps its points");
}

/// A panel that starts hidden keeps its first buffer without releasing
/// it: when show attaches it, the compositor's release is the app's own
/// and must reach it (D6 amendments 2 and 3).
#[test]
fn a_kept_buffer_of_a_panel_that_started_hidden_keeps_its_release() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let plan = hidden.commit_plan();
    assert!(
        plan.release.is_empty(),
        "the held attach owes no drop release"
    );

    hidden.set_hidden(false);
    hidden.on_proxy_attach(&kept);
    assert_eq!(
        hidden.on_release(kept.unique_id()),
        Release::Forward,
        "the held buffer's release reaches the app"
    );
}

/// A kept buffer the show no longer attaches is released to the app
/// exactly once, because the compositor never saw its held attach (D6
/// amendment 3).
#[test]
fn a_replaced_kept_buffer_is_released_once() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let _ = hidden.commit_plan();

    hidden.set_hidden(false);
    let next = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&next)), Attach::Forward);
    let plan = hidden.commit_plan();
    assert!(
        plan.points.is_some(),
        "the forwarded attach carries a buffer"
    );
    assert_eq!(plan.release.len(), 1, "the held buffer is released");
    assert_eq!(plan.release[0].unique_id(), kept.unique_id());
    assert!(
        hidden.commit_plan().release.is_empty(),
        "the app gets one release for it"
    );
}

/// A buffer whose app-side release already went out still has its
/// proxy-made attach's release swallowed (D6 amendment 3): the app owns the
/// buffer again, so no release is owed to it.
#[test]
fn a_released_buffer_that_the_proxy_attaches_is_still_swallowed() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let _ = hidden.commit_plan();
    let dropped = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&dropped)), Attach::Drop);
    let plan = hidden.commit_plan();
    assert_eq!(plan.release.len(), 1);
    assert_eq!(plan.release[0].unique_id(), dropped.unique_id());

    hidden.set_hidden(false);
    hidden.on_proxy_attach(&dropped);
    assert_eq!(
        hidden.on_release(dropped.unique_id()),
        Release::Swallow,
        "the app already had the release for its dropped attach"
    );
    assert_eq!(
        hidden.on_release(dropped.unique_id()),
        Release::Swallow,
        "no app attach is outstanding"
    );
}

/// A show that lands before the app commits a dropped hidden attach
/// emits no held points, because that commit carries no buffer (the
/// protocol's `no_buffer`): the dropped buffer is released instead (D6).
#[test]
fn a_show_before_the_commit_emits_no_points_for_a_dropped_attach() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let _ = hidden.commit_plan();

    let dropped = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&dropped)), Attach::Drop);
    let timeline = timeline(&state);
    hidden.remember_acquire(&timeline, 0, 1);
    hidden.remember_release(&timeline, 0, 2);
    hidden.set_hidden(false);

    let plan = hidden.commit_plan();
    assert!(
        plan.points.is_none(),
        "the commit carries no buffer, so it carries no points"
    );
    assert_eq!(plan.release.len(), 1);
    assert_eq!(plan.release[0].unique_id(), dropped.unique_id());
    let (buffer, _points) = hidden.show_buffer().expect("the kept buffer stays");
    assert_eq!(buffer.unique_id(), kept.unique_id());
}

/// The buffer a crop must stay inside is the one the compositor has after
/// the commit (D6, D8): while hidden the app's attach never reaches it, so
/// the kept buffer — the one show attaches — bounds the crop, not the
/// buffer the proxy just dropped.
#[test]
fn a_hidden_commit_names_the_kept_buffer_as_the_compositors_own() {
    let state = state();
    let buffers = buffers();
    let hidden = Hidden::new(true, Rc::clone(&buffers));
    assert_eq!(buffers.kept_id(), None, "nothing is kept yet");

    let first = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&first)), Attach::Hold);
    let _ = hidden.commit_plan();
    assert_eq!(buffers.kept_id(), Some(first.unique_id()));

    let dropped = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&dropped)), Attach::Drop);
    let _ = hidden.commit_plan();
    assert_eq!(
        buffers.kept_id(),
        Some(first.unique_id()),
        "a dropped hidden attach never reaches the compositor"
    );

    let forwarded = buffer(&state);
    hidden.set_hidden(false);
    assert_eq!(hidden.on_app_attach(Some(&forwarded)), Attach::Forward);
    let _ = hidden.commit_plan();
    assert_eq!(
        buffers.kept_id(),
        Some(forwarded.unique_id()),
        "a shown commit forwards the buffer it attached"
    );
}

/// A show that lands before the app commits a held hidden attach keeps
/// its points with the kept buffer, which waits for the configure ack
/// (D6 amendment 2).
#[test]
fn a_show_before_the_commit_keeps_a_held_attachs_points() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let timeline = timeline(&state);
    hidden.remember_acquire(&timeline, 0, 1);
    hidden.remember_release(&timeline, 0, 2);
    hidden.set_hidden(false);

    let plan = hidden.commit_plan();
    assert!(plan.points.is_none(), "the commit carries no buffer");
    assert!(plan.release.is_empty());
    let (buffer, points) = hidden.show_buffer().expect("the held buffer is kept");
    assert_eq!(buffer.unique_id(), kept.unique_id());
    assert!(!points.is_empty(), "the held points stay with the buffer");
}

/// The app destroying the buffer the proxy keeps for show leaves nothing to
/// attach (D6): show must not send a destroyed buffer.
#[test]
fn a_destroyed_kept_buffer_is_not_attached_again() {
    let state = state();
    let buffers = buffers();
    let hidden = Hidden::new(true, Rc::clone(&buffers));
    let kept = buffer(&state);
    buffers.record(&kept, 640, 480);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let _ = hidden.commit_plan();
    assert!(hidden.show_buffer().is_some());

    hidden.forget(kept.unique_id());

    assert!(
        hidden.show_buffer().is_none(),
        "a destroyed buffer is not attached"
    );
    assert_eq!(
        buffers.kept_id(),
        None,
        "a destroyed buffer is no crop bound"
    );
}

/// A dropped hidden attach whose buffer the app destroys before the commit
/// owes no release (D6): there is no buffer left to release.
#[test]
fn a_destroyed_dropped_buffer_owes_no_release() {
    let state = state();
    let hidden = hidden(true);
    let kept = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&kept)), Attach::Hold);
    let _ = hidden.commit_plan();
    let dropped = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&dropped)), Attach::Drop);

    hidden.forget(dropped.unique_id());

    assert!(
        hidden.commit_plan().release.is_empty(),
        "a destroyed buffer gets no release"
    );
}

/// A buffer the app destroys after attaching it but before the commit is not
/// kept for show (D6): the pending attach leaves with it.
#[test]
fn a_destroyed_pending_attach_is_not_kept_for_show() {
    let state = state();
    let buffers = buffers();
    let hidden = Hidden::new(true, Rc::clone(&buffers));
    let buffer = buffer(&state);
    assert_eq!(hidden.on_app_attach(Some(&buffer)), Attach::Hold);

    hidden.forget(buffer.unique_id());

    let plan = hidden.commit_plan();
    assert!(plan.points.is_none(), "the commit carries no buffer");
    assert!(
        buffers.kept_id().is_none(),
        "a destroyed pending attach is not kept"
    );
    assert!(hidden.show_buffer().is_none());
}
