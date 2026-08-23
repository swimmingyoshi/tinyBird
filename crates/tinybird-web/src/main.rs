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

const DEFAULT_HOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEFAULT_PORT: u16 = 8877;
const DEFAULT_SNAPSHOT_PATH: &str = "stream-data/current-game.json";
const DEFAULT_SPRITE_DIR: &str = "stream-data/pokemon-sprites";

const INDEX_HTML: &str = include_str!("assets/index.html");
const OVERLAY_HTML: &str = include_str!("assets/overlay.html");
const APP_JS: &str = include_str!("assets/app.js");
const STYLES_CSS: &str = include_str!("assets/styles.css");

#[derive(Clone, Debug)]
struct AppState {
    snapshot_path: PathBuf,
    sprite_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env_and_args(env::args().skip(1).collect());
    let state = AppState {
        snapshot_path: config.snapshot_path,
        sprite_dir: config.sprite_dir,
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/overlay", get(overlay))
        .route("/overlay/{section}", get(overlay))
        .route("/app.js", get(app_js))
        .route("/styles.css", get(styles_css))
        .route("/api/health", get(health))
        .route("/api/snapshot", get(snapshot))
        .route("/sprites/{species_id}", get(sprite_png))
        .with_state(state);

    let addr = SocketAddr::new(config.host, config.port);
    let listener = TcpListener::bind(addr).await?;

    println!("tinyBird web overlay listening on http://{addr}");
    println!("  setup:      http://{addr}/");
    println!("  full:       http://{addr}/overlay/full");
    println!("  party:      http://{addr}/overlay/party");
    println!("  area:       http://{addr}/overlay/area");
    println!("  battle:     http://{addr}/overlay/battle");

    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
struct Config {
    host: IpAddr,
    port: u16,
    snapshot_path: PathBuf,
    sprite_dir: PathBuf,
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
                _ => {}
            }
        }

        Self {
            host,
            port,
            snapshot_path,
            sprite_dir,
        }
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn overlay() -> Html<&'static str> {
    Html(OVERLAY_HTML)
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
    let body = format!(
        "{{\"ok\":true,\"snapshot_exists\":{snapshot_exists},\"sprites_exist\":{sprites_exist},\"snapshot_path\":\"{}\",\"sprite_dir\":\"{}\"}}",
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

async fn sprite_png(Path(species_id): Path<u16>, State(state): State<AppState>) -> Response {
    let path = state.sprite_dir.join(format!("{species_id}.png"));
    match tokio::fs::read(path).await {
        Ok(bytes) => binary(StatusCode::OK, bytes, "image/png"),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
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
}
