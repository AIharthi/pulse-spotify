//! XDG configuration, private credential storage, device identity, and cache setup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use pulse_model::AppConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spotatui_librespot_core::cache::Cache;

const APP_DIR_NAME: &str = "pulse-spotify";

/// Resolved XDG filesystem paths used by the Pulse Spotify backend.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config_file: PathBuf,
    pub credentials_file: PathBuf,
    pub web_api_token_file: PathBuf,
    pub auth_meta_file: PathBuf,
    pub device_id_file: PathBuf,
    pub recent_file: PathBuf,
    pub audio_cache_dir: PathBuf,
    pub cover_cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolves standard XDG directories (`$XDG_CONFIG_HOME`, `$XDG_DATA_HOME`, `$XDG_CACHE_HOME`).
    pub fn from_xdg() -> Self {
        let config_base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
        let cache_base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            config_dir: config_base.join(APP_DIR_NAME),
            data_dir: data_base.join(APP_DIR_NAME),
            cache_dir: cache_base.join(APP_DIR_NAME),
            config_file: config_base.join(APP_DIR_NAME).join("config.json"),
            credentials_file: data_base.join(APP_DIR_NAME).join("credentials.json"),
            web_api_token_file: data_base.join(APP_DIR_NAME).join("web_api_token.json"),
            auth_meta_file: data_base.join(APP_DIR_NAME).join("auth_meta.json"),
            device_id_file: data_base.join(APP_DIR_NAME).join("device_id"),
            recent_file: data_base.join(APP_DIR_NAME).join("recent.json"),
            audio_cache_dir: cache_base.join(APP_DIR_NAME).join("audio"),
            cover_cache_dir: cache_base.join(APP_DIR_NAME).join("covers"),
        }
    }

    /// Creates isolated paths rooted under `root` for deterministic testing.
    pub fn for_root(root: &Path) -> Self {
        let config_dir = root.join("config");
        let data_dir = root.join("data");
        let cache_dir = root.join("cache");
        Self {
            config_file: config_dir.join("config.json"),
            credentials_file: data_dir.join("credentials.json"),
            web_api_token_file: data_dir.join("web_api_token.json"),
            auth_meta_file: data_dir.join("auth_meta.json"),
            device_id_file: data_dir.join("device_id"),
            recent_file: data_dir.join("recent.json"),
            audio_cache_dir: cache_dir.join("audio"),
            cover_cache_dir: cache_dir.join("covers"),
            config_dir,
            data_dir,
            cache_dir,
        }
    }

    /// Ensures required directories exist with restrictive permissions on secret stores.
    pub fn ensure_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(&self.config_dir)?;
        fs::create_dir_all(&self.data_dir)?;
        fs::create_dir_all(&self.audio_cache_dir)?;
        fs::create_dir_all(&self.cover_cache_dir)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.config_dir, fs::Permissions::from_mode(0o700));
            let _ = fs::set_permissions(&self.data_dir, fs::Permissions::from_mode(0o700));
        }

        Ok(())
    }
}

/// Optional user-configured PKCE Web API token persisted under `data_dir`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebApiToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_epoch_secs: u64,
    pub scope: String,
}

/// Persisted metadata recording how the cached Spotify credentials were obtained.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthMeta {
    pub zeroconf_only: bool,
    pub display_name: Option<String>,
}

/// Persisted local recent-play entry used to supplement recently played queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentEntry {
    pub uri: String,
    pub title: String,
    pub subtitle: String,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub duration_ms: u32,
    pub played_at_secs: u64,
}

/// Persisted list of recently played URIs (most recent first).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentHistory {
    pub entries: Vec<RecentEntry>,
}

impl RecentHistory {
    const MAX_ENTRIES: usize = 50;

    /// Records a played item at the front, deduplicating by URI.
    pub fn record(&mut self, entry: RecentEntry) {
        self.entries.retain(|e| e.uri != entry.uri);
        self.entries.insert(0, entry);
        self.entries.truncate(Self::MAX_ENTRIES);
    }
}

/// Loads [`AppConfig`] from `$XDG_CONFIG_HOME/pulse-spotify/config.json`, falling back to defaults.
pub fn load_config(paths: &AppPaths) -> AppConfig {
    let Ok(bytes) = fs::read(&paths.config_file) else {
        return AppConfig::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(cfg) => cfg,
        Err(err) => {
            log::warn!(
                "invalid config at {}, using defaults: {err}",
                paths.config_file.display()
            );
            AppConfig::default()
        }
    }
}

/// Saves [`AppConfig`] cleanly to disk.
pub fn save_config(paths: &AppPaths, config: &AppConfig) -> io::Result<()> {
    paths.ensure_dirs()?;
    let json = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    write_atomic(&paths.config_file, &json, false)
}

/// Returns the persistent 40-char lowercase hex device ID, generating one on first launch.
pub fn load_or_create_device_id(paths: &AppPaths) -> String {
    if let Ok(existing) = fs::read_to_string(&paths.device_id_file) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let _ = paths.ensure_dirs();
    let id = generate_device_id(paths);
    if let Err(err) = write_atomic(&paths.device_id_file, id.as_bytes(), false) {
        log::warn!(
            "failed to persist device_id at {}: {err}",
            paths.device_id_file.display()
        );
    }
    id
}

