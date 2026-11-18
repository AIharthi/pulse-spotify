//! Spotify account library, playlists, search, metadata resolution, and bounded disk cover cache.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::stream::{FuturesUnordered, StreamExt};
use http::{Request, header};
use protobuf::Message as _;
use pulse_model::{
    AlbumSummary, AppConfig, ArtistDetail, ArtistSummary, BackendEvent, CrateRef, ImageRef,
    PlaylistSummary, SearchResults, SortOrder, TrackDetail, TrackSummary,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spotatui_librespot_core::http_client::HttpClient;
use spotatui_librespot_core::spotify_uri::SpotifyUri;
use spotatui_librespot_core::{FileId, Session};
use spotatui_librespot_metadata::{Album, Artist, Metadata, Track};
use spotatui_librespot_protocol::collection2v2::CollectionItem;
use spotatui_librespot_protocol::playlist4_external::{
    ListAttributes as ProtoListAttributes, SelectedListContent as ProtoSelectedListContent,
};
use url::form_urlencoded;

use crate::auth::refresh_web_api_token;
use crate::config::{
    AppPaths, RecentHistory, WebApiToken, prune_dir_to_bytes_lru, save_web_api_token,
};
use crate::connect::image_ref_from_raw;
use crate::telemetry::dir_size_bytes;

const COLLECTION_PAGE_LIMIT: i32 = 200;
const METADATA_CONCURRENCY: usize = 10;
const MAX_TRACK_CACHE: usize = 2048;
const MAX_ALBUM_CACHE: usize = 512;
/// Persisted GraphQL query SHA256 hash used by Spotify Web Player `searchDesktop`.
const SEARCH_DESKTOP_HASH: &str =
    "700b65e8a35017c17a9bd3f44483a73b8f35747be119c8786d573bb18d1a8d5a";

#[derive(Debug, Clone)]
struct CollectionStub {
    uri: String,
    added_at: i32,
}

/// Stateful library cache and Spotify API client wrapper for one session.
pub struct LibraryService {
    liked_tracks: Option<Vec<CollectionStub>>,
    liked_uri_set: HashSet<String>,
    saved_albums: Option<Vec<CollectionStub>>,
    followed_artists: Option<Vec<CollectionStub>>,
    playlists_index: Option<Vec<PlaylistSummary>>,
    track_cache: HashMap<String, TrackSummary>,
    track_cache_order: VecDeque<String>,
    album_cache: HashMap<String, AlbumSummary>,
    album_cache_order: VecDeque<String>,
}

impl Default for LibraryService {
    fn default() -> Self {
        Self::new()
    }
}

fn insert_bounded_cache<V>(
    map: &mut HashMap<String, V>,
    order: &mut VecDeque<String>,
    max_len: usize,
    key: String,
    value: V,
) {
    if let Some(slot) = map.get_mut(&key) {
        *slot = value;
        return;
    }
    while map.len() >= max_len {
        if let Some(evict) = order.pop_front() {
            map.remove(&evict);
        } else {
            break;
        }
    }
    order.push_back(key.clone());
    map.insert(key, value);
}

impl LibraryService {
    /// Creates an empty library cache.
    pub fn new() -> Self {
        Self {
            liked_tracks: None,
            liked_uri_set: HashSet::new(),
            saved_albums: None,
            followed_artists: None,
            playlists_index: None,
            track_cache: HashMap::new(),
            track_cache_order: VecDeque::new(),
            album_cache: HashMap::new(),
            album_cache_order: VecDeque::new(),
        }
    }

    /// Invalidates cached library indices (e.g. after sign-out or collection updates).
    pub fn invalidate(&mut self) {
        self.liked_tracks = None;
        self.liked_uri_set.clear();
        self.saved_albums = None;
        self.followed_artists = None;
        self.playlists_index = None;
        self.track_cache.clear();
        self.track_cache_order.clear();
        self.album_cache.clear();
        self.album_cache_order.clear();
    }

    /// Returns cached liked URI status if known.
    pub fn is_liked_cached(&self, uri: &str) -> bool {
        self.liked_uri_set.contains(uri)
    }

    fn cache_track_summary(&mut self, summary: TrackSummary) {
        let key = summary.uri.to_string();
        insert_bounded_cache(
            &mut self.track_cache,
            &mut self.track_cache_order,
            MAX_TRACK_CACHE,
            key,
            summary,
        );
    }

    fn cache_album_summary(&mut self, summary: AlbumSummary) {
        let key = summary.uri.to_string();
        insert_bounded_cache(
            &mut self.album_cache,
            &mut self.album_cache_order,
            MAX_ALBUM_CACHE,
            key,
            summary,
        );
    }

    async fn resolve_track_summaries_cached(
        &mut self,
        session: &Session,
        stubs: &[CollectionStub],
    ) -> Vec<TrackSummary> {
        let mut out: Vec<Option<TrackSummary>> = vec![None; stubs.len()];
        let mut missing_positions = Vec::new();
        let mut missing_stubs = Vec::new();

        for (idx, stub) in stubs.iter().enumerate() {
            if let Some(cached) = self.track_cache.get(&stub.uri) {
                let mut item = cached.clone();
                item.is_liked = self.liked_uri_set.contains(&stub.uri);
                if stub.added_at > 0 {
                    item.added_at_unix = Some(stub.added_at as i64);
                }
                out[idx] = Some(item);
            } else {
                missing_positions.push(idx);
                missing_stubs.push(stub.clone());
            }
        }

        if !missing_stubs.is_empty() {
            let resolved =
                resolve_track_summaries_indexed(session, &missing_stubs, &self.liked_uri_set)
                    .await;
            for (pos, maybe_item) in missing_positions.into_iter().zip(resolved) {
                if let Some(item) = maybe_item {
                    self.cache_track_summary(item.clone());
                    out[pos] = Some(item);
                }
            }
        }

        out.into_iter().flatten().collect()
    }

    async fn resolve_album_summaries_cached(
        &mut self,
        session: &Session,
        uris: &[String],
    ) -> Vec<AlbumSummary> {
        let mut out: Vec<Option<AlbumSummary>> = vec![None; uris.len()];
        let mut missing_positions = Vec::new();
        let mut missing_uris = Vec::new();

        for (idx, uri) in uris.iter().enumerate() {
            if let Some(cached) = self.album_cache.get(uri) {
                out[idx] = Some(cached.clone());
            } else {
                missing_positions.push(idx);
                missing_uris.push(uri.clone());
            }
        }

        if !missing_uris.is_empty() {
            let resolved = resolve_album_summaries_indexed(session, &missing_uris).await;
            for (pos, maybe_item) in missing_positions.into_iter().zip(resolved) {
                if let Some(item) = maybe_item {
                    self.cache_album_summary(item.clone());
                    out[pos] = Some(item);
                }
            }
        }

        out.into_iter().flatten().collect()
    }

    /// Returns ordered liked track URIs according to `sort` (up to `limit`).
    pub async fn liked_track_uris(
        &mut self,
        session: &Session,
        sort: SortOrder,
        limit: usize,
    ) -> Result<Vec<String>, String> {
        self.ensure_collection_loaded(session).await?;
        let mut stubs = self.liked_tracks.clone().unwrap_or_default();
        let _ = sort;
        stubs.sort_by_key(|a| std::cmp::Reverse(a.added_at));
        Ok(stubs.into_iter().take(limit).map(|s| s.uri).collect())
    }

    /// Fetches display name for the current user using `SpClient::get_user_profile`.
    pub async fn fetch_display_name(&self, session: &Session, username: &str) -> Option<String> {
        let bytes = session
            .spclient()
            .get_user_profile(username, None, None)
            .await
            .ok()?;
        let val: Value = serde_json::from_slice(&bytes).ok()?;
        val.get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
    }

    async fn ensure_collection_loaded(&mut self, session: &Session) -> Result<(), String> {
        if self.liked_tracks.is_some() && self.saved_albums.is_some() {
            return Ok(());
        }

        let mut tracks = Vec::new();
        let mut albums = Vec::new();
        let mut uri_set = HashSet::new();
        let mut token = String::new();

        for _ in 0..100 {
            let page = session
                .spclient()
                .get_collection_page("collection", &token, COLLECTION_PAGE_LIMIT)
                .await
                .map_err(|e| format!("failed to fetch Spotify collection: {e}"))?;

            for item in &page.items {
                if item.is_removed || item.uri.is_empty() {
                    continue;
                }
                uri_set.insert(item.uri.clone());
                let stub = CollectionStub {
                    uri: item.uri.clone(),
                    added_at: item.added_at,
                };
                if item.uri.starts_with("spotify:track:") {
                    tracks.push(stub);
                } else if item.uri.starts_with("spotify:album:") {
                    albums.push(stub);
                }
            }

            if page.next_page_token.is_empty() {
                break;
            }
            token = page.next_page_token;
        }

        self.liked_uri_set = uri_set;
        self.liked_tracks = Some(tracks);
        self.saved_albums = Some(albums);
        Ok(())
    }

    async fn ensure_artists_loaded(&mut self, session: &Session) -> Result<(), String> {
        if self.followed_artists.is_some() {
            return Ok(());
        }

        let mut artists = Vec::new();
        let mut token = String::new();

        for _ in 0..50 {
            let page = session
                .spclient()
                .get_collection_page("artist", &token, COLLECTION_PAGE_LIMIT)
                .await
                .map_err(|e| format!("failed to fetch followed artists: {e}"))?;

            for item in &page.items {
                if item.is_removed || !item.uri.starts_with("spotify:artist:") {
                    continue;
                }
                artists.push(CollectionStub {
                    uri: item.uri.clone(),
                    added_at: item.added_at,
                });
            }

            if page.next_page_token.is_empty() {
                break;
            }
            token = page.next_page_token;
        }

        self.followed_artists = Some(artists);
        Ok(())
    }

    /// Loads a paged slice for any [`CrateRef`] and emits matching [`BackendEvent`]s.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_crate_page(
        &mut self,
        session: &Session,
        paths: &AppPaths,
        web_token: &mut Option<WebApiToken>,
        recent: &RecentHistory,
        crate_ref: &CrateRef,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        match crate_ref {
            CrateRef::LikedSongs => {
                self.load_liked_songs(session, offset, limit, sort).await
            }
            CrateRef::PlaylistsIndex => self.load_playlists(session, offset, limit, sort).await,
            CrateRef::AlbumsIndex => self.load_saved_albums(session, offset, limit, sort).await,
            CrateRef::ArtistsIndex => self.load_followed_artists(session, offset, limit).await,
            CrateRef::Recent => {
                self.load_recent(session, paths, web_token, recent, offset, limit, sort)
                    .await
            }
            CrateRef::Playlist { uri, .. } => {
                self.load_playlist_detail(session, crate_ref, uri, offset, limit, sort)
                    .await
            }
            CrateRef::Album { uri, .. } => {
                self.load_album_detail(session, crate_ref, uri, offset, limit, sort)
                    .await
            }
            CrateRef::Artist { uri, .. } => {
                let detail = self.load_artist_detail(session, uri).await?;
                Ok(vec![BackendEvent::ArtistDetailLoaded(detail)])
            }
            CrateRef::Search { query } => {
                let results = self.search(session, paths, web_token, query).await?;
                Ok(vec![BackendEvent::SearchResultsLoaded(results)])
            }
        }
    }

    async fn load_liked_songs(
        &mut self,
        session: &Session,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        self.ensure_collection_loaded(session).await?;
        let mut stubs = self.liked_tracks.clone().unwrap_or_default();
        stubs.sort_by_key(|a| std::cmp::Reverse(a.added_at));

        let total = stubs.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);

        let items = if matches!(sort, SortOrder::Title | SortOrder::Artist | SortOrder::Duration) {
            let mut all_items = self.resolve_track_summaries_cached(session, &stubs).await;
            sort_track_summaries(&mut all_items, sort);
            let s = offset.min(all_items.len());
            let e = s.saturating_add(limit.max(1)).min(all_items.len());
            all_items[s..e].to_vec()
        } else {
            self.resolve_track_summaries_cached(session, &stubs[start..end])
                .await
        };

        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: CrateRef::LikedSongs,
                title: "Liked Songs".to_string(),
                subtitle: format!("{total} songs"),
                cover: None,
                total,
            });
        }
        events.push(BackendEvent::CrateTracksPageLoaded {
            crate_ref: CrateRef::LikedSongs,
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    async fn load_saved_albums(
        &mut self,
        session: &Session,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        self.ensure_collection_loaded(session).await?;
        let mut stubs = self.saved_albums.clone().unwrap_or_default();
        stubs.sort_by_key(|a| std::cmp::Reverse(a.added_at));
        let total = stubs.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);

        let items = if matches!(sort, SortOrder::Title | SortOrder::Artist | SortOrder::Duration) {
            let all_uris: Vec<String> = stubs.iter().map(|s| s.uri.clone()).collect();
            let mut all_items = self.resolve_album_summaries_cached(session, &all_uris).await;
            sort_album_summaries(&mut all_items, sort);
            let s = offset.min(all_items.len());
            let e = s.saturating_add(limit.max(1)).min(all_items.len());
            all_items[s..e].to_vec()
        } else {
            let slice_uris: Vec<String> = stubs[start..end].iter().map(|s| s.uri.clone()).collect();
            self.resolve_album_summaries_cached(session, &slice_uris).await
        };

        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: CrateRef::AlbumsIndex,
                title: "Albums".to_string(),
                subtitle: format!("{total} albums"),
                cover: None,
                total,
            });
        }
        events.push(BackendEvent::AlbumsPageLoaded {
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    async fn load_followed_artists(
        &mut self,
        session: &Session,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<BackendEvent>, String> {
        self.ensure_artists_loaded(session).await?;
        let stubs = self.followed_artists.as_deref().unwrap_or(&[]);
        let total = stubs.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);
        let slice_uris: Vec<String> = stubs[start..end].iter().map(|s| s.uri.clone()).collect();

        let items = resolve_artist_summaries(session, &slice_uris, true).await;
        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: CrateRef::ArtistsIndex,
                title: "Artists".to_string(),
                subtitle: format!("{total} followed artists"),
                cover: None,
                total,
            });
        }
        events.push(BackendEvent::ArtistsPageLoaded {
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    async fn load_playlists(
        &mut self,
        session: &Session,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        if self.playlists_index.is_none() {
            let rootlist = session
                .spclient()
                .get_rootlist(0, Some(200))
                .await
                .map_err(|e| format!("failed to fetch playlists rootlist: {e}"))?;

            let proto_list = ProtoSelectedListContent::parse_from_bytes(&rootlist)
                .map_err(|e| format!("failed to parse rootlist protobuf: {e}"))?;

            let mut stubs = Vec::new();
            let mut seen = HashSet::new();
            for (idx, item) in proto_list.contents.items.iter().enumerate() {
                let Some(uri) = normalize_playlist_uri(item.uri()) else {
                    continue;
                };
                if !seen.insert(uri.clone()) {
                    continue;
                }
                let meta = proto_list.contents.meta_items.get(idx);
                let (name, owner, track_count, cover, is_collaborative) = match meta {
                    Some(m) => {
                        let attrs = m.attributes.get_or_default();
                        (
                            attrs.name().trim().to_string(),
                            m.owner_username().trim().to_string(),
                            m.length().max(0) as usize,
                            proto_playlist_cover(attrs),
                            attrs.collaborative(),
                        )
                    }
                    None => (String::new(), String::new(), 0, None, false),
                };
                stubs.push(PlaylistSummary {
                    uri: Arc::from(uri.as_str()),
                    name,
                    owner: Arc::from(owner.as_str()),
                    track_count,
                    total_duration_ms: 0,
                    cover,
                    is_collaborative,
                });
            }
            self.playlists_index = Some(stubs);
        }

        let all = self.playlists_index.as_mut().unwrap();
        let total = all.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);

        let resolve_all = matches!(sort, SortOrder::Title | SortOrder::Artist);
        let check_range = if resolve_all { 0..total } else { start..end };

        let missing_indices: Vec<(usize, String)> = all[check_range.clone()]
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name.is_empty())
            .map(|(rel, p)| (check_range.start + rel, p.uri.to_string()))
            .collect();

        if !missing_indices.is_empty() {
            let uris: Vec<String> = missing_indices.iter().map(|(_, u)| u.clone()).collect();
            let resolved = resolve_playlist_summaries(session, &uris).await;
            let mut by_uri: HashMap<String, PlaylistSummary> = resolved
                .into_iter()
                .map(|p| (p.uri.to_string(), p))
                .collect();
            for (idx, uri) in missing_indices {
                if let Some(pl) = by_uri.remove(&uri) {
                    all[idx] = pl;
                } else {
                    all[idx].name = uri;
                }
            }
        }

        let items = if resolve_all {
            let mut sorted = all.clone();
            match sort {
                SortOrder::Title => sorted.sort_by_key(|p| p.name.to_lowercase()),
                SortOrder::Artist => sorted.sort_by_key(|p| p.owner.to_lowercase()),
                _ => {}
            }
            sorted[start..end].to_vec()
        } else {
            all[start..end].to_vec()
        };

        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: CrateRef::PlaylistsIndex,
                title: "Playlists".to_string(),
                subtitle: format!("{total} playlists"),
                cover: None,
                total,
            });
        }
        events.push(BackendEvent::PlaylistsPageLoaded {
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    async fn load_playlist_detail(
        &mut self,
        session: &Session,
        crate_ref: &CrateRef,
        uri_str: &str,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        let _ = self.ensure_collection_loaded(session).await;
        let playlist = fetch_proto_playlist(session, uri_str).await?;
        let all_stubs = playlist_track_stubs(&playlist, sort);

        let total = all_stubs.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);

        let items = if matches!(sort, SortOrder::Title | SortOrder::Artist | SortOrder::Duration) {
            let mut all_items = self.resolve_track_summaries_cached(session, &all_stubs).await;
            sort_track_summaries(&mut all_items, sort);
            let s = offset.min(all_items.len());
            let e = s.saturating_add(limit.max(1)).min(all_items.len());
            all_items[s..e].to_vec()
        } else {
            self.resolve_track_summaries_cached(session, &all_stubs[start..end])
                .await
        };

        let attrs = playlist.attributes.get_or_default();
        let cover = proto_playlist_cover(attrs);
        let mut events = Vec::new();
        if start == 0 {
            let owner = playlist.owner_username().trim();
            let owner_display = if owner.is_empty() { "Spotify" } else { owner };
            let subtitle = format!("{owner_display} • {total} tracks");
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: crate_ref.clone(),
                title: attrs.name().to_string(),
                subtitle,
                cover,
                total,
            });
        }
        events.push(BackendEvent::CrateTracksPageLoaded {
            crate_ref: crate_ref.clone(),
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    async fn load_album_detail(
        &mut self,
        session: &Session,
        crate_ref: &CrateRef,
        uri_str: &str,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        let _ = self.ensure_collection_loaded(session).await;
        let uri = SpotifyUri::from_uri(uri_str).map_err(|e| format!("invalid album URI: {e}"))?;
        let album = Album::get(session, &uri)
            .await
            .map_err(|e| format!("failed to load album {uri_str}: {e}"))?;

        let mut all_stubs = Vec::new();
        for disc in &album.discs.0 {
            for t in &disc.tracks.0 {
                all_stubs.push(CollectionStub {
                    uri: t.to_uri(),
                    added_at: 0,
                });
            }
        }
        let total = all_stubs.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);
        let items = if matches!(sort, SortOrder::Title | SortOrder::Artist | SortOrder::Duration) {
            let mut all_items = self.resolve_track_summaries_cached(session, &all_stubs).await;
            sort_track_summaries(&mut all_items, sort);
            let s = offset.min(all_items.len());
            let e = s.saturating_add(limit.max(1)).min(all_items.len());
            all_items[s..e].to_vec()
        } else {
            self.resolve_track_summaries_cached(session, &all_stubs[start..end])
                .await
        };

        let artist_names: Vec<&str> = album.artists.0.iter().map(|a| a.name.as_str()).collect();
        let year = album.date.0.year();
        let subtitle = if artist_names.is_empty() {
            format!("{year} • {total} tracks")
        } else {
            format!("{} • {year} • {total} tracks", artist_names.join(", "))
        };
        let cover = pick_image_ref(&album.covers);

        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: crate_ref.clone(),
                title: album.name,
                subtitle,
                cover,
                total,
            });
        }
        events.push(BackendEvent::CrateTracksPageLoaded {
            crate_ref: crate_ref.clone(),
            offset: start,
            total,
            items,
        });
        Ok(events)
    }

    /// Loads artist profile, top tracks, albums, and singles.
    pub async fn load_artist_detail(
        &mut self,
        session: &Session,
        uri_str: &str,
    ) -> Result<ArtistDetail, String> {
        let _ = self.ensure_collection_loaded(session).await;
        let uri = SpotifyUri::from_uri(uri_str).map_err(|e| format!("invalid artist URI: {e}"))?;
        let artist = Artist::get(session, &uri)
            .await
            .map_err(|e| format!("failed to load artist {uri_str}: {e}"))?;

        let country = session.country();
        let top_stubs: Vec<CollectionStub> = artist
            .top_tracks
            .for_country(&country)
            .0
            .into_iter()
            .take(10)
            .map(|u| CollectionStub {
                uri: u.to_uri(),
                added_at: 0,
            })
            .collect();

        let album_uris: Vec<String> = artist
            .albums_current()
            .take(12)
            .map(|u| u.to_uri())
            .collect();
        let single_uris: Vec<String> = artist
            .singles_current()
            .take(12)
            .map(|u| u.to_uri())
            .collect();

        let (top_tracks, albums, singles, is_followed) = tokio::join!(
            resolve_track_summaries(session, &top_stubs, &self.liked_uri_set),
            resolve_album_summaries(session, &album_uris),
            resolve_album_summaries(session, &single_uris),
            async {
                session
                    .spclient()
                    .collection_contains("artist", &[uri_str])
                    .await
                    .ok()
                    .and_then(|v| v.first().copied())
                    .unwrap_or(false)
            }
        );

        let cover = pick_image_ref(&artist.portraits);
        let summary = ArtistSummary {
            uri: Arc::from(uri_str),
            name: artist.name,
            cover,
            is_followed,
        };

        Ok(ArtistDetail {
            artist: summary,
            top_tracks,
            albums,
            singles,
        })
    }

    /// Toggles liked status (`"collection"` set) for a track or album URI and returns new `is_liked`.
    pub async fn toggle_like(&mut self, session: &Session, uri_str: &str) -> Result<bool, String> {
        let currently_liked = if self.liked_uri_set.contains(uri_str) {
            true
        } else {
            session
                .spclient()
                .collection_contains("collection", &[uri_str])
                .await
                .ok()
                .and_then(|v| v.first().copied())
                .unwrap_or(false)
        };
        let target_liked = !currently_liked;
        write_collection_item(session, "collection", uri_str, target_liked).await?;

        if target_liked {
            self.liked_uri_set.insert(uri_str.to_string());
        } else {
            self.liked_uri_set.remove(uri_str);
        }
        if uri_str.starts_with("spotify:track:") {
            if let Some(ref mut tracks) = self.liked_tracks {
                if target_liked {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i32;
                    tracks.retain(|t| t.uri != uri_str);
                    tracks.insert(
                        0,
                        CollectionStub {
                            uri: uri_str.to_string(),
                            added_at: now,
                        },
                    );
                } else {
                    tracks.retain(|t| t.uri != uri_str);
                }
            }
        } else if uri_str.starts_with("spotify:album:") {
            self.saved_albums = None;
        }
        Ok(target_liked)
    }

    /// Toggles follow status (`"artist"` set) for an artist URI.
    pub async fn toggle_follow_artist(
        &mut self,
        session: &Session,
        uri_str: &str,
        followed: bool,
    ) -> Result<(), String> {
        write_collection_item(session, "artist", uri_str, followed).await?;
        self.followed_artists = None;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn load_recent(
        &mut self,
        session: &Session,
        paths: &AppPaths,
        web_token: &mut Option<WebApiToken>,
        local_recent: &RecentHistory,
        offset: usize,
        limit: usize,
        sort: SortOrder,
    ) -> Result<Vec<BackendEvent>, String> {
        let _ = self.ensure_collection_loaded(session).await;
        let mut items: Vec<TrackSummary> = Vec::new();
        let mut seen = HashSet::new();

        if let Some(token) = web_token.as_mut()
            && ensure_fresh_web_token(session.http_client(), paths, token)
                .await
                .is_ok()
            && let Ok(val) = web_api_get(
                session.http_client(),
                &token.access_token,
                "https://api.spotify.com/v1/me/player/recently-played?limit=50",
            )
            .await
            && let Some(arr) = val.get("items").and_then(|v| v.as_array())
        {
            for item in arr {
                let Some(track) = item.get("track") else {
                    continue;
                };
                if let Some(ts) = track_summary_from_web_api(track, &self.liked_uri_set)
                    && seen.insert(ts.uri.to_string())
                {
                    items.push(ts);
                }
            }
        }

        // Supplement with local playback history persisted across runs.
        for entry in &local_recent.entries {
            if entry.uri.starts_with("spotify:track:") && seen.insert(entry.uri.clone()) {
                items.push(TrackSummary {
                    uri: Arc::from(entry.uri.as_str()),
                    title: entry.title.clone(),
                    artist: Arc::from(entry.subtitle.as_str()),
                    artist_uri: None,
                    album: Arc::from(""),
                    album_uri: None,
                    cover: entry.cover_url.as_deref().and_then(image_ref_from_raw),
                    duration_ms: entry.duration_ms,
                    is_liked: self.liked_uri_set.contains(&entry.uri),
                    is_playable: true,
                    added_at_unix: Some(entry.played_at_secs as i64),
                });
            }
        }

        if matches!(sort, SortOrder::Title | SortOrder::Artist | SortOrder::Duration) {
            sort_track_summaries(&mut items, sort);
        }

        let total = items.len();
        let start = offset.min(total);
        let end = start.saturating_add(limit.max(1)).min(total);
        let slice = items[start..end].to_vec();

        let mut events = Vec::new();
        if start == 0 {
            events.push(BackendEvent::CrateHeaderLoaded {
                crate_ref: CrateRef::Recent,
                title: "Recently Played".to_string(),
                subtitle: format!("{total} tracks"),
                cover: None,
                total,
            });
        }
        events.push(BackendEvent::CrateTracksPageLoaded {
            crate_ref: CrateRef::Recent,
            offset: start,
            total,
            items: slice,
        });
        Ok(events)
    }

    /// Searches Spotify using (1) Web API `/v1/search` if custom Web API token is ready,
    /// otherwise (2) Spotify internal `pathfinder_query` or `get_context("spotify:search:...")`.
    pub async fn search(
        &mut self,
        session: &Session,
        paths: &AppPaths,
        web_token: &mut Option<WebApiToken>,
        query: &str,
    ) -> Result<SearchResults, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(SearchResults::default());
        }
        let _ = self.ensure_collection_loaded(session).await;

        if let Some(token) = web_token.as_mut()
            && ensure_fresh_web_token(session.http_client(), paths, token)
                .await
                .is_ok()
        {
            let encoded = form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
            let url = format!(
                "https://api.spotify.com/v1/search?q={encoded}&type=track,album,artist,playlist&limit=15"
            );
            if let Ok(json) = web_api_get(session.http_client(), &token.access_token, &url).await {
                return Ok(parse_web_api_search(&json, trimmed, &self.liked_uri_set));
            }
        }

        let vars = json!({
            "searchTerm": trimmed,
            "offset": 0,
            "limit": 15,
            "numberOfTopResults": 5,
            "includeAudiobooks": false,
            "includePreReleases": false
        });
        if let Ok(val) = session
            .spclient()
            .pathfinder_query("searchDesktop", SEARCH_DESKTOP_HASH, vars)
            .await
            && let Some(results) = parse_pathfinder_search(&val, trimmed, &self.liked_uri_set)
            && (!results.tracks.is_empty()
                || !results.albums.is_empty()
                || !results.artists.is_empty()
                || !results.playlists.is_empty())
        {
            return Ok(results);
        }

        // Secondary librespot search context resolution (`spotify:search:<term>`).
        let encoded = form_urlencoded::byte_serialize(trimmed.as_bytes()).collect::<String>();
        let search_uri = format!("spotify:search:{encoded}");
        if let Ok(ctx) = session.spclient().get_context(&search_uri).await {
            let mut stubs = Vec::new();
            for page in &ctx.pages {
                for t in &page.tracks {
                    if let Some(ref u) = t.uri
                        && u.starts_with("spotify:track:")
                    {
                        stubs.push(CollectionStub {
                            uri: u.clone(),
                            added_at: 0,
                        });
                        if stubs.len() >= 25 {
                            break;
                        }
                    }
                }
                if stubs.len() >= 25 {
                    break;
                }
            }
            let tracks = resolve_track_summaries(session, &stubs, &self.liked_uri_set).await;
            let top_match_uri = tracks.first().map(|t| t.uri.clone());
            return Ok(SearchResults {
                query: trimmed.to_string(),
                top_match_uri,
                artists: Vec::new(),
                albums: Vec::new(),
                playlists: Vec::new(),
                tracks,
                tracks_only_fallback: true,
            });
        }

        Err("Spotify search is unavailable right now; check connection or configure a Web API Client ID in Settings.".to_string())
    }

    /// Resolves track summaries for starting/enqueuing playback of a [`CrateRef`].
    pub async fn resolve_crate_tracks(
        &mut self,
        session: &Session,
        crate_ref: &CrateRef,
        recent: &RecentHistory,
        sort: SortOrder,
    ) -> Result<(String, Option<String>, Vec<TrackSummary>), String> {
        let _ = self.ensure_collection_loaded(session).await;
        match crate_ref {
            CrateRef::LikedSongs => {
                let uris = self.liked_track_uris(session, sort, usize::MAX).await?;
                let stubs: Vec<CollectionStub> = uris
                    .into_iter()
                    .map(|uri| CollectionStub { uri, added_at: 0 })
                    .collect();
                let mut tracks = self.resolve_track_summaries_cached(session, &stubs).await;
                sort_track_summaries(&mut tracks, sort);
                Ok(("Liked Songs".to_string(), None, tracks))
            }
            CrateRef::Recent => {
                let stubs: Vec<CollectionStub> = recent
                    .entries
                    .iter()
                    .filter(|e| e.uri.starts_with("spotify:track:"))
                    .map(|e| CollectionStub {
                        uri: e.uri.clone(),
                        added_at: e.played_at_secs.min(i32::MAX as u64) as i32,
                    })
                    .collect();
                let mut tracks = self.resolve_track_summaries_cached(session, &stubs).await;
                sort_track_summaries(&mut tracks, sort);
                Ok(("Recently Played".to_string(), None, tracks))
            }
            CrateRef::Playlist { uri, .. } => {
                let pl = fetch_proto_playlist(session, uri).await?;
                let title = pl.attributes.get_or_default().name().to_string();
                let stubs = playlist_track_stubs(&pl, sort);
                let mut tracks = self.resolve_track_summaries_cached(session, &stubs).await;
                sort_track_summaries(&mut tracks, sort);
                Ok((title, Some(uri.to_string()), tracks))
            }
            CrateRef::Album { uri, .. } => {
                let parsed =
                    SpotifyUri::from_uri(uri).map_err(|e| format!("invalid album URI: {e}"))?;
                let album = Album::get(session, &parsed)
                    .await
                    .map_err(|e| format!("failed to load album {uri}: {e}"))?;
                let title = album.name.clone();
                let mut stubs = Vec::new();
                for disc in &album.discs.0 {
                    for t in &disc.tracks.0 {
                        stubs.push(CollectionStub {
                            uri: t.to_uri(),
                            added_at: 0,
                        });
                    }
                }
                let mut tracks = self.resolve_track_summaries_cached(session, &stubs).await;
                sort_track_summaries(&mut tracks, sort);
                Ok((title, Some(uri.to_string()), tracks))
            }
            CrateRef::Artist { uri, .. } => {
                let parsed =
                    SpotifyUri::from_uri(uri).map_err(|e| format!("invalid artist URI: {e}"))?;
                let artist = Artist::get(session, &parsed)
                    .await
                    .map_err(|e| format!("failed to load artist {uri}: {e}"))?;
                let title = artist.name.clone();
                let stubs: Vec<CollectionStub> = artist
                    .top_tracks
                    .for_country(&session.country())
                    .0
                    .into_iter()
                    .take(20)
                    .map(|u| CollectionStub {
                        uri: u.to_uri(),
                        added_at: 0,
                    })
                    .collect();
                let tracks = self.resolve_track_summaries_cached(session, &stubs).await;
                Ok((title, Some(uri.to_string()), tracks))
            }
            _ => Err("selected crate cannot be played directly".to_string()),
        }
    }

    /// Resolves rich [`TrackDetail`] for the currently playing track URI.
    pub async fn resolve_track_detail(
        &self,
        session: &Session,
        uri_str: &str,
    ) -> Option<TrackDetail> {
        let uri = SpotifyUri::from_uri(uri_str).ok()?;
        let track = Track::get(session, &uri).await.ok()?;
        let album = Album::get(session, &track.album.id).await.ok();

        let cover = album
            .as_ref()
            .and_then(|a| pick_image_ref(&a.covers))
            .or_else(|| pick_image_ref(&track.album.covers));
        let release_year = album
            .as_ref()
            .map(|a| a.date.0.year() as u16)
            .or_else(|| Some(track.album.date.0.year() as u16));
        let disc_total_tracks = album.as_ref().and_then(|a| {
            a.discs
                .0
                .iter()
                .find(|d| d.number == track.disc_number)
                .map(|d| d.tracks.0.len() as u32)
        });

        let artist_names = track
            .artists
            .0
            .iter()
            .map(|a| a.name.as_str())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let artist_uri = track
            .artists
            .0
            .first()
            .map(|a| Arc::from(a.id.to_uri().as_str()));
        let album_name = album
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or(&track.album.name);

        let explicit = track.content_ratings.0.iter().any(|r| {
            r.tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case("explicit"))
        });

        Some(TrackDetail {
            summary: TrackSummary {
                uri: Arc::from(uri_str),
                title: track.name,
                artist: Arc::from(artist_names.as_str()),
                artist_uri,
                album: Arc::from(album_name),
                album_uri: Some(Arc::from(track.album.id.to_uri().as_str())),
                cover,
                duration_ms: track.duration.clamp(0, i32::MAX) as u32,
                is_liked: self.liked_uri_set.contains(uri_str),
                is_playable: true,
                added_at_unix: None,
            },
            track_number: (track.number > 0).then_some(track.number as u32),
            disc_total_tracks,
            release_year,
            explicit,
        })
    }
}

