//! Offline YouTube -> MP3 transcoder (`ffmpeg` + `lame` with `libmp3lame` fallback) and ID3v2 tagger.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, TagExt};
use lofty::tag::{Tag, TagType};
use pulse_model::{Mp3Quality, SponsorSegment, TrackSummary};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempCleanupGuard {
    wav_path: PathBuf,
    mp3_path: PathBuf,
    keep_mp3: bool,
}

impl Drop for TempCleanupGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.wav_path);
        if !self.keep_mp3 {
            let _ = fs::remove_file(&self.mp3_path);
        }
    }
}

/// Builds a filesystem-safe basename (`Artist - Title`) for saving downloaded tracks.
pub fn sanitize_filename_stem(artist: &str, title: &str) -> String {
    let raw_title = title.trim();
    let raw_artist = artist.trim();
    let combined = if raw_artist.is_empty() || raw_artist.eq_ignore_ascii_case("YouTube") {
        raw_title.to_string()
    } else {
        format!("{raw_artist} - {raw_title}")
    };

    let mut clean = String::with_capacity(combined.len());
    let mut prev_space = false;
    for ch in combined.chars() {
        let mapped = match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => ' ',
            c => c,
        };
        if mapped.is_whitespace() {
            if !prev_space && !clean.is_empty() {
                clean.push(' ');
            }
            prev_space = true;
        } else {
            clean.push(mapped);
            prev_space = false;
        }
    }
    let trimmed = clean.trim_matches(|c: char| c == ' ' || c == '.');
    let mut truncated: String = trimmed.chars().take(140).collect();
    while truncated.ends_with([' ', '.']) {
        truncated.pop();
    }
    if truncated.is_empty() {
        "youtube_track".to_string()
    } else {
        truncated
    }
}

