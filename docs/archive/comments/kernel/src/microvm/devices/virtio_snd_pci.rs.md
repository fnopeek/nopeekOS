# `kernel/src/microvm/devices/virtio_snd_pci.rs` @ 5e0102684

## L1-13 · `#![allow(dead_code)]`

```
//! virtio-sound-pci device emulation (virtio 1.2 §5.14).
//!
//! Bridges guest audio (LibreWolf → ALSA/PipeWire → virtio_snd) to the
//! kernel audio mailbox (`crate::audio`) → audio_hda.wasm → HDA → speaker.
//! The guest decodes everything (YouTube/Opus/AAC); we just capture its PCM.
//!
//! vendor 0x1AF4, device 0x1059, class 04_01_00 (Audio). Four virtqueues
//! (spec-mandated): controlq(0), eventq(1), txq(2 = playback), rxq(3 =
//! capture). We service control + tx; event/rx are stubs (output-only).
//!
//! PCI cap chain / Common Cfg / MMIO machinery is the shared modern-virtio
//! pattern, identical to `virtio_net_pci.rs`. Only the device-cfg, the
//! control-queue protocol and the tx PCM path are sound-specific.
```

## L23 · `pub const BAR0_BASE: u64 = 0xFE01_8000;`

```
/// MMIO BAR0 — next 0x4000 window after virtio-9p (0xFE01_4000).
```

## L28-31 · `const IRQ_LINE: u8 = 14;`

```
/// IRQ line — its own. Shared with virtio-net's 10 it made every network
/// interrupt run the sound handler too, and that handler's ISR read is an MMIO
/// exit. Not 8: under `acpi=off` the guest's rtc_cmos claims it. 14 is the
/// legacy primary-IDE line, and the guest has no IDE.
```

## L54 · `const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00;`

```
// Common Cfg register offsets (virtio 1.2 §4.1.4.3).
```

## L75 · `const NUM_QUEUES: u16 = 4; // control, event, tx, rx (spec-mandated)`

```
// control, event, tx, rx (spec-mandated)
```

## L78-79 · `const R_PCM_INFO:       u32 = 0x0100;`

```
// ── virtio-sound protocol constants (§5.14.x) ────────────────────────────
// Request codes
```

## L88 · `const S_OK:       u32 = 0x8000;`

```
// Status codes (returned in virtio_snd_hdr.code)
```

## L93 · `const PCM_FMT_S16:   u64 = 1 << 5;  // VIRTIO_SND_PCM_FMT_S16`

```
// PCM format / rate / direction
```

## L94 · `const PCM_FMT_S16:   u64 = 1 << 5;  // VIRTIO_SND_PCM_FMT_S16`

```
// VIRTIO_SND_PCM_FMT_S16
```

## L95 · `const PCM_RATE_48000: u64 = 1 << 7; // VIRTIO_SND_PCM_RATE_48000`

```
// VIRTIO_SND_PCM_RATE_48000
```

## L98-115 · `const BYTES_PER_TICK: u64 = 192_000 / 100;`

```
/// PCM playback bytes per host 100 Hz tick at our advertised format
/// (48 kHz × 2 ch × 2 bytes = 192000 B/s ÷ 100 = 1920). Drives the wall-clock
/// pacing of tx-buffer completion in `service_tx`.
///
/// WHY wall-clock and not audio_hda's actual drain: the guest virtio-snd driver
/// is 100 % device-clocked — its hw_ptr, period wakeups and A/V delay ALL come
/// from when/how much the device completes tx buffers (sound/virtio/
/// virtio_pcm_msg.c: hw_ptr advances only in msg_complete; no guest timer). A
/// real device (QEMU virtio_snd_pcm_out_cb) completes each buffer as the real
/// audio sink consumes it, at the sink's regular small period, never starved.
/// Our sink (audio_hda.wasm) is a polled best-effort loop that drains in 43 ms
/// gulps and can be starved under browser load — pacing completion off its
/// `drained` counter (v0.222.37) made the guest's whole clock gulpy + starvable
/// → crackle + long buffering + pause-burst. The host HDA plays at exactly
/// 48 kHz, so a 192000 B/s wall-clock is a faithful, smooth, drain-independent
/// model of real consumption (this is the model that played 3 min of music
/// cleanly at v0.222.21). Drift vs the HDA crystal is ~0.01 % and bounded by the
/// free_space ceiling.
```

