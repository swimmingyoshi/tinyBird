use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::env;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use tinybird_addons::SNAPSHOT_SCHEMA_VERSION;
use tokio::net::TcpListener;

mod guard;
mod sprites;

const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEFAULT_PORT: u16 = 8877;
const DEFAULT_SNAPSHOT_PATH: &str = "stream-data/current-game.json";
const DEFAULT_SPRITE_DIR: &str = "stream-data/pokemon-sprites";
/// Addon manifests, served to the page because the browser has no disk.
const DEFAULT_ADDON_DIR: &str = "addons";
/// The ROM folder the page lists and loads from.
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

const HTML: &str = "text/html; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

/// Every page, script and stylesheet the server hands out unchanged.
///
/// One row each, rather than a route and a handler apiece: a new file is a
/// new line here and nothing else. All of them are compiled into the binary
/// and served with a content tag, so the browser revalidates instead of
/// downloading again.
const ASSETS: &[(&str, &str, &str)] = &[
    ("/", include_str!("assets/index.html"), HTML),
    ("/start.js", include_str!("assets/start.js"), JS),
    ("/play", include_str!("assets/play.html"), HTML),
    ("/play.js", include_str!("assets/play.js"), JS),
    ("/tinybird.js", include_str!("assets/tinybird.js"), JS),
    ("/saveformat.js", include_str!("assets/saveformat.js"), JS),
    ("/pacing.js", include_str!("assets/pacing.js"), JS),
    ("/controls.js", include_str!("assets/controls.js"), JS),
    ("/theme.js", include_str!("assets/theme.js"), JS),
    ("/recovery.js", include_str!("assets/recovery.js"), JS),
    (
        "/memory-sheets.js",
        include_str!("assets/memory-sheets.js"),
        JS,
    ),
    ("/addons", include_str!("assets/addons.html"), HTML),
    ("/addon", include_str!("assets/addons.html"), HTML),
    ("/addons.js", include_str!("assets/addons.js"), JS),
    (
        "/addon-client.js",
        include_str!("assets/addon-client.js"),
        JS,
    ),
    (
        "/reader-guide",
        include_str!("assets/reader-guide.html"),
        HTML,
    ),
    ("/workshop.js", include_str!("assets/workshop.js"), JS),
    (
        "/workshop-model.js",
        include_str!("assets/workshop-model.js"),
        JS,
    ),
    (
        "/workshop-observations.js",
        include_str!("assets/workshop-observations.js"),
        JS,
    ),
    ("/app.js", include_str!("assets/app.js"), JS),
    ("/styles.css", include_str!("assets/styles.css"), CSS),
    (
        "/console.css",
        concat!(
            include_str!("assets/console.css"),
            include_str!("assets/site.css"),
            include_str!("assets/playroom.css")
        ),
        CSS,
    ),
    ("/addons.css", include_str!("assets/addons.css"), CSS),
    ("/workshop.css", include_str!("assets/workshop.css"), CSS),
    (
        "/workshop-player.css",
        include_str!("assets/workshop-player.css"),
        CSS,
    ),
];

