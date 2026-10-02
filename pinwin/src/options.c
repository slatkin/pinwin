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
