//! x86-64 encoder. Just the instructions the generator emits — not an
//! assembler, and deliberately not a general one.
//!
//! Memory operands are always encoded with a 32-bit displacement (mod=10).
//! That costs three bytes on small offsets and buys the disappearance of two
//! special cases: `rbp`/`r13` as a base cannot use mod=00, and every frame
//! slot would otherwise need a size decision. A single-pass generator has no
//! second chance to shrink an instruction, so the uniform form is the honest
//! one.

use alloc::vec::Vec;

/// Register numbers are the hardware's, so `reg & 7` is the ModRM field and
/// `reg >> 3` is the REX bit. Never reorder.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Reg {
    #[inline]
    fn low(self) -> u8 {
        self as u8 & 7
    }
    #[inline]
    fn ext(self) -> u8 {
        (self as u8 >> 3) & 1
    }
}

/// Condition codes, in the encoding the `Jcc`/`SETcc` opcode byte adds.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Cond {
    E = 0x4,
    Ne = 0x5,
    B = 0x2,
    Ae = 0x3,
    Be = 0x6,
    A = 0x7,
    L = 0xC,
    Ge = 0xD,
    Le = 0xE,
    G = 0xF,
    S = 0x8,
    /// Parity: set when a float comparison was unordered, i.e. saw a NaN.
    P = 0xA,
    Np = 0xB,
}

/// A forward branch whose target is not known yet. Every one handed out must
/// be bound before the buffer is used — `Asm::finish` refuses otherwise, so a
/// forgotten label is a compile-time failure and never a wild jump.
#[must_use = "an unbound label leaves a hole in the code"]
pub struct Patch {
    /// Offset of the rel32 field.
    at: usize,
}

#[derive(Default)]
pub struct Asm {
    pub code: Vec<u8>,
    open: usize,
}

