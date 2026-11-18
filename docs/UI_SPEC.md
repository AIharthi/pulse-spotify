# Pulse — UI & UX Architecture Specification

> **Document Scope:** Complete visual, spatial, interaction, and memory-budget specification for **Pulse**, a pure-Rust Linux desktop Spotify player (`pulse-spotify`).  
> **Hard Constraints:** Pure Rust implementation · Strict `< 90 MB` total process RSS · Real Spotify account support (OAuth + optional Web API Client ID + Spotify Connect Zeroconf fallback) · Explicit rejection of Spotify's cluttered 3-column layout.

---

## 1. Design Philosophy & Why We Reject Spotify's Layout

Spotify's desktop interface treats music as a noisy social/algorithmic storefront:
1. **Permanent `250px` Left Navigation Rail:** Wastes ~20% of horizontal desktop width on a stagnant tree of folders and playlists.
2. **Algorithmic "Home" Wall:** Buries user music under podcasts, audiobooks, merch shelves, and looping Canvas videos.
3. **Hidden Queue:** Pushes the play queue behind a tiny bottom-right icon, hiding what plays next and making queue curation clumsy.
4. **Triple-Squeeze Tracklists:** Left rail + bottom `90px` transport bar + oversized `340px` playlist banners squeeze the actual song table into a cramped middle strip.

### Pulse's Spatial Model: **Command Bar + Split Stage & Crate Workbench**
Pulse replaces the 3-column streaming mall with an **audiophile listening workbench** built around two simultaneous priorities: **what is playing/next** and **what you are digging through**.

- **Zero Left Sidebar:** Navigation moves to a single **`48px` Top Command & Mode Bar** paired with a lightweight horizontal scope strip inside the Crate browser.
- **No Bottom Transport Bar:** Transport, artwork, precision scrubbing, and the **Live Reorderable Queue** live together in a dedicated **Live Stage** dock on the left (`~34%` width), freeing 100% of the right pane's vertical height for fast music browsing.
- **Zero Non-Music Content:** No podcasts, no audiobooks, no video clips, no algorithmic "For You" billboards.
- **Keyboard-First & Pointer-Crisp:** Every primary listening action (`Space`, `J/K`, `Enter`, `Q`, `L`, `/`, `Ctrl+K`, `[`/`]`, `1/2/3`) works without touching the mouse, while drag-and-drop from Crate to Queue works naturally with the pointer.
- **Frugal by Construction (`< 90 MB` Total RSS):** Built on **`iced 0.14.0` (`tiny-skia` CPU renderer)** + **`spotatui-librespot 0.8.7` (`alsa-backend` via PipeWire-ALSA, `rustls-webpki`)** + **`mpris-server`**, with virtualized fixed-height track tables, strictly bounded decoded cover-art LRU slots (`<= 4.0 MiB` raw / `<= 8.0 MiB` w/ `tiny-skia` pixmaps), zero heavy blur/Vulkan shaders, and damage-driven rendering.

---

## 2. Visual Identity, Color Palette & Typography Tokens

Pulse uses a **Cool Obsidian & Warm Analog Amber** palette inspired by studio monitors and hardware DACs—maximizing legibility and clearly separating itself from Spotify's `#121212` / `#1DB954` look.

### 2.1 Color Tokens

| Token | Hex / Value | Usage |
| :--- | :--- | :--- |
| `--bg-void` | `#0B0D10` | Window outer frame, backdrop behind modal sheets |
| `--bg-canvas` | `#101318` | Primary workspace canvas (Crate background, Focus backdrop) |
| `--bg-surface` | `#161A22` | Live Stage card, Queue panel, Crate index cards |
| `--bg-elevated` | `#1D222C` | Top Command Bar, popovers, settings sheet, row hover state |
| `--bg-selected` | `#242B38` | Focused keyboard cursor row, active scope pill background |
| `--border-subtle` | `#222834` | `1px` crisp structural dividers (replaces expensive shadows) |
| `--border-strong` | `#323B4D` | Input outlines, keyboard focus rings, control borders |
| `--text-primary` | `#EDF1F7` | Track titles, active headings, primary labels |
| `--text-secondary` | `#9AA5B8` | Artist names, album names, secondary metadata |
| `--text-muted` | `#636E82` | Track index numbers, column labels, disabled/unavailable tracks (`45%` alpha), keycap hints |
| `--accent-primary` | `#E59A3E` | Active play indicator, progress scrubber fill, primary CTA, keyboard cursor left bar (`2px`) |
| `--accent-hover` | `#F2AE52` | Primary CTA hover, scrubber thumb on hover |
| `--accent-dim` | `rgba(229, 154, 62, 0.14)` | Currently playing row wash, active mode pill tint, badge background |
| `--status-ok` | `#34D399` | Authenticated / PipeWire active / RSS safely under budget (`< 65 MB`) |
| `--status-warn` | `#FBBF24` | Connect-only mode / Free account notice / RSS approaching ceiling (`65–82 MB`) |
| `--status-err` | `#F87171` | Offline / auth expired / RSS critical (`> 82 MB`) |

#### Cover-Derived Dynamic Tint Rule (Stage & Focus Stage)
- Sample a downscaled `16×16` or `32×32` buffer of the active track's cover art on track transition.
- Pick the dominant non-gray pixel, clamp HSL saturation to `[20%, 45%]` and lightness to `[25%, 45%]`, and cache a single `4-byte` RGBA color per album ID.
- Because `iced_core::Gradient` only supports linear gradients, render the subtle radial wash behind the Stage sleeve (`10%–16%` max opacity over `--bg-canvas`) via a **precomputed `128 × 128` radial-alpha `image::Handle`** tinted with the album's clamped accent color (only `64 KB` raw / `128 KB` w/ `tiny-skia` pixmap), guaranteeing WCAG AA contrast (`>= 4.5:1`) with zero blur passes.

