//! Project file: geometry, engine, operating point and solver settings in one versioned JSON
//! document. `schemas/project.schema.json` is generated from these types (see the test in
//! `crates/core/tests/schema.rs`); older schema versions are upgraded by [`migrate`].
//!
//! The exhaust system is a graph: elements (source, outlet, area change, cap, …) own named
//! ports; routes are bent tubes that connect two ports. A route's centreline starts and ends at
//! its ports' positions, so moving an element moves the pipe ends with it.

use std::collections::{BTreeMap, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::{EngineGeometry, EvoState, ExhaustValves};
use crate::error::{Error, Result};
use crate::gas1d::Limiter;
use crate::geometry::{Vec3, add, scale, unit};
use crate::inputs::{Input, value_at};
use crate::manifest;

/// Current project schema version.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Provenance, source and range of recorded inputs, keyed by dotted field path (e.g.
    /// `engine.geometry.bore_mm`, or `system.elements.cat` for a whole element; `inputs`).
    /// Unrecorded values are the user's own entries.
    #[serde(default)]
    pub inputs: BTreeMap<String, Input>,
    pub engine: EngineSpec,
    pub turbine: TurbineSpec,
    pub gas: GasSpec,
    pub ambient: Ambient,
    pub operating: Operating,
    pub system: System,
    pub receiver: Receiver,
    pub solver: SolverSettings,
    pub materials: Vec<Material>,
    #[serde(default, skip_serializing_if = "Measurements::is_empty")]
    pub measurements: Measurements,
    #[serde(default)]
    pub fabrication: Fabrication,
}

/// Data measured on the vehicle and stored with the project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Measurements {
    /// Without it the interior drone is unavailable: the cabin is never synthesised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cabin_tf: Option<CabinTf>,
    /// Recordings of the car to set against the prediction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recordings: Vec<RecordingRef>,
}

impl Measurements {
    pub fn is_empty(&self) -> bool {
        self.cabin_tf.is_none() && self.recordings.is_empty()
    }
}

/// A recording of the car (WAV or CAF) and the engine-speed log taken with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordingRef {
    /// Relative to the project file, or absolute.
    pub path: String,
    pub position: MicPosition,
    /// CSV of engine speed against time from an OBD logger, as `path`; without one the
    /// engine speed is estimated from the recording and flagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm_log: Option<String>,
    /// Time on the log (from its first row) at the start of the recording, s.
    #[serde(default)]
    pub log_offset_s: f64,
    /// Sound pressure level of a full-scale sine, dB re 20 µPa; without it levels are dB re
    /// full scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_db: Option<f64>,
}

/// Where the microphone was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MicPosition {
    /// At the project's receiver.
    Exterior,
    /// At the driver's ear.
    Interior,
}

/// Cabin transfer function: level at the driver's ear minus level at the exterior receiver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CabinTf {
    pub method: CabinTfMethod,
    /// Where and when it was measured.
    pub source: String,
    /// `[[frequency_hz, gain_db], …]`, frequency increasing; linear between points, undefined
    /// outside them.
    pub gain_db: Vec<[f64; 2]>,
}

