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
#include "control.h"
#include "tray.h"

#include <gtk4-layer-shell.h>
#include <gio/gio.h>
#include <glib/gstdio.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* ---- state -------------------------------------------------------------- */

int32_t g_cols = 40;
int32_t g_gutter = 0;
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
int g_no_tray;
char g_control_socket_path[256];

/* ---- control socket (add-pinwin-control) -------------------------------- */

static GSocketService* g_control_service;

/* One client connection, read asynchronously up to 256 bytes or the first
 * '\n' (design D1): the request is parsed, one reply line is written and the
 * connection is closed. A connection that closes or overflows before a
 * complete line is dropped without effect. */
typedef struct {
    GSocketConnection* connection;
    GInputStream* in;
    GOutputStream* out;
    char buf[256];
    gsize len;
} ControlConnection;

static void control_connection_free(ControlConnection* c) {
    g_object_unref(c->connection);
    g_object_unref(c->in);
    g_object_unref(c->out);
    g_free(c);
}

static void control_write_done(GObject* source, GAsyncResult* result,
                               gpointer user_data) {
    ControlConnection* c = user_data;
    GError* err = NULL;

    (void)source;
    g_output_stream_write_all_finish(G_OUTPUT_STREAM(source), result, NULL, &err);
    if (err) g_error_free(err);
    g_io_stream_close_async(G_IO_STREAM(c->connection), G_PRIORITY_DEFAULT,
                            NULL, NULL, NULL);
    control_connection_free(c);
}

static void control_reply(ControlConnection* c, const char* reply) {
    g_output_stream_write_all_async(c->out, reply, strlen(reply),
                                    G_PRIORITY_DEFAULT, NULL, control_write_done, c);
}

static void control_read_done(GObject* source, GAsyncResult* result,
                              gpointer user_data) {
    ControlConnection* c = user_data;
    GError* err = NULL;
    gssize n;
    char* nl;

    n = g_input_stream_read_finish(G_INPUT_STREAM(source), result, &err);
    if (err) {
        g_error_free(err);
        control_connection_free(c);
        return;
    }
    if (n <= 0) { /* the client closed (or errored) before a complete line */
        control_connection_free(c);
        return;
    }
    c->len += (gsize)n;
    nl = memchr(c->buf, '\n', c->len);
    if (nl) {
        if (pinwin_control_parse(c->buf, (size_t)(nl - c->buf + 1)) ==
            PINWIN_CONTROL_OPTIONS) {
            pinwin_options_open();
            control_reply(c, "ok\n");
        } else {
            control_reply(c, "error unknown request\n");
        }
        return;
    }
    if (c->len >= sizeof(c->buf)) control_connection_free(c); /* drop */
    else g_input_stream_read_async(c->in, c->buf + c->len, sizeof(c->buf) - c->len,
                                   G_PRIORITY_DEFAULT, NULL, control_read_done, c);
}

static gboolean on_control_incoming(GSocketService* service,
                                    GSocketConnection* connection,
                                    GObject* source_object, gpointer user_data) {
    ControlConnection* c;

    (void)service;
    (void)source_object;
    (void)user_data;
    c = g_new0(ControlConnection, 1);
    c->connection = g_object_ref(connection);
    c->in = g_object_ref(g_io_stream_get_input_stream(G_IO_STREAM(connection)));
    c->out = g_object_ref(g_io_stream_get_output_stream(G_IO_STREAM(connection)));
    g_input_stream_read_async(c->in, c->buf, sizeof(c->buf), G_PRIORITY_DEFAULT,
                              NULL, control_read_done, c);
    return TRUE; /* handled; the service keeps listening */
}

/* Listen on $XDG_RUNTIME_DIR/pinwin/<pid>.sock before the command is spawned,
 * so the child can learn the path (design D1). Any problem is a diagnostic
 * plus a socketless run, never a failure. */
