//! Where each input's value comes from and how far it can be trusted. A project records, per
//! dotted field path (`engine.geometry.bore_mm`, or a whole element `system.elements.cat`),
//! the provenance of the value in the field, its source, and its range; other values the input
//! has had from other sources are kept as alternatives. Unrecorded values are the user's own
//! entries.
//!
//! An input without a stated range takes the default range of its class, labelled as such and
//! never left out of the sensitivity sweep (leaving it out would understate the uncertainty).
//! The class follows from the field's name, whose suffix carries its unit in this codebase:
//! - material properties (fill, substrate, tube material) ×0.5 to ×2;
//! - angles (`_deg`) ±10°;
//! - temperatures (`_k`) ±15 %;
//! - geometry (`_mm`, `_m`, `_l`, `_cm2`; elements and routes as a whole) ±5 %;
//! - dimensionless coefficients (discharge coefficients, loss and extraction factors,
//!   efficiencies, exponents) ±20 %;
//! - anything else ±30 %.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a value comes from, in order of precedence.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Measured on the vehicle or its parts.
    Measured,
    /// A manufacturer's specification, a standard, or a cited publication.
    Published,
    /// Computed from other inputs.
    Derived,
    /// Estimated: memory, forum figures, a placeholder, or a rule of thumb.
    Estimated,
}

/// How far a value may be from the truth.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Uncertainty {
    /// Anywhere in `[low, high]`, in the field's unit.
    Range { low: f64, high: f64 },
    /// Within ± `fraction` of the value.
    Relative { fraction: f64 },
    /// Within ± `plus_minus` of the value, in the field's unit.
    Absolute { plus_minus: f64 },
    /// Between the value / `factor` and the value × `factor`.
    Factor { factor: f64 },
}

impl Uncertainty {
    /// Lowest and highest value for a scalar `value`.
    pub fn bounds(&self, value: f64) -> (f64, f64) {
        match *self {
            Uncertainty::Range { low, high } => (low, high),
            Uncertainty::Relative { fraction } => {
                (value * (1.0 - fraction), value * (1.0 + fraction))
            }
            Uncertainty::Absolute { plus_minus } => (value - plus_minus, value + plus_minus),
            Uncertainty::Factor { factor } => (value / factor, value * factor),
        }
    }

    /// Scales every number of a group together: the lowest and highest factor. `None` for
    /// ranges in the field's unit, which only a scalar has.
    pub fn scale(&self) -> Option<(f64, f64)> {
        match *self {
            Uncertainty::Relative { fraction } => Some((1.0 - fraction, 1.0 + fraction)),
            Uncertainty::Factor { factor } => Some((1.0 / factor, factor)),
            Uncertainty::Range { .. } | Uncertainty::Absolute { .. } => None,
        }
    }

    fn valid(&self) -> bool {
        match *self {
            Uncertainty::Range { low, high } => low.is_finite() && high.is_finite() && low <= high,
            Uncertainty::Relative { fraction } => (0.0..1.0).contains(&fraction),
            Uncertainty::Absolute { plus_minus } => plus_minus.is_finite() && plus_minus >= 0.0,
            Uncertainty::Factor { factor } => factor.is_finite() && factor >= 1.0,
        }
    }
}

/// The recorded origin of one input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Provenance of the value in the field.
    pub provenance: Provenance,
    /// File, citation, or estimation method.
    pub source: String,
    /// Stated range; absent: the default range of the input's class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
    /// Values this input has had from other sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
    /// The user chose the value in the field over alternatives of higher precedence.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
}

/// A value an input has had from another source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Alternative {
    /// The field's value from this source, in the field's own form.
    pub value: serde_json::Value,
    pub provenance: Provenance,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
}

/// Kind of input, which sets the default range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Material,
    Angle,
    Temperature,
    Geometry,
    Coefficient,
    Unclassified,
}

/// Fields that describe a material rather than a shape.
const MATERIAL: [&str; 10] = [
    "cpsi",
    "cell_wall_mm",
    "fill_density_kg_m3",
    "fiber_diameter_um",
    "resistivity",
    "porosity",
    "tortuosity",
    "density_kg_m3",
    "conductivity_w_mk",
    "emissivity",
];

/// Names of dimensionless coefficients.
const COEFFICIENT: [&str; 6] = [
    "coefficient",
    "factor",
    "efficiency",
    "exponent",
    "retained",
    "n_compression",
];

impl Class {
    pub fn of(path: &str) -> Class {
        let field = path.rsplit('.').next().unwrap_or(path);
        if MATERIAL.contains(&field) {
            Class::Material
        } else if field.ends_with("_deg") {
            Class::Angle
        } else if field.ends_with("_k") {
            Class::Temperature
        } else if ["_mm", "_m", "_l", "_cm2"]
            .iter()
            .any(|s| field.ends_with(s))
            || path.starts_with("system.")
        {
            Class::Geometry
        } else if COEFFICIENT.iter().any(|c| field.contains(c)) || field == "n_expansion" {
            Class::Coefficient
        } else {
            Class::Unclassified
        }
    }

    pub fn default_uncertainty(self) -> Uncertainty {
        match self {
            Class::Material => Uncertainty::Factor { factor: 2.0 },
            Class::Angle => Uncertainty::Absolute { plus_minus: 10.0 },
            Class::Temperature => Uncertainty::Relative { fraction: 0.15 },
            Class::Geometry => Uncertainty::Relative { fraction: 0.05 },
            Class::Coefficient => Uncertainty::Relative { fraction: 0.20 },
            Class::Unclassified => Uncertainty::Relative { fraction: 0.30 },
        }
    }
}

