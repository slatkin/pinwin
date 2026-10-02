const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe_mod = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    // GTK4, gtk4-layer-shell and Pango come from the system. src/glue.c and
    // its siblings (src/glue_internal.h is their shared state, design D4) are
    // the only files that include their headers; the @cImport in src/main.zig
    // only sees ghostty/vt.h and pinwin.h.
    exe_mod.linkSystemLibrary("gtk4", .{});
    exe_mod.linkSystemLibrary("gtk4-layer-shell-0", .{});
    exe_mod.linkSystemLibrary("pangocairo", .{});
    exe_mod.linkSystemLibrary("gio-2.0", .{}); // tray publication (design D2)
    exe_mod.linkSystemLibrary("dbusmenu-glib-0.4", .{}); // host-rendered menu
    exe_mod.linkSystemLibrary("util", .{}); // forkpty; also pulls in libc

    // Static linkage keeps the installed executable independent of Zig's
    // build-cache runpath (and of any older system libghostty-vt).
    if (b.lazyDependency("ghostty", .{})) |dep| {
        exe_mod.linkLibrary(dep.artifact("ghostty-vt-static"));
        exe_mod.addIncludePath(dep.path("include"));
    }
    exe_mod.addIncludePath(b.path("src"));
    exe_mod.addCSourceFiles(.{
        .files = &.{ "src/glue.c", "src/render.c", "src/images.c", "src/pty.c", "src/input.c", "src/fontconfig.c", "src/control.c", "src/options.c", "src/tray.c" },
        .flags = &.{ "-std=gnu11", "-Wall" },
    });

    const exe = b.addExecutable(.{
        .name = "pinwin",
        .root_module = exe_mod,
    });
    b.installArtifact(exe);

    const run_step = b.step("run", "Run pinwin");
    const run_cmd = b.addRunArtifact(exe);
    run_cmd.step.dependOn(b.getInstallStep());
    if (b.args) |args| run_cmd.addArgs(args);
    run_step.dependOn(&run_cmd.step);

    // The lightweight layout-core check (tools/check_options.c): compiled
    // against the same system libraries and run (design D5).
    const check_mod = b.createModule(.{
        .target = target,
        .optimize = optimize,
        .link_libc = true,
    });
    check_mod.addIncludePath(b.path("src"));
    check_mod.addCSourceFiles(.{
        .files = &.{ "src/options.c", "src/control.c", "src/tray.c", "tools/check_options.c" },
        .flags = &.{ "-std=gnu11", "-Wall", "-Wextra" },
    });
    check_mod.linkSystemLibrary("gtk4", .{});
    check_mod.linkSystemLibrary("gio-2.0", .{});
    check_mod.linkSystemLibrary("dbusmenu-glib-0.4", .{});
    const check_exe = b.addExecutable(.{
        .name = "check_options",
        .root_module = check_mod,
    });
    const run_check = b.addRunArtifact(check_exe);
    const check_step = b.step("check", "Build and run the layout-core check");
    check_step.dependOn(&run_check.step);
}