### 2.2 Typography Scale

Installed Linux desktop system fonts (explicitly selected via `Font::with_name(...)` in `iced` so `fontdb` does not fall back to `OpenSans-Regular.ttf`):
- **UI Sans:** `Font::with_name("Inter")` (system fallback `"Noto Sans"`, sans-serif)
- **Monospace (Tabular):** `Font::with_name("JetBrains Mono")` (system fallback `"Liberation Mono"`, monospace — mandatory for all durations, scrubbers, track indices, bitrates, and RSS gauges)

| Role | Family | Size / Line Height | Weight | Tracking |
| :--- | :--- | :--- | :--- | :--- |
| Focus Track Title | Sans | `26px / 32px` | `700` Bold | `-0.02em` |
| Crate Detail Header | Sans | `20px / 26px` | `700` Bold | `-0.015em` |
| Stage Deck Track Title | Sans | `16px / 22px` | `600` SemiBold | `-0.01em` |
| Crate Card / Modal Title | Sans | `15px / 20px` | `600` SemiBold | `-0.005em` |
| Track Row Title | Sans | `14px / 18px` | `500` Medium | `0` |
| Secondary / Artist / UI Label | Sans | `13px / 18px` | `400` Regular | `0` |
| Compact Subtitle / Pill | Sans | `12px / 16px` | `500` Medium | `+0.01em` |
| Section Header / Table Col | Mono / Sans | `11px / 14px` | `600` SemiBold (Upper) | `+0.06em` |
| Timestamps / Bitrate / Keycap | Mono | `11px–12px / 16px` | `500` Medium | `0` |

### 2.3 Spacing, Radii & Elevation
- **Base Grid:** `4px` (`4, 8, 12, 16, 20, 24px`).
- **Radii:** `4px` (table mini-covers, keycaps, badges), `6px` (buttons, track highlight, inputs), `8px` (queue card, popovers, crate cards), `10px–12px` (Stage & Focus cover sleeves, onboarding/settings modals).
- **Elevation:** Use `1px` borders (`--border-subtle` / `--border-strong`) and surface stepping (`#101318 -> #161A22 -> #1D222C`) instead of large Gaussian blur filters or multi-pass shadow shaders that bloat Mesa/Wayland memory.

---

## 3. Window Modes & Global Navigation Architecture

Pulse supports three seamless window modes switchable via the Top Bar mode pill or single keys (`1`, `2`, `3`):

| Mode | Shortcut | Default Geometry | Min Size | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| **1. Workbench** | `1` | `1120 × 720` px | `860 × 560` px | Daily listening & crate digging: Top Bar + Live Stage/Queue + Crate Browser |
| **2. Focus Stage** | `2` or `F` | Inherits window | `680 × 520` px | Distraction-free immersive Now Playing + large art + optional live queue |
| **3. Mini-Player** | `3` or `Shift+M` | `420 × 112` px | `360 × 112` px (`fixed H`) | Ultra-compact floating/docked desk card (`<= 2 MiB` raw / `<= 4 MiB` w/ pixmap) |

### 3.1 Top Command & Mode Bar (`48px` fixed height, Workbench & Focus)

```text
+---------------------------------------------------------------------------------------------------+
| [1 Workbench | 2 Stage | 3 Mini]  < >  |  [ 🔍 Filter crate or Ctrl+K search...   / ] | [● ALSA 320k] [⚙] |
+---------------------------------------------------------------------------------------------------+
```

1. **Left Cluster:**
   - **Mode Segmented Pill:** `Workbench` | `Stage` | `Mini` (active pill uses `--bg-selected` + `2px` bottom `--accent-primary` indicator).
   - **Crate History Arrows (`<` `>`):** Back/Forward navigation across previously opened crates/searches (`Alt+←` / `Alt+→`).
2. **Center Cluster — Unified Omnibox (`max-width: 520px`, `32px` height):**
   - Operates in two unambiguous states without leaving your current view:
     - **Instant Crate Filter (`/`):** Typing filters the currently open playlist/album/liked list in `0ms` locally; shows compact inline badge `14 / 412` + `Esc` clear button.
     - **Global Spotify Search & Command (`Ctrl+K`):** Opens a floating command/search palette directly below the omnibox:
       - *Instant (`0ms`):* Fuzzy matches user's cached Playlists, Saved Albums, Followed Artists.
       - *Debounced (`250ms` after 2+ chars):* Queries Spotify (`spotatui-librespot` `pathfinder_query` / `spotify:search:` and/or Spotify Web API catalog when configured). Pressing `Enter` opens the full **Search Results** scope in the Crate pane.
       - *Command Prefix (`>`):* `> device`, `> connect`, `> shuffle`, `> clear queue`, `> settings`, `> purge cache`.
3. **Right Cluster:**
   - **Output Device & Engine Pill (`D`):** Displays active target + stream state (e.g., `● ALSA · 320k` via PipeWire-ALSA or `⇄ Living Room`), colored with `--status-ok` / `--status-warn`. Clicking opens the **Playback Device Switcher Popover** (Section 4.1, State C).
   - **Account & Settings Trigger (`Ctrl+,`):** Compact user initial/avatar pill + gear icon; displays a subtle status ring if auth is incomplete or RSS exceeds `82 MB`.

---

## 4. Detailed Screen & State Specifications

### 4.1 Screen 1 — First-Run Onboarding, Account Connection & Settings Sheet

#### State A: Unauthenticated First-Run Screen
Centered onboarding card (`max-width: 580px`) over `--bg-void` / `--bg-canvas` so the user never lands on an empty broken dashboard.

