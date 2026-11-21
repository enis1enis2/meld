#!/usr/bin/env python3
"""Validate all persisted schemas used by Meld releases."""
from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
PLACEHOLDER = "TODO_VERIFY"


def check(condition: bool, message: str, errors: list[str]) -> None:
    if not condition:
        errors.append(message)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=ROOT)
    parser.add_argument("--release-ready", action="store_true",
                        help="fail on installer checksum placeholders")
    args = parser.parse_args()
    root = args.root.resolve()
    errors: list[str] = []
    warnings: list[str] = []

    def load_json(relative: str) -> object:
        path = root / relative
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:  # noqa: BLE001 - report the file and reason
            errors.append(f"{relative}: invalid JSON ({exc})")
            return {}

    def load_toml(relative: str) -> dict:
        path = root / relative
        try:
            return tomllib.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:  # noqa: BLE001
            errors.append(f"{relative}: invalid TOML ({exc})")
            return {}

    grammar_sources = load_json("grammar-sources.json")
    check(grammar_sources.get("schema") == "meld-grammar-sources/1", "grammar-sources.json: bad schema", errors)
    check(bool(grammar_sources.get("crates")), "grammar-sources.json: no grammar crates", errors)

    registry = load_json("packs/registry/index.json")
    check(registry.get("schema") == "meld-pack-registry/1", "packs/registry/index.json: bad schema", errors)
    check(isinstance(registry.get("packs"), list), "packs/registry/index.json: packs must be a list", errors)

    # Cargo.toml is the single source of truth for the release version.
    workspace = load_toml("Cargo.toml")
    expected_version = workspace.get("workspace", {}).get("package", {}).get("version")
    check(bool(expected_version), "Cargo.toml: missing workspace.package.version", errors)

    bucket = load_json("bucket/meld-lang.json")
    check(bucket.get("version") == expected_version,
          f"bucket/meld-lang.json: version must match Cargo.toml ({expected_version})", errors)
    check(bool(bucket.get("architecture", {}).get("64bit", {}).get("url")),
          "bucket/meld-lang.json: missing 64bit URL", errors)
    check(len(bucket.get("bin", [])) == 3, "bucket/meld-lang.json: expected three binaries", errors)
    check("autoupdate" not in bucket,
          "bucket/meld-lang.json: autoupdate must be disabled (no trusted hash verification)", errors)
    if PLACEHOLDER in json.dumps(bucket):
        message = "bucket/meld-lang.json: checksum placeholder must be filled from the signed release"
        (errors if args.release_ready else warnings).append(message)

    formula_path = root / "Formula/meld-lang.rb"
    try:
        formula = formula_path.read_text(encoding="utf-8")
    except Exception as exc:  # noqa: BLE001 - report the file and reason
        errors.append(f"Formula/meld-lang.rb: unreadable ({exc})")
    else:
        check(f'version "{expected_version}"' in formula,
              f"Formula/meld-lang.rb: version must match Cargo.toml ({expected_version})", errors)
        begin = "# BEGIN MANAGED CHECKSUMS (tools/generate_channel_metadata.py)"
        end = "# END MANAGED CHECKSUMS"
        has_managed_block = begin in formula and end in formula
        check(has_managed_block, "Formula/meld-lang.rb: managed checksum block is missing", errors)
        managed = formula.split(begin, 1)[1].split(end, 1)[0] if has_managed_block else formula
        if PLACEHOLDER in managed:
            message = "Formula/meld-lang.rb: checksum placeholder must be filled from the signed release"
            (errors if args.release_ready else warnings).append(message)

    for manifest in sorted((root / "packs").glob("*/pack.toml")):
        data = load_toml(str(manifest.relative_to(root)))
        check(bool(data.get("id")) and bool(data.get("version")), f"{manifest.name}: missing id/version", errors)
        ts = data.get("tree_sitter", {})
        if ts:
            check(bool(ts.get("crate")) and bool(ts.get("rev")), f"{manifest.name}: incomplete tree_sitter pin", errors)
    for manifest in sorted((root / "packs").glob("*/library.toml")):
        data = load_toml(str(manifest.relative_to(root)))
        check(data.get("api") == "meld-lang-library/1", f"{manifest.name}: bad library api", errors)
    for conformance in sorted((root / "packs").glob("*/tests/conformance.json")):
        data = load_json(str(conformance.relative_to(root)))
        check(bool(data.get("lang")) and bool(data.get("cases")), f"{conformance.name}: missing lang/cases", errors)

    for config in (".cargo/audit.toml", "about.toml", "third-party/grammars.toml",
                   "third-party/overrides.toml", "docs/book.toml"):
        data = load_toml(config)
        check(bool(data), f"{config}: empty configuration", errors)
    grammars = load_toml("third-party/grammars.toml")
    check(grammars.get("schema") == "meld-third-party-grammars/1", "third-party/grammars.toml: bad schema", errors)

    manifest_candidates = list(root.glob("*.release-manifest.json"))
    for manifest in manifest_candidates:
        data = load_json(str(manifest.relative_to(root)))
        check(data.get("schema") == "meld-release-manifest/1", f"{manifest.name}: bad schema", errors)
        check(data.get("semver_spec") == "2.0.0", f"{manifest.name}: not SemVer 2.0.0", errors)

    for warning in warnings:
        print(f"check_schemas.py: WARNING {warning}")
    for error in errors:
        print(f"check_schemas.py: {error}", file=sys.stderr)
    if errors:
        print(f"check_schemas.py: FAIL ({len(errors)} problem(s))", file=sys.stderr)
        return 1
    print("check_schemas.py: PASS (all persisted schemas valid)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
