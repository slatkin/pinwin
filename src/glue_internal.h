/*
 * glue_internal.h - state and cross-file declarations shared by pinwin's C
 * glue files (glue.c, render.c, images.c, pty.c, input.c, fontconfig.c),
 * split from one glue.c along responsibility seams (design D4).
 *
 * Internal to the C side: the Zig side only ever sees pinwin.h, which holds
 * no GTK types (see pinwin.h for why).
 */
#ifndef PINWIN_GLUE_INTERNAL_H
#define PINWIN_GLUE_INTERNAL_H

#include "pinwin.h"
#include "options.h"

#include <pango/pangocairo.h>
#include <gtk/gtk.h>

/* ---- state (definitions in glue.c) --------------------------------------- */

extern int32_t g_cols;
extern int32_t g_keyboard_mode;

extern GtkApplication* g_app;
extern GtkWindow* g_win;
extern GtkWidget* g_area;

extern PangoFontDescription* g_font;
extern PangoFontDescription* g_font_bold;
extern PangoFontDescription* g_font_italic;
extern PangoFontDescription* g_font_bold_italic;
extern int32_t g_ascent;
/* Grid metrics in Ghostty's terms (see src/font/Metrics.zig in the pinned
 * commit); the Nerd Font constraints are expressed against these. */
extern int32_t g_cell_baseline; /* px from the cell's bottom to the baseline */
extern double g_nerd_face_w, g_nerd_face_h, g_nerd_face_y;
extern double g_nerd_icon_h, g_nerd_icon_h_single;
/* Terminal default colours from the Ghostty config/theme. */
extern uint8_t g_theme_bg[3];
extern uint8_t g_theme_fg[3];

extern int32_t g_cell_w;
extern int32_t g_cell_h;
extern int32_t g_rows;
extern int32_t g_grid_cols; /* cols of the last grid pushed to pinwin_size */

extern int g_pty_fd;
extern guint g_pty_source;
extern double g_last_x, g_last_y;
extern int g_key_layout;
extern int32_t g_pty_cols;
extern int32_t g_pty_rows;
extern int g_attached;

/* Layout settings. The applied layout is the startup layout supplied to
 * glue_init and is replaced by a later publish. */
extern PinwinLayout g_layout;
extern GtkWindow* g_reserve;
extern GdkMonitor* g_monitor; /* the visible panel's original monitor */
extern int g_layout_latch; /* first-draw monitor resolution pending */

/* ---- fontconfig.c: Ghostty config and theme colours --------------------- */

void theme_colours(uint8_t* bg, uint8_t* fg);
void font_config_load(char** family, double* size);

/* ---- render.c: cell metrics, text, sprite and nerd-font drawing --------- */

void cell_metrics_update(GtkWidget* widget);
void on_draw(GtkDrawingArea* area, cairo_t* cr, int width, int height,
             gpointer user_data);

/* ---- images.c: kitty image surfaces and cache ---------------------------- */

void draw_images(cairo_t* cr);

/* ---- pty.c: attach, read, write and resize ------------------------------- */

/* Recompute the grid from the allocated height and push it through
 * pinwin_size. Returns 0 when the grid is current (including nothing to do);
 * nonzero when the terminal could not be allocated, in which case the previous
 * grid and PTY winsize are left untouched (design D3). */
int apply_size(void);
void on_area_resize(GtkWidget* widget, gint width, gint height,
                    gpointer user_data);

/* ---- input.c: GDK controllers for key, mouse, scroll and focus ----------- */

void attach_controllers(GtkWidget* area);

/* ---- glue.c: layer-shell surfaces and layout application ----------------- */

int glue_init(const PinwinLayout* layout, int32_t keyboard_mode);
/* Push the current panel width (panel_px) onto both surfaces and queue a draw. */
void glue_apply_geometry(void);
void resolve_layout_monitor(void);

/* Close both layer-shell surfaces (GTK thread). Called by pinwin_stop's
 * teardown and by gtk_thread_main after a failed start quits the loop. */
void glue_close_surfaces(void);

/* Validate `layout` against the panel's original monitor and live cell
 * metrics, then publish it to both surfaces, resize the terminal grid through
 * the normal resize path and force a redraw. Runs on the GTK thread, driven by
 * pinwin_apply_layout (design D4/D5). Returns PINWIN_GEOM_OK, a PINWIN_GEOM_ERR_*
 * verdict without touching live state, GLUE_NOT_LIVE when the panel has no
 * metrics yet (not activated, or already torn down), or GLUE_ERR_TERMINAL when
 * the terminal grid could not be allocated (previous grid kept). */
#define GLUE_NOT_LIVE (-1)
/* The layout published but its terminal grid could not be allocated; the
 * previous grid stays (design D3). Maps to PINWIN_ERR_INTERNAL. */
#define GLUE_ERR_TERMINAL (-2)
int glue_publish_layout(const PinwinLayout* layout, uint32_t duration_ms);

/* ---- glue_anim.c: animated width transition ------------------------------ */

/* The panel's current pixel width: the animated width while a tween runs, else
 * g_cols * g_cell_w. */
int32_t panel_px(void);
int glue_anim_active(void);
/* False when gtk-enable-animations is off. */
int glue_anim_allowed(void);
void glue_anim_begin(int32_t from_px, int32_t to_px, uint32_t duration_ms);
void glue_anim_cancel(void);
/* Grid shift for a right-docked panel while animating, else 0. */
int32_t glue_anim_draw_offset(void);

/* ---- pinwin_api.c: start-up handshake (called from glue.c) --------------- */

/* Completes pinwin_start's handshake: 1 once the panel is activated and its
 * live metrics exist (design D2/D5), 0 when the GTK side failed to come up.
 * Only the first result counts. */
void pinwin_api_start_result(int ok);

#endif /* PINWIN_GLUE_INTERNAL_H */
