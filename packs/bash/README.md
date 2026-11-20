# Bash pack (`T2`, `L1`)

Bash is the first executable language added in the 2026-11-20 expansion batch.
It reuses the protocol worker proven by the third-party `shell` fixture
(`tests/fixtures/pack-third-party`) but pins a real published grammar and is a
strict built-in pack.

## Why L1, not L2

The worker implements `initialize`, `execute`, and `shutdown`, and reports
ordered stdout and structured errors. It deliberately does **not** implement
the value bridge (`import`/`export`/`fetch`), so the manifest honestly declares
`level = "L1"` instead of claiming value round-trips it cannot provide.

## Judgment calls

- The POSIX `sh` worker is invoked with `sh`, not `bash`, so the pack also
  serves the `sh`/`shell`/`posix-sh` aliases. Bash-only syntax is still parsed
  by the pinned tree-sitter grammar; execution follows the declared aliases.
- JSON/base64 protocol lines are encoded with the Python 3 standard library
  inside `worker.sh`. Python 3 is already required by the Meld tooling; the
  executed region itself still runs in `sh -c`.
- `region_end.sh` is a scanning stub returning `-1`, so Meld uses the
  deterministic raw-hash delimiter fallback. Source examples therefore use
  `@bash#"..." "#` blocks rather than relying on brace balancing.
- No `formatter.toml` is shipped. `meld fmt` keeps the region body verbatim
  instead of guessing a formatter (spec/lang-pack.md §4.4).
