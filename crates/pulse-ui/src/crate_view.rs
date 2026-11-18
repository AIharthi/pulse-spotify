//! Right-hand Crate Workbench browser with virtualized 44px track tables and grid views.

use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, mouse_area, row, scrollable, text};
use iced::{Alignment, Element, Length};
use pulse_assets::icons;
use pulse_model::{
    AccountTier, AlbumSummary, ArtistDetail, ArtistSummary, AuthState, ConnectionStatus, CrateRef,
    LibraryScope, PlaylistSummary, SearchFilter, SearchResults, SortOrder, TrackSummary, UiCommand,
};

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::{ActivePane, UiState, id_crate_scroll};
use crate::widgets;

const GRID_CARD_ROW_HEIGHT: f32 = 176.0;
const OVERSCAN_ROWS: usize = 6;

/// Root Crate Workbench pane (`44px` scope strip + non-blocking banners + active crate view).
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let mut root = column![view_scope_strip(state), widgets::hairline_h()]
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(banner) = view_non_blocking_notices(state) {
        root = root.push(banner);
    }

    let cv = &state.view.crate_view;
    let body: Element<'a, UiMsg> = match &cv.active_crate {
        CrateRef::PlaylistsIndex => view_playlists_grid(state),
        CrateRef::AlbumsIndex => view_albums_grid(state),
        CrateRef::ArtistsIndex => view_artists_grid(state),
        CrateRef::Artist { .. } => view_artist_detail(state, cv.artist_detail.as_ref()),
        CrateRef::Search { query } => view_search_results(state, query, cv.search_results.as_ref()),
        CrateRef::LikedSongs
        | CrateRef::Recent
        | CrateRef::Playlist { .. }
        | CrateRef::Album { .. } => view_track_crate(state),
    };

    root = root.push(body);

    container(root)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::canvas_container)
        .into()
}

fn view_scope_strip<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let scopes = [
        (LibraryScope::LikedSongs, "Liked Songs"),
        (LibraryScope::Playlists, "Playlists"),
        (LibraryScope::Albums, "Albums"),
        (LibraryScope::Artists, "Artists"),
        (LibraryScope::Recent, "Recent"),
    ];

    let mut pills = row![].spacing(theme::SPACE_XS).align_y(Vertical::Center);
    for (scope, label) in scopes {
        let active = cv.active_scope == scope;
        pills = pills.push(
            button(
                text(label)
                    .size(12)
                    .font(if active {
                        theme::FONT_SANS_SEMIBOLD
                    } else {
                        theme::FONT_SANS_MEDIUM
                    }),
            )
            .padding([5.0, 10.0])
            .style(theme::pill_button(active))
            .on_press(UiMsg::Command(UiCommand::SetScope(scope))),
        );
    }

    if cv.search_results.is_some() || cv.active_scope == LibraryScope::Search {
        let active = cv.active_scope == LibraryScope::Search;
        pills = pills.push(
            button(
                text("Search")
                    .size(12)
                    .font(theme::FONT_SANS_SEMIBOLD),
            )
            .padding([5.0, 10.0])
            .style(theme::pill_button(active))
            .on_press(UiMsg::Command(UiCommand::SetScope(LibraryScope::Search))),
        );
    }

    let mut strip = row![pills, Space::new().width(Length::Fill)]
        .spacing(theme::SPACE_SM)
        .align_y(Vertical::Center);

    if !cv.local_filter.trim().is_empty() {
        let matched = matched_filter_indices(cv).len();
        let total = cv.loaded_tracks.total().max(cv.loaded_tracks.loaded_count());
        strip = strip.push(
            row![
                widgets::status_badge(format!("{matched} / {total}"), theme::ACCENT_PRIMARY),
                button(widgets::icon_svg(icons::CLOSE, 12.0, theme::TEXT_SECONDARY))
                    .padding([4.0, 5.0])
                    .style(theme::ghost_button)
                    .on_press(UiMsg::ClearFilter),
            ]
            .spacing(4)
            .align_y(Vertical::Center),
        );
    }

    let next_sort = next_sort_order(cv.sort_order);
    let sort_btn = button(
        text(format!("Sort: {} v", sort_label(cv.sort_order)))
            .size(12)
            .font(theme::FONT_SANS_MEDIUM)
            .color(theme::TEXT_SECONDARY),
    )
    .padding([5.0, 10.0])
    .style(theme::secondary_button)
    .on_press(UiMsg::Command(UiCommand::SetSortOrder(next_sort)));

    strip = strip.push(sort_btn);

    container(strip)
        .padding([0.0, theme::SPACE_LG])
        .width(Length::Fill)
        .height(Length::Fixed(theme::SCOPE_STRIP_HEIGHT))
        .center_y(Length::Fixed(theme::SCOPE_STRIP_HEIGHT))
        .style(theme::canvas_container)
        .into()
}

