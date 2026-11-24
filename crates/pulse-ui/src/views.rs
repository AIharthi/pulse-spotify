//! Top Header, Left Rail, Center Canvas (virtualized ListView), Playback Deck, Stage & Mini views.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::gio;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use pulse_assets::{AdaptiveThemeTokens, CoverTier, icons};
use pulse_model::{
    AppMode, AuthState, CrateRef, ImageRef, LibraryScope, LocalSubView, MediaSource, RepeatMode,
    RightDrawerTab, SortOrder, TrackSummary, UiCommand,
};

use crate::SpectrumReadFn;
use crate::dialogs::{open_add_to_playlist_popover, populate_devices_popover, populate_sleep_popover};
use crate::drawers::LyricsPanelWidget;
use crate::icons_gtk::{make_clickable, vector_icon_button};
use crate::state::{
    DirtyFlags, UiBridge, format_duration_ms, sort_order_label, source_badge_css_class,
};
use crate::visualizer::AudioVisualizerWidget;

#[derive(Clone)]
struct TrackRowItem {
    index: usize,
    track: TrackSummary,
}

struct RowWidgets {
    root: gtk4::Box,
    num_lbl: gtk4::Label,
    thumb_pic: gtk4::Picture,
    thumb_fallback: gtk4::Image,
    title_lbl: gtk4::Label,
    sub_lbl: gtk4::Label,
    source_chip: gtk4::Label,
    dur_lbl: gtk4::Label,
    like_img: gtk4::Image,
    bound: Rc<RefCell<Option<TrackRowItem>>>,
    bound_cover_id: Rc<RefCell<String>>,
}

/// Top Command Header (`52px`).
pub struct TopHeaderWidget {
    pub root: gtk4::Box,
    pub notice_bar: gtk4::Box,
    notice_lbl: gtk4::Label,
    sidebar_img: gtk4::Image,
    back_btn: gtk4::Button,
    back_img: gtk4::Image,
    fwd_btn: gtk4::Button,
    fwd_img: gtk4::Image,
    pub mode_btns: Vec<(AppMode, gtk4::Button)>,
    pub omnibox_entry: gtk4::Entry,
    pub search_btn: gtk4::Button,
    search_btn_img: gtk4::Image,
    sleep_btn: gtk4::MenuButton,
    sleep_lbl: gtk4::Label,
    sleep_img: gtk4::Image,
    dev_btn: gtk4::MenuButton,
    dev_lbl: gtk4::Label,
    dev_img: gtk4::Image,
    pub theme_btn: gtk4::Button,
    theme_img: gtk4::Image,
    settings_img: gtk4::Image,
    suppress_omnibox: Rc<Cell<bool>>,
}

impl TopHeaderWidget {
    pub fn new(bridge: &UiBridge) -> Rc<Self> {
        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

        let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        root.add_css_class("top-bar");

        let fg_sec = {
            let s = bridge.store.borrow();
            AdaptiveThemeTokens::hex(s.tokens.fg_secondary)
        };

        let (sidebar_btn, sidebar_img) = vector_icon_button(
            &bridge.icons,
            "sidebar_toggle",
            icons::SIDEBAR_TOGGLE,
            &fg_sec,
            16,
            "Toggle library sidebar",
            &["ghost-btn"],
        );
        let b_sb = bridge.clone();
        sidebar_btn.connect_clicked(move |_| {
            let collapsed = {
                let mut s = b_sb.store.borrow_mut();
                let next = !s.is_nav_collapsed();
                s.nav_collapsed_override = Some(next);
                next
            };
            b_sb.record_action(format!("header:sidebar:{collapsed}"));
            (b_sb.redraw)(DirtyFlags::NAV | DirtyFlags::CHROME);
        });
        root.append(&sidebar_btn);

        let (back_btn, back_img) = vector_icon_button(
            &bridge.icons,
            "back",
            icons::BACK,
            &fg_sec,
            16,
            "Navigate back",
            &["ghost-btn"],
        );
        let b_back = bridge.clone();
        back_btn.connect_clicked(move |_| {
            b_back.record_action("header:back");
            (b_back.send)(UiCommand::NavigateBack);
        });
        let (fwd_btn, fwd_img) = vector_icon_button(
            &bridge.icons,
            "forward",
            icons::FORWARD,
            &fg_sec,
            16,
            "Navigate forward",
            &["ghost-btn"],
        );
        let b_fwd = bridge.clone();
        fwd_btn.connect_clicked(move |_| {
            b_fwd.record_action("header:forward");
            (b_fwd.send)(UiCommand::NavigateForward);
        });
        root.append(&back_btn);
        root.append(&fwd_btn);

        // Mode switcher (`Deck` | `Stage` | `Mini`)
        let seg = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
        seg.add_css_class("mode-seg");
        let mut mode_btns = Vec::new();
        for (mode, label) in [
            (AppMode::Workbench, "Deck"),
            (AppMode::FocusStage, "Stage"),
            (AppMode::MiniPlayer, "Mini"),
        ] {
            let btn = gtk4::Button::with_label(label);
            btn.set_css_classes(&["seg-btn"]);
            make_clickable(&btn, &format!("Switch to {label} mode"));
            let b_m = bridge.clone();
            btn.connect_clicked(move |_| {
                b_m.record_action(format!("mode:{label}"));
                (b_m.send)(UiCommand::SetMode(mode));
            });
            seg.append(&btn);
            mode_btns.push((mode, btn));
        }
        root.append(&seg);

        // Unified Omnibox
        let omnibox_entry = gtk4::Entry::new();
        omnibox_entry.add_css_class("pulse-entry");
        omnibox_entry.set_hexpand(true);
        omnibox_entry.set_placeholder_text(Some(
            "Search Local, Spotify, YouTube, SoundCloud or paste any URL/path...",
        ));

        let suppress_omnibox = Rc::new(Cell::new(false));
        let sup_chg = Rc::clone(&suppress_omnibox);
        let b_chg = bridge.clone();
        omnibox_entry.connect_changed(move |entry| {
            if sup_chg.get() {
                return;
            }
            let text = entry.text().to_string();
            let (global_mode, scope) = {
                let mut s = b_chg.store.borrow_mut();
                s.omnibox_text = text.clone();
                (s.omnibox_global_mode, s.view.crate_view.active_scope)
            };
            let looks_url = looks_like_direct_input(&text);
            if !global_mode && !looks_url && scope != LibraryScope::Home && scope != LibraryScope::Search {
                (b_chg.send)(UiCommand::SetLocalFilter(text));
            }
        });

        let submit_omnibox = |bridge: &UiBridge, text: String| {
            let trimmed = text.trim().to_string();
            if trimmed.is_empty() {
                return;
            }
            if looks_like_direct_input(&trimmed) {
                bridge.record_action("omnibox:resolve-url");
                (bridge.send)(UiCommand::ResolveInputUrl(trimmed));
            } else {
                let sf = bridge.store.borrow().view.crate_view.search_source_filter;
                bridge.store.borrow_mut().view.crate_view.is_loading = true;
                bridge.record_action(format!("omnibox:search:{trimmed}"));
                (bridge.send)(UiCommand::RunGlobalSearch {
                    query: trimmed,
                    source_filter: sf,
                });
                (bridge.redraw)(DirtyFlags::CONTENT);
            }
        };

        let b_act = bridge.clone();
        omnibox_entry.connect_activate(move |entry| {
            submit_omnibox(&b_act, entry.text().to_string());
        });

        let (search_btn, search_btn_img) = vector_icon_button(
            &bridge.icons,
            "search",
            icons::SEARCH,
            &fg_sec,
            15,
            "Search all sources",
            &["ctrl-btn"],
        );
        let b_sb2 = bridge.clone();
        let oe_c = omnibox_entry.clone();
        search_btn.connect_clicked(move |_| {
            submit_omnibox(&b_sb2, oe_c.text().to_string());
        });

        root.append(&omnibox_entry);
        root.append(&search_btn);

        // Sleep Timer MenuButton
        let sleep_btn = gtk4::MenuButton::new();
        sleep_btn.add_css_class("ctrl-btn");
        let sleep_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let sleep_img = bridge.icons.make_image("timer", icons::TIMER, &fg_sec, 15);
        let sleep_lbl = gtk4::Label::new(Some("Sleep"));
        sleep_lbl.set_css_classes(&["mono"]);
        sleep_content.append(&sleep_img);
        sleep_content.append(&sleep_lbl);
        sleep_btn.set_child(Some(&sleep_content));
        make_clickable(&sleep_btn, "Sleep timer");
        let sleep_pop = gtk4::Popover::new();
        populate_sleep_popover(&sleep_pop, bridge);
        sleep_btn.set_popover(Some(&sleep_pop));
        root.append(&sleep_btn);

        // Devices MenuButton
        let dev_btn = gtk4::MenuButton::new();
        dev_btn.add_css_class("ctrl-btn");
        let dev_content = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let dev_img = bridge.icons.make_image("device", icons::DEVICE, &fg_sec, 15);
        let dev_lbl = gtk4::Label::new(Some("Audio"));
        dev_lbl.set_ellipsize(pango::EllipsizeMode::End);
        dev_lbl.set_max_width_chars(12);
        dev_content.append(&dev_img);
        dev_content.append(&dev_lbl);
        dev_btn.set_child(Some(&dev_content));
        make_clickable(&dev_btn, "Playback output devices & Spotify Connect");
        let dev_pop = gtk4::Popover::new();
        let b_dev = bridge.clone();
        let dp_c = dev_pop.clone();
        dev_pop.connect_show(move |_| {
            populate_devices_popover(&dp_c, &b_dev);
        });
        dev_btn.set_popover(Some(&dev_pop));
        root.append(&dev_btn);

        // Dark / Light quick toggle
        let (theme_btn, theme_img) = vector_icon_button(
            &bridge.icons,
            "sun",
            icons::SUN,
            &fg_sec,
            16,
            "Switch Light / Dark theme",
            &["ghost-btn"],
        );
        let b_th = bridge.clone();
        theme_btn.connect_clicked(move |_| {
            let next = {
                let s = b_th.store.borrow();
                if s.tokens.is_light {
                    pulse_model::ThemePreference::Dark
                } else {
                    pulse_model::ThemePreference::Light
                }
            };
            b_th.record_action(format!("theme:{next:?}"));
            (b_th.send)(UiCommand::SetThemePreference(next));
        });
        root.append(&theme_btn);

        // Settings button
        let (settings_btn, settings_img) = vector_icon_button(
            &bridge.icons,
            "settings",
            icons::SETTINGS,
            &fg_sec,
            16,
            "Open Settings (Ctrl+,)",
            &["ghost-btn"],
        );
        let b_set = bridge.clone();
        settings_btn.connect_clicked(move |_| {
            {
                let mut s = b_set.store.borrow_mut();
                s.settings_open = true;
            }
            b_set.record_action("settings:open");
            (b_set.redraw)(DirtyFlags::SETTINGS);
        });
        root.append(&settings_btn);

        // Notice Banner strip
        let notice_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        notice_bar.add_css_class("notice-banner");
        notice_bar.set_visible(false);
        let notice_lbl = gtk4::Label::new(None);
        notice_lbl.set_hexpand(true);
        notice_lbl.set_xalign(0.0);
        notice_lbl.set_ellipsize(pango::EllipsizeMode::End);
        let dismiss_btn = gtk4::Button::with_label("Dismiss");
        dismiss_btn.set_css_classes(&["ghost-btn"]);
        let b_dis = bridge.clone();
        dismiss_btn.connect_clicked(move |_| {
            {
                let mut s = b_dis.store.borrow_mut();
                s.view.notice_banner = None;
                s.view.crate_view.error = None;
            }
            (b_dis.redraw)(DirtyFlags::CHROME);
        });
        notice_bar.append(&notice_lbl);
        notice_bar.append(&dismiss_btn);

        outer.append(&root);
        outer.append(&notice_bar);

        Rc::new(Self {
            root: outer,
            notice_bar,
            notice_lbl,
            sidebar_img,
            back_btn,
            back_img,
            fwd_btn,
            fwd_img,
            mode_btns,
            omnibox_entry,
            search_btn,
            search_btn_img,
            sleep_btn,
            sleep_lbl,
            sleep_img,
            dev_btn,
            dev_lbl,
            dev_img,
            theme_btn,
            theme_img,
            settings_img,
            suppress_omnibox,
        })
    }

    pub fn sync(&self, bridge: &UiBridge, flags: DirtyFlags) {
        let (mode, can_back, can_fwd, sleep, dev_name, is_light, notice, has_err, width, tokens) = {
            let s = bridge.store.borrow();
            let cv = &s.view.crate_view;
            let can_b = cv.history_index > 0;
            let can_f = cv.history_index + 1 < cv.history.len();
            let active_dev = s
                .view
                .devices
                .available_devices
                .iter()
                .find(|d| d.is_active)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "Audio".to_string());
            (
                s.view.mode,
                can_b,
                can_f,
                s.view.player_state.sleep_timer,
                active_dev,
                s.tokens.is_light,
                s.view.notice_banner.clone(),
                cv.error.is_some(),
                s.window_width,
                s.tokens.clone(),
            )
        };

        self.back_btn.set_sensitive(can_back);
        self.fwd_btn.set_sensitive(can_fwd);
        for (m, btn) in &self.mode_btns {
            if *m == mode {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }

        if let Some(secs) = sleep.remaining_secs {
            let mins = secs.div_ceil(60);
            self.sleep_lbl.set_text(&format!("{mins}m"));
            self.sleep_btn.add_css_class("active");
        } else if sleep.end_of_track {
            self.sleep_lbl.set_text("Track");
            self.sleep_btn.add_css_class("active");
        } else {
            self.sleep_lbl.set_text(if width < 860 { "" } else { "Sleep" });
            self.sleep_btn.remove_css_class("active");
        }
        self.sleep_lbl
            .set_visible(width >= 860 || sleep.remaining_secs.is_some() || sleep.end_of_track);
        self.dev_lbl.set_text(&dev_name);
        self.dev_lbl.set_visible(width >= 920);
        self.dev_btn.set_visible(width >= 760);

        if let Some(msg) = notice {
            self.notice_lbl.set_text(&msg);
            if has_err {
                self.notice_bar.remove_css_class("notice-banner");
                self.notice_bar.add_css_class("error-banner");
            } else {
                self.notice_bar.remove_css_class("error-banner");
                self.notice_bar.add_css_class("notice-banner");
            }
            self.notice_bar.set_visible(true);
        } else {
            self.notice_bar.set_visible(false);
        }

        if flags.any(DirtyFlags::THEME | DirtyFlags::ALL) {
            let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
            bridge.icons.apply_to_image(&self.sidebar_img, "sidebar_toggle", icons::SIDEBAR_TOGGLE, &fg_sec, 16);
            bridge.icons.apply_to_image(&self.back_img, "back", icons::BACK, &fg_sec, 16);
            bridge.icons.apply_to_image(&self.fwd_img, "forward", icons::FORWARD, &fg_sec, 16);
            bridge.icons.apply_to_image(&self.search_btn_img, "search", icons::SEARCH, &fg_sec, 15);
            bridge.icons.apply_to_image(&self.sleep_img, "timer", icons::TIMER, &fg_sec, 15);
            bridge.icons.apply_to_image(&self.dev_img, "device", icons::DEVICE, &fg_sec, 15);
            bridge.icons.apply_to_image(&self.settings_img, "settings", icons::SETTINGS, &fg_sec, 16);
            let (tk, tsvg) = if is_light {
                ("moon", icons::MOON)
            } else {
                ("sun", icons::SUN)
            };
            bridge.icons.apply_to_image(&self.theme_img, tk, tsvg, &fg_sec, 16);
        }
    }

