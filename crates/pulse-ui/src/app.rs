//! Top-level GTK4 `Application` lifecycle, reactive backend stream consumer, and layout controller.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use pulse_model::{AppMode, UiCommand};

use crate::PulseAppLaunch;
use crate::dialogs::SettingsModalController;
use crate::drawers::StudioDrawerWidget;
use crate::icons_gtk::{CoverTextureStore, VectorIconCache};
use crate::keybindings::install_key_controller;
use crate::self_test::{SelfTestContext, run_self_test};
use crate::state::{DirtyFlags, FocusFn, PromptDialogFn, RedrawFn, UiBridge, UiStore};
use crate::theme_css::{apply_theme_css, install_theme_provider};
use crate::views::{
    CenterCanvasWidget, LeftRailWidget, MiniPlayerWidget, PlaybackDeckWidget, StageViewWidget,
    TopHeaderWidget,
};

/// Launches the native GTK4 application and blocks until exit.
pub fn run(launch: PulseAppLaunch) -> Result<(), String> {
    let app = gtk4::Application::builder()
        .application_id("dev.pulse.Spotify")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let launch_cell = RefCell::new(Some(launch));

    app.connect_activate(move |app| {
        let Some(launch) = launch_cell.borrow_mut().take() else {
            if let Some(win) = app.active_window() {
                win.present();
            }
            return;
        };

        let store = Rc::new(RefCell::new(UiStore::new(launch.initial_view)));
        let icons = Rc::new(VectorIconCache::new());
        let covers = Rc::new(CoverTextureStore::new(
            launch.cover_cache,
            launch.cover_fetcher,
            launch.metrics_observer,
        ));
        let css_provider = install_theme_provider(&store.borrow().tokens);

        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .title("Pulse")
            .default_width(1280)
            .default_height(820)
            .build();
        window.add_css_class("pulse-root");

        let redraw_slot: Rc<RefCell<Option<RedrawFn>>> = Rc::new(RefCell::new(None));
        let redraw_dispatcher: RedrawFn = {
            let slot = Rc::clone(&redraw_slot);
            Rc::new(move |flags| {
                if let Some(ref f) = *slot.borrow() {
                    f(flags);
                }
            })
        };

        let focus_slot: Rc<RefCell<Option<FocusFn>>> = Rc::new(RefCell::new(None));
        let focus_dispatcher: FocusFn = {
            let slot = Rc::clone(&focus_slot);
            Rc::new(move |grab| {
                if let Some(ref f) = *slot.borrow() {
                    f(grab);
                }
            })
        };

        let win_weak = window.downgrade();
        let send_folder = launch.command_sender.clone();
        let open_folder_dialog: Rc<dyn Fn()> = Rc::new(move || {
            let Some(win) = win_weak.upgrade() else { return };
            let dlg = gtk4::FileDialog::builder()
                .title("Add Local Music Folder")
                .modal(true)
                .build();
            let send = send_folder.clone();
            dlg.select_folder(Some(&win), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    (send)(UiCommand::AddLocalMusicFolder(
                        path.to_string_lossy().to_string(),
                    ));
                }
            });
        });

        let win_weak = window.downgrade();
        let send_files = launch.command_sender.clone();
        let open_files_dialog: Rc<dyn Fn()> = Rc::new(move || {
            let Some(win) = win_weak.upgrade() else { return };
            let dlg = gtk4::FileDialog::builder()
                .title("Open Audio Files")
                .modal(true)
                .build();
            let send = send_files.clone();
            dlg.open_multiple(Some(&win), gio::Cancellable::NONE, move |res| {
                if let Ok(model) = res {
                    let mut paths = Vec::new();
                    for i in 0..model.n_items() {
                        if let Some(obj) = model.item(i)
                            && let Ok(file) = obj.downcast::<gio::File>()
                            && let Some(p) = file.path()
                        {
                            paths.push(p.to_string_lossy().to_string());
                        }
                    }
                    if !paths.is_empty() {
                        (send)(UiCommand::OpenLocalPaths(paths));
                    }
                }
            });
        });

        let win_weak = window.downgrade();
        let send_m3u = launch.command_sender.clone();
        let import_m3u_dialog: Rc<dyn Fn()> = Rc::new(move || {
            let Some(win) = win_weak.upgrade() else { return };
            let dlg = gtk4::FileDialog::builder()
                .title("Import .m3u / .m3u8 Playlist")
                .modal(true)
                .build();
            let send = send_m3u.clone();
            dlg.open(Some(&win), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    (send)(UiCommand::ImportM3uPlaylist(
                        path.to_string_lossy().to_string(),
                    ));
                }
            });
        });

        let win_weak = window.downgrade();
        let send_exp = launch.command_sender.clone();
        let export_m3u_dialog: Rc<dyn Fn(String, String)> = Rc::new(move |pl_id, pl_name| {
            let Some(win) = win_weak.upgrade() else { return };
            let dlg = gtk4::FileDialog::builder()
                .title("Export Playlist (.m3u8)")
                .initial_name(format!("{}.m3u8", pl_name.replace('/', "_")))
                .modal(true)
                .build();
            let send = send_exp.clone();
            dlg.save(Some(&win), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res
                    && let Some(path) = file.path()
                {
                    (send)(UiCommand::ExportCustomPlaylistM3u {
                        playlist_id: pl_id.clone(),
                        dest_path: path.to_string_lossy().to_string(),
                    });
                }
            });
        });

        let win_weak = window.downgrade();
        let prompt_text_dialog: PromptDialogFn =
            Rc::new(move |title, placeholder, initial, on_confirm| {
                let Some(parent) = win_weak.upgrade() else { return };
                show_text_prompt_modal(&parent, &title, &placeholder, &initial, on_confirm);
            });

        let open_external_url: Rc<dyn Fn(String)> = Rc::new(|url| {
            if let Ok(mut child) = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        });

        let raw_send = launch.command_sender.clone();
        let store_cmd = Rc::clone(&store);
        let redraw_cmd = redraw_dispatcher.clone();
        let smart_send: Rc<dyn Fn(UiCommand)> = Rc::new(move |cmd| match cmd {
            UiCommand::SetMode(mode) => {
                store_cmd.borrow_mut().view.mode = mode;
                (redraw_cmd)(DirtyFlags::MODE | DirtyFlags::ALL);
                (raw_send)(UiCommand::SetMode(mode));
            }
            UiCommand::SetLocalFilter(filter) => {
                store_cmd.borrow_mut().view.crate_view.local_filter = filter;
                (redraw_cmd)(DirtyFlags::CONTENT);
            }
            UiCommand::SetSearchFilter(sf) => {
                store_cmd.borrow_mut().view.crate_view.search_filter = sf;
                (redraw_cmd)(DirtyFlags::CONTENT);
            }
            UiCommand::NavigateBack => {
                let target = {
                    let mut s = store_cmd.borrow_mut();
                    let cv = &mut s.view.crate_view;
                    if cv.history_index > 0 {
                        cv.history_index -= 1;
                        cv.history.get(cv.history_index).cloned()
                    } else {
                        None
                    }
                };
                if let Some(cr) = target {
                    (raw_send)(UiCommand::OpenCrate(cr));
                }
            }
            UiCommand::NavigateForward => {
                let target = {
                    let mut s = store_cmd.borrow_mut();
                    let cv = &mut s.view.crate_view;
                    if cv.history_index + 1 < cv.history.len() {
                        cv.history_index += 1;
                        cv.history.get(cv.history_index).cloned()
                    } else {
                        None
                    }
                };
                if let Some(cr) = target {
                    (raw_send)(UiCommand::OpenCrate(cr));
                }
            }
            UiCommand::SetThemePreference(pref) => {
                {
                    let mut s = store_cmd.borrow_mut();
                    s.view.config.theme_preference = pref;
                    s.recompute_tokens();
                }
                (redraw_cmd)(DirtyFlags::THEME | DirtyFlags::ALL);
                (raw_send)(UiCommand::SetThemePreference(pref));
            }
            UiCommand::SetAccentSource(src) => {
                {
                    let mut s = store_cmd.borrow_mut();
                    s.view.config.accent_source = src;
                    s.recompute_tokens();
                }
                (redraw_cmd)(DirtyFlags::THEME | DirtyFlags::ALL);
                (raw_send)(UiCommand::SetAccentSource(src));
            }
            UiCommand::SetVisualizerMode(vis) => {
                store_cmd.borrow_mut().view.config.visualizer_mode = vis;
                (redraw_cmd)(DirtyFlags::PLAYER_META | DirtyFlags::CHROME);
                (raw_send)(UiCommand::SetVisualizerMode(vis));
            }
            other => (raw_send)(other),
        });

        let bridge = UiBridge {
            store: Rc::clone(&store),
            send: smart_send,
            icons: Rc::clone(&icons),
            covers: Rc::clone(&covers),
            redraw: redraw_dispatcher,
            focus_omnibox: focus_dispatcher,
            open_folder_dialog,
            open_files_dialog,
            import_m3u_dialog,
            export_m3u_dialog,
            prompt_text_dialog,
            open_external_url,
        };

        // Construct primary views
        let top_header = TopHeaderWidget::new(&bridge);
        let left_rail = LeftRailWidget::new();
        let center_canvas = CenterCanvasWidget::new(&bridge);
        let studio_drawer = StudioDrawerWidget::new(&bridge);
        let playback_deck = PlaybackDeckWidget::new(&bridge, launch.spectrum_reader.clone());
        let stage_view = StageViewWidget::new(&bridge, launch.spectrum_reader.clone());
        let mini_view = MiniPlayerWidget::new(&bridge);
        let settings_modal = SettingsModalController::new(&window, &bridge);

        {
            let header_c = Rc::clone(&top_header);
            let b_c = bridge.clone();
            *focus_slot.borrow_mut() = Some(Rc::new(move |grab| {
                if grab {
                    header_c.omnibox_entry.grab_focus();
                    header_c.omnibox_entry.select_region(0, -1);
                } else {
                    let txt = b_c.store.borrow().omnibox_text.clone();
                    header_c.set_omnibox_text(&txt);
                }
            }));
        }

        // Assemble Mode 1 Workbench Shell
        let workbench_root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        workbench_root.set_vexpand(true);
        workbench_root.set_hexpand(true);

        let middle_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        middle_row.set_vexpand(true);
        middle_row.set_hexpand(true);
        middle_row.append(&left_rail.root);
        middle_row.append(&center_canvas.root);
        middle_row.append(&studio_drawer.root);

        workbench_root.append(&top_header.root);
        workbench_root.append(&middle_row);
        workbench_root.append(&playback_deck.root);

        let main_stack = gtk4::Stack::new();
        main_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        main_stack.set_transition_duration(120);
        main_stack.add_named(&workbench_root, Some("deck"));
        main_stack.add_named(&stage_view.root, Some("stage"));
        main_stack.add_named(&mini_view.root, Some("mini"));
        window.set_child(Some(&main_stack));

        // Central dirty-flag render loop
        let prev_mode = Rc::new(Cell::new(AppMode::Workbench));
        {
            let b_rd = bridge.clone();
            let win_c = window.clone();
            let css_c = css_provider.clone();
            let th_c = Rc::clone(&top_header);
            let lr_c = Rc::clone(&left_rail);
            let cc_c = Rc::clone(&center_canvas);
            let sd_c = Rc::clone(&studio_drawer);
            let pd_c = Rc::clone(&playback_deck);
            let sv_c = Rc::clone(&stage_view);
            let mv_c = Rc::clone(&mini_view);
            let sm_c = Rc::clone(&settings_modal);
            let stack_c = main_stack.clone();
            let pm_c = Rc::clone(&prev_mode);

            *redraw_slot.borrow_mut() = Some(Rc::new(move |flags| {
                let (mode, tokens, loaded_cnt) = {
                    let s = b_rd.store.borrow();
                    (
                        s.view.mode,
                        s.tokens.clone(),
                        s.view.crate_view.loaded_tracks.loaded_count(),
                    )
                };

                if flags.any(DirtyFlags::THEME | DirtyFlags::ALL) {
                    apply_theme_css(&css_c, &tokens);
                }

                if pm_c.get() != mode {
                    pm_c.set(mode);
                    match mode {
                        AppMode::Workbench => {
                            win_c.set_size_request(-1, -1);
                            win_c.set_resizable(true);
                            win_c.set_default_size(1280, 820);
                            stack_c.set_visible_child_name("deck");
                        }
                        AppMode::FocusStage => {
                            win_c.set_size_request(-1, -1);
                            win_c.set_resizable(true);
                            win_c.set_default_size(1280, 820);
                            stack_c.set_visible_child_name("stage");
                        }
                        AppMode::MiniPlayer => {
                            b_rd.covers.clear_thumbs(loaded_cnt);
                            win_c.set_size_request(430, 124);
                            win_c.set_default_size(430, 124);
                            win_c.set_resizable(false);
                            stack_c.set_visible_child_name("mini");
                        }
                    }
                } else {
                    match mode {
                        AppMode::Workbench => stack_c.set_visible_child_name("deck"),
                        AppMode::FocusStage => stack_c.set_visible_child_name("stage"),
                        AppMode::MiniPlayer => stack_c.set_visible_child_name("mini"),
                    }
                }

                match mode {
                    AppMode::Workbench => {
                        if flags.any(DirtyFlags::CHROME | DirtyFlags::THEME | DirtyFlags::DEVICES | DirtyFlags::ALL) {
                            th_c.sync(&b_rd, flags);
                        }
                        if flags.any(DirtyFlags::NAV | DirtyFlags::THEME | DirtyFlags::ALL) {
                            lr_c.sync(&b_rd);
                        }
                        if flags.any(DirtyFlags::CONTENT | DirtyFlags::THEME | DirtyFlags::ALL) {
                            cc_c.sync(&b_rd);
                        }
                        sd_c.sync(&b_rd, flags);
                        pd_c.sync(&b_rd, flags);
                    }
                    AppMode::FocusStage => {
                        sv_c.sync(&b_rd, flags);
                    }
                    AppMode::MiniPlayer => {
                        mv_c.sync(&b_rd, flags);
                    }
                }

                if flags.any(DirtyFlags::SETTINGS | DirtyFlags::ALL) {
                    sm_c.sync(&b_rd);
                }
            }));
        }

        // Responsive width observer (Wayland surface width changes & programmatic resize)
        if launch.self_test_dir.is_none() {
            let b_res = bridge.clone();
            window.connect_realize(move |win| {
                if let Some(surface) = gtk4::prelude::NativeExt::surface(win) {
                    let b_s = b_res.clone();
                    let w_weak = win.downgrade();
                    surface.connect_width_notify(move |surf| {
                        let Some(w) = w_weak.upgrade() else { return };
                        let new_w = w.width().max(surf.width());
                        let old_w = b_s.store.borrow().window_width;
                        if (new_w - old_w).abs() >= 12 {
                            b_s.store.borrow_mut().window_width = new_w;
                            (b_s.redraw)(DirtyFlags::NAV | DirtyFlags::CHROME | DirtyFlags::PLAYER_META);
                        }
                    });
                }
            });
        }

        // Window DropTarget (Local audio files/folders & YouTube/SoundCloud/Spotify URLs)
        install_drop_target(&window, &bridge);
        install_key_controller(&window, &bridge);

        // Initial render pass
        (bridge.redraw)(DirtyFlags::ALL);
        window.present();

        // Dispatch CLI startup file paths / URLs cleanly
        if !launch.startup_inputs.is_empty() {
            let mut local_paths = Vec::new();
            for arg in launch.startup_inputs {
                if arg.starts_with("http://")
                    || arg.starts_with("https://")
                    || arg.starts_with("spotify:")
                    || arg.starts_with("yt:")
                    || arg.starts_with("sc:")
                {
                    (bridge.send)(UiCommand::ResolveInputUrl(arg));
                } else {
                    local_paths.push(arg);
                }
            }
            if !local_paths.is_empty() {
                (bridge.send)(UiCommand::OpenLocalPaths(local_paths));
            }
        }

        // Spawn reactive backend event consumer onto GTK main loop (zero polling).
        // Skip consuming live startup events during deterministic screenshot self-test.
        if launch.self_test_dir.is_none()
            && let Some(mut rx) = launch.event_rx.lock().unwrap().take()
        {
            let b_ev = bridge.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Some(ev) = rx.recv().await {
                    let mut combined = b_ev.store.borrow_mut().apply_backend_event(ev);
                    while let Ok(more) = rx.try_recv() {
                        combined |= b_ev.store.borrow_mut().apply_backend_event(more);
                    }
                    if !combined.is_empty() {
                        (b_ev.redraw)(combined);
                    }
                }
            });
        }

        // Automated screenshot & interactive verification mode
        if let Some(out_dir) = launch.self_test_dir {
            run_self_test(SelfTestContext {
                app: app.clone(),
                window,
                bridge,
                header: top_header,
                deck: playback_deck,
                stage: stage_view,
                drawer: studio_drawer,
                out_dir,
            });
        }
    });

    app.run_with_args::<&str>(&[]);
    Ok(())
}

