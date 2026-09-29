//! Element library. Each element adds its ducts and nodes to the network once; the
//! time-domain solver integrates them and the four-pole solver linearises the same objects,
//! so an element's 1D flow model and its four-pole model cannot drift apart.
//!
//! Acoustic end corrections that a plane-wave model cannot produce are added as length:
//! a side branch's mouth into the main pipe `0.6 r` (the directive's stub value), a Helmholtz
//! neck `0.6 r` at the pipe and `0.85 r` into the cavity (flanged piston). Extended-tube and
//! chamber evanescent-mode corrections are not modelled.

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::error::{Error, Result};
use crate::gas::Gas;
use crate::gas1d::nodes::JunctionKind;
use crate::gas1d::{Duct, JunctionBc, Node, Port, WallSpec};
use crate::project::{
    Catalyst, Element, ElementKind, ExpansionChamber, Helmholtz, PipeSpec, PortRef, Project,
    QuarterWaveStub, Valve, WallThermal,
};

/// Poiseuille number `f·Re` of fully developed laminar flow in a square channel
/// (Shah & London 1978, Table 42).
pub const SQUARE_CHANNEL_FRE: f64 = 14.227;
/// Incremental pressure-drop number `K(∞)` of hydrodynamically developing flow in a square
/// channel (Shah & London 1978): the entrance region adds `K(∞) ½ρu²`.
pub const SQUARE_CHANNEL_K_INF: f64 = 1.43;

/// Network under construction.
pub struct Builder<'a> {
    pub project: &'a Project,
    pub gas: Gas,
    pub dx: f64,
    pub ducts: Vec<Duct>,
    pub nodes: Vec<Node>,
    /// Duct end at each element port.
    pub ports: HashMap<PortRef, Port>,
    /// Pipe connected to each element port.
    pub pipes: HashMap<PortRef, PipeSpec>,
    pub warnings: Vec<String>,
}

impl Builder<'_> {
    pub fn port(&self, el: &Element, name: &str) -> Port {
        self.ports[&PortRef {
            element: el.id.clone(),
            port: name.into(),
        }]
    }

    fn pipe(&self, el: &Element, name: &str) -> &PipeSpec {
        &self.pipes[&PortRef {
            element: el.id.clone(),
            port: name.into(),
        }]
    }

    /// Inner diameter at a duct end, m.
    pub fn bore(&self, port: Port) -> f64 {
        (4.0 * self.ducts[port.duct].end_area(port.end) / PI).sqrt()
    }

    /// Wall of a pipe spec for the thermal model.
    pub fn pipe_wall(&self, od: f64, thickness: f64, material: &str) -> Option<WallSpec> {
        self.project.material(material).map(|m| WallSpec {
            outer_diameter: od,
            thickness,
            conductivity: m.conductivity_w_mk,
            emissivity: m.emissivity,
        })
    }

    /// Adds a duct with the project's friction and fixed-wall settings.
    pub fn duct(&mut self, mut duct: Duct) -> usize {
        if self.project.solver.friction {
            duct.friction = true;
            duct.roughness = self.project.solver.wall_roughness_mm * 1e-3;
        }
        if let WallThermal::Fixed { temperature_k } = self.project.solver.wall_thermal
            && duct.channel.is_none()
        {
            duct.t_wall = Some(vec![temperature_k; duct.n()]);
        }
        self.ducts.push(duct);
        self.ducts.len() - 1
    }

    pub fn node(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    fn junction(&mut self, ports: Vec<Port>, kind: JunctionKind) -> usize {
        self.node(Node::Junction(JunctionBc { ports, kind }))
    }

    /// Joins two duct ends: plain continuity for equal bores, the sudden area-change model
    /// otherwise.
    pub fn join(&mut self, a: Port, b: Port) -> usize {
        self.junction(vec![a, b], JunctionKind::AreaChange)
    }

    fn straight(&self, label: String, length: f64, d: f64) -> Duct {
        Duct::conical(label, length, d, d, self.dx)
    }
}

