/*
 * glue_anim.c - the animated width transition (add-animated-width design D2,
 * D4-D6): one frame-clock tick callback eases the visible panel's width and the
 * reservation's exclusive zone together, and a watchdog snaps to the final
 * layout if frames stop. Everything runs on the GTK thread; the state is one
 * static struct, so the tick allocates nothing.
 */

#include "glue_internal.h"

#include <math.h>

typedef struct {
    int active;
    int32_t from_px, to_px, cur_px;
    gint64 t0_us; /* 0 until the first tick stamps it */
    gint64 dur_us;
    GtkWidget* widget; /* the widget the tick callback is registered on */
    guint tick_id;
    guint watchdog_id;
} Anim;

static Anim a;

int32_t panel_px(void) { return a.active ? a.cur_px : g_cols * g_cell_w; }

int glue_anim_active(void) { return a.active; }

/* Ease-out cubic: close to niri's critically damped window-resize spring. */
static double ease(double t) {
    double u = 1.0 - t;
    return 1.0 - u * u * u;
}

int glue_anim_allowed(void) {
    gboolean enabled = TRUE;
    GtkSettings* settings = gtk_settings_get_default();

    if (settings) g_object_get(settings, "gtk-enable-animations", &enabled, NULL);
    return enabled;
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
    /* The tween frame cache belongs to the tween: drop it so the next draw
     * takes the ordinary full-render path and the surface is not held. */
    render_grid_cache_drop();
    glue_grid_resize_deferred_fire();
}

void glue_anim_cancel(void) { anim_stop(0, 0); }

/* End the tween at the exact target through the same geometry path as a
 * non-animated apply. */
static void anim_finish(int in_tick, int in_watchdog) {
    anim_stop(in_tick, in_watchdog);
    glue_apply_geometry();
}

static gboolean anim_tick(GtkWidget* widget, GdkFrameClock* clock, gpointer data) {
    gint64 now = gdk_frame_clock_get_frame_time(clock);
    double t;
    (void)widget;
    (void)data;

    if (a.t0_us == 0) a.t0_us = now;
    t = (double)(now - a.t0_us) / (double)a.dur_us;
    if (t >= 1.0) {
        anim_finish(1, 0);
        return G_SOURCE_REMOVE;
    }
    if (t < 0.0) t = 0.0;
    a.cur_px = a.from_px + (int32_t)lround((double)(a.to_px - a.from_px) * ease(t));
    glue_apply_geometry();
    return G_SOURCE_CONTINUE;
}

static gboolean anim_watchdog(gpointer data) {
    (void)data;
    anim_finish(0, 1);
    return G_SOURCE_REMOVE;
}

/* Start, or retarget from `from_px`, a tween to `to_px` over duration_ms. */
void glue_anim_begin(int32_t from_px, int32_t to_px, uint32_t duration_ms) {
    if (!g_win) return;
    anim_stop(0, 0);
    a.active = 1;
    a.from_px = from_px;
    a.to_px = to_px;
    a.cur_px = from_px;
    a.t0_us = 0;
    a.dur_us = (gint64)duration_ms * 1000;
    a.widget = GTK_WIDGET(g_win);
    a.tick_id = gtk_widget_add_tick_callback(a.widget, anim_tick, NULL, NULL);
    a.watchdog_id = g_timeout_add(duration_ms + 100, anim_watchdog, NULL);
}

/* Horizontal shift that keeps the grid against the docked edge while the
 * surface is wider or narrower than the grid; zero when not animating. */
int32_t glue_anim_draw_offset(void) {
    if (!a.active || g_layout.side != PINWIN_SIDE_RIGHT || !g_area) return 0;
    return gtk_widget_get_width(g_area) - g_cols * g_cell_w;
}