impl Asm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start with room for `n` bytes. A single-pass generator knows roughly
    /// how much it will emit, and growing a buffer is pure copying.
    pub fn with_capacity(n: usize) -> Self {
        Asm {
            code: Vec::with_capacity(n),
            open: 0,
        }
    }

    pub fn pos(&self) -> usize {
        self.code.len()
    }

    /// The finished bytes, or `None` if a branch was left dangling.
    pub fn finish(self) -> Option<Vec<u8>> {
        if self.open == 0 { Some(self.code) } else { None }
    }

    // --- raw ---

    fn b(&mut self, v: u8) {
        self.code.push(v);
    }
    fn d32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }
    fn d64(&mut self, v: i64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// REX prefix. Emitted whenever any bit is set; also forced for 8-bit
    /// access to sil/dil/bpl/spl, which is the caller's job to request.
    fn rex(&mut self, w: bool, r: u8, x: u8, b: u8) {
        let v = 0x40 | ((w as u8) << 3) | (r << 2) | (x << 1) | b;
        if v != 0x40 {
            self.b(v);
        }
    }
    fn rex_always(&mut self, w: bool, r: u8, x: u8, b: u8) {
        self.b(0x40 | ((w as u8) << 3) | (r << 2) | (x << 1) | b);
    }

    fn modrm_rr(&mut self, reg: Reg, rm: Reg) {
        self.b(0xC0 | (reg.low() << 3) | rm.low());
    }

    /// `[base + disp32]`, always mod=10. `rsp`/`r12` need a SIB byte because
    /// rm=100 means "SIB follows" rather than "register 4".
    fn modrm_mem(&mut self, reg: Reg, base: Reg, disp: i32) {
        self.b(0x80 | (reg.low() << 3) | base.low());
        if base.low() == 4 {
            self.b(0x24); // scale=0, index=none(100), base=100
        }
        self.d32(disp);
    }

    /// `[base + index*2^scale + disp32]`. Linear memory uses scale 0; the
    /// function table uses 3 for its eight-byte entries and 2 for the
    /// four-byte signature ids beside them.
    fn modrm_mem_sib(&mut self, reg: Reg, base: Reg, index: Reg, scale: u8, disp: i32) {
        self.b(0x80 | (reg.low() << 3) | 4); // rm=100 -> SIB follows
        self.b((scale << 6) | (index.low() << 3) | base.low());
        self.d32(disp);
    }

    fn modrm_mem_idx(&mut self, reg: Reg, base: Reg, index: Reg, disp: i32) {
        self.modrm_mem_sib(reg, base, index, 0, disp);
    }

    // --- moves ---

    /// `mov dst, src` (64-bit)
    pub fn mov_rr64(&mut self, dst: Reg, src: Reg) {
        self.rex(true, src.ext(), 0, dst.ext());
        self.b(0x89);
        self.modrm_rr(src, dst);
    }

    /// `mov dst, src` (32-bit; zero-extends into the upper half, which is
    /// exactly wasm's i32 semantics)
    pub fn mov_rr32(&mut self, dst: Reg, src: Reg) {
        self.rex(false, src.ext(), 0, dst.ext());
        self.b(0x89);
        self.modrm_rr(src, dst);
    }

    pub fn mov_r32_imm32(&mut self, dst: Reg, v: i32) {
        self.rex(false, 0, 0, dst.ext());
        self.b(0xB8 + dst.low());
        self.d32(v);
    }

    pub fn mov_r64_imm64(&mut self, dst: Reg, v: i64) {
        self.rex(true, 0, 0, dst.ext());
        self.b(0xB8 + dst.low());
        self.d64(v);
    }

    /// `mov dst, [base + disp]` (32-bit load, zero-extending)
    pub fn load32(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.rex(false, dst.ext(), 0, base.ext());
        self.b(0x8B);
        self.modrm_mem(dst, base, disp);
    }

    /// `mov dst, [base + disp]` (64-bit load)
    pub fn load64(&mut self, dst: Reg, base: Reg, disp: i32) {
        self.rex(true, dst.ext(), 0, base.ext());
        self.b(0x8B);
        self.modrm_mem(dst, base, disp);
    }

    /// `mov [base + disp], src` (32-bit store)
    pub fn store32(&mut self, base: Reg, disp: i32, src: Reg) {
        self.rex(false, src.ext(), 0, base.ext());
        self.b(0x89);
        self.modrm_mem(src, base, disp);
    }

    /// `mov [base + disp], src` (64-bit store)
    pub fn store64(&mut self, base: Reg, disp: i32, src: Reg) {
        self.rex(true, src.ext(), 0, base.ext());
        self.b(0x89);
        self.modrm_mem(src, base, disp);
    }

    /// `mov dst, [base + index + disp]` — linear memory.
    pub fn load_idx(&mut self, w: bool, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.rex(w, dst.ext(), index.ext(), base.ext());
        self.b(0x8B);
        self.modrm_mem_idx(dst, base, index, disp);
    }
    pub fn load32_idx(&mut self, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.load_idx(false, dst, base, index, disp);
    }
    pub fn load64_idx(&mut self, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.load_idx(true, dst, base, index, disp);
    }

    /// `mov [base + index + disp], src` — linear memory.
    pub fn store_idx(&mut self, w: bool, base: Reg, index: Reg, disp: i32, src: Reg) {
        self.rex(w, src.ext(), index.ext(), base.ext());
        self.b(0x89);
        self.modrm_mem_idx(src, base, index, disp);
    }
    pub fn store32_idx(&mut self, base: Reg, index: Reg, disp: i32, src: Reg) {
        self.store_idx(false, base, index, disp, src);
    }
    pub fn store64_idx(&mut self, base: Reg, index: Reg, disp: i32, src: Reg) {
        self.store_idx(true, base, index, disp, src);
    }

    /// `movsxd dst, dword [base + index + disp]` — the only widening load
    /// that is not a `movzx`/`movsx` pair: i64.load32_s.
    pub fn load32s_idx(&mut self, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.rex(true, dst.ext(), index.ext(), base.ext());
        self.b(0x63);
        self.modrm_mem_idx(dst, base, index, disp);
    }

    /// The narrow loads, all widening into a full 32-bit register the way
    /// wasm wants: `movzx`/`movsx` from byte or word.
    fn ext_load_idx(&mut self, w: bool, op2: u8, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.rex(w, dst.ext(), index.ext(), base.ext());
        self.b(0x0F);
        self.b(op2);
        self.modrm_mem_idx(dst, base, index, disp);
    }

    /// Zero-extending narrow loads. The 32-bit form already clears the upper
    /// half of the register, so it serves the i64 variants unchanged.
    pub fn load8u_idx(&mut self, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.ext_load_idx(false, 0xB6, dst, base, index, disp);
    }
    pub fn load16u_idx(&mut self, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.ext_load_idx(false, 0xB7, dst, base, index, disp);
    }
    /// Sign-extending narrow loads. Here the width DOES matter: filling 64
    /// bits with the sign is a different instruction from filling 32.
    pub fn load8s_idx(&mut self, w: bool, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.ext_load_idx(w, 0xBE, dst, base, index, disp);
    }
    pub fn load16s_idx(&mut self, w: bool, dst: Reg, base: Reg, index: Reg, disp: i32) {
        self.ext_load_idx(w, 0xBF, dst, base, index, disp);
    }

    /// `mov [base + index + disp], src8`. The REX prefix is forced: without
    /// one, encoding 4..7 means `ah/ch/dh/bh` rather than `spl/bpl/sil/dil`,
    /// and the store would hit the wrong half of the wrong register.
    pub fn store8_idx(&mut self, base: Reg, index: Reg, disp: i32, src: Reg) {
        self.rex_always(false, src.ext(), index.ext(), base.ext());
        self.b(0x88);
        self.modrm_mem_idx(src, base, index, disp);
    }

    /// `mov [base + index + disp], src16` — the 0x66 prefix picks operand
    /// size 16 and must come before REX.
    pub fn store16_idx(&mut self, base: Reg, index: Reg, disp: i32, src: Reg) {
        self.b(0x66);
        self.rex(false, src.ext(), index.ext(), base.ext());
        self.b(0x89);
        self.modrm_mem_idx(src, base, index, disp);
    }

    /// Shift or rotate `dst` (32-bit) by `cl`. wasm masks the count to five
    /// bits and so does x86 for 32-bit operands, so no masking is needed.
    /// `ext` picks the operation: 4 shl, 5 shr, 7 sar, 0 rol, 1 ror.
    fn shift_cl(&mut self, w: bool, ext: u8, dst: Reg) {
        self.rex(w, 0, 0, dst.ext());
        self.b(0xD3);
        self.b(0xC0 | (ext << 3) | dst.low());
    }

    /// Shift by a constant, for the places where routing the count through
    /// `cl` would cost more than it saves.
    pub fn shr_imm(&mut self, w: bool, dst: Reg, imm: u8) {
        self.rex(w, 0, 0, dst.ext());
        self.b(0xC1);
        self.b(0xE8 | dst.low());
        self.b(imm);
    }

    /// `and reg, imm32` (sign-extended in the 64-bit form).
    pub fn and_r_imm32(&mut self, w: bool, r: Reg, v: i32) {
        self.rex(w, 0, 0, r.ext());
        self.b(0x81);
        self.b(0xE0 | r.low());
        self.d32(v);
    }

    pub fn shl(&mut self, w: bool, dst: Reg) {
        self.shift_cl(w, 4, dst);
    }
    pub fn shr(&mut self, w: bool, dst: Reg) {
        self.shift_cl(w, 5, dst);
    }
    pub fn sar(&mut self, w: bool, dst: Reg) {
        self.shift_cl(w, 7, dst);
    }
    pub fn rol(&mut self, w: bool, dst: Reg) {
        self.shift_cl(w, 0, dst);
    }
    pub fn ror(&mut self, w: bool, dst: Reg) {
        self.shift_cl(w, 1, dst);
    }

    /// `add dst, src` (64-bit) — folding an oversized memory offset into the
    /// address register.
    pub fn add_rr64(&mut self, dst: Reg, src: Reg) {
        self.rex(true, src.ext(), 0, dst.ext());
        self.b(0x01);
        self.modrm_rr(src, dst);
    }

    /// `mov dst, [base + index*2^scale + disp]` (64-bit) — a table entry.
    pub fn load64_scaled(&mut self, dst: Reg, base: Reg, index: Reg, scale: u8, disp: i32) {
        self.rex(true, dst.ext(), index.ext(), base.ext());
        self.b(0x8B);
        self.modrm_mem_sib(dst, base, index, scale, disp);
    }

    /// `mov dst, [base + index*2^scale + disp]` (32-bit).
    pub fn load32_scaled(&mut self, dst: Reg, base: Reg, index: Reg, scale: u8, disp: i32) {
        self.rex(false, dst.ext(), index.ext(), base.ext());
        self.b(0x8B);
        self.modrm_mem_sib(dst, base, index, scale, disp);
    }

    /// `cmovcc dst, src` (32-bit). A conditional move keeps `select` free of
    /// a branch, which matters because its condition is usually unpredictable.
    pub fn cmov(&mut self, w: bool, cc: Cond, dst: Reg, src: Reg) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0x40 + cc as u8);
        self.modrm_rr(dst, src);
    }

    /// `cmp a, b` (64-bit)
    pub fn cmp_rr64(&mut self, a: Reg, b: Reg) {
        self.rex(true, b.ext(), 0, a.ext());
        self.b(0x39);
        self.modrm_rr(b, a);
    }

    /// `cmp reg, imm32`. In the 64-bit form the immediate is sign-extended,
    /// which is what a comparison against -1 wants.
    pub fn cmp_r_imm32(&mut self, w: bool, r: Reg, v: i32) {
        self.rex(w, 0, 0, r.ext());
        self.b(0x81);
        self.b(0xF8 | r.low());
        self.d32(v);
    }
    pub fn cmp_r32_imm32(&mut self, r: Reg, v: i32) {
        self.cmp_r_imm32(false, r, v);
    }

    /// `lea dst, [rip + disp32]` with the displacement left blank; the
    /// returned offset is where it goes. RIP-relative is what makes a jump
    /// table survive being copied to a different address — everything in it
    /// is relative, so the code needs no relocation at map time.
    pub fn lea_rip_blank(&mut self, dst: Reg) -> usize {
        self.rex(true, dst.ext(), 0, 0);
        self.b(0x8D);
        self.b((dst.low() << 3) | 5); // mod=00, rm=101 -> RIP-relative
        let at = self.pos();
        self.d32(0);
        at
    }

    /// `movsxd dst, dword [base + index*2^scale + disp]` — sign-extending,
    /// because jump-table entries are offsets back to code that precedes the
    /// table and are therefore negative.
    pub fn movsxd_scaled(&mut self, dst: Reg, base: Reg, index: Reg, scale: u8, disp: i32) {
        self.rex(true, dst.ext(), index.ext(), base.ext());
        self.b(0x63);
        self.modrm_mem_sib(dst, base, index, scale, disp);
    }

    /// `jmp reg`
    pub fn jmp_reg(&mut self, r: Reg) {
        self.rex(false, 0, 0, r.ext());
        self.b(0xFF);
        self.b(0xE0 | r.low());
    }

    /// `rep movsb` — copy `rcx` bytes from `[rsi]` to `[rdi]`, in the
    /// direction the direction flag says. On this target (SSE4.2/AVX2 implies
    /// Haswell or later) the microcoded fast path applies, so this beats any
    /// loop we could write by hand.
    pub fn rep_movsb(&mut self) {
        self.b(0xF3);
        self.b(0xA4);
    }

    /// `rep stosb` — write `al` to `rcx` bytes at `[rdi]`.
    pub fn rep_stosb(&mut self) {
        self.b(0xF3);
        self.b(0xAA);
    }

    /// Direction flag. SysV requires it clear at every call boundary, so
    /// `std` may only ever be paired with a `cld` before anything else can
    /// happen — no call may sit between them.
    pub fn std(&mut self) {
        self.b(0xFD);
    }
    pub fn cld(&mut self) {
        self.b(0xFC);
    }

    /// Raw four bytes — a jump-table entry, not an instruction.
    pub fn emit_i32(&mut self, v: i32) {
        self.d32(v);
    }

    pub fn patch_i32(&mut self, at: usize, v: i32) {
        self.code[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// `jmp rel32` with the displacement left blank, for a target the module
    /// linker fills in.
    pub fn jmp_rel32_blank(&mut self) -> usize {
        self.b(0xE9);
        let at = self.pos();
        self.d32(0);
        at
    }

    /// `jmp [base + disp32]` — an indirect jump through memory, which is how a
    /// trap reaches the resume address without needing a register that has
    /// survived whatever went wrong.
    pub fn jmp_mem(&mut self, base: Reg, disp: i32) {
        self.rex(false, 0, 0, base.ext());
        self.b(0xFF);
        self.b(0xA0 | base.low());
        if base.low() == 4 {
            self.b(0x24);
        }
        self.d32(disp);
    }

    /// `lea dst, [rip + disp32]` with the displacement filled in now.
    pub fn lea_rip(&mut self, dst: Reg, disp: i32) {
        self.rex(true, dst.ext(), 0, 0);
        self.b(0x8D);
        self.b((dst.low() << 3) | 5);
        self.d32(disp);
    }

    /// `call rel32` with the displacement left blank. The offset of that
    /// blank is returned so the module linker can fill it once every function
    /// has an address — a single-pass generator cannot know one yet.
    pub fn call_rel32_blank(&mut self) -> usize {
        self.b(0xE8);
        let at = self.pos();
        self.d32(0);
        at
    }

    // --- integer ALU, register to register ---
    //
    // Everything below takes the operand width as a flag rather than being
    // written twice. `w` is REX.W: false is 32-bit (and zero-extends into the
    // upper half, which is exactly wasm's i32), true is 64-bit.

    /// The register-to-register form by raw opcode, for callers that carry an
    /// `AluOp` around.
    pub fn alu_raw(&mut self, w: bool, op: u8, dst: Reg, src: Reg) {
        self.alu(w, op, dst, src);
    }

    /// The variable-count shift, by raw extension number.
    pub fn shift_cl_pub(&mut self, w: bool, ext: u8, dst: Reg) {
        self.shift_cl(w, ext, dst);
    }

    fn alu(&mut self, w: bool, op: u8, dst: Reg, src: Reg) {
        self.rex(w, src.ext(), 0, dst.ext());
        self.b(op);
        self.modrm_rr(src, dst);
    }

    pub fn add(&mut self, w: bool, dst: Reg, src: Reg) {
        self.alu(w, 0x01, dst, src);
    }
    pub fn sub(&mut self, w: bool, dst: Reg, src: Reg) {
        self.alu(w, 0x29, dst, src);
    }
    pub fn and(&mut self, w: bool, dst: Reg, src: Reg) {
        self.alu(w, 0x21, dst, src);
    }
    pub fn or(&mut self, w: bool, dst: Reg, src: Reg) {
        self.alu(w, 0x09, dst, src);
    }
    pub fn xor(&mut self, w: bool, dst: Reg, src: Reg) {
        self.alu(w, 0x31, dst, src);
    }
    pub fn cmp(&mut self, w: bool, a: Reg, b: Reg) {
        self.alu(w, 0x39, a, b);
    }
    pub fn test(&mut self, w: bool, a: Reg, b: Reg) {
        self.alu(w, 0x85, a, b);
    }

    pub fn add32(&mut self, dst: Reg, src: Reg) {
        self.add(false, dst, src);
    }
    pub fn sub32(&mut self, dst: Reg, src: Reg) {
        self.sub(false, dst, src);
    }
    pub fn and32(&mut self, dst: Reg, src: Reg) {
        self.and(false, dst, src);
    }
    pub fn or32(&mut self, dst: Reg, src: Reg) {
        self.or(false, dst, src);
    }
    pub fn xor32(&mut self, dst: Reg, src: Reg) {
        self.xor(false, dst, src);
    }
    pub fn cmp32(&mut self, a: Reg, b: Reg) {
        self.cmp(false, a, b);
    }
    pub fn test32(&mut self, a: Reg, b: Reg) {
        self.test(false, a, b);
    }

    /// `op dst, [base + disp32]` — the register-from-memory direction, which
    /// is what lets an operand that is still in a frame slot be used where it
    /// lies instead of being loaded into a register first.
    pub fn alu_rm(&mut self, w: bool, op: u8, dst: Reg, base: Reg, disp: i32) {
        self.rex(w, dst.ext(), 0, base.ext());
        self.b(op);
        self.modrm_mem(dst, base, disp);
    }

    /// `op dst, imm32`. `ext` selects the operation: 0 add, 1 or, 4 and,
    /// 5 sub, 6 xor, 7 cmp. In the 64-bit form the immediate is
    /// sign-extended, so the caller must have checked that it fits.
    pub fn alu_r_imm32(&mut self, w: bool, ext: u8, dst: Reg, v: i32) {
        self.rex(w, 0, 0, dst.ext());
        self.b(0x81);
        self.b(0xC0 | (ext << 3) | dst.low());
        self.d32(v);
    }

    /// `imul dst, [base + disp32]`
    pub fn imul_rm(&mut self, w: bool, dst: Reg, base: Reg, disp: i32) {
        self.rex(w, dst.ext(), 0, base.ext());
        self.b(0x0F);
        self.b(0xAF);
        self.modrm_mem(dst, base, disp);
    }

    /// `imul dst, src, imm32` — the three-operand form, so a multiply by a
    /// constant needs no register for the constant.
    pub fn imul_r_imm32(&mut self, w: bool, dst: Reg, src: Reg, v: i32) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x69);
        self.modrm_rr(dst, src);
        self.d32(v);
    }

    /// Shift or rotate by a constant. Same `ext` numbering as `shift_cl`.
    pub fn shift_imm(&mut self, w: bool, ext: u8, dst: Reg, imm: u8) {
        self.rex(w, 0, 0, dst.ext());
        self.b(0xC1);
        self.b(0xC0 | (ext << 3) | dst.low());
        self.b(imm);
    }

    /// `imul dst, src` (two-operand form)
    pub fn imul(&mut self, w: bool, dst: Reg, src: Reg) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xAF);
        self.modrm_rr(dst, src);
    }
    pub fn imul32(&mut self, dst: Reg, src: Reg) {
        self.imul(false, dst, src);
    }

    /// `bsr`/`bsf` — index of the highest / lowest set bit. Both leave ZF set
    /// when the source is zero, and leave the destination undefined in that
    /// case, which is why every use pairs them with a `cmov`.
    ///
    /// This is deliberately NOT `lzcnt`/`tzcnt`: those need BMI1, and
    /// `targets/x86_64-nopeek.json` promises only up to SSE4.2 + AVX2. On a
    /// CPU without BMI1 their encodings decode as `bsr`/`bsf` with different
    /// semantics — a silent wrong answer, not a fault.
    pub fn bsr(&mut self, w: bool, dst: Reg, src: Reg) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xBD);
        self.modrm_rr(dst, src);
    }
    pub fn bsf(&mut self, w: bool, dst: Reg, src: Reg) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xBC);
        self.modrm_rr(dst, src);
    }

    /// `popcnt dst, src`. Rides on the target's `+sse4.2`: POPCNT and SSE4.2
    /// arrived on the same hardware generation, and LLVM's `sse4.2` implies it.
    pub fn popcnt(&mut self, w: bool, dst: Reg, src: Reg) {
        self.b(0xF3);
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xB8);
        self.modrm_rr(dst, src);
    }

    /// `cdq` / `cqo` — sign-extend the accumulator into the high half, which
    /// is where `idiv` expects the upper bits of its dividend.
    pub fn sign_extend_acc(&mut self, w: bool) {
        if w {
            self.b(0x48);
        }
        self.b(0x99);
    }

    /// `idiv src` — signed divide of the high:low accumulator pair. Raises
    /// #DE both when the divisor is zero and when the quotient does not fit,
    /// which after `cdq`/`cqo` means exactly `INT_MIN / -1`.
    pub fn idiv(&mut self, w: bool, src: Reg) {
        self.rex(w, 0, 0, src.ext());
        self.b(0xF7);
        self.b(0xF8 | src.low());
    }

    /// `div src` — unsigned. With the high half cleared the quotient always
    /// fits, so #DE means the divisor was zero and nothing else.
    pub fn div(&mut self, w: bool, src: Reg) {
        self.rex(w, 0, 0, src.ext());
        self.b(0xF7);
        self.b(0xF0 | src.low());
    }

    /// `movsxd dst, src` (register form) — i32 to i64, sign-extending.
    pub fn movsxd_rr(&mut self, dst: Reg, src: Reg) {
        self.rex(true, dst.ext(), 0, src.ext());
        self.b(0x63);
        self.modrm_rr(dst, src);
    }

    /// `movsx dst, src8` / `movsx dst, src16` (register form). The REX prefix
    /// is forced for the byte form so encodings 4..7 mean `spl/bpl/sil/dil`
    /// and not `ah/ch/dh/bh`.
    pub fn movsx8_rr(&mut self, w: bool, dst: Reg, src: Reg) {
        self.rex_always(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xBE);
        self.modrm_rr(dst, src);
    }
    pub fn movsx16_rr(&mut self, w: bool, dst: Reg, src: Reg) {
        self.rex(w, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0xBF);
        self.modrm_rr(dst, src);
    }

    /// `add rsp-class register, imm32` (64-bit)
    pub fn add_r64_imm32(&mut self, dst: Reg, v: i32) {
        self.rex(true, 0, 0, dst.ext());
        self.b(0x81);
        self.b(0xC0 | dst.low());
        self.d32(v);
    }

    /// `sub reg, imm32` (64-bit)
    pub fn sub_r64_imm32(&mut self, dst: Reg, v: i32) {
        self.rex(true, 0, 0, dst.ext());
        self.b(0x81);
        self.b(0xE8 | dst.low());
        self.d32(v);
    }

    // --- flags to value ---

    /// `setcc r8` then `movzx r32, r8`. The 8-bit half of rsi/rdi/rbp/rsp is
    /// only reachable with a REX prefix present, so force one.
    pub fn set_cond(&mut self, cc: Cond, dst: Reg) {
        self.rex_always(false, 0, 0, dst.ext());
        self.b(0x0F);
        self.b(0x90 + cc as u8);
        self.b(0xC0 | dst.low());
        // movzx dst32, dst8
        self.rex_always(false, dst.ext(), 0, dst.ext());
        self.b(0x0F);
        self.b(0xB6);
        self.modrm_rr(dst, dst);
    }

    // --- stack ---

    pub fn push(&mut self, r: Reg) {
        if r.ext() != 0 {
            self.b(0x41);
        }
        self.b(0x50 + r.low());
    }

    pub fn pop(&mut self, r: Reg) {
        if r.ext() != 0 {
            self.b(0x41);
        }
        self.b(0x58 + r.low());
    }

    pub fn ret(&mut self) {
        self.b(0xC3);
    }

    /// `ud2` — an unmistakable fault. wasm's `unreachable` must not be able
    /// to look like a successful return; once the trap handler exists this
    /// becomes a jump to it.
    pub fn ud2(&mut self) {
        self.b(0x0F);
        self.b(0x0B);
    }

    // --- control flow ---

    /// `jmp rel32` to a target bound later.
    pub fn jmp(&mut self) -> Patch {
        self.b(0xE9);
        let at = self.pos();
        self.d32(0);
        self.open += 1;
        Patch { at }
    }

    /// `jcc rel32` to a target bound later.
    pub fn jcc(&mut self, cc: Cond) -> Patch {
        self.b(0x0F);
        self.b(0x80 + cc as u8);
        let at = self.pos();
        self.d32(0);
        self.open += 1;
        Patch { at }
    }

    /// Point a forward branch at the current position.
    pub fn bind(&mut self, p: Patch) {
        let here = self.pos();
        let rel = (here as i64 - (p.at as i64 + 4)) as i32;
        self.code[p.at..p.at + 4].copy_from_slice(&rel.to_le_bytes());
        self.open -= 1;
    }

    /// `jmp` backwards to an already-known position (loop back edge).
    pub fn jmp_back(&mut self, target: usize) {
        self.b(0xE9);
        let rel = (target as i64 - (self.pos() as i64 + 4)) as i32;
        self.d32(rel);
    }

    /// `jcc` backwards to an already-known position.
    pub fn jcc_back(&mut self, cc: Cond, target: usize) {
        self.b(0x0F);
        self.b(0x80 + cc as u8);
        let rel = (target as i64 - (self.pos() as i64 + 4)) as i32;
        self.d32(rel);
    }

    /// `call reg` — indirect through a register.
    pub fn call_reg(&mut self, r: Reg) {
        self.rex(false, 0, 0, r.ext());
        self.b(0xFF);
        self.b(0xD0 | r.low());
    }
}

