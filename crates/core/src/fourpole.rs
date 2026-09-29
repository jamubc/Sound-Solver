//! Linear frequency-domain acoustics (transfer-matrix / four-pole method) of the same network
//! the time-domain solver integrates.
//!
//! Every duct and node is linearised about a mean state ([`MeanState`]): the cycle-averaged
//! time-domain solution, or the thermal model's steady profile. There is no second geometry.
//!
//! Duct cell of length `Δx`, area `S`, mean density `ρ`, sound speed `c`, velocity `U = Mc`
//! (Munjal 2014, ch. 2–3): plane waves `p = A e^{−jk₊x} + B e^{jk₋x}`,
//! `ρc u = A e^{−jk₊x} − B e^{jk₋x}`, `k± = k/(1 ± M)`, so from `x = 0` to `x = Δx`
//! ```text
//! [p; ρcu]_Δx = ½ [e₁ + e₂, e₁ − e₂; e₁ − e₂, e₁ + e₂] [p; ρcu]_0,   e₁ = e^{−jk₊Δx}, e₂ = e^{jk₋Δx}
//! ```
//! with wall losses when the duct has friction on (the time-domain switch):
//! `k = ω/c + (1 − j)α_v − j(α_f + α_K)`, Kirchhoff's viscothermal
//! `α_v = (1/(r c)) √(ων/2) (1 + (γ−1)/√Pr)`, the quasi-steady wall shear linearised about the
//! mean flow `α_f = (4/D)(∂τ_w/∂u)/(2ρc)` (the time domain's `wall::shear_stress`), and bend
//! losses `α_K = K |U| / (2 c Δx)` per cell (always, as in the time domain). Cells are joined by
//! continuity of `p` and the acoustic mass flow `m' = (S/c)(ρc u + M p)`.
//!
//! Nodes, with `q` the acoustic mass flow toward the node and `M_t` the mean Mach number toward
//! it (matching the time-domain nodes):
//! - wall: `q = 0`; ideal open end: `p = 0`;
//! - radiating outlet: `p = z_r ρc v`, `z_r = (1 + R)/(1 − R)` with `R` the Levine–Schwinger
//!   fit used by the time-domain boundary (`radiation::LsFilter::response`);
//! - constant-pressure junction: equal `p`, `Σq = 0`;
//! - area change / orifice: `Σq = 0` and the linearised stagnation-pressure relation
//!   `δp_u − δp_d + ρ(1 − K) V_u δv_u − ρ V_d δv_d = 0` (`K` from `nodes::step_loss`);
//! - characteristic end: `p (1 + M_t) − (c/S) q = W_in` (no reflection, incident wave `W_in`);
//! - sources: Norton form, mass flow into the duct `Q − Y p`.

use nalgebra::{DMatrix, DVector};
use rustfft::num_complex::Complex64 as C64;

use crate::error::{Error, Result};
use crate::gas1d::nodes::step_loss;
use crate::gas1d::wall::shear_stress;
use crate::gas1d::{End, JunctionKind, Network, Node, Port};
use crate::radiation::LsFilter;

/// Linearisation point of one cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMean {
    pub rho: f64,
    pub c: f64,
    /// Velocity along the duct's +x, m/s.
    pub u: f64,
    pub t: f64,
}

/// Mean state of every cell of a network.
#[derive(Clone, Debug)]
pub struct MeanState {
    pub cells: Vec<Vec<CellMean>>,
}

impl MeanState {
    /// Gas at rest at `(p, t)` everywhere.
    pub fn quiescent(net: &Network, p: f64, t: f64) -> Self {
        let rho = p / (net.gas.r() * t);
        let c = net.gas.sound_speed(t);
        let cell = CellMean { rho, c, u: 0.0, t };
        Self {
            cells: net.ducts.iter().map(|d| vec![cell; d.n()]).collect(),
        }
    }

