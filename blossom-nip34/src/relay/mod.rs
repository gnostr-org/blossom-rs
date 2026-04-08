//! Nostr relay — NIP-34 event storage and WebSocket protocol.

pub mod admin;
pub mod dispatch;
pub mod nip11;
pub mod plugins;
pub mod policies;
pub mod policy_db;
pub mod websocket;

use std::sync::Arc;

use nostr_database::NostrDatabase;
use nostr_relay_builder::{LocalRelay, RelayBuilder};

use crate::config::Nip34Config;
use crate::relay::policies::SharedRelayPolicy;

/// Build the Nostr relay with the given shared policy.
pub async fn build_relay(
    config: &Nip34Config,
    database: Arc<dyn NostrDatabase>,
    policy: Arc<policies::RelayPolicy>,
) -> Result<LocalRelay, Box<dyn std::error::Error>> {
    let shared_policy = SharedRelayPolicy(policy);
    let mut builder = RelayBuilder::default()
        .database(database)
        .write_policy(shared_policy);

    if let Some(max) = config.max_connections {
        builder = builder.max_connections(max as usize);
    }

    let relay = LocalRelay::new(builder);

    tracing::info!(
        relay.domain = %config.domain,
        relay.admins = config.admin_pubkeys.len(),
        relay.whitelist = config.whitelist_pubkeys.len(),
        relay.blacklist = config.blacklist_pubkeys.len(),
        "NIP-34 relay initialized"
    );

    Ok(relay)
}
