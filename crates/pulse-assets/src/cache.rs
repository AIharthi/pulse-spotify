//! Two-tier bounded RAM + disk LRU cache for decoded album artwork.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use pulse_model::ImageRef;

use crate::mask::{CoverTier, DecodedCover, decode_and_mask_cover};
use crate::{MAX_HERO_SLOTS, MAX_THUMB_SLOTS};

/// Errors produced by cover decoding, fetching, or disk persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetError {
    Decode(String),
    Fetch(String),
    Io(String),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(msg) => write!(f, "cover decode error: {msg}"),
            Self::Fetch(msg) => write!(f, "cover fetch error: {msg}"),
            Self::Io(msg) => write!(f, "cover cache io error: {msg}"),
        }
    }
}

impl std::error::Error for AssetError {}

/// Live occupancy snapshot of the asset cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoverStats {
    pub ram_bytes: usize,
    pub thumb_slots_used: usize,
    pub hero_slots_used: usize,
    pub disk_bytes: u64,
}

#[derive(Debug, Default)]
struct AtomicCoverStats {
    ram_bytes: AtomicUsize,
    thumb_slots_used: AtomicUsize,
    hero_slots_used: AtomicUsize,
    disk_bytes: AtomicU64,
}

impl AtomicCoverStats {
    fn snapshot(&self) -> CoverStats {
        CoverStats {
            ram_bytes: self.ram_bytes.load(Ordering::Relaxed),
            thumb_slots_used: self.thumb_slots_used.load(Ordering::Relaxed),
            hero_slots_used: self.hero_slots_used.load(Ordering::Relaxed),
            disk_bytes: self.disk_bytes.load(Ordering::Relaxed),
        }
    }
}

/// Lock-free cloneable reader for backend telemetry polling.
#[derive(Debug, Clone)]
pub struct CoverStatsReader {
    inner: Arc<AtomicCoverStats>,
}

impl CoverStatsReader {
    /// Returns an atomic snapshot of current cover cache RAM and disk usage.
    pub fn snapshot(&self) -> CoverStats {
        self.inner.snapshot()
    }
}

/// Output of [`CoverCache::get_or_fetch`], carrying any LRU keys evicted from RAM.
#[derive(Debug, Clone)]
pub struct CoverFetchResult {
    pub id: Arc<str>,
    pub tier: CoverTier,
    pub cover: DecodedCover,
    /// Keys evicted from the RAM LRU cache; UI MUST drop matching `iced::image::Handle`s.
    pub evicted: Vec<(Arc<str>, CoverTier)>,
}

struct TierLru {
    tier: CoverTier,
    capacity: usize,
    entries: HashMap<Arc<str>, DecodedCover>,
    order: VecDeque<Arc<str>>,
    ram_bytes: usize,
}

impl TierLru {
    fn new(tier: CoverTier, capacity: usize) -> Self {
        Self {
            tier,
            capacity,
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            ram_bytes: 0,
        }
    }

    fn get(&mut self, id: &str) -> Option<DecodedCover> {
        let cover = self.entries.get(id).cloned()?;
        self.touch_existing(id);
        Some(cover)
    }

    fn insert(&mut self, id: Arc<str>, cover: DecodedCover) -> Vec<(Arc<str>, CoverTier)> {
        let mut evicted = Vec::new();
        let new_bytes = cover.byte_len();

        if let Some(prev) = self.entries.insert(Arc::clone(&id), cover) {
            self.ram_bytes = self.ram_bytes - prev.byte_len() + new_bytes;
            self.touch_existing(&id);
            return evicted;
        }

        while self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front()
                && let Some(removed) = self.entries.remove(&oldest)
            {
                self.ram_bytes -= removed.byte_len();
                evicted.push((oldest, self.tier));
            }
        }

