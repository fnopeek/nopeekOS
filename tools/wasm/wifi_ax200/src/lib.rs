//! wifi_ax200 — Intel Wi-Fi 6 AX200 driver (WASM module)
//!
//! iwlwifi-mvm, device family 22000 (gen2). Strict 1:1 port of Linux 6.18.26.
//! Plan: docs/archive/WIFI_AX200.md. Uses the nopeekOS WASM Driver ABI (npk_pci_*,
//! npk_mmio_*, npk_dma_*) — the same ABI proven by the RTL8852BE `wifi` driver.
//!
//! Stage 0a: bind PCI, map BAR0, read HW_REV + HW_RF_ID to confirm the chip.
//! Stage 0b (this file): reset + APM bring-up to MAC-clock-ready, following
//! `_iwl_trans_pcie_start_hw` for family 22000 (non-integrated AX200):
//!   prepare_card_hw → clear_persistence_bit → sw_reset → apm_init(→activate_nic).
//! All register pokes are 1:1 with the Linux source (no guessed values).

#![no_std]

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()]
    = *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

mod ba;
mod host;
mod regs;
use regs::*;

/// A panic used to spin here in silence: `loop {}` with the argument thrown
/// away. The module then hangs with every counter frozen at the instant it
/// died, the report stops being republished, and from the outside it is
/// indistinguishable from "the AP went quiet" — which cost an entire evening
/// of chasing the radio while the driver was standing still. Say where it
/// happened; `Location` survives `strip = true` because it is static data
/// referenced by the panic site, not a symbol.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Two channels on purpose. `print` reaches the terminal this driver was
    // launched from and is worker-core safe, so it lands where someone is
    // looking right now. `log` goes through kprintln to the serial capture,
    // which is the ONLY thing `dmesg` reads — without it the line dies with
    // the scrollback and cannot be recovered after a reboot. Print first: if
    // the kprintln path stalls (it routes through shade), the visible half has
    // already happened, and we are on our way to `loop {}` regardless.
    host::print("\n[ax200] PANIC — driver stopped");
    if let Some(l) = info.location() {
        host::print(" at ");
        host::print(l.file());
        host::print(":");
        host::print_dec(l.line());
    }
    host::print("\n");
    host::log("[ax200] PANIC — driver stopped (see the line above for file:line)");
    loop {}
}

/// AX200 runtime firmware (unified ucode), embedded like the RTL driver embeds
/// rtw8852b_fw.bin. API 77 = the exact version Linux 6.18.26 requests.
static FW: &[u8] = include_bytes!("../firmware/iwlwifi-cc-a0-77.ucode");

/// One source for the version string: the boot banner and every status snapshot
/// carry it, so a device measurement can never be traced to the wrong build.
// From Cargo.toml, never by hand: this string stood at 0.60.1 for three
// releases while the module was 0.63.0, and a report that misstates its own
// version makes every number in it suspect.
const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// Little-endian readers over the embedded firmware.
fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
// Little-endian writers into the context-info buffer.
fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
/// u32_encode_bits: place `v` into the field described by `mask`.
fn encode_bits(v: u32, mask: u32) -> u32 {
    (v << mask.trailing_zeros()) & mask
}

/// Write transfer block `i` (iwl_tfh_tb { __le16 tb_len; __le64 addr }) into a
/// TFD. tbs[] start at offset 2 (after num_tbs); addr is stored unaligned.
fn put_tfh_tb(tfd: &mut [u8], i: usize, len: u16, addr: u64) {
    let o = 2 + i * TFH_TB_LEN;
    tfd[o..o + 2].copy_from_slice(&len.to_le_bytes());
    tfd[o + 2..o + 10].copy_from_slice(&addr.to_le_bytes());
}

/// A coherent DMA allocation: kernel-held physical address + WASM handle.
#[derive(Clone, Copy)]
struct Dma {
    handle: i32,
    phys: u64,
}
impl Dma {
    const NONE: Dma = Dma { handle: -1, phys: 0 };
    fn ok(&self) -> bool { self.handle >= 0 }
}

/// What the firmware told us about a frame's place in its block-ack window.
/// Read from the descriptor while it is already in hand — a second DMA read per
/// received frame would cost more than the reordering saves.
#[derive(Clone, Copy)]
struct Agg {
    reorder: u32,
    status: u32,
    /// Not an A-MSDU, or the last sub-frame of one. NSSN advances on the FIRST
    /// sub-frame, so acting on it earlier releases frames still in flight.
    amsdu_last: bool,
    /// Unicast QoS data — the only thing a block-ack session covers.
    /// `iwl_mvm_reorder` bypasses everything else, and the frames it lets past
    /// are exactly the ones a client without an address depends on: a DHCP offer
    /// comes back to the broadcast address, and the window must never hold it.
    reorderable: bool,
}

/// An ADDBA request we have asked the firmware about and not yet answered. The
/// AP's dialog token, parameter set and timeout have to survive until the
/// firmware replies, because its answer decides what we put in ours.
#[derive(Clone, Copy)]
struct BaPending {
    tid: u8,
    ssn: u16,
    win: u16,
    dialog: u8,
    timeout: u16,
    /// When we asked the firmware. The AP gets its answer only after the
    /// firmware hands back a BAID — so if that never comes, the AP is left
    /// waiting for a reply that never arrives, and at least one AP answers
    /// that by sending NO DATA AT ALL on the TID: association up, DHCP
    /// never completes. A pending request must therefore expire.
    asked_ms: u64,
}

/// Classification of a received 802.11 data frame addressed to us.
enum RxKind {
    None,
    /// A data frame addressed to us whose payload we could not locate. Distinct
    /// from None on purpose: "nothing arrives" and "everything arrives and we
    /// drop it" are opposite faults and were indistinguishable before.
    Undecoded,
    /// EAPOL-Key frame (the 4-way) → forward to wifid. `out` holds the frame.
    Eapol(usize),
    /// IP/other data → the kernel IP stack. `out` holds an Ethernet frame.
    Ip(usize, Agg),
}

/// The AP's 802.11n capabilities, read from the HT Capability element of its
/// beacon. Everything we do at HT level derives from these: what we may put in
/// our own assoc request, the station flags, and the MCS set TLC scales over
/// (rs_fw_set_supp_rates uses the PEER's rx_mask — what the AP can receive).
#[derive(Clone, Copy)]
struct HtCap {
    cap_info: u16,
    ampdu_factor: u8,  // A-MPDU length exponent (0-3)
    ampdu_density: u8, // minimum MPDU start spacing (0-7)
    mcs_rx: [u8; 2],   // rx_mask[0] = MCS 0-7, rx_mask[1] = MCS 8-15
    /// From the HT OPERATION element, not the capability one: where the
    /// secondary 20 MHz channel sits. NONE means the AP runs 20 MHz only, and
    /// then a 40 MHz PHY context would be pointing at nothing.
    sec_chan_offs: u8,
    /// VHT capability element present, and its two fields we act on.
    vht: bool,
    vht_cap_info: u32,
    vht_rx_mcs_map: u16,
    /// From the VHT OPERATION element: USE_HT (fall back to the HT width) or
    /// 80MHZ. As with HT, the capability says CAN, the operation says DOES.
    vht_chan_width: u8,
    /// Centre-frequency channel index of the 80 MHz block.
    vht_seg0: u8,
    /// The HE (802.11ax) elements. For an AP that carries HE Capability AND an
    /// HE Operation with the VHT-Operation-Info bit, those three bytes ARE the
    /// operating width — `ieee80211_determine_ap_chan` reads them and never
    /// looks at element 192. A Wi-Fi 6 AP is the normal case here, so without
    /// this the width question is answered from the wrong element.
    he: bool,
    he_oper_params: u32,
    he_vht_op: [u8; 3],
    he_vht_op_present: bool,
    /// HT Operation `operation_mode` — the AP states here what protection the
    /// BSS needs, and it is the only place that information exists.
    ht_op_mode: u16,
    /// ERP Use_Protection: legacy 802.11b stations present.
    erp_protect: bool,
    present: bool,
    /// EDCA parameters from the AP's WMM Parameter element, indexed by the
    /// mac80211 AC number (VO, VI, BE, BK) — the same shape as Linux'
    /// `queue_params[]`. Without these the MAC context carries an all-zero
    /// EDCA table and no `MAC_QOS_FLG_UPDATE_EDCA`.
    wmm: bool,
    /// (aifs, cw_min, cw_max, txop) per AC. cw_* already expanded from ECW.
    edca: [(u8, u16, u16, u16); 4],
}
impl HtCap {
    const NONE: HtCap = HtCap {
        cap_info: 0,
        ampdu_factor: 0,
        ampdu_density: 0,
        mcs_rx: [0; 2],
        sec_chan_offs: IEEE80211_HT_PARAM_CHA_SEC_NONE,
        vht: false,
        vht_cap_info: 0,
        vht_rx_mcs_map: 0,
        vht_chan_width: IEEE80211_VHT_CHANWIDTH_USE_HT,
        vht_seg0: 0,
        he: false,
        he_oper_params: 0,
        he_vht_op: [0; 3],
        he_vht_op_present: false,
        ht_op_mode: 0,
        erp_protect: false,
        present: false,
        wmm: false,
        edca: [(0, 0, 0, 0); 4],
    };
}

/// A discovered access point (from a scan beacon / probe response).
#[derive(Clone, Copy)]
struct Ap {
    bssid: [u8; 6],
    ssid: [u8; SSID_MAX],
    ssid_len: u8,
    rssi: i8, // dBm
    channel: u8,
    beacon_int: u16, // beacon interval (TU) — for the connect MAC context
    privacy: bool,   // capability Privacy bit (encrypted → needs RSN in assoc-req)
    dtim_period: u8, // from the TIM element — the MAC context needs it to associate
    ht: HtCap,
    // The beacon timing the associated MAC context is built from. Linux keeps
    // exactly these three with the SCAN RESULT and reads them back at
    // association (mac80211/mlme.c:9464 — sync_tsf from the stored beacon,
    // sync_device_ts from bss->device_ts_beacon, sync_dtim_count from its TIM).
    // It never waits for a fresh beacon, which is what we used to do — an extra
    // step of our own invention that failed every time and left the firmware
    // with a made-up wake schedule.
    tsf: u64,
    device_ts: u32,
    dtim_count: u8,
    has_beacon: bool, // a probe response carries no usable device timestamp
}
impl Ap {
    const EMPTY: Ap = Ap {
        tsf: 0,
        device_ts: 0,
        dtim_count: 0,
        has_beacon: false,
        bssid: [0; 6],
        ssid: [0; SSID_MAX],
        ssid_len: 0,
        rssi: 0,
        channel: 0,
        beacon_int: 0,
        privacy: false,
        dtim_period: 0,
        ht: HtCap::NONE,
    };
}

/// Everything the host cannot see for itself. The kernel routes our frames but
/// knows nothing about the air: negotiated rate, retries, airtime. Without these
/// a slow link is indistinguishable from a busy one, so they are collected
/// always-on (not behind DEBUG) and published once a second via
/// `npk_driver_report`.
#[derive(Clone, Copy)]
struct Stats {
    // TX, cumulative.
    tx_frames: u32,
    tx_bytes: u64,
    tx_blocked: u32, // times the in-flight BYTE cap stopped us pulling another frame
    tx_blocked_ring: u32, // times the ring guard did instead — the byte cap was not the limit
    tx_wd_recoveries: u32, // times the queue watchdog reclaimed leaked TX slots
    inflight_corrections: u32, // times the derived read pointer beat the counter
    gtk_installs: u32,     // group keys installed = 4-way once + one per rekey
    tx_eapol_dropped: u32, // handshake replies that never reached the air
    inflight_peak: u32,
    // TX completions (iwl_tx_resp), cumulative.
    tx_ok: u32,
    tx_fail: u32,
    tx_retries: u32,  // sum of failure_frame — retransmissions on the air
    tx_rts_fail: u32, // sum of failure_rts
    tx_airtime_us: u64,
    // Is the firmware aggregating what we transmit? `frame_count` in every TX
    // response answers it outright (1 = no aggregation, >1 = aggregation), and
    // the compressed block-ack notification is the second witness. We read
    // neither until now, which is why "no TX aggregation" was an assumption.
    tx_subframes: u64, // sum of frame_count over all responses
    tx_agg_resp: u32,  // responses with frame_count > 1
    tx_agg_max: u8,    // largest frame_count seen
    ba_notifs: u32,    // BA_NOTIF received
    ba_reclaims: u32,  // TFD read pointers taken from them — the aggregated TX return path
    ba_tfd_over: u32,  // tfd_cnt beyond CBA_TFD_MAX: slots we could NOT reclaim
    ba_txed: u64,      // MPDUs the firmware says it sent in aggregates
    ba_done: u64,      // …and how many were acknowledged
    last_status: u16,
    last_init_rate: u32,
    // RX, cumulative.
    rx_frames: u32,
    rx_bytes: u64,
    rx_ip: u32,
    rx_eapol: u32,   // 4-way frames received from the AP
    tx_eapol: u32,   // …and answers wifid asked us to send back
    keys_set: u32,   // SET_KEY commands honoured (PTK + GTK = 2)
    ready_sent: u32, // EV_READY handed to wifid (once per association)
    rx_mgmt: u32,
    // Encrypted RX, counted but never acted on. `mic_fail` is the frame Linux
    // drops in iwl_mvm_rx_crypto (rxmq.c:452); `sec_none` is the firmware
    // saying it did not decrypt a frame whose Protected bit is set — a key it
    // does not have. Both are silent today: the frame goes up the stack as
    // whatever the bytes happen to be, and shows up as `undecoded`.
    // Air the AP's transmissions to us occupied, estimated per frame from the
    // rate the descriptor reports and the length: preamble + data + the SIFS
    // and ACK we are required to answer with. Backoff and DIFS are NOT counted,
    // so this is a LOWER bound — which is the safe direction, because the
    // question it answers is "is the channel full?" and an under-estimate can
    // only argue for "no".
    //
    // `own airtime` alone could never answer that: it is TX time, and during a
    // download our TX is nothing but acknowledgements.
    rx_airtime_us: u64,
    rx_airtime_pct: u32,
    win_rx_airtime_us: u64,
    // The window that carried the most bytes, kept whole. A blocking transfer
    // takes the terminal with it, so the numbers have to survive it.
    pk_rx_airtime_pct: u32,
    pk_tx_airtime_pct: u32,
    pk_retry_pct: u32,
    pk_drain: u32,
    rx_prot: u32,
    rx_mic_fail: u32,
    rx_sec_none: u32,
    rx_undecrypted: u32,
    rx_drain_max: u32, // most frames drained in one pass — RX ring pressure
    // Are we starved, or are we the bottleneck? With the air at 30 % and the
    // window at 8 MB, that is the whole remaining question, and the shape of
    // the arrivals answers it: steady frames mean the ceiling is ours, sparse
    // bursts with idle gaps mean the AP is not delivering. Per WINDOW, then
    // snapshot with the peak — an average over the whole uptime is idle time.
    win_pass_empty: u32,
    win_pass_few: u32,   // 1..=3
    win_pass_many: u32,  // 4..=15
    win_pass_burst: u32, // 16+
    win_gap_max_us: u32, // longest run of passes that found nothing
    last_rx_us: u64,
    pk_pass_empty: u32,
    pk_pass_few: u32,
    pk_pass_many: u32,
    pk_pass_burst: u32,
    pk_gap_max_us: u32,
    // Passes that drained (nearly) the whole RB pool. There are only RX_NUM_RBS
    // buffers: once they are all full the firmware has nowhere to put the next
    // frame and drops it, which TCP sees as loss and answers by backing off. A
    // rising count here means the poll loop is not keeping up, and no amount of
    // air rate will help until it does.
    rx_pool_exhausted: u32,
    // Free-BD ring health. `rb_bad_vid` = the firmware handed back a buffer id
    // outside the pool or one we never posted (Linux: WARN + iwl_force_nmi).
    // `rb_double_post` = we tried to post a buffer the firmware already owns —
    // impossible by construction in Linux, so any count here is our bug.
    // `rx_wd_fires` = RX-silence watchdog rounds, `rx_wd_dry` = consecutive
    // rounds that found nothing to re-arm, which means the pool is not the
    // fault and the firmware itself has stopped.
    rb_bad_vid: u32,
    rb_double_post: u32,
    rx_wd_fires: u32,
    rx_wd_dry: u32,
    // Unicast frames carrying our address, whatever their type, and the subset
    // we received but could not decode.
    rx_to_us: u32,
    rx_undecoded: u32,
    // Loop + events.
    loop_iters: u32,
    loop_busy: u32,
    deauth: u32,
    // Requests SEEN, separate from answers given. With `ampdu on` we answer the
    // AP only after the firmware hands back a BAID — so if that status never
    // arrives, both answer counters stay 0 and the AP is left waiting with no
    // reply at all. Without this number those two cases look identical.
    addba_seen: u32,
    addba_timeouts: u32,
    addba_declined: u32,
    addba_accepted: u32,
    // What the firmware reports about the beacons we no longer see ourselves.
    mb_notifs: u32,
    mb_consec: u32,
    mb_since_rx: u32,
    mb_expected: u32,
    mb_received: u32,
    mb_losses: u32,
    // Where a pass spends its microseconds, summed since the last report. Per
    // pass and per frame these say whether the interpreter is the ceiling.
    prof_work_us: u64,
    prof_rx_us: u64,
    prof_sleep_us: u64,
    prof_passes: u64,
    prof_frames: u64,
    // …reduced to per-pass values before the window is cleared, exactly like the
    // throughput numbers above: the report is built AFTER the reset.
    prof_work_pp: u64,
    prof_sleep_pp: u64,
    prof_drain_pp: u64,
    prof_us_frame: u64,
    // …and the same four from the second with the highest RX throughput.
    peak_work_pp: u64,
    peak_sleep_pp: u64,
    peak_drain_pp: u64,
    peak_us_frame: u64,
    peak_prof_passes: u64,
    // Rates last reported by the firmware (raw rate_n_flags).
    last_tx_rate: u32,
    last_rx_rate: u32,
    // Report window: throughput is computed here, where the counters and the
    // clock both live, so the intent only has to print it.
    win_start_ms: u64,
    win_tx_bytes: u64,
    win_rx_bytes: u64,
    win_airtime_us: u64,
    win_loop_iters: u32,
    tput_tx_kbit: u32,
    tput_rx_kbit: u32,
    airtime_pct: u32,
    passes_per_s: u32,
    // Best window seen since start. A blocking load generator (netbench holds
    // the terminal until it finishes) leaves nothing to read afterwards if only
    // the live window is kept — by then the link is idle again.
    peak_tput_tx_kbit: u32,
    peak_tput_rx_kbit: u32,
    peak_passes_per_s: u32,
    start_ms: u64,
}

impl Stats {
    const NEW: Stats = Stats {
        tx_frames: 0, tx_bytes: 0, tx_blocked: 0, tx_blocked_ring: 0, tx_wd_recoveries: 0, inflight_corrections: 0, gtk_installs: 0, tx_eapol_dropped: 0, inflight_peak: 0,
        tx_ok: 0, tx_fail: 0, tx_retries: 0, tx_rts_fail: 0, tx_airtime_us: 0,
        tx_subframes: 0, tx_agg_resp: 0, tx_agg_max: 0,
        ba_notifs: 0, ba_reclaims: 0, ba_tfd_over: 0, ba_txed: 0, ba_done: 0,
        last_status: 0, last_init_rate: 0,
        rx_frames: 0, rx_bytes: 0, rx_ip: 0, rx_eapol: 0, rx_mgmt: 0, rx_drain_max: 0,
        win_pass_empty: 0, win_pass_few: 0, win_pass_many: 0, win_pass_burst: 0,
        win_gap_max_us: 0, last_rx_us: 0,
        pk_pass_empty: 0, pk_pass_few: 0, pk_pass_many: 0, pk_pass_burst: 0,
        pk_gap_max_us: 0,
        rx_airtime_us: 0, rx_airtime_pct: 0, win_rx_airtime_us: 0,
        pk_rx_airtime_pct: 0, pk_tx_airtime_pct: 0, pk_retry_pct: 0, pk_drain: 0,
        rx_prot: 0, rx_mic_fail: 0, rx_sec_none: 0, rx_undecrypted: 0,
        tx_eapol: 0, keys_set: 0, ready_sent: 0,
        rx_pool_exhausted: 0, rx_to_us: 0, rx_undecoded: 0,
        rb_bad_vid: 0, rb_double_post: 0, rx_wd_fires: 0, rx_wd_dry: 0,
        loop_iters: 0, loop_busy: 0, deauth: 0, addba_seen: 0, addba_timeouts: 0, addba_declined: 0, addba_accepted: 0,
        mb_notifs: 0, mb_consec: 0, mb_since_rx: 0, mb_expected: 0, mb_received: 0,
        mb_losses: 0,
        prof_work_us: 0, prof_rx_us: 0, prof_sleep_us: 0, prof_passes: 0, prof_frames: 0,
        prof_work_pp: 0, prof_sleep_pp: 0, prof_drain_pp: 0, prof_us_frame: 0,
        peak_work_pp: 0, peak_sleep_pp: 0, peak_drain_pp: 0, peak_us_frame: 0,
        peak_prof_passes: 0,
        last_tx_rate: 0, last_rx_rate: 0,
        win_start_ms: 0, win_tx_bytes: 0, win_rx_bytes: 0, win_airtime_us: 0,
        win_loop_iters: 0,
        tput_tx_kbit: 0, tput_rx_kbit: 0, airtime_pct: 0, passes_per_s: 0,
        peak_tput_tx_kbit: 0, peak_tput_rx_kbit: 0, peak_passes_per_s: 0, start_ms: 0,
    };
}

/// Fixed-size text builder for the status snapshot (no allocator in a driver).
struct Rep {
    b: [u8; REPORT_CAP],
    n: usize,
}

impl Rep {
    const fn new() -> Rep { Rep { b: [0; REPORT_CAP], n: 0 } }

    /// Reserve the tail for a marker: silent truncation cost a whole debugging
    /// round once already (the last two lines were simply gone from the report).
    const LIMIT: usize = REPORT_CAP - 16;

    fn s(&mut self, s: &str) {
        for &c in s.as_bytes() {
            if self.n < Self::LIMIT { self.b[self.n] = c; self.n += 1; }
            else { self.mark_truncated(); return; }
        }
    }

    fn mark_truncated(&mut self) {
        const MARK: &[u8] = b"\n[truncated]\n";
        if self.n + MARK.len() <= REPORT_CAP && !self.b[..self.n].ends_with(MARK) {
            self.b[self.n..self.n + MARK.len()].copy_from_slice(MARK);
            self.n += MARK.len();
        }
    }
    fn c(&mut self, c: u8) {
        if self.n < Self::LIMIT { self.b[self.n] = c; self.n += 1; }
        else { self.mark_truncated(); }
    }
    fn d(&mut self, mut v: u64) {
        if v == 0 { self.c(b'0'); return; }
        let mut tmp = [0u8; 20];
        let mut i = 20;
        while v > 0 { i -= 1; tmp[i] = b'0' + (v % 10) as u8; v /= 10; }
        for k in i..20 { self.c(tmp[k]); }
    }
    fn i(&mut self, v: i32) {
        if v < 0 { self.c(b'-'); self.d((-(v as i64)) as u64); } else { self.d(v as u64); }
    }
    /// A value in thousandths printed as `x.y` — the driver has no float
    /// formatting and 11.9 Mbit/s reads better than 11900 kbit/s.
    fn kbit_as_mbit(&mut self, kbit: u32) {
        self.d((kbit / 1000) as u64);
        self.c(b'.');
        self.d(((kbit % 1000) / 100) as u64);
    }
    fn hex(&mut self, v: u32, digits: usize) {
        const H: &[u8; 16] = b"0123456789abcdef";
        for k in (0..digits).rev() {
            self.c(H[((v >> (k * 4)) & 0xf) as usize]);
        }
    }
    fn mac(&mut self, m: &[u8; 6]) {
        for (k, &b) in m.iter().enumerate() {
            if k > 0 { self.c(b':'); }
            self.hex(b as u32, 2);
        }
    }
    fn pct(&mut self, part: u64, whole: u64) {
        if whole == 0 { self.s("0%"); return; }
        self.d(part * 100 / whole);
        self.c(b'%');
    }
}

/// AX200 transport state. Mirrors the bits of `struct iwl_trans_pcie` we use.
struct Ax200 {
    mmio: i32,
    ltr_enabled: bool,
    hw_rev: u32,
    // RX queue DMA (iwl_pcie_alloc_rxq_dma) — addresses go into ctxt_info.
    rxq_bd: Dma,       // RBD ring (__le64 * NUM_RBDS)
    rxq_used_bd: Dma,  // used-BD ring (__le32 * NUM_RBDS)
    rxq_rb_stts: Dma,  // struct iwl_rb_status
    // TX command queue DMA (iwl_pcie_txq_alloc, gen2).
    cmd_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_CMD_QUEUE_SIZE)
    cmd_first_tb: Dma, // first-TB staging buffers
    cmd_data: Dma,     // payload buffer for large (NOCOPY) commands → TB1
    cmd_write_ptr: u32, // txq->write_ptr for the command queue
    // RX RB pool (vid v → rb_pool[v-1]) + our read index into the used-BD ring
    // + the free-BD ring write index (for recycling RBs during the scan).
    rb_pool: [Dma; RX_NUM_RBS],
    /// iwl_rx_mem_buffer.invalid, inverted: true while the buffer sits in the
    /// free-BD ring and the firmware owns it, false while it is ours. Linux
    /// keeps the same fact as list membership (rx_free / posted / rx_used) and
    /// restocks strictly from rx_free. A buffer can therefore be in the ring at
    /// most once, which is what keeps the write index from ever lapping the
    /// firmware's read index — no matter how big the pool gets.
    rb_in_fw: [bool; RX_NUM_RBS],
    rxq_read: u32,
    free_bd_write: u32,
    // Firmware error-table SRAM pointers (from the ALIVE notification), for
    // dumping the FW error log when a command/scan produces no response.
    lmac_err_ptr: u32,
    umac_err_ptr: u32,
    // Scan channel list parsed from the NVM_GET_INFO regulatory section: the
    // NVM_CHANNEL_VALID channels with their PHY band, for both 2.4 and 5 GHz.
    scan_chans: [u8; SCAN_MAX_CHANS], // channel numbers
    scan_bands: [u8; SCAN_MAX_CHANS], // PHY_BAND_24 / PHY_BAND_5 per channel
    n_scan_chans: usize,
    // The card's MAC address (from the CSR strap/OTP), for netdev registration.
    mac: [u8; 6],
    // Connect target picked from the scan (strongest AP) — for #3 connect (5a+).
    target_bssid: [u8; 6],
    target_chan: u8,
    target_band: u8, // PHY_BAND_24 / PHY_BAND_5
    target_beacon_int: u16, // beacon interval of the target AP (for MAC context)
    target_ssid: [u8; SSID_MAX],
    target_ssid_len: u8,
    target_privacy: bool, // target is encrypted (WPA2) → assoc-req carries an RSN IE
    target_rssi: i8,
    target_valid: bool,
    // The target AP's HT capabilities + DTIM period (from its beacon during the
    // scan) and the association id the AP handed us. `ht.present` gates the whole
    // 802.11n path: HT + WMM elements in the assoc request, QoS data frames, HT
    // station flags and TLC mode HT. An AP without HT keeps the legacy path.
    target_ht: HtCap,
    target_dtim_period: u8,
    assoc_aid: u16,
    qos: bool, // associated as an HT/QoS station → QoS data frames
    // Beacon timing captured right after association (iwl_mvm_set_fw_dtim_tbtt):
    // the AP's TSF and our device timestamp at the last beacon, plus the DTIM
    // count still to run. The MAC context needs them to be marked associated.
    sync_tsf: u64,
    sync_device_ts: u32,
    sync_dtim_count: u8,
    // gen2 management TX queue for the AP station (auth/assoc frames, 5b+).
    mgmt_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_MGMT_QUEUE_SIZE)
    mgmt_first_tb: Dma, // first-TB staging buffers
    mgmt_payload: Dma,  // per-slot TB1 payload staging (no shared-buffer clobber)
    mgmt_bc_tbl: Dma,   // byte-count table (FW DMA scheduling)
    mgmt_queue_id: u16, // queue id returned by the firmware
    mgmt_write_ptr: u32,
    // gen2 data TX queue (tid 0) for EAPOL + IP frames.
    data_tfd: Dma,
    data_first_tb: Dma,
    data_payload: Dma,  // per-slot TB1 payload staging (one region per TFD slot)
    data_bc_tbl: Dma,
    data_queue_id: u16,
    data_write_ptr: u32,
    /// The firmware's read pointer for the data queue, derived from the TFD
    /// index every TX response carries in its header sequence
    /// (`SEQ_TO_INDEX`, cmdhdr.h:20) — the same source `iwl_pcie_reclaim`
    /// uses. `data_in_flight` used to be a COUNTER: incremented per submit,
    /// decremented per completion, and therefore permanently wrong the moment
    /// one completion went missing. Derived from the two pointers it is
    /// self-correcting: the very next completion snaps it back to the truth.
    data_read_ptr: u32,
    // Frames handed to the data queue but not yet reported complete by the FW
    // (TX_CMD response). Flow control: never enqueue past the queue depth, or we
    // overwrite a TFD the firmware is still transmitting → corruption/stall.
    /// Firmware key-table bookkeeping (iwl_mvm_set_fw_key_idx). `freed`
    /// counts how long each slot has been free; the previous group slot is
    /// held until the one after it arrives, so a rekey never invalidates the
    /// key the AP may still be transmitting with.
    key_slot_used: [bool; STA_KEY_MAX_NUM as usize],
    key_slot_freed: [u16; STA_KEY_MAX_NUM as usize],
    key_slot_prev: Option<u8>,
    /// Is the pairwise key installed? It decides whether an EAPOL frame goes
    /// out in the clear. mac80211 (`ieee80211_tx_h_select_key`) picks the
    /// station's PTK for EVERY frame with a payload — an EAPOL-Key frame is
    /// one — and only the pre-key control port carries
    /// `IEEE80211_TX_INTFL_DONT_ENCRYPT`.
    ptk_installed: bool,
    data_in_flight: u32,
    /// Bytes sitting in the slots `data_in_flight` covers. Kept the same way
    /// the frame count is: incremented on submit, re-derived once per pass from
    /// the firmware's read pointer, so it inherits the self-correction instead
    /// of becoming a second, drifting truth.
    data_bytes_in_flight: u32,
    /// On-air length of the frame staged in each TFD slot. The only thing the
    /// byte accounting needs that the two pointers cannot tell us.
    slot_bytes: [u16; IWL_DATA_QUEUE_SIZE],
    /// `now_ms` of the last TX completion, or of the last moment the queue was
    /// empty. The queue watchdog measures from here (iwl_txq_stuck_timer).
    last_tx_done_ms: u64,
    // 802.11 sequence number for non-QoS data frames. mac80211 assigns this per
    // frame (ieee80211_tx_h_sequence); the gen2 firmware does NOT do it for us,
    // so every data frame must carry a unique, incrementing seq or the AP treats
    // distinct frames as duplicates (dropping TCP data, duplicating ACKed ones).
    tx_seq: u16,
    // An ADDBA request waiting for the firmware's verdict (see ba_request).
    ba_pending: Option<BaPending>,
    /// Link state as the kernel currently sees it. The truth is `authorized`;
    /// this is only what we last told netdev, so a missed edge can be noticed.
    link_published: bool,
    /// Consecutive unacknowledged transmissions, and when the firmware last
    /// answered a transmission at all. An associated link that stops being
    /// acknowledged is dead, and nothing else notices: a deauth never comes, the
    /// missed-beacon notification has never once fired on this hardware, and the
    /// 4-way watchdog only guards the time BEFORE authorization. So the link
    /// stays "up" and every packet vanishes — measured: ping 100 % loss with
    /// state UP, and it never recovered on its own.
    tx_fail_streak: u32,
    tx_fail_streak_peak: u32,
    last_tx_resp_ms: u64,
    want_ampdu: bool,
    /// 40 MHz. OFF by default — this is the one that took the link down on
    /// 2026-08-19, bisected over a whole evening: association succeeds and
    /// then not one frame reaches us again (`to-us 0`), on both bands, with a
    /// `pre-assoc beacon ok` right before. The port itself stays in place; the
    /// defect is somewhere in what we hand the firmware at the width change,
    /// and a switch beats deleting the code. Turn it on for a measurement, and
    /// `wlan unset ht40` is one command away when it goes dark again.
    want_ht40: bool,
    want_vht: bool,
    want_txagg: bool,
    /// Set once the firmware has ignored a block-ack setup. Asking again costs
    /// another silent 300 ms AND wedges the transmit path: measured on the
    /// device, 31 frames sent / 7 acknowledged / 500 refused, and no ping.
    /// Linux would not be sending this command at all on a firmware that
    /// advertises BAID_ML_SUPPORT (it uses RX_BAID_ALLOCATION_CONFIG_CMD via
    /// iwl_mvm_fw_baid_op, sta.c:2860) — until we implement that, one refusal
    /// is all the evidence needed to stop asking.
    ba_fw_broken: bool,
    // Diagnostics (see Stats) + what the scan found besides the chosen AP: the
    // strongest same-SSID AP on the OTHER band. Picking purely by RSSI always
    // lands on the near 2.4 GHz node, so the question "was there a 5 GHz one?"
    // has to survive the scan to be answerable later.
    st: Stats,
    n_aps: u8,
    alt_bssid: [u8; 6],
    alt_chan: u8,
    alt_rssi: i8,
    alt_ht: bool,
    alt_valid: bool,
    authorized: bool,
    // Connect policy, read once from npkFS at start-up (same place wifid takes
    // its credential from). Picking the loudest AP of any network is wrong on
    // two counts: on a dual-band mesh it is always the near 2.4 GHz node, and if
    // a neighbour's network is louder we associate to a BSS whose PSK wifid does
    // not have — a silent 4-way MIC failure.
    want_ssid: [u8; SSID_MAX],
    want_ssid_len: u8,
    band_pref: u8, // BAND_PREF_*
    want_power_save: bool,
    want_bt_coex: bool,
    settle_ms: u32,
    sync_ok: bool,
    /// 0 = unchecked, 1 = clean, else the LMAC error id. Sampled ONCE after
    /// bring-up: reading it needs grab_nic_access + PRPH reads, and doing that
    /// once a second from a status report pokes registers underneath a running
    /// firmware. Diagnostics must not be able to break what they measure.
    fw_assert: u32,
    blacklist: [[u8; 6]; 4],
    n_blacklist: usize,
    pick_reason: u8, // PICK_* — why the target was chosen, for the report
}

impl Ax200 {
    // ── CSR direct register access (BAR0) ────────────────────────
    fn r32(&self, reg: u32) -> u32 { host::mmio_r32(self.mmio, reg) }
    fn w32(&self, reg: u32, val: u32) { host::mmio_w32(self.mmio, reg, val); }
    /// iwl_set_bit: RMW preserving existing bits.
    fn set_bit(&self, reg: u32, bits: u32) { host::mmio_set32(self.mmio, reg, bits); }

    // ── PRPH access through HBUS (iwl_trans_pcie_read/write_prph) ─
    // umac_prph_offset is 0 for AX200, so umac == regular prph.
    fn prph_read(&self, reg: u32) -> u32 {
        self.w32(HBUS_TARG_PRPH_RADDR, (reg & PRPH_MASK) | (3 << 24));
        self.r32(HBUS_TARG_PRPH_RDAT)
    }
    fn prph_write(&self, reg: u32, val: u32) {
        self.w32(HBUS_TARG_PRPH_WADDR, (reg & PRPH_MASK) | (3 << 24));
        self.w32(HBUS_TARG_PRPH_WDAT, val);
    }

    /// iwl_poll_bit: spin until (reg & mask) == mask or timeout (microseconds).
    /// IWL_POLL_INTERVAL is 10us in Linux; we lack a us timer, so we pace with
    /// a short busy-spin (small timeouts) or a yielding 1ms sleep (large ones).
    fn poll_bit(&self, reg: u32, mask: u32, timeout_us: u32) -> bool {
        let mut waited = 0u32;
        loop {
            if self.r32(reg) & mask == mask { return true; }
            if waited >= timeout_us { return false; }
            if timeout_us >= 1000 {
                host::sleep_ms(1);
                waited += 1000;
            } else {
                for _ in 0..64 { core::hint::spin_loop(); }
                waited += 10;
            }
        }
    }

    // ── iwl_pcie_set_hw_ready / prepare_card_hw (trans.c) ────────
    fn set_hw_ready(&self) -> bool {
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_PCI_OWN_SET);
        let ready = self.poll_bit(
            CSR_HW_IF_CONFIG_REG,
            CSR_HW_IF_CONFIG_REG_PCI_OWN_SET,
            HW_READY_TIMEOUT_US,
        );
        if ready {
            self.set_bit(CSR_MBOX_SET_REG, CSR_MBOX_SET_REG_OS_ALIVE);
        }
        ready
    }

    /// Returns true once the card is ready (owned by us, not AMT/ME).
    fn prepare_card_hw(&self) -> bool {
        if self.set_hw_ready() {
            return true;
        }
        self.set_bit(CSR_DBG_LINK_PWR_MGMT_REG, CSR_RESET_LINK_PWR_MGMT_DISABLED);
        host::sleep_ms(2); // usleep_range(1000, 2000)

        for _ in 0..10 {
            // Prepare conditions to check again: wake the management bus.
            self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_WAKE_ME);
            // Inner do-while: up to 150ms (Linux uses 200us steps; we use 1ms).
            let mut t = 0u32;
            loop {
                if self.set_hw_ready() {
                    return true;
                }
                // No iwl_mei (CSME) in our world — that branch is skipped.
                host::sleep_ms(1);
                t += 1000;
                if t >= 150_000 {
                    break;
                }
            }
            host::sleep_ms(25);
        }
        false
    }

    // ── iwl_trans_pcie_sw_reset (trans.c) ───────────────────────
    // AX200 family 22000 < BZ → CSR_RESET path, usleep_range(5000, 6000).
    fn sw_reset(&self, retake_ownership: bool) -> bool {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        host::sleep_ms(6);
        if retake_ownership {
            self.prepare_card_hw()
        } else {
            true
        }
    }

    // ── iwl_trans_pcie_clear_persistence_bit (trans.c) ──────────
    // Family 22000 → wprot = PREG_PRPH_WPROT_22000. Returns false only on
    // the unrecoverable -EPERM path.
    fn clear_persistence_bit(&self) -> bool {
        let hpm = self.prph_read(HPM_DEBUG);
        if !is_hw_error_value(hpm) && (hpm & PERSISTENCE_BIT) != 0 {
            let wprot_val = self.prph_read(PREG_PRPH_WPROT_22000);
            if wprot_val & PREG_WFPM_ACCESS != 0 {
                host::dprint("[ax200] error: can not clear persistence bit\n");
                return false;
            }
            self.prph_write(HPM_DEBUG, hpm & !PERSISTENCE_BIT);
        }
        true
    }

    // ── iwl_pcie_apm_config (trans.c) ───────────────────────────
    // L0s is unstable on these devices: always set L0S_DISABLED. Then cache
    // ASPM / LTR capability for later (set_pwr / set_ltr in Stage 2).
    fn apm_config(&mut self) {
        self.set_bit(CSR_GIO_REG, CSR_GIO_REG_VAL_L0S_DISABLED);

        let cap = pcie_find_cap(PCI_CAP_ID_EXP);
        if cap != 0 {
            let _lctl = pci_read16(cap + PCI_EXP_LNKCTL);
            // pm_support = !(lctl & ASPM_L0S) — not needed until power mgmt.
            let cap2 = pci_read16(cap + PCI_EXP_DEVCTL2);
            self.ltr_enabled = cap2 & PCI_EXP_DEVCTL2_LTR_EN != 0;
        }
        host::dprint("[ax200] apm_config: LTR ");
        host::dprint(if self.ltr_enabled { "enabled\n" } else { "disabled\n" });
    }

    // ── iwl_pcie_gen1_2_activate_nic (trans.c) ──────────────────
    // bisr_workaround = 1 for AX200: mdelay(2) before, udelay(200) after.
    // Family < BZ → INIT_DONE, poll MAC_CLOCK_READY.
    fn activate_nic(&self) -> bool {
        host::sleep_ms(2); // bisr_workaround: TOP FSM settle

        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_INIT_DONE);

        let ok = self.poll_bit(
            CSR_GP_CNTRL,
            CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY,
            MAC_CLOCK_TIMEOUT_US,
        );
        if !ok {
            host::dprint("[ax200] failed to wake NIC (MAC clock not ready)\n");
        }

        for _ in 0..1024 { core::hint::spin_loop(); } // bisr_workaround: udelay(200)
        ok
    }

    // ── iwl_pcie_apm_init (trans.c, gen1 — the start_hw path) ────
    // Family 22000: DIS_L0S_EXIT_TIMER (family < 8000) skipped; no pll_cfg;
    // no host_interrupt_operation_mode.
    fn apm_init(&mut self) -> bool {
        // Disable L0s without affecting L1 (ICH bug W/A).
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        // FH wait threshold to maximum (HW error during stress W/A).
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        // HAP INTA: wake the PCIe link L1a -> L0s.
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);

        self.apm_config();
        // pll_cfg: AX200 base has none → no CSR_ANA_PLL_CFG.
        self.activate_nic()
    }

    // ── _iwl_trans_pcie_start_hw (trans.c) ──────────────────────
    fn start_hw(&mut self) -> bool {
        if !self.prepare_card_hw() {
            host::dprint("[ax200] error while preparing HW (AMT owns the device?)\n");
            return false;
        }
        host::dprint("[ax200] card prepared (ownership taken)\n");

        if !self.clear_persistence_bit() {
            return false;
        }

        if !self.sw_reset(true) {
            host::dprint("[ax200] sw_reset: card not ready after reset\n");
            return false;
        }
        host::dprint("[ax200] sw_reset done\n");

        // force_power_gating: family==22000 && integrated. AX200 is a discrete
        // M.2 card (not integrated) → skipped.

        if !self.apm_init() {
            return false;
        }
        host::dprint("[ax200] apm_init done — MAC clock ready\n");
        true
    }

    // ── iwl_pcie_gen2_apm_init (trans-gen2.c) ───────────────────
    // Same register effect as apm_init for family 22000 (gen1's DIS_L0S /
    // pll branches are already conditioned out). nic_init re-runs it.
    fn gen2_apm_init(&mut self) -> bool {
        self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);
        self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);
        self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);
        self.apm_config();
        self.activate_nic()
        // STATUS_DEVICE_ENABLED is host-side bookkeeping, not a register.
    }

    /// Allocate a coherent DMA buffer of at least `bytes`. npk_dma_alloc
    /// zeroes the pages and guarantees contiguous + below 4 GB, matching
    /// Linux' dma_alloc_coherent + the "no 4 GB boundary cross" requirement.
    fn alloc_dma(&self, bytes: usize, name: &str) -> Dma {
        let pages = ((bytes + 4095) / 4096) as u16;
        let handle = host::dma_alloc(pages);
        if handle < 0 {
            // LOUD. A failed DMA allocation leaves a NONE handle that every
            // later read and write silently ignores — the card simply never
            // works, with no message anyone sees. `dprint` was the wrong
            // channel for the one failure that makes the driver useless.
            host::print("[ax200] FATAL: DMA alloc failed for ");
            host::print(name);
            host::print(" — the kernel's per-module allocation limit is full\n");
            return Dma::NONE;
        }
        Dma { handle, phys: host::dma_phys(handle) }
    }

    // ── iwl_pcie_gen2_rx_init (rx.c) ────────────────────────────
    // gen2 does NOT configure the RFH (firmware does it at alive) and the RB
    // page pool is filled at restock (alive). So here: set the int-coalescing
    // timer and allocate the ctxt_info-referenced RX rings. num_rxqs = 1.
    fn gen2_rx_init(&mut self) -> bool {
        host::mmio_w8(self.mmio, CSR_INT_COALESCING, IWL_HOST_INT_TIMEOUT_DEF);

        self.rxq_bd = self.alloc_dma(FREE_BD_SIZE * NUM_RBDS, "rxq.bd");
        self.rxq_used_bd = self.alloc_dma(USED_BD_SIZE * NUM_RBDS, "rxq.used_bd");
        self.rxq_rb_stts = self.alloc_dma(RB_STTS_SIZE, "rxq.rb_stts");

        self.rxq_bd.ok() && self.rxq_used_bd.ok() && self.rxq_rb_stts.ok()
    }

    // ── iwl_txq_gen2_init (tx-gen2.c) — command queue ───────────
    // iwl_pcie_txq_alloc allocates the TFD ring + first-TB staging buffers.
    // dma_alloc zeroing leaves every TFD with num_tbs=0 (set-invalid-gen2).
    // The byte-count table is not in the gen2 cmd-queue path and is not
    // referenced by ctxt_info, so it is not allocated here.
    fn txq_gen2_init(&mut self) -> bool {
        let slots = IWL_CMD_QUEUE_SIZE; // max(IWL_CMD_QUEUE_SIZE, min_txq_size=0)
        self.cmd_tfd = self.alloc_dma(TFH_TFD_SIZE * slots, "cmd.tfd");
        self.cmd_first_tb = self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * slots, "cmd.first_tb");
        // Payload buffer for large host commands: their bulk is mapped as a
        // second TB (NOCOPY) instead of being copied into the cmd buffer. One
        // page covers the largest command we build (SCAN_REQ_UMAC ~1.7 KB).
        self.cmd_data = self.alloc_dma(CMD_DATA_BYTES, "cmd.data");
        self.cmd_tfd.ok() && self.cmd_first_tb.ok() && self.cmd_data.ok()
    }

    // ── iwl_pcie_gen2_nic_init (trans-gen2.c) ───────────────────
    fn nic_init(&mut self) -> bool {
        if !self.gen2_apm_init() {
            host::dprint("[ax200] nic_init: gen2_apm_init failed\n");
            return false;
        }
        // iwl_op_mode_nic_config (mvm): DEFERRED. It is the op-mode/NVM layer
        // (radio-stepping CSR bits), not the PCIe transport, and is not needed
        // for the firmware CPU to reach ALIVE. Lands with the mvm port.

        if !self.gen2_rx_init() {
            return false;
        }
        if !self.txq_gen2_init() {
            return false;
        }

        // enable shadow regs in HW
        self.set_bit(CSR_MAC_SHADOW_REG_CTRL, CSR_MAC_SHADOW_REG_CTRL_VAL);
        true
    }

    // ── start_fw prologue (trans-gen2.c, the bits before nic_init) ──
    // disable interrupts, check RF-kill, clear the RF-kill handshake so the
    // firmware doesn't think the radio is killed, then clear pending ints.
    fn prepare_for_fw_load(&self) {
        // iwl_disable_interrupts
        self.w32(CSR_INT_MASK, 0);
        self.w32(CSR_INT, 0xFFFF_FFFF);
        self.w32(CSR_FH_INT_STATUS, 0xFFFF_FFFF);

        // iwl_pcie_check_hw_rf_kill: bit clear == radio killed.
        if self.rf_killed() {
            // A killed radio means the firmware boots, answers commands and
            // keeps every receive buffer — and not one frame ever arrives. That
            // is indistinguishable from a driver bug unless someone says it, so
            // it goes over `print`.
            host::print("[ax200] HW RF-KILL asserted — the radio is off (switch/Fn key/BIOS). No frame can arrive until it is cleared.\n");
        }

        // make sure rfkill handshake bits are cleared
        self.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_SW_BIT_RFKILL);
        self.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_DRV_GP1_BIT_CMD_BLOCKED);
        self.w32(CSR_INT, 0xFFFF_FFFF);
    }

    // ── iwl_pcie_set_ltr (trans-gen2.c, 22000 non-integrated) ───
    fn set_ltr(&self) {
        let ltr = CSR_LTR_LONG_VAL_AD_NO_SNOOP_REQ
            | encode_bits(CSR_LTR_LONG_VAL_AD_SCALE_USEC, CSR_LTR_LONG_VAL_AD_NO_SNOOP_SCALE)
            | encode_bits(250, CSR_LTR_LONG_VAL_AD_NO_SNOOP_VAL)
            | CSR_LTR_LONG_VAL_AD_SNOOP_REQ
            | encode_bits(CSR_LTR_LONG_VAL_AD_SCALE_USEC, CSR_LTR_LONG_VAL_AD_SNOOP_SCALE)
            | encode_bits(250, CSR_LTR_LONG_VAL_AD_SNOOP_VAL);
        self.w32(CSR_LTR_LONG_VAL_AD, ltr);
    }

    // ── iwl_pcie_init_fw_sec (ctxt-info.c) ──────────────────────
    // Walk the SEC_RT TLVs in order; the CPU1_CPU2 / PAGING separators split
    // them into lmac / umac / paging. DMA each section's data and record its
    // physical address into the matching ctxt_dram image array. The firmware
    // reads these chunks itself; init_fw_sec ignores the per-section offset.
    fn init_fw_sec(
        &self,
        lmac: &mut [u64; IWL_MAX_DRAM_ENTRY],
        umac: &mut [u64; IWL_MAX_DRAM_ENTRY],
        virt: &mut [u64; IWL_MAX_DRAM_ENTRY],
    ) -> (usize, usize, usize) {
        let (mut lc, mut uc, mut vc) = (0usize, 0usize, 0usize);
        let mut region = 0u8; // 0 = lmac, 1 = umac, 2 = paging
        let mut off = FW_TLV_HEADER_LEN;

        while off + 8 <= FW.len() {
            let t = le32(FW, off);
            let l = le32(FW, off + 4) as usize;
            let body = off + 8;
            if body + l > FW.len() {
                break;
            }
            if t == IWL_UCODE_TLV_SEC_RT {
                let sec_off = le32(FW, body);
                if sec_off == CPU1_CPU2_SEPARATOR_SECTION {
                    region = 1;
                } else if sec_off == PAGING_SEPARATOR_SECTION {
                    region = 2;
                } else {
                    let data = &FW[body + 4..body + l];
                    let dma = self.alloc_dma(data.len(), "fw.sec");
                    if !dma.ok() {
                        return (lc, uc, vc); // caller checks counts
                    }
                    host::dma_write_buf(dma.handle, 0, data);
                    match region {
                        0 if lc < IWL_MAX_DRAM_ENTRY => { lmac[lc] = dma.phys; lc += 1; }
                        1 if uc < IWL_MAX_DRAM_ENTRY => { umac[uc] = dma.phys; uc += 1; }
                        2 if vc < IWL_MAX_DRAM_ENTRY => { virt[vc] = dma.phys; vc += 1; }
                        _ => {}
                    }
                }
            }
            off = body + ((l + 3) & !0x3);
        }
        (lc, uc, vc)
    }

    // ── iwl_pcie_ctxt_info_init + the start_fw tail → ALIVE ─────
    fn load_firmware(&mut self) -> bool {
        self.prepare_for_fw_load();

        // init_fw_sec: DMA the firmware sections.
        let mut lmac = [0u64; IWL_MAX_DRAM_ENTRY];
        let mut umac = [0u64; IWL_MAX_DRAM_ENTRY];
        let mut virt = [0u64; IWL_MAX_DRAM_ENTRY];
        let (lc, uc, vc) = self.init_fw_sec(&mut lmac, &mut umac, &mut virt);
        if lc == 0 || uc == 0 {
            host::dprint("[ax200] FW section load failed\n");
            return false;
        }
        host::dprint("[ax200] FW sections loaded: lmac=");
        host::dprint_hex32(lc as u32);
        host::dprint(" umac=");
        host::dprint_hex32(uc as u32);
        host::dprint(" paging=");
        host::dprint_hex32(vc as u32);
        host::dprint("\n");

        // Build the context-info structure (zeroed buffer + filled fields).
        let mut ci = [0u8; CTXT_INFO_SIZE];
        put_u16(&mut ci, CI_OFF_MAC_ID, self.hw_rev as u16);
        put_u16(&mut ci, CI_OFF_VERSION, 0);
        put_u16(&mut ci, CI_OFF_SIZE, (CTXT_INFO_SIZE / 4) as u16);

        let cb_size = (NUM_RBDS as u32).trailing_zeros(); // RX_QUEUE_CB_SIZE = ilog2
        let control_flags = IWL_CTXT_INFO_TFD_FORMAT_LONG
            | (cb_size << IWL_CTXT_INFO_RB_CB_SIZE_SHIFT)
            | (IWL_CTXT_INFO_RB_SIZE_4K << IWL_CTXT_INFO_RB_SIZE_SHIFT);
        put_u32(&mut ci, CI_OFF_CONTROL_FLAGS, control_flags);

        put_u64(&mut ci, CI_OFF_FREE_RBD, self.rxq_bd.phys);
        put_u64(&mut ci, CI_OFF_USED_RBD, self.rxq_used_bd.phys);
        put_u64(&mut ci, CI_OFF_STATUS_WR, self.rxq_rb_stts.phys);
        put_u64(&mut ci, CI_OFF_CMD_QUEUE_ADDR, self.cmd_tfd.phys);
        ci[CI_OFF_CMD_QUEUE_SIZE] = CMD_QUEUE_CB_SIZE;

        for i in 0..lc { put_u64(&mut ci, CI_OFF_LMAC_IMG + i * 8, lmac[i]); }
        for i in 0..uc { put_u64(&mut ci, CI_OFF_UMAC_IMG + i * 8, umac[i]); }
        for i in 0..vc { put_u64(&mut ci, CI_OFF_VIRTUAL_IMG + i * 8, virt[i]); }

        let ci_dma = self.alloc_dma(CTXT_INFO_SIZE, "ctxt_info");
        if !ci_dma.ok() {
            return false;
        }
        host::dma_write_buf(ci_dma.handle, 0, &ci);

        // iwl_enable_fw_load_int_ctx_info (non-MSI-X): the early ALIVE comes as
        // CSR_INT_BIT_ALIVE; FH_RX is for the later (Stage 3) ALIVE notification.
        self.w32(CSR_INT_MASK, CSR_INT_BIT_ALIVE | CSR_INT_BIT_FH_RX);

        // kick FW self-load: write the ctxt_info physical address.
        host::mmio_w64(self.mmio, CSR_CTXT_INFO_BA, ci_dma.phys);

        self.set_ltr();

        // tell the FW CPU to run (family < AX210 → regular PRPH).
        self.prph_write(UREG_CPU_INIT_RUN, 1);

        // Poll CSR_INT for the early ALIVE interrupt (no MSI-X, no notif_wait).
        host::dprint("[ax200] FW kicked, waiting for ALIVE...\n");
        if self.poll_bit(CSR_INT, CSR_INT_BIT_ALIVE, 2_000_000) {
            return true;
        }
        let intr = self.r32(CSR_INT);
        host::dprint("[ax200] ALIVE timeout — CSR_INT=0x");
        host::dprint_hex32(intr);
        host::dprint("\n");
        false
    }

    // ── iwl_pcie_rxmq_restock + read the ALIVE notification ─────
    // After the early ALIVE interrupt the firmware has configured the RFH, so
    // we may restock: allocate the RB pool, write each buffer into the free-RBD
    // ring (bd[i] = page_dma | vid, gen2 < AX210), then bump the HW write
    // pointer. The firmware then DMAs UCODE_ALIVE_NTFY into the first RB and
    // advances rb_stts.closed_rb_num, which we poll.
    fn rx_restock_and_alive(&mut self) -> Option<Dma> {
        let mut bd = [0u8; RX_NUM_RBS * 8];
        for i in 0..RX_NUM_RBS {
            let rb = self.alloc_dma(RB_SIZE_BYTES, "rb");
            if !rb.ok() {
                return None;
            }
            self.rb_pool[i] = rb;
            // Posted straight into slots 0..RX_NUM_RBS-1, so the firmware owns
            // every one of them from here on (recycle_rb is bypassed, hence the
            // explicit flag).
            self.rb_in_fw[i] = true;
            // vid = i + 1; page is 4K-aligned so the low bits hold the vid.
            let entry = rb.phys | (i as u64 + 1);
            bd[i * 8..i * 8 + 8].copy_from_slice(&entry.to_le_bytes());
        }
        host::dma_write_buf(self.rxq_bd.handle, 0, &bd);
        host::fence();

        // iwl_pcie_rxq_inc_wr_ptr: write_actual = round_down(write, 8).
        self.free_bd_write = RX_NUM_RBS as u32; // RBs posted at slots 0..N-1
        let write_actual = self.free_bd_write & !0x7;
        self.w32(RFH_Q0_FRBDCB_WIDX_TRG, write_actual);

        host::dprint("[ax200] RX restocked, waiting for alive notification...\n");
        let mut closed = 0u32;
        for _ in 0..1000 {
            host::fence();
            closed = host::dma_r32(self.rxq_rb_stts.handle, 0) & RB_STTS_CLOSED_MASK;
            if closed != 0 {
                break;
            }
            host::sleep_ms(1);
        }
        if closed == 0 {
            host::dprint("[ax200] no RX — alive notification timeout\n");
            return None;
        }
        host::dprint("[ax200] RX active — closed_rb_num=0x");
        host::dprint_hex32(closed);
        host::dprint("\n");

        // The FW reports each filled RB in the used-BD ring (vid). Read used_bd[0]
        // to find which RB holds the first frame (iwl_pcie_get_rxb, < AX210 path).
        let vid = host::dma_r32(self.rxq_used_bd.handle, 0) & RX_VID_MASK;
        let rb0 = match self.claim_rb(vid) {
            Some(rb) => rb,
            None => {
                host::dprint("[ax200] bad RX vid\n");
                return None;
            }
        };
        self.rxq_read = 1; // consumed used_bd[0]

        // Dump the first RB header (iwl_rx_packet: len_n_flags, cmd, group_id).
        let mut hdr = [0u8; 8];
        host::dma_read_buf(rb0.handle, 0, &mut hdr);
        let len_n_flags = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        host::dprint("[ax200] RB[0] len_n_flags=0x");
        host::dprint_hex32(len_n_flags);
        host::dprint(" cmd=0x");
        host::dprint_hex32(hdr[4] as u32);
        host::dprint(" group=0x");
        host::dprint_hex32(hdr[5] as u32);
        host::dprint("\n");
        if hdr[4] == UCODE_ALIVE_NTFY && hdr[5] == 0 {
            host::dprint("[ax200] → UCODE_ALIVE_NTFY confirmed\n");
        }
        Some(rb0)
    }

    // ── iwl_alive_fn (mvm/fw.c, version >= 6 path) ──────────────
    // Parse the `struct iwl_alive_ntf_v6` the firmware DMA'd into RB[0]'s
    // payload (data[] begins at RX_PKT_DATA_OFF). Confirms the firmware
    // reported OK status and surfaces the LMAC/UMAC version + error-table
    // pointers + sku_id. The sku_id gates the next stage: an all-zero
    // sku_id means PNVM load is skipped entirely (iwl_pnvm_load). Linux's
    // IMR / debug active-region bookkeeping is debug-only and is deferred.
    fn parse_alive_ntf(&mut self, rb0: &Dma) -> bool {
        let mut p = [0u8; 160]; // len_n_flags(4) + hdr(4) + v6(144) = 152
        host::dma_read_buf(rb0.handle, 0, &mut p);
        let s = RX_PKT_DATA_OFF; // alive struct base
        let rd16 = |o: usize| u16::from_le_bytes([p[s + o], p[s + o + 1]]);
        let rd32 = |o: usize| {
            u32::from_le_bytes([p[s + o], p[s + o + 1], p[s + o + 2], p[s + o + 3]])
        };

        let status = rd16(AL_OFF_STATUS);
        let l = AL_OFF_LMAC0; // lmac_data[0]
        let u = AL_OFF_UMAC;
        let sku = AL_OFF_SKU_ID;

        host::dprint("[ax200] ALIVE status=0x");
        host::dprint_hex16(status);
        host::dprint(if status == IWL_ALIVE_STATUS_OK {
            " (OK)\n"
        } else if status == IWL_ALIVE_STATUS_ERR {
            " (ERR!)\n"
        } else {
            " (unknown)\n"
        });

        host::dprint("[ax200]   LMAC ucode ");
        host::dprint_hex32(rd32(l + LMAC_OFF_UCODE_MAJOR));
        host::dprint(".");
        host::dprint_hex32(rd32(l + LMAC_OFF_UCODE_MINOR));
        host::dprint(" ver_type=0x");
        host::dprint_hex32(p[s + l + LMAC_OFF_VER_TYPE] as u32);
        host::dprint(" subtype=0x");
        host::dprint_hex32(p[s + l + LMAC_OFF_VER_SUBTYPE] as u32);
        host::dprint("\n");

        host::dprint("[ax200]   UMAC ver ");
        host::dprint_hex32(rd32(u + UMAC_OFF_MAJOR));
        host::dprint(".");
        host::dprint_hex32(rd32(u + UMAC_OFF_MINOR));
        host::dprint("\n");

        let umac_err = rd32(u + UMAC_OFF_ERR_INFO) & !FW_ADDR_CACHE_CONTROL;
        // Stash the error-table SRAM pointers for later error-log dumps.
        self.lmac_err_ptr = rd32(l + LMAC_OFF_ERR_TABLE);
        self.umac_err_ptr = umac_err;
        host::dprint("[ax200]   err tables: lmac=0x");
        host::dprint_hex32(rd32(l + LMAC_OFF_ERR_TABLE));
        host::dprint(" umac=0x");
        host::dprint_hex32(umac_err);
        host::dprint("\n");

        let sku0 = rd32(sku);
        let sku1 = rd32(sku + 4);
        let sku2 = rd32(sku + 8);
        host::dprint("[ax200]   sku_id: 0x");
        host::dprint_hex32(sku0);
        host::dprint(" 0x");
        host::dprint_hex32(sku1);
        host::dprint(" 0x");
        host::dprint_hex32(sku2);
        host::dprint(if sku0 == 0 && sku1 == 0 && sku2 == 0 {
            " (empty → PNVM skipped)\n"
        } else {
            " (PNVM required)\n"
        });

        status == IWL_ALIVE_STATUS_OK
    }

    // ── iwl_pcie_gen2_enqueue_hcmd (tx-gen2.c) ──────────────────
    // Enqueue a host command. The iwl_cmd_header_wide (8 B) + payload is laid
    // out across one or two TBs exactly as the Linux enqueue does:
    //   - The first IWL_FIRST_TB_SIZE (20) bytes of the command (header + the
    //     leading payload bytes) always go into the per-slot first-TB staging
    //     buffer as TB0 (it is the bidirectional-DMA buffer the HW writes back).
    //   - If the command is larger, the remaining payload is mapped as TB1
    //     pointing into the cmd_data buffer (this is the IWL_HCMD_DFL_NOCOPY
    //     path large commands like SCAN_REQ_UMAC use; the byte-count table is
    //     never touched by enqueue_hcmd). The command queue is DQA queue 0;
    //     seq = QUEUE_TO_SEQ(0) | INDEX_TO_SEQ(write_ptr).
    fn send_hcmd(&mut self, group: u8, opcode: u8, payload: &[u8]) {
        // iwl_trans_send_cmd (iwl-trans.c): with wide_cmd_header (always true on
        // gen2, where every command carries the long header), a legacy command
        // with group 0 is promoted to LONG_GROUP via DEF_ID(opcode) = (1<<8) |
        // opcode. The firmware registers these "legacy" commands (TX_ANT 0x98,
        // BT 0x9b, POWER 0x77, MCC 0xc8, MAC_CONTEXT 0x28, …) ONLY under group 1
        // — sending them as group 0 yields a BAD_COMMAND assert. (REPLY_ERROR is
        // the lone exception in Linux; we never send it.)
        let group = if group == 0 { IWL_ALWAYS_LONG_GROUP } else { group };
        let wp = self.cmd_write_ptr;
        let idx = (wp & (IWL_CMD_QUEUE_SIZE as u32 - 1)) as usize;
        let total = CMD_HDR_WIDE_LEN + payload.len();

        // first-TB staging: wide header (8 B) + up to FIRST_TB_HEAD_MAX (12)
        // payload bytes, capped at IWL_FIRST_TB_SIZE (20).
        let head = payload.len().min(FIRST_TB_HEAD_MAX);
        let tb0_len = CMD_HDR_WIDE_LEN + head;
        let mut ftb = [0u8; IWL_FIRST_TB_SIZE];
        ftb[HDRW_OFF_CMD] = opcode;
        ftb[HDRW_OFF_GROUP] = group;
        ftb[HDRW_OFF_SEQ..HDRW_OFF_SEQ + 2].copy_from_slice(&(wp as u16).to_le_bytes());
        ftb[HDRW_OFF_LEN..HDRW_OFF_LEN + 2]
            .copy_from_slice(&(payload.len() as u16).to_le_bytes());
        // reserved (6) + version (7) stay 0.
        ftb[CMD_HDR_WIDE_LEN..tb0_len].copy_from_slice(&payload[..head]);
        let ftb_off = (idx * IWL_FIRST_TB_SIZE_ALIGN) as u32;
        host::dma_write_buf(self.cmd_first_tb.handle, ftb_off, &ftb[..tb0_len]);
        let tb0_phys = self.cmd_first_tb.phys + (idx * IWL_FIRST_TB_SIZE_ALIGN) as u64;

        // Build the TFD: TB0 = staging buffer; TB1 (if any) = the remaining
        // payload mapped from cmd_data. iwl_tfh_tfd: num_tbs @0, then 10-byte
        // TBs {tb_len __le16, addr __le64}.
        let mut tfd = [0u8; TFH_TFD_SIZE];
        put_tfh_tb(&mut tfd, 0, tb0_len as u16, tb0_phys);
        let num_tbs = if total > IWL_FIRST_TB_SIZE {
            // remaining payload → cmd_data, mapped as TB1 (NOCOPY semantics).
            host::dma_write_buf(self.cmd_data.handle, 0, payload);
            let rest = payload.len() - head;
            put_tfh_tb(&mut tfd, 1, rest as u16, self.cmd_data.phys + head as u64);
            2u16
        } else {
            1u16
        };
        tfd[0..2].copy_from_slice(&num_tbs.to_le_bytes());
        let tfd_off = (idx * TFH_TFD_SIZE) as u32;
        host::dma_write_buf(self.cmd_tfd.handle, tfd_off, &tfd);
        host::fence();

        // iwl_txq_inc_wrap then iwl_txq_inc_wr_ptr: bump write_ptr (wrap at 256)
        // and ring the doorbell with the new write_ptr | (queue_id << 16).
        self.cmd_write_ptr = (wp + 1) & (MAX_TFD_QUEUE_SIZE - 1);
        self.w32(HBUS_TARG_WRPTR, self.cmd_write_ptr | (IWL_CMD_QUEUE_ID << 16));

        host::dprint("[ax200] hcmd sent: group=0x");
        host::dprint_hex32(group as u32);
        host::dprint(" cmd=0x");
        host::dprint_hex32(opcode as u32);
        host::dprint("\n");
    }

    // Drain newly-closed RBs from the used-BD ring, looking for a frame with the
    // given (cmd, group). Returns the matching RB on success. Mirrors the read-
    // pointer walk of iwl_pcie_rx_handle (mq path): r = closed_rb_num, walk
    // used_bd[read..r], vid → rb_pool[vid-1]. No RB recycling — 64 posted RBs
    // are plenty for the handful of init/NVM frames.
    fn drain_rx_until(&mut self, want_cmd: u8, want_group: u8) -> Option<Dma> {
        host::fence();
        let r = self.closed_rb();
        while self.rxq_read != r {
            let i = self.rxq_read as usize;
            let vid = host::dma_r32(self.rxq_used_bd.handle, (i * 4) as u32) & RX_VID_MASK;
            let mut matched = None;
            if let Some(rb) = self.claim_rb(vid) {
                let mut hdr = [0u8; 8];
                host::dma_read_buf(rb.handle, 0, &mut hdr);
                let cmd = hdr[4];
                let grp = hdr[5];
                host::dprint("[ax200]   RX cmd=0x");
                host::dprint_hex32(cmd as u32);
                host::dprint(" group=0x");
                host::dprint_hex32(grp as u32);
                host::dprint("\n");
                if cmd == want_cmd && grp == want_group {
                    matched = Some(rb);
                } else {
                    // Recycle non-matched RBs (notifications, echoes — the bulk)
                    // back into the free-BD ring. The driver is now resident, so
                    // the 64-RB pool must be replenished or the firmware runs dry
                    // and can post no further frames (TX completions, beacons).
                    // The matched RB is returned to the caller to read, so it is
                    // NOT recycled here (that would race the firmware writing it).
                    self.recycle_rb(vid);
                    self.flush_free_bd();
                }
            }
            self.rxq_read = (self.rxq_read + 1) & (NUM_RBDS as u32 - 1);
            if matched.is_some() {
                return matched;
            }
        }
        None
    }

    // Poll the RX queue for up to `ms` milliseconds for a (cmd, group) frame.
    fn wait_rx(&mut self, want_cmd: u8, want_group: u8, ms: u32) -> Option<Dma> {
        for _ in 0..ms {
            if let Some(rb) = self.drain_rx_until(want_cmd, want_group) {
                return Some(rb);
            }
            host::sleep_ms(1);
        }
        None
    }

    // ── iwl_run_unified_mvm_ucode post-alive init flow (mvm/fw.c) ──
    // For unified ucode (AX200) the flow after ALIVE is: INIT_EXTENDED_CFG_CMD
    // (declares we will send NVM access) → NVM_ACCESS_COMPLETE → wait for
    // INIT_COMPLETE_NOTIF. PNVM load is skipped (sku_id empty), the external
    // NVM file path is skipped (internal NVM), and iwl_send_phy_cfg_cmd is a
    // no-op for unified ucode. So exactly two host commands, then the notif.
    fn run_init_handshake(&mut self) -> bool {
        // INIT_EXTENDED_CFG_CMD { __le32 init_flags = BIT(IWL_INIT_NVM) }
        self.send_hcmd(SYSTEM_GROUP, INIT_EXTENDED_CFG_CMD, &IWL_INIT_NVM_FLAG.to_le_bytes());
        // NVM_ACCESS_COMPLETE { __le32 reserved = 0 }
        self.send_hcmd(REGULATORY_AND_NVM_GROUP, NVM_ACCESS_COMPLETE, &0u32.to_le_bytes());

        host::dprint("[ax200] init cmds sent, waiting for INIT_COMPLETE_NOTIF...\n");
        // INIT_COMPLETE_NOTIF is a legacy-group (0) notification.
        if self.wait_rx(INIT_COMPLETE_NOTIF, 0, 2000).is_some() {
            return true;
        }
        host::dprint("[ax200] INIT_COMPLETE_NOTIF timeout\n");
        false
    }

    // ── iwl_get_nvm (iwl-nvm-parse.c) — read NVM info ──────────────
    // Send NVM_GET_INFO and parse the response: nvm version, reserved-MAC count,
    // MAC SKU caps (bands / 11n / 11ac / 11ax), PHY tx/rx antenna chains, LAR.
    // The MAC address is NOT in this response — it is read from the CSR strap/OTP
    // registers (iwl_set_hw_address_from_csr). The channel profile in the
    // response feeds the scan channel list (Stage 4d).
    fn read_nvm(&mut self) -> bool {
        self.send_hcmd(REGULATORY_AND_NVM_GROUP, NVM_GET_INFO, &0u32.to_le_bytes());
        host::dprint("[ax200] NVM_GET_INFO sent, waiting for response...\n");
        let rb = match self.wait_rx(NVM_GET_INFO, REGULATORY_AND_NVM_GROUP, 2000) {
            Some(rb) => rb,
            None => {
                host::dprint("[ax200] NVM_GET_INFO timeout\n");
                return false;
            }
        };

        // Cover the header fields plus the regulatory channel_profile
        // (__le32[51] at payload offset 28 → absolute 8+28 = 36, ending at 240).
        let mut p = [0u8; 256];
        host::dma_read_buf(rb.handle, 0, &mut p);
        let lnf = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        let payload_len = (lnf & FH_FRAME_SIZE_MASK).wrapping_sub(4); // frame - hdr(4)
        let b = RX_PKT_DATA_OFF;
        let rd16 = |o: usize| u16::from_le_bytes([p[b + o], p[b + o + 1]]);
        let rd32 = |o: usize| {
            u32::from_le_bytes([p[b + o], p[b + o + 1], p[b + o + 2], p[b + o + 3]])
        };

        let mac_sku = rd32(NVM_OFF_MAC_SKU);
        host::dprint("[ax200]   NVM rsp_len=");
        host::dprint_hex32(payload_len);
        host::dprint(" version=0x");
        host::dprint_hex16(rd16(NVM_OFF_VERSION));
        host::dprint(" n_hw_addrs=");
        host::dprint_hex32(p[b + NVM_OFF_N_HW_ADDRS] as u32);
        host::dprint("\n");

        host::dprint("[ax200]   bands:");
        if mac_sku & NVM_SKU_BAND_24 != 0 { host::dprint(" 2.4G"); }
        if mac_sku & NVM_SKU_BAND_52 != 0 { host::dprint(" 5G"); }
        if mac_sku & NVM_SKU_11N != 0 { host::dprint(" 11n"); }
        if mac_sku & NVM_SKU_11AC != 0 { host::dprint(" 11ac"); }
        if mac_sku & NVM_SKU_11AX != 0 { host::dprint(" 11ax"); }
        host::dprint(" | tx_chains=0x");
        host::dprint_hex32(rd32(NVM_OFF_TX_CHAINS));
        host::dprint(" rx_chains=0x");
        host::dprint_hex32(rd32(NVM_OFF_RX_CHAINS));
        host::dprint(" lar=0x");
        host::dprint_hex32(rd32(NVM_OFF_LAR));
        host::dprint("\n");

        self.read_mac_address();

        // ── Build the scan channel list from the regulatory section ──
        // iwl_init_channel_map (iwl-nvm-parse.c): walk iwl_ext_nvm_channels,
        // read the per-channel __le32 flags from channel_profile, keep the
        // NVM_CHANNEL_VALID channels. 5 GHz channels are gated on the 5.2 band
        // SKU bit. Each channel's PHY band rides in v2.band in the scan command.
        let band_52 = mac_sku & NVM_SKU_BAND_52 != 0;
        let prof = b + NVM_OFF_CHANNEL_PROFILE;
        self.n_scan_chans = 0;
        for idx in 0..NVM_EXT_NUM_CHANNELS {
            let flags = u32::from_le_bytes([
                p[prof + idx * 4],
                p[prof + idx * 4 + 1],
                p[prof + idx * 4 + 2],
                p[prof + idx * 4 + 3],
            ]);
            if flags & NVM_CHANNEL_VALID == 0 {
                continue;
            }
            let is_5ghz = idx >= NVM_NUM_2GHZ;
            if is_5ghz && !band_52 {
                continue;
            }
            let band = if is_5ghz { PHY_BAND_5 } else { PHY_BAND_24 };
            if self.n_scan_chans < SCAN_MAX_CHANS {
                self.scan_chans[self.n_scan_chans] = IWL_EXT_NVM_CHANNELS[idx];
                self.scan_bands[self.n_scan_chans] = band as u8;
                self.n_scan_chans += 1;
            }
        }
        host::dprint("[ax200]   scan channels: ");
        host::dprint_dec(self.n_scan_chans as u32);
        host::dprint(" valid (2.4 + 5 GHz)\n");
        true
    }

    // Drain (and log) whatever the firmware has closed in the RX ring over the
    // next `ms` milliseconds, without matching anything. Used after fire-and-
    // forget commands that produce no response of their own but may surface
    // unrelated notifications. An impossible (cmd, group) makes drain_rx_until
    // log+advance every closed RB and return None.
    fn pump_rx(&mut self, ms: u32) {
        for _ in 0..ms {
            self.drain_rx_until(0xFF, 0xFF);
            host::sleep_ms(1);
        }
    }

    // ── Scan prerequisites from iwl_mvm_up (mvm/fw.c) ──────────────
    // The hard pre-scan config commands. These are fire-and-forget config
    // commands — unlike the init-phase commands they do NOT echo a response, so
    // we just send them and pump the RX ring briefly for diagnostics. (The many
    // best-effort / BIOS-gated commands in iwl_mvm_up — SAR, PPAG, TAS, RFI, BT
    // coex tuning, power, RSS, SF — are deferred like op_mode_nic_config; they
    // aren't needed for a scan to return APs.) Real validation is the scan.
    // The complete mandatory iwl_mvm_up command sequence between ALIVE and the
    // scan, in order — no cherry-picking. Faithful omissions: configure_rxq and
    // rss_cfg are no-ops for a single RX queue (both `return 0` when num_rxqs==1,
    // and ours is 1); the BIOS/ACPI-gated commands (lari_cfg, ppag_init,
    // sar_init, sgom_init, tas_init) send nothing without platform tables, just
    // as Linux on a machine that lacks them (ppag: !approved→0, sgom: !enabled→0,
    // sar: post-config_scan anyway); the remaining post-config_scan tuning is not
    // a scan prerequisite. Best-effort, non-fatal calls (shared_mem_conf,
    // sf_update, tt_tx_backoff, config_ltr) are also skipped — Linux itself
    // continues when they fail. Everything that gates with `goto error` and
    // actually emits a command for our config is here.
    fn run_scan_prereqs(&mut self) {
        // TX_ANT_CONFIGURATION_CMD (legacy group 0): valid tx antennas.
        self.send_hcmd(0, TX_ANT_CONFIGURATION_CMD, &ANT_AB.to_le_bytes());
        self.pump_rx(50);

        self.send_bt_init();      // iwl_mvm_send_bt_init_conf
        self.send_soc_latency();  // iwl_set_soc_latency (SOC_LATENCY_SUPPORT cap)
        self.send_dqa();          // iwl_mvm_send_dqa_cmd (DQA_SUPPORT cap)
        self.send_power();        // iwl_mvm_power_update_device

        // iwl_mvm_init_mcc: set the regulatory domain. With LAR enabled the FW
        // blocks scans until this is done (iwl_mvm_up does it before config_scan).
        self.set_regulatory();

        // iwl_mvm_config_scan → SCAN_CFG_CMD v5 (LONG_GROUP): reduced config —
        // tx/rx antenna chains. bcast_sta_id stays 0 (v5 firmware ignores it).
        let mut cfg = [0u8; SCAN_CFG_LEN];
        cfg[SCAN_CFG_OFF_TX_CHAINS..SCAN_CFG_OFF_TX_CHAINS + 4]
            .copy_from_slice(&ANT_AB.to_le_bytes());
        cfg[SCAN_CFG_OFF_RX_CHAINS..SCAN_CFG_OFF_RX_CHAINS + 4]
            .copy_from_slice(&ANT_AB.to_le_bytes());
        self.send_hcmd(IWL_ALWAYS_LONG_GROUP, SCAN_CFG_CMD, &cfg);
        self.pump_rx(50);
    }

    // ── iwl_mvm_send_bt_init_conf (mvm/coex.c) ────────────────────
    // BT coex config (combo chip shares the antenna). mode = network coex;
    // enabled_modules = SYNC2SCO (IWL_MVM_BT_COEX_SYNC2SCO=1, always) | MPLUT
    // (only if the BT_MPLUT_SUPPORT capability is present) | HIGH_BAND_RET.
    fn send_bt_init(&mut self) {
        // BT_COEX_DISABLE unless `btcoex:` in sys/config/wifi says otherwise — the
        // same escape hatch Linux exposes as iwlwifi.bt_coex_active=0.
        //
        // The AX200 is a combo chip: WiFi hangs off PCIe, its Bluetooth off USB
        // (8086:2723 + 8087:0029), and the two share the antenna through this
        // coexistence logic. We implement no Bluetooth at all, so nothing here
        // ever tells coex that BT is idle — and arbitrating an antenna on behalf
        // of a radio that was never brought up can only cost airtime.
        let on = self.want_bt_coex;
        let mut modules = 0u32;
        if on {
            modules = BT_COEX_SYNC2SCO_ENABLED | BT_COEX_HIGH_BAND_RET;
            if fw_has_capa(CAPA_BT_MPLUT_SUPPORT) {
                modules |= BT_COEX_MPLUT_ENABLED;
            }
        }
        let mut cmd = [0u8; BT_COEX_CMD_LEN];
        put_u32(&mut cmd, 0, if on { BT_COEX_NW } else { BT_COEX_DISABLE });
        put_u32(&mut cmd, 4, modules);
        self.send_hcmd(0, BT_CONFIG, &cmd);
        self.pump_rx(20);
        host::print("[ax200] BT coex: ");
        host::print(if on { "network mode (btcoex: on)" } else { "DISABLED, antenna is ours" });
        host::print("\n");
    }

    // ── iwl_set_soc_latency (fw/init.c) ───────────────────────────
    // SOC config. AX200 is a discrete card (mac_cfg.integrated unset) → flags =
    // DISCRETE, latency = xtal_latency (0). Sent only if the firmware advertises
    // SOC_LATENCY_SUPPORT (the gate in iwl_mvm_up).
    fn send_soc_latency(&mut self) {
        if !fw_has_capa(CAPA_SOC_LATENCY_SUPPORT) {
            return;
        }
        let mut cmd = [0u8; SOC_CONFIG_CMD_LEN];
        put_u32(&mut cmd, 0, SOC_CONFIG_CMD_FLAGS_DISCRETE);
        // latency @ 4 = 0 (AX200 mac_cfg.xtal_latency)
        self.send_hcmd(SYSTEM_GROUP, SOC_CONFIGURATION_CMD, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_mvm_send_dqa_cmd (mvm/fw.c) ───────────────────────────
    // Enable dynamic queue allocation. cmd_queue = IWL_MVM_DQA_CMD_QUEUE (0).
    // Sent only if the firmware advertises DQA_SUPPORT (the gate in iwl_mvm_up).
    fn send_dqa(&mut self) {
        if !fw_has_capa(CAPA_DQA_SUPPORT) {
            return;
        }
        let mut cmd = [0u8; DQA_ENABLE_CMD_LEN];
        put_u32(&mut cmd, 0, IWL_CMD_QUEUE_ID);
        self.send_hcmd(DATA_PATH_GROUP, DQA_ENABLE_CMD, &cmd);
        self.pump_rx(20);
    }

    // ── iwl_mvm_power_update_device (mvm/power.c) ─────────────────
    // Device power table. Default power scheme (BPS): POWER_SAVE_ENA set, like
    // Linux's default. CAM (flags = 0) was tried in 0.37 to kill the latency
    // sawtooth but regressed connectivity (radio always-on → broadcast flood
    // pins the driver core → freeze), so it is reverted. The latency spikes are
    // most likely fiber-starvation, not power-save — the WiFi-IRQ is the real fix.
    fn send_power(&mut self) {
        // CAM (flags = 0, radio always on) unless `ps:` in sys/config/wifi says
        // otherwise — iwl_mvm_power_update_device with ps_disabled.
        //
        // We implement no dynamic power save: nothing here tracks DTIM wake
        // windows or tells the AP when we are awake. Enabling device power save
        // on top of that lets the firmware sleep between beacons on timing we
        // never verified — and since 0.44.0 started sending is_assoc=1 with a
        // DTIM period, it finally has the information to actually do it. A
        // station that sleeps at the wrong moment does not look asleep; it looks
        // associated and deaf, which is exactly the symptom being chased.
        let ps = self.want_power_save;
        let mut cmd = [0u8; DEVICE_POWER_CMD_LEN];
        put_u16(&mut cmd, 0, if ps { DEVICE_POWER_FLAGS_POWER_SAVE_ENA } else { 0 });
        self.send_hcmd(0, POWER_TABLE_CMD, &cmd);
        self.pump_rx(20);
        host::print("[ax200] device power: ");
        host::print(if ps { "power-save enabled (ps: on)" } else { "CAM, radio always on" });
        host::print("\n");
    }

    // ── iwl_mvm_init_mcc → iwl_mvm_update_mcc (mvm/nvm.c) ──────────
    // Set the firmware regulatory domain. With LAR enabled the firmware refuses
    // to scan until the regdomain is set ("Disallow scans that might crash the
    // FW while the LAR regdomain is not set"). The first update queries the FW's
    // own default — alpha2 "ZZ", source GET_CURRENT — and the FW replies with
    // its chosen MCC + channel profile (CMD_WANT_SKB), after which scans run.
    fn set_regulatory(&mut self) {
        let mut cmd = [0u8; MCC_UPDATE_CMD_LEN];
        put_u16(&mut cmd, MCC_OFF_MCC, MCC_ALPHA2_ZZ);
        cmd[MCC_OFF_SOURCE] = MCC_SOURCE_GET_CURRENT;
        self.send_hcmd(0, MCC_UPDATE_CMD, &cmd);
        host::dprint("[ax200] MCC_UPDATE_CMD (ZZ / get-current) sent, waiting...\n");

        // The command is promoted to LONG_GROUP (1) in send_hcmd, so its
        // WANT_SKB response echoes group 1 (cf. NVM_GET_INFO echoing its group).
        match self.wait_rx(MCC_UPDATE_CMD, IWL_ALWAYS_LONG_GROUP, 2000) {
            Some(rb) => {
                let mut p = [0u8; 40];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                let rd32 = |o: usize| {
                    u32::from_le_bytes([p[b + o], p[b + o + 1], p[b + o + 2], p[b + o + 3]])
                };
                let mcc = u16::from_le_bytes([p[b + MCC_RESP_OFF_MCC], p[b + MCC_RESP_OFF_MCC + 1]]);
                let cc = [(mcc >> 8) as u8, mcc as u8];
                host::dprint("[ax200]   MCC set to '");
                host::dprint(unsafe { core::str::from_utf8_unchecked(&cc) });
                host::dprint("' status=0x");
                host::dprint_hex32(rd32(MCC_RESP_OFF_STATUS));
                host::dprint(" n_channels=0x");
                host::dprint_hex32(rd32(MCC_RESP_OFF_N_CHANNELS));
                host::dprint("\n");
            }
            None => {
                host::dprint("[ax200]   MCC: no response (scan may stay blocked)\n");
                // No response to a CMD_WANT_SKB command is a strong sign the FW
                // asserted on an earlier command — dump its error log.
                self.dump_fw_error_log();
            }
        }
    }

    // ── iwl_mvm_mac_ctxt_add → iwl_mvm_mac_ctxt_cmd_sta (mvm/mac-ctxt.c) ──
    // Add the firmware MAC context the scan references. mac80211 creates this
    // at add_interface; our driver-initiated scan must add it first or the
    // firmware silently drops the scan (scan_start_mac_or_link_id points at a
    // non-existent context). We model a single unassociated STATION vif:
    // iwl_mvm_mac_ctxt_init assigns the first non-p2p station id 0 / color 0 /
    // TSF A. node_addr is our own MAC (CSR strap, OTP fallback); bssid is
    // broadcast (no BSS yet). is_assoc = 0 makes the firmware forward foreign
    // beacons (MAC_FILTER_IN_BEACON). cck/ofdm_rates are the default mandatory
    // ACK bitmaps iwl_mvm_ack_rates yields for an empty BSSBasicRateSet.
    // protection_flags / qos_flags / ac[] stay 0: a passive scan transmits
    // nothing, so the per-AC EDCA params (populated by mac80211's conf_tx
    // before any real TX) are unused here. Sent fire-and-forget like the other
    // config commands (CMD_SYNC reclaim, no RX notification of its own).
    fn add_mac_context(&mut self) {
        let mut cmd = [0u8; MAC_CTX_CMD_LEN];
        put_u32(&mut cmd, MC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0, 0)
        put_u32(&mut cmd, MC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut cmd, MC_OFF_MAC_TYPE, FW_MAC_TYPE_BSS_STA);
        put_u32(&mut cmd, MC_OFF_TSF_ID, 0); // TSF_ID_A

        let mut mac =
            mac_from_regs(self.r32(CSR_MAC_ADDR0_STRAP), self.r32(CSR_MAC_ADDR1_STRAP));
        if !is_valid_mac(&mac) {
            mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_OTP), self.r32(CSR_MAC_ADDR1_OTP));
        }
        cmd[MC_OFF_NODE_ADDR..MC_OFF_NODE_ADDR + 6].copy_from_slice(&mac);
        for b in &mut cmd[MC_OFF_BSSID_ADDR..MC_OFF_BSSID_ADDR + 6] {
            *b = 0xFF; // eth_broadcast_addr (no bssid_override, no bss_conf.bssid)
        }

        put_u32(&mut cmd, MC_OFF_CCK_RATES, MAC_CCK_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_OFDM_RATES, MAC_OFDM_RATES_DEFAULT);
        // protection_flags / cck_short_preamble / short_slot / qos_flags = 0.
        put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP | MAC_FILTER_IN_BEACON);
        // union iwl_mac_data_sta: is_assoc = 0 and all timing fields = 0.

        self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);
        host::dprint("[ax200] MAC_CONTEXT_CMD (add station ctx id 0) sent\n");
        self.pump_rx(50);
    }

    // ── iwl_set_hw_address_from_csr / iwl_flip_hw_address ──────────
    // Read the 6-byte MAC from the STRAP registers; if the result isn't a valid
    // unicast address, fall back to the OTP registers. Store it for netdev
    // registration and log it.
    fn read_mac_address(&mut self) {
        let mut mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_STRAP), self.r32(CSR_MAC_ADDR1_STRAP));
        if !is_valid_mac(&mac) {
            mac = mac_from_regs(self.r32(CSR_MAC_ADDR0_OTP), self.r32(CSR_MAC_ADDR1_OTP));
        }
        self.mac = mac;
        host::dprint("[ax200]   MAC address: ");
        Self::print_mac(&mac);
        host::dprint("\n");
    }

    fn print_mac(mac: &[u8; 6]) {
        for i in 0..6 {
            if i != 0 {
                host::dprint(":");
            }
            host::dprint_hex8(mac[i]);
        }
    }

    /// iwl_is_rfkill_set (pcie/gen1_2/internal.h): the bit is CLEAR when the
    /// radio is killed. Linux polls this and reports it up; we never showed it
    /// at all, so an off switch looked exactly like a broken receive path.
    fn rf_killed(&self) -> bool {
        self.r32(CSR_GP_CNTRL) & CSR_GP_CNTRL_REG_FLAG_HW_RF_KILL_SW == 0
    }

    /// iwl_get_closed_rb_stts plus the mask Linux applies right after it
    /// (rx.c: `r &= (rxq->queue_size - 1)`, the 9000-A0 wrap-around W/A).
    /// `RB_STTS_CLOSED_MASK` is 12 bits = 0..4095, the ring is 2048 — without
    /// the second mask a closed index above the ring never equals our read
    /// index and the drain loop below never terminates.
    fn closed_rb(&self) -> u32 {
        (host::dma_r32(self.rxq_rb_stts.handle, 0) & RB_STTS_CLOSED_MASK)
            & (NUM_RBDS as u32 - 1)
    }

    /// iwl_pcie_get_rxb: take a returned buffer out of the firmware's hands.
    /// A vid outside the pool, or one we never posted, means the firmware and
    /// this ring disagree about who owns the page. Linux answers that with
    /// `iwl_force_nmi()` and a firmware restart; we count it and drop the
    /// entry, because handing the same page out twice is worse than losing it.
    fn claim_rb(&mut self, vid: u32) -> Option<Dma> {
        if vid == 0 || vid as usize > RX_NUM_RBS {
            self.st.rb_bad_vid = self.st.rb_bad_vid.wrapping_add(1);
            return None;
        }
        let i = vid as usize - 1;
        if !self.rb_in_fw[i] {
            self.st.rb_bad_vid = self.st.rb_bad_vid.wrapping_add(1);
            return None;
        }
        self.rb_in_fw[i] = false;
        Some(self.rb_pool[i])
    }

    // ── iwl_pcie_rxmq_restock — recycle one consumed RB ────────────
    // Re-post the RB identified by `vid` into the free-BD ring at the next write
    // slot so the firmware can fill it again. The page is the same; only its
    // ring position changes (bd[slot] = page_dma | vid, gen2 < AX210).
    fn recycle_rb(&mut self, vid: u32) {
        let i = vid as usize - 1;
        if self.rb_in_fw[i] {
            // The same page twice in the ring, and a write index advanced past
            // what we can back with buffers. Linux cannot even express this —
            // restock pulls from rx_free, and a posted buffer is not on it.
            self.st.rb_double_post = self.st.rb_double_post.wrapping_add(1);
            return;
        }
        let slot = (self.free_bd_write & (NUM_RBDS as u32 - 1)) as usize;
        let rb = self.rb_pool[i];
        let entry = rb.phys | vid as u64;
        host::dma_write_buf(self.rxq_bd.handle, (slot * 8) as u32, &entry.to_le_bytes());
        self.rb_in_fw[i] = true;
        self.free_bd_write += 1;
    }

    /// Hand back every RB the firmware does NOT currently own, and publish the
    /// index. Returns how many were posted.
    ///
    /// Recovery for RX going quiet while associated: buffers can be stranded on
    /// our side (a matched RB read during bring-up is never recycled), and the
    /// write pointer is published rounded down to 8, so a tail of fewer than 8
    /// stays invisible to the firmware.
    ///
    /// It used to re-post the whole pool unconditionally. That is a producer
    /// that ignores its consumer: every fire advanced the write index by
    /// RX_NUM_RBS whether or not the firmware had consumed anything, so a
    /// firmware that had stopped for its own reasons got the ring walked all
    /// the way round onto its own read index — published as EMPTY, and then it
    /// could never recover. Linux has no such path at all: a wedged RX path
    /// gets `iwl_force_nmi()` and a firmware restart, never a ring poke.
    ///
    /// A return of 0 is the useful answer: the firmware still holds every
    /// buffer, so an empty pool was never the cause and re-arming is not a cure.
    fn restock_all_rbs(&mut self) -> u32 {
        let mut armed = 0u32;
        for vid in 1..=RX_NUM_RBS as u32 {
            if !self.rb_in_fw[vid as usize - 1] {
                self.recycle_rb(vid);
                armed += 1;
            }
        }
        if armed > 0 {
            self.flush_free_bd();
        }
        armed
    }

    // Push the recycled free-BD write index to the HW (round down to 8).
    fn flush_free_bd(&self) {
        host::fence();
        // Mask into the ring before rounding: free_bd_write counts monotonically
        // (recycle_rb masks only for the slot it writes), so past NUM_RBDS this
        // handed the hardware an index outside its own ring.
        let idx = self.free_bd_write & (NUM_RBDS as u32 - 1);
        self.w32(RFH_Q0_FRBDCB_WIDX_TRG, idx & !0x7);
    }

    // ── iwl_mvm_scan_umac_v14_and_above (mvm/scan.c, version 15) ────
    // Build a passive regular scan over the 2.4 GHz channels (1..13) and send it
    // as SCAN_REQ_UMAC. Passive (n_ssids = 0 → FORCE_PASSIVE) means no probe
    // request is transmitted, so probe_params stays zeroed; PASS_ALL makes the
    // firmware forward every beacon to the host. All general/channel parameters
    // are filled exactly as the Linux fill helpers do (dwell 10/110, adwell
    // 2/8/10, budget 300, EXT_6 priority, UNASSOC timing = 0, adaptive dwell).
    fn build_scan_cmd(&self, buf: &mut [u8]) {
        // uid @ 0 = 0; ooc_priority + scan_priority = IWL_SCAN_PRIORITY_EXT_6.
        put_u32(buf, SC_OFF_OOC_PRIORITY, SCAN_OOC_PRIORITY_REGULAR);

        // general_params_v11
        put_u16(buf, SC_OFF_GP_FLAGS, SCAN_GP_FLAGS_PASSIVE);
        // scan_start_mac_or_link_id = scan_vif->id (version < 16). Names the FW
        // MAC context added in add_mac_context(); 0 here, but set explicitly.
        buf[SC_OFF_GP_SCAN_START_MAC] = SCAN_VIF_MAC_ID;
        buf[SC_OFF_GP_ACTIVE_DWELL] = IWL_SCAN_DWELL_ACTIVE; // LB
        buf[SC_OFF_GP_ACTIVE_DWELL + 1] = IWL_SCAN_DWELL_ACTIVE; // HB
        buf[SC_OFF_GP_ADWELL_2G] = ADWELL_DEFAULT_LB_N_APS;
        buf[SC_OFF_GP_ADWELL_5G] = ADWELL_DEFAULT_HB_N_APS;
        buf[SC_OFF_GP_ADWELL_SOCIAL] = ADWELL_DEFAULT_N_APS_SOCIAL;
        // flags2 @ 17 = 0
        put_u16(buf, SC_OFF_GP_ADWELL_BUDGET, ADWELL_MAX_BUDGET_FULL);
        // max_out_of_time / suspend_time = 0 (UNASSOC timing)
        put_u32(buf, SC_OFF_GP_SCAN_PRIO, SCAN_OOC_PRIORITY_REGULAR);
        buf[SC_OFF_GP_PASSIVE_DWELL] = IWL_SCAN_DWELL_PASSIVE; // LB
        buf[SC_OFF_GP_PASSIVE_DWELL + 1] = IWL_SCAN_DWELL_PASSIVE; // HB
        // num_of_fragments = 0

        // channel_params_v7 — the NVM_CHANNEL_VALID channels from read_nvm,
        // both bands. Per channel, the band rides in the v2.band BYTE (@ +5);
        // see the band-encoding note below.
        buf[SC_OFF_CP_FLAGS] = SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER;
        buf[SC_OFF_CP_COUNT] = self.n_scan_chans as u8;
        buf[SC_OFF_CP_N_APS_OVERRIDE] = SCAN_N_APS_GO_FRIENDLY;
        buf[SC_OFF_CP_N_APS_OVERRIDE + 1] = SCAN_N_APS_SOCIAL_CHS;
        for i in 0..self.n_scan_chans {
            let o = SC_OFF_CP_CHANNELS + i * SCAN_CH_CFG_LEN;
            // iwl_mvm_umac_scan_cfg_channels_v7, version < 17 (our cmd_ver is 15):
            // cfg.flags holds the directed-scan SSID bitmap (bits 0-19) — 0 for a
            // passive station scan (no SSID, n_aps_flag only for P2P) — and the
            // band rides in the v2.band BYTE (@ +5), NOT in flags bits 30-31.
            // (The v17 path puts band in flags; doing that for v15 left band=0 =
            // PHY_BAND_5/5GHz on 2.4GHz channels → BAD scan params → FW assert.)
            // cfg.flags @ o stays 0 (zeroed buffer).
            buf[o + 4] = self.scan_chans[i]; // channel_num
            buf[o + 5] = self.scan_bands[i]; // v2.band (1 = 2.4 GHz, 0 = 5 GHz)
            buf[o + 6] = 1; // v2.iter_count
            // v2.iter_interval @ o+7 = 0
        }

        // periodic_params: regular scan = one plan, one iteration.
        buf[SC_OFF_PERIODIC_SCHED0_ITER] = 1;
        // probe_params: zeroed (passive scan, no probe request transmitted).
    }

    // ── iwl_mvm_rx_mpdu_mq (mvm/rxmq.c) — parse a scan beacon ─────
    // A REPLY_RX_MPDU_CMD RB holds: [len_n_flags 4][cmd_hdr 4][iwl_rx_mpdu_desc]
    // [802.11 frame]. For family < AX210 the descriptor is IWL_RX_DESC_SIZE_V1
    // (48), so the frame starts at RX_PKT_DATA_OFF + 48. We extract the BSSID
    // (addr3), the SSID (IE 0), the RSSI (max of the two energy chains, negated
    // to dBm), and the channel, de-duplicating by BSSID. Only beacon / probe-
    // response management frames carry these, so other subtypes are skipped.
    fn parse_beacon(rb: &Dma, aps: &mut [Ap], n_aps: &mut usize) {
        // 384 bytes left 292 for the elements. A Wi-Fi 6 beacon carries RSN,
        // Extended Capabilities, WMM, WPS and the mesh vendor blocks BEFORE
        // VHT Operation (192) and the HE elements (255) — those fell off the
        // end, and the walk below stopped on the cut without a word. 1600 is
        // the buffer the data RX path already uses.
        let mut buf = [0u8; 1600];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let d = RX_PKT_DATA_OFF; // iwl_rx_mpdu_desc base

        // RSSI: iwl_mvm_get_signal_strength — energy is a positive magnitude,
        // negated to dBm; 0 means "no signal" (S8_MIN). Take the stronger chain.
        let to_dbm = |e: u8| if e != 0 { -(e as i16) } else { -128 };
        let rssi = to_dbm(buf[d + MPDU_OFF_ENERGY_A]).max(to_dbm(buf[d + MPDU_OFF_ENERGY_B])) as i8;
        let channel = buf[d + MPDU_OFF_CHANNEL];

        let f = d + IWL_RX_DESC_SIZE_V1; // 802.11 frame
        // frame_control low byte: type bits 2-3 (0 = management), subtype 4-7.
        let fc = buf[f];
        if fc & 0x0c != 0 {
            return; // not a management frame
        }
        let subtype = (fc >> 4) & 0xf;
        if subtype != DOT11_STYPE_BEACON && subtype != DOT11_STYPE_PROBE_RESP {
            return;
        }
        // The frame ends where the descriptor says (`iwl_mvm_rx_mpdu_mq`:
        // len = le16(desc->mpdu_len)); past it lies the next packet in the RB.
        // EVERY element walk below stops here, not at the buffer's end.
        let mpdu_len = u16::from_le_bytes([
            buf[d + MPDU_OFF_MPDU_LEN], buf[d + MPDU_OFF_MPDU_LEN + 1]]) as usize;
        if f + mpdu_len > RB_SIZE_BYTES {
            return; // iwl_mvm_rx_mpdu_mq: "FW lied about packet len"
        }
        let end = if f + mpdu_len > buf.len() {
            // OUR limit, not the firmware's — and the exception gets a line.
            // This is the failure that hid VHT and HE from us, and a silent
            // one is what let "the AP runs 20 MHz" stand for two days.
            host::print("[ax200] beacon ");
            host::print_dec(mpdu_len as u32);
            host::print(" B > parse buffer - elements past the cut are unseen\n");
            buf.len()
        } else {
            f + mpdu_len
        };

        let mut bssid = [0u8; 6];
        bssid.copy_from_slice(&buf[f + DOT11_OFF_ADDR3..f + DOT11_OFF_ADDR3 + 6]);
        // Beacon interval: fixed param after the 24-byte header + 8-byte timestamp
        // (__le16 TU). Needed for the connect MAC context (iwl_mac_data_sta.bi).
        let bi_off = f + DOT11_HDR_LEN + 8;
        let beacon_int = u16::from_le_bytes([buf[bi_off], buf[bi_off + 1]]);
        // Privacy bit of the capability field → AP is encrypted (needs RSN).
        let privacy = buf[f + DOT11_BEACON_CAP_OFF] & WLAN_CAP_PRIVACY_BIT != 0;
        // Timing, from a BEACON only: a probe response answers our probe and its
        // device timestamp says nothing about the AP's beacon schedule.
        let is_beacon = subtype == DOT11_STYPE_BEACON;
        let tsf = if is_beacon {
            let t = f + DOT11_HDR_LEN;
            u64::from_le_bytes([buf[t], buf[t+1], buf[t+2], buf[t+3],
                                buf[t+4], buf[t+5], buf[t+6], buf[t+7]])
        } else { 0 };
        let device_ts = if is_beacon { le32(&buf, d + MPDU_OFF_GP2_ON_AIR) } else { 0 };

        // De-dup by BSSID; refresh RSSI if we hear a stronger beacon, and the
        // timing on EVERY beacon — the freshest one is the one to associate with.
        for i in 0..*n_aps {
            if aps[i].bssid == bssid {
                if rssi > aps[i].rssi {
                    aps[i].rssi = rssi;
                }
                if is_beacon {
                    aps[i].tsf = tsf;
                    aps[i].device_ts = device_ts;
                    aps[i].dtim_count = Self::dtim_count_of(&buf[..end], f);
                    aps[i].has_beacon = true;
                }
                return;
            }
        }

        // Walk the information elements: SSID (0), TIM (5) for the DTIM period,
        // HT Capability (45) for the AP's 802.11n parameters.
        let mut ssid = [0u8; SSID_MAX];
        let mut ssid_len = 0u8;
        let mut dtim_period = 0u8;
        let mut ht = HtCap::NONE;
        let mut p = f + DOT11_OFF_IES;
        while p + 2 <= end {
            let id = buf[p];
            let len = buf[p + 1] as usize;
            if p + 2 + len > end {
                break;
            }
            let body = p + 2;
            match id {
                WLAN_EID_SSID => {
                    let l = len.min(SSID_MAX);
                    ssid[..l].copy_from_slice(&buf[body..body + l]);
                    ssid_len = l as u8;
                }
                // TIM: dtim_count, dtim_period, bitmap_control, virtual bitmap.
                WLAN_EID_TIM if len >= 2 => dtim_period = buf[body + 1],
                WLAN_EID_ERP_INFO if len >= 1 => {
                    ht.erp_protect = buf[body] & WLAN_ERP_USE_PROTECTION != 0;
                }
                // struct ieee80211_ht_cap — see HT_OFF_* in regs.rs.
                WLAN_EID_HT_CAPABILITY if len >= HT_CAP_IE_LEN => {
                    ht.cap_info =
                        u16::from_le_bytes([buf[body + HT_OFF_CAP_INFO], buf[body + HT_OFF_CAP_INFO + 1]]);
                    let ampdu = buf[body + HT_OFF_AMPDU_PARAMS];
                    ht.ampdu_factor = ampdu & IEEE80211_HT_AMPDU_PARM_FACTOR;
                    ht.ampdu_density =
                        (ampdu & IEEE80211_HT_AMPDU_PARM_DENSITY) >> IEEE80211_HT_AMPDU_PARM_DENSITY_SHIFT;
                    ht.mcs_rx[0] = buf[body + HT_OFF_MCS_RX_MASK];
                    ht.mcs_rx[1] = buf[body + HT_OFF_MCS_RX_MASK + 1];
                    ht.present = true;
                }
                // The capability element says the AP CAN do 40 MHz; only the
                // operation element says whether it currently DOES, and on
                // which side the secondary channel sits.
                WLAN_EID_HT_OPERATION if len >= 2 => {
                    ht.sec_chan_offs =
                        buf[body + HT_OP_OFF_HT_PARAM] & IEEE80211_HT_PARAM_CHA_SEC_OFFSET;
                    if len >= 4 {
                        ht.ht_op_mode = u16::from_le_bytes([
                            buf[body + HT_OP_OFF_OPERATION_MODE],
                            buf[body + HT_OP_OFF_OPERATION_MODE + 1]]);
                    }
                }
                WLAN_EID_VHT_CAPABILITY if len >= VHT_CAP_IE_LEN => {
                    ht.vht = true;
                    ht.vht_cap_info = le32(&buf, body + VHT_OFF_CAP_INFO);
                    ht.vht_rx_mcs_map = u16::from_le_bytes([
                        buf[body + VHT_OFF_RX_MCS_MAP], buf[body + VHT_OFF_RX_MCS_MAP + 1]]);
                }
                WLAN_EID_VHT_OPERATION if len >= 3 => {
                    ht.vht_chan_width = buf[body + VHT_OP_OFF_CHAN_WIDTH];
                    ht.vht_seg0 = buf[body + VHT_OP_OFF_SEG0];
                }
                // WMM Parameter element — the AP's EDCA table.
                // `ieee80211_sta_wmm_params` (mac80211/mlme.c): ACI in bits
                // 5-6 selects the AC, AIFSN is the low nibble, the next byte
                // is ECWmin in its low nibble and ECWmax in its high one, then
                // a little-endian TXOP limit.
                WLAN_EID_VENDOR_SPECIFIC
                    if len >= WMM_PARAM_IE_LEN
                        && buf[body..body + 3] == WMM_OUI
                        && buf[body + 3] == WMM_OUI_TYPE
                        && buf[body + 4] == WMM_OUI_SUBTYPE_PARAM =>
                {
                    let ecw2cw = |e: u8| -> u16 { (1u16 << e) - 1 };
                    for k in 0..4 {
                        let r = body + WMM_PARAM_OFF_AC + k * 4;
                        let aci = (buf[r] >> 5) & 0x03;
                        // ACI 0=BE, 1=BK, 2=VI, 3=VO → mac80211 AC index.
                        let ac = match aci {
                            1 => 3usize, // BK
                            2 => 1,      // VI
                            3 => 0,      // VO
                            _ => 2,      // BE
                        };
                        // "AP has invalid WMM params (AIFSN=%d), will use 2"
                        let aifs = (buf[r] & 0x0f).max(2);
                        let cw_min = ecw2cw(buf[r + 1] & 0x0f);
                        let cw_max = ecw2cw((buf[r + 1] & 0xf0) >> 4);
                        let txop = u16::from_le_bytes([buf[r + 2], buf[r + 3]]);
                        ht.edca[ac] = (aifs, cw_min, cw_max, txop);
                    }
                    ht.wmm = true;
                }
                // HE capability / operation live behind the extension ID.
                WLAN_EID_EXTENSION if len >= 1 => match buf[body] {
                    WLAN_EID_EXT_HE_CAPABILITY => ht.he = true,
                    WLAN_EID_EXT_HE_OPERATION if len >= HE_OP_MIN_LEN => {
                        ht.he_oper_params = le32(&buf, body + HE_OP_OFF_PARAMS);
                        if ht.he_oper_params & IEEE80211_HE_OPERATION_VHT_OPER_INFO != 0
                            && len >= HE_OP_OFF_VHT_OPER_INFO + 3
                        {
                            ht.he_vht_op.copy_from_slice(
                                &buf[body + HE_OP_OFF_VHT_OPER_INFO
                                    ..body + HE_OP_OFF_VHT_OPER_INFO + 3],
                            );
                            ht.he_vht_op_present = true;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            p += 2 + len;
        }

        if *n_aps < aps.len() {
            aps[*n_aps] = Ap {
                bssid, ssid, ssid_len, rssi, channel, beacon_int, privacy, dtim_period, ht,
                tsf, device_ts, dtim_count: Self::dtim_count_of(&buf[..end], f),
                has_beacon: is_beacon,
            };
            *n_aps += 1;
        }
    }

    /// The live DTIM count from a frame's TIM element (0 if absent). Read on its
    /// own because the de-dup path refreshes it without re-walking every element.
    fn dtim_count_of(buf: &[u8], f: usize) -> u8 {
        let mut p = f + DOT11_OFF_IES;
        while p + 2 <= buf.len() {
            let len = buf[p + 1] as usize;
            if p + 2 + len > buf.len() { break; }
            if buf[p] == WLAN_EID_TIM && len >= 2 { return buf[p + 2]; }
            p += 2 + len;
        }
        0
    }

    // Print the collected AP list (SSID, BSSID, RSSI, channel).
    fn print_aps(aps: &[Ap], n_aps: usize) {
        host::print("[ax200] scan: ");
        host::print_dec(n_aps as u32);
        host::print(" APs (full list needs DEBUG)\n");
        if !host::DEBUG {
            return;
        }
        for ap in &aps[..n_aps] {
            host::print("[ax200]   ");
            // SSID (printable ASCII; hidden / empty → <hidden>).
            if ap.ssid_len == 0 {
                host::print("<hidden>");
            } else {
                for &b in &ap.ssid[..ap.ssid_len as usize] {
                    let c = if (0x20..0x7f).contains(&b) { b } else { b'.' };
                    host::print(unsafe { core::str::from_utf8_unchecked(core::slice::from_ref(&c)) });
                }
            }
            host::print("  [");
            for i in 0..6 {
                if i != 0 {
                    host::print(":");
                }
                host::print_hex8(ap.bssid[i]);
            }
            host::print("]  ");
            // RSSI in dBm (always negative here).
            host::print("-");
            host::print_dec((-(ap.rssi as i32)) as u32);
            host::print(" dBm  ch ");
            host::print_dec(ap.channel as u32);
            host::print("\n");
        }
    }

    // ── iwl_pcie_rx_handle — service the multi-queue RX ring ──────
    // Drain every RB the firmware has closed since our last read, recycling
    // each one back into the free-BD ring so the firmware never runs dry.
    // `on_frame(cmd, group, &rb)` runs for each frame and returns `false` to
    // stop draining early (a terminal notification). Returns the number of
    // frames seen. Shared by the scan loop and the resident NIC service loop —
    // the one place that walks used_bd → rb_pool → recycle.
    fn service_rx<F: FnMut(u8, u8, &Dma) -> bool>(&mut self, mut on_frame: F) -> u32 {
        host::fence();
        let r = self.closed_rb();
        let mut frames = 0u32;
        let mut recycled = false;
        let mut stop = false;
        while self.rxq_read != r && !stop {
            let i = self.rxq_read as usize;
            let vid = host::dma_r32(self.rxq_used_bd.handle, (i * 4) as u32) & RX_VID_MASK;
            if let Some(rb) = self.claim_rb(vid) {
                let mut hdr = [0u8; 8];
                host::dma_read_buf(rb.handle, 0, &mut hdr);
                frames += 1;
                if !on_frame(hdr[4], hdr[5], &rb) {
                    stop = true;
                }
                self.recycle_rb(vid);
                recycled = true;
            }
            self.rxq_read = (self.rxq_read + 1) & (NUM_RBDS as u32 - 1);
        }
        if recycled {
            self.flush_free_bd();
        }
        frames
    }

    // Send the scan and poll the RX ring (npk_sleep yields — never input_wait)
    // until SCAN_COMPLETE_UMAC arrives, parsing every beacon / probe response
    // into the AP list along the way.
    fn run_scan(&mut self) -> bool {
        let mut buf = [0u8; SCAN_CMD_LEN];
        self.build_scan_cmd(&mut buf);
        self.send_hcmd(IWL_ALWAYS_LONG_GROUP, SCAN_REQ_UMAC, &buf);
        host::dprint("[ax200] SCAN_REQ_UMAC sent (passive, ");
        host::dprint_dec(self.n_scan_chans as u32);
        host::dprint(" channels, 2.4 + 5 GHz), scanning...\n");

        let mut frames = 0u32;
        let mut aps = [Ap::EMPTY; MAX_APS];
        let mut n_aps = 0usize;
        let mut completed = false;
        // ~12 s budget: a passive scan of both bands (up to ~40 channels at
        // 110 ms dwell) takes longer than the 2.4-GHz-only scan did.
        for _ in 0..12000 {
            self.service_rx(|cmd, grp, rb| {
                if cmd == SCAN_COMPLETE_UMAC && grp == 0 {
                    completed = true;
                    return false; // stop draining; the scan is done
                }
                // Beacons / probe responses arrive as REPLY_RX_MPDU_CMD
                // (0xc1, LEGACY_GROUP) — parse them into the AP list.
                if cmd == REPLY_RX_MPDU_CMD && grp == 0 {
                    Self::parse_beacon(rb, &mut aps, &mut n_aps);
                }
                frames += 1;
                true
            });
            if completed {
                host::dprint("[ax200] SCAN_COMPLETE_UMAC received — frames seen: ");
                host::dprint_dec(frames);
                host::dprint("\n");
                Self::print_aps(&aps, n_aps);
                let best = self.pick_target(&aps, n_aps);
                if best != usize::MAX {
                    self.target_bssid = aps[best].bssid;
                    self.target_chan = aps[best].channel;
                    // Band by channel: 1..14 = 2.4 GHz, else 5 GHz (iwl_nvm_channels).
                    self.target_band =
                        if aps[best].channel <= 14 { PHY_BAND_24 as u8 } else { PHY_BAND_5_U8 };
                    self.target_beacon_int = aps[best].beacon_int;
                    self.target_ssid = aps[best].ssid;
                    self.target_ssid_len = aps[best].ssid_len;
                    self.target_privacy = aps[best].privacy;
                    self.target_rssi = aps[best].rssi;
                    self.target_ht = aps[best].ht;
                    self.target_dtim_period = aps[best].dtim_period;
                    // …and the beacon timing that came with it, exactly as
                    // mac80211 reads it back out of the scan result.
                    self.sync_tsf = aps[best].tsf;
                    self.sync_device_ts = aps[best].device_ts;
                    self.sync_dtim_count = aps[best].dtim_count;
                    self.sync_ok = aps[best].has_beacon;
                    self.target_valid = true;
                    // Record what we passed over: the strongest AP carrying the
                    // same SSID on the OTHER band. Choosing by RSSI alone always
                    // lands on the nearest 2.4 GHz node, and without this the
                    // question "was a 5 GHz radio even in range?" is unanswerable
                    // after the scan buffer is gone.
                    self.n_aps = n_aps.min(255) as u8;
                    self.alt_valid = false;
                    let chosen_5g = aps[best].channel > 14;
                    for i in 0..n_aps {
                        if i == best { continue; }
                        if (aps[i].channel > 14) == chosen_5g { continue; }
                        if aps[i].ssid_len != aps[best].ssid_len
                            || aps[i].ssid[..aps[i].ssid_len as usize]
                                != aps[best].ssid[..aps[best].ssid_len as usize]
                        {
                            continue;
                        }
                        if !self.alt_valid || aps[i].rssi > self.alt_rssi {
                            self.alt_bssid = aps[i].bssid;
                            self.alt_chan = aps[i].channel;
                            self.alt_rssi = aps[i].rssi;
                            self.alt_ht = aps[i].ht.present;
                            self.alt_valid = true;
                        }
                    }
                    host::print("[ax200] target: ch ");
                    host::print_dec(self.target_chan as u32);
                    host::print(", dtim_period ");
                    host::print_dec(self.target_dtim_period as u32);
                    if self.target_ht.present {
                        host::print(", HT cap=0x");
                        host::print_hex16(self.target_ht.cap_info);
                        host::print(" mcs=");
                        host::print_hex8(self.target_ht.mcs_rx[1]);
                        host::print_hex8(self.target_ht.mcs_rx[0]);
                        host::print(" ampdu f/d=");
                        host::print_dec(self.target_ht.ampdu_factor as u32);
                        host::print("/");
                        host::print_dec(self.target_ht.ampdu_density as u32);
                        host::print("\n");
                    } else {
                        host::print(", NO HT element — legacy rates only\n");
                    }
                }
                return true;
            }
            host::sleep_ms(1);
        }
        host::dprint("[ax200] SCAN_COMPLETE timeout — frames seen: 0x");
        host::dprint_hex32(frames);
        host::dprint("\n");
        self.dump_fw_error_log();
        false
    }

    /// Stop considering the current target. Used when an association goes
    /// nowhere: re-scanning and picking the same unreachable AP again is not a
    /// retry, it is a loop. Oldest entry is evicted, so a roaming client cannot
    /// blacklist its way out of every AP it has.
    fn blacklist_target(&mut self) {
        if self.blacklist.iter().take(self.n_blacklist).any(|b| *b == self.target_bssid) {
            return;
        }
        if self.n_blacklist < self.blacklist.len() {
            self.blacklist[self.n_blacklist] = self.target_bssid;
            self.n_blacklist += 1;
        } else {
            self.blacklist.rotate_left(1);
            self.blacklist[self.blacklist.len() - 1] = self.target_bssid;
        }
        host::print("[ax200] blacklisted this BSS for the next scan\n");
    }

    // Read the connect policy from `sys/config/wifi` — one file, `key: value`
    // per line. `ssid` is the network wifid holds the PSK for; associating to
    // anything else can only end in a MIC failure.
    fn load_connect_policy(&mut self) {
        let mut cfg = [0u8; WIFI_CFG_MAX];
        let n = host::fetch(WIFI_CFG_PATH, &mut cfg);
        let text = &cfg[..n];

        if let Some(v) = cfg_get(text, b"ssid") {
            let len = v.len().min(SSID_MAX);
            if len > 0 {
                self.want_ssid[..len].copy_from_slice(&v[..len]);
                self.want_ssid_len = len as u8;
            }
        }

        // OFF by default, and it stays that way until a device measurement says
        // otherwise. Aggregation is the throughput lever, but it is also the one
        // feature that can take the whole link down (a receiver that mis-parses
        // an aggregate carries nothing) — and recovering from that needs the
        // network it just broke. Twice now. So the safe state is the default.
        self.want_ampdu = cfg_on(cfg_get(text, b"ampdu"));
        // VHT80 is OFF by default. Measured on the device: receiving at 80 MHz
        // works (292 Mbit), transmitting does not — 41 % retries, more RTS
        // failures than frames sent, and the rate control walking back down to
        // 20 MHz. HT40 measured 81 Mbit/s with a stable link, so that is the
        // safe state until the transmit side is understood.
        // 40 MHz is opt-in until the width path is understood; VHT80 needs it.
        // EXPERIMENT, not a port: Linux leaves tid_disable_tx at 0xffff on this
        // firmware, and the firmware is supposed to run the aggregation manager
        // itself. It does not — `aggregated 0` in three measured runs. `on`
        // sends 0x0000 instead, which is the one thing the field could mean.
        self.want_txagg = cfg_on(cfg_get(text, b"txagg"));
        self.want_ht40 = cfg_on(cfg_get(text, b"ht40"));
        self.want_vht = cfg_on(cfg_get(text, b"vht"));
        self.want_power_save = cfg_on(cfg_get(text, b"ps"));
        self.want_bt_coex = cfg_on(cfg_get(text, b"btcoex"));
        self.settle_ms = cfg_settle_ms(text);

        self.band_pref = match cfg_get(text, b"band") {
            Some(v) if v.starts_with(b"5") => BAND_PREF_5,
            Some(v) if v.starts_with(b"2") => BAND_PREF_24,
            _ => BAND_PREF_AUTO,
        };

        host::print(if fw_has_capa(IWL_UCODE_TLV_CAPA_BAID_ML_SUPPORT) {
            "[ax200] block-ack via RX_BAID_ALLOCATION_CONFIG_CMD (fw has BAID_ML)\n"
        } else {
            "[ax200] block-ack via ADD_STA (fw without BAID_ML)\n"
        });
        host::print("[ax200] connect policy: ssid ");
        if self.want_ssid_len > 0 {
            host::print("\"");
            for &b in &self.want_ssid[..self.want_ssid_len as usize] {
                let s = [if (0x20..0x7f).contains(&b) { b } else { b'?' }];
                host::print(unsafe { core::str::from_utf8_unchecked(&s) });
            }
            host::print("\"");
        } else {
            host::print("(any - no `ssid:` line in sys/config/wifi)");
        }
        host::print(", band ");
        host::print(match self.band_pref {
            BAND_PREF_5 => "5 GHz only",
            BAND_PREF_24 => "2.4 GHz only",
            _ => "auto (prefer 5 GHz when strong enough)",
        });
        host::print(", a-mpdu ");
        host::print(if self.want_ampdu { "on" } else { "off" });
        host::print(", txagg ");
        host::print(if self.want_txagg { "on (EXPERIMENT: tid_disable_tx=0)" } else { "off (Linux: 0xffff)" });
        host::print(", ht40 ");
        host::print(if self.want_ht40 { "on" } else { "off (20 MHz)" });
        host::print("\n");
    }

    // Choose the connect target from the scan results. Returns usize::MAX when
    // nothing qualifies.
    fn pick_target(&mut self, aps: &[Ap], n_aps: usize) -> usize {
        let want = &self.want_ssid[..self.want_ssid_len as usize];
        let bl = &self.blacklist[..self.n_blacklist];
        let matches_ssid = |a: &Ap| -> bool {
            if bl.iter().any(|b| *b == a.bssid) {
                return false;
            }
            want.is_empty() || (a.ssid_len as usize == want.len() && &a.ssid[..want.len()] == want)
        };

        // Strongest AP of our network per band.
        let mut best_24 = usize::MAX;
        let mut best_5 = usize::MAX;
        let mut n_ours = 0usize;
        for i in 0..n_aps {
            if !matches_ssid(&aps[i]) { continue; }
            n_ours += 1;
            let slot = if aps[i].channel > 14 { &mut best_5 } else { &mut best_24 };
            if *slot == usize::MAX || aps[i].rssi > aps[*slot].rssi {
                *slot = i;
            }
        }

        // Nothing with the configured SSID: fall back to the strongest AP of any
        // network rather than refusing to connect, but say so — a 4-way that then
        // fails on the MIC is otherwise a mystery.
        if n_ours == 0 {
            if !want.is_empty() {
                host::print("[ax200] no AP with the configured SSID in range - taking the strongest\n");
            }
            let mut best = usize::MAX;
            for i in 0..n_aps {
                if best == usize::MAX || aps[i].rssi > aps[best].rssi { best = i; }
            }
            self.pick_reason = PICK_SSID_FILTERED;
            return best;
        }

        match self.band_pref {
            BAND_PREF_5 if best_5 != usize::MAX => {
                self.pick_reason = PICK_BAND_FORCED;
                best_5
            }
            BAND_PREF_24 if best_24 != usize::MAX => {
                self.pick_reason = PICK_BAND_FORCED;
                best_24
            }
            _ => {
                // Auto: take 5 GHz when it is above the floor, even if a 2.4 GHz
                // AP is louder. Below the floor the extra range of 2.4 wins.
                if best_5 != usize::MAX && aps[best_5].rssi >= BAND_PREF_5_MIN_RSSI {
                    // Only when 2.4 GHz is not dramatically stronger. The wider
                    // band is worth a handicap, not an arbitrary one.
                    let penalty = if best_24 == usize::MAX {
                        0
                    } else {
                        aps[best_24].rssi as i16 - aps[best_5].rssi as i16
                    };
                    if penalty <= BAND_PREF_5_MAX_PENALTY_DB {
                        self.pick_reason = PICK_5G_PREFERRED;
                        return best_5;
                    }
                    self.pick_reason = PICK_5G_TOO_WEAK;
                    return best_24;
                }
                self.pick_reason = PICK_STRONGEST;
                if best_24 == usize::MAX { return best_5; }
                if best_5 == usize::MAX { return best_24; }
                if aps[best_5].rssi > aps[best_24].rssi { best_5 } else { best_24 }
            }
        }
    }

    // ── Stage 5a: PHY context + RLC + binding (connect step 1) ────
    // iwl_mvm_phy_ctxt_add + iwl_mvm_phy_send_rlc + iwl_mvm_binding_add_vif
    // (mvm/phy-ctxt.c, mvm/binding.c). Sets the target AP's operating channel
    // (PHY_CONTEXT_CMD v4, 20 MHz), configures the RX chains (RLC_CONFIG_CMD v2 —
    // not offloaded on this FW, cmd_ver=2 < 3), and binds the MAC context (id 0)
    // to the PHY context (id 0) (BINDING_CONTEXT_CMD v2, full struct: CDB binding
    // support, but lmac_id 0 since no CDB). PHY + RLC are fire-and-forget; BINDING
    // returns a status word (CMD_WANT_SKB). All cmd_vers parsed from the FW file.
    fn connect_phy_binding(&mut self) -> bool {
        // PHY_CONTEXT_CMD v4 (action ADD) — 20 MHz on the target channel/band.
        let mut pc = [0u8; PHY_CTX_CMD_LEN];
        put_u32(&mut pc, PC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(phy 0, color 0)
        put_u32(&mut pc, PC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut pc, PC_OFF_CI_CHANNEL, self.target_chan as u32);
        pc[PC_OFF_CI_BAND] = self.target_band;
        pc[PC_OFF_CI_WIDTH] = if self.use_vht80() {
            IWL_PHY_CHANNEL_MODE80
        } else if self.use_ht40() {
            IWL_PHY_CHANNEL_MODE40
        } else {
            IWL_PHY_CHANNEL_MODE20
        };
        pc[PC_OFF_CI_CTRL_POS] = if self.use_ht40() { self.ctrl_pos() } else { 0 };
        put_u32(&mut pc, PC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX); // no CDB → 0
        // Receive chains. iwl_mvm_phy_ctxt_apply fills this field for cmd_ver 3+
        // and sends RLC_CONFIG_CMD afterwards — both, not either. Left at zero
        // the context declares no valid RX antenna.
        put_u32(&mut pc, PC_OFF_RXCHAIN, RLC_RX_CHAIN_INFO_2X2);
        self.send_hcmd(0, PHY_CONTEXT_CMD, &pc); // legacy → LONG_GROUP
        host::dprint("[ax200] PHY_CONTEXT_CMD sent (ch ");
        host::dprint_dec(self.target_chan as u32);
        host::dprint(", band ");
        host::dprint_dec(self.target_band as u32);
        host::dprint(")\n");
        self.pump_rx(20);

        // RLC_CONFIG_CMD v2 (DATA_PATH_GROUP) — RX chains for the PHY context.
        let mut rlc = [0u8; RLC_CMD_LEN];
        put_u32(&mut rlc, RLC_OFF_PHY_ID, 0);
        put_u32(&mut rlc, RLC_OFF_RX_CHAIN_INFO, RLC_RX_CHAIN_INFO_2X2);
        self.send_hcmd(DATA_PATH_GROUP, RLC_CONFIG_CMD, &rlc);
        host::dprint("[ax200] RLC_CONFIG_CMD sent\n");
        self.pump_rx(20);

        // BINDING_CONTEXT_CMD v2 (action ADD): MAC ctx 0 ↔ PHY ctx 0.
        let mut bc = [0u8; BINDING_CMD_LEN];
        put_u32(&mut bc, BC_OFF_ID_COLOR, 0); // phy id 0 / color 0
        put_u32(&mut bc, BC_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut bc, BC_OFF_MACS, 0); // macs[0] = MAC ctx id 0
        put_u32(&mut bc, BC_OFF_MACS + 4, FW_CTXT_INVALID); // macs[1]
        put_u32(&mut bc, BC_OFF_MACS + 8, FW_CTXT_INVALID); // macs[2]
        put_u32(&mut bc, BC_OFF_PHY, 0); // phy id 0
        put_u32(&mut bc, BC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX);
        self.send_hcmd(0, BINDING_CONTEXT_CMD, &bc); // legacy → LONG_GROUP
        host::dprint("[ax200] BINDING_CONTEXT_CMD sent, waiting for status...\n");
        match self.wait_rx(BINDING_CONTEXT_CMD, IWL_ALWAYS_LONG_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let status = le32(&p, RX_PKT_DATA_OFF);
                host::dprint("[ax200]   binding status=0x");
                host::dprint_hex32(status);
                host::dprint("\n");
                host::dprint("[ax200] Stage 5a OK — PHY context + RLC + binding\n");
                true
            }
            None => {
                host::dprint("[ax200] Stage 5a FAILED — binding no response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5b: station (AP peer) + gen2 TX queue (connect step 2) ──
    // iwl_mvm_add_sta + iwl_mvm_tvqm_enable_txq → iwl_trans_txq_alloc (mvm/sta.c,
    // pcie/.../tx-gen2.c). Adds the AP as a LINK station (ADD_STA v12, sta_id 0,
    // minimal flags — HT/rate flags come at assoc) and allocates a dynamic gen2
    // management TX queue for it (for the auth/assoc frames): a TFD ring + first-
    // TB staging + byte-count table, registered with the firmware via
    // SCD_QUEUE_CONFIG_CMD v3 which returns the queue id. No frame is transmitted
    // yet (that is 5c). Both commands return a status (CMD_WANT_SKB).
    fn connect_add_station(&mut self) -> bool {
        // ADD_STA v12 (action ADD) — the AP peer station.
        let mut sc = [0u8; ADD_STA_CMD_LEN];
        sc[AS_OFF_ADD_MODIFY] = 0; // add (not modify)
        put_u16(&mut sc, AS_OFF_TID_DISABLE, self.tid_disable_tx());
        put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(mac 0, 0)
        sc[AS_OFF_ADDR..AS_OFF_ADDR + 6].copy_from_slice(&self.target_bssid);
        sc[AS_OFF_STA_ID] = AP_STA_ID;
        put_u32(&mut sc, AS_OFF_STATION_FLAGS, 0); // refined at assoc (5d)
        put_u32(&mut sc, AS_OFF_STATION_FLAGS_MSK, STA_FLAGS_MSK_ADD);
        sc[AS_OFF_STATION_TYPE] = IWL_STA_LINK;
        self.send_hcmd(0, ADD_STA, &sc); // legacy → LONG_GROUP
        host::dprint("[ax200] ADD_STA sent (AP peer, sta_id 0), waiting...\n");
        match self.wait_rx(ADD_STA, IWL_ALWAYS_LONG_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let status = le32(&p, RX_PKT_DATA_OFF) & ADD_STA_STATUS_MASK;
                host::dprint("[ax200]   ADD_STA status=0x");
                host::dprint_hex32(status);
                host::dprint("\n");
                if status != ADD_STA_SUCCESS {
                    host::dprint("[ax200] Stage 5b FAILED — ADD_STA rejected\n");
                    self.dump_fw_error_log();
                    return false;
                }
            }
            None => {
                host::dprint("[ax200] Stage 5b FAILED — ADD_STA no response\n");
                self.dump_fw_error_log();
                return false;
            }
        }

        // Allocate the management TX queue DMA: TFD ring + first-TB staging +
        // byte-count table (16-slot queue → IWL_MGMT_QUEUE_SIZE).
        self.mgmt_tfd = self.alloc_dma(TFH_TFD_SIZE * IWL_MGMT_QUEUE_SIZE, "mgmt.tfd");
        self.mgmt_first_tb =
            self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * IWL_MGMT_QUEUE_SIZE, "mgmt.first_tb");
        self.mgmt_payload = self.alloc_dma(TX_PAYLOAD_STRIDE * IWL_MGMT_QUEUE_SIZE, "mgmt.payload");
        self.mgmt_bc_tbl = self.alloc_dma(BC_TBL_BYTES, "mgmt.bc_tbl");
        if !self.mgmt_tfd.ok() || !self.mgmt_first_tb.ok() || !self.mgmt_payload.ok() || !self.mgmt_bc_tbl.ok() {
            host::dprint("[ax200] Stage 5b FAILED — TX queue DMA alloc\n");
            return false;
        }

        // SCD_QUEUE_CONFIG_CMD v3 (ADD): hand the DMA addresses to the firmware.
        let mut q = [0u8; SCD_CMD_LEN];
        put_u32(&mut q, SQ_OFF_OPERATION, IWL_SCD_QUEUE_ADD);
        put_u32(&mut q, SQ_OFF_STA_MASK, 1 << AP_STA_ID); // BIT(sta_id)
        q[SQ_OFF_TID] = IWL_MGMT_TID;
        put_u32(&mut q, SQ_OFF_FLAGS, 0);
        put_u32(&mut q, SQ_OFF_CB_SIZE, MGMT_QUEUE_CB_SIZE);
        put_u64(&mut q, SQ_OFF_BC_DRAM_ADDR, self.mgmt_bc_tbl.phys);
        put_u64(&mut q, SQ_OFF_TFDQ_DRAM_ADDR, self.mgmt_tfd.phys);
        self.send_hcmd(DATA_PATH_GROUP, SCD_QUEUE_CONFIG_CMD, &q);
        host::dprint("[ax200] SCD_QUEUE_CONFIG_CMD sent (mgmt tid 15), waiting...\n");
        match self.wait_rx(SCD_QUEUE_CONFIG_CMD, DATA_PATH_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                self.mgmt_queue_id = u16::from_le_bytes([
                    p[b + SQ_RSP_OFF_QUEUE_NUMBER],
                    p[b + SQ_RSP_OFF_QUEUE_NUMBER + 1],
                ]);
                self.mgmt_write_ptr = u16::from_le_bytes([
                    p[b + SQ_RSP_OFF_WRITE_PTR],
                    p[b + SQ_RSP_OFF_WRITE_PTR + 1],
                ]) as u32;
                host::dprint("[ax200]   mgmt queue_id=");
                host::dprint_dec(self.mgmt_queue_id as u32);
                host::dprint(" write_ptr=");
                host::dprint_dec(self.mgmt_write_ptr);
                host::dprint("\n");
                host::dprint("[ax200] Stage 5b OK — station + TX queue allocated\n");
                true
            }
            None => {
                host::dprint("[ax200] Stage 5b FAILED — SCD_QUEUE_CONFIG no response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5b': finish the connect chanctx tail (iwl_mvm_assign_vif_chanctx) ──
    // __iwl_mvm_assign_vif_chanctx does binding → power_update_mac → (quota, only
    // for monitor) and the connect flow then re-sends the MAC context with the
    // target BSSID (iwl_mvm_mac_ctxt_changed on BSS_CHANGED_BSSID). We had skipped
    // this whole tail and went straight to the auth TX with a MAC context still
    // holding the scan-time broadcast BSSID + zero timing — so once session
    // protection put the firmware on-channel and it actually processed the auth,
    // the time-event/scheduler (UMAC) asserted. Send both before the auth.
    fn connect_finish_chanctx(&mut self) {
        // iwl_mvm_power_update_mac → MAC_PM_POWER_TABLE for the bss vif. Power-save
        // disabled path: only id_and_color + keep_alive_seconds, flags = 0.
        let mut pm = [0u8; MAC_POWER_CMD_LEN];
        put_u32(&mut pm, MP_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0,0)
        put_u16(&mut pm, MP_OFF_KEEP_ALIVE, POWER_KEEP_ALIVE_PERIOD_SEC);
        self.send_hcmd(0, MAC_PM_POWER_TABLE, &pm);
        host::dprint("[ax200] MAC_PM_POWER_TABLE sent (PS disabled)\n");
        self.pump_rx(20);

        // iwl_mvm_mac_ctxt_changed (MODIFY) with the target AP's BSSID + timing,
        // unassociated branch (is_assoc = 0, MAC_FILTER_IN_BEACON).
        let mut cmd = [0u8; MAC_CTX_CMD_LEN];
        put_u32(&mut cmd, MC_OFF_ID_COLOR, 0);
        put_u32(&mut cmd, MC_OFF_ACTION, FW_CTXT_ACTION_MODIFY);
        put_u32(&mut cmd, MC_OFF_MAC_TYPE, FW_MAC_TYPE_BSS_STA);
        put_u32(&mut cmd, MC_OFF_TSF_ID, 0);
        cmd[MC_OFF_NODE_ADDR..MC_OFF_NODE_ADDR + 6].copy_from_slice(&self.mac);
        cmd[MC_OFF_BSSID_ADDR..MC_OFF_BSSID_ADDR + 6].copy_from_slice(&self.target_bssid);
        put_u32(&mut cmd, MC_OFF_CCK_RATES, MAC_CCK_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_OFDM_RATES, MAC_OFDM_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP | MAC_FILTER_IN_BEACON);
        // Every MAC context command goes through iwl_mvm_mac_ctxt_cmd_common,
        // and that always fills the QoS block — not only the associated one.
        self.fill_qos_params(&mut cmd);
        // iwl_mac_data_sta (unassoc): is_assoc = 0, bi = beacon interval, dtim
        // unknown pre-assoc (→ 0), assoc_id = 0.
        put_u32(&mut cmd, MC_OFF_STA_IS_ASSOC, 0);
        put_u32(&mut cmd, MC_OFF_STA_BI, self.target_beacon_int as u32);
        self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);
        host::dprint("[ax200] MAC_CONTEXT_CMD (modify, target BSSID) sent\n");
        self.pump_rx(50);

        // Grab the beacon timing HERE, not after the association: this is the
        // last point where nothing else is expected on the RX ring. Doing it
        // after the assoc response would mean draining (and discarding) the
        // frames the AP sends next — including the first EAPOL of the 4-way.
        // Auth + assoc take a few ms, so the timing is still current.
        // The timing came with the scan result (see collect_ap). Waiting here for
        // another beacon was a step of our own invention — mac80211 does not have
        // it — and it never once succeeded, which is why the firmware kept
        // getting a made-up wake schedule and never reported a missed beacon.
        if self.sync_ok {
            host::print("[ax200] beacon timing from scan: dtim ");
            host::print_dec(self.sync_dtim_count as u32);
            host::print("/");
            host::print_dec(self.target_dtim_period as u32);
            host::print("\n");
        } else {
            host::print("[ax200] chosen AP was seen only as a probe response - no beacon timing\n");
        }
    }

    // ── Post-association: tell the firmware we are associated ─────────────
    // Everything below runs once, right after the assoc response. Linux does it
    // from the BSS_CHANGED_ASSOC / sta-state path; we had none of it, so the
    // firmware kept a MAC context that still said "not associated" for the whole
    // life of the link.

    // Wait for one beacon of our BSS and capture the timing the MAC context
    // needs (iwl_mvm_set_fw_dtim_tbtt reads exactly these three): the AP's TSF
    // and our device timestamp at beacon arrival, plus the DTIM count still to
    // run. Also picks up the DTIM period / HT element if the scan missed them.
    // Returns false if no beacon arrived — then we cannot claim association.
    // iwl_mvm_mac_ctxt_cmd_sta, associated branch. Marks the MAC context as
    // associated with the DTIM timing + AID, and drops MAC_FILTER_IN_BEACON
    // (Linux only sets that while unassociated).
    /// `iwl_mvm_set_fw_qos_params` (mvm/mac-ctxt.c:475), line for line.
    ///
    /// Both fields were left at zero since the first port. `qos_flags = 0`
    /// says: no EDCA configuration, and NOT an 802.11n BSS. The firmware runs
    /// the TX aggregation manager itself on this ucode (TLC offload), and it
    /// has no reason to open a session for a BSS it was told is neither QoS
    /// nor HT. Measured before this: `tx agg aggregated 0 of 22353`.
    fn fill_qos_params(&self, cmd: &mut [u8]) {
        if self.target_ht.wmm {
            for i in 0..4 {
                let (aifs, cw_min, cw_max, txop) = self.target_ht.edca[i];
                let a = MC_OFF_AC + AC_TO_UCODE_AC[i] * AC_QOS_LEN;
                put_u16(cmd, a + ACQ_OFF_CW_MIN, cw_min);
                put_u16(cmd, a + ACQ_OFF_CW_MAX, cw_max);
                cmd[a + ACQ_OFF_AIFSN] = aifs;
                cmd[a + ACQ_OFF_FIFOS_MASK] = 1 << AC_TO_TX_FIFO[i];
                // mac80211 keeps txop in 32 us units; the firmware wants us.
                put_u16(cmd, a + ACQ_OFF_EDCA_TXOP, txop.saturating_mul(32));
            }
        }
        let mut qos = 0u32;
        if self.target_ht.wmm {
            qos |= MAC_QOS_FLG_UPDATE_EDCA;
        }
        // "if chanreq.oper.width != NL80211_CHAN_WIDTH_20_NOHT" — i.e. any
        // width that is not the legacy no-HT one. We are an HT station
        // whenever the AP carries an HT element, 20 MHz included.
        if self.target_ht.present {
            qos |= MAC_QOS_FLG_TGN;
        }
        put_u32(cmd, MC_OFF_QOS_FLAGS, qos);
    }

    fn mac_ctxt_assoc(&mut self) {
        let mut cmd = [0u8; MAC_CTX_CMD_LEN];
        put_u32(&mut cmd, MC_OFF_ID_COLOR, 0);
        put_u32(&mut cmd, MC_OFF_ACTION, FW_CTXT_ACTION_MODIFY);
        put_u32(&mut cmd, MC_OFF_MAC_TYPE, FW_MAC_TYPE_BSS_STA);
        put_u32(&mut cmd, MC_OFF_TSF_ID, 0);
        cmd[MC_OFF_NODE_ADDR..MC_OFF_NODE_ADDR + 6].copy_from_slice(&self.mac);
        cmd[MC_OFF_BSSID_ADDR..MC_OFF_BSSID_ADDR + 6].copy_from_slice(&self.target_bssid);
        put_u32(&mut cmd, MC_OFF_CCK_RATES, MAC_CCK_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_OFDM_RATES, MAC_OFDM_RATES_DEFAULT);
        put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP);
        // What the BSS asks us to protect against. Sent as 0 until now, i.e.
        // "nothing" — the firmware could not know that legacy or non-member
        // stations share this channel.
        let prot = self.protection_flags();
        put_u32(&mut cmd, MC_OFF_PROT_FLAGS, prot);
        self.fill_qos_params(&mut cmd);

        // iwl_mvm_set_fw_dtim_tbtt: the DTIM count counts down, so the next DTIM
        // TBTT is that many beacon intervals after the beacon we just heard.
        // Beacon intervals are TU (1024 us).
        let bi = self.target_beacon_int as u32;
        let dtim_offs = (self.sync_dtim_count as u32) * bi * 1024;
        put_u32(&mut cmd, MC_OFF_STA_IS_ASSOC, 1);
        put_u32(&mut cmd, MC_OFF_STA_DTIM_TIME, self.sync_device_ts.wrapping_add(dtim_offs));
        put_u64(&mut cmd, MC_OFF_STA_DTIM_TSF, self.sync_tsf.wrapping_add(dtim_offs as u64));
        put_u32(&mut cmd, MC_OFF_STA_BI, bi);
        put_u32(&mut cmd, MC_OFF_STA_DTIM_INTERVAL, bi * self.target_dtim_period as u32);
        put_u32(&mut cmd, MC_OFF_STA_LISTEN_INTERVAL, DOT11_LISTEN_INTERVAL as u32);
        put_u32(&mut cmd, MC_OFF_STA_ASSOC_ID, self.assoc_aid as u32);
        put_u32(&mut cmd, MC_OFF_STA_BEACON_ARRIVE, self.sync_device_ts);
        self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);
        host::print("[ax200] MAC_CONTEXT is_assoc=1 (aid ");
        host::print_dec(self.assoc_aid as u32);
        host::print(", bi ");
        host::print_dec(bi);
        host::print(", dtim ");
        host::print_dec(self.target_dtim_period as u32);
        host::print(")\n");
    }

    // iwl_mvm_sta_send_to_fw with update=true — the station flags the peer's HT
    // capabilities imply, plus the AID. On a modify Linux leaves addr zeroed and
    // lets station_flags_msk select which bits to apply.
    fn sta_assoc_update(&mut self) {
        // The station's TX width has to match the PHY context. Left at 20 MHz
        // it produced exactly the asymmetry measured on the device once VHT80
        // came up: RX at 650 Mbit, TX at 26, heavy loss, then a dropped link —
        // the receive path ran at 80 MHz while transmission was pinned to 20
        // and the rate control had to reconcile the two.
        // `iwl_mvm_sta_send_to_fw` sets this from the peer's bandwidth.
        let mut flags = if self.use_vht80() {
            STA_FLG_FAT_EN_80MHZ
        } else if self.use_ht40() {
            STA_FLG_FAT_EN_40MHZ
        } else {
            STA_FLG_FAT_EN_20MHZ
        };
        // …and the mask has to select it, or a modify leaves the old value.
        let mut msk = STA_FLAGS_MSK_ADD | STA_FLG_FAT_EN_MSK;
        if self.target_ht.present {
            // rx_nss: the AP's second-stream MCS mask decides 1 vs 2 streams.
            flags |= if self.target_ht.mcs_rx[1] != 0 {
                STA_FLG_MIMO_EN_MIMO2
            } else {
                STA_FLG_MIMO_EN_SISO
            };
            flags |= (self.target_ht.ampdu_factor as u32) << STA_FLG_MAX_AGG_SIZE_SHIFT;
            flags |= (self.target_ht.ampdu_density as u32) << STA_FLG_AGG_MPDU_DENS_SHIFT;
            msk |= STA_FLG_MAX_AGG_SIZE_MSK | STA_FLG_AGG_MPDU_DENS_MSK;
        } else {
            flags |= STA_FLG_MIMO_EN_SISO;
        }

        let mut sc = [0u8; ADD_STA_CMD_LEN];
        sc[AS_OFF_ADD_MODIFY] = 1; // modify
        if self.want_txagg {
            // A modify only applies the fields modify_mask selects.
            sc[AS_OFF_MODIFY_MASK] |= STA_MODIFY_TID_DISABLE_TX;
        }
        put_u16(&mut sc, AS_OFF_TID_DISABLE, self.tid_disable_tx());
        put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0);
        sc[AS_OFF_STA_ID] = AP_STA_ID;
        put_u32(&mut sc, AS_OFF_STATION_FLAGS, flags);
        put_u32(&mut sc, AS_OFF_STATION_FLAGS_MSK, msk);
        sc[AS_OFF_STATION_TYPE] = IWL_STA_LINK;
        put_u16(&mut sc, AS_OFF_ASSOC_ID, self.assoc_aid);
        self.send_hcmd(0, ADD_STA, &sc);
        host::print("[ax200] ADD_STA modify: station_flags=0x");
        host::print_hex32(flags);
        host::print("\n");
    }

    // The whole post-assoc chain, in Linux' order: mark the MAC context
    // associated, update the station, then start rate scaling.
    fn connect_post_assoc(&mut self) {
        // No RX draining in here: the AP starts the 4-way immediately after the
        // assoc response, and anything we drain now we throw away. The beacon
        // timing was captured before the auth (connect_finish_chanctx).
        // Linux: "We need the dtim_period to set the MAC as associated."
        if self.target_dtim_period != 0 {
            self.mac_ctxt_assoc();
        } else {
            host::print("[ax200] no DTIM period — MAC context stays unassociated\n");
        }
        self.sta_assoc_update();
        self.connect_tlc_config();
    }

    // ── Reconnect after a link loss (mesh steering / deauth) ──────────────
    // The Fritzbox + Fritz repeater run one SSID across two APs and steer the
    // client between them with a DEAUTH. We re-scan, re-point the already-added
    // PHY context + station + MAC context at the best AP (may be the OTHER mesh
    // node, on a different channel) via MODIFY actions — the binding (MAC0↔PHY0)
    // and the TX queues persist, so NO DMA is re-allocated (the DMA budget can't
    // churn per reconnect). Then redo auth + assoc and re-arm wifid for a fresh
    // 4-way. Returns true once associated.

    // PHY_CONTEXT_CMD v4 (action MODIFY) — re-point the PHY at the new channel.
    fn update_phy_context(&mut self) {
        let mut pc = [0u8; PHY_CTX_CMD_LEN];
        put_u32(&mut pc, PC_OFF_ID_COLOR, 0);
        put_u32(&mut pc, PC_OFF_ACTION, FW_CTXT_ACTION_MODIFY);
        put_u32(&mut pc, PC_OFF_CI_CHANNEL, self.target_chan as u32);
        pc[PC_OFF_CI_BAND] = self.target_band;
        pc[PC_OFF_CI_WIDTH] = IWL_PHY_CHANNEL_MODE20;
        pc[PC_OFF_CI_CTRL_POS] = 0;
        put_u32(&mut pc, PC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX);
        put_u32(&mut pc, PC_OFF_RXCHAIN, RLC_RX_CHAIN_INFO_2X2);
        self.send_hcmd(0, PHY_CONTEXT_CMD, &pc); // legacy → LONG_GROUP
        self.pump_rx(20);
    }

    // ADD_STA v12 (action MODIFY) — re-point the AP-peer station at the new BSSID.

    fn retarget_station(&mut self) {
        let mut sc = [0u8; ADD_STA_CMD_LEN];
        sc[AS_OFF_ADD_MODIFY] = 1; // modify
        if self.want_txagg {
            // A modify only applies the fields modify_mask selects.
            sc[AS_OFF_MODIFY_MASK] |= STA_MODIFY_TID_DISABLE_TX;
        }
        put_u16(&mut sc, AS_OFF_TID_DISABLE, self.tid_disable_tx());
        put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0);
        sc[AS_OFF_ADDR..AS_OFF_ADDR + 6].copy_from_slice(&self.target_bssid);
        sc[AS_OFF_STA_ID] = AP_STA_ID;
        put_u32(&mut sc, AS_OFF_STATION_FLAGS, 0);
        put_u32(&mut sc, AS_OFF_STATION_FLAGS_MSK, STA_FLAGS_MSK_ADD);
        sc[AS_OFF_STATION_TYPE] = IWL_STA_LINK;
        self.send_hcmd(0, ADD_STA, &sc); // legacy → LONG_GROUP
        self.pump_rx(50);
    }

    /// 40 MHz only when ALL THREE agree: the AP says it can (capability
    /// element), it says it currently does and on which side (operation
    /// element), and the band has the room. Anything less stays at 20 —
    /// a PHY context wider than the AP's actual channel points at silence.
    /// `iwl_mvm_sta_send_to_fw`: `tid_disable_tx = mvm_sta->tid_disable_agg`,
    /// which starts at 0xffff and is only ever cleared by `iwl_mvm_sta_tx_agg`
    /// — unreachable on TLC-offload firmware. So 0xffff IS Linux' value here,
    /// and it stays the default. `wlan set txagg on` sends 0x0000 to find out
    /// whether the firmware honours the field anyway.
    fn tid_disable_tx(&self) -> u16 {
        if self.want_txagg { TID_DISABLE_AGG_NONE } else { TID_DISABLE_AGG_INIT }
    }

    fn use_ht40(&self) -> bool {
        self.want_ht40
            && self.target_ht.present
            && self.target_ht.cap_info & IEEE80211_HT_CAP_SUP_WIDTH_20_40 != 0
            && self.target_ht.sec_chan_offs != IEEE80211_HT_PARAM_CHA_SEC_NONE
    }

    /// 80 MHz when the AP carries both VHT elements, the operation element
    /// actually says 80 (USE_HT means it is running an HT width after all),
    /// and it offers at least two spatial streams at MCS 0-9. Anything less
    /// falls through to `use_ht40`.
    /// `iwl_mvm_set_fw_protection_flags` (mvm/mac-ctxt.c), branch for branch.
    ///
    /// We sent 0 unconditionally — "no protection needed" — whatever the AP
    /// said. The firmware then has no way to know that legacy or non-member
    /// stations share the channel, and every transmission takes its chances.
    /// Measured on the device: `rts-fail` at 22 % of 122407 frames.
    ///
    /// Note this may make the firmware protect MORE, not less. That is the
    /// point: protection costs airtime and buys collisions avoided, and the AP
    /// is the only party that knows which trade its BSS needs.
    fn protection_flags(&self) -> u32 {
        let mut flags = 0u32;
        if self.target_ht.erp_protect {
            flags |= MAC_PROT_FLG_TGG_PROTECT;
        }
        // "for both sta and ap, ht_operation_mode hold the protection_mode",
        // and Linux treats a zero operation_mode as "HT protection not in use".
        if self.target_ht.ht_op_mode & IEEE80211_HT_OP_MODE_PROTECTION == 0 {
            return flags;
        }
        // The firmware does not distinguish HT from FAT, so Linux sets both.
        let ht_flag = MAC_PROT_FLG_HT_PROT | MAC_PROT_FLG_FAT_PROT;
        match self.target_ht.ht_op_mode & IEEE80211_HT_OP_MODE_PROTECTION {
            IEEE80211_HT_OP_MODE_PROTECTION_NONE => {}
            IEEE80211_HT_OP_MODE_PROTECTION_NONMEMBER
            | IEEE80211_HT_OP_MODE_PROTECTION_NONHT_MIXED => flags |= ht_flag,
            IEEE80211_HT_OP_MODE_PROTECTION_20MHZ => {
                // Only when we are actually wider than 20 MHz.
                if self.use_ht40() { flags |= ht_flag; }
            }
            _ => {}
        }
        flags
    }

    /// Where the VHT operation info comes from, per `ieee80211_determine_ap_chan`:
    /// an AP with HE Capability whose HE Operation sets VHT_OPER_INFO carries it
    /// in THOSE three bytes, and element 192 is then not consulted at all. Any
    /// other AP answers from element 192. Returns (chan_width, seg0, from_he).
    fn vht_oper(&self) -> (u8, u8, bool) {
        let h = &self.target_ht;
        if h.he && h.he_vht_op_present {
            (h.he_vht_op[0], h.he_vht_op[1], true)
        } else {
            (h.vht_chan_width, h.vht_seg0, false)
        }
    }

    fn use_vht80(&self) -> bool {
        self.want_vht
            && self.target_ht.vht
            && self.vht_oper().0 == IEEE80211_VHT_CHANWIDTH_80MHZ
            && self.use_ht40()
    }

    /// `iwl_mvm_get_ctrl_pos` for the HT case. The control channel is the
    /// UPPER of the pair exactly when the secondary sits BELOW it; for 40 MHz
    /// the offset term of that function is zero, so only the ABOVE bit is left.
    fn ctrl_pos(&self) -> u8 {
        if !self.use_vht80() {
            // 40 MHz: the offset term of iwl_mvm_get_ctrl_pos is zero, so only
            // the ABOVE bit remains — set when the secondary half is below.
            return if self.target_ht.sec_chan_offs == IEEE80211_HT_PARAM_CHA_SEC_BELOW {
                IWL_PHY_CTRL_POS_ABOVE
            } else {
                0
            };
        }
        // 80 MHz: iwl_mvm_get_ctrl_pos in full. Channel numbers are 5 MHz
        // apart, so the control channel sits 10 or 30 MHz from the centre.
        //     ret = (abs_offs - 10) / 20   →  0 or 1
        //     ret |= (offs > 0) * ABOVE
        let offs = (self.target_chan as i32 - self.vht_oper().1 as i32) * 5;
        if offs == 0 {
            return 0;
        }
        let mut ret = ((offs.abs() - 10) / 20) as u8 & IWL_PHY_CTRL_POS_OFFS_MSK;
        if offs > 0 {
            ret |= IWL_PHY_CTRL_POS_ABOVE;
        }
        ret
    }

    fn reconnect(&mut self) -> bool {
        host::print("[ax200] link lost — re-scanning to reconnect...\n");
        self.authorized = false;
        self.ptk_installed = false;
        self.link_published = false;
        // The association is gone too — this is a real carrier loss, not a
        // dormant phase.
        host::netdev_set_link_state(false, false);
        let mut down = [0u8; 7];
        down[0] = EV_LINK_DOWN;
        down[1..7].copy_from_slice(&self.target_bssid);
        host::wifi_send_event(&down);

        if !self.run_scan() || !self.target_valid {
            return false;
        }
        // Re-point PHY + station + MAC context at the (possibly new) best AP.
        self.update_phy_context();
        self.retarget_station();
        self.connect_finish_chanctx();

        let mut associated = false;
        for attempt in 0..3 {
            if attempt > 0 {
                host::print("[ax200] reconnect retry ");
                host::print_dec(attempt as u32 + 1);
                host::print("/3...\n");
            }
            if self.connect_send_auth() && self.connect_send_assoc() {
                associated = true;
                break;
            }
        }
        if associated {
            host::print("[ax200] re-associated — re-running 4-way\n");
            self.data_in_flight = 0;
            self.data_bytes_in_flight = 0;
            // The firmware's sessions died with the old association; anything
            // the windows still hold belongs to a link that no longer exists.
            self.ba_stop_all();
            let our_mac = self.mac;
            let mut ready = [0u8; 13];
            ready[0] = EV_READY;
            ready[1..7].copy_from_slice(&self.target_bssid);
            ready[7..13].copy_from_slice(&our_mac);
            host::wifi_send_event(&ready);
            self.st.ready_sent = self.st.ready_sent.wrapping_add(1);
            self.connect_post_assoc();
        }
        associated
    }

    // ── Rate scaling: TLC offload (iwl_mvm_rs_fw_rate_init, mvm/rs-fw.c) ──
    // Configure firmware rate scaling for the AP station so data frames stop
    // going out at the fixed host rate (1 Mbit CCK in tx_raw). We advertise the
    // station's legacy (non-HT) rate set; the firmware then picks the best rate
    // per frame from its TLC table. Sent once after association (Linux sends it
    // CMD_ASYNC → fire-and-forget, then a TLC_MNG_UPDATE_NOTIF reports the rate).
    // TLC_MNG_CONFIG_CMD cmd_ver=4 on this FW → struct iwl_tlc_config_cmd_v4.
    // HT/VHT/HE MCS (mode HT/VHT/HE + ht_rates) is a later rung: it needs the
    // matching cap IEs in the assoc request + station HT flags. Legacy alone
    // already lifts us from 1 Mbit to up to 54 Mbit OFDM.
    fn connect_tlc_config(&mut self) {
        let mut cmd = [0u8; TLC_CMD_LEN];
        cmd[TLC_OFF_STA_ID] = AP_STA_ID;
        cmd[TLC_OFF_MAX_CH_WIDTH] = TLC_CH_WIDTH_20MHZ;
        cmd[TLC_OFF_CHAINS] = ANT_AB as u8; // chain A|B = BIT(0)|BIT(1)
        // non_ht_rates is filled in every mode (rs_fw_set_supp_rates sets it
        // before the mode switch) — it is the fallback the firmware drops to.
        let non_ht = if self.target_band == PHY_BAND_24 as u8 {
            TLC_NON_HT_RATES_24
        } else {
            TLC_NON_HT_RATES_5
        };
        put_u16(&mut cmd, TLC_OFF_NON_HT_RATES, non_ht);

        if self.target_ht.present {
            cmd[TLC_OFF_MODE] = TLC_MODE_HT;
            // ht_rates carries the PEER's receive MCS mask — what the AP can
            // take from us — per spatial stream, in the "80 MHz and below" slot.
            put_u16(&mut cmd, TLC_OFF_HT_RATES_NSS1, self.target_ht.mcs_rx[0] as u16);
            put_u16(&mut cmd, TLC_OFF_HT_RATES_NSS2, self.target_ht.mcs_rx[1] as u16);
            // rs_fw_sgi_cw_support: one bit per channel width. And
            // max_ch_width has to say 40 too, or the rate control never picks a
            // 40 MHz rate no matter how the PHY is configured.
            if self.target_ht.cap_info & IEEE80211_HT_CAP_SGI_20 != 0 {
                cmd[TLC_OFF_SGI] |= 1 << TLC_CH_WIDTH_20MHZ;
            }
            if self.use_ht40() && self.authorized {
                cmd[TLC_OFF_MAX_CH_WIDTH] = TLC_CH_WIDTH_40MHZ;
                if self.target_ht.cap_info & IEEE80211_HT_CAP_SGI_40 != 0 {
                    cmd[TLC_OFF_SGI] |= 1 << TLC_CH_WIDTH_40MHZ;
                }
            }
            if self.use_vht80() {
                // The rate table is VHT's, not HT's: MCS 0-9 per stream. The
                // ht_rates field carries it — the firmware reads it by `mode`.
                cmd[TLC_OFF_MODE] = TLC_MODE_VHT;
                if self.authorized {
                    cmd[TLC_OFF_MAX_CH_WIDTH] = TLC_CH_WIDTH_80MHZ;
                }
                let vht_mcs = 0x03ffu16; // MCS 0-9
                put_u16(&mut cmd, TLC_OFF_HT_RATES_NSS1, vht_mcs);
                put_u16(&mut cmd, TLC_OFF_HT_RATES_NSS2, vht_mcs);
                if self.target_ht.vht_cap_info & IEEE80211_VHT_CAP_SHORT_GI_80 != 0 {
                    cmd[TLC_OFF_SGI] |= 1 << TLC_CH_WIDTH_80MHZ;
                }
            }
            let mut flags = 0u16;
            if self.target_ht.cap_info & IEEE80211_HT_CAP_LDPC_CODING != 0 {
                flags |= TLC_FLAGS_LDPC;
            }
            if self.target_ht.cap_info & IEEE80211_HT_CAP_RX_STBC != 0 {
                flags |= TLC_FLAGS_STBC;
            }
            put_u16(&mut cmd, TLC_OFF_FLAGS, flags);
            // max_mpdu_len stays 0: that field enables TX A-MSDU, and we build
            // our own frames. max_tx_op 0 = no limit.
            host::print("[ax200] TLC mode=HT mcs=");
            host::print_hex8(self.target_ht.mcs_rx[1]);
            host::print_hex8(self.target_ht.mcs_rx[0]);
            host::print(" sgi=");
            host::print_dec(cmd[TLC_OFF_SGI] as u32);
            host::print(" flags=0x");
            host::print_hex16(flags);
            host::print("\n");
        } else {
            cmd[TLC_OFF_MODE] = TLC_MODE_NON_HT;
            host::print("[ax200] TLC mode=NON_HT (legacy 1..54M)\n");
        }
        self.send_hcmd(DATA_PATH_GROUP, TLC_MNG_CONFIG_CMD, &cmd);
    }

    /// Decode a v2 rate_n_flags into the log. Always prints the raw word too —
    /// the decode is our reading of the format, the raw value is the truth.
    fn log_rate(prefix: &str, raw: u32) {
        host::print(prefix);
        host::print("0x");
        host::print_hex32(raw);
        let rnf = Self::rate_v3(raw);
        let code = rnf & RATE_MCS_CODE_MSK;
        match rnf & RATE_MCS_MOD_TYPE_MSK {
            RATE_MCS_MOD_TYPE_CCK => {
                host::print(" CCK idx ");
                host::print_dec(code);
            }
            RATE_MCS_MOD_TYPE_LEGACY_OFDM => {
                host::print(" OFDM idx ");
                host::print_dec(code);
            }
            RATE_MCS_MOD_TYPE_HT => {
                host::print(" HT MCS ");
                host::print_dec(code);
                host::print(if rnf & RATE_MCS_NSS_MSK != 0 { " 2ss" } else { " 1ss" });
            }
            RATE_MCS_MOD_TYPE_VHT => {
                host::print(" VHT MCS ");
                host::print_dec(code);
            }
            RATE_MCS_MOD_TYPE_HE => {
                host::print(" HE MCS ");
                host::print_dec(code);
            }
            _ => host::print(" mod?"),
        }
        host::print(" bw");
        host::print_dec((rnf & RATE_MCS_CHAN_WIDTH_MSK) >> RATE_MCS_CHAN_WIDTH_POS);
        host::print("\n");
    }

    // PHY rate of a rate_n_flags value in kbit/s, or 0 if we cannot tell. The
    // raw word is always printed next to it — this is our reading of the format,
    // the hex is the truth.
    //
    // Legacy code→rate mapping per iwl_mvm_legacy_hw_idx_to_mac80211_idx:
    // OFDM code 0 is the FIRST OFDM rate (6M), CCK code 0 is 1M.
    // iwl_v3_rate_from_v2_v3: lift a firmware rate_n_flags into the v3 layout.
    // The only difference between the two is where the NSS bit lives, so this
    // moves it from bit 4 to bit 5 and leaves the rest alone. Everything below
    // then decodes one format instead of two.
    fn rate_v3(rnf: u32) -> u32 {
        if fw_cmd_ver(0, TX_CMD) >= TX_CMD_VER_RATE_V3 {
            return rnf;
        }
        (rnf & !RATE_MCS_NSS_MSK_V2) | ((rnf & RATE_MCS_NSS_MSK_V2) << 1)
    }

    fn rate_kbit(rnf: u32) -> u32 {
        const CCK: [u32; 4] = [1000, 2000, 5500, 11000];
        const OFDM: [u32; 8] = [6000, 9000, 12000, 18000, 24000, 36000, 48000, 54000];
        // HT per spatial stream, [bw20 lgi, bw20 sgi, bw40 lgi, bw40 sgi].
        const HT: [[u32; 8]; 4] = [
            [6500, 13000, 19500, 26000, 39000, 52000, 58500, 65000],
            [7200, 14400, 21700, 28900, 43300, 57800, 65000, 72200],
            [13500, 27000, 40500, 54000, 81000, 108000, 121500, 135000],
            [15000, 30000, 45000, 60000, 90000, 120000, 135000, 150000],
        ];
        // VHT per spatial stream, [bw20 lgi, bw20 sgi, bw40 …, bw80 …].
        // Derived from the 802.11ac formula rather than copied: data
        // subcarriers (52 / 108 / 234) x bits-per-subcarrier x coding rate,
        // over the 4.0 us symbol (3.6 us with short GI).
        const VHT: [[u32; 10]; 6] = [
            [6500, 13000, 19500, 26000, 39000, 52000, 58500, 65000, 78000, 86667],
            [7222, 14444, 21667, 28889, 43333, 57778, 65000, 72222, 86667, 96296],
            [13500, 27000, 40500, 54000, 81000, 108000, 121500, 135000, 162000, 180000],
            [15000, 30000, 45000, 60000, 90000, 120000, 135000, 150000, 180000, 200000],
            [29250, 58500, 87750, 117000, 175500, 234000, 263250, 292500, 351000, 390000],
            [32500, 65000, 97500, 130000, 195000, 260000, 292500, 325000, 390000, 433333],
        ];
        let code = (rnf & RATE_MCS_CODE_MSK) as usize;
        let sgi = if rnf & RATE_MCS_SGI_MSK != 0 { 1 } else { 0 };
        let bw = (rnf & RATE_MCS_CHAN_WIDTH_MSK) >> RATE_MCS_CHAN_WIDTH_POS;
        let nss = if rnf & RATE_MCS_NSS_MSK != 0 { 2 } else { 1 };
        match rnf & RATE_MCS_MOD_TYPE_MSK {
            RATE_MCS_MOD_TYPE_CCK => if code < 4 { CCK[code] } else { 0 },
            RATE_MCS_MOD_TYPE_LEGACY_OFDM => if code < 8 { OFDM[code] } else { 0 },
            RATE_MCS_MOD_TYPE_HT => {
                if code >= 8 || bw > 1 { return 0; }
                HT[(bw as usize) * 2 + sgi][code] * nss
            }
            RATE_MCS_MOD_TYPE_VHT => {
                if code >= 10 || bw > 2 { return 0; }
                VHT[(bw as usize) * 2 + sgi][code] * nss
            }
            _ => 0, // HE: not negotiated, so no table for it
        }
    }

    // One rate line: raw word, decoded modulation, and the PHY rate it implies.
    fn rep_rate(r: &mut Rep, label: &str, raw: u32) {
        r.s(label);
        if raw == 0 || raw == u32::MAX {
            r.s("(none reported yet)\n");
            return;
        }
        // Print the RAW firmware word, decode the normalised one.
        r.s("0x");
        r.hex(raw, 8);
        r.s(" = ");
        let rnf = Self::rate_v3(raw);
        let code = rnf & RATE_MCS_CODE_MSK;
        match rnf & RATE_MCS_MOD_TYPE_MSK {
            RATE_MCS_MOD_TYPE_CCK => { r.s("CCK idx "); r.d(code as u64); }
            RATE_MCS_MOD_TYPE_LEGACY_OFDM => { r.s("OFDM idx "); r.d(code as u64); }
            RATE_MCS_MOD_TYPE_HT => {
                r.s("HT MCS "); r.d(code as u64);
                r.s(if rnf & RATE_MCS_NSS_MSK != 0 { " 2ss" } else { " 1ss" });
            }
            RATE_MCS_MOD_TYPE_VHT => { r.s("VHT MCS "); r.d(code as u64); }
            RATE_MCS_MOD_TYPE_HE => { r.s("HE MCS "); r.d(code as u64); }
            _ => r.s("mod?"),
        }
        r.s(" bw");
        r.d(match (rnf & RATE_MCS_CHAN_WIDTH_MSK) >> RATE_MCS_CHAN_WIDTH_POS {
            0 => 20, 1 => 40, 2 => 80, 3 => 160, _ => 0,
        });
        if rnf & RATE_MCS_SGI_MSK != 0 { r.s(" sgi"); }
        if rnf & RATE_MCS_LDPC_MSK != 0 { r.s(" ldpc"); }
        let kbit = Self::rate_kbit(rnf);
        if kbit > 0 {
            r.s(" -> ");
            r.kbit_as_mbit(kbit);
            r.s(" Mbit");
        }
        r.c(b'\n');
    }

    // Build and publish the status snapshot. Called from the resident loop once
    // a second; everything it prints is already counted, so this only formats.
    fn publish_report(&mut self, now_ms: u64) {
        // Throughput + airtime over the window that just closed.
        let win = now_ms.saturating_sub(self.st.win_start_ms).max(1);
        self.st.tput_tx_kbit = ((self.st.tx_bytes - self.st.win_tx_bytes) * 8 / win) as u32;
        self.st.tput_rx_kbit = ((self.st.rx_bytes - self.st.win_rx_bytes) * 8 / win) as u32;
        self.st.airtime_pct =
            ((self.st.tx_airtime_us - self.st.win_airtime_us) / (win * 10)) as u32;
        self.st.rx_airtime_pct =
            ((self.st.rx_airtime_us - self.st.win_rx_airtime_us) / (win * 10)) as u32;
        self.st.passes_per_s =
            (self.st.loop_iters.wrapping_sub(self.st.win_loop_iters) as u64 * 1000 / win) as u32;
        if self.st.tput_tx_kbit > self.st.peak_tput_tx_kbit {
            self.st.peak_tput_tx_kbit = self.st.tput_tx_kbit;
        }
        if self.st.passes_per_s > self.st.peak_passes_per_s {
            self.st.peak_passes_per_s = self.st.passes_per_s;
        }
        self.st.win_start_ms = now_ms;
        self.st.win_tx_bytes = self.st.tx_bytes;
        self.st.win_rx_bytes = self.st.rx_bytes;
        self.st.win_airtime_us = self.st.tx_airtime_us;
        self.st.win_rx_airtime_us = self.st.rx_airtime_us;
        self.st.win_loop_iters = self.st.loop_iters;
        // The profile is per WINDOW, not cumulative: an average over the whole
        // uptime would drown the loaded second in idle ones.
        let passes = self.st.prof_passes.max(1);
        self.st.prof_work_pp = self.st.prof_work_us / passes;
        self.st.prof_sleep_pp = self.st.prof_sleep_us / passes;
        self.st.prof_drain_pp = self.st.prof_rx_us / passes;
        self.st.prof_us_frame = if self.st.prof_frames > 0 {
            self.st.prof_rx_us / self.st.prof_frames
        } else { 0 };
        // Keep the profile of the BUSIEST second, not just the last one. A load
        // generator holds the terminal for its whole run, so by the time anyone
        // can type `wlan` the live window is idle again and shows nothing.
        if self.st.tput_rx_kbit > self.st.peak_tput_rx_kbit {
            self.st.peak_tput_rx_kbit = self.st.tput_rx_kbit;
            self.st.peak_work_pp = self.st.prof_work_pp;
            self.st.peak_sleep_pp = self.st.prof_sleep_pp;
            self.st.peak_drain_pp = self.st.prof_drain_pp;
            self.st.peak_us_frame = self.st.prof_us_frame;
            self.st.peak_prof_passes = self.st.prof_passes;
            // Taken WITH the peak, not as separate maxima: three maxima from
            // three different seconds would describe a second that never
            // happened, and the question is what the fastest one looked like.
            self.st.pk_rx_airtime_pct = self.st.rx_airtime_pct;
            self.st.pk_tx_airtime_pct = self.st.airtime_pct;
            self.st.pk_retry_pct = if self.st.tx_frames > 0 {
                (self.st.tx_retries as u64 * 100 / self.st.tx_frames as u64) as u32
            } else {
                0
            };
            self.st.pk_drain = self.st.rx_drain_max;
            self.st.pk_pass_empty = self.st.win_pass_empty;
            self.st.pk_pass_few = self.st.win_pass_few;
            self.st.pk_pass_many = self.st.win_pass_many;
            self.st.pk_pass_burst = self.st.win_pass_burst;
            self.st.pk_gap_max_us = self.st.win_gap_max_us;
        }
        // Reset only AFTER the snapshot above has read them. The first version
        // cleared them 36 lines earlier, with the window's other state, so the
        // snapshot copied zeros — and a diagnostic that reports zero reads as
        // "nothing happened" rather than "the instrument is broken".
        self.st.win_pass_empty = 0;
        self.st.win_pass_few = 0;
        self.st.win_pass_many = 0;
        self.st.win_pass_burst = 0;
        self.st.win_gap_max_us = 0;
        self.st.prof_work_us = 0;
        self.st.prof_rx_us = 0;
        self.st.prof_sleep_us = 0;
        self.st.prof_passes = 0;
        self.st.prof_frames = 0;

        let mut r = Rep::new();
        r.s("wifi_ax200 ");
        r.s(DRIVER_VERSION);
        r.s("  up ");
        r.d((now_ms.saturating_sub(self.st.start_ms)) / 1000);
        r.s(" s\n");

        r.s("state    assoc=");
        r.s(if self.assoc_aid != 0 { "yes" } else { "NO" });
        r.s(" authorized=");
        r.s(if self.authorized { "yes" } else { "NO" });
        r.s(" qos/ht=");
        r.s(if self.qos { "yes" } else { "NO" });
        r.s(" aid=");
        r.d(self.assoc_aid as u64);
        r.c(b'\n');

        r.s("ap       ");
        r.mac(&self.target_bssid);
        r.s(" \"");
        for &b in &self.target_ssid[..self.target_ssid_len as usize] {
            r.c(if (0x20..0x7f).contains(&b) { b } else { b'?' });
        }
        r.s("\" ch ");
        r.d(self.target_chan as u64);
        r.s(if self.target_band == PHY_BAND_5_U8 { " (5 GHz)" } else { " (2.4 GHz)" });
        r.s(" rssi ");
        r.i(self.target_rssi as i32);
        r.s(" dtim ");
        r.d(self.target_dtim_period as u64);
        r.c(b'\n');

        r.s("ap ht    ");
        if self.target_ht.present {
            r.s("cap 0x");
            r.hex(self.target_ht.cap_info as u32, 4);
            r.s(" mcs 0x");
            r.hex(self.target_ht.mcs_rx[1] as u32, 2);
            r.hex(self.target_ht.mcs_rx[0] as u32, 2);
            r.s(" prot 0x");
            r.hex(self.protection_flags(), 8);
            r.s(" (op-mode 0x");
            r.hex(self.target_ht.ht_op_mode as u32, 4);
            r.s(self.target_ht.erp_protect.then_some(", ERP").unwrap_or(""));
            r.c(b')');
            r.s(" ampdu f/d ");
            r.d(self.target_ht.ampdu_factor as u64);
            r.c(b'/');
            r.d(self.target_ht.ampdu_density as u64);
            // The negotiated width, and WHY — a link that quietly fell back to
            // 20 MHz looks identical to one that never tried.
            if self.use_vht80() {
                r.s("  width 80 MHz VHT (seg0 ch ");
                r.d(self.vht_oper().1 as u64);
                r.s(", ctrl-pos 0x");
                r.hex(self.ctrl_pos() as u32, 1);
                r.c(b')');
            } else if self.use_ht40() {
                r.s("  width 40 MHz (sec ");
                r.s(if self.target_ht.sec_chan_offs == IEEE80211_HT_PARAM_CHA_SEC_BELOW
                    { "below" } else { "above" });
                // Say WHOSE decision it was. "AP has VHT but runs HT" blamed
                // the AP even when the reason was our own `vht` switch.
                r.s(if !self.want_vht && self.target_ht.vht {
                    ", AP offers VHT — `wlan set vht on` to use it)"
                } else if self.target_ht.vht {
                    ", AP has VHT but runs HT)"
                } else {
                    ")"
                });
            } else {
                r.s("  width 20 MHz (");
                // WHOSE decision — `ht40` is off by DEFAULT, and this branch
                // printed "AP runs 20 only" for it. That sentence is what the
                // handover note rests on; it was our own switch talking.
                if !self.want_ht40 {
                    r.s("WE ask for 20 - `wlan set ht40 on`)");
                } else if self.target_ht.cap_info & IEEE80211_HT_CAP_SUP_WIDTH_20_40 == 0 {
                    r.s("AP cannot do 40)");
                } else {
                    r.s("AP runs 20 only)");
                }
            }
        } else {
            r.s("NONE - AP advertised no HT element, legacy rates only");
        }
        r.c(b'\n');

        // The EDCA table we hand the firmware, so "QoS BSS" is checkable.
        r.s("ap qos   ");
        if self.target_ht.wmm {
            for (i, name) in ["vo", "vi", "be", "bk"].iter().enumerate() {
                let (aifs, cw_min, cw_max, txop) = self.target_ht.edca[i];
                r.s(name);
                r.s(" a");
                r.d(aifs as u64);
                r.s(" cw");
                r.d(cw_min as u64);
                r.c(b'/');
                r.d(cw_max as u64);
                r.s(" txop ");
                r.d(txop.saturating_mul(32) as u64);
                r.s("  ");
            }
        } else {
            r.s("AP sent no WMM parameter element");
        }
        r.s(" tgn ");
        r.s(if self.target_ht.present { "yes" } else { "no" });
        r.c(b'\n');

        // The bytes the width decision rests on, so the line above can be
        // checked instead of believed.
        if self.target_ht.present {
            let (cw, seg0, from_he) = self.vht_oper();
            r.s("ap width sec ");
            r.s(match self.target_ht.sec_chan_offs {
                IEEE80211_HT_PARAM_CHA_SEC_ABOVE => "above",
                IEEE80211_HT_PARAM_CHA_SEC_BELOW => "below",
                _ => "none",
            });
            r.s("  vht-op cw ");
            r.d(cw as u64);
            r.s(" seg0 ");
            r.d(seg0 as u64);
            r.s(if from_he { " (from HE oper)" } else { " (from VHT oper)" });
            r.s("  he ");
            if self.target_ht.he {
                r.s("cap yes, oper 0x");
                r.hex(self.target_ht.he_oper_params, 8);
            } else {
                r.s("absent");
            }
            r.c(b'\n');
        }

        // Aggregation is the single biggest throughput lever, so it gets its own
        // line. `held` standing still while `buffered` climbs is the healthy
        // picture; `stalls` counts holes the AP never closed, which is the one
        // failure mode a reorder buffer can add to a working link.
        let live = ba::sessions().iter().filter(|s| s.active()).count();
        if live > 0 {
            let (delivered, buffered, dups, old_sn, stalls, held) = ba::totals();
            // One line PER live session: which TID carries the traffic is the
            // question the single-session report could not answer — it showed
            // tid 6 with `delivered 0` while every byte arrived on tid 0.
            for sess in ba::sessions().iter().filter(|s| s.active()) {
                r.s("aggr     A-MPDU on  baid ");
                r.d(sess.baid as u64);
                r.s(" tid ");
                r.d(sess.tid as u64);
                r.s(" win ");
                r.d(ba::BA_WIN as u64);
                r.s("  delivered ");
                r.d(sess.delivered as u64);
                r.s(" held ");
                r.d(sess.stored as u64);
                r.s(" timeout ");
                r.d(sess.timeout_tu as u64);
                r.s(" TU\n");
            }
            r.s("aggr     total delivered ");
            r.d(delivered as u64);
            r.s(" buffered ");
            r.d(buffered as u64);
            r.s(" held ");
            r.d(held as u64);
            r.s(" dups ");
            r.d(dups as u64);
            r.s(" old-sn ");
            r.d(old_sn as u64);
            r.s(" stalls ");
            r.d(stalls as u64);
            r.s(" (holes the AP never closed)  live ");
            r.d(live as u64);
            r.s(" of ");
            r.d(ba::NUM_TIDS as u64);
            r.s(" tids  accepted ");
            r.d(self.st.addba_accepted as u64);
            r.c(b'\n');
        } else {
            r.s(if self.ba_fw_broken && self.want_ampdu {
                "aggr     A-MPDU wants RX_BAID_ALLOCATION_CONFIG_CMD, not ported — declining; declined "
            } else if self.want_ampdu {
                "aggr     A-MPDU on but no session yet; declined "
            } else {
                "aggr     A-MPDU off (`ampdu: on` in sys/config/wifi); declined "
            });
            r.d(self.st.addba_declined as u64);
            r.s(" accepted ");
            r.d(self.st.addba_accepted as u64);
            r.s(" of ");
            r.d(self.st.addba_seen as u64);
            r.s(" asked");
            if self.st.addba_seen > self.st.addba_declined + self.st.addba_accepted {
                r.s("  <- UNANSWERED");
            }
            if self.st.addba_timeouts > 0 {
                r.s("  fw-silent ");
                r.d(self.st.addba_timeouts as u64);
            }
            r.c(b'\n');
        }

        // The 4-way, step by step. A stalled association always stops at one
        // specific rung, and which one names the culprit: no ready = never
        // associated; ready but no eapol in = the AP stayed silent; eapol in
        // but none out = wifid is not answering; keys but not authorized =
        // wifid did not finish.
        r.s("4-way    ready-sent ");
        r.d(self.st.ready_sent as u64);
        r.s("  eapol in ");
        r.d(self.st.rx_eapol as u64);
        r.s(" out ");
        r.d(self.st.tx_eapol as u64);
        r.s("  keys ");
        r.d(self.st.keys_set as u64);
        r.s("/2  authorized ");
        r.s(if self.authorized { "yes" } else { "NO" });
        // The 4-way installs one GTK; every further one is a rekey. A rekey
        // only ever announced itself as a log LINE, and background output is
        // pinned to the first loop window — so in any other window it was
        // invisible, and it was very nearly missed. A standing number cannot
        // scroll past.
        if self.st.gtk_installs > 1 {
            r.s("  group rekeys ");
            r.d((self.st.gtk_installs - 1) as u64);
        }
        if self.st.tx_eapol_dropped > 0 {
            r.s("  DROPPED ");
            r.d(self.st.tx_eapol_dropped as u64);
            r.s(" (never reached the air)");
        }
        if self.st.ready_sent > 0 && self.st.rx_eapol > 0 && self.st.tx_eapol == 0 {
            r.s("  <- wifid never answered msg1");
        }
        r.c(b'\n');

        Self::rep_rate(&mut r, "rate tx  ", self.st.last_tx_rate);
        Self::rep_rate(&mut r, "rate rx  ", self.st.last_rx_rate);
        Self::rep_rate(&mut r, "rate ini ", self.st.last_init_rate);

        r.s("tput     tx ");
        r.kbit_as_mbit(self.st.tput_tx_kbit);
        r.s(" Mbit/s  rx ");
        r.kbit_as_mbit(self.st.tput_rx_kbit);
        r.s(" Mbit/s  own airtime ");
        r.d(self.st.airtime_pct as u64);
        r.s("%  (live 1 s window)\n");
        // Survives the end of the load, so one `wlan` AFTER a blocking transfer
        // still answers "how fast did it actually go".
        r.s("peak     tx ");
        r.kbit_as_mbit(self.st.peak_tput_tx_kbit);
        r.s(" Mbit/s  rx ");
        r.kbit_as_mbit(self.st.peak_tput_rx_kbit);
        r.s(" Mbit/s  ");
        r.d(self.st.peak_passes_per_s as u64);
        r.s(" passes/s  (best window since driver start)\n");
        // The line that decides whether the AIR was the limit in that fastest
        // second. Near 100 = the channel was full and aggregation is the lever.
        // Well under it = the channel was idle and the ceiling is on our side.
        r.s("air      in that window: rx ");
        r.d(self.st.pk_rx_airtime_pct as u64);
        r.s("% tx ");
        r.d(self.st.pk_tx_airtime_pct as u64);
        r.s("% (incl. IFS+backoff)  retries ");
        r.d(self.st.pk_retry_pct as u64);
        r.s("%  drain ");
        r.d(self.st.pk_drain as u64);
        r.c(b'\n');
        // The shape of the arrivals in that same second. Mostly-empty passes
        // with a long gap = the AP is not delivering and the ceiling is not
        // ours. Passes consistently carrying frames = we are the bottleneck.
        r.s("arrive   passes empty ");
        r.d(self.st.pk_pass_empty as u64);
        r.s("  1-3 ");
        r.d(self.st.pk_pass_few as u64);
        r.s("  4-15 ");
        r.d(self.st.pk_pass_many as u64);
        r.s("  16+ ");
        r.d(self.st.pk_pass_burst as u64);
        r.s("  longest silence ");
        r.d((self.st.pk_gap_max_us / 1000) as u64);
        r.s(" ms\n");

        r.s("tx       frames ");
        r.d(self.st.tx_frames as u64);
        r.s(" bytes ");
        r.d(self.st.tx_bytes / 1024);
        r.s(" KiB  blocked ");
        r.d(self.st.tx_blocked as u64);
        r.s(" bytes / ");
        r.d(self.st.tx_blocked_ring as u64);
        r.s(" ring  inflight ");
        r.d(self.data_bytes_in_flight as u64 / 1024);
        r.s(" KiB/");
        r.d(TX_INFLIGHT_BYTES as u64 / 1024);
        r.s(" KiB, ");
        r.d(self.data_in_flight as u64);
        r.c(b'/');
        r.d(TX_INFLIGHT_MAX as u64);
        r.s(" slots peak ");
        r.d(self.st.inflight_peak as u64);
        if self.st.tx_wd_recoveries > 0 {
            r.s("  WD-RECLAIM ");
            r.d(self.st.tx_wd_recoveries as u64);
        }
        if self.st.inflight_corrections > 0 {
            r.s("  corrected ");
            r.d(self.st.inflight_corrections as u64);
            r.s(" (counter drifted from the firmware's read pointer)");
        }
        r.c(b'\n');

        r.s("tx resp  ok ");
        r.d(self.st.tx_ok as u64);
        r.s(" fail ");
        r.d(self.st.tx_fail as u64);
        r.s(" retries ");
        r.d(self.st.tx_retries as u64);
        r.s(" (");
        r.pct(self.st.tx_retries as u64, (self.st.tx_ok + self.st.tx_fail).max(1) as u64);
        r.s(" of frames) fail-streak ");
        r.d(self.tx_fail_streak as u64);
        r.c(b'/');
        r.d(self.tx_fail_streak_peak as u64);
        r.s(" rts-fail ");
        r.d(self.st.tx_rts_fail as u64);
        r.s(" last-status 0x");
        r.hex(self.st.last_status as u32, 4);
        r.c(b'\n');

        // Is the firmware aggregating what we hand it? `frame_count` says so
        // per response (1 = single MPDU), the compressed block-ack says it
        // again from the other side. Without this line "no TX aggregation" was
        // an assumption; `tx frames` counts what we QUEUED, not what went on
        // the air as one aggregate.
        r.s("tx agg   subframes ");
        r.d(self.st.tx_subframes);
        r.s(" over ");
        r.d((self.st.tx_ok + self.st.tx_fail) as u64);
        r.s(" resp; aggregated ");
        r.d(self.st.tx_agg_resp as u64);
        r.s(" max ");
        r.d(self.st.tx_agg_max as u64);
        r.s("  ba-notif ");
        r.d(self.st.ba_notifs as u64);
        r.s(" txed ");
        r.d(self.st.ba_txed);
        r.s(" done ");
        r.d(self.st.ba_done);
        // Whether the aggregated TX return path RAN. Without it the slots those
        // MPDUs sat in are never freed, and the queue wedges at its own depth.
        r.s(" reclaims ");
        r.d(self.st.ba_reclaims as u64);
        if self.st.ba_tfd_over > 0 {
            r.s("  TFD-OVERFLOW ");
            r.d(self.st.ba_tfd_over as u64);
            r.s(" (entries past CBA_TFD_MAX — slots not reclaimed)");
        }
        r.c(b'\n');

        r.s("rx       frames ");
        r.d(self.st.rx_frames as u64);
        r.s(" bytes ");
        r.d(self.st.rx_bytes / 1024);
        r.s(" KiB  ip ");
        r.d(self.st.rx_ip as u64);
        r.s(" eapol ");
        r.d(self.st.rx_eapol as u64);
        r.s(" mgmt ");
        r.d(self.st.rx_mgmt as u64);
        r.s(" to-us ");
        r.d(self.st.rx_to_us as u64);
        r.s(" undecoded ");
        r.d(self.st.rx_undecoded as u64);
        r.s(" drain-peak ");
        r.d(self.st.rx_drain_max as u64);
        r.c(b'/');
        r.d(RX_NUM_RBS as u64);
        r.s(" pool-exhausted ");
        r.d(self.st.rx_pool_exhausted as u64);
        r.c(b'\n');

        // The poll rate, and what it implies. The loop asks for a 1 ms sleep
        // while busy, but a fiber whose core has nothing else runnable idles in
        // HLT until the next 100 Hz worker tick — so the REAL period can be 10 ms,
        // and then one pass' worth of TX_INFLIGHT_BYTES is a hard throughput ceiling.
        // Printing the implied ceiling makes that visible instead of theoretical.
        // The cost of one pass, in the only unit that answers "are we CPU-bound":
        // microseconds. work = everything between waking and sleeping; drain =
        // the RX half of it; slept = what a 1 ms request really took. If work
        // approaches the wall time per pass, more air rate buys nothing.
        let work_pp = self.st.prof_work_pp;
        let sleep_pp = self.st.prof_sleep_pp;
        let wall_pp = (work_pp + sleep_pp).max(1);
        // Encrypted RX, the one number that separates "the AP went quiet" from
        // "the AP is talking and we cannot read it". mic-fail is what Linux
        // drops; sec-none is a protected frame the firmware never decrypted.
        r.s("crypto   protected ");
        r.d(self.st.rx_prot as u64);
        r.s("  mic-fail ");
        r.d(self.st.rx_mic_fail as u64);
        r.s("  sec-none ");
        r.d(self.st.rx_sec_none as u64);
        r.s("  never-decrypted ");
        r.d(self.st.rx_undecrypted as u64);
        r.c(b'\n');
        r.s("cpu      work ");
        r.d(work_pp);
        r.s(" us/pass = ");
        r.d(work_pp * 100 / wall_pp);
        r.s("% of ");
        r.d(wall_pp);
        r.s(" us wall; drain ");
        r.d(self.st.prof_drain_pp);
        r.s(" us");
        if self.st.prof_us_frame > 0 {
            r.s(" (");
            r.d(self.st.prof_us_frame);
            r.s(" us/frame)");
        }
        r.s("; sleep(1ms) took ");
        r.d(sleep_pp);
        r.s(" us\n");

        // The same numbers from the busiest second — the only ones that matter
        // when the load generator has the terminal.
        let pw = self.st.peak_work_pp;
        let ps = self.st.peak_sleep_pp;
        let pwall = (pw + ps).max(1);
        r.s("cpu peak work ");
        r.d(pw);
        r.s(" us/pass = ");
        r.d(pw * 100 / pwall);
        r.s("% of ");
        r.d(pwall);
        r.s(" us wall; drain ");
        r.d(self.st.peak_drain_pp);
        r.s(" us (");
        r.d(self.st.peak_us_frame);
        r.s(" us/frame); sleep(1ms) took ");
        r.d(ps);
        r.s(" us; passes ");
        r.d(self.st.peak_prof_passes);
        r.c(b'\n');

        r.s("loop     ");
        r.d(self.st.passes_per_s as u64);
        r.s(" passes/s (asks for 1 ms when busy) busy ");
        r.pct(self.st.loop_busy as u64, self.st.loop_iters.max(1) as u64);
        r.s(" iters ");
        r.d(self.st.loop_iters as u64);
        r.s(" deauth ");
        r.d(self.st.deauth as u64);
        r.c(b'\n');
        r.s("tx cap   ");
        r.d(TX_INFLIGHT_BYTES as u64 / 1024);
        r.s(" KiB/pass * ");
        r.d(self.st.passes_per_s as u64);
        r.s(" passes/s = ceiling ");
        r.kbit_as_mbit(
            (TX_INFLIGHT_BYTES as u64 * self.st.peak_passes_per_s as u64 * 8 / 1000) as u32,
        );
        r.s(" Mbit/s (at peak pass rate), or ");
        r.d(TX_INFLIGHT_MAX as u64 * self.st.peak_passes_per_s as u64);
        r.s(" frames/s at the ring guard\n");

        // What else the scan saw. The target is picked by RSSI alone, which on a
        // dual-band mesh always means the near 2.4 GHz node — this line is how we
        // find out whether a faster band was on the table.
        r.s("policy   power ");
        r.s(if self.want_power_save { "save (ps: on)" } else { "CAM (always on)" });
        r.s(", btcoex ");
        r.s(if self.want_bt_coex { "on" } else { "off" });
        r.s(", settle ");
        r.d(self.settle_ms as u64);
        r.s(" ms");
        r.s(", band ");
        r.s(match self.band_pref {
            BAND_PREF_5 => "5-only",
            BAND_PREF_24 => "2.4-only",
            _ => "auto",
        });
        r.s(", picked because ");
        r.s(match self.pick_reason {
            PICK_5G_PREFERRED => "5 GHz was above the RSSI floor",
            PICK_BAND_FORCED => "the band was forced by config",
            PICK_SSID_FILTERED => "NO AP matched `ssid:` in sys/config/wifi (fell back to loudest)",
            PICK_5G_TOO_WEAK => "2.4 GHz was far stronger than the 5 GHz AP",
            _ => "it was the strongest of our SSID",
        });
        r.c(b'\n');

        // Physical addresses of the rings. A driver that works with a USB
        // dongle plugged in and not without it is not talking to the dongle —
        // but the dongle allocates memory first, so OUR buffers land somewhere
        // else. This project already has one address-dependent fault on record
        // (MMIO map_page against 1 GB huge pages), so the addresses belong in
        // any report that gets compared across boots.
        // RX ring bookkeeping. "Receives for a while, then stops" is the
        // signature of a firmware that ran out of buffers, and only these three
        // numbers moving together show that they are being handed back.
        // Firmware assert state. The dump only ran from the TX-stall watchdog,
        // which needs in-flight at the cap — a firmware that died at 6 in-flight
        // never triggered it and its error table was never looked at.
        r.s("fw       ");
        match self.fw_assert {
            0 => r.s("not checked yet"),
            1 => r.s("no assert"),
            v => { r.s("LMAC ASSERT id=0x"); r.hex(v, 8); }
        }
        r.c(b'\n');

        r.s("radio    ");
        if self.rf_killed() {
            r.s("RF-KILL asserted — the radio is OFF, no frame can arrive\n");
        } else {
            r.s("on\n");
        }

        r.s("rxring   read ");
        r.d(self.rxq_read as u64);
        r.s("  closed ");
        r.d(self.closed_rb() as u64);
        r.s("  free-bd-write ");
        r.d(self.free_bd_write as u64);
        r.s(" (hw sees ");
        r.d(((self.free_bd_write & (NUM_RBDS as u32 - 1)) & !0x7) as u64);
        r.s(", pool ");
        r.d(RX_NUM_RBS as u64);
        r.s(", fw holds ");
        let held = self.rb_in_fw.iter().filter(|b| **b).count();
        r.d(held as u64);
        r.s(")\n");
        // Only when something is off — a ring that reports its normal state
        // every time is not a report.
        if self.st.rx_wd_fires > 0 || self.st.rb_bad_vid > 0 || self.st.rb_double_post > 0 {
            r.s("rxfault  silence-watchdog ");
            r.d(self.st.rx_wd_fires as u64);
            r.s(" (");
            r.d(self.st.rx_wd_dry as u64);
            r.s(" consecutive with nothing to re-arm)  bad-vid ");
            r.d(self.st.rb_bad_vid as u64);
            r.s("  double-post ");
            r.d(self.st.rb_double_post as u64);
            r.c(b'\n');
        }

        r.s("dma      mmio h");
        r.d(self.mmio as u64);
        r.s("  rxq 0x");
        r.hex((self.rxq_bd.phys >> 32) as u32, 8);
        r.hex(self.rxq_bd.phys as u32, 8);
        r.s("  rb0 0x");
        r.hex((self.rb_pool[0].phys >> 32) as u32, 8);
        r.hex(self.rb_pool[0].phys as u32, 8);
        r.s("  data 0x");
        r.hex((self.data_tfd.phys >> 32) as u32, 8);
        r.hex(self.data_tfd.phys as u32, 8);
        r.c(b'\n');

        // What the firmware reports about beacons — because WE no longer see
        // them. Once associated it stops passing them to the host (Linux sets
        // MAC_FILTER_IN_BEACON only while unassociated, mac-ctxt.c:704), so
        // silence here is normal and this notification is the only beacon news
        // there is. `losses` is how often it declared the AP gone: a mesh that
        // steers between router and repeater shows up exactly there.
        r.s("beacons  fw notifs ");
        r.d(self.st.mb_notifs as u64);
        r.s("  missed ");
        r.d(self.st.mb_consec as u64);
        r.s(" (");
        r.d(self.st.mb_since_rx as u64);
        r.s(" since RX)  expected ");
        r.d(self.st.mb_expected as u64);
        r.s(" got ");
        r.d(self.st.mb_received as u64);
        r.s("  declared-lost ");
        r.d(self.st.mb_losses as u64);
        r.c(b'\n');

        // The timing the associated MAC context was built from — captured from
        // the LAST pre-association beacon, which is the only one we ever see.
        r.s("sync     pre-assoc beacon ");
        r.s(if self.sync_ok { "ok" } else { "none (firmware got a made-up wake schedule)" });
        r.s("  tsf 0x");
        r.hex((self.sync_tsf >> 32) as u32, 8);
        r.hex(self.sync_tsf as u32, 8);
        r.s("  gp2 0x");
        r.hex(self.sync_device_ts, 8);
        r.s("  dtim-count ");
        r.d(self.sync_dtim_count as u64);
        r.c(b'\n');

        r.s("scan     ");
        r.d(self.n_aps as u64);
        r.s(" APs; same SSID on the other band: ");
        if self.alt_valid {
            r.s("ch ");
            r.d(self.alt_chan as u64);
            r.s(" rssi ");
            r.i(self.alt_rssi as i32);
            r.s(if self.alt_ht { " HT yes " } else { " HT no " });
            r.mac(&self.alt_bssid);
        } else {
            r.s("none");
        }
        r.c(b'\n');

        let n = r.n;
        host::driver_report(&r.b[..n]);
    }

    // ── gen2 mgmt-frame TX (iwl_txq_gen2_tx + iwl_txq_gen2_build_tx) ──────
    // Transmit one 802.11 management frame on the AP station's queue. Builds a
    // device TX command — short iwl_cmd_header (TX_CMD, group 0) + iwl_tx_cmd_v9
    // (len/flags/host-rate) + the frame — and lays it across the TFD as two TBs
    // (TB0 = first-TB staging with the first 20 bytes, TB1 = the remainder from
    // cmd_data), fills the byte-count table, bumps the write pointer and rings the
    // doorbell. For AX200 (< BZ) mgmt frames use the host rate (IWL_TX_FLAGS_CMD_RATE).
    // Generic gen2 TX onto a given queue (mgmt or data): build the dev TX command
    // (short header + tx_cmd_v9 + frame) across the TFD's two TBs, fill the
    // byte-count table, bump the write pointer and ring the doorbell. Returns the
    // advanced write pointer. (The 802.11 frame is built by the caller.)
    fn tx_raw(&self, qid: u16, wptr: u32, qsize: usize, tfd_ring: Dma, first_tb: Dma, payload: Dma, bc: Dma, flags: u32, frame: &[u8], hdr_len: usize) -> u32 {
        let idx = (wptr & (qsize as u32 - 1)) as usize;
        let mut buf = [0u8; TX_PAYLOAD_STRIDE]; // dev_cmd header + tx_cmd + full frame
        buf[0] = TX_CMD;
        buf[1] = 0; // group 0 (short header)
        let seq = (((qid) & 0x1f) << 8) | (idx as u16 & 0xff);
        buf[2..4].copy_from_slice(&seq.to_le_bytes());
        let frame_len = frame.len() as u16;
        put_u16(&mut buf, TXC_OFF_LEN, frame_len);
        // offload_assist (iwl_mvm_tx_csum): the 802.11 header length in 2-byte
        // words for EVERY frame, plus PAD when it is not a multiple of 4 — then
        // 2 bytes go between header and payload so the payload is DWORD-aligned
        // (Linux does that alignment in the transport's TB1). A QoS header is 26
        // bytes, so this is what makes the QoS data path work at all.
        let pad = if hdr_len % 4 != 0 { 2usize } else { 0 };
        let mut offload = ((hdr_len / 2) as u16) << TX_CMD_OFFLD_MH_SIZE_POS;
        if pad != 0 {
            offload |= TX_CMD_OFFLD_PAD;
        }
        put_u16(&mut buf, TXC_OFF_OFFLOAD, offload);
        put_u32(&mut buf, TXC_OFF_FLAGS, flags);
        let rate = if self.target_band == PHY_BAND_24 as u8 {
            RATE_1M_CCK_ANT_A
        } else {
            RATE_6M_OFDM_ANT_A
        };
        put_u32(&mut buf, TXC_OFF_RATE, rate);
        buf[TXC_OFF_FRAME..TXC_OFF_FRAME + hdr_len].copy_from_slice(&frame[..hdr_len]);
        let body = TXC_OFF_FRAME + hdr_len + pad; // pad bytes stay zero
        buf[body..body + frame.len() - hdr_len].copy_from_slice(&frame[hdr_len..]);
        let total = body + frame.len() - hdr_len;

        // Per-slot staging: TB0 = this slot's first-TB buffer (first 20 bytes),
        // TB1 = this slot's payload region (the rest). Every in-flight TFD has
        // its own payload region, so a later frame never overwrites an earlier
        // one before the firmware has DMA'd it.
        let pl_off = (idx * TX_PAYLOAD_STRIDE) as u32;
        host::dma_write_buf(payload.handle, pl_off, &buf[..total]);
        let ftb_off = (idx * IWL_FIRST_TB_SIZE_ALIGN) as u32;
        host::dma_write_buf(first_tb.handle, ftb_off, &buf[..IWL_FIRST_TB_SIZE]);

        let mut tfd = [0u8; TFH_TFD_SIZE];
        put_tfh_tb(&mut tfd, 0, IWL_FIRST_TB_SIZE as u16, first_tb.phys + ftb_off as u64);
        put_tfh_tb(&mut tfd, 1, (total - IWL_FIRST_TB_SIZE) as u16, payload.phys + pl_off as u64 + IWL_FIRST_TB_SIZE as u64);
        tfd[0..2].copy_from_slice(&2u16.to_le_bytes()); // num_tbs
        host::dma_write_buf(tfd_ring.handle, (idx * TFH_TFD_SIZE) as u32, &tfd);

        let bc_ent = ((frame_len as u32 + 3) / 4) as u16;
        host::dma_write_buf(bc.handle, (idx * 2) as u32, &bc_ent.to_le_bytes());

        host::fence();
        let next = (wptr + 1) & (MAX_TFD_QUEUE_SIZE - 1);
        self.w32(HBUS_TARG_WRPTR, next | ((qid as u32) << 16));
        next
    }

    fn tx_mgmt_frame(&mut self, frame: &[u8]) {
        self.mgmt_write_ptr = self.tx_raw(
            self.mgmt_queue_id,
            self.mgmt_write_ptr,
            IWL_MGMT_QUEUE_SIZE,
            self.mgmt_tfd,
            self.mgmt_first_tb,
            self.mgmt_payload,
            self.mgmt_bc_tbl,
            IWL_TX_FLAGS_ENCRYPT_DIS | IWL_TX_FLAGS_CMD_RATE,
            frame,
            DOT11_HDR_LEN,
        );
    }

    /// An ADDBA request arrived (`req` = category, action, dialog token,
    /// parameter set (2), timeout (2), start sequence control (2)).
    ///
    /// Order matters: the firmware has to know about the session BEFORE the AP
    /// starts aggregating, because it is the firmware that stamps each frame
    /// with the BAID and the window position we reorder by. So we ask it first
    /// (`iwl_mvm_fw_baid_op_sta`) and answer the AP only once it has agreed —
    /// the response is sent from the ADD_STA reply path, which also carries the
    /// BAID. A firmware that refuses gets us back to declining, which is a
    /// working link, just a slow one.
    fn ba_request(&mut self, req: &[u8; 12]) {
        let params = u16::from_le_bytes([req[3], req[4]]);
        let tid = ((params & BA_PARAM_TID_MASK) >> BA_PARAM_TID_SHIFT) as u8;
        let ssn = u16::from_le_bytes([req[7], req[8]]) >> BA_SSN_SHIFT;
        let dialog = req[2];
        let timeout = u16::from_le_bytes([req[5], req[6]]);
        let asked = (params & BA_PARAM_BUFSZ_MASK) >> BA_PARAM_BUFSZ_SHIFT;
        let immediate = params & BA_PARAM_POLICY_IMMEDIATE != 0;

        let decline = |me: &mut Self, status: u16| {
            let p = BaPending { tid, ssn, win: 1, dialog, timeout, asked_ms: 0 };
            me.ba_reply(&p, status);
            me.st.addba_declined = me.st.addba_declined.wrapping_add(1);
        };

        // ieee80211_process_addba_request, in its order.
        //
        // Block-ack is defined for TIDs 0-7; at IEEE80211_FIRST_TSPEC_TSID and
        // above Linux declines outright.
        if tid as usize >= ba::NUM_TIDS {
            decline(self, WLAN_STATUS_REQUEST_DECLINED);
            return;
        }
        // We only implement immediate block ack, and a buffer larger than the
        // HT maximum is a malformed request — both are INVALID_QOS_PARAM, not
        // a plain decline, so the AP learns WHY.
        if !immediate || asked as usize > IEEE80211_MAX_AMPDU_BUF_HT {
            decline(self, WLAN_STATUS_INVALID_QOS_PARAM);
            return;
        }
        if !self.want_ampdu || self.ba_fw_broken {
            decline(self, WLAN_STATUS_REQUEST_DECLINED);
            return;
        }

        // A repeat request on a live session. Same dialog token = the AP is
        // only updating the timeout; we have no way to change it in the
        // firmware, so we accept it unchanged and decline a real change,
        // WITHOUT disturbing the session.
        if let Some(sess) = ba::by_tid(tid) {
            if sess.active() {
                if sess.dialog == dialog {
                    let status = if sess.timeout_tu == timeout {
                        WLAN_STATUS_SUCCESS
                    } else {
                        WLAN_STATUS_REQUEST_DECLINED
                    };
                    let p = BaPending { tid, ssn, win: ba::BA_WIN as u16,
                                        dialog, timeout: sess.timeout_tu, asked_ms: 0 };
                    self.ba_reply(&p, status);
                    return;
                }
                // A genuinely new session on a TID that already has one. Linux
                // tears the old one down FIRST (agg-rx.c:379) and sends no
                // DELBA for it — the AP is replacing it on purpose. Skipping
                // this is what leaked the firmware BAID: we overwrote the slot
                // and could no longer name the old id to free it.
                host::print("[ax200] ADDBA replaces the session on tid ");
                host::print_dec(tid as u32);
                host::print(" — removing the old one first\n");
                self.ba_stop(tid, false, 0);
            }
        }

        // Our window, not the AP's: it asks for up to 64 and a responder may
        // answer with less. 32 already collapses the per-frame overhead and
        // halves what we hold in linear memory.
        let win = (if asked == 0 { IEEE80211_MAX_AMPDU_BUF_HT as u16 } else { asked } as usize)
            .min(ba::BA_WIN)
            .max(1) as u16;
        self.ba_pending = Some(BaPending { tid, ssn, win, dialog, timeout,
                                           asked_ms: host::now_ms() });
        if self.st.addba_seen <= 3 {
            host::print("[ax200] ADDBA request tid ");
            host::print_dec(tid as u32);
            host::print(" — asking firmware for a session\n");
        }

        // iwl_mvm_fw_baid_op (mvm/sta.c): the capability decides which of the
        // two commands opens the session. Skipping this branch is what cost us
        // the link — the fallback command is not ignored by a firmware that
        // wants the other one, it stops completing transmissions.
        if fw_has_capa(IWL_UCODE_TLV_CAPA_BAID_ML_SUPPORT) {
            let mut c = [0u8; BAID_CFG_CMD_LEN];
            put_u32(&mut c, BAID_OFF_ACTION, IWL_RX_BAID_ACTION_ADD);
            // iwl_mvm_sta_fw_id_mask with link -1 on a non-MLD station is
            // simply BIT(sta_id) (mvm/mld-sta.c).
            put_u32(&mut c, BAID_OFF_STA_MASK, 1u32 << AP_STA_ID);
            c[BAID_OFF_ALLOC_TID] = tid;
            put_u16(&mut c, BAID_OFF_ALLOC_SSN, ssn);
            put_u16(&mut c, BAID_OFF_ALLOC_WIN, win);
            self.send_hcmd(DATA_PATH_GROUP, RX_BAID_ALLOCATION_CONFIG_CMD, &c);
        } else {
            let mut sc = [0u8; ADD_STA_CMD_LEN];
            sc[AS_OFF_ADD_MODIFY] = 1; // modify
            put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0);
            sc[AS_OFF_STA_ID] = AP_STA_ID;
            sc[AS_OFF_MODIFY_MASK] = STA_MODIFY_ADD_BA_TID;
            sc[AS_OFF_ADD_IMM_BA_TID] = tid;
            put_u16(&mut sc, AS_OFF_ADD_IMM_BA_SSN, ssn);
            put_u16(&mut sc, AS_OFF_RX_BA_WINDOW, win);
            self.send_hcmd(0, ADD_STA, &sc);
        }
    }

    /// End one TID's session: drop our buffer, free the firmware BAID, and —
    /// only when WE are the one ending it — tell the AP with a DELBA.
    ///
    /// `__ieee80211_stop_rx_ba_session`: the DELBA goes out only for
    /// `initiator == WLAN_BACK_RECIPIENT && tx`. A session the AP itself ended
    /// (DELBA received) or replaced (new ADDBA on the same TID) gets none.
    fn ba_stop(&mut self, tid: u8, tell_ap: bool, reason: u16) {
        let baid = match ba::by_tid(tid) {
            Some(s) if s.active() => {
                let b = s.baid;
                s.stop();
                b
            }
            _ => return,
        };
        self.ba_remove(tid, baid);
        if tell_ap {
            self.send_delba(tid, WLAN_BACK_RECIPIENT, reason);
        }
    }

    /// Every session down — the firmware's are gone with the association, and
    /// a BAID we no longer remember is one we can never free. Linux does this
    /// in `iwl_mvm_rm_sta`; we kept the station across a reconnect (MODIFY,
    /// not remove/re-add) and so have to do it explicitly.
    fn ba_stop_all(&mut self) {
        for tid in 0..ba::NUM_TIDS as u8 {
            if ba::by_tid(tid).map(|s| s.active()).unwrap_or(false) {
                if let Some(s) = ba::by_tid(tid) { s.stop(); }
            }
        }
        self.ba_pending = None;
    }

    /// DELBA action frame (802.11 §9.6.5.4): category, action, params, reason.
    fn send_delba(&mut self, tid: u8, initiator: u16, reason: u16) {
        let mut fr = [0u8; DOT11_HDR_LEN + 6];
        fr[0] = (DOT11_STYPE_ACTION << 4) | 0x00;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let b = DOT11_HDR_LEN;
        fr[b] = WLAN_CATEGORY_BACK;
        fr[b + 1] = WLAN_ACTION_DELBA;
        let params = (((tid as u16) << IEEE80211_DELBA_PARAM_TID_SHIFT)
                        & IEEE80211_DELBA_PARAM_TID_MASK)
            | ((initiator << IEEE80211_DELBA_PARAM_INITIATOR_SHIFT)
                        & IEEE80211_DELBA_PARAM_INITIATOR_MASK);
        put_u16(&mut fr, b + 2, params);
        put_u16(&mut fr, b + 4, reason);
        self.tx_mgmt_frame(&fr);
    }

    /// The allocation command answers with a bare BAID (iwl_rx_baid_cfg_resp).
    /// Unlike ADD_STA there is no status word: an error arrives as a value
    /// outside the map, exactly as Linux checks it.
    fn ba_on_baid_alloc(&mut self, baid: u32) {
        let p = match self.ba_pending.take() {
            Some(p) => p,
            None => return,
        };
        if baid >= IWL_MAX_BAID {
            self.ba_reply(&p, WLAN_STATUS_REQUEST_DECLINED);
            self.st.addba_declined = self.st.addba_declined.wrapping_add(1);
            host::print("[ax200] block-ack refused by firmware (baid 0x");
            host::print_hex32(baid);
            host::print(")\n");
            return;
        }
        if let Some(sess) = ba::by_tid(p.tid) {
            sess.start(baid as u8, p.tid, p.ssn, p.dialog, p.timeout);
        }
        self.ba_reply(&p, WLAN_STATUS_SUCCESS);
        self.st.addba_accepted = self.st.addba_accepted.wrapping_add(1);
        if self.st.addba_accepted <= 2 {
            host::print("[ax200] block-ack RX session up: baid ");
            host::print_dec(baid);
            host::print(" tid ");
            host::print_dec(p.tid as u32);
            host::print(" window ");
            host::print_dec(p.win as u32);
            host::print("\n");
        }
    }

    /// Tear the session down in the firmware too (STA_MODIFY_REMOVE_BA_TID).
    /// Dropping only our buffer would leave the firmware stamping BAIDs for a
    /// session the AP has ended, and the next ADDBA would find the slot taken.
    fn ba_remove(&mut self, tid: u8, baid: u8) {
        if fw_has_capa(IWL_UCODE_TLV_CAPA_BAID_ML_SUPPORT) {
            let mut c = [0u8; BAID_CFG_CMD_LEN];
            put_u32(&mut c, BAID_OFF_ACTION, IWL_RX_BAID_ACTION_REMOVE);
            // iwl_mvm_fw_baid_op_cmd (sta.c:2833) has a THIRD branch we had
            // skipped: at command version 1 the remove payload is a bare
            // `__le32 baid` (remove_v1), not sta_id_mask + tid. Sending the v2
            // form to a v1 firmware would have it read our station mask
            // (BIT(0) = 1) as the BAID to free — i.e. free the wrong session.
            if fw_cmd_ver(DATA_PATH_GROUP, RX_BAID_ALLOCATION_CONFIG_CMD) == 1 {
                put_u32(&mut c, BAID_OFF_STA_MASK, baid as u32);
            } else {
                put_u32(&mut c, BAID_OFF_STA_MASK, 1u32 << AP_STA_ID);
                put_u32(&mut c, BAID_OFF_REMOVE_TID, tid as u32);
            }
            self.send_hcmd(DATA_PATH_GROUP, RX_BAID_ALLOCATION_CONFIG_CMD, &c);
            return;
        }
        let mut sc = [0u8; ADD_STA_CMD_LEN];
        sc[AS_OFF_ADD_MODIFY] = 1; // modify
        put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0);
        sc[AS_OFF_STA_ID] = AP_STA_ID;
        sc[AS_OFF_MODIFY_MASK] = STA_MODIFY_REMOVE_BA_TID;
        sc[AS_OFF_REMOVE_IMM_BA_TID] = tid;
        self.send_hcmd(0, ADD_STA, &sc);
    }

    /// The firmware answered our ADD_STA. On a started session the status word
    /// carries the BAID that will appear in every aggregated frame; only then do
    /// we tell the AP it may aggregate.
    fn ba_on_add_sta_status(&mut self, status: u32) {
        let p = match self.ba_pending.take() {
            Some(p) => p,
            None => {
                // An ADD_STA we sent for something else — or the answer to a
                // request that already timed out above. Worth saying once:
                // both counters staying at zero looked like "never called".
                if self.st.addba_seen > 0 && self.st.addba_timeouts == 0 {
                    host::print("[ax200] ADD_STA status arrived with no block-ack pending\n");
                }
                return;
            }
        };
        let ok = status & ADD_STA_STATUS_MASK == ADD_STA_SUCCESS
            && status & IWL_ADD_STA_BAID_VALID_MASK != 0;
        if !ok {
            self.ba_reply(&p, WLAN_STATUS_REQUEST_DECLINED);
            self.st.addba_declined = self.st.addba_declined.wrapping_add(1);
            host::print(if status & ADD_STA_STATUS_MASK == ADD_STA_IMMEDIATE_BA_FAILURE {
                "[ax200] block-ack refused: firmware has no session slot left (0x"
            } else {
                "[ax200] block-ack refused by firmware (status 0x"
            });
            host::print_hex32(status);
            host::print(")\n");
            return;
        }
        let baid = ((status & IWL_ADD_STA_BAID_MASK) >> IWL_ADD_STA_BAID_SHIFT) as u8;
        if let Some(sess) = ba::by_tid(p.tid) {
            sess.start(baid, p.tid, p.ssn, p.dialog, p.timeout);
        }
        self.ba_reply(&p, WLAN_STATUS_SUCCESS);
        self.st.addba_accepted = self.st.addba_accepted.wrapping_add(1);
        if self.st.addba_accepted <= 2 {
            host::print("[ax200] block-ack RX session up: baid ");
            host::print_dec(baid as u32);
            host::print(" tid ");
            host::print_dec(p.tid as u32);
            host::print(" window ");
            host::print_dec(p.win as u32);
            host::print("\n");
        }
    }

    /// The ADDBA response frame.
    ///
    /// The parameter set is built from scratch, exactly as
    /// `ieee80211_send_addba_resp` does, and NOT echoed from the request. That
    /// distinction cost a release: echoing kept the AP's A-MSDU bit, which told
    /// it that it may pack several MSDUs into one MPDU — and `rx_classify`
    /// decodes exactly one, so every aggregated frame turned to garbage and the
    /// link carried nothing at all. mac80211 sets the bit from its OWN
    /// capability (SUPPORTS_AMSDU_IN_AMPDU); ours is no.
    fn ba_reply(&mut self, p: &BaPending, status: u16) {
        let mut fr = [0u8; DOT11_HDR_LEN + 9];
        fr[0] = (DOT11_STYPE_ACTION << 4) | 0x00; // management, subtype action
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let b = DOT11_HDR_LEN;
        fr[b] = WLAN_CATEGORY_BACK;
        fr[b + 1] = WLAN_ACTION_ADDBA_RESP;
        fr[b + 2] = p.dialog;
        put_u16(&mut fr, b + 3, status);
        let params = BA_PARAM_POLICY_IMMEDIATE // A-MSDU bit deliberately 0
            | (((p.tid as u16) << BA_PARAM_TID_SHIFT) & BA_PARAM_TID_MASK)
            | ((p.win << BA_PARAM_BUFSZ_SHIFT) & BA_PARAM_BUFSZ_MASK);
        put_u16(&mut fr, b + 5, params);
        put_u16(&mut fr, b + 7, p.timeout);
        self.tx_mgmt_frame(&fr);
    }

    // Transmit a payload as an 802.11 DATA frame on the data queue (toDS:
    // addr1=BSSID, addr2=us, addr3=dst) + LLC/SNAP. `encrypt`=false sets
    // ENCRYPT_DIS (EAPOL during the 4-way); =true lets the firmware encrypt with
    // the installed PTK (IP traffic after AUTHORIZED).
    // Returns false if the frame was dropped because the data queue is full
    // (the firmware hasn't drained it yet) — the caller leaves it to the IP
    // stack to retransmit rather than overwrite an in-flight TFD.
    /// `critical` = this frame has NOBODY behind it to retransmit, so it must
    /// not be refused for flow control. EAPOL is the case that matters: wifid
    /// hands the group-rekey reply down exactly once. Dropped, the AP retries,
    /// we drop again, and after a few rounds it simply stops talking to us —
    /// no deauth, no error, the association still "up". Measured on the
    /// device as four identical rekeys followed by a dead link.
    ///
    /// Bypassing the cap is safe: it exists against bufferbloat, and the ring
    /// holds 256 TFDs against a cap of 16.
    fn tx_8023(&mut self, dst: [u8; 6], ethertype: u16, payload: &[u8], encrypt: bool,
               critical: bool) -> bool {
        // Flow control + anti-bufferbloat, in bytes (TX_INFLIGHT_BYTES) so a
        // 67-byte ACK does not cost what a 1514-byte frame costs, with the ring
        // guard behind it so write_ptr never laps the firmware's read pointer.
        // Caller leaves the rest in the kernel mailbox for retransmit.
        if !critical && (self.data_bytes_in_flight >= TX_INFLIGHT_BYTES
            || self.data_in_flight >= TX_INFLIGHT_MAX)
        {
            return false;
        }
        // As a QoS (HT) station every data frame carries a QoS control field, so
        // the header grows from 24 to 26 bytes. tx_raw derives offload_assist and
        // the DWORD padding from the length we pass it.
        let hdr_len = if self.qos { DOT11_QOS_HDR_LEN } else { DOT11_HDR_LEN };
        let mut fr = [0u8; 1600];
        fr[0] = if self.qos { DOT11_FC_QOS_DATA } else { DOT11_FC_DATA };
        fr[1] = DOT11_FC1_TODS;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&dst);
        // Sequence control. `iwl_mvm_tx_mpdu` (mvm/tx.c:1174) writes it into the
        // header ONLY on the OLD TX API, and the guard is explicit:
        //
        //     if (ieee80211_is_data_qos(fc) && !ieee80211_is_qos_nullfunc(fc)) {
        //             seq_number = mvmsta->tid_data[tid].seq_number;
        //             if (!iwl_mvm_has_new_tx_api(mvm)) {
        //                     hdr->seq_ctrl &= cpu_to_le16(IEEE80211_SCTL_FRAG);
        //                     hdr->seq_ctrl |= cpu_to_le16(seq_number);
        //             }
        //     }
        //
        // We are a gen2 device, i.e. the NEW TX API, so on a QoS frame the
        // FIRMWARE owns the sequence number — and it owns it because the
        // block-ack window is built on it. Our comment here used to say
        // "writing our own costs nothing and covers us if it does not".
        // Measured: `tx agg aggregated 0 max 1 ba-notif 0` over 21583
        // transmissions. It costs every aggregate we never got.
        //
        // Non-QoS data still needs a number from us: there is no mac80211
        // underneath that would already have assigned one.
        if !self.qos {
            put_u16(&mut fr, DOT11_OFF_SEQ, (self.tx_seq & 0x0fff) << 4);
            self.tx_seq = self.tx_seq.wrapping_add(1);
        }
        // QoS control: TID 0 (best effort), normal ack, no A-MSDU. Bytes stay 0.
        let mut p = hdr_len;
        fr[p..p + 6].copy_from_slice(&LLC_SNAP_HDR);
        fr[p + 6] = (ethertype >> 8) as u8;
        fr[p + 7] = ethertype as u8;
        p += 8;
        fr[p..p + payload.len()].copy_from_slice(payload);
        p += payload.len();
        let flags = if encrypt {
            // IP data after AUTHORIZED: no CMD_RATE → the firmware rate-scales
            // (TLC); no ENCRYPT_DIS → it encrypts with the installed PTK.
            0
        } else {
            // EAPOL during the 4-way: robust fixed 1 Mbit CCK, unencrypted.
            IWL_TX_FLAGS_ENCRYPT_DIS | IWL_TX_FLAGS_CMD_RATE
        };
        let wptr_before = self.data_write_ptr;
        self.data_write_ptr = self.tx_raw(
            self.data_queue_id,
            self.data_write_ptr,
            IWL_DATA_QUEUE_SIZE,
            self.data_tfd,
            self.data_first_tb,
            self.data_payload,
            self.data_bc_tbl,
            flags,
            &fr[..p],
            hdr_len,
        );
        // Record what this slot now holds BEFORE the pointer moved on, so the
        // per-pass re-derivation can walk back over it.
        let slot = (wptr_before & (IWL_DATA_QUEUE_SIZE as u32 - 1)) as usize;
        self.slot_bytes[slot] = p as u16;
        self.data_bytes_in_flight = self.data_bytes_in_flight.saturating_add(p as u32);
        self.data_in_flight += 1;
        true
    }

    /// Re-derive the in-flight byte count from the slots `data_in_flight`
    /// covers. Walking back from the write pointer ties it to the same
    /// authority as the frame count, so a correction there corrects this too —
    /// and a swallowed completion cannot leak bytes forever any more than it
    /// can leak slots.
    fn rederive_bytes_in_flight(&mut self) {
        let mask = IWL_DATA_QUEUE_SIZE as u32 - 1;
        let n = self.data_in_flight.min(IWL_DATA_QUEUE_SIZE as u32);
        let mut total = 0u32;
        for k in 1..=n {
            let slot = (self.data_write_ptr.wrapping_sub(k) & mask) as usize;
            total += self.slot_bytes[slot] as u32;
        }
        self.data_bytes_in_flight = total;
    }

    // Convert an Ethernet frame from the IP stack ([dst 6][src 6][etype 2][pl])
    // into an encrypted 802.11 data frame and transmit it. Returns false if the
    // queue was full (frame dropped → the IP stack will retransmit).
    fn tx_eth(&mut self, eth: &[u8]) -> bool {
        if eth.len() < 14 {
            return true;
        }
        let mut dst = [0u8; 6];
        dst.copy_from_slice(&eth[0..6]);
        let ethertype = ((eth[12] as u16) << 8) | eth[13] as u16;
        self.tx_8023(dst, ethertype, &eth[14..], true, false)
    }

    // Allocate a gen2 data TX queue (tid 0) for the AP station, so EAPOL frames
    // have a data path. Same SCD_QUEUE_CONFIG mechanism as the mgmt queue.
    fn alloc_data_queue(&mut self) -> bool {
        self.data_tfd = self.alloc_dma(TFH_TFD_SIZE * IWL_DATA_QUEUE_SIZE, "data.tfd");
        self.data_first_tb =
            self.alloc_dma(IWL_FIRST_TB_SIZE_ALIGN * IWL_DATA_QUEUE_SIZE, "data.first_tb");
        self.data_payload = self.alloc_dma(TX_PAYLOAD_STRIDE * IWL_DATA_QUEUE_SIZE, "data.payload");
        self.data_bc_tbl = self.alloc_dma(BC_TBL_BYTES, "data.bc_tbl");
        if !self.data_tfd.ok() || !self.data_first_tb.ok() || !self.data_payload.ok() || !self.data_bc_tbl.ok() {
            return false;
        }
        let mut q = [0u8; SCD_CMD_LEN];
        put_u32(&mut q, SQ_OFF_OPERATION, IWL_SCD_QUEUE_ADD);
        put_u32(&mut q, SQ_OFF_STA_MASK, 1 << AP_STA_ID);
        q[SQ_OFF_TID] = IWL_DATA_TID;
        put_u32(&mut q, SQ_OFF_CB_SIZE, DATA_QUEUE_CB_SIZE);
        put_u64(&mut q, SQ_OFF_BC_DRAM_ADDR, self.data_bc_tbl.phys);
        put_u64(&mut q, SQ_OFF_TFDQ_DRAM_ADDR, self.data_tfd.phys);
        self.send_hcmd(DATA_PATH_GROUP, SCD_QUEUE_CONFIG_CMD, &q);
        match self.wait_rx(SCD_QUEUE_CONFIG_CMD, DATA_PATH_GROUP, 1000) {
            Some(rb) => {
                let mut p = [0u8; 16];
                host::dma_read_buf(rb.handle, 0, &mut p);
                let b = RX_PKT_DATA_OFF;
                self.data_queue_id =
                    u16::from_le_bytes([p[b + SQ_RSP_OFF_QUEUE_NUMBER], p[b + SQ_RSP_OFF_QUEUE_NUMBER + 1]]);
                self.data_write_ptr =
                    u16::from_le_bytes([p[b + SQ_RSP_OFF_WRITE_PTR], p[b + SQ_RSP_OFF_WRITE_PTR + 1]]) as u32;
                host::dprint("[ax200] data queue_id=");
                host::dprint_dec(self.data_queue_id as u32);
                host::dprint("\n");
                true
            }
            None => false,
        }
    }

    // ADD_STA_KEY (0x17, cmd_ver 3) — install a CCMP key the supplicant computed.
    // `group` = GTK (multicast) vs PTK (pairwise). rx_mic/tx_mic/tx_seq stay 0.
    /// Pick a firmware key-table slot, `iwl_mvm_set_fw_key_idx` (sta.c:3457).
    ///
    /// Linux deliberately takes the unused slot that was freed LONGEST AGO —
    /// that is what the per-slot `deleted` counters are for. We pinned every
    /// group key to slot 1, so a GTK rekey overwrote the previous group key
    /// the instant the new one arrived. The AP switches key ID 1<->2 across a
    /// rekey and keeps sending with the OLD id for a moment; those frames then
    /// have no key here and are dropped. Group key = BROADCAST, so ARP
    /// requests and DHCP replies vanish while unicast (the pairwise key, its
    /// own slot) carries on undisturbed — measured exactly so on the device.
    fn alloc_key_slot(&mut self, group: bool) -> u8 {
        if !group {
            return 0; // pairwise: one station, one slot, never rotated
        }
        let mut best = STA_KEY_IDX_INVALID;
        let mut best_age = -1i32;
        for i in 1..STA_KEY_MAX_NUM {
            if self.key_slot_used[i as usize] { continue; }
            let age = self.key_slot_freed[i as usize] as i32;
            if age > best_age {
                best_age = age;
                best = i;
            }
        }
        if best == STA_KEY_IDX_INVALID {
            // Every slot busy: free the oldest group key rather than refuse.
            best = 1;
        }
        // Only TWO group keys can be live at once (802.11 key ids 1 and 2), so
        // releasing the one before last keeps the table from filling while the
        // previous key stays valid for the whole transition.
        if let Some(old) = self.key_slot_prev.replace(best) {
            self.key_slot_used[old as usize] = false;
            for i in 0..STA_KEY_MAX_NUM as usize {
                self.key_slot_freed[i] = self.key_slot_freed[i].saturating_add(1);
            }
            self.key_slot_freed[old as usize] = 0;
        }
        self.key_slot_used[best as usize] = true;
        best
    }

    fn install_key(&mut self, group: bool, key_idx: u8, key: &[u8], rsc: &[u8]) {
        let slot = self.alloc_key_slot(group);
        let mut cmd = [0u8; ADD_STA_KEY_LEN];
        cmd[KEY_OFF_STA_ID] = AP_STA_ID;
        cmd[KEY_OFF_KEY_OFFSET] = slot;
        let mut flags = STA_KEY_FLG_CCM | ((key_idx as u16) << STA_KEY_FLG_KEYID_POS);
        if group {
            flags |= STA_KEY_MULTICAST;
        }
        put_u16(&mut cmd, KEY_OFF_KEY_FLAGS, flags);
        let kl = key.len().min(32);
        cmd[KEY_OFF_KEY..KEY_OFF_KEY + kl].copy_from_slice(&key[..kl]);
        let rl = rsc.len().min(16);
        cmd[KEY_OFF_RX_SEQ..KEY_OFF_RX_SEQ + rl].copy_from_slice(&rsc[..rl]);
        self.send_hcmd(0, ADD_STA_KEY_CMD, &cmd); // → LONG_GROUP(1)
        self.pump_rx(20);
        if group {
            self.st.gtk_installs = self.st.gtk_installs.wrapping_add(1);
        } else {
            self.ptk_installed = true;
        }
        host::print(if group { "[ax200] GTK installed: key id " } else { "[ax200] PTK installed: key id " });
        host::print_dec(key_idx as u32);
        host::print(" -> fw slot ");
        host::print_dec(slot as u32);
        host::print("\n");
    }

    // Extract an 802.11 management frame from an RX buffer if it is addressed to
    // us (addr1 == our MAC). Returns (subtype, first 12 body bytes after the
    // 24-byte header) — 12 covers the longest body we inspect, an ADDBA request.
    // Same RB layout as parse_beacon: frame @ RX_PKT_DATA_OFF + desc(48).
    fn rx_mgmt_for_us(rb: &Dma, our_mac: &[u8; 6]) -> Option<(u8, [u8; 12])> {
        let mut buf = [0u8; 96];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let f = RX_PKT_DATA_OFF + IWL_RX_DESC_SIZE_V1; // 56
        let fc = buf[f];
        if fc & 0x0c != 0 {
            return None; // not a management frame
        }
        if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] != our_mac[..] {
            return None; // not addressed to us
        }
        let subtype = (fc >> 4) & 0xf;
        let b = f + DOT11_HDR_LEN;
        let mut body = [0u8; 12];
        body.copy_from_slice(&buf[b..b + 12]);
        Some((subtype, body))
    }

    // Classify a received 802.11 DATA frame addressed to us: EAPOL (4-way) vs IP.
    // Finds the LLC/SNAP header at the 802.11 header end or 8 bytes further (an
    // intact CCMP header on a just-decrypted frame). EAPOL → the self-describing
    // EAPOL frame in `out`; IP → an Ethernet frame [dst=us][src=addr3][etype][pl].
    /// Report that the payload was not where the descriptor said it would be.
    /// Budgeted: if the computation is systematically wrong this fires on every
    /// frame, and a log that writes the normal case is not a log any more.
    fn note_llc_miss(budget: &mut u32, want: usize, found: usize) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        host::print("[ax200] RX payload offset mismatch: computed +");
        host::print_dec(want as u32);
        if found == usize::MAX {
            host::print(", found NOWHERE — frame dropped\n");
        } else {
            host::print(", found +");
            host::print_dec(found as u32);
            host::print("\n");
        }
    }

    /// `ampdu_tog` carries the aggregate we are inside of across calls:
    /// `0xFF` = not in one, otherwise the firmware's TOGGLE bit as 0 or 1.
    fn rx_classify(rb: &Dma, our_mac: &[u8; 6], out: &mut [u8], miss_log: &mut u32,
                   to_us: &mut u32, crypt: &mut [u32; 4], air_us: &mut u64,
                   ampdu_tog: &mut u8) -> RxKind {
        let mut buf = [0u8; 1600];
        host::dma_read_buf(rb.handle, 0, &mut buf);
        let d = RX_PKT_DATA_OFF;
        let mpdu_len = (((buf[d + MPDU_OFF_MPDU_LEN + 1] as usize) << 8)
            | buf[d + MPDU_OFF_MPDU_LEN] as usize)
            .min(buf.len() - d);
        let f = d + IWL_RX_DESC_SIZE_V1; // 56
        let fc = buf[f];
        // Count anything unicast to our address, whatever its type. This is the
        // one number that separates "the AP stopped talking to us" from "it is
        // talking and we discard it" — and without it both look like silence.
        if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] == our_mac[..] {
            *to_us += 1;
        }
        if fc & 0x0c != DOT11_FC_TYPE_DATA {
            return RxKind::None;
        }
        // Accept frames to us OR to a group address (multicast bit / broadcast) —
        // a DHCP offer / ARP reply often comes back L2-broadcast.
        let multicast = buf[f + DOT11_OFF_ADDR1] & 0x01 != 0;
        if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] != our_mac[..] && !multicast {
            return RxKind::None;
        }
        let subtype = (fc >> 4) & 0xf;
        // Data subtypes with bit 2 set carry NO BODY: Null (4) and QoS Null (12)
        // are the ones that occur — the AP's keepalive, and what Linux sends in
        // `ieee80211_mgd_probe_ap`. They have no LLC/SNAP because they have no
        // payload, and treating that as a decode failure made a perfectly normal
        // frame look like corruption: it counted into `undecoded` and burned the
        // budget of a log meant for real misses. Observed as a recurring
        // "RX payload offset mismatch: computed +88" — 56 + 24 + 8, i.e. exactly
        // a non-QoS data header with CCMP.
        if subtype & DOT11_STYPE_NODATA != 0 {
            return RxKind::None;
        }
        let hdrlen = if subtype & DOT11_STYPE_QOS != 0 { DOT11_QOS_HDR_LEN } else { DOT11_HDR_LEN };
        // Where the payload actually starts, exactly as iwl_mvm_create_skb
        // computes it: 802.11 header, then the IV the firmware left in place for
        // the cipher it decrypted with, then the DWORD padding the firmware
        // inserts when header+IV is not a multiple of 4 (the QoS+CCMP case).
        // Air this frame occupied. `mpdu_len` and the rate are both already in
        // the buffer we just read, so this costs an integer divide, no DMA.
        let rnf = le32(&buf, d + MPDU_OFF_RATE_N_FLAGS);
        let kbit = Self::rate_kbit(Self::rate_v3(rnf));
        if kbit > 0 {
            // The whole exchange, not just the bits on the wire. The first
            // version counted preamble + SIFS + ACK and called itself a lower
            // bound — but DIFS and the average backoff are the BIGGEST term at
            // HT rates (102 of 268 µs for a 1500-byte frame at 144 Mbit), so
            // that "bound" read 1.5x low and argued the channel was idle when
            // it was about half full. A number that misleads by 1.5x is worse
            // than no number.
            let cck = rnf & RATE_MCS_MOD_TYPE_MSK == RATE_MCS_MOD_TYPE_CCK;
            let preamble = match rnf & RATE_MCS_MOD_TYPE_MSK {
                RATE_MCS_MOD_TYPE_CCK => 96,       // short preamble
                RATE_MCS_MOD_TYPE_HT => 40,        // HT-mixed, 2 spatial streams
                _ => 20,                           // legacy OFDM
            };
            // SIFS + ACK + DIFS + mean backoff (CWmin 15 for best effort, so
            // 7.5 slots). Slot and SIFS differ between the DSSS and OFDM PHYs.
            let overhead = if cck { 10 + 40 + 50 + 150 } else { 16 + 28 + 34 + 68 };
            // In an A-MPDU the preamble, SIFS, block-ack, DIFS and backoff are
            // paid ONCE for the whole aggregate — not per subframe. Charging
            // them per subframe made the report claim `rx 186 %` of a window,
            // which is not a busy channel but a broken ruler. The firmware
            // flips PHY_AMPDU_TOGGLE at the start of every new aggregate
            // (iwl_mvm_rx_mpdu_mq), so a subframe whose toggle matches the one
            // before it costs data time only.
            let phy_info = (buf[d + MPDU_OFF_PHY_INFO] as u16)
                | ((buf[d + MPDU_OFF_PHY_INFO + 1] as u16) << 8);
            let first_of_aggregate = if phy_info & IWL_RX_MPDU_PHY_AMPDU != 0 {
                let t = if phy_info & IWL_RX_MPDU_PHY_AMPDU_TOGGLE != 0 { 1u8 } else { 0u8 };
                let is_new = *ampdu_tog != t;
                *ampdu_tog = t;
                is_new
            } else {
                *ampdu_tog = 0xFF; // outside an aggregate every frame pays in full
                true
            };
            *air_us += (mpdu_len as u64 * 8 * 1000 / kbit as u64)
                + if first_of_aggregate { preamble + overhead } else { 0 };
        }

        let status = le32(&buf, d + MPDU_OFF_STATUS);
        // Did the firmware actually decrypt this? We do not drop on the answer —
        // the point is to learn whether a link that looks alive is receiving
        // frames it cannot read. See iwl_mvm_rx_crypto (rxmq.c:414).
        if buf[f + 1] & DOT11_FC_PROTECTED != 0 {
            crypt[0] += 1;
            match status & RX_STATUS_SEC_MASK {
                RX_STATUS_SEC_CCM => {
                    if status & RX_STATUS_MIC_OK == 0 {
                        crypt[1] += 1;
                    }
                }
                RX_STATUS_SEC_NONE => crypt[2] += 1,
                _ => {}
            }
            if status & RX_STATUS_DECRYPTED == 0 {
                crypt[3] += 1;
            }
        }
        let crypt_len = if status & RX_STATUS_SEC_MASK == RX_STATUS_SEC_CCM {
            IEEE80211_CCMP_HDR_LEN
        } else {
            0
        };
        let pad = if buf[d + MPDU_OFF_MAC_FLAGS2] & MFLG2_PAD != 0 { 2usize } else { 0 };
        let want = f + hdrlen + crypt_len + pad;
        let at = |o: usize| o + 6 <= buf.len() && buf[o..o + 6] == LLC_SNAP_HDR;
        // If the computed position is not where LLC/SNAP actually sits, fall back
        // to searching the two places it can be and say so — a silent mismatch
        // here would drop every frame and look like a dead link.
        let llc = if at(want) {
            want
        } else if at(f + hdrlen) {
            Self::note_llc_miss(miss_log, want, f + hdrlen);
            f + hdrlen
        } else if at(f + hdrlen + IEEE80211_CCMP_HDR_LEN) {
            Self::note_llc_miss(miss_log, want, f + hdrlen + IEEE80211_CCMP_HDR_LEN);
            f + hdrlen + IEEE80211_CCMP_HDR_LEN
        } else {
            // Addressed to us, a data frame WITH a body, and LLC/SNAP is at none
            // of the possible offsets. Silently dropping this was a blind spot.
            // `usize::MAX` = nowhere; `0` used to be the sentinel and read like
            // a real offset ("found +0"), which is its own small lie.
            Self::note_llc_miss(miss_log, want, usize::MAX);
            return RxKind::Undecoded;
        };
        if llc + 8 > buf.len() {
            return RxKind::None;
        }
        let ethertype = ((buf[llc + 6] as u16) << 8) | buf[llc + 7] as u16;
        let pl = llc + 8; // payload after LLC/SNAP
        if ethertype == ETHERTYPE_EAPOL {
            if pl + 4 > buf.len() {
                return RxKind::None;
            }
            let elen = 4 + (((buf[pl + 2] as usize) << 8) | buf[pl + 3] as usize);
            if pl + elen > buf.len() || elen > out.len() {
                return RxKind::None;
            }
            out[..elen].copy_from_slice(&buf[pl..pl + elen]);
            RxKind::Eapol(elen)
        } else {
            // Only IPv4 (0x0800) + ARP (0x0806) belong in the kernel IP stack.
            // Other ethertypes the AP floods (0x88e1 HomePlug, multicast, …) are
            // not ours to handle — drop them early instead of feeding the stack.
            if ethertype != 0x0800 && ethertype != 0x0806 {
                return RxKind::None;
            }
            // Ethernet frame for the IP stack: dst = us (addr1), src = addr3 (SA).
            // mpdu_len spans the whole frame including the firmware's padding and
            // whatever MIC/CRC the RADA left on the tail — strip both, exactly as
            // iwl_mvm_create_skb does, or the stack sees trailing garbage.
            // The payload always ends mic_crc_len before the end of the MPDU:
            // mpdu_len covers header + IV + padding + payload + MIC, and the
            // padding sits before the payload, so it cancels out. That makes the
            // end independent of how the IV/padding split was determined above.
            let mic_crc_len =
                ((buf[d + MPDU_OFF_MAC_FLAGS1] & MFLG1_MIC_CRC_LEN_MASK) >> 4) as usize * 2;
            let end = (f + mpdu_len.saturating_sub(mic_crc_len)).min(buf.len());
            if end <= pl || 14 + (end - pl) > out.len() {
                return RxKind::None;
            }
            let plen = end - pl;
            out[0..6].copy_from_slice(&buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6]);
            out[6..12].copy_from_slice(&buf[f + DOT11_OFF_ADDR3..f + DOT11_OFF_ADDR3 + 6]);
            out[12] = (ethertype >> 8) as u8;
            out[13] = ethertype as u8;
            out[14..14 + plen].copy_from_slice(&buf[pl..end]);
            let amsdu = buf[d + MPDU_OFF_MAC_FLAGS2] & IWL_RX_MPDU_MFLG2_AMSDU != 0;
            let last = buf[d + MPDU_OFF_AMSDU_INFO] & IWL_RX_MPDU_AMSDU_LAST_SUBFRAME != 0;
            RxKind::Ip(14 + plen, Agg {
                reorder: le32(&buf, d + MPDU_OFF_REORDER_DATA),
                status,
                amsdu_last: !amsdu || last,
                reorderable: subtype & DOT11_STYPE_QOS != 0 && !multicast,
            })
        }
    }

    // Drain the RX ring up to `ms` ms looking for a management frame of the given
    // subtype addressed to us; return its first 8 body bytes. Recycles RBs.
    fn wait_mgmt_response(&mut self, want_subtype: u8, ms: u32) -> Option<[u8; 12]> {
        let our_mac = self.mac;
        let mut found: Option<[u8; 12]> = None;
        for _ in 0..ms {
            self.service_rx(|cmd, grp, rb| {
                if cmd == REPLY_RX_MPDU_CMD && grp == 0 {
                    if let Some((st, body)) = Self::rx_mgmt_for_us(rb, &our_mac) {
                        if st == want_subtype {
                            found = Some(body);
                            return false;
                        }
                    }
                }
                true
            });
            if found.is_some() {
                break;
            }
            host::sleep_ms(1);
        }
        found
    }

    // ── Stage 5c: open-system AUTH (connect step 3) ──────────────────────
    // Session protection (the prepare_tx hook) then an open-system auth request to
    // the target AP, then wait for the AP's auth response (subtype auth, seq 2).
    fn connect_send_auth(&mut self) -> bool {
        // SESSION_PROTECTION_CMD (cmd_ver 1, wait_for_notif=false) — reserves
        // channel time so the firmware actually transmits the unassociated frame.
        let mut sp = [0u8; SP_CMD_LEN];
        put_u32(&mut sp, SP_OFF_ID_COLOR, 0);
        put_u32(&mut sp, SP_OFF_ACTION, FW_CTXT_ACTION_ADD);
        put_u32(&mut sp, SP_OFF_CONF_ID, SESSION_PROTECT_CONF_ASSOC);
        put_u32(&mut sp, SP_OFF_DURATION_TU, SP_DURATION_TU);
        self.send_hcmd(MAC_CONF_GROUP, SESSION_PROTECTION_CMD, &sp);
        host::dprint("[ax200] SESSION_PROTECTION_CMD sent (assoc, 878 TU)\n");
        self.pump_rx(50);

        // 802.11 open-system auth request: DA/BSSID = AP, SA = us, seq 1.
        let mut fr = [0u8; DOT11_HDR_LEN + DOT11_AUTH_BODY_LEN];
        fr[0] = DOT11_FC_AUTH;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let b = DOT11_HDR_LEN;
        put_u16(&mut fr, b, DOT11_AUTH_ALG_OPEN);
        put_u16(&mut fr, b + 2, DOT11_AUTH_SEQ_1);
        put_u16(&mut fr, b + 4, 0);
        self.tx_mgmt_frame(&fr);
        host::dprint("[ax200] AUTH request TX'd (open-system), waiting for response...\n");

        // Auth response body: algorithm(2), seq(2), status(2).
        match self.wait_mgmt_response(DOT11_STYPE_AUTH, 2000) {
            Some(body) => {
                let seq = u16::from_le_bytes([body[2], body[3]]);
                let status = u16::from_le_bytes([body[4], body[5]]);
                if status == DOT11_STATUS_SUCCESS && seq == DOT11_AUTH_SEQ_2 {
                    host::dprint("[ax200] Stage 5c OK — AUTH accepted (status 0, seq 2)\n");
                    true
                } else {
                    host::print("[ax200] AUTH rejected: status=");
                    host::print_dec(status as u32);
                    host::print(" seq=");
                    host::print_dec(seq as u32);
                    host::print("\n");
                    false
                }
            }
            None => {
                host::print("[ax200] no AUTH response\n");
                self.dump_fw_error_log();
                false
            }
        }
    }

    // ── Stage 5d: association request → response (connect step 4) ─────────
    // Build an association request (mac80211 ieee80211_send_assoc, legacy IE set)
    // for the target AP, transmit it, and wait for the association response
    // (subtype 1) to read the status code + AID. For an encrypted AP we include a
    // WPA2-PSK-CCMP RSN element so the AP accepts the association (the 4-way
    // handshake / key install that follows lives in wifid — Phase H).
    fn connect_send_assoc(&mut self) -> bool {
        let mut fr = [0u8; 256];
        fr[0] = DOT11_FC_ASSOC_REQ;
        fr[DOT11_OFF_ADDR1..DOT11_OFF_ADDR1 + 6].copy_from_slice(&self.target_bssid);
        fr[DOT11_OFF_ADDR2..DOT11_OFF_ADDR2 + 6].copy_from_slice(&self.mac);
        fr[DOT11_OFF_ADDR3..DOT11_OFF_ADDR3 + 6].copy_from_slice(&self.target_bssid);
        let mut p = DOT11_HDR_LEN;

        // Fixed fields: capability info + listen interval.
        let mut cap = WLAN_CAP_ESS | WLAN_CAP_SHORT_PREAMBLE;
        if self.target_band == PHY_BAND_24 as u8 {
            cap |= WLAN_CAP_SHORT_SLOT;
        }
        if self.target_privacy {
            cap |= WLAN_CAP_PRIVACY;
        }
        put_u16(&mut fr, p, cap);
        put_u16(&mut fr, p + 2, DOT11_LISTEN_INTERVAL);
        p += 4;

        // SSID element.
        let sl = self.target_ssid_len as usize;
        fr[p] = WLAN_EID_SSID;
        fr[p + 1] = sl as u8;
        fr[p + 2..p + 2 + sl].copy_from_slice(&self.target_ssid[..sl]);
        p += 2 + sl;

        // Supported + extended supported rates (rate byte = Mbps*2, basic bit 0x80).
        let (supp, ext): (&[u8], &[u8]) = if self.target_band == PHY_BAND_24 as u8 {
            (&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24], &[0x30, 0x48, 0x60, 0x6c])
        } else {
            (&[0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c], &[])
        };
        fr[p] = WLAN_EID_SUPP_RATES;
        fr[p + 1] = supp.len() as u8;
        fr[p + 2..p + 2 + supp.len()].copy_from_slice(supp);
        p += 2 + supp.len();
        if !ext.is_empty() {
            fr[p] = WLAN_EID_EXT_SUPP_RATES;
            fr[p + 1] = ext.len() as u8;
            fr[p + 2..p + 2 + ext.len()].copy_from_slice(ext);
            p += 2 + ext.len();
        }

        // HT Capability element (802.11n). Only when the AP advertised HT — an
        // AP without it would get an element it never asked for, and everything
        // downstream (station flags, TLC mode HT) derives from its parameters.
        // The claimed width MUST match the PHY context: advertising 20/40 with
        // a 20 MHz radio invites frames it cannot receive, and claiming 20 with
        // a 40 MHz context wastes the half we configured. Both follow
        // `use_ht40`, which is the single place that decides.
        if self.target_ht.present {
            fr[p] = WLAN_EID_HT_CAPABILITY;
            fr[p + 1] = HT_CAP_IE_LEN as u8;
            let b = p + 2;
            // SM Power Save disabled (both chains stay live), short GI at 20 MHz
            // and RX-STBC one stream — each only if the AP supports it too.
            let mut cap = IEEE80211_HT_CAP_SM_PS_DISABLED;
            if self.target_ht.cap_info & IEEE80211_HT_CAP_LDPC_CODING != 0 {
                cap |= IEEE80211_HT_CAP_LDPC_CODING;
            }
            if self.target_ht.cap_info & IEEE80211_HT_CAP_SGI_20 != 0 {
                cap |= IEEE80211_HT_CAP_SGI_20;
            }
            if self.use_ht40() {
                cap |= IEEE80211_HT_CAP_SUP_WIDTH_20_40;
                if self.target_ht.cap_info & IEEE80211_HT_CAP_SGI_40 != 0 {
                    cap |= IEEE80211_HT_CAP_SGI_40;
                }
            }
            if self.target_ht.cap_info & IEEE80211_HT_CAP_RX_STBC != 0 {
                cap |= IEEE80211_HT_CAP_RX_STBC_1;
            }
            put_u16(&mut fr, b + HT_OFF_CAP_INFO, cap);
            fr[b + HT_OFF_AMPDU_PARAMS] = HT_AMPDU_FACTOR_64K
                | (HT_AMPDU_DENSITY_4US << IEEE80211_HT_AMPDU_PARM_DENSITY_SHIFT);
            // Supported receive MCS set: both spatial streams, MCS 0-15 (2x2).
            fr[b + HT_OFF_MCS_RX_MASK] = 0xff;
            fr[b + HT_OFF_MCS_RX_MASK + 1] = 0xff;
            // tx_params: TX MCS set defined and equal to the RX set (no TX_RX_DIFF).
            fr[b + HT_OFF_MCS_TX_PARAMS] = IEEE80211_HT_MCS_TX_DEFINED;
            p += 2 + HT_CAP_IE_LEN;
        }

        // VHT capabilities — only when we actually run 80 MHz. Same rule as
        // HT: what we claim has to match the PHY context, or the AP sends at a
        // width the radio is not listening on.
        if self.use_vht80() {
            fr[p] = WLAN_EID_VHT_CAPABILITY;
            fr[p + 1] = VHT_CAP_IE_LEN as u8;
            let b = p + 2;
            let mut cap = 0u32; // MAX_MPDU_LENGTH_3895 is 0
            if self.target_ht.vht_cap_info & IEEE80211_VHT_CAP_RXLDPC != 0 {
                cap |= IEEE80211_VHT_CAP_RXLDPC;
            }
            if self.target_ht.vht_cap_info & IEEE80211_VHT_CAP_SHORT_GI_80 != 0 {
                cap |= IEEE80211_VHT_CAP_SHORT_GI_80;
            }
            put_u32(&mut fr, b + VHT_OFF_CAP_INFO, cap);
            // Two spatial streams at MCS 0-9, the rest marked unused.
            put_u16(&mut fr, b + VHT_OFF_RX_MCS_MAP, VHT_MCS_MAP_2SS);
            put_u16(&mut fr, b + VHT_OFF_TX_MCS_MAP, VHT_MCS_MAP_2SS);
            // rx_highest / tx_highest stay 0: "no specified maximum".
            p += 2 + VHT_CAP_IE_LEN;
        }

        // RSN element (WPA2-PSK-CCMP) for encrypted APs.
        if self.target_privacy {
            let rsn: [u8; 20] = [
                0x01, 0x00, // version 1
                0x00, 0x0f, 0xac, 0x04, // group cipher: CCMP
                0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, // pairwise: 1 × CCMP
                0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, // AKM: 1 × PSK
                0x00, 0x00, // RSN capabilities
            ];
            fr[p] = WLAN_EID_RSN;
            fr[p + 1] = rsn.len() as u8;
            fr[p + 2..p + 2 + rsn.len()].copy_from_slice(&rsn);
            p += 2 + rsn.len();
        }

        // WMM information element — vendor-specific, so it goes last. An HT
        // station is a QoS station; without this the AP has no reason to grant
        // us EDCA parameters and may decline to use HT rates at all.
        if self.target_ht.present {
            fr[p..p + WMM_INFO_IE.len()].copy_from_slice(&WMM_INFO_IE);
            p += WMM_INFO_IE.len();
        }

        self.tx_mgmt_frame(&fr[..p]);
        host::dprint("[ax200] ASSOC request TX'd, waiting for response...\n");

        // Assoc response body: capability(2), status_code(2), aid(2).
        match self.wait_mgmt_response(DOT11_STYPE_ASSOC_RESP, 2000) {
            Some(body) => {
                let status =
                    u16::from_le_bytes([body[ASSOC_RESP_OFF_STATUS], body[ASSOC_RESP_OFF_STATUS + 1]]);
                let aid =
                    u16::from_le_bytes([body[ASSOC_RESP_OFF_AID], body[ASSOC_RESP_OFF_AID + 1]]) & 0x3fff;
                if status == DOT11_STATUS_SUCCESS {
                    self.assoc_aid = aid;
                    // We asked for HT + WMM and the AP accepted → from here on we
                    // are a QoS station and send QoS data frames.
                    self.qos = self.target_ht.present;
                    host::print("[ax200] *** ASSOCIATED *** aid=");
                    host::print_dec(aid as u32);
                    host::print(if self.qos { " (HT/QoS)\n" } else { " (legacy)\n" });
                    true
                } else {
                    host::print("[ax200] ASSOC rejected: status=");
                    host::print_dec(status as u32);
                    host::print("\n");
                    false
                }
            }
            None => {
                host::print("[ax200] no ASSOC response\n");
                false
            }
        }
    }

    // ── Resident NIC service loop ─────────────────────────────────
    // The chip is up and the scan has run; register as a network interface and
    // own the card from here. Same shape as aml.wasm: an infinite loop that
    // does the driver's work and yields via npk_sleep — never returns (the
    // driver holds its DMA + the netdev registration for its lifetime). Frame
    // bridging to the kernel netdev mailboxes (TX poll / RX submit) plugs into
    // this loop once association brings up the data path.
    fn run_netdev(&mut self, associated: bool) -> ! {
        let mac = self.mac;
        if host::netdev_register(&mac) == 0 {
            host::dprint("[ax200] registered as network interface 'wlan' (");
            Self::print_mac(&mac);
            host::dprint(")\n");
        } else {
            host::dprint("[ax200] netdev_register failed\n");
        }
        // The data TX queue was allocated before auth (so its SCD-response wait
        // wouldn't swallow the AP's first EAPOL frame). Tell wifid the connection
        // is ready + the MACs it needs for the PTK, then listen immediately — but
        // ONLY if we actually associated. Otherwise the link stays down (wlan is
        // registered but not primary) and we don't arm wifid for a dead BSS.
        let our_mac = self.mac;
        if associated {
            let mut ready = [0u8; 13];
            ready[0] = EV_READY;
            ready[1..7].copy_from_slice(&self.target_bssid);
            ready[7..13].copy_from_slice(&our_mac);
            host::wifi_send_event(&ready);
            self.st.ready_sent = self.st.ready_sent.wrapping_add(1);
            host::dprint("[ax200] associated — READY sent, listening for EAPOL (4-way)\n");
            // Tell the firmware we are associated (MAC context + station), then
            // start rate scaling. Linux does all three at the assoc state change;
            // data only flows after AUTHORIZED, so this is always in place before
            // the first IP frame.
            self.connect_post_assoc();
        } else {
            host::print("[ax200] NOT associated — wlan registered but link down (no 4-way)\n");
        }
        let mut rxbuf = [0u8; 1600];
        let mut evt = [0u8; 1700];
        let mut cmd = [0u8; 600];
        let mut txbuf = [0u8; 1514];
        let mut rx_log = 0u32; // throttle the data-path diagnostics
        let mut rate_log = 0u32; // …and the rate-change lines, which flap at VHT80
        let mut tx_log = 0u32;
        // Air-rate visibility. The firmware reports the TX rate it settled on
        // via TLC_MNG_UPDATE_NOTIF; the RX descriptor carries the rate the AP
        // used towards us. Log only when either CHANGES — a per-frame log would
        // drown the ring, and the interesting event is the transition.
        let mut last_tx_rate = u32::MAX;
        let mut last_rx_rate = u32::MAX;
        let mut rx_rate_tick = 0u32;
        let mut llc_miss = 8u32; // budget for RX-offset mismatch reports
        // Which A-MPDU we are inside of, so its fixed overhead is charged once.
        let mut ampdu_tog = 0xFFu8;
        let mut addba: Option<[u8; 12]> = None;
        let mut deauth_total = 0u32; // diagnostic: link-loss events seen
        // One-shot: is the firmware healthy after bring-up?
        if self.lmac_err_ptr != 0 && self.grab_nic_access() {
            let mut l = [0u32; LERR_WORDS];
            self.read_mem(self.lmac_err_ptr, &mut l);
            self.fw_assert = if l[LERR_VALID] != 0 { l[LERR_ERROR_ID].max(2) } else { 1 };
        }
        self.st.start_ms = host::now_ms();
        self.st.win_start_ms = self.st.start_ms;
        let mut next_report = self.st.start_ms + REPORT_PERIOD_MS;
        let mut reconnect_cooldown = 0u64;
        let mut assoc_at_ms = host::now_ms();
        let mut last_rx_ms = assoc_at_ms;
        let mut handshake_deadline = assoc_at_ms + HANDSHAKE_TIMEOUT_MS;
        loop {
            // Where the pass's time actually goes. The old `busy` counter only
            // said whether a pass FOUND work — it was read as CPU load (by
            // Claude, and it was wrong). This measures: work microseconds, drain
            // microseconds, and what a 1 ms sleep really costs.
            let t_pass = host::now_us();
            // RX: drain + recycle the ring. EAPOL-Key frames → wifid (the 4-way);
            // decrypted IP/other data → the kernel IP stack as Ethernet frames.
            // TX completions (TX_CMD response) free data-queue slots.
            let mut tx_done = 0u32;
            let mut link_lost = false;
            let mut deauth_reason = 0u16;
            let mut deauth_subtype = 0u8;
            // Per-pass accumulators: the RX closure cannot touch `self` (it is
            // borrowed by service_rx), so everything is folded in afterwards.
            let mut a_ok = 0u32;
            let mut a_fail = 0u32;
            let mut a_retries = 0u32;
            let mut a_rts = 0u32;
            let mut a_subframes = 0u64;
            let mut a_agg_resp = 0u32;
            let mut a_agg_max = 0u8;
            let mut a_ba_notifs = 0u32;
            let mut a_ba_reclaims = 0u32;
            let mut a_ba_over = 0u32;
            let mut a_ba_txed = 0u64;
            let mut a_ba_done = 0u64;
            let mut a_airtime = 0u64;
            let mut a_status = 0u16;
            let mut a_init_rate = 0u32;
            let mut a_ip = 0u32;
            let mut a_eapol = 0u32;
            let mut a_mgmt = 0u32;
            let mut a_crypt = [0u32; 4];
            let mut a_air = 0u64;
            let mut a_addba = 0u32;
            let mut a_rx_bytes = 0u64;
            let mut a_to_us = 0u32;
            let mut a_undecoded = 0u32;
            let mut add_sta_status: Option<u32> = None;
            let mut baid_alloc: Option<u32> = None;
            let mut delba: Option<(u8, u16)> = None;
            let mut mb: Option<(u32, u32, u32, u32)> = None;
            // The data queue id, captured before the closure borrows self.
            let a_dataq = self.data_queue_id as u32;
            let mut a_read_ptr: Option<u32> = None;
            let t_rx0 = host::now_us();
            let rx_frames = self.service_rx(|c, g, rb| {
                if c == TX_CMD && g == 0 {
                    // gen2 TX completion — one per transmitted data/mgmt frame.
                    // struct iwl_tx_resp carries what it COST on the air: the
                    // retry count, the rate the firmware started at and the
                    // microseconds of airtime consumed. Reading it is the only
                    // way to tell a slow link from a retrying one.
                    tx_done += 1;
                    let mut tr = [0u8; RX_PKT_DATA_OFF + TX_RESP_LEN];
                    host::dma_read_buf(rb.handle, 0, &mut tr);
                    // The response header carries the TFD index it completes
                    // (`SEQ_TO_INDEX`, cmdhdr.h:20) and the queue it belongs to
                    // (`SEQ_TO_QUEUE`). That is the firmware's read pointer,
                    // stated outright — no need to count.
                    let seq = u16::from_le_bytes([tr[6], tr[7]]) as u32;
                    if (seq >> 8) & 0x1f == a_dataq {
                        // Mask to the QUEUE WINDOW, not to 256. The write
                        // pointer wraps at MAX_TFD_QUEUE_SIZE while the data
                        // queue holds IWL_DATA_QUEUE_SIZE entries and indexes
                        // with `wptr & (qsize-1)`. Differencing across the two
                        // moduli produced in-flight counts like 198 against a
                        // cap of 16 — which blocked every transmission and
                        // read on the device as a dead link (8 Mbit, 41 %
                        // retries). Masked to the window the result is bounded
                        // 0..QUEUE_SIZE-1 by construction and can never wedge
                        // the queue. At 256 slots the two masks coincide, which
                        // is exactly why 256 is the ceiling: an 8-bit index
                        // cannot address a deeper queue.
                        // No `+1`: measured on the device, the reported
                        // index is already the NEXT slot to read, not the last
                        // one completed. Adding one made the derived value
                        // trail the counter by exactly one on every single
                        // pass — five samples in a row, all off by one, with
                        // only the first (9 vs 0) a real leak.
                        a_read_ptr = Some((seq & 0xff) & (IWL_DATA_QUEUE_SIZE as u32 - 1));
                    }
                    let base = RX_PKT_DATA_OFF;
                    // "frame_count: 1 no aggregation, >1 aggregation"
                    // (fw/api/tx.h). The field has been in the struct since the
                    // first port and unread ever since — the compiler said so.
                    let fc = tr[base + TXR_OFF_FRAME_COUNT];
                    a_subframes += fc.max(1) as u64;
                    if fc > 1 { a_agg_resp += 1; }
                    if fc > a_agg_max { a_agg_max = fc; }
                    a_rts += tr[base + TXR_OFF_FAILURE_RTS] as u32;
                    a_retries += tr[base + TXR_OFF_FAILURE_FRAME] as u32;
                    a_init_rate = le32(&tr, base + TXR_OFF_INITIAL_RATE);
                    a_airtime += u16::from_le_bytes([
                        tr[base + TXR_OFF_MEDIA_TIME],
                        tr[base + TXR_OFF_MEDIA_TIME + 1],
                    ]) as u64;
                    let st = u16::from_le_bytes([
                        tr[base + TXR_OFF_STATUS],
                        tr[base + TXR_OFF_STATUS + 1],
                    ]);
                    a_status = st;
                    match st & TX_STATUS_MSK {
                        TX_STATUS_SUCCESS | TX_STATUS_DIRECT_DONE => a_ok += 1,
                        _ => a_fail += 1,
                    }
                } else if c == BA_NOTIF && g == 0 {
                    // iwl_mvm_rx_ba_notif, new-tx-api path. On TLC-offload
                    // firmware this is where an aggregate reports itself:
                    // `txed` MPDUs went out, `done` were acknowledged.
                    let mut bn = [0u8; RX_PKT_DATA_OFF + CBA_HDR_LEN];
                    host::dma_read_buf(rb.handle, 0, &mut bn);
                    let b = RX_PKT_DATA_OFF;
                    a_ba_notifs += 1;
                    a_ba_txed += u16::from_le_bytes(
                        [bn[b + CBA_OFF_TXED], bn[b + CBA_OFF_TXED + 1]]) as u64;
                    a_ba_done += u16::from_le_bytes(
                        [bn[b + CBA_OFF_DONE], bn[b + CBA_OFF_DONE + 1]]) as u64;
                    // The RECLAIM half, and the reason 0.99.0 collapsed to
                    // 16 Mbit: an aggregated MPDU gets NO TX_CMD response. Its
                    // TFD slot is freed here or it is never freed at all. Linux
                    // does exactly this — `iwl_mvm_rx_ba_notif` walks the tfd
                    // array and hands each `tfd_index` to `iwl_mvm_tx_reclaim`
                    // as the queue's new read pointer (mvm/tx.c, new-tx-api
                    // path). We read `txed`/`done` from this notification since
                    // 0.93.0 and left the two fields next to them unread.
                    let tfd_cnt = u16::from_le_bytes(
                        [bn[b + CBA_OFF_TFD_CNT], bn[b + CBA_OFF_TFD_CNT + 1]]) as usize;
                    if tfd_cnt > CBA_TFD_MAX {
                        a_ba_over += 1;
                    }
                    for i in 0..tfd_cnt.min(CBA_TFD_MAX) {
                        let off = (RX_PKT_DATA_OFF + CBA_HDR_LEN + i * CBA_TFD_LEN) as u32;
                        let mut e = [0u8; CBA_TFD_LEN];
                        host::dma_read_buf(rb.handle, off, &mut e);
                        let q_num = u16::from_le_bytes(
                            [e[CBA_TFD_Q_NUM], e[CBA_TFD_Q_NUM + 1]]) as u32;
                        if q_num != a_dataq {
                            continue;
                        }
                        a_read_ptr = Some(u16::from_le_bytes(
                            [e[CBA_TFD_INDEX], e[CBA_TFD_INDEX + 1]]) as u32
                            & (IWL_DATA_QUEUE_SIZE as u32 - 1));
                        a_ba_reclaims += 1;
                    }
                } else if c == ADD_STA && g == IWL_ALWAYS_LONG_GROUP {
                    // Only the block-ack setup sends ADD_STA while the loop runs;
                    // its status word carries the session id (see ba_request).
                    let mut p = [0u8; RX_PKT_DATA_OFF + 4];
                    host::dma_read_buf(rb.handle, 0, &mut p);
                    add_sta_status = Some(le32(&p, RX_PKT_DATA_OFF));
                } else if c == RX_BAID_ALLOCATION_CONFIG_CMD && g == DATA_PATH_GROUP {
                    // iwl_rx_baid_cfg_resp — a bare __le32 baid.
                    let mut p = [0u8; RX_PKT_DATA_OFF + 4];
                    host::dma_read_buf(rb.handle, 0, &mut p);
                    baid_alloc = Some(le32(&p, RX_PKT_DATA_OFF));
                } else if c == MISSED_BEACONS_NOTIFICATION && g == 0 {
                    // iwl_mvm_handle_missed_beacons_notif (mvm/mac-ctxt.c:1615).
                    let mut p = [0u8; RX_PKT_DATA_OFF + 20];
                    host::dma_read_buf(rb.handle, 0, &mut p);
                    let b = RX_PKT_DATA_OFF;
                    mb = Some((le32(&p, b + MB_OFF_CONSEC),
                               le32(&p, b + MB_OFF_SINCE_LAST_RX),
                               le32(&p, b + MB_OFF_EXPECTED),
                               le32(&p, b + MB_OFF_RECEIVED)));
                } else if c == FRAME_RELEASE && g == 0 {
                    // The window moved without a frame for us: the firmware saw
                    // the MPDUs on air. Nothing here may be held back for them.
                    let mut p = [0u8; RX_PKT_DATA_OFF + 4];
                    host::dma_read_buf(rb.handle, 0, &mut p);
                    let baid = p[RX_PKT_DATA_OFF + FR_OFF_BAID];
                    let nssn = u16::from_le_bytes([
                        p[RX_PKT_DATA_OFF + FR_OFF_NSSN],
                        p[RX_PKT_DATA_OFF + FR_OFF_NSSN + 1],
                    ]);
                    if let Some(sess) = ba::by_baid(baid) {
                        sess.on_frame_release(nssn & 0x0fff);
                    }
                } else if c == TLC_MNG_UPDATE_NOTIF && g == DATA_PATH_GROUP {
                    // The firmware's rate-scaling verdict: what it is actually
                    // transmitting at. Without this the host is blind to the
                    // negotiated air rate.
                    let mut p = [0u8; 24];
                    host::dma_read_buf(rb.handle, 0, &mut p);
                    let rnf = le32(&p, RX_PKT_DATA_OFF + TLC_NOTIF_OFF_RATE);
                    if rnf != last_tx_rate {
                        last_tx_rate = rnf;
                        if rate_log < 8 {
                            rate_log += 1;
                            Self::log_rate("[ax200] TX rate → ", rnf);
                        }
                    }
                } else if c == REPLY_RX_MPDU_CMD && g == 0 {
                    // DIAGNOSTIC ONLY: note a DEAUTH / DISASSOC addressed to us +
                    // its reason, but do NOT tear down or reconnect — a reconnect
                    // would just mask whatever made us lose the link (our bug vs a
                    // genuinely-absent AP). Keep draining so detection never
                    // disrupts a healthy link.
                    if let Some((st, body)) = Self::rx_mgmt_for_us(rb, &our_mac) {
                        a_mgmt += 1;
                        a_to_us += 1; // rx_classify never sees these — count here
                        if st == DOT11_STYPE_DEAUTH || st == DOT11_STYPE_DISASSOC {
                            link_lost = true;
                            deauth_subtype = st;
                            deauth_reason = u16::from_le_bytes([body[0], body[1]]);
                        } else if st == DOT11_STYPE_ACTION && body[0] == WLAN_CATEGORY_BACK {
                            match body[1] {
                                WLAN_ACTION_ADDBA_REQ => {
                                    a_addba += 1;
                                    addba = Some(body); // answered outside
                                }
                                WLAN_ACTION_DELBA => {
                                    let params = u16::from_le_bytes([body[2], body[3]]);
                                    let t = ((params & IEEE80211_DELBA_PARAM_TID_MASK)
                                        >> IEEE80211_DELBA_PARAM_TID_SHIFT) as u8;
                                    let ini = (params & IEEE80211_DELBA_PARAM_INITIATOR_MASK)
                                        >> IEEE80211_DELBA_PARAM_INITIATOR_SHIFT;
                                    delba = Some((t, ini));
                                }
                                _ => {}
                            }
                        }
                        return true; // mgmt frame — not for the IP path
                    }
                    let uni_before = a_to_us;
                    match Self::rx_classify(rb, &our_mac, &mut rxbuf, &mut llc_miss, &mut a_to_us, &mut a_crypt, &mut a_air, &mut ampdu_tog) {
                        RxKind::Eapol(n) => {
                            a_eapol += 1;
                            a_rx_bytes += n as u64;
                            evt[0] = EV_EAPOL_RX;
                            evt[1] = (n & 0xff) as u8;
                            evt[2] = (n >> 8) as u8;
                            evt[3..3 + n].copy_from_slice(&rxbuf[..n]);
                            host::wifi_send_event(&evt[..3 + n]);
                        }
                        RxKind::Ip(n, agg) => {
                            a_ip += 1;
                            a_rx_bytes += n as u64;
                            // The AP's downlink rate, from the RX descriptor —
                            // sampled HERE, not for every received frame. Most of
                            // what the ring carries is beacons and other networks'
                            // broadcast, and a beacon always goes out at the
                            // lowest basic rate: sampling those reported a 6 Mbit
                            // downlink on a link actually running HT.
                            // …and only for UNICAST frames. Moving the sample out
                            // of the ring loop was not enough: most IP frames on a
                            // home network are broadcast (ARP, mDNS, SSDP), and
                            // broadcast goes out at the lowest basic rate just like
                            // a beacon. That is why this kept reading 6 Mbit on a
                            // link running HT.
                            rx_rate_tick += 1;
                            if a_to_us > uni_before && rx_rate_tick & 0x7 == 0 {
                                let mut rd = [0u8; 64];
                                host::dma_read_buf(rb.handle, 0, &mut rd);
                                let rnf = le32(&rd, RX_PKT_DATA_OFF + MPDU_OFF_RATE_N_FLAGS);
                                if rnf != last_rx_rate {
                                    last_rx_rate = rnf;
                                    // Budgeted. At VHT80 the rate flaps between
                                    // MCS 8 and 9 continuously, and every line
                                    // goes to the terminal, the global mirror
                                    // AND out over TCP — in the middle of the
                                    // measurement it is meant to inform. The
                                    // report carries the current rate anyway.
                                    if rate_log < 8 {
                                        rate_log += 1;
                                        Self::log_rate("[ax200] RX rate -> ", rnf);
                                    }
                                }
                            }
                            if rx_log < 12 {
                                host::dprint("[ax200] data RX → IP stack (len ");
                                host::dprint_dec(n as u32);
                                host::dprint(", etype 0x");
                                host::dprint_hex8(rxbuf[12]);
                                host::dprint_hex8(rxbuf[13]);
                                host::dprint(")\n");
                                rx_log += 1;
                            }
                            // Through the block-ack reorder buffer first: inside
                            // an A-MPDU a retransmitted MPDU arrives after the
                            // ones behind it, and handing that to TCP as-is costs
                            // more than the aggregation gains. It returns true
                            // when it took the frame (held for a hole, or dropped
                            // as a duplicate); with no session it is a no-op.
                            // Hand the frame to the kernel via the relay ring;
                            // Core 0's net::poll drains it + runs the TCP tick.
                            // (Direct in-fiber delivery via npk_netdev_rx_deliver
                            // exists but starved the Core-0 TCP tick under load →
                            // connection drops; revisit with #2 WiFi-IRQ.)
                            // The descriptor names the BAID, not the TID, so
                            // the session is looked up by it — with one session
                            // per TID there is more than one candidate now.
                            let taken = agg.reorderable && {
                                let b = ((agg.reorder & IWL_RX_MPDU_REORDER_BAID_MASK)
                                    >> IWL_RX_MPDU_REORDER_BAID_SHIFT) as u8;
                                match ba::by_baid(b) {
                                    Some(sess) => sess.on_frame(
                                        agg.reorder, agg.status, agg.amsdu_last, &rxbuf[..n]),
                                    None => false,
                                }
                            };
                            if !taken {
                                host::netdev_submit_rx(&rxbuf[..n]);
                            }
                        }
                        RxKind::Undecoded => { a_undecoded += 1; }
                        RxKind::None => {}
                    }
                }
                true
            });
            self.st.prof_rx_us += host::now_us().saturating_sub(t_rx0);
            self.st.prof_frames += rx_frames as u64;
            // Free the data-queue slots the firmware just reported done.
            //
            // Derived, not counted: `(write - read) & 255` is what the firmware
            // and we actually disagree about, and a swallowed completion is
            // repaired by the next one instead of leaking a slot forever.
            // Measured before this: 8 of 16 slots lost for 314 s, and an OTA
            // update that failed because a one-second stall killed its TLS
            // handshake.
            let counted = self.data_in_flight.saturating_sub(tx_done);
            if let Some(rp) = a_read_ptr {
                self.data_read_ptr = rp;
                let derived = (self.data_write_ptr & (IWL_DATA_QUEUE_SIZE as u32 - 1))
                    .wrapping_sub(rp) & (IWL_DATA_QUEUE_SIZE as u32 - 1);
                // Say when the two disagree by more than the frames completed
                // in this pass — that difference IS the leak, and until now it
                // was invisible.
                // Only a gap of two or more is news. One is noise, and a
                // log that writes the normal case is not a log.
                // …but not while an aggregate just reclaimed. With TLC-offload
                // aggregation on, `counted` drifts high every single pass
                // because most frames never produce a TX_CMD response at all.
                // That is the design working, not a leak, and a log that writes
                // the normal case is not a log.
                if derived + 1 < counted && a_ba_reclaims == 0
                    && self.st.inflight_corrections < 8
                {
                    self.st.inflight_corrections += 1;
                    host::print("[ax200] in-flight counted ");
                    host::print_dec(counted);
                    host::print(", firmware says ");
                    host::print_dec(derived);
                    host::print(" — correcting\n");
                }
                // Only ever LOWER the count. The derived value exists to
                // repair a leak; it must never be able to create one. Getting
                // the two pointers into different moduli once already wedged
                // transmission completely (in-flight 198 against a cap of 16,
                // 8 Mbit, dead link), and a diagnostic that can block the
                // queue is worse than the leak it fixes. Taking the minimum
                // means a disagreement — whatever its cause — costs nothing.
                self.data_in_flight = counted.min(derived);
            } else {
                self.data_in_flight = counted;
            }
            self.rederive_bytes_in_flight();
            // Fold this pass's accumulators into the running statistics.
            self.st.loop_iters = self.st.loop_iters.wrapping_add(1);
            self.st.rx_frames = self.st.rx_frames.wrapping_add(rx_frames);
            self.st.rx_bytes += a_rx_bytes;
            self.st.rx_ip = self.st.rx_ip.wrapping_add(a_ip);
            self.st.rx_eapol = self.st.rx_eapol.wrapping_add(a_eapol);
            self.st.rx_mgmt = self.st.rx_mgmt.wrapping_add(a_mgmt);
            self.st.addba_seen = self.st.addba_seen.wrapping_add(a_addba);
            self.st.rx_airtime_us += a_air;
            self.st.tx_subframes += a_subframes;
            self.st.tx_agg_resp = self.st.tx_agg_resp.wrapping_add(a_agg_resp);
            if a_agg_max > self.st.tx_agg_max { self.st.tx_agg_max = a_agg_max; }
            self.st.ba_notifs = self.st.ba_notifs.wrapping_add(a_ba_notifs);
            self.st.ba_reclaims = self.st.ba_reclaims.wrapping_add(a_ba_reclaims);
            self.st.ba_tfd_over = self.st.ba_tfd_over.wrapping_add(a_ba_over);
            self.st.ba_txed += a_ba_txed;
            self.st.ba_done += a_ba_done;
            self.st.rx_prot = self.st.rx_prot.wrapping_add(a_crypt[0]);
            self.st.rx_mic_fail = self.st.rx_mic_fail.wrapping_add(a_crypt[1]);
            self.st.rx_sec_none = self.st.rx_sec_none.wrapping_add(a_crypt[2]);
            self.st.rx_undecrypted = self.st.rx_undecrypted.wrapping_add(a_crypt[3]);
            self.st.rx_to_us = self.st.rx_to_us.wrapping_add(a_to_us);
            self.st.rx_undecoded = self.st.rx_undecoded.wrapping_add(a_undecoded);
            if rx_frames > self.st.rx_drain_max { self.st.rx_drain_max = rx_frames; }
            // `t_pass` is read at the top of every pass anyway, so the shape of
            // the arrivals costs a compare and an add.
            match rx_frames {
                0 => self.st.win_pass_empty = self.st.win_pass_empty.wrapping_add(1),
                1..=3 => self.st.win_pass_few = self.st.win_pass_few.wrapping_add(1),
                4..=15 => self.st.win_pass_many = self.st.win_pass_many.wrapping_add(1),
                _ => self.st.win_pass_burst = self.st.win_pass_burst.wrapping_add(1),
            }
            if rx_frames > 0 {
                if self.st.last_rx_us > 0 {
                    let gap = t_pass.saturating_sub(self.st.last_rx_us) as u32;
                    if gap > self.st.win_gap_max_us { self.st.win_gap_max_us = gap; }
                }
                self.st.last_rx_us = t_pass;
            }
            if rx_frames as usize >= RX_NUM_RBS - 2 {
                self.st.rx_pool_exhausted = self.st.rx_pool_exhausted.wrapping_add(1);
            }
            self.st.tx_ok = self.st.tx_ok.wrapping_add(a_ok);
            self.st.tx_fail = self.st.tx_fail.wrapping_add(a_fail);
            self.st.tx_retries = self.st.tx_retries.wrapping_add(a_retries);
            self.st.tx_rts_fail = self.st.tx_rts_fail.wrapping_add(a_rts);
            self.st.tx_airtime_us += a_airtime;
            if a_init_rate != 0 { self.st.last_init_rate = a_init_rate; }
            if a_status != 0 { self.st.last_status = a_status; }
            if last_tx_rate != u32::MAX { self.st.last_tx_rate = last_tx_rate; }
            if last_rx_rate != u32::MAX { self.st.last_rx_rate = last_rx_rate; }
            // Answer a block-ack setup request (the TX has to happen outside the
            // RX closure, which holds &mut self through service_rx).
            if let Some((consec, since_rx, expected, received)) = mb.take() {
                self.st.mb_consec = consec;
                self.st.mb_since_rx = since_rx;
                self.st.mb_expected = expected;
                self.st.mb_received = received;
                self.st.mb_notifs = self.st.mb_notifs.wrapping_add(1);
                // iwl_mvm_handle_missed_beacons_notif, verbatim in its thresholds:
                // a long run of missed beacons AND nothing received since is a
                // link that is gone. The same run WITH data still arriving is not
                // — Linux stays connected there and says it expects trouble.
                if consec >= IWL_MVM_MISSED_BEACONS_THRESHOLD_LONG {
                    if since_rx >= IWL_MVM_MISSED_BEACONS_SINCE_RX_THOLD {
                        host::print("[ax200] missed beacons ");
                        host::print_dec(consec);
                        host::print(" (");
                        host::print_dec(since_rx);
                        host::print(" since last RX) - treating the AP as gone\n");
                        link_lost = true;
                        self.st.mb_losses = self.st.mb_losses.wrapping_add(1);
                    } else if self.st.mb_notifs < 4 {
                        host::print("[ax200] missed beacons past threshold but data still \
                                     arriving — staying connected\n");
                    }
                } else if since_rx > IWL_MVM_MISSED_BEACONS_THRESHOLD && self.st.mb_notifs < 8 {
                    host::print("[ax200] beacon loss warning (missed since last RX: ");
                    host::print_dec(since_rx);
                    host::print(")\n");
                }
            }
            if let Some(status) = add_sta_status.take() {
                self.ba_on_add_sta_status(status);
            }
            if let Some(baid) = baid_alloc.take() {
                self.ba_on_baid_alloc(baid);
            }
            if let Some(req) = addba.take() {
                self.ba_request(&req);
            }
            // A firmware that never answers must not cost us the link. Linux
            // treats a refusal as "decline and carry on"; silence has to mean
            // the same, or the AP waits for a reply forever and stops sending.
            // Declining is a working link, just a slow one.
            if let Some(p) = self.ba_pending.as_ref() {
                if p.asked_ms > 0 && host::now_ms().saturating_sub(p.asked_ms) > BA_SETUP_MS {
                    let p = self.ba_pending.take().unwrap();
                    self.ba_reply(&p, WLAN_STATUS_REQUEST_DECLINED);
                    self.st.addba_declined = self.st.addba_declined.wrapping_add(1);
                    self.st.addba_timeouts = self.st.addba_timeouts.wrapping_add(1);
                    // Once is enough. Every further attempt is another 300 ms of
                    // silence towards the AP and another command the firmware
                    // does not finish.
                    self.ba_fw_broken = true;
                    if self.st.addba_timeouts <= 3 {
                        host::print("[ax200] firmware did not answer the block-ack setup \
                                     in time — declining so the AP keeps sending\n");
                    }
                }
            }
            // `ieee80211_process_delba`: the frame names the TID and who is
            // ending it. Only an INITIATOR-side DELBA concerns our RX session,
            // and we send none back — the AP already knows. A single global
            // flag used to tear down whichever session happened to be in the
            // one slot, regardless of the TID the AP named.
            if let Some((tid, initiator)) = delba.take() {
                if initiator == WLAN_BACK_INITIATOR {
                    self.ba_stop(tid, false, 0);
                }
            }
            // A hole the AP never fills must not park the window forever.
            ba::tick_all();
            // sta_rx_agg_session_timer_expired: a session the AP has gone quiet
            // on for longer than it asked for is stale. Linux tears it down and
            // sends a DELBA with WLAN_REASON_QSTA_TIMEOUT. We kept such a
            // session forever, holding a firmware BAID nothing would ever use.
            let now_ms = host::now_ms();
            for tid in 0..ba::NUM_TIDS as u8 {
                if ba::by_tid(tid).map(|s| s.timed_out(now_ms)).unwrap_or(false) {
                    host::print("[ax200] block-ack session on tid ");
                    host::print_dec(tid as u32);
                    host::print(" timed out — removing\n");
                    self.ba_stop(tid, true, WLAN_REASON_QSTA_TIMEOUT);
                }
            }
            // DIAGNOSTIC: a DEAUTH (subtype 12) / DISASSOC (10) arrived. Log it
            // with the 802.11 reason code — do NOT reconnect (that would mask the
            // root cause). The reason tells us whether the AP genuinely dropped us
            // or our own behaviour provoked it:
            //   1=unspecified  2=prev-auth-invalid  4=inactivity  6/7=class2/3
            //   frame from nonassoc STA (= our state/TX bug)  15=4-way timeout.
            // A DEAUTH (subtype 12) / DISASSOC (10) arrived. The reason code says
            // whether the AP genuinely dropped us or our own behaviour provoked
            // it: 1=unspecified 2=prev-auth-invalid 4=inactivity 6/7=class2/3
            // frame from a nonassoc STA (= our state/TX bug) 15=4-way timeout.
            // It is always logged and counted — then we reconnect, because in a
            // mesh with one SSID on two APs a steering kick is NORMAL traffic
            // and staying down until a human re-runs the driver is not an option.
            // Count what the acknowledgements say, but do NOT act on it yet.
            //
            // The first version of this acted immediately and made things worse:
            // "no transmit response for 5 s while frames are in flight" fires on
            // an IDLE link the moment `data_in_flight` is stuck above zero — a
            // single lost response and the driver tears down a healthy link,
            // over and over. And eight unacknowledged frames in a row is a bad
            // moment on a radio, not necessarily a dead AP.
            //
            // So: measure first. The streak is in the report; once we have seen
            // what it does on a link that really dies, it can drive a reconnect.
            let now_ms = host::now_ms();
            if a_ok > 0 {
                self.tx_fail_streak = 0;
                self.last_tx_resp_ms = now_ms;
            } else if a_fail > 0 {
                self.tx_fail_streak = self.tx_fail_streak.saturating_add(a_fail);
                self.last_tx_resp_ms = now_ms;
                if self.tx_fail_streak > self.tx_fail_streak_peak {
                    self.tx_fail_streak_peak = self.tx_fail_streak;
                }
            }
            if link_lost {
                deauth_total += 1;
                self.st.deauth = deauth_total;
                // The cause is already printed for a beacon loss; a frame-borne
                // loss names its subtype and reason code, which is the part that
                // says whether the AP dropped us or our own behaviour did.
                host::print("[ax200] ** LINK-LOSS ** ");
                if deauth_subtype == 0 {
                    host::print("no beacons");
                } else {
                    host::print(if deauth_subtype == DOT11_STYPE_DEAUTH { "DEAUTH" } else { "DISASSOC" });
                    host::print(" reason=");
                    host::print_dec(deauth_reason as u32);
                }
                host::print(" count=");
                host::print_dec(deauth_total);
                self.authorized = false;
                let t = host::now_ms();
                if t < reconnect_cooldown {
                    // A failed reconnect just ran. Re-scanning on every kick of a
                    // deauth storm would spend the whole time scanning and never
                    // be listening when the AP is ready for us.
                    host::print(" - in cooldown, not re-scanning yet\n");
                } else {
                    host::print(" - reconnecting\n");
                    if self.reconnect() {
                        reconnect_cooldown = 0;
                    } else {
                        host::print("[ax200] reconnect failed - retrying in 2 s\n");
                        reconnect_cooldown = host::now_ms() + 2000;
                    }
                    // The scan inside reconnect drained the ring; restart the
                    // window so the next report measures fresh traffic.
                    next_report = host::now_ms() + REPORT_PERIOD_MS;
                }
            }
            // TX: send every Ethernet frame the IP stack queued (DHCP, ARP, …),
            // but stop once the data queue is full — leave the rest in the kernel
            // mailbox so we never pop a frame we'd have to drop (and never lap the
            // firmware's read pointer).
            let mut tx_any = false;
            loop {
                // Two different walls, counted apart: the byte cap is policy and
                // can be raised on its own; the ring guard is the queue depth and
                // needs IWL_DATA_QUEUE_SIZE to move with it. Which one bites is
                // the whole question for the next size change, and one shared
                // counter could not answer it.
                if self.data_bytes_in_flight >= TX_INFLIGHT_BYTES {
                    // Not a drop: the frame stays in the kernel queue. But it IS
                    // the moment the cap becomes the throughput limit, so it has
                    // to be visible before anyone raises it.
                    self.st.tx_blocked = self.st.tx_blocked.wrapping_add(1);
                    break;
                }
                if self.data_in_flight >= TX_INFLIGHT_MAX {
                    self.st.tx_blocked_ring = self.st.tx_blocked_ring.wrapping_add(1);
                    break;
                }
                // Second line of defence: do not pull traffic before the
                // station is authorized. The firmware cannot transmit for an
                // unauthorized station, so those frames occupy TFD slots that
                // are never completed — data_in_flight never returns to zero and
                // the queue is wedged before the link is even up.
                if !self.authorized {
                    break;
                }
                let n = host::netdev_poll_tx(&mut txbuf);
                if n == 0 {
                    break;
                }
                tx_any = true;
                if tx_log < 12 {
                    host::dprint("[ax200] data TX ← IP stack (len ");
                    host::dprint_dec(n as u32);
                    host::dprint(", etype 0x");
                    host::dprint_hex8(txbuf[12]);
                    host::dprint_hex8(txbuf[13]);
                    host::dprint(")\n");
                    tx_log += 1;
                }
                if self.tx_eth(&txbuf[..n]) {
                    self.st.tx_frames = self.st.tx_frames.wrapping_add(1);
                    self.st.tx_bytes += n as u64;
                    if self.data_in_flight > self.st.inflight_peak {
                        self.st.inflight_peak = self.data_in_flight;
                    }
                }
            }
            // Control commands from wifid (TX_EAPOL / SET_KEY / AUTHORIZED).
            let clen = host::wifi_poll_cmd(&mut cmd);
            if clen > 0 {
                self.handle_wifi_cmd(&cmd[..clen as usize]);
            }
            // Queue watchdog, `iwl_txq_stuck_timer` / tx.c:1055 verbatim in its
            // condition:
            //
            //     if (txq->read_ptr == txq->write_ptr) delete timer;
            //     else                                 mod_timer(+wd_timeout);
            //
            // NOT EMPTY arms it, every completion pushes it forward. Fullness
            // does not enter into it — and that was our bug: we required
            // `data_in_flight >= TX_INFLIGHT_MAX`, so a PARTIAL leak was
            // invisible. Measured on the device: 8 of 16 slots held frames the
            // firmware never completed, for 314 s, with nothing queued behind
            // them. Half the transmit capacity gone for the rest of the boot,
            // every pass resetting the counter, the watchdog never firing.
            let now_ms_pass = host::now_ms();
            if tx_done > 0 || a_ba_reclaims > 0 || self.data_in_flight == 0 {
                self.last_tx_done_ms = now_ms_pass;
            } else if now_ms_pass.wrapping_sub(self.last_tx_done_ms) > TX_WD_TIMEOUT_MS {
                // Budgeted: if this fires continuously the log stops being a
                // log. The report keeps the running count either way.
                if self.st.tx_wd_recoveries < 8 {
                    host::print("[ax200] TX queue stuck: ");
                    host::print_dec(self.data_in_flight);
                    host::print(" frame(s) unacknowledged for ");
                    host::print_dec(TX_WD_TIMEOUT_MS as u32);
                    host::print(" ms — reclaiming the slots\n");
                    self.dump_fw_error_log();
                }
                // Linux forces an NMI and restarts the firmware here. We cannot
                // do that cheaply, so we reclaim instead — the slots are lost
                // either way, and a half-width queue that keeps shrinking ends
                // as a dead link.
                self.data_in_flight = 0;
                self.data_bytes_in_flight = 0;
                self.st.tx_wd_recoveries = self.st.tx_wd_recoveries.wrapping_add(1);
                self.last_tx_done_ms = now_ms_pass;
            }
            // Adaptive pacing: while frames are flowing OR completions are still
            // pending, poll again in 1 ms so the RX ring is drained before it
            // overflows and queue slots free up quickly; when idle, 4 ms keeps the
            // RX latency floor low (the ping/round-trip baseline) while still
            // yielding the core (npk_sleep yields the fiber). A proper IRQ wake is
            // the eventual fix; 4 ms is the interim quick-win over the old 20 ms.
            // RX-silence watchdog. Frames of some kind always arrive on a live
            // channel — beacons alone are ~10/s. Total silence means the
            // firmware has no buffer to fill, not that the air went quiet.
            if rx_frames > 0 {
                last_rx_ms = host::now_ms();
                self.st.rx_wd_dry = 0;
            } else if host::now_ms().saturating_sub(last_rx_ms) > RX_SILENCE_MS {
                let armed = self.restock_all_rbs();
                self.st.rx_wd_fires = self.st.rx_wd_fires.wrapping_add(1);
                if armed == 0 {
                    self.st.rx_wd_dry = self.st.rx_wd_dry.wrapping_add(1);
                }
                // Budgeted like the TX watchdog above. This used to print
                // unconditionally on every round, so a permanent fault read as
                // an endless loop and the log stopped being a log. `armed` is
                // the fact worth having: 0 means the firmware still holds every
                // buffer, so the pool was never the cause.
                if self.st.rx_wd_fires <= 4 {
                    host::print("[ax200] RX silent - re-armed ");
                    host::print_dec(armed);
                    host::print(" of ");
                    host::print_dec(RX_NUM_RBS as u32);
                    host::print(" receive buffers\n");
                }
                // Linux answers a wedged RX path with iwl_force_nmi() and a
                // firmware restart. We cannot restart the firmware cheaply, so
                // after three rounds with nothing to re-arm take the one
                // escalation we have — read the error table once, then rebuild
                // the association instead of poking the ring forever.
                if self.st.rx_wd_dry == 3 && self.rf_killed() {
                    // Re-scanning against a dead radio is a loop, not a
                    // recovery: wait for the switch instead. Budgeted, because
                    // this is a state, not an event.
                    if self.st.rx_wd_fires <= 12 {
                        host::print("[ax200] RX silent because the radio is OFF (HW RF-kill) — not a driver fault\n");
                    }
                    self.st.rx_wd_dry = 0;
                } else if self.st.rx_wd_dry == 3 {
                    host::print("[ax200] RX silent and the firmware holds every buffer — error log:\n");
                    self.dump_fw_error_log();
                    if self.reconnect() {
                        assoc_at_ms = host::now_ms();
                        handshake_deadline = assoc_at_ms + HANDSHAKE_TIMEOUT_MS;
                        next_report = assoc_at_ms + REPORT_PERIOD_MS;
                    }
                    self.st.rx_wd_dry = 0;
                }
                last_rx_ms = host::now_ms();
            }

            // 4-way watchdog. An AP starts the handshake within milliseconds of
            // the association response; if nothing has arrived after this long,
            // either it gave up on us or we stopped hearing it — and in both
            // cases waiting forever is the one useless option. mac80211 does the
            // same (IEEE80211_ASSOC_TIMEOUT then a fresh attempt).
            if associated && !self.authorized && self.st.rx_eapol == 0 {
                let now = host::now_ms();
                if now >= handshake_deadline {
                    host::print("[ax200] no EAPOL ");
                    host::print_dec(((now - assoc_at_ms) / 1000) as u32);
                    host::print(" s after associating (frames to us: ");
                    host::print_dec(self.st.rx_to_us);
                    host::print(") - reconnecting\n");
                    self.blacklist_target();
                    if self.reconnect() {
                        assoc_at_ms = host::now_ms();
                        handshake_deadline = assoc_at_ms + HANDSHAKE_TIMEOUT_MS;
                        next_report = assoc_at_ms + REPORT_PERIOD_MS;
                    } else {
                        handshake_deadline = host::now_ms() + HANDSHAKE_TIMEOUT_MS;
                    }
                }
            }
            // Publish the link state from OUR state, every pass, instead of
            // trusting one event to arrive. `false` was set at the top of
            // reconnect() and `true` came only from wifid's AUTHORIZED — so a
            // single missed message left the carrier down forever, and with it
            // netdev::send refusing every packet: the link never came back by
            // itself. Now the kernel's view follows the handshake, edge or no
            // edge.
            // Carrier follows the ASSOCIATION, dormant follows the
            // authorization. Reported as one flag, the seconds between
            // association and the end of the 4-way read to the kernel as "the
            // link went away", and it answered with a full DHCP round.
            if self.authorized != self.link_published {
                self.link_published = self.authorized;
                host::netdev_set_link_state(self.assoc_aid != 0, !self.authorized);
                host::print(if self.authorized {
                    "[ax200] carrier UP (authorized)\n"
                } else {
                    "[ax200] carrier DOWN (not authorized)\n"
                });
            }
            let busy = rx_frames > 0 || tx_any || clen > 0 || self.data_in_flight > 0;
            if busy { self.st.loop_busy = self.st.loop_busy.wrapping_add(1); }
            // Publish the status snapshot once a second. Reading the clock is one
            // host call per pass; formatting happens 1/1000 of those.
            let now = host::now_ms();
            if now >= next_report {
                self.publish_report(now);
                next_report = now + REPORT_PERIOD_MS;
            }
            let t_work = host::now_us();
            self.st.prof_work_us += t_work.saturating_sub(t_pass);
            self.st.prof_passes += 1;
            host::sleep_ms(if busy { 1 } else { 4 });
            self.st.prof_sleep_us += host::now_us().saturating_sub(t_work);
        }
    }

    // Dispatch one control command from wifid (the supplicant).
    fn handle_wifi_cmd(&mut self, cmd: &[u8]) {
        match cmd.first().copied() {
            // TX_EAPOL: [op][len u16][frame] → unencrypted EAPOL to the AP.
            Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {
                self.st.tx_eapol = self.st.tx_eapol.wrapping_add(1);
                let len = ((cmd[2] as usize) << 8) | cmd[1] as usize;
                if cmd.len() >= 3 + len {
                    host::dprint("[ax200] TX_EAPOL (len ");
                    host::dprint_dec(len as u32);
                    host::dprint(")\n");
                    let dst = self.target_bssid;
                    // `critical`: there is no retransmit behind an EAPOL reply.
                    // And never `let _ =` on it — a handshake frame that failed
                    // to go out is the single most important thing the log can
                    // say, and it used to say nothing at all.
                    // Encrypt once the pairwise key exists. The 4-way's own
                    // msg2/msg4 go out before it is installed — plaintext, as
                    // they must be. But a GROUP REKEY arrives minutes later,
                    // with the PTK long in place, and mac80211 protects that
                    // frame like any other data frame
                    // (`ieee80211_tx_h_select_key`: the key is kept for
                    // anything with `ieee80211_is_data_present`). We sent every
                    // EAPOL frame in the clear, so the AP discarded our rekey
                    // answer, retried four times and then stopped talking to
                    // us — no deauth, association still "up".
                    let enc = self.ptk_installed;
                    if !self.tx_8023(dst, ETHERTYPE_EAPOL, &cmd[3..3 + len], enc, true) {
                        self.st.tx_eapol_dropped = self.st.tx_eapol_dropped.wrapping_add(1);
                        host::print("[ax200] EAPOL TX DROPPED — the AP will retry and then give up\n");
                    }
                }
            }
            // SET_KEY: [op][key_type][key_idx][cipher][key_len][key..][rsc 6].
            Some(CMD_SET_KEY) if cmd.len() >= 5 => {
                self.st.keys_set = self.st.keys_set.wrapping_add(1);
                let key_type = cmd[1]; // 0=PTK/pairwise 1=GTK/group
                let key_idx = cmd[2];
                let key_len = cmd[4] as usize;
                if cmd.len() >= 5 + key_len + 6 {
                    let key = &cmd[5..5 + key_len];
                    let rsc = &cmd[5 + key_len..5 + key_len + 6];
                    self.install_key(key_type == 1, key_idx, key, rsc);
                }
            }
            // AUTHORIZED: 4-way done → carrier up, IP data path live.
            Some(CMD_AUTHORIZED) => {
                self.authorized = true;
                // `iwl_mvm_sta_state_assoc_to_authorized` (mvm/mac80211.c:3901).
                // We used to set a bool and publish the link — the firmware was
                // never told. It kept the station in the pre-authorized state
                // for the rest of the connection, and `rs_fw_rate_init` reads
                // exactly that:
                //
                //     .max_ch_width = mvmsta->authorized ?
                //             rs_fw_bw_from_sta_bw(link_sta)
                //           : IWL_TLC_MNG_CH_WIDTH_20MHZ,
                //
                // Linux sends the same three commands again, in this order.
                // (`iwl_mvm_enable_beacon_filter` also belongs here and is
                // still missing — that is why `fw notifs 0` never moves.)
                self.mac_ctxt_assoc();      // callbacks->mac_ctxt_changed
                self.sta_assoc_update();    // callbacks->update_sta
                self.connect_tlc_config();  // iwl_mvm_rs_rate_init_all_links
                self.link_published = true;
                host::netdev_set_link(true);
                host::print("[ax200] *** AUTHORIZED *** — link up, data path live 🎉\n");
                let mut up = [0u8; 7];
                up[0] = EV_LINK_UP;
                up[1..7].copy_from_slice(&self.target_bssid);
                host::wifi_send_event(&up);
            }
            _ => {}
        }
    }

    // ── iwl_pcie_grab_nic_access + iwl_trans_pcie_read_mem ────────
    // Grab NIC access (so device SRAM is reachable) and read `out.len()` words
    // from device memory at `addr` through the HBUS periphery window (the read
    // data register auto-increments). Used only by the error-log dump.
    fn grab_nic_access(&self) -> bool {
        self.set_bit(CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
        for _ in 0..1500 {
            let gp = self.r32(CSR_GP_CNTRL);
            if gp & (CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY
                | CSR_GP_CNTRL_REG_FLAG_GOING_TO_SLEEP)
                == CSR_GP_CNTRL_REG_FLAG_MAC_CLOCK_READY
            {
                return true;
            }
            for _ in 0..64 {
                core::hint::spin_loop();
            }
        }
        false
    }

    fn read_mem(&self, addr: u32, out: &mut [u32]) {
        self.w32(HBUS_TARG_MEM_RADDR, addr);
        for w in out.iter_mut() {
            *w = self.r32(HBUS_TARG_MEM_RDAT);
        }
    }

    // ── iwl_mvm_dump_nic_error_log (mvm/utils.c) ──────────────────
    // Read the lmac + umac error tables from device SRAM. valid != 0 means the
    // firmware asserted; error_id classifies it and hcmd / last_cmd_id /
    // cmd_header name the command the firmware faulted on.
    fn dump_fw_error_log(&self) {
        if self.lmac_err_ptr == 0 || !self.grab_nic_access() {
            host::dprint("[ax200] err-log: no NIC access\n");
            return;
        }
        let mut l = [0u32; LERR_WORDS];
        self.read_mem(self.lmac_err_ptr, &mut l);
        host::dprint("[ax200] LMAC err: valid=0x");
        host::dprint_hex32(l[LERR_VALID]);
        host::dprint(" id=0x");
        host::dprint_hex32(l[LERR_ERROR_ID]);
        host::dprint(" data1=0x");
        host::dprint_hex32(l[LERR_DATA1]);
        host::dprint(" data2=0x");
        host::dprint_hex32(l[LERR_DATA2]);
        host::dprint(" data3=0x");
        host::dprint_hex32(l[LERR_DATA3]);
        host::dprint(" hcmd=0x");
        host::dprint_hex32(l[LERR_HCMD]);
        host::dprint(" last_cmd=0x");
        host::dprint_hex32(l[LERR_LAST_CMD_ID]);
        host::dprint("\n");

        if self.umac_err_ptr != 0 {
            let mut u = [0u32; UERR_WORDS];
            self.read_mem(self.umac_err_ptr, &mut u);
            host::dprint("[ax200] UMAC err: valid=0x");
            host::dprint_hex32(u[UERR_VALID]);
            host::dprint(" id=0x");
            host::dprint_hex32(u[UERR_ERROR_ID]);
            host::dprint(" data1=0x");
            host::dprint_hex32(u[UERR_DATA1]);
            host::dprint(" cmd_hdr=0x");
            host::dprint_hex32(u[UERR_CMD_HEADER]);
            host::dprint("\n");
        }
        host::mmio_clr32(self.mmio, CSR_GP_CNTRL, CSR_GP_CNTRL_REG_FLAG_MAC_ACCESS_REQ);
    }

    // Halt the firmware's DMA engines before the driver returns. The kernel
    // frees our DMA buffers on return; a still-running chip must not DMA into
    // them afterwards. sw_reset (CSR_RESET) stops the device.
    fn stop(&self) {
        self.set_bit(CSR_RESET, CSR_RESET_REG_FLAG_SW_RESET);
        host::sleep_ms(6);
    }
}

/// Log a DMA allocation's physical address (Stage 1 diagnostics).
fn log_dma(name: &str, d: &Dma) {
    host::dprint(name);
    host::dprint(": phys 0x");
    host::dprint_hex64(d.phys);
    host::dprint("\n");
}

/// iwl_fw_lookup_cmd_ver (fw/img.c): walk the embedded firmware's
/// IWL_UCODE_TLV_CMD_VERSIONS TLV for the (group, cmd) entry and return its
/// cmd_ver. Group 0 maps to LONG_GROUP (the legacy command space). Returns
/// IWL_FW_CMD_VER_UNKNOWN (99) if absent — callers fall back to a default.
fn fw_cmd_ver(group: u8, cmd: u8) -> u8 {
    let grp = if group == 0 { IWL_ALWAYS_LONG_GROUP } else { group };
    let mut off = FW_TLV_HEADER_LEN;
    while off + 8 <= FW.len() {
        let t = le32(FW, off);
        let l = le32(FW, off + 4) as usize;
        let body = off + 8;
        if body + l > FW.len() {
            break;
        }
        if t == IWL_UCODE_TLV_CMD_VERSIONS {
            let n = l / FW_CMD_VER_ENTRY_LEN;
            for i in 0..n {
                let e = body + i * FW_CMD_VER_ENTRY_LEN;
                if FW[e + 1] == grp && FW[e] == cmd {
                    return FW[e + 2]; // cmd_ver (may be IWL_FW_CMD_VER_UNKNOWN)
                }
            }
        }
        off = body + ((l + 3) & !0x3);
    }
    IWL_FW_CMD_VER_UNKNOWN
}

/// fw_has_capa (iwl-drv.c iwl_set_ucode_capabilities): true if the firmware's
/// IWL_UCODE_TLV_ENABLED_CAPABILITIES TLVs set capability bit `cap`. Each such
/// TLV is { __le32 api_index; __le32 api_capa }; bit `cap` lives in the TLV with
/// api_index == cap/32, at position cap%32 in api_capa.
fn fw_has_capa(cap: u32) -> bool {
    let want_index = cap / 32;
    let want_bit = 1u32 << (cap % 32);
    let mut off = FW_TLV_HEADER_LEN;
    while off + 8 <= FW.len() {
        let t = le32(FW, off);
        let l = le32(FW, off + 4) as usize;
        let body = off + 8;
        if body + l > FW.len() {
            break;
        }
        if t == IWL_UCODE_TLV_ENABLED_CAPABILITIES && l >= 8 {
            if le32(FW, body) == want_index && le32(FW, body + 4) & want_bit != 0 {
                return true;
            }
        }
        off = body + ((l + 3) & !0x3);
    }
    false
}

/// Log one command's firmware version (Stage 4d1 diagnostics).
fn log_cmd_ver(name: &str, group: u8, cmd: u8) {
    host::dprint("[ax200]   ");
    host::dprint(name);
    host::dprint(" v=");
    host::dprint_hex32(fw_cmd_ver(group, cmd) as u32);
    host::dprint("\n");
}

/// iwl_flip_hw_address: build the 6-byte MAC from the two CSR registers.
/// addr0 holds bytes [3,2,1,0] (high→low), addr1 holds bytes [4,5] in its low
/// half (byte1, byte0). On a little-endian host iwl_read32 + cpu_to_le32 leaves
/// the register value with byte k at (val >> 8*k).
fn mac_from_regs(addr0: u32, addr1: u32) -> [u8; 6] {
    [
        (addr0 >> 24) as u8,
        (addr0 >> 16) as u8,
        (addr0 >> 8) as u8,
        addr0 as u8,
        (addr1 >> 8) as u8,
        addr1 as u8,
    ]
}

/// is_valid_ether_addr: not multicast (bit 0 of first octet clear) and not all-zero.
fn is_valid_mac(mac: &[u8; 6]) -> bool {
    (mac[0] & 0x1) == 0 && mac.iter().any(|&b| b != 0)
}

/// iwl_trans_is_hw_error_value (iwl-trans.h).
fn is_hw_error_value(val: u32) -> bool {
    (val & !0xf) == 0xa5a5_a5a0 || (val & !0xf) == 0x5a5a_5a50
}

/// Read a 16-bit value from PCI config space (dword read + extract).
fn pci_read16(off: u8) -> u16 {
    let dword = host::pci_read_config(off & !0x3);
    ((dword >> ((off & 0x3) * 8)) & 0xFFFF) as u16
}

/// Walk the PCI capability list for the given capability ID. Returns the
/// config-space offset of the capability, or 0 if absent.
fn pcie_find_cap(id: u8) -> u8 {
    let mut ptr = (host::pci_read_config(PCI_CAP_PTR as u8) & 0xFC) as u8;
    let mut guard = 0;
    while ptr != 0 && guard < 48 {
        let hdr = host::pci_read_config(ptr);
        if (hdr & 0xFF) as u8 == id {
            return ptr;
        }
        ptr = ((hdr >> 8) & 0xFC) as u8;
        guard += 1;
    }
    0
}

/// Strip spaces, tabs and CR from both ends of a config token.
fn cfg_trim(v: &[u8]) -> &[u8] {
    let (mut a, mut b) = (0, v.len());
    while a < b && matches!(v[a], b' ' | b'\t' | b'\r') { a += 1; }
    while b > a && matches!(v[b - 1], b' ' | b'\t' | b'\r') { b -= 1; }
    &v[a..b]
}

/// Value of `key` in a `key: value` config, or None. `#` starts a comment;
/// only the first colon splits, so a value may contain more of them.
fn cfg_get<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    for line in text.split(|&b| b == b'\n') {
        let line = cfg_trim(line);
        if line.is_empty() || line[0] == b'#' { continue; }
        let Some(c) = line.iter().position(|&b| b == b':') else { continue };
        if cfg_trim(&line[..c]) == key {
            return Some(cfg_trim(&line[c + 1..]));
        }
    }
    None
}

fn cfg_on(v: Option<&[u8]>) -> bool {
    matches!(v, Some(v) if v.starts_with(b"on") || v.starts_with(b"1"))
}

fn cfg_settle_ms(text: &[u8]) -> u32 {
    let Some(v) = cfg_get(text, b"settle_ms") else { return SETTLE_MS_DEFAULT };
    let mut n = 0u32;
    let mut any = false;
    for &c in v {
        if c.is_ascii_digit() { n = n * 10 + (c - b'0') as u32; any = true; } else { break; }
    }
    if any { n.min(20_000) } else { SETTLE_MS_DEFAULT }
}

/// The settle pause, read before the driver struct exists.
fn settle_ms_config() -> u32 {
    let mut cfg = [0u8; WIFI_CFG_MAX];
    let n = host::fetch(WIFI_CFG_PATH, &mut cfg);
    cfg_settle_ms(&cfg[..n])
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // Wait before touching the card at all.
    //
    // The device comes up only when a USB dongle is present — and it does not
    // matter whether that dongle has a cable. Mere presence makes
    // netdev::is_available() true, which sends boot into a DHCP with three
    // retries plus an NTP attempt: several seconds during which nobody touches
    // this card. Without it autostart reaches the driver almost immediately
    // after power-up. So the delay is the difference, and it belongs HERE,
    // before pci_bind — not somewhere in the middle of bring-up, where a plain
    // sleep would also strand the RX ring.
    let settle = settle_ms_config();
    if settle > 0 {
        host::sleep_ms(settle); // no ring allocated yet — sleeping is safe here
    }

    host::print("[ax200] Intel Wi-Fi 6 AX200 driver v");
    host::print(DRIVER_VERSION);
    host::print(" - HT rates + live link diagnostics (run 'wlan')\n");

    // ── Stage 0a: bind, bus master, map BAR0, identity ───────────
    let rc = host::pci_bind(AX200_VENDOR, AX200_DEVICE);
    if rc != 0 {
        host::dprint("[ax200] PCI bind failed (");
        match rc {
            -1 => host::dprint("not found"),
            -2 => host::dprint("denied"),
            _ => host::dprint("unknown error"),
        }
        host::dprint(")\n");
        return;
    }
    host::dprint("[ax200] PCI bind OK\n");

    host::pci_enable_bus_master();

    let mmio = host::mmio_map_bar(BAR_CSR, 16);
    if mmio < 0 {
        host::dprint("[ax200] BAR0 map failed\n");
        return;
    }
    host::dprint("[ax200] BAR0 mapped\n");

    let mut dev = Ax200 {
        mmio,
        ltr_enabled: false,
        hw_rev: 0,
        rxq_bd: Dma::NONE,
        rxq_used_bd: Dma::NONE,
        rxq_rb_stts: Dma::NONE,
        cmd_tfd: Dma::NONE,
        cmd_first_tb: Dma::NONE,
        cmd_data: Dma::NONE,
        cmd_write_ptr: 0,
        rb_pool: [Dma::NONE; RX_NUM_RBS],
        rb_in_fw: [false; RX_NUM_RBS],
        rxq_read: 0,
        free_bd_write: 0,
        lmac_err_ptr: 0,
        umac_err_ptr: 0,
        scan_chans: [0; SCAN_MAX_CHANS],
        scan_bands: [0; SCAN_MAX_CHANS],
        n_scan_chans: 0,
        mac: [0; 6],
        target_bssid: [0; 6],
        target_chan: 0,
        target_band: 0,
        target_beacon_int: 0,
        target_ssid: [0; SSID_MAX],
        target_ssid_len: 0,
        target_privacy: false,
        target_rssi: 0,
        target_valid: false,
        target_ht: HtCap::NONE,
        target_dtim_period: 0,
        assoc_aid: 0,
        qos: false,
        sync_tsf: 0,
        sync_device_ts: 0,
        sync_dtim_count: 0,
        mgmt_tfd: Dma::NONE,
        mgmt_first_tb: Dma::NONE,
        mgmt_payload: Dma::NONE,
        mgmt_bc_tbl: Dma::NONE,
        mgmt_queue_id: 0,
        mgmt_write_ptr: 0,
        data_tfd: Dma::NONE,
        data_first_tb: Dma::NONE,
        data_payload: Dma::NONE,
        data_bc_tbl: Dma::NONE,
        data_queue_id: 0,
        data_write_ptr: 0,
        data_read_ptr: 0,
        key_slot_used: [false; STA_KEY_MAX_NUM as usize],
        key_slot_freed: [0; STA_KEY_MAX_NUM as usize],
        key_slot_prev: None,
        ptk_installed: false,
        data_in_flight: 0,
    data_bytes_in_flight: 0,
    slot_bytes: [0; IWL_DATA_QUEUE_SIZE],
        last_tx_done_ms: 0,
        tx_seq: 0,
        st: Stats::NEW,
        n_aps: 0,
        alt_bssid: [0; 6],
        alt_chan: 0,
        alt_rssi: 0,
        alt_ht: false,
        alt_valid: false,
        authorized: false,
        want_ssid: [0; SSID_MAX],
        want_ssid_len: 0,
        band_pref: BAND_PREF_AUTO,
        want_power_save: false,
        want_bt_coex: false,
        settle_ms: SETTLE_MS_DEFAULT,
        sync_ok: false,
        fw_assert: 0,
        blacklist: [[0u8; 6]; 4],
        n_blacklist: 0,
        pick_reason: PICK_STRONGEST,
        ba_pending: None,
        want_ampdu: false,
        want_ht40: false,
        want_vht: false,
        want_txagg: false,
        ba_fw_broken: false,
        link_published: false,
        tx_fail_streak: 0,
        tx_fail_streak_peak: 0,
        last_tx_resp_ms: 0,
    };
    dev.st.start_ms = host::now_ms();

    let hw_rev = dev.r32(CSR_HW_REV);
    dev.hw_rev = hw_rev;
    let rf_id = dev.r32(CSR_HW_RF_ID);
    host::log_reg("CSR_HW_REV", hw_rev);
    host::log_reg("CSR_HW_RF_ID", rf_id);

    if hw_rev == 0xFFFF_FFFF || hw_rev == 0 {
        host::dprint("[ax200] HW_REV looks wrong — chip not responding?\n");
        return;
    }
    // RF type lives in the high nibble pattern; HR is the AX200's radio.
    if rf_id & 0x0FFF_F000 == CSR_HW_RF_ID_TYPE_HR & 0x0FFF_F000 {
        host::dprint("[ax200] RF type = HR (matches AX200) — Stage 0a OK\n");
    } else {
        host::dprint("[ax200] RF type unexpected (continuing anyway)\n");
    }

    // ── Stage 0b: reset + APM bring-up ───────────────────────────
    if !dev.start_hw() {
        host::dprint("[ax200] Stage 0b FAILED\n");
        return;
    }
    host::dprint("[ax200] Stage 0b OK — chip powered up, PRPH accessible\n");

    // ── Stage 1: RX/TX rings + command queue ─────────────────────
    if !dev.nic_init() {
        host::dprint("[ax200] Stage 1 FAILED\n");
        return;
    }
    host::dprint("[ax200] Stage 1 OK — rings allocated:\n");
    log_dma("  rxq.bd      ", &dev.rxq_bd);
    log_dma("  rxq.used_bd ", &dev.rxq_used_bd);
    log_dma("  rxq.rb_stts ", &dev.rxq_rb_stts);
    log_dma("  cmd.tfd     ", &dev.cmd_tfd);
    log_dma("  cmd.first_tb", &dev.cmd_first_tb);

    // ── Stage 2: context-info + FW self-load + ALIVE ─────────────
    if dev.load_firmware() {
        host::dprint("[ax200] Stage 2 OK — *** FIRMWARE ALIVE *** 🎉\n");

        // ── Stage 3: RX restock + read the ALIVE notification ────
        match dev.rx_restock_and_alive() {
            Some(rb0) => {
                host::dprint("[ax200] Stage 3 OK — RX path live, alive notification received\n");

                // ── Stage 4a: parse the ALIVE notification struct ──
                if dev.parse_alive_ntf(&rb0) {
                    host::dprint("[ax200] Stage 4a OK — firmware ALIVE valid (status OK)\n");

                    // ── Stage 4b: init-flow host commands → INIT_COMPLETE ──
                    if dev.run_init_handshake() {
                        host::dprint("[ax200] Stage 4b OK — *** INIT_COMPLETE_NOTIF received *** 🎉\n");

                        // ── Stage 4c: read NVM info (caps + MAC address) ──
                        if dev.read_nvm() {
                            host::dprint("[ax200] Stage 4c OK — NVM info read\n");

                            // ── Stage 4d1: firmware command versions (scan path) ──
                            host::dprint("[ax200] FW cmd versions (scan path):\n");
                            log_cmd_ver("SCAN_REQ_UMAC ", IWL_ALWAYS_LONG_GROUP, SCAN_REQ_UMAC);
                            log_cmd_ver("SCAN_CFG_CMD  ", IWL_ALWAYS_LONG_GROUP, SCAN_CFG_CMD);
                            log_cmd_ver("ADD_STA       ", IWL_ALWAYS_LONG_GROUP, ADD_STA);
                            log_cmd_ver("PHY_CONTEXT   ", IWL_ALWAYS_LONG_GROUP, PHY_CONTEXT_CMD);
                            log_cmd_ver("MAC_CONTEXT   ", IWL_ALWAYS_LONG_GROUP, MAC_CONTEXT_CMD);
                            log_cmd_ver("TX_ANT_CFG    ", IWL_ALWAYS_LONG_GROUP, TX_ANT_CONFIGURATION_CMD);
                            log_cmd_ver("SCAN_COMPLETE ", IWL_ALWAYS_LONG_GROUP, SCAN_COMPLETE_UMAC);
                            host::dprint("[ax200] Stage 4d1 OK — cmd versions read\n");

                            // Which network, which band, radio power — read
                            // before the prerequisites, because POWER_TABLE_CMD
                            // goes out in there.
                            dev.load_connect_policy();

                            // Let the radio settle before scanning.
                            //
                            // The device only works when it was booted with a
                            // USB dongle plugged in — which changes nothing
                            // about this card except how long the rest of boot
                            // takes (enumeration, plus a DHCP that succeeds
                            // instead of running into three timeouts). Every
                            // other difference has been ruled out by now: the
                            // DMA addresses come out byte-identical, the init
                            // sequence matches iwl_run_unified_mvm_ucode
                            // exactly, power save and BT coex are off. What is
                            // left is that we start scanning sooner. Configurable
                            // so it can be measured rather than believed.
                            // ── Stage 4d2a: scan-config prerequisites ──
                            dev.run_scan_prereqs();
                            host::dprint("[ax200] Stage 4d2a OK — full iwl_mvm_up pre-scan seq sent\n");

                            // ── Stage 4d2b1b: add the MAC context the scan ──
                            // references (scan_start_mac_or_link_id → ctx id 0).
                            dev.add_mac_context();
                            host::dprint("[ax200] Stage 4d2b1b OK — MAC context added\n");

                            // ── Stage 4d2b1/2: passive scan → SCAN_COMPLETE,
                            // parse beacons → access points (SSID/BSSID/RSSI). ──
                            if dev.run_scan() {
                                host::dprint("[ax200] Stage 4d2b2 OK — *** SCAN COMPLETE, APs listed *** 🎉\n");

                                // ── Stage 5a: PHY context + RLC + binding ──
                                // First connect step (no TX yet): set the target
                                // AP's operating channel + bind MAC↔PHY. Target =
                                // strongest AP from the scan.
                                let mut associated = false;
                                if dev.target_valid && dev.connect_phy_binding()
                                    && dev.connect_add_station()
                                {
                                    // ── Stage 5b': power + MAC context (target BSSID) ──
                                    // The chanctx tail Linux runs before the auth TX.
                                    dev.connect_finish_chanctx();
                                    // Allocate the data TX queue BEFORE auth so its
                                    // SCD-response wait doesn't discard the AP's
                                    // first EAPOL frame, and the post-assoc listen
                                    // can begin immediately.
                                    if !dev.alloc_data_queue() {
                                        // Without this queue there is no path
                                        // for EAPOL, so the 4-way cannot run and
                                        // the link can never authorize. `dprint`
                                        // was the wrong channel for that.
                                        host::print("[ax200] FATAL: data TX queue DMA alloc failed — no 4-way, no traffic\n");
                                    }
                                    // ── Stage 5c/5d: AUTH → ASSOC mgmt dialog ──
                                    // Retry the whole auth+assoc up to 3× like
                                    // mac80211 (IEEE80211_AUTH_MAX_TRIES /
                                    // ASSOC_MAX_TRIES): a single lost mgmt frame
                                    // must not abort the connect.
                                    for attempt in 0..3 {
                                        if attempt > 0 {
                                            host::print("[ax200] connect retry ");
                                            host::print_dec(attempt as u32 + 1);
                                            host::print("/3...\n");
                                        }
                                        if dev.connect_send_auth() && dev.connect_send_assoc() {
                                            associated = true;
                                            break;
                                        }
                                    }
                                }

                                // ── Register as a NIC + go resident ──
                                // run_netdev never returns: the driver owns the
                                // card and the `wlan` interface for its lifetime
                                // (same model as aml.wasm). It only tells wifid the
                                // link is READY (→ the 4-way) when we actually
                                // associated — otherwise wifid would arm a
                                // supplicant for a BSS we never joined and stall.
                                dev.run_netdev(associated);
                            } else {
                                host::dprint("[ax200] Stage 4d2b1 FAILED — no SCAN_COMPLETE\n");
                            }
                        } else {
                            host::dprint("[ax200] Stage 4c FAILED — no NVM response\n");
                        }
                    } else {
                        host::dprint("[ax200] Stage 4b FAILED — no INIT_COMPLETE\n");
                    }
                } else {
                    host::dprint("[ax200] Stage 4a FAILED — firmware ALIVE not valid\n");
                }
            }
            None => host::dprint("[ax200] Stage 3 FAILED\n"),
        }
    } else {
        host::dprint("[ax200] Stage 2 FAILED (no ALIVE)\n");
    }

    // Halt the chip before returning: the kernel frees our DMA buffers on
    // return and a still-running firmware must not DMA into them afterwards.
    // (Probe-stage driver — we return rather than idle; npk_input_wait HLTs
    // without yielding, which would pin the core. A persistent yielding
    // run-loop arrives with Stage 4+.)
    dev.stop();
}
