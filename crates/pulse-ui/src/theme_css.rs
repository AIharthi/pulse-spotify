//! Adaptive Dark & Light GTK4 CSS generator registered above user themes (priority 810).

use gtk4::gdk;
use pulse_assets::{
    AdaptiveThemeTokens, DEFAULT_ACCENT_RGBA, load_system_matugen_accent,
};
use pulse_model::{AccentSource, AppConfig, ThemePreference};

fn rgba_css(rgb: [u8; 3], alpha: f32) -> String {
    format!("rgba({}, {}, {}, {:.3})", rgb[0], rgb[1], rgb[2], alpha)
}

/// Resolves whether Light mode should be active from user preference and GTK desktop settings.
pub fn resolve_is_light(pref: ThemePreference) -> bool {
    match pref {
        ThemePreference::Dark => false,
        ThemePreference::Light => true,
        ThemePreference::System => {
            if let Some(settings) = gtk4::Settings::default() {
                if settings.is_gtk_application_prefer_dark_theme() {
                    return false;
                }
                if let Some(name) = settings.gtk_theme_name() {
                    let lower = name.to_lowercase();
                    if lower.contains("dark") {
                        return false;
                    }
                    if lower.contains("light") {
                        return true;
                    }
                }
            }
            false
        }
    }
}

/// Resolves raw accent RGBA based on `AccentSource` config and current track cover accent.
pub fn resolve_raw_accent(config: &AppConfig, cover_accent_rgba: [u8; 4]) -> [u8; 4] {
    match config.accent_source {
        AccentSource::AlbumReactive => cover_accent_rgba,
        AccentSource::MatugenSystem => {
            load_system_matugen_accent().unwrap_or(cover_accent_rgba)
        }
        AccentSource::WarmAmber => DEFAULT_ACCENT_RGBA,
    }
}

/// Computes the WCAG AA token palette for the active theme preference and track artwork.
pub fn compute_tokens(config: &AppConfig, cover_accent_rgba: [u8; 4]) -> AdaptiveThemeTokens {
    let is_light = resolve_is_light(config.theme_preference);
    let raw = resolve_raw_accent(config, cover_accent_rgba);
    AdaptiveThemeTokens::from_accent(raw, is_light)
}

