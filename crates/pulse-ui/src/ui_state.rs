//! Presentation state, virtual-scroll metrics, widget IDs, and single-thread Tokio executor.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use iced::widget::Id;
use iced::{Executor, Point, Size, window};
use pulse_model::{
    AppMode, AppViewState, AuthState, BackendEvent, ConnectionStatus, CrateRef, CrateViewState,
    LibraryScope, SettingsTab, TrackSummary,
};

use crate::cover_store::{CoverHandleCache, CoverTier};

/// Widget identifiers used for programmatic focus and scroll operations in `iced 0.14`.
pub fn id_filter_input() -> Id {
    Id::new("pulse-filter-input")
}

pub fn id_omnibox_input() -> Id {
    Id::new("pulse-omnibox-input")
}

pub fn id_omnibox_scroll() -> Id {
    Id::new("pulse-omnibox-scroll")
}

pub fn id_crate_scroll() -> Id {
    Id::new("pulse-crate-scroll")
}

pub fn id_queue_scroll() -> Id {
    Id::new("pulse-queue-scroll")
}

pub fn id_client_id_input() -> Id {
    Id::new("pulse-client-id-input")
}

pub fn id_device_name_input() -> Id {
    Id::new("pulse-device-name-input")
}

/// Iced executor pinned to a single worker thread to keep RSS strictly bounded.
pub struct SingleThreadTokio(tokio::runtime::Runtime);

impl Executor for SingleThreadTokio {
    fn new() -> Result<Self, iced::futures::io::Error> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("pulse-ui-tokio")
            .enable_all()
            .build()
            .map(Self)
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        std::mem::drop(self.0.spawn(future));
    }

    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.0.enter();
        f()
    }

    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        self.0.block_on(future)
    }
}

/// Active keyboard-navigation focus target (`Tab` toggles between Crate and Queue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivePane {
    #[default]
    Crate,
    Queue,
}

/// Floating context menu state opened on right-click or `Shift+Enter`.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackContextMenu {
    pub pane: ActivePane,
    pub index: usize,
    pub track: TrackSummary,
    pub pos: Point,
}

/// Root UI state combining `AppViewState`, bounded `CoverHandleCache`, and interaction state.
#[derive(Debug)]
pub struct UiState {
    pub view: AppViewState,
    pub covers: CoverHandleCache,

    /// Last non-mini mode so exiting Mini-Player restores Workbench or FocusStage cleanly.
    pub mode_before_mini: AppMode,
    pub window_id: Option<window::Id>,
    pub window_size: Size,
    pub workbench_size: Size,
    pub stage_width: f32,
    pub always_on_top: bool,
    pub show_focus_queue: bool,
    pub show_narrow_queue: bool,

    // Navigation & virtual scroll state
    pub active_pane: ActivePane,
    pub crate_scroll_y: f32,
    pub crate_viewport_height: f32,
    pub queue_scroll_y: f32,
    pub queue_viewport_height: f32,
    pub selected_crate_row: Option<usize>,
    pub hovered_track_row: Option<usize>,
    pub mini_player_hovered: bool,
    pub selected_queue_row: Option<usize>,
    pub dragging_queue_from: Option<usize>,

    // Search, filter & modal overlays
    pub input_focused: bool,
    pub omnibox_open: bool,
    pub omnibox_query: String,
    pub omnibox_selected: usize,
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub settings_client_id_draft: String,
    pub settings_device_name_draft: String,
    pub device_popover_open: bool,
    pub context_menu: Option<TrackContextMenu>,
    pub confirm_clear_queue_open: bool,

    // Pointer / hover feedback
    pub cursor_pos: Point,
    pub last_radial_accent: Option<[u8; 4]>,
    pub status_toast: Option<String>,
    pub dismissed_conn_error: Option<String>,
    pub scrubbing_pos_ms: Option<u32>,
    pub failed_covers: HashMap<(CoverTier, Arc<str>), Instant>,
}

impl Default for UiState {
    fn default() -> Self {
        Self::new(AppViewState::default())
    }
}

