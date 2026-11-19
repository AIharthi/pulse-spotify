//! Pure-Rust Spotify Connect, playback, library, and MPRIS2 backend for Pulse.

pub mod auth;
pub mod config;
pub mod connect;
pub mod library;
pub mod mpris;
pub mod player;
pub mod queue;
pub mod telemetry;

use std::collections::HashSet;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use pulse_model::{
    AccountTier, AppConfig, ArtistDetail, AuthState, BackendEvent, ConnectionStatus, CrateRef,
    DeviceInfo, DevicesState, LibraryScope, PlayerState, RepeatMode, SearchResults, SortOrder,
    TrackDetail, UiCommand,
};
use spotatui_librespot_connect::{LoadRequest, PlayingTrack};
use spotatui_librespot_core::Session;
use spotatui_librespot_core::authentication::Credentials;
use spotatui_librespot_core::config::SessionConfig;
use spotatui_librespot_core::http_client::HttpClient;
use spotatui_librespot_core::spotify_uri::SpotifyUri;
use spotatui_librespot_metadata::audio::{AudioItem, UniqueFields};
use spotatui_librespot_playback::player::PlayerEvent;
use tokio::sync::{mpsc, oneshot};

pub use librespot_discovery as discovery;
pub use mpris_server;
pub use pulse_model::{AppViewState, BackendEvent as Event, UiCommand as Command};
pub use spotatui_librespot_connect as librespot_connect;
pub use spotatui_librespot_core as librespot_core;
pub use spotatui_librespot_metadata as librespot_metadata;
pub use spotatui_librespot_playback as librespot_playback;
pub use spotatui_librespot_protocol as librespot_protocol;

use crate::auth::{
    ConnectOutcome, KEYMASTER_CLIENT_ID, LIBRESPOT_REDIRECT_URI, LIBRESPOT_SCOPES,
    OAuthCancelToken, PkceChallenge, WEB_API_REDIRECT_URI, WEB_API_SCOPES, build_authorize_url,
    connect_and_verify_session, convert_discovery_credentials, exchange_pkce_code,
    wait_for_oauth_callback,
};
use crate::config::{
    AppPaths, AuthMeta, RecentEntry, RecentHistory, WebApiToken, clear_audio_cache_dir,
    clear_auth_files, clear_web_api_token, load_auth_meta, load_config, load_or_create_device_id,
    load_recent_history, load_web_api_token, open_librespot_cache, prune_dir_to_bytes_lru,
    save_auth_meta, save_config, save_recent_history, save_web_api_token,
};
use crate::connect::{
    RemotePlaybackSnapshot, RemoteQueueMeta, cmd_add_to_queue, cmd_play_context, cmd_play_tracks,
    cmd_seek_to, cmd_set_queue, cmd_set_repeat, cmd_set_shuffle, devices_from_cluster,
    extract_remote_queue_meta, manual_track_to_provided_json, refresh_cluster_passive,
    send_remote_command, set_remote_volume, snapshot_from_cluster, transfer_playback,
};
use crate::library::{LibraryService, fetch_cover_bytes};
use crate::mpris::MprisController;
use crate::player::{
    ClusterStream, LocalPlayback, audio_pipeline_changed, build_context_load_request,
    build_tracks_load_request, clear_staged_tracks_in_memory, ensure_track_playable_in_cache,
    is_track_staged_in_memory, purge_stale_encrypted_audio_cache, start_local_playback,
    stream_audio_metrics, unmark_track_staged_in_memory,
};
use crate::queue::TwoTierQueue;
use crate::telemetry::{CoverStatsHook, collect_telemetry};

/// Callback invoked whenever the backend pushes one or more [`BackendEvent`]s.
pub type WakeupCallback = Arc<dyn Fn() + Send + Sync>;

struct CoverFetchReq {
    url: String,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Thread-safe handle for controlling the dedicated Spotify backend thread from the UI.
pub struct BackendHandle {
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    cover_tx: mpsc::UnboundedSender<CoverFetchReq>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    cover_stats: CoverStatsHook,
    initial_config: AppConfig,
    thread: Option<JoinHandle<()>>,
}

impl BackendHandle {
    /// Spawns the single-threaded Tokio Spotify backend thread using default XDG paths.
    pub fn spawn(
        wakeup: WakeupCallback,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BackendEvent>), String> {
        Self::spawn_with_paths(AppPaths::from_xdg(), wakeup)
    }

    /// Spawns the backend thread using explicit filesystem paths (primarily for isolated tests).
    pub fn spawn_with_paths(
        paths: AppPaths,
        wakeup: WakeupCallback,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BackendEvent>), String> {
        let initial_config = load_config(&paths);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cover_tx, cover_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let cover_stats = CoverStatsHook::new();
        let cover_stats_thread = cover_stats.clone();
        let internal_cmd_tx = cmd_tx.clone();
        let config_for_thread = initial_config.clone();

        let thread = thread::Builder::new()
            .name("pulse-backend".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = event_tx.send(BackendEvent::ConnectionChanged(
                            ConnectionStatus::Error(format!(
                                "failed to initialize Tokio runtime: {err}"
                            )),
                        ));
                        wakeup();
                        return;
                    }
                };
                let local = tokio::task::LocalSet::new();
                rt.block_on(local.run_until(actor_main(
                    paths,
                    config_for_thread,
                    cover_stats_thread,
                    internal_cmd_tx,
                    cmd_rx,
                    cover_rx,
                    event_tx,
                    wakeup,
                    shutdown_rx,
                )));
            })
            .map_err(|e| format!("failed to spawn pulse-backend thread: {e}"))?;

        Ok((
            Self {
                cmd_tx,
                cover_tx,
                shutdown_tx: Some(shutdown_tx),
                cover_stats,
                initial_config,
                thread: Some(thread),
            },
            event_rx,
        ))
    }

    /// Returns the persisted [`AppConfig`] loaded synchronously at startup.
    pub fn initial_config(&self) -> &AppConfig {
        &self.initial_config
    }

    /// Enqueues a [`UiCommand`] for asynchronous execution on the backend thread.
    pub fn send(&self, cmd: UiCommand) -> Result<(), String> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| "backend actor is no longer running".to_string())
    }

    /// Requests raw image bytes (with bounded disk caching) and returns a oneshot receiver.
    pub fn request_cover_bytes(
        &self,
        url: impl Into<String>,
    ) -> oneshot::Receiver<Result<Vec<u8>, String>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.cover_tx.send(CoverFetchReq {
            url: url.into(),
            reply: tx,
        });
        rx
    }

    /// Fetches cover image bytes asynchronously via the backend cover pipeline.
    pub async fn fetch_cover_bytes(&self, url: impl Into<String>) -> Result<Vec<u8>, String> {
        self.request_cover_bytes(url)
            .await
            .map_err(|_| "backend cover task dropped".to_string())?
    }

    /// Fetches cover image bytes synchronously (for non-async cover worker threads).
    pub fn fetch_cover_bytes_blocking(&self, url: impl Into<String>) -> Result<Vec<u8>, String> {
        self.request_cover_bytes(url)
            .blocking_recv()
            .map_err(|_| "backend cover task dropped".to_string())?
    }

    /// Returns a thread-safe hook allowing UI layers to publish cover-cache RSS & slot metrics.
    pub fn cover_stats_hook(&self) -> CoverStatsHook {
        self.cover_stats.clone()
    }

    /// Gracefully shuts down playback, discovery, MPRIS, and joins the backend thread.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum InternalMsg {
    LibrespotOAuthFinished {
        attempt_id: u64,
        result: Result<WebApiToken, String>,
    },
    WebApiOAuthFinished {
        attempt_id: u64,
        result: Result<WebApiToken, String>,
    },
    DiscoveryCredentials(Credentials),
    DiscoveryError(String),
    SessionConnected {
        session_gen: u64,
        outcome: ConnectOutcome,
        zeroconf_only: bool,
    },
    SessionConnectFailed {
        session_gen: u64,
        error: String,
    },
    ReconnectTimerFired {
        session_gen: u64,
    },
    ClusterReceived(spotatui_librespot_protocol::connect::Cluster),
    PlayerEvent(PlayerEvent),
    DisplayNameLoaded {
        session_gen: u64,
        name: String,
    },
    CurrentTrackDetailLoaded {
        session_gen: u64,
        uri: String,
        detail: Option<TrackDetail>,
    },
    BackendEventsBatch {
        session_gen: u64,
        result: Result<Vec<BackendEvent>, String>,
    },
    LikeMutationFinished(Result<(String, bool), String>),
    FollowMutationFinished(Result<(), String>),
    PlayCrateResolved {
        session_gen: u64,
        play_seq: u64,
        shuffle: bool,
        result: Result<(String, Option<String>, Vec<pulse_model::TrackSummary>), String>,
    },
    QueueCrateResolved {
        session_gen: u64,
        result: Result<(String, Option<String>, Vec<pulse_model::TrackSummary>), String>,
    },
    PlayTrackResolved {
        session_gen: u64,
        play_seq: u64,
        context_uri: String,
        track_uri: String,
        hint_index: Option<usize>,
        tracks: Result<Vec<pulse_model::TrackSummary>, String>,
    },
    CoverFetched {
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
        result: Result<Vec<u8>, String>,
    },
    ExecuteStagedLocalLoad {
        session_gen: u64,
        seq: u64,
        request: LoadRequest,
        sync_queue: bool,
    },
    UnavailableRetryReady {
        session_gen: u64,
        uri: String,
    },
}

struct Emitter {
    tx: mpsc::UnboundedSender<BackendEvent>,
    wakeup: WakeupCallback,
}

impl Emitter {
    fn emit(&self, ev: BackendEvent) {
        if self.tx.send(ev).is_ok() {
            (self.wakeup)();
        }
    }

    fn error(&self, message: impl Into<String>) {
        self.emit(BackendEvent::Error(message.into()));
    }
}

struct BackendState {
    paths: AppPaths,
    config: AppConfig,
    device_id: String,
    auth_meta: AuthMeta,
    web_api_token: Rc<tokio::sync::Mutex<Option<WebApiToken>>>,
    recent_history: RecentHistory,
    auth_state: AuthState,
    conn_status: ConnectionStatus,
    player_state: PlayerState,
    queue: TwoTierQueue,
    devices_state: DevicesState,
    sort_order: SortOrder,
    pre_mute_volume: u16,
    local_is_active: bool,
    local_queue_owned: bool,
    current_creds: Option<Credentials>,
    retry_creds: Option<(Credentials, bool)>,
    session: Option<Session>,
    session_gen: u64,
    oauth_attempt_id: u64,
    web_oauth_attempt_id: u64,
    local_playback: Option<LocalPlayback>,
    library: Rc<tokio::sync::Mutex<LibraryService>>,
    http: Arc<HttpClient>,
    oauth_cancel: Option<OAuthCancelToken>,
    web_oauth_cancel: Option<OAuthCancelToken>,
    pending_web_api_oauth: Option<String>,
    current_crate: CrateRef,
    last_crate_tracks: Option<(CrateRef, usize, Vec<pulse_model::TrackSummary>)>,
    last_artist_detail: Option<ArtistDetail>,
    last_search_results: Option<SearchResults>,
    in_flight_detail_uri: Option<String>,
    play_load_seq: u64,
    remote_queue_meta: Option<RemoteQueueMeta>,
    discovery_task: Option<tokio::task::JoinHandle<()>>,
    cluster_task: Option<tokio::task::JoinHandle<()>>,
    player_ev_task: Option<tokio::task::JoinHandle<()>>,
    mpris: Option<MprisController>,
    cover_stats: CoverStatsHook,
    pending_seek_emit: bool,
    local_player_stopped: bool,
    local_spirc_queue_dirty: bool,
    current_playing_uri: Option<String>,
    loading_track_uri: Option<String>,
    pending_local_load_uri: Option<String>,
    pending_local_load_paused: bool,
    active_play_request_id: Option<u64>,
    expecting_remote_transfer_until: Option<Instant>,
    retried_unavailable_uri: Option<String>,
    consecutive_unavailable_skips: usize,
}

impl BackendState {
    fn is_remote_active(&self) -> Option<String> {
        let active_id = self.devices_state.active_device_id.as_ref()?;
        if active_id != &self.device_id {
            Some(active_id.clone())
        } else {
            None
        }
    }

    fn is_expecting_remote_transfer(&self) -> bool {
        self.expecting_remote_transfer_until
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    fn sync_audio_metrics(&mut self) {
        let (badge, rate, kbps) = stream_audio_metrics(&self.config);
        self.player_state.audio_badge = badge;
        self.player_state.sample_rate_hz = rate;
        self.player_state.bitrate_kbps = kbps;
    }

    fn emit_config(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::ConfigUpdated(self.config.clone()));
    }

