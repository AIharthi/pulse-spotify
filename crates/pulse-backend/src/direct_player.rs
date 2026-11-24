//! Sample-accurate `ffmpeg`-backed PCM audio engine for Local, YouTube, and SoundCloud tracks.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use libpulse_binding as pulse;
use libpulse_simple_binding::Simple;
use pulse_model::PlaybackSpeed;
use tokio::sync::mpsc;

use crate::dsp::SharedDsp;

const SAMPLE_RATE: f64 = 44_100.0;
const CHANNELS: usize = 2;
const CHUNK_FRAMES: usize = 2048;
const CHUNK_BYTES: usize = CHUNK_FRAMES * CHANNELS * 4;

#[derive(Debug, Clone)]
pub enum DirectCmd {
    Load {
        seq: u64,
        uri: String,
        input: String,
        start_ms: u32,
        speed: PlaybackSpeed,
        start_playing: bool,
    },
    Play,
    Pause,
    SeekMs(u32),
    SetVolume(u16),
    SetAudioDevice(Option<String>),
    SetSpeed(PlaybackSpeed),
    SetFadeMultiplier(f32),
    Stop,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum DirectEvent {
    Playing { seq: u64, uri: String, position_ms: u32 },
    Paused { seq: u64, uri: String, position_ms: u32 },
    PositionChanged { seq: u64, position_ms: u32 },
    EndOfTrack { seq: u64, uri: String },
    Error { seq: u64, message: String },
}

struct ScopedFfmpegChild {
    child: Child,
}

impl ScopedFfmpegChild {
    fn spawn(input: &str, start_ms: u32, speed: PlaybackSpeed) -> Result<(Self, std::process::ChildStdout), String> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if start_ms > 0 {
            let secs = format!("{:.3}", f64::from(start_ms) / 1000.0);
            cmd.args(["-ss", &secs]);
        }
        cmd.args(["-i", input, "-vn", "-sn", "-dn"]);

        let rate = speed.rate.clamp(0.5, 2.0);
        if (rate - 1.0).abs() > 0.01 {
            let filter = if speed.preserve_pitch {
                format!("atempo={rate:.3}")
            } else {
                let target_hz = (SAMPLE_RATE * f64::from(rate)).round();
                format!("asetrate={target_hz:.0},aresample=44100")
            };
            cmd.args(["-af", &filter]);
        }

        cmd.args(["-f", "f32le", "-ac", "2", "-ar", "44100", "pipe:1"]);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to launch ffmpeg audio decoder: {e}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg stdout unavailable".to_string())?;
        Ok((Self { child }, stdout))
    }

    fn try_wait_finished(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    fn drain_stderr(&mut self) -> String {
        if let Some(mut err) = self.child.stderr.take() {
            let mut msg = String::new();
            let _ = err.read_to_string(&mut msg);
            return msg.trim().to_string();
        }
        String::new()
    }
}

impl Drop for ScopedFfmpegChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct DirectPulseSink {
    sink: Option<Simple>,
    device: Option<String>,
}

impl DirectPulseSink {
    fn new(device: Option<String>) -> Self {
        Self { sink: None, device }
    }

    fn ensure_open(&mut self) -> Result<&Simple, String> {
        if self.sink.is_none() {
            let spec = pulse::sample::Spec {
                format: pulse::sample::Format::S16NE,
                channels: 2,
                rate: 44_100,
            };
            let attr = pulse::def::BufferAttr {
                maxlength: 35_280,
                tlength: 14_112,
                prebuf: 3_528,
                minreq: 3_528,
                fragsize: u32::MAX,
            };
            let simple = Simple::new(
                None,
                "Pulse",
                pulse::stream::Direction::Playback,
                self.device.as_deref(),
                "Pulse Multi-Source Audio",
                &spec,
                None,
                Some(&attr),
            )
            .map_err(|e| format!("PulseAudio sink open failed: {e}"))?;
            self.sink = Some(simple);
        }
        self.sink
            .as_ref()
            .ok_or_else(|| "PulseAudio sink missing".to_string())
    }

    fn flush_and_close(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.flush();
        }
    }

