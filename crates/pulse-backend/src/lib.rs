//! Multi-source (Local, Spotify, YouTube, SoundCloud) playback, DSP, lyrics, and MPRIS2 backend.

pub mod auth;
pub mod config;
pub mod connect;
pub mod direct_player;
pub mod dsp;
pub mod library;
pub mod local_lib;
pub mod lyrics;
pub mod mp3_export;
pub mod mpris;
pub mod player;
pub mod providers;
pub mod queue;
pub mod telemetry;
pub mod user_data;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use pulse_model::{
    AccountTier, AlbumSummary, AppConfig, ArtistDetail, AuthState, BackendEvent, ConnectionStatus,
    CrateRef, DeviceInfo, DevicesState, LibraryScope, LyricsState, MediaSource, PlayerState,
    PlaylistSummary, RepeatMode, SearchResults, SleepTimerTarget, SortOrder, SpectrumSnapshot,
    SponsorSegment, TrackDetail, TrackSummary, UiCommand, VisualizerMode,
};
use spotatui_librespot_connect::{LoadRequest, PlayingTrack};
use spotatui_librespot_core::Session;
use spotatui_librespot_core::authentication::Credentials;
use spotatui_librespot_core::config::SessionConfig;
use spotatui_librespot_core::http_client::HttpClient;
use spotatui_librespot_core::spotify_uri::SpotifyUri;
use spotatui_librespot_metadata::audio::{AudioItem, UniqueFields};
use spotatui_librespot_playback::mixer::Mixer as _;
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
    clear_auth_files, clear_web_api_token, is_dir_covered_by_roots, load_auth_meta, load_config,
    load_or_create_device_id, load_recent_history, load_web_api_token, open_librespot_cache,
    prune_dir_to_bytes_lru, save_auth_meta, save_config, save_recent_history, save_web_api_token,
};
use crate::connect::{
    RemotePlaybackSnapshot, RemoteQueueMeta, cmd_add_to_queue, cmd_play_context, cmd_play_tracks,
    cmd_seek_to, cmd_set_queue, cmd_set_repeat, cmd_set_shuffle, devices_from_cluster,
    extract_remote_queue_meta, manual_track_to_provided_json, refresh_cluster_passive,
    send_remote_command, set_remote_volume, snapshot_from_cluster, transfer_playback,
};
use crate::direct_player::{DirectAudioEngine, DirectEvent};
use crate::dsp::SharedDsp;
use crate::library::{LibraryService, fetch_cover_bytes, sort_track_summaries};
use crate::local_lib::{LocalIndexedTrack, LocalLibrary, is_audio_file};
use crate::lyrics::fetch_lyrics_for_track;
use crate::mp3_export::{
    build_sponsor_aselect_filter, resolve_unique_mp3_path, sanitize_filename_stem,
    transcode_and_tag_mp3_blocking,
};
use crate::mpris::MprisController;
use crate::player::{
    ClusterStream, LocalPlayback, audio_pipeline_changed, build_context_load_request,
    build_tracks_load_request, clear_staged_tracks_in_memory, ensure_track_playable_in_cache,
    is_track_staged_in_memory, purge_stale_encrypted_audio_cache, start_local_playback,
    stream_audio_metrics, unmark_track_staged_in_memory,
};
use crate::providers::soundcloud::{
    ScResolved, ensure_soundcloud_audio_cached, resolve_soundcloud_url, search_soundcloud,
};
use crate::providers::youtube::{
    ensure_youtube_audio_cached, fetch_youtube_cover_jpeg, fetch_youtube_skip_segments,
    resolve_youtube_playlist, resolve_youtube_video, search_youtube,
};
use crate::providers::{ResolvedInput, parse_direct_input};
use crate::queue::TwoTierQueue;
use crate::telemetry::{CoverStatsHook, collect_telemetry};
use crate::user_data::{
    CustomPlaylistsStore, ListeningStatsStore, SavedSessionState, UnifiedLikesStore,
    make_local_uri_summary, parse_m3u_entries,
};

/// Callback invoked whenever the backend pushes one or more [`BackendEvent`]s.
pub type WakeupCallback = Arc<dyn Fn() + Send + Sync>;

struct CoverFetchReq {
    url: String,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// Thread-safe handle for controlling the dedicated multi-source backend thread from the UI.
pub struct BackendHandle {
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
    cover_tx: mpsc::UnboundedSender<CoverFetchReq>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    cover_stats: CoverStatsHook,
    dsp: Arc<SharedDsp>,
    initial_config: AppConfig,
    thread: Option<JoinHandle<()>>,
}

impl BackendHandle {
    /// Spawns the single-threaded Tokio backend thread using default XDG paths.
    pub fn spawn(
        wakeup: WakeupCallback,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BackendEvent>), String> {
        Self::spawn_with_paths(AppPaths::from_xdg(), wakeup)
    }

    /// Spawns the backend thread using explicit filesystem paths.
    pub fn spawn_with_paths(
        paths: AppPaths,
        wakeup: WakeupCallback,
    ) -> Result<(Self, mpsc::UnboundedReceiver<BackendEvent>), String> {
        let initial_config = load_config(&paths);
        let dsp = Arc::new(SharedDsp::new(
            initial_config.eq.clone(),
            initial_config.visualizer_mode != VisualizerMode::Off,
        ));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cover_tx, cover_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let cover_stats = CoverStatsHook::new();
        let cover_stats_thread = cover_stats.clone();
        let internal_cmd_tx = cmd_tx.clone();
        let config_for_thread = initial_config.clone();
        let dsp_for_thread = Arc::clone(&dsp);

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
                    dsp_for_thread,
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
                dsp,
                initial_config,
                thread: Some(thread),
            },
            event_rx,
        ))
    }

    pub fn initial_config(&self) -> &AppConfig {
        &self.initial_config
    }

    pub fn snapshot_spectrum(&self) -> SpectrumSnapshot {
        self.dsp.snapshot_spectrum()
    }

    pub fn send(&self, cmd: UiCommand) -> Result<(), String> {
        self.cmd_tx
            .send(cmd)
            .map_err(|_| "backend actor is no longer running".to_string())
    }

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

    pub fn cover_stats_hook(&self) -> CoverStatsHook {
        self.cover_stats.clone()
    }

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
    DirectEvent(DirectEvent),
    DirectAudioStaged {
        seq: u64,
        track: TrackSummary,
        start_ms: u32,
        start_playing: bool,
        result: Result<(PathBuf, String), String>,
    },
    LocalScanFinished {
        tracks: Vec<LocalIndexedTrack>,
    },
    MultiSearchFinished {
        search_seq: u64,
        results: SearchResults,
    },
    UrlResolved {
        query: String,
        result: Result<UrlResolveOutcome, String>,
    },
    LyricsFinished {
        uri: String,
        state: LyricsState,
    },
    SponsorSegmentsLoaded {
        uri: String,
        segments: Vec<SponsorSegment>,
    },
    Mp3ExportFinished {
        uri: String,
        auto_index: bool,
        result: Result<PathBuf, String>,
    },
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
        result: Result<(String, Option<String>, Vec<TrackSummary>), String>,
    },
    QueueCrateResolved {
        session_gen: u64,
        result: Result<(String, Option<String>, Vec<TrackSummary>), String>,
    },
    PlayTrackResolved {
        session_gen: u64,
        play_seq: u64,
        context_uri: String,
        track_uri: String,
        hint_index: Option<usize>,
        tracks: Result<Vec<TrackSummary>, String>,
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

enum UrlResolveOutcome {
    SingleTrack(TrackSummary),
    Playlist {
        crate_ref: CrateRef,
        title: String,
        tracks: Vec<TrackSummary>,
    },
    SpotifyCrate(CrateRef),
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
    local_lib: LocalLibrary,
    likes_store: UnifiedLikesStore,
    custom_playlists: CustomPlaylistsStore,
    stats_store: ListeningStatsStore,
    dsp: Arc<SharedDsp>,
    direct_player: DirectAudioEngine,
    direct_seq: u64,
    active_direct_uri: Option<String>,
    search_seq: u64,
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
    last_crate_tracks: Option<(CrateRef, usize, Vec<TrackSummary>)>,
    last_artist_detail: Option<ArtistDetail>,
    last_search_results: Option<SearchResults>,
    in_flight_detail_uri: Option<String>,
    in_flight_lyrics_uri: Option<String>,
    current_lyrics: LyricsState,
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
    sleep_deadline: Option<Instant>,
    sleep_end_of_track: bool,
    active_play_counted: bool,
    last_ab_seek_at: Instant,
    active_sponsor_segments: Vec<SponsorSegment>,
    skipped_sponsor_starts: HashSet<u32>,
    last_sponsor_skip_at: Instant,
    ad_muted_restore_vol: Option<u16>,
    downloads_in_flight: HashSet<String>,
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
        self.player_state.sample_rate_hz = rate;
        self.player_state.bitrate_kbps = kbps;
        if self.player_state.active_track.is_none() || self.player_state.active_source == MediaSource::Spotify {
            self.player_state.audio_badge = badge;
        }
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

    fn emit_custom_playlists(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::CustomPlaylistsUpdated(
            self.custom_playlists.summaries(),
        ));
    }

    fn emit_local_status(&self, emitter: &Emitter, scanning: bool) {
        emitter.emit(BackendEvent::LocalLibraryStatus {
            track_count: self.local_lib.tracks.len(),
            dirs: self.config.local_music_dirs.clone(),
            scanning,
        });
    }

    fn emit_stats(&self, emitter: &Emitter) {
        emitter.emit(BackendEvent::ListeningStatsUpdated(
            self.stats_store.snapshot(),
        ));
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

    fn persist_session(&self) {
        let q = self.queue.to_queue_state();
        let snap = SavedSessionState {
            active_track: self
                .player_state
                .active_track
                .as_ref()
                .map(|t| t.summary.clone()),
            position_ms: self.player_state.position_ms,
            manual_items: q.manual_items,
            context_items: q.context_items,
            context_name: q.context_name,
            shuffle: self.player_state.shuffle,
            repeat: self.player_state.repeat,
        };
        snap.save(&self.paths);
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
            emitter.error("failed to open Aplayer cache directory");
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
        match start_local_playback(
            &new_session,
            creds.clone(),
            &self.config,
            old_vol,
            Arc::clone(&self.dsp),
        )
        .await
        {
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
    dsp: Arc<SharedDsp>,
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
    let local_lib = LocalLibrary::load_cached(&paths, config.local_music_dirs.clone());
    let likes_store = UnifiedLikesStore::load(&paths);
    let custom_playlists = CustomPlaylistsStore::load(&paths);
    let stats_store = ListeningStatsStore::load(&paths);

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
        .unwrap_or(52_428);

    let (direct_tx, mut direct_rx) = mpsc::unbounded_channel::<DirectEvent>();
    let direct_player = DirectAudioEngine::spawn(
        Arc::clone(&dsp),
        direct_tx,
        initial_vol,
        config.audio_device.clone(),
    );
    let direct_fwd_tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        while let Some(ev) = direct_rx.recv().await {
            let _ = direct_fwd_tx.send(InternalMsg::DirectEvent(ev));
        }
    });

    let mut state = BackendState {
        paths,
        config: config.clone(),
        device_id: device_id.clone(),
        auth_meta,
        web_api_token: Rc::new(tokio::sync::Mutex::new(web_api_token)),
        recent_history,
        local_lib,
        likes_store,
        custom_playlists,
        stats_store,
        dsp,
        direct_player,
        direct_seq: 0,
        active_direct_uri: None,
        search_seq: 0,
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
            playback_speed: config.playback_speed,
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
        local_queue_owned: true,
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
        current_crate: CrateRef::Home,
        last_crate_tracks: None,
        last_artist_detail: None,
        last_search_results: None,
        in_flight_detail_uri: None,
        in_flight_lyrics_uri: None,
        current_lyrics: LyricsState::default(),
        play_load_seq: 0,
        remote_queue_meta: None,
        discovery_task: None,
        cluster_task: None,
        player_ev_task: None,
        mpris,
        cover_stats,
        pending_seek_emit: false,
        local_player_stopped: true,
        local_spirc_queue_dirty: false,
        current_playing_uri: None,
        loading_track_uri: None,
        pending_local_load_uri: None,
        pending_local_load_paused: false,
        active_play_request_id: None,
        expecting_remote_transfer_until: None,
        retried_unavailable_uri: None,
        consecutive_unavailable_skips: 0,
        sleep_deadline: None,
        sleep_end_of_track: false,
        active_play_counted: false,
        last_ab_seek_at: Instant::now(),
        active_sponsor_segments: Vec::new(),
        skipped_sponsor_starts: HashSet::new(),
        last_sponsor_skip_at: Instant::now(),
        ad_muted_restore_vol: None,
        downloads_in_flight: HashSet::new(),
    };

    // Restore previous queue & track cleanly (paused, ready to resume).
    if state.config.resume_session_on_startup
        && let Some(saved) = SavedSessionState::load(&state.paths)
        && let Some(mut active) = saved.active_track
    {
        let liked = state.likes_store.uri_set();
        active.is_liked = liked.contains(active.uri.as_ref());
        state.player_state.active_source = active.source;
        state.player_state.duration_ms = active.duration_ms;
        state.player_state.position_ms = saved.position_ms.min(active.duration_ms.max(saved.position_ms));
        state.player_state.shuffle = saved.shuffle;
        state.player_state.repeat = saved.repeat;
        state.player_state.active_track = Some(track_detail_from_summary(&active));
        state.current_playing_uri = Some(active.uri.to_string());
        state.queue.restore_snapshot(
            saved.context_name,
            Some(active),
            saved.manual_items,
            saved.context_items,
        );
    }
    state.sync_audio_metrics();

    state.emit_config(&emitter);
    emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
    emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
    state.emit_player(&emitter);
    state.emit_queue(&emitter);
    state.emit_devices(&emitter);
    state.emit_custom_playlists(&emitter);
    state.emit_local_status(&emitter, !state.config.local_music_dirs.is_empty());
    state.emit_stats(&emitter);
    emit_home_crate(&state, &emitter);
    emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
        &state.paths,
        &state.cover_stats,
    )));

    // Kick off non-blocking local scan if folders exist.
    if !state.config.local_music_dirs.is_empty() {
        spawn_local_scan(&state, &int_tx);
    }

    if state.config.zeroconf_enabled {
        restart_discovery(&mut state, &int_tx);
    }

    if let Ok(cache) = open_librespot_cache(&state.paths, &state.config)
        && let Some(creds) = cache.credentials()
    {
        let zeroconf_only = state.auth_meta.zeroconf_only;
        spawn_session_connect(&mut state, creds, zeroconf_only, &int_tx, &emitter);
    }

    let mut telemetry_tick = tokio::time::interval(Duration::from_secs(1));
    telemetry_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick_count: u64 = 0;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                if let Some(c) = state.oauth_cancel.take() {
                    c.cancel();
                }
                if let Some(c) = state.web_oauth_cancel.take() {
                    c.cancel();
                }
                state.persist_session();
                state.stats_store.save(&state.paths);
                state.direct_player.stop();
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
                tick_count = tick_count.wrapping_add(1);
                handle_periodic_tick(&mut state, tick_count, &emitter).await;
            }
        }
    }
}