    fn emit_player(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::PlaybackChanged(self.player_state.clone()));
    }

    fn emit_queue(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::QueueChanged(self.queue.to_queue_state()));
    }

    fn emit_devices(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::DevicesUpdated(self.devices_state.clone()));
    }

    async fn sync_mpris(&self, seeked: bool) {
        if let Some(ref mpris) = self.mpris {
            mpris.sync_player_state(&self.player_state, seeked).await;
        }
    }

    fn refresh_offline_capability(&self) -> bool {
        self.config.disk_audio_cache_mb > 0
            && crate::telemetry::dir_size_bytes(&self.paths.audio_cache_dir) > 0
    }

    fn save_current_config(&self, emitter: &Emitter) {
        if let Err(err) = save_config(&self.paths, &self.config) {
            emitter.error(format!("Failed to save config: {err}"));
        }
        self.emit_config(emitter);
    }

    async fn rebuild_local_player_if_active(
        &mut self,
        int_tx: &mpsc::UnboundedSender<InternalMsg>,
        emitter: &Emitter,
    ) {
        if self.session.is_none() {
            return;
        }
        let Ok(cache) = open_librespot_cache(&self.paths, &self.config) else {
            emitter.error("failed to open Pulse cache directory");
            return;
        };
        let Some(creds) = cache.credentials().or_else(|| self.current_creds.clone()) else {
            return;
        };
        let old_vol = self.player_state.volume;
        teardown_session(self).await;
        self.local_player_stopped = true;
        self.player_state.is_playing = false;
        self.emit_player(emitter);
        self.sync_mpris(false).await;

        let session_cfg = SessionConfig {
            device_id: self.device_id.clone(),
            ..Default::default()
        };
        let new_session = Session::new(session_cfg, Some(cache));
        match start_local_playback(&new_session, creds.clone(), &self.config, old_vol).await {
            Ok(mut lp) => {
                self.current_creds = Some(
                    new_session
                        .cache()
                        .and_then(|c| c.credentials())
                        .unwrap_or(creds),
                );
                self.session = Some(new_session);
                self.conn_status = ConnectionStatus::Connected;
                emitter.emit(BackendEvent::ConnectionChanged(self.conn_status.clone()));
                let events = lp.player.get_player_event_channel();
                let cluster_stream = lp.cluster_stream.take();
                self.local_playback = Some(lp);
                spawn_player_event_loop(self, events, int_tx);
                if let Some(stream) = cluster_stream {
                    spawn_cluster_listener(self, stream, int_tx);
                }
            }
            Err(err) => {
                self.conn_status = ConnectionStatus::Error(err.clone());
                emitter.emit(BackendEvent::ConnectionChanged(self.conn_status.clone()));
                emitter.error(err);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn actor_main(
    paths: AppPaths,
    config: AppConfig,
    cover_stats: CoverStatsHook,
    ui_cmd_tx: mpsc::UnboundedSender<UiCommand>,
    mut cmd_rx: mpsc::UnboundedReceiver<UiCommand>,
    mut cover_rx: mpsc::UnboundedReceiver<CoverFetchReq>,
    event_tx: mpsc::UnboundedSender<BackendEvent>,
    wakeup: WakeupCallback,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let _ = paths.ensure_dirs();
    purge_stale_encrypted_audio_cache(&paths.audio_cache_dir);
    let device_id = load_or_create_device_id(&paths);
    let auth_meta = load_auth_meta(&paths);
    let web_api_token = load_web_api_token(&paths);
    let recent_history = load_recent_history(&paths);

    let (int_tx, mut int_rx) = mpsc::unbounded_channel::<InternalMsg>();
    let emitter = Emitter {
        tx: event_tx,
        wakeup,
    };

    let mpris = if config.mpris_enabled {
        match MprisController::start(ui_cmd_tx.clone(), config.media_keys_enabled).await {
            Ok(ctrl) => Some(ctrl),
            Err(err) => {
                log::warn!("MPRIS2 unavailable: {err}");
                None
            }
        }
    } else {
        None
    };

    let initial_vol = open_librespot_cache(&paths, &config)
        .ok()
        .and_then(|c| c.volume())
        .unwrap_or(65_535);

    let mut state = BackendState {
        paths,
        config: config.clone(),
        device_id: device_id.clone(),
        auth_meta,
        web_api_token: Rc::new(tokio::sync::Mutex::new(web_api_token)),
        recent_history,
        auth_state: if config.zeroconf_enabled {
            AuthState::ConnectOnly {
                device_name: config.device_name.clone(),
            }
        } else {
            AuthState::Unauthenticated
        },
        conn_status: ConnectionStatus::Disconnected,
        player_state: PlayerState {
            volume: initial_vol,
            ..Default::default()
        },
        queue: TwoTierQueue::new(),
        devices_state: DevicesState {
            active_device_id: None,
            local_device_id: device_id,
            local_device_name: config.device_name.clone(),
            zeroconf_active: config.zeroconf_enabled,
            available_devices: Vec::new(),
        },
        sort_order: SortOrder::default(),
        pre_mute_volume: initial_vol.max(32_768),
        local_is_active: false,
        local_queue_owned: false,
        current_creds: None,
        retry_creds: None,
        session: None,
        session_gen: 0,
        oauth_attempt_id: 0,
        web_oauth_attempt_id: 0,
        local_playback: None,
        library: Rc::new(tokio::sync::Mutex::new(LibraryService::new())),
        http: Arc::new(HttpClient::new(None)),
        oauth_cancel: None,
        web_oauth_cancel: None,
        pending_web_api_oauth: None,
        current_crate: CrateRef::LikedSongs,
        last_crate_tracks: None,
        last_artist_detail: None,
        last_search_results: None,
        in_flight_detail_uri: None,
        play_load_seq: 0,
        remote_queue_meta: None,
        discovery_task: None,
        cluster_task: None,
        player_ev_task: None,
        mpris,
        cover_stats,
        pending_seek_emit: false,
        local_player_stopped: false,
        local_spirc_queue_dirty: false,
        current_playing_uri: None,
        loading_track_uri: None,
        pending_local_load_uri: None,
        pending_local_load_paused: false,
        active_play_request_id: None,
        expecting_remote_transfer_until: None,
        retried_unavailable_uri: None,
        consecutive_unavailable_skips: 0,
    };
    state.sync_audio_metrics();

    state.emit_config(&emitter);
    emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
    emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
    state.emit_player(&emitter);
    state.emit_devices(&emitter);
    emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
        &state.paths,
        &state.cover_stats,
    )));

    if state.config.zeroconf_enabled {
        restart_discovery(&mut state, &int_tx);
    }

    if let Ok(cache) = open_librespot_cache(&state.paths, &state.config)
        && let Some(creds) = cache.credentials()
    {
        let zeroconf_only = state.auth_meta.zeroconf_only;
        spawn_session_connect(
            &mut state,
            creds,
            zeroconf_only,
            &int_tx,
            &emitter,
        );
    }

    let mut telemetry_tick = tokio::time::interval(Duration::from_secs(2));
    telemetry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                if let Some(c) = state.oauth_cancel.take() {
                    c.cancel();
                }
                if let Some(c) = state.web_oauth_cancel.take() {
                    c.cancel();
                }
                teardown_session(&mut state).await;
                stop_discovery(&mut state);
                if let Some(m) = state.mpris.take() {
                    m.stop();
                }
                break;
            }
            Some(cmd) = cmd_rx.recv() => {
                handle_ui_command(&mut state, cmd, &ui_cmd_tx, &int_tx, &emitter).await;
            }
            Some(msg) = int_rx.recv() => {
                handle_internal_msg(&mut state, msg, &int_tx, &emitter).await;
            }
            Some(req) = cover_rx.recv() => {
                let paths = state.paths.clone();
                let cfg = state.config.clone();
                let http = Arc::clone(&state.http);
                let sess = state.session.clone();
                let tx = int_tx.clone();
                tokio::task::spawn_local(async move {
                    let result = fetch_cover_bytes(&paths, &cfg, &http, sess.as_ref(), &req.url).await;
                    let _ = tx.send(InternalMsg::CoverFetched {
                        reply: req.reply,
                        result,
                    });
                });
            }
            _ = telemetry_tick.tick() => {
                enforce_audio_cache_limit(&state);
                emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
                    &state.paths,
                    &state.cover_stats,
                )));
            }
        }
    }
}

fn restart_discovery(state: &mut BackendState, int_tx: &mpsc::UnboundedSender<InternalMsg>) {
    stop_discovery(state);
    if !state.config.zeroconf_enabled {
        state.devices_state.zeroconf_active = false;
        return;
    }

    let builder = librespot_discovery::Discovery::builder(
        state.device_id.clone(),
        KEYMASTER_CLIENT_ID.to_string(),
    )
    .name(state.config.device_name.clone())
    .device_type(librespot_discovery::DeviceType::Computer);

    match builder.launch() {
        Ok(mut stream) => {
            state.devices_state.zeroconf_active = true;
            let tx = int_tx.clone();
            state.discovery_task = Some(tokio::task::spawn_local(async move {
                while let Some(raw_creds) = stream.next().await {
                    match convert_discovery_credentials(raw_creds) {
                        Ok(creds) => {
                            let _ = tx.send(InternalMsg::DiscoveryCredentials(creds));
                        }
                        Err(err) => {
                            let _ = tx.send(InternalMsg::DiscoveryError(err));
                        }
                    }
                }
                stream.shutdown().await;
            }));
        }
        Err(err) => {
            state.devices_state.zeroconf_active = false;
            log::warn!("Zeroconf discovery failed to start: {err}");
        }
    }
}

fn stop_discovery(state: &mut BackendState) {
    if let Some(handle) = state.discovery_task.take() {
        handle.abort();
    }
    state.devices_state.zeroconf_active = false;
}

async fn teardown_session(state: &mut BackendState) {
    if let Some(h) = state.cluster_task.take() {
        h.abort();
    }
    if let Some(h) = state.player_ev_task.take() {
        h.abort();
    }
    if let Some(lp) = state.local_playback.take() {
        lp.shutdown().await;
    }
    if let Some(sess) = state.session.take() {
        sess.shutdown();
    }
    state.local_is_active = false;
    state.local_queue_owned = false;
    state.local_player_stopped = false;
    state.local_spirc_queue_dirty = false;
    state.loading_track_uri = None;
    state.pending_local_load_uri = None;
    state.pending_local_load_paused = false;
    state.active_play_request_id = None;
    state.expecting_remote_transfer_until = None;
    state.retried_unavailable_uri = None;
    state.consecutive_unavailable_skips = 0;
}

fn spawn_session_connect(
    state: &mut BackendState,
    creds: Credentials,
    zeroconf_only: bool,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    state.session_gen = state.session_gen.wrapping_add(1);
    let session_gen = state.session_gen;
    state.retry_creds = Some((creds.clone(), zeroconf_only));
    state.conn_status = ConnectionStatus::Connecting;
    emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));

    let Ok(cache) = open_librespot_cache(&state.paths, &state.config) else {
        let msg = "failed to open Pulse cache directory".to_string();
        state.conn_status = ConnectionStatus::Error(msg.clone());
        emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
        emitter.error(msg);
        return;
    };

    let session_cfg = SessionConfig {
        device_id: state.device_id.clone(),
        ..Default::default()
    };

    let paths = state.paths.clone();
    let cfg = state.config.clone();
    let initial_vol = state.player_state.volume;
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        match connect_and_verify_session(&paths, session_cfg, cache, creds, &cfg, initial_vol).await
        {
            Ok(outcome) => {
                let _ = tx.send(InternalMsg::SessionConnected {
                    session_gen,
                    outcome,
                    zeroconf_only,
                });
            }
            Err(error) => {
                let _ = tx.send(InternalMsg::SessionConnectFailed { session_gen, error });
            }
        }
    });
}

fn open_url_in_browser(url: &str) {
    if let Ok(mut child) = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn start_librespot_oauth(
    state: &mut BackendState,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    if let Some(prev) = state.oauth_cancel.take() {
        prev.cancel();
    }
    state.oauth_attempt_id = state.oauth_attempt_id.wrapping_add(1);
    let attempt_id = state.oauth_attempt_id;
    let pkce = PkceChallenge::generate();
    let auth_url = match build_authorize_url(
        KEYMASTER_CLIENT_ID,
        LIBRESPOT_REDIRECT_URI,
        LIBRESPOT_SCOPES,
        &pkce,
    ) {
        Ok(u) => u,
        Err(e) => {
            emitter.error(e);
            return;
        }
    };
    let cancel = OAuthCancelToken::new();
    state.oauth_cancel = Some(cancel.clone());
    if !matches!(state.auth_state, AuthState::Authenticated { .. }) {
        state.auth_state = AuthState::WaitingOAuth {
            auth_url: auth_url.clone(),
        };
        emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
    }
    open_url_in_browser(&auth_url);

    let http = Arc::clone(&state.http);
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let result = async {
            let code = wait_for_oauth_callback(
                "127.0.0.1:5588".to_string(),
                "/login".to_string(),
                pkce.state,
                cancel,
            )
            .await?;
            exchange_pkce_code(
                &http,
                KEYMASTER_CLIENT_ID,
                &code,
                LIBRESPOT_REDIRECT_URI,
                &pkce.verifier,
            )
            .await
        }
        .await;
        let _ = tx.send(InternalMsg::LibrespotOAuthFinished { attempt_id, result });
    });
}

fn start_web_api_oauth(
    state: &mut BackendState,
    custom_cid: String,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    if let Some(prev) = state.web_oauth_cancel.take() {
        prev.cancel();
    }
    state.web_oauth_attempt_id = state.web_oauth_attempt_id.wrapping_add(1);
    let attempt_id = state.web_oauth_attempt_id;
    let pkce = PkceChallenge::generate();
    let auth_url = match build_authorize_url(
        &custom_cid,
        WEB_API_REDIRECT_URI,
        WEB_API_SCOPES,
        &pkce,
    ) {
        Ok(u) => u,
        Err(e) => {
            emitter.error(e);
            return;
        }
    };
    let cancel = OAuthCancelToken::new();
    state.web_oauth_cancel = Some(cancel.clone());
    emitter.emit(BackendEvent::Notice(Some(format!(
        "Authorize optional Web API in browser: {auth_url}"
    ))));
    open_url_in_browser(&auth_url);

    let http = Arc::clone(&state.http);
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let result = async {
            let code = wait_for_oauth_callback(
                "127.0.0.1:8898".to_string(),
                "/callback".to_string(),
                pkce.state,
                cancel,
            )
            .await?;
            exchange_pkce_code(
                &http,
                &custom_cid,
                &code,
                WEB_API_REDIRECT_URI,
                &pkce.verifier,
            )
            .await
        }
        .await;
        let _ = tx.send(InternalMsg::WebApiOAuthFinished { attempt_id, result });
    });
}