## L118-120 · `const HDA_RING_BYTES: u64 = 16_384;`

```
/// Fixed buffering downstream of the mailbox: audio_hda's HDA ring is 2 halves of
/// 2048 frames = 16384 bytes sitting between poll_mix and the speaker. Added to
/// the reported latency so the guest's A/V sync accounts for it too.
```

## L123-124 · `const PLAYBACK_BYTES_PER_SEC: u64 = 192_000;`

```
/// PCM playback rate in bytes/sec (48 kHz × 2 ch × 2 bytes). The TSC-based
/// real-time budget uses this directly.
```

## L127-132 · `const LEAD: u64 = 8_192;`

```
/// How far ahead of real-time playback we keep the mailbox filled (~43 ms). With
/// the budget paced to exact real time the mailbox hovered near empty (HW: buf
/// troughed at ~450 B = 2 ms); audio_hda refills in bursts (one worker-timer
/// period of drain at once, ~tens of ms), so a burst-pull against a near-empty
/// mailbox drained it to silence → residual stutter. This lead keeps a cushion;
/// it's reported to the guest as latency_bytes so A/V stays synced. Frame-aligned.
```

## L135-142 · `const MAX_FILL: u64 = 20_480;`

```
/// Hard cap on mailbox fill (~107 ms). The CPU TSC runs ~0.2 % faster than the
/// HDA codec crystal, so `bytes_completed` (TSC-paced) creeps ahead of what
/// audio_hda drains → the mailbox slowly fills. Without the lead, network feed
/// gaps drained it back down; WITH the lead it doesn't bottom out, so the drift
/// would accumulate until the mailbox is full and the `free_space` ceiling
/// throttles completion to the drain rate → the guest clock creeps slow ("the
/// speed bug came back after a while"). Capping the fill here bounds `buf` so the
/// drift is bled off continuously instead → real-time pace holds. Must be > LEAD.
```

## L145-147 · `const SND_DIAG: bool = true;`

```
/// Verbose lifecycle + rate diagnostics (host serial → run window). On while we
/// stabilise audio; strip once solid (cheap — lifecycle is a few lines/stream,
/// the rate heartbeat is throttled to ~2 s).
```

## L189 · `slot: i32,`

```
/// Audio mailbox slot held while the PCM stream is prepared/running.
```

## L193-202 · `play_start_tsc: u64,`

```
/// Wall-clock playback pacing. A real audio device returns each PCM buffer on
/// the used-ring only *after* it has been played; completing instantly
/// fast-forwards the guest's ALSA hw_ptr → underrun → cubeb error. We pace
/// completion off a smooth 192000 B/s clock derived from the TSC — NOT
/// `interrupts::ticks()`, whose 100 Hz counter is incremented by Core 0's
/// timer IRQ and runs slow when Core 0 is busy compositing the browser tile
/// under load (→ the guest's audio clock dragged "too slow / delayed", worse
/// at higher resolution). The TSC advances at a constant rate regardless of
/// interrupt load. `play_start_tsc` = TSC at PCM_START; `bytes_completed` =
/// PCM bytes returned to the guest since then.
```

## L206-211 · `period_bytes: u32,`

