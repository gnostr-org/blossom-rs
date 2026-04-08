//! WebSocket handler for Nostr relay connections.

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::Response;

use crate::Nip34State;

/// WebSocket upgrade handler. Mounted at `GET /ws` or merged into `/`.
pub async fn ws_handler(State(state): State<Arc<Nip34State>>, ws: WebSocketUpgrade) -> Response {
    let _relay = state.relay.clone();
    ws.on_upgrade(move |socket| async move {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        tracing::info!(%addr, "relay WebSocket connection");
        // TODO: bridge axum WebSocket messages ↔ relay.take_connection()
        // nostr-relay-builder expects AsyncRead+AsyncWrite (raw bytes),
        // axum gives Message-based WebSocket. Needs adapter.
        drop(socket);
    })
}
