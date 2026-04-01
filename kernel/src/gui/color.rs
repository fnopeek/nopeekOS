//! nopeekOS dark theme colors.
//!
//! All colors in 0x00RRGGBB format (32bpp BGRA on little-endian x86).

pub struct Theme;

impl Theme {
    // Background
    #[allow(dead_code)]
    pub const BG_DARK: u32       = 0x001A1A2E;  // Deep navy (fallback)
    pub const BG_CARD: u32       = 0x001A1030;  // Card background (dark purple, semi-opaque)
    pub const BG_INPUT: u32      = 0x00120A24;  // Input field background (dark)

    // Foreground / text
    pub const FG_PRIMARY: u32    = 0x00E8E8E8;  // Primary text (near-white)
    pub const FG_SECONDARY: u32  = 0x00888899;  // Subtitle text (muted)
    pub const FG_DOT: u32        = 0x00B080D0;  // Passphrase dots (light purple)

    // Accents
    pub const ACCENT_GREEN: u32  = 0x0000C853;  // Success
    pub const ACCENT_RED: u32    = 0x00FF4444;  // Error

    // Borders (purple-tinted to match aurora)
    pub const BORDER_CARD: u32   = 0x003A2555;  // Card border (muted purple)
    pub const BORDER_INPUT: u32  = 0x004A3570;  // Input field border
    pub const BORDER_FOCUS: u32  = 0x007B50A0;  // Focused input border (bright purple)

    // Cursor
    pub const CURSOR: u32        = 0x00E8E8E8;  // Blinking cursor bar
}
