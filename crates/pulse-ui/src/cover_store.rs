//! Strictly bounded Iced `image::Handle` LRU cache synchronized with `pulse-assets` caps.
//!
//! Never allocates `image::Handle::from_rgba` inside `view()`: handles are constructed once on
//! decode completion (`CoverReady` / `RadialTintReady`) and evicted entries are dropped immediately
//! so `iced_tiny_skia` frees their premultiplied `Pixmap` allocations.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bytes::Bytes;
use iced::widget::image::Handle;
use pulse_assets::{MAX_HERO_SLOTS, MAX_THUMB_SLOTS};

/// Cover resolution tier matching `docs/UI_SPEC.md` Section 6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoverTier {
    /// Virtualized list & crate grid artwork (`<= 128x128`, max 32 handles).
    Thumb,
    /// Stage / Focus Stage / Mini-Player sleeve artwork (`<= 512x512`, max 2 handles).
    Hero,
}

#[derive(Debug, Clone)]
struct HandleEntry {
    handle: Handle,
    accent_rgba: [u8; 4],
}

#[derive(Debug, Default)]
struct BoundedHandleLru {
    entries: HashMap<Arc<str>, HandleEntry>,
    order: VecDeque<Arc<str>>,
}

impl BoundedHandleLru {
    fn insert(
        &mut self,
        key: Arc<str>,
        handle: Handle,
        accent_rgba: [u8; 4],
        max_slots: usize,
    ) {
        self.remove_order(&key);
        self.order.push_back(Arc::clone(&key));
        self.entries
            .insert(key, HandleEntry { handle, accent_rgba });

        while self.entries.len() > max_slots {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k.as_ref() == key)
            && let Some(owned) = self.order.remove(pos)
        {
            self.order.push_back(owned);
        }
    }

    fn peek(&self, key: &str) -> Option<&HandleEntry> {
        self.entries.get(key)
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    fn remove(&mut self, key: &str) -> bool {
        self.remove_order(key);
        self.entries.remove(key).is_some()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn remove_order(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k.as_ref() == key) {
            self.order.remove(pos);
        }
    }
}

/// UI-side GPU/Skia handle registry with strict hard slot limits (`32` thumbs + `2` heroes + `1` radial tint).
#[derive(Debug, Default)]
pub struct CoverHandleCache {
    thumbs: BoundedHandleLru,
    heroes: BoundedHandleLru,
    radial_tint: Option<Handle>,
}

impl CoverHandleCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a newly decoded RGBA8 buffer once into the appropriate bounded LRU tier.
    pub fn insert_decoded(
        &mut self,
        key: Arc<str>,
        tier: CoverTier,
        width: u32,
        height: u32,
        pixels: Bytes,
        accent_rgba: [u8; 4],
    ) {
        let handle = Handle::from_rgba(width, height, pixels);
        match tier {
            CoverTier::Thumb => {
                self.thumbs
                    .insert(key, handle, accent_rgba, MAX_THUMB_SLOTS);
            }
            CoverTier::Hero => {
                self.heroes
                    .insert(key, handle, accent_rgba, MAX_HERO_SLOTS);
            }
        }
    }

    /// Replaces the single `128x128` radial accent tint handle behind Stage / Focus covers.
    pub fn set_radial_tint(&mut self, width: u32, height: u32, pixels: Bytes) {
        self.radial_tint = Some(Handle::from_rgba(width, height, pixels));
    }

    /// Looks up a thumbnail handle immutably during `view()`.
    pub fn thumb(&self, key: &str) -> Option<&Handle> {
        self.thumbs.peek(key).map(|entry| &entry.handle)
    }

    /// Looks up a hero cover handle immutably during `view()`.
    pub fn hero(&self, key: &str) -> Option<&Handle> {
        self.heroes.peek(key).map(|entry| &entry.handle)
    }

    /// Looks up either hero or thumbnail handle as fallback (useful in Mini-Player / Stage).
    pub fn best_for(&self, key: &str) -> Option<&Handle> {
        self.hero(key).or_else(|| self.thumb(key))
    }

    /// Returns the clamped RGBA accent cached alongside `key`, if present.
    pub fn accent_for(&self, key: &str) -> Option<[u8; 4]> {
        self.heroes
            .peek(key)
            .or_else(|| self.thumbs.peek(key))
            .map(|entry| entry.accent_rgba)
    }

    /// Returns the cached radial accent glow overlay handle, if generated.
    pub fn radial_tint(&self) -> Option<&Handle> {
        self.radial_tint.as_ref()
    }

    /// Marks `key` as recently used in its tier LRU during `update()`.
    pub fn touch(&mut self, key: &str, tier: CoverTier) {
        match tier {
            CoverTier::Thumb => self.thumbs.touch(key),
            CoverTier::Hero => self.heroes.touch(key),
        }
    }

    /// Evicts a single cover handle from `tier` (mirroring `pulse-assets` RAM evictions).
    pub fn remove(&mut self, key: &str, tier: CoverTier) -> bool {
        match tier {
            CoverTier::Thumb => self.thumbs.remove(key),
            CoverTier::Hero => self.heroes.remove(key),
        }
    }

    /// Releases all 32 list/crate thumbnail handles (called when switching to Mini-Player).
    pub fn clear_thumbs(&mut self) {
        self.thumbs.clear();
    }

    /// Releases every cached UI cover handle and radial tint buffer.
    pub fn purge(&mut self) {
        self.thumbs.clear();
        self.heroes.clear();
        self.radial_tint = None;
    }

    pub fn thumb_count(&self) -> usize {
        self.thumbs.len()
    }

    pub fn hero_count(&self) -> usize {
        self.heroes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_rgba(w: u32, h: u32, fill: u8) -> Bytes {
        Bytes::from(vec![fill; (w * h * 4) as usize])
    }

    #[test]
    fn enforces_thumb_and_hero_lru_caps() {
        let mut cache = CoverHandleCache::new();
        for i in 0..(MAX_THUMB_SLOTS + 8) {
            let key: Arc<str> = Arc::from(format!("thumb-{i}"));
            cache.insert_decoded(
                key,
                CoverTier::Thumb,
                4,
                4,
                dummy_rgba(4, 4, i as u8),
                [229, 154, 62, 255],
            );
        }
        assert_eq!(cache.thumb_count(), MAX_THUMB_SLOTS);
        assert!(cache.thumb("thumb-0").is_none());
        assert!(cache.thumb("thumb-7").is_none());
        assert!(cache.thumb("thumb-8").is_some());

        for i in 0..(MAX_HERO_SLOTS + 3) {
            let key: Arc<str> = Arc::from(format!("hero-{i}"));
            cache.insert_decoded(
                key,
                CoverTier::Hero,
                8,
                8,
                dummy_rgba(8, 8, i as u8),
                [100, 120, 140, 255],
            );
        }
        assert_eq!(cache.hero_count(), MAX_HERO_SLOTS);
        assert!(cache.hero("hero-0").is_none());
        assert!(cache.hero("hero-4").is_some());

        cache.clear_thumbs();
        assert_eq!(cache.thumb_count(), 0);
        assert_eq!(cache.hero_count(), MAX_HERO_SLOTS);

        cache.set_radial_tint(4, 4, dummy_rgba(4, 4, 16));
        assert!(cache.radial_tint().is_some());
        assert!(cache.remove("hero-4", CoverTier::Hero));
        assert!(cache.hero("hero-4").is_none());
        assert!(!cache.remove("missing", CoverTier::Thumb));
        cache.purge();
        assert_eq!(cache.hero_count(), 0);
        assert!(cache.radial_tint().is_none());
    }
}
