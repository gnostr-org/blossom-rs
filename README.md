# blossom-rs

Full-featured [Blossom](https://github.com/hzrd149/blossom) blob storage library for Rust.

Content-addressed blob storage over HTTP with BIP-340 Schnorr authorization via Nostr kind:24242 events.

[![CI](https://github.com/MonumentalSystems/blossom-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MonumentalSystems/blossom-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/blossom-rs.svg)](https://crates.io/crates/blossom-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

## Features

- **Embeddable server** — mount a Blossom-compliant Axum router into your app
- **Async client** — upload/download with multi-server failover and SHA256 integrity
- **BIP-340 auth** — kind:24242 Nostr events for upload/download/delete authorization
- **Pluggable storage** — memory (testing), filesystem, S3-compatible backends
- **Database layer** — metadata persistence with SQLite/Postgres via SQLx
- **Access control** — whitelist with hot-reload, custom policies via trait
- **File statistics** — lock-free egress tracking with DashMap accumulator
- **NIP-96** — Nostr file storage protocol endpoints
- **BUD-01/02/04/06** — core Blossom protocol + list, mirror, upload requirements
- **Trait-based** — implement `BlossomSigner`, `BlobBackend`, `BlobDatabase`, `AccessControl`, `MediaProcessor`, or `MediaLabeler` for your own types

## Quick Start

```rust
use blossom_rs::{BlobServer, FilesystemBackend, Signer};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = BlobServer::new(
        FilesystemBackend::new("./blobs")?,
        "http://localhost:3000",
    );

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, server.router()).await?;
    Ok(())
}
```

### With Auth, Quotas, and Access Control

```rust
use blossom_rs::{BlobServer, MemoryBackend, MemoryDatabase};
use blossom_rs::access::Whitelist;
use std::collections::HashSet;

let mut db = MemoryDatabase::new();
db.set_quota("pubkey_hex...", Some(50 * 1024 * 1024)).unwrap(); // 50 MB

let server = BlobServer::builder(MemoryBackend::new(), "http://localhost:3000")
    .database(db)
    .access_control(Whitelist::new(HashSet::from(["pubkey_hex...".into()])))
    .require_auth()
    .max_upload_size(10 * 1024 * 1024) // 10 MB
    .build();
```

### Client

```rust
use blossom_rs::{BlossomClient, Signer};

let signer = Signer::generate();
let client = BlossomClient::new(
    vec!["https://blossom.example.com".into()],
    signer,
);

let desc = client.upload(b"hello", "text/plain").await?;
let data = client.download(&desc.sha256).await?;
```

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `server` | yes | Axum BlobServer and router |
| `client` | yes | reqwest BlossomClient with multi-server failover |
| `filesystem` | yes | FilesystemBackend (persistent, restart-safe) |
| `s3` | no | S3/R2/MinIO backend via `aws-sdk-s3` |
| `s3-compat` | no | S3-protocol test router |
| `db-sqlite` | no | SQLite metadata backend via SQLx |
| `db-postgres` | no | PostgreSQL metadata backend via SQLx |
| `media` | no | Image processing (WebP, thumbnails, blurhash, EXIF) |
| `labels` | no | Content classification (Vision Transformer, LLM API) |

## Protocol Support

| Protocol | Status | Endpoints |
|----------|--------|-----------|
| **BUD-01** | Implemented | `PUT /upload`, `GET/HEAD/DELETE /:sha256` |
| **BUD-02** | Implemented | `GET /list/:pubkey` |
| **BUD-04** | Implemented | `PUT /mirror` |
| **BUD-06** | Implemented | `GET /upload-requirements` |
| **NIP-96** | Implemented | `GET /.well-known/nostr/nip96.json`, `POST/GET/DELETE /n96` |
| **BIP-340** | Implemented | Schnorr signature auth on all write operations |

## Architecture

All extension points are trait-based:

```
BlossomSigner  — BIP-340 signing (bring your own identity)
BlobBackend    — blob storage (Memory, Filesystem, S3)
BlobDatabase   — metadata persistence (Memory, SQLite, Postgres)
AccessControl  — authorization (OpenAccess, Whitelist, custom)
MediaProcessor — image/video processing (Passthrough, ImageProcessor)
MediaLabeler   — content classification (Noop, BlockAll, custom)
```

Storage backends use synchronous interfaces wrapped in `Arc<Mutex<>>` by the async server. This matches the common commutator pattern where registry adapters use `std::sync::Mutex`.

## Testing

```bash
cargo test               # 90 tests (unit + integration + property)
cargo test --all-features # With all feature gates
cargo llvm-cov           # Coverage report (91.6% line coverage)
```

## CI/CD

- **CI**: Runs on push/PR to master — `cargo fmt --check`, `cargo build`, `cargo test`, `cargo clippy`
- **Publish**: Triggers on `v*` tags — tests then publishes to crates.io

## Acknowledgments

This library draws on patterns and design decisions from:

- **[route96](https://github.com/v0l/route96)** by v0l — Rust Blossom/NIP-96 server implementation. Reference for NIP-96 protocol, database schema patterns, media processing pipeline, EXIF validation, content labeling traits, whitelist access control, and file statistics. Licensed under MIT.

- **[hzrd149/blossom](https://github.com/hzrd149/blossom)** — The Blossom protocol specification (BUD-01 through BUD-06).

- The broader [Nostr](https://github.com/nostr-protocol/nostr) ecosystem for NIP-01, NIP-96, NIP-98, and BIP-340 standards.

## License

MIT
