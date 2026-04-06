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
const PIPE_CONF_A: u32         = 0x70008;  // Pipe enable/disable
const PIPE_SRCSZ_A: u32       = 0x6001C;

// Pipe Scaler (PS) — firmware may use this to scale 1080p on 4K monitors
const PS_CTRL_1A: u32         = 0x68180;  // Scaler 1 control (bit 31 = enable)
const PS_WIN_POS_1A: u32      = 0x68170;  // Scaler 1 window position
const PS_WIN_SZ_1A: u32       = 0x68174;  // Scaler 1 window size

// Plane 1 on Pipe A
const PLANE_CTL_1_A: u32      = 0x70180;
const PLANE_STRIDE_1_A: u32   = 0x70188;
const PLANE_POS_1_A: u32      = 0x7018C;
const PLANE_SIZE_1_A: u32     = 0x70190;
const PLANE_SURF_1_A: u32     = 0x7019C;

// DDI
const DDI_BUF_CTL_A: u32      = 0x64000;
const DDI_BUF_CTL_B: u32      = 0x64100;

// DDI Clock routing (ICL+) — routes DPLL to DDI/PHY (separate from TRANS_CLK_SEL!)
const ICL_DPCLKA_CFGCR0: u32  = 0x164280;

// Combo PHY TX registers (for voltage swing / signal integrity)
// PHY A base = 0x162000, PHY B base = 0x6C000
// GRP = group write (all 4 lanes), LN0 = lane 0 (read)
const ICL_PORT_TX_DW2_GRP_B: u32 = 0x6CD08;
const ICL_PORT_TX_DW2_LN0_B: u32 = 0x6CF08;
const ICL_PORT_TX_DW4_GRP_B: u32 = 0x6CD10;
const ICL_PORT_TX_DW4_LN0_B: u32 = 0x6CF10;
const ICL_PORT_TX_DW5_GRP_B: u32 = 0x6CD14;
const ICL_PORT_TX_DW5_LN0_B: u32 = 0x6CF14;
const ICL_PORT_TX_DW7_GRP_B: u32 = 0x6CD1C;
const ICL_PORT_TX_DW7_LN0_B: u32 = 0x6CF1C;

// GMBUS (I2C controller for DDC/SCDC)
const GMBUS0: u32              = 0xC5100;  // Clock/Port Select (Gen 9+: 0xC5100)
const GMBUS1: u32              = 0xC5104;  // Command/Status
const GMBUS2: u32              = 0xC5108;  // Status
const GMBUS3: u32              = 0xC510C;  // Data
const GMBUS4: u32              = 0xC5110;  // Interrupt mask
const GMBUS5: u32              = 0xC5120;  // 2-byte index register

// GMBUS0 pin pair select (ICP/TGP/ADL combo PHY, from gmbus_pins_icp[])
const GMBUS_PIN_DPB: u32       = 0x02;    // DDI-B (HDMI) — i915: GMBUS_PIN_2_BXT = "dpb"

// GMBUS1 bits
const GMBUS_SW_CLR_INT: u32    = 1 << 31;
const GMBUS_SW_RDY: u32        = 1 << 30;
const GMBUS_CYCLE_WAIT: u32    = 1 << 25;
const GMBUS_CYCLE_INDEX: u32   = 1 << 26;  // use GMBUS5 index
const GMBUS_CYCLE_STOP: u32    = 1 << 27;
const GMBUS_SLAVE_WRITE: u32   = 0 << 0;
const GMBUS_SLAVE_READ: u32    = 1 << 0;

// GMBUS2 bits
const GMBUS_HW_RDY: u32        = 1 << 11;
const GMBUS_NAK: u32           = 1 << 10;  // SATOER
const GMBUS_ACTIVE: u32        = 1 << 9;

// HDMI SCDC (Status and Control Data Channel)
const SCDC_I2C_ADDR: u8        = 0x54;    // 7-bit I2C address
const SCDC_TMDS_CONFIG: u8     = 0x20;    // TMDS_Config register
const SCDC_SCRAMBLER_STATUS: u8 = 0x21;   // Scrambler_Status (read-only)

