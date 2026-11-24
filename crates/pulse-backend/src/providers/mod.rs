//! Multi-source streaming providers: YouTube (InnerTube) and SoundCloud (API v2).

pub mod soundcloud;
pub mod youtube;

use pulse_model::MediaSource;

/// Classifies an arbitrary user-pasted URL, URI, or local filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedInput {
    LocalPath(String),
    YouTubeVideo(String),
    YouTubePlaylist(String),
    SoundCloudUrl(String),
    SpotifyTrack(String),
    SpotifyPlaylist(String),
    SpotifyAlbum(String),
    SpotifyArtist(String),
}

pub fn parse_direct_input(raw: &str) -> Option<ResolvedInput> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(p) = s.strip_prefix("file://") {
        return Some(ResolvedInput::LocalPath(p.to_string()));
    }
    if s.starts_with('/') && std::path::Path::new(s).exists() {
        return Some(ResolvedInput::LocalPath(s.to_string()));
    }
    if let Some(vid) = s.strip_prefix("yt:video:") {
        return Some(ResolvedInput::YouTubeVideo(vid.to_string()));
    }
    if (s.contains("youtube.com/") || s.contains("youtu.be/") || s.contains("music.youtube.com/"))
        && let Ok(url) = url::Url::parse(s)
    {
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        if let Some(v) = pairs.get("v")
            && !v.is_empty()
        {
            return Some(ResolvedInput::YouTubeVideo(v.clone()));
        }
        if let Some(list) = pairs.get("list")
            && !list.is_empty()
        {
            return Some(ResolvedInput::YouTubePlaylist(list.clone()));
        }
        if url.host_str() == Some("youtu.be")
            && let Some(seg) = url.path_segments().and_then(|mut it| it.next())
            && !seg.is_empty()
        {
            return Some(ResolvedInput::YouTubeVideo(seg.to_string()));
        }
        let segs: Vec<&str> = url
            .path_segments()
            .map(|it| it.collect())
            .unwrap_or_default();
        if segs.len() >= 2 && (segs[0] == "shorts" || segs[0] == "embed" || segs[0] == "live") {
            return Some(ResolvedInput::YouTubeVideo(segs[1].to_string()));
        }
    }
    if s.starts_with("https://soundcloud.com/") || s.starts_with("https://on.soundcloud.com/") || s.starts_with("http://soundcloud.com/") {
        return Some(ResolvedInput::SoundCloudUrl(s.to_string()));
    }
    if let Some(rest) = s.strip_prefix("https://open.spotify.com/") {
        let clean = rest.split('?').next().unwrap_or(rest);
        let parts: Vec<&str> = clean.split('/').collect();
        if parts.len() >= 2 {
            match parts[0] {
                "track" => return Some(ResolvedInput::SpotifyTrack(format!("spotify:track:{}", parts[1]))),
                "playlist" => return Some(ResolvedInput::SpotifyPlaylist(format!("spotify:playlist:{}", parts[1]))),
                "album" => return Some(ResolvedInput::SpotifyAlbum(format!("spotify:album:{}", parts[1]))),
                "artist" => return Some(ResolvedInput::SpotifyArtist(format!("spotify:artist:{}", parts[1]))),
                _ => {}
            }
        }
    }
    if s.starts_with("spotify:track:") {
        return Some(ResolvedInput::SpotifyTrack(s.to_string()));
    }
    if s.starts_with("spotify:playlist:") {
        return Some(ResolvedInput::SpotifyPlaylist(s.to_string()));
    }
    if s.starts_with("spotify:album:") {
        return Some(ResolvedInput::SpotifyAlbum(s.to_string()));
    }
    if s.starts_with("spotify:artist:") {
        return Some(ResolvedInput::SpotifyArtist(s.to_string()));
    }
    None
}

pub fn source_index(src: MediaSource) -> usize {
    match src {
        MediaSource::Local => 0,
        MediaSource::Spotify => 1,
        MediaSource::YouTube => 2,
        MediaSource::SoundCloud => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_youtube_soundcloud_and_spotify_urls() {
        assert_eq!(
            parse_direct_input("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some(ResolvedInput::YouTubeVideo("dQw4w9WgXcQ".into()))
        );
        assert_eq!(
            parse_direct_input("https://youtu.be/dQw4w9WgXcQ?t=12"),
            Some(ResolvedInput::YouTubeVideo("dQw4w9WgXcQ".into()))
        );
        assert_eq!(
            parse_direct_input("https://music.youtube.com/playlist?list=PL12345"),
            Some(ResolvedInput::YouTubePlaylist("PL12345".into()))
        );
        assert_eq!(
            parse_direct_input("https://soundcloud.com/m83/midnight-city"),
            Some(ResolvedInput::SoundCloudUrl("https://soundcloud.com/m83/midnight-city".into()))
        );
        assert_eq!(
            parse_direct_input("https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC?si=xyz"),
            Some(ResolvedInput::SpotifyTrack("spotify:track:4uLU6hMCjMI75M1A2tKUQC".into()))
        );
    }
}