// === SSE: the float register file ===
//
// Every instruction below is SSE2 except the `round*` family, which is SSE4.1
// — and `targets/x86_64-nopeek.json` promises up to SSE4.2, so both are safe.
// Nothing here uses AVX encodings: the two-operand SSE forms keep the encoder
// small, and a single-pass generator gains nothing from three-operand forms it
// would only ever use as two.

/// The XMM registers, numbered as the hardware does so `reg & 7` is the ModRM
/// field and `reg >> 3` is the REX bit — same rule as the general registers.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Xmm {
    X0 = 0,
    X1 = 1,
    X2 = 2,
    X3 = 3,
    X4 = 4,
    X5 = 5,
    X6 = 6,
    X7 = 7,
    X8 = 8,
    X9 = 9,
    X10 = 10,
    X11 = 11,
    X12 = 12,
    X13 = 13,
    X14 = 14,
    X15 = 15,
}

impl Xmm {
    #[inline]
    fn low(self) -> u8 {
        self as u8 & 7
    }
    #[inline]
    fn ext(self) -> u8 {
        (self as u8 >> 3) & 1
    }
}

/// Which of the two float widths an instruction works on. `Single` picks the
/// `F3`-prefixed scalar forms and the packed-single bitwise ops, `Double` the
/// `F2`/`66` ones.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fw {
    Single,
    Double,
}

