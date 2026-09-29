//! Analytic validation suite. Every check compares a solver result with an independent
//! reference and a pass limit from the build directive; `exhaustctl validate` runs them all and
//! CI blocks on any failure.

pub mod acoustic;
pub mod duct;
pub mod grid;
pub mod outlet;
pub mod sod;

use serde::Serialize;

use crate::error::Result;

/// One validated quantity.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub case: String,
    pub metric: String,
    pub value: f64,
    /// Pass when `value <= limit`.
    pub limit: f64,
    pub pass: bool,
}

impl Check {
    pub fn new(case: impl Into<String>, metric: impl Into<String>, value: f64, limit: f64) -> Self {
        Self {
            case: case.into(),
            metric: metric.into(),
            value,
            limit,
            pass: value.is_finite() && value <= limit,
        }
    }
}

/// A validation case: name and the function that runs it.
pub type Case = (&'static str, fn() -> Result<Vec<Check>>);

/// All cases, in the order they are reported.
pub fn cases() -> Vec<Case> {
    vec![
        ("sod", sod::run),
        ("outlet", outlet::run),
        ("duct", duct::run),
        ("grid", grid::run),
        ("determinism", grid::run_determinism),
        ("open-pipe-fourpole", acoustic::open_pipe_fourpole),
        ("area-change", acoustic::area_change),
        ("chamber", acoustic::expansion_chamber),
        ("stub", acoustic::quarter_wave_stub),
        ("helmholtz", acoustic::helmholtz),
        ("tee", acoustic::tee_junction),
    ]
}
