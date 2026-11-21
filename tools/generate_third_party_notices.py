#!/usr/bin/env python3
"""Generate THIRD-PARTY-NOTICES.md from Cargo.lock and local registry metadata.

No license or version is guessed. License identifiers come from the downloaded
crate manifests in CARGO_HOME/registry/src; checksums come from Cargo.lock. If a
crate's license cannot be determined locally, generation fails with an explicit
TODO_VERIFY marker instead of publishing an invented attribution.
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
PLACEHOLDER = "TODO_VERIFY"


def registry_roots() -> list[pathlib.Path]:
    home = pathlib.Path(os.environ.get("CARGO_HOME", ROOT / ".cargo-home"))
    src = home / "registry" / "src"
    hashed = [p for p in src.glob("index.crates.io-*") if p.is_dir()]
    return hashed or [src]


def license_for(name: str, version: str, roots: list[pathlib.Path]) -> str:
    for root in roots:
        candidate = root / f"{name}-{version}" / "Cargo.toml"
        if candidate.is_file():
            data = tomllib.loads(candidate.read_text(encoding="utf-8"))
            package = data.get("package", {})
            license_id = package.get("license")
            if isinstance(license_id, str) and license_id.strip():
                return license_id.strip()
            if package.get("license-file"):
                return f"see {package['license-file']} in the crate archive"
    return f"{PLACEHOLDER} (license metadata unavailable locally)"


class SpdxExpressionError(ValueError):
    """Raised when a license string is not a supported SPDX expression."""


_TOKEN_RE = re.compile(
    r"\s*(?:(?P<op>AND|OR|WITH)\b|(?P<paren>[()])|(?P<slash>/)|"
    r"(?P<id>[A-Za-z0-9][A-Za-z0-9.-]*\+?))",
    flags=re.IGNORECASE,
)
_PRECEDENCE = {"OR": 1, "AND": 2}


def _tokenize_spdx(expression: str) -> list[tuple[str, str]]:
    tokens: list[tuple[str, str]] = []
    pos = 0
    while pos < len(expression):
        if expression[pos].isspace():
            pos += 1
            continue
        match = _TOKEN_RE.match(expression, pos)
        if not match:
            raise SpdxExpressionError(f"unexpected token near {expression[pos:pos + 16]!r}")
        if match.group("op"):
            # Normalize the legacy shorthand `A/B` to SPDX `A OR B`, but only
            # through the token stream so malformed slashes still fail closed.
            tokens.append(("OP", match.group("op").upper()))
        elif match.group("paren"):
            tokens.append(("PAREN", match.group("paren")))
        elif match.group("slash"):
            tokens.append(("OP", "OR"))
        else:
            tokens.append(("ID", match.group("id")))
        pos = match.end()
    return tokens


class _SpdxParser:
    def __init__(self, tokens: list[tuple[str, str]]):
        self.tokens = tokens
        self.pos = 0

    def peek(self) -> tuple[str, str] | None:
        return self.tokens[self.pos] if self.pos < len(self.tokens) else None

    def take(self) -> tuple[str, str]:
        token = self.peek()
        if token is None:
            raise SpdxExpressionError("unexpected end of license expression")
        self.pos += 1
        return token

    def expect_op(self, op: str) -> None:
        if self.take() != ("OP", op):
            raise SpdxExpressionError(f"expected {op} in license expression")

    def expect_id(self) -> str:
        kind, value = self.take()
        if kind != "ID":
            raise SpdxExpressionError("expected a license identifier")
        return value

    def expect_paren(self, value: str) -> None:
        if self.take() != ("PAREN", value):
            raise SpdxExpressionError(f"expected {value!r} in license expression")

    def parse(self) -> tuple:
        node = self.parse_or()
        if self.peek() is not None:
            raise SpdxExpressionError(f"unexpected trailing token {self.peek()!r}")
        return node

    def parse_or(self) -> tuple:
        node = self.parse_and()
        while self.peek() == ("OP", "OR"):
            self.take()
            node = ("OR", node, self.parse_and())
        return node

    def parse_and(self) -> tuple:
        node = self.parse_with()
        while self.peek() == ("OP", "AND"):
            self.take()
            node = ("AND", node, self.parse_with())
        return node

    def parse_with(self) -> tuple:
        node = self.parse_atom()
        if self.peek() == ("OP", "WITH"):
            self.take()
            if node[0] != "ID":
                raise SpdxExpressionError("WITH may only follow a single license identifier")
            return ("WITH", node[1], self.expect_id())
        return node

    def parse_atom(self) -> tuple:
        token = self.peek()
        if token == ("PAREN", "("):
            self.take()
            node = self.parse_or()
            self.expect_paren(")")
            return node
        if token is not None and token[0] == "ID":
            self.take()
            return ("ID", token[1])
        raise SpdxExpressionError(f"unexpected token {token!r} in license expression")


def normalize_spdx(expression: str) -> str:
    """Validate a supported SPDX subset and render it in canonical form."""
    if not isinstance(expression, str) or not expression.strip():
        raise SpdxExpressionError("empty license expression")
    node = _SpdxParser(_tokenize_spdx(expression.strip())).parse()
    return _render_spdx(node)


def _render_spdx(node: tuple, parent: str | None = None) -> str:
    if node[0] == "ID":
        return node[1]
    if node[0] == "WITH":
        return f"{node[1]} WITH {node[2]}"
    op = node[0]
    text = f"{_render_spdx(node[1], op)} {op} {_render_spdx(node[2], op)}"
    if parent is not None and _PRECEDENCE[op] < _PRECEDENCE[parent]:
        return f"({text})"
    return text


def _accepted_ids(node: tuple, accepted: set[str]) -> set[str]:
    """Return the accepted choices that satisfy each OR/AND branch."""
    if node[0] in ("ID", "WITH"):
        return {node[1]} if node[1] in accepted else set()
    if node[0] == "OR":
        return _accepted_ids(node[1], accepted) | _accepted_ids(node[2], accepted)
    left = _accepted_ids(node[1], accepted)
    right = _accepted_ids(node[2], accepted)
    # `A AND B` is acceptable only when *both* branches offer an accepted
    # license; the chosen licenses need not be identical.
    return left | right if left and right else set()


def licenses_ok(expression: str, accepted: set[str]) -> bool:
    """Every AND branch must offer at least one license from `about.toml`."""
    normalized = normalize_spdx(expression)
    if normalized in accepted:
        return True
    node = _SpdxParser(_tokenize_spdx(normalized)).parse()
    return bool(_accepted_ids(node, accepted))


def render(root: pathlib.Path) -> str:
    about = tomllib.loads((root / "about.toml").read_text(encoding="utf-8"))
    accepted = set(about["accepted"])
    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    roots = registry_roots()
    grammars = json.loads((root / "grammar-sources.json").read_text(encoding="utf-8"))

    lines = [
        "# Third-party notices",
        "",
        "Generated by `python3 tools/generate_third_party_notices.py`. Versions and",
        "SHA256 checksums come from `Cargo.lock`; license identifiers come from the",
        "crate manifests in the local Cargo registry cache. Do not edit by hand.",
        "",
        "Meld itself is dual-licensed `MIT OR Apache-2.0`; see `LICENSE-MIT`,",
        "`LICENSE-APACHE` and `NOTICE`.",
        "",
        "## Rust dependencies",
        "",
        "| Crate | Version | License | SHA256 (crates.io archive) |",
        "| --- | --- | --- | --- |",
    ]
    problems: list[str] = []
    rows = []
    for package in sorted(lock["package"], key=lambda p: (p["name"], p["version"])):
        name, version = package["name"], package["version"]
        source = package.get("source")
        checksum = package.get("checksum", "workspace crate (no registry checksum)")
        if source:
            license_id = license_for(name, version, roots)
            if license_id.startswith(PLACEHOLDER):
                problems.append(f"{name}@{version}: {license_id}")
            else:
                try:
                    license_id = normalize_spdx(license_id)
                except SpdxExpressionError as exc:
                    problems.append(f"{name}@{version}: invalid SPDX license {license_id!r} ({exc})")
                else:
                    if not licenses_ok(license_id, accepted):
                        problems.append(
                            f"{name}@{version}: license {license_id} is not in about.toml accepted list"
                        )
        else:
            license_id = normalize_spdx("MIT OR Apache-2.0")
        rows.append(f"| `{name}` | `{version}` | {license_id} | `{checksum}` |")
    lines.extend(rows)

    lines += [
        "",
        "## Pinned grammar sources",
        "",
        "Highlight/injection queries are copied from these exact upstream crate",
        "versions; see `grammar-sources.json` for per-query hashes and",
        "`third-party/overrides.toml` for substitutions and mechanical fixups.",
        "",
        "| Crate | Version | Packs | Cargo SHA256 |",
        "| --- | --- | --- | --- |",
    ]
    for entry in grammars["crates"]:
        lines.append(
            f'| `{entry["crate"]}` | `{entry["version"]}` | '
            f'{", ".join(entry["packs"])} | `{entry["cargo_checksum"]}` |'
        )
    lines += [
        "",
        "## Runtime expectations",
        "",
        "Meld does not install language runtimes. Lang Packs may reference an",
        "external interpreter through `runner.toml`; such a runtime is a user",
        "responsibility and is not part of this notice.",
        "",
    ]
    if problems:
        raise SystemExit(
            "generate_third_party_notices.py: unresolved license metadata:\n  "
            + "\n  ".join(problems)
            + "\nPopulate the Cargo registry cache or add a reviewed about.toml exception."
        )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, default=ROOT)
    parser.add_argument("--out", type=pathlib.Path, default=ROOT / "THIRD-PARTY-NOTICES.md")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    rendered = render(args.root.resolve())
    if args.check:
        if not args.out.is_file() or args.out.read_text(encoding="utf-8") != rendered:
            print(f"generate_third_party_notices.py: {args.out} is stale", file=sys.stderr)
            return 1
        print("generate_third_party_notices.py: PASS notices are current")
        return 0
    args.out.write_text(rendered, encoding="utf-8")
    print(f"generate_third_party_notices.py: wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
