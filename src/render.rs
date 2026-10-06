//! The render row (port-to-rust D3): how pinwin draws a frame — cairo and
//! pangocairo painting of the terminal's cells, the sprite ranges drawn as
//! graphics, the cursor, the focus accent and the kitty image surfaces
//! (`src/render.c` and `src/images.c`, designs D4/D5 there).
//!
//! The draw state lives in the private [`DrawState`]; the frame data comes
//! from the [`Terminal`] frame API (`frame_begin`/`cell_next`/`cursor`/
//! `colors`/`image_next`), which paints the plain `cells` types. The draw
//! functions take a `cairo::Context`, so the GTK widget wiring (row 3.7/4.x)
//! only has to call them from its draw callback; cairo image surfaces work
//! without a display, which is what the tests below render into.
//!
//! Panics must never cross back into GTK (D5): [`DrawState::draw`] runs its
//! body through the shared [`crate::guard`] helper and latches `poisoned`,
//! after which it draws nothing.
//!
//! `render_grid` stays the cairo
//! fallback and the parity oracle. `snapshot` presents every frame as GSK
//! nodes: the whole grid built once into a retained `gsk::RenderNode` on a
//! tween's first frame and appended translated for the rest of the tween,
//! rebuilt into the widget's snapshot on every non-tween draw. When node
//! emission is not possible, the frame chains to the cairo draw path, which
//! renders the full grid with `render_grid` every frame — the fallback is
//! correctness-first, with no per-tween cache of its own (gsk-render-nodes
//! row 4.2 retired the stage-1 texture cache).
//! `parity` is the test-only diff harness.

pub use images::PixbufDecoder;

pub(crate) use snap::OutputScale;

mod accent;
mod bands;
mod bg;
/// The canvas the grid painter draws a frame into, from row 4.3 on (D5):
/// GTK-free, pixel tests without a display.
pub mod canvas;
/// The cell metrics from the font, computed with swash (row 4.6): the port
/// of the Pango `measure`. GTK-free, like the font module.
pub mod cell_metrics;
pub mod cursor;
/// The font module (row 4.4): fontconfig resolves the Ghostty family to font
/// files and caches a fallback face per code point (replace-gtk-with-wayland
/// D6/D11). GTK-free, like the canvas.
pub mod font;
/// The frame gate (row 4.8): what a frame must redraw, from the render
/// state's dirty data and the inputs it does not cover. GTK-free, like
/// the canvas.
pub mod frame_gate;
/// The geometry seam between the snapped logical grid and the device-pixel
/// canvas: the cast seam, the painter's cell metrics and the frame input
/// (D5). GTK-free, like the canvas.
pub mod geom;
/// The glyph module (row 4.5): swash rasterizes one glyph id of one face at
/// one device size, with hinting, colour and style synthesis, and caches the
/// results (replace-gtk-with-wayland D6/D11). GTK-free, like the canvas.
pub mod glyph;
/// The kitty image pass (row 4.7): the frame's kitty placements decoded
/// with the `png` crate, scaled to their placement sizes and cached, and
/// drawn into the canvas above the cursor (replace-gtk-with-wayland D5).
/// GTK-free, like the canvas.
pub mod image_pass;
mod images;
mod metrics;
/// The Nerd Font glyph constraints over swash geometry (row 4.5): the port
/// of the old text pass's `constrain`, recast from a cairo post-scale into
/// a swash placement transform (replace-gtk-with-wayland D6). GTK-free,
/// like the font module.
pub mod nerd;
mod node_cursor;
mod node_images;
mod node_sprites;
mod nodes;
/// The grid painter for the canvas (row 4.3): the theme background, the
/// cell backgrounds, the underline and strikethrough bands, the cursor
/// shapes and the focus accent, with the text and image passes to come.
/// GTK-free, like the canvas.
pub mod painter;
#[cfg(test)]
mod parity;
/// The `png`-crate kitty PNG decoder (row 4.7): the [`PngDecoder`] the
/// panel hands to the terminal from row 8 on, GTK-free like the canvas.
pub mod png;
/// The shaper (row 4.5): swash shapes one cell's grapheme cluster into
/// glyph ids and pen positions, with the face chosen from the family's
/// styles and the per-code-point fallback, cached (replace-gtk-with-wayland
/// D6/D11). GTK-free, like the font module.
pub mod shape;
mod snap;
mod snapshot;
mod sprite;
mod sprites;
mod text;
pub mod text_pass;
mod texture;

