# `kernel/src/gpu/intel_xe.rs` @ 5e0102684

## L1-6 · `#![allow(dead_code)]`

```
//! Intel Xe Display Driver (Gen 12.2 / Alder Lake)
//!
//! Native modesetting for Intel UHD Graphics on Alder Lake-N (N100).
//! Display-only: no 3D, no compute, no GuC firmware.
//!
//! Reference: Intel Open Source PRM, Volume 12: Display Engine (Gen 12)
```

## L14 · `const INTEL_VENDOR: u16 = 0x8086;`

```
// ── PCI Device IDs ──────────────────────────────────────────────────
```

## L22-26 · `(0x9A78, "Tiger Lake-LP GT2 (blit-only)"),`

```
// Tiger Lake-LP GT2 (Iris Xe, Gen12.1) — same display register layout as
// ADL-N (Gen12.2). Activated blit-only (no PLL modeset, see `init`): we
// keep the firmware's live mode and only attach the BCS engine, so the
// CPU stops blitting to the slow UC framebuffer. Validated path; the 4K
// modeset stays ADL-N-only until tested on TGL.
```

## L30-31 · `fn is_adln(device_id: u16) -> bool {`

```
/// ADL-N GT1 — the only gen whose full PLL/modeset path is validated. Other
/// known Gen12 GPUs (e.g. Tiger Lake) take the blit-only route in `init`.
```

## L36 · `const PWR_WELL_CTL2: u32       = 0x45404;`

```
// ── MMIO Register Offsets (from BAR0) ───────────────────────────────
```

## L38 · `const PWR_WELL_CTL2: u32       = 0x45404;`

```
// Power well management
```

## L43 · `const CDCLK_CTL: u32           = 0x46000;`

```
// Core display clock
```

## L47 · `const DPLL_ENABLE_0: u32       = 0x46010;`

```
// DPLL (Display PLL) — TGL/ADL offsets (NOT ICL!)
```

## L50 · `const DPLL_CFGCR0_0: u32      = 0x164284;  // TGL/ADL DPLL0`

```
// TGL/ADL DPLL0
```

## L52 · `const DPLL_CFGCR0_1: u32      = 0x16428C;  // TGL/ADL DPLL1`

```
// TGL/ADL DPLL1
```

## L55 · `const TRANS_HTOTAL_A: u32      = 0x60000;`

```
// Transcoder A timing
```

## L65 · `const PIPE_CONF_A: u32         = 0x70008;  // Pipe enable/disable`

```
// Pipe A
```

## L66 · `const PIPE_CONF_A: u32         = 0x70008;  // Pipe enable/disable`

```
// Pipe enable/disable
```

## L68 · `const PIPE_FRMCNT_A: u32      = 0x70044;  // Frame counter (increments at vblank)`

```
// Frame counter (increments at vblank)
```

## L70 · `const PS_CTRL_1A: u32         = 0x68180;  // Scaler 1 control (bit 31 = enable)`

```
// Pipe Scaler (PS) — firmware may use this to scale 1080p on 4K monitors
```

## L71 · `const PS_CTRL_1A: u32         = 0x68180;  // Scaler 1 control (bit 31 = enable)`

```
// Scaler 1 control (bit 31 = enable)
```

## L72 · `const PS_WIN_POS_1A: u32      = 0x68170;  // Scaler 1 window position`

```
// Scaler 1 window position
```

## L73 · `const PS_WIN_SZ_1A: u32       = 0x68174;  // Scaler 1 window size`

```
// Scaler 1 window size
```

## L75 · `const PLANE_CTL_1_A: u32      = 0x70180;`

```
// Plane 1 on Pipe A
```

## L82 · `const DDI_BUF_CTL_A: u32      = 0x64000;`

```
// DDI
```

## L86 · `const ICL_DPCLKA_CFGCR0: u32  = 0x164280;`

```
// DDI Clock routing (ICL+) — routes DPLL to DDI/PHY (separate from TRANS_CLK_SEL!)
```

## L89-91 · `const ICL_PORT_TX_DW2_GRP_B: u32 = 0x6CD08;`

```
// Combo PHY TX registers (for voltage swing / signal integrity)
// PHY A base = 0x162000, PHY B base = 0x6C000
// GRP = group write (all 4 lanes), LN0 = lane 0 (read)
```

## L101 · `const GMBUS0: u32              = 0xC5100;  // Clock/Port Select (Gen 9+: 0xC5100)`

```
// GMBUS (I2C controller for DDC/SCDC)
```

## L102 · `const GMBUS0: u32              = 0xC5100;  // Clock/Port Select (Gen 9+: 0xC5100)`

```
// Clock/Port Select (Gen 9+: 0xC5100)
```

## L103 · `const GMBUS1: u32              = 0xC5104;  // Command/Status`

```
// Command/Status
```

## L104 · `const GMBUS2: u32              = 0xC5108;  // Status`

```
// Status
```

## L105 · `const GMBUS3: u32              = 0xC510C;  // Data`

```
// Data
```

## L106 · `const GMBUS4: u32              = 0xC5110;  // Interrupt mask`

```
// Interrupt mask
```

## L107 · `const GMBUS5: u32              = 0xC5120;  // 2-byte index register`

```
// 2-byte index register
```

## L109 · `const GMBUS_PIN_DPB: u32       = 0x02;    // DDI-B (HDMI) — i915: GMBUS_PIN_2_BXT = "dpb"`

```
// GMBUS0 pin pair select (ICP/TGP/ADL combo PHY, from gmbus_pins_icp[])
```

## L110 · `const GMBUS_PIN_DPB: u32       = 0x02;    // DDI-B (HDMI) — i915: GMBUS_PIN_2_BXT = "dpb"`

```
// DDI-B (HDMI) — i915: GMBUS_PIN_2_BXT = "dpb"
```

## L112 · `const GMBUS_SW_CLR_INT: u32    = 1 << 31;`

```
// GMBUS1 bits
```

## L116 · `const GMBUS_CYCLE_INDEX: u32   = 1 << 26;  // use GMBUS5 index`

```
// use GMBUS5 index
```

## L121 · `const GMBUS_HW_RDY: u32        = 1 << 11;`

```
// GMBUS2 bits
```

## L123 · `const GMBUS_NAK: u32           = 1 << 10;  // SATOER`

```
// SATOER
```

## L126 · `const SCDC_I2C_ADDR: u8        = 0x54;    // 7-bit I2C address`

```
// HDMI SCDC (Status and Control Data Channel)
```

## L127 · `const SCDC_I2C_ADDR: u8        = 0x54;    // 7-bit I2C address`

```
// 7-bit I2C address
```

## L128 · `const SCDC_TMDS_CONFIG: u8     = 0x20;    // TMDS_Config register`

```
// TMDS_Config register
```

## L129 · `const SCDC_SCRAMBLER_STATUS: u8 = 0x21;   // Scrambler_Status (read-only)`

```
// Scrambler_Status (read-only)
```

## L131 · `const TRANS_DDI_MODE_MASK: u32                 = 0x7 << 24;`

```
// TRANS_DDI_FUNC_CTL mode select (bits [26:24])
```

## L133 · `const TRANS_DDI_MODE_HDMI: u32                 = 0 << 24;  // HDMI mode (required for scrambling)`

```
// HDMI mode (required for scrambling)
```

## L134 · `const TRANS_DDI_MODE_DVI: u32                  = 1 << 24;  // DVI mode (no scrambling, max 340MHz)`

```
// DVI mode (no scrambling, max 340MHz)
```

## L136 · `const TRANS_DDI_HDMI_SCRAMBLING: u32          = 1 << 0;`

```
// TRANS_DDI_FUNC_CTL scrambling bits (HDMI 2.0)
```

## L142 · `const TRANS_DDI_SCRAMBLING_MASK: u32 = TRANS_DDI_HDMI_SCRAMBLING`

```
// All scrambling bits combined
```

## L148 · `const GGTT_BASE: u32           = 0x800000;`

```
// GGTT base (within BAR0)
```

## L151 · `const GFX_FLSH_CNTL_GEN6: u32 = 0x101008;  // Gen 8-11 GGTT flush`

```
// GGTT TLB invalidation
```

## L152 · `const GFX_FLSH_CNTL_GEN6: u32 = 0x101008;  // Gen 8-11 GGTT flush`

```
// Gen 8-11 GGTT flush
```

## L153 · `const GEN12_GUC_TLB_INV_CR: u32 = 0xCEE8;  // Gen 12 GGTT TLB invalidation (required!)`

```
// Gen 12 GGTT TLB invalidation (required!)
```

## L154 · `const GEN12_BLT_TLB_INV_CR: u32 = 0xCEE4;  // Gen 12 BCS per-engine TLB invalidation`

```
// Gen 12 BCS per-engine TLB invalidation
```

## L156-164 · `const FORCEWAKE_GT: u32          = 0xA188;    // GT domain request`

```
// ── BCS (Blitter Command Streamer) — Gen 12 ExecList (ELSQ) ─────────
//
// Gen 12 does NOT support legacy ring mode. All engine submission goes
// through Enhanced ExecList Submission Queue (ELSQ):
//   1. Build Logical Ring Context (LRC) image in memory
//   2. Write context descriptor to ELSQ port
//   3. Trigger load → GPU restores context + processes ring
//
// Reference: i915 intel_execlists_submission.c, intel_lrc.c
```

## L166-168 · `const FORCEWAKE_GT: u32          = 0xA188;    // GT domain request`

```
// Forcewake domains (Gen 9+ multithreaded, masked bit writes)
// BCS at 0x22000 needs GT domain per i915 range table, but ADL-N
// may need all domains — request all three to be safe.
```

## L169 · `const FORCEWAKE_GT: u32          = 0xA188;    // GT domain request`

```
// GT domain request
```

## L170 · `const FORCEWAKE_ACK_GT: u32      = 0x130044;  // GT domain ack`

```
// GT domain ack
```

## L171 · `const FORCEWAKE_RENDER: u32      = 0xA278;    // Render domain request`

