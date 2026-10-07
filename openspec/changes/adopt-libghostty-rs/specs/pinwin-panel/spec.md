## MODIFIED Requirements

### Requirement: Optimised terminal library build
Every build of this repository SHALL compile libghostty-vt with an explicit Zig optimise mode.
The mode SHALL never be the Zig default (Debug), in the dev or the release profile. Changing the mode SHALL
rebuild libghostty-vt instead of reusing an archive built under the previous mode.

#### Scenario: Explicit optimise mode
- **WHEN** a `cargo build` from the repository root (dev or release profile) builds ghostty
- **THEN** the zig command line includes `-Doptimize=` with the chosen mode, and the mode is
  not `Debug`

#### Scenario: Mode change invalidates the cache
- **WHEN** an archive exists built under one optimise mode and the build now requests another
- **THEN** the archive is rebuilt