    fn at(&self, port: Port, net: &Network) -> CellMean {
        let cells = &self.cells[port.duct];
        match port.end {
            End::Start => cells[0],
            End::End => cells[net.ducts[port.duct].n() - 1],
        }
    }
}

/// How a node is driven at the solve frequency.
#[derive(Clone, Copy, Debug)]
pub enum Drive {
    /// Norton source at a `Source` or `MassFlow` node: mass flow into the duct `q − y p`.
    Norton { q: C64, y: C64 },
    /// Incident wave `W_in = 2p⁺` sent into the duct by a `Characteristic` node.
    Incident(C64),
}

/// Acoustic pressure and mass flow `(p, m')` at the start and end of every duct.
#[derive(Clone, Debug)]
pub struct Solution {
    pub ends: Vec<[[C64; 2]; 2]>,
}

impl Solution {
    /// Pressure and acoustic mass flow toward the node at a port.
    pub fn at_port(&self, port: Port) -> (C64, C64) {
        let e = &self.ends[port.duct];
        match port.end {
            End::Start => (e[0][0], -e[0][1]),
            End::End => (e[1][0], e[1][1]),
        }
    }

    /// Volume velocity leaving the duct through a port, m³/s (`v S`, as the time-domain
    /// receiver uses the face velocity).
    pub fn volume_velocity_out(&self, net: &Network, mean: &MeanState, port: Port) -> C64 {
        let (p, q) = self.at_port(port);
        let m = mean.at(port, net);
        let s = net.ducts[port.duct].end_area(port.end);
        let mt = toward(port, m.u) / m.c;
        (m.c * q / s - mt * p) / (m.rho * m.c) * s
    }
}

fn toward(port: Port, u: f64) -> f64 {
    match port.end {
        End::Start => -u,
        End::End => u,
    }
}

type M2 = [[C64; 2]; 2];

fn mul(a: &M2, b: &M2) -> M2 {
    [
        [
            a[0][0] * b[0][0] + a[0][1] * b[1][0],
            a[0][0] * b[0][1] + a[0][1] * b[1][1],
        ],
        [
            a[1][0] * b[0][0] + a[1][1] * b[1][0],
            a[1][0] * b[0][1] + a[1][1] * b[1][1],
        ],
    ]
}

/// Start-to-end matrix of cell `i` of duct `d` over length `len`, in `(p, m')`.
fn cell_matrix(net: &Network, mean: &MeanState, d: usize, i: usize, omega: f64, len: f64) -> M2 {
    let duct = &net.ducts[d];
    let gas = &net.gas;
    let m = &mean.cells[d][i];
    let one = C64::new(1.0, 0.0);
    let s = duct.volume[i] / duct.dx;
    let r = 0.5 * duct.diameter[i];
    let mach = m.u / m.c;
    let alpha_k = duct.k_loss[i] * m.u.abs() / (2.0 * m.c * duct.dx);
    if let Some(fill) = &duct.porous {
        return porous_cell(gas, m, fill, s, len, omega);
    }
    if let (Some(fre), true) = (duct.channel, duct.friction) {
        // Radius whose Poiseuille resistance 8μ/r² equals the channel's 2(fRe)μ/D_h².
        let r_eff = 2.0 * duct.diameter[i] / fre.sqrt();
        return channel_cell(gas, m, r_eff, s, len, omega, alpha_k);
    }
    let (mut alpha_v, mut alpha_r) = (0.0, alpha_k);
    if duct.friction {
        let gamma = gas.gamma(m.t);
        let mu = gas.viscosity(m.t);
        alpha_v = (omega * mu / m.rho / 2.0).sqrt() * (1.0 + (gamma - 1.0) / gas.prandtl().sqrt())
            / (r * m.c);
        let du = 1e-3 * m.c;
        let dtau = (shear_stress(m.rho, m.u + du, mu, 2.0 * r, duct.roughness)
            - shear_stress(m.rho, m.u - du, mu, 2.0 * r, duct.roughness))
            / (2.0 * du);
        alpha_r += (4.0 / (2.0 * r)) * dtau / (2.0 * m.rho * m.c);
    }
    let k = C64::new(omega / m.c + alpha_v, -alpha_v - alpha_r);
    let e1 = (C64::new(0.0, -1.0) * k / (1.0 + mach) * len).exp();
    let e2 = (C64::new(0.0, 1.0) * k / (1.0 - mach) * len).exp();
    let p: M2 = [
        [0.5 * (e1 + e2), 0.5 * (e1 - e2)],
        [0.5 * (e1 - e2), 0.5 * (e1 + e2)],
    ];
    // (p, ρcu) → (p, m'):  m' = (S/c)(ρcu + M p).
    let c_mat: M2 = [
        [one, C64::new(0.0, 0.0)],
        [C64::new(s * mach / m.c, 0.0), C64::new(s / m.c, 0.0)],
    ];
    let c_inv: M2 = [
        [one, C64::new(0.0, 0.0)],
        [C64::new(-mach, 0.0), C64::new(m.c / s, 0.0)],
    ];
    mul(&mul(&c_mat, &p), &c_inv)
}

