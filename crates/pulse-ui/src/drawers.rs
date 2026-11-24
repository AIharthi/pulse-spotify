//! Right Studio Drawer (`Queue`, `Lyrics`, `EQ & FX`, `Insights`) and reusable Lyrics component.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use pulse_assets::{AdaptiveThemeTokens, icons};
use pulse_model::{
    EQ_BAND_FREQS_HZ, EQ_BAND_LABELS, EqConfig, EqPreset, LyricsState, MediaSource, PlaybackSpeed,
    RightDrawerTab, UiCommand,
};

use crate::icons_gtk::{make_clickable, vector_icon_button};
use crate::state::{
    DirtyFlags, UiBridge, format_duration_ms, format_listen_secs, source_badge_css_class,
};

/// Live-updating Lyrics panel shared between Right Studio Drawer and Focus Stage.
pub struct LyricsPanelWidget {
    pub root: gtk4::Box,
    provider_label: gtk4::Label,
    offset_label: gtk4::Label,
    scroller: gtk4::ScrolledWindow,
    lines_box: gtk4::Box,
    line_buttons: RefCell<Vec<(Option<u32>, gtk4::Button)>>,
    active_line_idx: Cell<Option<usize>>,
    rendered_track_uri: RefCell<String>,
    rendered_count: Cell<usize>,
    rendered_loading: Cell<bool>,
    rendered_error: RefCell<Option<String>>,
}

impl LyricsPanelWidget {
    pub fn new(bridge: &UiBridge) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        root.set_vexpand(true);

        let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let provider_label = gtk4::Label::new(Some("No lyrics"));
        provider_label.add_css_class("chip-pill");
        provider_label.set_halign(gtk4::Align::Start);
        provider_label.set_hexpand(true);
        provider_label.set_ellipsize(pango::EllipsizeMode::End);

        let minus_btn = gtk4::Button::with_label("-0.5s");
        minus_btn.set_css_classes(&["ctrl-btn", "mono"]);
        make_clickable(&minus_btn, "Shift lyrics -0.5 seconds");

        let offset_label = gtk4::Label::new(Some("0.0s"));
        offset_label.set_css_classes(&["mono", "text-muted"]);

        let plus_btn = gtk4::Button::with_label("+0.5s");
        plus_btn.set_css_classes(&["ctrl-btn", "mono"]);
        make_clickable(&plus_btn, "Shift lyrics +0.5 seconds");

        let hex = {
            let s = bridge.store.borrow();
            AdaptiveThemeTokens::hex(s.tokens.fg_secondary)
        };
        let (refresh_btn, _) = vector_icon_button(
            &bridge.icons,
            "refresh",
            icons::REFRESH,
            &hex,
            15,
            "Refresh lyrics",
            &["ghost-btn"],
        );

        let b_minus = bridge.clone();
        minus_btn.connect_clicked(move |_| {
            let next = {
                let mut s = b_minus.store.borrow_mut();
                s.view.lyrics.offset_ms = s.view.lyrics.offset_ms.saturating_sub(500);
                s.view.lyrics.offset_ms
            };
            b_minus.record_action("lyrics:offset-500ms");
            (b_minus.send)(UiCommand::SetLyricsOffsetMs(next));
            (b_minus.redraw)(DirtyFlags::DRAWER_LYRICS | DirtyFlags::DRAWER_LYRICS_TICK);
        });

        let b_plus = bridge.clone();
        plus_btn.connect_clicked(move |_| {
            let next = {
                let mut s = b_plus.store.borrow_mut();
                s.view.lyrics.offset_ms = s.view.lyrics.offset_ms.saturating_add(500);
                s.view.lyrics.offset_ms
            };
            b_plus.record_action("lyrics:offset+500ms");
            (b_plus.send)(UiCommand::SetLyricsOffsetMs(next));
            (b_plus.redraw)(DirtyFlags::DRAWER_LYRICS | DirtyFlags::DRAWER_LYRICS_TICK);
        });

        let b_ref = bridge.clone();
        refresh_btn.connect_clicked(move |_| {
            b_ref.record_action("lyrics:refresh");
            (b_ref.send)(UiCommand::RefreshLyrics);
        });

        header.append(&provider_label);
        header.append(&minus_btn);
        header.append(&offset_label);
        header.append(&plus_btn);
        header.append(&refresh_btn);
        root.append(&header);

