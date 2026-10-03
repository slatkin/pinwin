/*
 * options.h - pinwin's layout core: the six values (docking side, the applied
 * column count and four directional gutters) and checked geometry validation.
 *
 * Plain C, no GTK/GLib: compiled into libpinwin.a and covered by the Zig unit
 * tests (design D8).
 */
#ifndef PINWIN_OPTIONS_H
#define PINWIN_OPTIONS_H

#include <stdint.h>

/* Docking side. */
#define PINWIN_SIDE_LEFT 0
#define PINWIN_SIDE_RIGHT 1

/* Focus accent: a strip drawn on the workspace-facing edge while the panel
 * holds keyboard focus (layer surfaces get no compositor focus ring, so the
 * panel marks focus itself). enabled must be 0 or 1; width is the strip
 * width in px (1..=65535 when enabled, ignored when off). */
typedef struct {
    int32_t enabled;
    uint8_t r, g, b;
    int32_t width;
} PinwinAccent;

/* Validation results from pinwin_layout_validate. */
#define PINWIN_GEOM_OK 0
#define PINWIN_GEOM_ERR_SIDE 1      /* side is not PINWIN_SIDE_* */
#define PINWIN_GEOM_ERR_OVERFLOW 2  /* checked arithmetic would overflow */
#define PINWIN_GEOM_ERR_NO_RESERVE 3 /* left + panel + right is negative */
#define PINWIN_GEOM_ERR_NO_ROW 4    /* vertical space < one terminal row */
#define PINWIN_GEOM_ERR_NO_WIDTH 5  /* reservation leaves no output width */
#define PINWIN_GEOM_ERR_METRICS 6   /* nonsense panel/output dimensions */

typedef struct {
    int32_t side;                    /* PINWIN_SIDE_* */
    int32_t cols;                    /* panel width in terminal columns */
    int32_t top, bottom, left, right; /* gutters in pixels, may be negative */
} PinwinLayout;

/* Horizontal placement for the docking side, with checked arithmetic.
 * *edge_margin is the visible panel's margin on its docking edge (Left when
 * docked left, Right when docked right); *reservation is
 * left + panel_width + right, the strip reserved at that edge. Returns 1 on
 * success, 0 on overflow (including a panel_width beyond int32_t). */
int pinwin_side_geometry(const PinwinLayout* layout, long long panel_width,
                          int32_t* edge_margin, int32_t* reservation);

/* Validate a layout against the output and font metrics, all in logical
 * pixels. panel_width is panel_cols * cell_w, where panel_cols is the
 * layout's applied column count. Rejects unknown sides,
 * checked-arithmetic overflow, a reservation sum below zero
 * (left + panel_width + right), vertical space below one complete terminal
 * row (output_h - top - bottom < cell_h) and a reservation that leaves no
 * output width for other windows (reservation >= output_w). Negative
 * gutters are allowed (they move the panel edge past its output edge).
 * Returns one of the PINWIN_GEOM_* values. */
int pinwin_layout_validate(const PinwinLayout* layout, int32_t panel_cols,
                            int32_t cell_w, int32_t cell_h, int32_t output_w,
                            int32_t output_h);

/* Validate the focus accent: enabled must be 0 or 1, and when enabled the
 * width must be 1..=65535. The colour bytes are always valid and the width is
 * ignored when off. Returns 1 on success, 0 on rejection. */
int pinwin_accent_validate(const PinwinAccent* accent);

/* The pty read loop's yield decision for one main-loop dispatch. budget_us == 0
 * means unbounded (no tween running): never yield. Otherwise yield once the
 * dispatch has consumed budget_us of wall time since started_us, so an image
 * burst cannot starve the frame clock mid-tween. Returns 1 to yield. */
int pinwin_pty_yield(int64_t started_us, int64_t now_us, int64_t budget_us);

/* One 60 Hz frame, the cap on how much tween time a single frame may
 * advance. */
#define PINWIN_ANIM_FRAME_US 16667

/* One frame-clock tick of the animated width tween, as pure arithmetic so the
 * Zig unit tests can drive it. Advances *t_us by the interval since *last_us
 * capped at PINWIN_ANIM_FRAME_US (a stalled clock delays motion instead of
 * skipping it), stamps *last_us with now_us, and maps the ease-out-cubic
 * progress onto the width: *cur_px between from_px and to_px. A *last_us of 0
 * means no tick has run yet: it is stamped and nothing moves. Returns 1 when
 * the tween finished (*t_us reached dur_us; *cur_px is exactly to_px), 0
 * otherwise. */
int pinwin_anim_step(int64_t now_us, int64_t dur_us, int32_t from_px,
                      int32_t to_px, int64_t* t_us, int64_t* last_us,
                      int32_t* cur_px);

#endif /* PINWIN_OPTIONS_H */
