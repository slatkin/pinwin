/*
 * images.c - the kitty-graphics side of pinwin: the cairo surfaces built from
 * the terminal's image placements and their per-frame cache, plus the PNG
 * decode hook libghostty-vt calls back into (design D4).
 *
 * Shared state lives in glue_internal.h; the placements themselves are read
 * through pinwin_image_next (main.zig).
 */

#include "glue_internal.h"
#include "pinwin.h"

#include <stdlib.h>

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

void draw_images(cairo_t* cr) {
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
