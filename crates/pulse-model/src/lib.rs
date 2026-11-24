//! Shared, dependency-light UI <-> backend contract and memory-bounded data models.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Top-level window presentation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AppMode {
    #[default]
    Workbench,
    FocusStage,
    MiniPlayer,
}

/// Origin platform or local library backing a track or collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MediaSource {
    #[default]
    Local,
    Spotify,
    YouTube,
    SoundCloud,
}

impl MediaSource {
    /// Infers the [`MediaSource`] from a track or collection URI.
    pub fn from_uri(uri: &str) -> Self {
        let trimmed = uri.trim();
        if trimmed.starts_with("yt:")
            || trimmed.contains("youtube.com/")
            || trimmed.contains("youtu.be/")
        {
            Self::YouTube
        } else if trimmed.starts_with("sc:") || trimmed.contains("soundcloud.com/") {
            Self::SoundCloud
        } else if trimmed.starts_with("local:")
            || trimmed.starts_with("file://")
            || trimmed.starts_with('/')
        {
            Self::Local
        } else {
            Self::Spotify
        }
    }

    /// Crisp pill/badge label for UI source tags.
    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Spotify => "SPOTIFY",
            Self::YouTube => "YOUTUBE",
            Self::SoundCloud => "SOUNDCLOUD",
        }
    }

    /// Human-readable platform name.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Spotify => "Spotify",
            Self::YouTube => "YouTube",
            Self::SoundCloud => "SoundCloud",
        }
    }
}

/// Theme brightness preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    Dark,
    Light,
    System,
}

/// Dynamic palette accent driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AccentSource {
    #[default]
    AlbumReactive,
    MatugenSystem,
    WarmAmber,
}

/// Audio visualizer mode shown in the playback deck and Stage view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VisualizerMode {
    #[default]
    Spectrum,
    Waveform,
    VuMeters,
    Off,
}

impl VisualizerMode {
    pub const fn next(self) -> Self {
        match self {
            Self::Spectrum => Self::Waveform,
            Self::Waveform => Self::VuMeters,
            Self::VuMeters => Self::Off,
            Self::Off => Self::Spectrum,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "Spectrum",
            Self::Waveform => "Waveform",
            Self::VuMeters => "VU Meters",
            Self::Off => "Paused",
        }
    }
}

/// Slide-in studio drawer panel tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RightDrawerTab {
    #[default]
    Queue,
    Lyrics,
    Equalizer,
    Insights,
    Closed,
}

/// 10-band equalizer frequencies (Hz) for `[31, 62, 125, 250, 500, 1k, 2k, 4k, 8k, 16k]`.
pub const EQ_BAND_FREQS_HZ: [f32; 10] = [
    31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// Compact labels for each of the 10 EQ bands.
pub const EQ_BAND_LABELS: [&str; 10] = [
    "31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

/// Curated 10-band EQ presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EqPreset {
    #[default]
    Flat,
    BassBoost,
    TrebleCrisp,
    VocalPresence,
    Electronic,
    Rock,
    Acoustic,
    LateNight,
    Custom,
}

impl EqPreset {
    pub const ALL: [Self; 9] = [
        Self::Flat,
        Self::BassBoost,
        Self::TrebleCrisp,
        Self::VocalPresence,
        Self::Electronic,
        Self::Rock,
        Self::Acoustic,
        Self::LateNight,
        Self::Custom,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Flat => "Flat",
            Self::BassBoost => "Bass Boost",
            Self::TrebleCrisp => "Treble Crisp",
            Self::VocalPresence => "Vocal Presence",
            Self::Electronic => "Electronic",
            Self::Rock => "Rock",
            Self::Acoustic => "Acoustic",
            Self::LateNight => "Late Night",
            Self::Custom => "Custom",
        }
    }

    pub const fn gains_db(self) -> [f32; 10] {
        match self {
            Self::Flat | Self::Custom => [0.0; 10],
            Self::BassBoost => [5.5, 4.5, 3.0, 1.5, 0.0, -0.5, 0.0, 0.5, 1.0, 1.0],
            Self::TrebleCrisp => [-1.0, -0.5, 0.0, 0.0, 0.5, 1.0, 2.5, 4.0, 5.0, 5.5],
            Self::VocalPresence => [-2.0, -1.5, -0.5, 1.0, 2.5, 4.0, 3.5, 2.0, 0.5, -0.5],
            Self::Electronic => [4.5, 3.5, 1.5, 0.0, -1.5, 1.0, 2.0, 3.5, 4.5, 4.0],
            Self::Rock => [4.0, 3.0, 1.5, -0.5, -1.0, 0.5, 2.0, 3.5, 4.0, 3.5],
            Self::Acoustic => [2.5, 2.0, 1.0, 0.5, 1.5, 2.0, 2.5, 3.0, 2.5, 1.5],
            Self::LateNight => [-4.0, -2.5, 0.0, 1.5, 2.5, 2.5, 1.5, 0.0, -2.0, -3.5],
        }
    }
}

