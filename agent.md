# Pulse (`agent.md`)

Architectural and operational guide for contributors and AI agents working in this repository.

## Project Identity

- **Package**: `pulse-spotify` v0.2.0 (Rust 2024 edition, `rust-version = "1.93"`, workspace resolver `"3"`)
- **Goal**: Native Linux desktop multi-source audio player (`gtk4 0.11` + `cairo-rs 0.22` + `gsk4 0.11` + `lofty 0.25` + `spotatui-librespot-* 0.8.7` + `mpris-server 0.11.0`) unifying **Local Music**, **Spotify**, **YouTube**, and **SoundCloud**.
- **Runtime dependencies**: `gtk4` (`4.16+`), `ffmpeg`, `ffprobe`, `libpulse`, `alsa-lib`, `IBM Plex Sans` / `IBM Plex Mono`. `libadwaita-1`, `yt-dlp`, and `mpv` are not required.

## Build, Lint, and Test Commands

Always run Cargo with `--locked` unless deliberately updating dependencies:

```bash
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p pulse-backend --locked -- --ignored e2e_live_local_youtube_soundcloud_lrclib --nocapture
cargo run --locked -- --self-test-ui .impeccable/review
cargo build --release --locked
```

Desktop deployment target on this host:
- Binary: `/home/aurora/.local/bin/pulse-spotify`
- Desktop entry: `/home/aurora/.local/share/applications/pulse-spotify.desktop` (`StartupWMClass=dev.pulse.Spotify`)

## Workspace Architecture

| Crate / File | Role |
|---|---|
| `src/main.rs` | Sets `TOKIO_WORKER_THREADS=2`, `PULSE_PROP_*`, `mallopt(M_ARENA_MAX, 2)`; parses CLI flags (`--self-test-ui`, paths/URLs) and wires `BackendHandle`, `CoverCache`, `spectrum_reader`, and `pulse_ui::app::run`. |
| `crates/pulse-model/src/lib.rs` | Shared contract types (`MediaSource`, `ThemePreference`, `AccentSource`, `VisualizerMode`, `RightDrawerTab`, `EqConfig`, `PlaybackSpeed`, `AbLoopState`, `SleepTimerState`, `LyricsState`, `CustomPlaylistSummary`, `ListeningStatsSnapshot`, `SpectrumSnapshot`, `AppViewState`, `UiCommand`, `BackendEvent`). |
| `crates/pulse-assets/src/` | Bounded cover art LRU (`MAX_THUMB_SLOTS = 32` @ `128px`, `MAX_HERO_SLOTS = 2` @ `512px`), 44 monochrome `24x24` stroke-1.75 vector SVGs (`icons.rs`), and WCAG AA adaptive palette generator (`palette.rs`) supporting album art extraction and `~/.config/gtk-4.0/gtk.css` Matugen tokens. |
| `crates/pulse-backend/src/lib.rs` | Single-threaded Tokio backend actor managing multi-source playback, search, crate loading, custom playlists, likes, session persistence, sleep timer fade, A-B loop, lyrics offset, and telemetry. |
| `crates/pulse-backend/src/dsp.rs` | `SharedDsp` shared across Spotify (`player.rs`) and `DirectAudioEngine` (`direct_player.rs`): 10-band RBJ biquad EQ, preamp, balance, mono downmix, `tanh` limiter, 48-bin spectrum analyzer. |
| `crates/pulse-backend/src/direct_player.rs` | Sample-accurate `ffmpeg` PCM (`f32le` 44.1 kHz) playback engine over PulseAudio with `atempo`/`asetrate` speed control, seek, fade multiplier, and `ScopedFfmpegChild` RAII reaping. |
| `crates/pulse-backend/src/local_lib.rs` | Incremental recursive folder indexer (`lofty` tags + cover + lyrics + `ffprobe` fallback) persisted at `$XDG_CACHE_HOME/pulse-spotify/local_index.json`. |
| `crates/pulse-backend/src/providers/` | URL/path classifier (`mod.rs`), YouTube InnerTube `WEB`/`ANDROID_VR`/`IOS`/oEmbed (`youtube.rs`), SoundCloud v2 search/resolve (`soundcloud.rs`). |
| `crates/pulse-backend/src/lyrics.rs` & `user_data.rs` | `.lrc` / `LRCLIB` synced lyrics cache, atomic `likes.json`, `playlists.json` (`.m3u8` import/export), `session_state.json`, and `stats.json` (real playback tracking only). |
| `crates/pulse-backend/src/player.rs`, `library.rs`, `connect.rs`, `auth.rs`, `queue.rs`, `mpris.rs` | Spotify `Spirc` / `librespot` playback, Spotify fallback staging, library API, Connect discovery/transfer, two-tier queue (`TwoTierQueue`), and multi-scheme MPRIS2 controls. |
| `crates/pulse-ui/src/` | Native GTK4 + Cairo UI: `lib.rs`, `icons_gtk.rs`, `theme_css.rs`, `visualizer.rs`, `state.rs`, `drawers.rs`, `dialogs.rs`, `views.rs`, `keybindings.rs`, `app.rs`, `self_test.rs`. |

## Critical Invariants & Do-Not-Regress Rules

1. **Spotify Auth-Free Startup**: Local Music, YouTube, SoundCloud, Search, Cross-Source Playlists, Favorites, EQ, Lyrics, and Insights must initialize and play cleanly with zero Spotify account configured.
2. **GTK4 Theme & CSS Priority**: Register app CSS at `gtk4::STYLE_PROVIDER_PRIORITY_USER + 10` (`810`) so host `~/.config/gtk-4.0/gtk.css` overrides cannot strip card radii or break Light mode. Never put web-only CSS properties (`cursor:`, `display:`, `gap:`, `font-variant-numeric:`) in GTK4 stylesheets; use `set_cursor_from_name(Some("pointer"))` and `font-feature-settings: "tnum" 1;`.
3. **ListView Virtualization**: `gtk4::ListView` inside `CenterCanvasWidget` must sit directly as the child of a `gtk4::ScrolledWindow` (`"tracks"` page) so GTK4 virtualizes rows in O(visible) memory.
4. **Process Hygiene**: `DirectAudioEngine` spawns `ffmpeg` only via `ScopedFfmpegChild`, killing and reaping children on stop/drop so zero zombie `ffmpeg` processes persist.
5. **Single-Track Local `Spirc` Mode**: When playing Spotify tracks locally, `Spirc::load` receives only the single active track; `TwoTierQueue` owns queue advancement cleanly across mixed Local/Spotify/YouTube/SoundCloud tracks.
6. **Deferred Seek & Focus Guard**: Timeline sliders track `scrubbing_pos_ms` locally during drag and emit `UiCommand::SeekMs` on release. `keybindings.rs` checks window focus (`Editable` / `Entry` / `Text`) before handling single-key shortcuts.

## XDG Data & Cache Locations

Never commit credentials, tokens, or caches:
- Config: `$XDG_CONFIG_HOME/pulse-spotify/config.json`
- User data: `$XDG_DATA_HOME/pulse-spotify/` (`likes.json`, `playlists.json`, `session_state.json`, `stats.json`, `recent.json`, Spotify tokens/creds)
- Caches: `$XDG_CACHE_HOME/pulse-spotify/` (`local_index.json`, `audio/`, `covers/`, `lyrics/`, `soundcloud/`)