fn view_non_blocking_notices<'a>(state: &'a UiState) -> Option<Element<'a, UiMsg>> {
    let mut items: Vec<Element<'a, UiMsg>> = Vec::new();

    if matches!(state.view.auth_state, AuthState::ConnectOnly { .. }) {
        items.push(
            container(
                row![
                    text(
                        "Connected via Spotify Connect. Sign in via OAuth in Settings (Ctrl+,) for full library browsing."
                    )
                    .size(12)
                    .font(theme::FONT_SANS)
                    .color(theme::STATUS_WARN),
                    Space::new().width(Length::Fill),
                    button(
                        text("Settings")
                            .size(11)
                            .font(theme::FONT_SANS_SEMIBOLD),
                    )
                    .padding([3.0, 8.0])
                    .style(theme::secondary_button)
                    .on_press(UiMsg::ToggleSettings),
                ]
                .spacing(theme::SPACE_SM)
                .align_y(Vertical::Center),
            )
            .padding([5.0, theme::SPACE_LG])
            .width(Length::Fill)
            .style(theme::banner_container(false))
            .into(),
        );
    } else if matches!(
        state.view.auth_state,
        AuthState::Authenticated {
            tier: AccountTier::Free,
            ..
        }
    ) {
        items.push(
            container(
                text(
                    "Free Spotify account detected: librespot audio playback and Spotify Connect require Spotify Premium."
                )
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::STATUS_WARN),
            )
            .padding([5.0, theme::SPACE_LG])
            .width(Length::Fill)
            .style(theme::banner_container(false))
            .into(),
        );
    }

    if let ConnectionStatus::Offline {
        retry_in_secs,
        cached_playable,
    } = state.view.connection
    {
        let cached_note = if cached_playable {
            "cached tracks playable"
        } else {
            "waiting for network"
        };
        items.push(
            container(
                text(format!(
                    "Connection lost — retrying in {retry_in_secs}s · {cached_note}"
                ))
                .size(12)
                .font(theme::FONT_MONO)
                .color(theme::STATUS_WARN),
            )
            .padding([5.0, theme::SPACE_LG])
            .width(Length::Fill)
            .style(theme::banner_container(false))
            .into(),
        );
    }

    if let Some(err) = &state.view.crate_view.error {
        items.push(
            container(
                text(err)
                    .size(12)
                    .font(theme::FONT_SANS)
                    .color(theme::STATUS_ERR),
            )
            .padding([5.0, theme::SPACE_LG])
            .width(Length::Fill)
            .style(theme::banner_container(true))
            .into(),
        );
    }

    if items.is_empty() {
        None
    } else {
        Some(column(items).width(Length::Fill).into())
    }
}

