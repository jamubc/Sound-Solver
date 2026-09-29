//! Numerical utilities: Bessel functions, quadrature, scalar root finding.

use std::f64::consts::PI;

/// Bessel function of the first kind, order one.
/// Abramowitz & Stegun 9.4.4 (|x| ≤ 3, |ε| < 1.3e-8) and 9.4.6 (x ≥ 3, |ε| < 4e-8 in f₁).
pub fn bessel_j1(x: f64) -> f64 {
    let ax = x.abs();
    if ax <= 3.0 {
        let y = (x / 3.0) * (x / 3.0);
        x * (0.5
            + y * (-0.562_499_85
                + y * (0.210_935_73
                    + y * (-0.039_542_89
                        + y * (0.004_433_19 + y * (-0.000_317_61 + y * 0.000_011_09))))))
    } else {
        let (f1, theta1) = j1_y1_asymptotic(ax);
        f1 * theta1.cos() / ax.sqrt() * x.signum()
    }
}

/// Bessel function of the second kind, order one, x > 0.
/// Abramowitz & Stegun 9.4.5 (0 < x ≤ 3, |ε| < 1.1e-7 in x·Y₁) and 9.4.6 (x ≥ 3). The
/// leading coefficient, tabulated as −0.6366198, is the exact −2/π.
pub fn bessel_y1(x: f64) -> f64 {
    debug_assert!(x > 0.0);
    if x <= 3.0 {
        let y = (x / 3.0) * (x / 3.0);
        let poly = -std::f64::consts::FRAC_2_PI
            + y * (0.221_209_1
                + y * (2.168_270_9
                    + y * (-1.316_482_7
                        + y * (0.312_395_1 + y * (-0.040_097_6 + y * 0.002_787_3)))));
        (2.0 / PI) * (x / 2.0).ln() * bessel_j1(x) + poly / x
    } else {
        let (f1, theta1) = j1_y1_asymptotic(x);
        f1 * theta1.sin() / x.sqrt()
    }
}

fn j1_y1_asymptotic(x: f64) -> (f64, f64) {
    let y = 3.0 / x;
    let f1 = 0.797_884_56
        + y * (0.000_001_56
            + y * (0.016_596_67
                + y * (0.000_171_05
                    + y * (-0.002_495_11 + y * (0.001_136_53 - y * 0.000_200_33)))));
    let theta1 = x - 2.356_194_49
        + y * (0.124_996_12
            + y * (0.000_056_50
                + y * (-0.006_378_79
                    + y * (0.000_743_48 + y * (0.000_798_24 - y * 0.000_291_66)))));
    (f1, theta1)
}

/// Exponentially scaled modified Bessel function `e^{−x} I₁(x)`, x ≥ 0.
/// Abramowitz & Stegun 9.8.3 (x ≤ 3.75, |ε| < 8e-9) and 9.8.4 (x ≥ 3.75, |ε| < 2.2e-7).
pub fn bessel_i1_scaled(x: f64) -> f64 {
    debug_assert!(x >= 0.0);
    if x <= 3.75 {
        let t = (x / 3.75) * (x / 3.75);
        let i1 = x
            * (0.5
                + t * (0.878_905_94
                    + t * (0.514_988_69
                        + t * (0.150_849_34
                            + t * (0.026_587_33 + t * (0.003_015_32 + t * 0.000_324_11))))));
        i1 * (-x).exp()
    } else {
        let t = 3.75 / x;
        let p = 0.398_942_28
            + t * (-0.039_880_24
                + t * (-0.003_620_18
                    + t * (0.001_638_01
                        + t * (-0.010_315_55
                            + t * (0.022_829_67
                                + t * (-0.028_953_12 + t * (0.017_876_54 - t * 0.004_200_59)))))));
        p / x.sqrt()
    }
}

/// Exponentially scaled modified Bessel function `e^{x} K₁(x)`, x > 0.
/// Abramowitz & Stegun 9.8.7 (0 < x ≤ 2, |ε| < 8e-9) and 9.8.8 (x ≥ 2, |ε| < 2.2e-7).
pub fn bessel_k1_scaled(x: f64) -> f64 {
    debug_assert!(x > 0.0);
    if x <= 2.0 {
        let y = (x / 2.0) * (x / 2.0);
        let i1 = bessel_i1_scaled(x) * x.exp();
        let poly = 1.0
            + y * (0.154_431_44
                + y * (-0.672_785_79
                    + y * (-0.181_568_97
                        + y * (-0.019_194_02 + y * (-0.001_104_04 - y * 0.000_046_86)))));
        ((x / 2.0).ln() * i1 + poly / x) * x.exp()
    } else {
        let y = 2.0 / x;
        let p = 1.253_314_14
            + y * (0.234_986_19
                + y * (-0.036_556_20
                    + y * (0.015_042_68
                        + y * (-0.007_803_53 + y * (0.003_256_14 - y * 0.000_682_45)))));
        p / x.sqrt()
    }
}

