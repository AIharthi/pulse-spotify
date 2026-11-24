# Aplayer

Aplayer is a native Linux desktop music player and audio studio built in Rust (`GTK4` + `Cairo`) that brings **Local Music**, **Spotify**, **YouTube**, and **SoundCloud** together in one unified workspace without ads.

## Sources, Ad-Free Playback & MP3 Export

- **Local Music**: Recursive incremental indexing (`lofty` tags + embedded artwork + `.lrc` / embedded lyrics + `ffprobe` fallback) across `.flac`, `.mp3`, `.ogg`, `.opus`, `.m4a`, `.aac`, `.wav`, `.alac`, `.wv`, `.ape`, and `.aiff`. Supports drag-and-drop folders/files and CLI paths.
- **Ad-Free Shield & SponsorBlock**: Filters YouTube ad renderers and SoundCloud promotional entries, automatically skips SponsorBlock `sponsor`, `selfpromo`, `interaction`, and `music_offtopic` segments on YouTube playback (via privacy-preserving 4-char SHA-256 prefix lookup), and mutes/skips Spotify ad intervals when active.
- **YouTube to MP3 Export (`Ctrl+D` or Save MP3 popover)**: Export any YouTube track to ID3v2-tagged `.mp3` with embedded cover art, selectable encoding profile (`320 kbps`, `V0 VBR`, `192 kbps`, `128 kbps`), optional SponsorBlock segment trimming (`ffmpeg` `aselect`), Quick Save to your configured downloads folder or `Save MP3 As...`, and automatic indexing into Local Music.
- **YouTube**: Account-free InnerTube search, `VL` playlist expansion, video URL resolution, and full-length ranged chunk staging (`ANDROID_VR` + `IOS` fallback).
- **SoundCloud**: Account-free track/set search, permalink resolution (`/resolve`), and full stream playback.
- **Spotify**: Optional Spotify OAuth + Web API sync + Zeroconf Spotify Connect discovery and playback transfer (`spotatui-librespot-*` via PulseAudio / ALSA).

## Features & Audio Craft

- **Shared Real-Time DSP (`pulse-backend::dsp`)**: 10-band RBJ peaking biquad equalizer (`31 Hz` to `16 kHz`) with 9 presets, preamp (`+/-12 dB`), stereo balance, mono downmix, soft-knee `tanh` limiter, and 48-bin Hann-windowed spectrum / waveform / stereo VU analyzer across all sources.
- **Direct Audio Engine**: Sample-accurate `ffmpeg`-decoded `f32le` stereo playback over PulseAudio with fast seek, `atempo` pitch-preserved speed (`0.5x` to `2.0x`), `asetrate` tape varispeed mode, sleep-timer fade, and deterministic child process cleanup.
- **Unified Queue & Cross-Source Playlists**: Two-tier queue (`Queued Next` + context queue) with promote/reorder/deduplicate/save-as-playlist, unified cross-source favorites, and `.m3u` / `.m3u8` import and export.
- **Synced Lyrics**: Local `.lrc` / embedded tags + live `LRCLIB` synced lyrics with clickable timestamps to seek and `+/-0.5s` sync offset adjustments.
- **Listening Controls**: A-B section looping, Sleep Timer (`15m`, `30m`, `45m`, `60m`, `End of Track`) with 15-second soft fade, session restore on startup, real local listening breakdown in Insights, and MPRIS2 desktop media controls.
- **Adaptive Dark & Light Themes**: Dynamic WCAG AA palettes anchored on active album cover art, host Matugen GTK-4.0 (`~/.config/gtk-4.0/gtk.css`), or Warm Amber (`#E59A3E`).
- **Three Modes**: Full Listening Deck (`1`), Immersive Stage (`2` / `F`), and Floating Mini-Player (`3` / `Shift+M`).

## Keyboard Shortcuts

- `Space`: Play / Pause
- `/`: Filter active list; `Ctrl+K`: Multi-source Search or resolve URL / file path
- `Ctrl+D`: Save active or selected YouTube track as MP3
- `[` / `]`: Previous / Next track
- `Left` / `Right` (`Shift`): Seek `-/+5s` (`-/+15s`)
- `-` / `=`: Decrease / increase volume
- `M`: Mute toggle
- `S` / `R`: Shuffle / Repeat cycle
- `B`: Set A-B loop points (`Off -> A -> A-B -> Off`)
- `L`: Favorite active track
- `Q`: Toggle Queue studio drawer
- `1` / `2` / `F` / `3` / `Shift+M`: Switch between Deck, Stage, and Mini-Player modes
- `J` / `K` / `Enter`: Move track selection and play
- `Ctrl+,`: Open Settings (`Esc` closes dialogs/search)

## Build, Verify & Install

Prerequisites on Linux: Rust (`1.93+`), `gtk4`, `ffmpeg`, `ffprobe`, `lame` (optional for fast CLI MP3 encoding, otherwise `ffmpeg` `libmp3lame`), `libpulse`, and `alsa-lib`.

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked
```

Install the release binary locally:

```bash
install -Dm755 target/release/aplayer ~/.local/bin/aplayer
```
