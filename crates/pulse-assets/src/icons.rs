//! Monochrome 24x24 vector SVG icon registry (`stroke-width="1.75"`, dynamic tinting).

/// Renders an SVG byte template replacing `#ffffff` with the supplied `#RRGGBB` color string.
pub fn tint_svg(svg: &[u8], color_hex: &str) -> Vec<u8> {
    let text = std::str::from_utf8(svg).unwrap_or("");
    text.replace("#ffffff", color_hex).into_bytes()
}

pub const PLAY: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M6 4l14 8-14 8V4z" fill="#ffffff"/></svg>"##;
pub const PAUSE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M6 5h4v14H6V5zm8 0h4v14h-4V5z" fill="#ffffff"/></svg>"##;
pub const NEXT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M5 5l10 7-10 7V5zm12 0h2v14h-2V5z" fill="#ffffff"/></svg>"##;
pub const PREV: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M19 5L9 12l10 7V5zM5 5h2v14H5V5z" fill="#ffffff"/></svg>"##;
pub const SHUFFLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M16 3h5v5M4 20L21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/></svg>"##;
pub const REPEAT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M17 2l4 4-4 4"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><path d="M7 22l-4-4 4-4"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>"##;
pub const REPEAT_ONE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M17 2l4 4-4 4"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><path d="M7 22l-4-4 4-4"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/><path d="M11 10.5L12.5 9v6"/></svg>"##;
pub const HEART_OUTLINE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z"/></svg>"##;
pub const HEART_FILLED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" fill="#ffffff"/></svg>"##;
pub const SEARCH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-4.35-4.35"/></svg>"##;
pub const QUEUE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M3 12h12M3 18h9"/><path d="M16 15l5 3-5 3v-6z" fill="#ffffff" stroke="none"/></svg>"##;
pub const DEVICE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>"##;
pub const SETTINGS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.77 1 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"##;
pub const PIN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>"##;
pub const EXPAND: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/></svg>"##;
pub const BACK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>"##;
pub const FORWARD: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18l6-6-6-6"/></svg>"##;
pub const DRAG_HANDLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="9" cy="6" r="1.5" fill="#ffffff"/><circle cx="15" cy="6" r="1.5" fill="#ffffff"/><circle cx="9" cy="12" r="1.5" fill="#ffffff"/><circle cx="15" cy="12" r="1.5" fill="#ffffff"/><circle cx="9" cy="18" r="1.5" fill="#ffffff"/><circle cx="15" cy="18" r="1.5" fill="#ffffff"/></svg>"##;
pub const CLOSE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6L6 18M6 6l12 12"/></svg>"##;
pub const VOLUME_HIGH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07M19.07 4.93a10 10 0 0 1 0 14.14"/></svg>"##;
pub const VOLUME_MUTE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H2v6h4l5 4V5z"/><path d="M22 9l-6 6M16 9l6 6"/></svg>"##;
pub const CHECK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6L9 17l-5-5"/></svg>"##;
pub const MUSIC_NOTE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>"##;
pub const EXTERNAL_LINK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><path d="M15 3h6v6M10 14L21 3"/></svg>"##;
pub const HOME: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 10.5L12 3l9 7.5"/><path d="M5 9.5V20h5v-6h4v6h5V9.5"/></svg>"##;
pub const FOLDER_MUSIC: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"/><path d="M11 16v-5l4-1v5"/></svg>"##;
pub const PLAYLIST: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7h12M3 12h12M3 17h8"/><circle cx="17.5" cy="16.5" r="2.5"/><path d="M20 16.5V9l-3 1"/></svg>"##;
pub const PLUS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M5 12h14"/></svg>"##;
pub const LYRICS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h16M4 11h12M4 16h8"/><path d="M16 14v5a2 2 0 1 1-2-2"/><path d="M16 14l4-1v4"/></svg>"##;
pub const SLIDERS_EQ: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M6 4v16M12 4v16M18 4v16"/><circle cx="6" cy="9" r="2" fill="#ffffff"/><circle cx="12" cy="15" r="2" fill="#ffffff"/><circle cx="18" cy="10" r="2" fill="#ffffff"/></svg>"##;
pub const WAVEFORM: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round"><path d="M4 10v4M8 7v10M12 4v16M16 8v8M20 10v4"/></svg>"##;
pub const INSIGHTS: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 19h16"/><rect x="6" y="12" width="3" height="5" rx="0.5"/><rect x="11" y="7" width="3" height="10" rx="0.5"/><rect x="16" y="10" width="3" height="7" rx="0.5"/></svg>"##;
pub const TIMER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2.5M9 2h6"/></svg>"##;
pub const AB_LOOP: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12a6 6 0 0 1 6-6h8"/><path d="M15 3l3 3-3 3"/><path d="M20 12a6 6 0 0 1-6 6H6"/><path d="M9 15l-3 3 3 3"/></svg>"##;
pub const SUN: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41"/></svg>"##;
pub const MOON: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>"##;
pub const MINI_PLAYER: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><rect x="11" y="12" width="8" height="6" rx="1" fill="#ffffff"/></svg>"##;
pub const TRASH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"/></svg>"##;
pub const REFRESH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>"##;
pub const SIDEBAR_TOGGLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/></svg>"##;
pub const SPOTIFY_MARK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M8 9.5c2.8-.8 5.9-.5 8.4.8M8.7 12.6c2.2-.6 4.7-.4 6.8.7M9.3 15.5c1.7-.4 3.5-.3 5.1.5"/></svg>"##;
pub const YOUTUBE_MARK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="6" width="18" height="12" rx="3"/><path d="M10 9.5l5 2.5-5 2.5v-5z" fill="#ffffff" stroke="none"/></svg>"##;
pub const SOUNDCLOUD_MARK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M12 18h6.5a3.5 3.5 0 0 0 .3-7A5.5 5.5 0 0 0 8.5 9.5"/><path d="M3 14v4M6 12v6M9 10v8"/></svg>"##;
pub const LOCAL_DISC: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2.5"/></svg>"##;
pub const HISTORY_RECENT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#ffffff" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>"##;

