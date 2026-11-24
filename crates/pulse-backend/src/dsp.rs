//! Real-time 10-band biquad parametric EQ, stereo balance/mono, soft limiter, and spectrum analyzer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use pulse_model::{EQ_BAND_FREQS_HZ, EqConfig, SpectrumSnapshot};

const SAMPLE_RATE: f32 = 44_100.0;
const BIQUAD_Q: f32 = std::f32::consts::SQRT_2;
const ANALYZER_WINDOW: usize = 512;
const NUM_BINS: usize = 48;
const NUM_WAVE: usize = 64;

#[derive(Debug, Clone, Copy, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoeffs {
    fn peaking(freq_hz: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 0.05 {
            return Self {
                b0: 1.0,
                b1: 0.0,
                b2: 0.0,
                a1: 0.0,
                a2: 0.0,
            };
        }
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq_hz / SAMPLE_RATE;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * BIQUAD_Q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    #[inline]
    fn process(&mut self, x: f32, c: BiquadCoeffs) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        if !y.is_finite() {
            self.z1 = 0.0;
            self.z2 = 0.0;
            return x;
        }
        y
    }
}

struct DspInner {
    config: EqConfig,
    coeffs: [BiquadCoeffs; 10],
    states_l: [BiquadState; 10],
    states_r: [BiquadState; 10],
    preamp_lin: f32,
    gain_l: f32,
    gain_r: f32,
}

impl DspInner {
    fn from_config(cfg: EqConfig) -> Self {
        let mut inner = Self {
            config: cfg.clone(),
            coeffs: [BiquadCoeffs::peaking(1000.0, 0.0); 10],
            states_l: [BiquadState::default(); 10],
            states_r: [BiquadState::default(); 10],
            preamp_lin: 1.0,
            gain_l: 1.0,
            gain_r: 1.0,
        };
        inner.update(cfg);
        inner
    }

    fn update(&mut self, cfg: EqConfig) {
        self.config = cfg;
        for (idx, (&freq, &db)) in EQ_BAND_FREQS_HZ
            .iter()
            .zip(self.config.bands_db.iter())
            .enumerate()
        {
            self.coeffs[idx] = BiquadCoeffs::peaking(freq, db.clamp(-12.0, 12.0));
        }
        self.preamp_lin = 10.0_f32.powf(self.config.preamp_db.clamp(-12.0, 12.0) / 20.0);
        let bal = self.config.balance.clamp(-1.0, 1.0);
        self.gain_l = if bal > 0.0 { 1.0 - bal } else { 1.0 };
        self.gain_r = if bal < 0.0 { 1.0 + bal } else { 1.0 };
    }
}

struct AnalyzerState {
    ring: Vec<f32>,
    ring_pos: usize,
    hann: Vec<f32>,
    cos_lut: Vec<Vec<f32>>,
    sin_lut: Vec<Vec<f32>>,
    bins: Vec<f32>,
    waveform: Vec<f32>,
    peak_l: f32,
    peak_r: f32,
    active: bool,
}

