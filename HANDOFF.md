# blossom-rs Handoff

## What Was Built (Phase 1 MVP)

A standalone, embeddable Blossom (BUD-01) server library for Rust with async client
and BIP-340 Nostr auth. 19 tests, ~800 lines of library code across 9 source files.

### Module Map

```
src/
├── lib.rs              — Public API, feature-gated re-exports
├── protocol.rs         — NostrEvent, compute_event_id, BlobDescriptor, base64url, sha256_hex
├── auth/
│   ├── mod.rs          — build_blossom_auth, auth_header_value, verify_blossom_auth, AuthError
│   └── signer.rs       — BlossomSigner trait, default Signer (secp256k1 BIP-340)
├── storage/
│   ├── mod.rs          — BlobBackend trait, make_descriptor helper
│   ├── memory.rs       — MemoryBackend (HashMap, for testing/embedded)
│   ├── filesystem.rs   — FilesystemBackend (<sha256>.blob files, restart-safe index scan)
│   └── s3.rs           — Stub (Phase 2)
├── server/
│   └── mod.rs          — BlobServer (Axum router), PUT/GET/HEAD/DELETE + /status,
│                         optional auth enforcement, S3-compat router (feature-gated)
└── client/
    └── mod.rs          — BlossomClient with multi-server failover, SHA256 integrity checks
```

### Feature Flags

| Flag | Default | What |
|------|---------|------|
| `server` | yes | Axum server (BlobServer, router) |
| `client` | yes | reqwest-based BlossomClient |
| `filesystem` | yes | FilesystemBackend |
| `s3` | no | S3/R2 backend (stub) |
| `s3-compat` | no | S3-compat test router |

### Key Design Decisions

- **`BlossomSigner` trait** instead of hardcoded identity type. Consumers implement
  `public_key_hex() -> String` and `sign_schnorr(&[u8; 32]) -> String`.

- **`BlobBackend` trait** with synchronous interface (insert/get/exists/delete/len/total_bytes).
  Server wraps it in `Arc<Mutex<>>` for async handlers. This matches the commutator
  pattern where the registry adapter uses `std::sync::Mutex`.

- **Unified `BlobDescriptor`** — commutator had two divergent definitions (server and client).
  blossom-rs has one, with all optional fields defaulting via serde.

- **Auth verification implemented** — `verify_blossom_auth()` checks event kind, expiration,
  action tag, event ID hash, and BIP-340 signature. Commutator's server had this as TODO.

- **Axum 0.7** route syntax (`:param` not `{param}`). Matches commutator's current axum version.

---

## What Remains To Build

### Phase 2: Persistence + S3

**S3 Backend** (`src/storage/s3.rs`)
- Implement `BlobBackend` for S3-compatible stores (AWS S3, Cloudflare R2, MinIO)
- Reference: commutator's `blossom_server.rs` lines 232-304 has working `upload_to_s3()`
  and `download_from_s3()` using `aws-sdk-s3` + `aws-config`
- Add `S3Config` struct (endpoint, bucket, access_key, secret_key, region, public_url)
- Background async upload after insert (commutator spawns tokio task, line 446)
- CDN URL support: if `public_url` is set, blob URLs point to CDN instead of server

**Database Layer** (`src/db/`)
- `trait BlobDatabase` for metadata persistence (upload records, user quotas, stats)
- `MemoryDatabase` — in-memory index (current behavior, no persistence beyond filesystem)
- `SqliteDatabase` — SQLx SQLite backend
- `PostgresDatabase` — SQLx Postgres backend
- Reference: route96 uses SQLx with MySQL, has ~18 migrations. Adapt schema to SQLite/Postgres.
  Key tables: `uploads` (sha256, size, mime, uploader_pubkey, created_at),
  `users` (pubkey, quota_bytes), `file_stats` (sha256, egress_bytes, last_accessed)

**Quota Enforcement**
- Per-user storage limits via `BlobDatabase::check_quota()`
- Reference: route96 `src/routes/blossom.rs` checks quota before upload

