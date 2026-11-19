//! Focus-guarded keyboard shortcuts and global event interceptor for `iced 0.14`.

use std::sync::atomic::{AtomicU32, Ordering};

use iced::keyboard::key::{Key, Named};
use iced::widget::operation;
use iced::{Event, Point, Task, event, keyboard, mouse, window};
use pulse_model::{AppMode, AuthState, CrateRef, TrackSummary, UiCommand};

use crate::message::UiMsg;
use crate::omnibox::omnibox_entries;
use crate::theme::TRACK_ROW_HEIGHT;
use crate::ui_state::{
    ActivePane, UiState, id_client_id_input, id_crate_scroll, id_device_name_input,
    id_filter_input, id_omnibox_input, id_omnibox_scroll, id_queue_scroll,
};

static LAST_CURSOR_X_BITS: AtomicU32 = AtomicU32::new(0);
static LAST_CURSOR_Y_BITS: AtomicU32 = AtomicU32::new(0);

/// Returns the most recently observed pointer position without causing redraw wakeups.
pub fn last_cursor_pos() -> Point {
    let x = f32::from_bits(LAST_CURSOR_X_BITS.load(Ordering::Relaxed));
    let y = f32::from_bits(LAST_CURSOR_Y_BITS.load(Ordering::Relaxed));
    if x.is_finite() && y.is_finite() {
        Point::new(x, y)
    } else {
        Point::ORIGIN
    }
}

/// Plain `fn` pointer passed to `iced::event::listen_with`.
///
/// Tracks cursor coordinates lock-free without emitting messages on move, and emits `UiMsg::KeyEvent`
/// only for keyboard key presses and mouse button press/release transitions.
pub fn handle_global_event(
    event: Event,
    status: event::Status,
    _window: window::Id,
) -> Option<UiMsg> {
    match &event {
        Event::Mouse(mouse::Event::CursorMoved { position }) => {
            LAST_CURSOR_X_BITS.store(position.x.to_bits(), Ordering::Relaxed);
            LAST_CURSOR_Y_BITS.store(position.y.to_bits(), Ordering::Relaxed);
            None
        }
        Event::Keyboard(keyboard::Event::KeyPressed { .. }) => Some(UiMsg::KeyEvent(event, status)),
        Event::Mouse(mouse::Event::ButtonPressed(_) | mouse::Event::ButtonReleased(_)) => {
            Some(UiMsg::KeyEvent(event, status))
        }
        _ => None,
    }
}

