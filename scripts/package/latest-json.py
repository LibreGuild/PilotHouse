#!/usr/bin/env python3
"""Prints latest.json for a release: the version and, per kind of install,
the file the app's updater downloads, with its SHA-256 and size.

Files are named by the brand's first update source (brand/brand.toml:
`files` of the first [[updates.sources]], `<files><tag>/<name>`), which is
the address the app accepts them from; their names start with the brand's
name without spaces (PilotHouse-macOS-universal.zip).

usage: latest-json.py VERSION TAG OWNER/REPO DIST_FOLDER
(OWNER/REPO is no longer used: the brand names where files are.)
"""
import hashlib
import json
import os
import sys
import tomllib

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")

KINDS = {
    "macos-universal": "{stem}-macOS-universal.zip",
    "windows-x64-setup": "{stem}-Windows-Setup-x64.exe",
    "windows-x64-portable": "{stem}-Windows-Portable-x64.zip",
    "linux-x86_64-appimage": "{stem}-Linux-x86_64.AppImage",
    "linux-x86_64-tarball": "{stem}-Linux-x86_64.tar.gz",
}


def brand():
    with open(os.path.join(ROOT, "brand", "brand.toml"), "rb") as file:
        return tomllib.load(file)


def main(version, tag, _repo, dist):
    config = brand()
    stem = config["app"]["name"].replace(" ", "")
    sources = config.get("updates", {}).get("sources", [])
    if not sources:
        sys.exit("brand/brand.toml lists no [[updates.sources]]")
    files = sources[0]["files"]
    platforms = {}
    for key, pattern in KINDS.items():
        name = pattern.format(stem=stem)
        path = os.path.join(dist, name)
        if not os.path.isfile(path):
            continue
        digest = hashlib.sha256()
        with open(path, "rb") as file:
            for chunk in iter(lambda: file.read(1 << 20), b""):
                digest.update(chunk)
        platforms[key] = {
            "url": f"{files}{tag}/{name}",
            "sha256": digest.hexdigest(),
            "size": os.path.getsize(path),
        }
    if not platforms:
        sys.exit("no release files found in " + dist)
    print(json.dumps({"version": version, "platforms": platforms}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    main(*sys.argv[1:])