        let scroller = gtk4::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);

        let lines_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        lines_box.set_margin_top(4);
        lines_box.set_margin_bottom(16);
        scroller.set_child(Some(&lines_box));
        root.append(&scroller);

        Rc::new(Self {
            root,
            provider_label,
            offset_label,
            scroller,
            lines_box,
            line_buttons: RefCell::new(Vec::new()),
            active_line_idx: Cell::new(None),
            rendered_track_uri: RefCell::new(String::new()),
            rendered_count: Cell::new(0),
            rendered_loading: Cell::new(false),
            rendered_error: RefCell::new(None),
        })
    }

    pub fn sync_content(&self, bridge: &UiBridge, lyrics: &LyricsState) {
        let sync_mode = if lyrics.synced {
            "Synced"
        } else if !lyrics.lines.is_empty() {
            "Plain"
        } else {
            "None"
        };
        let prov = if lyrics.provider.is_empty() {
            sync_mode.to_string()
        } else {
            format!("{} ({sync_mode})", lyrics.provider)
        };
        self.provider_label.set_text(&prov);
        self.offset_label
            .set_text(&format!("{:+.1}s", lyrics.offset_ms as f32 / 1000.0));

        let same = *self.rendered_track_uri.borrow() == lyrics.track_uri
            && self.rendered_count.get() == lyrics.lines.len()
            && self.rendered_loading.get() == lyrics.is_loading
            && *self.rendered_error.borrow() == lyrics.error;
        if same {
            return;
        }

        *self.rendered_track_uri.borrow_mut() = lyrics.track_uri.clone();
        self.rendered_count.set(lyrics.lines.len());
        self.rendered_loading.set(lyrics.is_loading);
        *self.rendered_error.borrow_mut() = lyrics.error.clone();
        self.active_line_idx.set(None);

        while let Some(child) = self.lines_box.first_child() {
            self.lines_box.remove(&child);
        }
        self.line_buttons.borrow_mut().clear();

        if lyrics.is_loading {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let spin = gtk4::Spinner::new();
            spin.start();
            let lbl = gtk4::Label::new(Some("Fetching synced lyrics..."));
            lbl.add_css_class("text-secondary");
            row.append(&spin);
            row.append(&lbl);
            row.set_margin_top(24);
            self.lines_box.append(&row);
            return;
        }

        if let Some(ref err) = lyrics.error {
            let box_empty = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            box_empty.set_margin_top(24);
            let title = gtk4::Label::new(Some("Lyrics unavailable"));
            title.set_css_classes(&["heading-md", "text-primary"]);
            title.set_halign(gtk4::Align::Start);
            let body = gtk4::Label::new(Some(err));
            body.set_css_classes(&["text-secondary"]);
            body.set_wrap(true);
            body.set_halign(gtk4::Align::Start);
            box_empty.append(&title);
            box_empty.append(&body);
            self.lines_box.append(&box_empty);
            return;
        }

        if lyrics.lines.is_empty() {
            let box_empty = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            box_empty.set_margin_top(24);
            let title = gtk4::Label::new(Some("No lyrics loaded"));
            title.set_css_classes(&["heading-md", "text-primary"]);
            title.set_halign(gtk4::Align::Start);
            let body = gtk4::Label::new(Some(
                "Start playing any local or streamed track to load embedded .lrc or synced LRCLIB lyrics.",
            ));
            body.set_css_classes(&["text-secondary"]);
            body.set_wrap(true);
            body.set_halign(gtk4::Align::Start);
            box_empty.append(&title);
            box_empty.append(&body);
            self.lines_box.append(&box_empty);
            return;
        }

        let mut vec_btns = self.line_buttons.borrow_mut();
        for line in &lyrics.lines {
            let btn = gtk4::Button::new();
            btn.add_css_class("lyric-line");
            let inner = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            if let Some(ms) = line.time_ms {
                let ts = gtk4::Label::new(Some(&format_duration_ms(ms)));
                ts.set_css_classes(&["mono", "text-muted"]);
                ts.set_valign(gtk4::Align::Start);
                inner.append(&ts);
            }
            let text_lbl = gtk4::Label::new(Some(if line.text.trim().is_empty() {
                "..."
            } else {
                &line.text
            }));
            text_lbl.set_wrap(true);
            text_lbl.set_xalign(0.0);
            text_lbl.set_hexpand(true);
            inner.append(&text_lbl);
            btn.set_child(Some(&inner));

            if let Some(ms) = line.time_ms {
                let b_seek = bridge.clone();
                btn.connect_clicked(move |_| {
                    b_seek.record_action(format!("lyrics:seek:{ms}"));
                    (b_seek.send)(UiCommand::SeekMs(ms));
                });
                make_clickable(&btn, &format!("Jump to {}", format_duration_ms(ms)));
            }
            self.lines_box.append(&btn);
            vec_btns.push((line.time_ms, btn));
        }
    }

    pub fn sync_active_line(&self, position_ms: u32, offset_ms: i32, synced: bool) {
        if !synced {
            return;
        }
        let effective = (position_ms as i64 + offset_ms as i64).max(0) as u32;
        let btns = self.line_buttons.borrow();
        let mut next_idx: Option<usize> = None;
        for (i, (time_opt, _)) in btns.iter().enumerate() {
            if let Some(ms) = time_opt
                && *ms <= effective
            {
                next_idx = Some(i);
            }
        }
        if self.active_line_idx.get() == next_idx {
            return;
        }
        if let Some(old_idx) = self.active_line_idx.get()
            && let Some((_, old_btn)) = btns.get(old_idx)
        {
            old_btn.remove_css_class("is-active");
        }
        if let Some(new_idx) = next_idx
            && let Some((_, new_btn)) = btns.get(new_idx)
        {
            new_btn.add_css_class("is-active");
            let vadj = self.scroller.vadjustment();
            if let Some(bounds) = new_btn.compute_bounds(&self.lines_box) {
                let view_h = vadj.page_size();
                if view_h > 0.0 {
                    let target = (bounds.y() as f64 - view_h * 0.35)
                        .clamp(0.0, (vadj.upper() - view_h).max(0.0));
                    vadj.set_value(target);
                }
            }
        }
        self.active_line_idx.set(next_idx);
    }
}