/// Processes a global keyboard or focus-sync click event, returning resulting follow-up tasks.
pub fn dispatch_global_event<F>(
    state: &mut UiState,
    event: Event,
    status: event::Status,
    mut send_cmd: F,
) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(_)) => check_any_input_focused(),
        Event::Mouse(mouse::Event::ButtonReleased(_)) => {
            state.dragging_queue_from = None;
            Task::none()
        }
        Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            physical_key,
            modifiers,
            repeat,
            ..
        }) => {
            let named = match key.as_ref() {
                Key::Named(n) => Some(n),
                _ => None,
            };
            let ch = key.to_latin(physical_key).or_else(|| match key.as_ref() {
                Key::Character(s) if s.chars().count() == 1 => s.chars().next(),
                _ => None,
            });
            let is_onboarding = matches!(
                state.view.auth_state,
                AuthState::Unauthenticated | AuthState::WaitingOAuth { .. }
            );
            let mini = state.view.mode == AppMode::MiniPlayer;

            // 1. Esc & Ctrl+K & Ctrl+, work regardless of Status::Captured inside text_input.
            if named == Some(Named::Escape) {
                return handle_escape(state, &mut send_cmd);
            }

            if modifiers.command() {
                if let Some(c) = ch {
                    match c.to_ascii_lowercase() {
                        'k' if !is_onboarding && !mini => {
                            state.device_popover_open = false;
                            state.context_menu = None;
                            state.omnibox_open = true;
                            state.omnibox_query.clear();
                            state.omnibox_selected = 0;
                            state.input_focused = true;
                            return operation::focus(id_omnibox_input());
                        }
                        ',' => {
                            state.device_popover_open = false;
                            state.omnibox_open = false;
                            state.context_menu = None;
                            state.settings_open = !state.settings_open;
                            if !state.settings_open {
                                state.input_focused = false;
                            }
                            return Task::none();
                        }
                        _ => {}
                    }
                }
                return Task::none();
            }

            // 2. Omnibox list navigation (Up/Down) while omnibox input stays focused.
            if state.omnibox_open {
                let len = omnibox_entries(state).len();
                if len > 0 {
                    if named == Some(Named::ArrowDown) {
                        state.omnibox_selected = (state.omnibox_selected + 1) % len;
                        let target_y = ((state.omnibox_selected as f32 * 38.0) - 140.0).max(0.0);
                        return operation::scroll_to(
                            id_omnibox_scroll(),
                            operation::AbsoluteOffset {
                                x: None,
                                y: Some(target_y),
                            },
                        );
                    }
                    if named == Some(Named::ArrowUp) {
                        state.omnibox_selected = if state.omnibox_selected == 0 {
                            len - 1
                        } else {
                            state.omnibox_selected - 1
                        };
                        let target_y = ((state.omnibox_selected as f32 * 38.0) - 140.0).max(0.0);
                        return operation::scroll_to(
                            id_omnibox_scroll(),
                            operation::AbsoluteOffset {
                                x: None,
                                y: Some(target_y),
                            },
                        );
                    }
                }
                return Task::none();
            }

            // 3. Focus guard: never fire single-key shortcuts while typing, captured, modal, or onboarding.
            if status == event::Status::Captured || state.input_focused {
                return Task::none();
            }
            if state.settings_open || state.confirm_clear_queue_open || is_onboarding {
                return Task::none();
            }

            // 4. Modifier chords that are safe globally when not typing.
            if modifiers.alt() {
                if mini {
                    return Task::none();
                }
                return match named {
                    Some(Named::ArrowLeft) => send_cmd(UiCommand::NavigateBack),
                    Some(Named::ArrowRight) => send_cmd(UiCommand::NavigateForward),
                    Some(Named::ArrowUp) => move_selected_manual_queue(state, true, send_cmd),
                    Some(Named::ArrowDown) => move_selected_manual_queue(state, false, send_cmd),
                    _ => Task::none(),
                };
            }

            // 5. Named navigation & playback keys.
            if let Some(n) = named {
                return match n {
                    Named::Space => {
                        if repeat {
                            Task::none()
                        } else {
                            send_cmd(UiCommand::TogglePlayPause)
                        }
                    }
                    Named::Tab if !mini => {
                        state.active_pane = match state.active_pane {
                            ActivePane::Crate => ActivePane::Queue,
                            ActivePane::Queue => ActivePane::Crate,
                        };
                        Task::none()
                    }
                    Named::ArrowLeft => {
                        let step = if modifiers.shift() { 30_000 } else { 5_000 };
                        let next_ms = state.view.player_state.position_ms.saturating_sub(step);
                        state.view.player_state.position_ms = next_ms;
                        send_cmd(UiCommand::SeekMs(next_ms))
                    }
                    Named::ArrowRight => {
                        let step = if modifiers.shift() { 30_000 } else { 5_000 };
                        let dur = if state.view.player_state.duration_ms > 0 {
                            state.view.player_state.duration_ms
                        } else {
                            state
                                .view
                                .player_state
                                .active_track
                                .as_ref()
                                .map(|t| t.summary.duration_ms)
                                .unwrap_or(0)
                        };
                        let next_ms = if dur > 0 {
                            state
                                .view
                                .player_state
                                .position_ms
                                .saturating_add(step)
                                .min(dur)
                        } else {
                            state.view.player_state.position_ms.saturating_add(step)
                        };
                        state.view.player_state.position_ms = next_ms;
                        send_cmd(UiCommand::SeekMs(next_ms))
                    }
                    Named::ArrowDown if !mini => move_cursor(state, 1),
                    Named::ArrowUp if !mini => move_cursor(state, -1),
                    Named::PageDown if !mini => move_cursor(state, 10),
                    Named::PageUp if !mini => move_cursor(state, -10),
                    Named::Home if !mini => jump_cursor(state, true),
                    Named::End if !mini => jump_cursor(state, false),
                    Named::Enter if !mini => {
                        if repeat {
                            Task::none()
                        } else if modifiers.shift() {
                            open_selected_context_menu(state);
                            Task::none()
                        } else {
                            activate_selected_row(state, send_cmd)
                        }
                    }
                    Named::Delete | Named::Backspace if !mini => {
                        if repeat {
                            Task::none()
                        } else {
                            remove_selected_queue_row(state, send_cmd)
                        }
                    }
                    _ => Task::none(),
                };
            }

            // 6. Single-character shortcuts.
            if let Some(c) = ch {
                return dispatch_char_shortcut(state, c, modifiers, repeat, send_cmd);
            }

            Task::none()
        }
        _ => Task::none(),
    }
}

