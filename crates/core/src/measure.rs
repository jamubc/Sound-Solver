//! Measured sound (directive, "Sound evaluation"): a recording of the car (WAV, or CAF from an
//! iPhone; the first channel), an engine-speed log from an OBD logger (CSV), and what the tuning
//! loop takes from them. The core parses bytes; callers read the files.
//!
//! - Order tracks: Hann-windowed frames of 0.5 s every 0.125 s. An engine order's mean square
//!   in a frame is the one-sided power within `max(2 bins, rpm/240 Hz)` of `k·rpm/60`, with the
//!   frame's engine speed from the log or, without one, the speed whose orders 1–8 carry the
//!   most log-power (a harmonic sum in 5 rpm steps over the sweep; flagged as estimated). Mean
//!   squares are averaged within the sweep's speed bins. Levels are dB re 20 µPa when the
//!   recording's calibration is known, else dB re full scale (0 dBFS: a full-scale sine).
//! - Cabin transfer function by order ratio: interior minus exterior order levels of the same
//!   drive, at each order's frequency in every speed bin both cover.
//! - Cabin transfer function by impulse: every impulse (a clap or a balloon at the tailpipe)
//!   in each recording, windowed from 10 ms before its peak for 1 s; energy spectra averaged
//!   over the impulses; interior over exterior.
//!
//! Both transfer functions are energy-averaged in sixth-octave bands over 20 Hz–2 kHz; both
//! recordings must be calibrated, or neither (the same phone at the same gain).

use std::f64::consts::PI;
use std::io::Cursor;

use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;
use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::metrics::{DroneReport, drone};
use crate::project::{MicPosition, Project};
use crate::render::{Listener, Scene};

const FRAME_S: f64 = 0.5;
const HOP_S: f64 = 0.125;
const IMPULSE_S: f64 = 1.0;
const PRE_ROLL_S: f64 = 0.01;
const TF_BAND_HZ: [f64; 2] = [20.0, 2000.0];
const BANDS_PER_OCTAVE: f64 = 6.0;

/// Samples of one channel, full scale ±1.
#[derive(Clone, Debug)]
pub struct Recording {
    pub sample_rate: f64,
    pub samples: Vec<f64>,
}

impl Recording {
    /// Reads a CAF when `file_name` ends in `.caf`, else WAV.
    pub fn parse(file_name: &str, bytes: &[u8]) -> Result<Self> {
        if file_name.to_ascii_lowercase().ends_with(".caf") {
            Self::from_caf(bytes)
        } else {
            Self::from_wav(bytes)
        }
    }

    fn from_wav(bytes: &[u8]) -> Result<Self> {
        let bad = |e: hound::Error| Error::invalid(format!("WAV: {e}"));
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(bad)?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let samples: Vec<f64> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .step_by(channels)
                .map(|s| s.map(f64::from))
                .collect::<std::result::Result<_, _>>(),
            hound::SampleFormat::Int => {
                let full = (1u64 << (spec.bits_per_sample - 1)) as f64;
                reader
                    .samples::<i32>()
                    .step_by(channels)
                    .map(|s| s.map(|v| v as f64 / full))
                    .collect::<std::result::Result<_, _>>()
            }
        }
        .map_err(bad)?;
        Ok(Self {
            sample_rate: spec.sample_rate as f64,
            samples,
        })
    }

    fn from_caf(bytes: &[u8]) -> Result<Self> {
        use symphonia::core::codecs::audio::AudioDecoderOptions;
        use symphonia::core::formats::probe::Hint;
        use symphonia::core::formats::{FormatOptions, TrackType};
        use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
        use symphonia::core::meta::MetadataOptions;
        let bad = |e: symphonia::core::errors::Error| Error::invalid(format!("CAF: {e}"));
        let source = MediaSourceStream::new(
            Box::new(Cursor::new(bytes.to_vec())),
            MediaSourceStreamOptions::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension("caf");
        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                source,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(bad)?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::invalid("CAF: no audio track"))?;
        let id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| Error::invalid("CAF: unknown codec"))?
            .clone();
        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(bad)?;
        let (mut samples, mut rate, mut frame) = (Vec::new(), 0, Vec::<f32>::new());
        while let Some(packet) = format.next_packet().map_err(bad)? {
            if packet.track_id != id {
                continue;
            }
            let decoded = decoder.decode(&packet).map_err(bad)?;
            rate = decoded.spec().rate();
            let channels = decoded.spec().channels().count().max(1);
            decoded.copy_to_vec_interleaved(&mut frame);
            samples.extend(frame.iter().step_by(channels).map(|&s| f64::from(s)));
        }
        if rate == 0 {
            return Err(Error::invalid("CAF: no audio"));
        }
        Ok(Self {
            sample_rate: rate as f64,
            samples,
        })
    }
}