        self.ram_bytes += new_bytes;
        self.order.push_back(id);
        evicted
    }

    fn clear(&mut self) -> Vec<(Arc<str>, CoverTier)> {
        let drained = self
            .order
            .drain(..)
            .map(|id| (id, self.tier))
            .collect::<Vec<_>>();
        self.entries.clear();
        self.ram_bytes = 0;
        drained
    }

    fn touch_existing(&mut self, id: &str) {
        if let Some(pos) = self.order.iter().position(|k| k.as_ref() == id)
            && let Some(key) = self.order.remove(pos)
        {
            self.order.push_back(key);
        }
    }
}

struct RamLruState {
    thumbs: TierLru,
    heroes: TierLru,
}

impl RamLruState {
    fn new() -> Self {
        Self {
            thumbs: TierLru::new(CoverTier::Thumb, MAX_THUMB_SLOTS),
            heroes: TierLru::new(CoverTier::Hero, MAX_HERO_SLOTS),
        }
    }

    fn tier_mut(&mut self, tier: CoverTier) -> &mut TierLru {
        match tier {
            CoverTier::Thumb => &mut self.thumbs,
            CoverTier::Hero => &mut self.heroes,
        }
    }

    fn sync_stats(&self, stats: &AtomicCoverStats) {
        stats
            .ram_bytes
            .store(self.thumbs.ram_bytes + self.heroes.ram_bytes, Ordering::Relaxed);
        stats
            .thumb_slots_used
            .store(self.thumbs.entries.len(), Ordering::Relaxed);
        stats
            .hero_slots_used
            .store(self.heroes.entries.len(), Ordering::Relaxed);
    }
}

#[derive(Debug, Default)]
struct DiskEntryMeta {
    seq: u64,
    size: u64,
}

struct DiskCacheState {
    dir: PathBuf,
    cap_bytes: u64,
    next_seq: u64,
    total_bytes: u64,
    files: HashMap<PathBuf, DiskEntryMeta>,
}

impl DiskCacheState {
    fn new(dir: PathBuf, cap_mb: u64) -> Self {
        let mut state = Self {
            dir,
            cap_bytes: cap_mb.saturating_mul(1024 * 1024),
            next_seq: 1,
            total_bytes: 0,
            files: HashMap::new(),
        };
        if !state.dir.as_os_str().is_empty() && state.cap_bytes > 0 {
            state.scan_existing();
            state.prune_to_cap();
        }
        state
    }

    fn remove_file_entry(&mut self, path: &Path) {
        if let Some(meta) = self.files.remove(path) {
            self.total_bytes = self.total_bytes.saturating_sub(meta.size);
        }
        let _ = std::fs::remove_file(path);
    }

    fn scan_existing(&mut self) {
        self.files.clear();
        self.total_bytes = 0;

        if self.dir.as_os_str().is_empty() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };

        let mut discovered: Vec<(u128, PathBuf, u64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            discovered.push((mtime_ns, path, meta.len()));
        }

