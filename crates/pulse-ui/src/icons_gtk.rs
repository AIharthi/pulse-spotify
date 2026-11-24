//! Monochrome SVG icon texture cache and bounded cover artwork texture bridge for GTK4.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gtk4::gdk_pixbuf;
use gtk4::glib;
use gtk4::prelude::*;
use pulse_assets::{CoverCache, CoverTier, icons};
use pulse_model::ImageRef;

use crate::{CoverFetchFn, MetricsObserverFn};

/// Cache of tinted vector SVG icons converted into HiDPI `gdk4::Texture` paintables.
pub struct VectorIconCache {
    textures: RefCell<HashMap<(&'static str, String, i32), gdk4::Texture>>,
}

impl Default for VectorIconCache {
    fn default() -> Self {
        Self::new()
    }
}

impl VectorIconCache {
    pub fn new() -> Self {
        Self {
            textures: RefCell::new(HashMap::new()),
        }
    }

    pub fn texture_for(
        &self,
        key: &'static str,
        svg_bytes: &'static [u8],
        hex: &str,
        logical_px: i32,
    ) -> Option<gdk4::Texture> {
        let px = logical_px.max(8);
        let cache_key = (key, hex.to_string(), px);
        if let Some(tex) = self.textures.borrow().get(&cache_key) {
            return Some(tex.clone());
        }

        let tinted = icons::tint_svg(svg_bytes, hex);
        let loader = gdk_pixbuf::PixbufLoader::with_type("svg").ok()?;
        // Render SVG at 2x resolution so fractional scaling stays razor-sharp.
        loader.set_size(px * 2, px * 2);
        if loader.write(&tinted).is_err() || loader.close().is_err() {
            return None;
        }
        let pixbuf = loader.pixbuf()?;
        let texture = gdk4::Texture::for_pixbuf(&pixbuf);
        self.textures.borrow_mut().insert(cache_key, texture.clone());
        Some(texture)
    }

    pub fn apply_to_image(
        &self,
        image: &gtk4::Image,
        key: &'static str,
        svg_bytes: &'static [u8],
        hex: &str,
        logical_px: i32,
    ) {
        image.set_pixel_size(logical_px);
        if let Some(tex) = self.texture_for(key, svg_bytes, hex, logical_px) {
            image.set_paintable(Some(&tex));
        }
    }

    pub fn make_image(
        &self,
        key: &'static str,
        svg_bytes: &'static [u8],
        hex: &str,
        logical_px: i32,
    ) -> gtk4::Image {
        let img = gtk4::Image::new();
        self.apply_to_image(&img, key, svg_bytes, hex, logical_px);
        img
    }
}

/// Applies pointer cursor and accessible metadata cleanly to an interactive GTK widget.
pub fn make_clickable(widget: &(impl IsA<gtk4::Widget> + IsA<gtk4::Accessible>), label: &str) {
    widget.set_cursor_from_name(Some("pointer"));
    widget.set_tooltip_text(Some(label));
    widget.update_property(&[gtk4::accessible::Property::Label(label)]);
}

/// Creates a crisp vector icon button with pointer cursor, tooltip, and accessible label.
pub fn vector_icon_button(
    cache: &VectorIconCache,
    icon_key: &'static str,
    svg: &'static [u8],
    hex: &str,
    px: i32,
    label: &str,
    css_classes: &[&str],
) -> (gtk4::Button, gtk4::Image) {
    let img = cache.make_image(icon_key, svg, hex, px);
    let btn = gtk4::Button::new();
    btn.set_child(Some(&img));
    btn.set_css_classes(css_classes);
    make_clickable(&btn, label);
    (btn, img)
}

/// Bounded cover art texture cache wired to `pulse_assets::CoverCache`.
pub struct CoverTextureStore {
    cache: Arc<CoverCache>,
    fetcher: CoverFetchFn,
    metrics_observer: Option<MetricsObserverFn>,
    textures: RefCell<HashMap<(Arc<str>, CoverTier), gdk4::MemoryTexture>>,
    accents: RefCell<HashMap<Arc<str>, [u8; 4]>>,
    in_flight: RefCell<HashSet<(Arc<str>, CoverTier)>>,
}

impl CoverTextureStore {
    pub fn new(
        cache: Arc<CoverCache>,
        fetcher: CoverFetchFn,
        metrics_observer: Option<MetricsObserverFn>,
    ) -> Self {
        Self {
            cache,
            fetcher,
            metrics_observer,
            textures: RefCell::new(HashMap::new()),
            accents: RefCell::new(HashMap::new()),
            in_flight: RefCell::new(HashSet::new()),
        }
    }