impl AnalyzerState {
    fn new() -> Self {
        let hann: Vec<f32> = (0..ANALYZER_WINDOW)
            .map(|n| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / ANALYZER_WINDOW as f32).cos())
            })
            .collect();

        // Precompute Goertzel / DFT basis vectors for 48 log-spaced musical frequencies (32 Hz .. 15 kHz).
        let mut cos_lut = Vec::with_capacity(NUM_BINS);
        let mut sin_lut = Vec::with_capacity(NUM_BINS);
        let min_f = 32.0_f32.ln();
        let max_f = 15_000.0_f32.ln();
        for b in 0..NUM_BINS {
            let t = b as f32 / (NUM_BINS - 1) as f32;
            let freq = (min_f + t * (max_f - min_f)).exp();
            let omega = 2.0 * std::f32::consts::PI * freq / SAMPLE_RATE;
            let mut c_row = Vec::with_capacity(ANALYZER_WINDOW);
            let mut s_row = Vec::with_capacity(ANALYZER_WINDOW);
            for n in 0..ANALYZER_WINDOW {
                let ang = omega * n as f32;
                c_row.push(ang.cos());
                s_row.push(ang.sin());
            }
            cos_lut.push(c_row);
            sin_lut.push(s_row);
        }

        Self {
            ring: vec![0.0; ANALYZER_WINDOW],
            ring_pos: 0,
            hann,
            cos_lut,
            sin_lut,
            bins: vec![0.0; NUM_BINS],
            waveform: vec![0.0; NUM_WAVE],
            peak_l: 0.0,
            peak_r: 0.0,
            active: false,
        }
    }

    fn ingest_interleaved(&mut self, samples: &[f32]) {
        let mut max_l = 0.0_f32;
        let mut max_r = 0.0_f32;
        for frame in samples.as_chunks::<2>().0 {
            let l = frame[0].abs();
            let r = frame[1].abs();
            if l > max_l {
                max_l = l;
            }
            if r > max_r {
                max_r = r;
            }
            self.ring[self.ring_pos] = (frame[0] + frame[1]) * 0.5;
            self.ring_pos = (self.ring_pos + 1) % ANALYZER_WINDOW;
        }
        self.peak_l = (self.peak_l * 0.55).max(max_l.min(1.0));
        self.peak_r = (self.peak_r * 0.55).max(max_r.min(1.0));

        // Downsample recent window into 64 waveform samples.
        let step = ANALYZER_WINDOW / NUM_WAVE;
        for (i, w) in self.waveform.iter_mut().enumerate() {
            let idx = (self.ring_pos + i * step) % ANALYZER_WINDOW;
            *w = self.ring[idx].clamp(-1.0, 1.0);
        }

        // Compute spectral magnitude with perceptual log compression and smooth decay.
        let norm = 2.0 / ANALYZER_WINDOW as f32;
        for b in 0..NUM_BINS {
            let cos_row = &self.cos_lut[b];
            let sin_row = &self.sin_lut[b];
            let mut re = 0.0_f32;
            let mut im = 0.0_f32;
            for n in 0..ANALYZER_WINDOW {
                let sample =
                    self.ring[(self.ring_pos + n) % ANALYZER_WINDOW] * self.hann[n];
                re += sample * cos_row[n];
                im += sample * sin_row[n];
            }
            let mag = (re * re + im * im).sqrt() * norm;
            // High-frequency perceptual boost + dB normalization (-54 dB .. 0 dB).
            let tilt = 1.0 + (b as f32 / NUM_BINS as f32) * 1.6;
            let db = 20.0 * (mag * tilt + 1e-6).log10();
            let scaled = ((db + 54.0) / 54.0).clamp(0.0, 1.0);
            let prev = self.bins[b];
            self.bins[b] = if scaled > prev {
                prev * 0.35 + scaled * 0.65
            } else {
                prev * 0.78 + scaled * 0.22
            };
        }
        self.active = true;
    }

    fn decay_idle(&mut self) {
        let mut any = false;
        for b in &mut self.bins {
            *b *= 0.80;
            if *b > 0.005 {
                any = true;
            } else {
                *b = 0.0;
            }
        }
        for w in &mut self.waveform {
            *w *= 0.75;
        }
        self.peak_l *= 0.80;
        self.peak_r *= 0.80;
        self.active = any;
    }

    fn snapshot(&self) -> SpectrumSnapshot {
        SpectrumSnapshot {
            bins: self.bins.clone(),
            waveform: self.waveform.clone(),
            peak_l: self.peak_l,
            peak_r: self.peak_r,
            active: self.active,
        }
    }
}

/// Thread-safe shared audio processor + visualizer tap used by both Spotify `LowLatencyPulseSink`
/// and `DirectAudioEngine`.
pub struct SharedDsp {
    inner: Mutex<DspInner>,
    analyzer: Mutex<AnalyzerState>,
    visualizer_enabled: AtomicBool,
}

impl SharedDsp {
    pub fn new(initial: EqConfig, visualizer_enabled: bool) -> Self {
        Self {
            inner: Mutex::new(DspInner::from_config(initial)),
            analyzer: Mutex::new(AnalyzerState::new()),
            visualizer_enabled: AtomicBool::new(visualizer_enabled),
        }
    }

