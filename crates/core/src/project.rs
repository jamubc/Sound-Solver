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

/// Current project schema version.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Origin of each value, keyed by dotted field path (e.g. `engine.geometry.bore_mm`).
    /// The UI shows the label next to the value; unlisted values are user input.
    #[serde(default)]
    pub basis: BTreeMap<String, Basis>,
    pub engine: EngineSpec,
    pub turbine: TurbineSpec,
    pub gas: GasSpec,
    pub ambient: Ambient,
    pub operating: Operating,
    pub system: System,
    pub receiver: Receiver,
    pub solver: SolverSettings,
    pub materials: Vec<Material>,
}

/// Where a value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// Manufacturer or standard publication.
    Published,
    /// From memory or forum measurements; replace with a measurement.
    Approximate,
    /// Measured on the vehicle by the owner.
    Measured,
    /// Chosen by the owner.
    OwnerSet,
    /// Not known; a placeholder the results depend on.
    Unknown,
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
    /// Height of the ground plane in vehicle coordinates, mm (negative: below the flange).
    pub ground_z_mm: f64,
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

/// Element types. Each has fixed port names.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ElementKind {
    /// Engine and turbine; port `out` is the downpipe flange face.
    Source,
    /// Unflanged tailpipe tip radiating to atmosphere; port `in`.
    Outlet,
    /// Joins two pipes of different bore: sudden when `taper_length_mm` is 0, otherwise a
    /// cone of that length. Ports `in`, `out`; bores come from the connected pipes.
    AreaChange { taper_length_mm: f64 },
    /// Closed pipe end; port `in`.
    Cap,
}

impl ElementKind {
    pub fn port_names(&self) -> &'static [&'static str] {
        match self {
            ElementKind::Source => &["out"],
            ElementKind::Outlet | ElementKind::Cap => &["in"],
            ElementKind::AreaChange { .. } => &["in", "out"],
        }
    }
}

impl Element {
    /// Position of a named port, mm.
    pub fn port_position(&self, port: &str) -> Option<Vec3> {
        let axis = unit(self.axis);
        match (&self.kind, port) {
            (ElementKind::AreaChange { taper_length_mm }, "out") => {
                Some(add(self.position_mm, scale(axis, *taper_length_mm)))
            }
            (kind, p) if kind.port_names().contains(&p) => Some(self.position_mm),
            _ => None,
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

    /// BLAKE3 hash of the canonical JSON; keys result caches and provenance.
    pub fn hash(&self) -> String {
        blake3::hash(
            serde_json::to_string(self)
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
                if !e.kind.port_names().contains(&pr.port.as_str()) {
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
            for p in e.kind.port_names() {
                let pr = PortRef {
                    element: e.id.clone(),
                    port: (*p).to_string(),
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
        Ok(())
    }
}

/// Upgrades a project JSON value to [`SCHEMA_VERSION`]. Version 1 is the first; each future
/// version adds one tested step here.
pub fn migrate(value: serde_json::Value) -> Result<serde_json::Value> {
    match value.get("schema_version").and_then(|v| v.as_u64()) {
        Some(v) if v == SCHEMA_VERSION as u64 => Ok(value),
        Some(v) => Err(Error::invalid(format!(
            "unsupported project schema_version {v}"
        ))),
        None => Err(Error::invalid("project has no schema_version")),
    }
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
