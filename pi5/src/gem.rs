//! RP1's Ethernet MAC, a Cadence GEM_GXL (rev 0x00070109), after Linux
//! `drivers/net/ethernet/cadence/macb_main.c` and `macb_ptp.c` with
//! `raspberrypi_rp1_config`: one queue, 64-bit DMA addresses with the PTP
//! descriptor extension (24-byte descriptors), 512 descriptors per ring,
//! native register access, RGMII, jumbo frames, checksum offload
//! advertised, the FCS added by the driver (`macb_pad_and_fcs`).
//!
//! Descriptor rings and buffers live in memory the caller owns: [`Dma`]
//! reads and writes descriptor words and hands out and takes back
//! buffers by their bus address (RP1 sees CPU RAM at an offset).

use crate::hw::{self, Block, Hw};

/// `ethernet@100000` in BAR1.
pub const GEM_BASE: u64 = 0x10_0000;

const MACB_NCR: u64 = 0x000;
const MACB_NCFGR: u64 = 0x004;
const MACB_NSR: u64 = 0x008;
const GEM_USRIO: u64 = 0x00c;
const GEM_DMACFG: u64 = 0x010;
const MACB_TSR: u64 = 0x014;
const MACB_RBQP: u64 = 0x018;
const MACB_TBQP: u64 = 0x01c;
const MACB_RSR: u64 = 0x020;
const MACB_ISR: u64 = 0x024;
const MACB_IER: u64 = 0x028;
const MACB_IDR: u64 = 0x02c;
const MACB_MAN: u64 = 0x034;
const GEM_PBUFRXCUT: u64 = 0x044;
const GEM_JML: u64 = 0x048;
const GEM_AMP: u64 = 0x054;
const GEM_INTMOD: u64 = 0x05c;
const GEM_HRB: u64 = 0x080;
const GEM_HRT: u64 = 0x084;
const GEM_SA1B: u64 = 0x088;
const GEM_SA1T: u64 = 0x08c;
const GEM_RXPTPUNI: u64 = 0x0d4;
const GEM_TXPTPUNI: u64 = 0x0d8;
const MACB_MID: u64 = 0x0fc;
const GEM_TISUBN: u64 = 0x1bc;
const GEM_TSH: u64 = 0x1c0;
const GEM_TSL: u64 = 0x1d0;
const GEM_TN: u64 = 0x1d4;
const GEM_TA: u64 = 0x1d8;
const GEM_TI: u64 = 0x1dc;
const GEM_DCFG1: u64 = 0x280;
const GEM_DCFG2: u64 = 0x284;
const GEM_DCFG5: u64 = 0x290;
const GEM_DCFG6: u64 = 0x294;
const GEM_DCFG8: u64 = 0x29c;
const GEM_DCFG12: u64 = 0x2ac;
const MACB_TBQPH: u64 = 0x4c8;
const MACB_RBQPH: u64 = 0x4d4;
const GEM_ETHT: u64 = 0x6e0;

const fn bit(n: u32) -> u32 { 1 << n }

const MACB_LLB: u32 = bit(1);
const MACB_RE: u32 = bit(2);
const MACB_TE: u32 = bit(3);
const MACB_MPE: u32 = bit(4);
const MACB_CLRSTAT: u32 = bit(5);
const MACB_TSTART: u32 = bit(9);
const MACB_THALT: u32 = bit(10);
const GEM_TXLPIEN: u32 = bit(19);
const MACB_PTPUNI: u32 = bit(20);
const GEM_ENABLE_HS_MAC: u32 = bit(31);

const MACB_SPD: u32 = bit(0);
const MACB_FD: u32 = bit(1);
const MACB_JFRAME: u32 = bit(3);
const MACB_CAF: u32 = bit(4);
const MACB_NCFGR_MTI: u32 = bit(6);
const GEM_GBE: u32 = bit(10);
const GEM_PCSSEL: u32 = bit(11);
const MACB_PAE: u32 = bit(13);
const MACB_RBOF_SHIFT: u32 = 14;
const MACB_DRFCS: u32 = bit(17);
const GEM_CLK_SHIFT: u32 = 18;
const GEM_DBW_SHIFT: u32 = 21;
const GEM_RXCOEN: u32 = bit(24);
const GEM_SGMIIEN: u32 = bit(27);

const GEM_FBLDO_MASK: u32 = 0x1f;
const GEM_ENDIA_DESC: u32 = bit(6);
const GEM_ENDIA_PKT: u32 = bit(7);
const GEM_RXBMS_MASK: u32 = 0x3 << 8;
const GEM_TXPBMS: u32 = bit(10);
const GEM_TXCOEN: u32 = bit(11);
const GEM_RXBS_MASK: u32 = 0xff << 16;
const GEM_RXEXT: u32 = bit(28);
const GEM_TXEXT: u32 = bit(29);
const GEM_ADDR64: u32 = bit(30);

const GEM_AR2R_MAX_PIPE_MASK: u32 = 0xff;
const GEM_AW2W_MAX_PIPE_MASK: u32 = 0xff << 8;
const GEM_AW2B_FILL_MASK: u32 = 0x1 << 16;
const GEM_RX_MODERATION_MASK: u32 = 0xff;
const GEM_TX_MODERATION_MASK: u32 = 0xff << 16;

const MACB_IDLE: u32 = bit(2);
const MACB_TGO: u32 = bit(3);

pub const MACB_RCOMP: u32 = bit(1);
pub const MACB_RXUBR: u32 = bit(2);
pub const MACB_TXUBR: u32 = bit(3);
const MACB_ISR_TUND: u32 = bit(4);
const MACB_ISR_RLE: u32 = bit(5);
const MACB_TXERR: u32 = bit(6);
pub const MACB_TCOMP: u32 = bit(7);
const MACB_ISR_ROVR: u32 = bit(10);
const MACB_HRESP: u32 = bit(11);
const MACB_RX_INT_FLAGS: u32 = MACB_RCOMP | MACB_ISR_ROVR;
const MACB_TX_ERR_FLAGS: u32 = MACB_ISR_TUND | MACB_ISR_RLE | MACB_TXERR;
const MACB_TX_INT_FLAGS: u32 = MACB_TX_ERR_FLAGS | MACB_TCOMP | MACB_TXUBR;

const GEM_RGMII: u32 = bit(0);

const GEM_IRQCOR: u32 = bit(23);
const GEM_NO_PCS: u32 = bit(0);
const GEM_HIGH_SPEED: u32 = bit(26);
const GEM_RX_PKT_BUFF: u32 = bit(20);
const GEM_TX_PKT_BUFF: u32 = bit(21);
const GEM_TSU: u32 = bit(8);
const GEM_PBUF_LSO: u32 = bit(27);
const GEM_PBUF_CUTTHRU: u32 = bit(25);
const GEM_DAW64: u32 = bit(23);
const GEM_DBWDEF_MASK: u32 = 0x7 << 25;
const GEM_SCR2CMP_MASK: u32 = 0xff;
const GEM_SCR2ETH_MASK: u32 = 0xff << 8;
const GEM_T2SCR_MASK: u32 = 0xff << 16;
const MACB_IDNUM_MASK: u32 = 0xfff << 16;

