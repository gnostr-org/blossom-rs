//! Minimal Nostr relay WebSocket client for NIP-34 repo resolution.
//!
//! Queries a relay for kind:30617 (RepoAnnounce) events to find the
//! GRASP HTTP server URL for a given npub + repo.

use anyhow::{bail, Context, Result};
use serde_json::Value;

// ── Relay query ────────────────────────────────────────────────────────────

/// Query a Nostr relay for the NIP-34 kind:30617 event for `npub`/`repo`.
/// Returns the GRASP web URL extracted from the event `web` or `clone` tag.
pub fn resolve_grasp_url(relay_url: &str, pubkey_hex: &str, repo: &str) -> Result<String> {
    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    rt.block_on(async move { ws_query(relay_url, pubkey_hex, repo).await })
}

async fn ws_query(relay_url: &str, pubkey_hex: &str, repo: &str) -> Result<String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let (mut ws, _) = connect_async(relay_url)
        .await
        .with_context(|| format!("connect to relay {relay_url}"))?;

    // REQ filter: kind:30617, authored by pubkey, d-tag = repo
    let sub_id = "blossom-git-1";
    let filter = serde_json::json!({
        "kinds": [30617],
        "authors": [pubkey_hex],
        "#d": [repo]
    });
    let req_msg = serde_json::json!(["REQ", sub_id, filter]).to_string();
    ws.send(Message::Text(req_msg.into())).await.context("send REQ")?;

    // Read until we get an EVENT or EOSE
    let timeout = tokio::time::Duration::from_secs(10);
    let mut result: Option<String> = None;

    let _ = tokio::time::timeout(timeout, async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Text(text)) = msg else { continue };
            let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&text) else { continue };

            match arr.first().and_then(|v| v.as_str()) {
                Some("EVENT") => {
                    if let Some(event) = arr.get(2) {
                        if let Some(url) = extract_web_url(event) {
                            result = Some(url);
                            break;
                        }
                    }
                }
                Some("EOSE") => break,
                Some("NOTICE") => {
                    if let Some(msg) = arr.get(1).and_then(|v| v.as_str()) {
                        eprintln!("[nostr] relay notice: {msg}");
                    }
                }
                _ => {}
            }
        }
    })
    .await;

    let _ = ws.close(None).await;

    result.with_context(|| {
        format!(
            "no NIP-34 kind:30617 event found for {pubkey_hex:.8}…/{repo} on {relay_url}"
        )
    })
}

/// Extract a web/clone URL from a NIP-34 kind:30617 event's tags.
///
/// Tag priority:
/// 1. `["web", "<url>"]` — canonical web view
/// 2. `["clone", "<url>"]` — git clone URL (HTTP smart protocol)
fn extract_web_url(event: &Value) -> Option<String> {
    let tags = event["tags"].as_array()?;

    // Try "clone" first (direct git smart HTTP), then "web"
    for preferred in ["clone", "web"] {
        for tag in tags {
            let arr = tag.as_array()?;
            if arr.first().and_then(|v| v.as_str()) == Some(preferred) {
                if let Some(url) = arr.get(1).and_then(|v| v.as_str()) {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

// ── NIP-19 npub decoder ────────────────────────────────────────────────────

/// Decode an `npub1…` bech32 string to a 32-byte public key and return
/// its lowercase hex representation.
pub fn npub_to_hex(npub: &str) -> Result<String> {
    if npub.starts_with("npub1") {
        let (_hrp, data) = bech32::decode(npub).context("bech32 decode npub")?;
        let bytes: Vec<u8> = data;
        if bytes.len() != 32 {
            bail!("npub decoded to {} bytes, expected 32", bytes.len());
        }
        Ok(hex::encode(&bytes))
    } else if npub.len() == 64 && npub.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(npub.to_string())
    } else {
        bail!("expected npub1… bech32 or 64-char hex pubkey, got: {npub}");
    }
}
