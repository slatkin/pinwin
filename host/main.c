/*
 * host/main.c - the `pinwin` program: runs a command (default $SHELL) in a
 * layer-shell panel docked to the left edge, until the command exits.
 *
 *   pinwin [--] [command...]
 *   COLS=60 GUTTER=8 PINWIN_KEYBOARD=on-demand|exclusive|none
 *   PINWIN_ACCENT=on|off PINWIN_ACCENT_COLOR=#RRGGBB PINWIN_ACCENT_WIDTH=2
 *   pinwin htop
 *
 * A thin host over libpinwin's C ABI (pinwin_api.h): it owns the pty, the
 * child's environment and the process lifetime; the library owns the panel.
 */
#include <errno.h>
#include <pty.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#include "pinwin_api.h"

static pid_t child;

static void on_term(int sig) {
    (void)sig;
    if (child > 0) kill(child, SIGHUP); /* waitpid in main then returns */
}

/* COLS / GUTTER: unset or empty is the default; anything else must be a whole
 * number in range or the program exits 2 before any surface opens. */
static long env_number(const char* name, long fallback, long min, long max) {
    const char* raw = getenv(name);
    char* end;
    long value;

    if (raw == NULL || *raw == '\0') return fallback;
    value = strtol(raw, &end, 10);
    if (*end != '\0' || value < min || value > max) {
        fprintf(stderr, "pinwin: %s: expected a number from %ld to %ld, got '%s'\n",
                name, min, max, raw);
        exit(2);
    }
    return value;
}

static int32_t keyboard_mode(void) {
    const char* raw = getenv("PINWIN_KEYBOARD");

    if (raw == NULL || *raw == '\0' || strcmp(raw, "on-demand") == 0)
        return PINWIN_KEYBOARD_ON_DEMAND;
    if (strcmp(raw, "exclusive") == 0) return PINWIN_KEYBOARD_EXCLUSIVE;
    if (strcmp(raw, "none") == 0) return PINWIN_KEYBOARD_NONE;
    fprintf(stderr, "pinwin: PINWIN_KEYBOARD: expected on-demand, exclusive or none, got '%s'\n", raw);
    exit(2);
}

static int accent_enabled(void) {
    const char* raw = getenv("PINWIN_ACCENT");

    if (raw == NULL || *raw == '\0' || strcmp(raw, "on") == 0) return 1;
    if (strcmp(raw, "off") == 0) return 0;
    fprintf(stderr, "pinwin: PINWIN_ACCENT: expected on or off, got '%s'\n", raw);
    exit(2);
}

/* PINWIN_ACCENT_COLOR: #RRGGBB or RRGGBB; unset is the default, which matches
 * the niri focus ring so the panel's highlight reads like the tiling one. */
static void accent_color(uint8_t rgb[3]) {
    const char* raw = getenv("PINWIN_ACCENT_COLOR");
    const char* hex = raw;
    char* end;
    long value;

    if (raw == NULL || *raw == '\0') {
        rgb[0] = 0xda;
        rgb[1] = 0xbc;
        rgb[2] = 0x7f;
        return;
    }
    if (*hex == '#') hex++;
    value = strtol(hex, &end, 16);
    if (*end != '\0' || end - hex != 6 || value < 0) {
        fprintf(stderr, "pinwin: PINWIN_ACCENT_COLOR: expected #RRGGBB, got '%s'\n", raw);
        exit(2);
    }
    rgb[0] = (uint8_t)(value >> 16);
    rgb[1] = (uint8_t)(value >> 8);
    rgb[2] = (uint8_t)value;
}

int main(int argc, char** argv) {
    PinwinStartup startup;
    uint8_t accent_rgb[3];
    char* shell[2];
    char** command = argv + 1;
    int master = -1;
    int status = 0;
    int rc;

    if (argc > 1 && strcmp(argv[1], "--") == 0) command++;
    else if (argc > 1 && strncmp(argv[1], "--", 2) == 0) {
        fprintf(stderr, "pinwin: unknown option '%s'\n", argv[1]);
        return 2;
    }
    if (*command == NULL) {
        const char* sh = getenv("SHELL");
        shell[0] = (char*)((sh && *sh) ? sh : "/bin/sh");
        shell[1] = NULL;
        command = shell;
    }

    startup.layout.side = PINWIN_SIDE_LEFT;
    startup.layout.cols = (int32_t)env_number("COLS", 40, 1, 65535);
    startup.layout.top = startup.layout.bottom = startup.layout.left = 0;
    startup.layout.right = (int32_t)env_number("GUTTER", 0, 0, 65535);
    startup.keyboard_mode = keyboard_mode();
    startup.accent.enabled = accent_enabled();
    if (startup.accent.enabled) {
        accent_color(accent_rgb);
        startup.accent.r = accent_rgb[0];
        startup.accent.g = accent_rgb[1];
        startup.accent.b = accent_rgb[2];
        startup.accent.width =
            (int32_t)env_number("PINWIN_ACCENT_WIDTH", 1, 1, 65535);
    }

    /* The library sets no child environment: say we are a colour terminal. */
    setenv("TERM", "xterm-256color", 1);
    setenv("COLORTERM", "truecolor", 1);
    child = forkpty(&master, NULL, NULL, NULL);
    if (child < 0) {
        perror("pinwin: forkpty");
        return 1;
    }
    if (child == 0) {
        execvp(command[0], command);
        fprintf(stderr, "pinwin: %s: %s\n", command[0], strerror(errno));
        _exit(127);
    }

    signal(SIGINT, on_term);
    signal(SIGTERM, on_term);
    startup.master_fd = master;
    rc = pinwin_start(&startup);
    if (rc != PINWIN_OK) {
        fprintf(stderr, "pinwin: cannot start the panel (%s)\n",
                rc == PINWIN_ERR_NO_DISPLAY ? "no Wayland display or wlr-layer-shell" : "error");
        kill(child, SIGHUP);
        waitpid(child, NULL, 0);
        return 1;
    }

    while (waitpid(child, &status, 0) < 0 && errno == EINTR) {}
    pinwin_stop();
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
