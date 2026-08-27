use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tinybird_addons::SNAPSHOT_SCHEMA_VERSION;
use tokio::net::TcpListener;

mod auth;
mod dotenv;
mod lobby;
mod media;
mod sprites;

use auth::{AuthConfig, AuthError, Sessions};
use lobby::Lobby;
use media::MediaConfig;

const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEFAULT_PORT: u16 = 8877;
const DEFAULT_SNAPSHOT_PATH: &str = "stream-data/current-game.json";
const DEFAULT_SPRITE_DIR: &str = "stream-data/pokemon-sprites";
/// Addon manifests, served to the page because the browser has no disk.
const DEFAULT_ADDON_DIR: &str = "addons";
/// Local ROM folder offered alongside the vault, so the page is usable before
/// any storage key exists.
const DEFAULT_ROM_DIR: &str = "roms";
/// BIOS image served to the browser.
///
/// Games decompress their graphics through BIOS SWI calls, and the core's
/// high-level stand-ins are not accurate enough for all of them — Pokemon
/// FireRed renders with missing sprites and corrupt tiles without a real dump.
/// The desktop app already loads this file; the browser needs it too.
const DEFAULT_BIOS_PATH: &str = "gba_bios.bin";
/// Where `cargo build --target wasm32-unknown-unknown --release` leaves the module.
const DEFAULT_WASM_PATH: &str = "target/wasm32-unknown-unknown/release/tinybird_wasm.wasm";
/// Largest save state the browser may push to the vault.
const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

const INDEX_HTML: &str = include_str!("assets/index.html");
const OVERLAY_HTML: &str = include_str!("assets/overlay.html");
/// Shown in place of the overlay while it is switched off.
const OVERLAY_OFF_HTML: &str = concat!(
    "<!doctype html><meta charset=\"utf-8\"><title>Overlay is off</title>",
    "<style>body{background:#06090f;color:#7c8aa3;font:13px/1.6 ui-monospace,monospace;",
    "margin:0;display:grid;place-items:center;min-height:100vh;text-align:center;padding:24px}",
    "b{color:#ff9f1c}code{color:#2ec4b6}</style>",
    "<div><p><b>The stream overlay is switched off.</b></p>",
    "<p>It reads the desktop app's export rather than the emulator in the page,<br>",
    "and has had far less use than the rest of this. It is being reworked.</p>",
    "<p>To switch it on anyway, start the server with <code>TINYBIRD_WEB_OVERLAY=1</code>.</p></div>",
);
const PLAY_HTML: &str = include_str!("assets/play.html");
const APP_JS: &str = include_str!("assets/app.js");
const PLAY_JS: &str = include_str!("assets/play.js");
const HOME_JS: &str = include_str!("assets/home.js");
const TINYBIRD_JS: &str = include_str!("assets/tinybird.js");
const SAVEFORMAT_JS: &str = include_str!("assets/saveformat.js");
const PACING_JS: &str = include_str!("assets/pacing.js");
const LOBBY_JS: &str = include_str!("assets/lobby.js");
const CONTROLS_JS: &str = include_str!("assets/controls.js");
const STYLES_CSS: &str = include_str!("assets/styles.css");
const CONSOLE_CSS: &str = include_str!("assets/console.css");

#[derive(Clone, Debug)]
struct AppState {
    snapshot_path: PathBuf,
    sprite_dir: PathBuf,
    wasm_path: PathBuf,
    rom_dir: PathBuf,
    bios_path: PathBuf,
    /// Whether `/overlay` serves the overlay or the note explaining why not.
    overlay_enabled: bool,
    /// Directory of addon manifests served to the page.
    addon_dir: PathBuf,
    media: MediaConfig,
    auth: AuthConfig,
    /// Signed-in browsers. Shared, because the state is cloned per request.
    sessions: Arc<Sessions>,
    /// Open rooms. Shared for the same reason.
    lobby: Arc<Lobby>,
    /// Whether to serve ROMs off this machine's disk.
    ///
    /// On by default, because running the server for yourself is the ordinary
    /// case and reading your own ROM folder is the point of it. Off for
    /// anything reachable from the internet: that folder holds copyrighted
    /// cartridge dumps, and `/local/{path}` would hand them to whoever found
    /// the URL.
    serve_local_roms: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load `.env` before reading any configuration so a fresh checkout works
    // with `cp .env.example .env` and nothing else.
    dotenv::load(".env");

    let config = Config::from_env_and_args(env::args().skip(1).collect());
    let media = MediaConfig::from_env();
    let storage_configured = media.is_configured();
    let storage_label = format!("{} vault \"{}\"", media.base_url, media.vault);
    let save_project = media.project.clone();
    let wasm_ready = config.wasm_path.is_file();
    let wasm_display = config.wasm_path.display().to_string();
    let bios_ready = config.bios_path.is_file();
    let bios_display = config.bios_path.display().to_string();