/// Engine speed against time: `[s from the first row, rpm]`.
#[derive(Clone, Debug)]
pub struct RpmLog {
    pub points: Vec<[f64; 2]>,
}

impl RpmLog {
    /// Reads a CSV (comma, semicolon or tab) with a header naming a time column (`time`, `sec`;
    /// else the first) and an engine-speed column (`rpm`), or without one: time, rpm. Times in
    /// seconds or as clock time (`hh:mm:ss.s`, after any date).
    pub fn parse(text: &str) -> Result<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let first = lines
            .first()
            .ok_or_else(|| Error::invalid("engine-speed log: empty"))?;
        let delimiter = [',', ';', '\t']
            .into_iter()
            .max_by_key(|d| first.matches(*d).count())
            .expect("three delimiters");
        let split = |line: &str| -> Vec<String> {
            line.split(delimiter)
                .map(|c| c.trim().trim_matches('"').to_string())
                .collect()
        };
        let head = split(first);
        let titled = head.iter().any(|c| seconds(c).is_none());
        let (time, rpm) = if titled {
            let find = |keys: &[&str]| {
                head.iter().position(|c| {
                    let c = c.to_ascii_lowercase();
                    keys.iter().any(|k| c.contains(k))
                })
            };
            let rpm = find(&["rpm"])
                .ok_or_else(|| Error::invalid("engine-speed log: no column named rpm"))?;
            (find(&["time", "sec"]).unwrap_or(0), rpm)
        } else {
            (0, 1)
        };
        let mut points: Vec<[f64; 2]> = lines[usize::from(titled)..]
            .iter()
            .filter_map(|line| {
                let cells = split(line);
                Some([seconds(cells.get(time)?)?, cells.get(rpm)?.parse().ok()?])
            })
            .collect();
        if points.len() < 2 {
            return Err(Error::invalid("engine-speed log: fewer than two readings"));
        }
        let t0 = points[0][0];
        points.iter_mut().for_each(|p| p[0] -= t0);
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        Ok(Self { points })
    }

    /// Engine speed at `t` s, linear between readings; `None` outside the log.
    pub fn at(&self, t: f64) -> Option<f64> {
        let p = &self.points;
        let k = p.partition_point(|x| x[0] < t);
        if k == 0 || k == p.len() {
            return (p.get(k).is_some_and(|x| x[0] == t)).then(|| p[k][1]);
        }
        let (a, b) = (p[k - 1], p[k]);
        Some(a[1] + (b[1] - a[1]) * (t - a[0]) / (b[0] - a[0]))
    }
}

/// Seconds from `12.5`, or from clock time `hh:mm:ss.s` / `mm:ss.s` (the last word of the cell).
fn seconds(cell: &str) -> Option<f64> {
    let word = cell.split_whitespace().last()?;
    if !word.contains(':') {
        return word.parse().ok();
    }
    word.split(':').try_fold(0.0, |acc, part| {
        Some(acc * 60.0 + part.parse::<f64>().ok()?)
    })
}

/// Levels of engine orders against engine speed from a recording.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct OrderTracks {
    /// Speed-bin centres (the project's sweep), rpm.
    pub rpm: Vec<f64>,
    pub orders: Vec<OrderTrack>,
    /// Levels in dB re 20 µPa; otherwise dB re full scale.
    pub calibrated: bool,
    /// Engine speed estimated from the recording, without a log.
    pub rpm_estimated: bool,
    /// Frames analysed and frames within the sweep.
    pub frames: [usize; 2],
    /// The firing order's drone, as reported for the prediction.
    pub drone: Option<DroneReport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct OrderTrack {
    pub order: f64,
    /// Per speed bin; `None` where the recording has no frame.
    pub level_db: Vec<Option<f64>>,
}

