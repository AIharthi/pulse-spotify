//! Cool Obsidian & Warm Analog Amber visual design tokens and Iced 0.14 style helpers.

use iced::border::Radius;
use iced::theme::Palette;
use iced::widget::{button, container, progress_bar, scrollable, slider, svg, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, font};

/// Background / surface tokens (`docs/UI_SPEC.md` Section 2.1).
pub const BG_VOID: Color = Color::from_rgb8(0x0B, 0x0D, 0x10);
pub const BG_CANVAS: Color = Color::from_rgb8(0x10, 0x13, 0x18);
pub const BG_SURFACE: Color = Color::from_rgb8(0x16, 0x1A, 0x22);
pub const BG_ELEVATED: Color = Color::from_rgb8(0x1D, 0x22, 0x2C);
pub const BG_SELECTED: Color = Color::from_rgb8(0x24, 0x2B, 0x38);

/// Border & hairline tokens.
pub const BORDER_SUBTLE: Color = Color::from_rgb8(0x22, 0x28, 0x34);
pub const BORDER_STRONG: Color = Color::from_rgb8(0x32, 0x3B, 0x4D);

/// Foreground / text tokens.
pub const TEXT_PRIMARY: Color = Color::from_rgb8(0xED, 0xF1, 0xF7);
pub const TEXT_SECONDARY: Color = Color::from_rgb8(0x9A, 0xA5, 0xB8);
pub const TEXT_MUTED: Color = Color::from_rgb8(0x63, 0x6E, 0x82);
pub const TEXT_UNAVAILABLE: Color = Color::from_rgba(
    0x63 as f32 / 255.0,
    0x6E as f32 / 255.0,
    0x82 as f32 / 255.0,
    0.45,
);

/// Warm Analog Amber accent tokens.
pub const ACCENT_PRIMARY: Color = Color::from_rgb8(0xE5, 0x9A, 0x3E);
pub const ACCENT_HOVER: Color = Color::from_rgb8(0xF2, 0xAE, 0x52);
pub const ACCENT_DIM: Color = Color::from_rgba(
    0xE5 as f32 / 255.0,
    0x9A as f32 / 255.0,
    0x3E as f32 / 255.0,
    0.14,
);
pub const ACCENT_TEXT_ON: Color = BG_VOID;

/// Semantic status tokens.
pub const STATUS_OK: Color = Color::from_rgb8(0x34, 0xD3, 0x99);
pub const STATUS_WARN: Color = Color::from_rgb8(0xFB, 0xBF, 0x24);
pub const STATUS_ERR: Color = Color::from_rgb8(0xF8, 0x71, 0x71);

/// Embedded Iced sans font (`fira-sans`) + monospace font (avoids 320-font `cosmic-text` fallback bloat).
pub const FONT_SANS: Font = Font::with_name("Fira Sans");
pub const FONT_SANS_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..FONT_SANS
};
pub const FONT_SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..FONT_SANS
};
pub const FONT_SANS_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..FONT_SANS
};
pub const FONT_MONO: Font = Font::with_name("JetBrains Mono");
pub const FONT_MONO_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..FONT_MONO
};
pub const FONT_MONO_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..FONT_MONO
};

/// 4px base grid & component dimensions.
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 12.0;
pub const SPACE_LG: f32 = 16.0;
pub const SPACE_XL: f32 = 20.0;
pub const SPACE_2XL: f32 = 24.0;

pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 6.0;
pub const RADIUS_LG: f32 = 8.0;
pub const RADIUS_XL: f32 = 10.0;
pub const RADIUS_MODAL: f32 = 12.0;

pub const TOP_BAR_HEIGHT: f32 = 48.0;
pub const SCOPE_STRIP_HEIGHT: f32 = 44.0;
pub const CRATE_HEADER_HEIGHT: f32 = 88.0;
pub const TRACK_ROW_HEIGHT: f32 = 44.0;
pub const FOCUS_QUEUE_ROW_HEIGHT: f32 = 48.0;

/// Builds the custom Pulse Cool Obsidian `iced::Theme`.
pub fn pulse_theme() -> Theme {
    Theme::custom(
        "Pulse Obsidian",
        Palette {
            background: BG_CANVAS,
            text: TEXT_PRIMARY,
            primary: ACCENT_PRIMARY,
            success: STATUS_OK,
            warning: STATUS_WARN,
            danger: STATUS_ERR,
        },
    )
}

