/*
 * pinwin_api.c - libpinwin's public C ABI (pinwin_api.h).
 *
 * The host calls from its own thread; the GTK side runs on a thread this file
 * spawns, with glue_init (GTK/layer-shell setup) and the GtkApplication main
 * loop both on it (design D2). The start-up handshake keeps pinwin_start
 * synchronous despite that spawn. Every call returns a result code and writes
 * no diagnostics; the library never calls exit (design D3).
 *
 * State lives in glue.c/glue_internal.h (the GTK side); this file owns only the
 * lifecycle guard and the thread.
 */

#include "pinwin_api.h"
#include "glue_internal.h"

#include <glib.h>

/* ---- lifecycle state ---------------------------------------------------- */

static GMutex g_api_lock;
static GCond g_api_cond;
static GThread* g_api_thread;
static gboolean g_api_running;
static gboolean g_api_start_done;
static gboolean g_api_start_ok;
static PinwinStartup g_api_startup;

/* Pure argument checks, no GTK (design D2). An out-of-range keyboard mode is
 * rejected here too (pinwin-panel spec: Invalid keyboard mode). */
static int startup_valid(const PinwinStartup* startup) {
    if (startup == NULL) return 0;
    if (startup->master_fd < 0) return 0;
    if (startup->layout.side != PINWIN_SIDE_LEFT &&
        startup->layout.side != PINWIN_SIDE_RIGHT)
        return 0;
    if (startup->layout.cols < 1 || startup->layout.cols > 65535) return 0;
    if (startup->keyboard_mode != PINWIN_KEYBOARD_NONE &&
        startup->keyboard_mode != PINWIN_KEYBOARD_EXCLUSIVE &&
        startup->keyboard_mode != PINWIN_KEYBOARD_ON_DEMAND)
        return 0;
    return 1;
}

/* The GTK thread: init the GTK/layer-shell side, report the handshake result,
 * then own the GtkApplication main loop until pinwin_stop quits it. */
static gpointer gtk_thread_main(gpointer data) {
    (void)data;

    int ok = glue_init(&g_api_startup.layout, g_api_startup.keyboard_mode);

    g_mutex_lock(&g_api_lock);
    g_api_start_ok = ok;
    g_api_start_done = TRUE;
    g_cond_signal(&g_api_cond);
    g_mutex_unlock(&g_api_lock);

    if (ok) g_application_run(G_APPLICATION(g_app), 0, NULL);
    return NULL;
}

/* Runs on the GTK thread (posted with g_main_context_invoke): close both
 * surfaces, then quit the loop so the thread returns and pinwin_stop's join
 * completes. */
static gboolean stop_on_gtk_thread(gpointer data) {
    (void)data;

    if (g_reserve) {
        gtk_window_destroy(g_reserve);
        g_reserve = NULL;
    }
    if (g_win) {
        gtk_window_destroy(g_win);
        g_win = NULL;
    }
    g_area = NULL;
    if (g_app) g_application_quit(G_APPLICATION(g_app));
    return G_SOURCE_REMOVE;
}

/* ---- public ABI --------------------------------------------------------- */

int pinwin_start(const PinwinStartup* startup) {
    if (!startup_valid(startup)) return PINWIN_ERR_INVALID;

    g_mutex_lock(&g_api_lock);
    if (g_api_running) {
        g_mutex_unlock(&g_api_lock);
        return PINWIN_ERR_ALREADY_RUNNING;
    }

    g_api_startup = *startup;
    g_api_start_done = FALSE;
    g_api_start_ok = FALSE;
    g_api_thread = g_thread_new("pinwin-gtk", gtk_thread_main, NULL);

    /* Wait for GTK/layer-shell init on the spawned thread; the cond releases
     * the lock while waiting, which the thread takes to report. */
    while (!g_api_start_done) g_cond_wait(&g_api_cond, &g_api_lock);

    if (!g_api_start_ok) {
        GThread* thread = g_api_thread;
        g_api_thread = NULL;
        g_mutex_unlock(&g_api_lock);
        g_thread_join(thread);
        return PINWIN_ERR_NO_DISPLAY;
    }

    g_api_running = TRUE;
    g_mutex_unlock(&g_api_lock);
    return PINWIN_OK;
}

void pinwin_stop(void) {
    GThread* thread;

    g_mutex_lock(&g_api_lock);
    if (!g_api_running) {
        g_mutex_unlock(&g_api_lock);
        return;
    }
    g_api_running = FALSE;
    thread = g_api_thread;
    g_api_thread = NULL;
    g_mutex_unlock(&g_api_lock);

    /* GTK calls belong to the GTK thread: hand the teardown to its loop, then
     * wait for the thread to finish. */
    g_main_context_invoke(NULL, stop_on_gtk_thread, NULL);
    if (thread) g_thread_join(thread);
}
