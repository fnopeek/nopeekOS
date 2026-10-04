//! `mac.c` from Linux 6.18.26 rtw88: power sequencing, firmware download,
//! MAC init and queue flushing.
//!
//! Power path, ported completely and in call order:
//! `rtw_mac_pre_system_cfg` · `do_pwr_poll_cmd` · `rtw_pwr_cmd_polling` ·
//! `rtw_sub_pwr_seq_parser` · `rtw_pwr_seq_parser` · `rtw_mac_power_switch` ·
//! `__rtw_mac_init_system_cfg` · `rtw_mac_init_system_cfg` ·
//! `rtw_mac_power_on` · `rtw_mac_power_off`.
//!
//! The 8822C is WCPU_3081, not 8051, so every `rtw_chip_wcpu_8051()` branch
//! is statically false. It is kept as a constant so it stays visible that
//! Linux has a second path there and which one applies.

use crate::host;
use crate::pwrseq::*;
use crate::regs::*;

/// rtw8822c.c `rtw8822c_hw_spec.wlan_cpu = RTW_WCPU_3081`.
const WCPU_8051: bool = false;

/// PCIe only; `rtw_hci_type()` is a constant here.
const INTF_MASK: u8 = RTW_PWR_INTF_PCI_MSK;

pub enum PwrErr {
    /// Linux: `-EALREADY`, the chip is already in the requested state.
    Already,
    /// Linux: `-EBUSY`, a polling command timed out.
    Busy,
}