async fn handle_ui_command(
    state: &mut BackendState,
    cmd: UiCommand,
    ui_cmd_tx: &mpsc::UnboundedSender<UiCommand>,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match cmd {
        UiCommand::StartOAuthLogin { client_id } => {
            let has_full_session = state.session.is_some() && !state.auth_meta.zeroconf_only;
            let custom_cid = client_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            if let Some(cid) = custom_cid {
                state.config.custom_client_id = Some(cid.clone());
                state.save_current_config(emitter);
                if has_full_session {
                    start_web_api_oauth(state, cid, int_tx, emitter);
                } else {
                    state.pending_web_api_oauth = Some(cid);
                    start_librespot_oauth(state, int_tx, emitter);
                }
            } else {
                state.pending_web_api_oauth = None;
                start_librespot_oauth(state, int_tx, emitter);
            }
        }
        UiCommand::CancelOAuthLogin => {
            if let Some(c) = state.oauth_cancel.take() {
                c.cancel();
            }
            if let Some(c) = state.web_oauth_cancel.take() {
                c.cancel();
            }
            state.oauth_attempt_id = state.oauth_attempt_id.wrapping_add(1);
            state.web_oauth_attempt_id = state.web_oauth_attempt_id.wrapping_add(1);
            state.pending_web_api_oauth = None;
            if !matches!(state.auth_state, AuthState::Authenticated { .. }) {
                state.auth_state = if state.config.zeroconf_enabled {
                    AuthState::ConnectOnly {
                        device_name: state.config.device_name.clone(),
                    }
                } else {
                    AuthState::Unauthenticated
                };
                emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
            }
        }
        UiCommand::SaveClientId(cid) => {
            state.config.custom_client_id = cid
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
            state.save_current_config(emitter);
        }
        UiCommand::SetZeroconf {
            enabled,
            device_name,
        } => {
            let trimmed = device_name.trim();
            if !trimmed.is_empty() {
                state.config.device_name = trimmed.to_string();
                state.devices_state.local_device_name = state.config.device_name.clone();
            }
            state.config.zeroconf_enabled = enabled;
            state.save_current_config(emitter);
            restart_discovery(state, int_tx);
            if state.session.is_none() {
                state.auth_state = if enabled {
                    AuthState::ConnectOnly {
                        device_name: state.config.device_name.clone(),
                    }
                } else {
                    AuthState::Unauthenticated
                };
                emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
            }
            state.emit_devices(emitter);
        }
        UiCommand::SignOut => {
            state.session_gen = state.session_gen.wrapping_add(1);
            state.oauth_attempt_id = state.oauth_attempt_id.wrapping_add(1);
            state.web_oauth_attempt_id = state.web_oauth_attempt_id.wrapping_add(1);
            if let Some(c) = state.oauth_cancel.take() {
                c.cancel();
            }
            if let Some(c) = state.web_oauth_cancel.take() {
                c.cancel();
            }
            teardown_session(state).await;
            state.current_creds = None;
            state.retry_creds = None;
            *state.web_api_token.lock().await = None;
            state.pending_web_api_oauth = None;
            state.current_crate = CrateRef::LikedSongs;
            state.last_crate_tracks = None;
            state.last_artist_detail = None;
            state.last_search_results = None;
            state.in_flight_detail_uri = None;
            state.play_load_seq = state.play_load_seq.wrapping_add(1);
            state.local_player_stopped = false;
            state.local_spirc_queue_dirty = false;
            state.current_playing_uri = None;
            state.loading_track_uri = None;
            state.pending_local_load_uri = None;
            state.pending_local_load_paused = false;
            state.active_play_request_id = None;
            state.expecting_remote_transfer_until = None;
            state.retried_unavailable_uri = None;
            state.consecutive_unavailable_skips = 0;
            clear_staged_tracks_in_memory();
            state.remote_queue_meta = None;
            state.auth_meta = AuthMeta::default();
            state.library.lock().await.invalidate();
            let _ = clear_auth_files(&state.paths);
            let _ = clear_web_api_token(&state.paths);

            state.queue.clear_manual();
            state.queue.clear_context();
            state.queue.set_current_track(None);
            state.player_state.is_playing = false;
            state.player_state.active_track = None;
            state.player_state.position_ms = 0;
            state.player_state.duration_ms = 0;
            state.devices_state.active_device_id = None;
            state.devices_state.available_devices.clear();
            state.conn_status = ConnectionStatus::Disconnected;
            state.auth_state = if state.config.zeroconf_enabled {
                AuthState::ConnectOnly {
                    device_name: state.config.device_name.clone(),
                }
            } else {
                AuthState::Unauthenticated
            };
            emitter.emit(BackendEvent::Notice(None));
            emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
            emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
            state.emit_player(emitter);
            state.emit_queue(emitter);
            state.emit_devices(emitter);
            state.sync_mpris(false).await;
        }
        UiCommand::PurgeCoverCache => {
            if state.paths.cover_cache_dir.exists() {
                let _ = fs::remove_dir_all(&state.paths.cover_cache_dir);
            }
            let _ = fs::create_dir_all(&state.paths.cover_cache_dir);
            emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
                &state.paths,
                &state.cover_stats,
            )));
        }
        UiCommand::ClearAudioCache => {
            if let Err(err) = clear_audio_cache_dir(&state.paths) {
                emitter.error(format!("Failed to clear audio cache: {err}"));
            }
            clear_staged_tracks_in_memory();
            emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
                &state.paths,
                &state.cover_stats,
            )));
        }
        UiCommand::Play => {
            state.pending_local_load_paused = false;
            state.consecutive_unavailable_skips = 0;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = send_remote_command(
                    &sess,
                    &remote_id,
                    serde_json::json!({ "endpoint": "resume" }),
                )
                .await;
            } else if state.player_state.active_track.is_none() {
                if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                    start_play_crate(
                        state,
                        CrateRef::LikedSongs,
                        state.player_state.shuffle,
                        int_tx,
                        emitter,
                    )
                    .await;
                }
            } else if state.local_player_stopped {
                let uris = state.queue.remaining_uris_from_current();
                if !uris.is_empty() {
                    let play_uri = state
                        .queue
                        .current_uri()
                        .map(ToOwned::to_owned)
                        .or_else(|| uris.first().cloned());
                    if let Some(ref sum) =
                        play_uri.as_deref().and_then(|u| cached_track_summary(state, u))
                    {
                        set_optimistic_playing_track(state, sum, int_tx, emitter);
                    }
                    let loaded_now = load_tracks_on_active_device(
                        state,
                        uris,
                        play_uri,
                        false,
                        int_tx,
                        emitter,
                    )
                    .await;
                    if loaded_now && !state.queue.manual_items().is_empty() {
                        sync_manual_queue_to_active_device(state).await;
                    }
                } else if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                    start_play_crate(
                        state,
                        CrateRef::LikedSongs,
                        state.player_state.shuffle,
                        int_tx,
                        emitter,
                    )
                    .await;
                }
            } else if let Some(ref lp) = state.local_playback {
                let _ = lp.spirc.activate();
                let _ = lp.spirc.play();
                state.player_state.is_playing = true;
                state.emit_player(emitter);
            }
        }
        UiCommand::Pause => {
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = send_remote_command(
                    &sess,
                    &remote_id,
                    serde_json::json!({ "endpoint": "pause" }),
                )
                .await;
            } else if let Some(ref lp) = state.local_playback {
                if state.pending_local_load_uri.is_some() {
                    state.pending_local_load_paused = true;
                }
                lp.request_sink_flush();
                let _ = lp.spirc.pause();
                state.player_state.is_playing = false;
                state.emit_player(emitter);
            }
        }
        UiCommand::TogglePlayPause => {
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let ep = if state.player_state.is_playing {
                    "pause"
                } else {
                    "resume"
                };
                let _ = send_remote_command(&sess, &remote_id, serde_json::json!({ "endpoint": ep }))
                    .await;
            } else if state.player_state.active_track.is_none() {
                if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                    start_play_crate(
                        state,
                        CrateRef::LikedSongs,
                        state.player_state.shuffle,
                        int_tx,
                        emitter,
                    )
                    .await;
                }
            } else if state.player_state.is_playing {
                if state.pending_local_load_uri.is_some() {
                    state.pending_local_load_paused = true;
                }
                if let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                    let _ = lp.spirc.pause();
                    state.player_state.is_playing = false;
                    state.emit_player(emitter);
                }
            } else if state.local_player_stopped {
                state.pending_local_load_paused = false;
                state.consecutive_unavailable_skips = 0;
                let uris = state.queue.remaining_uris_from_current();
                if !uris.is_empty() {
                    let play_uri = state
                        .queue
                        .current_uri()
                        .map(ToOwned::to_owned)
                        .or_else(|| uris.first().cloned());
                    if let Some(ref sum) =
                        play_uri.as_deref().and_then(|u| cached_track_summary(state, u))
                    {
                        set_optimistic_playing_track(state, sum, int_tx, emitter);
                    }
                    let loaded_now = load_tracks_on_active_device(
                        state,
                        uris,
                        play_uri,
                        false,
                        int_tx,
                        emitter,
                    )
                    .await;
                    if loaded_now && !state.queue.manual_items().is_empty() {
                        sync_manual_queue_to_active_device(state).await;
                    }
                } else if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                    start_play_crate(
                        state,
                        CrateRef::LikedSongs,
                        state.player_state.shuffle,
                        int_tx,
                        emitter,
                    )
                    .await;
                }
            } else if let Some(ref lp) = state.local_playback {
                state.pending_local_load_paused = false;
                let _ = lp.spirc.activate();
                let _ = lp.spirc.play();
                state.player_state.is_playing = true;
                state.emit_player(emitter);
            }
        }
        UiCommand::NextTrack => {
            state.consecutive_unavailable_skips = 0;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = send_remote_command(
                    &sess,
                    &remote_id,
                    serde_json::json!({ "endpoint": "skip_next" }),
                )
                .await;
            } else if state.local_playback.is_some() {
                if let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                }
                if !state.local_queue_owned {
                    if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.next();
                    }
                } else {
                    let wrap = state.player_state.repeat == RepeatMode::Context;
                    if let Some(next) = state.queue.step_next_track(wrap) {
                        state.emit_queue(emitter);
                        set_optimistic_playing_track(state, &next, int_tx, emitter);
                        let uris = state.queue.remaining_uris_from_current();
                        let loaded_now = load_tracks_on_active_device(
                            state,
                            uris,
                            Some(next.uri.to_string()),
                            false,
                            int_tx,
                            emitter,
                        )
                        .await;
                        if loaded_now && !state.queue.manual_items().is_empty() {
                            sync_manual_queue_to_active_device(state).await;
                        }
                    } else if state.queue.current_uri().is_some() {
                        if let Some(ref lp) = state.local_playback {
                            let _ = lp.spirc.pause();
                        }
                        state.local_player_stopped = true;
                        state.player_state.is_playing = false;
                        state.player_state.position_ms = 0;
                        state.emit_player(emitter);
                        state.sync_mpris(false).await;
                    } else if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.next();
                    }
                }
            }
        }
        UiCommand::PrevTrack => {
            state.consecutive_unavailable_skips = 0;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = send_remote_command(
                    &sess,
                    &remote_id,
                    serde_json::json!({ "endpoint": "skip_prev" }),
                )
                .await;
            } else if state.local_playback.is_some() {
                if let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                }
                if state.player_state.position_ms > 3_000 {
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    if state.loading_track_uri.is_none()
                        && let Some(ref lp) = state.local_playback
                    {
                        state.pending_seek_emit = true;
                        let _ = lp.spirc.set_position_ms(0);
                    }
                } else if !state.local_queue_owned {
                    if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.prev();
                    }
                } else {
                    let wrap = state.player_state.repeat == RepeatMode::Context;
                    if let Some(prev) = state.queue.step_prev_track(wrap) {
                        state.emit_queue(emitter);
                        set_optimistic_playing_track(state, &prev, int_tx, emitter);
                        let uris = state.queue.remaining_uris_from_current();
                        let loaded_now = load_tracks_on_active_device(
                            state,
                            uris,
                            Some(prev.uri.to_string()),
                            false,
                            int_tx,
                            emitter,
                        )
                        .await;
                        if loaded_now && !state.queue.manual_items().is_empty() {
                            sync_manual_queue_to_active_device(state).await;
                        }
                    } else if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.prev();
                    }
                }
            }
        }
        UiCommand::SeekMs(pos_ms) => {
            let clamped = if state.player_state.duration_ms > 0 {
                pos_ms.min(state.player_state.duration_ms)
            } else {
                pos_ms
            };
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                state.player_state.position_ms = clamped;
                state.emit_player(emitter);
                state.sync_mpris(true).await;
                let _ = send_remote_command(&sess, &remote_id, cmd_seek_to(clamped)).await;
            } else if state.loading_track_uri.is_none()
                && state.pending_local_load_uri.is_none()
                && let Some(ref lp) = state.local_playback
            {
                state.player_state.position_ms = clamped;
                state.pending_seek_emit = true;
                state.emit_player(emitter);
                lp.request_sink_flush();
                let _ = lp.spirc.set_position_ms(clamped);
            }
        }
        UiCommand::SetVolume(vol) => {
            if vol > 0 {
                state.pre_mute_volume = vol;
                state.player_state.muted = false;
            } else {
                state.player_state.muted = true;
            }
            state.player_state.volume = vol;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = set_remote_volume(&sess, &remote_id, vol).await;
            } else if let Some(ref lp) = state.local_playback {
                let _ = lp.spirc.set_volume(vol);
            }
        }
        UiCommand::ToggleMute => {
            let new_vol = if state.player_state.muted || state.player_state.volume == 0 {
                state.player_state.muted = false;
                state.pre_mute_volume.max(8_192)
            } else {
                state.pre_mute_volume = state.player_state.volume;
                state.player_state.muted = true;
                0
            };
            state.player_state.volume = new_vol;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = set_remote_volume(&sess, &remote_id, new_vol).await;
            } else if let Some(ref lp) = state.local_playback {
                let _ = lp.spirc.set_volume(new_vol);
            }
        }
        UiCommand::ToggleShuffle => {
            let enabled = !state.player_state.shuffle;
            state.player_state.shuffle = enabled;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                state.emit_player(emitter);
                state.sync_mpris(false).await;
                let _ = send_remote_command(&sess, &remote_id, cmd_set_shuffle(enabled)).await;
            } else {
                if state.queue.set_shuffle(enabled) {
                    state.local_spirc_queue_dirty = true;
                    state.emit_queue(emitter);
                    prestage_upcoming_tracks(state);
                }
                state.emit_player(emitter);
                state.sync_mpris(false).await;
            }
        }
        UiCommand::CycleRepeat => {
            let mode = state.player_state.repeat.next();
            state.player_state.repeat = mode;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                for payload in cmd_set_repeat(mode) {
                    let _ = send_remote_command(&sess, &remote_id, payload).await;
                }
            } else if let Some(ref lp) = state.local_playback {
                // Local playback manages repeat cleanly at `EndOfTrack`; keep `Spirc` non-repeating
                // so `Spirc` never competes to reload or skip tracks behind `TwoTierQueue`.
                let _ = lp.spirc.repeat(false);
                let _ = lp.spirc.repeat_track(false);
            }
        }
        UiCommand::PlayTrackInContext {
            context_uri,
            track_uri,
            index,
        } => {
            let Some(sess) = state.session.clone() else {
                emitter.error("Sign in with Spotify Premium first.");
                return;
            };
            state.consecutive_unavailable_skips = 0;
            state.play_load_seq = state.play_load_seq.wrapping_add(1);
            let play_seq = state.play_load_seq;
            let session_gen = state.session_gen;
            if let Some(cref) = crate_ref_from_context_uri(&context_uri)
                && let Some((_, _, cached)) =
                    cached_tracks_for_crate(state, &cref, Some(&track_uri))
            {
                apply_play_track_resolved(
                    state,
                    context_uri,
                    track_uri,
                    Some(index),
                    Ok(cached),
                    int_tx,
                    emitter,
                )
                .await;
                return;
            }
            if context_uri.is_empty()
                && let Some(cached) = find_cached_track_list_for_uri(state, &track_uri)
            {
                apply_play_track_resolved(
                    state,
                    context_uri,
                    track_uri,
                    Some(index),
                    Ok(cached),
                    int_tx,
                    emitter,
                )
                .await;
                return;
            }
            let lib = Rc::clone(&state.library);
            let recent = state.recent_history.clone();
            let sort = state.sort_order;
            let tx = int_tx.clone();
            let ctx_clone = context_uri.clone();
            let track_clone = track_uri.clone();
            tokio::task::spawn_local(async move {
                let crate_ref = crate_ref_from_context_uri(&ctx_clone);
                let tracks = if let Some(cref) = crate_ref {
                    lib.lock()
                        .await
                        .resolve_crate_tracks(&sess, &cref, &recent, sort)
                        .await
                        .map(|(_, _, t)| t)
                } else {
                    resolve_uris_via_track_api(&sess, &[track_clone]).await
                };
                let _ = tx.send(InternalMsg::PlayTrackResolved {
                    session_gen,
                    play_seq,
                    context_uri: ctx_clone,
                    track_uri,
                    hint_index: Some(index),
                    tracks,
                });
            });
        }
        UiCommand::PlayCrate { crate_ref, shuffle } => {
            state.consecutive_unavailable_skips = 0;
            start_play_crate(state, crate_ref, shuffle, int_tx, emitter).await;
        }
        UiCommand::QueueCrate(crate_ref) => {
            let Some(sess) = state.session.clone() else {
                emitter.error("Sign in with Spotify Premium first.");
                return;
            };
            let session_gen = state.session_gen;
            let lib = Rc::clone(&state.library);
            let recent = state.recent_history.clone();
            let sort = state.sort_order;
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let result = lib
                    .lock()
                    .await
                    .resolve_crate_tracks(&sess, &crate_ref, &recent, sort)
                    .await;
                let _ = tx.send(InternalMsg::QueueCrateResolved {
                    session_gen,
                    result,
                });
            });
        }
        UiCommand::EnqueueNext(track) => {
            state.queue.enqueue_next(track);
            state.emit_queue(emitter);
            sync_manual_queue_to_active_device(state).await;
        }
        UiCommand::AppendQueue(track) => {
            state.queue.append(track.clone());
            state.emit_queue(emitter);
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
            {
                let _ = send_remote_command(&sess, &remote_id, cmd_add_to_queue(&track.uri)).await;
            } else {
                sync_manual_queue_to_active_device(state).await;
            }
        }
        UiCommand::ReorderManualQueue { from, to } => {
            if state.queue.reorder_manual(from, to) {
                state.emit_queue(emitter);
                sync_manual_queue_to_active_device(state).await;
            }
        }
        UiCommand::PromoteContextToManual {
            context_index,
            manual_index,
        } => {
            if let Some(promoted) = state
                .queue
                .promote_context_to_manual(context_index, manual_index)
            {
                if let Some(ref mut meta) = state.remote_queue_meta
                    && let Some(pos) = meta
                        .context_next_tracks
                        .iter()
                        .position(|v| v.get("uri").and_then(|u| u.as_str()) == Some(&*promoted.uri))
                {
                    meta.context_next_tracks.remove(pos);
                }
                state.local_spirc_queue_dirty = true;
                state.emit_queue(emitter);
                sync_manual_queue_to_active_device(state).await;
            }
        }
        UiCommand::RemoveManualQueueItem(idx) => {
            if state.queue.remove_manual(idx).is_some() {
                state.emit_queue(emitter);
                sync_manual_queue_to_active_device(state).await;
            }
        }
        UiCommand::ClearManualQueue => {
            state.queue.clear_manual();
            state.emit_queue(emitter);
            sync_manual_queue_to_active_device(state).await;
        }
        UiCommand::PlayQueueIndex(index) => {
            state.consecutive_unavailable_skips = 0;
            let _ = play_queue_index_cmd(state, index, int_tx, emitter).await;
        }
        UiCommand::OpenCrate(crate_ref) => {
            state.current_crate = crate_ref.clone();
            spawn_crate_fetch(state, crate_ref, 0, 50, int_tx);
        }
        UiCommand::FetchCratePage {
            crate_ref,
            offset,
            limit,
        } => {
            spawn_crate_fetch(state, crate_ref, offset, limit, int_tx);
        }
        UiCommand::RunGlobalSearch(query) => {
            let crate_ref = CrateRef::Search { query };
            state.current_crate = crate_ref.clone();
            spawn_crate_fetch(state, crate_ref, 0, 50, int_tx);
        }
        UiCommand::SetSortOrder(sort) => {
            state.sort_order = sort;
            state.last_crate_tracks = None;
            if state.session.is_some()
                && !matches!(
                    state.current_crate,
                    CrateRef::Search { .. } | CrateRef::Artist { .. }
                )
            {
                spawn_crate_fetch(state, state.current_crate.clone(), 0, 50, int_tx);
            }
        }
        UiCommand::ToggleLike(uri) => {
            let Some(sess) = state.session.clone() else { return };
            let lib = Rc::clone(&state.library);
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let res = lib
                    .lock()
                    .await
                    .toggle_like(&sess, &uri)
                    .await
                    .map(|liked| (uri, liked));
                let _ = tx.send(InternalMsg::LikeMutationFinished(res));
            });
        }
        UiCommand::ToggleFollowArtist { artist_uri, follow } => {
            let Some(sess) = state.session.clone() else { return };
            let lib = Rc::clone(&state.library);
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let res = lib
                    .lock()
                    .await
                    .toggle_follow_artist(&sess, &artist_uri, follow)
                    .await;
                let _ = tx.send(InternalMsg::FollowMutationFinished(res));
            });
        }
        UiCommand::RefreshDevices => {
            let Some(sess) = state.session.clone() else { return };
            if state.local_is_active {
                state.emit_devices(emitter);
                return;
            }
            let dev_id = state.device_id.clone();
            let cfg = state.config.clone();
            let vol = state.player_state.volume;
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                if let Ok(cluster) = refresh_cluster_passive(&sess, &dev_id, &cfg, vol).await {
                    let _ = tx.send(InternalMsg::ClusterReceived(cluster));
                }
            });
        }
        UiCommand::TransferPlayback { device_id } => {
            let Some(sess) = state.session.clone() else { return };
            if device_id == state.device_id {
                state.expecting_remote_transfer_until = None;
                if let Some(ref lp) = state.local_playback {
                    let _ = lp.spirc.transfer(None);
                }
            } else {
                state.expecting_remote_transfer_until = Some(Instant::now() + Duration::from_secs(8));
                let from_id = state.device_id.clone();
                if let Err(err) = transfer_playback(&sess, &from_id, &device_id).await {
                    state.expecting_remote_transfer_until = None;
                    emitter.error(err);
                }
            }
        }
        UiCommand::SetStreamQuality(quality) => {
            let old = state.config.clone();
            state.config.stream_quality = quality;
            state.save_current_config(emitter);
            state.sync_audio_metrics();
            state.emit_player(emitter);
            if audio_pipeline_changed(&old, &state.config) {
                state.rebuild_local_player_if_active(int_tx, emitter).await;
            }
        }
        UiCommand::SetNormalisation(norm) => {
            let old = state.config.clone();
            state.config.normalisation = norm;
            state.save_current_config(emitter);
            if audio_pipeline_changed(&old, &state.config) {
                state.rebuild_local_player_if_active(int_tx, emitter).await;
            }
        }
        UiCommand::SetGapless(gapless) => {
            let old = state.config.clone();
            state.config.gapless = gapless;
            state.save_current_config(emitter);
            if audio_pipeline_changed(&old, &state.config) {
                state.rebuild_local_player_if_active(int_tx, emitter).await;
            }
        }
        UiCommand::SetAudioDevice(audio_device) => {
            let old = state.config.clone();
            state.config.audio_device = audio_device;
            state.save_current_config(emitter);
            if audio_pipeline_changed(&old, &state.config) {
                state.rebuild_local_player_if_active(int_tx, emitter).await;
            }
        }
        UiCommand::SetMprisEnabled(enabled) => {
            state.config.mpris_enabled = enabled;
            state.save_current_config(emitter);
            if enabled {
                if state.mpris.is_none()
                    && let Ok(ctrl) =
                        MprisController::start(ui_cmd_tx.clone(), state.config.media_keys_enabled)
                            .await
                {
                    ctrl.sync_player_state(&state.player_state, false).await;
                    state.mpris = Some(ctrl);
                }
            } else if let Some(ctrl) = state.mpris.take() {
                ctrl.stop();
            }
        }
        UiCommand::SetConfirmClearQueue(confirm) => {
            state.config.confirm_clear_queue = confirm;
            state.save_current_config(emitter);
        }
        UiCommand::SetDiskCacheLimitMb(mb) => {
            state.config.disk_audio_cache_mb = mb;
            state.save_current_config(emitter);
            enforce_audio_cache_limit(state);
            emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
                &state.paths,
                &state.cover_stats,
            )));
        }
        UiCommand::SetScope(scope) => {
            let target = match scope {
                LibraryScope::LikedSongs => Some(CrateRef::LikedSongs),
                LibraryScope::Playlists => Some(CrateRef::PlaylistsIndex),
                LibraryScope::Albums => Some(CrateRef::AlbumsIndex),
                LibraryScope::Artists => Some(CrateRef::ArtistsIndex),
                LibraryScope::Recent => Some(CrateRef::Recent),
                LibraryScope::Search => {
                    let results = state.last_search_results.clone().unwrap_or_default();
                    state.current_crate = CrateRef::Search {
                        query: results.query.clone(),
                    };
                    emitter.emit(BackendEvent::SearchResultsLoaded(results));
                    None
                }
            };
            if let Some(crate_ref) = target {
                state.current_crate = crate_ref.clone();
                spawn_crate_fetch(state, crate_ref, 0, 50, int_tx);
            }
        }
        // UI-local presentation/navigation commands: acknowledged without side effects in backend.
        UiCommand::NavigateBack
        | UiCommand::NavigateForward
        | UiCommand::SetLocalFilter(_)
        | UiCommand::SetSearchFilter(_)
        | UiCommand::SetMode(_) => {}
    }
}

