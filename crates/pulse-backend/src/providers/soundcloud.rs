//! First-class SoundCloud search, URL/set resolver, metadata store, and full-length staging.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use pulse_model::{ImageRef, MediaSource, PlaylistSummary, TrackSummary};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spotatui_librespot_core::http_client::HttpClient;
use tokio::sync::Mutex as AsyncMutex;

use crate::config::{AppPaths, write_atomic};
use crate::player::{get_soundcloud_client_id, resolve_redirect_url};

const MAX_SC_AUDIO_BYTES: usize = 64 * 1024 * 1024;
static SC_TRACK_CACHE: std::sync::OnceLock<AsyncMutex<HashMap<String, CachedScTrack>>> =
    std::sync::OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedScTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub cover_url: Option<String>,
    pub duration_ms: u32,
    pub track_auth: String,
    pub transcodings: Vec<(String, bool)>,
}

fn sc_cache() -> &'static AsyncMutex<HashMap<String, CachedScTrack>> {
    SC_TRACK_CACHE.get_or_init(|| AsyncMutex::new(HashMap::new()))
}

async fn http_get_bytes(http: &HttpClient, url: &str) -> Option<Bytes> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header(header::USER_AGENT, "Mozilla/5.0 (X11; Linux x86_64) Pulse/0.2")
        .body(Bytes::new())
        .ok()?;
    if let Ok(Ok(body)) = tokio::time::timeout(Duration::from_secs(12), http.request_body(req)).await
    {
        return Some(body);
    }
    let resolved = resolve_redirect_url(http, url).await;
    if resolved == url {
        return None;
    }
    let req = Request::builder()
        .method(Method::GET)
        .uri(&resolved)
        .body(Bytes::new())
        .ok()?;
    tokio::time::timeout(Duration::from_secs(12), http.request_body(req))
        .await
        .ok()?
        .ok()
}

fn upgrade_artwork(raw: Option<&str>) -> Option<String> {
    let u = raw?.trim();
    if u.is_empty() {
        return None;
    }
    Some(u.replace("-large.", "-t500x500."))
}

fn parse_sc_track(
    item: &Value,
    liked_uris: &HashSet<String>,
) -> Option<(TrackSummary, CachedScTrack)> {
    let policy = item.get("policy").and_then(|v| v.as_str()).unwrap_or("");
    if policy == "SNIP" || policy == "BLOCK" {
        return None;
    }
    let streamable = item
        .get("streamable")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !streamable {
        return None;
    }
    let id_num = item.get("id")?.as_u64()?;
    let id = id_num.to_string();
    let title = item.get("title")?.as_str()?.trim().to_string();
    let duration_ms = item
        .get("duration")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32;
    let full_duration = item
        .get("full_duration")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::from(duration_ms));
    if duration_ms <= 32_000 && full_duration > 35_000 {
        return None;
    }

    let artist = item
        .pointer("/publisher_metadata/artist")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            item.pointer("/user/username")
                .and_then(|v| v.as_str())
                .map(str::trim)
        })
        .unwrap_or("SoundCloud Artist")
        .to_string();

    let cover_url = upgrade_artwork(item.get("artwork_url").and_then(|v| v.as_str()))
        .or_else(|| upgrade_artwork(item.pointer("/user/avatar_url").and_then(|v| v.as_str())));

    let raw_transcodings = item
        .pointer("/media/transcodings")
        .and_then(|t| t.as_array())?;

    let mut ordered = Vec::new();
    let mut push_tc = |t: &Value| {
        if t.get("snipped").and_then(|s| s.as_bool()).unwrap_or(false) {
            return;
        }
        let Some(url) = t.get("url").and_then(|u| u.as_str()) else {
            return;
        };
        let is_hls = t
            .pointer("/format/protocol")
            .and_then(|p| p.as_str())
            == Some("hls");
        if !ordered.iter().any(|(u, _)| u == url) {
            ordered.push((url.to_string(), is_hls));
        }
    };
    for t in raw_transcodings {
        if t.pointer("/format/protocol").and_then(|p| p.as_str()) == Some("progressive") {
            push_tc(t);
        }
    }
    for t in raw_transcodings {
        push_tc(t);
    }
    if ordered.is_empty() {
        return None;
    }

    let track_auth = item
        .get("track_authorization")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let uri = format!("sc:track:{id}");
    let cached = CachedScTrack {
        id: id.clone(),
        title: title.clone(),
        artist: artist.clone(),
        cover_url: cover_url.clone(),
        duration_ms,
        track_auth,
        transcodings: ordered,
    };
    let summary = TrackSummary {
        uri: Arc::from(uri.as_str()),
        title,
        artist: Arc::from(artist.as_str()),
        artist_uri: None,
        album: Arc::from("SoundCloud"),
        album_uri: None,
        cover: cover_url.map(|u| ImageRef {
            id: Arc::from(format!("sc:{id}")),
            url: Some(Arc::from(u.as_str())),
        }),
        duration_ms,
        is_liked: liked_uris.contains(&uri),
        is_playable: true,
        added_at_unix: None,
        source: MediaSource::SoundCloud,
        local_path: None,
        codec_label: Some(Arc::from("SC STREAM")),
    };
    Some((summary, cached))
}

