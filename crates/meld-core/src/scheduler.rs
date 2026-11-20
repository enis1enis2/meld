//! Runner scheduler: tier dispatch over a value bridge.
//! Through Phase 3 execution is sequential in plan order (project.md §13).
use crate::binding::{Bindings, Export, Import};
use crate::error::Result;
use meld_value::Value;
use crate::splitter::{Region, SplitDocument};

#[derive(Clone, Debug)]
pub struct Job {
    pub region: Region,
    pub code: String,
    /// Values this region's glue declares as exports. The backend fetches the
    /// native binding from its worker and publishes it into the shared Bridge.
    pub exports: Vec<Export>,
    pub imports: Vec<Import>,
}

#[derive(Clone, Debug, Default)]
pub struct Bridge {
    pub values: std::collections::HashMap<String, Value>,
}

pub trait Backend {
    fn tier(&self) -> &'static str;
    fn run(&mut self, job: &Job, bridge: &mut Bridge) -> Result<Vec<String>>;
}

pub struct Scheduler<B: Backend> {
    backend: B,
}

impl<B: Backend> Scheduler<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn run(&mut self, doc: &SplitDocument, plan: &[String], bindings: &Bindings) -> Result<Vec<String>> {
        let mut bridge = Bridge::default();
        let mut stdout = Vec::new();
        for id in plan {
            let region = doc
                .regions
                .iter()
                .find(|r| &r.id == id)
                .expect("plan references a known region");
            let imports: Vec<Import> = bindings
                .imports
                .iter()
                .filter(|i| &i.region_id == id)
                .cloned()
                .collect();
            let exports: Vec<Export> = bindings
                .exports
                .iter()
                .filter(|e| &e.region_id == id)
                .cloned()
                .collect();
            let job = Job {
                region: region.clone(),
                code: strip_glue(region.body(&doc.source)),
                exports,
                imports,
            };
            let lines = self.backend.run(&job, &mut bridge)?;
            stdout.extend(lines);
            // Owned handles are dropped at region exit (spec/value-model.md §3).
            bridge.values.retain(|_, v| !matches!(v, Value::Handle { borrow: false, .. }));
        }
        Ok(stdout)
    }
}

pub fn strip_glue(code: &str) -> String {
    let filtered = code
        .lines()
        .filter(|l| !l.trim_start().starts_with("export ") && !l.trim_start().starts_with("import "))
        .collect::<Vec<_>>()
        .join("\n");
    dedent(&filtered)
}

/// JUDGMENT CALL: block regions are written indented inside `@lang { ... }`,
/// but indentation is significant in languages like Python. Meld removes the
/// common leading whitespace so region bodies are valid standalone code.
/// Indentation characters are ASCII, so byte slicing is safe here.
pub fn dedent(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.len() >= indent {
                &l[indent..]
            } else {
                l.trim_start()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