    let state = AppState {
        snapshot_path: config.snapshot_path,
        sprite_dir: config.sprite_dir,
        overlay_enabled: config.overlay_enabled,
        addon_dir: config.addon_dir,
        wasm_path: config.wasm_path,
        rom_dir: config.rom_dir,
        bios_path: config.bios_path,
        media,
        auth: AuthConfig::from_env(),
        sessions: Arc::new(Sessions::new()),
        lobby: Arc::new(Lobby::new()),
        serve_local_roms: local_roms_enabled(),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/overlay", get(overlay))
        .route("/overlay/{section}", get(overlay))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/play", get(play))
        .route("/play.js", get(play_js))
        .route("/home.js", get(home_js))
        .route("/tinybird.js", get(tinybird_js))
        .route("/saveformat.js", get(saveformat_js))
        .route("/pacing.js", get(pacing_js))
        .route("/lobby.js", get(lobby_js))
        .route("/controls.js", get(controls_js))
        .route("/console.css", get(console_css))
        .route("/tinybird.wasm", get(wasm_module))
        .route("/bios", get(bios_image))
        .route("/api/library", get(library))
        .route("/api/local", get(local_roms))
        .route("/local/{*path}", get(local_rom_file))
        .route(
            "/api/library/upload",
            axum::routing::post(library_upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/api/saves",
            get(list_saves).post(upload_save).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/saves/claim", axum::routing::post(claim_saves))
        .route("/api/saves/{filename}", axum::routing::delete(delete_save))
        .route("/api/auth/register", axum::routing::post(auth_register))
        .route("/api/auth/login", axum::routing::post(auth_login))
        .route("/api/auth/logout", axum::routing::post(auth_logout))
        .route("/api/auth/me", get(auth_me))
        .route("/api/lobby", axum::routing::post(create_room))
        .route("/api/lobby/ws", get(lobby_socket))
        .route("/api/proxy", get(storage_proxy))
        .route("/api/health", get(health))
        .route("/api/snapshot", get(snapshot))
        .route("/api/addons", get(addon_manifests))
        .route("/sprites/{species_id}", get(sprite_png))
        .with_state(state);

    let addr = SocketAddr::new(config.host, config.port);
    // A port clash is the single most common way this fails to start, usually
    // because an earlier copy is still running. The raw OS error names neither
    // the port nor the fix, so say both.
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("Port {} is already in use.", config.port);
            eprintln!();
            eprintln!("Something else is listening on {addr} — most likely another");
            eprintln!("copy of tinybird-web. Either stop it, or use a different port:");
            eprintln!();
            eprintln!("  cargo run -p tinybird-web -- --port 8878");
            eprintln!();
            #[cfg(windows)]
            eprintln!("  To find it:  netstat -ano | findstr :{}", config.port);
            #[cfg(not(windows))]
            eprintln!("  To find it:  lsof -i :{}", config.port);
            std::process::exit(1);
        }
        Err(err) => return Err(err.into()),
    };

    println!("tinyBird web listening on http://{addr}");
    println!("  home:       http://{addr}/");
    println!("  play:       http://{addr}/play");
    println!("  full:       http://{addr}/overlay/full");
    println!("  party:      http://{addr}/overlay/party");
    println!("  area:       http://{addr}/overlay/area");
    println!("  battle:     http://{addr}/overlay/battle");
    if storage_configured {
        println!("  storage:    {storage_label}");
        println!("  saves:      project \"{save_project}\", {} per game", media::MAX_SAVES_PER_GAME);
    } else {
        println!("  storage:    off (set TINYBIRD_MEDIA_KEY in .env to enable the vault)");
    }
    if !bios_ready {
        println!();
        println!("  No {bios_display}: games that decompress graphics through BIOS");
        println!("  calls will render incorrectly. Copy a GBA BIOS dump there.");
    }
    if !wasm_ready {
        println!();
        println!("  {wasm_display} is missing, so /play cannot boot. Build it with:");
        println!("  cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release");
    }

    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
struct Config {
    host: IpAddr,
    port: u16,
    snapshot_path: PathBuf,
    sprite_dir: PathBuf,
    overlay_enabled: bool,
    addon_dir: PathBuf,
    wasm_path: PathBuf,
    rom_dir: PathBuf,
    bios_path: PathBuf,
}

impl Config {
    fn from_env_and_args(args: Vec<String>) -> Self {
        let mut host = env::var("TINYBIRD_WEB_HOST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_HOST);
        let mut port = env::var("TINYBIRD_WEB_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let mut snapshot_path = env::var("TINYBIRD_WEB_SNAPSHOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SNAPSHOT_PATH));
        let mut sprite_dir = env::var("TINYBIRD_WEB_SPRITES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_SPRITE_DIR));
        // Opt-in: an unfinished feature that silently does the wrong thing
        // is worse than one that is not there.
        let addon_dir = env::var("TINYBIRD_ADDONS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_ADDON_DIR));
        let overlay_enabled = env::var("TINYBIRD_WEB_OVERLAY")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let mut wasm_path = env::var("TINYBIRD_WEB_WASM")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_WASM_PATH));
        let mut rom_dir = env::var("TINYBIRD_WEB_ROMS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_ROM_DIR));
        let mut bios_path = env::var("TINYBIRD_WEB_BIOS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_BIOS_PATH));

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--host" => {
                    if let Some(value) = iter.next().and_then(|value| value.parse().ok()) {
                        host = value;
                    }
                }
                "--port" => {
                    if let Some(value) = iter.next().and_then(|value| value.parse().ok()) {
                        port = value;
                    }
                }
                "--snapshot" => {
                    if let Some(value) = iter.next() {
                        snapshot_path = PathBuf::from(value);
                    }
                }
                "--sprites" => {
                    if let Some(value) = iter.next() {
                        sprite_dir = PathBuf::from(value);
                    }
                }
                "--wasm" => {
                    if let Some(value) = iter.next() {
                        wasm_path = PathBuf::from(value);
                    }
                }
                "--roms" => {
                    if let Some(value) = iter.next() {
                        rom_dir = PathBuf::from(value);
                    }
                }
                "--bios" => {
                    if let Some(value) = iter.next() {
                        bios_path = PathBuf::from(value);
                    }
                }
                _ => {}
            }
        }

        Self {
            host,
            port,
            snapshot_path,
            sprite_dir,
            overlay_enabled,
            addon_dir,
            wasm_path,
            rom_dir,
            bios_path,
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// The stream overlay, when it is switched on.
///
/// Off by default. The overlay pages still read the desktop app's JSON export
/// rather than the emulator in the page beside them, so on a server run by
/// itself they show a snapshot that never changes — and they have had far less
/// use than the rest of this. Rather than leave a route that quietly
/// misbehaves, it says so, and `TINYBIRD_WEB_OVERLAY=1` brings it back for
/// anyone working on it.
async fn overlay(State(state): State<AppState>) -> Response {
    if !state.overlay_enabled {
        return Html(OVERLAY_OFF_HTML).into_response();
    }
    Html(OVERLAY_HTML).into_response()
}

async fn play() -> Html<&'static str> {
    Html(PLAY_HTML)
}

async fn play_js() -> Response {
    static_text(PLAY_JS, "application/javascript; charset=utf-8")
}

async fn home_js() -> Response {
    static_text(HOME_JS, "application/javascript; charset=utf-8")
}

async fn tinybird_js() -> Response {
    static_text(TINYBIRD_JS, "application/javascript; charset=utf-8")
}

async fn saveformat_js() -> Response {
    static_text(SAVEFORMAT_JS, "application/javascript; charset=utf-8")
}

async fn pacing_js() -> Response {
    static_text(PACING_JS, "application/javascript; charset=utf-8")
}

async fn controls_js() -> Response {
    static_text(CONTROLS_JS, "application/javascript; charset=utf-8")
}

async fn lobby_js() -> Response {
    static_text(LOBBY_JS, "application/javascript; charset=utf-8")
}

async fn console_css() -> Response {
    static_text(CONSOLE_CSS, "text/css; charset=utf-8")
}

/// Serve the emulator module.
///
/// Read from disk rather than embedded with `include_bytes!`, so the workspace
/// still builds for anyone who has not installed the `wasm32` target.
async fn wasm_module(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&state.wasm_path).await {
        Ok(bytes) => binary(StatusCode::OK, bytes, "application/wasm"),
        Err(_) => json_error(
            StatusCode::NOT_FOUND,
            &format!(
                "{} not found. Build it with: cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release",
                state.wasm_path.display()
            ),
        ),
    }
}

/// List the vault. The API key stays on this side; the page only ever receives
/// public asset URLs.
async fn library(State(state): State<AppState>) -> Response {
    let response = media::list_assets(&state.media).await;
    match serde_json::to_string(&response) {
        Ok(json) => text(StatusCode::OK, json, "application/json; charset=utf-8"),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

/// Push a file from the browser into the vault, such as a save state.
async fn library_upload(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    if !state.media.is_configured() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Storage is off. Set TINYBIRD_MEDIA_KEY in .env and restart.",
        );
    }

    let mut filename = String::new();
    let mut content_type = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() != Some("file") {
            continue;
        }
        filename = field
            .file_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "upload.bin".to_string());
        // The storage API verifies that the bytes match the declared type, so
        // the browser's own type has to reach it rather than being replaced.
        content_type = field
            .content_type()
            .map(|value| value.to_string())
            .unwrap_or_default();
        bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
        break;
    }

