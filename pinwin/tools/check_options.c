/*
 * check_options.c - the lightweight assertion-based check for pinwin's
 * layout core (task 1.1/1.2 of add-pinwin-tray-options). No GTK, only
 * GLib (for the config round-trip cases).
 *
 * Exact invocation, from the repository root:
 *
 *   cc -std=gnu11 -Wall -Wextra pinwin/src/options.c pinwin/tools/check_options.c \
 *     $(pkg-config --cflags --libs gtk4) -o /tmp/pinwin-check-options \
 *     && /tmp/pinwin-check-options
 */

#include "../src/options.h"
#include "../src/tray.h"

#include <glib.h>

#include <assert.h>
#include <stdio.h>
#include <sys/stat.h>
#include <unistd.h>

/* The GTK options editor (compiled as part of options.c) calls these glue.c
 * functions; the check never opens the window, so inert stubs keep it
 * linkable without glue.c and the whole GTK/PTY stack. */
void glue_current_layout(PinwinLayout* out) { *out = pinwin_layout_default(40, 0); }
void glue_layout_metrics(int32_t* cols, int32_t* cell_w, int32_t* cell_h,
                         int32_t* output_w, int32_t* output_h) {
    *cols = 1; *cell_w = 1; *cell_h = 1; *output_w = 1; *output_h = 1;
}
int glue_publish_layout(const PinwinLayout* layout) { (void)layout; return PINWIN_GEOM_OK; }

static int failures;

#define CHECK(cond)                                                          \
    do {                                                                     \
        if (!(cond)) {                                                       \
            fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);  \
            failures++;                                                      \
        }                                                                    \
    } while (0)

/* glib caches the XDG config dir on first use, so the whole check runs
 * against one temporary config directory and resets its state in place. */
static char* config_dir;

static void use_config_dir(void) {
    GError* err = NULL;
    config_dir = g_dir_make_tmp("pinwin-check-XXXXXX", &err);
    assert(config_dir && !err);
    /* Must precede the first g_get_user_config_dir() call. */
    setenv("XDG_CONFIG_HOME", config_dir, 1);
}

static char* config_path(void) {
    return g_build_filename(config_dir, "pinwin", "config", NULL);
}

static void write_config(const char* text) {
    char* path = config_path();
    char* dir = g_path_get_dirname(path);
    g_assert(g_mkdir_with_parents(dir, S_IRWXU) == 0);
    assert(g_file_set_contents(path, text, -1, NULL));
    g_free(dir);
    g_free(path);
}

static void read_config(char** text, gsize* len) {
    char* path = config_path();
    GError* err = NULL;
    if (!g_file_get_contents(path, text, len, &err)) {
        *text = NULL;
        *len = 0;
        g_error_free(err);
    }
    g_free(path);
}

/* Back to a writable directory with no config file. */
static void reset_config(void) {
    char* path = config_path();
    char* dir = g_path_get_dirname(path);
    chmod(path, S_IRUSR | S_IWUSR);
    chmod(dir, S_IRWXU);
    unlink(path);
    g_free(dir);
    g_free(path);
}

