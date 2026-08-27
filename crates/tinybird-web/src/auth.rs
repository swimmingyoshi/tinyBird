//! Sign-in against the 0xstash auth service.
//!
//! **The browser never talks to the auth service and never holds a token.**
//!
//! The obvious design has the page call `auth.0xstash.dev` directly and keep a
//! 15-minute access token in memory, refreshing it from a cookie. That cookie is
//! set by `auth.0xstash.dev` while the page runs on `127.0.0.1:8877`, which
//! makes it a third-party cookie: Safari blocks those by default and Chrome and
//! Edge are phasing them out. When it is blocked the refresh call silently sends
//! nothing and the player is signed out every fifteen minutes and on every
//! reload — intermittently, depending on browser settings, which is the worst
//! kind of bug to chase.
//!
//! So this server does the talking. It holds the access and refresh tokens, and
//! hands the browser only an opaque first-party session id:
//!
//! ```text
//! browser ──cookie: tinybird_session──▶ this server ──bearer token──▶ auth service
//! ```
//!
//! Nothing cryptographic reaches the page, which satisfies the service's own
//! rules (no tokens in `localStorage`, in URLs, or in browser logs) more
//! strictly than the direct flow would, and sidesteps third-party cookies
//! entirely.

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Auth service used when none is configured.
const DEFAULT_BASE_URL: &str = "https://auth.0xstash.dev";
/// Project slug this application signs in against.
const DEFAULT_PROJECT: &str = "tinybird";
/// Name of the first-party cookie holding the session id.
pub const SESSION_COOKIE: &str = "tinybird_session";
/// Refresh an access token this long before it actually expires, so a request
/// cannot lose a race with the clock.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Minimum password length the service enforces; checked here too so the UI can
/// say so without a round trip.
pub const MIN_PASSWORD_LEN: usize = 12;

/// Where and how to reach the auth service.
#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub base_url: String,
    pub project: String,
    /// Project client secret, used only for introspection.
    ///
    /// Server-side only. It never appears in a response, a log line, or
    /// anything the browser can reach.
    secret: Option<String>,
}

impl AuthConfig {
    pub fn from_env() -> Self {
        let read = |name: &str| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        Self {
            base_url: read("TINYBIRD_AUTH_URL").unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            project: read("TINYBIRD_AUTH_PROJECT").unwrap_or_else(|| DEFAULT_PROJECT.to_string()),
            secret: read("TINYBIRD_AUTH_PROJECT_SECRET"),
        }
    }

    /// Whether accounts are available. Without a secret this server cannot
    /// verify a token, so it does not pretend to offer sign-in.
    pub fn is_configured(&self) -> bool {
        self.secret.is_some()
    }
}

/// A signed-in person, as the browser is allowed to see them.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct User {
    /// Stable identifier, from the token's `sub` claim.
    ///
    /// This is what owns stored saves. Never the email: an address can be
    /// changed, and it would put a personal identifier in object names.
    pub id: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub role: String,
}

/// What the auth service says about a token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub sub: String,
    pub project: String,
    pub aud: String,
    pub role: String,
}

/// One signed-in browser.
#[derive(Clone, Debug)]
struct Session {
    access_token: String,
    /// When the access token stops being usable.
    access_expires_at: SystemTime,
    /// Cookies the auth service set, replayed verbatim on refresh.
    ///
    /// Held per session rather than in a shared jar: one jar across all users
    /// would let a refresh performed for one person rotate another's token.
    cookies: Vec<String>,
    user: User,
}

/// Every signed-in browser, by session id.
///
/// In memory, so a restart signs everyone out. That is the right trade for a
/// server you run yourself; a deployment with more than one process would need
/// this in shared storage.
#[derive(Debug, Default)]
pub struct Sessions {
    inner: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, id: String, session: Session) {
        if let Ok(mut map) = self.inner.lock() {
            map.insert(id, session);
        }
    }

    fn get(&self, id: &str) -> Option<Session> {
        self.inner.lock().ok()?.get(id).cloned()
    }

    fn remove(&self, id: &str) -> Option<Session> {
        self.inner.lock().ok()?.remove(id)
    }

    fn update(&self, id: &str, session: Session) {
        if let Ok(mut map) = self.inner.lock() {
            if let Some(slot) = map.get_mut(id) {
                *slot = session;
            }
        }
    }

}