async fn store_sc_meta(paths: Option<&AppPaths>, cached: CachedScTrack) {
    if let Some(p) = paths {
        let meta_path = p.audio_cache_dir.join(format!("sc_{}.json", cached.id));
        if let Ok(bytes) = serde_json::to_vec(&cached) {
            let _ = write_atomic(&meta_path, &bytes, false);
        }
    }
    sc_cache().lock().await.insert(cached.id.clone(), cached);
}

pub async fn search_soundcloud(
    http: &HttpClient,
    paths: &AppPaths,
    query: &str,
    liked_uris: &HashSet<String>,
) -> (Vec<TrackSummary>, Vec<PlaylistSummary>) {
    let encoded: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
    let mut cid = get_soundcloud_client_id(http, false).await;
    let mut tracks_json: Option<Value> = None;

    for attempt in 0..2 {
        let url = format!(
            "https://api-v2.soundcloud.com/search/tracks?q={encoded}&client_id={cid}&limit=25"
        );
        if let Some(raw) = http_get_bytes(http, &url).await
            && let Ok(parsed) = serde_json::from_slice::<Value>(&raw)
        {
            tracks_json = Some(parsed);
            break;
        }
        if attempt == 0 {
            cid = get_soundcloud_client_id(http, true).await;
        }
    }

    let mut tracks = Vec::new();
    if let Some(arr) = tracks_json
        .as_ref()
        .and_then(|j| j.get("collection"))
        .and_then(|c| c.as_array())
    {
        for item in arr {
            if let Some((summary, cached)) = parse_sc_track(item, liked_uris) {
                store_sc_meta(Some(paths), cached).await;
                tracks.push(summary);
            }
        }
    }

    let mut playlists = Vec::new();
    let pl_url = format!(
        "https://api-v2.soundcloud.com/search/playlists?q={encoded}&client_id={cid}&limit=8"
    );
    if let Some(raw) = http_get_bytes(http, &pl_url).await
        && let Ok(parsed) = serde_json::from_slice::<Value>(&raw)
        && let Some(arr) = parsed.get("collection").and_then(|c| c.as_array())
    {
        for item in arr {
            let Some(permalink) = item.get("permalink_url").and_then(|v| v.as_str()) else {
                continue;
            };
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("SoundCloud Set")
                .to_string();
            let owner = item
                .pointer("/user/username")
                .and_then(|v| v.as_str())
                .unwrap_or("SoundCloud");
            let count = item
                .get("track_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let cover = upgrade_artwork(item.get("artwork_url").and_then(|v| v.as_str()))
                .map(|u| ImageRef {
                    id: Arc::from(format!("scset:{permalink}")),
                    url: Some(Arc::from(u.as_str())),
                });
            playlists.push(PlaylistSummary {
                uri: Arc::from(permalink),
                name: title,
                owner: Arc::from(owner),
                track_count: count,
                total_duration_ms: 0,
                cover,
                is_collaborative: false,
                source: MediaSource::SoundCloud,
            });
        }
    }

    (tracks, playlists)
}

pub enum ScResolved {
    Track(TrackSummary),
    Set { title: String, tracks: Vec<TrackSummary> },
}

pub async fn resolve_soundcloud_url(
    http: &HttpClient,
    paths: &AppPaths,
    raw_url: &str,
    liked_uris: &HashSet<String>,
) -> Result<ScResolved, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(raw_url.trim().as_bytes()).collect();
    let mut cid = get_soundcloud_client_id(http, false).await;
    let mut resolved: Option<Value> = None;

    for attempt in 0..2 {
        let u = format!("https://api-v2.soundcloud.com/resolve?url={encoded}&client_id={cid}");
        if let Some(raw) = http_get_bytes(http, &u).await
            && let Ok(parsed) = serde_json::from_slice::<Value>(&raw)
        {
            resolved = Some(parsed);
            break;
        }
        if attempt == 0 {
            cid = get_soundcloud_client_id(http, true).await;
        }
    }

    let val = resolved.ok_or_else(|| "could not resolve SoundCloud URL".to_string())?;
    let kind = val.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if kind == "playlist" || kind == "system-playlist" {
        let title = val
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("SoundCloud Set")
            .to_string();
        let mut tracks = Vec::new();
        if let Some(arr) = val.get("tracks").and_then(|t| t.as_array()) {
            let mut missing_ids = Vec::new();
            for item in arr {
                if let Some((sum, cached)) = parse_sc_track(item, liked_uris) {
                    store_sc_meta(Some(paths), cached).await;
                    tracks.push(sum);
                } else if let Some(id) = item.get("id").and_then(|v| v.as_u64()) {
                    missing_ids.push(id.to_string());
                }
            }
            for chunk in missing_ids.chunks(25) {
                let ids_param = chunk.join(",");
                let batch_url = format!(
                    "https://api-v2.soundcloud.com/tracks?ids={ids_param}&client_id={cid}"
                );
                if let Some(raw) = http_get_bytes(http, &batch_url).await
                    && let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&raw)
                {
                    for item in items {
                        if let Some((sum, cached)) = parse_sc_track(&item, liked_uris) {
                            store_sc_meta(Some(paths), cached).await;
                            tracks.push(sum);
                        }
                    }
                }
            }
        }
        if tracks.is_empty() {
            return Err("SoundCloud set contains no streamable tracks".to_string());
        }
        return Ok(ScResolved::Set { title, tracks });
    }

    let (summary, cached) = parse_sc_track(&val, liked_uris)
        .ok_or_else(|| "SoundCloud track is preview-only or unavailable".to_string())?;
    store_sc_meta(Some(paths), cached).await;
    Ok(ScResolved::Track(summary))
}

