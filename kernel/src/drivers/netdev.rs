//! Network Device Abstraction
//!
//! Dispatches to Intel NIC, WASM driver NIC, or virtio-net (in that order).

use crate::{virtio_net, intel_nic, rtl8153};
use crate::virtio_net::NetError;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MTU: usize = 1514;

// ── WASM-backed NIC (registered by WASM driver modules) ──

static WASM_NIC_ACTIVE: AtomicBool = AtomicBool::new(false);
static WASM_NIC: Mutex<WasmNic> = Mutex::new(WasmNic::empty());

// Frame ring between the kernel net stack and a WASM NIC driver. Unlike a
// single-slot mailbox (which overwrites — and so DROPS — an undrained frame on
// the next submit), this absorbs bursts: the producer drops only when the ring
// is genuinely full, never clobbering a frame already queued. One slot is kept
// empty to distinguish full from empty. All access is under the WASM_NIC lock,
// so plain indices suffice (no atomics needed).
struct Ring<const N: usize> {
    bufs: [[u8; MTU]; N],
    lens: [u16; N],
    head: usize, // consumer reads here
    tail: usize, // producer writes here
}

impl<const N: usize> Ring<N> {
    const fn new() -> Self {
        Ring { bufs: [[0; MTU]; N], lens: [0; N], head: 0, tail: 0 }
    }
    /// Enqueue a frame. Returns false (frame dropped) only if the ring is full.
    fn push(&mut self, frame: &[u8]) -> bool {
        let next = (self.tail + 1) % N;
        if next == self.head { return false; }
        let len = frame.len().min(MTU);
        self.bufs[self.tail][..len].copy_from_slice(&frame[..len]);
        self.lens[self.tail] = len as u16;
        self.tail = next;
        true
    }
    /// Dequeue the oldest frame into `out`. None if empty.
    fn pop(&mut self, out: &mut [u8; MTU]) -> Option<usize> {
        if self.head == self.tail { return None; }
        let len = self.lens[self.head] as usize;
        out[..len].copy_from_slice(&self.bufs[self.head][..len]);
        self.head = (self.head + 1) % N;
        Some(len)
    }
    fn clear(&mut self) { self.head = 0; self.tail = 0; }
}

// RX is a FALLBACK: the driver normally delivers each frame straight into the IP
// stack from its own fiber (net::wasm_deliver_rx, the NAPI topology) and only
// spills to this ring when Core 0 holds the drain guard.
const RX_RING: usize = 64;

struct WasmNic {
    mac_addr: [u8; 6],
    /// Frames the WASM driver received, waiting for the kernel to consume.
    rx: Ring<RX_RING>,
    /// Frames the kernel queued for the driver to transmit. fq_codel (Linux
    /// "Make WiFi Fast"): per-flow fair queueing + CoDel AQM so a ping isn't
    /// stuck behind a bulk backlog and stale packets are dropped, not delayed.
    tx: crate::net::fq_codel::FqCodel,
    /// Carrier/link state, set by the driver via npk_netdev_set_link. For a
    /// WiFi NIC this is "associated + keyed" (data path live), distinct from
    /// mere registration.
    link_up: bool,
}

impl WasmNic {
    const fn empty() -> Self {
        WasmNic {
            mac_addr: [0; 6],
            rx: Ring::new(),
            tx: crate::net::fq_codel::FqCodel::new(),
            link_up: false,
        }
    }
}

/// Called by WASM host function npk_netdev_register
pub fn register_wasm_nic(mac: [u8; 6]) {
    let mut nic = WASM_NIC.lock();
    nic.mac_addr = mac;
    nic.rx.clear();
    nic.tx.clear();
    nic.link_up = false;
    WASM_NIC_ACTIVE.store(true, Ordering::Release);
}

/// Called by cleanup_hw_state when WASM driver exits
pub fn unregister_wasm_nic() {
    WASM_NIC.lock().link_up = false;
    WASM_NIC_ACTIVE.store(false, Ordering::Release);
}

pub fn wasm_nic_available() -> bool {
    WASM_NIC_ACTIVE.load(Ordering::Acquire)
}