/// Fetches cover image bytes with bounded on-disk cache (`disk_cover_cache_mb`).
pub async fn fetch_cover_bytes(
    paths: &AppPaths,
    config: &AppConfig,
    http: &HttpClient,
    session: Option<&Session>,
    url_or_id: &str,
) -> Result<Vec<u8>, String> {
    let trimmed = url_or_id.trim();
    if trimmed.is_empty() {
        return Err("empty cover identifier".to_string());
    }

    let cache_enabled = config.disk_cover_cache_mb > 0;
    let cache_path = cover_disk_path(paths, trimmed);
    if cache_enabled && let Ok(bytes) = fs::read(&cache_path) {
        return Ok(bytes);
    }

    let bytes = if let Some(image_hex) = extract_spotify_image_id(trimmed) {
        if let Some(sess) = session
            && let Some(raw_id) = decode_hex(&image_hex)
            && let Ok(raw) = sess.spclient().get_image(&FileId::from_raw(&raw_id)).await
        {
            raw.to_vec()
        } else {
            http_get_bytes(http, &format!("https://i.scdn.co/image/{image_hex}")).await?
        }
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        http_get_bytes(http, trimmed).await?
    } else {
        return Err(format!("unsupported cover URL/ID: {trimmed}"));
    };

    if cache_enabled {
        let _ = write_cover_disk_cache(paths, config.disk_cover_cache_mb, &cache_path, &bytes);
    }
    Ok(bytes)
}

