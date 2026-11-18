//! Left Live Stage deck, Always-Visible 2-tier Queue panel, and narrow-mode bottom strip.

use iced::alignment::Vertical;
use iced::mouse;
use iced::widget::text::Wrapping;
use iced::widget::{
    Space, button, column, container, mouse_area, progress_bar, row, scrollable, slider, stack,
    text,
};
use iced::{Alignment, Color, Element, Length};
use pulse_assets::icons;
use pulse_model::{AppMode, CrateRef, RepeatMode, TrackSummary, UiCommand};

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::{ActivePane, UiState, id_queue_scroll};
use crate::widgets;

const QUEUE_HEADER_HEIGHT: f32 = 26.0;

/// Left Live Stage + Live Queue panel (`state.stage_width` logical px in Workbench mode).
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let deck = view_now_playing_deck(state);
    let queue = view_queue_panel(state, theme::TRACK_ROW_HEIGHT);

    container(
        column![deck, widgets::hairline_h(), queue]
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(state.stage_width))
    .height(Length::Fill)
    .style(theme::surface_card)
    .into()
}

fn view_now_playing_deck<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let player = &state.view.player_state;

    let Some(detail) = &player.active_track else {
        let placeholder = button(widgets::cover_hero(
            None,
            220.0,
            theme::RADIUS_XL,
            Some(icons::MUSIC_NOTE),
        ))
        .padding(0)
        .style(theme::plain_interactive_button);

        return container(
            column![
                container(placeholder).center_x(Length::Fill),
                column![
                    text("Ready to Play")
                        .size(16)
                        .font(theme::FONT_SANS_SEMIBOLD)
                        .color(theme::TEXT_PRIMARY),
                    text("Select a track in your Crate or resume recent playback.")
                        .size(12)
                        .font(theme::FONT_SANS)
                        .color(theme::TEXT_SECONDARY),
                ]
                .spacing(4)
                .align_x(Alignment::Center),
                row![
                    text("00:00")
                        .size(11)
                        .font(theme::FONT_MONO_MEDIUM)
                        .color(theme::TEXT_MUTED),
                    Space::new().width(Length::Fill),
                    widgets::status_badge(&player.audio_badge, theme::TEXT_MUTED),
                    Space::new().width(Length::Fill),
                    text("00:00")
                        .size(11)
                        .font(theme::FONT_MONO_MEDIUM)
                        .color(theme::TEXT_MUTED),
                ]
                .align_y(Vertical::Center),
                progress_bar(0.0..=1.0, 0.0)
                    .length(Length::Fill)
                    .girth(Length::Fixed(4.0))
                    .style(theme::rss_progress_bar(0.0)),
                container(
                    row![
                        button(
                            row![
                                widgets::icon_svg(icons::PLAY, 13.0, theme::ACCENT_TEXT_ON),
                                text("Play Liked Songs")
                                    .size(12)
                                    .font(theme::FONT_SANS_SEMIBOLD),
                            ]
                            .spacing(6)
                            .align_y(Vertical::Center),
                        )
                        .padding([7.0, 12.0])
                        .style(theme::primary_button)
                        .on_press(UiMsg::Command(UiCommand::PlayCrate {
                            crate_ref: CrateRef::LikedSongs,
                            shuffle: false,
                        })),
                        button(
                            row![
                                widgets::icon_svg(icons::PLAY, 13.0, theme::TEXT_PRIMARY),
                                text("Resume Last Track")
                                    .size(12)
                                    .font(theme::FONT_SANS_MEDIUM),
                            ]
                            .spacing(6)
                            .align_y(Vertical::Center),
                        )
                        .padding([7.0, 12.0])
                        .style(theme::secondary_button)
                        .on_press(UiMsg::Command(UiCommand::PlayCrate {
                            crate_ref: CrateRef::Recent,
                            shuffle: false,
                        })),
                    ]
                    .spacing(8)
                    .wrap(),
                )
                .center_x(Length::Fill),
            ]
            .spacing(theme::SPACE_MD),
        )
        .padding(theme::SPACE_LG)
        .width(Length::Fill)
        .into();
    };

    let track = &detail.summary;
    let cover_handle = track
        .cover
        .as_ref()
        .and_then(|c| state.covers.best_for(&c.id));

    let sleeve_btn = button(widgets::cover_hero(
        cover_handle,
        220.0,
        theme::RADIUS_XL,
        Some(icons::MUSIC_NOTE),
    ))
    .padding(0)
    .style(theme::plain_interactive_button)
    .on_press(UiMsg::SwitchMode(AppMode::FocusStage));

    let sleeve_with_glow: Element<'a, UiMsg> = match state.covers.radial_tint() {
        Some(tint) => stack![
            container(
                iced::widget::image(tint)
                    .width(Length::Fixed(248.0))
                    .height(Length::Fixed(248.0))
            )
            .center_x(Length::Fill)
            .center_y(Length::Fixed(236.0)),
            container(sleeve_btn)
                .center_x(Length::Fill)
                .center_y(Length::Fixed(236.0)),
        ]
        .into(),
        None => container(sleeve_btn)
            .center_x(Length::Fill)
            .height(Length::Fixed(228.0))
            .center_y(Length::Fixed(228.0))
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
    let like_btn = button(widgets::icon_svg(heart_icon, 18.0, heart_color))
        .padding([4.0, 6.0])
        .style(theme::ghost_button)
        .on_press(UiMsg::Command(UiCommand::ToggleLike(track.uri.to_string())));

    let title_row = row![
        container(
            text(&track.title)
                .size(16)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::WordOrGlyph),
        )
        .width(Length::Fill)
        .clip(true),
        like_btn,
    ]
    .spacing(theme::SPACE_SM)
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

    let mut meta_row = row![
        button(
            text(track.artist.as_ref())
                .size(13)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY)
                .wrapping(Wrapping::None),
        )
        .padding([2.0, 4.0])
        .style(theme::ghost_button)
        .on_press(UiMsg::Command(UiCommand::OpenCrate(artist_target))),
    ]
    .spacing(4)
    .align_y(Vertical::Center);

    if let Some(album_uri) = &track.album_uri {
        meta_row = meta_row
            .push(text("·").size(13).color(theme::TEXT_MUTED))
            .push(
                button(
                    text(track.album.as_ref())
                        .size(13)
                        .font(theme::FONT_SANS)
                        .color(theme::TEXT_SECONDARY)
                        .wrapping(Wrapping::None),
                )
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Album {
                    uri: album_uri.clone(),
                    title: track.album.to_string(),
                }))),
            );
    }

    let duration_ms = player.duration_ms.max(track.duration_ms).max(1);
    let pos_ms = state.scrubbing_pos_ms.unwrap_or(player.position_ms).min(duration_ms);
    let scrubber = slider(0.0..=(duration_ms as f32), pos_ms as f32, |v| {
        UiMsg::SeekPreview(v as u32)
    })
    .on_release(UiMsg::SeekCommit)
    .step(1000.0_f32)
    .width(Length::Fill)
    .style(theme::amber_slider);

    let stream_info = format!(
        "{} · {:.1}k",
        player.audio_badge,
        player.sample_rate_hz as f32 / 1000.0
    );
    let timing_row = row![
        text(widgets::format_duration_ms(pos_ms))
            .size(11)
            .font(theme::FONT_MONO_MEDIUM)
            .color(theme::TEXT_SECONDARY),
        Space::new().width(Length::Fill),
        widgets::status_badge(stream_info, theme::TEXT_SECONDARY),
        Space::new().width(Length::Fill),
        text(widgets::format_duration_ms(duration_ms))
            .size(11)
            .font(theme::FONT_MONO_MEDIUM)
            .color(theme::TEXT_SECONDARY),
    ]
    .align_y(Vertical::Center);

    let transport = transport_controls(player.is_playing, player.shuffle, player.repeat, 40.0);

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
    let vol_value = if player.muted {
        0.0
    } else {
        player.volume as f32
    };
    let vol_pct = if player.muted {
        0
    } else {
        player.volume_pct()
    };

    let volume_row = row![
        button(widgets::icon_svg(vol_icon, 16.0, vol_color))
            .padding([4.0, 6.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::ToggleMute)),
        slider(0.0..=65_535.0_f32, vol_value, |v| {
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
    .align_y(Vertical::Center);

    container(
        column![
            sleeve_with_glow,
            title_row,
            container(meta_row).width(Length::Fill).clip(true),
            scrubber,
            timing_row,
            container(transport).center_x(Length::Fill),
            volume_row,
        ]
        .spacing(theme::SPACE_SM),
    )
    .padding(theme::SPACE_LG)
    .width(Length::Fill)
    .into()
}

/// Shared transport deck used by both Live Stage (`40px`) and Focus Stage (`48px`).
pub(crate) fn transport_controls<'a>(
    is_playing: bool,
    shuffle: bool,
    repeat: RepeatMode,
    play_btn_size: f32,
) -> Element<'a, UiMsg> {
    let shuffle_color = if shuffle {
        theme::ACCENT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let repeat_icon = match repeat {
        RepeatMode::Track => icons::REPEAT_ONE,
        RepeatMode::Off | RepeatMode::Context => icons::REPEAT,
    };
    let repeat_color = if repeat == RepeatMode::Off {
        theme::TEXT_SECONDARY
    } else {
        theme::ACCENT_PRIMARY
    };
    let play_icon = if is_playing {
        icons::PAUSE
    } else {
        icons::PLAY
    };

    row![
        button(widgets::icon_svg(icons::SHUFFLE, 16.0, shuffle_color))
            .padding(8)
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::ToggleShuffle)),
        button(widgets::icon_svg(icons::PREV, 18.0, theme::TEXT_PRIMARY))
            .padding(8)
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::PrevTrack)),
        button(
            container(widgets::icon_svg(
                play_icon,
                (play_btn_size * 0.46).round(),
                theme::ACCENT_TEXT_ON,
            ))
            .center_x(Length::Fixed(play_btn_size))
            .center_y(Length::Fixed(play_btn_size)),
        )
        .padding(0)
        .style(theme::round_play_button)
        .on_press(UiMsg::Command(UiCommand::TogglePlayPause)),
        button(widgets::icon_svg(icons::NEXT, 18.0, theme::TEXT_PRIMARY))
            .padding(8)
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::NextTrack)),
        button(widgets::icon_svg(repeat_icon, 16.0, repeat_color))
            .padding(8)
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::CycleRepeat)),
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Vertical::Center)
    .into()
}

