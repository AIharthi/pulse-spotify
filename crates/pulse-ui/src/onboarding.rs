//! Unauthenticated First-Run Onboarding screen (`docs/UI_SPEC.md` Section 4.1 State A).

use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use iced::{Alignment, Element, Length};
use pulse_assets::icons;
use pulse_model::{AuthState, ConnectionStatus, UiCommand};

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::{UiState, id_client_id_input};
use crate::widgets;

/// Redirect URI expected when users supply a custom Spotify Developer Web API PKCE Client ID.
pub const CUSTOM_WEB_API_REDIRECT_URI: &str = "http://127.0.0.1:8898/callback";

/// Renders the centered onboarding experience over `--bg-void` / `--bg-canvas`.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let header = row![
        column![
            row![
                text("PULSE")
                    .size(22)
                    .font(theme::FONT_SANS_BOLD)
                    .color(theme::TEXT_PRIMARY),
                widgets::status_badge("Pure Rust · < 90 MB Budget", theme::ACCENT_PRIMARY),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            text("Connect your Spotify account to start listening")
                .size(14)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(4)
        .width(Length::Fill),
        button(
            row![
                widgets::icon_svg(icons::SETTINGS, 14.0, theme::TEXT_SECONDARY),
                text("Audio & Cache")
                    .size(12)
                    .font(theme::FONT_SANS_MEDIUM)
                    .color(theme::TEXT_SECONDARY),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([6.0, 10.0])
        .style(theme::secondary_button)
        .on_press(UiMsg::ToggleSettings),
    ]
    .align_y(Alignment::Center);

    let option1 = oauth_card(state);
    let option2 = zeroconf_card(state);

    let notice_or_err: Option<Element<'a, UiMsg>> = match &state.view.connection {
        ConnectionStatus::Error(err)
            if state.dismissed_conn_error.as_deref() != Some(err.as_str()) =>
        {
            Some(status_strip(err, true))
        }
        _ => state
            .view
            .notice_banner
            .as_deref()
            .map(|msg| status_strip(msg, false)),
    };

    let premium_footer = container(
        row![
            widgets::status_badge("PREMIUM", theme::STATUS_WARN),
            text("Audio playback via librespot and Spotify Connect require Spotify Premium.")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8.0, 12.0])
    .width(Length::Fill)
    .style(theme::surface_card);

    let mut stack_col = column![header, option1, option2, premium_footer].spacing(theme::SPACE_MD);
    if let Some(strip) = notice_or_err {
        stack_col = stack_col.push(strip);
    }

    let card = container(stack_col)
        .width(Length::Fill)
        .max_width(580.0)
        .padding(20)
        .style(theme::modal_sheet);

    container(
        scrollable(
            container(card)
                .width(Length::Fill)
                .padding([28.0, 20.0])
                .center_x(Length::Fill),
        )
        .style(theme::dark_scrollable),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_y(Length::Fill)
    .style(theme::void_container)
    .into()
}

fn oauth_card<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let custom_id = trimmed_opt(&state.settings_client_id_draft);

    let auth_controls: Element<'a, UiMsg> = match &state.view.auth_state {
        AuthState::WaitingOAuth { auth_url } => {
            column![
                row![
                    container(
                        widgets::status_dot_label(
                            theme::STATUS_WARN,
                            "Waiting for Spotify browser callback...",
                            theme::TEXT_PRIMARY,
                        ),
                    )
                    .padding([8.0, 12.0])
                    .width(Length::Fill)
                    .style(theme::elevated_card),
                    button(
                        text("Cancel")
                            .size(13)
                            .font(theme::FONT_SANS_MEDIUM)
                            .color(theme::STATUS_ERR),
                    )
                    .padding([8.0, 14.0])
                    .style(theme::danger_button)
                    .on_press(UiMsg::Command(UiCommand::CancelOAuthLogin)),
                ]
                .spacing(theme::SPACE_SM)
                .align_y(Alignment::Center),
                container(
                    row![
                        text(auth_url.clone())
                            .size(11)
                            .font(theme::FONT_MONO)
                            .color(theme::TEXT_SECONDARY)
                            .width(Length::Fill),
                        button(
                            row![
                                widgets::icon_svg(icons::EXTERNAL_LINK, 13.0, theme::ACCENT_PRIMARY),
                                text("Copy URL")
                                    .size(11)
                                    .font(theme::FONT_MONO_MEDIUM)
                                    .color(theme::ACCENT_PRIMARY),
                            ]
                            .spacing(5)
                            .align_y(Alignment::Center),
                        )
                        .padding([4.0, 8.0])
                        .style(theme::secondary_button)
                        .on_press(UiMsg::CopyToClipboard(auth_url.clone())),
                    ]
                    .spacing(theme::SPACE_SM)
                    .align_y(Alignment::Center),
                )
                .padding([8.0, 10.0])
                .width(Length::Fill)
                .style(theme::surface_card),
            ]
            .spacing(theme::SPACE_SM)
            .into()
        }
        _ => button(
            container(
                row![
                    widgets::icon_svg(icons::EXTERNAL_LINK, 15.0, theme::ACCENT_TEXT_ON),
                    text("Authorize with Spotify (OAuth Browser)")
                        .size(13)
                        .font(theme::FONT_SANS_SEMIBOLD)
                        .color(theme::ACCENT_TEXT_ON),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        )
        .width(Length::Fill)
        .padding([10.0, 16.0])
        .style(theme::primary_button)
        .on_press(UiMsg::Command(UiCommand::StartOAuthLogin {
            client_id: custom_id.clone(),
        }))
        .into(),
    };

    let client_id_input = row![
        text_input(
            "Optional Spotify Developer Client ID (PKCE)",
            &state.settings_client_id_draft,
        )
        .id(id_client_id_input())
        .on_input(UiMsg::SettingsClientIdChanged)
        .on_submit(UiMsg::Command(UiCommand::SaveClientId(custom_id.clone())))
        .font(theme::FONT_MONO)
        .size(12)
        .padding([6.0, 8.0])
        .width(Length::Fill)
        .style(theme::search_input),
        button(
            text("Save")
                .size(12)
                .font(theme::FONT_SANS_MEDIUM)
                .color(theme::TEXT_PRIMARY),
        )
        .padding([6.0, 12.0])
        .style(theme::secondary_button)
        .on_press(UiMsg::Command(UiCommand::SaveClientId(custom_id))),
    ]
    .spacing(theme::SPACE_SM)
    .align_y(Alignment::Center);

    let redirect_row = row![
        text("Redirect URI:")
            .size(11)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_MUTED),
        text(CUSTOM_WEB_API_REDIRECT_URI)
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_SECONDARY)
            .width(Length::Fill),
        button(
            text("Copy URI")
                .size(11)
                .font(theme::FONT_MONO_MEDIUM)
                .color(theme::ACCENT_PRIMARY),
        )
        .padding([3.0, 8.0])
        .style(theme::secondary_button)
        .on_press(UiMsg::CopyToClipboard(String::from(
            CUSTOM_WEB_API_REDIRECT_URI,
        ))),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        column![
            widgets::section_label("OPTION 1 · FULL ACCOUNT & LIBRARY (RECOMMENDED)"),
            text("Sign in via Spotify OAuth to browse Liked Songs, Playlists, Saved Albums, Artists & Search. Uses built-in KEYMASTER_CLIENT_ID by default (127.0.0.1:5588/login); optional PKCE Client ID adds direct Spotify Web API search/catalog access.")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
            auth_controls,
            widgets::hairline_h(),
            text("Custom Web API Client ID (Optional · PKCE, no client secret)")
                .size(12)
                .font(theme::FONT_SANS_MEDIUM)
                .color(theme::TEXT_PRIMARY),
            client_id_input,
            redirect_row,
        ]
        .spacing(theme::SPACE_SM),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card)
    .into()
}

