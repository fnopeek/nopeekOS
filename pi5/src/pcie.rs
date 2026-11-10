//! The BCM2712 PCIe root complex, after Linux
//! `drivers/pci/controller/pcie-brcmstb.c` with `bcm2712_cfg`: probe,
//! `brcm_pcie_setup` with `brcm_pcie_post_setup_bcm2712`,
//! `brcm_pcie_start_link` and the config-space window (`brcm_pcie_map_bus`).
//!
//! Only the parts `bcm2712_cfg` reaches: no PHY (`has_phy` false), no SSC
//! (`CFG_QUIRK_NO_SSC`), no internal MSI controller (the DT routes MSI to
//! the MIP), no suspend, no LTSSM trace.

use crate::hw::{field_get, poll, replace_bits, Block, Hw};
use crate::reset;

const BRCM_PCIE_CAP_REGS: u64 = 0x00ac;

const PCI_EXP_LNKCAP_SLS: u32 = 0x0000_000f;
const PCI_EXP_LNKCAP_ASPM_L0S: u32 = 0x0000_0400;
const PCI_EXP_LNKSTA: u64 = 0x12;
const PCI_EXP_LNKSTA_CLS: u32 = 0x000f;
const PCI_EXP_LNKSTA_NLW: u32 = 0x03f0;
const PCI_EXP_RTCTL: u64 = 0x1c;
const PCI_EXP_RTCTL_CRSSVE: u16 = 0x0010;
const PCI_EXP_RTCAP: u64 = 0x1e;
const PCI_EXP_RTCAP_CRSVIS: u16 = 0x0001;
const PCI_EXP_LNKCTL2: u64 = 0x30;
const PCI_EXP_LNKCTL2_TLS: u32 = 0x000f;

const PCIE_RC_CFG_VENDOR_VENDOR_SPECIFIC_REG1: u64 = 0x0188;
const PCIE_RC_CFG_VENDOR_VENDOR_SPECIFIC_REG1_ENDIAN_MODE_BAR2_MASK: u32 = 0xc;
const PCIE_RC_CFG_VENDOR_SPECIFIC_REG1_LITTLE_ENDIAN: u32 = 0x0;

const PCIE_RC_CFG_PRIV1_ID_VAL3: u64 = 0x043c;
const PCIE_RC_CFG_PRIV1_ID_VAL3_CLASS_CODE_MASK: u32 = 0xff_ffff;

const PCIE_RC_CFG_PRIV1_LINK_CAPABILITY: u64 = 0x04dc;
const PCIE_RC_CFG_PRIV1_LINK_CAPABILITY_MAX_LINK_WIDTH_MASK: u32 = 0x1f0;

const PCIE_RC_CFG_PRIV1_ROOT_CAP: u64 = 0x4f8;
const PCIE_RC_CFG_PRIV1_ROOT_CAP_L1SS_MODE_MASK: u32 = 0xf8;

const PCIE_RC_DL_MDIO_ADDR: u64 = 0x1100;
const PCIE_RC_DL_MDIO_WR_DATA: u64 = 0x1104;

const PCIE_RC_PL_REG_PHY_CTL_1: u64 = 0x1804;
const PCIE_RC_PL_REG_PHY_CTL_1_REG_P2_POWERDOWN_ENA_NOSYNC_MASK: u32 = 0x8;

const PCIE_RC_PL_PHY_CTL_15: u64 = 0x184c;
const PCIE_RC_PL_PHY_CTL_15_PM_CLK_PERIOD_MASK: u32 = 0xff;

const PCIE_RC_PL_STATS_CTRL: u64 = 0x1940;
const PCIE_RC_PL_STATS_CTRL_EN_MASK: u32 = 0x1;
const PCIE_RC_PL_STATS_CTRL_LEN_MASK: u32 = 0xffff_fff0;
const PCIE_RC_PL_STATS_TXTLP_LO: u64 = 0x1944;
const PCIE_RC_PL_STATS_TXTLP_HI: u64 = 0x1948;
const PCIE_RC_PL_STATS_TXDLLP_LO: u64 = 0x194c;
const PCIE_RC_PL_STATS_TXDLLP_HI: u64 = 0x1950;
const PCIE_RC_PL_STATS_RXTLP_LO: u64 = 0x195c;
const PCIE_RC_PL_STATS_RXTLP_HI: u64 = 0x1960;
const PCIE_RC_PL_STATS_RXDLLP_LO: u64 = 0x1964;
const PCIE_RC_PL_STATS_RXDLLP_HI: u64 = 0x1968;
const PCIE_RC_PL_STATS_RXPL_ERR: u64 = 0x1974;
const PCIE_RC_PL_STATS_RXDL_ERR: u64 = 0x1978;
const PCIE_RC_PL_STATS_RXTL_ERR: u64 = 0x197c;
const PCIE_RC_PL_LTSSM_STATS_3: u64 = 0x19b0;
const PCIE_RC_PL_LTSSM_STATS_CNT: u64 = 0x19b4;

const PCIE_MISC_MISC_CTRL: u64 = 0x4008;
const PCIE_MISC_MISC_CTRL_PCIE_RCB_64B_MODE_MASK: u32 = 0x80;
const PCIE_MISC_MISC_CTRL_PCIE_RCB_MPS_MODE_MASK: u32 = 0x400;
const PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN_MASK: u32 = 0x1000;
const PCIE_MISC_MISC_CTRL_CFG_READ_UR_MODE_MASK: u32 = 0x2000;
const PCIE_MISC_MISC_CTRL_MAX_BURST_SIZE_MASK: u32 = 0x30_0000;

