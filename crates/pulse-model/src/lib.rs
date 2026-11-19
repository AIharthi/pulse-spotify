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

/// Active library browsing scope in the Crate Workbench strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LibraryScope {
    #[default]
    LikedSongs,
    Playlists,
    Albums,
    Artists,
    Recent,
    Search,
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
    AccountAndConnect,
    AudioAndDesktop,
    MemoryAndCache,
}

/// Persistent user configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            custom_client_id: None,
            device_name: String::from("Pulse"),
            zeroconf_enabled: true,
            audio_device: None,
            stream_quality: StreamQuality::Kbps320,
            normalisation: NormalisationMode::Off,
            gapless: true,
            prefetch_bitrate: true,
            mpris_enabled: true,
            media_keys_enabled: true,
            confirm_clear_queue: true,
            disk_audio_cache_mb: 256,
            disk_cover_cache_mb: 32,
        }
    }
}

/// Cover artwork identifier shared between backend metadata and `pulse-assets`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageRef {
    /// Stable asset key (Spotify file ID hex or hashed cover key).
    pub id: Arc<str>,
    /// Optional direct CDN URL if supplied by metadata/Web API.
    pub url: Option<Arc<str>>,
}

/// Compact track representation used in virtualized tables and queues.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// Extended track metadata for the active Stage / Focus view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackDetail {
    pub summary: TrackSummary,
    pub track_number: Option<u32>,
    pub disc_total_tracks: Option<u32>,
    pub release_year: Option<u16>,
    pub explicit: bool,
}

/// Navigation target within the Crate Workbench.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CrateRef {
    #[default]
    LikedSongs,
    PlaylistsIndex,
    AlbumsIndex,
    ArtistsIndex,
    Recent,
    Playlist { uri: Arc<str>, name: String },
    Album { uri: Arc<str>, title: String },
    Artist { uri: Arc<str>, name: String },
    Search { query: String },
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
}

/// Compact artist card metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistSummary {
    pub uri: Arc<str>,
    pub name: String,
    pub cover: Option<ImageRef>,
    pub is_followed: bool,
}

/// Expanded artist page payload (top tracks + discography sections).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistDetail {
    pub artist: ArtistSummary,
    pub top_tracks: Vec<TrackSummary>,
    pub albums: Vec<AlbumSummary>,
    pub singles: Vec<AlbumSummary>,
}

/// Search response grouped for the Search scope and Ctrl+K omnibox.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    pub query: String,
    pub top_match_uri: Option<Arc<str>>,
    pub artists: Vec<ArtistSummary>,
    pub albums: Vec<AlbumSummary>,
    pub playlists: Vec<PlaylistSummary>,
    pub tracks: Vec<TrackSummary>,
    /// True when results fell back to `spclient.get_context("spotify:search:...")`,
    /// which returns tracks context only without rich artist/album/playlist sections.
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

/// Two-tier queue snapshot exposed to the Live Stage and Focus Stage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueueState {
    pub manual_items: Vec<TrackSummary>,
    pub context_items: Vec<TrackSummary>,
    pub context_name: String,
    pub context_uri: Option<Arc<str>>,
}

/// Active playback transport and audio metadata state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub is_playing: bool,
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
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            is_playing: false,
            active_track: None,
            cover_accent_rgba: [229, 154, 62, 255],
            position_ms: 0,
            duration_ms: 0,
            volume: 49_151,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            audio_badge: String::from("OGG 320"),
            sample_rate_hz: 44_100,
            bitrate_kbps: 320,
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

        // Prune any stale pages that lie completely past the new total, and trim any straddling tail.
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

    /// Iterates currently loaded items in index order `(global_index, &T)`.
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

    /// Evicts cached pages farther than `keep_pages_radius` pages from `center_index`.
    pub fn evict_outside_window(&mut self, center_index: usize, keep_pages_radius: usize) {
        let center_start = self.page_start_for(center_index);
        let span = self.page_size.saturating_mul(keep_pages_radius);
        let min_keep = center_start.saturating_sub(span);
        let max_keep = center_start.saturating_add(span);
        self.pages
            .retain(|&page_start, _| page_start >= min_keep && page_start <= max_keep);
    }
}

/// Right-pane Crate Workbench state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateViewState {
    pub active_scope: LibraryScope,
    pub active_crate: CrateRef,
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
    pub sort_order: SortOrder,
    pub local_filter: String,
    pub is_loading: bool,
    pub error: Option<String>,
}

impl Default for CrateViewState {
    fn default() -> Self {
        Self {
            active_scope: LibraryScope::LikedSongs,
            active_crate: CrateRef::LikedSongs,
            title: String::from("Liked Songs"),
            subtitle: String::new(),
            cover: None,
            history: vec![CrateRef::LikedSongs],
            history_index: 0,
            playlists: SparsePagedList::new(50),
            albums: SparsePagedList::new(50),
            artists: SparsePagedList::new(50),
            loaded_tracks: SparsePagedList::new(50),
            artist_detail: None,
            search_results: None,
            search_filter: SearchFilter::All,
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
    pub telemetry: TelemetrySnapshot,
    pub notice_banner: Option<String>,
}

/// Typed commands emitted by `pulse-ui` toward `pulse-backend` / `pulse-assets`.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    ClearManualQueue,

    // Devices & library
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
    RunGlobalSearch(String),
    SetSearchFilter(SearchFilter),
    ToggleLike(String),
    ToggleFollowArtist {
        artist_uri: String,
        follow: bool,
    },
    PlayCrate {
        crate_ref: CrateRef,
        shuffle: bool,
    },
    QueueCrate(CrateRef),

    // Settings & window mode
    SetStreamQuality(StreamQuality),
    SetNormalisation(NormalisationMode),
    SetGapless(bool),
    SetAudioDevice(Option<String>),
    SetMprisEnabled(bool),
    SetConfirmClearQueue(bool),
    SetDiskCacheLimitMb(u64),
    SetMode(AppMode),
}

/// Asynchronous updates pushed from `pulse-backend` / `pulse-assets` to the UI.
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
    LikeStateChanged {
        uri: String,
        is_liked: bool,
    },
    TelemetryUpdated(TelemetrySnapshot),
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

        // Shrinking total from 6 -> 3 truncates straddling page 2, drops page 4, and prunes in_flight >= 3.
        list.insert_page(0, vec![10, 20], 3);
        assert!(list.mark_in_flight(6));
        assert_eq!(list.total(), 3);
        assert_eq!(list.get(2), Some(&30));
        assert_eq!(list.get(3), None);
        assert_eq!(list.get(4), None);
        assert_eq!(list.iter_loaded().count(), 3);

        // Eviction centered at index 1 (page 0) with radius 1 keeps pages [0, 2] and drops page 4.
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
}
