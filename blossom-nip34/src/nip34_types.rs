//! NIP-34 event kind constants and helpers.
//!
//! Defined from the public NIP-34 specification:
//! <https://github.com/nostr-protocol/nips/blob/master/34.md>

use nostr::Kind;

// NIP-34 event kinds
pub const REPO_ANNOUNCEMENT: Kind = Kind::Custom(30617);
pub const REPO_STATE: Kind = Kind::Custom(30618);
pub const PATCH: Kind = Kind::Custom(1617);
pub const PULL_REQUEST: Kind = Kind::Custom(1618);
pub const PULL_REQUEST_UPDATE: Kind = Kind::Custom(1619);
pub const ISSUE: Kind = Kind::Custom(1621);
pub const STATUS_OPEN: Kind = Kind::Custom(1630);
pub const STATUS_APPLIED: Kind = Kind::Custom(1631);
pub const STATUS_CLOSED: Kind = Kind::Custom(1632);
pub const STATUS_DRAFT: Kind = Kind::Custom(1633);
pub const USER_GRASP_LIST: Kind = Kind::Custom(10317);

/// All NIP-34 event kinds accepted by a GRASP relay.
pub const NIP34_KINDS: &[Kind] = &[
    REPO_ANNOUNCEMENT,
    REPO_STATE,
    PATCH,
    PULL_REQUEST,
    PULL_REQUEST_UPDATE,
    ISSUE,
    STATUS_OPEN,
    STATUS_APPLIED,
    STATUS_CLOSED,
    STATUS_DRAFT,
    USER_GRASP_LIST,
];

/// Status event kinds (1630-1633).
pub const STATUS_KINDS: &[Kind] = &[STATUS_OPEN, STATUS_APPLIED, STATUS_CLOSED, STATUS_DRAFT];

/// Check if a kind is a NIP-34 event.
pub fn is_nip34_kind(kind: Kind) -> bool {
    NIP34_KINDS.contains(&kind)
}

/// Check if a kind is a status event.
pub fn is_status_kind(kind: Kind) -> bool {
    STATUS_KINDS.contains(&kind)
}