/// mac.c `rtw_mac_pre_system_cfg`, PCIe-Zweig.
pub fn pre_system_cfg(h: i32) {
    host::w8(h, REG_RSV_CTRL, 0);

    if WCPU_8051 {
        // Linux sets REG_LDO_SWR_CTRL from BIT_LDO here and returns at once; the
        // rest of this function applies only to the 3081 family. Unreachable for
        // the 8822C.
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

/// mac.c `do_pwr_poll_cmd`. Linux polls every 50 us for up to
/// `50 * RTW_PWR_POLLING_CNT` us = 1 s.
///
/// `npk_sleep` has millisecond granularity, so this reads in a tight loop
/// and enforces the deadline against `now_us()`: the same total deadline
/// as Linux without the interval. There is deliberately no iteration cap.
fn do_pwr_poll_cmd(h: i32, addr: u32, mask: u8, target: u8) -> bool {
    let target = target & mask;
    let start = host::now_us();
    let deadline = start + 50 * RTW_PWR_POLLING_CNT as u64;
    // Spin tightly for the normal case (Linux polls every 50 us and is
    // usually done after a few rounds). After that, yield between reads:
    // three polling commands apply on PCIe, and three one-second deadlines
    // would otherwise block this core for seconds.
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

/// mac.c `rtw_pwr_cmd_polling`, including the PCIe special case: on
/// timeout, `BIT_PFM_WOWL` is toggled and polled once more. Without this
/// retry the sequence fails on some boards on the first cold boot.
fn pwr_cmd_polling(h: i32, cmd: &PwrCmd) -> Result<(), PwrErr> {
    // `base == RTW_PWR_ADDR_SDIO` adds SDIO_LOCAL_OFFSET in Linux. Every such
    // entry has intf_mask SDIO and is filtered out one level up; unreachable
    // on PCIe.
    let offset = cmd.offset as u32;

    if do_pwr_poll_cmd(h, offset, cmd.mask, cmd.value) {
        return Ok(());
    }

    // PCIe: toggle BIT_PFM_WOWL and retry.
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
                // The four 8822C sequences contain no DELAY (46 WRITE, 4 POLLING, 4 END).
                // The branch is kept because `rtw_sub_pwr_seq_parser` has it.
                if cmd.value == RTW_PWR_DELAY_US {
                    // Below our resolution; one millisecond is the shortest delay available.
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
    // rtw_chip_wcpu_3081 applies to the 8822C.
    if !WCPU_8051 {
        let rpwm = host::r8(h, PCIE_RPWM_ADDR);
        // Firmware still running? Then flip the RPWM toggle so it notices the
        // transition.
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

/// mac.c `__rtw_mac_init_system_cfg` (the 3081 path).
fn init_system_cfg(h: i32) {
    if WCPU_8051 {
        return; // `__rtw_mac_init_system_cfg_legacy`, unreachable here
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
/// The `-EALREADY` fallback is not a special case: after a warm reboot the
/// chip is still on, and off-then-on is the normal path.
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

// ── Firmware download (mac.c) ────────────────────────────────────

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

/// util.c `rtw_restore_reg`. mac.c `download_firmware_reg_restore` is a
/// one-line wrapper around it and is folded in here.
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
pub fn ltecoex_read_reg(h: i32, offset: u16) -> Option<u32> {
    if !check_hw_ready(h, LTECOEX_ACCESS_CTRL, LTECOEX_READY, 1) {
        return None;
    }
    host::w32(h, LTECOEX_ACCESS_CTRL, 0x800F_0000 | offset as u32);
    Some(host::r32(h, LTECOEX_READ_DATA))
}

/// util.c `ltecoex_reg_write`
pub fn ltecoex_reg_write(h: i32, offset: u16, value: u32) -> bool {
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

/// Fields of `struct rtw_fw_hdr` (fw.h:20-40), all little-endian.
/// `h2c_fmt_ver` selects the H2C format.
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
    band: u8, rsvd_boundary: u16, dump_first: bool,
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
        // pg_addr = src >> 7. The USB special case (+1 byte when
        // (size + TX_DESC_SIZE) is a multiple of 512) applies only there, and the
        // `kmemdup` is Linux memory management.
        // Only the very first chunk of the whole download is dumped.
        let verbose = dump_first && first_part && mem_offset == 0;
        if !write_data_rsvd_page(h, trx, stage, (src >> 7) as u16,
                                 &data[from..to], rsvd_boundary, band, verbose) {
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
fn start_download_firmware(h: i32, trx: &Trx, stage: i32, fw: &[u8], band: u8,
                           rsvd_boundary: u16) -> bool {
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
        if !download_firmware_to_mem(h, trx, stage, &fw[off..], 0, addr, size,
                                     band, rsvd_boundary, off == FW_HDR_SIZE) {
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
///
/// This waits for firmware that is starting up: it sets `BIT_FW_INIT_RDY`
/// itself after `wlan_cpu_enable` started its core. Linux allows 10 ms. If
/// that passes, measuring continues instead of giving up, so the log tells
/// a too-short deadline apart from a bit that never comes.
const VALIDATE_DIAG_US: u64 = 500_000;

fn download_firmware_validate(h: i32) -> bool {
    let (ok, us) = crate::fw::check_hw_ready_for(
        h, REG_MCUFW_CTRL, FW_READY_MASK, FW_READY, crate::fw::LINUX_FRIST_US);
    if ok {
        host::print("  FW_READY nach ");
        host::print_dec(us as u32);
        host::print(" us (Linux gibt 10000)\n");
        return true;
    }

    host::print("  nach Linux' 10 ms: MCUFW_CTRL = 0x");
    host::print_hex16(host::r16(h, REG_MCUFW_CTRL));
    host::print(" — weiter messen bis 500 ms …\n");
    let (ok2, us2) = crate::fw::check_hw_ready_for(
        h, REG_MCUFW_CTRL, FW_READY_MASK, FW_READY, VALIDATE_DIAG_US);
    if ok2 {
        host::print("  FW_READY doch, nach ");
        host::print_dec((us + us2) as u32);
        host::print(" us — Linux' Frist ist auf dieser Maschine zu knapp\n");
        return true;
    }
    host::print("  auch nach ");
    host::print_dec(((us + us2) / 1000) as u32);
    host::print(" ms nicht: MCUFW_CTRL = 0x");
    host::print_hex16(host::r16(h, REG_MCUFW_CTRL));
    host::print("\n");

    let fw_key = host::r32(h, REG_FW_DBG7) & FW_KEY_MASK;
    host::print("  FW_DBG7 = 0x");
    host::print_hex32(host::r32(h, REG_FW_DBG7));
    if fw_key == ILLEGAL_KEY_GROUP {
        host::print("  -> invalid fw key");
    }
    host::print("\n");
    false
}

/// mac.c `__rtw_download_firmware`, all thirteen steps in order.
pub fn download_firmware(h: i32, trx: &mut Trx, stage: i32, fw: &[u8], band: u8,
                         rsvd_boundary: u16) -> bool {
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

    // rtw_chip_efuse_enable: this sits between mac_power_on and the download;
    // without it the firmware later sends no hw-feature report.
    host::w8(h, REG_C2HEVT, C2H_HW_FEATURE_DUMP);

    // What the power sequence left behind, before the backup rewrites it.
    // REG_RQPN_CTRL_2 is the interesting one: the backup only ORs in
    // BIT_LD_RQPN and does not set the page counts, which come in
    // __priority_queue_cfg after the download. A 0 here means the HIQ has no
    // pages and no reserved page can land.
    host::print("  [dump] nach power_on, vor dem Backup:\n");
    crate::fw::dump_reg32(h, "RQPN_CTRL2", REG_RQPN_CTRL_2);
    crate::fw::dump_reg32(h, "FIFOPG_I1 ", REG_FIFOPAGE_INFO_1);
    crate::fw::dump_reg32(h, "CR        ", REG_CR);
    crate::fw::dump_reg32(h, "TXDMA_PQ  ", REG_TXDMA_PQ_MAP);

    wlan_cpu_enable(h, false);

    let bckp = download_firmware_reg_backup(h);
    download_firmware_reset_platform(h);

    let ok = start_download_firmware(h, trx, stage, fw, band, rsvd_boundary);

    if ok {
        restore_reg(h, &bckp);
        download_firmware_end_flow(h);
        wlan_cpu_enable(h, true);
        if !ltecoex_reg_write(h, 0x38, ltecoex_bckp) {
            host::print("[rtl8822ce] LTE-Coex-Ruecksicherung fehlgeschlagen\n");
            return false;
        }
        if download_firmware_validate(h) {
            // "reset desc and index": rtw_hci_setup after the download, so both
            // halves again.
            crate::pci::setup(h, trx, false);
            return true;
        }
    }

    // dlfw_fail
    host::clr8(h, REG_MCUFW_CTRL, BIT_MCUFWDL_EN);
    host::set8(h, REG_SYS_FUNC_EN + 1, BIT_FEN_CPUEN);
    false
}

// ── rtw_mac_init (mac.c:1391) ────────────────────────────────────
//
// Order as in Linux:
//   rtw_mac_init
//    ├─ rtw_init_trx_cfg
//    │   ├─ txdma_queue_mapping
//    │   ├─ priority_queue_cfg   (rtw_set_trx_fifo_info + __priority_queue_cfg)
//    │   └─ init_h2c
//    ├─ chip->ops->mac_init      = chip::mac_init
//    ├─ rtw_drv_info_cfg
//    └─ rtw_hci_interface_cfg    = pci::interface_cfg

use crate::chip;

/// main.h:1886-1900 `struct rtw_fifo_conf`, without the pointer to `rqpn`,
/// which is a constant here because there is only one bus type.
///
/// The default matters: `rtw_fw_write_data_rsvd_page` writes
/// `rsvd_boundary` back during cleanup, and before `rtw_mac_init` has run
/// Linux has 0 there. The firmware download happens before
/// `rtw_mac_init`, so 0 is correct.
#[derive(Default, Clone, Copy)]
pub struct Fifo {
    pub rsvd_boundary: u16,
    pub rsvd_pg_num: u16,
    pub rsvd_drv_pg_num: u16,
    pub txff_pg_num: u16,
    pub acq_pg_num: u16,
    pub rsvd_drv_addr: u16,
    pub rsvd_h2c_info_addr: u16,
    pub rsvd_h2c_sta_info_addr: u16,
    pub rsvd_h2cq_addr: u16,
    pub rsvd_cpu_instr_addr: u16,
    pub rsvd_fw_txbuf_addr: u16,
    pub rsvd_csibuf_addr: u16,
}

pub enum MacErr {
    /// Linux: `-ENOMEM`, the page plan does not fit into the TX FIFO.
    NoMem,
    /// Linux: `-EINVAL`, rsvd_boundary and rsvd_drv_addr disagree, or the H2C
    /// ring reports a fill level other than its size.
    Inval,
    /// Linux: `-EBUSY`, the hardware did not build the link list table.
    Busy,
}

/// mac.c:1086-1135 `txdma_queue_mapping`, PCIe-Zweig.
fn txdma_queue_mapping(h: i32) -> &'static chip::Rqpn {
    let rqpn = &chip::RQPN_PCIE;

    let mut txdma_pq_map: u16 = 0;
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_hi, BIT_SHIFT_TXDMA_HIQ_MAP);
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_mg, BIT_SHIFT_TXDMA_MGQ_MAP);
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_bk, BIT_SHIFT_TXDMA_BKQ_MAP);
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_be, BIT_SHIFT_TXDMA_BEQ_MAP);
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_vi, BIT_SHIFT_TXDMA_VIQ_MAP);
    txdma_pq_map |= bit_txdma_queue_map(rqpn.dma_map_vo, BIT_SHIFT_TXDMA_VOQ_MAP);
    host::w16(h, REG_TXDMA_PQ_MAP, txdma_pq_map);

    // First off, then all eight TRX bits on, as in Linux; a `write8` on
    // REG_CR leaves the upper bytes of the 32-bit register alone.
    host::w8(h, REG_CR, 0);
    host::w8(h, REG_CR, MAC_TRX_ENABLE);

    // rtw_chip_wcpu_3081, applies to the 8822C.
    if !WCPU_8051 {
        host::w32(h, REG_H2CQ_CSR, BIT_H2CQ_FULL);
    }

    rqpn
}

/// mac.c:1138-1186 `rtw_set_trx_fifo_info`, 3081 branch.
///
/// Computes the TX FIFO page plan top-down: what the firmware reserves sits
/// at the end of the FIFO, and `rsvd_boundary` is the limit below which
/// the TX queues operate. The final check (`rsvd_boundary ==
/// rsvd_drv_addr`) is Linux's own cross-check: the boundary is computed
/// two ways, and if they disagree the plan is wrong.
pub fn set_trx_fifo_info() -> Result<Fifo, MacErr> {
    let mut f = Fifo {
        rsvd_drv_pg_num: RSVD_DRV_PG_NUM_8822C,
        txff_pg_num: (TXFF_SIZE_8822C / TX_PAGE_SIZE) as u16,
        ..Default::default()
    };
    let csi_buf_pg_num = CSI_BUF_PG_NUM_8822C;

    f.rsvd_pg_num = if WCPU_8051 {
        f.rsvd_drv_pg_num
    } else {
        f.rsvd_drv_pg_num
            + RSVD_PG_H2C_EXTRAINFO_NUM
            + RSVD_PG_H2C_STATICINFO_NUM
            + RSVD_PG_H2CQ_NUM
            + RSVD_PG_CPU_INSTRUCTION_NUM
            + RSVD_PG_FW_TXBUF_NUM
            + csi_buf_pg_num
    };

    if f.rsvd_pg_num > f.txff_pg_num {
        return Err(MacErr::NoMem);
    }

    f.acq_pg_num = f.txff_pg_num - f.rsvd_pg_num;
    f.rsvd_boundary = f.txff_pg_num - f.rsvd_pg_num;

    let mut cur_pg_addr = f.txff_pg_num;
    if !WCPU_8051 {
        cur_pg_addr -= csi_buf_pg_num;
        f.rsvd_csibuf_addr = cur_pg_addr;
        cur_pg_addr -= RSVD_PG_FW_TXBUF_NUM;
        f.rsvd_fw_txbuf_addr = cur_pg_addr;
        cur_pg_addr -= RSVD_PG_CPU_INSTRUCTION_NUM;
        f.rsvd_cpu_instr_addr = cur_pg_addr;
        cur_pg_addr -= RSVD_PG_H2CQ_NUM;
        f.rsvd_h2cq_addr = cur_pg_addr;
        cur_pg_addr -= RSVD_PG_H2C_STATICINFO_NUM;
        f.rsvd_h2c_sta_info_addr = cur_pg_addr;
        cur_pg_addr -= RSVD_PG_H2C_EXTRAINFO_NUM;
        f.rsvd_h2c_info_addr = cur_pg_addr;
    }
    cur_pg_addr -= f.rsvd_drv_pg_num;
    f.rsvd_drv_addr = cur_pg_addr;

    if f.rsvd_boundary != f.rsvd_drv_addr {
        host::print("[rtl8822ce] wrong rsvd driver address\n");
        return Err(MacErr::Inval);
    }

    Ok(f)
}

/// mac.c:1192-1236 `__priority_queue_cfg` (3081-Zweig, PCIe).
fn priority_queue_cfg_3081(h: i32, f: &Fifo, pg: &chip::PageTable, pubq_num: u16)
    -> Result<(), MacErr>
{
    host::w16(h, REG_FIFOPAGE_INFO_1, pg.hq_num);
    host::w16(h, REG_FIFOPAGE_INFO_2, pg.lq_num);
    host::w16(h, REG_FIFOPAGE_INFO_3, pg.nq_num);
    host::w16(h, REG_FIFOPAGE_INFO_4, pg.exq_num);
    host::w16(h, REG_FIFOPAGE_INFO_5, pubq_num);
    host::set32(h, REG_RQPN_CTRL_2, BIT_LD_RQPN);

    host::w16(h, REG_FIFOPAGE_CTRL_2, f.rsvd_boundary);
    host::set8(h, REG_FWHW_TXQ_CTRL + 2, (BIT_EN_WR_FREE_TAIL >> 16) as u8);

    host::w16(h, REG_BCNQ_BDNY_V1, f.rsvd_boundary);
    host::w16(h, REG_FIFOPAGE_CTRL_2 + 2, f.rsvd_boundary);
    host::w16(h, REG_BCNQ1_BDNY_V1, f.rsvd_boundary);
    host::w32(h, REG_RXFF_BNDY, RXFF_SIZE_8822C - C2H_PKT_BUF - 1);

    // The USB branch (BIT_MASK_BLK_DESC_NUM, TXDMA_OFFSET_CHK+1) sits here in
    // Linux and does not apply.

    host::set8(h, REG_AUTO_LLT_V1, BIT_AUTO_INIT_LLT_V1 as u8);

    // The hardware now builds its link list table and clears the bit when
    // done. That is the only acknowledgement of the page plan.
    if !check_hw_ready(h, REG_AUTO_LLT_V1, BIT_AUTO_INIT_LLT_V1, 0) {
        host::print("[rtl8822ce] AUTO_INIT_LLT bleibt stehen — LLT nicht gebaut\n");
        return Err(MacErr::Busy);
    }

    host::w8(h, REG_CR + 3, 0);
    Ok(())
}

/// mac.c:1260-1299 `priority_queue_cfg`, PCIe-Zweig.
fn priority_queue_cfg(h: i32) -> Result<Fifo, MacErr> {
    let f = set_trx_fifo_info()?;
    let pg = &chip::PAGE_TABLE[1]; // RTW_HCI_TYPE_PCIE

    let pubq_num = f.acq_pg_num - pg.hq_num - pg.lq_num - pg.nq_num
        - pg.exq_num - pg.gapq_num;

    if WCPU_8051 {
        // `__priority_queue_cfg_legacy`, unreachable for the 8822C.
        return Err(MacErr::Inval);
    }
    priority_queue_cfg_3081(h, &f, pg, pubq_num)?;
    Ok(f)
}

/// mac.c:1301-1352 `init_h2c` (3081).
///
/// The H2C ring lives inside the TX FIFO, on the pages `set_trx_fifo_info`
/// reserved for it. Head, tail and read pointer are written as byte
/// addresses (`page << 7`), and the final check asks the hardware how full
/// it sees the ring: a fresh ring is empty, so `h2cq_free` must equal
/// `h2cq_size`.
fn init_h2c(h: i32, f: &Fifo) -> Result<(), MacErr> {
    if WCPU_8051 {
        return Ok(());
    }

    let h2cq_addr = (f.rsvd_h2cq_addr as u32) << TX_PAGE_SIZE_SHIFT;
    let h2cq_size = (RSVD_PG_H2CQ_NUM as u32) << TX_PAGE_SIZE_SHIFT;

    let mut value32 = host::r32(h, REG_H2C_HEAD);
    value32 = (value32 & 0xFFFC_0000) | h2cq_addr;
    host::w32(h, REG_H2C_HEAD, value32);

    let mut value32 = host::r32(h, REG_H2C_READ_ADDR);
    value32 = (value32 & 0xFFFC_0000) | h2cq_addr;
    host::w32(h, REG_H2C_READ_ADDR, value32);

    let mut value32 = host::r32(h, REG_H2C_TAIL);
    value32 &= 0xFFFC_0000;
    value32 |= h2cq_addr + h2cq_size;
    host::w32(h, REG_H2C_TAIL, value32);

    let value8 = (host::r8(h, REG_H2C_INFO) & 0xFC) | 0x01;
    host::w8(h, REG_H2C_INFO, value8);

    let value8 = (host::r8(h, REG_H2C_INFO) & 0xFB) | 0x04;
    host::w8(h, REG_H2C_INFO, value8);

    let value8 = (host::r8(h, REG_TXDMA_OFFSET_CHK + 1) & 0x7f) | 0x80;
    host::w8(h, REG_TXDMA_OFFSET_CHK + 1, value8);

    let wp = host::r32(h, REG_H2C_PKT_WRITEADDR) & 0x3FFFF;
    let rp = host::r32(h, REG_H2C_PKT_READADDR) & 0x3FFFF;
    let h2cq_free = if wp >= rp { h2cq_size - (wp - rp) } else { rp - wp };

    host::print("  H2C-Ring @0x");
    host::print_hex32(h2cq_addr);
    host::print(", ");
    host::print_dec(h2cq_size);
    host::print(" Bytes — frei ");
    host::print_dec(h2cq_free);
    host::print(" (wp 0x");
    host::print_hex32(wp);
    host::print(", rp 0x");
    host::print_hex32(rp);
    host::print(")\n");

    if h2cq_size != h2cq_free {
        host::print("[rtl8822ce] H2C queue mismatch\n");
        return Err(MacErr::Inval);
    }
    Ok(())
}

/// mac.c:1354-1371 `rtw_init_trx_cfg`
fn init_trx_cfg(h: i32) -> Result<Fifo, MacErr> {
    txdma_queue_mapping(h);
    let f = priority_queue_cfg(h)?;
    init_h2c(h, &f)?;
    Ok(f)
}

/// mac.c:1373-1389 `rtw_drv_info_cfg`
fn drv_info_cfg(h: i32) {
    host::w8(h, REG_RX_DRVINFO_SZ, PHY_STATUS_SIZE);
    if !WCPU_8051 {
        // "For rxdesc len = 0 issue"
        let value8 = (host::r8(h, REG_TRXFF_BNDY + 1) & 0xF0) | 0xF;
        host::w8(h, REG_TRXFF_BNDY + 1, value8);
    }
    host::set32(h, REG_RCR, BIT_APP_PHYSTS);
    host::clr32(h, REG_WMAC_OPTION_FUNCTION + 4, (1 << 8) | (1 << 9));
}

/// mac.c:1391-1411 `rtw_mac_init`.
///
/// Returns the page plan because it is needed from here on: every reserved
/// page written later is relative to `rsvd_boundary`.
pub fn mac_init(h: i32, cut_version: u8) -> Result<Fifo, MacErr> {
    let f = init_trx_cfg(h)?;

    // chip->ops->mac_init
    if !chip::mac_init(h) {
        return Err(MacErr::Inval);
    }

    drv_info_cfg(h);

    // rtw_hci_interface_cfg
    crate::pci::interface_cfg(h, cut_version);

    Ok(f)
}

/// mac.c:1029-1076 `rtw_set_channel_mac`, 3081 branch.
///
/// The MAC half of a channel switch: subchannel position, bandwidth in the
/// TX protocol, MAC clock and the CCK check, which is on for 5 GHz where no
/// CCK rate may arrive.
pub fn set_channel_mac(h: i32, channel: u8, bw: usize, primary_ch_idx: u8) {
    let txsc20 = primary_ch_idx;
    let mut txsc40 = 0u8;
    if bw == 2 {
        // RTW_CHANNEL_WIDTH_80
        txsc40 = if txsc20 == RTW_SC_20_UPPER || txsc20 == RTW_SC_20_UPMOST {
            RTW_SC_40_UPPER
        } else {
            RTW_SC_40_LOWER
        };
    }
    // reg.h:260-267 `BIT_TXSC_20M(x)` and `BIT_TXSC_40M(x)`
    host::w8(h, REG_DATA_SC,
             ((txsc20 & BIT_MASK_TXSC_20M) << BIT_SHIFT_TXSC_20M)
             | ((txsc40 & BIT_MASK_TXSC_40M) << BIT_SHIFT_TXSC_40M));

    let mut value32 = host::r32(h, REG_WMAC_TRXPTCL_CTL) & !BIT_RFMOD;
    match bw {
        2 => value32 |= BIT_RFMOD_80M,
        1 => value32 |= BIT_RFMOD_40M,
        // RTW_CHANNEL_WIDTH_20 and Linux's `default:`: nothing to do.
        _ => {}
    }
    host::w32(h, REG_WMAC_TRXPTCL_CTL, value32);

    if WCPU_8051 {
        return;
    }

    let mut value32 = host::r32(h, REG_AFE_CTRL1) & !BIT_MAC_CLK_SEL;
    value32 |= MAC_CLK_HW_DEF_80M << BIT_SHIFT_MAC_CLK_SEL;
    host::w32(h, REG_AFE_CTRL1, value32);

    host::w8(h, REG_USTIME_TSF, MAC_CLK_SPEED);
    host::w8(h, REG_USTIME_EDCA, MAC_CLK_SPEED);

    let mut value8 = host::r8(h, REG_CCK_CHECK) & !BIT_CHECK_CCK_EN;
    if channel >= 36 && channel <= 177 {
        value8 |= BIT_CHECK_CCK_EN;
    }
    host::w8(h, REG_CCK_CHECK, value8);
}

// ═══════════════════════════════════════════════════════════════════
// rtw_mac_flush_queues (mac.c:1020-1080): drain the TX queues
//
// Linux does this before a channel switch and before disconnecting;
// otherwise frames still go out on the old channel after a switch.
// ═══════════════════════════════════════════════════════════════════

/// rtw8822c.c:4943-4957 `prioq_addrs_8822c`: `(rsvd, avail)` per priority
/// queue, in the order of `enum rtw_dma_mapping` (EXTRA, LOW, NORMAL,
/// HIGH). `.wsize = true`, so 16-bit accesses.
const PRIOQ_ADDRS: [(u32, u32); 4] = [
    (REG_FIFOPAGE_INFO_4, REG_FIFOPAGE_INFO_4 + 2), // EXTRA
    (REG_FIFOPAGE_INFO_2, REG_FIFOPAGE_INFO_2 + 2), // LOW
    (REG_FIFOPAGE_INFO_3, REG_FIFOPAGE_INFO_3 + 2), // NORMAL
    (REG_FIFOPAGE_INFO_1, REG_FIFOPAGE_INFO_1 + 2), // HIGH
];

/// mac.c:1024-1060 `__rtw_mac_flush_prio_queue`.
///
/// The queue is empty when all reserved pages are available again. Five
/// rounds of 20 ms; Linux's own comment notes that a full queue can take
/// up to two seconds at 100 Mbit/s and frames may be dropped, so the
/// deadline is short on purpose.
fn flush_prio_queue(h: i32, prio: usize) -> bool {
    let (rsvd_reg, avail_reg) = PRIOQ_ADDRS[prio];
    for _ in 0..5 {
        let rsvd = host::r16(h, rsvd_reg);
        let avail = host::r16(h, avail_reg);
        if rsvd == avail {
            return true;
        }
        host::sleep_ms(20);
    }
    false
}

/// mac.c:1062-1069 `rtw_mac_flush_prio_queues` + mac.c:1071-1080
/// `rtw_mac_flush_queues`.
///
/// Always drains all four, Linux's "all queues requested or mapping not
/// yet set" branch; no caller drains individual access categories.
///
/// Returns how many queues became empty within the deadline.
pub fn flush_queues(h: i32) -> u32 {
    let mut done = 0;
    for prio in 0..PRIOQ_ADDRS.len() {
        if flush_prio_queue(h, prio) {
            done += 1;
        }
    }
    done
}
