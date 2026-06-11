//! Realtek RTL8153 USB Gigabit-Ethernet driver (USB-C dongles).
//!
//! Ported from Linux `drivers/net/usb/r8152.c` (RTL_VER_03/04/05 path), minimal
//! bring-up: register access via EP0 control transfers, chip init, MAC read,
//! and the RTL8153 tx_desc/rx_desc framing over bulk EP1 (IN) / EP2 (OUT).
//! The USB transport (control + bulk) lives in `xhci`; this is the chip logic.

use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use spin::Mutex;
use crate::virtio_net::NetError;
use crate::drivers::netdev::MTU;

const VID: u16 = 0x0bda;
const PID: u16 = 0x8153;
const EP_IN: u8 = 1;
const EP_OUT: u8 = 2;

// Control-transfer primitives (r8152.h)
const REQT_READ: u8 = 0xc0;
const REQT_WRITE: u8 = 0x40;
const REQ_REGS: u8 = 0x05;
const MCU_PLA: u16 = 0x0100;
const MCU_USB: u16 = 0x0000;
const BYEN_DWORD: u16 = 0xff;

// PLA registers
const PLA_IDR: u16 = 0xc000;
const PLA_RCR: u16 = 0xc010;
const PLA_RMS: u16 = 0xc016;
const PLA_RXFIFO_CTRL0: u16 = 0xc0a0;
const PLA_RXFIFO_CTRL1: u16 = 0xc0a4;
const PLA_RXFIFO_CTRL2: u16 = 0xc0a8;
const PLA_DMY_REG0: u16 = 0xc0b0;
const PLA_FMC: u16 = 0xc0b4;
const PLA_TEREDO_CFG: u16 = 0xc0bc;
const PLA_BACKUP: u16 = 0xd000;
const PLA_TEREDO_TIMER: u16 = 0xd2cc;
const PLA_REALWOW_TIMER: u16 = 0xd2e8;
const PLA_LED_FEATURE: u16 = 0xdd92;
const PLA_BOOT_CTRL: u16 = 0xe004;
const PLA_LWAKE_CTRL_REG: u16 = 0xe007;
const PLA_WDT6_CTRL: u16 = 0xe428;
const PLA_TCR0: u16 = 0xe610;
const PLA_MTPS: u16 = 0xe615;
const PLA_TXFIFO_CTRL: u16 = 0xe618;
const PLA_RSTTALLY: u16 = 0xe800;   // tally-counter reset
const PLA_TALLYCNT: u16 = 0xe890;   // hardware stats block (struct tally_counter)
const TALLY_RESET: u16 = 0x0001;
const PLA_PHYSTATUS: u16 = 0xe908;  // negotiated link speed/duplex (r8152)
const PLA_CR: u16 = 0xe813;
const PLA_CRWECR: u16 = 0xe81c;
const PLA_PHY_PWR: u16 = 0xe84c;
const PLA_OOB_CTRL: u16 = 0xe84f;
const PLA_MISC_1: u16 = 0xe85a;
const PLA_OCP_GPHY_BASE: u16 = 0xe86c;
const PLA_SFF_STS_7: u16 = 0xe8de;
const PLA_CONFIG6: u16 = 0xe90a;

// USB registers
const USB_USB2PHY: u16 = 0xb41e;
const USB_SSPHYLINK2: u16 = 0xb428;
const USB_U2P3_CTRL: u16 = 0xb460;
const USB_CSR_DUMMY1: u16 = 0xb464;
const USB_CSR_DUMMY2: u16 = 0xb466;
const USB_BURST_SIZE: u16 = 0xcfc0;
const USB_CONNECT_TIMER: u16 = 0xcbf8;
const USB_USB_CTRL: u16 = 0xd406;
const USB_LPM_CTRL: u16 = 0xd41a;
const USB_TOLERANCE: u16 = 0xd490;
const USB_BMU_RESET: u16 = 0xd4b0;
const USB_AFE_CTRL2: u16 = 0xd824;
const USB_WDT11_CTRL: u16 = 0xe43c;

// PHY/OCP (via base window)
const OCP_POWER_CFG: u16 = 0xa430;
const OCP_EEE_CFG: u16 = 0xa432;
const OCP_DOWN_SPEED: u16 = 0xa442;
const OCP_SRAM_ADDR: u16 = 0xa436;
const OCP_SRAM_DATA: u16 = 0xa438;
const OCP_PHY_STATUS: u16 = 0xa420;
const OCP_ADC_CFG: u16 = 0xbc06;
const OCP_BASE_MII: u16 = 0xa400;

// SRAM
const SRAM_LPF_CFG: u16 = 0x8012;
const SRAM_10M_AMP1: u16 = 0x8080;
const SRAM_10M_AMP2: u16 = 0x8082;
const SRAM_IMPEDANCE: u16 = 0x8084;

