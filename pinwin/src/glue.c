/*
 * glue.c - everything in pinwin that talks to GTK4, gtk4-layer-shell, Pango,
 * cairo, GdkPixbuf and the PTY. The Zig side (main.zig) only ever calls the
 * glue_* functions declared in pinwin.h and is called back through the
 * pinwin_* functions.
 *
 * See pinwin.h for why this file exists at all (translate-c cannot consume
 * the GTK4 headers).
 */

#include "pinwin.h"
#include "options.h"
#include "tray.h"
#include "nerd_font_tables.h"

#include <gtk/gtk.h>
#include <gtk4-layer-shell.h>
#include <pango/pangocairo.h>

#include <errno.h>
#include <fcntl.h>
#include <glib-unix.h>
#include <pty.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

/* ---- state -------------------------------------------------------------- */

static int32_t g_cols = 40;
static int32_t g_gutter = 0;
static int32_t g_keyboard_mode = PINWIN_KEYBOARD_ON_DEMAND;

static char* const* g_argv;

static GtkApplication* g_app;
static GtkWindow* g_win;
static GtkWidget* g_area;

static PangoFontDescription* g_font;
static PangoFontDescription* g_font_bold;
static PangoFontDescription* g_font_italic;
static PangoFontDescription* g_font_bold_italic;
static int32_t g_ascent;
/* Grid metrics in Ghostty's terms (see src/font/Metrics.zig in the pinned
 * commit); the Nerd Font constraints are expressed against these. */
static int32_t g_cell_baseline; /* px from the cell's bottom to the baseline */
static double g_nerd_face_w, g_nerd_face_h, g_nerd_face_y;
static double g_nerd_icon_h, g_nerd_icon_h_single;
/* Terminal default colours from the Ghostty config/theme. */
static uint8_t g_theme_bg[3] = {0, 0, 0};
static uint8_t g_theme_fg[3] = {255, 255, 255};

static int32_t g_cell_w;
static int32_t g_cell_h;
static int32_t g_rows;
static int32_t g_grid_cols; /* cols of the last grid pushed to pinwin_size */

static int g_pty_fd = -1;
static guint g_pty_source;
static double g_last_x, g_last_y;
static int g_key_layout;
static int32_t g_pty_cols = 40;
static int32_t g_pty_rows = 24;
static int32_t g_pty_xpixel;
static int32_t g_pty_ypixel;
static int g_spawned;

/* Layout settings (add-pinwin-tray-options). The applied layout starts as
 * the launch baseline and is loaded from the config / replaced by Apply. */
static PinwinLayout g_layout;
static GtkWindow* g_reserve;
static GdkMonitor* g_monitor; /* the visible panel's original monitor */
static int g_layout_latch; /* first-draw monitor resolution pending */

/* ---- runtime dependency checks ------------------------------------------ */

static int terminfo_exists(const char* name) {
    static const char* dirs[] = {"/usr/share/terminfo", "/etc/terminfo",
                                 "/usr/lib/terminfo", "/lib/terminfo"};
    const char* single = getenv("TERMINFO");
    const char* list = getenv("TERMINFO_DIRS");
    char path[512];
    size_t i;

    if (single && *single) {
        snprintf(path, sizeof(path), "%s/%c/%s", single, name[0], name);
        if (access(path, R_OK) == 0) return 1;
    }
    if (list && *list) {
        const char* p = list;
        while (*p) {
            const char* colon = strchr(p, ':');
            size_t len = colon ? (size_t)(colon - p) : strlen(p);
            if (len == 0) {
                snprintf(path, sizeof(path), "/usr/share/terminfo/%c/%s", name[0], name);
            } else if (len < sizeof(path)) {
                snprintf(path, sizeof(path), "%.*s/%c/%s", (int)len, p, name[0], name);
            } else {
                path[0] = '\0';
            }
            if (path[0] && access(path, R_OK) == 0) return 1;
            if (!colon) break;
            p = colon + 1;
        }
    }
    for (i = 0; i < sizeof(dirs) / sizeof(dirs[0]); i++) {
        snprintf(path, sizeof(path), "%s/%c/%s", dirs[i], name[0], name);
        if (access(path, R_OK) == 0) return 1;
    }
    return 0;
}

/* ---- font (design D5) --------------------------------------------------- */

/* The panel follows the Ghostty config (a plain "key = value" file) so that it
 * uses the font the user actually configured for their terminal, with
 * PINWIN_FONT / PINWIN_FONT_SIZE as overrides and "monospace 11" as the
 * fallback. Only the first font-family is used, matching Ghostty's rule that
 * the first family with a glyph wins. */
/* The terminal's default background and foreground: the panel paints the whole
 * widget with the background (the VT's own default is black, which shows up as
 * a black strip under the last row) and uses the foreground for cells that
 * don't set one. Read from the Ghostty config, following a `theme` file the
 * same way Ghostty does. */
static void theme_colour_parse(const char* line, uint8_t* bg, uint8_t* fg) {
    char key[32];
    char value[32];
    unsigned r, g, b;

    if (sscanf(line, " %31[a-z-] = %31s", key, value) != 2) return;
    if (value[0] != '#') return;
    if (sscanf(value + 1, "%2x%2x%2x", &r, &g, &b) != 3) return;
    if (strcmp(key, "background") == 0) {
        bg[0] = (uint8_t)r; bg[1] = (uint8_t)g; bg[2] = (uint8_t)b;
    } else if (strcmp(key, "foreground") == 0) {
        fg[0] = (uint8_t)r; fg[1] = (uint8_t)g; fg[2] = (uint8_t)b;
    }
}

static void theme_colours(char* theme_name, size_t theme_name_len,
                          uint8_t* bg, uint8_t* fg) {
    char* path;
    char* data = NULL;
    char* lines;
    char* line;
    gboolean found_theme = FALSE;

    theme_name[0] = '\0';
    path = g_build_filename(g_get_user_config_dir(), "ghostty", "config", NULL);
    if (g_file_get_contents(path, &data, NULL, NULL)) {
        lines = data;
        while ((line = strsep(&lines, "\n")) != NULL) {
            char key[32];
            char value[256];
            if (sscanf(line, " %31[a-z-] = %255[^#]", key, value) != 2) continue;
            g_strstrip(value);
            if (value[0] == '"') {
                char* end = strrchr(value + 1, '"');
                if (end) *end = '\0';
                memmove(value, value + 1, strlen(value));
            }
            if (strcmp(key, "theme") == 0) {
                snprintf(theme_name, theme_name_len, "%s", value);
                found_theme = TRUE;
            } else if (strcmp(key, "background") == 0 || strcmp(key, "foreground") == 0) {
                theme_colour_parse(line, bg, fg);
            }
        }
        g_free(data);
    }
    g_free(path);

    if (!found_theme) return;
    /* Theme files live next to the config or in Ghostty's install directory. */
    path = g_build_filename(g_get_user_config_dir(), "ghostty", "themes", theme_name, NULL);
    if (!g_file_get_contents(path, &data, NULL, NULL)) {
        g_free(path);
        path = g_build_filename("/usr/share/ghostty/themes", theme_name, NULL);
        if (!g_file_get_contents(path, &data, NULL, NULL)) {
            g_free(path);
            return;
        }
    }
    g_free(path);
    lines = data;
    while ((line = strsep(&lines, "\n")) != NULL) theme_colour_parse(line, bg, fg);
    g_free(data);
}

