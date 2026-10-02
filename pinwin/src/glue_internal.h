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

extern char* const* g_argv;

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
extern int32_t g_pty_xpixel;
extern int32_t g_pty_ypixel;
extern int g_spawned;

/* Layout settings. The applied layout is the startup layout supplied to
 * glue_init and is replaced by a later publish. */
extern PinwinLayout g_layout;
extern GtkWindow* g_reserve;
extern GdkMonitor* g_monitor; /* the visible panel's original monitor */
extern int g_layout_latch; /* first-draw monitor resolution pending */

/* ---- fontconfig.c: Ghostty config, theme colours, terminfo check -------- */

int terminfo_exists(const char* name);
void theme_colours(char* theme_name, size_t theme_name_len, uint8_t* bg,
                   uint8_t* fg);
void font_config_load(char** family, double* size);

/* ---- render.c: cell metrics, text, sprite and nerd-font drawing --------- */

void cell_metrics_update(GtkWidget* widget);
void on_draw(GtkDrawingArea* area, cairo_t* cr, int width, int height,
             gpointer user_data);

/* ---- images.c: kitty image surfaces and cache ---------------------------- */

void draw_images(cairo_t* cr);

/* ---- pty.c: spawn, read, write and resize -------------------------------- */

void apply_size(void);
void on_area_resize(GtkWidget* widget, gint width, gint height,
                    gpointer user_data);

/* ---- input.c: GDK controllers for key, mouse, scroll and focus ----------- */

void attach_controllers(GtkWidget* area);

/* ---- glue.c: layer-shell surfaces and layout application ----------------- */

int glue_init(const PinwinLayout* layout, int32_t keyboard_mode);
void resolve_layout_monitor(void);

#endif /* PINWIN_GLUE_INTERNAL_H */
