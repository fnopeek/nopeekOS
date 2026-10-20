//! The command stream of a scene, word for word as RADV writes it
//! (shaders_gen.rs): the queue preamble in an IB of its own, then per
//! dispatch the recorded template of its kind — first of the IB, the same
//! program after a barrier, or another program after a barrier — with the
//! holes filled, then the end of the command buffer and RADV's padding.
//! Not a Linux file.

use crate::shaders_gen::{self as sg, Hole, Kernel};
use alloc::vec::Vec;

/// `PKT3_NOP_PAD`: a one-dword NOP.
const PKT3_NOP_PAD: u32 = 0xffff_1000;
/// `PKT3(PKT3_NOP, count, 0)`.
fn pkt3_nop(count: u32) -> u32 {
    3 << 30 | (count & 0x3fff) << 16 | 0x10 << 8
}

/// `S_00B834_DATA`: COMPUTE_PGM_HI holds bits 47:40 of the program address.
pub fn pgm_hi() -> u32 {
    (sg::ADDRESS32_HI >> 8) & 0xff
}

/// What a dispatch fills its template's holes with.
pub struct dispatch<'a> {
    pub kernel: &'a Kernel,
    /// GPU address of the program.
    pub code_va: u64,
    /// The push constants, as dwords.
    pub push: &'a [u32],
    /// Low half of the address of the uploaded push constants, for a
    /// program that reads them through a pointer.
    pub push_ptr: u32,
    /// Low half of the address of its descriptor set, for a program that
    /// reads an image.
    pub set_ptr: u32,
    pub grid: [u32; 3],
}

/// `radv_cp_dma_prefetch`: the program from its 32-byte-aligned start to
/// its 32-byte-aligned end.
fn prefetch_size(va: u64, size: u32) -> u32 {
    let start = va & !31;
    let end = (va + size as u64 + 31) & !31;
    (end - start) as u32
}

fn fill(d: &dispatch, word: u32, hole: Hole) -> u32 {
    let k = d.kernel;
    let user_data = |n: u32| -> u32 {
        if k.push_ptr_sgpr == Some(n) {
            return d.push_ptr;
        }
        if k.set_sgpr == Some(n) {
            return d.set_ptr;
        }
        let i = n.wrapping_sub(k.inline_push_sgpr);
        d.push.get(i as usize).copied().unwrap_or(0)
    };
    match hole {
        Hole::Word => word,
        Hole::PgmLo => (d.code_va >> 8) as u32,
        Hole::PgmHi => pgm_hi(),
        Hole::ComputePgmRsrc1 => k.pgm_rsrc1,
        Hole::ComputePgmRsrc2 => k.pgm_rsrc2,
        Hole::ComputeResourceLimits => k.resource_limits,
        Hole::ComputeNumThreadX => k.num_thread[0],
        Hole::ComputeNumThreadY => k.num_thread[1],
        Hole::ComputeNumThreadZ => k.num_thread[2],
        Hole::GridX => d.grid[0],
        Hole::GridY => d.grid[1],
        Hole::GridZ => d.grid[2],
        Hole::PrefetchLo => d.code_va as u32,
        Hole::PrefetchHi => (d.code_va >> 32) as u32,
        // BYTE_COUNT is the low 21 bits; the rest are RADV's flags.
        Hole::PrefetchSize => (word & !0x1f_ffff) | prefetch_size(d.code_va, k.code.len() as u32),
        Hole::UserData2 => user_data(2),
        Hole::UserData3 => user_data(3),
        Hole::UserData4 => user_data(4),
        Hole::UserData5 => user_data(5),
        Hole::UserData6 => user_data(6),
        Hole::UserData7 => user_data(7),
        Hole::UserData8 => user_data(8),
        Hole::UserData9 => user_data(9),
        Hole::UserData10 => user_data(10),
        Hole::UserData11 => user_data(11),
        Hole::UserData12 => user_data(12),
    }
}

/// `tpl` with `d`'s holes filled, appended to `out`.
pub fn emit(out: &mut Vec<u32>, tpl: &[(u32, Hole)], d: &dispatch) {
    out.extend(tpl.iter().map(|&(w, h)| fill(d, w, h)));
}

/// The queue preamble (`initial_preamble_cs`), finalized.
pub fn preamble(out: &mut Vec<u32>) {
    out.extend(sg::RADV_PREAMBLE.iter().map(|&(w, h)| if h == Hole::PgmHi { pgm_hi() } else { w }));
    finalize(out);
}

/// The end of a command buffer (`vkEndCommandBuffer`), finalized.
pub fn end(out: &mut Vec<u32>) {
    out.extend(sg::RADV_END.iter().map(|&(w, _)| w));
    finalize(out);
}

/// `radv_amdgpu_winsys_cs_pad` leaving `leave` dwords: one NOP packet up
/// to the IB alignment of the compute queue, its body left as it is, or a
/// one-dword NOP.
fn pad(out: &mut Vec<u32>, leave: usize) {
    let mask = sg::IB_PAD_DW_MASK as usize;
    let unaligned = (out.len() + leave) & mask;
    if unaligned == 0 {
        return;
    }
    let remaining = mask + 1 - unaligned;
    if remaining == 1 {
        out.push(PKT3_NOP_PAD);
    } else {
        out.push(pkt3_nop(remaining as u32 - 2));
        out.extend(core::iter::repeat_n(0, remaining - 1));
    }
}

/// `radv_amdgpu_cs_finalize` of an IB that chains: padded so that the four
/// dwords a chaining INDIRECT_BUFFER would take end it, held by a NOP.
fn finalize(out: &mut Vec<u32>) {
    pad(out, 4);
    out.push(pkt3_nop(2));
    out.extend([0, 0, 0]);
}

/// Which template a dispatch takes after `prev`, the program of the
/// dispatch before it in the IB.
pub fn template_for(k: usize, prev: Option<usize>) -> &'static [(u32, Hole)] {
    let t = &TEMPLATES[k];
    match prev {
        None => t.0,
        Some(p) if p == k => t.1,
        Some(_) => t.2,
    }
}

/// The programs, in the order the composer uploads them.
pub const KERNELS: [&Kernel; 5] = [&sg::FILL_PATTERN, &sg::COMPOSE, &sg::BLUR_DOWN, &sg::BLUR_BOX, &sg::IMPORT];
pub const K_FILL_PATTERN: usize = 0;
pub const K_COMPOSE: usize = 1;
pub const K_BLUR_DOWN: usize = 2;
pub const K_BLUR_BOX: usize = 3;
pub const K_IMPORT: usize = 4;

/// Per program: first dispatch, same program again, after another one.
type templates = (&'static [(u32, Hole)], &'static [(u32, Hole)], &'static [(u32, Hole)]);
const TEMPLATES: [templates; 5] = [
    (sg::FILL_PATTERN_RADV_DISPATCH, sg::FILL_PATTERN_RADV_REPUSH, sg::FILL_PATTERN_RADV_SWITCH),
    (sg::COMPOSE_RADV_DISPATCH, sg::COMPOSE_RADV_REPUSH, sg::COMPOSE_RADV_SWITCH),
    (sg::BLUR_DOWN_RADV_DISPATCH, sg::BLUR_DOWN_RADV_REPUSH, sg::BLUR_DOWN_RADV_SWITCH),
    (sg::BLUR_BOX_RADV_DISPATCH, sg::BLUR_BOX_RADV_REPUSH, sg::BLUR_BOX_RADV_SWITCH),
    (sg::IMPORT_RADV_DISPATCH, sg::IMPORT_RADV_REPUSH, sg::IMPORT_RADV_SWITCH),
];
