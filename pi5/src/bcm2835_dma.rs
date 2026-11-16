//! The BCM2712's DMA controller for a cyclic transfer from memory to a
//! peripheral, after Linux `drivers/dma/bcm2835-dma.c` (Raspberry Pi
//! tree): the 40-bit channels of `bcm2712_dma_cfg`, their control blocks
//! (`struct bcm2711_dma40_scb`), `bcm2835_dma_prep_dma_cyclic` with
//! `bcm2835_dma_create_cb_chain`, `bcm2835_dma_start_desc`, the interrupt
//! callback, `bcm2835_dma_abort` and the residue of `bcm2835_dma_tx_status`.
//!
//! Only the 40-bit channels are ported: `dma40` (`brcm,bcm2712-dma` at
//! 0x10_00010600) offers channels 6 to 11, all of them 40-bit, and HDMI
//! audio is their one user. Control blocks live in memory the caller owns
//! (uncached, 32-byte aligned); bus addresses equal physical addresses on
//! the BCM2712 (`dma-ranges` of the SoC bus is 1:1).

use crate::hw::{Block, Hw};

/// `BCM2835_DMA_CHAN_SIZE`: one channel's register window.
pub const CHAN_SIZE: u64 = 0x100;
/// `BCM2835_DMA_MAX_DMA_CHAN_SUPPORTED`.
const MAX_DMA_CHAN_SUPPORTED: u32 = 14;
/// `bcm2712_dma_cfg.chan_40bit_mask`.
pub const CHAN_40BIT_MASK: u32 = (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
/// `MAX_DMA40_LEN`.
const MAX_DMA40_LEN: usize = 1 << 30;
/// A control block's size and alignment (`struct bcm2711_dma40_scb`).
pub const CB_SIZE: usize = 32;

// BCM2711_DMA40_* channel registers.
const DMA40_CS: u64 = 0x00;
const DMA40_CB: u64 = 0x04;
const DMA40_DEBUG: u64 = 0x0c;
const DMA40_SRC: u64 = 0x14;
const DMA40_SRCI: u64 = 0x18;

// CS.
const DMA40_ACTIVE: u32 = 1 << 0;
const DMA40_INT: u32 = 1 << 2;
const DMA40_PROT: u32 = (1 << 8) | (1 << 9);
const DMA40_ERR: u32 = 1 << 10;
const DMA40_TRANSACTIONS: u32 = 1 << 25;
const DMA40_WAIT_FOR_WRITES: u32 = 1 << 28;
const DMA40_DISDEBUG: u32 = 1 << 29;
const fn dma40_qos(x: u32) -> u32 { (x & 0x1f) << 16 }
const fn dma40_panic_qos(x: u32) -> u32 { (x & 0x1f) << 20 }
/// `BCM2711_DMA40_CS_FLAGS`.
const fn dma40_cs_flags(x: u32) -> u32 {
    x & (dma40_qos(15) | dma40_panic_qos(15) | DMA40_WAIT_FOR_WRITES | DMA40_DISDEBUG)
}

// TI.
const DMA40_INTEN: u32 = 1 << 0;
const DMA40_WAIT_RESP: u32 = 1 << 2;
const DMA40_WAIT_RD_RESP: u32 = 1 << 3;
const fn dma40_per_map(x: u32) -> u32 { (x & 31) << 9 }
const DMA40_S_DREQ: u32 = 1 << 14;
const DMA40_D_DREQ: u32 = 1 << 15;

// SRCI / DSTI.
const fn dma40_burst_len(x: u32) -> u32 { (x & 15) << 8 }
const DMA40_INC: u32 = 1 << 12;
const DMA40_SIZE_128: u32 = 2 << 13;

// DEBUG.
const DMA40_DEBUG_RESET: u32 = 1 << 23;

// The legacy TI bits the 40-bit fields are derived from (`to_bcm2711_*`).
const DMA_INT_EN: u32 = 1 << 0;
const DMA_WAIT_RESP: u32 = 1 << 3;
const DMA_D_INC: u32 = 1 << 4;
const DMA_D_WIDTH: u32 = 1 << 5;
const DMA_D_DREQ: u32 = 1 << 6;
const DMA_S_INC: u32 = 1 << 8;
const DMA_S_WIDTH: u32 = 1 << 9;
const DMA_S_DREQ: u32 = 1 << 10;
const fn dma_burst_length(x: u32) -> u32 { (x & 15) << 12 }
const fn dma_get_burst_length(x: u32) -> u32 { (x >> 12) & 15 }
const fn dma_per_map(x: u32) -> u32 { (x & 31) << 16 }

// Driver flags carried in the `dmas` cell beside the DREQ number.
const DMA_NO_WAIT_RESP: u32 = 1 << 27;
const DMA_WIDE_SOURCE: u32 = 1 << 24;
const DMA_WIDE_DEST: u32 = 1 << 25;
const DMA_BURST: u32 = 1 << 30;

const fn wait_resp(x: u32) -> u32 { if x & DMA_NO_WAIT_RESP != 0 { 0 } else { DMA_WAIT_RESP } }
const fn wide_source(x: u32) -> u32 { if x & DMA_WIDE_SOURCE != 0 { DMA_S_WIDTH } else { 0 } }
const fn wide_dest(x: u32) -> u32 { if x & DMA_WIDE_DEST != 0 { DMA_D_WIDTH } else { 0 } }
const fn burst_length(x: u32) -> u32 { if x & DMA_BURST != 0 { dma_burst_length(3) } else { 0 } }

fn to_bcm2711_ti(info: u32) -> u32 {
    (if info & DMA_INT_EN != 0 { DMA40_INTEN } else { 0 })
        | (if info & DMA_WAIT_RESP != 0 { DMA40_WAIT_RESP } else { 0 })
        | (if info & DMA_S_DREQ != 0 { DMA40_S_DREQ | DMA40_WAIT_RD_RESP } else { 0 })
        | (if info & DMA_D_DREQ != 0 { DMA40_D_DREQ } else { 0 })
        | dma40_per_map((info >> 16) & 0x1f)
}

fn to_bcm2711_srci(info: u32) -> u32 {
    (if info & DMA_S_INC != 0 { DMA40_INC } else { 0 })
        | (if info & DMA_S_WIDTH != 0 { DMA40_SIZE_128 } else { 0 })
        | dma40_burst_len(dma_get_burst_length(info))
}

fn to_bcm2711_dsti(info: u32) -> u32 {
    (if info & DMA_D_INC != 0 { DMA40_INC } else { 0 })
        | (if info & DMA_D_WIDTH != 0 { DMA40_SIZE_128 } else { 0 })
        | dma40_burst_len(dma_get_burst_length(info))
}

/// `to_40bit_cbaddr`: control blocks are 32-byte aligned.
fn to_40bit_cbaddr(addr: u64) -> Option<u32> {
    (addr & 0x1f == 0).then_some((addr >> 5) as u32)
}

/// The channels a controller instance offers, as `bcm2835_dma_probe`
/// splits them: the instance's `reg` starts at channel
/// `(base / CHAN_SIZE) & 0xf`, and `brcm,dma-channel-mask` names the free
/// ones. Returns the controller's channel-0 origin and the 40-bit channels
/// in the order `dma_get_any_slave_channel` hands them out.
pub fn channels(reg_base: u64, reg_size: u64, chans_available: u32) -> (u64, impl Iterator<Item = u32>) {
    let chan_start = ((reg_base / CHAN_SIZE) & 0xf) as u32;
    let origin = reg_base - chan_start as u64 * CHAN_SIZE;
    let chan_count = (reg_size / CHAN_SIZE) as u32;
    let chan_end = (chan_start + chan_count).min(MAX_DMA_CHAN_SUPPORTED + 1);
    let list = (chan_start..chan_end)
        .filter(move |&i| chans_available & (1 << i) != 0 && CHAN_40BIT_MASK & (1 << i) != 0);
    (origin, list)
}

/// A cyclic memory-to-device transfer prepared in a caller's control
/// block memory (`struct bcm2835_desc` with `cyclic`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cyclic {
    pub frames: usize,
    pub buf: u64,
    pub buf_len: usize,
    pub period_len: usize,
    cb_bus: u64,
}