        // Seed monotonic sequence in ascending mtime order so disk eviction survives restarts.
        discovered.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        for (_mtime, path, size) in discovered {
            let seq = self.next_seq;
            self.next_seq += 1;
            self.total_bytes += size;
            self.files.insert(path, DiskEntryMeta { seq, size });
        }
    }

    fn touch(&mut self, path: &Path) {
        if let Some(meta) = self.files.get_mut(path) {
            meta.seq = self.next_seq;
            self.next_seq += 1;
        }
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_modified(SystemTime::now());
        }
    }

    fn record_write(&mut self, path: PathBuf, size: u64) {
        if let Some(prev) = self.files.insert(
            path,
            DiskEntryMeta {
                seq: self.next_seq,
                size,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(prev.size);
        }
        self.next_seq += 1;
        self.total_bytes += size;
        self.prune_to_cap();
    }

    fn prune_to_cap(&mut self) {
        while self.total_bytes > self.cap_bytes && !self.files.is_empty() {
            let oldest = self
                .files
                .iter()
                .min_by_key(|(path, meta)| (meta.seq, *path))
                .map(|(path, _)| path.clone());
            let Some(victim) = oldest else {
                break;
            };
            if let Some(meta) = self.files.remove(&victim) {
                self.total_bytes = self.total_bytes.saturating_sub(meta.size);
                let _ = std::fs::remove_file(&victim);
            }
        }
    }

    fn clear_all(&mut self) -> io::Result<()> {
        for path in self.files.keys() {
            let _ = std::fs::remove_file(path);
        }
        self.files.clear();
        self.total_bytes = 0;

        if self.dir.exists() {
            for entry in std::fs::read_dir(&self.dir)?.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("bin") {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok(())
    }
}

type InFlightKey = (CoverTier, Arc<str>);
type InFlightMap = HashMap<InFlightKey, Weak<tokio::sync::Mutex<()>>>;

/// Bounded two-tier RAM (`32` thumbs + `2` heroes) and disk LRU cache for album covers.
pub struct CoverCache {
    ram: Mutex<RamLruState>,
    disk: Mutex<DiskCacheState>,
    in_flight: Mutex<InFlightMap>,
    stats: Arc<AtomicCoverStats>,
    tmp_seq: AtomicU64,
}

impl CoverCache {
    /// Creates a cache rooted at `$XDG_CACHE_HOME/pulse-spotify/covers/`.
    pub fn new(disk_cap_mb: u64) -> Self {
        let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        Self::with_dir(base.join("pulse-spotify").join("covers"), disk_cap_mb)
    }

    /// Creates a cache at an explicit directory (primarily for deterministic testing).
    pub fn with_dir(cache_dir: PathBuf, disk_cap_mb: u64) -> Self {
        let disk_state = DiskCacheState::new(cache_dir, disk_cap_mb);
        let stats = Arc::new(AtomicCoverStats::default());
        stats
            .disk_bytes
            .store(disk_state.total_bytes, Ordering::Relaxed);

        Self {
            ram: Mutex::new(RamLruState::new()),
            disk: Mutex::new(disk_state),
            in_flight: Mutex::new(HashMap::new()),
            stats,
            tmp_seq: AtomicU64::new(1),
        }
    }

    /// Returns a lock-free stats reader safe to poll from backend telemetry threads.
    pub fn stats_reader(&self) -> CoverStatsReader {
        CoverStatsReader {
            inner: Arc::clone(&self.stats),
        }
    }

    /// Returns the current RAM and disk cache usage snapshot.
    pub fn stats(&self) -> CoverStats {
        self.stats.snapshot()
    }

    /// Updates the disk cache limit (in MiB) and prunes immediately if needed.
    pub fn set_disk_cap_mb(&self, disk_cap_mb: u64) {
        self.set_disk_cap_bytes(disk_cap_mb.saturating_mul(1024 * 1024));
    }

    /// Updates the disk cache limit in exact bytes and prunes oldest files immediately.
    pub fn set_disk_cap_bytes(&self, cap_bytes: u64) {
        let mut disk = self.disk.lock().unwrap();
        disk.cap_bytes = cap_bytes;
        disk.prune_to_cap();
        self.stats
            .disk_bytes
            .store(disk.total_bytes, Ordering::Relaxed);
    }

    /// Synchronous RAM-only lookup; updates LRU recency on hit.
    pub fn get_cached(&self, tier: CoverTier, id: &str) -> Option<DecodedCover> {
        let mut ram = self.ram.lock().unwrap();
        ram.tier_mut(tier).get(id)
    }

    /// Clears all Tier 1 thumbnails (e.g. on entering Compact Mini-Player mode).
    /// Returns evicted keys so the UI drops matching `image::Handle`s immediately.
    pub fn clear_thumbs(&self) -> Vec<(Arc<str>, CoverTier)> {
        let mut ram = self.ram.lock().unwrap();
        let evicted = ram.thumbs.clear();
        ram.sync_stats(&self.stats);
        evicted
    }

    /// Clears all decoded covers (thumbs + heroes) from RAM, returning evicted keys.
    pub fn purge_ram(&self) -> Vec<(Arc<str>, CoverTier)> {
        let mut ram = self.ram.lock().unwrap();
        let mut evicted = ram.thumbs.clear();
        evicted.extend(ram.heroes.clear());
        ram.sync_stats(&self.stats);
        evicted
    }

    /// Removes all compressed cover files from the disk cache directory.
    pub fn clear_disk(&self) -> io::Result<()> {
        let mut disk = self.disk.lock().unwrap();
        let res = disk.clear_all();
        self.stats
            .disk_bytes
            .store(disk.total_bytes, Ordering::Relaxed);
        res
    }

    /// Fetches, decodes, pre-masks, and caches a cover image with in-flight deduplication
    /// and cancellation safety.
    ///
    /// If the future is dropped while awaiting `fetcher`, the per-key lock releases cleanly so
    /// subsequent callers can proceed without hanging or leaking compressed bytes in RAM.
    pub async fn get_or_fetch<F, Fut>(
        &self,
        tier: CoverTier,
        image: ImageRef,
        fetcher: F,
    ) -> Result<CoverFetchResult, AssetError>
    where
        F: FnOnce(ImageRef) -> Fut,
        Fut: Future<Output = Result<Bytes, String>>,
    {
        if let Some(cover) = self.get_cached(tier, &image.id) {
            return Ok(CoverFetchResult {
                id: image.id,
                tier,
                cover,
                evicted: Vec::new(),
            });
        }

        let key_mutex = self.acquire_key_mutex(tier, Arc::clone(&image.id));
        let _guard = key_mutex.lock().await;

        // Re-check RAM after acquiring per-key lock (dedup hit when another caller finished).
        if let Some(cover) = self.get_cached(tier, &image.id) {
            return Ok(CoverFetchResult {
                id: image.id,
                tier,
                cover,
                evicted: Vec::new(),
            });
        }

        // Check compressed disk cache; corrupt files are removed so we can try remaining
        // candidates or fall back to `fetcher`.
        if let Some(decoded) = self.decode_from_disk_cache(&image.id, tier) {
            let evicted = self.insert_ram(tier, Arc::clone(&image.id), decoded.clone());
            return Ok(CoverFetchResult {
                id: image.id,
                tier,
                cover: decoded,
                evicted,
            });
        }

        let compressed = fetcher(image.clone())
            .await
            .map_err(AssetError::Fetch)?;
        let decoded = decode_and_mask_cover(&compressed, tier)?;
        self.write_disk_compressed(&image.id, tier, &compressed);
        drop(compressed);

        let evicted = self.insert_ram(tier, Arc::clone(&image.id), decoded.clone());
        Ok(CoverFetchResult {
            id: image.id,
            tier,
            cover: decoded,
            evicted,
        })
    }

    fn acquire_key_mutex(&self, tier: CoverTier, id: Arc<str>) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.in_flight.lock().unwrap();
        map.retain(|_, weak| weak.strong_count() > 0);
        let key = (tier, id);
        if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        let created = Arc::new(tokio::sync::Mutex::new(()));
        map.insert(key, Arc::downgrade(&created));
        created
    }

    fn insert_ram(
        &self,
        tier: CoverTier,
        id: Arc<str>,
        cover: DecodedCover,
    ) -> Vec<(Arc<str>, CoverTier)> {
        let mut ram = self.ram.lock().unwrap();
        let evicted = ram.tier_mut(tier).insert(id, cover);
        ram.sync_stats(&self.stats);
        evicted
    }

    fn decode_from_disk_cache(&self, id: &str, tier: CoverTier) -> Option<DecodedCover> {
        let candidates = {
            let disk = self.disk.lock().unwrap();
            if disk.cap_bytes == 0 || disk.dir.as_os_str().is_empty() {
                return None;
            }
            match tier {
                CoverTier::Hero => vec![disk_path(&disk.dir, id, CoverTier::Hero)],
                // Thumbs may downscale an already cached Hero image to save a network fetch.
                CoverTier::Thumb => vec![
                    disk_path(&disk.dir, id, CoverTier::Thumb),
                    disk_path(&disk.dir, id, CoverTier::Hero),
                ],
            }
        };

        for path in candidates {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            match decode_and_mask_cover(&bytes, tier) {
                Ok(decoded) => {
                    let mut disk = self.disk.lock().unwrap();
                    disk.touch(&path);
                    return Some(decoded);
                }
                Err(_) => {
                    let mut disk = self.disk.lock().unwrap();
                    disk.remove_file_entry(&path);
                    self.stats
                        .disk_bytes
                        .store(disk.total_bytes, Ordering::Relaxed);
                }
            }
        }
        None
    }

    fn write_disk_compressed(&self, id: &str, tier: CoverTier, compressed: &[u8]) {
        let (dir, cap_bytes) = {
            let disk = self.disk.lock().unwrap();
            (disk.dir.clone(), disk.cap_bytes)
        };

        if dir.as_os_str().is_empty() || cap_bytes == 0 || compressed.len() as u64 > cap_bytes {
            return;
        }
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }

        let target = disk_path(&dir, id, tier);
        let seq = self.tmp_seq.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!(
            ".tmp_{}_{}.part",
            std::process::id(),
            seq
        );
        let tmp_path = dir.join(tmp_name);

        if std::fs::write(&tmp_path, compressed).is_err() {
            let _ = std::fs::remove_file(&tmp_path);
            return;
        }
        if std::fs::rename(&tmp_path, &target).is_err() {
            let _ = std::fs::remove_file(&tmp_path);
            return;
        }

        let mut disk = self.disk.lock().unwrap();
        disk.record_write(target, compressed.len() as u64);
        self.stats
            .disk_bytes
            .store(disk.total_bytes, Ordering::Relaxed);
    }
}

