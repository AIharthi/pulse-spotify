//! Native GTK4 Settings overlay dialog, Sleep Timer popover, Devices popover, and Playlist picker.

use std::rc::Rc;

use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use pulse_assets::{AdaptiveThemeTokens, icons};
use pulse_model::{
    AccentSource, AuthState, NormalisationMode, SettingsTab, SleepTimerTarget,
    StreamQuality, ThemePreference, TrackSummary, UiCommand, VisualizerMode,
};

use crate::icons_gtk::{make_clickable, vector_icon_button};
use crate::state::{DirtyFlags, UiBridge};

/// Populates the Sleep Timer popover menu cleanly.
pub fn populate_sleep_popover(popover: &gtk4::Popover, bridge: &UiBridge) {
    let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    box_v.set_margin_top(4);
    box_v.set_margin_bottom(4);

    let title = gtk4::Label::new(Some("Sleep Timer"));
    title.set_css_classes(&["section-kicker"]);
    title.set_xalign(0.0);
    box_v.append(&title);

    let options = [
        ("Turn Off", SleepTimerTarget::Off),
        ("15 Minutes", SleepTimerTarget::Minutes(15)),
        ("30 Minutes", SleepTimerTarget::Minutes(30)),
        ("45 Minutes", SleepTimerTarget::Minutes(45)),
        ("60 Minutes", SleepTimerTarget::Minutes(60)),
        ("End of Current Track", SleepTimerTarget::EndOfTrack),
    ];
    for (lbl, target) in options {
        let btn = gtk4::Button::with_label(lbl);
        btn.set_css_classes(&["nav-btn"]);
        if let Some(child) = btn.child()
            && let Ok(l) = child.downcast::<gtk4::Label>()
        {
            l.set_xalign(0.0);
        }
        make_clickable(&btn, lbl);
        let b = bridge.clone();
        let pop = popover.clone();
        btn.connect_clicked(move |_| {
            b.record_action(format!("sleep:{lbl}"));
            (b.send)(UiCommand::SetSleepTimer(target));
            pop.popdown();
        });
        box_v.append(&btn);
    }
    popover.set_child(Some(&box_v));
}

/// Populates the Playback Devices (Local & Spotify Connect) popover menu.
pub fn populate_devices_popover(popover: &gtk4::Popover, bridge: &UiBridge) {
    let (devices, tokens) = {
        let s = bridge.store.borrow();
        (s.view.devices.clone(), s.tokens.clone())
    };
    let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);

    let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    box_v.set_size_request(260, -1);

    let hdr = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let title = gtk4::Label::new(Some("Playback Devices"));
    title.set_css_classes(&["section-kicker"]);
    title.set_hexpand(true);
    title.set_xalign(0.0);
    let (ref_btn, _) = vector_icon_button(
        &bridge.icons,
        "refresh",
        icons::REFRESH,
        &fg_sec,
        14,
        "Refresh Spotify Connect devices",
        &["ghost-btn"],
    );
    let b_ref = bridge.clone();
    ref_btn.connect_clicked(move |_| {
        (b_ref.send)(UiCommand::RefreshDevices);
    });
    hdr.append(&title);
    hdr.append(&ref_btn);
    box_v.append(&hdr);

    if devices.available_devices.is_empty() {
        let row = gtk4::Label::new(Some(&format!(
            "Local Audio: {} (PulseAudio / PipeWire active)",
            devices.local_device_name
        )));
        row.set_css_classes(&["text-secondary"]);
        row.set_wrap(true);
        row.set_xalign(0.0);
        box_v.append(&row);
    } else {
        for dev in devices.available_devices {
            let btn = gtk4::Button::new();
            btn.set_css_classes(if dev.is_active {
                &["nav-btn", "active"]
            } else {
                &["nav-btn"]
            });
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let icon = bridge.icons.make_image("device", icons::DEVICE, &fg_sec, 15);
            let txt = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
            txt.set_hexpand(true);
            let name = gtk4::Label::new(Some(&dev.name));
            name.set_xalign(0.0);
            name.set_ellipsize(pango::EllipsizeMode::End);
            let sub = gtk4::Label::new(Some(&format!(
                "{} · {}%",
                dev.device_type,
                dev.volume_pct()
            )));
            sub.set_css_classes(&["text-muted", "mono"]);
            sub.set_xalign(0.0);
            txt.append(&name);
            txt.append(&sub);
            row.append(&icon);
            row.append(&txt);
            btn.set_child(Some(&row));
            make_clickable(&btn, &format!("Transfer playback to {}", dev.name));

            let id = dev.id.clone();
            let b_tr = bridge.clone();
            let pop = popover.clone();
            btn.connect_clicked(move |_| {
                (b_tr.send)(UiCommand::TransferPlayback {
                    device_id: id.clone(),
                });
                pop.popdown();
            });
            box_v.append(&btn);
        }
    }
    popover.set_child(Some(&box_v));
}

