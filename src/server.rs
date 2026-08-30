use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};

use crate::engine::StateEngine;
use crate::error::TaprootError;
use crate::fabric::{AuditEntry, Fabric};
use crate::registry::Registry;
use crate::state::SignedState;

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub fabric: Arc<Fabric>,
    pub registry_root: PathBuf,
}

/// POST /v1/states — body is SignedState JSON
pub async fn push_state(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(signed): Json<SignedState>,
) -> impl IntoResponse {
    // auth check if tokens configured
    let actor = match check_auth(&app.fabric, &headers) {
        Ok(a) => a,
        Err(e) => return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e}))).into_response(),
    };

    // verify hash + signature
    if let Err(e) = StateEngine::verify(&signed) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("verify failed: {e}")})),
        )
            .into_response();
    }
    let computed = match StateEngine::hash(&signed.state) {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    };
    if computed != signed.hash {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("hash mismatch expected {} got {}", signed.hash, computed)})),
        )
            .into_response();
    }

    // policy check: require signed
    let policy = app.fabric.get_policy(&signed.state.base.repo).unwrap_or_default();
    if policy.require_signed && signed.signature.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "policy requires signed state"})),
        )
            .into_response();
    }

    // push to registry
    match app.registry.push(&signed) {
        Ok(hash) => {
            let _ = app.fabric.audit(AuditEntry {
                ts: chrono::Utc::now(),
                action: "push".into(),
                repo: signed.state.base.repo.clone(),
                branch: signed.state.base.branch.clone(),
                hash: hash.clone(),
                actor: actor.unwrap_or_else(|| "local".into()),
                signed: signed.signature.is_some(),
            });
            (StatusCode::OK, Json(serde_json::json!({"hash": hash}))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /v1/states/:hash
pub async fn get_state(
    State(app): State<Arc<AppState>>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    match app.registry.pull(&hash) {
        Ok(signed) => (StatusCode::OK, Json(signed)).into_response(),
        Err(TaprootError::ObjectNotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /v1/refs/:repo/:branch
pub async fn get_ref(
    State(app): State<Arc<AppState>>,
    Path((repo, branch)): Path<(String, String)>,
) -> impl IntoResponse {
    let repo = crate::registry::desanitize(&repo);
    let branch = crate::registry::desanitize(&branch);
    match app.registry.resolve_ref(&repo, &branch) {
        Ok(Some(hash)) => (StatusCode::OK, Json(serde_json::json!({"repo": repo, "branch": branch, "hash": hash}))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "ref not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /v1/audit?repo=myapp
pub async fn get_audit(
    State(app): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let repo = q.get("repo").map(|s| s.as_str());
    match app.fabric.audit_log(repo) {
        Ok(entries) => (StatusCode::OK, Json(entries)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// GET /v1/policy/:repo
pub async fn get_policy(
    State(app): State<Arc<AppState>>,
    Path(repo): Path<String>,
) -> impl IntoResponse {
    let repo = crate::registry::desanitize(&repo);
    match app.fabric.get_policy(&repo) {
        Ok(p) => (StatusCode::OK, Json(p)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /v1/policy/:repo
#[derive(Debug, Deserialize)]
pub struct SetPolicyReq {
    pub require_signed: Option<bool>,
    pub require_check_strict: Option<bool>,
    pub allowed_branches: Option<Vec<String>>,
    pub blocked_env_keys: Option<Vec<String>>,
}

pub async fn set_policy(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(repo): Path<String>,
    Json(req): Json<SetPolicyReq>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&app.fabric, &headers) {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": e}))).into_response();
    }
    let repo = crate::registry::desanitize(&repo);
    let mut policy = app.fabric.get_policy(&repo).unwrap_or_default();
    policy.repo = repo.clone();
    if let Some(v) = req.require_signed {
        policy.require_signed = v;
    }
    if let Some(v) = req.require_check_strict {
        policy.require_check_strict = v;
    }
    if let Some(v) = req.allowed_branches {
        policy.allowed_branches = v;
    }
    if let Some(v) = req.blocked_env_keys {
        policy.blocked_env_keys = v;
    }
    match app.fabric.set_policy(&policy) {
        Ok(()) => (StatusCode::OK, Json(policy)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /v1/check — drift check between two hashes or states
#[derive(Debug, Deserialize)]
pub struct CheckReq {
    pub baseline_hash: String,
    pub current_hash: String,
    #[serde(default = "default_true")]
    pub strict: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct CheckResp {
    pub drifted: bool,
    pub has_breaking: bool,
    pub diffs: Vec<crate::diff::FieldDiff>,
    pub warnings: Vec<String>,
}

pub async fn check(
    State(app): State<Arc<AppState>>,
    Json(req): Json<CheckReq>,
) -> impl IntoResponse {
    let baseline = match app.registry.pull(&req.baseline_hash) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("baseline not found: {e}")})),
            )
                .into_response()
        }
    };
    let current = match app.registry.pull(&req.current_hash) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("current not found: {e}")})),
            )
                .into_response()
        }
    };

    let mut warnings = Vec::new();
    if baseline.signature.is_none() {
        warnings.push("baseline is unsigned".to_string());
    }
    if current.signature.is_none() {
        warnings.push("current is unsigned".to_string());
    }
    let diffs = crate::diff::diff_states(&baseline.state, &current.state, req.strict);
    let has_breaking = crate::diff::has_breaking(&diffs) || !warnings.is_empty();
    let drifted = !diffs.is_empty() || !warnings.is_empty();

    let resp = CheckResp {
        drifted,
        has_breaking,
        diffs,
        warnings,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

fn check_auth(fabric: &Fabric, headers: &HeaderMap) -> Result<Option<String>, String> {
    let tokens = fabric.tokens().map_err(|e| e.to_string())?;
    if tokens.is_empty() {
        // open registry — no auth required
        return Ok(None);
    }
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
    if token.is_empty() {
        return Err("missing Authorization Bearer token".into());
    }
    match fabric.verify_token(token) {
        Ok(Some(actor)) => Ok(Some(actor)),
        Ok(None) => Err("invalid token".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/states", post(push_state))
        .route("/v1/states/:hash", get(get_state))
        .route("/v1/refs/:repo/:branch", get(get_ref))
        .route("/v1/audit", get(get_audit))
        .route("/v1/policy/:repo", get(get_policy).post(set_policy))
        .route("/v1/check", post(check))
        .route("/health", get(|| async { Json(serde_json::json!({"status":"ok"})) }))
        .with_state(state)
}

pub async fn serve(registry_root: PathBuf, fabric_root: PathBuf, addr: String) -> Result<(), TaprootError> {
    let registry = Arc::new(Registry::init(&registry_root)?);
    let fabric = Arc::new(Fabric::init(&fabric_root, &registry_root)?);
    let state = Arc::new(AppState {
        registry,
        fabric,
        registry_root,
    });
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| TaprootError::Io(e))?;
    tracing::info!(%addr, "taproot registry API listening");
    axum::serve(listener, app)
        .await
        .map_err(|e| TaprootError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}
