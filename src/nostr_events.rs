//! Nostr event publishing for blob operations.
//!
//! Builds and publishes NIP-94 file metadata (kind:1063) and BUD-03
//! server list (kind:10063) events after blob uploads.

use crate::auth::BlossomSigner;
use crate::protocol::BlobDescriptor;

/// Kind 1063: NIP-94 file metadata event.
pub const KIND_FILE_METADATA: u16 = 1063;

/// Kind 10063: BUD-03 user server list (replaceable).
pub const KIND_SERVER_LIST: u16 = 10063;

/// Build a NIP-94 file metadata event (kind:1063) for an uploaded blob.
///
/// Tags: url, m (MIME), x (SHA256), size, ox (original SHA256).
pub fn build_file_metadata_event(
    signer: &dyn BlossomSigner,
    desc: &BlobDescriptor,
    server_url: &str,
    content_type: &str,
) -> serde_json::Value {
    let blob_url = format!("{}/{}", server_url.trim_end_matches('/'), desc.sha256);

    let tags = vec![
        vec!["url".to_string(), blob_url],
        vec!["m".to_string(), content_type.to_string()],
        vec!["x".to_string(), desc.sha256.clone()],
        vec!["ox".to_string(), desc.sha256.clone()],
        vec!["size".to_string(), desc.size.to_string()],
    ];

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    serde_json::json!({
        "kind": KIND_FILE_METADATA,
        "pubkey": signer.public_key_hex(),
        "created_at": created_at,
        "tags": tags,
        "content": "",
    })
}

/// Build a BUD-03 server list event (kind:10063) listing the user's servers.
///
/// This is a replaceable event — only the latest one matters.
pub fn build_server_list_event(
    signer: &dyn BlossomSigner,
    server_urls: &[String],
) -> serde_json::Value {
    let tags: Vec<Vec<String>> = server_urls
        .iter()
        .map(|url| vec!["server".to_string(), url.clone()])
        .collect();

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    serde_json::json!({
        "kind": KIND_SERVER_LIST,
        "pubkey": signer.public_key_hex(),
        "created_at": created_at,
        "tags": tags,
        "content": "",
    })
}

/// Publish an event to a Nostr relay via HTTP POST.
///
/// Sends a NIP-01 CLIENT message to a relay WebSocket URL.
/// Falls back to HTTP POST if the relay supports it.
pub async fn publish_to_relay(
    relay_url: &str,
    event_json: &serde_json::Value,
) -> Result<(), String> {
    // For HTTP relays, POST the event directly
    let http = reqwest::Client::new();
    let resp = http
        .post(relay_url)
        .json(event_json)
        .send()
        .await
        .map_err(|e| format!("publish to relay: {e}"))?;

    if resp.status().is_success() {
        tracing::info!(
            relay = %relay_url,
            kind = event_json["kind"].as_u64().unwrap_or(0),
            "event published to relay"
        );
        Ok(())
    } else {
        let text = resp.text().await.unwrap_or_default();
        Err(format!("relay rejected event: {text}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Signer;

    #[test]
    fn test_build_file_metadata_event() {
        let signer = Signer::generate();
        let desc = BlobDescriptor {
            sha256: "abc123".repeat(10),
            size: 1024,
            content_type: Some("image/png".into()),
            url: None,
            uploaded: None,
        };

        let event = build_file_metadata_event(&signer, &desc, "https://example.com", "image/png");
        assert_eq!(event["kind"], KIND_FILE_METADATA);
        assert_eq!(event["tags"][0][0], "url");
        assert_eq!(event["tags"][1][1], "image/png");
        assert_eq!(event["tags"][2][0], "x");
        assert_eq!(event["tags"][3][0], "ox");
        assert_eq!(event["tags"][4][1], "1024");
    }

    #[test]
    fn test_build_server_list_event() {
        let signer = Signer::generate();
        let servers = vec![
            "https://blossom1.example.com".into(),
            "https://blossom2.example.com".into(),
        ];

        let event = build_server_list_event(&signer, &servers);
        assert_eq!(event["kind"], KIND_SERVER_LIST);
        let tags = event["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0][0], "server");
        assert_eq!(tags[0][1], "https://blossom1.example.com");
    }
}