fn dispatch_char_shortcut<F>(
    state: &mut UiState,
    ch: char,
    modifiers: keyboard::Modifiers,
    repeat: bool,
    mut send_cmd: F,
) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    if repeat && !matches!(ch, '-' | '=' | '+' | 'j' | 'J' | 'k' | 'K') {
        return Task::none();
    }
    let mini = state.view.mode == AppMode::MiniPlayer;
    match ch {
        '/' if !mini => {
            state.input_focused = true;
            operation::focus(id_filter_input())
        }
        '[' => send_cmd(UiCommand::PrevTrack),
        ']' => send_cmd(UiCommand::NextTrack),
        '-' => adjust_volume(state, -3277, send_cmd),
        '=' | '+' => adjust_volume(state, 3277, send_cmd),
        '1' => switch_mode_msg(AppMode::Workbench),
        '2' => switch_mode_msg(AppMode::FocusStage),
        '3' => switch_mode_msg(AppMode::MiniPlayer),
        'j' | 'J' if !mini => move_cursor(state, 1),
        'k' | 'K' if !mini => move_cursor(state, -1),
        'g' | 'G' if !mini => jump_cursor(state, !(modifiers.shift() || ch == 'G')),
        'f' | 'F' => {
            let next = if state.view.mode == AppMode::FocusStage {
                AppMode::Workbench
            } else {
                AppMode::FocusStage
            };
            switch_mode_msg(next)
        }
        'm' | 'M' => {
            if modifiers.shift() || ch == 'M' {
                let next = if state.view.mode == AppMode::MiniPlayer {
                    state.mode_before_mini
                } else {
                    AppMode::MiniPlayer
                };
                switch_mode_msg(next)
            } else {
                send_cmd(UiCommand::ToggleMute)
            }
        }
        'q' | 'Q' if !mini => {
            if state.view.mode == AppMode::FocusStage && !modifiers.shift() && ch == 'q' {
                if state.window_size.width >= 900.0 {
                    state.show_focus_queue = !state.show_focus_queue;
                }
                return Task::none();
            }
            if let Some(track) = selected_or_active_track(state) {
                let cmd = if modifiers.shift() || ch == 'Q' {
                    UiCommand::AppendQueue(track)
                } else {
                    UiCommand::EnqueueNext(track)
                };
                return send_cmd(cmd);
            }
            Task::none()
        }
        'l' | 'L' => {
            if let Some(track) = selected_or_active_track(state) {
                return send_cmd(UiCommand::ToggleLike(track.uri.to_string()));
            }
            Task::none()
        }
        's' | 'S' => send_cmd(UiCommand::ToggleShuffle),
        'r' | 'R' => send_cmd(UiCommand::CycleRepeat),
        'd' | 'D' if !mini => {
            state.device_popover_open = !state.device_popover_open;
            if state.device_popover_open {
                return send_cmd(UiCommand::RefreshDevices);
            }
            Task::none()
        }
        _ => Task::none(),
    }
}

