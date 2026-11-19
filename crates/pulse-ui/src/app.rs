//! Iced 0.14 runtime wiring, bounded cover scheduling, lazy pagination, and overlay shell.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, mouse_area, operation, row, stack, text,
};
use iced::{Element, Length, Padding, Subscription, Task, Theme, mouse, window};
use pulse_assets::{
    CoverCache, CoverStats, CoverTier as AssetCoverTier, MAX_TINT_ALPHA, build_radial_tint_rgba,
    icons,
};
use pulse_model::{
    AppMode, AppViewState, AuthState, BackendEvent, ConnectionStatus, CrateRef, ImageRef,
    LibraryScope, TrackSummary, UiCommand,
};
use tokio::sync::mpsc;

use crate::cover_store::CoverTier;
use crate::message::UiMsg;
use crate::omnibox::omnibox_entries;
use crate::ui_state::{
    ActivePane, SingleThreadTokio, TrackContextMenu, UiState, id_crate_scroll, id_filter_input,
    id_omnibox_input, id_queue_scroll, same_crate_target, scope_for_crate,
};
use crate::{
    crate_view, device_popover, focus_stage, keybindings, mini_player, omnibox, onboarding,
    settings_modal, stage, theme, top_bar, widgets,
};

const GRID_CARD_ROW_HEIGHT: f32 = 176.0;
const PAGINATION_LOOKAHEAD_ROWS: usize = 20;
const MAX_THUMB_COVERS_PER_PASS: usize = 24;
const MAX_IN_FLIGHT_COVERS: usize = 16;
const COVER_FAILURE_COOLDOWN: Duration = Duration::from_secs(30);

static NEXT_BACKEND_SOURCE_ID: AtomicU64 = AtomicU64::new(1);

/// Synchronous callback dispatching UI commands to `pulse-backend`.
pub type CommandSendFn = Arc<dyn Fn(UiCommand) + Send + Sync>;

/// Async cover byte loader callback injected from `pulse-backend`.
pub type CoverFetchFn = Arc<
    dyn Fn(ImageRef) -> Pin<Box<dyn Future<Output = Result<Bytes, String>> + Send>> + Send + Sync,
>;

/// Optional callback notified whenever cover cache slots or loaded track counts change.
pub type MetricsObserverFn = Arc<dyn Fn(CoverStats, usize) + Send + Sync>;

/// Boot parameters passed by `src/main.rs` into [`run`].
pub struct PulseAppLaunch {
    pub initial_view: AppViewState,
    pub cover_cache: Arc<CoverCache>,
    pub command_sender: CommandSendFn,
    pub event_rx: Arc<StdMutex<Option<mpsc::UnboundedReceiver<BackendEvent>>>>,
    pub cover_fetcher: CoverFetchFn,
    pub metrics_observer: Option<MetricsObserverFn>,
}

#[derive(Clone)]
struct BackendRxSource {
    id: u64,
    rx: Arc<StdMutex<Option<mpsc::UnboundedReceiver<BackendEvent>>>>,
}

impl std::hash::Hash for BackendRxSource {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Top-level Iced runtime model owning [`UiState`] and bounded asset/backend channels.
pub struct PulseApp {
    pub ui: UiState,
    cover_cache: Arc<CoverCache>,
    command_sender: CommandSendFn,
    backend_source: BackendRxSource,
    cover_fetcher: CoverFetchFn,
    metrics_observer: Option<MetricsObserverFn>,
    in_flight_covers: HashSet<(CoverTier, Arc<str>)>,
    dragging_splitter: bool,
}

/// Launches the Iced GUI (`tiny-skia` CPU renderer + Wayland/X11 + single-thread Tokio).
pub fn run(launch: PulseAppLaunch) -> iced::Result {
    let launch_slot = Arc::new(StdMutex::new(Some(launch)));

    iced::application(
        move || {
            let launch = launch_slot
                .lock()
                .expect("launch slot mutex poisoned")
                .take()
                .expect("PulseApp::boot called more than once");
            PulseApp::boot(launch)
        },
        PulseApp::update,
        view,
    )
    .title("Pulse")
    .theme(PulseApp::theme)
    .subscription(PulseApp::subscription)
    .executor::<SingleThreadTokio>()
    .settings(iced::Settings {
        default_font: theme::FONT_SANS,
        ..Default::default()
    })
    .window(iced::window::Settings {
        size: UiState::WORKBENCH_DEFAULT_SIZE,
        min_size: Some(UiState::WORKBENCH_MIN_SIZE),
        decorations: true,
        platform_specific: iced::window::settings::PlatformSpecific {
            application_id: String::from("dev.pulse.Spotify"),
            override_redirect: false,
        },
        ..Default::default()
    })
    .run()
}

impl PulseApp {
    /// Initializes runtime state, resolves initial window handle, and primes visible artwork.
    pub fn boot(launch: PulseAppLaunch) -> (Self, Task<UiMsg>) {
        let mut app = Self {
            ui: UiState::new(launch.initial_view),
            cover_cache: launch.cover_cache,
            command_sender: launch.command_sender,
            backend_source: BackendRxSource {
                id: NEXT_BACKEND_SOURCE_ID.fetch_add(1, Ordering::Relaxed),
                rx: launch.event_rx,
            },
            cover_fetcher: launch.cover_fetcher,
            metrics_observer: launch.metrics_observer,
            in_flight_covers: HashSet::new(),
            dragging_splitter: false,
        };

        app.sync_radial_tint();
        app.publish_metrics();
        let cover_task = app.schedule_visible_covers();
        let win_task = window::oldest().map(UiMsg::WindowIdResolved);
        (app, Task::batch([win_task, cover_task]))
    }

    pub fn theme(&self) -> Theme {
        theme::pulse_theme()
    }

    pub fn subscription(&self) -> Subscription<UiMsg> {
        let mut subs = Vec::with_capacity(5);
        subs.push(iced::event::listen_with(keybindings::handle_global_event));
        subs.push(iced::window::resize_events().map(|(_id, sz)| UiMsg::WindowResized(sz)));
        subs.push(iced::window::close_requests().map(UiMsg::WindowCloseRequested));
        subs.push(Subscription::run_with(
            self.backend_source.clone(),
            backend_event_stream,
        ));

        let remote_active = self
            .ui
            .view
            .devices
            .active_device_id
            .as_ref()
            .is_some_and(|id| id != &self.ui.view.devices.local_device_id);
        if self.ui.view.player_state.is_playing && remote_active {
            subs.push(iced::time::every(Duration::from_secs(1)).map(|_| UiMsg::Tick1Hz));
        }

        Subscription::batch(subs)
    }