/// Order tracks of `rec` over the project's sweep; engine speed from `log` read `offset_s`
/// after the recording's start, or estimated.
pub fn order_tracks(
    project: &Project,
    rec: &Recording,
    log: Option<&RpmLog>,
    offset_s: f64,
    calibration_db: Option<f64>,
) -> Result<OrderTracks> {
    let [start, stop, step] = project.operating.sweep_rpm;
    let firing = project.engine.geometry.firing_order.len() as f64 / 2.0;
    let mut orders: Vec<f64> = (1..=8).map(f64::from).collect();
    if firing.fract() != 0.0 {
        orders.push(firing);
    }
    let bins = ((stop - start) / step).round() as usize + 1;
    let mut sums = vec![vec![(0.0, 0usize); bins]; orders.len()];
    let mut frames = [0, 0];
    each_frame(rec, 0.0, f64::INFINITY, |t, power, df| {
        frames[0] += 1;
        let rpm = match log {
            Some(log) => log.at(t + offset_s),
            None => estimate_rpm(power, df, start, stop),
        };
        let Some(rpm) = rpm.filter(|r| *r >= start - 0.5 * step && *r < stop + 0.5 * step) else {
            return;
        };
        frames[1] += 1;
        let bin = ((rpm - start) / step).round() as usize;
        let half = (2.0 * df).max(rpm / 240.0);
        for (k, order) in orders.iter().enumerate() {
            let f = order * rpm / 60.0;
            let (lo, hi) = (
                ((f - half) / df).ceil().max(0.0) as usize,
                ((f + half) / df).floor() as usize,
            );
            if hi < power.len() {
                let s = &mut sums[k][bin];
                (s.0, s.1) = (s.0 + power[lo..=hi].iter().sum::<f64>(), s.1 + 1);
            }
        }
    })?;
    let cal = calibration_db.unwrap_or(0.0);
    let rpms: Vec<f64> = (0..bins).map(|b| start + step * b as f64).collect();
    let orders: Vec<OrderTrack> = orders
        .iter()
        .zip(&sums)
        .map(|(&order, s)| OrderTrack {
            order,
            level_db: s
                .iter()
                .map(|&(ms, count)| {
                    (count > 0).then(|| 10.0 * (ms / count as f64 / 0.5).log10() + cal)
                })
                .collect(),
        })
        .collect();
    let track: Vec<(f64, f64, bool)> = orders
        .iter()
        .find(|o| o.order == firing)
        .map(|o| {
            rpms.iter()
                .zip(&o.level_db)
                .filter_map(|(&r, l)| l.map(|l| (r, l, true)))
                .collect()
        })
        .unwrap_or_default();
    Ok(OrderTracks {
        drone: drone(&track, project.operating.cruise_band_rpm),
        rpm: rpms,
        orders,
        calibrated: calibration_db.is_some(),
        rpm_estimated: log.is_none(),
        frames,
    })
}

/// Calls `f(t, power, df)` for each Hann-windowed frame (`FRAME_S` long, every `HOP_S`) of
/// `rec` lying within `[t0, t1]` s: its centre time, its one-sided mean-square spectrum and the
/// bin spacing, Hz.
fn each_frame(
    rec: &Recording,
    t0: f64,
    t1: f64,
    mut f: impl FnMut(f64, &[f64], f64),
) -> Result<()> {
    let fs = rec.sample_rate;
    let n = (FRAME_S * fs).round() as usize;
    if n < 16 || rec.samples.len() < n {
        return Err(Error::invalid(
            "the recording is shorter than one 0.5 s analysis frame",
        ));
    }
    let window = hann(n);
    let scale = 2.0 / (n as f64 * window.iter().map(|w| w * w).sum::<f64>());
    let fft = FftPlanner::new().plan_fft_forward(n);
    let hop = ((HOP_S * fs).round() as usize).max(1);
    let first = (t0 * fs).max(0.0).ceil() as usize;
    let last = ((t1 * fs).min(rec.samples.len() as f64) as usize).saturating_sub(n);
    for begin in (first..=last).step_by(hop) {
        let mut buf: Vec<Complex64> = rec.samples[begin..begin + n]
            .iter()
            .zip(&window)
            .map(|(x, w)| Complex64::new(x * w, 0.0))
            .collect();
        fft.process(&mut buf);
        let power: Vec<f64> = buf[..n / 2].iter().map(|c| c.norm_sqr() * scale).collect();
        f((begin as f64 + 0.5 * n as f64) / fs, &power, fs / n as f64);
    }
    Ok(())
}

fn hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos())
        .collect()
}