/// Start-to-end transfer matrix of duct `d` in `(p, m')`.
pub fn duct_matrix(net: &Network, mean: &MeanState, d: usize, omega: f64) -> M2 {
    let one = C64::new(1.0, 0.0);
    let mut acc: M2 = [[one, C64::new(0.0, 0.0)], [C64::new(0.0, 0.0), one]];
    for i in 0..net.ducts[d].n() {
        acc = mul(&cell_matrix(net, mean, d, i, omega, net.ducts[d].dx), &acc);
    }
    acc
}

/// Fibrous fill (`gas1d::porous`): the pore gas as an equivalent fluid of density `ρ̃` and bulk
/// modulus `K̃` on the pore area `S`: `k = ω√(ρ̃/K̃)`, `p/m' = √(ρ̃K̃)/(ρS)`.
fn porous_cell(
    gas: &crate::gas::Gas,
    m: &CellMean,
    fill: &crate::gas1d::Porous,
    pore_area: f64,
    len: f64,
    omega: f64,
) -> M2 {
    let (rho_e, k_e) = fill.equivalent_fluid(gas, m.rho, m.t, omega);
    let k = omega * (rho_e / k_e).sqrt();
    let zm = (rho_e * k_e).sqrt() / (m.rho * pore_area);
    let (cs, sn) = ((k * len).cos(), (k * len).sin());
    let j = C64::new(0.0, 1.0);
    [[cs, -j * zm * sn], [-j * sn / zm, cs]]
}

fn inv2(a: &M2) -> M2 {
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    [
        [a[1][1] / det, -a[0][1] / det],
        [-a[1][0] / det, a[0][0] / det],
    ]
}

fn add2(a: &M2, b: &M2) -> M2 {
    [
        [a[0][0] + b[0][0], a[0][1] + b[0][1]],
        [a[1][0] + b[1][0], a[1][1] + b[1][1]],
    ]
}

fn sub2(a: &M2, b: &M2) -> M2 {
    [
        [a[0][0] - b[0][0], a[0][1] - b[0][1]],
        [a[1][0] - b[1][0], a[1][1] - b[1][1]],
    ]
}

/// Impedance matrix of a two-channel segment in 2×2 blocks: pressures `(p_s, p_e)` of both
/// channels at its start and end against the mass flows into it `(m_s, −m_e)`:
/// `p_s = Z₁₁ F_s + Z₁₂ F_e`, `p_e = Z₂₁ F_s + Z₂₂ F_e`.
type Impedance = [[M2; 2]; 2];

