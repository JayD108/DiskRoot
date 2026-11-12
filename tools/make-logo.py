"""Builds DiskRoot's logo files from the master image (leaf on a dark tile):
the leaf on a transparent background, and the wordmark (leaf with
"DiskRoot" under it).

    python tools/make-logo.py

Writes:
  docs/logo/diskroot-logo.png       wordmark: leaf + "DiskRoot", transparent (README)
  docs/logo/diskroot-leaf-512.png   the leaf alone, transparent
  crates/native/assets/icon.ico     exe, taskbar, Start menu, installer (16-256 px)
  crates/native/assets/icon-256.png
  crates/native/assets/icon-64.rgba window icon (raw RGBA, 64x64)
  crates/native/assets/logo-128.rgba start screen / About (raw RGBA, 128x128)
"""
import os

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = Image.open(os.path.join(ROOT, "docs", "logo", "diskroot-logo-master.png")).convert("RGBA")

# The dark tile colour (also visible in the gaps between the leaf's parts).
bg = src.getpixel((8, 8))[:3]


def without_background(img: Image.Image) -> Image.Image:
    """Pixels close to the tile colour become transparent, with soft edges."""
    out = img.copy()
    px = out.load()
    w, h = out.size
    for y in range(h):
        for x in range(w):
            r, g, b, _ = px[x, y]
            d = max(abs(r - bg[0]), abs(g - bg[1]), abs(b - bg[2]))
            a = 0 if d <= 14 else 255 if d >= 54 else int((d - 14) * 255 / 40)
            px[x, y] = (r, g, b, a)
    return out


leaf = without_background(src)
leaf = leaf.crop(leaf.getchannel("A").point(lambda v: 255 if v > 24 else 0).getbbox())


def square(img: Image.Image, size: int, fill: float = 0.92) -> Image.Image:
    """The leaf centred on a transparent square, `fill` of its height."""
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    scale = size * fill / max(img.size)
    w, h = int(img.width * scale), int(img.height * scale)
    small = img.resize((w, h), Image.LANCZOS)
    if size <= 48:
        small = small.filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=2))
    canvas.paste(small, ((size - w) // 2, (size - h) // 2), small)
    return canvas


os.makedirs(os.path.join(ROOT, "docs", "logo"), exist_ok=True)
square(leaf, 512).save(os.path.join(ROOT, "docs", "logo", "diskroot-leaf-512.png"), optimize=True)

# Wordmark: the leaf, then "Disk" in the leaf's green and "Root" in its amber.
font = ImageFont.truetype(os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts", "segoeuib.ttf"), 150)
green, amber = (78, 138, 72, 255), (206, 138, 42, 255)
words = [("Disk", green), ("Root", amber)]
tmp = ImageDraw.Draw(Image.new("RGBA", (1, 1)))
widths = [tmp.textlength(t, font=font) for t, _ in words]
text_w = int(sum(widths))
ascent, descent = font.getmetrics()
leaf_h = 520
leaf_img = leaf.resize((int(leaf.width * leaf_h / leaf.height), leaf_h), Image.LANCZOS)
gap = 24
W = max(leaf_img.width, text_w) + 80
H = leaf_h + gap + ascent + descent + 40
mark = Image.new("RGBA", (W, H), (0, 0, 0, 0))
mark.paste(leaf_img, ((W - leaf_img.width) // 2, 20), leaf_img)
d = ImageDraw.Draw(mark)
x = (W - text_w) // 2
for (t, c), tw in zip(words, widths):
    d.text((x, 20 + leaf_h + gap), t, font=font, fill=c)
    x += tw
mark.save(os.path.join(ROOT, "docs", "logo", "diskroot-logo.png"), optimize=True)

# App icons: the transparent leaf (no tile).
assets = os.path.join(ROOT, "crates", "native", "assets")
square(leaf, 256).save(os.path.join(assets, "icon-256.png"))
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
icons = [square(leaf, s) for s in sizes]
icons[-1].save(os.path.join(assets, "icon.ico"), sizes=[(s, s) for s in sizes], append_images=icons[:-1])
with open(os.path.join(assets, "icon-64.rgba"), "wb") as f:
    f.write(square(leaf, 64).tobytes())
with open(os.path.join(assets, "logo-128.rgba"), "wb") as f:
    f.write(square(leaf, 128).tobytes())
print("ok", mark.size)
