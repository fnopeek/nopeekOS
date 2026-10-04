# `kernel/src/microvm/devices/virtio_gpu_pci.rs` @ 5e0102684

## L1-19 · `#![allow(dead_code)]`

```
//! virtio-gpu-pci device emulation (Phase 12.4).
//!
//! Modern virtio (1.0+) GPU device — vendor 0x1AF4, device 0x1050,
//! class 0x03_80_00 (Display controller / Other). Two virtqueues:
//!   q0 = controlq (resource/scanout commands)
//!   q1 = cursorq  (cursor updates — acknowledged but ignored)
//!
//! 12.4.0 scope: 2D scanout end-to-end. We accept:
//!   * GET_DISPLAY_INFO → report one 1280×720 display
//!   * RESOURCE_CREATE_2D → track resource (id, fmt, w, h)
//!   * RESOURCE_ATTACH_BACKING → record guest page list per resource
//!   * SET_SCANOUT → bind resource to scanout 0
//!   * TRANSFER_TO_HOST_2D → copy guest pages → host-side resource buffer
//!   * RESOURCE_FLUSH → log + (eventually) composite into shade
//!   * RESOURCE_UNREF / DETACH_BACKING → drop resource
//!   * Cursor cmds → respond OK_NODATA, no rendering
//!
//! Out of scope here: virgl/3D, blob resources, EDID, multi-display.
//! We advertise 0 capsets so Linux skips virgl probing entirely.
```

## L28 · `const DMG_LOG_EVERY: u32 = 120;`

```
/// How often the [gpu-dmg] coverage line is logged (every N flushes).
```

## L58 · `const CC_DEVICE_FEATURE_SELECT:  u32 = 0x00;`

```
// Common Cfg register offsets (same as virtio-blk / virtio-net)
```

## L79 · `const DC_EVENTS_READ:   u32 = 0x00;`

```
// virtio-gpu device-cfg layout (struct virtio_gpu_config)
```

## L85-87 · `const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1 << 0;`

```
/// `events_read` bit: the display configuration changed → the guest
/// must re-issue GET_DISPLAY_INFO. Raised by `signal_display_change`
/// when Shade resizes the tile (D4 live-resize round-trip).
```

## L90-97 · `const VIRTIO_GPU_F_EDID: u32 = 1 << 1;`

```
/// virtio-gpu feature bit. We advertise EDID so the guest's
/// virtio_gpu_config_changed_work_func also calls
/// virtio_gpu_cmd_get_edids on every DISPLAY event. The EDID callback
/// in turn calls drm_kms_helper_hotplug_event() **unconditionally** —
/// where drm_helper_hpd_irq_event would NOT, because the connector
/// stays "connected" across a tile resize. That's the only path that
/// makes wlroots/cage actually rescan the output and propagate a new
/// xdg_surface.configure to the client (LibreWolf etc.).
```

## L100 · `const VIRTIO_GPU_CMD_GET_DISPLAY_INFO:        u32 = 0x0100;`

```
// virtio-gpu protocol command/response types
```

## L123 · `const DISPLAY_W: u32 = 1280;`

```
// Display geometry advertised in GET_DISPLAY_INFO
```

## L128-132 · `const MAX_QUEUE_SIZE: u16 = 16;`

```
/// Small on purpose: the guest's virtio-gpu driver creates no fence for a
/// dumb primary buffer and emulates no vblank, so the ONLY thing that paces
/// its compositor is a full controlq (`virtio_gpu_queue_ctrl_sgs` sleeps
/// for space). 16 descriptors ≈ two frames (TRANSFER + SET_SCANOUT + FLUSH,
/// two descriptors each) — at 64 it rendered ~10 frames ahead.
```

## L134 · `const VBLANK_HZ: u64 = 60;`

```
/// The display refresh the controlq is paced to (see `service_controlq`).
```

## L136 · `const MAX_SCANOUTS: usize = 16;   // protocol max`

```
// protocol max
```

## L156-157 · `#[derive(Clone)]`

```
/// Per-resource state. Tracks what the guest has allocated + the page
/// list that backs the resource's pixel data in guest physical memory.
```

## L164-165 · `backing: Vec<(u64, u32)>,`

```
/// Guest-physical pages backing this resource. Filled in by
/// RESOURCE_ATTACH_BACKING, drained by RESOURCE_DETACH_BACKING.
```

## L167-168 · `host_pixels: Option<Vec<u8>>,`