/// Complete Right Studio Drawer (`Queue`, `Lyrics`, `EQ & FX`, `Insights`).
pub struct StudioDrawerWidget {
    pub root: gtk4::Box,
    pub tab_btns: Vec<(RightDrawerTab, gtk4::Button)>,
    close_btn_img: gtk4::Image,
    stack: gtk4::Stack,
    queue_box: gtk4::Box,
    lyrics_panel: Rc<LyricsPanelWidget>,
    eq_state: Rc<EqControlsState>,
    insights_box: gtk4::Box,
}

struct EqControlsState {
    suppress: Cell<bool>,
    enable_switch: gtk4::Switch,
    preset_dropdown: gtk4::DropDown,
    preamp_scale: gtk4::Scale,
    preamp_val_label: gtk4::Label,
    band_scales: Vec<gtk4::Scale>,
    band_labels: Vec<gtk4::Label>,
    balance_scale: gtk4::Scale,
    balance_val_label: gtk4::Label,
    mono_switch: gtk4::Switch,
    speed_scale: gtk4::Scale,
    speed_val_label: gtk4::Label,
    pitch_switch: gtk4::Switch,
}

impl StudioDrawerWidget {
    pub fn new(bridge: &UiBridge) -> Rc<Self> {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        root.add_css_class("right-drawer");
        root.set_size_request(316, -1);

        // Header bar with tab pills + Close button
        let tab_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        let tabs = [
            (RightDrawerTab::Queue, "Queue", "queue"),
            (RightDrawerTab::Lyrics, "Lyrics", "lyrics"),
            (RightDrawerTab::Equalizer, "EQ & FX", "sliders_eq"),
            (RightDrawerTab::Insights, "Insights", "insights"),
        ];
        let mut tab_btns = Vec::new();
        for (tab, label, _key) in tabs {
            let btn = gtk4::Button::with_label(label);
            btn.set_css_classes(&["seg-btn"]);
            make_clickable(&btn, label);
            let b_tab = bridge.clone();
            btn.connect_clicked(move |_| {
                {
                    let mut s = b_tab.store.borrow_mut();
                    s.view.right_drawer = tab;
                }
                b_tab.record_action(format!("drawer:{label}"));
                (b_tab.redraw)(
                    DirtyFlags::CHROME
                        | DirtyFlags::DRAWER_QUEUE
                        | DirtyFlags::DRAWER_LYRICS
                        | DirtyFlags::DRAWER_EQ
                        | DirtyFlags::DRAWER_INSIGHTS,
                );
            });
            tab_bar.append(&btn);
            tab_btns.push((tab, btn));
        }

        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        tab_bar.append(&spacer);

        let fg_sec = {
            let s = bridge.store.borrow();
            AdaptiveThemeTokens::hex(s.tokens.fg_secondary)
        };
        let (close_btn, close_btn_img) = vector_icon_button(
            &bridge.icons,
            "close",
            icons::CLOSE,
            &fg_sec,
            15,
            "Close studio drawer",
            &["ghost-btn"],
        );
        let b_close = bridge.clone();
        close_btn.connect_clicked(move |_| {
            {
                let mut s = b_close.store.borrow_mut();
                s.view.right_drawer = RightDrawerTab::Closed;
            }
            b_close.record_action("drawer:close");
            (b_close.redraw)(DirtyFlags::CHROME);
        });
        tab_bar.append(&close_btn);
        root.append(&tab_bar);

        let stack = gtk4::Stack::new();
        stack.set_vexpand(true);
        stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        stack.set_transition_duration(120);

        // 1. Queue Page
        let queue_scroller = gtk4::ScrolledWindow::new();
        queue_scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        queue_scroller.set_vexpand(true);
        let queue_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        queue_scroller.set_child(Some(&queue_box));
        stack.add_named(&queue_scroller, Some("queue"));

        // 2. Lyrics Page
        let lyrics_panel = LyricsPanelWidget::new(bridge);
        stack.add_named(&lyrics_panel.root, Some("lyrics"));

        // 3. EQ & FX Page
        let (eq_scroll, eq_state) = build_eq_panel(bridge);
        stack.add_named(&eq_scroll, Some("eq"));

        // 4. Insights Page
        let insights_scroller = gtk4::ScrolledWindow::new();
        insights_scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        insights_scroller.set_vexpand(true);
        let insights_box = gtk4::Box::new(gtk4::Orientation::Vertical, 10);
        insights_scroller.set_child(Some(&insights_box));
        stack.add_named(&insights_scroller, Some("insights"));

        root.append(&stack);

        Rc::new(Self {
            root,
            tab_btns,
            close_btn_img,
            stack,
            queue_box,
            lyrics_panel,
            eq_state,
            insights_box,
        })
    }

