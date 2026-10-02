/*
 * options.c - pinwin's layout core: the six layout values (docking side, the
 * applied column count and four directional gutters), their launch defaults,
 * strict parsing and checked geometry validation.
 *
 * Plain C, no GLib/GTK, so the core is compiled into libpinwin.a and exercised
 * by the Zig unit tests (design D8). The options window, the command line and
 * the config file are gone (design D1): the host supplies the full layout over
 * the ABI and owns persistence.
 */

#include "options.h"

#include <errno.h>
#include <stdlib.h>

PinwinLayout pinwin_layout_default(int32_t cols, int32_t gutter) {
    PinwinLayout layout;
    layout.side = PINWIN_SIDE_LEFT;
    layout.cols = cols;
    layout.top = 0;
    layout.bottom = 0;
    layout.left = 0;
    layout.right = gutter > 0 ? gutter : 0;
    return layout;
}

int pinwin_parse_cols(const char* text, int32_t* out) {
    long long value = 0;

    if (!text || !*text) return 0;
    for (const char* p = text; *p; p++) {
        if (*p < '0' || *p > '9') return 0; /* no sign, no whitespace */
        value = value * 10 + (*p - '0');
        /* The partial value still fits long long, so bailing out here is
         * safe for arbitrarily long input. */
        if (value > 65535) return 0;
    }
    if (value < 1) return 0; /* a zero-column terminal is meaningless */
    *out = (int32_t)value;
    return 1;
}

int pinwin_parse_gutter(const char* text, int32_t* out) {
    char* end;
    long long value;
    const char* digits;
    int negative = 0;

    if (!text || !*text) return 0;
    digits = text;
    if (*digits == '-') {
        negative = 1;
        digits++;
    }
    if (!*digits) return 0;
    for (const char* p = digits; *p; p++) {
        if (*p < '0' || *p > '9') return 0;
    }
    errno = 0;
    value = strtoll(digits, &end, 10);
    if (errno != 0 || *end != '\0') return 0;
    if (negative) value = -value;
    if (value > INT32_MAX || value < INT32_MIN) return 0;
    *out = (int32_t)value;
    return 1;
}

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