```
/// Host-side pixel buffer. Updated by TRANSFER_TO_HOST_2D. Layout
/// matches `format` × `width` × `height`. None until first transfer.
```

## L170 · `frame: u64,`

```
/// Bumped by every TRANSFER into `host_pixels`.
```

## L172 · `flushed_frame: u64,`

```
/// `frame` at this resource's last FLUSH to the surface.
```

## L176-177 · `#[derive(Default, Clone, Copy)]`

```
/// Per-scanout binding. We advertise 1 scanout (id 0); ids 1..16 stay
/// disabled.
```

## L205-206 · `events_read: u32,`

```
/// virtio-gpu device-cfg `events_read`. Sticky until the guest
/// clears the bits via `events_clear`. See `VIRTIO_GPU_EVENT_*`.
```

## L210-212 · `resources: Vec<Resource>,`

```
/// Resource table. Keyed by resource_id linearly — virtio-gpu
/// resource IDs are arbitrary u32, but Linux typically starts at 1
/// and increments. We store unsorted; lookup is by id.
```

## L215 · `scanouts: [Scanout; MAX_SCANOUTS],`

```
/// Per-scanout bindings.
```

## L218-229 · `d4_disconnect_until: Option<u64>,`

```
/// D4 disconnect/reconnect state. wlroots/cage only ever picks an
/// output mode at connector-up time — a "preferred mode changed
/// while still connected" hotplug uevent is silently ignored. To
/// force a real mode-set on tile resize we synthesize a connector
/// disconnect (GET_DISPLAY_INFO reports enabled=0) immediately,
/// then a reconnect ~100 ms later with the new geometry + fresh
/// EDID. The compositor tears down the output on disconnect,
/// brings it back up on reconnect, and picks our new preferred
/// mode by spec.
///
/// `Some(tick)` means we're in the disconnect window; reconnect
/// fires once `interrupts::ticks() >= tick`.
```

## L232-233 · `flush_log_count: u32,`

```
/// First N flush events get a verbose log; afterwards we silently
/// keep updating to avoid swamping the serial console.
```

## L235-237 · `transfer_log_count: u32,`

```
/// Same throttling for TRANSFER_TO_HOST_2D — fbcon issues one per
/// console-line update which is a full 3.6 MB blit; logging each
/// via kprintln stalls the guest for tens of seconds.
```

## L239-242 · `set_scanout_log_count: u32,`

```
/// Same for SET_SCANOUT — a wlroots/cage compositor double-buffers
/// by flipping the scanout between two resources every frame
/// (res 3 ↔ 4 at the guest's refresh rate). Logging each floods
/// the loop terminal forever; first 5 are enough to confirm setup.
```

## L245-248 · `dmg_area_acc: u64,`

```
/// [gpu-dmg] instrumentation: rolling sum of FLUSH damage-rect area vs
/// tile area, logged every `DMG_LOG_EVERY` flushes so we can see on HW
/// whether the guest sends tight damage rects (→ damage-clipped MMIO
/// blit is a big 4K win) or dirties the whole scanout (→ no win).
```

## L253-255 · `paused_until: u64,`

```
/// vblank pacing: the controlq takes no further command before this TSC
/// once a FLUSH was served (0 = not paused). What a display does: one
/// scan-out per refresh, the rest waits.
```

## L257 · `last_flush_res: u32,`

```
/// Resource of the last FLUSH that reached the surface.
```

## L312-329 · `pub fn signal_display_change(&mut self, now: u64) {`

```
/// Shade resized the tile → start a D4 disconnect/reconnect cycle.
/// Phase 1 (this call): GET_DISPLAY_INFO will report the connector
/// as `enabled=0`, the guest sees `connector_status_disconnected`,
/// wlroots/cage tears down the output. Phase 2 (`tick_d4`, ~100 ms
/// later): GET_DISPLAY_INFO flips back to `enabled=1` with the new
/// W×H, the guest re-detects the connector, picks the preferred
/// mode from our fresh EDID, the compositor rebuilds the output
/// with the new size and propagates `xdg_surface.configure` to
/// the client.
///
/// Why two phases: wlroots only mode-sets at connector-up time.
/// A "preferred mode changed while still connected" hotplug uevent
/// (which our prior signal_display_change emitted) is silently
/// dropped. The disconnect/reconnect round-trip is the one path
/// every Wayland compositor honors because it's what real HW does.
///
/// Caller injects IRQ 9 right after this returns. Pass the current
/// host tick from `interrupts::ticks()`.
```