    if bytes.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "The request had no file to upload.");
    }

    let name = sanitize_filename(&filename);
    let content_type = if content_type.is_empty() {
        media::guess_content_type(&name).to_string()
    } else {
        content_type
    };

    match media::upload_asset(&state.media, &name, &content_type, bytes).await {
        Ok(asset) => text(
            StatusCode::CREATED,
            serde_json::to_string(&asset).unwrap_or_else(|_| "{}".to_string()),
            "application/json; charset=utf-8",
        ),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

/// Reduce an uploaded name to something safe to put in a URL path.
///
/// The browser controls this string, so directory separators and traversal have
/// to go before it reaches the storage API.
fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "upload.bin".to_string()
    } else {
        cleaned
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    text(
        status,
        format!("{{\"error\":\"{}\"}}", escape_json_string(message)),
        "application/json; charset=utf-8",
    )
}

/// Serve the BIOS image so the browser build matches the desktop one.
///
/// Never bundled: a BIOS dump is copyrighted, is gitignored, and is read from
/// the machine running the server. A 404 here is normal and the page degrades
/// to high-level emulation with a warning.
async fn bios_image(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&state.bios_path).await {
        Ok(bytes) => binary(StatusCode::OK, bytes, "application/octet-stream"),
        Err(_) => json_error(
            StatusCode::NOT_FOUND,
            "No BIOS image. Put gba_bios.bin beside the server for full accuracy.",
        ),
    }
}

