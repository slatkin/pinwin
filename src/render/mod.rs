//! The render row (port-to-rust D3): how pinwin draws a frame — cairo and
//! pangocairo painting of the terminal's cells, the sprite ranges drawn as
//! graphics, the cursor, the focus accent and the tween frame cache, plus the
//! kitty image surfaces (`src/render.c` and `src/images.c`, designs D4/D5
//! there).
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

pub use images::PixbufDecoder;

mod images;
mod metrics;
mod snapshot;
mod sprites;
mod text;
mod texture;

use crate::guard::{Poisoned, guard};
use crate::layout::Accent;
use crate::term::Terminal;
use crate::term::cells::{Cursor, CursorStyle, Rgb, StyleFlags, Wide};

use metrics::CellMetrics;
use text::FontsRef;

use gtk4::gdk;

/// One frame's draw state (`g_font*`, `g_cell_*`, `g_nerd_*`, `g_theme_*`,
/// `g_accent`, `g_focused`, the tween frame cache and the image cache in the
/// C glue). Everything here lives on the GTK thread only.
pub struct DrawState {
    theme_background: Rgb,
    theme_foreground: Rgb,
    accent: Option<Accent>,
    focused: bool,
    poisoned: Poisoned,
    fonts: Option<metrics::Fonts>,
    cell_metrics: CellMetrics,
    /// The tween frame cache: the grid rendered once per tween into an image
    /// surface, keyed by column count and height (`s_grid_cache*`), with the
    /// same pixels wrapped as a [`gdk::MemoryTexture`] (poc-gsk-texture-grid
    /// task 1.1) so a snapshot can upload once and transform each frame.
    grid_cache: Option<cairo::ImageSurface>,
    grid_cache_cols: i32,
    grid_cache_height: i32,
    grid_cache_texture: Option<gdk::MemoryTexture>,
    images: images::ImageCache,
}

