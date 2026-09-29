//! Analytic validation suite. Every check compares a solver result with an independent
//! reference and a pass limit from the build directive; `exhaustctl validate` runs them all and
//! CI blocks on any failure.

pub mod acoustic;
pub mod crosscheck;
pub mod duct;
pub mod grid;
pub mod outlet;
pub mod perforate;
pub mod render;
pub mod sod;
pub mod steady;

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

/// Directive rows that need data this build does not have, with what each needs. Reported by
/// `exhaustctl validate`; neither passed nor failed.
pub const PENDING: [(&str, &str); 6] = [
    (
        "Thermal profile",
        "tailpipe gas temperature within 30 K of a thermocouple on the reference car, 2000 rpm cruise",
    ),
    (
        "Reference-car baseline",
        "2nd-order peak ±5 Hz and level ±3 dB of an ISO 5130 recording at three speeds; \
         backpressure ±15 % of a manometer at the downpipe flange",
    ),
    (
        "Turbine",
        "post-turbo pulse amplitude against a measured trace",
    ),
    (
        "Engine cylinder",
        "cylinder pressure against a measured trace",
    ),
    (
        "Catalyst",
        "ΔP against published brick data (the analytic model is checked in 'steady')",
    ),
    (
        "Bend",
        "K against Miller's charts (Idelchik's, implemented, is checked in 'steady')",
    ),
];

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
        ("extended-chamber", acoustic::extended_chamber),
        ("stub", acoustic::quarter_wave_stub),
        ("helmholtz", acoustic::helmholtz),
        ("tee", acoustic::tee_junction),
        ("crosscheck", crosscheck::run),
        ("perforate", perforate::run),
        ("steady", steady::run),
        ("render", render::run),
    ]
}