/// Why an auth operation failed, in terms the UI can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No project secret, so accounts are switched off.
    NotConfigured,
    /// The caller is not signed in, or the session has lapsed.
    NoSession,
    /// The service rejected the request, with its own status and message.
    Service { status: u16, message: String },
    /// Could not reach the service at all.
    Unreachable(String),
}

impl AuthError {
    /// The status to answer the browser with.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotConfigured => 503,
            Self::NoSession => 401,
            // Pass the service's own status through, so a 409 stays a 409 and
            // the page can say "that email is already registered" rather than
            // "something went wrong".
            Self::Service { status, .. } => *status,
            Self::Unreachable(_) => 502,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotConfigured => {
                "Accounts are off. Set TINYBIRD_AUTH_PROJECT_SECRET in .env and restart."
                    .to_string()
            }
            Self::NoSession => "Sign in first.".to_string(),
            Self::Service { message, .. } => message.clone(),
            Self::Unreachable(why) => format!("Could not reach the auth service: {why}"),
        }
    }
}

/// A new, unguessable session id.
fn new_session_id() -> String {
    let mut bytes = [0u8; 32];
    // A predictable session id is a login for everyone else, so this comes from
    // OS entropy rather than a general-purpose PRNG. If the OS cannot provide
    // randomness the only safe move is to refuse to mint a session.
    getrandom::getrandom(&mut bytes).expect("the OS must provide randomness for session ids");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // Cookies are tracked per session by hand, not in a process-wide jar.
        .build()
        .map_err(|err| AuthError::Unreachable(err.to_string()))
}

/// Pull the `name=value` part out of every `Set-Cookie` on a response.
fn collect_cookies(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .map(|pair| pair.trim().to_string())
        .filter(|pair| !pair.is_empty())
        .collect()
}

/// Read the message out of an error body, whatever shape it arrived in.
fn error_message(body: &str, status: u16) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    for key in ["detail", "message", "error"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return text.to_string();
        }
        // Some errors nest the text one level down.
        if let Some(text) = value
            .get(key)
            .and_then(|v| v.get("message"))
            .and_then(|v| v.as_str())
        {
            return text.to_string();
        }
    }
    match status {
        401 => "Wrong email or password.".to_string(),
        403 => "Registration is closed for this project.".to_string(),
        409 => "That email is already registered.".to_string(),
        422 => format!("The service rejected that request (needs {MIN_PASSWORD_LEN}+ characters)."),
        429 => "Too many attempts. Wait a moment and try again.".to_string(),
        _ => {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                format!("The auth service returned {status}.")
            } else {
                trimmed.chars().take(200).collect()
            }
        }
    }
}

fn now_plus(seconds: u64) -> SystemTime {
    SystemTime::now() + Duration::from_secs(seconds)
}

/// Turn an `expiresAt` field into a deadline.
///
/// The service reports an absolute epoch time; falling back to a short window
/// keeps a surprising shape from producing a token we believe in forever.
fn expiry_from(value: &serde_json::Value) -> SystemTime {
    let seconds = value
        .get("expiresAt")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));

    match seconds {
        // Tolerate milliseconds as well as seconds.
        Some(at) if at > 100_000_000_000 => UNIX_EPOCH + Duration::from_millis(at as u64),
        Some(at) if at > 0 => UNIX_EPOCH + Duration::from_secs(at as u64),
        _ => now_plus(15 * 60),
    }
}

/// Ask the auth service what a token means.
///
/// This is the only call that uses the project secret. Running it once after
/// sign-in does two jobs: it yields the canonical `sub` to own stored saves,
/// and it proves the token was minted for this project rather than another one
/// on the same service.
pub async fn introspect(config: &AuthConfig, token: &str) -> Result<Claims, AuthError> {
    let secret = config.secret.as_deref().ok_or(AuthError::NotConfigured)?;

    let response = client()?
        .post(format!("{}/api/auth/introspect", config.base_url))
        .header("X-Project", &config.project)
        .header("X-Project-Secret", secret)
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    if !(200..300).contains(&status) {
        return Err(AuthError::Service {
            status,
            message: error_message(&body, status),
        });
    }

    let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    parse_claims(&value, &config.project)
}