impl CabinTf {
    /// Gain at `f`, dB; `None` outside the measured range.
    pub fn at(&self, f: f64) -> Option<f64> {
        let g = &self.gain_db;
        let k = g.partition_point(|p| p[0] < f);
        if k < g.len() && g[k][0] == f {
            return Some(g[k][1]);
        }
        if k == 0 || k == g.len() {
            return None;
        }
        let (a, b) = (g[k - 1], g[k]);
        Some(a[1] + (b[1] - a[1]) * (f - a[0]) / (b[0] - a[0]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CabinTfMethod {
    /// Exterior and interior recordings at the same cruise point; ratio per engine order.
    OrderRatio,
    /// Impulse response: clap or balloon at the tailpipe, phone at the driver's ear.
    Impulse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    pub label: String,
    pub geometry: EngineGeometry,
    pub valves: ExhaustValves,
    /// Polytropic exponent of the cylinder gas after EVO.
    pub polytropic_exponent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurbineSpec {
    /// Fraction of the isentropic enthalpy drop across the turbine removed as shaft work.
    pub extraction_factor: f64,
    pub scrolls: Vec<ScrollSpec>,
    /// Temperature the ports and housing reject heat to, K.
    #[serde(default = "default_coolant")]
    pub coolant_temperature_k: f64,
}

fn default_coolant() -> f64 {
    363.15
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScrollSpec {
    /// 1-based cylinders feeding this scroll.
    pub cylinders: Vec<usize>,
    /// Volume of exhaust ports, manifold runners and scroll, litres.
    pub manifold_volume_l: f64,
    /// Effective nozzle area `C_d A` of the scroll including an open wastegate, cm².
    pub nozzle_area_cm2: f64,
    /// Heat-loss conductance of ports, runners and turbine housing to the coolant, W/K.
    #[serde(default)]
    pub heat_loss_w_per_k: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GasSpec {
    /// Fuel hydrogen-to-carbon atom ratio (gasoline ≈ 1.87).
    pub fuel_h_to_c: f64,
    /// Air-excess ratio λ (≥ 1).
    pub lambda: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ambient {
    pub pressure_pa: f64,
    pub temperature_k: f64,
    /// Vehicle speed; sets the external film coefficient in the thermal model.
    pub vehicle_speed_kmh: f64,
    /// Air speed across the pipes as a fraction of vehicle speed (underbody boundary layer).
    #[serde(default = "default_underbody_air")]
    pub underbody_air_factor: f64,
    /// Height of the ground plane in vehicle coordinates, mm (negative: below the flange).
    pub ground_z_mm: f64,
}

fn default_underbody_air() -> f64 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Operating {
    /// Intake manifold absolute pressure against engine speed: `[[rpm, kPa], …]`, linear,
    /// held constant outside the table.
    pub map_kpa: Vec<[f64; 2]>,
    pub intake_temperature_k: f64,
    pub volumetric_efficiency: f64,
    pub lhv_mj_per_kg: f64,
    /// Fraction of fuel heating value in the cylinder gas after combustion and wall losses.
    pub heat_retained: f64,
    pub n_compression: f64,
    pub n_expansion: f64,
    /// Measured in-cylinder state at EVO; replaces the Otto-cycle estimate when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evo_override: Option<EvoState>,
    /// Engine-speed band searched for drone, rpm.
    pub cruise_band_rpm: [f64; 2],
    /// Default sweep: `[start, stop, step]`, rpm.
    pub sweep_rpm: [f64; 3],
}

impl Operating {
    /// Manifold pressure at `rpm`, kPa.
    pub fn map_at(&self, rpm: f64) -> f64 {
        let t = &self.map_kpa;
        if rpm <= t[0][0] {
            return t[0][1];
        }
        if rpm >= t[t.len() - 1][0] {
            return t[t.len() - 1][1];
        }
        let k = t.partition_point(|p| p[0] <= rpm) - 1;
        t[k][1] + (t[k + 1][1] - t[k][1]) * (rpm - t[k][0]) / (t[k + 1][0] - t[k][0])
    }

    /// Engine speeds of the default sweep.
    pub fn sweep(&self) -> Vec<f64> {
        sweep_points(self.sweep_rpm[0], self.sweep_rpm[1], self.sweep_rpm[2])
    }
}

/// `start, start + step, …` up to and including `stop` (to within step/1000).
pub fn sweep_points(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let n = ((stop - start) / step + 1e-3).floor() as usize + 1;
    (0..n).map(|i| start + i as f64 * step).collect()
}

/// Exhaust system graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct System {
    pub elements: Vec<Element>,
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Element {
    pub id: String,
    /// Position of the element's first port, mm.
    pub position_mm: Vec3,
    /// Flow direction through the element (normalised on use).
    #[serde(default = "default_axis")]
    pub axis: Vec3,
    #[serde(flatten)]
    pub kind: ElementKind,
}

fn default_axis() -> Vec3 {
    [-1.0, 0.0, 0.0]
}

/// Element types and their parameters. Port names are fixed per type (see
/// [`ElementKind::port_names`]); pipes attach to ports by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ElementKind {
    /// Engine and turbine; port `out` is the downpipe flange face.
    Source,
    /// Unflanged tailpipe tip radiating to atmosphere; port `in`.
    Outlet,
    /// Joins two pipes of different bore: sudden when `taper_length_mm` is 0, otherwise a
    /// cone of that length. Ports `in`, `out`; bores come from the connected pipes.
    AreaChange {
        taper_length_mm: f64,
    },
    /// Closed pipe end; port `in`.
    Cap,
    ExpansionChamber(ExpansionChamber),
    QuarterWaveStub(QuarterWaveStub),
    Helmholtz(Helmholtz),
    Catalyst(Catalyst),
    Valve(Valve),
    Tee(Tee),
    Absorptive(Absorptive),
}

fn default_shell() -> f64 {
    1.2
}

fn down() -> Vec3 {
    [0.0, 0.0, -1.0]
}

/// Simple expansion chamber (reactive muffler) with optional inlet and outlet tubes extending
/// into it. Ports: `in` at `position_mm`, `out` at `position_mm + axis·length_mm`, and `out2`,
/// `out3`, … at `extra_outlets_mm` (offsets from `out`) for multi-outlet mufflers. Pipe bores
/// come from the connected routes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpansionChamber {
    /// Inner diameter of the shell, mm.
    pub diameter_mm: f64,
    pub length_mm: f64,
    #[serde(default)]
    pub inlet_extension_mm: f64,
    #[serde(default)]
    pub outlet_extension_mm: f64,
    /// Shell thickness, mm (heat loss and fabrication).
    #[serde(default = "default_shell")]
    pub shell_mm: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_outlets_mm: Vec<Vec3>,
}

/// Closed side branch (quarter-wave resonator) teed onto the main pipe. Ports `in` and `out`
/// both at `position_mm`; the branch leaves along `direction`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QuarterWaveStub {
    pub id_mm: f64,
    pub wall_mm: f64,
    /// Physical length from the main-pipe wall to the cap, mm.
    pub length_mm: f64,
    pub material: String,
    #[serde(default = "down")]
    pub direction: Vec3,
}

/// Helmholtz resonator teed onto the main pipe: a neck into a cylindrical cavity. Ports `in`
/// and `out` both at `position_mm`; the neck leaves along `direction`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Helmholtz {
    pub neck_id_mm: f64,
    /// Physical neck length, mm.
    pub neck_length_mm: f64,
    pub volume_l: f64,
    pub cavity_diameter_mm: f64,
    #[serde(default = "down")]
    pub direction: Vec3,
}

/// Monolith catalyst: inlet cone, brick of square channels, outlet cone. Ports `in` at
/// `position_mm` and `out` at `position_mm + axis·(inlet cone + brick + outlet cone)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Catalyst {
    /// Cells per square inch.
    pub cpsi: f64,
    /// Cell wall thickness, mm (4 mil = 0.1016 mm).
    pub cell_wall_mm: f64,
    pub brick_length_mm: f64,
    pub brick_diameter_mm: f64,
    #[serde(default)]
    pub inlet_cone_mm: f64,
    #[serde(default)]
    pub outlet_cone_mm: f64,
}

/// Absorptive straight-through muffler: perforated tube of the pipe bore inside a cylindrical
/// case; the annulus is packed with fibre (or empty: a concentric-tube resonator). Ports `in`
/// at `position_mm` and `out` at `position_mm + axis·length_mm`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Absorptive {
    /// Inner diameter of the case, mm.
    pub case_diameter_mm: f64,
    pub length_mm: f64,
    /// Open-area ratio of the perforated tube.
    pub open_area_ratio: f64,
    pub hole_diameter_mm: f64,
    /// Perforated tube wall, mm.
    pub perforate_thickness_mm: f64,
    /// Packing density, kg/m³ (0: empty annulus).
    #[serde(default)]
    pub fill_density_kg_m3: f64,
    /// Fibre diameter, µm (basalt or glass wool ≈ 10–13 µm).
    #[serde(default = "default_fibre")]
    pub fiber_diameter_um: f64,
    #[serde(default = "default_shell")]
    pub shell_mm: f64,
}

fn default_fibre() -> f64 {
    12.0
}

/// Butterfly valve (electric cutout or exhaust flap). Ports `in` and `out` at `position_mm`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Valve {
    /// Disc opening: 0° closed, 90° fully open.
    pub angle_deg: f64,
}

/// Tee or wye junction. Ports `in`, `out` and `branch`, all at `position_mm`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tee {
    #[serde(default = "down")]
    pub branch_direction: Vec3,
}

