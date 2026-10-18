//! amdgpu/atom.c (Linux 6.18.55): the ATOM command table interpreter.
//! The card's accessors (`struct card_info`, amdgpu_atombios.c's `cail_*`)
//! are `atom_card`; Linux's PLL and MC accessors read 0 and write nothing.

use crate::atom::atom_context;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

/// `struct card_info`.
pub trait atom_card {
    fn reg_read(&self, reg: u32) -> u32;
    fn reg_write(&self, reg: u32, val: u32);
    /// `cail_pll_read`.
    fn pll_read(&self, _reg: u32) -> u32 {
        0
    }
    /// `cail_pll_write`.
    fn pll_write(&self, _reg: u32, _val: u32) {}
    /// `cail_mc_read`.
    fn mc_read(&self, _reg: u32) -> u32 {
        0
    }
    /// `cail_mc_write`.
    fn mc_write(&self, _reg: u32, _val: u32) {}
}

const ATOM_ARG_MASK: [u32; 8] = [0xFFFF_FFFF, 0xFFFF, 0xFF_FF00, 0xFFFF_0000, 0xFF, 0xFF00, 0xFF_0000, 0xFF00_0000];
const ATOM_ARG_SHIFT: [u32; 8] = [0, 0, 8, 16, 0, 8, 16, 24];
// translate destination alignment field to the source alignment encoding
const ATOM_DST_TO_SRC: [[u32; 4]; 8] = [
    [0, 0, 0, 0],
    [1, 2, 3, 0],
    [1, 2, 3, 0],
    [1, 2, 3, 0],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
    [4, 5, 6, 7],
];
const ATOM_DEF_DST: [u32; 8] = [0, 0, 1, 2, 0, 1, 2, 3];

/// `atom_exec_context`.
struct atom_exec_context<'a> {
    ctx: &'a mut atom_context,
    card: &'a dyn atom_card,
    ps: &'a mut [u32],
    ws: Vec<u32>,
    ps_shift: usize,
    start: u32,
    last_jump: u32,
    last_jump_ms: u64,
    abort: bool,
}

impl atom_exec_context<'_> {
    fn u8_(&self, p: u32) -> u32 {
        self.ctx.cu8(p) as u32
    }

    fn u16_(&self, p: u32) -> u32 {
        self.ctx.cu16(p) as u32
    }

    fn u32_(&self, p: u32) -> u32 {
        self.ctx.cu32(p)
    }
}

/// `atom_iio_execute`.
fn atom_iio_execute(ctx: &atom_context, card: &dyn atom_card, mut base: u32, index: u32, data: u32) -> u32 {
    let mut temp: u32 = 0xCDCD_CDCD;
    let m = |n: u32| 0xFFFF_FFFFu32.checked_shr(32 - n).unwrap_or(0);
    loop {
        match ctx.cu8(base) as u32 {
            x if x == ATOM_IIO_NOP => base += 1,
            x if x == ATOM_IIO_READ => {
                temp = card.reg_read(ctx.cu16(base + 1) as u32);
                base += 3;
            }
            x if x == ATOM_IIO_WRITE => {
                card.reg_write(ctx.cu16(base + 1) as u32, temp);
                base += 3;
            }
            x if x == ATOM_IIO_CLEAR => {
                temp &= !(m(ctx.cu8(base + 1) as u32) << ctx.cu8(base + 2));
                base += 3;
            }
            x if x == ATOM_IIO_SET => {
                temp |= m(ctx.cu8(base + 1) as u32) << ctx.cu8(base + 2);
                base += 3;
            }
            x if x == ATOM_IIO_MOVE_INDEX || x == ATOM_IIO_MOVE_DATA || x == ATOM_IIO_MOVE_ATTR => {
                let w = m(ctx.cu8(base + 1) as u32);
                let src = if x == ATOM_IIO_MOVE_INDEX { index } else if x == ATOM_IIO_MOVE_DATA { data } else { ctx.io_attr as u32 };
                temp &= !(w << ctx.cu8(base + 3));
                temp |= ((src >> ctx.cu8(base + 2)) & w) << ctx.cu8(base + 3);
                base += 4;
            }
            x if x == ATOM_IIO_END => return temp,
            _ => {
                Line::new().s("Unknown IIO opcode").say();
                return 0;
            }
        }
    }
}

