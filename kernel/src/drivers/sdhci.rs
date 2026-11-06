//! SD host controller (SDHCI) on PCI, after Linux 6.18.60
//! drivers/mmc/host/sdhci.c, sdhci-pci-core.c and sdhci-pci-o2micro.c.
//!
//! Bound to the O2 Micro SeaBird (1217:8620/8621) with its fixes, and to
//! QEMU's sdhci-pci (1b36:0007) with none. The card protocol on top is
//! `mmc_sd`. Requests run to completion polled on the interrupt status
//! register; the controller signals no interrupt (SIGNAL_ENABLE stays 0).

use crate::drivers::pci::{self, PciAddr};
use crate::hw::{DmaRegion, Mmio};
use crate::kprintln;

// sdhci.h register map.
pub const SDHCI_DMA_ADDRESS: u32 = 0x00;
pub const SDHCI_ARGUMENT2: u32 = SDHCI_DMA_ADDRESS;
pub const SDHCI_BLOCK_SIZE: u32 = 0x04;
pub const SDHCI_BLOCK_COUNT: u32 = 0x06;
pub const SDHCI_ARGUMENT: u32 = 0x08;
pub const SDHCI_TRANSFER_MODE: u32 = 0x0C;
pub const SDHCI_TRNS_DMA: u16 = 0x01;
pub const SDHCI_TRNS_BLK_CNT_EN: u16 = 0x02;
pub const SDHCI_TRNS_AUTO_CMD12: u16 = 0x04;
pub const SDHCI_TRNS_AUTO_CMD23: u16 = 0x08;
pub const SDHCI_TRNS_READ: u16 = 0x10;
pub const SDHCI_TRNS_MULTI: u16 = 0x20;
pub const SDHCI_COMMAND: u32 = 0x0E;
pub const SDHCI_CMD_CRC: u16 = 0x08;
pub const SDHCI_CMD_INDEX: u16 = 0x10;
pub const SDHCI_CMD_DATA: u16 = 0x20;
pub const SDHCI_CMD_RESP_NONE: u16 = 0x00;
pub const SDHCI_CMD_RESP_LONG: u16 = 0x01;
pub const SDHCI_CMD_RESP_SHORT: u16 = 0x02;
pub const SDHCI_CMD_RESP_SHORT_BUSY: u16 = 0x03;
pub const SDHCI_RESPONSE: u32 = 0x10;
pub const SDHCI_PRESENT_STATE: u32 = 0x24;
pub const SDHCI_CMD_INHIBIT: u32 = 0x0000_0001;
pub const SDHCI_DATA_INHIBIT: u32 = 0x0000_0002;
pub const SDHCI_CARD_PRESENT: u32 = 0x0001_0000;
pub const SDHCI_CARD_PRES_SHIFT: u32 = 16;
pub const SDHCI_CD_LVL: u32 = 0x0004_0000;
pub const SDHCI_CD_LVL_SHIFT: u32 = 18;
pub const SDHCI_WRITE_PROTECT: u32 = 0x0008_0000;
pub const SDHCI_DATA_0_LVL_MASK: u32 = 0x0010_0000;
pub const SDHCI_HOST_CONTROL: u32 = 0x28;
pub const SDHCI_CTRL_4BITBUS: u8 = 0x02;
pub const SDHCI_CTRL_HISPD: u8 = 0x04;
pub const SDHCI_CTRL_DMA_MASK: u8 = 0x18;
pub const SDHCI_CTRL_ADMA32: u8 = 0x10;
pub const SDHCI_CTRL_8BITBUS: u8 = 0x20;
pub const SDHCI_POWER_CONTROL: u32 = 0x29;
pub const SDHCI_POWER_ON: u8 = 0x01;
pub const SDHCI_POWER_180: u8 = 0x0A;
pub const SDHCI_POWER_300: u8 = 0x0C;
pub const SDHCI_POWER_330: u8 = 0x0E;
pub const SDHCI_CLOCK_CONTROL: u32 = 0x2C;
pub const SDHCI_DIVIDER_SHIFT: u32 = 8;
pub const SDHCI_DIVIDER_HI_SHIFT: u32 = 6;
pub const SDHCI_DIV_MASK: u32 = 0xFF;
pub const SDHCI_DIV_MASK_LEN: u32 = 8;
pub const SDHCI_DIV_HI_MASK: u32 = 0x300;
pub const SDHCI_PROG_CLOCK_MODE: u16 = 0x0020;
pub const SDHCI_CLOCK_CARD_EN: u16 = 0x0004;
pub const SDHCI_CLOCK_INT_STABLE: u16 = 0x0002;
pub const SDHCI_CLOCK_INT_EN: u16 = 0x0001;
pub const SDHCI_TIMEOUT_CONTROL: u32 = 0x2E;
pub const SDHCI_SOFTWARE_RESET: u32 = 0x2F;
pub const SDHCI_RESET_ALL: u8 = 0x01;
pub const SDHCI_RESET_CMD: u8 = 0x02;
pub const SDHCI_RESET_DATA: u8 = 0x04;
pub const SDHCI_INT_STATUS: u32 = 0x30;
pub const SDHCI_INT_ENABLE: u32 = 0x34;
pub const SDHCI_SIGNAL_ENABLE: u32 = 0x38;
pub const SDHCI_INT_RESPONSE: u32 = 0x0000_0001;
pub const SDHCI_INT_DATA_END: u32 = 0x0000_0002;
pub const SDHCI_INT_BLK_GAP: u32 = 0x0000_0004;
pub const SDHCI_INT_DMA_END: u32 = 0x0000_0008;
pub const SDHCI_INT_SPACE_AVAIL: u32 = 0x0000_0010;
pub const SDHCI_INT_DATA_AVAIL: u32 = 0x0000_0020;
pub const SDHCI_INT_CARD_INSERT: u32 = 0x0000_0040;
pub const SDHCI_INT_CARD_REMOVE: u32 = 0x0000_0080;
pub const SDHCI_INT_RETUNE: u32 = 0x0000_1000;
pub const SDHCI_INT_TIMEOUT: u32 = 0x0001_0000;
pub const SDHCI_INT_CRC: u32 = 0x0002_0000;
pub const SDHCI_INT_END_BIT: u32 = 0x0004_0000;
pub const SDHCI_INT_INDEX: u32 = 0x0008_0000;
pub const SDHCI_INT_DATA_TIMEOUT: u32 = 0x0010_0000;
pub const SDHCI_INT_DATA_CRC: u32 = 0x0020_0000;
pub const SDHCI_INT_DATA_END_BIT: u32 = 0x0040_0000;
pub const SDHCI_INT_BUS_POWER: u32 = 0x0080_0000;
pub const SDHCI_INT_AUTO_CMD_ERR: u32 = 0x0100_0000;
pub const SDHCI_INT_ADMA_ERROR: u32 = 0x0200_0000;
pub const SDHCI_INT_TUNING_ERROR: u32 = 0x0400_0000;
pub const SDHCI_INT_CMD_MASK: u32 = SDHCI_INT_RESPONSE | SDHCI_INT_TIMEOUT | SDHCI_INT_CRC
    | SDHCI_INT_END_BIT | SDHCI_INT_INDEX | SDHCI_INT_AUTO_CMD_ERR;
pub const SDHCI_INT_DATA_MASK: u32 = SDHCI_INT_DATA_END | SDHCI_INT_DMA_END | SDHCI_INT_DATA_AVAIL
    | SDHCI_INT_SPACE_AVAIL | SDHCI_INT_DATA_TIMEOUT | SDHCI_INT_DATA_CRC | SDHCI_INT_DATA_END_BIT
    | SDHCI_INT_ADMA_ERROR | SDHCI_INT_BLK_GAP | SDHCI_INT_TUNING_ERROR;
pub const SDHCI_AUTO_CMD_STATUS: u32 = 0x3C;
pub const SDHCI_AUTO_CMD_TIMEOUT: u16 = 0x0002;
pub const SDHCI_HOST_CONTROL2: u32 = 0x3E;
pub const SDHCI_CTRL_UHS_MASK: u16 = 0x0007;
pub const SDHCI_CTRL_UHS_SDR12: u16 = 0x0000;
pub const SDHCI_CTRL_UHS_SDR25: u16 = 0x0001;
pub const SDHCI_CTRL_UHS_SDR50: u16 = 0x0002;
pub const SDHCI_CTRL_UHS_SDR104: u16 = 0x0003;
pub const SDHCI_CTRL_UHS_DDR50: u16 = 0x0004;
pub const SDHCI_CTRL_VDD_180: u16 = 0x0008;
pub const SDHCI_CTRL_DRV_TYPE_MASK: u16 = 0x0030;
pub const SDHCI_CTRL_DRV_TYPE_B: u16 = 0x0000;
pub const SDHCI_CTRL_DRV_TYPE_A: u16 = 0x0010;
pub const SDHCI_CTRL_DRV_TYPE_C: u16 = 0x0020;
pub const SDHCI_CTRL_DRV_TYPE_D: u16 = 0x0030;
pub const SDHCI_CTRL_EXEC_TUNING: u16 = 0x0040;
pub const SDHCI_CTRL_TUNED_CLK: u16 = 0x0080;
pub const SDHCI_CAPABILITIES: u32 = 0x40;
pub const SDHCI_TIMEOUT_CLK_MASK: u32 = 0x3F;
pub const SDHCI_TIMEOUT_CLK_UNIT: u32 = 0x0000_0080;
pub const SDHCI_CLOCK_BASE_MASK: u32 = 0x3F << 8;
pub const SDHCI_CLOCK_V3_BASE_MASK: u32 = 0xFF << 8;
pub const SDHCI_CAN_DO_8BIT: u32 = 0x0004_0000;
pub const SDHCI_CAN_DO_ADMA2: u32 = 0x0008_0000;
pub const SDHCI_CAN_DO_HISPD: u32 = 0x0020_0000;
pub const SDHCI_CAN_VDD_330: u32 = 0x0100_0000;
pub const SDHCI_CAN_VDD_300: u32 = 0x0200_0000;
pub const SDHCI_CAN_VDD_180: u32 = 0x0400_0000;
pub const SDHCI_CAPABILITIES_1: u32 = 0x44;
pub const SDHCI_SUPPORT_SDR50: u32 = 0x0000_0001;
pub const SDHCI_SUPPORT_SDR104: u32 = 0x0000_0002;
pub const SDHCI_SUPPORT_DDR50: u32 = 0x0000_0004;
pub const SDHCI_DRIVER_TYPE_A: u32 = 0x0000_0010;
pub const SDHCI_DRIVER_TYPE_C: u32 = 0x0000_0020;
pub const SDHCI_DRIVER_TYPE_D: u32 = 0x0000_0040;
pub const SDHCI_USE_SDR50_TUNING: u32 = 0x0000_2000;
pub const SDHCI_RETUNING_MODE_MASK: u32 = 0x3 << 14;
pub const SDHCI_CLOCK_MUL_MASK: u32 = 0xFF << 16;
pub const SDHCI_MAX_CURRENT: u32 = 0x48;
pub const SDHCI_MAX_CURRENT_MULTIPLIER: u32 = 4;
pub const SDHCI_ADMA_ADDRESS: u32 = 0x58;
pub const SDHCI_HOST_VERSION: u32 = 0xFE;
pub const SDHCI_SPEC_VER_MASK: u16 = 0x00FF;
pub const SDHCI_SPEC_200: u8 = 1;
pub const SDHCI_SPEC_300: u8 = 2;
pub const SDHCI_MAX_DIV_SPEC_200: u32 = 256;
pub const SDHCI_MAX_DIV_SPEC_300: u32 = 2046;
/// ilog2(SDHCI_DEFAULT_BOUNDARY_SIZE) - 12 for 512 KiB.
pub const SDHCI_DEFAULT_BOUNDARY_ARG: u16 = 7;
pub const ADMA2_TRAN_VALID: u16 = 0x21;
pub const ADMA2_NOP_END_VALID: u16 = 0x3;
pub const ADMA2_END: u16 = 0x2;
pub const SDHCI_ADMA2_32_DESC_SZ: usize = 8;
/// sdhci.c MAX_TUNING_LOOP.
const MAX_TUNING_LOOP: u32 = 40;