    pub fn update(&mut self, msg: UiMsg) -> Task<UiMsg> {
        match msg {
            UiMsg::Noop => Task::none(),
            UiMsg::Command(cmd) => self.handle_ui_command(cmd),
            UiMsg::Backend(ev) => self.handle_backend_event(ev),
            UiMsg::Tick1Hz => {
                let ps = &mut self.ui.view.player_state;
                if ps.is_playing && ps.duration_ms > 0 {
                    ps.position_ms = ps.position_ms.saturating_add(1_000).min(ps.duration_ms);
                }
                Task::none()
            }
            UiMsg::SeekPreview(ms) => {
                self.ui.scrubbing_pos_ms = Some(ms);
                Task::none()
            }
            UiMsg::SeekCommit => {
                if let Some(ms) = self.ui.scrubbing_pos_ms.take() {
                    self.ui.view.player_state.position_ms = ms;
                    return self.handle_ui_command(UiCommand::SeekMs(ms));
                }
                Task::none()
            }
            UiMsg::WindowResized(size) => {
                let was_narrow = self.ui.is_narrow_workbench();
                self.ui.window_size = size;
                if matches!(self.ui.view.mode, AppMode::Workbench | AppMode::FocusStage)
                    && size.height > 200.0
                {
                    self.ui.workbench_size = size;
                }
                if was_narrow != self.ui.is_narrow_workbench() {
                    self.ui.scrubbing_pos_ms = None;
                    self.ui.dragging_queue_from = None;
                    self.ui.hovered_track_row = None;
                    self.ui.mini_player_hovered = false;
                    self.ui.confirm_clear_queue_open = false;
                    self.dragging_splitter = false;
                }
                Task::none()
            }
            UiMsg::WindowIdResolved(id) => {
                self.ui.window_id = id;
                self.apply_window_constraints_for_mode(self.ui.view.mode)
            }
            UiMsg::WindowCloseRequested(id) => window::close(id),

            UiMsg::CoverReady {
                key,
                tier,
                width,
                height,
                pixels,
                accent_rgba,
                evicted,
            } => {
                self.in_flight_covers.remove(&(tier, Arc::clone(&key)));
                self.ui.failed_covers.remove(&(tier, Arc::clone(&key)));
                for (ev_id, ev_tier) in evicted {
                    self.ui.covers.remove(&ev_id, ev_tier);
                }
                if tier == CoverTier::Thumb && self.ui.view.mode == AppMode::MiniPlayer {
                    let _ = self.cover_cache.clear_thumbs();
                    self.ui.covers.clear_thumbs();
                    self.publish_metrics();
                    return Task::none();
                }
                if width > 0 && height > 0 && !pixels.is_empty() {
                    self.ui.covers.insert_decoded(
                        Arc::clone(&key),
                        tier,
                        width,
                        height,
                        pixels,
                        accent_rgba,
                    );
                    if tier == CoverTier::Hero
                        && active_track_cover_id(&self.ui) == Some(key.as_ref())
                    {
                        self.ui.view.player_state.cover_accent_rgba = accent_rgba;
                        self.sync_radial_tint();
                    }
                    self.publish_metrics();
                }
                self.schedule_visible_covers()
            }
            UiMsg::CoverFailed { key, tier } => {
                self.in_flight_covers.remove(&(tier, Arc::clone(&key)));
                self.ui
                    .failed_covers
                    .insert((tier, key), Instant::now());
                self.schedule_visible_covers()
            }
            UiMsg::RadialTintReady {
                width,
                height,
                pixels,
            } => {
                self.ui.covers.set_radial_tint(width, height, pixels);
                Task::none()
            }

            UiMsg::SwitchMode(mode) => self.switch_mode(mode),
            UiMsg::ToggleQueueInFocus => {
                if self.ui.window_size.width >= 900.0 {
                    self.ui.show_focus_queue = !self.ui.show_focus_queue;
                }
                self.schedule_visible_covers()
            }
            UiMsg::ToggleAlwaysOnTop => {
                self.ui.always_on_top = !self.ui.always_on_top;
                self.apply_window_level()
            }
            UiMsg::ToggleNarrowQueue => {
                self.ui.show_narrow_queue = !self.ui.show_narrow_queue;
                self.schedule_visible_covers()
            }
            UiMsg::SwitchFocusPane => {
                self.ui.active_pane = match self.ui.active_pane {
                    ActivePane::Crate => ActivePane::Queue,
                    ActivePane::Queue => ActivePane::Crate,
                };
                Task::none()
            }
            UiMsg::DragSplitter(width) => {
                if width < -1.5 {
                    self.dragging_splitter = false;
                } else if width <= 0.0 {
                    self.dragging_splitter = true;
                } else {
                    self.dragging_splitter = false;
                    self.ui.set_stage_width(width);
                }
                Task::none()
            }

            UiMsg::CrateScrolled(vp) => {
                self.ui.crate_scroll_y = vp.absolute_offset().y;
                self.ui.crate_viewport_height = vp.bounds().height.max(120.0);
                self.maybe_request_crate_pages();
                self.schedule_visible_covers()
            }
            UiMsg::QueueScrolled(vp) => {
                self.ui.queue_scroll_y = vp.absolute_offset().y;
                self.ui.queue_viewport_height = vp.bounds().height.max(120.0);
                self.schedule_visible_covers()
            }
            UiMsg::SelectCrateRow(idx) => {
                self.ui.active_pane = ActivePane::Crate;
                self.ui.selected_crate_row = Some(idx);
                self.ui.context_menu = None;
                Task::none()
            }
            UiMsg::SelectQueueRow(idx) => {
                self.ui.active_pane = ActivePane::Queue;
                self.ui.selected_queue_row = Some(idx);
                self.ui.context_menu = None;
                Task::none()
            }
            UiMsg::HoverTrackRowEnter(idx) => {
                self.ui.hovered_track_row = Some(idx);
                Task::none()
            }
            UiMsg::HoverTrackRowExit(idx) => {
                if self.ui.hovered_track_row == Some(idx) {
                    self.ui.hovered_track_row = None;
                }
                Task::none()
            }
            UiMsg::HoverMiniPlayer(hovered) => {
                self.ui.mini_player_hovered = hovered;
                Task::none()
            }
            UiMsg::OpenTrackContextMenu {
                pane,
                index,
                track,
                pos,
            } => {
                self.ui.device_popover_open = false;
                let menu_pos = if pos == iced::Point::ORIGIN {
                    let cursor = keybindings::last_cursor_pos();
                    if cursor != iced::Point::ORIGIN {
                        cursor
                    } else {
                        iced::Point::new(560.0, (180.0 + (index % 12) as f32 * 40.0).min(620.0))
                    }
                } else {
                    pos
                };
                self.ui.context_menu = Some(TrackContextMenu {
                    pane,
                    index,
                    track,
                    pos: menu_pos,
                });
                Task::none()
            }
            UiMsg::CloseContextMenu => {
                self.ui.context_menu = None;
                Task::none()
            }

            UiMsg::QueueMoveUp(idx) => {
                if idx > 0 && idx < self.ui.view.queue.manual_items.len() {
                    self.ui.selected_queue_row = Some(idx - 1);
                    self.send_cmd(UiCommand::ReorderManualQueue {
                        from: idx,
                        to: idx - 1,
                    });
                }
                Task::none()
            }
            UiMsg::QueueMoveDown(idx) => {
                let manual_len = self.ui.view.queue.manual_items.len();
                if idx + 1 < manual_len {
                    self.ui.selected_queue_row = Some(idx + 1);
                    self.send_cmd(UiCommand::ReorderManualQueue {
                        from: idx,
                        to: idx + 1,
                    });
                }
                Task::none()
            }
            UiMsg::DragQueueStart(idx) => {
                self.ui.dragging_queue_from = Some(idx);
                self.ui.selected_queue_row = Some(idx);
                Task::none()
            }
            UiMsg::DragQueueDrop(to) => {
                if let Some(from) = self.ui.dragging_queue_from.take() {
                    let manual_len = self.ui.view.queue.manual_items.len();
                    if to <= manual_len && from < manual_len && manual_len > 1 {
                        let target = to.min(manual_len - 1);
                        if from != target {
                            self.ui.selected_queue_row = Some(target);
                            self.send_cmd(UiCommand::ReorderManualQueue { from, to: target });
                        }
                    }
                }
                Task::none()
            }

            UiMsg::FocusFilter => {
                self.ui.input_focused = true;
                operation::focus(id_filter_input())
            }
            UiMsg::FilterInputChanged(val) => {
                self.ui.input_focused = true;
                self.ui.view.crate_view.local_filter = val.clone();
                self.ui.crate_scroll_y = 0.0;
                self.ui.selected_crate_row = if val.trim().is_empty() {
                    if self.ui.view.crate_view.loaded_tracks.total() > 0 {
                        Some(0)
                    } else {
                        None
                    }
                } else {
                    keybindings::first_filtered_crate_index(&self.ui)
                };
                self.send_cmd(UiCommand::SetLocalFilter(val));
                Task::batch([
                    operation::scroll_to(
                        id_crate_scroll(),
                        operation::AbsoluteOffset {
                            x: None,
                            y: Some(0.0),
                        },
                    ),
                    self.schedule_visible_covers(),
                ])
            }
            UiMsg::ClearFilter => {
                self.ui.view.crate_view.local_filter.clear();
                self.ui.input_focused = false;
                self.ui.crate_scroll_y = 0.0;
                if self.ui.selected_crate_row.is_none()
                    && self.ui.view.crate_view.loaded_tracks.total() > 0
                {
                    self.ui.selected_crate_row = Some(0);
                }
                self.send_cmd(UiCommand::SetLocalFilter(String::new()));
                Task::batch([
                    operation::scroll_to(
                        id_crate_scroll(),
                        operation::AbsoluteOffset {
                            x: None,
                            y: Some(0.0),
                        },
                    ),
                    self.schedule_visible_covers(),
                ])
            }
            UiMsg::OpenOmnibox => {
                self.ui.device_popover_open = false;
                self.ui.context_menu = None;
                self.ui.omnibox_open = true;
                self.ui.omnibox_query.clear();
                self.ui.omnibox_selected = 0;
                self.ui.input_focused = true;
                operation::focus(id_omnibox_input())
            }
            UiMsg::CloseOmnibox => {
                self.ui.omnibox_open = false;
                self.ui.input_focused = false;
                Task::none()
            }
            UiMsg::OmniboxInputChanged(query) => {
                self.ui.omnibox_query = query;
                self.ui.omnibox_selected = 0;
                self.ui.input_focused = true;
                Task::none()
            }
            UiMsg::OmniboxSubmit => {
                let entries = omnibox_entries(&self.ui);
                if entries.is_empty() {
                    let raw = self.ui.omnibox_query.trim().to_string();
                    self.ui.omnibox_open = false;
                    self.ui.input_focused = false;
                    if !raw.is_empty() && !raw.starts_with('>') {
                        return self.handle_ui_command(UiCommand::RunGlobalSearch(raw));
                    }
                    return Task::none();
                }
                let idx = self.ui.omnibox_selected.min(entries.len() - 1);
                let action = entries[idx].action.clone();
                self.execute_omnibox_action(action)
            }
            UiMsg::OmniboxSelect(idx) => {
                let entries = omnibox_entries(&self.ui);
                if let Some(entry) = entries.into_iter().nth(idx) {
                    return self.execute_omnibox_action(entry.action);
                }
                Task::none()
            }

            UiMsg::RequestClearManualQueue => {
                self.ui.context_menu = None;
                self.ui.device_popover_open = false;
                if self.ui.view.config.confirm_clear_queue {
                    self.ui.confirm_clear_queue_open = true;
                    return Task::none();
                }
                self.send_cmd(UiCommand::ClearManualQueue);
                Task::none()
            }
            UiMsg::ConfirmClearManualQueue => {
                self.ui.confirm_clear_queue_open = false;
                self.send_cmd(UiCommand::ClearManualQueue);
                Task::none()
            }
            UiMsg::CancelClearManualQueue => {
                self.ui.confirm_clear_queue_open = false;
                Task::none()
            }
            UiMsg::ToggleSettings => {
                self.ui.settings_open = !self.ui.settings_open;
                self.ui.device_popover_open = false;
                self.ui.context_menu = None;
                self.ui.omnibox_open = false;
                self.ui.confirm_clear_queue_open = false;
                if !self.ui.settings_open {
                    self.ui.input_focused = false;
                }
                Task::none()
            }
            UiMsg::SetSettingsTab(tab) => {
                self.ui.settings_tab = tab;
                Task::none()
            }
            UiMsg::SettingsClientIdChanged(val) => {
                self.ui.settings_client_id_draft = val.clone();
                let trimmed = val.trim();
                self.ui.view.config.custom_client_id = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
                self.ui.input_focused = true;
                Task::none()
            }
            UiMsg::SettingsDeviceNameChanged(val) => {
                self.ui.settings_device_name_draft = val.clone();
                self.ui.view.config.device_name = val;
                self.ui.input_focused = true;
                Task::none()
            }
            UiMsg::ToggleDevicePopover => {
                self.ui.device_popover_open = !self.ui.device_popover_open;
                self.ui.context_menu = None;
                if self.ui.device_popover_open {
                    self.send_cmd(UiCommand::RefreshDevices);
                }
                Task::none()
            }
            UiMsg::ClosePopovers => {
                self.ui.device_popover_open = false;
                self.ui.context_menu = None;
                self.ui.omnibox_open = false;
                self.ui.settings_open = false;
                self.ui.confirm_clear_queue_open = false;
                self.ui.show_narrow_queue = false;
                self.ui.input_focused = false;
                Task::none()
            }
            UiMsg::CopyToClipboard(content) => {
                self.ui.context_menu = None;
                self.ui.status_toast = Some(String::from("Copied to clipboard"));
                iced::clipboard::write(content)
            }
            UiMsg::DismissNotice => {
                if let ConnectionStatus::Error(err) = &self.ui.view.connection {
                    self.ui.dismissed_conn_error = Some(err.clone());
                }
                self.ui.view.notice_banner = None;
                self.ui.status_toast = None;
                Task::none()
            }
            UiMsg::CursorMoved(pos) => {
                self.ui.cursor_pos = pos;
                if self.dragging_splitter {
                    self.ui.set_stage_width(pos.x);
                }
                Task::none()
            }
            UiMsg::KeyEvent(ev, status) => {
                if matches!(ev, iced::Event::Mouse(mouse::Event::ButtonReleased(_))) {
                    self.dragging_splitter = false;
                }
                let mut queued_cmds = Vec::new();
                let mut followups = vec![keybindings::dispatch_global_event(
                    &mut self.ui,
                    ev,
                    status,
                    |cmd| {
                        queued_cmds.push(cmd);
                        Task::none()
                    },
                )];
                for cmd in queued_cmds {
                    followups.push(self.handle_ui_command(cmd));
                }
                self.maybe_request_crate_pages();
                followups.push(self.schedule_visible_covers());
                Task::batch(followups)
            }
            UiMsg::InputFocusChanged(focused) => {
                self.ui.input_focused = focused;
                Task::none()
            }
        }
    }

