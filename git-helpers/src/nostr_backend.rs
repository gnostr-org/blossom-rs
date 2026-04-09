//! Nostr / NIP-34 backend for `git-remote-nostr`.
//!
//! ## URL formats
//! ```text
//! nostr://<npub>/<repo>
//! nostr+wss://<relay-host>/<npub>/<repo>   (explicit relay)
//! nostr+ws://<relay-host>/<npub>/<repo>    (plaintext, dev only)
//! ```
//!
//! ## How it works
//!
//! 1. Parse the URL to extract npub (or hex pubkey) and repo name.
//! 2. Query a Nostr relay for a kind:30617 (NIP-34 RepoAnnounce) event
//!    whose `d` tag matches the repo name.
//! 3. Extract the `clone` or `web` URL from the event tags — this is the
//!    GRASP HTTP smart-protocol URL.
//! 4. Delegate all git transport to the resolved HTTP URL by shelling out
//!    to `git ls-remote`, `git fetch`, and `git push` with that URL.
//!
//! ## Environment variables
//!
//! | Variable        | Purpose                                    |
//! |-----------------|---------------------------------------------|
//! | `NOSTR_RELAY`   | WSS relay URL (used when not in the URL)   |
//! | `NOSTR_NSEC`    | Secret key for push auth (nsec1… or hex)   |
//! | `GRASP_SERVER`  | Override: skip relay lookup, use this URL  |

use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::nostr_relay::{npub_to_hex, resolve_grasp_url};
use crate::protocol::{FetchCmd, GitRef, PushResult, PushSpec, RemoteHelper};

// ── Backend ────────────────────────────────────────────────────────────────

pub struct NostrRemote {
    relay_url: String,
    pubkey_hex: String,
    repo: String,
    /// Resolved GRASP HTTP URL, cached after first lookup.
    resolved: Option<String>,
}

impl NostrRemote {
    pub fn new(relay_url: &str, pubkey_hex: &str, repo: &str) -> Self {
        Self {
            relay_url: relay_url.to_string(),
            pubkey_hex: pubkey_hex.to_string(),
            repo: repo.trim_end_matches(".git").to_string(),
            resolved: None,
        }
    }

    /// Return the resolved GRASP HTTP URL, querying the relay if needed.
    fn http_url(&mut self) -> Result<&str> {
        if self.resolved.is_some() {
            return Ok(self.resolved.as_deref().unwrap());
        }

        // Fast path: explicit override via env
        if let Ok(server) = std::env::var("GRASP_SERVER") {
            let url = format!(
                "{}/{}/{}",
                server.trim_end_matches('/'),
                self.pubkey_hex,
                self.repo
            );
            eprintln!("[nostr] using GRASP_SERVER: {url}");
            self.resolved = Some(url);
            return Ok(self.resolved.as_deref().unwrap());
        }

        // Query relay
        eprintln!("[nostr] querying {} for {:.8}…/{}", self.relay_url, self.pubkey_hex, self.repo);
        let url = resolve_grasp_url(&self.relay_url, &self.pubkey_hex, &self.repo)?;
        eprintln!("[nostr] resolved → {url}");
        self.resolved = Some(url);
        Ok(self.resolved.as_deref().unwrap())
    }
}

impl RemoteHelper for NostrRemote {
    fn capabilities(&self) -> &[&'static str] {
        &["fetch", "push", "option"]
    }

    fn list(&mut self, for_push: bool) -> Result<Vec<GitRef>> {
        let http_url = self.http_url()?.to_string();

        // Use `git ls-remote` to get the ref list from the GRASP server
        let mut cmd = Command::new("git");
        cmd.args(["ls-remote", "--symref"]);

        if for_push {
            // For push we still need the remote refs to detect conflicts
        }
        cmd.arg(&http_url);

        let out = cmd.output().context("git ls-remote")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("git ls-remote failed: {stderr}");
        }

