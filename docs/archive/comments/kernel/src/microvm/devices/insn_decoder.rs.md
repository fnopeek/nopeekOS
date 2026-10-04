# `kernel/src/microvm/devices/insn_decoder.rs` @ 5e0102684

## L1-11 · `#![allow(dead_code)]`

```
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
```

## L15-16 · `pub struct DecodedMov {`

```
/// Decoded MMIO MOV. The decoder does not compute the memory address —
/// the hypervisor provides the GPA via its exit-info already.
```

## L18 · `pub width: u8,`

```
/// Operand width in bytes: 1, 2, 4, or 8.
```

## L20 · `pub is_write: bool,`

```
/// True = MOV stores to memory (mem is destination).
```

## L22-25 · `pub reg: u8,`

```
/// Register index 0..15. The "reg" field of ModR/M, extended by REX.R.
/// Mapping: 0=RAX, 1=RCX, 2=RDX, 3=RBX, 4=RSP, 5=RBP, 6=RSI, 7=RDI,
/// 8=R8, …, 15=R15. Always the GPR side of the access (mem is the
/// other operand).
```

## L27-30 · `pub length: u8,`

```
/// Total instruction length in bytes (prefixes + REX + opcode +
/// ModR/M + SIB + displacement). The SVM MMIO handler adds this to
/// the faulting RIP because AMD's NRIP_SAVE is undefined for #NPF
/// (APM §15.7.1) — KVM nested SVM zeroes it.
```

## L34-36 · `pub fn decode_mov(bytes: &[u8]) -> Option<DecodedMov> {`

```
/// Decode a MOV reg<->mem instruction. `bytes` should be 1..=15 bytes
/// from the SVM decode-assists window. Returns `None` for opcodes
/// outside the supported subset or malformed sequences.
```

## L39 · `let mut op16 = false; // 0x66 prefix → 16-bit operand`

```
// 0x66 prefix → 16-bit operand
```

## L40 · `let mut addr32 = false; // 0x67 prefix → 32-bit addressing`

```
// 0x67 prefix → 32-bit addressing
```

## L44-48 · `while i < bytes.len() {`

```
// Eat legacy prefixes we care about. Segment overrides are
// accepted-and-ignored; 0x67 flips disp/SIB widths in 16-bit mode
// but in long mode it only switches between 64-bit and 32-bit
// addressing — the displacement encoding (disp8/disp32) is the
// same in both, so we just track it for completeness.
```

## L69 · `0x88 => (true, true),   // MOV m8,  r8`

```
// MOV m8,  r8
```

## L70 · `0x89 => (true, false),  // MOV m{16,32,64}, r{16,32,64}`

```
// MOV m{16,32,64}, r{16,32,64}
```

## L71 · `0x8A => (false, true),  // MOV r8, m8`

```
// MOV r8, m8
```

## L72 · `0x8B => (false, false), // MOV r{16,32,64}, m{16,32,64}`

```
// MOV r{16,32,64}, m{16,32,64}
```

## L82 · `if mod_field == 0b11 { return None; }`

```
// mod=11 means register-direct addressing — not an MMIO access.
```

## L86 · `let mut has_sib_base_disp32 = false;`

```
// SIB byte present when r/m == 100 (and mod != 11, already checked).
```

## L92-94 · `if mod_field == 0b00 && (sib & 0x07) == 0b101 {`

```
// SIB.base == 101 with mod == 00 means [disp32 + index*scale],
// i.e. there's an extra disp32 that wouldn't otherwise be
// present for mod=00. APM Vol 3 §1.7.4.
```

## L100 · `let disp_bytes: usize = match mod_field {`

```
// Displacement.
```

## L103-104 · `if rm_field == 0b101 || has_sib_base_disp32 {`

```
// mod=00, r/m=101 → [RIP+disp32] in 64-bit mode.
// Plus the SIB-induced disp32 above.
```

## L113 · `_ => 0, // unreachable, mod==11 already rejected`

```
// unreachable, mod==11 already rejected
```

## L133 · `pub const fn width_mask(width: u8) -> u64 {`

```
/// Mask the upper bits off a value to honour the operand width.
```

## L143-145 · `pub fn merge_reg(old: u64, value: u64, width: u8) -> u64 {`

```
/// Merge a value into an existing register according to x86 GPR rules.
/// 32-bit writes zero the upper 32 bits; 8/16-bit writes preserve the
/// rest. Used by the MMIO emulator when writing the MOV destination.
```