impl UiState {
    pub const WORKBENCH_DEFAULT_SIZE: Size = Size::new(1120.0, 720.0);
    pub const WORKBENCH_MIN_SIZE: Size = Size::new(560.0, 480.0);
    pub const FOCUS_MIN_SIZE: Size = Size::new(680.0, 520.0);
    pub const MINI_DEFAULT_SIZE: Size = Size::new(420.0, 112.0);
    pub const MINI_MIN_SIZE: Size = Size::new(360.0, 112.0);
    pub const MINI_MAX_SIZE: Size = Size::new(560.0, 112.0);
    pub const STAGE_MIN_WIDTH: f32 = 320.0;
    pub const STAGE_MAX_WIDTH: f32 = 440.0;
    pub const STAGE_DEFAULT_WIDTH: f32 = 360.0;

    /// Constructs UI state from an initial backend/config view snapshot.
    pub fn new(view: AppViewState) -> Self {
        let client_id_draft = view.config.custom_client_id.clone().unwrap_or_default();
        let device_name_draft = view.config.device_name.clone();
        Self {
            view,
            covers: CoverHandleCache::new(),
            mode_before_mini: AppMode::Workbench,
            window_id: None,
            window_size: Self::WORKBENCH_DEFAULT_SIZE,
            workbench_size: Self::WORKBENCH_DEFAULT_SIZE,
            stage_width: Self::STAGE_DEFAULT_WIDTH,
            always_on_top: false,
            show_focus_queue: true,
            show_narrow_queue: false,
            active_pane: ActivePane::Crate,
            crate_scroll_y: 0.0,
            crate_viewport_height: 520.0,
            queue_scroll_y: 0.0,
            queue_viewport_height: 300.0,
            selected_crate_row: Some(0),
            hovered_track_row: None,
            mini_player_hovered: false,
            selected_queue_row: None,
            dragging_queue_from: None,
            input_focused: false,
            omnibox_open: false,
            omnibox_query: String::new(),
            omnibox_selected: 0,
            settings_open: false,
            settings_tab: SettingsTab::AccountAndConnect,
            settings_client_id_draft: client_id_draft,
            settings_device_name_draft: device_name_draft,
            device_popover_open: false,
            context_menu: None,
            confirm_clear_queue_open: false,
            cursor_pos: Point::ORIGIN,
            last_radial_accent: None,
            status_toast: None,
            dismissed_conn_error: None,
            scrubbing_pos_ms: None,
            failed_covers: HashMap::new(),
        }
    }

    /// Width `< 860px` collapses the left Live Stage column into a compact `64px` bottom bar.
    pub fn is_narrow_workbench(&self) -> bool {
        self.window_size.width < 860.0
    }

    /// Width `< 1020px` auto-hides the Album column in the Crate track table.
    pub fn hide_album_column(&self) -> bool {
        self.window_size.width < 1020.0
    }

    /// Returns true if any modal sheet, popover, omnibox palette, or context menu is open.
    pub fn has_overlay_open(&self) -> bool {
        self.settings_open
            || self.device_popover_open
            || self.omnibox_open
            || self.context_menu.is_some()
            || self.confirm_clear_queue_open
    }

    /// Pushes a forward navigation target onto `crate_view.history`, pruning forward redo entries.
    pub fn push_history(&mut self, crate_ref: CrateRef) {
        let cv = &mut self.view.crate_view;
        if cv
            .history
            .get(cv.history_index)
            .is_some_and(|cur| same_crate_target(cur, &crate_ref))
        {
            if let Some(cur) = cv.history.get_mut(cv.history_index) {
                *cur = crate_ref;
            }
            return;
        }
        if cv.history_index + 1 < cv.history.len() {
            cv.history.truncate(cv.history_index + 1);
        }
        cv.history.push(crate_ref);
        cv.history_index = cv.history.len() - 1;
    }

