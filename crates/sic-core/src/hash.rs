//! SHA-256, and the digest type built on it.
//!
//! Written by hand, like everything else here. It is needed in more than one
//! place - the execution journal records digests instead of values so that a
//! secret cannot reach telemetry by default, and the bytecode format has a
//! signature section waiting for it - so a real cryptographic hash is worth the
//! hundred lines rather than a fast non-cryptographic one.
//!
//! The implementation follows FIPS 180-4 directly and is checked against the
//! published test vectors.

/// A 256-bit digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    pub fn of(bytes: &[u8]) -> Digest {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finish()
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuilds a digest that was stored somewhere. This says nothing about
    /// what it is the digest of; comparing it with one that was computed is
    /// what gives it meaning.
    pub fn from_bytes(bytes: [u8; 32]) -> Digest {
        Digest(bytes)
    }

    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.0 {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xF) as usize] as char);
        }
        out
    }

    /// The first eight hex characters, for a log line a person has to read.
    pub fn short(&self) -> String {
        self.hex()[..8].to_string()
    }

    /// Reads back what `hex` wrote: exactly 64 hex characters, and nothing
    /// else. Either case, because a digest is a number and a person retyping
    /// one should not be told it is the wrong number.
    ///
    /// This is the half a digest needs to be readable as well as writable, and
    /// it belongs here for the reason `hex` does. Without it every reader
    /// decided for itself what a sha256 is, and one of those readers decides
    /// whether a capability grant's binary pin is accepted.
    pub fn from_hex(text: &str) -> Option<Digest> {
        if text.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, slot) in bytes.iter_mut().enumerate() {
            // `get` rather than a slice: the length above is in bytes, and a
            // multi-byte character would make the boundary fall inside one.
            *slot = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Digest(bytes))
    }

    /// Reads back what `Display` wrote: `sha256:` and 64 hex characters.
    ///
    /// The prefix is required, not tolerated. Everything sic writes carries it,
    /// and a reader that also accepted a bare hex string would accept a field
    /// that had lost its prefix to a bug as if nothing had happened. Where a
    /// person types the digest instead - `sic upgrade --sha256` - the command
    /// makes the prefix optional itself, and that is a decision about a command
    /// line rather than about the format.
    pub fn parse(text: &str) -> Option<Digest> {
        Digest::from_hex(text.strip_prefix("sha256:")?)
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sha256:{}", self.hex())
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The round constants: the first 32 bits of the fractional parts of the cube
/// roots of the first 64 primes.
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

/// An incremental SHA-256.
#[derive(Debug, Clone)]
pub struct Sha256 {
    /// The eight working variables: the first 32 bits of the fractional parts
    /// of the square roots of the first eight primes.
    state: [u32; 8],
    /// Bytes that have not filled a block yet.
    buffer: [u8; 64],
    buffered: usize,
    /// Total message length, needed for the length padding at the end.
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);

        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            } else {
                // The block is still short, which means `data` is now empty.
                // Falling through would reset `buffered` from the remainder.
                return;
            }
        }

        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.compress(&block);
        }

        let rest = chunks.remainder();
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    pub fn finish(mut self) -> Digest {
        // Pad with 0x80, then zeros, then the length in bits as a big-endian
        // u64, so that the total is a whole number of blocks.
        let bit_length = self.length.wrapping_mul(8);
        self.update_no_count(&[0x80]);
        while self.buffered != 56 {
            self.update_no_count(&[0x00]);
        }
        self.update_no_count(&bit_length.to_be_bytes());

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        Digest(out)
    }

    /// Feeds padding, which must not count towards the message length.
    fn update_no_count(&mut self, data: &[u8]) {
        let before = self.length;
        self.update(data);
        self.length = before;
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
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
                .wrapping_add(K[i])
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

        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vectors published with FIPS 180-4.
    #[test]
    fn known_vectors() {
        assert_eq!(
            Digest::of(b"").hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Digest::of(b"abc").hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            Digest::of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_million_a_characters() {
        // Exercises the block loop and the length padding at a size where an
        // off-by-one in either would show.
        let mut h = Sha256::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(
            h.finish().hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn incremental_matches_one_shot() {
        let data: Vec<u8> = (0..200u32).map(|i| i as u8).collect();
        for split in [0, 1, 55, 56, 57, 63, 64, 65, 128, 199, 200] {
            let mut h = Sha256::new();
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.finish(), Digest::of(&data), "split at {split}");
        }
    }

    #[test]
    fn display_and_short_forms() {
        let d = Digest::of(b"abc");
        assert!(d.to_string().starts_with("sha256:ba7816bf"));
        assert_eq!(d.short(), "ba7816bf");
    }

    /// The two writers each have a reader, and each reader takes back exactly
    /// what its writer produced.
    #[test]
    fn each_written_form_reads_back() {
        let d = Digest::of(b"abc");
        assert_eq!(Digest::from_hex(&d.hex()), Some(d));
        assert_eq!(Digest::parse(&d.to_string()), Some(d));
    }

    /// A digest is a number, so the case it was typed in is not part of it.
    #[test]
    fn hex_is_read_in_either_case() {
        let d = Digest::of(b"abc");
        assert_eq!(Digest::from_hex(&d.hex().to_ascii_uppercase()), Some(d));
        assert_eq!(
            Digest::parse(&format!("sha256:{}", d.hex().to_ascii_uppercase())),
            Some(d)
        );
    }

    /// The prefix is required by `parse` and refused by `from_hex`, which is
    /// what keeps a field that lost its prefix from reading as a digest.
    #[test]
    fn the_prefix_belongs_to_one_of_the_two() {
        let hex = Digest::of(b"abc").hex();
        assert_eq!(Digest::parse(&hex), None);
        assert_eq!(Digest::from_hex(&format!("sha256:{hex}")), None);
    }

    #[test]
    fn anything_that_is_not_sixty_four_hex_characters_is_refused() {
        let hex = Digest::of(b"abc").hex();
        assert_eq!(Digest::from_hex(""), None);
        assert_eq!(Digest::from_hex(&hex[..63]), None);
        assert_eq!(Digest::from_hex(&format!("{hex}0")), None);
        // Sixty-four characters, one of which is not a hex digit.
        assert_eq!(Digest::from_hex(&format!("{}z", &hex[..63])), None);
        // Sixty-four *bytes* that are fewer than 64 characters. The length
        // check passes and the boundary between two digits falls inside a
        // character, which is why the digits are taken with `get`.
        let multibyte = format!("{}\u{3042}", &hex[..61]);
        assert_eq!(multibyte.len(), 64);
        assert_eq!(Digest::from_hex(&multibyte), None);
    }
}
