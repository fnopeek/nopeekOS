//! Intel I225/I226 (igc) data path — ported from Linux
//! `drivers/net/ethernet/intel/igc/igc_main.c`.
//!
//! A polled 32-descriptor ring holds only ~0.4 ms of traffic at 1 Gbit/s, so
//! this uses 256-descriptor rings (`IGC_DEFAULT_RXD/TXD`), one MSI-X vector
//! for queue pair 0 with hardware auto-mask (`igc_configure_msix`), and the
//! NAPI fiber (`net::napi`) drains on the interrupt and re-arms it when the
//! ring is empty (`igc_ring_irq_enable`).
//!
//! Link and PHY stay as the firmware left them (no `CTRL.RST`): the ports
//! below are the parts of `igc_configure`/`igc_up` that set up the
//! queues and the interrupt, not the PHY bring-up.

use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;
use crate::{kprintln, pci};
use crate::hw::{DmaRegion, Mmio};
use crate::virtio_net::NetError;

pub const MTU: usize = 1514;

// ── Registers (igc_regs.h) ──
const IGC_STATUS: u32 = 0x0008;
const IGC_CTRL_EXT: u32 = 0x0018;
const IGC_ICR: u32 = 0x1500;
const IGC_IMC: u32 = 0x150C;
const IGC_GPIE: u32 = 0x1514;
const IGC_EIMS: u32 = 0x1524;
const IGC_EIMC: u32 = 0x1528;
const IGC_EIAC: u32 = 0x152C;
const IGC_EIAM: u32 = 0x1530;
const IGC_IVAR0: u32 = 0x1700;
const IGC_RCTL: u32 = 0x0100;
const IGC_TCTL: u32 = 0x0400;
const fn igc_eitr(n: u32) -> u32 { 0x1680 + 4 * n }
const IGC_SRRCTL0: u32 = 0xC00C;
const IGC_RDBAL0: u32 = 0xC000;
const IGC_RDBAH0: u32 = 0xC004;
const IGC_RDLEN0: u32 = 0xC008;
const IGC_RDH0: u32 = 0xC010;
const IGC_RDT0: u32 = 0xC018;
const IGC_RXDCTL0: u32 = 0xC028;
const IGC_TDBAL0: u32 = 0xE000;
const IGC_TDBAH0: u32 = 0xE004;
const IGC_TDLEN0: u32 = 0xE008;
const IGC_TDH0: u32 = 0xE010;
const IGC_TDT0: u32 = 0xE018;
const IGC_TXDCTL0: u32 = 0xE028;

