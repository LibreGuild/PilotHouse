#!/usr/bin/env python3
"""Makes the app's icons in brand/icons/ from one square picture.

    python3 scripts/package/icon.py PICTURE.png [--mac MAC.png] [--title-bar MARK.png] [--out DIR]

PICTURE is the icon, square, at least 1024 px (a PNG with transparency
where it should show the desktop). It makes:

    app-1024.png   Linux packages
    app-256.png    in the app (welcome, chat, About)
    title-bar.png  the title bar's mark, 80 px (drawn at 20 px)
    app.ico        Windows (the program and the installer)
    app.icns       macOS (needs iconutil: run it on a Mac; elsewhere the old
                   one is left as it is)

--mac takes a picture drawn on Apple's icon grid (a rounded tile with a
margin) for macOS instead; --title-bar a picture for the title bar (often
the tile edge to edge, which reads better small). --out writes elsewhere
than brand/icons (to try it). app.svg and app-tile.svg are PilotHouse's
drawings, kept for reference; the app does not use them.

Needs Pillow (`pip install pillow`).
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile

try:
    from PIL import Image
except ImportError:
    sys.exit("Pillow is missing: pip install pillow")

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")


def square(path, size):
    """`path` as an RGBA square of `size` px; refuses a picture that is not square."""
    image = Image.open(path).convert("RGBA")
    if image.width != image.height:
        sys.exit(f"{path} is {image.width}x{image.height}: the icon must be square")
    if image.width < size:
        print(f"note: {path} is {image.width} px; {size} px or more looks sharper", file=sys.stderr)
    return image.resize((size, size), Image.Resampling.LANCZOS)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("picture")
    parser.add_argument("--mac")
    parser.add_argument("--title-bar")
    parser.add_argument("--out", default=os.path.join(ROOT, "brand", "icons"))
    args = parser.parse_args()

    os.makedirs(args.out, exist_ok=True)
    full = square(args.picture, 1024)
    mac = square(args.mac, 1024) if args.mac else full
    mark = square(args.title_bar, 1024) if args.title_bar else full

    full.save(os.path.join(args.out, "app-1024.png"))
    full.resize((256, 256), Image.Resampling.LANCZOS).save(os.path.join(args.out, "app-256.png"))
    mark.resize((80, 80), Image.Resampling.LANCZOS).save(os.path.join(args.out, "title-bar.png"))
    full.save(
        os.path.join(args.out, "app.ico"),
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    if shutil.which("iconutil"):
        with tempfile.TemporaryDirectory() as tmp:
            iconset = os.path.join(tmp, "app.iconset")
            os.mkdir(iconset)
            for size in (16, 32, 128, 256, 512):
                for scale in (1, 2):
                    name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
                    mac.resize((size * scale, size * scale), Image.Resampling.LANCZOS).save(os.path.join(iconset, name))
            subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(args.out, "app.icns")], check=True)
    else:
        print("iconutil not found (macOS only): app.icns left as it was", file=sys.stderr)
    print(f"icons written to {os.path.normpath(args.out)}")


if __name__ == "__main__":
    main()