fn zeroconf_card<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let dev_name = if state.view.devices.local_device_name.is_empty() {
        &state.view.config.device_name
    } else {
        &state.view.devices.local_device_name
    };
    let zc_active = state.view.devices.zeroconf_active;
    let dot_color = if zc_active {
        theme::STATUS_OK
    } else {
        theme::TEXT_MUTED
    };

    let status_line = if zc_active {
        format!("Listening via mDNS (with-libmdns) as \"{dev_name}\"")
    } else {
        format!("Zeroconf paused (configured name: \"{dev_name}\")")
    };

    container(
        column![
            row![
                widgets::section_label("OPTION 2 · ZERO-CONFIG SPOTIFY CONNECT (LAN PAIRING)"),
                Space::new().width(Length::Fill),
                button(
                    text(if zc_active { "Disable" } else { "Enable" })
                        .size(11)
                        .font(theme::FONT_MONO_MEDIUM)
                        .color(theme::TEXT_SECONDARY),
                )
                .padding([3.0, 8.0])
                .style(theme::secondary_button)
                .on_press(UiMsg::Command(UiCommand::SetZeroconf {
                    enabled: !zc_active,
                    device_name: dev_name.to_string(),
                })),
            ]
            .align_y(Alignment::Center),
            widgets::status_dot_label(dot_color, status_line, theme::TEXT_PRIMARY),
            text("Open Spotify on phone or desktop -> Devices -> select Pulse. Pairing caches reusable credentials locally (0600) for future launches.")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
        ]
        .spacing(6),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card)
    .into()
}

fn status_strip<'a>(msg: &str, is_error: bool) -> Element<'a, UiMsg> {
    container(
        row![
            text(msg.to_string())
                .size(12)
                .font(theme::FONT_SANS)
                .color(if is_error {
                    theme::STATUS_ERR
                } else {
                    theme::STATUS_WARN
                })
                .width(Length::Fill),
            button(widgets::icon_svg(icons::CLOSE, 12.0, theme::TEXT_SECONDARY))
                .padding([2.0, 4.0])
                .style(theme::ghost_button)
                .on_press(UiMsg::DismissNotice),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([8.0, 12.0])
    .width(Length::Fill)
    .style(theme::banner_container(is_error))
    .into()
}

fn trimmed_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
