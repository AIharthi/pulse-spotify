# Product

<!-- impeccable:product-schema 1 -->

## Platform

linux-desktop-gtk4

## Users

Linux desktop listeners who keep music across local lossless/MP3 libraries, Spotify accounts, YouTube, and SoundCloud, and want one unified ad-free native player with offline YouTube MP3 saving instead of juggling separate web tabs and clients.

## Product Purpose

Aplayer combines local folder libraries (FLAC, MP3, Ogg, Opus, M4A, WAV, ALAC), Spotify streaming and Connect, ad-free YouTube search/URLs/playlists with SponsorBlock segment skipping and MP3 export, and SoundCloud search/sets into one native GTK4 music player and audio studio. Users can search, save YouTube tracks as tagged MP3s, queue, favorite, build cross-source playlists, sculpt sound with a 10-band equalizer, view synced lyrics, and switch cleanly between a full listening deck, an immersive stage, and a compact mini-player.

## Positioning

Unlike single-service clients or ad-supported Electron wrappers, Aplayer runs locally on Linux with native GTK4 widgets, real-time Rust biquad DSP and Cairo spectrum analysis shared across every source, Ad-Free Shield filtering with SponsorBlock, offline MP3 archiving with ID3v2 cover tags, and zero account gate to start listening.

## Operating Context

- Runs on Wayland/X11 Linux desktops (Gentoo, PipeWire/PulseAudio/ALSA) with MPRIS2 D-Bus controls and desktop file associations.
- Reads local music folders incrementally, resolves embedded tags and cover art via `lofty`, decodes non-Spotify streams via `ffmpeg`, encodes YouTube MP3 exports via `ffmpeg` + `lame` (`libmp3lame` fallback), and integrates optionally with Spotify OAuth/Zeroconf Connect.
- Adapts accent colors dynamically to the playing cover art or the user's Matugen GTK-4.0 theme in both Dark and Light modes.

## Capabilities and Constraints

- System runtime includes `gtk4 4.20.4`, `ffmpeg`, `ffprobe`, `lame`, PipeWire/PulseAudio, ALSA, and `IBM Plex Sans` / `IBM Plex Mono` fonts. `libadwaita-1`, `yt-dlp`, and `mpv` are not required or installed.
- Pure-Rust backend (`pulse-backend`), GTK4 + Cairo UI (`pulse-ui`), bounded asset cache (`pulse-assets`), shared IPC contract (`pulse-model`), packaged as binary `aplayer`.
- Zero telemetry, zero fabricated stats: Listening Insights tracks only real local playback seconds and play counts recorded on the user's machine.

## Brand Commitments

- **Name**: Aplayer
- **Window Class / App ID**: `dev.aplayer.Player`
- **Visual Direction**: Dynamic Album-Reactive Deck with Dark & Light mode toggle, Matugen fallback, and clean native hierarchy.

## Product Principles

1. **Zero-Friction & Ad-Free Start**: Local files, YouTube, and SoundCloud work immediately on first launch with Ad-Free Shield and SponsorBlock enabled by default; Spotify Connect and library sync layer in cleanly when signed in.
2. **Offline MP3 Ownership**: Any YouTube track can be saved locally as an ID3v2-tagged MP3 (with bitrate selection, SponsorBlock trimming, and automatic Local Music indexing).
3. **Unified Queue & Playlists**: One two-tier queue and custom playlist engine treats Local, Spotify, YouTube, and SoundCloud tracks identically.
4. **Real Audio Craft**: Sample-accurate 10-band biquad EQ, playback speed/pitch control, A-B looping, sleep fade, and live spectrum/waveform meters work uniformly across all sources.
5. **Adaptive Readability & Process Hygiene**: Cover-reactive and Matugen palettes maintain strict WCAG AA contrast across Dark and Light themes while background decoders clean up deterministically with zero orphaned processes.
