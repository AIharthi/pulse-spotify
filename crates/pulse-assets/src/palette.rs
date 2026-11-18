//! Cover-derived accent color extraction and radial glow tint generation.

use bytes::Bytes;

/// Default Warm Analog Amber (`#E59A3E`) fallback for grayscale/empty covers.
pub const DEFAULT_ACCENT_RGBA: [u8; 4] = [229, 154, 62, 255];

/// Hard opacity ceiling for Stage / Focus radial cover tints (`<= 16%`).
pub const MAX_TINT_ALPHA: f32 = 0.16;

/// Dimension of the precomputed radial-alpha tint bitmap (`128×128`).
pub const RADIAL_TINT_DIM_PX: u32 = 128;

const SAMPLE_GRID: u32 = 16;
const HUE_BINS: usize = 24;

/// Extracts a muted, WCAG-safe accent color `[r, g, b, 255]` from straight RGBA8 pixels.
///
/// Downsamples uniformly to a `16×16` grid, picks the dominant chromatic hue bucket,
/// clamps HSL saturation to `[0.20, 0.45]` and lightness to `[0.25, 0.45]`, and falls
/// back to [`DEFAULT_ACCENT_RGBA`] for grayscale or transparent images.
pub fn extract_accent_rgba(rgba: &[u8], w: u32, h: u32) -> [u8; 4] {
    let expected_len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < expected_len {
        return DEFAULT_ACCENT_RGBA;
    }

    #[derive(Clone, Copy, Default)]
    struct HueBin {
        weight: f32,
        cos_sum: f32,
        sin_sum: f32,
        sat_sum: f32,
        light_sum: f32,
    }

    let mut bins = [HueBin::default(); HUE_BINS];
    let mut total_weight = 0.0_f32;

    let gx_max = SAMPLE_GRID.min(w);
    let gy_max = SAMPLE_GRID.min(h);

    for gy in 0..gy_max {
        let py = ((gy as u64 * 2 + 1) * h as u64 / (2 * gy_max as u64)) as u32;
        for gx in 0..gx_max {
            let px = ((gx as u64 * 2 + 1) * w as u64 / (2 * gx_max as u64)) as u32;
            let idx = ((py * w + px) as usize) * 4;
            let r = rgba[idx];
            let g = rgba[idx + 1];
            let b = rgba[idx + 2];
            let a = rgba[idx + 3];
            if a < 128 {
                continue;
            }

            let (hue_deg, sat, light) = rgb_to_hsl(r, g, b);
            // Exclude near-gray, crushed blacks, and blown-out whites from hue voting.
            if sat < 0.12 || !(0.08..=0.92).contains(&light) {
                continue;
            }

            let midtone_bias = (1.0 - (light - 0.5).abs() * 1.4).max(0.10);
            let weight = sat * midtone_bias;
            let bin_idx = (((hue_deg / 360.0) * HUE_BINS as f32).floor() as usize) % HUE_BINS;
            let rad = hue_deg.to_radians();

            let bin = &mut bins[bin_idx];
            bin.weight += weight;
            bin.cos_sum += rad.cos() * weight;
            bin.sin_sum += rad.sin() * weight;
            bin.sat_sum += sat * weight;
            bin.light_sum += light * weight;
            total_weight += weight;
        }
    }

    if total_weight <= 1e-5 {
        return DEFAULT_ACCENT_RGBA;
    }

    let best = bins
        .iter()
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
        .copied()
        .unwrap_or_default();

    if best.weight <= 1e-5 {
        return DEFAULT_ACCENT_RGBA;
    }

    let mut mean_hue = best.sin_sum.atan2(best.cos_sum).to_degrees();
    if mean_hue < 0.0 {
        mean_hue += 360.0;
    }
    let clamped_sat = (best.sat_sum / best.weight).clamp(0.20, 0.45);
    let clamped_light = (best.light_sum / best.weight).clamp(0.25, 0.45);

    let [r, g, b] = hsl_to_rgb(mean_hue, clamped_sat, clamped_light);
    [r, g, b, 255]
}

