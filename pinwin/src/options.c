/*
 * options.c - pinwin's layout settings: values, validation, GKeyFile
 * persistence and the options window. Kept out of glue.c so the renderer /
 * PTY file does not also own the settings editor (design D1).
 *
 * The value/parse/validate core is plain C (no GLib) so the lightweight
 * check in tools/ compiles it standalone; only config I/O and the GTK
 * window below use GLib/GTK.
 */

#include "options.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

/* ---- GKeyFile persistence (design D4) ----------------------------------- */

#include <glib.h>

/* Result codes for pinwin_config_load. */
#define PINWIN_CONFIG_LOADED 0
#define PINWIN_CONFIG_ABSENT 1
#define PINWIN_CONFIG_INVALID 2

/* Load the saved layout from `$XDG_CONFIG_HOME/pinwin/config` (default
 * ~/.config/pinwin/config). On PINWIN_CONFIG_LOADED, *out holds a valid
 * layout. PINWIN_CONFIG_ABSENT means no config file exists (normal; the
 * caller keeps its launch baseline). PINWIN_CONFIG_INVALID means the file
 * exists but is unreadable, malformed or geometrically meaningless as a set;
 * a diagnostic has been printed and the caller falls back to the baseline
 * without rewriting the file. Unknown keys are ignored. */
int pinwin_config_load(PinwinLayout* out);

/* Atomically save the layout to the same path: create the directory when
 * needed, write to a temporary file and rename, mode 0600. The previous file
 * is left untouched on any failure. Returns 0 on success; on failure a
 * diagnostic is printed and non-zero returned. */
int pinwin_config_save(const PinwinLayout* layout);

PinwinLayout pinwin_layout_default(int32_t cols, int32_t gutter) {
    PinwinLayout layout;
    layout.side = PINWIN_SIDE_LEFT;
    layout.cols = cols; /* COLS is validated at launch */
    layout.top = 0;
    layout.bottom = 0;
    layout.left = 0;
    layout.right = gutter > 0 ? gutter : 0;
    return layout;
}

/* ---- config load and atomic save ---------------------------------------- */

#include <gtk/gtk.h>

static char* config_file_path(void) {
    return g_build_filename(g_get_user_config_dir(), "pinwin", "config", NULL);
}

static void config_diagnostic(const char* path, const char* detail) {
    fprintf(stderr, "pinwin: config %s: %s; using launch defaults\n", path, detail);
}

int pinwin_config_load(PinwinLayout* out) {
    GKeyFile* kf;
    GError* err = NULL;
    char* path;
    gchar* side = NULL;
    gchar* cols_text = NULL;
    gchar* values[4] = {NULL, NULL, NULL, NULL};
    int result = PINWIN_CONFIG_INVALID;
    int i;
    static const char* names[4] = {"top", "bottom", "left", "right"};

    path = config_file_path();
    kf = g_key_file_new();
    if (!g_key_file_load_from_file(kf, path, G_KEY_FILE_NONE, &err)) {
        if (err->code == G_KEY_FILE_ERROR_NOT_FOUND ||
            g_error_matches(err, G_FILE_ERROR, G_FILE_ERROR_NOENT)) {
            result = PINWIN_CONFIG_ABSENT; /* normal; not a diagnostic */
        } else {
            config_diagnostic(path, err->message);
        }
        g_error_free(err);
        goto done;
    }

    side = g_key_file_get_string(kf, "layout", "side", &err);
    if (err) goto invalid;
    if (strcmp(side, "left") == 0) {
        out->side = PINWIN_SIDE_LEFT;
    } else if (strcmp(side, "right") == 0) {
        out->side = PINWIN_SIDE_RIGHT;
    } else {
        config_diagnostic(path, "unknown side");
        goto done;
    }

    /* cols is optional: an absent key leaves the caller's launch COLS in
     * place, a malformed one rejects the whole layout like a gutter. */
    cols_text = g_key_file_get_string(kf, "layout", "cols", &err);
    if (err) {
        if (g_error_matches(err, G_KEY_FILE_ERROR, G_KEY_FILE_ERROR_KEY_NOT_FOUND) ||
            g_error_matches(err, G_KEY_FILE_ERROR, G_KEY_FILE_ERROR_GROUP_NOT_FOUND)) {
            g_clear_error(&err);
        } else {
            goto invalid;
        }
    } else if (!pinwin_parse_cols(cols_text, &out->cols)) {
        config_diagnostic(path, "invalid cols");
        goto done;
    }
    g_free(cols_text);
    cols_text = NULL;

    for (i = 0; i < 4; i++) {
        values[i] = g_key_file_get_string(kf, "layout", names[i], &err);
        if (err) goto invalid;
    }
    if (!pinwin_parse_gutter(values[0], &out->top) ||
        !pinwin_parse_gutter(values[1], &out->bottom) ||
        !pinwin_parse_gutter(values[2], &out->left) ||
        !pinwin_parse_gutter(values[3], &out->right)) {
        /* Parse all four before assigning any: an unusable file must not
         * leave a half-written layout behind. */
        config_diagnostic(path, "invalid gutter");
        goto done;
    }
    g_key_file_free(kf);
    g_free(path);
    return PINWIN_CONFIG_LOADED;

invalid:
    config_diagnostic(path, err->message);
    g_error_free(err);
done:
    g_free(side);
    g_free(cols_text);
    for (i = 0; i < 4; i++) g_free(values[i]);
    g_key_file_free(kf);
    g_free(path);
    return result;
}

