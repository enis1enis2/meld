//! Real T2 integration test: spawns the Python worker over stdio and verifies
//! that glue exports are fetched into the shared bridge and imports land in
//! the worker namespace. Skipped when python3 is unavailable.
use meld_core::binding::Export;
use meld_core::scheduler::{Backend, Bridge, Job};
use meld_core::splitter::{Region, RegionKind};
use meld_t2::T2Backend;
use meld_value::Value;
use std::path::Path;
use std::process::Command;

fn python3_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn region(lang: &str) -> Region {
    Region {
        id: "r0".to_string(),
        lang: lang.to_string(),
        kind: RegionKind::Block,
        start: 0,
        end: 0,
        full_start: 0,
        full_end: 0,
        open_marker: String::new(),
        close_marker: String::new(),
        confidence: 1.0,
    }
}

#[test]
fn export_and_import_round_trip_through_python_worker() {
    if !python3_available() {
        eprintln!("skipping: python3 is unavailable");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let worker = root.join("packs/python/worker.py");
    let worker = worker.to_str().expect("utf-8 path");

    let mut backend = T2Backend::spawn("python3", &["-u", worker]).expect("spawn python worker");
    let mut bridge = Bridge::default();

    let producer = Job {
        region: region("python"),
        code: "values = [1, 2, 3]\nname = 'meld'".to_string(),
        exports: vec![
            Export {
                region_id: "r0".to_string(),
                name: "values".to_string(),
                bridge_name: "nums".to_string(),
            },
            Export {
                region_id: "r0".to_string(),
                name: "name".to_string(),
                bridge_name: "label".to_string(),
            },
        ],
        imports: vec![],
    };
    let stdout = backend
        .run(&producer, &mut bridge)
        .expect("producer executes");
    assert!(stdout.is_empty());
    assert_eq!(
        bridge.values.get("nums"),
        Some(&Value::List(vec![
            Value::int(1),
            Value::int(2),
            Value::int(3)
        ])),
    );
    assert_eq!(
        bridge.values.get("label"),
        Some(&Value::String("meld".to_string())),
    );

    let mut consumer_region = region("python");
    consumer_region.id = "r1".to_string();
    let imp = meld_core::binding::Import {
        region_id: "r1".to_string(),
        name: "nums".to_string(),
        source_region: "r0".to_string(),
        bridge_name: "nums".to_string(),
    };
    let consumer = Job {
        region: consumer_region,
        code: "print(len(nums))".to_string(),
        exports: vec![],
        imports: vec![imp],
    };
    let stdout = backend
        .run(&consumer, &mut bridge)
        .expect("consumer executes");
    assert_eq!(stdout, vec!["3".to_string()]);
}