fn crate_ref_from_context_uri(uri: &str) -> Option<CrateRef> {
    if uri.is_empty() {
        return None;
    }
    if uri == "spotify:collection:tracks" || uri == "liked" {
        return Some(CrateRef::LikedSongs);
    }
    if uri == "recent" {
        return Some(CrateRef::Recent);
    }
    if uri.starts_with("spotify:playlist:") {
        return Some(CrateRef::Playlist {
            uri: Arc::from(uri),
            name: String::new(),
        });
    }
    if uri.starts_with("spotify:album:") {
        return Some(CrateRef::Album {
            uri: Arc::from(uri),
            title: String::new(),
        });
    }
    if uri.starts_with("spotify:artist:") {
        return Some(CrateRef::Artist {
            uri: Arc::from(uri),
            name: String::new(),
        });
    }
    if let Some(query) = uri.strip_prefix("spotify:search:") {
        return Some(CrateRef::Search {
            query: query.to_string(),
        });
    }
    None
}

fn spawn_crate_fetch(
    state: &BackendState,
    crate_ref: CrateRef,
    offset: usize,
    limit: usize,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    let Some(sess) = state.session.clone() else {
        return;
    };
    let session_gen = state.session_gen;
    let lib = Rc::clone(&state.library);
    let web_tok = Rc::clone(&state.web_api_token);
    let paths = state.paths.clone();
    let recent = state.recent_history.clone();
    let sort = state.sort_order;
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let mut lib_guard = lib.lock().await;
        let mut tok_guard = web_tok.lock().await;
        let result = lib_guard
            .fetch_crate_page(
                &sess,
                &paths,
                &mut tok_guard,
                &recent,
                &crate_ref,
                offset,
                limit,
                sort,
            )
            .await;
        let _ = tx.send(InternalMsg::BackendEventsBatch {
            session_gen,
            result,
        });
    });
}