static void font_config_load(char** family, double* size) {
    char* path;
    char* data = NULL;
    const char* env;

    *family = NULL;
    *size = 11.0;

    path = g_build_filename(g_get_user_config_dir(), "ghostty", "config", NULL);
    if (g_file_get_contents(path, &data, NULL, NULL)) {
        char** lines = g_strsplit(data, "\n", -1);
        size_t i;
        for (i = 0; lines[i]; i++) {
            char* line = g_strdup(lines[i]);
            char* eq;
            char* key;
            char* value;

            /* Ghostty allows whole-line comments; nothing else uses '#'. */
            char* hash = strchr(line, '#');
            if (hash) *hash = '\0';
            eq = strchr(line, '=');
            if (!eq) {
                g_free(line);
                continue;
            }
            *eq = '\0';
            key = g_strstrip(line);
            value = g_strstrip(eq + 1);
            if (value[0] == '"') {
                char* end = strrchr(value + 1, '"');
                if (end) *end = '\0';
                value++;
            }
            if (strcmp(key, "font-family") == 0 && *value && !*family) {
                *family = g_strdup(value);
            } else if (strcmp(key, "font-size") == 0) {
                double parsed = g_ascii_strtod(value, NULL);
                if (parsed > 0) *size = parsed;
            }
            g_free(line);
        }
        g_strfreev(lines);
        g_free(data);
    }
    g_free(path);

    env = getenv("PINWIN_FONT");
    if (env && *env) {
        g_free(*family);
        *family = g_strdup(env);
    }
    env = getenv("PINWIN_FONT_SIZE");
    if (env && *env) {
        double parsed = g_ascii_strtod(env, NULL);
        if (parsed > 0) *size = parsed;
    }
}

/* ---- cell metrics (design D5) ------------------------------------------- */

int32_t glue_cell_width(void) { return g_cell_w; }
int32_t glue_cell_height(void) { return g_cell_h; }

static void cell_metrics_update(GtkWidget* widget) {
    PangoFontMetrics* metrics;
    int32_t digit;

    if (!g_font) {
        char* family;
        double size;
        font_config_load(&family, &size);
        g_font = pango_font_description_new();
        pango_font_description_set_family(g_font, family ? family : "monospace");
        pango_font_description_set_size(g_font, (int32_t)(size * PANGO_SCALE));
        if (getenv("PINWIN_DEBUG"))
            fprintf(stderr, "pinwin: font %s %g\n", family ? family : "monospace", size);
        g_free(family);
        g_font_bold = pango_font_description_copy(g_font);
        pango_font_description_set_weight(g_font_bold, PANGO_WEIGHT_BOLD);
        g_font_italic = pango_font_description_copy(g_font);
        pango_font_description_set_style(g_font_italic, PANGO_STYLE_ITALIC);
        g_font_bold_italic = pango_font_description_copy(g_font_bold);
        pango_font_description_set_style(g_font_bold_italic, PANGO_STYLE_ITALIC);
    }

    metrics = pango_context_get_metrics(gtk_widget_get_pango_context(widget),
                                        g_font, NULL);
    int32_t ascent = pango_font_metrics_get_ascent(metrics);
    int32_t descent = pango_font_metrics_get_descent(metrics);
    digit = pango_font_metrics_get_approximate_digit_width(metrics);
    pango_font_metrics_unref(metrics);

    g_ascent = (int32_t)(ascent / PANGO_SCALE);
    g_cell_w = (int32_t)((digit + PANGO_SCALE / 2) / PANGO_SCALE);
    g_cell_h = (int32_t)((ascent + descent + PANGO_SCALE / 2) / PANGO_SCALE);
    if (g_cell_w < 1) g_cell_w = 1;
    if (g_cell_h < 1) g_cell_h = 1;

    /* Ghostty's grid metrics: the face box is the unrounded line box, the
     * baseline is rounded to the pixel grid, and the "icon height" is the
     * font_patcher heuristic (2*cap + line height) / 3 for one-cell
     * constraints. */
    {
        const double face_w = (double)digit / PANGO_SCALE;
        const double face_h = (double)(ascent + descent) / PANGO_SCALE;
        const double descent_px = (double)descent / PANGO_SCALE;
        const double face_baseline = descent_px; /* half line gap is 0 in Pango */
        double cell_baseline = face_baseline - (g_cell_h - face_h) / 2.0;
        double cap_h = 0.75 * (double)ascent / PANGO_SCALE;

        cell_baseline = (double)(int32_t)(cell_baseline + 0.5 * (cell_baseline < 0 ? -1 : 1));
        g_cell_baseline = (int32_t)cell_baseline;
        g_nerd_face_w = face_w;
        g_nerd_face_h = face_h;
        g_nerd_face_y = cell_baseline - face_baseline;
        g_nerd_icon_h = face_h;
        g_nerd_icon_h_single = (2.0 * cap_h + face_h) / 3.0;
    }
}

/* ---- drawing ------------------------------------------------------------ */

static void set_rgb(cairo_t* cr, uint8_t r, uint8_t g, uint8_t b) {
    cairo_set_source_rgb(cr, r / 255.0, g / 255.0, b / 255.0);
}

/* Draws the grapheme in `cell` with an already-set source colour. */

/* ---- Nerd Font glyph constraints (design D5) ---------------------------- */

static uint32_t first_codepoint(const char* text, int32_t len);

/* Ghostty normalises every Nerd Font icon to a canonical box before drawing:
 * a generated table maps the codepoint to a constraint (scale rule, padding,
 * alignment, relative geometry) and the glyph is scaled and moved to fit. The
 * table header is generated from the pinned commit's nerd_font_tables.zig and
 * this is a port of Glyph.RenderOptions.Constraint (src/font/Glyph.zig). */

typedef struct {
    double x, y, width, height;
} NerdGlyph;

typedef struct {
    double face_w, face_h, face_y, icon_h, icon_h_single, cell_w, cell_h;
} NerdMetrics;

static const NerdConstraint* nerd_constraint(uint32_t cp) {
    uint16_t page;

    if (cp > 0x10FFFF) return NULL;
    page = nerd_stage1[cp >> 8];
    if (page == 0) return NULL; /* shared empty page */
    return &nerd_stage3[nerd_stage2[page + (cp & 0xFF)]];
}

static int nerd_does_anything(const NerdConstraint* c) {
    return c->size != NERD_SIZE_NONE || c->align_h != NERD_ALIGN_NONE ||
           c->align_v != NERD_ALIGN_NONE;
}

static double nerd_max(double a, double b) { return a > b ? a : b; }
static double nerd_min(double a, double b) { return a < b ? a : b; }

static void nerd_scale_factors(const NerdConstraint* c, const NerdMetrics* m,
                               const NerdGlyph* group, unsigned min_w,
                               double* width_factor, double* height_factor) {
    const int multi = min_w > 1;
    double pad_w, pad_h, target_w, target_h, w, h;

    if (c->size == NERD_SIZE_NONE) {
        *width_factor = 1.0;
        *height_factor = 1.0;
        return;
    }

    pad_w = (double)min_w - (c->pad_left + c->pad_right);
    pad_h = 1.0 - (c->pad_bottom + c->pad_top);
    target_w = pad_w * m->face_w;
    target_h = pad_h * (c->height == NERD_HEIGHT_ICON
                            ? (multi ? m->icon_h : m->icon_h_single)
                            : m->face_h);
    w = target_w / group->width;
    h = target_h / group->height;

    switch (c->size) {
    case NERD_SIZE_FIT:
        h = nerd_min(1.0, nerd_min(w, h));
        w = h;
        break;
    case NERD_SIZE_COVER:
        h = nerd_min(w, h);
        w = h;
        break;
    case NERD_SIZE_FIT_COVER1:
        h = nerd_min(w, h);
        if (multi && h > 1.0) {
            double single_w, single_h;
            nerd_scale_factors(c, m, group, 1, &single_w, &single_h);
            h = nerd_max(1.0, single_h);
        }
        w = h;
        break;
    default: /* stretch */
        break;
    }

    if (c->max_xy_ratio >= 0) {
        if (group->width * w > group->height * h * c->max_xy_ratio)
            w = group->height * h * c->max_xy_ratio / group->width;
    }

    *width_factor = w;
    *height_factor = h;
}