int pinwin_config_save(const PinwinLayout* layout) {
    GKeyFile* kf;
    GError* err = NULL;
    char* dir;
    char* path;
    gchar* data;
    gsize len;
    const char* side;

    kf = g_key_file_new();
    side = layout->side == PINWIN_SIDE_RIGHT ? "right" : "left";
    g_key_file_set_string(kf, "layout", "side", side);
    g_key_file_set_integer(kf, "layout", "cols", layout->cols);
    g_key_file_set_integer(kf, "layout", "top", layout->top);
    g_key_file_set_integer(kf, "layout", "bottom", layout->bottom);
    g_key_file_set_integer(kf, "layout", "left", layout->left);
    g_key_file_set_integer(kf, "layout", "right", layout->right);
    data = g_key_file_to_data(kf, &len, NULL);
    g_key_file_free(kf);
    if (!data) return 1;

    dir = g_build_filename(g_get_user_config_dir(), "pinwin", NULL);
    if (g_mkdir_with_parents(dir, S_IRWXU) < 0) {
        fprintf(stderr, "pinwin: config %s: %s\n", dir, g_strerror(errno));
        g_free(dir);
        g_free(data);
        return 1;
    }
    g_free(dir);

    path = config_file_path();
    /* Atomic replace: g_file_set_contents_full writes a temporary file and
     * renames it, so a failure leaves the previous config intact. */
    if (!g_file_set_contents_full(path, data, len, G_FILE_SET_CONTENTS_DURABLE,
                                  S_IRUSR | S_IWUSR, &err)) {
        fprintf(stderr, "pinwin: config %s: %s\n", path, err->message);
        g_error_free(err);
        g_free(path);
        g_free(data);
        return 1;
    }
    g_free(path);
    g_free(data);
    return 0;
}

/* ---- options window (design D6) ------------------------------------------ */

/* One normal GTK window per process, created on first use. Its spin buttons
 * ARE the draft: edits change nothing until Apply validates, saves and
 * publishes; the window is destroyed on close, which discards the draft, and
 * a fresh open re-initialises from the applied layout. */
static GtkWindow* g_options_window;
static GtkWidget* g_cols_spin;     /* panel width in terminal columns */
static GtkWidget* g_spin[4];        /* top, bottom, left, right */
static GtkWidget* g_side[2];        /* Left, Right check buttons */
static GtkWidget* g_error_label;

static const char* field_names[4] = {"Top", "Bottom", "Left", "Right"};

static void options_show_error(const char* text) {
    gtk_label_set_text(GTK_LABEL(g_error_label), text ? text : "");
}

static void options_reset_from_applied(void) {
    PinwinLayout l;
    int i;

    glue_current_layout(&l);
    gtk_spin_button_set_value(GTK_SPIN_BUTTON(g_cols_spin), (double)l.cols);
    for (i = 0; i < 4; i++) {
        int32_t value = (&l.top)[i];
        gtk_spin_button_set_value(GTK_SPIN_BUTTON(g_spin[i]), (double)value);
    }
    gtk_check_button_set_active(GTK_CHECK_BUTTON(g_side[l.side]), TRUE);
    options_show_error(NULL);
}