/// Reorderable 2-tier Live Queue (`Queued Manually` + `Next From <Context>`) virtualized by height.
pub fn view_queue_panel<'a>(state: &'a UiState, row_height: f32) -> Element<'a, UiMsg> {
    let queue = &state.view.queue;
    let total_tracks = queue.manual_items.len() + queue.context_items.len();
    let total_ms: u64 = queue
        .manual_items
        .iter()
        .chain(queue.context_items.iter())
        .map(|t| u64::from(t.duration_ms))
        .sum();
    let total_mins = total_ms / 60_000;

    let mut header_row = row![
        widgets::section_label("UP NEXT"),
        text(format!("· {total_tracks} tracks · {total_mins}m"))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        Space::new().width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Vertical::Center);

    if let Some(active_dev) = state
        .view
        .devices
        .available_devices
        .iter()
        .find(|d| d.is_active && !d.is_local)
    {
        header_row = header_row.push(widgets::status_badge(
            format!("ON {}", active_dev.name),
            theme::STATUS_WARN,
        ));
    }

    if !queue.manual_items.is_empty() {
        header_row = header_row.push(
            button(
                text("Clear")
                    .size(11)
                    .font(theme::FONT_SANS_MEDIUM)
                    .color(theme::TEXT_SECONDARY),
            )
            .padding([3.0, 8.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::RequestClearManualQueue),
        );
    }

    let header = container(header_row)
        .padding([theme::SPACE_SM, theme::SPACE_MD])
        .width(Length::Fill);

    if total_tracks == 0 {
        return column![
            header,
            container(
                column![
                    text("Queue is empty")
                        .size(13)
                        .font(theme::FONT_SANS_MEDIUM)
                        .color(theme::TEXT_SECONDARY),
                    text("Press Q on any track to play next, or Shift+Q to append.")
                        .size(11)
                        .font(theme::FONT_SANS)
                        .color(theme::TEXT_MUTED),
                ]
                .spacing(4)
                .align_x(Alignment::Center),
            )
            .padding(theme::SPACE_LG)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    }

    enum EntryKind<'b> {
        ManualHeader,
        ManualTrack {
            manual_idx: usize,
            queue_idx: usize,
            track: &'b TrackSummary,
        },
        ContextHeader,
        ContextTrack {
            context_idx: usize,
            queue_idx: usize,
            track: &'b TrackSummary,
        },
    }

    struct PlacedEntry<'b> {
        y: f32,
        h: f32,
        kind: EntryKind<'b>,
    }

    let mut entries = Vec::with_capacity(total_tracks + 2);
    let mut cursor_y = 0.0_f32;

    if !queue.manual_items.is_empty() {
        entries.push(PlacedEntry {
            y: cursor_y,
            h: QUEUE_HEADER_HEIGHT,
            kind: EntryKind::ManualHeader,
        });
        cursor_y += QUEUE_HEADER_HEIGHT;

        for (m_idx, track) in queue.manual_items.iter().enumerate() {
            entries.push(PlacedEntry {
                y: cursor_y,
                h: row_height,
                kind: EntryKind::ManualTrack {
                    manual_idx: m_idx,
                    queue_idx: m_idx,
                    track,
                },
            });
            cursor_y += row_height;
        }
    }

    if !queue.context_items.is_empty() {
        entries.push(PlacedEntry {
            y: cursor_y,
            h: QUEUE_HEADER_HEIGHT,
            kind: EntryKind::ContextHeader,
        });
        cursor_y += QUEUE_HEADER_HEIGHT;

        let manual_len = queue.manual_items.len();
        for (c_idx, track) in queue.context_items.iter().enumerate() {
            entries.push(PlacedEntry {
                y: cursor_y,
                h: row_height,
                kind: EntryKind::ContextTrack {
                    context_idx: c_idx,
                    queue_idx: manual_len + c_idx,
                    track,
                },
            });
            cursor_y += row_height;
        }
    }

    let overscan_px = row_height * 4.0;
    let vis_top = (state.queue_scroll_y - overscan_px).max(0.0);
    let vis_bot = state.queue_scroll_y + state.queue_viewport_height.max(300.0) + overscan_px;

    let first_idx = entries
        .iter()
        .position(|e| e.y + e.h >= vis_top)
        .unwrap_or(entries.len());
    let last_idx = entries
        .iter()
        .rposition(|e| e.y <= vis_bot)
        .map_or(first_idx, |i| i + 1);

    let top_spacer = entries.get(first_idx).map_or(0.0, |e| e.y);
    let rendered_end_y = if last_idx > first_idx {
        entries[last_idx - 1].y + entries[last_idx - 1].h
    } else {
        top_spacer
    };
    let bottom_spacer = (cursor_y - rendered_end_y).max(0.0);

    let mut list_col = column![].width(Length::Fill).padding([0.0, theme::SPACE_SM]);
    if top_spacer > 0.5 {
        list_col = list_col.push(Space::new().height(top_spacer));
    }

    let manual_len = queue.manual_items.len();
    let is_queue_focused = state.active_pane == ActivePane::Queue;

    for entry in &entries[first_idx..last_idx] {
        let elem: Element<'a, UiMsg> = match &entry.kind {
            EntryKind::ManualHeader => container(
                row![
                    widgets::section_label("QUEUED MANUALLY"),
                    text(format!("({})", queue.manual_items.len()))
                        .size(11)
                        .font(theme::FONT_MONO)
                        .color(theme::TEXT_MUTED),
                ]
                .spacing(6)
                .align_y(Vertical::Center),
            )
            .height(Length::Fixed(QUEUE_HEADER_HEIGHT))
            .padding([4.0, 6.0])
            .center_y(Length::Fixed(QUEUE_HEADER_HEIGHT))
            .into(),
            EntryKind::ContextHeader => {
                let ctx_label = if queue.context_name.is_empty() {
                    String::from("NEXT FROM CONTEXT")
                } else {
                    format!("NEXT FROM {}", queue.context_name.to_uppercase())
                };
                container(
                    text(ctx_label)
                        .size(11)
                        .font(theme::FONT_MONO_SEMIBOLD)
                        .color(theme::TEXT_MUTED)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .height(Length::Fixed(QUEUE_HEADER_HEIGHT))
                .padding([4.0, 6.0])
                .center_y(Length::Fixed(QUEUE_HEADER_HEIGHT))
                .clip(true)
                .into()
            }
            EntryKind::ManualTrack {
                manual_idx,
                queue_idx,
                track,
            } => queue_manual_row(
                state,
                *manual_idx,
                *queue_idx,
                manual_len,
                track,
                row_height,
                is_queue_focused,
            ),
            EntryKind::ContextTrack {
                context_idx,
                queue_idx,
                track,
            } => queue_context_row(
                state,
                *context_idx,
                *queue_idx,
                manual_len,
                track,
                row_height,
                is_queue_focused,
            ),
        };
        list_col = list_col.push(elem);
    }

    if bottom_spacer > 0.5 {
        list_col = list_col.push(Space::new().height(bottom_spacer));
    }

    let scroller = scrollable(list_col)
        .id(id_queue_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::QueueScrolled)
        .style(theme::dark_scrollable);

    column![header, scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn queue_manual_row<'a>(
    state: &'a UiState,
    manual_idx: usize,
    queue_idx: usize,
    manual_len: usize,
    track: &'a TrackSummary,
    row_height: f32,
    is_queue_focused: bool,
) -> Element<'a, UiMsg> {
    let is_selected = is_queue_focused && state.selected_queue_row == Some(queue_idx);
    let is_dragging_target = state.dragging_queue_from == Some(manual_idx);
    let cover = track
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));

    let drag_grip = mouse_area(
        container(widgets::icon_svg(
            icons::DRAG_HANDLE,
            13.0,
            theme::TEXT_MUTED,
        ))
        .padding([4.0, 2.0]),
    )
    .on_press(UiMsg::DragQueueStart(manual_idx))
    .interaction(mouse::Interaction::Grab);

    let mut actions = row![];
    if manual_idx > 0 {
        actions = actions.push(
            button(text("↑").size(11).color(theme::TEXT_SECONDARY))
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::QueueMoveUp(manual_idx)),
        );
    }
    if manual_idx + 1 < manual_len {
        actions = actions.push(
            button(text("↓").size(11).color(theme::TEXT_SECONDARY))
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::QueueMoveDown(manual_idx)),
        );
    }
    actions = actions.push(
        button(widgets::icon_svg(icons::CLOSE, 12.0, theme::TEXT_MUTED))
            .padding([3.0, 4.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::RemoveManualQueueItem(manual_idx))),
    );

    let inner = row![
        container(text("").size(1))
            .width(Length::Fixed(2.0))
            .height(Length::Fixed(row_height - 10.0))
            .style(theme::accent_strip(theme::ACCENT_PRIMARY)),
        drag_grip,
        widgets::cover_thumb(cover, 32.0, Some(icons::MUSIC_NOTE)),
        column![
            container(
                text(&track.title)
                    .size(13)
                    .font(theme::FONT_SANS_MEDIUM)
                    .color(theme::TEXT_PRIMARY)
                    .wrapping(Wrapping::None),
            )
            .width(Length::Fill)
            .clip(true),
            container(
                text(track.artist.as_ref())
                    .size(11)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_SECONDARY)
                    .wrapping(Wrapping::None),
            )
            .width(Length::Fill)
            .clip(true),
        ]
        .width(Length::Fill)
        .spacing(1),
        text(widgets::format_duration_ms(track.duration_ms))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        actions.spacing(1).align_y(Vertical::Center),
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .padding([2.0, 4.0]);

    let row_box = container(inner)
        .width(Length::Fill)
        .height(Length::Fixed(row_height))
        .center_y(Length::Fixed(row_height))
        .style(theme::row_container(false, is_selected, is_dragging_target));

    let mut area = mouse_area(row_box)
        .on_press(UiMsg::SelectQueueRow(queue_idx))
        .on_double_click(UiMsg::Command(UiCommand::PlayQueueIndex(queue_idx)))
        .on_right_press(UiMsg::OpenTrackContextMenu {
            index: queue_idx,
            track: track.clone(),
            pos: state.cursor_pos,
        });
    if state.dragging_queue_from.is_some() {
        area = area.on_release(UiMsg::DragQueueDrop(manual_idx));
    }
    area.into()
}