    pub fn cached_texture(&self, id: &str, tier: CoverTier) -> Option<gdk4::MemoryTexture> {
        let key: Arc<str> = Arc::from(id);
        self.textures.borrow().get(&(key, tier)).cloned()
    }

    pub fn cached_accent(&self, id: &str) -> Option<[u8; 4]> {
        self.accents.borrow().get(id).copied()
    }

    pub fn clear_thumbs(&self, loaded_records: usize) {
        let evicted = self.cache.clear_thumbs();
        let mut tex = self.textures.borrow_mut();
        for key in evicted {
            tex.remove(&key);
        }
        tex.retain(|(_, tier), _| *tier != CoverTier::Thumb);
        if let Some(ref obs) = self.metrics_observer {
            obs(self.cache.stats(), loaded_records);
        }
    }

    pub fn purge_all(&self, loaded_records: usize) {
        let evicted = self.cache.purge_ram();
        let mut tex = self.textures.borrow_mut();
        for key in evicted {
            tex.remove(&key);
        }
        tex.clear();
        self.accents.borrow_mut().clear();
        if let Some(ref obs) = self.metrics_observer {
            obs(self.cache.stats(), loaded_records);
        }
    }

    /// Requests a cover texture asynchronously on the GTK main context.
    /// Invokes `on_ready(texture, accent_rgba)` on the main thread when decoded.
    pub fn request_cover<F>(
        self: &Rc<Self>,
        image: ImageRef,
        tier: CoverTier,
        loaded_records: usize,
        on_ready: F,
    ) where
        F: FnOnce(gdk4::MemoryTexture, [u8; 4]) + 'static,
    {
        if let Some(tex) = self.cached_texture(&image.id, tier) {
            let accent = self
                .cached_accent(&image.id)
                .unwrap_or(pulse_assets::DEFAULT_ACCENT_RGBA);
            on_ready(tex, accent);
            return;
        }

        let inflight_key = (Arc::clone(&image.id), tier);
        if !self.in_flight.borrow_mut().insert(inflight_key.clone()) {
            return;
        }

        let this = Rc::clone(self);
        glib::MainContext::default().spawn_local(async move {
            let fetcher = Arc::clone(&this.fetcher);
            let res = this
                .cache
                .get_or_fetch(tier, image.clone(), move |img| fetcher(img))
                .await;
            this.in_flight.borrow_mut().remove(&inflight_key);

            if let Ok(fetched) = res {
                let bytes = glib::Bytes::from_owned(fetched.cover.pixels.to_vec());
                let texture = gdk4::MemoryTexture::new(
                    fetched.cover.width as i32,
                    fetched.cover.height as i32,
                    gdk4::MemoryFormat::R8g8b8a8,
                    &bytes,
                    (fetched.cover.width as usize) * 4,
                );
                {
                    let mut map = this.textures.borrow_mut();
                    for ev in &fetched.evicted {
                        map.remove(ev);
                    }
                    map.insert((Arc::clone(&fetched.id), fetched.tier), texture.clone());
                }
                this.accents
                    .borrow_mut()
                    .insert(Arc::clone(&fetched.id), fetched.cover.accent_rgba);
                if let Some(ref obs) = this.metrics_observer {
                    obs(this.cache.stats(), loaded_records);
                }
                on_ready(texture, fetched.cover.accent_rgba);
            }
        });
    }
}