fn handle_escape<F>(state: &mut UiState, send_cmd: &mut F) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    if state.context_menu.is_some() {
        state.context_menu = None;
        return Task::none();
    }
    if state.confirm_clear_queue_open {
        state.confirm_clear_queue_open = false;
        return Task::none();
    }
    if state.settings_open {
        state.settings_open = false;
        state.input_focused = false;
        return Task::none();
    }
    if state.device_popover_open {
        state.device_popover_open = false;
        return Task::none();
    }
    if state.omnibox_open {
        state.omnibox_open = false;
        state.input_focused = false;
        return Task::none();
    }
    if state.input_focused || !state.view.crate_view.local_filter.is_empty() {
        state.input_focused = false;
        let had_filter = !state.view.crate_view.local_filter.is_empty();
        state.view.crate_view.local_filter.clear();
        if had_filter {
            state.crate_scroll_y = 0.0;
            return Task::batch([
                send_cmd(UiCommand::SetLocalFilter(String::new())),
                operation::scroll_to(
                    id_crate_scroll(),
                    operation::AbsoluteOffset {
                        x: None,
                        y: Some(0.0),
                    },
                ),
            ]);
        }
        return send_cmd(UiCommand::SetLocalFilter(String::new()));
    }
    if state.view.mode == AppMode::MiniPlayer {
        let target = state.mode_before_mini;
        return Task::done(UiMsg::SwitchMode(target));
    }
    Task::none()
}

fn check_any_input_focused() -> Task<UiMsg> {
    operation::is_focused(id_filter_input()).then(|f1| {
        if f1 {
            Task::done(UiMsg::InputFocusChanged(true))
        } else {
            operation::is_focused(id_omnibox_input()).then(|f2| {
                if f2 {
                    Task::done(UiMsg::InputFocusChanged(true))
                } else {
                    operation::is_focused(id_client_id_input()).then(|f3| {
                        if f3 {
                            Task::done(UiMsg::InputFocusChanged(true))
                        } else {
                            operation::is_focused(id_device_name_input())
                                .map(UiMsg::InputFocusChanged)
                        }
                    })
                }
            })
        }
    })
}

fn switch_mode_msg(mode: AppMode) -> Task<UiMsg> {
    Task::done(UiMsg::SwitchMode(mode))
}

fn adjust_volume<F>(state: &mut UiState, delta: i32, mut send_cmd: F) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    let cur = i32::from(state.view.player_state.volume);
    let next = (cur + delta).clamp(0, 65_535) as u16;
    state.view.player_state.volume = next;
    if next > 0 {
        state.view.player_state.muted = false;
    }
    send_cmd(UiCommand::SetVolume(next))
}

fn move_selected_manual_queue<F>(
    state: &mut UiState,
    up: bool,
    mut send_cmd: F,
) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    let manual_len = state.view.queue.manual_items.len();
    let Some(idx) = state.selected_queue_row else {
        return Task::none();
    };
    if idx >= manual_len {
        return Task::none();
    }
    if up && idx > 0 {
        state.selected_queue_row = Some(idx - 1);
        return send_cmd(UiCommand::ReorderManualQueue {
            from: idx,
            to: idx - 1,
        });
    }
    if !up && idx + 1 < manual_len {
        state.selected_queue_row = Some(idx + 1);
        return send_cmd(UiCommand::ReorderManualQueue {
            from: idx,
            to: idx + 1,
        });
    }
    Task::none()
}

fn queue_row_bounds(state: &UiState, row_idx: usize) -> (f32, f32) {
    let row_h = if state.view.mode == AppMode::FocusStage {
        48.0
    } else {
        TRACK_ROW_HEIGHT
    };
    let manual_len = state.view.queue.manual_items.len();
    let header_h = 26.0;
    let top = if row_idx < manual_len {
        header_h + row_idx as f32 * row_h
    } else {
        let mut y = 0.0;
        if manual_len > 0 {
            y += header_h + manual_len as f32 * row_h + 8.0;
        }
        y += header_h + (row_idx - manual_len) as f32 * row_h;
        y
    };
    (top, row_h)
}

