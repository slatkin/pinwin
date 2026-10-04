/*
 * demo/main.c - dev-only driver for libpinwin's C ABI (design OQ-a).
 *
 * Built only by `zig build demo` and never installed. It makes its own pty
 * pair, forks a canned child on the slave side and sets the child's terminal
 * environment, then drives the ABI from this thread: a canned startup layout,
 * a live pinwin_apply_layout, a rejected layout and pinwin_stop. Fork/exec is
 * fine here because this is a program; only the library must not.
 *
 * Commands on the demo's own stdin (one per line):
 *   <enter>  toggle side/width and apply live (resize/re-dock)
 *   e        animated width toggle 40 <-> 120 cols, same side (200 ms)
 *   p        the same toggle through the plain snap apply
 *   b        apply an invalid layout: expect PINWIN_ERR_INVALID, host lives
 *   q        stop the panel and exit
 * Run `exit` inside the panel to watch a pty hangup leave the host alone.
 *
 * DEMO_DENSE=1 replaces the shell child with a dense stand-in: a full 120+
 * column text grid, one kitty image on screen and light periodic traffic,
 * retransmitting the image after every SIGWINCH like a real TUI host does.
 */
#include <errno.h>
#include <fcntl.h>
#include <pty.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

#include "pinwin_api.h"

#define DEMO_COLS 40
#define DEMO_GUTTER 12
#define SETTLE_US 1500000

static int32_t demo_cols = DEMO_COLS;
static int dense_mode;

/* ---- dense child (DEMO_DENSE=1) ------------------------------------------ */

/* A stand-in for a real pinned TUI: a full 120+ column text grid, one kitty
 * image on screen and light periodic traffic. On SIGWINCH it repaints the
 * whole grid and retransmits the image, like a real host does after a
 * resize. Image bytes come from a system icon PNG. */
#define DENSE_IMAGE "/usr/share/icons/hicolor/512x512/apps/com.mitchellh.ghostty.png"

static volatile sig_atomic_t dense_resized = 1;

static void dense_on_winch(int sig) {
    (void)sig;
    dense_resized = 1;
}

static void dense_send_image(void) {
    static char b64[1 << 19];
    static const char tab[] =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    uint8_t raw[1 << 17];
    FILE* f = fopen(DENSE_IMAGE, "rb");
    size_t n, i, o = 0;
    int id, slot;

    if (!f) return;
    n = fread(raw, 1, sizeof(raw), f);
    fclose(f);
    for (i = 0; i + 2 < n; i += 3) {
        uint32_t v = (uint32_t)raw[i] << 16 | (uint32_t)raw[i + 1] << 8 | raw[i + 2];
        b64[o++] = tab[(v >> 18) & 63];
        b64[o++] = tab[(v >> 12) & 63];
        b64[o++] = tab[(v >> 6) & 63];
        b64[o++] = tab[v & 63];
    }
    if (i < n) {
        uint32_t v = (uint32_t)raw[i] << 16;
        if (i + 1 < n) v |= (uint32_t)raw[i + 1] << 8;
        b64[o++] = tab[(v >> 18) & 63];
        b64[o++] = tab[(v >> 12) & 63];
        b64[o++] = i + 1 < n ? tab[(v >> 6) & 63] : '=';
        b64[o++] = '=';
    }
    b64[o] = 0;
    /* Four placements of the same PNG, distinct image ids: a ~480 KB kitty
     * burst per repaint, the same order as the real host's art re-encode.
     * q=1 so parse errors come back on stdin, where dump_stdin shows them. */
    for (id = 1; id <= 4; id++) {
        slot = id - 1;
        printf("\033[%d;%dH\033_Gf=24,a=T,i=%d,q=1,c=20,r=20;%s\033\\",
               2 + slot / 2 * 22, 2 + slot % 2 * 25, id, b64);
    }
}

