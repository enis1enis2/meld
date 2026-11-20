#!/usr/bin/env python3
"""CI-usable smoke checks for manual migration/generation tools.

All checks are read-only (``--check`` modes or pure function probes). Optional
checks that need a populated Cargo registry cache, vendored tree or missing
legacy lockfile report SKIP with a reason; pass ``--strict`` to turn any SKIP
into a failure for release jobs that require full coverage.
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / "tools"


def _summary(process: subprocess.CompletedProcess[str]) -> str:
    text = "\n".join(part for part in (process.stdout, process.stderr) if part.strip())
    return text.strip().splitlines()[-1] if text.strip() else "(no output)"


def run_python(name: str, script: str, *args: str,
               skip_reason: str | None = None) -> str:
    if skip_reason:
        print(f"SKIP {name}: {skip_reason}")
        return "skip"
    process = subprocess.run(
        [sys.executable, str(TOOLS / script), *args],
        cwd=ROOT, text=True, capture_output=True,
    )
    if process.returncode != 0:
        print(f"FAIL {name}: {_summary(process)}")
        for stream in (process.stdout, process.stderr):
            if stream.strip():
                print("  " + stream.strip().replace("\n", "\n  "))
        return "fail"
    print(f"PASS {name}: {_summary(process)}")
    return "pass"


def run_shell(name: str, script: str, *args: str) -> str:
    process = subprocess.run(
        ["sh", str(TOOLS / script), *args],
        cwd=ROOT, text=True, capture_output=True,
    )
    output = "\n".join(part for part in (process.stdout, process.stderr) if part.strip())
    if process.returncode != 0:
        print(f"FAIL {name}: {_summary(process)}")
        return "fail"
    if "SKIP" in output:
        print(f"SKIP {name}: {output.strip()}")
        return "skip"
    print(f"PASS {name}: {_summary(process)}")
    return "pass"


def registry_roots() -> list[Path]:
    home = Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home")) / "registry" / "src"
    hashed = [p for p in home.glob("index.crates.io-*") if p.is_dir()]
    return hashed or [home]


def missing_grammar_sources() -> list[str]:
    doc = json.loads((ROOT / "grammar-sources.json").read_text(encoding="utf-8"))
    roots = registry_roots()
    missing = []
    for entry in doc.get("crates", []):
        crate, version = entry.get("crate"), entry.get("version")
        if not crate or not version:
            continue
        if not any((base / f"{crate}-{version}").is_dir() for base in roots):
            missing.append(f"{crate}-{version}")
    return missing


def gen_pack_probe() -> str:
    sys.path.insert(0, str(TOOLS))
    import gen_pack  # noqa: PLC0415 - local import keeps startup side-effect free

    good = gen_pack.validate_grammar_rev("pinned:shell-v1", "tree-sitter-shell", ROOT)
    bad_refs = ("pinned:", "pinned:../escape", "main")
    rejected = []
    for ref in bad_refs:
        try:
            gen_pack.validate_grammar_rev(ref, "tree-sitter-shell", ROOT)
        except ValueError:
            rejected.append(ref)
    if good != "pinned:shell-v1" or len(rejected) != len(bad_refs):
        print(f"FAIL gen-pack pin validation: accepted={good!r}, rejected={rejected}")
        return "fail"
    print("PASS gen-pack pin validation: malformed refs rejected")
    return "pass"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--strict", action="store_true",
                        help="fail if an optional check is skipped")
    args = parser.parse_args()

    missing_sources = missing_grammar_sources()
    source_reason = None
    if missing_sources:
        preview = ", ".join(missing_sources[:3])
        suffix = "" if len(missing_sources) <= 3 else f", +{len(missing_sources) - 3} more"
        source_reason = f"local Cargo registry sources unavailable ({preview}{suffix})"

    schema_inputs = (
        ".cargo/audit.toml", "about.toml", "grammar-sources.json", "packs/registry/index.json",
        "bucket/meld-lang.json", "docs/book.toml", "third-party/grammars.toml",
        "third-party/overrides.toml",
    )
    missing_schema_inputs = [name for name in schema_inputs if not (ROOT / name).is_file()]
    schema_reason = (
        "required inputs missing: " + ", ".join(missing_schema_inputs)
        if missing_schema_inputs else None
    )
    lock_reason = "meld.lock not present in this checkout" if not (ROOT / "meld.lock").is_file() else None
    vendor_reason = (
        "third-party/vendor/vendor-manifest.json not present"
        if not (ROOT / "third-party" / "vendor" / "vendor-manifest.json").is_file() else None
    )

    results = [
        run_python("migrate-index", "migrate_index.py", "--check", "packs/registry/index.json"),
        run_python("migrate-lock", "migrate_lock.py", "--check", "meld.lock",
                   skip_reason=lock_reason),
        run_python("check-schemas", "check_schemas.py", "--root", ".",
                   skip_reason=schema_reason),
        run_python("check-grammar-sources", "check_grammar_sources.py", "--root", "."),
        run_python("generate-notices", "generate_third_party_notices.py", "--check",
                   skip_reason=source_reason),
        run_python("sync-grammars", "sync_grammars.py", "--check",
                   skip_reason=source_reason),
        run_python("vendor-grammars", "vendor_grammars.py", "--check",
                   skip_reason=source_reason or vendor_reason),
        run_python("meld-check-ref", "meld_check_ref.py"),
        run_python("highlight-ref", "highlight_ref.py"),
        run_shell("build-t3-guest", "build_t3_guest.sh", "--check"),
        gen_pack_probe(),
    ]
    failed = results.count("fail")
    skipped = results.count("skip")
    if failed or (args.strict and skipped):
        print(f"manual tool smoke: FAIL ({failed} failure(s), {skipped} skip(s))")
        return 1
    print(f"manual tool smoke: PASS ({len(results) - skipped} check(s), {skipped} skip(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
