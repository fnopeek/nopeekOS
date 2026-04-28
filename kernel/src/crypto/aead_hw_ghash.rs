//! 4-way aggregated GHASH using PCLMULQDQ — drop-in for `ghash::GHash`.
//!
//! `ghash 0.5` is built on `polyval 0.6.2` with single-block multiply +
//! reduction per block (~12-15 cycles/block latency-bound on N100).
//! Aggregating 4 blocks lets us share the reduction step:
//!
//!   batch X[0..4] :=
//!     (Y_old ⊕ X[0]) × H⁴
//!         ⊕  X[1]   × H³
//!         ⊕  X[2]   × H²
//!         ⊕  X[3]   × H¹
//!     (one final reduction over the 256-bit accumulator)
//!
//! 4 multiplications + 1 reduction vs 4 × (multiply + reduction). Saves
//! ~3 reductions per 4 blocks → ~1.5–2× GHASH throughput.
//!
//! Wire format compatible with `ghash::GHash` — validated against it
//! per-build via the `disk` bytes-match check. If a byte ever
//! differs, treat as Corrupt and bail.
//!
//! GHASH polynomial: x¹²⁸ + x⁷ + x² + x + 1.
//! Bytes are read big-endian (most-significant power first), so we
//! byte-reverse via `pshufb` at the I/O boundary — exactly what Linux
//! `arch/x86/crypto/ghash-clmulni-intel_asm.S` does.

#![allow(unsafe_op_in_unsafe_fn)]

use core::arch::x86_64::*;

const BSWAP_MASK: [u8; 16] = [15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0];

/// 4-way aggregated GHASH state.
#[derive(Clone, Copy)]
pub struct GHash4 {
    /// Powers of H in the "raw" GF(2¹²⁸) polynomial form (no
    /// pre-shift): h, h², h³, h⁴. All stored in PCLMULQDQ-native
    /// (little-endian) bit order — we byte-reverse incoming blocks
    /// before XOR'ing into the accumulator.
    h: __m128i,
    h2: __m128i,
    h3: __m128i,
    h4: __m128i,
    y: __m128i,
}

/// Carry-less multiply two GF(2¹²⁸) elements with reduction modulo
/// x¹²⁸ + x⁷ + x² + x + 1. Karatsuba: 3 PCLMULQDQ + shifts.
#[inline]
#[target_feature(enable = "sse2,ssse3,pclmulqdq")]
unsafe fn gf128_mul(a: __m128i, b: __m128i) -> __m128i {
    // Schoolbook via Karatsuba — 3 PCLMULQDQ.
    let t0 = _mm_clmulepi64_si128(a, b, 0x00); // a_lo × b_lo
    let t1 = _mm_clmulepi64_si128(a, b, 0x11); // a_hi × b_hi

    // (a_lo + a_hi) × (b_lo + b_hi) — single multiply for the cross term.
    let a_xor = _mm_xor_si128(a, _mm_shuffle_epi32(a, 0xEE));
    let b_xor = _mm_xor_si128(b, _mm_shuffle_epi32(b, 0xEE));
    let mid = _mm_clmulepi64_si128(a_xor, b_xor, 0x00);
    let mid = _mm_xor_si128(mid, _mm_xor_si128(t0, t1));

    // Assemble 256-bit (lo: t0, mid lifted by 64, hi: t1).
    let lo = _mm_xor_si128(t0, _mm_slli_si128(mid, 8));
    let hi = _mm_xor_si128(t1, _mm_srli_si128(mid, 8));

    reduce_256_to_128(lo, hi)
}

/// Aggregate 4 carry-less multiplications into one reduction.
/// Computes (a₀×b₀) ⊕ (a₁×b₁) ⊕ (a₂×b₂) ⊕ (a₃×b₃) mod P.
#[inline]
#[target_feature(enable = "sse2,ssse3,pclmulqdq")]
unsafe fn gf128_mul4(
    a0: __m128i, b0: __m128i,
    a1: __m128i, b1: __m128i,
    a2: __m128i, b2: __m128i,
    a3: __m128i, b3: __m128i,
) -> __m128i {
    macro_rules! kara {
        ($a:expr, $b:expr) => {{
            let t0 = _mm_clmulepi64_si128($a, $b, 0x00);
            let t1 = _mm_clmulepi64_si128($a, $b, 0x11);
            let a_xor = _mm_xor_si128($a, _mm_shuffle_epi32($a, 0xEE));
            let b_xor = _mm_xor_si128($b, _mm_shuffle_epi32($b, 0xEE));
            let mid = _mm_clmulepi64_si128(a_xor, b_xor, 0x00);
            let mid = _mm_xor_si128(mid, _mm_xor_si128(t0, t1));
            let lo = _mm_xor_si128(t0, _mm_slli_si128(mid, 8));
            let hi = _mm_xor_si128(t1, _mm_srli_si128(mid, 8));
            (lo, hi)
        }};
    }

    let (lo0, hi0) = kara!(a0, b0);
    let (lo1, hi1) = kara!(a1, b1);
    let (lo2, hi2) = kara!(a2, b2);
    let (lo3, hi3) = kara!(a3, b3);

    let lo = _mm_xor_si128(_mm_xor_si128(lo0, lo1), _mm_xor_si128(lo2, lo3));
    let hi = _mm_xor_si128(_mm_xor_si128(hi0, hi1), _mm_xor_si128(hi2, hi3));

    reduce_256_to_128(lo, hi)
}