// ── Bits (igc_defines.h / igc_base.h / igc.h) ──
const IGC_STATUS_LU: u32 = 1 << 1;
const IGC_CTRL_EXT_DRV_LOAD: u32 = 0x1000_0000;
const IGC_GPIE_NSICR: u32 = 0x0000_0001;
const IGC_GPIE_MSIX_MODE: u32 = 0x0000_0010;
const IGC_GPIE_EIAME: u32 = 0x4000_0000;
const IGC_GPIE_PBA: u32 = 0x8000_0000;
const IGC_IVAR_VALID: u32 = 0x80;
const IGC_START_ITR: u32 = 648; // ~6000 ints/s
const IGC_QVECTOR_MASK: u32 = 0x7FFC;
const IGC_EITR_CNT_IGNR: u32 = 0x8000_0000;
const IGC_RCTL_EN: u32 = 0x0000_0002;
const IGC_RCTL_SBP: u32 = 0x0000_0004;
const IGC_RCTL_LPE: u32 = 0x0000_0020;
const IGC_RCTL_LBM_MAC: u32 = 0x0000_0040;
const IGC_RCTL_LBM_TCVR: u32 = 0x0000_00C0;
const IGC_RCTL_RDMTS_HALF: u32 = 0;
const IGC_RCTL_MO_SHIFT: u32 = 12;
const IGC_RCTL_BAM: u32 = 0x0000_8000;
const IGC_RCTL_SZ_256: u32 = 0x0003_0000;
const IGC_RCTL_SECRC: u32 = 0x0400_0000;
const IGC_TCTL_EN: u32 = 0x0000_0002;
const IGC_TCTL_PSP: u32 = 0x0000_0008;
const IGC_TCTL_CT: u32 = 0x0000_0FF0;
const IGC_TCTL_RTLC: u32 = 0x0100_0000;
const IGC_COLLISION_THRESHOLD: u32 = 15;
const IGC_CT_SHIFT: u32 = 4;
const IGC_SRRCTL_BSIZEPKT_MASK: u32 = 0x7F;
const IGC_SRRCTL_BSIZEHDR_MASK: u32 = 0x3F << 8;
const IGC_SRRCTL_DESCTYPE_MASK: u32 = 0x7 << 25;
const IGC_SRRCTL_DESCTYPE_ADV_ONEBUF: u32 = 1 << 25;
const IGC_RX_HDR_LEN: u32 = 256;
const IGC_RXBUFFER_2048: u32 = 2048;
const IGC_RXDCTL_PTHRESH: u32 = 8;
const IGC_RXDCTL_HTHRESH: u32 = 8;
const IGC_RXDCTL_WTHRESH: u32 = 4;
const IGC_RXDCTL_QUEUE_ENABLE: u32 = 0x0200_0000;
const IGC_TXDCTL_QUEUE_ENABLE: u32 = 1 << 25;
const IGC_RXD_STAT_EOP: u32 = 0x02;
const IGC_RXDEXT_STATERR_RXE: u32 = 0x8000_0000;
const IGC_TXD_STAT_DD: u32 = 0x1;
const IGC_ADVTXD_DTYP_DATA: u32 = 0x0030_0000;
const IGC_ADVTXD_DCMD_EOP: u32 = 0x0100_0000;
const IGC_ADVTXD_DCMD_IFCS: u32 = 0x0200_0000;
const IGC_ADVTXD_DCMD_RS: u32 = 0x0800_0000;
const IGC_ADVTXD_DCMD_DEXT: u32 = 0x2000_0000;
const IGC_ADVTXD_PAYLEN_SHIFT: u32 = 14;
const IGC_ADVTXD_DTYP_CTXT: u32 = 0x0020_0000;
const IGC_ADVTXD_DCMD_TSE: u32 = 0x8000_0000;
const IGC_ADVTXD_TUCMD_IPV4: u32 = 0x0000_0400;
const IGC_ADVTXD_TUCMD_L4T_TCP: u32 = 0x0000_0800;
const IGC_ADVTXD_MACLEN_SHIFT: u32 = 9;
const IGC_ADVTXD_L4LEN_SHIFT: u32 = 8;
const IGC_ADVTXD_MSS_SHIFT: u32 = 16;
const IGC_TXD_POPTS_IXSM: u32 = 0x01;
const IGC_TXD_POPTS_TXSM: u32 = 0x02;

const IGC_DEFAULT_RXD: usize = 256;
const IGC_DEFAULT_TXD: usize = 256;
/// Return RX buffers in batches — "one at a time is too slow".
const IGC_RX_BUFFER_WRITE: u16 = 16;
const BUF_SIZE: usize = IGC_RXBUFFER_2048 as usize;
/// `IGC_MAX_DATA_PER_TXD`: one data descriptor carries up to 32 KiB.
const IGC_MAX_DATA_PER_TXD: usize = 1 << 15;
/// A TSO super-frame lies whole in one of these, so it takes one data
/// descriptor per 32 KiB (Linux maps the skb's fragments the same way)
/// instead of one per 2 KiB copy buffer.
const TSO_SLOT_SIZE: usize = 66 * 1024;
const TSO_SLOTS: usize = 32;
const NO_TSO_SLOT: u8 = 0xFF;

/// MSI-X table entry for queue pair 0; entry 0 is "other causes" (link),
/// which we leave masked — link state is read from STATUS.
const QUEUE_MSIX_ENTRY: u16 = 1;

