//! Local librespot Player (`pulseaudio` / `alsa` lazy sink + `SoftMixer`) and `Spirc` wiring.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::{Method, Request, header};
use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use pulse_model::{AppConfig, NormalisationMode, RepeatMode, StreamQuality};
use serde_json::Value;
use spotatui_librespot_connect::{
    ConnectConfig, LoadContextOptions, LoadRequest, LoadRequestOptions, Options, PlayingTrack,
    Spirc,
};
use spotatui_librespot_core::authentication::Credentials;
use spotatui_librespot_core::config::DeviceType;
use spotatui_librespot_core::dealer::manager::BoxedStreamResult;
use spotatui_librespot_core::dealer::protocol::Message as DealerMessage;
use spotatui_librespot_core::http_client::HttpClient;
use spotatui_librespot_core::{FileId, Session, SpotifyUri};
use spotatui_librespot_metadata::audio::AudioFileFormat;
use spotatui_librespot_metadata::{Metadata, Track};
use spotatui_librespot_playback::audio_backend::{self, Sink, SinkError, SinkResult};
use spotatui_librespot_playback::config::{
    AudioFormat, Bitrate, NormalisationMethod, NormalisationType, PlayerConfig,
};
use spotatui_librespot_playback::convert::Converter;
use spotatui_librespot_playback::decoder::AudioPacket;
use spotatui_librespot_playback::mixer::{Mixer, MixerConfig, softmixer::SoftMixer};
use spotatui_librespot_playback::player::Player;
use spotatui_librespot_protocol::connect::ClusterUpdate;
use tokio::sync::{Mutex as AsyncMutex, watch};

static PULSE_ENV_INIT: Once = Once::new();
static AUDIO_KEY_REJECTED: AtomicBool = AtomicBool::new(false);
static STAGED_TRACKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static SOUNDCLOUD_CLIENT_ID: OnceLock<AsyncMutex<Option<String>>> = OnceLock::new();
static IN_FLIGHT_STAGING: OnceLock<Mutex<HashMap<String, watch::Sender<bool>>>> = OnceLock::new();

const SPOTIFY_OGG_HEADER_SIZE: usize = 167;
// Bumped to PLS4 so truncated PLS3 clips (~61s YouTube) purge cleanly and upgrade to full tracks.
const FALLBACK_HEADER_MAGIC: &[u8; 4] = b"PLS4";
// Temporary magic for truncated/partial fallback clips: playable during this run, refreshed on restart.
const TEMP_FALLBACK_MAGIC: &[u8; 4] = b"PLST";
const DEFAULT_SOUNDCLOUD_CLIENT_ID: &str = "qLcgHhXgeEFFaSolTB9I71LBsqncsD93";
const MAX_DOWNLOAD_BYTES: usize = 16 * 1024 * 1024;
const MAX_FFMPEG_OUTPUT_BYTES: u64 = 24 * 1024 * 1024;
const YOUTUBE_WEB_CLIENT_VERSION: &str = "2.20260101.00.00";
const YOUTUBE_IOS_CLIENT_VERSION: &str = "20.10.4";

/// Stream of passive `ClusterUpdate` messages from the Spotify Dealer websocket.
pub type ClusterStream = BoxedStreamResult<ClusterUpdate>;

/// Active local audio output + Spotify Connect `Spirc` controller.
pub struct LocalPlayback {
    pub player: Arc<Player>,
    pub spirc: Spirc,
    pub mixer: Arc<SoftMixer>,
    pub sink_flush: Arc<AtomicBool>,
    pub spirc_task: tokio::task::JoinHandle<()>,
    pub cluster_stream: Option<ClusterStream>,
}

impl LocalPlayback {
    /// Requests the active audio sink to flush any buffered audio on the next write.
    pub fn request_sink_flush(&self) {
        self.sink_flush.store(true, Ordering::Release);
    }

    /// Cleanly shuts down `Spirc`, stops the local `Player`, and aborts the background task.
    pub fn shutdown(self) {
        let _ = self.spirc.shutdown();
        self.player.stop();
        self.spirc_task.abort();
    }
}

struct LowLatencyPulseSink {
    sink: Option<Simple>,
    device: Option<String>,
    flush_requested: Arc<AtomicBool>,
    pcm_bytes: Vec<u8>,
}

impl LowLatencyPulseSink {
    fn new(device: Option<String>, flush_requested: Arc<AtomicBool>) -> Self {
        Self {
            sink: None,
            device,
            flush_requested,
            pcm_bytes: Vec::with_capacity(8192),
        }
    }
}

impl Sink for LowLatencyPulseSink {
    fn start(&mut self) -> SinkResult<()> {
        if self.sink.is_none() {
            let sample_spec = pulse::sample::Spec {
                format: pulse::sample::Format::S16NE,
                channels: 2,
                rate: 44_100,
            };
            if !sample_spec.is_valid() {
                return Err(SinkError::InvalidParams(
                    "invalid PulseAudio sample specification".to_string(),
                ));
            }
            let attr = pulse::def::BufferAttr {
                maxlength: 35_280, // ~200 ms max buffer (44100 * 2ch * 2B = 176400 B/s)
                tlength: 14_112,   // ~80 ms target playback buffer
                prebuf: 3_528,     // ~20 ms prebuffer for near-instant start
                minreq: 3_528,     // ~20 ms minimum request
                fragsize: u32::MAX,
            };
            let sink = Simple::new(
                None,
                "Pulse",
                pulse::stream::Direction::Playback,
                self.device.as_deref(),
                "Pulse Spotify Player",
                &sample_spec,
                None,
                Some(&attr),
            )
            .map_err(|e| SinkError::ConnectionRefused(format!("<LowLatencyPulseSink> {e}")))?;
            self.sink = Some(sink);
        }
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.flush_requested.store(false, Ordering::Release);
        if let Some(sink) = self.sink.take() {
            // Flush immediately instead of draining ~seconds of buffered audio, and never
            // return Err from stop() so `PlayerInternal::ensure_sink_stopped` cannot exit(1).
            let _ = sink.flush();
        }
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if self.sink.is_none() {
            self.start()?;
        }
        if self.flush_requested.swap(false, Ordering::AcqRel)
            && let Some(ref sink) = self.sink
        {
            let _ = sink.flush();
        }
        let sink = self
            .sink
            .as_ref()
            .ok_or_else(|| SinkError::NotConnected("<LowLatencyPulseSink>".to_string()))?;

        match packet {
            AudioPacket::Samples(samples) => {
                let samples_s16 = converter.f64_to_s16(&samples);
                self.pcm_bytes.clear();
                self.pcm_bytes.reserve(samples_s16.len().saturating_mul(2));
                for sample in samples_s16 {
                    self.pcm_bytes.extend_from_slice(&sample.to_ne_bytes());
                }
                sink.write(&self.pcm_bytes)
                    .map_err(|e| SinkError::OnWrite(format!("<LowLatencyPulseSink> {e}")))?;
            }
            AudioPacket::Raw(raw) => {
                sink.write(&raw)
                    .map_err(|e| SinkError::OnWrite(format!("<LowLatencyPulseSink> {e}")))?;
            }
        }
        Ok(())
    }
}

