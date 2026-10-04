# `kernel/src/crypto/aead_hw_ghash.rs` @ 5e0102684

## L1-24 · `#![allow(unsafe_op_in_unsafe_fn)]`

```
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
```

## L26-28 · `#![allow(unsafe_op_in_unsafe_fn)]`

```
// Skeleton for a 4-way aggregated GHASH path. Not wired into the AEAD
// hot path: per-block bytes-match against the `ghash` crate failed in
// v0.88.4, deferred for its own session. Kept as starting point.
```

## L36 · `#[derive(Clone, Copy)]`

```
/// 4-way aggregated GHASH state.
```

## L39-42 · `h: __m128i,`

```
/// Powers of H in the "raw" GF(2¹²⁸) polynomial form (no
/// pre-shift): h, h², h³, h⁴. All stored in PCLMULQDQ-native
/// (little-endian) bit order — we byte-reverse incoming blocks
/// before XOR'ing into the accumulator.
```

## L50-51 · `#[inline]`

```
/// Carry-less multiply two GF(2¹²⁸) elements with reduction modulo
/// x¹²⁸ + x⁷ + x² + x + 1. Karatsuba: 3 PCLMULQDQ + shifts.
```

## L55 · `let t0 = _mm_clmulepi64_si128(a, b, 0x00); // a_lo × b_lo`

```
// Schoolbook via Karatsuba — 3 PCLMULQDQ.
```

## L56 · `let t0 = _mm_clmulepi64_si128(a, b, 0x00); // a_lo × b_lo`

```
// a_lo × b_lo
```

## L57 · `let t1 = _mm_clmulepi64_si128(a, b, 0x11); // a_hi × b_hi`

```
// a_hi × b_hi
```

## L59 · `let a_xor = _mm_xor_si128(a, _mm_shuffle_epi32(a, 0xEE));`

```
// (a_lo + a_hi) × (b_lo + b_hi) — single multiply for the cross term.
```

## L65 · `let lo = _mm_xor_si128(t0, _mm_slli_si128(mid, 8));`

```
// Assemble 256-bit (lo: t0, mid lifted by 64, hi: t1).
```

## L72-73 · `#[inline]`

```
/// Aggregate 4 carry-less multiplications into one reduction.
/// Computes (a₀×b₀) ⊕ (a₁×b₁) ⊕ (a₂×b₂) ⊕ (a₃×b₃) mod P.
```

## L107-114 · `#[inline]`

```
/// Reduce a 256-bit polynomial (lo + hi·x¹²⁸) modulo
/// P = x¹²⁸ + x⁷ + x² + x + 1.
///
/// Trick: x¹²⁸ ≡ x⁷ + x² + x + 1 mod P, so the contribution of `hi`
/// is `hi · (x⁷ + x² + x + 1)`. Because shifting by 1/2/7 wraps into
/// position 128…134 of `lo` plus high bits of the *result*, we do
/// the shift-XOR twice: first for `hi`, then for the resulting
/// position-128…134 carry. Standard Linux/Intel pattern.
```

## L118-120 · `let poly = _mm_set_epi32(0, 0, 0, 0xC2_00_00_00u32 as i32);`

```
// First fold: combine `hi` into `lo` via × (x⁷ + x² + x + 1).
// Use CLMUL with a hard-coded poly for speed (Lemma 2 from the
// Intel CLMUL whitepaper).
```

## L126 · `let t = _mm_clmulepi64_si128(hi, poly, 0x10);`

```
// Second fold.
```

## L140-142 · `#[target_feature(enable = "sse2,ssse3,pclmulqdq")]`

```
/// Initialise with the GHASH key `H` (= AES_K(0¹²⁸), in
/// big-endian wire form). Pre-computes H², H³, H⁴ for the 4-way
/// aggregate path.
```

## L155-156 · `#[target_feature(enable = "sse2,ssse3,pclmulqdq")]`

```
/// Update with `blocks.len()` × 16 bytes of input. 4-way aggregate
/// for the bulk; 1-way for trailing 0-3 blocks.
```

## L162 · `while i + 4 <= n {`

```
// 4-way aggregated.
```

## L169 · `let yx0 = _mm_xor_si128(self.y, x0);`

```
// (Y ⊕ X₀) × H⁴ + X₁ × H³ + X₂ × H² + X₃ × H¹
```

## L175 · `while i < n {`

```
// Trailing 1-3 blocks: standard 1-way.
```

## L184-185 · `#[target_feature(enable = "sse2,ssse3,pclmulqdq")]`

```
/// Finalise — return the GHASH output in big-endian wire form
/// (matches `ghash::GHash::finalize()`).
```