static void on_options_destroy(GtkWidget* widget, gpointer user_data) {
    (void)widget;
    (void)user_data;
    if (getenv("PINWIN_DEBUG"))
        fprintf(stderr, "pinwin: options window destroyed\n");
    g_options_window = NULL;
}

static void on_options_close_clicked(GtkButton* button, gpointer user_data) {
    (void)button;
    (void)user_data;
    gtk_window_destroy(g_options_window);
}

static void on_options_apply_clicked(GtkButton* button, gpointer user_data) {
    PinwinLayout l;
    int32_t cols, cell_w, cell_h, output_w, output_h;
    int i;

    (void)button;
    (void)user_data;

    /* Strict parsing of the typed text: GTK spin buttons allow arbitrary
     * input, and Apply must not silently clamp negative, fractional or
     * malformed values (design D6). */
    if (!pinwin_parse_cols(gtk_editable_get_text(GTK_EDITABLE(g_cols_spin)),
                            &l.cols)) {
        options_show_error("Columns: enter a whole number of terminal columns "
                           "between 1 and 65535");
        return;
    }
    for (i = 0; i < 4; i++) {
        const char* text = gtk_editable_get_text(GTK_EDITABLE(g_spin[i]));
        int32_t value;
        if (!pinwin_parse_gutter(text, &value)) {
            char* msg = g_strdup_printf("%s: enter a whole number of pixels "
                                        "(negative values push that edge past "
                                        "the screen)",
                                        field_names[i]);
            options_show_error(msg);
            g_free(msg);
            return;
        }
        (&l.top)[i] = value;
    }
    l.side = gtk_check_button_get_active(GTK_CHECK_BUTTON(g_side[PINWIN_SIDE_RIGHT]))
                 ? PINWIN_SIDE_RIGHT
                 : PINWIN_SIDE_LEFT;

    /* Geometry against the instance's original monitor; the staged column
     * count is the panel width validated here. */
    glue_layout_metrics(&cols, &cell_w, &cell_h, &output_w, &output_h);
    {
        int r = pinwin_layout_validate(&l, l.cols, cell_w, cell_h, output_w, output_h);
        if (r == PINWIN_GEOM_ERR_NO_RESERVE) {
            options_show_error("Left + panel width + Right is negative, so there "
                               "is no strip left to reserve");
            return;
        }
        if (r == PINWIN_GEOM_ERR_NO_WIDTH) {
            options_show_error("This layout would leave no horizontal space "
                               "for other windows on the panel's monitor");
            return;
        }
        if (r == PINWIN_GEOM_ERR_NO_ROW) {
            options_show_error("Top and Bottom leave less than one terminal row "
                               "of height on the panel's monitor");
            return;
        }
        if (r != PINWIN_GEOM_OK) {
            options_show_error("These values cannot be applied to the panel's "
                               "monitor");
            return;
        }
    }

    /* Save first: a failed save must leave both the live layout and the
     * previous config unchanged (design D6). */
    if (pinwin_config_save(&l) != 0) {
        options_show_error("Could not save the settings "
                           "(is the config directory writable?)");
        return;
    }

    /* Publish: validated again inside glue.c, then applied to both surfaces
     * and the terminal grid. */
    if (glue_publish_layout(&l) != PINWIN_GEOM_OK) {
        options_show_error("Could not apply the layout to the panel");
        return;
    }

    options_show_error(NULL);
    /* The draft's baseline is now the applied layout; keep the window open. */
}

