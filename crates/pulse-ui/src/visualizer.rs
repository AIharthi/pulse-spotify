//! Real-time Cairo audio analyzer (`Spectrum`, `Waveform`, `VU Meters`, and paused `Off` mode).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4::cairo;
use gtk4::glib;
use gtk4::prelude::*;
use pulse_assets::{AdaptiveThemeTokens, DEFAULT_ACCENT_RGBA};
use pulse_model::{SpectrumSnapshot, VisualizerMode};

use crate::SpectrumReadFn;

struct VizShared {
    reader: SpectrumReadFn,
    mode: VisualizerMode,
    is_playing: bool,
    preview_locked: bool,
    tokens: AdaptiveThemeTokens,
    smoothed_bins: Vec<f32>,
    snapshot: SpectrumSnapshot,
    timer_id: Option<glib::SourceId>,
}

/// Cairo DrawingArea rendering live FFT spectrum, waveform, or stereo VU meters.
#[derive(Clone)]
pub struct AudioVisualizerWidget {
    drawing_area: gtk4::DrawingArea,
    shared: Rc<RefCell<VizShared>>,
}

impl AudioVisualizerWidget {
    pub fn new(reader: SpectrumReadFn, tokens: AdaptiveThemeTokens, width: i32, height: i32) -> Self {
        let drawing_area = gtk4::DrawingArea::new();
        drawing_area.set_content_width(width);
        drawing_area.set_content_height(height);
        drawing_area.set_hexpand(width <= 0);
        drawing_area.set_vexpand(false);

        let shared = Rc::new(RefCell::new(VizShared {
            reader,
            mode: VisualizerMode::Spectrum,
            is_playing: false,
            preview_locked: false,
            tokens,
            smoothed_bins: vec![0.0; 48],
            snapshot: SpectrumSnapshot::default(),
            timer_id: None,
        }));

        let shared_draw = Rc::clone(&shared);
        drawing_area.set_draw_func(move |_area, cr, w, h| {
            let s = shared_draw.borrow();
            draw_visualizer(cr, w as f64, h as f64, s.mode, s.is_playing, &s.smoothed_bins, &s.snapshot, &s.tokens);
        });

        Self {
            drawing_area,
            shared,
        }
    }

    pub fn widget(&self) -> &gtk4::DrawingArea {
        &self.drawing_area
    }

    pub fn set_tokens(&self, tokens: AdaptiveThemeTokens) {
        self.shared.borrow_mut().tokens = tokens;
        self.drawing_area.queue_draw();
    }

