//! Versioned integrity fingerprints used by persisted Codocia artifacts.

use sha2::{Digest, Sha256};

pub(crate) const SHA256_ALGORITHM: &str = "sha256";
const SHA256_PREFIX: &str = "sha256:";
const SHA256_HEX_LEN: usize = 64;

/// Hash bytes using SHA-256 and include the algorithm in the serialized value.
pub(crate) fn sha256_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{SHA256_PREFIX}{digest:x}")
}

/// Accept only the canonical, algorithm-qualified SHA-256 representation.
///
/// Persisted 64-bit FNV values and unknown future algorithms deliberately fail
/// this check instead of being guessed or silently upgraded.
pub(crate) fn is_sha256_fingerprint(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(SHA256_PREFIX) else {
        return false;
    };
    hex.len() == SHA256_HEX_LEN
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Length-delimited builder for deterministic compound fingerprints.
pub(crate) struct Sha256Fingerprint {
    hasher: Sha256,
}

impl Sha256Fingerprint {
    pub(crate) fn new(domain: &[u8]) -> Self {
        let mut value = Self {
            hasher: Sha256::new(),
        };
        value.field(domain);
        value
    }

    pub(crate) fn field(&mut self, bytes: &[u8]) {
        self.hasher.update((bytes.len() as u64).to_be_bytes());
        self.hasher.update(bytes);
    }

    pub(crate) fn finish(self) -> String {
        format!("{SHA256_PREFIX}{:x}", self.hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_fingerprint_is_algorithm_qualified_and_known_correct() {
        assert_eq!(
            sha256_fingerprint(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn validator_rejects_legacy_and_noncanonical_values() {
        assert!(is_sha256_fingerprint(&sha256_fingerprint(b"content")));
        assert!(!is_sha256_fingerprint("e71fa2190541574b"));
        assert!(!is_sha256_fingerprint(
            "sha256:BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        ));
        assert!(!is_sha256_fingerprint(
            "blake3:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        ));
    }

    #[test]
    fn compound_fields_are_length_delimited() {
        let mut left = Sha256Fingerprint::new(b"test");
        left.field(b"ab");
        left.field(b"c");
        let mut right = Sha256Fingerprint::new(b"test");
        right.field(b"a");
        right.field(b"bc");

        assert_ne!(left.finish(), right.finish());
    }
}