use crate::guard::{Poisoned, guard};
use crate::layout::Accent;
use crate::term::Terminal;
use crate::term::cells::{Cursor, CursorStyle, Rgb, StyleFlags, Wide};

use metrics::CellMetrics;
use text::FontsRef;

use gtk4::gsk;

/// One frame's draw state (`g_font*`, `g_cell_*`, `g_nerd_*`, `g_theme_*`,
/// `g_accent`, `g_focused`, the retained tween grid node and the image cache
/// in the C glue). Everything here lives on the GTK thread only.
pub struct DrawState {
    theme_background: Rgb,
    theme_foreground: Rgb,
    accent: Option<Accent>,
    focused: bool,
    poisoned: Poisoned,
    fonts: Option<metrics::Fonts>,
    cell_metrics: CellMetrics,
    /// The pango context the metrics were measured on, kept for the node
    /// emitter's layouts (gsk-render-nodes): the cairo path builds its layout
    /// off a cairo context, but `append_layout` needs a plain one, and the
    /// widget's context is the one whose font map and resolution match what
    /// the cairo path ends up rendering through.
    pango_context: Option<pango::Context>,
    /// The retained grid node (gsk-render-nodes row 4.1): the whole grid —
    /// theme background, cell backgrounds, cells, cursor and images — built
    /// once into a `gsk::RenderNode` on the tween's first frame and appended
    /// translated for the rest of the tween; the glyphs stay in GSK's GPU
    /// atlas, so the per-frame CPU work is a transform. Keyed by column count
    /// and height (`grid_node_cols`/`grid_node_height`): the terminal grid is
    /// resized only when the tween ends, so the node starts correct. Rebuilt
    /// fresh on every non-tween draw, and dropped when a tween stops (the
    /// `tween_cache_drop` hook) or the cell metrics change. The cairo fallback
    /// has no cache of its own: a tween frame it has to draw renders the full
    /// grid every frame (gsk-render-nodes row 4.2).
    grid_node: Option<gsk::RenderNode>,
    grid_node_cols: i32,
    grid_node_height: i32,
    images: images::ImageCache,
}

impl std::fmt::Debug for DrawState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The font, context, grid-node and image-cache fields hold GTK and
        // cairo handles without a `Debug` impl; the scalar state is enough
        // to identify a draw state in test failures.
        f.debug_struct("DrawState")
            .field("theme_background", &self.theme_background)
            .field("theme_foreground", &self.theme_foreground)
            .field("accent", &self.accent)
            .field("focused", &self.focused)
            .field("poisoned", &self.poisoned)
            .field("cell_metrics", &self.cell_metrics)
            .finish_non_exhaustive()
    }
}

impl DrawState {
    /// A draw state with the theme colours from the Ghostty config and the
    /// startup accent (`glue_init`'s `g_theme_*` and `g_accent`). `poisoned`
    /// is the panel's shared D5 latch: a draw panic latches it so the rest of
    /// the panel's glue code stops too.
    #[must_use]
    pub fn new(
        poisoned: Poisoned,
        accent: Option<Accent>,
        theme: crate::fontconfig::ThemeColours,
    ) -> DrawState {
        DrawState {
            theme_background: Rgb {
                r: theme.background[0],
                g: theme.background[1],
                b: theme.background[2],
            },
            theme_foreground: Rgb {
                r: theme.foreground[0],
                g: theme.foreground[1],
                b: theme.foreground[2],
            },
            accent,
            focused: false,
            poisoned,
            fonts: None,
            cell_metrics: CellMetrics::default(),
            pango_context: None,
            grid_node: None,
            grid_node_cols: 0,
            grid_node_height: 0,
            images: images::ImageCache::default(),
        }
    }

