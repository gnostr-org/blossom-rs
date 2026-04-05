//! Pluggable access control for Blossom servers.
//!
//! The [`AccessControl`] trait lets you authorize or reject operations
//! based on the caller's public key and the requested action.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Actions that can be authorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Upload,
    Download,
    Delete,
    List,
    Mirror,
    Admin,
}

/// Trait for pluggable access control decisions.
///
/// Implementations decide whether a given pubkey is allowed to perform
/// a given action. Return `true` to allow, `false` to deny.
pub trait AccessControl: Send + Sync {
    /// Check if `pubkey` is authorized for `action`.
    fn is_allowed(&self, pubkey: &str, action: Action) -> bool;
}

/// Open access — allows everything. Default when no access control is configured.
pub struct OpenAccess;

impl AccessControl for OpenAccess {
    fn is_allowed(&self, _pubkey: &str, _action: Action) -> bool {
        true
    }
}

/// Pubkey whitelist access control.
///
/// Only pubkeys in the whitelist are allowed to perform any action.
/// Supports loading from a file (one hex pubkey per line) and hot-reload.
pub struct Whitelist {
    pubkeys: Arc<RwLock<HashSet<String>>>,
}

impl Whitelist {
    /// Create a whitelist from a set of hex-encoded pubkeys.
    pub fn new(pubkeys: HashSet<String>) -> Self {
        Self {
            pubkeys: Arc::new(RwLock::new(pubkeys)),
        }
    }

    /// Load a whitelist from a file (one hex pubkey per line).
    /// Empty lines and lines starting with `#` are ignored.
    pub fn from_file(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let pubkeys = Self::parse_pubkeys(&content);
        Ok(Self::new(pubkeys))
    }

    /// Reload the whitelist from a file. Call this periodically or on file change.
    pub async fn reload(&self, path: &Path) -> std::io::Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        let new_keys = Self::parse_pubkeys(&content);
        let mut keys = self.pubkeys.write().await;
        *keys = new_keys;
        tracing::info!(
            component = "blossom.access",
            count = keys.len(),
            "whitelist reloaded"
        );
        Ok(())
    }

    /// Add a pubkey to the whitelist at runtime.
    pub async fn add(&self, pubkey: String) {
        self.pubkeys.write().await.insert(pubkey);
    }

    /// Remove a pubkey from the whitelist at runtime.
    pub async fn remove(&self, pubkey: &str) {
        self.pubkeys.write().await.remove(pubkey);
    }

    /// Check if a pubkey is whitelisted (async version for direct use).
    pub async fn contains(&self, pubkey: &str) -> bool {
        self.pubkeys.read().await.contains(pubkey)
    }

    /// Number of whitelisted pubkeys.
    pub async fn len(&self) -> usize {
        self.pubkeys.read().await.len()
    }

    /// Whether the whitelist is empty.
    pub async fn is_empty(&self) -> bool {
        self.pubkeys.read().await.is_empty()
    }

    fn parse_pubkeys(content: &str) -> HashSet<String> {
        content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter(|line| line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|line| line.to_string())
            .collect()
    }
}

impl AccessControl for Whitelist {
    fn is_allowed(&self, pubkey: &str, _action: Action) -> bool {
        // Use try_read to avoid blocking. If we can't acquire the lock,
        // deny access (fail closed).
        match self.pubkeys.try_read() {
            Ok(keys) => keys.contains(pubkey),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_access_allows_all() {
        let ac = OpenAccess;
        assert!(ac.is_allowed("anything", Action::Upload));
        assert!(ac.is_allowed("anything", Action::Delete));
        assert!(ac.is_allowed("anything", Action::Admin));
    }

    #[test]
    fn test_whitelist_allows_listed() {
        let pubkey = "a".repeat(64);
        let mut keys = HashSet::new();
        keys.insert(pubkey.clone());
        let wl = Whitelist::new(keys);

        assert!(wl.is_allowed(&pubkey, Action::Upload));
        assert!(wl.is_allowed(&pubkey, Action::Download));
    }

    #[test]
    fn test_whitelist_denies_unlisted() {
        let wl = Whitelist::new(HashSet::new());
        let pubkey = "b".repeat(64);
        assert!(!wl.is_allowed(&pubkey, Action::Upload));
    }

    #[test]
    fn test_parse_pubkeys_from_content() {
        let content = format!(
            "# This is a comment\n\n{}\n{}\ninvalid-short\n  \n{}",
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
        );
        let keys = Whitelist::parse_pubkeys(&content);
        assert_eq!(keys.len(), 3);
        assert!(keys.contains(&"a".repeat(64)));
        assert!(!keys.contains("invalid-short"));
    }

    #[tokio::test]
    async fn test_whitelist_add_remove() {
        let wl = Whitelist::new(HashSet::new());
        let pk = "d".repeat(64);

        assert!(!wl.contains(&pk).await);
        wl.add(pk.clone()).await;
        assert!(wl.contains(&pk).await);
        assert_eq!(wl.len().await, 1);

        wl.remove(&pk).await;
        assert!(!wl.contains(&pk).await);
        assert!(wl.is_empty().await);
    }

    #[test]
    fn test_whitelist_from_file() {
        let dir = std::env::temp_dir().join(format!("blossom_wl_{}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("whitelist.txt");

        let content = format!("# allowed users\n{}\n{}\n", "e".repeat(64), "f".repeat(64),);
        std::fs::write(&file, &content).unwrap();

        let wl = Whitelist::from_file(&file).unwrap();
        assert!(wl.is_allowed(&"e".repeat(64), Action::Upload));
        assert!(wl.is_allowed(&"f".repeat(64), Action::Download));
        assert!(!wl.is_allowed(&"0".repeat(64), Action::Upload));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