const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO: u64 = 0x400c;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI: u64 = 0x4010;

const PCIE_MISC_RC_BAR1_CONFIG_LO: u64 = 0x402c;
const PCIE_MISC_RC_BAR1_CONFIG_LO_SIZE_MASK: u32 = 0x1f;
const PCIE_MISC_RC_BAR4_CONFIG_LO: u64 = 0x40d4;

const PCIE_MISC_PCIE_CTRL: u64 = 0x4064;
const PCIE_MISC_PCIE_CTRL_PCIE_PERSTB_MASK: u32 = 0x4;

const PCIE_MISC_PCIE_STATUS: u64 = 0x4068;
const PCIE_MISC_PCIE_STATUS_PCIE_PORT_MASK: u32 = 0x80;
const PCIE_MISC_PCIE_STATUS_PCIE_DL_ACTIVE_MASK: u32 = 0x20;
const PCIE_MISC_PCIE_STATUS_PCIE_PHYLINKUP_MASK: u32 = 0x10;

const PCIE_MISC_REVISION: u64 = 0x406c;

const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT: u64 = 0x4070;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT_LIMIT_MASK: u32 = 0xfff0_0000;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT_BASE_MASK: u32 = 0xfff0;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI: u64 = 0x4080;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI_BASE_MASK: u32 = 0xff;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI: u64 = 0x4084;
const PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI_LIMIT_MASK: u32 = 0xff;

const PCIE_MISC_HARD_PCIE_HARD_DEBUG_CLKREQ_DEBUG_ENABLE_MASK: u32 = 0x2;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG_PERST_ASSERT_MASK: u32 = 0x8;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_ENABLE_MASK: u32 = 0x1_0000;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_OUT_MASK: u32 = 0x10_0000;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG_L1SS_ENABLE_MASK: u32 = 0x20_0000;
const PCIE_MISC_HARD_PCIE_HARD_DEBUG_SERDES_IDDQ_MASK: u32 = 0x0800_0000;
const PCIE_CLKREQ_MASK: u32 = PCIE_MISC_HARD_PCIE_HARD_DEBUG_CLKREQ_DEBUG_ENABLE_MASK
    | PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_ENABLE_MASK
    | PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_OUT_MASK
    | PCIE_MISC_HARD_PCIE_HARD_DEBUG_L1SS_ENABLE_MASK;

const PCIE_MISC_UBUS_BAR1_CONFIG_REMAP: u64 = 0x40ac;
const PCIE_MISC_UBUS_BAR1_CONFIG_REMAP_ACCESS_EN_MASK: u32 = 1 << 0;
const PCIE_MISC_UBUS_BAR4_CONFIG_REMAP: u64 = 0x410c;

const PCIE_RC_TL_VDM_CTL0: u64 = 0x0a20;
const PCIE_RC_TL_VDM_CTL0_VDM_ENABLED_MASK: u32 = 0x1_0000;
const PCIE_RC_TL_VDM_CTL0_VDM_IGNORETAG_MASK: u32 = 0x2_0000;
const PCIE_RC_TL_VDM_CTL0_VDM_IGNOREVNDRID_MASK: u32 = 0x4_0000;
const PCIE_RC_TL_VDM_CTL1: u64 = 0x0a0c;

const PCIE_MISC_CTRL_1: u64 = 0x40a0;
const PCIE_MISC_CTRL_1_EN_VDM_QOS_CONTROL_MASK: u32 = 1 << 5;

const PCIE_MISC_UBUS_CTRL: u64 = 0x40a4;
const PCIE_MISC_UBUS_CTRL_UBUS_PCIE_REPLY_ERR_DIS_MASK: u32 = 1 << 13;
const PCIE_MISC_UBUS_CTRL_UBUS_PCIE_REPLY_DECERR_DIS_MASK: u32 = 1 << 19;
const PCIE_MISC_UBUS_TIMEOUT: u64 = 0x40a8;
const PCIE_MISC_RC_CONFIG_RETRY_TIMEOUT: u64 = 0x405c;

fn pcie_misc_tc_queue_to_qos_map(x: u64) -> u64 { 0x4160 - x * 4 }
const PCIE_MISC_VDM_PRIORITY_TO_QOS_MAP_HI: u64 = 0x4164;
const PCIE_MISC_VDM_PRIORITY_TO_QOS_MAP_LO: u64 = 0x4168;

const PCIE_MISC_AXI_INTF_CTRL: u64 = 0x416c;
const AXI_EN_RCLK_QOS_ARRAY_FIX: u32 = 1 << 13;
const AXI_EN_QOS_UPDATE_TIMING_FIX: u32 = 1 << 12;
const AXI_DIS_QOS_GATING_IN_MASTER: u32 = 1 << 11;
const AXI_REQFIFO_EN_QOS_PROPAGATION: u32 = 1 << 7;
const AXI_MASTER_MAX_OUTSTANDING_REQUESTS_MASK: u32 = 0x3f;
const PCIE_MISC_AXI_READ_ERROR_DATA: u64 = 0x4170;

