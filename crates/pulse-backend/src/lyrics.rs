//! Local `.lrc` / embedded tag lyrics & LRCLIB synced lyrics provider.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use pulse_model::{LyricLine, LyricsState, MediaSource, TrackSummary};
use serde_json::Value;
use sha2::{Digest, Sha256};
use spotatui_librespot_core::http_client::HttpClient;

use crate::config::{AppPaths, write_atomic};
use crate::local_lib::read_local_lyrics;

/// Parses LRC `[mm:ss.xx]` / `[mm:ss.xxx]` timestamps into sorted [`LyricLine`] entries.
pub fn parse_lrc(raw: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw_line in raw.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut cursor = line;
        let mut timestamps = Vec::new();
        while cursor.starts_with('[') {
            if let Some(close) = cursor.find(']') {
                let inside = &cursor[1..close];
                if let Some(ms) = parse_lrc_timestamp(inside) {
                    timestamps.push(ms);
                    cursor = cursor[close + 1..].trim_start();
                    continue;
                } else {
                    // Metadata tag like [ar:Artist] or [ti:Title]
                    cursor = cursor[close + 1..].trim_start();
                    if timestamps.is_empty() && cursor.is_empty() {
                        break;
                    }
                    continue;
                }
            }
            break;
        }
        let text = cursor.trim().to_string();
        if timestamps.is_empty() {
            if !text.is_empty() && !line.starts_with('[') {
                lines.push(LyricLine {
                    time_ms: None,
                    text,
                });
            }
        } else {
            for ts in timestamps {
                lines.push(LyricLine {
                    time_ms: Some(ts),
                    text: if text.is_empty() {
                        "...".to_string()
                    } else {
                        text.clone()
                    },
                });
            }
        }
    }
    lines.sort_by_key(|l| l.time_ms.unwrap_or(u32::MAX));
    lines
}

fn parse_lrc_timestamp(s: &str) -> Option<u32> {
    let (mm_str, rest) = s.split_once(':')?;
    let mins: u64 = mm_str.trim().parse().ok()?;
    let (ss_str, frac_str) = match rest.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let secs: u64 = ss_str.trim().parse().ok()?;
    let frac_ms: u64 = match frac_str {
        Some(f) => {
            let digits: String = f.chars().take(3).filter(|c| c.is_ascii_digit()).collect();
            match digits.len() {
                0 => 0,
                1 => digits.parse::<u64>().ok()? * 100,
                2 => digits.parse::<u64>().ok()? * 10,
                _ => digits.parse::<u64>().ok()?,
            }
        }
        None => 0,
    };
    Some((mins * 60_000 + secs * 1000 + frac_ms).min(u32::MAX as u64) as u32)
}

fn clean_title_for_lyrics(title: &str) -> String {
    let mut out = title.trim().to_string();
    for marker in [
        "(Official Audio)",
        "(Official Video)",
        "(Official Music Video)",
        "(Lyric Video)",
        "[Official Audio]",
        "[Official Video]",
        "(Audio)",
        "[Audio]",
    ] {
        if let Some(pos) = out.to_ascii_lowercase().find(&marker.to_ascii_lowercase()) {
            out.truncate(pos);
        }
    }
    out.trim().to_string()
}

fn lyrics_cache_path(paths: &AppPaths, uri: &str) -> PathBuf {
    let digest = Sha256::digest(uri.as_bytes());
    let mut hex = String::with_capacity(24);
    for b in &digest[..12] {
        use std::fmt::Write;
        let _ = write!(hex, "{b:02x}");
    }
    paths.cache_dir.join("lyrics").join(format!("{hex}.json"))
}

async fn http_get_json(http: &HttpClient, url: &str) -> Option<Value> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header(header::USER_AGENT, "PulsePlayer/0.2.0 (https://github.com/AIharthi/pulse-spotify)")
        .body(Bytes::new())
        .ok()?;
    let raw = tokio::time::timeout(Duration::from_secs(10), http.request_body(req))
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&raw).ok()
}