/// Engine speed whose orders 1–8 carry the most log-power in a frame's one-sided power
/// spectrum (bins `df` apart), in 5 rpm steps over `lo..=hi`; `None` at either end of the
/// range, where no speed stands out.
fn estimate_rpm(power: &[f64], df: f64, lo: f64, hi: f64) -> Option<f64> {
    let floor = power.iter().copied().fold(0.0, f64::max) * 1e-9 + f64::MIN_POSITIVE;
    let at = |f: f64| {
        let x = f / df;
        let i = x.floor() as usize;
        if i + 1 >= power.len() {
            return floor;
        }
        let a = x - i as f64;
        power[i] * (1.0 - a) + power[i + 1] * a + floor
    };
    let score = |rpm: f64| {
        (1..=8)
            .map(|k| at(k as f64 * rpm / 60.0).log10())
            .sum::<f64>()
    };
    let steps = ((hi - lo) / 5.0).floor() as usize;
    let best = (0..=steps)
        .map(|i| lo + 5.0 * i as f64)
        .max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    (best > lo && best < lo + 5.0 * steps as f64).then_some(best)
}

/// Cabin transfer function, `[Hz, dB]` in increasing frequency, from the order tracks of an
/// exterior and an interior recording of the same drive (the same speed bins).
pub fn cabin_tf_orders(exterior: &OrderTracks, interior: &OrderTracks) -> Result<Vec<[f64; 2]>> {
    if exterior.calibrated != interior.calibrated {
        return Err(Error::invalid(
            "calibrate both recordings, or neither (the same phone at the same gain)",
        ));
    }
    let mut points = Vec::new();
    for (e, i) in exterior.orders.iter().zip(&interior.orders) {
        for ((rpm, le), li) in exterior.rpm.iter().zip(&e.level_db).zip(&i.level_db) {
            if let (Some(le), Some(li)) = (le, li) {
                points.push((
                    e.order * rpm / 60.0,
                    10f64.powf(li / 10.0),
                    10f64.powf(le / 10.0),
                ));
            }
        }
    }
    bands(&points)
}

/// Cabin transfer function, `[Hz, dB]` in increasing frequency, from recordings of the same
/// impulses at the exterior receiver and at the driver's ear.
pub fn cabin_tf_impulse(exterior: &Recording, interior: &Recording) -> Result<Vec<[f64; 2]>> {
    let (ext, int) = (impulse_energy(exterior)?, impulse_energy(interior)?);
    // Both spectra have bins 1/IMPULSE_S apart.
    let points: Vec<(f64, f64, f64)> = ext
        .iter()
        .zip(&int)
        .enumerate()
        .map(|(k, (e, i))| (k as f64 / IMPULSE_S, *i, *e))
        .collect();
    bands(&points)
}

/// Energy spectrum (bins 1/IMPULSE_S apart, per unit bandwidth) averaged over the impulses of a
/// recording: each louder than 30 % of the loudest, windowed from PRE_ROLL_S before its peak for
/// IMPULSE_S with a 10 % half-Hann fade-out.
fn impulse_energy(rec: &Recording) -> Result<Vec<f64>> {
    let fs = rec.sample_rate;
    let (n, pre) = (
        (IMPULSE_S * fs).round() as usize,
        (PRE_ROLL_S * fs).round() as usize,
    );
    let x = &rec.samples;
    let loudest = x.iter().fold(0.0, |m: f64, s| m.max(s.abs()));
    let mut starts = Vec::new();
    let mut i = pre;
    while loudest > 0.0 && i + n <= x.len() + pre {
        if x[i].abs() < 0.3 * loudest {
            i += 1;
            continue;
        }
        let end = (i + (0.02 * fs) as usize).min(x.len());
        let peak = (i..end)
            .max_by(|&a, &b| x[a].abs().total_cmp(&x[b].abs()))
            .expect("non-empty");
        if peak - pre + n <= x.len() {
            starts.push(peak - pre);
        }
        i = peak + n;
    }
    if starts.is_empty() {
        return Err(Error::invalid("no impulse found in the recording"));
    }
    let fade = n / 10;
    let window: Vec<f64> = (0..n)
        .map(|j| match j.checked_sub(n - fade) {
            Some(k) => 0.5 + 0.5 * (PI * k as f64 / fade as f64).cos(),
            None => 1.0,
        })
        .collect();
    let fft = FftPlanner::new().plan_fft_forward(n);
    let mut energy = vec![0.0; n / 2];
    for &s in &starts {
        let mut buf: Vec<Complex64> = x[s..s + n]
            .iter()
            .zip(&window)
            .map(|(v, w)| Complex64::new(v * w, 0.0))
            .collect();
        fft.process(&mut buf);
        for (e, c) in energy.iter_mut().zip(&buf) {
            *e += c.norm_sqr() / (fs * fs * starts.len() as f64);
        }
    }
    Ok(energy)
}

