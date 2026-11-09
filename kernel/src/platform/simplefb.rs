//! The firmware's framebuffer as a device tree `simple-framebuffer` node
//! (Linux `drivers/video/fbdev/simplefb.c`, `simplefb_parse_dt`;
//! `of_platform_default_populate_init` looks for it under /chosen).
//!
//! The Raspberry Pi firmware reads the display's EDID, sets the mode and
//! adds this node before it starts the kernel; nothing has to be driven to
//! get a picture.

use super::dt::{Fdt, Node};

/// The pixel layouts the console draws (`simplefb_formats`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// 32 bpp, `a8r8g8b8` or `x8r8g8b8`.
    Xrgb8888,
    /// 16 bpp, `r5g6b5`.
    Rgb565,
}

impl Format {
    pub fn bpp(self) -> u8 {
        match self {
            Format::Xrgb8888 => 32,
            Format::Rgb565 => 16,
        }
    }
}

/// A framebuffer as the node describes it.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub addr: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: Format,
}

/// Why a node was found but cannot be used.
#[derive(Clone, Copy, Debug)]
pub enum Error {
    /// `width`, `height`, `stride` or `format` missing.
    Incomplete,
    /// A format the console does not draw; holds its first bytes.
    Format([u8; 12]),
    /// Neither `memory-region` nor `reg` gives an address.
    NoMemory,
    /// The buffer is smaller than `stride * height`.
    TooSmall,
}

fn node(fdt: &Fdt) -> Option<Node> {
    let mut under_chosen = None;
    let mut anywhere = None;
    let chosen = fdt.find_path(b"/chosen").map(|c| c.offset);
    fdt.walk(|n| {
        if fdt.prop_has_str(n, b"compatible", b"simple-framebuffer") && fdt.is_enabled(n) {
            if chosen.is_some() && n.ancestors().last().copied() == chosen {
                under_chosen = Some(*n);
                return false;
            }
            anywhere.get_or_insert(*n);
        }
        true
    });
    under_chosen.or(anywhere)
}

/// The node whose `phandle` is `ph`.
fn by_phandle(fdt: &Fdt, ph: u32) -> Option<Node> {
    let mut found = None;
    fdt.walk(|n| {
        if fdt.prop_u32(n, b"phandle") == Some(ph) {
            found = Some(*n);
            return false;
        }
        true
    });
    found
}

/// The firmware framebuffer, if the device tree has one.
pub fn find(fdt: &Fdt) -> Option<Result<Mode, Error>> {
    let n = node(fdt)?;
    Some(parse(fdt, &n))
}

fn parse(fdt: &Fdt, n: &Node) -> Result<Mode, Error> {
    let width = fdt.prop_u32(n, b"width").ok_or(Error::Incomplete)?;
    let height = fdt.prop_u32(n, b"height").ok_or(Error::Incomplete)?;
    let stride = fdt.prop_u32(n, b"stride").ok_or(Error::Incomplete)?;
    let mut fmt = [0u8; 16];
    let len = fdt.prop_str(n, b"format", &mut fmt).ok_or(Error::Incomplete)?;
    let format = match &fmt[..len] {
        b"a8r8g8b8" | b"x8r8g8b8" => Format::Xrgb8888,
        b"r5g6b5" => Format::Rgb565,
        other => {
            let mut name = [0u8; 12];
            let k = other.len().min(12);
            name[..k].copy_from_slice(&other[..k]);
            return Err(Error::Format(name));
        }
    };
    // `memory-region` wins over `reg`, as in simplefb_parse_dt.
    let region = fdt.prop_u32(n, b"memory-region").and_then(|ph| by_phandle(fdt, ph)).and_then(|r| fdt.reg0(&r));
    let (addr, size) = region.or_else(|| fdt.reg0(n)).ok_or(Error::NoMemory)?;
    if size < stride as u64 * height as u64 {
        return Err(Error::TooSmall);
    }
    Ok(Mode { addr, size, width, height, stride, format })
}
