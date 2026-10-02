/*
 * pty.c - the PTY side of pinwin: spawning the command on a forkpty, the
 * read/write paths libghostty-vt drives, the winsize resize path, and the
 * grid-size application that follows the drawing area's allocation
 * (design D4).
 *
 * Shared state lives in glue_internal.h; bytes flow through the pinwin_*
 * frame/callback functions in pinwin.h.
 */

#include "glue_internal.h"

#include <glib-unix.h>
#include <pty.h>

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

static void spawn_pty(void);

void apply_size(void) {
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

void on_area_resize(GtkWidget* widget, gint width, gint height,
                    gpointer user_data) {
    (void)widget;
    (void)width;
    (void)height;
    (void)user_data;
    apply_size();
}

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
        /* The command learns the control socket's address; without a socket,
         * an inherited value must never point at another instance. */
        if (g_control_socket_path[0])
            setenv("PINWIN_SOCKET", g_control_socket_path, 1);
        else
            unsetenv("PINWIN_SOCKET");
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
