/*
 * glue_anim.c - the animated width transition (add-animated-width design D2,
 * D4-D6): one frame-clock tick callback eases the visible panel's width and the
 * reservation's exclusive zone together, and a watchdog snaps to the final
 * layout if frames stop. The tween's math (capped per-frame advance, easing,
 * finish detection) lives in pinwin_anim_step (options.c) so the unit tests
 * can drive it; this file is the GTK plumbing. Everything runs on the GTK
 * thread; the state is one static struct, so the tick allocates nothing.
 */

#include "glue_internal.h"

#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

/* No tick for this long means frames stopped arriving; measured from the last
 * tick, checked every 100 ms. The old fixed begin+duration deadline also fired
 * while frames merely arrived late (a starved clock mid-repaint), snapping the
 * tween before it finished. */
#define ANIM_STALL_US(dur_us) ((dur_us) + 100000)

typedef struct {
    int active;
    int32_t from_px, to_px, cur_px;
    gint64 t_us; /* virtual tween time: advanced per frame, capped */
    gint64 dur_us;
    gint64 last_us; /* frame time of the last tick; 0 until the first tick */
    gint64 last_wall_us; /* monotonic stamp of the last tick, for the watchdog */
    GtkWidget* widget; /* the widget the tick callback is registered on */
    guint tick_id;
    guint watchdog_id;
} Anim;

static Anim a;

int32_t panel_px(void) { return a.active ? a.cur_px : g_cols * g_cell_w; }

int glue_anim_active(void) { return a.active; }

int glue_anim_allowed(void) {
    gboolean enabled = TRUE;
    GtkSettings* settings = gtk_settings_get_default();

    if (settings) g_object_get(settings, "gtk-enable-animations", &enabled, NULL);
    return enabled;
}

/* ---- timing instrumentation (PINWIN_ANIM_LOG=<path>) ---------------------- */

/* The fd is resolved once, on first use: unset or empty means off for the
 * process lifetime. Single-threaded (GTK main loop), so no locking. */
static int anim_log_fd(void) {
    static int fd = -2;

    if (fd == -2) {
        const char* path = getenv("PINWIN_ANIM_LOG");
        fd = path && *path ? open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC,
                                  S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH)
                           : -1;
    }
    return fd;
}

int anim_logging(void) { return anim_log_fd() >= 0; }

void anim_log(const char* fmt, ...) {
    char buf[256];
    va_list ap;
    int len;
    ssize_t written;
    int fd = anim_log_fd();

    if (fd < 0) return;
    va_start(ap, fmt);
    len = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);
    if (len <= 0) return;
    if (len > (int)sizeof(buf) - 1) len = (int)sizeof(buf) - 1;
    written = write(fd, buf, (size_t)len);
    (void)written; /* best effort: lost animation telemetry is not an error */
}

/* Drop the tick and watchdog. `in_tick` / `in_watchdog` mark the source that
 * is currently running (it returns G_SOURCE_REMOVE itself). */
static void anim_stop(int in_tick, int in_watchdog) {
    if (a.tick_id && !in_tick && a.widget) gtk_widget_remove_tick_callback(a.widget, a.tick_id);
    if (a.watchdog_id && !in_watchdog) g_source_remove(a.watchdog_id);
    a.tick_id = 0;
    a.watchdog_id = 0;
    a.widget = NULL;
    a.active = 0;
}

void glue_anim_cancel(void) {
    if (a.active) anim_log("cancel cur=%d\n", a.cur_px);
    anim_stop(0, 0);
}

static gboolean anim_tick(GtkWidget* widget, GdkFrameClock* clock, gpointer data) {
    gint64 now = gdk_frame_clock_get_frame_time(clock);
    gint64 geom0 = anim_logging() ? g_get_monotonic_time() : 0;
    int done;
    (void)widget;
    (void)data;

    a.last_wall_us = g_get_monotonic_time();
    done = pinwin_anim_step(now, a.dur_us, a.from_px, a.to_px, &a.t_us,
                            &a.last_us, &a.cur_px);
    if (done) {
        anim_stop(1, 0);
    }
    glue_apply_geometry();
    if (anim_logging()) {
        size_t pty_bytes;
        gint64 pty_us;
        pty_take_stats(&pty_bytes, &pty_us);
        anim_log("tick frame=%lld wall=%lld t=%lld cur=%d geom_us=%lld"
                 " pty_bytes=%zu pty_us=%lld\n",
                 (long long)now, (long long)g_get_monotonic_time(),
                 (long long)a.t_us, a.cur_px,
                 (long long)(g_get_monotonic_time() - geom0), pty_bytes,
                 (long long)pty_us);
    }
    if (done) {
        anim_log("finish reason=tick cur=%d\n", a.cur_px);
        return G_SOURCE_REMOVE;
    }
    return G_SOURCE_CONTINUE;
}

static gboolean anim_watchdog(gpointer data) {
    (void)data;
    /* Each tick re-arms this check by stamping last_wall_us: finish only when
     * frames have stopped arriving, not when wall clock outruns the tween's
     * virtual time. */
    if (a.last_us != 0 &&
        g_get_monotonic_time() - a.last_wall_us > ANIM_STALL_US(a.dur_us)) {
        anim_stop(0, 1);
        glue_apply_geometry();
        anim_log("finish reason=watchdog cur=%d\n", a.cur_px);
        return G_SOURCE_REMOVE;
    }
    return G_SOURCE_CONTINUE;
}

/* Start, or retarget from `from_px`, a tween to `to_px` over duration_ms. */
void glue_anim_begin(int32_t from_px, int32_t to_px, uint32_t duration_ms) {
    if (!g_win) return;
    anim_stop(0, 0);
    a.active = 1;
    a.from_px = from_px;
    a.to_px = to_px;
    a.cur_px = from_px;
    a.t_us = 0;
    a.dur_us = (gint64)duration_ms * 1000;
    a.last_us = 0;
    a.last_wall_us = 0;
    a.widget = GTK_WIDGET(g_win);
    a.tick_id = gtk_widget_add_tick_callback(a.widget, anim_tick, NULL, NULL);
    a.watchdog_id = g_timeout_add(100, anim_watchdog, NULL);
    anim_log("begin from=%d to=%d dur=%lldus\n", from_px, to_px,
             (long long)a.dur_us);
}

/* Horizontal shift that keeps the grid against the docked edge while the
 * surface is wider or narrower than the grid; zero when not animating. */
int32_t glue_anim_draw_offset(void) {
    if (!a.active || g_layout.side != PINWIN_SIDE_RIGHT || !g_area) return 0;
    return gtk_widget_get_width(g_area) - g_cols * g_cell_w;
}
