//! `48px` Top Command & Mode Bar (`Workbench | Stage | Mini`, history, dual-mode filter/search, devices, settings).

use iced::widget::{button, container, row, text, text_input, Space};
use iced::{Alignment, Color, Element, Length};
use pulse_assets::icons;
use pulse_model::{AppMode, AuthState, ConnectionStatus, UiCommand};

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::{UiState, id_filter_input};
use crate::widgets;

/// Renders the `48px` top command, mode switch, dual-mode omnibox trigger, and status pills.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let left = left_cluster(state);
    let center = center_omnibox_bar(state);
    let right = right_cluster(state);

    let bar = row![
        left,
        Space::new().width(Length::FillPortion(1)),
        center,
        Space::new().width(Length::FillPortion(1)),
        right,
    ]
    .spacing(theme::SPACE_MD)
    .padding([6.0, 12.0])
    .height(Length::Fixed(theme::TOP_BAR_HEIGHT))
    .align_y(Alignment::Center);

    container(
        iced::widget::column![bar, widgets::hairline_h()],
    )
    .width(Length::Fill)
    .style(theme::top_bar_container)
    .into()
}

fn left_cluster<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let active_mode = state.view.mode;
    let mode_group = container(
        row![
            mode_pill("Workbench", AppMode::Workbench, active_mode),
            mode_pill("Stage", AppMode::FocusStage, active_mode),
            mode_pill("Mini", AppMode::MiniPlayer, active_mode),
        ]
        .spacing(2),
    )
    .padding(3)
    .style(theme::pill_container(
        theme::BG_SURFACE,
        theme::BORDER_SUBTLE,
        theme::RADIUS_MD,
    ));

    let cv = &state.view.crate_view;
    let can_back = cv.history_index > 0;
    let can_forward = cv.history_index + 1 < cv.history.len();

    let back_btn = button(widgets::icon_svg(
        icons::BACK,
        14.0,
        if can_back {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_MUTED
        },
    ))
    .padding([6.0, 7.0])
    .style(theme::ghost_button)
    .on_press_maybe(can_back.then_some(UiMsg::Command(UiCommand::NavigateBack)));

    let fwd_btn = button(widgets::icon_svg(
        icons::FORWARD,
        14.0,
        if can_forward {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_MUTED
        },
    ))
    .padding([6.0, 7.0])
    .style(theme::ghost_button)
    .on_press_maybe(can_forward.then_some(UiMsg::Command(UiCommand::NavigateForward)));

    row![mode_group, row![back_btn, fwd_btn].spacing(2)]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center)
        .into()
}

fn mode_pill<'a>(label: &'a str, target: AppMode, active_mode: AppMode) -> Element<'a, UiMsg> {
    let is_active = target == active_mode;
    let text_color = if is_active {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };

    let content = iced::widget::column![
        text(label)
            .size(12)
            .font(theme::FONT_SANS_MEDIUM)
            .color(text_color),
        container(text("").size(1))
            .width(Length::Fixed(if is_active { 18.0 } else { 0.0 }))
            .height(Length::Fixed(2.0))
            .style(theme::accent_strip(if is_active {
                theme::ACCENT_PRIMARY
            } else {
                Color::TRANSPARENT
            })),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    button(content)
        .padding([4.0, 10.0])
        .style(theme::pill_button(is_active))
        .on_press(UiMsg::SwitchMode(target))
        .into()
}

