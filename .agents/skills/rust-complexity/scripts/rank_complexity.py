#!/usr/bin/env python3
"""Rank rust-code-analysis-cli JSON output by cyclomatic complexity.

Usage:
    rust-code-analysis-cli -p ./src -p ./crates -m -O json > rca.json
    rank_complexity.py rca.json [--top N] [--no-exclude]

Reads line-delimited JSON (one object per file unit), walks nested spaces
for kind == "function", and prints a ranked table. Prints top-N overall
plus top-N excluding examples/test paths when those rows differ.
"""
import json
import sys


def collect(path):
    rows = []
    try:
        with open(path, encoding="utf-8") as handle:
            rows = parse_lines(handle)
    except OSError as exc:
        print(f"error: cannot open {path}: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
    return rows


def parse_lines(handle):
    rows = []
    for lineno, line in enumerate(handle, start=1):
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError as exc:
            print(f"warning: skip line {lineno}: {exc}", file=sys.stderr)
            continue
        fname = obj.get("name", "?")

        def walk(spaces, prefix, fname=fname):
            for s in spaces or []:
                kind = s.get("kind", "")
                name = s.get("name", "")
                sl = s.get("start_line")
                el = s.get("end_line")
                m = s.get("metrics") or {}
                cyc = (m.get("cyclomatic") or {}).get("sum", 0)
                cog = (m.get("cognitive") or {}).get("sum", 0)
                if kind == "function":
                    rows.append((cyc, cog, fname, name, sl, el, prefix))
                walk(s.get("spaces", []),
                     prefix + name + "::" if name else prefix,
                     fname)

        walk(obj.get("spaces", []), "", fname)
    return rows


def is_aux(fname):
    low = fname.lower()
    return "/examples/" in low or "/tests/" in low or "test" in low


def show(title, rows, top):
    print(f"== {title} ==")
    for cyc, cog, fn, name, sl, el, pfx in rows[:top]:
        print(f"{cyc:5.0f} (cog {cog:.0f})  {fn}:{sl}-{el}  {pfx}{name}")


def parse_top(argv):
    top = 10
    for i, arg in enumerate(argv):
        if arg == "--top" and i + 1 < len(argv):
            try:
                top = int(argv[i + 1])
            except ValueError as exc:
                print(f"error: --top needs an integer, got {argv[i + 1]!r}",
                      file=sys.stderr)
                raise SystemExit(2) from exc
    return top


def main(argv):
    if len(argv) < 2 or argv[1].startswith("-"):
        print("usage: rank_complexity.py RCA_JSON [--top N] [--no-exclude]",
              file=sys.stderr)
        raise SystemExit(2)
    path = argv[1]
    top = parse_top(argv)
    rows = collect(path)
    rows.sort(key=lambda r: -r[0])
    print(f"total functions: {len(rows)}")
    show(f"top {top} overall", rows, top)
    if "--no-exclude" not in argv:
        filt = [r for r in rows if not is_aux(r[2])]
        if filt[:top] != rows[:top]:
            print("")
            show(f"top {top} excl examples/tests", filt, top)


if __name__ == "__main__":
    main(sys.argv)
