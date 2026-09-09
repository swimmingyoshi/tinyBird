//! Explicit deployment boundaries; UI visibility is never authorization.
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::{env, net::IpAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Local,
    Development,
    Production,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "local" => Ok(Self::Local),
            "development" => Ok(Self::Development),
            "production" => Ok(Self::Production),
            _ => Err("Mode must be local, development, or production.".into()),
        }
    }
    pub fn from_args(args: &[String]) -> Result<Self, String> {
        if let Some(index) = args.iter().position(|arg| arg == "--mode") {
            return Self::parse(args.get(index + 1).ok_or("--mode requires a value")?);
        }
        Self::parse(&env::var("TINYBIRD_MODE").unwrap_or_else(|_| "local".into()))
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Development => "development",
            Self::Production => "production",
        }
    }
}

#[derive(Clone)]
pub struct Deployment {
    pub mode: Mode,
    pub origin: Option<String>,
}
impl Deployment {
    pub fn new(mode: Mode, host: IpAddr, origin: Option<String>) -> Result<Self, String> {
        if mode != Mode::Production && !host.is_loopback() {
            return Err("Local and development modes must bind to loopback. Use production mode behind HTTPS for public hosting.".into());
        }
        let origin = origin
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_owned());
        if mode == Mode::Production {
            let url = origin
                .as_ref()
                .and_then(|value| reqwest::Url::parse(value).ok())
                .ok_or("Production requires TINYBIRD_PUBLIC_ORIGIN=https://your-host")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("TINYBIRD_PUBLIC_ORIGIN must be an HTTPS origin without credentials, path, query, or fragment.".into());
            }
            return Ok(Self {
                mode,
                origin: Some(url.origin().ascii_serialization()),
            });
        }
        Ok(Self { mode, origin: None })
    }
    fn permits(&self, request: &Request) -> bool {
        let headers = request.headers();
        let host = headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        let expected = if self.mode == Mode::Production {
            self.origin.clone().unwrap()
        } else {
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
            url.origin().ascii_serialization()
        };
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
                    && (self.mode == Mode::Production
                        || headers
                            .get("sec-fetch-site")
                            .is_none_or(|v| v != "cross-site"))
            }
        }
    }
}

pub async fn guard(
    State(deployment): State<Deployment>,
    mut request: Request,
    next: Next,
) -> Response {
    if !deployment.permits(&request) {
        return (StatusCode::FORBIDDEN, "Request origin is not allowed.").into_response();
    }
    let path = request.uri().path();
    let mut response = if path == "/api/deployment" {
        Json(serde_json::json!({"mode": deployment.mode.name(), "hosted": deployment.mode != Mode::Local})).into_response()
    } else if deployment.mode == Mode::Production
        && (path == "/bios"
            || path == "/api/snapshot"
            || path.starts_with("/overlay")
            || path.starts_with("/local/")
            || path == "/api/library/upload")
    {
        StatusCode::NOT_FOUND.into_response()
    } else if deployment.mode == Mode::Production && path == "/api/health" {
        Json(serde_json::json!({"ok": true})).into_response()
    } else if deployment.mode == Mode::Local
        && path == "/api/community-addons"
        && request.method() == Method::GET
    {
        Json(serde_json::json!({"addons": [], "next_offset": null})).into_response()
    } else if deployment.mode == Mode::Local
        && (path.starts_with("/api/community-addons/")
            || path.starts_with("/api/lobby")
            || path == "/api/proxy"
            || path == "/api/library/upload"
            || path == "/contact"
            || path.starts_with("/support/")
            || path.starts_with("/api/tickets"))
    {
        StatusCode::NOT_FOUND.into_response()
    } else {
        if deployment.mode == Mode::Production {
            request
                .headers_mut()
                .insert("x-forwarded-proto", HeaderValue::from_static("https"));
        }
        next.run(request).await
    };
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
    if deployment.mode == Mode::Production {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local() -> Deployment {
        Deployment::new(Mode::Local, "127.0.0.1".parse().unwrap(), None).unwrap()
    }
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
    fn local_rejects_dns_rebinding_and_cross_origin_writes() {
        let local = local();
        assert!(local.permits(&request(Method::GET, "localhost:8877", None, false)));
        assert!(!local.permits(&request(Method::GET, "evil.example:8877", None, false)));
        assert!(!local.permits(&request(Method::POST, "localhost:8877", None, false)));
        assert!(!local.permits(&request(
            Method::POST,
            "localhost:8877",
            Some("https://evil.example"),
            false
        )));
        assert!(local.permits(&request(
            Method::POST,
            "localhost:8877",
            Some("http://localhost:8877"),
            false
        )));
        assert!(!local.permits(&request(Method::GET, "localhost:8877", None, true)));
        assert!(!local.permits(&request(
            Method::GET,
            "localhost:8877",
            Some("http://localhost:8878"),
            true
        )));
    }
    #[test]
    fn unsafe_deployments_fail_closed() {
        assert!(Deployment::new(Mode::Local, "0.0.0.0".parse().unwrap(), None).is_err());
        for origin in [
            None,
            Some("http://example.com"),
            Some("https://example.com/path"),
            Some("https://user@example.com"),
        ] {
            assert!(Deployment::new(
                Mode::Production,
                "0.0.0.0".parse().unwrap(),
                origin.map(str::to_owned)
            )
            .is_err());
        }
        assert!(Mode::parse("prodution").is_err());
        let prod = Deployment::new(
            Mode::Production,
            "0.0.0.0".parse().unwrap(),
            Some("https://example.com".into()),
        )
        .unwrap();
        assert!(prod.permits(&request(
            Method::POST,
            "internal:8877",
            Some("https://example.com"),
            false
        )));
        assert!(!prod.permits(&request(
            Method::POST,
            "internal:8877",
            Some("https://example.com.evil"),
            false
        )));
    }
}
