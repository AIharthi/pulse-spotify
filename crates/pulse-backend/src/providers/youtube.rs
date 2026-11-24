//! Full-length YouTube InnerTube search, playlist expansion, and audio staging.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use pulse_model::{ImageRef, MediaSource, SponsorSegment, TrackSummary};
use serde_json::Value;
use sha2::{Digest, Sha256};
use spotatui_librespot_core::http_client::HttpClient;

use crate::config::{AppPaths, write_atomic};

const YT_WEB_VERSION: &str = "2.20260101.00.00";
const YT_ANDROID_VR_VERSION: &str = "1.61.48";
const YT_ANDROID_VR_UA: &str = "com.google.android.apps.youtube.vr.oculus/1.61.48 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip";
const YT_IOS_VERSION: &str = "20.10.4";
const YT_IOS_UA: &str = "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
const MAX_YT_AUDIO_BYTES: usize = 64 * 1024 * 1024;
const RANGE_CHUNK_SIZE: usize = 524_288; // 512 KiB

async fn post_json(
    http: &HttpClient,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Option<Value> {
    let raw_body = serde_json::to_vec(body).ok()?;
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = b.body(Bytes::from(raw_body)).ok()?;
    let resp = tokio::time::timeout(Duration::from_secs(12), http.request_body(req))
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&resp).ok()
}

async fn get_bytes_with_ua(http: &HttpClient, url: &str, ua: &str) -> Option<Bytes> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .header(header::USER_AGENT, ua)
        .body(Bytes::new())
        .ok()?;
    tokio::time::timeout(Duration::from_secs(15), http.request_body(req))
        .await
        .ok()?
        .ok()
}