// Bits
const RCR_AB: u32 = 0x08;
const RCR_APM: u32 = 0x02;
const RCR_AM: u32 = 0x04;
const RCR_ACPT_ALL: u32 = 0x0f;
const CR_RST: u8 = 0x10;
const CR_RE: u8 = 0x08;
const CR_TE: u8 = 0x04;
const CRWECR_CONFIG: u8 = 0xc0;
const CRWECR_NORMAL: u8 = 0x00;
const NOW_IS_OOB: u8 = 0x80;
const LINK_LIST_READY: u8 = 0x02;
const RXDY_GATED_EN: u16 = 0x0008;
const RE_INIT_LL: u16 = 0x8000;
const MCU_BORW_EN: u16 = 0x4000;
const TCR0_AUTO_FIFO: u16 = 0x0080;
const AUTOLOAD_DONE: u16 = 0x0002;
const FMC_FCR_MCU_EN: u16 = 0x0001;
const BMU_RESET_EP_IN: u8 = 0x01;
const BMU_RESET_EP_OUT: u8 = 0x02;
const RX_AGG_DISABLE: u16 = 0x0010;
const RX_ZERO_EN: u16 = 0x0080;
const MTPS_JUMBO: u8 = 192;
const RXFIFO_THR1_NORMAL: u32 = 0x0008_0002;
const RXFIFO_THR2_NORMAL: u16 = 0x00a0;
const RXFIFO_THR3_NORMAL: u16 = 0x0110;
const TXFIFO_THR_NORMAL2: u32 = 0x0100_0008;
const ADC_CFG_INIT: u16 = 0x01c0;            // CKADSEL_L | ADC_EN | EN_EMI_L
const EN_ALDPS: u16 = 0x0004;
const EEE_CLKDIV_EN: u16 = 0x8000;
const EN_10M_PLLOFF: u16 = 0x0001;
const EN_10M_BGOFF: u16 = 0x0080;
const CTAP_SHORT_EN: u16 = 0x0040;
const PFM_PWM_SWITCH: u16 = 0x0040;
const LED_MODE_MASK: u16 = 0x0700;
const TIMER11_EN: u16 = 0x0001;
const EP4_FULL_FC: u16 = 0x0001;
const LANWAKE_CLR_EN: u8 = 0x01;
const LANWAKE_PIN: u8 = 0x80;
const SEN_VAL_MASK: u16 = 0xf800;
const SEN_VAL_NORMAL: u16 = 0xa000;
const SEL_RXIDLE: u16 = 0x0100;
const LPM_CTRL_VAL: u8 = 0x3e;               // FIFO_EMPTY_1FB | ROK_EXIT_LPM | LPM_TIMER_500US
const U2P3_ENABLE: u16 = 0x0001;
const USB2PHY_L1: u8 = 0x0002;
const USB2PHY_SUSPEND: u8 = 0x0001;
const MII_BMCR: u16 = 0x00;
const MII_BMSR: u16 = 0x01;
const BMCR_PDOWN: u16 = 0x0800;
const BMSR_LSTATUS: u16 = 0x0004;
const PHY_STAT_MASK: u16 = 0x0007;
const PHY_STAT_LAN_ON: u16 = 3;

// rx_desc / tx_desc
const RX_DESC_LEN: usize = 24;
const TX_DESC_LEN: usize = 8;
const RX_LEN_MASK: u32 = 0x7fff;
const TX_FS: u32 = 1 << 31;
const TX_LS: u32 = 1 << 30;
const ETH_FCS_LEN: usize = 4;

const RX_BATCH_BYTES: usize = 16 * 1024;

// RX aggregation (USB MCU regs) — r8153_set_rx_early_{timeout,size}.
const USB_RX_EARLY_TIMEOUT: u16 = 0xd42c;
const USB_RX_EARLY_SIZE: u16 = 0xd42e;
const COALESCE_SUPER: u32 = 85_000;   // USB SuperSpeed
const COALESCE_HIGH: u32 = 250_000;   // USB High-Speed
const COALESCE_SLOW: u32 = 524_280;   // USB Full / other

// EEE (Energy Efficient Ethernet) — must be configured, not left at ROM
// default. With EEE on, the PHY enters Low-Power-Idle between bursts and drops
// the first frames of the next burst while waking → bursty TCP loss. We disable
// it (Linux keeps it on only with the full PHY-tuning + wake-timing FW patch we
// skip). r8153_eee_en(false) for RTL_VER_03/04/05.
const PLA_EEE_CR: u16 = 0xe040;
const EEE_RX_EN: u16 = 0x0001;
const EEE_TX_EN: u16 = 0x0002;
const EEE10_EN: u16 = 0x0010;          // OCP_EEE_CFG bit (PHY window)
const OCP_EEE_ADV: u16 = 0xa5d0;
// Flow control: advertise 802.3x PAUSE so the upstream switch throttles us
// instead of overrunning the chip RX FIFO (r8152b_enable_fc).
const MII_ADVERTISE: u16 = 0x04;
const ADVERTISE_PAUSE_CAP: u16 = 0x0400;
const ADVERTISE_PAUSE_ASYM: u16 = 0x0800;
// Speed advertisement (MII_ADVERTISE base page + MII_CTRL1000 gigabit page).
const MII_CTRL1000: u16 = 0x09;
const ADVERTISE_10HALF: u16 = 0x0020;
const ADVERTISE_10FULL: u16 = 0x0040;
const ADVERTISE_100HALF: u16 = 0x0080;
const ADVERTISE_100FULL: u16 = 0x0100;
const ADVERTISE_SPEED_MASK: u16 = ADVERTISE_10HALF | ADVERTISE_10FULL
    | ADVERTISE_100HALF | ADVERTISE_100FULL;