/// Ensures the directory exists and picks a non-colliding `<stem>.mp3` path.
pub fn resolve_unique_mp3_path(dir: &Path, stem: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create MP3 download folder {}: {e}", dir.display()))?;
    let first = dir.join(format!("{stem}.mp3"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 2..=999 {
        let cand = dir.join(format!("{stem} ({n}).mp3"));
        if !cand.exists() {
            return Ok(cand);
        }
    }
    Ok(dir.join(format!("{stem}_{}.mp3", std::process::id())))
}

/// Builds an `ffmpeg` `-af` expression that excises SponsorBlock intervals when safe to do so.
pub fn build_sponsor_aselect_filter(
    segments: &[SponsorSegment],
    total_duration_ms: u32,
) -> Option<String> {
    let mut valid = Vec::new();
    let mut skipped_ms: u64 = 0;
    for seg in segments {
        if seg.end_ms >= seg.start_ms.saturating_add(500) {
            skipped_ms = skipped_ms.saturating_add(u64::from(seg.end_ms - seg.start_ms));
            valid.push(seg);
        }
    }
    if valid.is_empty() {
        return None;
    }
    if total_duration_ms > 0 && skipped_ms.saturating_mul(100) >= u64::from(total_duration_ms).saturating_mul(85) {
        return None;
    }

    let exprs: Vec<String> = valid
        .into_iter()
        .map(|seg| {
            let s = f64::from(seg.start_ms) / 1000.0;
            let e = f64::from(seg.end_ms) / 1000.0;
            format!("between(t,{s:.3},{e:.3})")
        })
        .collect();
    Some(format!("aselect='not({})',asetpts=N/SR/TB", exprs.join("+")))
}

fn has_lame_cli() -> bool {
    Command::new("lame")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Transcodes cached YouTube audio into an ID3v2-tagged MP3 file, optionally trimming sponsor segments.
pub fn transcode_and_tag_mp3_blocking(
    input_audio: &Path,
    dest_mp3: &Path,
    quality: Mp3Quality,
    strip_filter: Option<&str>,
    track: &TrackSummary,
    video_id: &str,
    cover_jpeg: Option<&[u8]>,
) -> Result<PathBuf, String> {
    if !input_audio.is_file() {
        return Err(format!("source audio missing at {}", input_audio.display()));
    }
    let parent = dest_mp3
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| format!("failed to prepare destination folder {}: {e}", parent.display()))?;

    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let wav_path = parent.join(format!(".aplayer_tmp_{pid}_{seq}.wav"));
    let tmp_mp3 = parent.join(format!(".aplayer_tmp_{pid}_{seq}.mp3"));
    let mut guard = TempCleanupGuard {
        wav_path: wav_path.clone(),
        mp3_path: tmp_mp3.clone(),
        keep_mp3: false,
    };

    if has_lame_cli() {
        let mut ff = Command::new("ffmpeg");
        ff.args(["-v", "error", "-y", "-i"])
            .arg(input_audio)
            .arg("-vn");
        if let Some(af) = strip_filter {
            ff.arg("-af").arg(af);
        }
        ff.args(["-ac", "2", "-ar", "44100", "-f", "wav"])
            .arg(&wav_path)
            .stdin(Stdio::null());
        let ff_out = ff
            .output()
            .map_err(|e| format!("failed to launch ffmpeg decoder: {e}"))?;
        if !ff_out.status.success() {
            let msg = String::from_utf8_lossy(&ff_out.stderr);
            return Err(format!("ffmpeg WAV decode failed: {}", msg.trim()));
        }
        if fs::metadata(&wav_path).map(|m| m.len()).unwrap_or(0) <= 1024 {
            return Err("ffmpeg produced an empty WAV buffer".to_string());
        }

        let mut lame = Command::new("lame");
        lame.arg("--quiet");
        match quality {
            Mp3Quality::Cbr320 => {
                lame.args(["-b", "320", "-q", "2"]);
            }
            Mp3Quality::VbrV0 => {
                lame.args(["-V", "0"]);
            }
            Mp3Quality::Cbr192 => {
                lame.args(["-b", "192", "-q", "2"]);
            }
            Mp3Quality::Cbr128 => {
                lame.args(["-b", "128", "-q", "2"]);
            }
        }
        lame.arg(&wav_path).arg(&tmp_mp3).stdin(Stdio::null());
        let lame_out = lame
            .output()
            .map_err(|e| format!("failed to launch lame encoder: {e}"))?;
        let _ = fs::remove_file(&wav_path);
        if !lame_out.status.success() {
            let msg = String::from_utf8_lossy(&lame_out.stderr);
            return Err(format!("lame MP3 encode failed: {}", msg.trim()));
        }
    } else {
        let mut ff = Command::new("ffmpeg");
        ff.args(["-v", "error", "-y", "-i"])
            .arg(input_audio)
            .arg("-vn");
        if let Some(af) = strip_filter {
            ff.arg("-af").arg(af);
        }
        ff.args(["-ac", "2", "-ar", "44100", "-c:a", "libmp3lame"]);
        match quality {
            Mp3Quality::Cbr320 => {
                ff.args(["-b:a", "320k"]);
            }
            Mp3Quality::VbrV0 => {
                ff.args(["-q:a", "0"]);
            }
            Mp3Quality::Cbr192 => {
                ff.args(["-b:a", "192k"]);
            }
            Mp3Quality::Cbr128 => {
                ff.args(["-b:a", "128k"]);
            }
        }
        ff.args(["-f", "mp3"]).arg(&tmp_mp3).stdin(Stdio::null());
        let ff_out = ff
            .output()
            .map_err(|e| format!("failed to launch ffmpeg MP3 encoder: {e}"))?;
        if !ff_out.status.success() {
            let msg = String::from_utf8_lossy(&ff_out.stderr);
            return Err(format!("ffmpeg MP3 encode failed: {}", msg.trim()));
        }
    }

    if fs::metadata(&tmp_mp3).map(|m| m.len()).unwrap_or(0) <= 2048 {
        return Err("MP3 encoder produced an invalid or empty output file".to_string());
    }

    // Write clean ID3v2 tags + embedded album cover via lofty.
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(track.title.clone());
    tag.set_artist(track.artist.to_string());
    tag.set_album(if track.album.is_empty() {
        "YouTube".to_string()
    } else {
        track.album.to_string()
    });
    if !video_id.is_empty() {
        tag.set_comment(format!("https://www.youtube.com/watch?v={video_id}"));
    }
    if let Some(bytes) = cover_jpeg
        && bytes.len() >= 128
    {
        tag.push_picture(
            Picture::unchecked(bytes.to_vec())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Jpeg)
                .build(),
        );
    }
    let _ = tag.save_to_path(&tmp_mp3, WriteOptions::default());

    if dest_mp3.exists() {
        let _ = fs::remove_file(dest_mp3);
    }
    if fs::rename(&tmp_mp3, dest_mp3).is_err() {
        fs::copy(&tmp_mp3, dest_mp3)
            .map_err(|e| format!("failed to move MP3 to {}: {e}", dest_mp3.display()))?;
        let _ = fs::remove_file(&tmp_mp3);
    }
    guard.keep_mp3 = true;

    if fs::metadata(dest_mp3).map(|m| m.len()).unwrap_or(0) <= 2048 {
        return Err("saved MP3 verification failed".to_string());
    }
    Ok(dest_mp3.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lofty::file::{AudioFile, TaggedFileExt};
    use pulse_model::MediaSource;

    use super::*;

    #[test]
    fn sanitize_and_filter_builders_handle_edge_cases() {
        let stem = sanitize_filename_stem("Daft Punk - Topic", "Around / The: World?");
        assert_eq!(stem, "Daft Punk - Topic - Around _ The_ World_");
        assert_eq!(sanitize_filename_stem("YouTube", "   ...  "), "youtube_track");

        let segs = vec![
            SponsorSegment {
                start_ms: 0,
                end_ms: 1_500,
                category: "intro".to_string(),
            },
            SponsorSegment {
                start_ms: 30_000,
                end_ms: 45_000,
                category: "sponsor".to_string(),
            },
        ];
        let af = build_sponsor_aselect_filter(&segs, 180_000).unwrap();
        assert!(af.contains("between(t,0.000,1.500)+between(t,30.000,45.000)"));

        // Safety guard refuses to cut >= 85% of total duration
        assert!(build_sponsor_aselect_filter(&segs, 18_000).is_none());
    }

    #[test]
    fn transcodes_synthetic_opus_to_tagged_mp3_with_sponsor_trim() {
        let root = std::env::temp_dir().join(format!("aplayer_mp3_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let src_ogg = root.join("sine.ogg");
        let st = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:a",
                "libopus",
            ])
            .arg(&src_ogg)
            .status()
            .expect("ffmpeg sine generation");
        assert!(st.success());

        let track = TrackSummary {
            uri: Arc::from("yt:video:dQw4w9WgXcQ"),
            title: "Test Tone".to_string(),
            artist: Arc::from("Aplayer Lab"),
            artist_uri: None,
            album: Arc::from("YouTube"),
            album_uri: None,
            cover: None,
            duration_ms: 2_000,
            is_liked: false,
            is_playable: true,
            added_at_unix: None,
            source: MediaSource::YouTube,
            local_path: None,
            codec_label: Some(Arc::from("YT OPUS")),
        };

        let segs = [SponsorSegment {
            start_ms: 0,
            end_ms: 500,
            category: "sponsor".to_string(),
        }];
        let af = build_sponsor_aselect_filter(&segs, 2_000);
        let out_mp3 = resolve_unique_mp3_path(&root, "Aplayer Lab - Test Tone").unwrap();
        let saved = transcode_and_tag_mp3_blocking(
            &src_ogg,
            &out_mp3,
            Mp3Quality::Cbr192,
            af.as_deref(),
            &track,
            "dQw4w9WgXcQ",
            None,
        )
        .expect("transcode_and_tag_mp3_blocking");

        assert!(saved.is_file());
        let tagged = lofty::probe::Probe::open(&saved).unwrap().read().unwrap();
        let dur_ms = tagged.properties().duration().as_millis();
        assert!(
            (1200..=1800).contains(&dur_ms),
            "expected ~1500ms trimmed duration, got {dur_ms}ms"
        );
        let tag = tagged.primary_tag().expect("ID3v2 tag written");
        assert_eq!(tag.title().as_deref(), Some("Test Tone"));
        assert_eq!(tag.artist().as_deref(), Some("Aplayer Lab"));

        // Verify zero leftover temporary wav/mp3 files remain.
        let leftover_tmp = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains(".aplayer_tmp_"));
        assert!(!leftover_tmp, "temporary files were not cleaned up");

        let _ = fs::remove_dir_all(&root);
    }
}