/// `(Hz, interior, exterior)` energies summed in sixth-octave bands over TF_BAND_HZ: `[mean
/// frequency of the band's points, 10 log₁₀(Σ interior / Σ exterior)]`.
fn bands(points: &[(f64, f64, f64)]) -> Result<Vec<[f64; 2]>> {
    let mut sums: Vec<(f64, usize, f64, f64)> = vec![
        (0.0, 0, 0.0, 0.0);
        ((TF_BAND_HZ[1] / TF_BAND_HZ[0]).log2() * BANDS_PER_OCTAVE).ceil()
            as usize
    ];
    for &(f, int, ext) in points {
        if f < TF_BAND_HZ[0] || f >= TF_BAND_HZ[1] {
            continue;
        }
        let b = ((f / TF_BAND_HZ[0]).log2() * BANDS_PER_OCTAVE) as usize;
        let s = &mut sums[b];
        *s = (s.0 + f, s.1 + 1, s.2 + int, s.3 + ext);
    }
    let gain: Vec<[f64; 2]> = sums
        .iter()
        .filter(|s| s.1 > 0 && s.2 > 0.0 && s.3 > 0.0)
        .map(|s| [s.0 / s.1 as f64, 10.0 * (s.2 / s.3).log10()])
        .collect();
    if gain.is_empty() {
        return Err(Error::invalid(
            "the recordings share no level between 20 Hz and 2 kHz",
        ));
    }
    Ok(gain)
}

/// Pascals per unit of a recording calibrated so a full-scale sine is `calibration_db` dB re
/// 20 µPa.
fn pa_per_unit(calibration_db: f64) -> f64 {
    std::f64::consts::SQRT_2 * 20e-6 * 10f64.powf(calibration_db / 20.0)
}

/// Engine speed over `[t0, t1]` s of `rec`: mean and standard deviation, rpm, from `log` read
/// `offset_s` after the recording's start, else estimated frame by frame over `lo..=hi`
/// (`true` when estimated).
pub fn hold_rpm(
    rec: &Recording,
    log: Option<&RpmLog>,
    offset_s: f64,
    [t0, t1]: [f64; 2],
    [lo, hi]: [f64; 2],
) -> Result<(f64, f64, bool)> {
    let length = rec.samples.len() as f64 / rec.sample_rate;
    if !(t0 >= 0.0 && t1 > t0 && t1 <= length) {
        return Err(Error::invalid(format!(
            "the hold must lie within the recording's {length:.1} s"
        )));
    }
    let speeds: Vec<f64> = match log {
        Some(log) => (0..=100)
            .filter_map(|i| log.at(t0 + (t1 - t0) * i as f64 / 100.0 + offset_s))
            .collect(),
        None => {
            let mut v = Vec::new();
            each_frame(rec, t0, t1, |_, power, df| {
                v.extend(estimate_rpm(power, df, lo, hi));
            })?;
            v
        }
    };
    if speeds.is_empty() {
        return Err(Error::invalid("no engine speed over the hold"));
    }
    let n = speeds.len() as f64;
    let mean = speeds.iter().sum::<f64>() / n;
    let spread = (speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n).sqrt();
    Ok((mean, spread, log.is_none()))
}

/// Mean square of `x` (sampled at `fs`) in each ⅓-octave band (`render::third_octaves`),
/// Hann-windowed over its whole length; `None` above the Nyquist frequency.
pub fn band_mean_squares(x: &[f64], fs: f64) -> Vec<Option<f64>> {
    let n = x.len().max(2);
    let window = hann(n);
    let scale = 2.0 / (n as f64 * window.iter().map(|w| w * w).sum::<f64>());
    let mut buf: Vec<Complex64> = x
        .iter()
        .zip(&window)
        .map(|(v, w)| Complex64::new(v * w, 0.0))
        .collect();
    buf.resize(n, Complex64::new(0.0, 0.0));
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let df = fs / n as f64;
    crate::render::third_octaves()
        .into_iter()
        .map(|(_, lo, hi)| {
            (hi <= 0.5 * fs).then(|| {
                buf[1..n / 2]
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| (lo..hi).contains(&((k + 1) as f64 * df)))
                    .map(|(_, c)| c.norm_sqr() * scale)
                    .sum()
            })
        })
        .collect()
}

