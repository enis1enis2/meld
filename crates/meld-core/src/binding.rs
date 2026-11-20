//! Binding resolver: explicit export/import glue → execution DAG.
use crate::error::{MeldError, Result};
use crate::splitter::{Region, SplitDocument};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct Export {
    pub region_id: String,
    pub name: String,
    pub bridge_name: String,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub region_id: String,
    pub name: String,
    pub source_region: String,
    pub bridge_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Bindings {
    pub exports: Vec<Export>,
    pub imports: Vec<Import>,
    pub region_ids: HashSet<String>,
}

/// Extract glue lines. JUDGMENT CALL: glue is line-based and stripped from
/// runner code; this keeps the host language lexer untouched.
pub fn resolve(doc: &SplitDocument) -> Result<Bindings> {
    let mut out = Bindings::default();
    for r in &doc.regions {
        out.region_ids.insert(r.id.clone());
        for line in r.body(&doc.source).lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("export ") {
                let mut parts = rest.split_whitespace();
                let name = parts.next().unwrap_or("").to_string();
                let bridge = if let Some(_as) = parts.next().filter(|p| *p == "as") {
                    parts.next().unwrap_or(&name).to_string()
                } else {
                    name.clone()
                };
                out.exports.push(Export {
                    region_id: r.id.clone(),
                    name,
                    bridge_name: bridge,
                });
            } else if let Some(rest) = line.strip_prefix("import ") {
                let mut parts = rest.split_whitespace();
                let name = parts.next().unwrap_or("").to_string();
                if parts.next().filter(|p| *p == "from").is_none() {
                    return Err(MeldError::Parse {
                        region_id: r.id.clone(),
                        message: "import requires 'from <region-id>'".to_string(),
                    });
                }
                let source_region = parts.next().unwrap_or("").to_string();
                out.exports
                    .iter()
                    .find(|e| e.region_id == source_region && e.bridge_name == name)
                    .ok_or_else(|| MeldError::Parse {
                        region_id: r.id.clone(),
                        message: format!("no export '{name}' from '{source_region}'"),
                    })?;
                out.imports.push(Import {
                    region_id: r.id.clone(),
                    name: name.clone(),
                    source_region,
                    bridge_name: name,
                });
            }
        }
    }
    let mut seen = HashSet::new();
    for e in &out.exports {
        if !seen.insert(e.bridge_name.clone()) {
            return Err(MeldError::BridgeCollision(e.bridge_name.clone()));
        }
    }
    Ok(out)
}

/// Kahn topological sort. Sequential file order is preserved for independent
/// jobs (project.md §13 default).
pub fn execution_plan(doc: &SplitDocument, b: &Bindings) -> Result<Vec<String>> {
    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for r in &doc.regions {
        indegree.entry(r.id.clone()).or_insert(0);
    }
    for imp in &b.imports {
        if !b.region_ids.contains(&imp.source_region) {
            return Err(MeldError::UnknownLang(imp.source_region.clone()));
        }
        *indegree.entry(imp.region_id.clone()).or_insert(0) += 1;
        edges
            .entry(imp.source_region.clone())
            .or_default()
            .push(imp.region_id.clone());
    }
    let order: Vec<String> = doc
        .regions
        .iter()
        .map(|r: &Region| r.id.clone())
        .collect();
    let mut ready: Vec<String> = order
        .iter()
        .filter(|id| indegree.get(*id).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();
    let mut result = Vec::new();
    while let Some(id) = ready.first().cloned() {
        ready.remove(0);
        result.push(id.clone());
        if let Some(next) = edges.get(&id) {
            for n in next {
                let d = indegree.get_mut(n).expect("tracked node");
                *d -= 1;
                if *d == 0 {
                    ready.push(n.clone());
                }
            }
        }
    }
    if result.len() != indegree.len() {
        return Err(MeldError::BridgeCycle("import graph is cyclic".to_string()));
    }
    Ok(result)
}
