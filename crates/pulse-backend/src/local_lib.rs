//! Incremental local audio scanner, tag reader (`lofty` + `ffprobe` fallback), and cover extractor.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use lofty::config::ParseOptions;
use lofty::file::AudioFile;
use lofty::prelude::{Accessor, TaggedFileExt};
use pulse_model::{AlbumSummary, ArtistSummary, ImageRef, MediaSource, SortOrder, TrackSummary};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{AppPaths, write_atomic};

const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "oga", "opus", "m4a", "mp4", "aac", "wav", "wave", "alac", "wv", "ape",
    "aiff", "aif",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalIndexedTrack {
    pub path: String,
    pub mtime_secs: u64,
    pub byte_len: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
    pub track_number: Option<u32>,
    pub release_year: Option<u16>,
    pub codec_label: String,
    pub cover_path: Option<String>,
}

impl LocalIndexedTrack {
    pub fn to_summary(&self, liked_uris: &HashSet<String>) -> TrackSummary {
        let uri = format!("local:{}", self.path);
        let cover = self.cover_path.as_ref().map(|cp| ImageRef {
            id: Arc::from(format!("localcover:{cp}")),
            url: Some(Arc::from(format!("file://{cp}"))),
        });
        TrackSummary {
            uri: Arc::from(uri.as_str()),
            title: self.title.clone(),
            artist: Arc::from(self.artist.as_str()),
            artist_uri: None,
            album: Arc::from(self.album.as_str()),
            album_uri: None,
            cover,
            duration_ms: self.duration_ms,
            is_liked: liked_uris.contains(&uri),
            is_playable: true,
            added_at_unix: Some(self.mtime_secs.min(i64::MAX as u64) as i64),
            source: MediaSource::Local,
            local_path: Some(Arc::from(self.path.as_str())),
            codec_label: Some(Arc::from(self.codec_label.as_str())),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LocalIndexDisk {
    tracks: Vec<LocalIndexedTrack>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalLibrary {
    pub tracks: Vec<LocalIndexedTrack>,
    pub dirs: Vec<String>,
}

impl LocalLibrary {
    pub fn load_cached(paths: &AppPaths, dirs: Vec<String>) -> Self {
        let index_path = paths.cache_dir.join("local_index.json");
        let tracks = fs::read(&index_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<LocalIndexDisk>(&b).ok())
            .map(|d| d.tracks)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| Path::new(&t.path).is_file())
            .collect();
        Self { tracks, dirs }
    }

    pub fn scan_dirs_blocking(paths: &AppPaths, dirs: &[String], existing: &[LocalIndexedTrack]) -> Vec<LocalIndexedTrack> {
        let prev_map: HashMap<&str, &LocalIndexedTrack> =
            existing.iter().map(|t| (t.path.as_str(), t)).collect();
        let mut seen_paths = HashSet::new();
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = dirs
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect();

        while let Some(cur) = stack.pop() {
            if cur.is_file() {
                if is_audio_file(&cur)
                    && let Some(item) = index_single_file(paths, &cur, &prev_map, &mut seen_paths)
                {
                    out.push(item);
                }
                continue;
            }
            let Ok(entries) = fs::read_dir(&cur) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip hidden directories (.git, .Trash, etc.)
                    let hidden = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'));
                    if !hidden {
                        stack.push(path);
                    }
                } else if path.is_file()
                    && is_audio_file(&path)
                    && let Some(item) = index_single_file(paths, &path, &prev_map, &mut seen_paths)
                {
                    out.push(item);
                }
            }
        }

        out.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then_with(|| a.album.to_lowercase().cmp(&b.album.to_lowercase()))
                .then_with(|| a.track_number.unwrap_or(999).cmp(&b.track_number.unwrap_or(999)))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });

