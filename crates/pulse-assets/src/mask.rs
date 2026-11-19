//! Cover-art decoding, bounded square downscaling, and anti-aliased rounded-corner masking.

use bytes::Bytes;
use image::imageops::FilterType;

use crate::cache::AssetError;
use crate::palette::extract_accent_rgba;
use crate::{HERO_CORNER_RADIUS_PX, HERO_DIM_PX, THUMB_CORNER_RADIUS_PX, THUMB_DIM_PX};

/// Resolution tier determining decode cap and corner radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverTier {
    Thumb,
    Hero,
}

impl CoverTier {
    /// Maximum width/height in pixels for this tier (`128` for Thumb, `512` for Hero).
    pub const fn max_dim(self) -> u32 {
        match self {
            Self::Thumb => THUMB_DIM_PX,
            Self::Hero => HERO_DIM_PX,
        }
    }

    /// Reference rounded-corner radius in pixels (`8px` for Thumb, `20px` for Hero).
    pub const fn corner_radius_px(self) -> f32 {
        match self {
            Self::Thumb => THUMB_CORNER_RADIUS_PX,
            Self::Hero => HERO_CORNER_RADIUS_PX,
        }
    }
}

/// Decoded, bounded-size, pre-masked RGBA8 cover bitmap ready for `iced::widget::image::Handle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCover {
    pub width: u32,
    pub height: u32,
    pub pixels: Bytes,
    pub accent_rgba: [u8; 4],
}

impl DecodedCover {
    /// Returns raw RGBA8 memory footprint in bytes.
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }
}

/// Decodes compressed `jpeg`/`png`/`webp` artwork, square-crops & downscales to the tier cap,
/// extracts the muted accent tint, and bakes smooth anti-aliased rounded corners into alpha.
pub fn decode_and_mask_cover(
    compressed: &[u8],
    tier: CoverTier,
) -> Result<DecodedCover, AssetError> {
    let img = image::load_from_memory(compressed)
        .map_err(|err| AssetError::Decode(err.to_string()))?;

    let orig_w = img.width();
    let orig_h = img.height();
    if orig_w == 0 || orig_h == 0 {
        return Err(AssetError::Decode(String::from("zero-dimension image")));
    }

    let side = orig_w.min(orig_h);
    let cropped = if orig_w != orig_h {
        let x0 = (orig_w - side) / 2;
        let y0 = (orig_h - side) / 2;
        img.crop_imm(x0, y0, side, side)
    } else {
        img
    };

    let target = side.min(tier.max_dim()).max(1);
    let resized = if side > target {
        cropped.resize_exact(target, target, FilterType::Triangle)
    } else {
        cropped
    };

    let mut rgba = resized.to_rgba8();
    let (w, h) = rgba.dimensions();
    let accent_rgba = extract_accent_rgba(rgba.as_raw(), w, h);

    // Scale corner radius proportionally if Spotify served a smaller image (e.g. 64×64 thumb).
    let scaled_radius = tier.corner_radius_px() * (w as f32 / tier.max_dim() as f32);
    apply_rounded_corner_mask(rgba.as_mut(), w, h, scaled_radius);

    Ok(DecodedCover {
        width: w,
        height: h,
        pixels: Bytes::from(rgba.into_raw()),
        accent_rgba,
    })
}