    pub fn update_state(&self, mode: VisualizerMode, is_playing: bool) {
        let should_run = is_playing && mode != VisualizerMode::Off;
        let need_spawn = {
            let mut s = self.shared.borrow_mut();
            s.mode = mode;
            s.is_playing = is_playing;
            if !should_run {
                if let Some(id) = s.timer_id.take() {
                    id.remove();
                }
                false
            } else {
                s.timer_id.is_none()
            }
        };

        if need_spawn {
            let shared_tick = Rc::clone(&self.shared);
            let area = self.drawing_area.clone();
            let id = glib::timeout_add_local(Duration::from_millis(33), move || {
                let keep_going = {
                    let mut s = shared_tick.borrow_mut();
                    if !s.is_playing || s.mode == VisualizerMode::Off || s.preview_locked {
                        s.timer_id = None;
                        false
                    } else {
                        let snap = (s.reader)();
                        if s.smoothed_bins.len() != snap.bins.len() {
                            s.smoothed_bins.resize(snap.bins.len(), 0.0);
                        }
                        for (i, &val) in snap.bins.iter().enumerate() {
                            let prev = s.smoothed_bins[i];
                            let next = if val > prev {
                                prev * 0.35 + val * 0.65
                            } else {
                                prev * 0.78 + val * 0.22
                            };
                            s.smoothed_bins[i] = next.clamp(0.0, 1.0);
                        }
                        s.snapshot = snap;
                        true
                    }
                };
                area.queue_draw();
                if keep_going {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
            self.shared.borrow_mut().timer_id = Some(id);
        } else {
            self.drawing_area.queue_draw();
        }
    }

    /// Feeds synthetic bars for deterministic offline UI verification screenshots.
    pub fn seed_preview_snapshot(&self, snap: SpectrumSnapshot) {
        {
            let mut s = self.shared.borrow_mut();
            s.preview_locked = true;
            if let Some(id) = s.timer_id.take() {
                id.remove();
            }
            s.smoothed_bins = snap.bins.clone();
            s.snapshot = snap;
        }
        self.drawing_area.queue_draw();
    }
}

fn set_rgb(cr: &cairo::Context, rgb: [u8; 3], alpha: f64) {
    cr.set_source_rgba(
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
        alpha,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_visualizer(
    cr: &cairo::Context,
    w: f64,
    h: f64,
    mode: VisualizerMode,
    is_playing: bool,
    smoothed_bins: &[f32],
    snapshot: &SpectrumSnapshot,
    t: &AdaptiveThemeTokens,
) {
    if w <= 4.0 || h <= 4.0 {
        return;
    }

    let pad_x = 2.0;
    let pad_y = 3.0;
    let inner_w = (w - pad_x * 2.0).max(4.0);
    let inner_h = (h - pad_y * 2.0).max(4.0);

    // Subtle baseline guide
    set_rgb(cr, t.border_subtle, 0.7);
    cr.set_line_width(1.0);
    cr.move_to(pad_x, h - pad_y);
    cr.line_to(w - pad_x, h - pad_y);
    let _ = cr.stroke();

    match mode {
        VisualizerMode::Off => {
            set_rgb(cr, t.fg_muted, 0.35);
            cr.set_line_width(1.5);
            let mid_y = h * 0.5;
            cr.move_to(pad_x, mid_y);
            cr.line_to(w - pad_x, mid_y);
            let _ = cr.stroke();
        }
        VisualizerMode::Spectrum => {
            let count = smoothed_bins.len().min((inner_w / 3.5).floor() as usize).max(16);
            let step = inner_w / count as f64;
            let bar_w = (step * 0.64).clamp(2.0, 12.0);
            for i in 0..count {
                let bin_idx = (i * smoothed_bins.len()) / count;
                let mag = if is_playing || smoothed_bins[bin_idx] > 0.01 {
                    f64::from(smoothed_bins[bin_idx])
                } else {
                    0.06
                };
                let bar_h = (mag * (inner_h - 2.0)).max(2.5);
                let x = pad_x + i as f64 * step + (step - bar_w) * 0.5;
                let y = h - pad_y - bar_h;
                let alpha = 0.45 + 0.55 * mag;
                set_rgb(cr, t.accent, alpha);
                draw_rounded_rect(cr, x, y, bar_w, bar_h, (bar_w * 0.4).min(3.0));
                let _ = cr.fill();
            }
        }
        VisualizerMode::Waveform => {
            let pts = &snapshot.waveform;
            if pts.is_empty() {
                return;
            }
            let mid_y = h * 0.5;
            let amp = inner_h * 0.44;
            set_rgb(cr, t.accent, 0.92);
            cr.set_line_width(2.0);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.set_line_join(cairo::LineJoin::Round);
            for (i, &sample) in pts.iter().enumerate() {
                let x = pad_x + (i as f64 / (pts.len() - 1).max(1) as f64) * inner_w;
                let y = mid_y - f64::from(sample.clamp(-1.0, 1.0)) * amp;
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();
        }
        VisualizerMode::VuMeters => {
            let bar_h = ((inner_h - 5.0) * 0.45).clamp(4.0, 10.0);
            let y_l = pad_y + 1.0;
            let y_r = y_l + bar_h + 4.0;
            for (y, peak) in [(y_l, snapshot.peak_l), (y_r, snapshot.peak_r)] {
                set_rgb(cr, t.bg_highlight, 0.95);
                draw_rounded_rect(cr, pad_x, y, inner_w, bar_h, 3.0);
                let _ = cr.fill();

                let fill_w = (f64::from(peak.clamp(0.0, 1.0)) * inner_w).max(2.0);
                let color = if peak > 0.88 {
                    t.status_err
                } else if peak > 0.68 {
                    t.status_warn
                } else {
                    t.status_ok
                };
                set_rgb(cr, color, 0.95);
                draw_rounded_rect(cr, pad_x, y, fill_w, bar_h, 3.0);
                let _ = cr.fill();
            }
        }
    }
}

fn draw_rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let rad = r.min(w * 0.5).min(h * 0.5);
    cr.new_sub_path();
    cr.arc(x + w - rad, y + rad, rad, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - rad, y + h - rad, rad, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + rad, y + h - rad, rad, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + rad, y + rad, rad, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

/// Ensures `AdaptiveThemeTokens` default preview compiles cleanly if needed.
pub fn default_viz_tokens() -> AdaptiveThemeTokens {
    AdaptiveThemeTokens::from_accent(DEFAULT_ACCENT_RGBA, false)
}
