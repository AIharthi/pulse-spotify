//! Iced 0.14.0 (`tiny-skia` CPU renderer + Wayland/X11) presentation layer for Pulse.

pub use bytes;
pub use iced;
pub use pulse_assets::MAX_THUMB_SLOTS;
pub use pulse_model::{AppMode, AppViewState, UiCommand};
pub use tokio;

pub mod cover_store;
pub mod message;
pub mod theme;
pub mod ui_state;
pub mod widgets;

// Phase 2B views & runtime modules
pub mod app;
pub mod crate_view;
pub mod device_popover;
pub mod focus_stage;
pub mod keybindings;
pub mod mini_player;
pub mod omnibox;
pub mod onboarding;
pub mod settings_modal;
pub mod stage;
pub mod top_bar;

pub use app::{CommandSendFn, CoverFetchFn, MetricsObserverFn, PulseAppLaunch};
pub use cover_store::{CoverHandleCache, CoverTier};
pub use message::UiMsg;
pub use ui_state::{ActivePane, SingleThreadTokio, TrackContextMenu, UiState};