/// Builds an interactive popover button menu to add `track` to any custom playlist or create one.
pub fn open_add_to_playlist_popover(anchor: &impl IsA<gtk4::Widget>, bridge: &UiBridge, track: TrackSummary) {
    let pop = gtk4::Popover::new();
    pop.set_parent(anchor);
    let box_v = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    box_v.set_size_request(210, -1);

    let lbl = gtk4::Label::new(Some("Add to Playlist"));
    lbl.set_css_classes(&["section-kicker"]);
    lbl.set_xalign(0.0);
    box_v.append(&lbl);

    let playlists = bridge.store.borrow().view.custom_playlists.clone();
    for pl in playlists {
        let btn = gtk4::Button::with_label(&pl.name);
        btn.set_css_classes(&["nav-btn"]);
        if let Some(child) = btn.child()
            && let Ok(l) = child.downcast::<gtk4::Label>()
        {
            l.set_xalign(0.0);
            l.set_ellipsize(pango::EllipsizeMode::End);
        }
        make_clickable(&btn, &format!("Add to {}", pl.name));
        let pid = pl.id.clone();
        let tr = track.clone();
        let b = bridge.clone();
        let pop_c = pop.clone();
        btn.connect_clicked(move |_| {
            (b.send)(UiCommand::AddTrackToCustomPlaylist {
                playlist_id: pid.clone(),
                track: tr.clone(),
            });
            pop_c.popdown();
        });
        box_v.append(&btn);
    }

    let new_btn = gtk4::Button::with_label("+ New Playlist");
    new_btn.set_css_classes(&["ctrl-btn"]);
    make_clickable(&new_btn, "Create a new cross-source playlist");
    let b_new = bridge.clone();
    let pop_c = pop.clone();
    new_btn.connect_clicked(move |_| {
        pop_c.popdown();
        let b_confirm = b_new.clone();
        (b_new.prompt_text_dialog)(
            "New Playlist".to_string(),
            "Playlist name".to_string(),
            String::new(),
            Rc::new(move |name| {
                (b_confirm.send)(UiCommand::CreateCustomPlaylist(name));
            }),
        );
    });
    box_v.append(&new_btn);
    pop.set_child(Some(&box_v));
    pop.connect_closed(|p| p.unparent());
    pop.popup();
}

/// Settings Modal (`Sources & Folders`, `Spotify Account & Connect`, `Audio & Appearance`, `Memory & Cache`).
pub struct SettingsModalController {
    dialog: gtk4::Window,
    tab_btns: Vec<(SettingsTab, gtk4::Button)>,
    body_box: gtk4::Box,
    last_open: std::cell::Cell<bool>,
}

