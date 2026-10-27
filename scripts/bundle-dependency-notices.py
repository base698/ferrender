#!/usr/bin/env python3
"""Verify and bundle pinned notices for the added mesh solver dependencies.

All text is checked in, copied from checksum-verified crates or the recorded
exact upstream commit. Packaging never downloads license files.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def bundle(root=ROOT, destination=None):
    source = root / "assets/dependency-notices"
    manifest = json.loads((source / "manifest.json").read_text())
    locked = {(p["name"], p["version"]): p for p in tomllib.loads((root / "Cargo.lock").read_text())["package"]}
    if manifest.get("format") != 1:
        raise ValueError("Unrecognized dependency notice manifest")
    files = []
    for package in manifest["packages"]:
        key = package["name"], package["version"]
        entry = locked.get(key)
        if entry is None or entry.get("checksum") != package["crate_sha256"]:
            raise ValueError(f"Dependency notices differ from Cargo.lock: {key[0]} {key[1]}; review and update the notices")
        if not package["files"]:
            raise ValueError(f"No notice files for {key[0]} {key[1]}")
        for notice in package["files"]:
            relative = Path(notice["path"])
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError("Unexpected dependency notice path")
            path = source / relative
            if path.is_symlink() or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != notice["sha256"]:
                raise ValueError(f"Missing or changed dependency notice: {relative}")
            files.append((path, relative))
    # Validate everything before creating a partial license bundle.
    if destination is not None:
        destination = Path(destination)
        destination.mkdir(parents=True, exist_ok=True)
        for path, relative in files:
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
        shutil.copyfile(source / "manifest.json", destination / "manifest.json")
    return len(manifest["packages"]), len(files)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    packages, files = bundle(destination=args.output)
    print(f"Verified {files} notice files for {packages} locked dependencies")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError) as error:
        raise SystemExit(f"Dependency notice verification failed: {error}")
