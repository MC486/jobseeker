//! Bearer tokens (CLI / extension) and session cookies (password mode).
//!
//! `auth.mode = none` (the loopback default) does not require a header or cookie.
//! `token` and `password` both gate `/api/*`: ingest-scoped device tokens may only
//! hit `/ingest/*`; everything else needs an owner session or an owner-scoped token.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;
use jobseeker_core::config::AuthMode;
use jobseeker_core::Error;
use jobseeker_db::repo::session::{self, SessionRow};
use jobseeker_db::repo::token::{self, DeviceTokenRow};
use jobseeker_db::repo::user::UserRow;

use crate::{ApiError, AppState};

pub const SESSION_COOKIE: &str = "js_session";

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub token: Option<DeviceTokenRow>,
    pub session: Option<SessionIdentity>,
}

#[derive(Clone, Debug)]
pub struct SessionIdentity {
    pub session: SessionRow,
    pub user: UserRow,
}

impl Identity {
    pub fn allows_ingest(&self) -> bool {
        self.session.is_some()
            || self
                .token
                .as_ref()
                .is_some_and(|t| t.allows("ingest") || t.is_owner())
    }

    pub fn allows_owner(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.user.is_owner())
            || self.token.as_ref().is_some_and(|t| t.is_owner())
    }

    pub fn authenticated(&self) -> bool {
        self.token.is_some() || self.session.is_some()
    }

    pub fn display_name(&self) -> Option<String> {
        self.session
            .as_ref()
            .map(|s| s.user.username.clone())
            .or_else(|| self.token.as_ref().map(|t| t.name.clone()))
    }

    pub fn scopes(&self) -> Vec<String> {
        if let Some(s) = &self.session {
            return if s.user.is_owner() {
                vec!["owner".into()]
            } else {
                vec!["viewer".into()]
            };
        }
        self.token
            .as_ref()
            .map(|t| t.scopes.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Public,
    Ingest,
    Owner,
}

fn need_for(method: &Method, path: &str) -> Need {
    if path == "/healthz"
        || path == "/readyz"
        || path == "/openapi.json"
        || path == "/api/v1/meta"
        || path == "/api/v1/auth/me"
        || path == "/api/v1/auth/pair"
        || path == "/api/v1/auth/login"
        || path == "/api/v1/auth/logout"
    {
        return Need::Public;
    }
    if !path.starts_with("/api/") {
        return Need::Public;
    }
    if path.starts_with("/api/v1/ingest/") {
        return Need::Ingest;
    }
    let _ = method;
    Need::Owner
}

pub async fn layer(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cookie = cookie_value(req.headers(), SESSION_COOKIE);

    let identity = resolve(&state, header.as_deref(), cookie.as_deref()).await?;
    enforce(&state, &path, &method, &identity)?;
    if let Some(token) = &identity.token {
        let _ = token::touch(&state.pipeline.db, &token.id).await;
    }
    if let Some(sess) = &identity.session {
        let _ = session::touch(&state.pipeline.db, &sess.session.id).await;
    }
    req.extensions_mut().insert(identity);
    Ok(next.run(req).await)
}

async fn resolve(
    state: &AppState,
    header: Option<&str>,
    cookie: Option<&str>,
) -> Result<Identity, ApiError> {
    let mut identity = Identity::default();
    if let Some(header) = header {
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .unwrap_or(header)
            .trim();
        if !token.is_empty() {
            let row = token::lookup(&state.pipeline.db, token)
                .await
                .map_err(ApiError)?;
            if row.is_none() && state.pipeline.config.auth.mode != AuthMode::None {
                return Err(ApiError(Error::Unauthorized));
            }
            identity.token = row;
        }
    }
    if identity.token.is_none() {
        if let Some(cookie) = cookie {
            if let Some((sess, user)) = session::lookup(&state.pipeline.db, cookie)
                .await
                .map_err(ApiError)?
            {
                identity.session = Some(SessionIdentity {
                    session: sess,
                    user,
                });
            } else if state.pipeline.config.auth.mode == AuthMode::Password && header.is_none() {
                // A leftover / forged cookie is just "not logged in", not a hard 401, so
                // `/auth/me` can still tell the UI to show the login form.
            }
        }
    }
    Ok(identity)
}

fn enforce(
    state: &AppState,
    path: &str,
    method: &Method,
    identity: &Identity,
) -> Result<(), ApiError> {
    if state.pipeline.config.auth.mode == AuthMode::None {
        return Ok(());
    }
    match need_for(method, path) {
        Need::Public => Ok(()),
        Need::Ingest => {
            if identity.allows_ingest() {
                Ok(())
            } else {
                Err(ApiError(Error::Unauthorized))
            }
        }
        Need::Owner => {
            if identity.allows_owner() {
                Ok(())
            } else if identity.token.is_some() {
                Err(ApiError(Error::Forbidden("token is scoped to ingest")))
            } else {
                Err(ApiError(Error::Unauthorized))
            }
        }
    }
}

pub fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

pub fn session_cookie_header(token: &str, max_age_secs: i64, secure: bool) -> String {
    let mut buf =
        format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}");
    if secure {
        buf.push_str("; Secure");
    }
    buf
}

