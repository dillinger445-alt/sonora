//! Fast non-cryptographic hashing (XXH64) used for file identity, exact
//! duplicate detection and decoded-audio identity. Two independently seeded
//! XXH64 streams are combined into a 128-bit digest, which makes accidental
//! collisions negligible for libraries of any realistic size.

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;
const P5: u64 = 0x27D4_EB2F_1656_67C5;

#[inline]
fn round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(P2)).rotate_left(31).wrapping_mul(P1)
}

#[inline]
fn merge_round(acc: u64, val: u64) -> u64 {
    let val = round(0, val);
    (acc ^ val).wrapping_mul(P1).wrapping_add(P4)
}

#[inline]
fn read_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

#[inline]
fn read_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Streaming XXH64.
#[derive(Clone)]
pub struct Xxh64 {
    seed: u64,
    v: [u64; 4],
    buf: [u8; 32],
    buf_len: usize,
    total: u64,
}

impl Xxh64 {
    pub fn new(seed: u64) -> Self {
        Xxh64 {
            seed,
            v: [
                seed.wrapping_add(P1).wrapping_add(P2),
                seed.wrapping_add(P2),
                seed,
                seed.wrapping_sub(P1),
            ],
            buf: [0; 32],
            buf_len: 0,
            total: 0,
        }
    }

    #[inline]
    fn stripe(&mut self, b: &[u8]) {
        self.v[0] = round(self.v[0], read_u64(&b[0..]));
        self.v[1] = round(self.v[1], read_u64(&b[8..]));
        self.v[2] = round(self.v[2], read_u64(&b[16..]));
        self.v[3] = round(self.v[3], read_u64(&b[24..]));
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        if self.buf_len > 0 {
            let need = 32 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len < 32 {
                return;
            }
            let b = self.buf;
            self.stripe(&b);
            self.buf_len = 0;
        }
        while data.len() >= 32 {
            self.stripe(&data[..32]);
            data = &data[32..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn digest(&self) -> u64 {
        let mut h = if self.total >= 32 {
            let [v1, v2, v3, v4] = self.v;
            let mut h = v1
                .rotate_left(1)
                .wrapping_add(v2.rotate_left(7))
                .wrapping_add(v3.rotate_left(12))
                .wrapping_add(v4.rotate_left(18));
            h = merge_round(h, v1);
            h = merge_round(h, v2);
            h = merge_round(h, v3);
            merge_round(h, v4)
        } else {
            self.seed.wrapping_add(P5)
        };
        h = h.wrapping_add(self.total);
        let mut rest = &self.buf[..self.buf_len];
        while rest.len() >= 8 {
            let k1 = round(0, read_u64(rest));
            h ^= k1;
            h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
            rest = &rest[8..];
        }
        if rest.len() >= 4 {
            h ^= (read_u32(rest) as u64).wrapping_mul(P1);
            h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
            rest = &rest[4..];
        }
        for &byte in rest {
            h ^= (byte as u64).wrapping_mul(P5);
            h = h.rotate_left(11).wrapping_mul(P1);
        }
        h ^= h >> 33;
        h = h.wrapping_mul(P2);
        h ^= h >> 29;
        h = h.wrapping_mul(P3);
        h ^= h >> 32;
        h
    }
}

/// One-shot XXH64.
pub fn xxh64(data: &[u8], seed: u64) -> u64 {
    let mut h = Xxh64::new(seed);
    h.update(data);
    h.digest()
}

/// 128-bit digest built from two XXH64 streams with different seeds.
#[derive(Clone)]
pub struct Hash128 {
    a: Xxh64,
    b: Xxh64,
}

impl Default for Hash128 {
    fn default() -> Self {
        Self::new()
    }
}

impl Hash128 {
    pub fn new() -> Self {
        Hash128 { a: Xxh64::new(0x534F_4E4F_5241_0001), b: Xxh64::new(0x9E6D_3B1C_77A5_2F10) }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.a.update(data);
        self.b.update(data);
    }

    /// Lower-case hex string (32 characters).
    pub fn hex(&self) -> String {
        format!("{:016x}{:016x}", self.a.digest(), self.b.digest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_vectors() {
        assert_eq!(xxh64(b"", 0), 0xEF46_DB37_51D8_E999);
        assert_eq!(xxh64(b"Nobody inspects the spammish repetition", 0), 0xfbce_a83c_8a37_8bf1);
    }

    #[test]
    fn streaming_matches_oneshot() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let whole = xxh64(&data, 7);
        for split in [0usize, 1, 5, 31, 32, 33, 64, 999, 9999] {
            let mut h = Xxh64::new(7);
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.digest(), whole, "split {split}");
        }
        let mut byte_wise = Xxh64::new(7);
        for b in &data {
            byte_wise.update(std::slice::from_ref(b));
        }
        assert_eq!(byte_wise.digest(), whole);
    }

    #[test]
    fn hash128_hex() {
        let mut h = Hash128::new();
        h.update(b"abc");
        assert_eq!(h.hex().len(), 32);
        let mut h2 = Hash128::new();
        h2.update(b"abd");
        assert_ne!(h.hex(), h2.hex());
    }
}