async fn http_get_bytes(http: &HttpClient, url: &str) -> Result<Vec<u8>, String> {
    let req = Request::get(url)
        .body(Bytes::new())
        .map_err(|e| format!("invalid image request URL {url}: {e}"))?;
    let body = http
        .request_body(req)
        .await
        .map_err(|e| format!("failed to fetch cover {url}: {e}"))?;
    Ok(body.to_vec())
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let s = hex.get(i..i + 2)?;
        let byte = u8::from_str_radix(s, 16).ok()?;
        out.push(byte);
    }
    Some(out)
}

fn cover_disk_path(paths: &AppPaths, key: &str) -> PathBuf {
    let digest = Sha256::digest(key.as_bytes());
    let mut name = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(name, "{b:02x}");
    }
    paths.cover_cache_dir.join(format!("{name}.img"))
}

fn write_cover_disk_cache(
    paths: &AppPaths,
    limit_mb: u64,
    target: &PathBuf,
    bytes: &[u8],
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.cover_cache_dir)?;
    let limit_bytes = limit_mb.saturating_mul(1024 * 1024);
    if bytes.len() as u64 > limit_bytes {
        return Ok(());
    }
    if dir_size_bytes(&paths.cover_cache_dir).saturating_add(bytes.len() as u64) > limit_bytes {
        prune_dir_to_bytes_lru(
            &paths.cover_cache_dir,
            limit_bytes.saturating_sub(bytes.len() as u64),
        );
    }
    fs::write(target, bytes)
}