/// Outer window / backdrop style.
pub fn void_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_VOID)),
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Primary workspace canvas background.
pub fn canvas_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CANVAS)),
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Standard `1px`-bordered card container (`#161A22`).
pub fn surface_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Elevated container (`#1D222C`) for bars, cards, and active containers.
pub fn elevated_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_STRONG,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Top Command & Mode Bar (`48px` height).
pub fn top_bar_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_SUBTLE,
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Modal / Settings / Onboarding sheet container (`12px` radius, crisp `1px` border).
pub fn modal_sheet(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_STRONG,
            width: 1.0,
            radius: RADIUS_MODAL.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Translucent dark backdrop behind modals (`#0B0D10` at 74% alpha).
pub fn modal_backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.74, ..BG_VOID })),
        ..Default::default()
    }
}

/// Lightweight popover / context menu panel (`8px` radius).
pub fn popover_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_STRONG,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Mini-player outer frame with `1px` strong border.
pub fn mini_player_frame(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        text_color: Some(TEXT_PRIMARY),
        border: Border {
            color: BORDER_STRONG,
            width: 1.0,
            radius: RADIUS_LG.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Subtle `1px` divider strip (`BORDER_SUBTLE`).
pub fn divider_line(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_SUBTLE)),
        ..Default::default()
    }
}

/// Notice or warning slim banner bar (`28px` strip).
pub fn banner_container(is_error: bool) -> impl Fn(&Theme) -> container::Style {
    let accent = if is_error { STATUS_ERR } else { STATUS_WARN };
    let bg = Color { a: 0.14, ..accent };
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(accent),
        border: Border {
            color: Color { a: 0.35, ..accent },
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Generic tinted pill / badge container.
pub fn pill_container(
    bg: Color,
    border_color: Color,
    radius: f32,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(bg)),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: Shadow::default(),
        snap: true,
        text_color: None,
    }
}

/// Virtualized track/queue row styling by playback/selection/hover state.
pub fn row_container(
    is_playing: bool,
    is_selected: bool,
    is_hovered: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| {
        let bg = if is_playing {
            Some(Background::Color(ACCENT_DIM))
        } else if is_selected {
            Some(Background::Color(BG_SELECTED))
        } else if is_hovered {
            Some(Background::Color(BG_ELEVATED))
        } else {
            None
        };
        container::Style {
            background: bg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: RADIUS_MD.into(),
            },
            shadow: Shadow::default(),
            snap: true,
            text_color: None,
        }
    }
}

/// Solid color bar (used for `2px` cursor / `3px` mini-player left accent bar).
pub fn accent_strip(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    }
}

/// Placeholder artwork box shown while cover art is downloading/decoding.
pub fn cover_placeholder(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: radius.into(),
        },
        shadow: Shadow::default(),
        snap: true,
        text_color: Some(TEXT_MUTED),
    }
}