/// Driver reports its carrier/link state (associated + keyed) via
/// npk_netdev_set_link. Lets `net` show a real UP/DOWN for `wlan`.
pub fn set_wasm_nic_link(up: bool) {
    WASM_NIC.lock().link_up = up;
}

pub fn wasm_nic_link_up() -> bool {
    wasm_nic_available() && WASM_NIC.lock().link_up
}

// ── Wired link-state cache + active-NIC selection ─────────────────────────
// The wired NICs only report carrier live via an MMIO read (intel STATUS.LU) or
// a USB control transfer (rtl8153 PHY BMSR) — too costly for the dispatch path,
// which runs in IRQ context. refresh_link_state() (Core 0, ~1 Hz) reads them
// into this cache; dispatch + active() read the cache, staying cheap + IRQ-safe.
static INTEL_LINK: AtomicBool = AtomicBool::new(false);

/// Cached NIC preference: true = prefer WiFi (wlan), false = prefer wired (LAN).
/// Refreshed by refresh_link_state() (Core 0, ~1 Hz) from config `net_prefer`,
/// so active() stays cheap + IRQ-safe. Default = wired (the usual convention).
/// Whichever side is preferred wins ONLY when it has a usable link; otherwise we
/// fall back to the other interface if it has one.
static PREFER_WIFI: AtomicBool = AtomicBool::new(false);

/// Refresh the cached wired link state. Core 0 only (~1 Hz). ONLY the intel NIC
/// is polled live — a cheap, safe MMIO STATUS.LU read. The rtl8153 carrier is
/// NOT polled: reading it needs a USB control transfer, which takes the xHCI NIC
/// lock, and a timer IRQ landing mid-lock (poll_mouse takes the same lock)
/// deadlocks Core 0 (observed: networking died after ~20 ticks, instantly when
/// the USB NIC also carried traffic). A USB-LAN NIC's cable state is inferred
/// from presence + the WiFi link instead — see active().
pub fn refresh_link_state() {
    INTEL_LINK.store(intel_nic::link_up(), Ordering::Relaxed);
    PREFER_WIFI.store(
        crate::config::get("net_prefer")
            .map(|v| v.trim().eq_ignore_ascii_case("wifi"))
            .unwrap_or(false),
        Ordering::Relaxed,
    );
}

/// The active interface, honouring the `net_prefer` config (cached in
/// PREFER_WIFI). The preferred side (wired by default, or WiFi) wins ONLY when it
/// has a usable link; if it has none we fall back to the other interface if that
/// one does. "Usable link" = intel STATUS.LU live (read live), or a USB-LAN
/// (rtl8153) present (its carrier can't be safely probed, so presence is the best
/// signal), or WiFi associated + keyed. If nothing has a usable link we fall back
/// to mere presence.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Active { Intel, Rtl, Wasm, Virtio, None }

pub fn active() -> Active {
    let intel_up = intel_nic::is_available() && INTEL_LINK.load(Ordering::Relaxed);
    let rtl_present = rtl8153::is_available();
    let wifi_up = wasm_nic_link_up();

    if PREFER_WIFI.load(Ordering::Relaxed) {
        if wifi_up { return Active::Wasm; }
        if intel_up { return Active::Intel; }
        if rtl_present { return Active::Rtl; }
    } else {
        // Prefer wired — but only a PROVEN wired link (intel STATUS.LU) outranks
        // a proven WiFi link. A USB-LAN (rtl8153) has no carrier detect, so a
        // cable-less / merely-enumerated dongle must NOT strand a working WiFi
        // link (that would route DHCP out the dead NIC → no lease). So it ranks
        // BELOW associated WiFi.
        if intel_up { return Active::Intel; }
        if wifi_up { return Active::Wasm; }
        if rtl_present { return Active::Rtl; }
    }
    // Nothing with a usable link in the preferred order — fall back to presence.
    if intel_nic::is_available() { return Active::Intel; }
    if wasm_nic_available() { return Active::Wasm; }
    if virtio_net::is_available() { return Active::Virtio; }
    Active::None
}