/// Converts [`AppConfig`] quality/normalisation/gapless settings into librespot [`PlayerConfig`].
pub fn build_player_config(config: &AppConfig) -> PlayerConfig {
    let bitrate = match config.stream_quality {
        StreamQuality::Kbps96 => Bitrate::Bitrate96,
        StreamQuality::Kbps160 => Bitrate::Bitrate160,
        StreamQuality::Kbps320 => Bitrate::Bitrate320,
    };

    let (normalisation, normalisation_type) = match config.normalisation {
        NormalisationMode::Off => (false, NormalisationType::Album),
        NormalisationMode::Track => (true, NormalisationType::Track),
        NormalisationMode::Album => (true, NormalisationType::Album),
    };

    PlayerConfig {
        bitrate,
        gapless: config.gapless,
        normalisation,
        normalisation_type,
        normalisation_method: NormalisationMethod::Dynamic,
        position_update_interval: Some(Duration::from_millis(250)),
        ..Default::default()
    }
}

/// Returns `(audio_badge, sample_rate_hz, bitrate_kbps)` corresponding to `config`.
pub fn stream_audio_metrics(config: &AppConfig) -> (String, u32, u16) {
    (
        config.stream_quality.badge_label().to_string(),
        44_100,
        config.stream_quality.bitrate_kbps(),
    )
}

/// Returns `true` if `Player` must be rebuilt to apply audio pipeline settings.
pub fn audio_pipeline_changed(old: &AppConfig, new: &AppConfig) -> bool {
    old.audio_device != new.audio_device
        || old.stream_quality != new.stream_quality
        || old.normalisation != new.normalisation
        || old.gapless != new.gapless
}

fn configure_pulse_env_once() {
    PULSE_ENV_INIT.call_once(|| {
        // SAFETY: Called once during backend startup before creating `PulseAudioSink` / `Player`.
        unsafe {
            if std::env::var_os("PULSE_PROP_application.name").is_none() {
                std::env::set_var("PULSE_PROP_application.name", "Pulse");
            }
            if std::env::var_os("PULSE_PROP_stream.description").is_none() {
                std::env::set_var("PULSE_PROP_stream.description", "Pulse Spotify Player");
            }
        }
    });
}

/// Ensures ALSA's default PCM routes via PipeWire when system-wide `/etc/alsa/conf.d` is absent.
pub fn ensure_pipewire_alsa_user_config() {
    #[cfg(unix)]
    {
        let pw_50 = Path::new("/usr/share/alsa/alsa.conf.d/50-pipewire.conf");
        let pw_99 = Path::new("/usr/share/alsa/alsa.conf.d/99-pipewire-default.conf");
        if !pw_50.exists() || !pw_99.exists() {
            return;
        }
        if Path::new("/etc/alsa/conf.d").exists() {
            return;
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
            && home.join(".asoundrc").exists()
        {
            return;
        }
        let Some(cfg_dir) = dirs::config_dir() else {
            return;
        };
        let asoundrc = cfg_dir.join("alsa/asoundrc");
        if asoundrc.exists() {
            return;
        }
        if let Some(parent) = asoundrc.parent()
            && std::fs::create_dir_all(parent).is_ok()
        {
            let content = concat!(
                "</usr/share/alsa/alsa.conf.d/50-pipewire.conf>\n",
                "</usr/share/alsa/alsa.conf.d/99-pipewire-default.conf>\n",
            );
            let _ = std::fs::write(&asoundrc, content);
        }
    }
}

fn pulse_socket_available() -> bool {
    if std::env::var_os("PULSE_SERVER").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("pulse/native").exists())
        .unwrap_or(false)
}

enum ResolvedSink {
    Pulse(Option<String>),
    Alsa(audio_backend::SinkBuilder, Option<String>),
}

fn resolve_sink_and_device(requested_device: Option<&str>) -> Result<ResolvedSink, String> {
    let wants_pulse_or_default = matches!(
        requested_device,
        None | Some("pulseaudio") | Some("pipewire") | Some("pulse")
    );
    if wants_pulse_or_pulse_available(wants_pulse_or_default) {
        return Ok(ResolvedSink::Pulse(None));
    }

    let alsa_builder = audio_backend::find(Some("alsa".to_string()))
        .ok_or_else(|| "librespot audio backends (pulseaudio/alsa) are unavailable".to_string())?;
    let alsa_device = match requested_device {
        Some("pulseaudio") => None,
        other => other.map(ToOwned::to_owned),
    };
    Ok(ResolvedSink::Alsa(alsa_builder, alsa_device))
}

fn wants_pulse_or_pulse_available(wants_pulse: bool) -> bool {
    wants_pulse && pulse_socket_available()
}

/// Creates a lazy PipeWire/PulseAudio or ALSA [`Player`], `SoftMixer`, and [`Spirc`] task.
///
/// `session` MUST be a fresh, unconnected `Session` because [`Spirc::new`] registers dealer
/// listeners on its builder and calls `session.connect(creds, true).await` once internally.
pub async fn start_local_playback(
    session: &Session,
    creds: Credentials,
    config: &AppConfig,
    initial_volume: u16,
) -> Result<LocalPlayback, String> {
    AUDIO_KEY_REJECTED.store(false, Ordering::Relaxed);
    configure_pulse_env_once();
    ensure_pipewire_alsa_user_config();

    let resolved_sink = resolve_sink_and_device(config.audio_device.as_deref())?;
    let sink_flush = Arc::new(AtomicBool::new(false));
    let sink_flush_for_closure = Arc::clone(&sink_flush);

    let mixer = Arc::new(
        SoftMixer::open(MixerConfig::default())
            .map_err(|e| format!("failed to open SoftMixer: {e}"))?,
    );
    mixer.set_volume(initial_volume);

    let player_config = build_player_config(config);

    let player = Player::new(
        player_config,
        session.clone(),
        mixer.get_soft_volume(),
        move || match &resolved_sink {
            ResolvedSink::Pulse(dev) => Box::new(LowLatencyPulseSink::new(
                dev.clone(),
                Arc::clone(&sink_flush_for_closure),
            )),
            ResolvedSink::Alsa(builder, dev) => builder(dev.clone(), AudioFormat::S16),
        },
    );

    let connect_cfg = ConnectConfig {
        name: config.device_name.clone(),
        device_type: DeviceType::Computer,
        initial_volume,
        emit_set_queue_events: true,
        ..Default::default()
    };

    let cluster_stream = session
        .dealer()
        .listen_for(
            "hm://connect-state/v1/cluster",
            DealerMessage::from_raw::<ClusterUpdate>,
        )
        .ok();

    let (spirc, spirc_future) = Spirc::new(
        connect_cfg,
        session.clone(),
        creds,
        player.clone(),
        mixer.clone(),
    )
    .await
    .map_err(|e| format!("failed to initialize Spirc Connect device: {e}"))?;

    let spirc_task = tokio::task::spawn_local(async move {
        let _saved = spirc_future.await;
    });

    Ok(LocalPlayback {
        player,
        spirc,
        mixer,
        sink_flush,
        spirc_task,
        cluster_stream,
    })
}