fn disk_path(dir: &Path, id: &str, tier: CoverTier) -> PathBuf {
    let prefix: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(36)
        .collect();
    let hash = fnv1a_64(id.as_bytes());
    let suffix = match tier {
        CoverTier::Thumb => "t",
        CoverTier::Hero => "h",
    };
    let name = if prefix.is_empty() {
        format!("{hash:016x}.{suffix}.bin")
    } else {
        format!("{prefix}_{hash:016x}.{suffix}.bin")
    };
    dir.join(name)
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io::Cursor;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Poll, Wake, Waker};

    use image::{ImageFormat, Rgba, RgbaImage};

    use super::*;

    struct ThreadWaker(std::thread::Thread);

    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut fut = Box::pin(fut);
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(val) => return val,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    fn temp_cache_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pulse_assets_test_{tag}_{}_{}",
            std::process::id(),
            nanos
        ))
    }

    fn sample_png(color: [u8; 4], dim: u32) -> Bytes {
        let img = RgbaImage::from_pixel(dim, dim, Rgba(color));
        let mut cursor = Cursor::new(Vec::new());
        img.write_to(&mut cursor, ImageFormat::Png).unwrap();
        Bytes::from(cursor.into_inner())
    }

    fn image_ref(id: &str) -> ImageRef {
        ImageRef {
            id: Arc::from(id),
            url: None,
        }
    }

    #[test]
    fn ram_lru_enforces_32_thumb_and_2_hero_caps_and_reports_evictions() {
        let dir = temp_cache_dir("lru");
        let cache = CoverCache::with_dir(dir.clone(), 0); // disk off for pure RAM test
        let png = sample_png([200, 120, 40, 255], 128);

        // Insert 35 thumbs -> caps at 32, evicting 0, 1, 2 in order.
        let mut evicted_thumbs = Vec::new();
        for i in 0..35 {
            let png_clone = png.clone();
            let res = block_on(cache.get_or_fetch(
                CoverTier::Thumb,
                image_ref(&format!("thumb-{i}")),
                move |_| async move { Ok(png_clone) },
            ))
            .unwrap();
            evicted_thumbs.extend(res.evicted);
        }

        assert_eq!(evicted_thumbs.len(), 3);
        assert_eq!(evicted_thumbs[0].0.as_ref(), "thumb-0");
        assert_eq!(evicted_thumbs[1].0.as_ref(), "thumb-1");
        assert_eq!(evicted_thumbs[2].0.as_ref(), "thumb-2");
        assert!(cache.get_cached(CoverTier::Thumb, "thumb-0").is_none());
        assert!(cache.get_cached(CoverTier::Thumb, "thumb-34").is_some());

        // Insert 4 heroes -> caps at 2, evicting hero-0, hero-1.
        let hero_png = sample_png([40, 120, 200, 255], 512);
        let mut evicted_heroes = Vec::new();
        for i in 0..4 {
            let png_clone = hero_png.clone();
            let res = block_on(cache.get_or_fetch(
                CoverTier::Hero,
                image_ref(&format!("hero-{i}")),
                move |_| async move { Ok(png_clone) },
            ))
            .unwrap();
            evicted_heroes.extend(res.evicted);
        }

        assert_eq!(evicted_heroes.len(), 2);
        assert_eq!(evicted_heroes[0].0.as_ref(), "hero-0");
        assert_eq!(evicted_heroes[1].0.as_ref(), "hero-1");

        let stats = cache.stats();
        assert_eq!(stats.thumb_slots_used, MAX_THUMB_SLOTS);
        assert_eq!(stats.hero_slots_used, MAX_HERO_SLOTS);
        assert!(
            stats.ram_bytes <= 4 * 1024 * 1024,
            "raw cover RAM {} exceeded 4.0 MiB ceiling",
            stats.ram_bytes
        );

        // Mini-player thumb drop clears only thumbs while preserving heroes.
        let cleared = cache.clear_thumbs();
        assert_eq!(cleared.len(), MAX_THUMB_SLOTS);
        let stats_after = cache.stats();
        assert_eq!(stats_after.thumb_slots_used, 0);
        assert_eq!(stats_after.hero_slots_used, MAX_HERO_SLOTS);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_cache_prunes_oldest_entries_when_exceeding_byte_budget() {
        let dir = temp_cache_dir("disk_evict");
        let png_a = sample_png([10, 20, 30, 255], 128);
        let png_b = sample_png([40, 50, 60, 255], 128);
        let png_c = sample_png([70, 80, 90, 255], 128);

        // Allow ~2.5 compressed PNG files in disk budget so the 3rd write forces LRU eviction.
        let single_len = png_a.len() as u64;
        let cache = CoverCache::with_dir(dir.clone(), 1);
        cache.set_disk_cap_bytes(single_len * 2 + single_len / 2);

        for (id, bytes) in [("a", png_a), ("b", png_b), ("c", png_c)] {
            block_on(cache.get_or_fetch(
                CoverTier::Thumb,
                image_ref(id),
                move |_| async move { Ok(bytes) },
            ))
            .unwrap();
        }

        assert!(cache.stats().disk_bytes <= single_len * 2 + single_len / 2);
        assert!(!disk_path(&dir, "a", CoverTier::Thumb).exists());
        assert!(disk_path(&dir, "b", CoverTier::Thumb).exists());
        assert!(disk_path(&dir, "c", CoverTier::Thumb).exists());

        // Clear RAM and verify "b" still restores from disk without invoking fetcher.
        cache.purge_ram();
        let fetch_calls = AtomicUsize::new(0);
        let res = block_on(cache.get_or_fetch(
            CoverTier::Thumb,
            image_ref("b"),
            |_| async {
                fetch_calls.fetch_add(1, Ordering::SeqCst);
                Err(String::from("should hit disk"))
            },
        ))
        .unwrap();
        assert_eq!(fetch_calls.load(Ordering::SeqCst), 0);
        assert_eq!(res.cover.width, 128);

        cache.clear_disk().unwrap();
        assert_eq!(cache.stats().disk_bytes, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_mid_fetch_releases_key_lock_for_next_waiter() {
        let dir = temp_cache_dir("cancel");
        let cache = Arc::new(CoverCache::with_dir(dir.clone(), 1));
        let png = sample_png([220, 160, 60, 255], 128);

        // Start a future that returns Pending once, then drop (cancel) it mid-flight.
        let mut yielded_once = false;
        let pending_once = std::future::poll_fn(move |_cx| {
            if !yielded_once {
                yielded_once = true;
                Poll::Pending
            } else {
                Poll::Ready(Ok(Bytes::new()))
            }
        });

        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        {
            let mut fut: Pin<Box<dyn Future<Output = Result<CoverFetchResult, AssetError>>>> =
                Box::pin(cache.get_or_fetch(
                    CoverTier::Thumb,
                    image_ref("cancel-test"),
                    move |_| pending_once,
                ));
            assert!(matches!(fut.as_mut().poll(&mut cx), Poll::Pending));
            // `fut` is dropped here (simulating rapid scroll cancellation).
        }

        // Subsequent caller for the same key must NOT deadlock or see poisoned state.
        let png_clone = png.clone();
        let res = block_on(cache.get_or_fetch(
            CoverTier::Thumb,
            image_ref("cancel-test"),
            move |_| async move { Ok(png_clone) },
        ))
        .unwrap();
        assert_eq!(res.cover.width, 128);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_waiters_deduplicate_fetcher_invocation() {
        let dir = temp_cache_dir("dedup");
        let cache = Arc::new(CoverCache::with_dir(dir.clone(), 0));
        let png = sample_png([100, 200, 150, 255], 128);
        let fetches = Arc::new(AtomicUsize::new(0));

        // Poll future A until it pauses inside fetcher; poll future B until it parks on key mutex.
        let mut gate_open = false;
        let fetches_a = Arc::clone(&fetches);
        let png_a = png.clone();
        let mut fut_a = Box::pin(cache.get_or_fetch(
            CoverTier::Thumb,
            image_ref("shared-key"),
            move |_| async move {
                fetches_a.fetch_add(1, Ordering::SeqCst);
                std::future::poll_fn(|_cx| {
                    if gate_open {
                        Poll::Ready(Ok(png_a.clone()))
                    } else {
                        gate_open = true;
                        Poll::Pending
                    }
                })
                .await
            },
        ));

        let fetches_b = Arc::clone(&fetches);
        let png_b = png.clone();
        let mut fut_b = Box::pin(cache.get_or_fetch(
            CoverTier::Thumb,
            image_ref("shared-key"),
            move |_| async move {
                fetches_b.fetch_add(1, Ordering::SeqCst);
                Ok(png_b)
            },
        ));

        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = Context::from_waker(&waker);

        assert!(matches!(fut_a.as_mut().poll(&mut cx), Poll::Pending));
        assert!(matches!(fut_b.as_mut().poll(&mut cx), Poll::Pending));

        let res_a = match fut_a.as_mut().poll(&mut cx) {
            Poll::Ready(r) => r.unwrap(),
            Poll::Pending => panic!("fut_a should resolve on second poll"),
        };
        let res_b = match fut_b.as_mut().poll(&mut cx) {
            Poll::Ready(r) => r.unwrap(),
            Poll::Pending => panic!("fut_b should resolve from RAM dedup hit"),
        };

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(res_a.cover.width, 128);
        assert_eq!(res_b.cover.width, 128);
        assert!(res_b.evicted.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_disk_entry_is_evicted_and_falls_back_to_fetcher() {
        let dir = temp_cache_dir("corrupt_evict");
        std::fs::create_dir_all(&dir).unwrap();
        let corrupt_path = disk_path(&dir, "bad-cover", CoverTier::Thumb);
        std::fs::write(&corrupt_path, b"not-a-valid-image").unwrap();

        let cache = CoverCache::with_dir(dir.clone(), 1024 * 1024);
        assert!(cache.stats().disk_bytes > 0);

        let png = sample_png([40, 180, 120, 255], 128);
        let fetched = block_on(cache.get_or_fetch(
            CoverTier::Thumb,
            image_ref("bad-cover"),
            move |_| async move { Ok(png) },
        ))
        .unwrap();

        assert_eq!(fetched.cover.width, 128);
        // Corrupt file must have been replaced with valid compressed bytes.
        let disk_bytes = std::fs::read(&corrupt_path).unwrap();
        assert_ne!(disk_bytes, b"not-a-valid-image");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
