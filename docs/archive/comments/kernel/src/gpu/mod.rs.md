# `kernel/src/gpu/mod.rs` @ 5e0102684

## L1-9 · `#![allow(dead_code)]`

```
//! GPU Hardware Abstraction Layer
//!
//! Driver-agnostic GPU interface via GpuHal trait.
//! Any GPU driver (Intel, AMD, NVIDIA) implements the trait.
//! The kernel only talks to the trait, never to hardware directly.
//!
//! Backends:
//! - GOP: UEFI Graphics Output Protocol (fallback, bootloader-provided)
//! - Intel Xe: Native modesetting for Alder Lake / Gen 12.2 (N100)
```

## L22 · `#[derive(Debug)]`

```
/// GPU driver error.
```

## L25 · `NotFound,`

```
/// No GPU hardware found
```

## L27 · `MappingFailed,`

```
/// PCI/BAR mapping failed
```

## L29 · `PowerTimeout,`

```
/// Power well enable timed out
```

## L31 · `PllLockFailed,`

```
/// PLL failed to lock
```

## L33 · `UnsupportedMode,`

```
/// Requested mode not supported
```

## L35 · `PipelineFailed,`

```
/// Display pipeline enable failed
```

## L37 · `AllocFailed,`

```
/// Framebuffer allocation failed
```

## L41 · `#[derive(Debug, Clone, Copy)]`

```
/// Display mode description.
```

## L49 · `#[derive(Debug, Clone, Copy)]`

```
/// Framebuffer provided by the GPU driver.
```

## L52 · `pub addr: u64,`

```
/// Physical address for CPU writes (MMIO or identity-mapped RAM)
```

## L54 · `pub pitch: u32,`

```
/// Bytes per scanline
```

## L56 · `pub width: u32,`

```
/// Width in pixels
```

## L58 · `pub height: u32,`

```
/// Height in pixels
```

## L60 · `pub bpp: u8,`

```
/// Bits per pixel (typically 32)
```

## L64 · `pub trait GpuHal: Send {`

```
// ── GpuHal Trait — driver-agnostic interface ──────────────
```

## L66-67 · `pub trait GpuHal: Send {`

```
/// Hardware Abstraction Layer for GPU drivers.
/// Implement this trait for any GPU: Intel, AMD, NVIDIA, virtual.
```

## L69 · `fn name(&self) -> &'static str;`

```
/// Human-readable driver name (e.g. "Intel Xe ADL-N").
```

## L72 · `fn set_mode(&mut self, width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError>;`

```
/// Set display mode. Returns framebuffer info for CPU rendering.
```

## L75 · `fn framebuffer(&self) -> FramebufferInfo;`

```
/// Current framebuffer info.
```

## L78 · `fn supported_modes(&self) -> Vec<ModeInfo>;`

```
/// List of supported display modes.
```

## L81 · `fn current_hz(&self) -> u8;`

```
/// Current refresh rate in Hz (0 if unknown).
```

## L84 · `fn is_native(&self) -> bool;`

```
/// True if this is a native driver (not GOP fallback).
```

## L87-90 · `fn supports_modeset(&self) -> bool { false }`

```
/// True if a real modeset (DPLL/pipe/transcoder reprogram) is validated on
/// this GPU. False for GOP and for blit-only takeovers (e.g. non-ADL-N
/// Gen12), where set_mode would reprogram an unvalidated pipeline and black
/// the scanout — those keep the firmware mode. Conservative default: false.
```

## L93-94 · `fn flip(&mut self, surface_addr: u64);`

```
/// Schedule page flip — GPU scans from new surface at next vblank.
/// `surface_addr` is driver-specific (GGTT offset for Intel, phys for others).
```

## L97 · `fn wait_vblank(&self);`

```
/// Wait for vertical blank (synchronous). Returns immediately if unsupported.
```

## L100 · `fn supports_flip(&self) -> bool;`

```
/// True if the driver supports hardware page flip + vblank sync.
```

## L103 · `fn init_blit_engine(&mut self) -> bool { false }`

```
/// Initialize hardware blit engine (BCS). Returns true on success.
```

## L106 · `fn supports_blit(&self) -> bool { false }`

```
/// True if hardware blit (BCS) is available.
```

## L109 · `fn blit_verified(&self) -> bool { false }`

```
/// True if the BCS blit passed its readback self-test (pixels land).
```

## L111 · `fn blit_readback(&self) -> u32 { 0 }`

```
/// Last readback self-test dst[0] value (diagnostic).
```

## L114 · `fn blit_rect_hw(`

```
/// Submit a rectangular blit from src GGTT → dst GGTT.
```

## L121 · `fn map_for_blit(&mut self, phys_a: u64, phys_b: u64, pages: u32) {`

```
/// Map physical buffer into GPU address space for blit. Returns GGTT offset.
```

## L126 · `fn fb_gpu_addr(&self) -> u32 { 0 }`

```
/// Get framebuffer GGTT offset (for BCS destination).
```

## L129 · `fn shadow_gpu_addr(&self) -> (u32, u32) { (0, 0) }`