impl Input {
    /// The range in force: the stated one, else the class default (`true` when defaulted).
    /// Measured and published values carry only a stated range.
    pub fn range(&self, path: &str) -> Option<(Uncertainty, bool)> {
        match (self.uncertainty, self.provenance) {
            (Some(u), _) => Some((u, false)),
            (None, Provenance::Estimated | Provenance::Derived) => {
                Some((Class::of(path).default_uncertainty(), true))
            }
            (None, _) => None,
        }
    }

    /// Problems that do not stop a solve: a defaulted range, a range the value cannot take.
    pub fn warnings(&self, path: &str, value: &serde_json::Value) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(u) = self.uncertainty {
            if !u.valid() {
                out.push(format!("input {path}: the stated range is not a range"));
            } else if !value.is_number() && u.scale().is_none() {
                out.push(format!(
                    "input {path}: a whole group takes a relative or factor range"
                ));
            }
        } else if let Some((u, true)) = self.range(path) {
            out.push(format!(
                "input {path}: default range for its class ({})",
                describe(&u)
            ));
        }
        out
    }

    /// Offers a value from another source: it replaces the value in use when its provenance
    /// takes precedence and the user has not chosen the value in use; either way the other
    /// value is kept as an alternative. Returns the value now in use.
    pub fn offer(&mut self, current: serde_json::Value, offered: Alternative) -> serde_json::Value {
        if !self.chosen && offered.provenance < self.provenance {
            let old = Alternative {
                value: current,
                provenance: self.provenance,
                source: std::mem::take(&mut self.source),
                uncertainty: self.uncertainty,
            };
            self.provenance = offered.provenance;
            self.source = offered.source;
            self.uncertainty = offered.uncertainty;
            self.alternatives.push(old);
            offered.value
        } else {
            self.alternatives.push(offered);
            current
        }
    }
}

/// The value at a dotted `path` of a project's JSON. A segment names an object's field, an
/// array element by its `id`, or else the field in every element of an array (the values as
/// an array).
pub fn value_at(project: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let mut at = project.clone();
    for seg in path.split('.') {
        at = match at {
            serde_json::Value::Object(mut m) => m.remove(seg)?,
            serde_json::Value::Array(items) => {
                match items
                    .iter()
                    .find(|i| i.get("id").and_then(|v| v.as_str()) == Some(seg))
                {
                    Some(item) => item.clone(),
                    None => serde_json::Value::Array(
                        items
                            .into_iter()
                            .map(|mut i| i.as_object_mut().and_then(|m| m.remove(seg)))
                            .collect::<Option<_>>()?,
                    ),
                }
            }
            _ => return None,
        };
    }
    Some(at)
}

/// A recorded input as a result reports it.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct InputUse {
    pub path: String,
    /// The value in use.
    pub value: serde_json::Value,
    pub provenance: Provenance,
    pub source: String,
    /// The range in force.
    pub range: Option<Uncertainty>,
    /// The range is its class's default, not a stated one.
    pub default_range: bool,
}

/// Every recorded input of `project`, in path order.
pub fn summary(project: &crate::project::Project) -> Vec<InputUse> {
    let value = serde_json::to_value(project).expect("projects serialise");
    project
        .inputs
        .iter()
        .map(|(path, input)| {
            let range = input.range(path);
            InputUse {
                path: path.clone(),
                value: value_at(&value, path).unwrap_or_default(),
                provenance: input.provenance,
                source: input.source.clone(),
                range: range.map(|r| r.0),
                default_range: range.is_some_and(|r| r.1),
            }
        })
        .collect()
}

/// A range for people: "±5 %", "±10", "×0.5–×2", "1.2–1.4".
pub fn describe(u: &Uncertainty) -> String {
    match *u {
        Uncertainty::Range { low, high } => format!("{low}–{high}"),
        Uncertainty::Relative { fraction } => format!("±{} %", 100.0 * fraction),
        Uncertainty::Absolute { plus_minus } => format!("±{plus_minus}"),
        Uncertainty::Factor { factor } => format!("×{}–×{factor}", 1.0 / factor),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classes_follow_the_field_names() {
        assert_eq!(Class::of("engine.valves.evo_deg"), Class::Angle);
        assert_eq!(
            Class::of("operating.intake_temperature_k"),
            Class::Temperature
        );
        assert_eq!(Class::of("engine.geometry.rod_mm"), Class::Geometry);
        assert_eq!(Class::of("system.elements.muffler"), Class::Geometry);
        assert_eq!(
            Class::of("engine.valves.discharge_coefficient"),
            Class::Coefficient
        );
        assert_eq!(Class::of("system.elements.cat.cpsi"), Class::Material);
        assert_eq!(Class::of("operating.map_kpa"), Class::Unclassified);
    }

    /// A measured value displaces an estimate; the estimate stays as an alternative. A value
    /// the user chose stays in use whatever arrives.
    #[test]
    fn precedence_and_choice() {
        let mut input = Input {
            provenance: Provenance::Estimated,
            source: "forum".into(),
            uncertainty: None,
            alternatives: Vec::new(),
            chosen: false,
        };
        let measured = Alternative {
            value: json!(147.0),
            provenance: Provenance::Measured,
            source: "caliper".into(),
            uncertainty: Some(Uncertainty::Absolute { plus_minus: 0.1 }),
        };
        assert_eq!(input.offer(json!(145.0), measured.clone()), json!(147.0));
        assert_eq!(
            (input.provenance, input.alternatives[0].value.clone()),
            (Provenance::Measured, json!(145.0))
        );
        input.chosen = true;
        let published = Alternative {
            provenance: Provenance::Published,
            value: json!(148.0),
            ..measured
        };
        assert_eq!(input.offer(json!(147.0), published), json!(147.0));
        assert_eq!(input.alternatives.len(), 2);
    }
}
