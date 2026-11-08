//! G.711 (11/1988), mu-law table; RTP PCMU profile RFC 3551 (July 2003), §4.5.14.
//! Original implementation from the standard, not a third-party codec implementation.
pub fn encode(sample: f32) -> u8 {
    let sample = if sample.is_finite() { sample.clamp(-1.0, 1.0) } else { 0.0 };
    let linear = (sample * 32767.0).round() as i32;
    let mask = if linear < 0 { 0x7f } else { 0xff };
    let magnitude = (linear.abs().min(32635) + 132) as u32;
    let segment = (31u32.saturating_sub(magnitude.leading_zeros()).saturating_sub(7)).min(7);
    let interval = (magnitude >> (segment + 3)) & 15;
    ((segment << 4 | interval) as u8) ^ mask
}
pub fn decode(code: u8) -> f32 {
    let bits = !code;
    let magnitude = (((i32::from(bits & 15) << 3) + 132) << ((bits >> 4) & 7)) - 132;
    let value = if bits & 0x80 != 0 { -magnitude } else { magnitude };
    value as f32 / 32768.0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_endpoints_and_all_codewords_are_bounded() {
        assert_eq!(encode(0.0), 255);
        assert_eq!(encode(1.0), 128);
        assert_eq!(encode(-1.0), 0);
        assert_eq!(decode(255), 0.0);
        assert_eq!(decode(127), 0.0);
        assert_eq!(decode(128), 32124.0 / 32768.0);
        assert_eq!(decode(0), -32124.0 / 32768.0);
        for code in 0..=255 {
            let value = decode(code);
            assert!(value.is_finite() && value.abs() <= 1.0);
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(encode(value), 255);
        }
    }
}