/// Bessel functions `(J₀(z), J₁(z))` of complex argument by their power series
/// `J₀ = Σ (−z²/4)ᵏ/(k!)²`, `J₁ = (z/2) Σ (−z²/4)ᵏ/(k!(k+1)!)`; accurate for `|z| ≲ 30`.
pub fn bessel_j01_complex(
    z: rustfft::num_complex::Complex64,
) -> (
    rustfft::num_complex::Complex64,
    rustfft::num_complex::Complex64,
) {
    let q = -z * z / 4.0;
    let (mut t0, mut t1) = (rustfft::num_complex::Complex64::new(1.0, 0.0), z / 2.0);
    let (mut j0, mut j1) = (t0, t1);
    for k in 1..400 {
        let k = k as f64;
        t0 *= q / (k * k);
        t1 *= q / (k * (k + 1.0));
        j0 += t0;
        j1 += t1;
        if t0.norm() <= 1e-17 * j0.norm() && t1.norm() <= 1e-17 * j1.norm() {
            break;
        }
    }
    (j0, j1)
}

/// Gauss–Legendre nodes and weights on [−1, 1] (Newton iteration on Pₙ).
pub fn gauss_legendre(n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut x = vec![0.0; n];
    let mut w = vec![0.0; n];
    let m = n.div_ceil(2);
    for i in 0..m {
        let mut z = (PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        let mut dp;
        loop {
            let (mut p1, mut p2) = (1.0, 0.0);
            for j in 0..n {
                let p3 = p2;
                p2 = p1;
                p1 = ((2 * j + 1) as f64 * z * p2 - j as f64 * p3) / (j + 1) as f64;
            }
            dp = n as f64 * (z * p1 - p2) / (z * z - 1.0);
            let z_new = z - p1 / dp;
            let done = (z_new - z).abs() < 1e-15;
            z = z_new;
            if done {
                break;
            }
        }
        x[i] = -z;
        x[n - 1 - i] = z;
        w[i] = 2.0 / ((1.0 - z * z) * dp * dp);
        w[n - 1 - i] = w[i];
    }
    (x, w)
}

/// Composite Gauss–Legendre quadrature of `f` on [a, b] with `panels` panels of `n` points.
pub fn integrate<F: Fn(f64) -> f64>(f: F, a: f64, b: f64, panels: usize, n: usize) -> f64 {
    let (x, w) = gauss_legendre(n);
    let h = (b - a) / panels as f64;
    let mut sum = 0.0;
    for p in 0..panels {
        let mid = a + (p as f64 + 0.5) * h;
        for (xi, wi) in x.iter().zip(&w) {
            sum += wi * f(mid + 0.5 * h * xi);
        }
    }
    0.5 * h * sum
}

/// Brent's method for a root of `f` in [a, b]; `f(a)` and `f(b)` must differ in sign.
/// Returns `None` if the bracket is invalid.
pub fn brent<F: FnMut(f64) -> f64>(
    mut f: F,
    mut a: f64,
    mut b: f64,
    tol: f64,
    max_iter: usize,
) -> Option<f64> {
    let mut fa = f(a);
    let mut fb = f(b);
    if fa == 0.0 {
        return Some(a);
    }
    if fb == 0.0 {
        return Some(b);
    }
    if fa.signum() == fb.signum() {
        return None;
    }
    let (mut c, mut fc) = (a, fa);
    let mut d = b - a;
    let mut e = d;
    for _ in 0..max_iter {
        if fb.signum() == fc.signum() {
            c = a;
            fc = fa;
            d = b - a;
            e = d;
        }
        if fc.abs() < fb.abs() {
            a = b;
            b = c;
            c = a;
            fa = fb;
            fb = fc;
            fc = fa;
        }
        let tol1 = 2.0 * f64::EPSILON * b.abs() + 0.5 * tol;
        let xm = 0.5 * (c - b);
        if xm.abs() <= tol1 || fb == 0.0 {
            return Some(b);
        }
        if e.abs() >= tol1 && fa.abs() > fb.abs() {
            let s = fb / fa;
            let (mut p, mut q);
            if a == c {
                p = 2.0 * xm * s;
                q = 1.0 - s;
            } else {
                let qa = fa / fc;
                let r = fb / fc;
                p = s * (2.0 * xm * qa * (qa - r) - (b - a) * (r - 1.0));
                q = (qa - 1.0) * (r - 1.0) * (s - 1.0);
            }
            if p > 0.0 {
                q = -q;
            }
            p = p.abs();
            let min1 = 3.0 * xm * q - (tol1 * q).abs();
            let min2 = (e * q).abs();
            if 2.0 * p < min1.min(min2) {
                e = d;
                d = p / q;
            } else {
                d = xm;
                e = d;
            }
        } else {
            d = xm;
            e = d;
        }
        a = b;
        fa = fb;
        b += if d.abs() > tol1 { d } else { tol1.copysign(xm) };
        fb = f(b);
    }
    Some(b)
}

/// Piecewise-linear function of time through knots `[t, value]` (times strictly increasing),
/// held at the end values outside them, with its exact running integral from `t = 0`.
#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    knots: Vec<[f64; 2]>,
    /// `∫₀^{t_k} f` at each knot.
    integral: Vec<f64>,
}