/// Impedance matrix of one cell of a perforate-coupled pair (inner duct `d`, its partner):
/// half-cell propagation in each channel, the perforate at the cell centre as a transverse
/// mass flow `ρ A_s (p_in − p_out)/z_p`, `z_p = ρc (θ + j k l_e/σ)` (`gas1d::Perforate`,
/// linear part), the other half cell. From the cell's transfer blocks `p_e = A p_s + B m_s`,
/// `m_e = C p_s + D m_s`: `Z₁₁ = −C⁻¹D`, `Z₁₂ = −C⁻¹`, `Z₂₁ = B − AC⁻¹D`, `Z₂₂ = −AC⁻¹`.
fn coupled_cell(net: &Network, mean: &MeanState, d: usize, i: usize, omega: f64) -> Impedance {
    let duct = &net.ducts[d];
    let perf = duct.perforate.expect("coupled duct");
    let (hi, ho) = (
        cell_matrix(net, mean, d, i, omega, 0.5 * duct.dx),
        cell_matrix(net, mean, perf.partner, i, omega, 0.5 * duct.dx),
    );
    let m = &mean.cells[d][i];
    let zp = m.rho
        * m.c
        * C64::new(
            perf.resistance(m.u / m.c),
            omega / m.c * perf.effective_length() / perf.porosity,
        );
    let y = m.rho * perf.perimeter * duct.dx / zp;
    let zero = C64::new(0.0, 0.0);
    // Blocks over the channels (inner, outer): each half cell is diagonal in the channels.
    let half = |h_i: &M2, h_o: &M2| -> [[M2; 2]; 2] {
        [
            [
                [[h_i[0][0], zero], [zero, h_o[0][0]]],
                [[h_i[0][1], zero], [zero, h_o[0][1]]],
            ],
            [
                [[h_i[1][0], zero], [zero, h_o[1][0]]],
                [[h_i[1][1], zero], [zero, h_o[1][1]]],
            ],
        ]
    };
    let h = half(&hi, &ho);
    // Shunt: m ← m − Y p with Y = [[y, −y], [−y, y]]; with mean flow the extracted mass raises
    // the pressure by U ΔM/(S(1 − M²)) (momentum ρ(∂ₜ + U∂ₓ)u + ∂ₓp = 0 across the point).
    let shunt_y: M2 = [[y, -y], [-y, y]];
    let jump = |k: usize, c: usize| {
        let (m, s) = (&mean.cells[k][i], net.ducts[k].volume[i] / net.ducts[k].dx);
        shunt_y[c].map(|v| v * m.u / (s * (1.0 - (m.u / m.c).powi(2))))
    };
    let shunt_p: M2 = [jump(d, 0), jump(perf.partner, 1)];
    // T = H · S · H with S = [[I + DY, 0], [−Y, I]]: compose block by block.
    let apply = |t: &[[M2; 2]; 2]| -> [[M2; 2]; 2] {
        // S · T
        [
            [
                add2(&t[0][0], &mul(&shunt_p, &t[0][0])),
                add2(&t[0][1], &mul(&shunt_p, &t[0][1])),
            ],
            [
                sub2(&t[1][0], &mul(&shunt_y, &t[0][0])),
                sub2(&t[1][1], &mul(&shunt_y, &t[0][1])),
            ],
        ]
    };
    let compose = |a: &[[M2; 2]; 2], b: &[[M2; 2]; 2]| -> [[M2; 2]; 2] {
        let e = |r: usize, c: usize| add2(&mul(&a[r][0], &b[0][c]), &mul(&a[r][1], &b[1][c]));
        [[e(0, 0), e(0, 1)], [e(1, 0), e(1, 1)]]
    };
    let t = compose(&h, &apply(&h));
    let (a, b, c, dd) = (t[0][0], t[0][1], t[1][0], t[1][1]);
    let ci = inv2(&c);
    let neg = |x: &M2| x.map(|r| r.map(|z| -z));
    [
        [neg(&mul(&ci, &dd)), neg(&ci)],
        [sub2(&b, &mul(&mul(&a, &ci), &dd)), neg(&mul(&a, &ci))],
    ]
}