/// Primary Warm Amber CTA button.
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => ACCENT_HOVER,
        button::Status::Disabled => BORDER_SUBTLE,
        button::Status::Active => ACCENT_PRIMARY,
    };
    let text_color = match status {
        button::Status::Disabled => TEXT_MUTED,
        _ => ACCENT_TEXT_ON,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS_MD.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Circular transport Play/Pause primary button.
pub fn round_play_button(_theme: &Theme, status: button::Status) -> button::Style {
    let mut style = primary_button(_theme, status);
    style.border.radius = Radius::from(999.0);
    style
}

/// Secondary outlined dark button (`#1D222C`).
pub fn secondary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, border_color) = match status {
        button::Status::Hovered | button::Status::Pressed => (BG_SELECTED, BORDER_STRONG),
        button::Status::Disabled => (BG_SURFACE, BORDER_SUBTLE),
        button::Status::Active => (BG_ELEVATED, BORDER_STRONG),
    };
    let text_color = if matches!(status, button::Status::Disabled) {
        TEXT_MUTED
    } else {
        TEXT_PRIMARY
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS_MD.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Ghost / icon / clickable metadata button (transparent idle, `#1D222C` hover).
pub fn ghost_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (bg, text_color) = match status {
        button::Status::Hovered | button::Status::Pressed => (Some(Background::Color(BG_ELEVATED)), TEXT_PRIMARY),
        button::Status::Disabled => (None, TEXT_MUTED),
        button::Status::Active => (None, TEXT_SECONDARY),
    };
    button::Style {
        background: bg,
        text_color,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS_MD.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Subtle destructive action button (`--status-err`).
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg_alpha = match status {
        button::Status::Hovered | button::Status::Pressed => 0.24,
        button::Status::Disabled => 0.06,
        button::Status::Active => 0.14,
    };
    button::Style {
        background: Some(Background::Color(Color {
            a: bg_alpha,
            ..STATUS_ERR
        })),
        text_color: if matches!(status, button::Status::Disabled) {
            TEXT_MUTED
        } else {
            STATUS_ERR
        },
        border: Border {
            color: Color {
                a: 0.45,
                ..STATUS_ERR
            },
            width: 1.0,
            radius: RADIUS_MD.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Segmented mode/tab pill button (`Workbench | Stage | Mini`, Scope pills).
pub fn pill_button(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme: &Theme, status: button::Status| {
        let (bg, border_color, text_color) = if active {
            (
                Some(Background::Color(BG_SELECTED)),
                BORDER_STRONG,
                TEXT_PRIMARY,
            )
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => (
                    Some(Background::Color(BG_ELEVATED)),
                    BORDER_SUBTLE,
                    TEXT_PRIMARY,
                ),
                button::Status::Disabled => (None, Color::TRANSPARENT, TEXT_MUTED),
                button::Status::Active => (None, Color::TRANSPARENT, TEXT_SECONDARY),
            }
        };
        button::Style {
            background: bg,
            text_color,
            border: Border {
                color: border_color,
                width: if active { 1.0 } else { 0.0 },
                radius: RADIUS_MD.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Row/surface wrapper button without background chrome (used for clickable cards/tracks).
pub fn plain_interactive_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: TEXT_PRIMARY,
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Dark omnibox / filter / settings input style.
pub fn search_input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let (bg, border_color) = match status {
        text_input::Status::Active => (BG_CANVAS, BORDER_SUBTLE),
        text_input::Status::Hovered => (BG_CANVAS, BORDER_STRONG),
        text_input::Status::Focused { .. } => (BG_CANVAS, ACCENT_PRIMARY),
        text_input::Status::Disabled => (BG_SURFACE, BORDER_SUBTLE),
    };
    text_input::Style {
        background: Background::Color(bg),
        border: Border {
            color: border_color,
            width: 1.0,
            radius: RADIUS_MD.into(),
        },
        icon: TEXT_MUTED,
        placeholder: TEXT_MUTED,
        value: TEXT_PRIMARY,
        selection: ACCENT_DIM,
    }
}

/// Thin dark scrollbar styling matching Cool Obsidian.
pub fn dark_scrollable(_theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let scroller_color = match status {
        scrollable::Status::Active { .. } => BORDER_SUBTLE,
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered: true,
            ..
        }
        | scrollable::Status::Dragged {
            is_vertical_scrollbar_dragged: true,
            ..
        } => TEXT_MUTED,
        _ => BORDER_STRONG,
    };

    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(scroller_color),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: RADIUS_SM.into(),
            },
        },
    };

    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(BG_ELEVATED),
            border: Border {
                color: BORDER_STRONG,
                width: 1.0,
                radius: RADIUS_MD.into(),
            },
            shadow: Shadow::default(),
            icon: ACCENT_PRIMARY,
        },
    }
}

/// Thin 4px Warm Amber scrubber/volume slider (`#E59A3E` elapsed track).
pub fn amber_slider(_theme: &Theme, status: slider::Status) -> slider::Style {
    let (handle_radius, handle_color) = match status {
        slider::Status::Active => (0.0, Color::TRANSPARENT),
        slider::Status::Hovered => (6.0, ACCENT_PRIMARY),
        slider::Status::Dragged => (7.0, ACCENT_HOVER),
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (
                Background::Color(ACCENT_PRIMARY),
                Background::Color(BORDER_STRONG),
            ),
            width: 4.0,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 2.0.into(),
            },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle {
                radius: handle_radius,
            },
            background: Background::Color(handle_color),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

/// RSS budget progress bar colored green (`< 65`), amber (`65..=82`), or red (`> 82`).
pub fn rss_progress_bar(rss_mb: f32) -> impl Fn(&Theme) -> progress_bar::Style {
    let bar_color = rss_status_color(rss_mb);
    move |_theme: &Theme| progress_bar::Style {
        background: Background::Color(BG_CANVAS),
        bar: Background::Color(bar_color),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
    }
}

/// Returns status color for RSS memory ceiling (`90 MB` hard cap).
pub const fn rss_status_color(rss_mb: f32) -> Color {
    if rss_mb > 82.0 {
        STATUS_ERR
    } else if rss_mb >= 65.0 {
        STATUS_WARN
    } else {
        STATUS_OK
    }
}

/// Monochrome SVG icon color filter helper.
pub fn svg_colored(color: Color) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |_theme: &Theme, _status: svg::Status| svg::Style { color: Some(color) }
}