    /// Steps backward (`forward == false`) or forward (`forward == true`) inside `crate_view.history`.
    pub fn step_history(&mut self, forward: bool) -> Option<CrateRef> {
        let cv = &mut self.view.crate_view;
        let next_idx = if forward {
            if cv.history_index + 1 < cv.history.len() {
                cv.history_index + 1
            } else {
                return None;
            }
        } else if cv.history_index > 0 {
            cv.history_index - 1
        } else {
            return None;
        };
        cv.history_index = next_idx;
        cv.history.get(next_idx).cloned()
    }

    /// Updates the current history entry if it matches `crate_ref`, otherwise records it.
    pub fn sync_history_target(&mut self, crate_ref: CrateRef) {
        let cv = &mut self.view.crate_view;
        if let Some(cur) = cv.history.get_mut(cv.history_index)
            && same_crate_target(cur, &crate_ref)
        {
            *cur = crate_ref;
            return;
        }
        self.push_history(crate_ref);
    }

    /// Clamps and updates Stage splitter width inside `[320.0, 440.0]`.
    pub fn set_stage_width(&mut self, width: f32) {
        self.stage_width = width.clamp(Self::STAGE_MIN_WIDTH, Self::STAGE_MAX_WIDTH);
    }

    /// Clears library caches and transient overlays on sign-out.
    pub fn reset_for_sign_out(&mut self) {
        self.view.crate_view = CrateViewState::default();
        self.view.queue = pulse_model::QueueState::default();
        self.view.player_state = pulse_model::PlayerState::default();
        self.covers.purge();
        self.last_radial_accent = None;
        self.crate_scroll_y = 0.0;
        self.queue_scroll_y = 0.0;
        self.selected_crate_row = None;
        self.selected_queue_row = None;
        self.hovered_track_row = None;
        self.mini_player_hovered = false;
        self.dragging_queue_from = None;
        self.scrubbing_pos_ms = None;
        self.settings_open = false;
        self.device_popover_open = false;
        self.omnibox_open = false;
        self.context_menu = None;
        self.confirm_clear_queue_open = false;
        self.show_narrow_queue = false;
        self.input_focused = false;
        self.failed_covers.clear();
    }