    fn drain_and_close(&mut self) {
        if let Some(sink) = self.sink.take() {
            let _ = sink.drain();
        }
    }

    fn write_s16(&mut self, bytes: &[u8]) -> Result<(), String> {
        let sink = self.ensure_open()?;
        sink.write(bytes)
            .map_err(|e| format!("PulseAudio write error: {e}"))
    }
}

pub struct DirectAudioEngine {
    cmd_tx: std::sync::mpsc::Sender<DirectCmd>,
    volume: Arc<AtomicU32>,
    fade_milli: Arc<AtomicU32>,
    shutdown_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DirectAudioEngine {
    pub fn spawn(
        dsp: Arc<SharedDsp>,
        event_tx: mpsc::UnboundedSender<DirectEvent>,
        initial_volume: u16,
        initial_device: Option<String>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        let volume = Arc::new(AtomicU32::new(u32::from(initial_volume)));
        let fade_milli = Arc::new(AtomicU32::new(1000));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let vol_clone = Arc::clone(&volume);
        let fade_clone = Arc::clone(&fade_milli);
        let flag_clone = Arc::clone(&shutdown_flag);

        let worker = thread::Builder::new()
            .name("pulse-direct-audio".to_string())
            .spawn(move || {
                audio_worker_loop(
                    cmd_rx,
                    dsp,
                    event_tx,
                    vol_clone,
                    fade_clone,
                    flag_clone,
                    initial_device,
                );
            })
            .ok();

        Self {
            cmd_tx,
            volume,
            fade_milli,
            shutdown_flag,
            worker,
        }
    }

    pub fn load(&self, seq: u64, uri: String, input: String, start_ms: u32, speed: PlaybackSpeed, start_playing: bool) {
        let _ = self.cmd_tx.send(DirectCmd::Load {
            seq,
            uri,
            input,
            start_ms,
            speed,
            start_playing,
        });
    }

    pub fn play(&self) {
        let _ = self.cmd_tx.send(DirectCmd::Play);
    }

    pub fn pause(&self) {
        let _ = self.cmd_tx.send(DirectCmd::Pause);
    }

    pub fn seek_ms(&self, pos_ms: u32) {
        let _ = self.cmd_tx.send(DirectCmd::SeekMs(pos_ms));
    }

    pub fn set_volume(&self, vol: u16) {
        self.volume.store(u32::from(vol), Ordering::Release);
        let _ = self.cmd_tx.send(DirectCmd::SetVolume(vol));
    }

    pub fn set_speed(&self, speed: PlaybackSpeed) {
        let _ = self.cmd_tx.send(DirectCmd::SetSpeed(speed));
    }

    pub fn set_audio_device(&self, dev: Option<String>) {
        let _ = self.cmd_tx.send(DirectCmd::SetAudioDevice(dev));
    }

    pub fn set_fade_multiplier(&self, factor: f32) {
        let milli = (factor.clamp(0.0, 1.0) * 1000.0).round() as u32;
        self.fade_milli.store(milli, Ordering::Release);
        let _ = self.cmd_tx.send(DirectCmd::SetFadeMultiplier(factor));
    }

    pub fn stop(&self) {
        let _ = self.cmd_tx.send(DirectCmd::Stop);
    }
}

impl Drop for DirectAudioEngine {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        let _ = self.cmd_tx.send(DirectCmd::Shutdown);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

struct ActiveDecoder {
    seq: u64,
    uri: String,
    input: String,
    seek_base_ms: u32,
    frames_written: u64,
    speed: PlaybackSpeed,
    is_playing: bool,
    child: Option<ScopedFfmpegChild>,
    stdout: Option<std::process::ChildStdout>,
}

impl ActiveDecoder {
    fn current_pos_ms(&self) -> u32 {
        let elapsed_ms =
            ((self.frames_written as f64 * 1000.0 * f64::from(self.speed.rate)) / SAMPLE_RATE)
                .round() as u64;
        self.seek_base_ms.saturating_add(elapsed_ms.min(u32::MAX as u64) as u32)
    }