static NerdGlyph nerd_constrain(const NerdConstraint* c, const NerdMetrics* metrics,
                                NerdGlyph glyph, unsigned constraint_width) {
    NerdGlyph group, out;
    NerdMetrics m = *metrics;
    NerdConstraint cc = *c;
    const NerdConstraint* k = &cc;
    unsigned min_w;
    double width_factor, height_factor, center_x, center_y;

    if (c->size == NERD_SIZE_STRETCH) {
        /* Stretched glyphs are scaled and aligned to the grid, not the face;
         * negative padding would only cause overlap there. */
        m.face_w = m.cell_w;
        m.face_h = m.cell_h;
        m.face_y = 0.0;
        if (k->pad_bottom < 0) cc.pad_bottom = 0;
        if (k->pad_top < 0) cc.pad_top = 0;
        if (k->pad_left < 0) cc.pad_left = 0;
        if (k->pad_right < 0) cc.pad_right = 0;
    }

    min_w = (k->size == NERD_SIZE_STRETCH && m.face_w > 0.9 * m.face_h)
                ? 1
                : (unsigned)nerd_min((double)k->max_constraint_width,
                                     (double)constraint_width);

    group.width = glyph.width / k->relative_width;
    group.height = glyph.height / k->relative_height;
    group.x = glyph.x - group.width * k->relative_x;
    group.y = glyph.y - group.height * k->relative_y;

    nerd_scale_factors(k, &m, &group, min_w, &width_factor, &height_factor);
    center_x = group.x + group.width / 2;
    center_y = group.y + group.height / 2;
    group.width *= width_factor;
    group.height *= height_factor;
    group.x = center_x - group.width / 2;
    group.y = center_y - group.height / 2;

    /* Vertical alignment, relative to the cell's bottom. */
    if (!(k->size == NERD_SIZE_NONE && k->align_v == NERD_ALIGN_NONE)) {
        const double start_y = m.face_y + k->pad_bottom * m.face_h;
        const double end_y = m.face_y + (m.face_h - group.height - k->pad_top * m.face_h);
        const double center = (start_y + end_y) / 2;

        switch (k->align_v) {
        case NERD_ALIGN_START:
            group.y = start_y;
            break;
        case NERD_ALIGN_END:
            group.y = end_y;
            break;
        case NERD_ALIGN_CENTER:
        case NERD_ALIGN_CENTER1:
            group.y = center;
            break;
        default:
            group.y = end_y < start_y ? center : nerd_max(start_y, nerd_min(group.y, end_y));
            break;
        }
    }

    /* Horizontal alignment, relative to the face's left edge. */
    if (!(k->size == NERD_SIZE_NONE && k->align_h == NERD_ALIGN_NONE)) {
        const double span = m.face_w + (double)((min_w - 1) * (unsigned)m.cell_w);
        const double start_x = k->pad_left * m.face_w;
        const double end_x = span - group.width - k->pad_right * m.face_w;

        switch (k->align_h) {
        case NERD_ALIGN_START:
            group.x = start_x;
            break;
        case NERD_ALIGN_END:
            group.x = nerd_max(start_x, end_x);
            break;
        case NERD_ALIGN_CENTER:
            group.x = nerd_max(start_x, (start_x + end_x) / 2);
            break;
        case NERD_ALIGN_CENTER1: {
            const double end1_x = m.face_w - group.width - k->pad_right * m.face_w;
            group.x = nerd_max(start_x, (start_x + end1_x) / 2);
            break;
        }
        default:
            group.x = nerd_max(start_x, nerd_min(group.x, end_x));
            break;
        }
    }

    out.width = width_factor * glyph.width;
    out.height = height_factor * glyph.height;
    out.x = group.x + group.width * k->relative_x;
    out.y = group.y + group.height * k->relative_y;
    return out;
}

static void draw_text(cairo_t* cr, PangoLayout* layout, const PinwinCell* cell) {
    const PangoFontDescription* desc = g_font;
    int32_t baseline;

    if ((cell->flags & PINWIN_BOLD) && (cell->flags & PINWIN_ITALIC))
        desc = g_font_bold_italic;
    else if (cell->flags & PINWIN_BOLD)
        desc = g_font_bold;
    else if (cell->flags & PINWIN_ITALIC)
        desc = g_font_italic;

    pango_layout_set_font_description(layout, desc);
    pango_layout_set_text(layout, cell->text, cell->len);

    /* pango_cairo_show_layout puts the layout's top at the current point, and
     * that top is the layout's own ascent -- which changes when Pango fell
     * back to another font for a glyph (box drawing, nerd icons, emoji).
     * Pin the baseline to the cell's instead, or those glyphs drift off the
     * row they belong to. */
    if (getenv("PINWIN_DEBUG") && cell->y <= 5) {
        char hex[3 * 32 + 1];
        size_t i;
        PangoLayoutIter* it = pango_layout_get_iter(layout);
        for (i = 0; i < (size_t)cell->len && i < 32; i++)
            snprintf(hex + i * 3, 4, "%02x ", (unsigned char)cell->text[i]);
        hex[cell->len < 32 ? (size_t)cell->len * 3 : 96] = '\0';
        fprintf(stderr, "dbg y=%d x=%d len=%d %s", cell->y, cell->x, cell->len, hex);
        do {
            PangoLayoutRun* run = pango_layout_iter_get_run_readonly(it);
            if (run) {
                char* d = pango_font_description_to_string(pango_font_describe(run->item->analysis.font));
                fprintf(stderr, "[%s]", d);
                g_free(d);
            }
        } while (pango_layout_iter_next_run(it));
        pango_layout_iter_free(it);
        fprintf(stderr, "\n");
    }

    baseline = (pango_layout_get_baseline(layout) + PANGO_SCALE / 2) / PANGO_SCALE;

    if (cell->len > 0) {
        const NerdConstraint* c = nerd_constraint(first_codepoint(cell->text, cell->len));
        if (c && nerd_does_anything(c)) {
            PangoRectangle ink;
            NerdMetrics m;
            NerdGlyph glyph, box;

            pango_layout_get_pixel_extents(layout, &ink, NULL);
            if (ink.width > 0 && ink.height > 0) {
                m.face_w = g_nerd_face_w;
                m.face_h = g_nerd_face_h;
                m.face_y = g_nerd_face_y;
                m.icon_h = g_nerd_icon_h;
                m.icon_h_single = g_nerd_icon_h_single;
                m.cell_w = g_cell_w;
                m.cell_h = g_cell_h;

                /* The glyph's box relative to the cell's bottom-left corner. */
                glyph.width = ink.width;
                glyph.height = ink.height;
                glyph.x = ink.x;
                glyph.y = g_cell_baseline - ((ink.y + ink.height) - baseline);

                box = nerd_constrain(c, &m, glyph,
                                     cell->cw >= 1 ? (unsigned)cell->cw : 1u);


                cairo_save(cr);
                cairo_translate(cr, cell->x * g_cell_w + box.x,
                                (cell->y + 1) * g_cell_h - box.y - box.height);
                cairo_scale(cr, box.width / ink.width, box.height / ink.height);
                cairo_translate(cr, -ink.x, -ink.y);
                cairo_move_to(cr, 0, 0);
                pango_cairo_show_layout(cr, layout);
                cairo_restore(cr);
                return;
            }
        }
    }

    cairo_move_to(cr, cell->x * g_cell_w, cell->y * g_cell_h + g_ascent - baseline);
    pango_cairo_show_layout(cr, layout);
}