impl SettingsModalController {
    pub fn new(parent: &gtk4::ApplicationWindow, bridge: &UiBridge) -> Rc<Self> {
        let dialog = gtk4::Window::builder()
            .transient_for(parent)
            .modal(true)
            .title("Pulse Settings")
            .default_width(680)
            .default_height(520)
            .hide_on_close(true)
            .build();
        dialog.set_css_classes(&["pulse-root"]);

        let outer = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        outer.add_css_class("modal-card");
        outer.set_margin_top(10);
        outer.set_margin_bottom(10);
        outer.set_margin_start(10);
        outer.set_margin_end(10);

        let hdr = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let title = gtk4::Label::new(Some("Settings"));
        title.set_css_classes(&["heading-lg"]);
        title.set_hexpand(true);
        title.set_xalign(0.0);

        let fg_sec = {
            let s = bridge.store.borrow();
            AdaptiveThemeTokens::hex(s.tokens.fg_secondary)
        };
        let (close_btn, _) = vector_icon_button(
            &bridge.icons,
            "close",
            icons::CLOSE,
            &fg_sec,
            16,
            "Close settings (Esc)",
            &["ghost-btn"],
        );
        let b_cl = bridge.clone();
        close_btn.connect_clicked(move |_| {
            b_cl.store.borrow_mut().settings_open = false;
            (b_cl.redraw)(DirtyFlags::SETTINGS);
        });
        hdr.append(&title);
        hdr.append(&close_btn);
        outer.append(&hdr);

        let tab_strip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        tab_strip.add_css_class("mode-seg");
        let tabs = [
            (SettingsTab::SourcesAndFolders, "Sources & Folders"),
            (SettingsTab::AccountAndConnect, "Spotify & Connect"),
            (SettingsTab::AudioAndDesktop, "Audio & Appearance"),
            (SettingsTab::MemoryAndCache, "Memory & Cache"),
        ];
        let mut tab_btns = Vec::new();
        for (t_id, label) in tabs {
            let btn = gtk4::Button::with_label(label);
            btn.set_css_classes(&["seg-btn"]);
            make_clickable(&btn, label);
            let b_t = bridge.clone();
            btn.connect_clicked(move |_| {
                b_t.store.borrow_mut().settings_tab = t_id;
                (b_t.redraw)(DirtyFlags::SETTINGS);
            });
            tab_strip.append(&btn);
            tab_btns.push((t_id, btn));
        }
        outer.append(&tab_strip);

        let scroller = gtk4::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        let body_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        scroller.set_child(Some(&body_box));
        outer.append(&scroller);
        dialog.set_child(Some(&outer));

        // Esc key closes cleanly
        let key_ctrl = gtk4::EventControllerKey::new();
        let b_esc = bridge.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::Escape {
                b_esc.store.borrow_mut().settings_open = false;
                (b_esc.redraw)(DirtyFlags::SETTINGS);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(key_ctrl);

        let b_hide = bridge.clone();
        dialog.connect_close_request(move |_| {
            b_hide.store.borrow_mut().settings_open = false;
            glib::Propagation::Proceed
        });

        Rc::new(Self {
            dialog,
            tab_btns,
            body_box,
            last_open: std::cell::Cell::new(false),
        })
    }

    pub fn sync(&self, bridge: &UiBridge) {
        let open = bridge.store.borrow().settings_open;
        if !open {
            if self.last_open.get() {
                self.last_open.set(false);
                self.dialog.set_visible(false);
            }
            return;
        }
        self.last_open.set(true);
        self.dialog.present();

        let active_tab = bridge.store.borrow().settings_tab;
        for (t_id, btn) in &self.tab_btns {
            if *t_id == active_tab {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }

        while let Some(c) = self.body_box.first_child() {
            self.body_box.remove(&c);
        }

        match active_tab {
            SettingsTab::SourcesAndFolders => self.build_sources_tab(bridge),
            SettingsTab::AccountAndConnect => self.build_account_tab(bridge),
            SettingsTab::AudioAndDesktop => self.build_audio_appearance_tab(bridge),
            SettingsTab::MemoryAndCache => self.build_memory_tab(bridge),
        }
    }

    fn build_sources_tab(&self, bridge: &UiBridge) {
        let (dirs, count) = {
            let s = bridge.store.borrow();
            (s.view.config.local_music_dirs.clone(), s.view.local_track_count)
        };

        let summary = gtk4::Label::new(Some(&format!(
            "Local Library: {count} tracks indexed across {} folder(s)",
            dirs.len()
        )));
        summary.set_css_classes(&["heading-md"]);
        summary.set_xalign(0.0);
        self.body_box.append(&summary);

        let btns = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let add_btn = gtk4::Button::with_label("Add Music Folder");
        add_btn.set_css_classes(&["accent-btn"]);
        let b_add = bridge.clone();
        add_btn.connect_clicked(move |_| (b_add.open_folder_dialog)());

        let files_btn = gtk4::Button::with_label("Open Audio Files");
        files_btn.set_css_classes(&["ctrl-btn"]);
        let b_f = bridge.clone();
        files_btn.connect_clicked(move |_| (b_f.open_files_dialog)());

        let rescan_btn = gtk4::Button::with_label("Rescan Folders");
        rescan_btn.set_css_classes(&["ctrl-btn"]);
        let b_r = bridge.clone();
        rescan_btn.connect_clicked(move |_| (b_r.send)(UiCommand::RescanLocalMusic));

        btns.append(&add_btn);
        btns.append(&files_btn);
        btns.append(&rescan_btn);
        self.body_box.append(&btns);

        for dir in dirs {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            row.add_css_class("surface-elevated");
            let path_lbl = gtk4::Label::new(Some(&dir));
            path_lbl.set_css_classes(&["mono"]);
            path_lbl.set_hexpand(true);
            path_lbl.set_xalign(0.0);
            path_lbl.set_ellipsize(pango::EllipsizeMode::Middle);
            let rm = gtk4::Button::with_label("Remove");
            rm.set_css_classes(&["ghost-btn"]);
            let d_clone = dir.clone();
            let b_rm = bridge.clone();
            rm.connect_clicked(move |_| {
                (b_rm.send)(UiCommand::RemoveLocalMusicFolder(d_clone.clone()));
            });
            row.append(&path_lbl);
            row.append(&rm);
            self.body_box.append(&row);
        }

        let stream_info = gtk4::Label::new(Some(
            "YouTube (InnerTube search, playlists, full-length audio) and SoundCloud (search, URLs, sets) run natively with zero account setup.",
        ));
        stream_info.set_css_classes(&["text-secondary"]);
        stream_info.set_wrap(true);
        stream_info.set_xalign(0.0);
        self.body_box.append(&stream_info);
    }

    fn build_account_tab(&self, bridge: &UiBridge) {
        let (auth, conn, cfg) = {
            let s = bridge.store.borrow();
            (
                s.view.auth_state.clone(),
                s.view.connection.clone(),
                s.view.config.clone(),
            )
        };

        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        card.add_css_class("surface-elevated");

        let status_txt = match &auth {
            AuthState::Unauthenticated => "Spotify: Not signed in (Connect disabled)".to_string(),
            AuthState::ConnectOnly { device_name } => {
                format!("Spotify Connect Ready as \"{device_name}\" (Zeroconf)")
            }
            AuthState::WaitingOAuth { .. } => "Waiting for Spotify browser login...".to_string(),
            AuthState::Authenticated {
                display_name,
                username,
                web_api_ready,
                ..
            } => format!(
                "Signed in as {display_name} (@{username}) · Web API {}",
                if *web_api_ready { "Ready" } else { "Pending" }
            ),
        };
        let st_lbl = gtk4::Label::new(Some(&status_txt));
        st_lbl.set_css_classes(&["heading-md"]);
        st_lbl.set_xalign(0.0);
        st_lbl.set_wrap(true);
        card.append(&st_lbl);

        let conn_lbl = gtk4::Label::new(Some(&format!("Transport: {conn:?}")));
        conn_lbl.set_css_classes(&["mono", "text-muted"]);
        conn_lbl.set_xalign(0.0);
        card.append(&conn_lbl);

        let client_entry = gtk4::Entry::new();
        client_entry.add_css_class("pulse-entry");
        client_entry.set_placeholder_text(Some("Optional Spotify Web API Client ID"));
        if let Some(ref cid) = cfg.custom_client_id {
            client_entry.set_text(cid);
        }
        card.append(&client_entry);

        let btn_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        match auth {
            AuthState::WaitingOAuth { auth_url } => {
                let open_btn = gtk4::Button::with_label("Open Auth URL");
                open_btn.set_css_classes(&["accent-btn"]);
                let b_o = bridge.clone();
                open_btn.connect_clicked(move |_| (b_o.open_external_url)(auth_url.clone()));

                let cancel_btn = gtk4::Button::with_label("Cancel Login");
                cancel_btn.set_css_classes(&["ctrl-btn"]);
                let b_c = bridge.clone();
                cancel_btn.connect_clicked(move |_| (b_c.send)(UiCommand::CancelOAuthLogin));

                btn_row.append(&open_btn);
                btn_row.append(&cancel_btn);
            }
            _ => {
                let login_btn = gtk4::Button::with_label("Connect Spotify OAuth");
                login_btn.set_css_classes(&["accent-btn"]);
                let ce = client_entry.clone();
                let b_l = bridge.clone();
                login_btn.connect_clicked(move |_| {
                    let cid = ce.text().trim().to_string();
                    (b_l.send)(UiCommand::StartOAuthLogin {
                        client_id: (!cid.is_empty()).then_some(cid),
                    });
                });
                btn_row.append(&login_btn);

                if matches!(auth, AuthState::Authenticated { .. }) {
                    let out_btn = gtk4::Button::with_label("Sign Out Spotify");
                    out_btn.set_css_classes(&["danger-btn"]);
                    let b_so = bridge.clone();
                    out_btn.connect_clicked(move |_| (b_so.send)(UiCommand::SignOut));
                    btn_row.append(&out_btn);
                }
            }
        }
        card.append(&btn_row);
        self.body_box.append(&card);

        // Zeroconf Settings
        let z_card = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        z_card.add_css_class("surface-elevated");
        let z_hdr = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let z_lbl = gtk4::Label::new(Some("Spotify Connect Discovery (Zeroconf)"));
        z_lbl.set_css_classes(&["heading-md"]);
        z_lbl.set_hexpand(true);
        z_lbl.set_xalign(0.0);
        let z_sw = gtk4::Switch::new();
        z_sw.add_css_class("pulse-switch");
        z_sw.set_active(cfg.zeroconf_enabled);
        z_hdr.append(&z_lbl);
        z_hdr.append(&z_sw);
        z_card.append(&z_hdr);

        let dev_entry = gtk4::Entry::new();
        dev_entry.add_css_class("pulse-entry");
        dev_entry.set_text(&cfg.device_name);
        let save_dev = gtk4::Button::with_label("Apply Device Name");
        save_dev.set_css_classes(&["ctrl-btn"]);
        let b_z = bridge.clone();
        let de_c = dev_entry.clone();
        let sw_c = z_sw.clone();
        save_dev.connect_clicked(move |_| {
            (b_z.send)(UiCommand::SetZeroconf {
                enabled: sw_c.is_active(),
                device_name: de_c.text().to_string(),
            });
        });
        let b_z2 = bridge.clone();
        let de_c2 = dev_entry.clone();
        z_sw.connect_state_set(move |_, active| {
            (b_z2.send)(UiCommand::SetZeroconf {
                enabled: active,
                device_name: de_c2.text().to_string(),
            });
            glib::Propagation::Proceed
        });
        let d_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        dev_entry.set_hexpand(true);
        d_row.append(&dev_entry);
        d_row.append(&save_dev);
        z_card.append(&d_row);
        self.body_box.append(&z_card);
    }

    fn build_audio_appearance_tab(&self, bridge: &UiBridge) {
        let cfg = { bridge.store.borrow().view.config.clone() };

        // Theme Preference
        let theme_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let t_lbl = gtk4::Label::new(Some("Theme Brightness"));
        t_lbl.set_css_classes(&["heading-md"]);
        t_lbl.set_hexpand(true);
        t_lbl.set_xalign(0.0);
        theme_row.append(&t_lbl);
        for (lbl, pref) in [
            ("Dark", ThemePreference::Dark),
            ("Light", ThemePreference::Light),
            ("System", ThemePreference::System),
        ] {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if cfg.theme_preference == pref {
                &["ctrl-btn", "active"]
            } else {
                &["ctrl-btn"]
            });
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::SetThemePreference(pref)));
            theme_row.append(&btn);
        }
        self.body_box.append(&theme_row);