impl Fw {
    /// Prefix for the scalar arithmetic forms.
    fn scalar(self) -> u8 {
        match self {
            Fw::Single => 0xF3,
            Fw::Double => 0xF2,
        }
    }
    /// The bitwise ops come in packed-single and packed-double flavours; the
    /// double one needs a `66` prefix and the single one none at all. They do
    /// the same thing to the low lane either way, but keeping them apart
    /// avoids a domain-crossing penalty.
    fn packed_prefix(self) -> Option<u8> {
        match self {
            Fw::Single => None,
            Fw::Double => Some(0x66),
        }
    }
    pub fn is_double(self) -> bool {
        self == Fw::Double
    }
}

impl Asm {
    fn sse_pre(&mut self, prefix: Option<u8>, w: bool, r: u8, x: u8, b: u8) {
        if let Some(p) = prefix {
            self.b(p);
        }
        self.rex(w, r, x, b);
        self.b(0x0F);
    }

    /// `op dst, src` where both are XMM.
    fn sse_rr(&mut self, prefix: Option<u8>, w: bool, op: u8, dst: Xmm, src: Xmm) {
        self.sse_pre(prefix, w, dst.ext(), 0, src.ext());
        self.b(op);
        self.b(0xC0 | (dst.low() << 3) | src.low());
    }

