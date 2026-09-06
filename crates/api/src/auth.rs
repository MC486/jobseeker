//! Bearer-token auth for the CLI and the browser extension.
//!
//! `auth.mode = none` (the loopback default) does not require a header. `token` mode
//! does: ingest-scoped device tokens may only hit `/ingest/*`; everything else needs
//! an owner-scoped token. Pairing codes are the bootstrap — they are themselves secrets.

use axum::extract::{Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;
use jobseeker_core::config::AuthMode;
use jobseeker_core::Error;
use jobseeker_db::repo::token::{self, DeviceTokenRow};

use crate::{ApiError, AppState};

#[derive(Clone, Debug, Default)]
pub struct Identity {
    pub token: Option<DeviceTokenRow>,
}

impl Identity {
    pub fn allows_ingest(&self) -> bool {
        self.token
            .as_ref()
            .is_some_and(|t| t.allows("ingest") || t.is_owner())
    }

    pub fn allows_owner(&self) -> bool {
        self.token.as_ref().is_some_and(|t| t.is_owner())
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

    let identity = resolve(&state, header.as_deref()).await?;
    enforce(&state, &path, &method, &identity)?;
    if let Some(token) = &identity.token {
        let _ = token::touch(&state.pipeline.db, &token.id).await;
    }
    req.extensions_mut().insert(identity);
    Ok(next.run(req).await)
}

async fn resolve(state: &AppState, header: Option<&str>) -> Result<Identity, ApiError> {
    let Some(header) = header else {
        return Ok(Identity::default());
    };
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .unwrap_or(header)
        .trim();
    if token.is_empty() {
        return Ok(Identity::default());
    }
    let row = token::lookup(&state.pipeline.db, token)
        .await
        .map_err(ApiError)?;
    if row.is_none() && state.pipeline.config.auth.mode != AuthMode::None {
        return Err(ApiError(Error::Unauthorized));
    }
    Ok(Identity { token: row })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_and_health_are_public() {
        assert_eq!(need_for(&Method::POST, "/api/v1/auth/pair"), Need::Public);
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
    }
}