    pub fn sync(&self, bridge: &UiBridge, flags: DirtyFlags) {
        let (tab, width, tokens) = {
            let s = bridge.store.borrow();
            (s.view.right_drawer, s.window_width, s.tokens.clone())
        };

        let visible = tab != RightDrawerTab::Closed && (width >= 780 || flags.any(DirtyFlags::DRAWER_QUEUE | DirtyFlags::DRAWER_LYRICS | DirtyFlags::DRAWER_EQ | DirtyFlags::DRAWER_INSIGHTS));
        self.root.set_visible(tab != RightDrawerTab::Closed && visible);

        if flags.any(DirtyFlags::THEME) {
            let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);
            bridge
                .icons
                .apply_to_image(&self.close_btn_img, "close", icons::CLOSE, &fg_sec, 15);
        }

        for (t_id, btn) in &self.tab_btns {
            if *t_id == tab {
                btn.add_css_class("active");
            } else {
                btn.remove_css_class("active");
            }
        }

        match tab {
            RightDrawerTab::Queue => self.stack.set_visible_child_name("queue"),
            RightDrawerTab::Lyrics => self.stack.set_visible_child_name("lyrics"),
            RightDrawerTab::Equalizer => self.stack.set_visible_child_name("eq"),
            RightDrawerTab::Insights => self.stack.set_visible_child_name("insights"),
            RightDrawerTab::Closed => {}
        }