## L331 · `const DISCONNECT_TICKS: u64 = 10; // ~100 ms at 100 Hz host-timer`

```
// ~100 ms at 100 Hz host-timer
```

## L335 · `self.isr |= 0b10; // bit 1 = device configuration changed`

```
// bit 1 = device configuration changed
```

## L339-342 · `pub fn tick_d4(&mut self, now: u64) -> bool {`

```
/// Phase 2 of the D4 cycle: if a disconnect window is pending and
/// the reconnect tick has been reached, flip the connector back to
/// `enabled=1` and raise a fresh DISPLAY event. Returns `true` iff
/// the reconnect fired (caller injects IRQ 9 then).
```

## L357-360 · `pub fn d4_disconnecting(&self) -> bool {`

```
/// True while we're in the disconnect half of a D4 cycle. Used by
/// the vmx/svm run-loop to suppress fresh DISPLAY events until the
/// reconnect has fired (otherwise back-to-back resizes would queue
/// disconnects without the matching reconnect).
```

## L365 · `pub fn service_queues(&mut self, queue_idx: u16, mem: &GuestMem) -> bool {`

```
/// Process queue notify. q0 = controlq, q1 = cursorq.
```

## L374-377 · `fn service_controlq(&mut self, mem: &GuestMem) -> bool {`

```
/// controlq drain: walk avail-ring, for each chain head, gather the
/// driver-readable bytes into a command buffer, dispatch on the
/// command type, and write the response into the driver-writable
/// descriptor(s).
```

## L410-411 · `let mut request = Vec::with_capacity(256);`

```
// Walk chain, partition into request bytes (read-only) and
// response slots (writable).
```

## L413 · `let mut resp_descs: Vec<(u64, u32)> = Vec::new(); // (addr, len)`

```
// (addr, len)
```

## L430-431 · `flushed = request.len() >= 4`

```
// Dispatch the command + build the response payload (incl.
// the 24-byte virtio_gpu_ctrl_hdr).
```

## L437 · `let mut written: u32 = 0;`

```
// Write response across the writable descriptors in order.
```

## L455 · `let q = &mut self.queues[q_idx];`

```
// Commit local copies back to the queue struct.
```

## L460 · `if flushed {`

```
// A frame went out: the next command waits for the next refresh.
```

## L470-471 · `fn service_cursorq(&mut self, mem: &GuestMem) -> bool {`

```
/// cursorq drain: acknowledge UPDATE_CURSOR / MOVE_CURSOR without
/// rendering anything. Same chain shape as controlq.
```

## L500 · `let mut resp_descs: Vec<(u64, u32)> = Vec::new();`

```
// Walk chain, find first writable for the response.
```

## L532-534 · `fn dispatch_control(&mut self, request: &[u8], mem: &GuestMem) -> Vec<u8> {`

```
/// Dispatch one virtio-gpu command. `request` is the concatenated
/// read-only descriptor data — starts with a 24-byte ctrl_hdr,
/// followed by command-specific payload.
```

## L556-559 · `let (dw, dh) = crate::shade::surface::tile_size(`

```
// D4: report the bound window's content rect so the
// guest renders to the tile size (no host scaling).
// Falls back to the default if Shade hasn't placed the
// window yet / VM unbound (dev fullscreen path).
```

## L563-565 · `let enabled = !self.d4_disconnecting();`

```
// Disconnect half of a D4 cycle: report enabled=0 so
// the guest connector goes status_disconnected → the
// compositor tears down the output.
```

## L606-607 · `build_ctrl_hdr(VIRTIO_GPU_RESP_ERR_INVALID_PARAM, flags, fence_id)`

```
// We advertise num_capsets=0; Linux shouldn't ask. If
// it does, fail clean rather than corrupting.
```

## L611 · `if request.len() < 24 + 8 {`

```
// request body: scanout (u32) + padding (u32)
```

## L638 · `if let Some(r) = self.resources.iter_mut().find(|r| r.id == id) {`

```
// Replace if id exists, else push.
```

## L683-684 · `let n = self.set_scanout_log_count;`

```
// First 2 only (initial scanout binding); a wlroots compositor
// page-flips the scanout every frame — pure noise after setup.
```

## L723-725 · `let bpp: usize = 4;`

```
// Compute bytes per pixel from format. Virtio-gpu B8G8R8A8 etc.
// are all 4 bytes per pixel for the formats Linux's virtio-gpu
// driver uses. We're conservative and treat everything as 4 BPP.
```

