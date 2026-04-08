//! Main handler — dispatches between WebSocket, NIP-11, and fallback.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::Nip34State;

/// Main handler for `GET /` and `POST /`.
///
/// Dispatches based on request headers:
/// 1. Accept: application/nostr+json → NIP-11 info
/// 2. Default → 200 with relay name
///
/// WebSocket upgrade is handled by a separate route.
pub async fn main_handler(State(state): State<Arc<Nip34State>>, headers: HeaderMap) -> Response {
    // NIP-11 info document
    if let Some(accept) = headers.get("accept") {
        if let Ok(accept_str) = accept.to_str() {
            if accept_str.contains("application/nostr+json") {
                return super::nip11::handle_nip11(state).into_response();
            }
        }
    }

    // Default: simple relay info
    (
        StatusCode::OK,
        format!(
            "{} — NIP-34 relay + GRASP git server",
            state.config.nip11.name
        ),
    )
        .into_response()
}
