//! Ultra-compact Mini-Player window mode (`AppMode::MiniPlayer`, `420x112` logical px).

use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, mouse_area, row, slider, text};
use iced::{Element, Length};
use pulse_assets::icons;
use pulse_model::{AppMode, UiCommand};

use crate::message::UiMsg;
use crate::stage::accent_color_from_rgba;
use crate::theme;
use crate::ui_state::UiState;
use crate::widgets;

/// Renders the `420x112` compact Mini-Player card.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let player = &state.view.player_state;
    let active = player.active_track.as_ref();
    let accent_color = if active.is_some() {
        accent_color_from_rgba(player.cover_accent_rgba)
    } else {
        theme::ACCENT_PRIMARY
    };

    let cover_handle = active
        .and_then(|d| d.summary.cover.as_ref())
        .and_then(|c| state.covers.best_for(&c.id));

    let cover_btn = button(widgets::cover_hero(
        cover_handle,
        84.0,
        theme::RADIUS_LG,
        Some(icons::MUSIC_NOTE),
    ))
    .padding(0)
    .style(theme::plain_interactive_button)
    .on_press(UiMsg::SwitchMode(AppMode::FocusStage));

    let title_str = active.map_or("Ready to Play", |d| d.summary.title.as_str());
    let artist_str = active.map_or("No active track", |d| d.summary.artist.as_ref());

    let pin_color = if state.always_on_top {
        theme::ACCENT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };

    let mut header_row = row![
        container(
            text(title_str)
                .size(14)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
    ]
    .spacing(4)
    .align_y(Vertical::Center);

    if let Some(detail) = active {
        let heart_icon = if detail.summary.is_liked {
            icons::HEART_FILLED
        } else {
            icons::HEART_OUTLINE
        };
        let heart_color = if detail.summary.is_liked {
            theme::ACCENT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        header_row = header_row.push(
            button(widgets::icon_svg(heart_icon, 14.0, heart_color))
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::ToggleLike(
                    detail.summary.uri.to_string(),
                ))),
        );
    }

    let restore_mode = match state.mode_before_mini {
        AppMode::MiniPlayer => AppMode::Workbench,
        other => other,
    };

    header_row = header_row
        .push(
            button(widgets::icon_svg(icons::PIN, 13.0, pin_color))
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::ToggleAlwaysOnTop),
        )
        .push(
            button(widgets::icon_svg(
                icons::EXPAND,
                13.0,
                theme::TEXT_SECONDARY,
            ))
            .padding([2.0, 4.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::SwitchMode(restore_mode)),
        );

    let device_suffix = state
        .view
        .devices
        .available_devices
        .iter()
        .find(|d| d.is_active && !d.is_local)
        .map_or_else(
            || player.audio_badge.clone(),
            |d| format!("Connect · {}", d.name),
        );

    let meta_row = row![
        container(
            text(artist_str)
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        text(device_suffix)
            .size(10)
            .font(theme::FONT_MONO_MEDIUM)
            .color(theme::TEXT_MUTED),
    ]
    .spacing(6)
    .align_y(Vertical::Center);

    let duration_ms = active
        .map_or(player.duration_ms, |d| {
            player.duration_ms.max(d.summary.duration_ms)
        })
        .max(1);
    let pos_ms = state.scrubbing_pos_ms.unwrap_or(player.position_ms).min(duration_ms);
    let play_icon = if player.is_playing {
        icons::PAUSE
    } else {
        icons::PLAY
    };

    let play_btn = button(
        container(widgets::icon_svg(play_icon, 13.0, theme::ACCENT_TEXT_ON))
            .center_x(Length::Fixed(26.0))
            .center_y(Length::Fixed(26.0)),
    )
    .padding(0)
    .style(theme::round_play_button)
    .on_press(UiMsg::Command(UiCommand::TogglePlayPause));

    let scrub_and_transport = row![
        text(widgets::format_duration_ms(pos_ms))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_SECONDARY),
        slider(0.0..=(duration_ms as f32), pos_ms as f32, |v| {
            UiMsg::SeekPreview(v as u32)
        })
        .on_release(UiMsg::SeekCommit)
        .step(1000.0_f32)
        .width(Length::Fill)
        .style(theme::amber_slider),
        text(widgets::format_remaining_ms(pos_ms, duration_ms))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        button(widgets::icon_svg(icons::PREV, 13.0, theme::TEXT_PRIMARY))
            .padding([3.0, 4.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::PrevTrack)),
        play_btn,
        button(widgets::icon_svg(icons::NEXT, 13.0, theme::TEXT_PRIMARY))
            .padding([3.0, 4.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::NextTrack)),
    ]
    .spacing(6)
    .align_y(Vertical::Center);

    let next_track = state
        .view
        .queue
        .manual_items
        .first()
        .or_else(|| state.view.queue.context_items.first());
    let next_line_text = match next_track {
        Some(next) if player.is_playing || state.hovered_track_row.is_some() => {
            format!("Next: {} — {}", next.title, next.artist)
        }
        Some(next) => format!("Up next: {}", next.title),
        None => String::from("End of queue · Esc or 1 to expand"),
    };

    let next_row = container(
        text(next_line_text)
            .size(11)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_MUTED)
            .wrapping(Wrapping::None),
    )
    .width(Length::Fill)
    .clip(true);

    let right_col = column![
        header_row,
        meta_row,
        scrub_and_transport,
        Space::new().height(1),
        next_row,
    ]
    .width(Length::Fill)
    .spacing(2);

    let inner = row![
        container(text("").size(1))
            .width(Length::Fixed(3.0))
            .height(Length::Fill)
            .style(theme::accent_strip(accent_color)),
        container(
            row![cover_btn, right_col]
                .spacing(10)
                .align_y(Vertical::Center)
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .padding([8.0, 10.0])
        .width(Length::Fill)
        .height(Length::Fill),
    ]
    .width(Length::Fill)
    .height(Length::Fill);

    let card = container(inner)
        .width(Length::Fill)
        .height(Length::Fixed(112.0))
        .style(theme::mini_player_frame);

    mouse_area(card)
        .on_enter(UiMsg::HoverTrackRow(Some(0)))
        .on_exit(UiMsg::HoverTrackRow(None))
        .into()
}