/// `atom_get_src_int`.
fn atom_get_src_int(e: &mut atom_exec_context, attr: u32, ptr: &mut u32, saved: Option<&mut u32>) -> u32 {
    let arg = attr & 7;
    let align = (attr >> 3) & 7;
    let mut val: u32 = 0xCDCD_CDCD;
    match arg {
        a if a == ATOM_ARG_REG => {
            let mut idx = e.u16_(*ptr);
            *ptr += 2;
            idx = idx.wrapping_add(e.ctx.reg_block as u32);
            match e.ctx.io_mode {
                m if m == ATOM_IO_MM => val = e.card.reg_read(idx),
                m if m == ATOM_IO_PCI => {
                    Line::new().s("PCI registers are not implemented").say();
                    return 0;
                }
                m if m == ATOM_IO_SYSIO => {
                    Line::new().s("SYSIO registers are not implemented").say();
                    return 0;
                }
                m => {
                    if m & 0x80 == 0 {
                        Line::new().s("Bad IO mode").say();
                        return 0;
                    }
                    let iio = e.ctx.iio.get((m & 0x7F) as usize).copied().unwrap_or(0);
                    if iio == 0 {
                        Line::new().s("Undefined indirect IO read method ").dec((m & 0x7F) as u64).say();
                        return 0;
                    }
                    val = atom_iio_execute(e.ctx, e.card, iio as u32, idx, 0);
                }
            }
        }
        a if a == ATOM_ARG_PS => {
            let idx = e.u8_(*ptr) as usize;
            *ptr += 1;
            if idx < e.ps.len() {
                val = e.ps[idx];
            } else {
                Line::new().s("PS index out of range: ").dec(idx as u64).s(" > ").dec(e.ps.len() as u64).say();
            }
        }
        a if a == ATOM_ARG_WS => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            let g = &e.ctx;
            val = match idx {
                i if i == ATOM_WS_QUOTIENT => g.divmul[0],
                i if i == ATOM_WS_REMAINDER => g.divmul[1],
                i if i == ATOM_WS_DATAPTR => g.data_block as u32,
                i if i == ATOM_WS_SHIFT => g.shift as u32,
                i if i == ATOM_WS_OR_MASK => 1u32.wrapping_shl(g.shift as u32),
                i if i == ATOM_WS_AND_MASK => !1u32.wrapping_shl(g.shift as u32),
                i if i == ATOM_WS_FB_WINDOW => g.fb_base,
                i if i == ATOM_WS_ATTRIBUTES => g.io_attr as u32,
                i if i == ATOM_WS_REGPTR => g.reg_block as u32,
                i => {
                    if (i as usize) < e.ws.len() {
                        e.ws[i as usize]
                    } else {
                        Line::new().s("WS index out of range: ").dec(i as u64).s(" > ").dec(e.ws.len() as u64).say();
                        val
                    }
                }
            };
        }
        a if a == ATOM_ARG_ID => {
            let idx = e.u16_(*ptr);
            *ptr += 2;
            val = e.u32_(idx + e.ctx.data_block as u32);
        }
        a if a == ATOM_ARG_FB => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            if e.ctx.fb_base.wrapping_add(idx * 4) > e.ctx.scratch_size_bytes as u32 {
                Line::new().s("ATOM: fb read beyond scratch region: ").dec(e.ctx.fb_base.wrapping_add(idx * 4) as u64).s(" vs. ")
                    .dec(e.ctx.scratch_size_bytes as u64).say();
                val = 0;
            } else {
                val = e.ctx.scratch.get((e.ctx.fb_base / 4 + idx) as usize).copied().unwrap_or(0);
            }
        }
        a if a == ATOM_ARG_IMM => {
            match align {
                s if s == ATOM_SRC_DWORD => {
                    val = e.u32_(*ptr);
                    *ptr += 4;
                }
                s if s == ATOM_SRC_WORD0 || s == ATOM_SRC_WORD8 || s == ATOM_SRC_WORD16 => {
                    val = e.u16_(*ptr);
                    *ptr += 2;
                }
                _ => {
                    val = e.u8_(*ptr);
                    *ptr += 1;
                }
            }
            return val;
        }
        a if a == ATOM_ARG_PLL => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            val = e.card.pll_read(idx);
        }
        a if a == ATOM_ARG_MC => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            val = e.card.mc_read(idx);
        }
        _ => {}
    }
    if let Some(s) = saved {
        *s = val;
    }
    val &= ATOM_ARG_MASK[align as usize];
    val >> ATOM_ARG_SHIFT[align as usize]
}

