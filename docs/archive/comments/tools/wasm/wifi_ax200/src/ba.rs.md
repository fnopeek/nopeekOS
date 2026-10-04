# `tools/wasm/wifi_ax200/src/ba.rs` @ 5e0102684

## L1-24 · `use crate::host;`

```
//! RX block-ack reorder buffer — port of `iwl_mvm_reorder` (mvm/rxmq.c).
//!
//! An aggregating AP sends a whole A-MPDU and expects a single block ack, which
//! means frames may arrive with holes: MPDU 5 can be on the air before the
//! retransmission of MPDU 3. Handing that to the IP stack as-is looks like
//! reordering to TCP and costs more than the aggregation gains, so a receiver
//! that accepts a block-ack session MUST hold frames back until the gap closes.
//! That buffer is this file, and it is the whole reason we declined ADDBA before.
//!
//! Differences from Linux, all because the firmware does the hard part: it hands
//! us the session id, this frame's sequence number and the "next expected"
//! (NSSN) in every descriptor, so there is no window arithmetic to derive. And
//! with one RX queue there is one buffer per session instead of one per queue.
//!
//! Frames are stored DECODED (Ethernet, as `rx_classify` produced them) — the
//! 802.11 header has done its job by then and the reorder decision needs only
//! the descriptor.
//!
//! One session PER TID, as Linux keeps them (`sta->ampdu_mlme.tid_rx[]`). A
//! single shared session was wrong in both directions: a second ADDBA silently
//! overwrote the first — leaking its firmware BAID, which we then could not
//! even name to free — and a DELBA for one TID tore down whichever session
//! happened to be in the slot. Measured on the device as `sessions 2` with one
//! live BAID.
```

## L29-39 · `pub const BA_WIN_MAX: usize = 256;`

```
/// Largest window we can address, in MPDUs. `IEEE80211_MAX_AMPDU_BUF_HE`, which
/// is also what iwlwifi reports as `hw->max_rx_aggregation_subframes` for this
/// family (`mvm/ops.c:1233`, the pre-BZ branch). The size actually used is
/// negotiated per session — see `Reorder::buf_size`.
///
/// It was 32 until 0.101.0, and that was the ceiling nothing else could lift.
/// Measured at 585 Mbit (VHT80): 32 outstanding MPDUs are through in 600 us, so
/// the per-aggregate overhead — preamble, block ack, AIFS, backoff — could not
/// be spread over enough frames. Airtime per received frame was 29.3 us against
/// 9.4 us at HT40, three times worse, while the medium sat half idle. A window
/// is a count, and a count divides by the rate.
```

## L42-51 · `pub const BA_POOL: usize = 256;`

```
/// Frames held at once, ACROSS all sessions. Storage is decoupled from the
/// WINDOW: the window says how far ahead of a hole the AP may run, storage only
/// has to cover the frames actually held while one stands open. Every device
/// measurement so far reports `held 0` — holes are rare and shallow, so eight
/// private windows of 32 were the wrong shape twice over.
///
/// 256 shared slots is the same 410 KB the eight fixed windows cost, with a
/// window eight times as wide. Running dry is not a loss: `store` declines and
/// the caller delivers the frame immediately, out of order, which is what the
/// stall release does anyway.
```

## L54 · `const BA_FRAME_MAX: usize = 1600;`

```
/// Longest frame we keep. `rx_classify` already caps decoded frames at 1600.
```

## L57-58 · `pub const NUM_TIDS: usize = 8;`

```
/// Block-ack is defined for TIDs 0-7; `IEEE80211_FIRST_TSPEC_TSID` is 8 and
/// Linux declines any ADDBA at or above it (`ieee80211_process_addba_request`).
```

## L61 · `const SN_MODULO: u16 = 1 << 12;`

```
/// Sequence numbers are 12-bit and wrap.
```

## L64 · `pub fn sn_less(a: u16, b: u16) -> bool {`

```
/// a < b in 12-bit sequence space (ieee80211_sn_less).
```

## L75-78 · `static mut POOL: [[u8; BA_FRAME_MAX]; BA_POOL] = [[0; BA_FRAME_MAX]; BA_POOL];`

```
/// Frame storage, shared by every session. Static rather than a field of the
/// driver struct: the driver lives on the stack in `_start`, and 400 KB of
/// buffer does not belong there. Zero-initialised .bss, so it costs nothing in
/// the module binary.
```

## L80-81 · `static mut POOL_LEN: [u16; BA_POOL] = [0; BA_POOL];`

```
/// Payload length per pool slot. 0 = free, and it is the only free-list we need:
/// a decoded Ethernet frame is never shorter than its header.
```

## L83-84 · `static mut POOL_CURSOR: usize = 0;`

```
/// Where the next search for a free slot starts. Turns the scan into O(1)
/// amortised without a second array to keep in step.
```