fn view_track_crate<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let is_album_view = matches!(cv.active_crate, CrateRef::Album { .. });
    let hide_album_col = state.hide_album_column() || is_album_view;

    let cover_handle = cv.cover.as_ref().and_then(|c| state.covers.thumb(&c.id));
    let mut header_left = row![].spacing(theme::SPACE_MD).align_y(Vertical::Center);

    if cv.history_index > 0 {
        header_left = header_left.push(
            button(widgets::icon_svg(icons::BACK, 16.0, theme::TEXT_SECONDARY))
                .padding([6.0, 8.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::NavigateBack)),
        );
    }

    header_left = header_left
        .push(widgets::cover_thumb(
            cover_handle,
            72.0,
            Some(icons::MUSIC_NOTE),
        ))
        .push(
            column![
                container(
                    text(&cv.title)
                        .size(20)
                        .font(theme::FONT_SANS_BOLD)
                        .color(theme::TEXT_PRIMARY)
                        .wrapping(Wrapping::None),
                )
                .width(Length::Fill)
                .clip(true),
                text(if cv.subtitle.is_empty() {
                    crate_subtitle_fallback(&cv.active_crate, cv.loaded_tracks.total())
                } else {
                    cv.subtitle.clone()
                })
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
                row![
                    button(
                        row![
                            widgets::icon_svg(icons::PLAY, 12.0, theme::ACCENT_TEXT_ON),
                            text("Play")
                                .size(12)
                                .font(theme::FONT_SANS_SEMIBOLD),
                        ]
                        .spacing(6)
                        .align_y(Vertical::Center),
                    )
                    .padding([5.0, 12.0])
                    .style(theme::primary_button)
                    .on_press(UiMsg::Command(UiCommand::PlayCrate {
                        crate_ref: cv.active_crate.clone(),
                        shuffle: false,
                    })),
                    button(
                        row![
                            widgets::icon_svg(icons::SHUFFLE, 12.0, theme::TEXT_PRIMARY),
                            text("Shuffle")
                                .size(12)
                                .font(theme::FONT_SANS_MEDIUM),
                        ]
                        .spacing(6)
                        .align_y(Vertical::Center),
                    )
                    .padding([5.0, 10.0])
                    .style(theme::secondary_button)
                    .on_press(UiMsg::Command(UiCommand::PlayCrate {
                        crate_ref: cv.active_crate.clone(),
                        shuffle: true,
                    })),
                    button(
                        text("+ Queue")
                            .size(12)
                            .font(theme::FONT_SANS_MEDIUM),
                    )
                    .padding([5.0, 10.0])
                    .style(theme::secondary_button)
                    .on_press(UiMsg::Command(UiCommand::QueueCrate(
                        cv.active_crate.clone(),
                    ))),
                ]
                .spacing(theme::SPACE_SM)
                .align_y(Vertical::Center),
            ]
            .spacing(5)
            .width(Length::Fill),
        );

    let crate_header = container(header_left)
        .padding([theme::SPACE_SM, theme::SPACE_LG])
        .width(Length::Fill)
        .height(Length::Fixed(theme::CRATE_HEADER_HEIGHT))
        .center_y(Length::Fixed(theme::CRATE_HEADER_HEIGHT));

    let col_header = view_track_table_header(is_album_view, hide_album_col);

    let filter_trimmed = cv.local_filter.trim();
    let table_body: Element<'a, UiMsg> = if !filter_trimmed.is_empty() {
        let matched = matched_filter_indices(cv);
        if matched.is_empty() {
            view_filter_empty_state(filter_trimmed, cv.is_loading)
        } else {
            let total = matched.len();
            let first = ((state.crate_scroll_y / theme::TRACK_ROW_HEIGHT).floor() as usize)
                .saturating_sub(OVERSCAN_ROWS)
                .min(total);
            let visible_count =
                (state.crate_viewport_height / theme::TRACK_ROW_HEIGHT).ceil() as usize
                    + OVERSCAN_ROWS * 2;
            let end = (first + visible_count).min(total);
            let top_px = first as f32 * theme::TRACK_ROW_HEIGHT;
            let bot_px = total.saturating_sub(end) as f32 * theme::TRACK_ROW_HEIGHT;

            let mut list_col = column![].width(Length::Fill).padding([0.0, theme::SPACE_MD]);
            if top_px > 0.5 {
                list_col = list_col.push(Space::new().height(top_px));
            }
            for &global_idx in &matched[first..end] {
                if let Some(track) = cv.loaded_tracks.get(global_idx) {
                    list_col = list_col.push(view_track_row(
                        state,
                        global_idx,
                        track,
                        is_album_view,
                        hide_album_col,
                        crate_context_uri(&cv.active_crate),
                    ));
                }
            }
            if bot_px > 0.5 {
                list_col = list_col.push(Space::new().height(bot_px));
            }
            scrollable(list_col)
                .id(id_crate_scroll())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_scroll(UiMsg::CrateScrolled)
                .style(theme::dark_scrollable)
                .into()
        }
    } else {
        let total = cv.loaded_tracks.total();
        if total == 0 && !cv.is_loading {
            container(
                text("This crate has no tracks yet.")
                    .size(13)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_MUTED),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else if total == 0 {
            container(
                text("Loading tracks...")
                    .size(13)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_SECONDARY),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else {
            let first = ((state.crate_scroll_y / theme::TRACK_ROW_HEIGHT).floor() as usize)
                .saturating_sub(OVERSCAN_ROWS)
                .min(total);
            let visible_count =
                (state.crate_viewport_height / theme::TRACK_ROW_HEIGHT).ceil() as usize
                    + OVERSCAN_ROWS * 2;
            let end = (first + visible_count).min(total);
            let top_px = first as f32 * theme::TRACK_ROW_HEIGHT;
            let bot_px = total.saturating_sub(end) as f32 * theme::TRACK_ROW_HEIGHT;

            let mut list_col = column![].width(Length::Fill).padding([0.0, theme::SPACE_MD]);
            if top_px > 0.5 {
                list_col = list_col.push(Space::new().height(top_px));
            }
            for idx in first..end {
                if let Some(track) = cv.loaded_tracks.get(idx) {
                    list_col = list_col.push(view_track_row(
                        state,
                        idx,
                        track,
                        is_album_view,
                        hide_album_col,
                        crate_context_uri(&cv.active_crate),
                    ));
                } else {
                    list_col = list_col.push(view_track_skeleton_row(idx));
                }
            }
            if bot_px > 0.5 {
                list_col = list_col.push(Space::new().height(bot_px));
            }

            scrollable(list_col)
                .id(id_crate_scroll())
                .width(Length::Fill)
                .height(Length::Fill)
                .on_scroll(UiMsg::CrateScrolled)
                .style(theme::dark_scrollable)
                .into()
        }
    };

    column![
        crate_header,
        widgets::hairline_h(),
        col_header,
        widgets::hairline_h(),
        table_body,
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn view_track_table_header<'a>(is_album_view: bool, hide_album_col: bool) -> Element<'a, UiMsg> {
    let mut header = row![
        container(widgets::section_label("#"))
            .width(Length::Fixed(34.0))
            .align_x(Alignment::Center),
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Vertical::Center);

    if !is_album_view {
        header = header.push(Space::new().width(Length::Fixed(32.0)));
    }

    header = header.push(
        container(widgets::section_label("TITLE / ARTIST")).width(Length::FillPortion(5)),
    );

    if !hide_album_col {
        header = header.push(
            container(widgets::section_label("ALBUM")).width(Length::FillPortion(3)),
        );
    }

    header = header
        .push(Space::new().width(Length::Fixed(34.0)))
        .push(
            container(widgets::icon_svg(icons::HEART_OUTLINE, 12.0, theme::TEXT_MUTED))
                .width(Length::Fixed(28.0))
                .align_x(Alignment::Center),
        )
        .push(
            container(widgets::section_label("TIME"))
                .width(Length::Fixed(48.0))
                .align_x(Alignment::End),
        );

    container(header)
        .padding([4.0, theme::SPACE_LG])
        .width(Length::Fill)
        .height(Length::Fixed(28.0))
        .center_y(Length::Fixed(28.0))
        .into()
}

fn view_track_row<'a>(
    state: &'a UiState,
    idx: usize,
    track: &'a TrackSummary,
    is_album_view: bool,
    hide_album_col: bool,
    context_uri: String,
) -> Element<'a, UiMsg> {
    let active_uri = state
        .view
        .player_state
        .active_track
        .as_ref()
        .map(|d| d.summary.uri.as_ref());
    let is_playing_row = active_uri == Some(track.uri.as_ref());
    let is_selected =
        state.active_pane == ActivePane::Crate && state.selected_crate_row == Some(idx);
    let is_hovered = state.hovered_track_row == Some(idx);

    let title_color = if !track.is_playable {
        theme::TEXT_UNAVAILABLE
    } else if is_playing_row {
        theme::ACCENT_PRIMARY
    } else {
        theme::TEXT_PRIMARY
    };
    let meta_color = if !track.is_playable {
        theme::TEXT_UNAVAILABLE
    } else {
        theme::TEXT_SECONDARY
    };

    let idx_cell: Element<'a, UiMsg> = if is_playing_row && track.is_playable {
        widgets::icon_svg(icons::PLAY, 12.0, theme::ACCENT_PRIMARY)
    } else if (is_hovered || is_selected) && track.is_playable {
        button(widgets::icon_svg(icons::PLAY, 12.0, theme::TEXT_PRIMARY))
            .padding(4)
            .style(theme::ghost_button)
            .on_press(UiMsg::Command(UiCommand::PlayTrackInContext {
                context_uri: context_uri.clone(),
                track_uri: track.uri.to_string(),
                index: idx,
            }))
            .into()
    } else {
        text(format!("{:02}", idx + 1))
            .size(11)
            .font(theme::FONT_MONO)
            .color(if track.is_playable {
                theme::TEXT_MUTED
            } else {
                theme::TEXT_UNAVAILABLE
            })
            .into()
    };

    let left_bar_color = if is_selected || is_playing_row {
        theme::ACCENT_PRIMARY
    } else {
        iced::Color::TRANSPARENT
    };

    let mut inner = row![
        container(text("").size(1))
            .width(Length::Fixed(2.0))
            .height(Length::Fixed(theme::TRACK_ROW_HEIGHT - 10.0))
            .style(theme::accent_strip(left_bar_color)),
        container(idx_cell)
            .width(Length::Fixed(30.0))
            .align_x(Alignment::Center),
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Vertical::Center);

    if !is_album_view {
        let cover = track
            .cover
            .as_ref()
            .and_then(|c| state.covers.thumb(&c.id));
        inner = inner.push(widgets::cover_thumb(cover, 32.0, Some(icons::MUSIC_NOTE)));
    }

    let title_artist = column![
        container(
            text(&track.title)
                .size(14)
                .font(theme::FONT_SANS_MEDIUM)
                .color(title_color)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        container(
            text(track.artist.as_ref())
                .size(12)
                .font(theme::FONT_SANS)
                .color(meta_color)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
    ]
    .width(Length::FillPortion(5))
    .spacing(1);

    inner = inner.push(title_artist);

    if !hide_album_col {
        inner = inner.push(
            container(
                text(track.album.as_ref())
                    .size(13)
                    .font(theme::FONT_SANS)
                    .color(meta_color)
                    .wrapping(Wrapping::None),
            )
            .width(Length::FillPortion(3))
            .clip(true),
        );
    }

    let enqueue_btn: Element<'a, UiMsg> = if (is_hovered || is_selected) && track.is_playable {
        button(
            text("+Q")
                .size(11)
                .font(theme::FONT_MONO_SEMIBOLD)
                .color(theme::TEXT_SECONDARY),
        )
        .padding([3.0, 6.0])
        .style(theme::secondary_button)
        .on_press(UiMsg::Command(UiCommand::EnqueueNext(track.clone())))
        .into()
    } else {
        Space::new().width(Length::Fixed(34.0)).into()
    };

    let heart_icon = if track.is_liked {
        icons::HEART_FILLED
    } else {
        icons::HEART_OUTLINE
    };
    let heart_color = if track.is_liked {
        theme::ACCENT_PRIMARY
    } else if !track.is_playable {
        theme::TEXT_UNAVAILABLE
    } else {
        theme::TEXT_MUTED
    };

    let like_btn = button(widgets::icon_svg(heart_icon, 14.0, heart_color))
        .padding([4.0, 5.0])
        .style(theme::ghost_button)
        .on_press(UiMsg::Command(UiCommand::ToggleLike(track.uri.to_string())));

    inner = inner
        .push(
            container(enqueue_btn)
                .width(Length::Fixed(34.0))
                .align_x(Alignment::Center),
        )
        .push(
            container(like_btn)
                .width(Length::Fixed(28.0))
                .align_x(Alignment::Center),
        )
        .push(
            container(
                text(widgets::format_duration_ms(track.duration_ms))
                    .size(12)
                    .font(theme::FONT_MONO)
                    .color(if track.is_playable {
                        theme::TEXT_SECONDARY
                    } else {
                        theme::TEXT_UNAVAILABLE
                    }),
            )
            .width(Length::Fixed(48.0))
            .align_x(Alignment::End),
        );

    let row_box = container(inner)
        .padding([0.0, theme::SPACE_SM])
        .width(Length::Fill)
        .height(Length::Fixed(theme::TRACK_ROW_HEIGHT))
        .center_y(Length::Fixed(theme::TRACK_ROW_HEIGHT))
        .style(theme::row_container(is_playing_row, is_selected, is_hovered));

    let mut area = mouse_area(row_box)
        .on_enter(UiMsg::HoverTrackRow(Some(idx)))
        .on_exit(UiMsg::HoverTrackRow(None))
        .on_press(UiMsg::SelectCrateRow(idx))
        .on_right_press(UiMsg::OpenTrackContextMenu {
            index: idx,
            track: track.clone(),
            pos: state.cursor_pos,
        });

    if track.is_playable {
        area = area.on_double_click(UiMsg::Command(UiCommand::PlayTrackInContext {
            context_uri,
            track_uri: track.uri.to_string(),
            index: idx,
        }));
    }

    area.into()
}

fn view_track_skeleton_row<'a>(idx: usize) -> Element<'a, UiMsg> {
    container(
        row![
            Space::new().width(Length::Fixed(2.0)),
            container(
                text(format!("{:02}", idx + 1))
                    .size(11)
                    .font(theme::FONT_MONO)
                    .color(theme::TEXT_MUTED),
            )
            .width(Length::Fixed(30.0))
            .align_x(Alignment::Center),
            container(text("").size(1))
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(32.0))
                .style(theme::cover_placeholder(theme::RADIUS_SM)),
            text("Loading track...")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Vertical::Center),
    )
    .padding([0.0, theme::SPACE_SM])
    .width(Length::Fill)
    .height(Length::Fixed(theme::TRACK_ROW_HEIGHT))
    .center_y(Length::Fixed(theme::TRACK_ROW_HEIGHT))
    .into()
}