        // Accent Source
        let acc_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let a_lbl = gtk4::Label::new(Some("Accent Palette Source"));
        a_lbl.set_css_classes(&["heading-md"]);
        a_lbl.set_hexpand(true);
        a_lbl.set_xalign(0.0);
        acc_row.append(&a_lbl);
        for (lbl, src) in [
            ("Album Cover", AccentSource::AlbumReactive),
            ("Matugen GTK4", AccentSource::MatugenSystem),
            ("Warm Amber", AccentSource::WarmAmber),
        ] {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if cfg.accent_source == src {
                &["ctrl-btn", "active"]
            } else {
                &["ctrl-btn"]
            });
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::SetAccentSource(src)));
            acc_row.append(&btn);
        }
        self.body_box.append(&acc_row);

        // Visualizer Mode
        let viz_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let v_lbl = gtk4::Label::new(Some("Audio Visualizer"));
        v_lbl.set_css_classes(&["heading-md"]);
        v_lbl.set_hexpand(true);
        v_lbl.set_xalign(0.0);
        viz_row.append(&v_lbl);
        for mode in [
            VisualizerMode::Spectrum,
            VisualizerMode::Waveform,
            VisualizerMode::VuMeters,
            VisualizerMode::Off,
        ] {
            let btn = gtk4::Button::with_label(mode.label());
            btn.set_css_classes(if cfg.visualizer_mode == mode {
                &["ctrl-btn", "active"]
            } else {
                &["ctrl-btn"]
            });
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::SetVisualizerMode(mode)));
            viz_row.append(&btn);
        }
        self.body_box.append(&viz_row);

        // Stream Quality & Normalisation
        let q_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let q_lbl = gtk4::Label::new(Some("Spotify Stream Bitrate"));
        q_lbl.set_css_classes(&["heading-md"]);
        q_lbl.set_hexpand(true);
        q_lbl.set_xalign(0.0);
        q_row.append(&q_lbl);
        for (lbl, q) in [
            ("96 kbps", StreamQuality::Kbps96),
            ("160 kbps", StreamQuality::Kbps160),
            ("320 kbps", StreamQuality::Kbps320),
        ] {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if cfg.stream_quality == q {
                &["ctrl-btn", "active"]
            } else {
                &["ctrl-btn"]
            });
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::SetStreamQuality(q)));
            q_row.append(&btn);
        }
        self.body_box.append(&q_row);

        let n_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let n_lbl = gtk4::Label::new(Some("Volume Normalisation"));
        n_lbl.set_css_classes(&["heading-md"]);
        n_lbl.set_hexpand(true);
        n_lbl.set_xalign(0.0);
        n_row.append(&n_lbl);
        for (lbl, norm) in [
            ("Off", NormalisationMode::Off),
            ("Track", NormalisationMode::Track),
            ("Album", NormalisationMode::Album),
        ] {
            let btn = gtk4::Button::with_label(lbl);
            btn.set_css_classes(if cfg.normalisation == norm {
                &["ctrl-btn", "active"]
            } else {
                &["ctrl-btn"]
            });
            let b = bridge.clone();
            btn.connect_clicked(move |_| (b.send)(UiCommand::SetNormalisation(norm)));
            n_row.append(&btn);
        }
        self.body_box.append(&n_row);

        // Switches: Gapless, MPRIS, Resume session
        for (title, active, cmd_fn) in [
            (
                "Gapless Playback",
                cfg.gapless,
                Box::new(UiCommand::SetGapless) as Box<dyn Fn(bool) -> UiCommand>,
            ),
            (
                "MPRIS2 Desktop Media Integration",
                cfg.mpris_enabled,
                Box::new(UiCommand::SetMprisEnabled),
            ),
            (
                "Restore Last Track & Queue on Launch",
                cfg.resume_session_on_startup,
                Box::new(UiCommand::SetResumeOnStartup),
            ),
        ] {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let lbl = gtk4::Label::new(Some(title));
            lbl.set_css_classes(&["text-secondary"]);
            lbl.set_hexpand(true);
            lbl.set_xalign(0.0);
            let sw = gtk4::Switch::new();
            sw.add_css_class("pulse-switch");
            sw.set_active(active);
            let b = bridge.clone();
            sw.connect_state_set(move |_, v| {
                (b.send)(cmd_fn(v));
                glib::Propagation::Proceed
            });
            row.append(&lbl);
            row.append(&sw);
            self.body_box.append(&row);
        }
    }

    fn build_memory_tab(&self, bridge: &UiBridge) {
        let (tele, cache_cap) = {
            let s = bridge.store.borrow();
            (s.view.telemetry, s.view.config.disk_audio_cache_mb)
        };

        let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        card.add_css_class("surface-elevated");
        let rows = [
            ("Process Resident RAM (VmRSS)", format!("{:.1} MiB", tele.process_rss_mb)),
            ("Peak Resident RAM (VmHWM)", format!("{:.1} MiB", tele.process_hwm_mb)),
            (
                "Cover RAM LRU Cache",
                format!(
                    "{:.2} MiB (Thumbs {}/{}, Heroes {}/{})",
                    tele.cover_cache_mb,
                    tele.cover_thumb_slots_used,
                    tele.cover_thumb_slots_cap,
                    tele.cover_hero_slots_used,
                    tele.cover_hero_slots_cap
                ),
            ),
            ("Loaded Track Records", tele.loaded_track_records.to_string()),
            ("Disk Audio Cache", format!("{:.1} / {} MiB", tele.disk_audio_cache_mb, cache_cap)),
            ("Disk Cover Cache", format!("{:.1} MiB", tele.disk_cover_cache_mb)),
        ];
        for (k, v) in rows {
            let r = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let kl = gtk4::Label::new(Some(k));
            kl.set_css_classes(&["text-secondary"]);
            kl.set_hexpand(true);
            kl.set_xalign(0.0);
            let vl = gtk4::Label::new(Some(&v));
            vl.set_css_classes(&["mono", "text-primary"]);
            r.append(&kl);
            r.append(&vl);
            card.append(&r);
        }
        self.body_box.append(&card);

        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        let purge_covers = gtk4::Button::with_label("Purge Cover Cache");
        purge_covers.set_css_classes(&["ctrl-btn"]);
        let b_pc = bridge.clone();
        purge_covers.connect_clicked(move |_| {
            let count = b_pc
                .store
                .borrow()
                .view
                .crate_view
                .loaded_tracks
                .loaded_count();
            b_pc.covers.purge_all(count);
            (b_pc.send)(UiCommand::PurgeCoverCache);
        });

        let purge_audio = gtk4::Button::with_label("Clear Audio Cache");
        purge_audio.set_css_classes(&["danger-btn"]);
        let b_pa = bridge.clone();
        purge_audio.connect_clicked(move |_| {
            (b_pa.send)(UiCommand::ClearAudioCache);
        });
        actions.append(&purge_covers);
        actions.append(&purge_audio);
        self.body_box.append(&actions);
    }
}
