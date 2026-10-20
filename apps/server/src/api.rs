//! The axum router: the REST surface, the SSE stream, and the embedded SPA.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::HeaderMap;
use axum::http::{Method, StatusCode, Uri};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hc_core::write::{GuardedWriter, WriteError};

use crate::auth::TOKEN_HEADER;
use crate::config::ConfigView;
use crate::onboarding::{self, MAX_PROFILE_UPLOAD_BYTES, OnboardingError};
use crate::state::AppState;
use crate::types::{
    ApiError, CommitRequest, DeviceSummary, EntityDescriptor, Health, ManualHostRequest,
    OnboardingResult, ProfilePreview, ProgramDescriptor, RestartOutcome, SessionView, WriteOutcome,
    WriteRequest,
};
use crate::write;

/// The prefix every REST and SSE route lives under. The SPA is served from everything else, so the
/// two never collide.
pub const API_PREFIX: &str = "/api";

/// The daemon's name, as the UI shows it. The Cargo package is `server`; the binary is `kessel`.
pub const DAEMON_NAME: &str = "kessel";

/// Builds the whole application.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/devices", get(devices))
        .route("/api/devices/{slug}/entities", get(entities))
        .route("/api/devices/{slug}/programs", get(programs))
        .route("/api/devices/{slug}/write", post(write_feature))
        .route("/api/events", get(crate::sse::events))
        .route("/api/config", get(config).put(save_config))
        .route("/api/restart", post(restart))
        .route(
            "/api/session",
            get(session).post(open_session).delete(close_session),
        )
        .route("/api/devices/{slug}", axum::routing::delete(remove_device))
        .route(
            "/api/onboarding/preview",
            post(preview_profile).layer(DefaultBodyLimit::max(MAX_PROFILE_UPLOAD_BYTES)),
        )
        .route(
            "/api/onboarding/commit",
            post(commit_profile).layer(DefaultBodyLimit::max(MAX_PROFILE_UPLOAD_BYTES)),
        )
        .route("/api/onboarding/{slug}/host", post(onboard_manual_host))
        // Everything that is not an API route is the SPA.
        .fallback(spa_fallback)
        // The gate wraps the WHOLE router, fallback included, so a route added later is gated by
        // default rather than by someone remembering to gate it.
        .layer(from_fn_with_state(state.clone(), gate_mutations))
        .with_state(state)
}

/// `GET /api/health` — liveness plus the device count the dashboard header shows.
async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        name: DAEMON_NAME.to_owned(),
        version: crate::version().to_owned(),
        device_count: u32::try_from(state.devices().len()).unwrap_or(u32::MAX),
    })
}

/// `GET /api/devices` — every onboarded device, in slug order.
async fn devices(State(state): State<AppState>) -> Json<Vec<DeviceSummary>> {
    Json(state.devices().summaries())
}

