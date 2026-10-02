# Spec Delta

## Purpose

The tray entry and its options window are gone with the program form: the host drives the
panel through `pinwin_apply_layout` and owns persistence. This capability is retired in full.

## REMOVED Requirements

### Requirement: Tray access to options
The tray entry (`tray.c`, StatusNotifierItem, dbusmenu menu) is deleted with design D1. No
host-visible replacement: optionality the tray provided (per-instance reachability) is now
the host's own UI calling `pinwin_apply_layout`.

### Requirement: Staged options editing
The options window is deleted with design D1. Live-apply semantics move to `pinwin-panel`'s
"Apply layout without restarting the terminal" requirement; there is no draft state — every
`pinwin_apply_layout` call is validated and applied atomically or rejected whole.

### Requirement: Directional gutters and docking geometry
Geometry rules are not dropped, they move: folded into `pinwin-panel`'s "Directional gutters
and docking geometry" requirement unchanged.

### Requirement: Apply without restarting the command
Folded into `pinwin-panel`'s "Apply layout without restarting the terminal" requirement; the
"command" is now the host's child, which the library never restarts.

### Requirement: Validate before saving or applying
Validation rules move to `pinwin-panel`'s "Validate before applying" requirement. The save
half is deleted with the config file: there is nothing to save to, and persistence is the
host's job (mbv's `config.toml`).

### Requirement: Persist and restore layout settings
Deleted with the GKeyFile config (design D1). The host supplies the full layout at every
start; no saved layout overrides anything. mbv-side persistence lives in the mbv plan, not
here.

### Requirement: Running without a tray
Meaningless with no tray at all. Deleted.