/// One 40-bit channel (`struct bcm2835_chan`, `is_40bit_channel`), its
/// registers at `ch * CHAN_SIZE` in [`Block::Dma`].
pub struct Chan {
    pub ch: u32,
    /// The `dmas` cell: DREQ in bits 4:0 and the driver flags.
    pub dreq: u32,
}

impl Chan {
    pub fn new(ch: u32, dreq: u32) -> Self {
        Chan { ch, dreq }
    }

    fn base(&self) -> u64 {
        self.ch as u64 * CHAN_SIZE
    }

    /// `bcm2835_dma_prep_dma_cyclic` for `DMA_MEM_TO_DEV` with
    /// `DMA_PREP_INTERRUPT`: one control block per period (a period never
    /// exceeds `MAX_DMA40_LEN`), each ending with an interrupt, the last
    /// linked back to the first. `cbs` is the control block memory as seen
    /// by the CPU, `cb_bus` its bus address; `dst` the device's FIFO.
    pub fn prep_cyclic(&self, cbs: &mut [[u32; 8]], cb_bus: u64, buf: u64, buf_len: usize,
                       period_len: usize, dst: u64) -> Option<Cyclic> {
        if buf_len == 0 || period_len == 0 || period_len > MAX_DMA40_LEN || cb_bus & 0x1f != 0 {
            return None;
        }
        let mut info = wait_resp(self.dreq) | wide_source(self.dreq) | wide_dest(self.dreq)
            | burst_length(self.dreq);
        let extra = DMA_INT_EN;
        if self.dreq != 0 {
            info |= dma_per_map(self.dreq);
        }
        info |= DMA_D_DREQ | DMA_S_INC;
        let frames = buf_len.div_ceil(period_len);
        if frames > cbs.len() {
            return None;
        }
        let (mut src, mut len, mut size) = (buf, buf_len, 0usize);
        for (frame, cb) in cbs.iter_mut().take(frames).enumerate() {
            let cb_len = len.min(period_len);
            *cb = [0; 8];
            cb[0] = to_bcm2711_ti(info) | to_bcm2711_ti(extra);
            cb[1] = src as u32;
            cb[2] = (src >> 32) as u32 | to_bcm2711_srci(info);
            cb[3] = dst as u32;
            cb[4] = (dst >> 32) as u32 | to_bcm2711_dsti(info);
            cb[5] = cb_len as u32;
            let next = if frame + 1 == frames { cb_bus } else { cb_bus + ((frame + 1) * CB_SIZE) as u64 };
            cb[6] = to_40bit_cbaddr(next)?;
            src += cb_len as u64;
            len -= cb_len;
            size += cb_len;
        }
        (size == buf_len).then_some(Cyclic { frames, buf, buf_len, period_len, cb_bus })
    }

