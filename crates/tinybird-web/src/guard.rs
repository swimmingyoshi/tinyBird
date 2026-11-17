//! What this server accepts, and what it says about every response.
//!
//! The server is for the person running it: it binds to loopback and nothing
//! else, and it serves that person's ROM folder and BIOS. Binding to loopback
//! is not enough on its own, because any web page the same person has open can
//! make their browser send requests to `localhost`. So every request also has
//! to name a loopback host (which stops DNS rebinding), and anything that
//! changes state or opens a WebSocket has to come from this server's own pages.
use axum::{
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::IpAddr;

/// Refuse to listen anywhere but this machine.
pub fn check_bind(host: IpAddr) -> Result<(), String> {
    if host.is_loopback() {
        Ok(())
    } else {
        Err(format!(
            "tinybird-web only serves this machine; {host} is not a loopback address.              It serves your ROM folder and BIOS, which must not be reachable from a network."
        ))
    }
}

fn permits(request: &Request) -> bool {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let Ok(url) = reqwest::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    let loopback = url.host_str().is_some_and(|name| {
        name == "localhost"
            || name
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !loopback || url.path() != "/" || url.username() != "" || url.password().is_some() {
        return false;
    }
    let expected = url.origin().ascii_serialization();
    // Browsers must originate mutations and WebSocket handshakes here.
    // Requests with no Origin are accepted only for ordinary reads.
    let mutation = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let websocket = headers.contains_key(header::UPGRADE);
    match headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) {
        Some(origin) => origin == expected,
        None => {
            !mutation
                && !websocket
                && headers
                    .get("sec-fetch-site")
                    .is_none_or(|v| v != "cross-site")
        }
    }
}

pub async fn guard(request: Request, next: Next) -> Response {
    if !permits(&request) {
        return (StatusCode::FORBIDDEN, "Request origin is not allowed.").into_response();
    }
    let revalidating = matches!(*request.method(), Method::GET | Method::HEAD)
        .then(|| request.headers().get(header::IF_NONE_MATCH).cloned())
        .flatten();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("frame-ancestors 'self'; object-src 'none'; base-uri 'self'"),
    );
    cache_policy(response, revalidating.as_ref())
}

/// Decide what a browser may keep, last, after every handler has had its say.
///
/// **Nothing is cached unless a handler asked for it.** A response that says
/// nothing about caching gets `no-store`, so a new route is private until
/// someone decides otherwise. A handler that did set `Cache-Control` is left
/// alone: this used to overwrite every response with `no-store`, which threw
/// away the `immutable` on sprites and made the browser download the emulator
/// module and a couple of hundred kilobytes of script again on every page.
///
/// A response carrying an `ETag` the browser already holds is answered with an
/// empty 304. Static assets are served `no-cache` with one, so the browser
/// still asks every time — a rebuilt server is picked up at once — but only
/// pays for the bytes when they have changed.
fn cache_policy(mut response: Response, if_none_match: Option<&HeaderValue>) -> Response {
    let headers = response.headers_mut();
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    let unchanged = response.status() == StatusCode::OK
        && etag_matches(if_none_match, response.headers().get(header::ETAG));
    if !unchanged {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    parts.status = StatusCode::NOT_MODIFIED;
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::CONTENT_TYPE);
    Response::from_parts(parts, axum::body::Body::empty())
}

/// Whether `If-None-Match` names this entity tag. The header may list several
/// and may mark them weak; for a GET the weak comparison is the right one.
fn etag_matches(if_none_match: Option<&HeaderValue>, etag: Option<&HeaderValue>) -> bool {
    let (Some(wanted), Some(etag)) = (if_none_match, etag) else {
        return false;
    };
    let (Ok(wanted), Ok(etag)) = (wanted.to_str(), etag.to_str()) else {
        return false;
    };
    let strong = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    wanted.split(',').any(|tag| strong(tag) == strong(etag))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(method: Method, host: &str, origin: Option<&str>, upgrade: bool) -> Request {
        let mut builder = Request::builder().method(method).header("host", host);
        if let Some(origin) = origin {
            builder = builder.header("origin", origin);
        }
        if upgrade {
            builder = builder.header("upgrade", "websocket");
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }
    #[test]
    fn rejects_dns_rebinding_and_cross_origin_writes() {
        assert!(permits(&request(
            Method::GET,
            "localhost:8877",
            None,
            false
        )));
        assert!(!permits(&request(
            Method::GET,
            "evil.example:8877",
            None,
            false
        )));
        assert!(!permits(&request(
            Method::POST,
            "localhost:8877",
            None,
            false
        )));
        assert!(!permits(&request(
            Method::POST,
            "localhost:8877",
            Some("https://evil.example"),
            false
        )));
        assert!(permits(&request(
            Method::POST,
            "localhost:8877",
            Some("http://localhost:8877"),
            false
        )));
        assert!(!permits(&request(
            Method::GET,
            "localhost:8877",
            None,
            true
        )));
        assert!(!permits(&request(
            Method::GET,
            "localhost:8877",
            Some("http://localhost:8878"),
            true
        )));
    }
    const TAG: &str = "\"abc\"";
    fn answer(cache_control: Option<&'static str>, etag: Option<&'static str>) -> Response {
        let mut response = (StatusCode::OK, "body").into_response();
        if let Some(value) = cache_control {
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
        }
        if let Some(value) = etag {
            response
                .headers_mut()
                .insert(header::ETAG, HeaderValue::from_static(value));
        }
        response
    }
    fn cache_control(response: &Response) -> &str {
        response.headers()[header::CACHE_CONTROL].to_str().unwrap()
    }
    #[test]
    fn a_response_that_says_nothing_about_caching_is_never_stored() {
        let response = cache_policy(answer(None, None), None);
        assert_eq!(cache_control(&response), "no-store");
    }
    #[test]
    fn a_handlers_own_cache_policy_survives() {
        let immutable = "public, max-age=31536000, immutable";
        for policy in [immutable, "no-cache", "private, no-store"] {
            let response = cache_policy(answer(Some(policy), None), None);
            assert_eq!(cache_control(&response), policy);
        }
    }
    #[test]
    fn an_asset_the_browser_already_holds_is_not_sent_again() {
        let held = HeaderValue::from_static(TAG);
        let response = cache_policy(answer(Some("no-cache"), Some(TAG)), Some(&held));
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(cache_control(&response), "no-cache");
        assert_eq!(response.headers()[header::ETAG], TAG);
        assert!(!response.headers().contains_key(header::CONTENT_TYPE));

        let listed = HeaderValue::from_static("\"old\", W/\"abc\"");
        let response = cache_policy(answer(Some("no-cache"), Some(TAG)), Some(&listed));
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }
    #[test]
    fn a_changed_or_untagged_asset_is_sent_in_full() {
        let stale = HeaderValue::from_static("\"old\"");
        let changed = cache_policy(answer(Some("no-cache"), Some(TAG)), Some(&stale));
        assert_eq!(changed.status(), StatusCode::OK);
        let untagged = cache_policy(answer(None, None), Some(&stale));
        assert_eq!(untagged.status(), StatusCode::OK);

        let mut missing = answer(Some("no-cache"), Some(TAG));
        *missing.status_mut() = StatusCode::NOT_FOUND;
        let held = HeaderValue::from_static(TAG);
        assert_eq!(
            cache_policy(missing, Some(&held)).status(),
            StatusCode::NOT_FOUND
        );
    }
    #[test]
    fn refuses_to_listen_beyond_this_machine() {
        assert!(check_bind("127.0.0.1".parse().unwrap()).is_ok());
        assert!(check_bind("::1".parse().unwrap()).is_ok());
        assert!(check_bind("0.0.0.0".parse().unwrap()).is_err());
        assert!(check_bind("192.168.1.10".parse().unwrap()).is_err());
    }
}
