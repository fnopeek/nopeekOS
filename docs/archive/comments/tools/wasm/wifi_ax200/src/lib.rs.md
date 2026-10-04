# `tools/wasm/wifi_ax200/src/lib.rs` @ 5e0102684

## L1-11 · `#![no_std]`

```
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
```

## L25-31 · `#[panic_handler]`

```
/// A panic used to spin here in silence: `loop {}` with the argument thrown
/// away. The module then hangs with every counter frozen at the instant it
/// died, the report stops being republished, and from the outside it is
/// indistinguishable from "the AP went quiet" — which cost an entire evening
/// of chasing the radio while the driver was standing still. Say where it
/// happened; `Location` survives `strip = true` because it is static data
/// referenced by the panic site, not a symbol.
```

## L34-40 · `host::print("\n[ax200] PANIC — driver stopped");`

```
// Two channels on purpose. `print` reaches the terminal this driver was
// launched from and is worker-core safe, so it lands where someone is
// looking right now. `log` goes through kprintln to the serial capture,
// which is the ONLY thing `dmesg` reads — without it the line dies with
// the scrollback and cannot be recovered after a reboot. Print first: if
// the kprintln path stalls (it routes through shade), the visible half has
// already happened, and we are on our way to `loop {}` regardless.
```

## L53-54 · `static FW: &[u8] = include_bytes!("../firmware/iwlwifi-cc-a0-77.ucode");`

```
/// AX200 runtime firmware (unified ucode), embedded like the RTL driver embeds
/// rtw8852b_fw.bin. API 77 = the exact version Linux 6.18.26 requests.
```

## L57-61 · `const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");`

```
/// One source for the version string: the boot banner and every status snapshot
/// carry it, so a device measurement can never be traced to the wrong build.
// From Cargo.toml, never by hand: this string stood at 0.60.1 for three
// releases while the module was 0.63.0, and a report that misstates its own
// version makes every number in it suspect.
```

## L64 · `fn le32(b: &[u8], off: usize) -> u32 {`

```
// Little-endian readers over the embedded firmware.
```

## L68 · `fn put_u16(buf: &mut [u8], off: usize, v: u16) {`

```
// Little-endian writers into the context-info buffer.
```

## L78 · `fn encode_bits(v: u32, mask: u32) -> u32 {`

```
/// u32_encode_bits: place `v` into the field described by `mask`.
```

## L83-84 · `fn put_tfh_tb(tfd: &mut [u8], i: usize, len: u16, addr: u64) {`

```
/// Write transfer block `i` (iwl_tfh_tb { __le16 tb_len; __le64 addr }) into a
/// TFD. tbs[] start at offset 2 (after num_tbs); addr is stored unaligned.
```

## L91 · `#[derive(Clone, Copy)]`

```
/// A coherent DMA allocation: kernel-held physical address + WASM handle.
```

## L102-104 · `#[derive(Clone, Copy)]`

```
/// What the firmware told us about a frame's place in its block-ack window.
/// Read from the descriptor while it is already in hand — a second DMA read per
/// received frame would cost more than the reordering saves.
```

## L109-110 · `amsdu_last: bool,`

```
/// Not an A-MSDU, or the last sub-frame of one. NSSN advances on the FIRST
/// sub-frame, so acting on it earlier releases frames still in flight.
```

## L112-115 · `reorderable: bool,`

```
/// Unicast QoS data — the only thing a block-ack session covers.
/// `iwl_mvm_reorder` bypasses everything else, and the frames it lets past
/// are exactly the ones a client without an address depends on: a DHCP offer
/// comes back to the broadcast address, and the window must never hold it.
```

## L119-121 · `#[derive(Clone, Copy)]`

```
/// An ADDBA request we have asked the firmware about and not yet answered. The
/// AP's dialog token, parameter set and timeout have to survive until the
/// firmware replies, because its answer decides what we put in ours.
```

## L129-133 · `asked_ms: u64,`

```
/// When we asked the firmware. The AP gets its answer only after the
/// firmware hands back a BAID — so if that never comes, the AP is left
/// waiting for a reply that never arrives, and at least one AP answers
/// that by sending NO DATA AT ALL on the TID: association up, DHCP
/// never completes. A pending request must therefore expire.
```

## L137-145 · `#[derive(Clone, Copy)]`

```
/// Classification of a received 802.11 data frame addressed to us.
/// Running signal strength, `DECLARE_EWMA(signal, 10, 8)` (mac80211/sta_info.h:424)
/// — 1/8 weight on each new sample, carried at 2^10 precision so the average
/// does not quantise to whole dBm.
///
/// It exists because `rssi` in the report was written ONCE, at association
/// (`target_rssi`, from the scan), and never again. Carrying the laptop up to
/// the AP and back changed nothing in the report, which is exactly what the
/// number could do: it was minutes old and about a different position.
```

## L148 · `scaled: i32,`

```
/// dBm << 10, or 0 while nothing has been measured.
```

## L161 · `self.scaled += (v - self.scaled) / 8;`

```
// ewma_add: avg += (new - avg) / weight, weight = 8.
```

## L167 · `pub fn merge(&mut self, other: &SignalAvg) {`

```
/// Fold a pass' worth of samples into a longer-lived average.
```

## L187-189 · `Undecoded,`

```
/// A data frame addressed to us whose payload we could not locate. Distinct
/// from None on purpose: "nothing arrives" and "everything arrives and we
/// drop it" are opposite faults and were indistinguishable before.
```

## L191 · `Eapol(usize),`

```
/// EAPOL-Key frame (the 4-way) → forward to wifid. `out` holds the frame.
```

## L193 · `Ip(usize, Agg),`

```
/// IP/other data → the kernel IP stack. `out` holds an Ethernet frame.
```

## L197-200 · `#[derive(Clone, Copy)]`

```
/// The AP's 802.11n capabilities, read from the HT Capability element of its
/// beacon. Everything we do at HT level derives from these: what we may put in
/// our own assoc request, the station flags, and the MCS set TLC scales over
/// (rs_fw_set_supp_rates uses the PEER's rx_mask — what the AP can receive).
```

## L204 · `ampdu_factor: u8,  // A-MPDU length exponent (0-3)`

```
// A-MPDU length exponent (0-3)
```

## L205 · `ampdu_density: u8, // minimum MPDU start spacing (0-7)`

```
// minimum MPDU start spacing (0-7)
```

## L206 · `mcs_rx: [u8; 2],   // rx_mask[0] = MCS 0-7, rx_mask[1] = MCS 8-15`

```
// rx_mask[0] = MCS 0-7, rx_mask[1] = MCS 8-15
```

## L207-209 · `sec_chan_offs: u8,`

```
/// From the HT OPERATION element, not the capability one: where the
/// secondary 20 MHz channel sits. NONE means the AP runs 20 MHz only, and
/// then a 40 MHz PHY context would be pointing at nothing.
```

## L211 · `vht: bool,`

```
/// VHT capability element present, and its two fields we act on.
```

## L215-216 · `vht_chan_width: u8,`

```
/// From the VHT OPERATION element: USE_HT (fall back to the HT width) or
/// 80MHZ. As with HT, the capability says CAN, the operation says DOES.
```

## L218 · `vht_seg0: u8,`

```
/// Centre-frequency channel index of the 80 MHz block.
```

## L220-224 · `he: bool,`

```
/// The HE (802.11ax) elements. For an AP that carries HE Capability AND an
/// HE Operation with the VHT-Operation-Info bit, those three bytes ARE the
/// operating width — `ieee80211_determine_ap_chan` reads them and never
/// looks at element 192. A Wi-Fi 6 AP is the normal case here, so without
/// this the width question is answered from the wrong element.
```

## L229-230 · `ht_op_mode: u16,`

```
/// HT Operation `operation_mode` — the AP states here what protection the
/// BSS needs, and it is the only place that information exists.
```

## L232 · `erp_protect: bool,`

```
/// ERP Use_Protection: legacy 802.11b stations present.
```

## L235-238 · `wmm: bool,`

```
/// EDCA parameters from the AP's WMM Parameter element, indexed by the
/// mac80211 AC number (VO, VI, BE, BK) — the same shape as Linux'
/// `queue_params[]`. Without these the MAC context carries an all-zero
/// EDCA table and no `MAC_QOS_FLG_UPDATE_EDCA`.
```

## L240 · `edca: [(u8, u16, u16, u16); 4],`

```
/// (aifs, cw_min, cw_max, txop) per AC. cw_* already expanded from ECW.
```

## L267 · `#[derive(Clone, Copy)]`

```
/// A discovered access point (from a scan beacon / probe response).
```

## L273 · `rssi: i8, // dBm`

```
// dBm
```

## L275 · `beacon_int: u16, // beacon interval (TU) — for the connect MAC context`

```
// beacon interval (TU) — for the connect MAC context
```

## L276 · `privacy: bool,   // capability Privacy bit (encrypted → needs RSN in assoc-req)`

```
// capability Privacy bit (encrypted → needs RSN in assoc-req)
```

## L277 · `dtim_period: u8, // from the TIM element — the MAC context needs it to associate`

```
// from the TIM element — the MAC context needs it to associate
```

## L279-285 · `tsf: u64,`

```
// The beacon timing the associated MAC context is built from. Linux keeps
// exactly these three with the SCAN RESULT and reads them back at
// association (mac80211/mlme.c:9464 — sync_tsf from the stored beacon,
// sync_device_ts from bss->device_ts_beacon, sync_dtim_count from its TIM).
// It never waits for a fresh beacon, which is what we used to do — an extra
// step of our own invention that failed every time and left the firmware
// with a made-up wake schedule.
```

## L289 · `has_beacon: bool, // a probe response carries no usable device timestamp`

```
// a probe response carries no usable device timestamp
```

## L309-313 · `#[derive(Clone, Copy)]`

```
/// Everything the host cannot see for itself. The kernel routes our frames but
/// knows nothing about the air: negotiated rate, retries, airtime. Without these
/// a slow link is indistinguishable from a busy one, so they are collected
/// always-on (not behind DEBUG) and published once a second via
/// `npk_driver_report`.
```

## L316 · `tx_frames: u32,`

```
// TX, cumulative.
```

## L319 · `tx_blocked: u32, // times the in-flight BYTE cap stopped us pulling another frame`

```
// times the in-flight BYTE cap stopped us pulling another frame
```

## L320 · `tx_blocked_ring: u32, // times the ring guard did instead — the byte cap was not the limit`

```
// times the ring guard did instead — the byte cap was not the limit
```

## L321 · `tx_wd_recoveries: u32, // times the queue watchdog reclaimed leaked TX slots`

```
// times the queue watchdog reclaimed leaked TX slots
```

## L322 · `inflight_corrections: u32, // times the derived read pointer beat the counter`

```
// times the derived read pointer beat the counter
```

## L323 · `gtk_installs: u32,     // group keys installed = 4-way once + one per rekey`

```
// group keys installed = 4-way once + one per rekey
```

## L324 · `tx_eapol_dropped: u32, // handshake replies that never reached the air`

```
// handshake replies that never reached the air
```

## L326 · `tx_ok: u32,`

```
// TX completions (iwl_tx_resp), cumulative.
```

## L329 · `tx_retries: u32,  // sum of failure_frame — retransmissions on the air`

```
// sum of failure_frame — retransmissions on the air
```

## L330 · `tx_rts_fail: u32, // sum of failure_rts`

```
// sum of failure_rts
```

## L332-335 · `tx_subframes: u64, // sum of frame_count over all responses`

```
// Is the firmware aggregating what we transmit? `frame_count` in every TX
// response answers it outright (1 = no aggregation, >1 = aggregation), and
// the compressed block-ack notification is the second witness. We read
// neither until now, which is why "no TX aggregation" was an assumption.
```

## L336 · `tx_subframes: u64, // sum of frame_count over all responses`

```
// sum of frame_count over all responses
```

## L337 · `tx_agg_resp: u32,  // responses with frame_count > 1`

```
// responses with frame_count > 1
```

## L338 · `tx_agg_max: u8,    // largest frame_count seen`

```
// largest frame_count seen
```

## L339 · `ba_notifs: u32,    // BA_NOTIF received`

```
// BA_NOTIF received
```

## L340 · `ba_reclaims: u32,  // TFD read pointers taken from them — the aggregated TX return path`

```
// TFD read pointers taken from them — the aggregated TX return path
```

## L341 · `ba_tfd_over: u32,  // tfd_cnt beyond CBA_TFD_MAX: slots we could NOT reclaim`

```
// tfd_cnt beyond CBA_TFD_MAX: slots we could NOT reclaim
```

## L342 · `ba_txed: u64,      // MPDUs the firmware says it sent in aggregates`

```
// MPDUs the firmware says it sent in aggregates
```

## L343 · `ba_done: u64,      // …and how many were acknowledged`

```
// …and how many were acknowledged
```

## L346 · `rx_frames: u32,`

```
// RX, cumulative.
```

## L350 · `rx_eapol: u32,   // 4-way frames received from the AP`

```
// 4-way frames received from the AP
```

## L351 · `tx_eapol: u32,   // …and answers wifid asked us to send back`

```
// …and answers wifid asked us to send back
```

## L352 · `keys_set: u32,   // SET_KEY commands honoured (PTK + GTK = 2)`

```
// SET_KEY commands honoured (PTK + GTK = 2)
```

## L353 · `ready_sent: u32, // EV_READY handed to wifid (once per association)`

```
// EV_READY handed to wifid (once per association)
```

## L355-368 · `rx_airtime_us: u64,`

```
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
```

## L372-373 · `pk_rx_airtime_pct: u32,`

```
// The window that carried the most bytes, kept whole. A blocking transfer
// takes the terminal with it, so the numbers have to survive it.
```

## L382 · `rx_drain_max: u32, // most frames drained in one pass — RX ring pressure`

```
// most frames drained in one pass — RX ring pressure
```

## L383-387 · `win_pass_empty: u32,`

```
// Are we starved, or are we the bottleneck? With the air at 30 % and the
// window at 8 MB, that is the whole remaining question, and the shape of
// the arrivals answers it: steady frames mean the ceiling is ours, sparse
// bursts with idle gaps mean the AP is not delivering. Per WINDOW, then
// snapshot with the peak — an average over the whole uptime is idle time.
```

## L389 · `win_pass_few: u32,   // 1..=3`

```
// 1..=3
```

## L390 · `win_pass_many: u32,  // 4..=15`

```
// 4..=15
```

## L391 · `win_pass_burst: u32, // 16+`

```
// 16+
```

## L392 · `win_gap_max_us: u32, // longest run of passes that found nothing`

```
// longest run of passes that found nothing
```

## L399-403 · `rx_pool_exhausted: u32,`

```
// Passes that drained (nearly) the whole RB pool. There are only RX_NUM_RBS
// buffers: once they are all full the firmware has nowhere to put the next
// frame and drops it, which TCP sees as loss and answers by backing off. A
// rising count here means the poll loop is not keeping up, and no amount of
// air rate will help until it does.
```

## L405-411 · `rb_bad_vid: u32,`

```
// Free-BD ring health. `rb_bad_vid` = the firmware handed back a buffer id
// outside the pool or one we never posted (Linux: WARN + iwl_force_nmi).
// `rb_double_post` = we tried to post a buffer the firmware already owns —
// impossible by construction in Linux, so any count here is our bug.
// `rx_wd_fires` = RX-silence watchdog rounds, `rx_wd_dry` = consecutive
// rounds that found nothing to re-arm, which means the pool is not the
// fault and the firmware itself has stopped.
```

## L416-417 · `rx_to_us: u32,`

```
// Unicast frames carrying our address, whatever their type, and the subset
// we received but could not decode.
```

## L420 · `loop_iters: u32,`

```
// Loop + events.
```

## L424-427 · `addba_seen: u32,`

```
// Requests SEEN, separate from answers given. With `ampdu on` we answer the
// AP only after the firmware hands back a BAID — so if that status never
// arrives, both answer counters stay 0 and the AP is left waiting with no
// reply at all. Without this number those two cases look identical.
```

## L432 · `mb_notifs: u32,`

```
// What the firmware reports about the beacons we no longer see ourselves.
```

## L439-440 · `prof_work_us: u64,`

```
// Where a pass spends its microseconds, summed since the last report. Per
// pass and per frame these say whether the interpreter is the ceiling.
```

## L446-447 · `prof_work_pp: u64,`

```
// …reduced to per-pass values before the window is cleared, exactly like the
// throughput numbers above: the report is built AFTER the reset.
```

## L452 · `peak_work_pp: u64,`

```
// …and the same four from the second with the highest RX throughput.
```

## L458 · `last_tx_rate: u32,`

```
// Rates last reported by the firmware (raw rate_n_flags).
```

## L461-462 · `win_start_ms: u64,`

```
// Report window: throughput is computed here, where the counters and the
// clock both live, so the intent only has to print it.
```

## L472-474 · `peak_tput_tx_kbit: u32,`

```
// Best window seen since start. A blocking load generator (netbench holds
// the terminal until it finishes) leaves nothing to read afterwards if only
// the live window is kept — by then the link is idle again.
```

## L514 · `struct Rep {`

```
/// Fixed-size text builder for the status snapshot (no allocator in a driver).
```

## L523-524 · `const LIMIT: usize = REPORT_CAP - 16;`

```
/// Reserve the tail for a marker: silent truncation cost a whole debugging
/// round once already (the last two lines were simply gone from the report).
```

## L555-556 · `fn kbit_as_mbit(&mut self, kbit: u32) {`

```
/// A value in thousandths printed as `x.y` — the driver has no float
/// formatting and 11.9 Mbit/s reads better than 11900 kbit/s.
```

## L581 · `struct Ax200 {`

```
/// AX200 transport state. Mirrors the bits of `struct iwl_trans_pcie` we use.
```

## L586 · `rxq_bd: Dma,       // RBD ring (__le64 * NUM_RBDS)`

```
// RX queue DMA (iwl_pcie_alloc_rxq_dma) — addresses go into ctxt_info.
```

## L587 · `rxq_bd: Dma,       // RBD ring (__le64 * NUM_RBDS)`

```
// RBD ring (__le64 * NUM_RBDS)
```

## L588 · `rxq_used_bd: Dma,  // used-BD ring (__le32 * NUM_RBDS)`

```
// used-BD ring (__le32 * NUM_RBDS)
```

## L589 · `rxq_rb_stts: Dma,  // struct iwl_rb_status`

```
// struct iwl_rb_status
```

## L590 · `cmd_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_CMD_QUEUE_SIZE)`

```
// TX command queue DMA (iwl_pcie_txq_alloc, gen2).
```

## L591 · `cmd_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_CMD_QUEUE_SIZE)`

```
// TFD ring (iwl_tfh_tfd * IWL_CMD_QUEUE_SIZE)
```

## L592 · `cmd_first_tb: Dma, // first-TB staging buffers`

```
// first-TB staging buffers
```

## L593 · `cmd_data: Dma,     // payload buffer for large (NOCOPY) commands → TB1`

```
// payload buffer for large (NOCOPY) commands → TB1
```

## L594 · `cmd_write_ptr: u32, // txq->write_ptr for the command queue`

```
// txq->write_ptr for the command queue
```

## L595-596 · `rb_pool: [Dma; RX_NUM_RBS],`

```
// RX RB pool (vid v → rb_pool[v-1]) + our read index into the used-BD ring
// + the free-BD ring write index (for recycling RBs during the scan).
```

## L598-603 · `rb_in_fw: [bool; RX_NUM_RBS],`

