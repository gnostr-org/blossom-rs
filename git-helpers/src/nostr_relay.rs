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

/// Extract a GRASP clone/web URL from a NIP-34 kind:30617 event's tags.
///
/// Tag priority:
/// 1. `["clone", "<url>", ...]` — first HTTPS URL in a clone tag
/// 2. `["web",   "<url>"]`     — fallback if no clone tag present
///
/// The `clone` tag may list multiple URLs (all in the same tag array).  We
/// return the first one that starts with `https://` or `http://`.
fn extract_web_url(event: &Value) -> Option<String> {
    let tags = event["tags"].as_array()?;

    // Try all clone tags first — pick first HTTP(S) value in each
    for tag in tags {
        let arr = tag.as_array()?;
        if arr.first().and_then(|v| v.as_str()) != Some("clone") {
            continue;
        }
        for val in arr.iter().skip(1) {
            if let Some(url) = val.as_str() {
                if url.starts_with("https://") || url.starts_with("http://") {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Fall back to first web tag
    for tag in tags {
        let arr = tag.as_array()?;
        if arr.first().and_then(|v| v.as_str()) == Some("web") {
            if let Some(url) = arr.get(1).and_then(|v| v.as_str()) {
                if url.starts_with("https://") || url.starts_with("http://") {
                    return Some(url.to_string());
                }
            }
        }
    }

    None
}

/// Extract relay URLs from the `relays` tag of a kind:30617 event.
///
/// These can supplement the query relay list for subsequent fallback attempts.
/// Trailing slashes are stripped and URLs are normalised.
pub fn extract_event_relays(event: &Value) -> Vec<String> {
    let Some(tags) = event["tags"].as_array() else {
        return vec![];
    };

    for tag in tags {
        let Some(arr) = tag.as_array() else { continue };
        if arr.first().and_then(|v| v.as_str()) != Some("relays") {
            continue;
        }
        return arr
            .iter()
            .skip(1)
            .filter_map(|v| v.as_str())
            .map(|r| normalize_relay_url(r.trim_end_matches('/')))
            .collect();
    }
    vec![]
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

    // ── Real event fixtures (from `gnostr query --kinds 30617 --limit 10`) ─

    /// YouBlossom — clone tag has TWO URLs; web tag is a subdomain.
    fn event_youblossom() -> Value {
        serde_json::json!({
            "kind": 30617,
            "pubkey": "5c1eeccff05aa3ff47bc56fa80bc5c254a8eb67c3a8be2d29bf9b142aa57a7da",
            "tags": [
                ["d", "YouBlossom"],
                ["name", "YouBlossom"],
                ["clone",
                    "https://git.shakespeare.diy/npub1ts0wenlst23l73au2magp0zuy49gadnu8297955mlxc592jh5ldq0xzwcx/YouBlossom.git",
                    "https://relay.ngit.dev/npub1ts0wenlst23l73au2magp0zuy49gadnu8297955mlxc592jh5ldq0xzwcx/YouBlossom.git"
                ],
                ["relays",
                    "wss://git.shakespeare.diy/",
                    "wss://relay.ngit.dev/",
                    "wss://nos.lol/",
                    "wss://relay.damus.io/",
                    "wss://relay.primal.net/"
                ],
                ["web", "https://YouBlossom.shakespeare.wtf"],
                ["web", "https://YouBlossom.shakespeare.wtf"],
                ["alt", "git repository: YouBlossom"]
            ]
        })
    }

    /// fresh-repo — single clone URL; relay list is a single relay.
    fn event_fresh_repo() -> Value {
        serde_json::json!({
            "kind": 30617,
            "pubkey": "b1576eb99a4774158a32fc5e190afa3ded4da19f51fbfa0b1a1bf6421ea5733a",
            "tags": [
                ["d", "fresh-repo"],
                ["name", "fresh-repo"],
                ["clone", "https://blossom.gnostr.cloud/npub1k9tkawv6ga6ptz3jl30pjzh68hk5mgvl28al5zc6r0myy849wvaq38a70g/fresh-repo.git"],
                ["web", "https://gitworkshop.dev/repo/fresh-repo"],
                ["relays", "wss://blossom.gnostr.cloud"],
                ["alt", "git repository: fresh-repo"]
            ]
        })
    }

    /// jmp — two clone URLs; relays with no trailing slashes.
    fn event_jmp() -> Value {
        serde_json::json!({
            "kind": 30617,
            "pubkey": "7459d333af66066f066cf87796e690db3a96ff4534f9edf4eab74df2f207289b",
            "tags": [
                ["d", "jmp"],
                ["name", "jmp"],
                ["description", "JoinMarket Protocol (JMP) Specifications"],
                ["clone",
                    "https://relay.ngit.dev/npub1w3vaxva0vcrx7pnvlpmede5smvafdl69xnu7ma82kaxl9us89zdsht4c5c/jmp.git",
                    "https://gitnostr.com/npub1w3vaxva0vcrx7pnvlpmede5smvafdl69xnu7ma82kaxl9us89zdsht4c5c/jmp.git"
                ],
                ["web", "https://gitworkshop.dev/npub1w3vaxva0vcrx7pnvlpmede5smvafdl69xnu7ma82kaxl9us89zdsht4c5c/relay.ngit.dev/jmp"],
                ["relays", "wss://relay.ngit.dev", "wss://gitnostr.com"]
            ]
        })
    }

    /// Beer — NO clone tag, only web tags; relays have trailing slashes.
    fn event_beer() -> Value {
        serde_json::json!({
            "kind": 30617,
            "pubkey": "c62ea154ea5352df528b9bb79fdfcd0432636371098d4336943ace394a70b555",
            "tags": [
                ["d", "Beer"],
                ["name", "iDrink"],
                ["relays",
                    "wss://git.shakespeare.diy/",
                    "wss://relay.ngit.dev/",
                    "wss://relay.primal.net/",
                    "wss://relay.damus.io/",
                    "wss://relay.westernbtc.com/"
                ],
                ["web", "https://iBeer.shakespeare.wtf"],
                ["alt", "git repository: Beer"]
            ]
        })
    }

    /// satshoot — two clone URLs from different GRASP servers.
    fn event_satshoot() -> Value {
        serde_json::json!({
            "kind": 30617,
            "pubkey": "d04ecf33a303a59852fdb681ed8b412201ba85d8d2199aec73cb62681d62aa90",
            "tags": [
                ["d", "satshoot"],
                ["name", "satshoot"],
                ["clone",
                    "https://grasp.budabit.club/npub16p8v7varqwjes5hak6q7mz6pygqm4pwc6gve4mrned3xs8tz42gq7kfhdw/satshoot.git",
                    "https://gitnostr.com/npub16p8v7varqwjes5hak6q7mz6pygqm4pwc6gve4mrned3xs8tz42gq7kfhdw/satshoot.git"
                ],
                ["web", "https://gitnostr.com/npub16p8v7varqwjes5hak6q7mz6pygqm4pwc6gve4mrned3xs8tz42gq7kfhdw/satshoot"],
                ["relays", "wss://gitnostr.com", "wss://relay.primal.net", "wss://nos.lol", "wss://relay.damus.io", "wss://grasp.budabit.club"]
            ]
        })
    }

    /// Minimal event — no clone, no web, no relays tags.
    fn event_empty() -> Value {
        serde_json::json!({
            "kind": 30617,
            "tags": [["d", "empty-repo"], ["name", "empty"]]
        })
    }

    // ── extract_web_url ───────────────────────────────────────────────────

    #[test]
    fn multi_clone_returns_first_url() {
        // YouBlossom has two URLs in the clone tag — first wins
        let url = extract_web_url(&event_youblossom()).unwrap();
        assert_eq!(
            url,
            "https://git.shakespeare.diy/npub1ts0wenlst23l73au2magp0zuy49gadnu8297955mlxc592jh5ldq0xzwcx/YouBlossom.git"
        );
    }

    #[test]
    fn single_clone_returned() {
        let url = extract_web_url(&event_fresh_repo()).unwrap();
        assert_eq!(
            url,
            "https://blossom.gnostr.cloud/npub1k9tkawv6ga6ptz3jl30pjzh68hk5mgvl28al5zc6r0myy849wvaq38a70g/fresh-repo.git"
        );
    }

    #[test]
    fn clone_takes_priority_over_web() {
        // jmp has both clone and web; clone should win
        let url = extract_web_url(&event_jmp()).unwrap();
        assert!(url.starts_with("https://relay.ngit.dev/"), "expected ngit clone, got: {url}");
    }

    #[test]
    fn falls_back_to_web_when_no_clone() {
        // Beer has no clone tag — falls back to web
        let url = extract_web_url(&event_beer()).unwrap();
        assert_eq!(url, "https://iBeer.shakespeare.wtf");
    }

    #[test]
    fn satshoot_first_clone_url() {
        let url = extract_web_url(&event_satshoot()).unwrap();
        assert!(
            url.starts_with("https://grasp.budabit.club/"),
            "expected budabit.club as first clone, got: {url}"
        );
    }

    #[test]
    fn empty_event_returns_none() {
        assert!(extract_web_url(&event_empty()).is_none());
    }

    // ── extract_event_relays ──────────────────────────────────────────────

    #[test]
    fn extract_relays_from_youblossom() {
        let relays = extract_event_relays(&event_youblossom());
        // Trailing slashes must be stripped
        assert!(relays.contains(&"wss://git.shakespeare.diy".to_string()));
        assert!(relays.contains(&"wss://relay.ngit.dev".to_string()));
        assert!(relays.contains(&"wss://nos.lol".to_string()));
        assert_eq!(relays.len(), 5);
    }

    #[test]
    fn extract_relays_trailing_slashes_stripped() {
        let relays = extract_event_relays(&event_youblossom());
        for r in &relays {
            assert!(!r.ends_with('/'), "relay has trailing slash: {r}");
        }
    }

    #[test]
    fn extract_relays_single_entry() {
        let relays = extract_event_relays(&event_fresh_repo());
        assert_eq!(relays, vec!["wss://blossom.gnostr.cloud"]);
    }

    #[test]
    fn extract_relays_no_tag_returns_empty() {
        assert!(extract_event_relays(&event_empty()).is_empty());
    }

    #[test]
    fn extract_relays_normalises_bare_hosts() {
        let event = serde_json::json!({
            "tags": [["relays", "relay.damus.io", "nos.lol"]]
        });
        let relays = extract_event_relays(&event);
        assert_eq!(relays[0], "wss://relay.damus.io");
        assert_eq!(relays[1], "wss://nos.lol");
    }

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

    #[test]
    fn normalize_trailing_slash_handled_by_caller() {
        // normalize_relay_url does NOT strip trailing slash — callers do
        // (extract_event_relays strips it before calling)
        let r = normalize_relay_url("wss://relay.damus.io/");
        assert_eq!(r, "wss://relay.damus.io/");
    }

    // ── build_relay_list ──────────────────────────────────────────────────

    #[test]
    fn relay_list_primary_first() {
        let list = build_relay_list(Some("wss://my-relay.example.com"));
        assert_eq!(list[0], "wss://my-relay.example.com");
    }

    #[test]
    fn relay_list_no_duplicates() {
        let list = build_relay_list(Some("wss://relay.damus.io"));
        let count = list.iter().filter(|r| r.as_str() == "wss://relay.damus.io").count();
        assert_eq!(count, 1, "damus should appear once: {list:?}");
    }

    #[test]
    fn relay_list_without_primary_uses_defaults() {
        let list = build_relay_list(None);
        assert!(!list.is_empty());
        assert_eq!(list[0], DEFAULT_RELAY_FALLBACKS[0]);
    }

    #[test]
    fn relay_list_normalises_primary() {
        let list = build_relay_list(Some("relay.damus.io"));
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
        let hex = "ee".repeat(32);
        assert_eq!(npub_to_hex(&hex).unwrap(), hex);
    }

    #[test]
    fn npub_hex_uppercased_normalised_to_lowercase() {
        let upper = "EE".repeat(32);
        assert_eq!(npub_to_hex(&upper).unwrap(), "ee".repeat(32));
    }

    #[test]
    fn npub_real_pubkeys_from_events() {
        // Real pubkeys seen in the queried events
        for hex in [
            "5c1eeccff05aa3ff47bc56fa80bc5c254a8eb67c3a8be2d29bf9b142aa57a7da",
            "b1576eb99a4774158a32fc5e190afa3ded4da19f51fbfa0b1a1bf6421ea5733a",
            "7459d333af66066f066cf87796e690db3a96ff4534f9edf4eab74df2f207289b",
            "34aff4e955e5d9a609434f5768ea8c089ee2afaa36d6e2be210c6813049a295f",
            "d04ecf33a303a59852fdb681ed8b412201ba85d8d2199aec73cb62681d62aa90",
        ] {
            let out = npub_to_hex(hex).unwrap_or_else(|_| panic!("failed for {hex}"));
            assert_eq!(out, hex, "hex pubkey should pass through unchanged");
        }
    }

    #[test]
    fn npub_invalid_rejected() {
        assert!(npub_to_hex("not-a-key").is_err());
        assert!(npub_to_hex("npub1bad!").is_err());
        assert!(npub_to_hex("").is_err());
    }
}
