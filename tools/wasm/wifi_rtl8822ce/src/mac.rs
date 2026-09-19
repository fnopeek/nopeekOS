//! `mac.c` aus Linux 6.18.26 rtw88 — Stufe 1: der Strom.
//!
//! Portiert sind, in Aufrufreihenfolge und vollstaendig:
//! `rtw_mac_pre_system_cfg` · `do_pwr_poll_cmd` · `rtw_pwr_cmd_polling` ·
//! `rtw_sub_pwr_seq_parser` · `rtw_pwr_seq_parser` · `rtw_mac_power_switch` ·
//! `__rtw_mac_init_system_cfg` · `rtw_mac_init_system_cfg` ·
//! `rtw_mac_power_on` · `rtw_mac_power_off`.
//!
//! **Der 8822C ist WCPU_3081, nicht 8051.** Jede `rtw_chip_wcpu_8051()`-Abzweigung
//! ist damit statisch falsch — sie steht trotzdem als Konstante da, damit beim
//! Lesen sichtbar bleibt, dass Linux dort einen zweiten Weg hat und welcher
//! Zweig hier gilt.

use crate::host;
use crate::pwrseq::*;
use crate::regs::*;

/// rtw8822c.c `rtw8822c_hw_spec.wlan_cpu = RTW_WCPU_3081`.
const WCPU_8051: bool = false;

/// Wir sind PCIe. `rtw_hci_type()` ist bei uns eine Konstante.
const INTF_MASK: u8 = RTW_PWR_INTF_PCI_MSK;

pub enum PwrErr {
    /// Linux: `-EALREADY` — der Chip ist schon in dem Zustand, den wir wollen.
    Already,
    /// Linux: `-EBUSY` — ein Polling-Kommando ist ausgelaufen.
    Busy,
}

/// mac.c `rtw_mac_pre_system_cfg`, PCIe-Zweig.
pub fn pre_system_cfg(h: i32) {
    host::w8(h, REG_RSV_CTRL, 0);

    if WCPU_8051 {
        // Linux setzt hier REG_LDO_SWR_CTRL nach BIT_LDO und kehrt SOFORT
        // zurueck — der ganze Rest dieser Funktion gilt nur fuer die
        // 3081-Familie. Fuer den 8822C unerreichbar.
        return;
    }

    // PCIe
    host::set32(h, REG_HCI_OPT_CTRL, BIT_USB_SUS_DIS);

    // config PIN Mux
    let mut v = host::r32(h, REG_PAD_CTRL1);
    v |= BIT_PAPE_WLBT_SEL | BIT_LNAON_WLBT_SEL;
    host::w32(h, REG_PAD_CTRL1, v);

    let mut v = host::r32(h, REG_LED_CFG);
    v &= !(BIT_PAPE_SEL_EN | BIT_LNAON_SEL_EN);
    host::w32(h, REG_LED_CFG, v);

    let mut v = host::r32(h, REG_GPIO_MUXCFG);
    v |= BIT_WLRFE_4_5_EN;
    host::w32(h, REG_GPIO_MUXCFG, v);

    // disable BB/RF
    let mut v8 = host::r8(h, REG_SYS_FUNC_EN);
    v8 &= !(BIT_FEN_BB_RSTB | BIT_FEN_BB_GLB_RST);
    host::w8(h, REG_SYS_FUNC_EN, v8);

    let mut v8 = host::r8(h, REG_RF_CTRL);
    v8 &= !(BIT_RF_SDM_RSTB | BIT_RF_RSTB | BIT_RF_EN);
    host::w8(h, REG_RF_CTRL, v8);

    let mut v = host::r32(h, REG_WLRF1);
    v &= !BIT_WLRF1_BBRF_EN;
    host::w32(h, REG_WLRF1, v);
}

