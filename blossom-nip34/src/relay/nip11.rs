//! NIP-11 relay information document.

use std::sync::Arc;

use axum::http::header;
use axum::response::IntoResponse;

use crate::Nip34State;

/// Serve the NIP-11 relay information document.
pub fn handle_nip11(state: Arc<Nip34State>) -> impl IntoResponse {
    let info = &state.config.nip11;

    let doc = serde_json::json!({
        "name": info.name,
        "description": info.description,
        "contact": info.contact,
        "supported_nips": info.supported_nips,
        "software": "blossom-nip34",
        "version": env!("CARGO_PKG_VERSION"),
    });

    (
        [(header::CONTENT_TYPE, "application/nostr+json")],
        serde_json::to_string_pretty(&doc).unwrap_or_default(),
    )
}