/// Real-time 10-band parametric EQ, preamp, balance, and mono downmix settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqConfig {
    pub enabled: bool,
    pub preset: EqPreset,
    pub bands_db: [f32; 10],
    pub preamp_db: f32,
    pub balance: f32,
    pub mono: bool,
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: EqPreset::Flat,
            bands_db: [0.0; 10],
            preamp_db: 0.0,
            balance: 0.0,
            mono: false,
        }
    }
}

/// Playback speed and pitch behavior.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaybackSpeed {
    pub rate: f32,
    pub preserve_pitch: bool,
}

impl Default for PlaybackSpeed {
    fn default() -> Self {
        Self {
            rate: 1.0,
            preserve_pitch: true,
        }
    }
}

/// A-B section loop state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AbLoopState {
    pub a_ms: Option<u32>,
    pub b_ms: Option<u32>,
}

/// Sleep timer command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleepTimerTarget {
    Off,
    Minutes(u16),
    EndOfTrack,
}

/// Active sleep timer countdown snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SleepTimerState {
    pub remaining_secs: Option<u32>,
    pub end_of_track: bool,
}

/// Single lyric line (with optional LRC timestamp).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_ms: Option<u32>,
    pub text: String,
}

/// Synced or plain lyrics payload for the active track.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LyricsState {
    pub track_uri: String,
    pub synced: bool,
    pub lines: Vec<LyricLine>,
    pub offset_ms: i32,
    pub provider: String,
    pub is_loading: bool,
    pub error: Option<String>,
}

/// Cross-source custom playlist summary card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomPlaylistSummary {
    pub id: String,
    pub name: String,
    pub track_count: usize,
    pub total_duration_ms: u64,
    pub cover: Option<ImageRef>,
}

/// Real local playback analytics snapshot (never fabricated).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListeningStatsSnapshot {
    pub total_listen_secs: u64,
    pub total_plays: u64,
    /// `[Local, Spotify, YouTube, SoundCloud]` seconds listened.
    pub source_secs: [u64; 4],
    pub top_tracks: Vec<(TrackSummary, u32)>,
    pub top_artists: Vec<(String, u32)>,
}

/// Live audio analyzer bars + waveform + L/R VU levels (`0.0..=1.0`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumSnapshot {
    pub bins: Vec<f32>,
    pub waveform: Vec<f32>,
    pub peak_l: f32,
    pub peak_r: f32,
    pub active: bool,
}

impl Default for SpectrumSnapshot {
    fn default() -> Self {
        Self {
            bins: vec![0.0; 48],
            waveform: vec![0.0; 64],
            peak_l: 0.0,
            peak_r: 0.0,
            active: false,
        }
    }
}

/// Spotify account subscription tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AccountTier {
    Premium,
    Free,
    #[default]
    Unknown,
}

/// Authentication and session connection state surfaced to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthState {
    #[default]
    Unauthenticated,
    WaitingOAuth {
        auth_url: String,
    },
    ConnectOnly {
        device_name: String,
    },
    Authenticated {
        username: String,
        display_name: String,
        tier: AccountTier,
        web_api_ready: bool,
        zeroconf_only: bool,
    },
}

/// Session transport / network connectivity status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Offline {
        retry_in_secs: u8,
        cached_playable: bool,
    },
    Error(String),
}

/// Ogg Vorbis streaming bitrate target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StreamQuality {
    Kbps96,
    Kbps160,
    #[default]
    Kbps320,
}

impl StreamQuality {
    pub const fn bitrate_kbps(self) -> u16 {
        match self {
            Self::Kbps96 => 96,
            Self::Kbps160 => 160,
            Self::Kbps320 => 320,
        }
    }

    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::Kbps96 => "OGG 96",
            Self::Kbps160 => "OGG 160",
            Self::Kbps320 => "OGG 320",
        }
    }
}