/// `atom_skip_src_int`.
fn atom_skip_src_int(attr: u32, ptr: &mut u32) {
    let (align, arg) = ((attr >> 3) & 7, attr & 7);
    match arg {
        a if a == ATOM_ARG_REG || a == ATOM_ARG_ID => *ptr += 2,
        a if a == ATOM_ARG_PLL || a == ATOM_ARG_MC || a == ATOM_ARG_PS || a == ATOM_ARG_WS || a == ATOM_ARG_FB => {
            *ptr += 1
        }
        a if a == ATOM_ARG_IMM => match align {
            s if s == ATOM_SRC_DWORD => *ptr += 4,
            s if s == ATOM_SRC_WORD0 || s == ATOM_SRC_WORD8 || s == ATOM_SRC_WORD16 => *ptr += 2,
            _ => *ptr += 1,
        },
        _ => {}
    }
}

fn atom_get_src(e: &mut atom_exec_context, attr: u32, ptr: &mut u32) -> u32 {
    atom_get_src_int(e, attr, ptr, None)
}

/// `atom_get_src_direct`.
fn atom_get_src_direct(e: &atom_exec_context, align: u32, ptr: &mut u32) -> u32 {
    match align {
        s if s == ATOM_SRC_DWORD => {
            let v = e.u32_(*ptr);
            *ptr += 4;
            v
        }
        s if s == ATOM_SRC_WORD0 || s == ATOM_SRC_WORD8 || s == ATOM_SRC_WORD16 => {
            let v = e.u16_(*ptr);
            *ptr += 2;
            v
        }
        s if s == ATOM_SRC_BYTE0 || s == ATOM_SRC_BYTE8 || s == ATOM_SRC_BYTE16 || s == ATOM_SRC_BYTE24 => {
            let v = e.u8_(*ptr);
            *ptr += 1;
            v
        }
        _ => 0xCDCD_CDCD,
    }
}

fn dst_align(attr: u32) -> u32 {
    ATOM_DST_TO_SRC[((attr >> 3) & 7) as usize][((attr >> 6) & 3) as usize]
}

fn atom_get_dst(e: &mut atom_exec_context, arg: u32, attr: u32, ptr: &mut u32, saved: Option<&mut u32>) -> u32 {
    atom_get_src_int(e, arg | dst_align(attr) << 3, ptr, saved)
}

fn atom_skip_dst(arg: u32, attr: u32, ptr: &mut u32) {
    atom_skip_src_int(arg | dst_align(attr) << 3, ptr);
}

