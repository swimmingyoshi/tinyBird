//! Client for the 0xstash media API, used as tinyBird's asset store.
//!
//! # Why this runs on the server
//!
//! Writes to the API need a key. Reads do not — assets are served publicly
//! through a CDN. So the split is:
//!
//! - the **browser** fetches ROM and asset *bytes* straight from the public
//!   asset URL, which is fast, cacheable, and involves no credential;
//! - the **server** holds the key and does the listing and uploading.
//!
//! The key is read from the environment (see [`crate::dotenv`]) and is never
//! sent to the page. Everything degrades to "storage not configured" when it is
//! absent, so the emulator still runs from local files.

use std::env;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default deployment of the media API.
pub const DEFAULT_BASE_URL: &str = "https://media.0xstash.dev";
/// Vault used when none is configured.
pub const DEFAULT_VAULT: &str = "default";
/// Owner used for private saves when none is configured.
pub const DEFAULT_OWNER: &str = "local";
/// Download project used when none is configured.
///
/// Save states go to the downloads API rather than the media vault: the vault
/// only accepts image, audio, and video and verifies the bytes match. A key is
/// authorised for specific projects, so this has to match the key's scope.
pub const DEFAULT_PROJECT: &str = "tinybird";
/// Extension used for uploaded saves.
///
/// The project's `allowedDownloadExtensions` decides what is permitted; this is
/// the one we ask for. Sent as the multipart `type` field — note the OpenAPI
/// document still calls that field `filetype`, but only `type` is honoured, and
/// omitting it falls back to `.bin`, which most projects do not allow.
const SAVE_EXTENSION: &str = ".savestate";
/// Extensions accepted when reading a listing back.
///
/// Saves written before the project settled on `.savestate` used `.bin`; they
/// still list and still load.
const SAVE_EXTENSIONS: [&str; 4] = [".savestate", ".save", ".sav", ".bin"];
/// How many save states to keep per game.
///
/// The API key *is* the user here — one key, one project namespace — so this is
/// per game within that key's storage. A key shared between people shares the
/// quota.
pub const MAX_SAVES_PER_GAME: usize = 5;
/// Prefix marking a download as one of ours, so unrelated files in the same
/// project are left alone.
const SAVE_PREFIX: &str = "tb";
/// Largest file the storage API accepts.
///
/// A raw save state is around 18 MB because the emulator serializes the whole
/// cartridge into it, so the browser gzips before uploading — that lands near
/// 5 MB. This check exists to fail with a sentence rather than relaying a bare
/// 413 from the API.
pub const MAX_SAVE_BYTES: usize = 16 * 1024 * 1024;
/// The API rejects requests without this header, or an equivalent bearer token.
const KEY_HEADER: &str = "X-Media-Key";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Save states run to tens of megabytes, so they get a longer budget.
const SAVE_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Connection details for the media API.
#[derive(Clone, Debug)]
pub struct MediaConfig {
    pub base_url: String,
    pub vault: String,
    pub project: String,
    /// Who saves belong to when nobody is signed in.
    ///
    /// With accounts configured the owner is the token's `sub` claim and this
    /// is not consulted. It is the fallback for running the server by yourself
    /// with no auth service, which is still a supported way to use tinyBird.
    /// It is never taken from anything the browser sends.
    pub default_owner: String,
    key: Option<String>,
}

impl MediaConfig {
    /// Build from the environment.
    ///
    /// | variable | meaning | default |
    /// |---|---|---|
    /// | `TINYBIRD_MEDIA_URL` | API base URL | `https://media.0xstash.dev` |
    /// | `TINYBIRD_MEDIA_KEY` | API key; storage is disabled without it | none |
    /// | `TINYBIRD_MEDIA_VAULT` | vault to list and upload into | `default` |
    pub fn from_env() -> Self {
        Self {
            base_url: env::var("TINYBIRD_MEDIA_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            vault: env::var("TINYBIRD_MEDIA_VAULT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_VAULT.to_string()),
            project: env::var("TINYBIRD_MEDIA_PROJECT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_PROJECT.to_string()),
            default_owner: env::var("TINYBIRD_MEDIA_OWNER")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OWNER.to_string()),
            key: env::var("TINYBIRD_MEDIA_KEY")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        }
    }

    /// Whether a key is present. Without one, only local files are available.
    pub fn is_configured(&self) -> bool {
        self.key.is_some()
    }

    /// Public URL for an asset in the configured vault.
    ///
    /// Built rather than taken from the API response so the page always gets an
    /// absolute URL it can hand straight to `fetch`.
    pub fn asset_url(&self, name: &str) -> String {
        if self.vault == DEFAULT_VAULT {
            format!("{}/vault/{}", self.base_url, name)
        } else {
            format!("{}/{}/{}", self.base_url, self.vault, name)
        }
    }
}

/// One asset as the page needs to see it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MediaAsset {
    pub name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// What `/api/library` returns.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LibraryResponse {
    /// False when no key is set; the page shows a setup hint instead of an error.
    pub configured: bool,
    pub vault: String,
    pub assets: Vec<MediaAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LibraryResponse {
    pub fn unconfigured(vault: String) -> Self {
        Self {
            configured: false,
            vault,
            assets: Vec::new(),
            error: None,
        }
    }

    pub fn failed(vault: String, error: impl Into<String>) -> Self {
        Self {
            configured: true,
            vault,
            assets: Vec::new(),
            error: Some(error.into()),
        }
    }
}

/// Pull the asset names out of whatever shape the API returned.
///
/// The listing endpoint is not strictly typed in the OpenAPI document, so this
/// accepts a bare array or an object wrapping one under a handful of plausible
/// keys, and skips entries with no usable name rather than failing the request.
/// A rename upstream should degrade to an empty library, not a 500.
pub fn parse_asset_list(value: &serde_json::Value, config: &MediaConfig) -> Vec<MediaAsset> {
    let items = value
        .as_array()
        .or_else(|| value.get("assets").and_then(|v| v.as_array()))
        .or_else(|| value.get("images").and_then(|v| v.as_array()))
        .or_else(|| value.get("items").and_then(|v| v.as_array()))
        .or_else(|| value.get("data").and_then(|v| v.as_array()));

    let Some(items) = items else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let name = first_string(
                item,
                &[
                    // What the 0xstash API actually returns.
                    "originalName",
                    "original_name",
                    "name",
                    "filename",
                    "asset_name",
                    "assetName",
                    "key",
                ],
            )?;

            // Prefer a URL the API gave us; fall back to the vault path.
            let url = first_string(item, &["url", "publicUrl", "public_url", "href"])
                .map(|url| absolutise(&url, config))
                .unwrap_or_else(|| config.asset_url(&name));

            Some(MediaAsset {
                name,
                url,
                size: first_u64(item, &["size", "bytes", "sizeBytes", "size_bytes"]),
                content_type: first_string(item, &["contentType", "content_type", "mime", "type"]),
            })
        })
        .collect()
}

fn first_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(|text| text.to_string())
        .filter(|text| !text.is_empty())
}

fn first_u64(value: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_u64()))
}

/// Turn a possibly-relative URL from the API into an absolute one.
fn absolutise(url: &str, config: &MediaConfig) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("{}/{}", config.base_url, url.trim_start_matches('/'))
    }
}

