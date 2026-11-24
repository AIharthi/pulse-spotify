//! Native GTK4 + Cairo UI for Pulse multi-source audio player.

pub use bytes;

pub mod app;
pub mod dialogs;
pub mod drawers;
pub mod icons_gtk;
pub mod keybindings;
pub mod self_test;
pub mod state;
pub mod theme_css;
pub mod views;
pub mod visualizer;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use pulse_assets::{CoverCache, CoverStats};
use pulse_model::{AppViewState, BackendEvent, ImageRef, SpectrumSnapshot, UiCommand};
use tokio::sync::mpsc;

/// Callback sending typed commands to `pulse-backend`.
pub type CommandSendFn = Arc<dyn Fn(UiCommand) + Send + Sync>;

/// Lock-free callback sampling the backend DSP spectrum/waveform analyzer.
pub type SpectrumReadFn = Arc<dyn Fn() -> SpectrumSnapshot + Send + Sync>;

/// Asynchronous cover artwork fetcher bridged through `pulse-backend`.
pub type CoverFetchFn = Arc<
    dyn Fn(ImageRef) -> Pin<Box<dyn Future<Output = Result<Bytes, String>> + Send>> + Send + Sync,
>;

/// Telemetry hook forwarding cover cache RAM stats and loaded track counts.
pub type MetricsObserverFn = Arc<dyn Fn(CoverStats, usize) + Send + Sync>;

/// Launch bundle passed from the binary entry point into the GTK4 application.
pub struct PulseAppLaunch {
    pub initial_view: AppViewState,
    pub cover_cache: Arc<CoverCache>,
    pub command_sender: CommandSendFn,
    pub spectrum_reader: SpectrumReadFn,
    pub event_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<BackendEvent>>>>,
    pub cover_fetcher: CoverFetchFn,
    pub metrics_observer: Option<MetricsObserverFn>,
    pub startup_inputs: Vec<String>,
    pub self_test_dir: Option<PathBuf>,
}