async fn handle_periodic_tick(state: &mut BackendState, tick_count: u64, emitter: &Emitter) {
    // 1. Listening stats tracking while playing
    if state.player_state.is_playing && !state.player_state.is_buffering {
        state
            .stats_store
            .record_listen_secs(state.player_state.active_source, 1);
        if !state.active_play_counted
            && state.player_state.position_ms >= 15_000
            && let Some(ref tr) = state.player_state.active_track
        {
            state.active_play_counted = true;
            state.stats_store.record_track_play(&tr.summary);
            state.emit_stats(emitter);
        }
        if tick_count.is_multiple_of(15) {
            state.stats_store.save(&state.paths);
            state.emit_stats(emitter);
        }
    }

    // 2. Sleep timer countdown & soft volume fade during final 15s
    if let Some(deadline) = state.sleep_deadline {
        let now = Instant::now();
        if now >= deadline {
            state.sleep_deadline = None;
            state.player_state.sleep_timer.remaining_secs = None;
            state.direct_player.set_fade_multiplier(1.0);
            pause_active_playback(state, emitter).await;
            emitter.emit(BackendEvent::Notice(Some(
                "Sleep timer finished. Playback paused.".to_string(),
            )));
        } else {
            let rem = deadline.duration_since(now).as_secs() as u32;
            state.player_state.sleep_timer.remaining_secs = Some(rem);
            if rem <= 15 {
                let fade = (rem as f32 / 15.0).clamp(0.05, 1.0);
                state.direct_player.set_fade_multiplier(fade);
            }
            state.emit_player(emitter);
        }
    }

    // 3. Enforce audio disk budget & emit telemetry every 2s
    if tick_count.is_multiple_of(2) {
        enforce_audio_cache_limit(state);
        emitter.emit(BackendEvent::TelemetryUpdated(collect_telemetry(
            &state.paths,
            &state.cover_stats,
        )));
    }
}

fn spawn_local_scan(state: &BackendState, int_tx: &mpsc::UnboundedSender<InternalMsg>) {
    let paths = state.paths.clone();
    let dirs = state.config.local_music_dirs.clone();
    let existing = state.local_lib.tracks.clone();
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let tracks = tokio::task::spawn_blocking(move || {
            LocalLibrary::scan_dirs_blocking(&paths, &dirs, &existing)
        })
        .await
        .unwrap_or_default();
        let _ = tx.send(InternalMsg::LocalScanFinished { tracks });
    });
}

