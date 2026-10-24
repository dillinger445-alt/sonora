"""Renders SONORA's icon set and its mark (docs/DESIGN.md, "The mark").

    python3 src-tauri/icons/generate.py

writes every PNG, icon.ico and icon.icns in this folder, and the SVGs in
public/ (the favicon, the icon and the one-colour mark). Pillow only.

The mark is a meter whose scale is a sound wave: a needle on a pivot reads
a scale bent round an arc, and the scale's graduations are the samples of a
waveform, centred on the arc as on a zero line. It is drawn three ways, so
it stays a meter at every size: in full (19 graduations, above 96 px),
small (7, from 25 to 96 px) and tiny (5, for 16 and 24 px). It is one colour:
lime on graphite for the icon, any single colour for the mark.

`npx tauri icon public/sonora-icon.svg` makes a standard set from the full
drawing alone, should Tauri's own tool be preferred; it does not swap in the
small drawings.
"""
import math, os
from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
PUBLIC = os.path.join(HERE, '..', '..', 'public')

LIME = (192, 232, 78)        # --c-lime
GRAPHITE = (27, 30, 33)      # --c-canvas
LINE = (49, 54, 60)          # --c-line
HEX = lambda c: '#%02x%02x%02x' % c

LEVELS = {
    # graduations, their stroke, the needle's stroke, pivot radius, wave
    # amplitude (quiet ends, loud middle), half the scale's angle
    'full': dict(n=19, tick=2.7, needle=3.4, pivot=5.6, amp=(1.8, 10.5), span=64),
    'small': dict(n=7, tick=5.6, needle=6.0, pivot=7.4, amp=(3.0, 8.6), span=62),
    'tiny': dict(n=5, tick=7.6, needle=7.8, pivot=9.0, amp=(3.6, 8.4), span=58),
}


def level_for(px):
    return 'tiny' if px <= 24 else 'small' if px <= 96 else 'full'


def strokes(level, box):
    """The mark as strokes and dots, drawn in a 100-unit square scaled into
    `box` (x, y, size): ('line', x1, y1, x2, y2, width) and ('dot', x, y, r)."""
    L = LEVELS[level]
    x0, y0, size = box
    k = size / 100
    cx, cy, R = 50, 74, 40          # pivot, and the arc: the wave's zero line
    n, span = L['n'], L['span']
    quiet, loud = L['amp']
    amps = []
    for i in range(n):
        x = (i / (n - 1) - 0.5) * 2
        env = math.exp(-2.2 * x * x)
        detail = 1.0 if level != 'full' else 0.58 + 0.42 * abs(math.cos(i * 2.3 + 0.4))
        amps.append(quiet + (loud - quiet) * env * detail)
    # Centred on its own height.
    top = cy - R - max(amps) - L['tick'] / 2
    dy = 50 - (top + cy + L['pivot']) / 2
    P = lambda x, y: (x0 + x * k, y0 + (y + dy) * k)
    out = []
    for i, amp in enumerate(amps):
        a = math.radians(-span + 2 * span * i / (n - 1))
        s, c = math.sin(a), -math.cos(a)
        out.append(('line', *P(cx + s * (R - amp), cy + c * (R - amp)), *P(cx + s * (R + amp), cy + c * (R + amp)), L['tick'] * k))
    # The needle reads past the middle of the scale and stops short of it.
    na = math.radians(22)
    tip = R - loud - (4.5 if level == 'full' else 5.5)
    out.append(('line', *P(cx, cy), *P(cx + math.sin(na) * tip, cy - math.cos(na) * tip), L['needle'] * k))
    out.append(('dot', *P(cx, cy), L['pivot'] * k))
    return out


# Body: a rounded square, on the macOS grid (824 of 1024, room for a shadow)
# or nearly full bleed (Windows, Linux, the web).
BODIES = {'mac': (100, 824, 185), 'full': (20, 984, 220)}


# Room round the mark: less as the drawing gets simpler and the icon smaller.
INSET = {'full': 0.12, 'small': 0.07, 'tiny': 0.03}


def mark_box(body, level):
    bx, bw, _ = BODIES[body]
    inset = bw * INSET[level]
    return (bx + inset, bx + inset, bw - 2 * inset)


# ---------------------------------------------------------------- SVG

def svg_marks(items, color):
    out = []
    for it in items:
        if it[0] == 'line':
            _, x1, y1, x2, y2, w = it
            out.append(f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="{color}" stroke-width="{w:.1f}" stroke-linecap="round"/>')
        else:
            _, x, y, r = it
            out.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r:.1f}" fill="{color}"/>')
    return out


def svg_icon(level, body='full'):
    bx, bw, r = BODIES[body]
    parts = [f'<rect x="{bx}" y="{bx}" width="{bw}" height="{bw}" rx="{r}" fill="{HEX(GRAPHITE)}"/>']
    # The hairline edge only where it stays a hairline (not on the favicon).
    if level == 'full':
        parts.append(f'<rect x="{bx + 3}" y="{bx + 3}" width="{bw - 6}" height="{bw - 6}" rx="{r - 3}" fill="none" stroke="{HEX(LINE)}" stroke-width="6"/>')
    parts += svg_marks(strokes(level, mark_box(body, level)), HEX(LIME))
    return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">\n  ' + '\n  '.join(parts) + '\n</svg>\n'