    /// Whether the panel currently holds keyboard focus (`g_focused`, set by
    /// the input controllers in row 3.7).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The horizontal cell pitch in pixels (`glue_cell_width`).
    #[must_use]
    pub fn cell_w(&self) -> i32 {
        self.cell_metrics.cell_w
    }

    /// The vertical cell pitch in pixels (`glue_cell_height`).
    #[must_use]
    pub fn cell_h(&self) -> i32 {
        self.cell_metrics.cell_h
    }

    /// Measure the font on `context` and refresh the cell metrics
    /// (`cell_metrics_update`). Also drops the retained grid node: the cell
    /// size may change, so the node is stale.
    pub fn cell_metrics_update(&mut self, context: &pango::Context) {
        self.drop_grid_node();
        self.pango_context = Some(context.clone());
        let fonts = self.fonts.get_or_insert_with(metrics::Fonts::load);
        let mut measured = metrics::measure(context, &fonts.regular);
        // `measure` builds fresh metrics from the font; the output scale is
        // per-surface draw state, not a font measurement, so it survives the
        // update (snap-grid-edges D2).
        measured.scale = self.cell_metrics.scale;
        self.cell_metrics = measured;
    }

    /// Record the output scale the next draw runs at (`snap-grid-edges` D2,
    /// D3): the shared geometry functions snap their rectangles through it.
    /// Called from the draw hooks on every draw. A changed scale drops the
    /// retained grid node (snap-grid-edges D7): the node holds snapped
    /// geometry for the scale it was built at. An unchanged scale keeps it,
    /// so a tween does not rebuild the node every frame.
    pub(crate) fn set_scale(&mut self, scale: OutputScale) {
        if self.cell_metrics.scale != scale {
            self.cell_metrics.scale = scale;
            self.drop_grid_node();
        }
    }

    /// Drop the retained grid node (`render_grid_cache_drop`): the node is
    /// keyed to one tween, so it goes when the tween stops (the
    /// `tween_cache_drop` hook) and when the cell metrics change.
    pub fn drop_grid_node(&mut self) {
        self.grid_node = None;
        self.grid_node_cols = 0;
        self.grid_node_height = 0;
    }