fn circle(d: f64) -> f64 {
    PI / 4.0 * d * d
}

/// Adds the element's ducts and nodes. Source and outlet elements are handled by
/// `model::build`, which also records them.
pub fn build(el: &Element, b: &mut Builder) -> Result<()> {
    match &el.kind {
        ElementKind::Cap => {
            let p = b.port(el, "in");
            b.node(Node::Wall(p));
        }
        ElementKind::ExpansionChamber(c) => chamber(el, c, b)?,
        ElementKind::QuarterWaveStub(s) => stub(el, s, b),
        ElementKind::Helmholtz(h) => helmholtz(el, h, b),
        ElementKind::Catalyst(c) => catalyst(el, c, b),
        ElementKind::Valve(v) => valve(el, v, b),
        ElementKind::Tee(_) => {
            let ports = vec![b.port(el, "in"), b.port(el, "out"), b.port(el, "branch")];
            b.junction(ports, JunctionKind::ConstantPressure);
        }
        ElementKind::Source | ElementKind::Outlet | ElementKind::AreaChange { .. } => {
            return Err(Error::invalid(format!(
                "element '{}' is built by the model",
                el.id
            )));
        }
    }
    Ok(())
}

/// Expansion chamber: a duct of the shell bore between the tube ends. An extended tube is a
/// pipe duct inside the chamber; at its end a constant-pressure junction joins it to the
/// chamber and to the annular cavity around it, which is closed at the chamber end wall.
/// Without extensions the pipes meet the chamber through sudden area changes.
fn chamber(el: &Element, c: &ExpansionChamber, b: &mut Builder) -> Result<()> {
    let dc = c.diameter_mm * 1e-3;
    let (e_in, e_out) = (c.inlet_extension_mm * 1e-3, c.outlet_extension_mm * 1e-3);
    let pin_pipe = b.pipe(el, "in").clone();
    let shell = b.pipe_wall(
        dc + 2.0 * c.shell_mm * 1e-3,
        c.shell_mm * 1e-3,
        &pin_pipe.material,
    );
    let mut main_duct = b.straight(
        format!("{} (chamber)", el.id),
        c.length_mm * 1e-3 - e_in - e_out,
        dc,
    );
    main_duct.wall = shell;
    let main = b.duct(main_duct);
    // An extended tube of the given pipe, its annular cavity, and the tube-end junction.
    let extension =
        |b: &mut Builder, port: Port, pipe: &PipeSpec, length: f64, side: &str, main_end: Port| {
            let d = pipe.id_m();
            let od = pipe.od_mm * 1e-3;
            let tube = b.duct(b.straight(format!("{} ({side} tube)", el.id), length, d));
            // The annulus lies between the hot tube and the shell; its shell losses are carried by
            // the main chamber duct, so it has no wall of its own.
            let mut annulus = b.straight(
                format!("{} ({side} annulus)", el.id),
                length,
                (dc * dc - od * od).sqrt(),
            );
            annulus.diameter.iter_mut().for_each(|h| *h = dc - od);
            let ann = b.duct(annulus);
            b.join(
                port,
                if side == "inlet" {
                    Port::start(tube)
                } else {
                    Port::end(tube)
                },
            );
            let tube_inner = if side == "inlet" {
                Port::end(tube)
            } else {
                Port::start(tube)
            };
            b.junction(
                vec![tube_inner, main_end, Port::start(ann)],
                JunctionKind::ConstantPressure,
            );
            b.node(Node::Wall(Port::end(ann)));
        };
    let pin = b.port(el, "in");
    if e_in > 0.0 {
        extension(b, pin, &pin_pipe, e_in, "inlet", Port::start(main));
    } else {
        b.join(pin, Port::start(main));
    }
    let outs: Vec<Port> = el
        .kind
        .port_names()
        .iter()
        .filter(|p| p.starts_with("out"))
        .map(|p| b.port(el, p))
        .collect();
    if outs.len() > 1 {
        let mut ports = vec![Port::end(main)];
        ports.extend(outs);
        b.junction(ports, JunctionKind::ConstantPressure);
    } else if e_out > 0.0 {
        let pipe = b.pipe(el, "out").clone();
        extension(b, outs[0], &pipe, e_out, "outlet", Port::end(main));
    } else {
        b.join(Port::end(main), outs[0]);
    }
    Ok(())
}