/// mac.c `do_pwr_poll_cmd`. Linux pollt alle 50 us bis
/// `50 * RTW_PWR_POLLING_CNT` us = **1 s**.
///
/// Wir haben keinen 50-us-Schlaf — `npk_sleep` rastert in Millisekunden.
/// Also wird eng gelesen und die FRIST an `now_us()` gehalten: dieselbe
/// Gesamtfrist wie Linux, nur ohne den Takt dazwischen. Ein Deckel auf die
/// Runden gibt es bewusst nicht; die Frist ist die Frist
/// ([[feedback_a_cap_set_from_a_guess_is_below_the_normal_case]]).
fn do_pwr_poll_cmd(h: i32, addr: u32, mask: u8, target: u8) -> bool {
    let target = target & mask;
    let start = host::now_us();
    let deadline = start + 50 * RTW_PWR_POLLING_CNT as u64;
    // Eng lesen, solange der Normalfall dauert (Linux pollt alle 50 us und
    // ist meist nach wenigen Runden durch). Danach wird zwischen den Lesungen
    // abgegeben: drei Polling-Kommandos gelten auf PCIe, und drei Fristen
    // zu je einer Sekunde sind sechs Sekunden, in denen sonst niemand auf
    // diesem Kern drankaeme.
    const TIGHT_US: u64 = 2000;
    loop {
        if host::r8(h, addr) & mask == target {
            return true;
        }
        let now = host::now_us();
        if now >= deadline {
            return false;
        }
        if now - start > TIGHT_US {
            host::sleep_ms(1);
        }
    }
}

/// mac.c `rtw_pwr_cmd_polling` — samt dem PCIe-Sonderweg: laeuft das Polling
/// aus, wird `BIT_PFM_WOWL` getoggelt und EINMAL neu gepollt. Ohne diesen
/// zweiten Versuch schlaegt die Sequenz auf manchen Boards beim ersten
/// Kaltstart fehl.
fn pwr_cmd_polling(h: i32, cmd: &PwrCmd) -> Result<(), PwrErr> {
    // `base == RTW_PWR_ADDR_SDIO` haengt in Linux SDIO_LOCAL_OFFSET an. Jede
    // solche Zeile traegt intf_mask SDIO und wird eine Ebene hoeher schon
    // aussortiert — auf PCIe ist der Fall unerreichbar.
    let offset = cmd.offset as u32;

    if do_pwr_poll_cmd(h, offset, cmd.mask, cmd.value) {
        return Ok(());
    }

    // PCIe: BIT_PFM_WOWL toggeln und noch einmal.
    let value = host::r8(h, REG_SYS_PW_CTRL);
    host::w8(h, REG_SYS_PW_CTRL, value | BIT_PFM_WOWL);
    host::w8(h, REG_SYS_PW_CTRL, value & !BIT_PFM_WOWL);

    if do_pwr_poll_cmd(h, offset, cmd.mask, cmd.value) {
        return Ok(());
    }

    host::print("[rtl8822ce] Polling ausgelaufen: offset=0x");
    host::print_hex16(cmd.offset);
    host::print(" mask=0x");
    host::print_hex8(cmd.mask);
    host::print(" value=0x");
    host::print_hex8(cmd.value);
    host::print("\n");
    Err(PwrErr::Busy)
}

/// mac.c `rtw_sub_pwr_seq_parser`
fn sub_pwr_seq_parser(h: i32, cut_mask: u8, seq: &[PwrCmd]) -> Result<(), PwrErr> {
    for cmd in seq {
        if cmd.cmd == RTW_PWR_CMD_END {
            break;
        }
        if cmd.intf_mask & INTF_MASK == 0 || cmd.cut_mask & cut_mask == 0 {
            continue;
        }
        match cmd.cmd {
            RTW_PWR_CMD_WRITE => {
                let offset = cmd.offset as u32;
                let mut value = host::r8(h, offset);
                value &= !cmd.mask;
                value |= cmd.value & cmd.mask;
                host::w8(h, offset, value);
            }
            RTW_PWR_CMD_POLLING => pwr_cmd_polling(h, cmd)?,
            RTW_PWR_CMD_DELAY => {
                // Die vier 8822C-Sequenzen enthalten KEIN DELAY (ausgezaehlt:
                // 46 WRITE, 4 POLLING, 4 END). Der Zweig steht trotzdem hier,
                // weil er in `rtw_sub_pwr_seq_parser` steht.
                if cmd.value == RTW_PWR_DELAY_US {
                    // Unter unserer Aufloesung; eine Millisekunde ist die
                    // kleinste Pause, die wir ehrlich machen koennen.
                    host::sleep_ms(1);
                } else {
                    host::sleep_ms(cmd.offset as u32);
                }
            }
            RTW_PWR_CMD_READ => {}
            _ => return Err(PwrErr::Busy),
        }
    }
    Ok(())
}

