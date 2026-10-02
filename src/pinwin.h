/*
 * pinwin.h - the interface between pinwin's C GTK glue (glue.c) and its
 * Zig terminal core (main.zig).
 *
 * Zig cannot @cImport GTK4 (translate-c crashes on the header chain), so
 * every GTK, Pango, cairo, GdkPixbuf and PTY call lives in glue.c and this
 * header deliberately contains no GTK types at all: plain integers, plain
 * structs and pointers.
 *
 * The PINWIN_* constants mirror the libghostty-vt C enums of the same
 * concept, so the Zig side can pass them straight through.
 *
 * glue_*  functions are implemented in C and called from Zig.
 * pinwin_* functions are implemented in Zig and called from C.
 */
#ifndef PINWIN_H
#define PINWIN_H

#include <stddef.h>
#include <stdint.h>

/* GhosttyStyle flags for PinwinCell.flags. */
#define PINWIN_BOLD (1u << 0)
#define PINWIN_ITALIC (1u << 1)
#define PINWIN_INVERSE (1u << 2)
#define PINWIN_FAINT (1u << 3)
#define PINWIN_INVISIBLE (1u << 4)
#define PINWIN_STRIKETHROUGH (1u << 5)
#define PINWIN_UNDERLINE (1u << 6)

/* GhosttyRenderStateCursorVisualStyle */
#define PINWIN_CURSOR_BAR 0
#define PINWIN_CURSOR_BLOCK 1
#define PINWIN_CURSOR_UNDERLINE 2
#define PINWIN_CURSOR_BLOCK_HOLLOW 3

/* GtkLayerShellKeyboardMode */
#define PINWIN_KEYBOARD_NONE 0
#define PINWIN_KEYBOARD_EXCLUSIVE 1
#define PINWIN_KEYBOARD_ON_DEMAND 2

/* GhosttyKeyAction */
#define PINWIN_KEY_RELEASE 0
#define PINWIN_KEY_PRESS 1
#define PINWIN_KEY_REPEAT 2

/* GhosttyMouseAction */
#define PINWIN_MOUSE_PRESS 0
#define PINWIN_MOUSE_RELEASE 1
#define PINWIN_MOUSE_MOTION 2

/* GhosttyMouseButton */
#define PINWIN_MOUSE_UNKNOWN 0
#define PINWIN_MOUSE_LEFT 1
#define PINWIN_MOUSE_RIGHT 2
#define PINWIN_MOUSE_MIDDLE 3
#define PINWIN_MOUSE_FOUR 4 /* wheel up */
#define PINWIN_MOUSE_FIVE 5 /* wheel down */
#define PINWIN_MOUSE_SIX 6
#define PINWIN_MOUSE_SEVEN 7
#define PINWIN_MOUSE_EIGHT 8
#define PINWIN_MOUSE_NINE 9

/* GdkScrollUnit */
#define PINWIN_SCROLL_UNIT_WHEEL 0
#define PINWIN_SCROLL_UNIT_SURFACE 1

/* GhosttyMods */
#define PINWIN_MOD_SHIFT (1u << 0)
#define PINWIN_MOD_CTRL (1u << 1)
#define PINWIN_MOD_ALT (1u << 2)
#define PINWIN_MOD_SUPER (1u << 3)
#define PINWIN_MOD_CAPS_LOCK (1u << 4)

/* One grapheme of the frame the C draw callback should paint. */
typedef struct {
    int32_t x; /* viewport column */
    int32_t y; /* viewport row */
    int32_t wide; /* one of the PINWIN_WIDE_* values below */
    int32_t cw; /* cells this glyph may use when constrained (1 or 2) */
    int32_t len; /* UTF-8 byte count of text; 0 means the cell has no glyph */
    char text[32];
    int32_t has_fg; /* 0 means use the default foreground */
    uint8_t fr, fg, fb;
    int32_t has_bg; /* 0 means use the default background */
    uint8_t br, bg, bb;
    uint32_t flags;
} PinwinCell;

/* Cell width, mirroring GHOSTTY_CELL_WIDE_*: a wide cell spans two columns and
 * its tail must not be drawn at all. */
#define PINWIN_WIDE_NARROW 0
#define PINWIN_WIDE_WIDE 1
#define PINWIN_WIDE_SPACER_TAIL 2

typedef struct {
    int32_t has_value; /* cursor is visible and has a viewport position */
    int32_t x;
    int32_t y;
    int32_t style;
    int32_t wide_tail;
} PinwinCursor;

typedef struct {
    uint32_t image_id;
    int64_t generation;
    int32_t z;
    /* Destination rectangle in device pixels, relative to the widget. */
    int32_t x, y, w, h;
    /* Source rectangle in image pixels. */
    int32_t sx, sy, sw, sh;
    int32_t image_w, image_h;
    /* Straight (non-premultiplied) RGBA8 pixels of the whole image. */
    const uint8_t* pixels;
} PinwinImage;

/* ---- implemented in glue.c, called from Zig ---------------------------- */

/* Window metrics, valid once glue_init returned 1. */
int32_t glue_cell_width(void);
int32_t glue_cell_height(void);
void glue_queue_draw(void);

void glue_pty_write(const uint8_t* data, size_t len);
void glue_pty_resize(int32_t cols, int32_t rows);

/* Straight RGBA8 text produced by a keyval, 0 for none. */
uint32_t glue_keyval_unicode(uint32_t keyval);
/* The level-0 codepoint for a hardware keycode: the kitty keyboard protocol's
 * "unshifted" value. 0 if unknown. */
uint32_t glue_keycode_unshifted_codepoint(uint32_t keycode);

/* Decode PNG into straight RGBA8. Returns 0 on failure. The caller owns
 * *out_pixels (malloc'd) and must free it. */
int glue_decode_png(const uint8_t* data, size_t len, uint8_t** out_pixels,
                    uint32_t* out_w, uint32_t* out_h);

/* ---- implemented in main.zig, called from C ---------------------------- */

/* Grid size changed: keep the terminal, the ioctl and the encoders in sync.
 * Returns 0 on success; nonzero means the terminal could not be allocated, so
 * the previous grid stays in effect (the library never exits — design D3). */
int pinwin_size(int32_t cols, int32_t rows, int32_t cell_w, int32_t cell_h);
/* Bytes read from the PTY. */
void pinwin_pty_data(const uint8_t* data, size_t len);
void pinwin_key(int32_t action, int32_t keyval, int32_t keycode, uint32_t mods,
                 uint32_t consumed_mods, int32_t is_modifier);
void pinwin_mouse(int32_t action, double x, double y, int32_t button, uint32_t mods);
void pinwin_scroll(double x, double y, double dx, double dy, int32_t unit,
                    uint32_t mods);
void pinwin_focus(int32_t gained);

/* Frame protocol: begin, paint cell backgrounds, rewind, paint cell glyphs,
 * then cursor, images, and end. */
int32_t pinwin_frame_begin(void);
void pinwin_frame_rewind(void);
int32_t pinwin_cell_next(PinwinCell* out);
int32_t pinwin_cursor(PinwinCursor* out);
int32_t pinwin_image_next(PinwinImage* out);
void pinwin_frame_end(void);
void pinwin_colors(uint8_t* bg, uint8_t* fg);

#endif /* PINWIN_H */