/// Generates a `128×128` straight RGBA8 radial-alpha glow texture for Stage / Focus backdrops.
///
/// `max_alpha` is hard-clamped to `[0.0, 0.16]` (`<= 16%` opacity at the center) and falls off
/// smoothly to `0` at the outer radius so edges blend invisibly into `--bg-canvas`.
pub fn build_radial_tint_rgba(accent: [u8; 4], max_alpha: f32) -> (u32, u32, Bytes) {
    let dim = RADIAL_TINT_DIM_PX;
    let capped_alpha = if max_alpha.is_finite() {
        max_alpha.clamp(0.0, MAX_TINT_ALPHA)
    } else {
        MAX_TINT_ALPHA
    };

    let center = dim as f32 * 0.5;
    let inv_radius = 1.0 / center;
    let mut buf = vec![0_u8; (dim * dim * 4) as usize];

    for y in 0..dim {
        let dy = (y as f32 + 0.5 - center) * inv_radius;
        for x in 0..dim {
            let dx = (x as f32 + 0.5 - center) * inv_radius;
            let dist_sq = dx * dx + dy * dy;
            let falloff = if dist_sq >= 1.0 {
                0.0
            } else {
                let t = 1.0 - dist_sq;
                t * t
            };

            let alpha = (capped_alpha * falloff * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
            let idx = ((y * dim + x) as usize) * 4;
            buf[idx] = accent[0];
            buf[idx + 1] = accent[1];
            buf[idx + 2] = accent[2];
            buf[idx + 3] = alpha;
        }
    }

    (dim, dim, Bytes::from(buf))
}

pub(crate) fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = f32::from(r) / 255.0;
    let gf = f32::from(g) / 255.0;
    let bf = f32::from(b) / 255.0;

    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let light = (max + min) * 0.5;
    let delta = max - min;

    if delta <= 1e-6 {
        return (0.0, 0.0, light);
    }

    let sat = if light > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };

    let mut hue = if (max - rf).abs() <= 1e-6 {
        ((gf - bf) / delta) + if gf < bf { 6.0 } else { 0.0 }
    } else if (max - gf).abs() <= 1e-6 {
        ((bf - rf) / delta) + 2.0
    } else {
        ((rf - gf) / delta) + 4.0
    };
    hue *= 60.0;
    if hue >= 360.0 {
        hue -= 360.0;
    }

    (hue, sat.clamp(0.0, 1.0), light.clamp(0.0, 1.0))
}

fn hsl_to_rgb(hue_deg: f32, sat: f32, light: f32) -> [u8; 3] {
    if sat <= 1e-6 {
        let v = (light * 255.0).round().clamp(0.0, 255.0) as u8;
        return [v, v, v];
    }

    let q = if light < 0.5 {
        light * (1.0 + sat)
    } else {
        light + sat - light * sat
    };
    let p = 2.0 * light - q;
    let h = (hue_deg.rem_euclid(360.0)) / 360.0;

    let r = hue_to_channel(p, q, h + 1.0 / 3.0);
    let g = hue_to_channel(p, q, h);
    let b = hue_to_channel(p, q, h - 1.0 / 3.0);

    [
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn hue_to_channel(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 0.5 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_image_falls_back_to_default_amber() {
        let mut rgba = vec![0_u8; 32 * 32 * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            *px = [140, 140, 140, 255];
        }
        assert_eq!(extract_accent_rgba(&rgba, 32, 32), DEFAULT_ACCENT_RGBA);
    }

    #[test]
    fn vibrant_cover_clamps_hsl_saturation_and_lightness() {
        // Pure vivid cyan (#00FFFF -> S=1.0, L=0.5) should be muted into target bounds.
        let mut rgba = vec![0_u8; 32 * 32 * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            *px = [0, 255, 255, 255];
        }
        let accent = extract_accent_rgba(&rgba, 32, 32);
        assert_eq!(accent[3], 255);
        let (_h, s, l) = rgb_to_hsl(accent[0], accent[1], accent[2]);
        assert!(
            (0.19..=0.46).contains(&s),
            "expected clamped saturation in [0.20, 0.45], got {s}"
        );
        assert!(
            (0.24..=0.46).contains(&l),
            "expected clamped lightness in [0.25, 0.45], got {l}"
        );
    }

    #[test]
    fn radial_tint_respects_16_percent_alpha_ceiling_and_fades_to_zero() {
        let (w, h, pixels) = build_radial_tint_rgba([120, 180, 240, 255], 0.90);
        assert_eq!(w, 128);
        assert_eq!(h, 128);
        assert_eq!(pixels.len(), 128 * 128 * 4);

        let max_a = pixels.as_chunks::<4>().0.iter().map(|c| c[3]).max().unwrap();
        // 0.16 * 255 = 40.8 -> 41 max.
        assert!(max_a <= 41, "max alpha {max_a} exceeded 16% ceiling");
        assert!(max_a >= 38, "center alpha {max_a} unexpectedly low");

        // Corner (0,0) is outside unit circle -> alpha must be 0.
        assert_eq!(pixels[3], 0);
        // Bottom-right corner (127, 127) -> alpha must be 0.
        assert_eq!(pixels[pixels.len() - 1], 0);
    }
}
