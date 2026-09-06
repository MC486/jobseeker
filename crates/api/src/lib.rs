//! HTTP API: health, ingest, jobs, tasks, and the SPA shell.
//!
//! Domain types stay in `jobseeker-core`. This crate speaks DTOs and the error envelope
//! from `docs/09-api.md`.

mod auth;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures::{Stream, StreamExt};
use jobseeker_core::config::AuthMode;
use jobseeker_core::domain::capture::CaptureSubmission;
use jobseeker_core::domain::event::DomainEvent;
use jobseeker_core::ids::{JobId, TaskId};
use jobseeker_core::{Error, Result};
use jobseeker_db::queue::Queue;
use jobseeker_db::repo::event;
use jobseeker_db::repo::job::{self, JobFilter, JobPatch};
use jobseeker_db::repo::score;
use jobseeker_db::repo::session;
use jobseeker_db::repo::token;
use jobseeker_db::repo::user;
use jobseeker_pipeline::{IngestAccepted, Pipeline};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;

/// Shared by every handler.
#[derive(Clone)]
pub struct AppState {
    pub pipeline: Pipeline,
    pub login_limiter: Arc<auth::LoginLimiter>,
}

impl AppState {
    pub fn new(pipeline: Pipeline) -> Self {
        let limit = pipeline.config.auth.login_rate_limit_per_15min;
        Self {
            pipeline,
            login_limiter: auth::LoginLimiter::new(limit),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorBody {
    pub error: ErrorInner,
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorInner {
    pub code: &'static str,
    pub message: String,
}

pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(value: Error) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let message = if self.0.is_client_facing() {
            self.0.to_string()
        } else {
            tracing::error!(error = %self.0, "internal error");
            "internal error".into()
        };
        (
            status,
            Json(ErrorBody {
                error: ErrorInner {
                    code: self.0.code(),
                    message,
                },
            }),
        )
            .into_response()
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IngestUrlRequest {
    pub url: String,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IngestPasteRequest {
    pub text: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub source_hint: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Accepted {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listing_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_id: Option<String>,
}

impl From<IngestAccepted> for Accepted {
    fn from(v: IngestAccepted) -> Self {
        Self {
            task_id: v.task_id.to_string(),
            listing_id: v.listing_id.map(|id| id.to_string()),
            capture_id: v.capture_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct JobListQuery {
    pub q: Option<String>,
    pub status: Option<String>,
    pub work_mode: Option<String>,
    pub seniority: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub archived: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct PageDto<T: Serialize> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        healthz,
        readyz,
        meta,
        ingest_url,
        ingest_paste,
        list_jobs,
        get_job,
        patch_job,
        get_job_match,
        merge_job,
        get_profile,
        import_resume,
        get_task,
        events,
        auth_me,
        auth_login,
        auth_logout,
        auth_pair,
        list_tokens,
        create_token,
        revoke_token
    ),
    components(schemas(
        IngestUrlRequest,
        IngestPasteRequest,
        ImportResumeRequest,
        Accepted,
        MetaResponse,
        PatchJobRequest,
        MergeJobRequest,
        PairRequest,
        LoginRequest,
        CreateTokenRequest,
        DomainEventDto
    )),
    info(
        title = "jobseeker",
        description = "Self-hosted job-search workbench",
        version = "0.1.0"
    )
)]
pub struct ApiDoc;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetaResponse {
    pub version: &'static str,
    pub schema_migrations: usize,
    pub llm_provider: String,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/v1/meta", get(meta))
        .route("/api/v1/ingest/url", post(ingest_url))
        .route("/api/v1/ingest/paste", post(ingest_paste))
        .route("/api/v1/ingest/capture", post(ingest_capture))
        .route("/api/v1/jobs", get(list_jobs))
        .route("/api/v1/jobs/{id}", get(get_job).patch(patch_job))
        .route("/api/v1/jobs/{id}/match", get(get_job_match))
        .route("/api/v1/jobs/{id}/merge", post(merge_job))
        .route("/api/v1/profiles/default", get(get_profile))
        .route(
            "/api/v1/profiles/default/import-resume",
            post(import_resume),
        )
        .route("/api/v1/tasks/{id}", get(get_task))
        .route("/api/v1/events", get(events))
        .route("/api/v1/auth/me", get(auth_me))
        .route("/api/v1/auth/login", post(auth_login))
        .route("/api/v1/auth/logout", post(auth_logout))
        .route("/api/v1/auth/pair", post(auth_pair))
        .route("/api/v1/auth/tokens", get(list_tokens).post(create_token))
        .route(
            "/api/v1/auth/tokens/{id}",
            axum::routing::delete(revoke_token),
        )
        .route("/openapi.json", get(openapi));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api)
        .fallback(spa_fallback)
        .layer(middleware::from_fn_with_state(state.clone(), auth::layer))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::very_permissive())
        .with_state(state)
}

pub async fn serve(pipeline: Pipeline) -> Result<()> {
    let bind: SocketAddr = pipeline
        .config
        .server
        .bind
        .parse()
        .map_err(|e| Error::Config(format!("bad bind: {e}")))?;
    if pipeline.config.is_dangerously_open() {
        tracing::warn!(
            bind = %bind,
            "bound beyond loopback with auth.mode=none; anyone on the network can read your job search"
        );
    }
    if pipeline.config.auth.mode == AuthMode::Password {
        match user::count(&pipeline.db).await {
            Ok(0) => tracing::warn!(
                "auth.mode=password but no user exists; run `jobseeker user set-password`"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "could not count users at boot"),
        }
    }
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    tracing::info!(%bind, "jobseeker listening");
    axum::serve(listener, router(AppState::new(pipeline)))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| Error::Network(e.to_string()))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[utoipa::path(get, path = "/healthz", responses((status = 200, description = "alive")))]
async fn healthz() -> &'static str {
    "ok"
}

#[derive(Serialize)]
struct Ready {
    db: bool,
    migrations: usize,
    queue_depth: i64,
}

#[utoipa::path(get, path = "/readyz", responses((status = 200), (status = 503)))]
async fn readyz(State(state): State<AppState>) -> ApiResult<Response> {
    state.pipeline.db.ping().await.map_err(ApiError)?;
    let q = state.pipeline.queue();
    let depth: i64 = q
        .depth()
        .await
        .map_err(ApiError)?
        .into_iter()
        .map(|(_, n)| n)
        .sum();
    let body = Ready {
        db: true,
        migrations: jobseeker_db::Db::known_migrations(),
        queue_depth: depth,
    };
    Ok((StatusCode::OK, Json(body)).into_response())
}

#[utoipa::path(get, path = "/api/v1/meta", responses((status = 200, body = MetaResponse)))]
async fn meta(State(state): State<AppState>) -> Json<MetaResponse> {
    Json(MetaResponse {
        version: jobseeker_core::VERSION,
        schema_migrations: jobseeker_db::Db::known_migrations(),
        llm_provider: format!("{:?}", state.pipeline.config.llm.provider).to_ascii_lowercase(),
    })
}

/// OpenAPI 3 document derived from the handlers. The CLI dumps this without
/// opening a database so `just gen-client` does not need a running server.
pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

async fn openapi() -> impl IntoResponse {
    Json(openapi_spec())
}

#[utoipa::path(post, path = "/api/v1/ingest/url", request_body = IngestUrlRequest, responses((status = 202, body = Accepted)))]
async fn ingest_url(
    State(state): State<AppState>,
    Json(body): Json<IngestUrlRequest>,
) -> ApiResult<Response> {
    let accepted = state
        .pipeline
        .ingest_url(&body.url)
        .await
        .map_err(ApiError)?;
    Ok(accepted_response(accepted))
}

#[utoipa::path(post, path = "/api/v1/ingest/paste", request_body = IngestPasteRequest, responses((status = 202, body = Accepted)))]
async fn ingest_paste(
    State(state): State<AppState>,
    Json(body): Json<IngestPasteRequest>,
) -> ApiResult<Response> {
    let accepted = state
        .pipeline
        .ingest_paste(&body.text, body.url.as_deref())
        .await
        .map_err(ApiError)?;
    Ok(accepted_response(accepted))
}

async fn ingest_capture(
    State(state): State<AppState>,
    Json(body): Json<CaptureSubmission>,
) -> ApiResult<Response> {
    let accepted = state
        .pipeline
        .ingest_capture(body)
        .await
        .map_err(ApiError)?;
    Ok(accepted_response(accepted))
}

fn accepted_response(accepted: IngestAccepted) -> Response {
    let location = format!("/api/v1/tasks/{}", accepted.task_id);
    let mut res = (StatusCode::ACCEPTED, Json(Accepted::from(accepted))).into_response();
    if let Ok(value) = HeaderValue::from_str(&location) {
        res.headers_mut().insert(header::LOCATION, value);
    }
    res
}

#[utoipa::path(get, path = "/api/v1/jobs", responses((status = 200)))]
async fn list_jobs(
    State(state): State<AppState>,
    Query(q): Query<JobListQuery>,
) -> ApiResult<Json<PageDto<jobseeker_db::repo::job::JobListRow>>> {
    let filter = JobFilter {
        query: q.q,
        status: q.status.as_deref().and_then(|s| s.parse().ok()),
        work_mode: q.work_mode.as_deref().and_then(|s| s.parse().ok()),
        seniority: q.seniority.as_deref().and_then(|s| s.parse().ok()),
        include_archived: q.archived.unwrap_or(false),
        cursor: q.cursor,
        limit: q.limit,
        ..Default::default()
    };
    let page = job::list(&state.pipeline.db, &filter)
        .await
        .map_err(ApiError)?;
    Ok(Json(PageDto {
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(get, path = "/api/v1/jobs/{id}", responses((status = 200), (status = 404)))]
async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<jobseeker_db::repo::job::JobDetail>> {
    let id: JobId = id.parse().map_err(ApiError)?;
    let job = job::get(&state.pipeline.db, &id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("job")))?;
    Ok(Json(job))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PatchJobRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub user_rating: Option<i32>,
    #[serde(default)]
    pub user_notes_md: Option<String>,
    #[serde(default)]
    pub is_archived: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
}

#[utoipa::path(patch, path = "/api/v1/jobs/{id}", request_body = PatchJobRequest, responses((status = 200), (status = 404)))]
async fn patch_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchJobRequest>,
) -> ApiResult<Json<jobseeker_db::repo::job::JobDetail>> {
    let id: JobId = id.parse().map_err(ApiError)?;
    let n = job::patch(
        &state.pipeline.db,
        &id,
        &JobPatch {
            title: body.title,
            user_rating: body.user_rating,
            user_notes_md: body.user_notes_md,
            is_archived: body.is_archived,
            status: body.status,
        },
    )
    .await
    .map_err(ApiError)?;
    if n == 0 {
        return Err(ApiError(Error::NotFound("job")));
    }
    let job = job::get(&state.pipeline.db, &id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("job")))?;
    let _ = state
        .pipeline
        .emit(
            DomainEvent::JOB_UPDATED,
            "job",
            id.as_str(),
            serde_json::json!({ "source": "manual" }),
        )
        .await;
    Ok(Json(job))
}

#[utoipa::path(get, path = "/api/v1/jobs/{id}/match", responses((status = 200), (status = 404)))]
async fn get_job_match(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<jobseeker_db::repo::score::MatchSummary>> {
    let id: JobId = id.parse().map_err(ApiError)?;
    job::get(&state.pipeline.db, &id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("job")))?;
    let score = score::latest_for_job(&state.pipeline.db, &id, None)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("match")))?;
    Ok(Json(score))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MergeJobRequest {
    pub into_job_id: String,
}

#[utoipa::path(post, path = "/api/v1/jobs/{id}/merge", request_body = MergeJobRequest, responses((status = 200), (status = 400), (status = 404)))]
async fn merge_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MergeJobRequest>,
) -> ApiResult<Json<jobseeker_db::repo::job::MergeReport>> {
    let from: JobId = id.parse().map_err(ApiError)?;
    let into: JobId = body.into_job_id.parse().map_err(ApiError)?;
    let report = state
        .pipeline
        .merge_jobs(&from, &into)
        .await
        .map_err(ApiError)?;
    Ok(Json(report))
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ImportResumeRequest {
    pub text: String,
}

#[utoipa::path(get, path = "/api/v1/profiles/default", responses((status = 200), (status = 404)))]
async fn get_profile(
    State(state): State<AppState>,
) -> ApiResult<Json<jobseeker_db::repo::experience::ProfileView>> {
    let id = jobseeker_db::repo::profile::ensure_default(&state.pipeline.db)
        .await
        .map_err(ApiError)?;
    let view = jobseeker_db::repo::experience::get_view(&state.pipeline.db, &id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("profile")))?;
    Ok(Json(view))
}

#[utoipa::path(
    post,
    path = "/api/v1/profiles/default/import-resume",
    request_body = ImportResumeRequest,
    responses((status = 200), (status = 400))
)]
async fn import_resume(
    State(state): State<AppState>,
    Json(body): Json<ImportResumeRequest>,
) -> ApiResult<Json<jobseeker_db::repo::experience::ImportReport>> {
    let report = state
        .pipeline
        .import_resume(&body.text)
        .await
        .map_err(ApiError)?;
    Ok(Json(report))
}

