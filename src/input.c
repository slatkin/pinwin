/*
 * input.c - the GDK input side of pinwin: the controllers attached to the
 * drawing area for key, mouse, scroll and focus, and their translation into
 * the pinwin_* events main.zig encodes for the terminal (design D4).
 *
 * Shared state lives in glue_internal.h.
 */

#include "glue_internal.h"

static uint32_t mods_from_gdk(GdkModifierType state) {
    uint32_t mods = 0;
    if (state & GDK_SHIFT_MASK) mods |= PINWIN_MOD_SHIFT;
    if (state & GDK_CONTROL_MASK) mods |= PINWIN_MOD_CTRL;
    if (state & GDK_ALT_MASK) mods |= PINWIN_MOD_ALT;
    if (state & GDK_SUPER_MASK) mods |= PINWIN_MOD_SUPER;
    if (state & GDK_LOCK_MASK) mods |= PINWIN_MOD_CAPS_LOCK;
    return mods;
}

static int32_t pinwin_button_from_gdk(guint button) {
    switch (button) {
        case GDK_BUTTON_PRIMARY:
            return PINWIN_MOUSE_LEFT;
        case GDK_BUTTON_MIDDLE:
            return PINWIN_MOUSE_MIDDLE;
        case GDK_BUTTON_SECONDARY:
            return PINWIN_MOUSE_RIGHT;
        case 8:
            return PINWIN_MOUSE_EIGHT;
        case 9:
            return PINWIN_MOUSE_NINE;
        default:
            return PINWIN_MOUSE_UNKNOWN;
    }
}

uint32_t glue_keyval_unicode(uint32_t keyval) {
    return (uint32_t)gdk_keyval_to_unicode(keyval);
}

uint32_t glue_keycode_unshifted_codepoint(uint32_t keycode) {
    GdkDisplay* display = gdk_display_get_default();
    GdkKeymapKey* keys = NULL;
    guint* keyvals = NULL;
    gint n_entries = 0;
    uint32_t result = 0;

    if (!display) return 0;
    if (!gdk_display_map_keycode(display, keycode, &keys, &keyvals, &n_entries))
        return 0;

    for (gint i = 0; i < n_entries; i++) {
        if (keys[i].group == g_key_layout && keys[i].level == 0) {
            result = (uint32_t)gdk_keyval_to_unicode(keyvals[i]);
            break;
        }
    }
    if (result == 0) {
        for (gint i = 0; i < n_entries; i++) {
            if (keys[i].level == 0) {
                result = (uint32_t)gdk_keyval_to_unicode(keyvals[i]);
                break;
            }
        }
    }

    g_free(keys);
    g_free(keyvals);
    return result;
}

static gboolean on_key(GtkEventControllerKey* controller, guint keyval,
                       guint keycode, GdkModifierType state, int action) {
    GdkEvent* event = gtk_event_controller_get_current_event(
        GTK_EVENT_CONTROLLER(controller));
    uint32_t consumed = 0;
    int is_modifier = 0;

    if (event) {
        consumed = mods_from_gdk(
            gdk_key_event_get_consumed_modifiers(event));
        is_modifier = gdk_key_event_is_modifier(event);
        if (action == PINWIN_KEY_PRESS)
            g_key_layout = (int)gdk_key_event_get_layout(event);
    }
    pinwin_key(action, (int32_t)keyval, (int32_t)keycode,
               mods_from_gdk(state), consumed, is_modifier);
    return TRUE;
}

static gboolean on_key_pressed(GtkEventControllerKey* controller, guint keyval,
                               guint keycode, GdkModifierType state,
                               gpointer user_data) {
    (void)user_data;
    return on_key(controller, keyval, keycode, state, PINWIN_KEY_PRESS);
}

static gboolean on_key_released(GtkEventControllerKey* controller, guint keyval,
                                guint keycode, GdkModifierType state,
                                gpointer user_data) {
    (void)user_data;
    return on_key(controller, keyval, keycode, state, PINWIN_KEY_RELEASE);
}

static uint32_t motion_mods(GtkEventController* controller) {
    GdkModifierType state = gtk_event_controller_get_current_event_state(controller);
    return mods_from_gdk(state);
}