    pub fn set_omnibox_text(&self, text: &str) {
        self.suppress_omnibox.set(true);
        self.omnibox_entry.set_text(text);
        self.suppress_omnibox.set(false);
    }
}

fn looks_like_direct_input(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("spotify:")
        || t.starts_with("yt:")
        || t.starts_with("sc:")
        || t.starts_with("local:")
        || t.starts_with("file://")
        || t.starts_with('/')
}

/// Left Navigation Rail (`212px` expanded, `56px` collapsed).
pub struct LeftRailWidget {
    pub root: gtk4::Box,
    content_box: gtk4::Box,
}

impl LeftRailWidget {
    pub fn new() -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        root.add_css_class("left-rail");

        let scroller = gtk4::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        scroller.set_child(Some(&content_box));
        root.append(&scroller);

        Rc::new(Self { root, content_box })
    }

    pub fn sync(&self, bridge: &UiBridge) {
        while let Some(c) = self.content_box.first_child() {
            self.content_box.remove(&c);
        }

        let (collapsed, scope, active_crate, local_cnt, playlists, auth, tokens) = {
            let s = bridge.store.borrow();
            (
                s.is_nav_collapsed(),
                s.view.crate_view.active_scope,
                s.view.crate_view.active_crate.clone(),
                s.view.local_track_count,
                s.view.custom_playlists.clone(),
                s.view.auth_state.clone(),
                s.tokens.clone(),
            )
        };

        self.root.set_size_request(if collapsed { 56 } else { 212 }, -1);
        let fg_pri = AdaptiveThemeTokens::hex(tokens.fg_primary);
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);

        if !collapsed {
            let hdr = gtk4::Label::new(Some("Library"));
            hdr.set_css_classes(&["section-kicker"]);
            hdr.set_xalign(0.0);
            hdr.set_margin_start(6);
            hdr.set_margin_bottom(2);
            self.content_box.append(&hdr);
        }

        let lib_items = [
            (LibraryScope::Home, CrateRef::Home, "Home", "home", icons::HOME, None),
            (
                LibraryScope::LocalMusic,
                CrateRef::LocalTracks,
                "Local Music",
                "local_disc",
                icons::LOCAL_DISC,
                (local_cnt > 0).then(|| local_cnt.to_string()),
            ),
            (
                LibraryScope::UnifiedLikes,
                CrateRef::UnifiedLikes,
                "Favorites",
                "heart_filled",
                icons::HEART_FILLED,
                None,
            ),
            (
                LibraryScope::CustomPlaylists,
                CrateRef::CustomPlaylistsIndex,
                "Playlists",
                "playlist",
                icons::PLAYLIST,
                None,
            ),
            (
                LibraryScope::Recent,
                CrateRef::Recent,
                "Recently Played",
                "history_recent",
                icons::HISTORY_RECENT,
                None,
            ),
        ];

        for (item_scope, target, label, ikey, svg, badge_txt) in lib_items {
            let active = scope == item_scope;
            let btn = build_nav_item_btn(
                bridge,
                collapsed,
                active,
                label,
                ikey,
                svg,
                badge_txt.as_deref(),
                if active { &fg_pri } else { &fg_sec },
            );
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                b.record_action(format!("nav:{label}"));
                (b.send)(UiCommand::OpenCrate(target.clone()));
            });
            self.content_box.append(&btn);
        }

        if !collapsed {
            // Custom playlists inline shortcuts + New / Import buttons
            let pl_tools = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
            pl_tools.set_margin_top(2);
            let new_btn = gtk4::Button::with_label("+ New Playlist");
            new_btn.set_css_classes(&["ghost-btn"]);
            new_btn.set_hexpand(true);
            make_clickable(&new_btn, "Create cross-source playlist");
            let b_np = bridge.clone();
            new_btn.connect_clicked(move |_| {
                let b_c = b_np.clone();
                (b_np.prompt_text_dialog)(
                    "Create Cross-Source Playlist".to_string(),
                    "Playlist name".to_string(),
                    String::new(),
                    Rc::new(move |name| {
                        (b_c.send)(UiCommand::CreateCustomPlaylist(name));
                    }),
                );
            });

            let imp_btn = gtk4::Button::with_label("Import");
            imp_btn.set_css_classes(&["ghost-btn"]);
            make_clickable(&imp_btn, "Import .m3u / .m3u8 playlist");
            let b_im = bridge.clone();
            imp_btn.connect_clicked(move |_| (b_im.import_m3u_dialog)());
            pl_tools.append(&new_btn);
            pl_tools.append(&imp_btn);
            self.content_box.append(&pl_tools);

            for pl in playlists.iter().take(6) {
                let is_pl_active = matches!(&active_crate, CrateRef::CustomPlaylist { id, .. } if id == &pl.id);
                let btn = gtk4::Button::new();
                btn.set_css_classes(if is_pl_active { &["nav-btn", "active"] } else { &["nav-btn"] });
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                let pl_ic = bridge.icons.make_image("playlist", icons::PLAYLIST, &fg_sec, 14);
                let lbl = gtk4::Label::new(Some(&pl.name));
                lbl.set_hexpand(true);
                lbl.set_xalign(0.0);
                lbl.set_ellipsize(pango::EllipsizeMode::End);
                let cnt = gtk4::Label::new(Some(&pl.track_count.to_string()));
                cnt.set_css_classes(&["mono", "text-muted"]);
                row.append(&pl_ic);
                row.append(&lbl);
                row.append(&cnt);
                btn.set_child(Some(&row));
                make_clickable(&btn, &pl.name);
                let pid = pl.id.clone();
                let pname = pl.name.clone();
                let b_p = bridge.clone();
                btn.connect_clicked(move |_| {
                    (b_p.send)(UiCommand::OpenCrate(CrateRef::CustomPlaylist {
                        id: pid.clone(),
                        name: pname.clone(),
                    }));
                });
                self.content_box.append(&btn);
            }

            let st_hdr = gtk4::Label::new(Some("Streaming"));
            st_hdr.set_css_classes(&["section-kicker"]);
            st_hdr.set_xalign(0.0);
            st_hdr.set_margin_top(10);
            st_hdr.set_margin_start(6);
            st_hdr.set_margin_bottom(2);
            self.content_box.append(&st_hdr);
        }

        let stream_items = [
            (
                LibraryScope::Search,
                CrateRef::Search {
                    query: String::new(),
                    source_filter: None,
                },
                "Search All Sources",
                "search",
                icons::SEARCH,
            ),
            (
                LibraryScope::LikedSongs,
                CrateRef::LikedSongs,
                "Spotify Liked",
                "spotify_mark",
                icons::SPOTIFY_MARK,
            ),
            (
                LibraryScope::Playlists,
                CrateRef::PlaylistsIndex,
                "Spotify Playlists",
                "playlist",
                icons::PLAYLIST,
            ),
            (
                LibraryScope::Albums,
                CrateRef::AlbumsIndex,
                "Spotify Albums",
                "music_note",
                icons::MUSIC_NOTE,
            ),
            (
                LibraryScope::Artists,
                CrateRef::ArtistsIndex,
                "Spotify Artists",
                "spotify_mark",
                icons::SPOTIFY_MARK,
            ),
        ];

        for (item_scope, target, label, ikey, svg) in stream_items {
            let active = scope == item_scope;
            let btn = build_nav_item_btn(
                bridge,
                collapsed,
                active,
                label,
                ikey,
                svg,
                None,
                if active { &fg_pri } else { &fg_sec },
            );
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                b.record_action(format!("nav:{label}"));
                if item_scope == LibraryScope::Search {
                    (b.focus_omnibox)(true);
                    (b.send)(UiCommand::SetScope(LibraryScope::Search));
                } else {
                    (b.send)(UiCommand::OpenCrate(target.clone()));
                }
            });
            self.content_box.append(&btn);
        }

        if !collapsed {
            let spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            spacer.set_vexpand(true);
            self.content_box.append(&spacer);

            let acc_btn = gtk4::Button::new();
            acc_btn.set_css_classes(&["ctrl-btn"]);
            acc_btn.set_margin_top(8);
            let label_txt = match &auth {
                AuthState::Authenticated { display_name, .. } => format!("Spotify: {display_name}"),
                AuthState::WaitingOAuth { .. } => "Signing into Spotify...".to_string(),
                _ => "Connect Spotify".to_string(),
            };
            let lbl = gtk4::Label::new(Some(&label_txt));
            lbl.set_ellipsize(pango::EllipsizeMode::End);
            acc_btn.set_child(Some(&lbl));
            make_clickable(&acc_btn, "Configure Spotify account");
            let b_acc = bridge.clone();
            acc_btn.connect_clicked(move |_| {
                {
                    let mut s = b_acc.store.borrow_mut();
                    s.settings_open = true;
                    s.settings_tab = pulse_model::SettingsTab::AccountAndConnect;
                }
                (b_acc.redraw)(DirtyFlags::SETTINGS);
            });
            self.content_box.append(&acc_btn);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_nav_item_btn(
    bridge: &UiBridge,
    collapsed: bool,
    active: bool,
    label: &str,
    icon_key: &'static str,
    svg: &'static [u8],
    badge: Option<&str>,
    icon_hex: &str,
) -> gtk4::Button {
    let btn = gtk4::Button::new();
    btn.set_css_classes(if active { &["nav-btn", "active"] } else { &["nav-btn"] });
    make_clickable(&btn, label);

    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    if collapsed {
        row.set_halign(gtk4::Align::Center);
    }
    let img = bridge.icons.make_image(icon_key, svg, icon_hex, 16);
    row.append(&img);
    if !collapsed {
        let lbl = gtk4::Label::new(Some(label));
        lbl.set_hexpand(true);
        lbl.set_xalign(0.0);
        lbl.set_ellipsize(pango::EllipsizeMode::End);
        row.append(&lbl);
        if let Some(b_txt) = badge {
            let b_lbl = gtk4::Label::new(Some(b_txt));
            b_lbl.set_css_classes(&["chip-pill", "mono"]);
            row.append(&b_lbl);
        }
    }
    btn.set_child(Some(&row));
    btn
}

/// Center Content Canvas featuring virtualized track ListView + adaptive cards.
pub struct CenterCanvasWidget {
    pub root: gtk4::Box,
    top_box: gtk4::Box,
    view_stack: gtk4::Stack,
    cards_box: gtk4::Box,
    list_store: gio::ListStore,
}

impl CenterCanvasWidget {
    pub fn new(bridge: &UiBridge) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        root.add_css_class("content-canvas");
        root.set_hexpand(true);
        root.set_vexpand(true);

        let top_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);

        let list_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk4::NoSelection::new(Some(list_store.clone()));
        let factory = gtk4::SignalListItemFactory::new();
        let row_widgets_map: Rc<RefCell<HashMap<usize, Rc<RowWidgets>>>> =
            Rc::new(RefCell::new(HashMap::new()));

        let b_setup = bridge.clone();
        let map_setup = Rc::clone(&row_widgets_map);
        factory.connect_setup(move |_, obj| {
            let item = obj.downcast_ref::<gtk4::ListItem>().unwrap();
            let widgets = build_track_row_widgets(&b_setup);
            item.set_child(Some(&widgets.root));
            map_setup.borrow_mut().insert(item.as_ptr() as usize, widgets);
        });

        let b_bind = bridge.clone();
        let map_bind = Rc::clone(&row_widgets_map);
        factory.connect_bind(move |_, obj| {
            let item = obj.downcast_ref::<gtk4::ListItem>().unwrap();
            let key = item.as_ptr() as usize;
            let Some(widgets) = map_bind.borrow().get(&key).cloned() else {
                return;
            };
            if let Some(boxed) = item.item().and_then(|o| o.downcast::<glib::BoxedAnyObject>().ok())
                && let Ok(row_data) = boxed.try_borrow::<TrackRowItem>()
            {
                bind_track_row(&b_bind, &widgets, &row_data);
            }
        });

        let map_unbind = Rc::clone(&row_widgets_map);
        factory.connect_unbind(move |_, obj| {
            let item = obj.downcast_ref::<gtk4::ListItem>().unwrap();
            let key = item.as_ptr() as usize;
            if let Some(widgets) = map_unbind.borrow().get(&key) {
                *widgets.bound.borrow_mut() = None;
                *widgets.bound_cover_id.borrow_mut() = String::new();
            }
        });

        let map_tear = Rc::clone(&row_widgets_map);
        factory.connect_teardown(move |_, obj| {
            let item = obj.downcast_ref::<gtk4::ListItem>().unwrap();
            map_tear.borrow_mut().remove(&(item.as_ptr() as usize));
        });

        let list_view = gtk4::ListView::new(Some(selection), Some(factory));
        list_view.add_css_class("pulse-list");
        list_view.set_single_click_activate(false);

        // Place GtkListView directly inside ScrolledWindow for native O(visible) virtualization.
        let list_scroller = gtk4::ScrolledWindow::new();
        list_scroller.set_vexpand(true);
        list_scroller.set_hexpand(true);
        list_scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        list_scroller.set_child(Some(&list_view));

        let cards_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        let cards_scroller = gtk4::ScrolledWindow::new();
        cards_scroller.set_vexpand(true);
        cards_scroller.set_hexpand(true);
        cards_scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        cards_scroller.set_child(Some(&cards_box));

        let view_stack = gtk4::Stack::new();
        view_stack.set_vexpand(true);
        view_stack.set_hexpand(true);
        view_stack.add_named(&list_scroller, Some("tracks"));
        view_stack.add_named(&cards_scroller, Some("cards"));

        root.append(&top_box);
        root.append(&view_stack);

        Rc::new(Self {
            root,
            top_box,
            view_stack,
            cards_box,
            list_store,
        })
    }

    pub fn sync(&self, bridge: &UiBridge) {
        while let Some(c) = self.top_box.first_child() {
            self.top_box.remove(&c);
        }
        while let Some(c) = self.cards_box.first_child() {
            self.cards_box.remove(&c);
        }

        let (
            active_scope,
            active_crate,
            local_sub,
            title,
            subtitle,
            is_loading,
            error,
            local_count,
            local_dirs,
            sort_order,
            custom_playlists,
            active_track,
            auth,
            search_results,
            playlists_total,
            albums_total,
            artists_total,
            artist_detail,
            search_source_filter,
        ) = {
            let s = bridge.store.borrow();
            let cv = &s.view.crate_view;
            (
                cv.active_scope,
                cv.active_crate.clone(),
                cv.local_sub_view,
                cv.title.clone(),
                cv.subtitle.clone(),
                cv.is_loading,
                cv.error.clone(),
                s.view.local_track_count,
                s.view.config.local_music_dirs.clone(),
                cv.sort_order,
                s.view.custom_playlists.clone(),
                s.view.player_state.active_track.clone(),
                s.view.auth_state.clone(),
                cv.search_results.clone(),
                cv.playlists.total(),
                cv.albums.total(),
                cv.artists.total(),
                cv.artist_detail.clone(),
                cv.search_source_filter,
            )
        };

        // 1. Page Header
        let hdr_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        let hdr_text = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        hdr_text.set_hexpand(true);
        let h_title = gtk4::Label::new(Some(&title));
        h_title.set_css_classes(&["heading-xl"]);
        h_title.set_xalign(0.0);
        h_title.set_ellipsize(pango::EllipsizeMode::End);
        let h_sub = gtk4::Label::new(Some(&subtitle));
        h_sub.set_css_classes(&["text-secondary"]);
        h_sub.set_xalign(0.0);
        h_sub.set_ellipsize(pango::EllipsizeMode::End);
        hdr_text.append(&h_title);
        hdr_text.append(&h_sub);
        hdr_row.append(&hdr_text);

        // Context Play / Shuffle / Queue actions when crate holds playable tracks
        let tracks = bridge.store.borrow().visible_tracks();
        if !tracks.is_empty() && !matches!(active_crate, CrateRef::Home) {
            let play_all = gtk4::Button::with_label("Play All");
            play_all.set_css_classes(&["accent-btn"]);
            let cr_p = active_crate.clone();
            let b_p = bridge.clone();
            play_all.connect_clicked(move |_| {
                (b_p.send)(UiCommand::PlayCrate {
                    crate_ref: cr_p.clone(),
                    shuffle: false,
                });
            });

            let shuf_all = gtk4::Button::with_label("Shuffle");
            shuf_all.set_css_classes(&["ctrl-btn"]);
            let cr_s = active_crate.clone();
            let b_s = bridge.clone();
            shuf_all.connect_clicked(move |_| {
                (b_s.send)(UiCommand::PlayCrate {
                    crate_ref: cr_s.clone(),
                    shuffle: true,
                });
            });

            hdr_row.append(&play_all);
            hdr_row.append(&shuf_all);
        }

        if let CrateRef::CustomPlaylist { id, name } = &active_crate {
            let exp_btn = gtk4::Button::with_label("Export M3U8");
            exp_btn.set_css_classes(&["ctrl-btn"]);
            let pid_e = id.clone();
            let pnm_e = name.clone();
            let b_e = bridge.clone();
            exp_btn.connect_clicked(move |_| (b_e.export_m3u_dialog)(pid_e.clone(), pnm_e.clone()));

            let del_btn = gtk4::Button::with_label("Delete");
            del_btn.set_css_classes(&["danger-btn"]);
            let pid_d = id.clone();
            let b_d = bridge.clone();
            del_btn.connect_clicked(move |_| {
                (b_d.send)(UiCommand::DeleteCustomPlaylist(pid_d.clone()));
            });
            hdr_row.append(&exp_btn);
            hdr_row.append(&del_btn);
        }

        self.top_box.append(&hdr_row);

        // 2. Scope-specific Toolbars & Grids
        if active_scope == LibraryScope::Home {
            self.render_home_view(bridge, active_track.map(|d| d.summary), local_count, &custom_playlists, &auth);
            self.list_store.remove_all();
            self.view_stack.set_visible_child_name("cards");
            return;
        }

        if active_scope == LibraryScope::LocalMusic {
            self.render_local_toolbar(bridge, local_sub, sort_order, local_count, &local_dirs);
        }

        if active_scope == LibraryScope::Search {
            self.render_search_toolbar(bridge, search_source_filter, search_results.as_ref());
        }

        // 3. Explicit Loading / Error / Empty states
        if let Some(ref err_msg) = error {
            let err_card = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            err_card.add_css_class("surface-card");
            let txt = gtk4::Label::new(Some(err_msg));
            txt.set_css_classes(&["text-err"]);
            txt.set_wrap(true);
            txt.set_hexpand(true);
            txt.set_xalign(0.0);
            let retry = gtk4::Button::with_label("Retry");
            retry.set_css_classes(&["ctrl-btn"]);
            let cr_r = active_crate.clone();
            let b_r = bridge.clone();
            retry.connect_clicked(move |_| (b_r.send)(UiCommand::OpenCrate(cr_r.clone())));
            err_card.append(&txt);
            err_card.append(&retry);
            self.top_box.append(&err_card);
        }

        if is_loading && tracks.is_empty() {
            let load_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
            load_box.add_css_class("surface-card");
            let spin = gtk4::Spinner::new();
            spin.start();
            let msg = if active_scope == LibraryScope::Search {
                "Searching Local, Spotify, YouTube, and SoundCloud..."
            } else {
                "Loading library items..."
            };
            let lbl = gtk4::Label::new(Some(msg));
            lbl.set_css_classes(&["text-secondary"]);
            load_box.append(&spin);
            load_box.append(&lbl);
            self.top_box.append(&load_box);
        }

        // Index Views (PlaylistsIndex, AlbumsIndex, ArtistsIndex, CustomPlaylistsIndex, LocalAlbums, LocalArtists, LocalFolders)
        let is_index_grid = match &active_crate {
            CrateRef::PlaylistsIndex if playlists_total > 0 || !is_loading => {
                self.render_playlists_index(bridge);
                true
            }
            CrateRef::AlbumsIndex if albums_total > 0 || !is_loading => {
                self.render_albums_index(bridge);
                true
            }
            CrateRef::ArtistsIndex if artists_total > 0 || !is_loading => {
                self.render_artists_index(bridge);
                true
            }
            CrateRef::CustomPlaylistsIndex => {
                self.render_custom_playlists_grid(bridge, &custom_playlists);
                true
            }
            CrateRef::LocalAlbums => {
                self.render_local_albums_grid(bridge);
                true
            }
            CrateRef::LocalArtists => {
                self.render_local_artists_grid(bridge);
                true
            }
            CrateRef::LocalFolders => {
                self.render_local_folders_list(bridge, &local_dirs);
                true
            }
            _ => false,
        };

        if let Some(detail) = artist_detail
            && matches!(active_crate, CrateRef::Artist { .. })
            && !detail.albums.is_empty()
        {
            self.render_artist_albums_strip(bridge, &detail.albums);
        }

        if is_index_grid {
            self.list_store.remove_all();
            self.view_stack.set_visible_child_name("cards");
            return;
        }

        // Track Empty State
        if tracks.is_empty() && !is_loading {
            self.list_store.remove_all();
            self.render_empty_track_state(bridge, &active_crate, local_count, &auth);
            self.view_stack.set_visible_child_name("cards");
            return;
        }

        // Populate virtualized GtkListView cleanly
        self.view_stack.set_visible_child_name("tracks");
        let boxed_items: Vec<glib::BoxedAnyObject> = tracks
            .into_iter()
            .map(|(index, track)| glib::BoxedAnyObject::new(TrackRowItem { index, track }))
            .collect();
        self.list_store.splice(0, self.list_store.n_items(), &boxed_items);
    }

    fn render_home_view(
        &self,
        bridge: &UiBridge,
        active_track: Option<TrackSummary>,
        local_count: usize,
        custom_playlists: &[pulse_model::CustomPlaylistSummary],
        auth: &AuthState,
    ) {
        if let Some(tr) = active_track {
            let resume_card = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            resume_card.add_css_class("surface-card");
            let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            meta.set_hexpand(true);
            let kicker = gtk4::Label::new(Some("Session Ready"));
            kicker.set_css_classes(&["section-kicker"]);
            kicker.set_xalign(0.0);
            let title = gtk4::Label::new(Some(&tr.title));
            title.set_css_classes(&["heading-lg"]);
            title.set_xalign(0.0);
            title.set_ellipsize(pango::EllipsizeMode::End);
            let sub = gtk4::Label::new(Some(&format!(
                "{} · {} · {}",
                tr.artist,
                tr.source.display_name(),
                format_duration_ms(tr.duration_ms)
            )));
            sub.set_css_classes(&["text-secondary", "mono"]);
            sub.set_xalign(0.0);
            meta.append(&kicker);
            meta.append(&title);
            meta.append(&sub);

            let res_btn = gtk4::Button::with_label("Resume Playback");
            res_btn.set_css_classes(&["accent-btn"]);
            res_btn.set_valign(gtk4::Align::Center);
            let b_r = bridge.clone();
            res_btn.connect_clicked(move |_| (b_r.send)(UiCommand::TogglePlayPause));

            resume_card.append(&meta);
            resume_card.append(&res_btn);
            self.cards_box.append(&resume_card);
        }

        // Quick Actions Grid
        let qa_title = gtk4::Label::new(Some("Quick Actions"));
        qa_title.set_css_classes(&["section-kicker"]);
        qa_title.set_xalign(0.0);
        self.cards_box.append(&qa_title);

        let grid = gtk4::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(10);
        grid.set_column_homogeneous(true);
        let cols = if bridge.store.borrow().window_width < 820 { 1 } else { 2 };

        type HomeCardSpec = (&'static str, &'static str, Box<dyn Fn(&UiBridge)>);
        let cards: Vec<HomeCardSpec> = vec![
            (
                "Add Music Folder",
                "Scan local FLAC, MP3, Ogg, Opus, M4A, or WAV directories",
                Box::new(|b| (b.open_folder_dialog)()),
            ),
            (
                "Open Audio Files",
                "Play one or more local files immediately",
                Box::new(|b| (b.open_files_dialog)()),
            ),
            (
                "Search YouTube",
                "Stream full-length audio or open playlist URLs without a browser",
                Box::new(|b| {
                    (b.store.borrow_mut().view.crate_view.search_source_filter) =
                        Some(MediaSource::YouTube);
                    (b.focus_omnibox)(true);
                    (b.send)(UiCommand::SetScope(LibraryScope::Search));
                }),
            ),
            (
                "Search SoundCloud",
                "Discover tracks, mixes, and sets across SoundCloud",
                Box::new(|b| {
                    (b.store.borrow_mut().view.crate_view.search_source_filter) =
                        Some(MediaSource::SoundCloud);
                    (b.focus_omnibox)(true);
                    (b.send)(UiCommand::SetScope(LibraryScope::Search));
                }),
            ),
            (
                "New Cross-Source Playlist",
                "Mix Local, Spotify, YouTube, and SoundCloud in one playlist",
                Box::new(|b| {
                    let b_c = b.clone();
                    (b.prompt_text_dialog)(
                        "New Cross-Source Playlist".to_string(),
                        "Playlist name".to_string(),
                        String::new(),
                        Rc::new(move |name| (b_c.send)(UiCommand::CreateCustomPlaylist(name))),
                    );
                }),
            ),
            (
                if matches!(auth, AuthState::Authenticated { .. }) {
                    "Browse Spotify Library"
                } else {
                    "Connect Spotify Account"
                },
                "Sync Liked Songs, Playlists, and Connect devices",
                Box::new(|b| {
                    if matches!(b.store.borrow().view.auth_state, AuthState::Authenticated { .. }) {
                        (b.send)(UiCommand::OpenCrate(CrateRef::LikedSongs));
                    } else {
                        b.store.borrow_mut().settings_open = true;
                        b.store.borrow_mut().settings_tab =
                            pulse_model::SettingsTab::AccountAndConnect;
                        (b.redraw)(DirtyFlags::SETTINGS);
                    }
                }),
            ),
        ];

        for (idx, (title, desc, action)) in cards.into_iter().enumerate() {
            let btn = gtk4::Button::new();
            btn.set_css_classes(&["surface-card"]);
            btn.set_hexpand(true);
            let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            let t = gtk4::Label::new(Some(title));
            t.set_css_classes(&["heading-md"]);
            t.set_xalign(0.0);
            t.set_ellipsize(pango::EllipsizeMode::End);
            let d = gtk4::Label::new(Some(desc));
            d.set_css_classes(&["text-secondary"]);
            d.set_wrap(true);
            d.set_xalign(0.0);
            inner.append(&t);
            inner.append(&d);
            btn.set_child(Some(&inner));
            make_clickable(&btn, title);
            let b_c = bridge.clone();
            btn.connect_clicked(move |_| {
                b_c.record_action(format!("home:{title}"));
                action(&b_c);
            });
            let c = (idx % cols) as i32;
            let r = (idx / cols) as i32;
            grid.attach(&btn, c, r, 1, 1);
        }
        self.cards_box.append(&grid);

        // Library overview & custom playlists
        let lib_card = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
        lib_card.add_css_class("surface-elevated");
        let l_txt = gtk4::Label::new(Some(&format!(
            "Local Library: {local_count} tracks  ·  Cross-Source Playlists: {}",
            custom_playlists.len()
        )));
        l_txt.set_css_classes(&["mono", "text-secondary"]);
        l_txt.set_hexpand(true);
        l_txt.set_xalign(0.0);
        let go_local = gtk4::Button::with_label("Open Local Music");
        go_local.set_css_classes(&["ctrl-btn"]);
        let b_loc = bridge.clone();
        go_local.connect_clicked(move |_| {
            (b_loc.send)(UiCommand::OpenCrate(CrateRef::LocalTracks));
        });
        lib_card.append(&l_txt);
        lib_card.append(&go_local);
        self.cards_box.append(&lib_card);
    }

    fn render_local_toolbar(
        &self,
        bridge: &UiBridge,
        sub: LocalSubView,
        sort: SortOrder,
        _count: usize,
        _dirs: &[String],
    ) {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        for (lbl, target, sv) in [
            ("Tracks", CrateRef::LocalTracks, LocalSubView::Tracks),
            ("Albums", CrateRef::LocalAlbums, LocalSubView::Albums),
            ("Artists", CrateRef::LocalArtists, LocalSubView::Artists),
            ("Folders", CrateRef::LocalFolders, LocalSubView::Folders),
        ] {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if sub == sv {
                &["chip-pill", "active"]
            } else {
                &["chip-pill"]
            });
            make_clickable(&btn, &format!("View Local {lbl}"));
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                b.record_action(format!("local:sub:{lbl}"));
                (b.send)(UiCommand::OpenCrate(target.clone()));
            });
            row.append(&btn);
        }

        let sp = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        sp.set_hexpand(true);
        row.append(&sp);

        let add_f = gtk4::Button::with_label("+ Add Folder");
        add_f.set_css_classes(&["ctrl-btn"]);
        let b_af = bridge.clone();
        add_f.connect_clicked(move |_| (b_af.open_folder_dialog)());

        let open_f = gtk4::Button::with_label("Open Files");
        open_f.set_css_classes(&["ctrl-btn"]);
        let b_of = bridge.clone();
        open_f.connect_clicked(move |_| (b_of.open_files_dialog)());

        let rescan = gtk4::Button::with_label("Rescan");
        rescan.set_css_classes(&["ghost-btn"]);
        let b_rs = bridge.clone();
        rescan.connect_clicked(move |_| (b_rs.send)(UiCommand::RescanLocalMusic));

        let next_sort = match sort {
            SortOrder::Custom => SortOrder::Title,
            SortOrder::Title => SortOrder::Artist,
            SortOrder::Artist => SortOrder::Duration,
            SortOrder::Duration => SortOrder::RecentlyAdded,
            SortOrder::RecentlyAdded => SortOrder::Custom,
        };
        let sort_btn = gtk4::Button::with_label(&format!("Sort: {}", sort_order_label(sort)));
        sort_btn.set_css_classes(&["ghost-btn"]);
        let b_so = bridge.clone();
        sort_btn.connect_clicked(move |_| (b_so.send)(UiCommand::SetSortOrder(next_sort)));

        row.append(&sort_btn);
        row.append(&add_f);
        row.append(&open_f);
        row.append(&rescan);
        self.top_box.append(&row);
    }

    fn render_search_toolbar(
        &self,
        bridge: &UiBridge,
        current_filter: Option<MediaSource>,
        results: Option<&pulse_model::SearchResults>,
    ) {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let opts: [(&str, Option<MediaSource>); 5] = [
            ("All Sources", None),
            ("Local", Some(MediaSource::Local)),
            ("YouTube", Some(MediaSource::YouTube)),
            ("SoundCloud", Some(MediaSource::SoundCloud)),
            ("Spotify", Some(MediaSource::Spotify)),
        ];
        for (lbl, src) in opts {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if current_filter == src {
                &["chip-pill", "active"]
            } else {
                &["chip-pill"]
            });
            make_clickable(&btn, &format!("Filter search results to {lbl}"));
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                let q = {
                    let mut s = b.store.borrow_mut();
                    s.view.crate_view.search_source_filter = src;
                    if !s.omnibox_text.trim().is_empty() {
                        s.omnibox_text.trim().to_string()
                    } else if let Some(ref res) = s.view.crate_view.search_results {
                        res.query.clone()
                    } else {
                        String::new()
                    }
                };
                b.record_action(format!("search:source:{lbl}"));
                if !q.is_empty() {
                    b.store.borrow_mut().view.crate_view.is_loading = true;
                    (b.send)(UiCommand::RunGlobalSearch {
                        query: q,
                        source_filter: src,
                    });
                    (b.redraw)(DirtyFlags::CONTENT);
                } else {
                    (b.redraw)(DirtyFlags::CONTENT);
                }
            });
            row.append(&btn);
        }
        self.top_box.append(&row);

        if let Some(res) = results
            && (!res.playlists.is_empty() || !res.albums.is_empty())
        {
            let pl_strip = gtk4::FlowBox::new();
            pl_strip.set_selection_mode(gtk4::SelectionMode::None);
            pl_strip.set_column_spacing(8);
            pl_strip.set_row_spacing(6);
            pl_strip.set_min_children_per_line(1);
            pl_strip.set_max_children_per_line(4);

            for pl in res.playlists.iter().take(4) {
                let btn = gtk4::Button::new();
                btn.set_css_classes(&["surface-elevated"]);
                let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
                let n = gtk4::Label::new(Some(&pl.name));
                n.set_css_classes(&["heading-md"]);
                n.set_xalign(0.0);
                n.set_ellipsize(pango::EllipsizeMode::End);
                let s = gtk4::Label::new(Some(&format!(
                    "{} Playlist · {}",
                    pl.source.display_name(),
                    pl.owner
                )));
                s.set_css_classes(&["text-muted"]);
                s.set_xalign(0.0);
                s.set_ellipsize(pango::EllipsizeMode::End);
                inner.append(&n);
                inner.append(&s);
                btn.set_child(Some(&inner));
                let target = if pl.source == MediaSource::SoundCloud {
                    CrateRef::SoundCloudSet {
                        url: pl.uri.to_string(),
                        title: pl.name.clone(),
                    }
                } else {
                    CrateRef::Playlist {
                        uri: pl.uri.clone(),
                        name: pl.name.clone(),
                    }
                };
                let b_p = bridge.clone();
                btn.connect_clicked(move |_| (b_p.send)(UiCommand::OpenCrate(target.clone())));
                pl_strip.insert(&btn, -1);
            }
            self.top_box.append(&pl_strip);
        }
    }

    fn render_empty_track_state(
        &self,
        bridge: &UiBridge,
        active_crate: &CrateRef,
        local_count: usize,
        auth: &AuthState,
    ) {
        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        card.add_css_class("surface-card");
        card.set_margin_top(12);

        let (heading, body, action_label) = match active_crate {
            CrateRef::LocalTracks | CrateRef::LocalAlbums | CrateRef::LocalArtists | CrateRef::LocalFolders => {
                if local_count == 0 {
                    (
                        "No local audio folders added",
                        "Add a folder containing FLAC, MP3, Ogg, Opus, M4A, WAV, or ALAC files, or drag and drop audio files into Pulse.",
                        Some("Add Music Folder"),
                    )
                } else {
                    (
                        "No local tracks match your filter",
                        "Clear or change your filter in the header bar to show all indexed local tracks.",
                        None,
                    )
                }
            }
            CrateRef::Search { query, .. } => {
                if query.is_empty() {
                    (
                        "Type a query above to search all sources",
                        "Press Ctrl+K and enter any artist, song title, YouTube URL, SoundCloud link, or Spotify URI.",
                        None,
                    )
                } else {
                    (
                        "No matches found across active sources",
                        "Try broader keywords or switch the source filter chip to All Sources.",
                        None,
                    )
                }
            }
            CrateRef::LikedSongs | CrateRef::PlaylistsIndex | CrateRef::AlbumsIndex | CrateRef::ArtistsIndex => {
                if !matches!(auth, AuthState::Authenticated { .. }) {
                    (
                        "Spotify account not connected",
                        "Connect Spotify in Settings to sync Liked Songs and Playlists, or browse Local Music, YouTube, and SoundCloud right away.",
                        Some("Open Spotify Settings"),
                    )
                } else {
                    (
                        "No items found in this Spotify crate",
                        "This Spotify collection returned zero tracks or requires Web API authorization.",
                        None,
                    )
                }
            }
            _ => (
                "No tracks in this view yet",
                "Search tracks, add a local folder, or drop files/URLs to start listening.",
                None,
            ),
        };

        let h = gtk4::Label::new(Some(heading));
        h.set_css_classes(&["heading-lg"]);
        h.set_xalign(0.0);
        let b = gtk4::Label::new(Some(body));
        b.set_css_classes(&["text-secondary"]);
        b.set_wrap(true);
        b.set_xalign(0.0);
        card.append(&h);
        card.append(&b);

        if let Some(act) = action_label {
            let btns = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let btn = gtk4::Button::with_label(act);
            btn.set_css_classes(&["accent-btn"]);
            let b_c = bridge.clone();
            let is_local = act == "Add Music Folder";
            btn.connect_clicked(move |_| {
                if is_local {
                    (b_c.open_folder_dialog)();
                } else {
                    {
                        let mut s = b_c.store.borrow_mut();
                        s.settings_open = true;
                        s.settings_tab = pulse_model::SettingsTab::AccountAndConnect;
                    }
                    (b_c.redraw)(DirtyFlags::SETTINGS);
                }
            });
            btns.append(&btn);

            if is_local {
                let open_btn = gtk4::Button::with_label("Open Audio Files");
                open_btn.set_css_classes(&["ctrl-btn"]);
                let b_f = bridge.clone();
                open_btn.connect_clicked(move |_| (b_f.open_files_dialog)());
                btns.append(&open_btn);
            }
            card.append(&btns);
        }

        self.cards_box.append(&card);
    }

    fn render_custom_playlists_grid(
        &self,
        bridge: &UiBridge,
        playlists: &[pulse_model::CustomPlaylistSummary],
    ) {
        let tools = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let new_btn = gtk4::Button::with_label("+ New Playlist");
        new_btn.set_css_classes(&["accent-btn"]);
        let b_np = bridge.clone();
        new_btn.connect_clicked(move |_| {
            let b_c = b_np.clone();
            (b_np.prompt_text_dialog)(
                "New Cross-Source Playlist".to_string(),
                "Playlist name".to_string(),
                String::new(),
                Rc::new(move |name| (b_c.send)(UiCommand::CreateCustomPlaylist(name))),
            );
        });
        let imp_btn = gtk4::Button::with_label("Import .m3u / .m3u8");
        imp_btn.set_css_classes(&["ctrl-btn"]);
        let b_i = bridge.clone();
        imp_btn.connect_clicked(move |_| (b_i.import_m3u_dialog)());
        tools.append(&new_btn);
        tools.append(&imp_btn);
        self.cards_box.append(&tools);

        if playlists.is_empty() {
            let empty = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            empty.add_css_class("surface-card");
            let t = gtk4::Label::new(Some("No cross-source playlists created yet"));
            t.set_css_classes(&["heading-md"]);
            t.set_xalign(0.0);
            empty.append(&t);
            self.cards_box.append(&empty);
            return;
        }

        let grid = gtk4::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(10);
        grid.set_column_homogeneous(true);
        let cols = if bridge.store.borrow().window_width < 820 { 1 } else { 2 };

        for (idx, pl) in playlists.iter().enumerate() {
            let btn = gtk4::Button::new();
            btn.set_css_classes(&["surface-card"]);
            let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
            let n = gtk4::Label::new(Some(&pl.name));
            n.set_css_classes(&["heading-md"]);
            n.set_xalign(0.0);
            let m = gtk4::Label::new(Some(&format!("{} tracks", pl.track_count)));
            m.set_css_classes(&["mono", "text-muted"]);
            m.set_xalign(0.0);
            inner.append(&n);
            inner.append(&m);
            btn.set_child(Some(&inner));
            let pid = pl.id.clone();
            let pname = pl.name.clone();
            let b_c = bridge.clone();
            btn.connect_clicked(move |_| {
                (b_c.send)(UiCommand::OpenCrate(CrateRef::CustomPlaylist {
                    id: pid.clone(),
                    name: pname.clone(),
                }));
            });
            grid.attach(&btn, (idx % cols) as i32, (idx / cols) as i32, 1, 1);
        }
        self.cards_box.append(&grid);
    }

    fn render_playlists_index(&self, bridge: &UiBridge) {
        let items: Vec<_> = bridge
            .store
            .borrow()
            .view
            .crate_view
            .playlists
            .iter_loaded()
            .map(|(_, p)| p.clone())
            .collect();
        let grid = gtk4::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(10);
        grid.set_column_homogeneous(true);
        let cols = if bridge.store.borrow().window_width < 820 { 1 } else { 2 };
        for (idx, pl) in items.into_iter().enumerate() {
            let btn = gtk4::Button::new();
            btn.set_css_classes(&["surface-card"]);
            let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
            let n = gtk4::Label::new(Some(&pl.name));
            n.set_css_classes(&["heading-md"]);
            n.set_xalign(0.0);
            n.set_ellipsize(pango::EllipsizeMode::End);
            let o = gtk4::Label::new(Some(&format!("{} · {} tracks", pl.owner, pl.track_count)));
            o.set_css_classes(&["text-muted", "mono"]);
            o.set_xalign(0.0);
            inner.append(&n);
            inner.append(&o);
            btn.set_child(Some(&inner));
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                (b.send)(UiCommand::OpenCrate(CrateRef::Playlist {
                    uri: pl.uri.clone(),
                    name: pl.name.clone(),
                }));
            });
            grid.attach(&btn, (idx % cols) as i32, (idx / cols) as i32, 1, 1);
        }
        self.cards_box.append(&grid);
    }

    fn render_albums_index(&self, bridge: &UiBridge) {
        let items: Vec<_> = bridge
            .store
            .borrow()
            .view
            .crate_view
            .albums
            .iter_loaded()
            .map(|(_, a)| a.clone())
            .collect();
        self.render_album_flow(bridge, &items, false);
    }

    fn render_local_albums_grid(&self, bridge: &UiBridge) {
        let items: Vec<_> = bridge
            .store
            .borrow()
            .view
            .crate_view
            .albums
            .iter_loaded()
            .map(|(_, a)| a.clone())
            .collect();
        self.render_album_flow(bridge, &items, true);
    }

    fn render_album_flow(&self, bridge: &UiBridge, items: &[pulse_model::AlbumSummary], local: bool) {
        let grid = gtk4::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(10);
        grid.set_column_homogeneous(true);
        let cols = if bridge.store.borrow().window_width < 820 { 1 } else { 2 };
        for (idx, al) in items.iter().enumerate() {
            let btn = gtk4::Button::new();
            btn.set_css_classes(&["surface-card"]);
            let inner = gtk4::Box::new(gtk4::Orientation::Vertical, 3);
            let t = gtk4::Label::new(Some(&al.title));
            t.set_css_classes(&["heading-md"]);
            t.set_xalign(0.0);
            t.set_ellipsize(pango::EllipsizeMode::End);
            let s = gtk4::Label::new(Some(&format!("{} · {} tracks", al.artist, al.track_count)));
            s.set_css_classes(&["text-muted", "mono"]);
            s.set_xalign(0.0);
            s.set_ellipsize(pango::EllipsizeMode::End);
            inner.append(&t);
            inner.append(&s);
            btn.set_child(Some(&inner));
            let target = if local {
                CrateRef::LocalAlbum {
                    title: al.title.clone(),
                    artist: al.artist.to_string(),
                }
            } else {
                CrateRef::Album {
                    uri: al.uri.clone(),
                    title: al.title.clone(),
                }
            };
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::OpenCrate(target.clone())));
            grid.attach(&btn, (idx % cols) as i32, (idx / cols) as i32, 1, 1);
        }
        self.cards_box.append(&grid);
    }

    fn render_artist_albums_strip(&self, bridge: &UiBridge, albums: &[pulse_model::AlbumSummary]) {
        let hdr = gtk4::Label::new(Some("Albums"));
        hdr.set_css_classes(&["section-kicker"]);
        hdr.set_xalign(0.0);
        self.top_box.append(&hdr);
        let flow = gtk4::FlowBox::new();
        flow.set_selection_mode(gtk4::SelectionMode::None);
        flow.set_column_spacing(8);
        flow.set_min_children_per_line(1);
        flow.set_max_children_per_line(4);
        for al in &albums[..albums.len().min(4)] {
            let btn = gtk4::Button::with_label(&al.title);
            btn.set_css_classes(&["ctrl-btn"]);
            let uri = al.uri.clone();
            let title = al.title.clone();
            let b = bridge.clone();
            btn.connect_clicked(move |_| {
                (b.send)(UiCommand::OpenCrate(CrateRef::Album {
                    uri: uri.clone(),
                    title: title.clone(),
                }));
            });
            flow.insert(&btn, -1);
        }
        self.top_box.append(&flow);
    }

    fn render_artists_index(&self, bridge: &UiBridge) {
        let items: Vec<_> = bridge
            .store
            .borrow()
            .view
            .crate_view
            .artists
            .iter_loaded()
            .map(|(_, a)| a.clone())
            .collect();
        self.render_artist_flow(bridge, &items, false);
    }

    fn render_local_artists_grid(&self, bridge: &UiBridge) {
        let items: Vec<_> = bridge
            .store
            .borrow()
            .view
            .crate_view
            .artists
            .iter_loaded()
            .map(|(_, a)| a.clone())
            .collect();
        self.render_artist_flow(bridge, &items, true);
    }

    fn render_artist_flow(&self, bridge: &UiBridge, items: &[pulse_model::ArtistSummary], local: bool) {
        let flow = gtk4::FlowBox::new();
        flow.set_selection_mode(gtk4::SelectionMode::None);
        flow.set_column_spacing(10);
        flow.set_row_spacing(10);
        flow.set_min_children_per_line(1);
        flow.set_max_children_per_line(4);
        for ar in items {
            let btn = gtk4::Button::with_label(&ar.name);
            btn.set_css_classes(&["surface-card"]);
            make_clickable(&btn, &ar.name);
            let target = if local {
                CrateRef::LocalArtist {
                    name: ar.name.clone(),
                }
            } else {
                CrateRef::Artist {
                    uri: ar.uri.clone(),
                    name: ar.name.clone(),
                }
            };
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::OpenCrate(target.clone())));
            flow.insert(&btn, -1);
        }
        self.cards_box.append(&flow);
    }

    fn render_local_folders_list(&self, bridge: &UiBridge, dirs: &[String]) {
        for dir in dirs {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            row.add_css_class("surface-card");
            let lbl = gtk4::Label::new(Some(dir));
            lbl.set_css_classes(&["mono"]);
            lbl.set_hexpand(true);
            lbl.set_xalign(0.0);
            lbl.set_ellipsize(pango::EllipsizeMode::Middle);
            let open_btn = gtk4::Button::with_label("Open Tracks");
            open_btn.set_css_classes(&["ctrl-btn"]);
            let d = dir.clone();
            let b = bridge.clone();
            open_btn.connect_clicked(move |_| {
                (b.send)(UiCommand::OpenCrate(CrateRef::LocalFolder { path: d.clone() }));
            });
            row.append(&lbl);
            row.append(&open_btn);
            self.cards_box.append(&row);
        }
    }
}

