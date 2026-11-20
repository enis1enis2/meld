//! Pack-owned `region_end.toml` metadata (Phase 6 GO decision).
//!
//! JUDGMENT CALL: the pack declares *which built-in algorithm* to use, plus a
//! fallback and a human-readable hint. The host selects an algorithm by name;
//! `region_end.py/js/lua/sh/wat` payloads are never loaded or executed. An
//! unknown schema, unknown kind or `kind = "wasm"` warns and uses the balanced
//! brace fallback, never pack code and never a Wasm runtime.
use crate::scanners::{
    counted::{CountedDialect, CountedScanner},
    delimited::{DelimitedDialect, DelimitedScanner},
    heredoc::{HeredocDialect, HeredocScanner},
    indent::{IndentDialect, IndentScanner},
    template::TemplateScanner,
};
use crate::splitter::{BalancedBraceScanner, EndScanner, ScannerRegistry};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScannerKind {
    Brace,
    Heredoc,
    Indent,
    Delimited,
    Counted,
    Template,
    Wasm,
    Unknown(String),
}

impl ScannerKind {
    pub fn from_id(id: &str) -> Self {
        match id {
            "brace" => Self::Brace,
            "heredoc" => Self::Heredoc,
            "indent" => Self::Indent,
            "delimited" => Self::Delimited,
            "counted" => Self::Counted,
            "template" => Self::Template,
            "wasm" => Self::Wasm,
            other => Self::Unknown(other.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RegionEndSpec {
    pub schema: String,
    pub kind: ScannerKind,
    pub fallback: String,
    pub hint: String,
    pub options: BTreeMap<String, String>,
}

impl Default for RegionEndSpec {
    fn default() -> Self {
        Self::brace()
    }
}

impl RegionEndSpec {
    pub const SCHEMA: &'static str = "meld-region-end/1";

    pub fn brace() -> Self {
        Self {
            schema: Self::SCHEMA.to_string(),
            kind: ScannerKind::Brace,
            fallback: "brace".to_string(),
            hint: String::new(),
            options: BTreeMap::new(),
        }
    }

    /// Parse metadata; malformed or unsupported profiles warn and fall back
    /// to the brace scanner instead of failing `check`, `run` or LSP startup.
    pub fn parse(text: &str) -> Self {
        let Ok(value) = toml::from_str::<toml::Value>(text) else {
            eprintln!("meld: warning: malformed region_end.toml; using brace fallback");
            return Self::brace();
        };
        if value.get("schema").and_then(toml::Value::as_str) != Some(Self::SCHEMA) {
            eprintln!("meld: warning: unsupported region_end.toml schema; using brace fallback");
            return Self::brace();
        }
        let kind = ScannerKind::from_id(
            value
                .get("kind")
                .and_then(toml::Value::as_str)
                .unwrap_or("brace"),
        );
        if matches!(kind, ScannerKind::Wasm | ScannerKind::Unknown(_)) {
            eprintln!(
                "meld: warning: region_end.toml kind {:?} is not an executable built-in; using brace fallback",
                value.get("kind").and_then(toml::Value::as_str).unwrap_or("")
            );
        }
        let mut options = BTreeMap::new();
        if let Some(table) = value.get("options").and_then(toml::Value::as_table) {
            for (key, val) in table {
                let rendered = match val {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Array(xs) => xs
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                    other => other.to_string(),
                };
                options.insert(key.clone(), rendered);
            }
        }
        Self {
            schema: Self::SCHEMA.to_string(),
            kind,
            fallback: value
                .get("fallback")
                .and_then(toml::Value::as_str)
                .unwrap_or("brace")
                .to_string(),
            hint: value
                .get("hint")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string(),
            options,
        }
    }

    pub fn from_dir(dir: &Path) -> Option<Self> {
        std::fs::read_to_string(dir.join("region_end.toml"))
            .ok()
            .map(|text| Self::parse(&text))
    }

    fn option(&self, key: &str) -> &str {
        self.options.get(key).map(String::as_str).unwrap_or("")
    }

    /// Resolve to a built-in Rust algorithm. No filesystem path, command or
    /// Wasm module from the pack is consulted here.
    pub fn scanner(&self) -> Arc<dyn EndScanner> {
        match self.kind {
            ScannerKind::Brace | ScannerKind::Wasm | ScannerKind::Unknown(_) => {
                Arc::new(BalancedBraceScanner)
            }
            ScannerKind::Heredoc => {
                let dialect = match self.option("dialect") {
                    "bash" => HeredocDialect::Bash,
                    "php" => HeredocDialect::Php,
                    _ => HeredocDialect::Ruby,
                };
                Arc::new(HeredocScanner::with_dialect(dialect))
            }
            ScannerKind::Indent => {
                let dialect = if self.option("dialect") == "yaml" {
                    IndentDialect::Yaml
                } else {
                    IndentDialect::Python
                };
                Arc::new(IndentScanner::with_dialect(dialect))
            }
            ScannerKind::Delimited => {
                let dialect = if self.option("dialect") == "sql" {
                    DelimitedDialect::Sql
                } else {
                    DelimitedDialect::Lua
                };
                Arc::new(DelimitedScanner::with_dialect(dialect))
            }
            ScannerKind::Counted => {
                let dialect = match self.option("dialect") {
                    "cpp" | "c++" => CountedDialect::Cpp,
                    "r" => CountedDialect::R,
                    "ocaml" => CountedDialect::Ocaml,
                    "haskell" => CountedDialect::Haskell,
                    _ => CountedDialect::Rust,
                };
                Arc::new(CountedScanner::with_dialect(dialect))
            }
            ScannerKind::Template => Arc::new(TemplateScanner),
        }
    }

    /// Register this profile under a pack id and all of its aliases.
    pub fn register<I, S>(&self, registry: &mut ScannerRegistry, names: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        registry.register(names, self.scanner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splitter::{split, RegionKind};

    #[test]
    fn unknown_and_wasm_kinds_fall_back_without_executing_anything() {
        for kind in ["wasm", "region_end.py", "made-up"] {
            let text = format!("schema = \"meld-region-end/1\"\nkind = \"{kind}\"\n");
            let spec = RegionEndSpec::parse(&text);
            assert!(matches!(
                spec.kind,
                ScannerKind::Wasm | ScannerKind::Unknown(_)
            ));
            let src = "@ruby { x = 1 }\n";
            let mut registry = ScannerRegistry::empty();
            spec.register(&mut registry, ["ruby"]);
            let doc = split(src, &registry).unwrap();
            let block = doc
                .regions
                .iter()
                .find(|r| r.kind == RegionKind::Block)
                .unwrap();
            assert_eq!(block.body(src), " x = 1 ");
        }
    }

    #[test]
    fn bad_schema_is_ignored() {
        let spec = RegionEndSpec::parse("schema = \"other/9\"\nkind = \"heredoc\"\n");
        assert!(matches!(spec.kind, ScannerKind::Brace));
    }
}