/// `atom_put_dst`.
fn atom_put_dst(e: &mut atom_exec_context, arg: u32, attr: u32, ptr: &mut u32, mut val: u32, mut saved: u32) {
    let align = dst_align(attr) as usize;
    val <<= ATOM_ARG_SHIFT[align];
    val &= ATOM_ARG_MASK[align];
    saved &= !ATOM_ARG_MASK[align];
    val |= saved;
    match arg {
        a if a == ATOM_ARG_REG => {
            let mut idx = e.u16_(*ptr);
            *ptr += 2;
            idx = idx.wrapping_add(e.ctx.reg_block as u32);
            match e.ctx.io_mode {
                m if m == ATOM_IO_MM => {
                    if idx == 0 { e.card.reg_write(idx, val << 2) } else { e.card.reg_write(idx, val) }
                }
                m if m == ATOM_IO_PCI => {
                    Line::new().s("PCI registers are not implemented").say();
                }
                m if m == ATOM_IO_SYSIO => {
                    Line::new().s("SYSIO registers are not implemented").say();
                }
                m => {
                    if m & 0x80 == 0 {
                        Line::new().s("Bad IO mode").say();
                        return;
                    }
                    // `iio[io_mode & 0xFF]`, as Linux indexes it here.
                    let iio = e.ctx.iio.get((m & 0xFF) as usize).copied().unwrap_or(0);
                    if iio == 0 {
                        Line::new().s("Undefined indirect IO write method ").dec((m & 0x7F) as u64).say();
                        return;
                    }
                    atom_iio_execute(e.ctx, e.card, iio as u32, idx, val);
                }
            }
        }
        a if a == ATOM_ARG_PS => {
            let idx = e.u8_(*ptr) as usize;
            *ptr += 1;
            if idx >= e.ps.len() {
                Line::new().s("PS index out of range: ").dec(idx as u64).s(" > ").dec(e.ps.len() as u64).say();
                return;
            }
            e.ps[idx] = val;
        }
        a if a == ATOM_ARG_WS => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            let g = &mut e.ctx;
            match idx {
                i if i == ATOM_WS_QUOTIENT => g.divmul[0] = val,
                i if i == ATOM_WS_REMAINDER => g.divmul[1] = val,
                i if i == ATOM_WS_DATAPTR => g.data_block = val as u16,
                i if i == ATOM_WS_SHIFT => g.shift = val as u8,
                i if i == ATOM_WS_OR_MASK || i == ATOM_WS_AND_MASK => {}
                i if i == ATOM_WS_FB_WINDOW => g.fb_base = val,
                i if i == ATOM_WS_ATTRIBUTES => g.io_attr = val as u16,
                i if i == ATOM_WS_REGPTR => g.reg_block = val as u16,
                i => {
                    if (i as usize) >= e.ws.len() {
                        Line::new().s("WS index out of range: ").dec(i as u64).s(" > ").dec(e.ws.len() as u64).say();
                        return;
                    }
                    e.ws[i as usize] = val;
                }
            }
        }
        a if a == ATOM_ARG_FB => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            if e.ctx.fb_base.wrapping_add(idx * 4) > e.ctx.scratch_size_bytes as u32 {
                Line::new().s("ATOM: fb write beyond scratch region: ").dec(e.ctx.fb_base.wrapping_add(idx * 4) as u64).s(" vs. ")
                    .dec(e.ctx.scratch_size_bytes as u64).say();
            } else if let Some(s) = e.ctx.scratch.get_mut((e.ctx.fb_base / 4 + idx) as usize) {
                *s = val;
            }
        }
        a if a == ATOM_ARG_PLL => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            e.card.pll_write(idx, val);
        }
        a if a == ATOM_ARG_MC => {
            let idx = e.u8_(*ptr);
            *ptr += 1;
            e.card.mc_write(idx, val);
        }
        _ => {}
    }
}

/// The ops with a destination, a source and `dst = f(dst, src)`.
fn atom_op_arith(e: &mut atom_exec_context, ptr: &mut u32, arg: u32, f: fn(u32, u32) -> u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    let dst = atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    let src = atom_get_src(e, attr, ptr);
    atom_put_dst(e, arg, attr, &mut dptr, f(dst, src), saved);
}

/// `atom_op_calltable`.
fn atom_op_calltable(e: &mut atom_exec_context, ptr: &mut u32) {
    let idx = e.u8_(*ptr);
    *ptr += 1;
    let mut r = 0;
    if e.u16_(e.ctx.cmd_table + 4 + 2 * idx) != 0 {
        let shift = e.ps_shift.min(e.ps.len());
        r = amdgpu_atom_execute_table_locked(e.ctx, e.card, idx, &mut e.ps[shift..]);
    }
    if r != 0 {
        e.abort = true;
    }
}

/// `atom_op_clear`.
fn atom_op_clear(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let mut attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    attr &= 0x38;
    attr |= ATOM_DEF_DST[(attr >> 3) as usize] << 6;
    atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    atom_put_dst(e, arg, attr, &mut dptr, 0, saved);
}

/// `atom_op_compare`.
fn atom_op_compare(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    e.ctx.cs_equal = dst == src;
    e.ctx.cs_above = dst > src;
}

/// `atom_op_delay`.
fn atom_op_delay(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let count = e.u8_(*ptr) as u64;
    *ptr += 1;
    if arg == ATOM_UNIT_MICROSEC {
        crate::linux::udelay(count);
    } else {
        crate::linux::udelay(count * 1000);
    }
}

/// `atom_op_div`.
fn atom_op_div(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    if src != 0 {
        e.ctx.divmul[0] = dst / src;
        e.ctx.divmul[1] = dst % src;
    } else {
        e.ctx.divmul = [0, 0];
    }
}

/// `atom_op_div32`.
fn atom_op_div32(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    if src != 0 {
        let val64 = (dst as u64 | (e.ctx.divmul[1] as u64) << 32) / src as u64;
        e.ctx.divmul[0] = val64 as u32;
        e.ctx.divmul[1] = (val64 >> 32) as u32;
    } else {
        e.ctx.divmul = [0, 0];
    }
}