const fn sdhci_make_blksz(dma: u16, blksz: u16) -> u16 {
    ((dma & 0x7) << 12) | (blksz & 0xFFF)
}

// sdhci_host flags.
const SDHCI_USE_ADMA: u32 = 1 << 1;
const SDHCI_REQ_USE_DMA: u32 = 1 << 2;
const SDHCI_DEVICE_DEAD: u32 = 1 << 3;
const SDHCI_SDR50_NEEDS_TUNING: u32 = 1 << 4;
const SDHCI_AUTO_CMD12: u32 = 1 << 6;
const SDHCI_AUTO_CMD23: u32 = 1 << 7;
const SDHCI_SIGNALING_330: u32 = 1 << 14;
const SDHCI_SIGNALING_180: u32 = 1 << 15;

// Quirks the bound controllers carry.
const SDHCI_QUIRK_MULTIBLOCK_READ_ACMD12: u32 = 1 << 0;
const SDHCI_QUIRK_NO_ENDATTR_IN_NOPDESC: u32 = 1 << 1;
const SDHCI_QUIRK2_PRESET_VALUE_BROKEN: u32 = 1 << 0;
const SDHCI_QUIRK2_BROKEN_DDR50: u32 = 1 << 1;
const SDHCI_QUIRK2_CLEAR_TRANSFERMODE_REG_BEFORE_CMD: u32 = 1 << 2;

// mmc_host caps (include/linux/mmc/host.h), the ones used here.
pub const MMC_CAP_4_BIT_DATA: u32 = 1 << 0;
pub const MMC_CAP_SD_HIGHSPEED: u32 = 1 << 2;
pub const MMC_CAP_8_BIT_DATA: u32 = 1 << 6;
pub const MMC_CAP_UHS_SDR12: u32 = 1 << 16;
pub const MMC_CAP_UHS_SDR25: u32 = 1 << 17;
pub const MMC_CAP_UHS_SDR50: u32 = 1 << 18;
pub const MMC_CAP_UHS_SDR104: u32 = 1 << 19;
pub const MMC_CAP_UHS_DDR50: u32 = 1 << 20;
pub const MMC_CAP_DRIVER_TYPE_A: u32 = 1 << 23;
pub const MMC_CAP_DRIVER_TYPE_C: u32 = 1 << 24;
pub const MMC_CAP_DRIVER_TYPE_D: u32 = 1 << 25;
pub const MMC_CAP_CMD23: u32 = 1 << 30;
pub const MMC_CAP_UHS: u32 = MMC_CAP_UHS_SDR12 | MMC_CAP_UHS_SDR25 | MMC_CAP_UHS_SDR50
    | MMC_CAP_UHS_SDR104 | MMC_CAP_UHS_DDR50;

// include/linux/mmc/host.h
pub const MMC_VDD_165_195: u32 = 0x0000_0080;
pub const MMC_VDD_20_21: u32 = 0x0000_0100;
pub const MMC_VDD_29_30: u32 = 0x0002_0000;
pub const MMC_VDD_30_31: u32 = 0x0004_0000;
pub const MMC_VDD_32_33: u32 = 0x0010_0000;
pub const MMC_VDD_33_34: u32 = 0x0020_0000;
pub const MMC_VDD_34_35: u32 = 0x0040_0000;
pub const MMC_VDD_35_36: u32 = 0x0080_0000;

pub const MMC_POWER_OFF: u8 = 0;
pub const MMC_POWER_UP: u8 = 1;
pub const MMC_POWER_ON: u8 = 2;

pub const MMC_BUS_WIDTH_1: u8 = 0;
pub const MMC_BUS_WIDTH_4: u8 = 2;
pub const MMC_BUS_WIDTH_8: u8 = 3;

pub const MMC_TIMING_LEGACY: u8 = 0;
pub const MMC_TIMING_SD_HS: u8 = 2;
pub const MMC_TIMING_UHS_SDR12: u8 = 3;
pub const MMC_TIMING_UHS_SDR25: u8 = 4;
pub const MMC_TIMING_UHS_SDR50: u8 = 5;
pub const MMC_TIMING_UHS_SDR104: u8 = 6;
pub const MMC_TIMING_UHS_DDR50: u8 = 7;

pub const MMC_SIGNAL_VOLTAGE_330: u8 = 0;
pub const MMC_SIGNAL_VOLTAGE_180: u8 = 1;

pub const MMC_SET_DRIVER_TYPE_A: u8 = 1;
pub const MMC_SET_DRIVER_TYPE_C: u8 = 2;
pub const MMC_SET_DRIVER_TYPE_D: u8 = 3;

// include/linux/mmc/core.h response flags.
pub const MMC_RSP_PRESENT: u32 = 1 << 0;
pub const MMC_RSP_136: u32 = 1 << 1;
pub const MMC_RSP_CRC: u32 = 1 << 2;
pub const MMC_RSP_BUSY: u32 = 1 << 3;
pub const MMC_RSP_OPCODE: u32 = 1 << 4;
pub const MMC_RSP_NONE: u32 = 0;
pub const MMC_RSP_R1: u32 = MMC_RSP_PRESENT | MMC_RSP_CRC | MMC_RSP_OPCODE;
pub const MMC_RSP_R1B: u32 = MMC_RSP_R1 | MMC_RSP_BUSY;
pub const MMC_RSP_R2: u32 = MMC_RSP_PRESENT | MMC_RSP_136 | MMC_RSP_CRC;
pub const MMC_RSP_R3: u32 = MMC_RSP_PRESENT;
pub const MMC_RSP_R6: u32 = MMC_RSP_R1;
pub const MMC_RSP_R7: u32 = MMC_RSP_R1;

pub const MMC_STOP_TRANSMISSION: u8 = 12;
pub const MMC_SEND_TUNING_BLOCK: u8 = 19;

// sdhci-pci-o2micro.c
const O2_SD_MISC_CTRL2: u8 = 0xF0;
const O2_SD_LOCK_WP: u8 = 0xD3;
const O2_SD_MISC_CTRL: u32 = 0x1C0;
const O2_SD_PWR_FORCE_L0: u16 = 0x0002;
const O2_SD_TUNING_CTRL: u16 = 0x300;
const O2_SD_PLL_SETTING: u16 = 0x304;
const O2_SD_OUTPUT_CLK_SOURCE_SWITCH: u16 = 0x354;
const O2_SD_FUNC_REG4: u16 = 0x3E4;
const O2_SD_SEL_DLL: u32 = 1 << 16;
const O2_SD_FREG4_ENABLE_CLK_SET: u32 = 1 << 22;
const O2_SD_PHASE_MASK: u32 = 0xF << 20;
const O2_SD_FIX_PHASE: u32 = 0x9 << 20;
const O2_SD_VENDOR_SETTING: u32 = 0x110;
const O2_SD_HW_TUNING_DISABLE: u16 = 1 << 4;
const O2_PLL_DLL_WDT_CONTROL1: u32 = 0x1CC;
const O2_PLL_FORCE_ACTIVE: u32 = 1 << 18;
const O2_PLL_LOCK_STATUS: u32 = 1 << 14;
const O2_PLL_SOFT_RESET: u32 = 1 << 12;
const O2_DLL_LOCK_STATUS: u32 = 1 << 11;
const O2_SD_DETECT_SETTING: u16 = 0x324;
const DMDN_TABLE: [u32; 4] = [0x2B1C_0000, 0x2C1A_0000, 0x371B_0000, 0x3510_0000];

pub const PCI_VENDOR_ID_O2: u16 = 0x1217;
pub const PCI_DEVICE_ID_O2_SEABIRD0: u16 = 0x8620;
pub const PCI_DEVICE_ID_O2_SEABIRD1: u16 = 0x8621;
const PCI_VENDOR_ID_REDHAT: u16 = 0x1b36;
const PCI_DEVICE_ID_REDHAT_SDHCI: u16 = 0x0007;
/// sdhci-pci.h PCI_SLOT_INFO.
const PCI_SLOT_INFO: u8 = 0x40;

/// Largest request (`mmc->max_req_size`), and the DMA buffer behind it.
pub const MAX_REQ: usize = 524_288;
const MAX_ADMA: usize = 65_536;