const ADVERTISE_1000FULL: u16 = 0x0200;
const ADVERTISE_1000HALF: u16 = 0x0100;
const BMCR_ANRESTART: u16 = 0x0200;
const BMCR_ANENABLE: u16 = 0x1000;
// Inter-frame gap (rtl_set_ifg) + 10M idle, speed-dependent (PLA_PHYSTATUS bits).
const PLA_TCR1: u16 = 0xe612;
const PLA_MAC_PWR_CTRL4: u16 = 0xe0ce;
const PLA_EEEP_CR: u16 = 0xe080;
const IFG_MASK: u16 = (1 << 3) | (1 << 9) | (1 << 8);
const IFG_144NS: u16 = 1 << 9;
const IFG_96NS: u16 = (1 << 9) | (1 << 8);
const TX10MIDLE_EN: u16 = 0x0100;
const EEEP_CR_EEEP_TX: u16 = 0x0002;
const SPD_10: u16 = 0x04;             // PLA_PHYSTATUS speed bits
const SPD_100: u16 = 0x08;
const SPD_FULL: u16 = 0x01;

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static MAC: Mutex<[u8; 6]> = Mutex::new([0; 6]);
static OCP_BASE: AtomicU16 = AtomicU16::new(0xffff);

struct RxBatch { buf: [u8; RX_BATCH_BYTES], valid: usize, cursor: usize }
static RX: Mutex<RxBatch> = Mutex::new(RxBatch { buf: [0; RX_BATCH_BYTES], valid: 0, cursor: 0 });
static TX: Mutex<[u8; TX_DESC_LEN + MTU]> = Mutex::new([0; TX_DESC_LEN + MTU]);

// ── Register access (built on xhci EP0 control transfers) ──

fn get_regs(index: u16, mcu: u16, buf: &mut [u8]) -> bool {
    crate::xhci::nic_control(REQT_READ, REQ_REGS, index, mcu, buf, true)
}
fn set_regs(index: u16, mcu_byen: u16, buf: &mut [u8]) -> bool {
    crate::xhci::nic_control(REQT_WRITE, REQ_REGS, index, mcu_byen, buf, false)
}

fn ocp_read_dword(mcu: u16, index: u16) -> u32 {
    let mut b = [0u8; 4];
    get_regs(index, mcu, &mut b);
    u32::from_le_bytes(b)
}
fn ocp_write_dword(mcu: u16, index: u16, val: u32) {
    let mut b = val.to_le_bytes();
    set_regs(index, mcu | BYEN_DWORD, &mut b);
}
fn ocp_read_word(mcu: u16, index: u16) -> u16 {
    let shift = (index & 2) * 8;
    ((ocp_read_dword(mcu, index & !3) >> shift) & 0xffff) as u16
}
fn ocp_write_word(mcu: u16, index: u16, val: u16) {
    let mut byen: u16 = 0x33;
    let mut data = val as u32;
    let mut idx = index;
    if index & 2 != 0 { byen <<= 2; data <<= 16; idx &= !3; }
    let mut b = data.to_le_bytes();
    set_regs(idx, mcu | byen, &mut b);
}
fn ocp_read_byte(mcu: u16, index: u16) -> u8 {
    let shift = (index & 3) * 8;
    ((ocp_read_dword(mcu, index & !3) >> shift) & 0xff) as u8
}
fn ocp_write_byte(mcu: u16, index: u16, val: u8) {
    let shift = index & 3;
    let mut byen: u16 = 0x11;
    let mut data = val as u32;
    let mut idx = index;
    if shift != 0 { byen <<= shift; data <<= shift * 8; idx &= !3; }
    let mut b = data.to_le_bytes();
    set_regs(idx, mcu | byen, &mut b);
}

fn ocp_reg_write(addr: u16, data: u16) {
    let base = addr & 0xf000;
    if base != OCP_BASE.load(Ordering::Relaxed) {
        ocp_write_word(MCU_PLA, PLA_OCP_GPHY_BASE, base);
        OCP_BASE.store(base, Ordering::Relaxed);
    }
    ocp_write_word(MCU_PLA, (addr & 0x0fff) | 0xb000, data);
}
fn ocp_reg_read(addr: u16) -> u16 {
    let base = addr & 0xf000;
    if base != OCP_BASE.load(Ordering::Relaxed) {
        ocp_write_word(MCU_PLA, PLA_OCP_GPHY_BASE, base);
        OCP_BASE.store(base, Ordering::Relaxed);
    }
    ocp_read_word(MCU_PLA, (addr & 0x0fff) | 0xb000)
}
fn mdio_read(reg: u16) -> u16 { ocp_reg_read(OCP_BASE_MII + reg * 2) }
fn mdio_write(reg: u16, val: u16) { ocp_reg_write(OCP_BASE_MII + reg * 2, val) }
fn sram_write(addr: u16, data: u16) {
    ocp_reg_write(OCP_SRAM_ADDR, addr);
    ocp_reg_write(OCP_SRAM_DATA, data);
}