async fn load_or_fetch_sc_meta(
    http: &HttpClient,
    paths: &AppPaths,
    track_id: &str,
) -> Result<CachedScTrack, String> {
    if let Some(hit) = sc_cache().lock().await.get(track_id).cloned() {
        return Ok(hit);
    }
    let meta_path = paths.audio_cache_dir.join(format!("sc_{track_id}.json"));
    if let Ok(bytes) = std::fs::read(&meta_path)
        && let Ok(parsed) = serde_json::from_slice::<CachedScTrack>(&bytes)
    {
        sc_cache()
            .lock()
            .await
            .insert(track_id.to_string(), parsed.clone());
        return Ok(parsed);
    }
    let mut cid = get_soundcloud_client_id(http, false).await;
    for attempt in 0..2 {
        let url = format!("https://api-v2.soundcloud.com/tracks/{track_id}?client_id={cid}");
        if let Some(raw) = http_get_bytes(http, &url).await
            && let Ok(val) = serde_json::from_slice::<Value>(&raw)
            && let Some((_, cached)) = parse_sc_track(&val, &HashSet::new())
        {
            store_sc_meta(Some(paths), cached.clone()).await;
            return Ok(cached);
        }
        if attempt == 0 {
            cid = get_soundcloud_client_id(http, true).await;
        }
    }
    Err(format!("SoundCloud metadata unavailable for {track_id}"))
}