        let index_path = paths.cache_dir.join("local_index.json");
        if let Ok(json) = serde_json::to_vec(&LocalIndexDisk { tracks: out.clone() }) {
            let _ = write_atomic(&index_path, &json, false);
        }
        out
    }

    pub fn index_ad_hoc_path(&mut self, paths: &AppPaths, path: &Path) -> Option<LocalIndexedTrack> {
        let prev_map = HashMap::new();
        let mut seen = HashSet::new();
        let item = index_single_file(paths, path, &prev_map, &mut seen)?;
        if !self.tracks.iter().any(|t| t.path == item.path) {
            self.tracks.push(item.clone());
        }
        Some(item)
    }

    pub fn summaries(&self, liked: &HashSet<String>, sort: SortOrder) -> Vec<TrackSummary> {
        let mut items: Vec<TrackSummary> =
            self.tracks.iter().map(|t| t.to_summary(liked)).collect();
        match sort {
            SortOrder::RecentlyAdded => {
                items.sort_by_key(|t| std::cmp::Reverse(t.added_at_unix.unwrap_or(0)));
            }
            SortOrder::Title => {
                items.sort_by_key(|t| (t.title.to_lowercase(), t.artist.to_lowercase()));
            }
            SortOrder::Artist => {
                items.sort_by_key(|t| (t.artist.to_lowercase(), t.title.to_lowercase()));
            }
            SortOrder::Duration => {
                items.sort_by_key(|t| (t.duration_ms, t.title.to_lowercase()));
            }
            SortOrder::Custom => {}
        }
        items
    }

    pub fn albums(&self) -> Vec<AlbumSummary> {
        let mut groups: BTreeMap<(String, String), Vec<&LocalIndexedTrack>> = BTreeMap::new();
        for t in &self.tracks {
            let album = if t.album.is_empty() {
                "Singles & Unsorted".to_string()
            } else {
                t.album.clone()
            };
            let artist = if t.artist.is_empty() {
                "Unknown Artist".to_string()
            } else {
                t.artist.clone()
            };
            groups.entry((album, artist)).or_default().push(t);
        }
        groups
            .into_iter()
            .map(|((title, artist), list)| {
                let cover = list
                    .iter()
                    .find_map(|t| t.cover_path.as_ref())
                    .map(|cp| ImageRef {
                        id: Arc::from(format!("localcover:{cp}")),
                        url: Some(Arc::from(format!("file://{cp}"))),
                    });
                let year = list.iter().find_map(|t| t.release_year);
                let uri = format!("local_album:{title}\t{artist}");
                AlbumSummary {
                    uri: Arc::from(uri.as_str()),
                    title,
                    artist: Arc::from(artist.as_str()),
                    artist_uri: None,
                    release_year: year,
                    track_count: list.len(),
                    cover,
                    album_type: Arc::from("Local Album"),
                    source: MediaSource::Local,
                }
            })
            .collect()
    }

    pub fn artists(&self) -> Vec<ArtistSummary> {
        let mut by_artist: BTreeMap<String, Option<String>> = BTreeMap::new();
        for t in &self.tracks {
            let name = if t.artist.is_empty() {
                "Unknown Artist".to_string()
            } else {
                t.artist.clone()
            };
            let entry = by_artist.entry(name).or_insert(None);
            if entry.is_none() {
                *entry = t.cover_path.clone();
            }
        }
        by_artist
            .into_iter()
            .map(|(name, cover_path)| {
                let cover = cover_path.map(|cp| ImageRef {
                    id: Arc::from(format!("localcover:{cp}")),
                    url: Some(Arc::from(format!("file://{cp}"))),
                });
                let uri = format!("local_artist:{name}");
                ArtistSummary {
                    uri: Arc::from(uri.as_str()),
                    name,
                    cover,
                    is_followed: false,
                    source: MediaSource::Local,
                }
            })
            .collect()
    }

    pub fn search(&self, query: &str, liked: &HashSet<String>) -> Vec<TrackSummary> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        self.tracks
            .iter()
            .filter(|t| {
                t.title.to_lowercase().contains(&q)
                    || t.artist.to_lowercase().contains(&q)
                    || t.album.to_lowercase().contains(&q)
                    || t.path.to_lowercase().contains(&q)
            })
            .take(50)
            .map(|t| t.to_summary(liked))
            .collect()
    }
}

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            let lower = ext.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&lower.as_str())
        })
}

fn index_single_file(
    paths: &AppPaths,
    path: &Path,
    prev: &HashMap<&str, &LocalIndexedTrack>,
    seen: &mut HashSet<String>,
) -> Option<LocalIndexedTrack> {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical.to_string_lossy().to_string();
    if !seen.insert(path_str.clone()) {
        return None;
    }
    let meta = fs::metadata(&canonical).ok()?;
    if !meta.is_file() {
        return None;
    }
    let byte_len = meta.len();
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Some(cached) = prev.get(path_str.as_str())
        && cached.mtime_secs == mtime_secs
        && cached.byte_len == byte_len
        && cached
            .cover_path
            .as_ref()
            .is_none_or(|p| Path::new(p).exists())
    {
        return Some((*cached).clone());
    }

    Some(extract_metadata_and_cover(
        paths,
        &canonical,
        path_str,
        mtime_secs,
        byte_len,
    ))
}