impl ElementKind {
    pub fn port_names(&self) -> Vec<String> {
        let names: &[&str] = match self {
            ElementKind::Source => &["out"],
            ElementKind::Outlet | ElementKind::Cap => &["in"],
            ElementKind::Tee(_) => &["in", "out", "branch"],
            ElementKind::ExpansionChamber(c) => {
                let mut v = vec!["in".to_string(), "out".to_string()];
                v.extend((0..c.extra_outlets_mm.len()).map(|k| format!("out{}", k + 2)));
                return v;
            }
            _ => &["in", "out"],
        };
        names.iter().map(|s| s.to_string()).collect()
    }

    pub fn has_port(&self, port: &str) -> bool {
        self.port_names().iter().any(|p| p == port)
    }
}

impl Element {
    /// Position of a named port, mm.
    pub fn port_position(&self, port: &str) -> Option<Vec3> {
        if !self.kind.has_port(port) {
            return None;
        }
        let axis = unit(self.axis);
        let along = |len: f64| add(self.position_mm, scale(axis, len));
        Some(match (&self.kind, port) {
            (ElementKind::AreaChange { taper_length_mm }, "out") => along(*taper_length_mm),
            (ElementKind::ExpansionChamber(c), "out") => along(c.length_mm),
            (ElementKind::ExpansionChamber(c), p) if p.starts_with("out") => {
                let k: usize = p[3..].parse().ok()?;
                add(along(c.length_mm), *c.extra_outlets_mm.get(k - 2)?)
            }
            (ElementKind::Catalyst(c), "out") => {
                along(c.inlet_cone_mm + c.brick_length_mm + c.outlet_cone_mm)
            }
            (ElementKind::Absorptive(a), "out") => along(a.length_mm),
            _ => self.position_mm,
        })
    }

