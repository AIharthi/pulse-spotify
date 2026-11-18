//! Two-tier playback queue ("Next in Queue" manual priority + "Next from Context").

use std::sync::Arc;

use pulse_model::{QueueState, TrackSummary};
use spotatui_librespot_core::SpotifyUri;

/// Maintains the two-tier playback queue in the backend so manual priority items
/// always play before remaining context tracks and survive track changes cleanly.
#[derive(Debug, Clone, Default)]
pub struct TwoTierQueue {
    manual: Vec<TrackSummary>,
    context_tracks: Vec<TrackSummary>,
    /// Original unshuffled context order when shuffle is active.
    unshuffled_context: Option<Vec<TrackSummary>>,
    /// Index of the next upcoming context track (`context_tracks[context_cursor..]`).
    context_cursor: usize,
    /// Index of the currently playing context track (when playing from context).
    current_context_index: Option<usize>,
    /// Currently active track (whether from manual queue, context, or standalone).
    current_track: Option<TrackSummary>,
    /// URI of the active track (kept in sync even when metadata lookup hasn't resolved).
    current_uri: Option<Arc<str>>,
    context_name: String,
    context_uri: Option<Arc<str>>,
}

fn shuffle_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ 0xBF58_476D_1CE4_E5B9
}

fn next_rand(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl TwoTierQueue {
    /// Creates an empty two-tier queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a track at the very front of the manual queue ("Play Next").
    pub fn enqueue_next(&mut self, track: TrackSummary) {
        self.manual.insert(0, track);
    }

    /// Appends a track to the end of the manual queue ("Add to Queue").
    pub fn append(&mut self, track: TrackSummary) {
        self.manual.push(track);
    }

    /// Moves a manual queue item from `from` to `to`. Returns `true` if moved.
    pub fn reorder_manual(&mut self, from: usize, to: usize) -> bool {
        if from >= self.manual.len() || self.manual.is_empty() {
            return false;
        }
        let dest = to.min(self.manual.len() - 1);
        if from == dest {
            return false;
        }
        let item = self.manual.remove(from);
        self.manual.insert(dest, item);
        true
    }

    /// Promotes an upcoming context track at `context_index` into `manual[manual_index]`.
    pub fn promote_context_to_manual(
        &mut self,
        context_index: usize,
        manual_index: usize,
    ) -> Option<TrackSummary> {
        let abs = self.context_cursor.checked_add(context_index)?;
        if abs >= self.context_tracks.len() {
            return None;
        }
        let track = self.context_tracks.remove(abs);
        let insert_at = manual_index.min(self.manual.len());
        self.manual.insert(insert_at, track.clone());
        Some(track)
    }

    /// Removes a manual queue item by index.
    pub fn remove_manual(&mut self, index: usize) -> Option<TrackSummary> {
        if index < self.manual.len() {
            Some(self.manual.remove(index))
        } else {
            None
        }
    }

    /// Clears all manual queue entries while keeping context queue intact.
    pub fn clear_manual(&mut self) {
        self.manual.clear();
    }

    /// Replaces the active context and positions the upcoming cursor right after `playing_index`.
    pub fn set_context(
        &mut self,
        context_name: impl Into<String>,
        context_uri: Option<Arc<str>>,
        tracks: Vec<TrackSummary>,
        playing_index: Option<usize>,
    ) {
        self.context_name = context_name.into();
        self.context_uri = context_uri;
        self.context_tracks = tracks;
        self.unshuffled_context = None;
        self.current_context_index = playing_index.filter(|&i| i < self.context_tracks.len());
        self.context_cursor = match self.current_context_index {
            Some(i) => (i + 1).min(self.context_tracks.len()),
            None => 0,
        };
        if let Some(idx) = self.current_context_index {
            let track = self.context_tracks[idx].clone();
            self.current_uri = Some(track.uri.clone());
            self.current_track = Some(track);
        }
    }

    /// Clears the context queue.
    pub fn clear_context(&mut self) {
        self.context_tracks.clear();
        self.unshuffled_context = None;
        self.context_cursor = 0;
        self.current_context_index = None;
        self.context_name.clear();
        self.context_uri = None;
    }

    /// Sets the currently active track explicitly (e.g. optimistic play or resume restore).
    pub fn set_current_track(&mut self, track: Option<TrackSummary>) {
        self.current_uri = track.as_ref().map(|t| t.uri.clone());
        self.current_track = track;
    }

    /// Returns `true` if shuffle ordering is currently active on the local queue.
    pub fn is_shuffled(&self) -> bool {
        self.unshuffled_context.is_some()
    }

    /// Enables or disables local context shuffle while preserving the currently playing track
    /// and cleanly restoring its original position when unshuffled.
    pub fn set_shuffle(&mut self, enabled: bool) -> bool {
        if enabled {
            if self.unshuffled_context.is_some() {
                return false;
            }
            let original = self.context_tracks.clone();
            let mut seed = shuffle_seed();
            let shuffle_start = if self.context_tracks.len() > 1 {
                if let Some(cur_idx) = self.current_context_index
                    && cur_idx < self.context_tracks.len()
                {
                    self.context_tracks.swap(0, cur_idx);
                    self.current_context_index = Some(0);
                    self.context_cursor = 1;
                    1
                } else {
                    self.context_cursor.min(self.context_tracks.len())
                }
            } else {
                0
            };

            if shuffle_start < self.context_tracks.len() {
                let slice = &mut self.context_tracks[shuffle_start..];
                for i in (1..slice.len()).rev() {
                    let j = (next_rand(&mut seed) as usize) % (i + 1);
                    slice.swap(i, j);
                }
            }

            self.unshuffled_context = Some(original);
            true
        } else {
            let Some(original) = self.unshuffled_context.take() else {
                return false;
            };
            let active_uri = self
                .current_track
                .as_ref()
                .map(|t| t.uri.clone())
                .or_else(|| self.current_uri.clone());
            self.context_tracks = original;
            if let Some(uri) = active_uri
                && let Some(pos) = self.context_tracks.iter().position(|t| t.uri == uri)
            {
                self.current_context_index = Some(pos);
                self.context_cursor = (pos + 1).min(self.context_tracks.len());
            } else {
                self.current_context_index = None;
                self.context_cursor = 0;
            }
            true
        }
    }

    /// Jumps to a unified upcoming queue index:
    /// `0..manual.len()` selects from manual queue (draining prior manual entries);
    /// indices `>= manual.len()` jump into `context_items[idx - manual.len()]`.
    /// Returns `(chosen_track, is_manual)`.
    pub fn play_queue_index(&mut self, index: usize) -> Option<(TrackSummary, bool)> {
        if index < self.manual.len() {
            let chosen = self.manual[index].clone();
            self.manual.drain(0..=index);
            self.current_uri = Some(chosen.uri.clone());
            self.current_track = Some(chosen.clone());
            return Some((chosen, true));
        }

        let ctx_rel = index.checked_sub(self.manual.len())?;
        let abs = self.context_cursor.checked_add(ctx_rel)?;
        let chosen = self.context_tracks.get(abs)?.clone();
        self.current_context_index = Some(abs);
        self.context_cursor = (abs + 1).min(self.context_tracks.len());
        self.current_uri = Some(chosen.uri.clone());
        self.current_track = Some(chosen.clone());
        Some((chosen, false))
    }

    /// Advances to the next track (`manual` first, then `context_tracks`).
    pub fn step_next_track(&mut self, wrap_around: bool) -> Option<TrackSummary> {
        if !self.manual.is_empty() {
            let next = self.manual.remove(0);
            self.current_uri = Some(next.uri.clone());
            self.current_track = Some(next.clone());
            return Some(next);
        }
        if self.context_cursor < self.context_tracks.len() {
            let idx = self.context_cursor;
            let next = self.context_tracks[idx].clone();
            self.current_context_index = Some(idx);
            self.context_cursor = (idx + 1).min(self.context_tracks.len());
            self.current_uri = Some(next.uri.clone());
            self.current_track = Some(next.clone());
            return Some(next);
        }
        if wrap_around && !self.context_tracks.is_empty() {
            let next = self.context_tracks[0].clone();
            self.current_context_index = Some(0);
            self.context_cursor = 1.min(self.context_tracks.len());
            self.current_uri = Some(next.uri.clone());
            self.current_track = Some(next.clone());
            return Some(next);
        }
        None
    }

    /// Steps back to the previous track in `context_tracks` (wrapping when requested).
    pub fn step_prev_track(&mut self, wrap_around: bool) -> Option<TrackSummary> {
        if let Some(cur) = self.current_context_index
            && !self.context_tracks.is_empty()
        {
            let prev_idx = if cur > 0 {
                cur - 1
            } else if wrap_around {
                self.context_tracks.len() - 1
            } else {
                0
            };
            let prev = self.context_tracks[prev_idx].clone();
            self.current_context_index = Some(prev_idx);
            self.context_cursor = (prev_idx + 1).min(self.context_tracks.len());
            self.current_uri = Some(prev.uri.clone());
            self.current_track = Some(prev.clone());
            return Some(prev);
        }
        if !self.context_tracks.is_empty() {
            let prev = self.context_tracks[0].clone();
            self.current_context_index = Some(0);
            self.context_cursor = 1.min(self.context_tracks.len());
            self.current_uri = Some(prev.uri.clone());
            self.current_track = Some(prev.clone());
            return Some(prev);
        }
        self.current_track.clone()
    }

    /// Advances queue pointers when a new `track_uri` starts playing.
    ///
    /// Idempotent when called repeatedly for the currently playing `track_uri` (e.g. pause -> resume
    /// or `TrackChanged` followed by `Playing`), preventing manual queue corruption.
    pub fn notify_track_started(&mut self, track_uri: &str) {
        if self.current_uri.as_deref() == Some(track_uri) {
            return;
        }
        self.current_uri = Some(Arc::from(track_uri));

        if self.manual.first().is_some_and(|t| &*t.uri == track_uri) {
            let item = self.manual.remove(0);
            self.current_track = Some(item);
            return;
        }
        if let Some(pos) = self
            .context_tracks
            .iter()
            .enumerate()
            .skip(self.context_cursor)
            .find(|(_, t)| &*t.uri == track_uri)
            .map(|(i, _)| i)
        {
            self.current_context_index = Some(pos);
            self.context_cursor = (pos + 1).min(self.context_tracks.len());
            self.current_track = Some(self.context_tracks[pos].clone());
        } else if let Some(pos) = self
            .context_tracks
            .iter()
            .position(|t| &*t.uri == track_uri)
        {
            self.current_context_index = Some(pos);
            self.context_cursor = (pos + 1).min(self.context_tracks.len());
            self.current_track = Some(self.context_tracks[pos].clone());
        } else {
            self.current_track = None;
        }
    }

    /// Replaces manual and upcoming context lists from a remote Connect `Cluster`.
    pub fn apply_remote_snapshot(
        &mut self,
        context_name: String,
        context_uri: Option<Arc<str>>,
        manual: Vec<TrackSummary>,
        upcoming_context: Vec<TrackSummary>,
    ) {
        self.context_name = context_name;
        self.context_uri = context_uri;
        self.manual = manual;
        self.context_tracks = upcoming_context;
        self.unshuffled_context = None;
        self.current_context_index = None;
        self.context_cursor = 0;
    }

    /// Looks up metadata for `uri` from active/queued items in memory.
    pub fn find_track(&self, uri: &str) -> Option<&TrackSummary> {
        if let Some(ref cur) = self.current_track
            && &*cur.uri == uri
        {
            return Some(cur);
        }
        self.manual
            .iter()
            .chain(self.context_tracks.iter())
            .chain(self.unshuffled_context.iter().flatten())
            .find(|t| &*t.uri == uri)
    }

    /// Returns the currently active `TrackSummary` if known.
    pub fn current_track(&self) -> Option<&TrackSummary> {
        self.current_track.as_ref()
    }

    /// Returns the currently active track URI if known.
    pub fn current_uri(&self) -> Option<&str> {
        self.current_uri.as_deref()
    }

    /// Builds ordered context URIs starting at `current_uri` followed by upcoming context tracks.
    pub fn remaining_uris_from_current(&self) -> Vec<String> {
        let upcoming = self.context_upcoming();
        let mut uris = Vec::with_capacity(1 + upcoming.len().min(500));
        if let Some(cur) = self.current_uri() {
            uris.push(cur.to_string());
        }
        for t in upcoming.iter().take(500) {
            uris.push(t.uri.to_string());
        }
        uris
    }

    /// Returns parsed Spotify URIs for the manual queue in exact play order.
    pub fn manual_spotify_uris(&self) -> Vec<SpotifyUri> {
        self.manual
            .iter()
            .filter_map(|t| SpotifyUri::from_uri(&t.uri).ok())
            .collect()
    }

    /// Returns the manual queue slice.
    pub fn manual_items(&self) -> &[TrackSummary] {
        &self.manual
    }

    /// Returns the upcoming context tracks slice.
    pub fn context_upcoming(&self) -> &[TrackSummary] {
        self.context_tracks
            .get(self.context_cursor..)
            .unwrap_or(&[])
    }

    /// Returns the current context URI, if set.
    pub fn context_uri(&self) -> Option<&str> {
        self.context_uri.as_deref()
    }

    /// Builds an immutable [`QueueState`] snapshot for UI rendering.
    pub fn to_queue_state(&self) -> QueueState {
        QueueState {
            manual_items: self.manual.clone(),
            context_items: self.context_upcoming().to_vec(),
            context_name: self.context_name.clone(),
            context_uri: self.context_uri.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> TrackSummary {
        TrackSummary {
            uri: Arc::from(format!("spotify:track:{id}")),
            title: format!("Track {id}"),
            artist: Arc::from("Artist"),
            artist_uri: None,
            album: Arc::from("Album"),
            album_uri: None,
            cover: None,
            duration_ms: 180_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
        }
    }

    #[test]
    fn notify_track_started_is_idempotent_across_pause_resume() {
        let mut q = TwoTierQueue::new();
        q.set_context(
            "Liked",
            None,
            vec![track("a"), track("b"), track("a")],
            Some(0),
        );
        q.enqueue_next(track("m1"));

        q.notify_track_started("spotify:track:a");
        assert_eq!(q.manual_items().len(), 1);
        assert_eq!(q.current_context_index, Some(0));

        q.notify_track_started("spotify:track:m1");
        assert_eq!(q.manual_items().len(), 0);
        q.notify_track_started("spotify:track:m1");
        assert_eq!(q.current_uri(), Some("spotify:track:m1"));
    }

    #[test]
    fn step_next_and_prev_follow_manual_and_context_bounds() {
        let mut q = TwoTierQueue::new();
        q.set_context("Mix", None, vec![track("1"), track("2"), track("3")], Some(0));
        q.enqueue_next(track("m"));

        assert_eq!(&*q.step_next_track(false).unwrap().uri, "spotify:track:m");
        assert_eq!(&*q.step_next_track(false).unwrap().uri, "spotify:track:2");
        assert_eq!(&*q.step_next_track(false).unwrap().uri, "spotify:track:3");
        assert!(q.step_next_track(false).is_none());
        assert_eq!(&*q.step_next_track(true).unwrap().uri, "spotify:track:1");

        assert_eq!(&*q.step_prev_track(false).unwrap().uri, "spotify:track:1");
        assert_eq!(&*q.step_prev_track(true).unwrap().uri, "spotify:track:3");
    }

    #[test]
    fn set_shuffle_preserves_current_track_and_restores_original_order() {
        let mut q = TwoTierQueue::new();
        let original = vec![track("1"), track("2"), track("3"), track("4"), track("5")];
        q.set_context("Playlist", None, original.clone(), Some(2));

        assert!(q.set_shuffle(true));
        assert!(q.is_shuffled());
        assert_eq!(&*q.current_track().unwrap().uri, "spotify:track:3");
        assert_eq!(q.context_tracks.len(), original.len());
        assert_eq!(&*q.context_tracks[0].uri, "spotify:track:3");

        assert!(q.set_shuffle(false));
        assert!(!q.is_shuffled());
        let restored_uris: Vec<&str> = q.context_tracks.iter().map(|t| &*t.uri).collect();
        let orig_uris: Vec<&str> = original.iter().map(|t| &*t.uri).collect();
        assert_eq!(restored_uris, orig_uris);
        assert_eq!(q.current_context_index, Some(2));
    }
}