```
/// iwl_rx_mem_buffer.invalid, inverted: true while the buffer sits in the
/// free-BD ring and the firmware owns it, false while it is ours. Linux
/// keeps the same fact as list membership (rx_free / posted / rx_used) and
/// restocks strictly from rx_free. A buffer can therefore be in the ring at
/// most once, which is what keeps the write index from ever lapping the
/// firmware's read index — no matter how big the pool gets.
```

## L607-608 · `lmac_err_ptr: u32,`

```
// Firmware error-table SRAM pointers (from the ALIVE notification), for
// dumping the FW error log when a command/scan produces no response.
```

## L611-612 · `scan_chans: [u8; SCAN_MAX_CHANS], // channel numbers`

```
// Scan channel list parsed from the NVM_GET_INFO regulatory section: the
// NVM_CHANNEL_VALID channels with their PHY band, for both 2.4 and 5 GHz.
```

## L613 · `scan_chans: [u8; SCAN_MAX_CHANS], // channel numbers`

```
// channel numbers
```

## L614 · `scan_bands: [u8; SCAN_MAX_CHANS], // PHY_BAND_24 / PHY_BAND_5 per channel`

```
// PHY_BAND_24 / PHY_BAND_5 per channel
```

## L616 · `mac: [u8; 6],`

```
// The card's MAC address (from the CSR strap/OTP), for netdev registration.
```

## L618 · `target_bssid: [u8; 6],`

```
// Connect target picked from the scan (strongest AP) — for #3 connect (5a+).
```

## L621 · `target_band: u8, // PHY_BAND_24 / PHY_BAND_5`

```
// PHY_BAND_24 / PHY_BAND_5
```

## L622 · `target_beacon_int: u16, // beacon interval of the target AP (for MAC context)`

```
// beacon interval of the target AP (for MAC context)
```

## L625 · `target_privacy: bool, // target is encrypted (WPA2) → assoc-req carries an RSN IE`

```
// target is encrypted (WPA2) → assoc-req carries an RSN IE
```

## L626-627 · `target_rssi: i8,`

```
/// RSSI from the scan, at the moment we associated. Kept because it is what
/// the AP choice was made on — but it is NOT the link's signal now.
```

## L629 · `link_sig: SignalAvg,`

```
/// The live one, averaged over every frame the AP sends us.
```

## L632-635 · `target_ht: HtCap,`

```
// The target AP's HT capabilities + DTIM period (from its beacon during the
// scan) and the association id the AP handed us. `ht.present` gates the whole
// 802.11n path: HT + WMM elements in the assoc request, QoS data frames, HT
// station flags and TLC mode HT. An AP without HT keeps the legacy path.
```

## L639 · `qos: bool, // associated as an HT/QoS station → QoS data frames`

```
// associated as an HT/QoS station → QoS data frames
```

## L640-642 · `sync_tsf: u64,`

```
// Beacon timing captured right after association (iwl_mvm_set_fw_dtim_tbtt):
// the AP's TSF and our device timestamp at the last beacon, plus the DTIM
// count still to run. The MAC context needs them to be marked associated.
```

## L646 · `mgmt_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_MGMT_QUEUE_SIZE)`

```
// gen2 management TX queue for the AP station (auth/assoc frames, 5b+).
```

## L647 · `mgmt_tfd: Dma,      // TFD ring (iwl_tfh_tfd * IWL_MGMT_QUEUE_SIZE)`

```
// TFD ring (iwl_tfh_tfd * IWL_MGMT_QUEUE_SIZE)
```

## L648 · `mgmt_first_tb: Dma, // first-TB staging buffers`

```
// first-TB staging buffers
```

## L649 · `mgmt_payload: Dma,  // per-slot TB1 payload staging (no shared-buffer clobber)`

```
// per-slot TB1 payload staging (no shared-buffer clobber)
```

## L650 · `mgmt_bc_tbl: Dma,   // byte-count table (FW DMA scheduling)`

```
// byte-count table (FW DMA scheduling)
```

## L651 · `mgmt_queue_id: u16, // queue id returned by the firmware`

```
// queue id returned by the firmware
```

## L653 · `data_tfd: Dma,`

```
// gen2 data TX queue (tid 0) for EAPOL + IP frames.
```

## L656 · `data_payload: Dma,  // per-slot TB1 payload staging (one region per TFD slot)`

```
// per-slot TB1 payload staging (one region per TFD slot)
```

## L660-666 · `data_read_ptr: u32,`

```
/// The firmware's read pointer for the data queue, derived from the TFD
/// index every TX response carries in its header sequence
/// (`SEQ_TO_INDEX`, cmdhdr.h:20) — the same source `iwl_pcie_reclaim`
/// uses. `data_in_flight` used to be a COUNTER: incremented per submit,
/// decremented per completion, and therefore permanently wrong the moment
/// one completion went missing. Derived from the two pointers it is
/// self-correcting: the very next completion snaps it back to the truth.
```

## L668-674 · `key_slot_used: [bool; STA_KEY_MAX_NUM as usize],`

```
// Frames handed to the data queue but not yet reported complete by the FW
// (TX_CMD response). Flow control: never enqueue past the queue depth, or we
// overwrite a TFD the firmware is still transmitting → corruption/stall.
/// Firmware key-table bookkeeping (iwl_mvm_set_fw_key_idx). `freed`
/// counts how long each slot has been free; the previous group slot is
/// held until the one after it arrives, so a rekey never invalidates the
/// key the AP may still be transmitting with.
```

## L678-682 · `ptk_installed: bool,`

```
/// Is the pairwise key installed? It decides whether an EAPOL frame goes
/// out in the clear. mac80211 (`ieee80211_tx_h_select_key`) picks the
/// station's PTK for EVERY frame with a payload — an EAPOL-Key frame is
/// one — and only the pre-key control port carries
/// `IEEE80211_TX_INTFL_DONT_ENCRYPT`.
```

## L685-694 · `aql_pending_us: u32,`

```
/// `aql_tx_pending` — estimated airtime, in microseconds, of the frames
/// sitting in the slots `data_in_flight` covers. Kept the way the frame
/// count is: added on submit, re-derived once per pass from the firmware's
/// read pointer, so it inherits the self-correction instead of becoming a
/// second, drifting truth.
///
/// Linux subtracts on completion the SAME estimate it added on submit
/// (`ieee80211_info_get_tx_time_est`, status.c:1158) rather than the real
/// airtime the hardware reports. The accounting has to balance; being
/// right about the past is what the airtime STATISTICS are for.
```

## L696-697 · `slot_airtime_us: [u16; IWL_DATA_QUEUE_SIZE],`

```
/// Per-slot estimate, so the re-derivation can walk back over it. This is
/// the `tx_time_est` Linux stashes in each skb's control block.
```

## L699-700 · `last_tx_done_ms: u64,`

```
/// `now_ms` of the last TX completion, or of the last moment the queue was
/// empty. The queue watchdog measures from here (iwl_txq_stuck_timer).
```

## L702-705 · `tx_seq: u16,`

```
// 802.11 sequence number for non-QoS data frames. mac80211 assigns this per
// frame (ieee80211_tx_h_sequence); the gen2 firmware does NOT do it for us,
// so every data frame must carry a unique, incrementing seq or the AP treats
// distinct frames as duplicates (dropping TCP data, duplicating ACKed ones).
```

## L707 · `ba_pending: Option<BaPending>,`

```
// An ADDBA request waiting for the firmware's verdict (see ba_request).
```

## L709-710 · `link_published: bool,`

```
/// Link state as the kernel currently sees it. The truth is `authorized`;
/// this is only what we last told netdev, so a missed edge can be noticed.
```

## L712-718 · `tx_fail_streak: u32,`

```
/// Consecutive unacknowledged transmissions, and when the firmware last
/// answered a transmission at all. An associated link that stops being
/// acknowledged is dead, and nothing else notices: a deauth never comes, the
/// missed-beacon notification has never once fired on this hardware, and the
/// 4-way watchdog only guards the time BEFORE authorization. So the link
/// stays "up" and every packet vanishes — measured: ping 100 % loss with
/// state UP, and it never recovered on its own.
```

## L723-729 · `want_ht40: bool,`

```
/// 40 MHz. OFF by default — this is the one that took the link down on
/// 2026-08-19, bisected over a whole evening: association succeeds and
/// then not one frame reaches us again (`to-us 0`), on both bands, with a
/// `pre-assoc beacon ok` right before. The port itself stays in place; the
/// defect is somewhere in what we hand the firmware at the width change,
/// and a switch beats deleting the code. Turn it on for a measurement, and
/// `wlan unset ht40` is one command away when it goes dark again.
```

## L732-736 · `want_bawin: u16,`

```
/// Upper bound we put on the negotiated reorder window, from
/// `sys/config/wifi bawin`. 0 = no bound of ours, take what the AP asks
/// for. Exists because two device runs said 64 was SLOWER than 32 in both
/// widths, and comparing across sessions — different rssi, different rate
/// scaling, different loop rate — cannot settle that.
```

## L739-745 · `ba_fw_broken: bool,`

```
/// Set once the firmware has ignored a block-ack setup. Asking again costs
/// another silent 300 ms AND wedges the transmit path: measured on the
/// device, 31 frames sent / 7 acknowledged / 500 refused, and no ping.
/// Linux would not be sending this command at all on a firmware that
/// advertises BAID_ML_SUPPORT (it uses RX_BAID_ALLOCATION_CONFIG_CMD via
/// iwl_mvm_fw_baid_op, sta.c:2860) — until we implement that, one refusal
/// is all the evidence needed to stop asking.
```

## L747-750 · `st: Stats,`

```
// Diagnostics (see Stats) + what the scan found besides the chosen AP: the
// strongest same-SSID AP on the OTHER band. Picking purely by RSSI always
// lands on the near 2.4 GHz node, so the question "was there a 5 GHz one?"
// has to survive the scan to be answerable later.
```

## L759-763 · `want_ssid: [u8; SSID_MAX],`

```
// Connect policy, read once from npkFS at start-up (same place wifid takes
// its credential from). Picking the loudest AP of any network is wrong on
// two counts: on a dual-band mesh it is always the near 2.4 GHz node, and if
// a neighbour's network is louder we associate to a BSS whose PSK wifid does
// not have — a silent 4-way MIC failure.
```

## L766 · `band_pref: u8, // BAND_PREF_*`

```
// BAND_PREF_*
```

## L771-774 · `fw_assert: u32,`

```
/// 0 = unchecked, 1 = clean, else the LMAC error id. Sampled ONCE after
/// bring-up: reading it needs grab_nic_access + PRPH reads, and doing that
/// once a second from a status report pokes registers underneath a running
/// firmware. Diagnostics must not be able to break what they measure.
```

## L778 · `pick_reason: u8, // PICK_* — why the target was chosen, for the report`

```
// PICK_* — why the target was chosen, for the report
```

## L782 · `fn r32(&self, reg: u32) -> u32 { host::mmio_r32(self.mmio, reg) }`

```
// ── CSR direct register access (BAR0) ────────────────────────
```

## L785 · `fn set_bit(&self, reg: u32, bits: u32) { host::mmio_set32(self.mmio, reg, bits); }`

```
/// iwl_set_bit: RMW preserving existing bits.
```

## L788-789 · `fn prph_read(&self, reg: u32) -> u32 {`

```
// ── PRPH access through HBUS (iwl_trans_pcie_read/write_prph) ─
// umac_prph_offset is 0 for AX200, so umac == regular prph.
```

## L799-801 · `fn poll_bit(&self, reg: u32, mask: u32, timeout_us: u32) -> bool {`

```
/// iwl_poll_bit: spin until (reg & mask) == mask or timeout (microseconds).
/// IWL_POLL_INTERVAL is 10us in Linux; we lack a us timer, so we pace with
/// a short busy-spin (small timeouts) or a yielding 1ms sleep (large ones).
```

## L817 · `fn set_hw_ready(&self) -> bool {`

```
// ── iwl_pcie_set_hw_ready / prepare_card_hw (trans.c) ────────
```

## L831 · `fn prepare_card_hw(&self) -> bool {`

```
/// Returns true once the card is ready (owned by us, not AMT/ME).
```

## L837 · `host::sleep_ms(2); // usleep_range(1000, 2000)`

```
// usleep_range(1000, 2000)
```

## L840 · `self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_WAKE_ME);`

```
// Prepare conditions to check again: wake the management bus.
```

## L842 · `let mut t = 0u32;`

```
// Inner do-while: up to 150ms (Linux uses 200us steps; we use 1ms).
```

## L848 · `host::sleep_ms(1);`

```
// No iwl_mei (CSME) in our world — that branch is skipped.
```

## L860-861 · `fn sw_reset(&self, retake_ownership: bool) -> bool {`

```
// ── iwl_trans_pcie_sw_reset (trans.c) ───────────────────────
// AX200 family 22000 < BZ → CSR_RESET path, usleep_range(5000, 6000).
```

## L872-874 · `fn clear_persistence_bit(&self) -> bool {`

```
// ── iwl_trans_pcie_clear_persistence_bit (trans.c) ──────────
// Family 22000 → wprot = PREG_PRPH_WPROT_22000. Returns false only on
// the unrecoverable -EPERM path.
```

## L888-890 · `fn apm_config(&mut self) {`

```
// ── iwl_pcie_apm_config (trans.c) ───────────────────────────
// L0s is unstable on these devices: always set L0S_DISABLED. Then cache
// ASPM / LTR capability for later (set_pwr / set_ltr in Stage 2).
```

## L897 · `let cap2 = pci_read16(cap + PCI_EXP_DEVCTL2);`

```
// pm_support = !(lctl & ASPM_L0S) — not needed until power mgmt.
```

## L905-907 · `fn activate_nic(&self) -> bool {`

```
// ── iwl_pcie_gen1_2_activate_nic (trans.c) ──────────────────
// bisr_workaround = 1 for AX200: mdelay(2) before, udelay(200) after.
// Family < BZ → INIT_DONE, poll MAC_CLOCK_READY.
```

## L909 · `host::sleep_ms(2); // bisr_workaround: TOP FSM settle`

```
// bisr_workaround: TOP FSM settle
```

## L922 · `for _ in 0..1024 { core::hint::spin_loop(); } // bisr_workaround: udelay(200)`

```
// bisr_workaround: udelay(200)
```

## L926-928 · `fn apm_init(&mut self) -> bool {`

```
// ── iwl_pcie_apm_init (trans.c, gen1 — the start_hw path) ────
// Family 22000: DIS_L0S_EXIT_TIMER (family < 8000) skipped; no pll_cfg;
// no host_interrupt_operation_mode.
```

## L930 · `self.set_bit(CSR_GIO_CHICKEN_BITS, CSR_GIO_CHICKEN_BITS_REG_BIT_L1A_NO_L0S_RX);`

```
// Disable L0s without affecting L1 (ICH bug W/A).
```

## L932 · `self.set_bit(CSR_DBG_HPET_MEM_REG, CSR_DBG_HPET_MEM_REG_VAL);`

```
// FH wait threshold to maximum (HW error during stress W/A).
```

## L934 · `self.set_bit(CSR_HW_IF_CONFIG_REG, CSR_HW_IF_CONFIG_REG_HAP_WAKE);`

```
// HAP INTA: wake the PCIe link L1a -> L0s.
```

## L938 · `self.activate_nic()`

```
// pll_cfg: AX200 base has none → no CSR_ANA_PLL_CFG.
```

## L942 · `fn start_hw(&mut self) -> bool {`

```
// ── _iwl_trans_pcie_start_hw (trans.c) ──────────────────────
```

## L960-961 · `if !self.apm_init() {`

```
// force_power_gating: family==22000 && integrated. AX200 is a discrete
// M.2 card (not integrated) → skipped.
```

## L970-972 · `fn gen2_apm_init(&mut self) -> bool {`

```
// ── iwl_pcie_gen2_apm_init (trans-gen2.c) ───────────────────
// Same register effect as apm_init for family 22000 (gen1's DIS_L0S /
// pll branches are already conditioned out). nic_init re-runs it.
```

## L979 · `}`

```
// STATUS_DEVICE_ENABLED is host-side bookkeeping, not a register.
```

## L982-984 · `fn alloc_dma(&self, bytes: usize, name: &str) -> Dma {`

```
/// Allocate a coherent DMA buffer of at least `bytes`. npk_dma_alloc
/// zeroes the pages and guarantees contiguous + below 4 GB, matching
/// Linux' dma_alloc_coherent + the "no 4 GB boundary cross" requirement.
```

## L989-992 · `host::print("[ax200] FATAL: DMA alloc failed for ");`

```
// LOUD. A failed DMA allocation leaves a NONE handle that every
// later read and write silently ignores — the card simply never
// works, with no message anyone sees. `dprint` was the wrong
// channel for the one failure that makes the driver useless.
```

## L1001-1004 · `fn gen2_rx_init(&mut self) -> bool {`

```
// ── iwl_pcie_gen2_rx_init (rx.c) ────────────────────────────
// gen2 does NOT configure the RFH (firmware does it at alive) and the RB
// page pool is filled at restock (alive). So here: set the int-coalescing
// timer and allocate the ctxt_info-referenced RX rings. num_rxqs = 1.
```

## L1015-1019 · `fn txq_gen2_init(&mut self) -> bool {`

```
// ── iwl_txq_gen2_init (tx-gen2.c) — command queue ───────────
// iwl_pcie_txq_alloc allocates the TFD ring + first-TB staging buffers.
// dma_alloc zeroing leaves every TFD with num_tbs=0 (set-invalid-gen2).
// The byte-count table is not in the gen2 cmd-queue path and is not
// referenced by ctxt_info, so it is not allocated here.
```

## L1021 · `let slots = IWL_CMD_QUEUE_SIZE; // max(IWL_CMD_QUEUE_SIZE, min_txq_size=0)`

```
// max(IWL_CMD_QUEUE_SIZE, min_txq_size=0)
```

## L1024-1026 · `self.cmd_data = self.alloc_dma(CMD_DATA_BYTES, "cmd.data");`

```
// Payload buffer for large host commands: their bulk is mapped as a
// second TB (NOCOPY) instead of being copied into the cmd buffer. One
// page covers the largest command we build (SCAN_REQ_UMAC ~1.7 KB).
```

## L1031 · `fn nic_init(&mut self) -> bool {`

```
// ── iwl_pcie_gen2_nic_init (trans-gen2.c) ───────────────────
```

## L1037-1039 · `if !self.gen2_rx_init() {`

```
// iwl_op_mode_nic_config (mvm): DEFERRED. It is the op-mode/NVM layer
// (radio-stepping CSR bits), not the PCIe transport, and is not needed
// for the firmware CPU to reach ALIVE. Lands with the mvm port.
```

## L1048 · `self.set_bit(CSR_MAC_SHADOW_REG_CTRL, CSR_MAC_SHADOW_REG_CTRL_VAL);`

```
// enable shadow regs in HW
```

## L1053-1055 · `fn prepare_for_fw_load(&self) {`

```
// ── start_fw prologue (trans-gen2.c, the bits before nic_init) ──
// disable interrupts, check RF-kill, clear the RF-kill handshake so the
// firmware doesn't think the radio is killed, then clear pending ints.
```

## L1057 · `self.w32(CSR_INT_MASK, 0);`

```
// iwl_disable_interrupts
```

## L1062 · `if self.rf_killed() {`

```
// iwl_pcie_check_hw_rf_kill: bit clear == radio killed.
```

## L1064-1067 · `host::print("[ax200] HW RF-KILL asserted — the radio is off (switch/Fn key/BIOS). No frame can arrive until it is cleare`