void pinwin_options_open(void) {
    GtkWidget* win;
    GtkWidget* grid;
    GtkWidget* apply;
    GtkWidget* close;
    int i;

    if (g_options_window) {
        gtk_window_present(g_options_window);
        return;
    }

    win = gtk_window_new();
    g_options_window = GTK_WINDOW(win);
    gtk_window_set_application(GTK_WINDOW(win),
                               GTK_APPLICATION(g_application_get_default()));
    gtk_window_set_title(GTK_WINDOW(win), "pinwin options");
    g_signal_connect(win, "destroy", G_CALLBACK(on_options_destroy), NULL);

    grid = gtk_grid_new();
    gtk_grid_set_row_spacing(GTK_GRID(grid), 6);
    gtk_grid_set_column_spacing(GTK_GRID(grid), 12);
    gtk_window_set_child(GTK_WINDOW(win), grid);

    /* Columns first: it is the panel width and it changes what the gutter
     * validation runs against (design D6). */
    {
        GtkWidget* label = gtk_label_new_with_mnemonic("_Columns");
        gtk_label_set_xalign(GTK_LABEL(label), 1.0);
        g_cols_spin = gtk_spin_button_new_with_range(1.0, 65535.0, 1.0);
        gtk_spin_button_set_digits(GTK_SPIN_BUTTON(g_cols_spin), 0);
        gtk_spin_button_set_update_policy(GTK_SPIN_BUTTON(g_cols_spin),
                                          GTK_UPDATE_IF_VALID);
        gtk_label_set_mnemonic_widget(GTK_LABEL(label), g_cols_spin);
        gtk_grid_attach(GTK_GRID(grid), label, 0, 0, 1, 1);
        gtk_grid_attach(GTK_GRID(grid), g_cols_spin, 1, 0, 2, 1);
    }

    /* Side selection. */
    gtk_grid_attach(GTK_GRID(grid), gtk_label_new("Dock to"), 0, 1, 1, 1);
    g_side[PINWIN_SIDE_LEFT] = gtk_check_button_new_with_mnemonic("_Left");
    g_side[PINWIN_SIDE_RIGHT] =
        gtk_check_button_new_with_mnemonic("_Right");
    gtk_check_button_set_group(GTK_CHECK_BUTTON(g_side[PINWIN_SIDE_RIGHT]),
                               GTK_CHECK_BUTTON(g_side[PINWIN_SIDE_LEFT]));
    gtk_grid_attach(GTK_GRID(grid), g_side[PINWIN_SIDE_LEFT], 1, 1, 1, 1);
    gtk_grid_attach(GTK_GRID(grid), g_side[PINWIN_SIDE_RIGHT], 2, 1, 1, 1);

    /* Gutter fields; labels are mnemonics for the matching spin button. */
    for (i = 0; i < 4; i++) {
        GtkWidget* label = gtk_label_new_with_mnemonic("");
        char* text = g_strdup_printf("_%s", field_names[i]);
        g_object_set(label, "label", text, NULL);
        g_free(text);
        gtk_label_set_use_underline(GTK_LABEL(label), TRUE);
        gtk_label_set_xalign(GTK_LABEL(label), 1.0);
        g_spin[i] = gtk_spin_button_new_with_range((double)INT32_MIN,
                                               (double)INT32_MAX, 1.0);
        gtk_spin_button_set_digits(GTK_SPIN_BUTTON(g_spin[i]), 0);
        gtk_spin_button_set_update_policy(GTK_SPIN_BUTTON(g_spin[i]),
                                          GTK_UPDATE_IF_VALID);
        gtk_label_set_mnemonic_widget(GTK_LABEL(label), g_spin[i]);
        gtk_grid_attach(GTK_GRID(grid), label, 0, i + 2, 1, 1);
        gtk_grid_attach(GTK_GRID(grid), g_spin[i], 1, i + 2, 2, 1);
    }

    /* Inline error display. */
    g_error_label = gtk_label_new("");
    gtk_label_set_wrap(GTK_LABEL(g_error_label), TRUE);
    gtk_widget_set_hexpand(g_error_label, TRUE);
    gtk_widget_set_visible(g_error_label, FALSE);
    gtk_grid_attach(GTK_GRID(grid), g_error_label, 0, 6, 3, 1);

    /* Apply and Close. */
    {
        GtkWidget* buttons = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
        apply = gtk_button_new_with_mnemonic("_Apply");
        close = gtk_button_new_with_mnemonic("_Close");
        g_signal_connect(apply, "clicked", G_CALLBACK(on_options_apply_clicked), NULL);
        g_signal_connect(close, "clicked", G_CALLBACK(on_options_close_clicked), NULL);
        gtk_box_append(GTK_BOX(buttons), close);
        gtk_box_append(GTK_BOX(buttons), apply);
        gtk_grid_attach(GTK_GRID(grid), buttons, 0, 7, 3, 1);
        gtk_window_set_default_widget(GTK_WINDOW(win), apply);
    }

    options_reset_from_applied();
    gtk_window_present(GTK_WINDOW(win));
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