    /// `op dst, [base + disp32]`.
    fn sse_rm(&mut self, prefix: Option<u8>, w: bool, op: u8, dst: Xmm, base: Reg, disp: i32) {
        self.sse_pre(prefix, w, dst.ext(), 0, base.ext());
        self.b(op);
        self.b(0x80 | (dst.low() << 3) | base.low());
        if base.low() == 4 {
            self.b(0x24);
        }
        self.d32(disp);
    }

    /// `op dst, [base + index + disp32]` — linear memory.
    fn sse_rmi(
        &mut self,
        prefix: Option<u8>,
        op: u8,
        dst: Xmm,
        base: Reg,
        index: Reg,
        disp: i32,
    ) {
        self.sse_pre(prefix, false, dst.ext(), index.ext(), base.ext());
        self.b(op);
        self.b(0x80 | (dst.low() << 3) | 4);
        self.b((index.low() << 3) | base.low());
        self.d32(disp);
    }

    // --- moves ---

    /// `movaps dst, src` — a plain register copy.
    pub fn fmov(&mut self, dst: Xmm, src: Xmm) {
        self.sse_rr(None, false, 0x28, dst, src);
    }

    /// `movss`/`movsd` between a frame slot and a register.
    pub fn fload_slot(&mut self, fw: Fw, dst: Xmm, base: Reg, disp: i32) {
        self.sse_rm(Some(fw.scalar()), false, 0x10, dst, base, disp);
    }
    pub fn fstore_slot(&mut self, fw: Fw, base: Reg, disp: i32, src: Xmm) {
        self.sse_rm(Some(fw.scalar()), false, 0x11, src, base, disp);
    }

