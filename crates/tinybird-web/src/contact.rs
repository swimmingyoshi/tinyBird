//! Client for the 0xstash contact API, behind the "get in touch" form.
//!
//! # Why this runs on the server
//!
//! The published integration has the browser post straight to
//! `contact.0xstash.dev` with the project's form key in an `Authorization`
//! header, and the service pins the request to a registered `Origin`. That fits
//! a hosted marketing site, where the key sits in one deployment the project
//! controls.
//!
//! tinyBird is not that. It is a server people run on their own machine, so the
//! page's origin is `http://127.0.0.1:8877` — an origin nobody can register —
//! and a key shipped to that page would be a key committed to this repository
//! and readable by everyone who ever opened the form.
//!
//! So the page posts here and this server does the talking:
//!
//! ```text
//! browser ──/api/contact──▶ this server ──bearer form key──▶ contact service
//! ```
//!
//! Same arrangement as [`crate::auth`] and [`crate::media`], for the same
//! reason: the credential stays on the machine that owns it.
//!
//! # Tickets
//!
//! A sent message becomes a ticket, and the service will show the conversation
//! back to the person who opened it. That side of the API authorises
//! differently: the bearer is the *sender's* auth access token, which the
//! service introspects, and the project key rides in a header beside it. So the
//! relay carries one more thing — the token out of [`crate::auth`]'s session —
//! and the browser still holds neither credential. See [`tickets`] and below.

use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default deployment of the contact API.
pub const DEFAULT_BASE_URL: &str = "https://contact.0xstash.dev";
/// Category used when the page does not pick one.
const DEFAULT_CATEGORY: &str = "support";
/// The service's own floor for a message body, checked here so the form can say
/// so without spending a round trip or a rate-limit slot.
pub const MIN_MESSAGE_LEN: usize = 10;
/// Longest message accepted. Not a service limit — a ceiling so a relayed
/// request cannot be used to push megabytes through this server's key.
pub const MAX_MESSAGE_LEN: usize = 4000;
/// Longest single-line field: name, email, subject.
const MAX_LINE_LEN: usize = 200;
/// Longest `siteUrl` passed through.
const MAX_URL_LEN: usize = 400;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Shortest gap between one sender's messages.
///
/// Catches the double-click and the second thoughts, which is most of what a
/// contact form actually receives twice.
const COOLDOWN: Duration = Duration::from_secs(60);
/// How far back the per-sender window looks.
const SENDER_WINDOW: Duration = Duration::from_secs(3600);
/// How many messages one sender may send in that window.
const SENDER_BURST: usize = 3;
/// How far back the whole-process ceiling looks.
const GLOBAL_WINDOW: Duration = Duration::from_secs(600);
/// How many messages that window may carry, whoever is sending.
///
/// A backstop under the per-sender limits rather than the main defence: it is
/// what stops a pile of fresh accounts from becoming a pile of fresh quotas.
const GLOBAL_BURST: usize = 30;

/// Where and how to reach the contact API.
#[derive(Clone, Debug)]
pub struct ContactConfig {
    pub base_url: String,
    /// `Origin` to present to the service.
    ///
    /// The service authorises a form key against the origins registered for its
    /// project. A request from this server has no origin of its own, so the
    /// operator names the one their key is registered under. Left unset,
    /// nothing is sent and the service decides — which is right for a key with
    /// no origin restriction, and produces a clear refusal for one that has.
    origin: Option<String>,
    /// The project's form key.
    ///
    /// Server-side only. It never appears in a response, a log line, or
    /// anything the browser can reach.
    key: Option<String>,
}

impl ContactConfig {
    /// Build from the environment.
    ///
    /// | variable | meaning | default |
    /// |---|---|---|
    /// | `TINYBIRD_CONTACT_KEY` | form key; the form is hidden without it | none |
    /// | `TINYBIRD_CONTACT_URL` | API base URL | `https://contact.0xstash.dev` |
    /// | `TINYBIRD_CONTACT_ORIGIN` | `Origin` the key is registered under | none |
    pub fn from_env() -> Self {
        let read = |name: &str| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };

        Self {
            base_url: read("TINYBIRD_CONTACT_URL")
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            origin: read("TINYBIRD_CONTACT_ORIGIN").map(|o| o.trim_end_matches('/').to_string()),
            key: read("TINYBIRD_CONTACT_KEY"),
        }
    }

    /// Whether a key is present. Without one the page does not show a form it
    /// cannot send.
    pub fn is_configured(&self) -> bool {
        self.key.is_some()
    }
}

/// Who is sending, when there is an account to say so.
///
/// Built from the session, never from the request body. A form that requires
/// signing in and then takes the sender's word for who they are has bought
/// nothing: the reply address is the part that matters, and letting the page
/// name it is how a signed-in stranger asks for somebody else's account to be
/// looked at. Same rule as `save_owner` and `lobby_identity` in `main.rs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The token's `sub` claim: what the throttle counts and what support gets
    /// told, because an address can change and this cannot.
    pub id: String,
    pub name: String,
    pub email: String,
}

/// A message that passed validation, in the shape the service asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub name: String,
    pub email: String,
    pub subject: String,
    pub body: String,
    pub category: String,
    pub site_url: Option<String>,
    /// The account it was sent from, when accounts are configured.
    pub account: Option<String>,
    /// That account's username, kept alongside the name they typed.
    ///
    /// Both, because they answer different questions: `name` is what to call
    /// them in a reply, and this is who the account is. Collapsing them would
    /// mean either losing the handle support searches by, or addressing
    /// somebody as `ash_1996`.
    pub username: Option<String>,
}