fn queue_context_row<'a>(
    state: &'a UiState,
    context_idx: usize,
    queue_idx: usize,
    manual_len: usize,
    track: &'a TrackSummary,
    row_height: f32,
    is_queue_focused: bool,
) -> Element<'a, UiMsg> {
    let is_selected = is_queue_focused && state.selected_queue_row == Some(queue_idx);
    let cover = track
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));

    let promote_btn = button(
        text("+Q")
            .size(10)
            .font(theme::FONT_MONO_SEMIBOLD)
            .color(theme::TEXT_SECONDARY),
    )
    .padding([2.0, 5.0])
    .style(theme::ghost_button)
    .on_press(UiMsg::Command(UiCommand::PromoteContextToManual {
        context_index: context_idx,
        manual_index: manual_len,
    }));

    let inner = row![
        container(
            text(format!("{:02}", context_idx + 1))
                .size(11)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_MUTED),
        )
        .width(Length::Fixed(22.0))
        .align_x(Alignment::Center),
        widgets::cover_thumb(cover, 32.0, Some(icons::MUSIC_NOTE)),
        column![
            container(
                text(&track.title)
                    .size(13)
                    .font(theme::FONT_SANS_MEDIUM)
                    .color(theme::TEXT_PRIMARY)
                    .wrapping(Wrapping::None),
            )
            .width(Length::Fill)
            .clip(true),
            container(
                text(track.artist.as_ref())
                    .size(11)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_SECONDARY)
                    .wrapping(Wrapping::None),
            )
            .width(Length::Fill)
            .clip(true),
        ]
        .width(Length::Fill)
        .spacing(1),
        text(widgets::format_duration_ms(track.duration_ms))
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED),
        promote_btn,
    ]
    .spacing(6)
    .align_y(Vertical::Center)
    .padding([2.0, 4.0]);

    let row_box = container(inner)
        .width(Length::Fill)
        .height(Length::Fixed(row_height))
        .center_y(Length::Fixed(row_height))
        .style(theme::row_container(false, is_selected, false));

    let mut area = mouse_area(row_box)
        .on_press(UiMsg::SelectQueueRow(queue_idx))
        .on_double_click(UiMsg::Command(UiCommand::PlayQueueIndex(queue_idx)))
        .on_right_press(UiMsg::OpenTrackContextMenu {
            index: queue_idx,
            track: track.clone(),
            pos: state.cursor_pos,
        });
    if state.dragging_queue_from.is_some() {
        area = area.on_release(UiMsg::DragQueueDrop(manual_len));
    }
    area.into()
}