/// Exhaustive list of bundled vector icons.
pub const ALL_ICONS: [(&str, &[u8]); 44] = [
    ("play", PLAY),
    ("pause", PAUSE),
    ("next", NEXT),
    ("prev", PREV),
    ("shuffle", SHUFFLE),
    ("repeat", REPEAT),
    ("repeat_one", REPEAT_ONE),
    ("heart_outline", HEART_OUTLINE),
    ("heart_filled", HEART_FILLED),
    ("search", SEARCH),
    ("queue", QUEUE),
    ("device", DEVICE),
    ("settings", SETTINGS),
    ("pin", PIN),
    ("expand", EXPAND),
    ("back", BACK),
    ("forward", FORWARD),
    ("drag_handle", DRAG_HANDLE),
    ("close", CLOSE),
    ("volume_high", VOLUME_HIGH),
    ("volume_mute", VOLUME_MUTE),
    ("check", CHECK),
    ("music_note", MUSIC_NOTE),
    ("external_link", EXTERNAL_LINK),
    ("home", HOME),
    ("folder_music", FOLDER_MUSIC),
    ("playlist", PLAYLIST),
    ("plus", PLUS),
    ("lyrics", LYRICS),
    ("sliders_eq", SLIDERS_EQ),
    ("waveform", WAVEFORM),
    ("insights", INSIGHTS),
    ("timer", TIMER),
    ("ab_loop", AB_LOOP),
    ("sun", SUN),
    ("moon", MOON),
    ("mini_player", MINI_PLAYER),
    ("trash", TRASH),
    ("refresh", REFRESH),
    ("sidebar_toggle", SIDEBAR_TOGGLE),
    ("spotify_mark", SPOTIFY_MARK),
    ("youtube_mark", YOUTUBE_MARK),
    ("soundcloud_mark", SOUNDCLOUD_MARK),
    ("local_disc", LOCAL_DISC),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_valid_24x24_svgs_without_text_elements() {
        assert_eq!(ALL_ICONS.len(), 44);
        for (name, bytes) in ALL_ICONS {
            let s = std::str::from_utf8(bytes).expect("valid utf8 svg");
            assert!(s.contains("viewBox=\"0 0 24 24\""), "{name} missing 24x24 viewBox");
            assert!(!s.contains("<text"), "{name} must not trigger font lookups via <text>");
            assert!(s.ends_with("</svg>"), "{name} incomplete svg");
        }
        let tinted = String::from_utf8(tint_svg(HOME, "#11151E")).unwrap();
        assert!(tinted.contains("#11151E"));
    }
}