/// Volume normalisation mode for `spotatui-librespot-playback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NormalisationMode {
    #[default]
    Off,
    Track,
    Album,
}

/// Repeat state cycling order: Off -> Context -> Track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RepeatMode {
    #[default]
    Off,
    Context,
    Track,
}

impl RepeatMode {
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::Context,
            Self::Context => Self::Track,
            Self::Track => Self::Off,
        }
    }
}

/// Target MP3 encoding profile for saving YouTube audio locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Mp3Quality {
    #[default]
    Cbr320,
    VbrV0,
    Cbr192,
    Cbr128,
}

impl Mp3Quality {
    pub const ALL: [Self; 4] = [Self::Cbr320, Self::VbrV0, Self::Cbr192, Self::Cbr128];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cbr320 => "320 kbps",
            Self::VbrV0 => "V0 VBR",
            Self::Cbr192 => "192 kbps",
            Self::Cbr128 => "128 kbps",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Cbr320 => "320k",
            Self::VbrV0 => "V0",
            Self::Cbr192 => "192k",
            Self::Cbr128 => "128k",
        }
    }
}

/// Timed non-music or sponsor interval returned by SponsorBlock for YouTube tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorSegment {
    pub start_ms: u32,
    pub end_ms: u32,
    pub category: String,
}

/// Active library browsing scope in the navigation rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LibraryScope {
    #[default]
    Home,
    LocalMusic,
    UnifiedLikes,
    CustomPlaylists,
    Recent,
    Search,
    LikedSongs,
    Playlists,
    Albums,
    Artists,
}

/// Sub-tab inside the Local Music workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LocalSubView {
    #[default]
    Tracks,
    Albums,
    Artists,
    Folders,
}

/// Search category filter chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SearchFilter {
    #[default]
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

/// Track/crate sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortOrder {
    #[default]
    Custom,
    RecentlyAdded,
    Title,
    Artist,
    Duration,
}

/// Settings modal tab identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    SourcesAndFolders,
    AccountAndConnect,
    AudioAndDesktop,
    MemoryAndCache,
}

/// Persistent user configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub custom_client_id: Option<String>,
    pub device_name: String,
    pub zeroconf_enabled: bool,
    pub audio_device: Option<String>,
    pub stream_quality: StreamQuality,
    pub normalisation: NormalisationMode,
    pub gapless: bool,
    pub prefetch_bitrate: bool,
    pub mpris_enabled: bool,
    pub media_keys_enabled: bool,
    pub confirm_clear_queue: bool,
    pub disk_audio_cache_mb: u64,
    pub disk_cover_cache_mb: u64,
    pub local_music_dirs: Vec<String>,
    pub theme_preference: ThemePreference,
    pub accent_source: AccentSource,
    pub visualizer_mode: VisualizerMode,
    pub eq: EqConfig,
    pub playback_speed: PlaybackSpeed,
    pub crossfade_ms: u16,
    pub resume_session_on_startup: bool,
    pub ad_block_enabled: bool,
    pub mp3_download_dir: String,
    pub mp3_quality: Mp3Quality,
    pub mp3_auto_add_to_library: bool,
    pub mp3_strip_sponsors: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            custom_client_id: None,
            device_name: String::from("Aplayer"),
            zeroconf_enabled: true,
            audio_device: None,
            stream_quality: StreamQuality::Kbps320,
            normalisation: NormalisationMode::Off,
            gapless: true,
            prefetch_bitrate: true,
            mpris_enabled: true,
            media_keys_enabled: true,
            confirm_clear_queue: true,
            disk_audio_cache_mb: 512,
            disk_cover_cache_mb: 64,
            local_music_dirs: Vec::new(),
            theme_preference: ThemePreference::Dark,
            accent_source: AccentSource::AlbumReactive,
            visualizer_mode: VisualizerMode::Spectrum,
            eq: EqConfig::default(),
            playback_speed: PlaybackSpeed::default(),
            crossfade_ms: 0,
            resume_session_on_startup: true,
            ad_block_enabled: true,
            mp3_download_dir: String::new(),
            mp3_quality: Mp3Quality::Cbr320,
            mp3_auto_add_to_library: true,
            mp3_strip_sponsors: true,
        }
    }
}

