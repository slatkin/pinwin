/*
 * options.c - pinwin's layout core: the six layout values (docking side, the
 * applied column count and four directional gutters) and checked geometry
 * validation.
 *
 * Plain C, no GLib/GTK, so the core is compiled into libpinwin.a and exercised
 * by the Zig unit tests (design D8). The options window, the command line and
 * the config file are gone (design D1): the host supplies the full layout over
 * the ABI and owns persistence.
 */

#include "options.h"

#include <math.h>

int pinwin_side_geometry(const PinwinLayout* layout, long long panel_width,
                          int32_t* edge_margin, int32_t* reservation) {
    long long sum;

    if (layout->side != PINWIN_SIDE_LEFT && layout->side != PINWIN_SIDE_RIGHT)
        return 0;
    if (panel_width < 0 || panel_width > INT32_MAX) return 0;
    sum = (long long)layout->left + (long long)panel_width + (long long)layout->right;
    if (sum > INT32_MAX) return 0;
    *edge_margin = layout->side == PINWIN_SIDE_LEFT ? layout->left : layout->right;
    *reservation = (int32_t)sum;
    return 1;
}

int pinwin_layout_validate(const PinwinLayout* layout, int32_t panel_cols,
                            int32_t cell_w, int32_t cell_h, int32_t output_w,
                            int32_t output_h) {
    int32_t reservation;
    int32_t margin;

    if (layout->side != PINWIN_SIDE_LEFT && layout->side != PINWIN_SIDE_RIGHT)
        return PINWIN_GEOM_ERR_SIDE;
    if (panel_cols < 1 || cell_w < 1 || cell_h < 1 || output_w < 1 || output_h < 1)
        return PINWIN_GEOM_ERR_METRICS;

    if (!pinwin_side_geometry(layout, (long long)panel_cols * (long long)cell_w,
                               &margin, &reservation))
        return PINWIN_GEOM_ERR_OVERFLOW;
    if (reservation < 0) return PINWIN_GEOM_ERR_NO_RESERVE;
    if (reservation >= output_w) return PINWIN_GEOM_ERR_NO_WIDTH;

    /* Vertical: the visible panel must keep room for one complete row.
     * Negative top/bottom insets give more room, never less. */
    {
        long long height = (long long)layout->top + (long long)layout->bottom;
        if (height > output_h - (long long)cell_h) return PINWIN_GEOM_ERR_NO_ROW;
    }
    return PINWIN_GEOM_OK;
}

int pinwin_accent_validate(const PinwinAccent* accent) {
    if (accent->enabled != 0 && accent->enabled != 1) return 0;
    if (!accent->enabled) return 1;
    return accent->width >= 1 && accent->width <= 65535;
}

int pinwin_pty_yield(int64_t started_us, int64_t now_us, int64_t budget_us) {
    if (budget_us <= 0) return 0;
    return now_us - started_us >= budget_us;
}

/* Ease-out cubic, shared with nothing: close to niri's critically damped
 * window-resize spring. Kept here so pinwin_anim_step is the one place the
 * tween's math lives. */
static double anim_ease(double t) {
    double u = 1.0 - t;
    return 1.0 - u * u * u;
}

int pinwin_anim_step(int64_t now_us, int64_t dur_us, int32_t from_px,
                      int32_t to_px, int64_t* t_us, int64_t* last_us,
                      int32_t* cur_px) {
    int64_t step;

    if (*last_us == 0) {
        *last_us = now_us;
        *cur_px = from_px;
        return 0;
    }
    step = now_us - *last_us;
    if (step > PINWIN_ANIM_FRAME_US) step = PINWIN_ANIM_FRAME_US;
    if (step < 0) step = 0;
    *last_us = now_us;
    *t_us += step;
    if (*t_us >= dur_us) {
        *cur_px = to_px;
        return 1;
    }
    *cur_px = from_px + (int32_t)lround(
        (double)(to_px - from_px) * anim_ease((double)*t_us / (double)dur_us));
    return 0;
}