/// Applies an anti-aliased rounded-rectangle SDF mask to straight RGBA8 pixels in-place.
///
/// Only touches pixels within the four corner squares of side `ceil(radius_px) + 1`.
pub fn apply_rounded_corner_mask(rgba: &mut [u8], w: u32, h: u32, radius_px: f32) {
    let expected = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if w == 0 || h == 0 || rgba.len() < expected || !radius_px.is_finite() || radius_px <= 0.0 {
        return;
    }

    let half_w = w as f32 * 0.5;
    let half_h = h as f32 * 0.5;
    let r = radius_px.min(half_w).min(half_h);
    let corner_band = (r.ceil() as u32).saturating_add(1);

    let inner_x = half_w - r;
    let inner_y = half_h - r;

    for y in 0..h {
        if y >= corner_band && y < h.saturating_sub(corner_band) {
            continue;
        }
        let py = y as f32 + 0.5;
        let dy = (py - half_h).abs() - inner_y;
        if dy <= -0.5 {
            continue;
        }

        for x in 0..w {
            if x >= corner_band && x < w.saturating_sub(corner_band) {
                continue;
            }
            let px = x as f32 + 0.5;
            let dx = (px - half_w).abs() - inner_x;
            if dx <= -0.5 {
                continue;
            }

            let dist = if dx > 0.0 && dy > 0.0 {
                (dx * dx + dy * dy).sqrt() - r
            } else {
                dx.max(dy) - r
            };

            let coverage = (0.5 - dist).clamp(0.0, 1.0);
            if coverage < 1.0 {
                let idx = ((y * w + x) as usize) * 4 + 3;
                rgba[idx] = ((rgba[idx] as f32) * coverage)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::*;

    fn encode_test_png(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, Rgba(color));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        cursor.into_inner()
    }

    #[test]
    fn thumb_downscales_rectangular_cover_and_masks_corners() {
        let png = encode_test_png(400, 300, [210, 90, 50, 255]);
        let decoded = decode_and_mask_cover(&png, CoverTier::Thumb).unwrap();

        assert_eq!(decoded.width, THUMB_DIM_PX);
        assert_eq!(decoded.height, THUMB_DIM_PX);
        assert_eq!(decoded.byte_len(), (128 * 128 * 4) as usize);

        // Outer corners must be transparent.
        assert_eq!(decoded.pixels[3], 0);
        let top_right = ((128 - 1) * 4 + 3) as usize;
        assert_eq!(decoded.pixels[top_right], 0);
        let bottom_left = (((128 - 1) * 128) * 4 + 3) as usize;
        assert_eq!(decoded.pixels[bottom_left], 0);
        let bottom_right = decoded.pixels.len() - 1;
        assert_eq!(decoded.pixels[bottom_right], 0);

        // Center pixel must remain opaque.
        let center = ((64 * 128 + 64) * 4 + 3) as usize;
        assert_eq!(decoded.pixels[center], 255);
    }

    #[test]
    fn hero_caps_at_512_and_preserves_smooth_rounded_edge() {
        let png = encode_test_png(640, 640, [60, 140, 200, 255]);
        let decoded = decode_and_mask_cover(&png, CoverTier::Hero).unwrap();

        assert_eq!(decoded.width, HERO_DIM_PX);
        assert_eq!(decoded.height, HERO_DIM_PX);
        assert_eq!(decoded.byte_len(), (512 * 512 * 4) as usize);
        assert_eq!(decoded.pixels[3], 0);

        // Midpoint along top edge must remain opaque.
        let top_mid = ((256) * 4 + 3) as usize;
        assert_eq!(decoded.pixels[top_mid], 255);

        // Corner geometry check for r=20:
        // (4, 4) is outside (21.92 > 20), (5, 6) is on the anti-aliased arc (19.81), (20, 20) is inside.
        assert_eq!(decoded.pixels[((4 * 512 + 4) * 4 + 3) as usize], 0);
        let aa = decoded.pixels[((5 * 512 + 6) * 4 + 3) as usize];
        assert!(aa > 0 && aa < 255, "expected smooth AA alpha at (5,6), got {aa}");
        assert_eq!(decoded.pixels[((20 * 512 + 20) * 4 + 3) as usize], 255);
    }

    #[test]
    fn zero_negative_and_nan_radius_leave_alpha_untouched() {
        let mut pixels = vec![255u8; 16 * 16 * 4];
        apply_rounded_corner_mask(&mut pixels, 16, 16, 0.0);
        assert_eq!(pixels[3], 255);
        apply_rounded_corner_mask(&mut pixels, 16, 16, -5.0);
        assert_eq!(pixels[3], 255);
        apply_rounded_corner_mask(&mut pixels, 16, 16, f32::NAN);
        assert_eq!(pixels[3], 255);
    }
}
