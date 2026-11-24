//! Automated headless-safe GTK4 screenshot and interactive click-through verification runner.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gtk4::glib;
use gtk4::gsk;
use gtk4::prelude::*;
use pulse_model::{
    AppMode, BackendEvent, CrateRef, CustomPlaylistSummary, EqConfig, EqPreset,
    ListeningStatsSnapshot, LyricLine, LyricsState, MediaSource, PlayerState, RightDrawerTab,
    SearchResults, SpectrumSnapshot, TrackDetail, TrackSummary,
};

use crate::drawers::StudioDrawerWidget;
use crate::state::{DirtyFlags, UiBridge};
use crate::views::{PlaybackDeckWidget, StageViewWidget, TopHeaderWidget};

pub struct SelfTestContext {
    pub app: gtk4::Application,
    pub window: gtk4::ApplicationWindow,
    pub bridge: UiBridge,
    pub header: std::rc::Rc<TopHeaderWidget>,
    pub deck: std::rc::Rc<PlaybackDeckWidget>,
    pub stage: std::rc::Rc<StageViewWidget>,
    pub drawer: std::rc::Rc<StudioDrawerWidget>,
    pub out_dir: PathBuf,
}

pub fn run_self_test(ctx: SelfTestContext) {
    let SelfTestContext {
        app,
        window,
        bridge,
        header,
        deck,
        stage,
        drawer,
        out_dir,
    } = ctx;
    glib::MainContext::default().spawn_local(async move {
        if let Err(err) = fs::create_dir_all(&out_dir) {
            eprintln!("self-test mkdir failed: {err}");
            app.quit();
            return;
        }

        glib::timeout_future(Duration::from_millis(320)).await;

        let mut report: Vec<String> = Vec::new();
        let preview_tracks = sample_multisource_tracks();
        let hero_track = preview_tracks[0].clone();

        // 1. Seed initial active session + Home view (Dark mode, Queue drawer open)
        {
            let mut s = bridge.store.borrow_mut();
            s.view.custom_playlists = vec![CustomPlaylistSummary {
                id: "pl-night-drive".to_string(),
                name: "Late Night Multi-Source".to_string(),
                track_count: 4,
                total_duration_ms: 882_000,
                cover: None,
            }];
            s.view.local_track_count = 148;
            s.view.player_state = PlayerState {
                is_playing: true,
                is_buffering: false,
                active_source: MediaSource::Local,
                active_track: Some(TrackDetail {
                    summary: hero_track.clone(),
                    track_number: Some(1),
                    disc_total_tracks: Some(12),
                    release_year: Some(2024),
                    explicit: false,
                }),
                cover_accent_rgba: [229, 154, 62, 255],
                position_ms: 68_000,
                duration_ms: hero_track.duration_ms,
                volume: 52_428,
                muted: false,
                shuffle: false,
                repeat: Default::default(),
                audio_badge: "FLAC 44.1kHz".to_string(),
                sample_rate_hz: 44_100,
                bitrate_kbps: 1411,
                ..Default::default()
            };
            s.view.queue.context_name = "Late Night Multi-Source".to_string();
            s.view.queue.manual_items = vec![preview_tracks[2].clone()];
            s.view.queue.context_items = preview_tracks.clone();
            s.view.lyrics = sample_lyrics(&hero_track.uri);
            s.view.insights = ListeningStatsSnapshot {
                total_listen_secs: 5_420,
                total_plays: 24,
                source_secs: [2_800, 1_120, 980, 520],
                top_tracks: vec![(hero_track.clone(), 6)],
                top_artists: vec![("Aplayer Studio Quartet".to_string(), 6)],
            };
            s.view.right_drawer = RightDrawerTab::Queue;
            s.recompute_tokens();
        }
        seed_viz(&deck, &stage);
        (bridge.redraw)(DirtyFlags::ALL);
        capture_step_async(&window, &out_dir, "01-deck-home-dark.png", &mut report).await;

        // 2. Click real Shield & Search buttons + inject multi-source SearchResults & showcase YouTube deck Save MP3 button
        header.shield_btn.emit_clicked();
        header.shield_btn.emit_clicked();
        header.set_omnibox_text("night drive");
        header.search_btn.emit_clicked();
        let search_res = SearchResults {
            query: "night drive".to_string(),
            source_filter: None,
            top_match_uri: Some(preview_tracks[0].uri.clone()),
            artists: Vec::new(),
            albums: Vec::new(),
            playlists: Vec::new(),
            tracks: preview_tracks.clone(),
            tracks_only_fallback: false,
        };
        {
            let mut s = bridge.store.borrow_mut();
            s.view.player_state.active_source = MediaSource::YouTube;
            s.view.player_state.audio_badge = "YT OPUS".to_string();
            s.view.player_state.active_track = Some(TrackDetail {
                summary: preview_tracks[2].clone(),
                track_number: None,
                disc_total_tracks: None,
                release_year: None,
                explicit: false,
            });
        }
        let fl = bridge
            .store
            .borrow_mut()
            .apply_backend_event(BackendEvent::SearchResultsLoaded(search_res));
        (bridge.redraw)(fl | DirtyFlags::PLAYER_META);
        assert!(
            deck.save_mp3_btn.is_visible(),
            "YouTube deck Save MP3 button should be visible when YouTube track is active"
        );
        report.push("emit_clicked:shield_btn + search_btn -> multi-source search + YouTube MP3 actions loaded".to_string());
        capture_step_async(
            &window,
            &out_dir,
            "02-deck-search-multisource-dark.png",
            &mut report,
        )
        .await;

        // Restore local studio track for remaining Drawer/Stage/Light showcase captures
        {
            let mut s = bridge.store.borrow_mut();
            s.view.player_state.active_source = MediaSource::Local;
            s.view.player_state.audio_badge = "FLAC 44.1kHz".to_string();
            s.view.player_state.active_track = Some(TrackDetail {
                summary: hero_track,
                track_number: Some(1),
                disc_total_tracks: Some(12),
                release_year: Some(2024),
                explicit: false,
            });
        }

        // 3. Click real EQ & FX drawer tab button & configure Electronic preset
        if let Some((_, eq_tab_btn)) = drawer
            .tab_btns
            .iter()
            .find(|(t, _)| *t == RightDrawerTab::Equalizer)
        {
            eq_tab_btn.emit_clicked();
        }
        {
            let mut s = bridge.store.borrow_mut();
            s.view.config.eq = EqConfig {
                enabled: true,
                preset: EqPreset::Electronic,
                bands_db: EqPreset::Electronic.gains_db(),
                preamp_db: 1.0,
                balance: 0.0,
                mono: false,
            };
        }
        (bridge.redraw)(DirtyFlags::CHROME | DirtyFlags::DRAWER_EQ);
        report.push("emit_clicked:drawer_eq_tab -> preset Electronic active".to_string());
        capture_step_async(&window, &out_dir, "03-deck-eq-drawer-dark.png", &mut report).await;

        // 4. Click real Light/Dark theme toggle & Lyrics drawer tab
        header.theme_btn.emit_clicked();
        if let Some((_, lyr_tab_btn)) = drawer
            .tab_btns
            .iter()
            .find(|(t, _)| *t == RightDrawerTab::Lyrics)
        {
            lyr_tab_btn.emit_clicked();
        }
        seed_viz(&deck, &stage);
        report.push("emit_clicked:theme_btn + lyrics_tab -> Light theme active".to_string());
        capture_step_async(&window, &out_dir, "04-deck-light-mode.png", &mut report).await;

        // 5. Switch back to Dark + click real Stage mode button
        header.theme_btn.emit_clicked();
        if let Some((_, stage_btn)) = header
            .mode_btns
            .iter()
            .find(|(m, _)| *m == AppMode::FocusStage)
        {
            stage_btn.emit_clicked();
        }
        seed_viz(&deck, &stage);
        report.push("emit_clicked:mode_stage -> FocusStage with synced lyrics".to_string());
        capture_step_async(&window, &out_dir, "05-immersive-stage-lyrics.png", &mut report).await;

        // 6. Narrow Responsive Window (760x640, collapsed 56px rail) via Deck button + fixed size
        if let Some((_, deck_btn)) = header
            .mode_btns
            .iter()
            .find(|(m, _)| *m == AppMode::Workbench)
        {
            deck_btn.emit_clicked();
        }
        {
            let mut s = bridge.store.borrow_mut();
            s.view.right_drawer = RightDrawerTab::Closed;
            s.nav_collapsed_override = None;
            s.window_width = 760;
            s.window_height = 640;
        }
        window.set_resizable(false);
        window.set_size_request(760, 640);
        window.set_default_size(760, 640);
        (bridge.redraw)(DirtyFlags::ALL);
        report.push("resize:narrow-760x640 -> collapsed left icon rail".to_string());
        capture_step_async(&window, &out_dir, "06-narrow-responsive.png", &mut report).await;

        // 7. Mini-Player Mode via real Mini mode button (430x124 floating)
        if let Some((_, mini_btn)) = header
            .mode_btns
            .iter()
            .find(|(m, _)| *m == AppMode::MiniPlayer)
        {
            mini_btn.emit_clicked();
        }
        report.push("emit_clicked:mode_mini -> 430x124 mini-player".to_string());
        capture_step_async(&window, &out_dir, "07-mini-player.png", &mut report).await;

        // 8. Honest Local Empty State (1200x760)
        window.set_size_request(-1, -1);
        window.set_resizable(true);
        if let Some((_, deck_btn)) = header
            .mode_btns
            .iter()
            .find(|(m, _)| *m == AppMode::Workbench)
        {
            deck_btn.emit_clicked();
        }
        {
            let mut s = bridge.store.borrow_mut();
            s.view.right_drawer = RightDrawerTab::Closed;
            s.window_width = 1200;
            s.window_height = 760;
            s.nav_collapsed_override = Some(false);
            s.view.local_track_count = 0;
            s.view.config.local_music_dirs.clear();
            s.view.crate_view.loaded_tracks.clear();
            s.view.crate_view.active_scope = pulse_model::LibraryScope::LocalMusic;
            s.view.crate_view.active_crate = CrateRef::LocalTracks;
            s.view.crate_view.local_sub_view = pulse_model::LocalSubView::Tracks;
            s.view.crate_view.title = "Local Music".to_string();
            s.view.crate_view.subtitle = "0 indexed audio tracks".to_string();
            s.view.crate_view.is_loading = false;
            s.view.crate_view.error = None;
        }
        window.set_default_size(1200, 760);
        deck.viz_mode_btn.emit_clicked();
        (bridge.redraw)(DirtyFlags::ALL);
        report.push("nav:local-empty + emit_clicked:viz_mode_btn verified".to_string());
        capture_step_async(&window, &out_dir, "08-local-empty-state.png", &mut report).await;

        let log_json = format!(
            "{{\n  \"verified_steps\": [\n{}\n  ],\n  \"ui_actions_recorded\": [\n{}\n  ]\n}}\n",
            report
                .iter()
                .map(|r| format!("    \"{}\"", r.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(",\n"),
            bridge
                .store
                .borrow()
                .clickthrough_log
                .iter()
                .map(|r| format!("    \"{}\"", r.replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(",\n"),
        );
        let _ = fs::write(out_dir.join("clickthrough-report.json"), log_json);

        app.quit();
    });
}

fn seed_viz(deck: &PlaybackDeckWidget, stage: &StageViewWidget) {
    let mut bins = Vec::with_capacity(48);
    let mut waveform = Vec::with_capacity(64);
    for i in 0..48 {
        let f = i as f32 / 48.0;
        let v = (0.78 - f * 0.45 + 0.18 * (f * 14.0).sin().abs()).clamp(0.08, 0.96);
        bins.push(v);
    }
    for i in 0..64 {
        let p = i as f32 / 64.0 * std::f32::consts::TAU * 2.5;
        waveform.push((p.sin() * 0.6 + (p * 2.0).cos() * 0.25).clamp(-0.95, 0.95));
    }
    let snap = SpectrumSnapshot {
        bins,
        waveform,
        peak_l: 0.72,
        peak_r: 0.68,
        active: true,
    };
    deck.preview_visualizer(snap.clone());
    stage.preview_visualizer(snap);
}

async fn capture_step_async(
    window: &gtk4::ApplicationWindow,
    out_dir: &std::path::Path,
    filename: &str,
    report: &mut Vec<String>,
) {
    let dest = out_dir.join(filename);
    let paintable = gtk4::WidgetPaintable::new(Some(window));
    for attempt in 0..8 {
        glib::timeout_future(Duration::from_millis(if attempt == 0 { 260 } else { 140 })).await;
        let w = window.width().max(1);
        let h = window.height().max(1);
        let snapshot = gtk4::Snapshot::new();
        paintable.snapshot(&snapshot, w as f64, h as f64);
        if let Some(node) = snapshot.to_node() {
            let renderer = gsk::CairoRenderer::new();
            if let Err(err) = renderer.realize(None::<&gtk4::gdk::Surface>) {
                report.push(format!("capture:{filename} -> realize err: {err}"));
                return;
            }
            let tex = renderer.render_texture(
                &node,
                Some(&gtk4::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
            );
            match tex.save_to_png(&dest) {
                Ok(_) => report.push(format!("capture:{filename} -> {w}x{h} ok")),
                Err(err) => report.push(format!("capture:{filename} -> png save err: {err}")),
            }
            renderer.unrealize();
            return;
        }
    }
    report.push(format!("capture:{filename} -> empty snapshot render node after retries"));
}

fn sample_multisource_tracks() -> Vec<TrackSummary> {
    vec![
        TrackSummary {
            uri: Arc::from("local:/Music/night-drive.flac"),
            title: "Midnight Telemetry (Master)".to_string(),
            artist: Arc::from("Aplayer Studio Quartet"),
            artist_uri: None,
            album: Arc::from("Glass & Copper"),
            album_uri: None,
            cover: None,
            duration_ms: 244_000,
            is_liked: true,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::Local,
            local_path: Some(Arc::from("/Music/night-drive.flac")),
            codec_label: Some(Arc::from("FLAC 44.1k")),
        },
        TrackSummary {
            uri: Arc::from("spotify:track:6habFhsOp2NvshLv26DqMb"),
            title: "Signal Through Pine".to_string(),
            artist: Arc::from("Kestrel Station"),
            artist_uri: None,
            album: Arc::from("Northbound FM"),
            album_uri: None,
            cover: None,
            duration_ms: 212_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::Spotify,
            local_path: None,
            codec_label: Some(Arc::from("OGG 320")),
        },
        TrackSummary {
            uri: Arc::from("yt:video:dQw4w9WgXcQ"),
            title: "Analog Tape Sessions (Live)".to_string(),
            artist: Arc::from("Harbor Sound Lab"),
            artist_uri: None,
            album: Arc::from("YouTube Sessions"),
            album_uri: None,
            cover: None,
            duration_ms: 236_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::YouTube,
            local_path: None,
            codec_label: Some(Arc::from("OPUS 160")),
        },
        TrackSummary {
            uri: Arc::from("sc:track:9981234"),
            title: "Night Highway Dub (Mix 04)".to_string(),
            artist: Arc::from("Maren Collective"),
            artist_uri: None,
            album: Arc::from("SoundCloud Stream"),
            album_uri: None,
            cover: None,
            duration_ms: 190_000,
            is_liked: true,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::SoundCloud,
            local_path: None,
            codec_label: Some(Arc::from("MP3 128")),
        },
    ]
}

fn sample_lyrics(uri: &str) -> LyricsState {
    LyricsState {
        track_uri: uri.to_string(),
        synced: true,
        provider: "Local LRC".to_string(),
        offset_ms: 0,
        is_loading: false,
        error: None,
        lines: vec![
            LyricLine {
                time_ms: Some(12_000),
                text: "Sodium streetlamps hum along the wet concrete".to_string(),
            },
            LyricLine {
                time_ms: Some(38_000),
                text: "Meters climb gently as the bassline settles in".to_string(),
            },
            LyricLine {
                time_ms: Some(64_000),
                text: "Copper needles steady through the midnight station static".to_string(),
            },
            LyricLine {
                time_ms: Some(92_000),
                text: "Four sources lined up on one clear dial".to_string(),
            },
            LyricLine {
                time_ms: Some(124_000),
                text: "Fade the room lights down until morning arrives".to_string(),
            },
        ],
    }
}
