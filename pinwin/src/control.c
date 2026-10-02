/*
 * control.c - parsing for the per-instance control API (add-pinwin-control).
 * The only request is `options`, which opens the options window exactly as
 * the tray's `Options...` does; anything else is unknown and answered with
 * "error unknown request". The socket itself lives in glue.c (design D1).
 */

#include "control.h"

#include <string.h>

PinwinControlRequest pinwin_control_parse(const char* line, size_t len) {
    if (len > 0 && line[len - 1] == '\n') len--;
    if (len == 7 && memcmp(line, "options", 7) == 0) return PINWIN_CONTROL_OPTIONS;
    return PINWIN_CONTROL_UNKNOWN;
}