fn view_filter_empty_state<'a>(query: &str, is_loading: bool) -> Element<'a, UiMsg> {
    let hint = if is_loading {
        format!("No loaded tracks match '{query}' yet (still fetching pages...)")
    } else {
        format!("No tracks match '{query}'")
    };

    container(
        column![
            text(hint)
                .size(14)
                .font(theme::FONT_SANS_MEDIUM)
                .color(theme::TEXT_SECONDARY),
            row![
                button(
                    text("Clear Filter (Esc)")
                        .size(12)
                        .font(theme::FONT_SANS_MEDIUM),
                )
                .padding([7.0, 14.0])
                .style(theme::secondary_button)
                .on_press(UiMsg::ClearFilter),
                button(
                    text("Search All Spotify")
                        .size(12)
                        .font(theme::FONT_SANS_SEMIBOLD),
                )
                .padding([7.0, 14.0])
                .style(theme::primary_button)
                .on_press(UiMsg::Command(UiCommand::RunGlobalSearch(
                    query.to_owned(),
                ))),
            ]
            .spacing(theme::SPACE_MD),
        ]
        .spacing(theme::SPACE_MD)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn view_playlists_grid<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let total = cv.playlists.total();
    let cols = grid_columns(state);

    let header = index_header("Playlists", total, cv.history_index > 0);
    if total == 0 {
        return column![
            header,
            widgets::hairline_h(),
            empty_or_loading(cv.is_loading, "No playlists loaded yet."),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    }

    let row_count = total.div_ceil(cols);
    let (first_row, end_row, top_px, bot_px) = virtual_grid_range(state, row_count);

    let mut grid_col = column![]
        .spacing(theme::SPACE_MD)
        .padding(theme::SPACE_LG)
        .width(Length::Fill);
    if top_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(top_px));
    }
    for r in first_row..end_row {
        let mut r_row = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for c in 0..cols {
            let idx = r * cols + c;
            if idx < total {
                if let Some(p) = cv.playlists.get(idx) {
                    r_row = r_row.push(playlist_card(state, p));
                } else {
                    r_row = r_row.push(skeleton_grid_card());
                }
            } else {
                r_row = r_row.push(Space::new().width(Length::Fill));
            }
        }
        grid_col = grid_col.push(r_row);
    }
    if bot_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(bot_px));
    }

    let scroller = scrollable(grid_col)
        .id(id_crate_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::CrateScrolled)
        .style(theme::dark_scrollable);

    column![header, widgets::hairline_h(), scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn view_albums_grid<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let total = cv.albums.total();
    let cols = grid_columns(state);

    let header = index_header("Saved Albums", total, cv.history_index > 0);
    if total == 0 {
        return column![
            header,
            widgets::hairline_h(),
            empty_or_loading(cv.is_loading, "No saved albums loaded yet."),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    }

    let row_count = total.div_ceil(cols);
    let (first_row, end_row, top_px, bot_px) = virtual_grid_range(state, row_count);

    let mut grid_col = column![]
        .spacing(theme::SPACE_MD)
        .padding(theme::SPACE_LG)
        .width(Length::Fill);
    if top_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(top_px));
    }
    for r in first_row..end_row {
        let mut r_row = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for c in 0..cols {
            let idx = r * cols + c;
            if idx < total {
                if let Some(album) = cv.albums.get(idx) {
                    r_row = r_row.push(album_card(state, album));
                } else {
                    r_row = r_row.push(skeleton_grid_card());
                }
            } else {
                r_row = r_row.push(Space::new().width(Length::Fill));
            }
        }
        grid_col = grid_col.push(r_row);
    }
    if bot_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(bot_px));
    }

    let scroller = scrollable(grid_col)
        .id(id_crate_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::CrateScrolled)
        .style(theme::dark_scrollable);

    column![header, widgets::hairline_h(), scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn view_artists_grid<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let total = cv.artists.total();
    let cols = grid_columns(state);

    let header = index_header("Followed Artists", total, cv.history_index > 0);
    if total == 0 {
        return column![
            header,
            widgets::hairline_h(),
            empty_or_loading(cv.is_loading, "No followed artists loaded yet."),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    }

    let row_count = total.div_ceil(cols);
    let (first_row, end_row, top_px, bot_px) = virtual_grid_range(state, row_count);

    let mut grid_col = column![]
        .spacing(theme::SPACE_MD)
        .padding(theme::SPACE_LG)
        .width(Length::Fill);
    if top_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(top_px));
    }
    for r in first_row..end_row {
        let mut r_row = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for c in 0..cols {
            let idx = r * cols + c;
            if idx < total {
                if let Some(artist) = cv.artists.get(idx) {
                    r_row = r_row.push(artist_card(state, artist));
                } else {
                    r_row = r_row.push(skeleton_grid_card());
                }
            } else {
                r_row = r_row.push(Space::new().width(Length::Fill));
            }
        }
        grid_col = grid_col.push(r_row);
    }
    if bot_px > 0.5 {
        grid_col = grid_col.push(Space::new().height(bot_px));
    }

    let scroller = scrollable(grid_col)
        .id(id_crate_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::CrateScrolled)
        .style(theme::dark_scrollable);

    column![header, widgets::hairline_h(), scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn view_artist_detail<'a>(
    state: &'a UiState,
    detail: Option<&'a ArtistDetail>,
) -> Element<'a, UiMsg> {
    let Some(detail) = detail else {
        return empty_or_loading(state.view.crate_view.is_loading, "Artist details unavailable.");
    };

    let artist = &detail.artist;
    let cover = artist
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));

    let back_target = if state.view.crate_view.history_index > 0 {
        UiMsg::Command(UiCommand::NavigateBack)
    } else {
        UiMsg::Command(UiCommand::OpenCrate(CrateRef::ArtistsIndex))
    };

    let follow_label = if artist.is_followed {
        "Following"
    } else {
        "Follow"
    };

    let header = container(
        row![
            button(widgets::icon_svg(icons::BACK, 16.0, theme::TEXT_SECONDARY))
                .padding([6.0, 8.0])
                .style(theme::ghost_button)
                .on_press(back_target),
            widgets::cover_thumb(cover, 72.0, Some(icons::MUSIC_NOTE)),
            column![
                text(&artist.name)
                    .size(20)
                    .font(theme::FONT_SANS_BOLD)
                    .color(theme::TEXT_PRIMARY),
                row![
                    button(
                        row![
                            widgets::icon_svg(icons::PLAY, 12.0, theme::ACCENT_TEXT_ON),
                            text("Play Top")
                                .size(12)
                                .font(theme::FONT_SANS_SEMIBOLD),
                        ]
                        .spacing(6)
                        .align_y(Vertical::Center),
                    )
                    .padding([5.0, 12.0])
                    .style(theme::primary_button)
                    .on_press(UiMsg::Command(UiCommand::PlayCrate {
                        crate_ref: CrateRef::Artist {
                            uri: artist.uri.clone(),
                            name: artist.name.clone(),
                        },
                        shuffle: false,
                    })),
                    button(
                        text(follow_label)
                            .size(12)
                            .font(theme::FONT_SANS_MEDIUM),
                    )
                    .padding([5.0, 12.0])
                    .style(theme::secondary_button)
                    .on_press(UiMsg::Command(UiCommand::ToggleFollowArtist {
                        artist_uri: artist.uri.to_string(),
                        follow: !artist.is_followed,
                    })),
                ]
                .spacing(theme::SPACE_SM),
            ]
            .spacing(6)
            .width(Length::Fill),
        ]
        .spacing(theme::SPACE_MD)
        .align_y(Vertical::Center),
    )
    .padding([theme::SPACE_SM, theme::SPACE_LG])
    .width(Length::Fill)
    .height(Length::Fixed(theme::CRATE_HEADER_HEIGHT))
    .center_y(Length::Fixed(theme::CRATE_HEADER_HEIGHT));

    let mut body = column![]
        .spacing(theme::SPACE_MD)
        .padding([theme::SPACE_MD, theme::SPACE_LG])
        .width(Length::Fill);

    if !detail.top_tracks.is_empty() {
        body = body.push(widgets::section_label("TOP TRACKS"));
        let hide_album = state.hide_album_column();
        for (idx, track) in detail.top_tracks.iter().take(5).enumerate() {
            body = body.push(view_track_row(
                state,
                idx,
                track,
                false,
                hide_album,
                artist.uri.to_string(),
            ));
        }
    }

    let cols = grid_columns(state);
    if !detail.albums.is_empty() {
        body = body.push(widgets::section_label("ALBUMS"));
        body = body.push(album_slice_grid(state, &detail.albums, cols));
    }
    if !detail.singles.is_empty() {
        body = body.push(widgets::section_label("SINGLES & EPS"));
        body = body.push(album_slice_grid(state, &detail.singles, cols));
    }

    let scroller = scrollable(body)
        .id(id_crate_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::CrateScrolled)
        .style(theme::dark_scrollable);

    column![header, widgets::hairline_h(), scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn view_search_results<'a>(
    state: &'a UiState,
    query: &'a str,
    results: Option<&'a SearchResults>,
) -> Element<'a, UiMsg> {
    let cv = &state.view.crate_view;
    let chips = [
        (SearchFilter::All, "All"),
        (SearchFilter::Tracks, "Tracks"),
        (SearchFilter::Albums, "Albums"),
        (SearchFilter::Artists, "Artists"),
        (SearchFilter::Playlists, "Playlists"),
    ];

    let mut chip_row = row![
        text(format!("Results for \"{query}\""))
            .size(16)
            .font(theme::FONT_SANS_BOLD)
            .color(theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
    ]
    .spacing(theme::SPACE_XS)
    .align_y(Vertical::Center);

    for (filter, label) in chips {
        let active = cv.search_filter == filter;
        chip_row = chip_row.push(
            button(text(label).size(12).font(theme::FONT_SANS_MEDIUM))
                .padding([4.0, 10.0])
                .style(theme::pill_button(active))
                .on_press(UiMsg::Command(UiCommand::SetSearchFilter(filter))),
        );
    }

    let header = container(chip_row)
        .padding([theme::SPACE_SM, theme::SPACE_LG])
        .width(Length::Fill);

    let Some(res) = results else {
        return column![
            header,
            widgets::hairline_h(),
            empty_or_loading(cv.is_loading, "Press Ctrl+K to search Spotify."),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    };

    let mut body = column![]
        .spacing(theme::SPACE_MD)
        .padding([theme::SPACE_MD, theme::SPACE_LG])
        .width(Length::Fill);

    if res.tracks_only_fallback {
        body = body.push(
            container(
                text(
                    "Using librespot search context fallback (tracks only). Add a custom Web API Client ID in Settings for artist/album catalog sections."
                )
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_MUTED),
            )
            .padding([6.0, 10.0])
            .width(Length::Fill)
            .style(theme::surface_card),
        );
    }

    let show_all = cv.search_filter == SearchFilter::All;
    let cols = grid_columns(state);

    if (show_all || cv.search_filter == SearchFilter::Artists) && !res.artists.is_empty() {
        body = body.push(widgets::section_label("ARTISTS"));
        let limit = if show_all { 4 } else { res.artists.len() };
        body = body.push(artist_slice_grid(state, &res.artists[..limit.min(res.artists.len())], cols));
    }

    if (show_all || cv.search_filter == SearchFilter::Albums) && !res.albums.is_empty() {
        body = body.push(widgets::section_label("ALBUMS"));
        let limit = if show_all { 4 } else { res.albums.len() };
        body = body.push(album_slice_grid(state, &res.albums[..limit.min(res.albums.len())], cols));
    }

    if (show_all || cv.search_filter == SearchFilter::Playlists) && !res.playlists.is_empty() {
        body = body.push(widgets::section_label("PLAYLISTS"));
        let limit = if show_all { 4 } else { res.playlists.len() };
        body = body.push(playlist_slice_grid(
            state,
            &res.playlists[..limit.min(res.playlists.len())],
            cols,
        ));
    }

    if (show_all || cv.search_filter == SearchFilter::Tracks) && !res.tracks.is_empty() {
        body = body.push(widgets::section_label("TRACKS"));
        let hide_album = state.hide_album_column();
        let search_ctx = format!("spotify:search:{}", res.query);
        for (idx, track) in res.tracks.iter().enumerate() {
            body = body.push(view_track_row(
                state,
                idx,
                track,
                false,
                hide_album,
                search_ctx.clone(),
            ));
        }
    }

    let scroller = scrollable(body)
        .id(id_crate_scroll())
        .width(Length::Fill)
        .height(Length::Fill)
        .on_scroll(UiMsg::CrateScrolled)
        .style(theme::dark_scrollable);

    column![header, widgets::hairline_h(), scroller]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn index_header<'a>(title: &'a str, count: usize, can_back: bool) -> Element<'a, UiMsg> {
    let mut left = row![].spacing(theme::SPACE_SM).align_y(Vertical::Center);
    if can_back {
        left = left.push(
            button(widgets::icon_svg(icons::BACK, 15.0, theme::TEXT_SECONDARY))
                .padding([4.0, 6.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::Command(UiCommand::NavigateBack)),
        );
    }
    left = left
        .push(
            text(title)
                .size(20)
                .font(theme::FONT_SANS_BOLD)
                .color(theme::TEXT_PRIMARY),
        )
        .push(
            text(format!("{count} items"))
                .size(12)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_MUTED),
        );

    container(left)
        .padding([theme::SPACE_MD, theme::SPACE_LG])
        .width(Length::Fill)
        .into()
}