/// Does the ACTIVE interface have a usable link right now? Distinct from
/// `active_id`, which only says which interface would be used: a WiFi NIC is
/// registered (and therefore "active" by fallback) from the moment the driver
/// starts, long before it is associated and keyed. Anything that waits for the
/// network to become usable has to look at this, not at the id.
pub fn active_link_up() -> bool {
    match active() {
        Active::Intel => INTEL_LINK.load(Ordering::Relaxed),
        Active::Wasm => wasm_nic_link_up(),
        // No carrier detect on either — presence is all we have.
        Active::Rtl | Active::Virtio => true,
        Active::None => false,
    }
}

/// A cheap numeric id of the active interface, for change detection.
pub fn active_id() -> u8 {
    match active() {
        Active::None => 0,
        Active::Intel => 1,
        Active::Rtl => 2,
        Active::Wasm => 3,
        Active::Virtio => 4,
    }
}

// ── WASM-NIC path counters (read by the `wlan` intent) ────────────────────
// The whole point is to tell WHERE frames are lost between driver and stack:
// a spill ring that overflows and an AQM that drops look identical from the
// outside (throughput just sags), so each side counts its own drops.
static RX_TO_RING: AtomicU64 = AtomicU64::new(0);   // frames put in the fallback ring
static RX_RING_DROP: AtomicU64 = AtomicU64::new(0); // …and lost because it was full
static TX_ENQUEUED: AtomicU64 = AtomicU64::new(0);  // frames handed to fq_codel
static TX_DEQUEUED: AtomicU64 = AtomicU64::new(0);  // …and picked up by the driver

pub struct WasmNicStats {
    pub rx_to_ring: u64,
    pub rx_ring_drops: u64,
    pub tx_enqueued: u64,
    pub tx_dequeued: u64,
    pub tx_drops_aqm: u64,
    pub tx_drops_full: u64,
    pub tx_backlog: usize,
}

pub fn wasm_nic_stats() -> WasmNicStats {
    let (aqm, full, backlog) = {
        let n = WASM_NIC.lock();
        let (a, f) = n.tx.drops_split();
        (a, f, n.tx.backlog())
    };
    WasmNicStats {
        rx_to_ring: RX_TO_RING.load(Ordering::Relaxed),
        rx_ring_drops: RX_RING_DROP.load(Ordering::Relaxed),
        tx_enqueued: TX_ENQUEUED.load(Ordering::Relaxed),
        tx_dequeued: TX_DEQUEUED.load(Ordering::Relaxed),
        tx_drops_aqm: aqm,
        tx_drops_full: full,
        tx_backlog: backlog,
    }
}

pub fn wasm_nic_stats_reset() {
    RX_TO_RING.store(0, Ordering::Relaxed);
    RX_RING_DROP.store(0, Ordering::Relaxed);
    TX_ENQUEUED.store(0, Ordering::Relaxed);
    TX_DEQUEUED.store(0, Ordering::Relaxed);
    WASM_NIC.lock().tx.reset_drops();
}

/// WASM driver calls this to submit a received frame to the kernel network stack
pub fn wasm_nic_submit_rx(frame: &[u8]) {
    if frame.len() > MTU { return; }
    RX_TO_RING.fetch_add(1, Ordering::Relaxed);
    if !WASM_NIC.lock().rx.push(frame) {
        RX_RING_DROP.fetch_add(1, Ordering::Relaxed);
    }
}

/// WASM driver calls this to get a frame to transmit (fq_codel-scheduled)
pub fn wasm_nic_poll_tx(buf: &mut [u8; MTU]) -> Option<usize> {
    let r = WASM_NIC.lock().tx.dequeue(buf);
    if r.is_some() { TX_DEQUEUED.fetch_add(1, Ordering::Relaxed); }
    r
}

/// Pop the oldest fallback-RX frame, if any. Used by net::wasm_deliver_rx to
/// flush frames that spilled to the ring (while Core 0 held the drain guard)
/// before the freshly-delivered one — preserving FIFO order.
pub fn wasm_nic_poll_rx(buf: &mut [u8; MTU]) -> Option<usize> {
    WASM_NIC.lock().rx.pop(buf)
}