/// What arrived at `/api/contact`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// Something to send.
    Message(Box<Message>),
    /// The honeypot was filled in.
    ///
    /// Kept apart from an error on purpose: this is answered with the same
    /// "accepted" a person gets, because a bot told which field gave it away
    /// simply stops filling that field in.
    Spam,
}

/// Read the browser's JSON into a message, or say what is wrong with it.
///
/// `who` is the signed-in sender when accounts are configured. Their name and
/// address come from the session and the body's are ignored; without accounts
/// the server is one person's own machine and the body supplies both.
///
/// The wording of an error here is what the form shows, so each one names the
/// field and what it needs rather than reporting that validation failed.
pub fn parse(body: &serde_json::Value, who: Option<&Identity>) -> Result<Submission, String> {
    // The honeypot first: nothing else about a bot's submission is worth
    // checking, and a real person never sees the field to fill it in.
    if !field(body, "website").is_empty() {
        return Ok(Submission::Spam);
    }

    let (name, email) = match who {
        // The name is theirs to type, and the address is not. A display name
        // is a courtesy — what to call someone in a reply — and a username is
        // often not a name at all. The address is where the reply lands, so
        // that one comes from the session and nowhere else. Left blank, the
        // account's username stands in.
        Some(who) => {
            let typed = field(body, "name");
            let name = if typed.is_empty() {
                who.name.clone()
            } else {
                typed
            };
            (name, who.email.clone())
        }
        None => {
            let name = field(body, "name");
            if name.is_empty() {
                return Err("A name, please — it is who the reply is addressed to.".to_string());
            }
            let email = field(body, "email");
            if !looks_like_email(&email) {
                return Err("That email address does not look right.".to_string());
            }
            (name, email)
        }
    };
    let subject = field(body, "subject");
    if subject.is_empty() {
        return Err("A subject, please.".to_string());
    }
    let message = field(body, "message");
    let length = message.chars().count();
    if length < MIN_MESSAGE_LEN {
        return Err(format!(
            "The message needs at least {MIN_MESSAGE_LEN} characters, so there is something to answer."
        ));
    }
    if length > MAX_MESSAGE_LEN {
        return Err(format!(
            "That message is longer than the {MAX_MESSAGE_LEN} characters this form takes."
        ));
    }
    for (label, value) in [("name", &name), ("email", &email), ("subject", &subject)] {
        if value.chars().count() > MAX_LINE_LEN {
            return Err(format!("That {label} is too long."));
        }
    }

    Ok(Submission::Message(Box::new(Message {
        name,
        email,
        subject,
        body: message,
        category: category(body),
        site_url: site_url(body),
        account: who.map(|who| who.id.clone()),
        username: who.map(|who| who.name.clone()),
    })))
}