    /// Applies an asynchronous `BackendEvent` into `AppViewState` and syncs UI invariants.
    pub fn apply_backend_event(&mut self, ev: BackendEvent) {
        match ev {
            BackendEvent::AuthChanged(auth) => {
                let was_authed = matches!(self.view.auth_state, AuthState::Authenticated { .. });
                let is_authed = matches!(auth, AuthState::Authenticated { .. });
                let is_unauthed = matches!(auth, AuthState::Unauthenticated);
                self.view.auth_state = auth;
                if (was_authed && !is_authed) || is_unauthed {
                    self.reset_for_sign_out();
                }
            }
            BackendEvent::ConnectionChanged(conn) => {
                match &conn {
                    ConnectionStatus::Error(err) => {
                        if self.dismissed_conn_error.as_deref() != Some(err.as_str()) {
                            self.dismissed_conn_error = None;
                        }
                    }
                    _ => {
                        self.dismissed_conn_error = None;
                    }
                }
                self.view.connection = conn;
            }
            BackendEvent::ConfigUpdated(cfg) => {
                if !self.input_focused {
                    self.settings_client_id_draft =
                        cfg.custom_client_id.clone().unwrap_or_default();
                    self.settings_device_name_draft = cfg.device_name.clone();
                }
                self.view.config = cfg;
            }
            BackendEvent::PlaybackChanged(player) => {
                let prev_uri = self
                    .view
                    .player_state
                    .active_track
                    .as_ref()
                    .map(|t| &*t.summary.uri);
                let next_uri = player.active_track.as_ref().map(|t| &*t.summary.uri);
                if prev_uri != next_uri {
                    self.scrubbing_pos_ms = None;
                }
                self.view.player_state = player;
            }
            BackendEvent::PositionTick { position_ms } => {
                self.view.player_state.position_ms = position_ms;
            }
            BackendEvent::QueueChanged(queue) => {
                let manual_len = queue.manual_items.len();
                let ctx_len = queue.context_items.len();
                let total = manual_len + ctx_len;
                let headers = usize::from(manual_len > 0) + usize::from(ctx_len > 0);
                let row_h = if self.view.mode == AppMode::FocusStage {
                    48.0
                } else {
                    44.0
                };
                let content_h = total as f32 * row_h + headers as f32 * 26.0;
                let max_scroll = (content_h - self.queue_viewport_height).max(0.0);
                self.queue_scroll_y = self.queue_scroll_y.min(max_scroll);

                self.view.queue = queue;
                if total == 0 {
                    self.selected_queue_row = None;
                } else if let Some(idx) = self.selected_queue_row {
                    self.selected_queue_row = Some(idx.min(total - 1));
                }
            }
            BackendEvent::DevicesUpdated(devices) => {
                if self.settings_device_name_draft.is_empty()
                    && !devices.local_device_name.is_empty()
                {
                    self.settings_device_name_draft = devices.local_device_name.clone();
                }
                self.view.devices = devices;
            }
            BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title,
                subtitle,
                cover,
                total,
            } => {
                let cv = &mut self.view.crate_view;
                if !same_crate_target(&cv.active_crate, &crate_ref) {
                    return;
                }
                cv.active_crate = crate_ref.clone();
                cv.active_scope = scope_for_crate(&crate_ref);
                cv.title = title;
                cv.subtitle = subtitle;
                cv.cover = cover;
                cv.is_loading = false;
                cv.error = None;
                if matches!(
                    crate_ref,
                    CrateRef::LikedSongs
                        | CrateRef::Playlist { .. }
                        | CrateRef::Album { .. }
                        | CrateRef::Recent
                ) {
                    cv.loaded_tracks.set_total(total);
                }
                self.sync_history_target(crate_ref);
            }
            BackendEvent::CrateTracksPageLoaded {
                crate_ref,
                offset,
                total,
                items,
            } => {
                let cv = &mut self.view.crate_view;
                if same_crate_target(&cv.active_crate, &crate_ref) {
                    cv.loaded_tracks.insert_page(offset, items, total);
                    let center_idx = ((self.crate_scroll_y / 44.0).max(0.0) as usize)
                        .min(total.saturating_sub(1));
                    cv.loaded_tracks.evict_outside_window(center_idx, 4);
                    cv.is_loading = false;
                    cv.error = None;
                    if total == 0 {
                        self.selected_crate_row = None;
                    } else if self.selected_crate_row.is_none() {
                        self.selected_crate_row = Some(0);
                    }
                }
            }
            BackendEvent::PlaylistsPageLoaded {
                offset,
                total,
                items,
            } => {
                let cv = &mut self.view.crate_view;
                cv.playlists.insert_page(offset, items, total);
                if same_crate_target(&cv.active_crate, &CrateRef::PlaylistsIndex) {
                    cv.active_scope = LibraryScope::Playlists;
                    cv.title = String::from("Playlists");
                    cv.subtitle = format!("{total} playlists");
                    cv.cover = None;
                    cv.artist_detail = None;
                    cv.is_loading = false;
                    cv.error = None;
                    self.sync_history_target(CrateRef::PlaylistsIndex);
                }
            }
            BackendEvent::AlbumsPageLoaded {
                offset,
                total,
                items,
            } => {
                let cv = &mut self.view.crate_view;
                cv.albums.insert_page(offset, items, total);
                if same_crate_target(&cv.active_crate, &CrateRef::AlbumsIndex) {
                    cv.active_scope = LibraryScope::Albums;
                    cv.title = String::from("Albums");
                    cv.subtitle = format!("{total} albums");
                    cv.cover = None;
                    cv.artist_detail = None;
                    cv.is_loading = false;
                    cv.error = None;
                    self.sync_history_target(CrateRef::AlbumsIndex);
                }
            }
            BackendEvent::ArtistsPageLoaded {
                offset,
                total,
                items,
            } => {
                let cv = &mut self.view.crate_view;
                cv.artists.insert_page(offset, items, total);
                if same_crate_target(&cv.active_crate, &CrateRef::ArtistsIndex) {
                    cv.active_scope = LibraryScope::Artists;
                    cv.title = String::from("Artists");
                    cv.subtitle = format!("{total} artists");
                    cv.cover = None;
                    cv.artist_detail = None;
                    cv.is_loading = false;
                    cv.error = None;
                    self.sync_history_target(CrateRef::ArtistsIndex);
                }
            }
            BackendEvent::ArtistDetailLoaded(detail) => {
                let target = CrateRef::Artist {
                    uri: Arc::clone(&detail.artist.uri),
                    name: detail.artist.name.clone(),
                };
                let cv = &mut self.view.crate_view;
                if !same_crate_target(&cv.active_crate, &target) {
                    return;
                }
                cv.active_scope = LibraryScope::Artists;
                cv.active_crate = target.clone();
                cv.title = detail.artist.name.clone();
                cv.subtitle = String::from("Artist");
                cv.cover = detail.artist.cover.clone();
                cv.artist_detail = Some(detail);
                cv.is_loading = false;
                cv.error = None;
                self.sync_history_target(target);
            }
            BackendEvent::SearchResultsLoaded(results) => {
                let total_tracks = results.tracks.len();
                let query = results.query.clone();
                let target = CrateRef::Search {
                    query: query.clone(),
                };
                let cv = &mut self.view.crate_view;
                if !same_crate_target(&cv.active_crate, &target)
                    && !(matches!(cv.active_crate, CrateRef::Search { ref query } if query.is_empty()))
                {
                    return;
                }
                cv.active_scope = LibraryScope::Search;
                cv.active_crate = target.clone();
                cv.title = if query.is_empty() {
                    String::from("Search")
                } else {
                    format!("Search: {query}")
                };
                cv.search_results = Some(results);
                cv.is_loading = false;
                cv.error = None;
                self.selected_crate_row = if total_tracks > 0 { Some(0) } else { None };
                self.sync_history_target(target);
            }
            BackendEvent::LikeStateChanged { uri, is_liked } => {
                self.sync_track_liked(&uri, is_liked);
            }
            BackendEvent::TelemetryUpdated(snapshot) => {
                self.view.telemetry = snapshot;
            }
            BackendEvent::Notice(notice) => {
                self.view.notice_banner = notice;
            }
            BackendEvent::Error(err) => {
                let cv = &mut self.view.crate_view;
                cv.is_loading = false;
                cv.loaded_tracks.clear_all_in_flight();
                cv.playlists.clear_all_in_flight();
                cv.albums.clear_all_in_flight();
                cv.artists.clear_all_in_flight();
                self.view.notice_banner = Some(err);
            }
        }
    }

    fn sync_track_liked(&mut self, uri: &str, is_liked: bool) {
        if let Some(track) = &mut self.view.player_state.active_track
            && track.summary.uri.as_ref() == uri
        {
            track.summary.is_liked = is_liked;
        }
        for item in &mut self.view.queue.manual_items {
            if item.uri.as_ref() == uri {
                item.is_liked = is_liked;
            }
        }
        for item in &mut self.view.queue.context_items {
            if item.uri.as_ref() == uri {
                item.is_liked = is_liked;
            }
        }
        if let Some(detail) = &mut self.view.crate_view.artist_detail {
            for item in &mut detail.top_tracks {
                if item.uri.as_ref() == uri {
                    item.is_liked = is_liked;
                }
            }
        }
        if let Some(results) = &mut self.view.crate_view.search_results {
            for item in &mut results.tracks {
                if item.uri.as_ref() == uri {
                    item.is_liked = is_liked;
                }
            }
        }
        let total = self.view.crate_view.loaded_tracks.total();
        for idx in 0..total {
            if let Some(item) = self.view.crate_view.loaded_tracks.get_mut(idx)
                && item.uri.as_ref() == uri
            {
                item.is_liked = is_liked;
            }
        }
    }
}