```
// Render domain request
```

## L172 · `const FORCEWAKE_ACK_RENDER: u32  = 0x0D84;    // Render domain ack`

```
// Render domain ack
```

## L173 · `const FORCEWAKE_MEDIA: u32       = 0xA270;    // Media domain request`

```
// Media domain request
```

## L174 · `const FORCEWAKE_ACK_MEDIA: u32   = 0x0D88;    // Media domain ack`

```
// Media domain ack
```

## L176 · `const BCS_RING_TAIL: u32        = 0x22030;`

```
// BCS engine MMIO registers (engine base = 0x22000)
```

## L182 · `const BCS_MI_MODE: u32          = 0x2209C;  // MI mode (bit 0 = STOP_RING)`

```
// MI mode (bit 0 = STOP_RING)
```

## L183 · `const BCS_HWSTAM: u32           = 0x22098;  // HW Status Mask`

```
// HW Status Mask
```

## L184 · `const BCS_RING_MODE: u32        = 0x2229C;  // Ring mode (Gen11+: bit 3 = disable legacy)`

```
// Ring mode (Gen11+: bit 3 = disable legacy)
```

## L185 · `const BCS_RESET_CTL: u32        = 0x220D0;  // Engine reset control`

```
// Engine reset control
```

## L186 · `const BCS_IMR: u32              = 0x220A8;  // Interrupt mask`

```
// Interrupt mask
```

## L187 · `const BCS_CTX_STATUS_PTR: u32   = 0x223A0;  // Context Status Buffer read/write pointers`

```
// Context Status Buffer read/write pointers
```

## L189 · `const BCS_ELSQ0_LO: u32         = 0x22510;  // Context descriptor 0, low DW`

```
// ELSQ (Enhanced ExecList Submission Queue) — Gen 11+
```

## L190 · `const BCS_ELSQ0_LO: u32         = 0x22510;  // Context descriptor 0, low DW`

```
// Context descriptor 0, low DW
```

## L191 · `const BCS_ELSQ0_HI: u32         = 0x22514;  // Context descriptor 0, high DW`

```
// Context descriptor 0, high DW
```

## L192 · `const BCS_ELSQ1_LO: u32         = 0x22518;  // Context descriptor 1, low DW (unused)`

```
// Context descriptor 1, low DW (unused)
```

## L193 · `const BCS_ELSQ1_HI: u32         = 0x2251C;  // Context descriptor 1, high DW`

```
// Context descriptor 1, high DW
```

## L194 · `const BCS_ELSQ_CONTROL: u32     = 0x22550;  // Load trigger (write 1)`

```
// Load trigger (write 1)
```

## L195 · `const BCS_ELSQ_STATUS_LO: u32   = 0x22234;  // ExecList status`

```
// ExecList status
```

## L197 · `const GEN6_GDRST: u32           = 0x941C;   // Global reset`

```
// Global domain reset
```

## L198 · `const GEN6_GDRST: u32           = 0x941C;   // Global reset`

```
// Global reset
```

## L199 · `const GEN11_GRDOM_BLT: u32      = 1 << 2;   // BCS reset domain`

```
// BCS reset domain
```

## L201 · `const RING_CTL_VALID: u32       = 1 << 0;`

```
// Ring control
```

## L205 · `const GFX_RUN_LIST_ENABLE: u32  = 1 << 15;  // Gen 8-10 ExecList enable (readback indicator on Gen11+)`

```
// RING_MODE bits
```

## L206 · `const GFX_RUN_LIST_ENABLE: u32  = 1 << 15;  // Gen 8-10 ExecList enable (readback indicator on Gen11+)`

```
// Gen 8-10 ExecList enable (readback indicator on Gen11+)
```

## L207 · `const GEN11_GFX_DISABLE_LEGACY_MODE: u32 = 1 << 3;  // Gen 11+: replaces GFX_RUN_LIST_ENABLE`

```
// Gen 11+: replaces GFX_RUN_LIST_ENABLE
```

## L208 · `const GFX_PREFETCH_DISABLE: u32 = 1 << 10;  // Gen 12: MUST be set!`

```
// Gen 12: MUST be set!
```

## L209 · `const STOP_RING: u32            = 1 << 8;   // MI_MODE bit 8 (REG_BIT(8) in i915)`

```
// MI_MODE bit 8 (REG_BIT(8) in i915)
```

## L211 · `const RESET_CTL_REQUEST: u32    = 1 << 0;`

```
// RESET_CTL bits
```

## L215 · `const MI_NOOP: u32              = 0;`

```
// MI commands
```

## L217 · `const MI_LRI_CMD: u32           = 0x22 << 23;  // MI_LOAD_REGISTER_IMM (opcode 0x22!)`

```
// MI_LOAD_REGISTER_IMM (opcode 0x22!)
```

## L218 · `const MI_LRI_FORCE_POSTED: u32  = 1 << 12;     // Posted write (no ack wait)`

```
// Posted write (no ack wait)
```

## L219 · `const MI_BB_END: u32            = 0x0A << 23;   // MI_BATCH_BUFFER_END`

```
// MI_BATCH_BUFFER_END
```

## L221-223 · `const XY_FAST_COPY_BLT_CMD: u32 = (2 << 29) | (0x42 << 22);`

```
// XY_FAST_COPY_BLT (Gen 9+, 10 DWORDs)
// Bits 21:20 = Source Tiling (NOT GGTT flag! 00=linear, correct for us)
// Bits 14:13 = Dest Tiling (00=linear)
```

## L227-230 · `const CTX_VALID: u32            = 1 << 0;`

```
// Context descriptor flags (Gen 12)
// Addressing mode: bits [4:3]. i915 uses INTEL_LEGACY_32B_CONTEXT = 1 << 3
// on ALL Gen 12 hardware. "Legacy" means 32-bit VA, not legacy ring mode.
// "Advanced" (bit 4) requires 4-level PPGTT page tables we don't have.
```

## L232 · `const CTX_FORCE_RESTORE: u32    = 1 << 2;   // Bit 2 = Force Context Restore`

```
// Bit 2 = Force Context Restore
```

## L233 · `const CTX_LEGACY_32B: u32       = 1 << 3;   // 32-bit VA (i915 default on Gen 12)`

```
// 32-bit VA (i915 default on Gen 12)
```

## L234 · `const CTX_PRIVILEGE: u32        = 1 << 8;   // Privileged context`

```
// Privileged context
```

## L236 · `const BCS_RING_GGTT: u32        = 0x0400_0000;  // 64MB: ring buffer (4KB)`

```
// GGTT layout for BCS resources (beyond framebuffer at 0x0100_0000)
```

## L237 · `const BCS_RING_GGTT: u32        = 0x0400_0000;  // 64MB: ring buffer (4KB)`

```
// 64MB: ring buffer (4KB)
```

## L238 · `const BCS_LRC_GGTT: u32         = 0x0400_1000;  // LRC: page 0 = HWSP, page 1 = context`

```
// LRC: page 0 = HWSP, page 1 = context
```

## L239 · `const BCS_TEST_GGTT: u32        = 0x0400_3000;  // test surface (16KB)`

```
// test surface (16KB)
```

## L240 · `const SHADOW_A_GGTT_BASE: u32   = 0x0800_0000;  // 128MB: shadow buffer A`

```
// 128MB: shadow buffer A
```

## L241 · `const SHADOW_B_GGTT_BASE: u32   = 0x0A00_0000;  // 160MB: shadow buffer B`

```
// 160MB: shadow buffer B
```

## L243 · `struct DisplayTiming {`

```
// ── Display Timings ─────────────────────────────────────────────────
```

## L245 · `struct DisplayTiming {`

```
/// CEA-861 standard timings
```

## L268 · `const TIMING_4K_60: DisplayTiming = DisplayTiming {`

```
// Standard CEA/VESA timings
```

## L305 · `fn match_firmware_timing(width: u32, height: u32) -> &'static DisplayTiming {`

```
/// Match firmware's active resolution to a known timing.
```

## L314 · `struct PllParams {`

```
// ── DPLL Parameters ─────────────────────────────────────────────────
```

## L316-317 · `struct PllParams {`

```
/// Pre-calculated PLL parameters for known pixel clocks.
/// DCO frequency, integer/fraction, and output dividers.
```

## L327-337 · `match pixel_clock_khz {`

```
// Intel combo PHY PLL (TGL/ADL):
//   AFE_clock = pixel_clock * 5 (HDMI TMDS)
//   DCO = AFE_clock * divider
//   DCO range: [7,998,000 .. 10,000,000] kHz
//   Reference clock = 19.2 MHz (N100 / ADL-N)
//   dco_integer = DCO / 19200 (integer part)
//   dco_fraction = remainder * 0x8000 / 19200
//
// All modes use same DCO (8,910,000 kHz), only dividers differ.
// dco_integer = 464 (0x1D0), dco_fraction = 0x800
// Verified against firmware CFGCR0 = 0x001001D0
```

## L340 · `dco_integer: 464, dco_fraction: 0x800,`

```
// AFE=2970 MHz, div=3, DCO=8910 MHz
```

## L345 · `dco_integer: 464, dco_fraction: 0x800,`

```
// AFE=1485 MHz, div=6, DCO=8910 MHz
```

## L350-351 · `dco_integer: 464, dco_fraction: 0x800,`

```
// AFE=742.5 MHz, div=12, DCO=8910 MHz
// Verified: matches firmware CFGCR1=0x00000E84
```

## L356 · `dco_integer: 440, dco_fraction: 0x1800,`

```
// AFE=1207.5 MHz, div=7, DCO=8452.5 MHz
```

## L364-365 · `fn encode_cfgcr(params: &PllParams) -> (u32, u32) {`

```
/// Encode PLL params into DPLL_CFGCR0/CFGCR1 register values (TGL/ADL format).
/// Bit layout verified against NUC firmware values.
```

## L367 · `let cfgcr0 = ((params.dco_fraction as u32) << 9) | (params.dco_integer as u32 & 0x1FF);`