pub fn send(frame: &[u8]) -> Result<(), NetError> {
    // Never hand a frame to an interface that has no link. active() falls back
    // to mere PRESENCE when nothing has a carrier, which is right for display
    // and wrong for the data path: on a machine whose only device is a WiFi card
    // that has not finished associating, every DHCP attempt went to the driver,
    // reached the firmware, and sat in its TX queue unsendable. in-flight never
    // returned to zero and the association never completed — while the same
    // machine with any wired device present worked, because the traffic went
    // there instead and left the radio alone.
    if !active_link_up() {
        return Err(NetError::NotInitialized);
    }
    match active() {
        Active::Wasm => {
            TX_ENQUEUED.fetch_add(1, Ordering::Relaxed);
            WASM_NIC.lock().tx.enqueue(frame);
            Ok(())
        }
        Active::Intel => intel_nic::send(frame),
        Active::Rtl => rtl8153::send(frame),
        Active::Virtio | Active::None => virtio_net::send(frame),
    }
}

pub fn recv(buf: &mut [u8; MTU]) -> Option<usize> {
    match active() {
        Active::Wasm => WASM_NIC.lock().rx.pop(buf),
        Active::Intel => intel_nic::recv(buf),
        Active::Rtl => rtl8153::recv(buf),
        Active::Virtio | Active::None => virtio_net::recv(buf),
    }
}

pub fn mac() -> Option<[u8; 6]> {
    match active() {
        Active::Wasm => Some(WASM_NIC.lock().mac_addr),
        Active::Intel => intel_nic::mac(),
        Active::Rtl => rtl8153::mac(),
        Active::Virtio => virtio_net::mac(),
        Active::None => None,
    }
}

pub fn is_available() -> bool {
    intel_nic::is_available() || rtl8153::is_available() || wasm_nic_available() || virtio_net::is_available()
}

// ── Interface enumeration ──

#[derive(Clone, Copy)]
pub struct IfaceInfo {
    pub name: &'static str,
    pub driver: &'static str,
    pub mac: [u8; 6],
    /// True for the interface that carries the global IP/Gateway/DNS config.
    pub primary: bool,
    /// Carrier/link state. Wired NICs are linked once present; the WiFi NIC is
    /// linked only once associated + keyed (driver reports via set_wasm_nic_link).
    pub link_up: bool,
}

/// List all active network interfaces. The first UP interface (Intel → WASM → virtio)
/// is marked primary and carries the global IPv4/Gateway/DNS config.
pub fn list() -> alloc::vec::Vec<IfaceInfo> {
    let mut v = alloc::vec::Vec::new();
    // `primary` is the interface the dispatch actually uses (active()), and
    // `link_up` is the cached REAL carrier — so a pulled cable shows DOWN and
    // the WiFi link takes over, matching what the stack does.
    let act = active();
    if intel_nic::is_available() {
        if let Some(mac) = intel_nic::mac() {
            v.push(IfaceInfo { name: "eth", driver: "Intel I226-V", mac, primary: act == Active::Intel, link_up: INTEL_LINK.load(Ordering::Relaxed) });
        }
    }
    if rtl8153::is_available() {
        if let Some(mac) = rtl8153::mac() {
            // No safe per-tick USB carrier read (see refresh_link_state); report
            // presence. `primary` reflects what the stack actually uses.
            v.push(IfaceInfo { name: "eth", driver: "Realtek RTL8153 (USB)", mac, primary: act == Active::Rtl, link_up: rtl8153::is_available() });
        }
    }
    if wasm_nic_available() {
        let mac = WASM_NIC.lock().mac_addr;
        v.push(IfaceInfo { name: "wlan", driver: "WiFi (WASM)", mac, primary: act == Active::Wasm, link_up: wasm_nic_link_up() });
    }
    if virtio_net::is_available() {
        if let Some(mac) = virtio_net::mac() {
            v.push(IfaceInfo { name: "eth", driver: "virtio-net", mac, primary: act == Active::Virtio, link_up: true });
        }
    }
    v
}