fn emit_home_crate(state: &BackendState, emitter: &Emitter) {
    let liked_set = state.likes_store.uri_set();
    let mut tracks: Vec<TrackSummary> = state
        .recent_history
        .entries
        .iter()
        .take(15)
        .map(|e| {
            let mut sum = make_local_uri_summary(&e.uri, Some(&e.title), e.duration_ms);
            sum.uri = Arc::from(e.uri.as_str());
            sum.artist = Arc::from(e.subtitle.as_str());
            sum.source = MediaSource::from_uri(&e.uri);
            sum.local_path = e.uri.strip_prefix("local:").map(Arc::from);
            sum.cover = e.cover_url.as_ref().map(|u| pulse_model::ImageRef {
                id: Arc::from(u.as_str()),
                url: Some(Arc::from(u.as_str())),
            });
            sum.is_liked = liked_set.contains(&e.uri);
            sum
        })
        .collect();

    if tracks.is_empty() && !state.local_lib.tracks.is_empty() {
        tracks = state
            .local_lib
            .tracks
            .iter()
            .take(25)
            .map(|t| t.to_summary(&liked_set))
            .collect();
    }

    let total = tracks.len();
    emitter.emit(BackendEvent::CrateHeaderLoaded {
        crate_ref: CrateRef::Home,
        title: "Home".to_string(),
        subtitle: format!(
            "{} local tracks · {} playlists · {} favorites",
            state.local_lib.tracks.len(),
            state.custom_playlists.playlists.len(),
            state.likes_store.tracks.len()
        ),
        cover: None,
        total,
    });
    emitter.emit(BackendEvent::CrateTracksPageLoaded {
        crate_ref: CrateRef::Home,
        offset: 0,
        total,
        items: tracks,
    });
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
    state.local_player_stopped = true;
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
        let msg = "failed to open Aplayer cache directory".to_string();
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
    let dsp = Arc::clone(&state.dsp);
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        match connect_and_verify_session(&paths, session_cfg, cache, creds, &cfg, initial_vol, dsp)
            .await
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

async fn pause_active_playback(state: &mut BackendState, emitter: &Emitter) {
    if let Some(remote_id) = state.is_remote_active()
        && let Some(sess) = state.session.clone()
    {
        let _ = send_remote_command(&sess, &remote_id, serde_json::json!({ "endpoint": "pause" }))
            .await;
    } else if state.player_state.active_source != MediaSource::Spotify {
        state.direct_player.pause();
        state.player_state.is_playing = false;
        state.emit_player(emitter);
        state.sync_mpris(false).await;
    } else if let Some(ref lp) = state.local_playback {
        if state.pending_local_load_uri.is_some() {
            state.pending_local_load_paused = true;
        }
        lp.request_sink_flush();
        let _ = lp.spirc.pause();
        state.player_state.is_playing = false;
        state.emit_player(emitter);
        state.sync_mpris(false).await;
    }
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
            state.auth_meta = AuthMeta::default();
            state.library.lock().await.invalidate();
            let _ = clear_auth_files(&state.paths);
            let _ = clear_web_api_token(&state.paths);

            if state.player_state.active_source == MediaSource::Spotify {
                state.player_state.is_playing = false;
                state.player_state.active_track = None;
                state.player_state.position_ms = 0;
                state.player_state.duration_ms = 0;
            }
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
            emitter.emit(BackendEvent::Notice(Some(
                "Signed out of Spotify. Local, YouTube, and SoundCloud stay ready.".to_string(),
            )));
            emitter.emit(BackendEvent::AuthChanged(state.auth_state.clone()));
            emitter.emit(BackendEvent::ConnectionChanged(state.conn_status.clone()));
            state.emit_player(emitter);
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
            } else if let Some(ref active) = state.player_state.active_track.clone() {
                if active.summary.source != MediaSource::Spotify {
                    if state.active_direct_uri.as_deref() == Some(active.summary.uri.as_ref()) {
                        state.direct_player.play();
                        state.player_state.is_playing = true;
                        state.emit_player(emitter);
                    } else {
                        let start_ms = state.player_state.position_ms;
                        load_single_track_locally(state, &active.summary, start_ms, true, int_tx, emitter).await;
                    }
                } else if state.local_player_stopped {
                    load_single_track_locally(state, &active.summary, state.player_state.position_ms, true, int_tx, emitter).await;
                } else if let Some(ref lp) = state.local_playback {
                    let _ = lp.spirc.activate();
                    let _ = lp.spirc.play();
                    state.player_state.is_playing = true;
                    state.emit_player(emitter);
                }
            } else if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                if !state.local_lib.tracks.is_empty() {
                    start_play_crate(state, CrateRef::LocalTracks, state.player_state.shuffle, int_tx, emitter).await;
                } else if state.session.is_some() {
                    start_play_crate(state, CrateRef::LikedSongs, state.player_state.shuffle, int_tx, emitter).await;
                } else {
                    emitter.emit(BackendEvent::Notice(Some(
                        "Add a local folder or search YouTube/SoundCloud to start playing.".to_string(),
                    )));
                }
            }
        }
        UiCommand::Pause => {
            pause_active_playback(state, emitter).await;
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
            } else if state.player_state.is_playing {
                pause_active_playback(state, emitter).await;
            } else if let Some(active) = state.player_state.active_track.clone() {
                if active.summary.source != MediaSource::Spotify {
                    if state.active_direct_uri.as_deref() == Some(active.summary.uri.as_ref()) {
                        state.direct_player.play();
                        state.player_state.is_playing = true;
                        state.emit_player(emitter);
                    } else {
                        let start_ms = state.player_state.position_ms;
                        load_single_track_locally(state, &active.summary, start_ms, true, int_tx, emitter).await;
                    }
                } else if state.local_player_stopped {
                    load_single_track_locally(state, &active.summary, state.player_state.position_ms, true, int_tx, emitter).await;
                } else if let Some(ref lp) = state.local_playback {
                    let _ = lp.spirc.activate();
                    let _ = lp.spirc.play();
                    state.player_state.is_playing = true;
                    state.emit_player(emitter);
                }
            } else if !play_queue_index_cmd(state, 0, int_tx, emitter).await {
                if !state.local_lib.tracks.is_empty() {
                    start_play_crate(state, CrateRef::LocalTracks, state.player_state.shuffle, int_tx, emitter).await;
                } else if state.session.is_some() {
                    start_play_crate(state, CrateRef::LikedSongs, state.player_state.shuffle, int_tx, emitter).await;
                } else {
                    emitter.emit(BackendEvent::Notice(Some(
                        "Add a local folder or search YouTube/SoundCloud to start playing.".to_string(),
                    )));
                }
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
            } else if !state.local_queue_owned {
                if let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                    let _ = lp.spirc.next();
                }
            } else {
                let wrap = state.player_state.repeat == RepeatMode::Context;
                if let Some(next) = state.queue.step_next_track(wrap) {
                    state.emit_queue(emitter);
                    load_single_track_locally(state, &next, 0, true, int_tx, emitter).await;
                } else {
                    state.direct_player.stop();
                    state.active_direct_uri = None;
                    if let Some(ref lp) = state.local_playback {
                        lp.request_sink_flush();
                        let _ = lp.spirc.pause();
                    }
                    state.local_player_stopped = true;
                    state.player_state.is_playing = false;
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
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
            } else if state.player_state.position_ms > 3_000 {
                seek_active_ms(state, 0, emitter);
            } else if !state.local_queue_owned {
                if let Some(ref lp) = state.local_playback {
                    lp.request_sink_flush();
                    let _ = lp.spirc.prev();
                }
            } else {
                let wrap = state.player_state.repeat == RepeatMode::Context;
                if let Some(prev) = state.queue.step_prev_track(wrap) {
                    state.emit_queue(emitter);
                    load_single_track_locally(state, &prev, 0, true, int_tx, emitter).await;
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
            } else {
                seek_active_ms(state, clamped, emitter);
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
            state.direct_player.set_volume(vol);
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
            state.direct_player.set_volume(new_vol);
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
                let _ = lp.spirc.repeat(false);
                let _ = lp.spirc.repeat_track(false);
            }
        }
        UiCommand::PlayTrackDirect {
            track,
            context_name,
            context_tracks,
        } => {
            state.consecutive_unavailable_skips = 0;
            let list = if context_tracks.is_empty() {
                vec![track.clone()]
            } else {
                context_tracks
            };
            let idx = list.iter().position(|t| t.uri == track.uri).or(Some(0));
            state.queue.set_context(context_name, None, list, idx);
            if state.player_state.shuffle {
                state.queue.set_shuffle(true);
            }
            state.emit_queue(emitter);
            load_single_track_locally(state, &track, 0, true, int_tx, emitter).await;
        }
        UiCommand::PlayTrackInContext {
            context_uri,
            track_uri,
            index,
        } => {
            state.consecutive_unavailable_skips = 0;
            // Fast path: check non-Spotify crates or cached track list first (works offline/without Spotify auth!).
            if let Some(cref) = crate_ref_from_context_uri(&context_uri)
                && let Some((title, ctx_opt, tracks)) = resolve_non_spotify_crate(state, &cref)
            {
                let play_idx = Some(index)
                    .filter(|&i| tracks.get(i).is_some_and(|t| t.uri.as_ref() == track_uri))
                    .or_else(|| tracks.iter().position(|t| t.uri.as_ref() == track_uri));
                let target = play_idx
                    .and_then(|i| tracks.get(i).cloned())
                    .or_else(|| cached_track_summary(state, &track_uri));
                if let Some(selected) = target {
                    state
                        .queue
                        .set_context(title, ctx_opt.as_deref().map(Arc::from), tracks, play_idx);
                    if state.player_state.shuffle {
                        state.queue.set_shuffle(true);
                    }
                    state.emit_queue(emitter);
                    load_single_track_locally(state, &selected, 0, true, int_tx, emitter).await;
                    return;
                }
            }
            if let Some(cref) = crate_ref_from_context_uri(&context_uri)
                && let Some((_, _, cached)) = cached_tracks_for_crate(state, &cref, Some(&track_uri))
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
            if let Some(cached) = find_cached_track_list_for_uri(state, &track_uri) {
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

            if MediaSource::from_uri(&track_uri) != MediaSource::Spotify {
                if let Some(found) = cached_track_summary(state, &track_uri) {
                    state.queue.set_context("Queue", None, vec![found.clone()], Some(0));
                    state.emit_queue(emitter);
                    load_single_track_locally(state, &found, 0, true, int_tx, emitter).await;
                } else {
                    emitter.error("Track metadata not loaded.");
                }
                return;
            }

            let Some(sess) = state.session.clone() else {
                emitter.error("Sign in with Spotify Premium in Settings to play Spotify URIs.");
                return;
            };
            state.play_load_seq = state.play_load_seq.wrapping_add(1);
            let play_seq = state.play_load_seq;
            let session_gen = state.session_gen;
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
            if let Some((_title, _ctx, tracks)) = resolve_non_spotify_crate(state, &crate_ref) {
                for t in &tracks {
                    state.queue.append(t.clone());
                }
                state.emit_queue(emitter);
                prestage_upcoming_tracks(state);
                return;
            }
            let Some(sess) = state.session.clone() else {
                emitter.error("Sign in with Spotify to queue Spotify collections.");
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
            prestage_upcoming_tracks(state);
            sync_manual_queue_to_active_device(state).await;
        }
        UiCommand::AppendQueue(track) => {
            state.queue.append(track.clone());
            state.emit_queue(emitter);
            prestage_upcoming_tracks(state);
            if let Some(remote_id) = state.is_remote_active()
                && let Some(sess) = state.session.clone()
                && track.source == MediaSource::Spotify
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
        UiCommand::DeduplicateManualQueue => {
            let removed = state.queue.deduplicate_manual();
            if removed > 0 {
                state.emit_queue(emitter);
                emitter.emit(BackendEvent::Notice(Some(format!(
                    "Removed {removed} duplicate queue entries."
                ))));
            }
        }
        UiCommand::ClearManualQueue => {
            state.queue.clear_manual();
            state.emit_queue(emitter);
            sync_manual_queue_to_active_device(state).await;
        }
        UiCommand::SaveQueueAsPlaylist(name) => {
            let mut tracks = Vec::new();
            if let Some(ref cur) = state.player_state.active_track {
                tracks.push(cur.summary.clone());
            }
            let qs = state.queue.to_queue_state();
            tracks.extend(qs.manual_items);
            tracks.extend(qs.context_items.into_iter().take(100));
            if tracks.is_empty() {
                emitter.error("Queue is empty.");
                return;
            }
            let pl = state.custom_playlists.create(&name);
            let id = pl.id.clone();
            if let Some(target) = state.custom_playlists.find_mut(&id) {
                target.tracks = tracks;
            }
            state.custom_playlists.save(&state.paths);
            state.emit_custom_playlists(emitter);
            emitter.emit(BackendEvent::Notice(Some(format!(
                "Saved queue to playlist '{name}'."
            ))));
        }
        UiCommand::PlayQueueIndex(index) => {
            state.consecutive_unavailable_skips = 0;
            let _ = play_queue_index_cmd(state, index, int_tx, emitter).await;
        }
        UiCommand::OpenCrate(crate_ref) => {
            state.current_crate = crate_ref.clone();
            spawn_crate_fetch(state, crate_ref, 0, 50, int_tx, emitter);
        }
        UiCommand::FetchCratePage {
            crate_ref,
            offset,
            limit,
        } => {
            spawn_crate_fetch(state, crate_ref, offset, limit, int_tx, emitter);
        }
        UiCommand::RunGlobalSearch {
            query,
            source_filter,
        } => {
            let trimmed = query.trim().to_string();
            if trimmed.is_empty() {
                return;
            }
            if let Some(parsed) = parse_direct_input(&trimmed) {
                resolve_url_or_path(state, trimmed, parsed, int_tx, emitter);
                return;
            }
            let crate_ref = CrateRef::Search {
                query: trimmed.clone(),
                source_filter,
            };
            state.current_crate = crate_ref.clone();
            run_multi_source_search(state, trimmed, source_filter, int_tx);
        }
        UiCommand::ResolveInputUrl(raw) => {
            let trimmed = raw.trim().to_string();
            if let Some(parsed) = parse_direct_input(&trimmed) {
                resolve_url_or_path(state, trimmed, parsed, int_tx, emitter);
            } else {
                let crate_ref = CrateRef::Search {
                    query: trimmed.clone(),
                    source_filter: None,
                };
                state.current_crate = crate_ref;
                run_multi_source_search(state, trimmed, None, int_tx);
            }
        }
        UiCommand::SetSortOrder(sort) => {
            state.sort_order = sort;
            state.last_crate_tracks = None;
            if !matches!(
                state.current_crate,
                CrateRef::Search { .. } | CrateRef::Artist { .. }
            ) {
                spawn_crate_fetch(state, state.current_crate.clone(), 0, 50, int_tx, emitter);
            }
        }
        UiCommand::AddLocalMusicFolder(dir) => {
            let trimmed = dir.trim().to_string();
            if !trimmed.is_empty()
                && Path::new(&trimmed).is_dir()
                && !state.config.local_music_dirs.contains(&trimmed)
            {
                state.config.local_music_dirs.push(trimmed);
                state.save_current_config(emitter);
                state.emit_local_status(emitter, true);
                spawn_local_scan(state, int_tx);
            }
        }
        UiCommand::RemoveLocalMusicFolder(dir) => {
            state.config.local_music_dirs.retain(|d| d != &dir);
            state.save_current_config(emitter);
            state.emit_local_status(emitter, true);
            spawn_local_scan(state, int_tx);
        }
        UiCommand::RescanLocalMusic => {
            state.emit_local_status(emitter, true);
            spawn_local_scan(state, int_tx);
        }
        UiCommand::OpenLocalPaths(paths_vec) => {
            let mut added = Vec::new();
            for p_str in paths_vec {
                let p = Path::new(&p_str);
                if p.is_dir() {
                    let s = p.to_string_lossy().to_string();
                    if !state.config.local_music_dirs.contains(&s) {
                        state.config.local_music_dirs.push(s);
                        state.save_current_config(emitter);
                        spawn_local_scan(state, int_tx);
                    }
                } else if p.is_file() && is_audio_file(p) {
                    if let Some(indexed) = state.local_lib.index_ad_hoc_path(&state.paths, p) {
                        let liked = state.likes_store.uri_set();
                        added.push(indexed.to_summary(&liked));
                    }
                } else if p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
                {
                    import_m3u_file(state, p, emitter);
                }
            }
            state.emit_local_status(emitter, false);
            if let Some(first) = added.first().cloned() {
                state
                    .queue
                    .set_context("Opened Files", None, added, Some(0));
                state.emit_queue(emitter);
                load_single_track_locally(state, &first, 0, true, int_tx, emitter).await;
            }
        }
        UiCommand::ToggleLike(uri) => {
            let maybe_track = cached_track_summary(state, &uri);
            if let Some(track) = maybe_track {
                toggle_like_for_track(state, track, int_tx, emitter).await;
            } else if uri.starts_with("spotify:")
                && let Some(sess) = state.session.clone()
            {
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
        }
        UiCommand::ToggleLikeTrack(track) => {
            toggle_like_for_track(state, track, int_tx, emitter).await;
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
        UiCommand::CreateCustomPlaylist(name) => {
            state.custom_playlists.create(&name);
            state.custom_playlists.save(&state.paths);
            state.emit_custom_playlists(emitter);
        }
        UiCommand::RenameCustomPlaylist { id, name } => {
            if state.custom_playlists.rename(&id, &name) {
                state.custom_playlists.save(&state.paths);
                state.emit_custom_playlists(emitter);
            }
        }
        UiCommand::DeleteCustomPlaylist(id) => {
            if state.custom_playlists.delete(&id) {
                state.custom_playlists.save(&state.paths);
                state.emit_custom_playlists(emitter);
            }
        }
        UiCommand::AddTrackToCustomPlaylist { playlist_id, track } => {
            if let Some(pl) = state.custom_playlists.find_mut(&playlist_id) {
                if !pl.tracks.iter().any(|t| t.uri == track.uri) {
                    pl.tracks.push(track);
                }
                let pl_name = pl.name.clone();
                state.custom_playlists.save(&state.paths);
                state.emit_custom_playlists(emitter);
                emitter.emit(BackendEvent::Notice(Some(format!(
                    "Added track to '{pl_name}'."
                ))));
            }
        }
        UiCommand::RemoveTrackFromCustomPlaylist { playlist_id, index } => {
            if let Some(pl) = state.custom_playlists.find_mut(&playlist_id)
                && index < pl.tracks.len()
            {
                pl.tracks.remove(index);
                state.custom_playlists.save(&state.paths);
                state.emit_custom_playlists(emitter);
                if matches!(&state.current_crate, CrateRef::CustomPlaylist { id, .. } if id == &playlist_id)
                {
                    spawn_crate_fetch(state, state.current_crate.clone(), 0, 200, int_tx, emitter);
                }
            }
        }
        UiCommand::ReorderCustomPlaylistTrack {
            playlist_id,
            from,
            to,
        } => {
            if let Some(pl) = state.custom_playlists.find_mut(&playlist_id)
                && from < pl.tracks.len()
                && !pl.tracks.is_empty()
            {
                let item = pl.tracks.remove(from);
                let dest = to.min(pl.tracks.len());
                pl.tracks.insert(dest, item);
                state.custom_playlists.save(&state.paths);
                state.emit_custom_playlists(emitter);
            }
        }
        UiCommand::ImportM3uPlaylist(path_str) => {
            import_m3u_file(state, Path::new(&path_str), emitter);
        }
        UiCommand::ExportCustomPlaylistM3u {
            playlist_id,
            dest_path,
        } => match state.custom_playlists.export_m3u8(&playlist_id, Path::new(&dest_path)) {
            Ok(()) => emitter.emit(BackendEvent::Notice(Some(format!(
                "Exported playlist to {dest_path}"
            )))),
            Err(err) => emitter.error(err),
        },
        UiCommand::SetEqConfig(eq) => {
            state.config.eq = eq.clone();
            state.dsp.set_eq_config(eq);
            state.save_current_config(emitter);
        }
        UiCommand::SetPlaybackSpeed(speed) => {
            let clamped = pulse_model::PlaybackSpeed {
                rate: speed.rate.clamp(0.5, 2.0),
                preserve_pitch: speed.preserve_pitch,
            };
            state.config.playback_speed = clamped;
            state.player_state.playback_speed = clamped;
            state.direct_player.set_speed(clamped);
            state.save_current_config(emitter);
            state.emit_player(emitter);
        }
        UiCommand::SetAbLoop(ab) => {
            state.player_state.ab_loop = ab;
            state.emit_player(emitter);
        }
        UiCommand::ToggleAbLoopStep => {
            let pos = state.player_state.position_ms;
            let cur = state.player_state.ab_loop;
            state.player_state.ab_loop = match (cur.a_ms, cur.b_ms) {
                (None, _) => pulse_model::AbLoopState {
                    a_ms: Some(pos),
                    b_ms: None,
                },
                (Some(a), None) if pos > a + 500 => pulse_model::AbLoopState {
                    a_ms: Some(a),
                    b_ms: Some(pos),
                },
                _ => pulse_model::AbLoopState::default(),
            };
            state.emit_player(emitter);
        }
        UiCommand::SetSleepTimer(target) => {
            state.direct_player.set_fade_multiplier(1.0);
            match target {
                SleepTimerTarget::Off => {
                    state.sleep_deadline = None;
                    state.sleep_end_of_track = false;
                    state.player_state.sleep_timer = Default::default();
                }
                SleepTimerTarget::Minutes(mins) => {
                    let secs = u64::from(mins.max(1)) * 60;
                    state.sleep_deadline = Some(Instant::now() + Duration::from_secs(secs));
                    state.sleep_end_of_track = false;
                    state.player_state.sleep_timer = pulse_model::SleepTimerState {
                        remaining_secs: Some(secs as u32),
                        end_of_track: false,
                    };
                }
                SleepTimerTarget::EndOfTrack => {
                    state.sleep_deadline = None;
                    state.sleep_end_of_track = true;
                    state.player_state.sleep_timer = pulse_model::SleepTimerState {
                        remaining_secs: None,
                        end_of_track: true,
                    };
                }
            }
            state.emit_player(emitter);
        }
        UiCommand::SetLyricsOffsetMs(offset_ms) => {
            state.current_lyrics.offset_ms = offset_ms;
            if !state.current_lyrics.track_uri.is_empty() {
                emitter.emit(BackendEvent::LyricsLoaded(state.current_lyrics.clone()));
            }
            emitter.emit(BackendEvent::Notice(Some(format!(
                "Lyrics sync offset: {:+.1}s",
                offset_ms as f32 / 1000.0
            ))));
        }
        UiCommand::RefreshLyrics => {
            if let Some(tr) = state.player_state.active_track.clone() {
                state.in_flight_lyrics_uri = None;
                maybe_fetch_lyrics(state, &tr.summary, int_tx, emitter);
            }
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
                state.expecting_remote_transfer_until =
                    Some(Instant::now() + Duration::from_secs(8));
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
            state.config.audio_device = audio_device.clone();
            state.direct_player.set_audio_device(audio_device);
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
        UiCommand::SetThemePreference(pref) => {
            state.config.theme_preference = pref;
            state.save_current_config(emitter);
        }
        UiCommand::SetAccentSource(src) => {
            state.config.accent_source = src;
            state.save_current_config(emitter);
        }
        UiCommand::SetVisualizerMode(vis) => {
            state.config.visualizer_mode = vis;
            state.dsp.set_visualizer_enabled(vis != VisualizerMode::Off);
            state.save_current_config(emitter);
        }
        UiCommand::SetCrossfadeMs(ms) => {
            state.config.crossfade_ms = ms;
            state.save_current_config(emitter);
        }
        UiCommand::SetResumeOnStartup(resume) => {
            state.config.resume_session_on_startup = resume;
            state.save_current_config(emitter);
        }
        UiCommand::SetAdBlockEnabled(enabled) => {
            state.config.ad_block_enabled = enabled;
            if !enabled
                && let Some(restore_vol) = state.ad_muted_restore_vol.take()
                && let Some(ref lp) = state.local_playback
            {
                lp.mixer.set_volume(restore_vol);
            }
            state.save_current_config(emitter);
            emitter.emit(BackendEvent::Notice(Some(if enabled {
                "Ad-Free Shield & SponsorBlock enabled.".to_string()
            } else {
                "Ad-Free Shield disabled.".to_string()
            })));
        }
        UiCommand::SetMp3DownloadDir(dir) => {
            let trimmed = dir.trim().to_string();
            if !trimmed.is_empty() {
                let _ = fs::create_dir_all(&trimmed);
                state.config.mp3_download_dir = trimmed.clone();
                if state.config.mp3_auto_add_to_library
                    && !is_dir_covered_by_roots(&state.config.local_music_dirs, Path::new(&trimmed))
                {
                    state.config.local_music_dirs.push(trimmed);
                    state.emit_local_status(emitter, true);
                    spawn_local_scan(state, int_tx);
                }
                state.save_current_config(emitter);
            }
        }
        UiCommand::SetMp3Quality(quality) => {
            state.config.mp3_quality = quality;
            state.save_current_config(emitter);
        }
        UiCommand::SetMp3AutoAddToLibrary(auto_add) => {
            state.config.mp3_auto_add_to_library = auto_add;
            if auto_add {
                let dl = PathBuf::from(&state.config.mp3_download_dir);
                if dl.is_dir() && !is_dir_covered_by_roots(&state.config.local_music_dirs, &dl) {
                    state.config.local_music_dirs.push(state.config.mp3_download_dir.clone());
                    state.emit_local_status(emitter, true);
                    spawn_local_scan(state, int_tx);
                }
            }
            state.save_current_config(emitter);
        }
        UiCommand::SetMp3StripSponsors(strip) => {
            state.config.mp3_strip_sponsors = strip;
            state.save_current_config(emitter);
        }
        UiCommand::SaveYouTubeMp3 {
            track,
            dest_path,
            quality,
            strip_sponsors,
        } => {
            if track.source != MediaSource::YouTube {
                emitter.error("Only YouTube tracks can be saved as MP3.");
                return;
            }
            let Some(vid) = track.uri.strip_prefix("yt:video:").filter(|v| v.len() == 11) else {
                emitter.error("Invalid YouTube track URI for MP3 export.");
                return;
            };
            let uri_str = track.uri.to_string();
            if !state.downloads_in_flight.insert(uri_str.clone()) {
                emitter.emit(BackendEvent::Notice(Some(
                    "MP3 save is already in progress for this track.".to_string(),
                )));
                return;
            }
            let chosen_quality = quality.unwrap_or(state.config.mp3_quality);
            let chosen_strip = strip_sponsors.unwrap_or(state.config.mp3_strip_sponsors);
            let auto_index = state.config.mp3_auto_add_to_library;
            let dl_dir = PathBuf::from(&state.config.mp3_download_dir);
            let vid = vid.to_string();
            let http = Arc::clone(&state.http);
            let paths = state.paths.clone();
            let tx = int_tx.clone();
            let title = track.title.clone();

            emitter.emit(BackendEvent::Mp3DownloadStarted {
                uri: uri_str.clone(),
                title: title.clone(),
            });
            emitter.emit(BackendEvent::Notice(Some(format!(
                "Saving MP3 ({}): {} - {}...",
                chosen_quality.label(),
                track.artist,
                track.title
            ))));

            tokio::task::spawn_local(async move {
                let res = async {
                    let (cached_path, _) = ensure_youtube_audio_cached(&http, &paths, &vid).await?;
                    let segments = if chosen_strip {
                        fetch_youtube_skip_segments(&http, &vid).await
                    } else {
                        Vec::new()
                    };
                    let filter = build_sponsor_aselect_filter(&segments, track.duration_ms);
                    let cover = fetch_youtube_cover_jpeg(&http, &vid).await;
                    let dest = if let Some(raw_dest) = dest_path {
                        let mut pb = PathBuf::from(raw_dest);
                        if pb
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_none_or(|ext| !ext.eq_ignore_ascii_case("mp3"))
                        {
                            pb.set_extension("mp3");
                        }
                        pb
                    } else {
                        let stem = sanitize_filename_stem(&track.artist, &track.title);
                        resolve_unique_mp3_path(&dl_dir, &stem)?
                    };
                    let tr_c = track.clone();
                    let vid_c = vid.clone();
                    tokio::task::spawn_blocking(move || {
                        transcode_and_tag_mp3_blocking(
                            &cached_path,
                            &dest,
                            chosen_quality,
                            filter.as_deref(),
                            &tr_c,
                            &vid_c,
                            cover.as_deref(),
                        )
                    })
                    .await
                    .map_err(|e| format!("MP3 worker task failed: {e}"))?
                }
                .await;

                let _ = tx.send(InternalMsg::Mp3ExportFinished {
                    uri: uri_str,
                    auto_index,
                    result: res,
                });
            });
        }
        UiCommand::SetScope(scope) => {
            let target = match scope {
                LibraryScope::Home => Some(CrateRef::Home),
                LibraryScope::LocalMusic => Some(CrateRef::LocalTracks),
                LibraryScope::UnifiedLikes => Some(CrateRef::UnifiedLikes),
                LibraryScope::CustomPlaylists => Some(CrateRef::CustomPlaylistsIndex),
                LibraryScope::LikedSongs => Some(CrateRef::LikedSongs),
                LibraryScope::Playlists => Some(CrateRef::PlaylistsIndex),
                LibraryScope::Albums => Some(CrateRef::AlbumsIndex),
                LibraryScope::Artists => Some(CrateRef::ArtistsIndex),
                LibraryScope::Recent => Some(CrateRef::Recent),
                LibraryScope::Search => {
                    let results = state.last_search_results.clone().unwrap_or_default();
                    state.current_crate = CrateRef::Search {
                        query: results.query.clone(),
                        source_filter: results.source_filter,
                    };
                    emitter.emit(BackendEvent::SearchResultsLoaded(results));
                    None
                }
            };
            if let Some(crate_ref) = target {
                state.current_crate = crate_ref.clone();
                spawn_crate_fetch(state, crate_ref, 0, 50, int_tx, emitter);
            }
        }
        UiCommand::NavigateBack
        | UiCommand::NavigateForward
        | UiCommand::SetLocalFilter(_)
        | UiCommand::SetSearchFilter(_)
        | UiCommand::SetMode(_) => {}
    }
}

fn import_m3u_file(state: &mut BackendState, path: &Path, emitter: &Emitter) {
    let Ok(contents) = fs::read_to_string(path) else {
        emitter.error(format!("Could not read M3U file {}", path.display()));
        return;
    };
    let entries = parse_m3u_entries(&contents, path.parent());
    if entries.is_empty() {
        emitter.error("M3U playlist had no recognizable entries.");
        return;
    }
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Playlist")
        .to_string();
    let mut tracks = Vec::new();
    let liked = state.likes_store.uri_set();
    for (info, uri_or_path) in entries {
        if let Some(existing) = state
            .local_lib
            .tracks
            .iter()
            .find(|t| t.path == uri_or_path || format!("local:{}", t.path) == uri_or_path)
        {
            tracks.push(existing.to_summary(&liked));
        } else if Path::new(&uri_or_path).is_file() {
            let (dur, title_opt) = info
                .as_ref()
                .map(|(d, t)| (*d, Some(t.as_str())))
                .unwrap_or((0, None));
            let mut sum = make_local_uri_summary(&uri_or_path, title_opt, dur);
            sum.is_liked = liked.contains(sum.uri.as_ref());
            tracks.push(sum);
        } else {
            let (dur, title_opt) = info
                .as_ref()
                .map(|(d, t)| (*d, Some(t.as_str())))
                .unwrap_or((0, None));
            tracks.push(TrackSummary {
                uri: Arc::from(uri_or_path.as_str()),
                title: title_opt.unwrap_or(&uri_or_path).to_string(),
                artist: Arc::from(MediaSource::from_uri(&uri_or_path).display_name()),
                artist_uri: None,
                album: Arc::from(""),
                album_uri: None,
                cover: None,
                duration_ms: dur,
                is_liked: liked.contains(&uri_or_path),
                is_playable: true,
                added_at_unix: None,
                source: MediaSource::from_uri(&uri_or_path),
                local_path: None,
                codec_label: None,
            });
        }
    }
    let pl_id = state.custom_playlists.create(&title).id.clone();
    if let Some(pl) = state.custom_playlists.find_mut(&pl_id) {
        pl.tracks = tracks;
    }
    state.custom_playlists.save(&state.paths);
    state.emit_custom_playlists(emitter);
    emitter.emit(BackendEvent::Notice(Some(format!(
        "Imported playlist '{title}'."
    ))));
}

async fn toggle_like_for_track(
    state: &mut BackendState,
    track: TrackSummary,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    let uri = track.uri.to_string();
    let is_liked = state.likes_store.toggle(track);
    state.likes_store.save(&state.paths);

    if let Some(ref mut active) = state.player_state.active_track
        && active.summary.uri.as_ref() == uri
    {
        active.summary.is_liked = is_liked;
        state.emit_player(emitter);
    }
    emitter.emit(BackendEvent::LikeStateChanged {
        uri: uri.clone(),
        is_liked,
    });
    if state.current_crate == CrateRef::UnifiedLikes {
        spawn_crate_fetch(state, CrateRef::UnifiedLikes, 0, 200, int_tx, emitter);
    }

    // Mirror Spotify track likes to Spotify collection if authenticated.
    if uri.starts_with("spotify:track:")
        && let Some(sess) = state.session.clone()
    {
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
}

fn seek_active_ms(state: &mut BackendState, pos_ms: u32, emitter: &Emitter) {
    let clamped = if state.player_state.duration_ms > 0 {
        pos_ms.min(state.player_state.duration_ms)
    } else {
        pos_ms
    };
    state.player_state.position_ms = clamped;
    if state.player_state.active_source != MediaSource::Spotify {
        state.direct_player.seek_ms(clamped);
        state.emit_player(emitter);
    } else if state.loading_track_uri.is_none()
        && state.pending_local_load_uri.is_none()
        && let Some(ref lp) = state.local_playback
    {
        state.pending_seek_emit = true;
        state.emit_player(emitter);
        lp.request_sink_flush();
        let _ = lp.spirc.set_position_ms(clamped);
    }
}

fn run_multi_source_search(
    state: &mut BackendState,
    query: String,
    source_filter: Option<MediaSource>,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
) {
    state.search_seq = state.search_seq.wrapping_add(1);
    let search_seq = state.search_seq;
    let liked = state.likes_store.uri_set();
    let local_tracks = if source_filter.is_none_or(|s| s == MediaSource::Local) {
        state.local_lib.search(&query, &liked)
    } else {
        Vec::new()
    };

    let http = Arc::clone(&state.http);
    let paths = state.paths.clone();
    let sess_opt = state.session.clone();
    let lib = Rc::clone(&state.library);
    let web_tok = Rc::clone(&state.web_api_token);
    let tx = int_tx.clone();

    tokio::task::spawn_local(async move {
        let include_yt = source_filter.is_none_or(|s| s == MediaSource::YouTube);
        let include_sc = source_filter.is_none_or(|s| s == MediaSource::SoundCloud);
        let include_sp = source_filter.is_none_or(|s| s == MediaSource::Spotify) && sess_opt.is_some();

        let yt_fut = async {
            if include_yt {
                search_youtube(&http, &query, &liked).await
            } else {
                Vec::new()
            }
        };
        let sc_fut = async {
            if include_sc {
                search_soundcloud(&http, &paths, &query, &liked).await
            } else {
                (Vec::new(), Vec::new())
            }
        };
        let sp_fut = async {
            if include_sp
                && let Some(ref sess) = sess_opt
            {
                let mut lib_guard = lib.lock().await;
                let mut tok_guard = web_tok.lock().await;
                lib_guard.search(sess, &paths, &mut tok_guard, &query).await.ok()
            } else {
                None
            }
        };

        let (yt_tracks, (sc_tracks, sc_playlists), sp_res) =
            tokio::join!(yt_fut, sc_fut, sp_fut);

        let mut merged = SearchResults {
            query: query.clone(),
            source_filter,
            ..Default::default()
        };

        // Interleave top matches cleanly across enabled sources so all sources appear up front.
        let sp_tracks = sp_res.as_ref().map(|s| s.tracks.clone()).unwrap_or_default();
        let max_len = local_tracks
            .len()
            .max(sp_tracks.len())
            .max(yt_tracks.len())
            .max(sc_tracks.len());
        for i in 0..max_len {
            if let Some(t) = local_tracks.get(i) {
                merged.tracks.push(t.clone());
            }
            if let Some(t) = sp_tracks.get(i) {
                let mut tc = t.clone();
                if liked.contains(tc.uri.as_ref()) {
                    tc.is_liked = true;
                }
                merged.tracks.push(tc);
            }
            if let Some(t) = yt_tracks.get(i) {
                merged.tracks.push(t.clone());
            }
            if let Some(t) = sc_tracks.get(i) {
                merged.tracks.push(t.clone());
            }
        }

        if let Some(sp) = sp_res {
            merged.artists = sp.artists;
            merged.albums = sp.albums;
            merged.playlists.extend(sp.playlists);
        }
        merged.playlists.extend(sc_playlists);
        merged.top_match_uri = merged.tracks.first().map(|t| t.uri.clone());

        let _ = tx.send(InternalMsg::MultiSearchFinished {
            search_seq,
            results: merged,
        });
    });
}

fn resolve_url_or_path(
    state: &mut BackendState,
    raw: String,
    parsed: ResolvedInput,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match parsed {
        ResolvedInput::LocalPath(p) => {
            let path = Path::new(&p);
            if path.is_dir() {
                let s = path.to_string_lossy().to_string();
                if !state.config.local_music_dirs.contains(&s) {
                    state.config.local_music_dirs.push(s.clone());
                    state.save_current_config(emitter);
                    spawn_local_scan(state, int_tx);
                }
                let cr = CrateRef::LocalFolder { path: s };
                state.current_crate = cr.clone();
                spawn_crate_fetch(state, cr, 0, 100, int_tx, emitter);
            } else if path.is_file()
                && let Some(indexed) = state.local_lib.index_ad_hoc_path(&state.paths, path)
            {
                let liked = state.likes_store.uri_set();
                let sum = indexed.to_summary(&liked);
                let _ = int_tx.send(InternalMsg::UrlResolved {
                    query: raw,
                    result: Ok(UrlResolveOutcome::SingleTrack(sum)),
                });
            }
        }
        ResolvedInput::YouTubeVideo(vid) => {
            let http = Arc::clone(&state.http);
            let liked = state.likes_store.uri_set();
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let res = resolve_youtube_video(&http, &vid, &liked)
                    .await
                    .map(UrlResolveOutcome::SingleTrack);
                let _ = tx.send(InternalMsg::UrlResolved {
                    query: raw,
                    result: res,
                });
            });
        }
        ResolvedInput::YouTubePlaylist(pl_id) => {
            let http = Arc::clone(&state.http);
            let liked = state.likes_store.uri_set();
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let res = resolve_youtube_playlist(&http, &pl_id, &liked)
                    .await
                    .map(|(title, tracks)| UrlResolveOutcome::Playlist {
                        crate_ref: CrateRef::YouTubePlaylist {
                            id: pl_id,
                            title: title.clone(),
                        },
                        title,
                        tracks,
                    });
                let _ = tx.send(InternalMsg::UrlResolved {
                    query: raw,
                    result: res,
                });
            });
        }
        ResolvedInput::SoundCloudUrl(url) => {
            let http = Arc::clone(&state.http);
            let paths = state.paths.clone();
            let liked = state.likes_store.uri_set();
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let res = resolve_soundcloud_url(&http, &paths, &url, &liked)
                    .await
                    .map(|r| match r {
                        ScResolved::Track(t) => UrlResolveOutcome::SingleTrack(t),
                        ScResolved::Set { title, tracks } => UrlResolveOutcome::Playlist {
                            crate_ref: CrateRef::SoundCloudSet {
                                url,
                                title: title.clone(),
                            },
                            title,
                            tracks,
                        },
                    });
                let _ = tx.send(InternalMsg::UrlResolved {
                    query: raw,
                    result: res,
                });
            });
        }
        ResolvedInput::SpotifyTrack(uri) => {
            if let Some(sess) = state.session.clone() {
                let tx = int_tx.clone();
                tokio::task::spawn_local(async move {
                    let res = resolve_uris_via_track_api(&sess, &[uri]).await.and_then(|v| {
                        v.into_iter()
                            .next()
                            .map(UrlResolveOutcome::SingleTrack)
                            .ok_or_else(|| "Spotify track not found".to_string())
                    });
                    let _ = tx.send(InternalMsg::UrlResolved {
                        query: raw,
                        result: res,
                    });
                });
            } else {
                emitter.error("Sign in with Spotify in Settings to resolve Spotify URLs.");
            }
        }
        ResolvedInput::SpotifyPlaylist(uri) => {
            let _ = int_tx.send(InternalMsg::UrlResolved {
                query: raw,
                result: Ok(UrlResolveOutcome::SpotifyCrate(CrateRef::Playlist {
                    uri: Arc::from(uri.as_str()),
                    name: String::new(),
                })),
            });
        }
        ResolvedInput::SpotifyAlbum(uri) => {
            let _ = int_tx.send(InternalMsg::UrlResolved {
                query: raw,
                result: Ok(UrlResolveOutcome::SpotifyCrate(CrateRef::Album {
                    uri: Arc::from(uri.as_str()),
                    title: String::new(),
                })),
            });
        }
        ResolvedInput::SpotifyArtist(uri) => {
            let _ = int_tx.send(InternalMsg::UrlResolved {
                query: raw,
                result: Ok(UrlResolveOutcome::SpotifyCrate(CrateRef::Artist {
                    uri: Arc::from(uri.as_str()),
                    name: String::new(),
                })),
            });
        }
    }
}

