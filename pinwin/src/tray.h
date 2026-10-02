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

#endif /* PINWIN_TRAY_H */