/* The first codepoint of a cell's UTF-8 text, for the sprite ranges (which are
 * always a single codepoint). 0 when there is none. */
static uint32_t first_codepoint(const char* text, int32_t len) {
    const unsigned char* s = (const unsigned char*)text;
    if (len <= 0) return 0;
    if (s[0] < 0x80) return s[0];
    if ((s[0] & 0xE0) == 0xC0 && len >= 2)
        return ((uint32_t)(s[0] & 0x1F) << 6) | (s[1] & 0x3F);
    if ((s[0] & 0xF0) == 0xE0 && len >= 3)
        return ((uint32_t)(s[0] & 0x0F) << 12) | ((uint32_t)(s[1] & 0x3F) << 6) | (s[2] & 0x3F);
    if ((s[0] & 0xF8) == 0xF0 && len >= 4)
        return ((uint32_t)(s[0] & 0x07) << 18) | ((uint32_t)(s[1] & 0x3F) << 12) |
               ((uint32_t)(s[2] & 0x3F) << 6) | (s[3] & 0x3F);
    return 0;
}

/* ---- sprites (design D5) ------------------------------------------------ */

static void fill_rect(cairo_t* cr, double x, double y, double w, double h) {
    cairo_rectangle(cr, x, y, w, h);
    cairo_fill(cr);
}

/* Block elements, braille patterns and the powerline separators are drawn
 * here instead of with the font's glyphs, because the font's glyphs for them
 * do not line up between cells and leave seams across a row of them. Ghostty
 * draws the same ranges from its own sprites
 * (src/font/sprite/draw/{block,braille,powerline}.zig in the pinned commit),
 * and the geometry below is a port of those sprites. Returns 1 when cp was
 * drawn. */
static int draw_sprite(cairo_t* cr, const PinwinCell* cell, uint32_t cp) {
    const double cw = g_cell_w;
    const double ch = g_cell_h;
    const double x = cell->x * cw;
    const double y = cell->y * ch;
    const double w = cell->wide == PINWIN_WIDE_WIDE ? 2 * cw : cw;

    if (cp >= 0x25E2 && cp <= 0x25E5) {
        /* Ghostty draws the four corner triangles as full-cell sprites. */
        switch (cp) {
        case 0x25E2: cairo_move_to(cr, x, y + ch); cairo_line_to(cr, x + w, y + ch); cairo_line_to(cr, x + w, y); break;
        case 0x25E3: cairo_move_to(cr, x, y); cairo_line_to(cr, x, y + ch); cairo_line_to(cr, x + w, y + ch); break;
        case 0x25E4: cairo_move_to(cr, x, y); cairo_line_to(cr, x, y + ch); cairo_line_to(cr, x + w, y); break;
        case 0x25E5: cairo_move_to(cr, x, y); cairo_line_to(cr, x + w, y + ch); cairo_line_to(cr, x + w, y); break;
        }
        cairo_close_path(cr);
        cairo_fill(cr);
        return 1;
    }

    if (cp >= 0x2580 && cp <= 0x259F) {
        switch (cp) {
        case 0x2580: /* upper half */
            fill_rect(cr, x, y, w, ch / 2);
            return 1;
        case 0x2581: case 0x2582: case 0x2583: case 0x2584:
        case 0x2585: case 0x2586: case 0x2587: case 0x2588: { /* lower 1/8..8/8 */
            double h = ch * (double)(cp - 0x2580) / 8.0;
            fill_rect(cr, x, y + ch - h, w, h);
            return 1;
        }
        case 0x2589: case 0x258A: case 0x258B: case 0x258C:
        case 0x258D: case 0x258E: case 0x258F: { /* left 7/8..1/8 */
            fill_rect(cr, x, y, w * (double)(0x2590 - cp) / 8.0, ch);
            return 1;
        }
        case 0x2590: /* right half */
            fill_rect(cr, x + w / 2, y, w / 2, ch);
            return 1;
        case 0x2591: case 0x2592: case 0x2593: /* shades: the font has them */
            return 0;
        case 0x2594: /* upper 1/8: whole pixels, 3 px at our 19 px cell height */
            fill_rect(cr, x, y, w, (g_cell_h + 7) / 8);
            return 1;
        case 0x2595: /* right 1/8 */
            fill_rect(cr, x + w * 7 / 8, y, w / 8, ch);
            return 1;
        default: { /* quadrants, 0x2596..0x259F: bit0 upper left, bit1 upper
                    * right, bit2 lower left, bit3 lower right */
            static const uint8_t quadrants[10] = {0x4, 0x8, 0x1, 0xD, 0x9,
                                                  0x7, 0xB, 0x2, 0x6, 0xE};
            uint8_t q = quadrants[cp - 0x2596];
            if (q & 0x1) fill_rect(cr, x, y, w / 2, ch / 2);
            if (q & 0x2) fill_rect(cr, x + w / 2, y, w / 2, ch / 2);
            if (q & 0x4) fill_rect(cr, x, y + ch / 2, w / 2, ch / 2);
            if (q & 0x8) fill_rect(cr, x + w / 2, y + ch / 2, w / 2, ch / 2);
            return 1;
        }
        }
    }

    if (cp >= 0x2800 && cp <= 0x28FF) {
        /* Braille: a 2x4 dot grid. Leftover pixels go to spacing and margins
         * before the dots grow, so the pattern fills the cell like Ghostty's. */
        static const int8_t dot_col[8] = {0, 0, 0, 1, 1, 1, 0, 1};
        static const int8_t dot_row[8] = {0, 1, 2, 0, 1, 2, 3, 3};
        uint32_t p = cp & 0xFF;
        int dot = g_cell_w / 4 < g_cell_h / 8 ? g_cell_w / 4 : g_cell_h / 8;
        int x_spacing = g_cell_w / 4;
        int y_spacing = g_cell_h / 8;
        int x_margin = x_spacing / 2;
        int y_margin = y_spacing / 2;
        int x_left = g_cell_w - 2 * x_margin - x_spacing - 2 * dot;
        int y_left = g_cell_h - 2 * y_margin - 3 * y_spacing - 4 * dot;
        double dx[2], dy[4];
        int i;

        if (x_left >= 2 && y_left >= 4 && dot == 0) {
            dot += 1;
            x_left -= 2;
            y_left -= 4;
        }
        if (x_left >= 2 && x_margin == 0) {
            x_margin = 1;
            x_left -= 2;
        }
        if (y_left >= 2 && y_margin == 0) {
            y_margin = 1;
            y_left -= 2;
        }
        if (x_left >= 1) {
            x_spacing += 1;
            x_left -= 1;
        }
        if (y_left >= 3) {
            y_spacing += 1;
            y_left -= 3;
        }
        if (x_left >= 2) {
            x_margin += 1;
            x_left -= 2;
        }
        if (y_left >= 2) {
            y_margin += 1;
            y_left -= 2;
        }
        if (x_left >= 2 && y_left >= 4) {
            dot += 1;
            x_left -= 2;
            y_left -= 4;
        }

        dx[0] = x_margin;
        dx[1] = x_margin + dot + x_spacing;
        dy[0] = y_margin;
        dy[1] = dy[0] + dot + y_spacing;
        dy[2] = dy[1] + dot + y_spacing;
        dy[3] = dy[2] + dot + y_spacing;
        for (i = 0; i < 8; i++) {
            if (p & (1u << i))
                fill_rect(cr, x + dx[dot_col[i]], y + dy[dot_row[i]], dot, dot);
        }
        return 1;
    }

    if (cp == 0xE0B0 || cp == 0xE0B1 || cp == 0xE0B2 || cp == 0xE0B3) {
        /* Powerline separators: solid right/left triangle, or its outline. */
        int solid = cp == 0xE0B0 || cp == 0xE0B2;
        double ax = cp == 0xE0B0 || cp == 0xE0B1 ? x : x + w;
        double bx = cp == 0xE0B0 || cp == 0xE0B1 ? x + w : x;
        cairo_move_to(cr, ax, y);
        cairo_line_to(cr, bx, y + ch / 2);
        cairo_line_to(cr, ax, y + ch);
        if (solid) {
            cairo_close_path(cr);
            cairo_fill(cr);
        } else {
            cairo_set_line_width(cr, 2);
            cairo_stroke(cr);
        }
        return 1;
    }

    return 0;
}