fn set_bits16(mcu: u16, reg: u16, bits: u16) { let v = ocp_read_word(mcu, reg); ocp_write_word(mcu, reg, v | bits); }
fn clr_bits16(mcu: u16, reg: u16, bits: u16) { let v = ocp_read_word(mcu, reg); ocp_write_word(mcu, reg, v & !bits); }
fn set_bits8(mcu: u16, reg: u16, bits: u8) { let v = ocp_read_byte(mcu, reg); ocp_write_byte(mcu, reg, v | bits); }
fn clr_bits8(mcu: u16, reg: u16, bits: u8) { let v = ocp_read_byte(mcu, reg); ocp_write_byte(mcu, reg, v & !bits); }
fn ocp_set_reg(reg: u16, bits: u16) { let v = ocp_reg_read(reg); ocp_reg_write(reg, v | bits); }
fn ocp_clr_reg(reg: u16, bits: u16) { let v = ocp_reg_read(reg); ocp_reg_write(reg, v & !bits); }

// ── Polling helpers ──

fn deadline(ms: u64) -> u64 { crate::interrupts::ticks() + (ms / 10).max(1) }
fn expired(d: u64) -> bool { crate::interrupts::ticks() >= d }

fn nic_reset() {
    ocp_write_byte(MCU_PLA, PLA_CR, CR_RST);
    let d = deadline(1000);
    while ocp_read_byte(MCU_PLA, PLA_CR) & CR_RST != 0 && !expired(d) { core::hint::spin_loop(); }
}
fn wait_link_list_ready() {
    let d = deadline(2000);
    while ocp_read_byte(MCU_PLA, PLA_OOB_CTRL) & LINK_LIST_READY == 0 && !expired(d) { core::hint::spin_loop(); }
}
fn wait_phy_status(want: u16) {
    let d = deadline(5000);
    loop {
        let s = ocp_reg_read(OCP_PHY_STATUS) & PHY_STAT_MASK;
        if want == 0 { if s == 3 || s == 5 || s == 2 { break; } }
        else if s == want { break; }
        if expired(d) { break; }
        core::hint::spin_loop();
    }
}
fn u1u2en(en: bool) {
    let v = if en { 0xffff_ffff } else { 0 };
    ocp_write_dword(MCU_USB, USB_TOLERANCE, v);
    ocp_write_dword(MCU_USB, USB_TOLERANCE + 4, v);
}
fn u2p3en(en: bool) {
    let v = ocp_read_word(MCU_USB, USB_U2P3_CTRL);
    ocp_write_word(MCU_USB, USB_U2P3_CTRL, if en { v | U2P3_ENABLE } else { v & !U2P3_ENABLE });
}
fn aldps_en(en: bool) {
    let v = ocp_reg_read(OCP_POWER_CFG);
    ocp_reg_write(OCP_POWER_CFG, if en { v | EN_ALDPS } else { v & !EN_ALDPS });
}
fn reset_bmu() {
    clr_bits8(MCU_USB, USB_BMU_RESET, BMU_RESET_EP_IN | BMU_RESET_EP_OUT);
    set_bits8(MCU_USB, USB_BMU_RESET, BMU_RESET_EP_IN | BMU_RESET_EP_OUT);
}

/// Read chip version: dword at PLA_TCR0 >> 16, mask 0x7cf0. RTL8153 = 0x5c00/10/20.
fn get_version() -> u16 { ((ocp_read_dword(MCU_PLA, PLA_TCR0) >> 16) & 0x7cf0) as u16 }

// ── Init (r8153_init + hw_phy_cfg + first_init + enable) ──

fn r8153_init(ver: u16) {
    u1u2en(false);
    let d = deadline(10000);
    while ocp_read_word(MCU_PLA, PLA_BOOT_CTRL) & AUTOLOAD_DONE == 0 && !expired(d) { core::hint::spin_loop(); }
    wait_phy_status(0);

    ocp_reg_write(OCP_ADC_CFG, ADC_CFG_INIT);

    let bmcr = mdio_read(MII_BMCR);
    if bmcr & BMCR_PDOWN != 0 { mdio_write(MII_BMCR, bmcr & !BMCR_PDOWN); }
    wait_phy_status(PHY_STAT_LAN_ON);

    u2p3en(false);

    // Per-version step (r8152.c r8153_init)
    match ver {
        0x5c10 => { // VER_04
            let v = ocp_read_word(MCU_USB, USB_SSPHYLINK2) & !0x3ffe;
            ocp_write_word(MCU_USB, USB_SSPHYLINK2, v | (96 << 1));
            set_bits8(MCU_USB, USB_USB2PHY, USB2PHY_L1 | USB2PHY_SUSPEND);
        }
        0x5c20 => { // VER_05
            clr_bits8(MCU_PLA, PLA_DMY_REG0, 0x02); // ECM_ALDPS
            let burst = ocp_read_word(MCU_USB, USB_BURST_SIZE);
            if burst == 0 { clr_bits8(MCU_USB, USB_CSR_DUMMY1, 0x01); }
            else { set_bits8(MCU_USB, USB_CSR_DUMMY1, 0x01); }
        }
        _ => {} // VER_03: none
    }

    set_bits16(MCU_USB, USB_CSR_DUMMY2, EP4_FULL_FC);
    clr_bits16(MCU_USB, USB_WDT11_CTRL, TIMER11_EN);
    clr_bits16(MCU_PLA, PLA_LED_FEATURE, LED_MODE_MASK);
    ocp_write_byte(MCU_USB, USB_LPM_CTRL, LPM_CTRL_VAL);
    let afe = ocp_read_word(MCU_USB, USB_AFE_CTRL2) & !SEN_VAL_MASK;
    ocp_write_word(MCU_USB, USB_AFE_CTRL2, afe | SEN_VAL_NORMAL | SEL_RXIDLE);
    ocp_write_word(MCU_USB, USB_CONNECT_TIMER, 0x0001);

    u1u2en(true);
    set_bits8(MCU_PLA, PLA_CONFIG6, LANWAKE_CLR_EN);
    clr_bits8(MCU_PLA, PLA_LWAKE_CTRL_REG, LANWAKE_PIN);
    clr_bits16(MCU_USB, USB_USB_CTRL, RX_AGG_DISABLE | RX_ZERO_EN);
}

