//! I/O ports.

/// One I/O port.
#[derive(Clone, Copy, Debug)]
pub struct Port(u16);

impl Port {
    /// # Safety
    /// The port must belong to a device this code drives; an access to a
    /// foreign port can reset or reprogram hardware.
    pub const unsafe fn new(port: u16) -> Self {
        Port(port)
    }

    pub fn inb(&self) -> u8 {
        // SAFETY: the constructor's contract.
        unsafe { inb(self.0) }
    }
    pub fn inw(&self) -> u16 {
        // SAFETY: the constructor's contract.
        unsafe { inw(self.0) }
    }
    pub fn inl(&self) -> u32 {
        // SAFETY: the constructor's contract.
        unsafe { inl(self.0) }
    }
    pub fn outb(&self, v: u8) {
        // SAFETY: the constructor's contract.
        unsafe { outb(self.0, v) }
    }
    pub fn outw(&self, v: u16) {
        // SAFETY: the constructor's contract.
        unsafe { outw(self.0, v) }
    }
    pub fn outl(&self, v: u32) {
        // SAFETY: the constructor's contract.
        unsafe { outl(self.0, v) }
    }
}

/// A block of consecutive I/O ports, e.g. a legacy BAR.
#[derive(Clone, Copy, Debug)]
pub struct PortRange {
    base: u16,
    len: u16,
}

impl PortRange {
    /// # Safety
    /// As for `Port::new`, for every port in `base .. base + len`.
    pub const unsafe fn new(base: u16, len: u16) -> Self {
        PortRange { base, len }
    }

    pub fn base(&self) -> u16 { self.base }

    /// The port at `off`.
    pub fn port(&self, off: u16) -> Port {
        assert!(off < self.len, "port {:#x} outside a range of {:#x}", off, self.len);
        Port(self.base + off)
    }

    pub fn inb(&self, off: u16) -> u8 { self.port(off).inb() }
    pub fn inw(&self, off: u16) -> u16 { self.port(off).inw() }
    pub fn inl(&self, off: u16) -> u32 { self.port(off).inl() }
    pub fn outb(&self, off: u16, v: u8) { self.port(off).outb(v) }
    pub fn outw(&self, off: u16, v: u16) { self.port(off).outw(v) }
    pub fn outl(&self, off: u16, v: u32) { self.port(off).outl(v) }
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn outb(port: u16, val: u8) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") val); }
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn outw(port: u16, val: u16) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") val); }
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn outl(port: u16, val: u32) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") val); }
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") val); }
    val
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in ax, dx", in("dx") port, out("ax") val); }
    val
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `Port`.
#[inline(always)]
pub(crate) unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in eax, dx", in("dx") port, out("eax") val); }
    val
}