static void control_start(void) {
    const char* runtime = getenv("XDG_RUNTIME_DIR");
    char* dir;
    GSocketAddress* addr;
    GError* err = NULL;

    g_control_socket_path[0] = '\0';
    if (!runtime || !*runtime) {
        fprintf(stderr, "pinwin: no XDG_RUNTIME_DIR; running without a control socket\n");
        return;
    }
    dir = g_build_filename(runtime, "pinwin", NULL);
    if (g_mkdir_with_parents(dir, S_IRWXU) != 0) {
        fprintf(stderr, "pinwin: %s: %s\n", dir, g_strerror(errno));
        g_free(dir);
        return;
    }
    snprintf(g_control_socket_path, sizeof(g_control_socket_path), "%s/%d.sock",
             dir, (int)getpid());
    g_free(dir);
    /* Can only be left over from a killed instance whose PID was reused. */
    unlink(g_control_socket_path);

    g_control_service = g_socket_service_new();
    addr = g_unix_socket_address_new(g_control_socket_path);
    if (!g_socket_listener_add_address(G_SOCKET_LISTENER(g_control_service),
                                       G_SOCKET_ADDRESS(addr),
                                       G_SOCKET_TYPE_STREAM,
                                       G_SOCKET_PROTOCOL_DEFAULT,
                                       NULL, NULL, &err)) {
        fprintf(stderr, "pinwin: control socket %s: %s\n",
                g_control_socket_path, err->message);
        g_error_free(err);
        g_object_unref(addr);
        g_object_unref(g_control_service);
        g_control_service = NULL;
        return;
    }
    g_object_unref(addr);
    g_signal_connect(g_control_service, "incoming",
                     G_CALLBACK(on_control_incoming), NULL);
    g_socket_service_start(g_control_service);
    /* Bind does not restrict the file mode; keep the socket user-only. */
    chmod(g_control_socket_path, S_IRUSR | S_IWUSR);
}

/* ---- layout surfaces (add-pinwin-tray-options) -------------------------- */

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

/* Load the saved layout and validate it against the resolved monitor and the
 * cell metrics. Anything unusable falls back to the launch baseline with a
 * diagnostic and without rewriting the file (design D4). */
static void load_saved_layout(GdkMonitor* monitor) {
    PinwinLayout saved;
    GdkRectangle geom;
    int config_result;

    saved.cols = g_cols; /* an absent cols key keeps the launch COLS */
    config_result = pinwin_config_load(&saved);

    if (config_result != PINWIN_CONFIG_LOADED) return;
    gdk_monitor_get_geometry(monitor, &geom);
    if (pinwin_layout_validate(&saved, saved.cols, g_cell_w, g_cell_h,
                                geom.width, geom.height) != PINWIN_GEOM_OK) {
        fprintf(stderr, "pinwin: saved layout unusable on this output; "
                        "using launch defaults\n");
        return;
    }
    g_cols = saved.cols;
    g_layout = saved;
    apply_panel_width();
}

/* The instance's currently applied layout (options window initialisation). */
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
    load_saved_layout(g_monitor);
    apply_layout_surfaces();
    apply_size();
}

/* ---- activation / lifecycle --------------------------------------------- */

static void on_activate(GtkApplication* app, gpointer user_data) {
    (void)user_data;

    control_start();

    GtkWidget* win = gtk_application_window_new(app);
    g_win = GTK_WINDOW(win);

    gtk_layer_init_for_window(g_win);
    gtk_layer_set_namespace(g_win, "pinwin");
    gtk_layer_set_layer(g_win, GTK_LAYER_SHELL_LAYER_OVERLAY);
    gtk_layer_set_keyboard_mode(g_win, (GtkLayerShellKeyboardMode)g_keyboard_mode);

    cell_metrics_update(win);
    g_layout = pinwin_layout_default(g_cols, g_gutter);

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
    if (!g_no_tray) tray_init();
}

int glue_init(int32_t cols, int32_t gutter, int32_t keyboard_mode, int no_tray) {
    char theme_name[128];

    theme_colours(theme_name, sizeof(theme_name), g_theme_bg, g_theme_fg);
    if (getenv("PINWIN_DEBUG") && theme_name[0])
        fprintf(stderr, "pinwin: theme %s bg=#%02x%02x%02x fg=#%02x%02x%02x\n",
                theme_name, g_theme_bg[0], g_theme_bg[1], g_theme_bg[2],
                g_theme_fg[0], g_theme_fg[1], g_theme_fg[2]);
    g_cols = cols;
    g_gutter = gutter;
    g_keyboard_mode = keyboard_mode;
    g_no_tray = no_tray;

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

void glue_start(char* const argv[]) {
    g_argv = argv;
    g_application_run(G_APPLICATION(g_app), 0, NULL);
}

void glue_queue_draw(void) {
    if (g_area) gtk_widget_queue_draw(g_area);
}

void glue_exit(int32_t status) {
    tray_shutdown();
    if (g_pty_source) g_source_remove(g_pty_source);
    if (g_control_socket_path[0]) unlink(g_control_socket_path);
    exit(status);
}
