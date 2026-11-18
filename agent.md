# Pulse Spotify (`agent.md`)

Concise architectural and operational guide for AI agents working in this repository.

## Project Identity

- **Package**: `pulse-spotify` (Rust 2024 edition, `rust-version = "1.88"`, resolver `"3"`)
- **Goal**: Lightweight, native Linux Spotify desktop client (`iced 0.14.0` with `tiny-skia` software renderer + Wayland/X11, `spotatui-librespot-* 0.8.7`, `mpris-server 0.11.0`, `librespot-discovery 0.8.0`) targeting **< 90 MB RSS**.
- **Runtime dependency note**: Pure-Rust crates; `ffmpeg` is invoked as a subprocess only during audio fallback transcoding to Ogg Vorbis when native Spotify CDN `AudioKey` playback returns unavailable (`Player::load_track`). `ffprobe` is useful for inspecting cached Ogg streams during debugging.

## Build, Lint, and Test Commands

Always run with `--locked` unless deliberately updating dependencies:

```bash
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked
```

Release binary profile (`Cargo.toml`): `opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`.
Desktop deployment target on this host (if updating the installed app):
- Binary: `/home/aurora/.local/bin/pulse-spotify`
- Desktop entry: `/home/aurora/.local/share/applications/pulse-spotify.desktop` (`StartupWMClass=dev.pulse.Spotify`)

## Workspace Architecture

| Crate / File | Role |
|---|---|
| `src/main.rs` | Sets `TOKIO_WORKER_THREADS=2` and `libc::mallopt(libc::M_ARENA_MAX, 2)` before initializing runtimes; wires `pulse-backend`, `pulse-assets::CoverCache`, and `pulse-ui::app::run`. |
| `crates/pulse-model/src/lib.rs` | Shared state types (`AppViewState`, `PlayerState`, `TrackSummary`, `TrackDetail`, `CrateRef`, `QueueState`, `TelemetrySnapshot`) and IPC enums (`UiCommand`, `BackendEvent`). |
| `crates/pulse-assets/src/` | Memory-bounded image decode/corner masking/palette extraction (`MAX_THUMB_SLOTS = 32` @ `128px`, `MAX_HERO_SLOTS = 2` @ `512px`) and SVG icon registry. Note `main.rs` passes `PathBuf::new(), 0` to `CoverCache::with_dir` so `pulse-backend` owns the single on-disk cover cache. |
| `crates/pulse-backend/src/lib.rs` | Backend actor loop, `UiCommand` dispatch, `PlayerEvent` handler, Connect/cluster sync, library/crate actions, and XDG file persistence. |
| `crates/pulse-backend/src/player.rs` | `LocalPlayback`, `Spirc` initialization, audio sink selection (`ALSA` / `PulseAudio` / `librespot`), and the `PLS4`/`PLST` local audio fallback & staging pipeline. |
| `crates/pulse-backend/src/queue.rs` | `TwoTierQueue` managing priority manual queue + context queue, shuffle/repeat progression, and history. |
| `crates/pulse-backend/src/library.rs` | Liked Songs, playlists (`rootlist`), albums, artists, recent tracks, search, and on-disk cover fetching. |
| `crates/pulse-backend/src/connect.rs` & `auth.rs` | Spotify Connect device discovery/transfer, OAuth/Zeroconf auth, credentials storage. |
| `crates/pulse-backend/src/mpris.rs` | D-Bus MPRIS2 integration (`track_id` validated on `set_position`). |
| `crates/pulse-backend/src/config.rs` & `telemetry.rs` | XDG paths (`AppPaths`) and `/proc/self/status` `VmRSS`/`VmHWM` telemetry snapshots. |
| `crates/pulse-ui/src/` | Iced `0.14.0` UI (`app.rs`, `ui_state.rs`, `message.rs`, `keybindings.rs`, `stage.rs`, `focus_stage.rs`, `mini_player.rs`, `crate_view.rs`, `omnibox.rs`, `onboarding.rs`, `settings_modal.rs`, `device_popover.rs`, `top_bar.rs`, `cover_store.rs`, `theme.rs`, `widgets.rs`). |
| `docs/UI_SPEC.md` | UI/visual specification and budget targets. |
| `assets/` | App icons (`pulse-spotify.png`, `pulse-spotify-ai.png`, `pulse-spotify.svg`). |

## Critical Invariants & Do-Not-Regress Rules

### 1. Memory & Threading Budget (`< 90 MB RSS`)
- `src/main.rs` sets `TOKIO_WORKER_THREADS=2` and `M_ARENA_MAX=2` at startup.
- `crates/pulse-ui/src/ui_state.rs` uses `SingleThreadTokio` (`worker_threads(1)`) for Iced.
- `crates/pulse-ui/Cargo.toml` deliberately uses Iced's `image-without-codecs` feature so `iced` does not pull in extra `image/default` codecs (`rayon` / `ravif`).
- Keep `pulse-assets` cover caps bounded (`32` thumbs, `2` heroes) and evict stale UI handles when switching modes (e.g., MiniPlayer).

