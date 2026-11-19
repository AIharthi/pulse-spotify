//! Centered `640x540` Settings Sheet (`Account & Connect`, `Audio & Desktop`, `Memory & Cache`).

use iced::widget::{
    button, column, container, mouse_area, progress_bar, row, scrollable, text, text_input,
    toggler, Space,
};
use iced::{Alignment, Background, Color, Element, Length};
use pulse_assets::icons;
use pulse_model::{
    AccountTier, AuthState, NormalisationMode, SettingsTab, StreamQuality, UiCommand,
};

use crate::message::UiMsg;
use crate::onboarding::CUSTOM_WEB_API_REDIRECT_URI;
use crate::theme;
use crate::ui_state::{UiState, id_client_id_input, id_device_name_input};
use crate::widgets;

/// Renders the centered 3-tab Settings modal sheet over a dark backdrop.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let tabs = row![
        tab_pill("Account & Connect", SettingsTab::AccountAndConnect, state.settings_tab),
        tab_pill("Audio & Desktop", SettingsTab::AudioAndDesktop, state.settings_tab),
        tab_pill("Memory & Cache", SettingsTab::MemoryAndCache, state.settings_tab),
    ]
    .spacing(6);

    let header = row![
        text("Pulse Settings")
            .size(16)
            .font(theme::FONT_SANS_SEMIBOLD)
            .color(theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        tabs,
        Space::new().width(10.0),
        button(widgets::icon_svg(icons::CLOSE, 14.0, theme::TEXT_SECONDARY))
            .padding([5.0, 7.0])
            .style(theme::ghost_button)
            .on_press(UiMsg::ToggleSettings),
    ]
    .align_y(Alignment::Center);

    let body = match state.settings_tab {
        SettingsTab::AccountAndConnect => account_and_connect_tab(state),
        SettingsTab::AudioAndDesktop => audio_and_desktop_tab(state),
        SettingsTab::MemoryAndCache => memory_and_cache_tab(state),
    };

    let sheet = mouse_area(
        container(
            column![
                header,
                widgets::hairline_h(),
                scrollable(container(body).width(Length::Fill).padding([4.0, 2.0]))
                    .height(Length::Fill)
                    .style(theme::dark_scrollable),
            ]
            .spacing(theme::SPACE_MD),
        )
        .width(Length::Fill)
        .max_width(640.0)
        .height(Length::Fill)
        .max_height(540.0)
        .padding(18)
        .style(theme::modal_sheet),
    )
    .on_press(UiMsg::Noop);

    let backdrop = container(sheet)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(theme::modal_backdrop);

    mouse_area(backdrop)
        .on_press(UiMsg::ToggleSettings)
        .into()
}

fn tab_pill<'a>(label: &'a str, tab: SettingsTab, active: SettingsTab) -> Element<'a, UiMsg> {
    button(
        text(label)
            .size(12)
            .font(theme::FONT_SANS_MEDIUM)
            .color(if tab == active {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_SECONDARY
            }),
    )
    .padding([5.0, 10.0])
    .style(theme::pill_button(tab == active))
    .on_press(UiMsg::SetSettingsTab(tab))
    .into()
}