const MDIO_PORT0: u32 = 0x0;
const MDIO_PORT_MASK: u32 = 0xf_0000;
const MDIO_PORT_EXT_MASK: u32 = 0x20_0000;
const MDIO_REGAD_MASK: u32 = 0xffff;
const MDIO_CMD_MASK: u32 = 0x0010_0000;
const MDIO_CMD_WRITE: u32 = 0x0;
const MDIO_DATA_DONE_MASK: u32 = 0x8000_0000;
const SET_ADDR_OFFSET: u32 = 0x1f;

const BRCM_MSI_TARGET_ADDR_LT_4GB: u64 = 0x0_ffff_fffc;
const BRCM_MSI_TARGET_ADDR_GT_4GB: u64 = 0xf_ffff_fffc;

// pcie_offsets_bcm7712
const RGR1_SW_INIT_1: u64 = 0x9210;
const EXT_CFG_INDEX: u64 = 0x9000;
const EXT_CFG_DATA: u64 = 0x8000;
const HARD_DEBUG: u64 = 0x4304;

/// `bcm2712_cfg.num_inbound_wins`.
const NUM_INBOUND_WINS: usize = 10;
const BRCM_NUM_PCIE_OUT_WINS: usize = 4;
const PCIE_BRCM_MAX_INBOUND_WINS: usize = 16;
const PCIE_RESET_CONFIG_WAIT_MS: u64 = 100;
const SZ_4G: u64 = 1 << 32;
const SZ_1M: u64 = 1 << 20;

/// A window between CPU and PCI addresses: `ranges` (outbound) or
/// `dma-ranges` (inbound) of the controller's node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub cpu: u64,
    pub pci: u64,
    pub size: u64,
}

/// `brcm,clkreq-mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClkreqMode {
    Default,
    NoL1ss,
    Safe,
}

/// What the controller's device tree node says.
pub struct Config<'a> {
    /// The memory windows of `ranges`, in DT order.
    pub outbound: &'a [Window],
    /// `dma-ranges`, in DT order.
    pub inbound: &'a [Window],
    /// `max-link-speed`, 0 without one.
    pub max_link_speed: u32,
    /// `num-lanes`.
    pub num_lanes: Option<u32>,
    /// `aspm-no-l0s`.
    pub aspm_no_l0s: bool,
    /// `brcm,fifo-qos-map`.
    pub fifo_qos_map: Option<[u8; 4]>,
    /// `brcm,vdm-qos-map`.
    pub vdm_qos_map: Option<[u8; 8]>,
    /// `brcm,clkreq-mode`; an unknown string is `Safe`, as Linux falls back.
    pub clkreq_mode: ClkreqMode,
    /// `brcm,tperst-clk-ms`.
    pub tperst_clk_ms: u32,
    /// The `bridge` line on the brcmstb reset controller.
    pub bridge_reset: Option<u32>,
    /// The `swinit` line on the brcmstb reset controller.
    pub swinit_reset: Option<u32>,
    /// A `rescal` reset is present.
    pub rescal: bool,
}

/// Why bring-up stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Rescal(reset::RescalError),
    NoDmaRanges,
    EndpointMode,
    TooManyOutboundWindows,
    Mdio,
    LinkDown,
}

/// What the link came up with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// `PCI_EXP_LNKSTA_CLS`: 1 = 2.5, 2 = 5, 3 = 8 GT/s.
    pub speed: u16,
    pub width: u16,
}

/// Boot-time counters, `brcm_pcie_stats_capture`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub tx_tlp: u64,
    pub rx_tlp: u64,
    pub tx_dllp: u64,
    pub rx_dllp: u64,
    pub pl_rx_err: u32,
    pub dl_rx_err: u32,
    pub tl_rx_err: u32,
    pub l0s_exit_time: u16,
    pub recov_time: u16,
    pub l0s_fail_cnt: u16,
    pub recov_cnt: u16,
}

/// A probed controller.
pub struct Pcie {
    pub hw_rev: u32,
    pub msi_target_addr: u64,
    pub link: Link,
    pub stats: Stats,
    max_link_speed: u32,
    clkreq_mode: ClkreqMode,
    tperst_clk_ms: u32,
}

#[derive(Clone, Copy, Default)]
struct InboundWin {
    size: u64,
    pci_offset: u64,
    cpu_addr: u64,
}

/// `ilog2`.
fn ilog2(v: u64) -> u32 { 63 - v.leading_zeros() }

/// `brcm_pcie_encode_ibar_size`: the inbound window size field.
fn encode_ibar_size(size: u64) -> u32 {
    if size == 0 { return 0; }
    let log2_in = ilog2(size);
    if (12..=15).contains(&log2_in) {
        (log2_in - 12) + 0x1c
    } else if (16..=36).contains(&log2_in) {
        log2_in - 15
    } else {
        0
    }
}

fn mdio_form_pkt(port: u32, regad: u32, cmd: u32) -> u32 {
    let mut pkt = 0;
    replace_bits(&mut pkt, port >> 4, MDIO_PORT_EXT_MASK);
    replace_bits(&mut pkt, port, MDIO_PORT_MASK);
    replace_bits(&mut pkt, regad, MDIO_REGAD_MASK);
    replace_bits(&mut pkt, cmd, MDIO_CMD_MASK);
    pkt
}