/// `GET /api/devices/{slug}/entities` — one descriptor per mapped feature, each with its current value
/// when the device has a live task.
///
/// An unknown slug is a 404 rather than an empty list, so the UI can tell "no such device" from "a
/// device with no features". This is the snapshot; later value changes arrive on the SSE stream
/// (`SseEventKind::Value`).
async fn entities(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<EntityDescriptor>>, ApiFailure> {
    let entry = state
        .devices()
        .entry(&slug)
        .ok_or_else(|| ApiFailure::not_found("no such device"))?;
    // The shadow carries the values. Absent means nothing is connected, which the descriptors render
    // as an absent value rather than an empty one.
    let shadow = state.shadows().shadow(&slug);
    Ok(Json(entry.entity_descriptors(shadow.as_deref())))
}

/// `GET /api/devices/{slug}/programs` — the appliance's programmes, named.
///
/// A read only; there is no route that starts a programme. An unknown slug is a 404, matching
/// [`entities`]; a known device with no declared programmes is an empty list.
async fn programs(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Json<Vec<ProgramDescriptor>>, ApiFailure> {
    let entry = state
        .devices()
        .entry(&slug)
        .ok_or_else(|| ApiFailure::not_found("no such device"))?;
    Ok(Json(entry.program_descriptors()))
}

/// `GET /api/config` — what is stored, and what the environment is overriding.
///
/// Never carries the broker password; `mqtt_password_set` reports whether one exists.
async fn config(State(state): State<AppState>) -> Result<Json<ConfigView>, ApiFailure> {
    let stored = crate::config::load(state.config_path()).map_err(|error| {
        // The path is a container path, not an identifier, so it is safe to name.
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the stored configuration could not be read: {error}"),
        )
    })?;
    Ok(Json(
        crate::config::view(&stored, |key| std::env::var(key).ok())
            .with_status(state.mqtt_status()),
    ))
}

/// `PUT /api/config` — store a new configuration.
///
/// Nothing is applied live: the response's `restart_required` is always true.
async fn save_config(
    State(state): State<AppState>,
    Json(update): Json<crate::config::ConfigUpdate>,
) -> Result<Json<ConfigView>, ApiFailure> {
    // The state's path, not an ambient read: where a broker credential is stored is decided by
    // whoever built the state.
    let path = state.config_path();
    let current = crate::config::load(path).map_err(|error| {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the stored configuration could not be read: {error}"),
        )
    })?;
    // `apply` is what carries the stored password forward when the update omits it.
    let next = update.apply(&current);
    crate::config::save(path, &next).map_err(|error| {
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the configuration could not be saved: {error}"),
        )
    })?;
    tracing::info!(
        mqtt_configured = next.mqtt.is_some(),
        // Never the host or the credential: operators paste this line into issues.
        "the stored configuration was replaced; a restart is required for it to take effect"
    );
    // The live status is the one resolved at startup; it differs from what is stored until restart.
    Ok(Json(
        crate::config::view(&next, |key| std::env::var(key).ok()).with_status(state.mqtt_status()),
    ))
}

/// How this daemon is going to stop so that something starts it again.
///
/// The two mechanisms are not equivalent, so the UI says which one is about to happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartMechanism {
    /// `POST /addons/self/restart`: the Supervisor stops the container and starts it again.
    Supervisor,
    /// The process exits 0.
    ///
    /// Whether anything restarts it depends on the supervisor; under `docker run` with no restart
    /// policy nothing does, which is why the response names the mechanism.
    ProcessExit,
}

/// Which mechanism is available: whether there is a Supervisor to ask.
///
/// Takes a lookup rather than reading the environment so tests need no `set_var`, which is `unsafe`
/// in edition 2024.
#[must_use]
pub fn restart_mechanism(lookup: impl Fn(&str) -> Option<String>) -> RestartMechanism {
    if lookup(crate::hassio::TOKEN_ENV).is_some_and(|token| !token.trim().is_empty()) {
        RestartMechanism::Supervisor
    } else {
        RestartMechanism::ProcessExit
    }
}

/// How long to wait before acting, so the response is on the wire first. Too short shows the browser
/// a network error instead of "restarting"; longer costs nothing.
const RESTART_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// `POST /api/restart` — restart the daemon so a saved setting takes effect.
///
/// It answers 202 before it acts; the restart happens on a detached task after [`RESTART_GRACE`].
/// Nothing is flushed or drained first.
async fn restart(State(state): State<AppState>) -> (StatusCode, Json<RestartOutcome>) {
    let mechanism = restart_mechanism(|key| std::env::var(key).ok());
    let _ = &state;
    tracing::warn!(
        ?mechanism,
        "a restart was requested from the API; answering first, then acting"
    );
    tokio::spawn(async move {
        tokio::time::sleep(RESTART_GRACE).await;
        perform_restart(mechanism).await;
    });
    (
        StatusCode::ACCEPTED,
        Json(RestartOutcome {
            mechanism: match mechanism {
                RestartMechanism::Supervisor => "supervisor",
                RestartMechanism::ProcessExit => "process_exit",
            }
            .to_owned(),
            message: match mechanism {
                RestartMechanism::Supervisor => {
                    "Home Assistant is restarting the add-on.".to_owned()
                }
                RestartMechanism::ProcessExit => {
                    "Kessel is exiting. It comes back only if whatever started it restarts it."
                        .to_owned()
                }
            },
        }),
    )
}

