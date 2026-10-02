/*
 * tray.h - this pinwin instance's StatusNotifierItem on the session bus,
 * with a dbusmenu tree exposing "Options..." (add-pinwin-tray-options).
 *
 * Everything here is non-fatal: an absent bus, watcher or host leaves the
 * terminal running with a diagnostic.
 */
#ifndef PINWIN_TRAY_H
#define PINWIN_TRAY_H

#include <glib.h>

/* Publish the tray entry (session bus, StatusNotifierWatcher registration,
 * dbusmenu server). Call once after GTK is up. */
void tray_init(void);

/* Best-effort unregister and release of tray resources (command exit). */
void tray_shutdown(void);

/* Byte conversion for IconPixmap entries: straight RGBA8 pixels become
 * network-order ARGB bytes (A, R, G, B), n_pixels * 4 bytes appended to
 * `out`. Non-static so the lightweight check can cover it (task 4.2). */
void tray_argb_from_rgba(const unsigned char* rgba, int n_pixels,
                          GByteArray* out);

#endif /* PINWIN_TRAY_H */