// TRANS_DDI_FUNC_CTL mode select (bits [26:24])
const TRANS_DDI_MODE_MASK: u32                 = 0x7 << 24;
const TRANS_DDI_MODE_HDMI: u32                 = 0 << 24;  // HDMI mode (required for scrambling)
const TRANS_DDI_MODE_DVI: u32                  = 1 << 24;  // DVI mode (no scrambling, max 340MHz)

// TRANS_DDI_FUNC_CTL scrambling bits (HDMI 2.0)
const TRANS_DDI_HDMI_SCRAMBLING: u32          = 1 << 0;
const TRANS_DDI_HIGH_TMDS_CHAR_RATE: u32      = 1 << 4;
const TRANS_DDI_HDMI_SCRAMBLER_RESET_FREQ: u32 = 1 << 6;
const TRANS_DDI_HDMI_SCRAMBLER_CTS_ENABLE: u32 = 1 << 7;

// All scrambling bits combined
const TRANS_DDI_SCRAMBLING_MASK: u32 = TRANS_DDI_HDMI_SCRAMBLING
    | TRANS_DDI_HIGH_TMDS_CHAR_RATE
    | TRANS_DDI_HDMI_SCRAMBLER_RESET_FREQ
    | TRANS_DDI_HDMI_SCRAMBLER_CTS_ENABLE;

// GGTT base (within BAR0)
const GGTT_BASE: u32           = 0x800000;

// GGTT TLB invalidation (Gen 8+)
const GFX_FLSH_CNTL_GEN6: u32 = 0x101008;

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

