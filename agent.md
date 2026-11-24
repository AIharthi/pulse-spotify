# Aplayer (`agent.md`)

Architectural and operational guide for contributors and AI agents working in this repository.

## Project Identity

- **Package**: `aplayer` v0.3.0 (Rust 2024 edition, `rust-version = "1.93"`, workspace resolver `"3"`, internal crates `pulse-model`, `pulse-assets`, `pulse-backend`, `pulse-ui`)
- **Goal**: Native Linux desktop multi-source ad-free audio player (`gtk4 0.11` + `cairo-rs 0.22` + `gsk4 0.11` + `lofty 0.25` + `spotatui-librespot-* 0.8.7` + `mpris-server 0.11.0`) unifying **Local Music**, **Spotify**, **YouTube**, and **SoundCloud**, with offline ID3v2-tagged YouTube MP3 export and SponsorBlock skipping.
- **Runtime dependencies**: `gtk4` (`4.16+`), `ffmpeg`, `ffprobe`, `lame` (preferred MP3 CLI encoder when `ffmpeg` lacks `libmp3lame`), `libpulse`, `alsa-lib`, `IBM Plex Sans` / `IBM Plex Mono`. `libadwaita-1`, `yt-dlp`, and `mpv` are not required.

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
- Binary: `/home/aurora/.local/bin/aplayer` (with compatibility symlink `/home/aurora/.local/bin/pulse-spotify`)
- Desktop entry: `/home/aurora/.local/share/applications/aplayer.desktop` (`StartupWMClass=dev.aplayer.Player`)

## Workspace Architecture

| Crate / File | Role |
|---|---|
| `src/main.rs` | Sets `TOKIO_WORKER_THREADS=2`, `PULSE_PROP_*`, `mallopt(M_ARENA_MAX, 2)`; parses CLI flags (`--self-test-ui`, paths/URLs) and wires `BackendHandle`, `CoverCache`, `spectrum_reader`, and `pulse_ui::app::run`. |
| `crates/pulse-model/src/lib.rs` | Shared contract types (`MediaSource`, `Mp3Quality`, `SponsorSegment`, `ThemePreference`, `AccentSource`, `VisualizerMode`, `RightDrawerTab`, `EqConfig`, `PlaybackSpeed`, `AbLoopState`, `SleepTimerState`, `LyricsState`, `CustomPlaylistSummary`, `ListeningStatsSnapshot`, `SpectrumSnapshot`, `AppViewState`, `UiCommand`, `BackendEvent`). |
| `crates/pulse-assets/src/` | Bounded cover art LRU (`MAX_THUMB_SLOTS = 32` @ `128px`, `MAX_HERO_SLOTS = 2` @ `512px`), 46 monochrome `24x24` stroke-1.75 vector SVGs (`icons.rs`), and WCAG AA adaptive palette generator (`palette.rs`) supporting album art extraction and `~/.config/gtk-4.0/gtk.css` Matugen tokens. |
| `crates/pulse-backend/src/lib.rs` | Single-threaded Tokio backend actor managing multi-source playback, Ad-Free Shield + SponsorBlock auto-skip, YouTube -> MP3 export orchestration, search, crate loading, custom playlists, likes, session persistence, sleep timer fade, A-B loop, lyrics offset, and telemetry. |
| `crates/pulse-backend/src/mp3_export.rs` | Offline YouTube -> MP3 transcoder (`ffmpeg` WAV decode + `lame` encoding, `libmp3lame` fallback), optional SponsorBlock `aselect` trimming, and `lofty` ID3v2 tagging with cover art. |
| `crates/pulse-backend/src/dsp.rs` | `SharedDsp` shared across Spotify (`player.rs`) and `DirectAudioEngine` (`direct_player.rs`): 10-band RBJ biquad EQ, preamp, balance, mono downmix, `tanh` limiter, 48-bin spectrum analyzer. |
| `crates/pulse-backend/src/direct_player.rs` | Sample-accurate `ffmpeg` PCM (`f32le` 44.1 kHz) playback engine over PulseAudio with `atempo`/`asetrate` speed control, seek, fade multiplier, and `ScopedFfmpegChild` RAII reaping. |
| `crates/pulse-backend/src/local_lib.rs` | Incremental recursive folder indexer (`lofty` tags + cover + lyrics + `ffprobe` fallback) persisted at `$XDG_CACHE_HOME/aplayer/local_index.json`. |
| `crates/pulse-backend/src/providers/` | URL/path classifier (`mod.rs`), ad-filtered YouTube InnerTube `WEB`/`ANDROID_VR`/`IOS`/oEmbed + SponsorBlock prefix API (`youtube.rs`), ad-filtered SoundCloud v2 search/resolve (`soundcloud.rs`). |
| `crates/pulse-backend/src/lyrics.rs` & `user_data.rs` | `.lrc` / `LRCLIB` synced lyrics cache, atomic `likes.json`, `playlists.json` (`.m3u8` import/export), `session_state.json`, and `stats.json` (real playback tracking only). |
| `crates/pulse-backend/src/config.rs`, `player.rs`, `library.rs`, `connect.rs`, `auth.rs`, `queue.rs`, `mpris.rs` | XDG setup with one-time migration from `pulse-spotify` -> `aplayer`, Spotify `Spirc` / `librespot` playback + ad mute/skip guard, library API, Connect discovery/transfer, two-tier queue (`TwoTierQueue`), and multi-scheme MPRIS2 controls (`org.mpris.MediaPlayer2.aplayer`). |
| `crates/pulse-ui/src/` | Native GTK4 + Cairo UI: `lib.rs`, `icons_gtk.rs`, `theme_css.rs`, `visualizer.rs`, `state.rs`, `drawers.rs`, `dialogs.rs`, `views.rs`, `keybindings.rs`, `app.rs`, `self_test.rs`. |

