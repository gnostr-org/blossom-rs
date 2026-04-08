# BUD-20: LFS-Aware Storage Efficiency

**Status:** Draft
**Version:** 1.0
**Authors:** MonumentalSystems

## Abstract

Defines how a Blossom server can apply compression and delta encoding to
LFS blobs, driven by tags in the kind:24242 auth event. Regular Blossom
uploads are unaffected — no tags means standard behavior. LFS-tagged
uploads are transparently compressed and, when a previous version is
referenced, stored as binary deltas for significant storage savings.

## Motivation

LFS blobs within a repository have known relationships that general blob
storage does not:

- **Version chains** — successive pushes of the same file path produce
  nearly-identical blobs (model weights, datasets, game assets)
- **File path stability** — the same `path` is updated over time
- **Repo grouping** — all blobs belong to a known namespace

These relationships enable two storage optimizations:

1. **zstd compression** — LFS binaries (models, datasets, media) are
   often uncompressed. Compression yields 30-60% savings at modest CPU
   cost.
2. **xdelta3 delta encoding** — binary diffs between versions of the same
   file are typically 5-40% the size of the full blob, yielding 60-95%
   savings on successive pushes.

The server decides storage policy based on tags in the auth event. Clients
continue to see standard Blossom responses — compression and delta
reconstruction are transparent.

## Auth Event Tags

The kind:24242 auth event (BUD-01) carries LFS context as additional tags
on `PUT /upload`. The server reads these tags to decide storage policy.

### Tag Reference

| Tag | Required | Example | Purpose |
|-----|----------|---------|---------|
| `["t", "lfs"]` | For LFS mode | — | Enables LFS storage pipeline |
| `["path", "..."]` | Yes with `lfs` | `assets/model.bin` | File path for version tracking |
| `["repo", "..."]` | Yes with `lfs` | `github.com/org/repo` | Repository namespace |
| `["base", "..."]` | No | `a1b2c3...hex` | SHA-256 of previous version |
| `["manifest"]` | No | tag present | Marks as BUD-17 manifest (skip compression) |

### Example Auth Event

```json
{
  "kind": 24242,
  "tags": [
    ["t", "upload"],
    ["t", "lfs"],
    ["x", "f0b0a0f53884e00a4ddec4a4bc94e23f38d5a774..."],
    ["path", "assets/big-model.bin"],
    ["repo", "github.com/user/repo"],
    ["base", "a1b2c3d4e5f6...previous_sha256"],
    ["expiration", "1712345738"]
  ]
}
```

### Tag Semantics

- **`["t", "lfs"]`** — Client is uploading an LFS blob. Absence of this
  tag means standard (raw) storage.
- **`["path", "..."]`** — The file path within the repository. Used to
  group versions of the same file for delta encoding.
- **`["repo", "..."]`** — Repository identifier. Matches the `repo_id`
  used in BUD-19 lock endpoints. Must be present when `lfs` tag is set.
- **`["base", "..."]`** — SHA-256 hash of the previous version of this
  file. Enables delta encoding against the known base. If the base does
  not exist on the server, the server falls back to full compressed
  storage.
- **`["manifest"]`** — This blob is a BUD-17 manifest. Manifests are
  small JSON documents; compression is counterproductive. Store raw.

### Activation Rules

The server MUST check tags in this order:

1. No `["t", "lfs"]` tag → standard raw storage (no changes)
2. `["t", "lfs"]` + `["manifest"]` → store raw (manifest blob)
3. `["t", "lfs"]` + `["base", "..."]` → attempt delta, fall back to
   compressed
4. `["t", "lfs"]` (no `base`) → full compressed storage

## Storage Pipeline

### Upload

```
PUT /upload (with LFS tags in auth event)
        │
        ▼
  Parse LFS tags from auth event
        │
        ▼
  Has ["manifest"] tag? ──Yes──► Store raw (standard BUD-01)
        │
       No
        │
        ▼
  Has ["base"] tag? ──No──► Compress with zstd ► Store
        │
       Yes
        │
        ▼
  Fetch base blob ──Not found──► Compress with zstd ► Store
        │
      Found
        │
        ▼
  Compute xdelta3 delta (base + new)
        │
        ▼
  Delta > threshold of original?
        │
       Yes ──► Compress with zstd ► Store (full, compressed)
        │
       No
        │
        ▼
  Store delta blob + record in lfs_file_versions
```

### Delta Threshold

A delta is only stored if it is smaller than a configurable fraction of
the original blob size. The default threshold is **0.8** (80%). If the
delta is ≥ 80% of the original size, the server discards the delta and
stores the full compressed blob instead. This avoids the overhead of
delta reconstruction for marginal savings.

### Download (Transparent Reconstruction)

