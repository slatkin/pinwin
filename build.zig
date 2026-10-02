const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const lib_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    // GTK4, gtk4-layer-shell and Pango come from the system. src/glue.c and
    // its siblings (src/glue_internal.h is their shared state, design D4) are
    // the only files that include their headers; the @cImport in src/main.zig
    // only sees ghostty/vt.h and pinwin.h.
    lib_mod.linkSystemLibrary("gtk4", .{});
    lib_mod.linkSystemLibrary("gtk4-layer-shell-0", .{});
    lib_mod.linkSystemLibrary("pangocairo", .{});

    // Zig static-library artifacts do not merge linked static archives, so
    // libpinwin.a does NOT bundle libghostty-vt (design D7): the pinned
    // ghostty archive is installed next to it under the name consumers link.
    // As a dependency ghostty installs the plain vt archive, not the fat one
    // its standalone build makes, so its vendored SIMD archives are installed
    // alongside too: a plain-cc consumer needs all four on the link line.
    const ghostty = b.lazyDependency("ghostty", .{});
    if (ghostty) |dep| {
        const ghostty_lib = dep.artifact("ghostty-vt-static");
        lib_mod.linkLibrary(ghostty_lib);
        lib_mod.addIncludePath(dep.path("include"));
        b.getInstallStep().dependOn(&b.addInstallLibFile(
            ghostty_lib.getEmittedBin(),
            "libghostty-vt.a",
        ).step);
        if (dep.builder.lazyDependency("simdutf", .{ .target = target, .optimize = optimize, .no_libcxx = true })) |simdutf| {
            b.getInstallStep().dependOn(&b.addInstallLibFile(
                simdutf.artifact("simdutf").getEmittedBin(),
                "libsimdutf.a",
            ).step);
        }
        if (dep.builder.lazyDependency("highway", .{ .target = target, .optimize = optimize })) |highway| {
            b.getInstallStep().dependOn(&b.addInstallLibFile(
                highway.artifact("highway").getEmittedBin(),
                "libhighway.a",
            ).step);
        }
    }
    lib_mod.addIncludePath(b.path("src"));
    lib_mod.addCSourceFiles(.{
        .files = &.{ "src/glue.c", "src/render.c", "src/images.c", "src/pty.c", "src/input.c", "src/fontconfig.c", "src/options.c", "src/pinwin_api.c" },
        .flags = &.{ "-std=gnu11", "-Wall" },
    });

    const lib = b.addLibrary(.{
        .name = "pinwin",
        .linkage = .static,
        .root_module = lib_mod,
    });
    b.installArtifact(lib);

    // The `pinwin` program: a thin host over the C ABI (host/main.c), installed
    // by the default `zig build` as bin/pinwin. The Compile step is named
    // "pinwin-host" (not "pinwin") so a dependent's artifact("pinwin") resolves
    // unambiguously to the static library.
    const host = addHost(b, lib, ghostty, target, optimize, "pinwin-host", "host/main.c");
    b.getInstallStep().dependOn(&b.addInstallFile(host.getEmittedBin(), "bin/pinwin").step);

    // Dev-only demo (design OQ-a): drives the C ABI over a pty pair it creates
    // itself, built by `zig build demo` only and never installed.
    const demo_step = b.step("demo", "Build the dev-only pinwin demo executable");
    const demo = addHost(b, lib, ghostty, target, optimize, "pinwin-demo", "demo/main.c");
    demo_step.dependOn(&b.addInstallArtifact(demo, .{}).step);

    // Layout-core unit tests (design D8). The test root links the GTK-free
    // src/options.c directly and runs as `zig build check`.
    const check_step = b.step("check", "Run the layout-core Zig unit tests");
    const check_mod = b.createModule(.{
        .root_source_file = b.path("src/options_test.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    check_mod.addIncludePath(b.path("src"));
    check_mod.addCSourceFiles(.{
        .files = &.{"src/options.c"},
        .flags = &.{ "-std=gnu11", "-Wall" },
    });
    const check_tests = b.addTest(.{ .root_module = check_mod });
    check_step.dependOn(&b.addRunArtifact(check_tests).step);

    // ABI contract test (design D8): drives pinwin_apply_layout through the
    // real library but never pinwin_start, so no GTK thread is started and no
    // display is needed. The link shape is the demo's (design OQ-a).
    const api_check_mod = b.createModule(.{
        .root_source_file = b.path("src/pinwin_api_test.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    api_check_mod.addIncludePath(b.path("src"));
    api_check_mod.linkLibrary(lib);
    api_check_mod.linkSystemLibrary("gtk4", .{});
    api_check_mod.linkSystemLibrary("gtk4-layer-shell-0", .{});
    api_check_mod.linkSystemLibrary("pangocairo", .{});
    if (ghostty) |dep| {
        api_check_mod.linkLibrary(dep.artifact("ghostty-vt-static"));
        api_check_mod.addIncludePath(dep.path("include"));
    }
    const api_check_tests = b.addTest(.{ .root_module = api_check_mod });
    check_step.dependOn(&b.addRunArtifact(api_check_tests).step);
}

// A program that links the panel and may fork: forkpty(3) lives in libutil.
fn addHost(
    b: *std.Build,
    lib: *std.Build.Step.Compile,
    ghostty: ?*std.Build.Dependency,
    target: std.Build.ResolvedTarget,
    optimize: std.builtin.OptimizeMode,
    name: []const u8,
    source: []const u8,
) *std.Build.Step.Compile {
    const mod = b.createModule(.{ .target = target, .optimize = optimize });
    mod.addIncludePath(b.path("src"));
    mod.addCSourceFiles(.{ .files = &.{source}, .flags = &.{ "-std=gnu11", "-Wall" } });
    mod.linkLibrary(lib);
    mod.linkSystemLibrary("gtk4", .{});
    mod.linkSystemLibrary("gtk4-layer-shell-0", .{});
    mod.linkSystemLibrary("pangocairo", .{});
    mod.linkSystemLibrary("util", .{});
    if (ghostty) |dep| {
        mod.linkLibrary(dep.artifact("ghostty-vt-static"));
        mod.addIncludePath(dep.path("include"));
    }
    return b.addExecutable(.{ .name = name, .root_module = mod });
}
