#!/usr/bin/env python3
"""Verify the pinned official OCCT archive before making it available to cadrum.

Requires Python with tarfile.data_filter (Python 3.12+, or a security-backported
older Python). Raw cargo builds do not run this helper automatically; release
builds refuse to compile until OCCT_ROOT names the directory it prepared.
Digests: https://api.github.com/repos/lzpel/cadrum/releases/tags/occt-8_0_1_rev2
Recorded from GitHub's official release asset digest metadata on 2026-10-08.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
# Shared with crates/fr-core/build.rs, which records in the executable whether
# cadrum linked the directory this helper prepared.
PINS = json.loads((ROOT / "scripts" / "occt-pins.json").read_text())
TAG = PINS["tag"]
ASSETS = {target: (asset["size"], asset["sha256"]) for target, asset in PINS["assets"].items()}
MAX_EXPANDED_BYTES = 1024 * 1024 * 1024
MAX_ENTRIES = 50000


def verify(path, size, digest):
    if path.stat().st_size != size:
        raise ValueError("OCCT archive size does not match the pinned official asset")
    actual = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            actual.update(chunk)
    if actual.hexdigest() != digest:
        raise ValueError("OCCT archive SHA256 does not match the pinned official asset")


def download(url, destination, size, digest):
    with tempfile.NamedTemporaryFile(dir=destination.parent, suffix=".download", delete=False) as temporary:
        temporary_path = Path(temporary.name)
        try:
            # macOS ships curl with its system certificate integration; a
            # standalone Python installation may not have a CA bundle configured.
            # Keep HTTPS and redirect verification enabled, then authenticate the
            # bytes independently against the pinned SHA256.
            temporary.close()
            subprocess.run(["curl", "--fail", "--location", "--silent", "--show-error",
                            "--proto", "=https", "--proto-redir", "=https",
                            "--max-time", "180", "--max-filesize", str(size),
                            "--output", str(temporary_path), url], check=True)
            verify(temporary_path, size, digest)
            temporary.close()
            temporary_path.replace(destination)
        finally:
            temporary_path.unlink(missing_ok=True)


def validate_root(root):
    for required in ["include/opencascade/Standard.hxx", "lib/libTKBRep.a", "share/doc/opencascade/LICENSE_LGPL_21.txt", "share/doc/opencascade/OCCT_LGPL_EXCEPTION.txt"]:
        if not (root / required).is_file():
            raise ValueError(f"Verified OCCT archive is missing {required}")


def prepare_from_archive(archive, size, digest, top_name, destination):
    # Always verify the archive, even when the extracted cache already exists.
    verify(archive, size, digest)
    marker = destination / ".ferrender-verified.json"
    expected = {"sha256": digest, "size": size, "archive": archive.name}
    if destination.exists():
        if not marker.is_file() or json.loads(marker.read_text()) != expected:
            raise ValueError(f"Refusing an unverified existing OCCT directory: {destination}")
        validate_root(destination)
        return destination
    if not hasattr(tarfile, "data_filter"):
        raise ValueError("Safe OCCT extraction requires Python 3.12+ or a tarfile.data_filter security backport")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent, prefix=".occt-extract-") as temporary:
        with tarfile.open(archive, "r:gz") as source:
            members = source.getmembers()
            if len(members) > MAX_ENTRIES or sum(m.size for m in members) > MAX_EXPANDED_BYTES:
                raise ValueError("OCCT archive exceeds extraction limits")
            for member in members:
                name = PurePosixPath(member.name)
                if name.is_absolute() or ".." in name.parts or "\\" in member.name or not name.parts or name.parts[0] != top_name:
                    raise ValueError(f"Unexpected OCCT archive path: {member.name}")
            source.extractall(temporary, members=members, filter="data")
        extracted = Path(temporary) / top_name
        validate_root(extracted)
        (extracted / marker.name).write_text(json.dumps(expected, sort_keys=True) + "\n")
        extracted.rename(destination)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=ASSETS)
    parser.add_argument("--github-env", action="store_true", help="Append OCCT_ROOT for following GitHub Actions steps")
    args = parser.parse_args()
    target = args.target
    if target is None:
        version = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
    if target not in ASSETS:
        raise ValueError(f"No pinned OCCT asset for {target}; use a supported release target or an explicitly managed source build")
    size, digest = ASSETS[target]
    top_name = f"{TAG}-{target.replace('-', '_')}"
    cache = ROOT / "target" / "verified-occt"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"{top_name}.tar.gz"
    if not archive.exists():
        download(f"https://github.com/lzpel/cadrum/releases/download/{TAG}/{archive.name}", archive, size, digest)
    prepared = prepare_from_archive(archive, size, digest, top_name, cache / f"{top_name}-{digest[:16]}").resolve()
    if args.github_env:
        with open(os.environ["GITHUB_ENV"], "a") as environment:
            environment.write(f"OCCT_ROOT={prepared}\n")
    print(prepared)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, tarfile.TarError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"OCCT verification failed: {error}")