fn resolve_non_spotify_crate(
    state: &BackendState,
    crate_ref: &CrateRef,
) -> Option<(String, Option<String>, Vec<TrackSummary>)> {
    let liked = state.likes_store.uri_set();
    match crate_ref {
        CrateRef::Home => {
            let recent_list = state.local_lib.summaries(&liked, state.sort_order);
            Some(("Home".to_string(), None, recent_list))
        }
        CrateRef::LocalTracks => Some((
            "Local Music".to_string(),
            Some("local_tracks".to_string()),
            state.local_lib.summaries(&liked, state.sort_order),
        )),
        CrateRef::LocalAlbum { title, artist } => {
            let mut items: Vec<TrackSummary> = state
                .local_lib
                .tracks
                .iter()
                .filter(|t| {
                    let al = if t.album.is_empty() {
                        "Singles & Unsorted"
                    } else {
                        t.album.as_str()
                    };
                    let ar = if t.artist.is_empty() {
                        "Unknown Artist"
                    } else {
                        t.artist.as_str()
                    };
                    al == title && ar == artist
                })
                .map(|t| t.to_summary(&liked))
                .collect();
            sort_track_summaries(&mut items, state.sort_order);
            Some((title.clone(), Some(format!("local_album:{title}\t{artist}")), items))
        }
        CrateRef::LocalArtist { name } => {
            let mut items: Vec<TrackSummary> = state
                .local_lib
                .tracks
                .iter()
                .filter(|t| {
                    let ar = if t.artist.is_empty() {
                        "Unknown Artist"
                    } else {
                        t.artist.as_str()
                    };
                    ar == name
                })
                .map(|t| t.to_summary(&liked))
                .collect();
            sort_track_summaries(&mut items, state.sort_order);
            Some((name.clone(), Some(format!("local_artist:{name}")), items))
        }
        CrateRef::LocalFolder { path } => {
            let mut items: Vec<TrackSummary> = state
                .local_lib
                .tracks
                .iter()
                .filter(|t| t.path.starts_with(path))
                .map(|t| t.to_summary(&liked))
                .collect();
            sort_track_summaries(&mut items, state.sort_order);
            Some((path.clone(), Some(format!("local_folder:{path}")), items))
        }
        CrateRef::UnifiedLikes => {
            let mut items = state.likes_store.tracks.clone();
            sort_track_summaries(&mut items, state.sort_order);
            Some(("Favorites".to_string(), Some("unified_likes".to_string()), items))
        }
        CrateRef::CustomPlaylist { id, .. } => {
            let pl = state.custom_playlists.find(id)?;
            let mut items = pl.tracks.clone();
            for t in &mut items {
                t.is_liked = liked.contains(t.uri.as_ref());
            }
            sort_track_summaries(&mut items, state.sort_order);
            Some((pl.name.clone(), Some(format!("custom_playlist:{id}")), items))
        }
        CrateRef::Recent if state.session.is_none() => {
            let items: Vec<TrackSummary> = state
                .recent_history
                .entries
                .iter()
                .map(|e| {
                    let mut sum = make_local_uri_summary(&e.uri, Some(&e.title), e.duration_ms);
                    sum.uri = Arc::from(e.uri.as_str());
                    sum.artist = Arc::from(e.subtitle.as_str());
                    sum.source = MediaSource::from_uri(&e.uri);
                    sum.local_path = e.uri.strip_prefix("local:").map(Arc::from);
                    sum.cover = e.cover_url.as_ref().map(|u| pulse_model::ImageRef {
                        id: Arc::from(u.as_str()),
                        url: Some(Arc::from(u.as_str())),
                    });
                    sum.is_liked = liked.contains(&e.uri);
                    sum
                })
                .collect();
            Some(("Recently Played".to_string(), Some("recent".to_string()), items))
        }
        _ => None,
    }
}