/// Cover artwork identifier shared between backend metadata and `pulse-assets`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageRef {
    /// Stable asset key (Spotify file ID hex, YouTube ID, or hashed cover key).
    pub id: Arc<str>,
    /// Optional direct CDN URL or `file://` local artwork path.
    pub url: Option<Arc<str>>,
}

/// Compact track representation used across Local, Spotify, YouTube, and SoundCloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackSummary {
    pub uri: Arc<str>,
    pub title: String,
    pub artist: Arc<str>,
    pub artist_uri: Option<Arc<str>>,
    pub album: Arc<str>,
    pub album_uri: Option<Arc<str>>,
    pub cover: Option<ImageRef>,
    pub duration_ms: u32,
    pub is_liked: bool,
    pub is_playable: bool,
    pub added_at_unix: Option<i64>,
    #[serde(default)]
    pub source: MediaSource,
    #[serde(default)]
    pub local_path: Option<Arc<str>>,
    #[serde(default)]
    pub codec_label: Option<Arc<str>>,
}

/// Extended track metadata for the active Stage / Focus view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackDetail {
    pub summary: TrackSummary,
    pub track_number: Option<u32>,
    pub disc_total_tracks: Option<u32>,
    pub release_year: Option<u16>,
    pub explicit: bool,
}

/// Navigation target within the main library canvas.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CrateRef {
    #[default]
    Home,
    LocalTracks,
    LocalAlbums,
    LocalArtists,
    LocalFolders,
    LocalAlbum {
        title: String,
        artist: String,
    },
    LocalArtist {
        name: String,
    },
    LocalFolder {
        path: String,
    },
    UnifiedLikes,
    CustomPlaylistsIndex,
    CustomPlaylist {
        id: String,
        name: String,
    },
    YouTubePlaylist {
        id: String,
        title: String,
    },
    SoundCloudSet {
        url: String,
        title: String,
    },
    LikedSongs,
    PlaylistsIndex,
    AlbumsIndex,
    ArtistsIndex,
    Recent,
    Playlist {
        uri: Arc<str>,
        name: String,
    },
    Album {
        uri: Arc<str>,
        title: String,
    },
    Artist {
        uri: Arc<str>,
        name: String,
    },
    Search {
        query: String,
        #[serde(default)]
        source_filter: Option<MediaSource>,
    },
}

/// Compact playlist card metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub uri: Arc<str>,
    pub name: String,
    pub owner: Arc<str>,
    pub track_count: usize,
    pub total_duration_ms: u64,
    pub cover: Option<ImageRef>,
    pub is_collaborative: bool,
    pub source: MediaSource,
}

/// Compact album card metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumSummary {
    pub uri: Arc<str>,
    pub title: String,
    pub artist: Arc<str>,
    pub artist_uri: Option<Arc<str>>,
    pub release_year: Option<u16>,
    pub track_count: usize,
    pub cover: Option<ImageRef>,
    pub album_type: Arc<str>,
    pub source: MediaSource,
}

/// Compact artist card metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistSummary {
    pub uri: Arc<str>,
    pub name: String,
    pub cover: Option<ImageRef>,
    pub is_followed: bool,
    pub source: MediaSource,
}

/// Expanded artist page payload (top tracks + discography sections).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDetail {
    pub artist: ArtistSummary,
    pub top_tracks: Vec<TrackSummary>,
    pub albums: Vec<AlbumSummary>,
    pub singles: Vec<AlbumSummary>,
}

/// Search response grouped across Local, YouTube, SoundCloud, and Spotify.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub query: String,
    pub source_filter: Option<MediaSource>,
    pub top_match_uri: Option<Arc<str>>,
    pub artists: Vec<ArtistSummary>,
    pub albums: Vec<AlbumSummary>,
    pub playlists: Vec<PlaylistSummary>,
    pub tracks: Vec<TrackSummary>,
    pub tracks_only_fallback: bool,
}

/// Local or remote Spotify Connect playback target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub is_active: bool,
    pub is_local: bool,
    /// Librespot / Spotify Connect raw volume scale (`0..=65535`).
    pub volume: u16,
    pub supports_volume: bool,
    pub is_restricted: bool,
}

impl DeviceInfo {
    pub fn volume_pct(&self) -> u8 {
        ((u32::from(self.volume) * 100 + 32_767) / 65_535) as u8
    }
}