impl Trace {
    /// `None` without knots, with times not strictly increasing, or with a non-finite number.
    pub fn new(knots: Vec<[f64; 2]>) -> Option<Self> {
        if knots.is_empty()
            || knots.iter().flatten().any(|v| !v.is_finite())
            || knots.windows(2).any(|w| w[1][0] <= w[0][0])
        {
            return None;
        }
        let mut integral = vec![knots[0][1] * knots[0][0]];
        for w in knots.windows(2) {
            let last = integral[integral.len() - 1];
            integral.push(last + 0.5 * (w[0][1] + w[1][1]) * (w[1][0] - w[0][0]));
        }
        Some(Self { knots, integral })
    }

    pub fn constant(value: f64) -> Self {
        Self {
            knots: vec![[0.0, value]],
            integral: vec![0.0],
        }
    }

    pub fn knots(&self) -> &[[f64; 2]] {
        &self.knots
    }

    pub fn at(&self, t: f64) -> f64 {
        let k = self.knots.partition_point(|p| p[0] <= t);
        if k == 0 {
            return self.knots[0][1];
        }
        if k == self.knots.len() {
            return self.knots[k - 1][1];
        }
        let (a, b) = (self.knots[k - 1], self.knots[k]);
        a[1] + (b[1] - a[1]) * (t - a[0]) / (b[0] - a[0])
    }

    /// `∫₀ᵗ f`.
    pub fn integral(&self, t: f64) -> f64 {
        let k = self.knots.partition_point(|p| p[0] <= t);
        if k == 0 {
            return self.knots[0][1] * t;
        }
        let a = self.knots[k - 1];
        self.integral[k - 1] + 0.5 * (a[1] + self.at(t)) * (t - a[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_reference_values() {
        // Abramowitz & Stegun Table 9.1 / 9.8.
        let close = |a: f64, b: f64, tol: f64| (a - b).abs() < tol;
        assert!(close(bessel_j1(1.0), 0.440_050_586, 1e-8));
        assert!(close(bessel_j1(5.0), -0.327_579_138, 1e-7));
        assert!(close(bessel_y1(1.0), -0.781_212_821, 2e-7));
        assert!(close(bessel_y1(5.0), 0.147_863_143, 1e-7));
        assert!(close(
            bessel_i1_scaled(1.0) * 1f64.exp(),
            0.565_159_104,
            1e-8
        ));
        assert!(close(
            bessel_k1_scaled(1.0) * (-1f64).exp(),
            0.601_907_230,
            1e-8
        ));
        assert!(close(
            bessel_k1_scaled(5.0) * (-5f64).exp(),
            0.004_044_613_9,
            1e-9
        ));
    }

    #[test]
    fn quadrature_and_root() {
        let v = integrate(|x| x.sin(), 0.0, PI, 4, 16);
        assert!((v - 2.0).abs() < 1e-13);
        let r = brent(|x| x * x - 2.0, 0.0, 2.0, 1e-14, 100).unwrap();
        assert!((r - 2f64.sqrt()).abs() < 1e-13);
    }

    /// A ramp from 1000 to 3000 held either side: value and running integral against the
    /// closed form.
    #[test]
    fn trace_values_and_integral() {
        let tr = Trace::new(vec![[1.0, 1000.0], [3.0, 3000.0]]).unwrap();
        let exact = |t: f64| match t {
            t if t <= 1.0 => 1000.0 * t,
            t if t <= 3.0 => 1000.0 + 1000.0 * (t - 1.0) + 500.0 * (t - 1.0).powi(2),
            t => 1000.0 + 2000.0 + 2000.0 + 3000.0 * (t - 3.0),
        };
        for t in [-0.5, 0.0, 0.7, 1.0, 1.9, 3.0, 4.2] {
            assert!((tr.integral(t) - exact(t)).abs() < 1e-9, "t {t}");
        }
        assert_eq!(
            (tr.at(0.0), tr.at(2.0), tr.at(9.0)),
            (1000.0, 2000.0, 3000.0)
        );
        assert!(Trace::new(vec![[1.0, 0.0], [1.0, 2.0]]).is_none());
    }
}
