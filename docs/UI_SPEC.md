# Aplayer UI & Architecture Specification (v0.3.0)

Specification for **Aplayer** (`aplayer`), a native Linux GTK4 + Cairo multi-source ad-free music player and audio studio unifying Local Music, Spotify, YouTube, and SoundCloud.

---

## 1. Spatial Architecture

Aplayer presents three modes driven cleanly by a single GTK4 `ApplicationWindow` (`dev.aplayer.Player`):

1. **Mode 1 (`Deck`, `1`)**:
   - **Top Command Header (`52px`)**: Rail collapse button, Back/Forward crate history, Mode Switcher (`Deck`, `Stage`, `Mini`), Unified Omnibox (`/` filter or `Ctrl+K` global multi-source search and direct URL/path resolver), Sleep Timer menu, Playback Devices popover (`D`), Ad-Free Shield toggle (`SponsorBlock` + ad filtering), Light/Dark toggle, and Settings modal trigger (`Ctrl+,`).
   - **Left Source & Library Rail (`212px`, collapses to `56px` icon rail `< 980px`)**: Quiet sentence-case sections (`Library`: `Home`, `Local Music`, `Favorites`, `Playlists` + `+ New Playlist` / `Import`, `Recently Played`; `Streaming`: `Search All Sources`, `Spotify Liked`, `Spotify Playlists`, `Spotify Albums`, `Spotify Artists`, and optional `Connect Spotify`).
   - **Center Content Canvas**: Two-page `GtkStack` separating scrollable card grids/Home/empty states (`"cards"`) from a direct `GtkScrolledWindow -> GtkListView` (`"tracks"`) for O(visible) track virtualization. Explicit Loading, Empty (cause + recovery button), and Error (cause + Retry) states across all crates, plus YouTube row `Save MP3` download popover buttons.
   - **Right Studio Drawer (`316px`, toggleable tabs)**: `Queue` (`Queued Next` + context queue), `Lyrics` (synced `.lrc` / `LRCLIB`, clickable timestamp seek, `+/-0.5s` offset), `EQ & FX` (10-band biquad EQ, presets, preamp, balance, mono, speed/pitch), and `Insights` (real local playback tracking).
   - **Bottom Playback & Visualizer Deck (`88px`)**: Cover art (`56x56`), metadata + crisp Source Badge + `.mono` codec badge, `Save MP3` button (`Ctrl+D` when YouTube track is active), Favorite (`L`), transport controls (`Shuffle`, `Prev`, `Play/Pause`, `Next`, `Repeat`, `A-B Loop`), deferred-seek scrubber, live Cairo `AudioVisualizerWidget` with cycle/pause control, volume slider, and drawer toggles.
2. **Mode 2 (`Stage`, `2` or `F`)**: Adaptive album-tinted background wash (`<= 16%` accent alpha), `300x300` rounded cover tile, full track/codec details, wide Cairo visualizer, side-by-side live karaoke lyrics, `Save MP3` trigger for YouTube tracks, and clean transport bar.
3. **Mode 3 (`Mini-Player`, `3` or `Shift+M`)**: Floating compact `430x124` window (`set_resizable(false)`) with `76x76` cover art, title/artist/source badge, themed accent progress bar, transport controls, Favorite, and Expand button. Evicts Tier 1 thumbnail textures via `CoverCache::clear_thumbs()`.

---

## 2. Adaptive Dual-Theme Color & Typography System

Tokens are generated dynamically by `pulse_assets::AdaptiveThemeTokens::from_accent(raw_accent, is_light)` using dominant cover art (`AlbumReactive`), Matugen (`~/.config/gtk-4.0/gtk.css`), or Warm Amber (`#E59A3E`). Every foreground/surface pair and `on_accent/accent` pair is mathematically verified to satisfy WCAG 2.x AA (`>= 4.5:1` normal text, `>= 3.0:1` UI controls).

- **Font Stack**: `IBM Plex Sans` (fallback `Noto Sans`) for UI text; `IBM Plex Mono` (`font-feature-settings: "tnum" 1;`, fallback `JetBrains Mono`) for durations, bitrates, EQ readouts, and RAM telemetry.
- **Iconography**: Uniform `24x24` `1.75px` stroke monochrome SVGs from `pulse-assets::icons` rendered at 2x resolution via `PixbufLoader`. Zero emoji or Unicode symbol substitutes.
- **GTK4 CSS Priority**: Registered at `gtk4::STYLE_PROVIDER_PRIORITY_USER + 10` (`810`) above user `~/.config/gtk-4.0/gtk.css`.

---

## 3. Verification Gates

- Automated `--self-test-ui <dir>` captures PNG snapshots across Home Dark, Multi-Source Search (showing active YouTube deck & row MP3 export controls), EQ Drawer, Light Mode, Stage Lyrics, Narrow Responsive (`< 980px`), Mini-Player, and Local Empty State using `GtkWidgetPaintable` + `GskCairoRenderer`, firing genuine GTK button signals and writing `clickthrough-report.json`.
- Unit tests (`cargo test --workspace --locked`) and `contrast-check.py` verify palette WCAG ratios, cover LRU bounds, DSP limiter/EQ math, `.m3u8` round-trip, SponsorBlock hash-prefix parsing/merging, offline YouTube -> MP3 encoding & ID3v2 tagging, local audio fixture indexing, URI parsing, and queue state transitions.