/// `brcm_pcie_mdio_write`.
fn mdio_write(hw: &mut impl Hw, port: u32, regad: u32, wrdata: u16) -> Result<(), Error> {
    hw.w32(Block::Pcie, PCIE_RC_DL_MDIO_ADDR, mdio_form_pkt(port, regad, MDIO_CMD_WRITE));
    hw.r32(Block::Pcie, PCIE_RC_DL_MDIO_ADDR);
    hw.w32(Block::Pcie, PCIE_RC_DL_MDIO_WR_DATA, MDIO_DATA_DONE_MASK | wrdata as u32);
    let (ok, _) = poll(hw, Block::Pcie, PCIE_RC_DL_MDIO_WR_DATA, 10, 100, |d| d & MDIO_DATA_DONE_MASK == 0);
    if ok { Ok(()) } else { Err(Error::Mdio) }
}

/// `brcm_pcie_bridge_sw_init_set_generic` with a `bridge` reset line.
fn bridge_sw_init_set(hw: &mut impl Hw, cfg: &Config, val: bool) {
    match cfg.bridge_reset {
        Some(id) if val => reset::brcmstb_assert(hw, id),
        Some(id) => reset::brcmstb_deassert(hw, id),
        None => {
            const MASK: u32 = 0x2;
            let mut tmp = hw.r32(Block::Pcie, RGR1_SW_INIT_1);
            tmp = (tmp & !MASK) | ((val as u32) << 1 & MASK);
            hw.w32(Block::Pcie, RGR1_SW_INIT_1, tmp);
        }
    }
}

/// `brcm_pcie_perst_set_7278`.
fn perst_set(hw: &mut impl Hw, val: bool) {
    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_PCIE_CTRL);
    replace_bits(&mut tmp, !val as u32, PCIE_MISC_PCIE_CTRL_PCIE_PERSTB_MASK);
    hw.w32(Block::Pcie, PCIE_MISC_PCIE_CTRL, tmp);
}

/// `brcm_pcie_link_up`.
pub fn link_up(hw: &mut impl Hw) -> bool {
    let val = hw.r32(Block::Pcie, PCIE_MISC_PCIE_STATUS);
    field_get(val, PCIE_MISC_PCIE_STATUS_PCIE_DL_ACTIVE_MASK) != 0
        && field_get(val, PCIE_MISC_PCIE_STATUS_PCIE_PHYLINKUP_MASK) != 0
}

fn rc_mode(hw: &mut impl Hw) -> bool {
    let val = hw.r32(Block::Pcie, PCIE_MISC_PCIE_STATUS);
    field_get(val, PCIE_MISC_PCIE_STATUS_PCIE_PORT_MASK) != 0
}

/// `brcm_pcie_set_outbound_win`.
fn set_outbound_win(hw: &mut impl Hw, win: u64, cpu_addr: u64, pcie_addr: u64, size: u64) {
    hw.w32(Block::Pcie, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LO + win * 8, pcie_addr as u32);
    hw.w32(Block::Pcie, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_HI + win * 8, (pcie_addr >> 32) as u32);

    let cpu_addr_mb = cpu_addr / SZ_1M;
    let limit_addr_mb = (cpu_addr + size - 1) / SZ_1M;

    let off = PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT + win * 4;
    let mut tmp = hw.r32(Block::Pcie, off);
    replace_bits(&mut tmp, cpu_addr_mb as u32, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT_BASE_MASK);
    replace_bits(&mut tmp, limit_addr_mb as u32, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT_LIMIT_MASK);
    hw.w32(Block::Pcie, off, tmp);

    let high_addr_shift = PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_LIMIT_BASE_MASK.count_ones();
    let off = PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI + win * 8;
    let mut tmp = hw.r32(Block::Pcie, off);
    replace_bits(&mut tmp, (cpu_addr_mb >> high_addr_shift) as u32, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_BASE_HI_BASE_MASK);
    hw.w32(Block::Pcie, off, tmp);

    let off = PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI + win * 8;
    let mut tmp = hw.r32(Block::Pcie, off);
    replace_bits(&mut tmp, (limit_addr_mb >> high_addr_shift) as u32, PCIE_MISC_CPU_2_PCIE_MEM_WIN0_LIMIT_HI_LIMIT_MASK);
    hw.w32(Block::Pcie, off, tmp);
}

/// `brcm_pcie_get_inbound_wins` for BCM7712-based chips: one window per
/// `dma-ranges` entry, numbered from 1.
fn get_inbound_wins(cfg: &Config, wins: &mut [InboundWin; PCIE_BRCM_MAX_INBOUND_WINS]) -> Result<usize, Error> {
    let mut n = 0;
    for r in cfg.inbound {
        wins[1 + n] = InboundWin { size: r.size, cpu_addr: r.cpu, pci_offset: r.pci };
        n += 1;
        if n > NUM_INBOUND_WINS { break; }
    }
    if cfg.inbound.is_empty() {
        return Err(Error::NoDmaRanges);
    }
    Ok(n)
}

fn bar_reg_offset(bar: usize) -> u64 {
    if bar <= 3 {
        PCIE_MISC_RC_BAR1_CONFIG_LO + 8 * (bar as u64 - 1)
    } else {
        PCIE_MISC_RC_BAR4_CONFIG_LO + 8 * (bar as u64 - 4)
    }
}

fn ubus_reg_offset(bar: usize) -> u64 {
    if bar <= 3 {
        PCIE_MISC_UBUS_BAR1_CONFIG_REMAP + 8 * (bar as u64 - 1)
    } else {
        PCIE_MISC_UBUS_BAR4_CONFIG_REMAP + 8 * (bar as u64 - 4)
    }
}

