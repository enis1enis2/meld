#!/usr/bin/env python3
"""Validate a release archive's entries, then extract it.

Release verification already rejects unsafe archive names, but the install
smoke check must not call ``extractall`` before performing the same validation.
This helper is the single extraction path used by CI and local install checks.
"""

from __future__ import annotations

import argparse
import pathlib
import stat
import sys
import tarfile
import zipfile


def fail(message: str) -> None:
    raise SystemExit(f"extract_release_archive.py: {message}")


def validate(name: str, where: str) -> None:
    if not name or "\\" in name or name.startswith("/"):
        fail(f"{where}: unsafe archive entry {name!r}")
    parts = pathlib.PurePosixPath(name).parts
    if any(part in ("", ".", "..") for part in parts):
        fail(f"{where}: unsafe archive entry {name!r}")


def extract(archive: pathlib.Path, destination: pathlib.Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    if archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as tf:
            members = tf.getmembers()
            for member in members:
                validate(member.name, archive.name)
                if member.issym() or member.islnk():
                    validate(member.linkname, archive.name)
            for member in members:
                target = (destination / member.name).resolve()
                if not target.is_relative_to(destination):
                    fail(f"{archive.name}: entry escapes destination ({member.name})")
            tf.extractall(destination, members)
        return

    with zipfile.ZipFile(archive) as zf:
        for name in zf.namelist():
            validate(name, archive.name)
            if not name.endswith("/"):
                mode = zf.getinfo(name).external_attr >> 16
                if stat.S_IFMT(mode) == stat.S_IFLNK:
                    fail(f"{archive.name}: symlink entries are not allowed ({name})")
            target = (destination / name).resolve()
            if not target.is_relative_to(destination):
                fail(f"{archive.name}: entry escapes destination ({name})")
        zf.extractall(destination)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=pathlib.Path)
    parser.add_argument("destination", type=pathlib.Path)
    args = parser.parse_args()
    if not args.archive.is_file():
        fail(f"missing archive: {args.archive}")
    extract(args.archive, args.destination.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
