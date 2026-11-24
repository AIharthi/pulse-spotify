//! Spotify Connect device discovery, Cluster state parsing, and remote playback control.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use protobuf::Message;
use pulse_model::{
    AppConfig, DeviceInfo as ModelDeviceInfo, DevicesState, ImageRef, MediaSource, RepeatMode,
    TrackDetail, TrackSummary,
};
use serde_json::json;
use spotatui_librespot_core::{Session, version};
use spotatui_librespot_protocol::connect::{
    Capabilities, Cluster, Device, DeviceInfo as ProtoDeviceInfo, MemberType, PutStateReason,
    PutStateRequest,
};
use spotatui_librespot_protocol::devices::DeviceType;
use spotatui_librespot_protocol::media::AudioQuality;
use spotatui_librespot_protocol::player::{PlayerState as ProtoPlayerState, ProvidedTrack};

/// Extracted remote Connect queue metadata required for `set_queue` mutations.
#[derive(Debug, Clone, Default)]
pub struct RemoteQueueMeta {
    pub queue_revision: String,
    pub prev_tracks: Vec<serde_json::Value>,
    pub context_next_tracks: Vec<serde_json::Value>,
}

/// Extracted snapshot of what a remote Connect device is currently playing.
#[derive(Debug, Clone)]
pub struct RemotePlaybackSnapshot {
    pub active_device_id: Option<String>,
    pub active_device_volume: Option<u16>,
    pub is_playing: bool,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub active_track: Option<TrackDetail>,
    pub context_uri: Option<Arc<str>>,
    pub context_name: String,
    pub manual_queue: Vec<TrackSummary>,
    pub context_queue: Vec<TrackSummary>,
}

/// Converts a Spotify `spotify:image:<id>` URI or CDN URL into an [`ImageRef`].
pub fn image_ref_from_raw(raw: &str) -> Option<ImageRef> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(id) = trimmed.strip_prefix("spotify:image:") {
        return Some(ImageRef {
            id: Arc::from(id),
            url: Some(Arc::from(format!("https://i.scdn.co/image/{id}"))),
        });
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with("file://") {
        let id = trimmed
            .strip_prefix("https://i.scdn.co/image/")
            .or_else(|| trimmed.strip_prefix("http://i.scdn.co/image/"))
            .map(|s| s.split('?').next().unwrap_or(s))
            .unwrap_or(trimmed);
        return Some(ImageRef {
            id: Arc::from(id),
            url: Some(Arc::from(trimmed)),
        });
    }
    None
}

/// Maps protobuf `devices::DeviceType` to a clean human-readable device type label.
pub fn device_type_label(device_type: DeviceType) -> String {
    match device_type {
        DeviceType::COMPUTER | DeviceType::CHROMEBOOK => "Computer",
        DeviceType::SMARTPHONE | DeviceType::TABLET | DeviceType::SMARTWATCH => "Phone",
        DeviceType::SPEAKER
        | DeviceType::AVR
        | DeviceType::AUDIO_DONGLE
        | DeviceType::CAST_AUDIO => "Speaker",
        DeviceType::TV | DeviceType::STB | DeviceType::CAST_VIDEO | DeviceType::GAME_CONSOLE => {
            "TV"
        }
        _ => "Unknown",
    }
    .to_string()
}