static void check_config(void) {
    PinwinLayout l, saved;
    char* path;

    use_config_dir();

    /* Absent file: ABSENT, no write (the directory is not even created). */
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_ABSENT);
    path = config_path();
    CHECK(!g_file_test(path, G_FILE_TEST_EXISTS));
    g_free(path);

    /* Saved settings beat the GUTTER/COLS baseline: simulate the caller's
     * merge. */
    write_config("[layout]\nside=right\ncols=52\ntop=1\nbottom=2\nleft=3\nright=4\n");
    l.cols = 63;
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_LOADED);
    CHECK(l.side == PINWIN_SIDE_RIGHT && l.cols == 52 && l.top == 1 &&
          l.bottom == 2 && l.left == 3 && l.right == 4);
    {
        PinwinLayout baseline = pinwin_layout_default(40, 8);
        /* The saved layout, not the GUTTER/COLS-derived baseline, wins. */
        CHECK(l.side != baseline.side || l.right != baseline.right ||
              l.cols != baseline.cols);
    }
    reset_config();

    /* An absent cols key leaves the caller's launch COLS in place. */
    write_config("[layout]\nside=left\ntop=1\nbottom=1\nleft=1\nright=1\n");
    l.cols = 60;
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_LOADED);
    CHECK(l.cols == 60);
    reset_config();

    /* A malformed cols key rejects the whole layout, like a gutter. */
    write_config("[layout]\nside=left\ncols=0\ntop=0\nbottom=0\nleft=0\nright=0\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_INVALID);
    reset_config();
    write_config("[layout]\nside=left\ncols=abc\ntop=0\nbottom=0\nleft=0\nright=0\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_INVALID);
    reset_config();

    /* Round-trip through save. */
    l.side = PINWIN_SIDE_RIGHT;
    l.cols = 52;
    l.top = 24; l.bottom = 10; l.left = 123456; l.right = 0;
    CHECK(pinwin_config_save(&l) == 0);
    CHECK(pinwin_config_load(&saved) == PINWIN_CONFIG_LOADED);
    CHECK(saved.side == l.side && saved.cols == l.cols &&
          saved.top == l.top && saved.bottom == l.bottom &&
          saved.left == l.left && saved.right == l.right);
    reset_config();

    /* Corrupt: unknown side -> INVALID, file left untouched. */
    write_config("[layout]\nside=middle\ntop=0\nbottom=0\nleft=0\nright=0\n");
    {
        char* before = NULL;
        gsize len = 0;
        read_config(&before, &len);
        CHECK(before != NULL);
        CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_INVALID);
        {
            char* after = NULL;
            gsize len2 = 0;
            read_config(&after, &len2);
            CHECK(len == len2 && memcmp(before, after, len) == 0);
            g_free(after);
        }
        g_free(before);
    }
    reset_config();

    /* Incomplete: a missing field rejects the whole layout. */
    write_config("[layout]\nside=left\ntop=0\nbottom=0\nleft=0\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_INVALID);
    reset_config();

    /* Malformed: fractional gutters are rejected; negative ones now load. */
    write_config("[layout]\nside=left\ntop=1.5\nbottom=0\nleft=0\nright=0\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_INVALID);
    reset_config();
    write_config("[layout]\nside=left\ntop=-2\nbottom=0\nleft=0\nright=0\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_LOADED && l.top == -2);
    reset_config();

    /* Unknown keys are ignored for forward compatibility. */
    write_config("[layout]\nside=left\ntop=0\nbottom=0\nleft=0\nright=7\nfuture=1\n");
    CHECK(pinwin_config_load(&l) == PINWIN_CONFIG_LOADED && l.right == 7);
    reset_config();

    /* Failed replacement preserves the old file: make the config directory
     * and the existing config file unwritable, then try to save over it. */
    write_config("[layout]\nside=left\ntop=0\nbottom=0\nleft=0\nright=9\n");
    path = config_path();
    {
        char* dir = g_path_get_dirname(path);
        l.side = PINWIN_SIDE_LEFT; l.top = 5; l.bottom = 5; l.left = 5; l.right = 5;
        assert(chmod(dir, S_IRUSR | S_IXUSR) == 0);
        assert(chmod(path, S_IRUSR) == 0);
        CHECK(pinwin_config_save(&l) != 0);
        saved.cols = 40;
        CHECK(pinwin_config_load(&saved) == PINWIN_CONFIG_LOADED && saved.right == 9);
        g_free(dir);
    }
    g_free(path);
    reset_config();

    g_free(config_dir);
    config_dir = NULL;
}

static void check_parse(void) {
    int32_t v = 123;

    CHECK(pinwin_parse_gutter("12", &v) && v == 12);
    CHECK(pinwin_parse_gutter("0", &v) && v == 0);
    CHECK(pinwin_parse_gutter("2147483647", &v) && v == INT32_MAX);
    /* Negative gutters are allowed (they push the edge past the screen). */
    CHECK(pinwin_parse_gutter("-1", &v) && v == -1);
    CHECK(pinwin_parse_gutter("-2147483648", &v) && v == INT32_MIN);
    CHECK(pinwin_parse_gutter("-0", &v) && v == 0);
    /* Rejects: empty, signed beyond one '-', fractional, whitespace, garbage,
     * overflow. */
    CHECK(!pinwin_parse_gutter("", &v));
    CHECK(!pinwin_parse_gutter("+1", &v));
    CHECK(!pinwin_parse_gutter("--1", &v));
    CHECK(!pinwin_parse_gutter("12.5", &v));
    CHECK(!pinwin_parse_gutter(" 12", &v));
    CHECK(!pinwin_parse_gutter("12 ", &v));
    CHECK(!pinwin_parse_gutter("1e3", &v));
    CHECK(!pinwin_parse_gutter("abc", &v));
    CHECK(!pinwin_parse_gutter("2147483648", &v));
    CHECK(!pinwin_parse_gutter("-2147483649", &v));
    CHECK(!pinwin_parse_gutter("99999999999999999999", &v));
}

static void check_parse_cols(void) {
    int32_t v = 123;

    CHECK(pinwin_parse_cols("1", &v) && v == 1);
    CHECK(pinwin_parse_cols("40", &v) && v == 40);
    CHECK(pinwin_parse_cols("0040", &v) && v == 40);
    CHECK(pinwin_parse_cols("65535", &v) && v == 65535);
    /* Rejects: empty, zero, sign, fractional, whitespace, garbage, beyond the
     * PTY winsize column field. */
    CHECK(!pinwin_parse_cols("", &v));
    CHECK(!pinwin_parse_cols("0", &v));
    CHECK(!pinwin_parse_cols("-1", &v));
    CHECK(!pinwin_parse_cols("+1", &v));
    CHECK(!pinwin_parse_cols("12.5", &v));
    CHECK(!pinwin_parse_cols(" 12", &v));
    CHECK(!pinwin_parse_cols("12 ", &v));
    CHECK(!pinwin_parse_cols("1e3", &v));
    CHECK(!pinwin_parse_cols("abc", &v));
    CHECK(!pinwin_parse_cols("65536", &v));
    CHECK(!pinwin_parse_cols("99999999999999999999", &v));
}

static void check_side_formulas(void) {
    PinwinLayout l;
    int32_t margin, reservation;

    /* Spec scenario: panel 320, Left 8, Right 12, docked left. */
    l.side = PINWIN_SIDE_LEFT; l.top = 0; l.bottom = 0; l.left = 8; l.right = 12;
    CHECK(pinwin_side_geometry(&l, 320, &margin, &reservation));
    CHECK(margin == 8);
    CHECK(reservation == 340);

    /* Same values docked right: margin becomes Right, same sum. */
    l.side = PINWIN_SIDE_RIGHT;
    CHECK(pinwin_side_geometry(&l, 320, &margin, &reservation));
    CHECK(margin == 12);
    CHECK(reservation == 340);

    /* Unknown side and overflow are rejected. */
    l.side = 2;
    CHECK(!pinwin_side_geometry(&l, 320, &margin, &reservation));
    l.side = PINWIN_SIDE_LEFT;
    l.left = INT32_MAX; l.right = INT32_MAX;
    CHECK(!pinwin_side_geometry(&l, 320, &margin, &reservation));
    l.left = 0; l.right = 0;
    CHECK(!pinwin_side_geometry(&l, -1, &margin, &reservation));
}

static void check_validate(void) {
    PinwinLayout l;
    int32_t r;

    /* A sane layout passes. */
    l.side = PINWIN_SIDE_LEFT; l.top = 24; l.bottom = 10; l.left = 8; l.right = 12;
    r = pinwin_layout_validate(&l, 40, 10, 20, 1920, 1080);
    CHECK(r == PINWIN_GEOM_OK);

    /* Negative gutters: the panel bleeds past the edges; the reservation
     * shrinks but must stay non-negative and below the output width. */
    l.side = PINWIN_SIDE_LEFT; l.top = -40; l.bottom = 0; l.left = -40; l.right = 12;
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 1920, 1080) == PINWIN_GEOM_OK);
    /* -8,388,608 + 3200 + 12 < 0: nothing left to reserve. */
    l.left = -8388608;
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 1920, 1080) ==
          PINWIN_GEOM_ERR_NO_RESERVE);
    /* Spec scenario: panel 320, Left -40, Right 12 -> reservation 292. */
    l.side = PINWIN_SIDE_LEFT; l.top = 0; l.bottom = 0; l.left = -40; l.right = 12;
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 1920, 1080) == PINWIN_GEOM_OK);

    /* Tiles-overlap case: Right negative beyond the panel is rejected. */
    l.left = 0; l.right = -400; /* panel 320: 0 + 320 - 400 < 0 */
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 1920, 1080) ==
          PINWIN_GEOM_ERR_NO_RESERVE);
    l.right = -80; /* 0 + 320 - 80 = 240: tiles overlap the panel's last 80px */
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 1920, 1080) == PINWIN_GEOM_OK);

    l.side = 5;
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 1920, 1080) == PINWIN_GEOM_ERR_SIDE);
    l.side = PINWIN_SIDE_RIGHT;

    /* Panel width multiplication overflows. */
    CHECK(pinwin_layout_validate(&l, INT32_MAX, INT32_MAX, 20, 1920, 1080) ==
          PINWIN_GEOM_ERR_OVERFLOW);

    /* One-row boundary: exactly one row of space is OK, one pixel less is not.
     * Negative top/bottom insets give more room. */
    l.side = PINWIN_SIDE_LEFT; l.top = 30; l.bottom = 30;
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 1920, 1000) == PINWIN_GEOM_OK);
    CHECK(pinwin_layout_validate(&l, 40, 10, 941, 1920, 1000) == PINWIN_GEOM_ERR_NO_ROW);
    l.top = -100; l.bottom = 0; /* output 1000 -> visible 1100: still one-row OK */
    CHECK(pinwin_layout_validate(&l, 40, 10, 1041, 1920, 1000) == PINWIN_GEOM_OK);
    CHECK(pinwin_layout_validate(&l, 40, 10, 1241, 1920, 1000) == PINWIN_GEOM_ERR_NO_ROW);
    /* Extreme negatives cannot overflow: top + bottom is summed in long long. */
    l.top = INT32_MIN; l.bottom = INT32_MIN;
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 1920, 1000) == PINWIN_GEOM_OK);

    /* Remaining-width boundary: reservation == output_w - 1 is OK, == output_w
     * is not (nothing left for other windows). Panel 320 = 32 cols * 10 px,
     * reservation 8 + 320 + 12 = 340. */
    l.top = 0; l.bottom = 0; l.left = 8; l.right = 12;
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 341, 1080) == PINWIN_GEOM_OK);
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 340, 1080) == PINWIN_GEOM_ERR_NO_WIDTH);

    /* left + panel + right overflowing int32 is the overflow error. */
    l.left = INT32_MAX;
    CHECK(pinwin_layout_validate(&l, 32, 10, 20, 1920, 1080) == PINWIN_GEOM_ERR_OVERFLOW);

    /* Staged column counts (options Apply validates layout.cols): a wider
     * panel can exhaust the remaining output width. */
    l.side = PINWIN_SIDE_LEFT; l.top = 0; l.bottom = 0; l.left = 8; l.right = 12;
    l.cols = 60; /* 600 px panel, reservation 620 */
    CHECK(pinwin_layout_validate(&l, l.cols, 10, 20, 1920, 1080) == PINWIN_GEOM_OK);
    l.cols = 191; /* 1910 + 20 = 1930 >= 1920: nothing left for other windows */
    CHECK(pinwin_layout_validate(&l, l.cols, 10, 20, 1920, 1080) ==
          PINWIN_GEOM_ERR_NO_WIDTH);

    /* Nonsense metrics. */
    CHECK(pinwin_layout_validate(&l, 0, 10, 20, 1920, 1080) == PINWIN_GEOM_ERR_METRICS);
    CHECK(pinwin_layout_validate(&l, 40, 10, 0, 1920, 1080) == PINWIN_GEOM_ERR_METRICS);
    CHECK(pinwin_layout_validate(&l, 40, 10, 20, 0, 1080) == PINWIN_GEOM_ERR_METRICS);
}

