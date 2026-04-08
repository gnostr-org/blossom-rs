//! GRASP validation plugins for NIP-34 events.
//!
//! Clean-room implementation from the NIP-34 public specification.
//! <https://github.com/nostr-protocol/nips/blob/master/34.md>

// TODO: Phase 2 — implement WritePolicy + QueryPolicy plugins:
// - ValidateRepoEvent: d tag, name rules (≤30 chars, alphanumeric+hyphen+underscore), relay/clone URLs
// - ValidateRepoState: HEAD ref present, all refs valid SHA1
// - GraspRepo: domain must appear in relay and clone tags
// - RejectUnauthorizedState: only maintainer can push repo state
// - AcceptMention: accept events referencing accepted repos/patches/issues