static void draw_cursor(cairo_t* cr, const PinwinCursor* cursor, const char* text,
                        int text_len) {
    uint8_t bg[3], fg[3];
    double x = cursor->x * g_cell_w;
    double y = cursor->y * g_cell_h;

    pinwin_colors(bg, fg);
    set_rgb(cr, fg[0], fg[1], fg[2]);

    switch (cursor->style) {
        case PINWIN_CURSOR_BAR:
            cairo_rectangle(cr, x, y, 2, g_cell_h);
            cairo_fill(cr);
            break;
        case PINWIN_CURSOR_UNDERLINE:
            cairo_rectangle(cr, x, y + g_cell_h - 2, g_cell_w, 2);
            cairo_fill(cr);
            break;
        case PINWIN_CURSOR_BLOCK_HOLLOW:
            cairo_set_line_width(cr, 1);
            cairo_rectangle(cr, x + 0.5, y + 0.5, g_cell_w - 1, g_cell_h - 1);
            cairo_stroke(cr);
            break;
        case PINWIN_CURSOR_BLOCK:
        default:
            cairo_rectangle(cr, x, y, g_cell_w, g_cell_h);
            cairo_fill(cr);
            if (text && text_len > 0 && !cursor->wide_tail) {
                PinwinCell cell;
                memset(&cell, 0, sizeof(cell));
                cell.x = cursor->x;
                cell.y = cursor->y;
                cell.len = text_len < (int32_t)sizeof(cell.text) - 1
                               ? text_len
                               : (int32_t)sizeof(cell.text) - 1;
                memcpy(cell.text, text, (size_t)cell.len);
                cell.text[cell.len] = '\0';
                set_rgb(cr, bg[0], bg[1], bg[2]);
                draw_text(cr, pango_cairo_create_layout(cr), &cell);
            }
            break;
    }
}

/* ---- kitty graphics ----------------------------------------------------- */

/* One cairo surface per kitty image, rebuilt when the image is retransmitted
 * and dropped as soon as a frame draws no placement for it, so a long session
 * cannot accumulate surfaces. */
struct image_cache {
    uint32_t image_id;
    int64_t generation;
    cairo_surface_t* surface;
    uint64_t last_frame;
    struct image_cache* next;
};
static struct image_cache* g_images;
static uint64_t g_image_frame;

static cairo_surface_t* image_surface(const PinwinImage* img, uint64_t frame) {
    struct image_cache* e;
    int32_t w = img->image_w, h = img->image_h;

    for (e = g_images; e; e = e->next) {
        if (e->image_id != img->image_id) continue;
        if (e->generation == img->generation) {
            e->last_frame = frame;
            return e->surface;
        }
        break;
    }
    if (w <= 0 || h <= 0 || !img->pixels) return NULL;

    cairo_surface_t* s = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, w, h);
    if (cairo_surface_status(s) != CAIRO_STATUS_SUCCESS) {
        cairo_surface_destroy(s);
        return NULL;
    }
    unsigned char* dst = cairo_image_surface_get_data(s);
    int stride = cairo_image_surface_get_stride(s);
    for (int32_t y = 0; y < h; y++) {
        const uint8_t* src = img->pixels + (size_t)y * (size_t)w * 4;
        uint32_t* row = (uint32_t*)(dst + (size_t)y * (size_t)stride);
        for (int32_t x = 0; x < w; x++) {
            uint32_t r = src[0], g = src[1], b = src[2], a = src[3];
            /* ARGB32 is premultiplied on little-endian. */
            uint32_t pr = (r * a + 127) / 255;
            uint32_t pg = (g * a + 127) / 255;
            uint32_t pb = (b * a + 127) / 255;
            row[x] = (a << 24) | (pr << 16) | (pg << 8) | pb;
            src += 4;
        }
    }
    cairo_surface_mark_dirty(s);

    if (e) {
        cairo_surface_destroy(e->surface);
        e->surface = s;
        e->generation = img->generation;
        e->last_frame = frame;
        return s;
    }

    e = calloc(1, sizeof(*e));
    if (!e) {
        cairo_surface_destroy(s);
        return NULL;
    }
    e->image_id = img->image_id;
    e->generation = img->generation;
    e->surface = s;
    e->last_frame = frame;
    e->next = g_images;
    g_images = e;
    return s;
}

static void image_cache_evict(uint64_t frame) {
    struct image_cache** link = &g_images;
    while (*link) {
        struct image_cache* e = *link;
        if (e->last_frame == frame) {
            link = &e->next;
            continue;
        }
        *link = e->next;
        cairo_surface_destroy(e->surface);
        free(e);
    }
}

static void draw_images(cairo_t* cr) {
    PinwinImage img;
    const uint64_t frame = ++g_image_frame;

    while (pinwin_image_next(&img)) {
        cairo_surface_t* s = image_surface(&img, frame);
        if (!s) continue;
        if (img.sw <= 0 || img.sh <= 0 || img.w <= 0 || img.h <= 0) continue;
        cairo_save(cr);
        cairo_rectangle(cr, img.x, img.y, img.w, img.h);
        cairo_clip(cr);
        cairo_scale(cr, (double)img.w / (double)img.sw, (double)img.h / (double)img.sh);
        cairo_set_source_surface(cr, s, img.x - img.sx, img.y - img.sy);
        cairo_paint(cr);
        cairo_restore(cr);
    }
    image_cache_evict(frame);
}

/* ---- draw callback ------------------------------------------------------ */

static void apply_size(void);
static void resolve_layout_monitor(void);