## L87-89 · `static mut SLOT_IDX: [[u16; BA_WIN_MAX]; NUM_TIDS] = [[0; BA_WIN_MAX]; NUM_TIDS];`

```
/// Window position -> pool slot, per TID. Holds `slot + 1` so that 0 means
/// empty: an array whose empty value is 0xffff would be 4 KB of non-zero bytes
/// dragged out of .bss and into the module image.
```

## L92-94 · `static mut POOL_FULL: u32 = 0;`

```
/// Times the pool ran dry and a frame went up out of order instead of being
/// held. Zero in a healthy run; anything else means the sessions are holding
/// more than `BA_POOL` frames at once and the window outgrew its storage.
```

## L98 · `unsafe { POOL_FULL }`

```
// SAFETY: single-threaded module.
```

## L102-103 · `pub fn pool_used() -> u32 {`

```
/// How many pool slots are held right now — the high-water mark is what says
/// whether `BA_POOL` is sized right.
```

## L105 · `unsafe {`

```
// SAFETY: single-threaded module.
```

## L112 · `fn pool_alloc() -> Option<usize> {`

```
/// Take a free pool slot, or None when every one is held.
```

## L114 · `unsafe {`

```
// SAFETY: single-threaded module; the pool is touched only from the RX path.
```

## L130-131 · `pub fn sessions() -> &'static mut [Reorder; NUM_TIDS] {`

```
/// All sessions. Free function rather than a driver field because the RX path
/// classifies frames inside a closure that cannot also borrow the driver.
```

## L133 · `unsafe { &mut *(&raw mut REORDER) }`

```
// SAFETY: single-threaded WASM module; no host call re-enters the RX path.
```

## L137 · `pub fn by_tid(tid: u8) -> Option<&'static mut Reorder> {`

```
/// The session for a TID, or None for a TID block-ack does not cover.
```

## L144-145 · `pub fn by_baid(baid: u8) -> Option<&'static mut Reorder> {`

```
/// The session the firmware stamped this BAID with. Frames and FRAME_RELEASE
/// notifications name the BAID, not the TID, so this is the RX-path lookup.
```

## L151 · `pub fn totals() -> (u32, u32, u32, u32, u32, u16) {`

```
/// Sum a counter across sessions — the report shows one aggregate line.
```

## L165 · `pub fn tick_all() {`

```
/// Release held frames on every session whose hole has stood too long.
```

## L172 · `#[derive(Clone, Copy)]`

```
/// One RX aggregation session, one per TID.
```

## L175 · `pub baid: u8,`

```
/// Firmware session id, or INVALID while no session is up.
```

## L178-180 · `pub dialog: u8,`

```
/// The AP's dialog token for this session. A repeat ADDBA carrying the SAME
/// token is a timeout update, not a new session — Linux answers it without
/// touching the session (`ieee80211_process_addba_request`).
```

## L182-184 · `pub timeout_tu: u16,`

```
/// Inactivity timeout from the ADDBA request, in TU (0 = none). Linux arms
/// a timer on it and tears the session down when it runs out, sending a
/// DELBA with WLAN_REASON_QSTA_TIMEOUT.
```

## L186 · `pub last_rx_ms: u64,`

```
/// `now_ms` of the last frame on this session, for that timeout.
```

## L188-190 · `pub buf_size: u16,`

```
/// Negotiated window for THIS session, in MPDUs — `tid_rx->buf_size` in
/// Linux, and what `iwl_mvm_reorder` indexes with (`sn % buf_size`). Never
/// larger than `BA_WIN_MAX`.
```

## L192 · `pub head_sn: u16,`

```
/// Next sequence number we expect to deliver.
```

## L195-196 · `pub valid: bool,`

```
/// False until the first in-window frame; a session that starts mid-burst
/// must not treat the frames already on the air as holes.
```

## L198 · `pub last_move_ms: u64,`

```
/// `now_ms` of the last delivery, for the stall release below.
```

## L200 · `pub delivered: u32,`

```
// Counters, all reported by `wlan`.
```

## L208-212 · `const STALL_MS: u64 = 60;`

```
/// A hole that never fills would park the window forever: the AP is supposed to
/// close it with a BAR, but a BAR that is itself lost leaves the buffer holding
/// frames the stack needs. Linux runs a per-session timer; we check the clock on
/// the pass that notices stored frames, which is the same guarantee without a
/// timer we do not have.
```

## L238 · `pub fn start(&mut self, baid: u8, tid: u8, ssn: u16, dialog: u8, timeout_tu: u16,`

```
/// Session accepted by the firmware: it answered with this id.
```

## L241-243 · `self.flush();`

```
// Flush FIRST, while `buf_size` still describes the window the held
// frames were stored in. Assigning the new one first would walk the
// wrong positions and leak every pool slot beyond it.
```

