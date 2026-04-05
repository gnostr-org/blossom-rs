# CLAUDE.md — blossom-rs

## Project Overview

**blossom-rs** is an embeddable Blossom (BUD-01) blob storage library for Rust.
Content-addressed blob storage over HTTP with BIP-340 Schnorr authorization via
Nostr kind:24242 events. Targets crates.io publication.

Repository: `MonumentalSystems/blossom-rs`

## Build & Test Commands

```bash
cargo build                          # Build with default features (server, client, filesystem)
cargo build --all-features           # Build everything including s3, s3-compat
cargo test                           # Run all tests
cargo test --all-features            # Run tests with all feature gates
cargo clippy -- -D warnings          # Lint (CI enforces zero warnings)
cargo fmt --check                    # Format check (CI enforces)
cargo fmt                            # Auto-format
cargo doc --no-deps --open           # Generate and view docs
```

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `server` | yes | Axum BlobServer and router |
| `client` | yes | reqwest BlossomClient with multi-server failover |
| `filesystem` | yes | FilesystemBackend (persistent, restart-safe) |
| `s3` | no | S3/R2/MinIO backend via aws-sdk-s3 |
| `s3-compat` | no | S3-compat test router (requires `server`) |
| `db-sqlite` | no | SQLite metadata backend via SQLx |
| `db-postgres` | no | PostgreSQL metadata backend via SQLx |
| `media` | no | Media processing (WebP, thumbnails, blurhash, EXIF) |
| `labels` | no | Content labeling (Vision Transformer, LLM API) |

## Architecture

### Module Map

```
src/
├── lib.rs              — Public API, feature-gated re-exports
├── protocol.rs         — NostrEvent, BlobDescriptor, base64url, sha256_hex, compute_event_id
├── auth/
│   ├── mod.rs          — build_blossom_auth, verify_blossom_auth, AuthError
│   └── signer.rs       — BlossomSigner trait, default Signer (secp256k1 BIP-340)
├── storage/
│   ├── mod.rs          — BlobBackend trait, make_descriptor helper
│   ├── memory.rs       — MemoryBackend (HashMap, for testing)
│   ├── filesystem.rs   — FilesystemBackend (sha256.blob files, index scan on startup)
│   └── s3.rs           — S3Backend (aws-sdk-s3, optional CDN URL)
├── db/
│   ├── mod.rs          — BlobDatabase trait
│   ├── memory.rs       — MemoryDatabase (in-process, no persistence)
│   ├── sqlite.rs       — SqliteDatabase (SQLx)
│   └── postgres.rs     — PostgresDatabase (SQLx)
├── server/
│   ├── mod.rs          — BlobServer (Axum router), all HTTP handlers
│   ├── nip96.rs        — NIP-96 endpoints
│   └── admin.rs        — Admin endpoints
├── client/
│   └── mod.rs          — BlossomClient with failover + SHA256 integrity
├── access/
│   └── mod.rs          — AccessControl trait, Whitelist
├── media/
│   └── mod.rs          — MediaProcessor trait, WebP/thumbnail/blurhash/EXIF
├── labels/
│   └── mod.rs          — MediaLabeler trait, VitLabeler, LlmLabeler
└── stats.rs            — File statistics (DashMap + atomic counters)
```

### Key Traits

- **`BlossomSigner`** — BIP-340 signing. Implement for your identity type.
- **`BlobBackend`** — Synchronous blob storage (Memory, Filesystem, S3). Wrapped in `Arc<Mutex<>>` by server.
- **`BlobDatabase`** — Metadata persistence (uploads, users, quotas, stats).
- **`AccessControl`** — Authorization decisions (whitelist, role-based).
- **`MediaProcessor`** — Image/video processing pipeline.
- **`MediaLabeler`** — Content classification.

### Design Conventions

- Content-addressed: SHA256 = blob key = natural deduplication
- Traits for all extension points; concrete types behind feature flags
- Sync trait interfaces wrapped in `Arc<Mutex<>>` for async handlers
- `thiserror` for all error enums
- `tracing` for structured logging (no-op if subscriber not configured)
- `serde` derive on all public types; optional fields use `skip_serializing_if`
- Axum 0.7 route syntax (`:param` not `{param}`)

## Testing Conventions

- Unit tests in `#[cfg(test)] mod tests` at bottom of each module
- Async integration tests use `#[tokio::test]` with ephemeral TCP listeners
- Server tests spawn a real Axum server on `127.0.0.1:0` (random port)
- Filesystem tests use `std::env::temp_dir()` with random suffixes, clean up after
- All new public functions must have tests
- Use `MemoryBackend` / `MemoryDatabase` in tests to avoid external dependencies
- Property tests with `proptest` where data integrity matters

## CI/CD

- **CI** (`.github/workflows/ci.yml`): On push/PR to master — fmt check, build all targets, test, clippy
- **Publish** (`.github/workflows/publish.yml`): On `v*` tags — test then `cargo publish`
- Self-hosted runner, `dtolnay/rust-toolchain@stable`
- GPG signing disabled for CI commits

## Git Conventions

- Branch: `master`
- Commit messages: imperative, concise, focus on "why"
- No GPG signing required (`gpg nosign`)
- Tag releases as `v0.1.0`, `v0.2.0`, etc.

## Protocol References

- **BUD-01**: Core blob upload/download/delete (implemented)
- **BUD-02**: List blobs by pubkey
- **BUD-04**: Mirror from remote URL
- **BUD-05**: Media optimization (server-side compression)
- **BUD-06**: Upload requirements advertisement
- **NIP-96**: Nostr file storage protocol
- **NIP-01**: Nostr event format (used for auth)
- **BIP-340**: Schnorr signatures on secp256k1

## Dependencies Policy

- Minimal dependency tree for core (crypto + serde + tokio)
- Optional deps behind feature flags
- `secp256k1` 0.29 for BIP-340 (matches ecosystem)
- `axum` 0.7 for server
- `reqwest` 0.12 for client
- `sqlx` for database backends
- No `unsafe` code
