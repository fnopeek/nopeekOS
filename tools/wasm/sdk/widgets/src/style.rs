//! Design tokens — size scales apps reference by name instead of
//! hardcoded pixels. A future theme swap retunes the whole UI here.

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Radius {
    None = 0,
    Sm   = 4,
    Md   = 8,
    Lg   = 12,
    Xl   = 16,
    Pill = 255,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Spacing {
    None = 0,
    Xxs  = 2,
    Xs   = 4,
    Sm   = 8,
    Md   = 12,
    Lg   = 16,
    Xl   = 24,
    Xxl  = 32,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Padding {
    None = 0,
    Xxs  = 2,
    Xs   = 4,
    Sm   = 8,
    Md   = 12,
    Lg   = 16,
    Xl   = 24,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Elevation {
    Flat     = 0,
    Subtle   = 1,
    Raised   = 2,
    Floating = 3,
}

impl Radius {
    pub const fn as_u8(self) -> u8 { self as u8 }
}
impl Spacing {
    pub const fn as_u16(self) -> u16 { self as u16 }
}
impl Padding {
    pub const fn as_u16(self) -> u16 { self as u16 }
}

/// Motion duration token — semantic timing for `Transition::Linear`.
/// SDK-only convenience: mapped to milliseconds at modifier-construction
/// time, so the wire form stays the existing `Transition::Linear { ms }`
/// (no new variant, no wire-version bump).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Motion {
    Instant = 0,
    Quick   = 120,
    Normal  = 200,
    Slow    = 400,
}

impl Motion {
    pub const fn as_ms(self) -> u16 { self as u16 }

    /// Lower this token to a wire-format `Transition`. Use as
    /// `Modifier::Transition(Motion::Quick.as_transition())`.
    pub const fn as_transition(self) -> crate::abi::Transition {
        crate::abi::Transition::Linear { ms: self as u16 }
    }
}