```
// CFGCR0: dco_fraction[24:9] | dco_integer[8:0]
```

## L370-374 · `let pdiv_enc: u32 = match params.pdiv {`

```
// CFGCR1 (actual TGL/ADL layout, verified against firmware):
//   QDIV_RATIO [17:10]
//   QDIV_MODE  [9]     — 1 if qdiv > 1
//   KDIV       [8:6]   — encoded: 1→1, 2→2, 3→4
//   PDIV       [5:2]   — encoded: 2→1, 3→2, 5→4, 7→6
```

## L392 · `fn mmio_read32(base: u64, offset: u32) -> u32 {`

```
// ── MMIO Helpers ────────────────────────────────────────────────────
```

## L396 · `unsafe { core::ptr::read_volatile(addr) }`

```
// SAFETY: BAR0 is identity-mapped, volatile prevents reordering
```

## L402 · `unsafe { core::ptr::write_volatile(addr, val); }`

```
// SAFETY: BAR0 is identity-mapped, volatile prevents reordering
```

## L408 · `unsafe { core::ptr::write_volatile(addr, val); }`

```
// SAFETY: BAR0 is identity-mapped, volatile ensures write reaches device
```

## L412 · `fn poll_timeout(base: u64, reg: u32, mask: u32, expected: u32, max_iters: u32) -> bool {`

```
/// Spin-wait with timeout (in iterations). Returns true if condition met.
```

## L423 · `pub struct IntelXeDriver {`

```
// ── Driver State ────────────────────────────────────────────────────
```

## L429 · `bar0: u64,          // GTTMMADR: 16MB MMIO registers + GGTT`

```
// GTTMMADR: 16MB MMIO registers + GGTT
```

## L430 · `bar2: u64,          // GMADR: 256MB aperture`

```
// GMADR: 256MB aperture
```

## L432 · `fb_ggtt_offset: u32,  // GGTT offset of scanout framebuffer`

```
// GGTT offset of scanout framebuffer
```

## L433 · `fb_phys: u64,         // Physical address of framebuffer memory`

```
// Physical address of framebuffer memory
```

## L434 · `fb_pages: u32,        // Number of 4KB pages allocated`

```
// Number of 4KB pages allocated
```

## L436 · `measured_hz: u8,      // Refresh measured from the vblank frame counter`

```
// Refresh measured from the vblank frame counter
```

## L437-438 · `ddi_port: u8,         // Which DDI port (0=A, 1=B, etc.)`

```
// (0 = not measured; firmware mode is only ASSUMED
// from resolution, so measure it instead of guessing)
```

## L439 · `ddi_port: u8,         // Which DDI port (0=A, 1=B, etc.)`

```
// Which DDI port (0=A, 1=B, etc.)
```

## L440 · `firmware_dpll: u8,    // Which DPLL firmware used (detected at boot)`

```
// Which DPLL firmware used (detected at boot)
```

## L441 · `bcs_ring_phys: u64,   // Physical address of ring buffer (4KB)`

```
// BCS engine state (ExecList / ELSQ)
```

## L442 · `bcs_ring_phys: u64,   // Physical address of ring buffer (4KB)`

```
// Physical address of ring buffer (4KB)
```

## L443 · `bcs_lrc_phys: u64,    // Physical address of LRC (8KB: HWSP + context)`

```
// Physical address of LRC (8KB: HWSP + context)
```

## L445-447 · `bcs_verified: bool,`

```
// True only after a readback self-test proved the BCS blit actually
// PAINTS (ring advancing is not enough — on Tiger Lake the ring caught
// up but no pixels landed → black screen). Gates the display-blit path.
```

## L449 · `bcs_readback_got: u32, // last verify_blit_readback dst[0] (diagnostic)`

```
// last verify_blit_readback dst[0] (diagnostic)
```

## L450 · `shadow_a_ggtt: u32,   // GGTT offset of shadow buffer A (0 = not mapped)`

```
// Shadow buffer GGTT state
```

## L451 · `shadow_a_ggtt: u32,   // GGTT offset of shadow buffer A (0 = not mapped)`

```
// GGTT offset of shadow buffer A (0 = not mapped)
```

## L452 · `shadow_b_ggtt: u32,   // GGTT offset of shadow buffer B (0 = not mapped)`

```
// GGTT offset of shadow buffer B (0 = not mapped)
```

## L453 · `shadow_pages: u32,    // Pages per shadow buffer`

```
// Pages per shadow buffer
```

## L456 · `impl GpuHal for IntelXeDriver {`

```
// ── GpuHal implementation ──────────────────────────────────
```

## L464 · `IntelXeDriver::set_mode(self, width, height, hz)`

```
// Delegate to the existing set_mode implementation
```

## L489-491 · `fn supports_modeset(&self) -> bool { is_adln(self.device_id) }`

```
/// Only ADL-N has a validated DPLL/pipe modeset path. Non-ADL-N Gen12
/// (Tiger Lake) runs blit-only and keeps the firmware mode — a modeset
/// there reprograms an unvalidated pipeline and blacks the scanout.
```

## L496 · `mmio_write32(self.bar0, PLANE_SURF_1_A, surface_addr as u32);`

```
// Write PLANE_SURF — GPU reads new address at next vblank
```

## L502 · `let cnt = mmio_read32(self.bar0, PIPE_FRMCNT_A);`

```
// Poll frame counter until it increments (= vblank occurred)
```

## L519-522 · `self.bcs_verified = self.verify_blit_readback();`

```
// Ring up ≠ pixels land. Prove the blit actually paints (readback)
// before letting it drive the display — else a non-painting blit
// (Tiger Lake) silently blacks the screen because try_gpu_blit
// reports success and the CPU fallback is skipped.
```

## L584 · `if self.bcs_lrc_phys != 0 {`

```
// LRC values from RAM (GPU writes here on context-save)
```

## L587 · `unsafe {`

```
// SAFETY: lrc_phys is identity-mapped, within our allocation
```

## L589 · `let lh = core::ptr::read_volatile(ctx.add(5));  // HEAD val`

```
// HEAD val
```

## L590 · `let lt = core::ptr::read_volatile(ctx.add(7));  // TAIL val`

```
// TAIL val
```

## L591 · `let ls = core::ptr::read_volatile(ctx.add(9));  // START val`

```
// START val
```

## L592 · `let lc = core::ptr::read_volatile(ctx.add(11)); // CTL val`

```
// CTL val
```

## L593 · `let lx = core::ptr::read_volatile(ctx.add(3));  // CTX_CTRL val`

```
// CTX_CTRL val
```

## L598 · `let hwsp = self.bcs_lrc_phys as *const u32;`

```
// HWSP first 4 DWORDs
```

## L612 · `pub fn detect() -> Option<Self> {`

```
/// Scan PCI bus for Intel Xe GPU.
```

## L614-617 · `for &(did, name) in KNOWN_DEVICE_IDS {`

```
// Only activate native modesetting on hardware we explicitly support
// (ADL-N / N100). Any other Intel GPU stays on the safe GOP framebuffer
// the firmware set up — programming ADL-N registers on a different gen
// tears down the live pipe and blacks the screen.
```

## L627-636 · `pub fn auto_activate_ok(&self) -> bool {`

```
/// Whether this GPU may be auto-activated at boot (before login) — so the
/// user never has to run `gpu init` + `gpu blit init` by hand. The set is
/// the GPUs validated end-to-end on real hardware:
///   - ADL-N (modeset + BCS paints + scanout),
///   - Tiger Lake-LP GT2 0x9A78 (blit-only takeover, BCS paints @ ~4ms).
/// Safe regardless: init() for non-ADL-N is blit-only (no modeset glitch),
/// and the BCS display path is readback-gated — a GPU that detects but
/// doesn't paint falls back to the visible CPU/GOP blit, never black.
/// A newly-added (untested) device ID should be left OUT of this set until
/// validated, so it detects + can be brought up manually first.
```

## L642 · `let cmd = pci::read32(dev.addr, 0x04);`

```
// Enable PCI memory space access
```

## L654 · `if bar0 != 0 {`

```
// Map BAR0 (16MB) so registers are accessible for dump and init
```

## L701-702 · `pub fn test_pll(&self) {`

```
/// Test PLL locking by reading firmware values, disabling, re-writing, re-enabling.
/// Does NOT touch the display pipeline — only the PLL.
```

## L706 · `let orig_enable = mmio_read32(self.bar0, enable_reg);`

```
// Read current firmware PLL state
```

## L723 · `kprintln!("[npk]   Step 1: Disabling pipe + transcoder...");`

```
// Step 1: Disable the display pipeline first (must stop using PLL before disabling it)
```

## L729 · `let ddi_ctl = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };`

```
// Disable DDI buffer
```

## L735 · `mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, 0);`

```
// Disable transcoder DDI function
```

## L741 · `if !poll_timeout(self.bar0, enable_reg, 1 << 30, 0, 200_000) {`

```
// Wait for PLL to unlock
```

## L748 · `kprintln!("[npk]   Step 3: Writing CFGCR0={:#010x} CFGCR1={:#010x}",`

```
// Step 3: Write back the SAME CFGCR values
```

## L753 · `let _ = mmio_read32(self.bar0, cfgcr1_reg);`

```
// Posting read to ensure writes complete
```

## L756 · `kprintln!("[npk]   Step 4: Enabling DPLL{}...", self.firmware_dpll);`

```
// Step 4: Re-enable PLL
```

## L760 · `let locked = poll_timeout(self.bar0, enable_reg, 1 << 30, 1 << 30, 1_000_000);`

```
// Poll for lock
```

## L775-776 · `if self.measured_hz != 0 { return self.measured_hz; }`

```
// Prefer the empirically measured rate — the firmware mode's hz is only
// assumed from resolution (match_firmware_timing hardcodes 30 for 4K).
```

## L781-784 · `pub fn measure_refresh_hz(&self) -> u8 {`