/// Read and check an introspection response.
///
/// Kept separate from the request so the rules can be tested without a network.
pub fn parse_claims(value: &serde_json::Value, project: &str) -> Result<Claims, AuthError> {
    let reject = |message: &str| AuthError::Service {
        status: 401,
        message: message.to_string(),
    };

    if value.get("active").and_then(|v| v.as_bool()) != Some(true) {
        return Err(reject("That session is no longer active."));
    }

    let claims = value.get("claims").ok_or_else(|| reject("Malformed token."))?;
    let text = |key: &str| {
        claims
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let sub = text("sub");
    if sub.is_empty() {
        return Err(reject("That token identifies no one."));
    }

    // A token minted for another project on the same service must not be
    // accepted here, whatever else it says.
    let token_project = text("project");
    let aud = text("aud");
    if !token_project.is_empty() && token_project != project {
        return Err(reject("That token belongs to a different project."));
    }
    if !aud.is_empty() && aud != project {
        return Err(reject("That token was issued for a different audience."));
    }

    Ok(Claims {
        sub,
        project: token_project,
        aud,
        role: text("role"),
    })
}

/// Sign in or register, and open a session for the browser.
///
/// Returns the session id to put in the first-party cookie, and the user.
pub async fn sign_in(
    config: &AuthConfig,
    sessions: &Sessions,
    path: &str,
    body: serde_json::Value,
) -> Result<(String, User), AuthError> {
    if !config.is_configured() {
        return Err(AuthError::NotConfigured);
    }

    let mut payload = body;
    // The project is ours to state, not the browser's.
    payload["project"] = serde_json::Value::String(config.project.clone());

    let response = client()?
        .post(format!("{}{path}", config.base_url))
        .header("X-Project", &config.project)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    let status = response.status().as_u16();
    let cookies = collect_cookies(&response);
    let text = response
        .text()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    if !(200..300).contains(&status) {
        return Err(AuthError::Service {
            status,
            message: error_message(&text, status),
        });
    }

    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let access_token = value
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AuthError::Service {
            status: 502,
            message: "The auth service returned no access token.".to_string(),
        })?
        .to_string();

    // Introspect rather than trusting the response body: this is what pins the
    // owner id to the token's own subject and confirms the audience.
    let claims = introspect(config, &access_token).await?;

    let profile = value.get("user").cloned().unwrap_or_default();
    let user = User {
        id: claims.sub.clone(),
        email: profile
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        display_name: profile
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        role: if claims.role.is_empty() {
            profile
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string()
        } else {
            claims.role.clone()
        },
    };

    let id = new_session_id();
    sessions.insert(
        id.clone(),
        Session {
            access_token,
            access_expires_at: expiry_from(&value),
            cookies,
            user: user.clone(),
        },
    );
    Ok((id, user))
}

/// The signed-in user for a session id, refreshing the token if it is stale.
pub async fn current_user(
    config: &AuthConfig,
    sessions: &Sessions,
    session_id: Option<&str>,
) -> Result<User, AuthError> {
    let id = session_id.ok_or(AuthError::NoSession)?;
    let session = sessions.get(id).ok_or(AuthError::NoSession)?;

    if SystemTime::now() + REFRESH_MARGIN < session.access_expires_at {
        return Ok(session.user);
    }

    match refresh(config, &session).await {
        Ok(mut fresh) => {
            // Re-introspect the new token rather than carrying the old user
            // forward. A refresh only proves the browser still holds the
            // cookie; introspection is what reports an account that has since
            // been deactivated, and without this check such an account would
            // keep working for the whole 30-day refresh window.
            match introspect(config, &fresh.access_token).await {
                Ok(claims) if claims.sub == fresh.user.id => {
                    fresh.user.role = claims.role;
                }
                Ok(_) => {
                    // The token now names somebody else. Nothing good explains
                    // that, so the session ends.
                    sessions.remove(id);
                    return Err(AuthError::NoSession);
                }
                Err(err) => {
                    sessions.remove(id);
                    return Err(err);
                }
            }
            let user = fresh.user.clone();
            sessions.update(id, fresh);
            Ok(user)
        }
        Err(err) => {
            // A refresh that fails is a session that is over; leaving it in the
            // map would retry a dead token on every request.
            sessions.remove(id);
            Err(err)
        }
    }
}

