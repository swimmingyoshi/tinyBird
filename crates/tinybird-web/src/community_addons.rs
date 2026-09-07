//! Account-owned add-on releases and pinned installations. SQLite owns durability.
use crate::{auth, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::Path as FsPath,
    sync::{Arc, Mutex},
    time::Duration,
};
use tinybird_addons::Manifest;

#[derive(Clone, Debug)]
pub struct Store(Arc<Mutex<Connection>>);

#[derive(Debug)]
pub struct Error(StatusCode, String);
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        eprintln!("add-on database: {error}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Add-on storage is unavailable.".into(),
        )
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        crate::json_error(self.0, &self.1)
    }
}
fn bad(message: &str) -> Error {
    Error(StatusCode::BAD_REQUEST, message.into())
}
fn missing() -> Error {
    Error(StatusCode::NOT_FOUND, "Add-on release not found.".into())
}

impl Store {
    pub fn open(path: &FsPath) -> Result<Self, Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 1 {
            return Err("The add-on database needs a newer tinyBird server.".into());
        }
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS addon_packages (
            id TEXT PRIMARY KEY, owner TEXT NOT NULL, author TEXT NOT NULL,
            addon_key TEXT NOT NULL, listed INTEGER NOT NULL DEFAULT 1,
            UNIQUE(owner, addon_key));
          CREATE TABLE IF NOT EXISTS addon_releases (
            package TEXT NOT NULL REFERENCES addon_packages(id), release INTEGER NOT NULL,
            manifest TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL,
            license TEXT NOT NULL, created INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY(package, release));
          CREATE TABLE IF NOT EXISTS addon_installs (
            owner TEXT NOT NULL, package TEXT NOT NULL, release INTEGER NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(owner, package),
            FOREIGN KEY(package, release) REFERENCES addon_releases(package, release));
          CREATE TABLE IF NOT EXISTS addon_reports (
            owner TEXT NOT NULL, package TEXT NOT NULL REFERENCES addon_packages(id),
            reason TEXT NOT NULL, created INTEGER NOT NULL DEFAULT (unixepoch()),
            PRIMARY KEY(owner, package));
          CREATE INDEX IF NOT EXISTS addon_release_time ON addon_releases(created);
          PRAGMA user_version=1;",
        )?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut connection = store
                .0
                .lock()
                .map_err(|_| bad("Add-on storage is unavailable."))?;
            operation(&mut connection)
        })
        .await
        .map_err(|_| bad("Add-on storage task failed."))?
    }
}

#[derive(Deserialize, Default)]
pub struct Search {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: u32,
    #[serde(default)]
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    manifest: Value,
    description: String,
    license: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installation {
    release: u32,
    enabled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    reason: String,
}

async fn user(state: &AppState, headers: &HeaderMap) -> Result<auth::User, Response> {
    let user = auth::current_user(
        &state.auth,
        &state.sessions,
        crate::session_id(headers).as_deref(),
    )
    .await
    .map_err(|err| crate::auth_error(&err))?;
    if headers
        .get("x-tinybird-addon-owner")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|id| id != user.id)
    {
        return Err(crate::json_error(
            StatusCode::CONFLICT,
            "Your account changed in another tab. Refresh this page.",
        ));
    }
    Ok(user)
}
fn mutation(headers: &HeaderMap) -> Result<(), Response> {
    // A cross-origin form cannot send this header; no CORS permission is granted.
    if headers
        .get("x-tinybird-addons")
        .and_then(|h| h.to_str().ok())
        != Some("1")
    {
        return Err(crate::json_error(
            StatusCode::FORBIDDEN,
            "Use the add-on manager to make changes.",
        ));
    }
    Ok(())
}
fn reply(result: Result<Value, Error>) -> Response {
    let mut response = match result {
        Ok(value) => Json(value).into_response(),
        Err(err) => err.into_response(),
    };
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}

fn release_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(
        json!({"id": row.get::<_, String>(0)?, "author": row.get::<_, String>(1)?,
        "release": row.get::<_, u32>(2)?, "name": row.get::<_, String>(3)?,
        "description": row.get::<_, String>(4)?, "license": row.get::<_, String>(5)?,
        "created": row.get::<_, i64>(6)?}),
    )
}

