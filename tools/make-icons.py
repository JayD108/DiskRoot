"""Builds every DiskRoot icon/logo file from the master logo image.

    python tools/make-icons.py <master.png>

Writes:
  docs/logo/diskroot-logo-master.png   cropped square master (1024 px, no watermark)
  docs/logo/diskroot-logo-512.png      README / social size, rounded tile
  crates/native/assets/icon.ico     exe, taskbar, Start menu, installer (16-256 px)
  crates/native/assets/icon-256.png
  crates/native/assets/icon-64.rgba window icon (raw RGBA, 64x64)
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
src = Image.open(sys.argv[1]).convert("RGBA")
w, h = src.size

# Square crop around the leaf. The Gemini sparkle watermark sits in the
# bottom-right corner, outside this box.
side = int(w * 0.83)
cx, cy = w // 2, int(h * 0.502)
box = (cx - side // 2, cy - side // 2, cx + side // 2, cy + side // 2)
crop = src.crop(box)
# Background colour = the logo's own dark tile (sampled from a corner).
bg = crop.getpixel((8, 8))
# Standard icon padding: the leaf at ~86% of the tile, centred.
master = Image.new("RGBA", (1024, 1024), bg)
inner = 880
master.paste(crop.resize((inner, inner), Image.LANCZOS), ((1024 - inner) // 2, (1024 - inner) // 2))


def tile(size: int, radius_frac: float = 0.22) -> Image.Image:
    """The logo on a rounded dark tile, transparent outside the corners."""
    img = master.resize((size, size), Image.LANCZOS)
    if size <= 48:
        img = img.filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=2))
    mask = Image.new("L", (size * 4, size * 4), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size * 4 - 1, size * 4 - 1), radius=int(size * 4 * radius_frac), fill=255)
    mask = mask.resize((size, size), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(img, (0, 0), mask)
    return out


os.makedirs(os.path.join(ROOT, "docs", "logo"), exist_ok=True)
master.save(os.path.join(ROOT, "docs", "logo", "diskroot-logo-master.png"))
tile(512).save(os.path.join(ROOT, "docs", "logo", "diskroot-logo-512.png"))

assets = os.path.join(ROOT, "crates", "native", "assets")
tile(256).save(os.path.join(assets, "icon-256.png"))
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
icons = [tile(s) for s in sizes]
icons[-1].save(os.path.join(assets, "icon.ico"), sizes=[(s, s) for s in sizes], append_images=icons[:-1])
with open(os.path.join(assets, "icon-64.rgba"), "wb") as f:
    f.write(tile(64).tobytes())

# Preview sheet of the small sizes (for checking legibility).
sheet = Image.new("RGBA", (420, 140), (240, 240, 240, 255))
x = 10
for s in [16, 24, 32, 48, 64, 128]:
    sheet.paste(tile(s), (x, 70 - s // 2), tile(s))
    x += s + 14
sheet.save(os.path.join(os.environ.get("TEMP", "."), "diskroot-icon-preview.png"))
print("background", bg, "done")

# In-app logo (start screen, About): raw RGBA, 128x128, drawn by egui.
with open(os.path.join(assets, "logo-128.rgba"), "wb") as f:
    f.write(tile(128).tobytes())

# Installer welcome/finish page image: 164x314 24-bit BMP (NSIS MUI size),
# the logo on the dark tile colour, near the top.
side_img = Image.new("RGB", (164, 314), bg[:3])
logo = master.resize((132, 132), Image.LANCZOS).convert("RGB")
side_img.paste(logo, ((164 - 132) // 2, 40))
side_img.save(os.path.join(ROOT, "installer", "welcome.bmp"))
print("in-app logo and installer image written")