fn build_track_row_widgets(bridge: &UiBridge) -> Rc<RowWidgets> {
    let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
    root.add_css_class("track-row");

    let bound: Rc<RefCell<Option<TrackRowItem>>> = Rc::new(RefCell::new(None));
    let bound_cover_id: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let fg_sec = AdaptiveThemeTokens::hex(bridge.store.borrow().tokens.fg_secondary);

    let num_lbl = gtk4::Label::new(None);
    num_lbl.set_css_classes(&["mono", "text-muted"]);
    num_lbl.set_width_chars(3);
    num_lbl.set_xalign(1.0);

    // Thumbnail frame (40x40 rounded)
    let thumb_overlay = gtk4::Overlay::new();
    thumb_overlay.add_css_class("thumb-art");
    thumb_overlay.set_size_request(40, 40);
    let thumb_fallback = bridge.icons.make_image("music_note", icons::MUSIC_NOTE, &fg_sec, 18);
    thumb_fallback.set_halign(gtk4::Align::Center);
    thumb_fallback.set_valign(gtk4::Align::Center);
    let thumb_pic = gtk4::Picture::new();
    thumb_pic.set_size_request(40, 40);
    thumb_pic.set_content_fit(gtk4::ContentFit::Cover);
    thumb_overlay.set_child(Some(&thumb_fallback));
    thumb_overlay.add_overlay(&thumb_pic);

    // Clickable main title/meta button so row click plays immediately
    let play_area = gtk4::Button::new();
    play_area.set_css_classes(&["track-play-btn"]);
    play_area.set_hexpand(true);
    play_area.set_size_request(180, -1);
    let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
    let title_lbl = gtk4::Label::new(None);
    title_lbl.set_css_classes(&["heading-md"]);
    title_lbl.set_xalign(0.0);
    title_lbl.set_ellipsize(pango::EllipsizeMode::End);
    let sub_lbl = gtk4::Label::new(None);
    sub_lbl.set_css_classes(&["text-secondary"]);
    sub_lbl.set_xalign(0.0);
    sub_lbl.set_ellipsize(pango::EllipsizeMode::End);
    meta.append(&title_lbl);
    meta.append(&sub_lbl);
    play_area.set_child(Some(&meta));
    make_clickable(&play_area, "Play track");

    let bound_p = Rc::clone(&bound);
    let b_p = bridge.clone();
    play_area.connect_clicked(move |_| {
        if let Some(ref item) = *bound_p.borrow() {
            trigger_play_row(&b_p, item);
        }
    });

    let source_chip = gtk4::Label::new(None);
    source_chip.add_css_class("chip-pill");
    source_chip.set_valign(gtk4::Align::Center);

    let dur_lbl = gtk4::Label::new(None);
    dur_lbl.set_css_classes(&["mono", "text-secondary"]);
    dur_lbl.set_width_chars(5);
    dur_lbl.set_xalign(1.0);
    dur_lbl.set_valign(gtk4::Align::Center);

    let queue_btn = gtk4::Button::with_label("+ Next");
    queue_btn.set_css_classes(&["ghost-btn"]);
    make_clickable(&queue_btn, "Play next");
    let bound_q = Rc::clone(&bound);
    let b_q = bridge.clone();
    queue_btn.connect_clicked(move |_| {
        if let Some(ref item) = *bound_q.borrow() {
            (b_q.send)(UiCommand::EnqueueNext(item.track.clone()));
        }
    });

    let add_pl_btn = gtk4::Button::new();
    let add_img = bridge.icons.make_image("plus", icons::PLUS, &fg_sec, 14);
    add_pl_btn.set_child(Some(&add_img));
    add_pl_btn.set_css_classes(&["ghost-btn"]);
    make_clickable(&add_pl_btn, "Add to playlist");
    let bound_pl = Rc::clone(&bound);
    let b_pl = bridge.clone();
    let add_btn_ref = add_pl_btn.clone();
    add_pl_btn.connect_clicked(move |_| {
        if let Some(ref item) = *bound_pl.borrow() {
            open_add_to_playlist_popover(&add_btn_ref, &b_pl, item.track.clone());
        }
    });

    let (like_btn, like_img) = vector_icon_button(
        &bridge.icons,
        "heart_outline",
        icons::HEART_OUTLINE,
        &fg_sec,
        15,
        "Toggle favorite",
        &["ghost-btn"],
    );
    let bound_l = Rc::clone(&bound);
    let b_l = bridge.clone();
    like_btn.connect_clicked(move |_| {
        if let Some(ref item) = *bound_l.borrow() {
            (b_l.send)(UiCommand::ToggleLikeTrack(item.track.clone()));
        }
    });

    root.append(&num_lbl);
    root.append(&thumb_overlay);
    root.append(&play_area);
    root.append(&source_chip);
    root.append(&queue_btn);
    root.append(&add_pl_btn);
    root.append(&like_btn);
    root.append(&dur_lbl);

    Rc::new(RowWidgets {
        root,
        num_lbl,
        thumb_pic,
        thumb_fallback,
        title_lbl,
        sub_lbl,
        source_chip,
        dur_lbl,
        like_img,
        bound,
        bound_cover_id,
    })
}

