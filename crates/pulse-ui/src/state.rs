//! Reactive GTK4 UI view-model store, dirty flags, and presentation formatters.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pulse_assets::AdaptiveThemeTokens;
use pulse_model::{
    AppViewState, BackendEvent, CrateRef, CrateViewState, LibraryScope, LocalSubView, MediaSource,
    SettingsTab, SortOrder, TrackSummary,
};

use crate::icons_gtk::{CoverTextureStore, VectorIconCache};
use crate::theme_css::compute_tokens;

pub type RedrawFn = Rc<dyn Fn(DirtyFlags)>;
pub type FocusFn = Rc<dyn Fn(bool)>;
pub type PromptDialogFn = Rc<dyn Fn(String, String, String, Rc<dyn Fn(String)>)>;

/// Shared lightweight cloneable handle wiring views, drawers, dialogs, and shortcuts together.
#[derive(Clone)]
pub struct UiBridge {
    pub store: Rc<RefCell<UiStore>>,
    pub send: Rc<dyn Fn(pulse_model::UiCommand)>,
    pub icons: Rc<VectorIconCache>,
    pub covers: Rc<CoverTextureStore>,
    pub redraw: RedrawFn,
    pub focus_omnibox: FocusFn,
    pub open_folder_dialog: Rc<dyn Fn()>,
    pub open_files_dialog: Rc<dyn Fn()>,
    pub import_m3u_dialog: Rc<dyn Fn()>,
    pub export_m3u_dialog: Rc<dyn Fn(String, String)>,
    pub prompt_text_dialog: PromptDialogFn,
    pub open_external_url: Rc<dyn Fn(String)>,
}

impl UiBridge {
    pub fn record_action(&self, msg: impl Into<String>) {
        self.store.borrow_mut().clickthrough_log.push(msg.into());
    }
}

/// Bitmask controlling which GTK widget regions refresh for a state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DirtyFlags(u32);