/// The scene a hold is checked against: the hold's engine speed on the project's load line,
/// heard where the recording was made, for as long as the hold (1 to 4 s).
pub fn hold_scene(rpm: f64, window_s: [f64; 2], position: MicPosition) -> Scene {
    Scene {
        duration_s: (window_s[1] - window_s[0]).clamp(1.0, 4.0),
        rpm: vec![[0.0, rpm]],
        map_kpa: None,
        listener: match position {
            MicPosition::Exterior => Listener::Receiver,
            MicPosition::Interior => Listener::Cabin,
        },
    }
}

/// A steady hold of a calibrated recording set against a render of the same engine speed at
/// the same place, band by band. Recordings validate; nothing here is fed back into an input.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct HoldComparison {
    /// The hold, s from the recording's start.
    pub window_s: [f64; 2],
    /// Engine speed over the hold: mean and standard deviation, rpm.
    pub rpm: f64,
    pub rpm_spread: f64,
    /// Engine speed estimated from the recording, without a log.
    pub rpm_estimated: bool,
    pub bands: Vec<BandComparison>,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct BandComparison {
    pub center_hz: f64,
    /// Measured level, dB re 20 µPa, corrected for the background when one is given; `None`
    /// where the recording cannot give it.
    pub measured_db: Option<f64>,
    pub predicted_db: Option<f64>,
    /// Predicted less measured, dB.
    pub error_db: Option<f64>,
    /// Indicative target, ± dB (shown, not passed or failed): 3 up to 2 kHz, 5 above.
    pub target_db: f64,
    /// The render resolves this band.
    pub resolved: bool,
    /// Why the measured level is missing or corrected.
    pub note: Option<String>,
}

/// Dynamic range of a 16-bit recording, dB: a band further below full scale is not resolved.
const RESOLUTION_DB: f64 = 96.0;

/// `rec` over `window_s`, calibrated at `calibration_db`, against `predicted` (Pa, sampled at
/// `predicted_fs`) band by band, with `resolved` the render's band status. A band more than
/// the 16-bit range below full scale is left out. With a background recording (and its
/// calibration) each band is corrected energy-wise where it stands 6–10 dB above the
/// background and left out below 6 dB.
pub fn compare_hold(
    rec: &Recording,
    calibration_db: f64,
    window_s: [f64; 2],
    background: Option<(&Recording, f64)>,
    predicted: &[f32],
    predicted_fs: f64,
    resolved: &[bool],
) -> Vec<BandComparison> {
    let fs = rec.sample_rate;
    let seg = &rec.samples[(window_s[0] * fs) as usize..(window_s[1] * fs) as usize];
    let db = |ms: Option<f64>, per_unit: f64| {
        ms.filter(|m| *m > 0.0)
            .map(|m| 10.0 * (m * per_unit * per_unit / 4e-10).log10())
    };
    let measured = band_mean_squares(seg, fs);
    let noise = background.map(|(b, cal)| {
        (
            band_mean_squares(&b.samples, b.sample_rate),
            pa_per_unit(cal),
        )
    });
    let per_unit = pa_per_unit(calibration_db);
    let pred: Vec<f64> = predicted.iter().map(|&v| v as f64).collect();
    let predicted = band_mean_squares(&pred, predicted_fs);
    crate::render::third_octaves()
        .into_iter()
        .enumerate()
        .map(|(b, (center_hz, _, _))| {
            let mut level = db(measured[b], per_unit);
            let mut note = level.is_none().then(|| {
                if measured[b].is_none() {
                    "above the recording's Nyquist frequency".to_string()
                } else {
                    "nothing recorded in the band".to_string()
                }
            });
            if level.is_some_and(|l| l < calibration_db - RESOLUTION_DB) {
                level = None;
                note = Some("below the recording's resolution".into());
            }
            if let (Some(l), Some((bg, bg_unit))) = (level, &noise) {
                match db(bg[b], *bg_unit) {
                    Some(n) if l - n < 6.0 => {
                        level = None;
                        note = Some(format!("only {:.1} dB above the background", l - n));
                    }
                    Some(n) if l - n < 10.0 => {
                        level = Some(10.0 * (10f64.powf(l / 10.0) - 10f64.powf(n / 10.0)).log10());
                        note = Some(format!(
                            "corrected for the background ({:.1} dB above)",
                            l - n
                        ));
                    }
                    _ => {}
                }
            }
            let predicted_db = db(predicted[b], 1.0);
            BandComparison {
                center_hz,
                measured_db: level,
                predicted_db,
                error_db: level.zip(predicted_db).map(|(m, p)| p - m),
                target_db: if center_hz <= 2000.0 { 3.0 } else { 5.0 },
                resolved: resolved.get(b).copied().unwrap_or(false),
                note,
            }
        })
        .collect()
}

