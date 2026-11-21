#!/usr/bin/env python3
"""Verify the committed Phase 4 demo component against a recorded digest.

The component is committed so ordinary builds do not need a wasm target or
``wasm-tools``. Release verification still needs evidence that those bytes are
the bytes produced by ``tools/build_t3_guest.sh`` from ``guest/`` and
``wit/meld.wit``. This checker always verifies the recorded digest of the
committed artifact. When the optional toolchain is available it also rebuilds
the component in a temporary directory and compares the result byte-for-byte;
it never mutates the checked-in artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

RECORDED_DIGEST = "1f4ad3f28b77075cded9a3c0dcf3bca5cd91a7595a57da3079bfd4104e7f30e1"


class ToolchainUnavailable(RuntimeError):
    """The optional wasm target is not installed, so no rebuild can be compared."""

ROOT = Path(__file__).resolve().parent.parent
COMPONENT = ROOT / "crates" / "meld-t3" / "component" / "demo.component.wasm"
GUEST = ROOT / "crates" / "meld-t3" / "guest"
WIT = ROOT / "crates" / "meld-t3" / "wit"
BUILD_SCRIPT = ROOT / "tools" / "build_t3_guest.sh"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def rebuild_in_temp_dir() -> str:
    """Rebuild the guest using the repository script without touching the tree."""
    with tempfile.TemporaryDirectory(prefix="meld-t3-guest-") as tmp:
        root = Path(tmp)
        (root / "tools").mkdir()
        (root / "crates" / "meld-t3").mkdir(parents=True)
        shutil.copy2(BUILD_SCRIPT, root / "tools" / BUILD_SCRIPT.name)
        shutil.copytree(GUEST, root / "crates" / "meld-t3" / "guest")
        shutil.copytree(WIT, root / "crates" / "meld-t3" / "wit")
        (root / "crates" / "meld-t3" / "component").mkdir()
        result = subprocess.run(
            ["sh", str(root / "tools" / BUILD_SCRIPT.name)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            output = (result.stdout + result.stderr).strip()
            if (
                "wasm32-unknown-unknown target may not be installed" in output
                or "can't find crate for `core`" in output
            ):
                raise ToolchainUnavailable(output)
            raise subprocess.CalledProcessError(result.returncode, result.args, output)
        rebuilt = root / "crates" / "meld-t3" / "component" / "demo.component.wasm"
        if not rebuilt.is_file():
            raise RuntimeError("rebuild did not produce demo.component.wasm")
        # Compute the digest before the temporary directory is removed.
        return sha256(rebuilt)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail on digest drift (and on rebuild drift when the toolchain is present)",
    )
    args = parser.parse_args()

    if not COMPONENT.is_file():
        print(f"missing committed component: {COMPONENT}", file=sys.stderr)
        return 1

    actual = sha256(COMPONENT)
    if actual != RECORDED_DIGEST:
        print(
            f"T3 guest digest mismatch: recorded {RECORDED_DIGEST}, got {actual}",
            file=sys.stderr,
        )
        return 1

    probe = subprocess.run(
        ["sh", str(BUILD_SCRIPT), "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    probe_output = (probe.stdout + probe.stderr).strip()
    if probe.returncode != 0:
        print(f"T3 guest toolchain probe failed: {probe_output}", file=sys.stderr)
        return 1
    if "READY" not in probe_output:
        print(f"T3 guest digest OK ({actual}); rebuild SKIP: {probe_output}")
        return 0

    try:
        rebuilt_digest = rebuild_in_temp_dir()
    except ToolchainUnavailable as exc:
        print(f"T3 guest digest OK ({actual}); rebuild SKIP: {exc}")
        return 0
    except (OSError, subprocess.CalledProcessError, RuntimeError) as exc:
        print(f"T3 guest rebuild failed: {exc}", file=sys.stderr)
        return 1

    if rebuilt_digest != actual:
        print(
            f"T3 guest rebuild drift: committed {actual}, rebuilt {rebuilt_digest}",
            file=sys.stderr,
        )
        return 1

    print(f"T3 guest digest OK ({actual}); rebuild matches")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