        if tab == RightDrawerTab::Queue && flags.any(DirtyFlags::DRAWER_QUEUE | DirtyFlags::THEME | DirtyFlags::ALL) {
            self.rebuild_queue(bridge);
        }
        if tab == RightDrawerTab::Lyrics {
            let (lyrics, pos) = {
                let s = bridge.store.borrow();
                (s.view.lyrics.clone(), s.view.player_state.position_ms)
            };
            if flags.any(DirtyFlags::DRAWER_LYRICS | DirtyFlags::ALL) {
                self.lyrics_panel.sync_content(bridge, &lyrics);
            }
            if flags.any(DirtyFlags::DRAWER_LYRICS | DirtyFlags::DRAWER_LYRICS_TICK | DirtyFlags::ALL) {
                self.lyrics_panel
                    .sync_active_line(pos, lyrics.offset_ms, lyrics.synced);
            }
        }
        if tab == RightDrawerTab::Equalizer && flags.any(DirtyFlags::DRAWER_EQ | DirtyFlags::ALL) {
            let (eq, speed) = {
                let s = bridge.store.borrow();
                (s.view.config.eq.clone(), s.view.config.playback_speed)
            };
            sync_eq_controls(&self.eq_state, &eq, speed);
        }
        if tab == RightDrawerTab::Insights && flags.any(DirtyFlags::DRAWER_INSIGHTS | DirtyFlags::ALL) {
            self.rebuild_insights(bridge);
        }
    }

    fn rebuild_queue(&self, bridge: &UiBridge) {
        while let Some(c) = self.queue_box.first_child() {
            self.queue_box.remove(&c);
        }
        let (queue, active_uri, tokens) = {
            let s = bridge.store.borrow();
            (
                s.view.queue.clone(),
                s.view
                    .player_state
                    .active_track
                    .as_ref()
                    .map(|t| t.summary.uri.clone()),
                s.tokens.clone(),
            )
        };
        let fg_sec = AdaptiveThemeTokens::hex(tokens.fg_secondary);

        // Action row for manual queue
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let save_btn = gtk4::Button::with_label("Save Playlist");
        save_btn.set_css_classes(&["ctrl-btn"]);
        save_btn.set_sensitive(!queue.manual_items.is_empty() || active_uri.is_some());
        make_clickable(&save_btn, "Save current queue as a cross-source playlist");
        let b_save = bridge.clone();
        save_btn.connect_clicked(move |_| {
            let b_confirm = b_save.clone();
            (b_save.prompt_text_dialog)(
                "Save Queue as Playlist".to_string(),
                "Playlist name".to_string(),
                "Queued Session".to_string(),
                Rc::new(move |name| {
                    b_confirm.record_action("queue:save-playlist");
                    (b_confirm.send)(UiCommand::SaveQueueAsPlaylist(name));
                }),
            );
        });

        let dedup_btn = gtk4::Button::with_label("Deduplicate");
        dedup_btn.set_css_classes(&["ghost-btn"]);
        dedup_btn.set_sensitive(queue.manual_items.len() > 1);
        make_clickable(&dedup_btn, "Remove duplicate tracks from Queued Next");
        let b_dedup = bridge.clone();
        dedup_btn.connect_clicked(move |_| {
            b_dedup.record_action("queue:dedup");
            (b_dedup.send)(UiCommand::DeduplicateManualQueue);
        });

        let clear_btn = gtk4::Button::with_label("Clear");
        clear_btn.set_css_classes(&["ghost-btn"]);
        clear_btn.set_sensitive(!queue.manual_items.is_empty());
        make_clickable(&clear_btn, "Clear Queued Next items");
        let b_clr = bridge.clone();
        clear_btn.connect_clicked(move |_| {
            b_clr.record_action("queue:clear");
            (b_clr.send)(UiCommand::ClearManualQueue);
        });

        actions.append(&save_btn);
        actions.append(&dedup_btn);
        actions.append(&clear_btn);
        self.queue_box.append(&actions);

        // Section 1: Queued Next (manual priority)
        let q_header = gtk4::Label::new(Some(&format!(
            "Queued Next ({})",
            queue.manual_items.len()
        )));
        q_header.set_css_classes(&["section-kicker"]);
        q_header.set_halign(gtk4::Align::Start);
        self.queue_box.append(&q_header);

        if queue.manual_items.is_empty() {
            let empty_lbl = gtk4::Label::new(Some(
                "No manual tracks queued. Click + Next on any Local, Spotify, YouTube, or SoundCloud track.",
            ));
            empty_lbl.set_css_classes(&["text-muted"]);
            empty_lbl.set_wrap(true);
            empty_lbl.set_xalign(0.0);
            self.queue_box.append(&empty_lbl);
        } else {
            for (idx, tr) in queue.manual_items.iter().enumerate() {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                row.add_css_class("track-row");

                let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
                meta.set_hexpand(true);
                let title = gtk4::Label::new(Some(&tr.title));
                title.set_css_classes(&["heading-md"]);
                title.set_xalign(0.0);
                title.set_ellipsize(pango::EllipsizeMode::End);
                let sub = gtk4::Label::new(Some(&format!("{} · {}", tr.artist, tr.source.display_name())));
                sub.set_css_classes(&["text-muted"]);
                sub.set_xalign(0.0);
                sub.set_ellipsize(pango::EllipsizeMode::End);
                meta.append(&title);
                meta.append(&sub);

                let play_btn = gtk4::Button::new();
                let play_img = bridge.icons.make_image("play", icons::PLAY, &fg_sec, 13);
                play_btn.set_child(Some(&play_img));
                play_btn.set_css_classes(&["ghost-btn"]);
                make_clickable(&play_btn, "Play queued track");
                let b_p = bridge.clone();
                play_btn.connect_clicked(move |_| {
                    (b_p.send)(UiCommand::PlayQueueIndex(idx));
                });

                let rm_btn = gtk4::Button::new();
                let rm_img = bridge.icons.make_image("close", icons::CLOSE, &fg_sec, 13);
                rm_btn.set_child(Some(&rm_img));
                rm_btn.set_css_classes(&["ghost-btn"]);
                make_clickable(&rm_btn, "Remove from queue");
                let b_r = bridge.clone();
                rm_btn.connect_clicked(move |_| {
                    (b_r.send)(UiCommand::RemoveManualQueueItem(idx));
                });

                row.append(&play_btn);
                row.append(&meta);
                row.append(&rm_btn);
                self.queue_box.append(&row);
            }
        }

        // Section 2: Up Next from Context
        let ctx_title = if queue.context_name.is_empty() {
            "Up Next from Context".to_string()
        } else {
            format!("Up Next: {}", queue.context_name)
        };
        let c_header = gtk4::Label::new(Some(&ctx_title));
        c_header.set_css_classes(&["section-kicker"]);
        c_header.set_halign(gtk4::Align::Start);
        c_header.set_margin_top(6);
        self.queue_box.append(&c_header);

        if queue.context_items.is_empty() {
            let empty_ctx = gtk4::Label::new(Some("Context queue is empty."));
            empty_ctx.set_css_classes(&["text-muted"]);
            empty_ctx.set_xalign(0.0);
            self.queue_box.append(&empty_ctx);
        } else {
            let limit = queue.context_items.len().min(40);
            for (ctx_idx, tr) in queue.context_items.iter().take(limit).enumerate() {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
                row.add_css_class("track-row");
                if active_uri.as_ref().is_some_and(|u| u == &tr.uri) {
                    row.add_css_class("is-playing");
                }
                let meta = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
                meta.set_hexpand(true);
                let title = gtk4::Label::new(Some(&tr.title));
                title.set_xalign(0.0);
                title.set_ellipsize(pango::EllipsizeMode::End);
                let sub = gtk4::Label::new(Some(&format!("{} · {}", tr.artist, format_duration_ms(tr.duration_ms))));
                sub.set_css_classes(&["text-muted", "mono"]);
                sub.set_xalign(0.0);
                sub.set_ellipsize(pango::EllipsizeMode::End);
                meta.append(&title);
                meta.append(&sub);

                let promote_btn = gtk4::Button::new();
                let p_img = bridge.icons.make_image("plus", icons::PLUS, &fg_sec, 13);
                promote_btn.set_child(Some(&p_img));
                promote_btn.set_css_classes(&["ghost-btn"]);
                make_clickable(&promote_btn, "Move to Queued Next");
                let b_pr = bridge.clone();
                promote_btn.connect_clicked(move |_| {
                    (b_pr.send)(UiCommand::PromoteContextToManual {
                        context_index: ctx_idx,
                        manual_index: 0,
                    });
                });

                row.append(&meta);
                row.append(&promote_btn);
                self.queue_box.append(&row);
            }
        }
    }

    fn rebuild_insights(&self, bridge: &UiBridge) {
        while let Some(c) = self.insights_box.first_child() {
            self.insights_box.remove(&c);
        }
        let stats = { bridge.store.borrow().view.insights.clone() };

        if stats.total_plays == 0 && stats.total_listen_secs == 0 {
            let card = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
            card.add_css_class("surface-elevated");
            let title = gtk4::Label::new(Some("No listening history yet"));
            title.set_css_classes(&["heading-md"]);
            title.set_xalign(0.0);
            let sub = gtk4::Label::new(Some(
                "Only real local listening sessions are tracked here. Play music for 15 seconds to record your first track play.",
            ));
            sub.set_css_classes(&["text-secondary"]);
            sub.set_wrap(true);
            sub.set_xalign(0.0);
            card.append(&title);
            card.append(&sub);
            self.insights_box.append(&card);
            return;
        }

        let summary_card = gtk4::Box::new(gtk4::Orientation::Horizontal, 14);
        summary_card.add_css_class("surface-elevated");
        for (lbl_txt, val_txt) in [
            ("Total Listening", format_listen_secs(stats.total_listen_secs)),
            ("Tracks Played", stats.total_plays.to_string()),
        ] {
            let col = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            col.set_hexpand(true);
            let k = gtk4::Label::new(Some(lbl_txt));
            k.set_css_classes(&["text-muted"]);
            k.set_xalign(0.0);
            let v = gtk4::Label::new(Some(&val_txt));
            v.set_css_classes(&["heading-lg", "mono"]);
            v.set_xalign(0.0);
            col.append(&k);
            col.append(&v);
            summary_card.append(&col);
        }
        self.insights_box.append(&summary_card);

        // Source Breakdown
        let src_hdr = gtk4::Label::new(Some("By Source"));
        src_hdr.set_css_classes(&["section-kicker"]);
        src_hdr.set_xalign(0.0);
        self.insights_box.append(&src_hdr);

        let sources = [
            (MediaSource::Local, stats.source_secs[0]),
            (MediaSource::Spotify, stats.source_secs[1]),
            (MediaSource::YouTube, stats.source_secs[2]),
            (MediaSource::SoundCloud, stats.source_secs[3]),
        ];
        let total = stats.source_secs.iter().sum::<u64>().max(1);
        for (src, secs) in sources {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
            let badge = gtk4::Label::new(Some(src.badge_label()));
            badge.set_css_classes(&["chip-pill", source_badge_css_class(src)]);
            let bar = gtk4::ProgressBar::new();
            bar.set_fraction(secs as f64 / total as f64);
            bar.set_hexpand(true);
            bar.set_valign(gtk4::Align::Center);
            let val = gtk4::Label::new(Some(&format_listen_secs(secs)));
            val.set_css_classes(&["mono", "text-secondary"]);
            row.append(&badge);
            row.append(&bar);
            row.append(&val);
            self.insights_box.append(&row);
        }

        if !stats.top_tracks.is_empty() {
            let tt_hdr = gtk4::Label::new(Some("Most Played Tracks"));
            tt_hdr.set_css_classes(&["section-kicker"]);
            tt_hdr.set_xalign(0.0);
            tt_hdr.set_margin_top(6);
            self.insights_box.append(&tt_hdr);

            for (tr, plays) in stats.top_tracks.iter().take(6) {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                row.add_css_class("track-row");
                let lbl = gtk4::Label::new(Some(&format!("{} - {}", tr.title, tr.artist)));
                lbl.set_xalign(0.0);
                lbl.set_hexpand(true);
                lbl.set_ellipsize(pango::EllipsizeMode::End);
                let cnt = gtk4::Label::new(Some(&format!("{plays}x")));
                cnt.set_css_classes(&["mono", "text-muted"]);
                row.append(&lbl);
                row.append(&cnt);
                self.insights_box.append(&row);
            }
        }
    }
}