/// Two-tier queue snapshot exposed to the Queue drawer and Stage view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueState {
    pub manual_items: Vec<TrackSummary>,
    pub context_items: Vec<TrackSummary>,
    pub context_name: String,
    pub context_uri: Option<Arc<str>>,
}

/// Active playback transport and audio metadata state.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub is_playing: bool,
    pub is_buffering: bool,
    pub active_source: MediaSource,
    pub active_track: Option<TrackDetail>,
    pub cover_accent_rgba: [u8; 4],
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Raw volume (`0..=65535`).
    pub volume: u16,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub audio_badge: String,
    pub sample_rate_hz: u32,
    pub bitrate_kbps: u16,
    pub ab_loop: AbLoopState,
    pub sleep_timer: SleepTimerState,
    pub playback_speed: PlaybackSpeed,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            is_playing: false,
            is_buffering: false,
            active_source: MediaSource::Local,
            active_track: None,
            cover_accent_rgba: [229, 154, 62, 255],
            position_ms: 0,
            duration_ms: 0,
            volume: 49_151,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            audio_badge: String::from("READY"),
            sample_rate_hz: 44_100,
            bitrate_kbps: 320,
            ab_loop: AbLoopState::default(),
            sleep_timer: SleepTimerState::default(),
            playback_speed: PlaybackSpeed::default(),
        }
    }
}

impl PlayerState {
    pub fn volume_pct(&self) -> u8 {
        ((u32::from(self.volume) * 100 + 32_767) / 65_535) as u8
    }
}

/// Device switcher and Zeroconf advertising state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DevicesState {
    pub active_device_id: Option<String>,
    pub local_device_id: String,
    pub local_device_name: String,
    pub zeroconf_active: bool,
    pub available_devices: Vec<DeviceInfo>,
}

/// Page-backed sparse collection keeping large libraries bounded in RAM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SparsePagedList<T> {
    total: usize,
    page_size: usize,
    pages: BTreeMap<usize, Vec<T>>,
    in_flight: BTreeSet<usize>,
}

impl<T> Default for SparsePagedList<T> {
    fn default() -> Self {
        Self::new(50)
    }
}

impl<T> SparsePagedList<T> {
    pub fn new(page_size: usize) -> Self {
        Self {
            total: 0,
            page_size: page_size.max(1),
            pages: BTreeMap::new(),
            in_flight: BTreeSet::new(),
        }
    }