fn empty_or_loading<'a>(is_loading: bool, empty_msg: &'a str) -> Element<'a, UiMsg> {
    let msg = if is_loading { "Loading..." } else { empty_msg };
    container(
        text(msg)
            .size(13)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_SECONDARY),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn playlist_card<'a>(state: &'a UiState, playlist: &'a PlaylistSummary) -> Element<'a, UiMsg> {
    let cover = playlist
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));

    let body = column![
        widgets::cover_thumb(cover, 96.0, Some(icons::MUSIC_NOTE)),
        container(
            text(&playlist.name)
                .size(14)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        container(
            text(format!(
                "{} · {} tracks",
                playlist.owner, playlist.track_count
            ))
            .size(12)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_SECONDARY)
            .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
    ]
    .spacing(6);

    button(
        container(body)
            .padding(theme::SPACE_MD)
            .width(Length::Fill)
            .height(Length::Fixed(GRID_CARD_ROW_HEIGHT - theme::SPACE_MD))
            .style(theme::surface_card),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::plain_interactive_button)
    .on_press(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Playlist {
        uri: playlist.uri.clone(),
        name: playlist.name.clone(),
    })))
    .into()
}

fn album_card<'a>(state: &'a UiState, album: &'a AlbumSummary) -> Element<'a, UiMsg> {
    let cover = album
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));
    let sub = match album.release_year {
        Some(y) => format!("{} · {y}", album.artist),
        None => album.artist.to_string(),
    };

    let body = column![
        widgets::cover_thumb(cover, 96.0, Some(icons::MUSIC_NOTE)),
        container(
            text(&album.title)
                .size(14)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        container(
            text(sub)
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
    ]
    .spacing(6);

    button(
        container(body)
            .padding(theme::SPACE_MD)
            .width(Length::Fill)
            .height(Length::Fixed(GRID_CARD_ROW_HEIGHT - theme::SPACE_MD))
            .style(theme::surface_card),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::plain_interactive_button)
    .on_press(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Album {
        uri: album.uri.clone(),
        title: album.title.clone(),
    })))
    .into()
}

