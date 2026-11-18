//! Pulse Spotify client binary entry point (`iced 0.14` tiny-skia UI + `pulse-backend`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pulse_assets::{CoverCache, MAX_HERO_SLOTS, MAX_THUMB_SLOTS};
use pulse_backend::BackendHandle;
use pulse_model::{AppViewState, ImageRef};
use pulse_ui::bytes::Bytes;
use pulse_ui::{CommandSendFn, CoverFetchFn, MetricsObserverFn, PulseAppLaunch};

fn main() {
    // Bound Tokio worker threads and glibc malloc arenas before allocating any runtime state.
    unsafe {
        std::env::set_var("TOKIO_WORKER_THREADS", "2");
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }

    let (backend, event_rx) = match BackendHandle::spawn(Arc::new(|| {})) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("pulse-spotify startup error: {err}");
            std::process::exit(1);
        }
    };
    let backend = Arc::new(backend);

    let initial_view = AppViewState {
        config: backend.initial_config().clone(),
        ..AppViewState::default()
    };

    // Disable CoverCache's disk tier so `pulse-backend::library` exclusively owns
    // `$XDG_CACHE_HOME/pulse-spotify/covers/` without double disk caching.
    let cover_cache = Arc::new(CoverCache::with_dir(PathBuf::new(), 0));

    let backend_for_cmds = Arc::downgrade(&backend);
    let command_sender: CommandSendFn = Arc::new(move |cmd| {
        if let Some(handle) = backend_for_cmds.upgrade() {
            let _ = handle.send(cmd);
        }
    });

    let backend_for_covers = Arc::downgrade(&backend);
    let cover_fetcher: CoverFetchFn = Arc::new(move |img: ImageRef| {
        let backend_weak = backend_for_covers.clone();
        Box::pin(async move {
            let url_or_id = img
                .url
                .as_deref()
                .unwrap_or(img.id.as_ref())
                .to_owned();
            let rx = {
                let Some(handle) = backend_weak.upgrade() else {
                    return Err(String::from("backend stopped"));
                };
                handle.request_cover_bytes(url_or_id)
            };
            let raw = rx
                .await
                .map_err(|_| String::from("backend cover request dropped"))??;
            Ok(Bytes::from(raw))
        })
    });

    let stats_hook = backend.cover_stats_hook();
    let metrics_observer: MetricsObserverFn = Arc::new(move |cover_stats, loaded_tracks| {
        stats_hook.set_cover_cache(
            cover_stats.ram_bytes as u64,
            cover_stats.thumb_slots_used as u16,
            MAX_THUMB_SLOTS as u16,
            cover_stats.hero_slots_used as u8,
            MAX_HERO_SLOTS as u8,
        );
        stats_hook.set_loaded_track_records(loaded_tracks);
    });

    let launch = PulseAppLaunch {
        initial_view,
        cover_cache,
        command_sender,
        event_rx: Arc::new(Mutex::new(Some(event_rx))),
        cover_fetcher,
        metrics_observer: Some(metrics_observer),
    };

    let ui_result = pulse_ui::app::run(launch);

    if let Some(mut handle) = Arc::into_inner(backend) {
        handle.shutdown();
    }

    if let Err(err) = ui_result {
        eprintln!("pulse-spotify GUI error: {err}");
        std::process::exit(1);
    }
}