/// Builds pure GTK4-valid stylesheet CSS (never emits web-only properties or border-left stripes).
pub fn build_gtk_css(t: &AdaptiveThemeTokens) -> String {
    let hex = AdaptiveThemeTokens::hex;
    let bg_base = hex(t.bg_base);
    let bg_surface = hex(t.bg_surface);
    let bg_elevated = hex(t.bg_elevated);
    let bg_highlight = hex(t.bg_highlight);
    let border_subtle = hex(t.border_subtle);
    let border_strong = hex(t.border_strong);
    let fg_primary = hex(t.fg_primary);
    let fg_secondary = hex(t.fg_secondary);
    let fg_muted = hex(t.fg_muted);
    let accent = hex(t.accent);
    let accent_hover = hex(t.accent_hover);
    let on_accent = hex(t.on_accent);
    let status_ok = hex(t.status_ok);
    let status_warn = hex(t.status_warn);
    let status_err = hex(t.status_err);

    let accent_tint_bg = rgba_css(t.accent, 0.14);
    let accent_tint_border = rgba_css(t.accent, 0.42);
    let stage_wash_top = rgba_css(t.accent, if t.is_light { 0.12 } else { 0.16 });
    let stage_wash_mid = rgba_css(t.accent, 0.05);
    let err_tint_bg = rgba_css(t.status_err, 0.14);
    let err_tint_border = rgba_css(t.status_err, 0.42);
    let ok_tint_bg = rgba_css(t.status_ok, 0.14);
    let ok_tint_border = rgba_css(t.status_ok, 0.36);

    let sp_tint = rgba_css([29, 185, 84], 0.16);
    let sp_border = rgba_css([29, 185, 84], 0.44);
    let sp_fg = if t.is_light { "#0D6A35" } else { "#57E389" };

    let yt_tint = rgba_css([229, 62, 62], 0.16);
    let yt_border = rgba_css([229, 62, 62], 0.44);
    let yt_fg = if t.is_light { "#A61B1B" } else { "#FCA5A5" };

    let sc_tint = rgba_css([234, 88, 12], 0.16);
    let sc_border = rgba_css([234, 88, 12], 0.44);
    let sc_fg = if t.is_light { "#9A3412" } else { "#FDBA74" };

    let local_tint = rgba_css(t.accent, 0.15);
    let local_border = rgba_css(t.accent, 0.40);

    format!(
        r#"
window.pulse-root,
.pulse-root {{
    background-color: {bg_base};
    color: {fg_primary};
    font-family: "IBM Plex Sans", "Noto Sans", sans-serif;
    font-size: 13px;
    border-radius: 0px;
}}

.stage-root {{
    background-image: linear-gradient(180deg, {stage_wash_top} 0%, {stage_wash_mid} 45%, {bg_base} 100%);
    background-color: {bg_base};
}}

.top-bar {{
    background-color: {bg_surface};
    border-bottom: 1px solid {border_subtle};
    padding: 6px 12px;
}}

.left-rail {{
    background-color: {bg_surface};
    border-right: 1px solid {border_subtle};
    padding: 10px 8px;
}}

.right-drawer {{
    background-color: {bg_surface};
    border-left: 1px solid {border_subtle};
    padding: 10px 12px;
}}

.deck-bar {{
    background-color: {bg_surface};
    border-top: 1px solid {border_subtle};
    padding: 8px 14px;
}}

.mini-root {{
    background-color: {bg_surface};
    border: 1px solid {border_subtle};
    padding: 10px 12px;
}}

.content-canvas {{
    background-color: {bg_base};
    padding: 12px 18px;
}}

.surface-card {{
    background-color: {bg_surface};
    border: 1px solid {border_subtle};
    border-radius: 12px;
    padding: 14px;
}}

.surface-elevated {{
    background-color: {bg_elevated};
    border: 1px solid {border_subtle};
    border-radius: 12px;
    padding: 12px;
}}

.modal-card {{
    background-color: {bg_surface};
    border: 1px solid {border_strong};
    border-radius: 16px;
    padding: 16px;
}}

.hero-art {{
    border-radius: 12px;
    background-color: {bg_elevated};
    border: 1px solid {border_subtle};
}}

.thumb-art {{
    border-radius: 8px;
    background-color: {bg_elevated};
    border: 1px solid {border_subtle};
}}

.mono {{
    font-family: "IBM Plex Mono", "JetBrains Mono", monospace;
    font-feature-settings: "tnum" 1;
}}

.heading-xl {{
    font-size: 22px;
    font-weight: 700;
    color: {fg_primary};
}}

.heading-lg {{
    font-size: 17px;
    font-weight: 700;
    color: {fg_primary};
}}

.heading-md {{
    font-size: 14px;
    font-weight: 600;
    color: {fg_primary};
}}

.section-kicker {{
    font-size: 11px;
    font-weight: 600;
    color: {fg_muted};
    letter-spacing: 0.4px;
}}

.text-primary {{
    color: {fg_primary};
}}

.text-secondary {{
    color: {fg_secondary};
}}

.text-muted {{
    color: {fg_muted};
}}

.text-accent {{
    color: {accent};
}}

.text-ok {{
    color: {status_ok};
}}

.text-warn {{
    color: {status_warn};
}}

.text-err {{
    color: {status_err};
}}

button.ctrl-btn,
menubutton.ctrl-btn > button {{
    background-color: {bg_elevated};
    background-image: none;
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 8px;
    padding: 6px 10px;
    min-height: 28px;
    box-shadow: none;
    transition: background-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                border-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                color 150ms cubic-bezier(0.22, 1, 0.36, 1);
}}

button.ctrl-btn:hover,
menubutton.ctrl-btn > button:hover {{
    background-color: {bg_highlight};
    border-color: {border_strong};
}}

button.ctrl-btn.active,
menubutton.ctrl-btn.active > button {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
    color: {fg_primary};
    font-weight: 600;
}}

button.ghost-btn,
menubutton.ghost-btn > button {{
    background-color: transparent;
    background-image: none;
    color: {fg_secondary};
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 6px 8px;
    min-height: 28px;
    box-shadow: none;
    transition: background-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                border-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                color 150ms cubic-bezier(0.22, 1, 0.36, 1);
}}

button.ghost-btn:hover,
menubutton.ghost-btn > button:hover {{
    background-color: {bg_elevated};
    border-color: {border_subtle};
    color: {fg_primary};
}}

button.ghost-btn.active {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
    color: {fg_primary};
    font-weight: 600;
}}

