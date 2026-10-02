/*
 * tray.c - StatusNotifierItem publication over the session bus (design D2).
 *
 * One bus name per process (org.kde.StatusNotifierItem-<pid>-<n>) so
 * parallel instances do not collide, an org.kde.StatusNotifierItem object
 * whose Menu property points at a libdbusmenu tree, and a single
 * "Options..." item whose activation opens this instance's options window.
 * Right-click menu rendering is the host's job: ContextMenu/Activate only
 * acknowledge the request, they never open options directly.
 *
 * Bus, watcher and host problems are diagnostics, never command failures.
 * The item re-registers when org.kde.StatusNotifierWatcher returns.
 */

#include "tray.h"
#include "options.h"

#include <gio/gio.h>
#include <gdk-pixbuf/gdk-pixbuf.h>
#include <gtk/gtk.h>
#include <libdbusmenu-glib/server.h>
#include <libdbusmenu-glib/menuitem.h>

#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define WATCHER_NAME "org.kde.StatusNotifierWatcher"
#define WATCHER_PATH "/StatusNotifierWatcher"
#define ITEM_PATH "/StatusNotifierItem"
#define MENU_PATH "/StatusNotifierItem/menu"

static const char item_interface_xml[] =
    "<node>"
    "  <interface name='org.kde.StatusNotifierItem'>"
    "    <property name='Category' type='s' access='read'/>"
    "    <property name='Id' type='s' access='read'/>"
    "    <property name='Title' type='s' access='read'/>"
    "    <property name='Status' type='s' access='read'/>"
    "    <property name='WindowId' type='i' access='read'/>"
    "    <property name='IconName' type='s' access='read'/>"
    "    <property name='IconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='Menu' type='o' access='read'/>"
    "    <property name='ItemIsMenu' type='b' access='read'/>"
    "    <method name='Activate'>"
    "      <arg name='x' type='i' direction='in'/>"
    "      <arg name='y' type='i' direction='in'/>"
    "    </method>"
    "    <method name='SecondaryActivate'>"
    "      <arg name='x' type='i' direction='in'/>"
    "      <arg name='y' type='i' direction='in'/>"
    "    </method>"
    "    <method name='ContextMenu'>"
    "      <arg name='x' type='i' direction='in'/>"
    "      <arg name='y' type='i' direction='in'/>"
    "    </method>"
    "    <method name='Scroll'>"
    "      <arg name='delta' type='i' direction='in'/>"
    "      <arg name='orientation' type='s' direction='in'/>"
    "    </method>"
    "  </interface>"
    "</node>";

static guint g_own_id;
static GDBusConnection* g_bus;
static char* g_bus_name;
static gboolean g_registered;
static guint g_watcher_watch_id;
static DbusmenuServer* g_menu_server;
static DbusmenuMenuitem* g_options_item;
static GVariant* g_icon_pixmap; /* a(iiay), built by tray_icon_setup (design D3) */
static char* g_icon_name; /* NULL = empty IconName so hosts prefer pixmaps */

static void tray_diagnostic(const char* what, const char* detail) {
    fprintf(stderr, "pinwin: tray %s%s%s\n", what, detail ? ": " : "",
            detail ? detail : "");
}

/* ---- StatusNotifierItem object ------------------------------------------ */

static GVariant* item_get_property(GDBusConnection* connection,
                                   const gchar* sender,
                                   const gchar* object_path,
                                   const gchar* interface_name,
                                   const gchar* property_name, GError** error,
                                   gpointer user_data) {
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)error;
    (void)user_data;

    if (strcmp(property_name, "Category") == 0)
        return g_variant_new_string("ApplicationStatus");
    if (strcmp(property_name, "Id") == 0 || strcmp(property_name, "Title") == 0)
        return g_variant_new_string("pinwin");
    if (strcmp(property_name, "Status") == 0) return g_variant_new_string("Active");
    if (strcmp(property_name, "WindowId") == 0) return g_variant_new_int32(0);
    if (strcmp(property_name, "IconName") == 0)
        return g_variant_new_string(g_icon_name ? g_icon_name : "");
    if (strcmp(property_name, "IconPixmap") == 0) {
        if (g_icon_pixmap) return g_variant_ref(g_icon_pixmap);
        return g_variant_new("(ii@ay)", 0, 0,
                             g_variant_new_from_data(G_VARIANT_TYPE("ay"), "",
                                                     0, TRUE, NULL, NULL));
    }
    if (strcmp(property_name, "Menu") == 0)
        return g_variant_new_object_path(MENU_PATH);
    if (strcmp(property_name, "ItemIsMenu") == 0)
        return g_variant_new_boolean(TRUE);
    return NULL;
}

