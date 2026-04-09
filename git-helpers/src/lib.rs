//! git-remotes — `git-remote-blossom` and `git-remote-nostr` helpers.
//!
//! This crate provides two git remote helper binaries that extend git with
//! support for Blossom blob storage and Nostr NIP-34 git repositories.
//!
//! ## Install
//! ```bash
//! cargo install --path git-helpers
//! # or from the workspace root:
//! cargo build -p git-remotes --release
//! # copy both binaries somewhere on $PATH
//! ```
//!
//! ## Usage
//! ```bash
//! # Blossom remote — stores git bundles as Blossom blobs
//! git clone blossom://<server>/<pubkey>/<repo>
//! git remote add origin blossom://<server>/<pubkey>/<repo>
//!
//! # Nostr remote — resolves NIP-34 RepoAnnounce → GRASP HTTP server
//! git clone nostr://<npub>/<repo>
//! NOSTR_RELAY=wss://relay.example.com git clone nostr://<npub>/<repo>
//! git clone nostr+wss://relay.example.com/<npub>/<repo>
//! ```

pub mod auth;
pub mod blossom_backend;
pub mod nostr_backend;
pub mod nostr_relay;
pub mod protocol;