/// Performs the restart. Only called from a detached task, after the response has gone.
///
/// A failed Supervisor restart does not fall through to exiting: it usually means the add-on lacks
/// `hassio_api: true`, and exiting would turn that into a stopped add-on. The UI notices via
/// `GET /api/health`.
async fn perform_restart(mechanism: RestartMechanism) {
    match mechanism {
        RestartMechanism::Supervisor => match crate::hassio::restart_self().await {
            // Not reached in practice: the container is gone before the answer arrives.
            Ok(()) => tracing::info!("the Supervisor accepted the restart"),
            Err(error) => tracing::error!(
                %error,
                "the Supervisor refused to restart the add-on, so it is still running"
            ),
        },
        RestartMechanism::ProcessExit => {
            tracing::warn!("exiting on request; whatever started this process must start it again");
            std::process::exit(0);
        }
    }
}

/// A failure with its status, rendered as [`ApiError`] JSON.
///
/// The message is operator-facing prose. The slug is safe to include; appliance identifiers and keys
/// never are.
#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    message: String,
}

impl ApiFailure {
    /// A 404.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    /// A 400.
    #[must_use]
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// A 401.
    #[must_use]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    /// A 409 — the request is well formed but conflicts with something already there.
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    /// A 500 — our side failed, not the caller's.
    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Any status, with a message.
    #[must_use]
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ApiError {
                message: self.message,
            }),
        )
            .into_response()
    }
}

/// `GET /api/session` — whether this request is already authenticated, so the SPA knows whether to ask
/// for the token.
///
/// A read, so it answers anyone, but it reports only whether the caller's own cookie is valid, never
/// how many sessions exist.
async fn session(State(state): State<AppState>, headers: HeaderMap) -> Json<SessionView> {
    let presented = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::session_from_cookies)
        .unwrap_or_default();
    Json(SessionView {
        // The field means "may this caller mutate", so an ungated daemon answers yes.
        authenticated: !state.auth_required() || state.sessions().matches(presented),
        auth_required: state.auth_required(),
    })
}

/// `POST /api/session` — exchange the operator token for a session cookie.
///
/// [`gate_mutations`] has already required the token before this runs, so nothing is re-checked here.
/// The answer is `204`: returning the session in a body a script can read would undo `HttpOnly`.
async fn open_session(State(state): State<AppState>) -> Response {
    let session = state.sessions().open();
    tracing::info!(
        open = state.sessions().len(),
        max_age_s = crate::auth::SESSION_MAX_AGE_SECONDS,
        "opened a browser session; the operator token is no longer needed in this browser"
    );
    // `session` is moved into the header value and not logged anywhere: it is a live credential.
    match crate::auth::session_cookie(&session).parse() {
        Ok(cookie) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert("set-cookie", cookie);
            response
        }
        // Unreachable: the value is hex plus fixed ASCII attributes. Reported rather than `expect`ed
        // because library code does not panic.
        Err(_error) => ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the session cookie could not be encoded",
        )
        .into_response(),
    }
}

/// `DELETE /api/session` — end this browser's session.
///
/// An unknown session still answers `204` and clears the cookie: logging out is idempotent, and
/// distinguishing the cases would tell a caller whether a guessed value was live.
async fn close_session(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let presented = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::session_from_cookies)
        .unwrap_or_default();
    let revoked = state.sessions().revoke(presented);
    tracing::info!(
        revoked,
        open = state.sessions().len(),
        "closed a browser session"
    );
    match crate::auth::cleared_session_cookie().parse() {
        Ok(cookie) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert("set-cookie", cookie);
            response
        }
        Err(_error) => ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the session cookie could not be encoded",
        )
        .into_response(),
    }
}