impl DirtyFlags {
    pub const NONE: Self = Self(0);
    pub const CHROME: Self = Self(1 << 0);
    pub const NAV: Self = Self(1 << 1);
    pub const CONTENT: Self = Self(1 << 2);
    pub const TIMELINE: Self = Self(1 << 3);
    pub const PLAYER_META: Self = Self(1 << 4);
    pub const DRAWER_QUEUE: Self = Self(1 << 5);
    pub const DRAWER_LYRICS: Self = Self(1 << 6);
    pub const DRAWER_LYRICS_TICK: Self = Self(1 << 7);
    pub const DRAWER_EQ: Self = Self(1 << 8);
    pub const DRAWER_INSIGHTS: Self = Self(1 << 9);
    pub const DEVICES: Self = Self(1 << 10);
    pub const THEME: Self = Self(1 << 11);
    pub const SETTINGS: Self = Self(1 << 12);
    pub const MODE: Self = Self(1 << 13);
    pub const ALL: Self = Self(u32::MAX);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn any(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for DirtyFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for DirtyFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Complete client UI store driven by `pulse-backend` events.
pub struct UiStore {
    pub view: AppViewState,
    pub tokens: AdaptiveThemeTokens,
    pub scrubbing_pos_ms: Option<u32>,
    pub omnibox_text: String,
    pub omnibox_global_mode: bool,
    pub selected_track_index: Option<usize>,
    pub nav_collapsed_override: Option<bool>,
    pub window_width: i32,
    pub window_height: i32,
    pub settings_open: bool,
    pub settings_tab: SettingsTab,
    pub crate_loading_timeout_id: u64,
    pub liked_uri_overrides: HashSet<String>,
    pub clickthrough_log: Vec<String>,
}

impl UiStore {
    pub fn new(initial_view: AppViewState) -> Self {
        let tokens = compute_tokens(&initial_view.config, initial_view.player_state.cover_accent_rgba);
        Self {
            view: initial_view,
            tokens,
            scrubbing_pos_ms: None,
            omnibox_text: String::new(),
            omnibox_global_mode: false,
            selected_track_index: None,
            nav_collapsed_override: None,
            window_width: 1280,
            window_height: 820,
            settings_open: false,
            settings_tab: SettingsTab::SourcesAndFolders,
            crate_loading_timeout_id: 0,
            liked_uri_overrides: HashSet::new(),
            clickthrough_log: Vec::new(),
        }
    }

    pub fn is_nav_collapsed(&self) -> bool {
        self.nav_collapsed_override
            .unwrap_or(self.window_width < 980)
    }

    pub fn recompute_tokens(&mut self) -> bool {
        let next = compute_tokens(&self.view.config, self.view.player_state.cover_accent_rgba);
        if next != self.tokens {
            self.tokens = next;
            true
        } else {
            false
        }
    }

    pub fn apply_backend_event(&mut self, ev: BackendEvent) -> DirtyFlags {
        match ev {
            BackendEvent::AuthChanged(auth) => {
                self.view.auth_state = auth;
                DirtyFlags::CHROME | DirtyFlags::NAV | DirtyFlags::CONTENT | DirtyFlags::SETTINGS
            }
            BackendEvent::ConnectionChanged(conn) => {
                self.view.connection = conn;
                DirtyFlags::CHROME | DirtyFlags::SETTINGS
            }
            BackendEvent::ConfigUpdated(cfg) => {
                let mut flags = DirtyFlags::CHROME | DirtyFlags::DRAWER_EQ | DirtyFlags::SETTINGS;
                self.view.config = cfg;
                if self.recompute_tokens() {
                    flags |= DirtyFlags::THEME | DirtyFlags::ALL;
                }
                flags
            }
            BackendEvent::PlaybackChanged(mut ps) => {
                let prev_mode_track = self
                    .view
                    .player_state
                    .active_track
                    .as_ref()
                    .map(|t| t.summary.uri.clone());
                let new_track = ps.active_track.as_ref().map(|t| t.summary.uri.clone());
                if prev_mode_track != new_track {
                    // Keep cover_accent_rgba until the newly loaded cover art overrides it.
                    if let Some(ref tr) = ps.active_track
                        && tr.summary.cover.is_none()
                    {
                        ps.cover_accent_rgba = pulse_assets::DEFAULT_ACCENT_RGBA;
                    }
                } else {
                    ps.cover_accent_rgba = self.view.player_state.cover_accent_rgba;
                }
                self.view.player_state = ps;
                let mut flags = DirtyFlags::TIMELINE
                    | DirtyFlags::PLAYER_META
                    | DirtyFlags::CHROME
                    | DirtyFlags::DRAWER_EQ;
                if self.recompute_tokens() {
                    flags |= DirtyFlags::THEME;
                }
                flags
            }
            BackendEvent::PositionTick { position_ms } => {
                self.view.player_state.position_ms = position_ms;
                DirtyFlags::TIMELINE | DirtyFlags::DRAWER_LYRICS_TICK
            }
            BackendEvent::QueueChanged(queue) => {
                self.view.queue = queue;
                DirtyFlags::DRAWER_QUEUE | DirtyFlags::CONTENT
            }
            BackendEvent::DevicesUpdated(devices) => {
                self.view.devices = devices;
                DirtyFlags::DEVICES | DirtyFlags::CHROME
            }
            BackendEvent::CrateHeaderLoaded {
                crate_ref,
                title,
                subtitle,
                cover,
                total,
            } => {
                self.begin_crate_transition(&crate_ref);
                self.view.crate_view.active_crate = crate_ref;
                self.view.crate_view.title = title;
                self.view.crate_view.subtitle = subtitle;
                self.view.crate_view.cover = cover;
                self.view.crate_view.loaded_tracks.set_total(total);
                self.view.crate_view.is_loading = total > 0 && self.view.crate_view.loaded_tracks.loaded_count() == 0;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT | DirtyFlags::NAV | DirtyFlags::CHROME
            }
            BackendEvent::CrateTracksPageLoaded {
                crate_ref,
                offset,
                total,
                items,
            } => {
                if !crate_refs_match(&self.view.crate_view.active_crate, &crate_ref) {
                    self.begin_crate_transition(&crate_ref);
                    self.view.crate_view.active_crate = crate_ref;
                }
                self.view.crate_view.loaded_tracks.insert_page(offset, items, total);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT
            }
            BackendEvent::PlaylistsPageLoaded {
                offset,
                total,
                items,
            } => {
                self.view.crate_view.playlists.insert_page(offset, items, total);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT
            }
            BackendEvent::AlbumsPageLoaded {
                offset,
                total,
                items,
            } => {
                self.view.crate_view.albums.insert_page(offset, items, total);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT
            }
            BackendEvent::ArtistsPageLoaded {
                offset,
                total,
                items,
            } => {
                self.view.crate_view.artists.insert_page(offset, items, total);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT
            }
            BackendEvent::ArtistDetailLoaded(detail) => {
                self.view.crate_view.artist_detail = Some(detail);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT
            }
            BackendEvent::SearchResultsLoaded(results) => {
                let query = results.query.clone();
                let source_filter = results.source_filter;
                self.view.crate_view.active_scope = LibraryScope::Search;
                self.view.crate_view.active_crate = CrateRef::Search {
                    query: query.clone(),
                    source_filter,
                };
                self.view.crate_view.search_source_filter = source_filter;
                self.view.crate_view.title = format!("Search: {query}");
                self.view.crate_view.subtitle = match source_filter {
                    Some(src) => format!("Filtered to {}", src.display_name()),
                    None => String::from("Local, Spotify, YouTube, SoundCloud"),
                };
                self.view.crate_view.search_results = Some(results);
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = None;
                DirtyFlags::CONTENT | DirtyFlags::NAV | DirtyFlags::CHROME
            }
            BackendEvent::CustomPlaylistsUpdated(list) => {
                self.view.custom_playlists = list;
                DirtyFlags::NAV | DirtyFlags::CONTENT | DirtyFlags::DRAWER_QUEUE
            }
            BackendEvent::LocalLibraryStatus {
                track_count,
                dirs,
                scanning,
            } => {
                self.view.local_track_count = track_count;
                self.view.local_dirs = dirs;
                if scanning && matches!(self.view.crate_view.active_scope, LibraryScope::LocalMusic) {
                    self.view.crate_view.subtitle = format!("{track_count} tracks (scanning)");
                }
                DirtyFlags::NAV | DirtyFlags::CONTENT | DirtyFlags::SETTINGS
            }
            BackendEvent::LyricsLoaded(lyrics) => {
                self.view.lyrics = lyrics;
                DirtyFlags::DRAWER_LYRICS
            }
            BackendEvent::ListeningStatsUpdated(stats) => {
                self.view.insights = stats;
                DirtyFlags::DRAWER_INSIGHTS
            }
            BackendEvent::LikeStateChanged { uri, is_liked } => {
                if is_liked {
                    self.liked_uri_overrides.insert(uri.clone());
                } else {
                    self.liked_uri_overrides.remove(&uri);
                }
                if let Some(ref mut active) = self.view.player_state.active_track
                    && active.summary.uri.as_ref() == uri
                {
                    active.summary.is_liked = is_liked;
                }
                for idx in 0..self.view.crate_view.loaded_tracks.total() {
                    if let Some(tr) = self.view.crate_view.loaded_tracks.get_mut(idx)
                        && tr.uri.as_ref() == uri
                    {
                        tr.is_liked = is_liked;
                    }
                }
                if let Some(ref mut search) = self.view.crate_view.search_results {
                    for tr in &mut search.tracks {
                        if tr.uri.as_ref() == uri {
                            tr.is_liked = is_liked;
                        }
                    }
                }
                DirtyFlags::PLAYER_META | DirtyFlags::CONTENT
            }
            BackendEvent::TelemetryUpdated(tele) => {
                self.view.telemetry = tele;
                DirtyFlags::SETTINGS
            }
            BackendEvent::Notice(notice) => {
                self.view.notice_banner = notice;
                DirtyFlags::CHROME
            }
            BackendEvent::Error(err) => {
                self.view.crate_view.is_loading = false;
                self.view.crate_view.error = Some(err.clone());
                self.view.notice_banner = Some(err);
                DirtyFlags::CHROME | DirtyFlags::CONTENT
            }
        }
    }

    fn begin_crate_transition(&mut self, next_ref: &CrateRef) {
        if !crate_refs_match(&self.view.crate_view.active_crate, next_ref) {
            self.view.crate_view.loaded_tracks.clear();
            self.view.crate_view.playlists.clear();
            self.view.crate_view.albums.clear();
            self.view.crate_view.artists.clear();
            self.view.crate_view.artist_detail = None;
            self.selected_track_index = None;
            self.view.crate_view.local_filter.clear();
        }
        self.view.crate_view.active_scope = scope_for_crate(next_ref);
        if let Some(sub) = local_subview_for_crate(next_ref) {
            self.view.crate_view.local_sub_view = sub;
        }
        // Push history cleanly if not already pointing at target.
        let cv = &mut self.view.crate_view;
        let same_as_cur = cv
            .history
            .get(cv.history_index)
            .is_some_and(|cur| crate_refs_match(cur, next_ref));
        if !same_as_cur {
            cv.history.truncate(cv.history_index + 1);
            cv.history.push(next_ref.clone());
            if cv.history.len() > 32 {
                cv.history.remove(0);
            }
            cv.history_index = cv.history.len().saturating_sub(1);
        }
    }

    /// Returns currently visible track slice matching local filter (for Search / Track lists).
    pub fn visible_tracks(&self) -> Vec<(usize, TrackSummary)> {
        let filter = self.view.crate_view.local_filter.trim().to_lowercase();
        let matches_filter = |tr: &TrackSummary| -> bool {
            if filter.is_empty() {
                return true;
            }
            tr.title.to_lowercase().contains(&filter)
                || tr.artist.to_lowercase().contains(&filter)
                || tr.album.to_lowercase().contains(&filter)
                || tr.source.display_name().to_lowercase().contains(&filter)
        };

        if let CrateRef::Search { .. } = &self.view.crate_view.active_crate
            && let Some(ref search) = self.view.crate_view.search_results
        {
            return search
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, tr)| matches_filter(tr))
                .map(|(i, tr)| (i, tr.clone()))
                .collect();
        }

        if let Some(ref detail) = self.view.crate_view.artist_detail
            && matches!(self.view.crate_view.active_crate, CrateRef::Artist { .. })
        {
            return detail
                .top_tracks
                .iter()
                .enumerate()
                .filter(|(_, tr)| matches_filter(tr))
                .map(|(i, tr)| (i, tr.clone()))
                .collect();
        }

        self.view
            .crate_view
            .loaded_tracks
            .iter_loaded()
            .filter(|(_, tr)| matches_filter(tr))
            .map(|(idx, tr)| (idx, tr.clone()))
            .collect()
    }
}