fn bind_track_row(bridge: &UiBridge, w: &RowWidgets, item: &TrackRowItem) {
    *w.bound.borrow_mut() = Some(item.clone());
    w.num_lbl.set_text(&format!("{}", item.index + 1));
    w.title_lbl.set_text(&item.track.title);
    let subtitle = match item.track.codec_label {
        Some(ref codec) => format!("{} · {} · {}", item.track.artist, item.track.album, codec),
        None => format!("{} · {}", item.track.artist, item.track.album),
    };
    w.sub_lbl.set_text(&subtitle);

    for cls in ["source-local", "source-spotify", "source-youtube", "source-soundcloud"] {
        w.source_chip.remove_css_class(cls);
    }
    w.source_chip.add_css_class(source_badge_css_class(item.track.source));
    w.source_chip.set_text(item.track.source.badge_label());

    w.dur_lbl.set_text(&format_duration_ms(item.track.duration_ms));

    let (active_uri, selected_idx, tokens, loaded_cnt) = {
        let s = bridge.store.borrow();
        (
            s.view
                .player_state
                .active_track
                .as_ref()
                .map(|t| t.summary.uri.clone()),
            s.selected_track_index,
            s.tokens.clone(),
            s.view.crate_view.loaded_tracks.loaded_count(),
        )
    };

    if active_uri.as_ref().is_some_and(|u| u == &item.track.uri) {
        w.root.add_css_class("is-playing");
    } else {
        w.root.remove_css_class("is-playing");
    }
    if selected_idx == Some(item.index) {
        w.root.add_css_class("is-selected");
    } else {
        w.root.remove_css_class("is-selected");
    }

    let heart_hex = if item.track.is_liked {
        AdaptiveThemeTokens::hex(tokens.accent)
    } else {
        AdaptiveThemeTokens::hex(tokens.fg_secondary)
    };
    let (h_key, h_svg) = if item.track.is_liked {
        ("heart_filled", icons::HEART_FILLED)
    } else {
        ("heart_outline", icons::HEART_OUTLINE)
    };
    bridge
        .icons
        .apply_to_image(&w.like_img, h_key, h_svg, &heart_hex, 15);

    if let Some(ref cover) = item.track.cover {
        let cid = cover.id.to_string();
        *w.bound_cover_id.borrow_mut() = cid.clone();
        if let Some(tex) = bridge.covers.cached_texture(&cover.id, CoverTier::Thumb) {
            w.thumb_pic.set_paintable(Some(&tex));
            w.thumb_pic.set_visible(true);
            w.thumb_fallback.set_visible(false);
        } else {
            w.thumb_pic.set_paintable(gtk4::gdk::Paintable::NONE);
            w.thumb_pic.set_visible(false);
            w.thumb_fallback.set_visible(true);
            let pic_c = w.thumb_pic.clone();
            let fb_c = w.thumb_fallback.clone();
            let bound_cid = Rc::clone(&w.bound_cover_id);
            bridge
                .covers
                .request_cover(cover.clone(), CoverTier::Thumb, loaded_cnt, move |tex, _| {
                    if *bound_cid.borrow() == cid {
                        pic_c.set_paintable(Some(&tex));
                        pic_c.set_visible(true);
                        fb_c.set_visible(false);
                    }
                });
        }
    } else {
        *w.bound_cover_id.borrow_mut() = String::new();
        w.thumb_pic.set_paintable(gtk4::gdk::Paintable::NONE);
        w.thumb_pic.set_visible(false);
        w.thumb_fallback.set_visible(true);
    }

    // Trigger next sparse page load cleanly if scrolled close to unloaded end
    let (cr_active, page_size, total, needs_fetch) = {
        let s = bridge.store.borrow();
        let cv = &s.view.crate_view;
        let ps = cv.loaded_tracks.page_size();
        let tot = cv.loaded_tracks.total();
        let next_start = cv.loaded_tracks.page_start_for(item.index.saturating_add(12));
        let needs = next_start < tot && !cv.loaded_tracks.is_page_loaded(next_start);
        (cv.active_crate.clone(), ps, tot, (needs, next_start))
    };
    if needs_fetch.0 && total > 0 && !matches!(cr_active, CrateRef::Search { .. }) {
        (bridge.send)(UiCommand::FetchCratePage {
            crate_ref: cr_active,
            offset: needs_fetch.1,
            limit: page_size,
        });
    }
}