#[utoipa::path(get, path = "/api/v1/tasks/{id}", responses((status = 200), (status = 404)))]
async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<jobseeker_db::queue::TaskView>> {
    let id: TaskId = id.parse().map_err(ApiError)?;
    let q = Queue::new(
        &state.pipeline.db,
        state.pipeline.config.worker.lease_seconds,
        state.pipeline.config.worker.max_attempts,
    );
    let task = q
        .get(&id)
        .await
        .map_err(ApiError)?
        .ok_or(ApiError(Error::NotFound("task")))?;
    Ok(Json(task))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub since: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DomainEventDto {
    pub id: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    pub payload: serde_json::Value,
    pub created_at: String,
}

fn parse_since(q: &EventsQuery, headers: &axum::http::HeaderMap) -> i64 {
    if let Some(n) = q.since {
        return n.max(0);
    }
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn sse_event(ev: &DomainEvent) -> Result<Event, Infallible> {
    let data = serde_json::to_string(ev).unwrap_or_else(|_| "{}".into());
    Ok(Event::default()
        .id(ev.id.to_string())
        .event(ev.kind.clone())
        .data(data))
}

#[utoipa::path(
    get,
    path = "/api/v1/events",
    params(("since" = Option<i64>, Query, description = "Replay events with id greater than this")),
    responses((status = 200, description = "text/event-stream of DomainEvent"))
)]
async fn events(
    State(state): State<AppState>,
    Query(q): Query<EventsQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let since = parse_since(&q, &headers);
    let rx = state.pipeline.subscribe();
    let backfill = event::since(&state.pipeline.db, since, 200)
        .await
        .map_err(ApiError)?;
    let sent_upto = backfill.last().map(|e| e.id).unwrap_or(since);

    let backfill_stream = futures::stream::iter(backfill.into_iter().map(|ev| sse_event(&ev)));
    let live = futures::stream::unfold((rx, sent_upto), |(mut rx, sent_upto)| async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.id > sent_upto => {
                    let next_upto = sent_upto.max(ev.id);
                    return Some((sse_event(&ev), (rx, next_upto)));
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Ok(Sse::new(backfill_stream.chain(live)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .event(Event::default().comment("ping")),
    ))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeResponse {
    pub auth_mode: String,
    pub authenticated: bool,
    pub name: Option<String>,
    pub scopes: Vec<String>,
}

#[utoipa::path(get, path = "/api/v1/auth/me", responses((status = 200)))]
async fn auth_me(
    State(state): State<AppState>,
    identity: Option<Extension<auth::Identity>>,
) -> Json<MeResponse> {
    let identity = identity.map(|Extension(i)| i).unwrap_or_default();
    Json(MeResponse {
        auth_mode: match state.pipeline.config.auth.mode {
            AuthMode::None => "none",
            AuthMode::Token => "token",
            AuthMode::Password => "password",
        }
        .to_string(),
        authenticated: identity.authenticated(),
        name: identity.display_name(),
        scopes: identity.scopes(),
    })
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest, responses((status = 200), (status = 401), (status = 429)))]
async fn auth_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Response> {
    if state.pipeline.config.auth.mode != AuthMode::Password {
        return Err(ApiError(Error::BadRequest(
            "password login is disabled; set auth.mode = \"password\"".into(),
        )));
    }
    let ip = client_ip(&headers);
    if let Err(retry_after) = state.login_limiter.check(&ip) {
        return Err(ApiError(Error::RateLimited(retry_after)));
    }
    let user = match user::authenticate(&state.pipeline.db, &body.username, &body.password).await {
        Ok(u) => u,
        Err(Error::Unauthorized) => {
            state.login_limiter.record_failure(&ip);
            return Err(ApiError(Error::Unauthorized));
        }
        Err(e) => return Err(ApiError(e)),
    };
    state.login_limiter.clear(&ip);
    user::record_login(&state.pipeline.db, &user.id)
        .await
        .map_err(ApiError)?;
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let issued = session::issue(
        &state.pipeline.db,
        &user,
        state.pipeline.config.auth.session_ttl_days,
        ua,
        Some(&ip),
    )
    .await
    .map_err(ApiError)?;
    let max_age = i64::from(state.pipeline.config.auth.session_ttl_days) * 24 * 60 * 60;
    let secure = auth::cookie_secure(state.pipeline.config.server.public_url.as_deref());
    let cookie = auth::session_cookie_header(&issued.token, max_age, secure);
    let mut res = (
        StatusCode::OK,
        Json(MeResponse {
            auth_mode: "password".into(),
            authenticated: true,
            name: Some(user.username),
            scopes: vec!["owner".into()],
        }),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        res.headers_mut().append(header::SET_COOKIE, value);
    }
    Ok(res)
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 204)))]
async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::cookie_value(&headers, auth::SESSION_COOKIE) {
        let _ = session::revoke_token(&state.pipeline.db, &token).await;
    }
    let secure = auth::cookie_secure(state.pipeline.config.server.public_url.as_deref());
    let mut res = StatusCode::NO_CONTENT.into_response();
    if let Ok(value) = HeaderValue::from_str(&auth::clear_session_cookie(secure)) {
        res.headers_mut().append(header::SET_COOKIE, value);
    }
    res
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PairRequest {
    pub code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IssuedTokenResponse {
    pub token: String,
    pub name: String,
    pub scopes: Vec<String>,
}

#[utoipa::path(post, path = "/api/v1/auth/pair", request_body = PairRequest, responses((status = 200)))]
async fn auth_pair(
    State(state): State<AppState>,
    Json(body): Json<PairRequest>,
) -> ApiResult<Json<IssuedTokenResponse>> {
    let issued = token::exchange_pairing_code(&state.pipeline.db, &body.code)
        .await
        .map_err(ApiError)?;
    Ok(Json(IssuedTokenResponse {
        token: issued.token,
        name: issued.row.name,
        scopes: issued.row.scopes,
    }))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateTokenRequest {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[utoipa::path(get, path = "/api/v1/auth/tokens", responses((status = 200)))]
async fn list_tokens(
    State(state): State<AppState>,
) -> ApiResult<Json<PageDto<jobseeker_db::repo::token::DeviceTokenRow>>> {
    let items = token::list(&state.pipeline.db).await.map_err(ApiError)?;
    Ok(Json(PageDto {
        items,
        next_cursor: None,
    }))
}

#[utoipa::path(post, path = "/api/v1/auth/tokens", request_body = CreateTokenRequest, responses((status = 201)))]
async fn create_token(
    State(state): State<AppState>,
    Json(body): Json<CreateTokenRequest>,
) -> ApiResult<Response> {
    let issued = token::issue_token(&state.pipeline.db, &body.name, &body.scopes)
        .await
        .map_err(ApiError)?;
    Ok((
        StatusCode::CREATED,
        Json(IssuedTokenResponse {
            token: issued.token,
            name: issued.row.name,
            scopes: issued.row.scopes,
        }),
    )
        .into_response())
}

#[utoipa::path(delete, path = "/api/v1/auth/tokens/{id}", responses((status = 204), (status = 404)))]
async fn revoke_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let n = token::revoke(&state.pipeline.db, &id)
        .await
        .map_err(ApiError)?;
    if n == 0 {
        return Err(ApiError(Error::NotFound("token")));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn spa_fallback(State(state): State<AppState>, uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") || uri.path() == "/openapi.json" {
        return ApiError(Error::NotFound("route")).into_response();
    }
    let dist = PathBuf::from("web/dist");
    if dist.is_dir() {
        let rel = uri.path().trim_start_matches('/');
        let candidate = if rel.is_empty() {
            dist.join("index.html")
        } else {
            dist.join(rel)
        };
        if candidate.is_file() {
            if let Ok(bytes) = std::fs::read(&candidate) {
                let mime = mime_guess::from_path(&candidate)
                    .first_or_octet_stream()
                    .to_string();
                return (StatusCode::OK, [(header::CONTENT_TYPE, mime)], bytes).into_response();
            }
        }
        if let Ok(bytes) = std::fs::read(dist.join("index.html")) {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                bytes,
            )
                .into_response();
        }
    }
    let _ = state;
    Html(FALLBACK_HTML).into_response()
}

const FALLBACK_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <meta name="viewport" content="width=device-width, initial-scale=1"/>
  <title>jobseeker</title>
  <style>
    :root { color-scheme: light dark; --bg: #111; --fg: #eee; --muted: #9aa; --acc: #7c9cff; }
    body { font: 16px/1.45 system-ui, sans-serif; margin: 0; background: var(--bg); color: var(--fg); }
    header, main { max-width: 880px; margin: 0 auto; padding: 1.25rem; }
    header { display: flex; justify-content: space-between; align-items: baseline; }
    h1 { font-size: 1.15rem; letter-spacing: .04em; }
    a { color: var(--acc); text-decoration: none; }
    form { display: grid; gap: .6rem; margin: 1rem 0 1.5rem; }
    input, textarea, button { font: inherit; padding: .55rem .7rem; border-radius: 8px; border: 1px solid #333; background: #1b1b1b; color: inherit; }
    button { background: var(--acc); color: #111; border: 0; font-weight: 650; cursor: pointer; width: max-content; }
    .muted { color: var(--muted); }
    .job { padding: .8rem 0; border-bottom: 1px solid #2a2a2a; }
    .job b { display: block; }
    pre { white-space: pre-wrap; }
  </style>
</head>
<body>
  <header>
    <h1>jobseeker</h1>
    <span class="muted">self-hosted workbench</span>
  </header>
  <main>
    <form id="ingest">
      <label>Paste a public job URL
        <input name="url" placeholder="https://boards.greenhouse.io/…/jobs/123" required/>
      </label>
      <button type="submit">Ingest</button>
      <p class="muted">LinkedIn and Indeed need the browser extension — the server will not log in for you.</p>
    </form>
    <div id="status" class="muted"></div>
    <div id="jobs"></div>
  </main>
  <script>
    const status = document.getElementById('status');
    async function loadJobs() {
      const r = await fetch('/api/v1/jobs');
      const data = await r.json();
      document.getElementById('jobs').innerHTML = (data.items||[]).map(j =>
        `<div class="job"><b>${j.title}</b><span class="muted">${j.company_name} · ${j.work_mode} · ${j.status}</span></div>`
      ).join('') || '<p class="muted">No jobs yet.</p>';
    }
    document.getElementById('ingest').onsubmit = async (e) => {
      e.preventDefault();
      const url = e.target.url.value;
      status.textContent = 'Queuing…';
      const r = await fetch('/api/v1/ingest/url', {
        method: 'POST', headers: {'content-type':'application/json'},
        body: JSON.stringify({url})
      });
      const body = await r.json();
      if (!r.ok) { status.textContent = body.error?.message || 'failed'; return; }
      status.textContent = 'Queued ' + body.task_id + ' — refresh shortly.';
      e.target.reset();
      setTimeout(loadJobs, 1500);
    };
    loadJobs();
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn app() -> Router {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        // Leak the tempdir so the DB file outlives the router for the duration of the test.
        std::mem::forget(dir);
        router(AppState::new(pipe))
    }

    #[tokio::test]
    async fn healthz_is_unauthenticated_and_ok() {
        let app = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ingest_linkedin_returns_needs_browser() {
        let app = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ingest/url")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"url":"https://www.linkedin.com/jobs/view/1"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn paste_ingest_returns_202_and_jobs_list_is_empty_until_drain() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let app = router(AppState::new(pipe.clone()));
        let html = r#"<html><head><script type="application/ld+json">{"@type":"JobPosting","title":"Engineer","hiringOrganization":{"name":"Acme"},"description":"<p>Hi</p>"}</script></head></html>"#;
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ingest/paste")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"text": html}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn paste_then_drain_exposes_a_match_score() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let html = include_str!("../../../fixtures/greenhouse-platform-engineer.html");
        let accepted = pipe.ingest_paste(html, None).await.unwrap();
        pipe.drain().await.unwrap();
        let page =
            jobseeker_db::repo::job::list(&pipe.db, &jobseeker_db::repo::job::JobFilter::default())
                .await
                .unwrap();
        let job_id = page.items[0].id.clone();
        assert!(page.items[0].match_overall.is_some());
        assert!(
            page.items[0].skills_coverage.is_some(),
            "list rows carry the skills split so the table can show it"
        );

        let app = router(AppState::new(pipe.clone()));
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/jobs/{job_id}/match"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let patched = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/jobs/{job_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"title":"Renamed by hand","is_archived":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(patched.status(), StatusCode::OK);
        let _ = accepted;
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn openapi_document_is_served() {
        let app = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let spec: serde_json::Value = body_json(res).await;
        assert!(
            spec["paths"]["/api/v1/events"].is_object(),
            "OpenAPI must document GET /api/v1/events"
        );
        assert!(
            spec["paths"]["/api/v1/jobs/{id}/merge"].is_object(),
            "OpenAPI must document POST /api/v1/jobs/{{id}}/merge"
        );
    }

    #[tokio::test]
    async fn events_backfill_job_created_after_drain() {
        use futures::StreamExt;
        use tokio::time::{timeout, Duration};

        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let html = include_str!("../../../fixtures/greenhouse-platform-engineer.html");
        pipe.ingest_paste(html, None).await.unwrap();
        pipe.drain().await.unwrap();

        let app = router(AppState::new(pipe.clone()));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events?since=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ctype.contains("text/event-stream"),
            "expected SSE content type, got {ctype}"
        );

        let mut stream = res.into_body().into_data_stream();
        let mut buf = Vec::new();
        let _ = timeout(Duration::from_secs(2), async {
            while let Some(chunk) = stream.next().await {
                buf.extend_from_slice(&chunk.unwrap());
                let so_far = String::from_utf8_lossy(&buf);
                if so_far.contains("job.created") && so_far.contains("task.updated") {
                    break;
                }
            }
        })
        .await;
        let text = String::from_utf8_lossy(&buf);
        assert!(
            text.contains("job.created"),
            "SSE backfill must include job.created, got: {text}"
        );
        assert!(
            text.contains("task.updated"),
            "SSE backfill must include task.updated, got: {text}"
        );
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn token_mode_refuses_events_without_a_bearer() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Token;
        })
        .await
        .unwrap();
        let app = router(AppState::new(pipe));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        std::mem::forget(dir);
    }

    async fn body_json<T: serde::de::DeserializeOwned>(res: Response) -> T {
        let bytes = axum::body::to_bytes(res.into_body(), 1_000_000)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn token_mode_refuses_jobs_without_a_bearer() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Token;
        })
        .await
        .unwrap();
        let app = router(AppState::new(pipe));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn pairing_code_mints_an_ingest_token_that_cannot_read_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Token;
        })
        .await
        .unwrap();
        let code = token::create_pairing_code(&pipe.db, "Firefox on laptop")
            .await
            .unwrap();
        let app = router(AppState::new(pipe.clone()));

        let paired = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"code": code.code}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(paired.status(), StatusCode::OK);
        let issued: IssuedTokenResponse = body_json(paired).await;
        assert!(issued.token.starts_with("jst_"));

        let capture = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/ingest/capture")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {}", issued.token))
                    .body(Body::from(
                        serde_json::json!({
                            "url": "https://www.linkedin.com/jobs/view/1",
                            "html": "<html><body>Engineer at Acme</body></html>"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(capture.status(), StatusCode::ACCEPTED);

        let jobs = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .header("authorization", format!("Bearer {}", issued.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(jobs.status(), StatusCode::FORBIDDEN);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn owner_token_can_list_jobs_in_token_mode() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Token;
        })
        .await
        .unwrap();
        let issued = token::issue_token(&pipe.db, "cli", &["*".into()])
            .await
            .unwrap();
        let app = router(AppState::new(pipe));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .header("authorization", format!("Bearer {}", issued.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        std::mem::forget(dir);
    }

    async fn password_app() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Password;
            c.auth.login_rate_limit_per_15min = 5;
        })
        .await
        .unwrap();
        user::upsert_owner(&pipe.db, "owner", "correct-horse")
            .await
            .unwrap();
        (router(AppState::new(pipe)), dir)
    }

    fn session_cookie(res: &Response) -> String {
        res.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("js_session="))
            .and_then(|v| v.split(';').next())
            .expect("js_session cookie")
            .to_string()
    }

    #[tokio::test]
    async fn password_mode_refuses_jobs_without_a_session() {
        let (app, dir) = password_app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn login_sets_a_cookie_that_can_list_jobs() {
        let (app, dir) = password_app().await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"owner","password":"correct-horse"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let cookie = session_cookie(&res);
        assert!(cookie.contains("jss_"));
        let me: MeResponse = body_json(res).await;
        assert!(me.authenticated);
        assert_eq!(me.name.as_deref(), Some("owner"));

        let listed = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn a_wrong_password_is_unauthorized_and_does_not_set_a_cookie() {
        let (app, dir) = password_app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"owner","password":"wrong-horse"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(res.headers().get(header::SET_COOKIE).is_none());
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn an_unknown_user_is_indistinguishable_from_a_bad_password() {
        let (app, dir) = password_app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"nobody","password":"long-enough"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn logout_revokes_the_session() {
        let (app, dir) = password_app().await;
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"username":"owner","password":"correct-horse"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = session_cookie(&res);
        let out = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(out.status(), StatusCode::NO_CONTENT);
        let listed = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::UNAUTHORIZED);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn owner_bearer_still_works_in_password_mode() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test_with(dir.path().to_path_buf(), |c| {
            c.auth.mode = AuthMode::Password;
        })
        .await
        .unwrap();
        user::upsert_owner(&pipe.db, "owner", "correct-horse")
            .await
            .unwrap();
        let issued = token::issue_token(&pipe.db, "cli", &["*".into()])
            .await
            .unwrap();
        let app = router(AppState::new(pipe));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/jobs")
                    .header("authorization", format!("Bearer {}", issued.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn login_is_rate_limited_after_repeated_failures() {
        let (app, dir) = password_app().await;
        for _ in 0..5 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/auth/login")
                        .header("content-type", "application/json")
                        .header("x-forwarded-for", "203.0.113.9")
                        .body(Body::from(
                            r#"{"username":"owner","password":"wrong-horse"}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        }
        let blocked = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", "203.0.113.9")
                    .body(Body::from(
                        r#"{"username":"owner","password":"correct-horse"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn import_resume_replaces_the_default_profile() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let app = router(AppState::new(pipe));
        let md = include_str!("../../../fixtures/evidence-bank.md");
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/profiles/default/import-resume")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({ "text": md }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let report: serde_json::Value = body_json(res).await;
        assert!(report["accomplishments"].as_u64().unwrap() >= 5);

        let got = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/profiles/default")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(got.status(), StatusCode::OK);
        let profile: serde_json::Value = body_json(got).await;
        assert_eq!(profile["full_name"], "Alex Rivera");
        assert!(profile["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["slug"] == "python"));
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn merge_same_company_jobs_via_api() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let fuller = r#"<html><head><script type="application/ld+json">{"@type":"JobPosting","title":"Data Scientist","hiringOrganization":{"name":"Zillow"},"description":"<ul><li>5+ years data science</li><li>Python</li><li>SQL</li></ul>"}</script></head></html>"#;
        let thinner = r#"<html><head><script type="application/ld+json">{"@type":"JobPosting","title":"Platform Engineer","hiringOrganization":{"name":"Zillow"},"description":"<ul><li>Python</li></ul>"}</script></head></html>"#;
        pipe.ingest_paste(fuller, Some("https://boards.greenhouse.io/zillow/jobs/1"))
            .await
            .unwrap();
        pipe.drain().await.unwrap();
        pipe.ingest_paste(thinner, Some("https://boards.greenhouse.io/zillow/jobs/2"))
            .await
            .unwrap();
        pipe.drain().await.unwrap();

        let page =
            jobseeker_db::repo::job::list(&pipe.db, &jobseeker_db::repo::job::JobFilter::default())
                .await
                .unwrap();
        assert_eq!(page.items.len(), 2, "cross-posts stay visible until merge");
        let into = page
            .items
            .iter()
            .find(|row| row.title == "Data Scientist")
            .expect("fuller posting")
            .id
            .clone();
        let from = page
            .items
            .iter()
            .find(|row| row.title == "Platform Engineer")
            .expect("thinner posting")
            .id
            .clone();

        let app = router(AppState::new(pipe.clone()));
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/jobs/{from}/merge"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "into_job_id": into }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let report: serde_json::Value = body_json(res).await;
        assert_eq!(report["into_id"], into);
        assert!(report["listings_moved"].as_u64().unwrap() >= 1);

        let after =
            jobseeker_db::repo::job::list(&pipe.db, &jobseeker_db::repo::job::JobFilter::default())
                .await
                .unwrap();
        assert_eq!(after.items.len(), 1);
        assert_eq!(after.items[0].id, into);

        let kept = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/jobs/{into}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(kept.status(), StatusCode::OK);
        let detail: serde_json::Value = body_json(kept).await;
        assert!(detail["listings"].as_array().unwrap().len() >= 2);
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn merge_refuses_different_companies_via_api() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = jobseeker_pipeline::for_test(dir.path().to_path_buf())
            .await
            .unwrap();
        let zillow = r#"<html><head><script type="application/ld+json">{"@type":"JobPosting","title":"Data Scientist","hiringOrganization":{"name":"Zillow"},"description":"<p>Python</p>"}</script></head></html>"#;
        let harbor = r#"<html><head><script type="application/ld+json">{"@type":"JobPosting","title":"Applied Scientist","hiringOrganization":{"name":"Harbor"},"description":"<p>Python</p>"}</script></head></html>"#;
        pipe.ingest_paste(zillow, Some("https://boards.greenhouse.io/zillow/jobs/1"))
            .await
            .unwrap();
        pipe.drain().await.unwrap();
        pipe.ingest_paste(harbor, Some("https://boards.greenhouse.io/harbor/jobs/1"))
            .await
            .unwrap();
        pipe.drain().await.unwrap();

        let page =
            jobseeker_db::repo::job::list(&pipe.db, &jobseeker_db::repo::job::JobFilter::default())
                .await
                .unwrap();
        assert_eq!(page.items.len(), 2);
        let from = page.items[0].id.clone();
        let into = page.items[1].id.clone();

        let app = router(AppState::new(pipe));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/jobs/{from}/merge"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "into_job_id": into }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        std::mem::forget(dir);
    }
}