/// Trade the stored refresh cookie for a new access token.
async fn refresh(config: &AuthConfig, session: &Session) -> Result<Session, AuthError> {
    if session.cookies.is_empty() {
        return Err(AuthError::NoSession);
    }

    let response = client()?
        .post(format!("{}/api/auth/refresh", config.base_url))
        .header("X-Project", &config.project)
        .header(reqwest::header::COOKIE, session.cookies.join("; "))
        .send()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    let status = response.status().as_u16();
    let rotated = collect_cookies(&response);
    let text = response
        .text()
        .await
        .map_err(|err| AuthError::Unreachable(err.to_string()))?;

    if !(200..300).contains(&status) {
        return Err(AuthError::NoSession);
    }

    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let access_token = value
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or(AuthError::NoSession)?
        .to_string();

    Ok(Session {
        access_token,
        access_expires_at: expiry_from(&value),
        // Refresh tokens rotate on every use, so the new cookie replaces the
        // old one. Keeping the old one would replay a spent token.
        cookies: if rotated.is_empty() {
            session.cookies.clone()
        } else {
            rotated
        },
        user: session.user.clone(),
    })
}

/// End a session here and at the service.
pub async fn sign_out(config: &AuthConfig, sessions: &Sessions, session_id: Option<&str>) {
    let Some(id) = session_id else {
        return;
    };
    let Some(session) = sessions.remove(id) else {
        return;
    };

    // Best effort: the local session is already gone, which is what matters for
    // this browser. Telling the service lets it drop the refresh token too.
    let Ok(client) = client() else {
        return;
    };
    let _ = client
        .post(format!("{}/api/auth/logout", config.base_url))
        .header("X-Project", &config.project)
        .header(reqwest::header::COOKIE, session.cookies.join("; "))
        .send()
        .await;
}

/// The session id from a request's `Cookie` header, if there is one.
pub fn session_from_cookies(header: Option<&str>) -> Option<String> {
    let header = header?;
    header.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim().to_string())
    })
}

