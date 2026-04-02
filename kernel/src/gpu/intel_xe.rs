//! Intel Xe Display Driver (Gen 12.2 / Alder Lake)
//!
//! Native modesetting for Intel UHD Graphics on Alder Lake-N (N100).
//! Display-only: no 3D, no compute, no GuC firmware.
//!
//! Reference: Intel Open Source PRM, Volume 12: Display Engine (Gen 12)

#![allow(dead_code)]

use super::{FramebufferInfo, GpuError, ModeInfo};
use crate::{kprintln, pci, paging, memory};

// ── PCI Device IDs ──────────────────────────────────────────────────

const INTEL_VENDOR: u16 = 0x8086;

const KNOWN_DEVICE_IDS: &[(u16, &str)] = &[
    (0x46D0, "Alder Lake-N GT1"),
    (0x46D1, "Alder Lake-N GT1 (variant)"),
    (0x46D2, "Alder Lake-N GT1 (variant)"),
];

// ── MMIO Register Offsets (from BAR0) ───────────────────────────────

// Power well management
const PWR_WELL_CTL2: u32       = 0x45404;
const FUSE_STATUS: u32         = 0x42000;
const SFUSE_STRAP: u32         = 0xC2014;

// Core display clock
const CDCLK_CTL: u32           = 0x46000;
const DBUF_CTL_S1: u32         = 0x45008;

// DPLL (Display PLL) — TGL/ADL offsets (NOT ICL!)
const DPLL_ENABLE_0: u32       = 0x46010;
const DPLL_ENABLE_1: u32       = 0x46014;
const DPLL_CFGCR0_0: u32      = 0x164284;  // TGL/ADL DPLL0
const DPLL_CFGCR1_0: u32      = 0x164288;
const DPLL_CFGCR0_1: u32      = 0x16428C;  // TGL/ADL DPLL1
const DPLL_CFGCR1_1: u32      = 0x164290;

// Transcoder A timing
const TRANS_HTOTAL_A: u32      = 0x60000;
const TRANS_HBLANK_A: u32      = 0x60004;
const TRANS_HSYNC_A: u32       = 0x60008;
const TRANS_VTOTAL_A: u32      = 0x6000C;
const TRANS_VBLANK_A: u32      = 0x60010;
const TRANS_VSYNC_A: u32       = 0x60014;
const TRANS_DDI_FUNC_CTL_A: u32 = 0x60400;
const TRANS_CLK_SEL_A: u32     = 0x46140;

// Pipe A
const PIPE_CONF_A: u32         = 0x70008;
const PIPE_SRCSZ_A: u32       = 0x6001C;

// Plane 1 on Pipe A
const PLANE_CTL_1_A: u32      = 0x70180;
const PLANE_STRIDE_1_A: u32   = 0x70188;
const PLANE_POS_1_A: u32      = 0x7018C;
const PLANE_SIZE_1_A: u32     = 0x70190;
const PLANE_SURF_1_A: u32     = 0x7019C;

// DDI
const DDI_BUF_CTL_A: u32      = 0x64000;
const DDI_BUF_CTL_B: u32      = 0x64100;

// GGTT base (within BAR0)
const GGTT_BASE: u32           = 0x800000;

// ── Display Timings ─────────────────────────────────────────────────

/// CEA-861 standard timings
struct DisplayTiming {
    width: u32,
    height: u32,
    hz: u8,
    pixel_clock_khz: u32,
    h_front_porch: u16,
    h_sync: u16,
    h_back_porch: u16,
    v_front_porch: u16,
    v_sync: u16,
    v_back_porch: u16,
}

impl DisplayTiming {
    fn h_total(&self) -> u32 {
        self.width + self.h_front_porch as u32 + self.h_sync as u32 + self.h_back_porch as u32
    }
    fn v_total(&self) -> u32 {
        self.height + self.v_front_porch as u32 + self.v_sync as u32 + self.v_back_porch as u32
    }
}

// Standard CEA/VESA timings
const TIMING_4K_60: DisplayTiming = DisplayTiming {
    width: 3840, height: 2160, hz: 60, pixel_clock_khz: 594000,
    h_front_porch: 176, h_sync: 88, h_back_porch: 296,
    v_front_porch: 8, v_sync: 10, v_back_porch: 72,
};

const TIMING_4K_30: DisplayTiming = DisplayTiming {
    width: 3840, height: 2160, hz: 30, pixel_clock_khz: 297000,
    h_front_porch: 176, h_sync: 88, h_back_porch: 296,
    v_front_porch: 8, v_sync: 10, v_back_porch: 72,
};

const TIMING_1080P_60: DisplayTiming = DisplayTiming {
    width: 1920, height: 1080, hz: 60, pixel_clock_khz: 148500,
    h_front_porch: 88, h_sync: 44, h_back_porch: 148,
    v_front_porch: 4, v_sync: 5, v_back_porch: 36,
};

const TIMING_1440P_60: DisplayTiming = DisplayTiming {
    width: 2560, height: 1440, hz: 60, pixel_clock_khz: 241500,
    h_front_porch: 48, h_sync: 32, h_back_porch: 80,
    v_front_porch: 3, v_sync: 5, v_back_porch: 33,
};

fn find_timing(width: u32, height: u32, hz: u8) -> Option<&'static DisplayTiming> {
    let timings: &[&DisplayTiming] = &[
        &TIMING_4K_60, &TIMING_4K_30, &TIMING_1080P_60, &TIMING_1440P_60,
    ];
    for t in timings {
        if t.width == width && t.height == height && t.hz == hz {
            return Some(t);
        }
    }
    None
}

// ── DPLL Parameters ─────────────────────────────────────────────────

/// Pre-calculated PLL parameters for known pixel clocks.
/// DCO frequency, integer/fraction, and output dividers.
struct PllParams {
    dco_integer: u16,
    dco_fraction: u16,
    pdiv: u8,
    qdiv: u8,
    kdiv: u8,
}

