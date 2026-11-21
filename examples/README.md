# Examples

Run every command from the repository root after `cargo build --workspace`.
Commands use the source-build path; replace `./target/debug/meld` with `meld`
when the `meld-lang` package is installed.

| File | Feature | Command | Expected output |
| --- | --- | --- | --- |
| [01-hello.meld](01-hello.meld) | Block switching plus an inline Python island | `./target/debug/meld run examples/01-hello.meld` | `hello from python`, `hello from javascript: world`, `5` |
| [02-bridge.meld](02-bridge.meld) | Explicit Python → JavaScript value export/import | `./target/debug/meld run examples/02-bridge.meld` | `2,4,6` |
| [03-fenced.meld](03-fenced.meld) | Fenced regions, raw hash delimiters and auto-detected tail | `./target/debug/meld run examples/03-fenced.meld` | `a } inside a string is fine`, `literal brace: }`, `default python region` |
| [04-highlight.meld](04-highlight.meld) | Mixed-language highlighting across 15 fenced regions | `./target/debug/meld check examples/04-highlight.meld` | No stdout; check succeeds and regions are highlighted |
| [05-t2-run.meld](05-t2-run.meld) | T2 stdout merged from Python and JavaScript in file order | `./target/debug/meld run examples/05-t2-run.meld` | `python region one`, `python region two`, `javascript region one`, `javascript region two` |
| [06-bridge.meld](06-bridge.meld) | Typed values crossing Python → JavaScript → Lua | `./target/debug/meld run examples/06-bridge.meld` | `js doubled: 2,4,6`, `lua received 3 values` |
| [07-t3-bridge.meld](07-t3-bridge.meld) | Python calls typed Wasm Component functions via `meld_call` | `./target/debug/meld run examples/07-t3-bridge.meld` | `add(20, 22) = 42`, `mul(6, 7) = 42` |
| [08-shell-pack.meld](08-shell-pack.meld) | Language supplied by a third-party pack | `MELD_PACKS=tests/fixtures/pack-third-party ./target/debug/meld run examples/08-shell-pack.meld` | `hello from a third-party pack` |
| [09-smoke.meld](09-smoke.meld) | Front-end smoke test plus a three-language bridge | `./target/debug/meld run examples/09-smoke.meld` | `inline sum: 5`, `raw: ok`, `fence: ok`, `js: 20,40,60`, `lua: 3 values` |
| [10-more-languages.meld](10-more-languages.meld) | 15 strict language packs with an executed Bash region | `./target/debug/meld run examples/10-more-languages.meld` | `bash-ok` |

Use `./target/debug/meld check <file>` before `run` to inspect parse results
without executing regions. See [spec/syntax.md](../spec/syntax.md) for the
language forms and [spec/libraries.md](../spec/libraries.md) for pack
installation and verification.