    /// Physical consistency of the element's parameters: each number within its manifest's
    /// range (`manifest`), then the relations between them.
    pub fn validate(&self, project: &Project) -> Result<()> {
        let bad = |msg: &str| Err(Error::invalid(format!("element '{}': {msg}", self.id)));
        let value = serde_json::to_value(&self.kind).expect("elements serialise");
        let kind = value["type"].as_str().unwrap_or_default();
        let m = manifest::get(kind).expect("every element type has a manifest");
        for p in &m.params {
            if let (Some(lo), Some(hi)) = (p.min, p.max) {
                // Fields left out of the file take their serde default, which is the manifest's.
                let x = value[&p.key].as_f64().or(p.default);
                if !x.is_some_and(|x| (lo..=hi).contains(&x)) {
                    let unit = p.unit.as_deref().unwrap_or_default();
                    return bad(&format!("{} must be {lo}–{hi} {unit}", p.label));
                }
            }
        }
        match &self.kind {
            ElementKind::ExpansionChamber(c) => {
                if c.inlet_extension_mm + c.outlet_extension_mm >= c.length_mm {
                    return bad("tube extensions must leave part of the chamber open");
                }
                if !c.extra_outlets_mm.is_empty() && c.outlet_extension_mm > 0.0 {
                    return bad("outlet extensions are modelled for single-outlet chambers only");
                }
                Ok(())
            }
            ElementKind::QuarterWaveStub(s) if project.material(&s.material).is_none() => {
                bad("unknown stub material")
            }
            ElementKind::Helmholtz(h) if h.cavity_diameter_mm <= h.neck_id_mm => {
                bad("cavity must be wider than the neck")
            }
            ElementKind::Catalyst(c) if c.cell_wall_mm >= 0.5 * 25.4 / c.cpsi.sqrt() => {
                bad("the cell wall must be under half the cell pitch")
            }
            _ => Ok(()),
        }
    }
}