```
/// Measure the real refresh rate from the vblank frame counter
/// (PIPE_FRMCNT increments once per frame). Read-only — counts frames over
/// a fixed wall-clock window via the 100 Hz tick counter, no register
/// writes. Returns 0 if the counter doesn't advance (pipe idle / no clock).
```

## L787 · `const WINDOW_TICKS: u64 = 50; // 500 ms @ 100 Hz → 60Hz→~30 / 30Hz→~15 frames`

```
// 500 ms @ 100 Hz → 60Hz→~30 / 30Hz→~15 frames
```

## L790 · `let mut elapsed = 0u64;`

```
// Bail if ticks aren't advancing at all (no time source) to avoid a hang.
```

## L795 · `if elapsed > WINDOW_TICKS + 200 { break; } // ~2.5s safety ceiling`

```
// ~2.5s safety ceiling
```

## L799 · `((frames * 100 + elapsed / 2) / elapsed) as u8`

```
// hz = frames / (elapsed/100 s) = frames*100/elapsed, rounded to nearest.
```

## L818 · `pub fn init(&mut self) -> Result<FramebufferInfo, GpuError> {`

```
// ── Initialization ──────────────────────────────────────────────
```

## L820-823 · `pub fn init(&mut self) -> Result<FramebufferInfo, GpuError> {`

```
/// Initialize display by reusing the firmware's existing framebuffer.
/// The firmware (UEFI) already has a fully working display pipeline.
/// We use the GOP framebuffer address (known-good, already being drawn to)
/// and record hardware state for future mode changes.
```

## L825 · `let cmd = pci::read32(self.pci_addr, 0x04);`

```
// Enable PCI memory space + bus mastering
```

## L833 · `self.detect_ddi_ports();`

```
// Detect firmware DDI/DPLL config (read-only, no writes)
```

## L836 · `self.power_on()?;`

```
// Ensure power wells are on (usually already by firmware)
```

## L839 · `self.init_cdclk()?;`

```
// Ensure DBUF is enabled
```

## L842 · `let transconf = mmio_read32(self.bar0, PIPE_CONF_A);`

```
// Check if firmware has an active display pipeline
```

## L851 · `let htotal_reg = mmio_read32(self.bar0, TRANS_HTOTAL_A);`

```
// Read firmware's active resolution from transcoder A
```

## L858 · `let fw_plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);`

```
// Log firmware plane state
```

## L865 · `let ggtt_entry_idx = fw_plane_surf / 4096;`

```
// Diagnostic: read GGTT entry to see physical address (LOG ONLY, no writes)
```

## L873-876 · `let gop_addr = crate::framebuffer::with_fb(|fb| {`

```
// Use the GOP framebuffer address — it's the same memory the display
// is already scanning, and it's known-good (text is rendering on it).
// The GGTT entry might point to stolen memory (not CPU-accessible),
// but the GOP address from Multiboot2 is always safe.
```

## L895 · `self.fb_pages = 0; // Not our allocation — don't free firmware GGTT entries`

```
// Not our allocation — don't free firmware GGTT entries
```

## L902-904 · `self.measured_hz = self.measure_refresh_hz();`

```
// The firmware mode's hz above is ASSUMED from resolution. Measure the
// real refresh from the vblank counter so status + current_hz() report
// the truth (and we know whether a 60Hz modeset is even worth pursuing).
```

## L909-915 · `if !is_adln(self.device_id) {`

```
// Non-ADL-N Gen12 (e.g. Tiger Lake): the display register layout is
// shared, but the PLL/modeset path is only validated on ADL-N.
// Take the safe blit-only route — keep the firmware's live mode and
// expose the scanout (fb_ggtt_offset = fw_plane_surf, set above) for
// the BCS engine. No PLL/pipe reprogram → no black-screen risk. The
// CPU stops blitting to the slow UC framebuffer; the GPU copies
// shadow→scanout instead. `gpu blit init` then attaches BCS.
```

## L922 · `kprintln!("[npk]   Attempting 4K@60Hz...");`

```
// Try 4K@60 (HDMI 2.0 scrambling), fallback to 4K@30
```

## L946-948 · `pub fn set_mode(&mut self, width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {`

```
/// Set a new display mode. Reprogrms DPLL + transcoder timings,
/// allocates new framebuffer via GGTT, returns aperture address.
/// DDI/PHY stay running — only pipe+plane are cycled.
```

## L950-953 · `if !is_adln(self.device_id) {`

```
// Blit-only takeovers (non-ADL-N Gen12) keep the firmware mode; a real
// DPLL/pipe/transcoder reprogram is only validated on ADL-N and would
// black the scanout here. Refuse so neither the post-login auto-upgrade
// nor a manual `gpu mode` can blank a blit-only panel.
```

## L975 · `if had_scrambling && !needs_scrambling {`

```
// Step 0: Disable old scrambling before tearing down pipeline
```

## L980 · `let plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);`

```
// Step 1: Disable plane
```

## L986-987 · `let pipe_val = mmio_read32(self.bar0, PIPE_CONF_A);`

```
// Step 2: Disable pipe — try BOTH possible config registers
// ADL-N: PIPE_CONF may be at 0x70008 or 0xF0008 depending on stepping
```

## L990 · `mmio_write32(self.bar0, 0x70008, 0);  // PIPE_CONF_A`

```
// Write disable to both offsets (harmless if one is invalid)
```

## L991 · `mmio_write32(self.bar0, 0x70008, 0);  // PIPE_CONF_A`

```
// PIPE_CONF_A
```

## L992 · `mmio_write32(self.bar0, 0xF0008, 0);  // TRANSCONF_A`

```
// TRANSCONF_A
```

## L993 · `for _ in 0..2_000_000u32 { core::hint::spin_loop(); }`

```
// Blind wait ~20ms for pipe to drain (don't rely on polling)
```

## L998 · `let ps_ctrl = mmio_read32(self.bar0, PS_CTRL_1A);`

```
// Step 2b: Disable pipe scaler (firmware may use it for 1080p→4K upscale)
```

## L1005 · `let _ = mmio_read32(self.bar0, PS_CTRL_1A);`

```
// Posting read
```

## L1009 · `self.free_framebuffer();`

```
// Step 3: Free old GGTT entries (no-op if fb_pages == 0)
```

## L1012 · `if need_pll_change {`

```
// Step 4: Reprogram DPLL if pixel clock changes
```

## L1017 · `self.program_transcoder(timing);`

```
// Step 5: Program transcoder timings + pipe source size
```

## L1021 · `mmio_write32(self.bar0, 0x7001C, srcsz);`

```
// Also try pipe-domain offset 0x7001C (in case 0x6001C is transcoder-only)
```

## L1025 · `let pitch = timing.width * 4;`

```
// Step 6: Allocate framebuffer (contiguous physical RAM)
```

## L1032 · `unsafe { core::ptr::write_bytes(phys as *mut u8, 0, fb_size as usize); }`

```
// SAFETY: phys is identity-mapped, contiguous, freshly allocated
```

## L1037 · `self.fb_ggtt_offset = 0x0100_0000; // 16MB into GGTT (avoid firmware entries)`

```
// 16MB into GGTT (avoid firmware entries)
```

## L1042 · `self.program_ggtt_32()?;`

```
// Step 7: Program GGTT entries (32-bit writes for MMIO safety)
```

## L1045 · `mmio_write32(self.bar0, GFX_FLSH_CNTL_GEN6, 1);`

```
// Step 8: Invalidate GGTT TLB
```

## L1050-1051 · `let aperture_addr = self.bar2 + self.fb_ggtt_offset as u64;`

```
// Step 9: Map aperture pages for CPU access (BAR2 + GGTT offset)
// Use Write-Combining for ~5-10x faster sequential framebuffer writes.
```

## L1067 · `let new_plane_ctl = (1u32 << 31)    // enable`

```
// Step 10: Configure plane (match firmware CTL including bit 3)
```

## L1068 · `let new_plane_ctl = (1u32 << 31)    // enable`

```
// enable
```

## L1069 · `| (0x4 << 24)                    // XRGB 8:8:8:8`

```
// XRGB 8:8:8:8
```

## L1070 · `| (1 << 3);                      // bit 3 (matches firmware)`

```
// bit 3 (matches firmware)
```

## L1077 · `mmio_write32(self.bar0, PLANE_SURF_1_A, self.fb_ggtt_offset); // triggers flip`

```
// triggers flip
```

## L1081-1082 · `let fb = FramebufferInfo {`

```
// Update fb info NOW (before pipe re-enable which might timeout).
// The framebuffer, GGTT, and aperture are all valid at this point.
```

## L1093 · `if needs_scrambling {`

```
// Step 11: Enable HDMI 2.0 scrambling BEFORE pipe enable (i915 sequence)
```

## L1100 · `mmio_write32(self.bar0, 0x70008, 1 << 31);  // PIPE_CONF_A`

```
// Step 12: Re-enable pipe (write to both possible offsets)
```

## L1101 · `mmio_write32(self.bar0, 0x70008, 1 << 31);  // PIPE_CONF_A`

```
// PIPE_CONF_A
```

## L1102 · `mmio_write32(self.bar0, 0xF0008, 1 << 31);  // TRANSCONF_A`

```
// TRANSCONF_A
```

## L1103 · `for _ in 0..2_000_000u32 { core::hint::spin_loop(); }`

```
// Wait for pipe to start
```

## L1107 · `let srcsz = ((timing.width - 1) << 16) | (timing.height - 1);`

```
// Write PIPE_SRCSZ again after enable (some HW needs it live)
```

## L1115 · `pub fn dump_registers(&self) {`

```
// ── Register Dump ────────────────────────────────────────────────
```

## L1117-1118 · `pub fn dump_registers(&self) {`

```
/// Dump current display engine state (read-only, no writes).
/// Use this to understand what the firmware configured.
```

## L1123 · `let fuse = mmio_read32(self.bar0, FUSE_STATUS);`

```
// Fuses
```

## L1129 · `let pwr = mmio_read32(self.bar0, PWR_WELL_CTL2);`

```
// Power
```

## L1133 · `let cdclk = mmio_read32(self.bar0, CDCLK_CTL);`

