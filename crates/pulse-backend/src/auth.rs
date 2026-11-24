//! Spotify OAuth (PKCE), Zeroconf credential conversion, and Premium session verification.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http::{Request, header};
use pulse_model::{AccountTier, AppConfig};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use spotatui_librespot_core::authentication::Credentials;
use spotatui_librespot_core::cache::Cache;
use spotatui_librespot_core::config::SessionConfig;
use spotatui_librespot_core::http_client::HttpClient;
use spotatui_librespot_core::session::Session;
use url::{Url, form_urlencoded};

use crate::config::{AppPaths, WebApiToken, lock_down_credentials_file};
use crate::dsp::SharedDsp;
use crate::player::{LocalPlayback, start_local_playback};

/// Official Spotify Desktop "Keymaster" client ID used by librespot.
///
/// NOTE: Only safe with the `"streaming"` scope for librespot AP login.
pub const KEYMASTER_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
/// Loopback redirect URI whitelisted for `KEYMASTER_CLIENT_ID`.
pub const LIBRESPOT_REDIRECT_URI: &str = "http://127.0.0.1:5588/login";
/// Scopes safe for `KEYMASTER_CLIENT_ID` AP login.
pub const LIBRESPOT_SCOPES: &[&str] = &["streaming"];

/// Loopback redirect URI for optional user-supplied Spotify Web API client IDs.
pub const WEB_API_REDIRECT_URI: &str = "http://127.0.0.1:8898/callback";
/// Scopes requested when the user supplies their own Developer Portal Client ID.
pub const WEB_API_SCOPES: &[&str] = &[
    "user-read-private",
    "user-library-read",
    "user-library-modify",
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-follow-read",
    "user-follow-modify",
    "user-read-recently-played",
    "user-read-playback-state",
    "user-modify-playback-state",
];

const SPOTIFY_AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Cancellation flag shared with the blocking loopback OAuth HTTP listener.
#[derive(Clone, Default)]
pub struct OAuthCancelToken {
    cancelled: Arc<AtomicBool>,
}