fn move_cursor(state: &mut UiState, delta: isize) -> Task<UiMsg> {
    match state.active_pane {
        ActivePane::Crate => update_crate_cursor_pos(state, |cur, total| match cur {
            Some(c) => (c as isize + delta).clamp(0, (total - 1) as isize) as usize,
            None => if delta >= 0 { 0 } else { total - 1 },
        }),
        ActivePane::Queue => {
            let total = state.view.queue.manual_items.len() + state.view.queue.context_items.len();
            if total == 0 {
                state.selected_queue_row = None;
                return Task::none();
            }
            let cur = state.selected_queue_row.unwrap_or(0) as isize;
            let next = (cur + delta).clamp(0, (total - 1) as isize) as usize;
            state.selected_queue_row = Some(next);
            let (row_top, row_h) = queue_row_bounds(state, next);
            ensure_span_visible(
                id_queue_scroll(),
                row_top,
                row_h,
                state.queue_scroll_y,
                state.queue_viewport_height,
            )
        }
    }
}

fn jump_cursor(state: &mut UiState, to_start: bool) -> Task<UiMsg> {
    match state.active_pane {
        ActivePane::Crate => update_crate_cursor_pos(state, |_cur, total| {
            if to_start { 0 } else { total - 1 }
        }),
        ActivePane::Queue => {
            let total = state.view.queue.manual_items.len() + state.view.queue.context_items.len();
            if total == 0 {
                return Task::none();
            }
            let next = if to_start { 0 } else { total - 1 };
            state.selected_queue_row = Some(next);
            let (row_top, row_h) = queue_row_bounds(state, next);
            ensure_span_visible(
                id_queue_scroll(),
                row_top,
                row_h,
                state.queue_scroll_y,
                state.queue_viewport_height,
            )
        }
    }
}

fn update_crate_cursor_pos(
    state: &mut UiState,
    compute_pos: impl FnOnce(Option<usize>, usize) -> usize,
) -> Task<UiMsg> {
    if let Some(matched) = filtered_crate_indices(state) {
        if matched.is_empty() {
            state.selected_crate_row = None;
            return Task::none();
        }
        let cur_pos = state
            .selected_crate_row
            .and_then(|g| matched.iter().position(|&idx| idx == g));
        let next_pos = compute_pos(cur_pos, matched.len());
        state.selected_crate_row = Some(matched[next_pos]);
        return ensure_row_visible(
            id_crate_scroll(),
            next_pos,
            TRACK_ROW_HEIGHT,
            state.crate_scroll_y,
            state.crate_viewport_height,
        );
    }

    let total = crate_visible_count(state);
    if total == 0 {
        state.selected_crate_row = None;
        return Task::none();
    }
    let next = compute_pos(state.selected_crate_row, total);
    state.selected_crate_row = Some(next);
    ensure_row_visible(
        id_crate_scroll(),
        next,
        TRACK_ROW_HEIGHT,
        state.crate_scroll_y,
        state.crate_viewport_height,
    )
}

fn ensure_row_visible(
    id: iced::widget::Id,
    row_idx: usize,
    row_height: f32,
    scroll_y: f32,
    viewport_h: f32,
) -> Task<UiMsg> {
    ensure_span_visible(
        id,
        row_idx as f32 * row_height,
        row_height,
        scroll_y,
        viewport_h,
    )
}

fn ensure_span_visible(
    id: iced::widget::Id,
    row_top: f32,
    row_height: f32,
    scroll_y: f32,
    viewport_h: f32,
) -> Task<UiMsg> {
    let row_bottom = row_top + row_height;
    let vp_h = viewport_h.max(row_height);

    let target_y = if row_top < scroll_y {
        Some(row_top)
    } else if row_bottom > scroll_y + vp_h {
        Some((row_bottom - vp_h).max(0.0))
    } else {
        None
    };

    if let Some(y) = target_y {
        operation::scroll_to(id, operation::AbsoluteOffset { x: None, y: Some(y) })
    } else {
        Task::none()
    }
}

