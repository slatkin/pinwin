#!/usr/bin/env bash

set -u

readonly MAX_LINES=800
violations=0
errors=0

is_governed_path() {
    local path=$1

    case "$path" in
        # The generated glyph-constraint table; its regeneration is described
        # in its module comment and it is a documented exception to the
        # 800-line limit.
        src/nerd_font.rs)
            return 1
            ;;
        .githooks/*)
            return 0
            ;;
        docs/*|.github/*|openspec/*|dist/*|assets/*|fonts/*|contrib/*.service|target/*|build/*|generated/*)
            return 1
            ;;
        *.toml|*.json|*.yaml|*.yml|*.lock|*.service|*.png|*.jpg|*.jpeg|*.gif|*.svg|*.ico|*.ttf|*.otf|*.woff|*.woff2|*.pdf|*.zip|*.gz|*.tar)
            return 1
            ;;
        Makefile|PKGBUILD|PKGBUILD-git|*.rs|*.lua|*.sh|*.py|*.js|*.ts|*.tsx|*.c|*.h|*.cpp|*.hpp|*.zig)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

file_list=$(mktemp) || {
    printf 'code-file-lines: unable to create a temporary file for git ls-files\n' >&2
    exit 1
}
trap 'rm -f "$file_list"' EXIT

if ! git ls-files -z >"$file_list"; then
    printf 'code-file-lines: git ls-files failed; refusing to pass\n' >&2
    exit 1
fi

while IFS= read -r -d '' path; do
    is_governed_path "$path" || continue

    # A tracked deletion is still listed by git until the change is staged.
    # It has no source contents to count; existing files remain governed.
    [[ -e "$path" ]] || continue

    if ! lines=$(wc -l < "$path"); then
        printf 'code-file-lines: unable to count %s; refusing to pass\n' "$path" >&2
        errors=1
        continue
    fi
    if (( lines > MAX_LINES )); then
        printf 'code-file-lines: %s has %d lines (maximum %d)\n' \
            "$path" "$lines" "$MAX_LINES"
        violations=1
    fi
done < "$file_list"

if (( violations != 0 || errors != 0 )); then
    exit 1
fi

printf 'code-file-lines: all governed tracked files are at or below %d lines\n' "$MAX_LINES"
