/*
 * options.h - penguin's layout settings: the six values (docking side, the
 * applied column count and four directional gutters), their launch defaults,
 * strict parsing and checked geometry validation.
 *
 * Everything here is plain C so the lightweight check in tools/ can compile
 * this module without GTK. The GTK options editor and the GKeyFile config
 * I/O live in options.c alongside these.
 */
#ifndef PENGUIN_OPTIONS_H
#define PENGUIN_OPTIONS_H

#include <stdint.h>

/* Docking side. */
#define PENGUIN_SIDE_LEFT 0
#define PENGUIN_SIDE_RIGHT 1

/* Validation results from penguin_layout_validate. */
#define PENGUIN_GEOM_OK 0
#define PENGUIN_GEOM_ERR_SIDE 1      /* side is not PENGUIN_SIDE_* */
#define PENGUIN_GEOM_ERR_OVERFLOW 2  /* checked arithmetic would overflow */
#define PENGUIN_GEOM_ERR_NO_RESERVE 3 /* left + panel + right is negative */
#define PENGUIN_GEOM_ERR_NO_ROW 4    /* vertical space < one terminal row */
#define PENGUIN_GEOM_ERR_NO_WIDTH 5  /* reservation leaves no output width */
#define PENGUIN_GEOM_ERR_METRICS 6   /* nonsense panel/output dimensions */

typedef struct {
    int32_t side;                    /* PENGUIN_SIDE_* */
    int32_t cols;                    /* panel width in terminal columns */
    int32_t top, bottom, left, right; /* gutters in pixels, may be negative */
} PenguinLayout;

/* The launch baseline: left docking, the launch COLS column count,
 * top/bottom/left zero and right = the validated GUTTER value. */
PenguinLayout penguin_layout_default(int32_t cols, int32_t gutter);

/* Strict column parsing: digits only (no sign, no whitespace), 1 <= value <=
 * 65535 (the PTY winsize column field). Returns 1 and sets *out on success, 0
 * on anything else. */
int penguin_parse_cols(const char* text, int32_t* out);

/* Strict gutter parsing: an optional leading '-' (negative gutters push the
 * panel edge past the output edge or let tiles overlap the panel), then
 * digits only; no whitespace, value fits in int32_t. Returns 1 and sets *out
 * on success, 0 on anything else. */
int penguin_parse_gutter(const char* text, int32_t* out);

/* Horizontal placement for the docking side, with checked arithmetic.
 * *edge_margin is the visible panel's margin on its docking edge (Left when
 * docked left, Right when docked right); *reservation is
 * left + panel_width + right, the strip reserved at that edge. Returns 1 on
 * success, 0 on overflow (including a panel_width beyond int32_t). */
int penguin_side_geometry(const PenguinLayout* layout, long long panel_width,
                          int32_t* edge_margin, int32_t* reservation);

/* Validate a layout against the output and font metrics, all in logical
 * pixels. panel_width is panel_cols * cell_w, where panel_cols is the
 * layout's applied column count. Rejects unknown sides,
 * checked-arithmetic overflow, a reservation sum below zero
 * (left + panel_width + right), vertical space below one complete terminal
 * row (output_h - top - bottom < cell_h) and a reservation that leaves no
 * output width for other windows (reservation >= output_w). Negative
 * gutters are allowed (they move the panel edge past its output edge).
 * Returns one of the PENGUIN_GEOM_* values. */
int penguin_layout_validate(const PenguinLayout* layout, int32_t panel_cols,
                            int32_t cell_w, int32_t cell_h, int32_t output_w,
                            int32_t output_h);

/* Result codes for penguin_config_load. */
#define PENGUIN_CONFIG_LOADED 0
#define PENGUIN_CONFIG_ABSENT 1
#define PENGUIN_CONFIG_INVALID 2

/* Load the saved layout from `$XDG_CONFIG_HOME/penguin/config` (default
 * ~/.config/penguin/config). On PENGUIN_CONFIG_LOADED, *out holds a valid
 * layout. PENGUIN_CONFIG_ABSENT means no config file exists (normal; the
 * caller keeps its launch baseline). PENGUIN_CONFIG_INVALID means the file
 * exists but is unreadable, malformed or incomplete; a diagnostic has been
 * printed and the caller falls back to the baseline without rewriting the
 * file. Unknown keys are ignored. The caller initialises out->cols to the
 * launch COLS: a missing cols key leaves it in place, a malformed one
 * rejects the whole saved layout. Font, command and keyboard settings are
 * not part of the config. */
int penguin_config_load(PenguinLayout* out);

/* Atomically save the layout to the same path: create the directory when
 * needed, write to a temporary file and rename, mode 0600. The previous file
 * is left untouched on any failure. Returns 0 on success; on failure a
 * diagnostic is printed and non-zero returned. */
int penguin_config_save(const PenguinLayout* layout);

/* Implemented in glue.c (it owns the surfaces): validate `layout` against
 * the panel's original monitor and cell metrics, then publish it to both
 * surfaces, resize the terminal grid through the normal resize path and
 * force a redraw. Returns PENGUIN_GEOM_OK, or the validation error without
 * touching any live state. */
int glue_publish_layout(const PenguinLayout* layout);

/* Implemented in glue.c: the instance's currently applied layout. */
void glue_current_layout(PenguinLayout* out);

/* Implemented in glue.c: geometry inputs for validation — COLS, cell
 * metrics and the original monitor's size in logical pixels. */
void glue_layout_metrics(int32_t* cols, int32_t* cell_w, int32_t* cell_h,
                         int32_t* output_w, int32_t* output_h);

/* Open the singleton options window (or present the existing one without
 * resetting its pending edits). Implemented in options.c; called from
 * glue.c / tray.c. */
void penguin_options_open(void);

#endif /* PENGUIN_OPTIONS_H */