/// A port on an element.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub element: String,
    pub port: String,
}

/// A bent tube between two ports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: String,
    pub from: PortRef,
    pub to: PortRef,
    /// Interior centreline vertices, mm; the ends are the ports' positions.
    #[serde(default)]
    pub via_mm: Vec<Vec3>,
    /// Centreline bend radius at each interior vertex, mm; `null` uses 1.5 × OD.
    #[serde(default)]
    pub bend_radius_mm: Vec<Option<f64>>,
    pub pipe: PipeSpec,
    /// Hangers, mm along the centreline from the route's start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hangers_mm: Vec<f64>,
    /// Joint to the element at the route's start; absent: a flange at the source, else a
    /// butt weld.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_joint: Option<Joint>,
    /// Joint to the element at the route's end; absent as for `start_joint`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_joint: Option<Joint>,
}

/// How two parts are joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Joint {
    Butt,
    Slip,
    VBand,
    Flange,
}

/// Fabrication settings: stock, cutting, and underbody clearance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fabrication {
    /// Straight tube is bought in lengths of, mm (10 ft).
    #[serde(default = "default_stock_length")]
    pub stock_length_mm: f64,
    /// Saw kerf per cut, mm.
    #[serde(default = "default_kerf")]
    pub kerf_mm: f64,
    /// A stock 45° or 90° mandrel bend is used uncut within this angle, degrees.
    #[serde(default = "default_bend_tolerance")]
    pub stock_bend_tolerance_deg: f64,
    /// Gap wanted between pipe surface and underbody, mm.
    #[serde(default = "default_clearance")]
    pub clearance_mm: f64,
    /// Stretches that need a different gap (near the rear axle, say).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clearance_zones: Vec<ClearanceZone>,
    /// Underbody scan and where it sits on the car.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan: Option<Scan>,
}

impl Default for Fabrication {
    fn default() -> Self {
        Self {
            stock_length_mm: default_stock_length(),
            kerf_mm: default_kerf(),
            stock_bend_tolerance_deg: default_bend_tolerance(),
            clearance_mm: default_clearance(),
            clearance_zones: Vec::new(),
            scan: None,
        }
    }
}

fn default_stock_length() -> f64 {
    3048.0
}

fn default_kerf() -> f64 {
    3.0
}

fn default_bend_tolerance() -> f64 {
    0.5
}

fn default_clearance() -> f64 {
    25.0
}

/// A stretch along the car, `x_min_mm ≤ x ≤ x_max_mm`, with its own clearance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearanceZone {
    pub name: String,
    pub x_min_mm: f64,
    pub x_max_mm: f64,
    pub clearance_mm: f64,
}

/// An underbody scan (STL or OBJ, metres or millimetres as `unit_mm` says) and three points on
/// it with their places on the car (jack pads, say), which fix its position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    /// Relative to the project file, or absolute.
    pub path: String,
    /// Millimetres per scan unit (1000 for a scan in metres).
    #[serde(default = "default_scan_unit")]
    pub unit_mm: f64,
    /// Reference points in scan coordinates (scan units).
    pub scan_points: [Vec3; 3],
    /// The same points in vehicle coordinates, mm.
    pub vehicle_points_mm: [Vec3; 3],
}

fn default_scan_unit() -> f64 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PipeSpec {
    pub od_mm: f64,
    pub wall_mm: f64,
    /// Material id from `materials`.
    pub material: String,
}

impl PipeSpec {
    /// Inner diameter, m.
    pub fn id_m(&self) -> f64 {
        (self.od_mm - 2.0 * self.wall_mm) * 1e-3
    }
}

/// Microphone position (ISO 5130 by default: 0.5 m from the outlet, 45° to its axis,
/// outlet height, on the outboard side).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receiver {
    /// Reference outlet element id; the first outlet when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlet: Option<String>,
    pub distance_m: f64,
    pub angle_deg: f64,
}