const MACB_TX_USED: u32 = bit(31);
const MACB_TX_WRAP: u32 = bit(30);
const MACB_TX_NOCRC: u32 = bit(16);
const MACB_TX_LAST: u32 = bit(15);
const MACB_RX_USED: u32 = bit(0);
const MACB_RX_WRAP: u32 = bit(1);
const MACB_RX_SOF: u32 = bit(14);
const MACB_RX_EOF: u32 = bit(15);
const MACB_RX_JFRMLEN_MASK: u32 = 0x3fff;
const GEM_RX_CSUM_MASK: u32 = 0x3 << 22;
const GEM_RX_CSUM_CHECKED_MASK: u32 = 2;

/// `MACB_MAN_C22_*`.
const MAN_C22_SOF: u32 = 1 << 30;
const MAN_C22_READ: u32 = 2 << 28;
const MAN_C22_WRITE: u32 = 1 << 28;
const MAN_C22_CODE: u32 = 2 << 16;
const MACB_MDIO_TIMEOUT_US: u64 = 1_000_000;
const MACB_HALT_TIMEOUT_US: u64 = 14_000;

pub const RING_SIZE: usize = 512;
/// 64-bit address and PTP words: addr, ctrl, addrh, unused, ts1, ts2.
pub const DESC_WORDS: usize = 6;
pub const DESC_SIZE: usize = DESC_WORDS * 4;
/// One ring's memory (`macb_{tx,rx}_ring_size_per_queue`, no prefetch).
pub const RING_BYTES: usize = RING_SIZE * DESC_SIZE;
/// `GEM_MAX_TX_LEN`.
pub const MAX_TX_LEN: usize = 0x3fc0;
/// MTU 1500 + Ethernet header + FCS, rounded up to `RX_BUFFER_MULTIPLE`.
pub const RX_BUFFER_SIZE: usize = 0x600;
/// `raspberrypi_rp1_config.jumbo_max_len`.
const JUMBO_MAX_LEN: u32 = 10240;
const DMA_BURST_LENGTH: u32 = 16;
/// `cdns,aw2w-max-pipe`, `cdns,ar2r-max-pipe`, `cdns,use-aw2b-fill`.
const AW2W_MAX_PIPE: u32 = 8;
const AR2R_MAX_PIPE: u32 = 8;
const USE_AW2B_FILL: u32 = 1;

/// `gem_statistics`: each counter's ethtool name and register. The
/// registers clear on read; `GEM_OCTTXL` and `GEM_OCTRXL` have their bits
/// 47:32 in the next word.
pub const GEM_STATISTICS: [(&str, u64); GEM_STATS_LEN] = [
    ("tx_octets", 0x100),
    ("tx_frames", 0x108),
    ("tx_broadcast_frames", 0x10c),
    ("tx_multicast_frames", 0x110),
    ("tx_pause_frames", 0x114),
    ("tx_64_byte_frames", 0x118),
    ("tx_65_127_byte_frames", 0x11c),
    ("tx_128_255_byte_frames", 0x120),
    ("tx_256_511_byte_frames", 0x124),
    ("tx_512_1023_byte_frames", 0x128),
    ("tx_1024_1518_byte_frames", 0x12c),
    ("tx_greater_than_1518_byte_frames", 0x130),
    ("tx_underrun", 0x134),
    ("tx_single_collision_frames", 0x138),
    ("tx_multiple_collision_frames", 0x13c),
    ("tx_excessive_collisions", 0x140),
    ("tx_late_collisions", 0x144),
    ("tx_deferred_frames", 0x148),
    ("tx_carrier_sense_errors", 0x14c),
    ("rx_octets", 0x150),
    ("rx_frames", 0x158),
    ("rx_broadcast_frames", 0x15c),
    ("rx_multicast_frames", 0x160),
    ("rx_pause_frames", 0x164),
    ("rx_64_byte_frames", 0x168),
    ("rx_65_127_byte_frames", 0x16c),
    ("rx_128_255_byte_frames", 0x170),
    ("rx_256_511_byte_frames", 0x174),
    ("rx_512_1023_byte_frames", 0x178),
    ("rx_1024_1518_byte_frames", 0x17c),
    ("rx_greater_than_1518_byte_frames", 0x180),
    ("rx_undersized_frames", 0x184),
    ("rx_oversize_frames", 0x188),
    ("rx_jabbers", 0x18c),
    ("rx_frame_check_sequence_errors", 0x190),
    ("rx_length_field_frame_errors", 0x194),
    ("rx_symbol_errors", 0x198),
    ("rx_alignment_errors", 0x19c),
    ("rx_resource_errors", 0x1a0),
    ("rx_overruns", 0x1a4),
    ("rx_ip_header_checksum_errors", 0x1a8),
    ("rx_tcp_checksum_errors", 0x1ac),
    ("rx_udp_checksum_errors", 0x1b0),
    ("rx_lpi_transitions", 0x270),
    ("rx_lpi_time", 0x274),
    ("tx_lpi_transitions", 0x278),
    ("tx_lpi_time", 0x27c),
];
pub const GEM_STATS_LEN: usize = 47;
const GEM_OCTTXL: u64 = 0x100;
const GEM_OCTRXL: u64 = 0x150;

fn r(hw: &mut impl Hw, off: u64) -> u32 { hw.r32(Block::Rp1Bar1, GEM_BASE + off) }
fn w(hw: &mut impl Hw, off: u64, v: u32) { hw.w32(Block::Rp1Bar1, GEM_BASE + off, v) }

/// The descriptor memories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Tx,
    Rx,
    /// The 24-byte tie-off descriptor for unused RX queues.
    TieOff,
}

/// Bus addresses of the descriptor memories, as the GEM sees them. The TX
/// and RX rings share their upper 32 bits (one TBQPH/RBQPH for all queues).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rings {
    pub tx: u64,
    pub rx: u64,
    pub tieoff: u64,
}

/// Coherent descriptor memory and streaming buffers. `barrier` stands for
/// Linux's `dma_wmb` / `wmb` / `rmb` at the same places: descriptor
/// accesses on either side of it must not be reordered, nor with MMIO.
pub trait Dma {
    fn read(&mut self, ring: Ring, index: usize, word: usize) -> u32;
    fn write(&mut self, ring: Ring, index: usize, word: usize, v: u32);
    fn barrier(&mut self);
    /// A buffer of [`RX_BUFFER_SIZE`] bytes for RX slot `slot`
    /// (`netdev_alloc_skb` + `dma_map_single`); `None` ends the refill.
    fn rx_alloc(&mut self, slot: usize) -> Option<u64>;
    /// A frame of `len` bytes landed in `bus` (FCS stripped); the buffer is
    /// the caller's again. `csum_ok`: the GEM checked the IP/TCP/UDP sum.
    fn rx_deliver(&mut self, slot: usize, bus: u64, len: usize, csum_ok: bool);
    /// An RX buffer the GEM never filled, given back at close.
    fn rx_free(&mut self, slot: usize, bus: u64);
    /// A transmitted buffer is the caller's again (`macb_tx_unmap`).
    fn tx_done(&mut self, bus: u64, len: usize);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// MDIO did not go idle.
    MdioTimeout,
    /// No GEM at the MID register.
    NotGem(u32),
    /// The two rings' upper address halves differ.
    RingsSplit,
}