const FFTA_HUMAN_PNG: &[u8] = include_bytes!("assets/ffta-races/human.png");
const FFTA_BANGAA_PNG: &[u8] = include_bytes!("assets/ffta-races/bangaa.png");
const FFTA_NU_MOU_PNG: &[u8] = include_bytes!("assets/ffta-races/nu-mou.png");
const FFTA_VIERA_PNG: &[u8] = include_bytes!("assets/ffta-races/viera.png");
const FFTA_MOOGLE_PNG: &[u8] = include_bytes!("assets/ffta-races/moogle.png");
/// The 42 playable job labels, in FFTA's job-id order (0x02 through 0x2b).
const FFTA_JOB_PNGS: [&[u8]; 42] = [
    include_bytes!("assets/ffta-jobs/02.png"),
    include_bytes!("assets/ffta-jobs/03.png"),
    include_bytes!("assets/ffta-jobs/04.png"),
    include_bytes!("assets/ffta-jobs/05.png"),
    include_bytes!("assets/ffta-jobs/06.png"),
    include_bytes!("assets/ffta-jobs/07.png"),
    include_bytes!("assets/ffta-jobs/08.png"),
    include_bytes!("assets/ffta-jobs/09.png"),
    include_bytes!("assets/ffta-jobs/0a.png"),
    include_bytes!("assets/ffta-jobs/0b.png"),
    include_bytes!("assets/ffta-jobs/0c.png"),
    include_bytes!("assets/ffta-jobs/0d.png"),
    include_bytes!("assets/ffta-jobs/0e.png"),
    include_bytes!("assets/ffta-jobs/0f.png"),
    include_bytes!("assets/ffta-jobs/10.png"),
    include_bytes!("assets/ffta-jobs/11.png"),
    include_bytes!("assets/ffta-jobs/12.png"),
    include_bytes!("assets/ffta-jobs/13.png"),
    include_bytes!("assets/ffta-jobs/14.png"),
    include_bytes!("assets/ffta-jobs/15.png"),
    include_bytes!("assets/ffta-jobs/16.png"),
    include_bytes!("assets/ffta-jobs/17.png"),
    include_bytes!("assets/ffta-jobs/18.png"),
    include_bytes!("assets/ffta-jobs/19.png"),
    include_bytes!("assets/ffta-jobs/1a.png"),
    include_bytes!("assets/ffta-jobs/1b.png"),
    include_bytes!("assets/ffta-jobs/1c.png"),
    include_bytes!("assets/ffta-jobs/1d.png"),
    include_bytes!("assets/ffta-jobs/1e.png"),
    include_bytes!("assets/ffta-jobs/1f.png"),
    include_bytes!("assets/ffta-jobs/20.png"),
    include_bytes!("assets/ffta-jobs/21.png"),
    include_bytes!("assets/ffta-jobs/22.png"),
    include_bytes!("assets/ffta-jobs/23.png"),
    include_bytes!("assets/ffta-jobs/24.png"),
    include_bytes!("assets/ffta-jobs/25.png"),
    include_bytes!("assets/ffta-jobs/26.png"),
    include_bytes!("assets/ffta-jobs/27.png"),
    include_bytes!("assets/ffta-jobs/28.png"),
    include_bytes!("assets/ffta-jobs/29.png"),
    include_bytes!("assets/ffta-jobs/2a.png"),
    include_bytes!("assets/ffta-jobs/2b.png"),
];

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
    /// Whether to serve ROMs off this machine's disk. On unless
    /// `TINYBIRD_LOCAL_ROMS=off`.
    serve_local_roms: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = Config::from_env_and_args(args);
    guard::check_bind(config.host)?;
    let wasm_ready = config.wasm_path.is_file();
    let wasm_display = config.wasm_path.display().to_string();
    let bios_ready = config.bios_path.is_file();
    let bios_display = config.bios_path.display().to_string();

    let state = AppState {
        snapshot_path: config.snapshot_path,
        sprite_dir: config.sprite_dir,
        overlay_enabled: config.overlay_enabled,
        addon_dir: config.addon_dir,
        wasm_path: config.wasm_path.clone(),
        rom_dir: config.rom_dir,
        bios_path: config.bios_path,
        serve_local_roms: local_roms_enabled(),
    };
    let mut app = Router::new();
    for &(path, body, content_type) in ASSETS {
        app = app.route(
            path,
            get(move || async move { static_text(body, content_type) }),
        );
    }
    let app = app
        .route("/overlay", get(overlay))
        .route("/overlay/{section}", get(overlay))
        .route("/tinybird.wasm", get(wasm_module))
        .route("/bios", get(bios_image))
        .route("/api/local", get(local_roms))
        .route("/local/{*path}", get(local_rom_file))
        .route("/api/health", get(health))
        .route("/api/snapshot", get(snapshot))
        .route("/api/addons", get(addon_manifests))
        .route("/sprites/{species_id}", get(sprite_png))
        .route("/ffta/races/{race}", get(ffta_race_png))
        .route("/ffta/jobs/{job}", get(ffta_job_png))
        .with_state(state)
        .layer(axum::middleware::from_fn(guard::guard));

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
    println!("  start:      http://{addr}/");
    println!("  play:       http://{addr}/play");
    println!("  add-ons:    http://{addr}/addons");
    if let Some(age) = wasm_is_behind(&config.wasm_path) {
        println!();
        println!("  The emulator module is {age} older than this server.");
        println!("  Every addon and the emulator itself run inside it, so a fix built");
        println!("  with `cargo build` alone is not on the page. Rebuild it with:");
        println!("    cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release");
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

/// Crates compiled into the emulator module.
///
/// A change to any of these is a change the page cannot see until the module
/// is rebuilt. `tinybird-web` is deliberately not among them: this server's own
/// sources do not go into the wasm, and treating them as if they did is what
/// made the warning cry wolf through a week of front-end work.
const WASM_SOURCES: [&str; 4] = [
    "crates/tinybird-core/src",
    "crates/tinybird-addons/src",
    "crates/tinybird-games/src",
    "crates/tinybird-wasm/src",
];

/// The newest modification time anywhere under `dir`, if it can be read.
fn newest_file_in(dir: &FsPath) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).ok()?.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if let Ok(at) = entry.metadata().and_then(|m| m.modified()) {
                newest = Some(newest.map_or(at, |best: std::time::SystemTime| best.max(at)));
            }
        }
    }
    newest
}