button.accent-btn {{
    background-color: {accent};
    background-image: none;
    color: {on_accent};
    border: 1px solid {accent};
    border-radius: 8px;
    padding: 6px 14px;
    min-height: 30px;
    font-weight: 600;
    box-shadow: none;
    transition: background-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                border-color 150ms cubic-bezier(0.22, 1, 0.36, 1);
}}

button.accent-btn:hover {{
    background-color: {accent_hover};
    border-color: {accent_hover};
}}

button.danger-btn {{
    background-color: {err_tint_bg};
    background-image: none;
    color: {status_err};
    border: 1px solid {err_tint_border};
    border-radius: 8px;
    padding: 6px 12px;
    min-height: 28px;
    font-weight: 600;
    box-shadow: none;
}}

button.nav-btn {{
    background-color: transparent;
    background-image: none;
    color: {fg_secondary};
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 7px 10px;
    min-height: 32px;
    box-shadow: none;
    transition: background-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                border-color 150ms cubic-bezier(0.22, 1, 0.36, 1),
                color 150ms cubic-bezier(0.22, 1, 0.36, 1);
}}

button.nav-btn:hover {{
    background-color: {bg_elevated};
    color: {fg_primary};
}}

button.nav-btn.active {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
    color: {fg_primary};
    font-weight: 600;
}}

.mode-seg {{
    background-color: {bg_elevated};
    border: 1px solid {border_subtle};
    border-radius: 8px;
    padding: 2px;
}}

button.seg-btn {{
    background-color: transparent;
    background-image: none;
    color: {fg_secondary};
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 4px 10px;
    min-height: 24px;
    font-size: 12px;
    box-shadow: none;
}}

button.seg-btn:hover {{
    color: {fg_primary};
}}

button.seg-btn.active {{
    background-color: {bg_surface};
    border-color: {border_subtle};
    color: {fg_primary};
    font-weight: 600;
}}

.chip-pill,
button.chip-pill {{
    border-radius: 999px;
    padding: 2px 7px;
    font-size: 11px;
    font-weight: 600;
    background-color: {bg_elevated};
    background-image: none;
    color: {fg_secondary};
    border: 1px solid {border_subtle};
    min-height: 18px;
    box-shadow: none;
}}

button.track-play-btn {{
    background-color: transparent;
    background-image: none;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 2px 4px;
    box-shadow: none;
}}

button.track-play-btn:hover {{
    background-color: transparent;
}}

button.chip-pill:hover {{
    background-color: {bg_highlight};
    color: {fg_primary};
}}

button.chip-pill.active,
.chip-pill.active {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
    color: {fg_primary};
}}

.source-local {{
    background-color: {local_tint};
    border: 1px solid {local_border};
    color: {fg_primary};
}}

.source-spotify {{
    background-color: {sp_tint};
    border: 1px solid {sp_border};
    color: {sp_fg};
}}

.source-youtube {{
    background-color: {yt_tint};
    border: 1px solid {yt_border};
    color: {yt_fg};
}}

.source-soundcloud {{
    background-color: {sc_tint};
    border: 1px solid {sc_border};
    color: {sc_fg};
}}

.track-row {{
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 6px 10px;
    transition: background-color 120ms cubic-bezier(0.22, 1, 0.36, 1),
                border-color 120ms cubic-bezier(0.22, 1, 0.36, 1);
}}