fn trigger_play_row(bridge: &UiBridge, item: &TrackRowItem) {
    let (cr, title, all_tracks) = {
        let mut s = bridge.store.borrow_mut();
        s.selected_track_index = Some(item.index);
        let ctx_tracks: Vec<TrackSummary> = s
            .visible_tracks()
            .into_iter()
            .map(|(_, tr)| tr)
            .collect();
        (
            s.view.crate_view.active_crate.clone(),
            s.view.crate_view.title.clone(),
            ctx_tracks,
        )
    };
    bridge.record_action(format!("play:{}", item.track.uri));
    if item.track.source == MediaSource::Spotify
        && let Some(ctx_uri) = crate_context_uri(&cr)
    {
        (bridge.send)(UiCommand::PlayTrackInContext {
            context_uri: ctx_uri,
            track_uri: item.track.uri.to_string(),
            index: item.index,
        });
    } else {
        (bridge.send)(UiCommand::PlayTrackDirect {
            track: item.track.clone(),
            context_name: title,
            context_tracks: all_tracks,
        });
    }
    (bridge.redraw)(DirtyFlags::CONTENT);
}

fn crate_context_uri(cr: &CrateRef) -> Option<String> {
    match cr {
        CrateRef::LikedSongs => Some("spotify:user:me:collection".to_string()),
        CrateRef::Playlist { uri, .. } | CrateRef::Album { uri, .. } | CrateRef::Artist { uri, .. } => {
            Some(uri.to_string())
        }
        _ => None,
    }
}

