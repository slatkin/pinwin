## ADDED Requirements

### Requirement: Optimised terminal library build
The build SHALL compile libghostty-vt with an explicit Zig optimise mode, never the Zig
default (Debug). The mode SHALL be recorded in the archive's source identity so that changing
it rebuilds the archive.

#### Scenario: Explicit optimise mode
- **WHEN** `build.rs` builds ghostty
- **THEN** the zig command line includes `-Doptimize=` with the chosen mode

#### Scenario: Mode change invalidates the cache
- **WHEN** an archive exists built under one optimise mode and the build now requests another
- **THEN** the archive is rebuilt