```
// A killed radio means the firmware boots, answers commands and
// keeps every receive buffer — and not one frame ever arrives. That
// is indistinguishable from a driver bug unless someone says it, so
// it goes over `print`.
```

## L1071 · `self.w32(CSR_UCODE_DRV_GP1_CLR, CSR_UCODE_SW_BIT_RFKILL);`

```
// make sure rfkill handshake bits are cleared
```

## L1077 · `fn set_ltr(&self) {`

```
// ── iwl_pcie_set_ltr (trans-gen2.c, 22000 non-integrated) ───
```

## L1088-1092 · `fn init_fw_sec(`

```
// ── iwl_pcie_init_fw_sec (ctxt-info.c) ──────────────────────
// Walk the SEC_RT TLVs in order; the CPU1_CPU2 / PAGING separators split
// them into lmac / umac / paging. DMA each section's data and record its
// physical address into the matching ctxt_dram image array. The firmware
// reads these chunks itself; init_fw_sec ignores the per-section offset.
```

## L1100 · `let mut region = 0u8; // 0 = lmac, 1 = umac, 2 = paging`

```
// 0 = lmac, 1 = umac, 2 = paging
```

## L1120 · `return (lc, uc, vc); // caller checks counts`

```
// caller checks counts
```

## L1136 · `fn load_firmware(&mut self) -> bool {`

```
// ── iwl_pcie_ctxt_info_init + the start_fw tail → ALIVE ─────
```

## L1140 · `let mut lmac = [0u64; IWL_MAX_DRAM_ENTRY];`

```
// init_fw_sec: DMA the firmware sections.
```

## L1157 · `let mut ci = [0u8; CTXT_INFO_SIZE];`

```
// Build the context-info structure (zeroed buffer + filled fields).
```

## L1163 · `let cb_size = (NUM_RBDS as u32).trailing_zeros(); // RX_QUEUE_CB_SIZE = ilog2`

```
// RX_QUEUE_CB_SIZE = ilog2
```

## L1185-1186 · `self.w32(CSR_INT_MASK, CSR_INT_BIT_ALIVE | CSR_INT_BIT_FH_RX);`

```
// iwl_enable_fw_load_int_ctx_info (non-MSI-X): the early ALIVE comes as
// CSR_INT_BIT_ALIVE; FH_RX is for the later (Stage 3) ALIVE notification.
```

## L1189 · `host::mmio_w64(self.mmio, CSR_CTXT_INFO_BA, ci_dma.phys);`

```
// kick FW self-load: write the ctxt_info physical address.
```

## L1194 · `self.prph_write(UREG_CPU_INIT_RUN, 1);`

```
// tell the FW CPU to run (family < AX210 → regular PRPH).
```

## L1197 · `host::dprint("[ax200] FW kicked, waiting for ALIVE...\n");`

```
// Poll CSR_INT for the early ALIVE interrupt (no MSI-X, no notif_wait).
```

## L1209-1214 · `fn rx_restock_and_alive(&mut self) -> Option<Dma> {`

```
// ── iwl_pcie_rxmq_restock + read the ALIVE notification ─────
// After the early ALIVE interrupt the firmware has configured the RFH, so
// we may restock: allocate the RB pool, write each buffer into the free-RBD
// ring (bd[i] = page_dma | vid, gen2 < AX210), then bump the HW write
// pointer. The firmware then DMAs UCODE_ALIVE_NTFY into the first RB and
// advances rb_stts.closed_rb_num, which we poll.
```

## L1223-1225 · `self.rb_in_fw[i] = true;`

```
// Posted straight into slots 0..RX_NUM_RBS-1, so the firmware owns
// every one of them from here on (recycle_rb is bypassed, hence the
// explicit flag).
```

## L1227 · `let entry = rb.phys | (i as u64 + 1);`

```
// vid = i + 1; page is 4K-aligned so the low bits hold the vid.
```

## L1234 · `self.free_bd_write = RX_NUM_RBS as u32; // RBs posted at slots 0..N-1`

```
// iwl_pcie_rxq_inc_wr_ptr: write_actual = round_down(write, 8).
```

## L1235 · `self.free_bd_write = RX_NUM_RBS as u32; // RBs posted at slots 0..N-1`

```
// RBs posted at slots 0..N-1
```

## L1257-1258 · `let vid = host::dma_r32(self.rxq_used_bd.handle, 0) & RX_VID_MASK;`

```
// The FW reports each filled RB in the used-BD ring (vid). Read used_bd[0]
// to find which RB holds the first frame (iwl_pcie_get_rxb, < AX210 path).
```

## L1267 · `self.rxq_read = 1; // consumed used_bd[0]`

```
// consumed used_bd[0]
```

## L1269 · `let mut hdr = [0u8; 8];`

```
// Dump the first RB header (iwl_rx_packet: len_n_flags, cmd, group_id).
```

## L1286-1292 · `fn parse_alive_ntf(&mut self, rb0: &Dma) -> bool {`

```
// ── iwl_alive_fn (mvm/fw.c, version >= 6 path) ──────────────
// Parse the `struct iwl_alive_ntf_v6` the firmware DMA'd into RB[0]'s
// payload (data[] begins at RX_PKT_DATA_OFF). Confirms the firmware
// reported OK status and surfaces the LMAC/UMAC version + error-table
// pointers + sku_id. The sku_id gates the next stage: an all-zero
// sku_id means PNVM load is skipped entirely (iwl_pnvm_load). Linux's
// IMR / debug active-region bookkeeping is debug-only and is deferred.
```

## L1294 · `let mut p = [0u8; 160]; // len_n_flags(4) + hdr(4) + v6(144) = 152`

```
// len_n_flags(4) + hdr(4) + v6(144) = 152
```

## L1296 · `let s = RX_PKT_DATA_OFF; // alive struct base`

```
// alive struct base
```

## L1303 · `let l = AL_OFF_LMAC0; // lmac_data[0]`

```
// lmac_data[0]
```

## L1334 · `self.lmac_err_ptr = rd32(l + LMAC_OFF_ERR_TABLE);`

```
// Stash the error-table SRAM pointers for later error-log dumps.
```

## L1361-1371 · `fn send_hcmd(&mut self, group: u8, opcode: u8, payload: &[u8]) {`

```
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
```

## L1373-1379 · `let group = if group == 0 { IWL_ALWAYS_LONG_GROUP } else { group };`

```
// iwl_trans_send_cmd (iwl-trans.c): with wide_cmd_header (always true on
// gen2, where every command carries the long header), a legacy command
// with group 0 is promoted to LONG_GROUP via DEF_ID(opcode) = (1<<8) |
// opcode. The firmware registers these "legacy" commands (TX_ANT 0x98,
// BT 0x9b, POWER 0x77, MCC 0xc8, MAC_CONTEXT 0x28, …) ONLY under group 1
// — sending them as group 0 yields a BAD_COMMAND assert. (REPLY_ERROR is
// the lone exception in Linux; we never send it.)
```

## L1385-1386 · `let head = payload.len().min(FIRST_TB_HEAD_MAX);`

```
// first-TB staging: wide header (8 B) + up to FIRST_TB_HEAD_MAX (12)
// payload bytes, capped at IWL_FIRST_TB_SIZE (20).
```

## L1395 · `ftb[CMD_HDR_WIDE_LEN..tb0_len].copy_from_slice(&payload[..head]);`

```
// reserved (6) + version (7) stay 0.
```

## L1401-1403 · `let mut tfd = [0u8; TFH_TFD_SIZE];`

```
// Build the TFD: TB0 = staging buffer; TB1 (if any) = the remaining
// payload mapped from cmd_data. iwl_tfh_tfd: num_tbs @0, then 10-byte
// TBs {tb_len __le16, addr __le64}.
```

## L1407 · `host::dma_write_buf(self.cmd_data.handle, 0, payload);`

```
// remaining payload → cmd_data, mapped as TB1 (NOCOPY semantics).
```

## L1420-1421 · `self.cmd_write_ptr = (wp + 1) & (MAX_TFD_QUEUE_SIZE - 1);`

```
// iwl_txq_inc_wrap then iwl_txq_inc_wr_ptr: bump write_ptr (wrap at 256)
// and ring the doorbell with the new write_ptr | (queue_id << 16).
```

## L1432-1436 · `fn drain_rx_until(&mut self, want_cmd: u8, want_group: u8) -> Option<Dma> {`

```
// Drain newly-closed RBs from the used-BD ring, looking for a frame with the
// given (cmd, group). Returns the matching RB on success. Mirrors the read-
// pointer walk of iwl_pcie_rx_handle (mq path): r = closed_rb_num, walk
// used_bd[read..r], vid → rb_pool[vid-1]. No RB recycling — 64 posted RBs
// are plenty for the handful of init/NVM frames.
```

## L1457-1462 · `self.recycle_rb(vid);`

```
// Recycle non-matched RBs (notifications, echoes — the bulk)
// back into the free-BD ring. The driver is now resident, so
// the 64-RB pool must be replenished or the firmware runs dry
// and can post no further frames (TX completions, beacons).
// The matched RB is returned to the caller to read, so it is
// NOT recycled here (that would race the firmware writing it).
```

## L1475 · `fn wait_rx(&mut self, want_cmd: u8, want_group: u8, ms: u32) -> Option<Dma> {`

```
// Poll the RX queue for up to `ms` milliseconds for a (cmd, group) frame.
```

## L1486-1491 · `fn run_init_handshake(&mut self) -> bool {`

```
// ── iwl_run_unified_mvm_ucode post-alive init flow (mvm/fw.c) ──
// For unified ucode (AX200) the flow after ALIVE is: INIT_EXTENDED_CFG_CMD
// (declares we will send NVM access) → NVM_ACCESS_COMPLETE → wait for
// INIT_COMPLETE_NOTIF. PNVM load is skipped (sku_id empty), the external
// NVM file path is skipped (internal NVM), and iwl_send_phy_cfg_cmd is a
// no-op for unified ucode. So exactly two host commands, then the notif.
```

## L1493 · `self.send_hcmd(SYSTEM_GROUP, INIT_EXTENDED_CFG_CMD, &IWL_INIT_NVM_FLAG.to_le_bytes());`

```
// INIT_EXTENDED_CFG_CMD { __le32 init_flags = BIT(IWL_INIT_NVM) }
```

## L1495 · `self.send_hcmd(REGULATORY_AND_NVM_GROUP, NVM_ACCESS_COMPLETE, &0u32.to_le_bytes());`

```
// NVM_ACCESS_COMPLETE { __le32 reserved = 0 }
```

## L1499 · `if self.wait_rx(INIT_COMPLETE_NOTIF, 0, 2000).is_some() {`

```
// INIT_COMPLETE_NOTIF is a legacy-group (0) notification.
```

## L1507-1512 · `fn read_nvm(&mut self) -> bool {`

```
// ── iwl_get_nvm (iwl-nvm-parse.c) — read NVM info ──────────────
// Send NVM_GET_INFO and parse the response: nvm version, reserved-MAC count,
// MAC SKU caps (bands / 11n / 11ac / 11ax), PHY tx/rx antenna chains, LAR.
// The MAC address is NOT in this response — it is read from the CSR strap/OTP
// registers (iwl_set_hw_address_from_csr). The channel profile in the
// response feeds the scan channel list (Stage 4d).
```

## L1524-1525 · `let mut p = [0u8; 256];`

```
// Cover the header fields plus the regulatory channel_profile
// (__le32[51] at payload offset 28 → absolute 8+28 = 36, ending at 240).
```

## L1529 · `let payload_len = (lnf & FH_FRAME_SIZE_MASK).wrapping_sub(4); // frame - hdr(4)`

```
// frame - hdr(4)
```

## L1561-1565 · `let band_52 = mac_sku & NVM_SKU_BAND_52 != 0;`

```
// ── Build the scan channel list from the regulatory section ──
// iwl_init_channel_map (iwl-nvm-parse.c): walk iwl_ext_nvm_channels,
// read the per-channel __le32 flags from channel_profile, keep the
// NVM_CHANNEL_VALID channels. 5 GHz channels are gated on the 5.2 band
// SKU bit. Each channel's PHY band rides in v2.band in the scan command.
```

## L1596-1600 · `fn pump_rx(&mut self, ms: u32) {`

```
// Drain (and log) whatever the firmware has closed in the RX ring over the
// next `ms` milliseconds, without matching anything. Used after fire-and-
// forget commands that produce no response of their own but may surface
// unrelated notifications. An impossible (cmd, group) makes drain_rx_until
// log+advance every closed RB and return None.
```

## L1608-1625 · `fn run_scan_prereqs(&mut self) {`

```
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
```

## L1627 · `self.send_hcmd(0, TX_ANT_CONFIGURATION_CMD, &ANT_AB.to_le_bytes());`

```
// TX_ANT_CONFIGURATION_CMD (legacy group 0): valid tx antennas.
```

## L1631 · `self.send_bt_init();      // iwl_mvm_send_bt_init_conf`

```
// iwl_mvm_send_bt_init_conf
```

## L1632 · `self.send_soc_latency();  // iwl_set_soc_latency (SOC_LATENCY_SUPPORT cap)`

```
// iwl_set_soc_latency (SOC_LATENCY_SUPPORT cap)
```

## L1633 · `self.send_dqa();          // iwl_mvm_send_dqa_cmd (DQA_SUPPORT cap)`

```
// iwl_mvm_send_dqa_cmd (DQA_SUPPORT cap)
```

## L1634 · `self.send_power();        // iwl_mvm_power_update_device`

```
// iwl_mvm_power_update_device
```

## L1636-1637 · `self.set_regulatory();`

```
// iwl_mvm_init_mcc: set the regulatory domain. With LAR enabled the FW
// blocks scans until this is done (iwl_mvm_up does it before config_scan).
```

## L1640-1641 · `let mut cfg = [0u8; SCAN_CFG_LEN];`

```
// iwl_mvm_config_scan → SCAN_CFG_CMD v5 (LONG_GROUP): reduced config —
// tx/rx antenna chains. bcast_sta_id stays 0 (v5 firmware ignores it).
```

## L1651-1654 · `fn send_bt_init(&mut self) {`

```
// ── iwl_mvm_send_bt_init_conf (mvm/coex.c) ────────────────────
// BT coex config (combo chip shares the antenna). mode = network coex;
// enabled_modules = SYNC2SCO (IWL_MVM_BT_COEX_SYNC2SCO=1, always) | MPLUT
// (only if the BT_MPLUT_SUPPORT capability is present) | HIGH_BAND_RET.
```

## L1656-1663 · `let on = self.want_bt_coex;`

```
// BT_COEX_DISABLE unless `btcoex:` in sys/config/wifi says otherwise — the
// same escape hatch Linux exposes as iwlwifi.bt_coex_active=0.
//
// The AX200 is a combo chip: WiFi hangs off PCIe, its Bluetooth off USB
// (8086:2723 + 8087:0029), and the two share the antenna through this
// coexistence logic. We implement no Bluetooth at all, so nothing here
// ever tells coex that BT is idle — and arbitrating an antenna on behalf
// of a radio that was never brought up can only cost airtime.
```

## L1682-1685 · `fn send_soc_latency(&mut self) {`

```
// ── iwl_set_soc_latency (fw/init.c) ───────────────────────────
// SOC config. AX200 is a discrete card (mac_cfg.integrated unset) → flags =
// DISCRETE, latency = xtal_latency (0). Sent only if the firmware advertises
// SOC_LATENCY_SUPPORT (the gate in iwl_mvm_up).
```

## L1692 · `self.send_hcmd(SYSTEM_GROUP, SOC_CONFIGURATION_CMD, &cmd);`

```
// latency @ 4 = 0 (AX200 mac_cfg.xtal_latency)
```

## L1697-1699 · `fn send_dqa(&mut self) {`

```
// ── iwl_mvm_send_dqa_cmd (mvm/fw.c) ───────────────────────────
// Enable dynamic queue allocation. cmd_queue = IWL_MVM_DQA_CMD_QUEUE (0).
// Sent only if the firmware advertises DQA_SUPPORT (the gate in iwl_mvm_up).
```

## L1710-1715 · `fn send_power(&mut self) {`

```
// ── iwl_mvm_power_update_device (mvm/power.c) ─────────────────
// Device power table. Default power scheme (BPS): POWER_SAVE_ENA set, like
// Linux's default. CAM (flags = 0) was tried in 0.37 to kill the latency
// sawtooth but regressed connectivity (radio always-on → broadcast flood
// pins the driver core → freeze), so it is reverted. The latency spikes are
// most likely fiber-starvation, not power-save — the WiFi-IRQ is the real fix.
```

## L1717-1726 · `let ps = self.want_power_save;`

```
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
```

## L1737-1742 · `fn set_regulatory(&mut self) {`

```
// ── iwl_mvm_init_mcc → iwl_mvm_update_mcc (mvm/nvm.c) ──────────
// Set the firmware regulatory domain. With LAR enabled the firmware refuses
// to scan until the regdomain is set ("Disallow scans that might crash the
// FW while the LAR regdomain is not set"). The first update queries the FW's
// own default — alpha2 "ZZ", source GET_CURRENT — and the FW replies with
// its chosen MCC + channel profile (CMD_WANT_SKB), after which scans run.
```

## L1750-1751 · `match self.wait_rx(MCC_UPDATE_CMD, IWL_ALWAYS_LONG_GROUP, 2000) {`

```
// The command is promoted to LONG_GROUP (1) in send_hcmd, so its
// WANT_SKB response echoes group 1 (cf. NVM_GET_INFO echoing its group).
```

## L1772-1773 · `self.dump_fw_error_log();`

```
// No response to a CMD_WANT_SKB command is a strong sign the FW
// asserted on an earlier command — dump its error log.
```

## L1779-1792 · `fn add_mac_context(&mut self) {`

```
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
```

## L1795 · `put_u32(&mut cmd, MC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0, 0)`

```
// FW_CMD_ID_AND_COLOR(0, 0)
```

## L1798 · `put_u32(&mut cmd, MC_OFF_TSF_ID, 0); // TSF_ID_A`

```
// TSF_ID_A
```

## L1807 · `*b = 0xFF; // eth_broadcast_addr (no bssid_override, no bss_conf.bssid)`

```
// eth_broadcast_addr (no bssid_override, no bss_conf.bssid)
```

## L1812 · `put_u32(&mut cmd, MC_OFF_FILTER_FLAGS, MAC_FILTER_ACCEPT_GRP | MAC_FILTER_IN_BEACON);`

```
// protection_flags / cck_short_preamble / short_slot / qos_flags = 0.
```

## L1814 · `self.send_hcmd(0, MAC_CONTEXT_CMD_OP, &cmd);`

```
// union iwl_mac_data_sta: is_assoc = 0 and all timing fields = 0.
```

## L1821-1824 · `fn read_mac_address(&mut self) {`

```
// ── iwl_set_hw_address_from_csr / iwl_flip_hw_address ──────────
// Read the 6-byte MAC from the STRAP registers; if the result isn't a valid
// unicast address, fall back to the OTP registers. Store it for netdev
// registration and log it.
```

## L1845-1847 · `fn rf_killed(&self) -> bool {`

```
/// iwl_is_rfkill_set (pcie/gen1_2/internal.h): the bit is CLEAR when the
/// radio is killed. Linux polls this and reports it up; we never showed it
/// at all, so an off switch looked exactly like a broken receive path.
```

## L1852-1856 · `fn closed_rb(&self) -> u32 {`