pub async fn catalog(State(state): State<AppState>, Query(search): Query<Search>) -> Response {
    if search.q.len() > 120 || !["", "memory_sheet"].contains(&search.kind.as_str()) {
        return bad("Search is too long.").into_response();
    }
    reply(state.community_addons.run(move |db| {
        let mut stmt = db.prepare("SELECT p.id,p.author,r.release,r.name,r.description,r.license,r.created,r.manifest
            FROM addon_packages p JOIN addon_releases r ON r.package=p.id
            WHERE p.listed=1 AND r.release=(SELECT MAX(release) FROM addon_releases WHERE package=p.id)
            AND (instr(lower(r.name),lower(?1))>0 OR instr(lower(r.description),lower(?1))>0 OR instr(lower(r.manifest),lower(?1))>0)
            AND (?3='' OR json_extract(r.manifest,?4)=?3)
            ORDER BY r.created DESC,p.id LIMIT 40 OFFSET ?2")?;
        let entries = stmt.query_map(params![search.q, search.offset.min(100000), search.kind, "$.\"$comment\".kind"], |row| {
            let mut entry = release_row(row)?;
            let manifest: Value = serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or(Value::Null);
            entry["kind"] = json!(if manifest["$comment"]["kind"] == "memory_sheet" { "memory_sheet" } else { "reader" });
            Ok(entry)
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"addons": entries, "next_offset": if entries.len() == 40 { Some(search.offset + 40) } else { None }}))
    }).await)
}

pub async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    reply(state.community_addons.run(move |db| {
        let mut stmt = db.prepare("SELECT p.id,p.author,r.release,r.name,r.description,r.license,r.created,r.manifest
            FROM addon_packages p JOIN addon_releases r ON r.package=p.id WHERE p.id=?1 AND p.listed=1 ORDER BY r.release DESC")?;
        let entries = stmt.query_map([&id], |row| {
            let mut entry = release_row(row)?;
            let manifest: String = row.get(7)?;
            entry["manifest"] = serde_json::from_str(&manifest).unwrap_or(Value::Null);
            Ok(entry)
        })?.collect::<Result<Vec<_>, _>>()?;
        if entries.is_empty() { return Err(missing()); }
        Ok(json!({"releases": entries}))
    }).await)
}

fn publish_release(
    db: &mut Connection,
    owner: &str,
    author: &str,
    request: Publish,
) -> Result<Value, Error> {
    let manifest = Manifest::parse(&request.manifest.to_string()).map_err(|err| bad(&err))?;
    if request.description.trim().is_empty() || request.description.len() > 2000 {
        return Err(bad("Write a description of 1–2000 bytes."));
    }
    if !["MIT", "Apache-2.0", "CC0-1.0", "CC-BY-4.0"].contains(&request.license.as_str()) {
        return Err(bad("Choose a supported sharing license."));
    }
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let recent: u32 = tx.query_row("SELECT count(*) FROM addon_releases r JOIN addon_packages p ON p.id=r.package WHERE p.owner=?1 AND r.created>unixepoch()-3600", [owner], |r| r.get(0))?;
    if recent >= 10 {
        return Err(Error(
            StatusCode::TOO_MANY_REQUESTS,
            "Publish at most 10 releases per hour.".into(),
        ));
    }
    let existing: Option<(String, bool)> = tx
        .query_row(
            "SELECT id,listed FROM addon_packages WHERE owner=?1 AND addon_key=?2",
            params![owner, manifest.addon_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let id = if let Some((id, listed)) = existing {
        if !listed {
            return Err(bad(
                "This add-on was withdrawn and cannot be republished under the same ID.",
            ));
        }
        id
    } else {
        let count: u32 = tx.query_row(
            "SELECT count(*) FROM addon_packages WHERE owner=?1",
            [owner],
            |r| r.get(0),
        )?;
        if count >= 50 {
            return Err(bad("An account can publish at most 50 add-ons."));
        }
        let mut bytes = [0u8; 16];
        getrandom::getrandom(&mut bytes).map_err(|_| bad("Could not create an add-on ID."))?;
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        tx.execute(
            "INSERT INTO addon_packages(id,owner,author,addon_key) VALUES(?1,?2,?3,?4)",
            params![id, owner, author, manifest.addon_id],
        )?;
        id
    };
    let release: u32 = tx.query_row(
        "SELECT COALESCE(MAX(release),0)+1 FROM addon_releases WHERE package=?1",
        [&id],
        |r| r.get(0),
    )?;
    if release > 50 {
        return Err(bad("An add-on can have at most 50 releases."));
    }
    tx.execute("INSERT INTO addon_releases(package,release,manifest,name,description,license) VALUES(?1,?2,?3,?4,?5,?6)",
        params![id, release, serde_json::to_string(&manifest).map_err(|_| bad("Invalid manifest."))?, manifest.display_name, request.description, request.license])?;
    tx.commit()?;
    Ok(json!({"id": id, "release": release, "url": format!("/addons?id={id}&release={release}")}))
}

pub async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Publish>,
) -> Response {
    if let Err(response) = mutation(&headers) {
        return response;
    }
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    reply(
        state
            .community_addons
            .run(move |db| {
                publish_release(
                    db,
                    &owner.id,
                    owner.display_name.as_deref().unwrap_or("Community member"),
                    request,
                )
            })
            .await,
    )
}

pub async fn installed(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    reply(state.community_addons.run(move |db| {
        let mut stmt = db.prepare("SELECT p.id,p.author,r.release,r.name,r.description,r.license,r.created,i.enabled,r.manifest,p.listed
            FROM addon_installs i JOIN addon_packages p ON p.id=i.package JOIN addon_releases r ON r.package=i.package AND r.release=i.release
            WHERE i.owner=?1 ORDER BY p.id")?;
        let entries = stmt.query_map([&owner.id], |row| {
            let mut entry = release_row(row)?;
            let listed: bool = row.get(9)?;
            entry["enabled"] = json!(row.get::<_, bool>(7)?);
            entry["available"] = json!(listed);
            if listed {
                let text: String = row.get(8)?;
                let mut manifest: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                manifest["addon_id"] = json!(format!("community.{}", row.get::<_, String>(0)?));
                entry["manifest"] = manifest;
            }
            Ok(entry)
        })?.collect::<Result<Vec<_>, _>>()?;
        let mut own = db.prepare("SELECT id,addon_key,listed FROM addon_packages WHERE owner=?1 ORDER BY id")?;
        let published = own.query_map([&owner.id], |r| Ok(json!({"id":r.get::<_,String>(0)?,"addon_id":r.get::<_,String>(1)?,"available":r.get::<_,bool>(2)?})))?.collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"installed": entries, "published": published}))
    }).await)
}