pub(crate) fn scope_for_crate(crate_ref: &CrateRef) -> LibraryScope {
    match crate_ref {
        CrateRef::LikedSongs => LibraryScope::LikedSongs,
        CrateRef::PlaylistsIndex | CrateRef::Playlist { .. } => LibraryScope::Playlists,
        CrateRef::AlbumsIndex | CrateRef::Album { .. } => LibraryScope::Albums,
        CrateRef::ArtistsIndex | CrateRef::Artist { .. } => LibraryScope::Artists,
        CrateRef::Recent => LibraryScope::Recent,
        CrateRef::Search { .. } => LibraryScope::Search,
    }
}

pub(crate) fn same_crate_target(a: &CrateRef, b: &CrateRef) -> bool {
    match (a, b) {
        (CrateRef::LikedSongs, CrateRef::LikedSongs)
        | (CrateRef::PlaylistsIndex, CrateRef::PlaylistsIndex)
        | (CrateRef::AlbumsIndex, CrateRef::AlbumsIndex)
        | (CrateRef::ArtistsIndex, CrateRef::ArtistsIndex)
        | (CrateRef::Recent, CrateRef::Recent) => true,
        (CrateRef::Playlist { uri: ua, .. }, CrateRef::Playlist { uri: ub, .. })
        | (CrateRef::Album { uri: ua, .. }, CrateRef::Album { uri: ub, .. })
        | (CrateRef::Artist { uri: ua, .. }, CrateRef::Artist { uri: ub, .. }) => ua == ub,
        (CrateRef::Search { query: qa }, CrateRef::Search { query: qb }) => qa == qb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_model::QueueState;

    fn sample_track(id: &str) -> TrackSummary {
        TrackSummary {
            uri: Arc::from(format!("spotify:track:{id}")),
            title: format!("Track {id}"),
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
    fn backend_events_update_crate_and_like_state() {
        let mut ui = UiState::default();
        ui.apply_backend_event(BackendEvent::CrateTracksPageLoaded {
            crate_ref: CrateRef::LikedSongs,
            offset: 0,
            total: 2,
            items: vec![sample_track("a"), sample_track("b")],
        });
        assert_eq!(ui.view.crate_view.loaded_tracks.total(), 2);

        ui.apply_backend_event(BackendEvent::QueueChanged(QueueState {
            manual_items: vec![sample_track("a")],
            context_items: vec![sample_track("b")],
            context_name: String::from("Liked"),
            context_uri: None,
        }));
        ui.apply_backend_event(BackendEvent::LikeStateChanged {
            uri: String::from("spotify:track:a"),
            is_liked: true,
        });

        assert!(ui.view.crate_view.loaded_tracks.get(0).expect("row 0").is_liked);
        assert!(ui.view.queue.manual_items[0].is_liked);
        assert!(!ui.view.queue.context_items[0].is_liked);
    }

    #[test]
    fn index_pages_update_scope_crate_and_history_and_allow_narrow_resize() {
        const { assert!(UiState::WORKBENCH_MIN_SIZE.width < 860.0) };

        let mut ui = UiState::default();
        // Background PlaylistsPageLoaded while viewing LikedSongs populates cache without hijacking active_crate.
        ui.apply_backend_event(BackendEvent::PlaylistsPageLoaded {
            offset: 0,
            total: 3,
            items: Vec::new(),
        });
        assert_eq!(ui.view.crate_view.active_crate, CrateRef::LikedSongs);
        assert_eq!(ui.view.crate_view.playlists.total(), 3);

        // When PlaylistsIndex is the active crate, loading completes and updates history.
        ui.view.crate_view.active_crate = CrateRef::PlaylistsIndex;
        ui.view.crate_view.is_loading = true;
        ui.apply_backend_event(BackendEvent::PlaylistsPageLoaded {
            offset: 0,
            total: 3,
            items: Vec::new(),
        });
        assert_eq!(ui.view.crate_view.active_crate, CrateRef::PlaylistsIndex);
        assert_eq!(ui.view.crate_view.active_scope, LibraryScope::Playlists);
        assert!(!ui.view.crate_view.is_loading);
        assert_eq!(
            ui.view.crate_view.history,
            vec![CrateRef::LikedSongs, CrateRef::PlaylistsIndex]
        );
        assert_eq!(ui.view.crate_view.history_index, 1);

        ui.view.crate_view.active_crate = CrateRef::AlbumsIndex;
        ui.apply_backend_event(BackendEvent::AlbumsPageLoaded {
            offset: 0,
            total: 2,
            items: Vec::new(),
        });
        assert_eq!(ui.view.crate_view.active_crate, CrateRef::AlbumsIndex);
        assert_eq!(ui.view.crate_view.active_scope, LibraryScope::Albums);
        assert_eq!(ui.view.crate_view.history_index, 2);

        assert_eq!(ui.step_history(false), Some(CrateRef::PlaylistsIndex));
        assert_eq!(ui.view.crate_view.history_index, 1);
        assert_eq!(ui.step_history(true), Some(CrateRef::AlbumsIndex));
        assert_eq!(ui.view.crate_view.history_index, 2);
        assert_eq!(ui.step_history(true), None);

        ui.view.crate_view.active_crate = CrateRef::ArtistsIndex;
        ui.apply_backend_event(BackendEvent::ArtistsPageLoaded {
            offset: 0,
            total: 1,
            items: Vec::new(),
        });
        assert_eq!(ui.view.crate_view.active_crate, CrateRef::ArtistsIndex);
        assert_eq!(ui.view.crate_view.active_scope, LibraryScope::Artists);

        // Stale CrateTracksPageLoaded for another crate does not overwrite active crate.
        ui.apply_backend_event(BackendEvent::CrateTracksPageLoaded {
            crate_ref: CrateRef::Recent,
            offset: 0,
            total: 5,
            items: vec![sample_track("stale")],
        });
        assert_eq!(ui.view.crate_view.active_crate, CrateRef::ArtistsIndex);
        assert_eq!(ui.view.crate_view.loaded_tracks.total(), 0);

        // Sign-out resets library, queue, and transient modals.
        ui.view.auth_state = AuthState::ConnectOnly {
            device_name: String::from("Pulse"),
        };
        ui.omnibox_open = true;
        ui.apply_backend_event(BackendEvent::AuthChanged(AuthState::Unauthenticated));
        assert!(!ui.omnibox_open);
        assert_eq!(ui.view.crate_view.playlists.total(), 0);
    }

    #[test]
    fn scrubbing_preview_survives_ticks_and_clears_on_track_change() {
        use pulse_model::{PlayerState, TrackDetail};

        let make_detail = |id: &str| TrackDetail {
            summary: sample_track(id),
            track_number: None,
            disc_total_tracks: None,
            release_year: None,
            explicit: false,
        };

        let mut ui = UiState::default();
        ui.view.player_state = PlayerState {
            active_track: Some(make_detail("a")),
            ..Default::default()
        };
        ui.scrubbing_pos_ms = Some(42_000);

        ui.apply_backend_event(BackendEvent::PositionTick { position_ms: 15_000 });
        assert_eq!(ui.scrubbing_pos_ms, Some(42_000));
        assert_eq!(ui.view.player_state.position_ms, 15_000);

        // Same-track state updates (e.g. pause/play toggle) preserve the active drag preview.
        ui.apply_backend_event(BackendEvent::PlaybackChanged(PlayerState {
            active_track: Some(make_detail("a")),
            is_playing: true,
            ..Default::default()
        }));
        assert_eq!(ui.scrubbing_pos_ms, Some(42_000));

        // Track transitions reset any in-progress scrub preview.
        ui.apply_backend_event(BackendEvent::PlaybackChanged(PlayerState {
            active_track: Some(make_detail("b")),
            is_playing: true,
            ..Default::default()
        }));
        assert_eq!(ui.scrubbing_pos_ms, None);
    }
}
