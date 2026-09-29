//! `schemas/project.schema.json` must equal the schema generated from the project types.
//! Regenerate with `UPDATE_SCHEMA=1 cargo test -p exhaust-core --test schema`.

use exhaust_core::project::Project;

#[test]
fn checked_in_schema_matches_types() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../schemas/project.schema.json"
    );
    let generated = serde_json::to_string_pretty(&schemars::schema_for!(Project)).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(path, &generated).unwrap();
    }
    let on_disk = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        on_disk == generated,
        "schemas/project.schema.json is stale; run UPDATE_SCHEMA=1 cargo test -p exhaust-core --test schema"
    );
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