pub(crate) fn filtered_crate_indices(state: &UiState) -> Option<Vec<usize>> {
    let cv = &state.view.crate_view;
    let needle = cv.local_filter.trim().to_ascii_lowercase();
    if needle.is_empty()
        || !matches!(
            cv.active_crate,
            CrateRef::LikedSongs
                | CrateRef::Playlist { .. }
                | CrateRef::Album { .. }
                | CrateRef::Recent
        )
    {
        return None;
    }
    let indices = cv
        .loaded_tracks
        .iter_loaded()
        .filter(|(_, track)| {
            track.title.to_ascii_lowercase().contains(&needle)
                || track.artist.to_ascii_lowercase().contains(&needle)
                || track.album.to_ascii_lowercase().contains(&needle)
        })
        .map(|(idx, _)| idx)
        .collect();
    Some(indices)
}

pub(crate) fn first_filtered_crate_index(state: &UiState) -> Option<usize> {
    filtered_crate_indices(state).and_then(|indices| indices.into_iter().next())
}

fn crate_visible_count(state: &UiState) -> usize {
    let cv = &state.view.crate_view;
    if let Some(results) = &cv.search_results
        && matches!(cv.active_crate, CrateRef::Search { .. })
    {
        return results.tracks.len();
    }
    if let Some(detail) = &cv.artist_detail
        && matches!(cv.active_crate, CrateRef::Artist { .. })
    {
        return detail.top_tracks.len().min(5);
    }
    if let Some(matched) = filtered_crate_indices(state) {
        return matched.len();
    }
    cv.loaded_tracks.total()
}

fn track_at_crate_cursor(state: &UiState) -> Option<(usize, TrackSummary)> {
    let idx = state.selected_crate_row?;
    let cv = &state.view.crate_view;
    if let Some(results) = &cv.search_results
        && matches!(cv.active_crate, CrateRef::Search { .. })
    {
        return results.tracks.get(idx).cloned().map(|t| (idx, t));
    }
    if let Some(detail) = &cv.artist_detail
        && matches!(cv.active_crate, CrateRef::Artist { .. })
    {
        return if idx < 5 {
            detail.top_tracks.get(idx).cloned().map(|t| (idx, t))
        } else {
            None
        };
    }
    if let Some(matched) = filtered_crate_indices(state)
        && !matched.contains(&idx)
    {
        return None;
    }
    cv.loaded_tracks.get(idx).cloned().map(|t| (idx, t))
}

fn selected_or_active_track(state: &UiState) -> Option<TrackSummary> {
    if state.view.mode == AppMode::FocusStage {
        return state
            .view
            .player_state
            .active_track
            .as_ref()
            .map(|d| d.summary.clone());
    }
    if state.active_pane == ActivePane::Queue
        && let Some(idx) = state.selected_queue_row
    {
        let m_len = state.view.queue.manual_items.len();
        if idx < m_len {
            return state.view.queue.manual_items.get(idx).cloned();
        }
        return state.view.queue.context_items.get(idx - m_len).cloned();
    }
    track_at_crate_cursor(state)
        .map(|(_, t)| t)
        .or_else(|| {
            state
                .view
                .player_state
                .active_track
                .as_ref()
                .map(|d| d.summary.clone())
        })
}

fn activate_selected_row<F>(state: &UiState, mut send_cmd: F) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    match state.active_pane {
        ActivePane::Queue => {
            if let Some(idx) = state.selected_queue_row {
                return send_cmd(UiCommand::PlayQueueIndex(idx));
            }
            Task::none()
        }
        ActivePane::Crate => {
            if let Some((idx, track)) = track_at_crate_cursor(state)
                && track.is_playable
            {
                let ctx_uri = crate_context_uri(&state.view.crate_view.active_crate);
                return send_cmd(UiCommand::PlayTrackInContext {
                    context_uri: ctx_uri,
                    track_uri: track.uri.to_string(),
                    index: idx,
                });
            }
            Task::none()
        }
    }
}

