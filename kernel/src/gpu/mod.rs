//! GPU Hardware Abstraction Layer
//!
//! Driver-agnostic GPU interface via GpuHal trait.
//! Any GPU driver (Intel, AMD, NVIDIA) implements the trait.
//! The kernel only talks to the trait, never to hardware directly.
//!
//! Backends:
//! - GOP: UEFI Graphics Output Protocol, the CPU path every machine has
//! - gpudev: a GPU driver module that serves the compositor's copies
//!   (docs/plan/GPU_STACK.md §3.3)

#![allow(dead_code)]

pub mod gop;
pub mod gpudev;
mod stolen_ids;

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::Mutex;

/// GPU driver error.
#[derive(Debug)]
pub enum GpuError {
    /// No GPU hardware found
    NotFound,
    /// PCI/BAR mapping failed
    MappingFailed,
    /// Power well enable timed out
    PowerTimeout,
    /// PLL failed to lock
    PllLockFailed,
    /// Requested mode not supported
    UnsupportedMode,
    /// Display pipeline enable failed
    PipelineFailed,
    /// Framebuffer allocation failed
    AllocFailed,
    /// The device belongs to a driver module
    Owned,
}

/// Display mode description.
#[derive(Debug, Clone, Copy)]
pub struct ModeInfo {
    pub width: u32,
    pub height: u32,
    pub hz: u8,
}

/// Framebuffer provided by the GPU driver.
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Physical address for CPU writes (MMIO or identity-mapped RAM)
    pub addr: u64,
    /// Bytes per scanline
    pub pitch: u32,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Bits per pixel (typically 32)
    pub bpp: u8,
}

// ── GpuHal Trait — driver-agnostic interface ──────────────

/// Hardware Abstraction Layer for GPU drivers.
/// Implement this trait for any GPU: Intel, AMD, NVIDIA, virtual.
pub trait GpuHal: Send {
    /// Human-readable driver name.
    fn name(&self) -> &'static str;

