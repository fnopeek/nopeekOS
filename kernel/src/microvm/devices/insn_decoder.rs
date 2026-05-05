//! Minimal x86-64 instruction decoder for MMIO emulation.
//!
//! Scope: just the MOV variants Linux's MMIO accessors lower to.
//! Linux `ioread{8,16,32,64}` / `iowrite{8,16,32,64}` compile to plain
//! MOV with simple memory operands (typical: `[reg+disp]`). That covers
//! every PCI-config and virtio-modern register access in Phase 12.2.
//!
//! Anything more exotic (REP MOVS, atomic ops, vector ops on MMIO)
//! returns `None`; the caller logs and bails. We can extend on demand.
//!
//! Reference: Intel SDM Vol 2 / AMD APM Vol 3.

#![allow(dead_code)]

/// Decoded MMIO MOV. The decoder does not compute the memory address —
/// the hypervisor provides the GPA via its exit-info already.
pub struct DecodedMov {
    /// Operand width in bytes: 1, 2, 4, or 8.
    pub width: u8,
    /// True = MOV stores to memory (mem is destination).
    pub is_write: bool,
    /// Register index 0..15. The "reg" field of ModR/M, extended by REX.R.
    /// Mapping: 0=RAX, 1=RCX, 2=RDX, 3=RBX, 4=RSP, 5=RBP, 6=RSI, 7=RDI,
    /// 8=R8, …, 15=R15. Always the GPR side of the access (mem is the
    /// other operand).
    pub reg: u8,
}

/// Decode a MOV reg<->mem instruction. `bytes` should be 1..=15 bytes
/// from the SVM decode-assists window. Returns `None` for opcodes
/// outside the supported subset or malformed sequences.
pub fn decode_mov(bytes: &[u8]) -> Option<DecodedMov> {
    let mut i = 0usize;
    let mut op16 = false; // 0x66 prefix → 16-bit operand
    let mut rex_w = false;
    let mut rex_r = false;

    // Eat legacy prefixes we care about. Segment overrides + 0x67
    // address-size override are accepted-and-ignored.
    while i < bytes.len() {
        match bytes[i] {
            0x66 => { op16 = true; i += 1; }
            0x67 => { i += 1; }
            0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 => { i += 1; }
            _ => break,
        }
    }
    if i >= bytes.len() { return None; }

    if (bytes[i] & 0xF0) == 0x40 {
        rex_w = bytes[i] & 0x08 != 0;
        rex_r = bytes[i] & 0x04 != 0;
        i += 1;
        if i >= bytes.len() { return None; }
    }

    let op = bytes[i];
    let (is_write, byte_op) = match op {
        0x88 => (true, true),   // MOV m8,  r8
        0x89 => (true, false),  // MOV m{16,32,64}, r{16,32,64}
        0x8A => (false, true),  // MOV r8, m8
        0x8B => (false, false), // MOV r{16,32,64}, m{16,32,64}
        _ => return None,
    };
    i += 1;
    if i >= bytes.len() { return None; }

    let modrm = bytes[i];
    let reg_field = (modrm >> 3) & 0x07;
    let mod_field = modrm >> 6;
    // mod=11 means register-direct addressing — not an MMIO access.
    if mod_field == 0b11 { return None; }

    let reg = if rex_r { reg_field + 8 } else { reg_field };

    let width = if byte_op {
        1
    } else if rex_w {
        8
    } else if op16 {
        2
    } else {
        4
    };

    Some(DecodedMov { width, is_write, reg })
}

/// Mask the upper bits off a value to honour the operand width.
pub const fn width_mask(width: u8) -> u64 {
    match width {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => 0xFFFF_FFFF,
        _ => 0xFFFF_FFFF_FFFF_FFFF,
    }
}

/// Merge a value into an existing register according to x86 GPR rules.
/// 32-bit writes zero the upper 32 bits; 8/16-bit writes preserve the
/// rest. Used by the MMIO emulator when writing the MOV destination.
pub fn merge_reg(old: u64, value: u64, width: u8) -> u64 {
    match width {
        1 => (old & !0xFF) | (value & 0xFF),
        2 => (old & !0xFFFF) | (value & 0xFFFF),
        4 => value & 0xFFFF_FFFF,
        8 => value,
        _ => value,
    }
}
