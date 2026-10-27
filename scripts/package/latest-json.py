#!/usr/bin/env python3
"""Prints latest.json for a release: the version and, per kind of install,
the file PilotHouse's updater downloads, with its SHA-256 and size.

usage: latest-json.py VERSION TAG OWNER/REPO DIST_FOLDER
"""
import hashlib
import json
import os
import sys

FILES = {
    "macos-universal": "PilotHouse-macOS-universal.zip",
    "windows-x64-setup": "PilotHouse-Windows-Setup-x64.exe",
    "windows-x64-portable": "PilotHouse-Windows-Portable-x64.zip",
    "linux-x86_64-appimage": "PilotHouse-Linux-x86_64.AppImage",
    "linux-x86_64-tarball": "PilotHouse-Linux-x86_64.tar.gz",
}


def main(version, tag, repo, dist):
    platforms = {}
    for key, name in FILES.items():
        path = os.path.join(dist, name)
        if not os.path.isfile(path):
            continue
        digest = hashlib.sha256()
        with open(path, "rb") as file:
            for chunk in iter(lambda: file.read(1 << 20), b""):
                digest.update(chunk)
        platforms[key] = {
            "url": f"https://github.com/{repo}/releases/download/{tag}/{name}",
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