/// `hw_is_native_io`: the loopback bit reads back unswapped.
pub fn hw_is_native_io(hw: &mut impl Hw) -> bool {
    w(hw, MACB_NCR, MACB_LLB);
    let v = r(hw, MACB_NCR);
    w(hw, MACB_NCR, 0);
    v == MACB_LLB
}

/// `hw_is_gem`.
fn hw_is_gem(hw: &mut impl Hw) -> bool {
    hw::field_get(r(hw, MACB_MID), MACB_IDNUM_MASK) >= 0x2
}

/// `macb_mdio_wait_for_idle`.
fn mdio_wait_for_idle(hw: &mut impl Hw) -> Result<(), Error> {
    let (ok, _) = hw::poll(hw, Block::Rp1Bar1, GEM_BASE + MACB_NSR, 100, MACB_MDIO_TIMEOUT_US, |v| v & MACB_IDLE != 0);
    if ok { Ok(()) } else { Err(Error::MdioTimeout) }
}

/// `macb_mdio_read_c22`.
pub fn mdio_read(hw: &mut impl Hw, phy: u32, reg: u32) -> Result<u16, Error> {
    mdio_wait_for_idle(hw)?;
    w(hw, MACB_MAN, MAN_C22_SOF | MAN_C22_READ | (phy & 0x1f) << 23 | (reg & 0x1f) << 18 | MAN_C22_CODE);
    mdio_wait_for_idle(hw)?;
    Ok(r(hw, MACB_MAN) as u16)
}

/// `macb_mdio_write_c22`.
pub fn mdio_write(hw: &mut impl Hw, phy: u32, reg: u32, v: u16) -> Result<(), Error> {
    mdio_wait_for_idle(hw)?;
    w(hw, MACB_MAN, MAN_C22_SOF | MAN_C22_WRITE | (phy & 0x1f) << 23 | (reg & 0x1f) << 18 | MAN_C22_CODE | v as u32);
    mdio_wait_for_idle(hw)
}

/// `gem_mdc_clk_div`: MDC at most 2.5 MHz.
fn mdc_clk_div(pclk_hz: u64) -> u32 {
    const LIMITS: [u64; 7] = [20_000_000, 40_000_000, 80_000_000, 120_000_000, 160_000_000, 240_000_000, 320_000_000];
    let div = LIMITS.iter().position(|&l| pclk_hz <= l).unwrap_or(LIMITS.len()) as u32;
    div << GEM_CLK_SHIFT
}

/// `macb_dbw`.
fn dbw(hw: &mut impl Hw) -> u32 {
    match hw::field_get(r(hw, GEM_DCFG1), GEM_DBWDEF_MASK) {
        4 => 2 << GEM_DBW_SHIFT,
        2 => 1 << GEM_DBW_SHIFT,
        _ => 0,
    }
}

/// `crc32_le(~0, ...)`, inverted: the Ethernet FCS.
fn fcs(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// `macb_pad_and_fcs` for a linear frame without checksum offload: pad to
/// 60 bytes with zeros and append the FCS. `buf` holds the frame in its
/// first `len` bytes and has room for the result; the new length, or
/// `None` without room.
pub fn pad_and_fcs(buf: &mut [u8], len: usize) -> Option<usize> {
    let padded = len.max(60);
    if buf.len() < padded + 4 || len > buf.len() { return None; }
    buf[len..padded].fill(0);
    let f = fcs(&buf[..padded]);
    buf[padded..padded + 4].copy_from_slice(&f.to_le_bytes());
    Some(padded + 4)
}

/// `NETDEV_TX_BUSY`: the ring has no room for the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// What probe found and decided (`bp->caps`, `hw_dma_cap`, features).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub mid: u32,
    pub native_io: bool,
    pub num_queues: u32,
    /// `MACB_CAPS_ISR_CLEAR_ON_WRITE`: DCFG1.IRQCOR reads 0.
    pub isr_clear_on_write: bool,
    pub pcs: bool,
    pub high_speed: bool,
    pub fifo_mode: bool,
    pub ptp: bool,
    pub dma64: bool,
    pub lso: bool,
    /// `NETIF_F_HW_CSUM | NETIF_F_RXCSUM`, i.e. not FIFO mode.
    pub csum: bool,
    pub rx_watermark: u32,
    pub max_tuples: u32,
}

/// A transmit slot's buffer (`macb_tx_skb`); `last` holds the frame's skb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TxSlot {
    bus: u64,
    len: u32,
    last: bool,
}

/// What `macb_interrupt` scheduled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Irq {
    /// `IRQ_HANDLED`: ISR was not 0.
    pub handled: bool,
    /// Run [`Gem::rx_poll`] (`napi_schedule(&queue->napi_rx)`).
    pub rx: bool,
    /// Run [`Gem::tx_poll`].
    pub tx: bool,
    /// `tx_error_task` would run: TX underrun, retry limit or frame error.
    pub tx_error: bool,
    /// `hresp_err_bh_work` would run: DMA bus error.
    pub hresp: bool,
}

/// What the driver asks of the LPI timer (`tx_lpi_work`); the caller runs
/// [`Gem::tx_lpi_work`] when an armed timer expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LpiTimer {
    /// (Re)arm, due this many microseconds from now (`mod_delayed_work`).
    Arm(u64),
    Cancel,
}

/// `struct macb` with its one `macb_queue`.
pub struct Gem {
    pub caps: Caps,
    pub mac: [u8; 6],
    pclk_hz: u64,
    tsu_hz: u64,
    rings: Rings,
    tx_head: usize,
    tx_tail: usize,
    rx_tail: usize,
    rx_prepared_head: usize,
    tx_slot: [Option<TxSlot>; RING_SIZE],
    rx_buf: [Option<u64>; RING_SIZE],
    txubr_pending: bool,
    tx_pending: bool,
    running: bool,
    tx_stopped: bool,
    eee_active: bool,
    tx_lpi_timer_us: u64,
    lpi_req: Option<LpiTimer>,
    /// `hw_stats.gem`, in `GEM_STATISTICS` order.
    pub stats: [u64; GEM_STATS_LEN],
}

const fn circ_cnt(head: usize, tail: usize) -> usize { head.wrapping_sub(tail) & (RING_SIZE - 1) }
const fn circ_space(head: usize, tail: usize) -> usize { tail.wrapping_sub(head).wrapping_sub(1) & (RING_SIZE - 1) }
const fn wrap(i: usize) -> usize { i & (RING_SIZE - 1) }

