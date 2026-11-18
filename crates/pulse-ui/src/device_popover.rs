//! Top-right anchored `300px` Playback Device Switcher popover (`D`).

use iced::widget::{button, column, container, mouse_area, row, scrollable, text, Space};
use iced::{Alignment, Element, Length, Padding};
use pulse_assets::icons;
use pulse_model::UiCommand;

use crate::message::UiMsg;
use crate::theme;
use crate::ui_state::UiState;
use crate::widgets;

/// Renders the anchored Spotify Connect & PipeWire/ALSA device switcher popover.
pub fn view<'a>(state: &'a UiState) -> Element<'a, UiMsg> {
    let devices = &state.view.devices;
    let local_active = devices
        .active_device_id
        .as_deref()
        .map(|id| id == devices.local_device_id)
        .unwrap_or(true);

    let mut list = column![].spacing(4);

    // Local ALSA / PipeWire engine target
    let local_name = if devices.local_device_name.is_empty() {
        &state.view.config.device_name
    } else {
        &devices.local_device_name
    };
    let local_row = device_row(
        local_name,
        &format!(
            "This Computer · ALSA / PipeWire {}",
            state.view.player_state.audio_badge
        ),
        local_active,
        state.view.player_state.volume_pct(),
        (!local_active && !devices.local_device_id.is_empty()).then(|| {
            UiMsg::Command(UiCommand::TransferPlayback {
                device_id: devices.local_device_id.clone(),
            })
        }),
    );
    list = list.push(local_row);

    let remote_devices: Vec<_> = devices
        .available_devices
        .iter()
        .filter(|d| !d.is_local && d.id != devices.local_device_id)
        .collect();

    if !remote_devices.is_empty() {
        list = list.push(Space::new().height(4.0));
        list = list.push(widgets::section_label("SPOTIFY CONNECT DEVICES"));
        for d in remote_devices {
            let msg = if d.is_active || d.is_restricted {
                None
            } else {
                Some(UiMsg::Command(UiCommand::TransferPlayback {
                    device_id: d.id.clone(),
                }))
            };
            let subtitle = if d.is_restricted {
                format!("{} · Restricted", d.device_type)
            } else {
                d.device_type.clone()
            };
            list = list.push(device_row(
                &d.name,
                &subtitle,
                d.is_active,
                d.volume_pct(),
                msg,
            ));
        }
    } else {
        list = list.push(
            container(
                text("No other active Spotify Connect devices detected.")
                    .size(12)
                    .font(theme::FONT_SANS)
                    .color(theme::TEXT_MUTED),
            )
            .padding([8.0, 6.0]),
        );
    }

    let header = row![
        text("Playback Devices")
            .size(13)
            .font(theme::FONT_SANS_SEMIBOLD)
            .color(theme::TEXT_PRIMARY),
        Space::new().width(Length::Fill),
        button(
            text("Refresh")
                .size(11)
                .font(theme::FONT_MONO_MEDIUM)
                .color(theme::TEXT_SECONDARY),
        )
        .padding([3.0, 8.0])
        .style(theme::secondary_button)
        .on_press(UiMsg::Command(UiCommand::RefreshDevices)),
    ]
    .align_y(Alignment::Center);

    let zc_color = if devices.zeroconf_active {
        theme::STATUS_OK
    } else {
        theme::TEXT_MUTED
    };
    let zc_text = if devices.zeroconf_active {
        "Zeroconf mDNS: Active"
    } else {
        "Zeroconf mDNS: Off"
    };

    let footer = row![
        widgets::status_dot_label(zc_color, zc_text, theme::TEXT_SECONDARY),
        Space::new().width(Length::Fill),
        button(
            text("Settings (Ctrl+,)")
                .size(11)
                .font(theme::FONT_MONO_MEDIUM)
                .color(theme::ACCENT_PRIMARY),
        )
        .padding([2.0, 6.0])
        .style(theme::ghost_button)
        .on_press(UiMsg::ToggleSettings),
    ]
    .align_y(Alignment::Center);

    let panel = mouse_area(
        container(
            column![
                header,
                widgets::hairline_h(),
                scrollable(list)
                    .height(Length::Shrink)
                    .style(theme::dark_scrollable),
                widgets::hairline_h(),
                footer,
            ]
            .spacing(theme::SPACE_SM),
        )
        .width(Length::Fixed(300.0))
        .max_height(380.0)
        .padding(12)
        .style(theme::popover_panel),
    );

    let anchored = container(
        column![
            Space::new().height(Length::Fixed(theme::TOP_BAR_HEIGHT + 6.0)),
            row![
                Space::new().width(Length::Fill),
                panel,
                Space::new().width(Length::Fixed(12.0)),
            ],
        ],
    )
    .width(Length::Fill)
    .height(Length::Fill);

    mouse_area(anchored)
        .on_press(UiMsg::ClosePopovers)
        .into()
}

fn device_row<'a>(
    name: &str,
    subtitle: &str,
    is_active: bool,
    volume_pct: u8,
    on_press: Option<UiMsg>,
) -> Element<'a, UiMsg> {
    let dot_color = if is_active {
        theme::STATUS_OK
    } else {
        theme::TEXT_MUTED
    };

    let label_col = column![
        text(name.to_string())
            .size(13)
            .font(theme::FONT_SANS_MEDIUM)
            .color(if is_active {
                theme::ACCENT_PRIMARY
            } else {
                theme::TEXT_PRIMARY
            }),
        text(subtitle.to_string())
            .size(11)
            .font(theme::FONT_SANS)
            .color(theme::TEXT_SECONDARY),
    ]
    .spacing(1)
    .width(Length::Fill);

    let trailing: Element<'a, UiMsg> = if is_active {
        row![
            text(format!("{volume_pct}%"))
                .size(11)
                .font(theme::FONT_MONO)
                .color(theme::TEXT_SECONDARY),
            widgets::icon_svg(icons::CHECK, 14.0, theme::STATUS_OK),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    } else if on_press.is_some() {
        text("Transfer")
            .size(11)
            .font(theme::FONT_MONO_MEDIUM)
            .color(theme::TEXT_MUTED)
            .into()
    } else {
        text("Busy")
            .size(11)
            .font(theme::FONT_MONO)
            .color(theme::TEXT_MUTED)
            .into()
    };

    let row_body = row![
        widgets::icon_svg(icons::DEVICE, 15.0, dot_color),
        label_col,
        trailing,
    ]
    .spacing(theme::SPACE_SM)
    .padding(Padding::from([6.0, 8.0]))
    .align_y(Alignment::Center);

    button(row_body)
        .width(Length::Fill)
        .padding(0)
        .style(theme::pill_button(is_active))
        .on_press_maybe(on_press)
        .into()
}