    fn execute_omnibox_action(&mut self, action: UiMsg) -> Task<UiMsg> {
        if let UiMsg::OmniboxInputChanged(prefix) = action {
            self.ui.omnibox_query = prefix;
            self.ui.omnibox_selected = 0;
            self.ui.omnibox_open = true;
            self.ui.input_focused = true;
            return operation::focus(id_omnibox_input());
        }
        self.ui.omnibox_open = false;
        self.ui.input_focused = false;
        self.update(action)
    }

    fn handle_ui_command(&mut self, cmd: UiCommand) -> Task<UiMsg> {
        if !matches!(cmd, UiCommand::RefreshDevices) {
            self.ui.context_menu = None;
        }
        if matches!(cmd, UiCommand::TransferPlayback { .. }) {
            self.ui.device_popover_open = false;
        }

        match cmd {
            UiCommand::SetMode(mode) => return self.switch_mode(mode),
            UiCommand::PurgeCoverCache => {
                let evicted = self.cover_cache.purge_ram();
                self.ui.covers.purge();
                self.ui.last_radial_accent = None;
                self.ui.failed_covers.clear();
                for (id, tier) in evicted {
                    self.in_flight_covers.remove(&(from_asset_tier(tier), id));
                }
                self.in_flight_covers.clear();
                self.publish_metrics();
                self.send_cmd(UiCommand::PurgeCoverCache);
                return self.schedule_visible_covers();
            }
            UiCommand::ClearManualQueue => {
                if self.ui.view.config.confirm_clear_queue {
                    self.ui.confirm_clear_queue_open = true;
                    return Task::none();
                }
                self.send_cmd(UiCommand::ClearManualQueue);
                return Task::none();
            }
            UiCommand::SetLocalFilter(filter) => {
                self.ui.view.crate_view.local_filter = filter.clone();
                self.ui.crate_scroll_y = 0.0;
                if filter.trim().is_empty()
                    && self.ui.selected_crate_row.is_none()
                    && self.ui.view.crate_view.loaded_tracks.total() > 0
                {
                    self.ui.selected_crate_row = Some(0);
                }
                self.send_cmd(UiCommand::SetLocalFilter(filter));
                return Task::batch([
                    operation::scroll_to(
                        id_crate_scroll(),
                        operation::AbsoluteOffset {
                            x: None,
                            y: Some(0.0),
                        },
                    ),
                    self.schedule_visible_covers(),
                ]);
            }
            UiCommand::SetSearchFilter(filter) => {
                self.ui.view.crate_view.search_filter = filter;
                return Task::none();
            }
            UiCommand::SetSortOrder(sort) => {
                self.ui.view.crate_view.sort_order = sort;
                let mut reset_scroll = false;
                if matches!(
                    self.ui.view.crate_view.active_crate,
                    CrateRef::LikedSongs
                        | CrateRef::Playlist { .. }
                        | CrateRef::Album { .. }
                        | CrateRef::Recent
                        | CrateRef::AlbumsIndex
                ) {
                    if matches!(self.ui.view.crate_view.active_crate, CrateRef::AlbumsIndex) {
                        self.ui.view.crate_view.albums.clear();
                    } else {
                        self.ui.view.crate_view.loaded_tracks.clear();
                    }
                    self.ui.crate_scroll_y = 0.0;
                    self.ui.view.crate_view.is_loading = true;
                    reset_scroll = true;
                }
                self.send_cmd(UiCommand::SetSortOrder(sort));
                if reset_scroll {
                    return operation::scroll_to(
                        id_crate_scroll(),
                        operation::AbsoluteOffset {
                            x: None,
                            y: Some(0.0),
                        },
                    );
                }
                return Task::none();
            }
            UiCommand::SetScope(scope) => {
                if scope == LibraryScope::Search {
                    if let Some(results) = &self.ui.view.crate_view.search_results {
                        let target = CrateRef::Search {
                            query: results.query.clone(),
                        };
                        let scroll_task = self.activate_crate_view(target, true, false);
                        return Task::batch([scroll_task, self.schedule_visible_covers()]);
                    }
                    self.ui.view.crate_view.is_loading = true;
                    self.send_cmd(UiCommand::SetScope(LibraryScope::Search));
                    return Task::none();
                }
                let target = crate_for_scope(scope);
                let scroll_task = self.activate_crate_view(target, true, true);
                self.send_cmd(UiCommand::SetScope(scope));
                return Task::batch([scroll_task, self.schedule_visible_covers()]);
            }
            UiCommand::OpenCrate(target) => {
                let scroll_task = self.activate_crate_view(target.clone(), true, true);
                self.send_cmd(UiCommand::OpenCrate(target));
                return Task::batch([scroll_task, self.schedule_visible_covers()]);
            }
            UiCommand::RunGlobalSearch(query) => {
                let target = CrateRef::Search {
                    query: query.clone(),
                };
                self.ui.view.crate_view.search_results = None;
                let scroll_task = self.activate_crate_view(target, true, true);
                self.send_cmd(UiCommand::RunGlobalSearch(query));
                return scroll_task;
            }
            UiCommand::NavigateBack => {
                return self.navigate_history(false);
            }
            UiCommand::NavigateForward => {
                return self.navigate_history(true);
            }
            UiCommand::SetZeroconf {
                enabled,
                device_name,
            } => {
                self.ui.view.config.zeroconf_enabled = enabled;
                let trimmed = device_name.trim();
                if !trimmed.is_empty() {
                    self.ui.view.config.device_name = trimmed.to_string();
                    self.ui.settings_device_name_draft = trimmed.to_string();
                }
                self.send_cmd(UiCommand::SetZeroconf {
                    enabled,
                    device_name,
                });
            }
            UiCommand::SaveClientId(val) => {
                let normalized = val
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                self.ui.view.config.custom_client_id = normalized.clone();
                self.ui.settings_client_id_draft = normalized.unwrap_or_default();
                self.send_cmd(UiCommand::SaveClientId(val));
            }
            UiCommand::SetStreamQuality(q) => {
                self.ui.view.config.stream_quality = q;
                self.send_cmd(UiCommand::SetStreamQuality(q));
            }
            UiCommand::SetNormalisation(v) => {
                self.ui.view.config.normalisation = v;
                self.send_cmd(UiCommand::SetNormalisation(v));
            }
            UiCommand::SetGapless(v) => {
                self.ui.view.config.gapless = v;
                self.send_cmd(UiCommand::SetGapless(v));
            }
            UiCommand::SetAudioDevice(dev) => {
                self.ui.view.config.audio_device = dev.clone();
                self.send_cmd(UiCommand::SetAudioDevice(dev));
            }
            UiCommand::SetMprisEnabled(v) => {
                self.ui.view.config.mpris_enabled = v;
                self.send_cmd(UiCommand::SetMprisEnabled(v));
            }
            UiCommand::SetConfirmClearQueue(v) => {
                self.ui.view.config.confirm_clear_queue = v;
                self.send_cmd(UiCommand::SetConfirmClearQueue(v));
            }
            UiCommand::SetDiskCacheLimitMb(mb) => {
                self.ui.view.config.disk_audio_cache_mb = mb;
                self.send_cmd(UiCommand::SetDiskCacheLimitMb(mb));
            }
            UiCommand::SetVolume(vol) => {
                self.ui.view.player_state.volume = vol;
                if vol > 0 {
                    self.ui.view.player_state.muted = false;
                }
                self.send_cmd(UiCommand::SetVolume(vol));
            }
            UiCommand::ToggleMute => {
                self.ui.view.player_state.muted = !self.ui.view.player_state.muted;
                self.send_cmd(UiCommand::ToggleMute);
            }
            other => {
                self.send_cmd(other);
            }
        }
        Task::none()
    }

