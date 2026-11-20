#![forbid(unsafe_code)]
//! Pure safe-Rust SHA-256 implementation and canonical JSON serializer.
//!
//! This module intentionally avoids external cryptography crates so
//! `gestalt-kernel` stays at its 2-dependency ceiling (`serde`, `serde_json`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

const INITIAL_STATE: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Streaming FIPS 180-4 SHA-256 hasher using fixed-size stack buffers.
#[derive(Debug, Clone)]
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// Create a new SHA-256 hasher initialized with FIPS 180-4 IV constants.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            block: [0u8; 64],
            block_len: 0,
            total_bytes: 0,
        }
    }

    /// Feed an arbitrary byte slice into the hasher.
    ///
    /// # Arguments
    ///
    /// * `data` - Input bytes to absorb into the digest state
    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(data.len() as u64);

        if self.block_len > 0 {
            let take = (64 - self.block_len).min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];
            if self.block_len == 64 {
                let full_block = self.block;
                self.compress(&full_block);
                self.block_len = 0;
            }
        }

        while data.len() >= 64 {
            let (chunk, rest) = data.split_at(64);
            // `chunk` is guaranteed 64 bytes by the loop condition.
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.compress(&block);
            data = rest;
        }

        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.block_len = data.len();
        }
    }

    /// Consume the hasher and return the 32-byte SHA-256 digest.
    #[must_use]
    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.block[self.block_len] = 0x80;
        self.block_len += 1;

        if self.block_len > 56 {
            self.block[self.block_len..].fill(0);
            let full_block = self.block;
            self.compress(&full_block);
            self.block.fill(0);
        } else {
            self.block[self.block_len..56].fill(0);
        }

        self.block[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let final_block = self.block;
        self.compress(&final_block);

        let mut out = [0u8; 32];
        for (idx, word) in self.state.iter().enumerate() {
            out[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (idx, chunk) in block.chunks_exact(4).enumerate() {
            w[idx] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(ROUND_CONSTANTS[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        let next = [a, b, c, d, e, f, g, h];
        for (acc, delta) in self.state.iter_mut().zip(next) {
            *acc = acc.wrapping_add(delta);
        }
    }
}

/// Compute a lowercase hexadecimal SHA-256 digest of a byte slice.
///
/// # Arguments
///
/// * `data` - Input bytes to hash
///
/// # Returns
///
/// A 64-character lowercase hexadecimal digest string.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Serialize a JSON value deterministically with recursively sorted object keys
/// and zero insignificant whitespace.
///
/// # Arguments
///
/// * `value` - Any `serde_json::Value`
///
/// # Returns
///
/// Canonical compact UTF-8 JSON string suitable for cross-language hashing.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical_json(&mut out, value);
    out
}

fn write_canonical_json(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            // Delegate string escaping to serde_json so RFC 8259 escapes stay exact.
            let escaped = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
            out.push_str(&escaped);
        }
        Value::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                write_canonical_json(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // BTreeMap guarantees lexicographical UTF-8 key ordering even if
            // serde_json's preserve_order feature is enabled by another crate.
            let sorted: BTreeMap<&str, &Value> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            out.push('{');
            for (idx, (k, v)) in sorted.into_iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                let escaped_key = serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string());
                out.push_str(&escaped_key);
                out.push(':');
                write_canonical_json(out, v);
            }
            out.push('}');
        }
    }
}

/// Compute the content-addressed SHA-256 digest for a `SessionTree` node.
///
/// Uses length-prefixed framing (`<parent_len>:<parent>|<kind_len>:<kind>|<canonical_payload>`)
/// so boundary collisions between `parent_id` and `kind` are impossible.
///
/// # Arguments
///
/// * `parent_id` - Optional parent node ID (`None` for the root node)
/// * `kind` - Canonical or namespaced node kind string
/// * `payload` - Node payload JSON value
///
/// # Returns
///
/// A 64-character lowercase hexadecimal SHA-256 digest.
#[must_use]
pub fn compute_node_digest(parent_id: Option<&str>, kind: &str, payload: &Value) -> String {
    let parent = parent_id.unwrap_or("");
    let canonical = canonical_json(payload);
    let framed = format!(
        "{}:{parent}|{}:{kind}|{canonical}",
        parent.len(),
        kind.len()
    );
    sha256_hex(framed.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn canonical_json_is_key_order_independent() {
        let a = json!({"z": 1, "a": {"d": true, "b": [1, {"y": "hi", "x": null}]}});
        let b = json!({"a": {"b": [1, {"x": null, "y": "hi"}], "d": true}, "z": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        assert_eq!(
            compute_node_digest(Some("n0"), "effect", &a),
            compute_node_digest(Some("n0"), "effect", &b)
        );
    }

    #[test]
    fn compute_node_digest_distinguishes_parent_boundaries() {
        let payload = json!({"ok": true});
        assert_ne!(
            compute_node_digest(None, "session_open", &payload),
            compute_node_digest(Some("n0"), "session_open", &payload)
        );
        assert_ne!(
            compute_node_digest(Some("a|b"), "c", &payload),
            compute_node_digest(Some("a"), "b|c", &payload)
        );
    }
}