/// How the pipe wall exchanges heat with the gas in the time-domain solver.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "model", rename_all = "snake_case", deny_unknown_fields)]
pub enum WallThermal {
    /// No heat transfer.
    Adiabatic,
    /// Every wall at one temperature.
    Fixed { temperature_k: f64 },
    /// Walls and initial gas from the quasi-steady thermal model (`thermal`).
    Computed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SolverSettings {
    /// Target cell length, mm (10–20 mm baseline).
    pub dx_mm: f64,
    /// Courant number, ≤ 0.8.
    pub cfl: f64,
    pub limiter: Limiter,
    pub min_cycles: u32,
    pub max_cycles: u32,
    /// Converged when the last cycle differs from the one before by less than this fraction
    /// (RMS of the difference over RMS of the fluctuation) in every monitored signal.
    pub periodicity_tolerance: f64,
    /// Output samples per engine cycle (power of two).
    pub samples_per_cycle: u32,
    pub max_frequency_hz: f64,
    pub wall_roughness_mm: f64,
    pub friction: bool,
    pub wall_thermal: WallThermal,
}

/// Tube material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub family: MaterialFamily,
    pub density_kg_m3: f64,
    pub conductivity_w_mk: f64,
    /// Total hemispherical emissivity of the oxidised outer surface.
    pub emissivity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MaterialFamily {
    FerriticStainless,
    AusteniticStainless,
    Titanium,
}

impl Project {
    pub fn from_json(text: &str) -> Result<Self> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| Error::invalid(format!("project JSON: {e}")))?;
        let value = migrate(value)?;
        let project: Project =
            serde_json::from_value(value).map_err(|e| Error::invalid(format!("project: {e}")))?;
        project.validate()?;
        Ok(project)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("project serialises")
    }

    /// BLAKE3 hash of the canonical JSON without the data no solved point depends on (name,
    /// notes, input records; the sweep and cruise band; measurements; fabrication: stock, scan,
    /// hangers, joints); keys result caches and provenance.
    pub fn hash(&self) -> String {
        let mut solved = self.clone();
        (solved.name, solved.notes) = (String::new(), String::new());
        solved.inputs.clear();
        (solved.operating.sweep_rpm, solved.operating.cruise_band_rpm) = ([0.0; 3], [0.0; 2]);
        solved.fabrication = Fabrication::default();
        solved.measurements = Measurements::default();
        for r in &mut solved.system.routes {
            (r.hangers_mm, r.start_joint, r.end_joint) = (Vec::new(), None, None);
        }
        blake3::hash(
            serde_json::to_string(&solved)
                .expect("project serialises")
                .as_bytes(),
        )
        .to_hex()
        .to_string()
    }

    pub fn element(&self, id: &str) -> Option<&Element> {
        self.system.elements.iter().find(|e| e.id == id)
    }

    pub fn material(&self, id: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.id == id)
    }

    /// Full centreline vertices of a route in metres (port, via…, port).
    pub fn route_points(&self, route: &Route) -> Result<Vec<Vec3>> {
        let end = |r: &PortRef| -> Result<Vec3> {
            self.element(&r.element)
                .and_then(|e| e.port_position(&r.port))
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "route '{}': no port {}.{}",
                        route.id, r.element, r.port
                    ))
                })
        };
        let mut pts = vec![scale(end(&route.from)?, 1e-3)];
        pts.extend(route.via_mm.iter().map(|p| scale(*p, 1e-3)));
        pts.push(scale(end(&route.to)?, 1e-3));
        Ok(pts)
    }

    /// Bend radius at each interior vertex, m.
    pub fn route_radii(&self, route: &Route) -> Vec<f64> {
        (0..route.via_mm.len())
            .map(|i| {
                route
                    .bend_radius_mm
                    .get(i)
                    .copied()
                    .flatten()
                    .unwrap_or(1.5 * route.pipe.od_mm)
                    * 1e-3
            })
            .collect()
    }

    /// What is wrong with the recorded inputs without stopping a solve: defaulted ranges,
    /// ranges that cannot apply, records of fields the project does not have.
    pub fn input_warnings(&self) -> Vec<String> {
        let value = serde_json::to_value(self).expect("projects serialise");
        self.inputs
            .iter()
            .flat_map(|(path, input)| match value_at(&value, path) {
                Some(v) => input.warnings(path, &v),
                None => vec![format!("input {path}: the project has no such field")],
            })
            .collect()
    }

    /// Structural checks: unique ids, known ports, every port joined by exactly one route,
    /// known materials, physically meaningful numbers.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(Error::invalid(format!(
                "schema_version {} (expected {SCHEMA_VERSION})",
                self.schema_version
            )));
        }
        let mut ids = HashMap::new();
        for e in &self.system.elements {
            if ids.insert(e.id.as_str(), e).is_some() {
                return Err(Error::invalid(format!("duplicate element id '{}'", e.id)));
            }
        }
        let mut used: HashMap<PortRef, &str> = HashMap::new();
        let mut route_ids = HashMap::new();
        for r in &self.system.routes {
            if route_ids.insert(r.id.as_str(), ()).is_some() {
                return Err(Error::invalid(format!("duplicate route id '{}'", r.id)));
            }
            for pr in [&r.from, &r.to] {
                let e = ids.get(pr.element.as_str()).ok_or_else(|| {
                    Error::invalid(format!(
                        "route '{}': unknown element '{}'",
                        r.id, pr.element
                    ))
                })?;
                if !e.kind.has_port(&pr.port) {
                    return Err(Error::invalid(format!(
                        "route '{}': element '{}' has no port '{}'",
                        r.id, pr.element, pr.port
                    )));
                }
                if let Some(other) = used.insert(pr.clone(), &r.id) {
                    return Err(Error::invalid(format!(
                        "port {}.{} is joined by routes '{other}' and '{}'",
                        pr.element, pr.port, r.id
                    )));
                }
            }
            if !(r.pipe.od_mm > 2.0 * r.pipe.wall_mm && r.pipe.wall_mm > 0.0) {
                return Err(Error::invalid(format!(
                    "route '{}': OD must exceed twice the wall",
                    r.id
                )));
            }
            if self.material(&r.pipe.material).is_none() {
                return Err(Error::invalid(format!(
                    "route '{}': unknown material '{}'",
                    r.id, r.pipe.material
                )));
            }
            if r.bend_radius_mm.len() > r.via_mm.len() {
                return Err(Error::invalid(format!(
                    "route '{}': more bend radii than interior vertices",
                    r.id
                )));
            }
        }
        for e in &self.system.elements {
            e.validate(self)?;
            for p in e.kind.port_names() {
                let pr = PortRef {
                    element: e.id.clone(),
                    port: p.clone(),
                };
                if !used.contains_key(&pr) {
                    return Err(Error::invalid(format!(
                        "port {}.{} is not connected",
                        e.id, p
                    )));
                }
            }
        }
        let sources = self
            .system
            .elements
            .iter()
            .filter(|e| e.kind == ElementKind::Source)
            .count();
        if sources != 1 {
            return Err(Error::invalid("the system needs exactly one source"));
        }
        if !self
            .system
            .elements
            .iter()
            .any(|e| e.kind == ElementKind::Outlet)
        {
            return Err(Error::invalid("the system needs at least one outlet"));
        }
        if self.operating.map_kpa.is_empty()
            || self
                .operating
                .map_kpa
                .windows(2)
                .any(|w| w[1][0] <= w[0][0])
        {
            return Err(Error::invalid(
                "operating.map_kpa needs ≥ 1 point with increasing rpm",
            ));
        }
        let s = &self.solver;
        if !(s.dx_mm > 0.0
            && s.cfl > 0.0
            && s.cfl <= 0.8
            && s.min_cycles >= 5
            && s.max_cycles >= s.min_cycles)
        {
            return Err(Error::invalid(
                "solver: dx > 0, 0 < CFL ≤ 0.8, min_cycles ≥ 5, max_cycles ≥ min_cycles",
            ));
        }
        if !s.samples_per_cycle.is_power_of_two() || s.samples_per_cycle < 256 {
            return Err(Error::invalid(
                "solver.samples_per_cycle must be a power of two ≥ 256",
            ));
        }
        if let Some(tf) = &self.measurements.cabin_tf {
            let g = &tf.gain_db;
            if g.is_empty()
                || g.iter().any(|p| !(p[0] > 0.0 && p[1].is_finite()))
                || g.windows(2).any(|w| w[1][0] <= w[0][0])
            {
                return Err(Error::invalid(
                    "measurements.cabin_tf.gain_db needs finite gains at increasing positive frequencies",
                ));
            }
        }
        Ok(())
    }
}