pub fn crate_refs_match(a: &CrateRef, b: &CrateRef) -> bool {
    match (a, b) {
        (CrateRef::Playlist { uri: u1, .. }, CrateRef::Playlist { uri: u2, .. }) => u1 == u2,
        (CrateRef::Album { uri: u1, .. }, CrateRef::Album { uri: u2, .. }) => u1 == u2,
        (CrateRef::Artist { uri: u1, .. }, CrateRef::Artist { uri: u2, .. }) => u1 == u2,
        (
            CrateRef::Search {
                query: q1,
                source_filter: f1,
            },
            CrateRef::Search {
                query: q2,
                source_filter: f2,
            },
        ) => q1 == q2 && f1 == f2,
        _ => a == b,
    }
}

pub fn scope_for_crate(cr: &CrateRef) -> LibraryScope {
    match cr {
        CrateRef::Home => LibraryScope::Home,
        CrateRef::LocalTracks
        | CrateRef::LocalAlbums
        | CrateRef::LocalArtists
        | CrateRef::LocalFolders
        | CrateRef::LocalAlbum { .. }
        | CrateRef::LocalArtist { .. }
        | CrateRef::LocalFolder { .. } => LibraryScope::LocalMusic,
        CrateRef::UnifiedLikes => LibraryScope::UnifiedLikes,
        CrateRef::CustomPlaylistsIndex | CrateRef::CustomPlaylist { .. } => {
            LibraryScope::CustomPlaylists
        }
        CrateRef::LikedSongs => LibraryScope::LikedSongs,
        CrateRef::PlaylistsIndex | CrateRef::Playlist { .. } | CrateRef::YouTubePlaylist { .. } | CrateRef::SoundCloudSet { .. } => {
            LibraryScope::Playlists
        }
        CrateRef::AlbumsIndex | CrateRef::Album { .. } => LibraryScope::Albums,
        CrateRef::ArtistsIndex | CrateRef::Artist { .. } => LibraryScope::Artists,
        CrateRef::Recent => LibraryScope::Recent,
        CrateRef::Search { .. } => LibraryScope::Search,
    }
}

