#!/usr/bin/env python3
"""Generate src/nerd_font_tables.h from the pinned ghostty commit's
src/font/nerd_font_tables.zig (which is itself generated there by
nerd_font_codegen.py). The table maps a Nerd Font codepoint to the constraint
Ghostty applies when drawing it; pinwin ports both the table and the maths
(Glyph.RenderOptions.Constraint) so icons are normalised the same way.

Usage: tools/gen_nerd_tables.py /path/to/ghostty/src/font/nerd_font_tables.zig
"""

import re
import sys

SIZE = {'none': 0, 'fit': 1, 'cover': 2, 'fit_cover1': 3, 'stretch': 4}
ALIGN = {'none': 0, 'start': 1, 'end': 2, 'center': 3, 'center1': 4}
HEIGHT = {'cell': 0, 'icon': 1}
FLOATS = ('pad_top', 'pad_left', 'pad_right', 'pad_bottom', 'relative_width',
          'relative_height', 'relative_x', 'relative_y')


def parse_arrays(src):
    def u16(name):
        m = re.search(r'pub const %s: \[(\d+)\]u16 = \.\{(.*?)\};' % name, src, re.S)
        vals = [int(v) for v in re.findall(r'-?\d+', m.group(2))]
        assert len(vals) == int(m.group(1)), name
        return vals

    m = re.search(r'pub const stage3: \[(\d+)\]Elem = \.\{(.*?)\n        \};', src, re.S)
    body = m.group(2)
    entries, i = [], 0
    while i < len(body):
        if body.startswith('null', i):
            entries.append(None)
            i += 4
        elif body.startswith('.{', i):
            depth, j = 0, i
            while True:
                if body[j] == '{':
                    depth += 1
                elif body[j] == '}':
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            entries.append(body[i:j + 1])
            i = j + 1
        else:
            i += 1
    assert len(entries) == int(m.group(1))
    return u16('stage1'), u16('stage2'), entries


def field(entry, name, default):
    m = re.search(r'\.%s = ([^,}]+)' % name, entry)
    return m.group(1).strip() if m else default


def rows_of(entries):
    def enum(entry, name, table, default):
        value = field(entry, name, default).lstrip('.')
        assert value in table, (name, value)
        return table[value]

    def number(entry, name, default):
        value = field(entry, name, default)
        return -1.0 if value.startswith('null') else float(value)

    rows = []
    for entry in entries:
        if entry is None:
            rows.append((0, 0, 0, 0) + (0.0,) * 8 + (-1.0, 2))
            continue
        rows.append((enum(entry, 'size', SIZE, '.none'),
                     enum(entry, 'height', HEIGHT, '.cell'),
                     enum(entry, 'align_horizontal', ALIGN, '.none'),
                     enum(entry, 'align_vertical', ALIGN, '.none'))
                    + tuple(number(entry, n, '1.0' if n.startswith('relative_') and n in ('relative_width', 'relative_height') else '0.0')
                            for n in FLOATS)
                    + (number(entry, 'max_xy_ratio', 'null'),
                       int(field(entry, 'max_constraint_width', '2'))))
    return rows


def emit(stage1, stage2, rows, out):
    w = out.write
    w('/* Generated from the pinned ghostty commit (src/font/nerd_font_tables.zig,\n'
      ' * itself generated there by nerd_font_codegen.py). Do not edit by hand;\n'
      ' * regenerate with pinwin/tools/gen_nerd_tables.py. */\n')
    w('#ifndef PINWIN_NERD_FONT_TABLES_H\n#define PINWIN_NERD_FONT_TABLES_H\n\n#include <stdint.h>\n\n')
    w('/* One Nerd Font glyph constraint, mirroring Glyph.RenderOptions.Constraint.\n'
      ' * max_xy_ratio < 0 means unset. */\n')
    w('typedef struct {\n    uint8_t size, height, align_h, align_v;\n')
    for name in FLOATS + ('max_xy_ratio',):
        w('    double %s;\n' % name)
    w('    uint8_t max_constraint_width;\n} NerdConstraint;\n\n')
    w('#define NERD_SIZE_NONE 0u\n#define NERD_SIZE_FIT 1u\n#define NERD_SIZE_COVER 2u\n')
    w('#define NERD_SIZE_FIT_COVER1 3u\n#define NERD_SIZE_STRETCH 4u\n')
    w('#define NERD_ALIGN_NONE 0u\n#define NERD_ALIGN_START 1u\n#define NERD_ALIGN_END 2u\n')
    w('#define NERD_ALIGN_CENTER 3u\n#define NERD_ALIGN_CENTER1 4u\n')
    w('#define NERD_HEIGHT_CELL 0u\n#define NERD_HEIGHT_ICON 1u\n\n')
    for name, values in (('nerd_stage1', stage1), ('nerd_stage2', stage2)):
        w('static const uint16_t %s[%d] = {\n' % (name, len(values)))
        for i in range(0, len(values), 32):
            w('    ' + ','.join(str(v) for v in values[i:i + 32]) + ',\n')
        w('};\n\n')
    w('static const NerdConstraint nerd_stage3[%d] = {\n' % len(rows))
    for row in rows:
        w('    {%d,%d,%d,%d,%s,%d},\n' % (*row[:4],
                                          ','.join('%.12g' % v for v in row[4:13]),
                                          row[13]))
    w('};\n\n#endif /* PINWIN_NERD_FONT_TABLES_H */\n')


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    src = open(sys.argv[1]).read()
    stage1, stage2, entries = parse_arrays(src)
    with open('src/nerd_font_tables.h', 'w') as out:
        emit(stage1, stage2, rows_of(entries), out)
    print('wrote src/nerd_font_tables.h')


if __name__ == '__main__':
    main()
