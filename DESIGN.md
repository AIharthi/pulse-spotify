# Aplayer Design System

## Design Read
Reading this as: native Linux multi-source music player and audio studio for desktop listeners, in a Dynamic Album-Reactive Deck style, dial ENERGY 2 / RHYTHM 2 / MOTION 2.

## Spatial Architecture
1. **Top Command Header (`52px`)**: Navigation rail collapse toggle, Back/Forward crate history, Mode Switcher (`Deck`, `Stage`, `Mini`), Unified Omnibox (`/` filter or `Ctrl+K` multi-source search and direct URL resolver), Sleep Timer status button, Playback Device Switcher (`D`), Ad-Free Shield toggle (`SponsorBlock` + ad filtering), Dark/Light theme toggle, and Settings trigger (`Ctrl+,`).
2. **Left Source & Library Rail (`212px`, collapses to `56px` icon rail `< 980px` and compact drawer `< 780px`)**: Grouped with quiet sentence-case labels (`Library`, `Streaming`). Every destination maps directly to live content; active destination uses tonal fill + full subtle border + bold primary label (zero one-sided colored stripes).
3. **Center Content Canvas**: Virtualized GTK4 `ListView` (`SignalListItemFactory` + `gio::ListStore`) and adaptive card grids with explicit Loading, Empty (naming cause + action), and Error (naming error + retry) states.
4. **Right Studio Drawer (`320px`, toggleable tabs)**: `Queue`, `Lyrics` (clickable synced LRC timestamps + offset adjust), `EQ & FX` (10-band biquad EQ + speed/pitch), and `Insights` (real local listening breakdown).
5. **Bottom Playback & Visualizer Deck (`88px`)**: Cover art (`60x60`) + metadata + source/codec chip + Save MP3 button (`Ctrl+D` on YouTube tracks) + Favorite (`L`); Transport cluster (`Shuffle`, `Prev`, `Play/Pause`, `Next`, `Repeat`, `A-B Loop`) + precision deferred-seek scrubber (`IBM Plex Mono`); Live Cairo Visualizer (`Spectrum` / `Waveform` / `VU Meters` / `Off` with pause/off control) + Volume & Drawer buttons.
6. **Immersive Stage (`2` / `F`) & Mini-Player (`3` / `Shift+M`)**: Full album showcase with side-by-side live karaoke lyrics, full-width spectrum visualizer, and `430x124` compact desk controller.

## Adaptive Dual-Theme Color System (WCAG AA Enforced)
- **Accent Sources**: `AlbumReactive` (dominant hue extracted from active track artwork, clamped for contrast), `MatugenSystem` (`@define-color accent_color` parsed from `~/.config/gtk-4.0/gtk.css`), or `WarmAmber` (`#E59A3E` default).
- **Dark Mode**: Tinted obsidian base (`#0E1015`), elevated panels (`#151922`, `#1D222E`), crisp ink (`#EEF2F8`, `#A6B1C4`, `#8A96AB`), accent contrast `>= 3.3:1` against surfaces and `>= 4.6:1` with `on_accent`.
- **Light Mode**: Tinted daylight paper base (`#F4F6FA`), crisp cards (`#FFFFFF`, `#EAEFF6`), deep ink (`#11151E`, `#3B4457`, `#4F5A70`), accent contrast `>= 4.55:1` against surfaces and `>= 4.6:1` with `on_accent`.

## Typography & Iconography
- **Sans (`IBM Plex Sans`, fallback `Noto Sans`)**: Track titles, navigation, headings, buttons, and lyrics.
- **Mono (`IBM Plex Mono`, fallback `JetBrains Mono`, tabular numerals)**: Timestamps, track durations, EQ dB/Hz readouts, audio bitrate/sample rate, and RSS telemetry.
- **Vector Icon System**: Uniform `24x24` `1.75px` stroke monochrome SVGs from `pulse-assets::icons` tinted dynamically to foreground/accent tokens (zero emoji or unicode glyph substitutes).

## Elevation & Radius Scale
- **Modals**: `16px`
- **Album Art Hero / Primary Surface Cards**: `12px`
- **Buttons / Track Rows / Inputs / Nav Items**: `8px`
- **Source & Status Chips**: `999px` (strictly for compact chips and toggles)
- **Borders**: Maximum 2 bordered containment levels using crisp `1px` tonal borders; interactive focus rings always visible (`2px solid accent`, `outline-offset: 2px`).

## Motion Discipline
- Fast `150ms` cubic transitions (`cubic-bezier(0.22, 1, 0.36, 1)`) for drawer reveals, hover surfaces, and active lyric focus, honoring `gtk-enable-animations` (`prefers-reduced-motion`). Visualizer redraws run at `30 Hz` only while playing and unpaused (`VisualizerMode != Off`).