/// Compact `64px` bottom Stage Strip shown automatically in Workbench when window width `< 860px`.
pub fn view_narrow_strip<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let player = &state.view.player_state;
    let active = player.active_track.as_ref();

    let cover = active
        .and_then(|d| d.summary.cover.as_ref())
        .and_then(|c| state.covers.best_for(&c.id));

    let title = active.map_or("Ready to Play", |d| d.summary.title.as_str());
    let artist = active.map_or("Select a track", |d| d.summary.artist.as_ref());

    let duration_ms = player
        .active_track
        .as_ref()
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

    let cover_btn = button(widgets::cover_thumb(cover, 44.0, Some(icons::MUSIC_NOTE)))
        .padding(0)
        .style(theme::plain_interactive_button)
        .on_press(UiMsg::SwitchMode(AppMode::FocusStage));

    let queue_label = format!("Queue ({})", state.view.queue.manual_items.len());
    let queue_btn = button(
        row![
            widgets::icon_svg(icons::QUEUE, 14.0, theme::TEXT_PRIMARY),
            text(queue_label)
                .size(12)
                .font(theme::FONT_SANS_MEDIUM),
        ]
        .spacing(5)
        .align_y(Vertical::Center),
    )
    .padding([6.0, 10.0])
    .style(theme::pill_button(state.show_narrow_queue))
    .on_press(UiMsg::ToggleNarrowQueue);

    let play_btn = button(
        container(widgets::icon_svg(
            play_icon,
            15.0,
            theme::ACCENT_TEXT_ON,
        ))
        .center_x(Length::Fixed(32.0))
        .center_y(Length::Fixed(32.0)),
    )
    .padding(0)
    .style(theme::round_play_button)
    .on_press(UiMsg::Command(UiCommand::TogglePlayPause));

    container(
        row![
            cover_btn,
            column![
                container(
                    text(title)
                        .size(13)
                        .font(theme::FONT_SANS_SEMIBOLD)
                        .color(theme::TEXT_PRIMARY)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
                container(
                    text(artist)
                        .size(11)
                        .font(theme::FONT_SANS)
                        .color(theme::TEXT_SECONDARY)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
            ]
            .width(Length::FillPortion(2))
            .spacing(2),
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
                        .size(10)
                        .font(theme::FONT_MONO)
                        .color(theme::TEXT_MUTED),
                    Space::new().width(Length::Fill),
                    text(widgets::format_duration_ms(duration_ms))
                        .size(10)
                        .font(theme::FONT_MONO)
                        .color(theme::TEXT_MUTED),
                ],
            ]
            .width(Length::FillPortion(2))
            .spacing(2),
            row![
                button(widgets::icon_svg(icons::PREV, 15.0, theme::TEXT_PRIMARY))
                    .padding(6)
                    .style(theme::ghost_button)
                    .on_press(UiMsg::Command(UiCommand::PrevTrack)),
                play_btn,
                button(widgets::icon_svg(icons::NEXT, 15.0, theme::TEXT_PRIMARY))
                    .padding(6)
                    .style(theme::ghost_button)
                    .on_press(UiMsg::Command(UiCommand::NextTrack)),
            ]
            .spacing(4)
            .align_y(Vertical::Center),
            queue_btn,
        ]
        .spacing(theme::SPACE_MD)
        .align_y(Vertical::Center),
    )
    .padding([6.0, theme::SPACE_MD])
    .width(Length::Fill)
    .height(Length::Fixed(64.0))
    .center_y(Length::Fixed(64.0))
    .style(theme::top_bar_container)
    .into()
}

