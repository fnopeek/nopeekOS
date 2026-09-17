//! Gleitkomma-Methoden fuer `no_std`.
//!
//! `f32::sin` und Geschwister sind in `std`, nicht in `core`. Statt fuenfund-
//! dreissig Aufrufstellen auf `libm::sinf(x)` umzuschreiben — und damit den
//! Vendor gegen upstream undiffbar zu machen — steht hier ein Trait mit
//! denselben Namen. Ein `use crate::mathshim::Float;` je Datei genuegt, und
//! der Rest des Codes bleibt Zeile fuer Zeile der von upstream.
//!
//! Mit `std` ist das Trait leer und die inhaerenten Methoden gewinnen, weil
//! Rust sie vor Trait-Methoden waehlt.
#![allow(dead_code)]

#[cfg(not(feature = "std"))]
pub trait Float {
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn sqrt(self) -> Self;
    fn abs(self) -> Self;
    fn powf(self, n: Self) -> Self;
    fn powi(self, n: i32) -> Self;
    fn exp(self) -> Self;
    fn ln(self) -> Self;
    fn floor(self) -> Self;
    fn round(self) -> Self;
}

#[cfg(not(feature = "std"))]
impl Float for f32 {
    fn sin(self) -> f32 { libm::sinf(self) }
    fn cos(self) -> f32 { libm::cosf(self) }
    fn sqrt(self) -> f32 { libm::sqrtf(self) }
    fn abs(self) -> f32 { libm::fabsf(self) }
    fn powf(self, n: f32) -> f32 { libm::powf(self, n) }
    fn powi(self, n: i32) -> f32 { libm::powf(self, n as f32) }
    fn exp(self) -> f32 { libm::expf(self) }
    fn ln(self) -> f32 { libm::logf(self) }
    fn floor(self) -> f32 { libm::floorf(self) }
    fn round(self) -> f32 { libm::roundf(self) }
}

#[cfg(not(feature = "std"))]
impl Float for f64 {
    fn sin(self) -> f64 { libm::sin(self) }
    fn cos(self) -> f64 { libm::cos(self) }
    fn sqrt(self) -> f64 { libm::sqrt(self) }
    fn abs(self) -> f64 { libm::fabs(self) }
    fn powf(self, n: f64) -> f64 { libm::pow(self, n) }
    fn powi(self, n: i32) -> f64 { libm::pow(self, n as f64) }
    fn exp(self) -> f64 { libm::exp(self) }
    fn ln(self) -> f64 { libm::log(self) }
    fn floor(self) -> f64 { libm::floor(self) }
    fn round(self) -> f64 { libm::round(self) }
}

// Mit `std` gibt es nichts zu tun: die inhaerenten Methoden sind da.
#[cfg(feature = "std")]
pub trait Float {}
#[cfg(feature = "std")]
impl Float for f32 {}
#[cfg(feature = "std")]
impl Float for f64 {}

/// `OnceLock` fuer `no_std`.
///
/// `spin::Once` kann dasselbe, heisst seine Methode aber `call_once` statt
/// `get_or_init`. Dieser Wrapper traegt die Namen von `std`, damit die
/// Aufrufstellen in `dsp.rs` Zeile fuer Zeile die von upstream bleiben.
#[cfg(not(feature = "std"))]
pub struct OnceLock<T>(spin::Once<T>);

#[cfg(not(feature = "std"))]
impl<T> OnceLock<T> {
    pub const fn new() -> Self { OnceLock(spin::Once::new()) }
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T { self.0.call_once(f) }
}