    pub fn set_eq_config(&self, cfg: EqConfig) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.update(cfg);
        }
    }

    pub fn set_visualizer_enabled(&self, enabled: bool) {
        self.visualizer_enabled.store(enabled, Ordering::Release);
        if !enabled && let Ok(mut a) = self.analyzer.lock() {
            a.decay_idle();
        }
    }

    pub fn clear_analyzer(&self) {
        if let Ok(mut a) = self.analyzer.lock() {
            a.decay_idle();
        }
    }

    pub fn snapshot_spectrum(&self) -> SpectrumSnapshot {
        if !self.visualizer_enabled.load(Ordering::Acquire) {
            return SpectrumSnapshot::default();
        }
        if let Ok(a) = self.analyzer.lock() {
            a.snapshot()
        } else {
            SpectrumSnapshot::default()
        }
    }

    /// Applies 10-band biquad EQ, preamp, balance, mono downmix, soft limiter, and feeds analyzer
    /// on interleaved stereo `f32` frames (`[-1.0, 1.0]`).
    pub fn process_interleaved_f32(&self, samples: &mut [f32]) {
        if let Ok(mut inner) = self.inner.lock() {
            let eq_on = inner.config.enabled;
            let mono = inner.config.mono;
            let preamp = inner.preamp_lin;
            let gl = inner.gain_l;
            let gr = inner.gain_r;
            let need_channel_ops = eq_on || mono || (gl - 1.0).abs() > 1e-4 || (gr - 1.0).abs() > 1e-4;

            if need_channel_ops {
                let coeffs = inner.coeffs;
                let inner_ref = &mut *inner;
                for frame in samples.as_chunks_mut::<2>().0 {
                    let mut l = frame[0] * preamp;
                    let mut r = frame[1] * preamp;

                    if eq_on {
                        for (band, &coeff) in coeffs.iter().enumerate() {
                            l = inner_ref.states_l[band].process(l, coeff);
                            r = inner_ref.states_r[band].process(r, coeff);
                        }
                    }

                    if mono {
                        let m = (l + r) * 0.5;
                        l = m;
                        r = m;
                    }

                    l *= gl;
                    r *= gr;

                    frame[0] = soft_limit(l);
                    frame[1] = soft_limit(r);
                }
            }
        }

        if self.visualizer_enabled.load(Ordering::Relaxed)
            && let Ok(mut a) = self.analyzer.lock()
        {
            a.ingest_interleaved(samples);
        }
    }

    /// Processes `librespot` interleaved `f64` samples in place before `Converter::f64_to_s16`.
    pub fn process_interleaved_f64(&self, samples: &mut [f64]) {
        let mut f32_buf: Vec<f32> = samples.iter().map(|&s| s as f32).collect();
        self.process_interleaved_f32(&mut f32_buf);
        for (dst, src) in samples.iter_mut().zip(f32_buf) {
            *dst = f64::from(src);
        }
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    let abs = x.abs();
    if abs <= 0.92 {
        x
    } else {
        let sign = x.signum();
        let excess = abs - 0.92;
        sign * (0.92 + 0.08 * (excess / 0.08).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_boost_remains_finite_and_soft_limited() {
        let cfg = EqConfig {
            enabled: true,
            preamp_db: 6.0,
            bands_db: [12.0; 10],
            ..Default::default()
        };
        let dsp = SharedDsp::new(cfg, true);

        let mut buf = vec![0.0_f32; 1024];
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let s = (2.0 * std::f32::consts::PI * 120.0 * i as f32 / SAMPLE_RATE).sin() * 0.8;
            frame[0] = s;
            frame[1] = s;
        }
        dsp.process_interleaved_f32(&mut buf);
        for &s in &buf {
            assert!(s.is_finite());
            assert!(s.abs() <= 1.001, "soft limiter exceeded 1.0: {s}");
        }
        let spec = dsp.snapshot_spectrum();
        assert!(spec.active);
        assert!(spec.bins.iter().any(|&v| v > 0.2));
    }

    #[test]
    fn mono_and_balance_shape_channels_accurately() {
        let cfg = EqConfig {
            mono: true,
            balance: 1.0, // hard right -> left muted
            ..Default::default()
        };
        let dsp = SharedDsp::new(cfg, false);
        let mut buf = vec![0.8_f32, 0.2_f32, -0.4_f32, 0.4_f32];
        dsp.process_interleaved_f32(&mut buf);
        assert!(buf[0].abs() < 1e-5);
        assert!((buf[1] - 0.5).abs() < 1e-4);
    }
}