/// One trimmed string field, whatever the browser sent in its place.
fn field(body: &serde_json::Value, key: &str) -> String {
    body.get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Whether an address is worth sending on.
///
/// Deliberately shallow: the only address that can be fully validated is one
/// that has been mailed, and a stricter pattern here would reject real
/// addresses before the service ever saw them. This catches the typo — a
/// missing `@`, a stray space, a name with no domain — and nothing more.
fn looks_like_email(value: &str) -> bool {
    let mut parts = value.splitn(2, '@');
    let (Some(local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.len() > 2
        && !value.chars().any(char::is_whitespace)
}

/// The category the page picked, reduced to a plain slug.
///
/// The service does not publish its list of categories, so anything unexpected
/// falls back rather than being rejected: a message filed under the wrong
/// heading is better than one that never arrives.
fn category(body: &serde_json::Value) -> String {
    let raw = field(body, "category").to_ascii_lowercase();
    let usable = !raw.is_empty()
        && raw.len() <= 32
        && raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if usable {
        raw
    } else {
        DEFAULT_CATEGORY.to_string()
    }
}

/// The page this was sent from, when it is a web address at all.
///
/// Only useful to whoever reads the message, so anything that is not plainly a
/// URL is dropped rather than relayed.
fn site_url(body: &serde_json::Value) -> Option<String> {
    let url = field(body, "siteUrl");
    let usable = (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() <= MAX_URL_LEN
        && !url.chars().any(char::is_whitespace);
    usable.then_some(url)
}

/// Why a submission did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContactError {
    /// No form key, so the form is switched off.
    NotConfigured,
    /// Accounts are configured and nobody is signed in.
    NeedsAccount,
    /// This sender's last message was too recent.
    Cooldown { seconds: u64 },
    /// This sender has used their allowance for the hour.
    Quota { seconds: u64 },
    /// Too many messages from this server in too little time, from anyone.
    Throttled,
    /// The service refused this server's credentials.
    ///
    /// Apart from [`Self::Service`] because it is never the sender's fault: the
    /// key is missing, wrong, or not registered for the origin sent, and only
    /// the operator can fix it.
    Unauthorised,
    /// The sender's own access token was refused.
    ///
    /// Apart from [`Self::Unauthorised`] because the two belong to different
    /// people: that one is the operator's key, and this one is a session that
    /// has to be renewed or started over. It is what a single refresh-and-retry
    /// hangs off; see `ticket_call` in `main.rs`.
    TokenRejected,
    /// No such ticket on this account.
    ///
    /// Somebody else's ticket answers the same way, which is the point: a
    /// request id is not a capability, so nothing here distinguishes "gone"
    /// from "not yours".
    NotFound,
    /// The service rejected the message, in its own status and wording.
    Service { status: u16, message: String },
    /// Could not reach the service at all.
    Unreachable(String),
}

impl ContactError {
    /// The status to answer the browser with.
    pub fn status(&self) -> u16 {
        match self {
            Self::NotConfigured => 503,
            Self::NeedsAccount | Self::TokenRejected => 401,
            Self::NotFound => 404,
            Self::Cooldown { .. } | Self::Quota { .. } | Self::Throttled => 429,
            // The sender did nothing wrong, so these are reported as this
            // server failing to relay rather than as their message being bad.
            Self::Unauthorised | Self::Unreachable(_) => 502,
            Self::Service { status, .. } => *status,
        }
    }

    /// Seconds to wait, for a `Retry-After` header, when that is knowable.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Cooldown { seconds } | Self::Quota { seconds } => Some(*seconds),
            // The ceiling is about everyone at once, so when it clears depends
            // on what the rest of them do. Better to say nothing than to
            // promise a moment that may not be one.
            _ => None,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NotConfigured => {
                "The contact form is off. Set TINYBIRD_CONTACT_KEY in .env and restart.".to_string()
            }
            Self::NeedsAccount => "Sign in to send a message.".to_string(),
            Self::TokenRejected => {
                "That sign-in has expired. Sign in again to read your tickets.".to_string()
            }
            Self::NotFound => "No ticket by that name on this account.".to_string(),
            Self::Cooldown { seconds } => {
                format!(
                    "Just sent one — the next can go in {}.",
                    plain_wait(*seconds)
                )
            }
            Self::Quota { seconds } => format!(
                "That is {SENDER_BURST} messages this hour, which is the limit. \
                 The next one can go in {}.",
                plain_wait(*seconds)
            ),
            Self::Throttled => {
                "This server is sending a lot of messages at once. Try again in a few minutes."
                    .to_string()
            }
            Self::Unauthorised => concat!(
                "This server's contact key was refused. Check TINYBIRD_CONTACT_KEY, and ",
                "TINYBIRD_CONTACT_ORIGIN against the origins the key is registered for."
            )
            .to_string(),
            Self::Service { message, .. } => message.clone(),
            Self::Unreachable(why) => format!("Could not reach the contact service: {why}"),
        }
    }
}

/// A wait, in words a form can show without arithmetic.
///
/// "in 2400 seconds" is a number to be converted by whoever reads it; the point
/// of saying how long is that they should not have to.
fn plain_wait(seconds: u64) -> String {
    match seconds {
        0..=1 => "a moment".to_string(),
        2..=90 => format!("{seconds} seconds"),
        _ => {
            let minutes = seconds.div_ceil(60);
            format!("about {minutes} minutes")
        }
    }
}

/// What each sender has sent lately, and what the process has sent in total.
///
/// Counted per account rather than per address: with accounts configured the
/// sender is a `sub` claim they cannot vary, which is the thing that makes a
/// per-sender limit worth having at all. The address they would have typed is
/// not used for this, because it was never theirs to choose.
///
/// The process-wide ceiling stays underneath as a backstop. It is what a pile
/// of fresh accounts runs into, and what covers the no-accounts case, where
/// every message counts against the single `anonymous` sender.
#[derive(Debug)]
pub struct Throttle {
    senders: Mutex<HashMap<String, Vec<Instant>>>,
    everyone: Mutex<Vec<Instant>>,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    pub fn new() -> Self {
        Self {
            senders: Mutex::new(HashMap::new()),
            everyone: Mutex::new(Vec::new()),
        }
    }

    /// Take a slot for `who`, or say how long they have to wait and why.
    pub fn check(&self, who: &str) -> Result<(), ContactError> {
        self.check_at(who, Instant::now())
    }

    fn check_at(&self, who: &str, now: Instant) -> Result<(), ContactError> {
        let mut senders = self.senders.lock().unwrap_or_else(|err| err.into_inner());

        // Sweep the whole map, not just this sender's row: without it a server
        // that has been up for a month holds a row for everyone who ever used
        // the form, nearly all of them empty.
        senders.retain(|_, sent| {
            sent.retain(|at| now.duration_since(*at) < SENDER_WINDOW);
            !sent.is_empty()
        });

        let sent = senders.entry(who.to_string()).or_default();

        if let Some(last) = sent.last() {
            let since = now.duration_since(*last);
            if since < COOLDOWN {
                return Err(ContactError::Cooldown {
                    seconds: (COOLDOWN - since).as_secs() + 1,
                });
            }
        }
        if sent.len() >= SENDER_BURST {
            // The oldest is what has to age out before there is room again.
            let oldest = sent[0];
            return Err(ContactError::Quota {
                seconds: (SENDER_WINDOW - now.duration_since(oldest)).as_secs() + 1,
            });
        }

        // Checked before anything is recorded, so a message stopped by the
        // ceiling does not also spend the sender's own allowance.
        let mut everyone = self.everyone.lock().unwrap_or_else(|err| err.into_inner());
        everyone.retain(|at| now.duration_since(*at) < GLOBAL_WINDOW);
        if everyone.len() >= GLOBAL_BURST {
            return Err(ContactError::Throttled);
        }

        sent.push(now);
        everyone.push(now);
        Ok(())
    }
}

/// The JSON the service is sent.
///
/// Built here rather than passed through from the browser, so the page cannot
/// smuggle fields of its own into a request carrying this server's key.
fn payload(message: &Message) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": message.name,
        "email": message.email,
        "subject": message.subject,
        "message": message.body,
        "category": message.category,
        // Sent empty, as the contract asks: anything that arrived with it
        // filled in was dropped before reaching here.
        "website": "",
        "metadata": { "app": "tinybird", "version": env!("CARGO_PKG_VERSION") },
    });
    if let Some(url) = &message.site_url {
        body["siteUrl"] = serde_json::Value::String(url.clone());
    }
    // The account id rather than the address: it is what identifies the sender
    // across an address change, and it is what support can be asked about. The
    // username rides along beside it, so a typed name that differs from the
    // account's is visible as a preference rather than a discrepancy.
    if let Some(account) = &message.account {
        body["metadata"]["account"] = serde_json::Value::String(account.clone());
    }
    if let Some(username) = &message.username {
        body["metadata"]["username"] = serde_json::Value::String(username.clone());
    }
    body
}

