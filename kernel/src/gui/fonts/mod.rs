//! Spleen bitmap fonts (BSD 2-Clause, github.com/fcambus/spleen)
//!
//! Three sizes for different display resolutions and UI elements:
//!   8x16  — Console text at 1080p (4 KB)
//!   16x32 — Console text at 4K, UI elements (16 KB)
//!   32x64 — Large clock display on login screen (64 KB)

mod spleen_8x16;
mod spleen_16x32;
mod spleen_32x64;

pub use spleen_8x16::FONT_8X16;
pub use spleen_16x32::FONT_16X32;
pub use spleen_32x64::FONT_32X64;
