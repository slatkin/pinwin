/*
 * glue.c - pinwin's GTK glue entry points: application activation, the
 * layer-shell surfaces (the visible panel plus its transparent reservation),
 * layout application and the init/start/exit interface main.zig calls (see
 * pinwin.h). The rest of the former single glue.c lives in render.c,
 * images.c, pty.c, input.c and fontconfig.c, with the shared state in
 * glue_internal.h (design D4).
 *
 * Zig cannot @cImport the GTK4 headers (translate-c crashes on the header
 * chain), so every GTK, Pango, cairo, GdkPixbuf and PTY call lives in these
 * C files and pinwin.h deliberately contains no GTK types at all.
 */

#include "glue_internal.h"

#include <gtk4-layer-shell.h>
#include <gio/gio.h>
#include <glib/gstdio.h>

#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

/* ---- state -------------------------------------------------------------- */

int32_t g_cols = 40;
int32_t g_keyboard_mode = PINWIN_KEYBOARD_ON_DEMAND;

char* const* g_argv;

GtkApplication* g_app;
GtkWindow* g_win;
GtkWidget* g_area;

PangoFontDescription* g_font;
PangoFontDescription* g_font_bold;
PangoFontDescription* g_font_italic;
PangoFontDescription* g_font_bold_italic;
int32_t g_ascent;
int32_t g_cell_baseline; /* px from the cell's bottom to the baseline */
double g_nerd_face_w, g_nerd_face_h, g_nerd_face_y;
double g_nerd_icon_h, g_nerd_icon_h_single;
uint8_t g_theme_bg[3] = {0, 0, 0};
uint8_t g_theme_fg[3] = {255, 255, 255};

int32_t g_cell_w;
int32_t g_cell_h;
int32_t g_rows;
int32_t g_grid_cols; /* cols of the last grid pushed to pinwin_size */

int g_pty_fd = -1;
guint g_pty_source;
double g_last_x, g_last_y;
int g_key_layout;
int32_t g_pty_cols = 40;
int32_t g_pty_rows = 24;
int32_t g_pty_xpixel;
int32_t g_pty_ypixel;
int g_spawned;

PinwinLayout g_layout;
GtkWindow* g_reserve;
GdkMonitor* g_monitor; /* the visible panel's original monitor */
int g_layout_latch; /* first-draw monitor resolution pending */

/* Width follows the applied column count. GTK4 has no gtk_window_resize and
 * gtk_window_set_default_size does not move a mapped window, so the drawing
 * area's natural width is the mechanism. The window default size is updated
 * too: GtkWindow treats it as a size floor, so leaving the launch value there
 * would pin the panel at its launch width once it shrinks (design D3). */
static void apply_panel_width(void) {
    if (g_area) gtk_widget_set_size_request(g_area, g_cols * g_cell_w, -1);
    if (g_win) gtk_window_set_default_size(g_win, g_cols * g_cell_w, -1);
}

/* Push the applied layout onto both surfaces in one main-loop turn (design
 * D5): the visible panel gets its side anchor and directional margins, the
 * transparent reservation keeps zero margins and full height on the same
 * side with an explicit exclusive zone of left + panel + right. Never leaves
 * both horizontal anchors set. */
static void apply_layout_surfaces(void) {
    int32_t margin, reservation;

    if (!pinwin_side_geometry(&g_layout, g_cols * g_cell_w, &margin, &reservation))
        return; /* callers validate first; this is belt and braces */

    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_LEFT,
                         g_layout.side == PINWIN_SIDE_LEFT);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_RIGHT,
                         g_layout.side == PINWIN_SIDE_RIGHT);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_TOP, TRUE);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_BOTTOM, TRUE);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_LEFT,
                         g_layout.side == PINWIN_SIDE_LEFT ? margin : 0);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_RIGHT,
                         g_layout.side == PINWIN_SIDE_RIGHT ? margin : 0);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_TOP, g_layout.top);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_BOTTOM, g_layout.bottom);

    if (g_reserve) {
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_LEFT,
                             g_layout.side == PINWIN_SIDE_LEFT);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_RIGHT,
                             g_layout.side == PINWIN_SIDE_RIGHT);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_TOP, TRUE);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_BOTTOM, TRUE);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_LEFT, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_RIGHT, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_TOP, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_BOTTOM, 0);
        gtk_layer_set_exclusive_zone(g_reserve, reservation);
        if (g_monitor) gtk_layer_set_monitor(g_reserve, g_monitor);
    }
    if (g_area) gtk_widget_queue_draw(g_area);
}

/* The instance's currently applied layout. */
void glue_current_layout(PinwinLayout* out) { *out = g_layout; }

/* Geometry inputs for layout validation (options window Apply). */
void glue_layout_metrics(int32_t* cols, int32_t* cell_w, int32_t* cell_h,
                         int32_t* output_w, int32_t* output_h) {
    *cols = g_cols;
    *cell_w = g_cell_w;
    *cell_h = g_cell_h;
    *output_w = 0;
    *output_h = 0;
    if (g_monitor) {
        GdkRectangle geom;
        gdk_monitor_get_geometry(g_monitor, &geom);
        *output_w = geom.width;
        *output_h = geom.height;
    }
}

