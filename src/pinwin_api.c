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

#include <fcntl.h>
#include <glib.h>

/* ---- lifecycle state ---------------------------------------------------- */

static GMutex g_api_lock;
static GCond g_api_cond;
static GThread* g_api_thread;
static gboolean g_api_running;
static gboolean g_api_start_done;
static gboolean g_api_start_ok;
static PinwinStartup g_api_startup;

/* Completes pinwin_start's handshake (design D2/D5). glue.c calls it with 1
 * once the panel is activated and its live metrics exist; the GTK thread calls
 * it with 0 when init fails or the loop returns before going live. Only the
 * first result counts, so the post-loop call cannot undo a successful start. */
void pinwin_api_start_result(int ok) {
    g_mutex_lock(&g_api_lock);
    if (!g_api_start_done) {
        g_api_start_ok = ok;
        g_api_start_done = TRUE;
        g_cond_signal(&g_api_cond);
    }
    g_mutex_unlock(&g_api_lock);
}

/* Pure argument checks, no GTK (design D2). An out-of-range keyboard mode is
 * rejected here too (pinwin-panel spec: Invalid keyboard mode). */
static int startup_valid(const PinwinStartup* startup) {
    if (startup == NULL) return 0;
    /* A negative or already-closed fd is a bad argument, reported as an ABI
     * result before any GTK work (design D2/D3); the library never exits. */
    if (startup->master_fd < 0) return 0;
    if (fcntl(startup->master_fd, F_GETFL) < 0) return 0;
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

/* The GTK thread: init the GTK/layer-shell side, then own the GtkApplication
 * main loop until pinwin_stop quits it. The start handshake is completed by
 * glue.c once the panel is activated with live metrics (design D2/D5), so it
 * is not signalled here on success. */
static gpointer gtk_thread_main(gpointer data) {
    (void)data;

    /* The GTK thread owns the pty attachment from here (design D6): the host's
     * master fd arrives over the ABI and pty.c takes it non-blocking. */
    g_pty_fd = g_api_startup.master_fd;

    if (!glue_init(&g_api_startup.layout, g_api_startup.keyboard_mode)) {
        pinwin_api_start_result(0);
        return NULL;
    }

    g_application_run(G_APPLICATION(g_app), 0, NULL);

    /* The loop returned: a stop, or a failed start (a NULL monitor resolution
     * quits it). Close anything stop_on_gtk_thread did not, then fail a
     * still-pending handshake rather than leave pinwin_start waiting forever.
     * A completed handshake is unaffected. */
    glue_close_surfaces();
    pinwin_api_start_result(0);
    return NULL;
}

/* Runs on the GTK thread (posted with g_main_context_invoke): close both
 * surfaces, then quit the loop so the thread returns and pinwin_stop's join
 * completes. */
static gboolean stop_on_gtk_thread(gpointer data) {
    (void)data;

    glue_close_surfaces();
    if (g_app) g_application_quit(G_APPLICATION(g_app));
    return G_SOURCE_REMOVE;
}

/* The bounded part of Phase 2's invoke-and-wait: a wedged GTK loop must not
 * block the host thread forever (design D5, Risks). Degrades to
 * PINWIN_ERR_INTERNAL. */
#define APPLY_WAIT_TIMEOUT_US (G_GINT64_CONSTANT(5) * 1000000)

/* One in-flight apply: the caller copies the layout in here, the GTK main
 * context runs apply_on_gtk_thread and reports through the cond. The request
 * owns its layout copy, so a queued callback can never dereference caller
 * memory. Refcounted (caller + pending callback) because a caller that times
 * out must not free the request while the GTK thread may still run the
 * callback. */
typedef struct {
    PinwinLayout layout;
    uint32_t duration_ms; /* 0 snaps; see pinwin_apply_layout_animated */
    GMutex lock;
    GCond cond;
    gboolean done;
    int result;
    gint refs;
} ApplyRequest;

static void apply_request_unref(ApplyRequest* req) {
    if (g_atomic_int_dec_and_test(&req->refs)) {
        g_cond_clear(&req->cond);
        g_mutex_clear(&req->lock);
        g_free(req);
    }
}

/* Phase 1, caller thread, no GTK (design D5): a null layout, unknown side or
 * out-of-range column count is PINWIN_ERR_INVALID before any GTK interaction.
 * The panel width needs live cell metrics, so its checked arithmetic stays in
 * Phase 2; with any non-negative panel width the one overflow this phase can
 * rule out without metrics is the gutters' own sum, which the shared
 * pinwin_side_geometry check performs. */
static int apply_layout_structurally_valid(const PinwinLayout* layout) {
    int32_t margin, reservation;

    if (layout == NULL) return 0;
    if (layout->side != PINWIN_SIDE_LEFT && layout->side != PINWIN_SIDE_RIGHT)
        return 0;
    if (layout->cols < 1 || layout->cols > 65535) return 0;
    if (!pinwin_side_geometry(layout, 0, &margin, &reservation)) return 0;
    return 1;
}

/* Phase 2, GTK thread: validate the layout against the live monitor and cell
 * metrics and publish it, exactly like the Apply path did (glue_publish_layout
 * is validate-then-apply). Returns the result to the waiting caller. A panel
 * without live metrics is NOT_RUNNING, never INVALID (design D5): INVALID
 * means the layout itself was refused. A terminal that could not be allocated
 * keeps the previous grid and is INTERNAL (design D3), not a layout verdict. */
static gboolean apply_on_gtk_thread(gpointer data) {
    ApplyRequest* req = data;
    int geom = glue_publish_layout(&req->layout, req->duration_ms);
    int result;

    if (geom == GLUE_NOT_LIVE)
        result = PINWIN_ERR_NOT_RUNNING;
    else if (geom == GLUE_ERR_TERMINAL)
        result = PINWIN_ERR_INTERNAL;
    else
        result = geom == PINWIN_GEOM_OK ? PINWIN_OK : PINWIN_ERR_INVALID;

    g_mutex_lock(&req->lock);
    req->result = result;
    req->done = TRUE;
    g_cond_signal(&req->cond);
    g_mutex_unlock(&req->lock);

    apply_request_unref(req);
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

static int apply_layout_common(const PinwinLayout* layout, uint32_t duration_ms) {
    ApplyRequest* req;
    int result;

    if (!apply_layout_structurally_valid(layout)) return PINWIN_ERR_INVALID;

    /* Not started → PINWIN_ERR_NOT_RUNNING without blocking (design D5). */
    g_mutex_lock(&g_api_lock);
    if (!g_api_running) {
        g_mutex_unlock(&g_api_lock);
        return PINWIN_ERR_NOT_RUNNING;
    }
    g_mutex_unlock(&g_api_lock);

    req = g_new0(ApplyRequest, 1);
    req->layout = *layout; /* the request owns the copy (see ApplyRequest) */
    req->duration_ms = duration_ms;
    req->refs = 2; /* caller + the pending GTK callback */
    g_mutex_init(&req->lock);
    g_cond_init(&req->cond);

    /* The metrics live on the GTK thread, so the publish (and its validation)
     * must run there; the caller waits for the synchronous result. */
    g_main_context_invoke(NULL, apply_on_gtk_thread, req);

    g_mutex_lock(&req->lock);
    if (!req->done) {
        gint64 deadline = g_get_monotonic_time() + APPLY_WAIT_TIMEOUT_US;
        while (!req->done) {
            if (!g_cond_wait_until(&req->cond, &req->lock, deadline)) break;
        }
    }
    /* A timed-out or never-posted callback leaves done FALSE: report INTERNAL
     * rather than blocking forever. The request's refcount lets the callback
     * land later without touching freed memory. */
    result = req->done ? req->result : PINWIN_ERR_INTERNAL;
    g_mutex_unlock(&req->lock);
    apply_request_unref(req);
    return result;
}

int pinwin_apply_layout(const PinwinLayout* layout) {
    return apply_layout_common(layout, 0);
}

int pinwin_apply_layout_animated(const PinwinLayout* layout, uint32_t duration_ms) {
    if (duration_ms > PINWIN_ANIM_MAX_MS) duration_ms = PINWIN_ANIM_MAX_MS;
    return apply_layout_common(layout, duration_ms);
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