/* The host renders the menu itself; these SNI methods only acknowledge. */
static void item_method_call(GDBusConnection* connection, const gchar* sender,
                             const gchar* object_path,
                             const gchar* interface_name,
                             const gchar* method_name, GVariant* parameters,
                             GDBusMethodInvocation* invocation,
                             gpointer user_data) {
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)parameters;
    (void)user_data;
    if (strcmp(method_name, "Activate") == 0 ||
        strcmp(method_name, "SecondaryActivate") == 0 ||
        strcmp(method_name, "ContextMenu") == 0 ||
        strcmp(method_name, "Scroll") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_DBUS_ERROR,
                                          G_DBUS_ERROR_UNKNOWN_METHOD,
                                          "unknown method %s", method_name);
}

static const GDBusInterfaceVTable item_vtable = {
    item_method_call, item_get_property, NULL,
    {NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL},
};

/* ---- dbusmenu tree ------------------------------------------------------- */

static void on_options_item_activated(DbusmenuMenuitem* item, guint timestamp,
                                      gpointer user_data) {
    (void)item;
    (void)timestamp;
    (void)user_data;
    pinwin_options_open();
}

static void menu_server_setup(GDBusConnection* bus) {
    DbusmenuMenuitem* root;

    (void)bus;
    g_menu_server = dbusmenu_server_new(MENU_PATH);
    root = dbusmenu_menuitem_new();
    g_options_item = dbusmenu_menuitem_new();
    dbusmenu_menuitem_property_set(g_options_item, DBUSMENU_MENUITEM_PROP_LABEL,
                                   "Options...");
    dbusmenu_menuitem_child_append(root, g_options_item);
    g_signal_connect(g_options_item, "item-activated",
                     G_CALLBACK(on_options_item_activated), NULL);
    dbusmenu_server_set_root(g_menu_server, root);
    g_object_unref(root); /* the server holds its own reference */
}

/* ---- watcher registration ------------------------------------------------ */

static void on_register_finished(GObject* source, GAsyncResult* result,
                                 gpointer user_data) {
    GDBusConnection* connection = G_DBUS_CONNECTION(source);
    GError* err = NULL;
    (void)user_data;
    if (g_dbus_connection_call_finish(connection, result, &err)) {
        g_registered = TRUE;
    } else {
        g_registered = FALSE;
        tray_diagnostic("registration with the tray host failed", err->message);
        g_error_free(err);
    }
}

static void watcher_register(GDBusConnection* bus) {
    g_dbus_connection_call(
        bus, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME,
        "RegisterStatusNotifierItem", g_variant_new("(s)", g_bus_name), NULL,
        G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_register_finished, NULL);
}

static void on_watcher_appeared(GDBusConnection* bus, const gchar* name,
                                const gchar* owner, gpointer user_data) {
    (void)name;
    (void)owner;
    (void)user_data;
    /* Re-register whenever the watcher shows up, including a restart. */
    if (bus) watcher_register(bus);
}

static void on_watcher_vanished(GDBusConnection* bus, const gchar* name,
                                gpointer user_data) {
    (void)bus;
    (void)name;
    (void)user_data;
    if (g_registered) {
        g_registered = FALSE;
        tray_diagnostic("tray host left; the panel stays up", NULL);
    }}

/* ---- bus name ------------------------------------------------------------ */

static void on_bus_acquired(GDBusConnection* bus, const gchar* name,
                            gpointer user_data) {
    GError* err = NULL;
    GDBusNodeInfo* info;
    guint id;

    (void)name;
    (void)user_data;

    info = g_dbus_node_info_new_for_xml(item_interface_xml, &err);
    if (!info) {
        tray_diagnostic("interface introspection parse failed", err->message);
        g_error_free(err);
        return;
    }
    id = g_dbus_connection_register_object(
        bus, ITEM_PATH, info->interfaces[0], &item_vtable, NULL, NULL, &err);
    g_dbus_node_info_unref(info);
    if (id == 0) {
        tray_diagnostic("could not export the tray item", err->message);
        g_error_free(err);
        return;
    }

    g_bus = g_object_ref(bus);
    menu_server_setup(bus);
    /* Registration happens via the watcher watch: g_bus_watch_name fires
     * appeared immediately when the watcher already owns the name, so a
     * live host gets exactly one registration. */
}