pub fn clear_session_cookie(secure: bool) -> String {
    let mut buf = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    if secure {
        buf.push_str("; Secure");
    }
    buf
}

pub fn cookie_secure(public_url: Option<&str>) -> bool {
    public_url.is_some_and(|u| u.starts_with("https://"))
}

/// In-memory login rate limit: N failures / 15 minutes per IP, then exponential lockout.
#[derive(Debug)]
pub struct LoginLimiter {
    max_per_window: u32,
    window: Duration,
    inner: Mutex<HashMap<String, Vec<Instant>>>,
}

impl LoginLimiter {
    pub fn new(max_per_window: u32) -> Arc<Self> {
        Arc::new(Self {
            max_per_window: max_per_window.max(1),
            window: Duration::from_secs(15 * 60),
            inner: Mutex::new(HashMap::new()),
        })
    }

    pub fn check(&self, ip: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let hits = map.entry(ip.to_string()).or_default();
        hits.retain(|t| now.duration_since(*t) < self.window);
        if hits.len() as u32 >= self.max_per_window {
            let over = hits.len() as u32 - self.max_per_window + 1;
            let extra = 30u64.saturating_mul(1u64 << over.min(8));
            let oldest = hits.first().copied().unwrap_or(now);
            let window_left = self
                .window
                .saturating_sub(now.duration_since(oldest))
                .as_secs();
            return Err(extra.max(window_left).max(1));
        }
        Ok(())
    }

    pub fn record_failure(&self, ip: &str) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(ip.to_string()).or_default().push(Instant::now());
    }

    pub fn clear(&self, ip: &str) {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_login_and_health_are_public() {
        assert_eq!(need_for(&Method::POST, "/api/v1/auth/pair"), Need::Public);
        assert_eq!(need_for(&Method::POST, "/api/v1/auth/login"), Need::Public);
        assert_eq!(need_for(&Method::POST, "/api/v1/auth/logout"), Need::Public);
        assert_eq!(need_for(&Method::GET, "/healthz"), Need::Public);
        assert_eq!(need_for(&Method::GET, "/"), Need::Public);
    }

    #[test]
    fn capture_is_ingest_and_jobs_are_owner() {
        assert_eq!(
            need_for(&Method::POST, "/api/v1/ingest/capture"),
            Need::Ingest
        );
        assert_eq!(need_for(&Method::GET, "/api/v1/jobs"), Need::Owner);
        assert_eq!(need_for(&Method::PATCH, "/api/v1/jobs/abc"), Need::Owner);
        assert_eq!(need_for(&Method::GET, "/api/v1/events"), Need::Owner);
    }

    #[test]
    fn limiter_trips_after_the_configured_window() {
        let lim = LoginLimiter::new(3);
        assert!(lim.check("1.1.1.1").is_ok());
        lim.record_failure("1.1.1.1");
        lim.record_failure("1.1.1.1");
        lim.record_failure("1.1.1.1");
        assert!(lim.check("1.1.1.1").is_err());
        assert!(lim.check("2.2.2.2").is_ok());
        lim.clear("1.1.1.1");
        assert!(lim.check("1.1.1.1").is_ok());
    }
}
