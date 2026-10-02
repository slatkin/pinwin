# Repository Guidelines

## Project Structure & Module Organization

`pinwin.sh` is the executable Bash script and the only application source file. `README.md` documents installation, usage, behavior, and known caveats. There are no vendored assets or automated tests. At runtime, the script writes `~/.config/niri/woims/pin.kdl`; this generated file is not part of the repository.

## Build, Test, and Development Commands

No build step is required. Run `bash -n pinwin.sh` after editing to check shell syntax. Run `./pinwin.sh` to launch the default `mbv` sidebar, or `./pinwin.sh htop` to try another command. `COLS=60 GUTTER=8 ./pinwin.sh htop` exercises the configurable width and gap. Live runs require niri, Ghostty, and `jq`, plus the niri include described in `README.md`.

## Coding Style & Naming Conventions

Keep the script compatible with Bash and use four spaces inside functions and control blocks. Use lowercase names for functions and temporary variables, and uppercase names for user settings and layout constants such as `COLS`, `GUTTER`, and `TOP`. Quote path and command arguments where expansion is intended to remain one argument. Keep comments focused on niri behavior or timing that is not obvious from the code. Preserve the script's executable bit.

## Testing Guidelines

There is no test framework or coverage target. Check syntax with `bash -n pinwin.sh`, then test changes in a running niri session. Verify launch, workspace following, width changes, and cleanup when the launched command exits when those paths are affected. Confirm that `pin.kdl` is emptied and niri reloads after a normal exit. If changing struts, keep `LEFT`, `RIGHT`, `TOP`, and `BOTTOM` aligned with the user's niri layout as explained in `README.md`.

## Commit & Pull Request Guidelines

Recent commits use short, imperative subjects that describe the behavior changed, such as “Default GUTTER to 12” and “Rename winpin to pinwin.” Follow that pattern. In a pull request, describe the user-visible behavior, list the checks performed, and mention any required niri configuration change. Include screenshots only when a layout change is easier to assess visually.
