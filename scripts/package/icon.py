#!/usr/bin/env python3
"""Draws PilotHouse's icon: a magenta tile with "PH" in white (the letters
are Outfit ExtraBold, SIL Open Font License, as outlines below, so no font
is needed). Writes every size the packages use:

    crates/app/assets/icon/PilotHouse.svg       the drawing (macOS tile)
    crates/app/assets/icon/PilotHouse-1024.png  Linux, and the source of the rest
    crates/app/assets/icon/PilotHouse.icns      macOS (needs iconutil)
    crates/app/assets/icon/PilotHouse.ico       Windows
    crates/ui/assets/app-icon.png               the in-app mark

usage: scripts/package/icon.py   (needs Python 3 with Pillow)
"""
import os
import re
import shutil
import subprocess
import tempfile

from PIL import Image, ImageChops, ImageDraw

# The brand colour (also the app's accent): signal magenta, lighter at the
# top right.
LIGHT = (255, 79, 163)
DEEP = (176, 18, 98)
# "PH", centred on a 1024 square.
LETTERS = "M315.2 554.7V498.5H374.7Q385.8 498.5 394.9 493.9Q404 489.4 409.5 480.4Q415 471.4 415 458.4Q415 445.3 409.5 436.3Q404 427.3 394.9 422.8Q385.8 418.3 374.7 418.3H315.2V362H387.7Q416.6 362 439.8 373.6Q462.9 385.2 476.5 406.7Q490 428.2 490 458.3Q490 488.2 476.5 509.9Q462.9 531.6 439.8 543.2Q416.6 554.7 387.7 554.7ZM255.5 662V362H331.1V662ZM509.4 662V362H585V662ZM692.9 662V362H768.5V662ZM552.3 540.3V476.6H720.3V540.3Z"

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
SS = 4  # drawn at 4x, then scaled down


def contours(path):
    """The outlines of an SVG path of M, L, H, V, Q and Z commands, each as a
    list of points (curves flattened)."""
    tokens = re.findall(r"[MLHVQZ]|-?\d+(?:\.\d+)?", path)
    shapes, points, i, x, y, cmd = [], [], 0, 0.0, 0.0, None
    while i < len(tokens):
        if tokens[i] in "MLHVQZ":
            cmd = tokens[i]
            i += 1
            if cmd == "Z":
                if points:
                    shapes.append(points)
                points = []
                continue
        if cmd == "M":
            if points:
                shapes.append(points)
            x, y = float(tokens[i]), float(tokens[i + 1])
            points = [(x, y)]
            i += 2
            cmd = "L"
        elif cmd == "L":
            x, y = float(tokens[i]), float(tokens[i + 1])
            points.append((x, y))
            i += 2
        elif cmd == "H":
            x = float(tokens[i])
            points.append((x, y))
            i += 1
        elif cmd == "V":
            y = float(tokens[i])
            points.append((x, y))
            i += 1
        elif cmd == "Q":
            cx, cy = float(tokens[i]), float(tokens[i + 1])
            ex, ey = float(tokens[i + 2]), float(tokens[i + 3])
            for step in range(1, 13):
                t = step / 12
                points.append((
                    (1 - t) ** 2 * x + 2 * (1 - t) * t * cx + t * t * ex,
                    (1 - t) ** 2 * y + 2 * (1 - t) * t * cy + t * t * ey,
                ))
            x, y = ex, ey
            i += 4
    if points:
        shapes.append(points)
    return shapes


def icon(inset, radius):
    """The icon on a tile `inset` from the edges with corner `radius`
    (1024 units), RGBA, 1024 square."""
    n = 1024 * SS
    tile = Image.new("L", (n, n), 0)
    ImageDraw.Draw(tile).rounded_rectangle(
        [inset * SS, inset * SS, (1024 - inset) * SS, (1024 - inset) * SS],
        radius=radius * SS, fill=255)
    # Diagonal: light at the top right, deep at the bottom left.
    across = Image.linear_gradient("L").rotate(90).resize((n, n))
    down = Image.linear_gradient("L").resize((n, n))
    shade = ImageChops.add(across, down, scale=2)
    fill = Image.composite(Image.new("RGB", (n, n), DEEP), Image.new("RGB", (n, n), LIGHT), shade)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    out.paste(fill, (0, 0), tile)
    # Outlines turn one way, the holes in them (the bowl of the P) the other;
    # outlines may overlap. Letters = every outline, less every hole.
    solid = Image.new("1", (n, n), 0)
    holes = Image.new("1", (n, n), 0)
    for shape in contours(LETTERS):
        one = Image.new("1", (n, n), 0)
        ImageDraw.Draw(one).polygon([(px * SS, py * SS) for px, py in shape], fill=1)
        area = sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(shape, shape[1:] + shape[:1]))
        if area < 0:
            holes = ImageChops.logical_or(holes, one)
        else:
            solid = ImageChops.logical_or(solid, one)
    letters = ImageChops.logical_and(solid, ImageChops.invert(holes.convert("L")).convert("1"))
    out.paste((255, 255, 255, 255), (0, 0), letters.convert("L"))
    return out.resize((1024, 1024), Image.LANCZOS)


def svg():
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <!-- PilotHouse: "PH" on a signal-magenta tile. The macOS tile; Windows,
       Linux and the in-app mark use the same drawing on a tile inset 24
       with corner radius 220. Drawn by scripts/package/icon.py. -->
  <defs>
    <linearGradient id="tile" x1="1" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#{LIGHT[0]:02x}{LIGHT[1]:02x}{LIGHT[2]:02x}"/>
      <stop offset="1" stop-color="#{DEEP[0]:02x}{DEEP[1]:02x}{DEEP[2]:02x}"/>
    </linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#tile)"/>
  <path fill="#ffffff" fill-rule="evenodd" d="{LETTERS}"/>
</svg>
"""


def main():
    icons = os.path.join(ROOT, "crates", "app", "assets", "icon")
    mac = icon(100, 185)
    full = icon(24, 220)
    with open(os.path.join(icons, "PilotHouse.svg"), "w") as file:
        file.write(svg())
    full.save(os.path.join(icons, "PilotHouse-1024.png"))
    full.resize((256, 256), Image.LANCZOS).save(
        os.path.join(ROOT, "crates", "ui", "assets", "app-icon.png"))
    full.save(os.path.join(icons, "PilotHouse.ico"),
              sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    if shutil.which("iconutil"):
        with tempfile.TemporaryDirectory() as tmp:
            iconset = os.path.join(tmp, "PilotHouse.iconset")
            os.mkdir(iconset)
            for size in (16, 32, 128, 256, 512):
                for scale in (1, 2):
                    name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                    mac.resize((size * scale, size * scale), Image.LANCZOS).save(
                        os.path.join(iconset, name))
            subprocess.run(["iconutil", "-c", "icns", iconset, "-o",
                            os.path.join(icons, "PilotHouse.icns")], check=True)
    else:
        print("iconutil not found (macOS only): PilotHouse.icns left as it was")


if __name__ == "__main__":
    main()
