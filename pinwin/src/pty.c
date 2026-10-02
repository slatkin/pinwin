/*
 * pty.c - the PTY side of pinwin: attaching the host-supplied master fd, the
 * read/write paths libghostty-vt drives, the winsize resize path, and the
 * grid-size application that follows the drawing area's allocation
 * (design D4, D6).
 *
 * Shared state lives in glue_internal.h; bytes flow through the pinwin_*
 * frame/callback functions in pinwin.h. The host owns the child side of the
 * pty, so there is no fork, no child wait and no child environment here.
 */

#include "glue_internal.h"

#include <glib-unix.h>

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static void attach_pty(void);

int apply_size(void) {
    int height = gtk_widget_get_height(g_area);
    if (g_cell_h > 0 && height > 0) {
        int32_t rows = height / g_cell_h;
        if (rows < 1) rows = 1;
        /* Rows follow the allocated height; columns stay the input and are
         * never re-derived from the allocated width (design D4). A width-only
         * Apply changes g_cols and must resize the grid and PTY too. */
        if (rows != g_rows || g_cols != g_grid_cols) {
            /* Push the grid first: a terminal that cannot be allocated leaves
             * the previous grid and PTY winsize in place (design D3). */
            if (pinwin_size(g_cols, rows, g_cell_w, g_cell_h) != 0) return 1;
            g_rows = rows;
            g_grid_cols = g_cols;
            glue_pty_resize(g_cols, g_rows, g_cols * g_cell_w, g_rows * g_cell_h);
        }
    }
    if (!g_attached) attach_pty();
    return 0;
}

void on_area_resize(GtkWidget* widget, gint width, gint height,
                    gpointer user_data) {
    (void)widget;
    (void)width;
    (void)height;
    (void)user_data;
    apply_size();
}

static gboolean on_pty_readable(gint fd, GIOCondition condition, gpointer user_data) {
    uint8_t buf[65536];
    (void)user_data;

    for (;;) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n > 0) {
            pinwin_pty_data(buf, (size_t)n);
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            if (condition & (G_IO_HUP | G_IO_ERR)) break;
            return G_SOURCE_CONTINUE;
        }
        /* EOF or a read error: the child side is gone. Drop the read source
         * and leave the host's process lifetime alone (design D6). */
        break;
    }
    g_pty_source = 0;
    g_pty_fd = -1;
    return G_SOURCE_REMOVE;
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

/* Take the host-supplied master fd: non-blocking, the initial winsize (grid
 * plus pixel size, as glue_pty_resize builds it) and the read source. The host
 * owns the child side, so there is nothing else to set up (design D6). A fd
 * that cannot be used only degrades to no terminal; it never exits (design D3). */
static void attach_pty(void) {
    struct winsize ws;
    int fd = g_pty_fd;
    int flags;

    if (g_attached) return;
    if (fd < 0) return;
    g_attached = 1;

    flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) < 0) return;

    memset(&ws, 0, sizeof(ws));
    ws.ws_col = (unsigned short)(g_rows > 0 ? g_cols : g_pty_cols);
    ws.ws_row = (unsigned short)(g_rows > 0 ? g_rows : g_pty_rows);
    ws.ws_xpixel = (unsigned short)(g_cols * g_cell_w);
    ws.ws_ypixel = (unsigned short)(ws.ws_row * g_cell_h);
    g_pty_cols = ws.ws_col;
    g_pty_rows = ws.ws_row;
    g_pty_xpixel = ws.ws_xpixel;
    g_pty_ypixel = ws.ws_ypixel;
    ioctl(fd, TIOCSWINSZ, &ws);

    g_pty_source = g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR,
                                 on_pty_readable, NULL);
}