async fn download_hls_audio(http: &HttpClient, m3u8_url: &str) -> Option<Vec<u8>> {
    let base_url = url::Url::parse(m3u8_url).ok();
    let resolve = |raw: &str| -> Option<String> {
        if raw.starts_with("http://") || raw.starts_with("https://") {
            Some(raw.to_string())
        } else {
            base_url
                .as_ref()
                .and_then(|b| b.join(raw).ok())
                .map(|u| u.to_string())
        }
    };
    let m3u8_bytes = http_get_bytes(http, m3u8_url).await?;
    let m3u8_text = String::from_utf8_lossy(&m3u8_bytes);
    let mut urls = Vec::new();
    for line in m3u8_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-KEY") && !line.contains("METHOD=NONE") {
            return None;
        }
        if line.starts_with("#EXT-X-MAP:")
            && let Some(idx) = line.find("URI=\"")
        {
            let tail = &line[idx + 5..];
            if let Some(end) = tail.find('"')
                && let Some(u) = resolve(&tail[..end])
            {
                urls.push(u);
            }
            continue;
        }
        if !line.starts_with('#')
            && let Some(u) = resolve(line)
        {
            urls.push(u);
        }
    }
    if urls.is_empty() || urls.len() > 600 {
        return None;
    }
    let mut combined = Vec::new();
    for batch in urls.chunks(10) {
        let chunks = futures_util::future::join_all(
            batch.iter().map(|u| http_get_bytes(http, u)),
        )
        .await;
        for c in chunks {
            let c = c?;
            if combined.len() + c.len() > MAX_SC_AUDIO_BYTES {
                return None;
            }
            combined.extend_from_slice(&c);
        }
    }
    Some(combined)
}

pub fn soundcloud_cache_path(paths: &AppPaths, track_id: &str) -> PathBuf {
    paths.audio_cache_dir.join(format!("sc_{track_id}.audio"))
}

pub async fn ensure_soundcloud_audio_cached(
    http: &HttpClient,
    paths: &AppPaths,
    track_id: &str,
) -> Result<(PathBuf, String), String> {
    let target = soundcloud_cache_path(paths, track_id);
    if target.is_file()
        && std::fs::metadata(&target).is_ok_and(|m| m.len() >= 16_384)
    {
        return Ok((target, "SC CACHED".to_string()));
    }

    let meta = load_or_fetch_sc_meta(http, paths, track_id).await?;
    let mut cid = get_soundcloud_client_id(http, false).await;
    let auth_enc: String =
        url::form_urlencoded::byte_serialize(meta.track_auth.as_bytes()).collect();

    for attempt in 0..2 {
        for (transcode_url, is_hls) in &meta.transcodings {
            let sep = if transcode_url.contains('?') { '&' } else { '?' };
            let resolve_url = if meta.track_auth.is_empty() {
                format!("{transcode_url}{sep}client_id={cid}")
            } else {
                format!("{transcode_url}{sep}client_id={cid}&track_authorization={auth_enc}")
            };
            let Some(stream_meta_bytes) = http_get_bytes(http, &resolve_url).await else {
                continue;
            };
            let Ok(stream_meta) = serde_json::from_slice::<Value>(&stream_meta_bytes) else {
                continue;
            };
            let Some(direct_url) = stream_meta.get("url").and_then(|u| u.as_str()) else {
                continue;
            };
            let raw_audio = if *is_hls || direct_url.contains(".m3u8") {
                let Some(bytes) = download_hls_audio(http, direct_url).await else {
                    continue;
                };
                bytes
            } else {
                let Some(b) = http_get_bytes(http, direct_url).await else {
                    continue;
                };
                if b.len() > MAX_SC_AUDIO_BYTES {
                    continue;
                }
                b.to_vec()
            };
            if raw_audio.len() >= 16_384 {
                write_atomic(&target, &raw_audio, false)
                    .map_err(|e| format!("failed to cache SoundCloud audio: {e}"))?;
                return Ok((target, "SC STREAM".to_string()));
            }
        }
        if attempt == 0 {
            cid = get_soundcloud_client_id(http, true).await;
        }
    }

    Err(format!("SoundCloud stream unavailable for {track_id}"))
}