```
/// Guest's PCM period size in bytes (from SET_PARAMS). Sizes the pacing
/// cushion: the wall-clock budget may lead `bytes_completed` by at most two
/// periods. When the guest stops feeding (YouTube buffering / pause) while the
/// stream stays `started`, the budget would otherwise run far ahead and be
/// handed back as one instant burst on resume → hw_ptr jump → underrun →
/// cubeb error → "can't pause / silent reload". Clamping keeps resume paced.
```

## L259-260 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
/// Service a kicked queue. control(0) + tx(2) are real; event(1)/rx(3)
/// are output-only stubs.
```

## L269-273 · `pub fn playing(&self) -> bool { self.slot >= 0 && self.started }`

```
/// Periodic pump (from the VM run loop): keep draining tx as the mailbox
/// frees up, so playback paces even without a fresh queue-kick.
/// A stream is playing: buffers complete against the wall clock, so the
/// device needs service at a steady cadence even while the guest sleeps
/// (QEMU drives the same with its audio timer).
```

## L280 · `fn service_control(&mut self, mem: &GuestMem) -> bool {`

```
// ── control queue ────────────────────────────────────────────────────
```

## L296 · `let mut req = [0u8; 64];`

```
// Split the chain: readable bytes = request; writable segs = response.
```

## L317 · `let mut resp = [0u8; 64];`

```
// Build the response, write it across the writable segments.
```

## L339-341 · `fn process_control(&mut self, req: &[u8], resp: &mut [u8], mem: &GuestMem) -> usize {`

```
/// Process one control request into `resp`, return the response length.
/// Every response starts with a virtio_snd_hdr (le32 status). `mem` is needed
/// because RELEASE must flush the tx queue (return all pending I/O buffers).
```

## L346 · `let start = le32(req, 4);`

```
// virtio_snd_query_info { hdr; start_id; count; size }
```

## L352 · `if start == 0 && count >= 1 && size >= 27 {`

```
// We have exactly one stream (id 0), OUTPUT.
```

## L355 · `put32(resp, base + 8, (PCM_FMT_S16 & 0xFFFF_FFFF) as u32);   // formats lo`

```
// hda_fn_nid(4)=0, features(4)=0
```

## L356 · `put32(resp, base + 8, (PCM_FMT_S16 & 0xFFFF_FFFF) as u32);   // formats lo`

```
// formats lo
```

## L357 · `put32(resp, base + 12, (PCM_FMT_S16 >> 32) as u32);          // formats hi`

```
// formats hi
```

## L358 · `put32(resp, base + 16, (PCM_RATE_48000 & 0xFFFF_FFFF) as u32); // rates lo`

```
// rates lo
```

## L359 · `put32(resp, base + 20, (PCM_RATE_48000 >> 32) as u32);        // rates hi`

```
// rates hi
```

## L360 · `resp[base + 24] = D_OUTPUT;  // direction`

```
// direction
```

## L361 · `resp[base + 25] = 2;         // channels_min`

```
// channels_min
```

## L362 · `resp[base + 26] = 2;         // channels_max`

```
// channels_max
```

## L363 · `len += size; // advance by the guest's declared struct size`

```
// advance by the guest's declared struct size
```

## L368-370 · `self.period_bytes = le32(req, 12);`

```
// virtio_snd_pcm_set_params: hdr(4) stream_id(4) buffer_bytes(4)
// period_bytes(4) ... — remember the period to size the pacing
// cushion. We advertise only S16/48k/stereo, so just accept.
```

## L376-381 · `if self.slot < 0 { self.slot = crate::audio::open(); }`

```
// One OUTPUT stream only. If a slot is still held from a stream
// that wasn't cleanly released (cubeb hard-erroring and
// re-initialising after an underrun), reuse + clear it instead
// of leaking a second slot — four leaked re-inits exhaust the
// mailbox, open() returns -1, and every reinit ("OpenCubeb
// failed") is then permanently silent.
```

## L391 · `self.started = true;`

```
// Reset the playback clock so completion pacing starts now.
```

## L405-413 · `self.started = false;`

```
// Spec (§5.14.6.6.5.1): "upon receipt of the RELEASE command the
// device MUST complete all pending I/O messages for the stream."
// The guest's sync_stop() then waits for the tx queue to drain
// (virtsnd_pcm_msg_pending_num == 0) before it considers the
// stream released. Pacing always leaves a few tx buffers un-
// completed in the avail ring, so without this flush the guest's
// wait TIMES OUT ("failed to flush I/O queue") and the next stream
// (resume after pause / page reload) starts on an inconsistent
// queue → permanent silence. Flush BEFORE closing the slot.
```

## L421 · `R_JACK_INFO | R_CHMAP_INFO => { put32(resp, 0, S_OK); 4 }`

```
// jacks=0 / chmaps=0 → guest shouldn't query these, but answer safely.
```

## L427-432 · `fn flush_tx(&mut self, mem: &GuestMem) -> u32 {`

```
/// Complete (return on the used ring) EVERY pending tx buffer immediately,
/// ignoring pacing. Required on PCM_RELEASE: the guest's sync_stop() blocks
/// until the tx queue is empty, so any buffer left in the avail ring hangs
/// the teardown (and corrupts the next stream). We discard the un-played PCM
/// (the stream is ending) — the guest just needs the buffers handed back +
/// an IRQ so its tx callback decrements msg_count → wakes msg_empty.
```

## L445 · `let mut idx = head;`

```
// Find the writable status desc in the chain and ack it S_OK.
```

## L469-470 · `fn service_tx(&mut self, mem: &GuestMem) -> bool {`

```
// ── tx (playback) queue ──────────────────────────────────────────────
// Chain: [virtio_snd_pcm_xfer {le32 stream_id}] [PCM data...] [virtio_snd_pcm_status {le32 status; le32 latency} writable]
```

## L472-475 · `if self.slot < 0 || !self.started { return false; }`

```
// Per virtio-sound: do NOT consume tx buffers until PCM_START. cubeb
// pre-fills the tx queue before START; completing that pre-roll early
// advances the guest's ALSA hw_ptr before playback begins -> underrun
// -> cubeb errors right at START. Hold the pre-roll until started.
```

## L491-495 · `let tsc_now = crate::interrupts::rdtsc();`

```
// Real-time playback budget from the TSC (constant rate, immune to
// interrupt-load jitter — `interrupts::ticks()` runs slow when Core 0 is
// busy compositing under load, which dragged the guest's audio clock too
// slow). Buffers complete only up to this watermark, so the guest sees an
// honest real-time sink instead of an instant drain.
```

## L498-499 · `let position = ((tsc_now.wrapping_sub(self.play_start_tsc) as u128)`

```
// Real-time playback position (bytes that should have reached the speaker
// by now), from the TSC.
```

## L503-505 · `let mut budget = position.saturating_add(LEAD);`

```
// Complete up to LEAD ahead of real playback so the mailbox keeps a
// cushion against audio_hda's bursty poll-pulls (else it momentarily
// empties → silence → brief dropout, seen on YouTube + webradio).
```

## L508-511 · `let period = (self.period_bytes as u64).max(BYTES_PER_TICK);`

```
// Cushion clamp: while the guest stopped feeding (buffering / pause) the
// budget ran ahead of what we completed; handing that back as one burst on
// resume fast-forwards the guest's hw_ptr → underrun. Re-anchor the TSC
// clock so the lead never exceeds LEAD + two periods of catch-up.
```

## L516 · `let pos_cycles = ((target.wrapping_sub(LEAD) as u128).saturating_mul(tsc_hz as u128)`

```
// Re-anchor so `position` (= target - LEAD) maps to `tsc_now`.
```

## L523-525 · `if SND_DIAG {`

```
// Rate heartbeat (~2 s): actual bytes completed vs the 192000 B/s target
// — confirms the guest's clock runs real-time. Also prints ticks() so we
// can see how far the IRQ-counter lagged the TSC under load.
```

## L540-541 · `let mut pcm_segs: [(u64, u32); 16] = [(0, 0); 16];`

```
// Pass 1: walk the chain, collect readable PCM segments + the status
// desc. 16 segs covers any guest period (≤80 ms = ≤5 pages) with margin.
```

## L546 · `let mut readable_seen = 0usize; // bytes of readable seen (first 4 = stream_id)`

```
// bytes of readable seen (first 4 = stream_id)
```

## L553 · `let mut a = d.addr;`

```
// Skip the 4-byte xfer header that prefixes the readable data.
```

## L570-572 · `if self.bytes_completed.wrapping_add(pcm_len as u64) > budget {`

```
// Wall-clock floor: hold this buffer until the 48 kHz clock has
// advanced past it — completing early fast-forwards the guest's
// hw_ptr → underrun → cubeb abort.
```

## L574 · `break; // too early; retry next pump tick`

```
// too early; retry next pump tick
```

## L576-579 · `if (crate::audio::buffered(slot) as u64).wrapping_add(pcm_len as u64) > MAX_FILL {`

```
// Fill cap: bound mailbox occupancy at MAX_FILL so the TSC-vs-HDA
// crystal drift can't slowly fill it to the brim (→ free_space
// throttle → guest clock creeps slow). Also guarantees room to submit
// the whole buffer (MAX_FILL ≪ SLOT_BYTES), so no partial-submit drop.
```

## L584 · `for s in 0..pcm_seg_n {`

```
// Submit the PCM into the mailbox in ≤4 KB chunks.
```

## L595-599 · `if status_addr != 0 {`

```
// Write the status response (S_OK + real latency) + complete the
// buffer. latency_bytes = PCM received-but-not-yet-played = what is
// currently buffered in the mailbox plus audio_hda's HDA ring. Linux
// feeds this into runtime->delay (sound/virtio/virtio_pcm_msg.c) →
// snd_pcm_delay → cubeb's A/V clock accounts for our hidden buffer.
```

## L619 · `pub fn pci_read_dword(&self, reg: u8) -> u32 {`

```
// ── PCI config space ─────────────────────────────────────────────────
```

## L623 · `0x04 => (0x0010 << 16) | 0x0007,            // status(cap-list) | cmd(mem+busmaster+io)`

```
// status(cap-list) | cmd(mem+busmaster+io)
```

## L624 · `0x08 => (0x04_01_00 << 8) | 0x01,           // class 04_01_00 (Audio) | rev 1`

```
// class 04_01_00 (Audio) | rev 1
```

## L629 · `0x2C => (0x0001 << 16) | 0x1AF4,            // subsystem`

```
// subsystem
```

## L633 · `0x3C => (0x01 << 8) | IRQ_LINE as u32,      // INTA`

```
// INTA
```

## L675 · `pub fn mmio_read(&mut self, off: u32, width: u8) -> u64 {`

```
// ── MMIO BAR0 ────────────────────────────────────────────────────────
```

## L703 · `CC_DEVICE_FEATURE => if self.device_feature_select == 1 { 1 } else { 0 }, // only VIRTIO_F_VERSION_1`

```
// only VIRTIO_F_VERSION_1
```

## L736 · `for q in self.queues.iter_mut() {`

```
// Reset.
```

## L767 · `fn device_read(&self, off: u32, width: u8) -> u64 {`

```
/// Device config (§5.14.4): jacks(u32) streams(u32) chmaps(u32).
```

## L770 · `put32(&mut buf, 0, 0); // jacks = 0`

```
// jacks = 0
```

## L771 · `put32(&mut buf, 4, 1); // streams = 1 (one OUTPUT PCM stream)`

```
// streams = 1 (one OUTPUT PCM stream)
```

## L772 · `put32(&mut buf, 8, 0); // chmaps = 0`

```
// chmaps = 0
```