    /// `movss`/`movsd` against linear memory.
    pub fn fload_idx(&mut self, fw: Fw, dst: Xmm, base: Reg, index: Reg, disp: i32) {
        self.sse_rmi(Some(fw.scalar()), 0x10, dst, base, index, disp);
    }
    pub fn fstore_idx(&mut self, fw: Fw, base: Reg, index: Reg, disp: i32, src: Xmm) {
        self.sse_rmi(Some(fw.scalar()), 0x11, src, base, index, disp);
    }

    /// `movd`/`movq` from a general register into an XMM — how a float
    /// constant, a mask, or a `reinterpret` gets across.
    pub fn gpr_to_xmm(&mut self, w: bool, dst: Xmm, src: Reg) {
        self.sse_pre(Some(0x66), w, dst.ext(), 0, src.ext());
        self.b(0x6E);
        self.b(0xC0 | (dst.low() << 3) | src.low());
    }

    /// `movd`/`movq` the other way.
    pub fn xmm_to_gpr(&mut self, w: bool, dst: Reg, src: Xmm) {
        self.sse_pre(Some(0x66), w, src.ext(), 0, dst.ext());
        self.b(0x7E);
        self.b(0xC0 | (src.low() << 3) | dst.low());
    }

    // --- scalar arithmetic ---