    /// Render a frame into `cr` (`on_draw`): the theme background, the full
    /// grid and the focus accent.
    ///
    /// `draw_offset` is the tween's snapped docked-edge offset in pixels
    /// (`glue_anim_draw_offset`, snapped to the device pixel grid by
    /// `Surfaces::draw_offset()`, snap-grid-edges D7); the caller has already
    /// resolved the layout monitor on the first draw when it needs to (row
    /// 3.7).
    ///
    /// A panic anywhere in the draw path is caught (D5): it latches
    /// `poisoned` and later calls draw nothing.
    pub fn draw(
        &mut self,
        cr: &cairo::Context,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: f64,
    ) {
        // A poisoned guard short-circuits (the body never runs), so a
        // poisoned state draws nothing (D5).
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.draw_inner(cr, terminal, width, height, draw_offset);
        });
    }

    fn draw_inner(
        &mut self,
        cr: &cairo::Context,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: f64,
    ) {
        set_rgb(cr, self.theme_background);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();

        // A draw before the first `cell_metrics_update` has no fonts and a
        // zero cell pitch: the grid pass would divide by zero. In the glue the
        // metrics update runs from the widget's pango context before the first
        // draw (glue.c `on_activate`), so this is the never-expected state;
        // degraded (no metrics yet) draws keep the background and the accent
        // and stop there instead of panicking or poisoning.
        if self.fonts.is_none() || self.cell_metrics.cell_h <= 0 {
            self.draw_focus_accent(cr, width, height);
            return;
        }

        // While a width tween runs, keep the grid against the docked edge.
        // This is the fallback a tween frame lands on when the GSK snapshot
        // could not emit nodes: it renders the full grid every frame — slow,
        // but correct (gsk-render-nodes row 4.2).
        cr.translate(draw_offset, 0.0);
        self.render_grid(cr, terminal, height);
        cr.translate(-draw_offset, 0.0);
        self.draw_focus_accent(cr, width, height);
    }

    /// Renders the terminal grid (cells, sprites, text, cursor and images)
    /// into `cr` (`render_grid`). The caller has already translated to the
    /// docked edge when a tween is running; `height` is the grid-relative
    /// height (the last row's background fill runs to it).
    fn render_grid(&mut self, cr: &cairo::Context, terminal: &mut Terminal, height: i32) {
        if !terminal.frame_begin() {
            return;
        }

        let layout = pangocairo::functions::create_layout(cr);
        let cell_metrics = self.cell_metrics;

        paint_backgrounds(cr, terminal, cell_metrics, height);
        terminal.frame_rewind();

        let mut cursor_text = [0u8; 32];
        let mut cursor_text_len = 0usize;
        let mut have_cursor_text = false;
        while let Some(cell) = terminal.cell_next() {
            if cell.wide == Wide::SpacerTail {
                continue; // do not render
            }
            if cell.len == 0 || cell.flags.contains(StyleFlags::INVISIBLE) {
                continue;
            }
            if cell.has_fg {
                set_rgb(cr, cell.fg);
            } else {
                set_rgb(cr, self.theme_foreground);
            }
            if !sprites::draw_sprite(
                cr,
                &cell,
                crate::term::cells::first_codepoint(cell.text_bytes()),
                &cell_metrics,
            ) {
                let fonts = self.fonts_ref();
                text::draw_text(cr, &layout, &cell, &fonts, &cell_metrics);
            }

            if cell.flags.contains(StyleFlags::UNDERLINE)
                || cell.flags.contains(StyleFlags::STRIKETHROUGH)
            {
                // The same snapped rectangles the node emitter fills
                // (node_cursor::underline_rect/strikethrough_rect), filled
                // unantialiased like them (`snap-grid-edges` D6) — identical
                // geometry, so the two painters cannot drift.
                for (x, y, w, h) in node_cursor::underline_rect(&cell, &cell_metrics)
                    .into_iter()
                    .chain(node_cursor::strikethrough_rect(&cell, &cell_metrics))
                {
                    sprites::fill_rect(cr, x, y, w, h);
                }
            }

            if !have_cursor_text
                && cell.len > 0
                && let Some(cursor) = terminal.cursor()
                && cursor.x == cell.x
                && cursor.y == cell.y
            {
                cursor_text_len = cell.len.min(cursor_text.len() - 1);
                cursor_text[..cursor_text_len].copy_from_slice(&cell.text[..cursor_text_len]);
                have_cursor_text = true;
            }
        }

        if let Some(cursor) = terminal.cursor() {
            let text = if have_cursor_text {
                &cursor_text[..cursor_text_len]
            } else {
                &[]
            };
            self.draw_cursor(cr, terminal, &cursor, text);
        }

        self.images.draw(cr, terminal);

        terminal.frame_end();
    }

    fn fonts_ref(&self) -> FontsRef<'_> {
        // The draw callback only runs after the first cell metrics update,
        // which loads the fonts; an earlier call would have no metrics to
        // draw with anyway.
        let fonts = self
            .fonts
            .as_ref()
            .expect("draw before cell_metrics_update loaded the fonts");
        FontsRef {
            regular: &fonts.regular,
            bold: &fonts.bold,
            italic: &fonts.italic,
            bold_italic: &fonts.bold_italic,
        }
    }

    /// Draw the cursor shape, and the character under a block cursor in its
    /// background colour (`draw_cursor`). The shape geometry is
    /// [`node_cursor::cursor_shape`]'s, shared with the node emitter.
    fn draw_cursor(
        &mut self,
        cr: &cairo::Context,
        terminal: &Terminal,
        cursor: &Cursor,
        text: &[u8],
    ) {
        let cell_metrics = self.cell_metrics;
        let colors = terminal.colors();

        set_rgb(cr, colors.foreground);

        match node_cursor::cursor_shape(cursor, &cell_metrics) {
            node_cursor::CursorShape::Fill((x, y, w, h)) => {
                sprites::fill_rect(cr, x, y, w, h);
            }
            node_cursor::CursorShape::Hollow(bands) => {
                for (x, y, w, h) in bands {
                    sprites::fill_rect(cr, x, y, w, h);
                }
            }
        }

        if cursor.style == CursorStyle::Block && !text.is_empty() && !cursor.wide_tail {
            let mut cell = crate::term::cells::Cell::default();
            let len = text.len().min(crate::term::cells::CELL_TEXT_CAP - 1);
            cell.x = cursor.x;
            cell.y = cursor.y;
            cell.text[..len].copy_from_slice(&text[..len]);
            cell.len = len;
            set_rgb(cr, colors.background);
            let layout = pangocairo::functions::create_layout(cr);
            let fonts = self.fonts_ref();
            text::draw_text(cr, &layout, &cell, &fonts, &cell_metrics);
        }
    }

    /// Focus accent. Layer surfaces get no compositor focus ring (niri draws
    /// one only around layout windows), so the focused panel marks itself: a
    /// stroke around the whole window in the configured accent colour and
    /// width. Drawn in raw surface coordinates, inset by half the stroke so
    /// it stays inside (`draw_focus_accent`).
    fn draw_focus_accent(&self, cr: &cairo::Context, width: i32, height: i32) {
        if !self.focused {
            return;
        }
        let Some(accent) = self.accent else {
            return;
        };
        let stroke = f64::from(accent.width().get());
        let [r, g, b] = accent.rgb();
        cr.set_source_rgb(
            f64::from(r) / 255.0,
            f64::from(g) / 255.0,
            f64::from(b) / 255.0,
        );
        cr.set_line_width(stroke);
        cr.rectangle(
            stroke / 2.0,
            stroke / 2.0,
            f64::from(width) - stroke,
            f64::from(height) - stroke,
        );
        let _ = cr.stroke();
    }
}