fn artist_card<'a>(state: &'a UiState, artist: &'a ArtistSummary) -> Element<'a, UiMsg> {
    let cover = artist
        .cover
        .as_ref()
        .and_then(|c| state.covers.thumb(&c.id));
    let sub = if artist.is_followed {
        "Followed Artist"
    } else {
        "Artist"
    };

    let body = column![
        widgets::cover_thumb(cover, 96.0, Some(icons::MUSIC_NOTE)),
        container(
            text(&artist.name)
                .size(14)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        text(sub)
            .size(12)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_SECONDARY),
    ]
    .spacing(6);

    button(
        container(body)
            .padding(theme::SPACE_MD)
            .width(Length::Fill)
            .height(Length::Fixed(GRID_CARD_ROW_HEIGHT - theme::SPACE_MD))
            .style(theme::surface_card),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::plain_interactive_button)
    .on_press(UiMsg::Command(UiCommand::OpenCrate(CrateRef::Artist {
        uri: artist.uri.clone(),
        name: artist.name.clone(),
    })))
    .into()
}

fn skeleton_grid_card<'a>() -> Element<'a, UiMsg> {
    container(
        column![
            container(text("").size(1))
                .width(Length::Fixed(96.0))
                .height(Length::Fixed(96.0))
                .style(theme::cover_placeholder(theme::RADIUS_SM)),
            text("Loading...")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(8),
    )
    .padding(theme::SPACE_MD)
    .width(Length::Fill)
    .height(Length::Fixed(GRID_CARD_ROW_HEIGHT - theme::SPACE_MD))
    .style(theme::surface_card)
    .into()
}

fn playlist_slice_grid<'a>(
    state: &'a UiState,
    items: &'a [PlaylistSummary],
    cols: usize,
) -> Element<'a, UiMsg> {
    let mut grid = column![].spacing(theme::SPACE_MD).width(Length::Fill);
    for chunk in items.chunks(cols) {
        let mut r = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for item in chunk {
            r = r.push(playlist_card(state, item));
        }
        for _ in chunk.len()..cols {
            r = r.push(Space::new().width(Length::Fill));
        }
        grid = grid.push(r);
    }
    grid.into()
}

