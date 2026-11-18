//! `Ctrl+K` Global Spotify Search & Quick Command Omnibox palette.

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length, Padding};
use pulse_assets::icons;
use pulse_model::{AppMode, CrateRef, UiCommand};

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::{UiState, id_omnibox_input};
use crate::widgets;

/// Resolved omnibox match entry rendered inside the palette and triggered on Enter/click.
#[derive(Debug, Clone)]
pub struct OmniboxEntry {
    pub kind_badge: &'static str,
    pub title: String,
    pub subtitle: String,
    pub action: UiMsg,
}

/// Computes bounded instant local matches + `>` quick commands (`<= 12` rows).
pub fn omnibox_entries(state: &UiState) -> Vec<OmniboxEntry> {
    let raw = state.omnibox_query.trim();
    if let Some(cmd_query) = raw.strip_prefix('>') {
        let q = cmd_query.trim().to_ascii_lowercase();
        let all_cmds = [
            (
                "CMD",
                "> device",
                "Open Playback Device Switcher",
                UiMsg::ToggleDevicePopover,
            ),
            (
                "CMD",
                "> shuffle",
                "Toggle playback shuffle",
                UiMsg::Command(UiCommand::ToggleShuffle),
            ),
            (
                "CMD",
                "> clear queue",
                "Clear manual priority queue",
                UiMsg::Command(UiCommand::ClearManualQueue),
            ),
            (
                "CMD",
                "> settings",
                "Open Account, Audio & Memory Settings",
                UiMsg::ToggleSettings,
            ),
            (
                "CMD",
                "> purge cache",
                "Purge decoded cover art RAM cache",
                UiMsg::Command(UiCommand::PurgeCoverCache),
            ),
            (
                "CMD",
                "> workbench",
                "Switch to Split Workbench mode (1)",
                UiMsg::SwitchMode(AppMode::Workbench),
            ),
            (
                "CMD",
                "> stage",
                "Switch to Focus Stage mode (2)",
                UiMsg::SwitchMode(AppMode::FocusStage),
            ),
            (
                "CMD",
                "> mini",
                "Switch to Mini-Player mode (3)",
                UiMsg::SwitchMode(AppMode::MiniPlayer),
            ),
            (
                "CMD",
                "> liked",
                "Open Liked Songs crate",
                UiMsg::Command(UiCommand::OpenCrate(CrateRef::LikedSongs)),
            ),
        ];

        return all_cmds
            .into_iter()
            .filter(|(_, name, desc, _)| {
                q.is_empty()
                    || name.to_ascii_lowercase().contains(&q)
                    || desc.to_ascii_lowercase().contains(&q)
            })
            .map(|(badge, name, desc, action)| OmniboxEntry {
                kind_badge: badge,
                title: name.to_string(),
                subtitle: desc.to_string(),
                action,
            })
            .collect();
    }

    let mut out = Vec::with_capacity(12);
    if !raw.is_empty() {
        out.push(OmniboxEntry {
            kind_badge: "SEARCH",
            title: format!("Search Spotify for \"{raw}\""),
            subtitle: String::from("Open full Search Results crate"),
            action: UiMsg::Command(UiCommand::RunGlobalSearch(raw.to_string())),
        });
    } else {
        out.push(OmniboxEntry {
            kind_badge: "CRATE",
            title: String::from("Liked Songs"),
            subtitle: String::from("Open saved tracks library"),
            action: UiMsg::Command(UiCommand::OpenCrate(CrateRef::LikedSongs)),
        });
        out.push(OmniboxEntry {
            kind_badge: "TIP",
            title: String::from("Type > for quick commands"),
            subtitle: String::from("> device · > shuffle · > clear queue · > settings · > purge cache"),
            action: UiMsg::OmniboxInputChanged(String::from("> ")),
        });
    }

    let q = raw.to_ascii_lowercase();
    let cv = &state.view.crate_view;

    for (_, pl) in cv.playlists.iter_loaded() {
        if out.len() >= 12 {
            break;
        }
        if q.is_empty() || pl.name.to_ascii_lowercase().contains(&q) {
            out.push(OmniboxEntry {
                kind_badge: "PLAYLIST",
                title: pl.name.clone(),
                subtitle: format!("{} · {} tracks", pl.owner, pl.track_count),
                action: UiMsg::Command(UiCommand::OpenCrate(CrateRef::Playlist {
                    uri: std::sync::Arc::clone(&pl.uri),
                    name: pl.name.clone(),
                })),
            });
        }
    }

    for (_, al) in cv.albums.iter_loaded() {
        if out.len() >= 12 {
            break;
        }
        if q.is_empty()
            || al.title.to_ascii_lowercase().contains(&q)
            || al.artist.to_ascii_lowercase().contains(&q)
        {
            out.push(OmniboxEntry {
                kind_badge: "ALBUM",
                title: al.title.clone(),
                subtitle: format!("{} · {} tracks", al.artist, al.track_count),
                action: UiMsg::Command(UiCommand::OpenCrate(CrateRef::Album {
                    uri: std::sync::Arc::clone(&al.uri),
                    title: al.title.clone(),
                })),
            });
        }
    }

    for (_, ar) in cv.artists.iter_loaded() {
        if out.len() >= 12 {
            break;
        }
        if q.is_empty() || ar.name.to_ascii_lowercase().contains(&q) {
            out.push(OmniboxEntry {
                kind_badge: "ARTIST",
                title: ar.name.clone(),
                subtitle: String::from("Followed artist"),
                action: UiMsg::Command(UiCommand::OpenCrate(CrateRef::Artist {
                    uri: std::sync::Arc::clone(&ar.uri),
                    name: ar.name.clone(),
                })),
            });
        }
    }

    out
}