struct Igc {
    mmio: Mmio,
    rx_descs: DmaRegion,
    rx_bufs: DmaRegion,
    tx_descs: DmaRegion,
    tx_bufs: DmaRegion,
    rx_next_to_clean: u16,
    /// Consumed and re-armed, not yet handed back through RDT.
    rx_cleaned: u16,
    /// Inside a frame that spans descriptors (LPE on, frame > 2 KiB): drop
    /// through its EOP descriptor rather than deliver the pieces as frames.
    rx_discarding: bool,
    tx_next_to_use: u16,
    tx_next_to_clean: u16,
    /// `next_to_watch`: for the first slot of each packet, the index of its
    /// EOP descriptor — the only one the NIC writes DD back to (RS is set on
    /// the last descriptor; a TSO context descriptor never gets one).
    tx_eop: [u16; IGC_DEFAULT_TXD],
    /// `TSO_SLOTS` super-frame buffers.
    tso_data: DmaRegion,
    /// Free TSO slots, one bit each.
    tso_free: u32,
    /// For the first slot of a TSO packet, the super-frame buffer it uses.
    tso_slot_of: [u8; IGC_DEFAULT_TXD],
    /// For the first slot of each packet, its length; `tx_inflight` sums the
    /// packets on the ring (BQL's `num_queued - num_completed`).
    tx_len: [u32; IGC_DEFAULT_TXD],
    tx_inflight: u32,
    /// EIMS bit of the queue vector; 0 = no interrupt, polled.
    eims_value: u32,
}

static DEVICE: Mutex<Option<Igc>> = Mutex::new(None);
static RX_VECTOR: AtomicU8 = AtomicU8::new(0);

const IGC_CTRL: u32 = 0x0000;
const IGC_CTRL_RFCE: u32 = 0x0800_0000;
const IGC_CTRL_TFCE: u32 = 0x1000_0000;
const IGC_FCAL: u32 = 0x0028;
const IGC_FCAH: u32 = 0x002C;
const IGC_FCT: u32 = 0x0030;
const IGC_FCTTV: u32 = 0x0170;
const IGC_FCRTL: u32 = 0x2160;
const IGC_FCRTH: u32 = 0x2168;
const IGC_FCRTL_XONE: u32 = 0x8000_0000;
const FLOW_CONTROL_ADDRESS_LOW: u32 = 0x00C2_8001;
const FLOW_CONTROL_ADDRESS_HIGH: u32 = 0x0000_0100;
const FLOW_CONTROL_TYPE: u32 = 0x8808;
const FC_PAUSE_TIME: u32 = 0xFFFF;
const IGC_PBA_34K: u32 = 0x22;
const MAX_JUMBO_FRAME_SIZE: u32 = 0x2600;
/// MTU + Ethernet header + FCS + VLAN tag (`igc_sw_init`).
const MAX_FRAME_SIZE: u32 = 1500 + 14 + 4 + 4;
const IGC_STATUS_SPEED_100: u32 = 0x0000_0040;
const IGC_STATUS_SPEED_1000: u32 = 0x0000_0080;
const IGC_STATUS_SPEED_2500: u32 = 0x0040_0000;
const IGC_MPC: u32 = 0x4010;
const IGC_XONRXC: u32 = 0x4048;
const IGC_XOFFRXC: u32 = 0x4050;

/// Statistics registers clear on read; these sum what was read.
static STAT_MPC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static STAT_XOFF_RX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static STAT_XON_RX: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// (missed RX frames, XOFF received, XON received) since boot.
pub fn link_stats() -> (u64, u64, u64) {
    if let Some(d) = DEVICE.lock().as_ref() {
        STAT_MPC.fetch_add(d.mmio.r32(IGC_MPC) as u64, Ordering::Relaxed);
        STAT_XOFF_RX.fetch_add(d.mmio.r32(IGC_XOFFRXC) as u64, Ordering::Relaxed);
        STAT_XON_RX.fetch_add(d.mmio.r32(IGC_XONRXC) as u64, Ordering::Relaxed);
    }
    (STAT_MPC.load(Ordering::Relaxed), STAT_XOFF_RX.load(Ordering::Relaxed),
     STAT_XON_RX.load(Ordering::Relaxed))
}

fn wrfl(mmio: &Mmio) { let _ = mmio.r32(IGC_STATUS); }

/// Offset of descriptor `i` in its ring.
fn desc_off(i: u16) -> u64 { i as u64 * 16 }