    fn navigate_history(&mut self, forward: bool) -> Task<UiMsg> {
        let Some(target) = self.ui.step_history(forward) else {
            return Task::none();
        };
        if let CrateRef::Search { query } = &target {
            let cached_hit = self
                .ui
                .view
                .crate_view
                .search_results
                .as_ref()
                .is_some_and(|r| r.query == *query);
            if cached_hit {
                let scroll_task = self.activate_crate_view(target, false, false);
                return Task::batch([scroll_task, self.schedule_visible_covers()]);
            }
            let q = query.clone();
            let scroll_task = self.activate_crate_view(target, false, true);
            self.send_cmd(UiCommand::RunGlobalSearch(q));
            return scroll_task;
        }
        let scroll_task = self.activate_crate_view(target.clone(), false, true);
        self.send_cmd(UiCommand::OpenCrate(target));
        Task::batch([scroll_task, self.schedule_visible_covers()])
    }

    fn activate_crate_view(
        &mut self,
        target: CrateRef,
        push_history: bool,
        is_loading: bool,
    ) -> Task<UiMsg> {
        if push_history {
            self.ui.push_history(target.clone());
        }
        let crate_changed = !same_crate_target(&self.ui.view.crate_view.active_crate, &target);
        {
            let cv = &mut self.ui.view.crate_view;
            cv.active_crate = target.clone();
            cv.active_scope = scope_for_crate(&target);
            cv.is_loading = is_loading;
            cv.error = None;
            cv.local_filter.clear();
            if crate_changed {
                cv.loaded_tracks.clear();
                cv.artist_detail = None;
                cv.title = default_title_for_crate(&target);
                cv.subtitle.clear();
                cv.cover = None;
            }
        }
        if crate_changed {
            self.ui.crate_scroll_y = 0.0;
            self.ui.selected_crate_row = None;
            self.ui.hovered_track_row = None;
            operation::scroll_to(
                id_crate_scroll(),
                operation::AbsoluteOffset {
                    x: None,
                    y: Some(0.0),
                },
            )
        } else {
            Task::none()
        }
    }