/// Renders the `Ctrl+K` floating command/search palette over the workspace.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let entries = omnibox_entries(state);
    let selected = if entries.is_empty() {
        0
    } else {
        state.omnibox_selected.min(entries.len() - 1)
    };

    let search_bar = container(
        row![
            widgets::icon_svg(icons::SEARCH, 16.0, theme::ACCENT_PRIMARY),
            text_input(
                "Search Spotify catalog, jump to crate, or type > command...",
                &state.omnibox_query,
            )
            .id(id_omnibox_input())
            .on_input(UiMsg::OmniboxInputChanged)
            .on_submit(UiMsg::OmniboxSubmit)
            .font(theme::FONT_SANS)
            .size(14)
            .padding([8.0, 6.0])
            .width(Length::Fill)
            .style(theme::search_input),
            widgets::keycap("Esc"),
        ]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center),
    )
    .padding([4.0, 10.0])
    .style(theme::pill_container(
        theme::BG_CANVAS,
        theme::BORDER_STRONG,
        theme::RADIUS_MD,
    ));

    let mut list_col = column![].spacing(3);
    if entries.is_empty() {
        list_col = list_col.push(
            container(
                text("No matching local crates or commands.")
                    .size(13)
                    .color(theme::TEXT_MUTED),
            )
            .padding(12),
        );
    } else {
        for (idx, entry) in entries.into_iter().enumerate() {
            let is_sel = idx == selected;
            let badge_color = match entry.kind_badge {
                "CMD" => theme::ACCENT_PRIMARY,
                "SEARCH" => theme::STATUS_OK,
                _ => theme::TEXT_SECONDARY,
            };
            let row_btn = button(
                row![
                    widgets::status_badge(entry.kind_badge, badge_color),
                    column![
                        text(entry.title)
                            .size(13)
                            .font(theme::FONT_SANS_MEDIUM)
                            .color(theme::TEXT_PRIMARY),
                        text(entry.subtitle)
                            .size(11)
                            .font(theme::FONT_SANS)
                            .color(theme::TEXT_SECONDARY),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                ]
                .spacing(10)
                .padding(Padding::from([7.0, 10.0]))
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(0)
            .style(theme::pill_button(is_sel))
            .on_press(UiMsg::OmniboxSelect(idx));

            list_col = list_col.push(row_btn);
        }
    }

    let footer = row![
        widgets::keycap("Enter"),
        text("Open / Search").size(11).color(theme::TEXT_MUTED),
        Space::new().width(8.0),
        widgets::keycap("↑/↓"),
        text("Navigate").size(11).color(theme::TEXT_MUTED),
        Space::new().width(8.0),
        widgets::keycap(">"),
        text("Commands").size(11).color(theme::TEXT_MUTED),
        Space::new().width(Length::Fill),
        widgets::keycap("Esc"),
        text("Close").size(11).color(theme::TEXT_MUTED),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let card = mouse_area(
        container(
            column![
                search_bar,
                container(
                    scrollable(list_col)
                        .height(Length::Shrink)
                        .style(theme::dark_scrollable),
                )
                .max_height(320.0),
                widgets::hairline_h(),
                footer,
            ]
            .spacing(theme::SPACE_SM),
        )
        .width(Length::Fixed(560.0))
        .padding(14)
        .style(theme::modal_sheet),
    );

    let overlay = container(
        column![
            Space::new().height(Length::Fixed(72.0)),
            card,
        ]
        .align_x(Alignment::Center)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::modal_backdrop);

    mouse_area(overlay)
        .on_press(UiMsg::CloseOmnibox)
        .into()
}