```
/// Get shadow buffer GGTT offsets (A, B). 0 = not mapped.
```

## L132 · `fn test_blit(&mut self) -> bool { false }`

```
/// Visual BCS test — blit a colored square to screen.
```

## L135 · `fn dump_bcs_regs(&self) {}`

```
/// Dump BCS engine registers for debug (printed via kprintln).
```

## L141 · `pub fn init(boot_info: &crate::boot_info::BootInfo) {`

```
/// Initialize GPU subsystem. Uses GOP, detects native GPU for later activation.
```

## L143 · `match gop::GopDriver::from_boot_info(boot_info) {`

```
// Always start with GOP (safe, UEFI-provided framebuffer)
```

## L155 · `match intel_xe::IntelXeDriver::detect() {`

```
// Detect native GPU (PCI scan only — no hardware init yet)
```

## L166 · `static DETECTED_XE: Mutex<Option<intel_xe::IntelXeDriver>> = Mutex::new(None);`

```
/// Detected but not yet initialized Intel Xe driver.
```

## L169 · `pub fn activate_native() -> Result<FramebufferInfo, GpuError> {`

```
/// Activate native GPU driver (Intel Xe). Call from intent loop, not boot.
```

## L189 · `pub fn dump_native() {`

```
/// Dump native GPU registers (Intel Xe specific, debug only).
```

## L199 · `pub fn test_pll() {`

```
/// Test PLL re-lock (Intel Xe specific, debug only).
```

## L209 · `pub fn native_detected() -> bool {`

```
/// Check if a native GPU was detected (but not necessarily activated).
```

## L214 · `pub fn native_gpu_name() -> Option<&'static str> {`

```
/// Name of detected native GPU (if any).
```

## L219-221 · `pub fn native_auto_activate_ok() -> bool {`

```
/// Whether the detected GPU may be AUTO-activated at boot. Only validated
/// generations (ADL-N) qualify; others (Tiger Lake) are manual-only for
/// bring-up so a non-painting BCS blit can't black the desktop at every boot.
```

## L226 · `pub fn blit_verified() -> bool {`

```
/// True if the active GPU's BCS blit passed its readback self-test.
```

## L231 · `pub fn blit_readback() -> u32 {`

```
/// Last BCS readback self-test dst[0] value (diagnostic).
```

## L236 · `pub fn framebuffer_info() -> Option<FramebufferInfo> {`

```
/// Get current framebuffer info (if any GPU is active).
```

## L241 · `pub fn set_mode(width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {`

```
/// Try to set a new display mode.
```

## L249 · `pub fn supported_modes() -> Vec<ModeInfo> {`

```
/// List supported display modes.
```

## L257 · `pub fn driver_name() -> &'static str {`

```
/// Get name of active GPU driver.
```

## L265 · `pub fn current_hz() -> u8 {`

```
/// Current refresh rate (0 if unknown/GOP).
```

## L273 · `pub fn is_native() -> bool {`

```
/// Check if a native GPU driver is active (not just GOP fallback).
```

## L281-282 · `pub fn supports_modeset() -> bool {`

```
/// True if a real modeset is validated on the active GPU (false for GOP and
/// blit-only takeovers, which must keep the firmware mode).
```

## L290 · `pub fn wait_vblank() {`

```
/// Wait for vertical blank (pass-through to HAL).
```

## L297 · `pub fn supports_flip() -> bool {`

```
/// Check if hardware flip is supported.
```

## L305 · `pub fn init_blit_engine() -> bool {`

```
// ── BCS Blit API ────────────────────────────────────────────────────
```

## L307 · `pub fn init_blit_engine() -> bool {`

```
/// Initialize hardware blit engine. Call after activate_native().
```

## L315 · `pub fn supports_blit() -> bool {`

```
/// True if hardware blit (BCS) is available.
```

## L323 · `pub fn gpu_blit_rect(`

```
/// Submit GPU blit: copy rectangle from src GGTT → dst GGTT.
```

## L335 · `pub fn map_shadows_for_blit(phys_a: u64, phys_b: u64, pages: u32) {`

```
/// Map shadow buffers into GPU address space for blit.
```

## L342 · `pub fn fb_ggtt_offset() -> u32 {`

```
/// Get framebuffer GGTT offset (for BCS destination).
```

## L350 · `pub fn shadow_ggtt() -> (u32, u32) {`

```
/// Get shadow buffer GGTT offsets (A, B).
```

## L358 · `pub fn test_blit() -> bool {`

```
/// Visual BCS test.
```

## L366 · `pub fn dump_bcs_regs() {`

```
/// Dump BCS register state for debug.
```

## L373 · `static GPU_LOG_SEQ: Mutex<u32> = Mutex::new(0);`

```
/// Auto-incrementing GPU log counter.
```

## L376 · `pub fn next_log_name() -> alloc::string::String {`

```
/// Generate a unique GPU log name: gpu-log-NNN-YYYY-MM-DD
```

## L382 · `if let Some(unix) = crate::rtc::read_unix_time() {`

```
// Try RTC for date, fallback to seq-only
```

## L384 · `let days = unix / 86400;`

```
// Convert unix timestamp to date
```