/// Disable EEE (r8153_eee_en(false) + advertise no EEE). We skip the FW patch
/// that makes EEE's LPI wake reliable, so leaving EEE on (ROM default) drops
/// the head of each RX burst — disable it outright.
fn eee_disable() {
    clr_bits16(MCU_PLA, PLA_EEE_CR, EEE_RX_EN | EEE_TX_EN);
    ocp_clr_reg(OCP_EEE_CFG, EEE10_EN);  // PHY-window register
    ocp_reg_write(OCP_EEE_ADV, 0);       // advertise no EEE to the link partner
}

/// Advertise 802.3x PAUSE flow control (r8152b_enable_fc). Done before link-up
/// so the first auto-negotiation carries it.
fn enable_fc() {
    let anar = mdio_read(MII_ADVERTISE) | ADVERTISE_PAUSE_CAP | ADVERTISE_PAUSE_ASYM;
    mdio_write(MII_ADVERTISE, anar);
}

/// Restrict (or restore) the auto-negotiated Ethernet link speed and restart
/// auto-negotiation. `mbit`: 10 / 100 / 1000, or 0 = auto (all speeds).
///
/// On a USB High-Speed dongle a gigabit wire overruns the ~300 Mbit USB pipe →
/// the chip RX FIFO overflows (rx_missed) → TCP collapses. Forcing 100 Mbit
/// makes the wire slower than the USB drain so the FIFO never overflows: lower
/// peak, but zero loss and a rock-steady link. Always re-advertises PAUSE.
pub fn set_link_speed(mbit: u16) -> bool {
    if !is_available() { return false; }
    let mut anar = mdio_read(MII_ADVERTISE) & !ADVERTISE_SPEED_MASK;
    let mut gctrl = mdio_read(MII_CTRL1000) & !(ADVERTISE_1000FULL | ADVERTISE_1000HALF);
    match mbit {
        10 => anar |= ADVERTISE_10HALF | ADVERTISE_10FULL,
        100 => anar |= ADVERTISE_100HALF | ADVERTISE_100FULL,
        1000 => gctrl |= ADVERTISE_1000FULL,
        _ => { anar |= ADVERTISE_SPEED_MASK; gctrl |= ADVERTISE_1000FULL; }
    }
    anar |= ADVERTISE_PAUSE_CAP | ADVERTISE_PAUSE_ASYM;
    mdio_write(MII_ADVERTISE, anar);
    mdio_write(MII_CTRL1000, gctrl);
    mdio_write(MII_BMCR, BMCR_ANENABLE | BMCR_ANRESTART);
    crate::kprintln!("[npk] rtl8153: link speed set to {} — renegotiating...",
        if mbit == 0 { 0 } else { mbit });
    // Wait for the link to come back up, then reprogram speed-dependent MAC regs.
    let mut up = false;
    for _ in 0..50 {
        if mdio_read(MII_BMSR) & BMSR_LSTATUS != 0 { up = true; break; }
        crate::interrupts::delay_ms(100);
    }
    if up { on_link_up(); log_link_diag(); }
    crate::kprintln!("[npk] rtl8153: link {}", if up { "up" } else { "down" });
    up
}

fn r8153_hw_phy_cfg(ver: u16) {
    aldps_en(false);
    eee_disable();                       // before PHY param updates (Linux order)
    if ver == 0x5c00 { ocp_clr_reg(OCP_EEE_CFG, CTAP_SHORT_EN); } // VER_03
    ocp_set_reg(OCP_POWER_CFG, EEE_CLKDIV_EN);
    ocp_set_reg(OCP_DOWN_SPEED, EN_10M_BGOFF);
    ocp_set_reg(OCP_POWER_CFG, EN_10M_PLLOFF);
    sram_write(SRAM_IMPEDANCE, 0x0b13);
    set_bits16(MCU_PLA, PLA_PHY_PWR, PFM_PWM_SWITCH);
    sram_write(SRAM_LPF_CFG, 0xf70f);
    sram_write(SRAM_10M_AMP1, 0x00af);
    sram_write(SRAM_10M_AMP2, 0x0208);
    enable_fc();                         // advertise PAUSE before negotiation
    aldps_en(true);
    if ver == 0x5c20 { u2p3en(true); } // VER_05
}