/// Joins two segments at their shared node (continuous pressure, mass flow out of the first
/// into the second) by eliminating it: with `S = Z¹₂₂ + Z²₁₁`, the node flow is
/// `S⁻¹(Z¹₂₁ F_s − Z²₁₂ F_e)`. Stable for passive segments, unlike chaining transfer matrices,
/// whose entries grow like the fastest evanescent mode over the whole length.
fn combine(z1: &Impedance, z2: &Impedance) -> Impedance {
    let si = inv2(&add2(&z1[1][1], &z2[0][0]));
    [
        [
            sub2(&z1[0][0], &mul(&mul(&z1[0][1], &si), &z1[1][0])),
            mul(&mul(&z1[0][1], &si), &z2[0][1]),
        ],
        [
            mul(&mul(&z2[1][0], &si), &z1[1][0]),
            sub2(&z2[1][1], &mul(&mul(&z2[1][0], &si), &z2[0][1])),
        ],
    ]
}

/// Impedance matrix of the perforated inner duct `d` of an absorptive pair whose annulus is
/// closed at both ends: `p_s = z₁₁F_s + z₁₂F_e`, `p_e = z₂₁F_s + z₂₂F_e`, flows `F` into the duct.
/// Kept in this form because at high attenuation the transfer form cancels to round-off.
fn coupled_inner_impedance(net: &Network, mean: &MeanState, d: usize, omega: f64) -> M2 {
    let mut z = coupled_cell(net, mean, d, 0, omega);
    for i in 1..net.ducts[d].n() {
        z = combine(&z, &coupled_cell(net, mean, d, i, omega));
    }
    [
        [z[0][0][0][0], z[0][1][0][0]],
        [z[1][0][0][0], z[1][1][0][0]],
    ]
}

/// Viscothermal function of a circular tube, `F(s) = 2 J₁(σ)/(σ J₀(σ))`, `σ = s √(−j)`
/// (Zwikker & Kosten 1949; Tijdeman 1975 low-reduced-frequency model).
fn zk_function(s: f64) -> C64 {
    let sigma = s * C64::from_polar(1.0, -std::f64::consts::FRAC_PI_4);
    let (j0, j1) = crate::math::bessel_j01_complex(sigma);
    2.0 * j1 / (sigma * j0)
}

/// Start-to-end matrix of one cell of a laminar channel bundle (catalyst brick) with no mean
/// convection: effective density `ρ/(1 − F(s))` and compressibility
/// `(1 + (γ − 1)F(s√Pr))/(γp)`, `s = r √(ωρ/μ)`, with the circular-tube functions at the
/// radius `r` that reproduces the channel's Poiseuille resistance (Stinson & Champoux 1992
/// shape-factor approach). Its low-frequency limit is the time-domain channel friction.
/// `alpha_k` adds the linearised entrance loss.
fn channel_cell(
    gas: &crate::gas::Gas,
    m: &CellMean,
    r: f64,
    area: f64,
    dx: f64,
    omega: f64,
    alpha_k: f64,
) -> M2 {
    let gamma = gas.gamma(m.t);
    let mu = gas.viscosity(m.t);
    let p0 = m.rho * gas.r() * m.t;
    let s = r * (omega * m.rho / mu).sqrt();
    let rho_e = m.rho / (1.0 - zk_function(s));
    let comp = (1.0 + (gamma - 1.0) * zk_function(s * gas.prandtl().sqrt())) / (gamma * p0);
    // Z = ωρ_e/k pairs with either root of k; the matrix is even in (k, Z).
    let k0 = omega * (rho_e * comp).sqrt();
    let zm = omega * rho_e / k0 / (m.rho * area);
    let k = k0 - C64::new(0.0, alpha_k);
    let (cs, sn) = ((k * dx).cos(), (k * dx).sin());
    let j = C64::new(0.0, 1.0);
    [[cs, -j * zm * sn], [-j * sn / zm, cs]]
}