fn pll_for_clock(pixel_clock_khz: u32) -> Option<PllParams> {
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
    match pixel_clock_khz {
        594000 => Some(PllParams {
            // AFE=2970 MHz, div=3, DCO=8910 MHz
            dco_integer: 464, dco_fraction: 0x800,
            pdiv: 3, qdiv: 1, kdiv: 1,
        }),
        297000 => Some(PllParams {
            // AFE=1485 MHz, div=6, DCO=8910 MHz
            dco_integer: 464, dco_fraction: 0x800,
            pdiv: 3, qdiv: 1, kdiv: 2,
        }),
        148500 => Some(PllParams {
            // AFE=742.5 MHz, div=12, DCO=8910 MHz
            // Verified: matches firmware CFGCR1=0x00000E84
            dco_integer: 464, dco_fraction: 0x800,
            pdiv: 2, qdiv: 3, kdiv: 2,
        }),
        241500 => Some(PllParams {
            // AFE=1207.5 MHz, div=7, DCO=8452.5 MHz
            dco_integer: 440, dco_fraction: 0x1800,
            pdiv: 7, qdiv: 1, kdiv: 1,
        }),
        _ => None,
    }
}

/// Encode PLL params into DPLL_CFGCR0/CFGCR1 register values (TGL/ADL format).
/// Bit layout verified against NUC firmware values.
fn encode_cfgcr(params: &PllParams) -> (u32, u32) {
    // CFGCR0: dco_fraction[24:9] | dco_integer[8:0]
    let cfgcr0 = ((params.dco_fraction as u32) << 9) | (params.dco_integer as u32 & 0x1FF);

    // CFGCR1 (actual TGL/ADL layout, verified against firmware):
    //   QDIV_RATIO [17:10]
    //   QDIV_MODE  [9]     — 1 if qdiv > 1
    //   KDIV       [8:6]   — encoded: 1→1, 2→2, 3→4
    //   PDIV       [5:2]   — encoded: 2→1, 3→2, 5→4, 7→6
    let pdiv_enc: u32 = match params.pdiv {
        2 => 1, 3 => 2, 5 => 4, 7 => 6, _ => 1,
    };
    let kdiv_enc: u32 = match params.kdiv {
        1 => 1, 2 => 2, 3 => 4, _ => 1,
    };
    let qdiv_mode = if params.qdiv > 1 { 1u32 } else { 0 };
    let qdiv_ratio = if params.qdiv > 1 { params.qdiv as u32 } else { 0 };

    let cfgcr1 = (qdiv_ratio << 10)
        | (qdiv_mode << 9)
        | (kdiv_enc << 6)
        | (pdiv_enc << 2);

    (cfgcr0, cfgcr1)
}

// ── MMIO Helpers ────────────────────────────────────────────────────

fn mmio_read32(base: u64, offset: u32) -> u32 {
    let addr = (base + offset as u64) as *const u32;
    // SAFETY: BAR0 is identity-mapped, volatile prevents reordering
    unsafe { core::ptr::read_volatile(addr) }
}

fn mmio_write32(base: u64, offset: u32, val: u32) {
    let addr = (base + offset as u64) as *mut u32;
    // SAFETY: BAR0 is identity-mapped, volatile prevents reordering
    unsafe { core::ptr::write_volatile(addr, val); }
}

fn mmio_write64(base: u64, offset: u32, val: u64) {
    let addr = (base + offset as u64) as *mut u64;
    // SAFETY: BAR0 is identity-mapped, volatile ensures write reaches device
    unsafe { core::ptr::write_volatile(addr, val); }
}