fn crate_ref_from_context_uri(uri: &str) -> Option<CrateRef> {
    if uri.is_empty() {
        return None;
    }
    if uri == "local_tracks" {
        return Some(CrateRef::LocalTracks);
    }
    if let Some(rest) = uri.strip_prefix("local_album:")
        && let Some((title, artist)) = rest.split_once('\t')
    {
        return Some(CrateRef::LocalAlbum {
            title: title.to_string(),
            artist: artist.to_string(),
        });
    }
    if let Some(name) = uri.strip_prefix("local_artist:") {
        return Some(CrateRef::LocalArtist {
            name: name.to_string(),
        });
    }
    if let Some(path) = uri.strip_prefix("local_folder:") {
        return Some(CrateRef::LocalFolder {
            path: path.to_string(),
        });
    }
    if uri == "unified_likes" {
        return Some(CrateRef::UnifiedLikes);
    }
    if let Some(id) = uri.strip_prefix("custom_playlist:") {
        return Some(CrateRef::CustomPlaylist {
            id: id.to_string(),
            name: String::new(),
        });
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
            source_filter: None,
        });
    }
    None
}

fn spawn_crate_fetch(
    state: &mut BackendState,
    crate_ref: CrateRef,
    offset: usize,
    limit: usize,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    match &crate_ref {
        CrateRef::Home => {
            emit_home_crate(state, emitter);
            return;
        }
        CrateRef::LocalAlbums => {
            let albums: Vec<AlbumSummary> = state.local_lib.albums();
            let total = albums.len();
            emitter.emit(BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title: "Local Albums".to_string(),
                subtitle: format!("{total} albums"),
                cover: None,
                total,
            });
            emitter.emit(BackendEvent::AlbumsPageLoaded {
                offset: 0,
                total,
                items: albums,
            });
            return;
        }
        CrateRef::LocalArtists => {
            let artists = state.local_lib.artists();
            let total = artists.len();
            emitter.emit(BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title: "Local Artists".to_string(),
                subtitle: format!("{total} artists"),
                cover: None,
                total,
            });
            emitter.emit(BackendEvent::ArtistsPageLoaded {
                offset: 0,
                total,
                items: artists,
            });
            return;
        }
        CrateRef::LocalFolders => {
            let liked = state.likes_store.uri_set();
            let items = state.local_lib.summaries(&liked, state.sort_order);
            let total = items.len();
            emitter.emit(BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title: "Music Folders".to_string(),
                subtitle: format!("{} folders · {total} tracks", state.config.local_music_dirs.len()),
                cover: None,
                total,
            });
            emitter.emit(BackendEvent::CrateTracksPageLoaded {
                crate_ref: CrateRef::LocalFolders,
                offset: 0,
                total,
                items,
            });
            return;
        }
        CrateRef::CustomPlaylistsIndex => {
            let list: Vec<PlaylistSummary> = state
                .custom_playlists
                .playlists
                .iter()
                .map(|p| PlaylistSummary {
                    uri: Arc::from(format!("custom_playlist:{}", p.id).as_str()),
                    name: p.name.clone(),
                    owner: Arc::from("Custom"),
                    track_count: p.tracks.len(),
                    total_duration_ms: p.tracks.iter().map(|t| u64::from(t.duration_ms)).sum(),
                    cover: p.tracks.iter().find_map(|t| t.cover.clone()),
                    is_collaborative: false,
                    source: MediaSource::Local,
                })
                .collect();
            let total = list.len();
            emitter.emit(BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title: "Playlists".to_string(),
                subtitle: format!("{total} cross-source playlists"),
                cover: None,
                total,
            });
            emitter.emit(BackendEvent::PlaylistsPageLoaded {
                offset: 0,
                total,
                items: list,
            });
            return;
        }
        CrateRef::YouTubePlaylist { id, title } => {
            let http = Arc::clone(&state.http);
            let liked = state.likes_store.uri_set();
            let pl_id = id.clone();
            let title_fallback = title.clone();
            let cr = crate_ref.clone();
            let tx = int_tx.clone();
            let sgen = state.session_gen;
            tokio::task::spawn_local(async move {
                let res = resolve_youtube_playlist(&http, &pl_id, &liked).await.map(|(t, tracks)| {
                    let total = tracks.len();
                    let display_title = if t.is_empty() { title_fallback } else { t };
                    vec![
                        BackendEvent::CrateHeaderLoaded {
                            crate_ref: cr.clone(),
                            title: display_title,
                            subtitle: format!("YouTube Playlist · {total} tracks"),
                            cover: tracks.first().and_then(|x| x.cover.clone()),
                            total,
                        },
                        BackendEvent::CrateTracksPageLoaded {
                            crate_ref: cr,
                            offset: 0,
                            total,
                            items: tracks,
                        },
                    ]
                });
                let _ = tx.send(InternalMsg::BackendEventsBatch {
                    session_gen: sgen,
                    result: res,
                });
            });
            return;
        }
        CrateRef::SoundCloudSet { url, title } => {
            let http = Arc::clone(&state.http);
            let paths = state.paths.clone();
            let liked = state.likes_store.uri_set();
            let sc_url = url.clone();
            let title_fallback = title.clone();
            let cr = crate_ref.clone();
            let tx = int_tx.clone();
            let sgen = state.session_gen;
            tokio::task::spawn_local(async move {
                let res = resolve_soundcloud_url(&http, &paths, &sc_url, &liked)
                    .await
                    .map(|r| match r {
                        ScResolved::Track(t) => vec![
                            BackendEvent::CrateHeaderLoaded {
                                crate_ref: cr.clone(),
                                title: t.title.clone(),
                                subtitle: "SoundCloud · 1 track".to_string(),
                                cover: t.cover.clone(),
                                total: 1,
                            },
                            BackendEvent::CrateTracksPageLoaded {
                                crate_ref: cr,
                                offset: 0,
                                total: 1,
                                items: vec![t],
                            },
                        ],
                        ScResolved::Set { title: t, tracks } => {
                            let total = tracks.len();
                            let disp = if t.is_empty() { title_fallback } else { t };
                            vec![
                                BackendEvent::CrateHeaderLoaded {
                                    crate_ref: cr.clone(),
                                    title: disp,
                                    subtitle: format!("SoundCloud Set · {total} tracks"),
                                    cover: tracks.first().and_then(|x| x.cover.clone()),
                                    total,
                                },
                                BackendEvent::CrateTracksPageLoaded {
                                    crate_ref: cr,
                                    offset: 0,
                                    total,
                                    items: tracks,
                                },
                            ]
                        }
                    });
                let _ = tx.send(InternalMsg::BackendEventsBatch {
                    session_gen: sgen,
                    result: res,
                });
            });
            return;
        }
        CrateRef::Search {
            query,
            source_filter,
        } => {
            run_multi_source_search(state, query.clone(), *source_filter, int_tx);
            return;
        }
        _ => {}
    }

    if let Some((title, _ctx, tracks)) = resolve_non_spotify_crate(state, &crate_ref) {
        let total = tracks.len();
        let cover = tracks.first().and_then(|t| t.cover.clone());
        let subtitle = format!("{total} tracks");
        state.last_crate_tracks = Some((crate_ref.clone(), total, tracks.clone()));
        emitter.emit(BackendEvent::CrateHeaderLoaded {
            crate_ref: crate_ref.clone(),
            title,
            subtitle,
            cover,
            total,
        });
        emitter.emit(BackendEvent::CrateTracksPageLoaded {
            crate_ref,
            offset: 0,
            total,
            items: tracks,
        });
        return;
    }

    let Some(sess) = state.session.clone() else {
        emitter.emit(BackendEvent::CrateHeaderLoaded {
            crate_ref: crate_ref.clone(),
            title: cached_crate_title_and_ctx(&crate_ref).0,
            subtitle: "Spotify sign-in required for Spotify cloud library".to_string(),
            cover: None,
            total: 0,
        });
        emitter.emit(BackendEvent::CrateTracksPageLoaded {
            crate_ref,
            offset: 0,
            total: 0,
            items: Vec::new(),
        });
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

async fn load_single_track_locally(
    state: &mut BackendState,
    track: &TrackSummary,
    start_ms: u32,
    start_playing: bool,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    state.active_play_counted = false;
    state.player_state.ab_loop = Default::default();
    state.active_sponsor_segments.clear();
    state.skipped_sponsor_starts.clear();
    let mut track = track.clone();
    if state.likes_store.uri_set().contains(track.uri.as_ref()) {
        track.is_liked = true;
    }

    set_optimistic_playing_track(state, &track, int_tx, emitter);
    if !start_playing {
        state.player_state.is_playing = false;
    }

    if track.source == MediaSource::YouTube
        && state.config.ad_block_enabled
        && let Some(vid) = track.uri.strip_prefix("yt:video:").filter(|v| v.len() == 11)
    {
        let http = Arc::clone(&state.http);
        let uri = track.uri.to_string();
        let vid = vid.to_string();
        let tx = int_tx.clone();
        tokio::task::spawn_local(async move {
            let segments = fetch_youtube_skip_segments(&http, &vid).await;
            if !segments.is_empty() {
                let _ = tx.send(InternalMsg::SponsorSegmentsLoaded { uri, segments });
            }
        });
    }

    match track.source {
        MediaSource::Spotify => {
            state.direct_player.stop();
            state.active_direct_uri = None;
            state.player_state.is_buffering = false;
            if state.session.is_some() && state.local_playback.is_some() {
                let target = track.uri.to_string();
                let request = build_tracks_load_request(
                    vec![target.clone()],
                    Some(PlayingTrack::Uri(target.clone())),
                    false,
                    RepeatMode::Off,
                    start_playing,
                    start_ms,
                );
                dispatch_local_load(state, Some(&target), request, int_tx);
            } else {
                // Direct fallback: if Spotify isn't signed in, try resolving song on SoundCloud/YouTube.
                state.player_state.is_buffering = true;
                state.player_state.audio_badge = "STAGE FALLBACK".to_string();
                state.emit_player(emitter);
                state.play_load_seq = state.play_load_seq.wrapping_add(1);
                let seq = state.play_load_seq;
                let http = Arc::clone(&state.http);
                let paths = state.paths.clone();
                let tr = track.clone();
                let tx = int_tx.clone();
                tokio::task::spawn_local(async move {
                    let q = format!("{} {}", tr.artist, tr.title);
                    let liked = HashSet::new();
                    let yt = search_youtube(&http, &q, &liked).await;
                    let result = if let Some(first) = yt.first() {
                        let vid = first.uri.strip_prefix("yt:video:").unwrap_or("");
                        ensure_youtube_audio_cached(&http, &paths, vid).await
                    } else {
                        Err("Sign in to Spotify or search YouTube/SoundCloud directly.".to_string())
                    };
                    let _ = tx.send(InternalMsg::DirectAudioStaged {
                        seq,
                        track: tr,
                        start_ms,
                        start_playing,
                        result,
                    });
                });
            }
        }
        MediaSource::Local => {
            if let Some(ref lp) = state.local_playback {
                lp.request_sink_flush();
                let _ = lp.spirc.pause();
            }
            state.pending_local_load_uri = None;
            state.loading_track_uri = None;
            state.active_play_request_id = None;
            state.play_load_seq = state.play_load_seq.wrapping_add(1);
            state.direct_seq = state.play_load_seq;

            let file_path = track
                .local_path
                .as_deref()
                .map(ToOwned::to_owned)
                .or_else(|| track.uri.strip_prefix("local:").map(ToOwned::to_owned))
                .or_else(|| track.uri.strip_prefix("file://").map(ToOwned::to_owned))
                .unwrap_or_else(|| track.uri.to_string());

            if !Path::new(&file_path).is_file() {
                state.player_state.is_playing = false;
                state.emit_player(emitter);
                emitter.error(format!("Local file missing: {file_path}"));
                return;
            }
            state.player_state.is_buffering = false;
            state.player_state.audio_badge = track
                .codec_label
                .as_deref()
                .unwrap_or("LOCAL AUDIO")
                .to_string();
            state.active_direct_uri = Some(track.uri.to_string());
            state.emit_player(emitter);
            state.direct_player.load(
                state.direct_seq,
                track.uri.to_string(),
                file_path,
                start_ms,
                state.config.playback_speed,
                start_playing,
            );
            record_recent_entry(state, &track);
            prestage_upcoming_tracks(state);
        }
        MediaSource::YouTube | MediaSource::SoundCloud => {
            if let Some(ref lp) = state.local_playback {
                lp.request_sink_flush();
                let _ = lp.spirc.pause();
            }
            state.pending_local_load_uri = None;
            state.loading_track_uri = None;
            state.active_play_request_id = None;
            state.direct_player.stop();
            state.play_load_seq = state.play_load_seq.wrapping_add(1);
            let seq = state.play_load_seq;
            state.player_state.is_buffering = true;
            state.player_state.audio_badge = if track.source == MediaSource::YouTube {
                "YT BUFFERING".to_string()
            } else {
                "SC BUFFERING".to_string()
            };
            state.emit_player(emitter);

            let http = Arc::clone(&state.http);
            let paths = state.paths.clone();
            let tr = track.clone();
            let tx = int_tx.clone();
            tokio::task::spawn_local(async move {
                let result = match tr.source {
                    MediaSource::YouTube => {
                        let vid = tr.uri.strip_prefix("yt:video:").unwrap_or(&tr.uri);
                        ensure_youtube_audio_cached(&http, &paths, vid).await
                    }
                    MediaSource::SoundCloud => {
                        let id = tr.uri.strip_prefix("sc:track:").unwrap_or(&tr.uri);
                        ensure_soundcloud_audio_cached(&http, &paths, id).await
                    }
                    _ => Err("unsupported source".to_string()),
                };
                let _ = tx.send(InternalMsg::DirectAudioStaged {
                    seq,
                    track: tr,
                    start_ms,
                    start_playing,
                    result,
                });
            });
            prestage_upcoming_tracks(state);
        }
    }
}

fn record_recent_entry(state: &mut BackendState, track: &TrackSummary) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    state.recent_history.record(RecentEntry {
        uri: track.uri.to_string(),
        title: track.title.clone(),
        subtitle: track.artist.to_string(),
        cover_url: track
            .cover
            .as_ref()
            .and_then(|c| c.url.as_ref().map(|u| u.to_string())),
        duration_ms: track.duration_ms,
        played_at_secs: now,
    });
    let _ = save_recent_history(&state.paths, &state.recent_history);
}