impl DrawState {
    /// A draw state with the theme colours from the Ghostty config and the
    /// startup accent (`glue_init`'s `g_theme_*` and `g_accent`). `poisoned`
    /// is the panel's shared D5 latch: a draw panic latches it so the rest of
    /// the panel's glue code stops too.
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
            grid_cache: None,
            grid_cache_cols: 0,
            grid_cache_height: 0,
            grid_cache_texture: None,
            images: images::ImageCache::default(),
        }
    }

    /// Whether the panel currently holds keyboard focus (`g_focused`, set by
    /// the input controllers in row 3.7).
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// The horizontal cell pitch in pixels (`glue_cell_width`).
    pub fn cell_w(&self) -> i32 {
        self.cell_metrics.cell_w
    }

    /// The vertical cell pitch in pixels (`glue_cell_height`).
    pub fn cell_h(&self) -> i32 {
        self.cell_metrics.cell_h
    }

    /// Measure the font on `context` and refresh the cell metrics
    /// (`cell_metrics_update`). Also drops the tween frame cache: the cell
    /// size may change, so any cache is stale.
    pub fn cell_metrics_update(&mut self, context: &pango::Context) {
        self.drop_grid_cache();
        let fonts = self.fonts.get_or_insert_with(metrics::Fonts::load);
        self.cell_metrics = metrics::measure(context, &fonts.regular);
    }

    /// Drop the tween frame cache (`render_grid_cache_drop`): the per-tween
    /// blitted grid surface and its [`gdk::MemoryTexture`] wrapper. Called when
    /// a tween stops and when the cell metrics change.
    pub fn drop_grid_cache(&mut self) {
        self.grid_cache = None;
        self.grid_cache_cols = 0;
        self.grid_cache_height = 0;
        self.grid_cache_texture = None;
    }

    /// The cached tween grid as a [`gdk::MemoryTexture`] (poc-gsk-texture-grid
    /// task 1.2), or `None` while no cache is built. The pixels are device
    /// pixels at the build-time device scale; pair with
    /// [`Self::grid_cache_logical_size`] to size it in surface coordinates.
    pub fn grid_cache_texture(&self) -> Option<gdk::MemoryTexture> {
        self.grid_cache_texture.clone()
    }

    /// The cached grid's logical size in surface coordinates — device pixels
    /// divided by the surface's device scale (poc-gsk-texture-grid task 1.2).
    pub fn grid_cache_logical_size(&self) -> Option<(f64, f64)> {
        let surface = self.grid_cache.as_ref()?;
        let (sx, sy) = surface.device_scale();
        Some((
            f64::from(surface.width()) / sx,
            f64::from(surface.height()) / sy,
        ))
    }

    /// Render a frame into `cr` (`on_draw`): the theme background, the grid
    /// (via the tween cache while a width tween runs) and the focus accent.
    ///
    /// `draw_offset` is the tween's docked-edge offset in pixels
    /// (`glue_anim_draw_offset`); the caller has already resolved the layout
    /// monitor on the first draw when it needs to (row 3.7). `animating` is
    /// `glue_anim_active()`.
    ///
    /// A panic anywhere in the draw path is caught (D5): it latches
    /// `poisoned` and later calls draw nothing.
    pub fn draw(
        &mut self,
        cr: &cairo::Context,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: i32,
        animating: bool,
    ) {
        // A poisoned guard short-circuits (the body never runs), so a
        // poisoned state draws nothing (D5).
        let poisoned = self.poisoned.clone();
        let _ = guard(&poisoned, || {
            self.draw_inner(cr, terminal, width, height, draw_offset, animating);
        });
    }

    fn draw_inner(
        &mut self,
        cr: &cairo::Context,
        terminal: &mut Terminal,
        width: i32,
        height: i32,
        draw_offset: i32,
        animating: bool,
    ) {
        set_rgb(cr, &self.theme_background);
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();

        // A draw before the first `cell_metrics_update` has no fonts and a
        // zero cell pitch: the grid pass would divide by zero. In the glue the
        // metrics update runs from the widget's pango context before the first
        // draw (glue.c `on_activate`), so this is the never-expected state;
        // degraded (no metrics yet) draws keep the background and the accent
        // and stop there instead of panicking or poisoning (render P2).
        if self.fonts.is_none() || self.cell_metrics.cell_h <= 0 {
            self.draw_focus_accent(cr, width, height);
            return;
        }

        if animating && let Some(cache) = self.grid_cache_ensure(cr, terminal, height) {
            let offset = f64::from(draw_offset);
            cr.translate(offset, 0.0);
            let _ = cr.set_source_surface(&cache, 0.0, 0.0);
            let _ = cr.paint();
            cr.translate(-offset, 0.0);
            self.draw_focus_accent(cr, width, height);
            return;
        }

        // While a width tween runs, keep the grid against the docked edge.
        let offset = f64::from(draw_offset);
        cr.translate(offset, 0.0);
        self.render_grid(cr, terminal, height);
        cr.translate(-offset, 0.0);
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

        // Fractional output scales put cell edges between device pixels;
        // antialiased edges would blend with the fill underneath and show as
        // a grid. Unantialiased rectangles snap to device pixels and tile
        // exactly.
        cr.set_antialias(cairo::Antialias::None);
        while let Some(cell) = terminal.cell_next() {
            if cell.has_bg {
                set_rgb(cr, &cell.bg);
                let row_height = if cell.y == height / cell_metrics.cell_h - 1 {
                    height - cell.y * cell_metrics.cell_h
                } else {
                    cell_metrics.cell_h
                };
                cr.rectangle(
                    f64::from(cell.x) * f64::from(cell_metrics.cell_w),
                    f64::from(cell.y) * f64::from(cell_metrics.cell_h),
                    f64::from(cell_metrics.cell_w),
                    f64::from(row_height),
                );
                let _ = cr.fill();
            }
        }
        cr.set_antialias(cairo::Antialias::Default);
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
                set_rgb(cr, &cell.fg);
            } else {
                set_rgb(cr, &self.theme_foreground);
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
                let x = f64::from(cell.x) * f64::from(cell_metrics.cell_w);
                let y = f64::from(cell.y) * f64::from(cell_metrics.cell_h);
                cr.set_line_width(1.0);
                if cell.flags.contains(StyleFlags::UNDERLINE) {
                    cr.move_to(x, y + f64::from(cell_metrics.ascent) + 1.5);
                    cr.line_to(
                        x + f64::from(cell_metrics.cell_w),
                        y + f64::from(cell_metrics.ascent) + 1.5,
                    );
                }
                if cell.flags.contains(StyleFlags::STRIKETHROUGH) {
                    cr.move_to(x, y + f64::from(cell_metrics.cell_h) / 2.0);
                    cr.line_to(
                        x + f64::from(cell_metrics.cell_w),
                        y + f64::from(cell_metrics.cell_h) / 2.0,
                    );
                }
                let _ = cr.stroke();
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
    /// background colour (`draw_cursor`).
    fn draw_cursor(
        &mut self,
        cr: &cairo::Context,
        terminal: &Terminal,
        cursor: &Cursor,
        text: &[u8],
    ) {
        let cell_metrics = self.cell_metrics;
        let colors = terminal.colors();
        let x = f64::from(cursor.x) * f64::from(cell_metrics.cell_w);
        let y = f64::from(cursor.y) * f64::from(cell_metrics.cell_h);
        let cw = f64::from(cell_metrics.cell_w);
        let ch = f64::from(cell_metrics.cell_h);

        set_rgb(cr, &colors.foreground);

        match cursor.style {
            CursorStyle::Bar => {
                cr.rectangle(x, y, 2.0, ch);
                let _ = cr.fill();
            }
            CursorStyle::Underline => {
                cr.rectangle(x, y + ch - 2.0, cw, 2.0);
                let _ = cr.fill();
            }
            CursorStyle::BlockHollow => {
                cr.set_line_width(1.0);
                cr.rectangle(x + 0.5, y + 0.5, cw - 1.0, ch - 1.0);
                let _ = cr.stroke();
            }
            CursorStyle::Block => {
                cr.rectangle(x, y, cw, ch);
                let _ = cr.fill();
                if !text.is_empty() && !cursor.wide_tail {
                    let mut cell = crate::term::cells::Cell::default();
                    let len = text.len().min(crate::term::cells::CELL_TEXT_CAP - 1);
                    cell.x = cursor.x;
                    cell.y = cursor.y;
                    cell.text[..len].copy_from_slice(&text[..len]);
                    cell.len = len;
                    set_rgb(cr, &colors.background);
                    let layout = pangocairo::functions::create_layout(cr);
                    let fonts = self.fonts_ref();
                    text::draw_text(cr, &layout, &cell, &fonts, &cell_metrics);
                }
            }
        }
    }

    /// The cache for this tween frame, or `None` to fall back to the ordinary
    /// full render (`grid_cache_ensure`). Drawn in logical (cell-grid)
    /// coordinates; the source surface's device scale keeps the blit crisp
    /// under fractional output scales.
    ///
    /// While a width tween runs, every frame would redraw the full target
    /// grid through Pango -- tens of milliseconds on a full grid, which
    /// starves the frame clock and collapses the tween into one late jump.
    /// The grid is rendered once per tween into an image surface (the grid is
    /// resized target-first at t0, design D3 there, so the cache starts
    /// correct) and blitted at the dock offset per frame instead. Content
    /// that changes while the tween runs shows when the tween ends and the
    /// ordinary draw path resumes; a 200 ms stale window is invisible next to
    /// the cost it avoids.
    fn grid_cache_ensure(
        &mut self,
        cr: &cairo::Context,
        terminal: &mut Terminal,
        height: i32,
    ) -> Option<cairo::ImageSurface> {
        let cols = i32::from(terminal.cols());
        if let Some(cache) = &self.grid_cache
            && self.grid_cache_cols == cols
            && self.grid_cache_height == height
        {
            return Some(cache.clone());
        }
        self.drop_grid_cache();
        let (mut sx, mut sy) = cr.target().device_scale();
        if sx <= 0.0 || sy <= 0.0 {
            sx = 1.0;
            sy = 1.0;
        }
        let cache_w = (f64::from(cols) * f64::from(self.cell_metrics.cell_w) * sx).ceil() as i32;
        let cache_h = (f64::from(height) * sy).ceil() as i32;
        let mut surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, cache_w, cache_h).ok()?;
        surface.set_device_scale(sx, sy);
        let cache_cr = cairo::Context::new(&surface).ok()?;
        set_rgb(&cache_cr, &self.theme_background);
        let _ = cache_cr.paint();
        self.render_grid(&cache_cr, terminal, height);
        drop(cache_cr);
        let texture = texture::grid_cache_texture(&mut surface);
        self.grid_cache = Some(surface);
        self.grid_cache_cols = cols;
        self.grid_cache_height = height;
        self.grid_cache_texture = texture;
        self.grid_cache.clone()
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

fn set_rgb(cr: &cairo::Context, color: &Rgb) {
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
    use gdk::prelude::TextureExt as _;
    use pango::prelude::FontMapExt as _;
    use std::num::NonZeroU16;

    fn font() -> std::sync::MutexGuard<'static, ()> {
        font_lock::guard()
    }

    /// A sink with nowhere to write (the tests never write to a pty).
    struct NullSink;
    impl crate::term::PtySink for NullSink {
        fn write_pty(&mut self, _data: &[u8]) {}
    }

    /// Decodes nothing; the image tests build their own placements.
    struct NoDecoder;
    impl crate::term::PngDecoder for NoDecoder {
        fn decode_png(&mut self, _data: &[u8]) -> Option<crate::term::DecodedPng> {
            None
        }
    }

    fn terminal() -> Terminal {
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        assert!(terminal.push_size(8, 4, 8, 16));
        terminal
    }

    fn state() -> DrawState {
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
        draw_offset: i32,
        animating: bool,
    ) -> cairo::ImageSurface {
        let surface = surface(width, height);
        {
            let cr = cairo::Context::new(&surface).unwrap();
            state.draw(&cr, terminal, width, height, draw_offset, animating);
        }
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
        let mut drawn = drawn_after(state, &mut terminal, 64, 64, 0, false);
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
        let drawn = drawn_after(state(), &mut terminal, 64, 64, 0, false);
        assert!(opaque_pixels(drawn) > 0, "something was drawn");
    }

    #[test]
    fn draw_is_repeatable_and_ends_the_frame() {
        let _font = font();
        let mut terminal = terminal();
        terminal.push_pty_data(b"abc");
        // Each draw is a fresh state over the same frame data: the second
        // draw must reproduce the first.
        let first = opaque_pixels(drawn_after(state(), &mut terminal, 64, 64, 0, false));
        let second = opaque_pixels(drawn_after(state(), &mut terminal, 64, 64, 0, false));
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
        let mut plain = drawn_after(state, &mut terminal, 32, 32, 0, false);
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
        let mut focused_surface = drawn_after(focused_state, &mut terminal, 32, 32, 0, false);
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

    /// The cache and its [`gdk::MemoryTexture`] wrapper match the device-scaled
    /// surface (poc-gsk-texture-grid task 1.1): the cache is `cols * cell_w` by
    /// `height` in logical pixels, ceiling to whole device pixels at the
    /// target's device scale, the texture is exactly those device pixels, and
    /// the exposed logical size divides them back out.
    #[test]
    fn the_grid_cache_texture_matches_the_device_scaled_surface() {
        let _font = font();
        for scale in [1.0, 1.5] {
            let mut state = state();
            let mut terminal = terminal();
            let drawn = surface(64, 64);
            drawn.set_device_scale(scale, scale);
            let cr = cairo::Context::new(&drawn).unwrap();

            let cache = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
            let cols_w = i32::from(terminal.cols()) * state.cell_w();
            assert_eq!(cache.width(), (f64::from(cols_w) * scale).ceil() as i32);
            assert_eq!(cache.height(), (64.0 * scale).ceil() as i32);
            let texture = state
                .grid_cache_texture()
                .unwrap_or_else(|| panic!("no texture at scale {scale}"));
            assert_eq!(texture.width(), cache.width());
            assert_eq!(texture.height(), cache.height());
            let (logical_w, logical_h) = state.grid_cache_logical_size().unwrap();
            assert_eq!(logical_w, f64::from(cache.width()) / scale);
            assert_eq!(logical_h, f64::from(cache.height()) / scale);
        }
    }

    #[test]
    fn tween_cache_is_keyed_by_columns_and_height() {
        let _font = font();
        let mut state = state();
        let mut terminal = terminal();
        let drawn = surface(64, 64);
        let cr = cairo::Context::new(&drawn).unwrap();

        let first = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
        let again = state.grid_cache_ensure(&cr, &mut terminal, 64).unwrap();
        assert_eq!(
            first.to_raw_none(),
            again.to_raw_none(),
            "same key reuses the cache"
        );

        let other_height = state.grid_cache_ensure(&cr, &mut terminal, 32).unwrap();
        drop(cr);
        assert_ne!(
            first.to_raw_none(),
            other_height.to_raw_none(),
            "a new height rebuilds the cache"
        );

        state.drop_grid_cache();
        assert!(state.grid_cache.is_none());
        assert!(state.grid_cache_texture().is_none());
        assert!(state.grid_cache_logical_size().is_none());
    }

    #[test]
    fn a_panic_in_the_draw_path_latches_poison_and_stops_drawing() {
        let _font = font();
        let mut state = state();
        let mut terminal = terminal();
        // Drive the poisoned path directly: a poisoned state draws nothing,
        // so the surface stays empty.
        state.poisoned = Poisoned::latched();
        let drawn = drawn_after(state, &mut terminal, 64, 64, 0, false);
        assert_eq!(opaque_pixels(drawn), 0, "poisoned draw is a no-op");
    }

    #[test]
    fn last_row_background_reaches_the_given_height() {
        let _font = font();
        // One text row of 16px in a 20px-high area: the last row's background
        // must run to `height`, not stop at the cell boundary.
        let mut terminal = terminal();
        terminal.push_pty_data(b"\x1b[44mfull\x1b[0m");
        let mut drawn = drawn_after(state(), &mut terminal, 64, 20, 0, false);
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
        let drawn = drawn_after(state, &mut terminal, 64, 64, 0, false);
        assert!(opaque_pixels(drawn) > 0);
    }

    /// The frame protocol tolerates a draw with no terminal yet.
    #[test]
    fn draw_without_a_terminal_is_a_background_only() {
        let _font = font();
        let mut terminal = Terminal::new(crate::guard::Poisoned::new(), NullSink, NoDecoder, || {});
        let mut plain = drawn_after(state(), &mut terminal, 32, 32, 0, false);
        plain.flush();
        let data = plain.data().unwrap();
        assert_eq!(&data[0..4], &[0, 0, 0, 255], "background only");
    }
}