/// `atom_op_jump`; the loop check counts milliseconds as Linux counts
/// jiffies.
fn atom_op_jump(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let target = e.u16_(*ptr);
    *ptr += 2;
    let g = &e.ctx;
    let execute = match arg {
        a if a == ATOM_COND_ABOVE => g.cs_above,
        a if a == ATOM_COND_ABOVEOREQUAL => g.cs_above || g.cs_equal,
        a if a == ATOM_COND_ALWAYS => true,
        a if a == ATOM_COND_BELOW => !(g.cs_above || g.cs_equal),
        a if a == ATOM_COND_BELOWOREQUAL => !g.cs_above,
        a if a == ATOM_COND_EQUAL => g.cs_equal,
        a if a == ATOM_COND_NOTEQUAL => !g.cs_equal,
        _ => false,
    };
    if execute {
        let now = crate::linux::now_us() / 1000;
        if e.last_jump == e.start + target {
            if now > e.last_jump_ms {
                if now - e.last_jump_ms > ATOM_CMD_TIMEOUT_SEC as u64 * 1000 {
                    Line::new().s("atombios stuck in loop for more than ").dec(ATOM_CMD_TIMEOUT_SEC as u64)
                        .s("secs aborting").say();
                    e.abort = true;
                }
            } else {
                e.last_jump_ms = now;
            }
        } else {
            e.last_jump = e.start + target;
            e.last_jump_ms = now;
        }
        *ptr = e.start + target;
    }
}

/// `atom_op_mask`.
fn atom_op_mask(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    let mut dst = atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    let mask = atom_get_src_direct(e, (attr >> 3) & 7, ptr);
    let src = atom_get_src(e, attr, ptr);
    dst &= mask;
    dst |= src;
    atom_put_dst(e, arg, attr, &mut dptr, dst, saved);
}

/// `atom_op_move`.
fn atom_op_move(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    if (attr >> 3) & 7 != ATOM_SRC_DWORD {
        atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    } else {
        atom_skip_dst(arg, attr, ptr);
        saved = 0xCDCD_CDCD;
    }
    let src = atom_get_src(e, attr, ptr);
    atom_put_dst(e, arg, attr, &mut dptr, src, saved);
}

/// `atom_op_mul`.
fn atom_op_mul(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    e.ctx.divmul[0] = dst.wrapping_mul(src);
}

/// `atom_op_mul32`.
fn atom_op_mul32(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    let val64 = dst as u64 * src as u64;
    e.ctx.divmul[0] = val64 as u32;
    e.ctx.divmul[1] = (val64 >> 32) as u32;
}

/// `atom_op_setdatablock`.
fn atom_op_setdatablock(e: &mut atom_exec_context, ptr: &mut u32) {
    let idx = e.u8_(*ptr);
    *ptr += 1;
    e.ctx.data_block = match idx {
        0 => 0,
        255 => e.start as u16,
        i => e.ctx.cu16(e.ctx.data_table + 4 + 2 * i),
    };
}

/// `atom_op_setfbbase`.
fn atom_op_setfbbase(e: &mut atom_exec_context, ptr: &mut u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    e.ctx.fb_base = atom_get_src(e, attr, ptr);
}

/// `atom_op_setport`.
fn atom_op_setport(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    match arg {
        a if a == ATOM_PORT_ATI => {
            let port = e.u16_(*ptr);
            e.ctx.io_mode = if port == 0 { ATOM_IO_MM } else { ATOM_IO_IIO | port };
            *ptr += 2;
        }
        a if a == ATOM_PORT_PCI => {
            e.ctx.io_mode = ATOM_IO_PCI;
            *ptr += 1;
        }
        a if a == ATOM_PORT_SYSIO => {
            e.ctx.io_mode = ATOM_IO_SYSIO;
            *ptr += 1;
        }
        _ => {}
    }
}

/// `atom_op_setregblock`.
fn atom_op_setregblock(e: &mut atom_exec_context, ptr: &mut u32) {
    e.ctx.reg_block = e.u16_(*ptr) as u16;
    *ptr += 2;
}

/// `atom_op_shift_left` / `atom_op_shift_right`.
fn atom_op_shift(e: &mut atom_exec_context, ptr: &mut u32, arg: u32, left: bool) {
    let mut attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    attr &= 0x38;
    attr |= ATOM_DEF_DST[(attr >> 3) as usize] << 6;
    let mut dst = atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    let shift = atom_get_src_direct(e, ATOM_SRC_BYTE0, ptr) as u8 as u32;
    dst = if left { dst.wrapping_shl(shift) } else { dst.wrapping_shr(shift) };
    if shift >= 32 {
        dst = 0;
    }
    atom_put_dst(e, arg, attr, &mut dptr, dst, saved);
}