/// The probe-time share of `macb_probe` from `hw_is_native_io` to the
/// DMA mask: queues, capabilities, features.
pub fn probe_caps(hw: &mut impl Hw) -> Result<Caps, Error> {
    let native_io = hw_is_native_io(hw);
    // macb_probe_queues
    let gem = hw_is_gem(hw);
    let mut queue_mask = 1;
    if gem {
        queue_mask |= r(hw, GEM_DCFG6) & 0xff;
    }
    let num_queues = queue_mask.count_ones();
    // max_tx_length
    let mid = r(hw, MACB_MID);
    if hw::field_get(mid, MACB_IDNUM_MASK) < 0x2 {
        return Err(Error::NotGem(mid));
    }
    let mut rx_watermark = 0;
    if r(hw, GEM_DCFG6) & GEM_PBUF_CUTTHRU != 0 {
        // No `cdns,rx-watermark` in this DT.
        rx_watermark = 0;
    }
    // macb_configure_caps
    let mut caps = Caps {
        mid, native_io, num_queues, isr_clear_on_write: false, pcs: false, high_speed: false,
        fifo_mode: false, ptp: false, dma64: false, lso: false, csum: false, rx_watermark, max_tuples: 0,
    };
    if hw_is_gem(hw) {
        let d1 = r(hw, GEM_DCFG1);
        caps.isr_clear_on_write = d1 & GEM_IRQCOR == 0;
        caps.pcs = d1 & GEM_NO_PCS == 0;
        caps.high_speed = r(hw, GEM_DCFG12) & GEM_HIGH_SPEED != 0;
        caps.fifo_mode = r(hw, GEM_DCFG2) & (GEM_RX_PKT_BUFF | GEM_TX_PKT_BUFF) == 0;
        caps.ptp = r(hw, GEM_DCFG5) & GEM_TSU != 0;
    }
    caps.dma64 = r(hw, GEM_DCFG6) & GEM_DAW64 != 0;
    Ok(caps)
}

impl Gem {
    /// `macb_get_hwaddr`: the first valid unicast address in SA1..SA4.
    pub fn get_hwaddr(hw: &mut impl Hw) -> Option<[u8; 6]> {
        for i in 0..4u64 {
            let bottom = r(hw, GEM_SA1B + i * 8);
            let top = r(hw, GEM_SA1T + i * 8) as u16;
            let b = bottom.to_le_bytes();
            let t = top.to_le_bytes();
            let a = [b[0], b[1], b[2], b[3], t[0], t[1]];
            if a[0] & 1 == 0 && a != [0; 6] {
                return Some(a);
            }
        }
        None
    }

    /// `macb_init` after the interrupt is requested: features, screeners,
    /// USRIO for RGMII, MDC divider and bus width.
    pub fn init(hw: &mut impl Hw, mut caps: Caps, mac: [u8; 6], pclk_hz: u64, tsu_hz: u64) -> Gem {
        caps.lso = r(hw, GEM_DCFG6) & GEM_PBUF_LSO != 0;
        caps.csum = !caps.fifo_mode;
        let reg = r(hw, GEM_DCFG8);
        caps.max_tuples = (hw::field_get(reg, GEM_SCR2CMP_MASK) / 3).min(hw::field_get(reg, GEM_T2SCR_MASK));
        if caps.max_tuples > 0 {
            if hw::field_get(reg, GEM_SCR2ETH_MASK) > 0 {
                w(hw, GEM_ETHT, 0x0800);
            } else {
                caps.max_tuples = 0;
            }
        }
        w(hw, GEM_USRIO, GEM_RGMII);
        let val = mdc_clk_div(pclk_hz) | dbw(hw);
        w(hw, MACB_NCFGR, val);
        Gem {
            caps, mac, pclk_hz, tsu_hz,
            rings: Rings { tx: 0, rx: 0, tieoff: 0 },
            tx_head: 0, tx_tail: 0, rx_tail: 0, rx_prepared_head: 0,
            tx_slot: [None; RING_SIZE], rx_buf: [None; RING_SIZE],
            txubr_pending: false, tx_pending: false, running: false, tx_stopped: false,
            eee_active: false, tx_lpi_timer_us: 0, lpi_req: None,
            stats: [0; GEM_STATS_LEN],
        }
    }

    /// `macb_mii_init`'s first access: the management port on.
    pub fn mdio_enable(hw: &mut impl Hw) {
        w(hw, MACB_NCR, MACB_MPE);
    }

    /// `macb_set_addr`: upper half first, then the word that hands the
    /// descriptor over.
    fn set_addr(dma: &mut impl Dma, ring: Ring, i: usize, addr: u64) {
        dma.write(ring, i, 2, (addr >> 32) as u32);
        dma.barrier();
        dma.write(ring, i, 0, addr as u32);
    }

    /// `gem_rx_refill`: one slot always stays free (`CIRC_SPACE`).
    fn rx_refill(&mut self, dma: &mut impl Dma) {
        while circ_space(self.rx_prepared_head, self.rx_tail) > 0 {
            let entry = wrap(self.rx_prepared_head);
            dma.barrier();
            if self.rx_buf[entry].is_none() {
                let Some(bus) = dma.rx_alloc(entry) else { break };
                self.rx_buf[entry] = Some(bus);
                let paddr = if entry == RING_SIZE - 1 { bus | MACB_RX_WRAP as u64 } else { bus };
                dma.write(Ring::Rx, entry, 1, 0);
                dma.barrier();
                Self::set_addr(dma, Ring::Rx, entry, paddr);
            } else {
                dma.write(Ring::Rx, entry, 1, 0);
                dma.barrier();
                let a = dma.read(Ring::Rx, entry, 0);
                dma.write(Ring::Rx, entry, 0, a & !MACB_RX_USED);
            }
            self.rx_prepared_head = self.rx_prepared_head.wrapping_add(1);
        }
        dma.barrier();
    }

    /// `macb_open` up to `macb_init_hw`: `gem_init_rings`,
    /// `macb_init_buffers`, `macb_init_hw`. `rings` is zeroed memory of
    /// [`RING_BYTES`], [`RING_BYTES`] and [`DESC_SIZE`] bytes.
    pub fn open(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, rings: Rings) -> Result<(), Error> {
        if (rings.tx >> 32) != ((rings.tx + RING_BYTES as u64 - 1) >> 32)
            || (rings.rx >> 32) != ((rings.rx + RING_BYTES as u64 - 1) >> 32)
        {
            return Err(Error::RingsSplit);
        }
        self.rings = rings;
        self.init_rings(dma);
        self.init_buffers(hw);
        self.running = true;
        self.init_hw(hw);
        Ok(())
    }

    /// `gem_init_rings`: every TX descriptor used, the last wrapping; the RX
    /// ring filled; the tie-off descriptor.
    fn init_rings(&mut self, dma: &mut impl Dma) {
        for i in 0..RING_SIZE {
            Self::set_addr(dma, Ring::Tx, i, 0);
            dma.write(Ring::Tx, i, 1, MACB_TX_USED);
        }
        dma.write(Ring::Tx, RING_SIZE - 1, 1, MACB_TX_USED | MACB_TX_WRAP);
        self.tx_head = 0;
        self.tx_tail = 0;
        self.rx_tail = 0;
        self.rx_prepared_head = 0;
        self.rx_refill(dma);
        Self::set_addr(dma, Ring::TieOff, 0, (MACB_RX_WRAP | MACB_RX_USED) as u64);
        dma.write(Ring::TieOff, 0, 1, 0);
    }

