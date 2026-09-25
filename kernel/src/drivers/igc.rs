//! Intel I225/I226 (igc) data path — ported from Linux
//! `drivers/net/ethernet/intel/igc/igc_main.c`.
//!
//! Replaces the igc branch of `intel_nic`, which polled a 32-descriptor ring
//! with no interrupt: at 1 Gbit/s that is ~0.4 ms of traffic, so any gap in
//! the polling dropped frames on the card. Here: 256-descriptor rings
//! (`IGC_DEFAULT_RXD/TXD`), one MSI-X vector for queue pair 0 with hardware
//! auto-mask (`igc_configure_msix`), the NAPI fiber (`net::napi`) drains on
//! the interrupt and re-arms it when the ring is empty (`igc_ring_irq_enable`).
//!
//! Link and PHY stay as the firmware left them (no `CTRL.RST`), as before:
//! the ports below are the parts of `igc_configure`/`igc_up` that set up the
//! queues and the interrupt, not the PHY bring-up.

use core::sync::atomic::{AtomicU8, Ordering};
use spin::Mutex;
use crate::{kprintln, memory, pci};
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

/// MSI-X table entry for queue pair 0; entry 0 is "other causes" (link),
/// which we leave masked — link state is read from STATUS.
const QUEUE_MSIX_ENTRY: u16 = 1;

struct Igc {
    mmio: u64,
    rx_descs: u64,
    rx_bufs: u64,
    tx_descs: u64,
    tx_bufs: u64,
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
    /// EIMS bit of the queue vector; 0 = no interrupt, polled.
    eims_value: u32,
}

static DEVICE: Mutex<Option<Igc>> = Mutex::new(None);
static RX_VECTOR: AtomicU8 = AtomicU8::new(0);

fn rd32(base: u64, reg: u32) -> u32 {
    // SAFETY: BAR0 mapped by `intel_nic::init`, identity-mapped, uncached.
    unsafe { core::ptr::read_volatile((base + reg as u64) as *const u32) }
}

fn wr32(base: u64, reg: u32, val: u32) {
    // SAFETY: as `rd32`.
    unsafe { core::ptr::write_volatile((base + reg as u64) as *mut u32, val); }
}

fn wrfl(base: u64) { let _ = rd32(base, IGC_STATUS); }

fn rx_desc(d: &Igc, i: u16) -> u64 { d.rx_descs + i as u64 * 16 }
fn tx_desc(d: &Igc, i: u16) -> u64 { d.tx_descs + i as u64 * 16 }