```
// CDCLK
```

## L1139 · `let dpll0_en = mmio_read32(self.bar0, DPLL_ENABLE_0);`

```
// DPLL 0 and 1
```

## L1153 · `let dpclka = mmio_read32(self.bar0, ICL_DPCLKA_CFGCR0);`

```
// DDI clock routing
```

## L1157 · `let clk_sel = mmio_read32(self.bar0, TRANS_CLK_SEL_A);`

```
// Transcoder A clock selection
```

## L1161 · `let htotal = mmio_read32(self.bar0, TRANS_HTOTAL_A);`

```
// Transcoder A timings
```

## L1177 · `let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);`

```
// Transcoder DDI function control
```

## L1181 · `let ddi_sel = (ddi_func >> 27) & 0xF;`

```
// TGL+ port select is bits [30:27], encoding: 1=A, 2=B, 3=C
```

## L1194 · `let pipe_conf = mmio_read32(self.bar0, PIPE_CONF_A);`

```
// Pipe A
```

## L1205 · `let plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);`

```
// Plane 1
```

## L1220 · `let ddi_a = mmio_read32(self.bar0, DDI_BUF_CTL_A);`

```
// DDI buffer control
```

## L1231 · `fn detect_ddi_ports(&mut self) {`

```
// ── DDI Port Detection ──────────────────────────────────────────
```

## L1240 · `let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);`

```
// Read TRANS_DDI_FUNC_CTL to see what the firmware configured
```

## L1243-1244 · `let ddi_sel = ((ddi_func >> 27) & 0xF) as u8;`

```
// Firmware has an active DDI — use the same port
// TGL+ port select is bits [30:27]
```

## L1246 · `let port = if ddi_sel > 0 { ddi_sel - 1 } else { 0 };`

```
// TGL+ encoding: 1=A, 2=B, 3=C, ...
```

## L1252 · `self.ddi_port = 1; // DDI-B default (NUC HDMI is on DDI-B)`

```
// DDI-B default (NUC HDMI is on DDI-B)
```

## L1256 · `let clk_sel = mmio_read32(self.bar0, TRANS_CLK_SEL_A);`

```
// Detect which DPLL the firmware uses
```

## L1263 · `fn power_on(&self) -> Result<(), GpuError> {`

```
// ── Power Management ────────────────────────────────────────────
```

## L1268 · `let pwr = mmio_read32(self.bar0, PWR_WELL_CTL2);`

```
// Read current power well state
```

## L1272 · `self.enable_power_well(0, "PW1")?;`

```
// Enable PW1 (Power Group 1): bit 1 = request, bit 0 = state
```

## L1275 · `self.enable_power_well(1, "PW2")?;`

```
// Enable PW2 (Power Group 2): bit 3 = request, bit 2 = state
```

## L1278-1280 · `kprintln!("[npk]   GPU: power wells enabled");`

```
// Note: Combo PHY DDI ports (HDMI on ADL-N) do NOT need separate
// DDI power wells. Those are for TypeC/TBT ports only.
// PW1 + PW2 cover all combo PHY display functionality.
```

## L1290 · `let val = mmio_read32(self.bar0, PWR_WELL_CTL2);`

```
// Check if already on
```

## L1297 · `mmio_write32(self.bar0, PWR_WELL_CTL2, val | request_bit);`

```
// Request enable
```

## L1300 · `if !poll_timeout(self.bar0, PWR_WELL_CTL2, state_bit, state_bit, 200_000) {`

```
// Poll for state bit (up to 20ms equivalent in iterations)
```

## L1310 · `fn init_cdclk(&self) -> Result<(), GpuError> {`

```
// ── CDCLK (Core Display Clock) ──────────────────────────────────
```

## L1313 · `let cdclk = mmio_read32(self.bar0, CDCLK_CTL);`

```
// Read current CDCLK
```

## L1317-1328 · `let dbuf = mmio_read32(self.bar0, DBUF_CTL_S1);`

```
// ADL CDCLK_CTL format (Gen 12):
//   Bits 10:8 = cd2x divider select (0=bypass/1x, 1=/2)
//   Bits 25:22 = SSA precharge
//   Bit 26 = PLL enable
//
// ADL CDCLK frequencies (from ref clock 38.4 MHz with cd2x):
//   cd2x=0 (bypass): 172.8, 192, 307.2, 312, 552, 556.8, 648, 652.8 MHz
//
// For 4K@60Hz (594 MHz pixel clock), CDCLK must be >= 312 MHz.
// Firmware typically sets 312 or higher for HDMI output.
// Log current value for diagnostics but don't reprogram yet —
// if 4K@60 fails, CDCLK will be a suspect to investigate.
```

## L1330 · `let dbuf = mmio_read32(self.bar0, DBUF_CTL_S1);`

```
// Enable DBUF (Display Buffer)
```

## L1346 · `fn allocate_framebuffer(&mut self, timing: &DisplayTiming) -> Result<FramebufferInfo, GpuError> {`

```
// ── Display Pipeline ────────────────────────────────────────────
```

## L1348 · `fn allocate_framebuffer(&mut self, timing: &DisplayTiming) -> Result<FramebufferInfo, GpuError> {`

```
// ── Framebuffer Allocation ──────────────────────────────────────
```

## L1351 · `let pitch = timing.width * 4; // 32bpp XRGB8888`

```
// 32bpp XRGB8888
```

## L1355 · `let phys = memory::allocate_contiguous(pages as usize)`

```
// Allocate contiguous physical memory for scanout
```

## L1359-1360 · `unsafe {`

```
// Zero the framebuffer (black)
// SAFETY: phys is identity-mapped, contiguous, and we just allocated it
```

## L1367 · `self.fb_ggtt_offset = 0x0100_0000;`

```
// Use a GGTT offset that doesn't conflict with firmware (16MB in)
```

## L1374 · `addr: phys,  // CPU writes via identity-mapped physical address`

```
// CPU writes via identity-mapped physical address
```

## L1384 · `let ggtt_base = self.bar0 + GGTT_BASE as u64;`

```
// Clear GGTT entries
```

## L1391 · `self.fb_pages = 0;`

```
// Note: physical memory is not freed (no free API in memory.rs)
```

## L1397 · `fn program_ggtt(&self) -> Result<(), GpuError> {`

```
// ── GGTT Programming ────────────────────────────────────────────
```

## L1405-1408 · `let ggtt_entry: u64 = (phys_addr & 0xFFFF_FFFF_FFFF_F000) | 0x01; // valid, system mem`

```
// GGTT PTE format (Gen 12):
//   Bits 47:12 = physical page address
//   Bit 1 = local memory (0 = system RAM)
//   Bit 0 = valid/present
```

## L1409 · `let ggtt_entry: u64 = (phys_addr & 0xFFFF_FFFF_FFFF_F000) | 0x01; // valid, system mem`

```
// valid, system mem
```

## L1415 · `let _ = mmio_read32(self.bar0, GGTT_BASE);`

```
// Flush GGTT writes with a read-back
```

## L1418 · `let first_off = (start_entry * 8) as u32;`

```
// Log first and last entries for verification
```

## L1429 · `fn program_ggtt_32(&self) -> Result<(), GpuError> {`

```
/// Program GGTT entries using 32-bit writes (safer for MMIO than 64-bit).
```

## L1435 · `let entry_lo = (phys_addr as u32 & 0xFFFF_F000) | 0x01; // valid, system mem`

```
// valid, system mem
```

## L1443 · `let _ = mmio_read32(self.bar0, GGTT_BASE);`

```
// Flush with read-back
```

## L1446 · `let first_off = GGTT_BASE as u32 + start_entry * 8;`

```
// Log first entry for verification
```

## L1455 · `fn dpll_regs(&self) -> (u32, u32, u32) {`

```
// ── DPLL Programming ────────────────────────────────────────────
```

## L1457 · `fn dpll_regs(&self) -> (u32, u32, u32) {`

```
/// Get DPLL register offsets for the active DPLL (0 or 1).
```

## L1479 · `let dpll = mmio_read32(self.bar0, enable_reg);`

```
// Disable DPLL first
```

## L1486 · `mmio_write32(self.bar0, cfgcr0_reg, cfgcr0);`

```
// Write PLL configuration
```

## L1490 · `mmio_write32(self.bar0, enable_reg, 1 << 31);`

```
// Enable DPLL
```

## L1493 · `if !poll_timeout(self.bar0, enable_reg, 1 << 30, 1 << 30, 500_000) {`

```
// Poll for PLL lock (bit 30 on TGL+)
```

## L1504 · `fn program_transcoder(&self, t: &DisplayTiming) {`

```
// ── Transcoder Timing ───────────────────────────────────────────
```

## L1514 · `mmio_write32(self.bar0, TRANS_HTOTAL_A, ((h_total - 1) << 16) | (t.width - 1));`

```
// HTOTAL = (total-1) << 16 | (active-1)
```

## L1516 · `mmio_write32(self.bar0, TRANS_HBLANK_A, ((h_total - 1) << 16) | (t.width - 1));`

```
// HBLANK = (total-1) << 16 | (active-1) — blank covers non-active area
```

## L1518 · `mmio_write32(self.bar0, TRANS_HSYNC_A, ((h_sync_end - 1) << 16) | (h_sync_start - 1));`

```
// HSYNC = (sync_end-1) << 16 | (sync_start-1)
```

## L1529 · `fn configure_plane(&self, timing: &DisplayTiming) -> Result<(), GpuError> {`

```
// ── Plane Configuration ─────────────────────────────────────────
```

## L1532 · `let stride_64b = (timing.width * 4) / 64; // Stride in 64-byte units`

```
// Stride in 64-byte units
```

## L1534 · `let plane_ctl = (1u32 << 31)       // enable`

```
// PLANE_CTL: enable, XRGB8888 format, linear tiling
```

## L1535 · `let plane_ctl = (1u32 << 31)       // enable`

```
// enable
```

## L1536 · `| (0x4 << 24)                  // XRGB 8:8:8:8 pixel format`