/// `atom_op_shl` / `atom_op_shr`: on the full destination value.
fn atom_op_shlr(e: &mut atom_exec_context, ptr: &mut u32, arg: u32, left: bool) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let mut saved = 0;
    let mut dptr = *ptr;
    let align = dst_align(attr) as usize;
    atom_get_dst(e, arg, attr, ptr, Some(&mut saved));
    // op needs to full dst value
    let mut dst = saved;
    let shift = atom_get_src(e, attr, ptr) as u8 as u32;
    dst = if shift >= 32 { 0 } else if left { dst << shift } else { dst >> shift };
    dst &= ATOM_ARG_MASK[align];
    dst >>= ATOM_ARG_SHIFT[align];
    atom_put_dst(e, arg, attr, &mut dptr, dst, saved);
}

/// `atom_op_switch`.
fn atom_op_switch(e: &mut atom_exec_context, ptr: &mut u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let src = atom_get_src(e, attr, ptr);
    while e.u16_(*ptr) != ATOM_CASE_END {
        if e.u8_(*ptr) == ATOM_CASE_MAGIC {
            *ptr += 1;
            let val = atom_get_src(e, (attr & 0x38) | ATOM_ARG_IMM, ptr);
            let target = e.u16_(*ptr);
            if val == src {
                *ptr = e.start + target;
                return;
            }
            *ptr += 2;
        } else {
            Line::new().s("Bad case").say();
            return;
        }
    }
    *ptr += 2;
}

/// `atom_op_test`.
fn atom_op_test(e: &mut atom_exec_context, ptr: &mut u32, arg: u32) {
    let attr = e.u8_(*ptr);
    *ptr += 1;
    let dst = atom_get_dst(e, arg, attr, ptr, None);
    let src = atom_get_src(e, attr, ptr);
    e.ctx.cs_equal = dst & src == 0;
}

/// One entry of `opcode_table`.
fn atom_op(e: &mut atom_exec_context, op: u32, ptr: &mut u32) {
    // ARG order of each six-entry group: REG PS WS FB PLL MC.
    let args = [ATOM_ARG_REG, ATOM_ARG_PS, ATOM_ARG_WS, ATOM_ARG_FB, ATOM_ARG_PLL, ATOM_ARG_MC];
    let g = |base: u32| args[(op - base) as usize];
    match op {
        1..=6 => atom_op_move(e, ptr, g(1)),
        7..=12 => atom_op_arith(e, ptr, g(7), |d, s| d & s),
        13..=18 => atom_op_arith(e, ptr, g(13), |d, s| d | s),
        19..=24 => atom_op_shift(e, ptr, g(19), true),
        25..=30 => atom_op_shift(e, ptr, g(25), false),
        31..=36 => atom_op_mul(e, ptr, g(31)),
        37..=42 => atom_op_div(e, ptr, g(37)),
        43..=48 => atom_op_arith(e, ptr, g(43), |d, s| d.wrapping_add(s)),
        49..=54 => atom_op_arith(e, ptr, g(49), |d, s| d.wrapping_sub(s)),
        55 => atom_op_setport(e, ptr, ATOM_PORT_ATI),
        56 => atom_op_setport(e, ptr, ATOM_PORT_PCI),
        57 => atom_op_setport(e, ptr, ATOM_PORT_SYSIO),
        58 => atom_op_setregblock(e, ptr),
        59 => atom_op_setfbbase(e, ptr),
        60..=65 => atom_op_compare(e, ptr, g(60)),
        66 => atom_op_switch(e, ptr),
        67 => atom_op_jump(e, ptr, ATOM_COND_ALWAYS),
        68 => atom_op_jump(e, ptr, ATOM_COND_EQUAL),
        69 => atom_op_jump(e, ptr, ATOM_COND_BELOW),
        70 => atom_op_jump(e, ptr, ATOM_COND_ABOVE),
        71 => atom_op_jump(e, ptr, ATOM_COND_BELOWOREQUAL),
        72 => atom_op_jump(e, ptr, ATOM_COND_ABOVEOREQUAL),
        73 => atom_op_jump(e, ptr, ATOM_COND_NOTEQUAL),
        74..=79 => atom_op_test(e, ptr, g(74)),
        80 => atom_op_delay(e, ptr, ATOM_UNIT_MILLISEC),
        81 => atom_op_delay(e, ptr, ATOM_UNIT_MICROSEC),
        82 => atom_op_calltable(e, ptr),
        83 => Line::new().s("unimplemented!").say(),
        84..=89 => atom_op_clear(e, ptr, g(84)),
        90 => {}
        91 => {}
        92..=97 => atom_op_mask(e, ptr, g(92)),
        98 => *ptr += 1,
        99 => Line::new().s("ATOM BIOS beeped!").say(),
        100 | 101 => Line::new().s("unimplemented!").say(),
        102 => atom_op_setdatablock(e, ptr),
        103..=108 => atom_op_arith(e, ptr, g(103), |d, s| d ^ s),
        109..=114 => atom_op_shlr(e, ptr, g(109), true),
        115..=120 => atom_op_shlr(e, ptr, g(115), false),
        121 => *ptr += 1,
        122 => {
            let val = e.u16_(*ptr);
            *ptr += val + 2;
        }
        123 => atom_op_mul32(e, ptr, ATOM_ARG_PS),
        124 => atom_op_mul32(e, ptr, ATOM_ARG_WS),
        125 => atom_op_div32(e, ptr, ATOM_ARG_PS),
        126 => atom_op_div32(e, ptr, ATOM_ARG_WS),
        _ => {}
    }
}