/// Quarter-wave stub: a closed duct of length `L + 0.6 r` on a constant-pressure tee.
fn stub(el: &Element, s: &QuarterWaveStub, b: &mut Builder) {
    let d = s.id_mm * 1e-3;
    let mut duct = b.straight(
        format!("{} (stub)", el.id),
        s.length_mm * 1e-3 + 0.6 * d / 2.0,
        d,
    );
    duct.wall = b.pipe_wall(d + 2.0 * s.wall_mm * 1e-3, s.wall_mm * 1e-3, &s.material);
    let branch = b.duct(duct);
    let ports = vec![b.port(el, "in"), b.port(el, "out"), Port::start(branch)];
    b.junction(ports, JunctionKind::ConstantPressure);
    b.node(Node::Wall(Port::end(branch)));
}

/// Helmholtz resonator: neck of effective length `l + 0.6 r + 0.85 r` on a constant-pressure
/// tee, sudden expansion into a cylindrical cavity, closed end.
fn helmholtz(el: &Element, h: &Helmholtz, b: &mut Builder) {
    let dn = h.neck_id_mm * 1e-3;
    let dc = h.cavity_diameter_mm * 1e-3;
    let pipe = b.pipe(el, "in").clone();
    let mut neck = b.straight(
        format!("{} (neck)", el.id),
        h.neck_length_mm * 1e-3 + 1.45 * dn / 2.0,
        dn,
    );
    neck.wall = b.pipe_wall(
        dn + 2.0 * pipe.wall_mm * 1e-3,
        pipe.wall_mm * 1e-3,
        &pipe.material,
    );
    let neck = b.duct(neck);
    let mut cavity = b.straight(
        format!("{} (cavity)", el.id),
        h.volume_l * 1e-3 / circle(dc),
        dc,
    );
    cavity.wall = b.pipe_wall(dc + 2.4e-3, 1.2e-3, &pipe.material);
    let cavity = b.duct(cavity);
    let ports = vec![b.port(el, "in"), b.port(el, "out"), Port::start(neck)];
    b.junction(ports, JunctionKind::ConstantPressure);
    b.join(Port::end(neck), Port::start(cavity));
    b.node(Node::Wall(Port::end(cavity)));
}

/// Catalyst: cones between pipe and can, and a brick of square channels. The brick is a duct
/// of the open frontal area `OFA · A_can`, `OFA = (w/pitch)²`, hydraulic diameter `w`
/// (channel width), laminar channel friction (`f Re = 14.227`) plus the developing-flow
/// entrance loss `K(∞) = 1.43`. Faces are sudden area changes (contraction into the channels,
/// Borda–Carnot expansion out). The can is insulated: no heat loss.
fn catalyst(el: &Element, c: &Catalyst, b: &mut Builder) {
    let pitch = 25.4e-3 / c.cpsi.sqrt();
    let w = pitch - c.cell_wall_mm * 1e-3;
    let ofa = (w / pitch).powi(2);
    let db = c.brick_diameter_mm * 1e-3;
    let open = ofa * circle(db);
    let pin = b.port(el, "in");
    let pout = b.port(el, "out");
    let mut face_in = pin;
    if c.inlet_cone_mm > 0.0 {
        let d = b.bore(pin);
        let cone = b.duct(Duct::conical(
            format!("{} (inlet cone)", el.id),
            c.inlet_cone_mm * 1e-3,
            d,
            db,
            b.dx,
        ));
        b.join(pin, Port::start(cone));
        face_in = Port::end(cone);
    }
    let d_open = (4.0 * open / PI).sqrt();
    let mut brick = b.straight(
        format!("{} (brick)", el.id),
        c.brick_length_mm * 1e-3,
        d_open,
    );
    brick.diameter.iter_mut().for_each(|h| *h = w);
    brick.channel = Some(SQUARE_CHANNEL_FRE);
    brick.k_loss[0] += SQUARE_CHANNEL_K_INF;
    let brick = b.duct(brick);
    b.join(face_in, Port::start(brick));
    if c.outlet_cone_mm > 0.0 {
        let d = b.bore(pout);
        let cone = b.duct(Duct::conical(
            format!("{} (outlet cone)", el.id),
            c.outlet_cone_mm * 1e-3,
            db,
            d,
            b.dx,
        ));
        b.join(Port::end(brick), Port::start(cone));
        b.join(Port::end(cone), pout);
    } else {
        b.join(Port::end(brick), pout);
    }
}