```text
+--------------------------------------------------------------------+
|  PULSE                              [Pure Rust · < 90 MB Budget]   |
|  Connect your Spotify account to start listening                   |
+--------------------------------------------------------------------+
|  OPTION 1 · FULL ACCOUNT & LIBRARY (Recommended)                   |
|  Sign in with Spotify in your browser (`librespot-oauth`) to       |
|  enable playback, Liked Songs, Playlists, Saved Albums & Search.   |
|                                                                    |
|  [  Authorize with Spotify (OAuth Browser)  ]                      |
|                                                                    |
|  ▸ Custom Web API Client ID (Optional / Advanced)                  |
|    Client ID: [ _______________________________ ]  (PKCE, no sec)  |
|    Redirect URI: http://127.0.0.1:5588/login      [ Copy URI ]     |
|    Default: Built-in KEYMASTER_CLIENT_ID; optional custom ID      |
+--------------------------------------------------------------------+
|  OPTION 2 · ZERO-CONFIG SPOTIFY CONNECT (LAN Pairing)              |
|  ● Listening via mDNS (`with-libmdns`) as "Pulse on gentoo-box"    |
|  Open Spotify on phone/desktop -> Devices -> select Pulse.         |
|  Pairing automatically caches reusable credentials for future runs.|
+--------------------------------------------------------------------+
|  ℹ Audio playback via librespot requires Spotify Premium.          |
|  [ Audio & Cache Defaults ]                                        |
+--------------------------------------------------------------------+
```

**OAuth In-Progress & Error States:**
- **Waiting for Browser Callback:** Runs `OAuthClientBuilder` on `http://127.0.0.1:5588/login` (executed off the async UI loop via `spawn_blocking` since `TcpListener::incoming()` blocks). Button transforms into `Waiting for Spotify callback... [ Cancel ]`, showing a copyable authorization URL below in case `xdg-open` fails on minimal WMs.
- **Credential Persistence & Token Exchange:** Using `KEYMASTER_CLIENT_ID` (`SessionConfig::default().client_id`) or a user-provided custom Client ID, `session.connect(Credentials::with_access_token(tok), true)` exchanges the OAuth token for reusable credentials persisted via `Cache` to `$XDG_DATA_HOME/pulse-spotify/credentials.json` (`0600`) so subsequent launches boot directly into **Workbench** in `< 1s`.
- **Spotify Free Account Handling (`SessionError::NotPremium`, Honest Capability Banner):** Upstream `librespot-core 0.8.0` terminates the process (`std::process::exit(1)`) on non-Premium accounts; Pulse uses **`spotatui-librespot 0.8.7`**, which shuts down the session cleanly and surfaces `SessionError::NotPremium`. Pulse catches this without crashing and displays a clear `--status-warn` banner: *"Free Spotify account detected: librespot audio playback and Spotify Connect require Spotify Premium."*
- **Connect-Only / Limited Library Fallback:** If the user pairs solely via **Spotify Connect Zeroconf** (`librespot-discovery` `with-libmdns`) without completing OAuth, Pulse enters **Live Stage + Queue** mode seamlessly for Connect playback, showing a non-blocking banner in the Crate pane: *"Connected via Spotify Connect. Sign in via OAuth in Settings (`Ctrl+,`) for full library browsing."*

#### State B: Account, Connect & Engine Settings Sheet (`Ctrl+,`)
Centered modal overlay (`640 × 540` max, `Esc` to close), split into **3 concise tabs**:

1. **Account & Connect Tab:**
   - **Account Identity Card:** Display name, Product Tier badge (`Premium` / `Free`), OAuth / `login5()` token status (`Active` / `Not configured`), `[ Re-authenticate OAuth ]`, `[ Sign Out & Forget Credentials ]`.
   - **Web API Client ID Field:** Editable PKCE Client ID (defaults to `KEYMASTER_CLIENT_ID`) + `[ Copy Redirect URI ]` (`http://127.0.0.1:5588/login`) + inline 2-line setup hint for optional personal Spotify Developer App Web API calls (reusing `session.http_client()` with `rustls-webpki`).
   - **Spotify Connect (Zeroconf):** Toggle `[✓] Advertise via mDNS on local network`, editable Device Name (`Pulse on <hostname>`), live `with-libmdns` status, and list of remote Spotify Connect devices (`get_connect_state` + `send_player_command`) with `[ Transfer Playback ]`.
2. **Audio & Desktop Tab:**
   - **Audio Backend:** Pure-Rust **`ALSA (PipeWire-ALSA)`** (`alsa-backend`, lazy PCM open on play, releases SHM on stop) + PCM device selector (`default` system sink).
   - **Streaming Quality:** Segmented selector `[ 96 kbps | 160 kbps | 320 kbps ]` (Ogg Vorbis).
   - **Playback Behavior:** Volume Normalization (`Off | Track | Album`), Gapless Playback (`On | Off`), Initial Bitrate Pre-fetch (`On | Off`).
   - **Desktop Integration:** Pure-Rust **`mpris-server` (`zbus`)** media controls (`On | Off`), Hardware Media Keys (`On | Off`), Confirm before clearing manual queue (`On | Off`).
3. **Memory & Cache (`< 90 MB` Telemetry) Tab:**
   - **Live Process RSS Gauge:** Progress bar (`0 – 90 MB`) showing real `/proc/self/status` `VmRSS` & `VmHWM` (`e.g. 38.4 MB / 90.0 MB ceiling`), colored green `< 65 MB`, amber `65–82 MB`, red `> 82 MB`.
   - **Memory Breakdown Table:**
     - Decoded Cover Art RAM: `N / 32` crate slots + `M / 2` hero slots (`X.X / 4.0 MiB` raw; `<= 8.0 MiB` w/ `tiny-skia` pixmaps) + `[ Purge Cover RAM ]`
     - Loaded Track Metadata Records: `N` records in active crate window
     - Audio & Tokio Engine Overhead: single-worker Tokio + `librespot` disk-backed audio cache footprint
   - **Disk Audio Cache:** Cap selector `[ Off | 128 MB | 256 MB | 512 MB | 1 GB ]` (default `256 MB`), current disk footprint in `$XDG_CACHE_HOME/pulse-spotify/`, `[ Clear Disk Cache ]`.