/// Reduce a 256-bit polynomial (lo + hi·x¹²⁸) modulo
/// P = x¹²⁸ + x⁷ + x² + x + 1.
///
/// Trick: x¹²⁸ ≡ x⁷ + x² + x + 1 mod P, so the contribution of `hi`
/// is `hi · (x⁷ + x² + x + 1)`. Because shifting by 1/2/7 wraps into
/// position 128…134 of `lo` plus high bits of the *result*, we do
/// the shift-XOR twice: first for `hi`, then for the resulting
/// position-128…134 carry. Standard Linux/Intel pattern.
#[inline]
#[target_feature(enable = "sse2,ssse3,pclmulqdq")]
unsafe fn reduce_256_to_128(lo: __m128i, hi: __m128i) -> __m128i {
    // First fold: combine `hi` into `lo` via × (x⁷ + x² + x + 1).
    // Use CLMUL with a hard-coded poly for speed (Lemma 2 from the
    // Intel CLMUL whitepaper).
    let poly = _mm_set_epi32(0, 0, 0, 0xC2_00_00_00u32 as i32);
    let t = _mm_clmulepi64_si128(hi, poly, 0x00);
    let lo = _mm_xor_si128(lo, _mm_slli_si128(t, 8));
    let hi = _mm_xor_si128(hi, _mm_srli_si128(t, 8));

    // Second fold.
    let t = _mm_clmulepi64_si128(hi, poly, 0x10);
    let lo = _mm_xor_si128(lo, t);
    _mm_xor_si128(lo, hi)
}

#[inline]
#[target_feature(enable = "sse2,ssse3,pclmulqdq")]
unsafe fn bswap128(x: __m128i) -> __m128i {
    let mask = _mm_loadu_si128(BSWAP_MASK.as_ptr() as *const __m128i);
    _mm_shuffle_epi8(x, mask)
}

impl GHash4 {
    /// Initialise with the GHASH key `H` (= AES_K(0¹²⁸), in
    /// big-endian wire form). Pre-computes H², H³, H⁴ for the 4-way
    /// aggregate path.
    #[target_feature(enable = "sse2,ssse3,pclmulqdq")]
    pub unsafe fn new(h_be: &[u8; 16]) -> Self {
        let h_raw = _mm_loadu_si128(h_be.as_ptr() as *const __m128i);
        let h = bswap128(h_raw);

        let h2 = gf128_mul(h, h);
        let h3 = gf128_mul(h2, h);
        let h4 = gf128_mul(h3, h);

        Self { h, h2, h3, h4, y: _mm_setzero_si128() }
    }

    /// Update with `blocks.len()` × 16 bytes of input. 4-way aggregate
    /// for the bulk; 1-way for trailing 0-3 blocks.
    #[target_feature(enable = "sse2,ssse3,pclmulqdq")]
    pub unsafe fn update(&mut self, blocks: &[[u8; 16]]) {
        let n = blocks.len();
        let mut i = 0;

        // 4-way aggregated.
        while i + 4 <= n {
            let x0 = bswap128(_mm_loadu_si128(blocks[i    ].as_ptr() as *const __m128i));
            let x1 = bswap128(_mm_loadu_si128(blocks[i + 1].as_ptr() as *const __m128i));
            let x2 = bswap128(_mm_loadu_si128(blocks[i + 2].as_ptr() as *const __m128i));
            let x3 = bswap128(_mm_loadu_si128(blocks[i + 3].as_ptr() as *const __m128i));

            // (Y ⊕ X₀) × H⁴ + X₁ × H³ + X₂ × H² + X₃ × H¹
            let yx0 = _mm_xor_si128(self.y, x0);
            self.y = gf128_mul4(yx0, self.h4, x1, self.h3, x2, self.h2, x3, self.h);
            i += 4;
        }

        // Trailing 1-3 blocks: standard 1-way.
        while i < n {
            let x = bswap128(_mm_loadu_si128(blocks[i].as_ptr() as *const __m128i));
            self.y = _mm_xor_si128(self.y, x);
            self.y = gf128_mul(self.y, self.h);
            i += 1;
        }
    }

    /// Finalise — return the GHASH output in big-endian wire form
    /// (matches `ghash::GHash::finalize()`).
    #[target_feature(enable = "sse2,ssse3,pclmulqdq")]
    pub unsafe fn finalize(self) -> [u8; 16] {
        let result = bswap128(self.y);
        let mut out = [0u8; 16];
        _mm_storeu_si128(out.as_mut_ptr() as *mut __m128i, result);
        out
    }
}