/// `struct mmc_ios`: what the core asks the host to drive.
#[derive(Clone, Copy)]
pub struct Ios {
    pub clock: u32,
    pub vdd: u8,
    pub power_mode: u8,
    pub bus_width: u8,
    pub timing: u8,
    pub signal_voltage: u8,
    pub drv_type: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// -ETIMEDOUT
    Timeout,
    /// -EILSEQ: CRC, end bit or index.
    Ilseq,
    /// -EIO
    Io,
    /// -ENOMEDIUM
    NoMedium,
    /// -EINVAL / -EOPNOTSUPP
    Invalid,
    /// -EAGAIN
    Again,
}

/// `struct mmc_command`.
pub struct Command {
    pub opcode: u8,
    pub arg: u32,
    pub flags: u32,
    pub resp: [u32; 4],
    /// For R1B without data, in ms.
    pub busy_timeout: u32,
}

impl Command {
    pub fn new(opcode: u8, arg: u32, flags: u32) -> Self {
        Command { opcode, arg, flags, resp: [0; 4], busy_timeout: 0 }
    }
}

/// `struct mmc_data`. The data sits at the start of the host's DMA buffer.
pub struct Data {
    pub blksz: u32,
    pub blocks: u32,
    pub write: bool,
    pub timeout_ns: u32,
    pub timeout_clks: u32,
}

/// How a multi-block transfer ends.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    None,
    /// CMD23 sent by the controller (`sdhci_auto_cmd23`).
    AutoCmd23,
    /// CMD23 sent first by the driver.
    ManualCmd23,
    /// CMD12 sent by the controller (`sdhci_auto_cmd12`).
    AutoCmd12,
    /// CMD12 sent by the driver after the data (`data->stop`).
    ManualCmd12,
}

pub struct Host {
    regs: Mmio,
    pub pci: PciAddr,
    device: u16,
    is_o2: bool,
    pub name: &'static str,
    pub version: u8,
    caps: u32,
    caps1: u32,
    quirks: u32,
    quirks2: u32,
    flags: u32,
    pub mmc_caps: u32,
    pub max_clk: u32,
    clk_mul: u32,
    /// kHz
    timeout_clk: u32,
    max_timeout_count: u8,
    pub f_min: u32,
    pub f_max: u32,
    pub ocr_avail: u32,
    pub max_current_330: u32,
    pub max_current_300: u32,
    pub max_current_180: u32,
    tuning_mode: u32,
    ier: u32,
    pwr: u8,
    /// `host->clock`
    clock: u32,
    pub actual_clock: u32,
    timing: u8,
    drv_type: u8,
    reinit_uhs: bool,
    pub ios: Ios,
    adma: DmaRegion,
    /// The data of a request, `MAX_REQ` bytes below 4 GiB.
    pub buf: DmaRegion,
    dll_adjust_count: u8,
    pub tuning_done: bool,
}

fn now_us() -> u64 {
    crate::interrupts::rdtsc() / (crate::interrupts::tsc_freq() / 1_000_000).max(1)
}

pub fn udelay(us: u64) {
    let end = now_us() + us;
    while now_us() < end {
        core::hint::spin_loop();
    }
}

/// `mmc_delay` / `usleep_range`: yields the core when it can.
pub fn mdelay(ms: u64) {
    if !crate::smp::fiber::yield_sleep(ms) {
        crate::interrupts::delay_ms(ms);
    }
}