/// Whether `method` mutates state, and therefore needs the token.
///
/// Keyed on the method rather than a route list: `GET`, `HEAD` and `OPTIONS` are safe (RFC 9110
/// §9.2.1), and every other method is gated, including ones no route uses today.
#[must_use]
pub const fn is_mutating(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The authentication gate. Mutating requests must present [`TOKEN_HEADER`] **or** a valid session
/// cookie; reads pass untouched.
///
/// `AppState::auth_required` switches the gate; it is off by default in the add-on, where Home
/// Assistant's ingress has already authenticated the browser (see
/// `crate::config::effective_require_auth`). Off, every request goes straight through.
///
/// The header carries the long-lived operator token, for `curl` and scripts. The cookie carries a
/// session secret exchanged for the token at `POST /api/session` (`HttpOnly`, `SameSite=Strict`). The
/// header is checked first, so a stale cookie alongside a valid token cannot cause a refusal.
async fn gate_mutations(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if !state.auth_required() || !is_mutating(request.method()) {
        return next.run(request).await;
    }
    let presented = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if state.auth().matches(presented) {
        return next.run(request).await;
    }
    let cookie_session = request
        .headers()
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(crate::auth::session_from_cookies)
        .unwrap_or_default();
    if state.sessions().matches(cookie_session) {
        return next.run(request).await;
    }
    // Never echo what was presented, and never say which of "absent" or "wrong" it was.
    tracing::warn!(
        method = %request.method(),
        "refused a mutating request: the API token is missing or does not match"
    );
    ApiFailure::unauthorized(format!(
        "mutating requests must present the API token in the `{TOKEN_HEADER}` header"
    ))
    .into_response()
}

/// `POST /api/devices/{slug}/write` — write one feature, **through the guard**.
///
/// Every refusal path returns a [`WriteOutcome::Refused`] body so the UI can say why.
async fn write_feature(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(request): Json<WriteRequest>,
) -> Result<(StatusCode, Json<WriteOutcome>), ApiFailure> {
    let entry = state
        .devices()
        .entry(&slug)
        .ok_or_else(|| ApiFailure::not_found("no such device"))?;
    let entity = entry
        .model
        .entity_by_key(&request.feature_key)
        .ok_or_else(|| ApiFailure::not_found("no such feature on this device"))?;
    let Some(set_topic) = entry
        .mapping
        .entries()
        .iter()
        .find(|mapped| mapped.uid == entity.uid)
        .and_then(|mapped| mapped.command_topic.clone())
    else {
        // A `set` topic exists iff the feature is writable, so this is a refusal, not a 404.
        return Ok(refused("this feature is not writable"));
    };

    let command = hc_core::bridge::build_write_command(
        &entry.model,
        &entry.mapping,
        &set_topic,
        &request.value,
    )
    .map_err(|error| ApiFailure::bad_request(error.to_string()))?;

    // Both cases go through a `GuardedWriter`; they differ only in what is behind the guard:
    //
    // 1. a live session — the device's writer, installed at CONNECTED, using that connection's own
    //    `msgID` allocator;
    // 2. no live session — a refusing transport, so the write fails closed. Nothing is queued for a
    //    disconnected appliance.
    let verdict = match state.writers().writer(&slug) {
        Some(shared) => shared.lock().await.write(&command).await,
        None => {
            GuardedWriter::new(write::no_live_session_transport())
                .write(&command)
                .await
        }
    };
    // The feature key and slug are safe to log; appliance identifiers never are.
    tracing::info!(
        device = %slug,
        feature = %request.feature_key,
        allowed = verdict.is_ok(),
        "write decision at the guard"
    );
    match verdict {
        Ok(()) => Ok((StatusCode::OK, Json(WriteOutcome::Accepted))),
        Err(WriteError::Transport(reason)) => Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(WriteOutcome::Refused { reason }),
        )),
        Err(error) => Ok(refused(error.to_string())),
    }
}

/// A 403 with a [`WriteOutcome::Refused`] body.
fn refused(reason: impl Into<String>) -> (StatusCode, Json<WriteOutcome>) {
    (
        StatusCode::FORBIDDEN,
        Json(WriteOutcome::Refused {
            reason: reason.into(),
        }),
    )
}