/// Pressure of `rec` over `window_s`, Pa, calibrated at `calibration_db`.
pub fn pascals(rec: &Recording, calibration_db: f64, window_s: [f64; 2]) -> Vec<f32> {
    let fs = rec.sample_rate;
    let k = pa_per_unit(calibration_db);
    rec.samples[(window_s[0] * fs) as usize..(window_s[1] * fs) as usize]
        .iter()
        .map(|&v| (v * k) as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::grid::STRAIGHT_PIPE;

    const FS: f64 = 8000.0;

    /// A run-up 1500 → 4500 rpm over 30 s: orders 1–8 at −40 dBFS, the firing order (2) with a
    /// resonance of +20 dB at 3000 rpm, 3 dB down at ±200 rpm; `gain` scales every order.
    fn run_up(gain: f64) -> (Recording, String) {
        let rpm = |t: f64| 1500.0 + 100.0 * t;
        let n = (30.0 * FS) as usize;
        let mut phase = [0.0; 8];
        let samples = (0..n)
            .map(|i| {
                let r = rpm(i as f64 / FS);
                let x = (r - 3000.0) / 200.0;
                // +20 dB: ten times the amplitude.
                let firing = 10.0 / (1.0 + x * x).sqrt();
                (0..8)
                    .map(|k| {
                        phase[k] += 2.0 * PI * (k + 1) as f64 * r / 60.0 / FS;
                        let a = 0.01 * if k == 1 { firing } else { 1.0 };
                        gain * a * phase[k].sin()
                    })
                    .sum()
            })
            .collect();
        let log = (0..=300)
            .map(|i| format!("{:.1},{:.1}\n", 0.1 * i as f64, rpm(0.1 * i as f64)))
            .collect::<String>();
        (
            Recording {
                sample_rate: FS,
                samples,
            },
            format!("Time (s),Engine RPM (rpm)\n{log}"),
        )
    }

    #[test]
    fn order_tracks_of_a_run_up() {
        let project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let (rec, csv) = run_up(1.0);
        let log = RpmLog::parse(&csv).unwrap();
        for (tracks, tolerance) in [
            (
                order_tracks(&project, &rec, Some(&log), 0.0, None).unwrap(),
                0.5,
            ),
            (order_tracks(&project, &rec, None, 0.0, None).unwrap(), 1.0),
        ] {
            let level = |order: f64, rpm: f64| {
                let bin = tracks.rpm.iter().position(|r| *r == rpm).unwrap();
                tracks
                    .orders
                    .iter()
                    .find(|o| o.order == order)
                    .unwrap()
                    .level_db[bin]
                    .unwrap()
            };
            // −40 dBFS off resonance, +20 dB on it.
            for rpm in [1800.0, 4200.0] {
                assert!(
                    (level(3.0, rpm) + 40.0).abs() < tolerance,
                    "{}",
                    level(3.0, rpm)
                );
            }
            assert!(
                (level(2.0, 3000.0) + 20.0).abs() < tolerance,
                "{}",
                level(2.0, 3000.0)
            );
            let drone = tracks.drone.clone().unwrap();
            assert!((drone.peak_rpm - 3000.0).abs() < 50.0, "{}", drone.peak_rpm);
        }
    }

    #[test]
    fn cabin_tf_by_order_ratio_and_impulse() {
        let project = Project::from_json(STRAIGHT_PIPE).unwrap();
        let (ext, csv) = run_up(1.0);
        let (int, _) = run_up(0.1);
        let log = RpmLog::parse(&csv).unwrap();
        let tracks = |r: &Recording| order_tracks(&project, r, Some(&log), 0.0, None).unwrap();
        let tf = cabin_tf_orders(&tracks(&ext), &tracks(&int)).unwrap();
        assert!(
            tf.len() > 20 && tf.iter().all(|p| (p[1] + 20.0).abs() < 0.1),
            "{tf:?}"
        );
        // Balloon pops 2 s apart; the cabin passes 10 % of the pressure.
        let pops = |gain: f64| Recording {
            sample_rate: FS,
            samples: (0..(10.0 * FS) as usize)
                .map(|i| {
                    let t = (i as f64 / FS) % 2.0 - 0.5;
                    gain * (-(t / 2e-4).powi(2)).exp()
                })
                .collect(),
        };
        let tf = cabin_tf_impulse(&pops(1.0), &pops(0.1)).unwrap();
        assert!(
            tf.len() > 30 && tf.iter().all(|p| (p[1] + 20.0).abs() < 0.1),
            "{tf:?}"
        );
    }

    #[test]
    fn reads_recordings_and_logs() {
        let wav = {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut bytes = Cursor::new(Vec::new());
            let mut w = hound::WavWriter::new(&mut bytes, spec).unwrap();
            for s in [16384i16, -1, -16384, -1] {
                w.write_sample(s).unwrap();
            }
            w.finalize().unwrap();
            bytes.into_inner()
        };
        let rec = Recording::parse("run.wav", &wav).unwrap();
        assert_eq!(
            (rec.sample_rate, rec.samples.clone()),
            (48_000.0, vec![0.5, -0.5])
        );
        // Linear PCM CAF: file header, description, data (big-endian 16-bit, mono).
        let mut caf = b"caff\x00\x01\x00\x00".to_vec();
        caf.extend(b"desc");
        caf.extend(32u64.to_be_bytes());
        caf.extend(48_000f64.to_be_bytes());
        caf.extend(b"lpcm");
        caf.extend(0u32.to_be_bytes()); // flags: integer, big-endian
        caf.extend([2u32, 1, 1, 16].iter().flat_map(|x| x.to_be_bytes()));
        caf.extend(b"data");
        caf.extend(8u64.to_be_bytes());
        caf.extend(0u32.to_be_bytes()); // edit count
        caf.extend([16384i16, -16384].iter().flat_map(|x| x.to_be_bytes()));
        let rec = Recording::parse("run.CAF", &caf).unwrap();
        assert_eq!((rec.sample_rate, rec.samples), (48_000.0, vec![0.5, -0.5]));
        let log = RpmLog::parse(
            "Device Time;Engine RPM(rpm)\n28-Sep 12:00:01.5;1000\n28-Sep 12:00:02.5;2000\n",
        )
        .unwrap();
        assert_eq!(log.points, vec![[0.0, 1000.0], [1.0, 2000.0]]);
        assert_eq!(log.at(0.25), Some(1250.0));
        assert_eq!(
            RpmLog::parse("0,800\n2,1200\n").unwrap().at(1.0),
            Some(1000.0)
        );
    }

    /// A 94 dB tone recorded at a calibration of 100 dB for full scale reads 94 dB in its band;
    /// the same tone predicted in pascals agrees; a background 8 dB down is removed energy-wise
    /// and one 3 dB down leaves the band out.
    #[test]
    fn a_hold_compares_band_by_band() {
        let fs = 48_000.0;
        let tone = |amplitude: f64| -> Vec<f64> {
            (0..96_000)
                .map(|i| amplitude * (2.0 * PI * 1000.0 * i as f64 / fs).sin())
                .collect()
        };
        // 94 dB is 1 Pa RMS: √2 Pa peak, at 100 dB full scale a full-scale sine is 2 Pa RMS.
        let rec = Recording {
            sample_rate: fs,
            samples: tone(0.5),
        };
        let predicted: Vec<f32> = tone(std::f64::consts::SQRT_2)
            .iter()
            .map(|&v| v as f32)
            .collect();
        let bands = crate::render::third_octaves();
        let k = bands.iter().position(|b| b.0 == 1000.0).unwrap();
        let resolved = vec![true; bands.len()];
        let c = compare_hold(&rec, 100.0, [0.5, 1.5], None, &predicted, fs, &resolved);
        assert!(
            (c[k].measured_db.unwrap() - 94.0).abs() < 0.05,
            "{:?}",
            c[k]
        );
        assert!(c[k].error_db.unwrap().abs() < 0.05, "{:?}", c[k]);
        let quiet = |db_down: f64| Recording {
            sample_rate: fs,
            samples: tone(0.5 * 10f64.powf(-db_down / 20.0)),
        };
        let bg = quiet(8.0);
        let c = compare_hold(
            &rec,
            100.0,
            [0.5, 1.5],
            Some((&bg, 100.0)),
            &predicted,
            fs,
            &resolved,
        );
        let expected = 10.0 * (10f64.powf(9.4) - 10f64.powf(8.6)).log10();
        assert!(
            (c[k].measured_db.unwrap() - expected).abs() < 0.05,
            "{:?}",
            c[k]
        );
        let bg = quiet(3.0);
        let c = compare_hold(
            &rec,
            100.0,
            [0.5, 1.5],
            Some((&bg, 100.0)),
            &predicted,
            fs,
            &resolved,
        );
        assert_eq!(c[k].measured_db, None);
    }
}
