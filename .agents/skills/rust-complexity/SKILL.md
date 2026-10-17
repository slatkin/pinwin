---
name: rust-complexity
description: Run rust-code-analysis-cli to find cyclomatic/cognitive complexity hotspots in Rust code and report the top-N most complex functions. Use whenever the user mentions complexity, hotspots, most complex functions, refactor targets, splitting large files, or asks what code is hardest to maintain — even if they don't name the tool. Also useful on your own initiative before planning a refactor or file split.
---

# Rust complexity hotspots

Find the most complex Rust functions with `rust-code-analysis-cli` and rank them.

## Prerequisites

`rust-code-analysis-cli` must be on PATH (`which rust-code-analysis-cli`).
If missing, stop and tell the user — do not substitute another tool.

## Workflow

1. From the repo root, run the analysis over the Rust sources in scope
   (default: whole workspace):
   ```bash
   rust-code-analysis-cli -p ./src -p ./crates -m -O json > rca.json
   ```
   Narrow `-p` to the subsystem under discussion when the user names one
   (e.g. `-p ./src/proxy`).
2. Rank with the bundled script (deterministic — do not hand-roll parsing):
   ```bash
   python3 <skill-dir>/scripts/rank_complexity.py rca.json [--top N]
   ```
   Default N is 10.
3. Report using the template below. Delete the scratch `rca.json` when done.

## Report structure

ALWAYS use this exact shape:

| # | CC (cog) | Method |
|---|----------|--------|
| 1 | <cyclomatic> (<cognitive>) | `<file>:<start>-<end>` `<qualified>::<name>` |

Followed by at most two short lines:
- Flag rows from `examples/` or test paths as non-production.
- If excluding them changes the top-N, append the replacement rows.

## Notes

- Rank key is `metrics.cyclomatic.sum` per `kind == "function"` space;
  cognitive sum is context only.
- Total function count (script's first line) goes in the report intro —
  it tells the reader whether 24 is an outlier or the norm.
- Keep prose to the minimum: table first, then the two lines. No essays,
  no per-function commentary unless the user asks.
