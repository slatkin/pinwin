/*
 * control.h - the per-instance control API (add-pinwin-control): parsing of
 * the line requests a client sends over the instance's Unix socket.
 *
 * pinwin_control_parse is pure C (no GTK, no GLib) so the lightweight check
 * in tools/ covers it standalone; the socket itself lives in glue.c.
 */
#ifndef PINWIN_CONTROL_H
#define PINWIN_CONTROL_H

#include <stddef.h>

/* One request line, without its terminating '\n'. */
typedef enum {
    PINWIN_CONTROL_OPTIONS, /* open the options window */
    PINWIN_CONTROL_UNKNOWN  /* anything else: reply "error unknown request" */
} PinwinControlRequest;

/* Parse one request line. A single trailing '\n' is ignored; anything that
 * is not exactly "options" is UNKNOWN (including the empty line). */
PinwinControlRequest pinwin_control_parse(const char* line, size_t len);

#endif /* PINWIN_CONTROL_H */
