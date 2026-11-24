//! Cover-derived accent color extraction, Matugen parsing, and WCAG AA adaptive theme tokens.

use bytes::Bytes;

/// Default Warm Analog Amber (`#E59A3E`) fallback for grayscale/empty covers.
pub const DEFAULT_ACCENT_RGBA: [u8; 4] = [229, 154, 62, 255];

/// Hard opacity ceiling for Stage / Focus radial cover tints (`<= 16%`).
pub const MAX_TINT_ALPHA: f32 = 0.16;

/// Dimension of the precomputed radial-alpha tint bitmap (`128x128`).
pub const RADIAL_TINT_DIM_PX: u32 = 128;

const SAMPLE_GRID: u32 = 16;
const HUE_BINS: usize = 24;

/// Complete, mathematically WCAG-verified color token set for either Dark or Light mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveThemeTokens {
    pub is_light: bool,
    pub bg_base: [u8; 3],
    pub bg_surface: [u8; 3],
    pub bg_elevated: [u8; 3],
    pub bg_highlight: [u8; 3],
    pub border_subtle: [u8; 3],
    pub border_strong: [u8; 3],
    pub fg_primary: [u8; 3],
    pub fg_secondary: [u8; 3],
    pub fg_muted: [u8; 3],
    pub accent: [u8; 3],
    pub accent_hover: [u8; 3],
    pub on_accent: [u8; 3],
    pub status_ok: [u8; 3],
    pub status_warn: [u8; 3],
    pub status_err: [u8; 3],
}

impl AdaptiveThemeTokens {
    /// Formats an RGB triple as `#RRGGBB`.
    pub fn hex(rgb: [u8; 3]) -> String {
        format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
    }

    /// Generates an adaptive tonal theme anchored on `raw_accent` for Dark or Light mode.
    ///
    /// Guarantees:
    /// - `fg_primary`, `fg_secondary`, and `fg_muted` achieve `>= 4.5:1` contrast against `bg_base`, `bg_surface`, and `bg_elevated`.
    /// - `accent` achieves `>= 3.3:1` in Dark mode (`>= 4.55:1` in Light mode) against `bg_surface`.
    /// - `on_accent` achieves `>= 4.6:1` against `accent`.
    pub fn from_accent(raw_accent: [u8; 4], is_light: bool) -> Self {
        let (hue, raw_sat, _) = rgb_to_hsl(raw_accent[0], raw_accent[1], raw_accent[2]);
        let has_chroma = raw_sat >= 0.08;
        let tint_sat = if has_chroma { 0.10 } else { 0.03 };

        if is_light {
            let bg_base = hsl_to_rgb(hue, tint_sat, 0.96);
            let bg_surface = hsl_to_rgb(hue, tint_sat * 0.4, 0.992);
            let bg_elevated = hsl_to_rgb(hue, tint_sat, 0.925);
            let bg_highlight = hsl_to_rgb(hue, tint_sat * 1.3, 0.875);
            let border_subtle = hsl_to_rgb(hue, tint_sat, 0.835);
            let border_strong = hsl_to_rgb(hue, tint_sat * 1.2, 0.62);

            let fg_primary = hsl_to_rgb(hue, tint_sat * 0.6, 0.09);
            let fg_secondary = hsl_to_rgb(hue, tint_sat * 0.5, 0.25);
            let fg_muted = ensure_text_contrast(
                hsl_to_rgb(hue, tint_sat * 0.4, 0.36),
                bg_elevated,
                4.65,
                false,
            );

            let acc_sat = if has_chroma {
                raw_sat.clamp(0.45, 0.78)
            } else {
                0.55
            };
            let acc_hue = if has_chroma { hue } else { 32.0 };
            let (accent, on_accent) = ensure_accent_and_ink(
                hsl_to_rgb(acc_hue, acc_sat, 0.38),
                bg_surface,
                4.65,
                true,
            );
            let (accent_hover, _) = ensure_accent_and_ink(
                hsl_to_rgb(acc_hue, acc_sat, 0.31),
                bg_surface,
                5.4,
                true,
            );

            Self {
                is_light: true,
                bg_base,
                bg_surface,
                bg_elevated,
                bg_highlight,
                border_subtle,
                border_strong,
                fg_primary,
                fg_secondary,
                fg_muted,
                accent,
                accent_hover,
                on_accent,
                status_ok: [14, 116, 80],
                status_warn: [148, 86, 0],
                status_err: [185, 28, 44],
            }
        } else {
            let bg_base = hsl_to_rgb(hue, tint_sat, 0.058);
            let bg_surface = hsl_to_rgb(hue, tint_sat, 0.092);
            let bg_elevated = hsl_to_rgb(hue, tint_sat * 1.1, 0.132);
            let bg_highlight = hsl_to_rgb(hue, tint_sat * 1.25, 0.18);
            let border_subtle = hsl_to_rgb(hue, tint_sat, 0.205);
            let border_strong = hsl_to_rgb(hue, tint_sat * 1.1, 0.33);

            let fg_primary = hsl_to_rgb(hue, tint_sat * 0.4, 0.945);
            let fg_secondary = hsl_to_rgb(hue, tint_sat * 0.5, 0.745);
            let fg_muted = ensure_text_contrast(
                hsl_to_rgb(hue, tint_sat * 0.5, 0.61),
                bg_elevated,
                4.65,
                true,
            );

            let acc_sat = if has_chroma {
                raw_sat.clamp(0.45, 0.80)
            } else {
                0.72
            };
            let acc_hue = if has_chroma { hue } else { 33.0 };
            let (accent, on_accent) = ensure_accent_and_ink(
                hsl_to_rgb(acc_hue, acc_sat, 0.59),
                bg_elevated,
                3.6,
                false,
            );
            let (accent_hover, _) = ensure_accent_and_ink(
                hsl_to_rgb(acc_hue, acc_sat, 0.68),
                bg_elevated,
                4.5,
                false,
            );

            Self {
                is_light: false,
                bg_base,
                bg_surface,
                bg_elevated,
                bg_highlight,
                border_subtle,
                border_strong,
                fg_primary,
                fg_secondary,
                fg_muted,
                accent,
                accent_hover,
                on_accent,
                status_ok: [52, 211, 153],
                status_warn: [251, 191, 36],
                status_err: [248, 113, 113],
            }
        }
    }
}