```
/// iwl_get_closed_rb_stts plus the mask Linux applies right after it
/// (rx.c: `r &= (rxq->queue_size - 1)`, the 9000-A0 wrap-around W/A).
/// `RB_STTS_CLOSED_MASK` is 12 bits = 0..4095, the ring is 2048 — without
/// the second mask a closed index above the ring never equals our read
/// index and the drain loop below never terminates.
```

## L1862-1866 · `fn claim_rb(&mut self, vid: u32) -> Option<Dma> {`

```
/// iwl_pcie_get_rxb: take a returned buffer out of the firmware's hands.
/// A vid outside the pool, or one we never posted, means the firmware and
/// this ring disagree about who owns the page. Linux answers that with
/// `iwl_force_nmi()` and a firmware restart; we count it and drop the
/// entry, because handing the same page out twice is worse than losing it.
```

## L1881-1884 · `fn recycle_rb(&mut self, vid: u32) {`

```
// ── iwl_pcie_rxmq_restock — recycle one consumed RB ────────────
// Re-post the RB identified by `vid` into the free-BD ring at the next write
// slot so the firmware can fill it again. The page is the same; only its
// ring position changes (bd[slot] = page_dma | vid, gen2 < AX210).
```

## L1888-1890 · `self.st.rb_double_post = self.st.rb_double_post.wrapping_add(1);`

```
// The same page twice in the ring, and a write index advanced past
// what we can back with buffers. Linux cannot even express this —
// restock pulls from rx_free, and a posted buffer is not on it.
```

## L1902-1919 · `fn restock_all_rbs(&mut self) -> u32 {`

```
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
```

## L1934 · `fn flush_free_bd(&self) {`

```
// Push the recycled free-BD write index to the HW (round down to 8).
```

## L1937-1939 · `let idx = self.free_bd_write & (NUM_RBDS as u32 - 1);`

```
// Mask into the ring before rounding: free_bd_write counts monotonically
// (recycle_rb masks only for the slot it writes), so past NUM_RBDS this
// handed the hardware an index outside its own ring.
```

## L1944-1950 · `fn build_scan_cmd(&self, buf: &mut [u8]) {`

```
// ── iwl_mvm_scan_umac_v14_and_above (mvm/scan.c, version 15) ────
// Build a passive regular scan over the 2.4 GHz channels (1..13) and send it
// as SCAN_REQ_UMAC. Passive (n_ssids = 0 → FORCE_PASSIVE) means no probe
// request is transmitted, so probe_params stays zeroed; PASS_ALL makes the
// firmware forward every beacon to the host. All general/channel parameters
// are filled exactly as the Linux fill helpers do (dwell 10/110, adwell
// 2/8/10, budget 300, EXT_6 priority, UNASSOC timing = 0, adaptive dwell).
```

## L1952 · `put_u32(buf, SC_OFF_OOC_PRIORITY, SCAN_OOC_PRIORITY_REGULAR);`

```
// uid @ 0 = 0; ooc_priority + scan_priority = IWL_SCAN_PRIORITY_EXT_6.
```

## L1955 · `put_u16(buf, SC_OFF_GP_FLAGS, SCAN_GP_FLAGS_PASSIVE);`

```
// general_params_v11
```

## L1957-1958 · `buf[SC_OFF_GP_SCAN_START_MAC] = SCAN_VIF_MAC_ID;`

```
// scan_start_mac_or_link_id = scan_vif->id (version < 16). Names the FW
// MAC context added in add_mac_context(); 0 here, but set explicitly.
```

## L1960 · `buf[SC_OFF_GP_ACTIVE_DWELL] = IWL_SCAN_DWELL_ACTIVE; // LB`

```
// LB
```

## L1961 · `buf[SC_OFF_GP_ACTIVE_DWELL + 1] = IWL_SCAN_DWELL_ACTIVE; // HB`

```
// HB
```

## L1965 · `put_u16(buf, SC_OFF_GP_ADWELL_BUDGET, ADWELL_MAX_BUDGET_FULL);`

```
// flags2 @ 17 = 0
```

## L1967 · `put_u32(buf, SC_OFF_GP_SCAN_PRIO, SCAN_OOC_PRIORITY_REGULAR);`

```
// max_out_of_time / suspend_time = 0 (UNASSOC timing)
```

## L1969 · `buf[SC_OFF_GP_PASSIVE_DWELL] = IWL_SCAN_DWELL_PASSIVE; // LB`

```
// LB
```

## L1970 · `buf[SC_OFF_GP_PASSIVE_DWELL + 1] = IWL_SCAN_DWELL_PASSIVE; // HB`

```
// HB
```

## L1971 · `buf[SC_OFF_CP_FLAGS] = SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER;`

```
// num_of_fragments = 0
```

## L1973-1975 · `buf[SC_OFF_CP_FLAGS] = SCAN_CHAN_FLAG_ENABLE_CHAN_ORDER;`

```
// channel_params_v7 — the NVM_CHANNEL_VALID channels from read_nvm,
// both bands. Per channel, the band rides in the v2.band BYTE (@ +5);
// see the band-encoding note below.
```

## L1982-1988 · `buf[o + 4] = self.scan_chans[i]; // channel_num`

```
// iwl_mvm_umac_scan_cfg_channels_v7, version < 17 (our cmd_ver is 15):
// cfg.flags holds the directed-scan SSID bitmap (bits 0-19) — 0 for a
// passive station scan (no SSID, n_aps_flag only for P2P) — and the
// band rides in the v2.band BYTE (@ +5), NOT in flags bits 30-31.
// (The v17 path puts band in flags; doing that for v15 left band=0 =
// PHY_BAND_5/5GHz on 2.4GHz channels → BAD scan params → FW assert.)
// cfg.flags @ o stays 0 (zeroed buffer).
```

## L1989 · `buf[o + 4] = self.scan_chans[i]; // channel_num`

```
// channel_num
```

## L1990 · `buf[o + 5] = self.scan_bands[i]; // v2.band (1 = 2.4 GHz, 0 = 5 GHz)`

```
// v2.band (1 = 2.4 GHz, 0 = 5 GHz)
```

## L1991 · `buf[o + 6] = 1; // v2.iter_count`

```
// v2.iter_count
```

## L1992 · `}`

```
// v2.iter_interval @ o+7 = 0
```

## L1995 · `buf[SC_OFF_PERIODIC_SCHED0_ITER] = 1;`

```
// periodic_params: regular scan = one plan, one iteration.
```

## L1997 · `}`

```
// probe_params: zeroed (passive scan, no probe request transmitted).
```

## L2000-2006 · `fn parse_beacon(rb: &Dma, aps: &mut [Ap], n_aps: &mut usize) {`

```
// ── iwl_mvm_rx_mpdu_mq (mvm/rxmq.c) — parse a scan beacon ─────
// A REPLY_RX_MPDU_CMD RB holds: [len_n_flags 4][cmd_hdr 4][iwl_rx_mpdu_desc]
// [802.11 frame]. For family < AX210 the descriptor is IWL_RX_DESC_SIZE_V1
// (48), so the frame starts at RX_PKT_DATA_OFF + 48. We extract the BSSID
// (addr3), the SSID (IE 0), the RSSI (max of the two energy chains, negated
// to dBm), and the channel, de-duplicating by BSSID. Only beacon / probe-
// response management frames carry these, so other subtypes are skipped.
```

## L2008-2012 · `let mut buf = [0u8; 1600];`

```
// 384 bytes left 292 for the elements. A Wi-Fi 6 beacon carries RSN,
// Extended Capabilities, WMM, WPS and the mesh vendor blocks BEFORE
// VHT Operation (192) and the HE elements (255) — those fell off the
// end, and the walk below stopped on the cut without a word. 1600 is
// the buffer the data RX path already uses.
```

## L2015 · `let d = RX_PKT_DATA_OFF; // iwl_rx_mpdu_desc base`

```
// iwl_rx_mpdu_desc base
```

## L2017-2018 · `let to_dbm = |e: u8| if e != 0 { -(e as i16) } else { -128 };`

```
// RSSI: iwl_mvm_get_signal_strength — energy is a positive magnitude,
// negated to dBm; 0 means "no signal" (S8_MIN). Take the stronger chain.
```

## L2023 · `let f = d + IWL_RX_DESC_SIZE_V1; // 802.11 frame`

```
// 802.11 frame
```

## L2024 · `let fc = buf[f];`

```
// frame_control low byte: type bits 2-3 (0 = management), subtype 4-7.
```

## L2027 · `return; // not a management frame`

```
// not a management frame
```

## L2033-2035 · `let mpdu_len = u16::from_le_bytes([`

```
// The frame ends where the descriptor says (`iwl_mvm_rx_mpdu_mq`:
// len = le16(desc->mpdu_len)); past it lies the next packet in the RB.
// EVERY element walk below stops here, not at the buffer's end.
```

## L2039 · `return; // iwl_mvm_rx_mpdu_mq: "FW lied about packet len"`

```
// iwl_mvm_rx_mpdu_mq: "FW lied about packet len"
```

## L2042-2044 · `host::print("[ax200] beacon ");`

```
// OUR limit, not the firmware's — and the exception gets a line.
// This is the failure that hid VHT and HE from us, and a silent
// one is what let "the AP runs 20 MHz" stand for two days.
```

## L2055-2056 · `let bi_off = f + DOT11_HDR_LEN + 8;`

```
// Beacon interval: fixed param after the 24-byte header + 8-byte timestamp
// (__le16 TU). Needed for the connect MAC context (iwl_mac_data_sta.bi).
```

## L2059 · `let privacy = buf[f + DOT11_BEACON_CAP_OFF] & WLAN_CAP_PRIVACY_BIT != 0;`

```
// Privacy bit of the capability field → AP is encrypted (needs RSN).
```

## L2061-2062 · `let is_beacon = subtype == DOT11_STYPE_BEACON;`

```
// Timing, from a BEACON only: a probe response answers our probe and its
// device timestamp says nothing about the AP's beacon schedule.
```

## L2071-2072 · `for i in 0..*n_aps {`

```
// De-dup by BSSID; refresh RSSI if we hear a stronger beacon, and the
// timing on EVERY beacon — the freshest one is the one to associate with.
```

## L2088-2089 · `let mut ssid = [0u8; SSID_MAX];`

```
// Walk the information elements: SSID (0), TIM (5) for the DTIM period,
// HT Capability (45) for the AP's 802.11n parameters.
```

## L2108 · `WLAN_EID_TIM if len >= 2 => dtim_period = buf[body + 1],`

```
// TIM: dtim_count, dtim_period, bitmap_control, virtual bitmap.
```

## L2113 · `WLAN_EID_HT_CAPABILITY if len >= HT_CAP_IE_LEN => {`

```
// struct ieee80211_ht_cap — see HT_OFF_* in regs.rs.
```

## L2125-2127 · `WLAN_EID_HT_OPERATION if len >= 2 => {`

```
// The capability element says the AP CAN do 40 MHz; only the
// operation element says whether it currently DOES, and on
// which side the secondary channel sits.
```

## L2147-2151 · `WLAN_EID_VENDOR_SPECIFIC`

```
// WMM Parameter element — the AP's EDCA table.
// `ieee80211_sta_wmm_params` (mac80211/mlme.c): ACI in bits
// 5-6 selects the AC, AIFSN is the low nibble, the next byte
// is ECWmin in its low nibble and ECWmax in its high one, then
// a little-endian TXOP limit.
```

## L2162 · `let ac = match aci {`

```
// ACI 0=BE, 1=BK, 2=VI, 3=VO → mac80211 AC index.
```

## L2164 · `1 => 3usize, // BK`

```
// BK
```

## L2165 · `2 => 1,      // VI`

```
// VI
```

## L2166 · `3 => 0,      // VO`

```
// VO
```

## L2167 · `_ => 2,      // BE`

```
// BE
```

## L2169 · `let aifs = (buf[r] & 0x0f).max(2);`

```
// "AP has invalid WMM params (AIFSN=%d), will use 2"
```

## L2178 · `WLAN_EID_EXTENSION if len >= 1 => match buf[body] {`

```
// HE capability / operation live behind the extension ID.
```

## L2210-2211 · `fn dtim_count_of(buf: &[u8], f: usize) -> u8 {`

```
/// The live DTIM count from a frame's TIM element (0 if absent). Read on its
/// own because the de-dup path refreshes it without re-walking every element.
```

## L2223 · `fn print_aps(aps: &[Ap], n_aps: usize) {`

```
// Print the collected AP list (SSID, BSSID, RSSI, channel).
```

## L2233 · `if ap.ssid_len == 0 {`

```
// SSID (printable ASCII; hidden / empty → <hidden>).
```

## L2250 · `host::print("-");`

```
// RSSI in dBm (always negative here).
```

## L2259-2265 · `fn service_rx<F: FnMut(u8, u8, &Dma) -> bool>(&mut self, mut on_frame: F) -> u32 {`

```
// ── iwl_pcie_rx_handle — service the multi-queue RX ring ──────
// Drain every RB the firmware has closed since our last read, recycling
// each one back into the free-BD ring so the firmware never runs dry.
// `on_frame(cmd, group, &rb)` runs for each frame and returns `false` to
// stop draining early (a terminal notification). Returns the number of
// frames seen. Shared by the scan loop and the resident NIC service loop —
// the one place that walks used_bd → rb_pool → recycle.
```

## L2293-2295 · `fn run_scan(&mut self) -> bool {`

```
// Send the scan and poll the RX ring (npk_sleep yields — never input_wait)
// until SCAN_COMPLETE_UMAC arrives, parsing every beacon / probe response
// into the AP list along the way.
```

## L2308-2309 · `for _ in 0..12000 {`

```
// ~12 s budget: a passive scan of both bands (up to ~40 channels at
// 110 ms dwell) takes longer than the 2.4-GHz-only scan did.
```

## L2314 · `return false; // stop draining; the scan is done`

```
// stop draining; the scan is done
```

## L2316-2317 · `if cmd == REPLY_RX_MPDU_CMD && grp == 0 {`

```
// Beacons / probe responses arrive as REPLY_RX_MPDU_CMD
// (0xc1, LEGACY_GROUP) — parse them into the AP list.
```

## L2333 · `self.target_band =`

```
// Band by channel: 1..14 = 2.4 GHz, else 5 GHz (iwl_nvm_channels).
```

## L2343-2344 · `self.sync_tsf = aps[best].tsf;`

```
// …and the beacon timing that came with it, exactly as
// mac80211 reads it back out of the scan result.
```

## L2350-2354 · `self.n_aps = n_aps.min(255) as u8;`

```
// Record what we passed over: the strongest AP carrying the
// same SSID on the OTHER band. Choosing by RSSI alone always
// lands on the nearest 2.4 GHz node, and without this the
// question "was a 5 GHz radio even in range?" is unanswerable
// after the scan buffer is gone.
```

## L2405-2408 · `fn blacklist_target(&mut self) {`

```
/// Stop considering the current target. Used when an association goes
/// nowhere: re-scanning and picking the same unreachable AP again is not a
/// retry, it is a loop. Oldest entry is evicted, so a roaming client cannot
/// blacklist its way out of every AP it has.
```

## L2423-2425 · `fn load_connect_policy(&mut self) {`

```
// Read the connect policy from `sys/config/wifi` — one file, `key: value`
// per line. `ssid` is the network wifid holds the PSK for; associating to
// anything else can only end in a MIC failure.
```

## L2439-2443 · `self.want_ampdu = cfg_on(cfg_get(text, b"ampdu"));`

```
// OFF by default, and it stays that way until a device measurement says
// otherwise. Aggregation is the throughput lever, but it is also the one
// feature that can take the whole link down (a receiver that mis-parses
// an aggregate carries nothing) — and recovering from that needs the
// network it just broke. Twice now. So the safe state is the default.
```

## L2445-2454 · `self.want_txagg = cfg_on(cfg_get(text, b"txagg"));`

```
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
```

## L2500-2501 · `fn pick_target(&mut self, aps: &[Ap], n_aps: usize) -> usize {`

```
// Choose the connect target from the scan results. Returns usize::MAX when
// nothing qualifies.
```

## L2512 · `let mut best_24 = usize::MAX;`

```
// Strongest AP of our network per band.
```

## L2525-2527 · `if n_ours == 0 {`

```
// Nothing with the configured SSID: fall back to the strongest AP of any
// network rather than refusing to connect, but say so — a 4-way that then
// fails on the MIC is otherwise a mystery.
```

## L2550-2551 · `if best_5 != usize::MAX && aps[best_5].rssi >= BAND_PREF_5_MIN_RSSI {`

```
// Auto: take 5 GHz when it is above the floor, even if a 2.4 GHz
// AP is louder. Below the floor the extra range of 2.4 wins.
```

## L2553-2554 · `let penalty = if best_24 == usize::MAX {`

```
// Only when 2.4 GHz is not dramatically stronger. The wider
// band is worth a handicap, not an arbitrary one.
```

## L2575-2582 · `fn connect_phy_binding(&mut self) -> bool {`

```
// ── Stage 5a: PHY context + RLC + binding (connect step 1) ────
// iwl_mvm_phy_ctxt_add + iwl_mvm_phy_send_rlc + iwl_mvm_binding_add_vif
// (mvm/phy-ctxt.c, mvm/binding.c). Sets the target AP's operating channel
// (PHY_CONTEXT_CMD v4, 20 MHz), configures the RX chains (RLC_CONFIG_CMD v2 —
// not offloaded on this FW, cmd_ver=2 < 3), and binds the MAC context (id 0)
// to the PHY context (id 0) (BINDING_CONTEXT_CMD v2, full struct: CDB binding
// support, but lmac_id 0 since no CDB). PHY + RLC are fire-and-forget; BINDING
// returns a status word (CMD_WANT_SKB). All cmd_vers parsed from the FW file.
```

## L2584 · `let mut pc = [0u8; PHY_CTX_CMD_LEN];`

```
// PHY_CONTEXT_CMD v4 (action ADD) — 20 MHz on the target channel/band.
```

## L2586 · `put_u32(&mut pc, PC_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(phy 0, color 0)`

```
// FW_CMD_ID_AND_COLOR(phy 0, color 0)
```

## L2598 · `put_u32(&mut pc, PC_OFF_LMAC_ID, IWL_LMAC_24G_INDEX); // no CDB → 0`

```
// no CDB → 0
```

## L2599-2601 · `put_u32(&mut pc, PC_OFF_RXCHAIN, RLC_RX_CHAIN_INFO_2X2);`

```
// Receive chains. iwl_mvm_phy_ctxt_apply fills this field for cmd_ver 3+
// and sends RLC_CONFIG_CMD afterwards — both, not either. Left at zero
// the context declares no valid RX antenna.
```

## L2603 · `self.send_hcmd(0, PHY_CONTEXT_CMD, &pc); // legacy → LONG_GROUP`

```
// legacy → LONG_GROUP
```

## L2611 · `let mut rlc = [0u8; RLC_CMD_LEN];`

```
// RLC_CONFIG_CMD v2 (DATA_PATH_GROUP) — RX chains for the PHY context.
```

## L2619 · `let mut bc = [0u8; BINDING_CMD_LEN];`

```
// BINDING_CONTEXT_CMD v2 (action ADD): MAC ctx 0 ↔ PHY ctx 0.
```

## L2621 · `put_u32(&mut bc, BC_OFF_ID_COLOR, 0); // phy id 0 / color 0`

```
// phy id 0 / color 0
```

## L2623 · `put_u32(&mut bc, BC_OFF_MACS, 0); // macs[0] = MAC ctx id 0`

