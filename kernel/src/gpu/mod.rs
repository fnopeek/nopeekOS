//! GPU Hardware Abstraction Layer
//!
//! Driver-agnostic GPU interface via GpuHal trait.
//! Any GPU driver (Intel, AMD, NVIDIA) implements the trait.
//! The kernel only talks to the trait, never to hardware directly.
//!
//! Backends:
//! - GOP: UEFI Graphics Output Protocol (fallback, bootloader-provided)
//! - Intel Xe: Native modesetting for Alder Lake / Gen 12.2 (N100)

#![allow(dead_code)]

pub mod gop;
pub mod intel_xe;

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
    /// Human-readable driver name (e.g. "Intel Xe ADL-N").
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

    /// Schedule page flip — GPU scans from new surface at next vblank.
    /// `surface_addr` is driver-specific (GGTT offset for Intel, phys for others).
    fn flip(&mut self, surface_addr: u64);

    /// Wait for vertical blank (synchronous). Returns immediately if unsupported.
    fn wait_vblank(&self);

    /// True if the driver supports hardware page flip + vblank sync.
    fn supports_flip(&self) -> bool;
}

static GPU: Mutex<Option<Box<dyn GpuHal>>> = Mutex::new(None);

/// Initialize GPU subsystem. Uses GOP, detects native GPU for later activation.
pub fn init(multiboot_info: u32) {
    // Always start with GOP (safe, bootloader-provided framebuffer)
    match gop::GopDriver::from_multiboot2(multiboot_info) {
        Some(drv) => {
            crate::kprintln!("[npk] GPU: GOP {}x{} (bootloader)",
                drv.framebuffer().width, drv.framebuffer().height);
            *GPU.lock() = Some(Box::new(drv) as Box<dyn GpuHal>);
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
pub fn activate_native() -> Result<FramebufferInfo, GpuError> {
    let mut detected = DETECTED_XE.lock();
    let mut drv = detected.take().ok_or(GpuError::NotFound)?;

    match drv.init() {
        Ok(fb) => {
            crate::kprintln!("[npk] GPU: {}x{} @ {}Hz (native, HAL)",
                fb.width, fb.height, drv.current_hz());
            *GPU.lock() = Some(Box::new(drv) as Box<dyn GpuHal>);
            Ok(fb)
        }
        Err(e) => {
            crate::kprintln!("[npk] GPU: Intel Xe init failed: {:?}", e);
            *detected = Some(drv);
            Err(e)
        }
    }
}

/// Dump native GPU registers (Intel Xe specific, debug only).
pub fn dump_native() {
    let detected = DETECTED_XE.lock();
    if let Some(ref drv) = *detected {
        drv.dump_registers();
    } else {
        crate::kprintln!("[npk] GPU: use before 'gpu init' for register dump");
    }
}

/// Test PLL re-lock (Intel Xe specific, debug only).
pub fn test_pll() {
    let detected = DETECTED_XE.lock();
    if let Some(ref drv) = *detected {
        drv.test_pll();
    } else {
        crate::kprintln!("[npk] GPU: no detected GPU for PLL test");
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

/// Auto-incrementing GPU log counter.
static GPU_LOG_SEQ: Mutex<u32> = Mutex::new(0);

/// Generate a unique GPU log name: gpu-log-NNN-YYYY-MM-DD
pub fn next_log_name() -> alloc::string::String {
    let mut seq = GPU_LOG_SEQ.lock();
    *seq += 1;
    let n = *seq;

    // Try RTC for date, fallback to seq-only
    if let Some(unix) = crate::rtc::read_unix_time() {
        // Convert unix timestamp to date
        let days = unix / 86400;
        let mut y = 1970u32;
        let mut rem = days;
        loop {
            let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366u64 } else { 365 };
            if rem < days_in_year { break; }
            rem -= days_in_year;
            y += 1;
        }
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let mdays: [u64; 12] = [31, if leap {29} else {28}, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut m = 0u32;
        for md in &mdays {
            if rem < *md { break; }
            rem -= *md;
            m += 1;
        }
        let d = rem as u32 + 1;
        alloc::format!("gpu-log-{:03}-{:04}-{:02}-{:02}", n, y, m + 1, d)
    } else {
        alloc::format!("gpu-log-{:03}", n)
    }
}