static void on_draw(GtkDrawingArea* area, cairo_t* cr, int width, int height,
                    gpointer user_data) {
    uint8_t bg[3], fg[3];
    PinwinCell cell;
    PinwinCursor cursor;
    char cursor_text[32];
    int cursor_text_len = 0;
    int have_cursor_text = 0;

    (void)area;
    (void)user_data;

    if (g_layout_latch) resolve_layout_monitor();
    if (getenv("PINWIN_DEBUG"))
        fprintf(stderr, "pinwin: draw %dx%d latch=%d side=%d margins t%d b%d l%d r%d\n",
                width, height, g_layout_latch, g_layout.side, g_layout.top,
                g_layout.bottom, g_layout.left, g_layout.right);

    pinwin_colors(bg, fg);
    set_rgb(cr, g_theme_bg[0], g_theme_bg[1], g_theme_bg[2]);
    cairo_rectangle(cr, 0, 0, width, height);
    cairo_fill(cr);

    if (!pinwin_frame_begin()) return;

    PangoLayout* layout = pango_cairo_create_layout(cr);

    while (pinwin_cell_next(&cell)) {
        if (cell.has_bg) {
            set_rgb(cr, cell.br, cell.bg, cell.bb);
            int row_height = cell.y == height / g_cell_h - 1
                                 ? height - cell.y * g_cell_h : g_cell_h;
            cairo_rectangle(cr, cell.x * g_cell_w, cell.y * g_cell_h, g_cell_w,
                            row_height);
            cairo_fill(cr);
        }
    }
    pinwin_frame_rewind();
    while (pinwin_cell_next(&cell)) {
        if (cell.wide == PINWIN_WIDE_SPACER_TAIL) continue; /* do not render */
        if (cell.len == 0 || (cell.flags & PINWIN_INVISIBLE)) continue;
        set_rgb(cr, cell.has_fg ? cell.fr : g_theme_fg[0],
                cell.has_fg ? cell.fg : g_theme_fg[1],
                cell.has_fg ? cell.fb : g_theme_fg[2]);
        if (!draw_sprite(cr, &cell, first_codepoint(cell.text, cell.len)))
            draw_text(cr, layout, &cell);

        if ((cell.flags & PINWIN_UNDERLINE) || (cell.flags & PINWIN_STRIKETHROUGH)) {
            double x = cell.x * g_cell_w;
            double y = cell.y * g_cell_h;
            cairo_set_line_width(cr, 1);
            if (cell.flags & PINWIN_UNDERLINE) {
                cairo_move_to(cr, x, y + g_ascent + 1.5);
                cairo_line_to(cr, x + g_cell_w, y + g_ascent + 1.5);
            }
            if (cell.flags & PINWIN_STRIKETHROUGH) {
                cairo_move_to(cr, x, y + g_cell_h / 2.0);
                cairo_line_to(cr, x + g_cell_w, y + g_cell_h / 2.0);
            }
            cairo_stroke(cr);
        }

        if (pinwin_cursor(&cursor) && cursor.has_value && cursor.x == cell.x &&
            cursor.y == cell.y && cell.len > 0 && !have_cursor_text) {
            cursor_text_len = cell.len < (int32_t)sizeof(cursor_text) - 1
                                  ? cell.len
                                  : (int32_t)sizeof(cursor_text) - 1;
            memcpy(cursor_text, cell.text, (size_t)cursor_text_len);
            cursor_text[cursor_text_len] = '\0';
            have_cursor_text = 1;
        }
    }

    g_object_unref(layout);

    if (pinwin_cursor(&cursor) && cursor.has_value)
        draw_cursor(cr, &cursor, have_cursor_text ? cursor_text : NULL,
                    have_cursor_text ? cursor_text_len : 0);

    draw_images(cr);
    pinwin_frame_end();
}

/* ---- geometry ----------------------------------------------------------- */

static void spawn_pty(void);

static void apply_size(void) {
    int height = gtk_widget_get_height(g_area);
    if (g_cell_h > 0 && height > 0) {
        int32_t rows = height / g_cell_h;
        if (rows < 1) rows = 1;
        /* Rows follow the allocated height; columns stay the input and are
         * never re-derived from the allocated width (design D4). A width-only
         * Apply changes g_cols and must resize the grid and PTY too. */
        if (rows != g_rows || g_cols != g_grid_cols) {
            g_rows = rows;
            g_grid_cols = g_cols;
            pinwin_size(g_cols, g_rows, g_cell_w, g_cell_h);
            glue_pty_resize(g_cols, g_rows, g_cols * g_cell_w, g_rows * g_cell_h);
        }
    }
    if (!g_spawned) spawn_pty();
}

static void on_area_resize(GtkWidget* widget, gint width, gint height,
                           gpointer user_data) {
    (void)widget;
    (void)width;
    (void)height;
    (void)user_data;
    apply_size();
}

void glue_queue_draw(void) {
    if (g_area) gtk_widget_queue_draw(g_area);
}

/* ---- pty ---------------------------------------------------------------- */

static void on_child_exit(GPid pid, gint status, gpointer user_data) {
    (void)user_data;
    g_spawn_close_pid(pid);
    if (WIFEXITED(status)) {
        glue_exit(WEXITSTATUS(status));
    } else {
        glue_exit(128 + WTERMSIG(status));
    }
}

static gboolean on_pty_readable(gint fd, GIOCondition condition, gpointer user_data) {
    uint8_t buf[65536];
    (void)condition;
    (void)user_data;

    for (;;) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n > 0) {
            pinwin_pty_data(buf, (size_t)n);
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) return G_SOURCE_CONTINUE;
        return G_SOURCE_CONTINUE;
    }
}

void glue_pty_write(const uint8_t* data, size_t len) {
    size_t off = 0;
    if (g_pty_fd < 0) return;
    while (off < len) {
        ssize_t n = write(g_pty_fd, data + off, len - off);
        if (n > 0) {
            off += (size_t)n;
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) continue;
        break;
    }
}

void glue_pty_resize(int32_t cols, int32_t rows, int32_t xpixel, int32_t ypixel) {
    struct winsize ws;
    g_pty_cols = cols;
    g_pty_rows = rows;
    g_pty_xpixel = xpixel;
    g_pty_ypixel = ypixel;
    if (g_pty_fd < 0) return;
    memset(&ws, 0, sizeof(ws));
    ws.ws_col = (unsigned short)cols;
    ws.ws_row = (unsigned short)rows;
    ws.ws_xpixel = (unsigned short)xpixel;
    ws.ws_ypixel = (unsigned short)ypixel;
    ioctl(g_pty_fd, TIOCSWINSZ, &ws);
}

static void spawn_pty(void) {
    struct winsize ws;
    int fd = -1;
    pid_t pid;
    const char* term;

    if (g_spawned) return;
    if (!g_argv || !g_argv[0]) return;
    g_spawned = 1;

    memset(&ws, 0, sizeof(ws));
    ws.ws_col = (unsigned short)(g_rows > 0 ? g_cols : g_pty_cols);
    ws.ws_row = (unsigned short)(g_rows > 0 ? g_rows : g_pty_rows);
    ws.ws_xpixel = (unsigned short)(g_cols * g_cell_w);
    ws.ws_ypixel = (unsigned short)(ws.ws_row * g_cell_h);

    term = terminfo_exists("xterm-ghostty") ? "xterm-ghostty" : "xterm-256color";

    pid = forkpty(&fd, NULL, NULL, &ws);
    if (pid < 0) {
        fprintf(stderr, "pinwin: forkpty: %s\n", strerror(errno));
        exit(1);
    }
    if (pid == 0) {
        setenv("TERM", term, 1);
        setenv("COLORTERM", "truecolor", 1);
        execvp(g_argv[0], g_argv);
        fprintf(stderr, "pinwin: %s: %s\n", g_argv[0], strerror(errno));
        _exit(127);
    }

    g_pty_fd = fd;
    g_pty_cols = ws.ws_col;
    g_pty_rows = ws.ws_row;
    g_pty_xpixel = ws.ws_xpixel;
    g_pty_ypixel = ws.ws_ypixel;
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK);
    g_pty_source = g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR,
                                 on_pty_readable, NULL);
    g_child_watch_add(pid, on_child_exit, NULL);
}