## L730 · `let host_buf = r.host_pixels.get_or_insert_with(|| alloc::vec![0u8; total_pixels_bytes]);`

```
// Ensure host_pixels exists with the right size.
```

## L736-739 · `let row_bytes = w as usize * bpp;`

```
// Walk the rect row by row, copying from guest backing pages.
// The guest-side resource is laid out contiguously starting at
// `offset` (relative to the backing). We translate (row, col)
// → linear offset → page-walk.
```

## L748-754 · `super::gpu_backend::note(`

```
// The per-frame pixel-copy cost on the vCPU core — in CYCLES, not just
// bytes. The comment here has claimed to measure this for months while
// recording only the byte count, and bytes cannot answer the question:
// 179 MB/s is either 3 % of a core or 90 % of one depending entirely on
// how fast `copy_from_backing` walks the guest's backing pages. That
// difference is the difference between "the browser renders" and "the
// browser renders INSTEAD of running its network stack".
```

## L761-762 · `let n = self.transfer_log_count;`

```
// One-time bring-up confirmation; the pixel pipeline is proven
// (LibreWolf renders) so the per-frame churn is just noise.
```

## L785-786 · `if resource_id == last_res && r.frame == r.flushed_frame {`

```
// Same resource, nothing transferred since its last flush: the surface
// already shows exactly this. (A flip to the OTHER buffer must copy.)
```

## L794-797 · `let n = self.flush_log_count;`

```
// One-time "first guest frame reached the host" confirmation,
// no hex preview (it was always-zero bring-up debug + an
// expensive per-flush String build). The pixel pipeline is
// proven end-to-end; further flushes are silent.
```

## L807-814 · `let dmg_w = w.min(r.width.saturating_sub(x.min(r.width)));`

```
// Bridge to Shade. If the VM is bound to a Surface window
// (normal case), the just-flushed frame becomes that window's
// tile content — composited with z-order, the tiling
// invariant holds (never fullscreen). Fallback to the legacy
// fullscreen blit only if unbound (e.g. compositor not up).
// The FLUSH rect (x,y,w,h) is the guest's damage region for this
// present. Clamp it to the resource and forward it so the host
// blits only the changed pixels (4K win) instead of the whole tile.
```

## L818 · `self.dmg_area_acc += (dmg_w as u64) * (dmg_h as u64);`

```
// [gpu-dmg] rolling coverage: damage area vs tile area.
```

## L824-826 · `if pct < 95 {`

```
// HW confirmed the guest dirties 100% of the scanout every frame,
// so only shout when partial damage actually appears (a future
// guest/compositor that would make damage-clipping pay off).
```

## L850 · `0x04 => (0x0010 << 16) | 0x0007,                  // status + command`

```
// status + command
```

## L851 · `0x08 => (0x03_80_00 << 8) | 0x01,                 // class display/other rev1`

```
// class display/other rev1
```

## L857 · `0x2C => (0x0010 << 16) | 0x1AF4,`

```
// Subsystem vendor 0x1AF4 / device 0x0010 (display)
```

## L862 · `0x3C => 0x0000_0109,`

```
// Interrupt: line=9, pin=INTA. Distinct from blk (11) + net (10).
```

## L865 · `0x40 => 0x09 | ((CAP_NOTIFY_OFF as u32) << 8) | (16 << 16) | ((VIRTIO_PCI_CAP_COMMON_CFG as u32) << 24),`

```
// Modern virtio cap list (same shape as blk/net).
```

## L938-940 · `fn device_write(&mut self, off: u32, value: u64) {`

```
/// virtio-gpu device-cfg writes. Only `events_clear` is writable:
/// the guest's config-changed handler writes the bits it observed
/// in `events_read` to acknowledge them (write-1-to-clear).
```

## L953 · `1 // VIRTIO_F_VERSION_1 (bit 32 → bit 0 of upper half)`

```
// VIRTIO_F_VERSION_1 (bit 32 → bit 0 of upper half)
```

## L1037 · `b[DC_NUM_SCANOUTS as usize..DC_NUM_SCANOUTS as usize + 4]`

```
// events_clear reads back 0 (write-1-to-clear).
```

## L1062-1063 · `fn build_ctrl_hdr(resp_type: u32, flags: u32, fence_id: u64) -> Vec<u8> {`

```
/// Build a virtio_gpu_ctrl_hdr with the given response type. flags +
/// fence echo the request so the guest's fence-completion logic works.
```

## L1069 · `h`

```
// ctx_id, ring_idx, padding all 0
```