    /// `macb_init_buffers`.
    fn init_buffers(&mut self, hw: &mut impl Hw) {
        let rings = self.rings;
        if self.caps.dma64 {
            w(hw, MACB_RBQPH, (rings.rx >> 32) as u32);
            w(hw, MACB_TBQPH, (rings.tx >> 32) as u32);
        }
        w(hw, MACB_RBQP, rings.rx as u32);
        w(hw, MACB_TBQP, rings.tx as u32);
    }

    /// `macb_reset_hw`.
    fn reset_hw(&mut self, hw: &mut impl Hw) {
        let ctrl = (r(hw, MACB_NCR) & !(MACB_RE | MACB_TE)) | MACB_CLRSTAT;
        w(hw, MACB_NCR, ctrl);
        w(hw, MACB_TSR, !0);
        w(hw, MACB_RSR, !0);
        w(hw, GEM_PBUFRXCUT, 0);
        w(hw, MACB_IDR, !0);
        r(hw, MACB_ISR);
        if self.caps.isr_clear_on_write {
            w(hw, MACB_ISR, !0);
        }
    }

    /// `macb_set_hwaddr`.
    fn set_hwaddr(&mut self, hw: &mut impl Hw) {
        let m = self.mac;
        let bottom = u32::from_le_bytes([m[0], m[1], m[2], m[3]]);
        let top = u16::from_le_bytes([m[4], m[5]]) as u32;
        w(hw, GEM_SA1B, bottom);
        w(hw, GEM_SA1T, top);
        if self.caps.ptp {
            w(hw, GEM_RXPTPUNI, bottom);
            w(hw, GEM_TXPTPUNI, bottom);
        }
        for i in 1..4u64 {
            w(hw, GEM_SA1B + i * 8, 0);
            w(hw, GEM_SA1T + i * 8, 0);
        }
    }

    /// `macb_init_hw` (not promiscuous, broadcast on).
    fn init_hw(&mut self, hw: &mut impl Hw) {
        self.reset_hw(hw);
        self.set_hwaddr(hw);
        let mut config = mdc_clk_div(self.pclk_hz);
        config |= 0 << MACB_RBOF_SHIFT;
        config |= MACB_DRFCS | MACB_JFRAME;
        if self.caps.csum {
            config |= GEM_RXCOEN;
        }
        config |= dbw(hw);
        w(hw, MACB_NCFGR, config);
        w(hw, GEM_JML, JUMBO_MAX_LEN);
        // gem_init_axi
        let mut amp = r(hw, GEM_AMP);
        hw::replace_bits(&mut amp, USE_AW2B_FILL, GEM_AW2B_FILL_MASK);
        hw::replace_bits(&mut amp, AW2W_MAX_PIPE, GEM_AW2W_MAX_PIPE_MASK);
        hw::replace_bits(&mut amp, AR2R_MAX_PIPE, GEM_AR2R_MAX_PIPE_MASK);
        w(hw, GEM_AMP, amp);
        // gem_init_intmod: 50 us in units of 800 ns
        let throttle = 1000 * 50 / 800;
        let mut intmod = 0;
        hw::replace_bits(&mut intmod, throttle, GEM_TX_MODERATION_MASK);
        hw::replace_bits(&mut intmod, throttle, GEM_RX_MODERATION_MASK);
        w(hw, GEM_INTMOD, intmod);
        // macb_configure_dma
        let buffer_size = (RX_BUFFER_SIZE / 64) as u32;
        let mut dmacfg = r(hw, GEM_DMACFG) & !GEM_RXBS_MASK;
        dmacfg |= buffer_size << 16;
        hw::replace_bits(&mut dmacfg, DMA_BURST_LENGTH, GEM_FBLDO_MASK);
        dmacfg |= GEM_TXPBMS | GEM_RXBMS_MASK;
        dmacfg &= !GEM_ENDIA_PKT;
        if self.caps.native_io { dmacfg &= !GEM_ENDIA_DESC; } else { dmacfg |= GEM_ENDIA_DESC; }
        if self.caps.csum { dmacfg |= GEM_TXCOEN; } else { dmacfg &= !GEM_TXCOEN; }
        dmacfg &= !GEM_ADDR64;
        if self.caps.dma64 { dmacfg |= GEM_ADDR64; }
        if self.caps.ptp { dmacfg |= GEM_RXEXT | GEM_TXEXT; }
        w(hw, GEM_DMACFG, dmacfg);
        if self.caps.rx_watermark != 0 {
            w(hw, GEM_PBUFRXCUT, self.caps.rx_watermark | bit(31));
        }
    }

    /// `macb_set_rx_mode`. `multicast` are the addresses to hash when
    /// neither promiscuous nor all-multicast.
    pub fn set_rx_mode(&mut self, hw: &mut impl Hw, promisc: bool, allmulti: bool, multicast: &[[u8; 6]]) {
        let mut cfg = r(hw, MACB_NCFGR);
        if promisc {
            cfg |= MACB_CAF;
            cfg &= !GEM_RXCOEN;
        } else {
            cfg &= !MACB_CAF;
            if self.caps.csum { cfg |= GEM_RXCOEN; }
        }
        if allmulti {
            w(hw, GEM_HRB, !0);
            w(hw, GEM_HRT, !0);
            cfg |= MACB_NCFGR_MTI;
        } else if !multicast.is_empty() {
            let mut filter = [0u32; 2];
            for a in multicast {
                let mut idx = 0;
                for j in 0..6 {
                    let mut v = 0;
                    for i in 0..8 {
                        let b = i * 6 + j;
                        v ^= (a[b / 8] >> (b % 8)) & 1;
                    }
                    idx |= (v as usize) << j;
                }
                filter[idx >> 5] |= 1 << (idx & 31);
            }
            w(hw, GEM_HRB, filter[0]);
            w(hw, GEM_HRT, filter[1]);
            cfg |= MACB_NCFGR_MTI;
        } else {
            w(hw, GEM_HRB, 0);
            w(hw, GEM_HRT, 0);
            cfg &= !MACB_NCFGR_MTI;
        }
        w(hw, MACB_NCFGR, cfg);
    }

    /// `gem_ptp_init`'s register share, `gem_ptp_init_tsu`: the timer set to
    /// `now` (seconds, nanoseconds), its increment from the TSU clock, no
    /// adjustment.
    pub fn ptp_init(&mut self, hw: &mut impl Hw, now_s: u64, now_ns: u32) {
        let ns = (1_000_000_000 / self.tsu_hz) as u32;
        let rem = 1_000_000_000 % self.tsu_hz;
        let sub_ns = if rem != 0 { ((rem << 24) / self.tsu_hz) as u32 } else { 0 };
        w(hw, GEM_TN, 0);
        w(hw, GEM_TSH, ((now_s >> 32) & 0xffff) as u32);
        w(hw, GEM_TSL, now_s as u32);
        w(hw, GEM_TN, now_ns);
        w(hw, GEM_TISUBN, (sub_ns & 0xff) << 24 | ((sub_ns >> 8) & 0xffff));
        w(hw, GEM_TI, ns & 0xff);
        w(hw, GEM_TA, 0);
    }