```
// macs[0] = MAC ctx id 0
```

## L2624 · `put_u32(&mut bc, BC_OFF_MACS + 4, FW_CTXT_INVALID); // macs[1]`

```
// macs[1]
```

## L2625 · `put_u32(&mut bc, BC_OFF_MACS + 8, FW_CTXT_INVALID); // macs[2]`

```
// macs[2]
```

## L2626 · `put_u32(&mut bc, BC_OFF_PHY, 0); // phy id 0`

```
// phy id 0
```

## L2628 · `self.send_hcmd(0, BINDING_CONTEXT_CMD, &bc); // legacy → LONG_GROUP`

```
// legacy → LONG_GROUP
```

## L2649-2656 · `fn connect_add_station(&mut self) -> bool {`

```
// ── Stage 5b: station (AP peer) + gen2 TX queue (connect step 2) ──
// iwl_mvm_add_sta + iwl_mvm_tvqm_enable_txq → iwl_trans_txq_alloc (mvm/sta.c,
// pcie/.../tx-gen2.c). Adds the AP as a LINK station (ADD_STA v12, sta_id 0,
// minimal flags — HT/rate flags come at assoc) and allocates a dynamic gen2
// management TX queue for it (for the auth/assoc frames): a TFD ring + first-
// TB staging + byte-count table, registered with the firmware via
// SCD_QUEUE_CONFIG_CMD v3 which returns the queue id. No frame is transmitted
// yet (that is 5c). Both commands return a status (CMD_WANT_SKB).
```

## L2658 · `let mut sc = [0u8; ADD_STA_CMD_LEN];`

```
// ADD_STA v12 (action ADD) — the AP peer station.
```

## L2660 · `sc[AS_OFF_ADD_MODIFY] = 0; // add (not modify)`

```
// add (not modify)
```

## L2662 · `put_u32(&mut sc, AS_OFF_MAC_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(mac 0, 0)`

```
// FW_CMD_ID_AND_COLOR(mac 0, 0)
```

## L2665 · `put_u32(&mut sc, AS_OFF_STATION_FLAGS, 0); // refined at assoc (5d)`

```
// refined at assoc (5d)
```

## L2668 · `self.send_hcmd(0, ADD_STA, &sc); // legacy → LONG_GROUP`

```
// legacy → LONG_GROUP
```

## L2691-2692 · `self.mgmt_tfd = self.alloc_dma(TFH_TFD_SIZE * IWL_MGMT_QUEUE_SIZE, "mgmt.tfd");`

```
// Allocate the management TX queue DMA: TFD ring + first-TB staging +
// byte-count table (16-slot queue → IWL_MGMT_QUEUE_SIZE).
```

## L2703 · `let mut q = [0u8; SCD_CMD_LEN];`

```
// SCD_QUEUE_CONFIG_CMD v3 (ADD): hand the DMA addresses to the firmware.
```

## L2706 · `put_u32(&mut q, SQ_OFF_STA_MASK, 1 << AP_STA_ID); // BIT(sta_id)`

```
// BIT(sta_id)
```

## L2743-2750 · `fn connect_finish_chanctx(&mut self) {`

```
// ── Stage 5b': finish the connect chanctx tail (iwl_mvm_assign_vif_chanctx) ──
// __iwl_mvm_assign_vif_chanctx does binding → power_update_mac → (quota, only
// for monitor) and the connect flow then re-sends the MAC context with the
// target BSSID (iwl_mvm_mac_ctxt_changed on BSS_CHANGED_BSSID). We had skipped
// this whole tail and went straight to the auth TX with a MAC context still
// holding the scan-time broadcast BSSID + zero timing — so once session
// protection put the firmware on-channel and it actually processed the auth,
// the time-event/scheduler (UMAC) asserted. Send both before the auth.
```

## L2752-2753 · `let mut pm = [0u8; MAC_POWER_CMD_LEN];`

```
// iwl_mvm_power_update_mac → MAC_PM_POWER_TABLE for the bss vif. Power-save
// disabled path: only id_and_color + keep_alive_seconds, flags = 0.
```

## L2755 · `put_u32(&mut pm, MP_OFF_ID_COLOR, 0); // FW_CMD_ID_AND_COLOR(0,0)`

```
// FW_CMD_ID_AND_COLOR(0,0)
```

## L2761-2762 · `let mut cmd = [0u8; MAC_CTX_CMD_LEN];`

```
// iwl_mvm_mac_ctxt_changed (MODIFY) with the target AP's BSSID + timing,
// unassociated branch (is_assoc = 0, MAC_FILTER_IN_BEACON).
```

## L2773-2774 · `self.fill_qos_params(&mut cmd);`

```
// Every MAC context command goes through iwl_mvm_mac_ctxt_cmd_common,
// and that always fills the QoS block — not only the associated one.
```

## L2776-2777 · `put_u32(&mut cmd, MC_OFF_STA_IS_ASSOC, 0);`

```
// iwl_mac_data_sta (unassoc): is_assoc = 0, bi = beacon interval, dtim
// unknown pre-assoc (→ 0), assoc_id = 0.
```

## L2784-2792 · `if self.sync_ok {`

```
// Grab the beacon timing HERE, not after the association: this is the
// last point where nothing else is expected on the RX ring. Doing it
// after the assoc response would mean draining (and discarding) the
// frames the AP sends next — including the first EAPOL of the 4-way.
// Auth + assoc take a few ms, so the timing is still current.
// The timing came with the scan result (see collect_ap). Waiting here for
// another beacon was a step of our own invention — mac80211 does not have
// it — and it never once succeeded, which is why the firmware kept
// getting a made-up wake schedule and never reported a missed beacon.
```

## L2804-2808 · `fn fill_qos_params(&self, cmd: &mut [u8]) {`

```
// ── Post-association: tell the firmware we are associated ─────────────
// Everything below runs once, right after the assoc response. Linux does it
// from the BSS_CHANGED_ASSOC / sta-state path; we had none of it, so the
// firmware kept a MAC context that still said "not associated" for the whole
// life of the link.
```

## L2810-2824 · `fn fill_qos_params(&self, cmd: &mut [u8]) {`

```
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
```

## L2834 · `put_u16(cmd, a + ACQ_OFF_EDCA_TXOP, txop.saturating_mul(32));`

```
// mac80211 keeps txop in 32 us units; the firmware wants us.
```

## L2842-2844 · `if self.target_ht.present {`

```
// "if chanreq.oper.width != NL80211_CHAN_WIDTH_20_NOHT" — i.e. any
// width that is not the legacy no-HT one. We are an HT station
// whenever the AP carries an HT element, 20 MHz included.
```

## L2862-2864 · `let prot = self.protection_flags();`

```
// What the BSS asks us to protect against. Sent as 0 until now, i.e.
// "nothing" — the firmware could not know that legacy or non-member
// stations share this channel.
```

## L2869-2871 · `let bi = self.target_beacon_int as u32;`

```
// iwl_mvm_set_fw_dtim_tbtt: the DTIM count counts down, so the next DTIM
// TBTT is that many beacon intervals after the beacon we just heard.
// Beacon intervals are TU (1024 us).
```

## L2892-2894 · `fn sta_assoc_update(&mut self) {`

```
// iwl_mvm_sta_send_to_fw with update=true — the station flags the peer's HT
// capabilities imply, plus the AID. On a modify Linux leaves addr zeroed and
// lets station_flags_msk select which bits to apply.
```

## L2896-2901 · `let mut flags = if self.use_vht80() {`

```
// The station's TX width has to match the PHY context. Left at 20 MHz
// it produced exactly the asymmetry measured on the device once VHT80
// came up: RX at 650 Mbit, TX at 26, heavy loss, then a dropped link —
// the receive path ran at 80 MHz while transmission was pinned to 20
// and the rate control had to reconcile the two.
// `iwl_mvm_sta_send_to_fw` sets this from the peer's bandwidth.
```

## L2909 · `let mut msk = STA_FLAGS_MSK_ADD | STA_FLG_FAT_EN_MSK;`

```
// …and the mask has to select it, or a modify leaves the old value.
```

## L2912 · `flags |= if self.target_ht.mcs_rx[1] != 0 {`

```
// rx_nss: the AP's second-stream MCS mask decides 1 vs 2 streams.
```

## L2926 · `sc[AS_OFF_ADD_MODIFY] = 1; // modify`

```
// modify
```

## L2928 · `sc[AS_OFF_MODIFY_MASK] |= STA_MODIFY_TID_DISABLE_TX;`

```
// A modify only applies the fields modify_mask selects.
```

## L2944-2945 · `fn connect_post_assoc(&mut self) {`

```
// The whole post-assoc chain, in Linux' order: mark the MAC context
// associated, update the station, then start rate scaling.
```

## L2947-2950 · `if self.target_dtim_period != 0 {`

```
// No RX draining in here: the AP starts the 4-way immediately after the
// assoc response, and anything we drain now we throw away. The beacon
// timing was captured before the auth (connect_finish_chanctx).
// Linux: "We need the dtim_period to set the MAC as associated."
```

## L2960-2967 · `fn update_phy_context(&mut self) {`

```
// ── Reconnect after a link loss (mesh steering / deauth) ──────────────
// The Fritzbox + Fritz repeater run one SSID across two APs and steer the
// client between them with a DEAUTH. We re-scan, re-point the already-added
// PHY context + station + MAC context at the best AP (may be the OTHER mesh
// node, on a different channel) via MODIFY actions — the binding (MAC0↔PHY0)
// and the TX queues persist, so NO DMA is re-allocated (the DMA budget can't
// churn per reconnect). Then redo auth + assoc and re-arm wifid for a fresh
// 4-way. Returns true once associated.
```

## L2969 · `fn update_phy_context(&mut self) {`

```
// PHY_CONTEXT_CMD v4 (action MODIFY) — re-point the PHY at the new channel.
```

## L2980 · `self.send_hcmd(0, PHY_CONTEXT_CMD, &pc); // legacy → LONG_GROUP`

```
// legacy → LONG_GROUP
```

## L2984 · `fn retarget_station(&mut self) {`

```
// ADD_STA v12 (action MODIFY) — re-point the AP-peer station at the new BSSID.
```

## L2988 · `sc[AS_OFF_ADD_MODIFY] = 1; // modify`

```
// modify
```

## L2990 · `sc[AS_OFF_MODIFY_MASK] |= STA_MODIFY_TID_DISABLE_TX;`

```
// A modify only applies the fields modify_mask selects.
```

## L3000 · `self.send_hcmd(0, ADD_STA, &sc); // legacy → LONG_GROUP`

```
// legacy → LONG_GROUP
```

## L3004-3012 · `fn tid_disable_tx(&self) -> u16 {`

```
/// 40 MHz only when ALL THREE agree: the AP says it can (capability
/// element), it says it currently does and on which side (operation
/// element), and the band has the room. Anything less stays at 20 —
/// a PHY context wider than the AP's actual channel points at silence.
/// `iwl_mvm_sta_send_to_fw`: `tid_disable_tx = mvm_sta->tid_disable_agg`,
/// which starts at 0xffff and is only ever cleared by `iwl_mvm_sta_tx_agg`
/// — unreachable on TLC-offload firmware. So 0xffff IS Linux' value here,
/// and it stays the default. `wlan set txagg on` sends 0x0000 to find out
/// whether the firmware honours the field anyway.
```

## L3024-3037 · `fn protection_flags(&self) -> u32 {`

```
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
```

## L3043-3044 · `if self.target_ht.ht_op_mode & IEEE80211_HT_OP_MODE_PROTECTION == 0 {`

```
// "for both sta and ap, ht_operation_mode hold the protection_mode",
// and Linux treats a zero operation_mode as "HT protection not in use".
```

## L3048 · `let ht_flag = MAC_PROT_FLG_HT_PROT | MAC_PROT_FLG_FAT_PROT;`

```
// The firmware does not distinguish HT from FAT, so Linux sets both.
```

## L3055 · `if self.use_ht40() { flags |= ht_flag; }`

```
// Only when we are actually wider than 20 MHz.
```

## L3063-3066 · `fn vht_oper(&self) -> (u8, u8, bool) {`

```
/// Where the VHT operation info comes from, per `ieee80211_determine_ap_chan`:
/// an AP with HE Capability whose HE Operation sets VHT_OPER_INFO carries it
/// in THOSE three bytes, and element 192 is then not consulted at all. Any
/// other AP answers from element 192. Returns (chan_width, seg0, from_he).
```

## L3083-3085 · `fn ctrl_pos(&self) -> u8 {`

```
/// `iwl_mvm_get_ctrl_pos` for the HT case. The control channel is the
/// UPPER of the pair exactly when the secondary sits BELOW it; for 40 MHz
/// the offset term of that function is zero, so only the ABOVE bit is left.
```

## L3088-3089 · `return if self.target_ht.sec_chan_offs == IEEE80211_HT_PARAM_CHA_SEC_BELOW {`

```
// 40 MHz: the offset term of iwl_mvm_get_ctrl_pos is zero, so only
// the ABOVE bit remains — set when the secondary half is below.
```

## L3096-3099 · `let offs = (self.target_chan as i32 - self.vht_oper().1 as i32) * 5;`

```
// 80 MHz: iwl_mvm_get_ctrl_pos in full. Channel numbers are 5 MHz
// apart, so the control channel sits 10 or 30 MHz from the centre.
//     ret = (abs_offs - 10) / 20   →  0 or 1
//     ret |= (offs > 0) * ABOVE
```

## L3116-3117 · `host::netdev_set_link_state(false, false);`

```
// The association is gone too — this is a real carrier loss, not a
// dormant phase.
```

## L3127 · `self.update_phy_context();`

```
// Re-point PHY + station + MAC context at the (possibly new) best AP.
```

## L3148-3149 · `self.ba_stop_all();`

```
// The firmware's sessions died with the old association; anything
// the windows still hold belongs to a link that no longer exists.
```

## L3163-3172 · `fn connect_tlc_config(&mut self) {`

```
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
```

## L3177 · `cmd[TLC_OFF_CHAINS] = ANT_AB as u8; // chain A|B = BIT(0)|BIT(1)`

```
// chain A|B = BIT(0)|BIT(1)
```

## L3178-3179 · `let non_ht = if self.target_band == PHY_BAND_24 as u8 {`

```
// non_ht_rates is filled in every mode (rs_fw_set_supp_rates sets it
// before the mode switch) — it is the fallback the firmware drops to.
```

## L3189-3190 · `put_u16(&mut cmd, TLC_OFF_HT_RATES_NSS1, self.target_ht.mcs_rx[0] as u16);`

```
// ht_rates carries the PEER's receive MCS mask — what the AP can
// take from us — per spatial stream, in the "80 MHz and below" slot.
```

## L3193-3195 · `if self.target_ht.cap_info & IEEE80211_HT_CAP_SGI_20 != 0 {`

```
// rs_fw_sgi_cw_support: one bit per channel width. And
// max_ch_width has to say 40 too, or the rate control never picks a
// 40 MHz rate no matter how the PHY is configured.
```

## L3206-3207 · `cmd[TLC_OFF_MODE] = TLC_MODE_VHT;`

```
// The rate table is VHT's, not HT's: MCS 0-9 per stream. The
// ht_rates field carries it — the firmware reads it by `mode`.
```

## L3212 · `let vht_mcs = 0x03ffu16; // MCS 0-9`

```
// MCS 0-9
```

## L3227-3228 · `host::print("[ax200] TLC mode=HT mcs=");`

```
// max_mpdu_len stays 0: that field enables TX A-MSDU, and we build
// our own frames. max_tx_op 0 = no limit.
```

## L3244-3245 · `fn log_rate(prefix: &str, raw: u32) {`

```
/// Decode a v2 rate_n_flags into the log. Always prints the raw word too —
/// the decode is our reading of the format, the raw value is the truth.
```

## L3281-3290 · `fn rate_v3(rnf: u32) -> u32 {`

```
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
```

## L3301 · `const HT: [[u32; 8]; 4] = [`

```
// HT per spatial stream, [bw20 lgi, bw20 sgi, bw40 lgi, bw40 sgi].
```

## L3308-3311 · `const VHT: [[u32; 10]; 6] = [`

```
// VHT per spatial stream, [bw20 lgi, bw20 sgi, bw40 …, bw80 …].
// Derived from the 802.11ac formula rather than copied: data
// subcarriers (52 / 108 / 234) x bits-per-subcarrier x coding rate,
// over the 4.0 us symbol (3.6 us with short GI).
```

## L3335 · `_ => 0, // HE: not negotiated, so no table for it`

```
// HE: not negotiated, so no table for it
```

## L3339 · `fn rep_rate(r: &mut Rep, label: &str, raw: u32) {`

```
// One rate line: raw word, decoded modulation, and the PHY rate it implies.
```

## L3346 · `r.s("0x");`

```
// Print the RAW firmware word, decode the normalised one.
```

## L3378-3379 · `fn publish_report(&mut self, now_ms: u64) {`

```
// Build and publish the status snapshot. Called from the resident loop once
// a second; everything it prints is already counted, so this only formats.
```

## L3381 · `let win = now_ms.saturating_sub(self.st.win_start_ms).max(1);`

```
// Throughput + airtime over the window that just closed.
```

## L3403-3404 · `let passes = self.st.prof_passes.max(1);`

```
// The profile is per WINDOW, not cumulative: an average over the whole
// uptime would drown the loaded second in idle ones.
```

## L3412-3414 · `if self.st.tput_rx_kbit > self.st.peak_tput_rx_kbit {`

```
// Keep the profile of the BUSIEST second, not just the last one. A load
// generator holds the terminal for its whole run, so by the time anyone
// can type `wlan` the live window is idle again and shows nothing.
```

## L3422-3424 · `self.st.pk_rx_airtime_pct = self.st.rx_airtime_pct;`

```
// Taken WITH the peak, not as separate maxima: three maxima from
// three different seconds would describe a second that never
// happened, and the question is what the fastest one looked like.
```

## L3439-3442 · `self.st.win_pass_empty = 0;`

```
// Reset only AFTER the snapshot above has read them. The first version
// cleared them 36 lines earlier, with the window's other state, so the
// snapshot copied zeros — and a diagnostic that reports zero reads as
// "nothing happened" rather than "the instrument is broken".
```

## L3513-3514 · `if self.use_vht80() {`

```
// The negotiated width, and WHY — a link that quietly fell back to
// 20 MHz looks identical to one that never tried.
```

## L3525-3526 · `r.s(if !self.want_vht && self.target_ht.vht {`

```
// Say WHOSE decision it was. "AP has VHT but runs HT" blamed
// the AP even when the reason was our own `vht` switch.
```

## L3536-3538 · `if !self.want_ht40 {`

```
// WHOSE decision — `ht40` is off by DEFAULT, and this branch
// printed "AP runs 20 only" for it. That sentence is what the
// handover note rests on; it was our own switch talking.
```

## L3552 · `r.s("ap qos   ");`

```
// The EDCA table we hand the firmware, so "QoS BSS" is checkable.
```

## L3575-3576 · `if self.target_ht.present {`

```
// The bytes the width decision rests on, so the line above can be
// checked instead of believed.
```

## L3600-3603 · `let live = ba::sessions().iter().filter(|s| s.active()).count();`

```
// Aggregation is the single biggest throughput lever, so it gets its own
// line. `held` standing still while `buffered` climbs is the healthy
// picture; `stalls` counts holes the AP never closed, which is the one
// failure mode a reorder buffer can add to a working link.
```

