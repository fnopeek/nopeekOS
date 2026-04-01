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

    // Accents (soft, matches aurora aesthetic)
    pub const ACCENT_GREEN: u32  = 0x0090C8A0;  // Success (muted sage)
    pub const ACCENT_RED: u32    = 0x00C87070;  // Error (soft rose)

    // Borders (purple-tinted to match aurora)
    pub const BORDER_CARD: u32   = 0x003A2555;  // Card border (muted purple)
    pub const BORDER_INPUT: u32  = 0x004A3570;  // Input field border
    pub const BORDER_FOCUS: u32  = 0x007B50A0;  // Focused input border (bright purple)

    // Hyprlock-style input field
    pub const INPUT_OUTER: u32   = 0x00151515;  // Input outline (dark gray)
    pub const INPUT_INNER: u32   = 0x00C8C8C8;  // Input fill (light gray)
    pub const INPUT_DOT: u32     = 0x000A0A0A;  // Dot color (near-black)
    pub const CHECK_COLOR: u32   = 0x00B0A080;  // Verifying color (warm muted)
    pub const FAIL_COLOR: u32    = 0x00A05050;  // Fail color (muted rose, subtle)

    // Cursor
    pub const CURSOR: u32        = 0x00E8E8E8;  // Blinking cursor bar
}
