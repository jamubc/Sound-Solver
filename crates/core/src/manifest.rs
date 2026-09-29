//! Element manifests (`crates/core/manifests/*.toml`): each element type's parameters with
//! labels, units and allowed ranges, for the editor and for `Element::validate`. Compiled in;
//! their keys, kinds and ports are checked against the project schema.

use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Element type, as in the project file.
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub summary: String,
    /// Fixed ports; a chamber adds `out2`, `out3`, … for its extra outlets.
    pub ports: Vec<String>,
    #[serde(default, rename = "param")]
    pub params: Vec<Param>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Param {
    /// Field name in the project file.
    pub key: String,
    pub label: String,
    pub kind: ParamKind,
    /// Numbers only: unit, inclusive range, and the editor's default.
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub default: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Number,
    /// A material id from the project's table.
    Material,
    /// A direction in vehicle coordinates (normalised on use).
    Direction,
    /// Offsets from a port, mm, one per extra port.
    Offsets,
}

const SOURCES: [&str; 11] = [
    include_str!("../manifests/source.toml"),
    include_str!("../manifests/outlet.toml"),
    include_str!("../manifests/area_change.toml"),
    include_str!("../manifests/cap.toml"),
    include_str!("../manifests/expansion_chamber.toml"),
    include_str!("../manifests/quarter_wave_stub.toml"),
    include_str!("../manifests/helmholtz.toml"),
    include_str!("../manifests/catalyst.toml"),
    include_str!("../manifests/valve.toml"),
    include_str!("../manifests/tee.toml"),
    include_str!("../manifests/absorptive.toml"),
];

/// Every element type's manifest.
pub fn all() -> &'static [Manifest] {
    static ALL: LazyLock<Vec<Manifest>> = LazyLock::new(|| {
        SOURCES
            .iter()
            .map(|s| toml::from_str(s).expect("manifests parse (checked by test)"))
            .collect()
    });
    &ALL
}

/// The manifest of element type `kind`.
pub fn get(kind: &str) -> Option<&'static Manifest> {
    all().iter().find(|m| m.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Element, Project};
    use crate::validation::grid::STRAIGHT_PIPE;

    #[test]
    fn manifests_match_the_schema_and_their_defaults_validate() {
        let schema = serde_json::to_value(schemars::schema_for!(Project)).expect("schema");
        let variants = schema["$defs"]["Element"]["oneOf"]
            .as_array()
            .expect("variants");
        assert_eq!(variants.len(), all().len());
        let project = Project::from_json(STRAIGHT_PIPE).expect("case");
        for v in variants {
            let props = v["properties"].as_object().expect("properties");
            let kind = props["type"]["const"].as_str().expect("type");
            let m = get(kind).unwrap_or_else(|| panic!("no manifest for {kind}"));
            let mut keys: Vec<&str> = props
                .keys()
                .map(String::as_str)
                .filter(|&k| k != "type")
                .collect();
            let mut listed: Vec<&str> = m.params.iter().map(|p| p.key.as_str()).collect();
            keys.sort_unstable();
            listed.sort_unstable();
            assert_eq!(keys, listed, "{kind}: schema fields against manifest");
            let mut element =
                serde_json::json!({ "id": "e", "type": kind, "position_mm": [0.0, 0.0, 0.0] });
            for p in &m.params {
                let number = props[&p.key]["type"] == "number";
                assert_eq!(number, p.kind == ParamKind::Number, "{kind}.{}", p.key);
                match p.kind {
                    ParamKind::Number => {
                        let (lo, hi, def) = (p.min.unwrap(), p.max.unwrap(), p.default.unwrap());
                        assert!(
                            p.unit.is_some() && lo <= def && def <= hi,
                            "{kind}.{}",
                            p.key
                        );
                        element[&p.key] = def.into();
                    }
                    ParamKind::Material => element[&p.key] = "409".into(),
                    ParamKind::Direction | ParamKind::Offsets => {}
                }
            }
            let element: Element = serde_json::from_value(element).expect("defaults deserialise");
            assert_eq!(element.kind.port_names(), m.ports, "{kind} ports");
            element
                .validate(&project)
                .unwrap_or_else(|e| panic!("{kind} defaults: {e}"));
        }
    }
}