## Critical Invariants & Do-Not-Regress Rules

1. **Spotify Auth-Free & Ad-Free Startup**: Local Music, YouTube, SoundCloud, Search, YouTube MP3 export, Cross-Source Playlists, Favorites, EQ, Lyrics, and Insights initialize cleanly with zero Spotify account and Ad-Free Shield + SponsorBlock enabled by default.
2. **Host MP3 Toolchain**: On hosts where `ffmpeg` disables `libmp3lame`, `mp3_export.rs` decodes audio via `ffmpeg` to a temporary stereo WAV and encodes via `lame` (`320 kbps`, `V0 VBR`, `192 kbps`, `128 kbps`), cleaning up temp files deterministically.
3. **GTK4 Theme & CSS Priority**: Register app CSS at `gtk4::STYLE_PROVIDER_PRIORITY_USER + 10` (`810`) so host `~/.config/gtk-4.0/gtk.css` overrides cannot strip card radii or break Light mode. Never put web-only CSS properties (`cursor:`, `display:`, `gap:`, `font-variant-numeric:`) in GTK4 stylesheets; use `set_cursor_from_name(Some("pointer"))` and `font-feature-settings: "tnum" 1;`.
4. **ListView Virtualization**: `gtk4::ListView` inside `CenterCanvasWidget` must sit directly as the child of a `gtk4::ScrolledWindow` (`"tracks"` page) so GTK4 virtualizes rows in O(visible) memory.
5. **Process Hygiene**: `DirectAudioEngine` spawns `ffmpeg` only via `ScopedFfmpegChild`, killing and reaping children on stop/drop so zero zombie `ffmpeg` processes persist.
6. **Single-Track Local `Spirc` Mode**: When playing Spotify tracks locally, `Spirc::load` receives only the single active track; `TwoTierQueue` owns queue advancement cleanly across mixed Local/Spotify/YouTube/SoundCloud tracks.
7. **Deferred Seek & Focus Guard**: Timeline sliders track `scrubbing_pos_ms` locally during drag and emit `UiCommand::SeekMs` on release. `keybindings.rs` checks window focus (`Editable` / `Entry` / `Text`) before handling single-key shortcuts (`Ctrl+D` triggers quick MP3 export for YouTube tracks).

## XDG Data & Cache Locations

Legacy `pulse-spotify` config and credentials migrate once automatically into `aplayer` when absent:
- Config: `$XDG_CONFIG_HOME/aplayer/config.json`
- User data: `$XDG_DATA_HOME/aplayer/` (`likes.json`, `playlists.json`, `session_state.json`, `stats.json`, `recent.json`, Spotify tokens/creds)
- Caches: `$XDG_CACHE_HOME/aplayer/` (`local_index.json`, `audio/`, `covers/`, `lyrics/`, `soundcloud/`)