fn pick_best_on_accent(accent: [u8; 3]) -> [u8; 3] {
    let dark_ink = [13, 16, 23];
    let light_ink = [255, 255, 255];
    if contrast_ratio(dark_ink, accent) >= contrast_ratio(light_ink, accent) {
        dark_ink
    } else {
        light_ink
    }
}

fn ensure_accent_and_ink(
    raw_rgb: [u8; 3],
    surface_bg: [u8; 3],
    min_surface_ratio: f32,
    is_light_mode: bool,
) -> ([u8; 3], [u8; 3]) {
    let (h, s, mut l) = rgb_to_hsl(raw_rgb[0], raw_rgb[1], raw_rgb[2]);
    let mut best = raw_rgb;
    for _ in 0..45 {
        let candidate = hsl_to_rgb(h, s, l);
        let ink = pick_best_on_accent(candidate);
        if contrast_ratio(candidate, surface_bg) >= min_surface_ratio
            && contrast_ratio(ink, candidate) >= 4.6
        {
            return (candidate, ink);
        }
        best = candidate;
        l = if is_light_mode {
            (l - 0.02).max(0.10)
        } else {
            (l + 0.02).min(0.88)
        };
    }
    let ink = pick_best_on_accent(best);
    (best, ink)
}

fn ensure_text_contrast(
    fg: [u8; 3],
    bg: [u8; 3],
    min_ratio: f32,
    lighten: bool,
) -> [u8; 3] {
    let (h, s, mut l) = rgb_to_hsl(fg[0], fg[1], fg[2]);
    let mut candidate = fg;
    for _ in 0..40 {
        if contrast_ratio(candidate, bg) >= min_ratio {
            return candidate;
        }
        l = if lighten {
            (l + 0.02).min(0.98)
        } else {
            (l - 0.02).max(0.04)
        };
        candidate = hsl_to_rgb(h, s, l);
    }
    candidate
}

