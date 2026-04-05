//! Property-based tests for blossom-rs.

use proptest::prelude::*;

use blossom_rs::protocol::{base64url_decode, base64url_encode, sha256_hex};

proptest! {
    /// base64url encode/decode is a perfect round-trip for any byte sequence.
    #[test]
    fn base64url_roundtrip(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let encoded = base64url_encode(&data);
        let decoded = base64url_decode(&encoded).unwrap();
        prop_assert_eq!(decoded, data);
    }

    /// base64url encoding never contains `+`, `/`, or `=`.
    #[test]
    fn base64url_no_special_chars(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let encoded = base64url_encode(&data);
        prop_assert!(!encoded.contains('+'));
        prop_assert!(!encoded.contains('/'));
        prop_assert!(!encoded.contains('='));
    }

    /// SHA256 always produces a 64-char hex string.
    #[test]
    fn sha256_hex_length(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let hash = sha256_hex(&data);
        prop_assert_eq!(hash.len(), 64);
        prop_assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// SHA256 is deterministic.
    #[test]
    fn sha256_deterministic(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let h1 = sha256_hex(&data);
        let h2 = sha256_hex(&data);
        prop_assert_eq!(h1, h2);
    }

    /// Different data produces different SHA256 (with overwhelming probability).
    #[test]
    fn sha256_different_data(
        data1 in proptest::collection::vec(any::<u8>(), 1..100),
        data2 in proptest::collection::vec(any::<u8>(), 1..100),
    ) {
        prop_assume!(data1 != data2);
        let h1 = sha256_hex(&data1);
        let h2 = sha256_hex(&data2);
        prop_assert_ne!(h1, h2);
    }
}