fn collect_head_file_ids(track: &Track) -> Vec<FileId> {
    let mut fids = Vec::with_capacity(3);
    for fmt in [
        AudioFileFormat::OGG_VORBIS_96,
        AudioFileFormat::OGG_VORBIS_160,
        AudioFileFormat::OGG_VORBIS_320,
    ] {
        if let Some(fid) = track.files.get(&fmt) {
            fids.push(*fid);
        }
    }
    fids
}

fn in_flight_staging() -> &'static Mutex<HashMap<String, watch::Sender<bool>>> {
    IN_FLIGHT_STAGING.get_or_init(|| Mutex::new(HashMap::new()))
}

struct InFlightStageGuard {
    uri: String,
    tx: Option<watch::Sender<bool>>,
    completed: bool,
}

impl InFlightStageGuard {
    fn complete(mut self) {
        self.completed = true;
    }
}

impl Drop for InFlightStageGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = in_flight_staging().lock() {
            map.remove(&self.uri);
        }
        if let Some(tx) = self.tx.take()
            && self.completed
        {
            let _ = tx.send(true);
        }
    }
}

fn staged_tracks() -> &'static Mutex<HashSet<String>> {
    STAGED_TRACKS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Returns `true` if `track_uri` has already been verified/staged in memory during this run.
pub fn is_track_staged_in_memory(track_uri: &str) -> bool {
    staged_tracks()
        .lock()
        .map(|g| g.contains(track_uri))
        .unwrap_or(false)
}

/// Marks `track_uri` as staged in memory so subsequent play requests skip async metadata/key waits.
pub fn mark_track_staged_in_memory(track_uri: &str) {
    if let Ok(mut g) = staged_tracks().lock() {
        g.insert(track_uri.to_string());
    }
}

/// Clears the in-memory set of staged track URIs (used on cache clear or sign-out).
pub fn clear_staged_tracks_in_memory() {
    if let Ok(mut g) = staged_tracks().lock() {
        g.clear();
    }
}

fn is_cached_file_unencrypted(file: &mut std::fs::File) -> bool {
    let mut buf = [0u8; SPOTIFY_OGG_HEADER_SIZE + 4];
    if file.read_exact(&mut buf).is_err() {
        return false;
    }
    let body = &buf[SPOTIFY_OGG_HEADER_SIZE..];
    &buf[0..4] == b"OggS"
        || (&buf[0..4] == FALLBACK_HEADER_MAGIC
            && (body == b"OggS"
                || &body[0..3] == b"ID3"
                || (body[0] == 0xff && (body[1] & 0xe0) == 0xe0)))
}