/// Builds and posts a passive [`PutStateRequest`] with `PICKER_OPENED` to fetch
/// the latest Spotify Connect [`Cluster`] when local playback is NOT active.
///
/// Caller MUST NOT invoke this while local `Spirc` is active, as posting `is_active: false`
/// could clear local Connect playback state on the server.
pub async fn refresh_cluster_passive(
    session: &Session,
    device_id: &str,
    config: &AppConfig,
    current_volume: u16,
) -> Result<Cluster, String> {
    let conn_id = session.connection_id();
    if conn_id.is_empty() {
        return Err("Connect dealer connection ID not yet ready".to_string());
    }

    let mut caps = Capabilities::new();
    caps.volume_steps = 64;
    caps.disable_volume = false;
    caps.gaia_eq_connect_id = true;
    caps.can_be_player = true;
    caps.needs_full_player_state = true;
    caps.is_observable = true;
    caps.is_controllable = true;
    caps.hidden = false;
    caps.supports_gzip_pushes = true;
    caps.supports_logout = false;
    caps.supported_types = vec![
        "audio/episode".into(),
        "audio/track".into(),
        "audio/local".into(),
    ];
    caps.supports_playlist_v2 = true;
    caps.supports_transfer_command = true;
    caps.supports_command_request = true;
    caps.supports_set_options_command = true;
    caps.supported_audio_quality = AudioQuality::VERY_HIGH.into();
    caps.command_acks = true;

    let mut device_info = ProtoDeviceInfo::new();
    device_info.can_play = true;
    device_info.volume = current_volume.into();
    device_info.name = config.device_name.clone();
    device_info.device_id = device_id.to_string();
    device_info.client_id = session.client_id();
    device_info.device_type = DeviceType::COMPUTER.into();
    device_info.device_software_version = version::SEMVER.to_string();
    device_info.spirc_version = version::SPOTIFY_SPIRC_VERSION.to_string();
    device_info.capabilities = Some(caps).into();

    let mut device = Device::new();
    device.device_info = Some(device_info).into();

    let mut req = PutStateRequest::new();
    req.callback_url = format!("hm://connect-state/v1/{conn_id}");
    req.member_type = MemberType::CONNECT_STATE.into();
    req.is_active = false;
    req.put_state_reason = PutStateReason::PICKER_OPENED.into();
    req.device = Some(device).into();

    let bytes = session
        .spclient()
        .put_connect_state_request(&req)
        .await
        .map_err(|e| format!("failed to refresh Connect devices: {e}"))?;

    Cluster::parse_from_bytes(&bytes)
        .map_err(|e| format!("invalid Connect cluster protobuf: {e}"))
}

