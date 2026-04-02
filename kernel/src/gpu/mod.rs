//! GPU Abstraction Layer
//!
//! Vendor-neutral GPU driver interface. Detects available GPU hardware
//! at boot and provides a unified API for display output.
//!
//! Backends:
//! - GOP: UEFI Graphics Output Protocol (fallback, bootloader-provided)
//! - Intel Xe: Native modesetting for Alder Lake / Gen 12.2 (N100)

#![allow(dead_code)]

pub mod gop;
pub mod intel_xe;

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

/// Active GPU driver instance.
enum GpuBackend {
    None,
    Gop(gop::GopDriver),
    IntelXe(intel_xe::IntelXeDriver),
}

static GPU: Mutex<GpuBackend> = Mutex::new(GpuBackend::None);

/// Initialize GPU subsystem. Uses GOP, detects native GPU for later activation.
pub fn init(multiboot_info: u32) {
    // Always start with GOP (safe, bootloader-provided framebuffer)
    match gop::GopDriver::from_multiboot2(multiboot_info) {
        Some(drv) => {
            crate::kprintln!("[npk] GPU: GOP {}x{} (bootloader)",
                drv.framebuffer().width, drv.framebuffer().height);
            *GPU.lock() = GpuBackend::Gop(drv);
        }
        None => {
            crate::kprintln!("[npk] GPU: no display available");
        }
    }

    // Detect native GPU (PCI scan only — no hardware init yet)
    match intel_xe::IntelXeDriver::detect() {
        Some(drv) => {
            crate::kprintln!("[npk] GPU: {} available (use 'gpu init' to activate)",
                drv.name());
            *DETECTED_XE.lock() = Some(drv);
        }
        None => {}
    }
}

/// Detected but not yet initialized Intel Xe driver.
static DETECTED_XE: Mutex<Option<intel_xe::IntelXeDriver>> = Mutex::new(None);

/// Activate native GPU driver (Intel Xe). Call from intent loop, not boot.
/// Returns description of result for user output.
pub fn activate_native() -> Result<FramebufferInfo, GpuError> {
    let mut detected = DETECTED_XE.lock();
    let mut drv = detected.take().ok_or(GpuError::NotFound)?;

    match drv.init() {
        Ok(fb) => {
            crate::kprintln!("[npk] GPU: {}x{} @ {}Hz (native)",
                fb.width, fb.height, drv.current_hz());
            *GPU.lock() = GpuBackend::IntelXe(drv);
            Ok(fb)
        }
        Err(e) => {
            crate::kprintln!("[npk] GPU: Intel Xe init failed: {:?}", e);
            // Put driver back so user can retry
            *detected = Some(drv);
            Err(e)
        }
    }
}

/// Check if a native GPU was detected (but not necessarily activated).
pub fn native_detected() -> bool {
    DETECTED_XE.lock().is_some()
}

/// Name of detected native GPU (if any).
pub fn native_gpu_name() -> Option<&'static str> {
    DETECTED_XE.lock().as_ref().map(|d| d.name())
}

/// Get current framebuffer info (if any GPU is active).
pub fn framebuffer_info() -> Option<FramebufferInfo> {
    let gpu = GPU.lock();
    match &*gpu {
        GpuBackend::None => None,
        GpuBackend::Gop(drv) => Some(drv.framebuffer()),
        GpuBackend::IntelXe(drv) => Some(drv.framebuffer()),
    }
}

/// Try to set a new display mode. Returns new framebuffer info on success.
pub fn set_mode(width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {
    let mut gpu = GPU.lock();
    match &mut *gpu {
        GpuBackend::None => Err(GpuError::NotFound),
        GpuBackend::Gop(_) => Err(GpuError::UnsupportedMode), // GOP can't switch modes
        GpuBackend::IntelXe(drv) => drv.set_mode(width, height, hz),
    }
}

/// List supported display modes.
pub fn supported_modes() -> alloc::vec::Vec<ModeInfo> {
    let gpu = GPU.lock();
    match &*gpu {
        GpuBackend::None => alloc::vec::Vec::new(),
        GpuBackend::Gop(drv) => drv.supported_modes(),
        GpuBackend::IntelXe(drv) => drv.supported_modes(),
    }
}

/// Get name of active GPU driver.
pub fn driver_name() -> &'static str {
    let gpu = GPU.lock();
    match &*gpu {
        GpuBackend::None => "none",
        GpuBackend::Gop(_) => "GOP",
        GpuBackend::IntelXe(drv) => drv.name(),
    }
}

/// Check if a native GPU driver is active (not just GOP fallback).
pub fn is_native() -> bool {
    let gpu = GPU.lock();
    matches!(&*gpu, GpuBackend::IntelXe(_))
}