/// Removes encrypted or obsolete fallback audio cache files left over from prior runs so `Player`
/// never stalls trying to decrypt them or plays 30s chorus previews from `PLSE` entries.
pub fn purge_stale_encrypted_audio_cache(audio_cache_dir: &Path) {
    let mut dirs = vec![audio_cache_dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let keep = std::fs::File::open(&path)
                .map(|mut f| is_cached_file_unencrypted(&mut f))
                .unwrap_or(false);
            if !keep {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Trims a raw Ogg byte buffer down to the end of its last complete `OggS` page,
/// preventing mid-page decode glitches when storing truncated `head_file` clips.
pub fn trim_to_valid_ogg_pages(bytes: &[u8]) -> &[u8] {
    let mut offset = 0usize;
    let mut last_valid_end = 0usize;
    while offset + 27 <= bytes.len() {
        if &bytes[offset..offset + 4] != b"OggS" || bytes[offset + 4] != 0 {
            break;
        }
        let num_segments = bytes[offset + 26] as usize;
        let header_end = offset + 27 + num_segments;
        if header_end > bytes.len() {
            break;
        }
        let body_len: usize = bytes[offset + 27..header_end]
            .iter()
            .map(|&b| b as usize)
            .sum();
        let Some(page_end) = header_end.checked_add(body_len) else {
            break;
        };
        if page_end > bytes.len() {
            break;
        }
        last_valid_end = page_end;
        offset = page_end;
    }
    &bytes[..last_valid_end]
}

fn build_with_magic(magic: &[u8; 4], audio_bytes: &[u8]) -> Vec<u8> {
    let mut buf = vec![0u8; SPOTIFY_OGG_HEADER_SIZE + audio_bytes.len()];
    buf[0..4].copy_from_slice(magic);
    // Neutral ReplayGain / NormalisationData fields expected at bytes 144..160.
    buf[144..148].copy_from_slice(&0.0_f32.to_le_bytes());
    buf[148..152].copy_from_slice(&1.0_f32.to_le_bytes());
    buf[152..156].copy_from_slice(&0.0_f32.to_le_bytes());
    buf[156..160].copy_from_slice(&1.0_f32.to_le_bytes());
    buf[SPOTIFY_OGG_HEADER_SIZE..].copy_from_slice(audio_bytes);
    buf
}

fn build_preview_with_spotify_header(audio_bytes: &[u8]) -> Vec<u8> {
    build_with_magic(FALLBACK_HEADER_MAGIC, audio_bytes)
}

fn contains_term(haystack_lower: &str, term: &str) -> bool {
    let bytes = haystack_lower.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = haystack_lower[start..].find(term) {
        let abs = start + pos;
        let end = abs + term.len();
        let left_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let right_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

fn has_unwanted_variant(candidate_title: &str, spotify_title: &str) -> bool {
    const FORBIDDEN: &[&str] = &[
        "sped up",
        "speed up",
        "slowed",
        "reverb",
        "nightcore",
        "remix",
        "rmx",
        "mashup",
        "instrumental",
        "karaoke",
        "cover",
        "acoustic",
        "8d",
        "loop",
        "best part",
        "live",
        "extended",
        "bootleg",
        "bass boosted",
        "fast",
        "mix",
        "parody",
        "bitcrushed",
        "8-bit",
    ];
    let cand = candidate_title.to_ascii_lowercase();
    let target = spotify_title.to_ascii_lowercase();
    FORBIDDEN
        .iter()
        .any(|term| contains_term(&cand, term) && !contains_term(&target, term))
}

fn normalize_alnum(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_title_suffixes(title: &str) -> &str {
    let mut out = title;
    for sep in [" (", " [", " - "] {
        if let Some(idx) = out.find(sep)
            && idx >= 3
        {
            out = &out[..idx];
        }
    }
    out.trim()
}

fn title_matches(candidate: &str, spotify_title: &str) -> bool {
    let cand_norm = normalize_alnum(candidate);
    let base_norm = normalize_alnum(strip_title_suffixes(spotify_title));
    if base_norm.is_empty() || cand_norm.is_empty() {
        return false;
    }
    if cand_norm.contains(&base_norm) {
        return true;
    }
    let words: Vec<&str> = base_norm.split_whitespace().collect();
    let matched = words
        .iter()
        .filter(|w| contains_term(&cand_norm, w))
        .count();
    matched * 2 >= words.len() && matched >= 1
}

fn artist_matches(haystack: &str, artist: &str) -> bool {
    let hay = normalize_alnum(haystack);
    let art = normalize_alnum(artist);
    if hay.is_empty() || art.is_empty() {
        return true;
    }
    if hay.contains(&art) {
        return true;
    }
    art.split_whitespace()
        .filter(|w| w.len() >= 3)
        .any(|w| contains_term(&hay, w))
}

async fn http_get_bytes(http: &HttpClient, url: &str) -> Option<Bytes> {
    let req = Request::builder()
        .method(Method::GET)
        .uri(url)
        .body(Bytes::new())
        .ok()?;
    tokio::time::timeout(Duration::from_secs(12), http.request_body(req))
        .await
        .ok()?
        .ok()
}

async fn resolve_redirect_url(http: &HttpClient, start_url: &str) -> String {
    let mut current = start_url.to_string();
    for _ in 0..5 {
        let Ok(req) = Request::builder()
            .method(Method::GET)
            .uri(&current)
            .body(Bytes::new())
        else {
            break;
        };
        let Ok(fut) = http.request_fut(req) else {
            break;
        };
        let Ok(Ok(resp)) = tokio::time::timeout(Duration::from_secs(10), fut).await else {
            break;
        };
        if resp.status().is_redirection()
            && let Some(loc) = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
            && let Ok(base) = url::Url::parse(&current)
            && let Ok(next) = base.join(loc)
        {
            current = next.to_string();
            continue;
        }
        break;
    }
    current
}

async fn http_post_json(
    http: &HttpClient,
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Option<Value> {
    let json_bytes = serde_json::to_vec(body).ok()?;
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(url)
        .header(header::CONTENT_TYPE, "application/json");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let req = builder.body(Bytes::from(json_bytes)).ok()?;
    let raw = tokio::time::timeout(Duration::from_secs(10), http.request_body(req))
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&raw).ok()
}

fn extract_sc_client_id_from_js(js: &str) -> Option<String> {
    for marker in ["client_id:\"", "client_id:'", "client_id%3D"] {
        let mut idx = 0usize;
        while let Some(pos) = js[idx..].find(marker) {
            let start = idx + pos + marker.len();
            let tail = &js[start..];
            let id: String = tail
                .chars()
                .take(40)
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if (24..=40).contains(&id.len()) {
                return Some(id);
            }
            idx = start;
        }
    }
    None
}

async fn refresh_soundcloud_client_id(http: &HttpClient) -> Option<String> {
    let html_bytes = http_get_bytes(http, "https://soundcloud.com").await?;
    let html = String::from_utf8_lossy(&html_bytes);
    let mut script_urls = Vec::new();
    let mut idx = 0usize;
    while let Some(pos) = html[idx..].find("https://a-v2.sndcdn.com/assets/") {
        let start = idx + pos;
        if let Some(end_rel) = html[start..].find(".js") {
            let end = start + end_rel + 3;
            script_urls.push(html[start..end].to_string());
            idx = end;
        } else {
            break;
        }
    }
    for url in script_urls.into_iter().rev().take(5) {
        if let Some(js_bytes) = http_get_bytes(http, &url).await {
            let js = String::from_utf8_lossy(&js_bytes);
            if let Some(cid) = extract_sc_client_id_from_js(&js) {
                return Some(cid);
            }
        }
    }
    None
}

async fn get_soundcloud_client_id(http: &HttpClient, force_refresh: bool) -> String {
    let lock = SOUNDCLOUD_CLIENT_ID.get_or_init(|| AsyncMutex::new(None));
    let mut guard = lock.lock().await;
    if force_refresh {
        if let Some(fresh) = refresh_soundcloud_client_id(http).await {
            *guard = Some(fresh.clone());
            return fresh;
        }
        return guard
            .clone()
            .unwrap_or_else(|| DEFAULT_SOUNDCLOUD_CLIENT_ID.to_string());
    }
    guard
        .get_or_insert_with(|| DEFAULT_SOUNDCLOUD_CLIENT_ID.to_string())
        .clone()
}

async fn ffmpeg_transcode_to_ogg(input: Vec<u8>) -> Option<Vec<u8>> {
    if input.is_empty() || input.len() > MAX_DOWNLOAD_BYTES {
        return None;
    }
    let res = tokio::time::timeout(
        Duration::from_secs(12),
        tokio::task::spawn_blocking(move || -> Option<Vec<u8>> {
            let mut child = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-i",
                    "pipe:0",
                    "-vn",
                    "-c:a",
                    "libvorbis",
                    "-q:a",
                    "5",
                    "-f",
                    "ogg",
                    "pipe:1",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;

            let mut stdin = child.stdin.take()?;
            let mut stdout = child.stdout.take()?;

            let out = std::thread::scope(|s| {
                let writer = s.spawn(move || {
                    let _ = stdin.write_all(&input);
                    drop(stdin);
                });
                let mut ogg = Vec::new();
                let mut limited = (&mut stdout).take(MAX_FFMPEG_OUTPUT_BYTES);
                let read_res = limited.read_to_end(&mut ogg);
                let _ = writer.join();
                read_res.ok().map(|_| ogg)
            });

            let status = child.wait().ok()?;
            if !status.success() {
                return None;
            }
            let ogg = out?;
            let trimmed = trim_to_valid_ogg_pages(&ogg);
            if trimmed.len() < 28 || &trimmed[0..4] != b"OggS" {
                return None;
            }
            Some(trimmed.to_vec())
        }),
    )
    .await;

    match res {
        Ok(Ok(Some(ogg))) => Some(ogg),
        _ => None,
    }
}

struct SoundCloudCandidate {
    score: i64,
    transcode_url: String,
    is_hls: bool,
    track_auth: String,
}

fn score_soundcloud_item(
    item: &Value,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<SoundCloudCandidate> {
    let cand_title = item.get("title")?.as_str()?;
    let duration = item.get("duration")?.as_u64()?;
    let full_duration = item
        .get("full_duration")
        .and_then(|v| v.as_u64())
        .unwrap_or(duration);
    let policy = item.get("policy").and_then(|v| v.as_str()).unwrap_or("");

    // Reject 30-second SoundCloud Go+ preview snippets cleanly.
    if duration <= 35_000 || (duration <= 32_000 && full_duration > 35_000) || policy == "SNIP" {
        return None;
    }
    if duration > 12 * 60 * 1000 {
        return None;
    }
    if has_unwanted_variant(cand_title, spotify_title) {
        return None;
    }
    if !title_matches(cand_title, spotify_title) {
        return None;
    }
    if spotify_duration_ms > 0 && duration.abs_diff(spotify_duration_ms) > 25_000 {
        return None;
    }

    let uploader = item
        .get("user")
        .and_then(|u| u.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let meta_artist = item
        .get("publisher_metadata")
        .and_then(|p| p.get("artist"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let combined_haystack = format!("{cand_title} {uploader} {meta_artist}");
    if !artist_matches(&combined_haystack, primary_artist) {
        return None;
    }

    let transcodings = item
        .get("media")
        .and_then(|m| m.get("transcodings"))
        .and_then(|t| t.as_array())?;

    let progressive = transcodings.iter().find(|t| {
        t.get("format")
            .and_then(|f| f.get("protocol"))
            .and_then(|p| p.as_str())
            == Some("progressive")
    });
    let hls_mp4 = transcodings.iter().find(|t| {
        t.get("format")
            .and_then(|f| f.get("protocol"))
            .and_then(|p| p.as_str())
            == Some("hls")
            && t.get("format")
                .and_then(|f| f.get("mime_type"))
                .and_then(|m| m.as_str())
                .is_some_and(|m| m.contains("mp4"))
    });
    let hls_any = transcodings.iter().find(|t| {
        t.get("format")
            .and_then(|f| f.get("protocol"))
            .and_then(|p| p.as_str())
            == Some("hls")
            && !t
                .get("snipped")
                .and_then(|s| s.as_bool())
                .unwrap_or(false)
    });

    let chosen = progressive.or(hls_mp4).or(hls_any)?;
    let is_hls = chosen
        .get("format")
        .and_then(|f| f.get("protocol"))
        .and_then(|p| p.as_str())
        == Some("hls");
    let transcode_url = chosen.get("url")?.as_str()?.to_string();
    let track_auth = item
        .get("track_authorization")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut score = 0i64;
    if !is_hls {
        score += 10;
    }
    if spotify_duration_ms > 0 {
        score -= (duration.abs_diff(spotify_duration_ms) as i64) / 1000;
    }
    if item
        .get("user")
        .and_then(|u| u.get("verified"))
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        score += 30;
    }
    if artist_matches(uploader, primary_artist) || artist_matches(meta_artist, primary_artist) {
        score += 20;
    }
    Some(SoundCloudCandidate {
        score,
        transcode_url,
        is_hls,
        track_auth,
    })
}

async fn download_hls_segments(http: &HttpClient, m3u8_url: &str) -> Option<Vec<u8>> {
    let m3u8_bytes = http_get_bytes(http, m3u8_url).await?;
    let m3u8_text = String::from_utf8_lossy(&m3u8_bytes);
    let mut urls: Vec<String> = Vec::new();
    for line in m3u8_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#EXT-X-KEY") && !line.contains("METHOD=NONE") {
            return None;
        }
        if let Some(idx) = line.find("URI=\"") {
            let tail = &line[idx + 5..];
            if let Some(end) = tail.find('"') {
                urls.push(tail[..end].to_string());
            }
            continue;
        }
        if !line.starts_with('#') {
            urls.push(line.to_string());
        }
    }
    if urls.is_empty() || urls.len() > 120 {
        return None;
    }

    let mut combined = Vec::new();
    for batch in urls.chunks(6) {
        let chunks = futures_util::future::join_all(
            batch.iter().map(|u| http_get_bytes(http, u)),
        )
        .await;
        for chunk in chunks {
            let chunk = chunk?;
            if combined.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
                return None;
            }
            combined.extend_from_slice(&chunk);
        }
    }
    Some(combined)
}

async fn try_fetch_soundcloud_ogg(
    http: &HttpClient,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<Vec<u8>> {
    let mut queries = vec![format!("{primary_artist} {spotify_title}")];
    if !primary_artist.is_empty() {
        queries.push(format!("{spotify_title} {primary_artist}"));
    }

    let mut cid = get_soundcloud_client_id(http, false).await;
    let mut best: Option<SoundCloudCandidate> = None;

    for raw_q in queries {
        let query = url::form_urlencoded::byte_serialize(raw_q.as_bytes()).collect::<String>();
        let mut search_json: Option<Value> = None;

        for attempt in 0..2 {
            let search_url = format!(
                "https://api-v2.soundcloud.com/search/tracks?q={query}&client_id={cid}&limit=25"
            );
            if let Some(body) = http_get_bytes(http, &search_url).await
                && let Ok(parsed) = serde_json::from_slice::<Value>(&body)
            {
                search_json = Some(parsed);
                break;
            }
            if attempt == 0 {
                cid = get_soundcloud_client_id(http, true).await;
            }
        }

        if let Some(collection) = search_json
            .as_ref()
            .and_then(|j| j.get("collection"))
            .and_then(|c| c.as_array())
        {
            for item in collection {
                if let Some(cand) =
                    score_soundcloud_item(item, spotify_title, primary_artist, spotify_duration_ms)
                    && best.as_ref().is_none_or(|b| cand.score > b.score)
                {
                    best = Some(cand);
                }
            }
        }

        if best.is_some() {
            break;
        }
    }

    let cand = best?;
    let sep = if cand.transcode_url.contains('?') { '&' } else { '?' };
    let resolve_url = if cand.track_auth.is_empty() {
        format!("{}{}client_id={cid}", cand.transcode_url, sep)
    } else {
        let auth_enc: String =
            url::form_urlencoded::byte_serialize(cand.track_auth.as_bytes()).collect();
        format!(
            "{}{}client_id={cid}&track_authorization={auth_enc}",
            cand.transcode_url, sep
        )
    };
    let stream_meta_bytes = http_get_bytes(http, &resolve_url).await?;
    let stream_meta: Value = serde_json::from_slice(&stream_meta_bytes).ok()?;
    let direct_url = stream_meta.get("url")?.as_str()?;

    let raw_audio = if cand.is_hls || direct_url.contains(".m3u8") {
        download_hls_segments(http, direct_url).await?
    } else {
        let b = http_get_bytes(http, direct_url).await?;
        if b.len() > MAX_DOWNLOAD_BYTES {
            return None;
        }
        b.to_vec()
    };
    if raw_audio.len() < 16_000 {
        return None;
    }
    ffmpeg_transcode_to_ogg(raw_audio).await
}

fn score_audius_item(
    item: &Value,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<(i64, String)> {
    if item.get("is_streamable").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let id = item.get("id")?.as_str()?.trim();
    if id.is_empty() {
        return None;
    }
    let cand_title = item.get("title")?.as_str()?;
    let duration_secs = item.get("duration")?.as_u64()?;
    let duration_ms = duration_secs.saturating_mul(1000);
    if (duration_ms <= 35_000 && spotify_duration_ms > 40_000) || duration_ms > 12 * 60 * 1000 {
        return None;
    }
    if spotify_duration_ms > 0 && duration_ms.abs_diff(spotify_duration_ms) > 18_000 {
        return None;
    }
    if has_unwanted_variant(cand_title, spotify_title) || !title_matches(cand_title, spotify_title)
    {
        return None;
    }
    let user_name = item
        .get("user")
        .and_then(|u| u.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let user_handle = item
        .get("user")
        .and_then(|u| u.get("handle"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let combined = format!("{cand_title} {user_name} {user_handle}");
    if !artist_matches(&combined, primary_artist) {
        return None;
    }

    let mut score = 0i64;
    if spotify_duration_ms > 0 {
        score -= (duration_ms.abs_diff(spotify_duration_ms) as i64) / 1000;
    }
    if artist_matches(user_name, primary_artist) {
        score += 30;
    }
    Some((score, id.to_string()))
}

async fn try_fetch_audius_ogg(
    http: &HttpClient,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<Vec<u8>> {
    let raw_query = if primary_artist.is_empty() {
        spotify_title.to_string()
    } else {
        format!("{primary_artist} {spotify_title}")
    };
    let query: String = url::form_urlencoded::byte_serialize(raw_query.as_bytes()).collect();
    let search_url = format!("https://discoveryprovider.audius.co/v1/tracks/search?query={query}");
    let body = http_get_bytes(http, &search_url).await?;
    let json: Value = serde_json::from_slice(&body).ok()?;
    let items = json.get("data")?.as_array()?;

    let mut best: Option<(i64, String)> = None;
    for item in items {
        if let Some((score, id)) =
            score_audius_item(item, spotify_title, primary_artist, spotify_duration_ms)
            && best.as_ref().is_none_or(|(b, _)| score > *b)
        {
            best = Some((score, id));
        }
    }

    let (_, track_id) = best?;
    let meta_url =
        format!("https://discoveryprovider.audius.co/v1/tracks/{track_id}/stream?no_redirect=true");
    let meta_bytes = http_get_bytes(http, &meta_url).await?;
    let meta_json: Value = serde_json::from_slice(&meta_bytes).ok()?;
    let stream_url = meta_json.get("data")?.as_str()?;
    let final_url = resolve_redirect_url(http, stream_url).await;
    let raw_audio = http_get_bytes(http, &final_url).await?;
    if raw_audio.len() < 16_000 || raw_audio.len() > MAX_DOWNLOAD_BYTES {
        return None;
    }
    ffmpeg_transcode_to_ogg(raw_audio.to_vec()).await
}

fn parse_duration_ms(label: &str) -> Option<u64> {
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
    Some(secs * 1000)
}

fn collect_video_renderers<'a>(val: &'a Value, out: &mut Vec<&'a Value>) {
    if out.len() >= 12 {
        return;
    }
    match val {
        Value::Object(map) => {
            if let Some(vr) = map.get("videoRenderer") {
                out.push(vr);
            }
            for child in map.values() {
                collect_video_renderers(child, out);
                if out.len() >= 12 {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for child in arr {
                collect_video_renderers(child, out);
                if out.len() >= 12 {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn extract_yt_text(node: &Value) -> String {
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

fn score_youtube_candidate(
    vr: &Value,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<(i64, String)> {
    let vid = vr.get("videoId")?.as_str()?;
    if vid.len() != 11 {
        return None;
    }
    let cand_title = extract_yt_text(vr.get("title")?);
    if cand_title.is_empty()
        || has_unwanted_variant(&cand_title, spotify_title)
        || !title_matches(&cand_title, spotify_title)
    {
        return None;
    }
    let owner = vr
        .get("ownerText")
        .or_else(|| vr.get("longBylineText"))
        .map(extract_yt_text)
        .unwrap_or_default();

    let combined = format!("{cand_title} {owner}");
    if !artist_matches(&combined, primary_artist) {
        return None;
    }

    let length_ms = vr
        .get("lengthText")
        .map(extract_yt_text)
        .and_then(|t| parse_duration_ms(&t));

    let mut score = 0i64;
    if let Some(dur) = length_ms {
        if dur < 35_000 && spotify_duration_ms > 40_000 {
            return None;
        }
        if spotify_duration_ms > 0 {
            let diff = dur.abs_diff(spotify_duration_ms);
            if diff > 45_000 {
                return None;
            }
            score -= (diff as i64) / 1000;
        }
    }

    if owner.ends_with(" - Topic") {
        score += 100;
    } else if artist_matches(&owner, primary_artist) {
        score += 50;
    }
    let lower_title = cand_title.to_ascii_lowercase();
    if lower_title.contains("official audio") || lower_title.contains("(audio)") {
        score += 60;
    } else if lower_title.contains("audio") || lower_title.contains("lyrics") {
        score += 25;
    }
    Some((score, vid.to_string()))
}

async fn try_fetch_youtube_ios_ogg(
    http: &HttpClient,
    spotify_title: &str,
    primary_artist: &str,
    spotify_duration_ms: u64,
) -> Option<(Vec<u8>, bool)> {
    let search_body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": YOUTUBE_WEB_CLIENT_VERSION,
                "hl": "en",
                "gl": "US"
            }
        },
        "query": format!("{primary_artist} - {spotify_title} Audio")
    });
    let search_json = http_post_json(
        http,
        "https://youtubei.googleapis.com/youtubei/v1/search?prettyPrint=false",
        &[],
        &search_body,
    )
    .await?;

    let mut renderers = Vec::new();
    collect_video_renderers(&search_json, &mut renderers);
    let mut scored: Vec<(i64, String)> = renderers
        .into_iter()
        .filter_map(|vr| {
            score_youtube_candidate(vr, spotify_title, primary_artist, spotify_duration_ms)
        })
        .collect();
    scored.sort_by_key(|a| std::cmp::Reverse(a.0));

    for (_, vid) in scored.into_iter().take(2) {
        let player_body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "IOS",
                    "clientVersion": YOUTUBE_IOS_CLIENT_VERSION,
                    "deviceMake": "Apple",
                    "deviceModel": "iPhone16,2",
                    "osName": "iOS",
                    "osVersion": "18.3.2.22D82",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": vid,
            "contentCheckOk": true,
            "racyCheckOk": true
        });
        let Some(player_json) = http_post_json(
            http,
            "https://youtubei.googleapis.com/youtubei/v1/player?prettyPrint=false",
            &[
                ("X-YouTube-Client-Name", "5"),
                ("X-YouTube-Client-Version", YOUTUBE_IOS_CLIENT_VERSION),
            ],
            &player_body,
        )
        .await
        else {
            continue;
        };
        if player_json
            .get("playabilityStatus")
            .and_then(|p| p.get("status"))
            .and_then(|s| s.as_str())
            != Some("OK")
        {
            continue;
        }
        let Some(formats) = player_json
            .get("streamingData")
            .and_then(|s| s.get("adaptiveFormats"))
            .and_then(|f| f.as_array())
        else {
            continue;
        };

        let chosen_fmt = formats
            .iter()
            .find(|f| f.get("itag").and_then(|i| i.as_i64()) == Some(140))
            .or_else(|| {
                formats.iter().find(|f| {
                    f.get("mimeType")
                        .and_then(|m| m.as_str())
                        .is_some_and(|m| m.starts_with("audio/"))
                })
            });

        let Some(fmt) = chosen_fmt else {
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

        // YouTube IOS serves ~1 MB (~61.5s at itag 140) starting cleanly at 0:00 via ranged GETs.
        let mut raw = Vec::new();
        let chunk_size = 200_000usize;
        let sep = if base_url.contains('?') { '&' } else { '?' };
        for chunk_idx in 0..8usize {
            let start = chunk_idx * chunk_size;
            let end = start + chunk_size - 1;
            let ranged_url = format!("{base_url}{sep}range={start}-{end}");
            let Some(chunk) = http_get_bytes(http, &ranged_url).await else {
                break;
            };
            if chunk.is_empty() {
                break;
            }
            let len = chunk.len();
            if raw.len() + len > MAX_DOWNLOAD_BYTES {
                break;
            }
            raw.extend_from_slice(&chunk);
            if len < chunk_size {
                break;
            }
        }

        if raw.len() >= 100_000 {
            let is_complete = raw
                .len()
                .saturating_mul(100)
                .checked_div(expected_len)
                .map_or(
                    spotify_duration_ms > 0 && spotify_duration_ms <= 65_000,
                    |pct| pct >= 85,
                );
            if let Some(ogg) = ffmpeg_transcode_to_ogg(raw).await {
                return Some((ogg, is_complete));
            }
        }
    }
    None
}

async fn acquire_staging_slot(track_uri_str: &str) -> Option<InFlightStageGuard> {
    loop {
        if is_track_staged_in_memory(track_uri_str) {
            return None;
        }
        let mut rx = {
            let mut map = in_flight_staging().lock().ok()?;
            if let Some(existing_tx) = map.get(track_uri_str) {
                existing_tx.subscribe()
            } else {
                let (tx, _rx) = watch::channel(false);
                map.insert(track_uri_str.to_string(), tx.clone());
                return Some(InFlightStageGuard {
                    uri: track_uri_str.to_string(),
                    tx: Some(tx),
                    completed: false,
                });
            }
        };
        let _ = rx.changed().await;
        if *rx.borrow() {
            return None;
        }
    }
}

/// Ensures `track_uri_str` is playable when Spotify rejects `AudioKey` requests (`error audio key 0 1`).
///
/// If the session AP grants audio keys, returns cleanly so encrypted full-track streaming proceeds.
/// Once AP rejects an audio key request, resolves unencrypted `0:00` audio (Stage A: SoundCloud full
/// clean track; Stage B: YouTube InnerTube `IOS` `0:00`-~61.5s; Stage C: longest trimmed `head_file`;
/// Stage D: Spotify preview MP3 last resort) and caches it under the track's Ogg Vorbis `FileId`s.
pub async fn ensure_track_playable_in_cache(session: &Session, track_uri_str: &str) {
    let Some(guard) = acquire_staging_slot(track_uri_str).await else {
        return;
    };
    let Ok(uri) = SpotifyUri::from_uri(track_uri_str) else {
        return;
    };
    let SpotifyUri::Track { id: track_id } = uri else {
        return;
    };
    let Some(cache) = session.cache() else {
        return;
    };
    let Ok(mut track) = <Track as Metadata>::get(session, &uri).await else {
        return;
    };

    let mut key_track_id = track_id;
    let mut head_fids = collect_head_file_ids(&track);
    if head_fids.is_empty() {
        for alt_uri in &track.alternatives.0 {
            if let Ok(alt_track) = <Track as Metadata>::get(session, alt_uri).await {
                let alt_fids = collect_head_file_ids(&alt_track);
                if !alt_fids.is_empty() {
                    if let SpotifyUri::Track { id } = alt_track.id {
                        key_track_id = id;
                    }
                    track = alt_track;
                    head_fids = alt_fids;
                    break;
                }
            }
        }
    }

    let Some(&first_fid) = head_fids.first() else {
        return;
    };

    if cache
        .file(first_fid)
        .is_some_and(|mut f| is_cached_file_unencrypted(&mut f))
    {
        mark_track_staged_in_memory(track_uri_str);
        guard.complete();
        return;
    }

    if !AUDIO_KEY_REJECTED.load(Ordering::Relaxed) {
        let key_res = tokio::time::timeout(
            Duration::from_millis(1500),
            session.audio_key().request(key_track_id, first_fid),
        )
        .await;
        if let Ok(Ok(_key)) = key_res {
            mark_track_staged_in_memory(track_uri_str);
            guard.complete();
            return;
        }
        AUDIO_KEY_REJECTED.store(true, Ordering::Relaxed);
    }

    let spotify_title = track.name.trim().to_string();
    let primary_artist = track
        .artists
        .0
        .first()
        .map(|a| a.name.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            track
                .artists_with_role
                .0
                .first()
                .map(|a| a.name.trim())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            track
                .album
                .artists
                .0
                .first()
                .map(|a| a.name.trim())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default()
        .to_string();
    let spotify_duration_ms = track.duration.max(0) as u64;
    let http = session.http_client();

    // Stage A: SoundCloud full-length clean match.
    let mut bytes_to_save =
        try_fetch_soundcloud_ogg(http, &spotify_title, &primary_artist, spotify_duration_ms)
            .await
            .map(|ogg| build_preview_with_spotify_header(&ogg));

    // Stage B: Audius decentralized full-length clean match.
    if bytes_to_save.is_none() {
        bytes_to_save =
            try_fetch_audius_ogg(http, &spotify_title, &primary_artist, spotify_duration_ms)
                .await
                .map(|ogg| build_preview_with_spotify_header(&ogg));
    }

    // Stage C: YouTube InnerTube IOS (0:00 studio audio; temporary PLST if truncated < 85%).
    if bytes_to_save.is_none()
        && let Some((ogg, is_complete)) =
            try_fetch_youtube_ios_ogg(http, &spotify_title, &primary_artist, spotify_duration_ms)
                .await
    {
        let magic = if is_complete {
            FALLBACK_HEADER_MAGIC
        } else {
            TEMP_FALLBACK_MAGIC
        };
        bytes_to_save = Some(build_with_magic(magic, &ogg));
    }

    // Stage D: Spotify head_file ordered lowest-bitrate-first (maximizes 0:00 duration).
    if bytes_to_save.is_none() {
        for fid in &head_fids {
            if let Ok(head) = session.spclient().get_head_file(fid).await
                && head.len() > SPOTIFY_OGG_HEADER_SIZE
            {
                let trimmed = trim_to_valid_ogg_pages(&head[SPOTIFY_OGG_HEADER_SIZE..]);
                if !trimmed.is_empty() {
                    bytes_to_save = Some(build_with_magic(TEMP_FALLBACK_MAGIC, trimmed));
                    break;
                }
            }
        }
    }

    // Stage E: Spotify 30s MP3 preview strictly as last offline resort.
    if bytes_to_save.is_none()
        && let Some(preview_id) = track.previews.0.values().next()
        && let Ok(mp3) = session.spclient().get_audio_preview(preview_id).await
        && mp3.len() >= 4
    {
        bytes_to_save = Some(build_with_magic(TEMP_FALLBACK_MAGIC, &mp3));
    }

    let Some(bytes_to_save) = bytes_to_save else {
        return;
    };

    for fid in head_fids {
        let _ = cache.remove_file(fid);
        let _ = cache.save_file(fid, &mut Cursor::new(&bytes_to_save));
    }
    mark_track_staged_in_memory(track_uri_str);
    guard.complete();
}

/// Builds a `Spirc` `LoadRequest` from a context URI with current shuffle/repeat flags.
pub fn build_context_load_request(
    uri: &str,
    playing_track: Option<PlayingTrack>,
    shuffle: bool,
    repeat: RepeatMode,
    start_playing: bool,
    seek_to: u32,
) -> LoadRequest {
    let (repeat_ctx, repeat_track) = match repeat {
        RepeatMode::Off => (false, false),
        RepeatMode::Context => (true, false),
        RepeatMode::Track => (false, true),
    };
    LoadRequest::from_context_uri(
        uri.to_string(),
        LoadRequestOptions {
            start_playing,
            seek_to,
            playing_track,
            context_options: Some(LoadContextOptions::Options(Options {
                shuffle,
                repeat: repeat_ctx,
                repeat_track,
            })),
        },
    )
}

/// Builds a `Spirc` `LoadRequest` from a concrete list of track URIs
/// (used for `LikedSongs`, `Search`, etc. where a single Spotify context URI doesn't exist).
pub fn build_tracks_load_request(
    track_uris: Vec<String>,
    playing_track: Option<PlayingTrack>,
    shuffle: bool,
    repeat: RepeatMode,
    start_playing: bool,
    seek_to: u32,
) -> LoadRequest {
    let (repeat_ctx, repeat_track) = match repeat {
        RepeatMode::Off => (false, false),
        RepeatMode::Context => (true, false),
        RepeatMode::Track => (false, true),
    };
    LoadRequest::from_tracks(
        track_uris,
        LoadRequestOptions {
            start_playing,
            seek_to,
            playing_track,
            context_options: Some(LoadContextOptions::Options(Options {
                shuffle,
                repeat: repeat_ctx,
                repeat_track,
            })),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ogg_page(body: &[u8]) -> Vec<u8> {
        let segs = body.len().div_ceil(255).max(1);
        let mut page = vec![0u8; 27 + segs + body.len()];
        page[0..4].copy_from_slice(b"OggS");
        page[4] = 0;
        page[26] = segs as u8;
        let mut rem = body.len();
        for i in 0..segs {
            let chunk = rem.min(255);
            page[27 + i] = chunk as u8;
            rem -= chunk;
        }
        page[27 + segs..].copy_from_slice(body);
        page
    }

    #[test]
    fn trim_to_valid_ogg_pages_keeps_complete_pages_and_drops_truncated_tail() {
        let p1 = make_ogg_page(b"first-page-payload");
        let p2 = make_ogg_page(b"second-page-payload-data");
        let p3 = make_ogg_page(b"third-page-truncated");

        let mut raw = Vec::new();
        raw.extend_from_slice(&p1);
        raw.extend_from_slice(&p2);
        let expected_len = raw.len();
        raw.extend_from_slice(&p3[..p3.len() - 4]);

        let trimmed = trim_to_valid_ogg_pages(&raw);
        assert_eq!(trimmed.len(), expected_len);
        assert_eq!(trimmed, &raw[..expected_len]);
    }

    #[test]
    fn has_unwanted_variant_respects_word_boundaries_and_spotify_title() {
        assert!(has_unwanted_variant("Harvey (Sped Up)", "Harvey"));
        assert!(has_unwanted_variant("Earrings - Slowed + Reverb", "Earrings"));
        assert!(has_unwanted_variant("I'm Not In Love (Live)", "I'm Not In Love"));
        assert!(!has_unwanted_variant("Olivia - Deliver Us", "Olivia"));
        assert!(!has_unwanted_variant("Discover Credit", "Discover"));
        assert!(!has_unwanted_variant(
            "Harvey (Slowed Version)",
            "Harvey (Slowed)"
        ));
    }

    #[test]
    fn purge_stale_encrypted_audio_cache_drops_old_and_temp_entries_and_keeps_pls4() {
        let dir = std::env::temp_dir().join(format!(
            "pulse_spotify_cache_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let old_plse = dir.join("old_plse");
        let mut old_bytes = vec![0u8; SPOTIFY_OGG_HEADER_SIZE + 16];
        old_bytes[0..4].copy_from_slice(b"PLSE");
        old_bytes[SPOTIFY_OGG_HEADER_SIZE..SPOTIFY_OGG_HEADER_SIZE + 3].copy_from_slice(b"ID3");
        std::fs::write(&old_plse, &old_bytes).unwrap();

        let old_pls3 = dir.join("old_pls3");
        let pls3_bytes = build_with_magic(b"PLS3", b"OggS\x00\x02prev");
        std::fs::write(&old_pls3, &pls3_bytes).unwrap();

        let temp_plst = dir.join("temp_plst");
        let plst_bytes = build_with_magic(TEMP_FALLBACK_MAGIC, b"OggS\x00\x02temp");
        std::fs::write(&temp_plst, &plst_bytes).unwrap();

        let valid_pls4 = dir.join("valid_pls4");
        let pls4_bytes = build_preview_with_spotify_header(b"OggS\x00\x02rest");
        std::fs::write(&valid_pls4, &pls4_bytes).unwrap();

        purge_stale_encrypted_audio_cache(&dir);

        assert!(!old_plse.exists(), "stale PLSE cache must be deleted");
        assert!(!old_pls3.exists(), "old PLS3 cache must be upgraded");
        assert!(!temp_plst.exists(), "temporary PLST clip must be refreshed");
        assert!(valid_pls4.exists(), "valid PLS4 Ogg cache must be kept");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_duration_ms_handles_mm_ss_and_h_mm_ss() {
        assert_eq!(parse_duration_ms("03:31"), Some(211_000));
        assert_eq!(parse_duration_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_duration_ms("bad"), None);
    }
}