/// Fetch storage bytes for the browser.
///
/// The storage CDN sends no `Access-Control-Allow-Origin`, so a page cannot
/// fetch a ROM or a save directly. This relays the bytes; `media::proxy_fetch`
/// restricts it to the configured host so it cannot be used to reach anything
/// else on the network.
async fn storage_proxy(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let Some(url) = params.get("url") else {
        return json_error(StatusCode::BAD_REQUEST, "No url parameter.");
    };

    match media::proxy_fetch(&state.media, url).await {
        Ok((bytes, content_type)) => {
            let mut response = (StatusCode::OK, bytes).into_response();
            if let Ok(value) = HeaderValue::from_str(&content_type) {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            response
        }
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

/// Who this request's saves belong to.
///
/// The signed-in user's `sub` when accounts are configured, and the fallback
/// owner otherwise so a server run by one person with no auth service keeps
/// working. Never anything the browser sent: a caller who could name the owner
/// could read and delete anyone's saves.
async fn save_owner(state: &AppState, headers: &axum::http::HeaderMap) -> Result<String, AuthError> {
    if !state.auth.is_configured() {
        return Ok(state.media.default_owner.clone());
    }
    auth::current_user(&state.auth, &state.sessions, session_id(headers).as_deref())
        .await
        .map(|user| user.id)
}

/// Saves for one game, newest first.
///
/// `?game=BPRE` narrows to a cartridge; without it every stored save is
/// returned.
async fn list_saves(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let owner = match save_owner(&state, &headers).await {
        Ok(owner) => owner,
        Err(err) => return auth_error(&err),
    };

    // `?legacy=1` asks a different question: how many saves from before
    // accounts are still unclaimed. The page uses it to decide whether
    // offering to claim them is worth a button.
    if params.contains_key("legacy") {
        let legacy = media::list_saves(&state.media, media::LEGACY_OWNER, None).await;
        return text(
            StatusCode::OK,
            serde_json::json!({ "legacy": legacy.saves.len() }).to_string(),
            "application/json; charset=utf-8",
        );
    }

    let game = params.get("game").map(|value| value.as_str());
    let listing = media::list_saves(&state.media, &owner, game).await;
    match serde_json::to_string(&listing) {
        Ok(json) => text(StatusCode::OK, json, "application/json; charset=utf-8"),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

/// Store a save state, then prune that game's oldest beyond the limit.
async fn upload_save(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Response {
    if !state.media.is_configured() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Storage is off. Set TINYBIRD_MEDIA_KEY in .env and restart.",
        );
    }

    let mut bytes: Vec<u8> = Vec::new();
    let mut game = String::new();
    let mut label = String::new();
    // The id of a save to overwrite. Absent for an ordinary save.
    let mut replace = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => bytes = field.bytes().await.map(|b| b.to_vec()).unwrap_or_default(),
            Some("game") => game = field.text().await.unwrap_or_default(),
            Some("label") => label = field.text().await.unwrap_or_default(),
            Some("replace") => replace = field.text().await.unwrap_or_default(),
            _ => {}
        }
    }

    if bytes.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "The request had no save to store.");
    }
    if game.trim().is_empty() {
        // Without a cartridge code the per-game limit cannot be applied, and
        // the save could not be listed against the right game later.
        return json_error(
            StatusCode::BAD_REQUEST,
            "A game code is required so the save can be filed against a cartridge.",
        );
    }

    let saved_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);

    let replace = Some(replace.trim()).filter(|id| !id.is_empty());
    let owner = match save_owner(&state, &headers).await {
        Ok(owner) => owner,
        Err(err) => return auth_error(&err),
    };

    match media::upload_save(
        &state.media,
        &owner,
        &game,
        &label,
        saved_at_ms,
        bytes,
        replace,
    )
    .await
    {
        Ok(entry) => text(
            StatusCode::CREATED,
            serde_json::to_string(&entry).unwrap_or_else(|_| "{}".to_string()),
            "application/json; charset=utf-8",
        ),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

/// Take ownership of the saves that were stored before accounts existed.
///
/// A one-time migration, and only for a signed-in caller: the saves have no
/// owner to check against, so whoever claims them gets them. That is the right
/// model for a server you run yourself and would not be for a public one.
async fn claim_saves(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if !state.media.is_configured() {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "Storage is off.");
    }
    if !state.auth.is_configured() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Accounts are off, so there is nobody to claim these saves for.",
        );
    }

    let owner = match auth::current_user(
        &state.auth,
        &state.sessions,
        session_id(&headers).as_deref(),
    )
    .await
    {
        Ok(user) => user.id,
        Err(err) => return auth_error(&err),
    };

    match media::claim_legacy_saves(&state.media, &owner).await {
        Ok(result) => text(
            StatusCode::OK,
            serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string()),
            "application/json; charset=utf-8",
        ),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err),
    }
}

/// Delete one stored save.
async fn delete_save(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(filename): Path<String>,
) -> Response {
    if !state.media.is_configured() {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "Storage is off.");
    }
    let owner = match save_owner(&state, &headers).await {
        Ok(owner) => owner,
        Err(err) => return auth_error(&err),
    };
    match media::delete_save(&state.media, &owner, &filename).await {
        Ok(()) => text(
            StatusCode::OK,
            "{\"deleted\":true}".to_string(),
            "application/json; charset=utf-8",
        ),
        // "Not yours" and "not there" answer the same way, so a caller cannot
        // probe for other people's saves.
        Err(err) if err.is_not_found() => json_error(StatusCode::NOT_FOUND, &err.message()),
        Err(err) => json_error(StatusCode::BAD_GATEWAY, &err.message()),
    }
}

// ------------------------------------------------------------------- accounts
//
// These four routes are the whole surface the browser sees of the auth service.
// It never learns a token; it gets an opaque first-party session cookie and
// this server does the rest. See `auth.rs` for why.

/// The session id the caller presented, if any.
fn session_id(headers: &axum::http::HeaderMap) -> Option<String> {
    auth::session_from_cookies(headers.get(header::COOKIE).and_then(|v| v.to_str().ok()))
}