/// Builds a sorted [`DevicesState`] from a Spotify Connect [`Cluster`].
pub fn devices_from_cluster(
    cluster: &Cluster,
    local_device_id: &str,
    local_device_name: &str,
    zeroconf_active: bool,
) -> DevicesState {
    let active_device_id = if cluster.active_device_id.is_empty() {
        None
    } else {
        Some(cluster.active_device_id.clone())
    };

    let mut available_devices: Vec<ModelDeviceInfo> = cluster
        .device
        .iter()
        .filter(|(_, d)| {
            !d.is_offline && !d.capabilities.as_ref().is_some_and(|c| c.hidden)
        })
        .map(|(map_key, d)| {
            let id = if d.device_id.is_empty() {
                map_key.clone()
            } else {
                d.device_id.clone()
            };
            let is_active = active_device_id.as_deref() == Some(id.as_str());
            let is_local = id == local_device_id;
            let (supports_volume, caps_restricted) = match d.capabilities.as_ref() {
                Some(caps) => (!caps.disable_volume, !caps.is_controllable),
                None => (true, false),
            };
            let is_restricted = caps_restricted
                || !d.can_play
                || !d.disallow_playback_reasons.is_empty()
                || !d.disallow_transfer_reasons.is_empty();
            ModelDeviceInfo {
                id,
                name: d.name.clone(),
                device_type: device_type_label(d.device_type.enum_value_or_default()),
                is_active,
                is_local,
                volume: d.volume.min(65535) as u16,
                supports_volume,
                is_restricted,
            }
        })
        .collect();

    available_devices.sort_by(|a, b| {
        b.is_active
            .cmp(&a.is_active)
            .then_with(|| b.is_local.cmp(&a.is_local))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    DevicesState {
        active_device_id,
        local_device_id: local_device_id.to_string(),
        local_device_name: local_device_name.to_string(),
        zeroconf_active,
        available_devices,
    }
}

/// Parses remote player & queue state from a Connect [`Cluster`].
pub fn snapshot_from_cluster(cluster: &Cluster) -> RemotePlaybackSnapshot {
    let active_device_id = if cluster.active_device_id.is_empty() {
        None
    } else {
        Some(cluster.active_device_id.clone())
    };
    let active_device_volume = active_device_id
        .as_deref()
        .and_then(|id| cluster.device.get(id))
        .map(|d| d.volume.min(65535) as u16);

    let Some(ps) = cluster.player_state.as_ref() else {
        return RemotePlaybackSnapshot {
            active_device_id,
            active_device_volume,
            is_playing: false,
            position_ms: 0,
            duration_ms: 0,
            shuffle: false,
            repeat: RepeatMode::Off,
            active_track: None,
            context_uri: None,
            context_name: String::new(),
            manual_queue: Vec::new(),
            context_queue: Vec::new(),
        };
    };

    extract_player_state(ps, active_device_id, active_device_volume)
}

fn extract_player_state(
    ps: &ProtoPlayerState,
    active_device_id: Option<String>,
    active_device_volume: Option<u16>,
) -> RemotePlaybackSnapshot {
    let is_playing = ps.is_playing && !ps.is_paused;
    let duration_ms = ps.duration.clamp(0, u32::MAX as i64) as u32;

    let mut pos_u64 = ps.position_as_of_timestamp.max(0) as u64;
    if is_playing && ps.timestamp > 0 {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let delta = now_ms.saturating_sub(ps.timestamp).max(0) as u64;
        pos_u64 = pos_u64.saturating_add(delta);
    }
    let mut position_ms = pos_u64.min(u32::MAX as u64) as u32;
    if duration_ms > 0 {
        position_ms = position_ms.min(duration_ms);
    }

    let (shuffle, repeat) = if let Some(opts) = ps.options.as_ref() {
        let repeat = if opts.repeating_track {
            RepeatMode::Track
        } else if opts.repeating_context {
            RepeatMode::Context
        } else {
            RepeatMode::Off
        };
        (opts.shuffling_context, repeat)
    } else {
        (false, RepeatMode::Off)
    };

    let context_uri: Option<Arc<str>> = if ps.context_uri.is_empty() {
        None
    } else {
        Some(Arc::from(ps.context_uri.as_str()))
    };
    let context_name = ps
        .context_metadata
        .get("context_description")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            ps.context_metadata
                .get("title")
                .filter(|s| !s.trim().is_empty())
        })
        .cloned()
        .unwrap_or_else(|| {
            context_uri
                .as_deref()
                .map(ToOwned::to_owned)
                .unwrap_or_default()
        });

    let active_track = ps
        .track
        .as_ref()
        .filter(|t| !t.uri.is_empty() && t.provider != "delimiter")
        .map(|t| track_detail_from_provided(t, duration_ms));

    let mut manual_queue = Vec::new();
    let mut context_queue = Vec::new();
    for t in &ps.next_tracks {
        if t.uri.is_empty() || t.provider == "delimiter" || t.uri.starts_with("spotify:delimiter") {
            continue;
        }
        let summary = track_summary_from_provided(t);
        if t.provider == "queue" {
            manual_queue.push(summary);
        } else {
            context_queue.push(summary);
        }
    }

    RemotePlaybackSnapshot {
        active_device_id,
        active_device_volume,
        is_playing,
        position_ms,
        duration_ms,
        shuffle,
        repeat,
        active_track,
        context_uri,
        context_name,
        manual_queue,
        context_queue,
    }
}

fn provided_cover(t: &ProvidedTrack) -> Option<ImageRef> {
    [
        "image_xlarge_url",
        "image_large_url",
        "image_url",
        "image_small_url",
    ]
    .into_iter()
    .find_map(|key| {
        t.metadata
            .get(key)
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| image_ref_from_raw(s))
    })
}

