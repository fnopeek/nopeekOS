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