/// Upgrades a project JSON value to [`SCHEMA_VERSION`], one tested step per version.
pub fn migrate(mut value: serde_json::Value) -> Result<serde_json::Value> {
    loop {
        match value.get("schema_version").and_then(|v| v.as_u64()) {
            Some(v) if v == SCHEMA_VERSION as u64 => return Ok(value),
            Some(1) => value = v1_to_v2(value),
            Some(v) => {
                return Err(Error::invalid(format!(
                    "unsupported project schema_version {v}"
                )));
            }
            None => return Err(Error::invalid("project has no schema_version")),
        }
    }
}

/// Version 2 records inputs (`inputs`) where version 1 labelled their basis: published and
/// measured keep their provenance, approximate and unknown values become estimates (with the
/// default range of their class), and the owner's own settings go unrecorded, as the user's
/// entries.
fn v1_to_v2(mut value: serde_json::Value) -> serde_json::Value {
    let project = value
        .as_object_mut()
        .expect("a versioned project is an object");
    let basis = project.remove("basis").unwrap_or_default();
    let mut inputs = serde_json::Map::new();
    for (path, b) in basis.as_object().into_iter().flatten() {
        let (provenance, source) = match b.as_str() {
            Some("published") => ("published", "published specification"),
            Some("measured") => ("measured", "measured on the vehicle"),
            Some("approximate") => ("estimated", "from memory or forum measurements"),
            Some("unknown") => ("estimated", "placeholder: not known"),
            _ => continue,
        };
        inputs.insert(
            path.clone(),
            serde_json::json!({ "provenance": provenance, "source": source }),
        );
    }
    project.insert("inputs".into(), inputs.into());
    project.insert("schema_version".into(), 2.into());
    value
}

