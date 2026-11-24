//! Memory-bounded cover art decoding, pre-masking, and disk/LRU caching (`<= 4.0 MiB` raw).

pub mod cache;
pub mod icons;
pub mod mask;
pub mod palette;

pub use cache::{AssetError, CoverCache, CoverFetchResult, CoverStats, CoverStatsReader};
pub use mask::{CoverTier, DecodedCover, apply_rounded_corner_mask, decode_and_mask_cover};
pub use palette::{
    AdaptiveThemeTokens, DEFAULT_ACCENT_RGBA, MAX_TINT_ALPHA, RADIAL_TINT_DIM_PX,
    build_radial_tint_rgba, contrast_ratio, extract_accent_rgba, load_system_matugen_accent,
    parse_matugen_accent_from_gtk_css, relative_luminance,
};
pub use pulse_model::ImageRef;

/// Enforced decoded bitmap ceilings aligned with `docs/UI_SPEC.md` Section 6.2.
pub const MAX_THUMB_SLOTS: usize = 32;
pub const MAX_HERO_SLOTS: usize = 2;
pub const THUMB_DIM_PX: u32 = 128;
pub const HERO_DIM_PX: u32 = 512;
pub const THUMB_CORNER_RADIUS_PX: f32 = 8.0;
pub const HERO_CORNER_RADIUS_PX: f32 = 20.0;
