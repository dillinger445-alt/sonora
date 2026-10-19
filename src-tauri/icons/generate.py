"""Renders the SONORA app icon (1024x1024, 4x supersampled) with Pillow.

    python3 generate.py icon-source.png                 # macOS grid, baked shadow
    python3 generate.py icon-source-windows.png --full  # full bleed (Windows/Linux)
    python3 generate.py icon-source-small.png --small   # simplified glyph for <= 48 px
    python3 generate.py small-mac.png --small-mac       # simplified glyph, macOS grid

The files in this folder were produced from these renders; `npx tauri icon
icon-source.png` regenerates a standard set from a single master as well.
"""
import math, sys
from PIL import Image, ImageDraw, ImageFilter, ImageChops

S = 4                      # supersampling factor
N = 1024 * S
# macOS icon grid: 824px body on a 1024px canvas with a baked-in shadow.
# Windows/Linux variant: near full-bleed body, no shadow.
SMALL = '--small' in sys.argv or '--small-mac' in sys.argv
FULL = '--full' in sys.argv or '--small' in sys.argv
BODY = (984 if FULL else 824) * S
OFF = (N - BODY) // 2
K = BODY / (824 * S)       # scale design dimensions with the body

def superellipse_mask(size, n=5.0, scale=1.0):
    """Continuous-corner 'squircle' mask (superellipse exponent n)."""
    m = Image.new('L', (size, size), 0)
    d = ImageDraw.Draw(m)
    a = size / 2 * scale
    cx = cy = size / 2
    pts = []
    steps = 2400
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + a * (abs(c) ** (2 / n)) * (1 if c >= 0 else -1)
        y = cy + a * (abs(s) ** (2 / n)) * (1 if s >= 0 else -1)
        pts.append((x, y))
    d.polygon(pts, fill=255)
    return m

def vgradient(w, h, top, bottom):
    g = Image.new('RGB', (1, h))
    for y in range(h):
        t = y / (h - 1)
        g.putpixel((0, y), tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3)))
    return g.resize((w, h))

canvas = Image.new('RGBA', (N, N), (0, 0, 0, 0))

# --- drop shadow (baked in, per macOS icon template) -----------------------
mask = superellipse_mask(BODY)
if not FULL:
    shadow = Image.new('L', (N, N), 0)
    shadow.paste(mask, (OFF, OFF + 12 * S))
    shadow = shadow.filter(ImageFilter.GaussianBlur(20 * S))
    shadow = shadow.point(lambda v: int(v * 0.42))
    canvas.paste((0, 0, 0, 255), (0, 0), shadow)

# --- body -------------------------------------------------------------------
body = vgradient(BODY, BODY, (44, 47, 54), (16, 17, 20)).convert('RGBA')
# soft top-left light
glow = Image.new('L', (BODY, BODY), 0)
ImageDraw.Draw(glow).ellipse((-BODY * 0.35, -BODY * 0.55, BODY * 0.95, BODY * 0.55), fill=24)
glow = glow.filter(ImageFilter.GaussianBlur(90 * S))
body = Image.composite(Image.new('RGBA', (BODY, BODY), (255, 255, 255, 255)), body, glow)

# fine engineering grid (very faint)
grid = Image.new('L', (BODY, BODY), 0)
gd = ImageDraw.Draw(grid)
step = BODY // 16
for i in range(1, 16 if not SMALL else 1):
    gd.line([(i * step, 0), (i * step, BODY)], fill=10, width=S)
    gd.line([(0, i * step), (BODY, i * step)], fill=10, width=S)
body = Image.composite(Image.new('RGBA', (BODY, BODY), (200, 210, 230, 255)), body, grid)

# --- glyph: waveform bars --------------------------------------------------
LIME_TOP = (214, 255, 98)
LIME_BOTTOM = (156, 222, 30)
# A realistic waveform envelope: a dominant transient with decaying detail.
count = 9 if SMALL else 25
def env(i):
    x = (i - (count - 1) / 2) / ((count - 1) / 2)          # -1 .. 1
    base = math.exp(-2.6 * x * x)                            # overall shape
    detail = 1.0 if SMALL else 0.70 + 0.30 * math.cos(i * 2.35) * math.cos(i * 0.9)
    return max(0.22 if SMALL else 0.10, base * detail)
heights = [env(i) for i in range(count)]
peak = max(heights)
heights = [h / peak for h in heights]
bar_w = int((52 if SMALL else 14) * S * K)
gap = int((30 if SMALL else 11) * S * K)
total = len(heights) * bar_w + (len(heights) - 1) * gap
x0 = (BODY - total) // 2
cy = BODY // 2
max_h = int(268 * S * K)

glyph = Image.new('L', (BODY, BODY), 0)
gdraw = ImageDraw.Draw(glyph)
for i, h in enumerate(heights):
    x = x0 + i * (bar_w + gap)
    half = max(int(h * max_h), bar_w // 2 + 2 * S)
    gdraw.rounded_rectangle((x, cy - half, x + bar_w, cy + half), radius=bar_w // 2, fill=255)

# subtle bloom behind the glyph (not neon)
bloom = glyph.filter(ImageFilter.GaussianBlur(30 * S)).point(lambda v: int(v * 0.20))
body = Image.composite(Image.new('RGBA', (BODY, BODY), LIME_BOTTOM + (255,)), body, bloom)

lime = vgradient(BODY, BODY, LIME_TOP, LIME_BOTTOM).convert('RGBA')
body = Image.composite(lime, body, glyph)

# centre baseline through the bars (precision cue)
base = Image.new('L', (BODY, BODY), 0)
ImageDraw.Draw(base).line([(x0 - int(54 * S * K), cy), (x0 + total + int(54 * S * K), cy)], fill=0 if SMALL else 60, width=2 * S)
base = ImageChops.subtract(base, glyph)
body = Image.composite(Image.new('RGBA', (BODY, BODY), (210, 230, 190, 255)), body, base)

# peak-ceiling reference lines with end ticks (meter-scale cue)
ref = Image.new('L', (BODY, BODY), 0)
rd = ImageDraw.Draw(ref)
ceil = max_h + int(30 * S * K)
lx0, lx1 = x0 - int(54 * S * K), x0 + total + int(54 * S * K)
for yy in (() if SMALL else (cy - ceil, cy + ceil)):
    rd.line([(lx0, yy), (lx1, yy)], fill=46, width=2 * S)
    for xx in (lx0, lx1):
        rd.line([(xx, yy - 14 * S), (xx, yy + 14 * S)], fill=90, width=3 * S)
body = Image.composite(Image.new('RGBA', (BODY, BODY), (200, 214, 225, 255)), body, ref)

# --- edge treatment: thin top highlight + bottom shade ----------------------
inner = superellipse_mask(BODY, scale=(BODY - 6 * S) / BODY)
edge = ImageChops.subtract(mask, inner)
top_fade = vgradient(BODY, BODY, (255, 255, 255), (0, 0, 0)).convert('L').point(lambda v: int(v * 0.16))
edge_hi = ImageChops.multiply(edge, top_fade)
body = Image.composite(Image.new('RGBA', (BODY, BODY), (255, 255, 255, 255)), body, edge_hi)

body.putalpha(mask)
canvas.alpha_composite(body, (OFF, OFF))

out = canvas.resize((1024, 1024), Image.LANCZOS)
out.save([a for a in sys.argv[1:] if not a.startswith('--')][0])
print('ok')