/// Best-guess content type from a filename.
///
/// Used when the uploader did not declare one. The media API only accepts
/// image, audio, and video, so anything else is passed through as-is and will
/// be refused with a clear 415 rather than silently mislabelled.
pub fn guess_content_type(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    match lower.rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Whether a name looks like a GBA ROM, for filtering the library view.
pub fn is_rom_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gba") || lower.ends_with(".bin") || lower.ends_with(".agb")
}

/// List the configured vault.
pub async fn list_assets(config: &MediaConfig) -> LibraryResponse {
    let Some(key) = config.key.as_deref() else {
        return LibraryResponse::unconfigured(config.vault.clone());
    };

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => return LibraryResponse::failed(config.vault.clone(), err.to_string()),
    };

    let response = client
        .get(format!("{}/api/assets", config.base_url))
        .header(KEY_HEADER, key)
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(err) => return LibraryResponse::failed(config.vault.clone(), err.to_string()),
    };

    if !response.status().is_success() {
        return LibraryResponse::failed(
            config.vault.clone(),
            format!("media API returned {}", response.status()),
        );
    }

    match response.json::<serde_json::Value>().await {
        Ok(value) => LibraryResponse {
            configured: true,
            vault: config.vault.clone(),
            assets: parse_asset_list(&value, config),
            error: None,
        },
        Err(err) => LibraryResponse::failed(config.vault.clone(), err.to_string()),
    }
}

/// Upload bytes into the configured vault, returning the public URL.
///
/// The media API accepts images, audio, and video only, and verifies that the
/// bytes match the declared type — so the caller must send a real media file.
/// Save states and ROMs are rejected with a 415.
pub async fn upload_asset(
    config: &MediaConfig,
    filename: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> Result<MediaAsset, String> {
    let key = config.key.as_deref().ok_or("storage is not configured")?;

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    // The declared type has to be the real one: the API sniffs the bytes and
    // rejects a mismatch, so passing the browser's own type through is what
    // makes a PNG upload succeed.
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(content_type)
        .map_err(|err| err.to_string())?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("vault", config.vault.clone());

    let response = client
        .post(format!("{}/api/assets", config.base_url))
        .header(KEY_HEADER, key)
        .multipart(form)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|err| err.to_string())?;

    if !status.is_success() {
        let detail = value
            .get("detail")
            .and_then(|d| d.as_str())
            .unwrap_or("upload rejected");
        return Err(format!("{status}: {detail}"));
    }

    let name = first_string(
        &value,
        &["originalName", "original_name", "name", "filename", "assetName"],
    )
    .unwrap_or_else(|| filename.to_string());
    let url = first_string(&value, &["url", "publicUrl", "public_url", "href"])
        .map(|url| absolutise(&url, config))
        .unwrap_or_else(|| config.asset_url(&name));

    Ok(MediaAsset {
        name,
        url,
        size: first_u64(&value, &["size", "bytes", "sizeBytes"]),
        content_type: first_string(&value, &["contentType", "content_type", "mime"]),
    })
}


/// Whether `url` points at the configured storage host.
///
/// The proxy exists because the storage CDN sends no
/// `Access-Control-Allow-Origin`, so the browser cannot fetch asset bytes
/// itself. Restricting it to the configured host keeps it from becoming an
/// open relay that could be pointed at anything on the network.
pub fn is_storage_url(config: &MediaConfig, url: &str) -> bool {
    let base = config.base_url.trim_end_matches('/');
    // Compare against `base/` so a lookalike host cannot pass on a prefix match.
    url.starts_with(&format!("{base}/"))
}

