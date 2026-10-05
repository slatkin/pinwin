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
//! `PINWIN_GHOSTTY_SRC=<dir>` uses an existing git checkout at the pinned
//! commit instead of fetching, for offline or repeated development builds. Its
//! `HEAD` is checked against the pin exactly like the fetch path; a wrong
//! checkout would link against the wrong ABI and only fail at runtime. The
//! chosen source (path and commit) is recorded beside the archive so that
//! switching the override rebuilds it.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The one ghostty commit pinwin's FFI targets (D2).
const GHOSTTY_COMMIT: &str = "3a3047f6b62a791fd8b12d9f07a85b3d2160370b";

/// Zig optimise mode for libghostty-vt. ghostty's build defaults to `Debug`, which makes kitty
/// graphics (image decode and storage) slow; `ReleaseSafe` matches the pre-Rust build.
const GHOSTTY_OPTIMIZE: &str = "ReleaseSafe";

/// Explicit CPU model for libghostty-vt. Without `-Dcpu`, zig targets the
/// build host's native CPU, and ghostty's bundled vectorized memset
/// (`src/quirks_memset.zig`, which overrides `compiler_rt` for everything in
/// the archive, including mimalloc) picks the host's vector width with no
/// runtime guard: a CI runner with `AVX-512` shipped binaries that `SIGILL` at
/// the first allocation on every CPU without `AVX-512`. `x86_64_v2`
/// (`SSE4.2`/`POPCNT`, ~2009+) is the distributed floor.
const GHOSTTY_CPU: &str = "x86_64_v2";
const GHOSTTY_REPO: &str = "https://github.com/ghostty-org/ghostty.git";

fn main() {
    // The archive is cached under OUT_DIR, so only rerun this script when it
    // or the source override changes, not on every source edit.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PINWIN_GHOSTTY_SRC");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let prefix = out_dir.join("ghostty");
    let archive = prefix.join("lib").join("libghostty-vt.a");

    // The archive lives in OUT_DIR and survives a build-script rerun, so
    // `!archive.is_file()` alone would keep a stale archive after
    // PINWIN_GHOSTTY_SRC changes. Record the source path and pin beside the
    // archive and rebuild whenever that identity changes.
    let override_dir = env::var_os("PINWIN_GHOSTTY_SRC").map(PathBuf::from);
    let source_dir = override_dir
        .clone()
        .unwrap_or_else(|| out_dir.join("ghostty-src"));
    let stamp = prefix.join("source-stamp");
    let identity = format!(
        "{}\n{}\n{}\n{}\n",
        source_dir.display(),
        GHOSTTY_COMMIT,
        GHOSTTY_OPTIMIZE,
        GHOSTTY_CPU
    );

    let stale = !archive.is_file()
        || fs::read_to_string(&stamp)
            .map_or(true, |recorded| recorded != identity);

    if stale {
        let source = match &override_dir {
            Some(dir) => dir.clone(),
            None => fetch_pinned_source(&out_dir),
        };
        assert_pinned(&source);
        build_ghostty(&source, &prefix, &out_dir);
        fs::write(&stamp, &identity).expect("record the ghostty source identity");
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

    source
}

/// Assert `source` is a git checkout at the pinned commit (D2). Both the fetch
/// path and the `PINWIN_GHOSTTY_SRC` override must be pinned: the FFI
/// declarations target that commit's ABI, and a mismatch otherwise fails only at
/// runtime.
fn assert_pinned(source: &Path) {
    let output = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to run git rev-parse in {}: {error}",
                source.display()
            )
        });
    let actual = String::from_utf8_lossy(&output.stdout);
    let actual = actual.trim();
    assert!(
        output.status.success() && actual == GHOSTTY_COMMIT,
        "{} is not at the pinned ghostty commit (D2): expected {GHOSTTY_COMMIT}, got {}",
        source.display(),
        if actual.is_empty() {
            String::from_utf8_lossy(&output.stderr).trim().to_owned()
        } else {
            actual.to_owned()
        }
    );
}

/// Build `libghostty-vt.a` from `source` into `prefix` with ghostty's build.
fn build_ghostty(source: &Path, prefix: &Path, out_dir: &Path) {
    assert!(
        source.join("build.zig").is_file(),
        "PINWIN_GHOSTTY_SRC {} is not a ghostty checkout",
        source.display()
    );

    let status = Command::new("zig")
        .args(["build", "-Demit-lib-vt"])
        .arg(format!("-Doptimize={GHOSTTY_OPTIMIZE}"))
        .arg(format!("-Dcpu={GHOSTTY_CPU}"))
        .arg("--prefix")
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