fn maybe_fetch_lyrics(
    state: &mut BackendState,
    track: &TrackSummary,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    let uri = track.uri.to_string();
    if state.in_flight_lyrics_uri.as_deref() == Some(uri.as_str()) {
        return;
    }
    state.in_flight_lyrics_uri = Some(uri.clone());
    let loading_lyrics = LyricsState {
        track_uri: uri.clone(),
        is_loading: true,
        offset_ms: if state.current_lyrics.track_uri == uri {
            state.current_lyrics.offset_ms
        } else {
            0
        },
        ..Default::default()
    };
    state.current_lyrics = loading_lyrics.clone();
    emitter.emit(BackendEvent::LyricsLoaded(loading_lyrics));

    let http = Arc::clone(&state.http);
    let paths = state.paths.clone();
    let tr = track.clone();
    let tx = int_tx.clone();
    tokio::task::spawn_local(async move {
        let lyrics_state = fetch_lyrics_for_track(&http, &paths, &tr).await;
        let _ = tx.send(InternalMsg::LyricsFinished {
            uri,
            state: lyrics_state,
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
        InternalMsg::LocalScanFinished { tracks } => {
            state.local_lib.tracks = tracks;
            state.emit_local_status(emitter, false);
            if matches!(
                state.current_crate,
                CrateRef::Home
                    | CrateRef::LocalTracks
                    | CrateRef::LocalAlbums
                    | CrateRef::LocalArtists
                    | CrateRef::LocalFolders
            ) {
                spawn_crate_fetch(state, state.current_crate.clone(), 0, 100, int_tx, emitter);
            }
        }
        InternalMsg::MultiSearchFinished {
            search_seq,
            results,
        } => {
            if search_seq == state.search_seq {
                state.last_search_results = Some(results.clone());
                emitter.emit(BackendEvent::SearchResultsLoaded(results));
            }
        }
        InternalMsg::UrlResolved { query, result } => match result {
            Ok(UrlResolveOutcome::SingleTrack(track)) => {
                state
                    .queue
                    .set_context(query, None, vec![track.clone()], Some(0));
                state.emit_queue(emitter);
                load_single_track_locally(state, &track, 0, true, int_tx, emitter).await;
            }
            Ok(UrlResolveOutcome::Playlist {
                crate_ref,
                title,
                tracks,
            }) => {
                let total = tracks.len();
                state.current_crate = crate_ref.clone();
                state.last_crate_tracks = Some((crate_ref.clone(), total, tracks.clone()));
                emitter.emit(BackendEvent::CrateHeaderLoaded {
                    crate_ref: crate_ref.clone(),
                    title,
                    subtitle: format!("{total} tracks"),
                    cover: tracks.first().and_then(|t| t.cover.clone()),
                    total,
                });
                emitter.emit(BackendEvent::CrateTracksPageLoaded {
                    crate_ref,
                    offset: 0,
                    total,
                    items: tracks,
                });
            }
            Ok(UrlResolveOutcome::SpotifyCrate(crate_ref)) => {
                state.current_crate = crate_ref.clone();
                spawn_crate_fetch(state, crate_ref, 0, 50, int_tx, emitter);
            }
            Err(err) => emitter.error(err),
        },
        InternalMsg::DirectAudioStaged {
            seq,
            track,
            start_ms,
            start_playing,
            result,
        } => {
            if seq != state.play_load_seq {
                return;
            }
            state.player_state.is_buffering = false;
            match result {
                Ok((path, codec_label)) => {
                    state.direct_seq = seq;
                    state.active_direct_uri = Some(track.uri.to_string());
                    state.player_state.audio_badge = codec_label;
                    state.emit_player(emitter);
                    state.direct_player.load(
                        seq,
                        track.uri.to_string(),
                        path.to_string_lossy().to_string(),
                        start_ms,
                        state.config.playback_speed,
                        start_playing,
                    );
                    record_recent_entry(state, &track);
                }
                Err(err) => {
                    state.player_state.is_playing = false;
                    state.emit_player(emitter);
                    emitter.error(err);
                }
            }
        }
        InternalMsg::DirectEvent(ev) => match ev {
            DirectEvent::Playing {
                seq,
                uri: _,
                position_ms,
            } => {
                if seq != state.direct_seq {
                    return;
                }
                state.local_player_stopped = false;
                state.player_state.is_playing = true;
                state.player_state.is_buffering = false;
                state.player_state.position_ms = position_ms;
                state.emit_player(emitter);
                state.sync_mpris(false).await;
            }
            DirectEvent::Paused {
                seq,
                uri: _,
                position_ms,
            } => {
                if seq != state.direct_seq {
                    return;
                }
                state.player_state.is_playing = false;
                state.player_state.position_ms = position_ms;
                state.emit_player(emitter);
                state.sync_mpris(false).await;
            }
            DirectEvent::PositionChanged { seq, position_ms } => {
                if seq != state.direct_seq {
                    return;
                }
                // Check A-B loop wrap!
                if let (Some(a), Some(b)) =
                    (state.player_state.ab_loop.a_ms, state.player_state.ab_loop.b_ms)
                    && position_ms >= b
                    && state.last_ab_seek_at.elapsed() >= Duration::from_millis(350)
                {
                    state.last_ab_seek_at = Instant::now();
                    seek_active_ms(state, a, emitter);
                    return;
                }
                // Check SponsorBlock auto-skip when Ad-Free Shield is active
                if state.config.ad_block_enabled
                    && state.player_state.ab_loop.b_ms.is_none()
                    && state.last_sponsor_skip_at.elapsed() >= Duration::from_millis(1200)
                    && let Some(seg) = state
                        .active_sponsor_segments
                        .iter()
                        .find(|seg| {
                            !state.skipped_sponsor_starts.contains(&seg.start_ms)
                                && position_ms >= seg.start_ms
                                && position_ms.saturating_add(350) < seg.end_ms
                        })
                        .cloned()
                {
                    state.skipped_sponsor_starts.insert(seg.start_ms);
                    state.last_sponsor_skip_at = Instant::now();
                    if state.player_state.duration_ms > 0
                        && seg.end_ms.saturating_add(800) >= state.player_state.duration_ms
                    {
                        let wrap = state.player_state.repeat == RepeatMode::Context;
                        if let Some(next) = state.queue.step_next_track(wrap) {
                            emitter.emit(BackendEvent::Notice(Some(format!(
                                "Ad-Free Shield skipped ending {} segment",
                                seg.category
                            ))));
                            state.emit_queue(emitter);
                            load_single_track_locally(state, &next, 0, true, int_tx, emitter)
                                .await;
                            return;
                        }
                    }
                    emitter.emit(BackendEvent::Notice(Some(format!(
                        "Ad-Free Shield skipped {} segment",
                        seg.category
                    ))));
                    seek_active_ms(state, seg.end_ms, emitter);
                    return;
                }
                state.player_state.position_ms = position_ms;
                if let Some(ref mpris) = state.mpris {
                    mpris.update_position(position_ms);
                }
                emitter.emit(BackendEvent::PositionTick { position_ms });
            }
            DirectEvent::EndOfTrack { seq, uri: _ } => {
                if seq != state.direct_seq {
                    return;
                }
                if state.sleep_end_of_track {
                    state.sleep_end_of_track = false;
                    state.player_state.sleep_timer = Default::default();
                    state.player_state.is_playing = false;
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
                    emitter.emit(BackendEvent::Notice(Some(
                        "Sleep timer ended after track.".to_string(),
                    )));
                    return;
                }
                if state.player_state.repeat == RepeatMode::Track
                    && let Some(cur) = state.player_state.active_track.as_ref().map(|t| t.summary.clone())
                {
                    load_single_track_locally(state, &cur, 0, true, int_tx, emitter).await;
                    return;
                }
                let wrap = state.player_state.repeat == RepeatMode::Context;
                if let Some(next) = state.queue.step_next_track(wrap) {
                    state.emit_queue(emitter);
                    load_single_track_locally(state, &next, 0, true, int_tx, emitter).await;
                } else {
                    state.active_direct_uri = None;
                    state.local_player_stopped = true;
                    state.player_state.is_playing = false;
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
                }
            }
            DirectEvent::Error { seq, message } => {
                if seq != state.direct_seq {
                    return;
                }
                state.player_state.is_playing = false;
                state.player_state.is_buffering = false;
                state.emit_player(emitter);
                emitter.error(message);
            }
        },
        InternalMsg::LyricsFinished {
            uri,
            state: mut lyrics_state,
        } => {
            if state.in_flight_lyrics_uri.as_deref() == Some(uri.as_str()) {
                state.in_flight_lyrics_uri = None;
            }
            if state.current_playing_uri.as_deref() == Some(uri.as_str()) {
                if state.current_lyrics.track_uri == uri && lyrics_state.offset_ms == 0 {
                    lyrics_state.offset_ms = state.current_lyrics.offset_ms;
                }
                state.current_lyrics = lyrics_state.clone();
                emitter.emit(BackendEvent::LyricsLoaded(lyrics_state));
            }
        }
        InternalMsg::SponsorSegmentsLoaded { uri, segments } => {
            if state.current_playing_uri.as_deref() == Some(uri.as_str())
                || state.active_direct_uri.as_deref() == Some(uri.as_str())
            {
                state.active_sponsor_segments = segments;
                if state.config.ad_block_enabled
                    && state.player_state.ab_loop.b_ms.is_none()
                    && let Some(first) = state.active_sponsor_segments.first().cloned()
                    && first.start_ms <= 800
                    && state.player_state.position_ms.saturating_add(350) < first.end_ms
                {
                    state.skipped_sponsor_starts.insert(first.start_ms);
                    state.last_sponsor_skip_at = Instant::now();
                    emitter.emit(BackendEvent::Notice(Some(format!(
                        "Ad-Free Shield skipped intro {} segment",
                        first.category
                    ))));
                    seek_active_ms(state, first.end_ms, emitter);
                }
            }
        }
        InternalMsg::Mp3ExportFinished {
            uri,
            auto_index,
            result,
        } => {
            state.downloads_in_flight.remove(&uri);
            match result {
                Ok(saved_path) => {
                    if auto_index {
                        let _ = state.local_lib.index_ad_hoc_path(&state.paths, &saved_path);
                        state.emit_local_status(emitter, false);
                        if matches!(state.current_crate, CrateRef::Home | CrateRef::LocalTracks) {
                            spawn_crate_fetch(
                                state,
                                state.current_crate.clone(),
                                0,
                                100,
                                int_tx,
                                emitter,
                            );
                        }
                    }
                    let path_str = saved_path.to_string_lossy().to_string();
                    emitter.emit(BackendEvent::Mp3DownloadFinished {
                        uri,
                        saved_path: Some(path_str.clone()),
                        error: None,
                    });
                    emitter.emit(BackendEvent::Notice(Some(format!(
                        "Saved MP3: {path_str}"
                    ))));
                }
                Err(err) => {
                    emitter.emit(BackendEvent::Mp3DownloadFinished {
                        uri,
                        saved_path: None,
                        error: Some(err.clone()),
                    });
                    emitter.error(format!("MP3 save failed: {err}"));
                }
            }
        }
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
                    }
                }
                Err(err) => emitter.error(err),
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
                        "Signed in to Free Spotify ({account_type}). Local, YouTube, and SoundCloud playback remain active."
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
                .filter(|id| *id != state.device_id.as_str())
                .map(ToOwned::to_owned);
            let was_local = state.local_is_active;

            if cluster_local_active {
                state.local_is_active = true;
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

            // Never clobber active DirectAudioEngine (Local/YouTube/SoundCloud) playback!
            if !state.local_is_active && state.active_direct_uri.is_none() && remote_active_id.is_some() {
                let snap: RemotePlaybackSnapshot = snapshot_from_cluster(&cluster);
                state.player_state.is_playing = snap.is_playing;
                state.player_state.position_ms = snap.position_ms;
                state.player_state.duration_ms = snap.duration_ms;
                state.player_state.shuffle = snap.shuffle;
                state.player_state.repeat = snap.repeat;
                state.player_state.active_source = MediaSource::Spotify;
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
            let Some(mut detail) = detail else {
                return;
            };
            if session_gen != state.session_gen || state.session.is_none() {
                return;
            }
            if state.likes_store.uri_set().contains(&uri) {
                detail.summary.is_liked = true;
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
                record_recent_entry(state, &detail.summary);
            }
        }
        InternalMsg::BackendEventsBatch {
            session_gen,
            result,
        } => {
            let _ = session_gen;
            match result {
                Ok(events) => {
                    for ev in events {
                        if let BackendEvent::CrateTracksPageLoaded {
                            crate_ref,
                            offset,
                            total,
                            items,
                        } = &ev
                        {
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
                        } else if let BackendEvent::ArtistDetailLoaded(detail) = &ev {
                            state.last_artist_detail = Some(detail.clone());
                        } else if let BackendEvent::SearchResultsLoaded(search) = &ev {
                            state.last_search_results = Some(search.clone());
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
            if session_gen != state.session_gen || play_seq != state.play_load_seq {
                return;
            }
            apply_play_crate_resolved(state, shuffle, result, int_tx, emitter).await;
        }
        InternalMsg::QueueCrateResolved {
            session_gen,
            result,
        } => {
            if session_gen != state.session_gen {
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
            if session_gen != state.session_gen || play_seq != state.play_load_seq {
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
                && state.active_direct_uri.is_none()
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
                        load_single_track_locally(state, &next, 0, true, int_tx, emitter).await;
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
    // Ignore stale librespot PlayerEvents while DirectAudioEngine owns playback.
    if state.active_direct_uri.is_some() {
        return;
    }
    match ev {
        PlayerEvent::Loading {
            play_request_id,
            track_id,
            position_ms,
        } => {
            let uri_str = track_id.to_uri();
            if state.config.ad_block_enabled && uri_str.starts_with("spotify:ad:") {
                if state.ad_muted_restore_vol.is_none() {
                    state.ad_muted_restore_vol = Some(state.player_state.volume);
                }
                if let Some(ref lp) = state.local_playback {
                    lp.mixer.set_volume(0);
                    let _ = lp.spirc.next();
                }
                emitter.emit(BackendEvent::Notice(Some(
                    "Ad-Free Shield muted Spotify ad.".to_string(),
                )));
                return;
            }
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
        PlayerEvent::Stopped {
            play_request_id, ..
        } => {
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
                    return;
                }
                if state.sleep_end_of_track {
                    state.sleep_end_of_track = false;
                    state.player_state.sleep_timer = Default::default();
                    if let Some(ref lp) = state.local_playback {
                        let _ = lp.spirc.pause();
                    }
                    state.local_player_stopped = true;
                    state.player_state.is_playing = false;
                    state.player_state.position_ms = 0;
                    state.emit_player(emitter);
                    state.sync_mpris(false).await;
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
                    load_single_track_locally(state, &next, 0, true, int_tx, emitter).await;
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
            if state.config.ad_block_enabled && uri_str.starts_with("spotify:ad:") {
                if state.ad_muted_restore_vol.is_none() {
                    state.ad_muted_restore_vol = Some(state.player_state.volume);
                }
                if let Some(ref lp) = state.local_playback {
                    lp.mixer.set_volume(0);
                    let _ = lp.spirc.next();
                }
                return;
            }
            if let Some(restore_vol) = state.ad_muted_restore_vol.take()
                && let Some(ref lp) = state.local_playback
            {
                lp.mixer.set_volume(restore_vol);
            }
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
            state.player_state.active_source = MediaSource::Spotify;
            state.queue.notify_track_started(&uri_str);
            sync_active_track_for_uri(state, &uri_str, Some(&audio_item), int_tx);
            if let Some(ref tr) = state.player_state.active_track.clone() {
                maybe_fetch_lyrics(state, &tr.summary, int_tx, emitter);
            }
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
            if let (Some(a), Some(b)) =
                (state.player_state.ab_loop.a_ms, state.player_state.ab_loop.b_ms)
                && position_ms >= b
                && state.last_ab_seek_at.elapsed() >= Duration::from_millis(350)
            {
                state.last_ab_seek_at = Instant::now();
                seek_active_ms(state, a, emitter);
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
            state.direct_player.set_volume(volume);
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
                load_single_track_locally(state, &next, 0, true, int_tx, emitter).await;
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
        (CrateRef::Home, CrateRef::Home)
        | (CrateRef::LocalTracks, CrateRef::LocalTracks)
        | (CrateRef::LocalAlbums, CrateRef::LocalAlbums)
        | (CrateRef::LocalArtists, CrateRef::LocalArtists)
        | (CrateRef::LocalFolders, CrateRef::LocalFolders)
        | (CrateRef::UnifiedLikes, CrateRef::UnifiedLikes)
        | (CrateRef::CustomPlaylistsIndex, CrateRef::CustomPlaylistsIndex)
        | (CrateRef::LikedSongs, CrateRef::LikedSongs)
        | (CrateRef::PlaylistsIndex, CrateRef::PlaylistsIndex)
        | (CrateRef::AlbumsIndex, CrateRef::AlbumsIndex)
        | (CrateRef::ArtistsIndex, CrateRef::ArtistsIndex)
        | (CrateRef::Recent, CrateRef::Recent) => true,
        (CrateRef::Playlist { uri: u1, .. }, CrateRef::Playlist { uri: u2, .. })
        | (CrateRef::Album { uri: u1, .. }, CrateRef::Album { uri: u2, .. })
        | (CrateRef::Artist { uri: u1, .. }, CrateRef::Artist { uri: u2, .. }) => u1 == u2,
        (CrateRef::CustomPlaylist { id: a, .. }, CrateRef::CustomPlaylist { id: b, .. }) => a == b,
        (CrateRef::YouTubePlaylist { id: a, .. }, CrateRef::YouTubePlaylist { id: b, .. }) => {
            a == b
        }
        (CrateRef::SoundCloudSet { url: a, .. }, CrateRef::SoundCloudSet { url: b, .. }) => a == b,
        (CrateRef::Search { query: q1, .. }, CrateRef::Search { query: q2, .. }) => q1 == q2,
        _ => false,
    }
}

fn cached_crate_title_and_ctx(crate_ref: &CrateRef) -> (String, Option<String>) {
    match crate_ref {
        CrateRef::Home => ("Home".to_string(), None),
        CrateRef::LocalTracks => ("Local Music".to_string(), Some("local_tracks".to_string())),
        CrateRef::LocalAlbums => ("Local Albums".to_string(), None),
        CrateRef::LocalArtists => ("Local Artists".to_string(), None),
        CrateRef::LocalFolders => ("Music Folders".to_string(), None),
        CrateRef::LocalAlbum { title, .. } => (title.clone(), None),
        CrateRef::LocalArtist { name } => (name.clone(), None),
        CrateRef::LocalFolder { path } => (path.clone(), None),
        CrateRef::UnifiedLikes => ("Favorites".to_string(), Some("unified_likes".to_string())),
        CrateRef::CustomPlaylistsIndex => ("Playlists".to_string(), None),
        CrateRef::CustomPlaylist { id, name } => {
            (name.clone(), Some(format!("custom_playlist:{id}")))
        }
        CrateRef::YouTubePlaylist { title, .. } => (title.clone(), None),
        CrateRef::SoundCloudSet { title, .. } => (title.clone(), None),
        CrateRef::LikedSongs => ("Liked Songs".to_string(), None),
        CrateRef::PlaylistsIndex => ("Playlists".to_string(), None),
        CrateRef::AlbumsIndex => ("Albums".to_string(), None),
        CrateRef::ArtistsIndex => ("Artists".to_string(), None),
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
        CrateRef::Search { query, .. } => (format!("Search: {query}"), None),
    }
}

fn cached_tracks_for_crate(
    state: &BackendState,
    crate_ref: &CrateRef,
    required_track_uri: Option<&str>,
) -> Option<(String, Option<String>, Vec<TrackSummary>)> {
    if let Some((cached_ref, total, tracks)) = state.last_crate_tracks.as_ref()
        && crate_refs_same_target(cached_ref, crate_ref)
        && !tracks.is_empty()
    {
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
        && required_track_uri
            .is_none_or(|u| detail.top_tracks.iter().any(|t| t.uri.as_ref() == u))
    {
        return Some((
            detail.artist.name.clone(),
            Some(detail.artist.uri.to_string()),
            detail.top_tracks.clone(),
        ));
    }
    if let CrateRef::Search { query, .. } = crate_ref
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
) -> Option<Vec<TrackSummary>> {
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
    result: Result<(String, Option<String>, Vec<TrackSummary>), String>,
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
                state
                    .queue
                    .set_context(title, ctx_arc, tracks.clone(), None);
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

            if !is_local
                && selected.source == MediaSource::Spotify
                && let Some(ctx_uri) = maybe_ctx_uri
            {
                set_optimistic_playing_track(state, &selected, int_tx, emitter);
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
                .await;
            } else {
                load_single_track_locally(state, &selected, 0, true, int_tx, emitter).await;
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
    tracks: Result<Vec<TrackSummary>, String>,
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
                if !is_local && selected.source == MediaSource::Spotify && has_real_ctx {
                    set_optimistic_playing_track(state, &selected, int_tx, emitter);
                    load_context_on_active_device(
                        state,
                        &context_uri,
                        Some(track_uri),
                        state.player_state.shuffle,
                        int_tx,
                        emitter,
                    )
                    .await;
                } else {
                    load_single_track_locally(state, &selected, 0, true, int_tx, emitter).await;
                }
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
    if let Some(resolved) = resolve_non_spotify_crate(state, &crate_ref) {
        apply_play_crate_resolved(state, shuffle, Ok(resolved), int_tx, emitter).await;
        return;
    }
    if let Some(cached) = cached_tracks_for_crate(state, &crate_ref, None) {
        apply_play_crate_resolved(state, shuffle, Ok(cached), int_tx, emitter).await;
        return;
    }
    let Some(sess) = state.session.clone() else {
        emitter.error("Sign in with Spotify Premium first.");
        return;
    };
    state.play_load_seq = state.play_load_seq.wrapping_add(1);
    let play_seq = state.play_load_seq;
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
    let Some((chosen, _is_manual)) = state.queue.play_queue_index(index) else {
        return false;
    };
    state.emit_queue(emitter);
    load_single_track_locally(state, &chosen, 0, true, int_tx, emitter).await;
    true
}

fn track_detail_from_summary(summary: &TrackSummary) -> TrackDetail {
    TrackDetail {
        summary: summary.clone(),
        track_number: None,
        disc_total_tracks: None,
        release_year: None,
        explicit: false,
    }
}

fn cached_track_summary(state: &BackendState, uri: &str) -> Option<TrackSummary> {
    if let Some(t) = state.queue.find_track(uri) {
        return Some(t.clone());
    }
    if let Some(t) = state.likes_store.tracks.iter().find(|t| t.uri.as_ref() == uri) {
        return Some(t.clone());
    }
    if let Some(t) = state
        .local_lib
        .tracks
        .iter()
        .find(|t| format!("local:{}", t.path) == uri)
    {
        return Some(t.to_summary(&state.likes_store.uri_set()));
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
        summary: TrackSummary {
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
            source: MediaSource::Spotify,
            local_path: None,
            codec_label: None,
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
    summary: &TrackSummary,
    int_tx: &mpsc::UnboundedSender<InternalMsg>,
    emitter: &Emitter,
) {
    state.local_is_active = state.is_remote_active().is_none();
    state.local_player_stopped = false;
    state.current_playing_uri = Some(summary.uri.to_string());
    state.queue.set_current_track(Some(summary.clone()));
    state.player_state.active_source = summary.source;
    state.player_state.active_track = Some(track_detail_from_summary(summary));
    state.player_state.position_ms = 0;
    state.player_state.duration_ms = summary.duration_ms;
    state.player_state.is_playing = true;
    state.emit_player(emitter);
    maybe_fetch_lyrics(state, summary, int_tx, emitter);
    if summary.source == MediaSource::Spotify {
        maybe_fetch_track_detail(state, summary.uri.as_ref(), int_tx);
    }
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
    let upcoming: Vec<TrackSummary> = state
        .queue
        .manual_items()
        .iter()
        .chain(state.queue.context_upcoming().iter())
        .take(2)
        .cloned()
        .collect();

    let sp_uris: Vec<String> = upcoming
        .iter()
        .filter(|t| t.source == MediaSource::Spotify)
        .map(|t| t.uri.to_string())
        .collect();
    if !sp_uris.is_empty()
        && let Some(ref sess) = state.session
    {
        prestage_track_uris(sess, sp_uris);
    }

    for tr in upcoming {
        if tr.source == MediaSource::YouTube || tr.source == MediaSource::SoundCloud {
            let http = Arc::clone(&state.http);
            let paths = state.paths.clone();
            tokio::task::spawn_local(async move {
                match tr.source {
                    MediaSource::YouTube => {
                        let vid = tr.uri.strip_prefix("yt:video:").unwrap_or(&tr.uri);
                        let _ = ensure_youtube_audio_cached(&http, &paths, vid).await;
                    }
                    MediaSource::SoundCloud => {
                        let id = tr.uri.strip_prefix("sc:track:").unwrap_or(&tr.uri);
                        let _ = ensure_soundcloud_audio_cached(&http, &paths, id).await;
                    }
                    _ => {}
                }
            });
        }
    }
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
    let effective_mb = state.config.disk_audio_cache_mb.max(32);
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
    let request =
        build_context_load_request(ctx_uri, None, false, RepeatMode::Off, true, 0);
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
            .filter(|t| t.source == MediaSource::Spotify)
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
}

async fn resolve_uris_via_track_api(
    session: &Session,
    uris: &[String],
) -> Result<Vec<TrackSummary>, String> {
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
            out.push(TrackSummary {
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
                source: MediaSource::Spotify,
                local_path: None,
                codec_label: None,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_model::{NormalisationMode, StreamQuality, TrackSummary};

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
            source: MediaSource::Spotify,
            local_path: None,
            codec_label: None,
        }
    }

    #[test]
    fn config_round_trip_and_device_id_stability() {
        let root = temp_root("cfg");
        let paths = AppPaths::for_root(&root);
        let cfg = AppConfig {
            device_name: "Aplayer Gentoo".to_string(),
            stream_quality: StreamQuality::Kbps160,
            normalisation: NormalisationMode::Track,
            disk_audio_cache_mb: 128,
            disk_cover_cache_mb: 16,
            ..Default::default()
        };
        save_config(&paths, &cfg).unwrap();

        let loaded = load_config(&paths);
        assert_eq!(loaded.device_name, "Aplayer Gentoo");
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
        q.append(mk_track("m1"));
        q.append(mk_track("m2"));
        q.enqueue_next(mk_track("m0"));
        assert!(q.reorder_manual(2, 0));
        assert!(q.promote_context_to_manual(1, 3).is_some());
        let (played, is_manual) = q.play_queue_index(1).unwrap();
        assert!(is_manual);
        assert_eq!(&*played.uri, "spotify:track:m0");
        q.remove_manual(0);
        q.clear_manual();
        assert!(q.manual_items().is_empty());
    }

    #[test]
    fn proc_status_parser_extracts_rss_and_hwm() {
        let sample = "\
Name:\taplayer\n\
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
    #[ignore]
    fn e2e_live_local_youtube_soundcloud_lrclib() {
        let root = temp_root("e2e_live");
        let music_dir = root.join("music");
        fs::create_dir_all(&music_dir).unwrap();
        let flac_path = music_dir.join("e2e_sine.flac");
        let opus_path = music_dir.join("e2e_sine.ogg");
        let lrc_path = music_dir.join("e2e_sine.lrc");

        let status = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-metadata",
                "title=E2E Master Signal",
                "-metadata",
                "artist=Aplayer Verification",
                "-metadata",
                "album=Runtime Suite",
                flac_path.to_str().unwrap(),
            ])
            .status()
            .expect("ffmpeg flac encode");
        assert!(status.success());

        let status_ogg = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=523:duration=3",
                "-c:a",
                "libopus",
                "-metadata",
                "title=E2E Opus Stream",
                "-metadata",
                "artist=Aplayer Verification",
                opus_path.to_str().unwrap(),
            ])
            .status()
            .expect("ffmpeg opus encode");
        assert!(status_ogg.success());

        fs::write(&lrc_path, "[00:00.50]Line One\n[00:02.00]Line Two\n").unwrap();

        let paths = AppPaths::for_root(&root);
        paths.ensure_dirs().unwrap();
        let rescan = LocalLibrary::scan_dirs_blocking(
            &paths,
            &[music_dir.to_string_lossy().to_string()],
            &[],
        );
        assert_eq!(rescan.len(), 2, "expected flac + ogg indexed");
        assert!(crate::local_lib::read_local_lyrics(&flac_path).is_some());

        // Exercise DirectAudioEngine with muted volume so speakers remain silent while DSP runs.
        let dsp = Arc::new(SharedDsp::new(pulse_model::EqConfig::default(), true));
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let engine = DirectAudioEngine::spawn(Arc::clone(&dsp), ev_tx, 0, None);
        engine.load(
            1,
            format!("local:{}", flac_path.display()),
            flac_path.to_string_lossy().to_string(),
            0,
            pulse_model::PlaybackSpeed::default(),
            true,
        );
        std::thread::sleep(Duration::from_millis(450));
        let mut saw_playing = false;
        while let Ok(ev) = ev_rx.try_recv() {
            if matches!(ev, DirectEvent::Playing { .. } | DirectEvent::PositionChanged { .. }) {
                saw_playing = true;
            }
        }
        assert!(saw_playing, "DirectAudioEngine should start decoding/playing");
        let spec = dsp.snapshot_spectrum();
        assert!(spec.active, "DSP analyzer should populate spectrum bins");

        engine.seek_ms(1500);
        std::thread::sleep(Duration::from_millis(200));
        engine.stop();
        drop(engine);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let http = HttpClient::new(None);
            let liked = HashSet::new();

            let yt = search_youtube(&http, "Daft Punk Around the World", &liked).await;
            assert!(!yt.is_empty(), "YouTube search should return tracks");
            let vid = yt[0].uri.strip_prefix("yt:video:").unwrap().to_string();
            let resolved_yt = resolve_youtube_video(&http, &vid, &liked).await;
            assert!(
                resolved_yt.is_ok(),
                "YouTube video resolution should succeed for {vid}: {:?}",
                resolved_yt.err()
            );

            // Live YouTube audio cache + SponsorBlock + MP3 transcode + ID3 verification
            let (yt_cached, yt_codec) = ensure_youtube_audio_cached(&http, &paths, &vid)
                .await
                .expect("YouTube audio cache stage");
            let sb_segs = fetch_youtube_skip_segments(&http, &vid).await;
            let yt_track = resolved_yt.unwrap();
            let mp3_out = root.join("yt_e2e_export.mp3");
            let filter = build_sponsor_aselect_filter(&sb_segs, yt_track.duration_ms);
            let cover = fetch_youtube_cover_jpeg(&http, &vid).await;
            let saved_mp3 = transcode_and_tag_mp3_blocking(
                &yt_cached,
                &mp3_out,
                pulse_model::Mp3Quality::Cbr192,
                filter.as_deref(),
                &yt_track,
                &vid,
                cover.as_deref(),
            )
            .expect("live YouTube MP3 export");
            let ffprobe_out = std::process::Command::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=codec_name:format=duration",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&saved_mp3)
                .output()
                .expect("ffprobe mp3");
            let ffprobe_txt = String::from_utf8_lossy(&ffprobe_out.stdout);
            assert!(
                ffprobe_txt.contains("codec_name=mp3"),
                "expected mp3 stream, got {ffprobe_txt}"
            );

            let (sc_tracks, _) = search_soundcloud(&http, &paths, "electronic ambient", &liked).await;
            assert!(!sc_tracks.is_empty(), "SoundCloud search should return tracks");

            let query_tr = TrackSummary {
                uri: Arc::from("spotify:track:test"),
                title: "Around the World".to_string(),
                artist: Arc::from("Daft Punk"),
                artist_uri: None,
                album: Arc::from("Homework"),
                album_uri: None,
                cover: None,
                duration_ms: 429_000,
                is_liked: false,
                is_playable: true,
                added_at_unix: None,
                source: MediaSource::Spotify,
                local_path: None,
                codec_label: None,
            };
            let lyr = fetch_lyrics_for_track(&http, &paths, &query_tr).await;
            assert!(!lyr.lines.is_empty(), "LRCLIB live fetch should return lyrics lines");
            println!(
                "E2E OK: local={} yt={} yt_codec={} sb_segs={} mp3_bytes={} sc={} lrclib_lines={}",
                rescan.len(),
                yt.len(),
                yt_codec,
                sb_segs.len(),
                fs::metadata(&saved_mp3).map(|m| m.len()).unwrap_or(0),
                sc_tracks.len(),
                lyr.lines.len()
            );
        });

        let _ = fs::remove_dir_all(&root);
        let pgrep = std::process::Command::new("pgrep")
            .args(["-fa", "e2e_sine"])
            .output();
        if let Ok(out) = pgrep {
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(stdout.trim().is_empty(), "leftover ffmpeg process: {stdout}");
        }
    }
}