/// Fetch bytes from the storage host on the browser's behalf.
///
/// Returns the body and its content type. Reads are public, so no key is sent.
pub async fn proxy_fetch(config: &MediaConfig, url: &str) -> Result<(Vec<u8>, String), String> {
    if !is_storage_url(config, url) {
        return Err("that URL is not on the configured storage host".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(SAVE_UPLOAD_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        return Err(format!("storage returned {}", response.status()));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = response.bytes().await.map_err(|err| err.to_string())?;
    Ok((bytes.to_vec(), content_type))
}


// -------------------------------------------------------------- capabilities

/// Where save states are stored.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveBackend {
    /// Owner-scoped, delivered through expiring signed URLs. Preferred: a save
    /// is someone's progress and does not belong on a guessable public URL.
    Private,
    /// Public `/downloads` URLs. Used when the project does not allow binary
    /// private assets.
    PublicDownloads,
}

/// What the configured vault permits, read at the time of use.
#[derive(Clone, Debug, Serialize)]
pub struct Capabilities {
    pub vault_id: String,
    pub backend: SaveBackend,
    pub private_enabled: bool,
    pub allowed_download_extensions: Vec<String>,
    pub allowed_private_extensions: Vec<String>,
}

impl Capabilities {
    /// Choose a backend from the vault's extension allowlists.
    ///
    /// Read rather than assumed, so enabling binary private assets takes effect
    /// without a code change or a restart.
    fn decide(
        vault_id: String,
        private_enabled: bool,
        allowed_download: Vec<String>,
        allowed_private: Vec<String>,
    ) -> Self {
        let private_ok = private_enabled
            && allowed_private
                .iter()
                .any(|ext| ext.eq_ignore_ascii_case(SAVE_EXTENSION));
        Self {
            vault_id,
            backend: if private_ok {
                SaveBackend::Private
            } else {
                SaveBackend::PublicDownloads
            },
            private_enabled,
            allowed_download_extensions: allowed_download,
            allowed_private_extensions: allowed_private,
        }
    }
}

fn string_list(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the vault's configuration and decide how saves should be stored.
/// How long a read of the vault's configuration stays good for.
///
/// It changes when an administrator edits the project, which is rare, so a
/// short cache costs nothing and a switch is still picked up within the minute
/// without a restart.
const CAPABILITIES_TTL: Duration = Duration::from_secs(60);

static CAPABILITIES_CACHE: std::sync::OnceLock<
    std::sync::Mutex<Option<(std::time::Instant, Capabilities)>>,
> = std::sync::OnceLock::new();

fn capabilities_cache() -> &'static std::sync::Mutex<Option<(std::time::Instant, Capabilities)>> {
    CAPABILITIES_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Read the vault's configuration, using a recent answer when there is one.
///
/// Without the cache an overwrite made thirteen requests to storage, eight of
/// them re-reading configuration that had not changed. That is slow, and every
/// extra request is another chance for a transient network failure to abort
/// somebody's save.
pub async fn capabilities(config: &MediaConfig) -> Result<Capabilities, String> {
    if let Ok(cache) = capabilities_cache().lock() {
        if let Some((read_at, capabilities)) = cache.as_ref() {
            if read_at.elapsed() < CAPABILITIES_TTL {
                return Ok(capabilities.clone());
            }
        }
    }

    let capabilities = fetch_capabilities(config).await?;
    if let Ok(mut cache) = capabilities_cache().lock() {
        *cache = Some((std::time::Instant::now(), capabilities.clone()));
    }
    Ok(capabilities)
}

/// Forget the cached configuration, so the next read goes to the service.
#[cfg(test)]
pub fn forget_capabilities() {
    if let Ok(mut cache) = capabilities_cache().lock() {
        *cache = None;
    }
}

async fn fetch_capabilities(config: &MediaConfig) -> Result<Capabilities, String> {
    let key = config.key.as_deref().ok_or("storage is not configured")?;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    let vaults = client
        .get(format!("{}/api/vaults", config.base_url))
        .header(KEY_HEADER, key)
        .send()
        .await
        .map_err(|err| err.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|err| err.to_string())?;

    let vaults = vaults.as_array().cloned().unwrap_or_default();
    // Prefer the vault matching the configured project; a project key usually
    // only sees one, so fall back to whatever it can see.
    let vault = vaults
        .iter()
        .find(|v| {
            first_string(v, &["slug", "path"])
                .map(|slug| slug.eq_ignore_ascii_case(&config.project))
                .unwrap_or(false)
        })
        .or_else(|| vaults.first())
        .ok_or("the key can see no vaults")?;

    let vault_id = first_string(vault, &["id"]).ok_or("the vault has no id")?;

    let settings = client
        .get(format!("{}/api/vaults/{vault_id}/config", config.base_url))
        .header(KEY_HEADER, key)
        .send()
        .await
        .map_err(|err| err.to_string())?
        .json::<serde_json::Value>()
        .await
        .map_err(|err| err.to_string())?;

    Ok(Capabilities::decide(
        vault_id,
        settings
            .get("privateEnabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        string_list(&settings, "allowedDownloadExtensions"),
        string_list(&settings, "allowedPrivateExtensions"),
    ))
}

// ---------------------------------------------------------------- save states

/// One stored save state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SaveEntry {
    /// Who this save belongs to. [`LEGACY_OWNER`] for anything written before
    /// accounts existed.
    pub owner: String,
    /// What delete takes: the private asset id, or the downloads filename.
    pub id: String,
    /// True when this save lives in private, owner-scoped storage.
    pub private: bool,
    /// Server-generated name. Empty for private assets, which use `id`.
    pub filename: String,
    /// The name we uploaded, carrying the game code and timestamp.
    pub original_name: String,
    /// Cartridge code the save belongs to, e.g. `BPRE`.
    pub game_code: String,
    /// Optional human label.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// When it was taken, in milliseconds since the epoch.
    pub saved_at_ms: u64,
    /// Public URL; no credential needed to fetch it.
    pub url: String,
    pub size: u64,
}

/// What `/api/saves` returns.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SavesResponse {
    pub configured: bool,
    pub limit: usize,
    /// Where these saves live. `None` when storage could not be reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<SaveBackend>,
    pub saves: Vec<SaveEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Build the upload name for a save.
///
/// The project only permits one extension, so everything the UI needs — which
/// game, when, and any label — has to live in the stem. Underscore-separated
/// because the API preserves the name verbatim and it round-trips cleanly.
/// Owner recorded for saves written before accounts existed.
pub const LEGACY_OWNER: &str = "local";

/// What a save's name tells us about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSave {
    pub owner: String,
    pub game_code: String,
    pub saved_at_ms: u64,
    pub label: String,
}

/// Build the name a save is stored under.
///
/// The owner is part of the name because the downloads backend has no notion of
/// one: a name is all there is to say whose save this is. The private backend
/// does track an owner, and carrying it in both places costs nothing and keeps
/// one parser working for either.
pub fn save_name(owner: &str, game_code: &str, saved_at_ms: u64, label: &str) -> String {
    let owner = sanitize_token_capped(owner, MAX_OWNER_LEN);
    let code = sanitize_token(game_code);
    let label = sanitize_token(label);
    let stem = format!("{SAVE_PREFIX}_{owner}_{code}_{saved_at_ms}");
    if label.is_empty() {
        format!("{stem}{SAVE_EXTENSION}")
    } else {
        format!("{stem}_{label}{SAVE_EXTENSION}")
    }
}

/// Read back what [`save_name`] wrote. `None` for anything we did not write.
/// Read a save's name back.
///
/// Two shapes exist. Saves written before accounts have no owner field, and
/// belong to [`LEGACY_OWNER`]:
///
/// ```text
/// tb_BPRE_1787570960801_route4.savestate            (before accounts)
/// tb_<owner>_BPRE_1787570960801_route4.savestate    (since)
/// ```
///
/// The timestamp is what tells them apart: it is the only field that is always
/// a number, and a game code never is.
pub fn parse_save_name(original_name: &str) -> Option<ParsedSave> {
    let stem = SAVE_EXTENSIONS
        .iter()
        .find_map(|ext| original_name.strip_suffix(ext))?;
    let (prefix, rest) = stem.split_once('_')?;
    if prefix != SAVE_PREFIX {
        return None;
    }

    let mut parts = rest.splitn(4, '_');
    let first = parts.next()?.to_string();
    let second = parts.next()?.to_string();
    let third = parts.next().unwrap_or("").to_string();

    if let Ok(saved_at_ms) = second.parse::<u64>() {
        // tb_<game>_<millis>[_<label>]
        if first.is_empty() {
            return None;
        }
        let label = match parts.next() {
            Some(rest) if !third.is_empty() => format!("{third}_{rest}"),
            _ => third,
        };
        return Some(ParsedSave {
            owner: LEGACY_OWNER.to_string(),
            game_code: first,
            saved_at_ms,
            label,
        });
    }

    // tb_<owner>_<game>_<millis>[_<label>]
    let saved_at_ms = third.parse::<u64>().ok()?;
    if first.is_empty() || second.is_empty() {
        return None;
    }
    Some(ParsedSave {
        owner: first,
        game_code: second,
        saved_at_ms,
        label: parts.next().unwrap_or("").to_string(),
    })
}

/// Keep names to characters that survive a URL path unchanged.
/// Reduce a value to something safe to put in a filename.
///
/// Drops anything that would need escaping in a URL or could introduce a path
/// separator, and caps the length so a long label cannot push a name past what
/// the storage API accepts.
fn sanitize_token(value: &str) -> String {
    sanitize_token_capped(value, 24)
}

/// [`sanitize_token`] with an explicit cap.
///
/// Owner ids get a longer one. A subject claim is 32 characters, and truncating
/// it to 24 would let two people whose ids share a prefix own each other's
/// saves — the kind of collision that only shows up once there are enough
/// accounts to hit it.
fn sanitize_token_capped(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .take(max)
        .collect()
}

/// The longest owner id a save name will carry. A subject claim is a UUID,
/// with or without dashes.
const MAX_OWNER_LEN: usize = 36;

/// Which saves exceed the per-game limit, newest first.
///
/// Split out from the network calls so the retention rule is testable on its
/// own — getting it wrong deletes someone's progress.
pub fn saves_to_evict(saves: &[SaveEntry], limit: usize) -> Vec<&SaveEntry> {
    let mut sorted: Vec<&SaveEntry> = saves.iter().collect();
    sorted.sort_by_key(|save| std::cmp::Reverse(save.saved_at_ms));
    sorted.into_iter().skip(limit).collect()
}

/// Turn a downloads listing entry into a [`SaveEntry`], if we wrote it.
fn parse_save_entry(item: &serde_json::Value, config: &MediaConfig) -> Option<SaveEntry> {
    let original_name = first_string(item, &["originalName", "original_name", "name"])?;
    let parsed = parse_save_name(&original_name)?;
    let filename = first_string(item, &["filename", "name"])?;
    let url = first_string(item, &["url", "publicUrl", "public_url"])
        .map(|url| absolutise(&url, config))
        .unwrap_or_else(|| {
            format!("{}/downloads/{}/{}", config.base_url, config.project, filename)
        });

    Some(SaveEntry {
        owner: parsed.owner,
        id: filename.clone(),
        private: false,
        filename,
        original_name,
        game_code: parsed.game_code,
        label: parsed.label,
        saved_at_ms: parsed.saved_at_ms,
        url,
        size: first_u64(item, &["size", "bytes"]).unwrap_or(0),
    })
}

/// Turn a private gallery item into a [`SaveEntry`], if we wrote it.
fn parse_private_entry(item: &serde_json::Value, config: &MediaConfig) -> Option<SaveEntry> {
    let original_name = first_string(item, &["originalName", "original_name", "name"])?;
    let parsed = parse_save_name(&original_name)?;
    let id = first_string(item, &["id", "assetId"])?;
    // The gallery already signs each URL, so no extra round trip is needed.
    let url = first_string(item, &["url"])
        .map(|url| absolutise(&url, config))
        .unwrap_or_default();

    Some(SaveEntry {
        // The gallery records an owner of its own; prefer it, since it is what
        // the storage service actually enforces.
        owner: first_string(item, &["ownerId", "owner_id"]).unwrap_or(parsed.owner),
        id,
        private: true,
        filename: String::new(),
        original_name,
        game_code: parsed.game_code,
        label: parsed.label,
        saved_at_ms: parsed.saved_at_ms,
        url,
        size: first_u64(item, &["size", "bytes"]).unwrap_or(0),
    })
}

/// List stored saves, optionally narrowed to one game.
/// Saves belonging to `owner`, newest first.
///
/// `owner` is not optional and does not come from the browser: a caller who
/// could choose it could read anyone's saves.
pub async fn list_saves(
    config: &MediaConfig,
    owner: &str,
    game_code: Option<&str>,
) -> SavesResponse {
    let Some(key) = config.key.as_deref() else {
        return SavesResponse {
            configured: false,
            limit: MAX_SAVES_PER_GAME,
            backend: None,
            saves: Vec::new(),
            error: None,
        };
    };

    let failed = |error: String| SavesResponse {
        configured: true,
        limit: MAX_SAVES_PER_GAME,
        backend: None,
        saves: Vec::new(),
        error: Some(error),
    };

    let capabilities = match capabilities(config).await {
        Ok(capabilities) => capabilities,
        Err(err) => return failed(err),
    };

    if capabilities.backend == SaveBackend::Private {
        return list_private_saves(config, owner, game_code, capabilities.backend).await;
    }

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => return failed(err.to_string()),
    };

    let response = match client
        .get(format!("{}/api/downloads", config.base_url))
        .header(KEY_HEADER, key)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return failed(err.to_string()),
    };

    if !response.status().is_success() {
        return failed(format!("storage returned {}", response.status()));
    }

    let value = match response.json::<serde_json::Value>().await {
        Ok(value) => value,
        Err(err) => return failed(err.to_string()),
    };

    let items = value
        .as_array()
        .cloned()
        .or_else(|| value.get("downloads").and_then(|v| v.as_array()).cloned())
        .or_else(|| value.get("items").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();

    let mut saves: Vec<SaveEntry> = items
        .iter()
        .filter(|item| {
            // Ignore other projects sharing the key.
            first_string(item, &["project"])
                .map(|project| project == config.project)
                .unwrap_or(true)
        })
        .filter_map(|item| parse_save_entry(item, config))
        .filter(|save| save.owner == owner)
        .filter(|save| game_code.is_none_or(|code| save.game_code == code))
        .collect();

    saves.sort_by_key(|save| std::cmp::Reverse(save.saved_at_ms));

    SavesResponse {
        configured: true,
        limit: MAX_SAVES_PER_GAME,
        backend: Some(SaveBackend::PublicDownloads),
        saves,
        error: None,
    }
}

/// List owner-scoped saves from the private gallery.
async fn list_private_saves(
    config: &MediaConfig,
    owner: &str,
    game_code: Option<&str>,
    backend: SaveBackend,
) -> SavesResponse {
    let failed = |error: String| SavesResponse {
        configured: true,
        limit: MAX_SAVES_PER_GAME,
        backend: None,
        saves: Vec::new(),
        error: Some(error),
    };

    let Some(key) = config.key.as_deref() else {
        return failed("storage is not configured".to_string());
    };

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(err) => return failed(err.to_string()),
    };

    let response = match client
        .get(format!(
            "{}/api/private-assets/{}/users/{}?view=saved&sort=newest&limit=100",
            config.base_url, config.project, owner
        ))
        .header(KEY_HEADER, key)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => return failed(err.to_string()),
    };

    if !response.status().is_success() {
        return failed(format!("storage returned {}", response.status()));
    }

    let value = match response.json::<serde_json::Value>().await {
        Ok(value) => value,
        Err(err) => return failed(err.to_string()),
    };

    let mut saves: Vec<SaveEntry> = value
        .get("items")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_private_entry(item, config))
                .filter(|save| save.owner == owner)
                .filter(|save| game_code.is_none_or(|code| save.game_code == code))
                .collect()
        })
        .unwrap_or_default();

    saves.sort_by_key(|save| std::cmp::Reverse(save.saved_at_ms));

    SavesResponse {
        configured: true,
        limit: MAX_SAVES_PER_GAME,
        backend: Some(backend),
        saves,
        error: None,
    }
}