async fn handle_internal_msg(
    state: &mut BackendState,
    msg: InternalMsg,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match msg {
        InternalMsg::LibrespotOAuthFinished { attempt_id, result } => {
            if attempt_id != state.oauth_attempt_id {
                return;
            }
            state.oauth_cancel = None;
            match result {
                Ok(tok) => {
                    let creds = Credentials::with_access_token(&tok.access_token);
                    spawn_session_connect(state, creds, false, int_tx, emitter);
                }
                Err(err) => {
                    state.pending_web_api_oauth = None;
                    if !matches!(state.auth_state, AuthState::Authenticated { .. }) {
                        state.auth_state = if state.config.zeroconf_enabled {
                            AuthState::ConnectOnly {
                                device_name: state.config.device_name.clone(),
                            }
                        } else {
                            AuthState::Unauthenticated
                        };
                        emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                    }
                    emitter.error(err);
                }
            }
        }
        InternalMsg::WebApiOAuthFinished { attempt_id, result } => {
            if attempt_id != state.web_oauth_attempt_id {
                return;
            }
            state.web_oauth_cancel = None;
            match result {
                Ok(token) => {
                    if let Err(err) = save_web_api_token(&state.paths, &token) {
                        emitter.error(format!("Failed to save Web API token: {err}"));
                    } else {
                        *state.web_api_token.lock().await = Some(token);
                    }
                    let web_ready = state.web_api_token.lock().await.is_some();
                    if let AuthState::Authenticated {
                        username,
                        display_name,
                        tier,
                        zeroconf_only,
                        ..
                    } = &state.auth_state
                    {
                        state.auth_state = AuthState::Authenticated {
                            username: username.clone(),
                            display_name: display_name.clone(),
                            tier: *tier,
                            web_api_ready: web_ready,
                            zeroconf_only: *zeroconf_only,
                        };
                        emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                        emitter.emit(BackendEvent::Notice(Some(
                            "Spotify Web API authorized.".to_string(),
                        )));
                    } else {
                        state.auth_state = if state.config.zeroconf_enabled {
                            AuthState::ConnectOnly {
                                device_name: state.config.device_name.clone(),
                            }
                        } else {
                            AuthState::Unauthenticated
                        };
                        emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                        emitter.emit(BackendEvent::Notice(Some(
                            "Saved Web API token. Click Authorize to sign in with Spotify streaming."
                                .to_string(),
                        )));
                    }
                }
                Err(err) => {
                    if !matches!(state.auth_state, AuthState::Authenticated { .. }) {
                        state.auth_state = if state.config.zeroconf_enabled {
                            AuthState::ConnectOnly {
                                device_name: state.config.device_name.clone(),
                            }
                        } else {
                            AuthState::Unauthenticated
                        };
                        emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                    }
                    emitter.error(err);
                }
            }
        }
        InternalMsg::DiscoveryCredentials(creds) => {
            if state.session.is_none() && !matches!(state.conn_status, ConnectionStatus::Connecting)
            {
                spawn_session_connect(state, creds, true, int_tx, emitter);
            }
        }
        InternalMsg::DiscoveryError(err) => {
            emitter.emit(BackendEvent::Notice(Some(format!(
                "Zeroconf discovery error: {err}"
            ))));
        }
        InternalMsg::SessionConnectFailed { session_gen, error } => {
            if session_gen != state.session_gen {
                return;
            }
            let cached_playable = state.refresh_offline_capability();
            state.conn_status = ConnectionStatus::Offline {
                retry_in_secs: 15,
                cached_playable,
            };
            emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
            emitter.error(error);

            if state.retry_creds.is_some() {
                let tx = int_tx.clone();
                tokio::task::spawn_local(async move {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                    let _ = tx.send(InternalMsg::ReconnectTimerFired { session_gen });
                });
            }
        }
        InternalMsg::ReconnectTimerFired { session_gen } => {
            if session_gen == state.session_gen
                && state.session.is_none()
                && matches!(state.conn_status, ConnectionStatus::Offline { .. })
                && let Some((creds, zeroconf_only)) = state.retry_creds.clone()
            {
                spawn_session_connect(state, creds, zeroconf_only, int_tx, emitter);
            }
        }
        InternalMsg::SessionConnected {
            session_gen,
            outcome,
            zeroconf_only,
        } => {
            if session_gen != state.session_gen {
                if let ConnectOutcome::Premium {
                    session,
                    local_playback,
                    ..
                } = outcome
                {
                    local_playback.shutdown().await;
                    session.shutdown();
                }
                return;
            }
            teardown_session(state).await;
            let web_ready = state.web_api_token.lock().await.is_some();
            match outcome {
                ConnectOutcome::NonPremium {
                    username,
                    account_type,
                } => {
                    state.retry_creds = None;
                    state.auth_state = AuthState::Authenticated {
                        username: username.clone(),
                        display_name: username,
                        tier: AccountTier::Free,
                        web_api_ready: web_ready,
                        zeroconf_only,
                    };
                    state.conn_status = ConnectionStatus::Error(
                        "Spotify Premium required for direct streaming".to_string(),
                    );
                    emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                    emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
                    emitter.emit(BackendEvent::Notice(Some(format!(
                        "Signed in to Free account ({account_type}). Spotify Premium is required for direct streaming & Connect playback."
                    ))));
                }
                ConnectOutcome::Premium {
                    session,
                    username,
                    reusable_creds,
                    mut local_playback,
                } => {
                    state.auth_meta.zeroconf_only = zeroconf_only;
                    let _ = save_auth_meta(&state.paths, &state.auth_meta);
                    state.current_creds = Some(reusable_creds.clone());
                    state.retry_creds = Some((reusable_creds, zeroconf_only));
                    state.session = Some(session.clone());
                    state.conn_status = ConnectionStatus::Connected;
                    let display_name = state
                        .auth_meta
                        .display_name
                        .clone()
                        .unwrap_or_else(|| username.clone());
                    state.auth_state = AuthState::Authenticated {
                        username: username.clone(),
                        display_name,
                        tier: AccountTier::Premium,
                        web_api_ready: web_ready,
                        zeroconf_only,
                    };
                    emitter.emit(BackendEvent::Notice(None));
                    emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
                    emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));

                    let events = local_playback.player.get_player_event_channel();
                    let cluster_stream = local_playback.cluster_stream.take();
                    state.local_playback = Some(*local_playback);
                    spawn_player_event_loop(state, events, int_tx);
                    if let Some(stream) = cluster_stream {
                        spawn_cluster_listener(state, stream, int_tx);
                    }

                    state.devices_state = DevicesState {
                        active_device_id: None,
                        local_device_id: state.device_id.clone(),
                        local_device_name: state.config.device_name.clone(),
                        zeroconf_active: state.config.zeroconf_enabled,
                        available_devices: vec![DeviceInfo {
                            id: state.device_id.clone(),
                            name: state.config.device_name.clone(),
                            device_type: "Computer".to_string(),
                            is_active: false,
                            is_local: true,
                            volume: state.player_state.volume,
                            supports_volume: true,
                            is_restricted: false,
                        }],
                    };
                    state.emit_devices(emitter);

                    let lib = Rc::clone(&state.library);
                    let tx = int_tx.clone();
                    tokio::task::spawn_local(async move {
                        let name = lib.lock().await.fetch_display_name(&session, &username).await;
                        if let Some(dn) = name {
                            let _ = tx.send(InternalMsg::DisplayNameLoaded {
                                session_gen,
                                name: dn,
                            });
                        }
                    });

                    state.current_crate = CrateRef::LikedSongs;
                    spawn_crate_fetch(state, CrateRef::LikedSongs, 0, 50, int_tx);

                    if !zeroconf_only
                        && let Some(cid) = state.pending_web_api_oauth.take()
                    {
                        start_web_api_oauth(state, cid, int_tx, emitter);
                    }
                }
            }
        }
        InternalMsg::DisplayNameLoaded { session_gen, name } => {
            if session_gen != state.session_gen {
                return;
            }
            state.auth_meta.display_name = Some(name.clone());
            let _ = save_auth_meta(&state.paths, &state.auth_meta);
            if let AuthState::Authenticated {
                username,
                tier,
                web_api_ready,
                zeroconf_only,
                ..
            } = &state.auth_state
            {
                state.auth_state = AuthState::Authenticated {
                    username: username.clone(),
                    display_name: name,
                    tier: *tier,
                    web_api_ready: *web_api_ready,
                    zeroconf_only: *zeroconf_only,
                };
                emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
            }
        }
        InternalMsg::ClusterReceived(cluster) => {
            let mut devices = devices_from_cluster(
                &cluster,
                &state.device_id,
                &state.config.device_name,
                state.config.zeroconf_enabled,
            );
            let cluster_local_active =
                devices.active_device_id.as_deref() == Some(state.device_id.as_str());
            let remote_active_id = devices
                .active_device_id
                .as_deref()
                .filter(|id| *id != state.device_id.as_str());
            let was_local = state.local_is_active;

            if cluster_local_active {
                state.local_is_active = true;
                // Keep `expecting_remote_transfer_until` intact if a transfer was just requested
                // so an intermediate stale cluster listing the local device does not cancel it.
            } else if !state.local_is_active {
                state.local_is_active = false;
                state.expecting_remote_transfer_until = None;
            } else if state.is_expecting_remote_transfer()
                || (remote_active_id.is_some()
                    && state.pending_local_load_uri.is_none()
                    && state.loading_track_uri.is_none())
            {
                state.local_is_active = false;
                state.local_queue_owned = false;
                state.expecting_remote_transfer_until = None;
                state.pending_local_load_uri = None;
                state.pending_local_load_paused = false;
                state.loading_track_uri = None;
                state.active_play_request_id = None;
                state.current_playing_uri = None;
                if was_local && let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                    let _ = lp.spirc.pause();
                }
            } else {
                // Local playback is actively loading or cluster has no remote active device:
                // do not let a stale/empty Cluster snapshot clobber local playback or queue.
                devices.active_device_id = Some(state.device_id.clone());
                for dev in &mut devices.available_devices {
                    dev.is_active = dev.is_local;
                }
            }

            state.devices_state = devices;
            state.emit_devices(emitter);

            state.remote_queue_meta = if state.local_is_active {
                None
            } else {
                extract_remote_queue_meta(&cluster)
            };

            if !state.local_is_active {
                let snap: RemotePlaybackSnapshot = snapshot_from_cluster(&cluster);
                state.player_state.is_playing = snap.is_playing;
                state.player_state.position_ms = snap.position_ms;
                state.player_state.duration_ms = snap.duration_ms;
                state.player_state.shuffle = snap.shuffle;
                state.player_state.repeat = snap.repeat;
                if let Some(v) = snap.active_device_volume {
                    state.player_state.volume = v;
                    state.player_state.muted = v == 0;
                }
                let active_summary = snap.active_track.as_ref().map(|t| t.summary.clone());
                state.player_state.active_track = snap.active_track;
                state.queue.apply_remote_snapshot(
                    snap.context_name,
                    snap.context_uri,
                    active_summary,
                    snap.manual_queue,
                    snap.context_queue,
                );
                state.emit_player(emitter);
                state.emit_queue(emitter);
                state.sync_mpris(false).await;
            }
        }
        InternalMsg::PlayerEvent(ev) => {
            handle_player_event(state, ev, int_tx, emitter).await;
        }
        InternalMsg::CurrentTrackDetailLoaded {
            session_gen,
            uri,
            detail,
        } => {
            if state.in_flight_detail_uri.as_deref() == Some(uri.as_str()) {
                state.in_flight_detail_uri = None;
            }
            let Some(detail) = detail else {
                return;
            };
            if session_gen != state.session_gen || state.session.is_none() {
                return;
            }
            let matches_current = state.current_playing_uri.as_deref() == Some(uri.as_str());
            let matches_active = state
                .player_state
                .active_track
                .as_ref()
                .is_some_and(|t| t.summary.uri.as_ref() == uri);
            if matches_current || matches_active {
                state.player_state.duration_ms = detail.summary.duration_ms;
                state.player_state.active_track = Some(detail.clone());
                state.emit_player(emitter);
                state.sync_mpris(false).await;

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                state.recent_history.record(RecentEntry {
                    uri: detail.summary.uri.to_string(),
                    title: detail.summary.title,
                    subtitle: detail.summary.artist.to_string(),
                    cover_url: detail
                        .summary
                        .cover
                        .and_then(|c| c.url.map(|u| u.to_string())),
                    duration_ms: detail.summary.duration_ms,
                    played_at_secs: now,
                });
                let _ = save_recent_history(&state.paths, &state.recent_history);
            }
        }
        InternalMsg::BackendEventsBatch {
            session_gen,
            result,
        } => {
            if session_gen != state.session_gen || state.session.is_none() {
                return;
            }
            match result {
                Ok(events) => {
                    for ev in events {
                        match &ev {
                            BackendEvent::CrateTracksPageLoaded {
                                crate_ref,
                                offset,
                                total,
                                items,
                            } => {
                                if *offset == 0 {
                                    state.last_crate_tracks =
                                        Some((crate_ref.clone(), *total, items.clone()));
                                } else if let Some((cached_ref, cached_total, cached_items)) =
                                    state.last_crate_tracks.as_mut()
                                    && crate_refs_same_target(cached_ref, crate_ref)
                                {
                                    *cached_total = *total;
                                    if *offset <= cached_items.len() && cached_items.len() < 1000 {
                                        cached_items.truncate(*offset);
                                        let remaining = 1000 - cached_items.len();
                                        cached_items.extend(items.iter().take(remaining).cloned());
                                    }
                                }
                                if let Some(ref sess) = state.session {
                                    prestage_track_uris(
                                        sess,
                                        items.iter().take(2).map(|t| t.uri.to_string()).collect(),
                                    );
                                }
                            }
                            BackendEvent::ArtistDetailLoaded(detail) => {
                                state.last_artist_detail = Some(detail.clone());
                                if let Some(ref sess) = state.session {
                                    prestage_track_uris(
                                        sess,
                                        detail
                                            .top_tracks
                                            .iter()
                                            .take(2)
                                            .map(|t| t.uri.to_string())
                                            .collect(),
                                    );
                                }
                            }
                            BackendEvent::SearchResultsLoaded(search) => {
                                state.last_search_results = Some(search.clone());
                                if let Some(ref sess) = state.session {
                                    prestage_track_uris(
                                        sess,
                                        search
                                            .tracks
                                            .iter()
                                            .take(2)
                                            .map(|t| t.uri.to_string())
                                            .collect(),
                                    );
                                }
                            }
                            _ => {}
                        }
                        emitter.emit(ev);
                    }
                }
                Err(err) => emitter.error(err),
            }
        }
        InternalMsg::LikeMutationFinished(res) => match res {
            Ok((uri, is_liked)) => {
                if let Some(ref mut active) = state.player_state.active_track
                    && active.summary.uri.as_ref() == uri
                {
                    active.summary.is_liked = is_liked;
                    state.emit_player(emitter);
                }
                emitter.emit(BackendEvent::LikeStateChanged { uri, is_liked });
            }
            Err(err) => emitter.error(err),
        },
        InternalMsg::FollowMutationFinished(res) => {
            if let Err(err) = res {
                emitter.error(err);
            }
        }
        InternalMsg::PlayCrateResolved {
            session_gen,
            play_seq,
            shuffle,
            result,
        } => {
            if session_gen != state.session_gen
                || play_seq != state.play_load_seq
                || state.session.is_none()
            {
                return;
            }
            apply_play_crate_resolved(state, shuffle, result, int_tx, emitter).await;
        }
        InternalMsg::QueueCrateResolved {
            session_gen,
            result,
        } => {
            if session_gen != state.session_gen || state.session.is_none() {
                return;
            }
            match result {
                Ok((_title, _ctx, tracks)) => {
                    for t in &tracks {
                        state.queue.append(t.clone());
                    }
                    state.emit_queue(emitter);
                    if !state.queue.manual_items().is_empty() {
                        sync_manual_queue_to_active_device(state).await;
                    }
                }
                Err(err) => emitter.error(err),
            }
        }
        InternalMsg::PlayTrackResolved {
            session_gen,
            play_seq,
            context_uri,
            track_uri,
            hint_index,
            tracks,
        } => {
            if session_gen != state.session_gen
                || play_seq != state.play_load_seq
                || state.session.is_none()
            {
                return;
            }
            apply_play_track_resolved(
                state,
                context_uri,
                track_uri,
                hint_index,
                tracks,
                int_tx,
                emitter,
            )
            .await;
        }
        InternalMsg::ExecuteStagedLocalLoad {
            session_gen,
            seq,
            request,
            sync_queue,
        } => {
            if session_gen == state.session_gen
                && seq == state.play_load_seq
                && state.is_remote_active().is_none()
                && let Some(ref lp) = state.local_playback
            {
                if state.pending_local_load_paused {
                    state.pending_local_load_paused = false;
                    state.pending_local_load_uri = None;
                    state.loading_track_uri = None;
                    state.local_player_stopped = true;
                    state.player_state.is_playing = false;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
                    return;
                }
                lp.request_sink_flush();
                state.local_player_stopped = false;
                state.local_spirc_queue_dirty = false;
                let _ = lp.spirc.activate();
                let _ = lp.spirc.load(request);
                if sync_queue && !state.queue.manual_items().is_empty() {
                    sync_manual_queue_to_active_device(state).await;
                }
            }
        }
        InternalMsg::UnavailableRetryReady { session_gen, uri } => {
            if session_gen == state.session_gen
                && state.is_remote_active().is_none()
                && state.session.is_some()
                && state.current_playing_uri.as_deref() == Some(uri.as_str())
            {
                if is_track_staged_in_memory(&uri) {
                    let loaded_now = load_tracks_on_active_device(
                        state,
                        vec![uri.clone()],
                        Some(uri),
                        false,
                        int_tx,
                        emitter,
                    )
                    .await;
                    if loaded_now && state.is_remote_active().is_some() {
                        sync_manual_queue_to_active_device(state).await;
                    }
                } else {
                    let max_skips = state
                        .queue
                        .total_context_len()
                        .saturating_add(state.queue.manual_items().len())
                        .max(1);
                    state.consecutive_unavailable_skips =
                        state.consecutive_unavailable_skips.saturating_add(1);
                    let wrap = state.player_state.repeat == RepeatMode::Context;
                    if state.consecutive_unavailable_skips < max_skips
                        && let Some(next) = state.queue.step_next_track(wrap)
                        && next.uri.as_ref() != uri
                    {
                        state.emit_queue(emitter);
                        set_optimistic_playing_track(state, &next, int_tx, emitter);
                        load_tracks_on_active_device(
                            state,
                            vec![next.uri.to_string()],
                            Some(next.uri.to_string()),
                            false,
                            int_tx,
                            emitter,
                        )
                        .await;
                    } else {
                        state.pending_local_load_uri = None;
                        state.pending_local_load_paused = false;
                        state.loading_track_uri = None;
                        state.local_player_stopped = true;
                        state.player_state.is_playing = false;
                        state.consecutive_unavailable_skips = 0;
                        state.emit_player(emitter);
                        state.sync_mpris(false).await;
                        emitter.error(format!("Track unavailable: {uri}"));
                    }
                }
            }
        }
        InternalMsg::CoverFetched { reply, result } => {
            let _ = reply.send(result);
        }
    }
}

