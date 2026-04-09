//! Minimal Nostr relay WebSocket client for NIP-34 repo resolution.
//!
//! Queries a relay for kind:30617 (RepoAnnounce) events to find the
//! GRASP HTTP server URL for a given npub + repo.

use anyhow::{bail, Context, Result};
use serde_json::Value;

// ── Well-known public relays (fallback when none specified) ────────────────

/// Ordered list of well-known public relays tried as fallbacks when the
/// primary relay fails or is not specified.
pub const DEFAULT_RELAY_FALLBACKS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://relay.nostr.band",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://nostr.wine",
    "wss://relay.snort.social",
];

// ── Relay URL normalisation ────────────────────────────────────────────────

/// Normalise a relay host/URL string to a full `wss://` (or `ws://`) URL.
///
/// Handles all common variations:
/// - `relay.damus.io`         → `wss://relay.damus.io`
/// - `wss://relay.damus.io`   → unchanged
/// - `ws://localhost:7777`    → unchanged
/// - `https://relay.example`  → `wss://relay.example`  (scheme swap)
/// - `http://localhost:7777`  → `ws://localhost:7777`   (scheme swap)
pub fn normalize_relay_url(raw: &str) -> String {
    if raw.starts_with("wss://") || raw.starts_with("ws://") {
        raw.to_string()
    } else if let Some(host) = raw.strip_prefix("https://") {
        format!("wss://{host}")
    } else if let Some(host) = raw.strip_prefix("http://") {
        format!("ws://{host}")
    } else {
        // bare hostname / host:port
        format!("wss://{raw}")
    }
}

// ── Relay query ────────────────────────────────────────────────────────────

/// Query a single Nostr relay for the NIP-34 kind:30617 event.
/// Returns the GRASP clone/web URL if found.
pub fn resolve_grasp_url(relay_url: &str, pubkey_hex: &str, repo: &str) -> Result<String> {
    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    rt.block_on(async move { ws_query(relay_url, pubkey_hex, repo).await })
}

/// Try each relay in `relays` in order, returning the first successful
/// GRASP URL resolution.  Logs a warning for each failing relay.
pub fn resolve_grasp_url_with_fallbacks(
    relays: &[String],
    pubkey_hex: &str,
    repo: &str,
) -> Result<String> {
    let mut last_err = anyhow::anyhow!("no relays to try");

    for relay in relays {
        eprintln!("[nostr] trying relay {}…", relay);
        match resolve_grasp_url(relay, pubkey_hex, repo) {
            Ok(url) => {
                eprintln!("[nostr] resolved via {relay} → {url}");
                return Ok(url);
            }
            Err(e) => {
                eprintln!("[nostr] relay {relay} failed: {e:#}");
                last_err = e;
            }
        }
    }

    Err(last_err).context(format!(
        "no NIP-34 kind:30617 event found for repo '{}' on any relay ({} tried)",
        repo,
        relays.len()
    ))
}

/// Build the ordered relay list for a query: primary relay first, then
/// fallbacks (deduped).
pub fn build_relay_list(primary: Option<&str>) -> Vec<String> {
    let mut relays: Vec<String> = Vec::new();

    if let Some(p) = primary {
        let norm = normalize_relay_url(p);
        if !norm.is_empty() {
            relays.push(norm);
        }
    }

    for &r in DEFAULT_RELAY_FALLBACKS {
        let norm = normalize_relay_url(r);
        if !relays.contains(&norm) {
            relays.push(norm);
        }
    }

    relays
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
/// 1. `["clone", "<url>"]` — git clone URL (HTTP smart protocol)
/// 2. `["web", "<url>"]`   — canonical web view
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
/// its lowercase hex representation.  Also accepts a raw 64-char hex string.
pub fn npub_to_hex(npub: &str) -> Result<String> {
    if npub.starts_with("npub1") {
        let (_hrp, data) = bech32::decode(npub).context("bech32 decode npub")?;
        let bytes: Vec<u8> = data;
        if bytes.len() != 32 {
            bail!("npub decoded to {} bytes, expected 32", bytes.len());
        }
        Ok(hex::encode(&bytes))
    } else if npub.len() == 64 && npub.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(npub.to_lowercase())
    } else {
        bail!("expected npub1… bech32 or 64-char hex pubkey, got: {npub}");
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_relay_url ───────────────────────────────────────────────

    #[test]
    fn normalize_bare_hostname() {
        assert_eq!(normalize_relay_url("relay.damus.io"), "wss://relay.damus.io");
    }

    #[test]
    fn normalize_bare_host_port() {
        assert_eq!(normalize_relay_url("localhost:7777"), "wss://localhost:7777");
    }

    #[test]
    fn normalize_wss_passthrough() {
        assert_eq!(
            normalize_relay_url("wss://relay.nostr.band"),
            "wss://relay.nostr.band"
        );
    }

    #[test]
    fn normalize_ws_passthrough() {
        assert_eq!(normalize_relay_url("ws://localhost:7777"), "ws://localhost:7777");
    }

    #[test]
    fn normalize_https_to_wss() {
        assert_eq!(
            normalize_relay_url("https://relay.example.com"),
            "wss://relay.example.com"
        );
    }

    #[test]
    fn normalize_http_to_ws() {
        assert_eq!(
            normalize_relay_url("http://localhost:7777"),
            "ws://localhost:7777"
        );
    }

    // ── build_relay_list ──────────────────────────────────────────────────

    #[test]
    fn relay_list_primary_first() {
        let list = build_relay_list(Some("wss://my-relay.example.com"));
        assert_eq!(list[0], "wss://my-relay.example.com");
    }

    #[test]
    fn relay_list_no_duplicates() {
        // If primary is one of the defaults, it should only appear once
        let list = build_relay_list(Some("wss://relay.damus.io"));
        let damus_count = list.iter().filter(|r| r.as_str() == "wss://relay.damus.io").count();
        assert_eq!(damus_count, 1, "damus should appear exactly once: {list:?}");
    }

    #[test]
    fn relay_list_without_primary_uses_defaults() {
        let list = build_relay_list(None);
        assert!(!list.is_empty());
        assert_eq!(list[0], DEFAULT_RELAY_FALLBACKS[0]);
    }

    #[test]
    fn relay_list_normalises_primary() {
        let list = build_relay_list(Some("relay.damus.io")); // bare hostname
        assert_eq!(list[0], "wss://relay.damus.io");
    }

    // ── npub_to_hex ───────────────────────────────────────────────────────

    #[test]
    fn npub_bech32_roundtrip() {
        let npub = "npub1ahaz04ya9tehace3uy39hdhdryfvdkve9qdndkqp3tvehs6h8s5slq45hy";
        let hex = npub_to_hex(npub).expect("valid npub");
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn npub_hex_passthrough() {
        let hex = "ee".repeat(32); // 64 chars
        assert_eq!(npub_to_hex(&hex).unwrap(), hex);
    }

    #[test]
    fn npub_hex_uppercased_lowercased() {
        let upper = "EE".repeat(32);
        assert_eq!(npub_to_hex(&upper).unwrap(), "ee".repeat(32));
    }

    #[test]
    fn npub_invalid_rejected() {
        assert!(npub_to_hex("not-a-key").is_err());
        assert!(npub_to_hex("npub1bad!").is_err());
        assert!(npub_to_hex("").is_err());
    }
}
