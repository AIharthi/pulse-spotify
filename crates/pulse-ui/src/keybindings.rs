//! Focus-guarded keyboard shortcuts and media transport bindings for GTK4.

use gtk4::gdk::{self, Key, ModifierType};
use gtk4::glib;
use gtk4::prelude::*;
use pulse_model::{AppMode, RightDrawerTab, UiCommand};

use crate::state::{DirtyFlags, UiBridge};

/// Installs window-level keyboard shortcuts that yield cleanly when text inputs have focus.
pub fn install_key_controller(window: &gtk4::ApplicationWindow, bridge: &UiBridge) {
    let ctrl = gtk4::EventControllerKey::new();
    ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let win_weak = window.downgrade();
    let b = bridge.clone();
    ctrl.connect_key_pressed(move |_, key, _keycode, mods| {
        let Some(win) = win_weak.upgrade() else {
            return glib::Propagation::Proceed;
        };

        let is_ctrl = mods.contains(ModifierType::CONTROL_MASK);
        let is_shift = mods.contains(ModifierType::SHIFT_MASK);

        // Always allow global Ctrl combos & Escape cleanly.
        if is_ctrl && (key == Key::k || key == Key::K) {
            b.store.borrow_mut().omnibox_global_mode = true;
            (b.focus_omnibox)(true);
            return glib::Propagation::Stop;
        }
        if is_ctrl && key == Key::comma {
            b.store.borrow_mut().settings_open = true;
            (b.redraw)(DirtyFlags::SETTINGS);
            return glib::Propagation::Stop;
        }
        if key == Key::Escape {
            let mut s = b.store.borrow_mut();
            if s.settings_open {
                s.settings_open = false;
                drop(s);
                (b.redraw)(DirtyFlags::SETTINGS);
                return glib::Propagation::Stop;
            }
            if !s.omnibox_text.is_empty() {
                s.omnibox_text.clear();
                s.omnibox_global_mode = false;
                drop(s);
                (b.send)(UiCommand::SetLocalFilter(String::new()));
                (b.focus_omnibox)(false);
                return glib::Propagation::Stop;
            }
            if s.view.mode == AppMode::FocusStage {
                drop(s);
                (b.send)(UiCommand::SetMode(AppMode::Workbench));
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }

        // Focus guard: never hijack typing when focus sits on an Editable / Entry / Text widget.
        if let Some(focus) = gtk4::prelude::GtkWindowExt::focus(&win)
            && (focus.is::<gtk4::Editable>() || focus.is::<gtk4::Text>() || focus.is::<gtk4::Entry>())
        {
            return glib::Propagation::Proceed;
        }

        match key {
            Key::space => {
                (b.send)(UiCommand::TogglePlayPause);
                glib::Propagation::Stop
            }
            Key::slash => {
                b.store.borrow_mut().omnibox_global_mode = false;
                (b.focus_omnibox)(true);
                glib::Propagation::Stop
            }
            Key::bracketleft => {
                (b.send)(UiCommand::PrevTrack);
                glib::Propagation::Stop
            }
            Key::bracketright => {
                (b.send)(UiCommand::NextTrack);
                glib::Propagation::Stop
            }
            Key::Left => {
                let delta: i64 = if is_shift { -15_000 } else { -5_000 };
                let cur = b.store.borrow().view.player_state.position_ms as i64;
                (b.send)(UiCommand::SeekMs((cur + delta).max(0) as u32));
                glib::Propagation::Stop
            }
            Key::Right => {
                let delta: i64 = if is_shift { 15_000 } else { 5_000 };
                let (cur, dur) = {
                    let s = b.store.borrow();
                    (
                        s.view.player_state.position_ms as i64,
                        s.view.player_state.duration_ms as i64,
                    )
                };
                let target = if dur > 0 {
                    (cur + delta).min(dur)
                } else {
                    cur + delta
                };
                (b.send)(UiCommand::SeekMs(target.max(0) as u32));
                glib::Propagation::Stop
            }
            Key::minus | Key::KP_Subtract => {
                let cur = b.store.borrow().view.player_state.volume;
                (b.send)(UiCommand::SetVolume(cur.saturating_sub(3277)));
                glib::Propagation::Stop
            }
            Key::equal | Key::plus | Key::KP_Add => {
                let cur = b.store.borrow().view.player_state.volume;
                (b.send)(UiCommand::SetVolume(cur.saturating_add(3277)));
                glib::Propagation::Stop
            }
            Key::m | Key::M if is_shift => {
                (b.send)(UiCommand::SetMode(AppMode::MiniPlayer));
                glib::Propagation::Stop
            }
            Key::m | Key::M => {
                (b.send)(UiCommand::ToggleMute);
                glib::Propagation::Stop
            }
            Key::s | Key::S => {
                (b.send)(UiCommand::ToggleShuffle);
                glib::Propagation::Stop
            }
            Key::r | Key::R => {
                (b.send)(UiCommand::CycleRepeat);
                glib::Propagation::Stop
            }
            Key::b | Key::B => {
                (b.send)(UiCommand::ToggleAbLoopStep);
                glib::Propagation::Stop
            }
            Key::l | Key::L => {
                if let Some(tr) = b
                    .store
                    .borrow()
                    .view
                    .player_state
                    .active_track
                    .as_ref()
                    .map(|t| t.summary.clone())
                {
                    (b.send)(UiCommand::ToggleLikeTrack(tr));
                }
                glib::Propagation::Stop
            }
            Key::q | Key::Q => {
                {
                    let mut s = b.store.borrow_mut();
                    s.view.right_drawer = if s.view.right_drawer == RightDrawerTab::Queue {
                        RightDrawerTab::Closed
                    } else {
                        RightDrawerTab::Queue
                    };
                }
                (b.redraw)(DirtyFlags::CHROME | DirtyFlags::DRAWER_QUEUE);
                glib::Propagation::Stop
            }
            Key::f | Key::F | Key::_2 => {
                (b.send)(UiCommand::SetMode(AppMode::FocusStage));
                glib::Propagation::Stop
            }
            Key::_1 => {
                (b.send)(UiCommand::SetMode(AppMode::Workbench));
                glib::Propagation::Stop
            }
            Key::_3 => {
                (b.send)(UiCommand::SetMode(AppMode::MiniPlayer));
                glib::Propagation::Stop
            }
            Key::j | Key::J | Key::Down => {
                move_selection(&b, 1);
                glib::Propagation::Stop
            }
            Key::k | Key::K | Key::Up => {
                move_selection(&b, -1);
                glib::Propagation::Stop
            }
            Key::Return | Key::KP_Enter => {
                let chosen = {
                    let s = b.store.borrow();
                    let tracks = s.visible_tracks();
                    let idx = s.selected_track_index.unwrap_or(0);
                    tracks.into_iter().find(|(i, _)| *i == idx).map(|(i, tr)| (i, tr, s.view.crate_view.title.clone()))
                };
                if let Some((_i, tr, title)) = chosen {
                    let ctx_tracks = b.store.borrow().visible_tracks().into_iter().map(|(_, t)| t).collect();
                    (b.send)(UiCommand::PlayTrackDirect {
                        track: tr,
                        context_name: title,
                        context_tracks: ctx_tracks,
                    });
                }
                glib::Propagation::Stop
            }
            _ => {
                let _ = gdk::Key::Escape;
                glib::Propagation::Proceed
            }
        }
    });

    window.add_controller(ctrl);
}

fn move_selection(bridge: &UiBridge, delta: isize) {
    {
        let mut s = bridge.store.borrow_mut();
        let vis = s.visible_tracks();
        if vis.is_empty() {
            return;
        }
        let cur_pos = s
            .selected_track_index
            .and_then(|sel| vis.iter().position(|(idx, _)| *idx == sel))
            .unwrap_or(0);
        let next_pos = (cur_pos as isize + delta).clamp(0, vis.len() as isize - 1) as usize;
        s.selected_track_index = Some(vis[next_pos].0);
    }
    (bridge.redraw)(DirtyFlags::CONTENT);
}