/// Bring the queues up on the BAR0 window `intel_nic` has mapped. Returns
/// false if a ring cannot be allocated.
pub fn init(dev: pci::PciAddr, mmio: Mmio) -> bool {
    // Interrupts off and acknowledged while the queues are rebuilt.
    mmio.w32(IGC_IMC, 0xFFFF_FFFF);
    mmio.w32(IGC_EIMC, 0xFFFF_FFFF);
    let _ = mmio.r32(IGC_ICR);

    let Some(rx_descs) = alloc_zeroed(IGC_DEFAULT_RXD * 16) else { return false };
    let Some(rx_bufs) = alloc_zeroed(IGC_DEFAULT_RXD * BUF_SIZE) else { return false };
    let Some(tx_descs) = alloc_zeroed(IGC_DEFAULT_TXD * 16) else { return false };
    let Some(tx_bufs) = alloc_zeroed(IGC_DEFAULT_TXD * BUF_SIZE) else { return false };
    // Without the super-frame buffers the card still runs: every TSO frame
    // is then refused and the caller segments in software.
    let (tso_data, tso_free) = match alloc_zeroed(TSO_SLOTS * TSO_SLOT_SIZE) {
        Some(r) => (r, ((1u64 << TSO_SLOTS) - 1) as u32),
        None => {
            crate::kprintln!("[npk] igc: no memory for TSO buffers, segmenting in software");
            let Some(r) = alloc_zeroed(4096) else { return false };
            (r, 0)
        }
    };

    // igc_get_hw_control
    mmio.w32(IGC_CTRL_EXT, mmio.r32(IGC_CTRL_EXT) | IGC_CTRL_EXT_DRV_LOAD);

    // igc_setup_link + igc_set_fc_watermarks + igc_force_mac_fc, requested
    // mode full (the default): honour the switch's PAUSE frames and send our
    // own before the receive FIFO overflows.
    mmio.w32(IGC_FCT, FLOW_CONTROL_TYPE);
    mmio.w32(IGC_FCAH, FLOW_CONTROL_ADDRESS_HIGH);
    mmio.w32(IGC_FCAL, FLOW_CONTROL_ADDRESS_LOW);
    mmio.w32(IGC_FCTTV, FC_PAUSE_TIME);
    let high_water = ((IGC_PBA_34K << 10) - (MAX_FRAME_SIZE + MAX_JUMBO_FRAME_SIZE)) & 0xFFFF_FFF0;
    mmio.w32(IGC_FCRTL, (high_water - 16) | IGC_FCRTL_XONE);
    mmio.w32(IGC_FCRTH, high_water);
    mmio.w32(IGC_CTRL, mmio.r32(IGC_CTRL) | IGC_CTRL_RFCE | IGC_CTRL_TFCE);
    let status = mmio.r32(IGC_STATUS);
    let speed = if status & IGC_STATUS_SPEED_2500 != 0 { 2500 }
        else if status & IGC_STATUS_SPEED_1000 != 0 { 1000 }
        else if status & IGC_STATUS_SPEED_100 != 0 { 100 } else { 10 };
    kprintln!("[npk] igc: link {} Mbit/s, flow control rx+tx", speed);

    // igc_setup_tctl
    mmio.w32(IGC_TXDCTL0, 0);
    let mut tctl = mmio.r32(IGC_TCTL);
    tctl &= !IGC_TCTL_CT;
    tctl |= IGC_TCTL_PSP | IGC_TCTL_RTLC | (IGC_COLLISION_THRESHOLD << IGC_CT_SHIFT);
    tctl |= IGC_TCTL_EN;
    mmio.w32(IGC_TCTL, tctl);

    // igc_setup_rctl (mc_filter_type 0; RSS off — one queue)
    let mut rctl = mmio.r32(IGC_RCTL);
    rctl &= !(3 << IGC_RCTL_MO_SHIFT);
    rctl &= !(IGC_RCTL_LBM_TCVR | IGC_RCTL_LBM_MAC);
    rctl |= IGC_RCTL_EN | IGC_RCTL_BAM | IGC_RCTL_RDMTS_HALF;
    rctl |= IGC_RCTL_SECRC;
    rctl &= !(IGC_RCTL_SBP | IGC_RCTL_SZ_256);
    rctl |= IGC_RCTL_LPE;
    mmio.w32(IGC_RXDCTL0, 0);
    mmio.w32(IGC_RCTL, rctl);

    // igc_configure_tx_ring
    mmio.w32(IGC_TXDCTL0, 0);
    wrfl(&mmio);
    mmio.w32(IGC_TDLEN0, (IGC_DEFAULT_TXD * 16) as u32);
    mmio.w32(IGC_TDBAL0, tx_descs.phys() as u32);
    mmio.w32(IGC_TDBAH0, (tx_descs.phys() >> 32) as u32);
    mmio.w32(IGC_TDH0, 0);
    mmio.w32(IGC_TDT0, 0);
    mmio.w32(IGC_TXDCTL0, 8 | (1 << 8) | (16 << 16) | IGC_TXDCTL_QUEUE_ENABLE);

    // igc_configure_rx_ring
    mmio.w32(IGC_RXDCTL0, 0);
    mmio.w32(IGC_RDBAL0, rx_descs.phys() as u32);
    mmio.w32(IGC_RDBAH0, (rx_descs.phys() >> 32) as u32);
    mmio.w32(IGC_RDLEN0, (IGC_DEFAULT_RXD * 16) as u32);
    mmio.w32(IGC_RDH0, 0);
    mmio.w32(IGC_RDT0, 0);
    let mut srrctl = mmio.r32(IGC_SRRCTL0);
    srrctl &= !(IGC_SRRCTL_BSIZEPKT_MASK | IGC_SRRCTL_BSIZEHDR_MASK | IGC_SRRCTL_DESCTYPE_MASK);
    srrctl |= (IGC_RX_HDR_LEN / 64) << 8;
    srrctl |= IGC_RXBUFFER_2048 / 1024;
    srrctl |= IGC_SRRCTL_DESCTYPE_ADV_ONEBUF;
    mmio.w32(IGC_SRRCTL0, srrctl);
    mmio.w32(IGC_RXDCTL0, IGC_RXDCTL_PTHRESH | (IGC_RXDCTL_HTHRESH << 8)
        | (IGC_RXDCTL_WTHRESH << 16) | IGC_RXDCTL_QUEUE_ENABLE);

    let mut d = Igc {
        mmio, rx_descs, rx_bufs, tx_descs, tx_bufs,
        rx_next_to_clean: 0, rx_cleaned: 0, rx_discarding: false,
        tx_next_to_use: 0, tx_next_to_clean: 0, tx_eop: [0; IGC_DEFAULT_TXD],
        tso_data, tso_free,
        tso_slot_of: [NO_TSO_SLOT; IGC_DEFAULT_TXD],
        tx_len: [0; IGC_DEFAULT_TXD], tx_inflight: 0,
        eims_value: 0,
    };

    // igc_alloc_rx_buffers(ring, igc_desc_unused) — all but one descriptor.
    for i in 0..IGC_DEFAULT_RXD as u16 {
        arm_rx_desc(&d, i);
    }
    mmio.w32(IGC_RDT0, (IGC_DEFAULT_RXD - 1) as u32);

    // igc_configure_msix + igc_irq_enable, queue vector only.
    if let Some(vector) = crate::irq::register(dev, QUEUE_MSIX_ENTRY) {
        mmio.w32(IGC_GPIE, IGC_GPIE_MSIX_MODE | IGC_GPIE_PBA | IGC_GPIE_EIAME | IGC_GPIE_NSICR);
        // igc_assign_vector: rx queue 0 at offset 0, tx queue 0 at offset 8.
        let msix = QUEUE_MSIX_ENTRY as u32;
        let mut ivar = mmio.r32(IGC_IVAR0);
        ivar &= !0xFFFF;
        ivar |= (msix | IGC_IVAR_VALID) | ((msix | IGC_IVAR_VALID) << 8);
        mmio.w32(IGC_IVAR0, ivar);
        // igc_write_itr
        mmio.w32(igc_eitr(msix), (IGC_START_ITR & IGC_QVECTOR_MASK) | IGC_EITR_CNT_IGNR);
        d.eims_value = 1 << msix;
        wrfl(&mmio);
        let _ = mmio.r32(IGC_ICR);
        mmio.w32(IGC_EIAC, mmio.r32(IGC_EIAC) | d.eims_value);
        mmio.w32(IGC_EIAM, mmio.r32(IGC_EIAM) | d.eims_value);
        mmio.w32(IGC_EIMS, d.eims_value);
        RX_VECTOR.store(vector, Ordering::Release);
        kprintln!("[npk] igc: {} RX/{} TX descriptors, MSI-X entry {} on vector {:#x}",
            IGC_DEFAULT_RXD, IGC_DEFAULT_TXD, QUEUE_MSIX_ENTRY, vector);
    } else {
        kprintln!("[npk] igc: {} RX/{} TX descriptors, no MSI-X — polled",
            IGC_DEFAULT_RXD, IGC_DEFAULT_TXD);
    }

    *DEVICE.lock() = Some(d);
    true
}