/// How far behind the emulator's sources the WebAssembly module is, if at all.
///
/// The addons and the emulator are compiled into that module, not into this
/// binary, and `cargo build` does not build the wasm target. So a fix can be
/// built, tested, and running in the server while the page carries on executing
/// the version before it — with nothing to say so, because the page is not
/// stale, only the module it loads.
fn wasm_is_behind(wasm_path: &FsPath) -> Option<String> {
    let wasm = std::fs::metadata(wasm_path).ok()?.modified().ok()?;

    // Against the emulator's own sources, not against this binary.
    //
    // It used to compare the module with the server executable, on the theory
    // that a recent `cargo build` meant recent emulator work. It does not: every
    // edit to a page, a stylesheet, or a route rebuilds this binary and leaves
    // the module correctly untouched, and the warning then claimed the page was
    // running a stale core when nothing about the core had changed.
    //
    // Missing source directories mean this is not a checkout — a distributed
    // binary, say — and there is nothing to compare, so nothing is claimed.
    let newest_source = WASM_SOURCES
        .iter()
        .filter_map(|dir| newest_file_in(FsPath::new(dir)))
        .max()?;

    let behind = newest_source.duration_since(wasm).ok()?;
    // A minute of slack: a build writes the two some seconds apart even when
    // both are current.
    if behind < std::time::Duration::from_secs(60) {
        return None;
    }

    let minutes = behind.as_secs() / 60;
    Some(match minutes {
        0..=59 => format!("{minutes} minutes"),
        60..=1439 => format!("{} hours", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    })
}

/// Serve the emulator module.
///
/// Read from disk rather than embedded with `include_bytes!`, so the workspace
/// still builds for anyone who has not installed the `wasm32` target.
async fn wasm_module(State(state): State<AppState>) -> Response {
    match tokio::fs::read(&state.wasm_path).await {
        Ok(bytes) => {
            // Half a megabyte, and the same half a megabyte until it is
            // rebuilt. Tagged by content rather than by modification time, so
            // a rebuild that changes nothing costs the browser nothing either.
            let tag = content_tag(&bytes);
            let mut response = binary(StatusCode::OK, bytes, "application/wasm");
            revalidate_with(&mut response, tag);
            response
        }
        Err(_) => json_error(
            StatusCode::NOT_FOUND,
            &format!(
                "{} not found. Build it with: cargo build -p tinybird-wasm --target wasm32-unknown-unknown --release",
                state.wasm_path.display()
            ),
        ),
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

/// Names of ROMs in the local folder.
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

/// Whether a file name looks like a GBA ROM.
fn is_rom_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gba") || lower.ends_with(".bin") || lower.ends_with(".agb")
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
/// `TINYBIRD_LOCAL_ROMS=off` switches it off, for a server whose only job is
/// the add-on tools or the overlay.
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
        if !is_rom_name(name) {
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

/// Whether this checkout is ready to play, for the start page.
///
/// Reports facts about the files on disk, not claims: whether the emulator
/// module has been built and is current with its sources, whether a BIOS dump
/// is in place, and how many ROMs and add-on manifests were found.
async fn health(State(state): State<AppState>) -> Response {
    let snapshot_exists = FsPath::new(&state.snapshot_path).is_file();
    let sprites_exist = FsPath::new(&state.sprite_dir).is_dir();
    let wasm_ready = state.wasm_path.is_file();
    let wasm_behind = match wasm_is_behind(&state.wasm_path) {
        Some(age) => format!("\"{}\"", escape_json_string(&age)),
        None => "null".to_string(),
    };
    let bios_ready = state.bios_path.is_file();
    let roms = if state.serve_local_roms {
        scan_rom_dir(&state.rom_dir).await.len()
    } else {
        0
    };
    let addons = std::fs::read_dir(&state.addon_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0);
    let body = format!(
        "{{\"ok\":true,\"wasm_ready\":{wasm_ready},\"wasm_behind\":{wasm_behind},\"bios_ready\":{bios_ready},\"roms\":{roms},\"roms_served\":{},\"rom_dir\":\"{}\",\"addons\":{addons},\"snapshot_exists\":{snapshot_exists},\"sprites_exist\":{sprites_exist},\"snapshot_path\":\"{}\",\"sprite_dir\":\"{}\"}}",
        state.serve_local_roms,
        escape_json_string(&state.rom_dir.display().to_string()),
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

/// A species picture for the addon read-out.
///
/// Served from the on-disk cache when it is there and fetched once when it is
/// not, so the first run of a server populates itself and every run after it
/// works offline. A miss is a plain 404: the page hides the picture and keeps
/// the card, which is why the addon also sends the species name in words.
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

async fn ffta_race_png(Path(race): Path<String>) -> Response {
    let bytes = match race.as_str() {
        "human" => FFTA_HUMAN_PNG,
        "bangaa" => FFTA_BANGAA_PNG,
        "nu-mou" => FFTA_NU_MOU_PNG,
        "viera" => FFTA_VIERA_PNG,
        "moogle" => FFTA_MOOGLE_PNG,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut response = binary(StatusCode::OK, bytes.to_vec(), "image/png");
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

async fn ffta_job_png(Path(job): Path<String>) -> Response {
    let Ok(job_id) = u8::from_str_radix(&job, 16) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(bytes) = job_id
        .checked_sub(2)
        .and_then(|index| FFTA_JOB_PNGS.get(index as usize))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = binary(StatusCode::OK, bytes.to_vec(), "image/png");
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    response
}

/// An asset compiled into the binary.
///
/// The URL is stable and the content changes when the server is rebuilt, so the
/// browser is told to ask every time (`no-cache`) and given a tag to ask with.
/// `deployment::guard` turns a matching `If-None-Match` into an empty 304.
fn static_text(body: &'static str, content_type: &'static str) -> Response {
    let mut response = text(StatusCode::OK, body.to_string(), content_type);
    revalidate_with(&mut response, content_tag(body.as_bytes()));
    response
}

/// Mark a response as "check with me first", identified by `tag`.
fn revalidate_with(response: &mut Response, tag: u64) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{tag:016x}\"")) {
        headers.insert(header::ETAG, etag);
    }
}

/// FNV-1a over the content. It identifies a version of a file to a browser
/// that already has one; nothing depends on it being hard to forge.
fn content_tag(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
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

    /// A scratch directory of our own, since the crate has no tempfile dep.
    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = env::temp_dir().join(format!("tinybird-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn the_newest_file_is_found_through_subdirectories() {
        let root = scratch("newest");
        std::fs::write(root.join("old.rs"), "1").unwrap();
        std::fs::create_dir_all(root.join("deep/deeper")).unwrap();
        let buried = root.join("deep/deeper/new.rs");
        // Written second, so it is the newer of the two whatever the clock's
        // resolution: the check only has to prefer it, not date it.
        std::fs::write(&buried, "2").unwrap();

        let newest = newest_file_in(&root).expect("a file was found");
        let buried_at = std::fs::metadata(&buried).unwrap().modified().unwrap();
        assert!(newest >= buried_at);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_that_is_not_there_yields_nothing() {
        assert!(newest_file_in(FsPath::new("no/such/directory/anywhere")).is_none());
    }

    #[test]
    fn nothing_is_claimed_when_the_emulator_sources_are_not_around() {
        // The distributed-binary case: `WASM_SOURCES` are checkout-relative, so
        // running from anywhere else finds none of them. Silence is the only
        // honest answer there — and the tests run from the crate directory,
        // which is exactly such a place.
        let root = scratch("wasm");
        let wasm = root.join("tinybird_wasm.wasm");
        std::fs::write(&wasm, b"\0asm").unwrap();

        assert_eq!(wasm_is_behind(&wasm), None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_module_is_not_reported_as_behind() {
        // "Not built yet" is a different message, delivered by the route that
        // serves it. This one only speaks about a module that exists.
        assert_eq!(wasm_is_behind(FsPath::new("no/such/module.wasm")), None);
    }

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
        let block = asset("/play.js")
            .split_once("const el = {")
            .and_then(|(_, rest)| {
                rest.split_once(
                    "
};",
                )
            })
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
        assert!(
            seen.len() > 20,
            "expected the whole map, found {}",
            seen.len()
        );
    }

    /// Every element play.js reaches for has to be in the page.
    ///
    /// `$` returns null for an id that is not there, and the failure surfaces
    /// much later as a property access on null rather than at the lookup.
    #[test]
    fn every_element_the_play_page_looks_up_exists_in_the_markup() {
        let missing = missing_ids(asset("/play.js"), asset("/play"));
        assert!(
            missing.is_empty(),
            "play.js looks up ids the page has not got: {missing:?}"
        );
    }

    /// Ids a script reaches for with `$("...")` that the markup has not got.
    fn missing_ids<'a>(js: &'a str, html: &str) -> Vec<&'a str> {
        let mut missing: Vec<&str> = Vec::new();
        for (_, rest) in js.match_indices("$(\"").map(|(i, _)| js.split_at(i + 3)) {
            let Some((id, _)) = rest.split_once('"') else {
                continue;
            };
            if !html.contains(&format!("id=\"{id}\"")) && !missing.contains(&id) {
                missing.push(id);
            }
        }
        missing
    }

    #[test]
    fn every_element_the_start_page_looks_up_exists_in_the_markup() {
        let missing = missing_ids(asset("/start.js"), asset("/"));
        assert!(
            missing.is_empty(),
            "start.js looks up ids the page has not got: {missing:?}"
        );
    }

    /// The table is the whole static surface, so a path listed twice would be
    /// a route axum refuses at startup rather than in a test.
    #[test]
    fn every_asset_path_is_listed_once() {
        let mut paths: Vec<&str> = ASSETS.iter().map(|(path, _, _)| *path).collect();
        paths.sort();
        let before = paths.len();
        paths.dedup();
        assert_eq!(before, paths.len());
    }

    /// The hosted site's pages and scripts are not part of this server.
    #[test]
    fn nothing_links_to_the_hosted_site() {
        for (path, body, _) in ASSETS {
            for gone in [
                "/api/auth",
                "/api/saves",
                "/api/lobby",
                "/api/library",
                "/contact",
                "/support/tickets",
            ] {
                assert!(!body.contains(gone), "{path} still mentions {gone}");
            }
        }
    }

    fn asset(path: &str) -> &'static str {
        ASSETS
            .iter()
            .find(|(candidate, _, _)| *candidate == path)
            .map(|(_, body, _)| *body)
            .unwrap_or_else(|| panic!("no asset at {path}"))
    }
}