static void check_defaults(void) {
    PinwinLayout l = pinwin_layout_default(40, 8);

    CHECK(l.side == PINWIN_SIDE_LEFT);
    CHECK(l.cols == 40);
    CHECK(l.top == 0 && l.bottom == 0 && l.left == 0 && l.right == 8);
    l = pinwin_layout_default(52, 0);
    CHECK(l.cols == 52 && l.right == 0);
    /* GUTTER is validated upstream; clamp anyway at the launch baseline. */
    l = pinwin_layout_default(1, -3);
    CHECK(l.cols == 1 && l.right == 0);
}

static void check_icon_bytes(void) {
    GByteArray* out = g_byte_array_new();
    const unsigned char rgba[2][4] = {{1, 2, 3, 4}, {250, 0, 7, 255}};

    tray_argb_from_rgba(rgba[0], 2, out);
    CHECK(out->len == 8);
    /* Network-order ARGB: alpha first, then R, G, B. */
    CHECK(out->data[0] == 4 && out->data[1] == 1 && out->data[2] == 2 &&
          out->data[3] == 3);
    CHECK(out->data[4] == 255 && out->data[5] == 250 && out->data[6] == 0 &&
          out->data[7] == 7);
    g_byte_array_unref(out);
}

int main(void) {
    check_parse();
    check_parse_cols();
    check_side_formulas();
    check_validate();
    check_defaults();
    check_config();
    check_icon_bytes();

    if (failures) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    printf("check_options: all passed\n");
    return 0;
}