/// `set_inbound_win_registers`.
fn set_inbound_win_registers(hw: &mut impl Hw, wins: &[InboundWin], n: usize) {
    for (i, w) in wins.iter().enumerate().skip(1).take(n) {
        let reg_offset = bar_reg_offset(i);
        let mut tmp = w.pci_offset as u32;
        replace_bits(&mut tmp, encode_ibar_size(w.size), PCIE_MISC_RC_BAR1_CONFIG_LO_SIZE_MASK);
        hw.w32(Block::Pcie, reg_offset, tmp);
        hw.w32(Block::Pcie, reg_offset + 4, (w.pci_offset >> 32) as u32);

        let reg_offset = ubus_reg_offset(i);
        let tmp = (w.cpu_addr as u32 & !0xfff) | PCIE_MISC_UBUS_BAR1_CONFIG_REMAP_ACCESS_EN_MASK;
        hw.w32(Block::Pcie, reg_offset, tmp);
        hw.w32(Block::Pcie, reg_offset + 4, (w.cpu_addr >> 32) as u32);
    }
}

/// `brcm_pcie_post_setup_bcm2712`.
fn post_setup_bcm2712(hw: &mut impl Hw, cfg: &Config) -> Result<(), Error> {
    const DATA: [u16; 7] = [0x50b9, 0xbda1, 0x0094, 0x97b4, 0x5030, 0x5030, 0x0007];
    const REGS: [u32; 7] = [0x16, 0x17, 0x18, 0x19, 0x1b, 0x1c, 0x1e];

    // Allow a 54 MHz (xosc) refclk source.
    mdio_write(hw, MDIO_PORT0, SET_ADDR_OFFSET, 0x1600)?;
    for (&r, &d) in REGS.iter().zip(DATA.iter()) {
        mdio_write(hw, MDIO_PORT0, r, d)?;
    }
    hw.delay_us(100);

    // L1SS sub-state timers: PM clock period 18.52 ns (1/54 MHz).
    let mut tmp = hw.r32(Block::Pcie, PCIE_RC_PL_PHY_CTL_15);
    tmp &= !PCIE_RC_PL_PHY_CTL_15_PM_CLK_PERIOD_MASK;
    tmp |= 0x12;
    hw.w32(Block::Pcie, PCIE_RC_PL_PHY_CTL_15, tmp);

    // UBUS-AXI bridge: no AXI error responses, all-ones for failed reads.
    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_UBUS_CTRL);
    replace_bits(&mut tmp, 1, PCIE_MISC_UBUS_CTRL_UBUS_PCIE_REPLY_ERR_DIS_MASK);
    replace_bits(&mut tmp, 1, PCIE_MISC_UBUS_CTRL_UBUS_PCIE_REPLY_DECERR_DIS_MASK);
    hw.w32(Block::Pcie, PCIE_MISC_UBUS_CTRL, tmp);
    hw.w32(Block::Pcie, PCIE_MISC_AXI_READ_ERROR_DATA, 0xffff_ffff);

    // UBUS timeout 250 ms, config retry timeout ~240 ms (750 MHz clocks).
    hw.w32(Block::Pcie, PCIE_MISC_UBUS_TIMEOUT, 0x0B2D_0000);
    hw.w32(Block::Pcie, PCIE_MISC_RC_CONFIG_RETRY_TIMEOUT, 0x0ABA_0000);

    // Broken forwarding search off; the 2712D0 fix bits on.
    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_AXI_INTF_CTRL);
    tmp &= !AXI_REQFIFO_EN_QOS_PROPAGATION;
    tmp |= AXI_EN_RCLK_QOS_ARRAY_FIX | AXI_EN_QOS_UPDATE_TIMING_FIX | AXI_DIS_QOS_GATING_IN_MASTER;
    hw.w32(Block::Pcie, PCIE_MISC_AXI_INTF_CTRL, tmp);

    // Without the timing fix (2712C1, single-lane RC) throttle requests
    // in flight to SDRAM instead.
    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_AXI_INTF_CTRL);
    if tmp & AXI_EN_QOS_UPDATE_TIMING_FIX == 0 {
        tmp &= !AXI_MASTER_MAX_OUTSTANDING_REQUESTS_MASK;
        tmp |= 15;
        hw.w32(Block::Pcie, PCIE_MISC_AXI_INTF_CTRL, tmp);
    }

    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_CTRL_1);
    tmp &= !PCIE_MISC_CTRL_1_EN_VDM_QOS_CONTROL_MASK;
    hw.w32(Block::Pcie, PCIE_MISC_CTRL_1, tmp);

    if let Some(map) = cfg.fifo_qos_map {
        let tmp = map.iter().enumerate().fold(0u32, |t, (i, &q)| t | ((q & 0x0f) as u32) << (i * 4));
        for i in 0..8 {
            hw.w32(Block::Pcie, pcie_misc_tc_queue_to_qos_map(i), tmp);
        }
    } else if let Some(map) = cfg.vdm_qos_map {
        let mut t = hw.r32(Block::Pcie, PCIE_MISC_CTRL_1);
        t |= PCIE_MISC_CTRL_1_EN_VDM_QOS_CONTROL_MASK;
        hw.w32(Block::Pcie, PCIE_MISC_CTRL_1, t);

        let tmp = map.iter().enumerate().fold(0u32, |t, (i, &q)| t | ((q & 0x0f) as u32) << (i * 4));
        hw.w32(Block::Pcie, PCIE_MISC_VDM_PRIORITY_TO_QOS_MAP_LO, tmp);
        hw.w32(Block::Pcie, PCIE_MISC_VDM_PRIORITY_TO_QOS_MAP_HI, tmp);
        hw.w32(Block::Pcie, PCIE_RC_TL_VDM_CTL1, 0);

        let mut t = hw.r32(Block::Pcie, PCIE_RC_TL_VDM_CTL0);
        t |= PCIE_RC_TL_VDM_CTL0_VDM_ENABLED_MASK | PCIE_RC_TL_VDM_CTL0_VDM_IGNORETAG_MASK
            | PCIE_RC_TL_VDM_CTL0_VDM_IGNOREVNDRID_MASK;
        hw.w32(Block::Pcie, PCIE_RC_TL_VDM_CTL0, t);
    }
    Ok(())
}