```
// XRGB 8:8:8:8 pixel format
```

## L1537 · `| (0 << 10);                   // linear tiling (no tiling)`

```
// linear tiling (no tiling)
```

## L1540 · `mmio_write32(self.bar0, PLANE_STRIDE_1_A, stride_64b);`

```
// Stride in 64-byte chunks
```

## L1543 · `mmio_write32(self.bar0, PLANE_POS_1_A, 0);`

```
// Position (0,0)
```

## L1546 · `mmio_write32(self.bar0, PLANE_SIZE_1_A,`

```
// Size: (height-1) << 16 | (width-1)
```

## L1550 · `mmio_write32(self.bar0, PLANE_SURF_1_A, self.fb_ggtt_offset);`

```
// Surface address (GGTT offset, 4K-aligned) — writing this triggers the flip
```

## L1558 · `fn gmbus_wait_idle(&self) -> bool {`

```
// ── GMBUS I2C (for HDMI SCDC) ──────────────────────────────────
```

## L1560 · `fn gmbus_wait_idle(&self) -> bool {`

```
/// Wait for GMBUS to become idle/ready.
```

## L1572 · `fn gmbus_wait_hw_rdy(&self) -> bool {`

```
/// Wait for GMBUS HW_RDY (data transferred).
```

## L1588 · `fn gmbus_reset(&self) {`

```
/// Reset GMBUS after error or before use.
```

## L1590 · `mmio_write32(self.bar0, GMBUS1, GMBUS_SW_CLR_INT);`

```
// Set SW_CLR_INT to clear any pending state
```

## L1593 · `mmio_write32(self.bar0, GMBUS0, 0);`

```
// Select no port
```

## L1598 · `fn gmbus_write_byte(&self, slave_addr: u8, reg: u8, val: u8) -> bool {`

```
/// Write a single byte to an I2C register via GMBUS.
```

## L1602 · `mmio_write32(self.bar0, GMBUS0, pin);`

```
// Select port
```

## L1611 · `mmio_write32(self.bar0, GMBUS3, (val as u32) << 8 | reg as u32);`

```
// Data: reg byte + value byte (little-endian in GMBUS3)
```

## L1614 · `let cmd = GMBUS_SW_RDY`

```
// Command: 2 bytes, write, slave address, WAIT+STOP cycle
```

## L1618 · `| (2u32 << 16)                          // byte count = 2 (reg + val)`

```
// byte count = 2 (reg + val)
```

## L1619 · `| ((slave_addr as u32) << 1)            // slave addr (7-bit, shifted)`

```
// slave addr (7-bit, shifted)
```

## L1624 · `self.gmbus_wait_idle();`

```
// Wait for bus to go idle
```

## L1626 · `mmio_write32(self.bar0, GMBUS1, GMBUS_SW_CLR_INT);`

```
// Clean up
```

## L1639 · `fn gmbus_read_byte(&self, slave_addr: u8, reg: u8) -> Option<u8> {`

```
/// Read a single byte from an I2C register via GMBUS (indexed read).
```

## L1643 · `mmio_write32(self.bar0, GMBUS0, pin);`

```
// Select port
```

## L1652 · `mmio_write32(self.bar0, GMBUS5, (reg as u32) | (1 << 31)); // index enable`

```
// Set index register (GMBUS5) for indexed read
```

## L1653 · `mmio_write32(self.bar0, GMBUS5, (reg as u32) | (1 << 31)); // index enable`

```
// index enable
```

## L1655 · `let cmd = GMBUS_SW_RDY`

```
// Command: 1 byte, read, slave address, INDEX+WAIT+STOP
```

## L1660 · `| (1u32 << 16)                          // byte count = 1`

```
// byte count = 1
```

## L1674 · `mmio_write32(self.bar0, GMBUS5, 0); // disable index`

```
// disable index
```

## L1682 · `pub fn init_bcs(&mut self) -> Result<(), GpuError> {`

```
// ── BCS (Blitter Command Streamer) — ExecList Submission ──────────
```

## L1684-1694 · `pub fn init_bcs(&mut self) -> Result<(), GpuError> {`

```
/// Initialize BCS via Gen 12 ExecList (ELSQ) submission.
///
/// Sequence:
///   1. Allocate ring buffer (4KB) + LRC (8KB: HWSP + context state)
///   2. Map in GGTT
///   3. Acquire GT forcewake
///   4. Engine reset (RING_RESET_CTL + GEN6_GDRST)
///   5. Enable ExecList mode (i915 enable_execlists + reset_csb_pointers)
///   6. Populate LRC context image (MI_LRI with ring config)
///   7. Build context descriptor + submit via ELSQ
///   8. Probe: MI_NOOP in ring, check HWSP seqno
```

## L1704 · `let ring_phys = memory::allocate_contiguous(1)`

```
// ── Step 1: Allocate memory ─────────────────────────────────
```

## L1707 · `unsafe { core::ptr::write_bytes(ring_phys as *mut u8, 0, 4096); }`

```
// SAFETY: identity-mapped, freshly allocated
```

## L1710-1712 · `let lrc_phys = memory::allocate_contiguous(5)`

```
// LRC = 5 pages: page 0 = HWSP, pages 1-4 = context state + power context save area
// GPU writes additional "power context" state beyond the LRI template on context-save.
// Gen 12 BCS HW context is ~80 DWORDs but save area needs extra pages.
```

## L1715 · `unsafe { core::ptr::write_bytes(lrc_phys as *mut u8, 0, 5 * 4096); }`

```
// SAFETY: identity-mapped, freshly allocated
```

## L1724 · `self.map_pages_ggtt_at(ring_phys, 1, BCS_RING_GGTT);`

```
// ── Step 2: Map in GGTT ─────────────────────────────────────
```

## L1727-1729 · `mmio_write32(self.bar0, GFX_FLSH_CNTL_GEN6, 1);`

```
// Gen 12 GGTT TLB invalidation (i915 guc_ggtt_invalidate):
// GFX_FLSH_CNTL_GEN6 alone is NOT sufficient on Gen 12!
// Must also write GEN12_GUC_TLB_INV_CR + BLT engine TLB.
```

## L1737-1739 · `mmio_write32(self.bar0, FORCEWAKE_GT, (1 << 16) | 1);`

```
// ── Step 3: Acquire ALL forcewake domains ────────────────────
// Request GT + Render + Media (masked bit write: bit 16 = mask, bit 0 = value)
// Posted reads after each write flush the PCIe bus (writes are async/posted).
```

## L1741 · `let _ = mmio_read32(self.bar0, FORCEWAKE_GT); // posted read flush`

```
// posted read flush
```

## L1743 · `let _ = mmio_read32(self.bar0, FORCEWAKE_RENDER); // posted read flush`

```
// posted read flush
```

## L1745 · `let _ = mmio_read32(self.bar0, FORCEWAKE_MEDIA); // posted read flush`

```
// posted read flush
```

## L1747 · `let gt_ok = poll_timeout(self.bar0, FORCEWAKE_ACK_GT, 1, 1, 500_000);`

```
// Poll all three acks
```

## L1758 · `let mode_readback = mmio_read32(self.bar0, BCS_RING_MODE);`

```
// Verify BCS registers are accessible: read RING_MODE + RESET_CTL
```

## L1764-1766 · `mmio_write32(self.bar0, GEN12_GUC_TLB_INV_CR, 1);`

```
// Re-invalidate GGTT TLB NOW that forcewake is held!
// TLB regs at 0xCExx are in FORCEWAKE_GT range — writes without
// forcewake are silently dropped by hardware.
```

## L1772-1773 · `mmio_write32(self.bar0, BCS_RESET_CTL, (1 << 16) | RESET_CTL_REQUEST);`

```
// ── Step 4: Engine reset ────────────────────────────────────
// 4a: Request reset via RING_RESET_CTL (masked write)
```

## L1775 · `let _ = mmio_read32(self.bar0, BCS_RESET_CTL); // posted read flush`

```
// posted read flush
```

## L1779 · `} else {`

```
// Non-fatal: continue anyway
```

## L1783-1784 · `mmio_write32(self.bar0, GEN6_GDRST,`

```
// 4b: Trigger BCS domain reset — GEN6_GDRST is a MASKED register on Gen 11+!
// Must set mask bit (bit 18 = GEN11_GRDOM_BLT << 16) for write to take effect.
```

## L1787 · `let _ = mmio_read32(self.bar0, GEN6_GDRST); // posted read flush`

```
// posted read flush
```

## L1788 · `if !poll_timeout(self.bar0, GEN6_GDRST, GEN11_GRDOM_BLT, 0, 500_000) {`

```
// Poll for reset complete (bit clears when reset done)
```

## L1793-1794 · `mmio_write32(self.bar0, BCS_RESET_CTL, (0x05 << 16) | 0); // mask bits 0+2, clear both`

```
// 4c: Clear reset request AND CAT_ERROR (bits 0 + 2, masked write)
// CAT_ERROR left set could prevent engine from accepting new contexts!
```

## L1795 · `mmio_write32(self.bar0, BCS_RESET_CTL, (0x05 << 16) | 0); // mask bits 0+2, clear both`

```
// mask bits 0+2, clear both
```

## L1796 · `let _ = mmio_read32(self.bar0, BCS_RESET_CTL); // posted read flush`

```
// posted read flush
```

## L1801-1802 · `mmio_write32(self.bar0, BCS_HWSTAM, 0xFFFF_FFFF);`

```
// ── Step 5: Enable ExecList mode (i915 enable_execlists) ─────
// 5a: HWSTAM — mask all HW status interrupts (we poll, don't use IRQs)
```

## L1805-1807 · `let mode_bits = GFX_RUN_LIST_ENABLE | GEN11_GFX_DISABLE_LEGACY_MODE | GFX_PREFETCH_DISABLE;`

```
// 5b: RING_MODE — set both GEN11_GFX_DISABLE_LEGACY_MODE (bit 3) AND
//     GFX_RUN_LIST_ENABLE (bit 15) for ADL-N compatibility.
//     Also set PREFETCH_DISABLE (bit 10) — Gen 12 requirement.
```