/// A port's `p` and `q` (toward the node) as linear forms in its duct's two unknowns.
#[derive(Clone, Copy)]
struct Form {
    idx: [usize; 2],
    p: [C64; 2],
    q: [C64; 2],
}

/// How a duct's two unknowns give its end values `(p, m')`.
#[derive(Clone, Copy)]
enum Relation {
    /// Unknowns `(p_s, m'_s)`; the end values by the transfer matrix.
    Transfer(M2),
    /// Unknowns `(m'_s, m'_e)`; the pressures by the impedance matrix, with flows `m'_s` and
    /// `−m'_e` into the duct.
    Impedance(M2),
}

struct Transfers {
    /// Per duct: the index of its first unknown and its relation, or `None` for the annulus of
    /// an absorptive pair, folded with the walls closing it into its perforated tube.
    ducts: Vec<Option<(usize, Relation)>>,
    unknowns: usize,
}

impl Transfers {
    fn new(net: &Network, mean: &MeanState, omega: f64) -> Result<Self> {
        let n = net.ducts.len();
        let mut folded = vec![false; n];
        for d in 0..n {
            if let Some(perf) = net.ducts[d].perforate {
                let closed = |end: End| {
                    net.nodes.iter().any(|node| {
                        matches!(node, Node::Wall(p) if p.duct == perf.partner && p.end == end)
                    })
                };
                if !(closed(End::Start) && closed(End::End)) {
                    return Err(Error::invalid(format!(
                        "four-pole: the annulus of '{}' must be closed at both ends",
                        net.ducts[d].label
                    )));
                }
                folded[perf.partner] = true;
            }
        }
        let mut next = 0;
        let ducts = (0..n)
            .map(|d| {
                if folded[d] {
                    return None;
                }
                let rel = match net.ducts[d].perforate {
                    Some(_) => Relation::Impedance(coupled_inner_impedance(net, mean, d, omega)),
                    None => Relation::Transfer(duct_matrix(net, mean, d, omega)),
                };
                next += 2;
                Some((next - 2, rel))
            })
            .collect();
        Ok(Self {
            ducts,
            unknowns: next,
        })
    }

    /// Start and end values `(p, m')` of duct `d` from the unknowns (zero for folded ducts).
    fn values(&self, d: usize, x: &DVector<C64>) -> [[C64; 2]; 2] {
        match self.ducts[d] {
            None => [[C64::new(0.0, 0.0); 2]; 2],
            Some((k, Relation::Transfer(m))) => {
                let s = [x[k], x[k + 1]];
                [
                    s,
                    [
                        m[0][0] * s[0] + m[0][1] * s[1],
                        m[1][0] * s[0] + m[1][1] * s[1],
                    ],
                ]
            }
            Some((k, Relation::Impedance(z))) => {
                let (ms, me) = (x[k], x[k + 1]);
                [
                    [z[0][0] * ms - z[0][1] * me, ms],
                    [z[1][0] * ms - z[1][1] * me, me],
                ]
            }
        }
    }

    fn form(&self, port: Port) -> Form {
        let (k, rel) = self.ducts[port.duct].expect("port of a folded duct");
        let (one, zero) = (C64::new(1.0, 0.0), C64::new(0.0, 0.0));
        let (p, q) = match (rel, port.end) {
            (Relation::Transfer(_), End::Start) => ([one, zero], [zero, -one]),
            (Relation::Transfer(m), End::End) => (m[0], m[1]),
            (Relation::Impedance(z), End::Start) => ([z[0][0], -z[0][1]], [-one, zero]),
            (Relation::Impedance(z), End::End) => ([z[1][0], -z[1][1]], [zero, one]),
        };
        Form {
            idx: [k, k + 1],
            p,
            q,
        }
    }
}

struct System {
    a: DMatrix<C64>,
    b: DVector<C64>,
    row: usize,
}