/// Butterfly valve loss coefficient on the pipe velocity against closure angle from fully
/// open (Idelchik 1986, diagram 9-17, circular pipe): `(δ°, ζ)`.
const BUTTERFLY_ZETA: [(f64, f64); 15] = [
    (0.0, 0.20),
    (5.0, 0.24),
    (10.0, 0.52),
    (15.0, 0.90),
    (20.0, 1.54),
    (25.0, 2.51),
    (30.0, 3.91),
    (35.0, 6.22),
    (40.0, 10.8),
    (45.0, 18.7),
    (50.0, 32.6),
    (55.0, 58.8),
    (60.0, 118.0),
    (65.0, 256.0),
    (70.0, 751.0),
];

/// Fraction of the pipe area left open by a closed valve (seat leakage).
pub const VALVE_LEAKAGE: f64 = 1e-3;

/// Loss coefficient of a butterfly valve opened to `angle_deg` (90° open). Idelchik's table up
/// to 70° closure (log-linear between points); beyond it the gap geometry, open area
/// `A (1 − sin δ)` with `C_d = 0.6`, which gives 707 at 70° against the table's 751.
pub fn butterfly_loss(angle_deg: f64) -> f64 {
    let delta = 90.0 - angle_deg.clamp(0.0, 90.0);
    if delta <= 70.0 {
        let k = BUTTERFLY_ZETA
            .partition_point(|p| p.0 <= delta)
            .clamp(1, BUTTERFLY_ZETA.len() - 1);
        let (a, b) = (BUTTERFLY_ZETA[k - 1], BUTTERFLY_ZETA[k]);
        let w = (delta - a.0) / (b.0 - a.0);
        (a.1.ln() * (1.0 - w) + b.1.ln() * w).exp()
    } else {
        let open = (1.0 - delta.to_radians().sin()).max(VALVE_LEAKAGE);
        (1.0 / (0.6 * open) - 1.0).powi(2)
    }
}

/// Butterfly valve as an orifice junction whose effective area reproduces the loss:
/// `K = (A/A_e − 1)²` between equal pipes ⇒ `A_e = A/(1 + √K)`.
fn valve(el: &Element, v: &Valve, b: &mut Builder) {
    let (pin, pout) = (b.port(el, "in"), b.port(el, "out"));
    let a = circle(b.bore(pin).min(b.bore(pout)));
    let k = butterfly_loss(v.angle_deg);
    b.junction(
        vec![pin, pout],
        JunctionKind::Orifice {
            effective_area: a / (1.0 + k.sqrt()),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn butterfly_table_and_gap_model_meet() {
        assert!((butterfly_loss(90.0) - 0.20).abs() < 1e-12);
        assert!((butterfly_loss(60.0) - 3.91).abs() < 1e-9);
        let (table, gap) = (
            butterfly_loss(20.0),
            (1.0 / (0.6 * (1.0 - 70f64.to_radians().sin())) - 1.0).powi(2),
        );
        assert!((table / gap - 1.0).abs() < 0.07);
        assert!(butterfly_loss(0.0) > 1e5);
    }
}