static void on_click_pressed(GtkGestureClick* gesture, int n_press, double x,
                             double y, gpointer user_data) {
    guint button;
    (void)n_press;
    (void)user_data;
    x -= glue_anim_draw_offset();
    g_last_x = x;
    g_last_y = y;
    button = gtk_gesture_single_get_current_button(GTK_GESTURE_SINGLE(gesture));
    pinwin_mouse(PINWIN_MOUSE_PRESS, x, y, pinwin_button_from_gdk(button),
                  motion_mods(GTK_EVENT_CONTROLLER(gesture)));
}

static void on_click_released(GtkGestureClick* gesture, int n_press, double x,
                              double y, gpointer user_data) {
    guint button;
    (void)n_press;
    (void)user_data;
    x -= glue_anim_draw_offset();
    button = gtk_gesture_single_get_current_button(GTK_GESTURE_SINGLE(gesture));
    pinwin_mouse(PINWIN_MOUSE_RELEASE, x, y, pinwin_button_from_gdk(button),
                  motion_mods(GTK_EVENT_CONTROLLER(gesture)));
}

static void on_motion(GtkEventControllerMotion* controller, double x, double y,
                      gpointer user_data) {
    (void)user_data;
    x -= glue_anim_draw_offset();
    g_last_x = x;
    g_last_y = y;
    pinwin_mouse(PINWIN_MOUSE_MOTION, x, y, PINWIN_MOUSE_UNKNOWN,
                  motion_mods(GTK_EVENT_CONTROLLER(controller)));
}

static gboolean on_scroll(GtkEventControllerScroll* controller, double dx, double dy,
                          gpointer user_data) {
    GdkModifierType state =
        gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(controller));
    (void)user_data;
    pinwin_scroll(g_last_x, g_last_y, dx, dy,
                   (int32_t)gtk_event_controller_scroll_get_unit(controller),
                   mods_from_gdk(state));
    return TRUE;
}

static void on_focus_enter(GtkEventControllerFocus* controller, gpointer user_data) {
    (void)controller;
    (void)user_data;
    g_focused = 1;
    gtk_widget_queue_draw(GTK_WIDGET(g_area));
    pinwin_focus(1);
}

static void on_focus_leave(GtkEventControllerFocus* controller, gpointer user_data) {
    (void)controller;
    (void)user_data;
    g_focused = 0;
    gtk_widget_queue_draw(GTK_WIDGET(g_area));
    pinwin_focus(0);
}

void attach_controllers(GtkWidget* area) {
    GtkEventController* key = gtk_event_controller_key_new();
    GtkGesture* click = gtk_gesture_click_new();
    GtkEventController* motion = gtk_event_controller_motion_new();
    /* The flags gate the axes: DISCRETE alone delivers no scroll events.
     * DISCRETE makes a wheel notch exactly one unit, which is what the mouse
     * encoder turns into a wheel button. */
    GtkEventController* scroll = gtk_event_controller_scroll_new(
        GTK_EVENT_CONTROLLER_SCROLL_VERTICAL |
        GTK_EVENT_CONTROLLER_SCROLL_HORIZONTAL);
    GtkEventController* focus = gtk_event_controller_focus_new();

    gtk_event_controller_set_propagation_phase(key, GTK_PHASE_CAPTURE);
    g_signal_connect(key, "key-pressed", G_CALLBACK(on_key_pressed), NULL);
    g_signal_connect(key, "key-released", G_CALLBACK(on_key_released), NULL);
    gtk_widget_add_controller(area, key);

    gtk_gesture_single_set_button(GTK_GESTURE_SINGLE(click), 0);
    g_signal_connect(click, "pressed", G_CALLBACK(on_click_pressed), NULL);
    g_signal_connect(click, "released", G_CALLBACK(on_click_released), NULL);
    gtk_widget_add_controller(area, GTK_EVENT_CONTROLLER(click));

    g_signal_connect(motion, "motion", G_CALLBACK(on_motion), NULL);
    gtk_widget_add_controller(area, motion);

    g_signal_connect(scroll, "scroll", G_CALLBACK(on_scroll), NULL);
    gtk_widget_add_controller(area, scroll);

    g_signal_connect(focus, "enter", G_CALLBACK(on_focus_enter), NULL);
    g_signal_connect(focus, "leave", G_CALLBACK(on_focus_leave), NULL);
    gtk_widget_add_controller(area, focus);
}