/// `brcm_pcie_setup`; returns the MSI target address.
fn setup(hw: &mut impl Hw, cfg: &Config) -> Result<u64, Error> {
    let mut wins = [InboundWin::default(); PCIE_BRCM_MAX_INBOUND_WINS];

    bridge_sw_init_set(hw, cfg, true);
    hw.delay_us(100);
    bridge_sw_init_set(hw, cfg, false);

    let mut tmp = hw.r32(Block::Pcie, HARD_DEBUG);
    tmp &= !PCIE_MISC_HARD_PCIE_HARD_DEBUG_SERDES_IDDQ_MASK;
    hw.w32(Block::Pcie, HARD_DEBUG, tmp);
    // SerDes settles.
    hw.delay_us(100);

    // SCB_MAX_BURST_SIZE 2 = 512 bytes.
    let burst = 0x2;
    let mut tmp = hw.r32(Block::Pcie, PCIE_MISC_MISC_CTRL);
    replace_bits(&mut tmp, 1, PCIE_MISC_MISC_CTRL_SCB_ACCESS_EN_MASK);
    replace_bits(&mut tmp, 1, PCIE_MISC_MISC_CTRL_CFG_READ_UR_MODE_MASK);
    replace_bits(&mut tmp, burst, PCIE_MISC_MISC_CTRL_MAX_BURST_SIZE_MASK);
    replace_bits(&mut tmp, 1, PCIE_MISC_MISC_CTRL_PCIE_RCB_MPS_MODE_MASK);
    replace_bits(&mut tmp, 1, PCIE_MISC_MISC_CTRL_PCIE_RCB_64B_MODE_MASK);
    hw.w32(Block::Pcie, PCIE_MISC_MISC_CTRL, tmp);

    let n = get_inbound_wins(cfg, &mut wins)?;
    set_inbound_win_registers(hw, &wins, n);

    if !rc_mode(hw) {
        return Err(Error::EndpointMode);
    }

    // BCM7712 parts have no memory controller sizes to program
    // (`num_memc` stays 0); the register is written back unchanged.
    let tmp = hw.r32(Block::Pcie, PCIE_MISC_MISC_CTRL);
    hw.w32(Block::Pcie, PCIE_MISC_MISC_CTRL, tmp);

    let msi_target_addr = if wins[2].pci_offset >= SZ_4G || wins[2].size + wins[2].pci_offset < SZ_4G {
        BRCM_MSI_TARGET_ADDR_LT_4GB
    } else {
        BRCM_MSI_TARGET_ADDR_GT_4GB
    };

    let mut tmp = hw.r32(Block::Pcie, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY);
    if cfg.aspm_no_l0s {
        tmp &= !PCI_EXP_LNKCAP_ASPM_L0S;
    }
    hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY, tmp);

    let num_lanes_cap = field_get(tmp, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY_MAX_LINK_WIDTH_MASK);
    if let Some(num_lanes) = cfg.num_lanes
        && num_lanes != 0 && num_lanes <= 4 && num_lanes_cap != num_lanes
    {
        replace_bits(&mut tmp, num_lanes, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY_MAX_LINK_WIDTH_MASK);
        hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY, tmp);
        let mut t = hw.r32(Block::Pcie, PCIE_RC_PL_REG_PHY_CTL_1);
        replace_bits(&mut t, 1, PCIE_RC_PL_REG_PHY_CTL_1_REG_P2_POWERDOWN_ENA_NOSYNC_MASK);
        hw.w32(Block::Pcie, PCIE_RC_PL_REG_PHY_CTL_1, t);
    }

    // The RC shows the class of a PCI-PCI bridge (its default is EP mode).
    let mut tmp = hw.r32(Block::Pcie, PCIE_RC_CFG_PRIV1_ID_VAL3);
    replace_bits(&mut tmp, 0x06_0400, PCIE_RC_CFG_PRIV1_ID_VAL3_CLASS_CODE_MASK);
    hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_ID_VAL3, tmp);

    if cfg.outbound.len() > BRCM_NUM_PCIE_OUT_WINS {
        return Err(Error::TooManyOutboundWindows);
    }
    for (i, w) in cfg.outbound.iter().enumerate() {
        set_outbound_win(hw, i as u64, w.cpu, w.pci, w.size);
    }

    let mut tmp = hw.r32(Block::Pcie, PCIE_RC_CFG_VENDOR_VENDOR_SPECIFIC_REG1);
    replace_bits(&mut tmp, PCIE_RC_CFG_VENDOR_SPECIFIC_REG1_LITTLE_ENDIAN,
        PCIE_RC_CFG_VENDOR_VENDOR_SPECIFIC_REG1_ENDIAN_MODE_BAR2_MASK);
    hw.w32(Block::Pcie, PCIE_RC_CFG_VENDOR_VENDOR_SPECIFIC_REG1, tmp);

    post_setup_bcm2712(hw, cfg)?;
    Ok(msi_target_addr)
}