/* Kitty responses (q=1 errors) arrive on stdin; dump them to stderr so the
 * demo log shows why an image did not display. */
static void dump_stdin(void) {
    char buf[256];
    int flags = fcntl(0, F_GETFL, 0);
    ssize_t n;

    if (flags < 0 || fcntl(0, F_SETFL, flags | O_NONBLOCK) < 0) return;
    while ((n = read(0, buf, sizeof(buf))) > 0) {
        fprintf(stderr, "child got %zd bytes: ", n);
        fwrite(buf, 1, (size_t)n, stderr);
        fputc('\n', stderr);
    }
    if (n < 0 && errno != EAGAIN && errno != EWOULDBLOCK)
        perror("child stdin");
    fcntl(0, F_SETFL, flags);
}

static void dense_draw_screen(void) {
    struct winsize ws;
    int rows, cols, r, c;

    if (ioctl(1, TIOCGWINSZ, &ws) != 0) return;
    rows = ws.ws_row;
    cols = ws.ws_col;
    if (rows < 1 || cols < 1) return;

    /* Full-grid repaint: header/footer bars with reverse video, a body of
     * dense varied text (the renderer pays per cell) and background-colour
     * spans, like a real TUI's cards. */
    printf("\033[?25l\033[H\033[2J");
    for (r = 0; r < rows; r++) {
        printf("\033[%d;1H", r + 1);
        for (c = 0; c < cols; c++) {
            if (r == 0 || r == rows - 1) {
                if (c == 0) printf("\033[7m");
                putchar(c % 2 == 0 ? ' ' : (r == 0 ? '-' : '='));
                if (c == cols - 1) printf("\033[27m");
            } else if ((c / 9) % 2 == 0) {
                printf("\033[48;5;%dm", (unsigned)((r * 7 + c / 9) % 200 + 16));
                putchar('a' + (r * 31 + c * 7) % 26);
            } else {
                printf("\033[49m");
                putchar('0' + (r + c) % 10);
            }
        }
        printf("\033[49m");
    }
    fflush(stdout);
    dense_send_image();
    printf("\033[%d;1H", rows);
    fflush(stdout);
}

static void dense_child_main(void) {
    struct sigaction act;
    time_t last = 0;

    memset(&act, 0, sizeof(act));
    act.sa_handler = dense_on_winch;
    sigaction(SIGWINCH, &act, NULL);

    for (;;) {
        time_t now = time(NULL);
        if (dense_resized) {
            dense_resized = 0;
            dense_draw_screen();
        } else if (now != last) {
            /* Light traffic like a clock row, so the pty is not idle. */
            char buf[64];
            struct tm tmv;
            last = now;
            localtime_r(&now, &tmv);
            strftime(buf, sizeof(buf), "\033[s%H:%M:%S\033[u", &tmv);
            printf("\033[%d;1H\033[7m %s \033[27m", 1, buf);
            fflush(stdout);
        }
        dump_stdin();
        usleep(100000);
    }
}

static PinwinLayout canned_layout(int side, int32_t cols) {
    PinwinLayout layout;
    layout.side = side;
    layout.cols = cols;
    layout.top = 0;
    layout.bottom = 0;
    layout.left = 0;
    layout.right = DEMO_GUTTER;
    return layout;
}

/* A canned child on the slave side: the tester's shell, told it is a colour
 * terminal (the library sets no child environment, design D6). With
 * DEMO_DENSE=1 the child is the dense stand-in instead. */
static int spawn_child(int* master_out) {
    pid_t pid;
    const char* sh;

    setenv("TERM", "xterm-256color", 1);
    setenv("COLORTERM", "truecolor", 1);
    pid = forkpty(master_out, NULL, NULL, NULL);
    if (pid < 0) return -1;
    if (pid == 0) {
        if (dense_mode) {
            /* Re-exec self in dense-child mode: no shell, no session. */
            execl("/proc/self/exe", "pinwin-demo", "--dense-child",
                  (char*)NULL);
            _exit(127);
        }
        sh = getenv("SHELL");
        if (sh == NULL || *sh == '\0') sh = "/bin/sh";
        execl(sh, sh, (char*)NULL);
        _exit(127);
    }
    return 0;
}