/// Store a save state and prune the game's oldest beyond the limit.
///
/// `replace` overwrites an existing slot. Neither storage backend can rewrite
/// an object in place — the timestamp is part of the name — so an overwrite is
/// an upload followed by a delete, in that order. If the upload fails the old
/// save is still there, which is the only ordering that cannot lose progress.
pub async fn upload_save(
    config: &MediaConfig,
    owner: &str,
    game_code: &str,
    label: &str,
    saved_at_ms: u64,
    bytes: Vec<u8>,
    replace: Option<&str>,
) -> Result<SaveEntry, String> {
    let key = config.key.as_deref().ok_or("storage is not configured")?;

    if bytes.len() > MAX_SAVE_BYTES {
        return Err(format!(
            "That save is {:.1} MB compressed and the limit is {} MB.",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_SAVE_BYTES / (1024 * 1024)
        ));
    }

    let name = save_name(owner, game_code, saved_at_ms, label);
    let capabilities = capabilities(config).await?;

    let client = reqwest::Client::builder()
        .timeout(SAVE_UPLOAD_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;

    // The extension is inferred from the filename and checked against the
    // project's allowlist, so no `type` field is sent. Older builds sent one;
    // it is now a deprecated fallback.
    let url = match capabilities.backend {
        SaveBackend::Private => format!("{}/api/private-assets/{}", config.base_url, config.project),
        SaveBackend::PublicDownloads => {
            format!("{}/api/downloads/{}", config.base_url, config.project)
        }
    };

    // Rebuilt per attempt, because sending a multipart form consumes it.
    let build_form = |bytes: Vec<u8>| -> Result<reqwest::multipart::Form, String> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.clone())
            .mime_str("application/octet-stream")
            .map_err(|err| err.to_string())?;
        Ok(match capabilities.backend {
            SaveBackend::Private => reqwest::multipart::Form::new()
                .part("file", part)
                .text("ownerId", owner.to_string())
                .text("source", "uploaded")
                .text("saved", "true"),
            SaveBackend::PublicDownloads => reqwest::multipart::Form::new().part("file", part),
        })
    };

    // Retry once, but only when the connection was never established.
    //
    // A save is someone's progress and losing it to a momentary network blip is
    // the worst outcome here. Retrying more broadly would risk storing the save
    // twice: a timeout or a dropped response can mean the upload actually
    // landed. A failure to connect cannot, because nothing was sent.
    let mut response = None;
    for attempt in 0..2 {
        match client
            .post(&url)
            .header(KEY_HEADER, key)
            .multipart(build_form(bytes.clone())?)
            .send()
            .await
        {
            Ok(ok) => {
                response = Some(ok);
                break;
            }
            Err(err) if attempt == 0 && err.is_connect() => {
                eprintln!("Could not reach storage to save; retrying once: {err}");
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    let response = response.ok_or("could not reach storage to save")?;

    let status = response.status();
    // Read as text first: an over-size upload or an upstream proxy answers with
    // something that is not JSON, and `json()` would hide the reason.
    let body = response.text().await.map_err(|err| err.to_string())?;
    let value = serde_json::from_str::<serde_json::Value>(&body).unwrap_or_default();

    if !status.is_success() {
        return Err(format!("{status}: {}", describe_detail(&value, &body)));
    }

    let entry = match capabilities.backend {
        SaveBackend::Private => parse_private_entry(&value, config),
        SaveBackend::PublicDownloads => parse_save_entry(&value, config),
    }
    .ok_or_else(|| "the storage API returned a save we cannot read back".to_string())?;

    // Only now that the replacement is safely stored. A failure here leaves the
    // game one save over quota, which the next prune corrects; the alternative
    // is deleting progress that was never replaced.
    if let Some(id) = replace {
        if id != entry.id {
            if let Err(err) = delete_save(config, owner, id).await {
                eprintln!("Could not remove the overwritten save {id}: {}", err.message());
            }
        }
    }

    // Prune after a successful upload, never before: losing the oldest save to
    // make room for one that then fails to arrive would be the worst outcome.
    prune_saves(config, owner, game_code).await;
    Ok(entry)
}

/// What happened when someone claimed the saves from before accounts existed.
#[derive(Debug, Serialize)]
pub struct ClaimResult {
    pub claimed: usize,
    /// Saves that could not be moved, with the reason. Reported rather than
    /// swallowed: a half-finished migration the user cannot see is worse than
    /// one that says which saves are still where they were.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

/// Move saves written before accounts existed to a real owner.
///
/// Neither backend can rename an object, so each save is re-uploaded under the
/// new owner and the original is then removed — in that order, so a failure
/// leaves the save where it was rather than losing it. Timestamps and labels
/// are preserved, so the slots look unchanged to the player.
pub async fn claim_legacy_saves(
    config: &MediaConfig,
    new_owner: &str,
) -> Result<ClaimResult, String> {
    if new_owner == LEGACY_OWNER {
        return Err("that is already the legacy owner".to_string());
    }

    let listing = list_saves(config, LEGACY_OWNER, None).await;
    if let Some(error) = listing.error {
        return Err(error);
    }

    let mut result = ClaimResult {
        claimed: 0,
        failed: Vec::new(),
    };

    for save in listing.saves {
        let bytes = match proxy_fetch(config, &save.url).await {
            Ok((bytes, _)) => bytes,
            Err(err) => {
                result.failed.push(format!("{}: {err}", save.original_name));
                continue;
            }
        };

        if let Err(err) = upload_save(
            config,
            new_owner,
            &save.game_code,
            &save.label,
            save.saved_at_ms,
            bytes,
            None,
        )
        .await
        {
            result.failed.push(format!("{}: {err}", save.original_name));
            continue;
        }

        // Only now that the copy is stored.
        if let Err(err) = delete_save(config, LEGACY_OWNER, &save.id).await {
            eprintln!(
                "Claimed {} but could not remove the original: {}",
                save.original_name,
                err.message()
            );
        }
        result.claimed += 1;
    }

    Ok(result)
}

/// Render a `detail` field, which may be a string or a structured object.
///
/// Since 0.5.0 a rejected extension returns `{message, extension,
/// allowedExtensions, source}`; naming the allowed list in the message is what
/// makes a misconfigured project diagnosable from the UI.
fn describe_detail(value: &serde_json::Value, raw_body: &str) -> String {
    let Some(detail) = value.get("detail") else {
        let trimmed = raw_body.trim();
        return if trimmed.is_empty() {
            "upload rejected".to_string()
        } else {
            trimmed.chars().take(200).collect()
        };
    };

    if let Some(text) = detail.as_str() {
        return text.to_string();
    }

    let message = detail
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("upload rejected");
    let allowed = string_list(detail, "allowedExtensions");
    if allowed.is_empty() {
        message.to_string()
    } else {
        format!("{message} (allowed: {})", allowed.join(", "))
    }
}

/// Delete saves for `game_code` beyond [`MAX_SAVES_PER_GAME`], oldest first.
///
/// Returns how many were removed. Failures are reported but not fatal: an
/// over-quota vault is better than a failed save.
pub async fn prune_saves(config: &MediaConfig, owner: &str, game_code: &str) -> usize {
    // Scoped to one owner, so the limit is per person rather than shared: one
    // player filling their slots must not evict somebody else's progress.
    let listing = list_saves(config, owner, Some(game_code)).await;
    if listing.error.is_some() {
        return 0;
    }

    let mut removed = 0;
    for save in saves_to_evict(&listing.saves, MAX_SAVES_PER_GAME) {
        match delete_save(config, owner, &save.id).await {
            Ok(()) => removed += 1,
            Err(err) => eprintln!("Could not prune save {}: {}", save.id, err.message()),
        }
    }
    removed
}

/// Delete one stored save by its server-generated filename.
/// Delete one save, if it belongs to `owner`.
///
/// The id comes from a listing and the browser can echo back any id it likes,
/// so ownership is checked against a fresh listing rather than trusted. Neither
/// backend does this for us: the downloads project is shared, and a private
/// asset id is only scoped by the URL we choose to build.
/// Why a delete did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteError {
    /// No save with that id belongs to this owner. Also the answer when the
    /// save belongs to somebody else, so the two cannot be told apart.
    NotFound,
    Failed(String),
}

impl DeleteError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "No save by that id.".to_string(),
            Self::Failed(why) => why.clone(),
        }
    }

    /// True when the caller asked for something that is not there, rather than
    /// the storage service having failed.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound)
    }
}

