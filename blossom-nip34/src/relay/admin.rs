//! Relay admin HTTP endpoints for runtime policy management.
//!
//! All endpoints modify the in-memory policy and persist to the database.
//! Mounted at `/relay/admin/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::Nip34State;

/// Build the relay admin router.
pub fn relay_admin_router() -> axum::Router<Arc<Nip34State>> {
    axum::Router::new()
        .route("/relay/admin/policy", axum::routing::get(get_policy))
        .route(
            "/relay/admin/whitelist",
            axum::routing::get(get_whitelist)
                .put(add_whitelist)
                .delete(remove_whitelist),
        )
        .route(
            "/relay/admin/blacklist",
            axum::routing::get(get_blacklist)
                .put(add_blacklist)
                .delete(remove_blacklist),
        )
        .route(
            "/relay/admin/admins",
            axum::routing::get(get_admins).put(add_admin),
        )
}

#[derive(Deserialize)]
struct PubkeyRequest {
    pubkey: String,
}

/// GET /relay/admin/policy — current policy summary
async fn get_policy(State(state): State<Arc<Nip34State>>) -> impl IntoResponse {
    let policy = &state.policy;
    let admins: Vec<String> = policy.admins.read().unwrap().iter().cloned().collect();
    let whitelist: Vec<String> = policy.whitelist.read().unwrap().iter().cloned().collect();
    let blacklist: Vec<String> = policy.blacklist.read().unwrap().iter().cloned().collect();
    let allowed_kinds: Vec<u16> = policy
        .allowed_kinds
        .read()
        .unwrap()
        .iter()
        .map(|k| k.as_u16())
        .collect();
    let disallowed_kinds: Vec<u16> = policy
        .disallowed_kinds
        .read()
        .unwrap()
        .iter()
        .map(|k| k.as_u16())
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "admins": admins,
            "whitelist": whitelist,
            "blacklist": blacklist,
            "max_event_size": policy.max_event_size,
            "allowed_kinds": allowed_kinds,
            "disallowed_kinds": disallowed_kinds,
        })),
    )
}

/// GET /relay/admin/whitelist
async fn get_whitelist(State(state): State<Arc<Nip34State>>) -> impl IntoResponse {
    let list: Vec<String> = state
        .policy
        .whitelist
        .read()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    Json(serde_json::json!({ "whitelist": list }))
}

/// PUT /relay/admin/whitelist — add pubkey (persisted)
async fn add_whitelist(
    State(state): State<Arc<Nip34State>>,
    Json(req): Json<PubkeyRequest>,
) -> impl IntoResponse {
    state.policy.add_whitelist(&req.pubkey);
    let _ = state.policy_db.add("whitelist", &req.pubkey).await;
    tracing::info!(pubkey = %req.pubkey, "added to relay whitelist");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "added": req.pubkey })),
    )
}

/// DELETE /relay/admin/whitelist — remove pubkey (persisted)
async fn remove_whitelist(
    State(state): State<Arc<Nip34State>>,
    Json(req): Json<PubkeyRequest>,
) -> impl IntoResponse {
    state.policy.remove_whitelist(&req.pubkey);
    let _ = state.policy_db.remove("whitelist", &req.pubkey).await;
    tracing::info!(pubkey = %req.pubkey, "removed from relay whitelist");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": req.pubkey })),
    )
}

/// GET /relay/admin/blacklist
async fn get_blacklist(State(state): State<Arc<Nip34State>>) -> impl IntoResponse {
    let list: Vec<String> = state
        .policy
        .blacklist
        .read()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    Json(serde_json::json!({ "blacklist": list }))
}

/// PUT /relay/admin/blacklist — add pubkey (persisted)
async fn add_blacklist(
    State(state): State<Arc<Nip34State>>,
    Json(req): Json<PubkeyRequest>,
) -> impl IntoResponse {
    state.policy.add_blacklist(&req.pubkey);
    let _ = state.policy_db.add("blacklist", &req.pubkey).await;
    tracing::info!(pubkey = %req.pubkey, "added to relay blacklist");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "added": req.pubkey })),
    )
}

/// DELETE /relay/admin/blacklist — remove pubkey (persisted)
async fn remove_blacklist(
    State(state): State<Arc<Nip34State>>,
    Json(req): Json<PubkeyRequest>,
) -> impl IntoResponse {
    state.policy.remove_blacklist(&req.pubkey);
    let _ = state.policy_db.remove("blacklist", &req.pubkey).await;
    tracing::info!(pubkey = %req.pubkey, "removed from relay blacklist");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": req.pubkey })),
    )
}

/// GET /relay/admin/admins
async fn get_admins(State(state): State<Arc<Nip34State>>) -> impl IntoResponse {
    let list: Vec<String> = state
        .policy
        .admins
        .read()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    Json(serde_json::json!({ "admins": list }))
}

/// PUT /relay/admin/admins — add admin pubkey (persisted)
async fn add_admin(
    State(state): State<Arc<Nip34State>>,
    Json(req): Json<PubkeyRequest>,
) -> impl IntoResponse {
    state.policy.add_admin(&req.pubkey);
    let _ = state.policy_db.add("admin", &req.pubkey).await;
    tracing::info!(pubkey = %req.pubkey, "added relay admin");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "added": req.pubkey })),
    )
}