    fn respawn_at(&mut self, pos_ms: u32) -> Result<(), String> {
        self.child = None;
        self.stdout = None;
        self.seek_base_ms = pos_ms;
        self.frames_written = 0;
        let (child, stdout) = ScopedFfmpegChild::spawn(&self.input, pos_ms, self.speed)?;
        self.child = Some(child);
        self.stdout = Some(stdout);
        Ok(())
    }
}

fn audio_worker_loop(
    cmd_rx: std::sync::mpsc::Receiver<DirectCmd>,
    dsp: Arc<SharedDsp>,
    event_tx: mpsc::UnboundedSender<DirectEvent>,
    volume: Arc<AtomicU32>,
    fade_milli: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
    initial_device: Option<String>,
) {
    let mut sink = DirectPulseSink::new(initial_device);
    let mut active: Option<ActiveDecoder> = None;
    let mut raw_buf = vec![0u8; CHUNK_BYTES];
    let mut f32_buf = Vec::<f32>::with_capacity(CHUNK_FRAMES * CHANNELS);
    let mut pcm_bytes = Vec::<u8>::with_capacity(CHUNK_FRAMES * CHANNELS * 2);
    let mut last_pos_emit = Instant::now();

    loop {
        if shutdown.load(Ordering::Acquire) {
            sink.flush_and_close();
            break;
        }

        // Drain all queued commands cleanly so rapid seek/skip collapses to latest intent.
        let mut should_exit = false;
        loop {
            let msg = if active.as_ref().is_some_and(|a| a.is_playing) {
                cmd_rx.try_recv().ok()
            } else {
                match cmd_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(c) => Some(c),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        should_exit = true;
                        None
                    }
                }
            };
            let Some(cmd) = msg else { break };
            match cmd {
                DirectCmd::Shutdown => {
                    should_exit = true;
                    break;
                }
                DirectCmd::Stop => {
                    active = None;
                    sink.flush_and_close();
                    dsp.clear_analyzer();
                }
                DirectCmd::SetVolume(_) | DirectCmd::SetFadeMultiplier(_) => {}
                DirectCmd::SetAudioDevice(dev) => {
                    sink.flush_and_close();
                    sink.device = dev;
                }
                DirectCmd::Play => {
                    if let Some(ref mut dec) = active
                        && !dec.is_playing
                    {
                        dec.is_playing = true;
                        if dec.stdout.is_none() {
                            let pos = dec.current_pos_ms();
                            if let Err(err) = dec.respawn_at(pos) {
                                let _ = event_tx.send(DirectEvent::Error {
                                    seq: dec.seq,
                                    message: err,
                                });
                                active = None;
                                continue;
                            }
                        }
                        let _ = event_tx.send(DirectEvent::Playing {
                            seq: dec.seq,
                            uri: dec.uri.clone(),
                            position_ms: dec.current_pos_ms(),
                        });
                    }
                }
                DirectCmd::Pause => {
                    if let Some(ref mut dec) = active {
                        dec.is_playing = false;
                        sink.flush_and_close();
                        dsp.clear_analyzer();
                        let _ = event_tx.send(DirectEvent::Paused {
                            seq: dec.seq,
                            uri: dec.uri.clone(),
                            position_ms: dec.current_pos_ms(),
                        });
                    }
                }
                DirectCmd::SeekMs(pos_ms) => {
                    if let Some(ref mut dec) = active {
                        sink.flush_and_close();
                        if let Err(err) = dec.respawn_at(pos_ms) {
                            let _ = event_tx.send(DirectEvent::Error {
                                seq: dec.seq,
                                message: err,
                            });
                            active = None;
                        } else {
                            let _ = event_tx.send(DirectEvent::PositionChanged {
                                seq: dec.seq,
                                position_ms: pos_ms,
                            });
                        }
                    }
                }
                DirectCmd::SetSpeed(speed) => {
                    if let Some(ref mut dec) = active {
                        let cur = dec.current_pos_ms();
                        dec.speed = speed;
                        sink.flush_and_close();
                        if let Err(err) = dec.respawn_at(cur) {
                            let _ = event_tx.send(DirectEvent::Error {
                                seq: dec.seq,
                                message: err,
                            });
                            active = None;
                        }
                    }
                }
                DirectCmd::Load {
                    seq,
                    uri,
                    input,
                    start_ms,
                    speed,
                    start_playing,
                } => {
                    sink.flush_and_close();
                    let mut dec = ActiveDecoder {
                        seq,
                        uri: uri.clone(),
                        input,
                        seek_base_ms: start_ms,
                        frames_written: 0,
                        speed,
                        is_playing: start_playing,
                        child: None,
                        stdout: None,
                    };
                    if let Err(err) = dec.respawn_at(start_ms) {
                        let _ = event_tx.send(DirectEvent::Error { seq, message: err });
                        continue;
                    }
                    if start_playing {
                        let _ = event_tx.send(DirectEvent::Playing {
                            seq,
                            uri,
                            position_ms: start_ms,
                        });
                    } else {
                        let _ = event_tx.send(DirectEvent::Paused {
                            seq,
                            uri,
                            position_ms: start_ms,
                        });
                    }
                    last_pos_emit = Instant::now();
                    active = Some(dec);
                }
            }
        }
        if should_exit {
            drop(active);
            sink.flush_and_close();
            break;
        }