/// Whether this request arrived over a connection a `Secure` cookie may use.
///
/// Behind a TLS-terminating proxy the scheme only survives in a header, so both
/// are consulted before deciding.
fn is_secure(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|proto| proto.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

fn auth_error(err: &AuthError) -> Response {
    json_error(
        StatusCode::from_u16(err.status()).unwrap_or(StatusCode::BAD_GATEWAY),
        &err.message(),
    )
}

/// Answer with the signed-in user, optionally establishing the session cookie.
fn user_response(user: &auth::User, cookie: Option<String>) -> Response {
    let body = serde_json::json!({ "user": user }).to_string();
    let mut response = text(StatusCode::OK, body, "application/json; charset=utf-8");
    if let Some(cookie) = cookie {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

async fn auth_register(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    sign_in_route(state, headers, body, "/api/auth/register").await
}

async fn auth_login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    sign_in_route(state, headers, body, "/api/auth/login").await
}

async fn sign_in_route(
    state: AppState,
    headers: axum::http::HeaderMap,
    body: serde_json::Value,
    path: &str,
) -> Response {
    // Check the length here as well as at the service: a local answer is
    // instant and does not spend one of the caller's rate-limited attempts.
    let password = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    if password.chars().count() < auth::MIN_PASSWORD_LEN {
        return json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "That password is too short. It needs at least {} characters.",
                auth::MIN_PASSWORD_LEN
            ),
        );
    }

    match auth::sign_in(&state.auth, &state.sessions, path, body).await {
        Ok((id, user)) => {
            let cookie = auth::session_cookie(&id, is_secure(&headers));
            user_response(&user, Some(cookie))
        }
        Err(err) => auth_error(&err),
    }
}