impl System {
    fn add(&mut self, terms: &[(Form, C64, C64)], rhs: C64) {
        // Each term contributes cp·p + cq·q of one port.
        for (f, cp, cq) in terms {
            for k in 0..2 {
                self.a[(self.row, f.idx[k])] += cp * f.p[k] + cq * f.q[k];
            }
        }
        self.b[self.row] = rhs;
        self.row += 1;
    }
}

/// Solves the network at angular frequency `omega`. `drives` sets the excitation of source
/// and characteristic nodes by node index; undriven sources are blocked (`Q = 0`, `Y = 0`)
/// and undriven characteristic ends are anechoic.
pub fn solve(
    net: &Network,
    mean: &MeanState,
    omega: f64,
    drives: &[(usize, Drive)],
) -> Result<Solution> {
    let (sys, transfers) = assemble(net, mean, omega, drives)?;
    let x = sys.a.lu().solve(&sys.b).ok_or_else(|| {
        Error::solver(format!("four-pole system singular at ω = {omega:.3} rad/s"))
    })?;
    let ends = (0..net.ducts.len())
        .map(|d| transfers.values(d, &x))
        .collect();
    Ok(Solution { ends })
}

/// Determinant of the undriven system; its zeros in complex ω are the network's natural
/// frequencies, so for light damping the minima of `|det|` on the real axis mark resonances.
pub fn determinant(net: &Network, mean: &MeanState, omega: f64) -> Result<C64> {
    Ok(assemble(net, mean, omega, &[])?.0.a.determinant())
}

fn assemble(
    net: &Network,
    mean: &MeanState,
    omega: f64,
    drives: &[(usize, Drive)],
) -> Result<(System, Transfers)> {
    let transfers = Transfers::new(net, mean, omega)?;
    let n = transfers.unknowns;
    let (one, zero) = (C64::new(1.0, 0.0), C64::new(0.0, 0.0));
    let mut sys = System {
        a: DMatrix::zeros(n, n),
        b: DVector::zeros(n),
        row: 0,
    };
    let drive_of = |i: usize| drives.iter().find(|d| d.0 == i).map(|d| d.1);
    for (i, node) in net.nodes.iter().enumerate() {
        let ports = node.ports();
        if ports.iter().any(|p| transfers.ducts[p.duct].is_none()) {
            continue;
        }
        let forms: Vec<Form> = ports.iter().map(|&p| transfers.form(p)).collect();
        let f0 = forms[0];
        match node {
            Node::Wall(_) => sys.add(&[(f0, zero, one)], zero),
            Node::Pressure { .. } => sys.add(&[(f0, one, zero)], zero),
            Node::Radiation(bc) => {
                let m = mean.at(bc.port, net);
                let s = net.ducts[bc.port.duct].end_area(bc.port.end);
                let r = LsFilter::response(omega * bc.radius / m.c);
                let z = (one + r) / (one - r);
                let mt = toward(bc.port, m.u) / m.c;
                sys.add(&[(f0, one + z * mt, -z * m.c / s)], zero);
            }
            Node::Characteristic(bc) => {
                let m = mean.at(bc.port, net);
                let s = net.ducts[bc.port.duct].end_area(bc.port.end);
                let mt = toward(bc.port, m.u) / m.c;
                let w = match drive_of(i) {
                    Some(Drive::Incident(w)) => w,
                    _ => zero,
                };
                sys.add(&[(f0, C64::new(1.0 + mt, 0.0), C64::new(-m.c / s, 0.0))], w);
            }
            Node::Source(_) | Node::MassFlow(_) => {
                let (q, y) = match drive_of(i) {
                    Some(Drive::Norton { q, y }) => (q, y),
                    _ => (zero, zero),
                };
                // q_toward = −(Q − Y p)  ⇔  q_toward − Y p = −Q.
                sys.add(&[(f0, -y, one)], -q);
            }
            Node::Junction(j) => match j.kind {
                JunctionKind::ConstantPressure => {
                    for f in &forms[1..] {
                        sys.add(&[(f0, one, zero), (*f, -one, zero)], zero);
                    }
                    let terms: Vec<(Form, C64, C64)> =
                        forms.iter().map(|f| (*f, zero, one)).collect();
                    sys.add(&terms, zero);
                }
                JunctionKind::AreaChange | JunctionKind::Orifice { .. } => {
                    let (pa, pb) = (ports[0], ports[1]);
                    let (ma, mb) = (mean.at(pa, net), mean.at(pb, net));
                    let (sa, sb) = (
                        net.ducts[pa.duct].end_area(pa.end),
                        net.ducts[pb.duct].end_area(pb.end),
                    );
                    let (va, vb) = (toward(pa, ma.u), toward(pb, mb.u));
                    // Upstream: mean flow toward the node.
                    let (u, d) = if va >= vb { (0, 1) } else { (1, 0) };
                    let (fm, sm, vm) = ([ma, mb], [sa, sb], [va, vb]);
                    let orifice = match j.kind {
                        JunctionKind::Orifice { effective_area } => Some(effective_area),
                        _ => None,
                    };
                    let k = if vm[u] > 0.0 {
                        step_loss(sm[u], sm[d], orifice)
                    } else {
                        0.0
                    };
                    let rho = fm[u].rho;
                    // δv = (c q/S − M_t p)/(ρc) per port; collect coefficients on (p, q).
                    let dv = |idx: usize| {
                        let mm = fm[idx];
                        let mt = vm[idx] / mm.c;
                        (-mt / (mm.rho * mm.c), 1.0 / (mm.rho * sm[idx]))
                    };
                    let (dvu_p, dvu_q) = dv(u);
                    let (dvd_p, dvd_q) = dv(d);
                    let cu = rho * (1.0 - k) * vm[u];
                    let cd = -rho * vm[d];
                    sys.add(
                        &[
                            (
                                forms[u],
                                C64::new(1.0 + cu * dvu_p, 0.0),
                                C64::new(cu * dvu_q, 0.0),
                            ),
                            (
                                forms[d],
                                C64::new(-1.0 + cd * dvd_p, 0.0),
                                C64::new(cd * dvd_q, 0.0),
                            ),
                        ],
                        zero,
                    );
                    sys.add(&[(forms[0], zero, one), (forms[1], zero, one)], zero);
                }
            },
        }
    }
    if sys.row != n {
        return Err(Error::invalid(format!(
            "four-pole network has {} equations for {n} unknowns",
            sys.row
        )));
    }
    Ok((sys, transfers))
}