### 2. Local Playback & Queue State Machine (`crates/pulse-backend/src/lib.rs`, `player.rs`, `queue.rs`)
- **Single-Track Local `Spirc` Mode**: When playing locally (`state.is_remote_active()` is `None`), `Spirc::load` in `load_tracks_on_active_device` / `load_context_on_active_device` MUST only receive the single target track (`vec![target.clone()]`) with `shuffle: false` and `RepeatMode::Off`, and `sync_manual_queue_to_active_device` is a no-op locally.
  - **Why**: `spotatui-librespot-playback` emits `TimeToPreloadNextTrack` ~30s before track end. If `Spirc` holds a multi-track queue and preloads an un-staged track that returns `PlayerEvent::Unavailable { track_id: next_track, play_request_id }`, naively reacting to `Unavailable` interrupts or skips the currently playing song early.
  - **How progression works**: `TwoTierQueue` in `pulse-backend` owns manual queue, context order, shuffle, and repeat. `EndOfTrack` advances `TwoTierQueue` cleanly and stages/loads the next track.
- **Request & Transition Guards**: `BackendState` tracks `pending_local_load_uri`, `loading_track_uri`, `active_play_request_id`, and `expecting_remote_transfer`:
  - `dispatch_local_load` pauses stale local audio immediately while async fallback staging runs, preventing the previous song from playing past a user track selection.
  - `handle_player_event` filters `Paused`, `Stopped`, `Playing`, `PositionCorrection`, `TimeToPreloadNextTrack`, and `EndOfTrack` by `active_play_request_id` and transition flags so stale `PlayerEvent`s from superseded tracks cannot clobber UI state or trigger spurious skips.
  - `ClusterReceived` must not overwrite active local playback/queue while local playback or local track loading is active unless `expecting_remote_transfer` is true.
- **Playlist/Crate Row Resolution**: Always preserve `hint_index` through `PlayTrackInContext` -> `PlayTrackResolved` -> `apply_play_track_resolved` so duplicate track URIs in a playlist start at the clicked row index rather than the first occurrence.

### 3. Audio Staging & Cache Format (`crates/pulse-backend/src/player.rs`)
- Cache header magic versions:
  - `b"PLS4"`: Verified full Ogg Vorbis fallback audio (`FALLBACK_HEADER_MAGIC`, permanent cache entry).
  - `b"PLST"`: Temporary fallback (`TEMP_FALLBACK_MAGIC` — truncated `< 85%` YouTube stream, `head_file`, or 30s MP3 preview) — playable once and auto-purged on startup.
  - Legacy `PLS3` entries are auto-purged on startup by `purge_stale_encrypted_audio_cache`.
- Fallback resolution order inside `ensure_track_playable_in_cache`:
  1. **Stage A** (`try_fetch_soundcloud_ogg`): SoundCloud search (`limit=25`, queries both `"{artist} {title}"` and `"{title} {artist}"`, filters unwanted variants such as `remix`, `cover`, `karaoke`, `nightcore`, `slowed`, `sped up`, `fast`, `mix`, `parody`, `bitcrushed`, `8-bit`, `8d`, `live`, etc., transcodes to Ogg via `ffmpeg`).
  2. **Stage B** (`try_fetch_audius_ogg`): Audius REST discovery (`discoveryprovider.audius.co`, follows HTTP redirects via `resolve_redirect_url` using `session.http_client().request_fut`, transcodes full stream to Ogg via `ffmpeg`).
  3. **Stage C** (`try_fetch_youtube_ios_ogg`): YouTube InnerTube `IOS` stream (compares downloaded bytes against `expected_len`; truncated `< 85%` streams are saved as temporary `PLST`).
  4. **Stage D** (`session.spclient().get_head_file`): Lowest-bitrate Spotify head file trimmed to valid Ogg pages (saved as `PLST`).
  5. **Stage E** (`session.spclient().get_audio_preview`): Spotify 30s MP3 preview strictly as last resort (saved as `PLST`).
- HTTP dependency note: Reuse `session.http_client().request_fut(...)` + `url::Url`; do **not** add `reqwest` or `http-body-util` to `Cargo.toml`.

### 4. UI Interaction Invariants (`crates/pulse-ui/src/`)
- **Deferred timeline seek**: Timeline sliders in `stage.rs`, `focus_stage.rs`, and `mini_player.rs` emit `UiMsg::SeekPreview(ms)` during drag (`scrubbing_pos_ms`) and only dispatch `UiCommand::SeekMs` on `.on_release(UiMsg::SeekCommit)`. `PositionTick` must not erase `scrubbing_pos_ms`, while track URI changes in `BackendEvent::PlaybackChanged` clear it.
- **Focus- & repeat-guarded keybindings** (`keybindings.rs`):
  - Text inputs guard single-key shortcuts (`input_focused`), while `Esc` exits focus/modals.
  - `KeyPressed { repeat: true, .. }` must be ignored for discrete playback/navigation/toggle keys (`Space`, `Enter`, `Delete`/`Backspace`, `[`, `]`, `s`, `r`, `m`, `f`, `q`, `l`, `d`, `1`, `2`, `3`, `/`), while remaining active for volume (`-`, `=`, `+`) and cursor scrolling (`j`, `k`, arrows, page keys).

## XDG Data & Cache Locations

Never commit user credentials, tokens, or caches:
- Config: `$XDG_CONFIG_HOME/pulse-spotify/config.json` (`0700` dir)
- Secrets & state: `$XDG_DATA_HOME/pulse-spotify/` (`credentials.json`, `web_api_token.json`, `auth_meta.json`, `device_id`, `recent.json`, `volume`; private `0700` dir / `0600` credential files)
- Audio & cover caches: `$XDG_CACHE_HOME/pulse-spotify/audio/` and `$XDG_CACHE_HOME/pulse-spotify/covers/`