fn album_slice_grid<'a>(
    state: &'a UiState,
    items: &'a [AlbumSummary],
    cols: usize,
) -> Element<'a, UiMsg> {
    let mut grid = column![].spacing(theme::SPACE_MD).width(Length::Fill);
    for chunk in items.chunks(cols) {
        let mut r = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for item in chunk {
            r = r.push(album_card(state, item));
        }
        for _ in chunk.len()..cols {
            r = r.push(Space::new().width(Length::Fill));
        }
        grid = grid.push(r);
    }
    grid.into()
}

fn artist_slice_grid<'a>(
    state: &'a UiState,
    items: &'a [ArtistSummary],
    cols: usize,
) -> Element<'a, UiMsg> {
    let mut grid = column![].spacing(theme::SPACE_MD).width(Length::Fill);
    for chunk in items.chunks(cols) {
        let mut r = row![].spacing(theme::SPACE_MD).width(Length::Fill);
        for item in chunk {
            r = r.push(artist_card(state, item));
        }
        for _ in chunk.len()..cols {
            r = r.push(Space::new().width(Length::Fill));
        }
        grid = grid.push(r);
    }
    grid.into()
}

fn grid_columns(state: &UiState) -> usize {
    let crate_w = if state.is_narrow_workbench() {
        state.window_size.width
    } else {
        (state.window_size.width - state.stage_width).max(320.0)
    };
    ((crate_w / 210.0).floor() as usize).clamp(2, 4)
}