/// Probe the first supported controller.
pub fn probe() -> Option<Host> {
    let table: [(u16, u16, &'static str, bool); 3] = [
        (PCI_VENDOR_ID_O2, PCI_DEVICE_ID_O2_SEABIRD1, "O2 Micro SeaBird", true),
        (PCI_VENDOR_ID_O2, PCI_DEVICE_ID_O2_SEABIRD0, "O2 Micro SeaBird", true),
        (PCI_VENDOR_ID_REDHAT, PCI_DEVICE_ID_REDHAT_SDHCI, "QEMU SDHCI", false),
    ];
    let Some((dev, name, is_o2)) = table.iter()
        .find_map(|&(v, d, n, o2)| pci::find_device(v, d).map(|dev| (dev, n, o2))) else {
        crate::kdebug!("[npk] sd: no supported SD host controller");
        return None;
    };
    let addr = dev.addr;
    if pci::claim(addr, pci::Owner::Kernel("sdhci")).is_err() {
        kprintln!("[npk] sd: {:02x}:{:02x}.{} is owned already", addr.bus, addr.device, addr.function);
        return None;
    }
    // sdhci_pci_probe: one slot, its BAR from PCI_SLOT_INFO.
    let info = pci::read8(addr, PCI_SLOT_INFO);
    let first_bar = (info & 7) as usize;
    let Some(bar) = pci::bar(addr, first_bar).filter(|b| b.base != 0) else {
        kprintln!("[npk] sd: BAR {} is not iomem", first_bar);
        return None;
    };
    let len = bar.size.clamp(0x100, 0x1000);
    // SAFETY: the memory BAR of the SD host controller this driver claimed.
    let regs = match unsafe { Mmio::map(bar.base, len) } {
        Ok(m) => m,
        Err(e) => { kprintln!("[npk] sd: BAR map failed: {:?}", e); return None; }
    };
    let (Some(adma), Some(buf)) = (DmaRegion::alloc_zeroed_dma32_aligned(1),
        DmaRegion::alloc_zeroed_dma32_aligned(MAX_REQ / 4096)) else {
        kprintln!("[npk] sd: no DMA memory below 4 GiB");
        return None;
    };
    let mut host = Host {
        regs, pci: addr, device: dev.device_id, is_o2, name,
        version: 0, caps: 0, caps1: 0, quirks: 0, quirks2: 0,
        flags: SDHCI_SIGNALING_330, mmc_caps: 0, max_clk: 0, clk_mul: 0,
        timeout_clk: 0, max_timeout_count: 0xE, f_min: 0, f_max: 0, ocr_avail: 0,
        max_current_330: 0, max_current_300: 0, max_current_180: 0,
        tuning_mode: 0, ier: 0, pwr: 0, clock: 0, actual_clock: 0,
        timing: MMC_TIMING_LEGACY, drv_type: 0, reinit_uhs: false,
        ios: Ios { clock: 0, vdd: 0, power_mode: MMC_POWER_OFF, bus_width: MMC_BUS_WIDTH_1,
            timing: MMC_TIMING_LEGACY, signal_voltage: MMC_SIGNAL_VOLTAGE_330, drv_type: 0 },
        adma, buf, dll_adjust_count: 0, tuning_done: false,
    };
    if is_o2 {
        // sdhci_o2 fixes
        host.quirks |= SDHCI_QUIRK_NO_ENDATTR_IN_NOPDESC;
        host.quirks2 |= SDHCI_QUIRK2_CLEAR_TRANSFERMODE_REG_BEFORE_CMD;
        host.o2_probe();
    }
    pci::enable_bus_master(addr);
    if is_o2 {
        host.o2_probe_slot();
    }
    if let Err(e) = host.setup_host() {
        kprintln!("[npk] sd: {} setup failed: {:?}", name, e);
        return None;
    }
    host.init(false);
    host.set_card_detection(true);
    kprintln!("[npk] sd: {} [{:04x}:{:04x}] at {:02x}:{:02x}.{}, SDHCI spec {}, base {} MHz{}{}",
        name, dev.vendor_id, dev.device_id, addr.bus, addr.device, addr.function,
        match host.version { 0 => "1.00", 1 => "2.00", 2 => "3.00", 3 => "4.00", 4 => "4.10", _ => "4.20" },
        host.max_clk / 1_000_000,
        if host.mmc_caps & MMC_CAP_UHS_SDR104 != 0 { ", SDR104" }
        else if host.mmc_caps & MMC_CAP_UHS_SDR50 != 0 { ", SDR50" } else { "" },
        if host.flags & SDHCI_USE_ADMA != 0 { ", ADMA2" } else { "" });
    Some(host)
}

impl Host {
    pub fn has_auto_cmd12(&self) -> bool { self.flags & SDHCI_AUTO_CMD12 != 0 }
    pub fn has_auto_cmd23(&self) -> bool { self.flags & SDHCI_AUTO_CMD23 != 0 }

    pub fn readl(&self, reg: u32) -> u32 { self.regs.r32(reg) }
    pub fn readw(&self, reg: u32) -> u16 { self.regs.r16(reg) }
    pub fn readb(&self, reg: u32) -> u8 { self.regs.r8(reg) }
    pub fn writel(&self, v: u32, reg: u32) { self.regs.w32(reg, v) }
    pub fn writew(&self, v: u16, reg: u32) { self.regs.w16(reg, v) }
    pub fn writeb(&self, v: u8, reg: u32) { self.regs.w8(reg, v) }

    fn cfg_read32(&self, off: u16) -> u32 {
        if off < 0x100 { pci::read32(self.pci, off as u8) } else { pci::read32_ext(self.pci, off).unwrap_or(0) }
    }
    fn cfg_write32(&self, off: u16, v: u32) {
        if off < 0x100 { pci::write32(self.pci, off as u8, v) } else { pci::write32_ext(self.pci, off, v); }
    }
    fn cfg_read8(&self, off: u16) -> u8 {
        (self.cfg_read32(off & !3) >> ((off & 3) * 8)) as u8
    }
    fn cfg_write8(&self, off: u16, v: u8) {
        if off < 0x100 { pci::write8(self.pci, off as u8, v) } else { pci::write8_ext(self.pci, off, v); }
    }

    // ── sdhci.c: reset, init, irqs, DMA ─────────────────────────────

    pub fn reset(&mut self, mask: u8) {
        self.writeb(mask, SDHCI_SOFTWARE_RESET);
        if mask & SDHCI_RESET_ALL != 0 {
            self.clock = 0;
        }
        let deadline = now_us() + 100_000;
        loop {
            let timedout = now_us() > deadline;
            if self.readb(SDHCI_SOFTWARE_RESET) & mask == 0 { break; }
            if timedout {
                kprintln!("[npk] sd: reset {:#x} never completed", mask);
                return;
            }
            udelay(10);
        }
    }

    fn reset_for_all(&mut self) {
        self.reset(SDHCI_RESET_ALL);
        // sdhci_pci_enable_dma: bus master only, done at probe.
    }

    pub fn reset_for_request_error(&mut self) {
        self.reset(SDHCI_RESET_CMD);
        self.reset(SDHCI_RESET_DATA);
    }

    fn set_default_irqs(&mut self) {
        self.ier = SDHCI_INT_BUS_POWER | SDHCI_INT_DATA_END_BIT | SDHCI_INT_DATA_CRC
            | SDHCI_INT_DATA_TIMEOUT | SDHCI_INT_INDEX | SDHCI_INT_END_BIT | SDHCI_INT_CRC
            | SDHCI_INT_TIMEOUT | SDHCI_INT_DATA_END | SDHCI_INT_RESPONSE;
        if self.tuning_mode == 1 || self.tuning_mode == 2 {
            self.ier |= SDHCI_INT_RETUNE;
        }
        self.write_ier();
    }

    /// INT_ENABLE latches the status bits that are polled; nothing is
    /// signalled as an interrupt.
    fn write_ier(&self) {
        self.writel(self.ier, SDHCI_INT_ENABLE);
        self.writel(0, SDHCI_SIGNAL_ENABLE);
    }

    pub fn set_card_detection(&mut self, enable: bool) {
        self.ier &= !(SDHCI_INT_CARD_REMOVE | SDHCI_INT_CARD_INSERT);
        if enable {
            let present = self.readl(SDHCI_PRESENT_STATE) & SDHCI_CARD_PRESENT != 0;
            self.ier |= if present { SDHCI_INT_CARD_REMOVE } else { SDHCI_INT_CARD_INSERT };
        }
        self.write_ier();
    }

    fn config_dma(&self) {
        if self.version < SDHCI_SPEC_200 { return; }
        let mut ctrl = self.readb(SDHCI_HOST_CONTROL);
        ctrl &= !SDHCI_CTRL_DMA_MASK;
        if self.flags & SDHCI_REQ_USE_DMA != 0 && self.flags & SDHCI_USE_ADMA != 0 {
            ctrl |= SDHCI_CTRL_ADMA32;
        }
        self.writeb(ctrl, SDHCI_HOST_CONTROL);
    }

    pub fn init(&mut self, soft: bool) {
        if soft {
            self.reset(SDHCI_RESET_CMD | SDHCI_RESET_DATA);
        } else {
            self.reset_for_all();
        }
        self.set_default_irqs();
        if soft {
            self.clock = 0;
            self.reinit_uhs = true;
            let ios = self.ios;
            self.set_ios(&ios);
        }
    }

    fn reinit(&mut self) {
        self.init(false);
        self.set_card_detection(true);
    }

    /// Card insert/remove latched since the last call (`sdhci_irq`).
    pub fn take_card_event(&mut self) -> bool {
        let st = self.readl(SDHCI_INT_STATUS) & (SDHCI_INT_CARD_INSERT | SDHCI_INT_CARD_REMOVE);
        if st == 0 { return false; }
        self.writel(st, SDHCI_INT_STATUS);
        self.set_card_detection(true);
        true
    }

    // ── sdhci_setup_host ─────────────────────────────────────────────

    fn setup_host(&mut self) -> Result<(), Error> {
        // __sdhci_read_caps
        self.reset_for_all();
        let v = self.readw(SDHCI_HOST_VERSION);
        self.version = (v & SDHCI_SPEC_VER_MASK) as u8;
        self.caps = self.readl(SDHCI_CAPABILITIES);
        if self.version >= SDHCI_SPEC_300 {
            self.caps1 = self.readl(SDHCI_CAPABILITIES_1);
        }
        if self.version >= SDHCI_SPEC_200 && self.caps & SDHCI_CAN_DO_ADMA2 != 0 {
            self.flags |= SDHCI_USE_ADMA;
        }
        if self.flags & SDHCI_USE_ADMA == 0 {
            kprintln!("[npk] sd: controller without ADMA2 is not driven");
            return Err(Error::Invalid);
        }
        self.max_clk = if self.version >= SDHCI_SPEC_300 {
            (self.caps & SDHCI_CLOCK_V3_BASE_MASK) >> 8
        } else {
            (self.caps & SDHCI_CLOCK_BASE_MASK) >> 8
        } * 1_000_000;
        if self.max_clk == 0 {
            kprintln!("[npk] sd: hardware doesn't specify base clock frequency");
            return Err(Error::Invalid);
        }
        self.clk_mul = (self.caps1 & SDHCI_CLOCK_MUL_MASK) >> 16;
        if self.clk_mul != 0 { self.clk_mul += 1; }
        let mut max_clk = self.max_clk;
        if self.version >= SDHCI_SPEC_300 {
            if self.clk_mul != 0 { max_clk = self.max_clk * self.clk_mul; }
            self.f_min = self.max_clk / SDHCI_MAX_DIV_SPEC_300;
        } else {
            self.f_min = self.max_clk / SDHCI_MAX_DIV_SPEC_200;
        }
        if self.f_max == 0 || self.f_max > max_clk { self.f_max = max_clk; }
        self.timeout_clk = self.caps & SDHCI_TIMEOUT_CLK_MASK;
        if self.caps & SDHCI_TIMEOUT_CLK_UNIT != 0 { self.timeout_clk *= 1000; }
        if self.timeout_clk == 0 {
            kprintln!("[npk] sd: hardware doesn't specify timeout clock frequency");
            return Err(Error::Invalid);
        }
        self.mmc_caps |= MMC_CAP_CMD23;
        if self.quirks & SDHCI_QUIRK_MULTIBLOCK_READ_ACMD12 != 0 {
            self.flags |= SDHCI_AUTO_CMD12;
        }
        if self.version >= SDHCI_SPEC_300 {
            self.flags |= SDHCI_AUTO_CMD23;
        }
        self.mmc_caps |= MMC_CAP_4_BIT_DATA;
        if self.caps & SDHCI_CAN_DO_HISPD != 0 {
            self.mmc_caps |= MMC_CAP_SD_HIGHSPEED;
        }
        if self.caps1 & (SDHCI_SUPPORT_SDR104 | SDHCI_SUPPORT_SDR50 | SDHCI_SUPPORT_DDR50) != 0 {
            self.mmc_caps |= MMC_CAP_UHS_SDR12 | MMC_CAP_UHS_SDR25;
        }
        if self.caps1 & SDHCI_SUPPORT_SDR104 != 0 {
            self.mmc_caps |= MMC_CAP_UHS_SDR104 | MMC_CAP_UHS_SDR50;
        } else if self.caps1 & SDHCI_SUPPORT_SDR50 != 0 {
            self.mmc_caps |= MMC_CAP_UHS_SDR50;
        }
        if self.caps1 & SDHCI_SUPPORT_DDR50 != 0 && self.quirks2 & SDHCI_QUIRK2_BROKEN_DDR50 == 0 {
            self.mmc_caps |= MMC_CAP_UHS_DDR50;
        }
        if self.caps1 & SDHCI_USE_SDR50_TUNING != 0 {
            self.flags |= SDHCI_SDR50_NEEDS_TUNING;
        }
        if self.caps1 & SDHCI_DRIVER_TYPE_A != 0 { self.mmc_caps |= MMC_CAP_DRIVER_TYPE_A; }
        if self.caps1 & SDHCI_DRIVER_TYPE_C != 0 { self.mmc_caps |= MMC_CAP_DRIVER_TYPE_C; }
        if self.caps1 & SDHCI_DRIVER_TYPE_D != 0 { self.mmc_caps |= MMC_CAP_DRIVER_TYPE_D; }
        self.tuning_mode = (self.caps1 & SDHCI_RETUNING_MODE_MASK) >> 14;

        let max_current_caps = self.readl(SDHCI_MAX_CURRENT);
        let mut ocr_avail = 0;
        if self.caps & SDHCI_CAN_VDD_330 != 0 {
            ocr_avail |= MMC_VDD_32_33 | MMC_VDD_33_34;
            self.max_current_330 = (max_current_caps & 0xFF) * SDHCI_MAX_CURRENT_MULTIPLIER;
        }
        if self.caps & SDHCI_CAN_VDD_300 != 0 {
            ocr_avail |= MMC_VDD_29_30 | MMC_VDD_30_31;
            self.max_current_300 = ((max_current_caps >> 8) & 0xFF) * SDHCI_MAX_CURRENT_MULTIPLIER;
        }
        if self.caps & SDHCI_CAN_VDD_180 != 0 {
            ocr_avail |= MMC_VDD_165_195;
            self.max_current_180 = ((max_current_caps >> 16) & 0xFF) * SDHCI_MAX_CURRENT_MULTIPLIER;
        }
        // mmc->ocr_avail_sd: normal SD controllers don't support 1.8 V VDD.
        self.ocr_avail = ocr_avail & !MMC_VDD_165_195;
        if self.ocr_avail == 0 {
            kprintln!("[npk] sd: hardware doesn't report any support voltages");
            return Err(Error::Invalid);
        }
        if self.mmc_caps & MMC_CAP_UHS != 0 {
            self.flags |= SDHCI_SIGNALING_180;
        }
        Ok(())
    }

    // ── clock ─────────────────────────────────────────────────────────

    fn calc_clk(&mut self, clock: u32) -> u16 {
        let mut div: u32;
        let real_div: u32;
        let mut clk_mul = 1;
        let mut clk: u16 = 0;
        let mut switch_base_clk = false;
        if self.version >= SDHCI_SPEC_300 {
            // Presets are never enabled (PRESET_VALUE_BROKEN, or not set up).
            div = 0;
            let mut rd = 0;
            if self.clk_mul != 0 {
                div = 1;
                while div <= 1024 {
                    if self.max_clk * self.clk_mul / div <= clock { break; }
                    div += 1;
                }
                if self.max_clk * self.clk_mul / div <= clock {
                    clk = SDHCI_PROG_CLOCK_MODE;
                    rd = div;
                    clk_mul = self.clk_mul;
                    div -= 1;
                } else {
                    switch_base_clk = true;
                }
            }
            if self.clk_mul == 0 || switch_base_clk {
                if self.max_clk <= clock {
                    div = 1;
                } else {
                    div = 2;
                    while div < SDHCI_MAX_DIV_SPEC_300 {
                        if self.max_clk / div <= clock { break; }
                        div += 2;
                    }
                }
                rd = div;
                div >>= 1;
            }
            real_div = rd;
        } else {
            div = 1;
            while div < SDHCI_MAX_DIV_SPEC_200 {
                if self.max_clk / div <= clock { break; }
                div *= 2;
            }
            real_div = div;
            div >>= 1;
        }
        if real_div != 0 {
            self.actual_clock = self.max_clk * clk_mul / real_div;
        }
        clk |= ((div & SDHCI_DIV_MASK) << SDHCI_DIVIDER_SHIFT) as u16;
        clk |= (((div & SDHCI_DIV_HI_MASK) >> SDHCI_DIV_MASK_LEN) << SDHCI_DIVIDER_HI_SHIFT) as u16;
        clk
    }

    fn enable_clk(&mut self, mut clk: u16) {
        clk |= SDHCI_CLOCK_INT_EN;
        self.writew(clk, SDHCI_CLOCK_CONTROL);
        let deadline = now_us() + 150_000;
        loop {
            let timedout = now_us() > deadline;
            clk = self.readw(SDHCI_CLOCK_CONTROL);
            if clk & SDHCI_CLOCK_INT_STABLE != 0 { break; }
            if timedout {
                kprintln!("[npk] sd: internal clock never stabilised");
                return;
            }
            udelay(10);
        }
        clk |= SDHCI_CLOCK_CARD_EN;
        self.writew(clk, SDHCI_CLOCK_CONTROL);
    }

    /// `host->ops->set_clock`
    fn set_clock(&mut self, clock: u32) {
        if self.is_o2 {
            self.o2_set_clock(clock);
            return;
        }
        self.actual_clock = 0;
        self.writew(0, SDHCI_CLOCK_CONTROL);
        if clock == 0 { return; }
        let clk = self.calc_clk(clock);
        self.enable_clk(clk);
    }

    // ── power ─────────────────────────────────────────────────────────

    fn get_vdd_value(vdd: u8) -> u8 {
        match 1u32.checked_shl(vdd as u32).unwrap_or(0) {
            MMC_VDD_165_195 | MMC_VDD_20_21 => SDHCI_POWER_180,
            MMC_VDD_29_30 | MMC_VDD_30_31 => SDHCI_POWER_300,
            MMC_VDD_32_33 | MMC_VDD_33_34 | MMC_VDD_34_35 | MMC_VDD_35_36 => SDHCI_POWER_330,
            _ => 0,
        }
    }

    fn set_power_noreg(&mut self, mode: u8, vdd: u8) {
        let mut pwr = 0;
        if mode != MMC_POWER_OFF {
            pwr = Self::get_vdd_value(vdd);
            if pwr == 0 {
                kprintln!("[npk] sd: invalid vdd {:#x}", vdd);
            }
        }
        if self.pwr == pwr { return; }
        self.pwr = pwr;
        if pwr == 0 {
            self.writeb(0, SDHCI_POWER_CONTROL);
        } else {
            self.writeb(0, SDHCI_POWER_CONTROL);
            pwr |= SDHCI_POWER_ON;
            self.writeb(pwr, SDHCI_POWER_CONTROL);
        }
    }

    /// `host->ops->set_power`
    fn set_power(&mut self, mode: u8, vdd: u8) {
        if self.is_o2 && mode == MMC_POWER_OFF {
            // sdhci_pci_o2_set_power: clock source back to the OPE clock.
            self.o2_unlock_wp();
            let v = self.cfg_read32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH) & !O2_SD_SEL_DLL;
            self.cfg_write32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH, v);
            self.o2_lock_wp();
        }
        self.set_power_noreg(mode, vdd);
    }

    // ── bus ───────────────────────────────────────────────────────────

    pub fn set_bus_width(&mut self, width: u8) {
        let mut ctrl = self.readb(SDHCI_HOST_CONTROL);
        if width == MMC_BUS_WIDTH_8 {
            ctrl &= !SDHCI_CTRL_4BITBUS;
            ctrl |= SDHCI_CTRL_8BITBUS;
        } else {
            if self.mmc_caps & MMC_CAP_8_BIT_DATA != 0 {
                ctrl &= !SDHCI_CTRL_8BITBUS;
            }
            if width == MMC_BUS_WIDTH_4 {
                ctrl |= SDHCI_CTRL_4BITBUS;
            } else {
                ctrl &= !SDHCI_CTRL_4BITBUS;
            }
        }
        self.writeb(ctrl, SDHCI_HOST_CONTROL);
    }

    fn set_uhs_signaling(&mut self, timing: u8) {
        let mut ctrl_2 = self.readw(SDHCI_HOST_CONTROL2);
        ctrl_2 &= !SDHCI_CTRL_UHS_MASK;
        ctrl_2 |= match timing {
            MMC_TIMING_UHS_SDR104 => SDHCI_CTRL_UHS_SDR104,
            MMC_TIMING_UHS_SDR12 => SDHCI_CTRL_UHS_SDR12,
            MMC_TIMING_UHS_SDR25 => SDHCI_CTRL_UHS_SDR25,
            MMC_TIMING_UHS_SDR50 => SDHCI_CTRL_UHS_SDR50,
            MMC_TIMING_UHS_DDR50 => SDHCI_CTRL_UHS_DDR50,
            _ => 0,
        };
        self.writew(ctrl_2, SDHCI_HOST_CONTROL2);
    }

    /// `sdhci_set_ios`
    pub fn set_ios(&mut self, ios: &Ios) {
        let reinit_uhs = self.reinit_uhs;
        self.reinit_uhs = false;
        if self.flags & SDHCI_DEVICE_DEAD != 0 { return; }
        let turning_on_clk = ios.clock != self.clock && ios.clock != 0 && self.clock == 0;

        // sdhci_set_ios_common
        if ios.power_mode == MMC_POWER_OFF {
            self.writel(0, SDHCI_SIGNAL_ENABLE);
            self.reinit();
        }

        if ios.clock == 0 || ios.clock != self.clock {
            self.set_clock(ios.clock);
            self.clock = ios.clock;
        }
        self.set_power(ios.power_mode, ios.vdd);
        self.set_bus_width(ios.bus_width);

        if !reinit_uhs && turning_on_clk && self.timing == ios.timing
            && self.version >= SDHCI_SPEC_300 && self.drv_type == ios.drv_type
        {
            return;
        }

        let mut ctrl = self.readb(SDHCI_HOST_CONTROL);
        if matches!(ios.timing, MMC_TIMING_SD_HS | MMC_TIMING_UHS_SDR50 | MMC_TIMING_UHS_SDR104
            | MMC_TIMING_UHS_DDR50 | MMC_TIMING_UHS_SDR25)
        {
            ctrl |= SDHCI_CTRL_HISPD;
        } else {
            ctrl &= !SDHCI_CTRL_HISPD;
        }

        if self.version >= SDHCI_SPEC_300 {
            let mut clk = self.readw(SDHCI_CLOCK_CONTROL);
            if clk & SDHCI_CLOCK_CARD_EN != 0 {
                clk &= !SDHCI_CLOCK_CARD_EN;
                self.writew(clk, SDHCI_CLOCK_CONTROL);
            }
            self.writeb(ctrl, SDHCI_HOST_CONTROL);
            let mut ctrl_2 = self.readw(SDHCI_HOST_CONTROL2);
            ctrl_2 &= !SDHCI_CTRL_DRV_TYPE_MASK;
            ctrl_2 |= match ios.drv_type {
                MMC_SET_DRIVER_TYPE_A => SDHCI_CTRL_DRV_TYPE_A,
                MMC_SET_DRIVER_TYPE_C => SDHCI_CTRL_DRV_TYPE_C,
                MMC_SET_DRIVER_TYPE_D => SDHCI_CTRL_DRV_TYPE_D,
                _ => SDHCI_CTRL_DRV_TYPE_B,
            };
            self.writew(ctrl_2, SDHCI_HOST_CONTROL2);
            self.drv_type = ios.drv_type;
            self.set_uhs_signaling(ios.timing);
            self.timing = ios.timing;
            let c = self.clock;
            self.set_clock(c);
        } else {
            self.writeb(ctrl, SDHCI_HOST_CONTROL);
        }
    }

    pub fn get_ro(&self) -> bool {
        if self.flags & SDHCI_DEVICE_DEAD != 0 { return false; }
        self.readl(SDHCI_PRESENT_STATE) & SDHCI_WRITE_PROTECT == 0
    }

    /// `mmc->ops->get_cd`
    pub fn get_cd(&mut self) -> bool {
        if self.flags & SDHCI_DEVICE_DEAD != 0 { return false; }
        if self.is_o2 {
            return self.o2_get_cd();
        }
        self.readl(SDHCI_PRESENT_STATE) & SDHCI_CARD_PRESENT != 0
    }

    pub fn card_busy(&self) -> bool {
        self.readl(SDHCI_PRESENT_STATE) & SDHCI_DATA_0_LVL_MASK == 0
    }

    /// `sdhci_start_signal_voltage_switch`
    pub fn start_signal_voltage_switch(&mut self, signal_voltage: u8) -> Result<(), Error> {
        if self.version < SDHCI_SPEC_300 { return Ok(()); }
        let mut ctrl = self.readw(SDHCI_HOST_CONTROL2);
        match signal_voltage {
            MMC_SIGNAL_VOLTAGE_330 => {
                if self.flags & SDHCI_SIGNALING_330 == 0 { return Err(Error::Invalid); }
                ctrl &= !SDHCI_CTRL_VDD_180;
                self.writew(ctrl, SDHCI_HOST_CONTROL2);
                mdelay(5);
                if self.readw(SDHCI_HOST_CONTROL2) & SDHCI_CTRL_VDD_180 == 0 { return Ok(()); }
                kprintln!("[npk] sd: 3.3V regulator output did not become stable");
                Err(Error::Again)
            }
            MMC_SIGNAL_VOLTAGE_180 => {
                if self.flags & SDHCI_SIGNALING_180 == 0 { return Err(Error::Invalid); }
                ctrl |= SDHCI_CTRL_VDD_180;
                self.writew(ctrl, SDHCI_HOST_CONTROL2);
                if self.readw(SDHCI_HOST_CONTROL2) & SDHCI_CTRL_VDD_180 != 0 { return Ok(()); }
                kprintln!("[npk] sd: 1.8V regulator output did not become stable");
                Err(Error::Again)
            }
            _ => Ok(()),
        }
    }

    // ── tuning ────────────────────────────────────────────────────────

    fn start_tuning(&mut self) {
        let mut ctrl = self.readw(SDHCI_HOST_CONTROL2);
        ctrl |= SDHCI_CTRL_EXEC_TUNING;
        self.writew(ctrl, SDHCI_HOST_CONTROL2);
        self.writel(SDHCI_INT_DATA_AVAIL, SDHCI_INT_ENABLE);
        self.writel(0, SDHCI_SIGNAL_ENABLE);
    }

    fn end_tuning(&mut self) {
        self.write_ier();
    }

    fn reset_tuning(&mut self) {
        let mut ctrl = self.readw(SDHCI_HOST_CONTROL2);
        ctrl &= !SDHCI_CTRL_TUNED_CLK;
        ctrl &= !SDHCI_CTRL_EXEC_TUNING;
        self.writew(ctrl, SDHCI_HOST_CONTROL2);
    }

    fn abort_tuning(&mut self, opcode: u8) {
        self.reset_tuning();
        self.reset_for_request_error();
        self.end_tuning();
        // mmc_send_abort_tuning: only HS200 eMMC needs CMD12 here.
        let _ = opcode;
    }

    /// Wait until the command (and data) lines are free; `sdhci_send_command_retry`.
    fn wait_inhibit(&mut self, mask: u32) -> Result<(), Error> {
        let mut timeout = 10;
        while self.readl(SDHCI_PRESENT_STATE) & mask != 0 {
            if timeout == 0 {
                kprintln!("[npk] sd: controller never released inhibit bit(s)");
                return Err(Error::Io);
            }
            timeout -= 1;
            mdelay(1);
            if !self.get_cd_quick() {
                return Err(Error::NoMedium);
            }
        }
        Ok(())
    }

    fn get_cd_quick(&self) -> bool {
        self.flags & SDHCI_DEVICE_DEAD == 0 && self.readl(SDHCI_PRESENT_STATE) & SDHCI_CARD_PRESENT != 0
    }

    pub fn send_tuning(&mut self, opcode: u8) {
        let b = SDHCI_DEFAULT_BOUNDARY_ARG;
        self.writew(sdhci_make_blksz(b, 64), SDHCI_BLOCK_SIZE);
        self.writew(SDHCI_TRNS_READ, SDHCI_TRANSFER_MODE);
        if self.wait_inhibit(SDHCI_CMD_INHIBIT | SDHCI_DATA_INHIBIT).is_err() {
            self.tuning_done = false;
            return;
        }
        self.writel(0, SDHCI_ARGUMENT);
        let flags = SDHCI_CMD_RESP_SHORT | SDHCI_CMD_CRC | SDHCI_CMD_INDEX | SDHCI_CMD_DATA;
        self.writew(((opcode as u16) << 8) | flags, SDHCI_COMMAND);
        self.tuning_done = false;
        // Buffer Read Ready, at most 50 ms.
        let deadline = now_us() + 50_000;
        while now_us() < deadline {
            let st = self.readl(SDHCI_INT_STATUS);
            if st & SDHCI_INT_DATA_AVAIL != 0 {
                self.writel(st & (SDHCI_INT_CMD_MASK | SDHCI_INT_DATA_MASK), SDHCI_INT_STATUS);
                self.tuning_done = true;
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn __execute_tuning(&mut self, opcode: u8) -> Result<(), Error> {
        for _ in 0..MAX_TUNING_LOOP {
            self.send_tuning(opcode);
            if !self.tuning_done {
                self.abort_tuning(opcode);
                return Err(Error::Timeout);
            }
            // tuning_delay: 1 ms for CMD19.
            if opcode == MMC_SEND_TUNING_BLOCK { udelay(1000); }
            let ctrl = self.readw(SDHCI_HOST_CONTROL2);
            if ctrl & SDHCI_CTRL_EXEC_TUNING == 0 {
                if ctrl & SDHCI_CTRL_TUNED_CLK != 0 { return Ok(()); }
                break;
            }
        }
        kprintln!("[npk] sd: tuning failed, falling back to fixed sampling clock");
        self.reset_tuning();
        Err(Error::Again)
    }

    fn sdhci_execute_tuning(&mut self, opcode: u8) -> Result<(), Error> {
        match self.timing {
            MMC_TIMING_UHS_SDR104 | MMC_TIMING_UHS_DDR50 => {}
            MMC_TIMING_UHS_SDR50 if self.flags & SDHCI_SDR50_NEEDS_TUNING != 0 => {}
            _ => return Ok(()),
        }
        self.start_tuning();
        let r = self.__execute_tuning(opcode);
        self.end_tuning();
        // host->tuning_err is reported, not returned (Linux returns 0).
        if r.is_err() {
            kprintln!("[npk] sd: tuning error {:?}", r);
        }
        Ok(())
    }

    /// `mmc->ops->execute_tuning`
    pub fn execute_tuning(&mut self, opcode: u8) -> Result<(), Error> {
        if self.is_o2 {
            self.o2_execute_tuning(opcode)
        } else {
            self.sdhci_execute_tuning(opcode)
        }
    }

    // ── requests ──────────────────────────────────────────────────────

    fn target_timeout_us(&self, cmd: &Command, data: Option<&Data>) -> u32 {
        match data {
            None => cmd.busy_timeout.saturating_mul(1000),
            Some(d) => {
                let mut t = d.timeout_ns.div_ceil(1000);
                if self.clock != 0 && d.timeout_clks != 0 {
                    let val = 1_000_000u64 * d.timeout_clks as u64;
                    t = t.saturating_add(val.div_ceil(self.clock as u64) as u32);
                }
                t
            }
        }
    }

    fn calc_timeout(&self, cmd: &Command, data: Option<&Data>) -> u8 {
        if data.is_none() && cmd.busy_timeout == 0 {
            return self.max_timeout_count;
        }
        let target = self.target_timeout_us(cmd, data);
        let mut count = 0u8;
        let mut current: u32 = (1u32 << 13) * 1000 / self.timeout_clk.max(1);
        while current < target {
            count += 1;
            current = current.saturating_mul(2);
            if count > self.max_timeout_count {
                count = self.max_timeout_count;
                break;
            }
        }
        count
    }

    fn set_transfer_irqs(&mut self) {
        let pio_irqs = SDHCI_INT_DATA_AVAIL | SDHCI_INT_SPACE_AVAIL;
        let dma_irqs = SDHCI_INT_DMA_END | SDHCI_INT_ADMA_ERROR;
        if self.flags & SDHCI_REQ_USE_DMA != 0 {
            self.ier = (self.ier & !pio_irqs) | dma_irqs;
        } else {
            self.ier = (self.ier & !dma_irqs) | pio_irqs;
        }
        if self.flags & (SDHCI_AUTO_CMD23 | SDHCI_AUTO_CMD12) != 0 {
            self.ier |= SDHCI_INT_AUTO_CMD_ERR;
        } else {
            self.ier &= !SDHCI_INT_AUTO_CMD_ERR;
        }
        self.write_ier();
    }

    /// `sdhci_adma_table_pre` for the one contiguous buffer.
    fn adma_table_pre(&self, len: usize) {
        let mut off = 0usize;
        let mut addr = self.buf.phys();
        let mut left = len;
        let mut last = None;
        while left > 0 {
            let n = left.min(MAX_ADMA);
            // A length of 65536 is written as 0, as Linux's u16 store does.
            self.adma.w16(off, ADMA2_TRAN_VALID);
            self.adma.w16(off + 2, n as u16);
            self.adma.w32(off + 4, addr as u32);
            last = Some(off);
            off += SDHCI_ADMA2_32_DESC_SZ;
            addr += n as u64;
            left -= n;
        }
        if self.quirks & SDHCI_QUIRK_NO_ENDATTR_IN_NOPDESC != 0 {
            if let Some(l) = last {
                let c = self.adma.r16(l);
                self.adma.w16(l, c | ADMA2_END);
            }
        } else {
            self.adma.w16(off, ADMA2_NOP_END_VALID);
            self.adma.w16(off + 2, 0);
            self.adma.w32(off + 4, 0);
        }
    }

    fn prepare_data(&mut self, data: &Data) {
        self.flags |= SDHCI_REQ_USE_DMA;
        self.config_dma();
        self.adma_table_pre((data.blksz * data.blocks) as usize);
        self.writel(self.adma.phys() as u32, SDHCI_ADMA_ADDRESS);
        self.set_transfer_irqs();
        self.writew(sdhci_make_blksz(SDHCI_DEFAULT_BOUNDARY_ARG, data.blksz as u16), SDHCI_BLOCK_SIZE);
        self.writew(data.blocks as u16, SDHCI_BLOCK_COUNT);
    }

    fn set_transfer_mode(&mut self, cmd: &Command, data: Option<&Data>, stop: Stop) {
        let Some(data) = data else {
            if self.quirks2 & SDHCI_QUIRK2_CLEAR_TRANSFERMODE_REG_BEFORE_CMD != 0 {
                if cmd.opcode != MMC_SEND_TUNING_BLOCK {
                    self.writew(0, SDHCI_TRANSFER_MODE);
                }
            } else {
                let mode = self.readw(SDHCI_TRANSFER_MODE);
                self.writew(mode & !(SDHCI_TRNS_AUTO_CMD12 | SDHCI_TRNS_AUTO_CMD23), SDHCI_TRANSFER_MODE);
            }
            return;
        };
        let mut mode = SDHCI_TRNS_BLK_CNT_EN;
        if matches!(cmd.opcode, 18 | 25) || data.blocks > 1 {
            mode = SDHCI_TRNS_BLK_CNT_EN | SDHCI_TRNS_MULTI;
            match stop {
                Stop::AutoCmd12 => mode |= SDHCI_TRNS_AUTO_CMD12,
                Stop::AutoCmd23 => {
                    mode |= SDHCI_TRNS_AUTO_CMD23;
                    self.writel(data.blocks, SDHCI_ARGUMENT2);
                }
                _ => {}
            }
        }
        if !data.write { mode |= SDHCI_TRNS_READ; }
        if self.flags & SDHCI_REQ_USE_DMA != 0 { mode |= SDHCI_TRNS_DMA; }
        self.writew(mode, SDHCI_TRANSFER_MODE);
    }

    fn read_rsp_136(&self, cmd: &mut Command) {
        for i in 0..4 {
            cmd.resp[i] = self.readl(SDHCI_RESPONSE + (3 - i as u32) * 4);
        }
        for i in 0..4 {
            cmd.resp[i] <<= 8;
            if i != 3 {
                cmd.resp[i] |= cmd.resp[i + 1] >> 24;
            }
        }
    }

    /// Send one command and wait for it; `sdhci_send_command` and the
    /// `sdhci_cmd_irq`/`sdhci_data_irq` paths, polled. An error says
    /// whether it was the data phase's.
    fn run(&mut self, cmd: &mut Command, data: Option<&Data>, stop: Stop, is_stop: bool) -> Result<(), (Error, bool)> {
        let data_line = data.is_some() || cmd.flags & MMC_RSP_BUSY != 0;
        let mut mask = SDHCI_CMD_INHIBIT;
        if data_line && !is_stop { mask |= SDHCI_DATA_INHIBIT; }
        self.wait_inhibit(mask).map_err(|e| (e, false))?;

        if data_line {
            let count = self.calc_timeout(cmd, data);
            self.writeb(count, SDHCI_TIMEOUT_CONTROL);
        }
        if let Some(d) = data {
            self.prepare_data(d);
        }
        self.writel(cmd.arg, SDHCI_ARGUMENT);
        self.set_transfer_mode(cmd, data, stop);

        let mut flags = if cmd.flags & MMC_RSP_PRESENT == 0 {
            SDHCI_CMD_RESP_NONE
        } else if cmd.flags & MMC_RSP_136 != 0 {
            SDHCI_CMD_RESP_LONG
        } else if cmd.flags & MMC_RSP_BUSY != 0 {
            SDHCI_CMD_RESP_SHORT_BUSY
        } else {
            SDHCI_CMD_RESP_SHORT
        };
        if cmd.flags & MMC_RSP_CRC != 0 { flags |= SDHCI_CMD_CRC; }
        if cmd.flags & MMC_RSP_OPCODE != 0 { flags |= SDHCI_CMD_INDEX; }
        if data.is_some() || cmd.opcode == MMC_SEND_TUNING_BLOCK { flags |= SDHCI_CMD_DATA; }

        let timeout_ms: u64 = if data.is_none() && cmd.busy_timeout > 9000 {
            cmd.busy_timeout.div_ceil(1000) as u64 * 1000 + 1000
        } else {
            10_000
        };
        let deadline = now_us() + timeout_ms * 1000;
        self.writew(((cmd.opcode as u16) << 8) | flags, SDHCI_COMMAND);

        let mut cmd_done = false;
        let mut cmd_err: Option<Error> = None;
        let mut data_err: Option<Error> = None;
        // Busy end (R1B without data) arrives as DATA_END.
        let wait_busy = data.is_none() && cmd.flags & MMC_RSP_BUSY != 0;
        let mut data_done = data.is_none() && !wait_busy;
        loop {
            let mut intmask = self.readl(SDHCI_INT_STATUS);
            if intmask == 0xFFFF_FFFF {
                return Err((Error::NoMedium, false));
            }
            let clear = intmask & (SDHCI_INT_CMD_MASK | SDHCI_INT_DATA_MASK | SDHCI_INT_BUS_POWER);
            if clear != 0 {
                self.writel(clear, SDHCI_INT_STATUS);
            }
            // sdhci_cmd_irq
            if intmask & SDHCI_INT_AUTO_CMD_ERR != 0 && data.is_some() {
                let st = self.readw(SDHCI_AUTO_CMD_STATUS);
                if stop == Stop::AutoCmd12 {
                    intmask |= if st & SDHCI_AUTO_CMD_TIMEOUT != 0 { SDHCI_INT_DATA_TIMEOUT } else { SDHCI_INT_DATA_CRC };
                } else if stop == Stop::AutoCmd23 {
                    cmd_err = Some(if st & SDHCI_AUTO_CMD_TIMEOUT != 0 { Error::Timeout } else { Error::Ilseq });
                    break;
                }
            }
            if !cmd_done && intmask & SDHCI_INT_CMD_MASK != 0 {
                if intmask & (SDHCI_INT_TIMEOUT | SDHCI_INT_CRC | SDHCI_INT_END_BIT | SDHCI_INT_INDEX) != 0 {
                    let e = if intmask & SDHCI_INT_TIMEOUT != 0 { Error::Timeout } else { Error::Ilseq };
                    // A data command's CRC error counts as a data CRC error.
                    if data.is_some() && intmask & (SDHCI_INT_CRC | SDHCI_INT_TIMEOUT) == SDHCI_INT_CRC {
                        cmd_done = true;
                        intmask |= SDHCI_INT_DATA_CRC;
                    } else {
                        cmd_err = Some(e);
                        break;
                    }
                } else if intmask & SDHCI_INT_RESPONSE != 0 {
                    if cmd.flags & MMC_RSP_PRESENT != 0 {
                        if cmd.flags & MMC_RSP_136 != 0 {
                            self.read_rsp_136(cmd);
                        } else {
                            cmd.resp[0] = self.readl(SDHCI_RESPONSE);
                        }
                    }
                    cmd_done = true;
                }
            }
            // sdhci_data_irq
            if intmask & SDHCI_INT_DATA_MASK != 0 {
                if wait_busy {
                    if intmask & SDHCI_INT_DATA_TIMEOUT != 0 {
                        cmd_err = Some(Error::Timeout);
                        break;
                    }
                    if intmask & SDHCI_INT_DATA_END != 0 {
                        data_done = true;
                    }
                } else if data.is_some() {
                    if intmask & SDHCI_INT_DATA_TIMEOUT != 0 {
                        data_err = Some(Error::Timeout);
                    } else if intmask & SDHCI_INT_DATA_END_BIT != 0 {
                        data_err = Some(Error::Ilseq);
                    } else if intmask & (SDHCI_INT_DATA_CRC | SDHCI_INT_TUNING_ERROR) != 0 {
                        data_err = Some(Error::Ilseq);
                        if intmask & SDHCI_INT_TUNING_ERROR != 0 {
                            let c2 = self.readw(SDHCI_HOST_CONTROL2) & !SDHCI_CTRL_TUNED_CLK;
                            self.writew(c2, SDHCI_HOST_CONTROL2);
                        }
                    } else if intmask & SDHCI_INT_ADMA_ERROR != 0 {
                        kprintln!("[npk] sd: ADMA error {:#010x}", intmask);
                        data_err = Some(Error::Io);
                    }
                    if data_err.is_some() { break; }
                    if intmask & SDHCI_INT_DATA_END != 0 {
                        data_done = true;
                    }
                }
            }
            if intmask & SDHCI_INT_BUS_POWER != 0 {
                kprintln!("[npk] sd: card is consuming too much power");
            }
            if cmd_done && data_done { break; }
            if now_us() > deadline {
                kprintln!("[npk] sd: CMD{} timed out (status {:#010x})", cmd.opcode, intmask);
                cmd_err = Some(Error::Timeout);
                break;
            }
            if !self.get_cd_quick() {
                cmd_err = Some(Error::NoMedium);
                break;
            }
            crate::smp::fiber::spin_or_yield();
        }
        self.flags &= !SDHCI_REQ_USE_DMA;
        if let Some(e) = cmd_err {
            return Err((e, false));
        }
        if let Some(e) = data_err {
            return Err((e, true));
        }
        Ok(())
    }

    /// One `mmc_request`: optional CMD23, the command with its data, and
    /// a CMD12 the driver sends itself. Any error resets CMD and DATA.
    pub fn request(&mut self, cmd: &mut Command, data: Option<&Data>, stop: Stop) -> Result<(), Error> {
        if stop == Stop::ManualCmd23 {
            if let Some(d) = data {
                let mut sbc = Command::new(23, d.blocks, MMC_RSP_R1);
                if let Err((e, _)) = self.run(&mut sbc, None, Stop::None, false) {
                    self.reset_for_request_error();
                    return Err(e);
                }
            }
        }
        let r = self.run(cmd, data, stop, false);
        let data_error = matches!(r, Err((_, true)));
        let r = r.map_err(|(e, _)| e);
        // __sdhci_finish_data: CMD12 for an open-ended transfer, and after
        // a data error of a multi-block one.
        let needs_stop = data.is_some() && (stop == Stop::ManualCmd12 && r.is_ok()
            || data_error && stop != Stop::None);
        let mut stop_r = Ok(());
        if needs_stop {
            if data_error { self.reset_for_request_error(); }
            let write = data.is_some_and(|d| d.write);
            let mut stop_cmd = Command::new(MMC_STOP_TRANSMISSION, 0,
                if write { MMC_RSP_R1B } else { MMC_RSP_R1 });
            stop_cmd.busy_timeout = 3000;
            stop_r = self.run(&mut stop_cmd, None, Stop::None, true).map_err(|(e, _)| e);
        }
        if r.is_err() || stop_r.is_err() {
            self.reset_for_request_error();
        }
        r.and(stop_r)
    }

    // ── sdhci-pci-o2micro.c ───────────────────────────────────────────

    fn o2_unlock_wp(&self) {
        let s = self.cfg_read8(O2_SD_LOCK_WP as u16) & 0x7f;
        self.cfg_write8(O2_SD_LOCK_WP as u16, s);
    }

    fn o2_lock_wp(&self) {
        let s = self.cfg_read8(O2_SD_LOCK_WP as u16) | 0x80;
        self.cfg_write8(O2_SD_LOCK_WP as u16, s);
    }

    fn o2_wait_card_detect_stable(&self) {
        let deadline = now_us() + 50_000;
        loop {
            let timedout = now_us() > deadline;
            let s = self.readl(SDHCI_PRESENT_STATE);
            if (s & SDHCI_CARD_PRESENT) >> SDHCI_CARD_PRES_SHIFT == (s & SDHCI_CD_LVL) >> SDHCI_CD_LVL_SHIFT {
                break;
            }
            if timedout {
                kprintln!("[npk] sd: card detect debounce never finished");
                return;
            }
            udelay(10);
        }
    }

    fn o2_enable_internal_clock(&self) {
        let mut s = self.readl(O2_PLL_DLL_WDT_CONTROL1);
        s |= O2_PLL_SOFT_RESET;
        self.writel(s, O2_PLL_DLL_WDT_CONTROL1);
        udelay(1);
        s &= !O2_PLL_SOFT_RESET;
        self.writel(s, O2_PLL_DLL_WDT_CONTROL1);
        s |= O2_PLL_FORCE_ACTIVE;
        self.writel(s, O2_PLL_DLL_WDT_CONTROL1);
        let deadline = now_us() + 20_000;
        let mut locked = true;
        loop {
            let timedout = now_us() > deadline;
            if self.readw(O2_PLL_DLL_WDT_CONTROL1) as u32 & O2_PLL_LOCK_STATUS != 0 { break; }
            if timedout {
                kprintln!("[npk] sd: internal clock never stabilised");
                locked = false;
                break;
            }
            udelay(10);
        }
        if locked {
            udelay(1);
            self.o2_wait_card_detect_stable();
        }
        let s = self.readl(O2_PLL_DLL_WDT_CONTROL1) & !O2_PLL_FORCE_ACTIVE;
        self.writel(s, O2_PLL_DLL_WDT_CONTROL1);
    }

    fn o2_get_cd(&self) -> bool {
        if self.readw(O2_PLL_DLL_WDT_CONTROL1) as u32 & O2_PLL_LOCK_STATUS == 0 {
            self.o2_enable_internal_clock();
        } else {
            self.o2_wait_card_detect_stable();
        }
        self.readl(SDHCI_PRESENT_STATE) & SDHCI_CARD_PRESENT != 0
    }

    fn o2_set_baseclk(&self, value: u32) {
        let s = (self.cfg_read32(O2_SD_PLL_SETTING) & 0x0000_FFFF) | value;
        self.cfg_write32(O2_SD_PLL_SETTING, s);
    }

    /// `readx_poll_timeout(sdhci_o2_pll_dll_wdt_control, ..., cond, 1, timeout_us)`
    fn o2_poll_dll(&self, want_locked: bool, timeout_us: u64) -> bool {
        let deadline = now_us() + timeout_us;
        loop {
            let v = self.readl(O2_PLL_DLL_WDT_CONTROL1) & O2_DLL_LOCK_STATUS != 0;
            if v == want_locked { return true; }
            if now_us() > deadline {
                return (self.readl(O2_PLL_DLL_WDT_CONTROL1) & O2_DLL_LOCK_STATUS != 0) == want_locked;
            }
            udelay(1);
        }
    }

    /// `sdhci_o2_wait_dll_detect_lock`: true when the DLL stayed unlocked
    /// for none of the polls, i.e. the poll for "unlocked" timed out.
    fn o2_wait_dll_detect_lock(&self) -> bool {
        !self.o2_poll_dll(false, 1_000_000)
    }

    fn o2_set_tuning_mode(&self) {
        let reg = self.readw(O2_SD_VENDOR_SETTING) & !O2_SD_HW_TUNING_DISABLE;
        self.writew(reg, O2_SD_VENDOR_SETTING);
    }

    fn __o2_execute_tuning(&mut self, opcode: u8) {
        self.send_tuning(opcode);
        for _ in 0..150 {
            let ctrl = self.readw(SDHCI_HOST_CONTROL2);
            if ctrl & SDHCI_CTRL_EXEC_TUNING == 0 {
                if ctrl & SDHCI_CTRL_TUNED_CLK != 0 {
                    self.tuning_done = true;
                    return;
                }
                kprintln!("[npk] sd: HW tuning failed");
                break;
            }
            udelay(1000);
        }
        kprintln!("[npk] sd: tuning failed, falling back to fixed sampling clock");
        self.reset_tuning();
    }

    fn o2_dll_recovery(&mut self) -> bool {
        let mut ret = false;
        self.o2_unlock_wp();
        while (self.dll_adjust_count as usize) < DMDN_TABLE.len() && !ret {
            self.writeb(0, SDHCI_CLOCK_CONTROL);
            let s = self.readl(O2_PLL_DLL_WDT_CONTROL1) | O2_PLL_SOFT_RESET;
            self.writel(s, O2_PLL_DLL_WDT_CONTROL1);
            let f4 = self.cfg_read32(O2_SD_FUNC_REG4) | O2_SD_FREG4_ENABLE_CLK_SET;
            self.cfg_write32(O2_SD_FUNC_REG4, f4);
            self.o2_set_baseclk(DMDN_TABLE[self.dll_adjust_count as usize]);
            let mut clk8 = SDHCI_CLOCK_INT_EN as u8;
            self.writeb(clk8, SDHCI_CLOCK_CONTROL);
            if self.o2_get_cd() {
                mdelay(5);
                if self.o2_wait_dll_detect_lock() {
                    clk8 |= SDHCI_CLOCK_CARD_EN as u8;
                    self.writeb(clk8, SDHCI_CLOCK_CONTROL);
                    ret = true;
                } else {
                    kprintln!("[npk] sd: DLL unlocked when dll_adjust_count is {}", self.dll_adjust_count);
                }
            } else {
                kprintln!("[npk] sd: card present detect failed");
                break;
            }
            self.dll_adjust_count += 1;
        }
        if !ret && self.dll_adjust_count as usize == DMDN_TABLE.len() {
            kprintln!("[npk] sd: DLL adjust over max times");
        }
        self.o2_lock_wp();
        ret
    }

    fn o2_execute_tuning(&mut self, opcode: u8) -> Result<(), Error> {
        if self.timing != MMC_TIMING_UHS_SDR104 && self.timing != MMC_TIMING_UHS_SDR50 {
            return self.sdhci_execute_tuning(opcode);
        }
        let s = self.readw(O2_SD_MISC_CTRL) | O2_SD_PWR_FORCE_L0;
        self.writew(s, O2_SD_MISC_CTRL);

        // SeaBird: update the output phase.
        let mut reg = self.readw(SDHCI_CLOCK_CONTROL) & !SDHCI_CLOCK_CARD_EN;
        self.writew(reg, SDHCI_CLOCK_CONTROL);
        if self.timing == MMC_TIMING_UHS_SDR104 {
            self.o2_unlock_wp();
            let mut v = self.cfg_read32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH);
            v &= !(O2_SD_SEL_DLL | O2_SD_PHASE_MASK);
            v |= O2_SD_SEL_DLL | O2_SD_FIX_PHASE;
            self.cfg_write32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH, v);
            self.o2_lock_wp();
        }
        reg = self.readw(SDHCI_CLOCK_CONTROL) | SDHCI_CLOCK_CARD_EN;
        self.writew(reg, SDHCI_CLOCK_CONTROL);

        if !self.o2_poll_dll(true, 5000) {
            kprintln!("[npk] sd: DLL can't lock in 5ms after force L0 during tuning");
        }
        if !self.o2_wait_dll_detect_lock() && !self.o2_dll_recovery() {
            kprintln!("[npk] sd: o2 dll recovery failed");
            return Err(Error::Invalid);
        }
        self.o2_set_tuning_mode();
        self.start_tuning();
        self.__o2_execute_tuning(opcode);
        self.end_tuning();

        let s = self.readw(O2_SD_MISC_CTRL) & !O2_SD_PWR_FORCE_L0;
        self.writew(s, O2_SD_MISC_CTRL);
        self.reset(SDHCI_RESET_CMD);
        self.reset(SDHCI_RESET_DATA);
        Ok(())
    }

    fn o2_enable_clk(&self, mut clk: u16) {
        clk |= SDHCI_CLOCK_INT_EN;
        self.writew(clk, SDHCI_CLOCK_CONTROL);
        self.o2_enable_internal_clock();
        if self.o2_get_cd() {
            clk |= SDHCI_CLOCK_CARD_EN;
            self.writew(clk, SDHCI_CLOCK_CONTROL);
        }
    }

    fn o2_set_clock(&mut self, clock: u32) {
        self.actual_clock = 0;
        self.writew(0, SDHCI_CLOCK_CONTROL);
        if clock == 0 { return; }
        self.o2_unlock_wp();
        let dmdn_208m = 0x2c28_0000;
        let dmdn_200m = 0x2510_0000;
        let pll = self.cfg_read32(O2_SD_PLL_SETTING);
        if self.timing == MMC_TIMING_UHS_SDR104 && clock == 200_000_000 {
            if pll & 0xFFFF_0000 != dmdn_208m { self.o2_set_baseclk(dmdn_208m); }
        } else if pll & 0xFFFF_0000 != dmdn_200m {
            self.o2_set_baseclk(dmdn_200m);
        }
        let mut v = self.cfg_read32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH);
        v &= !(O2_SD_SEL_DLL | O2_SD_PHASE_MASK);
        self.cfg_write32(O2_SD_OUTPUT_CLK_SOURCE_SWITCH, v);
        self.o2_lock_wp();
        let clk = self.calc_clk(clock);
        self.o2_enable_clk(clk);
    }

    /// `sdhci_pci_o2_probe` for SeaBird.
    fn o2_probe(&self) {
        self.o2_unlock_wp();
        let mut s = self.cfg_read32(O2_SD_PLL_SETTING);
        if s & 0xff00_0000 == 0x0100_0000 {
            s &= 0x0000_FFFF;
            s |= 0x1F34_0000;
            self.cfg_write32(O2_SD_PLL_SETTING, s);
        } else {
            s &= 0x0000_FFFF;
            s |= 0x2510_0000;
            self.cfg_write32(O2_SD_PLL_SETTING, s);
            let f4 = self.cfg_read32(O2_SD_FUNC_REG4) | (1 << 22);
            self.cfg_write32(O2_SD_FUNC_REG4, f4);
        }
        // Tuning window 5.
        self.cfg_write8(O2_SD_TUNING_CTRL, 0x55);
        // 1st and 2nd card detect debounce time.
        let mut m = self.cfg_read32(O2_SD_MISC_CTRL2 as u16);
        m &= 0xFFE7_FFFF;
        m |= 0x0018_0000;
        self.cfg_write32(O2_SD_MISC_CTRL2 as u16, m);
        self.cfg_write32(O2_SD_DETECT_SETTING, 1);
        self.o2_lock_wp();
    }

    /// `sdhci_pci_o2_probe_slot` for SeaBird.
    fn o2_probe_slot(&mut self) {
        self.dll_adjust_count = 0;
        let caps = self.readl(SDHCI_CAPABILITIES);
        if caps & SDHCI_CAN_DO_8BIT != 0 {
            self.mmc_caps |= MMC_CAP_8_BIT_DATA;
        }
        self.quirks2 |= SDHCI_QUIRK2_BROKEN_DDR50;
        let reg = self.readl(O2_SD_VENDOR_SETTING);
        if reg & 0x1 != 0 {
            self.quirks |= SDHCI_QUIRK_MULTIBLOCK_READ_ACMD12;
        }
        if self.device == PCI_DEVICE_ID_O2_SEABIRD1 {
            self.quirks2 |= SDHCI_QUIRK2_PRESET_VALUE_BROKEN;
        }
    }
}