/// Bottom Playback & Visualizer Deck (`88px`).
pub struct PlaybackDeckWidget {
    pub root: gtk4::Box,
    cover_pic: gtk4::Picture,
    cover_fb: gtk4::Image,
    title_lbl: gtk4::Label,
    artist_lbl: gtk4::Label,
    source_chip: gtk4::Label,
    codec_lbl: gtk4::Label,
    like_img: gtk4::Image,
    shuffle_btn: gtk4::Button,
    shuffle_img: gtk4::Image,
    prev_img: gtk4::Image,
    play_img: gtk4::Image,
    next_img: gtk4::Image,
    repeat_btn: gtk4::Button,
    repeat_img: gtk4::Image,
    ab_btn: gtk4::Button,
    ab_lbl: gtk4::Label,
    pos_lbl: gtk4::Label,
    dur_lbl: gtk4::Label,
    scrub_scale: gtk4::Scale,
    suppress_scrub: Rc<Cell<bool>>,
    viz: AudioVisualizerWidget,
    pub viz_mode_btn: gtk4::Button,
    viz_mode_lbl: gtk4::Label,
    viz_img: gtk4::Image,
    mute_img: gtk4::Image,
    vol_scale: gtk4::Scale,
    suppress_vol: Rc<Cell<bool>>,
    queue_btn: gtk4::Button,
    lyrics_btn: gtk4::Button,
    eq_btn: gtk4::Button,
    last_cover_id: RefCell<String>,
}

impl PlaybackDeckWidget {
    pub fn new(bridge: &UiBridge, spectrum_reader: SpectrumReadFn) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
        root.add_css_class("deck-bar");

        let tokens = bridge.store.borrow().tokens.clone();
        let fg_pri = AdaptiveThemeTokens::hex(tokens.fg_primary);
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
        let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);

        // Left Track Cluster
        let left_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        left_box.set_size_request(250, -1);

        let cover_btn = gtk4::Button::new();
        cover_btn.set_css_classes(&["ghost-btn", "thumb-art"]);
        cover_btn.set_size_request(56, 56);
        let cov_overlay = gtk4::Overlay::new();
        let cover_fb = bridge.icons.make_image("music_note", icons::MUSIC_NOTE, &fg_sec, 22);
        cover_fb.set_halign(gtk4::Align::Center);
        cover_fb.set_valign(gtk4::Align::Center);
        let cover_pic = gtk4::Picture::new();
        cover_pic.set_size_request(56, 56);
        cover_pic.set_content_fit(gtk4::ContentFit::Cover);
        cov_overlay.set_child(Some(&cover_fb));
        cov_overlay.add_overlay(&cover_pic);
        cover_btn.set_child(Some(&cov_overlay));
        make_clickable(&cover_btn, "Open Immersive Stage view (2)");
        let b_stage = bridge.clone();
        cover_btn.connect_clicked(move |_| {
            (b_stage.send)(UiCommand::SetMode(AppMode::FocusStage));
        });

        let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        meta.set_hexpand(true);
        meta.set_valign(gtk4::Align::Center);
        let title_lbl = gtk4::Label::new(Some("Ready to play"));
        title_lbl.set_css_classes(&["heading-md"]);
        title_lbl.set_xalign(0.0);
        title_lbl.set_ellipsize(pango::EllipsizeMode::End);
        let artist_lbl = gtk4::Label::new(Some("Drop audio files, paste a URL, or pick a track"));
        artist_lbl.set_css_classes(&["text-secondary"]);
        artist_lbl.set_xalign(0.0);
        artist_lbl.set_ellipsize(pango::EllipsizeMode::End);

        let badges = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let source_chip = gtk4::Label::new(Some("LOCAL"));
        source_chip.set_css_classes(&["chip-pill", "source-local"]);
        let codec_lbl = gtk4::Label::new(Some("READY"));
        codec_lbl.set_css_classes(&["mono", "text-muted"]);
        badges.append(&source_chip);
        badges.append(&codec_lbl);

        meta.append(&title_lbl);
        meta.append(&artist_lbl);
        meta.append(&badges);

        let (like_btn, like_img) = vector_icon_button(
            &bridge.icons,
            "heart_outline",
            icons::HEART_OUTLINE,
            &fg_sec,
            16,
            "Favorite track (L)",
            &["ghost-btn"],
        );
        let b_lk = bridge.clone();
        like_btn.connect_clicked(move |_| {
            if let Some(tr) = b_lk
                .store
                .borrow()
                .view
                .player_state
                .active_track
                .as_ref()
                .map(|t| t.summary.clone())
            {
                (b_lk.send)(UiCommand::ToggleLikeTrack(tr));
            }
        });

        left_box.append(&cover_btn);
        left_box.append(&meta);
        left_box.append(&like_btn);

        // Center Transport & Scrubber Cluster
        let center_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        center_box.set_hexpand(true);
        center_box.set_valign(gtk4::Align::Center);

        let transport = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        transport.set_halign(gtk4::Align::Center);

        let (shuffle_btn, shuffle_img) = vector_icon_button(
            &bridge.icons,
            "shuffle",
            icons::SHUFFLE,
            &fg_sec,
            15,
            "Shuffle (S)",
            &["ghost-btn"],
        );
        let b_sh = bridge.clone();
        shuffle_btn.connect_clicked(move |_| (b_sh.send)(UiCommand::ToggleShuffle));

        let (prev_btn, prev_img) = vector_icon_button(
            &bridge.icons,
            "prev",
            icons::PREV,
            &fg_pri,
            16,
            "Previous track ([)",
            &["ghost-btn"],
        );
        let b_pr = bridge.clone();
        prev_btn.connect_clicked(move |_| (b_pr.send)(UiCommand::PrevTrack));

        let (play_btn, play_img) = vector_icon_button(
            &bridge.icons,
            "play",
            icons::PLAY,
            &on_acc,
            18,
            "Play / Pause (Space)",
            &["accent-btn"],
        );
        let b_pl = bridge.clone();
        play_btn.connect_clicked(move |_| {
            b_pl.record_action("deck:play-pause");
            (b_pl.send)(UiCommand::TogglePlayPause);
        });

        let (next_btn, next_img) = vector_icon_button(
            &bridge.icons,
            "next",
            icons::NEXT,
            &fg_pri,
            16,
            "Next track (])",
            &["ghost-btn"],
        );
        let b_nx = bridge.clone();
        next_btn.connect_clicked(move |_| (b_nx.send)(UiCommand::NextTrack));

        let (repeat_btn, repeat_img) = vector_icon_button(
            &bridge.icons,
            "repeat",
            icons::REPEAT,
            &fg_sec,
            15,
            "Cycle Repeat (R)",
            &["ghost-btn"],
        );
        let b_rp = bridge.clone();
        repeat_btn.connect_clicked(move |_| (b_rp.send)(UiCommand::CycleRepeat));

        let ab_btn = gtk4::Button::new();
        ab_btn.set_css_classes(&["ghost-btn"]);
        let ab_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let ab_img = bridge.icons.make_image("ab_loop", icons::AB_LOOP, &fg_sec, 14);
        let ab_lbl = gtk4::Label::new(Some("A-B"));
        ab_lbl.set_css_classes(&["mono"]);
        ab_box.append(&ab_img);
        ab_box.append(&ab_lbl);
        ab_btn.set_child(Some(&ab_box));
        make_clickable(&ab_btn, "Set A-B section loop (B)");
        let b_ab = bridge.clone();
        ab_btn.connect_clicked(move |_| (b_ab.send)(UiCommand::ToggleAbLoopStep));

        transport.append(&shuffle_btn);
        transport.append(&prev_btn);
        transport.append(&play_btn);
        transport.append(&next_btn);
        transport.append(&repeat_btn);
        transport.append(&ab_btn);

        let scrub_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let pos_lbl = gtk4::Label::new(Some("0:00"));
        pos_lbl.set_css_classes(&["mono", "text-muted"]);
        pos_lbl.set_width_chars(5);
        pos_lbl.set_xalign(1.0);

        let scrub_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1000.0, 100.0);
        scrub_scale.add_css_class("pulse-scale");
        scrub_scale.set_hexpand(true);
        scrub_scale.set_draw_value(false);

        let dur_lbl = gtk4::Label::new(Some("0:00"));
        dur_lbl.set_css_classes(&["mono", "text-muted"]);
        dur_lbl.set_width_chars(5);

        let suppress_scrub = Rc::new(Cell::new(false));
        let gesture = gtk4::GestureClick::new();
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let b_press = bridge.clone();
        let sc_p = scrub_scale.clone();
        gesture.connect_pressed(move |_, _, _, _| {
            b_press.store.borrow_mut().scrubbing_pos_ms = Some(sc_p.value() as u32);
        });
        let b_rel = bridge.clone();
        let sc_r = scrub_scale.clone();
        gesture.connect_released(move |_, _, _, _| {
            let ms = sc_r.value().max(0.0) as u32;
            b_rel.store.borrow_mut().scrubbing_pos_ms = None;
            (b_rel.send)(UiCommand::SeekMs(ms));
        });
        scrub_scale.add_controller(gesture);

        let sup_sc = Rc::clone(&suppress_scrub);
        let b_sc = bridge.clone();
        let pos_c = pos_lbl.clone();
        scrub_scale.connect_value_changed(move |sc| {
            if sup_sc.get() {
                return;
            }
            let ms = sc.value().max(0.0) as u32;
            pos_c.set_text(&format_duration_ms(ms));
            if b_sc.store.borrow().scrubbing_pos_ms.is_some() {
                b_sc.store.borrow_mut().scrubbing_pos_ms = Some(ms);
            }
        });

        scrub_row.append(&pos_lbl);
        scrub_row.append(&scrub_scale);
        scrub_row.append(&dur_lbl);
        center_box.append(&transport);
        center_box.append(&scrub_row);

        // Right Visualizer + Volume + Drawer Toggle Cluster
        let right_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        right_box.set_valign(gtk4::Align::Center);

        let viz = AudioVisualizerWidget::new(spectrum_reader, tokens.clone(), 120, 34);
        let viz_mode_btn = gtk4::Button::new();
        viz_mode_btn.set_css_classes(&["ghost-btn"]);
        let vm_inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let viz_img = bridge.icons.make_image("waveform", icons::WAVEFORM, &fg_sec, 14);
        let viz_mode_lbl = gtk4::Label::new(Some("Spectrum"));
        viz_mode_lbl.set_css_classes(&["mono", "text-muted"]);
        vm_inner.append(&viz_img);
        vm_inner.append(&viz_mode_lbl);
        viz_mode_btn.set_child(Some(&vm_inner));
        make_clickable(&viz_mode_btn, "Cycle visualizer mode or pause animation");
        let b_vz = bridge.clone();
        viz_mode_btn.connect_clicked(move |_| {
            let next = b_vz.store.borrow().view.config.visualizer_mode.next();
            b_vz.record_action(format!("viz:{}", next.label()));
            (b_vz.send)(UiCommand::SetVisualizerMode(next));
        });

        let (mute_btn, mute_img) = vector_icon_button(
            &bridge.icons,
            "volume_high",
            icons::VOLUME_HIGH,
            &fg_sec,
            15,
            "Toggle mute (M)",
            &["ghost-btn"],
        );
        let b_mu = bridge.clone();
        mute_btn.connect_clicked(move |_| (b_mu.send)(UiCommand::ToggleMute));

        let vol_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 100.0, 1.0);
        vol_scale.add_css_class("pulse-scale");
        vol_scale.set_size_request(82, -1);
        vol_scale.set_draw_value(false);
        let suppress_vol = Rc::new(Cell::new(false));
        let sup_v = Rc::clone(&suppress_vol);
        let b_v = bridge.clone();
        vol_scale.connect_value_changed(move |sc| {
            if sup_v.get() {
                return;
            }
            let pct = sc.value().clamp(0.0, 100.0);
            let raw = ((pct / 100.0) * 65535.0).round() as u16;
            (b_v.send)(UiCommand::SetVolume(raw));
        });

        let queue_btn = gtk4::Button::with_label("Queue");
        queue_btn.set_css_classes(&["ctrl-btn"]);
        let b_qb = bridge.clone();
        queue_btn.connect_clicked(move |_| {
            toggle_drawer_tab(&b_qb, RightDrawerTab::Queue);
        });
        let lyrics_btn = gtk4::Button::with_label("Lyrics");
        lyrics_btn.set_css_classes(&["ctrl-btn"]);
        let b_lb = bridge.clone();
        lyrics_btn.connect_clicked(move |_| {
            toggle_drawer_tab(&b_lb, RightDrawerTab::Lyrics);
        });
        let eq_btn = gtk4::Button::with_label("EQ");
        eq_btn.set_css_classes(&["ctrl-btn"]);
        let b_eb = bridge.clone();
        eq_btn.connect_clicked(move |_| {
            toggle_drawer_tab(&b_eb, RightDrawerTab::Equalizer);
        });

        right_box.append(viz.widget());
        right_box.append(&viz_mode_btn);
        right_box.append(&mute_btn);
        right_box.append(&vol_scale);
        right_box.append(&queue_btn);
        right_box.append(&lyrics_btn);
        right_box.append(&eq_btn);

        root.append(&left_box);
        root.append(&center_box);
        root.append(&right_box);

        Rc::new(Self {
            root,
            cover_pic,
            cover_fb,
            title_lbl,
            artist_lbl,
            source_chip,
            codec_lbl,
            like_img,
            shuffle_btn,
            shuffle_img,
            prev_img,
            play_img,
            next_img,
            repeat_btn,
            repeat_img,
            ab_btn,
            ab_lbl,
            pos_lbl,
            dur_lbl,
            scrub_scale,
            suppress_scrub,
            viz,
            viz_mode_btn,
            viz_mode_lbl,
            viz_img,
            mute_img,
            vol_scale,
            suppress_vol,
            queue_btn,
            lyrics_btn,
            eq_btn,
            last_cover_id: RefCell::new(String::new()),
        })
    }

    pub fn sync(&self, bridge: &UiBridge, flags: DirtyFlags) {
        let (ps, viz_mode, drawer_tab, width, scrubbing_ms, tokens, loaded_cnt) = {
            let s = bridge.store.borrow();
            (
                s.view.player_state.clone(),
                s.view.config.visualizer_mode,
                s.view.right_drawer,
                s.window_width,
                s.scrubbing_pos_ms,
                s.tokens.clone(),
                s.view.crate_view.loaded_tracks.loaded_count(),
            )
        };

        let compact_viz = width >= 1040;
        self.viz.widget().set_visible(compact_viz);
        self.viz_mode_btn.set_visible(width >= 900);
        self.viz.update_state(viz_mode, ps.is_playing);
        self.viz_mode_lbl.set_text(viz_mode.label());

        if flags.any(DirtyFlags::THEME | DirtyFlags::PLAYER_META | DirtyFlags::ALL) {
            self.viz.set_tokens(tokens.clone());
            let fg_pri = AdaptiveThemeTokens::hex(tokens.fg_primary);
            let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
            let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);
            let acc = AdaptiveThemeTokens::hex(tokens.accent);

            let (pk, ps_svg) = if ps.is_playing {
                ("pause", icons::PAUSE)
            } else {
                ("play", icons::PLAY)
            };
            bridge.icons.apply_to_image(&self.play_img, pk, ps_svg, &on_acc, 18);
            bridge.icons.apply_to_image(&self.prev_img, "prev", icons::PREV, &fg_pri, 16);
            bridge.icons.apply_to_image(&self.next_img, "next", icons::NEXT, &fg_pri, 16);
            bridge.icons.apply_to_image(
                &self.shuffle_img,
                "shuffle",
                icons::SHUFFLE,
                if ps.shuffle { &acc } else { &fg_sec },
                15,
            );
            let (rk, rsvg) = if ps.repeat == RepeatMode::Track {
                ("repeat_one", icons::REPEAT_ONE)
            } else {
                ("repeat", icons::REPEAT)
            };
            bridge.icons.apply_to_image(
                &self.repeat_img,
                rk,
                rsvg,
                if ps.repeat == RepeatMode::Off { &fg_sec } else { &acc },
                15,
            );
            let (mk, msvg) = if ps.muted || ps.volume == 0 {
                ("volume_mute", icons::VOLUME_MUTE)
            } else {
                ("volume_high", icons::VOLUME_HIGH)
            };
            bridge.icons.apply_to_image(&self.mute_img, mk, msvg, &fg_sec, 15);
            bridge.icons.apply_to_image(&self.viz_img, "waveform", icons::WAVEFORM, &fg_sec, 14);

            if ps.shuffle {
                self.shuffle_btn.add_css_class("active");
            } else {
                self.shuffle_btn.remove_css_class("active");
            }
            if ps.repeat != RepeatMode::Off {
                self.repeat_btn.add_css_class("active");
            } else {
                self.repeat_btn.remove_css_class("active");
            }
            match (ps.ab_loop.a_ms, ps.ab_loop.b_ms) {
                (Some(_), Some(_)) => {
                    self.ab_lbl.set_text("A-B ON");
                    self.ab_btn.add_css_class("active");
                }
                (Some(_), None) => {
                    self.ab_lbl.set_text("A...");
                    self.ab_btn.add_css_class("active");
                }
                _ => {
                    self.ab_lbl.set_text("A-B");
                    self.ab_btn.remove_css_class("active");
                }
            }

            if let Some(ref tr) = ps.active_track {
                self.title_lbl.set_text(&tr.summary.title);
                self.artist_lbl.set_text(&tr.summary.artist);
                for cls in ["source-local", "source-spotify", "source-youtube", "source-soundcloud"] {
                    self.source_chip.remove_css_class(cls);
                }
                self.source_chip
                    .add_css_class(source_badge_css_class(tr.summary.source));
                self.source_chip.set_text(tr.summary.source.badge_label());
                self.codec_lbl.set_text(&ps.audio_badge);

                let (lk, lsvg, lhex) = if tr.summary.is_liked {
                    ("heart_filled", icons::HEART_FILLED, acc.as_str())
                } else {
                    ("heart_outline", icons::HEART_OUTLINE, fg_sec.as_str())
                };
                bridge.icons.apply_to_image(&self.like_img, lk, lsvg, lhex, 16);

                if let Some(ref cover) = tr.summary.cover {
                    let cid = cover.id.to_string();
                    if *self.last_cover_id.borrow() != cid {
                        *self.last_cover_id.borrow_mut() = cid.clone();
                        self.sync_active_cover(bridge, cover.clone(), loaded_cnt);
                    }
                } else {
                    *self.last_cover_id.borrow_mut() = String::new();
                    self.cover_pic.set_paintable(gtk4::gdk::Paintable::NONE);
                    self.cover_pic.set_visible(false);
                    self.cover_fb.set_visible(true);
                }
            } else {
                self.title_lbl.set_text("Ready to play");
                self.artist_lbl.set_text("Drop files, paste a URL, or select any track");
                self.codec_lbl.set_text(&ps.audio_badge);
                bridge
                    .icons
                    .apply_to_image(&self.like_img, "heart_outline", icons::HEART_OUTLINE, &fg_sec, 16);
                self.cover_pic.set_paintable(gtk4::gdk::Paintable::NONE);
                self.cover_pic.set_visible(false);
                self.cover_fb.set_visible(true);
            }

            self.suppress_vol.set(true);
            self.vol_scale.set_value(f64::from(ps.volume_pct()));
            self.suppress_vol.set(false);
        }

        if flags.any(DirtyFlags::TIMELINE | DirtyFlags::PLAYER_META | DirtyFlags::ALL) {
            let dur = ps.duration_ms.max(1);
            let shown_pos = scrubbing_ms.unwrap_or(ps.position_ms).min(dur);
            self.dur_lbl.set_text(&format_duration_ms(ps.duration_ms));
            self.pos_lbl.set_text(&format_duration_ms(shown_pos));
            if scrubbing_ms.is_none() {
                self.suppress_scrub.set(true);
                self.scrub_scale.set_range(0.0, f64::from(dur));
                self.scrub_scale.set_value(f64::from(shown_pos));
                self.suppress_scrub.set(false);
            }
        }

        for (tab, btn) in [
            (RightDrawerTab::Queue, &self.queue_btn),
            (RightDrawerTab::Lyrics, &self.lyrics_btn),
            (RightDrawerTab::Equalizer, &self.eq_btn),
        ] {
            if drawer_tab == tab {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }
    }

    fn sync_active_cover(&self, bridge: &UiBridge, cover: ImageRef, loaded_cnt: usize) {
        if let Some(tex) = bridge.covers.cached_texture(&cover.id, CoverTier::Thumb) {
            self.cover_pic.set_paintable(Some(&tex));
            self.cover_pic.set_visible(true);
            self.cover_fb.set_visible(false);
            if let Some(acc) = bridge.covers.cached_accent(&cover.id) {
                apply_album_accent(bridge, acc);
            }
            return;
        }
        let pic = self.cover_pic.clone();
        let fb = self.cover_fb.clone();
        let cid = cover.id.to_string();
        let last_id = self.last_cover_id.borrow().clone();
        let b_cb = bridge.clone();
        bridge
            .covers
            .request_cover(cover, CoverTier::Thumb, loaded_cnt, move |tex, acc| {
                if cid == last_id {
                    pic.set_paintable(Some(&tex));
                    pic.set_visible(true);
                    fb.set_visible(false);
                    apply_album_accent(&b_cb, acc);
                }
            });
    }

    pub fn preview_visualizer(&self, snap: pulse_model::SpectrumSnapshot) {
        self.viz.seed_preview_snapshot(snap);
    }
}