### Phase 3: Extended Protocols

**BUD-02** (Upload + List)
- `GET /list/<pubkey>` — list blobs uploaded by a pubkey (requires DB)
- Reference: route96 `src/routes/blossom.rs` `handle_list_files()`

**BUD-04** (Mirroring)
- `PUT /mirror` — server fetches blob from remote URL, stores locally
- Reference: route96 `src/routes/blossom.rs` `handle_mirror()`

**BUD-05** (Media Optimization)
- `PUT /media` — upload with server-side compression/conversion
- Reference: route96 `src/processing/` (WebP conversion, thumbnails)

**BUD-06** (Upload Requirements)
- Server advertises upload constraints (max size, allowed types)
- Reference: route96 settings-based upload validation

**NIP-96** (`src/server/nip96.rs`)
- `GET /.well-known/nostr/nip96.json` — server capabilities
- `POST /n96` — file upload with metadata (expiry, caption, alt)
- `GET /n96` — paginated file list
- `DELETE /n96/<sha256>` — file deletion
- Reference: route96 `src/routes/nip96.rs`

**Admin Endpoints** (`src/server/admin.rs`)
- User management, blob management, server stats
- Reference: route96 `src/routes/admin.rs`

### Phase 4: Media + Moderation

**Media Processing** (`src/media/`)
- `trait MediaProcessor` — pluggable image/video processing
- WebP conversion + thumbnails (reference: route96 `src/processing/`)
- Blurhash generation for progressive loading
- EXIF privacy validation — reject images with GPS/serial data
  (reference: route96 `check_for_sensitive_exif()`, nearly standalone function)
- Perceptual hashing for duplicate detection (DCT-based + LSH bands)
  (reference: route96 phash implementation)

**Content Labeling** (`src/labels/`)
- `trait MediaLabeler` — pluggable content classification
- `VitLabeler` — local Vision Transformer inference via Candle
- `LlmLabeler` — remote API-based classification (OpenAI-compatible)
- Reference: route96 `src/processing/labeling.rs` — already trait-based, clean extraction

**Access Control** (`src/access/`)
- `trait AccessControl` — pluggable authorization
- `Whitelist` — pubkey whitelist with hot-reload from file
- Reference: route96 whitelist uses strategy pattern with file watching

**File Statistics** (`src/stats.rs`)
- DashMap accumulator with atomic counters, periodic flush to DB
- Tracks egress_bytes, last_accessed per blob
- Reference: route96 file stats module — nearly independent, uses DashMap + AtomicU64

---

## Reference Implementations

| Component | Commutator Source | Route96 Source |
|-----------|-------------------|----------------|
| BIP-340 signing | `commutator-protocol/src/identity.rs` | `src/auth/blossom.rs` |
| Kind:24242 auth | `commutator-protocol/src/blossom.rs:60-98` | `src/auth/blossom.rs` |
| NIP-98 auth | — | `src/auth/nip98.rs` |
| Blob server | `commutator-server/src/blossom_server.rs` | `src/routes/blossom.rs` |
| Blob client | `commutator-server/src/blossom_client.rs` | — |
| S3 backend | `commutator-server/src/blossom_server.rs:232-304` | — |
| S3 compat router | `commutator-server/src/blossom_server.rs:337-418` | — |
| Filesystem store | `commutator-server/src/blossom_server.rs:93-127` | `src/filesystem.rs` |
| Media processing | — | `src/processing/` |
| Content labeling | — | `src/processing/labeling.rs` |
| EXIF validation | — | `src/processing/exif.rs` |
| Perceptual hash | — | `src/processing/phash.rs` |
| Whitelist | — | `src/whitelist.rs` |
| File statistics | — | `src/file_stats.rs` |
| NIP-96 protocol | — | `src/routes/nip96.rs` |
| Admin endpoints | — | `src/routes/admin.rs` |
| DB (adapt to SQLite) | — | `src/db.rs` + `migrations/` |

Route96 repo: https://github.com/v0l/route96 (Rust, Axum 0.8, MySQL/SQLx)