fn alloc_zeroed(bytes: usize) -> Option<DmaRegion> {
    DmaRegion::alloc_zeroed(bytes.div_ceil(4096))
}

/// Read format for descriptor `i`: buffer address, no header buffer. The
/// `hdr_addr` word overlaps the write-back `status_error`/`length`, so zeroing
/// it also clears the length the clean loop tests (Linux clears
/// `wb.upper.length` of the next descriptor for the same reason).
fn arm_rx_desc(d: &Igc, i: u16) {
    let desc = desc_off(i);
    let buf = d.rx_bufs.phys() + i as u64 * BUF_SIZE as u64;
    d.rx_descs.w64(desc, buf);
    d.rx_descs.w64(desc + 8, 0);
}

/// Hand the re-armed descriptors back: RDT one behind next_to_clean, so one
/// descriptor always stays unused (`igc_desc_unused`).
fn rx_refill(d: &mut Igc) {
    if d.rx_cleaned == 0 { return; }
    let n = IGC_DEFAULT_RXD as u16;
    let tail = (d.rx_next_to_clean + n - 1) % n;
    core::sync::atomic::fence(Ordering::Release); // wmb()
    d.mmio.w32(IGC_RDT0, tail as u32);
    d.rx_cleaned = 0;
}

/// One frame of `igc_clean_rx_irq`. At the empty ring it returns the batch
/// and re-enables the queue interrupt (`napi_complete_done` +
/// `igc_ring_irq_enable`); a frame that lands meanwhile is left pending in
/// EICR and fires when EIMS unmasks it.
pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    let mut lock = DEVICE.lock();
    let d = lock.as_mut()?;
    loop {
        if d.rx_cleaned >= IGC_RX_BUFFER_WRITE { rx_refill(d); }
        let i = d.rx_next_to_clean;
        let upper = d.rx_descs.r64(desc_off(i) + 8);
        let (status, len) = (upper as u32, ((upper >> 32) & 0xFFFF) as usize);
        if len == 0 {
            rx_refill(d);
            if d.eims_value != 0 {
                d.mmio.w32(IGC_EIMS, d.eims_value);
            }
            return None;
        }
        core::sync::atomic::fence(Ordering::Acquire); // dma_rmb()

        let eop = status & IGC_RXD_STAT_EOP != 0;
        let good = eop && !d.rx_discarding && status & IGC_RXDEXT_STATERR_RXE == 0;
        d.rx_discarding = !eop;
        let n = len.min(MTU);
        if good {
            // The NIC finished writing this buffer (length != 0 + rmb).
            d.rx_bufs.copy_out(i as u64 * BUF_SIZE as u64, &mut buf[..n]);
        }
        arm_rx_desc(d, i);
        d.rx_next_to_clean = (i + 1) % IGC_DEFAULT_RXD as u16;
        d.rx_cleaned += 1;
        // A frame spanning buffers or flagged bad is dropped.
        if good { return Some(n); }
    }
}