/// mac.c `rtw_pwr_seq_parser`
pub fn pwr_seq_parser(h: i32, cut_version: u8, flow: &[&[PwrCmd]]) -> Result<(), PwrErr> {
    let cut_mask = cut_version_to_mask(cut_version);
    for seq in flow {
        sub_pwr_seq_parser(h, cut_mask, seq)?;
    }
    Ok(())
}

/// mac.c `rtw_mac_power_switch`
pub fn mac_power_switch(h: i32, cut_version: u8, pwr_on: bool) -> Result<(), PwrErr> {
    // rtw_chip_wcpu_3081 gilt fuer den 8822C.
    if !WCPU_8051 {
        let rpwm = host::r8(h, PCIE_RPWM_ADDR);
        // Laeuft noch Firmware? Dann den RPWM-Umschalter kippen, damit sie
        // den Wechsel mitbekommt.
        if host::r16(h, REG_MCUFW_CTRL) == MCUFW_CTRL_FW_ALIVE {
            let rpwm = (rpwm ^ BIT_RPWM_TOGGLE) & BIT_RPWM_TOGGLE;
            host::w8(h, PCIE_RPWM_ADDR, rpwm);
        }
    }

    let cur_pwr = host::r8(h, REG_CR) != CR_POWER_OFF;

    if pwr_on == cur_pwr {
        return Err(PwrErr::Already);
    }

    let flow: &[&[PwrCmd]] = if pwr_on {
        &CARD_ENABLE_FLOW
    } else {
        &CARD_DISABLE_FLOW
    };
    pwr_seq_parser(h, cut_version, flow)
}

/// mac.c `__rtw_mac_init_system_cfg` (der 3081-Weg).
fn init_system_cfg(h: i32) {
    if WCPU_8051 {
        return; // `__rtw_mac_init_system_cfg_legacy`, hier unerreichbar
    }

    let mut value = host::r32(h, REG_CPU_DMEM_CON);
    value |= BIT_WL_PLATFORM_RST | BIT_DDMA_EN;
    host::w32(h, REG_CPU_DMEM_CON, value);

    host::set8(h, REG_SYS_FUNC_EN + 1, SYS_FUNC_EN_8822C);
    let value8 = (host::r8(h, REG_CR_EXT + 3) & 0xF0) | 0x0C;
    host::w8(h, REG_CR_EXT + 3, value8);

    // disable boot-from-flash for driver's DL FW
    let tmp = host::r32(h, REG_MCUFW_CTRL);
    if tmp & BIT_BOOT_FSPI_EN != 0 {
        host::w32(h, REG_MCUFW_CTRL, tmp & !BIT_BOOT_FSPI_EN);
        let value = host::r32(h, REG_GPIO_MUXCFG) & !BIT_FSPI_EN;
        host::w32(h, REG_GPIO_MUXCFG, value);
    }
}

/// mac.c `rtw_mac_power_on`.
///
/// Der `-EALREADY`-Rueckfall ist kein Sonderfall: nach einem Warmstart steht
/// der Chip noch an, und dann ist AUS-dann-AN der normale Weg.
pub fn mac_power_on(h: i32, cut_version: u8) -> Result<(), PwrErr> {
    pre_system_cfg(h);

    match mac_power_switch(h, cut_version, true) {
        Ok(()) => {}
        Err(PwrErr::Already) => {
            host::print("[rtl8822ce] Chip war schon an — aus und wieder an\n");
            let _ = mac_power_switch(h, cut_version, false);
            pre_system_cfg(h);
            mac_power_switch(h, cut_version, true)?;
        }
        Err(e) => return Err(e),
    }

    init_system_cfg(h);
    Ok(())
}

