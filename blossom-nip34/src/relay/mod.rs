//! Nostr relay — NIP-34 event storage and WebSocket protocol.

pub mod dispatch;
pub mod nip11;
pub mod plugins;
pub mod websocket;

use std::sync::Arc;

use nostr_database::NostrDatabase;
use nostr_relay_builder::{LocalRelay, RelayBuilder};

use crate::config::Nip34Config;

/// Build the Nostr relay with NIP-34 configuration.
pub async fn build_relay(
    config: &Nip34Config,
    database: Arc<dyn NostrDatabase>,
) -> Result<LocalRelay, Box<dyn std::error::Error>> {
    let builder = RelayBuilder::default().database(database);

    let relay = LocalRelay::new(builder);

    tracing::info!(
        relay.domain = %config.domain,
        "NIP-34 relay initialized"
    );

    Ok(relay)
}