fn track_summary_from_provided(t: &ProvidedTrack) -> TrackSummary {
    let title = t
        .metadata
        .get("title")
        .cloned()
        .unwrap_or_else(|| t.uri.clone());
    let artist: Arc<str> = Arc::from(
        t.metadata
            .get("artist_name")
            .map(String::as_str)
            .unwrap_or(""),
    );
    let album: Arc<str> = Arc::from(
        t.metadata
            .get("album_title")
            .map(String::as_str)
            .unwrap_or(""),
    );
    let duration_ms = t
        .metadata
        .get("duration")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let album_uri = if t.album_uri.is_empty() {
        t.metadata.get("album_uri").map(|s| Arc::from(s.as_str()))
    } else {
        Some(Arc::from(t.album_uri.as_str()))
    };
    let artist_uri = if t.artist_uri.is_empty() {
        t.metadata.get("artist_uri").map(|s| Arc::from(s.as_str()))
    } else {
        Some(Arc::from(t.artist_uri.as_str()))
    };

    TrackSummary {
        uri: Arc::from(t.uri.as_str()),
        title,
        artist,
        artist_uri,
        album,
        album_uri,
        cover: provided_cover(t),
        duration_ms,
        is_liked: false,
        is_playable: true,
        added_at_unix: None,
        source: MediaSource::from_uri(&t.uri),
        local_path: None,
        codec_label: None,
    }
}

fn track_detail_from_provided(t: &ProvidedTrack, duration_ms: u32) -> TrackDetail {
    let mut summary = track_summary_from_provided(t);
    if summary.duration_ms == 0 {
        summary.duration_ms = duration_ms;
    }
    let explicit = t
        .metadata
        .get("is_explicit")
        .is_some_and(|v| v == "true");

    TrackDetail {
        summary,
        track_number: None,
        disc_total_tracks: None,
        release_year: None,
        explicit,
    }
}

/// Transfers Spotify Connect playback between `from_device_id` and `to_device_id`.
pub async fn transfer_playback(
    session: &Session,
    from_device_id: &str,
    to_device_id: &str,
) -> Result<(), String> {
    session
        .spclient()
        .transfer(from_device_id, to_device_id, None)
        .await
        .map_err(|e| format!("playback transfer failed: {e}"))?;
    Ok(())
}

/// Sends a Connect player command (play/pause/skip/seek/options/queue) to `target_device_id`.
pub async fn send_remote_command(
    session: &Session,
    target_device_id: &str,
    payload: serde_json::Value,
) -> Result<(), String> {
    session
        .spclient()
        .send_player_command(target_device_id, payload)
        .await
        .map_err(|e| format!("remote Connect command failed: {e}"))?;
    Ok(())
}

/// Sends a Connect volume change (`0..=65535`) to a remote active device.
pub async fn set_remote_volume(
    session: &Session,
    target_device_id: &str,
    volume: u16,
) -> Result<(), String> {
    session
        .spclient()
        .set_remote_volume(target_device_id, volume)
        .await
        .map_err(|e| format!("remote volume update failed: {e}"))?;
    Ok(())
}

/// Builds a Connect command payload for seeking to `position_ms`.
pub fn cmd_seek_to(position_ms: u32) -> serde_json::Value {
    json!({
        "endpoint": "seek_to",
        "value": position_ms,
    })
}

/// Builds a Connect command payload for setting shuffle on a remote player.
pub fn cmd_set_shuffle(enabled: bool) -> serde_json::Value {
    json!({
        "endpoint": "set_shuffling_context",
        "value": enabled,
    })
}

/// Builds a Connect command payload for setting repeat flags on a remote player.
pub fn cmd_set_repeat(mode: RepeatMode) -> Vec<serde_json::Value> {
    let (repeat_ctx, repeat_track) = match mode {
        RepeatMode::Off => (false, false),
        RepeatMode::Context => (true, false),
        RepeatMode::Track => (false, true),
    };
    vec![
        json!({ "endpoint": "set_repeating_context", "value": repeat_ctx }),
        json!({ "endpoint": "set_repeating_track", "value": repeat_track }),
    ]
}

/// Builds a Connect command payload for appending a track URI to a remote player's queue.
pub fn cmd_add_to_queue(uri: &str) -> serde_json::Value {
    json!({
        "endpoint": "add_to_queue",
        "track": {
            "uri": uri,
        }
    })
}