.track-row:hover {{
    background-color: {bg_elevated};
    border-color: {border_subtle};
}}

.track-row.is-selected {{
    background-color: {bg_highlight};
    border-color: {border_strong};
}}

.track-row.is-playing {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
}}

listview.pulse-list,
listview.pulse-list > row {{
    background-color: transparent;
    color: {fg_primary};
    padding: 1px 0px;
}}

listview.pulse-list > row:selected {{
    background-color: transparent;
}}

button.lyric-line {{
    background-color: transparent;
    background-image: none;
    border: 1px solid transparent;
    border-radius: 8px;
    padding: 6px 10px;
    color: {fg_secondary};
    font-size: 13px;
    box-shadow: none;
}}

button.lyric-line:hover {{
    background-color: {bg_elevated};
    color: {fg_primary};
}}

button.lyric-line.is-active {{
    background-color: {accent_tint_bg};
    border-color: {accent_tint_border};
    color: {fg_primary};
    font-size: 15px;
    font-weight: 700;
}}

entry.pulse-entry {{
    background-color: {bg_elevated};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 8px;
    padding: 5px 10px;
    min-height: 30px;
    caret-color: {accent};
    box-shadow: none;
}}

entry.pulse-entry:focus-within {{
    border-color: {accent};
    background-color: {bg_surface};
}}

dropdown.pulse-dropdown > button {{
    background-color: {bg_elevated};
    color: {fg_primary};
    border: 1px solid {border_subtle};
    border-radius: 8px;
    padding: 4px 10px;
    min-height: 28px;
}}

scale.pulse-scale trough {{
    min-height: 5px;
    min-width: 5px;
    border-radius: 999px;
    background-color: {bg_highlight};
}}

scale.pulse-scale highlight {{
    border-radius: 999px;
    background-color: {accent};
}}

scale.pulse-scale slider {{
    min-width: 12px;
    min-height: 12px;
    border-radius: 999px;
    background-color: {fg_primary};
    border: 2px solid {accent};
    box-shadow: none;
}}

progressbar trough {{
    min-height: 5px;
    border-radius: 999px;
    background-color: {bg_highlight};
}}

progressbar progress {{
    min-height: 5px;
    border-radius: 999px;
    background-color: {accent};
}}

switch.pulse-switch {{
    background-color: {bg_highlight};
    border: 1px solid {border_subtle};
    border-radius: 999px;
    min-width: 40px;
    min-height: 22px;
}}

switch.pulse-switch:checked {{
    background-color: {accent};
    border-color: {accent};
}}

switch.pulse-switch slider {{
    background-color: {on_accent};
    border-radius: 999px;
    min-width: 16px;
    min-height: 16px;
}}

.notice-banner {{
    background-color: {ok_tint_bg};
    border-bottom: 1px solid {ok_tint_border};
    color: {fg_primary};
    padding: 6px 14px;
}}

.error-banner {{
    background-color: {err_tint_bg};
    border-bottom: 1px solid {err_tint_border};
    color: {fg_primary};
    padding: 6px 14px;
}}

popover > contents {{
    background-color: {bg_surface};
    color: {fg_primary};
    border: 1px solid {border_strong};
    border-radius: 12px;
    padding: 8px;
}}

button:focus-visible,
entry:focus-visible,
scale:focus-visible,
switch:focus-visible {{
    outline: 2px solid {accent};
    outline-offset: 2px;
}}
"#
    )
}

/// Applies the generated CSS to the default GDK display at priority 810.
pub fn apply_theme_css(provider: &gtk4::CssProvider, tokens: &AdaptiveThemeTokens) {
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(!tokens.is_light);
    }
    let css = build_gtk_css(tokens);
    provider.load_from_string(&css);
}

/// Registers our CSS provider above `~/.config/gtk-4.0/gtk.css` (`PRIORITY_USER + 10 = 810`).
pub fn install_theme_provider(tokens: &AdaptiveThemeTokens) -> gtk4::CssProvider {
    let provider = gtk4::CssProvider::new();
    apply_theme_css(&provider, tokens);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER + 10,
        );
    }
    provider
}