fn account_and_connect_tab<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let custom_id = trimmed_opt(&state.settings_client_id_draft);

    let (identity_line, tier_badge, web_api_line, is_waiting) = match &state.view.auth_state {
        AuthState::Authenticated {
            username,
            display_name,
            tier,
            web_api_ready,
            zeroconf_only,
        } => {
            let tier_el = match tier {
                AccountTier::Premium => widgets::status_badge("PREMIUM", theme::STATUS_OK),
                AccountTier::Free => widgets::status_badge("FREE TIER", theme::STATUS_WARN),
                AccountTier::Unknown => widgets::status_badge("TIER UNKNOWN", theme::TEXT_SECONDARY),
            };
            let web_text = if *web_api_ready {
                "librespot login5: Active · Web API token: Ready"
            } else if *zeroconf_only {
                "Paired via Zeroconf Connect (sign in with OAuth for full library)"
            } else {
                "librespot login5: Active"
            };
            (
                format!("{display_name} ({username})"),
                tier_el,
                String::from(web_text),
                false,
            )
        }
        AuthState::ConnectOnly { device_name } => (
            format!("Connect-Only ({device_name})"),
            widgets::status_badge("CONNECT ONLY", theme::STATUS_WARN),
            String::from("Sign in via OAuth below to unlock full library browsing & search."),
            false,
        ),
        AuthState::WaitingOAuth { .. } => (
            String::from("OAuth Login in Progress"),
            widgets::status_badge("WAITING CALLBACK", theme::STATUS_WARN),
            String::from("Complete authorization in your browser..."),
            true,
        ),
        AuthState::Unauthenticated => (
            String::from("Not signed in"),
            widgets::status_badge("SIGNED OUT", theme::TEXT_MUTED),
            String::from("Authorize via OAuth or pair via Spotify Connect on your LAN."),
            false,
        ),
    };

    let auth_actions = if is_waiting {
        row![
            button(text("Cancel OAuth").size(12).color(theme::STATUS_ERR))
                .padding([6.0, 12.0])
                .style(theme::danger_button)
                .on_press(UiMsg::Command(UiCommand::CancelOAuthLogin)),
        ]
        .spacing(8)
    } else {
        row![
            button(
                text("Re-authenticate OAuth")
                    .size(12)
                    .font(theme::FONT_SANS_MEDIUM)
                    .color(theme::ACCENT_TEXT_ON),
            )
            .padding([6.0, 12.0])
            .style(theme::primary_button)
            .on_press(UiMsg::Command(UiCommand::StartOAuthLogin {
                client_id: custom_id.clone(),
            })),
            button(
                text("Sign Out & Forget")
                    .size(12)
                    .font(theme::FONT_SANS_MEDIUM),
            )
            .padding([6.0, 12.0])
            .style(theme::danger_button)
            .on_press(UiMsg::Command(UiCommand::SignOut)),
        ]
        .spacing(8)
    };

    let identity_card = container(
        column![
            row![
                widgets::section_label("ACCOUNT IDENTITY"),
                Space::new().width(Length::Fill),
                tier_badge,
            ]
            .align_y(Alignment::Center),
            text(identity_line)
                .size(14)
                .font(theme::FONT_SANS_SEMIBOLD)
                .color(theme::TEXT_PRIMARY),
            text(web_api_line)
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
            auth_actions,
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let client_id_card = container(
        column![
            widgets::section_label("SPOTIFY WEB API CLIENT ID (OPTIONAL PKCE)"),
            row![
                text_input(
                    "Default: KEYMASTER_CLIENT_ID (optional custom Client ID)",
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
                button(text("Save ID").size(12).font(theme::FONT_SANS_MEDIUM))
                    .padding([6.0, 12.0])
                    .style(theme::secondary_button)
                    .on_press(UiMsg::Command(UiCommand::SaveClientId(custom_id))),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            row![
                text(format!("PKCE Redirect URI: {CUSTOM_WEB_API_REDIRECT_URI}"))
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
            .align_y(Alignment::Center),
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let cfg = &state.view.config;
    let draft_name = if state.settings_device_name_draft.trim().is_empty() {
        cfg.device_name.clone()
    } else {
        state.settings_device_name_draft.trim().to_string()
    };

    let mut connect_col = column![
        row![
            widgets::section_label("SPOTIFY CONNECT (ZEROCONF mDNS)"),
            Space::new().width(Length::Fill),
            widgets::status_dot_label(
                if state.view.devices.zeroconf_active {
                    theme::STATUS_OK
                } else {
                    theme::TEXT_MUTED
                },
                if state.view.devices.zeroconf_active {
                    "with-libmdns active"
                } else {
                    "Disabled"
                },
                theme::TEXT_SECONDARY,
            ),
        ]
        .align_y(Alignment::Center),
        toggler(cfg.zeroconf_enabled)
            .label("Advertise this player on the local network via mDNS")
            .on_toggle({
                let name = draft_name.clone();
                move |enabled| {
                    UiMsg::Command(UiCommand::SetZeroconf {
                        enabled,
                        device_name: name.clone(),
                    })
                }
            })
            .text_size(13)
            .font(theme::FONT_SANS)
            .style(dark_toggler),
        row![
            text("Device Name:")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
            text_input("Pulse", &state.settings_device_name_draft)
                .id(id_device_name_input())
                .on_input(UiMsg::SettingsDeviceNameChanged)
                .on_submit(UiMsg::Command(UiCommand::SetZeroconf {
                    enabled: cfg.zeroconf_enabled,
                    device_name: draft_name.clone(),
                }))
                .font(theme::FONT_SANS)
                .size(12)
                .padding([5.0, 8.0])
                .width(Length::Fill)
                .style(theme::search_input),
            button(text("Apply").size(12).font(theme::FONT_SANS_MEDIUM))
                .padding([5.0, 12.0])
                .style(theme::secondary_button)
                .on_press(UiMsg::Command(UiCommand::SetZeroconf {
                    enabled: cfg.zeroconf_enabled,
                    device_name: draft_name,
                })),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(8);

    for d in &state.view.devices.available_devices {
        if !d.is_local {
            connect_col = connect_col.push(
                row![
                    text(format!("{} ({})", d.name, d.device_type))
                        .size(12)
                        .color(if d.is_active {
                            theme::ACCENT_PRIMARY
                        } else {
                            theme::TEXT_PRIMARY
                        })
                        .width(Length::Fill),
                    button(text(if d.is_active { "Active" } else { "Transfer" }).size(11))
                        .padding([3.0, 9.0])
                        .style(theme::secondary_button)
                        .on_press_maybe((!d.is_active && !d.is_restricted).then(|| {
                            UiMsg::Command(UiCommand::TransferPlayback {
                                device_id: d.id.clone(),
                            })
                        })),
                ]
                .align_y(Alignment::Center),
            );
        }
    }

    let connect_card = container(connect_col)
        .padding(14)
        .width(Length::Fill)
        .style(theme::surface_card);

    column![identity_card, client_id_card, connect_card]
        .spacing(theme::SPACE_MD)
        .into()
}

fn audio_and_desktop_tab<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let cfg = &state.view.config;

    let sink_pills = row![
        sink_pill("System Default", None, cfg.audio_device.as_deref()),
        sink_pill("pipewire", Some("pipewire"), cfg.audio_device.as_deref()),
        sink_pill("pulse", Some("pulse"), cfg.audio_device.as_deref()),
    ]
    .spacing(6);

    let backend_card = container(
        column![
            widgets::section_label("AUDIO OUTPUT BACKEND (PURE RUST ALSA)"),
            text("ALSA via PipeWire-ALSA (lazy PCM open on playback, releases shared memory on stop).")
                .size(12)
                .font(theme::FONT_SANS)
                .color(theme::TEXT_SECONDARY),
            sink_pills,
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let quality_pills = row![
        quality_pill("96 kbps", StreamQuality::Kbps96, cfg.stream_quality),
        quality_pill("160 kbps", StreamQuality::Kbps160, cfg.stream_quality),
        quality_pill("320 kbps", StreamQuality::Kbps320, cfg.stream_quality),
    ]
    .spacing(6);

    let norm_pills = row![
        norm_pill("Off", NormalisationMode::Off, cfg.normalisation),
        norm_pill("Track", NormalisationMode::Track, cfg.normalisation),
        norm_pill("Album", NormalisationMode::Album, cfg.normalisation),
    ]
    .spacing(6);

    let playback_card = container(
        column![
            widgets::section_label("STREAMING QUALITY & PLAYBACK"),
            row![
                text("Ogg Vorbis Bitrate")
                    .size(13)
                    .color(theme::TEXT_PRIMARY)
                    .width(Length::Fill),
                quality_pills,
            ]
            .align_y(Alignment::Center),
            row![
                text("Volume Normalisation")
                    .size(13)
                    .color(theme::TEXT_PRIMARY)
                    .width(Length::Fill),
                norm_pills,
            ]
            .align_y(Alignment::Center),
            toggler(cfg.gapless)
                .label("Gapless track transitions")
                .on_toggle(|v| UiMsg::Command(UiCommand::SetGapless(v)))
                .text_size(13)
                .font(theme::FONT_SANS)
                .style(dark_toggler),
        ]
        .spacing(10),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let desktop_card = container(
        column![
            widgets::section_label("DESKTOP INTEGRATION (MPRIS2 & QUEUE)"),
            toggler(cfg.mpris_enabled)
                .label("Expose MPRIS2 D-Bus media player controls (Play/Pause/Next/Metadata)")
                .on_toggle(|v| UiMsg::Command(UiCommand::SetMprisEnabled(v)))
                .text_size(13)
                .font(theme::FONT_SANS)
                .style(dark_toggler),
            toggler(cfg.confirm_clear_queue)
                .label("Confirm before clearing manual priority queue")
                .on_toggle(|v| UiMsg::Command(UiCommand::SetConfirmClearQueue(v)))
                .text_size(13)
                .font(theme::FONT_SANS)
                .style(dark_toggler),
        ]
        .spacing(10),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    column![backend_card, playback_card, desktop_card]
        .spacing(theme::SPACE_MD)
        .into()
}

fn memory_and_cache_tab<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let t = &state.view.telemetry;
    let rss = t.process_rss_mb;
    let status_col = theme::rss_status_color(rss);
    let status_label = if rss > 82.0 {
        "NEAR CAP"
    } else if rss >= 65.0 {
        "ELEVATED"
    } else {
        "WITHIN BUDGET"
    };

    let rss_card = container(
        column![
            row![
                widgets::section_label("LIVE PROCESS RSS TELEMETRY (/proc/self/status)"),
                Space::new().width(Length::Fill),
                widgets::status_badge(status_label, status_col),
            ]
            .align_y(Alignment::Center),
            row![
                text(format!("{rss:.1} MB / 90.0 MB ceiling"))
                    .size(15)
                    .font(theme::FONT_MONO_SEMIBOLD)
                    .color(status_col)
                    .width(Length::Fill),
                text(format!("Peak HWM: {:.1} MB", t.process_hwm_mb))
                    .size(12)
                    .font(theme::FONT_MONO)
                    .color(theme::TEXT_SECONDARY),
            ]
            .align_y(Alignment::Center),
            progress_bar(0.0..=90.0, rss.clamp(0.0, 90.0))
                .girth(10)
                .style(theme::rss_progress_bar(rss)),
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let breakdown_card = container(
        column![
            widgets::section_label("RAM SUBSYSTEM BREAKDOWN"),
            metric_row(
                "Decoded Cover Art RAM",
                format!(
                    "{:.2} / 4.00 MiB raw ({}/{} thumbs · {}/{} heroes · UI handles {}t/{}h)",
                    t.cover_cache_mb,
                    t.cover_thumb_slots_used,
                    t.cover_thumb_slots_cap.max(32),
                    t.cover_hero_slots_used,
                    t.cover_hero_slots_cap.max(2),
                    state.covers.thumb_count(),
                    state.covers.hero_count(),
                ),
            ),
            metric_row(
                "Loaded Track Metadata Records",
                format!(
                    "{} tracks ({} in active crate window)",
                    t.loaded_track_records,
                    state.view.crate_view.loaded_tracks.loaded_count()
                ),
            ),
            metric_row(
                "Engine Runtime",
                String::from("iced tiny-skia CPU + SingleThreadTokio + MALLOC_ARENA_MAX=2"),
            ),
            row![
                Space::new().width(Length::Fill),
                button(
                    text("Purge Cover RAM")
                        .size(12)
                        .font(theme::FONT_SANS_MEDIUM),
                )
                .padding([6.0, 12.0])
                .style(theme::secondary_button)
                .on_press(UiMsg::Command(UiCommand::PurgeCoverCache)),
            ],
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    let current_cap = state.view.config.disk_audio_cache_mb;
    let cap_pills = row![
        cache_cap_pill("Off", 0, current_cap),
        cache_cap_pill("128 MB", 128, current_cap),
        cache_cap_pill("256 MB", 256, current_cap),
        cache_cap_pill("512 MB", 512, current_cap),
        cache_cap_pill("1 GB", 1024, current_cap),
    ]
    .spacing(6);

    let disk_card = container(
        column![
            widgets::section_label("DISK-BACKED AUDIO & COVER CACHES"),
            row![
                text("Audio Cache Limit")
                    .size(13)
                    .color(theme::TEXT_PRIMARY)
                    .width(Length::Fill),
                cap_pills,
            ]
            .align_y(Alignment::Center),
            metric_row(
                "Current Disk Footprint",
                format!(
                    "Audio: {:.1} MB · Covers: {:.1} MB",
                    t.disk_audio_cache_mb, t.disk_cover_cache_mb
                ),
            ),
            row![
                Space::new().width(Length::Fill),
                button(
                    text("Clear Disk Audio Cache")
                        .size(12)
                        .font(theme::FONT_SANS_MEDIUM),
                )
                .padding([6.0, 12.0])
                .style(theme::danger_button)
                .on_press(UiMsg::Command(UiCommand::ClearAudioCache)),
            ],
        ]
        .spacing(8),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::surface_card);

    column![rss_card, breakdown_card, disk_card]
        .spacing(theme::SPACE_MD)
        .into()
}

fn metric_row<'a>(label: &'a str, value: String) -> Element<'a, UiMsg> {
    row![
        text(label)
            .size(12)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_SECONDARY)
            .width(Length::Fill),
        text(value)
            .size(12)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_PRIMARY),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn sink_pill<'a>(
    label: &'a str,
    target: Option<&'static str>,
    current: Option<&str>,
) -> Element<'a, UiMsg> {
    let active = target == current;
    button(text(label).size(12).font(theme::FONT_MONO_MEDIUM))
        .padding([5.0, 10.0])
        .style(theme::pill_button(active))
        .on_press(UiMsg::Command(UiCommand::SetAudioDevice(
            target.map(String::from),
        )))
        .into()
}

fn quality_pill<'a>(
    label: &'a str,
    quality: StreamQuality,
    current: StreamQuality,
) -> Element<'a, UiMsg> {
    button(text(label).size(12).font(theme::FONT_MONO_MEDIUM))
        .padding([5.0, 10.0])
        .style(theme::pill_button(quality == current))
        .on_press(UiMsg::Command(UiCommand::SetStreamQuality(quality)))
        .into()
}

fn norm_pill<'a>(
    label: &'a str,
    mode: NormalisationMode,
    current: NormalisationMode,
) -> Element<'a, UiMsg> {
    button(text(label).size(12).font(theme::FONT_SANS_MEDIUM))
        .padding([5.0, 10.0])
        .style(theme::pill_button(mode == current))
        .on_press(UiMsg::Command(UiCommand::SetNormalisation(mode)))
        .into()
}

fn cache_cap_pill<'a>(label: &'a str, mb: u64, current: u64) -> Element<'a, UiMsg> {
    button(text(label).size(11).font(theme::FONT_MONO_MEDIUM))
        .padding([4.0, 9.0])
        .style(theme::pill_button(mb == current))
        .on_press(UiMsg::Command(UiCommand::SetDiskCacheLimitMb(mb)))
        .into()
}

fn dark_toggler(_theme: &iced::Theme, status: toggler::Status) -> toggler::Style {
    let (is_toggled, is_hovered) = match status {
        toggler::Status::Active { is_toggled } => (is_toggled, false),
        toggler::Status::Hovered { is_toggled } => (is_toggled, true),
        toggler::Status::Disabled { is_toggled } => (is_toggled, false),
    };
    toggler::Style {
        background: Background::Color(if is_toggled {
            if is_hovered {
                theme::ACCENT_HOVER
            } else {
                theme::ACCENT_PRIMARY
            }
        } else {
            theme::BG_CANVAS
        }),
        background_border_width: 1.0,
        background_border_color: if is_toggled {
            Color::TRANSPARENT
        } else if is_hovered {
            theme::BORDER_STRONG
        } else {
            theme::BORDER_SUBTLE
        },
        foreground: Background::Color(if is_toggled {
            theme::BG_VOID
        } else {
            theme::TEXT_SECONDARY
        }),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(theme::TEXT_PRIMARY),
        border_radius: None,
        padding_ratio: 0.14,
    }
}

fn trimmed_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