def svg_mark(level):
    parts = svg_marks(strokes(level, (0, 0, 1024)), 'currentColor')
    return '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" fill="currentColor">\n  ' + '\n  '.join(parts) + '\n</svg>\n'


# ---------------------------------------------------------------- PNG

S = 4  # supersampling


def draw_mark(mask, items, scale):
    d = ImageDraw.Draw(mask)
    for it in items:
        if it[0] == 'line':
            _, x1, y1, x2, y2, w = it
            x1, y1, x2, y2, h = x1 * scale, y1 * scale, x2 * scale, y2 * scale, w * scale / 2
            dx, dy = x2 - x1, y2 - y1
            ln = math.hypot(dx, dy) or 1
            nx, ny = -dy / ln * h, dx / ln * h
            d.polygon([(x1 + nx, y1 + ny), (x2 + nx, y2 + ny), (x2 - nx, y2 - ny), (x1 - nx, y1 - ny)], fill=255)
            for x, y in ((x1, y1), (x2, y2)):
                d.ellipse((x - h, y - h, x + h, y + h), fill=255)
        else:
            _, x, y, r = it
            d.ellipse(((x - r) * scale, (y - r) * scale, (x + r) * scale, (y + r) * scale), fill=255)


def render(px, body='full', shadow=False, level=None):
    """The icon at `px` pixels."""
    level = level or level_for(px)
    N = px * S
    sc = N / 1024
    bx, bw, r = BODIES[body]
    canvas = Image.new('RGBA', (N, N), (0, 0, 0, 0))
    box = [bx * sc, bx * sc, (bx + bw) * sc, (bx + bw) * sc]
    if shadow:
        sh = Image.new('L', (N, N), 0)
        ImageDraw.Draw(sh).rounded_rectangle([box[0], box[1] + 12 * sc, box[2], box[3] + 12 * sc], radius=r * sc, fill=int(255 * 0.38))
        sh = sh.filter(ImageFilter.GaussianBlur(18 * sc))
        canvas.paste((0, 0, 0, 255), (0, 0), sh)
    d = ImageDraw.Draw(canvas)
    d.rounded_rectangle(box, radius=r * sc, fill=GRAPHITE + (255,))
    # The hairline edge from 64 px up; below, it would be a grey frame.
    if px >= 64:
        w = max(1, round(6 * sc))
        d.rounded_rectangle([b + (3 * sc if i < 2 else -3 * sc) for i, b in enumerate(box)], radius=(r - 3) * sc, outline=LINE + (255,), width=w)
    m = Image.new('L', (N, N), 0)
    draw_mark(m, strokes(level, mark_box(body, level)), sc)
    canvas.paste(LIME + (255,), (0, 0), m)
    # An exact average of each 4x4 block (no ringing at the edges).
    return canvas.resize((px, px), Image.BOX)


def main():
    out = lambda name: os.path.join(HERE, name)
    # Masters (1024 px): the macOS grid with its shadow, full bleed, and the
    # small drawing full bleed.
    render(1024, 'mac', shadow=True, level='full').save(out('icon-source.png'))
    render(1024, 'full', level='full').save(out('icon-source-windows.png'))
    render(1024, 'full', level='small').save(out('icon-source-small.png'))
    # The set Tauri bundles and the Windows tiles.
    for name, px in [
        ('32x32.png', 32), ('64x64.png', 64), ('128x128.png', 128), ('128x128@2x.png', 256), ('icon.png', 512),
        ('Square30x30Logo.png', 30), ('Square44x44Logo.png', 44), ('Square71x71Logo.png', 71), ('Square89x89Logo.png', 89),
        ('Square107x107Logo.png', 107), ('Square142x142Logo.png', 142), ('Square150x150Logo.png', 150),
        ('Square284x284Logo.png', 284), ('Square310x310Logo.png', 310), ('StoreLogo.png', 50),
    ]:
        render(px, 'full').save(out(name))
    ico = [render(px, 'full') for px in (16, 24, 32, 48, 64, 128, 256)]
    ico[-1].save(out('icon.ico'), sizes=[(i.width, i.height) for i in ico], append_images=ico[:-1])
    # macOS: 16@2x and 32@2x take the small drawing.
    icns = [render(px, 'mac', shadow=True, level='small' if px <= 64 else 'full') for px in (32, 64, 128, 256, 512, 1024)]
    icns[-1].save(out('icon.icns'), append_images=icns[:-1])
    # The web: the favicon (shown at 16 and 32 px), the icon, the mark.
    os.makedirs(PUBLIC, exist_ok=True)
    for name, text in [
        ('favicon.svg', svg_icon('tiny')),
        ('sonora-icon.svg', svg_icon('full')),
        ('sonora-mark.svg', svg_mark('full')),
        ('sonora-mark-small.svg', svg_mark('small')),
        ('sonora-mark-tiny.svg', svg_mark('tiny')),
    ]:
        with open(os.path.join(PUBLIC, name), 'w') as f:
            f.write(text)
    print('ok')


if __name__ == '__main__':
    main()
