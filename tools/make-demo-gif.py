"""Turns frames from tools/ui-record.ps1 into the README demo GIF.

    python tools/make-demo-gif.py <frames dir> <out.gif> [--blur x0,y0,x1,y1 ...] [--width 1100]

- Plays the scan in REAL TIME: each frame lasts as long as it really did.
- The time spent on the Windows admin prompt (the secure desktop can't be
  captured) is cut: two frames of "Approve the Windows prompt" are kept and
  labelled, and the counter starts when the admin helper starts.
- --blur x0,y0,x1,y1 (fractions of the window) and --ring cx,cy,r0,r1 (centre
  as fractions of width/height, radii as fractions of width: the sunburst's
  outer ring holds place names) are blurred in every frame, so folder names
  never appear in a public image.
"""
import csv
import os
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageStat

args = sys.argv[1:]
src, out = args[0], args[1]
blur, rings, width = [], [], 1100
i = 2
while i < len(args):
    if args[i] == "--blur":
        blur.append(tuple(float(v) for v in args[i + 1].split(",")))
        i += 2
    elif args[i] == "--ring":
        rings.append(tuple(float(v) for v in args[i + 1].split(",")))
        i += 2
    elif args[i] == "--width":
        width = int(args[i + 1])
        i += 2
    else:
        sys.exit(f"unknown option {args[i]}")

rows = [(int(r["frame"]), int(r["ms"]), int(r["helper"])) for r in csv.DictReader(open(os.path.join(src, "frames.csv")))]
first_helper = next((k for k, (_, _, h) in enumerate(rows) if h), None)
if first_helper is None:
    sys.exit("the helper never ran: was the Windows prompt approved?")
t0 = rows[first_helper][1]

fonts = r"C:\Windows\Fonts"
font = ImageFont.truetype(os.path.join(fonts, "seguisb.ttf"), 26)


def load(n):
    im = Image.open(os.path.join(src, f"f{n:04d}.png")).convert("RGB")
    w, h = im.size
    soft = im.filter(ImageFilter.GaussianBlur(max(4, w // 230)))
    for x0, y0, x1, y1 in blur:
        box = (int(x0 * w), int(y0 * h), int(x1 * w), int(y1 * h))
        im.paste(soft.crop(box), box)
    for cx, cy, r0, r1 in rings:
        mask = Image.new("L", im.size, 0)
        d = ImageDraw.Draw(mask)
        c = (cx * w, cy * h)
        d.ellipse((c[0] - r1 * w, c[1] - r1 * w, c[0] + r1 * w, c[1] + r1 * w), fill=255)
        d.ellipse((c[0] - r0 * w, c[1] - r0 * w, c[0] + r0 * w, c[1] + r0 * w), fill=0)
        im.paste(soft, (0, 0), mask)
    return im if w == width else im.resize((width, round(h * width / w)), Image.LANCZOS)


def label(im, text):
    d = ImageDraw.Draw(im)
    x0, y0, x1, y1 = d.textbbox((0, 0), text, font=font)
    w, h = x1 - x0 + 28, y1 - y0 + 18
    X, Y = im.width - w - 18, im.height - h - 60
    d.rounded_rectangle((X, Y, X + w, Y + h), radius=10, fill=(30, 30, 28))
    d.text((X + 14 - x0, Y + 9 - y0), text, font=font, fill=(255, 255, 255))
    return im


frames, durations = [], []
# 1. Start screen (shown for 0.8 s).
frames.append(label(load(rows[0][0]), "Fast scan of C:  (DiskRoot 0.2)"))
durations.append(800)
# 2. The admin prompt: one frame, labelled, then cut.
if first_helper > 4:
    frames.append(label(load(rows[first_helper - 1][0]), "Windows asks for admin  (wait cut)"))
    durations.append(900)
# 3. Real time from the moment the helper starts, until the result is on
#    screen: the first frame after the helper ran whose tree area is no
#    longer dimmed by the progress dialog.
def brightness(n):
    im = Image.open(os.path.join(src, f"f{n:04d}.png")).convert("L")
    w, h = im.size
    box = im.crop((int(0.02 * w), int(0.5 * h), int(0.55 * w), int(0.7 * h)))
    return ImageStat.Stat(box).mean[0]


bright = brightness(rows[0][0])
done = next((k for k in range(first_helper + 1, len(rows)) if brightness(rows[k][0]) > 0.92 * bright), len(rows) - 1)
for k in range(first_helper, done + 1):
    n, ms, _ = rows[k]
    t = (ms - t0) / 1000
    if k == done:
        frames.append(label(load(n), f"on screen after {t:.1f} s (real time)"))
        durations.append(4000)
    else:
        frames.append(label(load(n), f"real time  {t:.1f} s"))
        durations.append(max(20, rows[k + 1][1] - ms))

pal = [f.convert("P", palette=Image.ADAPTIVE, colors=160) for f in frames]
pal[0].save(out, save_all=True, append_images=pal[1:], duration=durations, loop=0, optimize=True, disposal=1)
print(f"result on screen {(rows[done][1] - t0) / 1000:.1f} s after the helper started")
print(f"{out}: {len(frames)} frames, {sum(durations) / 1000:.1f} s, {os.path.getsize(out) / 1048576:.1f} MB")