/// `brcm_pcie_probe` up to `pci_host_probe`: resets, setup, revision.
pub fn probe(hw: &mut impl Hw, cfg: &Config) -> Result<Pcie, Error> {
    bridge_sw_init_set(hw, cfg, false);
    if let Some(id) = cfg.swinit_reset {
        reset::brcmstb_assert(hw, id);
        // 1 us for the reset to propagate.
        hw.delay_us(1);
        reset::brcmstb_deassert(hw, id);
    }
    if cfg.rescal {
        reset::rescal_reset(hw).map_err(Error::Rescal)?;
    }
    let msi_target_addr = setup(hw, cfg)?;
    let hw_rev = hw.r32(Block::Pcie, PCIE_MISC_REVISION);
    Ok(Pcie {
        hw_rev,
        msi_target_addr,
        link: Link::default(),
        stats: Stats::default(),
        max_link_speed: cfg.max_link_speed,
        clkreq_mode: cfg.clkreq_mode,
        tperst_clk_ms: cfg.tperst_clk_ms,
    })
}

impl Pcie {
    /// `brcm_pcie_set_gen`.
    fn set_gen(&self, hw: &mut impl Hw, speed: u32) {
        let mut lnkctl2 = hw.r16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_LNKCTL2) as u32;
        let mut lnkcap = hw.r32(Block::Pcie, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY);
        replace_bits(&mut lnkcap, speed, PCI_EXP_LNKCAP_SLS);
        hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_LINK_CAPABILITY, lnkcap);
        replace_bits(&mut lnkctl2, speed, PCI_EXP_LNKCTL2_TLS);
        hw.w16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_LNKCTL2, lnkctl2 as u16);
    }

    /// `brcm_pcie_stats_trigger`: clear and start the counters; 0 = run on.
    fn stats_trigger(&self, hw: &mut impl Hw, mut micros: u32) {
        let mut tmp = hw.r32(Block::Pcie, PCIE_RC_PL_STATS_CTRL);
        replace_bits(&mut tmp, 0, PCIE_RC_PL_STATS_CTRL_EN_MASK);
        hw.w32(Block::Pcie, PCIE_RC_PL_STATS_CTRL, tmp);
        if micros >= 1 << 28 {
            micros = (1 << 28) - 1;
        }
        replace_bits(&mut tmp, micros, PCIE_RC_PL_STATS_CTRL_LEN_MASK);
        replace_bits(&mut tmp, 1, PCIE_RC_PL_STATS_CTRL_EN_MASK);
        hw.w32(Block::Pcie, PCIE_RC_PL_STATS_CTRL, tmp);
    }

    /// `brcm_pcie_stats_capture`.
    fn stats_capture(&mut self, hw: &mut impl Hw) {
        let r64 = |hw: &mut _, lo, hi| {
            let l = Hw::r32(hw, Block::Pcie, lo) as u64;
            l + ((Hw::r32(hw, Block::Pcie, hi) as u64) << 32)
        };
        self.stats.tx_tlp = r64(hw, PCIE_RC_PL_STATS_TXTLP_LO, PCIE_RC_PL_STATS_TXTLP_HI);
        self.stats.rx_tlp = r64(hw, PCIE_RC_PL_STATS_RXTLP_LO, PCIE_RC_PL_STATS_RXTLP_HI);
        self.stats.tx_dllp = r64(hw, PCIE_RC_PL_STATS_TXDLLP_LO, PCIE_RC_PL_STATS_TXDLLP_HI);
        self.stats.rx_dllp = r64(hw, PCIE_RC_PL_STATS_RXDLLP_LO, PCIE_RC_PL_STATS_RXDLLP_HI);
        self.stats.pl_rx_err = hw.r32(Block::Pcie, PCIE_RC_PL_STATS_RXPL_ERR);
        self.stats.dl_rx_err = hw.r32(Block::Pcie, PCIE_RC_PL_STATS_RXDL_ERR);
        self.stats.tl_rx_err = hw.r32(Block::Pcie, PCIE_RC_PL_STATS_RXTL_ERR);
        let tmp = hw.r32(Block::Pcie, PCIE_RC_PL_LTSSM_STATS_3);
        self.stats.l0s_exit_time = (tmp >> 16) as u16;
        self.stats.recov_time = tmp as u16;
        let tmp = hw.r32(Block::Pcie, PCIE_RC_PL_LTSSM_STATS_CNT);
        self.stats.l0s_fail_cnt = (tmp >> 16) as u16;
        self.stats.recov_cnt = tmp as u16;
    }

    /// `brcm_config_clkreq`. BCM7712 parts have no RGR1 timeout to extend.
    fn config_clkreq(&self, hw: &mut impl Hw) {
        let mut clkreq_cntl = hw.r32(Block::Pcie, HARD_DEBUG);
        clkreq_cntl &= !PCIE_CLKREQ_MASK;
        match self.clkreq_mode {
            ClkreqMode::NoL1ss => {
                clkreq_cntl |= PCIE_MISC_HARD_PCIE_HARD_DEBUG_CLKREQ_DEBUG_ENABLE_MASK;
                let mut tmp = hw.r32(Block::Pcie, PCIE_RC_CFG_PRIV1_ROOT_CAP);
                replace_bits(&mut tmp, 2, PCIE_RC_CFG_PRIV1_ROOT_CAP_L1SS_MODE_MASK);
                hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_ROOT_CAP, tmp);
            }
            ClkreqMode::Default => {
                clkreq_cntl |= PCIE_MISC_HARD_PCIE_HARD_DEBUG_L1SS_ENABLE_MASK;
            }
            ClkreqMode::Safe => {
                clkreq_cntl |= PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_OUT_MASK;
                clkreq_cntl |= PCIE_MISC_HARD_PCIE_HARD_DEBUG_REFCLK_OVRD_ENABLE_MASK;
                let mut tmp = hw.r32(Block::Pcie, PCIE_RC_CFG_PRIV1_ROOT_CAP);
                replace_bits(&mut tmp, 2, PCIE_RC_CFG_PRIV1_ROOT_CAP_L1SS_MODE_MASK);
                hw.w32(Block::Pcie, PCIE_RC_CFG_PRIV1_ROOT_CAP, tmp);
            }
        }
        hw.w32(Block::Pcie, HARD_DEBUG, clkreq_cntl);
    }

    /// `brcm_pcie_start_link`: release PERST#, wait for the link, report
    /// speed and width.
    pub fn start_link(&mut self, hw: &mut impl Hw) -> Result<Link, Error> {
        if self.max_link_speed != 0 {
            self.set_gen(hw, self.max_link_speed);
        }
        self.stats_trigger(hw, 0);

        // CLKREQ# input off until the link is up.
        let mut tmp = hw.r32(Block::Pcie, HARD_DEBUG);
        tmp &= !PCIE_CLKREQ_MASK;
        hw.w32(Block::Pcie, HARD_DEBUG, tmp);

        if self.tperst_clk_ms != 0 {
            // PERST# held low while the internal reset is released, so the
            // PLL drives a stable refclk before PERST# goes away.
            let mut tmp = hw.r32(Block::Pcie, HARD_DEBUG);
            replace_bits(&mut tmp, 1, PCIE_MISC_HARD_PCIE_HARD_DEBUG_PERST_ASSERT_MASK);
            hw.w32(Block::Pcie, HARD_DEBUG, tmp);
            perst_set(hw, false);
            hw.delay_us(self.tperst_clk_ms as u64 * 1000);
            let mut tmp = hw.r32(Block::Pcie, HARD_DEBUG);
            replace_bits(&mut tmp, 0, PCIE_MISC_HARD_PCIE_HARD_DEBUG_PERST_ASSERT_MASK);
            hw.w32(Block::Pcie, HARD_DEBUG, tmp);
        } else {
            perst_set(hw, false);
        }

        // 100 ms after PERST# (PCIe CEM 2.2, PCIe r5.0 6.6.1), then up to
        // 100 ms more in 5 ms steps.
        hw.delay_us(PCIE_RESET_CONFIG_WAIT_MS * 1000);
        let mut i = 0;
        while i < 100 && !link_up(hw) {
            hw.delay_us(5000);
            i += 5;
        }
        if !link_up(hw) {
            return Err(Error::LinkDown);
        }

        self.config_clkreq(hw);

        let lnksta = hw.r16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_LNKSTA) as u32;
        self.link = Link {
            speed: field_get(lnksta, PCI_EXP_LNKSTA_CLS) as u16,
            width: field_get(lnksta, PCI_EXP_LNKSTA_NLW) as u16,
        };
        self.stats_capture(hw);

        // PERST# reset the RootCtl bits; CRS software visibility back on.
        let tmp16 = hw.r16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_RTCAP);
        if tmp16 & PCI_EXP_RTCAP_CRSVIS != 0 {
            let mut t = hw.r16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_RTCTL);
            t |= PCI_EXP_RTCTL_CRSSVE;
            hw.w16(Block::Pcie, BRCM_PCIE_CAP_REGS + PCI_EXP_RTCTL, t);
        }
        Ok(self.link)
    }
}

