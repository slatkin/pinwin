//! Build the pinned libghostty-vt (port-to-rust D2).
//!
//! pinwin uses its own FFI declarations against one pinned ghostty commit, so
//! this script obtains that commit's source, builds ghostty's own static VT
//! library with `zig build -Demit-lib-vt`, and points rustc at the archive. It
//! deliberately does not generate a Zig build project of its own: ghostty's
//! build system owns the build, and pinwin contains no Zig code.
//!
//! Build requirements (also recorded in D2 and the README):
//!   * `zig` 0.16.0 on PATH, the pinned commit's minimum Zig version.
//!   * `git` to fetch the pinned commit (content-addressed by its SHA).
//!   * network access on a cold cache: the `git fetch` above, and ghostty's
//!     own Zig dependencies, which `zig build` downloads.
//!
//! `PINWIN_GHOSTTY_SRC=<dir>` uses an existing ghostty checkout at the pinned
//! commit instead of fetching, for offline or repeated development builds.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one ghostty commit pinwin's FFI targets (D2).
const GHOSTTY_COMMIT: &str = "3a3047f6b62a791fd8b12d9f07a85b3d2160370b";
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";

fn main() {
    // The archive is cached under OUT_DIR, so only rerun this script when it
    // or the source override changes, not on every source edit.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PINWIN_GHOSTTY_SRC");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let prefix = out_dir.join("ghostty");
    let archive = prefix.join("lib").join("libghostty-vt.a");

    if !archive.is_file() {
        let source = match env::var_os("PINWIN_GHOSTTY_SRC") {
            Some(dir) => PathBuf::from(dir),
            None => fetch_pinned_source(&out_dir),
        };
        build_ghostty(&source, &prefix, &out_dir);
    }

    assert!(
        archive.is_file(),
        "libghostty-vt.a was not built at {}",
        archive.display()
    );

    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    // `static=` forces the archive even though ghostty's build also emits a
    // shared object next to it.
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    // The archive's vendored SIMD code and Zig runtime use the C math library.
    println!("cargo:rustc-link-lib=m");
}

/// Fetch the pinned commit into `OUT_DIR` and return the checkout directory.
fn fetch_pinned_source(out_dir: &Path) -> PathBuf {
    let source = out_dir.join("ghostty-src");

    if !source.join("build.zig").is_file() {
        if !source.join(".git").is_dir() {
            fs::create_dir_all(&source).expect("create ghostty source dir");
            git(&source, &["init", "-q"]);
            git(&source, &["remote", "add", "origin", GHOSTTY_REPO]);
        }
        git(
            &source,
            &["fetch", "-q", "--depth", "1", "origin", GHOSTTY_COMMIT],
        );
        git(&source, &["checkout", "-q", "FETCH_HEAD"]);
    }

    let head = git(&source, &["rev-parse", "HEAD"]);
    assert_eq!(
        head.trim(),
        GHOSTTY_COMMIT,
        "ghostty checkout is not at the pinned commit (D2)"
    );
    source
}

/// Build `libghostty-vt.a` from `source` into `prefix` with ghostty's build.
fn build_ghostty(source: &Path, prefix: &Path, out_dir: &Path) {
    assert!(
        source.join("build.zig").is_file(),
        "PINWIN_GHOSTTY_SRC {} is not a ghostty checkout",
        source.display()
    );

    let status = Command::new("zig")
        .args(["build", "-Demit-lib-vt", "--prefix"])
        .arg(prefix)
        .arg("--cache-dir")
        .arg(out_dir.join("zig-cache"))
        .current_dir(source)
        .status()
        .unwrap_or_else(|error| {
            panic!("failed to run zig (Zig 0.16.0 is required to build ghostty, D2): {error}")
        });
    assert!(status.success(), "zig build of the pinned ghostty failed");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