```
GET /<sha256>
        │
        ▼
  Look up sha256 in lfs_file_versions
        │
        ▼
  Storage type?
        │
        ├── "raw" ────────────► Return blob as-is
        │
        ├── "compressed" ─────► zstd decompress ► Return
        │
        └── "delta" ──────────► Walk delta chain (max depth 10)
                                 │
                                 ▼
                               Fetch base + each delta
                                 │
                                 ▼
                               Reconstruct via xdelta3 decode
                                 │
                                 ▼
                               Return full blob
```

Clients see standard Blossom responses. The `GET /<sha256>` endpoint
returns the full decompressed blob regardless of how it is stored. The
`HEAD /<sha256>` endpoint returns the **original** (uncompressed) size
in `Content-Length`, not the stored size.

### Delta Chain Walking

When a delta references a base that is itself a delta, the server walks
the chain recursively:

1. Find the version record for `sha256`
2. If `storage = "delta"`, get `base_sha256`
3. Repeat until `storage != "delta"` or chain depth exceeds limit
4. Reconstruct from the base upward through each delta
5. Maximum chain depth: **10**. Beyond this, the server stores a new
   full compressed blob (rebases the chain).

### Compression

All non-manifest LFS blobs are compressed with **zstd** (Zstandard) at
compression level **3** (default, good speed-to-ratio balance). Delta
blobs are also compressed with zstd after xdelta3 encoding.

Compression and decompression happen on the server side. The
`BlobDescriptor.size` returned to the client reflects the original
(uncompressed) size. The `BlobDescriptor.sha256` is the hash of the
original (uncompressed) content — this is critical because clients
verify uploads by comparing SHA-256.

### SHA-256 Identity

**The SHA-256 hash and size in all API responses MUST refer to the
original, uncompressed content.** Compression and delta encoding are
internal storage details. This ensures:

- Clients can verify integrity using the SHA-256 they computed locally
- `HEAD /<sha256>` returns the original size for progress bars
- BUD-17 manifests reference the original content hashes
- Blob dedup (`PUT /upload` of existing blob) still works correctly

## Database Schema

### Migration V4 — LFS File Versions

```sql
CREATE TABLE IF NOT EXISTS lfs_file_versions (
    repo_id       TEXT NOT NULL,
    path          TEXT NOT NULL,
    version       INTEGER NOT NULL,
    sha256        TEXT NOT NULL,
    base_sha256   TEXT,
    storage       TEXT NOT NULL DEFAULT 'full',
    delta_algo    TEXT,
    original_size INTEGER NOT NULL,
    stored_size   INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (repo_id, path, version)
);
CREATE INDEX IF NOT EXISTS idx_lfs_v_sha ON lfs_file_versions(sha256);
CREATE INDEX IF NOT EXISTS idx_lfs_v_base ON lfs_file_versions(base_sha256);
CREATE INDEX IF NOT EXISTS idx_lfs_v_repo_path ON lfs_file_versions(repo_id, path);
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `repo_id` | TEXT | Repository namespace (from `["repo", ...]` tag) |
| `path` | TEXT | File path within repo (from `["path", ...]` tag) |
| `version` | INTEGER | Monotonically increasing version number per `(repo_id, path)` |
| `sha256` | TEXT | SHA-256 of the original (uncompressed) content |
| `base_sha256` | TEXT | SHA-256 of the base version (NULL for full blobs) |
| `storage` | TEXT | `"full"`, `"compressed"`, or `"delta"` |
| `delta_algo` | TEXT | `"xdelta3"` or NULL |
| `original_size` | INTEGER | Size of the original (uncompressed) blob |
| `stored_size` | INTEGER | Size of the blob as stored (after compression/delta) |
| `created_at` | INTEGER | Unix timestamp of upload |

### Upload Record Extension

The existing `UploadRecord` is extended with two optional fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `storage` | TEXT | `"raw"` | `"raw"`, `"compressed"`, or `"delta"` |
| `compression_algo` | TEXT | NULL | `"zstd"` or NULL |

These fields allow the storage backend to determine how to read back a
blob without querying `lfs_file_versions` for non-LFS uploads.

## Storage Layout

### Filesystem Backend

```
data_dir/
  blobs/
    <sha256>.blob              # Full blobs (raw or zstd-compressed)
  deltas/
    <sha256>.delta             # Delta blobs (xdelta3 + zstd)