/// Speed-dependent MAC config that must run AFTER auto-negotiation settles
/// (rtl_set_ifg + rtl_set_eee_plus). Linux re-runs the enable path on every
/// carrier-up; we call this once the link comes up in `init()`.
fn on_link_up() {
    let speed = ocp_read_word(MCU_PLA, PLA_PHYSTATUS);
    // rtl_set_ifg: inter-frame gap depends on speed/duplex.
    let mut tcr1 = ocp_read_word(MCU_PLA, PLA_TCR1) & !IFG_MASK;
    let slow_half = (speed & (SPD_10 | SPD_100)) != 0 && (speed & SPD_FULL) == 0;
    if slow_half {
        tcr1 |= IFG_144NS;
        ocp_write_word(MCU_PLA, PLA_TCR1, tcr1);
        clr_bits16(MCU_PLA, PLA_MAC_PWR_CTRL4, TX10MIDLE_EN);
    } else {
        tcr1 |= IFG_96NS;
        ocp_write_word(MCU_PLA, PLA_TCR1, tcr1);
        set_bits16(MCU_PLA, PLA_MAC_PWR_CTRL4, TX10MIDLE_EN);
    }
    // rtl_set_eee_plus: only the 10 Mbit path needs EEEP TX-idle.
    if speed & SPD_10 != 0 { set_bits16(MCU_PLA, PLA_EEEP_CR, EEEP_CR_EEEP_TX); }
    else { clr_bits16(MCU_PLA, PLA_EEEP_CR, EEEP_CR_EEEP_TX); }
}

fn teredo_off() {
    clr_bits16(MCU_PLA, PLA_TEREDO_CFG, 0x8000 | 0x00fe | 0x0001);
    ocp_write_word(MCU_PLA, PLA_WDT6_CTRL, 0x0010);
    ocp_write_word(MCU_PLA, PLA_REALWOW_TIMER, 0);
    ocp_write_dword(MCU_PLA, PLA_TEREDO_TIMER, 0);
}

fn first_init() {
    set_bits16(MCU_PLA, PLA_MISC_1, RXDY_GATED_EN);   // gate RX during init
    teredo_off();
    let rcr = ocp_read_dword(MCU_PLA, PLA_RCR) & !RCR_ACPT_ALL;
    ocp_write_dword(MCU_PLA, PLA_RCR, rcr);
    nic_reset();
    reset_bmu();
    clr_bits8(MCU_PLA, PLA_OOB_CTRL, NOW_IS_OOB);
    clr_bits16(MCU_PLA, PLA_SFF_STS_7, MCU_BORW_EN);
    wait_link_list_ready();
    set_bits16(MCU_PLA, PLA_SFF_STS_7, RE_INIT_LL);
    wait_link_list_ready();
    // mtu_to_size(L3 mtu) = mtu + VLAN_ETH_HLEN(18) + ETH_FCS_LEN(4). MTU here
    // is the L2 frame length (1514), so the L3 mtu is MTU-14 → RMS = 1522.
    ocp_write_word(MCU_PLA, PLA_RMS, (MTU as u16) - 14 + 22);
    ocp_write_byte(MCU_PLA, PLA_MTPS, MTPS_JUMBO);
    set_bits16(MCU_PLA, PLA_TCR0, TCR0_AUTO_FIFO);
    nic_reset();
    ocp_write_dword(MCU_PLA, PLA_RXFIFO_CTRL0, RXFIFO_THR1_NORMAL);
    ocp_write_word(MCU_PLA, PLA_RXFIFO_CTRL1, RXFIFO_THR2_NORMAL);
    ocp_write_word(MCU_PLA, PLA_RXFIFO_CTRL2, RXFIFO_THR3_NORMAL);
    ocp_write_dword(MCU_PLA, PLA_TXFIFO_CTRL, TXFIFO_THR_NORMAL2);
}

fn read_mac() -> [u8; 6] {
    let mut b = [0u8; 8];
    get_regs(PLA_BACKUP, MCU_PLA, &mut b);
    [b[0], b[1], b[2], b[3], b[4], b[5]]
}
fn set_mac(mac: &[u8; 6]) {
    ocp_write_byte(MCU_PLA, PLA_CRWECR, CRWECR_CONFIG);
    ocp_write_dword(MCU_PLA, PLA_IDR, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));
    ocp_write_word(MCU_PLA, PLA_IDR + 4, u16::from_le_bytes([mac[4], mac[5]]));
    ocp_write_byte(MCU_PLA, PLA_CRWECR, CRWECR_NORMAL);
}

/// RX aggregation tuning (r8152 RTL_VER_03/04/05). Tells the chip how long and
/// how full to coalesce received packets into one bulk-IN buffer before flushing
/// it. Without this the chip uses power-on defaults that flush tiny aggregates →
/// one short USB transfer per packet → throughput collapses to a few Mbit.
fn set_rx_early() {
    let coalesce = match crate::xhci::nic_speed_class() {
        2 => COALESCE_SUPER,
        1 => COALESCE_HIGH,
        _ => COALESCE_SLOW,
    };
    ocp_write_word(MCU_USB, USB_RX_EARLY_TIMEOUT, (coalesce / 8) as u16);

    // ocp_data = rx_buf_sz - rx_reserved_size(mtu), VER_03/04/05 writes it /4.
    // rx_reserved_size = mtu_to_size(L3 mtu) + sizeof(rx_desc) + RX_ALIGN
    //                  = ((MTU-14) + 22) + 24 + 8
    let reserved = (MTU as u32 - 14) + 22 + RX_DESC_LEN as u32 + 8;
    let ocp_data = (RX_BATCH_BYTES as u32).saturating_sub(reserved);
    ocp_write_word(MCU_USB, USB_RX_EARLY_SIZE, (ocp_data / 4) as u16);
}

