//! MPRIS2 D-Bus media player integration (`org.mpris.MediaPlayer2.pulse_spotify`).

use std::cell::RefCell;
use std::rc::Rc;

use mpris_server::{
    LocalPlayerInterface, LocalRootInterface, LocalServer, LoopStatus, Metadata, PlaybackRate,
    PlaybackStatus, Property, Signal, Time, TrackId, Volume, zbus::fdo,
};
use pulse_model::{CrateRef, PlayerState, RepeatMode, UiCommand};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
struct MprisSnapshot {
    is_playing: bool,
    position_ms: u32,
    duration_ms: u32,
    volume: f64,
    shuffle: bool,
    repeat: RepeatMode,
    media_keys_enabled: bool,
    track_id: TrackId,
    track_uri: String,
    title: String,
    artist: String,
    album: String,
    cover_url: Option<String>,
}

impl Default for MprisSnapshot {
    fn default() -> Self {
        Self {
            is_playing: false,
            position_ms: 0,
            duration_ms: 0,
            volume: 1.0,
            shuffle: false,
            repeat: RepeatMode::Off,
            media_keys_enabled: true,
            track_id: TrackId::NO_TRACK,
            track_uri: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            cover_url: None,
        }
    }
}

impl MprisSnapshot {
    fn playback_status(&self) -> PlaybackStatus {
        if self.track_uri.is_empty() {
            PlaybackStatus::Stopped
        } else if self.is_playing {
            PlaybackStatus::Playing
        } else {
            PlaybackStatus::Paused
        }
    }

    fn loop_status(&self) -> LoopStatus {
        match self.repeat {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::Context => LoopStatus::Playlist,
            RepeatMode::Track => LoopStatus::Track,
        }
    }

    fn metadata(&self) -> Metadata {
        if self.track_uri.is_empty() {
            return Metadata::new();
        }
        let mut meta = Metadata::builder()
            .trackid(self.track_id.clone())
            .title(&self.title)
            .album(&self.album)
            .length(Time::from_millis(self.duration_ms as i64))
            .url(&self.track_uri)
            .build();
        if !self.artist.is_empty() {
            meta.set_artist(Some([self.artist.as_str()]));
        }
        if let Some(ref art) = self.cover_url {
            meta.set_art_url(Some(art.as_str()));
        }
        meta
    }
}

struct PulseMprisImp {
    state: Rc<RefCell<MprisSnapshot>>,
    cmd_tx: mpsc::UnboundedSender<UiCommand>,
}

impl PulseMprisImp {
    fn send_if_enabled(&self, cmd: UiCommand) {
        if self.state.borrow().media_keys_enabled {
            let _ = self.cmd_tx.send(cmd);
        }
    }
}

impl LocalRootInterface for PulseMprisImp {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("Pulse".to_string())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("pulse-spotify".to_string())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(vec!["spotify".to_string()])
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl LocalPlayerInterface for PulseMprisImp {
    async fn next(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::NextTrack);
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::PrevTrack);
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::Pause);
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::TogglePlayPause);
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::Pause);
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        self.send_if_enabled(UiCommand::Play);
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let s = self.state.borrow();
        if !s.media_keys_enabled {
            return Ok(());
        }
        let target = (s.position_ms as i64).saturating_add(offset.as_millis()).max(0) as u64;
        let clamped = if s.duration_ms > 0 {
            target.min(s.duration_ms as u64)
        } else {
            target
        };
        drop(s);
        let _ = self
            .cmd_tx
            .send(UiCommand::SeekMs(clamped.min(u32::MAX as u64) as u32));
        Ok(())
    }

    async fn set_position(&self, track_id: TrackId, position: Time) -> fdo::Result<()> {
        let s = self.state.borrow();
        if !s.media_keys_enabled || track_id != s.track_id {
            return Ok(());
        }
        let pos_ms = position.as_millis().clamp(0, u32::MAX as i64) as u32;
        drop(s);
        let _ = self.cmd_tx.send(UiCommand::SeekMs(pos_ms));
        Ok(())
    }

    async fn open_uri(&self, uri: String) -> fdo::Result<()> {
        if !self.state.borrow().media_keys_enabled {
            return Ok(());
        }
        let crate_ref = if uri.starts_with("spotify:playlist:") {
            Some(CrateRef::Playlist {
                uri: uri.as_str().into(),
                name: String::new(),
            })
        } else if uri.starts_with("spotify:album:") {
            Some(CrateRef::Album {
                uri: uri.as_str().into(),
                title: String::new(),
            })
        } else if uri.starts_with("spotify:artist:") {
            Some(CrateRef::Artist {
                uri: uri.as_str().into(),
                name: String::new(),
            })
        } else {
            None
        };
        if let Some(crate_ref) = crate_ref {
            let _ = self.cmd_tx.send(UiCommand::PlayCrate {
                crate_ref,
                shuffle: false,
            });
        } else if uri.starts_with("spotify:track:") {
            let _ = self.cmd_tx.send(UiCommand::PlayTrackInContext {
                context_uri: String::new(),
                track_uri: uri,
                index: 0,
            });
        }
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.state.borrow().playback_status())
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(self.state.borrow().loop_status())
    }

    async fn set_loop_status(&self, loop_status: LoopStatus) -> mpris_server::zbus::Result<()> {
        let s = self.state.borrow();
        if !s.media_keys_enabled {
            return Ok(());
        }
        let desired = match loop_status {
            LoopStatus::None => RepeatMode::Off,
            LoopStatus::Playlist => RepeatMode::Context,
            LoopStatus::Track => RepeatMode::Track,
        };
        let mut current = s.repeat;
        drop(s);

        // `UiCommand::CycleRepeat` cycles Off -> Context -> Track -> Off.
        for _ in 0..3 {
            if current == desired {
                break;
            }
            let _ = self.cmd_tx.send(UiCommand::CycleRepeat);
            current = current.next();
        }
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().shuffle)
    }

    async fn set_shuffle(&self, shuffle: bool) -> mpris_server::zbus::Result<()> {
        let s = self.state.borrow();
        if s.media_keys_enabled && s.shuffle != shuffle {
            drop(s);
            let _ = self.cmd_tx.send(UiCommand::ToggleShuffle);
        }
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.state.borrow().metadata())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.state.borrow().volume)
    }

    async fn set_volume(&self, volume: Volume) -> mpris_server::zbus::Result<()> {
        let v = (volume.clamp(0.0, 1.0) * 65535.0).round() as u16;
        self.send_if_enabled(UiCommand::SetVolume(v));
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::from_millis(self.state.borrow().position_ms as i64))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(self.state.borrow().media_keys_enabled)
    }
}