static void on_name_acquired(GDBusConnection* bus, const gchar* name,
                             gpointer user_data) {
    (void)bus;
    (void)name;
    (void)user_data;
    /* Watch for the watcher only once our name exists. */
    g_watcher_watch_id = g_bus_watch_name(
        G_BUS_TYPE_SESSION, WATCHER_NAME, G_BUS_NAME_WATCHER_FLAGS_NONE,
        on_watcher_appeared, on_watcher_vanished, NULL, NULL);
}

static void on_name_lost(GDBusConnection* bus, const gchar* name,
                         gpointer user_data) {
    (void)name;
    (void)user_data;
    if (!bus) {
        tray_diagnostic("no session bus; running without a tray entry", NULL);
    } else {
        tray_diagnostic("tray bus name unavailable; running without a tray entry",
                        NULL);
    }
}

/* ---- icon (task 4.2 fills this) ------------------------------------------ */

/* ---- icon (design D3) ---------------------------------------------------- */

/* Straight RGBA8 -> network-order ARGB bytes (A, R, G, B), the IconPixmap
 * wire format. Not premultiplied; hosts composite them themselves. */
void tray_argb_from_rgba(const unsigned char* rgba, int n_pixels,
                         GByteArray* out) {
    int i;
    for (i = 0; i < n_pixels; i++) {
        g_byte_array_append(out, rgba + 3, 1); /* A */
        g_byte_array_append(out, rgba + 0, 1); /* R */
        g_byte_array_append(out, rgba + 1, 1); /* G */
        g_byte_array_append(out, rgba + 2, 1); /* B */
        rgba += 4;
    }
}

/* The tray icon at one standard size: the SVG raster scaled to fit and
 * centred on a transparent square. Returns (iiay) or NULL on OOM. */
static GVariant* pixmap_entry(GdkPixbuf* svg, int size) {
    GdkPixbuf* canvas;
    GdkPixbuf* scaled;
    GByteArray* bytes;
    GVariant* bytes_variant;
    GVariant* entry;
    const guint8* pixels;
    int stride, w, h, dw, dh, y;
    double scale;

    w = gdk_pixbuf_get_width(svg);
    h = gdk_pixbuf_get_height(svg);
    if (w < 1 || h < 1) return NULL;
    scale = (double)size / (double)(w > h ? w : h);
    dw = (int)(w * scale + 0.5);
    dh = (int)(h * scale + 0.5);
    if (dw < 1) dw = 1;
    if (dh < 1) dh = 1;
    if (dw > size) dw = size;
    if (dh > size) dh = size;

    scaled = gdk_pixbuf_scale_simple(svg, dw, dh, GDK_INTERP_BILINEAR);
    canvas = gdk_pixbuf_new(GDK_COLORSPACE_RGB, TRUE, 8, size, size);
    if (!scaled || !canvas) {
        g_clear_object(&scaled);
        g_clear_object(&canvas);
        return NULL;
    }
    gdk_pixbuf_fill(canvas, 0x00000000);
    gdk_pixbuf_composite(scaled, canvas, (size - dw) / 2, (size - dh) / 2,
                         dw, dh, (double)((size - dw) / 2),
                         (double)((size - dh) / 2), 1.0, 1.0,
                         GDK_INTERP_BILINEAR, 255);
    g_object_unref(scaled);

    pixels = gdk_pixbuf_read_pixels(canvas);
    stride = gdk_pixbuf_get_rowstride(canvas);
    bytes = g_byte_array_new();
    for (y = 0; y < size; y++)
        tray_argb_from_rgba(pixels + (gsize)y * (gsize)stride, size, bytes);

    bytes_variant = g_variant_new_from_data(
        G_VARIANT_TYPE("ay"), bytes->data, bytes->len, TRUE,
        (GDestroyNotify)g_byte_array_unref, g_byte_array_ref(bytes));

    g_byte_array_unref(bytes);
    if (!bytes_variant) {
        g_object_unref(canvas);
        return NULL;
    }
    entry = g_variant_new("(ii@ay)", size, size, bytes_variant);
    g_object_unref(canvas);
    return entry;
}

/* Rasterise $HOME/pinwin.svg at the standard tray sizes. Missing or
 * unloadable artwork is a diagnostic plus a generic fallback, never a
 * terminal failure (spec: Missing icon). */