    fn handle_backend_event(&mut self, ev: BackendEvent) -> Task<UiMsg> {
        let was_authenticated = matches!(self.ui.view.auth_state, AuthState::Authenticated { .. });
        let prev_queue_scroll = self.ui.queue_scroll_y;
        let should_schedule_pages = matches!(
            ev,
            BackendEvent::CrateHeaderLoaded { .. }
                | BackendEvent::CrateTracksPageLoaded { .. }
                | BackendEvent::PlaylistsPageLoaded { .. }
                | BackendEvent::AlbumsPageLoaded { .. }
                | BackendEvent::ArtistsPageLoaded { .. }
        );
        let should_sync_tint = matches!(ev, BackendEvent::PlaybackChanged(_));
        let should_refresh_covers = matches!(
            ev,
            BackendEvent::PlaybackChanged(_)
                | BackendEvent::QueueChanged(_)
                | BackendEvent::CrateHeaderLoaded { .. }
                | BackendEvent::CrateTracksPageLoaded { .. }
                | BackendEvent::PlaylistsPageLoaded { .. }
                | BackendEvent::AlbumsPageLoaded { .. }
                | BackendEvent::ArtistsPageLoaded { .. }
                | BackendEvent::ArtistDetailLoaded(_)
                | BackendEvent::SearchResultsLoaded(_)
                | BackendEvent::AuthChanged(_)
        );

        self.ui.apply_backend_event(ev);
        self.publish_metrics();

        let is_authenticated = matches!(self.ui.view.auth_state, AuthState::Authenticated { .. });
        if !was_authenticated
            && is_authenticated
            && self.ui.view.crate_view.loaded_tracks.total() == 0
            && !self.ui.view.crate_view.is_loading
        {
            self.ui.view.crate_view.is_loading = true;
        }
        if was_authenticated && !is_authenticated {
            self.in_flight_covers.clear();
            self.dragging_splitter = false;
        }

        if should_sync_tint {
            self.sync_radial_tint();
        }
        if should_schedule_pages {
            self.maybe_request_crate_pages();
        }

        let mut tasks = Vec::with_capacity(2);
        if (self.ui.queue_scroll_y - prev_queue_scroll).abs() > f32::EPSILON {
            tasks.push(operation::scroll_to(
                id_queue_scroll(),
                operation::AbsoluteOffset {
                    x: None,
                    y: Some(self.ui.queue_scroll_y),
                },
            ));
        }
        if should_refresh_covers {
            tasks.push(self.schedule_visible_covers());
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn switch_mode(&mut self, mode: AppMode) -> Task<UiMsg> {
        let prev_mode = self.ui.view.mode;
        if prev_mode == mode {
            return Task::none();
        }

        if prev_mode != AppMode::MiniPlayer {
            self.ui.mode_before_mini = prev_mode;
            if self.ui.window_size.height > 200.0 {
                self.ui.workbench_size = self.ui.window_size;
            }
        }

        self.ui.view.mode = mode;
        self.ui.context_menu = None;
        self.ui.device_popover_open = false;
        self.ui.omnibox_open = false;
        self.ui.show_narrow_queue = false;
        self.ui.confirm_clear_queue_open = false;
        self.ui.scrubbing_pos_ms = None;
        self.ui.dragging_queue_from = None;
        self.ui.hovered_track_row = None;
        self.ui.mini_player_hovered = false;
        self.dragging_splitter = false;

        if mode == AppMode::MiniPlayer {
            let evicted = self.cover_cache.clear_thumbs();
            self.ui.covers.clear_thumbs();
            for (id, _) in evicted {
                self.in_flight_covers.remove(&(CoverTier::Thumb, id));
            }
            self.publish_metrics();
        }

        self.send_cmd(UiCommand::SetMode(mode));

        let mut tasks = Vec::with_capacity(4);
        tasks.push(self.apply_window_constraints_for_mode(mode));
        if let Some(id) = self.ui.window_id {
            match mode {
                AppMode::MiniPlayer => {
                    tasks.push(window::resize(id, UiState::MINI_DEFAULT_SIZE));
                }
                AppMode::Workbench | AppMode::FocusStage if prev_mode == AppMode::MiniPlayer => {
                    tasks.push(window::resize(id, self.ui.workbench_size));
                }
                _ => {}
            }
        }
        tasks.push(self.schedule_visible_covers());
        Task::batch(tasks)
    }

    fn apply_window_constraints_for_mode(&self, mode: AppMode) -> Task<UiMsg> {
        let Some(id) = self.ui.window_id else {
            return Task::none();
        };
        let (min_size, max_size) = match mode {
            AppMode::Workbench => (Some(UiState::WORKBENCH_MIN_SIZE), None),
            AppMode::FocusStage => (Some(UiState::FOCUS_MIN_SIZE), None),
            AppMode::MiniPlayer => (
                Some(UiState::MINI_MIN_SIZE),
                Some(UiState::MINI_MAX_SIZE),
            ),
        };
        Task::batch([
            window::set_min_size(id, min_size),
            window::set_max_size(id, max_size),
            self.apply_window_level(),
        ])
    }

    fn apply_window_level(&self) -> Task<UiMsg> {
        let Some(id) = self.ui.window_id else {
            return Task::none();
        };
        let level = if self.ui.view.mode == AppMode::MiniPlayer && self.ui.always_on_top {
            window::Level::AlwaysOnTop
        } else {
            window::Level::Normal
        };
        window::set_level(id, level)
    }

    fn send_cmd(&self, cmd: UiCommand) {
        (self.command_sender)(cmd);
    }

    fn publish_metrics(&self) {
        if let Some(observer) = &self.metrics_observer {
            let cv = &self.ui.view.crate_view;
            let loaded_tracks = cv.loaded_tracks.loaded_count()
                + self.ui.view.queue.manual_items.len()
                + self.ui.view.queue.context_items.len();
            observer(self.cover_cache.stats(), loaded_tracks);
        }
    }

    fn sync_radial_tint(&mut self) {
        let Some(active) = &self.ui.view.player_state.active_track else {
            self.ui.last_radial_accent = None;
            return;
        };
        let accent = if let Some(cached) = active
            .summary
            .cover
            .as_ref()
            .and_then(|c| self.ui.covers.accent_for(&c.id))
        {
            self.ui.view.player_state.cover_accent_rgba = cached;
            cached
        } else {
            self.ui.view.player_state.cover_accent_rgba
        };

        if self.ui.last_radial_accent == Some(accent) && self.ui.covers.radial_tint().is_some() {
            return;
        }
        let (w, h, pixels) = build_radial_tint_rgba(accent, MAX_TINT_ALPHA);
        self.ui.last_radial_accent = Some(accent);
        self.ui.covers.set_radial_tint(w, h, pixels);
    }

    /// Requests missing `SparsePagedList` pages near the visible scroll viewport.
    fn maybe_request_crate_pages(&mut self) {
        let offsets = compute_missing_page_offsets(&self.ui);
        if offsets.is_empty() {
            return;
        }

        let cv = &mut self.ui.view.crate_view;
        let crate_ref = cv.active_crate.clone();
        let page_size = match crate_ref {
            CrateRef::PlaylistsIndex => cv.playlists.page_size(),
            CrateRef::AlbumsIndex => cv.albums.page_size(),
            CrateRef::ArtistsIndex => cv.artists.page_size(),
            CrateRef::LikedSongs
            | CrateRef::Playlist { .. }
            | CrateRef::Album { .. }
            | CrateRef::Recent => cv.loaded_tracks.page_size(),
            CrateRef::Artist { .. } | CrateRef::Search { .. } => return,
        };

        for offset in offsets {
            let claimed = match crate_ref {
                CrateRef::PlaylistsIndex => cv.playlists.mark_in_flight(offset),
                CrateRef::AlbumsIndex => cv.albums.mark_in_flight(offset),
                CrateRef::ArtistsIndex => cv.artists.mark_in_flight(offset),
                _ => cv.loaded_tracks.mark_in_flight(offset),
            };
            if claimed {
                (self.command_sender)(UiCommand::FetchCratePage {
                    crate_ref: crate_ref.clone(),
                    offset,
                    limit: page_size,
                });
            }
        }
    }

    /// Ensures active Hero cover and visible list/grid Thumbnails are decoded inside LRU caps.
    fn schedule_visible_covers(&mut self) -> Task<UiMsg> {
        let mut tasks = Vec::new();

        if let Some(cover) = self
            .ui
            .view
            .player_state
            .active_track
            .as_ref()
            .and_then(|t| t.summary.cover.clone())
            && let Some(task) = self.ensure_cover(CoverTier::Hero, cover)
        {
            tasks.push(task);
        }

        if self.ui.view.mode == AppMode::MiniPlayer {
            return if tasks.is_empty() {
                Task::none()
            } else {
                Task::batch(tasks)
            };
        }

        let candidates = collect_visible_thumb_refs(&self.ui);
        for img in candidates.into_iter().take(MAX_THUMB_COVERS_PER_PASS) {
            if self.in_flight_covers.len() >= MAX_IN_FLIGHT_COVERS {
                break;
            }
            if let Some(task) = self.ensure_cover(CoverTier::Thumb, img) {
                tasks.push(task);
            }
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn ensure_cover(&mut self, tier: CoverTier, image: ImageRef) -> Option<Task<UiMsg>> {
        let hit = match tier {
            CoverTier::Hero => self.ui.covers.hero(&image.id).is_some(),
            CoverTier::Thumb => self.ui.covers.thumb(&image.id).is_some(),
        };
        if hit {
            self.ui.covers.touch(&image.id, tier);
            return None;
        }

        let lookup_key = (tier, Arc::clone(&image.id));
        if let Some(failed_at) = self.ui.failed_covers.get(&lookup_key) {
            if failed_at.elapsed() < COVER_FAILURE_COOLDOWN {
                return None;
            }
            self.ui.failed_covers.remove(&lookup_key);
        }

        let asset_tier = to_asset_tier(tier);
        if let Some(decoded) = self.cover_cache.get_cached(asset_tier, &image.id) {
            self.ui.covers.insert_decoded(
                Arc::clone(&image.id),
                tier,
                decoded.width,
                decoded.height,
                decoded.pixels,
                decoded.accent_rgba,
            );
            if tier == CoverTier::Hero && active_track_cover_id(&self.ui) == Some(image.id.as_ref())
            {
                self.ui.view.player_state.cover_accent_rgba = decoded.accent_rgba;
                self.sync_radial_tint();
            }
            return None;
        }

        if !self.in_flight_covers.insert(lookup_key) {
            return None;
        }

        let cache = Arc::clone(&self.cover_cache);
        let fetcher = Arc::clone(&self.cover_fetcher);
        let fallback_id = Arc::clone(&image.id);
        Some(Task::perform(
            async move { cache.get_or_fetch(asset_tier, image, move |img| fetcher(img)).await },
            move |result| match result {
                Ok(fetched) => UiMsg::CoverReady {
                    key: fetched.id,
                    tier: from_asset_tier(fetched.tier),
                    width: fetched.cover.width,
                    height: fetched.cover.height,
                    pixels: fetched.cover.pixels,
                    accent_rgba: fetched.cover.accent_rgba,
                    evicted: fetched
                        .evicted
                        .into_iter()
                        .map(|(id, t)| (id, from_asset_tier(t)))
                        .collect(),
                },
                Err(_) => UiMsg::CoverFailed {
                    key: fallback_id,
                    tier,
                },
            },
        ))
    }
}

/// Top-level Iced `view()` function building the active mode shell and overlay stack.
pub fn view(app: &PulseApp) -> Element<'_, UiMsg> {
    let state = &app.ui;

    if matches!(
        state.view.auth_state,
        AuthState::Unauthenticated | AuthState::WaitingOAuth { .. }
    ) {
        let base = onboarding::view(state);
        return if state.settings_open {
            stack![base, settings_modal::view(state)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            base
        };
    }

    if state.view.mode == AppMode::MiniPlayer {
        let base = mini_player::view(state);
        return if state.settings_open {
            stack![base, settings_modal::view(state)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            base
        };
    }

    let mut shell = column![top_bar::view(state)]
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(banner) = view_notice_strip(state) {
        shell = shell.push(banner);
    }

    let workspace = match state.view.mode {
        AppMode::FocusStage => focus_stage::view(state),
        AppMode::MiniPlayer => mini_player::view(state),
        AppMode::Workbench if state.is_narrow_workbench() => {
            let main_col = column![
                container(crate_view::view(state))
                    .width(Length::Fill)
                    .height(Length::Fill),
                stage::view_narrow_strip(state),
            ]
            .width(Length::Fill)
            .height(Length::Fill);

            if state.show_narrow_queue {
                let sheet = container(
                    column![
                        Space::new().height(Length::Fill),
                        mouse_area(
                            container(stage::view_queue_panel(state, theme::TRACK_ROW_HEIGHT))
                                .width(Length::Fill)
                                .height(Length::Fixed(320.0))
                                .style(theme::popover_panel),
                        )
                        .on_press(UiMsg::Noop),
                        Space::new().height(Length::Fixed(64.0)),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
                .padding([0.0, theme::SPACE_MD])
                .width(Length::Fill)
                .height(Length::Fill);

                let dismiss_backdrop = mouse_area(
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .style(theme::modal_backdrop),
                )
                .on_press(UiMsg::ToggleNarrowQueue);

                stack![main_col, dismiss_backdrop, sheet]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                main_col.into()
            }
        }
        AppMode::Workbench => {
            let splitter_active = app.dragging_splitter;
            let splitter = mouse_area(
                container(Space::new().width(Length::Fixed(4.0)).height(Length::Fill)).style(
                    move |_theme: &Theme| container::Style {
                        background: Some(iced::Background::Color(if splitter_active {
                            theme::ACCENT_PRIMARY
                        } else {
                            theme::BORDER_SUBTLE
                        })),
                        ..Default::default()
                    },
                ),
            )
            .interaction(mouse::Interaction::ResizingHorizontally)
            .on_press(UiMsg::DragSplitter(-1.0));

            row![
                stage::view(state),
                splitter,
                container(crate_view::view(state))
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
    };

    shell = shell.push(workspace);

    let mut root_area = mouse_area(
        container(shell)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::canvas_container),
    );

    if app.dragging_splitter {
        root_area = root_area
            .on_move(UiMsg::CursorMoved)
            .on_release(UiMsg::DragSplitter(-2.0));
    } else if state.dragging_queue_from.is_some() {
        root_area = root_area.on_release(UiMsg::DragQueueDrop(usize::MAX));
    }

    let mut layers = stack![root_area]
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(menu) = &state.context_menu {
        layers = layers.push(view_context_menu(state, menu));
    }
    if state.device_popover_open {
        layers = layers.push(device_popover::view(state));
    }
    if state.omnibox_open {
        layers = layers.push(omnibox::view(state));
    }
    if state.confirm_clear_queue_open {
        layers = layers.push(view_confirm_clear_queue_modal(state));
    }
    if state.settings_open {
        layers = layers.push(settings_modal::view(state));
    }

    layers.into()
}

fn view_confirm_clear_queue_modal<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let count = state.view.queue.manual_items.len();
    let noun = if count == 1 { "track" } else { "tracks" };
    let body = if count > 0 {
        format!("Remove {count} {noun} from your manual priority queue?")
    } else {
        String::from("Clear all items from your manual priority queue?")
    };

    let card = mouse_area(
        container(
            column![
                text("Clear manual queue?")
                    .size(16)
                    .font(theme::FONT_SANS_BOLD)
                    .color(theme::TEXT_PRIMARY),
                text(body)
                    .size(13)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_SECONDARY),
                row![
                    Space::new().width(Length::Fill),
                    button(text("Cancel").size(12).font(theme::FONT_SANS_MEDIUM))
                        .padding(Padding::from([7.0, 14.0]))
                        .style(theme::secondary_button)
                        .on_press(UiMsg::CancelClearManualQueue),
                    button(text("Clear Queue").size(12).font(theme::FONT_SANS_MEDIUM))
                        .padding(Padding::from([7.0, 14.0]))
                        .style(theme::danger_button)
                        .on_press(UiMsg::ConfirmClearManualQueue),
                ]
                .spacing(theme::SPACE_SM)
                .align_y(Vertical::Center),
            ]
            .spacing(theme::SPACE_MD),
        )
        .width(Length::Fixed(380.0))
        .padding(theme::SPACE_LG)
        .style(theme::popover_panel),
    )
    .on_press(UiMsg::Noop);

    let backdrop = mouse_area(
        container(Space::new().width(Length::Fill).height(Length::Fill))
            .style(theme::modal_backdrop),
    )
    .on_press(UiMsg::CancelClearManualQueue);

    stack![
        backdrop,
        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn view_notice_strip<'a>(state: &'a UiState) -> Option<Element<'a, UiMsg>> {
    let (msg, is_err) = if let Some(banner) = state.view.notice_banner.as_deref() {
        let err = banner.to_ascii_lowercase().contains("error")
            || banner.to_ascii_lowercase().contains("failed");
        (banner, err)
    } else {
        let toast = state.status_toast.as_deref()?;
        (toast, false)
    };

    let fg = if is_err {
        theme::STATUS_ERR
    } else {
        theme::STATUS_WARN
    };

    Some(
        container(
            row![
                text(msg).size(12).font(theme::FONT_SANS).color(fg),
                Space::new().width(Length::Fill),
                button(widgets::icon_svg(icons::CLOSE, 12.0, fg))
                    .padding([2.0, 5.0])
                    .style(theme::ghost_button)
                    .on_press(UiMsg::DismissNotice),
            ]
            .spacing(theme::SPACE_SM)
            .align_y(Vertical::Center),
        )
        .padding([4.0, theme::SPACE_LG])
        .width(Length::Fill)
        .style(theme::banner_container(is_err))
        .into(),
    )
}

fn view_context_menu<'a>(
    state: &'a UiState,
    menu: &'a TrackContextMenu,
) -> Element<'a, UiMsg> {
    let track = &menu.track;
    let menu_w = 228.0_f32;
    let menu_h = 264.0_f32;
    let x = menu
        .pos
        .x
        .clamp(8.0, (state.window_size.width - menu_w - 8.0).max(8.0));
    let y = menu
        .pos
        .y
        .clamp(8.0, (state.window_size.height - menu_h - 8.0).max(8.0));

    let play_cmd = if menu.pane == ActivePane::Queue {
        UiCommand::PlayQueueIndex(menu.index)
    } else {
        UiCommand::PlayTrackInContext {
            context_uri: context_uri_for_track(&state.view.crate_view.active_crate, track),
            track_uri: track.uri.to_string(),
            index: menu.index,
        }
    };

    let mut items = column![
        container(
            column![
                container(
                    text(&track.title)
                        .size(12)
                        .font(theme::FONT_SANS_SEMIBOLD)
                        .color(theme::TEXT_PRIMARY)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
                container(
                    text(track.artist.as_ref())
                        .size(11)
                        .font(theme::FONT_SANS)
                        .color(theme::TEXT_MUTED)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
            ]
            .spacing(2),
        )
        .padding([4.0, 8.0]),
        widgets::hairline_h(),
    ]
    .spacing(2);

    if track.is_playable {
        items = items.push(ctx_menu_item("Play Now", None, UiMsg::Command(play_cmd)));
    }
    items = items
        .push(ctx_menu_item(
            "Play Next",
            Some("Q"),
            UiMsg::Command(UiCommand::EnqueueNext(track.clone())),
        ))
        .push(ctx_menu_item(
            "Add to Queue",
            Some("Shift+Q"),
            UiMsg::Command(UiCommand::AppendQueue(track.clone())),
        ))
        .push(ctx_menu_item(
            if track.is_liked {
                "Remove from Liked"
            } else {
                "Save to Liked Songs"
            },
            Some("L"),
            UiMsg::Command(UiCommand::ToggleLike(track.uri.to_string())),
        ));

    if let Some(album_uri) = &track.album_uri {
        items = items.push(ctx_menu_item(
            "Go to Album",
            None,
            UiMsg::Command(UiCommand::OpenCrate(CrateRef::Album {
                uri: Arc::clone(album_uri),
                title: track.album.to_string(),
            })),
        ));
    }
    if let Some(artist_uri) = &track.artist_uri {
        items = items.push(ctx_menu_item(
            "Go to Artist",
            None,
            UiMsg::Command(UiCommand::OpenCrate(CrateRef::Artist {
                uri: Arc::clone(artist_uri),
                name: track.artist.to_string(),
            })),
        ));
    }

    items = items
        .push(widgets::hairline_h())
        .push(ctx_menu_item(
            "Copy Spotify URI",
            None,
            UiMsg::CopyToClipboard(track.uri.to_string()),
        ));

    let card = mouse_area(
        container(items)
            .width(Length::Fixed(menu_w))
            .padding(6)
            .style(theme::popover_panel),
    )
    .on_press(UiMsg::Noop);

    let positioned = container(
        column![
            Space::new().height(Length::Fixed(y)),
            row![Space::new().width(Length::Fixed(x)), card],
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill);

    mouse_area(positioned)
        .on_press(UiMsg::CloseContextMenu)
        .into()
}

fn ctx_menu_item<'a>(
    label: &'a str,
    shortcut: Option<&'a str>,
    msg: UiMsg,
) -> Element<'a, UiMsg> {
    let mut r = row![
        text(label)
            .size(12)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_PRIMARY)
            .width(Length::Fill),
    ]
    .align_y(Vertical::Center);
    if let Some(key) = shortcut {
        r = r.push(
            text(key)
                .size(11)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_MUTED),
        );
    }
    button(r)
        .width(Length::Fill)
        .padding(Padding::from([6.0, 8.0]))
        .style(theme::ghost_button)
        .on_press(msg)
        .into()
}

fn backend_event_stream(
    source: &BackendRxSource,
) -> iced::futures::stream::BoxStream<'static, UiMsg> {
    let rx = source.rx.lock().ok().and_then(|mut guard| guard.take());
    Box::pin(iced::futures::stream::unfold(rx, |maybe_rx| async move {
        let mut rx = maybe_rx?;
        let ev = rx.recv().await?;
        Some((UiMsg::Backend(ev), Some(rx)))
    }))
}

fn active_track_cover_id(state: &UiState) -> Option<&str> {
    state
        .view
        .player_state
        .active_track
        .as_ref()
        .and_then(|t| t.summary.cover.as_ref())
        .map(|c| c.id.as_ref())
}

fn crate_for_scope(scope: LibraryScope) -> CrateRef {
    match scope {
        LibraryScope::LikedSongs => CrateRef::LikedSongs,
        LibraryScope::Playlists => CrateRef::PlaylistsIndex,
        LibraryScope::Albums => CrateRef::AlbumsIndex,
        LibraryScope::Artists => CrateRef::ArtistsIndex,
        LibraryScope::Recent => CrateRef::Recent,
        LibraryScope::Search => CrateRef::Search {
            query: String::new(),
        },
    }
}

fn default_title_for_crate(crate_ref: &CrateRef) -> String {
    match crate_ref {
        CrateRef::LikedSongs => String::from("Liked Songs"),
        CrateRef::PlaylistsIndex => String::from("Playlists"),
        CrateRef::AlbumsIndex => String::from("Albums"),
        CrateRef::ArtistsIndex => String::from("Artists"),
        CrateRef::Recent => String::from("Recently Played"),
        CrateRef::Playlist { name, .. } | CrateRef::Artist { name, .. } => {
            if name.is_empty() {
                String::from("Loading...")
            } else {
                name.clone()
            }
        }
        CrateRef::Album { title, .. } => {
            if title.is_empty() {
                String::from("Album")
            } else {
                title.clone()
            }
        }
        CrateRef::Search { query } => {
            if query.is_empty() {
                String::from("Search")
            } else {
                format!("Search: {query}")
            }
        }
    }
}

fn context_uri_for_track(active_crate: &CrateRef, _track: &TrackSummary) -> String {
    match active_crate {
        CrateRef::Playlist { uri, .. }
        | CrateRef::Album { uri, .. }
        | CrateRef::Artist { uri, .. } => uri.to_string(),
        CrateRef::LikedSongs => String::from("spotify:collection:tracks"),
        CrateRef::Recent => String::from("recent"),
        CrateRef::Search { query } => format!("spotify:search:{query}"),
        CrateRef::PlaylistsIndex | CrateRef::AlbumsIndex | CrateRef::ArtistsIndex => {
            String::from("spotify:collection:tracks")
        }
    }
}

fn grid_columns(state: &UiState) -> usize {
    let crate_w = if state.is_narrow_workbench() {
        state.window_size.width
    } else {
        (state.window_size.width - state.stage_width).max(320.0)
    };
    ((crate_w / 210.0).floor() as usize).clamp(2, 4)
}

fn compute_missing_page_offsets(state: &UiState) -> Vec<usize> {
    let cv = &state.view.crate_view;
    let scroll_y = state.crate_scroll_y.max(0.0);
    let vp_h = state.crate_viewport_height.max(120.0);

    let grid_range = || {
        let cols = grid_columns(state);
        let first = ((scroll_y / GRID_CARD_ROW_HEIGHT).floor() as usize) * cols;
        let last = ((scroll_y + vp_h) / GRID_CARD_ROW_HEIGHT).ceil() as usize * cols;
        (first, last)
    };

    match cv.active_crate {
        CrateRef::PlaylistsIndex => {
            let (first, last) = grid_range();
            unloaded_pages_in_window(&cv.playlists, first, last)
        }
        CrateRef::AlbumsIndex => {
            let (first, last) = grid_range();
            unloaded_pages_in_window(&cv.albums, first, last)
        }
        CrateRef::ArtistsIndex => {
            let (first, last) = grid_range();
            unloaded_pages_in_window(&cv.artists, first, last)
        }
        CrateRef::LikedSongs
        | CrateRef::Playlist { .. }
        | CrateRef::Album { .. }
        | CrateRef::Recent => {
            let first = (scroll_y / theme::TRACK_ROW_HEIGHT).floor() as usize;
            let last = ((scroll_y + vp_h) / theme::TRACK_ROW_HEIGHT).ceil() as usize;
            unloaded_pages_in_window(&cv.loaded_tracks, first, last)
        }
        CrateRef::Artist { .. } | CrateRef::Search { .. } => Vec::new(),
    }
}

fn unloaded_pages_in_window<T>(
    list: &pulse_model::SparsePagedList<T>,
    first_idx: usize,
    last_idx: usize,
) -> Vec<usize> {
    let total = list.total();
    let page_size = list.page_size();
    if total == 0 || page_size == 0 {
        return Vec::new();
    }

    let win_start = first_idx.saturating_sub(5);
    let win_end = (last_idx + PAGINATION_LOOKAHEAD_ROWS).min(total - 1);
    if win_start > win_end {
        return Vec::new();
    }

    let first_page = win_start - (win_start % page_size);
    let last_page = win_end - (win_end % page_size);

    let mut offsets = Vec::new();
    let mut page = first_page;
    while page <= last_page {
        if !list.is_page_loaded(page) {
            offsets.push(page);
        }
        page = match page.checked_add(page_size) {
            Some(next) => next,
            None => break,
        };
    }
    offsets
}

fn collect_visible_thumb_refs(state: &UiState) -> Vec<ImageRef> {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();

    let mut push_unique = |img: Option<&ImageRef>| {
        if let Some(img) = img
            && seen.insert(Arc::clone(&img.id))
        {
            refs.push(img.clone());
        }
    };

    // Prioritize visible Queue artwork first (up to 8 visible items).
    let q_first = (state.queue_scroll_y.max(0.0) / theme::TRACK_ROW_HEIGHT).floor() as usize;
    let q_count = ((state.queue_viewport_height.max(120.0) / theme::TRACK_ROW_HEIGHT).ceil()
        as usize)
        + 4;
    let queue_iter = state
        .view
        .queue
        .manual_items
        .iter()
        .chain(state.view.queue.context_items.iter());
    for track in queue_iter.skip(q_first.saturating_sub(2)).take(q_count.min(10)) {
        push_unique(track.cover.as_ref());
    }

    if state.view.mode == AppMode::FocusStage {
        return refs;
    }

    let cv = &state.view.crate_view;
    push_unique(cv.cover.as_ref());

    let scroll_y = state.crate_scroll_y.max(0.0);
    let vp_h = state.crate_viewport_height.max(120.0);

    match &cv.active_crate {
        CrateRef::PlaylistsIndex => {
            let cols = grid_columns(state);
            let first = ((scroll_y / GRID_CARD_ROW_HEIGHT).floor() as usize).saturating_sub(1);
            let vis = (vp_h / GRID_CARD_ROW_HEIGHT).ceil() as usize + 3;
            for idx in (first * cols)..((first + vis) * cols).min(cv.playlists.total()) {
                if let Some(pl) = cv.playlists.get(idx) {
                    push_unique(pl.cover.as_ref());
                }
            }
        }
        CrateRef::AlbumsIndex => {
            let cols = grid_columns(state);
            let first = ((scroll_y / GRID_CARD_ROW_HEIGHT).floor() as usize).saturating_sub(1);
            let vis = (vp_h / GRID_CARD_ROW_HEIGHT).ceil() as usize + 3;
            for idx in (first * cols)..((first + vis) * cols).min(cv.albums.total()) {
                if let Some(al) = cv.albums.get(idx) {
                    push_unique(al.cover.as_ref());
                }
            }
        }
        CrateRef::ArtistsIndex => {
            let cols = grid_columns(state);
            let first = ((scroll_y / GRID_CARD_ROW_HEIGHT).floor() as usize).saturating_sub(1);
            let vis = (vp_h / GRID_CARD_ROW_HEIGHT).ceil() as usize + 3;
            for idx in (first * cols)..((first + vis) * cols).min(cv.artists.total()) {
                if let Some(ar) = cv.artists.get(idx) {
                    push_unique(ar.cover.as_ref());
                }
            }
        }
        CrateRef::Artist { .. } => {
            if let Some(detail) = &cv.artist_detail {
                for track in detail.top_tracks.iter().take(10) {
                    push_unique(track.cover.as_ref());
                }
                for al in detail.albums.iter().take(12) {
                    push_unique(al.cover.as_ref());
                }
                for single in detail.singles.iter().take(12) {
                    push_unique(single.cover.as_ref());
                }
            }
        }
        CrateRef::Search { .. } => {
            if let Some(res) = &cv.search_results {
                for track in res.tracks.iter().take(14) {
                    push_unique(track.cover.as_ref());
                }
                for ar in res.artists.iter().take(6) {
                    push_unique(ar.cover.as_ref());
                }
                for al in res.albums.iter().take(6) {
                    push_unique(al.cover.as_ref());
                }
                for pl in res.playlists.iter().take(6) {
                    push_unique(pl.cover.as_ref());
                }
            }
        }
        CrateRef::LikedSongs
        | CrateRef::Playlist { .. }
        | CrateRef::Album { .. }
        | CrateRef::Recent => {
            let first = ((scroll_y / theme::TRACK_ROW_HEIGHT).floor() as usize).saturating_sub(2);
            let vis = (vp_h / theme::TRACK_ROW_HEIGHT).ceil() as usize + 6;
            if let Some(matched) = keybindings::filtered_crate_indices(state) {
                let end = (first + vis).min(matched.len());
                for &idx in &matched[first.min(end)..end] {
                    if let Some(track) = cv.loaded_tracks.get(idx) {
                        push_unique(track.cover.as_ref());
                    }
                }
            } else {
                for idx in first..(first + vis).min(cv.loaded_tracks.total()) {
                    if let Some(track) = cv.loaded_tracks.get(idx) {
                        push_unique(track.cover.as_ref());
                    }
                }
            }
        }
    }

    refs
}

fn to_asset_tier(tier: CoverTier) -> AssetCoverTier {
    match tier {
        CoverTier::Thumb => AssetCoverTier::Thumb,
        CoverTier::Hero => AssetCoverTier::Hero,
    }
}

fn from_asset_tier(tier: AssetCoverTier) -> CoverTier {
    match tier {
        AssetCoverTier::Thumb => CoverTier::Thumb,
        AssetCoverTier::Hero => CoverTier::Hero,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_model::PlaylistSummary;

    fn test_app() -> (PulseApp, mpsc::UnboundedReceiver<UiCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (_ev_tx, ev_rx) = mpsc::unbounded_channel();
        let cache = Arc::new(CoverCache::with_dir(
            std::env::temp_dir().join("pulse_app_test_cache"),
            0,
        ));
        let fetcher: CoverFetchFn = Arc::new(|_| Box::pin(async { Err(String::from("offline")) }));
        let (app, _) = PulseApp::boot(PulseAppLaunch {
            initial_view: AppViewState::default(),
            cover_cache: cache,
            command_sender: Arc::new(move |cmd| {
                let _ = cmd_tx.send(cmd);
            }),
            event_rx: Arc::new(StdMutex::new(Some(ev_rx))),
            cover_fetcher: fetcher,
            metrics_observer: None,
        });
        (app, cmd_rx)
    }

    #[test]
    fn switch_mode_to_mini_clears_thumbs_and_restores_previous_mode() {
        let (mut app, mut cmd_rx) = test_app();
        app.ui.covers.insert_decoded(
            Arc::from("cover-1"),
            CoverTier::Thumb,
            4,
            4,
            Bytes::from(vec![200; 64]),
            [229, 154, 62, 255],
        );
        app.ui.covers.insert_decoded(
            Arc::from("hero-1"),
            CoverTier::Hero,
            8,
            8,
            Bytes::from(vec![180; 256]),
            [120, 180, 220, 255],
        );
        assert_eq!(app.ui.covers.thumb_count(), 1);
        assert_eq!(app.ui.covers.hero_count(), 1);

        let _ = app.update(UiMsg::SwitchMode(AppMode::FocusStage));
        let _ = app.update(UiMsg::SwitchMode(AppMode::MiniPlayer));

        assert_eq!(app.ui.view.mode, AppMode::MiniPlayer);
        assert_eq!(app.ui.mode_before_mini, AppMode::FocusStage);
        assert_eq!(app.ui.covers.thumb_count(), 0);
        assert_eq!(app.ui.covers.hero_count(), 1);

        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::SetMode(AppMode::FocusStage))
        );
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::SetMode(AppMode::MiniPlayer))
        );
    }