/* ---- png decoding ------------------------------------------------------- */

int glue_decode_png(const uint8_t* data, size_t len, uint8_t** out_pixels,
                    uint32_t* out_w, uint32_t* out_h) {
    GdkPixbufLoader* loader = gdk_pixbuf_loader_new_with_type("png", NULL);
    GdkPixbuf* pixbuf = NULL;
    GdkPixbuf* alpha = NULL;
    uint8_t* out;

    if (!loader) return 0;
    if (!gdk_pixbuf_loader_write(loader, data, len, NULL)) {
        g_object_unref(loader);
        return 0;
    }
    if (!gdk_pixbuf_loader_close(loader, NULL)) {
        g_object_unref(loader);
        return 0;
    }
    pixbuf = gdk_pixbuf_loader_get_pixbuf(loader);
    if (!pixbuf) {
        g_object_unref(loader);
        return 0;
    }

    alpha = gdk_pixbuf_add_alpha(pixbuf, FALSE, 0, 0, 0);
    if (!alpha) {
        g_object_unref(loader);
        return 0;
    }

    int w = gdk_pixbuf_get_width(alpha);
    int h = gdk_pixbuf_get_height(alpha);
    int channels = gdk_pixbuf_get_n_channels(alpha);
    int stride = gdk_pixbuf_get_rowstride(alpha);
    const guint8* pixels = gdk_pixbuf_get_pixels(alpha);

    /* Owned by the caller (main.zig's PNG decode callback), which frees it
     * with free() after copying it into libghostty-vt's allocator. */
    out = malloc((size_t)w * (size_t)h * 4);
    if (!out) {
        g_object_unref(alpha);
        g_object_unref(loader);
        return 0;
    }
    for (int y = 0; y < h; y++) {
        const guint8* src = pixels + (size_t)y * (size_t)stride;
        uint8_t* dst = out + (size_t)y * (size_t)w * 4;
        for (int x = 0; x < w; x++) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = channels == 4 ? src[3] : 0xff;
            src += channels;
            dst += 4;
        }
    }

    g_object_unref(alpha);
    g_object_unref(loader);
    *out_pixels = out;
    *out_w = (uint32_t)w;
    *out_h = (uint32_t)h;
    return 1;
}

/* ---- input -------------------------------------------------------------- */

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

static gboolean on_key_pressed(GtkEventControllerKey* controller, guint keyval,
                               guint keycode, GdkModifierType state,
                               gpointer user_data) {
    GdkEvent* event = gtk_event_controller_get_current_event(
        GTK_EVENT_CONTROLLER(controller));
    uint32_t consumed = 0;
    int is_modifier = 0;

    (void)user_data;
    if (event) {
        consumed = mods_from_gdk(
            gdk_key_event_get_consumed_modifiers(event));
        is_modifier = gdk_key_event_is_modifier(event);
        g_key_layout = (int)gdk_key_event_get_layout(event);
    }
    pinwin_key(PINWIN_KEY_PRESS, (int32_t)keyval, (int32_t)keycode,
                mods_from_gdk(state), consumed, is_modifier);
    return TRUE;
}

static gboolean on_key_released(GtkEventControllerKey* controller, guint keyval,
                                guint keycode, GdkModifierType state,
                                gpointer user_data) {
    GdkEvent* event = gtk_event_controller_get_current_event(
        GTK_EVENT_CONTROLLER(controller));
    uint32_t consumed = 0;
    int is_modifier = 0;

    (void)user_data;
    if (event) {
        consumed = mods_from_gdk(
            gdk_key_event_get_consumed_modifiers(event));
        is_modifier = gdk_key_event_is_modifier(event);
    }
    pinwin_key(PINWIN_KEY_RELEASE, (int32_t)keyval, (int32_t)keycode,
                mods_from_gdk(state), consumed, is_modifier);
    return TRUE;
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
    button = gtk_gesture_single_get_current_button(GTK_GESTURE_SINGLE(gesture));
    pinwin_mouse(PINWIN_MOUSE_RELEASE, x, y, pinwin_button_from_gdk(button),
                  motion_mods(GTK_EVENT_CONTROLLER(gesture)));
}

static void on_motion(GtkEventControllerMotion* controller, double x, double y,
                      gpointer user_data) {
    (void)user_data;
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
    pinwin_focus(1);
}

static void on_focus_leave(GtkEventControllerFocus* controller, gpointer user_data) {
    (void)controller;
    (void)user_data;
    pinwin_focus(0);
}

/* ---- window ------------------------------------------------------------- */

static void attach_controllers(GtkWidget* area) {
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

/* ---- layout surfaces (add-pinwin-tray-options) -------------------------- */

/* Width follows the applied column count. GTK4 has no gtk_window_resize and
 * gtk_window_set_default_size does not move a mapped window, so the drawing
 * area's natural width is the mechanism. The window default size is updated
 * too: GtkWindow treats it as a size floor, so leaving the launch value there
 * would pin the panel at its launch width once it shrinks (design D3). */
static void apply_panel_width(void) {
    if (g_area) gtk_widget_set_size_request(g_area, g_cols * g_cell_w, -1);
    if (g_win) gtk_window_set_default_size(g_win, g_cols * g_cell_w, -1);
}

/* Push the applied layout onto both surfaces in one main-loop turn (design
 * D5): the visible panel gets its side anchor and directional margins, the
 * transparent reservation keeps zero margins and full height on the same
 * side with an explicit exclusive zone of left + panel + right. Never leaves
 * both horizontal anchors set. */
static void apply_layout_surfaces(void) {
    int32_t margin, reservation;

    if (!pinwin_side_geometry(&g_layout, g_cols * g_cell_w, &margin, &reservation))
        return; /* callers validate first; this is belt and braces */

    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_LEFT,
                         g_layout.side == PINWIN_SIDE_LEFT);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_RIGHT,
                         g_layout.side == PINWIN_SIDE_RIGHT);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_TOP, TRUE);
    gtk_layer_set_anchor(g_win, GTK_LAYER_SHELL_EDGE_BOTTOM, TRUE);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_LEFT,
                         g_layout.side == PINWIN_SIDE_LEFT ? margin : 0);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_RIGHT,
                         g_layout.side == PINWIN_SIDE_RIGHT ? margin : 0);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_TOP, g_layout.top);
    gtk_layer_set_margin(g_win, GTK_LAYER_SHELL_EDGE_BOTTOM, g_layout.bottom);

    if (g_reserve) {
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_LEFT,
                             g_layout.side == PINWIN_SIDE_LEFT);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_RIGHT,
                             g_layout.side == PINWIN_SIDE_RIGHT);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_TOP, TRUE);
        gtk_layer_set_anchor(g_reserve, GTK_LAYER_SHELL_EDGE_BOTTOM, TRUE);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_LEFT, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_RIGHT, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_TOP, 0);
        gtk_layer_set_margin(g_reserve, GTK_LAYER_SHELL_EDGE_BOTTOM, 0);
        gtk_layer_set_exclusive_zone(g_reserve, reservation);
        if (g_monitor) gtk_layer_set_monitor(g_reserve, g_monitor);
    }
    if (g_area) gtk_widget_queue_draw(g_area);
}