async fn auth_logout(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    auth::sign_out(&state.auth, &state.sessions, session_id(&headers).as_deref()).await;

    let mut response = text(
        StatusCode::OK,
        "{\"ok\":true}".to_string(),
        "application/json; charset=utf-8",
    );
    if let Ok(value) = HeaderValue::from_str(&auth::clear_cookie()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// Who the caller is, and whether accounts are available at all.
///
/// The page asks this on load: it is how a session survives a reload without
/// the browser holding anything but an opaque cookie.
async fn auth_me(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    if !state.auth.is_configured() {
        return text(
            StatusCode::OK,
            serde_json::json!({ "configured": false }).to_string(),
            "application/json; charset=utf-8",
        );
    }

    match auth::current_user(&state.auth, &state.sessions, session_id(&headers).as_deref()).await {
        Ok(user) => text(
            StatusCode::OK,
            serde_json::json!({ "configured": true, "user": user }).to_string(),
            "application/json; charset=utf-8",
        ),
        // Not being signed in is an ordinary answer here, not an error: the
        // page needs to know so it can show the sign-in panel.
        Err(AuthError::NoSession) => text(
            StatusCode::OK,
            serde_json::json!({ "configured": true, "user": null }).to_string(),
            "application/json; charset=utf-8",
        ),
        Err(err) => auth_error(&err),
    }
}

// ---------------------------------------------------------------------- lobby
//
// Everyone in a room runs their own emulator; this server only relays what they
// publish. See `lobby.rs`.

/// How often to ping an idle lobby socket, to stop a proxy closing it.
const LOBBY_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Who is asking, for the lobby.
///
/// With accounts configured this is the signed-in user and nothing else will
/// do: the name a member is shown under has to come from the session, not from
/// the browser, or anyone can appear as anyone. Without accounts the server is
/// yours alone, and a name the page supplies is fine.
async fn lobby_identity(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    offered_name: &str,
) -> Result<lobby::Identity, AuthError> {
    if !state.auth.is_configured() {
        return Ok(lobby::Identity::guest(offered_name));
    }

    let user =
        auth::current_user(&state.auth, &state.sessions, session_id(headers).as_deref()).await?;
    let name = user
        .display_name
        .clone()
        .unwrap_or_else(|| user.email.split('@').next().unwrap_or("player").to_string());
    Ok(lobby::Identity::account(&user.id, &name))
}

/// Open a room.
///
/// A room exists because somebody made one. Joining a code that was never
/// created is an error rather than a new room, so a mistyped code says so.
async fn create_room(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let who = match lobby_identity(&state, &headers, "host").await {
        Ok(who) => who,
        Err(err) => return auth_error(&err),
    };

    match state.lobby.create(who.user.as_deref()) {
        Ok(room) => text(
            StatusCode::OK,
            serde_json::json!({ "room": room }).to_string(),
            "application/json; charset=utf-8",
        ),
        Err(refused) => json_error(StatusCode::SERVICE_UNAVAILABLE, refused.message()),
    }
}

/// Join a room over a WebSocket.
///
/// `?room=CODE` and `?name=…`. The name is whatever the page offers, cleaned
/// before anyone else sees it; a signed-in caller's account name is used when
/// the page sends one, but this is a room you share a code for, not a
/// permission boundary, so it is not required.
async fn lobby_socket(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> Response {
    let code = params.get("room").cloned().unwrap_or_default();
    if lobby::normalise_code(&code).is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "A room code is required.");
    }

    // Refuse before the upgrade rather than after: a browser can read a status
    // and a message here, where a socket that opens and immediately closes
    // tells the page nothing it can show anyone.
    let offered = params.get("name").cloned().unwrap_or_default();
    let who = match lobby_identity(&state, &headers, &offered).await {
        Ok(who) => who,
        Err(err) => return auth_error(&err),
    };

    ws.on_upgrade(move |socket| run_lobby_member(socket, state, code, who))
}

/// One member's connection, for as long as they hold it.
async fn run_lobby_member(
    socket: axum::extract::ws::WebSocket,
    state: AppState,
    code: String,
    who: lobby::Identity,
) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut stream) = socket.split();
    let joined = match state.lobby.join(&code, &who) {
        Ok(joined) => joined,
        Err(refused) => {
            // Say why, then close: a socket that stays open without a room is
            // worse than one that never opened.
            let message = lobby::Outgoing::Error {
                message: refused.message().to_string(),
            };
            if let Ok(text) = serde_json::to_string(&message) {
                let _ = sink.send(Message::Text(text.into())).await;
            }
            return;
        }
    };
    let (room, me) = (joined.room.clone(), joined.member_id.clone());

    // Tell the new arrival who they are and who else is here.
    let welcome = lobby::Outgoing::Welcome {
        room: room.clone(),
        you: me.clone(),
        members: joined.members.clone(),
    };
    if let Ok(text) = serde_json::to_string(&welcome) {
        if sink.send(Message::Text(text.into())).await.is_err() {
            state.lobby.leave(&room, &me);
            return;
        }
    }
    // And everyone else that the list changed.
    state.lobby.broadcast(
        &room,
        &lobby::Outgoing::Members {
            members: joined.members,
        },
    );

    // Fan the room's messages out to this socket, and keep it warm.
    //
    // A member with no game loaded publishes nothing, and a proxy in front of
    // this server will close a connection that has been silent for a minute or
    // two — Cloudflare does. The page reconnects on its own, but a room that
    // drops everyone every hundred seconds and quietly rebuilds itself is not
    // something to leave in. A ping costs nothing and browsers answer it
    // without any page code.
    let mut receiver = joined.receiver;
    let writer = tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(LOBBY_PING_INTERVAL);
        // The first tick fires immediately; a ping before the welcome is
        // pointless.
        keepalive.tick().await;
        loop {
            let outgoing = tokio::select! {
                message = receiver.recv() => match message {
                    Ok(text) => Message::Text(text.into()),
                    // Lagged behind: the buffer holds snapshots, which are worth
                    // nothing once stale, so skipping ahead is right.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                _ = keepalive.tick() => Message::Ping(Vec::new().into()),
            };
            if sink.send(outgoing).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            // Binary, ping and close frames need no handling here.
            continue;
        };
        if text.len() > lobby::MAX_MESSAGE_BYTES {
            state.lobby.broadcast(
                &room,
                &lobby::Outgoing::Error {
                    message: "That message was too large.".to_string(),
                },
            );
            continue;
        }

        match serde_json::from_str::<lobby::Incoming>(&text) {
            Ok(lobby::Incoming::Playing { playing, game_code }) => {
                if let Some(members) =
                    state.lobby.set_playing(&room, &me, playing, game_code)
                {
                    state
                        .lobby
                        .broadcast(&room, &lobby::Outgoing::Members { members });
                }
            }
            Ok(lobby::Incoming::Lock { locked }) => {
                // Ignored for anyone but the host, and silently: a member who
                // is not the host has no business being told the room's rules.
                if state.lobby.set_locked(&room, &me, locked).is_some() {
                    state
                        .lobby
                        .broadcast(&room, &lobby::Outgoing::Locked { locked });
                }
            }
            Ok(lobby::Incoming::Snapshot { snapshot }) => {
                state.lobby.broadcast(
                    &room,
                    &lobby::Outgoing::Snapshot {
                        from: me.clone(),
                        snapshot,
                    },
                );
            }
            // The link cable. The server relays halfwords and never looks at
            // one: it has no idea what a transfer means and does not need to.
            //
            // What it does enforce is who may drive. A cable has one parent,
            // and letting any member announce a transfer or publish its result
            // would let them step on somebody else's game — corrupting a trade
            // is a real thing to lose, so the check is here rather than in the
            // page, where it would be advice.
            Ok(lobby::Incoming::LinkTick { frame }) => {
                // Only the parent has a clock worth following.
                if state.lobby.is_link_parent(&room, &me) {
                    state
                        .lobby
                        .broadcast(&room, &lobby::Outgoing::LinkTick { frame });
                }
            }
            Ok(lobby::Incoming::LinkStart { seq, frame, offset }) => {
                if state.lobby.is_link_parent(&room, &me) {
                    state.lobby.broadcast(
                        &room,
                        &lobby::Outgoing::LinkStart { seq, frame, offset },
                    );
                }
            }
            Ok(lobby::Incoming::LinkData {
                seq,
                values,
                cycles,
            }) => {
                if state.lobby.is_link_parent(&room, &me) {
                    state.lobby.broadcast(
                        &room,
                        &lobby::Outgoing::LinkData {
                            seq,
                            values,
                            cycles,
                        },
                    );
                }
            }
            Ok(lobby::Incoming::LinkValue { seq, value }) => {
                // Anyone may answer, because answering is all a child does.
                // The parent decides which answers belong to which seat.
                state.lobby.broadcast(
                    &room,
                    &lobby::Outgoing::LinkValue {
                        from: me.clone(),
                        seq,
                        value,
                    },
                );
            }
            Ok(lobby::Incoming::Frame { frame }) => {
                // Relayed as-is. The server has no idea what a frame looks
                // like and does not need one: it is a string to everyone here.
                state.lobby.broadcast(
                    &room,
                    &lobby::Outgoing::Frame {
                        from: me.clone(),
                        frame,
                    },
                );
            }
            // A message this server does not understand is ignored rather than
            // closing the connection: a newer page talking to an older server
            // should lose the feature, not the room.
            Err(_) => {}
        }
    }

    writer.abort();
    if let Some(members) = state.lobby.leave(&room, &me) {
        state
            .lobby
            .broadcast(&room, &lobby::Outgoing::Members { members });
    }
}

