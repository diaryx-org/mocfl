//! Content addressing — the digests OCFL keys everything on.
//!
//! In OCFL a digest is not an integrity *check* bolted onto storage; it is the
//! address. The inventory's `manifest` maps a digest to the content paths
//! holding those bytes, and every version's `state` maps a digest to the logical
//! paths the user sees. Deduplication, fixity, and the whole versioning model
//! fall out of that one indirection.
//!
//! ## Spelling
//!
//! OCFL writes a digest as **bare lowercase hex** — no algorithm prefix, because
//! the algorithm is named once in the inventory's `digestAlgorithm` field rather
//! than repeated on every value. That differs from the `sha256:<hex>` form used
//! elsewhere in this family, and the difference is load-bearing: a prefixed
//! digest in an inventory is simply an invalid inventory. [`Digest`] therefore
//! holds the bare form, and callers carrying prefixed digests strip at this
//! boundary.
//!
//! ## Why the hash is hand-rolled
//!
//! Same reason as everywhere else in this family: SHA-256 is a fully specified,
//! deterministic function with published test vectors, so correctness is
//! *checked* rather than trusted, and implementing it here keeps the dependency
//! surface at exactly one crate. The tests below pin it to the FIPS 180-4
//! known-answer vectors.

use std::fmt;

use crate::error::{Error, Result};

/// A digest algorithm an inventory may name in `digestAlgorithm`.
///
/// The spec permits `sha256` or `sha512` for content addressing and recommends
/// `sha512`; both are represented here so an inventory naming either can be
/// *read* and structurally validated. Whether this crate can recompute a digest
/// is a separate question — see [`DigestAlgorithm::is_computable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DigestAlgorithm {
    /// SHA-256. Permitted for content addressing, and the only algorithm this
    /// crate can currently compute.
    Sha256,
    /// SHA-512. Permitted for content addressing and the spec's recommended
    /// default. Recognized so foreign objects parse and validate structurally;
    /// content verification against one is not yet implemented.
    Sha512,
}

impl DigestAlgorithm {
    /// The inventory spelling (`"sha256"` / `"sha512"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    /// Parse an inventory's `digestAlgorithm` value. `None` for anything the
    /// spec does not permit for *content addressing* — notably the weaker
    /// algorithms (md5, sha1) that are legal only inside the `fixity` block.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            _ => None,
        }
    }

    /// Whether this crate can *compute* a digest with this algorithm, as opposed
    /// to merely recognizing it in an inventory.
    ///
    /// The distinction matters for honesty about coverage: an object addressed
    /// with SHA-512 can still be opened, parsed, and structurally validated, but
    /// its content cannot be verified here, and a caller must be able to tell
    /// "verified" from "taken on faith" rather than being told everything is
    /// fine. This mirrors the `is_recognized` seam in the sister crates.
    pub fn is_computable(self) -> bool {
        matches!(self, Self::Sha256)
    }

    /// The number of hex characters a digest of this algorithm occupies.
    pub fn hex_len(self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }

    /// Hash `bytes` with this algorithm.
    ///
    /// Returns [`Error::UnsupportedDigest`] rather than a wrong answer when the
    /// algorithm is recognized but not implemented — never a silent fallback to
    /// a different one.
    pub fn digest(self, bytes: &[u8]) -> Result<Digest> {
        match self {
            Self::Sha256 => Ok(Digest(hex(&sha256(bytes)))),
            Self::Sha512 => Err(Error::UnsupportedDigest(self)),
        }
    }
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A content digest in OCFL's spelling: bare lowercase hex.
///
/// Digests are compared case-insensitively by the spec (an inventory may record
/// uppercase hex), so construction normalizes to lowercase and equality is then
/// ordinary string equality.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(String);

impl Digest {
    /// Wrap a hex string, lowercasing it so equality and map lookups behave.
    ///
    /// Rejects anything that is not pure hex: a digest is a map *key* in the
    /// inventory, and a malformed one would otherwise sit there addressing
    /// nothing.
    pub fn parse(hex: &str) -> Result<Self> {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::MalformedDigest(hex.to_string()));
        }
        Ok(Self(hex.to_ascii_lowercase()))
    }

    /// The bare lowercase hex form — what goes into an inventory.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lowercase hex encoding.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        s.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    s
}

/// The SHA-256 round constants — the first 32 bits of the fractional parts of
/// the cube roots of the first 64 primes (FIPS 180-4 §4.2.2).
#[rustfmt::skip]
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// The initial hash state — the first 32 bits of the fractional parts of the
/// square roots of the first 8 primes (FIPS 180-4 §5.3.3).
const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// The raw SHA-256 digest of `bytes`, as 32 bytes.
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = H0;

    // Pad: the message, a 0x80 byte, zeros, then the bit-length as a 64-bit
    // big-endian integer — to a multiple of 64 bytes (FIPS 180-4 §5.1.1).
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    let mut msg = bytes.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (chunk, word) in out.chunks_exact_mut(4).zip(h) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The NIST / FIPS 180-4 known-answer vectors. If these pass, the
    // implementation is SHA-256 — correctness is checked, not trusted.
    #[test]
    fn matches_the_published_sha256_vectors() {
        let d = |b: &[u8]| DigestAlgorithm::Sha256.digest(b).unwrap().0;
        assert_eq!(
            d(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            d(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            d(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn crosses_a_block_boundary_correctly() {
        let million_a = vec![b'a'; 1_000_000];
        assert_eq!(
            DigestAlgorithm::Sha256.digest(&million_a).unwrap().0,
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn digests_carry_no_algorithm_prefix() {
        // The one spelling difference from the sister crates, and the one that
        // would silently produce invalid inventories if it regressed.
        let d = DigestAlgorithm::Sha256.digest(b"abc").unwrap();
        assert!(!d.as_str().contains(':'), "{d}");
        assert_eq!(d.as_str().len(), DigestAlgorithm::Sha256.hex_len());
    }

    #[test]
    fn digest_comparison_is_case_insensitive() {
        // An inventory may record uppercase hex; it must still match.
        let lower = Digest::parse("ABCDEF01").unwrap();
        assert_eq!(lower.as_str(), "abcdef01");
        assert_eq!(Digest::parse("abcdef01").unwrap(), lower);
    }

    #[test]
    fn a_non_hex_digest_is_refused() {
        // Digests are inventory map *keys*; a malformed one addresses nothing.
        assert!(Digest::parse("sha256:abcd").is_err());
        assert!(Digest::parse("").is_err());
        assert!(Digest::parse("zzzz").is_err());
    }

    #[test]
    fn sha512_is_recognized_but_reports_that_it_cannot_be_computed() {
        // Honesty over silent fallback: a recognized-but-unimplemented algorithm
        // must error, never quietly hash with a different one.
        let alg = DigestAlgorithm::parse("sha512").unwrap();
        assert!(!alg.is_computable());
        assert!(alg.digest(b"abc").is_err());
        assert!(DigestAlgorithm::Sha256.is_computable());
        // md5/sha1 are legal only in the `fixity` block, never for addressing.
        assert!(DigestAlgorithm::parse("md5").is_none());
    }
}
