//! `schemas/project.schema.json` and `schemas/api.schema.json` must equal the schemas generated
//! from the project types and from everything the app backend sends its frontend.
//! Regenerate with `UPDATE_SCHEMA=1 cargo test -p exhaust-core --test schema`.

use exhaust_core::layout::Layout;
use exhaust_core::manifest::Manifest;
use exhaust_core::project::Project;
use exhaust_core::solve::{PointOutcome, SweepResult};
use schemars::{JsonSchema, Schema};

/// The app backend's replies, for generating the frontend's types.
#[allow(dead_code)]
#[derive(JsonSchema)]
struct Api {
    layout: Layout,
    manifests: Vec<Manifest>,
    sweep: SweepResult,
    /// One operating point as a time-domain sweep streams it.
    point: PointOutcome,
}

fn check(file: &str, schema: Schema) {
    let path = format!("{}/../../schemas/{file}", env!("CARGO_MANIFEST_DIR"));
    let generated = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == generated,
        "schemas/{file} is stale; run UPDATE_SCHEMA=1 cargo test -p exhaust-core --test schema"
    );
}

#[test]
fn checked_in_schemas_match_types() {
    check("project.schema.json", schemars::schema_for!(Project));
    check("api.schema.json", schemars::schema_for!(Api));
}

#[test]
fn reference_cases_parse_and_round_trip() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/cases");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let project =
            Project::from_json(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(Project::from_json(&project.to_json()).unwrap(), project);
    }
}
