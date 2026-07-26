use super::optimization::SimdScalar;

pub const SIMD128: u16 = 128;
pub const SIMD256: u16 = 256;
pub const SIMD512: u16 = 512;

/// Fixed-width SIMD selected for code generated on this machine. WASM callers
/// deliberately bypass this and request SIMD128, the portable WebAssembly
/// width. `LU_SIMD_WIDTH` is primarily a compiler-testing and benchmarking
/// override; unsupported widths are still legal IR and may be split by the
/// backend.
pub fn native_width_bits() -> u16 {
    if let Ok(value) = std::env::var("LU_SIMD_WIDTH") {
        if let Ok(bits @ (SIMD128 | SIMD256 | SIMD512)) = value.parse() {
            return bits;
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if std::is_x86_feature_detected!("avx512f") {
            return SIMD512;
        }
        if std::is_x86_feature_detected!("avx2") {
            return SIMD256;
        }
    }
    SIMD128
}

pub fn lane_count(bits: u16, scalar: SimdScalar) -> u16 {
    bits / match scalar {
        SimdScalar::F32 => 32,
        SimdScalar::F64 | SimdScalar::I64 => 64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_widths_have_the_expected_lane_shapes() {
        assert_eq!(lane_count(SIMD128, SimdScalar::F32), 4);
        assert_eq!(lane_count(SIMD128, SimdScalar::F64), 2);
        assert_eq!(lane_count(SIMD256, SimdScalar::I64), 4);
        assert_eq!(lane_count(SIMD512, SimdScalar::F32), 16);
    }
}
