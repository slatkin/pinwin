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
 *   b        apply an invalid layout: expect PINWIN_ERR_INVALID, host lives
 *   q        stop the panel and exit
 * Run `exit` inside the panel to watch a pty hangup leave the host alone.
 */
#include <pty.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "pinwin_api.h"

#define DEMO_COLS 40
#define DEMO_GUTTER 12
#define SETTLE_US 1500000

static int32_t demo_cols = DEMO_COLS;

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
 * terminal (the library sets no child environment, design D6). */
static int spawn_child(int* master_out) {
    pid_t pid;
    const char* sh;

    setenv("TERM", "xterm-256color", 1);
    setenv("COLORTERM", "truecolor", 1);
    pid = forkpty(master_out, NULL, NULL, NULL);
    if (pid < 0) return -1;
    if (pid == 0) {
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

    printf("commands: <enter> toggle side/width, 'b' rejected layout, 'q' quit;\n"
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
        side = side == PINWIN_SIDE_LEFT ? PINWIN_SIDE_RIGHT : PINWIN_SIDE_LEFT;
        demo_cols = demo_cols == DEMO_COLS ? DEMO_COLS + 8 : DEMO_COLS;
        apply(side);
    }

    pinwin_stop();
    printf("pinwin_stop done\n");
    return 0;
}