/// The `Set-Cookie` value that establishes a session.
///
/// `HttpOnly` keeps the id out of page scripts, and `SameSite=Lax` keeps it off
/// cross-site requests while still surviving ordinary navigation. It is
/// first-party to this server, so none of the third-party cookie restrictions
/// that motivated this whole module apply to it.
pub fn session_cookie(id: &str, secure: bool) -> String {
    let mut cookie =
        format!("{SESSION_COOKIE}={id}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// The `Set-Cookie` value that clears a session.
pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims_json(active: bool, sub: &str, project: &str, aud: &str) -> serde_json::Value {
        serde_json::json!({
            "active": active,
            "claims": { "sub": sub, "project": project, "aud": aud, "role": "user", "exp": 1 }
        })
    }

    #[test]
    fn a_live_token_for_this_project_is_accepted() {
        let claims = parse_claims(&claims_json(true, "abc-123", "tinybird", "tinybird"), "tinybird")
            .expect("should be accepted");
        assert_eq!(claims.sub, "abc-123");
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn an_inactive_token_is_refused() {
        let err = parse_claims(&claims_json(false, "abc", "tinybird", "tinybird"), "tinybird")
            .unwrap_err();
        assert_eq!(err.status(), 401);
    }

    /// The service hosts more than one project; a token from another one must
    /// not be able to own saves here.
    #[test]
    fn a_token_from_another_project_is_refused() {
        assert!(parse_claims(&claims_json(true, "abc", "someone-else", "tinybird"), "tinybird").is_err());
        assert!(parse_claims(&claims_json(true, "abc", "tinybird", "someone-else"), "tinybird").is_err());
    }

    #[test]
    fn a_token_identifying_no_one_is_refused() {
        assert!(parse_claims(&claims_json(true, "", "tinybird", "tinybird"), "tinybird").is_err());
    }

    #[test]
    fn a_response_with_no_claims_is_refused() {
        let value = serde_json::json!({ "active": true });
        assert!(parse_claims(&value, "tinybird").is_err());
    }

    #[test]
    fn session_ids_are_unguessable_and_distinct() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64, "32 bytes of entropy, hex encoded");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_session_cookie_is_read_back_from_a_header() {
        let header = format!("theme=dark; {SESSION_COOKIE}=abc123; other=1");
        assert_eq!(session_from_cookies(Some(&header)), Some("abc123".to_string()));
        assert_eq!(session_from_cookies(Some("theme=dark")), None);
        assert_eq!(session_from_cookies(None), None);
    }

    #[test]
    fn the_session_cookie_is_not_reachable_from_page_scripts() {
        let cookie = session_cookie("abc", false);
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(!cookie.contains("Secure"));
        assert!(session_cookie("abc", true).contains("Secure"));
    }

    #[test]
    fn clearing_the_cookie_expires_it() {
        assert!(clear_cookie().contains("Max-Age=0"));
    }

    #[test]
    fn service_errors_keep_their_status_so_the_page_can_explain() {
        let err = AuthError::Service {
            status: 409,
            message: "taken".into(),
        };
        assert_eq!(err.status(), 409);
        assert_eq!(err.message(), "taken");
        assert_eq!(AuthError::NoSession.status(), 401);
        assert_eq!(AuthError::NotConfigured.status(), 503);
    }

    #[test]
    fn an_error_body_is_read_whatever_shape_it_takes() {
        assert_eq!(error_message(r#"{"detail":"nope"}"#, 400), "nope");
        assert_eq!(error_message(r#"{"message":"nope"}"#, 400), "nope");
        assert_eq!(error_message(r#"{"detail":{"message":"deep"}}"#, 400), "deep");
        // No body: fall back to something a person can act on.
        assert!(error_message("", 409).contains("already registered"));
        assert!(error_message("", 403).contains("Registration is closed"));
    }

    #[test]
    fn cookies_are_kept_per_session_not_shared() {
        // Two sessions must not be able to see each other's refresh cookie.
        let sessions = Sessions::new();
        let make = |token: &str, cookie: &str| Session {
            access_token: token.to_string(),
            access_expires_at: now_plus(900),
            cookies: vec![cookie.to_string()],
            user: User {
                id: token.to_string(),
                email: "a@b.c".into(),
                display_name: None,
                role: "user".into(),
            },
        };
        sessions.insert("one".into(), make("t1", "r=1"));
        sessions.insert("two".into(), make("t2", "r=2"));

        assert_eq!(sessions.get("one").unwrap().cookies, vec!["r=1".to_string()]);
        assert_eq!(sessions.get("two").unwrap().cookies, vec!["r=2".to_string()]);

        sessions.remove("one");
        assert!(sessions.get("one").is_none());
        assert!(sessions.get("two").is_some());
    }

    #[test]
    fn an_absolute_expiry_is_read_in_seconds_or_milliseconds() {
        let secs = expiry_from(&serde_json::json!({ "expiresAt": 2_000_000_000i64 }));
        let millis = expiry_from(&serde_json::json!({ "expiresAt": 2_000_000_000_000i64 }));
        assert_eq!(secs, millis, "both should mean the same instant");
    }

    #[test]
    fn a_missing_expiry_does_not_mean_forever() {
        let expiry = expiry_from(&serde_json::json!({}));
        assert!(expiry <= now_plus(15 * 60));
        assert!(expiry > SystemTime::now());
    }

    #[test]
    fn without_a_secret_accounts_are_off() {
        let config = AuthConfig {
            base_url: DEFAULT_BASE_URL.into(),
            project: DEFAULT_PROJECT.into(),
            secret: None,
        };
        assert!(!config.is_configured());
    }
}