fn spawn_cluster_listener(
    state: &mut BackendState,
    mut stream: ClusterStream,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    if let Some(h) = state.cluster_task.take() {
        h.abort();
    }
    let tx = int_tx.clone();
    state.cluster_task = Some(tokio::task::spawn_local(async move {
        while let Some(item) = stream.next().await {
            if let Ok(update) = item
                && let Some(cluster) = update.cluster.into_option()
            {
                let _ = tx.send(InternalMsg::ClusterReceived(cluster));
            }
        }
    }));
}

fn spawn_player_event_loop(
    state: &mut BackendState,
    mut events: spotatui_librespot_playback::player::PlayerEventChannel,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    if let Some(h) = state.player_ev_task.take() {
        h.abort();
    }
    let tx = int_tx.clone();
    state.player_ev_task = Some(tokio::task::spawn_local(async move {
        while let Some(ev) = events.recv().await {
            let _ = tx.send(InternalMsg::PlayerEvent(ev));
        }
    }));
}

async fn handle_player_event(
    state: &mut BackendState,
    ev: PlayerEvent,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match ev {
        PlayerEvent::Loading {
            play_request_id,
            track_id,
            position_ms,
        } => {
            let uri_str = track_id.to_uri();
            if state
                .pending_local_load_uri
                .as_deref()
                .is_some_and(|p| p != uri_str)
            {
                return;
            }
            if state.pending_local_load_uri.is_none()
                && state.queue.current_uri() != Some(uri_str.as_str())
                && state.queue.manual_items().is_empty()
                && state.queue.total_context_len() == 0
            {
                state.local_queue_owned = false;
            }
            state.pending_local_load_uri = None;
            state.active_play_request_id = Some(play_request_id);
            state.local_is_active = true;
            state.local_player_stopped = false;
            state.player_state.position_ms = position_ms;
            state.loading_track_uri = Some(uri_str.clone());
            state.queue.notify_track_started(&uri_str);
            sync_active_track_for_uri(state, &uri_str, None, int_tx);
            state.emit_player(emitter);
            state.emit_queue(emitter);
        }
        PlayerEvent::Playing {
            play_request_id,
            track_id,
            position_ms,
        } => {
            let uri_str = track_id.to_uri();
            if state
                .pending_local_load_uri
                .as_deref()
                .is_some_and(|p| p != uri_str)
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            if state.pending_local_load_uri.as_deref() == Some(uri_str.as_str()) {
                state.pending_local_load_uri = None;
            }
            if state.active_play_request_id.is_none() {
                state.active_play_request_id = Some(play_request_id);
            }
            state.local_is_active = true;
            state.local_player_stopped = false;
            state.loading_track_uri = None;
            state.consecutive_unavailable_skips = 0;
            state.player_state.is_playing = true;
            state.player_state.position_ms = position_ms;
            let seeked = std::mem::take(&mut state.pending_seek_emit);
            state.queue.notify_track_started(&uri_str);
            sync_active_track_for_uri(state, &uri_str, None, int_tx);
            state.emit_player(emitter);
            state.emit_queue(emitter);
            state.sync_mpris(seeked).await;
        }
        PlayerEvent::Paused {
            play_request_id,
            position_ms,
            ..
        } => {
            if state.pending_local_load_uri.is_some()
                || state.loading_track_uri.is_some()
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            state.local_player_stopped = false;
            state.player_state.is_playing = false;
            state.player_state.position_ms = position_ms;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
        }
        PlayerEvent::Stopped { play_request_id, .. } => {
            if state.pending_local_load_uri.is_some()
                || state.loading_track_uri.is_some()
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            state.local_player_stopped = true;
            state.loading_track_uri = None;
            state.player_state.is_playing = false;
            state.player_state.position_ms = 0;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
        }
        PlayerEvent::EndOfTrack {
            play_request_id,
            track_id,
        } => {
            if state.pending_local_load_uri.is_some()
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            state.loading_track_uri = None;
            if state.is_remote_active().is_none() {
                if !state.local_queue_owned {
                    // External Spotify Connect / Zeroconf playback owns Spirc's queue;
                    // allow Spirc to advance to its next track without pausing it.
                    return;
                }
                if state.player_state.repeat == RepeatMode::Track {
                    let cur_uri = state
                        .current_playing_uri
                        .clone()
                        .or_else(|| state.queue.current_uri().map(ToOwned::to_owned))
                        .unwrap_or_else(|| track_id.to_uri());
                    state.player_state.position_ms = 0;
                    state.player_state.is_playing = true;
                    state.emit_player(emitter);
                    load_tracks_on_active_device(
                        state,
                        vec![cur_uri.clone()],
                        Some(cur_uri),
                        false,
                        int_tx,
                        emitter,
                    )
                    .await;
                    return;
                }
                let wrap = state.player_state.repeat == RepeatMode::Context;
                if let Some(next) = state.queue.step_next_track(wrap) {
                    state.emit_queue(emitter);
                    set_optimistic_playing_track(state, &next, int_tx, emitter);
                    load_tracks_on_active_device(
                        state,
                        vec![next.uri.to_string()],
                        Some(next.uri.to_string()),
                        false,
                        int_tx,
                        emitter,
                    )
                    .await;
                } else {
                    if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.pause();
                    }
                    state.local_player_stopped = true;
                    state.local_spirc_queue_dirty = false;
                    state.player_state.is_playing = false;
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
                }
            }
        }
        PlayerEvent::TrackChanged { audio_item } => {
            let uri_str = audio_item.track_id.to_uri();
            if state
                .pending_local_load_uri
                .as_deref()
                .is_some_and(|p| p != uri_str)
            {
                return;
            }
            if state.pending_local_load_uri.is_none()
                && state.queue.current_uri() != Some(uri_str.as_str())
                && state.queue.manual_items().is_empty()
                && state.queue.total_context_len() == 0
            {
                state.local_queue_owned = false;
            }
            state.pending_local_load_uri = None;
            state.local_is_active = true;
            state.local_player_stopped = false;
            state.loading_track_uri = None;
            state.retried_unavailable_uri = None;
            state.player_state.duration_ms = audio_item.duration_ms;
            state.queue.notify_track_started(&uri_str);
            sync_active_track_for_uri(state, &uri_str, Some(&audio_item), int_tx);
            state.emit_player(emitter);
            state.emit_queue(emitter);
            state.sync_mpris(false).await;
            prestage_upcoming_tracks(state);
        }
        PlayerEvent::PositionChanged {
            play_request_id,
            position_ms,
            ..
        } => {
            if state.pending_local_load_uri.is_some()
                || state.loading_track_uri.is_some()
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            state.player_state.position_ms = position_ms;
            if let Some(ref mpris) = state.mpris {
                mpris.update_position(position_ms);
            }
            emitter.emit(BackendEvent::PositionTick { position_ms });
        }
        PlayerEvent::Seeked {
            play_request_id,
            position_ms,
            ..
        }
        | PlayerEvent::PositionCorrection {
            play_request_id,
            position_ms,
            ..
        } => {
            if state.pending_local_load_uri.is_some()
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            state.player_state.position_ms = position_ms;
            state.pending_seek_emit = false;
            state.emit_player(emitter);
            state.sync_mpris(true).await;
        }
        PlayerEvent::VolumeChanged { volume } => {
            state.player_state.volume = volume;
            state.player_state.muted = volume == 0;
            if volume > 0 {
                state.pre_mute_volume = volume;
            }
            state.emit_player(emitter);
            state.sync_mpris(false).await;
        }
        PlayerEvent::ShuffleChanged { shuffle } => {
            if state.is_remote_active().is_none() && state.local_queue_owned {
                return;
            }
            state.player_state.shuffle = shuffle;
            state.emit_player(emitter);
            state.sync_mpris(false).await;
        }
        PlayerEvent::RepeatChanged { context, track } => {
            if state.is_remote_active().is_none() && state.local_queue_owned {
                return;
            }
            state.player_state.repeat = if track {
                RepeatMode::Track
            } else if context {
                RepeatMode::Context
            } else {
                RepeatMode::Off
            };
            state.emit_player(emitter);
            state.sync_mpris(false).await;
        }
        PlayerEvent::Unavailable {
            play_request_id,
            track_id,
        } => {
            let uri_str = track_id.to_uri();
            if state
                .pending_local_load_uri
                .as_deref()
                .is_some_and(|p| p != uri_str)
                || state
                    .active_play_request_id
                    .is_some_and(|id| id != play_request_id)
            {
                return;
            }
            let is_active_uri = state.current_playing_uri.as_deref() == Some(uri_str.as_str())
                || state.loading_track_uri.as_deref() == Some(uri_str.as_str());
            if !is_active_uri {
                // Never interrupt the currently playing track when an upcoming preload fails.
                if let Some(ref sess) = state.session {
                    prestage_track_uris(sess, vec![uri_str]);
                }
                return;
            }
            if state.is_remote_active().is_none()
                && state.retried_unavailable_uri.as_deref() != Some(uri_str.as_str())
                && let Some(sess) = state.session.clone()
            {
                state.retried_unavailable_uri = Some(uri_str.clone());
                unmark_track_staged_in_memory(&uri_str);
                let session_gen = state.session_gen;
                let tx = int_tx.clone();
                tokio::task::spawn_local(async move {
                    ensure_track_playable_in_cache(&sess, &uri_str).await;
                    let _ = tx.send(InternalMsg::UnavailableRetryReady {
                        session_gen,
                        uri: uri_str,
                    });
                });
                return;
            }
            let max_skips = state
                .queue
                .total_context_len()
                .saturating_add(state.queue.manual_items().len())
                .max(1);
            state.consecutive_unavailable_skips =
                state.consecutive_unavailable_skips.saturating_add(1);
            let wrap = state.player_state.repeat == RepeatMode::Context;
            if state.is_remote_active().is_none()
                && state.local_queue_owned
                && state.consecutive_unavailable_skips < max_skips
                && let Some(next) = state.queue.step_next_track(wrap)
                && next.uri.as_ref() != uri_str
            {
                state.emit_queue(emitter);
                set_optimistic_playing_track(state, &next, int_tx, emitter);
                load_tracks_on_active_device(
                    state,
                    vec![next.uri.to_string()],
                    Some(next.uri.to_string()),
                    false,
                    int_tx,
                    emitter,
                )
                .await;
            } else {
                state.pending_local_load_uri = None;
                state.pending_local_load_paused = false;
                state.loading_track_uri = None;
                state.local_player_stopped = true;
                state.player_state.is_playing = false;
                state.consecutive_unavailable_skips = 0;
                state.emit_player(emitter);
                state.sync_mpris(false).await;
                emitter.error(format!("Track unavailable: {uri_str}"));
            }
        }
        _ => {}
    }
}

fn crate_refs_same_target(a: &CrateRef, b: &CrateRef) -> bool {
    match (a, b) {
        (CrateRef::LikedSongs, CrateRef::LikedSongs)
        | (CrateRef::PlaylistsIndex, CrateRef::PlaylistsIndex)
        | (CrateRef::AlbumsIndex, CrateRef::AlbumsIndex)
        | (CrateRef::ArtistsIndex, CrateRef::ArtistsIndex)
        | (CrateRef::Recent, CrateRef::Recent) => true,
        (CrateRef::Playlist { uri: u1, .. }, CrateRef::Playlist { uri: u2, .. })
        | (CrateRef::Album { uri: u1, .. }, CrateRef::Album { uri: u2, .. })
        | (CrateRef::Artist { uri: u1, .. }, CrateRef::Artist { uri: u2, .. }) => u1 == u2,
        (CrateRef::Search { query: q1 }, CrateRef::Search { query: q2 }) => q1 == q2,
        _ => false,
    }
}

fn cached_crate_title_and_ctx(crate_ref: &CrateRef) -> (String, Option<String>) {
    match crate_ref {
        CrateRef::LikedSongs => ("Liked Songs".to_string(), None),
        CrateRef::Recent => ("Recently Played".to_string(), None),
        CrateRef::Playlist { uri, name } => (
            if name.is_empty() {
                uri.to_string()
            } else {
                name.clone()
            },
            Some(uri.to_string()),
        ),
        CrateRef::Album { uri, title } => (
            if title.is_empty() {
                uri.to_string()
            } else {
                title.clone()
            },
            Some(uri.to_string()),
        ),
        CrateRef::Artist { uri, name } => (
            if name.is_empty() {
                uri.to_string()
            } else {
                name.clone()
            },
            Some(uri.to_string()),
        ),
        CrateRef::Search { query } => (format!("Search: {query}"), None),
        _ => ("Collection".to_string(), None),
    }
}