#### State C: Playback Device Switcher Popover (`D` or clicking Top-Right Device Pill)
Lightweight `300px` floating popover anchored to top-right:
- **This Computer (`Pulse · ALSA / PipeWire 320k`)** — radio active dot + inline hardware sink summary.
- **Remote Spotify Connect Devices** (fetched via `get_connect_state` on popover open; e.g., `Living Room Speaker`, `Phone`, `Web Player`) — clicking transfers playback immediately (`send_player_command`) and updates the Stage header to `PLAYING ON <DEVICE>` with `--status-warn`/`--accent-primary` indicator.
- Bottom footer link: `Zeroconf mDNS Discovery: Active · Settings (Ctrl+,)`.

---

### 4.2 Screen 2 — Primary Listening & Crate Workbench (`Workbench` Mode)

```text
+---------------------------------------------------------------------------------------------------+
| [1 Workbench | 2 Stage | 3 Mini]  < >  | [ 🔍 Filter or Ctrl+K search...             / ] | [● ALSA 320k] [⚙]|
+--------------------------------+------------------------------------------------------------------+
|  LIVE STAGE (340–380px)        |  CRATE WORKBENCH (flex: 1, >= 540px)                             |
|  +--------------------------+  |  [ Liked ] [ Playlists ] [ Albums ] [ Artists ] [ Recent ]       |
|  |                          |  +------------------------------------------------------------------+
|  |       [220x220 Cover]    |  |  +------+  Late Night Crate                   Sort: Added ▾      |
|  |                          |  |  | 72x  |  Playlist · 148 tracks · 9h 14m                        |
|  +--------------------------+  |  | art  |  [ ▶ Play Crate ]  [ ⇄ Shuffle ]  [ + Queue ]          |
|  Midnight City             ♡   |  +------+--------------------------------------------------------+
|  M83 · Hurry Up, We're Dreaming|  |  #  | Art | Title / Artist              | Album          | ♡ | Time|
|  01:42 ━━━━━━●───────── 04:03  |  |------------------------------------------------------------------|
|  OGG 320 · 44.1k               |  |  01 | [ ] | Intro                       | Hurry Up...    | ♥ | 5:22|
|    [⇄]  [⏮]  [ ▶/⏸ ]  [⏭] [↻]  |  | ▶02 | [ ] | Midnight City (ACTIVE)      | Hurry Up...    | ♥ | 4:03|
|    🔊 ━━━━━━━━●─── 78%         |  |> 03 | [ ] | Reunion (CURSOR)            | Hurry Up...    |   | 3:55|
|--------------------------------|  |  04 | [ ] | Where the Boats Go          | Hurry Up...    |   | 1:44|
|  UP NEXT · 14 tracks   [Clear] |  |  05 | [ ] | Wait                        | Hurry Up...    | ♥ | 5:43|
|  -- QUEUED MANUALLY (2) ------ |  |  06 | [ ] | Raconte-Moi Une Histoire    | Hurry Up...    |   | 4:04|
|  ⋮⋮ [32] Wait — M83       5:43 ×|  |  07 | [ ] | Train to Pluton             | Hurry Up...    |   | 1:15|
|  ⋮⋮ [32] Halcyon — Bicep  4:51 ×|  |  08 | [ ] | Claudia Lewis               | Hurry Up...    |   | 4:31|
|  -- FROM LATE NIGHT CRATE ---- |  |  09 | [ ] | This Bright Flash           | Hurry Up...    |   | 2:23|
|  03 [32] Reunion — M83    3:55 |  | ... (virtualized 44px rows; zero unvirtualized allocations)     |
|  04 [32] Where the Boats  1:44 |  +------------------------------------------------------------------+
+--------------------------------+------------------------------------------------------------------+
```

#### Left Pane: The Live Stage & Play Queue (`340px–380px`, resizable splitter `[320px .. 440px]`)
1. **Now-Playing Deck (Top Section):**
   - `220 × 220` rounded cover sleeve (`10px` radius, `1px` `--border-subtle` edge, soft `10%` cover-derived radial tint behind). Clicking the sleeve transitions immediately to **Focus Stage**.
   - **Track Title** (`16px` SemiBold, up to 2 lines with ellipsis) + **Like Button** (`L`) aligned right.
   - **Artist · Album** (`13px` Regular `--text-secondary`): clicking Artist opens that Artist in the Crate pane; clicking Album opens that Album in the Crate pane.
   - **Interactive Scrubber:** `4px` track (`16px` vertical pointer hit area), `--accent-primary` elapsed fill, circular thumb on hover/drag, hover preview timestamp tooltip, flanked/paired with `JetBrains Mono` elapsed / total time (`01:42` / `04:03`) and subtle stream badge (`OGG 320` or `CONNECT`).
   - **Transport Deck:** Shuffle (`S`), Previous (`[`), Play/Pause (`Space`, `40px` primary circular button), Next (`]`), Repeat (`R`: `Off -> Context -> Track`), plus compact Volume bar (`M` mute toggle + smooth drag slider + numeric `%` tooltip).