fn extract_spotify_image_id(raw: &str) -> Option<String> {
    if let Some(hex) = raw.strip_prefix("spotify:image:") {
        return Some(hex.to_string());
    }
    if let Some(hex) = raw
        .strip_prefix("https://i.scdn.co/image/")
        .or_else(|| raw.strip_prefix("http://i.scdn.co/image/"))
    {
        return Some(hex.split('?').next().unwrap_or(hex).to_string());
    }
    if raw.len() >= 16 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(raw.to_string());
    }
    None
}

async fn write_collection_item(
    session: &Session,
    set: &str,
    uri: &str,
    keep: bool,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i32;
    let mut item = CollectionItem::new();
    item.uri = uri.to_string();
    item.added_at = now;
    item.is_removed = !keep;

    session
        .spclient()
        .write_collection(set, vec![item])
        .await
        .map_err(|e| format!("failed to update Spotify {set}: {e}"))?;
    Ok(())
}

async fn ensure_fresh_web_token(
    http: &HttpClient,
    paths: &AppPaths,
    token: &mut WebApiToken,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now.saturating_add(60) >= token.expires_at_epoch_secs {
        let refreshed = refresh_web_api_token(http, token).await?;
        save_web_api_token(paths, &refreshed).map_err(|e| e.to_string())?;
        *token = refreshed;
    }
    Ok(())
}

