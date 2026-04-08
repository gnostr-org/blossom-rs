# blossom-nip34

NIP-34 Nostr relay + GRASP git server library for [Blossom](https://github.com/MonumentalSystems/blossom-rs).

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Adds decentralized git hosting capabilities to any axum application. Provides a mountable router with:

- **Nostr relay** — NIP-34 event storage (repos, patches, issues, status) via WebSocket
- **GRASP git server** — HTTP smart protocol for `git clone`/`git push`
- **NIP-11** — relay information document
- **Push auth** — Nostr signature verification for `git push` (only repo owner can push)

## Quick Start

```rust
use blossom_nip34::{Nip34Config, build_nip34_router};

let config = Nip34Config {
    domain: "git.example.com".into(),
    ..Default::default()
};

let nip34_router = build_nip34_router(config).await?;

// Merge with your existing axum app
let app = your_app.merge(nip34_router);
```

## With blossom-server

```bash
cargo install blossom-server --features nip34

blossom-server --nip34 --nip34-domain git.example.com
```

This enables:
- Nostr relay at `wss://git.example.com/` (WebSocket)
- NIP-11 at `https://git.example.com/` (with `Accept: application/nostr+json`)
- Git repos at `https://git.example.com/{npub}/{repo}.git`
- PKARR `_nostr` TXT record for relay discovery (with `--pkarr`)

## NIP-34 Event Kinds

| Kind | Event | Description |
|------|-------|-------------|
| 30617 | Repository Announcement | Repo metadata, clone URLs, relay list |
| 30618 | Repository State | Branch/tag refs with commit SHAs |
| 1617 | Patch | `git format-patch` content |
| 1618 | Pull Request | Larger submissions with descriptions |
| 1619 | PR Update | Modifications to existing PRs |
| 1621 | Issue | Bug reports, feature requests |
| 1630 | Status: Open | |
| 1631 | Status: Applied | |
| 1632 | Status: Closed | |
| 1633 | Status: Draft | |
| 10317 | User Grasp List | Preferred grasp servers |

## Git HTTP Protocol

| Endpoint | Method | Auth | Description |
|----------|--------|------|-------------|
| `/{npub}/{repo}/info/refs` | GET | No | Advertise refs |
| `/{npub}/{repo}/git-upload-pack` | POST | No | Fetch objects (clone/pull) |
| `/{npub}/{repo}/git-receive-pack` | POST | Yes | Push objects |

Push authentication uses `Authorization: Nostr <base64-encoded-event>` where the event must be signed by the repo owner's key (matching the npub in the URL).

## GRASP Validation

Repository announcements (kind 30617) are validated:
- Must have `d` tag (identifier): 1-30 chars, alphanumeric + hyphens + underscores
- Must have at least one `clone` URL tag
- Must have at least one `relays` tag

Repository state events (kind 30618) are validated:
- Must have `HEAD` ref tag
- All ref SHA values must be valid 40-char hex

## Architecture

```
Nostr clients (ngit, nak, gitworkshop.dev)
     │ WebSocket (NIP-01)
     ▼
blossom-nip34 relay
     │ LMDB (nostr-lmdb)
     │ GRASP plugins (validation)
     ▼
NIP-34 events stored
     │
     │  On kind 30617 → create bare git repo
     ▼
Git HTTP server
     │ stateless-rpc (git CLI)
     ▼
/{npub}/{repo}.git (bare repos on filesystem)
```

## Dependencies

All MIT-licensed:
- `nostr` 0.44 — Nostr protocol types
- `nostr-relay-builder` 0.44 — relay construction
- `nostr-lmdb` 0.44 — LMDB event storage
- `axum` 0.8 — HTTP framework

## License

MIT