pub async fn delete_save(config: &MediaConfig, owner: &str, id: &str) -> Result<(), DeleteError> {
    let key = config
        .key
        .as_deref()
        .ok_or_else(|| DeleteError::Failed("storage is not configured".to_string()))?;

    // Keep the id to a single path segment so it cannot escape the project.
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(DeleteError::Failed("that is not a valid save id".to_string()));
    }

    let listing = list_saves(config, owner, None).await;
    if let Some(error) = listing.error {
        return Err(DeleteError::Failed(error));
    }
    if !listing.saves.iter().any(|save| save.id == id) {
        // Deliberately the same answer as a save that does not exist: a
        // distinct one would confirm that somebody else's save is there.
        return Err(DeleteError::NotFound);
    }

    let capabilities = capabilities(config).await.map_err(DeleteError::Failed)?;
    let url = match capabilities.backend {
        SaveBackend::Private => format!(
            "{}/api/private-assets/{}/users/{}/{}",
            config.base_url, config.project, owner, id
        ),
        SaveBackend::PublicDownloads => format!(
            "{}/api/downloads/{}/{}",
            config.base_url, config.project, id
        ),
    };

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| DeleteError::Failed(err.to_string()))?;

    let response = client
        .delete(url)
        .header(KEY_HEADER, key)
        .send()
        .await
        .map_err(|err| DeleteError::Failed(err.to_string()))?;

    if response.status().is_success() {
        Ok(())
    } else if response.status() == reqwest::StatusCode::NOT_FOUND {
        // Already gone, which is the state the caller asked for.
        Err(DeleteError::NotFound)
    } else {
        Err(DeleteError::Failed(format!(
            "storage returned {}",
            response.status()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> MediaConfig {
        MediaConfig {
            base_url: "https://media.example".to_string(),
            vault: "tinybird".to_string(),
            project: "tinybird".to_string(),
            default_owner: "local".to_string(),
            key: Some("secret".to_string()),
        }
    }

    fn save(game_code: &str, saved_at_ms: u64) -> SaveEntry {
        SaveEntry {
            owner: "owner-1".to_string(),
            id: format!("{saved_at_ms}.savestate"),
            private: false,
            filename: format!("{saved_at_ms}.savestate"),
            original_name: save_name("owner-1", game_code, saved_at_ms, ""),
            game_code: game_code.to_string(),
            label: String::new(),
            saved_at_ms,
            url: "https://media.example/downloads/tinybird/x.bin".to_string(),
            size: 1024,
        }
    }

    #[test]
    fn save_names_round_trip() {
        let name = save_name("u42", "BPRE", 1_787_570_960_801, "route4");
        assert_eq!(name, "tb_u42_BPRE_1787570960801_route4.savestate");

        let parsed = parse_save_name(&name).expect("should parse");
        assert_eq!(parsed.owner, "u42");
        assert_eq!(parsed.game_code, "BPRE");
        assert_eq!(parsed.saved_at_ms, 1_787_570_960_801);
        assert_eq!(parsed.label, "route4");
    }

    /// A real subject claim, to be sure the length and shape survive.
    #[test]
    fn a_real_owner_id_round_trips() {
        let owner = "0feeca011bc64a088afb8d10966a9f46";
        let name = save_name(owner, "BZME", 1_787_600_000_000, "minish");
        let parsed = parse_save_name(&name).expect("should parse");
        assert_eq!(parsed.owner, owner);
        assert_eq!(parsed.game_code, "BZME");
    }

    /// Saves written before accounts existed have no owner field in the name.
    #[test]
    fn saves_from_before_accounts_belong_to_the_legacy_owner() {
        let parsed = parse_save_name("tb_BPRE_1787570960801_route4.savestate")
            .expect("should still parse");
        assert_eq!(parsed.owner, LEGACY_OWNER);
        assert_eq!(parsed.game_code, "BPRE");
        assert_eq!(parsed.saved_at_ms, 1_787_570_960_801);
        assert_eq!(parsed.label, "route4");
    }

    #[test]
    fn a_legacy_save_without_a_label_still_parses() {
        let parsed = parse_save_name("tb_AFXE_42.savestate").expect("should parse");
        assert_eq!(parsed.owner, LEGACY_OWNER);
        assert_eq!(parsed.game_code, "AFXE");
        assert_eq!(parsed.saved_at_ms, 42);
        assert_eq!(parsed.label, "");
    }

    /// One person's save must not be readable as another's.
    #[test]
    fn owners_are_distinguished_by_name() {
        let mine = save_name("me", "BPRE", 1, "x");
        let yours = save_name("you", "BPRE", 1, "x");
        assert_ne!(mine, yours);
        assert_eq!(parse_save_name(&mine).unwrap().owner, "me");
        assert_eq!(parse_save_name(&yours).unwrap().owner, "you");
    }

    /// An owner cannot smuggle separators into the name to pose as someone else.
    #[test]
    fn an_owner_cannot_forge_another_owners_name() {
        let forged = save_name("me_BPRE_1", "BPRE", 2, "");
        let parsed = parse_save_name(&forged).expect("should parse");
        assert_ne!(parsed.owner, "me", "the underscores must not split the field");
        assert_eq!(parsed.saved_at_ms, 2, "the real timestamp must win");
    }

    #[test]
    fn save_names_round_trip_without_a_label() {
        let name = save_name("u42", "AFXE", 42, "");
        assert_eq!(name, "tb_u42_AFXE_42.savestate");
        let parsed = parse_save_name(&name).expect("should parse");
        assert_eq!(parsed.game_code, "AFXE");
        assert_eq!(parsed.label, "");
    }

    #[test]
    fn save_names_drop_characters_that_would_need_escaping() {
        let name = save_name("o/w", "BP/RE", 1, "a b..c/d");
        assert!(!name.contains('/'), "got {name}");
        assert!(!name.contains(' '), "got {name}");
        assert!(parse_save_name(&name).is_some());
    }

    #[test]
    fn saves_written_under_the_old_extension_still_parse() {
        // The project used to allow `.bin`; those saves must keep listing.
        let parsed = parse_save_name("tb_BPRE_1787570960801_route4.bin").expect("should parse");
        assert_eq!(parsed.game_code, "BPRE");
        assert_eq!(parsed.label, "route4");
    }

    #[test]
    fn foreign_downloads_are_not_treated_as_saves() {
        // The project can hold other things; only our own names are ours.
        assert!(parse_save_name("screenshot.bin").is_none());
        assert!(parse_save_name("tb_BPRE.bin").is_none(), "no timestamp");
        assert!(parse_save_name("tb_BPRE_notanumber.bin").is_none());
        assert!(parse_save_name("tb_owner_BPRE_notanumber.bin").is_none());
        assert!(parse_save_name("tb_BPRE_1.txt").is_none(), "wrong extension");
        assert!(parse_save_name("other_BPRE_1.bin").is_none(), "wrong prefix");
    }

    #[test]
    fn the_proxy_only_accepts_the_configured_storage_host() {
        let config = config();
        assert!(is_storage_url(&config, "https://media.example/downloads/tinybird/a.bin"));
        assert!(is_storage_url(&config, "https://media.example/vault/a.png"));

        // An open proxy could be pointed at anything reachable from the server.
        assert!(!is_storage_url(&config, "http://169.254.169.254/latest/meta-data/"));
        assert!(!is_storage_url(&config, "https://evil.example/x"));
        assert!(!is_storage_url(&config, "file:///etc/passwd"));
    }

    #[test]
    fn a_lookalike_host_cannot_pass_the_proxy_check() {
        // "https://media.example.evil.com/..." shares a prefix with the base.
        assert!(!is_storage_url(&config(), "https://media.example.evil.com/a.bin"));
    }

    fn caps(private_enabled: bool, allowed_private: &[&str]) -> Capabilities {
        Capabilities::decide(
            "vault-id".to_string(),
            private_enabled,
            vec![".savestate".to_string()],
            allowed_private.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// The cache is what keeps an overwrite down to a handful of requests.
    ///
    /// Pointed at a port nothing listens on, so a call that succeeds proves
    /// nothing went out over the network.
    #[tokio::test]
    async fn a_cached_configuration_is_reused_without_asking_again() {
        forget_capabilities();
        let nowhere = MediaConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            ..config()
        };
        assert!(
            capabilities(&nowhere).await.is_err(),
            "an empty cache must actually try the service"
        );

        *capabilities_cache().lock().unwrap() =
            Some((std::time::Instant::now(), caps(true, &[".savestate"])));
        let cached = capabilities(&nowhere).await.expect("should come from the cache");
        assert_eq!(cached.backend, SaveBackend::Private);

        forget_capabilities();
        assert!(
            capabilities(&nowhere).await.is_err(),
            "forgetting must send the next read back to the service"
        );
    }

    #[tokio::test]
    async fn a_stale_configuration_is_not_reused() {
        forget_capabilities();
        let nowhere = MediaConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            ..config()
        };
        // Older than the TTL, so it must not be trusted.
        let stale = std::time::Instant::now()
            .checked_sub(CAPABILITIES_TTL * 2)
            .expect("clock should support this");
        *capabilities_cache().lock().unwrap() = Some((stale, caps(true, &[".savestate"])));

        assert!(capabilities(&nowhere).await.is_err(), "stale must be refetched");
        forget_capabilities();
    }

    #[test]
    fn private_storage_is_used_when_the_project_allows_our_extension() {
        // Saves are someone's progress; owner-scoped signed URLs beat public
        // ones whenever the project permits them.
        assert_eq!(caps(true, &[".png", ".savestate"]).backend, SaveBackend::Private);
    }

    #[test]
    fn public_downloads_are_the_fallback() {
        // Private assets exist but do not accept our extension.
        assert_eq!(
            caps(true, &[".png", ".mp4"]).backend,
            SaveBackend::PublicDownloads
        );
        // Private assets disabled entirely.
        assert_eq!(
            caps(false, &[".savestate"]).backend,
            SaveBackend::PublicDownloads
        );
        assert_eq!(caps(true, &[]).backend, SaveBackend::PublicDownloads);
    }

    #[test]
    fn extension_matching_ignores_case() {
        assert_eq!(caps(true, &[".SAVESTATE"]).backend, SaveBackend::Private);
    }

    #[test]
    fn a_structured_rejection_names_the_allowed_extensions() {
        // 0.5.0 returns an object here; surfacing the list is what makes a
        // misconfigured project diagnosable without reading server logs.
        let value = json!({
            "detail": {
                "message": "Extension .txt is not allowed for this project",
                "extension": ".txt",
                "allowedExtensions": [".sav", ".save", ".savestate"],
                "source": "projectConfig"
            }
        });
        let described = describe_detail(&value, "");
        assert!(described.contains("not allowed"), "got {described}");
        assert!(described.contains(".savestate"), "got {described}");
    }

    #[test]
    fn a_plain_string_detail_still_renders() {
        let value = json!({ "detail": "Invalid media API key" });
        assert_eq!(describe_detail(&value, ""), "Invalid media API key");
    }

    #[test]
    fn a_non_json_body_falls_back_to_its_text() {
        let value = serde_json::Value::Null;
        assert_eq!(describe_detail(&value, "  Bad Gateway  "), "Bad Gateway");
        assert_eq!(describe_detail(&value, ""), "upload rejected");
    }

    #[test]
    fn a_private_gallery_item_parses() {
        // Shape copied from the live API on 2026-08-24.
        let item = json!({
            "id": "c477d2a8b5db434b8c801663b7703d43",
            "ownerId": "local",
            "originalName": "tb_local_BPRE_1787570960801_route4.savestate",
            "contentType": "application/octet-stream",
            "source": "uploaded",
            "saved": true,
            "size": 5479845,
            "url": "https://media.0xstash.dev/private/c477d2a8?expires=1&signature=x"
        });

        let entry = parse_private_entry(&item, &config()).expect("should parse");
        assert!(entry.private);
        assert_eq!(entry.owner, "local", "the gallery's own owner field wins");
        assert_eq!(entry.id, "c477d2a8b5db434b8c801663b7703d43");
        assert_eq!(entry.game_code, "BPRE");
        assert_eq!(entry.size, 5_479_845);
        assert!(entry.url.contains("signature="), "signed URL must be kept");
    }

    #[test]
    fn the_size_cap_matches_the_storage_limit() {
        // The API rejects anything larger with a 413; failing here first lets
        // the page say something useful instead of relaying a status code.
        assert_eq!(MAX_SAVE_BYTES, 16_777_216);
    }

    #[test]
    fn nothing_is_evicted_below_the_limit() {
        let saves: Vec<_> = (0..MAX_SAVES_PER_GAME as u64).map(|i| save("BPRE", i)).collect();
        assert!(saves_to_evict(&saves, MAX_SAVES_PER_GAME).is_empty());
    }

    #[test]
    fn the_oldest_saves_are_evicted_first() {
        let saves: Vec<_> = (0..8u64).map(|i| save("BPRE", i * 1000)).collect();
        let evicted = saves_to_evict(&saves, MAX_SAVES_PER_GAME);

        assert_eq!(evicted.len(), 3);
        // Oldest three: 0, 1000, 2000.
        let mut stamps: Vec<u64> = evicted.iter().map(|s| s.saved_at_ms).collect();
        stamps.sort();
        assert_eq!(stamps, vec![0, 1000, 2000]);
    }

    #[test]
    fn eviction_does_not_depend_on_listing_order() {
        // The API does not promise an order, and sorting wrongly would delete
        // the newest save instead of the oldest.
        let saves = vec![
            save("BPRE", 500),
            save("BPRE", 9000),
            save("BPRE", 100),
            save("BPRE", 7000),
            save("BPRE", 3000),
            save("BPRE", 8000),
        ];
        let evicted = saves_to_evict(&saves, MAX_SAVES_PER_GAME);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].saved_at_ms, 100, "the oldest must go");
    }

    #[test]
    fn a_limit_of_zero_evicts_everything() {
        let saves = vec![save("BPRE", 1), save("BPRE", 2)];
        assert_eq!(saves_to_evict(&saves, 0).len(), 2);
    }

    #[test]
    fn parses_a_real_downloads_listing_entry() {
        // Copied from the live API on 2026-08-24.
        let item = json!({
            "id": "4ec04bd66d0d42f098e605dc2eec9964",
            "project": "tinybird",
            "filename": "4ec04bd66d0d42f098e605dc.bin",
            "originalName": "tb_BPRE_1787570960801_route4.bin",
            "type": ".bin",
            "size": 8192,
            "modifiedAt": "2026-08-24T18:41:30.363705+00:00",
            "url": "/downloads/tinybird/4ec04bd66d0d42f098e605dc.bin",
            "uploaded": true
        });

        let entry = parse_save_entry(&item, &config()).expect("should parse");
        assert_eq!(entry.game_code, "BPRE");
        assert_eq!(entry.label, "route4");
        assert_eq!(entry.filename, "4ec04bd66d0d42f098e605dc.bin");
        assert_eq!(entry.size, 8192);
        assert_eq!(
            entry.url,
            "https://media.example/downloads/tinybird/4ec04bd66d0d42f098e605dc.bin",
            "the relative URL from the API must be made absolute"
        );
    }

    fn default_vault_config() -> MediaConfig {
        MediaConfig {
            vault: DEFAULT_VAULT.to_string(),
            ..config()
        }
    }

    #[test]
    fn a_save_name_cannot_escape_its_project() {
        // delete_save takes a name from a listing, which the browser can echo
        // back; a path separator must never reach the storage URL.
        let name = save_name("../..", "../../etc", 1, "passwd");
        assert!(!name.contains(".."), "got {name}");
    }

    #[test]
    fn a_named_vault_uses_the_slug_path() {
        assert_eq!(
            config().asset_url("game.gba"),
            "https://media.example/tinybird/game.gba"
        );
    }

    #[test]
    fn the_default_vault_uses_the_vault_path() {
        assert_eq!(
            default_vault_config().asset_url("game.gba"),
            "https://media.example/vault/game.gba"
        );
    }

    #[test]
    fn a_missing_key_means_unconfigured() {
        let without_key = MediaConfig {
            key: None,
            ..config()
        };
        assert!(!without_key.is_configured());
        assert!(config().is_configured());
    }

    #[test]
    fn parses_a_bare_array_listing() {
        let value = json!([{ "name": "a.gba", "size": 1024 }]);
        let assets = parse_asset_list(&value, &config());

        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].name, "a.gba");
        assert_eq!(assets[0].size, Some(1024));
        assert_eq!(assets[0].url, "https://media.example/tinybird/a.gba");
    }

    /// A response copied verbatim from the live API on 2026-08-24.
    ///
    /// The names are under `originalName` and the public URL is built from the
    /// asset id, not the filename. An earlier parser looked only for `name` and
    /// silently returned an empty library.
    #[test]
    fn parses_the_real_0xstash_listing_shape() {
        let value = json!([{
            "id": "56ec1f00-ad28-4852-a155-8801f6650143",
            "originalName": "1c65fbd6a295cf70587cf50105887eea.jpg",
            "contentType": "image/jpeg",
            "kind": "image",
            "size": 38825,
            "createdAt": "2026-08-24T09:48:36.357289+00:00",
            "vault": { "id": "default", "name": "Default", "slug": "default", "path": "default" },
            "url": "https://media.0xstash.dev/vault/56ec1f00ad284852a155.jpg"
        }]);

        let assets = parse_asset_list(&value, &default_vault_config());
        assert_eq!(assets.len(), 1, "the real listing shape must parse");
        assert_eq!(assets[0].name, "1c65fbd6a295cf70587cf50105887eea.jpg");
        assert_eq!(assets[0].size, Some(38825));
        assert_eq!(assets[0].content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(
            assets[0].url,
            "https://media.0xstash.dev/vault/56ec1f00ad284852a155.jpg",
            "the served URL is derived from the asset id, so it must be taken              from the response rather than rebuilt from the name"
        );
    }

    #[test]
    fn parses_listings_wrapped_under_any_of_the_usual_keys() {
        // The listing shape is not pinned by the OpenAPI document, so a rename
        // upstream must not blank the library.
        for key in ["assets", "images", "items", "data"] {
            let value = json!({ key: [{ "name": "a.gba" }] });
            assert_eq!(
                parse_asset_list(&value, &config()).len(),
                1,
                "failed for wrapper key {key}"
            );
        }
    }

    #[test]
    fn an_unrecognised_shape_yields_an_empty_library_not_an_error() {
        let value = json!({ "unexpected": true });
        assert!(parse_asset_list(&value, &config()).is_empty());
    }

    #[test]
    fn entries_without_a_name_are_skipped() {
        let value = json!([{ "size": 10 }, { "name": "ok.gba" }]);
        let assets = parse_asset_list(&value, &config());
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].name, "ok.gba");
    }

    #[test]
    fn a_relative_url_from_the_api_is_made_absolute() {
        let value = json!([{ "name": "a.gba", "url": "/media/abc.gba" }]);
        let assets = parse_asset_list(&value, &config());
        assert_eq!(assets[0].url, "https://media.example/media/abc.gba");
    }

    #[test]
    fn an_absolute_url_from_the_api_is_kept() {
        let value = json!([{ "name": "a.gba", "url": "https://cdn.example/x.gba" }]);
        let assets = parse_asset_list(&value, &config());
        assert_eq!(assets[0].url, "https://cdn.example/x.gba");
    }

    #[test]
    fn content_types_are_guessed_from_the_extension() {
        assert_eq!(guess_content_type("shot.PNG"), "image/png");
        assert_eq!(guess_content_type("clip.mp4"), "video/mp4");
        // The API refuses this, and a clear rejection beats a wrong label.
        assert_eq!(guess_content_type("save.state"), "application/octet-stream");
        assert_eq!(guess_content_type("noextension"), "application/octet-stream");
    }

    #[test]
    fn rom_names_are_recognised_case_insensitively() {
        assert!(is_rom_name("Game.GBA"));
        assert!(is_rom_name("dump.bin"));
        assert!(is_rom_name("x.agb"));
        assert!(!is_rom_name("save.state"));
        assert!(!is_rom_name("art.png"));
    }

    #[test]
    fn an_unconfigured_library_reports_no_error() {
        // "Not set up" is a different state from "broken", and the page shows
        // a setup hint for one and a failure for the other.
        let response = LibraryResponse::unconfigured("tinybird".to_string());
        assert!(!response.configured);
        assert!(response.error.is_none());

        let failed = LibraryResponse::failed("tinybird".to_string(), "boom");
        assert!(failed.configured);
        assert_eq!(failed.error.as_deref(), Some("boom"));
    }

    #[test]
    fn the_library_response_serializes_without_null_noise() {
        let json = serde_json::to_string(&LibraryResponse::unconfigured("v".into())).unwrap();
        assert!(!json.contains("null"), "got {json}");
        assert!(json.contains("\"configured\":false"));
    }

    #[test]
    fn base_urls_lose_a_trailing_slash() {
        // Otherwise every built URL would contain a double slash.
        temp_env(&[("TINYBIRD_MEDIA_URL", Some("https://media.example/"))], || {
            assert_eq!(MediaConfig::from_env().base_url, "https://media.example");
        });
    }

    #[test]
    fn blank_environment_values_fall_back_to_defaults() {
        temp_env(
            &[
                ("TINYBIRD_MEDIA_URL", Some("   ")),
                ("TINYBIRD_MEDIA_KEY", Some("")),
            ],
            || {
                let config = MediaConfig::from_env();
                assert_eq!(config.base_url, DEFAULT_BASE_URL);
                assert!(!config.is_configured(), "blank key must not count as set");
            },
        );
    }

    /// Serialises the tests that mutate the environment.
    ///
    /// The environment is process-wide but the test harness is threaded, so two
    /// tests setting the same variable would otherwise read each other's value
    /// and fail at random.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Set environment variables for the duration of `body`.
    ///
    /// Takes them all at once rather than nesting: the lock is not reentrant,
    /// and a nested call would deadlock rather than fail.
    fn temp_env(vars: &[(&str, Option<&str>)], body: impl FnOnce()) {
        // A panicking test poisons the lock; it only protects ordering, so
        // carrying on with it is correct.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        let restore: Vec<_> = vars
            .iter()
            .map(|(key, value)| {
                let previous = env::var(key).ok();
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
                (*key, previous)
            })
            .collect();

        body();

        for (key, previous) in restore {
            match previous {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}