/// Default material table (editable in the project). Room-temperature handbook values;
/// emissivity is for an oxidised, heat-cycled surface.
pub fn default_materials() -> Vec<Material> {
    let m = |id: &str, name: &str, family, density, k, eps| Material {
        id: id.into(),
        name: name.into(),
        family,
        density_kg_m3: density,
        conductivity_w_mk: k,
        emissivity: eps,
    };
    vec![
        m(
            "409",
            "AISI 409 ferritic stainless",
            MaterialFamily::FerriticStainless,
            7800.0,
            25.0,
            0.70,
        ),
        m(
            "304",
            "AISI 304 austenitic stainless",
            MaterialFamily::AusteniticStainless,
            8000.0,
            16.2,
            0.60,
        ),
        m(
            "321",
            "AISI 321 austenitic stainless",
            MaterialFamily::AusteniticStainless,
            8000.0,
            16.1,
            0.60,
        ),
        m(
            "ti-gr5",
            "Titanium Grade 5 (Ti-6Al-4V)",
            MaterialFamily::Titanium,
            4430.0,
            6.7,
            0.50,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_bases_become_input_records() {
        let v1 = serde_json::json!({
            "schema_version": 1,
            "basis": {
                "engine.geometry.bore_mm": "published",
                "engine.geometry.rod_mm": "approximate",
                "engine.valves.evo_deg": "unknown",
                "operating.cruise_band_rpm": "owner_set",
            },
        });
        let v2 = migrate(v1).unwrap();
        assert_eq!(v2["schema_version"], 2);
        let inputs = v2["inputs"].as_object().unwrap();
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs["engine.geometry.bore_mm"]["provenance"], "published");
        assert_eq!(inputs["engine.geometry.rod_mm"]["provenance"], "estimated");
        assert_eq!(
            inputs["engine.valves.evo_deg"]["source"],
            "placeholder: not known"
        );
    }
}