## L1073-1076 · `fn build_edid_resp(flags: u32, fence_id: u64, edid: &[u8]) -> Vec<u8> {`

```
/// Build a GET_EDID response: ctrl_hdr + size (le32) + padding (le32)
/// + edid bytes. Linux's `virtio_gpu_resp_edid` has a 1024-byte EDID
/// array; we only ship the 128 bytes we synthesize (size field tells
/// the guest how much is valid).
```

## L1083 · `h[32..32 + edid.len()].copy_from_slice(edid);`

```
// padding bytes 28..32 are zero
```

## L1088-1095 · `fn build_edid(width: u32, height: u32) -> [u8; 128] {`

```
/// Synthesize a minimal 128-byte EDID 1.4 block advertising exactly
/// one preferred display mode at `width × height @ 60 Hz`. This is the
/// "current tile size" Shade has placed the microvm window into.
///
/// Linux's virtio-gpu EDID callback calls drm_kms_helper_hotplug_event()
/// unconditionally after parsing — that's the uevent path wlroots
/// needs to rescan the output. Without EDID (or with a stale EDID
/// whose preferred-mode never changes), wlroots ignores the resize.
```

## L1099 · `e[0..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);`

```
// Header (8 bytes)
```

## L1102-1103 · `e[8] = 0x3A;`

```
// Manufacturer ID "NPK" — 5 bits each, big-endian, MSB=0.
// N=14, P=16, K=11 → (14<<10) | (16<<5) | 11 = 0x3A0B
```

## L1106 · `e[10] = 0x01;`

```
// Product code (LE)
```

## L1109-1110 · `e[16] = 0;`

```
// Serial number (LE) — leave zero
// Manufacturing week / year (year offset from 1990)
```

## L1112 · `e[17] = 36; // 2026`

```
// 2026
```

## L1114 · `e[18] = 1;`

```
// EDID version 1.4
```

## L1118 · `e[20] = 0xA5;`

```
// Video input: digital (0x80) | 8 bpc (0x20) | DisplayPort (0x05)
```

## L1120 · `e[21] = 0;`

```
// Max H/V image size (cm) — 0 = variable / projector style
```

## L1123 · `e[23] = 0x78;`

```
// Gamma 2.2 → (gamma*100)-100 = 120 = 0x78
```

## L1125-1126 · `e[24] = 0x06; // 0b00000110: sRGB default colorspace + preferred is native`

```
// Features: standby/suspend/off off; RGB; sRGB defaults; preferred
// timing has native pixel format + refresh; continuous frequency.
```

## L1127 · `e[24] = 0x06; // 0b00000110: sRGB default colorspace + preferred is native`

```
// 0b00000110: sRGB default colorspace + preferred is native
```

## L1129 · `e[25..35].copy_from_slice(`

```
// Color characteristics (10 bytes 25..35): standard sRGB primaries
```

## L1133-1135 · `for i in 0..8 {`

```
// Established timings 1/2/Manufacturer (3 bytes): none — we don't
// want the guest to pick any of them, only our DTD.
// Standard timings (16 bytes 38..54): unused (each 0x0101)
```

## L1141 · `write_cvt_dtd(&mut e[54..72], width, height);`

```
// First DTD (54..72) — the one mode we want the compositor to use.
```

## L1144 · `e[75] = 0xFC;`

```
// Second DTD (72..90) — monitor name "nopeekOS"
```

## L1150 · `e[77 + name.len()] = 0x0a; // terminator per VESA`

```
// terminator per VESA
```

## L1152 · `e[77 + i] = 0x20; // pad with spaces`

```
// pad with spaces
```

## L1155-1158 · `e[93] = 0xFD;`

```
// Third DTD (90..108) — display range limits descriptor (helps
// wlroots/Xorg decide the connector accepts arbitrary refresh).
// type=0xFD; min/max V Hz = 50/75; min/max H kHz = 30/120; max
// pixel clock = 300 MHz / 10 = 30 → 0x1E
```

## L1160 · `e[95] = 50;   // V min`

```
// V min
```

## L1161 · `e[96] = 75;   // V max`

```
// V max
```

## L1162 · `e[97] = 30;   // H min kHz`

```
// H min kHz
```

## L1163 · `e[98] = 120;  // H max kHz`

```
// H max kHz
```

## L1164 · `e[99] = 30;   // max pixel clock / 10 MHz`

```
// max pixel clock / 10 MHz
```

## L1165 · `e[100] = 0x01; // GTF standard (closest legacy default)`

