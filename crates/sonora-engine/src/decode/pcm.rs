//! Raw PCM sample conversion shared by the native WAV and AIFF decoders.

/// Encoding of raw PCM samples in a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmEncoding {
    U8,
    S8,
    S16Le,
    S16Be,
    S24Le,
    S24Be,
    S32Le,
    S32Be,
    F32Le,
    F32Be,
    F64Le,
    F64Be,
}

impl PcmEncoding {
    pub fn bytes(self) -> usize {
        match self {
            PcmEncoding::U8 | PcmEncoding::S8 => 1,
            PcmEncoding::S16Le | PcmEncoding::S16Be => 2,
            PcmEncoding::S24Le | PcmEncoding::S24Be => 3,
            PcmEncoding::S32Le | PcmEncoding::S32Be | PcmEncoding::F32Le | PcmEncoding::F32Be => 4,
            PcmEncoding::F64Le | PcmEncoding::F64Be => 8,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, PcmEncoding::F32Le | PcmEncoding::F32Be | PcmEncoding::F64Le | PcmEncoding::F64Be)
    }

    pub fn container_bits(self) -> u16 {
        (self.bytes() * 8) as u16
    }
}

/// Converts raw interleaved samples to `f32` in `[-1, 1)` (floats pass
/// through unclipped). For integer encodings, ORs every raw sample (left
/// aligned to 32 bits) into `or_mask` so effective bit depth can be measured.
pub fn convert(raw: &[u8], enc: PcmEncoding, out: &mut Vec<f32>, or_mask: &mut u32) {
    let n = raw.len() / enc.bytes();
    out.reserve(n);
    let mut mask = 0u32;
    match enc {
        PcmEncoding::U8 => {
            for &b in raw {
                let v = (b as i32 - 128) as i8;
                mask |= ((v as u8) as u32) << 24;
                out.push(v as f32 / 128.0);
            }
        }
        PcmEncoding::S8 => {
            for &b in raw {
                mask |= (b as u32) << 24;
                out.push((b as i8) as f32 / 128.0);
            }
        }
        PcmEncoding::S16Le | PcmEncoding::S16Be => {
            for c in raw.as_chunks::<2>().0 {
                let v = if enc == PcmEncoding::S16Le { i16::from_le_bytes(*c) } else { i16::from_be_bytes(*c) };
                mask |= ((v as u16) as u32) << 16;
                out.push(v as f32 / 32768.0);
            }
        }
        PcmEncoding::S24Le | PcmEncoding::S24Be => {
            for c in raw.as_chunks::<3>().0 {
                let (b0, b1, b2) = if enc == PcmEncoding::S24Le { (c[0], c[1], c[2]) } else { (c[2], c[1], c[0]) };
                let u = ((b2 as u32) << 24) | ((b1 as u32) << 16) | ((b0 as u32) << 8);
                mask |= u;
                let v = (u as i32) >> 8;
                out.push(v as f32 / 8_388_608.0);
            }
        }
        PcmEncoding::S32Le | PcmEncoding::S32Be => {
            for c in raw.as_chunks::<4>().0 {
                let v = if enc == PcmEncoding::S32Le { i32::from_le_bytes(*c) } else { i32::from_be_bytes(*c) };
                mask |= v as u32;
                out.push((v as f64 / 2_147_483_648.0) as f32);
            }
        }
        PcmEncoding::F32Le | PcmEncoding::F32Be => {
            for c in raw.as_chunks::<4>().0 {
                let v = if enc == PcmEncoding::F32Le { f32::from_le_bytes(*c) } else { f32::from_be_bytes(*c) };
                out.push(if v.is_finite() { v } else { 0.0 });
            }
        }
        PcmEncoding::F64Le | PcmEncoding::F64Be => {
            for c in raw.as_chunks::<8>().0 {
                let v = if enc == PcmEncoding::F64Le { f64::from_le_bytes(*c) } else { f64::from_be_bytes(*c) };
                out.push(if v.is_finite() { v as f32 } else { 0.0 });
            }
        }
    }
    *or_mask |= mask;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s16_and_mask() {
        let raw = [0x00u8, 0x40, 0x00, 0xC0]; // +16384, -16384
        let mut out = Vec::new();
        let mut mask = 0;
        convert(&raw, PcmEncoding::S16Le, &mut out, &mut mask);
        assert_eq!(out, vec![0.5, -0.5]);
        assert_eq!(mask, 0xC000_0000);
    }

    #[test]
    fn s24_sign_extension() {
        let raw = [0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x80];
        let mut out = Vec::new();
        let mut mask = 0;
        convert(&raw, PcmEncoding::S24Le, &mut out, &mut mask);
        assert!((out[0] - (8_388_607.0 / 8_388_608.0)).abs() < 1e-9);
        assert_eq!(out[1], -1.0);
        // Low 8 bits of the 32-bit aligned mask are always zero for 24-bit data.
        assert_eq!(mask & 0xFF, 0);
    }

    #[test]
    fn u8_center() {
        let mut out = Vec::new();
        let mut mask = 0;
        convert(&[128, 0, 255], PcmEncoding::U8, &mut out, &mut mask);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], -1.0);
    }
}