```

The filesystem backend checks for a blob in `blobs/` first, then
`deltas/`. If found in `deltas/`, it reconstructs the full blob by
walking the delta chain before returning it.

### S3 Backend

S3 uses key prefixes instead of directories:

| Key | Content |
|-----|---------|
| `blobs/<sha256>` | Full blobs (raw or zstd-compressed) |
| `deltas/<sha256>` | Delta blobs (xdelta3 + zstd) |

### Memory Backend

The in-memory backend stores all blobs in a single `HashMap<String,
Vec<u8>>`, tagged with storage metadata. No layout changes needed.

## Server Behavior

### `PUT /upload` (Modified)

When LFS tags are present in the auth event:

1. Parse `["t", "lfs"]`, `["path", "..."]`, `["repo", "..."]`,
   `["base", "..."]`, and `["manifest"]` tags
2. Compute SHA-256 of the uploaded data (this is the content identity)
3. If blob already exists (dedup check), skip upload, return existing
   descriptor
4. If `["manifest"]` tag is present, store raw (no compression)
5. If `["base", "..."]` tag is present:
   a. Look up `base_sha256` in `lfs_file_versions`
   b. If base exists as a full/compressed blob, fetch it
   c. Compute xdelta3 delta
   d. If delta < threshold × original_size, compress delta with zstd,
      store in `deltas/`, record in `lfs_file_versions` with
      `storage = "delta"`
   e. If delta ≥ threshold, fall through to compressed storage
6. Compress with zstd, store in `blobs/`, record in `lfs_file_versions`
   with `storage = "compressed"`
7. Return `BlobDescriptor` with original (uncompressed) `sha256` and
   `size`

### `GET /<sha256>` (Modified)

Before returning a blob:

1. Check if blob exists in `blobs/` — if so, decompress if needed,
   return
2. Check if blob exists in `deltas/` — if so, reconstruct:
   a. Walk delta chain via `lfs_file_versions`
   b. Fetch base blob (decompress if needed)
   c. Apply each delta in order via xdelta3 decode
   d. Return reconstructed blob
3. Return 404 if not found in either location

### `HEAD /<sha256>` (Modified)

Returns the **original** (uncompressed) `Content-Length`, not the stored
size. The server looks up `original_size` from `lfs_file_versions` or
the upload record.

### `DELETE /<sha256>` (Modified)

When deleting an LFS blob:

1. Check `lfs_file_versions` for any deltas that reference this blob as
   a base
2. If deltas exist, reconstruct those deltas into full blobs before
   deleting the base (otherwise the delta chain breaks)
3. Delete the blob and its version records

### Admin Endpoints

A new admin endpoint provides storage efficiency metrics:

```
GET /admin/lfs-stats
```

```json
{
  "total_versions": 42,
  "total_original_bytes": 1073741824,
  "total_stored_bytes": 268435456,
  "compression_ratio": 0.25,
  "by_storage_type": {
    "full": { "count": 10, "original_bytes": 536870912, "stored_bytes": 536870912 },
    "compressed": { "count": 15, "original_bytes": 322122547, "stored_bytes": 161061273 },
    "delta": { "count": 17, "original_bytes": 214748365, "stored_bytes": 37580963 }
  }
}
```

## Client Behavior (blossom-lfs)

The `blossom-lfs` daemon adds LFS tags to its auth events when uploading
blobs. This is a client-side change only — the server decides storage
policy.

### Tag Construction

When uploading a blob, the daemon:

1. Always includes `["t", "lfs"]`
2. Includes `["path", "<file-path>"]` if the file path is known
3. Includes `["repo", "<repo-slug>"]` using the same slug as BUD-19
4. Includes `["base", "<previous-sha256>"]` if the file was previously
   tracked (the daemon looks up the previous version's SHA-256 from git
   history)
5. Includes `["manifest"]` when uploading a BUD-17 manifest blob

### Base Version Lookup

The daemon can determine the previous version's SHA-256 by checking:

1. `git log --follow -1 --format=%H -- <path>` to find the previous
   commit touching this file
2. `git lfs pointer --file <path>` in that commit to extract the LFS OID
   (SHA-256)

If no previous version exists (new file), the `base` tag is omitted.

## Backward Compatibility

- **Servers that do not implement BUD-20** ignore unknown tags in the
  auth event. LFS uploads work normally (stored raw, no compression).
- **Clients that do not send LFS tags** get standard raw storage.
  No behavioral change.
- **BUD-17 manifests** are explicitly excluded from compression via the
  `["manifest"]` tag.
- **BUD-19 locks** are independent and unaffected.

## Security Considerations

- LFS tags in auth events do not grant additional permissions. Standard
  Blossom auth (kind:24242 signature verification, expiration, access
  control) applies unchanged.
- Delta chain walking is bounded (max depth 10) to prevent DoS via
  pathological chains.
- The `original_size` field is trusted (set by the server, not the
  client) to prevent Content-Length spoofing.
- Delta reconstruction failures fall back to 500 Internal Server Error
  with a log message. No partial or corrupted data is returned.

## Open Questions

- **xdelta3 crate**: Evaluate `xdelta3` crate (C library binding) vs a
  pure-Rust implementation. The C binding is well-tested but adds a
  native dependency.
- **Delta chain cache**: Should reconstructed blobs be cached in memory
  or on disk? Tradeoff between disk I/O and memory usage for large blobs.
- **Chain depth**: Is 10 the right default? Deep chains save storage but
  increase reconstruction latency.
- **Admin stats endpoint**: Should this be part of BUD-20 or a separate
  admin API spec?
- **S3 delta packing**: Should deltas for the same `(repo_id, path)` be
  packed into a single S3 object prefix for listing efficiency?