        let mut refs = Vec::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            // symref lines: "ref: refs/heads/main	HEAD"
            if let Some(rest) = line.strip_prefix("ref: ") {
                let (target, name) = rest.split_once('\t').unwrap_or((rest, "HEAD"));
                refs.push(GitRef {
                    name: name.to_string(),
                    oid: String::new(),
                    symref_target: Some(target.to_string()),
                });
                continue;
            }
            // normal lines: "<sha1>\t<refname>"
            if let Some((oid, name)) = line.split_once('\t') {
                refs.push(GitRef {
                    name: name.to_string(),
                    oid: oid.to_string(),
                    symref_target: None,
                });
            }
        }

        Ok(refs)
    }

    fn fetch(&mut self, cmds: Vec<FetchCmd>) -> Result<()> {
        let http_url = self.http_url()?.to_string();

        // Build fetch refspecs: "<oid>:refs/remotes/..." isn't needed —
        // we just need git to fetch the specific SHAs.
        let refspecs: Vec<String> = cmds
            .iter()
            .map(|c| format!("{}:{}", c.name, c.name))
            .collect();

        let mut cmd = Command::new("git");
        cmd.args(["fetch", "--no-write-fetch-head", &http_url]);
        for spec in &refspecs {
            cmd.arg(spec);
        }

        let status = cmd.status().context("git fetch")?;
        if !status.success() {
            bail!("git fetch from {} failed", http_url);
        }
        Ok(())
    }

    fn push(&mut self, specs: Vec<PushSpec>) -> Result<Vec<PushResult>> {
        let http_url = self.http_url()?.to_string();

        let refspecs: Vec<String> = specs
            .iter()
            .map(|s| {
                let force = if s.force { "+" } else { "" };
                format!("{force}{}:{}", s.src, s.dst)
            })
            .collect();

        let mut cmd = Command::new("git");
        cmd.arg("push").arg(&http_url);
        for spec in &refspecs {
            cmd.arg(spec);
        }

        // Add Nostr auth header via GIT_CONFIG_COUNT env if NOSTR_NSEC is set
        if std::env::var("NOSTR_NSEC").is_ok() {
            // git will present this as an HTTP header in the push request
            // The GRASP server verifies it for receive-pack
            cmd.env("GIT_HTTP_EXTRA_HEADER", nostr_push_auth_header(&http_url));
        }

        let status = cmd.status().context("git push")?;

        let result = if status.success() {
            Ok(())
        } else {
            Err(format!("git push to {http_url} failed"))
        };

        Ok(specs
            .iter()
            .map(|s| PushResult {
                dst: s.dst.clone(),
                result: result.clone(),
            })
            .collect())
    }
}