    /// `bcm2835_dma_start_desc`.
    pub fn start(&self, hw: &mut impl Hw, d: &Cyclic) {
        let Some(cb) = to_40bit_cbaddr(d.cb_bus) else { return };
        hw.w32(Block::Dma, self.base() + DMA40_CB, cb);
        hw.w32(Block::Dma, self.base() + DMA40_CS,
               DMA40_ACTIVE | DMA40_PROT | dma40_cs_flags(self.dreq));
    }

    /// The interrupt callback's acknowledge: INT cleared, the channel kept
    /// active for the next period. True if this channel raised it; an
    /// error is reported as `Err` with the CS and DEBUG registers.
    pub fn irq(&self, hw: &mut impl Hw) -> Result<bool, (u32, u32)> {
        let cs = hw.r32(Block::Dma, self.base() + DMA40_CS);
        if cs & DMA40_ERR != 0 {
            return Err((cs, hw.r32(Block::Dma, self.base() + DMA40_DEBUG)));
        }
        if cs & DMA40_INT == 0 {
            return Ok(false);
        }
        hw.w32(Block::Dma, self.base() + DMA40_CS,
               DMA40_INT | DMA40_ACTIVE | DMA40_PROT | dma40_cs_flags(self.dreq));
        Ok(true)
    }

    /// `bcm2835_dma_abort`: pause, let outstanding transactions finish,
    /// back to the default CS, reset. False if the pause did not complete.
    pub fn abort(&self, hw: &mut impl Hw) -> bool {
        let b = self.base();
        if hw.r32(Block::Dma, b + DMA40_CB) == 0 {
            return true;
        }
        let cs = hw.r32(Block::Dma, b + DMA40_CS);
        hw.w32(Block::Dma, b + DMA40_CS, cs & !DMA40_ACTIVE);
        let mut timeout = 100;
        while hw.r32(Block::Dma, b + DMA40_CS) & DMA40_TRANSACTIONS != 0 {
            timeout -= 1;
            if timeout == 0 {
                break;
            }
        }
        hw.w32(Block::Dma, b + DMA40_CS, DMA40_PROT);
        let dbg = hw.r32(Block::Dma, b + DMA40_DEBUG);
        hw.w32(Block::Dma, b + DMA40_DEBUG, dbg | DMA40_DEBUG_RESET);
        timeout != 0
    }

    /// `bcm2835_dma_tx_status` for the running cyclic transfer: the bytes
    /// from the source position to the end of the buffer
    /// (`bcm2835_dma_desc_size_pos`), so the read position is
    /// `buf_len - residue`.
    pub fn residue(&self, hw: &mut impl Hw, d: &Cyclic) -> usize {
        let lo = hw.r32(Block::Dma, self.base() + DMA40_SRC) as u64;
        let hi = (hw.r32(Block::Dma, self.base() + DMA40_SRCI) & 0xff) as u64;
        let pos = hi << 32 | lo;
        let mut size = 0usize;
        for i in 0..d.frames {
            let dma = d.buf + (i * d.period_len) as u64;
            let this = d.period_len.min(d.buf_len - i * d.period_len);
            if size != 0 {
                size += this;
            } else if pos >= dma && pos < dma + this as u64 {
                size += (dma + this as u64 - pos) as usize;
            }
        }
        size
    }
}