/// Builds a Connect `play` command payload for starting playback of a context URI on a remote device.
pub fn cmd_play_context(
    context_uri: &str,
    track_uri: Option<&str>,
    shuffle: bool,
    repeat: RepeatMode,
) -> serde_json::Value {
    let (rep_ctx, rep_track) = match repeat {
        RepeatMode::Off => (false, false),
        RepeatMode::Context => (true, false),
        RepeatMode::Track => (false, true),
    };
    let mut options = json!({
        "player_options_override": {
            "shuffling_context": shuffle,
            "repeating_context": rep_ctx,
            "repeating_track": rep_track,
        }
    });
    if let Some(u) = track_uri {
        options["skip_to"] = json!({ "track_uri": u });
    }
    json!({
        "endpoint": "play",
        "context": {
            "uri": context_uri,
            "url": format!("context://{context_uri}"),
        },
        "play_origin": {
            "feature_identifier": "pulse-spotify"
        },
        "options": options,
    })
}

/// Builds a Connect `play` command payload for an explicit track list on a remote device.
pub fn cmd_play_tracks(
    track_uris: &[String],
    playing_track_uri: Option<&str>,
    shuffle: bool,
    repeat: RepeatMode,
) -> serde_json::Value {
    let (rep_ctx, rep_track) = match repeat {
        RepeatMode::Off => (false, false),
        RepeatMode::Context => (true, false),
        RepeatMode::Track => (false, true),
    };
    let tracks_json: Vec<serde_json::Value> = track_uris
        .iter()
        .enumerate()
        .map(|(i, u)| json!({ "uri": u, "uid": format!("t{i}") }))
        .collect();
    let mut options = json!({
        "player_options_override": {
            "shuffling_context": shuffle,
            "repeating_context": rep_ctx,
            "repeating_track": rep_track,
        }
    });
    if let Some(u) = playing_track_uri {
        options["skip_to"] = json!({ "track_uri": u });
    }
    json!({
        "endpoint": "play",
        "context": {
            "pages": [{
                "tracks": tracks_json,
            }]
        },
        "play_origin": {
            "feature_identifier": "pulse-spotify"
        },
        "options": options,
    })
}

/// Builds a Connect `set_queue` payload preserving context tracks behind manual queue items.
pub fn cmd_set_queue(
    next_tracks: Vec<serde_json::Value>,
    prev_tracks: Vec<serde_json::Value>,
    queue_revision: &str,
) -> serde_json::Value {
    json!({
        "endpoint": "set_queue",
        "next_tracks": next_tracks,
        "prev_tracks": prev_tracks,
        "queue_revision": queue_revision,
    })
}

/// Serializes a manual queue URI into a `ProvidedTrack` JSON object for Connect `set_queue`.
pub fn manual_track_to_provided_json(uri: &str, idx: usize) -> serde_json::Value {
    json!({
        "uri": uri,
        "uid": format!("q{idx}"),
        "provider": "queue",
        "metadata": {
            "is_queued": "true"
        }
    })
}

/// Extracts `prev_tracks`, context `next_tracks`, and `queue_revision` from a remote `Cluster`.
pub fn extract_remote_queue_meta(cluster: &Cluster) -> Option<RemoteQueueMeta> {
    let ps = cluster.player_state.as_ref()?;
    if ps.queue_revision.is_empty() {
        return None;
    }
    let prev_tracks = ps.prev_tracks.iter().map(provided_track_to_json).collect();
    let context_next_tracks = ps
        .next_tracks
        .iter()
        .filter(|t| t.provider != "queue")
        .map(provided_track_to_json)
        .collect();
    Some(RemoteQueueMeta {
        queue_revision: ps.queue_revision.clone(),
        prev_tracks,
        context_next_tracks,
    })
}

fn provided_track_to_json(t: &ProvidedTrack) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("uri".to_string(), json!(t.uri));
    if !t.uid.is_empty() {
        obj.insert("uid".to_string(), json!(t.uid));
    }
    if !t.provider.is_empty() {
        obj.insert("provider".to_string(), json!(t.provider));
    }
    if !t.album_uri.is_empty() {
        obj.insert("album_uri".to_string(), json!(t.album_uri));
    }
    if !t.artist_uri.is_empty() {
        obj.insert("artist_uri".to_string(), json!(t.artist_uri));
    }
    if !t.metadata.is_empty() {
        obj.insert("metadata".to_string(), json!(t.metadata));
    }
    serde_json::Value::Object(obj)
}