/// Names of ROMs in the local folder.
///
/// Offered next to the vault so the page works with no storage configured, and
/// so a ROM you would rather not upload can still be played.
async fn local_roms(State(state): State<AppState>) -> Response {
    if !state.serve_local_roms {
        // An empty library rather than an error: this server simply has
        // nothing local to offer, which is the truth.
        return text(
            StatusCode::OK,
            "{\"assets\":[]}".to_string(),
            "application/json; charset=utf-8",
        );
    }
    let roms = scan_rom_dir(&state.rom_dir).await;
    let entries: Vec<String> = roms
        .iter()
        .map(|rom| {
            format!(
                "{{\"name\":\"{}\",\"url\":\"/local/{}\",\"size\":{},\"source\":\"local\"}}",
                escape_json_string(&rom.display),
                escape_json_string(&encode_path(&rom.relative)),
                rom.size,
            )
        })
        .collect();
    text(
        StatusCode::OK,
        format!("{{\"assets\":[{}]}}", entries.join(",")),
        "application/json; charset=utf-8",
    )
}

/// Serve one local ROM.
///
/// Only paths the listing produced are accepted, which is what keeps a crafted
/// path from reaching outside the ROM folder. The scan is the allowlist, so a
/// wildcard route cannot be walked upwards however it is spelled.
async fn local_rom_file(Path(path): Path<String>, State(state): State<AppState>) -> Response {
    if !state.serve_local_roms {
        return json_error(
            StatusCode::NOT_FOUND,
            "This server does not serve local ROMs.",
        );
    }
    let wanted = path.replace('\\', "/");
    let Some(rom) = scan_rom_dir(&state.rom_dir)
        .await
        .into_iter()
        .find(|candidate| candidate.relative == wanted)
    else {
        return json_error(StatusCode::NOT_FOUND, "No local ROM by that name.");
    };

    let mut full = state.rom_dir.clone();
    for segment in rom.relative.split('/') {
        full.push(segment);
    }
    match tokio::fs::read(&full).await {
        Ok(bytes) => binary(StatusCode::OK, bytes, "application/octet-stream"),
        Err(_) => json_error(StatusCode::NOT_FOUND, "That ROM could not be read."),
    }
}

/// Percent-encode a relative path for use in a URL, keeping the separators.
fn encode_path(relative: &str) -> String {
    let mut out = String::with_capacity(relative.len());
    for byte in relative.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// One ROM found on disk.
struct LocalRom {
    /// Path relative to the ROM folder, always with `/` separators.
    relative: String,
    /// What to show in the library.
    display: String,
    size: u64,
}

/// Whether the local ROM folder should be served.
///
/// `TINYBIRD_LOCAL_ROMS=off` switches it off. A public deployment wants that:
/// everything else this server offers is either public already or scoped to an
/// account, and the ROM folder is neither.
fn local_roms_enabled() -> bool {
    match env::var("TINYBIRD_LOCAL_ROMS") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        ),
        Err(_) => true,
    }
}

/// ROM files in `dir` and one level of subfolders, with their sizes.
///
/// One level, not a full walk: dumps are conventionally kept one folder per
/// game, so a single level finds them all without the cost of descending an
/// arbitrary tree. Anything deeper stays loadable through the file picker.
async fn scan_rom_dir(dir: &FsPath) -> Vec<LocalRom> {
    let mut roms = Vec::new();
    collect_roms(dir, None, &mut roms).await;

    let mut subdirs = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    subdirs.push(name.to_string());
                }
            }
        }
    }
    subdirs.sort();
    for name in subdirs {
        collect_roms(&dir.join(&name), Some(&name), &mut roms).await;
    }

    roms.sort_by(|a, b| a.display.cmp(&b.display));
    roms
}

/// Add the ROMs directly inside `dir` to `roms`.
async fn collect_roms(dir: &FsPath, prefix: Option<&str>, roms: &mut Vec<LocalRom>) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !media::is_rom_name(name) {
            continue;
        }
        roms.push(LocalRom {
            relative: match prefix {
                Some(folder) => format!("{folder}/{name}"),
                None => name.to_string(),
            },
            display: name.to_string(),
            size: entry.metadata().await.map(|m| m.len()).unwrap_or(0),
        });
    }
}

async fn app_js() -> Response {
    static_text(APP_JS, "application/javascript; charset=utf-8")
}

async fn styles_css() -> Response {
    static_text(STYLES_CSS, "text/css; charset=utf-8")
}

async fn health(State(state): State<AppState>) -> Response {
    let snapshot_exists = FsPath::new(&state.snapshot_path).is_file();
    let sprites_exist = FsPath::new(&state.sprite_dir).is_dir();
    let rooms = state.lobby.room_count();
    let body = format!(
        "{{\"ok\":true,\"rooms\":{rooms},\"snapshot_exists\":{snapshot_exists},\"sprites_exist\":{sprites_exist},\"snapshot_path\":\"{}\",\"sprite_dir\":\"{}\"}}",
        escape_json_string(&state.snapshot_path.display().to_string()),
        escape_json_string(&state.sprite_dir.display().to_string()),
    );
    text(StatusCode::OK, body, "application/json; charset=utf-8")
}