fn build_eq_panel(bridge: &UiBridge) -> (gtk4::ScrolledWindow, Rc<EqControlsState>) {
    let scroller = gtk4::ScrolledWindow::new();
    scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroller.set_vexpand(true);

    let root = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    scroller.set_child(Some(&root));

    let suppress = Rc::new(Cell::new(false));
    let (init_eq, init_speed) = {
        let s = bridge.store.borrow();
        (s.view.config.eq.clone(), s.view.config.playback_speed)
    };

    // Enable switch + Preset selector
    let top_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let en_lbl = gtk4::Label::new(Some("10-Band Biquad EQ"));
    en_lbl.set_css_classes(&["heading-md"]);
    en_lbl.set_hexpand(true);
    en_lbl.set_xalign(0.0);
    let enable_switch = gtk4::Switch::new();
    enable_switch.add_css_class("pulse-switch");
    enable_switch.set_active(init_eq.enabled);
    make_clickable(&enable_switch, "Toggle 10-band equalizer DSP");
    top_row.append(&en_lbl);
    top_row.append(&enable_switch);
    root.append(&top_row);

    let preset_names: Vec<&str> = EqPreset::ALL.iter().map(|p| p.label()).collect();
    let preset_model = gtk4::StringList::new(&preset_names);
    let preset_dropdown = gtk4::DropDown::new(Some(preset_model), gtk4::Expression::NONE);
    preset_dropdown.add_css_class("pulse-dropdown");
    let reset_btn = gtk4::Button::with_label("Reset Flat");
    reset_btn.set_css_classes(&["ghost-btn"]);
    make_clickable(&reset_btn, "Reset all EQ bands to 0.0 dB");
    let preset_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    preset_dropdown.set_hexpand(true);
    preset_row.append(&preset_dropdown);
    preset_row.append(&reset_btn);
    root.append(&preset_row);

    // Preamp
    let pre_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let pre_title = gtk4::Label::new(Some("Preamp"));
    pre_title.set_css_classes(&["text-secondary"]);
    let preamp_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, -12.0, 12.0, 0.5);
    preamp_scale.add_css_class("pulse-scale");
    preamp_scale.set_hexpand(true);
    let preamp_val_label = gtk4::Label::new(Some(&format!("{:+.1} dB", init_eq.preamp_db)));
    preamp_val_label.set_css_classes(&["mono", "text-muted"]);
    preamp_val_label.set_width_chars(8);
    pre_row.append(&pre_title);
    pre_row.append(&preamp_scale);
    pre_row.append(&preamp_val_label);
    root.append(&pre_row);

    // 10 vertical band sliders
    let bands_grid = gtk4::Grid::new();
    bands_grid.set_column_spacing(4);
    bands_grid.set_row_spacing(4);
    bands_grid.set_column_homogeneous(true);
    bands_grid.add_css_class("surface-elevated");

    let mut band_scales = Vec::with_capacity(10);
    let mut band_labels = Vec::with_capacity(10);
    for (i, label) in EQ_BAND_LABELS.iter().enumerate() {
        let val_lbl = gtk4::Label::new(Some(&format!("{:+.0}", init_eq.bands_db[i])));
        val_lbl.set_css_classes(&["mono", "text-muted"]);
        let slider = gtk4::Scale::with_range(gtk4::Orientation::Vertical, -12.0, 12.0, 0.5);
        slider.set_inverted(true);
        slider.add_css_class("pulse-scale");
        slider.set_size_request(-1, 130);
        slider.set_vexpand(false);
        make_clickable(&slider, &format!("{} Hz band", EQ_BAND_FREQS_HZ[i]));
        let freq_lbl = gtk4::Label::new(Some(label));
        freq_lbl.set_css_classes(&["mono", "text-secondary"]);

        bands_grid.attach(&val_lbl, i as i32, 0, 1, 1);
        bands_grid.attach(&slider, i as i32, 1, 1, 1);
        bands_grid.attach(&freq_lbl, i as i32, 2, 1, 1);
        band_scales.push(slider);
        band_labels.push(val_lbl);
    }
    root.append(&bands_grid);

    // Stereo balance & Mono toggle
    let bal_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let bal_lbl = gtk4::Label::new(Some("Balance"));
    bal_lbl.set_css_classes(&["text-secondary"]);
    let balance_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, -1.0, 1.0, 0.05);
    balance_scale.add_css_class("pulse-scale");
    balance_scale.set_hexpand(true);
    let balance_val_label = gtk4::Label::new(Some("C"));
    balance_val_label.set_css_classes(&["mono", "text-muted"]);
    balance_val_label.set_width_chars(6);
    bal_row.append(&bal_lbl);
    bal_row.append(&balance_scale);
    bal_row.append(&balance_val_label);
    root.append(&bal_row);

    let mono_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let mono_lbl = gtk4::Label::new(Some("Mono Downmix"));
    mono_lbl.set_css_classes(&["text-secondary"]);
    mono_lbl.set_hexpand(true);
    mono_lbl.set_xalign(0.0);
    let mono_switch = gtk4::Switch::new();
    mono_switch.add_css_class("pulse-switch");
    mono_row.append(&mono_lbl);
    mono_row.append(&mono_switch);
    root.append(&mono_row);

    // Playback Speed & Pitch Mode
    let speed_card = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    speed_card.add_css_class("surface-elevated");
    let speed_hdr = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let sp_lbl = gtk4::Label::new(Some("Playback Speed (Direct Engine)"));
    sp_lbl.set_css_classes(&["heading-md"]);
    sp_lbl.set_hexpand(true);
    sp_lbl.set_xalign(0.0);
    let speed_val_label = gtk4::Label::new(Some("1.00x"));
    speed_val_label.set_css_classes(&["mono", "text-accent"]);
    speed_hdr.append(&sp_lbl);
    speed_hdr.append(&speed_val_label);
    speed_card.append(&speed_hdr);

    let speed_scale = gtk4::Scale::with_range(gtk4::Orientation::Horizontal, 0.50, 2.00, 0.05);
    speed_scale.add_css_class("pulse-scale");
    speed_card.append(&speed_scale);

    let pitch_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let pitch_lbl = gtk4::Label::new(Some("Preserve Pitch (off = Tape Varispeed)"));
    pitch_lbl.set_css_classes(&["text-secondary"]);
    pitch_lbl.set_hexpand(true);
    pitch_lbl.set_xalign(0.0);
    let pitch_switch = gtk4::Switch::new();
    pitch_switch.add_css_class("pulse-switch");
    pitch_row.append(&pitch_lbl);
    pitch_row.append(&pitch_switch);
    speed_card.append(&pitch_row);
    root.append(&speed_card);

    let eq_state = Rc::new(EqControlsState {
        suppress: suppress.as_ref().clone(),
        enable_switch,
        preset_dropdown,
        preamp_scale,
        preamp_val_label,
        band_scales,
        band_labels,
        balance_scale,
        balance_val_label,
        mono_switch,
        speed_scale,
        speed_val_label,
        pitch_switch,
    });
    sync_eq_controls(&eq_state, &init_eq, init_speed);

    // Wire signals cleanly
    let emit_eq_now = {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        move |eq: EqConfig| {
            sync_eq_controls(&st, &eq, b.store.borrow().view.config.playback_speed);
            b.store.borrow_mut().view.config.eq = eq.clone();
            (b.send)(UiCommand::SetEqConfig(eq));
        }
    };

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        eq_state.enable_switch.connect_state_set(move |_, active| {
            if st.suppress.get() {
                return glib::Propagation::Proceed;
            }
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.enabled = active;
            b.record_action(format!("eq:enabled:{active}"));
            emit(eq);
            glib::Propagation::Proceed
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        eq_state.preset_dropdown.connect_selected_notify(move |dd| {
            if st.suppress.get() {
                return;
            }
            let idx = dd.selected() as usize;
            if let Some(&preset) = EqPreset::ALL.get(idx) {
                let mut eq = b.store.borrow().view.config.eq.clone();
                eq.preset = preset;
                if preset != EqPreset::Custom {
                    eq.bands_db = preset.gains_db();
                    eq.enabled = preset != EqPreset::Flat;
                }
                b.record_action(format!("eq:preset:{}", preset.label()));
                emit(eq);
            }
        });
    }

    {
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        reset_btn.connect_clicked(move |_| {
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.preset = EqPreset::Flat;
            eq.bands_db = [0.0; 10];
            eq.preamp_db = 0.0;
            eq.balance = 0.0;
            eq.mono = false;
            b.record_action("eq:reset-flat");
            emit(eq);
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        eq_state.preamp_scale.connect_value_changed(move |sc| {
            if st.suppress.get() {
                return;
            }
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.preamp_db = sc.value() as f32;
            emit(eq);
        });
    }

    for (band_idx, slider) in eq_state.band_scales.iter().enumerate() {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        slider.connect_value_changed(move |sc| {
            if st.suppress.get() {
                return;
            }
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.bands_db[band_idx] = sc.value() as f32;
            eq.preset = EqPreset::Custom;
            eq.enabled = true;
            emit(eq);
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        eq_state.balance_scale.connect_value_changed(move |sc| {
            if st.suppress.get() {
                return;
            }
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.balance = sc.value() as f32;
            emit(eq);
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        let emit = emit_eq_now.clone();
        eq_state.mono_switch.connect_state_set(move |_, active| {
            if st.suppress.get() {
                return glib::Propagation::Proceed;
            }
            let mut eq = b.store.borrow().view.config.eq.clone();
            eq.mono = active;
            b.record_action(format!("eq:mono:{active}"));
            emit(eq);
            glib::Propagation::Proceed
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        eq_state.speed_scale.connect_value_changed(move |sc| {
            if st.suppress.get() {
                return;
            }
            let rate = ((sc.value() * 20.0).round() / 20.0) as f32;
            st.speed_val_label.set_text(&format!("{rate:.2}x"));
            let preserve = b.store.borrow().view.config.playback_speed.preserve_pitch;
            let next = PlaybackSpeed {
                rate,
                preserve_pitch: preserve,
            };
            b.store.borrow_mut().view.config.playback_speed = next;
            (b.send)(UiCommand::SetPlaybackSpeed(next));
        });
    }

    {
        let st = Rc::clone(&eq_state);
        let b = bridge.clone();
        eq_state.pitch_switch.connect_state_set(move |_, active| {
            if st.suppress.get() {
                return glib::Propagation::Proceed;
            }
            let rate = b.store.borrow().view.config.playback_speed.rate;
            let next = PlaybackSpeed {
                rate,
                preserve_pitch: active,
            };
            b.store.borrow_mut().view.config.playback_speed = next;
            (b.send)(UiCommand::SetPlaybackSpeed(next));
            glib::Propagation::Proceed
        });
    }

    (scroller, eq_state)
}

fn sync_eq_controls(st: &EqControlsState, eq: &EqConfig, speed: PlaybackSpeed) {
    st.suppress.set(true);
    st.enable_switch.set_active(eq.enabled);
    if let Some(pos) = EqPreset::ALL.iter().position(|&p| p == eq.preset) {
        st.preset_dropdown.set_selected(pos as u32);
    }
    st.preamp_scale.set_value(f64::from(eq.preamp_db));
    st.preamp_val_label.set_text(&format!("{:+.1} dB", eq.preamp_db));
    for (i, &db) in eq.bands_db.iter().enumerate() {
        if let Some(s) = st.band_scales.get(i) {
            s.set_value(f64::from(db));
        }
        if let Some(l) = st.band_labels.get(i) {
            l.set_text(&format!("{db:+.0}"));
        }
    }
    st.balance_scale.set_value(f64::from(eq.balance));
    let bal_txt = if eq.balance.abs() < 0.04 {
        "C".to_string()
    } else if eq.balance < 0.0 {
        format!("L{:.0}%", eq.balance.abs() * 100.0)
    } else {
        format!("R{:.0}%", eq.balance * 100.0)
    };
    st.balance_val_label.set_text(&bal_txt);
    st.mono_switch.set_active(eq.mono);
    st.speed_scale.set_value(f64::from(speed.rate));
    st.speed_val_label.set_text(&format!("{:.2}x", speed.rate));
    st.pitch_switch.set_active(speed.preserve_pitch);
    st.suppress.set(false);
}