/// mac.c `rtw_mac_power_off`
pub fn mac_power_off(h: i32, cut_version: u8) {
    let _ = mac_power_switch(h, cut_version, false);
}

// ── Stufe 2b: der Firmware-Download (mac.c) ──────────────────────

use crate::fw::{check_hw_ready, write_data_rsvd_page};
use crate::pci::Trx;

/// `struct rtw_backup_info` (main.h)
#[derive(Clone, Copy, Default)]
struct Backup {
    len: u8,
    reg: u32,
    val: u32,
}

/// mac.c `DLFW_RESTORE_REG_NUM`
const DLFW_RESTORE_REG_NUM: usize = 6;

/// util.c `rtw_restore_reg`. mac.c `download_firmware_reg_restore` ist ein
/// Einzeiler darum herum und faellt deshalb hier mit hinein.
fn restore_reg(h: i32, bckp: &[Backup]) {
    for b in bckp {
        match b.len {
            1 => host::w8(h, b.reg, b.val as u8),
            2 => host::w16(h, b.reg, b.val as u16),
            4 => host::w32(h, b.reg, b.val),
            _ => {}
        }
    }
}

/// util.c `ltecoex_read_reg`
fn ltecoex_read_reg(h: i32, offset: u16) -> Option<u32> {
    if !check_hw_ready(h, LTECOEX_ACCESS_CTRL, LTECOEX_READY, 1) {
        return None;
    }
    host::w32(h, LTECOEX_ACCESS_CTRL, 0x800F_0000 | offset as u32);
    Some(host::r32(h, LTECOEX_READ_DATA))
}

/// util.c `ltecoex_reg_write`
fn ltecoex_reg_write(h: i32, offset: u16, value: u32) -> bool {
    if !check_hw_ready(h, LTECOEX_ACCESS_CTRL, LTECOEX_READY, 1) {
        return false;
    }
    host::w32(h, LTECOEX_WRITE_DATA, value);
    host::w32(h, LTECOEX_ACCESS_CTRL, 0xC00F_0000 | offset as u32);
    true
}

/// mac.c `wlan_cpu_enable`
fn wlan_cpu_enable(h: i32, enable: bool) {
    if enable {
        host::set8(h, REG_RSV_CTRL + 1, BIT_WLMCU_IOIF);
        host::set8(h, REG_SYS_FUNC_EN + 1, BIT_FEN_CPUEN);
    } else {
        host::clr8(h, REG_SYS_FUNC_EN + 1, BIT_FEN_CPUEN);
        host::clr8(h, REG_RSV_CTRL + 1, BIT_WLMCU_IOIF);
    }
}