/// Build the `Authorization: Nostr <base64>` value for a git push.
fn nostr_push_auth_header(url: &str) -> String {
    let nsec = match std::env::var("NOSTR_NSEC") {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    match crate::auth::build_push_auth(&nsec, url) {
        Ok(b64) => format!("Authorization: Nostr {b64}"),
        Err(e) => {
            eprintln!("[nostr] auth error: {e}");
            String::new()
        }
    }
}

// ── URL parser ─────────────────────────────────────────────────────────────

/// Parse a `nostr://` URL into `(relay_wss_url, pubkey_hex, repo_name)`.
///
/// Accepted formats:
/// - `nostr://<npub>/<repo>`                         → relay from `$NOSTR_RELAY`
/// - `nostr://<npub>/<relay-host>/<repo>`            → relay embedded as `wss://<relay-host>`
/// - `nostr+wss://<relay-host>/<npub>/<repo>`       → explicit WSS relay
/// - `nostr+ws://<relay-host>/<npub>/<repo>`        → explicit WS relay (dev)
pub fn parse_nostr_url(url: &str) -> Result<(String, String, String)> {
    let env_relay = std::env::var("NOSTR_RELAY").ok();
    parse_nostr_url_inner(url, env_relay.as_deref())
}

/// Inner parser — accepts the relay env value explicitly so it can be tested
/// without touching process-global env vars.
fn parse_nostr_url_inner(url: &str, env_relay: Option<&str>) -> Result<(String, String, String)> {
    let (relay_host_opt, rest) = if let Some(r) = url.strip_prefix("nostr+wss://") {
        let (host, rest) = r.split_once('/').context("missing / after relay host")?;
        (Some(format!("wss://{host}")), rest)
    } else if let Some(r) = url.strip_prefix("nostr+ws://") {
        let (host, rest) = r.split_once('/').context("missing / after relay host")?;
        (Some(format!("ws://{host}")), rest)
    } else if let Some(r) = url.strip_prefix("nostr://") {
        (None, r)
    } else {
        bail!("not a nostr:// URL: {url}");
    };

    // Split path into up to 3 segments.
    // 2 → <npub>/<repo>              (relay from env or scheme)
    // 3 → <npub>/<relay-host>/<repo> (relay embedded in path)
    let parts: Vec<&str> = rest.splitn(3, '/').collect();

    let (relay_url, npub_str, repo_str) = match (relay_host_opt, parts.as_slice()) {
        // Relay in scheme → always wins, regardless of path segment count
        (Some(relay), [npub, repo]) => (relay, *npub, *repo),
        (Some(relay), [npub, _embedded, repo]) => (relay, *npub, *repo),

        // 3-segment path → middle segment is the relay host
        (None, [npub, relay_host, repo]) => {
            (format!("wss://{relay_host}"), *npub, *repo)
        }

        // 2-segment path → relay from env
        (None, [npub, repo]) => {
            let relay = env_relay
                .ok_or_else(|| anyhow::anyhow!("no relay in URL and NOSTR_RELAY env var not set"))?;
            (relay.to_string(), *npub, *repo)
        }

        _ => bail!("nostr URL must contain <npub>/<repo> or <npub>/<relay>/<repo>, got: {rest}"),
    };

    let pubkey_hex = npub_to_hex(npub_str)?;
    let repo = repo_str.trim_end_matches(".git").to_string();

    Ok((relay_url, pubkey_hex, repo))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NPUB: &str =
        "npub1ahaz04ya9tehace3uy39hdhdryfvdkve9qdndkqp3tvehs6h8s5slq45hy";

    fn hex_of_npub() -> String {
        npub_to_hex(NPUB).expect("valid npub")
    }

    // ── URL from the original bug report ─────────────────────────────────

    #[test]
    fn bug_report_url_extracts_relay_from_path() {
        let url = format!("nostr://{NPUB}/nostr.cro.social/gnostr");
        let (relay, pubkey, repo) =
            parse_nostr_url_inner(&url, None).expect("should parse");
        assert_eq!(relay, "wss://nostr.cro.social");
        assert_eq!(pubkey, hex_of_npub());
        assert_eq!(repo, "gnostr");
    }

    // ── 3-segment: embedded relay ─────────────────────────────────────────

    #[test]
    fn three_segment_url_extracts_relay_from_path() {
        let url = format!("nostr://{NPUB}/relay.damus.io/my-repo");
        let (relay, pubkey, repo) =
            parse_nostr_url_inner(&url, None).expect("should parse");
        assert_eq!(relay, "wss://relay.damus.io");
        assert_eq!(pubkey, hex_of_npub());
        assert_eq!(repo, "my-repo");
    }

    // ── 2-segment: relay from env ─────────────────────────────────────────

    #[test]
    fn two_segment_url_without_relay_returns_error() {
        let url = format!("nostr://{NPUB}/gnostr");
        let err = parse_nostr_url_inner(&url, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("no relay in URL and NOSTR_RELAY env var not set"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn two_segment_url_with_env_relay_succeeds() {
        let url = format!("nostr://{NPUB}/my-repo");
        let (relay, pubkey, repo) =
            parse_nostr_url_inner(&url, Some("wss://relay.damus.io")).expect("should parse");
        assert_eq!(relay, "wss://relay.damus.io");
        assert_eq!(pubkey, hex_of_npub());
        assert_eq!(repo, "my-repo");
    }

    // ── Scheme relay takes precedence ─────────────────────────────────────

    #[test]
    fn explicit_scheme_relay_beats_embedded_path_relay() {
        // nostr+wss://relay.example.com/<npub>/<other-relay>/<repo>
        let url = format!("nostr+wss://relay.example.com/{NPUB}/other.relay.com/gnostr");
        let (relay, _pubkey, repo) =
            parse_nostr_url_inner(&url, None).expect("should parse");
        assert_eq!(relay, "wss://relay.example.com");
        assert_eq!(repo, "gnostr");
    }

    #[test]
    fn nostr_plus_wss_two_segments() {
        let url = format!("nostr+wss://relay.nostr.band/{NPUB}/my-repo");
        let (relay, pubkey, repo) =
            parse_nostr_url_inner(&url, None).expect("should parse");
        assert_eq!(relay, "wss://relay.nostr.band");
        assert_eq!(pubkey, hex_of_npub());
        assert_eq!(repo, "my-repo");
    }

    // ── .git suffix stripping ─────────────────────────────────────────────

    #[test]
    fn git_suffix_stripped_three_segment() {
        let url = format!("nostr://{NPUB}/nostr.cro.social/gnostr.git");
        let (_relay, _pubkey, repo) =
            parse_nostr_url_inner(&url, None).expect("should parse");
        assert_eq!(repo, "gnostr");
    }

    #[test]
    fn git_suffix_stripped_two_segment() {
        let url = format!("nostr://{NPUB}/my-repo.git");
        let (_relay, _pubkey, repo) =
            parse_nostr_url_inner(&url, Some("wss://relay.example.com")).expect("should parse");
        assert_eq!(repo, "my-repo");
    }
}