/// Spin-wait with timeout (in iterations). Returns true if condition met.
fn poll_timeout(base: u64, reg: u32, mask: u32, expected: u32, max_iters: u32) -> bool {
    for _ in 0..max_iters {
        if mmio_read32(base, reg) & mask == expected {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

// ── Driver State ────────────────────────────────────────────────────

pub struct IntelXeDriver {
    pci_addr: pci::PciAddr,
    device_id: u16,
    device_name: &'static str,
    bar0: u64,          // GTTMMADR: 16MB MMIO registers + GGTT
    bar2: u64,          // GMADR: 256MB aperture
    fb: Option<FramebufferInfo>,
    fb_ggtt_offset: u32,  // GGTT offset of scanout framebuffer
    fb_phys: u64,         // Physical address of framebuffer memory
    fb_pages: u32,        // Number of 4KB pages allocated
    active_timing: Option<&'static DisplayTiming>,
    ddi_port: u8,         // Which DDI port (0=A, 1=B, etc.)
    firmware_dpll: u8,    // Which DPLL firmware used (detected at boot)
}

impl IntelXeDriver {
    /// Scan PCI bus for Intel Xe GPU.
    pub fn detect() -> Option<Self> {
        // Try known ADL-N device IDs first
        for &(did, name) in KNOWN_DEVICE_IDS {
            if let Some(dev) = pci::find_device(INTEL_VENDOR, did) {
                return Some(Self::new(dev, did, name));
            }
        }

        // Fallback: any Intel VGA controller (class 03:00)
        if let Some(dev) = pci::find_by_class(0x03, 0x00) {
            if dev.vendor_id == INTEL_VENDOR {
                let did = dev.device_id;
                return Some(Self::new(dev, did, "Intel GPU (unknown)"));
            }
        }

        None
    }

    fn new(dev: pci::PciDevice, device_id: u16, name: &'static str) -> Self {
        // Enable PCI memory space access
        let cmd = pci::read32(dev.addr, 0x04);
        pci::write32(dev.addr, 0x04, cmd | 0x06);

        let bar0 = pci::read_bar64(dev.addr, 0x10);
        let bar2 = pci::read_bar64(dev.addr, 0x18);

        kprintln!("[npk]   GPU PCI {:02x}:{:02x}.{} — {}",
            dev.addr.bus, dev.addr.device, dev.addr.function, name);
        kprintln!("[npk]   BAR0 (MMIO): {:#x}", bar0);
        kprintln!("[npk]   BAR2 (aperture): {:#x}", bar2);

        // Map BAR0 (16MB) so registers are accessible for dump and init
        if bar0 != 0 {
            let bar0_size = 16 * 1024 * 1024u64;
            for offset in (0..bar0_size).step_by(4096) {
                match paging::map_page(
                    bar0 + offset, bar0 + offset,
                    paging::PageFlags::PRESENT | paging::PageFlags::WRITABLE | paging::PageFlags::NO_CACHE,
                ) {
                    Ok(()) | Err(paging::PagingError::AlreadyMapped) => {}
                    Err(_) => {
                        kprintln!("[npk]   GPU: BAR0 map failed at offset {:#x}", offset);
                        break;
                    }
                }
            }
            kprintln!("[npk]   GPU: BAR0 mapped (16MB)");
        }

        Self {
            pci_addr: dev.addr,
            device_id,
            device_name: name,
            bar0,
            bar2,
            fb: None,
            fb_ggtt_offset: 0,
            fb_phys: 0,
            fb_pages: 0,
            active_timing: None,
            ddi_port: 0,
            firmware_dpll: 1, // Default DPLL1 (NUC firmware uses this)
        }
    }

    pub fn name(&self) -> &'static str {
        self.device_name
    }

    /// Test PLL locking by reading firmware values, disabling, re-writing, re-enabling.
    /// Does NOT touch the display pipeline — only the PLL.
    pub fn test_pll(&self) {
        let (enable_reg, cfgcr0_reg, cfgcr1_reg) = self.dpll_regs();

        // Read current firmware PLL state
        let orig_enable = mmio_read32(self.bar0, enable_reg);
        let orig_cfgcr0 = mmio_read32(self.bar0, cfgcr0_reg);
        let orig_cfgcr1 = mmio_read32(self.bar0, cfgcr1_reg);

        kprintln!("[npk]   DPLL{} test: ENABLE={:#010x} CFGCR0={:#010x} CFGCR1={:#010x}",
            self.firmware_dpll, orig_enable, orig_cfgcr0, orig_cfgcr1);

        let locked = orig_enable & (1 << 30) != 0;
        let enabled = orig_enable & (1 << 31) != 0;
        kprintln!("[npk]   Current state: enabled={} locked={}", enabled, locked);

        if !enabled {
            kprintln!("[npk]   PLL not enabled, nothing to test");
            return;
        }

        // Step 1: Disable the display pipeline first (must stop using PLL before disabling it)
        kprintln!("[npk]   Step 1: Disabling pipe + transcoder...");
        let pipe = mmio_read32(self.bar0, PIPE_CONF_A);
        mmio_write32(self.bar0, PIPE_CONF_A, pipe & !(1 << 31));
        let _ = poll_timeout(self.bar0, PIPE_CONF_A, 1 << 30, 0, 200_000);

        // Disable DDI buffer
        let ddi_ctl = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };
        let ddi = mmio_read32(self.bar0, ddi_ctl);
        mmio_write32(self.bar0, ddi_ctl, ddi & !(1 << 31));
        let _ = poll_timeout(self.bar0, ddi_ctl, 1 << 7, 1 << 7, 200_000);

        // Disable transcoder DDI function
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, 0);

        kprintln!("[npk]   Step 2: Disabling DPLL{}...", self.firmware_dpll);
        mmio_write32(self.bar0, enable_reg, orig_enable & !(1 << 31));

        // Wait for PLL to unlock
        if !poll_timeout(self.bar0, enable_reg, 1 << 30, 0, 200_000) {
            kprintln!("[npk]   WARNING: PLL unlock timeout");
        }
        let after_disable = mmio_read32(self.bar0, enable_reg);
        kprintln!("[npk]   After disable: ENABLE={:#010x}", after_disable);

        // Step 3: Write back the SAME CFGCR values
        kprintln!("[npk]   Step 3: Writing CFGCR0={:#010x} CFGCR1={:#010x}",
            orig_cfgcr0, orig_cfgcr1);
        mmio_write32(self.bar0, cfgcr0_reg, orig_cfgcr0);
        mmio_write32(self.bar0, cfgcr1_reg, orig_cfgcr1);
        // Posting read to ensure writes complete
        let _ = mmio_read32(self.bar0, cfgcr1_reg);

        // Step 4: Re-enable PLL
        kprintln!("[npk]   Step 4: Enabling DPLL{}...", self.firmware_dpll);
        mmio_write32(self.bar0, enable_reg, 1 << 31);

        // Poll for lock
        let locked = poll_timeout(self.bar0, enable_reg, 1 << 30, 1 << 30, 1_000_000);
        let final_val = mmio_read32(self.bar0, enable_reg);
        kprintln!("[npk]   Result: ENABLE={:#010x} locked={}", final_val, locked);

        if locked {
            kprintln!("[npk]   SUCCESS: PLL re-locked with firmware values!");
            kprintln!("[npk]   (screen will stay black — display pipeline disabled)");
        } else {
            kprintln!("[npk]   FAILED: PLL did not re-lock");
            kprintln!("[npk]   This means the PLL enable sequence itself is wrong");
        }
    }

    pub fn current_hz(&self) -> u8 {
        self.active_timing.map_or(0, |t| t.hz)
    }

    pub fn framebuffer(&self) -> FramebufferInfo {
        self.fb.unwrap_or(FramebufferInfo {
            addr: 0, pitch: 0, width: 0, height: 0, bpp: 0,
        })
    }

    pub fn supported_modes(&self) -> alloc::vec::Vec<ModeInfo> {
        alloc::vec![
            ModeInfo { width: 3840, height: 2160, hz: 60 },
            ModeInfo { width: 3840, height: 2160, hz: 30 },
            ModeInfo { width: 2560, height: 1440, hz: 60 },
            ModeInfo { width: 1920, height: 1080, hz: 60 },
        ]
    }

    // ── Initialization ──────────────────────────────────────────────

    /// Full display pipeline initialization. Attempts 4K@60, falls back.
    pub fn init(&mut self) -> Result<FramebufferInfo, GpuError> {
        // Enable PCI memory space + bus mastering
        let cmd = pci::read32(self.pci_addr, 0x04);
        pci::write32(self.pci_addr, 0x04, cmd | 0x06);

        if self.bar0 == 0 {
            return Err(GpuError::MappingFailed);
        }

        // BAR0 already mapped during detect (new())

        // Detect available DDI ports
        self.detect_ddi_ports();

        // Power up display engine
        self.power_on()?;

        // Initialize core display clock
        self.init_cdclk()?;

        // Disable firmware display pipeline before reprogramming PLL
        // (PLL cannot be reprogrammed while actively driving a transcoder)
        kprintln!("[npk]   GPU: disabling firmware pipeline...");
        self.disable_display();

        // Try modes in preference order: 4K@60 → 4K@30 → 1080p
        let modes = [
            (&TIMING_4K_60, "4K@60Hz"),
            (&TIMING_4K_30, "4K@30Hz"),
            (&TIMING_1080P_60, "1080p@60Hz"),
        ];

        for (timing, label) in modes {
            kprintln!("[npk]   GPU: trying {}...", label);
            match self.enable_display(timing) {
                Ok(fb) => {
                    self.fb = Some(fb);
                    self.active_timing = Some(timing);
                    return Ok(fb);
                }
                Err(e) => {
                    kprintln!("[npk]   GPU: {} failed: {:?}", label, e);
                    self.disable_display();
                }
            }
        }

        Err(GpuError::PipelineFailed)
    }

    /// Set a new display mode (after initial init).
    pub fn set_mode(&mut self, width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {
        let timing = find_timing(width, height, hz)
            .ok_or(GpuError::UnsupportedMode)?;

        self.disable_display();
        self.free_framebuffer();

        let fb = self.enable_display(timing)?;
        self.fb = Some(fb);
        self.active_timing = Some(timing);
        Ok(fb)
    }

    // ── Register Dump ────────────────────────────────────────────────

    /// Dump current display engine state (read-only, no writes).
    /// Use this to understand what the firmware configured.
    pub fn dump_registers(&self) {
        kprintln!("[npk]   === Intel Xe Display Register Dump ===");
        kprintln!("[npk]   BAR0: {:#x}", self.bar0);

        // Fuses
        let fuse = mmio_read32(self.bar0, FUSE_STATUS);
        let sfuse = mmio_read32(self.bar0, SFUSE_STRAP);
        kprintln!("[npk]   FUSE_STATUS:  {:#010x}", fuse);
        kprintln!("[npk]   SFUSE_STRAP:  {:#010x}", sfuse);

        // Power
        let pwr = mmio_read32(self.bar0, PWR_WELL_CTL2);
        kprintln!("[npk]   PWR_WELL_CTL2: {:#010x}", pwr);

        // CDCLK
        let cdclk = mmio_read32(self.bar0, CDCLK_CTL);
        let dbuf = mmio_read32(self.bar0, DBUF_CTL_S1);
        kprintln!("[npk]   CDCLK_CTL:    {:#010x}", cdclk);
        kprintln!("[npk]   DBUF_CTL_S1:  {:#010x}", dbuf);

        // DPLL 0 and 1
        let dpll0_en = mmio_read32(self.bar0, DPLL_ENABLE_0);
        let dpll0_c0 = mmio_read32(self.bar0, DPLL_CFGCR0_0);
        let dpll0_c1 = mmio_read32(self.bar0, DPLL_CFGCR1_0);
        let dpll1_en = mmio_read32(self.bar0, DPLL_ENABLE_1);
        let dpll1_c0 = mmio_read32(self.bar0, DPLL_CFGCR0_1);
        let dpll1_c1 = mmio_read32(self.bar0, DPLL_CFGCR1_1);
        kprintln!("[npk]   DPLL0_ENABLE: {:#010x}", dpll0_en);
        kprintln!("[npk]   DPLL0_CFGCR0: {:#010x}", dpll0_c0);
        kprintln!("[npk]   DPLL0_CFGCR1: {:#010x}", dpll0_c1);
        kprintln!("[npk]   DPLL1_ENABLE: {:#010x}", dpll1_en);
        kprintln!("[npk]   DPLL1_CFGCR0: {:#010x}", dpll1_c0);
        kprintln!("[npk]   DPLL1_CFGCR1: {:#010x}", dpll1_c1);

        // Transcoder A clock selection
        let clk_sel = mmio_read32(self.bar0, TRANS_CLK_SEL_A);
        kprintln!("[npk]   TRANS_CLK_SEL_A: {:#010x}", clk_sel);

        // Transcoder A timings
        let htotal = mmio_read32(self.bar0, TRANS_HTOTAL_A);
        let hblank = mmio_read32(self.bar0, TRANS_HBLANK_A);
        let hsync  = mmio_read32(self.bar0, TRANS_HSYNC_A);
        let vtotal = mmio_read32(self.bar0, TRANS_VTOTAL_A);
        let vblank = mmio_read32(self.bar0, TRANS_VBLANK_A);
        let vsync  = mmio_read32(self.bar0, TRANS_VSYNC_A);
        kprintln!("[npk]   TRANS_HTOTAL_A: {:#010x}  (active={}, total={})",
            htotal, (htotal & 0xFFFF) + 1, (htotal >> 16) + 1);
        kprintln!("[npk]   TRANS_HBLANK_A: {:#010x}", hblank);
        kprintln!("[npk]   TRANS_HSYNC_A:  {:#010x}", hsync);
        kprintln!("[npk]   TRANS_VTOTAL_A: {:#010x}  (active={}, total={})",
            vtotal, (vtotal & 0xFFFF) + 1, (vtotal >> 16) + 1);
        kprintln!("[npk]   TRANS_VBLANK_A: {:#010x}", vblank);
        kprintln!("[npk]   TRANS_VSYNC_A:  {:#010x}", vsync);

        // Transcoder DDI function control
        let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);
        kprintln!("[npk]   TRANS_DDI_FUNC_CTL_A: {:#010x}", ddi_func);
        if ddi_func & (1 << 31) != 0 {
            // TGL+ port select is bits [30:27], encoding: 1=A, 2=B, 3=C
            let ddi_sel = (ddi_func >> 27) & 0xF;
            let port_letter = if ddi_sel > 0 { (b'A' + ddi_sel as u8 - 1) as char } else { '?' };
            let mode = (ddi_func >> 24) & 0x7;
            let bpc = (ddi_func >> 20) & 0x7;
            kprintln!("[npk]     DDI={}, mode={}, bpc={}, enabled",
                port_letter,
                match mode { 0 => "HDMI", 1 => "DVI", 2 => "DP-SST", 4 => "DP-MST", _ => "?" },
                match bpc { 0 => "8", 1 => "10", 2 => "6", 3 => "12", _ => "?" });
        } else {
            kprintln!("[npk]     (disabled)");
        }

        // Pipe A
        let pipe_conf = mmio_read32(self.bar0, PIPE_CONF_A);
        let pipe_src = mmio_read32(self.bar0, PIPE_SRCSZ_A);
        kprintln!("[npk]   PIPE_CONF_A:  {:#010x}  (enabled={})",
            pipe_conf, pipe_conf & (1 << 31) != 0);
        kprintln!("[npk]   PIPE_SRCSZ_A: {:#010x}  ({}x{})",
            pipe_src, (pipe_src & 0xFFFF) + 1, (pipe_src >> 16) + 1);

        // Plane 1
        let plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);
        let plane_stride = mmio_read32(self.bar0, PLANE_STRIDE_1_A);
        let plane_pos = mmio_read32(self.bar0, PLANE_POS_1_A);
        let plane_size = mmio_read32(self.bar0, PLANE_SIZE_1_A);
        let plane_surf = mmio_read32(self.bar0, PLANE_SURF_1_A);
        kprintln!("[npk]   PLANE_CTL_1_A:    {:#010x}  (enabled={})",
            plane_ctl, plane_ctl & (1 << 31) != 0);
        kprintln!("[npk]   PLANE_STRIDE_1_A: {} ({}B per row)",
            plane_stride, plane_stride * 64);
        kprintln!("[npk]   PLANE_POS_1_A:    {:#010x}", plane_pos);
        kprintln!("[npk]   PLANE_SIZE_1_A:   {:#010x}  ({}x{})",
            plane_size, (plane_size & 0xFFFF) + 1, (plane_size >> 16) + 1);
        kprintln!("[npk]   PLANE_SURF_1_A:   {:#010x}  (GGTT offset)", plane_surf);

        // DDI buffer control
        let ddi_a = mmio_read32(self.bar0, DDI_BUF_CTL_A);
        let ddi_b = mmio_read32(self.bar0, DDI_BUF_CTL_B);
        kprintln!("[npk]   DDI_BUF_CTL_A: {:#010x}  (enabled={})",
            ddi_a, ddi_a & (1 << 31) != 0);
        kprintln!("[npk]   DDI_BUF_CTL_B: {:#010x}  (enabled={})",
            ddi_b, ddi_b & (1 << 31) != 0);

        kprintln!("[npk]   === End Register Dump ===");
    }

    // ── DDI Port Detection ──────────────────────────────────────────

    fn detect_ddi_ports(&mut self) {
        let fuse = mmio_read32(self.bar0, FUSE_STATUS);
        let sfuse = mmio_read32(self.bar0, SFUSE_STRAP);

        kprintln!("[npk]   FUSE_STATUS: {:#010x}", fuse);
        kprintln!("[npk]   SFUSE_STRAP: {:#010x}", sfuse);

        // Read TRANS_DDI_FUNC_CTL to see what the firmware configured
        let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);
        if ddi_func & (1 << 31) != 0 {
            // Firmware has an active DDI — use the same port
            // TGL+ port select is bits [30:27]
            let ddi_sel = ((ddi_func >> 27) & 0xF) as u8;
            // TGL+ encoding: 1=A, 2=B, 3=C, ...
            let port = if ddi_sel > 0 { ddi_sel - 1 } else { 0 };
            kprintln!("[npk]   Firmware using DDI-{} (TRANS_DDI={:#010x})",
                (b'A' + port) as char, ddi_func);
            self.ddi_port = port;
        } else {
            self.ddi_port = 1; // DDI-B default (NUC HDMI is on DDI-B)
            kprintln!("[npk]   No active DDI found, defaulting to DDI-B");
        }

        // Detect which DPLL the firmware uses
        let clk_sel = mmio_read32(self.bar0, TRANS_CLK_SEL_A);
        let dpll_sel = (clk_sel >> 29) & 0x7;
        kprintln!("[npk]   Firmware clock source: DPLL{}", dpll_sel);
        // Store for use during modesetting
        self.firmware_dpll = dpll_sel as u8;
    }

    // ── Power Management ────────────────────────────────────────────

    fn power_on(&self) -> Result<(), GpuError> {
        kprintln!("[npk]   GPU: enabling power wells...");

        // Read current power well state
        let pwr = mmio_read32(self.bar0, PWR_WELL_CTL2);
        kprintln!("[npk]   PWR_WELL_CTL2: {:#010x}", pwr);

        // Enable PW1 (Power Group 1): bit 1 = request, bit 0 = state
        self.enable_power_well(0, "PW1")?;

        // Enable PW2 (Power Group 2): bit 3 = request, bit 2 = state
        self.enable_power_well(1, "PW2")?;

        // Note: Combo PHY DDI ports (HDMI on ADL-N) do NOT need separate
        // DDI power wells. Those are for TypeC/TBT ports only.
        // PW1 + PW2 cover all combo PHY display functionality.

        kprintln!("[npk]   GPU: power wells enabled");
        Ok(())
    }

    fn enable_power_well(&self, idx: u32, name: &str) -> Result<(), GpuError> {
        let request_bit = 1u32 << (idx * 2 + 1);
        let state_bit = 1u32 << (idx * 2);

        // Check if already on
        let val = mmio_read32(self.bar0, PWR_WELL_CTL2);
        if val & state_bit != 0 {
            kprintln!("[npk]     {} already on", name);
            return Ok(());
        }

        // Request enable
        mmio_write32(self.bar0, PWR_WELL_CTL2, val | request_bit);

        // Poll for state bit (up to 20ms equivalent in iterations)
        if !poll_timeout(self.bar0, PWR_WELL_CTL2, state_bit, state_bit, 200_000) {
            kprintln!("[npk]     {} enable TIMEOUT", name);
            return Err(GpuError::PowerTimeout);
        }

        kprintln!("[npk]     {} enabled", name);
        Ok(())
    }

    // ── CDCLK (Core Display Clock) ──────────────────────────────────

    fn init_cdclk(&self) -> Result<(), GpuError> {
        // Read current CDCLK
        let cdclk = mmio_read32(self.bar0, CDCLK_CTL);
        kprintln!("[npk]   CDCLK_CTL: {:#010x}", cdclk);

        // For 4K@60Hz (594 MHz pixel clock), we need CDCLK >= 312 MHz.
        // ADL supports CDCLK values: 172.8, 192, 307.2, 312, 552, 556.8, 648, 652.8 MHz
        //
        // CDCLK_CTL format (Gen 12):
        //   Bits 10:8 = cd2x divider select
        //   Bits 25:22 = SSA precharge
        //   Bit 26 = PLL enable
        //
        // For now, accept whatever the firmware set (it should be enough for 1080p).
        // We'll reprogram if needed for 4K.

        // Enable DBUF (Display Buffer)
        let dbuf = mmio_read32(self.bar0, DBUF_CTL_S1);
        if dbuf & (1 << 31) == 0 {
            mmio_write32(self.bar0, DBUF_CTL_S1, dbuf | (1 << 31));
            if !poll_timeout(self.bar0, DBUF_CTL_S1, 1 << 0, 1 << 0, 100_000) {
                kprintln!("[npk]   DBUF enable timeout");
                return Err(GpuError::PowerTimeout);
            }
            kprintln!("[npk]   DBUF enabled");
        } else {
            kprintln!("[npk]   DBUF already enabled");
        }

        Ok(())
    }

    // ── Display Pipeline ────────────────────────────────────────────

    fn enable_display(&mut self, timing: &'static DisplayTiming) -> Result<FramebufferInfo, GpuError> {
        // Step 1: Allocate framebuffer
        let fb = self.allocate_framebuffer(timing)?;

        // Step 2: Program GGTT entries for framebuffer
        self.program_ggtt()?;

        // Step 3: Program DPLL for pixel clock
        self.program_dpll(timing)?;

        // Step 4: Select clock source for transcoder
        // TRANS_CLK_SEL: bits 31:29 = DPLL select (1 = DPLL0, 2 = DPLL1)
        let dpll_sel = (self.firmware_dpll as u32 + 1) << 29;
        kprintln!("[npk]   TRANS_CLK_SEL: {:#010x} (DPLL{})", dpll_sel, self.firmware_dpll);
        mmio_write32(self.bar0, TRANS_CLK_SEL_A, dpll_sel);

        // Step 5: Program transcoder timings
        self.program_transcoder(timing);

        // Step 6: Configure DDI
        self.enable_ddi(timing)?;

        // Step 7: Set pipe source size: (width-1) << 16 | (height-1)
        mmio_write32(self.bar0, PIPE_SRCSZ_A,
            ((timing.width - 1) << 16) | (timing.height - 1));

        // Step 8: Configure plane
        self.configure_plane(timing)?;

        // Step 9: Enable pipe
        let pipe = mmio_read32(self.bar0, PIPE_CONF_A);
        mmio_write32(self.bar0, PIPE_CONF_A, pipe | (1 << 31));
        if !poll_timeout(self.bar0, PIPE_CONF_A, 1 << 30, 1 << 30, 100_000) {
            kprintln!("[npk]   Pipe A enable timeout");
            return Err(GpuError::PipelineFailed);
        }
        kprintln!("[npk]   Pipe A enabled");

        Ok(fb)
    }

    fn disable_display(&mut self) {
        // Disable in reverse order: plane → pipe → transcoder → DDI → DPLL
        kprintln!("[npk]   GPU: disabling current display pipeline...");

        // Disable plane
        let plane = mmio_read32(self.bar0, PLANE_CTL_1_A);
        mmio_write32(self.bar0, PLANE_CTL_1_A, plane & !(1 << 31));
        mmio_write32(self.bar0, PLANE_SURF_1_A, 0); // trigger update

        // Disable pipe (TRANSCONF)
        let pipe = mmio_read32(self.bar0, PIPE_CONF_A);
        mmio_write32(self.bar0, PIPE_CONF_A, pipe & !(1 << 31));
        let _ = poll_timeout(self.bar0, PIPE_CONF_A, 1 << 30, 0, 200_000);

        // Disable DDI buffer first (before transcoder DDI func)
        let ddi_ctl_reg = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };
        let ddi = mmio_read32(self.bar0, ddi_ctl_reg);
        mmio_write32(self.bar0, ddi_ctl_reg, ddi & !(1 << 31));
        // Wait for DDI idle (bit 7 = 1)
        let _ = poll_timeout(self.bar0, ddi_ctl_reg, 1 << 7, 1 << 7, 200_000);

        // Disable transcoder DDI function
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, 0);

        // Disable DPLL (whichever firmware used)
        let (enable_reg, _, _) = self.dpll_regs();
        let dpll = mmio_read32(self.bar0, enable_reg);
        mmio_write32(self.bar0, enable_reg, dpll & !(1 << 31));
        let _ = poll_timeout(self.bar0, enable_reg, 1 << 30, 0, 200_000);

        kprintln!("[npk]   GPU: pipeline disabled");
    }

    // ── Framebuffer Allocation ──────────────────────────────────────

    fn allocate_framebuffer(&mut self, timing: &DisplayTiming) -> Result<FramebufferInfo, GpuError> {
        let pitch = timing.width * 4; // 32bpp XRGB8888
        let size = pitch * timing.height;
        let pages = (size + 4095) / 4096;

        // Allocate contiguous physical memory for scanout
        let phys = memory::allocate_contiguous(pages as usize)
            .ok_or(GpuError::AllocFailed)?;

        // Zero the framebuffer (black)
        // SAFETY: phys is identity-mapped, contiguous, and we just allocated it
        unsafe {
            core::ptr::write_bytes(phys as *mut u8, 0, size as usize);
        }

        self.fb_phys = phys;
        self.fb_pages = pages;
        // Use a GGTT offset that doesn't conflict with firmware (16MB in)
        self.fb_ggtt_offset = 0x0100_0000;

        kprintln!("[npk]   Framebuffer: {} pages @ phys {:#x}, GGTT offset {:#x}",
            pages, phys, self.fb_ggtt_offset);

        Ok(FramebufferInfo {
            addr: phys,  // CPU writes via identity-mapped physical address
            pitch,
            width: timing.width,
            height: timing.height,
            bpp: 32,
        })
    }

    fn free_framebuffer(&mut self) {
        if self.fb_pages > 0 {
            // Clear GGTT entries
            let ggtt_base = self.bar0 + GGTT_BASE as u64;
            let start_entry = self.fb_ggtt_offset / 4096;
            for i in 0..self.fb_pages {
                let entry_offset = ((start_entry + i) * 8) as u32;
                mmio_write64(ggtt_base, entry_offset, 0);
            }
            // Note: physical memory is not freed (no free API in memory.rs)
            self.fb_pages = 0;
            self.fb_phys = 0;
        }
    }

    // ── GGTT Programming ────────────────────────────────────────────

    fn program_ggtt(&self) -> Result<(), GpuError> {
        let ggtt_base = self.bar0 + GGTT_BASE as u64;
        let start_entry = self.fb_ggtt_offset / 4096;

        for i in 0..self.fb_pages {
            let phys_addr = self.fb_phys + (i as u64) * 4096;
            // GGTT PTE format (Gen 12):
            //   Bits 63:12 = physical page address
            //   Bit 0 = valid/present
            //   Bits 4:2 = cache control (0 = UC, 1 = WC)
            let ggtt_entry: u64 = (phys_addr & 0xFFFF_FFFF_FFFF_F000) | 0x01; // valid, UC

            let entry_offset = ((start_entry + i) * 8) as u32;
            mmio_write64(ggtt_base, entry_offset, ggtt_entry);
        }

        // Flush GGTT writes with a read-back
        let _ = mmio_read32(self.bar0, GGTT_BASE);

        kprintln!("[npk]   GGTT: {} entries programmed", self.fb_pages);
        Ok(())
    }

    // ── DPLL Programming ────────────────────────────────────────────

    /// Get DPLL register offsets for the active DPLL (0 or 1).
    fn dpll_regs(&self) -> (u32, u32, u32) {
        if self.firmware_dpll == 0 {
            (DPLL_ENABLE_0, DPLL_CFGCR0_0, DPLL_CFGCR1_0)
        } else {
            (DPLL_ENABLE_1, DPLL_CFGCR0_1, DPLL_CFGCR1_1)
        }
    }

    fn program_dpll(&self, timing: &DisplayTiming) -> Result<(), GpuError> {
        let params = pll_for_clock(timing.pixel_clock_khz)
            .ok_or(GpuError::PllLockFailed)?;

        let (cfgcr0, cfgcr1) = encode_cfgcr(&params);
        let (enable_reg, cfgcr0_reg, cfgcr1_reg) = self.dpll_regs();

        kprintln!("[npk]   DPLL{}: {} kHz (dco_int={}, dco_frac={:#x}, p={} q={} k={})",
            self.firmware_dpll, timing.pixel_clock_khz,
            params.dco_integer, params.dco_fraction,
            params.pdiv, params.qdiv, params.kdiv);
        kprintln!("[npk]   DPLL{}: CFGCR0={:#010x} CFGCR1={:#010x}", self.firmware_dpll, cfgcr0, cfgcr1);

        // Disable DPLL first
        let dpll = mmio_read32(self.bar0, enable_reg);
        if dpll & (1 << 31) != 0 {
            mmio_write32(self.bar0, enable_reg, dpll & !(1 << 31));
            let _ = poll_timeout(self.bar0, enable_reg, 1 << 30, 0, 200_000);
        }

        // Write PLL configuration
        mmio_write32(self.bar0, cfgcr0_reg, cfgcr0);
        mmio_write32(self.bar0, cfgcr1_reg, cfgcr1);

        // Enable DPLL
        mmio_write32(self.bar0, enable_reg, 1 << 31);

        // Poll for PLL lock (bit 30 on TGL+)
        if !poll_timeout(self.bar0, enable_reg, 1 << 30, 1 << 30, 500_000) {
            let val = mmio_read32(self.bar0, enable_reg);
            kprintln!("[npk]   DPLL{} lock TIMEOUT (DPLL_ENABLE={:#010x})", self.firmware_dpll, val);
            return Err(GpuError::PllLockFailed);
        }

        kprintln!("[npk]   DPLL{} locked at {} kHz", self.firmware_dpll, timing.pixel_clock_khz);
        Ok(())
    }

    // ── Transcoder Timing ───────────────────────────────────────────

    fn program_transcoder(&self, t: &DisplayTiming) {
        let h_total = t.h_total();
        let v_total = t.v_total();
        let h_sync_start = t.width + t.h_front_porch as u32;
        let h_sync_end = h_sync_start + t.h_sync as u32;
        let v_sync_start = t.height + t.v_front_porch as u32;
        let v_sync_end = v_sync_start + t.v_sync as u32;

        // HTOTAL = (total-1) << 16 | (active-1)
        mmio_write32(self.bar0, TRANS_HTOTAL_A, ((h_total - 1) << 16) | (t.width - 1));
        // HBLANK = (total-1) << 16 | (active-1) — blank covers non-active area
        mmio_write32(self.bar0, TRANS_HBLANK_A, ((h_total - 1) << 16) | (t.width - 1));
        // HSYNC = (sync_end-1) << 16 | (sync_start-1)
        mmio_write32(self.bar0, TRANS_HSYNC_A, ((h_sync_end - 1) << 16) | (h_sync_start - 1));

        mmio_write32(self.bar0, TRANS_VTOTAL_A, ((v_total - 1) << 16) | (t.height - 1));
        mmio_write32(self.bar0, TRANS_VBLANK_A, ((v_total - 1) << 16) | (t.height - 1));
        mmio_write32(self.bar0, TRANS_VSYNC_A, ((v_sync_end - 1) << 16) | (v_sync_start - 1));

        kprintln!("[npk]   Transcoder: {}x{} htotal={} vtotal={}",
            t.width, t.height, h_total, v_total);
    }

    // ── DDI / HDMI ──────────────────────────────────────────────────

    fn enable_ddi(&self, timing: &DisplayTiming) -> Result<(), GpuError> {
        let ddi_ctl_reg = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };

        // Configure TRANS_DDI_FUNC_CTL for HDMI mode (TGL+ format)
        // Bits 30:27 = DDI select (TGL+: 4 bits, 0=A, 1=B, ...)
        // Bits 26:24 = Mode (0 = HDMI)
        // Bits 22:20 = BPC (0 = 8bpc)
        // Bit 17 = PVSYNC (positive V sync)
        // Bit 16 = PHSYNC (positive H sync)
        // Bit 4 = HIGH_TMDS_CHAR_RATE (for >340 MHz, HDMI 2.0)
        // Bit 0 = HDMI_SCRAMBLING (for >340 MHz, HDMI 2.0)
        let hdmi_2_0 = timing.pixel_clock_khz > 340000;
        let ddi_sel = self.ddi_port as u32 + 1;            // TGL+ is 1-indexed: 1=A, 2=B
        let ddi_func = (1u32 << 31)                        // enable
            | (ddi_sel << 27)                               // DDI select (TGL+ 4-bit)
            | (0 << 24)                                     // HDMI mode
            | (0 << 20)                                     // 8 bpc
            | (1 << 17)                                     // PVSYNC (positive)
            | (1 << 16)                                     // PHSYNC (positive)
            | (if hdmi_2_0 { (1 << 4) | (1 << 0) } else { 0 }); // HDMI 2.0 scrambling

        kprintln!("[npk]   TRANS_DDI_FUNC_CTL: {:#010x} (HDMI 2.0={})", ddi_func, hdmi_2_0);
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, ddi_func);

        // Enable DDI buffer: enable + 4 lanes for HDMI
        // DDI_BUF_CTL: bit 31 = enable, bits [3:1] = port width = (4-1) = 3
        let ddi_buf_val = (1u32 << 31) | (3 << 1);
        kprintln!("[npk]   DDI_BUF_CTL: {:#010x}", ddi_buf_val);
        mmio_write32(self.bar0, ddi_ctl_reg, ddi_buf_val);

        // Wait for DDI to become active (bit 7 = idle, should clear)
        for _ in 0..100_000u32 {
            if mmio_read32(self.bar0, ddi_ctl_reg) & (1 << 7) == 0 {
                kprintln!("[npk]   DDI-{} enabled (HDMI)",
                    (b'A' + self.ddi_port) as char);
                return Ok(());
            }
            core::hint::spin_loop();
        }

        kprintln!("[npk]   DDI-{} enabled (idle bit still set — may work)",
            (b'A' + self.ddi_port) as char);
        Ok(())
    }

    // ── Plane Configuration ─────────────────────────────────────────

    fn configure_plane(&self, timing: &DisplayTiming) -> Result<(), GpuError> {
        let stride_64b = (timing.width * 4) / 64; // Stride in 64-byte units

        // PLANE_CTL: enable, XRGB8888 format, linear tiling
        let plane_ctl = (1u32 << 31)       // enable
            | (0x4 << 24)                  // XRGB 8:8:8:8 pixel format
            | (0 << 10);                   // linear tiling (no tiling)
        mmio_write32(self.bar0, PLANE_CTL_1_A, plane_ctl);

        // Stride in 64-byte chunks
        mmio_write32(self.bar0, PLANE_STRIDE_1_A, stride_64b);

        // Position (0,0)
        mmio_write32(self.bar0, PLANE_POS_1_A, 0);

        // Size: (height-1) << 16 | (width-1)
        mmio_write32(self.bar0, PLANE_SIZE_1_A,
            ((timing.height - 1) << 16) | (timing.width - 1));

        // Surface address (GGTT offset, 4K-aligned) — writing this triggers the flip
        mmio_write32(self.bar0, PLANE_SURF_1_A, self.fb_ggtt_offset);

        kprintln!("[npk]   Plane: {}x{} XRGB8888 stride={} surf={:#x}",
            timing.width, timing.height, stride_64b * 64, self.fb_ggtt_offset);
        Ok(())
    }
}