fn toggle_drawer_tab(bridge: &UiBridge, target: RightDrawerTab) {
    {
        let mut s = bridge.store.borrow_mut();
        s.view.right_drawer = if s.view.right_drawer == target {
            RightDrawerTab::Closed
        } else {
            target
        };
    }
    bridge.record_action(format!("deck:drawer:{target:?}"));
    (bridge.redraw)(
        DirtyFlags::CHROME
            | DirtyFlags::DRAWER_QUEUE
            | DirtyFlags::DRAWER_LYRICS
            | DirtyFlags::DRAWER_EQ
            | DirtyFlags::DRAWER_INSIGHTS,
    );
}

fn apply_album_accent(bridge: &UiBridge, rgba: [u8; 4]) {
    let changed = {
        let mut s = bridge.store.borrow_mut();
        s.view.player_state.cover_accent_rgba = rgba;
        s.recompute_tokens()
    };
    if changed {
        (bridge.redraw)(DirtyFlags::THEME | DirtyFlags::PLAYER_META);
    }
}

/// Mode 2 (`FocusStage`): Full album showcase, live visualizer, side-by-side synced lyrics.
pub struct StageViewWidget {
    pub root: gtk4::Box,
    hero_pic: gtk4::Picture,
    hero_fb: gtk4::Image,
    title_lbl: gtk4::Label,
    artist_lbl: gtk4::Label,
    album_lbl: gtk4::Label,
    source_chip: gtk4::Label,
    codec_lbl: gtk4::Label,
    viz: AudioVisualizerWidget,
    lyrics_panel: Rc<LyricsPanelWidget>,
    play_img: gtk4::Image,
    scrub_scale: gtk4::Scale,
    suppress_scrub: Rc<Cell<bool>>,
    pos_lbl: gtk4::Label,
    dur_lbl: gtk4::Label,
    last_hero_id: RefCell<String>,
}

impl StageViewWidget {
    pub fn new(bridge: &UiBridge, spectrum_reader: SpectrumReadFn) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        root.add_css_class("stage-root");
        root.set_vexpand(true);
        root.set_hexpand(true);