/// Active local-task MPRIS2 D-Bus server controller.
pub struct MprisController {
    server: Rc<LocalServer<PulseMprisImp>>,
    state: Rc<RefCell<MprisSnapshot>>,
    task: tokio::task::JoinHandle<()>,
}

impl MprisController {
    /// Starts an MPRIS2 server on the current `LocalSet`.
    pub async fn start(
        cmd_tx: mpsc::UnboundedSender<UiCommand>,
        media_keys_enabled: bool,
    ) -> Result<Self, String> {
        let state = Rc::new(RefCell::new(MprisSnapshot {
            media_keys_enabled,
            ..Default::default()
        }));
        let imp = PulseMprisImp {
            state: Rc::clone(&state),
            cmd_tx,
        };
        let server = LocalServer::new("pulse_spotify", imp)
            .await
            .map_err(|e| format!("failed to register MPRIS2 D-Bus server: {e}"))?;
        let server = Rc::new(server);
        let server_run = Rc::clone(&server);
        let task = tokio::task::spawn_local(async move {
            server_run.run().await;
        });

        Ok(Self {
            server,
            state,
            task,
        })
    }

    /// Updates media-key enablement dynamically when settings change.
    pub async fn set_media_keys_enabled(&self, enabled: bool) {
        let changed = {
            let mut s = self.state.borrow_mut();
            let old = s.media_keys_enabled;
            s.media_keys_enabled = enabled;
            old != enabled
        };
        if changed {
            let _ = self
                .server
                .properties_changed([
                    Property::CanPlay(enabled),
                    Property::CanPause(enabled),
                    Property::CanGoNext(enabled),
                    Property::CanGoPrevious(enabled),
                    Property::CanSeek(enabled),
                ])
                .await;
        }
    }

    /// Syncs changed [`PlayerState`] properties and D-Bus signals.
    pub async fn sync_player_state(&self, player: &PlayerState, seeked: bool) {
        let prev = self.state.borrow().clone();

        let new_vol = if player.muted {
            0.0
        } else {
            (player.volume as f64 / 65535.0).clamp(0.0, 1.0)
        };
        let (track_uri, title, artist, album, cover_url) = match &player.active_track {
            Some(t) => (
                t.summary.uri.to_string(),
                t.summary.title.clone(),
                t.summary.artist.to_string(),
                t.summary.album.to_string(),
                t.summary
                    .cover
                    .as_ref()
                    .and_then(|c| c.url.as_deref().map(ToOwned::to_owned)),
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                None,
            ),
        };
        let track_id = make_track_id(&track_uri);

        let next = MprisSnapshot {
            is_playing: player.is_playing,
            position_ms: player.position_ms,
            duration_ms: player.duration_ms,
            volume: new_vol,
            shuffle: player.shuffle,
            repeat: player.repeat,
            media_keys_enabled: prev.media_keys_enabled,
            track_id,
            track_uri,
            title,
            artist,
            album,
            cover_url,
        };

        let mut props = Vec::new();
        if prev.playback_status() != next.playback_status() {
            props.push(Property::PlaybackStatus(next.playback_status()));
        }
        if prev.loop_status() != next.loop_status() {
            props.push(Property::LoopStatus(next.loop_status()));
        }
        if prev.shuffle != next.shuffle {
            props.push(Property::Shuffle(next.shuffle));
        }
        if (prev.volume - next.volume).abs() > 0.005 {
            props.push(Property::Volume(next.volume));
        }
        if prev.track_uri != next.track_uri
            || prev.title != next.title
            || prev.artist != next.artist
            || prev.cover_url != next.cover_url
            || prev.duration_ms != next.duration_ms
        {
            props.push(Property::Metadata(next.metadata()));
        }

        *self.state.borrow_mut() = next;

        if !props.is_empty() {
            let _ = self.server.properties_changed(props).await;
        }
        if seeked {
            let _ = self
                .server
                .emit(Signal::Seeked {
                    position: Time::from_millis(player.position_ms as i64),
                })
                .await;
        }
    }

    /// Stops the MPRIS2 D-Bus server task.
    pub fn stop(self) {
        self.task.abort();
    }
}

fn make_track_id(uri: &str) -> TrackId {
    if uri.is_empty() {
        return TrackId::NO_TRACK;
    }
    let sanitized: String = uri
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path = format!("/org/pulse_spotify/track/{sanitized}");
    TrackId::try_from(path).unwrap_or(TrackId::NO_TRACK)
}