## L3607-3609 · `for sess in ba::sessions().iter().filter(|s| s.active()) {`

```
// One line PER live session: which TID carries the traffic is the
// question the single-session report could not answer — it showed
// tid 6 with `delivered 0` while every byte arrived on tid 0.
```

## L3643-3646 · `r.s("  pool ");`

```
// Storage is shared and finite now. `pool` says how close the held
// frames come to it; `POOL-FULL` says it was not enough and a frame
// went up out of order. Both are the exception, so only the second
// one shouts.
```

## L3651-3652 · `if self.want_bawin != 0 {`

```
// Whose decision the window was. Without this, `win 32` reads as
// "the AP asked for 32" when it may be our own bound.
```

## L3688-3692 · `r.s("4-way    ready-sent ");`

```
// The 4-way, step by step. A stalled association always stops at one
// specific rung, and which one names the culprit: no ready = never
// associated; ready but no eapol in = the AP stayed silent; eapol in
// but none out = wifid is not answering; keys but not authorized =
// wifid did not finish.
```

## L3703-3707 · `if self.st.gtk_installs > 1 {`

```
// The 4-way installs one GTK; every further one is a rekey. A rekey
// only ever announced itself as a log LINE, and background output is
// pinned to the first loop window — so in any other window it was
// invisible, and it was very nearly missed. A standing number cannot
// scroll past.
```

## L3733-3734 · `r.s("peak     tx ");`

```
// Survives the end of the load, so one `wlan` AFTER a blocking transfer
// still answers "how fast did it actually go".
```

## L3742-3744 · `r.s("air      in that window: rx ");`

```
// The line that decides whether the AIR was the limit in that fastest
// second. Near 100 = the channel was full and aggregation is the lever.
// Well under it = the channel was idle and the ceiling is on our side.
```

## L3754-3756 · `r.s("arrive   passes empty ");`

```
// The shape of the arrivals in that same second. Mostly-empty passes
// with a long gap = the AP is not delivering and the ceiling is not
// ours. Passes consistently carrying frames = we are the bottleneck.
```

## L3816-3820 · `r.s("tx agg   subframes ");`

```
// Is the firmware aggregating what we hand it? `frame_count` says so
// per response (1 = single MPDU), the compressed block-ack says it
// again from the other side. Without this line "no TX aggregation" was
// an assumption; `tx frames` counts what we QUEUED, not what went on
// the air as one aggregate.
```

## L3835-3836 · `r.s(" reclaims ");`

```
// Whether the aggregated TX return path RAN. Without it the slots those
// MPDUs sat in are never freed, and the queue wedges at its own depth.
```

## L3868-3876 · `let work_pp = self.st.prof_work_pp;`

```
// The poll rate, and what it implies. The loop asks for a 1 ms sleep
// while busy, but a fiber whose core has nothing else runnable idles in
// HLT until the next 100 Hz worker tick — so the REAL period can be 10 ms,
// and then one pass' worth of AQL-admitted frames is a hard ceiling.
// Printing the implied ceiling makes that visible instead of theoretical.
// The cost of one pass, in the only unit that answers "are we CPU-bound":
// microseconds. work = everything between waking and sleeping; drain =
// the RX half of it; slept = what a 1 ms request really took. If work
// approaches the wall time per pass, more air rate buys nothing.
```

## L3880-3882 · `r.s("crypto   protected ");`

```
// Encrypted RX, the one number that separates "the AP went quiet" from
// "the AP is talking and we cannot read it". mic-fail is what Linux
// drops; sec-none is a protected frame the firmware never decrypted.
```

## L3910-3911 · `let pw = self.st.peak_work_pp;`

```
// The same numbers from the busiest second — the only ones that matter
// when the load generator has the terminal.
```

## L3940-3942 · `let est = self.expected_tx_airtime(1514);`

```
// AQL in the only units that make it checkable: what one full frame is
// estimated to cost right now, and how many of them the limit therefore
// allows. Both move with the rate — that is the whole point of it.
```

## L3958-3960 · `r.s("policy   power ");`

```
// What else the scan saw. The target is picked by RSSI alone, which on a
// dual-band mesh always means the near 2.4 GHz node — this line is how we
// find out whether a faster band was on the table.
```

## L3984-3995 · `r.s("fw       ");`

```
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
```

## L4025-4026 · `if self.st.rx_wd_fires > 0 || self.st.rb_bad_vid > 0 || self.st.rb_double_post > 0 {`

```
// Only when something is off — a ring that reports its normal state
// every time is not a report.
```

## L4052-4057 · `r.s("beacons  fw notifs ");`

```
// What the firmware reports about beacons — because WE no longer see
// them. Once associated it stops passing them to the host (Linux sets
// MAC_FILTER_IN_BEACON only while unassociated, mac-ctxt.c:704), so
// silence here is normal and this notification is the only beacon news
// there is. `losses` is how often it declared the AP gone: a mesh that
// steers between router and repeater shows up exactly there.
```

## L4072-4073 · `r.s("sync     pre-assoc beacon ");`

```
// The timing the associated MAC context was built from — captured from
// the LAST pre-association beacon, which is the only one we ever see.
```

## L4104-4114 · `fn tx_raw(&self, qid: u16, wptr: u32, qsize: usize, tfd_ring: Dma, first_tb: Dma, payload: Dma, bc: Dma, flags: u32, fra`

```
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
```

## L4117 · `let mut buf = [0u8; TX_PAYLOAD_STRIDE]; // dev_cmd header + tx_cmd + full frame`

```
// dev_cmd header + tx_cmd + full frame
```

## L4119 · `buf[1] = 0; // group 0 (short header)`

```
// group 0 (short header)
```

## L4124-4128 · `let pad = if hdr_len % 4 != 0 { 2usize } else { 0 };`

```
// offload_assist (iwl_mvm_tx_csum): the 802.11 header length in 2-byte
// words for EVERY frame, plus PAD when it is not a multiple of 4 — then
// 2 bytes go between header and payload so the payload is DWORD-aligned
// (Linux does that alignment in the transport's TB1). A QoS header is 26
// bytes, so this is what makes the QoS data path work at all.
```

## L4143 · `let body = TXC_OFF_FRAME + hdr_len + pad; // pad bytes stay zero`

```
// pad bytes stay zero
```

## L4147-4150 · `let pl_off = (idx * TX_PAYLOAD_STRIDE) as u32;`

```
// Per-slot staging: TB0 = this slot's first-TB buffer (first 20 bytes),
// TB1 = this slot's payload region (the rest). Every in-flight TFD has
// its own payload region, so a later frame never overwrites an earlier
// one before the firmware has DMA'd it.
```

## L4159 · `tfd[0..2].copy_from_slice(&2u16.to_le_bytes()); // num_tbs`

```
// num_tbs
```

## L4186-4195 · `fn ba_request(&mut self, req: &[u8; 12]) {`

```
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
```

## L4211-4214 · `if tid as usize >= ba::NUM_TIDS {`

```
// ieee80211_process_addba_request, in its order.
//
// Block-ack is defined for TIDs 0-7; at IEEE80211_FIRST_TSPEC_TSID and
// above Linux declines outright.
```

## L4219-4221 · `if !immediate || asked as usize > IEEE80211_MAX_AMPDU_BUF_HT {`

```
// We only implement immediate block ack, and a buffer larger than the
// HT maximum is a malformed request — both are INVALID_QOS_PARAM, not
// a plain decline, so the AP learns WHY.
```

## L4231-4234 · `if let Some(sess) = ba::by_tid(tid) {`

```
// A repeat request on a live session. Same dialog token = the AP is
// only updating the timeout; we have no way to change it in the
// firmware, so we accept it unchanged and decline a real change,
// WITHOUT disturbing the session.
```

## L4248-4252 · `host::print("[ax200] ADDBA replaces the session on tid ");`

```
// A genuinely new session on a TID that already has one. Linux
// tears the old one down FIRST (agg-rx.c:379) and sends no
// DELBA for it — the AP is replacing it on purpose. Skipping
// this is what leaked the firmware BAID: we overwrote the slot
// and could no longer name the old id to free it.
```

## L4260-4266 · `let max_buf = if self.target_ht.he {`

```
// `ieee80211_process_addba_request` (agg-rx.c:319): the ceiling comes
// from the PEER's capability, not from a constant of ours. An HE AP may
// run 256, a plain HT one 64. `buf_size == 0` means "your maximum".
//
// Then the local hardware limit, which for this family is 256 as well
// (`hw->max_rx_aggregation_subframes`, mvm/ops.c:1233) — the same value
// `BA_WIN_MAX` is sized for.
```

## L4286-4289 · `if fw_has_capa(IWL_UCODE_TLV_CAPA_BAID_ML_SUPPORT) {`

```
// iwl_mvm_fw_baid_op (mvm/sta.c): the capability decides which of the
// two commands opens the session. Skipping this branch is what cost us
// the link — the fallback command is not ignored by a firmware that
// wants the other one, it stops completing transmissions.
```

## L4293-4294 · `put_u32(&mut c, BAID_OFF_STA_MASK, 1u32 << AP_STA_ID);`

```
// iwl_mvm_sta_fw_id_mask with link -1 on a non-MLD station is
// simply BIT(sta_id) (mvm/mld-sta.c).
```

## L4302 · `sc[AS_OFF_ADD_MODIFY] = 1; // modify`

```
// modify
```

## L4313-4318 · `fn ba_stop(&mut self, tid: u8, tell_ap: bool, reason: u16) {`

```
/// End one TID's session: drop our buffer, free the firmware BAID, and —
/// only when WE are the one ending it — tell the AP with a DELBA.
///
/// `__ieee80211_stop_rx_ba_session`: the DELBA goes out only for
/// `initiator == WLAN_BACK_RECIPIENT && tx`. A session the AP itself ended
/// (DELBA received) or replaced (new ADDBA on the same TID) gets none.
```

## L4334-4337 · `fn ba_stop_all(&mut self) {`

```
/// Every session down — the firmware's are gone with the association, and
/// a BAID we no longer remember is one we can never free. Linux does this
/// in `iwl_mvm_rm_sta`; we kept the station across a reconnect (MODIFY,
/// not remove/re-add) and so have to do it explicitly.
```

## L4347 · `fn send_delba(&mut self, tid: u8, initiator: u16, reason: u16) {`

```
/// DELBA action frame (802.11 §9.6.5.4): category, action, params, reason.
```

## L4366-4368 · `fn ba_on_baid_alloc(&mut self, baid: u32) {`

```
/// The allocation command answers with a bare BAID (iwl_rx_baid_cfg_resp).
/// Unlike ADD_STA there is no status word: an error arrives as a value
/// outside the map, exactly as Linux checks it.
```

## L4398-4400 · `fn ba_remove(&mut self, tid: u8, baid: u8) {`

```
/// Tear the session down in the firmware too (STA_MODIFY_REMOVE_BA_TID).
/// Dropping only our buffer would leave the firmware stamping BAIDs for a
/// session the AP has ended, and the next ADDBA would find the slot taken.
```

## L4405-4409 · `if fw_cmd_ver(DATA_PATH_GROUP, RX_BAID_ALLOCATION_CONFIG_CMD) == 1 {`

```
// iwl_mvm_fw_baid_op_cmd (sta.c:2833) has a THIRD branch we had
// skipped: at command version 1 the remove payload is a bare
// `__le32 baid` (remove_v1), not sta_id_mask + tid. Sending the v2
// form to a v1 firmware would have it read our station mask
// (BIT(0) = 1) as the BAID to free — i.e. free the wrong session.
```

## L4420 · `sc[AS_OFF_ADD_MODIFY] = 1; // modify`

```
// modify
```

## L4428-4430 · `fn ba_on_add_sta_status(&mut self, status: u32) {`

```
/// The firmware answered our ADD_STA. On a started session the status word
/// carries the BAID that will appear in every aggregated frame; only then do
/// we tell the AP it may aggregate.
```

## L4435-4437 · `if self.st.addba_seen > 0 && self.st.addba_timeouts == 0 {`

```
// An ADD_STA we sent for something else — or the answer to a
// request that already timed out above. Worth saying once:
// both counters staying at zero looked like "never called".
```

## L4475-4483 · `fn ba_reply(&mut self, p: &BaPending, status: u16) {`

```
/// The ADDBA response frame.
///
/// The parameter set is built from scratch, exactly as
/// `ieee80211_send_addba_resp` does, and NOT echoed from the request. That
/// distinction cost a release: echoing kept the AP's A-MSDU bit, which told
/// it that it may pack several MSDUs into one MPDU — and `rx_classify`
/// decodes exactly one, so every aggregated frame turned to garbage and the
/// link carried nothing at all. mac80211 sets the bit from its OWN
/// capability (SUPPORTS_AMSDU_IN_AMPDU); ours is no.
```

## L4486 · `fr[0] = (DOT11_STYPE_ACTION << 4) | 0x00; // management, subtype action`

```
// management, subtype action
```

## L4495 · `let params = BA_PARAM_POLICY_IMMEDIATE // A-MSDU bit deliberately 0`

```
// A-MSDU bit deliberately 0
```

## L4503-4518 · `fn tx_8023(&mut self, dst: [u8; 6], ethertype: u16, payload: &[u8], encrypt: bool,`

```
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
```

## L4521-4525 · `if !critical && (!self.aql_admits() || self.data_in_flight >= TX_INFLIGHT_MAX) {`

```
// Flow control + anti-bufferbloat: AQL, so what a queued frame costs is
// measured in AIRTIME at the current rate — a 67-byte ACK and a
// 1514-byte frame at 6 Mbit and at 300 Mbit are four different prices.
// Ring guard behind it so write_ptr never laps the firmware's read
// pointer. Caller leaves the rest in the kernel mailbox for retransmit.
```

## L4529-4531 · `let hdr_len = if self.qos { DOT11_QOS_HDR_LEN } else { DOT11_HDR_LEN };`

```
// As a QoS (HT) station every data frame carries a QoS control field, so
// the header grows from 24 to 26 bytes. tx_raw derives offload_assist and
// the DWORD padding from the length we pass it.
```

## L4539-4558 · `if !self.qos {`

```
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
```

## L4563 · `let mut p = hdr_len;`

```
// QoS control: TID 0 (best effort), normal ack, no A-MSDU. Bytes stay 0.
```

## L4572-4573 · `0`

```
// IP data after AUTHORIZED: no CMD_RATE → the firmware rate-scales
// (TLC); no ENCRYPT_DIS → it encrypts with the installed PTK.
```

## L4576 · `IWL_TX_FLAGS_ENCRYPT_DIS | IWL_TX_FLAGS_CMD_RATE`

```
// EAPOL during the 4-way: robust fixed 1 Mbit CCK, unencrypted.
```

## L4592-4593 · `let slot = (wptr_before & (IWL_DATA_QUEUE_SIZE as u32 - 1)) as usize;`

```
// Record what this slot now holds BEFORE the pointer moved on, so the
// per-pass re-derivation can walk back over it.
```

## L4595 · `let est = self.expected_tx_airtime(p).min(u16::MAX as u32);`

```
// `ieee80211_sta_update_pending_airtime(..., tx_completed = false)`.
```

## L4603-4623 · `fn expected_tx_airtime(&self, len: usize) -> u32 {`

```
/// Re-derive the in-flight byte count from the slots `data_in_flight`
/// covers. Walking back from the write pointer ties it to the same
/// authority as the frame count, so a correction there corrects this too —
/// and a swallowed completion cannot leak bytes forever any more than it
/// can leak slots.
/// `ieee80211_calc_expected_tx_airtime` (mac80211/airtime.c:756) for the
/// path we actually take: an HT/VHT station on a non-VO access category,
/// i.e. the aggregated branch.
///
/// Linux looks the per-MCS duration up in `airtime_mcs_groups`, a table of
/// transmit times for an `AVG_PKT_SIZE` packet, then scales it by the real
/// length. That table IS a rate table — and we already decode the exact
/// rate the firmware last used (`rate_kbit`, the same numbers printed as
/// `rate tx`). So the lookup is replaced by the rate we have; the formula,
/// the overhead term and the aggregation thresholds are Linux's, unchanged.
///
/// The `agg_shift` ladder is the interesting part and the reason a byte cap
/// can never do this: Linux divides the fixed per-PPDU overhead by an
/// assumed aggregate length, and assumes MORE aggregation the faster the
/// link is. Its thresholds are stated in duration-per-AVG_PKT_SIZE, so they
/// are compared against exactly that.
```

## L4629-4632 · `return AQL_MIN_US;`

```
// No rate reported yet. Linux falls back to the lowest basic rate
// here; before the first TX response we have no station rate at
// all, and the frames sent in that window are `critical` anyway
// (they bypass the cap), so the floor is the honest answer.
```

## L4635 · `let data_us = ((len as u64) * 8 * 1000 / kbit as u64) as u32;`

```
// Raw data time for `len` bytes at this rate, in microseconds.
```

## L4641-4642 · `let streams = 1u32;`

```
// `stat.encoding == RX_ENC_LEGACY || !ampdu` -> the un-aggregated
// path, where the whole per-frame overhead is paid once per frame.
```

## L4648 · `let avg_us = ((AQL_AVG_PKT_SIZE as u64) * 8 * 1000 / kbit as u64) as u32;`

```
// The ladder's thresholds are durations for an AVG_PKT_SIZE packet.
```

## L4666-4667 · `fn aql_admits(&self) -> bool {`

```
/// `ieee80211_txq_airtime_check` (mac80211/tx.c:4164), branch for branch.
/// Returns true when the frame may be queued.
```

## L4672-4673 · `let total_pending = self.aql_pending_us;`

```
// `total_pending` is this station's pending for us — one station, one
// AC. The branch stays whole so a second station finds it correct.
```

## L4689-4691 · `fn tx_eth(&mut self, eth: &[u8]) -> bool {`

```
// Convert an Ethernet frame from the IP stack ([dst 6][src 6][etype 2][pl])
// into an encrypted 802.11 data frame and transmit it. Returns false if the
// queue was full (frame dropped → the IP stack will retransmit).
```

## L4702-4703 · `fn alloc_data_queue(&mut self) -> bool {`

```
// Allocate a gen2 data TX queue (tid 0) for the AP station, so EAPOL frames
// have a data path. Same SCD_QUEUE_CONFIG mechanism as the mgmt queue.
```

## L4739-4750 · `fn alloc_key_slot(&mut self, group: bool) -> u8 {`

```
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
```

## L4753 · `return 0; // pairwise: one station, one slot, never rotated`

```
// pairwise: one station, one slot, never rotated
```

## L4766 · `best = 1;`

```
// Every slot busy: free the oldest group key rather than refuse.
```

## L4769-4771 · `if let Some(old) = self.key_slot_prev.replace(best) {`

```
// Only TWO group keys can be live at once (802.11 key ids 1 and 2), so
// releasing the one before last keeps the table from filling while the
// previous key stays valid for the whole transition.
```

## L4797 · `self.send_hcmd(0, ADD_STA_KEY_CMD, &cmd); // → LONG_GROUP(1)`

```
// → LONG_GROUP(1)
```

## L4811-4814 · `fn rx_mgmt_for_us(rb: &Dma, our_mac: &[u8; 6]) -> Option<(u8, [u8; 12])> {`

```
// Extract an 802.11 management frame from an RX buffer if it is addressed to
// us (addr1 == our MAC). Returns (subtype, first 12 body bytes after the
// 24-byte header) — 12 covers the longest body we inspect, an ADDBA request.
// Same RB layout as parse_beacon: frame @ RX_PKT_DATA_OFF + desc(48).
```