impl OAuthCancelToken {
    /// Creates an active cancellation token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Signals any blocking OAuth listener to abort cleanly.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Returns `true` once cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// RFC 7636 PKCE verifier/challenge pair plus CSRF `state` token.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl PkceChallenge {
    /// Generates fresh high-entropy PKCE verifier, S256 challenge, and CSRF state.
    pub fn generate() -> Self {
        let verifier = random_url_safe(32);
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(digest);
        let state = random_url_safe(18);
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

fn random_url_safe(byte_len: usize) -> String {
    let mut buf = vec![0u8; byte_len];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom")
        && file.read_exact(&mut buf).is_ok()
    {
        return URL_SAFE_NO_PAD.encode(&buf);
    }

    // Portable fallback entropy mixed via SHA-256.
    let seq = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(seq.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    URL_SAFE_NO_PAD.encode(&digest[..byte_len.min(digest.len())])
}

/// Builds the Spotify `/authorize` PKCE URL for the given client ID, redirect URI, and scopes.
pub fn build_authorize_url(
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
    pkce: &PkceChallenge,
) -> Result<String, String> {
    let scope_str = scopes.join(" ");
    let mut url = Url::parse(SPOTIFY_AUTHORIZE_URL).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("state", &pkce.state)
        .append_pair("scope", &scope_str);
    Ok(url.into())
}

/// Result of establishing and verifying a Spotify AP session.
pub enum ConnectOutcome {
    /// Authenticated Spotify Premium account with active `Session`, cached reusable credentials,
    /// and initialized `Spirc` + `Player`.
    Premium {
        session: Session,
        username: String,
        reusable_creds: Credentials,
        local_playback: Box<LocalPlayback>,
    },
    /// Authenticated Spotify Free account (cannot stream/use Connect via librespot).
    NonPremium {
        username: String,
        account_type: String,
    },
}

#[derive(Debug, Deserialize)]
struct SpotifyTokenPayload {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

fn default_expires_in() -> u64 {
    3600
}

/// Runs the cancellable `127.0.0.1` loopback HTTP listener on `bind_addr` (e.g. `127.0.0.1:5588`),
/// verifies `state`, and returns the authorization `code`.
pub async fn wait_for_oauth_callback(
    bind_addr: String,
    expected_path: String,
    expected_state: String,
    cancel: OAuthCancelToken,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let mut attempts = 0;
        let listener = loop {
            match TcpListener::bind(&bind_addr) {
                Ok(l) => break l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && attempts < 20 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    return Err(format!("failed to bind OAuth callback on {bind_addr}: {e}"));
                }
            }
        };
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to configure non-blocking OAuth socket: {e}"))?;

        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        loop {
            if cancel.is_cancelled() {
                return Err("Spotify login cancelled".to_string());
            }
            if std::time::Instant::now() >= deadline {
                return Err("Spotify login timed out waiting for browser callback".to_string());
            }

            match listener.accept() {
                Ok((mut stream, _addr)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                    let Some(raw_bytes) = read_http_request_head(&mut stream, &cancel) else {
                        continue;
                    };
                    let raw = String::from_utf8_lossy(&raw_bytes);
                    let first_line = raw.lines().next().unwrap_or("");
                    let mut parts = first_line.split_whitespace();
                    let _method = parts.next();
                    let target = parts.next().unwrap_or("/");

                    let full_url = if target.starts_with("http://") || target.starts_with("https://")
                    {
                        Url::parse(target)
                    } else {
                        Url::parse(&format!("http://127.0.0.1{target}"))
                    };
                    let full_url = match full_url {
                        Ok(u) => u,
                        Err(_) => {
                            write_http_response(&mut stream, 400, "Invalid callback request.");
                            continue;
                        }
                    };

                    if full_url.path() != expected_path {
                        write_http_response(&mut stream, 404, "Not found.");
                        continue;
                    }

                    let params: HashMap<String, String> =
                        full_url.query_pairs().into_owned().collect();

                    // Validate CSRF `state` BEFORE honoring `error` or `code`, and ignore forged
                    // requests instead of aborting the user's in-progress OAuth flow.
                    let state = params.get("state").map(String::as_str).unwrap_or("");
                    if state != expected_state {
                        write_http_response(
                            &mut stream,
                            400,
                            "OAuth state mismatch. Please retry login in Aplayer.",
                        );
                        continue;
                    }

                    if let Some(err) = params.get("error") {
                        write_http_response(
                            &mut stream,
                            400,
                            &format!("Authorization denied: {err}. You can close this tab."),
                        );
                        return Err(format!("Spotify authorization failed: {err}"));
                    }

                    let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
                        write_http_response(&mut stream, 400, "Missing authorization code.");
                        continue;
                    };

                    write_http_response(
                        &mut stream,
                        200,
                        "Aplayer Spotify login complete. You may close this browser tab.",
                    );
                    return Ok(code.clone());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(err) => {
                    return Err(format!("OAuth listener socket error: {err}"));
                }
            }
        }
    })
    .await
    .map_err(|e| format!("OAuth callback task failed: {e}"))?
}

fn read_http_request_head(
    stream: &mut std::net::TcpStream,
    cancel: &OAuthCancelToken,
) -> Option<Vec<u8>> {
    const MAX_REQ_HEAD_BYTES: usize = 16 * 1024;
    let mut out = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let read_deadline = std::time::Instant::now() + Duration::from_secs(2);

    while out.len() < MAX_REQ_HEAD_BYTES {
        if cancel.is_cancelled() || std::time::Instant::now() >= read_deadline {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.windows(4).any(|w| w == b"\r\n\r\n")
                    || out.windows(2).any(|w| w == b"\n\n")
                {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if out.windows(2).any(|w| w == b"\r\n") {
                    break;
                }
                continue;
            }
            Err(_) => return None,
        }
    }

    if out.is_empty() { None } else { Some(out) }
}

fn html_escape(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn write_http_response(stream: &mut std::net::TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let safe_message = html_escape(message);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Aplayer</title>\
         <style>body{{font-family:sans-serif;background:#121316;color:#f4f4f5;\
         display:flex;align-items:center;justify-content:center;height:100vh;margin:0;}}\
         .card{{background:#1b1d22;padding:28px 34px;border-radius:12px;border:1px solid #2b2e36;}}</style>\
         </head><body><div class=\"card\"><h3>Aplayer</h3><p>{safe_message}</p></div></body></html>"
    );
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// Exchanges a PKCE authorization code for Spotify access + refresh tokens via `HttpClient`.
pub async fn exchange_pkce_code(
    http: &HttpClient,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<WebApiToken, String> {
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("client_id", client_id)
        .append_pair("code_verifier", verifier)
        .finish();

    request_spotify_token(http, client_id, body, None).await
}

/// Refreshes an expired user-configured Web API PKCE token.
pub async fn refresh_web_api_token(
    http: &HttpClient,
    token: &WebApiToken,
) -> Result<WebApiToken, String> {
    let refresh_token = token
        .refresh_token
        .as_deref()
        .ok_or_else(|| "missing Web API refresh_token".to_string())?;
    let body = form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", refresh_token)
        .append_pair("client_id", &token.client_id)
        .finish();

    request_spotify_token(http, &token.client_id, body, Some(refresh_token.to_string())).await
}

async fn request_spotify_token(
    http: &HttpClient,
    client_id: &str,
    form_body: String,
    fallback_refresh: Option<String>,
) -> Result<WebApiToken, String> {
    let req = Request::post(SPOTIFY_TOKEN_URL)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Bytes::from(form_body))
        .map_err(|e| format!("failed to build token request: {e}"))?;

    let body_bytes = tokio::time::timeout(Duration::from_secs(20), http.request_body(req))
        .await
        .map_err(|_| "Spotify token exchange timed out".to_string())?
        .map_err(|e| format!("Spotify token exchange failed: {e}"))?;

    let payload: SpotifyTokenPayload = serde_json::from_slice(&body_bytes)
        .map_err(|e| format!("invalid Spotify token JSON: {e}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    Ok(WebApiToken {
        client_id: client_id.to_string(),
        access_token: payload.access_token,
        refresh_token: payload.refresh_token.or(fallback_refresh),
        expires_at_epoch_secs: now.saturating_add(payload.expires_in),
        scope: payload.scope,
    })
}

/// Converts a `librespot-discovery 0.8.0` credential payload into
/// `spotatui-librespot-core 0.8.7` [`Credentials`].
pub fn convert_discovery_credentials(
    creds: librespot_discovery::Credentials,
) -> Result<Credentials, String> {
    let json = serde_json::to_vec(&creds)
        .map_err(|e| format!("failed to serialize discovery credentials: {e}"))?;
    serde_json::from_slice(&json)
        .map_err(|e| format!("failed to decode discovery credentials: {e}"))
}

/// Connects a librespot `Session`, initializes `Player` + `Spirc` once on that unconnected
/// session, and waits briefly for `ProductInfo` so Free accounts are caught cleanly.
pub async fn connect_and_verify_session(
    paths: &AppPaths,
    session_cfg: SessionConfig,
    cache: Cache,
    creds: Credentials,
    config: &AppConfig,
    initial_volume: u16,
    dsp: Arc<SharedDsp>,
) -> Result<ConnectOutcome, String> {
    let session = Session::new(session_cfg, Some(cache));
    let local_playback =
        start_local_playback(&session, creds.clone(), config, initial_volume, dsp).await?;

    lock_down_credentials_file(paths);
    let reusable_creds = session
        .cache()
        .and_then(|c| c.credentials())
        .unwrap_or(creds);

    // `PacketType::ProductInfo` arrives asynchronously right after AP login and shuts down
    // the session inside DispatchTask if `type != "premium"` (shortly before writing
    // `user_attributes`). Give one brief yield after `is_invalid()` so `type` is observed.
    let mut account_type: Option<String> = None;
    for _ in 0..60 {
        if let Some(ty) = session.get_user_attribute("type") {
            account_type = Some(ty);
            break;
        }
        if session.is_invalid() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            account_type = session.get_user_attribute("type");
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let username = session.username();
    if let Some(ty) = account_type {
        if ty != "premium" {
            local_playback.shutdown().await;
            session.shutdown();
            return Ok(ConnectOutcome::NonPremium {
                username,
                account_type: ty,
            });
        }
        return Ok(ConnectOutcome::Premium {
            session,
            username,
            reusable_creds,
            local_playback: Box::new(local_playback),
        });
    }

    if session.is_invalid() {
        local_playback.shutdown().await;
        session.shutdown();
        return Err(
            "Spotify rejected this session (Spotify Premium is required for direct streaming)."
                .to_string(),
        );
    }

    // ProductInfo delayed slightly longer; let session proceed unless invalidated later.
    Ok(ConnectOutcome::Premium {
        session,
        username,
        reusable_creds,
        local_playback: Box::new(local_playback),
    })
}

/// Helper for mapping raw `ProductInfo["type"]` string into [`AccountTier`].
pub fn account_tier_from_attribute(ty: Option<&str>) -> AccountTier {
    match ty {
        Some("premium") => AccountTier::Premium,
        Some(_) => AccountTier::Free,
        None => AccountTier::Unknown,
    }
}
