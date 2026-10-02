/*
 * render.c - how pinwin draws a frame: cell metrics from the Pango font,
 * text through Pango with Ghostty's Nerd Font glyph constraints, the sprite
 * ranges drawn as graphics (blocks, braille, powerline), the cursor, and the
 * draw callback that walks the terminal's cells (design D4).
 *
 * Cell iteration is the frame protocol in pinwin.h; the pixels the callback
 * reads are produced by main.zig. Shared state lives in glue_internal.h.
 */

#include "glue_internal.h"
#include "nerd_font_tables.h"

#include <pango/pangocairo.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int32_t glue_cell_width(void) { return g_cell_w; }
int32_t glue_cell_height(void) { return g_cell_h; }

void cell_metrics_update(GtkWidget* widget) {
    PangoFontMetrics* metrics;
    int32_t digit;

    if (!g_font) {
        char* family;
        double size;
        font_config_load(&family, &size);
        g_font = pango_font_description_new();
        pango_font_description_set_family(g_font, family ? family : "monospace");
        pango_font_description_set_size(g_font, (int32_t)(size * PANGO_SCALE));
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

/* ---- draw callback ------------------------------------------------------ */

void on_draw(GtkDrawingArea* area, cairo_t* cr, int width, int height,
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