/// Transmission loss, dB, between a `Characteristic` inlet node (driven by a unit incident
/// wave) and an anechoic `Characteristic` outlet node:
/// `TL = 10 log₁₀(W_inc/W_trans)`, `W± = S |p±|² (1 ± M)² / (ρc)` along the propagation direction.
pub fn transmission_loss(
    net: &Network,
    mean: &MeanState,
    omega: f64,
    inlet: usize,
    outlet: usize,
) -> Result<f64> {
    let w_in = C64::new(1.0, 0.0);
    let sol = solve(net, mean, omega, &[(inlet, Drive::Incident(w_in))])?;
    let pin = net.nodes[inlet].ports()[0];
    let pout = net.nodes[outlet].ports()[0];
    let (mi, mo) = (mean.at(pin, net), mean.at(pout, net));
    let (si, so) = (
        net.ducts[pin.duct].end_area(pin.end),
        net.ducts[pout.duct].end_area(pout.end),
    );
    let m_in = -toward(pin, mi.u) / mi.c;
    let m_out = toward(pout, mo.u) / mo.c;
    let p_inc = 0.5 * w_in.norm();
    let p_tr = sol.at_port(pout).0.norm();
    let w_inc = si * p_inc * p_inc * (1.0 + m_in).powi(2) / (mi.rho * mi.c);
    let w_tr = so * p_tr * p_tr * (1.0 + m_out).powi(2) / (mo.rho * mo.c);
    Ok(10.0 * (w_inc / w_tr).log10())
}