fn extract_text(node: &Value) -> String {
    if let Some(s) = node.get("simpleText").and_then(|v| v.as_str()) {
        return s.to_string();
    }
    if let Some(runs) = node.get("runs").and_then(|v| v.as_array()) {
        return runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn parse_duration_ms(label: &str) -> Option<u32> {
    let parts: Vec<u64> = label
        .trim()
        .split(':')
        .map(|p| p.trim().parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m.checked_mul(60)?.checked_add(*s)?,
        [h, m, s] => h
            .checked_mul(3600)?
            .checked_add(m.checked_mul(60)?)?
            .checked_add(*s)?,
        _ => return None,
    };
    Some((secs * 1000).min(u32::MAX as u64) as u32)
}

fn collect_renderers<'a>(val: &'a Value, key: &str, out: &mut Vec<&'a Value>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    match val {
        Value::Object(map) => {
            if let Some(item) = map.get(key) {
                out.push(item);
            }
            for child in map.values() {
                collect_renderers(child, key, out, limit);
                if out.len() >= limit {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for child in arr {
                collect_renderers(child, key, out, limit);
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn clean_yt_channel(owner: &str) -> String {
    owner
        .strip_suffix(" - Topic")
        .unwrap_or(owner)
        .trim()
        .to_string()
}

fn track_from_video_renderer(vr: &Value, liked_uris: &HashSet<String>) -> Option<TrackSummary> {
    if vr.get("promotedSparklesWebRenderer").is_some()
        || vr.get("adBadgeRenderer").is_some()
        || vr
            .get("badges")
            .and_then(|b| b.as_array())
            .is_some_and(|arr| {
                arr.iter().any(|x| {
                    let s = x.to_string();
                    s.contains("adBadgeRenderer")
                        || s.contains("\"Ad\"")
                        || s.contains("Sponsored")
                })
            })
    {
        return None;
    }
    let vid = vr.get("videoId")?.as_str()?.trim();
    if vid.len() != 11 {
        return None;
    }
    let title = extract_text(vr.get("title")?);
    if title.is_empty() {
        return None;
    }
    let owner = vr
        .get("ownerText")
        .or_else(|| vr.get("shortBylineText"))
        .or_else(|| vr.get("longBylineText"))
        .map(extract_text)
        .map(|s| clean_yt_channel(&s))
        .unwrap_or_else(|| "YouTube".to_string());
    let duration_ms = vr
        .get("lengthText")
        .map(extract_text)
        .and_then(|s| parse_duration_ms(&s))
        .unwrap_or(0);
    // Skip live streams without finite duration
    if duration_ms == 0
        && vr
            .get("badges")
            .and_then(|b| b.as_array())
            .is_some_and(|arr| arr.iter().any(|x| x.to_string().contains("LIVE")))
    {
        return None;
    }
    let uri = format!("yt:video:{vid}");
    let thumb_url = format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg");
    Some(TrackSummary {
        uri: Arc::from(uri.as_str()),
        title,
        artist: Arc::from(owner.as_str()),
        artist_uri: None,
        album: Arc::from("YouTube"),
        album_uri: None,
        cover: Some(ImageRef {
            id: Arc::from(format!("yt:{vid}")),
            url: Some(Arc::from(thumb_url.as_str())),
        }),
        duration_ms,
        is_liked: liked_uris.contains(&uri),
        is_playable: true,
        added_at_unix: None,
        source: MediaSource::YouTube,
        local_path: None,
        codec_label: Some(Arc::from("YT AUDIO")),
    })
}

pub async fn search_youtube(
    http: &HttpClient,
    query: &str,
    liked_uris: &HashSet<String>,
) -> Vec<TrackSummary> {
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": YT_WEB_VERSION,
                "hl": "en",
                "gl": "US"
            }
        },
        "query": query.trim(),
    });
    let Some(json) = post_json(
        http,
        "https://youtubei.googleapis.com/youtubei/v1/search?prettyPrint=false",
        &[],
        &body,
    )
    .await
    else {
        return Vec::new();
    };
    let mut renderers = Vec::new();
    collect_renderers(&json, "videoRenderer", &mut renderers, 30);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for vr in renderers {
        if let Some(t) = track_from_video_renderer(vr, liked_uris)
            && seen.insert(t.uri.to_string())
        {
            out.push(t);
            if out.len() >= 25 {
                break;
            }
        }
    }
    out
}

pub async fn resolve_youtube_video(
    http: &HttpClient,
    video_id: &str,
    liked_uris: &HashSet<String>,
) -> Result<TrackSummary, String> {
    let clients = [
        (
            serde_json::json!({
                "clientName": "ANDROID_VR",
                "clientVersion": YT_ANDROID_VR_VERSION,
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": 32,
                "osName": "Android",
                "osVersion": "12L",
                "hl": "en",
                "gl": "US"
            }),
            YT_ANDROID_VR_UA,
            "28",
            YT_ANDROID_VR_VERSION,
        ),
        (
            serde_json::json!({
                "clientName": "IOS",
                "clientVersion": YT_IOS_VERSION,
                "deviceMake": "Apple",
                "deviceModel": "iPhone16,2",
                "osName": "iOS",
                "osVersion": "18.3.2.22D82",
                "hl": "en",
                "gl": "US"
            }),
            YT_IOS_UA,
            "5",
            YT_IOS_VERSION,
        ),
    ];

    let mut json_opt = None;
    for (client_obj, ua, cnum, cver) in clients {
        let body = serde_json::json!({
            "context": { "client": client_obj },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        if let Some(j) = post_json(
            http,
            "https://youtubei.googleapis.com/youtubei/v1/player?prettyPrint=false",
            &[
                ("User-Agent", ua),
                ("X-YouTube-Client-Name", cnum),
                ("X-YouTube-Client-Version", cver),
            ],
            &body,
        )
        .await
            && j.get("videoDetails").is_some()
        {
            json_opt = Some(j);
            break;
        }
    }

    let json = match json_opt {
        Some(j) => j,
        None => {
            // Public oEmbed fallback when InnerTube player throttles or omits metadata
            let oembed_url = format!(
                "https://www.youtube.com/oembed?url=https://www.youtube.com/watch?v={video_id}&format=json"
            );
            let bytes = get_bytes_with_ua(http, &oembed_url, "Mozilla/5.0")
                .await
                .ok_or_else(|| "failed to contact YouTube metadata endpoint".to_string())?;
            let meta: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid YouTube metadata: {e}"))?;
            let title = meta
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(video_id)
                .to_string();
            let author = clean_yt_channel(
                meta.get("author_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("YouTube"),
            );
            let uri = format!("yt:video:{video_id}");
            return Ok(TrackSummary {
                uri: Arc::from(uri.as_str()),
                title,
                artist: Arc::from(author.as_str()),
                artist_uri: None,
                album: Arc::from("YouTube"),
                album_uri: None,
                cover: Some(ImageRef {
                    id: Arc::from(format!("yt:{video_id}")),
                    url: Some(Arc::from(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))),
                }),
                duration_ms: 0,
                is_liked: liked_uris.contains(&uri),
                is_playable: true,
                added_at_unix: None,
                source: MediaSource::YouTube,
                local_path: None,
                codec_label: Some(Arc::from("YT AUDIO")),
            });
        }
    };

    let details = json
        .get("videoDetails")
        .ok_or_else(|| "YouTube video details missing".to_string())?;
    let title = details
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(video_id)
        .to_string();
    let author = clean_yt_channel(
        details
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("YouTube"),
    );
    let secs = details
        .get("lengthSeconds")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let uri = format!("yt:video:{video_id}");
    Ok(TrackSummary {
        uri: Arc::from(uri.as_str()),
        title,
        artist: Arc::from(author.as_str()),
        artist_uri: None,
        album: Arc::from("YouTube"),
        album_uri: None,
        cover: Some(ImageRef {
            id: Arc::from(format!("yt:{video_id}")),
            url: Some(Arc::from(format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg"))),
        }),
        duration_ms: (secs * 1000).min(u32::MAX as u64) as u32,
        is_liked: liked_uris.contains(&uri),
        is_playable: true,
        added_at_unix: None,
        source: MediaSource::YouTube,
        local_path: None,
        codec_label: Some(Arc::from("YT AUDIO")),
    })
}

pub async fn resolve_youtube_playlist(
    http: &HttpClient,
    playlist_id: &str,
    liked_uris: &HashSet<String>,
) -> Result<(String, Vec<TrackSummary>), String> {
    let browse_id = if playlist_id.starts_with("VL") {
        playlist_id.to_string()
    } else {
        format!("VL{playlist_id}")
    };
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": YT_WEB_VERSION,
                "hl": "en",
                "gl": "US"
            }
        },
        "browseId": browse_id
    });
    let json = post_json(
        http,
        "https://youtubei.googleapis.com/youtubei/v1/browse?prettyPrint=false",
        &[],
        &body,
    )
    .await
    .ok_or_else(|| "failed to load YouTube playlist".to_string())?;

    let title = json
        .pointer("/header/playlistHeaderRenderer/title")
        .or_else(|| json.pointer("/metadata/playlistMetadataRenderer/title"))
        .map(extract_text)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            json.pointer("/metadata/playlistMetadataRenderer/title")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "YouTube Playlist".to_string());

    let mut renderers = Vec::new();
    collect_renderers(&json, "playlistVideoRenderer", &mut renderers, 100);
    let mut tracks = Vec::new();
    let mut seen = HashSet::new();
    for pvr in renderers {
        if let Some(t) = track_from_video_renderer(pvr, liked_uris)
            && seen.insert(t.uri.to_string())
        {
            tracks.push(t);
        }
    }
    if tracks.is_empty() {
        return Err("YouTube playlist returned no playable items".to_string());
    }
    Ok((title, tracks))
}

/// Fetches full-length audio bytes from YouTube (`ANDROID_VR` primary, `IOS` fallback).
/// Returns `(bytes, is_complete, codec_label)`.
pub async fn download_youtube_audio_bytes(
    http: &HttpClient,
    video_id: &str,
) -> Result<(Vec<u8>, bool, String), String> {
    let clients = [
        (
            serde_json::json!({
                "clientName": "ANDROID_VR",
                "clientVersion": YT_ANDROID_VR_VERSION,
                "deviceMake": "Oculus",
                "deviceModel": "Quest 3",
                "androidSdkVersion": 32,
                "osName": "Android",
                "osVersion": "12L",
                "hl": "en",
                "gl": "US"
            }),
            YT_ANDROID_VR_UA,
            "28",
            YT_ANDROID_VR_VERSION,
        ),
        (
            serde_json::json!({
                "clientName": "IOS",
                "clientVersion": YT_IOS_VERSION,
                "deviceMake": "Apple",
                "deviceModel": "iPhone16,2",
                "osName": "iOS",
                "osVersion": "18.3.2.22D82",
                "hl": "en",
                "gl": "US"
            }),
            YT_IOS_UA,
            "5",
            YT_IOS_VERSION,
        ),
    ];

    for (client_obj, ua, client_num, client_ver) in clients {
        let player_body = serde_json::json!({
            "context": { "client": client_obj },
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        let Some(player_json) = post_json(
            http,
            "https://youtubei.googleapis.com/youtubei/v1/player?prettyPrint=false",
            &[
                ("User-Agent", ua),
                ("X-YouTube-Client-Name", client_num),
                ("X-YouTube-Client-Version", client_ver),
            ],
            &player_body,
        )
        .await
        else {
            continue;
        };

        if player_json
            .pointer("/playabilityStatus/status")
            .and_then(|s| s.as_str())
            != Some("OK")
        {
            continue;
        }
        if let Some(resp_vid) = player_json
            .pointer("/videoDetails/videoId")
            .and_then(|v| v.as_str())
            && resp_vid != video_id
        {
            continue;
        }
        let Some(formats) = player_json
            .pointer("/streamingData/adaptiveFormats")
            .and_then(|f| f.as_array())
        else {
            continue;
        };

        let chosen = formats
            .iter()
            .find(|f| f.get("itag").and_then(|i| i.as_i64()) == Some(251) && f.get("url").is_some())
            .or_else(|| {
                formats.iter().find(|f| {
                    f.get("itag").and_then(|i| i.as_i64()) == Some(140) && f.get("url").is_some()
                })
            })
            .or_else(|| {
                formats.iter().find(|f| {
                    f.get("mimeType")
                        .and_then(|m| m.as_str())
                        .is_some_and(|m| m.starts_with("audio/"))
                        && f.get("url").is_some()
                })
            });

        let Some(fmt) = chosen else {
            continue;
        };
        let Some(base_url) = fmt.get("url").and_then(|u| u.as_str()) else {
            continue;
        };
        let expected_len = fmt
            .get("contentLength")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        if expected_len > MAX_YT_AUDIO_BYTES {
            continue;
        }
        let mime = fmt.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
        let codec_label = if mime.contains("opus") {
            "YT OPUS"
        } else {
            "YT AAC"
        }
        .to_string();

        let total_target = if expected_len > 0 {
            expected_len
        } else {
            16 * 1024 * 1024
        };
        let mut ranges = Vec::new();
        let mut off = 0usize;
        while off < total_target && off < MAX_YT_AUDIO_BYTES {
            let end = (off + RANGE_CHUNK_SIZE - 1).min(total_target.saturating_sub(1));
            ranges.push((off, end));
            off += RANGE_CHUNK_SIZE;
        }

        let sep = if base_url.contains('?') { '&' } else { '?' };
        let mut raw = Vec::with_capacity(expected_len.min(MAX_YT_AUDIO_BYTES));
        let mut aborted = false;

        for batch in ranges.chunks(4) {
            let futs = batch.iter().map(|&(s, e)| {
                let u = format!("{base_url}{sep}range={s}-{e}");
                async move { get_bytes_with_ua(http, &u, ua).await }
            });
            let results = futures_util::future::join_all(futs).await;
            for ((s, e), chunk_opt) in batch.iter().zip(results) {
                let Some(chunk) = chunk_opt else {
                    aborted = true;
                    break;
                };
                if chunk.is_empty() {
                    aborted = true;
                    break;
                }
                raw.extend_from_slice(&chunk);
                let expected_chunk = e - s + 1;
                if chunk.len() < expected_chunk {
                    aborted = expected_len > 0 && raw.len() < expected_len;
                    break;
                }
            }
            if aborted || (expected_len > 0 && raw.len() >= expected_len) {
                break;
            }
        }

        if raw.len() >= 32_768 {
            let is_complete = expected_len == 0 || raw.len() * 100 / expected_len >= 95;
            return Ok((raw, is_complete, codec_label));
        }
    }

    Err(format!("YouTube stream unavailable for {video_id}"))
}

pub fn youtube_cache_path(paths: &AppPaths, video_id: &str) -> PathBuf {
    paths.audio_cache_dir.join(format!("yt_{video_id}.audio"))
}

pub async fn ensure_youtube_audio_cached(
    http: &HttpClient,
    paths: &AppPaths,
    video_id: &str,
) -> Result<(PathBuf, String), String> {
    let target = youtube_cache_path(paths, video_id);
    if target.is_file()
        && std::fs::metadata(&target).is_ok_and(|m| m.len() >= 32_768)
    {
        return Ok((target, "YT CACHED".to_string()));
    }
    let (bytes, _complete, label) = download_youtube_audio_bytes(http, video_id).await?;
    write_atomic(&target, &bytes, false)
        .map_err(|e| format!("failed to cache YouTube audio: {e}"))?;
    Ok((target, label))
}

/// Fetches the YouTube thumbnail JPEG bytes best-effort for ID3 cover embedding.
pub async fn fetch_youtube_cover_jpeg(http: &HttpClient, video_id: &str) -> Option<Vec<u8>> {
    let url = format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg");
    let req = Request::builder()
        .method(Method::GET)
        .uri(&url)
        .header(header::USER_AGENT, "Aplayer/0.3")
        .body(Bytes::new())
        .ok()?;
    let bytes = tokio::time::timeout(Duration::from_secs(6), http.request_body(req))
        .await
        .ok()?
        .ok()?;
    if bytes.len() >= 128 {
        Some(bytes.to_vec())
    } else {
        None
    }
}

/// Parses and merges overlapping/adjacent SponsorBlock skip intervals for `video_id`.
pub fn parse_sponsorblock_payload(raw: &[u8], video_id: &str) -> Vec<SponsorSegment> {
    let Ok(Value::Array(videos)) = serde_json::from_slice::<Value>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in videos {
        if entry.get("videoID").and_then(|v| v.as_str()) != Some(video_id) {
            continue;
        }
        let Some(segs) = entry.get("segments").and_then(|v| v.as_array()) else {
            continue;
        };
        for seg in segs {
            let action = seg
                .get("actionType")
                .and_then(|v| v.as_str())
                .unwrap_or("skip");
            if action != "skip" {
                continue;
            }
            if seg.get("votes").and_then(|v| v.as_i64()).unwrap_or(0) < 0 {
                continue;
            }
            let Some(pair) = seg.get("segment").and_then(|v| v.as_array()) else {
                continue;
            };
            if pair.len() < 2 {
                continue;
            }
            let Some(start_s) = pair[0].as_f64() else {
                continue;
            };
            let Some(end_s) = pair[1].as_f64() else {
                continue;
            };
            if !(start_s.is_finite() && end_s.is_finite()) || end_s <= start_s + 0.5 {
                continue;
            }
            let start_ms = (start_s.max(0.0) * 1000.0)
                .min(u32::MAX as f64)
                .round() as u32;
            let end_ms = (end_s.max(0.0) * 1000.0)
                .min(u32::MAX as f64)
                .round() as u32;
            if end_ms > start_ms.saturating_add(500) {
                let category = seg
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("sponsor")
                    .to_string();
                out.push(SponsorSegment {
                    start_ms,
                    end_ms,
                    category,
                });
            }
        }
    }
    out.sort_by_key(|s| (s.start_ms, s.end_ms));
    let mut merged: Vec<SponsorSegment> = Vec::with_capacity(out.len());
    for seg in out {
        if let Some(last) = merged.last_mut()
            && seg.start_ms <= last.end_ms.saturating_add(250)
        {
            if seg.end_ms > last.end_ms {
                last.end_ms = seg.end_ms;
            }
            continue;
        }
        merged.push(seg);
    }
    merged
}

/// Queries SponsorBlock using k-anonymous 4-char SHA-256 prefix lookup (`sponsor.ajay.app`).
pub async fn fetch_youtube_skip_segments(
    http: &HttpClient,
    video_id: &str,
) -> Vec<SponsorSegment> {
    if video_id.len() != 11 {
        return Vec::new();
    }
    let digest = Sha256::digest(video_id.as_bytes());
    let prefix = format!("{:02x}{:02x}", digest[0], digest[1]);
    let url = format!(
        "https://sponsor.ajay.app/api/skipSegments/{prefix}?categories=%5B%22sponsor%22,%22selfpromo%22,%22interaction%22,%22music_offtopic%22%5D"
    );
    let Ok(req) = Request::builder()
        .method(Method::GET)
        .uri(&url)
        .header(header::USER_AGENT, "Aplayer/0.3")
        .body(Bytes::new())
    else {
        return Vec::new();
    };
    let Ok(Ok(body)) = tokio::time::timeout(Duration::from_secs(5), http.request_body(req)).await
    else {
        return Vec::new();
    };
    parse_sponsorblock_payload(&body, video_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_merge_sponsorblock_payload() {
        let json = br#"[
            {
                "videoID": "otherVideo1",
                "segments": [{"category": "sponsor", "actionType": "skip", "votes": 5, "segment": [0.0, 20.0]}]
            },
            {
                "videoID": "dQw4w9WgXcQ",
                "segments": [
                    {"category": "music_offtopic", "actionType": "skip", "votes": 3, "segment": [0.0, 12.2]},
                    {"category": "sponsor", "actionType": "skip", "votes": 10, "segment": [12.3, 25.5]},
                    {"category": "selfpromo", "actionType": "skip", "votes": -2, "segment": [30.0, 40.0]},
                    {"category": "poi_highlight", "actionType": "poi", "votes": 5, "segment": [45.0, 45.0]},
                    {"category": "outro", "actionType": "skip", "votes": 2, "segment": [180.0, 195.0]}
                ]
            }
        ]"#;
        let segs = parse_sponsorblock_payload(json, "dQw4w9WgXcQ");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].start_ms, 0);
        assert_eq!(segs[0].end_ms, 25_500);
        assert_eq!(segs[1].start_ms, 180_000);
        assert_eq!(segs[1].end_ms, 195_000);
    }
}