/// `amdgpu_atom_execute_table_locked`.
fn amdgpu_atom_execute_table_locked(ctx: &mut atom_context, card: &dyn atom_card, index: u32, params: &mut [u32]) -> i32 {
    let base = ctx.cu16(ctx.cmd_table + 4 + 2 * index) as u32;
    if base == 0 {
        return -22;
    }
    if ctx.execute_depth >= ATOM_EXECUTE_MAX_DEPTH {
        Line::new().s("atombios command table nesting exceeded limit (").dec(ATOM_EXECUTE_MAX_DEPTH as u64).s(")").say();
        return -40;
    }
    ctx.execute_depth += 1;
    let len = ctx.cu16(base + ATOM_CT_SIZE_PTR) as u32;
    let ws = ctx.cu8(base + ATOM_CT_WS_PTR) as usize;
    let ps = (ctx.cu8(base + ATOM_CT_PS_PTR) as u32 & ATOM_CT_PS_MASK) as usize;
    let mut ptr = base + ATOM_CT_CODE_PTR;
    let mut ret = 0;
    {
        let mut e = atom_exec_context {
            ctx,
            card,
            ps: params,
            ws: vec![0; ws],
            ps_shift: ps / 4,
            start: base,
            last_jump: 0,
            last_jump_ms: 0,
            abort: false,
        };
        loop {
            let op = e.u8_(ptr);
            ptr += 1;
            if e.abort {
                Line::new().s("atombios stuck executing ").hexw(base, 4).s(" (len ").dec(len as u64).s(", WS ")
                    .dec(ws as u64).s(", PS ").dec(ps as u64).s(") @ 0x").hexw(ptr - 1, 4).say();
                ret = -22;
                break;
            }
            if op < ATOM_OP_CNT && op > 0 {
                atom_op(&mut e, op, &mut ptr);
            } else {
                break;
            }
            if op == ATOM_OP_EOT {
                break;
            }
        }
    }
    ctx.execute_depth -= 1;
    ret
}

/// `amdgpu_atom_execute_table`: the data block, register block, fb window,
/// io mode and divmul reset first. Linux bounds a PS index by
/// `params_size` in bytes, not in dwords; the tables run on a copy that
/// long, zero past `params`, so every index Linux accepts is in bounds.
pub fn amdgpu_atom_execute_table(
    ctx: &mut atom_context,
    card: &dyn atom_card,
    index: u32,
    params: &mut [u32],
    params_size: usize,
) -> i32 {
    ctx.data_block = 0;
    ctx.reg_block = 0;
    ctx.fb_base = 0;
    ctx.io_mode = ATOM_IO_MM;
    ctx.divmul = [0, 0];
    let mut ps = vec![0u32; params_size.max(params.len())];
    ps[..params.len()].copy_from_slice(params);
    let r = amdgpu_atom_execute_table_locked(ctx, card, index, &mut ps[..params_size]);
    let n = params.len();
    params.copy_from_slice(&ps[..n]);
    r
}