pub fn local_subview_for_crate(cr: &CrateRef) -> Option<LocalSubView> {
    match cr {
        CrateRef::LocalTracks => Some(LocalSubView::Tracks),
        CrateRef::LocalAlbums | CrateRef::LocalAlbum { .. } => Some(LocalSubView::Albums),
        CrateRef::LocalArtists | CrateRef::LocalArtist { .. } => Some(LocalSubView::Artists),
        CrateRef::LocalFolders | CrateRef::LocalFolder { .. } => Some(LocalSubView::Folders),
        _ => None,
    }
}

pub fn source_badge_css_class(source: MediaSource) -> &'static str {
    match source {
        MediaSource::Local => "source-local",
        MediaSource::Spotify => "source-spotify",
        MediaSource::YouTube => "source-youtube",
        MediaSource::SoundCloud => "source-soundcloud",
    }
}

pub fn format_duration_ms(ms: u32) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins}:{secs:02}")
    }
}

pub fn format_listen_secs(secs: u64) -> String {
    let mins = secs / 60;
    let hours = mins / 60;
    if hours > 0 {
        format!("{hours}h {}m", mins % 60)
    } else if mins > 0 {
        format!("{mins}m {rem}s", rem = secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub fn default_crate_state() -> CrateViewState {
    CrateViewState::default()
}

pub fn sort_order_label(order: SortOrder) -> &'static str {
    match order {
        SortOrder::Custom => "Default Order",
        SortOrder::RecentlyAdded => "Recently Added",
        SortOrder::Title => "Title (A-Z)",
        SortOrder::Artist => "Artist (A-Z)",
        SortOrder::Duration => "Duration",
    }
}