    pub fn clear(&mut self) {
        self.total = 0;
        self.pages.clear();
        self.in_flight.clear();
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn set_total(&mut self, total: usize) {
        self.total = total;
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn loaded_count(&self) -> usize {
        self.pages.values().map(Vec::len).sum()
    }

    pub fn page_start_for(&self, index: usize) -> usize {
        index - (index % self.page_size)
    }

    pub fn is_page_loaded(&self, page_start: usize) -> bool {
        self.pages.contains_key(&page_start)
    }

    pub fn mark_in_flight(&mut self, page_start: usize) -> bool {
        if self.pages.contains_key(&page_start) {
            return false;
        }
        self.in_flight.insert(page_start)
    }

    pub fn clear_in_flight(&mut self, page_start: usize) {
        self.in_flight.remove(&page_start);
    }

    pub fn clear_all_in_flight(&mut self) {
        self.in_flight.clear();
    }

    pub fn insert_page(&mut self, offset: usize, mut items: Vec<T>, total: usize) {
        self.total = total;
        self.in_flight.remove(&offset);
        self.in_flight.retain(|&start| start < total);

        self.pages.retain(|&page_start, _| page_start < total);
        for (&page_start, page_items) in &mut self.pages {
            let max_len = total.saturating_sub(page_start);
            if page_items.len() > max_len {
                page_items.truncate(max_len);
            }
        }

        if total == 0 {
            self.pages.clear();
            return;
        }
        if offset < total {
            let max_len = total.saturating_sub(offset);
            if items.len() > max_len {
                items.truncate(max_len);
            }
            if !items.is_empty() {
                self.pages.insert(offset, items);
            }
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.total {
            return None;
        }
        let (&page_start, items) = self.pages.range(..=index).next_back()?;
        let local_idx = index.checked_sub(page_start)?;
        items.get(local_idx)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.total {
            return None;
        }
        let (&page_start, items) = self.pages.range_mut(..=index).next_back()?;
        let local_idx = index.checked_sub(page_start)?;
        items.get_mut(local_idx)
    }

    pub fn iter_loaded(&self) -> impl Iterator<Item = (usize, &T)> {
        let total = self.total;
        self.pages.iter().flat_map(move |(&start, items)| {
            items
                .iter()
                .enumerate()
                .filter(move |(i, _)| start + *i < total)
                .map(move |(i, item)| (start + i, item))
        })
    }

    pub fn evict_outside_window(&mut self, center_index: usize, keep_pages_radius: usize) {
        let center_start = self.page_start_for(center_index);
        let span = self.page_size.saturating_mul(keep_pages_radius);
        let min_keep = center_start.saturating_sub(span);
        let max_keep = center_start.saturating_add(span);
        self.pages
            .retain(|&page_start, _| page_start >= min_keep && page_start <= max_keep);
    }
}

/// Main library canvas state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateViewState {
    pub active_scope: LibraryScope,
    pub active_crate: CrateRef,
    pub local_sub_view: LocalSubView,
    pub title: String,
    pub subtitle: String,
    pub cover: Option<ImageRef>,
    pub history: Vec<CrateRef>,
    pub history_index: usize,
    pub playlists: SparsePagedList<PlaylistSummary>,
    pub albums: SparsePagedList<AlbumSummary>,
    pub artists: SparsePagedList<ArtistSummary>,
    pub loaded_tracks: SparsePagedList<TrackSummary>,
    pub artist_detail: Option<ArtistDetail>,
    pub search_results: Option<SearchResults>,
    pub search_filter: SearchFilter,
    pub search_source_filter: Option<MediaSource>,
    pub sort_order: SortOrder,
    pub local_filter: String,
    pub is_loading: bool,
    pub error: Option<String>,
}

impl Default for CrateViewState {
    fn default() -> Self {
        Self {
            active_scope: LibraryScope::Home,
            active_crate: CrateRef::Home,
            local_sub_view: LocalSubView::Tracks,
            title: String::from("Home"),
            subtitle: String::from("Local, Spotify, YouTube, and SoundCloud"),
            cover: None,
            history: vec![CrateRef::Home],
            history_index: 0,
            playlists: SparsePagedList::new(50),
            albums: SparsePagedList::new(50),
            artists: SparsePagedList::new(50),
            loaded_tracks: SparsePagedList::new(50),
            artist_detail: None,
            search_results: None,
            search_filter: SearchFilter::All,
            search_source_filter: None,
            sort_order: SortOrder::Custom,
            local_filter: String::new(),
            is_loading: false,
            error: None,
        }
    }
}

/// Real-time process RSS and cache occupancy counters for Settings -> Memory & Cache.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TelemetrySnapshot {
    pub process_rss_mb: f32,
    pub process_hwm_mb: f32,
    pub cover_cache_mb: f32,
    pub cover_thumb_slots_used: u16,
    pub cover_thumb_slots_cap: u16,
    pub cover_hero_slots_used: u8,
    pub cover_hero_slots_cap: u8,
    pub loaded_track_records: usize,
    pub disk_audio_cache_mb: f32,
    pub disk_cover_cache_mb: f32,
}

/// Complete reactive view model consumed by `pulse-ui`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppViewState {
    pub mode: AppMode,
    pub auth_state: AuthState,
    pub connection: ConnectionStatus,
    pub config: AppConfig,
    pub player_state: PlayerState,
    pub devices: DevicesState,
    pub queue: QueueState,
    pub crate_view: CrateViewState,
    pub custom_playlists: Vec<CustomPlaylistSummary>,
    pub local_track_count: usize,
    pub local_dirs: Vec<String>,
    pub lyrics: LyricsState,
    pub insights: ListeningStatsSnapshot,
    pub right_drawer: RightDrawerTab,
    pub downloads_in_flight: BTreeSet<String>,
    pub telemetry: TelemetrySnapshot,
    pub notice_banner: Option<String>,
}

/// Typed commands emitted by `pulse-ui` toward `pulse-backend`.
#[derive(Debug, Clone, PartialEq)]
pub enum UiCommand {
    // Auth & engine
    StartOAuthLogin {
        client_id: Option<String>,
    },
    CancelOAuthLogin,
    SaveClientId(Option<String>),
    SetZeroconf {
        enabled: bool,
        device_name: String,
    },
    SignOut,
    PurgeCoverCache,
    ClearAudioCache,