fn extract_metadata_and_cover(
    paths: &AppPaths,
    path: &Path,
    path_str: String,
    mtime_secs: u64,
    byte_len: u64,
) -> LocalIndexedTrack {
    let fallback_title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown Track")
        .to_string();
    let ext_upper = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_else(|| "AUDIO".to_string());

    let mut title = fallback_title;
    let mut artist = String::from("Local Artist");
    let mut album = String::new();
    let mut duration_ms = 0u32;
    let mut track_number = None;
    let mut release_year = None;
    let mut codec_label = ext_upper.clone();
    let mut cover_path = find_folder_cover(path);

    if let Ok(tagged) = lofty::probe::Probe::open(path)
        .and_then(|p| p.options(ParseOptions::new()).read())
    {
        let props = tagged.properties();
        let dur = props.duration();
        duration_ms = dur.as_millis().min(u32::MAX as u128) as u32;
        let rate_str = props
            .sample_rate()
            .map(|r| format!("{:.1}k", r as f32 / 1000.0))
            .unwrap_or_default();
        let bit_str = props
            .bit_depth()
            .map(|b| format!("{b}b"))
            .or_else(|| props.audio_bitrate().map(|k| format!("{k}k")))
            .unwrap_or_default();
        let parts: Vec<&str> = [ext_upper.as_str(), bit_str.as_str(), rate_str.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        if !parts.is_empty() {
            codec_label = parts.join(" · ");
        }

        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(t) = tag.title()
                && !t.trim().is_empty()
            {
                title = t.trim().to_string();
            }
            if let Some(a) = tag.artist()
                && !a.trim().is_empty()
            {
                artist = a.trim().to_string();
            }
            if let Some(al) = tag.album()
                && !al.trim().is_empty()
            {
                album = al.trim().to_string();
            }
            track_number = tag.track();
            release_year = tag.date().map(|d| d.year);

            if let Some(pic) = tag.pictures().first() {
                let bytes = pic.data();
                if bytes.len() >= 32
                    && let Some(saved) = persist_embedded_cover(paths, bytes)
                {
                    cover_path = Some(saved);
                }
            }
        }
    } else if let Some((dur, t, a, al)) = ffprobe_fallback(path) {
        duration_ms = dur;
        if let Some(v) = t {
            title = v;
        }
        if let Some(v) = a {
            artist = v;
        }
        if let Some(v) = al {
            album = v;
        }
    }

    LocalIndexedTrack {
        path: path_str,
        mtime_secs,
        byte_len,
        title,
        artist,
        album,
        duration_ms,
        track_number,
        release_year,
        codec_label,
        cover_path,
    }
}

fn persist_embedded_cover(paths: &AppPaths, bytes: &[u8]) -> Option<String> {
    let _ = fs::create_dir_all(&paths.cover_cache_dir);
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(24);
    for b in &digest[..12] {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    let out = paths.cover_cache_dir.join(format!("local_{hex}.img"));
    if !out.exists() {
        write_atomic(&out, bytes, false).ok()?;
    }
    Some(out.to_string_lossy().to_string())
}

fn find_folder_cover(audio_path: &Path) -> Option<String> {
    let dir = audio_path.parent()?;
    for name in [
        "cover.jpg",
        "cover.png",
        "cover.webp",
        "folder.jpg",
        "folder.png",
        "front.jpg",
        "Cover.jpg",
    ] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

type FfprobeMeta = (u32, Option<String>, Option<String>, Option<String>);

fn ffprobe_fallback(path: &Path) -> Option<FfprobeMeta> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            path.to_str()?,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let val: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let fmt = val.get("format")?;
    let dur_ms = fmt
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| (s * 1000.0).round() as u32)
        .unwrap_or(0);
    let tags = fmt.get("tags");
    let get_tag = |k: &str| -> Option<String> {
        tags.and_then(|t| t.get(k).or_else(|| t.get(k.to_ascii_uppercase())))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    };
    Some((dur_ms, get_tag("title"), get_tag("artist"), get_tag("album")))
}

/// Extracts embedded lyrics from a local audio file or sibling `.lrc` file.
pub fn read_local_lyrics(audio_path: &Path) -> Option<(String, bool)> {
    let lrc_path = audio_path.with_extension("lrc");
    if lrc_path.is_file()
        && let Ok(text) = fs::read_to_string(&lrc_path)
        && !text.trim().is_empty()
    {
        return Some((text, true));
    }
    let tagged = lofty::probe::Probe::open(audio_path)
        .ok()?
        .options(ParseOptions::new())
        .read()
        .ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let raw = tag.get_string(lofty::tag::ItemKey::Lyrics)?;
    if raw.trim().is_empty() {
        return None;
    }
    let has_ts = raw.contains('[') && raw.contains(':') && raw.contains(']');
    Some((raw.to_string(), has_ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_generated_audio_fixture_and_extracts_metadata() {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("pulse_local_scan_{nanos}"));
        let paths = AppPaths::for_root(&root);
        paths.ensure_dirs().unwrap();
        let music_dir = root.join("music");
        fs::create_dir_all(&music_dir).unwrap();
        let flac_path = music_dir.join("tone.flac");

        let ok = Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-metadata",
                "title=Calibration Tone",
                "-metadata",
                "artist=Pulse Lab",
                "-metadata",
                "album=Benchmarks",
                flac_path.to_str().unwrap(),
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let dirs = vec![music_dir.to_string_lossy().to_string()];
            let indexed = LocalLibrary::scan_dirs_blocking(&paths, &dirs, &[]);
            assert_eq!(indexed.len(), 1);
            assert_eq!(indexed[0].title, "Calibration Tone");
            assert_eq!(indexed[0].artist, "Pulse Lab");
            assert_eq!(indexed[0].album, "Benchmarks");
            assert!(indexed[0].duration_ms >= 1500);
        }
        let _ = fs::remove_dir_all(&root);
    }
}