/// Hand one message to the contact service.
///
/// `user_token` is the sender's auth access token when there is a session. It
/// is what binds the new ticket to their account: without it the service has
/// only the address in the body, which it will not take anyone's word for, and
/// the message arrives as a ticket the sender cannot then read back. The
/// address in the body is the session's own — see [`parse`] — which is the
/// match the service checks the header against.
///
/// Answers with the id the service filed the message under, when it gave one.
/// That is what lets the form offer the ticket rather than only mentioning that
/// there is one; see [`request_id`].
pub async fn submit(
    config: &ContactConfig,
    message: &Message,
    user_token: Option<&str>,
) -> Result<Option<String>, ContactError> {
    let key = config.key.as_deref().ok_or(ContactError::NotConfigured)?;

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| ContactError::Unreachable(err.to_string()))?;

    let mut request = client
        .post(format!("{}/api/requests", config.base_url))
        .bearer_auth(key)
        .json(&payload(message));
    if let Some(token) = user_token {
        request = request.header(USER_TOKEN_HEADER, token);
    }
    if let Some(origin) = &config.origin {
        request = request.header(reqwest::header::ORIGIN, origin);
    }

    let response = request
        .send()
        .await
        .map_err(|err| ContactError::Unreachable(err.to_string()))?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    // The contract answers 202, but any 2xx means the service took it.
    if (200..300).contains(&status) {
        return Ok(request_id(&body));
    }

    Err(match status {
        401 | 403 => ContactError::Unauthorised,
        // What the sender could act on keeps the service's own words. A 404 is
        // this server pointed at the wrong URL, so it is not shown as theirs.
        400 | 409 | 413 | 422 | 429 => ContactError::Service {
            status,
            message: error_message(&body, status),
        },
        _ => ContactError::Unreachable(format!("the service answered {status}")),
    })
}

/// The id the service filed a message under, out of its 202.
///
/// Named half a dozen ways across the contract's own examples and sometimes one
/// level down, so this looks the way the ticket pages read: the first name that
/// is actually there, and nothing insisted upon.
///
/// Checked with [`is_request_id`] before it is handed on, because the page will
/// put it in a URL. A service that starts answering with something else costs
/// the sender a link, not a broken page.
fn request_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let holders = [
        Some(&value),
        value.get("request"),
        value.get("ticket"),
        value.get("data"),
    ];

    for holder in holders.into_iter().flatten() {
        for name in ["id", "requestId", "request_id", "ticketId"] {
            let found = holder
                .get(name)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if is_request_id(found) {
                return Some(found.to_string());
            }
        }
    }
    None
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
    format!("The contact service answered {status}.")
}

// --------------------------------------------------------------------- tickets
//
// What a sent message became, read back. The service keeps the conversation —
// the original, whatever support replied, whatever the sender said next — and
// these are the four calls that show it.
//
// The credentials are the other way round from `submit`: there the bearer is
// this server's form key and the sender rides in a header, here the bearer is
// the *sender's* access token and the form key rides in a header. That is not
// an inconsistency in the API, it is the security model — a ticket read is
// authorised by whoever the token introspects to, and the project key only says
// which project's tickets are being asked about. A request id authorises
// nothing, so a guessed one returns the same nothing a wrong one does.

/// Header carrying the sender's identity on a form submission.
const USER_TOKEN_HEADER: &str = "X-0xstash-User-Token";
/// Header carrying the project on a ticket call.
const PROJECT_KEY_HEADER: &str = "X-Contact-Project-Key";

/// How many tickets or messages to ask for when the page does not say.
pub const TICKET_PAGE: usize = 50;
/// Most that may be asked for at once, whatever the page asks for.
///
/// The page reads a list and a conversation, neither of which is long. A
/// ceiling here keeps a crafted query from turning one request through this
/// server into a very large one at the service.
pub const MAX_TICKET_PAGE: usize = 100;
/// Longest reply the service takes, in characters.
pub const MAX_REPLY_LEN: usize = 10_000;
/// Longest idempotency key accepted from the page.
const MAX_KEY_LEN: usize = 100;

/// Whether a request id is shaped like one.
///
/// Checked before it is pasted into a URL, so a path of dots or a slash cannot
/// turn a ticket read into a request for some other route on the service.
pub fn is_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A page size the service will accept, whatever the browser asked for.
fn page_size(asked: Option<usize>) -> usize {
    asked.unwrap_or(TICKET_PAGE).clamp(1, MAX_TICKET_PAGE)
}