pub async fn fetch_lyrics_for_track(
    http: &HttpClient,
    paths: &AppPaths,
    track: &TrackSummary,
) -> LyricsState {
    let uri = track.uri.to_string();
    // 1. Local .lrc / embedded tag check
    if track.source == MediaSource::Local
        && let Some(ref p) = track.local_path
        && let Some((raw, is_synced)) = read_local_lyrics(Path::new(p.as_ref()))
    {
        let lines = if is_synced {
            parse_lrc(&raw)
        } else {
            raw.lines()
                .map(|l| LyricLine {
                    time_ms: None,
                    text: l.to_string(),
                })
                .collect()
        };
        if !lines.is_empty() {
            return LyricsState {
                track_uri: uri,
                synced: is_synced && lines.iter().any(|l| l.time_ms.is_some()),
                lines,
                offset_ms: 0,
                provider: "Local Tag / .lrc".to_string(),
                is_loading: false,
                error: None,
            };
        }
    }

    // 2. Disk cache check
    let cache_path = lyrics_cache_path(paths, &uri);
    if let Ok(bytes) = fs::read(&cache_path)
        && let Ok(cached) = serde_json::from_slice::<LyricsState>(&bytes)
    {
        return cached;
    }

    // 3. LRCLIB exact match then search fallback
    let clean_title = clean_title_for_lyrics(&track.title);
    let artist = track.artist.trim();
    let title_enc: String = url::form_urlencoded::byte_serialize(clean_title.as_bytes()).collect();
    let artist_enc: String = url::form_urlencoded::byte_serialize(artist.as_bytes()).collect();
    let dur_secs = track.duration_ms / 1000;

    let mut candidate: Option<Value> = None;
    if !artist.is_empty() && !clean_title.is_empty() {
        let mut get_url = format!(
            "https://lrclib.net/api/get?artist_name={artist_enc}&track_name={title_enc}"
        );
        if dur_secs > 0 {
            get_url.push_str(&format!("&duration={dur_secs}"));
        }
        if let Some(val) = http_get_json(http, &get_url).await
            && (val.get("syncedLyrics").is_some_and(|v| v.is_string())
                || val.get("plainLyrics").is_some_and(|v| v.is_string()))
        {
            candidate = Some(val);
        }
    }

    if candidate.is_none() {
        let query = if artist.is_empty() {
            clean_title.clone()
        } else {
            format!("{artist} {clean_title}")
        };
        let q_enc: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let search_url = format!("https://lrclib.net/api/search?q={q_enc}");
        if let Some(Value::Array(items)) = http_get_json(http, &search_url).await {
            candidate = items
                .iter()
                .find(|x| x.get("syncedLyrics").is_some_and(|v| v.is_string()))
                .or_else(|| {
                    items
                        .iter()
                        .find(|x| x.get("plainLyrics").is_some_and(|v| v.is_string()))
                })
                .cloned();
        }
    }

    let state = if let Some(val) = candidate {
        if let Some(synced_raw) = val.get("syncedLyrics").and_then(|v| v.as_str()) {
            let lines = parse_lrc(synced_raw);
            LyricsState {
                track_uri: uri,
                synced: true,
                lines,
                offset_ms: 0,
                provider: "LRCLIB (Synced)".to_string(),
                is_loading: false,
                error: None,
            }
        } else if let Some(plain_raw) = val.get("plainLyrics").and_then(|v| v.as_str()) {
            let lines = plain_raw
                .lines()
                .map(|l| LyricLine {
                    time_ms: None,
                    text: l.to_string(),
                })
                .collect();
            LyricsState {
                track_uri: uri,
                synced: false,
                lines,
                offset_ms: 0,
                provider: "LRCLIB (Plain)".to_string(),
                is_loading: false,
                error: None,
            }
        } else {
            LyricsState {
                track_uri: uri,
                synced: false,
                lines: Vec::new(),
                offset_ms: 0,
                provider: "LRCLIB".to_string(),
                is_loading: false,
                error: Some("No lyrics found for this track.".to_string()),
            }
        }
    } else {
        LyricsState {
            track_uri: uri,
            synced: false,
            lines: Vec::new(),
            offset_ms: 0,
            provider: "LRCLIB".to_string(),
            is_loading: false,
            error: Some("No lyrics matched on LRCLIB.".to_string()),
        }
    };

    if !state.lines.is_empty()
        && let Ok(bytes) = serde_json::to_vec(&state)
    {
        let _ = write_atomic(&cache_path, &bytes, false);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synced_lrc_timestamps_cleanly() {
        let sample = "[ti:Midnight City]\n[00:12.50]Waiting in a car\n[00:16.250]Waiting for a ride in the dark\n";
        let lines = parse_lrc(sample);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, Some(12_500));
        assert_eq!(lines[0].text, "Waiting in a car");
        assert_eq!(lines[1].time_ms, Some(16_250));
    }
}