fn open_selected_context_menu(state: &mut UiState) {
    match state.active_pane {
        ActivePane::Queue => {
            if let Some(idx) = state.selected_queue_row {
                let m_len = state.view.queue.manual_items.len();
                let track = if idx < m_len {
                    state.view.queue.manual_items.get(idx).cloned()
                } else {
                    state.view.queue.context_items.get(idx - m_len).cloned()
                };
                if let Some(track) = track {
                    let pos = iced::Point::new(
                        (state.stage_width - 240.0).max(20.0),
                        (360.0 + (idx % 6) as f32 * 40.0)
                            .min((state.window_size.height - 270.0).max(20.0)),
                    );
                    state.context_menu = Some(crate::ui_state::TrackContextMenu {
                        pane: ActivePane::Queue,
                        index: idx,
                        track,
                        pos,
                    });
                }
            }
        }
        ActivePane::Crate => {
            if let Some((idx, track)) = track_at_crate_cursor(state) {
                let pos = iced::Point::new(
                    (state.stage_width + 180.0).min((state.window_size.width - 240.0).max(20.0)),
                    180.0,
                );
                state.context_menu = Some(crate::ui_state::TrackContextMenu {
                    pane: ActivePane::Crate,
                    index: idx,
                    track,
                    pos,
                });
            }
        }
    }
}

fn remove_selected_queue_row<F>(state: &mut UiState, mut send_cmd: F) -> Task<UiMsg>
where
    F: FnMut(UiCommand) -> Task<UiMsg>,
{
    if state.active_pane != ActivePane::Queue {
        return Task::none();
    }
    if let Some(idx) = state.selected_queue_row
        && idx < state.view.queue.manual_items.len()
    {
        return send_cmd(UiCommand::RemoveManualQueueItem(idx));
    }
    Task::none()
}