/// Read a reply out of the browser's JSON: what to say, and the key that stops
/// it being said twice.
///
/// The key comes from the page rather than being minted here, and that is the
/// point of it. A composer that retries after a timeout sends the same key, so
/// the service files one message however many times the request arrives; a key
/// minted per request would make every retry a new message, which is the
/// failure this is meant to prevent.
pub fn parse_reply(body: &serde_json::Value) -> Result<(String, String), String> {
    let message = field(body, "message");
    let length = message.chars().count();
    if length == 0 {
        return Err("Nothing to send.".to_string());
    }
    if length > MAX_REPLY_LEN {
        return Err(format!(
            "That reply is longer than the {MAX_REPLY_LEN} characters a ticket takes."
        ));
    }

    let key = field(body, "idempotencyKey");
    let usable = (8..=MAX_KEY_LEN).contains(&key.len())
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !usable {
        return Err("That reply arrived without a usable idempotency key.".to_string());
    }

    Ok((message, key))
}

/// Start a ticket-API request, with both credentials on it.
fn ticket_request(
    config: &ContactConfig,
    token: &str,
    method: reqwest::Method,
    path: &str,
) -> Result<reqwest::RequestBuilder, ContactError> {
    let key = config.key.as_deref().ok_or(ContactError::NotConfigured)?;

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| ContactError::Unreachable(err.to_string()))?;

    let mut request = client
        .request(method, format!("{}{path}", config.base_url))
        .bearer_auth(token)
        .header(PROJECT_KEY_HEADER, key);
    if let Some(origin) = &config.origin {
        request = request.header(reqwest::header::ORIGIN, origin);
    }
    Ok(request)
}

/// Send one ticket-API request and read the JSON back.
///
/// The service's own body is returned whole rather than reshaped. It owns the
/// schema, it has not published one, and a struct here would quietly drop every
/// field it did not know about — including the ones a later version adds. The
/// page reads it defensively for the same reason.
async fn ticket_json(request: reqwest::RequestBuilder) -> Result<serde_json::Value, ContactError> {
    let response = request
        .send()
        .await
        .map_err(|err| ContactError::Unreachable(err.to_string()))?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    if (200..300).contains(&status) {
        return serde_json::from_str(&body)
            .map_err(|err| ContactError::Unreachable(format!("unreadable answer: {err}")));
    }

    Err(match status {
        // The sender's token, not this server's key, so this is the one status
        // worth a second attempt. Kept apart for that reason alone.
        401 => ContactError::TokenRejected,
        403 => ContactError::Unauthorised,
        404 => ContactError::NotFound,
        400 | 409 | 413 | 422 | 429 => ContactError::Service {
            status,
            message: error_message(&body, status),
        },
        _ => ContactError::Unreachable(format!("the service answered {status}")),
    })
}

/// One page of the sender's tickets, in whatever order the service returns.
pub async fn tickets(
    config: &ContactConfig,
    token: &str,
    limit: Option<usize>,
    offset: usize,
) -> Result<serde_json::Value, ContactError> {
    let request = ticket_request(config, token, reqwest::Method::GET, "/api/v1/tickets")?
        .query(&[("limit", page_size(limit)), ("offset", offset)]);
    ticket_json(request).await
}

/// One ticket.
pub async fn ticket(
    config: &ContactConfig,
    token: &str,
    id: &str,
) -> Result<serde_json::Value, ContactError> {
    if !is_request_id(id) {
        return Err(ContactError::NotFound);
    }
    let request = ticket_request(
        config,
        token,
        reqwest::Method::GET,
        &format!("/api/v1/tickets/{id}"),
    )?;
    ticket_json(request).await
}

/// One ticket's conversation.
pub async fn ticket_messages(
    config: &ContactConfig,
    token: &str,
    id: &str,
    limit: Option<usize>,
    offset: usize,
) -> Result<serde_json::Value, ContactError> {
    if !is_request_id(id) {
        return Err(ContactError::NotFound);
    }
    let request = ticket_request(
        config,
        token,
        reqwest::Method::GET,
        &format!("/api/v1/tickets/{id}/messages"),
    )?
    .query(&[("limit", page_size(limit)), ("offset", offset)]);
    ticket_json(request).await
}