static void apply(int side) {
    PinwinLayout layout = canned_layout(side, demo_cols);
    int rc = pinwin_apply_layout(&layout);
    printf("pinwin_apply_layout(side=%s, cols=%d) = %d\n",
           side == PINWIN_SIDE_LEFT ? "left" : "right", (int)demo_cols, rc);
    fflush(stdout);
}

int main(int argc, char** argv) {
    int master = -1;
    int side = PINWIN_SIDE_LEFT;
    int rc;
    PinwinStartup startup;
    char line[64];

    dense_mode = getenv("DEMO_DENSE") != NULL;
    if (argc > 1 && strcmp(argv[1], "--dense-child") == 0) {
        dense_child_main();
        return 0;
    }
    if (argc > 1) {
        long parsed = strtol(argv[1], NULL, 10);
        if (parsed >= 1 && parsed <= 65535) demo_cols = (int32_t)parsed;
    }

    /* The child's exit is its own business; reap it silently. */
    signal(SIGCHLD, SIG_IGN);
    if (spawn_child(&master) != 0) {
        perror("forkpty");
        return 1;
    }

    startup.master_fd = master;
    startup.layout = canned_layout(side, demo_cols);
    startup.keyboard_mode = PINWIN_KEYBOARD_ON_DEMAND;
    startup.accent.enabled = 1;
    startup.accent.r = 0xda;
    startup.accent.g = 0xbc;
    startup.accent.b = 0x7f;
    startup.accent.width = 1;
    rc = pinwin_start(&startup);
    printf("pinwin_start = %d\n", rc);
    if (rc != PINWIN_OK) {
        fprintf(stderr, "pinwin-demo: pinwin_start failed (%d)\n", rc);
        return 1;
    }

    /* Let the canned layout dock, then re-dock live so the change is visible. */
    usleep(SETTLE_US);
    side = PINWIN_SIDE_RIGHT;
    apply(side);

    printf("commands, typed in THIS terminal (not in the panel):\n"
           "          <enter> toggle side/width, 'b' rejected layout, 'q' quit;\n"
           "          run `exit` in the panel to see a pty hangup survive.\n");
    fflush(stdout);

    while (fgets(line, sizeof(line), stdin) != NULL) {
        if (line[0] == 'q') break;
        if (line[0] == 'b') {
            PinwinLayout bad = canned_layout(side, 0); /* cols 0 is invalid */
            rc = pinwin_apply_layout(&bad);
            printf("pinwin_apply_layout(invalid) = %d (want %d)\n", rc,
                   PINWIN_ERR_INVALID);
            fflush(stdout);
            continue;
        }
        if (line[0] == 'e' || line[0] == 'p') {
            PinwinLayout l;
            demo_cols = demo_cols == DEMO_COLS ? 120 : DEMO_COLS;
            l = canned_layout(side, demo_cols);
            rc = line[0] == 'e'
                     ? pinwin_apply_layout_animated(&l, PINWIN_ANIM_DEFAULT_MS)
                     : pinwin_apply_layout(&l);
            printf("%s(cols=%d) = %d\n", line[0] == 'e' ? "animated" : "plain",
                   (int)demo_cols, rc);
            fflush(stdout);
            continue;
        }
        side = side == PINWIN_SIDE_LEFT ? PINWIN_SIDE_RIGHT : PINWIN_SIDE_LEFT;
        demo_cols = demo_cols == DEMO_COLS ? DEMO_COLS + 8 : DEMO_COLS;
        apply(side);
    }

    pinwin_stop();
    printf("pinwin_stop done\n");
    return 0;
}