async fn snapshot(State(state): State<AppState>) -> Response {
    match tokio::fs::read_to_string(&state.snapshot_path).await {
        Ok(json) if serde_json_like(&json) => {
            text(StatusCode::OK, json, "application/json; charset=utf-8")
        }
        Ok(_) | Err(_) => text(
            StatusCode::OK,
            empty_snapshot_json(),
            "application/json; charset=utf-8",
        ),
    }
}

fn empty_snapshot_json() -> String {
    format!("{{\n  \"schema_version\": {SNAPSHOT_SCHEMA_VERSION}\n}}")
}

/// A species picture for the addon read-out.
///
/// Served from the on-disk cache when it is there and fetched once when it is
/// not, so the first run of a server populates itself and every run after it
/// works offline. A miss is a plain 404: the page hides the picture and keeps
/// the card, which is why the addon also sends the species name in words.
/// Addon manifests, as one array the page can hand straight to the emulator.
///
/// The browser has no filesystem, so this is how a manifest reaches it. One
/// array rather than a file listing plus fetches: the registry can only be
/// built once, so the page needs all of them before it starts.
///
/// A file that will not parse is skipped rather than failing the request. One
/// bad manifest should cost that manifest.
async fn addon_manifests(State(state): State<AppState>) -> Response {
    let mut manifests: Vec<serde_json::Value> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&state.addon_dir) {
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        // Sorted, so registration order does not depend on the filesystem.
        paths.sort();

        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                manifests.push(value);
            }
        }
    }

    let body = serde_json::to_string(&manifests).unwrap_or_else(|_| "[]".to_string());
    text(StatusCode::OK, body, "application/json; charset=utf-8")
}

async fn sprite_png(Path(species_id): Path<u16>, State(state): State<AppState>) -> Response {
    match sprites::fetch(&state.sprite_dir, species_id).await {
        sprites::Sprite::Png(bytes) => {
            let mut response = binary(StatusCode::OK, bytes, "image/png");
            // A species sprite is the same picture forever, so let the browser
            // stop asking. Without this the rail re-requests six of them on
            // every reload.
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            response
        }
        sprites::Sprite::OutOfRange | sprites::Sprite::Unavailable => {
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

fn static_text(body: &'static str, content_type: &'static str) -> Response {
    text(StatusCode::OK, body.to_string(), content_type)
}

fn text(status: StatusCode, body: String, content_type: &'static str) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn binary(status: StatusCode, body: Vec<u8>, content_type: &'static str) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn serde_json_like(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with('{') && trimmed.ends_with('}')
}

fn escape_json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_uses_defaults_without_args() {
        let config = Config::from_env_and_args(Vec::new());
        assert_eq!(config.host, DEFAULT_HOST);
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.snapshot_path, PathBuf::from(DEFAULT_SNAPSHOT_PATH));
        assert_eq!(config.sprite_dir, PathBuf::from(DEFAULT_SPRITE_DIR));
    }

    #[test]
    fn config_accepts_cli_overrides() {
        let args = vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "9001".to_string(),
            "--snapshot".to_string(),
            "custom/game.json".to_string(),
            "--sprites".to_string(),
            "custom/sprites".to_string(),
        ];
        let config = Config::from_env_and_args(args);
        assert_eq!(config.host, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(config.port, 9001);
        assert_eq!(config.snapshot_path, PathBuf::from("custom/game.json"));
        assert_eq!(config.sprite_dir, PathBuf::from("custom/sprites"));
    }

    /// The `el` map in play.js must not name the same key twice.
    ///
    /// A duplicate is silent in JavaScript — the last one wins — and the way it
    /// failed was not obvious from anything: adding the lobby's `screen` image
    /// quietly took over the console's `screen` bezel, so loading a cartridge
    /// set the running state on a hidden thumbnail and the picture stayed at
    /// 240x160 with "no cartridge" over it, with no error anywhere.
    #[test]
    fn the_play_page_looks_up_each_element_under_one_name() {
        let block = PLAY_JS
            .split_once("const el = {")
            .and_then(|(_, rest)| rest.split_once("
};"))
            .expect("play.js should declare `const el = { ... };`")
            .0;

        let mut seen: Vec<&str> = Vec::new();
        for line in block.lines() {
            let line = line.trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            // Only the lookups; comments and anything else are not keys.
            if !value.trim_start().starts_with("$(") {
                continue;
            }
            assert!(
                !seen.contains(&key),
                "`{key}` is looked up twice in the el map; the later one silently wins"
            );
            seen.push(key);
        }
        assert!(seen.len() > 20, "expected the whole map, found {}", seen.len());
    }

    /// Every element play.js reaches for has to be in the page.
    ///
    /// `$` returns null for an id that is not there, and the failure surfaces
    /// much later as a property access on null rather than at the lookup.
    #[test]
    fn every_element_the_play_page_looks_up_exists_in_the_markup() {
        let mut missing: Vec<&str> = Vec::new();
        for (_, rest) in PLAY_JS.match_indices("$(\"").map(|(i, _)| PLAY_JS.split_at(i + 3)) {
            let Some((id, _)) = rest.split_once('"') else {
                continue;
            };
            if !PLAY_HTML.contains(&format!("id=\"{id}\"")) && !missing.contains(&id) {
                missing.push(id);
            }
        }
        assert!(missing.is_empty(), "play.js looks up ids the page has not got: {missing:?}");
    }
}