/// `brcm_pcie_map_bus`: the register offset of config space `where` of
/// `bus`/`devfn`, or None where Linux returns NULL. The root bus holds
/// only the RC itself; devices behind it go through the index window,
/// which must not be touched without a link (the access aborts).
pub fn map_bus(hw: &mut impl Hw, bus: u8, devfn: u8, wh: u16) -> Option<u64> {
    let reg = (wh & 0xfff) as u64;
    if bus == 0 {
        return if devfn == 0 { Some(reg) } else { None };
    }
    if !link_up(hw) {
        return None;
    }
    let idx = (bus as u32) << 20 | (devfn as u32) << 12;
    hw.w32(Block::Pcie, EXT_CFG_INDEX, idx);
    Some(EXT_CFG_DATA + reg)
}

/// `pci_generic_config_read`: all ones where nothing answers.
pub fn cfg_read(hw: &mut impl Hw, bus: u8, devfn: u8, wh: u16, width: u8) -> u32 {
    match map_bus(hw, bus, devfn, wh) {
        Some(off) => hw.read(Block::Pcie, off, width),
        None => !0 >> (32 - 8 * width as u32),
    }
}

/// `pci_generic_config_write`.
pub fn cfg_write(hw: &mut impl Hw, bus: u8, devfn: u8, wh: u16, width: u8, v: u32) {
    if let Some(off) = map_bus(hw, bus, devfn, wh) {
        hw.write(Block::Pcie, off, width, v);
    }
}