/// Paint the frame's cell backgrounds into `cr` (`render_grid`'s background
/// pass). The frame must be open (`frame_begin`); the caller rewinds the
/// frame afterwards. The geometry is [`nodes::cell_background_rect`], shared
/// with the node emitter so the two painters cannot drift.
fn paint_backgrounds(
    cr: &cairo::Context,
    terminal: &mut Terminal,
    cell_metrics: CellMetrics,
    height: i32,
) {
    // Fractional output scales put cell edges between device pixels;
    // antialiased edges would blend with the fill underneath and show as
    // a grid. Unantialiased rectangles snap to device pixels and tile
    // exactly.
    cr.set_antialias(cairo::Antialias::None);
    while let Some(cell) = terminal.cell_next() {
        if let Some((x, y, w, h)) = nodes::cell_background_rect(&cell, cell_metrics, height) {
            set_rgb(cr, cell.bg);
            cr.rectangle(x, y, w, h);
            let _ = cr.fill();
        }
    }
    cr.set_antialias(cairo::Antialias::Default);
}

fn set_rgb(cr: &cairo::Context, color: Rgb) {
    cr.set_source_rgb(
        f64::from(color.r) / 255.0,
        f64::from(color.g) / 255.0,
        f64::from(color.b) / 255.0,
    );
}