/// Computes WCAG 2.x relative luminance (`0.0..=1.0`) for sRGB `[r, g, b]`.
pub fn relative_luminance(rgb: [u8; 3]) -> f32 {
    fn lin(c: u8) -> f32 {
        let s = f32::from(c) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

/// Computes WCAG 2.x contrast ratio (`1.0..=21.0`) between two sRGB triples.
pub fn contrast_ratio(a: [u8; 3], b: [u8; 3]) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (l1, l2) = if la >= lb { (la, lb) } else { (lb, la) };
    (l1 + 0.05) / (l2 + 0.05)
}

/// Parses `@define-color accent_color #RRGGBB;` (or `accent_bg_color`) from Matugen GTK4 CSS.
pub fn parse_matugen_accent_from_gtk_css(css: &str) -> Option<[u8; 4]> {
    for key in ["accent_color", "accent_bg_color", "theme_selected_bg_color"] {
        for line in css.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("@define-color") {
                continue;
            }
            if !trimmed.contains(key) {
                continue;
            }
            if let Some(hash_pos) = trimmed.find('#') {
                let hex = trimmed[hash_pos + 1..].trim_end_matches(';').trim();
                if hex.len() >= 6
                    && let (Ok(r), Ok(g), Ok(b)) = (
                        u8::from_str_radix(&hex[0..2], 16),
                        u8::from_str_radix(&hex[2..4], 16),
                        u8::from_str_radix(&hex[4..6], 16),
                    )
                {
                    return Some([r, g, b, 255]);
                }
            }
        }
    }
    None
}

/// Reads the host user's Matugen accent color from `$XDG_CONFIG_HOME/gtk-4.0/gtk.css`.
pub fn load_system_matugen_accent() -> Option<[u8; 4]> {
    let cfg = dirs::config_dir()?;
    let css = std::fs::read_to_string(cfg.join("gtk-4.0").join("gtk.css")).ok()?;
    parse_matugen_accent_from_gtk_css(&css)
}

/// Extracts a muted, WCAG-safe accent color `[r, g, b, 255]` from straight RGBA8 pixels.
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

/// Generates a `128x128` straight RGBA8 radial-alpha glow texture for Stage / Focus backdrops.
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

pub(crate) fn hsl_to_rgb(hue_deg: f32, sat: f32, light: f32) -> [u8; 3] {
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
        assert!(max_a <= 41, "max alpha {max_a} exceeded 16% ceiling");
        assert!(max_a >= 38, "center alpha {max_a} unexpectedly low");
        assert_eq!(pixels[3], 0);
        assert_eq!(pixels[pixels.len() - 1], 0);
    }

    #[test]
    fn adaptive_theme_enforces_wcag_aa_across_extreme_hues_dark_and_light() {
        let accents = [
            [235, 188, 186, 255], // Rose Pine
            [229, 154, 62, 255],  // Warm Amber
            [255, 255, 0, 255],   // Extreme Yellow
            [0, 40, 255, 255],    // Deep Blue
            [255, 20, 40, 255],   // Crimson
            [140, 140, 140, 255], // Grayscale
        ];
        for acc in accents {
            for is_light in [false, true] {
                let t = AdaptiveThemeTokens::from_accent(acc, is_light);
                for bg in [t.bg_base, t.bg_surface, t.bg_elevated] {
                    assert!(
                        contrast_ratio(t.fg_primary, bg) >= 4.5,
                        "fg_primary failed AA for light={is_light}"
                    );
                    assert!(
                        contrast_ratio(t.fg_secondary, bg) >= 4.5,
                        "fg_secondary failed AA for light={is_light}"
                    );
                    assert!(
                        contrast_ratio(t.fg_muted, bg) >= 4.5,
                        "fg_muted failed AA for light={is_light}: {}",
                        contrast_ratio(t.fg_muted, bg)
                    );
                }
                let min_acc = if is_light { 4.5 } else { 3.2 };
                assert!(
                    contrast_ratio(t.accent, t.bg_surface) >= min_acc,
                    "accent failed contrast on bg_surface for light={is_light}"
                );
                assert!(
                    contrast_ratio(t.on_accent, t.accent) >= 4.5,
                    "on_accent failed contrast on accent for light={is_light}"
                );
            }
        }
    }

    #[test]
    fn parses_matugen_gtk4_accent_color() {
        let sample = "/* Rose Pine */\n@define-color accent_color #ebbcba;\n@define-color window_bg_color #1f1d2e;";
        assert_eq!(
            parse_matugen_accent_from_gtk_css(sample),
            Some([0xeb, 0xbc, 0xba, 255])
        );
    }
}
