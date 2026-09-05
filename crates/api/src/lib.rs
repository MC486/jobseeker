//! HTTP API: health, ingest, jobs, tasks, and the SPA shell.
//!
//! Domain types stay in `jobseeker-core`. This crate speaks DTOs and the error envelope
//! from `docs/09-api.md`.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jobseeker_core::domain::capture::CaptureSubmission;
use jobseeker_core::ids::{JobId, TaskId};
use jobseeker_core::{Error, Result};
use jobseeker_db::queue::Queue;
use jobseeker_db::repo::job::{self, JobFilter, JobPatch};
use jobseeker_db::repo::score;
use jobseeker_pipeline::{IngestAccepted, Pipeline};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;

/// Shared by every handler.
#[derive(Clone)]
pub struct AppState {
    pub pipeline: Pipeline,
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
        get_task
    ),
    components(schemas(
        IngestUrlRequest,
        IngestPasteRequest,
        Accepted,
        MetaResponse,
        PatchJobRequest
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
        .route("/api/v1/tasks/{id}", get(get_task))
        .route("/openapi.json", get(openapi));

    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api)
        .fallback(spa_fallback)
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
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    tracing::info!(%bind, "jobseeker listening");
    axum::serve(listener, router(AppState { pipeline }))
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

async fn openapi() -> impl IntoResponse {
    Json(ApiDoc::openapi())
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
        router(AppState { pipeline: pipe })
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
        let app = router(AppState {
            pipeline: pipe.clone(),
        });
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

        let app = router(AppState {
            pipeline: pipe.clone(),
        });
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
    }
}
