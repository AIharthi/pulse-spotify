//! Top-level Iced application message enum for Pulse UI.

use std::sync::Arc;

use bytes::Bytes;
use iced::widget::scrollable;
use iced::{Event, Point, Size, event, window};
use pulse_model::{AppMode, BackendEvent, SettingsTab, TrackSummary, UiCommand};

use crate::cover_store::CoverTier;
use crate::ui_state::ActivePane;

/// Unified message type dispatched across views, keybindings, backend events, and asset tasks.
#[derive(Debug, Clone)]
pub enum UiMsg {
    /// No-op message used to capture pointer clicks on modal/popover bodies without dismissing them.
    Noop,
    /// Pass-through command dispatched to `pulse-backend` / `pulse-assets`.
    Command(UiCommand),
    /// State snapshot or delta pushed asynchronously by `pulse-backend`.
    Backend(BackendEvent),
    /// 1 Hz UI-local tick (only active while playing; smooth scrubbing stays event-driven).
    Tick1Hz,
    /// Local scrubber preview position while dragging the timeline slider.
    SeekPreview(u32),
    /// Commit the pending scrubber position to the backend when the drag finishes.
    SeekCommit,
    /// Native Wayland/X11 window resize notification.
    WindowResized(Size),
    /// Initial or updated main window handle resolution (`window::oldest()`).
    WindowIdResolved(Option<window::Id>),
    /// Window close request intercepted from compositor.
    WindowCloseRequested(window::Id),

    /// Decoded and pre-masked cover bitmap ready to wrap in `image::Handle::from_rgba`.
    CoverReady {
        key: Arc<str>,
        tier: CoverTier,
        width: u32,
        height: u32,
        pixels: Bytes,
        accent_rgba: [u8; 4],
        evicted: Vec<(Arc<str>, CoverTier)>,
    },
    /// Cover fetch/decode failed so UI can apply temporary negative-cache backoff.
    CoverFailed {
        key: Arc<str>,
        tier: CoverTier,
    },
    /// Regenerated `128x128` radial-alpha cover tint bitmap.
    RadialTintReady {
        width: u32,
        height: u32,
        pixels: Bytes,
    },

    // Mode & layout controls
    SwitchMode(AppMode),
    ToggleQueueInFocus,
    ToggleAlwaysOnTop,
    ToggleNarrowQueue,
    SwitchFocusPane,
    DragSplitter(f32),

    // Crate & queue virtualization / interaction
    CrateScrolled(scrollable::Viewport),
    QueueScrolled(scrollable::Viewport),
    SelectCrateRow(usize),
    SelectQueueRow(usize),
    HoverTrackRowEnter(usize),
    HoverTrackRowExit(usize),
    HoverMiniPlayer(bool),
    OpenTrackContextMenu {
        pane: ActivePane,
        index: usize,
        track: TrackSummary,
        pos: Point,
    },
    CloseContextMenu,

    // Live Queue reorder & drag-and-drop
    QueueMoveUp(usize),
    QueueMoveDown(usize),
    DragQueueStart(usize),
    DragQueueDrop(usize),

    // Top bar / Local filter / Ctrl+K Omnibox
    FocusFilter,
    FilterInputChanged(String),
    ClearFilter,
    OpenOmnibox,
    CloseOmnibox,
    OmniboxInputChanged(String),
    OmniboxSubmit,
    OmniboxSelect(usize),

    // Modals / Popovers / Utilities
    RequestClearManualQueue,
    ConfirmClearManualQueue,
    CancelClearManualQueue,
    ToggleSettings,
    SetSettingsTab(SettingsTab),
    SettingsClientIdChanged(String),
    SettingsDeviceNameChanged(String),
    ToggleDevicePopover,
    ClosePopovers,
    CopyToClipboard(String),
    DismissNotice,

    // Pointer tracking & focus-guarded keyboard handling
    CursorMoved(Point),
    KeyEvent(Event, event::Status),
    InputFocusChanged(bool),
}
