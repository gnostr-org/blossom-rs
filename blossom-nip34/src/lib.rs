//! blossom-nip34 — NIP-34 Nostr relay + GRASP git server library.
//!
//! Provides a mountable axum [`Router`] that adds NIP-34 relay capabilities
//! and GRASP-compatible git HTTP smart protocol to any application.

pub mod config;
pub mod git_server;
pub mod nip34_types;
pub mod relay;
mod state;

pub use config::Nip34Config;
pub use state::Nip34State;

use std::sync::Arc;

/// Build an axum Router with NIP-34 relay + GRASP git server.
///
/// The returned router handles:
/// - `GET /` — NIP-11 relay info (with `Accept: application/nostr+json`) or default page
/// - `GET /ws` — WebSocket for Nostr relay protocol
/// - `GET /{npub}/{repo}.git/info/refs` — git smart HTTP
/// - `POST /{npub}/{repo}.git/git-upload-pack` — git fetch
/// - `POST /{npub}/{repo}.git/git-receive-pack` — git push
pub async fn build_nip34_router(
    config: Nip34Config,
) -> Result<axum::Router, Box<dyn std::error::Error>> {
    let state = Arc::new(Nip34State::new(config).await?);

    let app = axum::Router::new()
        .route(
            "/",
            axum::routing::get(relay::dispatch::main_handler).post(relay::dispatch::main_handler),
        )
        .route("/ws", axum::routing::get(relay::websocket::ws_handler))
        .merge(git_server::git_router())
        .with_state(state);

    Ok(app)
}