/// `igc_clean_tx_irq`: retire whole packets whose EOP descriptor is done.
fn tx_clean(d: &mut Igc) {
    let n = IGC_DEFAULT_TXD as u16;
    while d.tx_next_to_clean != d.tx_next_to_use {
        let first = d.tx_next_to_clean as usize;
        let eop = d.tx_eop[first];
        // `wb.status` is the olinfo word.
        let status = d.tx_descs.r32(desc_off(eop) + 12);
        if status & IGC_TXD_STAT_DD == 0 { break; }
        d.tx_inflight -= core::mem::replace(&mut d.tx_len[first], 0);
        let slot = core::mem::replace(&mut d.tso_slot_of[first], NO_TSO_SLOT);
        if slot != NO_TSO_SLOT {
            d.tso_free |= 1 << slot;
        }
        d.tx_next_to_clean = (eop + 1) % n;
    }
}

/// Bytes the non-waiting senders may keep on the ring; 0 = the ring's size.
/// `set microvm_txq_kb`, read at each microVM launch.
static TX_LIMIT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn set_tx_limit(bytes: u32) { TX_LIMIT.store(bytes, Ordering::Relaxed); }

/// A packet of `len` would pass the limit. The first one always goes.
fn over_limit(d: &Igc, len: usize) -> bool {
    let lim = TX_LIMIT.load(Ordering::Relaxed);
    lim != 0 && d.tx_inflight != 0 && d.tx_inflight as usize + len > lim as usize
}