/* Load the saved layout and validate it against the resolved monitor and the
 * cell metrics. Anything unusable falls back to the launch baseline with a
 * diagnostic and without rewriting the file (design D4). */
static void load_saved_layout(GdkMonitor* monitor) {
    PinwinLayout saved;
    GdkRectangle geom;
    int config_result;

    saved.cols = g_cols; /* an absent cols key keeps the launch COLS */
    config_result = pinwin_config_load(&saved);

    if (config_result != PINWIN_CONFIG_LOADED) return;
    gdk_monitor_get_geometry(monitor, &geom);
    if (pinwin_layout_validate(&saved, saved.cols, g_cell_w, g_cell_h,
                                geom.width, geom.height) != PINWIN_GEOM_OK) {
        fprintf(stderr, "pinwin: saved layout unusable on this output; "
                        "using launch defaults\n");
        return;
    }
    g_cols = saved.cols;
    g_layout = saved;
    apply_panel_width();
}

/* The instance's currently applied layout (options window initialisation). */
void glue_current_layout(PinwinLayout* out) { *out = g_layout; }

/* Geometry inputs for layout validation (options window Apply). */
void glue_layout_metrics(int32_t* cols, int32_t* cell_w, int32_t* cell_h,
                         int32_t* output_w, int32_t* output_h) {
    *cols = g_cols;
    *cell_w = g_cell_w;
    *cell_h = g_cell_h;
    *output_w = 0;
    *output_h = 0;
    if (g_monitor) {
        GdkRectangle geom;
        gdk_monitor_get_geometry(g_monitor, &geom);
        *output_w = geom.width;
        *output_h = geom.height;
    }
}

int glue_publish_layout(const PinwinLayout* layout) {
    if (!g_monitor || !g_win) return PINWIN_GEOM_ERR_METRICS;
    {
        GdkRectangle geom;
        gdk_monitor_get_geometry(g_monitor, &geom);
        /* Validate the staged column count, not the one already applied: a
         * rejected Apply must leave g_cols untouched (design D3). */
        if (pinwin_layout_validate(layout, layout->cols, g_cell_w, g_cell_h,
                                    geom.width, geom.height) != PINWIN_GEOM_OK)
            return PINWIN_GEOM_ERR_METRICS;
    }
    g_layout = *layout;
    g_cols = layout->cols;
    apply_panel_width();
    apply_layout_surfaces();
    apply_size();
    return PINWIN_GEOM_OK;
}

static void on_win_map(GtkWidget* widget, gpointer user_data) {
    (void)user_data;

    /* The visible panel's original monitor is resolved on the first frame
     * (g_layout_latch): at map time the compositor has not yet told the
     * surface which output it is on, so gdk_display_get_monitor_at_surface
     * still reports a fallback. Once resolved, the saved layout is loaded,
     * validated against that monitor and the reservation is pinned to it
     * (design D5). */
    g_layout_latch = 1;

    /* The reservation is created and presented only after the visible panel
     * is mapped; the first draw then pins it to the resolved monitor. */
    {
        GtkApplication* app = GTK_APPLICATION(g_application_get_default());
        GtkWindow* reserve = GTK_WINDOW(gtk_application_window_new(app));
        gtk_layer_init_for_window(reserve);
        gtk_layer_set_namespace(reserve, "pinwin-reserve");
        gtk_layer_set_layer(reserve, GTK_LAYER_SHELL_LAYER_BOTTOM);
        gtk_window_set_default_size(reserve, 1, -1);
        gtk_widget_set_opacity(GTK_WIDGET(reserve), 0.0);
        g_reserve = reserve;
    }

    apply_layout_surfaces();
    gtk_window_present(g_reserve);
    apply_size();
}

/* One-shot, from the first draw after map: by then the surface has entered
 * its output, so the monitor reported here is the panel's real monitor. */
static void resolve_layout_monitor(void) {
    g_layout_latch = 0;
    g_monitor = gdk_display_get_monitor_at_surface(
        gdk_display_get_default(),
        gtk_native_get_surface(GTK_NATIVE(g_win)));
    load_saved_layout(g_monitor);
    apply_layout_surfaces();
    apply_size();
}

static void on_activate(GtkApplication* app, gpointer user_data) {
    (void)user_data;

    GtkWidget* win = gtk_application_window_new(app);
    g_win = GTK_WINDOW(win);

    gtk_layer_init_for_window(g_win);
    gtk_layer_set_namespace(g_win, "pinwin");
    gtk_layer_set_layer(g_win, GTK_LAYER_SHELL_LAYER_OVERLAY);
    gtk_layer_set_keyboard_mode(g_win, (GtkLayerShellKeyboardMode)g_keyboard_mode);

    cell_metrics_update(win);
    g_layout = pinwin_layout_default(g_cols, g_gutter);

    g_area = gtk_drawing_area_new();
    gtk_drawing_area_set_draw_func(GTK_DRAWING_AREA(g_area), on_draw, NULL, NULL);
    g_signal_connect(g_area, "resize", G_CALLBACK(on_area_resize), NULL);
    attach_controllers(g_area);
    gtk_widget_set_focusable(g_area, TRUE);
    gtk_window_set_child(g_win, g_area);

    /* Width is fixed by COLS; the top/bottom anchors give the height. */
    gtk_window_set_default_size(g_win, g_cols * g_cell_w, -1);
    gtk_layer_set_exclusive_zone(g_win, -1); /* ignore Noctalia's top zone */

    /* Initial anchors/margins from the launch baseline; the map callback
     * loads the saved layout, resolves the original monitor and presents the
     * reservation once those are known. */
    apply_layout_surfaces();
    g_signal_connect(win, "map", G_CALLBACK(on_win_map), NULL);

    gtk_window_present(g_win);
    apply_size();
    tray_init();
}

int glue_init(int32_t cols, int32_t gutter, int32_t keyboard_mode) {
    char theme_name[128];

    theme_colours(theme_name, sizeof(theme_name), g_theme_bg, g_theme_fg);
    if (getenv("PINWIN_DEBUG") && theme_name[0])
        fprintf(stderr, "pinwin: theme %s bg=#%02x%02x%02x fg=#%02x%02x%02x\n",
                theme_name, g_theme_bg[0], g_theme_bg[1], g_theme_bg[2],
                g_theme_fg[0], g_theme_fg[1], g_theme_fg[2]);
    g_cols = cols;
    g_gutter = gutter;
    g_keyboard_mode = keyboard_mode;

    if (!gtk_init_check()) {
        fprintf(stderr, "pinwin: no display\n");
        return 0;
    }
    if (!gtk_layer_is_supported()) {
        fprintf(stderr, "pinwin: compositor does not support wlr-layer-shell\n");
        return 0;
    }

    g_app = gtk_application_new(NULL, G_APPLICATION_DEFAULT_FLAGS);
    if (!g_app) return 0;
    g_signal_connect(g_app, "activate", G_CALLBACK(on_activate), NULL);
    return 1;
}

static gboolean on_tick(gpointer data) {
    (void)data;
    glue_queue_draw();
    return G_SOURCE_CONTINUE;
}

void glue_start(char* const argv[]) {
    g_argv = argv;
    g_application_run(G_APPLICATION(g_app), 0, NULL);
}

void glue_exit(int32_t status) {
    tray_shutdown();
    if (g_pty_source) g_source_remove(g_pty_source);
    exit(status);
}