## L4818 · `let f = RX_PKT_DATA_OFF + IWL_RX_DESC_SIZE_V1; // 56`

```
// 56
```

## L4821 · `return None; // not a management frame`

```
// not a management frame
```

## L4824 · `return None; // not addressed to us`

```
// not addressed to us
```

## L4833-4839 · `fn note_llc_miss(budget: &mut u32, want: usize, found: usize) {`

```
// Classify a received 802.11 DATA frame addressed to us: EAPOL (4-way) vs IP.
// Finds the LLC/SNAP header at the 802.11 header end or 8 bytes further (an
// intact CCMP header on a just-decrypted frame). EAPOL → the self-describing
// EAPOL frame in `out`; IP → an Ethernet frame [dst=us][src=addr3][etype][pl].
/// Report that the payload was not where the descriptor said it would be.
/// Budgeted: if the computation is systematically wrong this fires on every
/// frame, and a log that writes the normal case is not a log any more.
```

## L4856-4857 · `fn rx_classify(rb: &Dma, our_mac: &[u8; 6], out: &mut [u8], miss_log: &mut u32,`

```
/// `ampdu_tog` carries the aggregate we are inside of across calls:
/// `0xFF` = not in one, otherwise the firmware's TOGGLE bit as 0 or 1.
```

## L4867 · `let f = d + IWL_RX_DESC_SIZE_V1; // 56`

```
// 56
```

## L4869-4871 · `if buf[f + DOT11_OFF_ADDR1..f + DOT11_OFF_ADDR1 + 6] == our_mac[..] {`

```
// Count anything unicast to our address, whatever its type. This is the
// one number that separates "the AP stopped talking to us" from "it is
// talking and we discard it" — and without it both look like silence.
```

## L4874-4880 · `let to_dbm = |e: u8| if e != 0 { -(e as i16) } else { -128 };`

```
// `ieee80211_rx_h_sta_process` (mac80211/rx.c:1807) feeds the signal
// of every frame from the station into an EWMA. Frames addressed to
// us in a BSS come from the AP, so this is the same set.
//
// `iwl_mvm_get_signal_strength` (mvm/rxmq.c:306): a zero chain means
// "not measured", not 0 dBm, and the stronger of the two chains
// wins.
```

## L4892-4893 · `let multicast = buf[f + DOT11_OFF_ADDR1] & 0x01 != 0;`

```
// Accept frames to us OR to a group address (multicast bit / broadcast) —
// a DHCP offer / ARP reply often comes back L2-broadcast.
```

## L4899-4906 · `if subtype & DOT11_STYPE_NODATA != 0 {`

```
// Data subtypes with bit 2 set carry NO BODY: Null (4) and QoS Null (12)
// are the ones that occur — the AP's keepalive, and what Linux sends in
// `ieee80211_mgd_probe_ap`. They have no LLC/SNAP because they have no
// payload, and treating that as a decode failure made a perfectly normal
// frame look like corruption: it counted into `undecoded` and burned the
// budget of a log meant for real misses. Observed as a recurring
// "RX payload offset mismatch: computed +88" — 56 + 24 + 8, i.e. exactly
// a non-QoS data header with CCMP.
```

## L4911-4916 · `let rnf = le32(&buf, d + MPDU_OFF_RATE_N_FLAGS);`

```
// Where the payload actually starts, exactly as iwl_mvm_create_skb
// computes it: 802.11 header, then the IV the firmware left in place for
// the cipher it decrypted with, then the DWORD padding the firmware
// inserts when header+IV is not a multiple of 4 (the QoS+CCMP case).
// Air this frame occupied. `mpdu_len` and the rate are both already in
// the buffer we just read, so this costs an integer divide, no DMA.
```

## L4920-4926 · `let cck = rnf & RATE_MCS_MOD_TYPE_MSK == RATE_MCS_MOD_TYPE_CCK;`

```
// The whole exchange, not just the bits on the wire. The first
// version counted preamble + SIFS + ACK and called itself a lower
// bound — but DIFS and the average backoff are the BIGGEST term at
// HT rates (102 of 268 µs for a 1500-byte frame at 144 Mbit), so
// that "bound" read 1.5x low and argued the channel was idle when
// it was about half full. A number that misleads by 1.5x is worse
// than no number.
```

## L4929 · `RATE_MCS_MOD_TYPE_CCK => 96,       // short preamble`

```
// short preamble
```

## L4930 · `RATE_MCS_MOD_TYPE_HT => 40,        // HT-mixed, 2 spatial streams`

```
// HT-mixed, 2 spatial streams
```

## L4931 · `_ => 20,                           // legacy OFDM`

```
// legacy OFDM
```

## L4933-4934 · `let overhead = if cck { 10 + 40 + 50 + 150 } else { 16 + 28 + 34 + 68 };`

```
// SIFS + ACK + DIFS + mean backoff (CWmin 15 for best effort, so
// 7.5 slots). Slot and SIFS differ between the DSSS and OFDM PHYs.
```

## L4936-4942 · `let phy_info = (buf[d + MPDU_OFF_PHY_INFO] as u16)`

```
// In an A-MPDU the preamble, SIFS, block-ack, DIFS and backoff are
// paid ONCE for the whole aggregate — not per subframe. Charging
// them per subframe made the report claim `rx 186 %` of a window,
// which is not a busy channel but a broken ruler. The firmware
// flips PHY_AMPDU_TOGGLE at the start of every new aggregate
// (iwl_mvm_rx_mpdu_mq), so a subframe whose toggle matches the one
// before it costs data time only.
```

## L4951 · `*ampdu_tog = 0xFF; // outside an aggregate every frame pays in full`

```
// outside an aggregate every frame pays in full
```

## L4959-4961 · `if buf[f + 1] & DOT11_FC_PROTECTED != 0 {`

```
// Did the firmware actually decrypt this? We do not drop on the answer —
// the point is to learn whether a link that looks alive is receiving
// frames it cannot read. See iwl_mvm_rx_crypto (rxmq.c:414).
```

## L4985-4987 · `let llc = if at(want) {`

```
// If the computed position is not where LLC/SNAP actually sits, fall back
// to searching the two places it can be and say so — a silent mismatch
// here would drop every frame and look like a dead link.
```

## L4997-5000 · `Self::note_llc_miss(miss_log, want, usize::MAX);`

```
// Addressed to us, a data frame WITH a body, and LLC/SNAP is at none
// of the possible offsets. Silently dropping this was a blind spot.
// `usize::MAX` = nowhere; `0` used to be the sentinel and read like
// a real offset ("found +0"), which is its own small lie.
```

## L5008 · `let pl = llc + 8; // payload after LLC/SNAP`

```
// payload after LLC/SNAP
```

## L5020-5022 · `if ethertype != 0x0800 && ethertype != 0x0806 {`

```
// Only IPv4 (0x0800) + ARP (0x0806) belong in the kernel IP stack.
// Other ethertypes the AP floods (0x88e1 HomePlug, multicast, …) are
// not ours to handle — drop them early instead of feeding the stack.
```

## L5026-5033 · `let mic_crc_len =`

```
// Ethernet frame for the IP stack: dst = us (addr1), src = addr3 (SA).
// mpdu_len spans the whole frame including the firmware's padding and
// whatever MIC/CRC the RADA left on the tail — strip both, exactly as
// iwl_mvm_create_skb does, or the stack sees trailing garbage.
// The payload always ends mic_crc_len before the end of the MPDU:
// mpdu_len covers header + IV + padding + payload + MIC, and the
// padding sits before the payload, so it cancels out. That makes the
// end independent of how the IV/padding split was determined above.
```

## L5057-5058 · `fn wait_mgmt_response(&mut self, want_subtype: u8, ms: u32) -> Option<[u8; 12]> {`

```
// Drain the RX ring up to `ms` ms looking for a management frame of the given
// subtype addressed to us; return its first 8 body bytes. Recycles RBs.
```

## L5082-5084 · `fn connect_send_auth(&mut self) -> bool {`

```
// ── Stage 5c: open-system AUTH (connect step 3) ──────────────────────
// Session protection (the prepare_tx hook) then an open-system auth request to
// the target AP, then wait for the AP's auth response (subtype auth, seq 2).
```

## L5086-5087 · `let mut sp = [0u8; SP_CMD_LEN];`

```
// SESSION_PROTECTION_CMD (cmd_ver 1, wait_for_notif=false) — reserves
// channel time so the firmware actually transmits the unassociated frame.
```

## L5097 · `let mut fr = [0u8; DOT11_HDR_LEN + DOT11_AUTH_BODY_LEN];`

```
// 802.11 open-system auth request: DA/BSSID = AP, SA = us, seq 1.
```

## L5110 · `match self.wait_mgmt_response(DOT11_STYPE_AUTH, 2000) {`

```
// Auth response body: algorithm(2), seq(2), status(2).
```

## L5135-5140 · `fn connect_send_assoc(&mut self) -> bool {`

```
// ── Stage 5d: association request → response (connect step 4) ─────────
// Build an association request (mac80211 ieee80211_send_assoc, legacy IE set)
// for the target AP, transmit it, and wait for the association response
// (subtype 1) to read the status code + AID. For an encrypted AP we include a
// WPA2-PSK-CCMP RSN element so the AP accepts the association (the 4-way
// handshake / key install that follows lives in wifid — Phase H).
```

## L5149 · `let mut cap = WLAN_CAP_ESS | WLAN_CAP_SHORT_PREAMBLE;`

```
// Fixed fields: capability info + listen interval.
```

## L5161 · `let sl = self.target_ssid_len as usize;`

```
// SSID element.
```

## L5168 · `let (supp, ext): (&[u8], &[u8]) = if self.target_band == PHY_BAND_24 as u8 {`

```
// Supported + extended supported rates (rate byte = Mbps*2, basic bit 0x80).
```

## L5185-5191 · `if self.target_ht.present {`

```
// HT Capability element (802.11n). Only when the AP advertised HT — an
// AP without it would get an element it never asked for, and everything
// downstream (station flags, TLC mode HT) derives from its parameters.
// The claimed width MUST match the PHY context: advertising 20/40 with
// a 20 MHz radio invites frames it cannot receive, and claiming 20 with
// a 40 MHz context wastes the half we configured. Both follow
// `use_ht40`, which is the single place that decides.
```

## L5196-5197 · `let mut cap = IEEE80211_HT_CAP_SM_PS_DISABLED;`

```
// SM Power Save disabled (both chains stay live), short GI at 20 MHz
// and RX-STBC one stream — each only if the AP supports it too.
```

## L5217 · `fr[b + HT_OFF_MCS_RX_MASK] = 0xff;`

```
// Supported receive MCS set: both spatial streams, MCS 0-15 (2x2).
```

## L5220 · `fr[b + HT_OFF_MCS_TX_PARAMS] = IEEE80211_HT_MCS_TX_DEFINED;`

```
// tx_params: TX MCS set defined and equal to the RX set (no TX_RX_DIFF).
```

## L5225-5227 · `if self.use_vht80() {`

```
// VHT capabilities — only when we actually run 80 MHz. Same rule as
// HT: what we claim has to match the PHY context, or the AP sends at a
// width the radio is not listening on.
```

## L5232 · `let mut cap = 0u32; // MAX_MPDU_LENGTH_3895 is 0`

```
// MAX_MPDU_LENGTH_3895 is 0
```

## L5240 · `put_u16(&mut fr, b + VHT_OFF_RX_MCS_MAP, VHT_MCS_MAP_2SS);`

```
// Two spatial streams at MCS 0-9, the rest marked unused.
```

## L5243 · `p += 2 + VHT_CAP_IE_LEN;`

```
// rx_highest / tx_highest stay 0: "no specified maximum".
```

## L5247 · `if self.target_privacy {`

```
// RSN element (WPA2-PSK-CCMP) for encrypted APs.
```

## L5250 · `0x01, 0x00, // version 1`

```
// version 1
```

## L5251 · `0x00, 0x0f, 0xac, 0x04, // group cipher: CCMP`

```
// group cipher: CCMP
```

## L5252 · `0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, // pairwise: 1 × CCMP`

```
// pairwise: 1 × CCMP
```

## L5253 · `0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, // AKM: 1 × PSK`

```
// AKM: 1 × PSK
```

## L5254 · `0x00, 0x00, // RSN capabilities`

```
// RSN capabilities
```

## L5262-5264 · `if self.target_ht.present {`

```
// WMM information element — vendor-specific, so it goes last. An HT
// station is a QoS station; without this the AP has no reason to grant
// us EDCA parameters and may decline to use HT rates at all.
```

## L5273 · `match self.wait_mgmt_response(DOT11_STYPE_ASSOC_RESP, 2000) {`

```
// Assoc response body: capability(2), status_code(2), aid(2).
```

## L5282-5283 · `self.qos = self.target_ht.present;`

```
// We asked for HT + WMM and the AP accepted → from here on we
// are a QoS station and send QoS data frames.
```

## L5303-5309 · `fn run_netdev(&mut self, associated: bool) -> ! {`

```
// ── Resident NIC service loop ─────────────────────────────────
// The chip is up and the scan has run; register as a network interface and
// own the card from here. Same shape as aml.wasm: an infinite loop that
// does the driver's work and yields via npk_sleep — never returns (the
// driver holds its DMA + the netdev registration for its lifetime). Frame
// bridging to the kernel netdev mailboxes (TX poll / RX submit) plugs into
// this loop once association brings up the data path.
```

## L5319-5323 · `let our_mac = self.mac;`

```
// The data TX queue was allocated before auth (so its SCD-response wait
// wouldn't swallow the AP's first EAPOL frame). Tell wifid the connection
// is ready + the MACs it needs for the PTK, then listen immediately — but
// ONLY if we actually associated. Otherwise the link stays down (wlan is
// registered but not primary) and we don't arm wifid for a dead BSS.
```

## L5333-5336 · `self.connect_post_assoc();`

```
// Tell the firmware we are associated (MAC context + station), then
// start rate scaling. Linux does all three at the assoc state change;
// data only flows after AUTHORIZED, so this is always in place before
// the first IP frame.
```

## L5345 · `let mut rx_log = 0u32; // throttle the data-path diagnostics`

```
// throttle the data-path diagnostics
```

## L5346 · `let mut rate_log = 0u32; // …and the rate-change lines, which flap at VHT80`

```
// …and the rate-change lines, which flap at VHT80
```

## L5348-5351 · `let mut last_tx_rate = u32::MAX;`

```
// Air-rate visibility. The firmware reports the TX rate it settled on
// via TLC_MNG_UPDATE_NOTIF; the RX descriptor carries the rate the AP
// used towards us. Log only when either CHANGES — a per-frame log would
// drown the ring, and the interesting event is the transition.
```

## L5355 · `let mut llc_miss = 8u32; // budget for RX-offset mismatch reports`

```
// budget for RX-offset mismatch reports
```

## L5356 · `let mut ampdu_tog = 0xFFu8;`

```
// Which A-MPDU we are inside of, so its fixed overhead is charged once.
```

## L5359 · `let mut deauth_total = 0u32; // diagnostic: link-loss events seen`

```
// diagnostic: link-loss events seen
```

## L5360 · `if self.lmac_err_ptr != 0 && self.grab_nic_access() {`

```
// One-shot: is the firmware healthy after bring-up?
```

## L5374-5377 · `let t_pass = host::now_us();`

```
// Where the pass's time actually goes. The old `busy` counter only
// said whether a pass FOUND work — it was read as CPU load (by
// Claude, and it was wrong). This measures: work microseconds, drain
// microseconds, and what a 1 ms sleep really costs.
```

## L5379-5381 · `let mut tx_done = 0u32;`

```
// RX: drain + recycle the ring. EAPOL-Key frames → wifid (the 4-way);
// decrypted IP/other data → the kernel IP stack as Ethernet frames.
// TX completions (TX_CMD response) free data-queue slots.
```

## L5386-5387 · `let mut a_ok = 0u32;`

```
// Per-pass accumulators: the RX closure cannot touch `self` (it is
// borrowed by service_rx), so everything is folded in afterwards.
```

## L5417 · `let a_dataq = self.data_queue_id as u32;`

```
// The data queue id, captured before the closure borrows self.
```

## L5423-5427 · `tx_done += 1;`

```
// gen2 TX completion — one per transmitted data/mgmt frame.
// struct iwl_tx_resp carries what it COST on the air: the
// retry count, the rate the firmware started at and the
// microseconds of airtime consumed. Reading it is the only
// way to tell a slow link from a retrying one.
```

## L5431-5434 · `let seq = u16::from_le_bytes([tr[6], tr[7]]) as u32;`

```
// The response header carries the TFD index it completes
// (`SEQ_TO_INDEX`, cmdhdr.h:20) and the queue it belongs to
// (`SEQ_TO_QUEUE`). That is the firmware's read pointer,
// stated outright — no need to count.
```

## L5437-5454 · `a_read_ptr = Some((seq & 0xff) & (IWL_DATA_QUEUE_SIZE as u32 - 1));`

```
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
```

## L5458-5460 · `let fc = tr[base + TXR_OFF_FRAME_COUNT];`

```
// "frame_count: 1 no aggregation, >1 aggregation"
// (fw/api/tx.h). The field has been in the struct since the
// first port and unread ever since — the compiler said so.
```

## L5482-5484 · `let mut bn = [0u8; RX_PKT_DATA_OFF + CBA_HDR_LEN];`

```
// iwl_mvm_rx_ba_notif, new-tx-api path. On TLC-offload
// firmware this is where an aggregate reports itself:
// `txed` MPDUs went out, `done` were acknowledged.
```

## L5493-5499 · `a_airtime += u32::from_le_bytes([`

```
// Airtime for the whole aggregate (`iwl_mvm_tx_airtime`,
// mvm/tx.c:2151, from `ba_res->wireless_time`). The TX_CMD
// response carries `wireless_media_time` and we have read it
// since the first port — but an aggregated MPDU produces no
// TX_CMD response, so on an upload only 405 of 147379 frames
// reported any airtime at all and `air … tx` showed 0 %.
// Same shape as the read pointer: one of two sources read.
```

## L5506-5513 · `let tfd_cnt = u16::from_le_bytes(`

```
// The RECLAIM half, and the reason 0.99.0 collapsed to
// 16 Mbit: an aggregated MPDU gets NO TX_CMD response. Its
// TFD slot is freed here or it is never freed at all. Linux
// does exactly this — `iwl_mvm_rx_ba_notif` walks the tfd
// array and hands each `tfd_index` to `iwl_mvm_tx_reclaim`
// as the queue's new read pointer (mvm/tx.c, new-tx-api
// path). We read `txed`/`done` from this notification since
// 0.93.0 and left the two fields next to them unread.
```

## L5534-5535 · `let mut p = [0u8; RX_PKT_DATA_OFF + 4];`

```
// Only the block-ack setup sends ADD_STA while the loop runs;
// its status word carries the session id (see ba_request).
```

## L5540 · `let mut p = [0u8; RX_PKT_DATA_OFF + 4];`

```
// iwl_rx_baid_cfg_resp — a bare __le32 baid.
```

## L5545 · `let mut p = [0u8; RX_PKT_DATA_OFF + 20];`

```
// iwl_mvm_handle_missed_beacons_notif (mvm/mac-ctxt.c:1615).
```

## L5554-5555 · `let mut p = [0u8; RX_PKT_DATA_OFF + 4];`

```
// The window moved without a frame for us: the firmware saw
// the MPDUs on air. Nothing here may be held back for them.
```