pub(crate) fn accent_color_from_rgba(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], f32::from(rgba[3]) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use pulse_model::{QueueState, TrackDetail};

    #[test]
    fn renders_idle_and_active_stage_queue_views() {
        let mut ui = UiState::default();
        let _ = view(&ui);
        let _ = view_narrow_strip(&ui);
        let _ = crate::focus_stage::view(&ui);
        let _ = crate::mini_player::view(&ui);

        let track = TrackSummary {
            uri: Arc::from("spotify:track:1"),
            title: String::from("Midnight City"),
            artist: Arc::from("M83"),
            artist_uri: Some(Arc::from("spotify:artist:1")),
            album: Arc::from("Hurry Up, We're Dreaming"),
            album_uri: Some(Arc::from("spotify:album:1")),
            cover: None,
            duration_ms: 243_000,
            is_liked: true,
            is_playable: true,
            added_at_unix: None,
        };
        ui.view.player_state.active_track = Some(TrackDetail {
            summary: track.clone(),
            track_number: Some(2),
            disc_total_tracks: Some(11),
            release_year: Some(2011),
            explicit: false,
        });
        ui.view.queue = QueueState {
            manual_items: vec![track.clone()],
            context_items: vec![track],
            context_name: String::from("Late Night"),
            context_uri: None,
        };

        ui.view.config.confirm_clear_queue = true;
        let _ = view(&ui);
        let _ = view_queue_panel(&ui, theme::TRACK_ROW_HEIGHT);
        ui.view.config.confirm_clear_queue = false;
        let _ = view_queue_panel(&ui, theme::TRACK_ROW_HEIGHT);
        let _ = view_narrow_strip(&ui);
        let _ = crate::focus_stage::view(&ui);
        let _ = crate::mini_player::view(&ui);
    }
}
