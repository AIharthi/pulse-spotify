//! Immersive Focus / Stage view (`AppMode::FocusStage`, key `2` or `F`).

use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row, slider, stack, text};
use iced::{Alignment, Element, Length};
use pulse_assets::icons;
use pulse_model::{AppMode, CrateRef, UiCommand};

use crate::message::UiMsg;
use crate::stage::transport_controls;
use crate::theme;
use crate::ui_state::UiState;
use crate::widgets;

/// Focus Stage listening hero + optional right-hand Queue panel (`Q` to toggle).
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let show_queue = state.show_focus_queue && state.window_size.width >= 900.0;
    let hero = container(view_focus_hero(state, show_queue))
        .width(if show_queue {
            Length::FillPortion(58)
        } else {
            Length::Fill
        })
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    if !show_queue {
        return container(hero)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::canvas_container)
            .into();
    }

    let queue_header = container(
        row![
            widgets::section_label("FOCUS QUEUE"),
            Space::new().width(Length::Fill),
            button(
                row![
                    widgets::keycap("Q"),
                    text("Hide")
                        .size(11)
                        .font(theme::FONT_SANS_MEDIUM)
                        .color(theme::TEXT_SECONDARY),
                ]
                .spacing(5)
                .align_y(Vertical::Center),
            )
            .padding([4.0, 8.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::ToggleQueueInFocus),
        ]
        .align_y(Vertical::Center),
    )
    .padding([theme::SPACE_SM, theme::SPACE_MD])
    .width(Length::Fill);

    let queue_pane = container(
        column![
            queue_header,
            widgets::hairline_h(),
            crate::stage::view_queue_panel(state, theme::FOCUS_QUEUE_ROW_HEIGHT),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::FillPortion(42))
    .height(Length::Fill)
    .style(theme::surface_card);

    container(
        row![hero, widgets::hairline_v(), queue_pane]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::canvas_container)
    .into()
}

fn view_focus_hero<'a>(state: &'a UiState, queue_visible: bool) -> Element<'a, UiMsg> {
    let player = &state.view.player_state;
    let pane_w = if queue_visible {
        state.window_size.width * 0.58
    } else {
        state.window_size.width
    };
    let cover_size = (state.window_size.height - 380.0)
        .min(pane_w - 96.0)
        .clamp(120.0, 420.0);

    let toggle_queue_btn: Option<Element<'a, UiMsg>> = if !queue_visible
        && state.window_size.width >= 900.0
    {
        Some(
            button(
                row![
                    widgets::icon_svg(icons::QUEUE, 14.0, theme::TEXT_SECONDARY),
                    text("Show Queue (Q)")
                        .size(12)
                        .font(theme::FONT_SANS_MEDIUM)
                        .color(theme::TEXT_SECONDARY),
                ]
                .spacing(6)
                .align_y(Vertical::Center),
            )
            .padding([5.0, 10.0])
            .style(theme::secondary_button)
            .on_press(UiMsg::ToggleQueueInFocus)
            .into(),
        )
    } else {
        None
    };

    let Some(detail) = &player.active_track else {
        let mut empty_col = column![
            widgets::cover_hero(
                None,
                cover_size.min(340.0),
                theme::RADIUS_MODAL,
                Some(icons::MUSIC_NOTE),
            ),
            text("Nothing Playing")
                .size(24)
                .font(theme::FONT_SANS_BOLD)
                .color(theme::TEXT_PRIMARY),
            text("Choose music in Workbench or resume your Liked Songs.")
                .size(14)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
            row![
                button(
                    row![
                        widgets::icon_svg(icons::PLAY, 14.0, theme::ACCENT_TEXT_ON),
                        text("Play Liked Songs")
                            .size(13)
                            .font(theme::FONT_SANS_SEMIBOLD),
                    ]
                    .spacing(8)
                    .align_y(Vertical::Center),
                )
                .padding([10.0, 18.0])
                .style(theme::primary_button)
                .on_press(UiMsg::Command(UiCommand::PlayCrate {
                    crate_ref: CrateRef::LikedSongs,
                    shuffle: false,
                })),
                button(
                    text("Open Workbench (1)")
                        .size(13)
                        .font(theme::FONT_SANS_MEDIUM),
                )
                .padding([10.0, 16.0])
                .style(theme::secondary_button)
                .on_press(UiMsg::SwitchMode(AppMode::Workbench)),
            ]
            .spacing(theme::SPACE_MD),
        ]
        .spacing(theme::SPACE_LG)
        .align_x(Alignment::Center);

        if let Some(btn) = toggle_queue_btn {
            empty_col = empty_col.push(btn);
        }

        return container(empty_col)
            .padding(theme::SPACE_2XL)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    };

    let track = &detail.summary;
    let cover_handle = track
        .cover
        .as_ref()
        .and_then(|c| state.covers.best_for(&c.id));

    let sleeve = widgets::cover_hero(
        cover_handle,
        cover_size,
        theme::RADIUS_MODAL,
        Some(icons::MUSIC_NOTE),
    );

    let glow_size = cover_size + 112.0;
    let sleeve_stage: Element<'a, UiMsg> = match state.covers.radial_tint() {
        Some(tint) => stack![
            container(
                iced::widget::image(tint)
                    .width(Length::Fixed(glow_size))
                    .height(Length::Fixed(glow_size))
            )
            .center_x(Length::Fixed(glow_size))
            .center_y(Length::Fixed(cover_size + 28.0)),
            container(sleeve)
                .center_x(Length::Fixed(glow_size))
                .center_y(Length::Fixed(cover_size + 28.0)),
        ]
        .into(),
        None => container(sleeve)
            .center_x(Length::Fill)
            .height(Length::Fixed(cover_size + 12.0))
            .center_y(Length::Fixed(cover_size + 12.0))
            .into(),
    };

    let heart_icon = if track.is_liked {
        icons::HEART_FILLED
    } else {
        icons::HEART_OUTLINE
    };
    let heart_color = if track.is_liked {
        theme::ACCENT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };

    let title_row = row![
        container(
            text(&track.title)
                .size(26)
                .font(theme::FONT_SANS_BOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::WordOrGlyph),
        )
        .max_width(480.0),
        button(widgets::icon_svg(heart_icon, 20.0, heart_color))
            .padding([6.0, 8.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::ToggleLike(track.uri.to_string()))),
    ]
    .spacing(theme::SPACE_MD)
    .align_y(Vertical::Center);

    let artist_target = track.artist_uri.as_ref().map_or_else(
        || CrateRef::Search {
            query: track.artist.to_string(),
        },
        |uri| CrateRef::Artist {
            uri: uri.clone(),
            name: track.artist.to_string(),
        },
    );

    let mut artist_album_row = row![
        button(
            text(track.artist.as_ref())
                .size(15)
                .font(theme::FONT_SANS_MEDIUM)
                .color(theme::TEXT_SECONDARY),
        )
        .padding([3.0, 6.0])
        .style(theme::ghost_button)
        .on_press(UiMsg::Command(UiCommand::OpenCrate(artist_target))),
    ]
    .spacing(4)
    .align_y(Vertical::Center);

    if let Some(album_uri) = &track.album_uri {
        artist_album_row = artist_album_row
            .push(text("·").size(15).color(theme::TEXT_MUTED))
            .push(
                button(
                    text(track.album.as_ref())
                        .size(15)
                        .font(theme::FONT_SANS_MEDIUM)
                        .color(theme::TEXT_SECONDARY),
                )
                .padding([3.0, 6.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Album {
                    uri: album_uri.clone(),
                    title: track.album.to_string(),
                }))),
            );
    }

    let mut strip_parts = Vec::with_capacity(4);
    if let Some(year) = detail.release_year {
        strip_parts.push(year.to_string());
    }
    if let Some(num) = detail.track_number {
        if let Some(total) = detail.disc_total_tracks {
            strip_parts.push(format!("Track {num} of {total}"));
        } else {
            strip_parts.push(format!("Track {num}"));
        }
    }
    strip_parts.push(player.audio_badge.clone());
    strip_parts.push(format!("{:.1} kHz", player.sample_rate_hz as f32 / 1000.0));
    let context_strip = strip_parts.join(" · ");

    let duration_ms = player.duration_ms.max(track.duration_ms).max(1);
    let pos_ms = state.scrubbing_pos_ms.unwrap_or(player.position_ms).min(duration_ms);

    let scrubber_block = container(
        column![
            slider(0.0..=(duration_ms as f32), pos_ms as f32, |v| {
                UiMsg::SeekPreview(v as u32)
            })
            .on_release(UiMsg::SeekCommit)
            .step(1000.0_f32)
            .width(Length::Fill)
            .style(theme::amber_slider),
            row![
                text(widgets::format_duration_ms(pos_ms))
                    .size(12)
                    .font(theme::FONT_MONO_MEDIUM)
                    .color(theme::TEXT_SECONDARY),
                Space::new().width(Length::Fill),
                text(widgets::format_remaining_ms(pos_ms, duration_ms))
                    .size(12)
                    .font(theme::FONT_MONO_MEDIUM)
                    .color(theme::TEXT_MUTED),
            ],
        ]
        .spacing(4),
    )
    .max_width(520.0)
    .width(Length::Fill);

    let vol_icon = if player.muted || player.volume == 0 {
        icons::VOLUME_MUTE
    } else {
        icons::VOLUME_HIGH
    };
    let vol_color = if player.muted {
        theme::STATUS_WARN
    } else {
        theme::TEXT_SECONDARY
    };
    let vol_val = if player.muted {
        0.0
    } else {
        player.volume as f32
    };
    let vol_pct = if player.muted {
        0
    } else {
        player.volume_pct()
    };

    let volume_block = container(
        row![
            button(widgets::icon_svg(vol_icon, 16.0, vol_color))
                .padding([4.0, 6.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::ToggleMute)),
            slider(0.0..=65_535.0_f32, vol_val, |v| {
                UiMsg::Command(UiCommand::SetVolume(v as u16))
            })
            .step(655.0_f32)
            .width(Length::Fill)
            .style(theme::amber_slider),
            container(
                text(format!("{vol_pct}%"))
                    .size(11)
                    .font(theme::FONT_MONO_MEDIUM)
                    .color(theme::TEXT_MUTED),
            )
            .width(Length::Fixed(34.0))
            .align_x(Alignment::End),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Vertical::Center),
    )
    .max_width(280.0)
    .width(Length::Fill);

    let mut hero_col = column![
        sleeve_stage,
        title_row,
        artist_album_row,
        text(context_strip)
            .size(12)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        scrubber_block,
        transport_controls(player.is_playing, player.shuffle, player.repeat, 48.0),
        volume_block,
    ]
    .spacing(theme::SPACE_SM)
    .align_x(Alignment::Center);

    if let Some(btn) = toggle_queue_btn {
        hero_col = hero_col.push(btn);
    }

    iced::widget::scrollable(
        container(hero_col)
            .padding([theme::SPACE_LG, theme::SPACE_2XL])
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::dark_scrollable)
    .into()
}