    // Playback & queue
    TogglePlayPause,
    Play,
    Pause,
    NextTrack,
    PrevTrack,
    PlayTrackInContext {
        context_uri: String,
        track_uri: String,
        index: usize,
    },
    PlayTrackDirect {
        track: TrackSummary,
        context_name: String,
        context_tracks: Vec<TrackSummary>,
    },
    PlayQueueIndex(usize),
    SeekMs(u32),
    SetVolume(u16),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    EnqueueNext(TrackSummary),
    AppendQueue(TrackSummary),
    ReorderManualQueue {
        from: usize,
        to: usize,
    },
    PromoteContextToManual {
        context_index: usize,
        manual_index: usize,
    },
    RemoveManualQueueItem(usize),
    DeduplicateManualQueue,
    ClearManualQueue,
    SaveQueueAsPlaylist(String),

    // Multi-source library & local files
    RefreshDevices,
    TransferPlayback {
        device_id: String,
    },
    SetScope(LibraryScope),
    OpenCrate(CrateRef),
    NavigateBack,
    NavigateForward,
    FetchCratePage {
        crate_ref: CrateRef,
        offset: usize,
        limit: usize,
    },
    SetSortOrder(SortOrder),
    SetLocalFilter(String),
    RunGlobalSearch {
        query: String,
        source_filter: Option<MediaSource>,
    },
    SetSearchFilter(SearchFilter),
    ResolveInputUrl(String),
    AddLocalMusicFolder(String),
    RemoveLocalMusicFolder(String),
    RescanLocalMusic,
    OpenLocalPaths(Vec<String>),

    // Favorites & cross-source custom playlists
    ToggleLike(String),
    ToggleLikeTrack(TrackSummary),
    ToggleFollowArtist {
        artist_uri: String,
        follow: bool,
    },
    CreateCustomPlaylist(String),
    RenameCustomPlaylist {
        id: String,
        name: String,
    },
    DeleteCustomPlaylist(String),
    AddTrackToCustomPlaylist {
        playlist_id: String,
        track: TrackSummary,
    },
    RemoveTrackFromCustomPlaylist {
        playlist_id: String,
        index: usize,
    },
    ReorderCustomPlaylistTrack {
        playlist_id: String,
        from: usize,
        to: usize,
    },
    ImportM3uPlaylist(String),
    ExportCustomPlaylistM3u {
        playlist_id: String,
        dest_path: String,
    },
    PlayCrate {
        crate_ref: CrateRef,
        shuffle: bool,
    },
    QueueCrate(CrateRef),

    // DSP, FX, Lyrics & QoL
    SetEqConfig(EqConfig),
    SetPlaybackSpeed(PlaybackSpeed),
    SetAbLoop(AbLoopState),
    ToggleAbLoopStep,
    SetSleepTimer(SleepTimerTarget),
    SetLyricsOffsetMs(i32),
    RefreshLyrics,

    // Settings & UI preferences
    SetStreamQuality(StreamQuality),
    SetNormalisation(NormalisationMode),
    SetGapless(bool),
    SetAudioDevice(Option<String>),
    SetMprisEnabled(bool),
    SetConfirmClearQueue(bool),
    SetDiskCacheLimitMb(u64),
    SetThemePreference(ThemePreference),
    SetAccentSource(AccentSource),
    SetVisualizerMode(VisualizerMode),
    SetCrossfadeMs(u16),
    SetResumeOnStartup(bool),
    SetMode(AppMode),
    SetAdBlockEnabled(bool),
    SetMp3DownloadDir(String),
    SetMp3Quality(Mp3Quality),
    SetMp3AutoAddToLibrary(bool),
    SetMp3StripSponsors(bool),
    SaveYouTubeMp3 {
        track: TrackSummary,
        dest_path: Option<String>,
        quality: Option<Mp3Quality>,
        strip_sponsors: Option<bool>,
    },
}