/// `POST /api/onboarding/preview` — phase 1: parse the pasted ZIP and report, dialling nothing.
///
/// Raw bytes rather than multipart: the SPA posts with `fetch` (the token gate needs a custom header
/// a `<form>` cannot set), so multipart would only add a boundary to parse.
async fn preview_profile(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ProfilePreview>, ApiFailure> {
    onboarding::preview(&state, &body)
        .map(Json)
        .map_err(ApiFailure::from)
}

/// `POST /api/onboarding/commit?slug=&host=&port=` — phase 2: the operator's confirmed choices.
///
/// The ZIP is the body and the choices are the query string. The archive is re-posted rather than
/// staged server-side, because staging would need an expiry that a long-open dialog could hit.
async fn commit_profile(
    State(state): State<AppState>,
    Query(request): Query<CommitRequest>,
    body: Bytes,
) -> Result<Json<OnboardingResult>, ApiFailure> {
    onboarding::commit(&state, &body, &request)
        .await
        .map(Json)
        .map_err(ApiFailure::from)
}

/// `DELETE /api/devices/{slug}` — forget a device, on disk as well as in memory.
///
/// A failed connection test still persists the device, so this is how an operator removes an entry
/// with a wrong address.
async fn remove_device(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiFailure> {
    onboarding::delete_device(&state, &slug)
        .map(|()| StatusCode::OK)
        .map_err(ApiFailure::from)
}

/// `POST /api/onboarding/{slug}/host` — retry onboarding with a manually entered address.
async fn onboard_manual_host(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(request): Json<ManualHostRequest>,
) -> Result<Json<OnboardingResult>, ApiFailure> {
    onboarding::retry_with_manual_host(&state, &slug, request.host)
        .await
        .map(Json)
        .map_err(ApiFailure::from)
}

impl From<OnboardingError> for ApiFailure {
    fn from(error: OnboardingError) -> Self {
        match error {
            OnboardingError::Parse(_) | OnboardingError::InvalidSlug(_) => {
                Self::bad_request(error.to_string())
            }
            OnboardingError::UnknownDevice => Self::not_found(error.to_string()),
            // 409 for both; separate variants because the operator's next step differs (pick another
            // name, or restart the daemon).
            OnboardingError::SlugTaken(_) | OnboardingError::DeviceBusy(_) => {
                Self::conflict(error.to_string())
            }
            // 500: the device IS onboarded and working; what failed is our side of persisting it.
            OnboardingError::Storage(_) => Self::internal(error.to_string()),
        }
    }
}

/// The SPA, served out of the binary.
///
/// A path matching an embedded file is served as that file; anything else gets `index.html` with
/// status 200. An unmatched path under [`API_PREFIX`] is a 404 instead, so a typo in an API call does
/// not look like a working endpoint.
async fn spa_fallback(uri: Uri, headers: HeaderMap) -> Response {
    let requested = uri.path();
    let path = requested.trim_start_matches('/');
    if path == API_PREFIX.trim_start_matches('/') || path.starts_with("api/") {
        return ApiFailure::not_found("no such API route").into_response();
    }
    // An asset is a file, not a directory: it is served at its exact path, never redirected.
    if let Some(asset) = crate::spa::serve(path) {
        return asset;
    }
    // The shell is only ever served from a directory URL, so relative assets resolve under ingress.
    let ingress_prefix = headers
        .get(crate::spa::INGRESS_PATH_HEADER)
        .and_then(|value| value.to_str().ok());
    if let Some(location) = crate::spa::directory_redirect(requested, ingress_prefix) {
        return (
            StatusCode::PERMANENT_REDIRECT,
            [(axum::http::header::LOCATION, location)],
        )
            .into_response();
    }
    crate::spa::serve(crate::spa::SHELL).unwrap_or_else(|| {
        // Only reachable if the binary carries no shell at all, which `build.rs` prevents.
        ApiFailure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "this binary carries no SPA; it was built without apps/web/build/",
        )
        .into_response()
    })
}