/// Add to one ticket's conversation — once, however many times this is called
/// with the same `key`.
pub async fn ticket_reply(
    config: &ContactConfig,
    token: &str,
    id: &str,
    message: &str,
    key: &str,
) -> Result<serde_json::Value, ContactError> {
    if !is_request_id(id) {
        return Err(ContactError::NotFound);
    }
    let request = ticket_request(
        config,
        token,
        reqwest::Method::POST,
        &format!("/api/v1/tickets/{id}/messages"),
    )?
    .header("Idempotency-Key", key)
    .json(&serde_json::json!({ "message": message }));
    ticket_json(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(message: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "Ash",
            "email": "ash@pallet.town",
            "subject": "Sprites",
            "message": message,
            "website": "",
        })
    }

    /// The no-accounts case, which is what most of these exercise: the body
    /// supplies the name and the address.
    fn parsed(value: &serde_json::Value) -> Result<Submission, String> {
        parse(value, None)
    }

    fn message_of(value: &serde_json::Value) -> Message {
        match parsed(value).expect("should have passed validation") {
            Submission::Message(message) => *message,
            Submission::Spam => panic!("treated as spam"),
        }
    }

    fn ash() -> Identity {
        Identity {
            id: "user_01".to_string(),
            name: "Ash".to_string(),
            email: "ash@pallet.town".to_string(),
        }
    }

    fn message_as(value: &serde_json::Value, who: &Identity) -> Message {
        match parse(value, Some(who)).expect("should have passed validation") {
            Submission::Message(message) => *message,
            Submission::Spam => panic!("treated as spam"),
        }
    }

    #[test]
    fn a_signed_in_sender_cannot_change_where_the_reply_goes() {
        let mut value = form("a message long enough to send");
        value["email"] = serde_json::json!("rockets@example.test");

        // The point of requiring an account: the reply goes where the session
        // says, not where the page asked it to.
        let message = message_as(&value, &ash());
        assert_eq!(message.email, "ash@pallet.town");
        assert_eq!(message.account.as_deref(), Some("user_01"));
    }

    #[test]
    fn a_signed_in_sender_may_still_say_what_to_call_them() {
        let mut value = form("a message long enough to send");
        value["name"] = serde_json::json!("Ash Ketchum");

        // A name is a courtesy, not an identity claim — unlike the address,
        // typing it costs nobody anything.
        let message = message_as(&value, &ash());
        assert_eq!(message.name, "Ash Ketchum");
        // The account's own handle is kept beside it rather than replaced.
        assert_eq!(message.username.as_deref(), Some("Ash"));
    }

    #[test]
    fn an_unfilled_name_falls_back_to_the_username() {
        let mut value = form("a message long enough to send");
        value["name"] = serde_json::json!("   ");
        assert_eq!(message_as(&value, &ash()).name, "Ash");
    }

    #[test]
    fn a_signed_in_sender_need_not_retype_what_the_session_knows() {
        // The form does not show the address when signed in, so it arrives
        // absent rather than empty — and that must not read as a validation
        // failure.
        let value = serde_json::json!({
            "subject": "Sprites",
            "message": "a message long enough to send",
            "website": "",
        });
        let message = message_as(&value, &ash());
        assert_eq!(message.email, "ash@pallet.town");
        assert_eq!(message.name, "Ash");
    }

    #[test]
    fn a_typed_name_is_still_held_to_the_line_length() {
        let mut value = form("a message long enough to send");
        value["name"] = serde_json::json!("x".repeat(MAX_LINE_LEN + 1));
        assert!(parse(&value, Some(&ash())).is_err());
    }

    #[test]
    fn the_account_reaches_support_but_the_honeypot_still_wins() {
        let mut value = form("a message long enough to send");
        let body = payload(&message_as(&value, &ash()));
        assert_eq!(body["metadata"]["account"], "user_01");
        assert_eq!(body["metadata"]["username"], "Ash");

        // Signed in or not, a filled trap is a filled trap.
        value["website"] = serde_json::json!("http://spam.example");
        assert_eq!(parse(&value, Some(&ash())), Ok(Submission::Spam));
    }

    #[test]
    fn without_accounts_nothing_is_attributed() {
        let body = payload(&message_of(&form("a message long enough to send")));
        assert!(body["metadata"].get("account").is_none());
        assert!(body["metadata"].get("username").is_none());
    }

    #[test]
    fn accepts_a_filled_in_form() {
        let message = message_of(&form("FireRed draws no sprites without a BIOS."));
        assert_eq!(message.name, "Ash");
        assert_eq!(message.email, "ash@pallet.town");
        assert_eq!(message.category, "support");
        assert_eq!(message.site_url, None);
    }

    #[test]
    fn trims_before_judging_a_field() {
        let mut value = form("  a message long enough to send  ");
        value["name"] = serde_json::json!("  Ash  ");
        let message = message_of(&value);
        assert_eq!(message.name, "Ash");
        assert_eq!(message.body, "a message long enough to send");

        // Whitespace is not a name, and is not a message either.
        value["name"] = serde_json::json!("   ");
        assert!(parsed(&value).is_err());
    }

    #[test]
    fn holds_the_ten_character_floor() {
        assert!(parsed(&form("too short")).is_err());
        assert!(parsed(&form("ten chars.")).is_ok());
    }

    #[test]
    fn refuses_a_message_past_the_ceiling() {
        assert!(parsed(&form(&"x".repeat(MAX_MESSAGE_LEN + 1))).is_err());
        assert!(parsed(&form(&"x".repeat(MAX_MESSAGE_LEN))).is_ok());
    }

    #[test]
    fn counts_characters_rather_than_bytes() {
        // Six of these is eighteen bytes: a byte count would wave it through.
        assert!(parsed(&form("ながいメッセ")).is_err());
        assert!(parsed(&form("ながいメッセージです")).is_ok());
    }

    #[test]
    fn catches_the_addresses_a_typo_makes() {
        for bad in ["ash", "ash@", "@pallet.town", "ash@town", "a s@h.com", ""] {
            let mut value = form("a message long enough to send");
            value["email"] = serde_json::json!(bad);
            assert!(parsed(&value).is_err(), "{bad} should not have passed");
        }
        let mut value = form("a message long enough to send");
        value["email"] = serde_json::json!("ash+oak@pallet.town.co.uk");
        assert!(parsed(&value).is_ok());
    }

    #[test]
    fn a_filled_honeypot_is_dropped_rather_than_refused() {
        let mut value = form("a message long enough to send");
        value["website"] = serde_json::json!("http://spam.example");
        assert_eq!(parsed(&value), Ok(Submission::Spam));

        // Even when the rest of it is nonsense: the answer must not differ
        // from the one a real message gets, or it teaches the sender which
        // field to leave alone next time.
        value["email"] = serde_json::json!("not-an-address");
        assert_eq!(parsed(&value), Ok(Submission::Spam));
    }

    #[test]
    fn falls_back_rather_than_refusing_an_odd_category() {
        let mut value = form("a message long enough to send");
        for (given, expected) in [
            ("bug", "bug"),
            ("Feature-Request", "feature-request"),
            ("", "support"),
            ("../etc", "support"),
            (
                "a really long category name that nobody has registered",
                "support",
            ),
        ] {
            value["category"] = serde_json::json!(given);
            assert_eq!(message_of(&value).category, expected, "{given}");
        }
    }

    #[test]
    fn passes_on_only_a_plausible_site_url() {
        let mut value = form("a message long enough to send");
        value["siteUrl"] = serde_json::json!("http://127.0.0.1:8877/");
        assert_eq!(
            message_of(&value).site_url.as_deref(),
            Some("http://127.0.0.1:8877/")
        );

        value["siteUrl"] = serde_json::json!("javascript:alert(1)");
        assert_eq!(message_of(&value).site_url, None);
    }

    #[test]
    fn builds_the_body_the_contract_asks_for() {
        let mut value = form("a message long enough to send");
        value["siteUrl"] = serde_json::json!("https://example.test/play");
        let body = payload(&message_of(&value));

        assert_eq!(body["name"], "Ash");
        assert_eq!(body["email"], "ash@pallet.town");
        assert_eq!(body["subject"], "Sprites");
        assert_eq!(body["message"], "a message long enough to send");
        assert_eq!(body["category"], "support");
        assert_eq!(body["website"], "");
        assert_eq!(body["siteUrl"], "https://example.test/play");
        assert_eq!(body["metadata"]["app"], "tinybird");
    }

    #[test]
    fn leaves_out_a_site_url_it_does_not_have() {
        let body = payload(&message_of(&form("a message long enough to send")));
        assert!(body.get("siteUrl").is_none());
    }

    /// One sender's messages, spaced far enough apart to clear the cooldown.
    fn spaced(throttle: &Throttle, who: &str, start: Instant, count: usize) {
        for step in 0..count {
            let at = start + COOLDOWN * (step as u32);
            assert!(throttle.check_at(who, at).is_ok(), "message {step}");
        }
    }

    #[test]
    fn a_second_message_has_to_wait_out_the_cooldown() {
        let throttle = Throttle::new();
        let start = Instant::now();
        assert!(throttle.check_at("ash", start).is_ok());

        match throttle.check_at("ash", start + Duration::from_secs(10)) {
            Err(ContactError::Cooldown { seconds }) => assert_eq!(seconds, 51),
            other => panic!("expected a cooldown, got {other:?}"),
        }
        assert!(throttle.check_at("ash", start + COOLDOWN).is_ok());
    }

    #[test]
    fn a_sender_gets_three_an_hour() {
        let throttle = Throttle::new();
        let start = Instant::now();
        spaced(&throttle, "ash", start, SENDER_BURST);

        let fourth = start + COOLDOWN * (SENDER_BURST as u32);
        match throttle.check_at("ash", fourth) {
            Err(ContactError::Quota { seconds }) => {
                // The wait is until the *first* one ages out, not the last.
                let expected = (SENDER_WINDOW - (fourth - start)).as_secs() + 1;
                assert_eq!(seconds, expected);
            }
            other => panic!("expected a quota refusal, got {other:?}"),
        }
        assert!(throttle.check_at("ash", start + SENDER_WINDOW).is_ok());
    }

    #[test]
    fn one_sender_running_out_does_not_stop_another() {
        let throttle = Throttle::new();
        let start = Instant::now();
        spaced(&throttle, "ash", start, SENDER_BURST);

        let now = start + COOLDOWN * (SENDER_BURST as u32);
        assert!(throttle.check_at("ash", now).is_err());
        // A limit that one person can exhaust for everybody is a limit that
        // hands them the form as a weapon.
        assert!(throttle.check_at("misty", now).is_ok());
    }

    #[test]
    fn the_ceiling_holds_whoever_is_sending() {
        let throttle = Throttle::new();
        let start = Instant::now();
        // A fresh sender each time, so nobody meets their own limit first.
        for n in 0..GLOBAL_BURST {
            assert!(throttle.check_at(&format!("sender-{n}"), start).is_ok());
        }
        assert_eq!(
            throttle.check_at("one-more", start),
            Err(ContactError::Throttled)
        );
    }

    #[test]
    fn the_ceiling_does_not_spend_the_sender_allowance_it_refused() {
        let throttle = Throttle::new();
        let start = Instant::now();
        for n in 0..GLOBAL_BURST {
            assert!(throttle.check_at(&format!("sender-{n}"), start).is_ok());
        }
        assert!(throttle.check_at("ash", start).is_err());

        // Once the ceiling clears, Ash still has the full three: the refused
        // attempt was never recorded against them.
        let after = start + GLOBAL_WINDOW;
        spaced(&throttle, "ash", after, SENDER_BURST);
    }

    #[test]
    fn senders_who_went_quiet_are_forgotten() {
        let throttle = Throttle::new();
        let start = Instant::now();
        assert!(throttle.check_at("ash", start).is_ok());
        assert!(throttle.check_at("misty", start + SENDER_WINDOW).is_ok());

        // Ash's row aged out rather than sitting in the map for the life of
        // the process.
        let senders = throttle.senders.lock().unwrap();
        assert!(!senders.contains_key("ash"));
        assert!(senders.contains_key("misty"));
    }

    #[test]
    fn a_wait_is_given_in_words_not_arithmetic() {
        assert_eq!(plain_wait(0), "a moment");
        assert_eq!(plain_wait(45), "45 seconds");
        assert_eq!(plain_wait(2400), "about 40 minutes");
    }

    #[test]
    fn only_a_wait_that_is_known_gets_a_retry_after() {
        assert_eq!(
            ContactError::Cooldown { seconds: 30 }.retry_after(),
            Some(30)
        );
        assert_eq!(
            ContactError::Quota { seconds: 900 }.retry_after(),
            Some(900)
        );
        // When it clears depends on everyone else, so nothing is promised.
        assert_eq!(ContactError::Throttled.retry_after(), None);
        assert_eq!(ContactError::NeedsAccount.retry_after(), None);
    }

    #[test]
    fn signing_in_is_the_price_of_the_form() {
        assert_eq!(ContactError::NeedsAccount.status(), 401);
    }

    #[test]
    fn a_refused_key_is_reported_as_this_server_failing() {
        let err = ContactError::Unauthorised;
        assert_eq!(err.status(), 502);
        assert!(err.message().contains("TINYBIRD_CONTACT_KEY"));
    }

    #[test]
    fn a_service_refusal_keeps_its_own_status() {
        let err = ContactError::Service {
            status: 429,
            message: "slow down".to_string(),
        };
        assert_eq!(err.status(), 429);
        assert_eq!(err.message(), "slow down");
    }

    #[test]
    fn reads_a_service_error_whatever_shape_it_is() {
        assert_eq!(error_message(r#"{"detail":"nope"}"#, 400), "nope");
        assert_eq!(
            error_message(r#"{"error":{"message":"nope"}}"#, 400),
            "nope"
        );
        assert_eq!(
            error_message("<html>502</html>", 502),
            "The contact service answered 502."
        );
    }

    #[test]
    fn no_key_means_no_form() {
        let config = ContactConfig {
            base_url: DEFAULT_BASE_URL.to_string(),
            origin: None,
            key: None,
        };
        assert!(!config.is_configured());
    }
    #[test]
    fn the_id_a_message_was_filed_under_is_read_out_of_the_202() {
        for body in [
            r#"{"id":"req_01HZY4"}"#,
            r#"{"requestId":"req_01HZY4"}"#,
            r#"{"request":{"id":"req_01HZY4"}}"#,
            r#"{"data":{"ticketId":"req_01HZY4"}}"#,
        ] {
            assert_eq!(request_id(body).as_deref(), Some("req_01HZY4"), "{body}");
        }
    }

    /// The link is a convenience; a message that arrived is what matters. So
    /// anything unreadable costs the link and nothing else.
    #[test]
    fn an_answer_with_no_usable_id_simply_has_none() {
        assert_eq!(request_id("{}"), None);
        assert_eq!(request_id("not json at all"), None);
        assert_eq!(request_id(r#"{"id":""}"#), None);
        // Would be pasted into a URL, so it is not an id however it is labelled.
        assert_eq!(request_id(r#"{"id":"../admin/requests"}"#), None);
    }

    // ------------------------------------------------------------- tickets

    #[test]
    fn a_request_id_may_not_carry_a_path() {
        assert!(is_request_id("req_01HZY4"));
        assert!(is_request_id("9f3c-4a1b"));

        // The reason this check exists: an id is pasted into a URL, and these
        // would each address something other than a ticket.
        assert!(!is_request_id("../admin/requests"));
        assert!(!is_request_id("req/01"));
        assert!(!is_request_id("req 01"));
        assert!(!is_request_id(""));
        assert!(!is_request_id(&"a".repeat(65)));
    }

    #[test]
    fn a_page_is_clamped_to_something_the_service_will_answer() {
        assert_eq!(page_size(None), TICKET_PAGE);
        assert_eq!(page_size(Some(10)), 10);
        assert_eq!(page_size(Some(0)), 1);
        assert_eq!(page_size(Some(100_000)), MAX_TICKET_PAGE);
    }

    fn reply(message: &str, key: &str) -> serde_json::Value {
        serde_json::json!({ "message": message, "idempotencyKey": key })
    }

    #[test]
    fn a_reply_needs_words_and_a_key() {
        let (message, key) = parse_reply(&reply("still broken", "7a1f2c9d-4e5b-6a7c-8d9e"))
            .expect("should have passed validation");
        assert_eq!(message, "still broken");
        assert_eq!(key, "7a1f2c9d-4e5b-6a7c-8d9e");
    }

    #[test]
    fn an_empty_reply_is_not_sent() {
        assert!(parse_reply(&reply("   ", "7a1f2c9d-4e5b")).is_err());
    }

    #[test]
    fn a_reply_longer_than_the_service_takes_is_refused_here() {
        let long = "x".repeat(MAX_REPLY_LEN + 1);
        assert!(parse_reply(&reply(&long, "7a1f2c9d-4e5b")).is_err());
    }

    /// Without a key a retry would file a second message, which is the whole
    /// thing the key is there to prevent — so a reply without one is refused
    /// rather than sent with one invented here.
    #[test]
    fn a_reply_without_a_usable_key_is_refused() {
        assert!(parse_reply(&serde_json::json!({ "message": "hello there" })).is_err());
        assert!(parse_reply(&reply("hello there", "short")).is_err());
        assert!(parse_reply(&reply("hello there", "has spaces in it")).is_err());
        assert!(parse_reply(&reply("hello there", &"k".repeat(MAX_KEY_LEN + 1))).is_err());
    }

    /// The two ticket refusals the page acts on differently: one asks for a
    /// fresh token, the other is an answer.
    #[test]
    fn a_refused_token_and_a_missing_ticket_answer_differently() {
        assert_eq!(ContactError::TokenRejected.status(), 401);
        assert_eq!(ContactError::NotFound.status(), 404);
        // Neither promises a moment to try again; only the throttles know one.
        assert_eq!(ContactError::TokenRejected.retry_after(), None);
    }
}