    /// `gem_ptp_remove`'s register share, `gem_ptp_clear_timer`.
    fn ptp_remove(&mut self, hw: &mut impl Hw) {
        w(hw, GEM_TISUBN, 0);
        w(hw, GEM_TI, 0);
        w(hw, GEM_TA, 0);
    }

    /// `macb_mac_config` for RGMII: no PCS, no high-speed MAC.
    pub fn mac_config(&mut self, hw: &mut impl Hw) {
        let old_ctrl = r(hw, MACB_NCFGR);
        let old_ncr = r(hw, MACB_NCR);
        let ctrl = old_ctrl & !(GEM_SGMIIEN | GEM_PCSSEL);
        let ncr = old_ncr & !GEM_ENABLE_HS_MAC;
        if old_ctrl != ctrl { w(hw, MACB_NCFGR, ctrl); }
        if old_ncr != ncr { w(hw, MACB_NCR, ncr); }
    }

    /// `gem_shuffle_tx_one_ring`: rotate ring and slots so the tail is 0.
    fn shuffle_tx_ring(&mut self, dma: &mut impl Dma) {
        let (head, tail) = (self.tx_head, self.tx_tail);
        let count = circ_cnt(head, tail);
        if tail % RING_SIZE == 0 { return; }
        if count == 0 {
            self.tx_head = 0;
            self.tx_tail = 0;
            return;
        }
        let shift = tail % RING_SIZE;
        let cycles = gcd(RING_SIZE, shift);
        for i in 0..cycles {
            let saved = read_desc(dma, i);
            let saved_slot = self.tx_slot[i];
            let mut curr = i;
            let mut next = (curr + shift) % RING_SIZE;
            while next != i {
                let mut d = read_desc(dma, next);
                if next == RING_SIZE - 1 { d[1] &= !MACB_TX_WRAP; }
                if curr == RING_SIZE - 1 { d[1] |= MACB_TX_WRAP; }
                for (k, v) in d.iter().enumerate() { dma.write(Ring::Tx, curr, k, *v); }
                self.tx_slot[curr] = self.tx_slot[next];
                curr = next;
                next = (curr + shift) % RING_SIZE;
            }
            let mut d = saved;
            if i == RING_SIZE - 1 { d[1] &= !MACB_TX_WRAP; }
            if curr == RING_SIZE - 1 { d[1] |= MACB_TX_WRAP; }
            for (k, v) in d.iter().enumerate() { dma.write(Ring::Tx, curr, k, *v); }
            self.tx_slot[curr] = saved_slot;
        }
        self.tx_head = count;
        self.tx_tail = 0;
        dma.barrier();
    }

    /// `macb_mac_link_up`. The RP1's TX clock follows the speed by itself
    /// (`MACB_CAPS_CLK_HW_CHG`).
    pub fn link_up(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, speed: u32, full_duplex: bool, rx_pause: bool) {
        let mut ctrl = r(hw, MACB_NCFGR);
        ctrl &= !(MACB_SPD | MACB_FD);
        if speed == 100 { ctrl |= MACB_SPD; }
        if full_duplex { ctrl |= MACB_FD; }
        ctrl &= !MACB_PAE;
        ctrl &= !GEM_GBE;
        if speed == 1000 { ctrl |= GEM_GBE; }
        if rx_pause { ctrl |= MACB_PAE; }
        w(hw, MACB_IER, MACB_RX_INT_FLAGS | MACB_TX_INT_FLAGS | MACB_HRESP);
        w(hw, MACB_NCFGR, ctrl);
        self.shuffle_tx_ring(dma);
        let mut ncr = r(hw, MACB_NCR);
        if self.caps.ptp { ncr |= MACB_PTPUNI; }
        w(hw, MACB_NCR, ncr | MACB_RE | MACB_TE);
        self.tx_stopped = false;
    }

    /// `macb_mac_link_down`.
    pub fn link_down(&mut self, hw: &mut impl Hw) {
        w(hw, MACB_IDR, MACB_RX_INT_FLAGS | MACB_TX_INT_FLAGS | MACB_HRESP);
        let ctrl = r(hw, MACB_NCR) & !(MACB_RE | MACB_TE);
        w(hw, MACB_NCR, ctrl);
        self.tx_stopped = true;
    }

    /// `macb_tx_lpi_set`: true when NCR changed.
    fn tx_lpi_set(&mut self, hw: &mut impl Hw, enable: bool) -> bool {
        let old = r(hw, MACB_NCR);
        let ncr = if enable { old | GEM_TXLPIEN } else { old & !GEM_TXLPIEN };
        if old != ncr { w(hw, MACB_NCR, ncr); }
        old != ncr
    }

    /// `macb_mac_enable_tx_lpi`: LPI entry deferred by one second.
    pub fn enable_tx_lpi(&mut self, timer_us: u64) {
        self.tx_lpi_timer_us = timer_us;
        self.eee_active = true;
        self.lpi_req = Some(LpiTimer::Arm(1_000_000));
    }

    /// `macb_mac_disable_tx_lpi`.
    pub fn disable_tx_lpi(&mut self, hw: &mut impl Hw) {
        self.lpi_req = Some(LpiTimer::Cancel);
        self.eee_active = false;
        self.tx_lpi_set(hw, false);
    }

    /// The timer request the last calls left, if any.
    pub fn take_lpi_timer(&mut self) -> Option<LpiTimer> { self.lpi_req.take() }

    /// `macb_tx_lpi_work_fn`, when the armed timer expired.
    pub fn tx_lpi_work(&mut self, hw: &mut impl Hw) {
        if self.eee_active && self.tx_head == self.tx_tail {
            self.tx_lpi_set(hw, true);
        }
    }

    /// `macb_tx_lpi_wake`; the PHY needs 50 us to leave LPI.
    fn tx_lpi_wake(&mut self, hw: &mut impl Hw) {
        if !self.eee_active { return; }
        if !self.tx_lpi_set(hw, false) { return; }
        self.lpi_req = Some(LpiTimer::Cancel);
        hw.delay_us(50);
    }

    /// `macb_start_xmit` for a linear frame of `len` bytes at `bus`, padded
    /// and with its FCS ([`pad_and_fcs`]). `Ok(true)` when the queue is now
    /// full (`netif_stop_subqueue`).
    pub fn xmit(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, bus: u64, len: usize) -> Result<bool, Busy> {
        let desc_cnt = len.div_ceil(MAX_TX_LEN);
        if circ_space(self.tx_head, self.tx_tail) < desc_cnt {
            self.tx_stopped = true;
            return Err(Busy);
        }
        self.tx_map(dma, bus, len);
        dma.barrier();
        self.tx_lpi_wake(hw);
        if r(hw, MACB_TSR) & MACB_TGO != 0 {
            self.tx_pending = true;
        }
        let ncr = r(hw, MACB_NCR);
        w(hw, MACB_NCR, ncr | MACB_TSTART);
        r(hw, MACB_NCR);
        if circ_space(self.tx_head, self.tx_tail) < 1 {
            self.tx_stopped = true;
        }
        Ok(self.tx_stopped)
    }

