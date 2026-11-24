//! Persistent user data: unified favorites, cross-source custom playlists, M3U import/export,
//! session restore, and real local listening insights.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pulse_model::{
    CustomPlaylistSummary, ListeningStatsSnapshot, MediaSource, RepeatMode, TrackSummary,
};
use serde::{Deserialize, Serialize};

use crate::config::{AppPaths, write_atomic};
use crate::providers::source_index;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnifiedLikesStore {
    pub tracks: Vec<TrackSummary>,
}

impl UnifiedLikesStore {
    pub fn load(paths: &AppPaths) -> Self {
        let p = paths.data_dir.join("likes.json");
        fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) {
        let p = paths.data_dir.join("likes.json");
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = write_atomic(&p, &bytes, false);
        }
    }

    pub fn uri_set(&self) -> HashSet<String> {
        self.tracks.iter().map(|t| t.uri.to_string()).collect()
    }

    pub fn toggle(&mut self, mut track: TrackSummary) -> bool {
        if let Some(pos) = self.tracks.iter().position(|t| t.uri == track.uri) {
            self.tracks.remove(pos);
            false
        } else {
            track.is_liked = true;
            track.added_at_unix = Some(now_secs() as i64);
            self.tracks.insert(0, track);
            true
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPlaylist {
    pub id: String,
    pub name: String,
    pub tracks: Vec<TrackSummary>,
    pub created_at_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomPlaylistsStore {
    pub playlists: Vec<CustomPlaylist>,
}

impl CustomPlaylistsStore {
    pub fn load(paths: &AppPaths) -> Self {
        let p = paths.data_dir.join("playlists.json");
        fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) {
        let p = paths.data_dir.join("playlists.json");
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = write_atomic(&p, &bytes, false);
        }
    }

    pub fn summaries(&self) -> Vec<CustomPlaylistSummary> {
        self.playlists
            .iter()
            .map(|p| CustomPlaylistSummary {
                id: p.id.clone(),
                name: p.name.clone(),
                track_count: p.tracks.len(),
                total_duration_ms: p.tracks.iter().map(|t| u64::from(t.duration_ms)).sum(),
                cover: p.tracks.iter().find_map(|t| t.cover.clone()),
            })
            .collect()
    }

    pub fn create(&mut self, name: &str) -> &CustomPlaylist {
        let id = format!("pl_{}_{}", now_secs(), self.playlists.len() + 1);
        let trimmed = name.trim();
        self.playlists.insert(
            0,
            CustomPlaylist {
                id,
                name: if trimmed.is_empty() {
                    "Untitled Playlist".to_string()
                } else {
                    trimmed.to_string()
                },
                tracks: Vec::new(),
                created_at_secs: now_secs(),
            },
        );
        &self.playlists[0]
    }

    pub fn find(&self, id: &str) -> Option<&CustomPlaylist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut CustomPlaylist> {
        self.playlists.iter_mut().find(|p| p.id == id)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some(pl) = self.find_mut(id) {
            pl.name = trimmed.to_string();
            true
        } else {
            false
        }
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.playlists.len();
        self.playlists.retain(|p| p.id != id);
        self.playlists.len() != before
    }

    pub fn export_m3u8(&self, id: &str, dest: &Path) -> Result<(), String> {
        let pl = self
            .find(id)
            .ok_or_else(|| "playlist not found".to_string())?;
        let mut out = String::from("#EXTM3U\n");
        out.push_str(&format!("#PLAYLIST:{}\n", pl.name));
        for t in &pl.tracks {
            let secs = (t.duration_ms / 1000) as i64;
            let disp = if t.artist.is_empty() {
                t.title.clone()
            } else {
                format!("{} - {}", t.artist, t.title)
            };
            out.push_str(&format!("#EXTINF:{secs},{disp}\n"));
            if let Some(ref lp) = t.local_path {
                out.push_str(lp.as_ref());
            } else {
                out.push_str(t.uri.as_ref());
            }
            out.push('\n');
        }
        write_atomic(dest, out.as_bytes(), false)
            .map_err(|e| format!("failed to write M3U8 file: {e}"))
    }
}

pub fn parse_m3u_entries(contents: &str, base_dir: Option<&Path>) -> Vec<(Option<(u32, String)>, String)> {
    let mut entries = Vec::new();
    let mut pending_info: Option<(u32, String)> = None;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#EXTINF:") {
            let (dur_s, title) = rest.split_once(',').unwrap_or(("0", rest));
            let dur_ms = dur_s.trim().parse::<i64>().unwrap_or(0).max(0) as u32 * 1000;
            pending_info = Some((dur_ms, title.trim().to_string()));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let resolved = if trimmed.contains("://")
            || trimmed.starts_with("spotify:")
            || trimmed.starts_with("yt:")
            || trimmed.starts_with("sc:")
            || trimmed.starts_with("local:")
        {
            trimmed.to_string()
        } else {
            let p = Path::new(trimmed);
            if p.is_absolute() {
                trimmed.to_string()
            } else if let Some(dir) = base_dir {
                dir.join(p).to_string_lossy().to_string()
            } else {
                trimmed.to_string()
            }
        };
        entries.push((pending_info.take(), resolved));
    }
    entries
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedSessionState {
    pub active_track: Option<TrackSummary>,
    pub position_ms: u32,
    pub manual_items: Vec<TrackSummary>,
    pub context_items: Vec<TrackSummary>,
    pub context_name: String,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

impl SavedSessionState {
    pub fn load(paths: &AppPaths) -> Option<Self> {
        let p = paths.data_dir.join("session_state.json");
        let bytes = fs::read(p).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, paths: &AppPaths) {
        let p = paths.data_dir.join("session_state.json");
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = write_atomic(&p, &bytes, false);
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListeningStatsStore {
    pub total_listen_secs: u64,
    pub total_plays: u64,
    pub source_secs: [u64; 4],
    pub track_counts: HashMap<String, (TrackSummary, u32)>,
    pub artist_counts: HashMap<String, u32>,
}

impl ListeningStatsStore {
    pub fn load(paths: &AppPaths) -> Self {
        let p = paths.data_dir.join("stats.json");
        fs::read(&p)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) {
        let p = paths.data_dir.join("stats.json");
        if let Ok(bytes) = serde_json::to_vec(self) {
            let _ = write_atomic(&p, &bytes, false);
        }
    }

    pub fn record_listen_secs(&mut self, source: MediaSource, secs: u64) {
        if secs == 0 {
            return;
        }
        self.total_listen_secs = self.total_listen_secs.saturating_add(secs);
        let idx = source_index(source);
        self.source_secs[idx] = self.source_secs[idx].saturating_add(secs);
    }

    pub fn record_track_play(&mut self, track: &TrackSummary) {
        self.total_plays = self.total_plays.saturating_add(1);
        let key = track.uri.to_string();
        let entry = self
            .track_counts
            .entry(key)
            .or_insert_with(|| (track.clone(), 0));
        entry.0 = track.clone();
        entry.1 = entry.1.saturating_add(1);
        if !track.artist.trim().is_empty() {
            let ac = self
                .artist_counts
                .entry(track.artist.to_string())
                .or_insert(0);
            *ac = ac.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> ListeningStatsSnapshot {
        let mut top_tracks: Vec<(TrackSummary, u32)> = self.track_counts.values().cloned().collect();
        top_tracks.sort_by_key(|t| std::cmp::Reverse(t.1));
        top_tracks.truncate(10);

        let mut top_artists: Vec<(String, u32)> = self
            .artist_counts
            .iter()
            .map(|(k, &v)| (k.clone(), v))
            .collect();
        top_artists.sort_by_key(|a| std::cmp::Reverse(a.1));
        top_artists.truncate(8);

        ListeningStatsSnapshot {
            total_listen_secs: self.total_listen_secs,
            total_plays: self.total_plays,
            source_secs: self.source_secs,
            top_tracks,
            top_artists,
        }
    }
}

pub fn make_local_uri_summary(path: &str, title_opt: Option<&str>, dur_ms: u32) -> TrackSummary {
    let file_name = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Local Audio");
    let title = title_opt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(file_name)
        .to_string();
    TrackSummary {
        uri: Arc::from(format!("local:{path}").as_str()),
        title,
        artist: Arc::from("Local File"),
        artist_uri: None,
        album: Arc::from(""),
        album_uri: None,
        cover: None,
        duration_ms: dur_ms,
        is_liked: false,
        is_playable: true,
        added_at_unix: None,
        source: MediaSource::Local,
        local_path: Some(Arc::from(path)),
        codec_label: None,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3u8_round_trip_preserves_multi_source_tracks() {
        let root = std::env::temp_dir().join(format!("pulse_m3u_test_{}", now_secs()));
        let paths = AppPaths::for_root(&root);
        paths.ensure_dirs().unwrap();
        let mut store = CustomPlaylistsStore::default();
        let id = store.create("Mixtape").id.clone();
        store.find_mut(&id).unwrap().tracks.push(TrackSummary {
            uri: Arc::from("yt:video:dQw4w9WgXcQ"),
            title: "Never Gonna Give You Up".into(),
            artist: Arc::from("Rick Astley"),
            artist_uri: None,
            album: Arc::from("YouTube"),
            album_uri: None,
            cover: None,
            duration_ms: 213_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::YouTube,
            local_path: None,
            codec_label: None,
        });
        let m3u_file = root.join("mixtape.m3u8");
        store.export_m3u8(&id, &m3u_file).unwrap();
        let text = fs::read_to_string(&m3u_file).unwrap();
        let parsed = parse_m3u_entries(&text, Some(&root));
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].1, "yt:video:dQw4w9WgXcQ");
        assert_eq!(parsed[0].0.as_ref().unwrap().0, 213_000);
        let _ = fs::remove_dir_all(&root);
    }
}