static void tray_icon_setup(void) {
    static const int sizes[] = {16, 22, 32, 48};
    GdkPixbufLoader* loader;
    GdkPixbuf* svg = NULL;
    char* path;
    gchar* data = NULL;
    gsize len = 0;
    GError* err = NULL;
    GVariantBuilder builder;
    GVariant* pixmaps;
    int i;

    path = g_build_filename(g_get_home_dir(), "pinwin.svg", NULL);
    if (g_file_get_contents(path, &data, &len, NULL)) {
        loader = gdk_pixbuf_loader_new_with_type("svg", &err);
        if (loader) {
            if (gdk_pixbuf_loader_write(loader, (const guint8*)data, len, &err) &&
                gdk_pixbuf_loader_close(loader, &err)) {
                svg = gdk_pixbuf_loader_get_pixbuf(loader);
                if (svg) svg = g_object_ref(svg);
            }
            g_object_unref(loader);
        }
        g_free(data);
    } else {
        err = g_error_new(G_FILE_ERROR, G_FILE_ERROR_NOENT, "%s does not exist",
                          path);
    }

    if (svg) {
        g_variant_builder_init(&builder, G_VARIANT_TYPE("a(iiay)"));
        for (i = 0; i < (int)(sizeof(sizes) / sizeof(sizes[0])); i++) {
            GVariant* entry = pixmap_entry(svg, sizes[i]);
            if (entry) g_variant_builder_add_value(&builder, entry);
        }
        g_object_unref(svg);
        pixmaps = g_variant_builder_end(&builder);
        g_icon_pixmap = g_variant_ref_sink(pixmaps);
        g_icon_name = NULL; /* empty IconName: hosts prefer the pixmaps */
        g_free(path);
        return;
    }

    tray_diagnostic("icon", err ? err->message : "could not rasterise");
    tray_diagnostic("icon fallback", "using the terminal theme icon; install "
                                      "artwork at $HOME/pinwin.svg to replace it");
    if (err) g_error_free(err);

    /* Generic theme icon; if even that is unavailable, a simple in-memory
     * pixmap so the entry is never blank. */
    if (gtk_icon_theme_has_icon(
            gtk_icon_theme_get_for_display(gdk_display_get_default()),
            "utilities-terminal")) {
        g_icon_name = g_strdup("utilities-terminal");
        g_free(path);
        return;
    }
    {
        GdkPixbuf* fallback = gdk_pixbuf_new(GDK_COLORSPACE_RGB, TRUE, 8, 16, 16);
        if (fallback) {
            gdk_pixbuf_fill(fallback, 0x000000E0); /* opaque dark square */
            g_variant_builder_init(&builder, G_VARIANT_TYPE("a(iiay)"));
            {
                GVariant* entry = pixmap_entry(fallback, 16);
                if (entry) g_variant_builder_add_value(&builder, entry);
            }
            g_object_unref(fallback);
            g_icon_pixmap = g_variant_ref_sink(g_variant_builder_end(&builder));
            g_icon_name = NULL;
        }
    }
    g_free(path);
}

/* ---- lifecycle ------------------------------------------------------------ */

void tray_init(void) {
    static int instance_counter;

    if (g_own_id) return;
    tray_icon_setup();
    instance_counter++;
    g_bus_name = g_strdup_printf("org.kde.StatusNotifierItem-%d-%d", getpid(),
                                 instance_counter);
    g_own_id = g_bus_own_name(G_BUS_TYPE_SESSION, g_bus_name,
                              G_BUS_NAME_OWNER_FLAGS_NONE, on_bus_acquired,
                              on_name_acquired, on_name_lost, NULL, NULL);
}

void tray_shutdown(void) {
    if (g_watcher_watch_id) {
        g_bus_unwatch_name(g_watcher_watch_id);
        g_watcher_watch_id = 0;
    }
    if (g_registered && g_bus) {
        g_dbus_connection_call_sync(
            g_bus, WATCHER_NAME, WATCHER_PATH, WATCHER_NAME,
            "UnregisterStatusNotifierItem", g_variant_new("(s)", g_bus_name),
            NULL, G_DBUS_CALL_FLAGS_NONE, 1000, NULL, NULL);
        g_registered = FALSE;
    }
    if (g_own_id) {
        g_bus_unown_name(g_own_id);
        g_own_id = 0;
    }
    g_clear_object(&g_menu_server);
    g_clear_object(&g_options_item);
    g_clear_object(&g_bus);
    g_clear_pointer(&g_bus_name, g_free);
    g_clear_pointer(&g_icon_name, g_free);
    g_clear_pointer(&g_icon_pixmap, g_variant_unref);
}