int glue_publish_layout(const PinwinLayout* layout) {
    if (!g_monitor || !g_win) return PINWIN_GEOM_ERR_METRICS;
    {
        GdkRectangle geom;
        gdk_monitor_get_geometry(g_monitor, &geom);
        /* Validate the staged column count, not the one already applied: a
         * rejected Apply must leave g_cols untouched (design D3). */
        if (pinwin_layout_validate(layout, layout->cols, g_cell_w, g_cell_h,
                                    geom.width, geom.height) != PINWIN_GEOM_OK)
            return PINWIN_GEOM_ERR_METRICS;
    }
    g_layout = *layout;
    g_cols = layout->cols;
    apply_panel_width();
    apply_layout_surfaces();
    apply_size();
    return PINWIN_GEOM_OK;
}

static void on_win_map(GtkWidget* widget, gpointer user_data) {
    (void)user_data;

    /* The visible panel's original monitor is resolved on the first frame
     * (g_layout_latch): at map time the compositor has not yet told the
     * surface which output it is on, so gdk_display_get_monitor_at_surface
     * still reports a fallback. Once resolved, the saved layout is loaded,
     * validated against that monitor and the reservation is pinned to it
     * (design D5). */
    g_layout_latch = 1;

    /* The reservation is created and presented only after the visible panel
     * is mapped; the first draw then pins it to the resolved monitor. */
    {
        GtkApplication* app = GTK_APPLICATION(g_application_get_default());
        GtkWindow* reserve = GTK_WINDOW(gtk_application_window_new(app));
        gtk_layer_init_for_window(reserve);
        gtk_layer_set_namespace(reserve, "pinwin-reserve");
        gtk_layer_set_layer(reserve, GTK_LAYER_SHELL_LAYER_BOTTOM);
        gtk_window_set_default_size(reserve, 1, -1);
        gtk_widget_set_opacity(GTK_WIDGET(reserve), 0.0);
        g_reserve = reserve;
    }

    apply_layout_surfaces();
    gtk_window_present(g_reserve);
    apply_size();
}

/* One-shot, from the first draw after map: by then the surface has entered
 * its output, so the monitor reported here is the panel's real monitor. */
void resolve_layout_monitor(void) {
    g_layout_latch = 0;
    g_monitor = gdk_display_get_monitor_at_surface(
        gdk_display_get_default(),
        gtk_native_get_surface(GTK_NATIVE(g_win)));
    apply_layout_surfaces();
    apply_size();
}

/* ---- activation / lifecycle --------------------------------------------- */

static void on_activate(GtkApplication* app, gpointer user_data) {
    (void)user_data;

    GtkWidget* win = gtk_application_window_new(app);
    g_win = GTK_WINDOW(win);

    gtk_layer_init_for_window(g_win);
    gtk_layer_set_namespace(g_win, "pinwin");
    gtk_layer_set_layer(g_win, GTK_LAYER_SHELL_LAYER_OVERLAY);
    gtk_layer_set_keyboard_mode(g_win, (GtkLayerShellKeyboardMode)g_keyboard_mode);

    cell_metrics_update(win);

    g_area = gtk_drawing_area_new();
    gtk_drawing_area_set_draw_func(GTK_DRAWING_AREA(g_area), on_draw, NULL, NULL);
    g_signal_connect(g_area, "resize", G_CALLBACK(on_area_resize), NULL);
    attach_controllers(g_area);
    gtk_widget_set_focusable(g_area, TRUE);
    gtk_window_set_child(g_win, g_area);

    /* Width is fixed by COLS; the top/bottom anchors give the height. */
    gtk_window_set_default_size(g_win, g_cols * g_cell_w, -1);
    gtk_layer_set_exclusive_zone(g_win, -1); /* ignore Noctalia's top zone */

    /* Initial anchors/margins from the launch baseline; the map callback
     * loads the saved layout, resolves the original monitor and presents the
     * reservation once those are known. */
    apply_layout_surfaces();
    g_signal_connect(win, "map", G_CALLBACK(on_win_map), NULL);

    gtk_window_present(g_win);
    apply_size();
}

int glue_init(const PinwinLayout* layout, int32_t keyboard_mode) {
    char theme_name[128];

    theme_colours(theme_name, sizeof(theme_name), g_theme_bg, g_theme_fg);
    (void)theme_name;
    g_layout = *layout;
    g_cols = layout->cols;
    g_keyboard_mode = keyboard_mode;

    if (!gtk_init_check()) {
        fprintf(stderr, "pinwin: no display\n");
        return 0;
    }
    if (!gtk_layer_is_supported()) {
        fprintf(stderr, "pinwin: compositor does not support wlr-layer-shell\n");
        return 0;
    }

    g_app = gtk_application_new(NULL, G_APPLICATION_DEFAULT_FLAGS);
    if (!g_app) return 0;
    g_signal_connect(g_app, "activate", G_CALLBACK(on_activate), NULL);
    return 1;
}

static gboolean on_tick(gpointer data) {
    (void)data;
    glue_queue_draw();
    return G_SOURCE_CONTINUE;
}

void glue_queue_draw(void) {
    if (g_area) gtk_widget_queue_draw(g_area);
}