        let tokens = bridge.store.borrow().tokens.clone();
        let fg_pri = AdaptiveThemeTokens::hex(tokens.fg_primary);
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
        let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);

        // Top Stage Bar
        let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        top.set_margin_top(12);
        top.set_margin_start(18);
        top.set_margin_end(18);

        let back_btn = gtk4::Button::with_label("Back to Deck");
        back_btn.set_css_classes(&["ctrl-btn"]);
        let b_bk = bridge.clone();
        back_btn.connect_clicked(move |_| (b_bk.send)(UiCommand::SetMode(AppMode::Workbench)));

        let sp = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        sp.set_hexpand(true);

        let mini_btn = gtk4::Button::with_label("Mini-Player");
        mini_btn.set_css_classes(&["ghost-btn"]);
        let b_mn = bridge.clone();
        mini_btn.connect_clicked(move |_| (b_mn.send)(UiCommand::SetMode(AppMode::MiniPlayer)));

        let theme_btn = gtk4::Button::with_label("Theme");
        theme_btn.set_css_classes(&["ghost-btn"]);
        let b_th = bridge.clone();
        theme_btn.connect_clicked(move |_| {
            let next = if b_th.store.borrow().tokens.is_light {
                pulse_model::ThemePreference::Dark
            } else {
                pulse_model::ThemePreference::Light
            };
            (b_th.send)(UiCommand::SetThemePreference(next));
        });

        top.append(&back_btn);
        top.append(&sp);
        top.append(&theme_btn);
        top.append(&mini_btn);
        root.append(&top);

        // Center Showcase + Synced Lyrics Split
        let split = gtk4::Box::new(gtk4::Orientation::Horizontal, 24);
        split.set_vexpand(true);
        split.set_margin_start(28);
        split.set_margin_end(28);

        let left = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        left.set_halign(gtk4::Align::Center);
        left.set_valign(gtk4::Align::Center);
        left.set_size_request(340, -1);

        let hero_overlay = gtk4::Overlay::new();
        hero_overlay.add_css_class("hero-art");
        hero_overlay.set_size_request(300, 300);
        let hero_fb = bridge.icons.make_image("music_note", icons::MUSIC_NOTE, &fg_sec, 48);
        hero_fb.set_halign(gtk4::Align::Center);
        hero_fb.set_valign(gtk4::Align::Center);
        let hero_pic = gtk4::Picture::new();
        hero_pic.set_size_request(300, 300);
        hero_pic.set_content_fit(gtk4::ContentFit::Cover);
        hero_overlay.set_child(Some(&hero_fb));
        hero_overlay.add_overlay(&hero_pic);

        let title_lbl = gtk4::Label::new(Some("Ready to Play"));
        title_lbl.set_css_classes(&["heading-xl"]);
        title_lbl.set_xalign(0.0);
        title_lbl.set_ellipsize(pango::EllipsizeMode::End);

        let artist_lbl = gtk4::Label::new(Some("No active track"));
        artist_lbl.set_css_classes(&["heading-md", "text-secondary"]);
        artist_lbl.set_xalign(0.0);
        artist_lbl.set_ellipsize(pango::EllipsizeMode::End);

        let album_lbl = gtk4::Label::new(None);
        album_lbl.set_css_classes(&["text-muted"]);
        album_lbl.set_xalign(0.0);
        album_lbl.set_ellipsize(pango::EllipsizeMode::End);

        let chips = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let source_chip = gtk4::Label::new(Some("LOCAL"));
        source_chip.set_css_classes(&["chip-pill", "source-local"]);
        let codec_lbl = gtk4::Label::new(Some("READY"));
        codec_lbl.set_css_classes(&["mono", "text-muted"]);
        chips.append(&source_chip);
        chips.append(&codec_lbl);

        let viz = AudioVisualizerWidget::new(spectrum_reader, tokens, 300, 52);

        left.append(&hero_overlay);
        left.append(&title_lbl);
        left.append(&artist_lbl);
        left.append(&album_lbl);
        left.append(&chips);
        left.append(viz.widget());

        let right_card = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        right_card.add_css_class("surface-card");
        right_card.set_hexpand(true);
        right_card.set_vexpand(true);
        let lyr_hdr = gtk4::Label::new(Some("Synced Lyrics"));
        lyr_hdr.set_css_classes(&["heading-lg"]);
        lyr_hdr.set_xalign(0.0);
        let lyrics_panel = LyricsPanelWidget::new(bridge);
        right_card.append(&lyr_hdr);
        right_card.append(&lyrics_panel.root);

        split.append(&left);
        split.append(&right_card);
        root.append(&split);

        // Stage Bottom Transport
        let bottom = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        bottom.set_margin_start(28);
        bottom.set_margin_end(28);
        bottom.set_margin_bottom(16);

        let scrub_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        let pos_lbl = gtk4::Label::new(Some("0:00"));
        pos_lbl.set_css_classes(&["mono", "text-muted"]);
        let scrub_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.0, 1000.0, 100.0);
        scrub_scale.add_css_class("pulse-scale");
        scrub_scale.set_hexpand(true);
        scrub_scale.set_draw_value(false);
        let dur_lbl = gtk4::Label::new(Some("0:00"));
        dur_lbl.set_css_classes(&["mono", "text-muted"]);
        let suppress_scrub = Rc::new(Cell::new(false));
        let gesture = gtk4::GestureClick::new();
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let b_p = bridge.clone();
        let sc_p = scrub_scale.clone();
        gesture.connect_pressed(move |_, _, _, _| {
            b_p.store.borrow_mut().scrubbing_pos_ms = Some(sc_p.value() as u32);
        });
        let b_r = bridge.clone();
        let sc_r = scrub_scale.clone();
        gesture.connect_released(move |_, _, _, _| {
            let ms = sc_r.value().max(0.0) as u32;
            b_r.store.borrow_mut().scrubbing_pos_ms = None;
            (b_r.send)(UiCommand::SeekMs(ms));
        });
        scrub_scale.add_controller(gesture);

        scrub_row.append(&pos_lbl);
        scrub_row.append(&scrub_scale);
        scrub_row.append(&dur_lbl);

        let tr_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        tr_row.set_halign(gtk4::Align::Center);
        let (prev_btn, _) = vector_icon_button(&bridge.icons, "prev", icons::PREV, &fg_pri, 18, "Previous", &["ctrl-btn"]);
        let b_pv = bridge.clone();
        prev_btn.connect_clicked(move |_| (b_pv.send)(UiCommand::PrevTrack));

        let (play_btn, play_img) = vector_icon_button(&bridge.icons, "play", icons::PLAY, &on_acc, 20, "Play / Pause", &["accent-btn"]);
        let b_pl = bridge.clone();
        play_btn.connect_clicked(move |_| (b_pl.send)(UiCommand::TogglePlayPause));

        let (next_btn, _) = vector_icon_button(&bridge.icons, "next", icons::NEXT, &fg_pri, 18, "Next", &["ctrl-btn"]);
        let b_nx = bridge.clone();
        next_btn.connect_clicked(move |_| (b_nx.send)(UiCommand::NextTrack));

        tr_row.append(&prev_btn);
        tr_row.append(&play_btn);
        tr_row.append(&next_btn);

        bottom.append(&scrub_row);
        bottom.append(&tr_row);
        root.append(&bottom);

        Rc::new(Self {
            root,
            hero_pic,
            hero_fb,
            title_lbl,
            artist_lbl,
            album_lbl,
            source_chip,
            codec_lbl,
            viz,
            lyrics_panel,
            play_img,
            scrub_scale,
            suppress_scrub,
            pos_lbl,
            dur_lbl,
            last_hero_id: RefCell::new(String::new()),
        })
    }

    pub fn sync(&self, bridge: &UiBridge, flags: DirtyFlags) {
        let (ps, lyrics, viz_mode, scrubbing_ms, tokens, loaded_cnt) = {
            let s = bridge.store.borrow();
            (
                s.view.player_state.clone(),
                s.view.lyrics.clone(),
                s.view.config.visualizer_mode,
                s.scrubbing_pos_ms,
                s.tokens.clone(),
                s.view.crate_view.loaded_tracks.loaded_count(),
            )
        };

        self.viz.update_state(viz_mode, ps.is_playing);
        self.viz.set_tokens(tokens.clone());

        if flags.any(DirtyFlags::THEME | DirtyFlags::PLAYER_META | DirtyFlags::MODE | DirtyFlags::ALL) {
            let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);
            let (pk, ps_svg) = if ps.is_playing {
                ("pause", icons::PAUSE)
            } else {
                ("play", icons::PLAY)
            };
            bridge.icons.apply_to_image(&self.play_img, pk, ps_svg, &on_acc, 20);

            if let Some(ref tr) = ps.active_track {
                self.title_lbl.set_text(&tr.summary.title);
                self.artist_lbl.set_text(&tr.summary.artist);
                self.album_lbl.set_text(&tr.summary.album);
                for cls in ["source-local", "source-spotify", "source-youtube", "source-soundcloud"] {
                    self.source_chip.remove_css_class(cls);
                }
                self.source_chip.add_css_class(source_badge_css_class(tr.summary.source));
                self.source_chip.set_text(tr.summary.source.badge_label());
                self.codec_lbl.set_text(&ps.audio_badge);

                if let Some(ref cover) = tr.summary.cover {
                    let cid = cover.id.to_string();
                    if *self.last_hero_id.borrow() != cid {
                        *self.last_hero_id.borrow_mut() = cid.clone();
                        if let Some(tex) = bridge.covers.cached_texture(&cover.id, CoverTier::Hero) {
                            self.hero_pic.set_paintable(Some(&tex));
                            self.hero_pic.set_visible(true);
                            self.hero_fb.set_visible(false);
                        } else {
                            let pic = self.hero_pic.clone();
                            let fb = self.hero_fb.clone();
                            let b_cb = bridge.clone();
                            bridge.covers.request_cover(
                                cover.clone(),
                                CoverTier::Hero,
                                loaded_cnt,
                                move |tex, acc| {
                                    pic.set_paintable(Some(&tex));
                                    pic.set_visible(true);
                                    fb.set_visible(false);
                                    apply_album_accent(&b_cb, acc);
                                },
                            );
                        }
                    }
                }
            }
        }

        if flags.any(DirtyFlags::DRAWER_LYRICS | DirtyFlags::MODE | DirtyFlags::ALL) {
            self.lyrics_panel.sync_content(bridge, &lyrics);
        }
        if flags.any(DirtyFlags::DRAWER_LYRICS | DirtyFlags::DRAWER_LYRICS_TICK | DirtyFlags::MODE | DirtyFlags::ALL) {
            self.lyrics_panel
                .sync_active_line(ps.position_ms, lyrics.offset_ms, lyrics.synced);
        }

        if flags.any(DirtyFlags::TIMELINE | DirtyFlags::PLAYER_META | DirtyFlags::MODE | DirtyFlags::ALL) {
            let dur = ps.duration_ms.max(1);
            let shown = scrubbing_ms.unwrap_or(ps.position_ms).min(dur);
            self.dur_lbl.set_text(&format_duration_ms(ps.duration_ms));
            self.pos_lbl.set_text(&format_duration_ms(shown));
            if scrubbing_ms.is_none() {
                self.suppress_scrub.set(true);
                self.scrub_scale.set_range(0.0, f64::from(dur));
                self.scrub_scale.set_value(f64::from(shown));
                self.suppress_scrub.set(false);
            }
        }
    }

    pub fn preview_visualizer(&self, snap: pulse_model::SpectrumSnapshot) {
        self.viz.seed_preview_snapshot(snap);
    }
}

/// Mode 3 (`MiniPlayer`): Compact `430x124` desk widget.
pub struct MiniPlayerWidget {
    pub root: gtk4::Box,
    cover_pic: gtk4::Picture,
    cover_fb: gtk4::Image,
    title_lbl: gtk4::Label,
    sub_lbl: gtk4::Label,
    source_chip: gtk4::Label,
    play_img: gtk4::Image,
    like_img: gtk4::Image,
    progress_bar: gtk4::ProgressBar,
    last_cover_id: RefCell<String>,
}

impl MiniPlayerWidget {
    pub fn new(bridge: &UiBridge) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        root.add_css_class("mini-root");
        root.set_valign(gtk4::Align::Start);

        let tokens = bridge.store.borrow().tokens.clone();
        let fg_pri = AdaptiveThemeTokens::hex(tokens.fg_primary);
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
        let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);

        let cov_overlay = gtk4::Overlay::new();
        cov_overlay.add_css_class("thumb-art");
        cov_overlay.set_size_request(76, 76);
        cov_overlay.set_valign(gtk4::Align::Center);
        cov_overlay.set_halign(gtk4::Align::Center);
        let cover_fb = bridge.icons.make_image("music_note", icons::MUSIC_NOTE, &fg_sec, 24);
        cover_fb.set_halign(gtk4::Align::Center);
        cover_fb.set_valign(gtk4::Align::Center);
        let cover_pic = gtk4::Picture::new();
        cover_pic.set_size_request(76, 76);
        cover_pic.set_content_fit(gtk4::ContentFit::Cover);
        cov_overlay.set_child(Some(&cover_fb));
        cov_overlay.add_overlay(&cover_pic);

        let right = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        right.set_hexpand(true);
        right.set_valign(gtk4::Align::Center);

        let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
        meta.set_hexpand(true);
        let title_lbl = gtk4::Label::new(Some("Ready"));
        title_lbl.set_css_classes(&["heading-md"]);
        title_lbl.set_xalign(0.0);
        title_lbl.set_ellipsize(pango::EllipsizeMode::End);
        let sub_lbl = gtk4::Label::new(Some("Pulse Audio"));
        sub_lbl.set_css_classes(&["text-secondary"]);
        sub_lbl.set_xalign(0.0);
        sub_lbl.set_ellipsize(pango::EllipsizeMode::End);
        meta.append(&title_lbl);
        meta.append(&sub_lbl);

        let source_chip = gtk4::Label::new(Some("LOCAL"));
        source_chip.set_css_classes(&["chip-pill", "source-local"]);
        source_chip.set_valign(gtk4::Align::Start);

        let (expand_btn, _) = vector_icon_button(
            &bridge.icons,
            "expand",
            icons::EXPAND,
            &fg_sec,
            15,
            "Expand to Full Deck",
            &["ghost-btn"],
        );
        let b_ex = bridge.clone();
        expand_btn.connect_clicked(move |_| (b_ex.send)(UiCommand::SetMode(AppMode::Workbench)));

        top.append(&meta);
        top.append(&source_chip);
        top.append(&expand_btn);

        let progress_bar = gtk4::ProgressBar::new();

        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let (prev_btn, _) = vector_icon_button(&bridge.icons, "prev", icons::PREV, &fg_pri, 14, "Previous", &["ghost-btn"]);
        let b_pv = bridge.clone();
        prev_btn.connect_clicked(move |_| (b_pv.send)(UiCommand::PrevTrack));

        let (play_btn, play_img) = vector_icon_button(&bridge.icons, "play", icons::PLAY, &on_acc, 15, "Play/Pause", &["accent-btn"]);
        let b_pl = bridge.clone();
        play_btn.connect_clicked(move |_| (b_pl.send)(UiCommand::TogglePlayPause));

        let (next_btn, _) = vector_icon_button(&bridge.icons, "next", icons::NEXT, &fg_pri, 14, "Next", &["ghost-btn"]);
        let b_nx = bridge.clone();
        next_btn.connect_clicked(move |_| (b_nx.send)(UiCommand::NextTrack));

        let (like_btn, like_img) = vector_icon_button(&bridge.icons, "heart_outline", icons::HEART_OUTLINE, &fg_sec, 14, "Favorite", &["ghost-btn"]);
        let b_lk = bridge.clone();
        like_btn.connect_clicked(move |_| {
            if let Some(tr) = b_lk
                .store
                .borrow()
                .view
                .player_state
                .active_track
                .as_ref()
                .map(|t| t.summary.clone())
            {
                (b_lk.send)(UiCommand::ToggleLikeTrack(tr));
            }
        });

        controls.append(&prev_btn);
        controls.append(&play_btn);
        controls.append(&next_btn);
        controls.append(&like_btn);

        right.append(&top);
        right.append(&progress_bar);
        right.append(&controls);

        root.append(&cov_overlay);
        root.append(&right);

        Rc::new(Self {
            root,
            cover_pic,
            cover_fb,
            title_lbl,
            sub_lbl,
            source_chip,
            play_img,
            like_img,
            progress_bar,
            last_cover_id: RefCell::new(String::new()),
        })
    }

    pub fn sync(&self, bridge: &UiBridge, _flags: DirtyFlags) {
        let (ps, tokens, loaded_cnt) = {
            let s = bridge.store.borrow();
            (
                s.view.player_state.clone(),
                s.tokens.clone(),
                s.view.crate_view.loaded_tracks.loaded_count(),
            )
        };
        let on_acc = AdaptiveThemeTokens::hex(tokens.on_accent);
        let acc = AdaptiveThemeTokens::hex(tokens.accent);
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);

        let (pk, psvg) = if ps.is_playing {
            ("pause", icons::PAUSE)
        } else {
            ("play", icons::PLAY)
        };
        bridge.icons.apply_to_image(&self.play_img, pk, psvg, &on_acc, 15);

        let frac = if ps.duration_ms > 0 {
            (ps.position_ms as f64 / ps.duration_ms as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.progress_bar.set_fraction(frac);

        if let Some(ref tr) = ps.active_track {
            self.title_lbl.set_text(&tr.summary.title);
            self.sub_lbl.set_text(&tr.summary.artist);
            for cls in ["source-local", "source-spotify", "source-youtube", "source-soundcloud"] {
                self.source_chip.remove_css_class(cls);
            }
            self.source_chip.add_css_class(source_badge_css_class(tr.summary.source));
            self.source_chip.set_text(tr.summary.source.badge_label());

            let (lk, lsvg, lhex) = if tr.summary.is_liked {
                ("heart_filled", icons::HEART_FILLED, acc.as_str())
            } else {
                ("heart_outline", icons::HEART_OUTLINE, fg_sec.as_str())
            };
            bridge.icons.apply_to_image(&self.like_img, lk, lsvg, lhex, 14);

            if let Some(ref cover) = tr.summary.cover {
                let cid = cover.id.to_string();
                if *self.last_cover_id.borrow() != cid {
                    *self.last_cover_id.borrow_mut() = cid;
                    if let Some(tex) = bridge.covers.cached_texture(&cover.id, CoverTier::Thumb) {
                        self.cover_pic.set_paintable(Some(&tex));
                        self.cover_pic.set_visible(true);
                        self.cover_fb.set_visible(false);
                    } else {
                        let pic = self.cover_pic.clone();
                        let fb = self.cover_fb.clone();
                        bridge.covers.request_cover(
                            cover.clone(),
                            CoverTier::Thumb,
                            loaded_cnt,
                            move |tex, _| {
                                pic.set_paintable(Some(&tex));
                                pic.set_visible(true);
                                fb.set_visible(false);
                            },
                        );
                    }
                }
            }
        }
    }
}
