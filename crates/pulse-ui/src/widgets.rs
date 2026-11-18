//! Reusable Cool Obsidian UI primitives, cover frames, badges, keycaps, and time formatting.

use iced::widget::{container, image, row, svg, text};
use iced::{Alignment, Color, ContentFit, Element, Length};

use crate::theme;

/// Renders a monochrome SVG icon from static memory bytes tinted with `color`.
pub fn icon_svg<'a, Message: 'a>(
    bytes: &'static [u8],
    size: f32,
    color: Color,
) -> Element<'a, Message> {
    svg(svg::Handle::from_memory(bytes))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(theme::svg_colored(color))
        .into()
}

/// Renders a square cover thumbnail (`size x size`) with a dark bordered placeholder on cache miss.
pub fn cover_thumb<'a, Message: 'a>(
    handle: Option<&'a image::Handle>,
    size: f32,
    fallback_icon: Option<&'static [u8]>,
) -> Element<'a, Message> {
    match handle {
        Some(h) => image(h)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .content_fit(ContentFit::Cover)
            .into(),
        None => {
            let inner: Element<'a, Message> = match fallback_icon {
                Some(bytes) => icon_svg(bytes, (size * 0.45).clamp(12.0, 24.0), theme::TEXT_MUTED),
                None => text("")
                    .size(10)
                    .font(theme::FONT_MONO)
                    .into(),
            };
            container(inner)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .center_x(Length::Fixed(size))
                .center_y(Length::Fixed(size))
                .style(theme::cover_placeholder(theme::RADIUS_SM))
                .into()
        }
    }
}

/// Renders a large Stage / Focus hero sleeve with optional rounded placeholder border.
pub fn cover_hero<'a, Message: 'a>(
    handle: Option<&'a image::Handle>,
    size: f32,
    radius: f32,
    fallback_icon: Option<&'static [u8]>,
) -> Element<'a, Message> {
    match handle {
        Some(h) => image(h)
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .content_fit(ContentFit::Cover)
            .into(),
        None => {
            let inner: Element<'a, Message> = match fallback_icon {
                Some(bytes) => icon_svg(bytes, (size * 0.24).clamp(28.0, 64.0), theme::TEXT_MUTED),
                None => text("PULSE")
                    .size(12)
                    .font(theme::FONT_MONO_SEMIBOLD)
                    .color(theme::TEXT_MUTED)
                    .into(),
            };
            container(inner)
                .width(Length::Fixed(size))
                .height(Length::Fixed(size))
                .center_x(Length::Fixed(size))
                .center_y(Length::Fixed(size))
                .style(theme::cover_placeholder(radius))
                .into()
        }
    }
}

/// Formats millisecond durations as `MM:SS` or `H:MM:SS` for tabular `JetBrains Mono` labels.
pub fn format_duration_ms(ms: u32) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

/// Formats remaining track duration as `-MM:SS` (or `-H:MM:SS`) for Mini-Player & Stage.
pub fn format_remaining_ms(position_ms: u32, duration_ms: u32) -> String {
    let rem = duration_ms.saturating_sub(position_ms);
    format!("-{}", format_duration_ms(rem))
}

/// Compact rounded badge (`OGG 320`, `PREMIUM`, `ALSA · 320k`, `CONNECT`).
pub fn status_badge<'a, Message: 'a>(
    label: impl Into<String>,
    tone_color: Color,
) -> Element<'a, Message> {
    let bg = Color { a: 0.14, ..tone_color };
    let border = Color { a: 0.36, ..tone_color };
    container(
        text(label.into())
            .size(11)
            .font(theme::FONT_MONO_MEDIUM)
            .color(tone_color),
    )
    .padding([2.0, 7.0])
    .style(theme::pill_container(bg, border, theme::RADIUS_SM))
    .into()
}

/// Status indicator dot + monospace/sans label inside a clickable or static pill.
pub fn status_dot_label<'a, Message: 'a>(
    dot_color: Color,
    label: impl Into<String>,
    text_color: Color,
) -> Element<'a, Message> {
    row![
        container(text("").size(6))
            .width(Length::Fixed(7.0))
            .height(Length::Fixed(7.0))
            .style(theme::pill_container(dot_color, dot_color, 999.0)),
        text(label.into())
            .size(12)
            .font(theme::FONT_MONO_MEDIUM)
            .color(text_color),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Compact monospace keyboard shortcut cap (`Ctrl+K`, `/`, `Esc`).
pub fn keycap<'a, Message: 'a>(label: &'a str) -> Element<'a, Message> {
    container(
        text(label)
            .size(11)
            .font(theme::FONT_MONO_MEDIUM)
            .color(theme::TEXT_SECONDARY),
    )
    .padding([2.0, 6.0])
    .style(theme::pill_container(
        theme::BG_SURFACE,
        theme::BORDER_STRONG,
        theme::RADIUS_SM,
    ))
    .into()
}

/// Uppercase tracked section header label (`11px`, `--text-muted`).
pub fn section_label<'a, Message: 'a>(label: &'a str) -> Element<'a, Message> {
    text(label)
        .size(11)
        .font(theme::FONT_MONO_SEMIBOLD)
        .color(theme::TEXT_MUTED)
        .into()
}

/// Thin horizontal `1px` structural hairline divider.
pub fn hairline_h<'a, Message: 'a>() -> Element<'a, Message> {
    container(text("").size(1))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(theme::divider_line)
        .into()
}

/// Thin vertical `1px` structural hairline divider.
pub fn hairline_v<'a, Message: 'a>() -> Element<'a, Message> {
    container(text("").size(1))
        .width(Length::Fixed(1.0))
        .height(Length::Fill)
        .style(theme::divider_line)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_track_and_remaining_times() {
        assert_eq!(format_duration_ms(0), "00:00");
        assert_eq!(format_duration_ms(102_500), "01:42");
        assert_eq!(format_duration_ms(3_661_000), "1:01:01");
        assert_eq!(format_remaining_ms(102_000, 243_000), "-02:21");
        assert_eq!(format_remaining_ms(300_000, 243_000), "-00:00");
    }
}
