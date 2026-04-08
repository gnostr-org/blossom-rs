//! Shared state for the NIP-34 relay and git server.

use std::path::PathBuf;
use std::sync::Arc;

use nostr_database::NostrDatabase;
use nostr_relay_builder::LocalRelay;

use crate::config::Nip34Config;
use crate::relay;

/// Shared state for all NIP-34 handlers.
pub struct Nip34State {
    pub config: Nip34Config,
    pub relay: Arc<LocalRelay>,
    pub database: Arc<dyn NostrDatabase>,
}

impl Nip34State {
    /// Create a new NIP-34 state, initializing the LMDB database and relay.
    pub async fn new(config: Nip34Config) -> Result<Self, Box<dyn std::error::Error>> {
        // Ensure repos directory exists
        tokio::fs::create_dir_all(&config.repos_path).await?;

        // Initialize LMDB event database
        let database: Arc<dyn NostrDatabase> =
            Arc::new(nostr_lmdb::NostrLMDB::open(&config.lmdb_path)?);

        // Build the relay
        let local_relay = relay::build_relay(&config, database.clone()).await?;

        Ok(Self {
            config,
            relay: Arc::new(local_relay),
            database,
        })
    }

    /// Get the filesystem path for a repository.
    /// Returns `None` if the repo name is invalid.
    pub fn repo_path(&self, npub: &str, repo_name: &str) -> Option<PathBuf> {
        // Validate repo name: alphanumeric + hyphens + underscores, max 30 chars
        if repo_name.is_empty()
            || repo_name.len() > 30
            || !repo_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }

        let path = self
            .config
            .repos_path
            .join(npub)
            .join(format!("{}.git", repo_name));
        Some(path)
    }

    /// Check if a repository exists on disk.
    pub fn repo_exists(&self, npub: &str, repo_name: &str) -> bool {
        self.repo_path(npub, repo_name)
            .map(|p| p.join("HEAD").exists())
            .unwrap_or(false)
    }

    /// Create a bare git repository for a given npub/repo.
    pub async fn create_bare_repo(
        &self,
        npub: &str,
        repo_name: &str,
        description: &str,
    ) -> Result<PathBuf, String> {
        let path = self
            .repo_path(npub, repo_name)
            .ok_or_else(|| format!("invalid repo name: {}", repo_name))?;

        if path.join("HEAD").exists() {
            return Ok(path);
        }

        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| format!("create repo dir: {e}"))?;

        let status = tokio::process::Command::new(&self.config.git_path)
            .args(["init", "--bare", "--quiet", "."])
            .current_dir(&path)
            .status()
            .await
            .map_err(|e| format!("git init: {e}"))?;

        if !status.success() {
            return Err("git init --bare failed".into());
        }

        // Write description
        let desc_path = path.join("description");
        tokio::fs::write(&desc_path, description)
            .await
            .map_err(|e| format!("write description: {e}"))?;

        tracing::info!(
            repo.npub = %npub,
            repo.name = %repo_name,
            repo.path = %path.display(),
            "created bare git repository"
        );

        Ok(path)
    }
}
