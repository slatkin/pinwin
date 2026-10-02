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
    const ghostty = b.lazyDependency("ghostty", .{});
    if (ghostty) |dep| {
        const ghostty_lib = dep.artifact("ghostty-vt-static");
        lib_mod.linkLibrary(ghostty_lib);
        lib_mod.addIncludePath(dep.path("include"));
        b.getInstallStep().dependOn(&b.addInstallLibFile(
            ghostty_lib.getEmittedBin(),
            "libghostty-vt.a",
        ).step);
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
}