fn generate_device_id(paths: &AppPaths) -> String {
    let mut hasher = Sha256::new();
    if let Ok(machine_id) = fs::read_to_string("/etc/machine-id") {
        hasher.update(machine_id.trim().as_bytes());
    }
    hasher.update(paths.data_dir.to_string_lossy().as_bytes());
    hasher.update(std::process::id().to_le_bytes());
    if let Ok(dur) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.update(dur.as_nanos().to_le_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(40);
    for byte in &digest[..20] {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Loads saved PKCE Web API token, if present and valid JSON.
pub fn load_web_api_token(paths: &AppPaths) -> Option<WebApiToken> {
    let bytes = fs::read(&paths.web_api_token_file).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Saves PKCE Web API token with `0600` private file permissions on Unix.
pub fn save_web_api_token(paths: &AppPaths, token: &WebApiToken) -> io::Result<()> {
    paths.ensure_dirs()?;
    let json = serde_json::to_vec_pretty(token).map_err(io::Error::other)?;
    write_atomic(&paths.web_api_token_file, &json, true)
}

/// Deletes stored Web API token (called on sign-out).
pub fn clear_web_api_token(paths: &AppPaths) -> io::Result<()> {
    remove_if_exists(&paths.web_api_token_file)
}

/// Loads persisted authentication origin metadata (`zeroconf_only`, `display_name`).
pub fn load_auth_meta(paths: &AppPaths) -> AuthMeta {
    fs::read(&paths.auth_meta_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Persists authentication origin metadata.
pub fn save_auth_meta(paths: &AppPaths, meta: &AuthMeta) -> io::Result<()> {
    paths.ensure_dirs()?;
    let json = serde_json::to_vec_pretty(meta).map_err(io::Error::other)?;
    write_atomic(&paths.auth_meta_file, &json, true)
}

/// Deletes stored Spotify credentials, metadata, and Web API tokens on sign-out.
pub fn clear_auth_files(paths: &AppPaths) -> io::Result<()> {
    remove_if_exists(&paths.credentials_file)?;
    remove_if_exists(&paths.web_api_token_file)?;
    remove_if_exists(&paths.auth_meta_file)?;
    Ok(())
}

/// Enforces `0600` mode on `credentials.json` after librespot writes it.
pub fn lock_down_credentials_file(paths: &AppPaths) {
    #[cfg(unix)]
    if paths.credentials_file.exists() {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&paths.credentials_file, fs::Permissions::from_mode(0o600));
    }
}

/// Loads local recently played history.
pub fn load_recent_history(paths: &AppPaths) -> RecentHistory {
    fs::read(&paths.recent_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Saves local recently played history.
pub fn save_recent_history(paths: &AppPaths, history: &RecentHistory) -> io::Result<()> {
    paths.ensure_dirs()?;
    let json = serde_json::to_vec_pretty(history).map_err(io::Error::other)?;
    write_atomic(&paths.recent_file, &json, false)
}

/// Opens a [`Cache`] honoring the configured `disk_audio_cache_mb` ceiling.
///
/// Always configures `audio_cache_dir` (floor 16 MiB when `disk_audio_cache_mb == 0`)
/// so `session.cache()` can hold the currently playing track's fallback audio file.
pub fn open_librespot_cache(paths: &AppPaths, config: &AppConfig) -> io::Result<Cache> {
    paths.ensure_dirs()?;
    let size_limit = if config.disk_audio_cache_mb == 0 {
        16 * 1024 * 1024
    } else {
        config.disk_audio_cache_mb.saturating_mul(1024 * 1024)
    };
    let cache = Cache::new(
        Some(paths.data_dir.as_path()),
        Some(paths.data_dir.as_path()),
        Some(paths.audio_cache_dir.as_path()),
        Some(size_limit),
    )
    .map_err(|err| io::Error::other(format!("failed to initialize librespot cache: {err}")))?;
    lock_down_credentials_file(paths);
    Ok(cache)
}

/// Removes all cached audio files and recreates the cache directory.
pub fn clear_audio_cache_dir(paths: &AppPaths) -> io::Result<()> {
    if paths.audio_cache_dir.exists() {
        fs::remove_dir_all(&paths.audio_cache_dir)?;
    }
    fs::create_dir_all(&paths.audio_cache_dir)
}

/// Evicts oldest regular files recursively under `dir` until total bytes `<= target_max_bytes`.
pub fn prune_dir_to_bytes_lru(dir: &Path, target_max_bytes: u64) {
    use std::time::{SystemTime, UNIX_EPOCH};

    if !dir.exists() {
        return;
    }
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file()
                && let Ok(meta) = entry.metadata()
            {
                let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                files.push((mtime, meta.len(), entry.path()));
            }
        }
    }

    files.sort_by_key(|(mtime, _, _)| *mtime);
    let mut current: u64 = files.iter().map(|(_, len, _)| *len).sum();
    for (_, len, path) in files {
        if current <= target_max_bytes {
            break;
        }
        if fs::remove_file(path).is_ok() {
            current = current.saturating_sub(len);
        }
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Writes `contents` via a sibling temporary file so partial writes cannot corrupt files.
pub fn write_atomic(path: &Path, contents: &[u8], private: bool) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("pulse"),
        std::process::id()
    );
    let tmp_path = path.with_file_name(tmp_name);

    let mut opts = fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let mut file = opts.open(&tmp_path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);

    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&tmp_path, path)?;
    Ok(())
}