fn cached_tracks_for_crate(
    state: &BackendState,
    crate_ref: &CrateRef,
    required_track_uri: Option<&str>,
) -> Option<(String, Option<String>, Vec<pulse_model::TrackSummary>)> {
    if let Some((cached_ref, total, tracks)) = state.last_crate_tracks.as_ref()
        && crate_refs_same_target(cached_ref, crate_ref)
        && !tracks.is_empty()
    {
        // Only use the page cache shortcut when the full collection is loaded so playing a track
        // from page 0 of a >50-track playlist/Liked Songs does not truncate the queue/shuffle pool.
        let complete = tracks.len() >= *total
            && required_track_uri.is_none_or(|u| tracks.iter().any(|t| t.uri.as_ref() == u));
        if complete {
            let (title, ctx_uri) = cached_crate_title_and_ctx(cached_ref);
            return Some((title, ctx_uri, tracks.clone()));
        }
    }
    if let CrateRef::Artist { uri, .. } = crate_ref
        && let Some(detail) = state.last_artist_detail.as_ref()
        && detail.artist.uri == *uri
        && !detail.top_tracks.is_empty()
        && required_track_uri.is_none_or(|u| detail.top_tracks.iter().any(|t| t.uri.as_ref() == u))
    {
        return Some((
            detail.artist.name.clone(),
            Some(detail.artist.uri.to_string()),
            detail.top_tracks.clone(),
        ));
    }
    if let CrateRef::Search { query } = crate_ref
        && let Some(search) = state.last_search_results.as_ref()
        && search.query == *query
        && !search.tracks.is_empty()
        && required_track_uri.is_none_or(|u| search.tracks.iter().any(|t| t.uri.as_ref() == u))
    {
        return Some((format!("Search: {query}"), None, search.tracks.clone()));
    }
    None
}

fn find_cached_track_list_for_uri(
    state: &BackendState,
    track_uri: &str,
) -> Option<Vec<pulse_model::TrackSummary>> {
    if let Some(search) = state.last_search_results.as_ref()
        && search.tracks.iter().any(|t| t.uri.as_ref() == track_uri)
    {
        return Some(search.tracks.clone());
    }
    if let Some((_, total, tracks)) = state.last_crate_tracks.as_ref()
        && tracks.len() >= *total
        && tracks.iter().any(|t| t.uri.as_ref() == track_uri)
    {
        return Some(tracks.clone());
    }
    if let Some(detail) = state.last_artist_detail.as_ref()
        && detail.top_tracks.iter().any(|t| t.uri.as_ref() == track_uri)
    {
        return Some(detail.top_tracks.clone());
    }
    None
}

async fn apply_play_crate_resolved(
    state: &mut BackendState,
    shuffle: bool,
    result: Result<(String, Option<String>, Vec<pulse_model::TrackSummary>), String>,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match result {
        Ok((title, maybe_ctx_uri, tracks)) => {
            if tracks.is_empty() {
                emitter.error("This collection has no playable tracks.");
                return;
            }
            let is_local = state.is_remote_active().is_none();
            state.player_state.shuffle = shuffle;
            let ctx_arc = maybe_ctx_uri.as_deref().map(Arc::from);
            if is_local && shuffle {
                // Shuffle the full context BEFORE selecting track 0 so Shuffle Play starts on a
                // randomized track rather than always pinning tracks[0].
                state.queue.set_context(title, ctx_arc, tracks.clone(), None);
                state.queue.set_shuffle(true);
                let _ = state.queue.step_next_track(false);
            } else {
                state
                    .queue
                    .set_context(title, ctx_arc, tracks.clone(), Some(0));
            }
            state.emit_queue(emitter);
            let selected = state
                .queue
                .current_track()
                .cloned()
                .unwrap_or_else(|| tracks[0].clone());
            set_optimistic_playing_track(state, &selected, int_tx, emitter);

            let loaded_now = if !is_local
                && let Some(ctx_uri) = maybe_ctx_uri
            {
                let start_uri = if shuffle {
                    None
                } else {
                    Some(tracks[0].uri.to_string())
                };
                load_context_on_active_device(
                    state,
                    &ctx_uri,
                    start_uri,
                    shuffle,
                    int_tx,
                    emitter,
                )
                .await
            } else {
                let uris = if is_local {
                    state.queue.remaining_uris_from_current()
                } else {
                    tracks.iter().map(|t| t.uri.to_string()).collect()
                };
                let playing_uri = is_local.then(|| selected.uri.to_string());
                let device_shuffle = if is_local { false } else { shuffle };
                load_tracks_on_active_device(
                    state,
                    uris,
                    playing_uri,
                    device_shuffle,
                    int_tx,
                    emitter,
                )
                .await
            };
            if loaded_now && !state.queue.manual_items().is_empty() {
                sync_manual_queue_to_active_device(state).await;
            }
        }
        Err(err) => emitter.error(err),
    }
}

async fn apply_play_track_resolved(
    state: &mut BackendState,
    context_uri: String,
    track_uri: String,
    hint_index: Option<usize>,
    tracks: Result<Vec<pulse_model::TrackSummary>, String>,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match tracks {
        Ok(resolved) => {
            let play_idx = hint_index
                .filter(|&i| resolved.get(i).is_some_and(|t| t.uri.as_ref() == track_uri))
                .or_else(|| resolved.iter().position(|t| t.uri.as_ref() == track_uri));
            let has_real_ctx = !context_uri.is_empty()
                && context_uri != "spotify:collection:tracks"
                && context_uri != "liked"
                && context_uri != "recent";
            let ctx_arc = has_real_ctx.then(|| Arc::from(context_uri.as_str()));
            let title = if context_uri == "spotify:collection:tracks" || context_uri == "liked" {
                "Liked Songs".to_string()
            } else if context_uri == "recent" {
                "Recently Played".to_string()
            } else if context_uri.is_empty() {
                "Queue".to_string()
            } else if let Some(cref) = crate_ref_from_context_uri(&context_uri)
                && let Some((cached_ref, _, _)) = state.last_crate_tracks.as_ref()
                && crate_refs_same_target(cached_ref, &cref)
            {
                cached_crate_title_and_ctx(cached_ref).0
            } else if let Some(detail) = state.last_artist_detail.as_ref()
                && detail.artist.uri.as_ref() == context_uri
            {
                detail.artist.name.clone()
            } else {
                context_uri.clone()
            };
            let is_local = state.is_remote_active().is_none();
            state
                .queue
                .set_context(title, ctx_arc, resolved.clone(), play_idx);
            if is_local && state.player_state.shuffle {
                state.queue.set_shuffle(true);
            }
            state.emit_queue(emitter);
            if let Some(selected) = state
                .queue
                .current_track()
                .cloned()
                .or_else(|| play_idx.and_then(|i| resolved.get(i)).cloned())
            {
                set_optimistic_playing_track(state, &selected, int_tx, emitter);
            }

            let shuffle = state.player_state.shuffle;
            let loaded_now = if !is_local && has_real_ctx {
                load_context_on_active_device(
                    state,
                    &context_uri,
                    Some(track_uri),
                    shuffle,
                    int_tx,
                    emitter,
                )
                .await
            } else if is_local {
                let mut list = state.queue.remaining_uris_from_current();
                if list.is_empty() {
                    list.push(track_uri.clone());
                }
                let playing = state
                    .queue
                    .current_uri()
                    .map(ToOwned::to_owned)
                    .unwrap_or(track_uri);
                load_tracks_on_active_device(state, list, Some(playing), false, int_tx, emitter)
                    .await
            } else {
                let list = if resolved.is_empty() {
                    vec![track_uri.clone()]
                } else {
                    resolved.iter().map(|t| t.uri.to_string()).collect()
                };
                let playing = resolved
                    .iter()
                    .find(|t| t.uri.as_ref() == track_uri)
                    .map(|t| t.uri.to_string())
                    .unwrap_or(track_uri);
                load_tracks_on_active_device(state, list, Some(playing), shuffle, int_tx, emitter)
                    .await
            };
            if loaded_now && !state.queue.manual_items().is_empty() {
                sync_manual_queue_to_active_device(state).await;
            }
        }
        Err(err) => emitter.error(err),
    }
}

async fn start_play_crate(
    state: &mut BackendState,
    crate_ref: CrateRef,
    shuffle: bool,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    let Some(sess) = state.session.clone() else {
        emitter.error("Sign in with Spotify Premium first.");
        return;
    };
    state.play_load_seq = state.play_load_seq.wrapping_add(1);
    let play_seq = state.play_load_seq;
    let session_gen = state.session_gen;
    if let Some(cached) = cached_tracks_for_crate(state, &crate_ref, None) {
        apply_play_crate_resolved(state, shuffle, Ok(cached), int_tx, emitter).await;
        return;
    }
    let lib = Rc::clone(&state.library);
    let recent = state.recent_history.clone();
    let sort = state.sort_order;
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let result = lib
            .lock()
            .await
            .resolve_crate_tracks(&sess, &crate_ref, &recent, sort)
            .await;
        let _ = tx.send(InternalMsg::PlayCrateResolved {
            session_gen,
            play_seq,
            shuffle,
            result,
        });
    });
}

async fn play_queue_index_cmd(
    state: &mut BackendState,
    index: usize,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) -> bool {
    let Some((chosen, is_manual)) = state.queue.play_queue_index(index) else {
        return false;
    };
    let ctx_uri = state.queue.context_uri().map(ToOwned::to_owned);
    state.emit_queue(emitter);
    set_optimistic_playing_track(state, &chosen, int_tx, emitter);

    let loaded_now = if is_manual || ctx_uri.is_none() || state.is_remote_active().is_none() {
        let mut uris = Vec::with_capacity(1 + state.queue.context_upcoming().len().min(500));
        uris.push(chosen.uri.to_string());
        for t in state.queue.context_upcoming().iter().take(500) {
            uris.push(t.uri.to_string());
        }
        load_tracks_on_active_device(
            state,
            uris,
            Some(chosen.uri.to_string()),
            false,
            int_tx,
            emitter,
        )
        .await
    } else if let Some(ctx) = ctx_uri {
        let shuffle = state.player_state.shuffle;
        load_context_on_active_device(
            state,
            &ctx,
            Some(chosen.uri.to_string()),
            shuffle,
            int_tx,
            emitter,
        )
        .await
    } else {
        true
    };
    if loaded_now && !state.queue.manual_items().is_empty() {
        sync_manual_queue_to_active_device(state).await;
    }
    true
}

fn track_detail_from_summary(summary: &pulse_model::TrackSummary) -> TrackDetail {
    TrackDetail {
        summary: summary.clone(),
        track_number: None,
        disc_total_tracks: None,
        release_year: None,
        explicit: false,
    }
}

fn cached_track_summary(state: &BackendState, uri: &str) -> Option<pulse_model::TrackSummary> {
    if let Some(t) = state.queue.find_track(uri) {
        return Some(t.clone());
    }
    if let Some((_, _, tracks)) = state.last_crate_tracks.as_ref()
        && let Some(t) = tracks.iter().find(|t| t.uri.as_ref() == uri)
    {
        return Some(t.clone());
    }
    if let Some(detail) = state.last_artist_detail.as_ref()
        && let Some(t) = detail.top_tracks.iter().find(|t| t.uri.as_ref() == uri)
    {
        return Some(t.clone());
    }
    if let Some(search) = state.last_search_results.as_ref()
        && let Some(t) = search.tracks.iter().find(|t| t.uri.as_ref() == uri)
    {
        return Some(t.clone());
    }
    None
}

fn track_detail_from_audio_item(audio_item: &AudioItem) -> TrackDetail {
    let (artist, artist_uri, album) = match &audio_item.unique_fields {
        UniqueFields::Track { artists, album, .. } => {
            let names = artists
                .0
                .iter()
                .map(|a| a.name.as_str())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            let first_uri = artists.0.first().map(|a| Arc::from(a.id.to_uri().as_str()));
            (names, first_uri, album.as_str())
        }
        UniqueFields::Local { artists, album, .. } => (
            artists.clone().unwrap_or_default(),
            None,
            album.as_deref().unwrap_or(""),
        ),
        UniqueFields::Episode { show_name, .. } => (show_name.clone(), None, show_name.as_str()),
    };
    let cover = audio_item.covers.first().map(|c| {
        let id = c.url.rsplit('/').next().unwrap_or(audio_item.uri.as_str());
        pulse_model::ImageRef {
            id: Arc::from(id),
            url: Some(Arc::from(c.url.as_str())),
        }
    });
    TrackDetail {
        summary: pulse_model::TrackSummary {
            uri: Arc::from(audio_item.uri.as_str()),
            title: audio_item.name.clone(),
            artist: Arc::from(artist.as_str()),
            artist_uri,
            album: Arc::from(album),
            album_uri: None,
            cover,
            duration_ms: audio_item.duration_ms,
            is_liked: false,
            is_playable: audio_item.availability.is_ok(),
            added_at_unix: None,
        },
        track_number: None,
        disc_total_tracks: None,
        release_year: None,
        explicit: audio_item.is_explicit,
    }
}

fn sync_active_track_for_uri(
    state: &mut BackendState,
    uri_str: &str,
    maybe_audio_item: Option<&AudioItem>,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    state.current_playing_uri = Some(uri_str.to_string());
    let needs_replace = state
        .player_state
        .active_track
        .as_ref()
        .is_none_or(|t| t.summary.uri.as_ref() != uri_str);
    if needs_replace {
        if let Some(summary) = cached_track_summary(state, uri_str) {
            state.player_state.duration_ms = summary.duration_ms;
            state.player_state.active_track = Some(track_detail_from_summary(&summary));
        } else if let Some(item) = maybe_audio_item {
            state.player_state.duration_ms = item.duration_ms;
            state.player_state.active_track = Some(track_detail_from_audio_item(item));
        }
    }
    maybe_fetch_track_detail(state, uri_str, int_tx);
}

fn set_optimistic_playing_track(
    state: &mut BackendState,
    summary: &pulse_model::TrackSummary,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    state.local_is_active = state.is_remote_active().is_none();
    state.local_player_stopped = false;
    state.current_playing_uri = Some(summary.uri.to_string());
    state.queue.set_current_track(Some(summary.clone()));
    state.player_state.active_track = Some(track_detail_from_summary(summary));
    state.player_state.position_ms = 0;
    state.player_state.duration_ms = summary.duration_ms;
    state.player_state.is_playing = true;
    state.emit_player(emitter);
    maybe_fetch_track_detail(state, summary.uri.as_ref(), int_tx);
}

fn prestage_track_uris(session: &Session, uris: Vec<String>) {
    let mut seen = HashSet::new();
    let pending: Vec<String> = uris
        .into_iter()
        .filter(|u| !is_track_staged_in_memory(u) && seen.insert(u.clone()))
        .collect();
    if pending.is_empty() {
        return;
    }
    let sess = session.clone();
    tokio::task::spawn_local(async move {
        for uri in pending {
            ensure_track_playable_in_cache(&sess, &uri).await;
        }
    });
}