## L256-258 · `pub fn timed_out(&self, now_ms: u64) -> bool {`

```
/// Has the AP gone quiet on this session for longer than it asked for?
/// `sta_rx_agg_session_timer_expired`: the timer is reset by every frame,
/// and expiry means the session is stale. 1 TU = 1024 us.
```

## L265-266 · `pub fn stop(&mut self) {`

```
/// Session gone (DELBA, deauth, reassociation). Everything still held goes
/// up: out of order beats never.
```

## L273 · `fn flush(&mut self) {`

```
/// Deliver every stored frame regardless of holes.
```

## L287-289 · `let slot = match unsafe { SLOT_IDX[t][index] } {`

```
// SAFETY: single-threaded module; the pool and the index array are
// touched only here and in `store`, never across a host call that could
// re-enter.
```

## L317 · `unsafe {`

```
// SAFETY: as in emit_slot.
```

## L320-321 · `self.emit_slot(index);`

```
// Slot occupied by a frame a full window away — the window has
// outrun itself. Release the old one rather than lose it.
```

## L326-328 · `let slot = match pool_alloc() {`

```
// Storage is shared now, so it can genuinely run out. Declining is the
// right answer: the caller then delivers this frame straight away, out
// of order — the same trade the stall release makes.
```

## L332 · `unsafe { POOL_FULL = POOL_FULL.wrapping_add(1) };`

```
// SAFETY: single-threaded module.
```

## L337 · `unsafe {`

```
// SAFETY: as in emit_slot.
```

## L345-348 · `self.last_move_ms = host::now_ms();`

```
// The clock the stall release runs on starts when a hole appears —
// not on the last delivery. Refreshing it on every release would
// keep it from ever firing while traffic flows, which is exactly
// when a hole hurts.
```

## L356-358 · `pub fn release_upto(&mut self, nssn: u16) {`

```
/// Deliver everything below `nssn` (iwl_mvm_release_frames). Empty slots are
/// normal: NSSN moving past a sequence number means the firmware saw it, not
/// that we hold it.
```

## L360-364 · `let ahead = sn_less(self.head_sn, nssn);`

```
// A jump FORWARD wider than the window means everything held is below
// nssn — walk the slots once instead of stepping through up to 2047
// sequence numbers one at a time in the RX path. Only forward: NSSN can
// legitimately sit behind head_sn after a stall release, and treating
// that as a jump would drag the window backwards.
```

## L378 · `if unsafe { SLOT_IDX[t][index] } != 0 {`

```
// SAFETY: as in emit_slot.
```

## L388-389 · `pub fn on_frame_release(&mut self, nssn: u16) {`

```
/// A FRAME_RELEASE notification for our session: the firmware advanced the
/// window without giving us a frame (it saw the MPDUs on air).
```

## L394-395 · `pub fn tick(&mut self) {`

```
/// A hole that has stood for STALL_MS gives up its claim. Call once per poll
/// pass; it costs one clock read when frames are held and nothing otherwise.
```

## L404-405 · `let upto = sn_add(self.head_sn, self.buf_size);`

```
// Release the whole window: head_sn moves past everything we hold, so a
// late arrival is then correctly treated as old rather than re-buffered.
```

## L410-416 · `pub fn on_frame(&mut self, reorder: u32, status: u32, amsdu_last: bool, frame: &[u8]) -> bool {`

```
/// The decision, per received data frame. `true` = we took the frame (it is
/// buffered or dropped); `false` = caller delivers it now.
///
/// Mirrors iwl_mvm_reorder's order exactly: invalid BAID and non-session
/// frames pass through, duplicates and outdated frames are dropped, an
/// in-order frame with nothing held is delivered without touching the
/// buffer, and only a frame that actually sits ahead of a hole is stored.
```

## L422-423 · `self.last_rx_ms = host::now_ms();`

```
// Every frame on the session resets its inactivity timer, exactly as
// Linux does in `ieee80211_sta_reorder_release`'s caller.
```

## L430-433 · `if reorder & IWL_RX_MPDU_REORDER_BA_OLD_SN != 0 {`

```
// Do not start on a frame the firmware already considers old — that
// is the tail of a burst that began before the session. head_sn
// stays at the SSN the session was set up with; NSSN pulls it
// forward on the first release.
```

## L442 · `return true; // consumed = dropped`

```
// consumed = dropped
```

## L449-450 · `if self.stored == 0 && sn_less(sn, nssn) {`

```
// Nothing held and the firmware has already moved past this frame, or it
// is exactly the one we were waiting for: straight through.
```

## L465 · `return false; // oversized — better out of order than dropped`

```
// oversized — better out of order than dropped
```

## L468-469 · `if amsdu_last {`

```
// An A-MSDU's NSSN advances on its FIRST sub-frame, so acting on it
// before the last one arrives would release frames still in flight.
```

