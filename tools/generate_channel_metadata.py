#!/usr/bin/env python3
"""Fill Homebrew/Scoop checksums from a signed release SHA256SUMS.

The tool never invents a digest: it only copies checksums that already exist in
``SHA256SUMS`` for the exact requested stable version. Run it after the signed
release artifacts exist, then verify the result with ``--check``.
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from typing import NoReturn

ROOT = pathlib.Path(__file__).resolve().parent.parent
PLACEHOLDER = "TODO_VERIFY"
SHA256_RE = re.compile(r"[0-9a-fA-F]{64}\Z")
SUMS_LINE_RE = re.compile(r"([0-9a-fA-F]{64})\s+\*?(\S+)\s*\Z")
SEMVER_STABLE_RE = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+\Z")

FORMULA_BEGIN = "# BEGIN MANAGED CHECKSUMS (tools/generate_channel_metadata.py)"
FORMULA_END = "# END MANAGED CHECKSUMS"
FORMULA_VERSION_RE = re.compile(r'^\s*version\s+"([^"]+)"', re.MULTILINE)
FORMULA_ENTRY_RE = re.compile(r'^(\s*"{target}"\s*=>\s*")([^"]+)(".*)$', re.MULTILINE)

# target triple -> archive suffix used by the release packager
CHANNEL_TARGETS = {
    "aarch64-apple-darwin": "tar.gz",
    "x86_64-apple-darwin": "tar.gz",
    "x86_64-pc-windows-msvc": "zip",
}
FORMULA_TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin")
BUCKET_TARGET = "x86_64-pc-windows-msvc"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"generate_channel_metadata.py: {message}")


def load_checksums(sums_path: pathlib.Path, version: str) -> dict[str, str]:
    if not sums_path.is_file():
        fail(f"missing SHA256SUMS: {sums_path}")
    seen: dict[str, str] = {}
    for line in sums_path.read_text(encoding="utf-8").splitlines():
        match = SUMS_LINE_RE.match(line.strip())
        if not match:
            continue
        digest, raw_name = match.groups()
        name = raw_name.replace("\\", "/").rsplit("/", 1)[-1]
        if name in seen and seen[name] != digest.lower():
            fail(f"conflicting checksums for {name} in {sums_path}")
        seen[name] = digest.lower()

    digests: dict[str, str] = {}
    missing: list[str] = []
    for target, suffix in CHANNEL_TARGETS.items():
        artifact = f"meld-lang-{version}-{target}.{suffix}"
        digest = seen.get(artifact, "")
        if not SHA256_RE.fullmatch(digest):
            missing.append(artifact)
        else:
            digests[target] = digest
    if missing:
        fail("SHA256SUMS has no usable checksum for: " + ", ".join(missing)
             + f" (version {version}; generate signed artifacts first)")
    return digests


def check_versions(formula: str, bucket_text: str, version: str) -> dict:
    match = FORMULA_VERSION_RE.search(formula)
    if not match or match.group(1) != version:
        fail(f"Formula/meld-lang.rb version must be {version} before updating hashes")
    try:
        bucket = json.loads(bucket_text)
    except json.JSONDecodeError as exc:
        fail(f"bucket/meld-lang.json: invalid JSON ({exc})")
    if bucket.get("version") != version:
        fail(f"bucket/meld-lang.json version must be {version} before updating hashes")
    return bucket


def rewrite_formula(formula: str, digests: dict[str, str]) -> tuple[str, list[str]]:
    start = formula.find(FORMULA_BEGIN)
    end = formula.find(FORMULA_END)
    if start < 0 or end < 0 or end <= start:
        fail("Formula/meld-lang.rb: managed checksum block is missing or malformed")
    block = formula[start:end]
    found: dict[str, str] = {}
    for target, current in re.findall(r'^\s*"([a-z0-9_-]+)"\s*=>\s*"([^"]+)"', block, re.MULTILINE):
        found[target] = current
    if set(found) != set(FORMULA_TARGETS):
        fail(f"Formula/meld-lang.rb: managed checksum entries must be exactly {list(FORMULA_TARGETS)}")

    problems: list[str] = []
    new_block = block
    for target in FORMULA_TARGETS:
        pattern = FORMULA_ENTRY_RE.pattern.format(target=re.escape(target))
        matches = list(re.finditer(pattern, new_block, re.MULTILINE))
        if len(matches) != 1:
            fail(f"Formula/meld-lang.rb: expected one checksum entry for {target}")
        match = matches[0]
        expected = digests[target]
        if match.group(2) != expected:
            problems.append(f"Formula/meld-lang.rb {target}: {match.group(2)} != SHA256SUMS {expected}")
        new_block = new_block[:match.start(2)] + expected + new_block[match.end(2):]
    if PLACEHOLDER in block:
        problems.append("Formula/meld-lang.rb: checksum placeholder must be filled from the signed release")
    return formula[:start] + new_block + formula[end:], problems


def rewrite_bucket(bucket: dict, bucket_text: str, digests: dict[str, str]) -> tuple[str, list[str]]:
    target = BUCKET_TARGET
    architecture = bucket.get("architecture")
    if not isinstance(architecture, dict) or not isinstance(architecture.get("64bit"), dict):
        fail("bucket/meld-lang.json: missing architecture.64bit")
    slot = architecture["64bit"]
    if "hash" not in slot:
        fail("bucket/meld-lang.json: missing architecture.64bit.hash")
    expected = digests[target]
    problems: list[str] = []
    if slot["hash"] != expected:
        problems.append(f"bucket/meld-lang.json {target}: {slot['hash']} != SHA256SUMS {expected}")
    if PLACEHOLDER in bucket_text:
        problems.append("bucket/meld-lang.json: checksum placeholder must be filled from the signed release")
    slot["hash"] = expected
    return json.dumps(bucket, indent=2, ensure_ascii=False) + "\n", problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist-dir", type=pathlib.Path, required=True,
                        help="release distribution directory containing SHA256SUMS")
    parser.add_argument("--version", required=True, help="stable version, e.g. 0.2.0")
    parser.add_argument("--check", action="store_true",
                        help="verify checked-in hashes instead of rewriting them")
    args = parser.parse_args()

    if not SEMVER_STABLE_RE.fullmatch(args.version):
        fail("--version must be a stable MAJOR.MINOR.PATCH version (no pre-release suffix)")

    digests = load_checksums(args.dist_dir.resolve() / "SHA256SUMS", args.version)

    formula_path = ROOT / "Formula/meld-lang.rb"
    bucket_path = ROOT / "bucket/meld-lang.json"
    formula = formula_path.read_text(encoding="utf-8")
    bucket_text = bucket_path.read_text(encoding="utf-8")
    bucket = check_versions(formula, bucket_text, args.version)

    new_formula, formula_problems = rewrite_formula(formula, digests)
    new_bucket, bucket_problems = rewrite_bucket(bucket, bucket_text, digests)
    problems = formula_problems + bucket_problems

    if args.check:
        for problem in problems:
            print(f"generate_channel_metadata.py: STALE {problem}", file=sys.stderr)
        if problems:
            print(f"generate_channel_metadata.py: FAIL ({len(problems)} problem(s))", file=sys.stderr)
            return 1
        print("generate_channel_metadata.py: PASS (channel hashes match SHA256SUMS)")
        return 0

    if new_formula != formula:
        formula_path.write_text(new_formula, encoding="utf-8")
    if new_bucket != bucket_text:
        bucket_path.write_text(new_bucket, encoding="utf-8")
    print(f"generate_channel_metadata.py: wrote channel hashes for {args.version} from {args.dist_dir / 'SHA256SUMS'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