```
// GTF standard (closest legacy default)
```

## L1167 · `e[111] = 0x10;`

```
// Fourth DTD (108..126) — unused descriptor (type 0x10 dummy)
```

## L1170 · `e[126] = 0;`

```
// Number of extensions
```

## L1173 · `let sum: u32 = e[..127].iter().map(|&b| b as u32).sum();`

```
// Checksum: makes the byte sum 0 mod 256
```

## L1180-1185 · `fn write_cvt_dtd(dtd: &mut [u8], w: u32, h: u32) {`

```
/// Write a CVT-Reduced-Blanking-style 18-byte Detailed Timing
/// Descriptor for `width × height @ 60 Hz`. Reduced blanking means
/// H blanking = 160 px total (rather than the legacy ~25 % overhead),
/// so the pixel clock stays sane for arbitrary modes our compositor
/// might pick. Linux's connector helpers parse this into a
/// drm_display_mode and `drm_set_preferred_mode` flag.
```

## L1187 · `let h_blank = 160u32;`

```
// CVT-RB constants
```

## L1191 · `let v_blank = 14u32;       // 4 front porch + 6 sync + 4 back porch`

```
// 4 front porch + 6 sync + 4 back porch
```

## L1198 · `let pclk_10khz = ((h_total as u64) * (v_total as u64) * 60 / 10_000) as u32;`

```
// Pixel clock in units of 10 kHz: H_total * V_total * 60 Hz / 10_000
```

## L1215-1216 · `dtd[11] = 0;`

```
// Upper-bits byte (sync offsets/widths above 255): all zero for
// our small numbers.
```

## L1219 · `dtd[12] = 0;`

```
// H/V image size in mm — leave zero (display size unknown).
```

## L1223 · `dtd[15] = 0;`

```
// H/V border
```

## L1226 · `dtd[17] = 0x1E;`

```
// Flags: digital separate sync, vsync polarity +, hsync polarity +
```

## L1230-1233 · `fn build_display_info_resp(flags: u32, fence_id: u64, disp_w: u32, disp_h: u32, enabled: bool) -> Vec<u8> {`

```
/// Build a GET_DISPLAY_INFO response: ctrl_hdr + array of 16
/// virtio_gpu_display_one. Only scanout 0 is reported; `enabled` is
/// driven by the D4 disconnect/reconnect state (false during the
/// disconnect half of a tile-resize round-trip).
```

## L1235-1239 · `let mut buf = alloc::vec![0u8; 24 + 16 * 24];`

```
// Per virtio-gpu spec: VIRTIO_GPU_MAX_SCANOUTS = 16.
// struct virtio_gpu_resp_display_info:
//   hdr (24)
//   pmodes[16]: each { r{x,y,w,h}: u32×4, enabled: u32, flags: u32 } = 24 bytes
// Total = 24 + 16×24 = 408 bytes
```

## L1246 · `buf[p0 +  0..p0 +  4].copy_from_slice(&0u32.to_le_bytes()); // x`

```
// x
```

## L1247 · `buf[p0 +  4..p0 +  8].copy_from_slice(&0u32.to_le_bytes()); // y`

```
// y
```

## L1251 · `buf[p0 + 20..p0 + 24].copy_from_slice(&0u32.to_le_bytes()); // flags`

```
// flags
```

## L1253 · `buf`

```
// pmodes[1..16] stay zero (disabled)
```

## L1257-1259 · `fn copy_from_backing(mem: &GuestMem, backing: &[(u64, u32)], src_lin: usize, dst: &mut [u8]) {`

```
/// Copy `dst.len()` bytes from a guest backing-page list, starting at
/// linear offset `src_lin` into the conceptual resource buffer. The
/// backing pages are an ordered list of (guest_phys_addr, len).
```

## L1293-1301 · `fn blit_to_host_fb(src_pixels: &[u8], src_w: u32, src_h: u32) {`

```
/// Blit `src_pixels` (BGRX 32bpp, stride = src_w * 4) into the host
/// framebuffer's front shadow buffer at (0, 0). Clipped to the host
/// fb's geometry. Caller's pixel format must match host fb's pixel
/// format — virtio-gpu uses B8G8R8X8_UNORM (format=2) which matches
/// UEFI GOP's typical BGRX layout, so a byte-for-byte row-copy works.
///
/// SAFETY: writes through `framebuffer::cached_shadow_front()`. The
/// pointer is valid as long as the FbConsole is initialized. We bound
/// every write to the host fb's declared dimensions.
```