2. **Always-Visible Live Queue (Bottom Section, `flex: 1`):**
   - Header: `UP NEXT` + count/duration (`14 tracks · 52m`) + `[ Clear ]` (clears only the manual priority queue, leaving context intact).
   - **Explicit Queue Tiers:**
     - **Queued Manually:** Priority tracks added via `Q` (Play Next), `Shift+Q` (Append), or Crate-to-Queue drag-and-drop. Marked with a `2px` `--accent-primary` left edge tick. Supports drag handle reorder, `Alt+↑`/`Alt+↓` keyboard move, and `×` / `Delete` removal.
     - **Next From `<Context Name>`:** Previews the upcoming tracks from the active Playlist/Album/Artist radio. Pressing `Enter` jumps playback directly to that context position; dragging a context track into the manual zone promotes it.
   - Virtualized `44px` rows (`32×32` thumbnail + 2-line title/artist + `JetBrains Mono` duration).

#### Right Pane: The Crate Workbench (`flex: 1`, `>= 540px`)
1. **Horizontal Scope Strip (`44px`):**
   - Single-row segmented scopes replacing Spotify's 250px left rail:
     `[ Liked Songs ]` · `[ Playlists ]` · `[ Albums ]` · `[ Artists ]` · `[ Recent ]` · (`[ Search ]` appears dynamically when a search is active).
   - Right-aligned `Sort ▾` (`Custom` / `Recently Added` / `Title` / `Artist` / `Duration`) and active filter count indicator.