async fn web_api_get(http: &HttpClient, access_token: &str, url: &str) -> Result<Value, String> {
    let req = Request::get(url)
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .body(Bytes::new())
        .map_err(|e| format!("failed to build Web API request: {e}"))?;
    let body = http
        .request_body(req)
        .await
        .map_err(|e| format!("Spotify Web API call failed: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("invalid Spotify Web API JSON: {e}"))
}

/// Sorts resolved tracks uniformly across Liked Songs, Playlists, Albums, and Recent.
pub(crate) fn sort_track_summaries(items: &mut [TrackSummary], sort: SortOrder) {
    match sort {
        SortOrder::Title => {
            items.sort_by_key(|a| (a.title.to_lowercase(), a.artist.to_lowercase()));
        }
        SortOrder::Artist => {
            items.sort_by_key(|a| (a.artist.to_lowercase(), a.title.to_lowercase()));
        }
        SortOrder::Duration => {
            items.sort_by_key(|a| (a.duration_ms, a.title.to_lowercase()));
        }
        SortOrder::Custom | SortOrder::RecentlyAdded => {}
    }
}

fn sort_album_summaries(items: &mut [AlbumSummary], sort: SortOrder) {
    match sort {
        SortOrder::Title => {
            items.sort_by_key(|a| (a.title.to_lowercase(), a.artist.to_lowercase()));
        }
        SortOrder::Artist => {
            items.sort_by_key(|a| (a.artist.to_lowercase(), a.title.to_lowercase()));
        }
        SortOrder::Duration => {
            items.sort_by_key(|a| (std::cmp::Reverse(a.release_year), a.title.to_lowercase()));
        }
        SortOrder::Custom | SortOrder::RecentlyAdded => {}
    }
}

async fn resolve_track_summaries(
    session: &Session,
    stubs: &[CollectionStub],
    liked_set: &HashSet<String>,
) -> Vec<TrackSummary> {
    resolve_track_summaries_indexed(session, stubs, liked_set)
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn resolve_track_summaries_indexed(
    session: &Session,
    stubs: &[CollectionStub],
    liked_set: &HashSet<String>,
) -> Vec<Option<TrackSummary>> {
    let mut out = vec![None; stubs.len()];
    let mut futs = FuturesUnordered::new();
    let mut next_idx = 0;

    while next_idx < stubs.len() && futs.len() < METADATA_CONCURRENCY {
        let idx = next_idx;
        let stub = stubs[idx].clone();
        futs.push(fetch_one_track_summary(
            session,
            idx,
            stub,
            liked_set.clone(),
        ));
        next_idx += 1;
    }

    while let Some((idx, res)) = futs.next().await {
        out[idx] = res;
        if next_idx < stubs.len() {
            let idx = next_idx;
            let stub = stubs[idx].clone();
            futs.push(fetch_one_track_summary(
                session,
                idx,
                stub,
                liked_set.clone(),
            ));
            next_idx += 1;
        }
    }

    out
}

async fn fetch_one_track_summary(
    session: &Session,
    idx: usize,
    stub: CollectionStub,
    liked_set: HashSet<String>,
) -> (usize, Option<TrackSummary>) {
    let Ok(uri) = SpotifyUri::from_uri(&stub.uri) else {
        return (idx, None);
    };
    let Ok(track) = Track::get(session, &uri).await else {
        return (idx, None);
    };

    let artist_names = track
        .artists
        .0
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let artist_uri = track
        .artists
        .0
        .first()
        .map(|a| Arc::from(a.id.to_uri().as_str()));

    let cover = if !track.album.covers.0.is_empty() {
        pick_image_ref(&track.album.covers)
    } else {
        Album::get(session, &track.album.id)
            .await
            .ok()
            .and_then(|a| pick_image_ref(&a.covers))
    };

    (
        idx,
        Some(TrackSummary {
            uri: Arc::from(stub.uri.as_str()),
            title: track.name,
            artist: Arc::from(artist_names.as_str()),
            artist_uri,
            album: Arc::from(track.album.name.as_str()),
            album_uri: Some(Arc::from(track.album.id.to_uri().as_str())),
            cover,
            duration_ms: track.duration.clamp(0, i32::MAX) as u32,
            is_liked: liked_set.contains(&stub.uri),
            is_playable: true,
            added_at_unix: (stub.added_at > 0).then_some(stub.added_at as i64),
        }),
    )
}

async fn resolve_album_summaries(session: &Session, uris: &[String]) -> Vec<AlbumSummary> {
    resolve_album_summaries_indexed(session, uris)
        .await
        .into_iter()
        .flatten()
        .collect()
}

async fn resolve_album_summaries_indexed(
    session: &Session,
    uris: &[String],
) -> Vec<Option<AlbumSummary>> {
    let mut out = vec![None; uris.len()];
    let mut futs = FuturesUnordered::new();
    let mut next_idx = 0;

    while next_idx < uris.len() && futs.len() < METADATA_CONCURRENCY {
        let idx = next_idx;
        let uri_str = uris[idx].clone();
        futs.push(fetch_one_album_summary(session, idx, uri_str));
        next_idx += 1;
    }

    while let Some((idx, res)) = futs.next().await {
        out[idx] = res;
        if next_idx < uris.len() {
            let idx = next_idx;
            let uri_str = uris[idx].clone();
            futs.push(fetch_one_album_summary(session, idx, uri_str));
            next_idx += 1;
        }
    }

    out
}

async fn fetch_one_album_summary(
    session: &Session,
    idx: usize,
    uri_str: String,
) -> (usize, Option<AlbumSummary>) {
    let Ok(uri) = SpotifyUri::from_uri(&uri_str) else {
        return (idx, None);
    };
    let Ok(album) = Album::get(session, &uri).await else {
        return (idx, None);
    };
    let track_count: usize = album.discs.0.iter().map(|d| d.tracks.0.len()).sum();
    let artist_names = album
        .artists
        .0
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| !n.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let artist_uri = album
        .artists
        .0
        .first()
        .map(|a| Arc::from(a.id.to_uri().as_str()));

    (
        idx,
        Some(AlbumSummary {
            uri: Arc::from(uri_str.as_str()),
            title: album.name,
            artist: Arc::from(artist_names.as_str()),
            artist_uri,
            release_year: Some(album.date.0.year() as u16),
            track_count,
            cover: pick_image_ref(&album.covers),
            album_type: Arc::from(format!("{:?}", album.album_type).to_lowercase().as_str()),
        }),
    )
}

async fn resolve_artist_summaries(
    session: &Session,
    uris: &[String],
    followed: bool,
) -> Vec<ArtistSummary> {
    let mut out = vec![None; uris.len()];
    let mut futs = FuturesUnordered::new();
    let mut next_idx = 0;

    while next_idx < uris.len() && futs.len() < METADATA_CONCURRENCY {
        let idx = next_idx;
        let uri_str = uris[idx].clone();
        futs.push(fetch_one_artist_summary(session, idx, uri_str, followed));
        next_idx += 1;
    }

    while let Some((idx, res)) = futs.next().await {
        out[idx] = res;
        if next_idx < uris.len() {
            let idx = next_idx;
            let uri_str = uris[idx].clone();
            futs.push(fetch_one_artist_summary(session, idx, uri_str, followed));
            next_idx += 1;
        }
    }

    out.into_iter().flatten().collect()
}

async fn fetch_one_artist_summary(
    session: &Session,
    idx: usize,
    uri_str: String,
    followed: bool,
) -> (usize, Option<ArtistSummary>) {
    let Ok(uri) = SpotifyUri::from_uri(&uri_str) else {
        return (idx, None);
    };
    let Ok(artist) = Artist::get(session, &uri).await else {
        return (idx, None);
    };
    (
        idx,
        Some(ArtistSummary {
            uri: Arc::from(uri_str.as_str()),
            name: artist.name,
            cover: pick_image_ref(&artist.portraits),
            is_followed: followed,
        }),
    )
}

async fn resolve_playlist_summaries(session: &Session, uris: &[String]) -> Vec<PlaylistSummary> {
    let mut out = vec![None; uris.len()];
    let mut futs = FuturesUnordered::new();
    let mut next_idx = 0;

    while next_idx < uris.len() && futs.len() < METADATA_CONCURRENCY {
        let idx = next_idx;
        let uri_str = uris[idx].clone();
        futs.push(fetch_one_playlist_summary(session, idx, uri_str));
        next_idx += 1;
    }

    while let Some((idx, res)) = futs.next().await {
        out[idx] = res;
        if next_idx < uris.len() {
            let idx = next_idx;
            let uri_str = uris[idx].clone();
            futs.push(fetch_one_playlist_summary(session, idx, uri_str));
            next_idx += 1;
        }
    }

    out.into_iter().flatten().collect()
}

fn normalize_playlist_uri(raw: &str) -> Option<String> {
    let parsed = SpotifyUri::from_uri(raw.trim()).ok()?;
    match parsed {
        SpotifyUri::Playlist { id, .. } => Some(format!("spotify:playlist:{}", id.to_base62())),
        _ => None,
    }
}

fn normalize_track_uri(raw: &str) -> Option<String> {
    let parsed = SpotifyUri::from_uri(raw.trim()).ok()?;
    match parsed {
        SpotifyUri::Track { id } => Some(format!("spotify:track:{}", id.to_base62())),
        _ => None,
    }
}

fn proto_item_added_at_secs(ts_raw: i64) -> i32 {
    if ts_raw <= 0 {
        0
    } else if ts_raw > 9_295_169_800_000 {
        (ts_raw / 1_000_000).clamp(0, i32::MAX as i64) as i32
    } else if ts_raw > 10_000_000_000 {
        (ts_raw / 1_000).clamp(0, i32::MAX as i64) as i32
    } else {
        ts_raw.clamp(0, i32::MAX as i64) as i32
    }
}

fn proto_playlist_cover(attrs: &ProtoListAttributes) -> Option<ImageRef> {
    attrs
        .picture_size
        .first()
        .and_then(|p| image_ref_from_raw(p.url()))
        .or_else(|| {
            let pic = attrs.picture();
            if pic.is_empty() {
                None
            } else {
                let id_hex = FileId::from_raw(pic).to_base16();
                let url = format!("https://i.scdn.co/image/{id_hex}");
                Some(ImageRef {
                    id: Arc::from(id_hex.as_str()),
                    url: Some(Arc::from(url.as_str())),
                })
            }
        })
}

async fn fetch_proto_playlist(
    session: &Session,
    uri_str: &str,
) -> Result<ProtoSelectedListContent, String> {
    let parsed =
        SpotifyUri::from_uri(uri_str).map_err(|e| format!("invalid playlist URI: {e}"))?;
    let SpotifyUri::Playlist { id, .. } = parsed else {
        return Err(format!("not a playlist URI: {uri_str}"));
    };
    let bytes = session
        .spclient()
        .get_playlist(&id)
        .await
        .map_err(|e| format!("failed to load playlist {uri_str}: {e}"))?;
    ProtoSelectedListContent::parse_from_bytes(&bytes)
        .map_err(|e| format!("failed to decode playlist protobuf {uri_str}: {e}"))
}

fn playlist_track_stubs(
    playlist: &ProtoSelectedListContent,
    sort: SortOrder,
) -> Vec<CollectionStub> {
    let mut stubs: Vec<CollectionStub> = playlist
        .contents
        .items
        .iter()
        .filter_map(|item| {
            let uri = normalize_track_uri(item.uri())?;
            let added_at = proto_item_added_at_secs(item.attributes.timestamp());
            Some(CollectionStub { uri, added_at })
        })
        .collect();
    if matches!(sort, SortOrder::RecentlyAdded) {
        if stubs.iter().any(|s| s.added_at > 0) {
            stubs.sort_by_key(|s| std::cmp::Reverse(s.added_at));
        } else {
            stubs.reverse();
        }
    }
    stubs
}

async fn fetch_one_playlist_summary(
    session: &Session,
    idx: usize,
    uri_str: String,
) -> (usize, Option<PlaylistSummary>) {
    let Ok(pl) = fetch_proto_playlist(session, &uri_str).await else {
        return (idx, None);
    };
    let attrs = pl.attributes.get_or_default();
    let cover = proto_playlist_cover(attrs);
    (
        idx,
        Some(PlaylistSummary {
            uri: Arc::from(uri_str.as_str()),
            name: attrs.name().to_string(),
            owner: Arc::from(pl.owner_username().trim()),
            track_count: pl.length().max(0) as usize,
            total_duration_ms: 0,
            cover,
            is_collaborative: attrs.collaborative(),
        }),
    )
}

fn pick_image_ref(images: &[spotatui_librespot_metadata::image::Image]) -> Option<ImageRef> {
    use spotatui_librespot_metadata::image::ImageSize;
    images
        .iter()
        .find(|i| i.size == ImageSize::DEFAULT)
        .or_else(|| images.iter().find(|i| i.size == ImageSize::LARGE))
        .or_else(|| images.first())
        .map(|i| {
            let hex = i.id.to_base16();
            let url = format!("https://i.scdn.co/image/{hex}");
            ImageRef {
                id: Arc::from(hex.as_str()),
                url: Some(Arc::from(url.as_str())),
            }
        })
}

fn track_summary_from_web_api(t: &Value, liked_set: &HashSet<String>) -> Option<TrackSummary> {
    let uri = t.get("uri")?.as_str()?;
    let title = t.get("name")?.as_str()?.to_string();
    let artists_arr = t.get("artists").and_then(|a| a.as_array());
    let artist_names = artists_arr
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let artist_uri = artists_arr
        .and_then(|arr| arr.first())
        .and_then(|x| x.get("uri"))
        .and_then(|u| u.as_str())
        .map(Arc::from);
    let album_obj = t.get("album");
    let album_name = album_obj
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let album_uri = album_obj
        .and_then(|a| a.get("uri"))
        .and_then(|v| v.as_str())
        .map(Arc::from);
    let cover = album_obj
        .and_then(|a| a.get("images"))
        .and_then(|imgs| imgs.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
        .and_then(image_ref_from_raw);
    let duration_ms = t.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    Some(TrackSummary {
        uri: Arc::from(uri),
        title,
        artist: Arc::from(artist_names.as_str()),
        artist_uri,
        album: Arc::from(album_name),
        album_uri,
        cover,
        duration_ms,
        is_liked: liked_set.contains(uri),
        is_playable: t.get("is_playable").and_then(|v| v.as_bool()).unwrap_or(true),
        added_at_unix: None,
    })
}

fn parse_web_api_search(json: &Value, query: &str, liked_set: &HashSet<String>) -> SearchResults {
    let mut results = SearchResults {
        query: query.to_string(),
        ..Default::default()
    };

    if let Some(items) = json
        .get("tracks")
        .and_then(|x| x.get("items"))
        .and_then(|x| x.as_array())
    {
        for t in items {
            if let Some(ts) = track_summary_from_web_api(t, liked_set) {
                results.tracks.push(ts);
            }
        }
    }

    if let Some(items) = json
        .get("albums")
        .and_then(|x| x.get("items"))
        .and_then(|x| x.as_array())
    {
        for a in items {
            let Some(uri) = a.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let title = a.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let artists_arr = a.get("artists").and_then(|v| v.as_array());
            let artist_names = artists_arr
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let artist_uri = artists_arr
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("uri"))
                .and_then(|u| u.as_str())
                .map(Arc::from);
            let cover = a
                .get("images")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|i| i.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            let release_year = a
                .get("release_date")
                .and_then(|v| v.as_str())
                .and_then(|s| s.get(..4))
                .and_then(|y| y.parse::<u16>().ok());

            results.albums.push(AlbumSummary {
                uri: Arc::from(uri),
                title: title.to_string(),
                artist: Arc::from(artist_names.as_str()),
                artist_uri,
                release_year,
                track_count: a.get("total_tracks").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                cover,
                album_type: Arc::from(
                    a.get("album_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("album"),
                ),
            });
        }
    }

    if let Some(items) = json
        .get("artists")
        .and_then(|x| x.get("items"))
        .and_then(|x| x.as_array())
    {
        for a in items {
            let Some(uri) = a.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let cover = a
                .get("images")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|i| i.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            results.artists.push(ArtistSummary {
                uri: Arc::from(uri),
                name: name.to_string(),
                cover,
                is_followed: false,
            });
        }
    }

    if let Some(items) = json
        .get("playlists")
        .and_then(|x| x.get("items"))
        .and_then(|x| x.as_array())
    {
        for p in items.iter().filter(|x| !x.is_null()) {
            let Some(uri) = p.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let owner = p
                .get("owner")
                .and_then(|o| o.get("display_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cover = p
                .get("images")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|i| i.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            results.playlists.push(PlaylistSummary {
                uri: Arc::from(uri),
                name: name.to_string(),
                owner: Arc::from(owner),
                track_count: p
                    .get("tracks")
                    .and_then(|t| t.get("total"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                total_duration_ms: 0,
                cover,
                is_collaborative: p
                    .get("collaborative")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    results.top_match_uri = results
        .tracks
        .first()
        .map(|t| t.uri.clone())
        .or_else(|| results.artists.first().map(|a| a.uri.clone()))
        .or_else(|| results.albums.first().map(|a| a.uri.clone()));

    results
}

fn parse_pathfinder_search(
    json: &Value,
    query: &str,
    liked_set: &HashSet<String>,
) -> Option<SearchResults> {
    let search_v2 = json.get("data")?.get("searchV2")?;
    let mut results = SearchResults {
        query: query.to_string(),
        ..Default::default()
    };

    if let Some(items) = search_v2
        .get("tracksV2")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
    {
        for wrapper in items {
            let Some(data) = wrapper.get("item").and_then(|i| i.get("data")) else {
                continue;
            };
            let Some(uri) = data.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let title = data.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let artists_arr = data
                .get("artists")
                .and_then(|a| a.get("items"))
                .and_then(|v| v.as_array());
            let artist = artists_arr
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| {
                            x.get("profile")
                                .and_then(|p| p.get("name"))
                                .and_then(|n| n.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let artist_uri = artists_arr
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("uri"))
                .and_then(|u| u.as_str())
                .map(Arc::from);
            let album_obj = data.get("albumOfTrack");
            let album = album_obj
                .and_then(|a| a.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let album_uri = album_obj
                .and_then(|a| a.get("uri"))
                .and_then(|v| v.as_str())
                .map(Arc::from);
            let cover = album_obj
                .and_then(|a| a.get("coverArt"))
                .and_then(|c| c.get("sources"))
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            let duration_ms = data
                .get("duration")
                .and_then(|d| d.get("totalMilliseconds"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            results.tracks.push(TrackSummary {
                uri: Arc::from(uri),
                title: title.to_string(),
                artist: Arc::from(artist.as_str()),
                artist_uri,
                album: Arc::from(album),
                album_uri,
                cover,
                duration_ms,
                is_liked: liked_set.contains(uri),
                is_playable: true,
                added_at_unix: None,
            });
        }
    }

    if let Some(items) = search_v2
        .get("albumsV2")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
    {
        for wrapper in items {
            let Some(data) = wrapper.get("data") else {
                continue;
            };
            let Some(uri) = data.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let title = data.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let artist_names = data
                .get("artists")
                .and_then(|a| a.get("items"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| {
                            x.get("profile")
                                .and_then(|p| p.get("name"))
                                .and_then(|n| n.as_str())
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let cover = data
                .get("coverArt")
                .and_then(|c| c.get("sources"))
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            let release_year = data
                .get("date")
                .and_then(|d| d.get("year"))
                .and_then(|y| y.as_u64())
                .map(|y| y as u16);

            results.albums.push(AlbumSummary {
                uri: Arc::from(uri),
                title: title.to_string(),
                artist: Arc::from(artist_names.as_str()),
                artist_uri: None,
                release_year,
                track_count: 0,
                cover,
                album_type: Arc::from("album"),
            });
        }
    }

    if let Some(items) = search_v2
        .get("artists")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
    {
        for wrapper in items {
            let Some(data) = wrapper.get("data") else {
                continue;
            };
            let Some(uri) = data.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = data
                .get("profile")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(uri);
            let cover = data
                .get("visuals")
                .and_then(|v| v.get("avatarImage"))
                .and_then(|a| a.get("sources"))
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            results.artists.push(ArtistSummary {
                uri: Arc::from(uri),
                name: name.to_string(),
                cover,
                is_followed: false,
            });
        }
    }

    if let Some(items) = search_v2
        .get("playlists")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
    {
        for wrapper in items {
            let Some(data) = wrapper.get("data") else {
                continue;
            };
            let Some(uri) = data.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or(uri);
            let owner = data
                .get("ownerV2")
                .and_then(|o| o.get("data"))
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let cover = data
                .get("images")
                .and_then(|i| i.get("items"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("sources"))
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("url"))
                .and_then(|u| u.as_str())
                .and_then(image_ref_from_raw);
            results.playlists.push(PlaylistSummary {
                uri: Arc::from(uri),
                name: name.to_string(),
                owner: Arc::from(owner),
                track_count: 0,
                total_duration_ms: 0,
                cover,
                is_collaborative: false,
            });
        }
    }

    results.top_match_uri = results
        .tracks
        .first()
        .map(|t| t.uri.clone())
        .or_else(|| results.artists.first().map(|a| a.uri.clone()))
        .or_else(|| results.albums.first().map(|a| a.uri.clone()));

    Some(results)
}

#[cfg(test)]
mod tests {
    use super::{normalize_playlist_uri, normalize_track_uri, proto_item_added_at_secs};

    #[test]
    fn playlist_uri_normalization_accepts_user_scoped_and_skips_folders() {
        assert_eq!(
            normalize_playlist_uri("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M").as_deref(),
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M")
        );
        assert_eq!(
            normalize_playlist_uri("spotify:user:alice:playlist:37i9dQZF1DXcBWIGoYBM5M")
                .as_deref(),
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M")
        );
        assert_eq!(normalize_playlist_uri("spotify:start-group:1234:MyFolder"), None);
        assert_eq!(normalize_playlist_uri("spotify:end-group:1234"), None);
        assert_eq!(
            normalize_track_uri("spotify:track:4uLU6hMCjMI75M1A2tKUQC").as_deref(),
            Some("spotify:track:4uLU6hMCjMI75M1A2tKUQC")
        );
        assert_eq!(normalize_track_uri("spotify:local:a:b:c:10"), None);
    }

    #[test]
    fn proto_item_timestamp_normalizes_seconds_millis_and_micros() {
        assert_eq!(proto_item_added_at_secs(0), 0);
        assert_eq!(proto_item_added_at_secs(-5), 0);
        assert_eq!(proto_item_added_at_secs(1_700_000_000), 1_700_000_000);
        assert_eq!(proto_item_added_at_secs(1_700_000_000_000), 1_700_000_000);
        assert_eq!(proto_item_added_at_secs(10_000_000_000_000), 10_000_000);
    }
}