        let Some(ref mut dec) = active else {
            continue;
        };
        if !dec.is_playing {
            continue;
        }

        let Some(ref mut stdout) = dec.stdout else {
            continue;
        };

        // Read up to CHUNK_BYTES (handling short reads at EOF cleanly).
        let mut filled = 0usize;
        let mut read_err = None;
        while filled < raw_buf.len() {
            match stdout.read(&mut raw_buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    read_err = Some(e.to_string());
                    break;
                }
            }
        }

        if let Some(err) = read_err {
            let seq = dec.seq;
            active = None;
            sink.flush_and_close();
            let _ = event_tx.send(DirectEvent::Error {
                seq,
                message: format!("audio read error: {err}"),
            });
            continue;
        }

        let aligned = filled - (filled % (CHANNELS * 4));
        if aligned > 0 {
            f32_buf.clear();
            for chunk in raw_buf[..aligned].as_chunks::<4>().0 {
                f32_buf.push(f32::from_le_bytes(*chunk));
            }
            dsp.process_interleaved_f32(&mut f32_buf);

            let vol = volume.load(Ordering::Acquire) as f32 / 65_535.0;
            let fade = fade_milli.load(Ordering::Acquire) as f32 / 1000.0;
            let gain = (vol * fade).clamp(0.0, 1.0);

            pcm_bytes.clear();
            for &sample in &f32_buf {
                let s16 = (sample * gain * 32_767.0)
                    .round()
                    .clamp(-32_768.0, 32_767.0) as i16;
                pcm_bytes.extend_from_slice(&s16.to_ne_bytes());
            }

            if let Err(err) = sink.write_s16(&pcm_bytes) {
                let seq = dec.seq;
                active = None;
                sink.flush_and_close();
                let _ = event_tx.send(DirectEvent::Error { seq, message: err });
                continue;
            }
            dec.frames_written += (aligned / (CHANNELS * 4)) as u64;
        }

        if filled == 0 {
            let seq = dec.seq;
            let uri = dec.uri.clone();
            let failed_msg = dec
                .child
                .as_mut()
                .and_then(|c| {
                    let status = c.try_wait_finished()?;
                    if status.success() {
                        None
                    } else {
                        let stderr = c.drain_stderr();
                        Some(if stderr.is_empty() {
                            format!("ffmpeg exited with status {status}")
                        } else {
                            stderr
                        })
                    }
                });
            active = None;
            dsp.clear_analyzer();
            if let Some(msg) = failed_msg {
                sink.flush_and_close();
                let _ = event_tx.send(DirectEvent::Error { seq, message: msg });
            } else {
                sink.drain_and_close();
                let _ = event_tx.send(DirectEvent::EndOfTrack { seq, uri });
            }
            continue;
        }

        if last_pos_emit.elapsed() >= Duration::from_millis(200) {
            last_pos_emit = Instant::now();
            let _ = event_tx.send(DirectEvent::PositionChanged {
                seq: dec.seq,
                position_ms: dec.current_pos_ms(),
            });
        }
    }
}
