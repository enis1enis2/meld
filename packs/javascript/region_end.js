// End-scanner reference for plain `@javascript { ... }` blocks.
// JUDGMENT CALL: the host uses the generic balanced-brace scanner until Wasm
// scanners are wired in Phase 4; this file records the pack-owned contract.
function findRegionEnd(text, start, marker = "}") {
  let depth = 1;
  let quote = null;
  let lineComment = false;
  let blockComment = false;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (lineComment) { if (c === "\n") lineComment = false; continue; }
    if (blockComment) {
      if (c === "*" && text[i + 1] === "/") { blockComment = false; i++; }
      continue;
    }
    if (quote) {
      if (c === "\\") { i++; continue; }
      if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") { quote = c; continue; }
    if (c === "/" && text[i + 1] === "/") { lineComment = true; i++; continue; }
    if (c === "/" && text[i + 1] === "*") { blockComment = true; i++; continue; }
    if (c === "{") depth++;
    else if (c === "}") { if (--depth === 0) return i; }
  }
  return -1;
}
module.exports = { findRegionEnd };