/// Match firmware's active resolution to a known timing.
fn match_firmware_timing(width: u32, height: u32) -> &'static DisplayTiming {
    if width == 3840 && height == 2160 { return &TIMING_4K_30; }
    if width == 2560 && height == 1440 { return &TIMING_1440P_60; }
    if width == 1920 && height == 1080 { return &TIMING_1080P_60; }
    kprintln!("[npk]   Unknown firmware resolution {}x{}, assuming 1080p", width, height);
    &TIMING_1080P_60
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
            firmware_dpll: 1,
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

    /// Initialize display by reusing the firmware's existing framebuffer.
    /// The firmware (UEFI) already has a fully working display pipeline.
    /// We use the GOP framebuffer address (known-good, already being drawn to)
    /// and record hardware state for future mode changes.
    pub fn init(&mut self) -> Result<FramebufferInfo, GpuError> {
        // Enable PCI memory space + bus mastering
        let cmd = pci::read32(self.pci_addr, 0x04);
        pci::write32(self.pci_addr, 0x04, cmd | 0x06);

        if self.bar0 == 0 {
            return Err(GpuError::MappingFailed);
        }

        // Detect firmware DDI/DPLL config (read-only, no writes)
        self.detect_ddi_ports();

        // Ensure power wells are on (usually already by firmware)
        self.power_on()?;

        // Ensure DBUF is enabled
        self.init_cdclk()?;

        // Check if firmware has an active display pipeline
        let transconf = mmio_read32(self.bar0, PIPE_CONF_A);
        kprintln!("[npk]   PIPE_CONF_A: {:#010x} (enabled={}, active={})",
            transconf, transconf & (1 << 31) != 0, transconf & (1 << 30) != 0);
        if transconf & (1 << 30) == 0 {
            kprintln!("[npk]   GPU: no firmware pipe active, cannot take over");
            return Err(GpuError::PipelineFailed);
        }

        // Read firmware's active resolution from transcoder A
        let htotal_reg = mmio_read32(self.bar0, TRANS_HTOTAL_A);
        let vtotal_reg = mmio_read32(self.bar0, TRANS_VTOTAL_A);
        let fw_width = (htotal_reg & 0xFFFF) + 1;
        let fw_height = (vtotal_reg & 0xFFFF) + 1;
        kprintln!("[npk]   Firmware mode: {}x{}", fw_width, fw_height);

        // Log firmware plane state
        let fw_plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);
        let fw_plane_stride = mmio_read32(self.bar0, PLANE_STRIDE_1_A);
        let fw_plane_surf = mmio_read32(self.bar0, PLANE_SURF_1_A);
        kprintln!("[npk]   FW plane: CTL={:#010x} STRIDE={} SURF={:#010x}",
            fw_plane_ctl, fw_plane_stride, fw_plane_surf);

        // Diagnostic: read GGTT entry to see physical address (LOG ONLY, no writes)
        let ggtt_entry_idx = fw_plane_surf / 4096;
        let ggtt_entry_off = ggtt_entry_idx * 8;
        let ggtt_lo = mmio_read32(self.bar0, GGTT_BASE as u32 + ggtt_entry_off);
        let ggtt_hi = mmio_read32(self.bar0, GGTT_BASE as u32 + ggtt_entry_off + 4);
        kprintln!("[npk]   FW GGTT[{}]: {:#010x}_{:08x}",
            ggtt_entry_idx, ggtt_hi, ggtt_lo);

        // Use the GOP framebuffer address — it's the same memory the display
        // is already scanning, and it's known-good (text is rendering on it).
        // The GGTT entry might point to stolen memory (not CPU-accessible),
        // but the GOP address from Multiboot2 is always safe.
        let gop_addr = crate::framebuffer::with_fb(|fb| {
            let info = fb.info();
            (info.addr, info.pitch, info.width, info.height, info.bpp)
        });
        let (addr, pitch, width, height, bpp) = match gop_addr {
            Some(v) => v,
            None => {
                kprintln!("[npk]   GPU: no GOP framebuffer to take over");
                return Err(GpuError::PipelineFailed);
            }
        };

        kprintln!("[npk]   GOP FB: {:#x} {}x{} pitch={}", addr, width, height, pitch);

        let timing = match_firmware_timing(width, height);

        self.fb_phys = addr;
        self.fb_ggtt_offset = fw_plane_surf;
        self.fb_pages = 0; // Not our allocation — don't free firmware GGTT entries

        let fb = FramebufferInfo { addr, pitch, width, height, bpp };

        self.fb = Some(fb);
        self.active_timing = Some(timing);

        // Try 4K@60 (HDMI 2.0 scrambling), fallback to 4K@30
        kprintln!("[npk]   Attempting 4K@60Hz...");
        match self.set_mode(3840, 2160, 60) {
            Ok(fb4k) => {
                kprintln!("[npk]   4K@60Hz active");
                return Ok(fb4k);
            }
            Err(e) => {
                kprintln!("[npk]   4K@60 failed: {:?}, trying 4K@30...", e);
            }
        }
        match self.set_mode(3840, 2160, 30) {
            Ok(fb4k) => {
                kprintln!("[npk]   4K@30Hz active");
                return Ok(fb4k);
            }
            Err(e) => {
                kprintln!("[npk]   4K@30 failed: {:?}, staying at {}x{}", e, width, height);
            }
        }

        Ok(fb)
    }

    /// Set a new display mode. Reprogrms DPLL + transcoder timings,
    /// allocates new framebuffer via GGTT, returns aperture address.
    /// DDI/PHY stay running — only pipe+plane are cycled.
    pub fn set_mode(&mut self, width: u32, height: u32, hz: u8) -> Result<FramebufferInfo, GpuError> {
        let timing = find_timing(width, height, hz)
            .ok_or(GpuError::UnsupportedMode)?;

        kprintln!("[npk]   set_mode: {}x{}@{}Hz (pclk={}kHz)", width, height, hz, timing.pixel_clock_khz);
        kprintln!("[npk]   BAR2 (aperture): {:#x}", self.bar2);
        let cdclk = mmio_read32(self.bar0, CDCLK_CTL);
        kprintln!("[npk]   CDCLK_CTL: {:#010x}", cdclk);

        let need_pll_change = self.active_timing
            .map_or(true, |t| t.pixel_clock_khz != timing.pixel_clock_khz);

        let needs_scrambling = timing.pixel_clock_khz > 340000;
        let had_scrambling = self.active_timing
            .map_or(false, |t| t.pixel_clock_khz > 340000);

        // Step 0: Disable old scrambling before tearing down pipeline
        if had_scrambling && !needs_scrambling {
            self.disable_scrambling();
        }

        // Step 1: Disable plane
        let plane_ctl = mmio_read32(self.bar0, PLANE_CTL_1_A);
        mmio_write32(self.bar0, PLANE_CTL_1_A, plane_ctl & !(1 << 31));
        mmio_write32(self.bar0, PLANE_SURF_1_A, 0);
        kprintln!("[npk]   Plane disabled");

        // Step 2: Disable pipe — try BOTH possible config registers
        // ADL-N: PIPE_CONF may be at 0x70008 or 0xF0008 depending on stepping
        let pipe_val = mmio_read32(self.bar0, PIPE_CONF_A);
        kprintln!("[npk]   PIPE_CONF(0x70008): {:#010x}", pipe_val);
        // Write disable to both offsets (harmless if one is invalid)
        mmio_write32(self.bar0, 0x70008, 0);  // PIPE_CONF_A
        mmio_write32(self.bar0, 0xF0008, 0);  // TRANSCONF_A
        // Blind wait ~20ms for pipe to drain (don't rely on polling)
        for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
        let after = mmio_read32(self.bar0, PIPE_CONF_A);
        kprintln!("[npk]   Pipe after disable: {:#010x}", after);

        // Step 2b: Disable pipe scaler (firmware may use it for 1080p→4K upscale)
        let ps_ctrl = mmio_read32(self.bar0, PS_CTRL_1A);
        let ps_winsz = mmio_read32(self.bar0, PS_WIN_SZ_1A);
        kprintln!("[npk]   Scaler: CTRL={:#010x} WIN_SZ={:#010x}", ps_ctrl, ps_winsz);
        if ps_ctrl & (1 << 31) != 0 {
            kprintln!("[npk]   Disabling pipe scaler (was limiting output)");
            mmio_write32(self.bar0, PS_CTRL_1A, 0);
            // Posting read
            let _ = mmio_read32(self.bar0, PS_CTRL_1A);
        }

        // Step 3: Free old GGTT entries (no-op if fb_pages == 0)
        self.free_framebuffer();

        // Step 4: Reprogram DPLL if pixel clock changes
        if need_pll_change {
            self.program_dpll(timing)?;
        }

        // Step 5: Program transcoder timings + pipe source size
        self.program_transcoder(timing);
        let srcsz = ((timing.width - 1) << 16) | (timing.height - 1);
        mmio_write32(self.bar0, PIPE_SRCSZ_A, srcsz);
        // Also try pipe-domain offset 0x7001C (in case 0x6001C is transcoder-only)
        mmio_write32(self.bar0, 0x7001C, srcsz);
        kprintln!("[npk]   PIPE_SRCSZ: {:#010x} (written to 0x6001C + 0x7001C)", srcsz);

        // Step 6: Allocate framebuffer (contiguous physical RAM)
        let pitch = timing.width * 4;
        let fb_size = pitch * timing.height;
        let pages = (fb_size + 4095) / 4096;
        let phys = memory::allocate_contiguous(pages as usize)
            .ok_or(GpuError::AllocFailed)?;

        // SAFETY: phys is identity-mapped, contiguous, freshly allocated
        unsafe { core::ptr::write_bytes(phys as *mut u8, 0, fb_size as usize); }

        self.fb_phys = phys;
        self.fb_pages = pages;
        self.fb_ggtt_offset = 0x0100_0000; // 16MB into GGTT (avoid firmware entries)

        kprintln!("[npk]   FB: {} pages @ phys {:#x}, GGTT offset {:#x}",
            pages, phys, self.fb_ggtt_offset);

        // Step 7: Program GGTT entries (32-bit writes for MMIO safety)
        self.program_ggtt_32()?;

        // Step 8: Invalidate GGTT TLB
        mmio_write32(self.bar0, GFX_FLSH_CNTL_GEN6, 1);
        let _ = mmio_read32(self.bar0, GFX_FLSH_CNTL_GEN6);
        kprintln!("[npk]   GGTT TLB invalidated");

        // Step 9: Map aperture pages for CPU access (BAR2 + GGTT offset)
        // Use Write-Combining for ~5-10x faster sequential framebuffer writes.
        let aperture_addr = self.bar2 + self.fb_ggtt_offset as u64;
        let map_flags = paging::PageFlags::PRESENT
            | paging::PageFlags::WRITABLE
            | paging::PageFlags::WRITE_COMBINE;
        for off in (0..fb_size as u64).step_by(4096) {
            match paging::map_page(aperture_addr + off, aperture_addr + off, map_flags) {
                Ok(()) | Err(paging::PagingError::AlreadyMapped) => {}
                Err(e) => {
                    kprintln!("[npk]   Aperture map failed at +{:#x}: {:?}", off, e);
                    return Err(GpuError::MappingFailed);
                }
            }
        }
        kprintln!("[npk]   Aperture mapped: {:#x} ({} pages)", aperture_addr, pages);

        // Step 10: Configure plane (match firmware CTL including bit 3)
        let new_plane_ctl = (1u32 << 31)    // enable
            | (0x4 << 24)                    // XRGB 8:8:8:8
            | (1 << 3);                      // bit 3 (matches firmware)
        let stride_64b = pitch / 64;
        mmio_write32(self.bar0, PLANE_CTL_1_A, new_plane_ctl);
        mmio_write32(self.bar0, PLANE_STRIDE_1_A, stride_64b);
        mmio_write32(self.bar0, PLANE_POS_1_A, 0);
        mmio_write32(self.bar0, PLANE_SIZE_1_A,
            ((timing.height - 1) << 16) | (timing.width - 1));
        mmio_write32(self.bar0, PLANE_SURF_1_A, self.fb_ggtt_offset); // triggers flip
        kprintln!("[npk]   Plane: {}x{} stride={} surf={:#x}",
            timing.width, timing.height, stride_64b * 64, self.fb_ggtt_offset);

        // Update fb info NOW (before pipe re-enable which might timeout).
        // The framebuffer, GGTT, and aperture are all valid at this point.
        let fb = FramebufferInfo {
            addr: aperture_addr,
            pitch,
            width: timing.width,
            height: timing.height,
            bpp: 32,
        };
        self.fb = Some(fb);
        self.active_timing = Some(timing);

        // Step 11: Enable HDMI 2.0 scrambling BEFORE pipe enable (i915 sequence)
        if needs_scrambling {
            if !self.enable_scrambling() {
                kprintln!("[npk]   WARNING: scrambling failed, display may not sync");
            }
        }

        // Step 12: Re-enable pipe (write to both possible offsets)
        mmio_write32(self.bar0, 0x70008, 1 << 31);  // PIPE_CONF_A
        mmio_write32(self.bar0, 0xF0008, 1 << 31);  // TRANSCONF_A
        // Wait for pipe to start
        for _ in 0..2_000_000u32 { core::hint::spin_loop(); }
        kprintln!("[npk]   Pipe enabled (blind)");

        // Write PIPE_SRCSZ again after enable (some HW needs it live)
        let srcsz = ((timing.width - 1) << 16) | (timing.height - 1);
        mmio_write32(self.bar0, PIPE_SRCSZ_A, srcsz);
        mmio_write32(self.bar0, 0x7001C, srcsz);

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

        // DDI clock routing
        let dpclka = mmio_read32(self.bar0, ICL_DPCLKA_CFGCR0);
        kprintln!("[npk]   DPCLKA_CFGCR0: {:#010x}", dpclka);

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
        let transconf = mmio_read32(self.bar0, PIPE_CONF_A);
        let pipe_src = mmio_read32(self.bar0, PIPE_SRCSZ_A);
        kprintln!("[npk]   PIPE_CONF_A:  {:#010x}  (enabled={})",
            pipe_conf, pipe_conf & (1 << 31) != 0);
        kprintln!("[npk]   PIPE_CONF_A:  {:#010x}  (enabled={})",
            transconf, transconf & (1 << 31) != 0);
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
            //   Bits 47:12 = physical page address
            //   Bit 1 = local memory (0 = system RAM)
            //   Bit 0 = valid/present
            let ggtt_entry: u64 = (phys_addr & 0xFFFF_FFFF_FFFF_F000) | 0x01; // valid, system mem

            let entry_offset = ((start_entry + i) * 8) as u32;
            mmio_write64(ggtt_base, entry_offset, ggtt_entry);
        }

        // Flush GGTT writes with a read-back
        let _ = mmio_read32(self.bar0, GGTT_BASE);

        // Log first and last entries for verification
        let first_off = (start_entry * 8) as u32;
        let last_off = ((start_entry + self.fb_pages - 1) * 8) as u32;
        let first_lo = mmio_read32(self.bar0, GGTT_BASE as u32 + first_off);
        let first_hi = mmio_read32(self.bar0, GGTT_BASE as u32 + first_off + 4);
        let last_lo = mmio_read32(self.bar0, GGTT_BASE as u32 + last_off);
        kprintln!("[npk]   GGTT: {} entries @ offset {:#x} (entry[0]={:#010x}_{:08x}, last_lo={:#010x})",
            self.fb_pages, self.fb_ggtt_offset, first_hi, first_lo, last_lo);
        Ok(())
    }

    /// Program GGTT entries using 32-bit writes (safer for MMIO than 64-bit).
    fn program_ggtt_32(&self) -> Result<(), GpuError> {
        let start_entry = self.fb_ggtt_offset / 4096;

        for i in 0..self.fb_pages {
            let phys_addr = self.fb_phys + (i as u64) * 4096;
            let entry_lo = (phys_addr as u32 & 0xFFFF_F000) | 0x01; // valid, system mem
            let entry_hi = (phys_addr >> 32) as u32;

            let off = GGTT_BASE as u32 + (start_entry + i) * 8;
            mmio_write32(self.bar0, off, entry_lo);
            mmio_write32(self.bar0, off + 4, entry_hi);
        }

        // Flush with read-back
        let _ = mmio_read32(self.bar0, GGTT_BASE);

        // Log first entry for verification
        let first_off = GGTT_BASE as u32 + start_entry * 8;
        let lo = mmio_read32(self.bar0, first_off);
        let hi = mmio_read32(self.bar0, first_off + 4);
        kprintln!("[npk]   GGTT: {} entries @ {:#x} (first={:#010x}_{:08x})",
            self.fb_pages, self.fb_ggtt_offset, hi, lo);
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

    // ── GMBUS I2C (for HDMI SCDC) ──────────────────────────────────

    /// Wait for GMBUS to become idle/ready.
    fn gmbus_wait_idle(&self) -> bool {
        for _ in 0..50_000u32 {
            let st = mmio_read32(self.bar0, GMBUS2);
            if st & GMBUS_ACTIVE == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Wait for GMBUS HW_RDY (data transferred).
    fn gmbus_wait_hw_rdy(&self) -> bool {
        for _ in 0..100_000u32 {
            let st = mmio_read32(self.bar0, GMBUS2);
            if st & GMBUS_NAK != 0 {
                kprintln!("[npk]   GMBUS: NAK received");
                return false;
            }
            if st & GMBUS_HW_RDY != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Reset GMBUS after error or before use.
    fn gmbus_reset(&self) {
        // Set SW_CLR_INT to clear any pending state
        mmio_write32(self.bar0, GMBUS1, GMBUS_SW_CLR_INT);
        mmio_write32(self.bar0, GMBUS1, 0);
        // Select no port
        mmio_write32(self.bar0, GMBUS0, 0);
        let _ = mmio_read32(self.bar0, GMBUS2);
    }

    /// Write a single byte to an I2C register via GMBUS.
    fn gmbus_write_byte(&self, slave_addr: u8, reg: u8, val: u8) -> bool {
        let pin = if self.ddi_port == 1 { GMBUS_PIN_DPB } else { 1 };

        // Select port
        mmio_write32(self.bar0, GMBUS0, pin);

        if !self.gmbus_wait_idle() {
            kprintln!("[npk]   GMBUS: not idle before write");
            self.gmbus_reset();
            return false;
        }

        // Data: reg byte + value byte (little-endian in GMBUS3)
        mmio_write32(self.bar0, GMBUS3, (val as u32) << 8 | reg as u32);

        // Command: 2 bytes, write, slave address, WAIT+STOP cycle
        let cmd = GMBUS_SW_RDY
            | GMBUS_CYCLE_WAIT
            | GMBUS_CYCLE_STOP
            | (2u32 << 16)                          // byte count = 2 (reg + val)
            | ((slave_addr as u32) << 1)            // slave addr (7-bit, shifted)
            | GMBUS_SLAVE_WRITE;
        mmio_write32(self.bar0, GMBUS1, cmd);

        let ok = self.gmbus_wait_hw_rdy();
        // Wait for bus to go idle
        self.gmbus_wait_idle();
        // Clean up
        mmio_write32(self.bar0, GMBUS1, GMBUS_SW_CLR_INT);
        mmio_write32(self.bar0, GMBUS1, 0);
        mmio_write32(self.bar0, GMBUS0, 0);

        if ok {
            kprintln!("[npk]   GMBUS: wrote {:#04x}={:#04x} to slave {:#04x}", reg, val, slave_addr);
        } else {
            kprintln!("[npk]   GMBUS: write FAILED (reg={:#04x} val={:#04x})", reg, val);
        }
        ok
    }

    /// Read a single byte from an I2C register via GMBUS (indexed read).
    fn gmbus_read_byte(&self, slave_addr: u8, reg: u8) -> Option<u8> {
        let pin = if self.ddi_port == 1 { GMBUS_PIN_DPB } else { 1 };

        // Select port
        mmio_write32(self.bar0, GMBUS0, pin);

        if !self.gmbus_wait_idle() {
            kprintln!("[npk]   GMBUS: not idle before read");
            self.gmbus_reset();
            return None;
        }

        // Set index register (GMBUS5) for indexed read
        mmio_write32(self.bar0, GMBUS5, (reg as u32) | (1 << 31)); // index enable

        // Command: 1 byte, read, slave address, INDEX+WAIT+STOP
        let cmd = GMBUS_SW_RDY
            | GMBUS_CYCLE_WAIT
            | GMBUS_CYCLE_STOP
            | GMBUS_CYCLE_INDEX
            | (1u32 << 16)                          // byte count = 1
            | ((slave_addr as u32) << 1)
            | GMBUS_SLAVE_READ;
        mmio_write32(self.bar0, GMBUS1, cmd);

        let ok = self.gmbus_wait_hw_rdy();
        let data = if ok {
            let d = mmio_read32(self.bar0, GMBUS3);
            Some((d & 0xFF) as u8)
        } else {
            None
        };

        self.gmbus_wait_idle();
        mmio_write32(self.bar0, GMBUS5, 0); // disable index
        mmio_write32(self.bar0, GMBUS1, GMBUS_SW_CLR_INT);
        mmio_write32(self.bar0, GMBUS1, 0);
        mmio_write32(self.bar0, GMBUS0, 0);

        data
    }

    // ── HDMI 2.0 Scrambling ─────────────────────────────────────────

    /// Enable HDMI 2.0 scrambling for TMDS >340 MHz (required for 4K@60).
    /// Follows i915 sequence: configure sink (SCDC) FIRST, then source (transcoder).
    /// Retries SCDC writes if monitor isn't connected yet (HDMI input switching).
    fn enable_scrambling(&self) -> bool {
        kprintln!("[npk]   Enabling HDMI 2.0 scrambling...");

        // Step 1: Tell the monitor to enable scrambling via SCDC I2C (BEFORE source).
        // i915 does this in intel_hdmi_handle_sink_scrambling() before DDI enable.
        // Retry up to 10 times with ~500ms pause — monitor may not be
        // connected yet (e.g. HDMI input auto-switching during reboot).
        let mut scdc_ok = false;
        for attempt in 0..10u32 {
            self.gmbus_reset();

            // SCDC TMDS_Config (0x20): bit 0 = scrambling, bit 1 = clock ratio 1/40
            if self.gmbus_write_byte(SCDC_I2C_ADDR, SCDC_TMDS_CONFIG, 0x03) {
                kprintln!("[npk]   SCDC configured (attempt {})", attempt + 1);
                scdc_ok = true;
                break;
            }

            if attempt < 9 {
                kprintln!("[npk]   SCDC write failed (attempt {}), monitor not ready — retrying...",
                    attempt + 1);
                // ~500ms pause
                for _ in 0..50_000_000u32 { core::hint::spin_loop(); }
            }
        }

        if !scdc_ok {
            kprintln!("[npk]   SCDC: all retries failed — monitor may not support HDMI 2.0");
            return false;
        }

        // Step 2: Cycle TRANS_DDI_FUNC_CTL: disable, switch DVI→HDMI, enable scrambling.
        // i915 does a full disable/reconfigure/enable cycle (intel_ddi_disable_transcoder_func
        // + intel_ddi_enable_transcoder_func). Just flipping bits in-place doesn't work.
        let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);
        kprintln!("[npk]   TRANS_DDI_FUNC_CTL before: {:#010x}", ddi_func);

        // Disable transcoder DDI function
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, 0);
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }

        // Also disable + re-enable DDI buffer for clean handshake
        let ddi_ctl = if self.ddi_port == 0 { DDI_BUF_CTL_A } else { DDI_BUF_CTL_B };
        let ddi_buf = mmio_read32(self.bar0, ddi_ctl);
        if ddi_buf & (1 << 31) != 0 {
            mmio_write32(self.bar0, ddi_ctl, ddi_buf & !(1 << 31));
            // Wait for DDI idle (bit 7 = 1 when idle)
            let _ = poll_timeout(self.bar0, ddi_ctl, 1 << 7, 1 << 7, 200_000);
            kprintln!("[npk]   DDI buffer disabled");
            for _ in 0..1_000_000u32 { core::hint::spin_loop(); }
        }

        // Re-enable DDI buffer
        mmio_write32(self.bar0, ddi_ctl, ddi_buf | (1 << 31));
        for _ in 0..1_000_000u32 { core::hint::spin_loop(); }
        kprintln!("[npk]   DDI buffer re-enabled");

        // Write new TRANS_DDI_FUNC_CTL: HDMI mode + scrambling + enable
        let new_func = (ddi_func & !TRANS_DDI_MODE_MASK & !TRANS_DDI_SCRAMBLING_MASK)
            | TRANS_DDI_MODE_HDMI
            | TRANS_DDI_SCRAMBLING_MASK
            | (1 << 31);  // enable
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, new_func);
        kprintln!("[npk]   TRANS_DDI_FUNC_CTL: {:#010x} -> {:#010x} (DVI->HDMI+scrambling)",
            ddi_func, new_func);

        // Step 3: Wait for monitor to lock to scrambled signal (~200ms)
        for _ in 0..20_000_000u32 { core::hint::spin_loop(); }

        // Step 4: Check scrambler status
        match self.gmbus_read_byte(SCDC_I2C_ADDR, SCDC_SCRAMBLER_STATUS) {
            Some(status) => {
                let locked = status & 0x01 != 0;
                kprintln!("[npk]   SCDC Scrambler_Status: {:#04x} (locked={})", status, locked);
                if !locked {
                    kprintln!("[npk]   WARNING: monitor did not lock to scrambled signal");
                }
                true
            }
            None => {
                kprintln!("[npk]   SCDC status read failed — proceeding anyway");
                true
            }
        }
    }

    /// Disable HDMI 2.0 scrambling (for modes <=340 MHz TMDS).
    fn disable_scrambling(&self) {
        // Clear scrambling bits and restore DVI mode
        let ddi_func = mmio_read32(self.bar0, TRANS_DDI_FUNC_CTL_A);
        let new_func = (ddi_func & !TRANS_DDI_SCRAMBLING_MASK & !TRANS_DDI_MODE_MASK)
            | TRANS_DDI_MODE_DVI;
        mmio_write32(self.bar0, TRANS_DDI_FUNC_CTL_A, new_func);

        // Tell monitor to disable scrambling
        self.gmbus_reset();
        let _ = self.gmbus_write_byte(SCDC_I2C_ADDR, SCDC_TMDS_CONFIG, 0x00);
    }
}
