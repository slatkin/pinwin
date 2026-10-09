#!/usr/bin/env bash

set -u

readonly CHECKER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check-code-file-lines.sh"
readonly TEST_ROOT="$(mktemp -d)"

cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

fail() {
    printf 'check-code-file-lines-test: %s\n' "$1" >&2
    exit 1
}

write_lines() {
    local path=$1
    local count=$2
    local i

    : > "$path" || return 1
    for ((i = 1; i <= count; i++)); do
        printf 'line %d\n' "$i" >> "$path" || return 1
    done
}

init_repo() {
    local repo=$1

    mkdir -p "$repo"
    git -C "$repo" init -q || return 1
    git -C "$repo" config user.email test@example.invalid || return 1
    git -C "$repo" config user.name checker-test || return 1
}

run_checker() {
    local repo=$1

    (cd "$repo" && "$CHECKER") 2>&1
}

repo="$TEST_ROOT/classifier"
init_repo "$repo" || fail 'unable to initialize classifier repository'
mkdir -p "$repo"/{src,scripts,docs,.github,openspec,dist,assets,fonts,contrib,.githooks,target,build,generated}
write_lines "$repo/src/large.rs" 801 || fail 'unable to create source fixture'
write_lines "$repo/scripts/other.py" 802 || fail 'unable to create second source fixture'
for extension in lua sh js ts tsx c h cpp hpp zig; do
    write_lines "$repo/src/large.$extension" 801 || fail "unable to create .$extension fixture"
done
write_lines "$repo/src/nerd_font.rs" 1200 || fail 'unable to create generated-table fixture'
write_lines "$repo/Makefile" 801 || fail 'unable to create Makefile fixture'
write_lines "$repo/PKGBUILD" 801 || fail 'unable to create PKGBUILD fixture'
write_lines "$repo/PKGBUILD-git" 801 || fail 'unable to create PKGBUILD-git fixture'
write_lines "$repo/.githooks/check" 801 || fail 'unable to create .githooks fixture'
write_lines "$repo/src/space name.rs" 801 || fail 'unable to create whitespace fixture'
newline_path="$repo/src/newline"$'\n'"name.rs"
write_lines "$newline_path" 801 || fail 'unable to create newline fixture'
write_lines "$repo/docs/large.sh" 1200 || fail 'unable to create docs fixture'
write_lines "$repo/.github/large.sh" 1200 || fail 'unable to create GitHub fixture'
write_lines "$repo/openspec/large.lua" 1200 || fail 'unable to create OpenSpec fixture'
write_lines "$repo/dist/large.rs" 1200 || fail 'unable to create dist fixture'
write_lines "$repo/assets/large.js" 1200 || fail 'unable to create assets fixture'
write_lines "$repo/fonts/large.ts" 1200 || fail 'unable to create fonts fixture'
write_lines "$repo/contrib/large.service" 1200 || fail 'unable to create service fixture'
write_lines "$repo/target/large.rs" 1200 || fail 'unable to create target fixture'
write_lines "$repo/build/large.rs" 1200 || fail 'unable to create build fixture'
write_lines "$repo/generated/large.rs" 1200 || fail 'unable to create generated fixture'
write_lines "$repo/config.toml" 1200 || fail 'unable to create TOML fixture'
write_lines "$repo/config.json" 1200 || fail 'unable to create JSON fixture'
write_lines "$repo/config.yaml" 1200 || fail 'unable to create YAML fixture'
write_lines "$repo/Cargo.lock" 1200 || fail 'unable to create lockfile fixture'
write_lines "$repo/unit.service" 1200 || fail 'unable to create service extension fixture'
write_lines "$repo/image.png" 1200 || fail 'unable to create binary fixture'
git -C "$repo" add -- . || fail 'unable to stage classifier fixtures'

if classifier_output=$(run_checker "$repo"); then
    fail 'classifier accepted ordinary size violations'
fi
[[ "$classifier_output" == *'src/large.rs has 801 lines'* ]] ||
    fail 'classifier missed the first ordinary violation'
[[ "$classifier_output" == *'scripts/other.py has 802 lines'* ]] ||
    fail 'classifier missed the second ordinary violation'
for governed in src/large.lua src/large.sh src/large.js src/large.ts src/large.tsx \
    src/large.c src/large.h src/large.cpp src/large.hpp src/large.zig Makefile PKGBUILD PKGBUILD-git \
    .githooks/check; do
    [[ "$classifier_output" == *"$governed has 801 lines"* ]] ||
        fail "classifier missed $governed"
done
[[ "$classifier_output" == *'src/space name.rs has 801 lines'* ]] ||
    fail 'classifier did not handle whitespace in a governed filename'
[[ "$classifier_output" == *$'src/newline\nname.rs has 801 lines'* ]] ||
    fail 'classifier did not handle a newline in a governed filename'
for excluded in src/nerd_font.rs docs/large.sh .github/large.sh openspec/large.lua dist/large.rs \
    assets/large.js fonts/large.ts contrib/large.service target/large.rs \
    build/large.rs generated/large.rs config.toml config.json config.yaml \
    Cargo.lock unit.service image.png; do
    [[ "$classifier_output" != *"$excluded"* ]] || fail "classifier included $excluded"
done

repo="$TEST_ROOT/read-error"
init_repo "$repo" || fail 'unable to initialize read-error repository'
mkdir -p "$repo/src"
write_lines "$repo/src/large.rs" 801 || fail 'unable to create read-error violation'
write_lines "$repo/src/unreadable.rs" 1 || fail 'unable to create unreadable-file fixture'
git -C "$repo" add . || fail 'unable to stage read-error fixtures'
chmod 000 "$repo/src/unreadable.rs" || fail 'unable to make the read-error fixture unreadable'

if read_error_output=$(run_checker "$repo"); then
    fail 'checker passed after a tracked file became unreadable'
fi
[[ "$read_error_output" == *'src/large.rs has 801 lines'* ]] ||
    fail 'checker stopped before reporting an ordinary violation'
[[ "$read_error_output" == *'unable to count src/unreadable.rs'* ]] ||
    fail 'checker did not report the read error'

if outside_repo_output=$(run_checker "$TEST_ROOT"); then
    fail 'checker passed outside a Git repository'
fi
[[ "$outside_repo_output" == *'git ls-files failed'* ]] ||
    fail 'checker did not report the Git enumeration error'

printf 'check-code-file-lines-test: passed\n'
