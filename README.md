# blossom-rs

Full-featured [Blossom](https://github.com/hzrd149/blossom) blob storage library for Rust.

Content-addressed blob storage over HTTP with BIP-340 Schnorr authorization via Nostr kind:24242 events.

[![CI](https://github.com/MonumentalSystems/blossom-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/MonumentalSystems/blossom-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/blossom-rs.svg)](https://crates.io/crates/blossom-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

## Workspace

| Crate | Description |
|-------|-------------|
| **blossom-rs** | Core library — embeddable server, async client, all traits |
| **blossom-server** | API server binary — filesystem + SQLite, CORS, TLS, graceful shutdown |
| **blossom-cli** | CLI client — upload/download/mirror/keygen, hex + nsec1 key support |

## Features

- **Embeddable server** — mount a Blossom-compliant Axum router into your app
- **Async client** — upload/download with multi-server failover and SHA256 integrity
- **BIP-340 auth** — kind:24242 Nostr events for upload/download/delete authorization
- **Pluggable storage** — memory (testing), filesystem, S3-compatible backends
- **Database layer** — metadata persistence with SQLite/Postgres via SQLx
- **Access control** — whitelist with hot-reload, custom policies via trait
- **File statistics** — lock-free egress tracking with DashMap accumulator, periodic DB flush
- **Observability** — OTEL-compatible structured tracing with opt-in OTLP export
- **NIP-96** — Nostr file storage protocol endpoints
- **BUD-01/02/04/06** — core Blossom protocol + list, mirror, upload requirements
- **Health check** — `GET /health` for load balancer probes
- **CORS** — configurable cross-origin support for browser clients
- **TLS** — optional rustls-based HTTPS via `axum-server`
- **Graceful shutdown** — flushes stats to DB on Ctrl+C
- **Media processing** — WebP conversion, thumbnails, blurhash, EXIF validation (feature-gated)
- **Content labeling** — pluggable classification traits for moderation (feature-gated)
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
    .body_limit(50 * 1024 * 1024)      // 50 MB HTTP body limit
    .build();
```

### Client

Keys are accepted as hex (64 chars) or nsec1 bech32 format.

```rust
use blossom_rs::{BlossomClient, Signer};

let signer = Signer::generate();
// Or from existing key: Signer::from_secret_hex("deadbeef...")
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
| `server` | yes | Axum BlobServer and router with TraceLayer |
| `client` | yes | reqwest BlossomClient with multi-server failover |
| `filesystem` | yes | FilesystemBackend (persistent, restart-safe) |
| `s3` | no | S3/R2/MinIO backend via `aws-sdk-s3` |
| `s3-compat` | no | S3-protocol compatibility test router |
| `db-sqlite` | no | SQLite metadata backend via SQLx |
| `db-postgres` | no | PostgreSQL metadata backend via SQLx |
| `media` | no | Image processing (WebP, thumbnails, blurhash, EXIF) |
| `labels` | no | Content classification (Vision Transformer, LLM API) |
| `otel` | no | OpenTelemetry OTLP export (Jaeger, Tempo, Seq, Honeycomb) |

## Protocol Support

| Protocol | Status | Endpoints |
|----------|--------|-----------|
| **BUD-01** | Implemented | `PUT /upload`, `GET/HEAD/DELETE /:sha256` |
| **BUD-02** | Implemented | `GET /list/:pubkey` |
| **BUD-04** | Implemented | `PUT /mirror` |
| **BUD-06** | Implemented | `GET /upload-requirements` |
| **NIP-96** | Implemented | `GET /.well-known/nostr/nip96.json`, `POST/GET/DELETE /n96` |
| **BIP-340** | Implemented | Schnorr signature auth on all write operations |
| **S3-compat** | Implemented | `PUT/GET/HEAD/DELETE /:bucket/*key` (feature-gated) |
| **Health** | Implemented | `GET /health` |
| **Status** | Implemented | `GET /status` |

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

Storage backends use synchronous interfaces wrapped in `Arc<Mutex<>>` by the async server. Database backends use `tokio::task::block_in_place` for safe async-to-sync bridging.

### Server Builder

```rust
BlobServer::builder(backend, "http://localhost:3000")
    .database(db)               // BlobDatabase impl
    .access_control(whitelist)  // AccessControl impl
    .require_auth()             // Enforce BIP-340 auth on uploads
    .max_upload_size(10_000_000) // BUD-06 size limit
    .allowed_types(vec!["image/png".into()]) // BUD-06 type filter
    .body_limit(50_000_000)     // HTTP body size limit
    .build();
```

## Observability

All key functions are instrumented with `#[tracing::instrument]` using [OTEL semantic conventions](https://opentelemetry.io/docs/specs/semconv/):

| Namespace | Fields |
|-----------|--------|
| `http.*` | `method`, `route`, `status_code` |
| `blob.*` | `sha256`, `size`, `content_type` |
| `auth.*` | `pubkey`, `action`, `kind` |
| `storage.*` | `backend`, `data_dir`, `bucket` |
| `server.*` | `url` |
| `error.*` | `message` |

**Zero-cost by default** — `tracing` is a no-op without a subscriber.

**Opt-in OTLP export** for Jaeger, Grafana Tempo, Seq, Honeycomb, etc.:

```toml
blossom-rs = { version = "0.1", features = ["otel"] }
```

```rust
// Exports to OTEL_EXPORTER_OTLP_ENDPOINT (default: http://localhost:4317)
let _guard = blossom_rs::otel::init_tracing("my-server", "info")?;
```

## Testing

```bash
cargo test --workspace             # 117 tests (unit + integration + property + e2e)
cargo test --workspace --features db-sqlite  # Include SQLite backend tests
cargo llvm-cov --features db-sqlite          # Coverage report
```

### Code Coverage — 94.3% library line coverage

| Module | Lines | Coverage |
|--------|-------|----------|
| `server/nip96.rs` | 280 | **99.3%** |
| `db/memory.rs` | 205 | **96.6%** |
| `labels/mod.rs` | 81 | **96.3%** |
| `server/mod.rs` | 569 | **96.0%** |
| `protocol.rs` | 115 | **95.7%** |
| `stats.rs` | 119 | **95.8%** |
| `storage/memory.rs` | 53 | **94.3%** |
| `auth/mod.rs` | 124 | **93.6%** |
| `db/sqlite.rs` | 317 | **92.7%** |
| `storage/filesystem.rs` | 86 | **90.7%** |
| `auth/signer.rs` | 71 | **90.1%** |
| `client/mod.rs` | 28 | **89.3%** |
| `access/mod.rs` | 119 | **88.2%** |
| **Total** | **2252** | **94.3%** |

## CI/CD

- **CI**: On push/PR to master — `cargo fmt --all --check`, `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace`
- **Publish**: On `v*` tags — test, then publish `blossom-rs` → `blossom-server` + `blossom-cli` to crates.io
- Self-hosted runner for trusted pushes; GitHub-hosted for fork PRs

## Acknowledgments

This library draws on patterns and design decisions from:

- **[route96](https://github.com/v0l/route96)** by v0l — Rust Blossom/NIP-96 server implementation. Reference for NIP-96 protocol, database schema patterns, media processing pipeline, EXIF validation, content labeling traits, whitelist access control, and file statistics. Licensed under MIT.

- **[hzrd149/blossom](https://github.com/hzrd149/blossom)** — The Blossom protocol specification (BUD-01 through BUD-06).

- The broader [Nostr](https://github.com/nostr-protocol/nostr) ecosystem for NIP-01, NIP-96, NIP-98, and BIP-340 standards.

## License

MIT