2. **Scope Behaviors:**
   - **Playlists / Albums / Artists Index:** Clean `2–3` column card grid (`96×96` cover thumbnail, title, secondary count/year, `1px` `--border-subtle`) OR toggleable dense index list. Clicking any crate opens it inline with a `< Playlists` back link in the crate header (no modal popups or multi-level tree nesting).
   - **Open Crate Detail (Playlist / Album / Liked Songs):**
     - **Compact `88px` Crate Header:** `72×72` cover art, title (`20px` Bold), concise metadata (`Owner · 148 tracks · 9h 14m` or `Album · 2011 · 11 tracks`), and `[ ▶ Play ]`, `[ ⇄ Shuffle ]`, `[ + Queue ]` buttons. (Notice: `250px` shorter than Spotify's banners, leaving room for ~12–16 visible track rows even on a 720p laptop!)
     - **Artist View:** Compact `88px` Artist Header (`[ ▶ Play Top ]`, `[ Follow/Unfollow ]`) followed by **Top 5 Tracks** compact table and **Discography** grid (`Albums` / `Singles & EPs`) below.
   - **Search Results Scope:**
     - Scope filter chips: `[ All ] [ Tracks ] [ Albums ] [ Artists ] [ Playlists ]`.
     - Single clean layout: Top Match + horizontal strip of up to 4 Artists & 4 Albums, followed directly by the virtualized **Tracks** table (`Enter` plays track immediately without wiping your saved crate navigation history; `Q` inserts next in queue).
3. **Virtualized Track Table Row States (`44px` fixed row height):**
   - **Default:** Index (`JetBrains Mono`), `32×32` cover (hidden inside single-Album view for cleanliness), Title + Artist (`14px`/`12px`), Album (auto-hides below `1020px` window width), Like toggle (`L`), Duration (`JetBrains Mono` right-aligned).
   - **Keyboard Cursor / Hover:** `--bg-elevated` / `--bg-selected` background with `2px` `--accent-primary` left border; index transforms into quick `▶` play trigger, and subtle trailing `[ +Q ]` (`Play Next`) icon appears cleanly on hover.
   - **Currently Playing Row:** `--accent-dim` row background, Track Title in `--accent-primary`, index replaced by minimal pulsing/static equalizer icon.
   - **Unavailable / Unplayable Track:** `--text-muted` at `45%` opacity, `Enter`/`Q` disabled with tooltip *"Unavailable in your region or offline"*.
   - **Row Context Menu (`Right-Click` or `Shift+Enter`):** Compact menu (`Play Now`, `Play Next (Q)`, `Add to Queue (Shift+Q)`, `Toggle Like (L)`, `Go to Album`, `Go to Artist`, `Copy Spotify URI`).

#### Narrow Window Responsive Rule (`< 860px` width)
- Below `860px` width, the Workbench automatically hides the side-by-side Live Stage column and renders a **`64px` compact bottom Stage Strip** (cover + title + play/pause + scrubber + `[ Queue ]` drawer toggle) so the Crate table remains readable without horizontal squishing.

---

### 4.3 Screen 3 — Focus / Immersive Now Playing (`Stage` Mode, Key `2` or `F`)

```text
+---------------------------------------------------------------------------------------------------+
| [1 Workbench | 2 Stage | 3 Mini]        Now Playing · Late Night Crate           [● ALSA 320k] [⚙] |
+---------------------------------------------------------------------------------------------------+
|                                                                                                   |
|        +----------------------------+           +------------------------------------------+      |
|        |                            |           |  UP NEXT (Queue) · 14 tracks      [Q Hide]|      |
|        |                            |           |------------------------------------------|      |
|        |       [380x380 Cover]      |           |  QUEUED MANUALLY                         |      |
|        |      (soft amber/cover     |           |  ⋮⋮ [36] Wait — M83                 5:43 |      |
|        |         radial tint)       |           |  ⋮⋮ [36] Halcyon — Bicep            4:51 |      |
|        |                            |           |------------------------------------------|      |
|        +----------------------------+           |  NEXT FROM LATE NIGHT CRATE              |      |
|                                                 |  03 [36] Reunion — M83              3:55 |      |
|        Midnight City                     ♥      |  04 [36] Where the Boats Go         1:44 |      |
|        M83 · Hurry Up, We're Dreaming           |  05 [36] Claudia Lewis              4:31 |      |
|        2011 · Track 2 of 11 · OGG 320k          |  06 [36] This Bright Flash          2:23 |      |
|                                                 |  07 [36] Steve McQueen              3:48 |      |
|        01:42 ━━━━━━━━━●───────────── 04:03      |                                          |      |
|           [⇄]   [⏮]   [ ▶/⏸ ]   [⏭]   [↻]       |                                          |      |
|               🔊 ━━━━━━━━━━●─── 78%             |                                          |      |
+---------------------------------------------------------------------------------------------------+
```

- **Atmosphere:** Full `--bg-canvas` (`#101318`) washed behind the album sleeve with a bounded radial cover-derived accent tint (`<= 16%` opacity, zero Gaussian blur passes).
- **Left/Center Listening Hero (`~58%` width, or `100%` centered when Queue is toggled off with `Q`):**
  - Hero Cover Sleeve (`340×340` to `440×440` logical px scaled to window height, backed by a single `512×512` decoded hero bitmap).
  - Track Title (`26px` Bold), clickable Artist & Album links (`15px` Medium `--text-secondary`) — clicking either navigates directly back to Workbench opened to that Artist/Album!
  - Audio & Album Context Strip (`JetBrains Mono` `12px`): `2011 · Track 2 of 11 · OGG 320 kbps · 44.1 kHz`.
  - Wide Interactive Scrubber (`max-width: 520px`) with keyboard fine-seek (`←` / `→` `5s`, `Shift+←` / `Shift+→` `30s`).
  - Centered Transport Cluster (`48px` primary Play/Pause button) + Volume Slider + Like Button (`L`).
- **Right Queue Panel (`~42%`, toggleable with `Q`):**
  - Uses `--bg-surface` (`#161A22`) with `1px` `--border-subtle` (no frosted glass), slightly roomier `48px` queue rows, and full keyboard/drag reordering (`J/K`, `Enter`, `Alt+↑/↓`, `Delete`).
  - Below `900px` window width, the Queue collapses cleanly into a toggleable drawer (`Q`) so the hero sleeve stays centered and spacious.

---

### 4.4 Screen 4 — Compact Mini-Player (`Mini` Mode, Key `3` or `Shift+M`)

```text
+-------------------------------------------------------------------+
| +----------+  Midnight City                            ♡  [📌][⤢] |
| |  84×84   |  M83 · ALSA / PipeWire 320k                         |
| |  Cover   |  01:42 ━━━━━━━━●──────────── -02:21  [⏮] [▶/⏸] [⏭]  |
| +----------+  Next: Wait — M83 (on hover)                         |
+-------------------------------------------------------------------+
  Geometry: 420 × 112 logical px (resizable width 360–560px, fixed height 112px)
```

- **Window & Desktop Ergonomics:**
  - Compact `420 × 112` logical px card (`min-width: 360px`, `max-width: 560px`, fixed height `112px`), `1px` `--border-strong` border, subtle `3px` left accent indicator in `--accent-primary` (or cover-derived tint).
  - Top-right micro utility controls: `[ 📌 ]` (`Always on Top` via `iced::window::set_level`) and `[ ⤢ ]` (restore previous `Workbench` or `Stage` mode via `iced::window::resize`; `Esc`, `1`, `2`, `3`, or `Shift+M` also restore immediately).
- **Interactive Controls:**
  - Clicking the `84 × 84` album cover expands back to **Focus Stage** (`2`).
  - Single-line truncated Title (`14px` SemiBold) + Like button (`L`).
  - Single-line Artist + tiny status indicator (`ALSA 320k` or Connect icon).
  - Compact interactive scrubber (`4px` bar, `16px` hit zone) with tabular elapsed / remaining time (`01:42` / `-02:21`) + `[ ⏮ ] [ ▶/⏸ ] [ ⏭ ]` transport cluster.
  - Hovering the track area or `⏭` reveals a crisp 1-line `Next: <Track> — <Artist>` cue without resizing the window.
- **Memory Drop Optimization:** Entering Mini-Player immediately clears the Crate list thumbnail LRU cache (`32` thumbnails), retaining only the active + next hero/mini covers (`<= 2.0 MiB` raw / `<= 4.0 MiB` effective w/ `tiny-skia` pixmaps).

---

### 4.5 Edge, Empty & Connection Recovery States

- **Authenticated Idle (Nothing Playing Yet):** Live Stage displays a clean `--bg-surface` vinyl/sleeve placeholder, `Ready to Play` prompt, disabled scrubber (`00:00 / 00:00`), and quick-start action `[ Play Liked Songs ]` or `[ Resume Last Track ]`.
- **Zero Filter / Search Matches:** Crate list displays centered empty state `"No tracks match '<query>'"` with `[ Clear Filter (Esc) ]` (or `[ Search All Spotify (Enter) ]` when currently in local filter mode).
- **Network Drop / Session Reconnect:** Non-modal `--status-warn` slim strip (`28px`) below the Top Bar (`"Connection lost — retrying in 3s · cached tracks playable"`), automatically dismissing on recovery without clearing the user's open Crate or Queue.

---

## 5. Complete Keyboard Shortcuts & Focus Rules

### 5.1 Focus Guard Rule
- Single-key shortcuts (`Space`, `J`, `K`, `G`, `Q`, `L`, `M`, `S`, `R`, `[`, `]`, `1`, `2`, `3`, `F`, `D`, `-`, `=`, `/`) are **always suppressed** while a text input (`Filter`, `Ctrl+K Omnibox`, `Client ID` input, `Device Name` input) holds focus.
- Pressing `Esc` in any text input clears active transient text (or exits the search palette) and immediately returns keyboard focus to the active Track List or Queue.

### 5.2 Shortcut Reference

| Category | Key / Chord | Action |
| :--- | :--- | :--- |
| **Playback** | `Space` | Play / Pause (also bound to `XF86AudioPlay` via MPRIS) |
| | `[` / `]` | Previous Track / Next Track (`XF86AudioPrev` / `XF86AudioNext`) |
| | `←` / `→` | Seek `-5s` / `+5s` |
| | `Shift+←` / `Shift+→` | Seek `-30s` / `+30s` |
| | `-` / `=` (`+`) | Volume `-5%` / `+5%` |
| | `M` | Toggle Mute / Unmute |
| | `S` | Toggle Shuffle |
| | `R` | Cycle Repeat (`Off -> Context -> Track`) |
| **View Modes** | `1` | Switch to **Workbench** mode |
| | `2` or `F` | Switch to / Toggle **Focus Stage** mode |
| | `3` or `Shift+M` | Switch to / Toggle **Compact Mini-Player** mode |
| **Navigation & Search** | `/` | Focus **Local Crate Filter** (filters current view in `0ms`) |
| | `Ctrl+K` | Focus **Global Omnibox** (Spotify search, crate jump, `>` commands) |
| | `Alt+←` / `Alt+→` | Crate History Back / Forward |
| | `Tab` | Switch keyboard focus between **Crate List** and **Live Queue** |
| | `D` | Open **Playback Device Switcher** popover |
| | `Ctrl+,` | Open **Account, Connect & Settings** sheet |
| | `Esc` | Close active popover/sheet, blur Omnibox, or exit Mini-Player |
| **List & Queue Actions** | `J` / `↓` | Move list cursor down (`PgDn` / `G` or `End` jumps to bottom) |
| | `K` / `↑` | Move list cursor up (`PgUp` / `Home` jumps to top) |
| | `Enter` | Play selected track (sets Crate as context, or jumps to Queue item) |
| | `Shift+Enter` | Open track context action menu |
| | `Q` | Enqueue selected track as **Play Next** (in Focus Stage: toggle Queue panel) |
| | `Shift+Q` | Append selected track to **end of Manual Queue** |
| | `L` | Toggle **Like** on selected track (or current track in Focus Stage) |
| | `Alt+↑` / `Alt+↓` | Reorder selected track up/down inside **Live Queue** |
| | `Delete` / `Backspace` | Remove selected track from **Live Queue** |

---

## 6. Memory-Conscious Asset, Rendering & Virtualization Rules (`< 90 MB` RSS Budget)

To guarantee the entire Rust application (`iced` GUI + Tokio + `spotatui-librespot` + ALSA/PipeWire + `mpris-server` + image caches) stays **strictly below `90 MB` total process RSS** on Linux Wayland/X11, the UI layer MUST enforce these invariants grounded in our verified stack benchmarks:

### 6.1 Verified Target Memory Envelope (`~35–55 MiB` Projected on `1x` 1080p)

| Subsystem | Budget / Measured Target | Hard Rules Enforced by Architecture |
| :--- | :--- | :--- |
| **Decoded Cover Art Cache** | **`<= 4.0 MiB` raw (`<= 8.0 MiB` w/ `tiny-skia` pixmaps)** | Bounded 2-tier LRU cache (`32` thumbs + `2` heroes; see Section 6.2) because `iced_tiny_skia` keeps both `image::Handle` RGBA and premultiplied `Pixmap` |
| **Track / Crate Metadata** | **`<= 6.0 MiB`** | Paginated fetches (`50–100` items/page) + interned artist/album `Arc<str>` strings; evict closed crate pages beyond `3` recent crates |
| **GUI (`iced 0.14.0` `tiny-skia`)** | **`~19–32 MiB`** (`1x`) / **`~41–50 MiB`** (`2x` HiDPI) | `tiny-skia` CPU renderer + `crisp` + `wayland` (NO `wgpu`/Vulkan/GL shader compile spikes of `92–146 MiB`); release crate textures on Mini-Player switch |
| **Backend + Audio + Tokio** | **`~10–22 MiB`** (`9.60 MiB` idle baseline) | `rustls-webpki` + `alsa-backend` (lazy PCM open) + `TOKIO_WORKER_THREADS=1` + `MALLOC_ARENA_MAX=2` + `librespot` disk-backed `NamedTempFile` audio buffer (`256 MB` disk cache cap) |
| **Total Projected App RSS** | **`~35–55 MiB` (`1x`) / `~62–72 MiB` (`2x`)** | **`18–35+ MiB` safety margin** strictly under the **`90.0 MB` ceiling** (built with `opt-level = "z"`, `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`) |

### 6.2 Bounded Cover-Art Decoding, Pre-Masking & Caching Rules
1. **Never decode full-resolution `640×640` artwork for thumbnails.** Select the smallest Spotify CDN image variant `>= target size` (`64×64` or `300×300` URL) and decode directly via lean `image` (`jpeg`, `png`, `webp` features only) into pre-rounded RGBA8 buffers:
   - **Pre-Masked Rounded Corners (`iced_tiny_skia` rule):** `iced_tiny_skia 0.14.1` ignores `Image::border_radius` on raster images. Bake rounded-corner alpha masking (`4px` radius on `128×128` thumbnails, `10px–12px` radius on `512×512` hero covers) directly into the RGBA8 pixels once during decode (`~10` lines, zero per-frame cost).
   - **Tier 1 — List & Crate Thumbnails:** Max decode dimension **`128 × 128` RGBA8** (`65,536 bytes = 64 KB` per image).
     - **Strict LRU Cap:** Maximum **`32` decoded thumbnails** in RAM (`32 × 64 KB = 2.0 MiB` raw / `~4.0 MiB` effective with `tiny-skia` premultiplied `Pixmap`).
     - Immediately evict thumbnails outside the active Crate viewport + `12`-item scroll overscan window; auto-trimmed on `Handle` drop.
   - **Tier 2 — Stage / Focus Hero Covers:** Max decode dimension **`512 × 512` RGBA8** (`1,048,576 bytes = 1.0 MiB` per image).
     - **Strict LRU Cap:** Maximum **`2` decoded hero covers** in RAM (`Now Playing` + pre-warmed `Next Track` = `2.0 MiB` raw / `~4.0 MiB` effective with `tiny-skia` pixmap).
   - **Total Decoded Bitmap Ceiling:** **`<= 4.0 MiB` raw (`<= 8.0 MiB` effective w/ `tiny-skia` pixmaps)** hard-enforced by the asset LRU manager.
2. **Precomputed Radial Tint Buffer:** Render Focus/Stage cover-derived tints using a single `128 × 128` radial-alpha `image::Handle` (`64 KB` raw / `128 KB` w/ pixmap) tinted to the album's clamped accent color.
3. **Compressed Disk Cache Only:** Store raw compressed JPEG/WebP cover bytes on disk (`$XDG_CACHE_HOME/pulse-spotify/covers/`, capped at `32 MB` LRU on disk), never holding compressed byte arrays in RAM after decode.
4. **Async Decode Cancellation:** Rapid scrolling cancels queued HTTP image fetches and image decodes for rows that scrolled out of the viewport before decode started.
5. **Placeholder Box:** While an image is fetching, render a lightweight solid `--bg-elevated` rounded container with a subtle `1px` border—never allocate animated gradient masks per row.

### 6.3 Virtualization, Pagination & Redraw Discipline
1. **Strict Fixed-Height Virtualization (`iced::widget::scrollable`):**
   - Crate Track rows are fixed **`44px`** (`48px` in Focus Queue).
   - Virtualize via `scrollable(...).on_scroll(|viewport| ...)` using top and bottom `Space` spacers so only `ceil(viewport_height / 44) + 6` overscan rows (~`20–28` rows total) exist in the Iced widget tree, whether Liked Songs holds `50` or `15,000` tracks.
   - Crate Index grids virtualize by row slices (`ceil(viewport_height / card_row_height) + 2`).
2. **Lazy Playlist / Liked Songs Pagination:**
   - Fetch pages of `50–100` track items lazily as the scroll offset approaches within `20` rows of a fetched boundary.
   - Local Crate Filter `/` filters loaded pages immediately and triggers background fetch completion or server search when needed, displaying `Filtered N loaded tracks (fetching more...)` cleanly.
3. **Zero Idle Frame Loop:**
   - **No continuous `60 Hz` repaint loop.** When music is playing and the user is not scrolling or dragging the scrubber, emit playback progress ticks **only at `1 Hz`** (to advance the `01:42` timestamp and `1px` scrubber progress) and redraw on discrete track/queue/input events.
   - Smooth pointer scrub drags and list scrolling rely on standard event-driven Iced updates without background timers when paused/idle.
4. **Compact Vector Iconography & Explicit Fonts:** Bundle `<= 24` crisp `16–20px` monochrome SVG icons (`iced` `svg` feature / `svg::Handle::from_memory`) and select `Font::with_name("Inter")` + `Font::with_name("JetBrains Mono")` explicitly.

---

## 7. UI <-> Backend Integration Contract (Implementation Handoff)

To keep the Rust UI layer decoupled from `librespot` / Web API internals, the UI binds strictly to a compact reactive `AppViewState` and emits typed `UiCommand` actions:

### 7.1 `UiCommand` Events Emitted by UI
- **Auth & Engine:** `StartOAuthLogin { client_id: Option<String> }`, `CancelOAuthLogin`, `SaveClientId(String)`, `EnableZeroconf(bool, device_name: String)`, `SignOut`, `PurgeCoverCache`, `ClearAudioCache`.
- **Playback & Queue:** `TogglePlayPause`, `PlayTrackInContext { context_uri, track_uri, index }`, `PlayQueueIndex(usize)`, `SeekMs(u32)`, `SetVolume(u16)`, `ToggleMute`, `ToggleShuffle`, `CycleRepeat`, `EnqueueNext(TrackSummary)`, `AppendQueue(TrackSummary)`, `ReorderQueue { from, to }`, `RemoveQueueItem(usize)`, `ClearManualQueue`.
- **Devices & Library:** `RefreshDevices`, `TransferPlayback { device_id }`, `OpenCrate(CrateRef)`, `FetchCratePage { crate_ref, offset, limit }`, `SetLocalFilter(String)`, `RunGlobalSearch(String)`, `ToggleLike(TrackId)`, `SetStreamQuality(Quality)`.
- **Window Mode:** `SetMode(AppMode::Workbench | AppMode::FocusStage | AppMode::MiniPlayer)`.

### 7.2 `AppViewState` Read by UI
- `auth_state`: `Unauthenticated | WaitingOAuth { auth_url } | ConnectOnly | Authenticated { display_name, tier: Premium | Free, web_api_ready: bool }`
- `player_state`: `is_playing`, `active_track: Option<TrackDetail>`, `cover_accent_rgba: [u8; 4]`, `position_ms`, `duration_ms`, `volume_pct`, `muted`, `shuffle`, `repeat`, `audio_badge: String` (`"OGG 320"` / `"CONNECT"`)
- `devices`: `active_device_id`, `local_device_name`, `zeroconf_active: bool`, `available_devices: Vec<DeviceInfo>`
- `queue`: `manual_items: Vec<TrackSummary>`, `context_items: Vec<TrackSummary>`, `context_name: String`
- `crate_view`: `active_scope`, `breadcrumb`, `loaded_tracks: SparsePagedList<TrackSummary>`, `total_count: usize`, `sort_order`, `local_filter`
- `telemetry`: `process_rss_mb: f32`, `process_hwm_mb: f32`, `cover_cache_mb: f32`, `cover_slots_used: u8`, `disk_cache_mb: f32`
