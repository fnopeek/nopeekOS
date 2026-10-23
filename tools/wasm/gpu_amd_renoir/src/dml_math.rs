//! The pieces of the Display Mode Library dmlgen.py does not translate:
//! two functions that read a float's bits through a pointer
//! (dml_inline_defs.h `dml_log2`, dcn_calc_math.c `dcn_bw_log`), the
//! conversions C performs with an x86 instruction, the byte comparison
//! behind `memcmp`, and the `int *` / `unsigned int *` casts DML passes.
#![allow(dead_code, non_snake_case)]

/// `dml_log2` (dml_inline_defs.h): the unbiased exponent of `x`.
pub fn dml_log2(x: f64) -> i32 {
    let ix = x.to_bits();
    ((ix >> 52) & 0x7ff) as i32 - 1023
}

/// `dcn_bw_log` (dcn_calc_math.c): log of `a` to base `b`, from the
/// exponent and a quadratic fit of the mantissa.
pub fn dcn_bw_log(a: f32, b: f32) -> f32 {
    let mut x = a.to_bits() as i32;
    let log_2 = ((x >> 23) & 255) - 128;
    x &= !(255 << 23);
    x += 127 << 23;
    let mut a = f32::from_bits(x as u32);
    a = ((-1.0f32 / 3.0) * a + 2.0) * a - 2.0f32 / 3.0;
    if (b as f64) > 2.00001 || (b as f64) < 1.99999 {
        (a + log_2 as f32) / dcn_bw_log(b, 2.0)
    } else {
        a + log_2 as f32
    }
}

/// `(int)x`: cvttsd2si, which gives INT_MIN for NaN and out-of-range values.
pub fn f2i32(x: f64) -> i32 {
    if x.is_nan() || x >= 2147483648.0 || x <= -2147483649.0 { i32::MIN } else { x as i32 }
}

/// `(long long)x`: the 64-bit cvttsd2si.
pub fn f2i64(x: f64) -> i64 {
    if x.is_nan() || x >= 9223372036854775808.0 || x < -9223372036854775808.0 { i64::MIN } else { x as i64 }
}

/// `(unsigned int)x`: gcc converts through the 64-bit cvttsd2si and keeps
/// the low half.
pub fn f2u32(x: f64) -> u32 {
    f2i64(x) as u32
}

/// Bitwise equality, as `memcmp` sees a struct (floats by their bits).
pub trait BitsEq {
    fn bits_eq(&self, o: &Self) -> bool;
}

macro_rules! bits_eq_int {
    ($($t:ty),*) => { $(impl BitsEq for $t { fn bits_eq(&self, o: &Self) -> bool { self == o } })* };
}
bits_eq_int!(bool, u8, i8, u16, i16, u32, i32, u64, i64, ());

impl BitsEq for f64 {
    fn bits_eq(&self, o: &Self) -> bool {
        self.to_bits() == o.to_bits()
    }
}

impl BitsEq for f32 {
    fn bits_eq(&self, o: &Self) -> bool {
        self.to_bits() == o.to_bits()
    }
}

impl<T: BitsEq, const N: usize> BitsEq for [T; N] {
    fn bits_eq(&self, o: &Self) -> bool {
        bits_eq_slice(self, o)
    }
}

impl<T> BitsEq for Option<T> {
    fn bits_eq(&self, o: &Self) -> bool {
        self.is_none() && o.is_none()
    }
}

/// `memcmp(a, b, sizeof(*a) * n) == 0` over `n` elements.
pub fn bits_eq_slice<T: BitsEq>(a: &[T], b: &[T]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.bits_eq(y))
}

/// The two 32-bit integer types an `int *` / `unsigned int *` cast moves between.
pub trait Int32: Copy + private::Sealed {}
impl Int32 for i32 {}
impl Int32 for u32 {}
mod private {
    pub trait Sealed {}
    impl Sealed for i32 {}
    impl Sealed for u32 {}
}

/// `(U *)p` for a 32-bit integer.
pub fn reint_mut<T: Int32, U: Int32>(p: &mut T) -> &mut U {
    // SAFETY: T and U are i32 or u32 (sealed): same size and alignment, and
    // every bit pattern is valid for both; the borrow is carried over.
    unsafe { &mut *(p as *mut T as *mut U) }
}

/// `(const U *)p` for a 32-bit integer.
pub fn reint<T: Int32, U: Int32>(p: &T) -> &U {
    // SAFETY: as in reint_mut.
    unsafe { &*(p as *const T as *const U) }
}

/// `(U *)p` for a run of 32-bit integers.
pub fn reint_slice_mut<T: Int32, U: Int32>(p: &mut [T]) -> &mut [U] {
    // SAFETY: as in reint_mut; the length in elements is the same.
    unsafe { core::slice::from_raw_parts_mut(p.as_mut_ptr() as *mut U, p.len()) }
}

/// `(const U *)p` for a run of 32-bit integers.
pub fn reint_slice<T: Int32, U: Int32>(p: &[T]) -> &[U] {
    // SAFETY: as in reint_mut; the length in elements is the same.
    unsafe { core::slice::from_raw_parts(p.as_ptr() as *const U, p.len()) }
}