fn virtual_grid_range(state: &UiState, row_count: usize) -> (usize, usize, f32, f32) {
    let first = ((state.crate_scroll_y / GRID_CARD_ROW_HEIGHT).floor() as usize)
        .saturating_sub(2)
        .min(row_count);
    let visible = (state.crate_viewport_height / GRID_CARD_ROW_HEIGHT).ceil() as usize + 4;
    let end = (first + visible).min(row_count);
    let top_px = first as f32 * GRID_CARD_ROW_HEIGHT;
    let bot_px = row_count.saturating_sub(end) as f32 * GRID_CARD_ROW_HEIGHT;
    (first, end, top_px, bot_px)
}

fn matched_filter_indices(cv: &pulse_model::CrateViewState) -> Vec<usize> {
    let needle = cv.local_filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    cv.loaded_tracks
        .iter_loaded()
        .filter(|(_, track)| {
            track.title.to_ascii_lowercase().contains(&needle)
                || track.artist.to_ascii_lowercase().contains(&needle)
                || track.album.to_ascii_lowercase().contains(&needle)
        })
        .map(|(idx, _)| idx)
        .collect()
}

fn crate_context_uri(active_crate: &CrateRef) -> String {
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

fn crate_subtitle_fallback(crate_ref: &CrateRef, total: usize) -> String {
    match crate_ref {
        CrateRef::LikedSongs => format!("Your Saved Library · {total} tracks"),
        CrateRef::Recent => format!("Recently Played · {total} tracks"),
        CrateRef::Playlist { .. } => format!("Playlist · {total} tracks"),
        CrateRef::Album { .. } => format!("Album · {total} tracks"),
        _ => format!("{total} tracks"),
    }
}

const fn sort_label(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::Custom => "Custom",
        SortOrder::RecentlyAdded => "Added",
        SortOrder::Title => "Title",
        SortOrder::Artist => "Artist",
        SortOrder::Duration => "Duration",
    }
}

const fn next_sort_order(sort: SortOrder) -> SortOrder {
    match sort {
        SortOrder::Custom => SortOrder::RecentlyAdded,
        SortOrder::RecentlyAdded => SortOrder::Title,
        SortOrder::Title => SortOrder::Artist,
        SortOrder::Artist => SortOrder::Duration,
        SortOrder::Duration => SortOrder::Custom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sample_track(title: &str, artist: &str) -> TrackSummary {
        TrackSummary {
            uri: Arc::from("spotify:track:test"),
            title: title.to_owned(),
            artist: Arc::from(artist),
            artist_uri: None,
            album: Arc::from("Album"),
            album_uri: None,
            cover: None,
            duration_ms: 200_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
        }
    }

    #[test]
    fn local_filter_matches_loaded_rows_case_insensitively() {
        let mut ui = UiState::default();
        ui.view.crate_view.loaded_tracks.insert_page(
            0,
            vec![
                sample_track("Midnight City", "M83"),
                sample_track("Halcyon", "Bicep"),
            ],
            2,
        );
        ui.view.crate_view.local_filter = String::from("m83");
        let matched = matched_filter_indices(&ui.view.crate_view);
        assert_eq!(matched, vec![0]);

        let _ = view(&ui);
        ui.view.crate_view.active_crate = CrateRef::PlaylistsIndex;
        let _ = view(&ui);
        ui.view.crate_view.active_crate = CrateRef::AlbumsIndex;
        let _ = view(&ui);
        ui.view.crate_view.active_crate = CrateRef::ArtistsIndex;
        let _ = view(&ui);
    }

    #[test]
    fn recent_crate_uses_recent_context_uri() {
        assert_eq!(crate_context_uri(&CrateRef::Recent), "recent");
        assert_eq!(
            crate_context_uri(&CrateRef::LikedSongs),
            "spotify:collection:tracks"
        );
    }
}