    /// `macb_tx_map`: buffers of at most [`MAX_TX_LEN`], the end marker
    /// first, then the descriptors from last to first.
    fn tx_map(&mut self, dma: &mut impl Dma, bus: u64, len: usize) {
        let first = self.tx_head;
        let mut head = first;
        let (mut offset, mut left) = (0, len);
        while left > 0 {
            let size = left.min(MAX_TX_LEN);
            self.tx_slot[wrap(head)] = Some(TxSlot { bus: bus + offset as u64, len: size as u32, last: false });
            offset += size;
            left -= size;
            head = head.wrapping_add(1);
        }
        if let Some(s) = self.tx_slot[wrap(head.wrapping_sub(1))].as_mut() { s.last = true; }
        dma.write(Ring::Tx, wrap(head), 1, MACB_TX_USED);
        let mut i = head;
        let mut eof = true;
        loop {
            i = i.wrapping_sub(1);
            let entry = wrap(i);
            let slot = self.tx_slot[entry].unwrap_or(TxSlot { bus: 0, len: 0, last: false });
            let mut ctrl = slot.len;
            if eof { ctrl |= MACB_TX_LAST; eof = false; }
            if entry == RING_SIZE - 1 { ctrl |= MACB_TX_WRAP; }
            if i == first && self.caps.csum { ctrl |= MACB_TX_NOCRC; }
            Self::set_addr(dma, Ring::Tx, entry, slot.bus);
            dma.barrier();
            dma.write(Ring::Tx, entry, 1, ctrl);
            if i == first { break; }
        }
        self.tx_head = head;
    }

    /// `gem_update_stats`: the hardware counters added to `stats`.
    pub fn update_stats(&mut self, hw: &mut impl Hw) {
        for (i, &(_, off)) in GEM_STATISTICS.iter().enumerate() {
            let mut v = r(hw, off) as u64;
            if off == GEM_OCTTXL || off == GEM_OCTRXL {
                v += (r(hw, off + 4) as u64) << 32;
            }
            self.stats[i] += v;
        }
    }

    /// `macb_rx_pending`, for a caller that only counts.
    pub fn rx_waiting(&mut self, dma: &mut impl Dma) -> bool {
        self.rx_pending(dma)
    }

    /// `macb_interrupt`.
    pub fn interrupt(&mut self, hw: &mut impl Hw) -> Irq {
        let mut irq = Irq::default();
        let mut status = r(hw, MACB_ISR);
        if status == 0 { return irq; }
        irq.handled = true;
        let cow = self.caps.isr_clear_on_write;
        while status != 0 {
            if !self.running {
                w(hw, MACB_IDR, !0);
                if cow { w(hw, MACB_ISR, !0); }
                break;
            }
            if status & MACB_RX_INT_FLAGS != 0 {
                w(hw, MACB_IDR, MACB_RX_INT_FLAGS);
                if cow { w(hw, MACB_ISR, MACB_RCOMP); }
                irq.rx = true;
            }
            if status & (MACB_TCOMP | MACB_TXUBR) != 0 {
                w(hw, MACB_IDR, MACB_TCOMP);
                if cow { w(hw, MACB_ISR, MACB_TCOMP | MACB_TXUBR); }
                if status & MACB_TXUBR != 0 || self.tx_pending {
                    self.txubr_pending = true;
                    self.tx_pending = false;
                }
                irq.tx = true;
            }
            if status & MACB_TX_ERR_FLAGS != 0 {
                w(hw, MACB_IDR, MACB_TX_INT_FLAGS);
                irq.tx_error = true;
                if cow { w(hw, MACB_ISR, MACB_TX_ERR_FLAGS); }
                break;
            }
            if status & MACB_RXUBR != 0 {
                let ctrl = r(hw, MACB_NCR);
                w(hw, MACB_NCR, ctrl & !MACB_RE);
                w(hw, MACB_NCR, ctrl | MACB_RE);
                if cow { w(hw, MACB_ISR, MACB_RXUBR); }
            }
            if status & MACB_ISR_ROVR != 0 && cow {
                w(hw, MACB_ISR, MACB_ISR_ROVR);
            }
            if status & MACB_HRESP != 0 {
                irq.hresp = true;
                if cow { w(hw, MACB_ISR, MACB_HRESP); }
            }
            status = r(hw, MACB_ISR);
        }
        irq
    }

    /// `macb_tx_complete`: frames done, their buffers handed back.
    fn tx_complete(&mut self, dma: &mut impl Dma, budget: usize) -> usize {
        let head = self.tx_head;
        let mut tail = self.tx_tail;
        let mut packets = 0;
        while tail != head && packets < budget {
            dma.barrier();
            let ctrl = dma.read(Ring::Tx, wrap(tail), 1);
            if ctrl & MACB_TX_USED == 0 { break; }
            loop {
                let slot = self.tx_slot[wrap(tail)].take();
                let last = slot.is_some_and(|s| s.last);
                if last { packets += 1; }
                if let Some(s) = slot { dma.tx_done(s.bus, s.len as usize); }
                if last || wrap(tail) == wrap(head) { break; }
                tail = tail.wrapping_add(1);
            }
            tail = tail.wrapping_add(1);
        }
        self.tx_tail = tail;
        if self.tx_stopped && circ_cnt(self.tx_head, self.tx_tail) <= 3 * RING_SIZE / 4 {
            self.tx_stopped = false;
        }
        if packets > 0 && self.eee_active {
            self.lpi_req = Some(LpiTimer::Arm(self.tx_lpi_timer_us));
        }
        packets
    }

    /// `macb_tx_restart`.
    fn tx_restart(&mut self, hw: &mut impl Hw) {
        if self.tx_head == self.tx_tail { return; }
        let tbqp = r(hw, MACB_TBQP) as usize / DESC_SIZE;
        let tbqp = wrap(tbqp) * 3;
        let head_idx = wrap(self.tx_head) * 3;
        if tbqp == head_idx { return; }
        let ncr = r(hw, MACB_NCR);
        w(hw, MACB_NCR, ncr | MACB_TSTART);
        r(hw, MACB_NCR);
    }

    /// `macb_tx_complete_pending`.
    fn tx_complete_pending(&mut self, dma: &mut impl Dma) -> bool {
        if self.tx_head == self.tx_tail { return false; }
        dma.barrier();
        dma.read(Ring::Tx, wrap(self.tx_tail), 1) & MACB_TX_USED != 0
    }

    /// `macb_tx_poll`: (work done, run again). With `napi_complete_done`
    /// taken as succeeding: no poll runs concurrently here.
    pub fn tx_poll(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, budget: usize) -> (usize, bool) {
        let work = self.tx_complete(dma, budget);
        if self.txubr_pending {
            self.txubr_pending = false;
            self.tx_restart(hw);
        }
        let mut again = work >= budget;
        if work < budget {
            w(hw, MACB_IER, MACB_TCOMP);
            if self.tx_complete_pending(dma) {
                w(hw, MACB_IDR, MACB_TCOMP);
                if self.caps.isr_clear_on_write { w(hw, MACB_ISR, MACB_TCOMP); }
                again = true;
            }
        }
        (work, again)
    }