fn crate_context_uri(crate_ref: &CrateRef) -> String {
    match crate_ref {
        CrateRef::Playlist { uri, .. }
        | CrateRef::Album { uri, .. }
        | CrateRef::Artist { uri, .. } => uri.to_string(),
        CrateRef::LikedSongs => String::from("spotify:collection:tracks"),
        CrateRef::Recent => String::from("recent"),
        CrateRef::Search { query } => format!("spotify:search:{query}"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn key_press(k: Key, ch: Option<&str>, modifiers: keyboard::Modifiers) -> Event {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: k.clone(),
            modified_key: k,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: ch.map(Into::into),
            repeat: false,
        })
    }

    #[test]
    fn focus_guard_blocks_shortcuts_when_typing() {
        let mut ui = UiState {
            input_focused: true,
            ..Default::default()
        };
        let mut emitted = Vec::new();
        let _ = dispatch_global_event(
            &mut ui,
            key_press(Key::Named(Named::Space), Some(" "), keyboard::Modifiers::empty()),
            event::Status::Captured,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        assert!(emitted.is_empty());

        // Esc still escapes input focus and clears filter
        ui.view.crate_view.local_filter = String::from("m83");
        let _ = dispatch_global_event(
            &mut ui,
            key_press(Key::Named(Named::Escape), None, keyboard::Modifiers::empty()),
            event::Status::Captured,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        assert!(!ui.input_focused);
        assert!(ui.view.crate_view.local_filter.is_empty());
        assert_eq!(emitted, vec![UiCommand::SetLocalFilter(String::new())]);
    }

    fn make_track(id: &str, title: &str) -> TrackSummary {
        TrackSummary {
            uri: Arc::from(format!("spotify:track:{id}")),
            title: String::from(title),
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
    fn filtered_crate_cursor_and_artist_cap_and_recent_uri() {
        let mut ui = UiState::default();
        ui.view.crate_view.loaded_tracks.insert_page(
            0,
            vec![
                make_track("0", "Alpha"),
                make_track("1", "Nightfall"),
                make_track("2", "Midnight Run"),
                make_track("3", "Midnight Echo"),
            ],
            4,
        );
        ui.view.crate_view.local_filter = String::from("midnight");
        ui.selected_crate_row = Some(0); // hidden under filter

        assert_eq!(crate_visible_count(&ui), 2);
        assert!(track_at_crate_cursor(&ui).is_none());

        let _ = move_cursor(&mut ui, 1);
        assert_eq!(ui.selected_crate_row, Some(2));
        let _ = move_cursor(&mut ui, 1);
        assert_eq!(ui.selected_crate_row, Some(3));
        let _ = move_cursor(&mut ui, 5);
        assert_eq!(ui.selected_crate_row, Some(3));
        assert_eq!(
            track_at_crate_cursor(&ui).map(|(i, t)| (i, t.title)),
            Some((3, String::from("Midnight Echo")))
        );

        let top_tracks = (0..8)
            .map(|i| make_track(&i.to_string(), &format!("Top {i}")))
            .collect();
        ui.view.crate_view.active_crate = CrateRef::Artist {
            uri: Arc::from("spotify:artist:a1"),
            name: String::from("Artist"),
        };
        ui.view.crate_view.artist_detail = Some(pulse_model::ArtistDetail {
            artist: pulse_model::ArtistSummary {
                uri: Arc::from("spotify:artist:a1"),
                name: String::from("Artist"),
                cover: None,
                is_followed: false,
            },
            top_tracks,
            albums: Vec::new(),
            singles: Vec::new(),
        });
        ui.selected_crate_row = Some(0);
        assert_eq!(crate_visible_count(&ui), 5);
        let _ = jump_cursor(&mut ui, false);
        assert_eq!(ui.selected_crate_row, Some(4));

        assert_eq!(crate_context_uri(&CrateRef::Recent), "recent");
    }

    #[test]
    fn key_repeat_skips_discrete_transport_shortcuts() {
        let mut ui = UiState::default();
        let mut emitted = Vec::new();
        let make_repeat = |k: Key, ch: Option<&str>| {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: k.clone(),
                modified_key: k,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: ch.map(Into::into),
                repeat: true,
            })
        };

        // While unauthenticated (onboarding), single-key shortcuts are blocked.
        let _ = dispatch_global_event(
            &mut ui,
            make_repeat(Key::Character("=".into()), Some("=")),
            event::Status::Ignored,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        assert!(emitted.is_empty());

        ui.view.auth_state = AuthState::ConnectOnly {
            device_name: String::from("Pulse"),
        };

        let _ = dispatch_global_event(
            &mut ui,
            make_repeat(Key::Named(Named::Space), Some(" ")),
            event::Status::Ignored,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        let _ = dispatch_global_event(
            &mut ui,
            make_repeat(Key::Character("]".into()), Some("]")),
            event::Status::Ignored,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        assert!(emitted.is_empty());

        // Continuous volume adjustment still responds to key repeat once authenticated.
        let _ = dispatch_global_event(
            &mut ui,
            make_repeat(Key::Character("=".into()), Some("=")),
            event::Status::Ignored,
            |cmd| {
                emitted.push(cmd);
                Task::none()
            },
        );
        assert_eq!(emitted.len(), 1);

        // Shift+G jumps to the end even when physical key to_latin() produces lowercase 'g'.
        ui.view.crate_view.loaded_tracks.insert_page(
            0,
            vec![make_track("0", "A"), make_track("1", "B"), make_track("2", "C")],
            3,
        );
        ui.selected_crate_row = Some(0);
        let _ = dispatch_global_event(
            &mut ui,
            key_press(
                Key::Character("g".into()),
                Some("G"),
                keyboard::Modifiers::SHIFT,
            ),
            event::Status::Ignored,
            |_| Task::none(),
        );
        assert_eq!(ui.selected_crate_row, Some(2));

        // Shift+Enter on Queue pane opens context menu tagged with ActivePane::Queue.
        ui.active_pane = ActivePane::Queue;
        ui.view.queue.manual_items = vec![make_track("q0", "Queued")];
        ui.selected_queue_row = Some(0);
        let _ = dispatch_global_event(
            &mut ui,
            key_press(
                Key::Named(Named::Enter),
                None,
                keyboard::Modifiers::SHIFT,
            ),
            event::Status::Ignored,
            |_| Task::none(),
        );
        assert_eq!(
            ui.context_menu.as_ref().map(|m| (m.pane, m.index)),
            Some((ActivePane::Queue, 0))
        );
    }
}