fn enable() {
    // Configure RX aggregation before turning the receiver on.
    set_rx_early();
    // reset packet filter
    clr_bits16(MCU_PLA, PLA_FMC, FMC_FCR_MCU_EN);
    set_bits16(MCU_PLA, PLA_FMC, FMC_FCR_MCU_EN);
    // receiver + transmitter on
    set_bits8(MCU_PLA, PLA_CR, CR_RE | CR_TE);
    // ungate RX
    clr_bits16(MCU_PLA, PLA_MISC_1, RXDY_GATED_EN);
    // accept broadcast + perfect-match (unicast to our MAC)
    let rcr = (ocp_read_dword(MCU_PLA, PLA_RCR) & !RCR_ACPT_ALL) | RCR_AB | RCR_APM | RCR_AM;
    ocp_write_dword(MCU_PLA, PLA_RCR, rcr);
}

/// Probe + bring up the RTL8153 dongle. Returns true if a NIC is live.
pub fn init() -> bool {
    if !crate::xhci::nic_attach(VID, PID, EP_IN, EP_OUT) { return false; }

    let ver = get_version();
    crate::kprintln!("[npk] rtl8153: version {:#06x}", ver);
    if !(ver == 0x5c00 || ver == 0x5c10 || ver == 0x5c20) {
        crate::kprintln!("[npk] rtl8153: unsupported version {:#06x} (expected RTL8153 0x5c00/10/20)", ver);
        // Try anyway with the VER_03 path — most 8153 variants share it.
    }

    r8153_init(ver);
    r8153_hw_phy_cfg(ver);

    // up: u1u2/u2p3/aldps down → first_init → settle → aldps up
    u1u2en(false);
    u2p3en(false);
    aldps_en(false);
    first_init();
    set_bits8(MCU_PLA, PLA_CONFIG6, LANWAKE_CLR_EN);
    clr_bits8(MCU_PLA, PLA_LWAKE_CTRL_REG, LANWAKE_PIN);
    aldps_en(true);
    if ver == 0x5c20 { u2p3en(true); }
    u1u2en(true);

    let mac = read_mac();
    set_mac(&mac);
    *MAC.lock() = mac;

    enable();

    AVAILABLE.store(true, Ordering::Release);
    crate::kprintln!("[npk] rtl8153: up — MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Wait for carrier (gigabit auto-negotiation takes ~2-3s) before the
    // caller runs DHCP, which would otherwise fire into a dead link.
    let mut up = false;
    for _ in 0..40 {
        if mdio_read(MII_BMSR) & BMSR_LSTATUS != 0 { up = true; break; }
        crate::interrupts::delay_ms(100);
    }
    if up {
        // Auto-negotiation settled — program the speed-dependent MAC regs
        // (IFG, EEEP) now that PLA_PHYSTATUS reflects the real link.
        on_link_up();
        log_link_diag();
    }
    crate::kprintln!("[npk] rtl8153: link {}", if up { "up" } else { "down (no cable?)" });
    true
}

pub fn is_available() -> bool { AVAILABLE.load(Ordering::Acquire) }

// recv() batch-parse instrumentation: catches the case where OUR rx_desc
// walker discards received bytes (a wrong bulk-IN length from the ring, or a
// bogus desc) — which would show up as TCP out-of-order/loss.
static RX_FRAMES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RX_TRUNC: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RX_DISCARD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// (frames_delivered, truncated_batches, discarded_bytes), each reset to 0.
pub fn take_rx_parse_stats() -> (u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (RX_FRAMES.swap(0, Relaxed), RX_TRUNC.swap(0, Relaxed), RX_DISCARD.swap(0, Relaxed))
}

/// Zero the chip's hardware tally counters (PLA_RSTTALLY |= TALLY_RESET) so a
/// following download measures only its own RX FIFO overflow / error counts.
pub fn tally_reset() {
    if !is_available() { return; }
    set_bits16(MCU_PLA, PLA_RSTTALLY, TALLY_RESET);
}

/// Read + print the chip's hardware tally counters. `rx_missed` is the RX FIFO
/// overflow drop count straight from the silicon — the definitive answer to
/// whether the gigabit-wire→USB2-pipe mismatch is overflowing the chip (i.e.
/// flow control is not throttling the switch) vs. loss happening elsewhere.
pub fn dump_tally(label: &str) {
    if !is_available() { return; }
    let mut b = [0u8; 64];
    if !get_regs(PLA_TALLYCNT, MCU_PLA, &mut b) {
        crate::kprintln!("[npk] rtl8153 tally[{}]: read failed", label);
        return;
    }
    let rx_packets = u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
    let rx_errors = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let rx_missed = u16::from_le_bytes([b[28], b[29]]);     // RX FIFO overflow drops
    let align_errors = u16::from_le_bytes([b[30], b[31]]);
    let tx_underrun = u16::from_le_bytes([b[62], b[63]]);
    crate::kprintln!(
        "[npk] rtl8153 tally[{}]: rx_pkts={} rx_missed(FIFO-overflow)={} rx_errors={} align={} tx_underrun={}",
        label, rx_packets, rx_missed, rx_errors, align_errors, tx_underrun);
}

/// Dump the PHY link registers raw + derived speed. PLA_PHYSTATUS is the chip's
/// own speed view; BMSR/LPA/STAT1000 show what auto-negotiation settled on with
/// the link partner — so a 10/100 or half-duplex link (the prime suspect for a
/// dongle stuck at single-digit Mbit) is visible directly.
pub fn log_link_diag() {
    if !is_available() { return; }
    let physt = ocp_read_word(MCU_PLA, PLA_PHYSTATUS);
    let bmsr = mdio_read(MII_BMSR);
    let lpa = mdio_read(0x05);       // link-partner ability (10/100)
    let stat1000 = mdio_read(0x0a);  // 1000BASE-T status
    let mbit = if physt & 0x10 != 0 { 1000 } else if physt & 0x08 != 0 { 100 }
               else if physt & 0x04 != 0 { 10 } else { 0 };
    crate::kprintln!("[npk] rtl8153 link: {}Mbit {} | PHYSTATUS={:#06x} BMSR={:#06x} LPA={:#06x} STAT1000={:#06x}",
        mbit, if physt & 1 != 0 { "full-dup" } else { "half-dup" }, physt, bmsr, lpa, stat1000);
}
pub fn mac() -> Option<[u8; 6]> { if is_available() { Some(*MAC.lock()) } else { None } }

/// Live carrier state: reads the PHY's BMSR link bit over USB, so a pulled or
/// plugged cable is reflected right now (a yanked cable drops the carrier even
/// though the USB device stays present). Does a USB control transfer — call
/// from Core 0 at a low rate, never from an interrupt handler.
pub fn link_up() -> bool {
    if !AVAILABLE.load(Ordering::Acquire) { return false; }
    mdio_read(MII_BMSR) & BMSR_LSTATUS != 0
}

/// Transmit one Ethernet frame: prepend an 8-byte tx_desc, send on bulk OUT.
/// The chip appends the FCS itself.
pub fn send(frame: &[u8]) -> Result<(), NetError> {
    if !is_available() { return Err(NetError::NotInitialized); }
    let len = frame.len().min(MTU);
    let mut tx = TX.lock();
    let opts1 = (len as u32) | TX_FS | TX_LS;
    tx[0..4].copy_from_slice(&opts1.to_le_bytes());
    tx[4..8].copy_from_slice(&0u32.to_le_bytes());
    tx[TX_DESC_LEN..TX_DESC_LEN + len].copy_from_slice(&frame[..len]);
    if crate::xhci::nic_bulk_out(&tx[..TX_DESC_LEN + len]) { Ok(()) } else { Err(NetError::QueueFull) }
}

/// Receive one Ethernet frame. The chip aggregates multiple rx_desc-prefixed
/// packets per bulk-IN buffer; we walk the current batch and pull a new one
/// when it's exhausted. Returns None when nothing is available.
pub fn recv(out: &mut [u8; MTU]) -> Option<usize> {
    if !is_available() { return None; }
    let mut rx = RX.lock();
    loop {
        // Extract the next frame from the current batch.
        while rx.cursor + RX_DESC_LEN <= rx.valid {
            let c = rx.cursor;
            let opts1 = u32::from_le_bytes([rx.buf[c], rx.buf[c + 1], rx.buf[c + 2], rx.buf[c + 3]]);
            let pkt_len = (opts1 & RX_LEN_MASK) as usize;        // incl. 4-byte FCS
            use core::sync::atomic::Ordering::Relaxed;
            if pkt_len < 60 {
                // Normal aggregation terminator — but if >64 real-looking bytes
                // remain, we just threw away packets (a bad length upstream).
                let rem = rx.valid - c;
                if rem > 64 { RX_DISCARD.fetch_add(rem as u64, Relaxed); }
                rx.cursor = rx.valid; break;
            }
            if c + RX_DESC_LEN + pkt_len > rx.valid {            // truncated last packet
                RX_TRUNC.fetch_add(1, Relaxed);
                RX_DISCARD.fetch_add((rx.valid - c) as u64, Relaxed);
                rx.cursor = rx.valid; break;
            }
            let frame_len = pkt_len - ETH_FCS_LEN;
            let n = frame_len.min(MTU);
            let start = c + RX_DESC_LEN;
            out[..n].copy_from_slice(&rx.buf[start..start + n]);
            rx.cursor = (c + RX_DESC_LEN + pkt_len + 7) & !7;    // 8-byte aligned stride
            RX_FRAMES.fetch_add(1, Relaxed);
            return Some(n);
        }
        // Batch exhausted — pull a new one (non-blocking) straight into the
        // batch buffer (no large stack temporary).
        let got = crate::xhci::nic_bulk_in(&mut rx.buf);
        if got == 0 { rx.valid = 0; rx.cursor = 0; return None; }
        rx.valid = got;
        rx.cursor = 0;
    }
}
