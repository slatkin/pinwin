/*
 * penguin.h - the interface between penguin's C GTK glue (glue.c) and its
 * Zig terminal core (main.zig).
 *
 * Zig cannot @cImport GTK4 (translate-c crashes on the header chain), so
 * every GTK, Pango, cairo, GdkPixbuf and PTY call lives in glue.c and this
 * header deliberately contains no GTK types at all: plain integers, plain
 * structs and pointers.
 *
 * The PENGUIN_* constants mirror the libghostty-vt C enums of the same
 * concept, so the Zig side can pass them straight through.
 *
 * glue_*  functions are implemented in C and called from Zig.
 * penguin_* functions are implemented in Zig and called from C.
 */
#ifndef PENGUIN_H
#define PENGUIN_H

#include <stddef.h>
#include <stdint.h>

/* GhosttyStyle flags for PenguinCell.flags. */
#define PENGUIN_BOLD (1u << 0)
#define PENGUIN_ITALIC (1u << 1)
#define PENGUIN_INVERSE (1u << 2)
#define PENGUIN_FAINT (1u << 3)
#define PENGUIN_INVISIBLE (1u << 4)
#define PENGUIN_STRIKETHROUGH (1u << 5)
#define PENGUIN_UNDERLINE (1u << 6)

/* GhosttyRenderStateCursorVisualStyle */
#define PENGUIN_CURSOR_BAR 0
#define PENGUIN_CURSOR_BLOCK 1
#define PENGUIN_CURSOR_UNDERLINE 2
#define PENGUIN_CURSOR_BLOCK_HOLLOW 3

/* GtkLayerShellKeyboardMode */
#define PENGUIN_KEYBOARD_NONE 0
#define PENGUIN_KEYBOARD_EXCLUSIVE 1
#define PENGUIN_KEYBOARD_ON_DEMAND 2

/* GhosttyKeyAction */
#define PENGUIN_KEY_RELEASE 0
#define PENGUIN_KEY_PRESS 1
#define PENGUIN_KEY_REPEAT 2

/* GhosttyMouseAction */
#define PENGUIN_MOUSE_PRESS 0
#define PENGUIN_MOUSE_RELEASE 1
#define PENGUIN_MOUSE_MOTION 2

/* GhosttyMouseButton */
#define PENGUIN_MOUSE_UNKNOWN 0
#define PENGUIN_MOUSE_LEFT 1
#define PENGUIN_MOUSE_RIGHT 2
#define PENGUIN_MOUSE_MIDDLE 3
#define PENGUIN_MOUSE_FOUR 4 /* wheel up */
#define PENGUIN_MOUSE_FIVE 5 /* wheel down */
#define PENGUIN_MOUSE_SIX 6
#define PENGUIN_MOUSE_SEVEN 7
#define PENGUIN_MOUSE_EIGHT 8
#define PENGUIN_MOUSE_NINE 9

/* GdkScrollUnit */
#define PENGUIN_SCROLL_UNIT_WHEEL 0
#define PENGUIN_SCROLL_UNIT_SURFACE 1

/* GhosttyMods */
#define PENGUIN_MOD_SHIFT (1u << 0)
#define PENGUIN_MOD_CTRL (1u << 1)
#define PENGUIN_MOD_ALT (1u << 2)
#define PENGUIN_MOD_SUPER (1u << 3)
#define PENGUIN_MOD_CAPS_LOCK (1u << 4)
#define PENGUIN_MOD_NUM_LOCK (1u << 5)

/* One grapheme of the frame the C draw callback should paint. */
typedef struct {
    int32_t x; /* viewport column */
    int32_t y; /* viewport row */
    int32_t wide; /* one of the PENGUIN_WIDE_* values below */
    int32_t cw; /* cells this glyph may use when constrained (1 or 2) */
    int32_t len; /* UTF-8 byte count of text; 0 means the cell has no glyph */
    char text[32];
    int32_t has_fg; /* 0 means use the default foreground */
    uint8_t fr, fg, fb;
    int32_t has_bg; /* 0 means use the default background */
    uint8_t br, bg, bb;
    uint32_t flags;
} PenguinCell;

/* Cell width, mirroring GHOSTTY_CELL_WIDE_*: a wide cell spans two columns and
 * its tail must not be drawn at all. */
#define PENGUIN_WIDE_NARROW 0
#define PENGUIN_WIDE_WIDE 1
#define PENGUIN_WIDE_SPACER_TAIL 2

typedef struct {
    int32_t has_value; /* cursor is visible and has a viewport position */
    int32_t x;
    int32_t y;
    int32_t style;
    int32_t wide_tail;
} PenguinCursor;

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
} PenguinImage;

/* ---- implemented in glue.c, called from Zig ---------------------------- */

/* gtk_init + application setup. Returns 0 on failure. `keyboard_mode` is one
 * of the PENGUIN_KEYBOARD_* values. */
int glue_init(int32_t cols, int32_t gutter, int32_t keyboard_mode);
/* Window metrics, valid once glue_init returned 1. */
int32_t glue_cell_width(void);
int32_t glue_cell_height(void);
/* Create the surface, spawn argv in a PTY and run the main loop. */
void glue_start(char* const argv[]);
void glue_queue_draw(void);
void glue_exit(int32_t status);

void glue_pty_write(const uint8_t* data, size_t len);
void glue_pty_resize(int32_t cols, int32_t rows, int32_t xpixel, int32_t ypixel);

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

/* Grid size changed: keep the terminal, the ioctl and the encoders in sync. */
void penguin_size(int32_t cols, int32_t rows, int32_t cell_w, int32_t cell_h);
/* Bytes read from the PTY. */
void penguin_pty_data(const uint8_t* data, size_t len);
void penguin_key(int32_t action, int32_t keyval, int32_t keycode, uint32_t mods,
                 uint32_t consumed_mods, int32_t is_modifier);
void penguin_mouse(int32_t action, double x, double y, int32_t button, uint32_t mods);
void penguin_scroll(double x, double y, double dx, double dy, int32_t unit,
                    uint32_t mods);
void penguin_focus(int32_t gained);

/* Frame protocol: begin, paint cell backgrounds, rewind, paint cell glyphs,
 * then cursor, images, and end. */
int32_t penguin_frame_begin(void);
void penguin_frame_rewind(void);
int32_t penguin_cell_next(PenguinCell* out);
int32_t penguin_cursor(PenguinCursor* out);
int32_t penguin_image_next(PenguinImage* out);
void penguin_frame_end(void);
void penguin_colors(uint8_t* bg, uint8_t* fg);

#endif /* PENGUIN_H */