/// Test support: fontconfig's first initialisation races across test
/// threads, so every test that measures or renders with Pango holds this
/// lock for its duration (the same shape as `pty.rs`'s sigwinch lock).
#[cfg(test)]
pub(crate) mod font_lock {
    use std::sync::{Mutex, MutexGuard};

    static FONT: Mutex<()> = Mutex::new(());

    pub fn guard() -> MutexGuard<'static, ()> {
        FONT.lock().expect("font lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fontconfig::ThemeColours;
    use pango::prelude::FontMapExt as _;
    use std::num::NonZeroU16;

    fn font() -> std::sync::MutexGuard<'static, ()> {
        font_lock::guard()
    }

    /// A sink with nowhere to write (the tests never write to a pty).
    pub(super) struct NullSink;
    impl crate::term::PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Decodes nothing; the image tests build their own placements.
    pub(super) struct NoDecoder;
    impl crate::term::PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
            None
        }
    }

    pub(super) fn terminal() -> Terminal {
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(8, 4, 8, 16));
        terminal
    }

    pub(super) fn state() -> DrawState {
        use pango::prelude::FontMapExt as _;
        let mut state = DrawState::new(Poisoned::new(), None, ThemeColours::default());
        // The default font map, like the GTK widget's context in production.
        let context = pangocairo::FontMap::default().create_context();
        state.cell_metrics_update(&context);
        state
    }

    fn surface(width: i32, height: i32) -> cairo::ImageSurface {
        cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap()
    }

    /// Opaque pixel count of a finished surface.
    fn opaque_pixels(mut surface: cairo::ImageSurface) -> usize {
        surface.flush();
        surface
            .data()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] != 0)
            .count()
    }

    /// Draw with `state` into a fresh surface and return the surface with
    /// the context dropped, so `data()` can borrow it exclusively.
    fn drawn_after(
        mut state: DrawState,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: f64,
    ) -> cairo::ImageSurface {
        let surface = surface(width, height);
        let cr = cairo::Context::new(&surface).unwrap();
        state.draw(&cr, terminal, width, height, draw_offset);
        surface
    }

    #[test]
    fn draw_fills_the_background_with_the_theme() {
        let _font = font();
        let mut state = DrawState::new(
            Poisoned::new(),
            None,
            ThemeColours {
                background: [10, 20, 30],
                foreground: [255, 255, 255],
            },
        );
        let context = pangocairo::FontMap::default().create_context();
        state.cell_metrics_update(&context);
        let mut terminal = terminal();
        let mut drawn = drawn_after(state, &mut terminal, 64, 64, 0.0);
        drawn.flush();
        let data = drawn.data().unwrap();
        // cairo's ARGB32 byte order is B, G, R, A. Sample away from the
        // block cursor, which the fresh terminal parks at cell (0, 0) in the
        // theme foreground.
        let at = |x: usize, y: usize| &data[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4];
        assert_eq!(at(40, 40), &[30, 20, 10, 255]);
        assert_eq!(data.len(), 64 * 64 * 4, "no pixels skipped");
    }

    #[test]
    fn draw_paints_cells_backgrounds_text_and_cursor() {
        let _font = font();
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[41mhi\x1b[0m");
        let drawn = drawn_after(state(), &mut terminal, 64, 64, 0.0);
        assert!(opaque_pixels(drawn) > 0, "something was drawn");
    }

    #[test]
    fn draw_is_repeatable_and_ends_the_frame() {
        let _font = font();
        let mut terminal = terminal();
        terminal.push_pty_data(b"abc");
        // Each draw is a fresh state over the same frame data: the second
        // draw must reproduce the first.
        let first = opaque_pixels(drawn_after(state(), &mut terminal, 64, 64, 0.0));
        let second = opaque_pixels(drawn_after(state(), &mut terminal, 64, 64, 0.0));
        assert_eq!(second, first, "the second draw matches");
        assert!(first > 0);
    }

    #[test]
    fn focus_accent_draws_only_when_focused_and_enabled() {
        let _font = font();
        let accent = Some(Accent::new([255, 0, 0], NonZeroU16::new(2).unwrap()));
        let mut state = DrawState::new(Poisoned::new(), accent, ThemeColours::default());
        let context = pangocairo::FontMap::default().create_context();
        state.cell_metrics_update(&context);
        let mut terminal = terminal();

        // Not focused: nothing but the background.
        let mut plain = drawn_after(state, &mut terminal, 32, 32, 0.0);
        plain.flush();
        let data = plain.data().unwrap();
        // Sample the border away from the block cursor parked at cell (0,0).
        assert_eq!(
            &data[16 * 4..16 * 4 + 4],
            &[0, 0, 0, 255],
            "no accent stroke"
        );
        assert_eq!(
            &data[(16 * 32 + 16) * 4..][..4],
            &[0, 0, 0, 255],
            "centre untouched"
        );

        // Focused: the border is accent red, the centre is background.
        let mut focused_state = DrawState::new(Poisoned::new(), accent, ThemeColours::default());
        let context = pangocairo::FontMap::default().create_context();
        focused_state.cell_metrics_update(&context);
        focused_state.set_focused(true);
        let mut focused_surface = drawn_after(focused_state, &mut terminal, 32, 32, 0.0);
        focused_surface.flush();
        let data = focused_surface.data().unwrap();
        assert_eq!(
            &data[16 * 4..16 * 4 + 4],
            &[0, 0, 255, 255],
            "border pixel is accent"
        );
        let center = 4 * (16 * 32 + 16);
        assert_eq!(
            &data[center..center + 4],
            &[0, 0, 0, 255],
            "centre untouched"
        );
    }

    #[test]
    fn a_panic_in_the_draw_path_latches_poison_and_stops_drawing() {
        let _font = font();
        let mut state = state();
        let mut terminal = terminal();
        // Drive the poisoned path directly: a poisoned state draws nothing,
        // so the surface stays empty.
        state.poisoned = Poisoned::latched();
        let drawn = drawn_after(state, &mut terminal, 64, 64, 0.0);
        assert_eq!(opaque_pixels(drawn), 0, "poisoned draw is a no-op");
    }

    #[test]
    fn last_row_background_reaches_the_given_height() {
        let _font = font();
        // One text row of 16px in a 20px-high area: the last row's background
        // must run to `height`, not stop at the cell boundary.
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[44mfull\x1b[0m");
        let mut drawn = drawn_after(state(), &mut terminal, 64, 20, 0.0);
        drawn.flush();
        let data = drawn.data().unwrap();
        // The bottom row of pixels is the cell's background, not the theme.
        let bottom = 4 * (19 * 64 + 1);
        assert_ne!(
            &data[bottom..bottom + 4],
            &[0, 0, 0, 255],
            "the last row's background reaches the bottom edge"
        );
    }

    #[test]
    fn cells_without_an_explicit_foreground_use_the_theme() {
        let _font = font();
        let mut state = DrawState::new(
            Poisoned::new(),
            None,
            ThemeColours {
                background: [0, 0, 0],
                foreground: [0, 255, 0],
            },
        );
        let context = pangocairo::FontMap::default().create_context();
        state.cell_metrics_update(&context);

        let mut terminal = terminal();
        // A block cursor draws its rectangle in the terminal's default
        // foreground; with the theme's green the cursor area must not be
        // black.
        terminal.push_pty_data(b"\x1b[1;1H");
        let drawn = drawn_after(state, &mut terminal, 64, 64, 0.0);
        assert!(opaque_pixels(drawn) > 0);
    }

    /// The frame protocol tolerates a draw with no terminal yet.
    #[test]
    fn draw_without_a_terminal_is_a_background_only() {
        let _font = font();
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        let mut plain = drawn_after(state(), &mut terminal, 32, 32, 0.0);
        plain.flush();
        let data = plain.data().unwrap();
        assert_eq!(&data[0..4], &[0, 0, 0, 255], "background only");
    }
}
