#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Writes client/icons/splash-text.svg: the splash screen's line of text as glyph outlines, so the
apps need neither the font nor any text rendering for it. Only needed when the line changes; builds
and scripts/make-icons.sh read the SVG.

    python3 -I scripts/make-splash-text.py [<out svg>]

Needs pango-view and Noto Sans CJK KR Regular (fonts-noto-cjk, SIL Open Font License 1.1). The
outlines are taken at 1000 px, which is the font's units per em, without hinting, then scaled to
13 units per em with the ink box centred on (100, 70) of a 200 x 80 canvas: the size of the
branding image on the Android 12+ splash screen. One <path> per glyph keeps every VectorDrawable
path short."""
import os
import re
import subprocess
import sys
import tempfile

LINE = '연세대학교 프로젝트TF'
FONT = 'Noto Sans CJK KR Regular'
EM, CX, CY = 13.0, 100.0, 70.0
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, 'client', 'icons', 'splash-text.svg')
NUM = r'-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?'


def outlines():
    """[(command, [(x, y), ...])] per glyph, in font units with the baseline at y = 0."""
    with tempfile.TemporaryDirectory() as tmp:
        svg = os.path.join(tmp, 'line.svg')
        subprocess.run(['pango-view', '--no-display', '--font=%s 1000px' % FONT, '--language=ko',
                        '--hinting=none', '--hint-metrics=off', '--subpixel-positions', '--margin=0',
                        '--pixels', '--text=' + LINE, '-o', svg], check=True)
        text = open(svg, encoding='utf-8').read()
    shapes = {gid: ' '.join(re.findall(r'\sd="([^"]*)"', body))
              for gid, body in re.findall(r'<g id="(glyph-[\d-]+)">(.*?)</g>', text, re.S)}
    uses = [(m.group(1), float(m.group(2)), float(m.group(3))) for m in
            re.finditer(r'<use xlink:href="#(glyph-[\d-]+)" x="(%s)" y="(%s)"/>' % (NUM, NUM), text)]
    need = {'M': 2, 'L': 2, 'C': 6}
    glyphs = []
    for gid, gx, gy in uses:
        cmds, cur, args = [], None, []
        for t in re.findall(r'[A-Za-z]|%s' % NUM, shapes[gid]):
            if t.isalpha():
                if t not in 'MLCZ' or args:
                    raise SystemExit('error: unexpected path data from pango-view: %s' % t)
                cur = t
                if t == 'Z':
                    cmds.append(('Z', []))
                continue
            args.append(float(t))
            if len(args) == need[cur]:
                cmds.append((cur, [(args[i] + gx, args[i + 1] + gy - uses[0][2])
                                   for i in range(0, len(args), 2)]))
                args, cur = [], 'L' if cur == 'M' else cur
        # cairo ends each glyph with a bare move to the start of its last contour
        while len(cmds) > 1 and cmds[-1][0] == 'M' and cmds[-2][0] == 'Z':
            cmds.pop()
        if cmds:
            glyphs.append(cmds)
    return glyphs


def fmt(v):
    s = ('%.2f' % v).rstrip('0').rstrip('.')
    return '0' if s in ('-0', '') else s


def main():
    glyphs = outlines()
    pts = [p for g in glyphs for c, ps in g for p in ps]
    x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    k = EM / 1000
    dx, dy = CX - (x0 + x1) / 2 * k, CY - (y0 + y1) / 2 * k
    paths = []
    for g in glyphs:
        paths.append(''.join(c if c == 'Z' else c + ' '.join('%s,%s' % (fmt(x * k + dx), fmt(y * k + dy))
                                                            for x, y in ps) for c, ps in g))
    svg = ('<svg xmlns="http://www.w3.org/2000/svg" width="200" height="80" viewBox="0 0 200 80">'
           '<!-- The splash line of client/overlay (Android branding image): the organisation name in '
           'Noto Sans CJK KR Regular 2.004 (c) 2014-2021 Adobe, SIL Open Font License 1.1, at 13 units '
           'per em, as outlines; ink centred on (100, 70). No font file ships. -->'
           '%s</svg>\n' % ''.join('<path d="%s"/>' % d for d in paths))
    with open(OUT, 'w', encoding='utf-8', newline='\n') as f:
        f.write(svg)
    print('wrote %s (%d glyphs, %d bytes)' % (OUT, len(paths), len(svg.encode())))


if __name__ == '__main__':
    main()