/// Asynchronous updates pushed from `pulse-backend` to the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    AuthChanged(AuthState),
    ConnectionChanged(ConnectionStatus),
    ConfigUpdated(AppConfig),
    PlaybackChanged(PlayerState),
    PositionTick {
        position_ms: u32,
    },
    QueueChanged(QueueState),
    DevicesUpdated(DevicesState),
    CrateHeaderLoaded {
        crate_ref: CrateRef,
        title: String,
        subtitle: String,
        cover: Option<ImageRef>,
        total: usize,
    },
    CrateTracksPageLoaded {
        crate_ref: CrateRef,
        offset: usize,
        total: usize,
        items: Vec<TrackSummary>,
    },
    PlaylistsPageLoaded {
        offset: usize,
        total: usize,
        items: Vec<PlaylistSummary>,
    },
    AlbumsPageLoaded {
        offset: usize,
        total: usize,
        items: Vec<AlbumSummary>,
    },
    ArtistsPageLoaded {
        offset: usize,
        total: usize,
        items: Vec<ArtistSummary>,
    },
    ArtistDetailLoaded(ArtistDetail),
    SearchResultsLoaded(SearchResults),
    CustomPlaylistsUpdated(Vec<CustomPlaylistSummary>),
    LocalLibraryStatus {
        track_count: usize,
        dirs: Vec<String>,
        scanning: bool,
    },
    LyricsLoaded(LyricsState),
    ListeningStatsUpdated(ListeningStatsSnapshot),
    LikeStateChanged {
        uri: String,
        is_liked: bool,
    },
    TelemetryUpdated(TelemetrySnapshot),
    Mp3DownloadStarted {
        uri: String,
        title: String,
    },
    Mp3DownloadFinished {
        uri: String,
        saved_path: Option<String>,
        error: Option<String>,
    },
    Notice(Option<String>),
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_paged_list_prunes_stale_pages_on_shrinking_total_and_aligns_eviction() {
        let mut list = SparsePagedList::new(2);
        list.insert_page(0, vec![10, 20], 6);
        list.insert_page(2, vec![30, 40], 6);
        list.insert_page(4, vec![50, 60], 6);
        assert!(list.mark_in_flight(6));
        assert_eq!(list.loaded_count(), 6);

        list.insert_page(0, vec![10, 20], 3);
        assert!(list.mark_in_flight(6));
        assert_eq!(list.total(), 3);
        assert_eq!(list.get(2), Some(&30));
        assert_eq!(list.get(3), None);
        assert_eq!(list.get(4), None);
        assert_eq!(list.iter_loaded().count(), 3);

        list.insert_page(2, vec![30, 40], 6);
        list.insert_page(4, vec![50, 60], 6);
        list.evict_outside_window(1, 1);
        assert!(list.is_page_loaded(0));
        assert!(list.is_page_loaded(2));
        assert!(!list.is_page_loaded(4));

        assert!(list.mark_in_flight(4));
        list.clear_all_in_flight();
        assert!(list.mark_in_flight(4));
    }

    #[test]
    fn media_source_detection_maps_all_four_platforms() {
        assert_eq!(
            MediaSource::from_uri("local:abc123"),
            MediaSource::Local
        );
        assert_eq!(
            MediaSource::from_uri("/home/aurora/Music/track.flac"),
            MediaSource::Local
        );
        assert_eq!(
            MediaSource::from_uri("yt:video:dQw4w9WgXcQ"),
            MediaSource::YouTube
        );
        assert_eq!(
            MediaSource::from_uri("https://youtu.be/dQw4w9WgXcQ"),
            MediaSource::YouTube
        );
        assert_eq!(
            MediaSource::from_uri("sc:track:12345"),
            MediaSource::SoundCloud
        );
        assert_eq!(
            MediaSource::from_uri("https://soundcloud.com/artist/track"),
            MediaSource::SoundCloud
        );
        assert_eq!(
            MediaSource::from_uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC"),
            MediaSource::Spotify
        );
    }

    #[test]
    fn mp3_quality_and_aplayer_config_defaults_are_valid() {
        assert_eq!(Mp3Quality::ALL.len(), 4);
        assert_eq!(Mp3Quality::Cbr320.label(), "320 kbps");
        assert_eq!(Mp3Quality::VbrV0.short_label(), "V0");
        assert_eq!(Mp3Quality::Cbr192.label(), "192 kbps");
        assert_eq!(Mp3Quality::Cbr128.short_label(), "128k");

        let cfg = AppConfig::default();
        assert_eq!(cfg.device_name, "Aplayer");
        assert!(cfg.ad_block_enabled);
        assert_eq!(cfg.mp3_quality, Mp3Quality::Cbr320);
        assert!(cfg.mp3_auto_add_to_library);
        assert!(cfg.mp3_strip_sponsors);
    }
}
