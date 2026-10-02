/*
 * fontconfig.c - where pinwin reads the user's terminal preferences: the
 * Ghostty config (a plain "key = value" file), the theme colours behind it
 * and the terminfo check the spawned command depends on (design D4).
 *
 * Only font_config_load, theme_colours and terminfo_exists are shared; the
 * rest of the former glue.c lives in the other glue files, with shared state
 * in glue_internal.h.
 */

#include "glue_internal.h"

#include <glib.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int terminfo_exists(const char* name) {
    static const char* dirs[] = {"/usr/share/terminfo", "/etc/terminfo",
                                 "/usr/lib/terminfo", "/lib/terminfo"};
    const char* single = getenv("TERMINFO");
    const char* list = getenv("TERMINFO_DIRS");
    char path[512];
    size_t i;

    if (single && *single) {
        snprintf(path, sizeof(path), "%s/%c/%s", single, name[0], name);
        if (access(path, R_OK) == 0) return 1;
    }
    if (list && *list) {
        const char* p = list;
        while (*p) {
            const char* colon = strchr(p, ':');
            size_t len = colon ? (size_t)(colon - p) : strlen(p);
            if (len == 0) {
                snprintf(path, sizeof(path), "/usr/share/terminfo/%c/%s", name[0], name);
            } else if (len < sizeof(path)) {
                snprintf(path, sizeof(path), "%.*s/%c/%s", (int)len, p, name[0], name);
            } else {
                path[0] = '\0';
            }
            if (path[0] && access(path, R_OK) == 0) return 1;
            if (!colon) break;
            p = colon + 1;
        }
    }
    for (i = 0; i < sizeof(dirs) / sizeof(dirs[0]); i++) {
        snprintf(path, sizeof(path), "%s/%c/%s", dirs[i], name[0], name);
        if (access(path, R_OK) == 0) return 1;
    }
    return 0;
}

/* The panel follows the Ghostty config (a plain "key = value" file) so that it
 * uses the font the user actually configured for their terminal, with
 * PINWIN_FONT / PINWIN_FONT_SIZE as overrides and "monospace 11" as the
 * fallback. Only the first font-family is used, matching Ghostty's rule that
 * the first family with a glyph wins. */
/* The terminal's default background and foreground: the panel paints the whole
 * widget with the background (the VT's own default is black, which shows up as
 * a black strip under the last row) and uses the foreground for cells that
 * don't set one. Read from the Ghostty config, following a `theme` file the
 * same way Ghostty does. */
static void theme_colour_parse(const char* line, uint8_t* bg, uint8_t* fg) {
    char key[32];
    char value[32];
    unsigned r, g, b;

    if (sscanf(line, " %31[a-z-] = %31s", key, value) != 2) return;
    if (value[0] != '#') return;
    if (sscanf(value + 1, "%2x%2x%2x", &r, &g, &b) != 3) return;
    if (strcmp(key, "background") == 0) {
        bg[0] = (uint8_t)r; bg[1] = (uint8_t)g; bg[2] = (uint8_t)b;
    } else if (strcmp(key, "foreground") == 0) {
        fg[0] = (uint8_t)r; fg[1] = (uint8_t)g; fg[2] = (uint8_t)b;
    }
}

void theme_colours(char* theme_name, size_t theme_name_len,
                   uint8_t* bg, uint8_t* fg) {
    char* path;
    char* data = NULL;
    char* lines;
    char* line;
    gboolean found_theme = FALSE;

    theme_name[0] = '\0';
    path = g_build_filename(g_get_user_config_dir(), "ghostty", "config", NULL);
    if (g_file_get_contents(path, &data, NULL, NULL)) {
        lines = data;
        while ((line = strsep(&lines, "\n")) != NULL) {
            char key[32];
            char value[256];
            if (sscanf(line, " %31[a-z-] = %255[^#]", key, value) != 2) continue;
            g_strstrip(value);
            if (value[0] == '"') {
                char* end = strrchr(value + 1, '"');
                if (end) *end = '\0';
                memmove(value, value + 1, strlen(value));
            }
            if (strcmp(key, "theme") == 0) {
                snprintf(theme_name, theme_name_len, "%s", value);
                found_theme = TRUE;
            } else if (strcmp(key, "background") == 0 || strcmp(key, "foreground") == 0) {
                theme_colour_parse(line, bg, fg);
            }
        }
        g_free(data);
    }
    g_free(path);

    if (!found_theme) return;
    /* Theme files live next to the config or in Ghostty's install directory. */
    path = g_build_filename(g_get_user_config_dir(), "ghostty", "themes", theme_name, NULL);
    if (!g_file_get_contents(path, &data, NULL, NULL)) {
        g_free(path);
        path = g_build_filename("/usr/share/ghostty/themes", theme_name, NULL);
        if (!g_file_get_contents(path, &data, NULL, NULL)) {
            g_free(path);
            return;
        }
    }
    g_free(path);
    lines = data;
    while ((line = strsep(&lines, "\n")) != NULL) theme_colour_parse(line, bg, fg);
    g_free(data);
}

void font_config_load(char** family, double* size) {
    char* path;
    char* data = NULL;
    const char* env;

    *family = NULL;
    *size = 11.0;

    path = g_build_filename(g_get_user_config_dir(), "ghostty", "config", NULL);
    if (g_file_get_contents(path, &data, NULL, NULL)) {
        char** lines = g_strsplit(data, "\n", -1);
        size_t i;
        for (i = 0; lines[i]; i++) {
            char* line = g_strdup(lines[i]);
            char* eq;
            char* key;
            char* value;

            /* Ghostty allows whole-line comments; nothing else uses '#'. */
            char* hash = strchr(line, '#');
            if (hash) *hash = '\0';
            eq = strchr(line, '=');
            if (!eq) {
                g_free(line);
                continue;
            }
            *eq = '\0';
            key = g_strstrip(line);
            value = g_strstrip(eq + 1);
            if (value[0] == '"') {
                char* end = strrchr(value + 1, '"');
                if (end) *end = '\0';
                value++;
            }
            if (strcmp(key, "font-family") == 0 && *value && !*family) {
                *family = g_strdup(value);
            } else if (strcmp(key, "font-size") == 0) {
                double parsed = g_ascii_strtod(value, NULL);
                if (parsed > 0) *size = parsed;
            }
            g_free(line);
        }
        g_strfreev(lines);
        g_free(data);
    }
    g_free(path);

    env = getenv("PINWIN_FONT");
    if (env && *env) {
        g_free(*family);
        *family = g_strdup(env);
    }
    env = getenv("PINWIN_FONT_SIZE");
    if (env && *env) {
        double parsed = g_ascii_strtod(env, NULL);
        if (parsed > 0) *size = parsed;
    }
}