fn center_omnibox_bar<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let filter = &state.view.crate_view.local_filter;

    let input = text_input("Filter crate (/) or Ctrl+K search...", filter)
        .id(id_filter_input())
        .on_input(UiMsg::FilterInputChanged)
        .on_submit(if filter.trim().is_empty() {
            UiMsg::OpenOmnibox
        } else {
            UiMsg::Command(UiCommand::RunGlobalSearch(filter.trim().to_string()))
        })
        .font(theme::FONT_SANS)
        .size(13)
        .padding([5.0, 8.0])
        .width(Length::Fill)
        .style(theme::search_input);

    let mut trailing = row![].spacing(6).align_y(Alignment::Center);

    let is_track_crate = matches!(
        state.view.crate_view.active_crate,
        pulse_model::CrateRef::LikedSongs
            | pulse_model::CrateRef::Recent
            | pulse_model::CrateRef::Playlist { .. }
            | pulse_model::CrateRef::Album { .. }
    );
    if !filter.trim().is_empty() {
        let total = state.view.crate_view.loaded_tracks.total();
        let needle = filter.trim().to_ascii_lowercase();
        let matched = state
            .view
            .crate_view
            .loaded_tracks
            .iter_loaded()
            .filter(|(_, t)| {
                t.title.to_ascii_lowercase().contains(&needle)
                    || t.artist.to_ascii_lowercase().contains(&needle)
                    || t.album.to_ascii_lowercase().contains(&needle)
            })
            .count();

        if is_track_crate {
            trailing = trailing.push(widgets::status_badge(
                format!("{matched} / {total}"),
                theme::ACCENT_PRIMARY,
            ));
        }
        trailing = trailing.push(
            button(widgets::icon_svg(icons::CLOSE, 12.0, theme::TEXT_SECONDARY))
                .padding([3.0, 5.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::ClearFilter),
        );
    } else {
        trailing = trailing.push(widgets::keycap("/"));
    }

    let omnibox_trigger = button(
        row![
            widgets::icon_svg(icons::SEARCH, 12.0, theme::TEXT_SECONDARY),
            text("Ctrl+K")
                .size(11)
                .font(theme::FONT_MONO_MEDIUM)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([4.0, 8.0])
    .style(theme::secondary_button)
    .on_press(UiMsg::OpenOmnibox);

    let mut bar_row = row![
        widgets::icon_svg(icons::SEARCH, 14.0, theme::TEXT_MUTED),
        input,
        trailing,
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Alignment::Center);
    if state.window_size.width >= 680.0 {
        bar_row = bar_row.push(omnibox_trigger);
    }

    container(bar_row)
    .width(Length::FillPortion(3))
    .max_width(520.0)
    .padding([0.0, 8.0])
    .style(theme::pill_container(
        theme::BG_CANVAS,
        theme::BORDER_SUBTLE,
        theme::RADIUS_MD,
    ))
    .into()
}

fn right_cluster<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let dev = &state.view.devices;
    let active_remote = dev
        .available_devices
        .iter()
        .find(|d| d.is_active && !d.is_local);

    let (dot_color, dev_label) = match &state.view.connection {
        ConnectionStatus::Offline { .. } | ConnectionStatus::Error(_) => {
            (theme::STATUS_ERR, String::from("Offline"))
        }
        ConnectionStatus::Connecting => (theme::STATUS_WARN, String::from("Connecting...")),
        _ => {
            if let Some(remote) = active_remote {
                (theme::STATUS_WARN, format!("{} ", remote.name))
            } else {
                let badge = &state.view.player_state.audio_badge;
                (
                    theme::STATUS_OK,
                    format!("ALSA · {}", badge),
                )
            }
        }
    };

    let compact = state.window_size.width < 680.0;
    let device_content: Element<'a, UiMsg> = if compact {
        widgets::icon_svg(icons::DEVICE, 14.0, dot_color)
    } else {
        row![
            widgets::icon_svg(icons::DEVICE, 14.0, dot_color),
            widgets::status_dot_label(dot_color, dev_label, theme::TEXT_PRIMARY),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    };
    let device_pill = button(device_content)
        .padding([5.0, if compact { 7.0 } else { 10.0 }])
        .style(theme::pill_button(state.device_popover_open))
        .on_press(UiMsg::ToggleDevicePopover);

    let rss_mb = state.view.telemetry.process_rss_mb;
    let rss_warn = rss_mb > 82.0;
    let auth_incomplete = !matches!(state.view.auth_state, AuthState::Authenticated { .. });

    let (user_label, ring_color) = match &state.view.auth_state {
        AuthState::Authenticated { display_name, .. } => {
            let label = display_name.chars().next().unwrap_or('P').to_string();
            let ring = if rss_warn {
                theme::STATUS_ERR
            } else {
                theme::BORDER_STRONG
            };
            (label, ring)
        }
        AuthState::ConnectOnly { .. } => ("C".to_string(), theme::STATUS_WARN),
        _ => ("?".to_string(), theme::STATUS_WARN),
    };

    let avatar = container(
        text(user_label)
            .size(11)
            .font(theme::FONT_MONO_SEMIBOLD)
            .color(theme::TEXT_PRIMARY),
    )
    .width(Length::Fixed(20.0))
    .height(Length::Fixed(20.0))
    .center_x(20.0)
    .center_y(20.0)
    .style(theme::pill_container(
        if rss_warn {
            Color {
                a: 0.22,
                ..theme::STATUS_ERR
            }
        } else {
            theme::BG_SURFACE
        },
        ring_color,
        999.0,
    ));

    let mut settings_row = row![
        avatar,
        widgets::icon_svg(
            icons::SETTINGS,
            14.0,
            if rss_warn {
                theme::STATUS_ERR
            } else if auth_incomplete {
                theme::STATUS_WARN
            } else {
                theme::TEXT_SECONDARY
            },
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    if rss_warn {
        settings_row = settings_row.push(
            text(format!("{rss_mb:.0}M"))
                .size(11)
                .font(theme::FONT_MONO_SEMIBOLD)
                .color(theme::STATUS_ERR),
        );
    }

    let settings_btn = button(settings_row)
        .padding([4.0, 9.0])
        .style(theme::pill_button(state.settings_open))
        .on_press(UiMsg::ToggleSettings);

    row![device_pill, settings_btn]
        .spacing(theme::SPACE_SM)
        .align_y(Alignment::Center)
        .into()
}