## L1811-1813 · `mmio_write32(self.bar0, BCS_MI_MODE, (STOP_RING << 16) | 0);`

```
// 5c: MI_MODE — clear STOP_RING! After reset, command streamer is stopped.
//     Without this, GPU accepts context via ELSQ but never executes ring.
//     i915: ENGINE_WRITE(RING_MI_MODE, _MASKED_BIT_DISABLE(STOP_RING))
```

## L1815 · `let _ = mmio_read32(self.bar0, BCS_MI_MODE); // posted read flush`

```
// posted read flush
```

## L1817 · `mmio_write32(self.bar0, BCS_HWS_PGA, BCS_LRC_GGTT);`

```
// 5d: HWS_PGA — HWSP GGTT address
```

## L1819 · `let _ = mmio_read32(self.bar0, BCS_HWS_PGA); // posted read flush`

```
// posted read flush
```

## L1821-1823 · `mmio_write32(self.bar0, BCS_CTX_STATUS_PTR, 0xFFFF_0B0B);`

```
// 5e: Initialize CSB pointers (i915 reset_csb_pointers)
//     Gen 12 has 12 CSB entries. reset_value = 12 - 1 = 11 (0xB).
//     Format: mask[31:16]=0xFFFF, write_ptr[15:8]=11, read_ptr[7:0]=11
```

## L1825 · `let _ = mmio_read32(self.bar0, BCS_CTX_STATUS_PTR); // posted read flush`

```
// posted read flush
```

## L1827 · `mmio_write32(self.bar0, BCS_IMR, 0xFFFF_FFFF);`

```
// 5f: Mask all interrupts
```

## L1834 · `self.populate_bcs_lrc();`

```
// ── Step 6: Populate LRC + write probe commands ─────────────
```

## L1837 · `let ring = ring_phys as *mut u32;`

```
// Write MI_NOOPs to ring buffer (probe commands)
```

## L1839 · `unsafe {`

```
// SAFETY: ring is identity-mapped, freshly allocated
```

## L1846 · `self.update_lrc_tail(8);`

```
// Set TAIL=8 in LRC BEFORE submit (single submit, no double-submit race)
```

## L1849 · `self.elsq_submit(true);`

```
// ── Step 7: Single ELSQ submit with FORCE_RESTORE ───────────
```

## L1852-1855 · `let lrc_head_ptr = (self.bcs_lrc_phys + 4096 + 5 * 4) as *const u32; // ctx[5] = HEAD val`

```
// ── Step 8: Probe — check LRC + HWSP in RAM ────────────────
// Gen 12 ExecList does NOT update RING_HEAD MMIO live.
// After context runs + saves, GPU writes HEAD back to LRC in RAM.
// We check: (a) LRC HEAD value, (b) HWSP modifications, (c) MMIO as fallback.
```

## L1856 · `let lrc_head_ptr = (self.bcs_lrc_phys + 4096 + 5 * 4) as *const u32; // ctx[5] = HEAD val`

```
// ctx[5] = HEAD val
```

## L1857 · `let hwsp_ptr = self.bcs_lrc_phys as *const u32; // HWSP page 0`

```
// HWSP page 0
```

## L1859 · `let mut probe_ok = false;`

```
// Poll LRC HEAD in system RAM (GPU writes here on context-save)
```

## L1862 · `let lrc_head = unsafe { core::ptr::read_volatile(lrc_head_ptr) };`

```
// SAFETY: lrc_phys is identity-mapped, within our allocation
```

## L1871 · `let lrc_head_final = unsafe { core::ptr::read_volatile(lrc_head_ptr) };`

```
// Read diagnostics
```

## L1900-1914 · `fn populate_bcs_lrc(&self) {`

```
/// Populate BCS Logical Ring Context (LRC) image.
///
/// Gen 12 hardcoded layout: the GPU does NOT execute MI_LRI dynamically.
/// The hardware has a fixed mapping: DWORD N always goes to register X.
/// The register addresses in the LRC are just markers — positions are fixed.
///
/// Layout from gen12_xcs_offsets (i915 intel_lrc.c):
///   [0]  = NOP
///   [1]  = MI_LRI header (13 pairs = 25 DWORDs)
///   [2]  = CTX_CONTEXT_CONTROL addr,  [3]  = value
///   [4]  = RING_HEAD addr,            [5]  = value
///   [6]  = RING_TAIL addr,            [7]  = value  ← update_lrc_tail writes here
///   [8]  = RING_START addr,           [9]  = value
///   [10] = RING_CTL addr,             [11] = value
///   [12..27] = BB_HEAD, BB_STATE, etc. (zeroed)
```

## L1916 · `let ctx = (self.bcs_lrc_phys + 4096) as *mut u32; // page 1 = context state`

```
// page 1 = context state
```

## L1918-1920 · `unsafe {`

```
// SAFETY: lrc_phys is identity-mapped, page 1 is within our 5-page allocation
// ALL writes MUST be write_volatile — GPU reads from RAM but the Rust
// compiler doesn't see the consumer. Non-volatile writes get eliminated.
```

## L1922 · `core::ptr::write_bytes(ctx as *mut u8, 0, 4096);`

```
// Zero entire context page first
```

## L1925 · `ctx.add(0).write_volatile(MI_NOOP);`

```
// Gen 12 hardcoded context layout (order is critical!)
```

## L1928 · `ctx.add(1).write_volatile(MI_LRI_CMD | MI_LRI_FORCE_POSTED | (13 * 2 - 1));`

```
// MI_LRI: 13 register/value pairs, posted
```

## L1931-1936 · `ctx.add(2).write_volatile(0x22244);`

```
// Pair 1: CTX_CONTEXT_CONTROL (must be first!)
// i915 init_common_regs(inhibit=true):
//   _MASKED_BIT_ENABLE(RESTORE_INHIBIT)       → bit 0: set (no saved state)
//   _MASKED_BIT_ENABLE(INHIBIT_SYN_CTX_SWITCH) → bit 3: set
//   _MASKED_BIT_DISABLE(SAVE_INHIBIT)          → bit 2: clear (allow save!)
// mask=0x000D (bits 0,2,3), value=0x0009 (bits 0,3 set, bit 2 clear)
```

## L1940 · `ctx.add(4).write_volatile(BCS_RING_HEAD);`

```
// Pair 2: RING_HEAD
```

## L1944 · `ctx.add(6).write_volatile(BCS_RING_TAIL);`

```
// Pair 3: RING_TAIL (update_lrc_tail writes to ctx[7])
```

## L1948 · `ctx.add(8).write_volatile(BCS_RING_START);`

```
// Pair 4: RING_START
```

## L1952 · `ctx.add(10).write_volatile(BCS_RING_CTL);`

```
// Pair 5: RING_CTL (4KB ring, valid)
```

## L1956 · `ctx.add(12).write_volatile(0x22168); // BBADDR_UDW`

```
// Pairs 6-13: BB regs, CCID, semaphore (matching gen12_xcs_offsets order)
```

## L1957 · `ctx.add(12).write_volatile(0x22168); // BBADDR_UDW`

```
// BBADDR_UDW
```

## L1959 · `ctx.add(14).write_volatile(0x22140); // BBADDR`

```
// BBADDR
```

## L1961 · `ctx.add(16).write_volatile(0x22110); // BB_STATE`

```
// BB_STATE
```

## L1963 · `ctx.add(18).write_volatile(0x221C0); // BB_PER_CTX_PTR`

```
// BB_PER_CTX_PTR
```

## L1965 · `ctx.add(20).write_volatile(0x221C4); // INDIRECT_CTX`

```
// INDIRECT_CTX
```

## L1967 · `ctx.add(22).write_volatile(0x221C8); // INDIRECT_CTX_OFFSET`

```
// INDIRECT_CTX_OFFSET
```

## L1969 · `ctx.add(24).write_volatile(0x22180); // CCID`

```
// CCID
```

## L1971 · `ctx.add(26).write_volatile(0x222B4); // semaphore`

```
// semaphore
```

## L1974-1975 · `ctx.add(28).write_volatile(MI_NOOP);`

```
// ── Second LRI section (gen12_xcs_offsets requires both!) ────
// NOP(5): DWords 28-32
```

## L1982 · `ctx.add(33).write_volatile(MI_LRI_CMD | MI_LRI_FORCE_POSTED | (9 * 2 - 1));`

```
// MI_LRI: 9 register/value pairs (timestamp + status regs)
```

## L1985 · `ctx.add(34).write_volatile(0x223A8); // CTX_TIMESTAMP`

```
// CTX_TIMESTAMP
```

## L1987 · `ctx.add(36).write_volatile(0x2228C); // CTX_STATUS[7]`

```
// CTX_STATUS[7]
```

## L1989 · `ctx.add(38).write_volatile(0x22288); // CTX_STATUS[6]`

```
// CTX_STATUS[6]
```

## L1991 · `ctx.add(40).write_volatile(0x22284); // CTX_STATUS[5]`

```
// CTX_STATUS[5]
```

## L1993 · `ctx.add(42).write_volatile(0x22280); // CTX_STATUS[4]`

```
// CTX_STATUS[4]
```

## L1995 · `ctx.add(44).write_volatile(0x2227C); // CTX_STATUS[3]`

```
// CTX_STATUS[3]
```

## L1997 · `ctx.add(46).write_volatile(0x22278); // CTX_STATUS[2]`

```
// CTX_STATUS[2]
```

## L1999 · `ctx.add(48).write_volatile(0x22274); // CTX_STATUS[1]`

```
// CTX_STATUS[1]
```

## L2001 · `ctx.add(50).write_volatile(0x22270); // CTX_STATUS[0]`

```
// CTX_STATUS[0]
```

## L2006-2010 · `fn update_lrc_tail(&self, tail_bytes: u32) {`