    #[test]
    fn omnibox_submit_and_select_dispatch_commands_cleanly() {
        let (mut app, mut cmd_rx) = test_app();
        app.ui.view.crate_view.playlists.insert_page(
            0,
            vec![PlaylistSummary {
                uri: Arc::from("spotify:playlist:night"),
                name: String::from("Night Drive"),
                owner: Arc::from("pulse"),
                track_count: 18,
                total_duration_ms: 0,
                cover: None,
                is_collaborative: false,
            }],
            1,
        );

        let _ = app.update(UiMsg::OpenOmnibox);
        let _ = app.update(UiMsg::OmniboxInputChanged(String::from("night")));
        let _ = app.update(UiMsg::OmniboxSelect(1));

        assert!(!app.ui.omnibox_open);
        assert!(!app.ui.input_focused);
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::OpenCrate(CrateRef::Playlist {
                uri: Arc::from("spotify:playlist:night"),
                name: String::from("Night Drive"),
            }))
        );
    }

    #[test]
    fn lazy_pagination_requests_unloaded_pages_without_duplicates() {
        let (mut app, mut cmd_rx) = test_app();
        app.ui.view.crate_view.active_crate = CrateRef::LikedSongs;
        app.ui.view.crate_view.loaded_tracks.set_total(200);
        app.ui.crate_scroll_y = 40.0 * theme::TRACK_ROW_HEIGHT;
        app.ui.crate_viewport_height = 500.0;

        app.maybe_request_crate_pages();
        app.maybe_request_crate_pages();

        let mut received = Vec::new();
        while let Ok(cmd) = cmd_rx.try_recv() {
            received.push(cmd);
        }
        assert_eq!(
            received,
            vec![
                UiCommand::FetchCratePage {
                    crate_ref: CrateRef::LikedSongs,
                    offset: 0,
                    limit: 50,
                },
                UiCommand::FetchCratePage {
                    crate_ref: CrateRef::LikedSongs,
                    offset: 50,
                    limit: 50,
                },
            ]
        );
    }

    #[test]
    fn cover_ready_updates_hero_accent_and_radial_tint() {
        let (mut app, _cmd_rx) = test_app();
        let active = TrackSummary {
            uri: Arc::from("spotify:track:t1"),
            title: String::from("Track"),
            artist: Arc::from("Artist"),
            artist_uri: None,
            album: Arc::from("Album"),
            album_uri: None,
            cover: Some(ImageRef {
                id: Arc::from("cov-hero"),
                url: None,
            }),
            duration_ms: 180_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
        };
        app.ui.view.player_state.active_track = Some(pulse_model::TrackDetail {
            summary: active,
            track_number: Some(1),
            disc_total_tracks: None,
            release_year: Some(2024),
            explicit: false,
        });

        let _ = app.update(UiMsg::CoverReady {
            key: Arc::from("cov-hero"),
            tier: CoverTier::Hero,
            width: 4,
            height: 4,
            pixels: Bytes::from(vec![100; 64]),
            accent_rgba: [12, 220, 180, 255],
            evicted: Vec::new(),
        });

        assert_eq!(
            app.ui.view.player_state.cover_accent_rgba,
            [12, 220, 180, 255]
        );
        assert!(app.ui.covers.radial_tint().is_some());
    }

    #[test]
    fn navigation_scope_filter_sort_and_initial_auth_load() {
        let (mut app, mut cmd_rx) = test_app();

        // Auth transition marks crate loading while backend fetches initial LikedSongs (no duplicate OpenCrate).
        let _ = app.update(UiMsg::Backend(BackendEvent::AuthChanged(
            AuthState::Authenticated {
                username: String::from("user"),
                display_name: String::from("User"),
                tier: pulse_model::AccountTier::Premium,
                web_api_ready: false,
                zeroconf_only: false,
            },
        )));
        assert!(app.ui.view.crate_view.is_loading);
        assert!(cmd_rx.try_recv().is_err());

        // SetScope navigates, records history, and dispatches SetScope cleanly.
        let _ = app.update(UiMsg::Command(UiCommand::SetScope(LibraryScope::Playlists)));
        assert_eq!(app.ui.view.crate_view.active_crate, CrateRef::PlaylistsIndex);
        assert_eq!(app.ui.view.crate_view.active_scope, LibraryScope::Playlists);
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::SetScope(LibraryScope::Playlists))
        );

        // NavigateBack & NavigateForward step through history cleanly without losing forward entries.
        let _ = app.update(UiMsg::Command(UiCommand::NavigateBack));
        assert_eq!(app.ui.view.crate_view.active_crate, CrateRef::LikedSongs);
        assert_eq!(app.ui.view.crate_view.history_index, 0);
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::OpenCrate(CrateRef::LikedSongs))
        );

        // Back at start is a clean no-op without sticking is_loading.
        app.ui.view.crate_view.is_loading = false;
        let _ = app.update(UiMsg::Command(UiCommand::NavigateBack));
        assert!(!app.ui.view.crate_view.is_loading);
        assert!(cmd_rx.try_recv().is_err());

        let _ = app.update(UiMsg::Command(UiCommand::NavigateForward));
        assert_eq!(app.ui.view.crate_view.active_crate, CrateRef::PlaylistsIndex);
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::OpenCrate(CrateRef::PlaylistsIndex))
        );

        // SetSearchFilter updates UI filter immediately.
        let _ = app.update(UiMsg::Command(UiCommand::SetSearchFilter(
            pulse_model::SearchFilter::Albums,
        )));
        assert_eq!(
            app.ui.view.crate_view.search_filter,
            pulse_model::SearchFilter::Albums
        );

        // SetSortOrder updates state, sets loading, and emits SetSortOrder (backend re-fetches 0..50).
        let _ = app.update(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Recent)));
        let _ = cmd_rx.try_recv();
        let _ = app.update(UiMsg::Command(UiCommand::SetSortOrder(
            pulse_model::SortOrder::Title,
        )));
        assert_eq!(app.ui.view.crate_view.sort_order, pulse_model::SortOrder::Title);
        assert!(app.ui.view.crate_view.is_loading);
        assert_eq!(
            cmd_rx.try_recv().ok(),
            Some(UiCommand::SetSortOrder(pulse_model::SortOrder::Title))
        );
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn confirm_clear_queue_cover_eviction_and_recent_context_uri() {
        let (mut app, mut cmd_rx) = test_app();
        app.ui.view.config.confirm_clear_queue = true;

        let _ = app.update(UiMsg::RequestClearManualQueue);
        assert!(app.ui.confirm_clear_queue_open);
        assert!(cmd_rx.try_recv().is_err());

        let _ = app.update(UiMsg::CancelClearManualQueue);
        assert!(!app.ui.confirm_clear_queue_open);
        assert!(cmd_rx.try_recv().is_err());

        let _ = app.update(UiMsg::Command(UiCommand::ClearManualQueue));
        assert!(app.ui.confirm_clear_queue_open);
        let _ = app.update(UiMsg::ConfirmClearManualQueue);
        assert!(!app.ui.confirm_clear_queue_open);
        assert_eq!(cmd_rx.try_recv().ok(), Some(UiCommand::ClearManualQueue));

        // CoverReady eviction removes evicted handle immediately.
        app.ui.covers.insert_decoded(
            Arc::from("old-thumb"),
            CoverTier::Thumb,
            4,
            4,
            Bytes::from(vec![1; 64]),
            [200, 150, 80, 255],
        );
        let _ = app.update(UiMsg::CoverReady {
            key: Arc::from("new-thumb"),
            tier: CoverTier::Thumb,
            width: 4,
            height: 4,
            pixels: Bytes::from(vec![2; 64]),
            accent_rgba: [100, 120, 140, 255],
            evicted: vec![(Arc::from("old-thumb"), CoverTier::Thumb)],
        });
        assert!(app.ui.covers.thumb("old-thumb").is_none());
        assert!(app.ui.covers.thumb("new-thumb").is_some());

        let dummy = TrackSummary {
            uri: Arc::from("spotify:track:t"),
            title: String::from("T"),
            artist: Arc::from("A"),
            artist_uri: None,
            album: Arc::from("Al"),
            album_uri: Some(Arc::from("spotify:album:al")),
            cover: None,
            duration_ms: 1000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
        };
        assert_eq!(context_uri_for_track(&CrateRef::Recent, &dummy), "recent");
    }

    #[test]
    fn optimistic_settings_cover_failure_cooldown_and_hover_race() {
        let (mut app, mut cmd_rx) = test_app();

        // Settings commands update UI config state optimistically.
        let _ = app.update(UiMsg::Command(UiCommand::SetGapless(false)));
        assert!(!app.ui.view.config.gapless);
        assert_eq!(cmd_rx.try_recv().ok(), Some(UiCommand::SetGapless(false)));

        let _ = app.update(UiMsg::Command(UiCommand::SetZeroconf {
            enabled: false,
            device_name: String::from("Living Room"),
        }));
        assert!(!app.ui.view.config.zeroconf_enabled);
        assert_eq!(app.ui.view.config.device_name, "Living Room");

        // Out-of-order HoverTrackRowExit(0) after HoverTrackRowEnter(1) does not clear row 1.
        let _ = app.update(UiMsg::HoverTrackRowEnter(0));
        let _ = app.update(UiMsg::HoverTrackRowEnter(1));
        let _ = app.update(UiMsg::HoverTrackRowExit(0));
        assert_eq!(app.ui.hovered_track_row, Some(1));
        let _ = app.update(UiMsg::HoverTrackRowExit(1));
        assert_eq!(app.ui.hovered_track_row, None);

        // CoverFailed records cooldown so ensure_cover backs off instead of busy-looping.
        let img = ImageRef {
            id: Arc::from("missing-cover"),
            url: None,
        };
        assert!(app.ensure_cover(CoverTier::Thumb, img.clone()).is_some());
        let _ = app.update(UiMsg::CoverFailed {
            key: Arc::from("missing-cover"),
            tier: CoverTier::Thumb,
        });
        assert!(app.ensure_cover(CoverTier::Thumb, img).is_none());
    }
}