    fn farith(&mut self, fw: Fw, op: u8, dst: Xmm, src: Xmm) {
        self.sse_rr(Some(fw.scalar()), false, op, dst, src);
    }
    pub fn fadd(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x58, dst, src);
    }
    pub fn fsub(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x5C, dst, src);
    }
    pub fn fmul(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x59, dst, src);
    }
    pub fn fdiv(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x5E, dst, src);
    }
    pub fn fsqrt(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x51, dst, src);
    }
    /// The hardware's min/max, which do NOT match wasm's on NaN or on signed
    /// zero — see the generator for what has to be built around them.
    pub fn fmin_raw(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x5D, dst, src);
    }
    pub fn fmax_raw(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.farith(fw, 0x5F, dst, src);
    }

    // --- bitwise, for sign manipulation ---

    fn fbits(&mut self, fw: Fw, op: u8, dst: Xmm, src: Xmm) {
        self.sse_rr(fw.packed_prefix(), false, op, dst, src);
    }
    pub fn fand(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.fbits(fw, 0x54, dst, src);
    }
    /// `dst := (~dst) & src`
    pub fn fandn(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.fbits(fw, 0x55, dst, src);
    }
    pub fn f_or(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.fbits(fw, 0x56, dst, src);
    }
    pub fn fxor(&mut self, fw: Fw, dst: Xmm, src: Xmm) {
        self.fbits(fw, 0x57, dst, src);
    }

    // --- rounding (SSE4.1) ---

    /// `roundss`/`roundsd`. Mode 0 is nearest-even, 1 floor, 2 ceil,
    /// 3 truncate — the same four wasm asks for.
    pub fn fround(&mut self, fw: Fw, mode: u8, dst: Xmm, src: Xmm) {
        self.b(0x66);
        self.rex(false, dst.ext(), 0, src.ext());
        self.b(0x0F);
        self.b(0x3A);
        self.b(if fw.is_double() { 0x0B } else { 0x0A });
        self.b(0xC0 | (dst.low() << 3) | src.low());
        self.b(mode);
    }

    // --- comparison ---

    /// `ucomiss`/`ucomisd`: sets ZF/PF/CF. Unordered means PF=1, and that is
    /// the bit every wasm float comparison has to account for.
    pub fn fucomi(&mut self, fw: Fw, a: Xmm, b: Xmm) {
        self.sse_rr(fw.packed_prefix(), false, 0x2E, a, b);
    }

    // --- conversions ---

    /// `cvtss2sd` / `cvtsd2ss` — the source width is what the prefix names.
    pub fn fconvert(&mut self, from: Fw, dst: Xmm, src: Xmm) {
        self.sse_rr(Some(from.scalar()), false, 0x5A, dst, src);
    }

    /// `cvtsi2ss`/`cvtsi2sd` from a general register; `w` picks i32 or i64.
    pub fn int_to_float(&mut self, fw: Fw, w: bool, dst: Xmm, src: Reg) {
        self.sse_pre(Some(fw.scalar()), w, dst.ext(), 0, src.ext());
        self.b(0x2A);
        self.b(0xC0 | (dst.low() << 3) | src.low());
    }

    /// `cvttss2si`/`cvttsd2si` — truncating toward zero, which is what wasm
    /// wants. Out of range or NaN yields the "integer indefinite" value, and
    /// the generator has to sort out which case it was.
    pub fn float_to_int(&mut self, fw: Fw, w: bool, dst: Reg, src: Xmm) {
        self.sse_pre(Some(fw.scalar()), w, dst.ext(), 0, src.ext());
        self.b(0x2C);
        self.b(0xC0 | (dst.low() << 3) | src.low());
    }
}