fn install_drop_target(window: &gtk4::ApplicationWindow, bridge: &UiBridge) {
    let dt = gtk4::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
    dt.set_types(&[gdk::FileList::static_type(), glib::types::Type::STRING]);

    let b = bridge.clone();
    dt.connect_drop(move |_, value, _, _| {
        if let Ok(file_list) = value.get::<gdk::FileList>() {
            let paths: Vec<String> = file_list
                .files()
                .into_iter()
                .filter_map(|f| f.path().map(|p| p.to_string_lossy().to_string()))
                .collect();
            if !paths.is_empty() {
                (b.send)(UiCommand::OpenLocalPaths(paths));
                return true;
            }
        }
        if let Ok(text) = value.get::<String>() {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                (b.send)(UiCommand::ResolveInputUrl(trimmed));
                return true;
            }
        }
        false
    });

    window.add_controller(dt);
}

fn show_text_prompt_modal(
    parent: &gtk4::ApplicationWindow,
    title_str: &str,
    placeholder: &str,
    initial: &str,
    on_confirm: Rc<dyn Fn(String)>,
) {
    let dlg = gtk4::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title(title_str)
        .default_width(380)
        .build();
    dlg.set_css_classes(&["pulse-root"]);

    let card = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    card.add_css_class("modal-card");
    card.set_margin_top(10);
    card.set_margin_bottom(10);
    card.set_margin_start(10);
    card.set_margin_end(10);

    let title = gtk4::Label::new(Some(title_str));
    title.set_css_classes(&["heading-md"]);
    title.set_xalign(0.0);

    let entry = gtk4::Entry::new();
    entry.add_css_class("pulse-entry");
    entry.set_placeholder_text(Some(placeholder));
    if !initial.is_empty() {
        entry.set_text(initial);
    }

    let btns = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btns.set_halign(gtk4::Align::End);
    let cancel = gtk4::Button::with_label("Cancel");
    cancel.set_css_classes(&["ctrl-btn"]);
    let ok = gtk4::Button::with_label("Create");
    ok.set_css_classes(&["accent-btn"]);

    let d_c = dlg.clone();
    cancel.connect_clicked(move |_| d_c.close());

    let submit = {
        let e = entry.clone();
        let d = dlg.clone();
        let cb = Rc::clone(&on_confirm);
        move || {
            let val = e.text().trim().to_string();
            if !val.is_empty() {
                cb(val);
            }
            d.close();
        }
    };
    let s1 = submit.clone();
    ok.connect_clicked(move |_| s1());
    entry.connect_activate(move |_| submit());

    btns.append(&cancel);
    btns.append(&ok);
    card.append(&title);
    card.append(&entry);
    card.append(&btns);
    dlg.set_child(Some(&card));
    dlg.present();
    entry.grab_focus();
}