/// Bring the queues up on a BAR0 `intel_nic` has mapped. Returns false if a
/// ring cannot be allocated.
pub fn init(dev: pci::PciAddr, mmio: u64) -> bool {
    // Interrupts off and acknowledged while the queues are rebuilt.
    wr32(mmio, IGC_IMC, 0xFFFF_FFFF);
    wr32(mmio, IGC_EIMC, 0xFFFF_FFFF);
    let _ = rd32(mmio, IGC_ICR);

    let Some(rx_descs) = alloc_zeroed(IGC_DEFAULT_RXD * 16) else { return false };
    let Some(rx_bufs) = alloc_zeroed(IGC_DEFAULT_RXD * BUF_SIZE) else { return false };
    let Some(tx_descs) = alloc_zeroed(IGC_DEFAULT_TXD * 16) else { return false };
    let Some(tx_bufs) = alloc_zeroed(IGC_DEFAULT_TXD * BUF_SIZE) else { return false };

    // igc_get_hw_control
    wr32(mmio, IGC_CTRL_EXT, rd32(mmio, IGC_CTRL_EXT) | IGC_CTRL_EXT_DRV_LOAD);

    // igc_setup_tctl
    wr32(mmio, IGC_TXDCTL0, 0);
    let mut tctl = rd32(mmio, IGC_TCTL);
    tctl &= !IGC_TCTL_CT;
    tctl |= IGC_TCTL_PSP | IGC_TCTL_RTLC | (IGC_COLLISION_THRESHOLD << IGC_CT_SHIFT);
    tctl |= IGC_TCTL_EN;
    wr32(mmio, IGC_TCTL, tctl);

    // igc_setup_rctl (mc_filter_type 0; RSS off — one queue)
    let mut rctl = rd32(mmio, IGC_RCTL);
    rctl &= !(3 << IGC_RCTL_MO_SHIFT);
    rctl &= !(IGC_RCTL_LBM_TCVR | IGC_RCTL_LBM_MAC);
    rctl |= IGC_RCTL_EN | IGC_RCTL_BAM | IGC_RCTL_RDMTS_HALF;
    rctl |= IGC_RCTL_SECRC;
    rctl &= !(IGC_RCTL_SBP | IGC_RCTL_SZ_256);
    rctl |= IGC_RCTL_LPE;
    wr32(mmio, IGC_RXDCTL0, 0);
    wr32(mmio, IGC_RCTL, rctl);

    // igc_configure_tx_ring
    wr32(mmio, IGC_TXDCTL0, 0);
    wrfl(mmio);
    wr32(mmio, IGC_TDLEN0, (IGC_DEFAULT_TXD * 16) as u32);
    wr32(mmio, IGC_TDBAL0, tx_descs as u32);
    wr32(mmio, IGC_TDBAH0, (tx_descs >> 32) as u32);
    wr32(mmio, IGC_TDH0, 0);
    wr32(mmio, IGC_TDT0, 0);
    wr32(mmio, IGC_TXDCTL0, 8 | (1 << 8) | (16 << 16) | IGC_TXDCTL_QUEUE_ENABLE);

    // igc_configure_rx_ring
    wr32(mmio, IGC_RXDCTL0, 0);
    wr32(mmio, IGC_RDBAL0, rx_descs as u32);
    wr32(mmio, IGC_RDBAH0, (rx_descs >> 32) as u32);
    wr32(mmio, IGC_RDLEN0, (IGC_DEFAULT_RXD * 16) as u32);
    wr32(mmio, IGC_RDH0, 0);
    wr32(mmio, IGC_RDT0, 0);
    let mut srrctl = rd32(mmio, IGC_SRRCTL0);
    srrctl &= !(IGC_SRRCTL_BSIZEPKT_MASK | IGC_SRRCTL_BSIZEHDR_MASK | IGC_SRRCTL_DESCTYPE_MASK);
    srrctl |= (IGC_RX_HDR_LEN / 64) << 8;
    srrctl |= IGC_RXBUFFER_2048 / 1024;
    srrctl |= IGC_SRRCTL_DESCTYPE_ADV_ONEBUF;
    wr32(mmio, IGC_SRRCTL0, srrctl);
    wr32(mmio, IGC_RXDCTL0, IGC_RXDCTL_PTHRESH | (IGC_RXDCTL_HTHRESH << 8)
        | (IGC_RXDCTL_WTHRESH << 16) | IGC_RXDCTL_QUEUE_ENABLE);

    let mut d = Igc {
        mmio, rx_descs, rx_bufs, tx_descs, tx_bufs,
        rx_next_to_clean: 0, rx_cleaned: 0, rx_discarding: false,
        tx_next_to_use: 0, tx_next_to_clean: 0, tx_eop: [0; IGC_DEFAULT_TXD],
        eims_value: 0,
    };

    // igc_alloc_rx_buffers(ring, igc_desc_unused) — all but one descriptor.
    for i in 0..IGC_DEFAULT_RXD as u16 {
        arm_rx_desc(&d, i);
    }
    wr32(mmio, IGC_RDT0, (IGC_DEFAULT_RXD - 1) as u32);

    // igc_configure_msix + igc_irq_enable, queue vector only.
    if let Some(vector) = crate::irq::register(dev, QUEUE_MSIX_ENTRY) {
        wr32(mmio, IGC_GPIE, IGC_GPIE_MSIX_MODE | IGC_GPIE_PBA | IGC_GPIE_EIAME | IGC_GPIE_NSICR);
        // igc_assign_vector: rx queue 0 at offset 0, tx queue 0 at offset 8.
        let msix = QUEUE_MSIX_ENTRY as u32;
        let mut ivar = rd32(mmio, IGC_IVAR0);
        ivar &= !0xFFFF;
        ivar |= (msix | IGC_IVAR_VALID) | ((msix | IGC_IVAR_VALID) << 8);
        wr32(mmio, IGC_IVAR0, ivar);
        // igc_write_itr
        wr32(mmio, igc_eitr(msix), (IGC_START_ITR & IGC_QVECTOR_MASK) | IGC_EITR_CNT_IGNR);
        d.eims_value = 1 << msix;
        wrfl(mmio);
        let _ = rd32(mmio, IGC_ICR);
        wr32(mmio, IGC_EIAC, rd32(mmio, IGC_EIAC) | d.eims_value);
        wr32(mmio, IGC_EIAM, rd32(mmio, IGC_EIAM) | d.eims_value);
        wr32(mmio, IGC_EIMS, d.eims_value);
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

fn alloc_zeroed(bytes: usize) -> Option<u64> {
    let pages = bytes.div_ceil(4096);
    let a = memory::allocate_contiguous(pages)?;
    // SAFETY: freshly allocated, identity-mapped, exclusively ours.
    unsafe { core::ptr::write_bytes(a as *mut u8, 0, pages * 4096); }
    Some(a)
}

/// Read format for descriptor `i`: buffer address, no header buffer. The
/// `hdr_addr` word overlaps the write-back `status_error`/`length`, so zeroing
/// it also clears the length the clean loop tests (Linux clears
/// `wb.upper.length` of the next descriptor for the same reason).
fn arm_rx_desc(d: &Igc, i: u16) {
    let desc = rx_desc(d, i);
    let buf = d.rx_bufs + i as u64 * BUF_SIZE as u64;
    // SAFETY: `desc` is inside the ring we allocated.
    unsafe {
        core::ptr::write_volatile(desc as *mut u64, buf);
        core::ptr::write_volatile((desc + 8) as *mut u64, 0);
    }
}

/// Hand the re-armed descriptors back: RDT one behind next_to_clean, so one
/// descriptor always stays unused (`igc_desc_unused`).
fn rx_refill(d: &mut Igc) {
    if d.rx_cleaned == 0 { return; }
    let n = IGC_DEFAULT_RXD as u16;
    let tail = (d.rx_next_to_clean + n - 1) % n;
    core::sync::atomic::fence(Ordering::Release); // wmb()
    wr32(d.mmio, IGC_RDT0, tail as u32);
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
        let desc = rx_desc(d, i);
        // SAFETY: descriptor inside our ring; the NIC writes it back by DMA.
        let (status, len) = unsafe {
            let upper = core::ptr::read_volatile((desc + 8) as *const u64);
            (upper as u32, ((upper >> 32) & 0xFFFF) as usize)
        };
        if len == 0 {
            rx_refill(d);
            if d.eims_value != 0 {
                wr32(d.mmio, IGC_EIMS, d.eims_value);
            }
            return None;
        }
        core::sync::atomic::fence(Ordering::Acquire); // dma_rmb()

        let eop = status & IGC_RXD_STAT_EOP != 0;
        let good = eop && !d.rx_discarding && status & IGC_RXDEXT_STATERR_RXE == 0;
        d.rx_discarding = !eop;
        let n = len.min(MTU);
        if good {
            let src = d.rx_bufs + i as u64 * BUF_SIZE as u64;
            // SAFETY: the NIC finished writing this buffer (length != 0 + rmb).
            unsafe { core::ptr::copy_nonoverlapping(src as *const u8, buf.as_mut_ptr(), n); }
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
        let eop = d.tx_eop[d.tx_next_to_clean as usize];
        let desc = tx_desc(d, eop);
        // SAFETY: descriptor inside our ring; `wb.status` is the olinfo word.
        let status = unsafe { core::ptr::read_volatile((desc + 12) as *const u32) };
        if status & IGC_TXD_STAT_DD == 0 { break; }
        d.tx_next_to_clean = (eop + 1) % n;
    }
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
    let buf = d.tx_bufs + i as u64 * BUF_SIZE as u64;
    let desc = tx_desc(d, i);
    // SAFETY: slot `i` was retired by `tx_clean`; buffer and descriptor are ours.
    unsafe {
        core::ptr::copy_nonoverlapping(chunk.as_ptr(), buf as *mut u8, chunk.len());
        core::ptr::write_volatile(desc as *mut u64, buf);
        core::ptr::write_volatile((desc + 8) as *mut u32, cmd | chunk.len() as u32);
        core::ptr::write_volatile((desc + 12) as *mut u32, olinfo);
    }
}

fn tx_bump_tail(d: &mut Igc, next: u16) {
    d.tx_next_to_use = next;
    core::sync::atomic::fence(Ordering::SeqCst); // wmb() before the tail
    wr32(d.mmio, IGC_TDT0, next as u32);
}

/// `igc_xmit_frame_ring` for a single-buffer frame.
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if frame.len() > MTU { return Err(NetError::FrameTooLarge); }
    let mut lock = DEVICE.lock();
    let d = lock.as_mut().ok_or(NetError::NotInitialized)?;
    tx_reserve(d, 1)?;
    let i = d.tx_next_to_use;
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
    let mut lock = DEVICE.lock();
    let d = lock.as_mut().ok_or(NetError::NotInitialized)?;
    let chunks = frame.len().div_ceil(BUF_SIZE);
    tx_reserve(d, 1 + chunks as u16)?;

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

    let n = IGC_DEFAULT_TXD as u16;
    let first = d.tx_next_to_use;
    // Context descriptor.
    let ctx = tx_desc(d, first);
    let ip_len = (l4_off - 14) as u32;
    // SAFETY: slot `first` is free (reserved above).
    unsafe {
        core::ptr::write_volatile(ctx as *mut u32, ip_len | (14 << IGC_ADVTXD_MACLEN_SHIFT));
        core::ptr::write_volatile((ctx + 4) as *mut u32, 0);
        core::ptr::write_volatile((ctx + 8) as *mut u32,
            IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DTYP_CTXT
            | IGC_ADVTXD_TUCMD_IPV4 | IGC_ADVTXD_TUCMD_L4T_TCP);
        core::ptr::write_volatile((ctx + 12) as *mut u32,
            (((hdr_len - l4_off) as u32) << IGC_ADVTXD_L4LEN_SHIFT)
            | ((mss as u32) << IGC_ADVTXD_MSS_SHIFT));
    }
    // Data descriptors; PAYLEN + checksum insertion on the first only.
    let cmd = IGC_ADVTXD_DTYP_DATA | IGC_ADVTXD_DCMD_DEXT | IGC_ADVTXD_DCMD_IFCS
        | IGC_ADVTXD_DCMD_TSE;
    let olinfo = ((frame.len() - hdr_len) as u32) << IGC_ADVTXD_PAYLEN_SHIFT
        | ((IGC_TXD_POPTS_TXSM | IGC_TXD_POPTS_IXSM) << 8);
    let mut i = (first + 1) % n;
    let mut chunk = [0u8; BUF_SIZE];
    for c in 0..chunks {
        let off = c * BUF_SIZE;
        let end = (off + BUF_SIZE).min(frame.len());
        let len = end - off;
        chunk[..len].copy_from_slice(&frame[off..end]);
        // The rewritten headers replace the head of the first chunk.
        if c == 0 { chunk[..hdr_len].copy_from_slice(&hdr[..hdr_len]); }
        let last = c == chunks - 1;
        let this_cmd = if last { cmd | IGC_ADVTXD_DCMD_EOP | IGC_ADVTXD_DCMD_RS } else { cmd };
        write_data_desc(d, i, &chunk[..len], this_cmd, if c == 0 { olinfo } else { 0 });
        if last { d.tx_eop[first as usize] = i; }
        i = (i + 1) % n;
    }
    tx_bump_tail(d, i);
    Ok(())
}

pub fn link_up() -> bool {
    match DEVICE.lock().as_ref() {
        Some(d) => rd32(d.mmio, IGC_STATUS) & IGC_STATUS_LU != 0,
        None => false,
    }
}


/// LAPIC vector of the RX queue interrupt, 0 when polled.
pub fn rx_irq_vector() -> u8 { RX_VECTOR.load(Ordering::Acquire) }