fn note_queued(d: &mut Igc, first: u16, len: usize) {
    d.tx_len[first as usize] = len as u32;
    d.tx_inflight += len as u32;
}

/// Descriptors free for a new packet (one always stays unused).
fn tx_unused(d: &Igc) -> u16 {
    let n = IGC_DEFAULT_TXD as u16;
    (d.tx_next_to_clean + n - d.tx_next_to_use - 1) % n
}

/// Wait until `need` descriptors are free: at most ~200 µs for completions,
/// then refuse (`NETDEV_TX_BUSY`) instead of spinning the caller for seconds.
fn tx_reserve(d: &mut Igc, need: u16) -> Result<(), NetError> {
    tx_clean(d);
    if tx_unused(d) >= need { return Ok(()); }
    let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() / 5000;
    loop {
        tx_clean(d);
        if tx_unused(d) >= need { return Ok(()); }
        if crate::interrupts::rdtsc() >= deadline { return Err(NetError::QueueFull); }
        core::hint::spin_loop();
    }
}

fn write_data_desc(d: &Igc, i: u16, chunk: &[u8], cmd: u32, olinfo: u32) {
    // Slot `i` was retired by `tx_clean`.
    let buf_off = i as u64 * BUF_SIZE as u64;
    let desc = desc_off(i);
    d.tx_bufs.copy_in(buf_off, chunk);
    d.tx_descs.w64(desc, d.tx_bufs.phys() + buf_off);
    d.tx_descs.w32(desc + 8, cmd | chunk.len() as u32);
    d.tx_descs.w32(desc + 12, olinfo);
}

fn tx_bump_tail(d: &mut Igc, next: u16) {
    d.tx_next_to_use = next;
    core::sync::atomic::fence(Ordering::SeqCst); // wmb() before the tail
    d.mmio.w32(IGC_TDT0, next as u32);
}

/// `igc_xmit_frame_ring` for a single-buffer frame.
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    send_inner(frame, true)
}

/// `send` without waiting for completions: a full ring is `QueueFull` at
/// once (`NETDEV_TX_BUSY`), for a caller that keeps the frame and retries
/// (`nat`'s egress backlog). Waiting would hold `DEVICE`, which the receive
/// path needs for the ACKs that free the ring.
pub fn try_send(frame: &[u8]) -> Result<(), NetError> {
    send_inner(frame, false)
}

fn send_inner(frame: &[u8], wait: bool) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }
    let mut lock = DEVICE.lock();
    let d = lock.as_mut().ok_or(NetError::NotInitialized)?;
    if wait {
        tx_reserve(d, 1)?;
    } else {
        tx_clean(d);
        if tx_unused(d) < 1 || over_limit(d, frame.len()) { return Err(NetError::QueueFull); }
    }
    let i = d.tx_next_to_use;
    note_queued(d, i, frame.len());
    write_data_desc(d, i, frame,
        IGC_ADVTXD_DTYP_DATA | IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DCMD_IFCS
        | IGC_ADVTXD_DCMD_EOP | IGC_ADVTXD_DCMD_RS,
        (frame.len() as u32) << IGC_ADVTXD_PAYLEN_SHIFT);
    d.tx_eop[i as usize] = i;
    tx_bump_tail(d, (i + 1) % IGC_DEFAULT_TXD as u16);
    Ok(())
}

