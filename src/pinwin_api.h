/*
 * pinwin_api.h - libpinwin's public C ABI.
 *
 * The whole surface a host needs: hand over a pty master fd, a full layout and
 * a keyboard mode, and the library runs the panel on its own GTK thread until
 * pinwin_stop. Plain C with no GTK types, so a consumer includes this without
 * the GTK headers.
 *
 * Threading contract (design D5): call from the host's thread. pinwin_apply_layout
 * and pinwin_stop MUST NOT be called from the GTK thread (the thread running the
 * GtkApplication main loop): pinwin_apply_layout would deadlock on its own wait,
 * and pinwin_stop would join the calling thread.
 *
 * The library never terminates the host process: every failure is one of the
 * PINWIN_ERR_* results below, and it writes no diagnostics.
 */
#ifndef PINWIN_API_H
#define PINWIN_API_H

#include <stdint.h>

#include "options.h" /* PinwinLayout */
#include "pinwin.h"  /* PINWIN_KEYBOARD_* */

/* The keyboard_mode values travel in the ABI, so pin the constants the
 * consumer passes: changing one is an ABI break, not a rename. */
#if PINWIN_KEYBOARD_NONE != 0 || PINWIN_KEYBOARD_EXCLUSIVE != 1 || \
    PINWIN_KEYBOARD_ON_DEMAND != 2
#error "PINWIN_KEYBOARD_* values are part of the ABI"
#endif

/* Result codes; PINWIN_OK is 0 so `if (pinwin_start(...))` reads as failure. */
#define PINWIN_OK 0
#define PINWIN_ERR_INVALID 1         /* bad arguments or layout */
#define PINWIN_ERR_ALREADY_RUNNING 2 /* a panel is already running */
#define PINWIN_ERR_NOT_RUNNING 3     /* no panel to act on */
#define PINWIN_ERR_NO_DISPLAY 4      /* no GTK display / no wlr-layer-shell */
#define PINWIN_ERR_INTERNAL 5        /* unexpected failure */

/* Everything pinwin_start needs: the host-owned pty master and the full initial
 * panel layout. master_fd is stored for the terminal attachment; the host keeps
 * ownership of the child side of the pty. */
typedef struct {
    int32_t master_fd;
    PinwinLayout layout;
    int32_t keyboard_mode; /* one of PINWIN_KEYBOARD_* */
} PinwinStartup;

/* Start the panel. Validates the arguments purely (no GTK), stores the startup,
 * spawns the GTK thread that owns the GtkApplication main loop, and blocks until
 * the panel is activated with live metrics (GTK/layer-shell init plus the first
 * draw that resolves the monitor), or that thread reports a startup failure.
 * Returning only once the panel is live is what lets a valid layout applied
 * immediately after pinwin_start succeed.
 *
 * Returns PINWIN_OK once the thread is up, PINWIN_ERR_INVALID for a null
 * startup, a negative master_fd, an unknown side or keyboard mode, or cols
 * outside 1..=65535, PINWIN_ERR_NO_DISPLAY when GTK or wlr-layer-shell is
 * unavailable (nothing opens), or PINWIN_ERR_ALREADY_RUNNING when a panel is
 * already running. A prior successful pinwin_stop leaves the library
 * restartable. */
int pinwin_start(const PinwinStartup* startup);

/* Apply a full layout to the running panel. Two-phase (design D5): pure
 * structural checks on the caller's thread (unknown side, cols outside
 * 1..=65535, checked arithmetic), then validation against the live monitor and
 * cell metrics on the GTK thread; a synchronous result is returned.
 *
 * Returns PINWIN_OK, PINWIN_ERR_INVALID for a layout the panel refuses (the
 * applied layout is then unchanged), PINWIN_ERR_NOT_RUNNING when no panel is
 * running, or PINWIN_ERR_INTERNAL when the terminal grid could not be allocated
 * (the previous grid stays; the host keeps running). PINWIN_ERR_INVALID means
 * only that the layout itself was refused; it is never used for a panel that is
 * not yet live (pinwin_start only returns once the panel is live, and a panel
 * without live metrics reports PINWIN_ERR_NOT_RUNNING). MUST NOT be called from
 * the GTK thread. */
int pinwin_apply_layout(const PinwinLayout* layout);

/* Documented default animation duration for hosts to pass; the library itself
 * has no default (duration_ms is always the caller's). Longer durations are
 * clamped to PINWIN_ANIM_MAX_MS. */
#define PINWIN_ANIM_DEFAULT_MS 200
#define PINWIN_ANIM_MAX_MS 1000

/* pinwin_apply_layout with an animated width change. Validation, result codes
 * and threading are exactly pinwin_apply_layout's; PINWIN_OK means the layout
 * was validated and accepted, not that the animation finished. When only the
 * column count differs from the applied layout (same side, left and right
 * gutters), duration_ms > 0 and GTK animations are enabled, the panel width and
 * the reservation ease from the current width to the target over duration_ms
 * (ease-out cubic), ending at exactly cols * cell width. Anything else applies
 * in one step. The terminal grid and pty winsize resize once, to the target
 * columns, when the animation starts. A call during an animation retargets from
 * the current width; MUST NOT be called from the GTK thread. */
int pinwin_apply_layout_animated(const PinwinLayout* layout, uint32_t duration_ms);

/* Stop the panel: close the visible and reservation surfaces, quit the GTK main
 * loop and join the GTK thread. A no-op when not running. MUST NOT be called
 * from the GTK thread. */
void pinwin_stop(void);

#endif /* PINWIN_API_H */