/// Felder aus `struct rtw_fw_hdr` (fw.h:20-40), alle little-endian.
/// `h2c_fmt_ver` liest heute niemand — es entscheidet ab 2c, welches
/// H2C-Format gilt, und gehoert deshalb schon hier hin.
#[allow(dead_code)]
pub struct FwHdr {
    pub version: u16,
    pub sub_version: u8,
    pub sub_index: u8,
    pub feature: u32,
    pub h2c_fmt_ver: u16,
    pub mem_usage: u8,
    pub dmem_addr: u32,
    pub dmem_size: u32,
    pub imem_size: u32,
    pub emem_size: u32,
    pub emem_addr: u32,
    pub imem_addr: u32,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub fn parse_fw_hdr(d: &[u8]) -> FwHdr {
    FwHdr {
        version: le16(d, 0x04),
        sub_version: d[0x06],
        sub_index: d[0x07],
        feature: le32(d, 0x0C),
        h2c_fmt_ver: le16(d, 0x1C),
        dmem_addr: le32(d, 0x20),
        dmem_size: le32(d, 0x24),
        mem_usage: d[0x18],
        imem_size: le32(d, 0x30),
        emem_size: le32(d, 0x34),
        emem_addr: le32(d, 0x38),
        imem_addr: le32(d, 0x3C),
    }
}

/// mac.c `check_firmware_size`
pub fn check_firmware_size(d: &[u8]) -> bool {
    if d.len() < FW_HDR_SIZE {
        return false;
    }
    let hdr = parse_fw_hdr(d);
    let dmem = hdr.dmem_size + FW_HDR_CHKSUM_SIZE;
    let imem = hdr.imem_size + FW_HDR_CHKSUM_SIZE;
    let emem = if hdr.mem_usage & (1 << 4) != 0 {
        hdr.emem_size + FW_HDR_CHKSUM_SIZE
    } else {
        0
    };
    FW_HDR_SIZE as u32 + dmem + imem + emem == d.len() as u32
}

/// mac.c `download_firmware_reg_backup`
fn download_firmware_reg_backup(h: i32) -> [Backup; DLFW_RESTORE_REG_NUM] {
    let mut b = [Backup::default(); DLFW_RESTORE_REG_NUM];
    let mut i = 0;

    // set HIQ to hi priority
    b[i] = Backup { len: 1, reg: REG_TXDMA_PQ_MAP + 1,
                    val: host::r8(h, REG_TXDMA_PQ_MAP + 1) as u32 };
    i += 1;
    host::w8(h, REG_TXDMA_PQ_MAP + 1, RTW_DMA_MAPPING_HIGH << 6);

    // DLFW only use HIQ, map HIQ to hi priority
    b[i] = Backup { len: 1, reg: REG_CR, val: host::r8(h, REG_CR) as u32 };
    i += 1;
    b[i] = Backup { len: 4, reg: REG_H2CQ_CSR, val: BIT_H2CQ_FULL };
    i += 1;
    host::w8(h, REG_CR, BIT_HCI_TXDMA_EN | BIT_TXDMA_EN);
    host::w32(h, REG_H2CQ_CSR, BIT_H2CQ_FULL);

    // Config hi priority queue and public priority queue page number
    b[i] = Backup { len: 2, reg: REG_FIFOPAGE_INFO_1,
                    val: host::r16(h, REG_FIFOPAGE_INFO_1) as u32 };
    i += 1;
    b[i] = Backup { len: 4, reg: REG_RQPN_CTRL_2,
                    val: host::r32(h, REG_RQPN_CTRL_2) | BIT_LD_RQPN };
    i += 1;
    host::w16(h, REG_FIFOPAGE_INFO_1, 0x200);
    host::w32(h, REG_RQPN_CTRL_2, b[i - 1].val);

    // Disable beacon related functions
    let tmp = host::r8(h, REG_BCN_CTRL);
    b[i] = Backup { len: 1, reg: REG_BCN_CTRL, val: tmp as u32 };
    i += 1;
    host::w8(h, REG_BCN_CTRL, (tmp & !BIT_EN_BCN_FUNCTION) | BIT_DIS_TSF_UDT);

    debug_assert!(i == DLFW_RESTORE_REG_NUM);
    b
}

/// mac.c `download_firmware_reset_platform`
fn download_firmware_reset_platform(h: i32) {
    host::clr8(h, REG_CPU_DMEM_CON + 2, (BIT_WL_PLATFORM_RST >> 16) as u8);
    host::clr8(h, REG_SYS_CLK_CTRL + 1, (BIT_CPU_CLK_EN >> 8) as u8);
    host::set8(h, REG_CPU_DMEM_CON + 2, (BIT_WL_PLATFORM_RST >> 16) as u8);
    host::set8(h, REG_SYS_CLK_CTRL + 1, (BIT_CPU_CLK_EN >> 8) as u8);
}

/// mac.c `iddma_enable`
fn iddma_enable(h: i32, src: u32, dst: u32, ctrl: u32) -> bool {
    host::w32(h, REG_DDMA_CH0SA, src);
    host::w32(h, REG_DDMA_CH0DA, dst);
    host::w32(h, REG_DDMA_CH0CTRL, ctrl);
    check_hw_ready(h, REG_DDMA_CH0CTRL, BIT_DDMACH0_OWN, 0)
}

/// mac.c `iddma_download_firmware`
fn iddma_download_firmware(h: i32, src: u32, dst: u32, len: u32, first: bool) -> bool {
    let mut ch0_ctrl = BIT_DDMACH0_CHKSUM_EN | BIT_DDMACH0_OWN;
    if !check_hw_ready(h, REG_DDMA_CH0CTRL, BIT_DDMACH0_OWN, 0) {
        return false;
    }
    ch0_ctrl |= len & BIT_MASK_DDMACH0_DLEN;
    if !first {
        ch0_ctrl |= BIT_DDMACH0_CHKSUM_CONT;
    }
    iddma_enable(h, src, dst, ch0_ctrl)
}

/// mac.c `check_fw_checksum`
fn check_fw_checksum(h: i32, addr: u32) -> bool {
    let fw_ctrl = host::r8(h, REG_MCUFW_CTRL) as u32;

    if host::r32(h, REG_DDMA_CH0CTRL) & BIT_DDMACH0_CHKSUM_STS != 0 {
        let v = if addr < OCPBASE_DMEM_88XX {
            (fw_ctrl | BIT_IMEM_DW_OK) & !BIT_IMEM_CHKSUM_OK
        } else {
            (fw_ctrl | BIT_DMEM_DW_OK) & !BIT_DMEM_CHKSUM_OK
        };
        host::w8(h, REG_MCUFW_CTRL, v as u8);
        host::print("[rtl8822ce] invalid fw checksum\n");
        return false;
    }

    let v = if addr < OCPBASE_DMEM_88XX {
        fw_ctrl | BIT_IMEM_DW_OK | BIT_IMEM_CHKSUM_OK
    } else {
        fw_ctrl | BIT_DMEM_DW_OK | BIT_DMEM_CHKSUM_OK
    };
    host::w8(h, REG_MCUFW_CTRL, v as u8);
    true
}

/// mac.c `download_firmware_to_mem`
#[allow(clippy::too_many_arguments)]
fn download_firmware_to_mem(
    h: i32, trx: &Trx, stage: i32, data: &[u8], src: u32, dst: u32, size: u32,
    band: u8,
) -> bool {
    const MAX_SIZE: u32 = 0x1000;
    let desc_size = crate::tx::TX_PKT_DESC_SZ as u32;

    let mut mem_offset = 0u32;
    let mut first_part = true;
    let mut residue = size;

    host::set32(h, REG_DDMA_CH0CTRL, BIT_DDMACH0_RESET_CHKSUM_STS);

    while residue > 0 {
        let pkt_size = residue.min(MAX_SIZE);
        let from = mem_offset as usize;
        let to = from + pkt_size as usize;
        if to > data.len() {
            host::print("[rtl8822ce] FW-Stueck liegt ausserhalb des Blobs\n");
            return false;
        }

        // mac.c `send_firmware_pkt` -> `send_firmware_pkt_rsvd_page`:
        // pg_addr = src >> 7. Der USB-Sonderfall (+1 Byte, wenn
        // (size + TX_DESC_SIZE) auf 512 aufgeht) gilt nur dort, und
        // `kmemdup` daneben ist Linux-Speicherverwaltung.
        if !write_data_rsvd_page(h, trx, stage, (src >> 7) as u16,
                                 &data[from..to], 0, band) {
            host::print("[rtl8822ce] rsvd page fehlgeschlagen bei Offset ");
            host::print_dec(mem_offset);
            host::print("\n");
            return false;
        }

        if !iddma_download_firmware(h, OCPBASE_TXBUF_88XX + src + desc_size,
                                    dst + mem_offset, pkt_size, first_part) {
            host::print("[rtl8822ce] iddma fehlgeschlagen bei Offset ");
            host::print_dec(mem_offset);
            host::print("\n");
            return false;
        }

        first_part = false;
        mem_offset += pkt_size;
        residue -= pkt_size;
    }

    check_fw_checksum(h, dst)
}

/// mac.c `start_download_firmware`
fn start_download_firmware(h: i32, trx: &Trx, stage: i32, fw: &[u8], band: u8) -> bool {
    let hdr = parse_fw_hdr(fw);
    let dmem_size = hdr.dmem_size + FW_HDR_CHKSUM_SIZE;
    let imem_size = hdr.imem_size + FW_HDR_CHKSUM_SIZE;
    let emem_size = if hdr.mem_usage & (1 << 4) != 0 {
        hdr.emem_size + FW_HDR_CHKSUM_SIZE
    } else {
        0
    };

    let val = (host::r16(h, REG_MCUFW_CTRL) & 0x3800) | BIT_MCUFWDL_EN as u16;
    host::w16(h, REG_MCUFW_CTRL, val);

    let mut off = FW_HDR_SIZE;
    for (name, size, addr) in [
        ("dmem", dmem_size, hdr.dmem_addr),
        ("imem", imem_size, hdr.imem_addr),
        ("emem", emem_size, hdr.emem_addr),
    ] {
        if size == 0 {
            continue;
        }
        let addr = addr & !(1u32 << 31);
        host::print("  ");
        host::print(name);
        host::print(" -> 0x");
        host::print_hex32(addr);
        host::print(", ");
        host::print_dec(size);
        host::print(" Bytes ");
        let t0 = host::now_us();
        if !download_firmware_to_mem(h, trx, stage, &fw[off..], 0, addr, size, band) {
            host::print("— FEHLER\n");
            return false;
        }
        host::print("— ok (");
        host::print_dec((host::now_us() - t0) as u32 / 1000);
        host::print(" ms)\n");
        off += size as usize;
    }
    true
}

/// mac.c `download_firmware_end_flow`
fn download_firmware_end_flow(h: i32) {
    host::w32(h, REG_TXDMA_STATUS, BTI_PAGE_OVF);

    let fw_ctrl = host::r16(h, REG_MCUFW_CTRL) as u32;
    if fw_ctrl & BIT_CHECK_SUM_OK != BIT_CHECK_SUM_OK {
        return;
    }
    let v = (fw_ctrl | BIT_FW_DW_RDY) & !(BIT_MCUFWDL_EN as u32);
    host::w16(h, REG_MCUFW_CTRL, v as u16);
}

/// mac.c `download_firmware_validate`
fn download_firmware_validate(h: i32) -> bool {
    if check_hw_ready(h, REG_MCUFW_CTRL, FW_READY_MASK, FW_READY) {
        return true;
    }
    let fw_key = host::r32(h, REG_FW_DBG7) & FW_KEY_MASK;
    if fw_key == ILLEGAL_KEY_GROUP {
        host::print("[rtl8822ce] invalid fw key\n");
    }
    false
}

/// mac.c `__rtw_download_firmware`, alle dreizehn Schritte in Reihenfolge.
pub fn download_firmware(h: i32, trx: &mut Trx, stage: i32, fw: &[u8], band: u8) -> bool {
    if !check_firmware_size(fw) {
        host::print("[rtl8822ce] Firmware-Groesse passt nicht zum Kopf\n");
        return false;
    }

    let ltecoex_bckp = match ltecoex_read_reg(h, 0x38) {
        Some(v) => v,
        None => {
            host::print("[rtl8822ce] LTE-Coex antwortet nicht\n");
            return false;
        }
    };

    wlan_cpu_enable(h, false);

    let bckp = download_firmware_reg_backup(h);
    download_firmware_reset_platform(h);

    let ok = start_download_firmware(h, trx, stage, fw, band);

    if ok {
        restore_reg(h, &bckp);
        download_firmware_end_flow(h);
        wlan_cpu_enable(h, true);
        if !ltecoex_reg_write(h, 0x38, ltecoex_bckp) {
            host::print("[rtl8822ce] LTE-Coex-Ruecksicherung fehlgeschlagen\n");
            return false;
        }
        if download_firmware_validate(h) {
            // "reset desc and index" — rtw_hci_setup nach dem Download,
            // also wieder BEIDE Haelften.
            crate::pci::setup(h, trx);
            return true;
        }
    }

    // dlfw_fail
    host::clr8(h, REG_MCUFW_CTRL, BIT_MCUFWDL_EN);
    host::set8(h, REG_SYS_FUNC_EN + 1, BIT_FEN_CPUEN);
    false
}