fn set_install(
    db: &mut Connection,
    owner: &str,
    id: &str,
    request: Installation,
) -> Result<Value, Error> {
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM addon_releases r JOIN addon_packages p ON p.id=r.package WHERE p.id=?1 AND r.release=?2 AND p.listed=1)", params![id,request.release], |r| r.get(0))?;
    if !exists {
        return Err(missing());
    }
    let count: u32 = tx.query_row(
        "SELECT count(*) FROM addon_installs WHERE owner=?1 AND package<>?2",
        params![owner, id],
        |r| r.get(0),
    )?;
    if count >= 12 {
        return Err(bad(
            "Install at most 12 community add-ons. Remove one before installing another.",
        ));
    }
    tx.execute("INSERT INTO addon_installs(owner,package,release,enabled) VALUES(?1,?2,?3,?4) ON CONFLICT(owner,package) DO UPDATE SET release=excluded.release,enabled=excluded.enabled", params![owner,id,request.release,request.enabled])?;
    tx.commit()?;
    Ok(json!({"ok":true}))
}

pub async fn install(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Installation>,
) -> Response {
    if let Err(response) = mutation(&headers) {
        return response;
    }
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    reply(
        state
            .community_addons
            .run(move |db| set_install(db, &owner.id, &id, request))
            .await,
    )
}

pub async fn uninstall(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = mutation(&headers) {
        return response;
    }
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    reply(
        state
            .community_addons
            .run(move |db| {
                db.execute(
                    "DELETE FROM addon_installs WHERE owner=?1 AND package=?2",
                    params![owner.id, id],
                )?;
                Ok(json!({"ok":true}))
            })
            .await,
    )
}

pub async fn withdraw(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = mutation(&headers) {
        return response;
    }
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    reply(
        state
            .community_addons
            .run(move |db| {
                let affected = db.execute(
                    "UPDATE addon_packages SET listed=0 WHERE id=?1 AND (owner=?2 OR ?3='admin')",
                    params![id, owner.id, owner.role],
                )?;
                if affected == 0 {
                    return Err(missing());
                }
                Ok(json!({"ok":true}))
            })
            .await,
    )
}

pub async fn report(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Report>,
) -> Response {
    if let Err(response) = mutation(&headers) {
        return response;
    }
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    if request.reason.trim().is_empty() || request.reason.len() > 1000 {
        return bad("Reports need 1–1000 bytes.").into_response();
    }
    reply(state.community_addons.run(move |db| {
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM addon_packages WHERE id=?1 AND listed=1)", [&id], |r| r.get(0))?;
        if !exists { return Err(missing()); }
        let count: u32 = db.query_row("SELECT count(*) FROM addon_reports WHERE owner=?1 AND created>unixepoch()-3600", [&owner.id], |r| r.get(0))?;
        if count >= 10 { return Err(Error(StatusCode::TOO_MANY_REQUESTS, "At most 10 reports per hour.".into())); }
        let added = db.execute("INSERT INTO addon_reports(owner,package,reason) VALUES(?1,?2,?3) ON CONFLICT(owner,package) DO NOTHING", params![owner.id,id,request.reason])?;
        if added == 0 { return Err(Error(StatusCode::CONFLICT, "You have already reported this add-on.".into())); }
        Ok(json!({"ok":true}))
    }).await)
}