```
/// Update RING_TAIL in LRC and reset HEAD to 0 (stateless ring hack).
///
/// MUST use write_volatile — the GPU reads this from RAM, but the
/// Rust compiler doesn't know that. Non-volatile writes get eliminated
/// as "dead stores" in release builds.
```

## L2013 · `unsafe {`

```
// SAFETY: within our allocated LRC page
```

## L2015 · `ctx.add(5).write_volatile(0);             // Reset HEAD to 0`

```
// Reset HEAD to 0
```

## L2016 · `ctx.add(7).write_volatile(tail_bytes);    // Set new TAIL`

```
// Set new TAIL
```

## L2021-2022 · `fn elsq_submit(&self, _force_restore: bool) {`

```
/// Submit BCS context via ELSQ. Always uses FORCE_RESTORE for
/// stateless ring operation (HEAD/TAIL reset from LRC each time).
```

## L2024-2025 · `let lrca_ggtt = BCS_LRC_GGTT; // page 0!`

```
// LRCA = page 0 of LRC (HWSP). GPU adds +4096 for context state.
// Bug was: we pointed at page 1, GPU read uninitialized memory as context.
```

## L2026 · `let lrca_ggtt = BCS_LRC_GGTT; // page 0!`

```
// page 0!
```

## L2028-2030 · `let desc_lo: u32 = lrca_ggtt`

```
// i915 lrc_descriptor(): LRCA | LEGACY_32B | CTX_VALID = LRCA | 0x9
// Bit 8 is NOT privilege on Gen 12 — it's GEN12_CTX_CTRL_OAR_CONTEXT_ENABLE
// FORCE_RESTORE only needed when TAIL unchanged on re-submit (i915 fallback)
```

## L2032 · `| CTX_LEGACY_32B         // bit 3 (i915 default on Gen 12)`

```
// bit 3 (i915 default on Gen 12)
```

## L2033 · `| CTX_VALID;             // bit 0`

```
// bit 0
```

## L2035 · `let desc_hi: u32 = 0;  // i915: upper 32 bits = context ID (0 for single context)`

```
// i915: upper 32 bits = context ID (0 for single context)
```

## L2044 · `fn map_pages_ggtt_at(&self, phys: u64, pages: u32, ggtt_offset: u32) {`

```
/// Map contiguous physical pages into GGTT at a given offset.
```

## L2049 · `let entry_lo = (page_phys as u32 & 0xFFFF_F000) | 0x01; // valid, system mem`

```
// valid, system mem
```

## L2057 · `pub fn map_shadows(&mut self, phys_a: u64, phys_b: u64, pages: u32) {`

```
/// Map shadow buffers into GGTT for BCS access.
```

## L2067 · `mmio_write32(self.bar0, GFX_FLSH_CNTL_GEN6, 1);`

```
// Gen 12 GGTT TLB invalidation
```

## L2081-2089 · `fn verify_blit_readback(&mut self) -> bool {`

```
/// Submit XY_FAST_COPY_BLT via ELSQ context re-submission.
/// Prove the BCS engine actually PAINTS: blit a known pattern between two
/// scratch GGTT buffers (NOT the live scanout — invisible, safe), then
/// CPU-read the destination. Returns true only if the bytes arrived. On
/// Tiger Lake the ring advances but no pixels land, so the ring-head
/// check in `submit_blit` is not enough; this readback is the real proof.
/// Also a bring-up oracle: FAIL here = the copy itself is broken (command
/// / context / GGTT); PASS-but-screen-black = a scanout/flip targeting
/// problem instead.
```

## L2094 · `const SRC_GGTT: u32 = 0x0400_3000; // reuse BCS_TEST_GGTT slot`

```
// reuse BCS_TEST_GGTT slot
```

## L2099 · `unsafe {`

```
// SAFETY: freshly allocated, identity-mapped, one page each.
```

## L2111 · `self.submit_blit(SRC_GGTT, 256, DST_GGTT, 256, 0, 0, 16, 16);`

```
// Copy a 16×16 px block src→dst (64 px/row → 256 B pitch).
```

## L2114-2117 · `unsafe {`

```
// The dst page is WB identity-mapped and we just wrote zeros to it, so
// they sit in cache. The GPU wrote the real bytes to memory via GGTT —
// flush the cached zeros so the CPU re-reads from memory.
// SAFETY: dst_phys is identity-mapped; clflush on a mapped address.
```

## L2144 · `let ring = self.bcs_ring_phys as *mut u32;`

```
// Write XY_FAST_COPY_BLT to ring buffer at offset 0
```

## L2146 · `unsafe {`

```
// SAFETY: ring is identity-mapped, within allocated page
```

## L2163 · `let tail_bytes = 48u32; // 12 DWORDs (10 cmd + 2 noop padding)`

```
// 12 DWORDs (10 cmd + 2 noop padding)
```

## L2165 · `self.update_lrc_tail(tail_bytes);`

```
// Update tail in LRC and re-submit context
```

## L2169-2171 · `let lrc_head_ptr = (self.bcs_lrc_phys + 4096 + 5 * 4) as *const u32; // ctx[5] = HEAD val`

```
// Poll LRC HEAD in RAM (not MMIO — MMIO RING_HEAD is unreliable on Gen 12).
// GPU writes HEAD back to LRC on context-save. This is the same approach
// used in the init probe, and avoids race conditions with cursor overlay.
```

## L2172 · `let lrc_head_ptr = (self.bcs_lrc_phys + 4096 + 5 * 4) as *const u32; // ctx[5] = HEAD val`

```
// ctx[5] = HEAD val
```

## L2174 · `let head = unsafe { core::ptr::read_volatile(lrc_head_ptr) };`

```
// SAFETY: lrc_phys is identity-mapped, within our 5-page allocation
```

## L2184 · `pub fn test_blit(&mut self) -> bool {`

```
/// Visual test: blit a 64×64 magenta square via BCS.
```

## L2197 · `let test_phys = match memory::allocate_contiguous(4) {`

```
// Allocate + fill test surface
```

## L2203 · `unsafe {`

```
// SAFETY: freshly allocated, identity-mapped
```

## L2211 · `let ring = self.bcs_ring_phys as *mut u32;`

```
// Write blit command
```

## L2213 · `unsafe {`

```
// SAFETY: ring is our allocated page
```

## L2233 · `let lrc_head_ptr = (self.bcs_lrc_phys + 4096 + 5 * 4) as *const u32;`

```
// Poll LRC HEAD in RAM (reliable on Gen 12, unlike MMIO RING_HEAD)
```

## L2252 · `pub fn fb_ggtt(&self) -> u32 {`

```
/// Get the GGTT offset of the framebuffer (for BCS destination).
```

## L2257 · `pub fn shadow_ggtt(&self) -> (u32, u32) {`

```
/// Get shadow GGTT offsets (A, B).
```

## L2262 · `pub fn bcs_ready(&self) -> bool {`

```
/// Check if BCS is initialized and ready.
```

## L2267 · `fn enable_scrambling(&self) -> bool {`

```
// ── HDMI 2.0 Scrambling ─────────────────────────────────────────
```

## L2269-2271 · `fn enable_scrambling(&self) -> bool {`

```
/// Enable HDMI 2.0 scrambling for TMDS >340 MHz (required for 4K@60).
/// Follows i915 sequence: configure sink (SCDC) FIRST, then source (transcoder).
/// Retries SCDC writes if monitor isn't connected yet (HDMI input switching).
```

## L2275-2278 · `let mut scdc_ok = false;`

```
// Step 1: Tell the monitor to enable scrambling via SCDC I2C (BEFORE source).
// i915 does this in intel_hdmi_handle_sink_scrambling() before DDI enable.
// Retry up to 10 times with ~500ms pause — monitor may not be
// connected yet (e.g. HDMI input auto-switching during reboot).
```

## L2283 · `if self.gmbus_write_byte(SCDC_I2C_ADDR, SCDC_TMDS_CONFIG, 0x03) {`

```
// SCDC TMDS_Config (0x20): bit 0 = scrambling, bit 1 = clock ratio 1/40
```

## L2293 · `for _ in 0..50_000_000u32 { core::hint::spin_loop(); }`

```
// ~500ms pause
```

## L2303-2305 · `let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);`

```
// Step 2: Cycle TRANS_DDI_FUNC_CTL: disable, switch DVI→HDMI, enable scrambling.
// i915 does a full disable/reconfigure/enable cycle (intel_ddi_disable_transcoder_func
// + intel_ddi_enable_transcoder_func). Just flipping bits in-place doesn't work.
```

## L2309 · `mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, 0);`

```
// Disable transcoder DDI function
```

## L2313 · `let ddi_ctl = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };`

```
// Also disable + re-enable DDI buffer for clean handshake
```

## L2318 · `let _ = poll_timeout(self.bar0, ddi_ctl, 1 << 7, 1 << 7, 200_000);`

```
// Wait for DDI idle (bit 7 = 1 when idle)
```

## L2324 · `mmio_write32(self.bar0, ddi_ctl, ddi_buf | (1 << 31));`

```
// Re-enable DDI buffer
```

## L2329 · `let new_func = (ddi_func & !TRANS_DDI_MODE_MASK & !TRANS_DDI_SCRAMBLING_MASK)`

```
// Write new TRANS_DDI_FUNC_CTL: HDMI mode + scrambling + enable
```

## L2333 · `| (1 << 31);  // enable`

```
// enable
```

## L2338 · `for _ in 0..20_000_000u32 { core::hint::spin_loop(); }`

```
// Step 3: Wait for monitor to lock to scrambled signal (~200ms)
```

## L2341 · `match self.gmbus_read_byte(SCDC_I2C_ADDR, SCDC_SCRAMBLER_STATUS) {`

```
// Step 4: Check scrambler status
```

## L2358 · `fn disable_scrambling(&self) {`

```
/// Disable HDMI 2.0 scrambling (for modes <=340 MHz TMDS).
```

## L2360 · `let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);`

```
// Clear scrambling bits and restore DVI mode
```

## L2366 · `self.gmbus_reset();`

```
// Tell monitor to disable scrambling
```

