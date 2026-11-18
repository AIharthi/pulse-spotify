//! Process memory, cover-cache, and disk-cache telemetry collection.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU64, AtomicUsize, Ordering};

use pulse_model::TelemetrySnapshot;

use crate::config::AppPaths;

const BYTES_PER_MIB: f32 = 1024.0 * 1024.0;

/// Parsed process memory metrics from `/proc/self/status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcMemInfo {
    /// Current resident set size in bytes (`VmRSS`).
    pub rss_bytes: u64,
    /// Peak resident set size in bytes (`VmHWM`).
    pub hwm_bytes: u64,
}

#[derive(Default)]
struct CoverStatsInner {
    cover_cache_bytes: AtomicU64,
    thumb_used: AtomicU16,
    thumb_cap: AtomicU16,
    hero_used: AtomicU8,
    hero_cap: AtomicU8,
    loaded_track_records: AtomicUsize,
}

/// Thread-safe handle allowing UI/cover layers to publish cache occupancy metrics
/// into backend telemetry snapshots without cross-crate locking.
#[derive(Clone, Default)]
pub struct CoverStatsHook {
    inner: Arc<CoverStatsInner>,
}

impl CoverStatsHook {
    /// Creates a new zero-initialized telemetry hook.
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates in-memory decoded cover cache usage and slot counts.
    pub fn set_cover_cache(
        &self,
        bytes: u64,
        thumb_used: u16,
        thumb_cap: u16,
        hero_used: u8,
        hero_cap: u8,
    ) {
        self.inner.cover_cache_bytes.store(bytes, Ordering::Relaxed);
        self.inner.thumb_used.store(thumb_used, Ordering::Relaxed);
        self.inner.thumb_cap.store(thumb_cap, Ordering::Relaxed);
        self.inner.hero_used.store(hero_used, Ordering::Relaxed);
        self.inner.hero_cap.store(hero_cap, Ordering::Relaxed);
    }

    /// Updates the count of loaded track metadata records held by the backend/UI.
    pub fn set_loaded_track_records(&self, count: usize) {
        self.inner
            .loaded_track_records
            .store(count, Ordering::Relaxed);
    }
}

/// Parses `VmRSS` and `VmHWM` fields from Linux `/proc/<pid>/status` text.
pub fn parse_proc_status(contents: &str) -> ProcMemInfo {
    let mut info = ProcMemInfo::default();
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            info.rss_bytes = parse_proc_size_bytes(rest);
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            info.hwm_bytes = parse_proc_size_bytes(rest);
        }
    }
    info
}

fn parse_proc_size_bytes(value: &str) -> u64 {
    let mut parts = value.split_whitespace();
    let num: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    match parts.next() {
        Some("kB") | Some("KB") | Some("KiB") => num.saturating_mul(1024),
        Some("mB") | Some("MB") | Some("MiB") => num.saturating_mul(1024 * 1024),
        Some("gB") | Some("GB") | Some("GiB") => num.saturating_mul(1024 * 1024 * 1024),
        _ => num,
    }
}

/// Reads current and peak RSS for this process from `/proc/self/status`.
pub fn read_self_mem() -> ProcMemInfo {
    match fs::read_to_string("/proc/self/status") {
        Ok(contents) => parse_proc_status(&contents),
        Err(_) => ProcMemInfo::default(),
    }
}

/// Recursively sums regular-file byte sizes under `root`, ignoring symlinks and missing paths.
pub fn dir_size_bytes(root: &Path) -> u64 {
    if !root.exists() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file()
                && let Ok(meta) = entry.metadata()
            {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Collects a full [`TelemetrySnapshot`] combining `/proc/self/status`,
/// cover-cache counters, and on-disk cache directories.
pub fn collect_telemetry(paths: &AppPaths, hook: &CoverStatsHook) -> TelemetrySnapshot {
    let mem = read_self_mem();
    let audio_bytes = dir_size_bytes(&paths.audio_cache_dir);
    let cover_disk_bytes = dir_size_bytes(&paths.cover_cache_dir);
    let cover_mem_bytes = hook.inner.cover_cache_bytes.load(Ordering::Relaxed);

    TelemetrySnapshot {
        process_rss_mb: mem.rss_bytes as f32 / BYTES_PER_MIB,
        process_hwm_mb: mem.hwm_bytes as f32 / BYTES_PER_MIB,
        cover_cache_mb: cover_mem_bytes as f32 / BYTES_PER_MIB,
        cover_thumb_slots_used: hook.inner.thumb_used.load(Ordering::Relaxed),
        cover_thumb_slots_cap: hook.inner.thumb_cap.load(Ordering::Relaxed),
        cover_hero_slots_used: hook.inner.hero_used.load(Ordering::Relaxed),
        cover_hero_slots_cap: hook.inner.hero_cap.load(Ordering::Relaxed),
        loaded_track_records: hook.inner.loaded_track_records.load(Ordering::Relaxed),
        disk_audio_cache_mb: audio_bytes as f32 / BYTES_PER_MIB,
        disk_cover_cache_mb: cover_disk_bytes as f32 / BYTES_PER_MIB,
    }
}