pub async fn reports(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let owner = match user(&state, &headers).await {
        Ok(user) => user,
        Err(response) => return response,
    };
    if owner.role != "admin" {
        return StatusCode::FORBIDDEN.into_response();
    }
    reply(state.community_addons.run(move |db| {
        let mut stmt = db.prepare("SELECT r.package,r.reason,r.created FROM addon_reports r JOIN addon_packages p ON p.id=r.package WHERE p.listed=1 ORDER BY r.created DESC LIMIT 100")?;
        let reports = stmt.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"reason":r.get::<_,String>(1)?,"created":r.get::<_,i64>(2)?})))?.collect::<Result<Vec<_>,_>>()?;
        Ok(json!({"reports":reports}))
    }).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Publish {
        Publish {
            description: "A tested counter".into(),
            license: "MIT".into(),
            manifest: json!({
                "addon_id":"counter", "display_name":"Counter", "matches":{"game_code":["BPRE"]},
                "sections":[{"id":"stats","title":"Stats","kind":"key_value","fields":[{"label":"Count","read":{"u8":"0x02000000"}}]}]
            }),
        }
    }
    #[tokio::test]
    async fn handlers_enforce_session_ownership_and_withdrawal() {
        let sessions = Arc::new(auth::Sessions::new());
        for id in ["alice", "bob"] {
            sessions.add_test_user(
                id,
                auth::User {
                    id: id.into(),
                    email: format!("{id}@example.test"),
                    display_name: Some(id.into()),
                    role: "member".into(),
                },
            );
        }
        let state = AppState {
            community_addons: Store::open(FsPath::new(":memory:")).unwrap(),
            snapshot_path: "unused".into(),
            sprite_dir: "unused".into(),
            wasm_path: "unused".into(),
            rom_dir: "unused".into(),
            bios_path: "unused".into(),
            overlay_enabled: false,
            addon_dir: "unused".into(),
            media: crate::media::MediaConfig::from_env(),
            auth: auth::AuthConfig::from_env(),
            contact: crate::contact::ContactConfig::from_env(),
            contact_throttle: Arc::new(crate::contact::Throttle::new()),
            sessions,
            lobby: Arc::new(crate::lobby::Lobby::new()),
            serve_local_roms: false,
        };
        fn headers(id: &str) -> HeaderMap {
            let mut h = HeaderMap::new();
            h.insert("cookie", format!("tinybird_session={id}").parse().unwrap());
            h.insert("x-tinybird-addons", "1".parse().unwrap());
            h
        }
        async fn body(response: Response) -> Value {
            serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap(),
            )
            .unwrap()
        }
        assert_eq!(
            publish(State(state.clone()), headers("unknown"), Json(request()))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            publish(State(state.clone()), HeaderMap::new(), Json(request()))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let mut sheet = request();
        sheet.manifest["$comment"] = json!({"kind":"memory_sheet","memory_sheet_version":1});
        let response = publish(State(state.clone()), headers("alice"), Json(sheet)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = body(response).await;
        let id = value["id"].as_str().unwrap().to_string();
        let found = body(catalog(State(state.clone()), Query(Search { q: "02000000".into(), kind: "memory_sheet".into(), offset: 0 })).await).await;
        assert_eq!(found["addons"].as_array().unwrap().len(), 1);
        assert_eq!(found["addons"][0]["kind"], "memory_sheet");
        let absent = body(catalog(State(state.clone()), Query(Search { q: "missing-field".into(), kind: "memory_sheet".into(), offset: 0 })).await).await;
        assert!(absent["addons"].as_array().unwrap().is_empty());
        let sheet_detail = body(detail(State(state.clone()), Path(id.clone())).await).await;
        assert_eq!(sheet_detail["releases"][0]["manifest"]["$comment"]["kind"], "memory_sheet");
        let response = install(
            State(state.clone()),
            Path(id.clone()),
            headers("bob"),
            Json(Installation {
                release: 1,
                enabled: true,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let alice = body(installed(State(state.clone()), headers("alice")).await).await;
        assert_eq!(alice["installed"].as_array().unwrap().len(), 0);
        let bob = body(installed(State(state.clone()), headers("bob")).await).await;
        assert_eq!(
            bob["installed"][0]["manifest"]["addon_id"],
            format!("community.{id}")
        );
        assert!(!bob.to_string().contains("@example.test"));
        let mut wrong = headers("bob");
        wrong.insert("x-tinybird-addon-owner", "alice".parse().unwrap());
        assert_eq!(
            uninstall(State(state.clone()), Path(id.clone()), wrong)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            withdraw(State(state.clone()), Path(id.clone()), headers("bob"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            reports(State(state.clone()), headers("bob")).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            withdraw(State(state.clone()), Path(id.clone()), headers("alice"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            detail(State(state.clone()), Path(id.clone()))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let bob = body(installed(State(state), headers("bob")).await).await;
        assert_eq!(bob["installed"][0]["available"], false);
        assert!(bob["installed"][0].get("manifest").is_none());
    }
    #[test]
    fn releases_are_immutable_owned_and_installations_are_pinned() {
        let store = Store::open(FsPath::new(":memory:")).unwrap();
        let mut db = store.0.lock().unwrap();
        let first = publish_release(&mut db, "alice", "Alice", request()).unwrap();
        let id = first["id"].as_str().unwrap();
        set_install(
            &mut db,
            "bob",
            id,
            Installation {
                release: 1,
                enabled: true,
            },
        )
        .unwrap();
        let mut update = request();
        update.description = "Second release".into();
        let second = publish_release(&mut db, "alice", "Alice", update).unwrap();
        assert_eq!(second["id"], first["id"]);
        assert_eq!(second["release"], 2);
        let pinned: u32 = db
            .query_row(
                "SELECT release FROM addon_installs WHERE owner='bob'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pinned, 1);
        let original: String = db
            .query_row(
                "SELECT description FROM addon_releases WHERE package=?1 AND release=1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original, "A tested counter");
        let another = publish_release(&mut db, "bob", "Bob", request()).unwrap();
        assert_ne!(
            another["id"], first["id"],
            "same addon key cannot take over another author"
        );
        assert_eq!(
            db.execute(
                "DELETE FROM addon_installs WHERE owner='alice' AND package=?1",
                [id]
            )
            .unwrap(),
            0
        );
        assert!(set_install(
            &mut db,
            "bob",
            id,
            Installation {
                release: 99,
                enabled: true
            }
        )
        .is_err());
        let affected = db
            .execute(
                "UPDATE addon_packages SET listed=0 WHERE id=?1 AND owner='bob'",
                [id],
            )
            .unwrap();
        assert_eq!(affected, 0);
        db.execute(
            "UPDATE addon_packages SET listed=0 WHERE id=?1 AND owner='alice'",
            [id],
        )
        .unwrap();
        assert!(set_install(
            &mut db,
            "charlie",
            id,
            Installation {
                release: 1,
                enabled: true
            }
        )
        .is_err());
        assert!(publish_release(&mut db, "alice", "Alice", request()).is_err());
    }
    #[test]
    fn invalid_packages_never_reach_storage_and_mutations_need_custom_header() {
        let store = Store::open(FsPath::new(":memory:")).unwrap();
        let mut db = store.0.lock().unwrap();
        let mut invalid = request();
        invalid.manifest["sections"][0]["fields"][0]["read"]["u8"] = json!("0x04000000");
        assert!(publish_release(&mut db, "alice", "Alice", invalid).is_err());
        let count: u32 = db
            .query_row("SELECT count(*) FROM addon_packages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert!(mutation(&HeaderMap::new()).is_err());
        let mut headers = HeaderMap::new();
        headers.insert("x-tinybird-addons", "1".parse().unwrap());
        assert!(mutation(&headers).is_ok());
    }
    #[test]
    fn releases_and_settings_survive_reopening_database() {
        let path = std::env::temp_dir().join(format!(
            "tinybird-addon-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let store = Store::open(&path).unwrap();
            let mut db = store.0.lock().unwrap();
            let published = publish_release(&mut db, "alice", "Alice", request()).unwrap();
            set_install(
                &mut db,
                "bob",
                published["id"].as_str().unwrap(),
                Installation {
                    release: 1,
                    enabled: false,
                },
            )
            .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            let db = store.0.lock().unwrap();
            let enabled: bool = db
                .query_row(
                    "SELECT enabled FROM addon_installs WHERE owner='bob'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!enabled);
        }
        std::fs::remove_file(path).unwrap();
    }
}