    /// `gem_rx`.
    fn rx(&mut self, dma: &mut impl Dma, budget: usize) -> usize {
        let mut count = 0;
        while count < budget {
            let entry = wrap(self.rx_tail);
            dma.barrier();
            let a = dma.read(Ring::Rx, entry, 0);
            if a & MACB_RX_USED == 0 { break; }
            dma.barrier();
            let ctrl = dma.read(Ring::Rx, entry, 1);
            self.rx_tail = self.rx_tail.wrapping_add(1);
            count += 1;
            if ctrl & MACB_RX_SOF == 0 || ctrl & MACB_RX_EOF == 0 { break; }
            let Some(bus) = self.rx_buf[entry].take() else { break };
            let len = (ctrl & MACB_RX_JFRMLEN_MASK) as usize;
            let csum_ok = self.caps.csum && hw::field_get(ctrl, GEM_RX_CSUM_MASK) & GEM_RX_CSUM_CHECKED_MASK != 0;
            dma.rx_deliver(entry, bus, len, csum_ok);
        }
        self.rx_refill(dma);
        count
    }

    /// `macb_rx_pending`.
    fn rx_pending(&mut self, dma: &mut impl Dma) -> bool {
        dma.barrier();
        dma.read(Ring::Rx, wrap(self.rx_tail), 0) & MACB_RX_USED != 0
    }

    /// `macb_rx_poll`: (work done, run again).
    pub fn rx_poll(&mut self, hw: &mut impl Hw, dma: &mut impl Dma, budget: usize) -> (usize, bool) {
        let work = self.rx(dma, budget);
        let mut again = work >= budget;
        if work < budget {
            w(hw, MACB_IER, MACB_RX_INT_FLAGS);
            if self.rx_pending(dma) {
                w(hw, MACB_IDR, MACB_RX_INT_FLAGS);
                if self.caps.isr_clear_on_write { w(hw, MACB_ISR, MACB_RCOMP); }
                again = true;
            }
        }
        (work, again)
    }

    /// `__dev_close` before `ndo_stop`: the interface no longer runs, the
    /// LPI timer is cancelled (`macb_close`'s first steps).
    pub fn begin_close(&mut self) {
        self.running = false;
        self.lpi_req = Some(LpiTimer::Cancel);
    }

    /// `macb_close` after phylink stopped: reset, buffers back
    /// (`gem_free_rx_buffers`; TX buffers still queued go back too), PTP off.
    pub fn close(&mut self, hw: &mut impl Hw, dma: &mut impl Dma) {
        self.reset_hw(hw);
        for slot in 0..RING_SIZE {
            if let Some(bus) = self.rx_buf[slot].take() { dma.rx_free(slot, bus); }
            if let Some(s) = self.tx_slot[slot].take() { dma.tx_done(s.bus, s.len as usize); }
        }
        if self.caps.ptp {
            self.ptp_remove(hw);
        }
    }

    /// `macb_halt_tx`: false when TGO did not clear in time.
    fn halt_tx(&mut self, hw: &mut impl Hw) -> bool {
        let ncr = r(hw, MACB_NCR);
        w(hw, MACB_NCR, ncr | MACB_THALT);
        hw::poll(hw, Block::Rp1Bar1, GEM_BASE + MACB_TSR, 250, MACB_HALT_TIMEOUT_US, |v| v & MACB_TGO == 0).0
    }

    /// `macb_tx_error_task`, after [`Irq::tx_error`] (with TX NAPI off):
    /// halt, hand back every queued buffer, restart the ring at 0.
    pub fn tx_error_task(&mut self, hw: &mut impl Hw, dma: &mut impl Dma) {
        self.tx_stopped = true;
        let halt_timeout = !self.halt_tx(hw);
        if halt_timeout {
            let ncr = r(hw, MACB_NCR);
            w(hw, MACB_NCR, ncr & !MACB_TE);
        }
        let mut tail = self.tx_tail;
        while tail != self.tx_head {
            let ctrl = dma.read(Ring::Tx, wrap(tail), 1);
            if ctrl & MACB_TX_USED != 0 {
                while !self.tx_slot[wrap(tail)].is_some_and(|s| s.last) && tail != self.tx_head {
                    if let Some(s) = self.tx_slot[wrap(tail)].take() { dma.tx_done(s.bus, s.len as usize); }
                    tail = tail.wrapping_add(1);
                }
            } else {
                dma.write(Ring::Tx, wrap(tail), 1, ctrl | MACB_TX_USED);
            }
            if let Some(s) = self.tx_slot[wrap(tail)].take() { dma.tx_done(s.bus, s.len as usize); }
            if tail == self.tx_head { break; }
            tail = tail.wrapping_add(1);
        }
        Self::set_addr(dma, Ring::Tx, 0, 0);
        dma.write(Ring::Tx, 0, 1, MACB_TX_USED);
        dma.barrier();
        w(hw, MACB_TBQP, self.rings.tx as u32);
        self.tx_head = 0;
        self.tx_tail = 0;
        let tsr = r(hw, MACB_TSR);
        w(hw, MACB_TSR, tsr);
        w(hw, MACB_IER, MACB_TX_INT_FLAGS);
        if halt_timeout {
            let ncr = r(hw, MACB_NCR);
            w(hw, MACB_NCR, ncr | MACB_TE);
        }
        self.tx_stopped = false;
        let ncr = r(hw, MACB_NCR);
        w(hw, MACB_NCR, ncr | MACB_TSTART);
    }

    /// `macb_hresp_error_task`, after [`Irq::hresp`]: both rings anew. The
    /// caller takes the carrier down and up around it. Queued TX buffers,
    /// which Linux leaves to the reinitialised ring, go back to the caller.
    pub fn hresp_error_task(&mut self, hw: &mut impl Hw, dma: &mut impl Dma) {
        let all = MACB_RX_INT_FLAGS | MACB_TX_INT_FLAGS | MACB_HRESP;
        w(hw, MACB_IDR, all);
        let ctrl = r(hw, MACB_NCR) & !(MACB_RE | MACB_TE);
        w(hw, MACB_NCR, ctrl);
        for slot in self.tx_slot.iter_mut() {
            if let Some(s) = slot.take() { dma.tx_done(s.bus, s.len as usize); }
        }
        self.init_rings(dma);
        self.init_buffers(hw);
        w(hw, MACB_IER, all);
        w(hw, MACB_NCR, ctrl | MACB_RE | MACB_TE);
        self.tx_stopped = false;
    }

    /// The queue cannot take a frame (`netif_queue_stopped`).
    pub fn tx_stopped(&self) -> bool { self.tx_stopped }
}

fn read_desc(dma: &mut impl Dma, i: usize) -> [u32; DESC_WORDS] {
    let mut d = [0u32; DESC_WORDS];
    for (k, v) in d.iter_mut().enumerate() { *v = dma.read(Ring::Tx, i, k); }
    d
}

const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 { (a, b) = (b, a % b); }
    a
}