## L5567-5569 · `let mut p = [0u8; 24];`

```
// The firmware's rate-scaling verdict: what it is actually
// transmitting at. Without this the host is blind to the
// negotiated air rate.
```

## L5581-5585 · `if let Some((st, body)) = Self::rx_mgmt_for_us(rb, &our_mac) {`

```
// DIAGNOSTIC ONLY: note a DEAUTH / DISASSOC addressed to us +
// its reason, but do NOT tear down or reconnect — a reconnect
// would just mask whatever made us lose the link (our bug vs a
// genuinely-absent AP). Keep draining so detection never
// disrupts a healthy link.
```

## L5588 · `a_to_us += 1; // rx_classify never sees these — count here`

```
// rx_classify never sees these — count here
```

## L5597 · `addba = Some(body); // answered outside`

```
// answered outside
```

## L5610 · `return true; // mgmt frame — not for the IP path`

```
// mgmt frame — not for the IP path
```

## L5626-5637 · `rx_rate_tick += 1;`

```
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
```

## L5645-5650 · `if rate_log < 8 {`

```
// Budgeted. At VHT80 the rate flaps between
// MCS 8 and 9 continuously, and every line
// goes to the terminal, the global mirror
// AND out over TCP — in the middle of the
// measurement it is meant to inform. The
// report carries the current rate anyway.
```

## L5666-5679 · `let taken = agg.reorderable && {`

```
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
```

## L5701-5708 · `let counted = self.data_in_flight.saturating_sub(tx_done);`

```
// Free the data-queue slots the firmware just reported done.
//
// Derived, not counted: `(write - read) & 255` is what the firmware
// and we actually disagree about, and a swallowed completion is
// repaired by the next one instead of leaking a slot forever.
// Measured before this: 8 of 16 slots lost for 314 s, and an OTA
// update that failed because a one-second stall killed its TLS
// handshake.
```

## L5714-5723 · `if derived + 1 < counted && a_ba_reclaims == 0`

```
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
```

## L5734-5740 · `self.data_in_flight = counted.min(derived);`

```
// Only ever LOWER the count. The derived value exists to
// repair a leak; it must never be able to create one. Getting
// the two pointers into different moduli once already wedged
// transmission completely (in-flight 198 against a cap of 16,
// 8 Mbit, dead link), and a diagnostic that can block the
// queue is worse than the leak it fixes. Taking the minimum
// means a disagreement — whatever its cause — costs nothing.
```

## L5746 · `self.st.loop_iters = self.st.loop_iters.wrapping_add(1);`

```
// Fold this pass's accumulators into the running statistics.
```

## L5771-5772 · `match rx_frames {`

```
// `t_pass` is read at the top of every pass anyway, so the shape of
// the arrivals costs a compare and an add.
```

## L5798-5799 · `if let Some((consec, since_rx, expected, received)) = mb.take() {`

```
// Answer a block-ack setup request (the TX has to happen outside the
// RX closure, which holds &mut self through service_rx).
```

## L5806-5809 · `if consec >= IWL_MVM_MISSED_BEACONS_THRESHOLD_LONG {`

```
// iwl_mvm_handle_missed_beacons_notif, verbatim in its thresholds:
// a long run of missed beacons AND nothing received since is a
// link that is gone. The same run WITH data still arriving is not
// — Linux stays connected there and says it expects trouble.
```

## L5838-5841 · `if let Some(p) = self.ba_pending.as_ref() {`

```
// A firmware that never answers must not cost us the link. Linux
// treats a refusal as "decline and carry on"; silence has to mean
// the same, or the AP waits for a reply forever and stops sending.
// Declining is a working link, just a slow one.
```

## L5848-5850 · `self.ba_fw_broken = true;`

```
// Once is enough. Every further attempt is another 300 ms of
// silence towards the AP and another command the firmware
// does not finish.
```

## L5858-5862 · `if let Some((tid, initiator)) = delba.take() {`

```
// `ieee80211_process_delba`: the frame names the TID and who is
// ending it. Only an INITIATOR-side DELBA concerns our RX session,
// and we send none back — the AP already knows. A single global
// flag used to tear down whichever session happened to be in the
// one slot, regardless of the TID the AP named.
```

## L5868 · `ba::tick_all();`

```
// A hole the AP never fills must not park the window forever.
```

## L5870-5873 · `let now_ms = host::now_ms();`

```
// sta_rx_agg_session_timer_expired: a session the AP has gone quiet
// on for longer than it asked for is stale. Linux tears it down and
// sends a DELBA with WLAN_REASON_QSTA_TIMEOUT. We kept such a
// session forever, holding a firmware BAID nothing would ever use.
```

## L5883-5906 · `let now_ms = host::now_ms();`

```
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
```

## L5921-5923 · `host::print("[ax200] ** LINK-LOSS ** ");`

```
// The cause is already printed for a beacon loss; a frame-borne
// loss names its subtype and reason code, which is the part that
// says whether the AP dropped us or our own behaviour did.
```

## L5937-5939 · `host::print(" - in cooldown, not re-scanning yet\n");`

```
// A failed reconnect just ran. Re-scanning on every kick of a
// deauth storm would spend the whole time scanning and never
// be listening when the AP is ready for us.
```

## L5949-5950 · `next_report = host::now_ms() + REPORT_PERIOD_MS;`

```
// The scan inside reconnect drained the ring; restart the
// window so the next report measures fresh traffic.
```

## L5954-5957 · `let mut tx_any = false;`

```
// TX: send every Ethernet frame the IP stack queued (DHCP, ARP, …),
// but stop once the data queue is full — leave the rest in the kernel
// mailbox so we never pop a frame we'd have to drop (and never lap the
// firmware's read pointer).
```

## L5960-5964 · `if !self.aql_admits() {`

```
// Two different walls, counted apart: the byte cap is policy and
// can be raised on its own; the ring guard is the queue depth and
// needs IWL_DATA_QUEUE_SIZE to move with it. Which one bites is
// the whole question for the next size change, and one shared
// counter could not answer it.
```

## L5966-5968 · `self.st.tx_blocked = self.st.tx_blocked.wrapping_add(1);`

```
// Not a drop: the frame stays in the kernel queue. But it IS
// the moment the cap becomes the throughput limit, so it has
// to be visible before anyone raises it.
```

## L5976-5980 · `if !self.authorized {`

```
// Second line of defence: do not pull traffic before the
// station is authorized. The firmware cannot transmit for an
// unauthorized station, so those frames occupy TFD slots that
// are never completed — data_in_flight never returns to zero and
// the queue is wedged before the link is even up.
```

## L6006 · `let clen = host::wifi_poll_cmd(&mut cmd);`

```
// Control commands from wifid (TX_EAPOL / SET_KEY / AUTHORIZED).
```

## L6011-6023 · `let now_ms_pass = host::now_ms();`

```
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
```

## L6028-6029 · `if self.st.tx_wd_recoveries < 8 {`

```
// Budgeted: if this fires continuously the log stops being a
// log. The report keeps the running count either way.
```

## L6038-6041 · `self.data_in_flight = 0;`

```
// Linux forces an NMI and restarts the firmware here. We cannot
// do that cheaply, so we reclaim instead — the slots are lost
// either way, and a half-width queue that keeps shrinking ends
// as a dead link.
```

## L6047-6055 · `if rx_frames > 0 {`

```
// Adaptive pacing: while frames are flowing OR completions are still
// pending, poll again in 1 ms so the RX ring is drained before it
// overflows and queue slots free up quickly; when idle, 4 ms keeps the
// RX latency floor low (the ping/round-trip baseline) while still
// yielding the core (npk_sleep yields the fiber). A proper IRQ wake is
// the eventual fix; 4 ms is the interim quick-win over the old 20 ms.
// RX-silence watchdog. Frames of some kind always arrive on a live
// channel — beacons alone are ~10/s. Total silence means the
// firmware has no buffer to fill, not that the air went quiet.
```

## L6065-6069 · `if self.st.rx_wd_fires <= 4 {`

```
// Budgeted like the TX watchdog above. This used to print
// unconditionally on every round, so a permanent fault read as
// an endless loop and the log stopped being a log. `armed` is
// the fact worth having: 0 means the firmware still holds every
// buffer, so the pool was never the cause.
```

## L6077-6081 · `if self.st.rx_wd_dry == 3 && self.rf_killed() {`

```
// Linux answers a wedged RX path with iwl_force_nmi() and a
// firmware restart. We cannot restart the firmware cheaply, so
// after three rounds with nothing to re-arm take the one
// escalation we have — read the error table once, then rebuild
// the association instead of poking the ring forever.
```

## L6083-6085 · `if self.st.rx_wd_fires <= 12 {`

```
// Re-scanning against a dead radio is a loop, not a
// recovery: wait for the switch instead. Budgeted, because
// this is a state, not an event.
```

## L6103-6107 · `if associated && !self.authorized && self.st.rx_eapol == 0 {`

```
// 4-way watchdog. An AP starts the handshake within milliseconds of
// the association response; if nothing has arrived after this long,
// either it gave up on us or we stopped hearing it — and in both
// cases waiting forever is the one useless option. mac80211 does the
// same (IEEE80211_ASSOC_TIMEOUT then a fresh attempt).
```

## L6126-6136 · `if self.authorized != self.link_published {`

```
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
```

## L6148-6149 · `let now = host::now_ms();`

```
// Publish the status snapshot once a second. Reading the clock is one
// host call per pass; formatting happens 1/1000 of those.
```

## L6163 · `fn handle_wifi_cmd(&mut self, cmd: &[u8]) {`

```
// Dispatch one control command from wifid (the supplicant).
```

## L6166 · `Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {`

```
// TX_EAPOL: [op][len u16][frame] → unencrypted EAPOL to the AP.
```

## L6175-6188 · `let enc = self.ptk_installed;`

```
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
```

## L6196 · `Some(CMD_SET_KEY) if cmd.len() >= 5 => {`

```
// SET_KEY: [op][key_type][key_idx][cipher][key_len][key..][rsc 6].
```

## L6199 · `let key_type = cmd[1]; // 0=PTK/pairwise 1=GTK/group`

```
// 0=PTK/pairwise 1=GTK/group
```

## L6208 · `Some(CMD_AUTHORIZED) => {`

```
// AUTHORIZED: 4-way done → carrier up, IP data path live.
```

## L6211-6223 · `self.mac_ctxt_assoc();      // callbacks->mac_ctxt_changed`

```
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
```

## L6224 · `self.mac_ctxt_assoc();      // callbacks->mac_ctxt_changed`

```
// callbacks->mac_ctxt_changed
```

## L6225 · `self.sta_assoc_update();    // callbacks->update_sta`

```
// callbacks->update_sta
```

## L6226 · `self.connect_tlc_config();  // iwl_mvm_rs_rate_init_all_links`

```
// iwl_mvm_rs_rate_init_all_links
```

## L6239-6242 · `fn grab_nic_access(&self) -> bool {`

```
// ── iwl_pcie_grab_nic_access + iwl_trans_pcie_read_mem ────────
// Grab NIC access (so device SRAM is reachable) and read `out.len()` words
// from device memory at `addr` through the HBUS periphery window (the read
// data register auto-increments). Used only by the error-log dump.
```

## L6267-6270 · `fn dump_fw_error_log(&self) {`

```
// ── iwl_mvm_dump_nic_error_log (mvm/utils.c) ──────────────────
// Read the lmac + umac error tables from device SRAM. valid != 0 means the
// firmware asserted; error_id classifies it and hcmd / last_cmd_id /
// cmd_header name the command the firmware faulted on.
```

## L6310-6312 · `fn stop(&self) {`

```
// Halt the firmware's DMA engines before the driver returns. The kernel
// frees our DMA buffers on return; a still-running chip must not DMA into
// them afterwards. sw_reset (CSR_RESET) stops the device.
```

## L6319 · `fn log_dma(name: &str, d: &Dma) {`

```
/// Log a DMA allocation's physical address (Stage 1 diagnostics).
```

## L6327-6330 · `fn fw_cmd_ver(group: u8, cmd: u8) -> u8 {`

```
/// iwl_fw_lookup_cmd_ver (fw/img.c): walk the embedded firmware's
/// IWL_UCODE_TLV_CMD_VERSIONS TLV for the (group, cmd) entry and return its
/// cmd_ver. Group 0 maps to LONG_GROUP (the legacy command space). Returns
/// IWL_FW_CMD_VER_UNKNOWN (99) if absent — callers fall back to a default.
```

## L6346 · `return FW[e + 2]; // cmd_ver (may be IWL_FW_CMD_VER_UNKNOWN)`

```
// cmd_ver (may be IWL_FW_CMD_VER_UNKNOWN)
```

## L6355-6358 · `fn fw_has_capa(cap: u32) -> bool {`

```
/// fw_has_capa (iwl-drv.c iwl_set_ucode_capabilities): true if the firmware's
/// IWL_UCODE_TLV_ENABLED_CAPABILITIES TLVs set capability bit `cap`. Each such
/// TLV is { __le32 api_index; __le32 api_capa }; bit `cap` lives in the TLV with
/// api_index == cap/32, at position cap%32 in api_capa.
```

## L6380 · `fn log_cmd_ver(name: &str, group: u8, cmd: u8) {`

```
/// Log one command's firmware version (Stage 4d1 diagnostics).
```

## L6389-6392 · `fn mac_from_regs(addr0: u32, addr1: u32) -> [u8; 6] {`

```
/// iwl_flip_hw_address: build the 6-byte MAC from the two CSR registers.
/// addr0 holds bytes [3,2,1,0] (high→low), addr1 holds bytes [4,5] in its low
/// half (byte1, byte0). On a little-endian host iwl_read32 + cpu_to_le32 leaves
/// the register value with byte k at (val >> 8*k).
```

## L6404 · `fn is_valid_mac(mac: &[u8; 6]) -> bool {`

```
/// is_valid_ether_addr: not multicast (bit 0 of first octet clear) and not all-zero.
```

## L6409 · `fn is_hw_error_value(val: u32) -> bool {`

```
/// iwl_trans_is_hw_error_value (iwl-trans.h).
```

## L6414 · `fn pci_read16(off: u8) -> u16 {`

```
/// Read a 16-bit value from PCI config space (dword read + extract).
```

## L6420-6421 · `fn pcie_find_cap(id: u8) -> u8 {`

```
/// Walk the PCI capability list for the given capability ID. Returns the
/// config-space offset of the capability, or 0 if absent.
```

## L6436 · `fn cfg_trim(v: &[u8]) -> &[u8] {`

```
/// Strip spaces, tabs and CR from both ends of a config token.
```

## L6444-6445 · `fn cfg_get<'a>(text: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {`

```
/// Value of `key` in a `key: value` config, or None. `#` starts a comment;
/// only the first colon splits, so a value may contain more of them.
```

## L6462-6463 · `fn cfg_u16(v: Option<&[u8]>, dflt: u16, max: u16) -> u16 {`

```
/// A plain number from the config, clamped. 0 (and anything unparsable) means
/// "not set" — the caller decides what that implies.
```

## L6485 · `fn settle_ms_config() -> u32 {`

```
/// The settle pause, read before the driver struct exists.
```

## L6494-6503 · `let settle = settle_ms_config();`

```
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
```

## L6506 · `host::sleep_ms(settle); // no ring allocated yet — sleeping is safe here`

```
// no ring allocated yet — sleeping is safe here
```

## L6513 · `let rc = host::pci_bind(AX200_VENDOR, AX200_DEVICE);`

```
// ── Stage 0a: bind, bus master, map BAR0, identity ───────────
```

## L6639 · `if rf_id & 0x0FFF_F000 == CSR_HW_RF_ID_TYPE_HR & 0x0FFF_F000 {`

```
// RF type lives in the high nibble pattern; HR is the AX200's radio.
```

## L6646 · `if !dev.start_hw() {`

```
// ── Stage 0b: reset + APM bring-up ───────────────────────────
```

## L6653 · `if !dev.nic_init() {`

```
// ── Stage 1: RX/TX rings + command queue ─────────────────────
```

## L6665 · `if dev.load_firmware() {`

```
// ── Stage 2: context-info + FW self-load + ALIVE ─────────────
```

## L6669 · `match dev.rx_restock_and_alive() {`

```
// ── Stage 3: RX restock + read the ALIVE notification ────
```

## L6674 · `if dev.parse_alive_ntf(&rb0) {`

```
// ── Stage 4a: parse the ALIVE notification struct ──
```

## L6678 · `if dev.run_init_handshake() {`

```
// ── Stage 4b: init-flow host commands → INIT_COMPLETE ──
```

## L6682 · `if dev.read_nvm() {`

```
// ── Stage 4c: read NVM info (caps + MAC address) ──
```

## L6686 · `host::dprint("[ax200] FW cmd versions (scan path):\n");`

```
// ── Stage 4d1: firmware command versions (scan path) ──
```

## L6697-6699 · `dev.load_connect_policy();`

```
// Which network, which band, radio power — read
// before the prerequisites, because POWER_TABLE_CMD
// goes out in there.
```

## L6702-6715 · `dev.run_scan_prereqs();`

```
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
```

## L6719-6720 · `dev.add_mac_context();`

```
// ── Stage 4d2b1b: add the MAC context the scan ──
// references (scan_start_mac_or_link_id → ctx id 0).
```

## L6724-6725 · `if dev.run_scan() {`

```
// ── Stage 4d2b1/2: passive scan → SCAN_COMPLETE,
// parse beacons → access points (SSID/BSSID/RSSI). ──
```

## L6729-6732 · `let mut associated = false;`

```
// ── Stage 5a: PHY context + RLC + binding ──
// First connect step (no TX yet): set the target
// AP's operating channel + bind MAC↔PHY. Target =
// strongest AP from the scan.
```

## L6737-6738 · `dev.connect_finish_chanctx();`

```
// ── Stage 5b': power + MAC context (target BSSID) ──
// The chanctx tail Linux runs before the auth TX.
```

## L6740-6743 · `if !dev.alloc_data_queue() {`

```
// Allocate the data TX queue BEFORE auth so its
// SCD-response wait doesn't discard the AP's
// first EAPOL frame, and the post-assoc listen
// can begin immediately.
```

## L6745-6748 · `host::print("[ax200] FATAL: data TX queue DMA alloc failed — no 4-way, no traffic\n");`

```
// Without this queue there is no path
// for EAPOL, so the 4-way cannot run and
// the link can never authorize. `dprint`
// was the wrong channel for that.
```

## L6751-6755 · `for attempt in 0..3 {`

```
// ── Stage 5c/5d: AUTH → ASSOC mgmt dialog ──
// Retry the whole auth+assoc up to 3× like
// mac80211 (IEEE80211_AUTH_MAX_TRIES /
// ASSOC_MAX_TRIES): a single lost mgmt frame
// must not abort the connect.
```

## L6769-6775 · `dev.run_netdev(associated);`

```
// ── Register as a NIC + go resident ──
// run_netdev never returns: the driver owns the
// card and the `wlan` interface for its lifetime
// (same model as aml.wasm). It only tells wifid the
// link is READY (→ the 4-way) when we actually
// associated — otherwise wifid would arm a
// supplicant for a BSS we never joined and stall.
```

## L6796-6800 · `dev.stop();`

```
// Halt the chip before returning: the kernel frees our DMA buffers on
// return and a still-running firmware must not DMA into them afterwards.
// (Probe-stage driver — we return rather than idle; npk_input_wait HLTs
// without yielding, which would pin the core. A persistent yielding
// run-loop arrives with Stage 4+.)
```