/// TCPv4 segmentation offload — `igc_tso` + `igc_tx_ctxtdesc` + `igc_tx_map`.
/// `frame` is a whole Ethernet frame (IPv4, no options, TCP) whose TCP check
/// holds the Linux `CHECKSUM_PARTIAL` seed (pseudo-header incl. length).
pub fn send_tso(frame: &[u8], mss: u16, l4_off: usize, hdr_len: usize) -> Result<(), NetError> {
    if frame.len() > TSO_SLOT_SIZE { return Err(NetError::FrameTooLarge); }
    let mut lock = DEVICE.lock();
    let d = lock.as_mut().ok_or(NetError::NotInitialized)?;
    // `igc_maybe_stop_tx` without the wait: a full ring is the caller's to
    // handle, and it segments in software.
    let pieces = frame.len().div_ceil(IGC_MAX_DATA_PER_TXD);
    tx_clean(d);
    if tx_unused(d) < 1 + pieces as u16 || d.tso_free == 0 || over_limit(d, frame.len()) {
        return Err(NetError::QueueFull);
    }

    // igc_tso: IP tot_len 0 (HW fills per segment), IP check 0 (IXSM), and
    // the payload length taken back out of the TCP seed.
    let mut hdr = [0u8; 128];
    if hdr_len > hdr.len() || l4_off < 34 { return Err(NetError::FrameTooLarge); }
    hdr[..hdr_len].copy_from_slice(&frame[..hdr_len]);
    hdr[16] = 0; hdr[17] = 0;
    hdr[24] = 0; hdr[25] = 0;
    let paylen = (frame.len() - l4_off) as u32;
    let seed = u16::from_be_bytes([hdr[l4_off + 16], hdr[l4_off + 17]]);
    let mut sum = (!seed) as u32 + paylen;
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    hdr[l4_off + 16..l4_off + 18].copy_from_slice(&(!(sum as u16)).to_be_bytes());

    // The whole frame into a free super-frame buffer, its headers rewritten.
    let slot = d.tso_free.trailing_zeros() as usize;
    d.tso_free &= !(1u32 << slot);
    let slot_off = (slot * TSO_SLOT_SIZE) as u64;
    d.tso_data.copy_in(slot_off, frame);
    d.tso_data.copy_in(slot_off, &hdr[..hdr_len]);

    let n = IGC_DEFAULT_TXD as u16;
    let first = d.tx_next_to_use;
    // Context descriptor.
    let ctx = desc_off(first);
    let ip_len = (l4_off - 14) as u32;
    let txd = &d.tx_descs;
    txd.w32(ctx, ip_len | (14 << IGC_ADVTXD_MACLEN_SHIFT));
    txd.w32(ctx + 4, 0);
    txd.w32(ctx + 8,
        IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DTYP_CTXT
        | IGC_ADVTXD_TUCMD_IPV4 | IGC_ADVTXD_TUCMD_L4T_TCP);
    txd.w32(ctx + 12,
        (((hdr_len - l4_off) as u32) << IGC_ADVTXD_L4LEN_SHIFT)
        | ((mss as u32) << IGC_ADVTXD_MSS_SHIFT));
    // `igc_tx_map`: data descriptors of at most IGC_MAX_DATA_PER_TXD; PAYLEN
    // and checksum insertion on the first only, EOP + RS on the last.
    let cmd = IGC_ADVTXD_DTYP_DATA | IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DCMD_IFCS
        | IGC_ADVTXD_DCMD_TSE;
    let olinfo = ((frame.len() - hdr_len) as u32) << IGC_ADVTXD_PAYLEN_SHIFT
        | ((IGC_TXD_POPTS_TXSM | IGC_TXD_POPTS_IXSM) << 8);
    let mut i = (first + 1) % n;
    for c in 0..pieces {
        let off = c * IGC_MAX_DATA_PER_TXD;
        let len = (frame.len() - off).min(IGC_MAX_DATA_PER_TXD);
        let last = c == pieces - 1;
        let this_cmd = if last { cmd | IGC_ADVTXD_DCMD_EOP | IGC_ADVTXD_DCMD_RS } else { cmd };
        let desc = desc_off(i);
        d.tx_descs.w64(desc, d.tso_data.phys() + slot_off + off as u64);
        d.tx_descs.w32(desc + 8, this_cmd | len as u32);
        d.tx_descs.w32(desc + 12, if c == 0 { olinfo } else { 0 });
        if last { d.tx_eop[first as usize] = i; }
        i = (i + 1) % n;
    }
    d.tso_slot_of[first as usize] = slot as u8;
    note_queued(d, first, frame.len());
    tx_bump_tail(d, i);
    Ok(())
}

pub fn link_up() -> bool {
    match DEVICE.lock().as_ref() {
        Some(d) => d.mmio.r32(IGC_STATUS) & IGC_STATUS_LU != 0,
        None => false,
    }
}


/// LAPIC vector of the RX queue interrupt, 0 when polled.
pub fn rx_irq_vector() -> u8 { RX_VECTOR.load(Ordering::Acquire) }