    /// Set display mode. Returns framebuffer info for CPU rendering.
    fn set_mode(&mut self, width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError>;

    /// Current framebuffer info.
    fn framebuffer(&self) -> FramebufferInfo;

    /// List of supported display modes.
    fn supported_modes(&self) -> Vec<ModeInfo>;

    /// Current refresh rate in Hz (0 if unknown).
    fn current_hz(&self) -> u8;

    /// True if this is a native driver (not GOP fallback).
    fn is_native(&self) -> bool;

    /// True if a real modeset (DPLL/pipe/transcoder reprogram) is validated on
    /// this GPU. False for GOP and for blit-only takeovers (e.g. non-ADL-N
    /// Gen12), where set_mode would reprogram an unvalidated pipeline and black
    /// the scanout — those keep the firmware mode. Conservative default: false.
    fn supports_modeset(&self) -> bool { false }

    /// Schedule page flip — GPU scans from new surface at next vblank.
    /// `surface_addr` is driver-specific (GGTT offset for Intel, phys for others).
    fn flip(&mut self, surface_addr: u64);

    /// Wait for vertical blank (synchronous). Returns immediately if unsupported.
    fn wait_vblank(&self);

    /// True if the driver supports hardware page flip + vblank sync.
    fn supports_flip(&self) -> bool;

    /// True if hardware blit (BCS) is available.
    fn supports_blit(&self) -> bool { false }

    /// Submit a rectangular blit from src GGTT → dst GGTT.
    fn blit_rect_hw(
        &mut self, src_ggtt: u32, src_pitch: u32,
        dst_ggtt: u32, dst_pitch: u32,
        x: u32, y: u32, w: u32, h: u32,
    ) -> bool { let _ = (src_ggtt, src_pitch, dst_ggtt, dst_pitch, x, y, w, h); false }

    /// Get framebuffer GGTT offset (for BCS destination).
    fn fb_gpu_addr(&self) -> u32 { 0 }

    /// Get shadow buffer GGTT offsets (A, B). 0 = not mapped.
    fn shadow_gpu_addr(&self) -> (u32, u32) { (0, 0) }
}

static GPU: Mutex<Option<Box<dyn GpuHal>>> = Mutex::new(None);

/// Initialize GPU subsystem. Uses GOP, detects native GPU for later activation.
pub fn init(boot_info: &crate::boot_info::BootInfo) {
    // Always start with GOP (safe, UEFI-provided framebuffer)
    match gop::GopDriver::from_boot_info(boot_info) {
        Some(drv) => {
            crate::kdebug!("[npk] GPU: GOP {}x{} (bootloader)",
                drv.framebuffer().width, drv.framebuffer().height);
            *GPU.lock() = Some(Box::new(drv) as Box<dyn GpuHal>);
        }
        None => {
            crate::kprintln!("[npk] GPU: no display available");
        }
    }
}


/// Whether a driver module may take the card at `addr`. The kernel drives
/// no GPU itself (GOP draws without touching the card), so a display
/// controller nobody owns is free.
pub fn yield_to_module(addr: crate::drivers::pci::PciAddr) -> bool {
    crate::drivers::pci::owner(addr).is_none()
}

/// Make `drv` the active GPU driver (gpudev on a module's ready, GOP when
/// it goes away).
pub(crate) fn install(drv: Box<dyn GpuHal>) {
    *GPU.lock() = Some(drv);
}

/// Get current framebuffer info (if any GPU is active).
pub fn framebuffer_info() -> Option<FramebufferInfo> {
    GPU.lock().as_ref().map(|drv| drv.framebuffer())
}

/// Try to set a new display mode.
pub fn set_mode(width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {
    match GPU.lock().as_mut() {
        Some(drv) => drv.set_mode(width, height, hz),
        None => Err(GpuError::NotFound),
    }
}

/// List supported display modes.
pub fn supported_modes() -> Vec<ModeInfo> {
    match GPU.lock().as_ref() {
        Some(drv) => drv.supported_modes(),
        None => Vec::new(),
    }
}

/// Get name of active GPU driver.
pub fn driver_name() -> &'static str {
    match GPU.lock().as_ref() {
        Some(drv) => drv.name(),
        None => "none",
    }
}

/// Current refresh rate (0 if unknown/GOP).
pub fn current_hz() -> u8 {
    match GPU.lock().as_ref() {
        Some(drv) => drv.current_hz(),
        None => 0,
    }
}

/// Check if a native GPU driver is active (not just GOP fallback).
pub fn is_native() -> bool {
    match GPU.lock().as_ref() {
        Some(drv) => drv.is_native(),
        None => false,
    }
}

/// True if a real modeset is validated on the active GPU (false for GOP and
/// blit-only takeovers, which must keep the firmware mode).
pub fn supports_modeset() -> bool {
    match GPU.lock().as_ref() {
        Some(drv) => drv.supports_modeset(),
        None => false,
    }
}

/// Wait for vertical blank (pass-through to HAL).
pub fn wait_vblank() {
    if let Some(drv) = GPU.lock().as_ref() {
        drv.wait_vblank();
    }
}

/// Check if hardware flip is supported.
pub fn supports_flip() -> bool {
    match GPU.lock().as_ref() {
        Some(drv) => drv.supports_flip(),
        None => false,
    }
}

/// True if the active driver copies the compositor's frames on the GPU.
pub fn supports_blit() -> bool {
    match GPU.lock().as_ref() {
        Some(drv) => drv.supports_blit(),
        None => false,
    }
}

/// Submit GPU blit: copy rectangle from src GGTT → dst GGTT.
pub fn gpu_blit_rect(
    src_ggtt: u32, src_pitch: u32,
    dst_ggtt: u32, dst_pitch: u32,
    x: u32, y: u32, w: u32, h: u32,
) -> bool {
    match GPU.lock().as_mut() {
        Some(drv) => drv.blit_rect_hw(src_ggtt, src_pitch, dst_ggtt, dst_pitch, x, y, w, h),
        None => false,
    }
}

/// Get framebuffer GGTT offset (for BCS destination).
pub fn fb_ggtt_offset() -> u32 {
    match GPU.lock().as_ref() {
        Some(drv) => drv.fb_gpu_addr(),
        None => 0,
    }
}

/// Get shadow buffer GGTT offsets (A, B).
pub fn shadow_ggtt() -> (u32, u32) {
    match GPU.lock().as_ref() {
        Some(drv) => drv.shadow_gpu_addr(),
        None => (0, 0),
    }
}

/// The Intel part of Linux's x86 early quirks (`intel_graphics_quirks`,
/// `gen11_early_ops`): the integrated GPU's stolen memory is taken out of
/// the frame allocator, whatever the firmware's map says. The GPU keeps
/// its framebuffer and its own reserved area there; a frame handed out of
/// it would be shared with the device. A firmware map that types any of it
/// as RAM is reported, since frames from it may already be in use.
pub fn reserve_intel_stolen() {
    use crate::drivers::pci;
    let dev = pci::PciAddr { bus: 0, device: 2, function: 0 };
    let id = pci::read32(dev, 0x00);
    if id & 0xffff != 0x8086 {
        return;
    }
    let device = (id >> 16) as u16;
    if !stolen_ids::GEN11_STOLEN_IDS.contains(&device) {
        return;
    }
    // gen9_stolen_size. Names from include/drm/intel/i915_drm.h:
    // SNB_GMCH_CTRL 0x50, BDW_GMCH_GMS_SHIFT 8, BDW_GMCH_GMS_MASK 0xff.
    let gmch_ctrl = pci::read16(dev, 0x50);
    let gms = ((gmch_ctrl >> 8) & 0xff) as u64;
    let size = if gms < 0xf0 { gms * (32 << 20) } else { (gms - 0xf0) * (4 << 20) + (4 << 20) };
    // gen11_stolen_base: INTEL_GEN11_BSM_DW0 0xc0 masked with
    // INTEL_BSM_MASK, INTEL_GEN11_BSM_DW1 0xc4 as the upper half.
    let base = (pci::read32(dev, 0xc0) & 0xfff0_0000) as u64 | (pci::read32(dev, 0xc4) as u64) << 32;
    if size == 0 || base == 0 {
        return;
    }
    if crate::mm::memory::firmware_ram_in(base, size) {
        crate::kprintln!("[npk] GPU: stolen memory {:#x}+{} MB is RAM in the firmware map; reserving it now",
            base, size >> 20);
    }
    crate::mm::memory::reserve_region(base, size);
    crate::kdebug!("[npk] GPU: reserved stolen memory {:#x}+{} MB", base, size >> 20);
}