fn prestage_upcoming_tracks(state: &BackendState) {
    let Some(ref sess) = state.session else {
        return;
    };
    let uris: Vec<String> = state
        .queue
        .manual_items()
        .iter()
        .chain(state.queue.context_upcoming().iter())
        .take(2)
        .map(|t| t.uri.to_string())
        .collect();
    prestage_track_uris(sess, uris);
}

fn maybe_fetch_track_detail(
    state: &mut BackendState,
    uri_str: &str,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    let already_loaded = state
        .player_state
        .active_track
        .as_ref()
        .is_some_and(|t| {
            t.summary.uri.as_ref() == uri_str
                && (t.track_number.is_some() || t.release_year.is_some())
        });
    if already_loaded || state.in_flight_detail_uri.as_deref() == Some(uri_str) {
        return;
    }
    let Some(sess) = state.session.clone() else {
        return;
    };
    state.in_flight_detail_uri = Some(uri_str.to_string());
    let session_gen = state.session_gen;
    let lib = Rc::clone(&state.library);
    let uri = uri_str.to_string();
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let detail = lib.lock().await.resolve_track_detail(&sess, &uri).await;
        let _ = tx.send(InternalMsg::CurrentTrackDetailLoaded {
            session_gen,
            uri,
            detail,
        });
    });
}

fn enforce_audio_cache_limit(state: &BackendState) {
    // Match `open_librespot_cache`'s 16 MiB floor so `disk_audio_cache_mb == 0` does not
    // wipe active staged fallback files every 2 seconds.
    let effective_mb = state.config.disk_audio_cache_mb.max(16);
    let limit_bytes = effective_mb.saturating_mul(1024 * 1024);
    let removed = prune_dir_to_bytes_lru(&state.paths.audio_cache_dir, limit_bytes);
    if removed > 0 {
        clear_staged_tracks_in_memory();
    }
}

fn dispatch_local_load(
    state: &mut BackendState,
    target_uri: Option<&str>,
    request: LoadRequest,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) -> bool {
    let Some(ref lp) = state.local_playback else {
        return true;
    };
    state.play_load_seq = state.play_load_seq.wrapping_add(1);
    let seq = state.play_load_seq;
    let session_gen = state.session_gen;
    lp.request_sink_flush();
    state.local_queue_owned = true;
    state.local_player_stopped = false;
    state.local_spirc_queue_dirty = false;
    state.pending_local_load_uri = target_uri.map(ToOwned::to_owned);
    state.pending_local_load_paused = false;
    state.active_play_request_id = None;

    if let Some(target) = target_uri
        && !is_track_staged_in_memory(target)
        && let Some(sess) = state.session.clone()
    {
        // Stop/pause the old track right away so it does not play on while staging the new one.
        let _ = lp.spirc.pause();
        let target_owned = target.to_string();
        let sync_queue = false;
        let tx = int_tx.clone();
        tokio::task::spawn_local(async move {
            ensure_track_playable_in_cache(&sess, &target_owned).await;
            let _ = tx.send(InternalMsg::ExecuteStagedLocalLoad {
                session_gen,
                seq,
                request,
                sync_queue,
            });
        });
        prestage_upcoming_tracks(state);
        return false;
    }

    let _ = lp.spirc.activate();
    let _ = lp.spirc.load(request);
    prestage_upcoming_tracks(state);
    true
}

async fn load_context_on_active_device(
    state: &mut BackendState,
    ctx_uri: &str,
    track_uri: Option<String>,
    shuffle: bool,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) -> bool {
    if let Some(remote_id) = state.is_remote_active()
        && let Some(ref sess) = state.session
    {
        state.remote_queue_meta = None;
        let payload = cmd_play_context(
            ctx_uri,
            track_uri.as_deref(),
            shuffle,
            state.player_state.repeat,
        );
        if let Err(err) = send_remote_command(sess, &remote_id, payload).await {
            emitter.error(err);
        }
        return true;
    }
    if let Some(target) = track_uri {
        let request = build_tracks_load_request(
            vec![target.clone()],
            Some(PlayingTrack::Uri(target.clone())),
            false,
            RepeatMode::Off,
            true,
            0,
        );
        return dispatch_local_load(state, Some(&target), request, int_tx);
    }
    let request = build_context_load_request(
        ctx_uri,
        None,
        false,
        RepeatMode::Off,
        true,
        0,
    );
    dispatch_local_load(state, None, request, int_tx)
}

async fn load_tracks_on_active_device(
    state: &mut BackendState,
    uris: Vec<String>,
    playing_uri: Option<String>,
    shuffle: bool,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) -> bool {
    if uris.is_empty() && playing_uri.is_none() {
        return true;
    }
    if let Some(remote_id) = state.is_remote_active()
        && let Some(ref sess) = state.session
    {
        state.remote_queue_meta = None;
        let payload = cmd_play_tracks(
            &uris,
            playing_uri.as_deref(),
            shuffle,
            state.player_state.repeat,
        );
        if let Err(err) = send_remote_command(sess, &remote_id, payload).await {
            emitter.error(err);
        }
        return true;
    }
    // Single-track local mode: keep Spirc loaded with ONLY the active track so it never
    // preloads un-staged upcoming tracks (which fire spurious Unavailable mid-song) and never
    // fights `TwoTierQueue` on `EndOfTrack`.
    let Some(target) = playing_uri.or_else(|| uris.into_iter().next()) else {
        return true;
    };
    let request = build_tracks_load_request(
        vec![target.clone()],
        Some(PlayingTrack::Uri(target.clone())),
        false,
        RepeatMode::Off,
        true,
        0,
    );
    dispatch_local_load(state, Some(&target), request, int_tx)
}

async fn sync_manual_queue_to_active_device(state: &BackendState) {
    if let Some(remote_id) = state.is_remote_active()
        && let Some(ref sess) = state.session
        && let Some(ref meta) = state.remote_queue_meta
        && !meta.queue_revision.is_empty()
    {
        let manual_uris: Vec<String> = state
            .queue
            .manual_items()
            .iter()
            .map(|t| t.uri.to_string())
            .collect();
        let mut next_tracks: Vec<serde_json::Value> = manual_uris
            .iter()
            .enumerate()
            .map(|(i, u)| manual_track_to_provided_json(u, i))
            .collect();
        next_tracks.extend(meta.context_next_tracks.iter().cloned());
        let payload = cmd_set_queue(
            next_tracks,
            meta.prev_tracks.clone(),
            &meta.queue_revision,
        );
        let _ = send_remote_command(sess, &remote_id, payload).await;
    }
    // Local playback drives manual + context queue directly via `TwoTierQueue` on `EndOfTrack`.
}

async fn resolve_uris_via_track_api(
    session: &Session,
    uris: &[String],
) -> Result<Vec<pulse_model::TrackSummary>, String> {
    use spotatui_librespot_metadata::Metadata as _;
    let mut out = Vec::with_capacity(uris.len());
    for raw in uris {
        let Ok(parsed) = SpotifyUri::from_uri(raw) else {
            continue;
        };
        if let Ok(tr) = spotatui_librespot_metadata::Track::get(session, &parsed).await {
            let artists = tr
                .artists
                .0
                .iter()
                .map(|a| a.name.as_str())
                .filter(|n| !n.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            out.push(pulse_model::TrackSummary {
                uri: Arc::from(raw.as_str()),
                title: tr.name,
                artist: Arc::from(artists.as_str()),
                artist_uri: tr
                    .artists
                    .0
                    .first()
                    .map(|a| Arc::from(a.id.to_uri().as_str())),
                album: Arc::from(tr.album.name.as_str()),
                album_uri: Some(Arc::from(tr.album.id.to_uri().as_str())),
                cover: None,
                duration_ms: tr.duration.clamp(0, i32::MAX) as u32,
                is_liked: false,
                is_playable: true,
                added_at_unix: None,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_model::{NormalisationMode, StreamQuality, TrackSummary};
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("pulse-backend-test-{tag}-{nanos}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mk_track(id: &str) -> TrackSummary {
        TrackSummary {
            uri: Arc::from(format!("spotify:track:{id}").as_str()),
            title: format!("Title {id}"),
            artist: Arc::from("Artist"),
            artist_uri: None,
            album: Arc::from("Album"),
            album_uri: None,
            cover: None,
            duration_ms: 180_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
        }
    }

    #[test]
    fn config_round_trip_and_device_id_stability() {
        let root = temp_root("cfg");
        let paths = AppPaths::for_root(&root);
        let cfg = AppConfig {
            device_name: "Pulse Gentoo".to_string(),
            stream_quality: StreamQuality::Kbps160,
            normalisation: NormalisationMode::Track,
            disk_audio_cache_mb: 128,
            disk_cover_cache_mb: 16,
            ..Default::default()
        };
        save_config(&paths, &cfg).unwrap();

        let loaded = load_config(&paths);
        assert_eq!(loaded.device_name, "Pulse Gentoo");
        assert_eq!(loaded.stream_quality, StreamQuality::Kbps160);
        assert_eq!(loaded.normalisation, NormalisationMode::Track);
        assert_eq!(loaded.disk_audio_cache_mb, 128);
        assert_eq!(loaded.disk_cover_cache_mb, 16);

        let id1 = load_or_create_device_id(&paths);
        let id2 = load_or_create_device_id(&paths);
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 40);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_tier_queue_operations_preserve_priority_and_context() {
        let mut q = TwoTierQueue::new();
        q.set_context(
            "Focus Mix",
            Some(Arc::from("spotify:playlist:mix")),
            vec![mk_track("c0"), mk_track("c1"), mk_track("c2"), mk_track("c3")],
            Some(0),
        );
        assert_eq!(q.context_upcoming().len(), 3);
        assert_eq!(&*q.context_upcoming()[0].uri, "spotify:track:c1");

        q.append(mk_track("m1"));
        q.append(mk_track("m2"));
        q.enqueue_next(mk_track("m0"));
        assert_eq!(
            q.manual_items()
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Title m0", "Title m1", "Title m2"]
        );

        assert!(q.reorder_manual(2, 0));
        assert_eq!(
            q.manual_items()
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Title m2", "Title m0", "Title m1"]
        );

        let promoted = q.promote_context_to_manual(1, 3).unwrap();
        assert_eq!(&*promoted.uri, "spotify:track:c2");
        assert_eq!(q.context_upcoming().len(), 2);
        assert_eq!(q.manual_items().len(), 4);

        let (played, is_manual) = q.play_queue_index(1).unwrap();
        assert!(is_manual);
        assert_eq!(&*played.uri, "spotify:track:m0");
        assert_eq!(
            q.manual_items()
                .iter()
                .map(|t| &*t.uri)
                .collect::<Vec<_>>(),
            vec!["spotify:track:m1", "spotify:track:c2"]
        );

        q.remove_manual(0);
        assert_eq!(q.manual_items().len(), 1);
        q.clear_manual();
        assert!(q.manual_items().is_empty());
        assert_eq!(q.context_upcoming().len(), 2);
    }

    #[test]
    fn proc_status_parser_extracts_rss_and_hwm() {
        let sample = "\
Name:\tpulse-spotify\n\
State:\tS (sleeping)\n\
VmHWM:\t   34560 kB\n\
VmRSS:\t   28160 kB\n\
Threads:\t2\n";
        let info = crate::telemetry::parse_proc_status(sample);
        assert_eq!(info.hwm_bytes, 34_560 * 1024);
        assert_eq!(info.rss_bytes, 28_160 * 1024);
    }

    #[test]
    fn discovery_credential_conversion_preserves_fields() {
        let raw = librespot_discovery::Credentials {
            username: Some("alice".to_string()),
            auth_type: Default::default(),
            auth_data: vec![1, 2, 3, 4],
        };
        let converted = convert_discovery_credentials(raw).unwrap();
        assert_eq!(converted.username.as_deref(), Some("alice"));
        assert_eq!(converted.auth_data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn web_api_token_private_file_permissions() {
        let root = temp_root("token");
        let paths = AppPaths::for_root(&root);
        let token = WebApiToken {
            client_id: "cid123".to_string(),
            access_token: "acc".to_string(),
            refresh_token: Some("ref".to_string()),
            expires_at_epoch_secs: 999999999,
            scope: "user-library-read".to_string(),
        };
        save_web_api_token(&paths, &token).unwrap();
        let loaded = load_web_api_token(&paths).unwrap();
        assert_eq!(loaded, token);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&paths.web_api_token_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prune_dir_to_bytes_lru_evicts_oldest_files_first() {
        let root = temp_root("prune_lru");
        let sub = root.join("nested");
        fs::create_dir_all(&sub).unwrap();
        let old_file = root.join("old.bin");
        let mid_file = sub.join("mid.bin");
        let new_file = sub.join("new.bin");

        fs::write(&old_file, vec![1u8; 400]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&mid_file, vec![2u8; 400]).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&new_file, vec![3u8; 400]).unwrap();

        prune_dir_to_bytes_lru(&root, 800);
        assert!(!old_file.exists());
        assert!(mid_file.exists());
        assert!(new_file.exists());

        prune_dir_to_bytes_lru(&root, 0);
        assert!(!mid_file.exists());
        assert!(!new_file.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn play_manual_queue_index_preserves_remaining_context_order() {
        let mut q = TwoTierQueue::new();
        q.set_context(
            "Focus Mix",
            Some(Arc::from("spotify:playlist:mix")),
            vec![mk_track("c0"), mk_track("c1"), mk_track("c2")],
            Some(0),
        );
        q.append(mk_track("m0"));
        q.append(mk_track("m1"));

        let (chosen, is_manual) = q.play_queue_index(0).unwrap();
        assert!(is_manual);
        assert_eq!(&*chosen.uri, "spotify:track:m0");

        let mut uris = vec![chosen.uri.to_string()];
        for t in q.context_upcoming() {
            uris.push(t.uri.to_string());
        }
        assert_eq!(
            uris,
            vec![
                "spotify:track:m0",
                "spotify:track:c1",
                "spotify:track:c2",
            ]
        );
        assert_eq!(q.manual_items().len(), 1);
        assert_eq!(&*q.manual_items()[0].uri, "spotify:track:m1");
    }

    #[test]
    fn app_config_deserializes_partial_json_with_defaults() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"device_name":"Studio","gapless":false}"#).unwrap();
        assert_eq!(cfg.device_name, "Studio");
        assert!(!cfg.gapless);
        assert!(cfg.zeroconf_enabled);
        assert_eq!(cfg.stream_quality, StreamQuality::Kbps320);
        assert_eq!(cfg.disk_audio_cache_mb, 256);
    }
}
