//! Spectral analysis helpers.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

/// Continuous-time Fourier integral `∫ x(t) e^{−jωt} dt` of a sampled signal by the
/// trapezoidal rule (samples need not be uniform).
pub fn fourier_integral(t: &[f64], x: &[f64], omega: f64) -> Complex64 {
    let mut sum = Complex64::new(0.0, 0.0);
    for i in 1..t.len() {
        let a = x[i - 1] * Complex64::from_polar(1.0, -omega * t[i - 1]);
        let b = x[i] * Complex64::from_polar(1.0, -omega * t[i]);
        sum += 0.5 * (a + b) * (t[i] - t[i - 1]);
    }
    sum
}

/// Forward DFT `X_k = Σ x_n e^{−2πjkn/N}` of a real signal.
pub fn fft(x: &[f64]) -> Vec<Complex64> {
    let mut buf: Vec<Complex64> = x.iter().map(|&v| Complex64::new(v, 0.0)).collect();
    FftPlanner::new()
        .plan_fft_forward(buf.len())
        .process(&mut buf);
    buf
}

/// Frequencies of local maxima of the Hann-windowed, `pad`-times zero-padded magnitude
/// spectrum of a uniformly sampled signal, refined by parabolic interpolation of the log
/// magnitude over three bins. Returns the maximum nearest to each `guess`.
pub fn peak_frequencies(x: &[f64], dt: f64, pad: usize, guesses: &[f64]) -> Vec<f64> {
    let n = x.len();
    let mean = x.iter().sum::<f64>() / n as f64;
    let mut padded = vec![0.0; n * pad];
    for (i, v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
        padded[i] = (v - mean) * w;
    }
    let spec = fft(&padded);
    let df = 1.0 / (padded.len() as f64 * dt);
    let mag: Vec<f64> = spec[..padded.len() / 2]
        .iter()
        .map(|c| c.norm().max(1e-300).ln())
        .collect();
    let peaks: Vec<f64> = (1..mag.len() - 1)
        .filter(|&k| mag[k] > mag[k - 1] && mag[k] >= mag[k + 1])
        .map(|k| {
            let (a, b, c) = (mag[k - 1], mag[k], mag[k + 1]);
            let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
            (k as f64 + delta) * df
        })
        .collect();
    guesses
        .iter()
        .map(|g| {
            peaks
                .iter()
                .copied()
                .min_by(|a, b| (a - g).abs().total_cmp(&(b - g).abs()))
                .unwrap_or(f64::NAN)
        })
        .collect()
}
