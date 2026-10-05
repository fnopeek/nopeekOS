//! wifi_rtl8822ce — Realtek RTL8822CE (Wi-Fi 5, 2T2R, PCIe) WASM driver.
//!
//! Strict 1:1 port of Linux 6.18.26, `drivers/net/wireless/realtek/rtw88/`
//! (module `rtw_8822ce`). Plan: `docs/plan/WIFI_RTL8822CE.md`, map:
//! `docs/plan/WIFI_RTL8822CE_LINUX_MAP.md`.
//!
//! The driver runs as a chain of stages, each behind a gate:
//!
//! - 0: bind PCI, enable bus master, map BAR2 (pci.c `rtw_pci_io_mapping`:
//!   `u8 bar_id = 2`, not BAR0) and read `REG_SYS_CFG1` as
//!   `rtw_chip_parameter_setup` does. Writes nothing. Gate: plausible
//!   `chip_version`, RF type 2T2R, and 8-bit reads agree with 32-bit reads.
//! - 1: `rtw_mac_power_on` (`mac.rs`) with the four power sequence tables in
//!   `pwrseq.rs`, generated from the C source. Gate in both directions:
//!   `REG_CR` leaves `0xea` on power-on and returns to it on power-off.
//! - 2a: `rtw_pci_init_trx_ring` + `rtw_pci_reset_buf_desc` (`pci.rs`), in
//!   Linux order: the ring registers are programmed before the MAC powers on
//!   (`rtw_power_on` calls `rtw_hci_setup` before `rtw_mac_power_on`). Gate:
//!   every address and count register reads back, with MAC off and on.
//! - 2b: `rtw_download_firmware` (`mac.rs` + `fw.rs` + `tx.rs`) via the BCN
//!   queue and DDMA into dmem/imem/emem. Gate: `REG_MCUFW_CTRL` reads
//!   `FW_READY`.
//!
//! Whenever the driver returns, running firmware must be stopped first: the
//! kernel frees our DMA buffers on return, and firmware must not DMA into
//! them afterwards.

#![no_std]

mod host;
mod bf;
mod chip;
mod coex;
mod dm;
mod efuse;
mod fw;
mod mac;
mod pci;
mod phy;
mod pwrseq;
mod rfk;
mod sta;
mod rfkcal;
mod dpk;
mod txgapk;
mod rx;
mod sec;
mod tables;
mod tx;
mod vif;
mod txpower;
mod regs;
use regs::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

// The default rights plus HARDWARE, which binding the PCI device requires.
// A section replaces the default, so READ, EXECUTE and RENDER are listed.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x01 | 0x04 | 0x08 | 0x40]; // READ|EXEC|RENDER|HARDWARE

/// Never die silently: a bare `loop {}` looks like an unresponsive chip from
/// the outside. `Location` survives `strip = true` because it is static data.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // A panic is never stage output: loud even without `debug: 1`, and the
    // loud bracket is not closed again since nothing follows.
    host::loud_begin();
    host::print("\n[rtl8822ce] PANIC — Treiber gestoppt");
    if let Some(l) = info.location() {
        host::print(" at ");
        host::print(l.file());
        host::print(":");
        host::print_dec(l.line());
    }
    host::print("\n");
    host::log("[rtl8822ce] PANIC — Treiber gestoppt (Datei:Zeile steht oben)");
    loop {}
}

/// Single source for the version, so banner and report cannot diverge.
const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// rtw8822c.c `.fw_name = "rtw88/rtw8822c_fw.bin"`, bundled. Version 9.9.15;
/// `check_firmware_size` checks the header against the file length before a
/// byte goes to the chip.
static FW: &[u8] = include_bytes!("../firmware/rtw8822c_fw.bin");

/// `hal.current_band_type` is still 0 at download time; it is set later in
/// `rtw_set_channel`. It selects the branch in
/// `rtw_tx_pkt_info_update_rate`, and we take the same one as Linux.
const BAND_AT_FWDL: u8 = 0;

/// `struct rtw_dev`: state that lives as long as the driver, as `rtwdev`
/// lives from load to unload in Linux.
///
/// It must not be recreated per stage: `cfo_track.crystal_cap` comes from
/// `rtw_phy_init` (the efuse value), `dpk_info.thermal_dpk` from the stage 5d
/// calibration (without it `dpk_track` returns at once), and
/// `coex.bt_disabled` decides whether crystal tracking may run at all.
struct Dev {
    dm: dm::DmInfo,
    path_div: dm::PathDiv,
    dpk: dpk::DpkInfo,
    cx: coex::Coex,
    /// main.h:660-672 `struct rtw_traffic_stats`
    stats: TrafficStats,
    /// `rtwdev->watch_dog_cnt`; `rtw_phy_ra_info_update` runs only on every
    /// fourth tick.
    watch_dog_cnt: u32,
    /// `RTW_FLAG_BUSY_TRAFFIC`
    busy_traffic: bool,
    /// `rtwdev->beacon_loss`
    beacon_loss: bool,
    /// `hal->current_band_width`: the width the PHY is currently on, set by
    /// stage 5e on a channel switch. Kept here so stage 5f does not decide
    /// it a second time from the same inputs.
    cur_bw: usize,
}

/// main.h:660-672 `struct rtw_traffic_stats`. The units from the C comments
/// are part of the field names: bytes per two seconds, converted with
/// `RTW_TP_SHIFT`.
#[derive(Default, Clone, Copy)]
struct TrafficStats {
    tx_unicast: u64,
    rx_unicast: u64,
    tx_cnt: u64,
    rx_cnt: u64,
    tx_throughput: u32,
    rx_throughput: u32,
    /// Highest value ever seen; the smoothed value drops to zero after a
    /// transfer ends.
    tx_peak: u32,
    rx_peak: u32,
    tx_ewma_tp: dm::Ewma,
    rx_ewma_tp: dm::Ewma,
}

impl TrafficStats {
    const fn new() -> Self {
        TrafficStats {
            tx_unicast: 0, rx_unicast: 0, tx_cnt: 0, rx_cnt: 0,
            tx_throughput: 0, rx_throughput: 0,
            tx_peak: 0, rx_peak: 0,
            tx_ewma_tp: dm::Ewma::new(), rx_ewma_tp: dm::Ewma::new(),
        }
    }
}

/// Result of `rtw_chip_parameter_setup` (main.c:1876-1900).
struct Hal {
    chip_version: u32,
    cut_version: u8,
    mp_chip: u8,
    vendor_id: u8,
    rf_2t2r: bool,
    rf_path_num: u8,
    /// main.c:1884-1893: `BB_PATH_AB` for both on 2T2R, else `BB_PATH_A`.
    antenna_tx: u8,
    antenna_rx: u8,
    /// Default from main.c:2183, ORed with `BIT_VHT_DACK` at main.c:1903.
    /// `rtw_core_start` writes it to the register after `mac_init` left
    /// `WLAN_RCR_CFG` there ("rcr reset after powered on").
    rcr: u32,
}

/// main.c `rtw_chip_parameter_setup`, the part that reads a register. The
/// rest of the function only copies fields from `chip`.
fn chip_parameter_setup(h: i32) -> Hal {
    let chip_version = host::r32(h, REG_SYS_CFG1);
    let rf_2t2r = chip_version & BIT_RF_TYPE_ID != 0;
    Hal {
        chip_version,
        cut_version: bit_get_chip_ver(chip_version),
        // main.c:1883: a set BIT_RTL_ID means not mp_chip.
        mp_chip: if chip_version & BIT_RTL_ID != 0 { 0 } else { 1 },
        vendor_id: bit_get_vendor_id(chip_version),
        rf_2t2r,
        rf_path_num: if rf_2t2r { 2 } else { 1 },
        antenna_tx: if rf_2t2r { BB_PATH_AB } else { BB_PATH_A },
        antenna_rx: if rf_2t2r { BB_PATH_AB } else { BB_PATH_A },
        // main.c:2183 "default rx filter setting" plus main.c:1903.
        rcr: BIT_APP_FCS | BIT_APP_MIC | BIT_APP_ICV | BIT_PKTCTL_DLEN
            | BIT_HTC_LOC_CTRL | BIT_APP_PHYSTS | BIT_AB | BIT_AM | BIT_APM
            | BIT_VHT_DACK,
    }
}

/// Checks the 8- and 16-bit MMIO paths against the 32-bit path: four single
/// byte reads must yield the same word. Read-only, and it catches a broken
/// narrow path before the power sequence depends on it.
fn check_access_widths(h: i32, word: u32) -> bool {
    let b0 = host::r8(h, REG_SYS_CFG1) as u32;
    let b1 = host::r8(h, REG_SYS_CFG1 + 1) as u32;
    let b2 = host::r8(h, REG_SYS_CFG1 + 2) as u32;
    let b3 = host::r8(h, REG_SYS_CFG1 + 3) as u32;
    let from8 = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);

    let w0 = host::r16(h, REG_SYS_CFG1) as u32;
    let w1 = host::r16(h, REG_SYS_CFG1 + 2) as u32;
    let from16 = w0 | (w1 << 16);

    host::print("  8-Bit  : 0x");
    host::print_hex32(from8);
    host::print("   16-Bit : 0x");
    host::print_hex32(from16);
    host::print("   32-Bit : 0x");
    host::print_hex32(word);
    host::print("\n");

    from8 == word && from16 == word
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    // ── Verbosity ────────────────────────────────────────────────
    // Decided before the first output line. Stage output is off by default;
    // `debug: 1` in `sys/config/wifi` enables it. Stage 5c reads `ssid:` from
    // the same file, so the configuration lives in one place.
    let (verbose, cfg_rc) = read_debug_flag();
    host::set_verbose(verbose);

    host::print("[rtl8822ce] Realtek RTL8822CE (rtw88) v");
    host::print(DRIVER_VERSION);
    host::print(" — Stufe 0: binden, BAR2, Chipkennung\n");
    if !verbose {
        // The one line a silent run still prints: the driver exists, where
        // the switch is, and whether the file was read at all. `wifid`
        // documents a race with the rest of boot for this object; without
        // this, a failed read looks like a switch that does nothing.
        host::say("[rtl8822ce] v");
        host::say(DRIVER_VERSION);
        if cfg_rc > 0 {
            host::say(" — still (`debug: 1` in sys/config/wifi zeigt die Stufen)\n");
        } else {
            host::say(" — still, und sys/config/wifi war beim Start nicht\n\
             \x20         lesbar: ein `debug: 1` darin greift dann NICHT\n");
        }
    }

    // ── `rtwdev`: state for the whole driver run ─────────────────
    // Too large for the stack (DACK backups and rate counters dominate).
    static mut DEV: Dev = Dev {
        dm: dm::DmInfo::new(),
        path_div: dm::PathDiv::new(),
        dpk: dpk::DpkInfo::new(),
        cx: coex::Coex::new(),
        stats: TrafficStats::new(),
        watch_dog_cnt: 0,
        busy_traffic: false,
        beacon_loss: false,
        cur_bw: 0,
    };
    // SAFETY: single-threaded, exactly one caller, and `_start` returns only
    // when the driver ends.
    let rtwdev = unsafe { &mut *core::ptr::addr_of_mut!(DEV) };

    // ── Bind PCI ─────────────────────────────────────────────────
    // rtw8822ce.c lists two device IDs for the same chip.
    let mut dev = RTL8822CE_DEVICE;
    let mut rc = host::pci_bind(RTL_VENDOR, dev);
    if rc != 0 {
        dev = RTL8822CE_DEVICE_ALT;
        rc = host::pci_bind(RTL_VENDOR, dev);
    }
    if rc != 0 {
        host::loud_begin();
        host::print("[rtl8822ce] PCI-Bind fehlgeschlagen (");
        match rc {
            -1 => host::print("nicht gefunden"),
            -2 => host::print("abgelehnt"),
            _ => host::print("unbekannter Fehler"),
        }
        host::print(") — erwartet 10ec:c822 oder 10ec:c82f\n");
        host::loud_end();
        return;
    }
    host::print("[rtl8822ce] gebunden: 10ec:");
    host::print_hex16(dev);
    host::print("\n");

    // Check the result: without bus mastering the chip cannot fetch a
    // descriptor from memory, which would look like a driver bug rather
    // than a missing permission.
    let bm = host::pci_enable_bus_master();
    if bm != 0 {
        host::say("[rtl8822ce] Bus-Master konnte nicht eingeschaltet werden\n");
    }
    fw::dump_pci_cmd("nach bind");

    // ── D0 before any register read ──────────────────────────────
    // A device in D3hot answers every MMIO read with all ones while config
    // space answers normally. That looks like a timing problem because it
    // depends on the state a previous run left the card in.
    match pci::power_up_d0(0) {
        Some(0) => {}
        Some(st) => {
            host::loud_begin();
            host::print("[rtl8822ce] die Karte lag in D");
            host::print_dec(st as u32);
            host::print(" — nach D0 geholt und 10 ms gewartet\n");
            host::loud_end();
        }
        None => host::print("[rtl8822ce] keine PM-Capability (kein D-State)\n"),
    }

    // ── Map BAR2 ─────────────────────────────────────────────────
    let h = host::mmio_map_bar(BAR_REG, BAR_PAGES);
    if h < 0 {
        host::say("[rtl8822ce] BAR2 nicht abbildbar — Stufe 0 endet hier\n");
        return;
    }
    host::print("[rtl8822ce] BAR2 abgebildet (handle ");
    host::print_dec(h as u32);
    host::print(", 64 KiB)\n");

    // ── `rtw_pci_phy_cfg` / `rtw_pci_link_cfg` ───────────────────
    // Must follow the BAR mapping: the DBI path goes through MMIO.
    pci::link_cfg(h);
    let aspm_vorher = match read_aspm_pref() {
        Some(an) => pci::aspm_host_set(an).map(|v| (v, Some(an))),
        None => pci::link_state().map(|l| (l.aspm, None)),
    };
    host::print("[rtl8822ce] PCIe-Link: ASPM vorgefunden ");
    match aspm_vorher {
        Some((v, gesetzt)) => {
            host::print(match v {
                0 => "aus",
                1 => "L0s",
                2 => "L1",
                _ => "L0s+L1",
            });
            match gesetzt {
                Some(true) => host::print(", von uns EINgeschaltet"),
                Some(false) => host::print(", von uns AUSgeschaltet"),
                None => host::print(", unangetastet (aspm: wie-gefunden)"),
            }
        }
        None => host::print("keine PCIe-Capability gefunden"),
    }
    if let Some((l1, clk)) = pci::link_cfg_state(h) {
        host::print(" · Realtek L1_SW ");
        host::print(if l1 { "an" } else { "aus" });
        host::print(", CLKREQ_SW ");
        host::print(if clk { "an" } else { "aus" });
    }
    host::print("\n");

    // ── Read the chip ID (rtw_chip_parameter_setup) ──────────────
    // Poll instead of reading once: a waking chip may need time even after
    // the 10 ms `power_up_d0` waits per spec. Normally the first read hits;
    // on failure this costs 200 ms instead of a reboot.
    const WINDOW_WAIT_US: u64 = 200_000;
    let t_win = host::now_us();
    let mut hal = chip_parameter_setup(h);
    let mut runden = 1u32;
    while (hal.chip_version == 0xFFFF_FFFF || hal.chip_version == 0)
        && host::now_us() - t_win < WINDOW_WAIT_US
    {
        host::sleep_ms(1);
        hal = chip_parameter_setup(h);
        runden += 1;
    }

    // All ones is not the chip answering but the bus answering an address
    // where nobody is.
    let dead = hal.chip_version == 0xFFFF_FFFF || hal.chip_version == 0;
    if runden > 1 {
        host::loud_begin();
        host::print("[rtl8822ce] das Registerfenster brauchte ");
        host::print_dec((host::now_us() - t_win) as u32);
        host::print(" us und ");
        host::print_dec(runden);
        host::print(" Versuche\n");
        host::loud_end();
    }
    if dead {
        // Config space answers even when MMIO does not. A valid ID there
        // means the card is present and the memory path is the problem,
        // which is a different failure from "not found".
        host::loud_begin();
        host::print("[rtl8822ce] MMIO liefert 0x");
        host::print_hex32(hal.chip_version);
        host::print(", aber PCI-Konfig sagt 0x");
        host::print_hex32(host::pci_read_config(0x00));
        host::print(" · CMD 0x");
        host::print_hex32(host::pci_read_config(0x04));
        host::print(" · BAR2 0x");
        host::print_hex32(host::pci_read_config(0x18));
        host::print("\n");
        host::loud_end();
    }

    host::print("[rtl8822ce] Register:\n");
    host::log_reg32("SYS_CFG1 ", hal.chip_version);
    host::log_reg32("SYS_CFG2 ", host::r32(h, REG_SYS_CFG2));
    host::log_reg32("SYS_PWCTL", host::r32(h, REG_SYS_PW_CTRL));
    host::log_reg32("SYS_STAT1", host::r32(h, REG_SYS_STATUS1));

    // The two values Linux itself compares literally; the first is
    // explicitly an 8-bit read (mac.c `rtw_mac_power_switch`:
    // `rtw_read8(rtwdev, REG_CR) == 0xea`).
    let cr = host::r8(h, REG_CR);
    let fwctrl = host::r16(h, REG_MCUFW_CTRL);
    host::print("  CR (8)   = 0x");
    host::print_hex8(cr);
    host::print(if cr == CR_POWER_OFF { "  → MAC AUS\n" } else { "  → MAC an\n" });
    host::print("  MCUFWCTL = 0x");
    host::print_hex16(fwctrl);
    host::print(if fwctrl == MCUFW_CTRL_FW_ALIVE {
        "  → Firmware laeuft noch\n"
    } else {
        "  → keine laufende Firmware\n"
    });

    // ── Evaluation ───────────────────────────────────────────────
    host::print("[rtl8822ce] Chip: cut ");
    host::print_dec(hal.cut_version as u32);
    host::print(" (Maske 0x");
    host::print_hex8(cut_version_to_mask(hal.cut_version));
    host::print("), vendor ");
    host::print_dec(hal.vendor_id as u32);
    host::print(", mp_chip ");
    host::print_dec(hal.mp_chip as u32);
    host::print(", RF ");
    host::print(if hal.rf_2t2r { "2T2R" } else { "1T1R" });
    host::print(" (");
    host::print_dec(hal.rf_path_num as u32);
    host::print(" Pfade)\n");

    // ── Gates ────────────────────────────────────────────────────
    let widths_ok = check_access_widths(h, hal.chip_version);
    let mut all = true;

    all &= gate("Registerfenster antwortet", !dead);
    all &= gate("RF-Typ 2T2R (Datenblatt: 2x2)", hal.rf_2t2r && !dead);
    all &= gate("8/16/32-Bit-Zugriff stimmen ueberein", widths_ok && !dead);

    if !all {
        host::say("[rtl8822ce] Stufe 0: NEIN — Stufe 1 und 2a werden nicht gefahren\n");
        return;
    }
    host::print("[rtl8822ce] Stufe 0: GRUEN\n");

    // ── Stage 2a: rings (rtw_pci_setup_resource) ─────────────────
    // Linux order: rtw_power_on calls rtw_hci_setup, and with it
    // rtw_pci_setup, before rtw_mac_power_on. The ring registers live in the
    // PCIe block, independent of the MAC.
    let mut trx = match pci::init_trx_ring() {
        Some(t) => t,
        None => {
            host::say("[rtl8822ce] DMA reicht nicht fuer die Ringe — Stufe 2a aus\n");
            return;
        }
    };
    host::print("[rtl8822ce] Stufe 2a: Ringe belegt unter 1 GiB — ");
    host::print_dec(trx.dma_pages);
    host::print(" Seiten (");
    host::print_dec(trx.dma_pages * 4 / 1024);
    host::print(" MiB) in ");
    host::print_dec(trx.dma_allocs);
    host::print(" Stuecken, von 2048 Seiten / 1024 Stuecken\n");

    pci::setup(h, &mut trx, true); // = rtw_hci_setup: reset_trx_ring + dma_reset
    host::print("[rtl8822ce] Ringregister mit MAC AUS:\n");
    let rings_off_ok = pci::verify_rings(h, &trx);

    // ── Stage 1: power ───────────────────────────────────────────
    host::print("[rtl8822ce] Stufe 1: Power-Sequenz (");
    host::print_dec(pwr_cmds_for_us(hal.cut_version) as u32);
    host::print(" von 54 Kommandos gelten fuer PCIe + cut ");
    host::print_dec(hal.cut_version as u32);
    host::print(")\n");

    let t0 = host::now_us();
    let on = mac::mac_power_on(h, hal.cut_version);
    let dt_on = host::now_us() - t0;

    let on_ok = on.is_ok();
    if let Err(e) = on {
        host::say(match e {
            mac::PwrErr::Busy => "[rtl8822ce] Power-Sequenz abgebrochen (Polling)\n",
            mac::PwrErr::Already => "[rtl8822ce] Power-Sequenz: unerwartetes EALREADY\n",
        });
    }

    let cr_on = host::r8(h, REG_CR);
    let fsmco_on = host::r32(h, REG_SYS_PW_CTRL);
    let funcen_on = host::r8(h, REG_SYS_FUNC_EN + 1);
    host::print("  nach AN : CR = 0x");
    host::print_hex8(cr_on);
    host::print("  APS_FSMCO = 0x");
    host::print_hex32(fsmco_on);
    host::print("  SYS_FUNC_EN+1 = 0x");
    host::print_hex8(funcen_on);
    host::print("  (");
    host::print_dec(dt_on as u32);
    host::print(" us)\n");

    let pwr_on_ok = gate("MAC laeuft nach der Power-Sequenz (CR != 0xea)",
                         on_ok && cr_on != CR_POWER_OFF);

    // Same check with the MAC running. Linux programs the rings again after
    // the firmware download (`rtw_hci_setup` in `__rtw_download_firmware`,
    // "reset desc and index"), so check that nothing is lost in between.
    host::print("[rtl8822ce] dieselben Register mit MAC AN:\n");
    let rings_on_ok = pci::verify_rings(h, &trx);
    let rx_idx = host::r32(h, pci::RTK_PCI_RXBD_IDX_MPDUQ);
    host::print("  RXBD_IDX = 0x");
    host::print_hex32(rx_idx);
    host::print("  (HW-Schreibzeiger ");
    host::print_dec((rx_idx & pci::TRX_BD_HW_IDX_MASK) >> 16);
    host::print(", unser Lesezeiger ");
    host::print_dec(rx_idx & pci::TRX_BD_IDX_MASK);
    host::print(")\n");

    let rings_ok = gate("Ringregister halten ihre Werte (MAC aus UND an)",
                        rings_off_ok && rings_on_ok);

    // ── Stage 2b: firmware (rtw_download_firmware) ───────────────
    // Order as in `rtw_power_on`: hci_setup, mac_power_on, then the
    // download. The pages go through the BCN queue of a running MAC.
    let hdr = mac::parse_fw_hdr(FW);
    host::print("[rtl8822ce] Stufe 2b: Firmware v");
    host::print_dec(hdr.version as u32);
    host::print(".");
    host::print_dec(hdr.sub_version as u32);
    host::print(".");
    host::print_dec(hdr.sub_index as u32);
    host::print(", ");
    host::print_dec(FW.len() as u32);
    host::print(" Bytes, feature 0x");
    host::print_hex32(hdr.feature);
    host::print("\n");

    // `rtwdev->fifo` lives for the whole driver in Linux and is NULL until
    // the first `rtw_mac_init`. The download reads `rsvd_boundary` from it,
    // so it is 0 here, as there.
    let mut fifo = mac::Fifo::default();

    // The firmware feature bits decide which H2C commands it knows
    // (`rtw_fw_feature_check`). They are in the image header, so they are
    // available before the download.
    let fw_feature = mac::parse_fw_hdr(FW).feature;

    // `rtwdev->h2c`: one per device. The driver rotates through the four
    // mailboxes so the firmware has time to drain the previous one; a fresh
    // state per stage would restart at box 0.
    let mut h2c = fw::H2cState::default();

    let stage_buf = host::dma_alloc_below(
        (pci::RSVD_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    // The H2C ring needs its own staging buffer; the one above is for the
    // firmware download and exactly one chunk large.
    let h2c_buf = host::dma_alloc_below(
        (pci::H2C_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    // The MGMT queue needs a third: its frames are up to 2 KB, and the
    // 128-byte H2C slots cannot hold one.
    let mgmt_buf = host::dma_alloc_below(
        (pci::MGMT_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    let fw_ok = match stage_buf {
        st if st >= 0 => {
            let ok = mac::download_firmware(h, &mut trx, st, FW, BAND_AT_FWDL,
                                            fifo.rsvd_boundary);
            let ctrl = host::r16(h, REG_MCUFW_CTRL);
            host::print("  MCUFWCTL = 0x");
            host::print_hex16(ctrl);
            host::print(" (FW_READY waere 0x");
            host::print_hex16(FW_READY as u16);
            host::print(" unter Maske 0x");
            host::print_hex16(FW_READY_MASK as u16);
            host::print(")\n");
            ok
        }
        _ => {
            host::print("  kein DMA fuer den Zwischenpuffer\n");
            false
        }
    };
    let stage2b = gate("Firmware laeuft (MCUFW_CTRL liest FW_READY)", fw_ok);

    // ── Stage 2c: efuse and hw_feature ───────────────────────────
    let mut stage2c = false;
    let mut efuse = None;
    if stage2b {
        host::print("[rtl8822ce] Stufe 2c: efuse (");
        host::print_dec(efuse::PHYSICAL_SIZE as u32);
        host::print(" physisch -> ");
        host::print_dec(efuse::LOGICAL_SIZE as u32);
        host::print(" logisch)\n");
        if let Some(e) = efuse::efuse_info_setup(h, hal.rf_path_num) {
            host::print("  MAC  ");
            for (i, b) in e.addr.iter().enumerate() {
                if i > 0 { host::print(":"); }
                host::print_hex8(*b);
            }
            host::print("\n  rfe_option ");
            host::print_dec(e.rfe_option as u32);
            host::print(" · channel_plan 0x");
            host::print_hex8(e.channel_plan);
            host::print(" · crystal_cap ");
            host::print_dec(e.crystal_cap as u32);
            host::print(" · regd ");
            host::print_dec(e.regd as u32);
            host::print("\n  rf_board_option 0x");
            host::print_hex8(e.rf_board_option);
            host::print(" · btcoex ");
            host::print(if e.btcoex { "JA" } else { "nein" });
            host::print(" · share_ant ");
            host::print(if e.share_ant { "JA" } else { "nein" });
            host::print("\n  thermal A/B ");
            host::print_dec(e.thermal_meter[0] as u32);
            host::print("/");
            host::print_dec(e.thermal_meter[1] as u32);
            host::print(" · hw_cap nss ");
            host::print_dec(e.hw_cap_nss as u32);
            host::print(", ant ");
            host::print_dec(e.hw_cap_ant_num as u32);
            host::print(", bw 0x");
            host::print_hex8(e.hw_cap_bw);
            host::print(", hci 0x");
            host::print_hex8(e.hw_cap_hci);
            host::print("\n");

            // main.c: is_valid_ether_addr — not zero, not multicast.
            let valid = e.addr != [0u8; 6]
                && e.addr != [0xffu8; 6]
                && e.addr[0] & 0x01 == 0;
            stage2c = gate("MAC-Adresse aus der efuse ist gueltig", valid);
            efuse = Some(e);
        } else {
            let _ = gate("MAC-Adresse aus der efuse ist gueltig", false);
        }
    }

    // Power off again, as rtw_chip_efuse_info_setup does. Required from here
    // on: running firmware must not write into buffers the kernel frees on
    // return.
    mac::mac_power_off(h, hal.cut_version);
    let cr_off = host::r8(h, REG_CR);
    host::print("  nach AUS: CR = 0x");
    host::print_hex8(cr_off);
    host::print("  APS_FSMCO = 0x");
    host::print_hex32(host::r32(h, REG_SYS_PW_CTRL));
    host::print("\n");

    let pwr_off_ok = gate("MAC ist nach dem Abschalten wieder aus (CR == 0xea)",
                          cr_off == CR_POWER_OFF);

    let stage1 = pwr_on_ok && pwr_off_ok;
    let stage2a = rings_ok;
    stage_line(stage1,
        "[rtl8822ce] Stufe 1: GRUEN\n",
        "[rtl8822ce] Stufe 1: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage2a,
        "[rtl8822ce] Stufe 2a: GRUEN\n",
        "[rtl8822ce] Stufe 2a: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage2b,
        "[rtl8822ce] Stufe 2b: GRUEN\n",
        "[rtl8822ce] Stufe 2b: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage2c,
        "[rtl8822ce] Stufe 2c: GRUEN\n",
        "[rtl8822ce] Stufe 2c: NEIN — nicht weiterbauen, bevor das steht\n");

    // ── Stage 4b: rtw_chip_board_info_setup ──────────────────────
    // Runs before stage 3 because it precedes `rtw_power_on` in Linux:
    // `rtw_chip_info_setup` = parameter_setup -> efuse_info_setup ->
    // board_info_setup. Stage numbers are build order, not run order.
    let (stage4b, _txpwr) = match efuse.as_ref() {
        Some(e) if stage2c => stage4b_board_info_setup(e.rfe_option),
        _ => {
            host::say("[rtl8822ce] Stufe 4b: uebersprungen, 2c steht nicht\n");
            (false, None)
        }
    };

    // ── Stage 3a: rtw_power_on up to rtw_mac_init ────────────────
    //
    // Everything before was `rtw_chip_info_setup`, the probe-time cycle that
    // powers the chip only to read the efuse. This is the second cycle,
    // `rtw_power_on` (main.c:1374), starting from the beginning:
    //
    //     rtw_hci_setup -> rtw_mac_power_on -> rtw_download_firmware
    //                   -> rtw_mac_init
    //
    // The firmware is loaded a second time, as in Linux: the MAC was off in
    // between, and a powered-off MAC has no firmware.
    let mut stage3b = false;
    let mut stage3c = false;
    let stage3a = match (stage2c, efuse.as_ref()) {
        (true, Some(e)) => {
            host::print("[rtl8822ce] Stufe 3a: rtw_power_on (zweiter Zyklus) + rtw_mac_init\n");
            let ok = power_on_and_mac_init(h, &hal, &mut trx, stage_buf, &mut fifo);
            if ok {
                let (b, c) = phy_set_param_and_check(h, &hal, e, rtwdev);
                stage3b = b;
                stage3c = c;
            } else {
                host::say("[rtl8822ce] Stufe 3b/3c: uebersprungen, 3a steht nicht\n");
            }
            ok
        }
        _ => {
            host::say("[rtl8822ce] Stufe 3a: uebersprungen, 2c steht nicht\n");
            false
        }
    };

    // ── Stage 4a: rest of rtw_power_on and rtw_core_start ────────
    let stage4a = match (stage3c, efuse.as_ref()) {
        (true, Some(e)) => stage4a_power_on_tail(h, &hal, &mut trx, h2c_buf, &mut h2c,
                                                 &mut fifo, e, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 4a: uebersprungen, 3c steht nicht\n");
            false
        }
    };

    // ── Stage 4c: rtw_set_channel ────────────────────────────────
    let stage4c = match (stage4a && stage4b, efuse.as_ref(), _txpwr.as_ref()) {
        (true, Some(e), Some(t)) => stage4c_set_channel(h, &hal, e, t, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 4c: uebersprungen, 4a/4b stehen nicht\n");
            false
        }
    };

    // ── Stage 5a: receive path ───────────────────────────────────
    let stage5a = if stage4c {
        stage5a_rx(h, &hal, &mut trx, rtwdev)
    } else {
        host::say("[rtl8822ce] Stufe 5a: uebersprungen, 4c steht nicht\n");
        false
    };

    // ── Stage 5b: transmit path ──────────────────────────────────
    let stage5b = match (stage5a, efuse.as_ref()) {
        (true, Some(e)) => stage5b_tx(h, &hal, &mut trx, mgmt_buf, e.addr, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5b: uebersprungen, 5a steht nicht\n");
            false
        }
    };

    // ── Stage 5c: scan ───────────────────────────────────────────
    let mut target: Option<Bss> = None;
    let stage5c = match (stage5b, efuse.as_ref(), _txpwr.as_ref()) {
        (true, Some(e), Some(t)) => stage5c_scan(h, &hal, &mut trx, mgmt_buf,
                                                 &mut h2c, e, t, e.addr,
                                                 fw_feature, &mut target,
                                                 rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5c: uebersprungen, 5b steht nicht\n");
            false
        }
    };

    // ── Stage 5d: RF calibration ─────────────────────────────────
    let stage5d = match (stage5c, efuse.as_ref()) {
        (true, Some(e)) => stage5d_calibration(h, &hal, &mut trx, h2c_buf, &mut h2c, e, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5d: uebersprungen, 5c steht nicht\n");
            false
        }
    };

    // ── Stage 5e: auth and assoc ─────────────────────────────────
    let mut linked: Option<vif::Vif> = None;
    let stage5e = match (stage5d, efuse.as_ref(), _txpwr.as_ref(),
                         target.as_ref()) {
        (true, Some(e), Some(t), Some(b)) =>
            stage5e_connect(h, &hal, &mut trx, mgmt_buf, &mut h2c, e, t,
                            e.addr, b, &mut linked, rtwdev),
        (true, _, _, None) => {
            host::say("[rtl8822ce] Stufe 5e: uebersprungen, der Suchlauf\n             \x20         hat kein Ziel auf 2,4 GHz gefunden\n");
            false
        }
        _ => {
            host::say("[rtl8822ce] Stufe 5e: uebersprungen, 5d steht nicht\n");
            false
        }
    };

    // ── Stage 5f: rate adaptation ────────────────────────────────
    let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;
    let stage5f = match (stage5e, linked.as_ref(), target.as_ref()) {
        (true, Some(v), Some(b)) =>
            stage5f_rates(h, &mut trx, &mut h2c, &hal, v, b, &mut rates, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5f: uebersprungen, 5e steht nicht\n");
            false
        }
    };

    // ── Stage 6a: control port, handshake, data path ─────────────
    let mut link: Option<Link> = None;
    let mut lstats = LinkStats::default();
    let stage6a = match (stage5f, linked.as_ref(), target.as_ref(),
                         rates.as_ref(), efuse.as_ref(), _txpwr.as_ref()) {
        (true, Some(_v), Some(b), Some((caps, si)), Some(e), Some(tp)) =>
            stage6a_link(h, &hal, &mut trx, mgmt_buf, b, caps, *si,
                         e.addr, &mut link, &mut lstats, rtwdev, &mut h2c, e,
                         tp, fw_feature),
        _ => {
            host::say("[rtl8822ce] Stufe 6a: uebersprungen, 5f steht nicht\n");
            false
        }
    };


    stage_line(stage3a,
        "[rtl8822ce] Stufe 3a: GRUEN\n",
        "[rtl8822ce] Stufe 3a: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage3b,
        "[rtl8822ce] Stufe 3b: GRUEN\n",
        "[rtl8822ce] Stufe 3b: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage3c,
        "[rtl8822ce] Stufe 3c: GRUEN — BB und RF stehen\n",
        "[rtl8822ce] Stufe 3c: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage4a,
        "[rtl8822ce] Stufe 4a: GRUEN\n",
        "[rtl8822ce] Stufe 4a: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage4b,
        "[rtl8822ce] Stufe 4b: GRUEN\n",
        "[rtl8822ce] Stufe 4b: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage4c,
        "[rtl8822ce] Stufe 4c: GRUEN — DER EMPFAENGER HOERT\n",
        "[rtl8822ce] Stufe 4c: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5a,
        "[rtl8822ce] Stufe 5a: GRUEN — DIE PAKETE KOMMEN AN\n",
        "[rtl8822ce] Stufe 5a: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5b,
        "[rtl8822ce] Stufe 5b: GRUEN — WIR SENDEN, UND ES WIRD GEANTWORTET\n",
        "[rtl8822ce] Stufe 5b: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5c,
        "[rtl8822ce] Stufe 5c: GRUEN — WIR SEHEN DIE UMGEBUNG\n",
        "[rtl8822ce] Stufe 5c: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5d,
        "[rtl8822ce] Stufe 5d: GRUEN — DER SENDER IST KALIBRIERT\n",
        "[rtl8822ce] Stufe 5d: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5e,
        "[rtl8822ce] Stufe 5e: GRUEN — DER AP HAT UNS ANGENOMMEN\n",
        "[rtl8822ce] Stufe 5e: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage5f,
        "[rtl8822ce] Stufe 5f: GRUEN — DIE FIRMWARE WAEHLT DIE RATE\n",
        "[rtl8822ce] Stufe 5f: NEIN — nicht weiterbauen, bevor das steht\n");
    stage_line(stage6a,
        "[rtl8822ce] Stufe 6a: GRUEN — DER HANDSCHLAG IST DURCH\n",
        "[rtl8822ce] Stufe 6a: NEIN — nicht weiterbauen, bevor das steht\n");

    // ── Stage 6b: stay running ───────────────────────────────────
    //
    // From here the driver does not return; powering the chip off at the
    // end would drop the link. The summary is printed before, since nothing
    // runs after this point.
    if stage6a {
        if let (Some(e), Some(l)) = (efuse.as_ref(), link.as_mut()) {
            host::print("[rtl8822ce] Stufe 6b: der Treiber bleibt stehen —\n             \x20         Bericht je Sekunde, RX-Wachhund, kein\n             \x20         Abschalten mehr\n");
            // The one line of a silent run. It is placed after the 6a gates
            // so that it reports a connection, not an intermediate state.
            report_connected(l, target.as_ref(), linked.as_ref());
            // Same loop, same link, same counters: 6b continues rather than
            // restarts. A second `EV_READY` would make `wifid` build a fresh
            // supplicant waiting for an msg1 the AP never sends again.
            let caps = rates.as_ref().map(|(c, _)| *c).unwrap_or_default();
            // From here the link runs and can end. Reconnecting takes the
            // same path as connecting, stages 5e and 5f, without registering
            // with the kernel again.
            let mut fehlschlaege = 0u32;
            loop {
                let Some(tp) = _txpwr.as_ref() else { break };
                let end = link_pump(h, &hal, &mut trx, mgmt_buf, l,
                                    &mut lstats, e.addr, 0, rtwdev,
                                    &mut h2c, e, tp, &caps, fw_feature);
                // A roam takes the same path as a reconnect, stages 5e and
                // 5f, with a different cell. `reconnect` clears the keys,
                // updates the `Link` and has `wifid` build a fresh
                // supplicant.
                if end == PumpEnd::Roam {
                    let Some(z) = l.roam.to.take() else { break };
                    let t0 = host::now_ms();
                    target = Some(z);
                    if reconnect(h, &hal, &mut trx, mgmt_buf, &mut h2c, e, tp,
                                 &z, l, &mut lstats, rtwdev, &mut linked) {
                        host::loud_begin();
                        host::print("[rtl8822ce] gewechselt — Unterbruch ");
                        host::print_dec((host::now_ms() - t0) as u32);
                        host::print(" ms\n");
                        host::loud_end();
                        fehlschlaege = 0;
                    } else {
                        host::say("[rtl8822ce] der neue AP hat NICHT angenommen\n");
                        host::sleep_ms(RECONNECT_BACKOFF_MS);
                    }
                    continue;
                }
                if end != PumpEnd::LinkLost {
                    break;
                }
                let (Some(t), Some(b)) = (_txpwr.as_ref(), target) else {
                    // If this ever triggers, the driver ends: the caller
                    // powers off the MAC and the link stays down for good.
                    // It must not happen silently.
                    host::say("[rtl8822ce] kein Ziel mehr — der Treiber \
gibt auf\n");
                    break;
                };
                if reconnect(h, &hal, &mut trx, mgmt_buf, &mut h2c, e, t, &b,
                             l, &mut lstats, rtwdev, &mut linked) {
                    fehlschlaege = 0;
                    continue;
                }
                fehlschlaege += 1;
                // After two failures, scan again instead of retrying the
                // same BSSID and channel: a station that moved out of range
                // would otherwise retry an AP that is no longer there. A
                // full scan is fine here because the link is already down;
                // while the link is up, a directed scan on known channels
                // is used instead.
                if fehlschlaege >= RESCAN_AFTER_TRIES {
                    fehlschlaege = 0;
                    host::say("[rtl8822ce] zweimal vergeblich — die Umgebung wird neu abgesucht\n");
                    let _ = stage5c_scan(h, &hal, &mut trx, mgmt_buf,
                                         &mut h2c, e, t, e.addr, fw_feature,
                                         &mut target, rtwdev);
                }
                // Do not give up, but do not spin either: a restarting AP
                // needs seconds.
                host::sleep_ms(RECONNECT_BACKOFF_MS);
            }
        }
    }

    // Reached only when a stage failed: the kernel is about to free the DMA
    // buffers that running firmware would otherwise keep writing into.
    mac::mac_power_off(h, hal.cut_version);


    // Return rather than sleep. The kernel then frees DMA, unbinds PCI and
    // clears the report, so a dead driver's numbers do not look live; the
    // stage results are therefore printed to the terminal here and not
    // shown in `wlan`. The chip is already off, so nothing can write into
    // the buffers being freed. Only a failed run gets here (with a link, 6b
    // never returns), so this is loud even without `debug: 1`.
    host::say("[rtl8822ce] fertig — Chip ist aus, Geraet freigegeben\n");
}

/// main.c:2064-2081 `rtw_chip_board_info_setup`, stage 4b.
///
/// It runs here because Linux runs it here: `rtw_chip_info_setup` calls
/// `parameter_setup`, `efuse_info_setup`, then `board_info_setup`, all at
/// probe time before `rtw_power_on`. It touches no register; it fills the
/// tables `rtw_set_channel` later derives TX power from.
///
/// The gate needs no hardware: `gen_tables.py` computes the same chain in
/// Python and stores checksums over the derived state, so a single wrong
/// byte shows up here rather than as skewed TX power on one channel.
fn stage4b_board_info_setup(rfe_option: u8) -> (bool, Option<txpower::TxPower>) {
    host::print("[rtl8822ce] Stufe 4b: rtw_chip_board_info_setup (Sendeleistung)\n");

    let t0 = host::now_us();
    let t = match txpower::board_info_setup(rfe_option) {
        Some(t) => t,
        None => {
            host::print("  kein RFE-Satz fuer rfe_option ");
            host::print_dec(rfe_option as u32);
            host::print("\n");
            return (false, None);
        }
    };
    let dt = host::now_us() - t0;

    let got = txpower::checksums(&t);
    let want = tables::EXPECTED_TXPWR_SUMS;
    host::print("  Tabellen: bb_pg ");
    host::print_dec(tables::BB_PG_TYPE0.len() as u32);
    host::print(" Zeilen, txpwr_lmt ");
    host::print_dec(txpower::txpwr_lmt_tbl(rfe_option).map_or(0, |x| x.len()) as u32);
    host::print(" Zeilen  (");
    host::print_dec(dt as u32);
    host::print(" us)\n");

    const NAMEN: [&str; 6] = ["by_rate_offset_2g", "by_rate_offset_5g",
                              "by_rate_base_2g  ", "by_rate_base_5g  ",
                              "limit_2g         ", "limit_5g         "];
    let mut ok = true;
    for i in 0..6 {
        host::print("    ");
        host::print(NAMEN[i]);
        host::print(" 0x");
        host::print_hex32(got[i]);
        if got[i] == want[i] {
            host::print("  (erwartet)\n");
        } else {
            host::print("  <- ERWARTET 0x");
            host::print_hex32(want[i]);
            host::print("\n");
            ok = false;
        }
    }

    // A concrete sample: the FCC limit for 20 MHz, CCK, channel 1.
    host::print("  Beispiel: FCC/20MHz/CCK/Kanal 1 -> ");
    let v = t.limit_2g[0][0][0][0];
    if v < 0 {
        host::print("-");
        host::print_dec((-(v as i32)) as u32);
    } else {
        host::print_dec(v as u32);
    }
    host::print(", Basis CCK Pfad 0 ");
    let b = t.by_rate_base_2g[0][0];
    if b < 0 {
        host::print("-");
        host::print_dec((-(b as i32)) as u32);
    } else {
        host::print_dec(b as u32);
    }
    host::print("\n");

    let ok = gate("jede Pruefsumme der Sendeleistung stimmt mit der Nachrechnung",
                  ok);
    (ok, Some(t))
}

/// main.c:1374-1411 `rtw_power_on`, up to and including `rtw_mac_init`.
///
/// What follows (`phy_set_param`, `mac_postinit`, `hci_start`, the H2C
/// messages and coexistence) is stages 3b and 3c, deliberately not stubbed
/// here so a partial chain cannot pass for a complete one.
fn power_on_and_mac_init(
    h: i32, hal: &Hal, trx: &mut pci::Trx, stage_buf: i32, fifo: &mut mac::Fifo,
) -> bool {
    // rtw_hci_setup
    pci::setup(h, trx, false);

    // rtw_mac_power_on
    let t0 = host::now_us();
    if mac::mac_power_on(h, hal.cut_version).is_err() {
        host::say("  rtw_mac_power_on fehlgeschlagen\n");
        return false;
    }
    host::print("  MAC an nach ");
    host::print_dec((host::now_us() - t0) as u32);
    host::print(" us, CR = 0x");
    host::print_hex8(host::r8(h, REG_CR));
    host::print("\n");

    // rtw_wait_firmware_completion is not needed: the firmware is part of the
    // binary, there is no asynchronous load to wait for.

    // rtw_download_firmware
    if stage_buf < 0 {
        host::print("  kein DMA fuer den Zwischenpuffer\n");
        return false;
    }
    let t0 = host::now_us();
    if !mac::download_firmware(h, trx, stage_buf, FW, BAND_AT_FWDL,
                               fifo.rsvd_boundary) {
        host::say("  zweiter Firmware-Download fehlgeschlagen\n");
        return false;
    }
    host::print("  Firmware zum zweiten Mal geladen (");
    host::print_dec((host::now_us() - t0) as u32 / 1000);
    host::print(" ms)\n");

    // rtw_mac_init
    let t0 = host::now_us();
    let f = match mac::mac_init(h, hal.cut_version) {
        Ok(f) => f,
        Err(e) => {
            host::print(match e {
                mac::MacErr::NoMem =>
                    "  rtw_mac_init: Seitenplan passt nicht in den TX-FIFO\n",
                mac::MacErr::Inval =>
                    "  rtw_mac_init: eine Gegenrechnung stimmt nicht\n",
                mac::MacErr::Busy =>
                    "  rtw_mac_init: die Hardware quittiert nicht\n",
            });
            return false;
        }
    };
    let dt = host::now_us() - t0;
    *fifo = f;

    // The page plan in plain text. Every reserved page depends on it, and it
    // is printed nowhere else.
    host::print("  Seitenplan: txff ");
    host::print_dec(f.txff_pg_num as u32);
    host::print(" Seiten, rsvd ");
    host::print_dec(f.rsvd_pg_num as u32);
    host::print(", acq ");
    host::print_dec(f.acq_pg_num as u32);
    host::print("  ->  rsvd_boundary ");
    host::print_dec(f.rsvd_boundary as u32);
    host::print("\n  rsvd: drv ");
    host::print_dec(f.rsvd_drv_addr as u32);
    host::print(" · h2c_info ");
    host::print_dec(f.rsvd_h2c_info_addr as u32);
    host::print(" · h2c_sta ");
    host::print_dec(f.rsvd_h2c_sta_info_addr as u32);
    host::print(" · h2cq ");
    host::print_dec(f.rsvd_h2cq_addr as u32);
    host::print(" · fw_txbuf ");
    host::print_dec(f.rsvd_fw_txbuf_addr as u32);
    host::print(" · csibuf ");
    host::print_dec(f.rsvd_csibuf_addr as u32);
    host::print("  (");
    host::print_dec(dt as u32);
    host::print(" us)\n");

    // The stage gates: the two hardware acknowledgements and the two numbers
    // that follow from Linux' own computation.
    let llt = host::r8(h, REG_AUTO_LLT_V1) & BIT_AUTO_INIT_LLT_V1 as u8;
    let mut ok = true;
    ok &= gate("Link-List-Tabelle gebaut (AUTO_INIT_LLT_V1 geloescht)", llt == 0);
    ok &= gate("rsvd_boundary == 1938 (2048 Seiten minus 110 reservierte)",
               f.rsvd_boundary == 1938);
    // rtw_pci_interface_cfg on cut >= D.
    let mix = host::r32(h, REG_HCI_MIX_CFG);
    ok &= gate("PCIE_EMAC_PDN_AUX_TO_FAST_CLK steht (cut D)",
               mix & BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK != 0);
    // The MAC is running: REG_CR carries all eight TRX bits.
    let cr = host::r8(h, REG_CR);
    ok &= gate("REG_CR traegt MAC_TRX_ENABLE", cr & MAC_TRX_ENABLE == MAC_TRX_ENABLE);
    ok
}

/// main.c:1413 `chip->ops->phy_set_param`: stages 3b (tables) and 3c (BB/RF
/// setup) together, because `rtw_phy_load_tables` sits inside
/// `rtw8822c_phy_set_param`.
///
/// The two gates ask different questions. 3b: did the tables arrive? RF
/// register 0x00 of both paths holds the value the table wrote, neither 0
/// nor 0xfffff. 3c: did `phy_set_param` complete with both RF paths
/// answering.
fn phy_set_param_and_check(h: i32, hal: &Hal, e: &efuse::Efuse, d: &mut Dev)
    -> (bool, bool) {
    host::print("[rtl8822ce] Stufe 3b/3c: rtw8822c_phy_set_param\n");

    let dm = &mut d.dm;
    let path_div = &mut d.path_div;

    let t0 = host::now_us();
    let (tables_ok, dack_ok) = chip::phy_set_param(h, dm, path_div, e,
                                                   hal.cut_version, hal.rf_path_num,
                                                   hal.antenna_tx, hal.antenna_rx);
    host::print("  phy_set_param fertig in ");
    host::print_dec((host::now_us() - t0) as u32);
    host::print(" us\n");

    // ── Gate 3b ──────────────────────────────────────────────────
    // `rtw_phy_read_rf` uses the direct window; a path that does not answer
    // returns 0xfffff (all bits) or 0.
    let mut rf_ok = true;
    for path in [phy::RF_PATH_A, phy::RF_PATH_B] {
        let v0 = phy::read_rf(h, path, 0x00, phy::RFREG_MASK);
        let v18 = phy::read_rf(h, path, 0x18, phy::RFREG_MASK);
        host::print("  RF ");
        host::print(if path == phy::RF_PATH_A { "A" } else { "B" });
        host::print(": 0x00 = 0x");
        host::print_hex32(v0);
        host::print("  0x18 = 0x");
        host::print_hex32(v18);
        host::print("\n");
        rf_ok &= v0 != 0 && v0 != phy::RFREG_MASK
            && v18 != 0 && v18 != phy::RFREG_MASK;
    }
    let mut stage3b = gate("jede Tabelle gibt so viele Schreibzugriffe ab wie gerechnet",
                           tables_ok);
    stage3b &= gate("beide RF-Pfade antworten mit Tabellenwerten", rf_ok);

    // ── Gate 3c ──────────────────────────────────────────────────
    // The gate must not invent conditions Linux does not have. CCA events
    // cannot occur yet, in Linux either: `false_alarm_statistics` runs only
    // in the watchdog (main.c:280), after `rtw_coex_power_on_setting`
    // switches the shared antenna and after `rtw_set_channel` programs AGC,
    // CCA mask and RX filter, both stage 4. DAC calibration convergence is
    // not checked by Linux either: `rtw8822c_rf_dac_cal` runs ten times and
    // continues regardless. So the gate is that `phy_set_param` completed
    // and both RF paths answer; convergence is printed as a finding below.
    let stage3c = gate("phy_set_param lief durch, beide RF-Pfade antworten",
                       rf_ok);

    host::print(if dack_ok {
        "  [Befund] die DAC-Kalibrierung konvergiert auf beiden Pfaden\n"
    } else {
        "  [Befund] die DAC-Kalibrierung konvergiert NICHT auf beiden Pfaden.\n                    Linux prueft das nicht, wir haben kein Vergleichsmass —\n                    benannt und offen, siehe docs/plan/WIFI_RTL8822CE.md\n"
    });

    // Measured but not gated: the counters are expected to be 0 here. They
    // are logged because from stage 4 on they are the gate, and the baseline
    // is useful then.
    chip::false_alarm_statistics(h, dm);
    host::sleep_ms(50);
    chip::false_alarm_statistics(h, dm);

    host::print("  [Vorgriff Stufe 4, hier erwartungsgemaess 0]\n    Falschalarme: cck ");
    host::print_dec(dm.cck_fa_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_fa_cnt);
    host::print("\n    CCA: cck ");
    host::print_dec(dm.cck_cca_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_cca_cnt);
    host::print(" · gesamt ");
    host::print_dec(dm.total_cca_cnt);
    host::print("\n    CRC ok/err: cck ");
    host::print_dec(dm.cck_ok_cnt);
    host::print("/");
    host::print_dec(dm.cck_err_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_ok_cnt);
    host::print("/");
    host::print_dec(dm.ofdm_err_cnt);
    host::print("\n    IGI 0x");
    host::print_hex8(dm.igi_history[0]);
    host::print(" · cck_gi Grenzen u/l ");
    host::print_dec(dm.cck_gi_u_bnd as u32);
    host::print("/");
    host::print_dec(dm.cck_gi_l_bnd as u32);
    host::print("\n");

    (stage3b, stage3c)
}

/// main.c:1413-1434 the rest of `rtw_power_on`, then main.c:1517-1533
/// `rtw_core_start` up to the RCR write.
///
///     rtw_mac_postinit        NULL on the 8822C (rtw8822c.c:4967) -> nothing
///     rtw_hci_start           = rtw_pci_start: only enables interrupts.
///                               Deviation: see docs/plan/WIFI_RTL8822CE.md
///     rtw_fw_send_general_info    H2C packet through the H2C queue
///     rtw_fw_send_phydm_info      likewise
///     rtw_coex_power_on_setting   antenna to BT
///     rtw_coex_init_hw_config     then to INIT
///     rtw_sec_enable_sec_engine
///     rtw_write32(REG_RCR, hal->rcr)
fn stage4a_power_on_tail(h: i32, hal: &Hal, trx: &mut pci::Trx, h2c_buf: i32,
                         h2c: &mut fw::H2cState,
                         fifo: &mut mac::Fifo, e: &efuse::Efuse, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 4a: rtw_power_on (Rest) + rtw_core_start\n");

    if h2c_buf < 0 {
        host::print("  kein DMA fuer den H2C-Zwischenpuffer\n");
        return false;
    }

    let cx = &mut d.cx;

    // `rtw_mac_postinit`: `chip->ops->mac_postinit` is NULL on the 8822C,
    // the function returns without a register access.

    // `rtw_hci_start` = `rtw_pci_start`: sets `rtwpci->running` and calls
    // `rtw_pci_enable_interrupt`. The MSI is registered here
    // (`rtw_pci_request_irq`: one vector). HIMR is armed later in the pump
    // loop when idle; during bring-up the driver works through its steps in
    // order and waits on nothing.
    let vec = host::irq_register();
    // SAFETY: only this fiber reads and writes IRQ_VEC.
    unsafe { IRQ_VEC = vec; }
    if vec >= 0 {
        host::print("  MSI auf Vektor ");
        host::print_dec(vec as u32);
        host::print(" — Empfang per Interrupt\n");
    } else {
        host::print("  kein MSI — Abfragebetrieb\n");
    }

    // ── The two H2C packets ──────────────────────────────────────
    // They go through the H2C queue, not the mailbox. Its ring exists since
    // stage 3a (`init_h2c`).
    let wp_before = trx.tx[pci::Q_H2C].wp;
    let gi = fw::send_general_info(h, trx, h2c_buf, h2c, fifo);
    let pi = fw::send_phydm_info(h, trx, h2c_buf, h2c, e.rfe_option,
                                 hal.rf_2t2r, hal.cut_version,
                                 hal.antenna_rx, hal.antenna_tx);

    host::print("  H2C-Pakete: general_info ");
    host::print(if gi { "ok" } else { "FEHLER" });
    host::print(" (fw_tx_boundary ");
    host::print_dec((fifo.rsvd_fw_txbuf_addr - fifo.rsvd_boundary) as u32);
    host::print("), phydm_info ");
    host::print(if pi { "ok" } else { "FEHLER" });
    host::print("\n");

    // The chip fetches the entries itself: the upper twelve bits of the
    // index register are its read pointer. Wait for it rather than sample
    // once, since right after the kick it may still lag behind ours.
    let (consumed, dt_h2c, hw_idx) =
        pci::h2c_wait_consumed(h, trx, 10_000);
    host::print("  H2C-Queue: Schreibzeiger ");
    host::print_dec(wp_before);
    host::print(" -> ");
    host::print_dec(trx.tx[pci::Q_H2C].wp);
    host::print(", HW-Lesezeiger ");
    host::print_dec(hw_idx);
    host::print(if consumed { " (aufgeholt nach " } else { " (NICHT aufgeholt, " });
    host::print_dec(dt_h2c as u32);
    host::print(" us)\n");

    // ── Coexistence, and with it the antenna ─────────────────────
    // `wifi_only = !efuse->btcoex` (main.c:1431). With btcoex set in the
    // efuse it is false, and the INIT branch applies.
    let wifi_only = !e.btcoex;
    host::print("  Coex: btcoex ");
    host::print(if e.btcoex { "JA" } else { "nein" });
    host::print(", share_ant ");
    host::print(if e.share_ant { "JA" } else { "nein" });
    host::print(" -> wifi_only ");
    host::print(if wifi_only { "JA" } else { "nein" });
    host::print("\n");

    let scbd_before = coex::read_scbd_raw(h);
    let t0 = host::now_us();
    coex::power_on_setting(h, cx, h2c, e.share_ant, e.rfe_option);
    let scbd_poweron = coex::read_scbd_raw(h);
    coex::init_hw_config(h, cx, h2c, e.share_ant, wifi_only,
                         e.rfe_option);
    let scbd_after = coex::read_scbd_raw(h);
    let dt = host::now_us() - t0;

    // Raw, without the mask of `read_scbd`; otherwise a 0 cannot be told
    // apart from "our own write never arrived".
    host::print("  Score-Board roh: vorher 0x");
    host::print_hex16(scbd_before);
    host::print(", nach power_on 0x");
    host::print_hex16(scbd_poweron);
    host::print(", nach init 0x");
    host::print_hex16(scbd_after);
    host::print("  (wir schrieben 0x");
    host::print_hex16(cx.score_board | 0x8000);
    host::print(")\n  BT ");
    host::print(if cx.bt_disabled { "AUS" } else { "an" });
    host::print(", kt_ver ");
    host::print_dec(cx.kt_ver as u32);
    host::print(", ");
    host::print_dec(dt as u32);
    host::print(" us\n");

    // The point of this stage: where is the antenna?
    let ant = coex::read_ant_state(h);
    host::print("  Antenne: LTE_COEX_CTRL 0x");
    host::print_hex32(ant.lte_coex_ctrl);
    host::print("  GNT_WL ");
    host::print_dec(ant.gnt_wl);
    host::print(" GNT_BT ");
    host::print_dec(ant.gnt_bt);
    host::print("  Pfadbesitzer ");
    host::print(if ant.wifi_owns_path { "WLAN" } else { "BT" });
    host::print("\n");

    // ── rtw_core_start ───────────────────────────────────────────
    sec::enable_sec_engine(h);
    host::w32(h, REG_RCR, hal.rcr);

    let rcr = host::r32(h, REG_RCR);
    host::print("  RCR = 0x");
    host::print_hex32(rcr);
    host::print(" (geschrieben 0x");
    host::print_hex32(hal.rcr);
    host::print(")\n");

    // ── Gates ────────────────────────────────────────────────────
    let mut ok = true;
    ok &= gate("beide H2C-Pakete geschrieben", gi && pi);
    ok &= gate("die Firmware hat die H2C-Queue leergeraeumt", consumed);

    // The score board is not a gate. It is a shared mailbox whose relevant
    // bits are written by the BT core; with BT not running it reads 0, which
    // is correct. Linux never checks it. What can be checked is the effect:
    //
    // With `bt_disabled`, `set_ant_path(COEX_SET_ANT_INIT)` takes the branch
    // GNT_BT = SW_LOW (1), GNT_WL = SW_HIGH (3), giving the antenna to WLAN.
    // With BT running it is the other way round and the PTA shares it.
    let (want_wl, want_bt) = if cx.bt_disabled {
        (COEX_GNT_SET_SW_HIGH, COEX_GNT_SET_SW_LOW)
    } else {
        (COEX_GNT_SET_SW_LOW, COEX_GNT_SET_SW_HIGH)
    };
    ok &= gate("GNT_WL/GNT_BT stehen so, wie set_ant_path(INIT) sie setzt",
               ant.gnt_wl == want_wl && ant.gnt_bt == want_bt);
    ok &= gate("der Pfadbesitzer ist WLAN", ant.wifi_owns_path);
    ok &= gate("RCR steht auf hal->rcr", rcr == hal.rcr);

    // ── And the question 4a is for ───────────────────────────────
    let dm = &mut d.dm;
    chip::false_alarm_statistics(h, dm);
    host::sleep_ms(50);
    chip::false_alarm_statistics(h, dm);
    host::print("  [nach der Coex-Antenne] CCA: cck ");
    host::print_dec(dm.cck_cca_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_cca_cnt);
    host::print(" · gesamt ");
    host::print_dec(dm.total_cca_cnt);
    host::print("  ·  Falschalarme gesamt ");
    host::print_dec(dm.total_fa_cnt);
    host::print("\n");
    if dm.total_cca_cnt != 0 {
        host::print("  [Befund] der Empfaenger zaehlt SCHON OHNE Kanal — die\n         \x20          Antenne war der Grund. Stufe 4c wird es bestaetigen.\n");
    } else {
        host::print("  [Befund] weiter still. Dann fehlt der KANAL, und das\n         \x20          ist Stufe 4c (set_channel programmiert AGC und\n         \x20          CCA-Maske). Kein Widerspruch, nur die naechste Stufe.\n");
    }

    ok
}

/// main.c:1440-1476 `rtw_set_channel`, stage 4c.
///
///     rtw_get_channel_params      center channel, bandwidth and primary
///                                 position from the channel choice
///     rtw_update_channel          the same state in `hal`
///     chip->ops->set_channel      = rtw8822c_set_channel: BB, MAC, RF,
///                                   toggle_igi
///     rtw_coex_switchband_notify  deviation, see below
///     rtw_phy_set_tx_power_level  the 4b tables become a power index per
///                                 rate and path
///
/// The channel comes from us, not from mac80211: `rtw_get_channel_params`
/// reads a `cfg80211_chan_def` in Linux. For 20 MHz its result is exactly
/// `center = primary = channel`, which is what is used here.
fn stage4c_set_channel(h: i32, hal: &Hal, e: &efuse::Efuse,
                       t: &txpower::TxPower, d: &mut Dev) -> bool {
    // Channel 1, 20 MHz: the lowest 2.4 GHz channel is the one most likely
    // to carry traffic, which is what the measurement needs.
    const CH: u8 = 1;
    const BW: usize = 0; // RTW_CHANNEL_WIDTH_20
    const PRIMARY_IDX: u8 = RTW_SC_DONT_CARE;

    host::print("[rtl8822ce] Stufe 4c: rtw_set_channel (Kanal ");
    host::print_dec(CH as u32);
    host::print(", 20 MHz)\n");

    // `rtw_update_channel`: at 20 MHz the center channel is the primary, and
    // `cch_by_bw[20M]` holds it. The rest of `hal` (sar_band,
    // current_band_*) is kept as locals here.
    let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };
    t2.cch_by_bw[0] = CH;

    let t0 = host::now_us();
    chip::set_channel(h, CH, BW, PRIMARY_IDX);
    let dt_ch = host::now_us() - t0;

    // `rtw_coex_switchband_notify` belongs to running coexistence
    // (`rtw_coex_run_coex` with COEX_RSN_2GSWITCHBAND) and needs traffic
    // state that only a link has. Not implemented; it does not decide
    // whether the receiver hears.

    // `rtw_phy_set_tx_power_level`
    let t0 = host::now_us();
    let mut tbl = [[0u8; txpower::DESC_RATE_MAX]; txpower::RTW_RF_PATH_MAX];
    let idx: [txpower::TxPwrIdx; 4] = [
        txpower::TxPwrIdx(&e.txpwr_idx[0]), txpower::TxPwrIdx(&e.txpwr_idx[1]),
        txpower::TxPwrIdx(&e.txpwr_idx[2]), txpower::TxPwrIdx(&e.txpwr_idx[3]),
    ];
    txpower::set_tx_power_level(&t2, &idx, &mut tbl, hal.rf_path_num, CH, BW,
                                txpower::PHY_BAND_2G, e.regd as usize);
    chip::set_tx_power_index(h, hal.rf_path_num, &tbl);
    let dt_pwr = host::now_us() - t0;

    host::print("  set_channel ");
    host::print_dec(dt_ch as u32);
    host::print(" us, Sendeleistung ");
    host::print_dec(dt_pwr as u32);
    host::print(" us (regd ");
    host::print_dec(e.regd as u32);
    host::print(")\n  Leistungsindex Pfad A: 1M ");
    host::print_dec(tbl[0][0x00] as u32);
    host::print(" · 6M ");
    host::print_dec(tbl[0][0x04] as u32);
    host::print(" · MCS7 ");
    host::print_dec(tbl[0][0x13] as u32);
    host::print("  ·  Pfad B: 1M ");
    host::print_dec(tbl[1][0x00] as u32);
    host::print(" · 6M ");
    host::print_dec(tbl[1][0x04] as u32);
    host::print(" · MCS7 ");
    host::print_dec(tbl[1][0x13] as u32);
    host::print("\n");

    // RF 0x18 now carries band, channel and bandwidth; read back.
    let rf18_a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);
    let rf18_b = phy::read_rf(h, phy::RF_PATH_B, 0x18, phy::RFREG_MASK);
    host::print("  RF 0x18: A 0x");
    host::print_hex32(rf18_a);
    host::print(" B 0x");
    host::print_hex32(rf18_b);
    host::print("  (Kanal ");
    host::print_dec(rf18_a & 0xff);
    host::print(", Bandbreite 0x");
    host::print_hex8(((rf18_a >> 12) & 0x3) as u8);
    host::print(")\n");

    let mut ok = true;
    ok &= gate("RF 0x18 traegt auf beiden Pfaden den gesetzten Kanal",
               rf18_a & 0xff == CH as u32 && rf18_b & 0xff == CH as u32);
    // 20 MHz is RF18_BW_20M = BIT(13)|BIT(12), i.e. 0x3 in the field.
    ok &= gate("RF 0x18 traegt die Bandbreite 20 MHz",
               (rf18_a >> 12) & 0x3 == 0x3);

    // ── The receiver gate ────────────────────────────────────────
    let dm = &mut d.dm;
    chip::false_alarm_statistics(h, dm);
    host::sleep_ms(200);
    chip::false_alarm_statistics(h, dm);

    host::print("  Falschalarme: cck ");
    host::print_dec(dm.cck_fa_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_fa_cnt);
    host::print(" · gesamt ");
    host::print_dec(dm.total_fa_cnt);
    host::print("\n  CCA: cck ");
    host::print_dec(dm.cck_cca_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_cca_cnt);
    host::print(" · gesamt ");
    host::print_dec(dm.total_cca_cnt);
    host::print("\n  CRC ok/err: cck ");
    host::print_dec(dm.cck_ok_cnt);
    host::print("/");
    host::print_dec(dm.cck_err_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_ok_cnt);
    host::print("/");
    host::print_dec(dm.ofdm_err_cnt);
    host::print(" · ht ");
    host::print_dec(dm.ht_ok_cnt);
    host::print("/");
    host::print_dec(dm.ht_err_cnt);
    host::print("\n");

    ok &= gate("der Empfaenger zaehlt CCA-Ereignisse", dm.total_cca_cnt != 0);
    if dm.cck_ok_cnt + dm.ofdm_ok_cnt + dm.ht_ok_cnt > 0 {
        host::print("  [Befund] und er hat PAKETE mit gueltiger Pruefsumme\n\
         \x20          gesehen — das ist fremder Funkverkehr auf Kanal 1.\n");
    }

    ok
}

/// How many of the 54 power sequence commands apply to this cut.
fn pwr_cmds_for_us(cut: u8) -> usize {
    let m = cut_version_to_mask(cut);
    let mut n = 0;
    for seq in pwrseq::CARD_ENABLE_FLOW.iter().chain(pwrseq::CARD_DISABLE_FLOW.iter()) {
        for c in seq.iter() {
            if c.cmd != pwrseq::RTW_PWR_CMD_END
                && c.intf_mask & pwrseq::RTW_PWR_INTF_PCI_MSK != 0
                && c.cut_mask & m != 0
            {
                n += 1;
            }
        }
    }
    n
}

/// Stage 5a: the host around `pci::rx_poll`, not the port itself.
///
/// The port is `pci::rx_poll` (= `rtw_pci_rx_napi`); this only decides how
/// long to poll and what to report. Linux runs it from the interrupt in
/// NAPI; here the write pointer is polled.
fn stage5a_rx(h: i32, hal: &Hal, trx: &mut pci::Trx, d: &mut Dev) -> bool {
    /// Same channel as stage 4c (`hal.current_channel`).
    const CH_5A: u8 = 1;

    host::print("[rtl8822ce] Stufe 5a: der Empfangsweg\n");

    // `dm_info` carries the CCK gain limits a CCK packet's signal strength
    // is computed from. They are in hardware since `phy_set_param` read
    // them.
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    host::print("  cck_gi Grenzen u/l ");
    host::print_dec(dm.cck_gi_u_bnd as u32);
    host::print("/");
    host::print_dec(dm.cck_gi_l_bnd as u32);
    host::print("\n");

    // A full receive buffer. Static because this driver has no allocator
    // and 11 KB do not belong on the stack.
    static mut RXBUF: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: single-threaded, one caller, and the buffer does not leave
    // this function. No other path in this driver touches it.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF) };

    let mut total = 0u32;
    let mut c2h = 0u32;
    let mut crc = 0u32;
    let mut shown = 0u32;
    let mut best: i8 = -128;
    let mut rounds = 0u32;

    // 2000 ms. A beacon interval is 102.4 ms, so every reachable network
    // sends more than one frame in this time if the path works. Nothing in
    // two seconds is not a timing problem.
    let t0 = host::now_us();
    while host::now_us() - t0 < 2_000_000 {
        rounds += 1;
        let n = pci::rx_poll(h, trx, 64, buf, dm, path_div,
                             hal.rf_path_num, 0, CH_5A, |st, pkt| {
            total += 1;
            if st.is_c2h {
                c2h += 1;
                return;
            }
            if st.crc_err {
                crc += 1;
            }
            if st.signal_power > best {
                best = st.signal_power;
            }
            // Show the first eight in full, to see what arrives.
            if shown < 8 && !st.crc_err {
                shown += 1;
                print_pkt(st, pkt);
            }
        });
        if n == 0 {
            host::sleep_ms(1);
        }
    }

    host::print("  Runden ");
    host::print_dec(rounds);
    host::print(" · Pakete ");
    host::print_dec(total);
    host::print(" (c2h ");
    host::print_dec(c2h);
    host::print(", crc-Fehler ");
    host::print_dec(crc);
    host::print(")\n  staerkstes Signal ");
    print_dbm(best);
    host::print("\n  Ringzeiger rp=");
    host::print_dec(trx.rx.rp);
    host::print(" · rx_tag ");
    host::print_dec(trx.rx_tag as u32);
    host::print("\n");

    let mut ok = true;
    ok &= gate("der Ring liefert Pakete", total > 0);
    ok &= gate("und mindestens eins davon ist ein Funkrahmen mit\n\
         \x20         gueltiger Pruefsumme", total - c2h - crc > 0);
    ok &= gate("der PHY-Status traegt eine Signalstaerke", best > -128);
    ok
}

/// One line per packet: length, rate, bandwidth, channel, signal, and the
/// first bytes of the frame, since `frame_control` shows whether it is a
/// beacon.
fn print_pkt(st: &rx::RxPktStat, pkt: &[u8]) {
    let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
        + st.shift as usize;
    host::print("    len ");
    host::print_dec(st.pkt_len as u32);
    host::print(" · rate 0x");
    host::print_hex8(st.rate);
    host::print(" · bw ");
    host::print(match st.bw {
        0 => "20",
        1 => "40",
        _ => "80",
    });
    host::print(" · ch ");
    host::print_dec(st.channel as u32);
    host::print(" (");
    host::print_dec(st.freq as u32);
    host::print(" MHz) · ");
    print_dbm(st.signal_power);
    host::print(" · rssi ");
    host::print_dec(st.rssi as u32);
    if off + 2 <= pkt.len() {
        let fc = u16::from_le_bytes([pkt[off], pkt[off + 1]]);
        host::print(" · fc 0x");
        host::print_hex16(fc);
        // 802.11: type in bits 3:2, subtype in 7:4. 0x80 = beacon.
        if fc & 0xfc == 0x80 {
            host::print(" BEACON");
        }
    }
    host::print("\n");
}

fn print_dbm(v: i8) {
    if v < 0 {
        host::print("-");
        host::print_dec((-(v as i32)) as u32);
    } else {
        host::print_dec(v as u32);
    }
    host::print(" dBm");
}

/// Stage 5b: the transmit path, and the answer to it.
///
/// The port gets its address (`rtw_ops_add_interface`), a probe request goes
/// out and we listen. The gate is the response, not the consumed
/// descriptor: a fetched descriptor only shows that DMA works, a response
/// from an AP shows the frame left the antenna and was well-formed.
///
/// No calibration here. `rtw_set_channel` sets `need_rfk = true`, and
/// `rtw_chip_prepare_tx` runs GAPK/IQK/DPK only when mac80211 calls
/// `mgd_prepare_tx`, i.e. before association, not on channel switch; per
/// Linux' comment, calibrating on every channel during a scan takes too
/// long. A probe request goes out uncalibrated in Linux too.
fn stage5b_tx(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
              mac: [u8; 6], d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5b: der Sendeweg\n");
    if mgmt_buf < 0 {
        host::print("  kein DMA-Puffer fuer die MGMT-Queue\n");
        return false;
    }

    // `rtw_ops_add_interface`, STATION branch. Without it the port register
    // holds no address and `BIT_APM` in RCR lets only broadcast through,
    // while a probe response is addressed to us.
    let vif = vif::add_interface_station(h, mac);
    host::print("  Port 0: Adresse ");
    for (i, b) in mac.iter().enumerate() {
        if i > 0 {
            host::print(":");
        }
        host::print_hex8(*b);
    }
    host::print(" · net_type ");
    host::print_dec(vif.net_type);
    host::print(" · zurueckgelesen ");
    let mut back = [0u8; 6];
    for (i, b) in back.iter_mut().enumerate() {
        *b = host::r8(h, PORT0_MAC_ADDR + i as u32);
    }
    let addr_ok = back == mac;
    host::print(if addr_ok { "gleich" } else { "ANDERS" });
    host::print("\n");

    let mut ok = gate("die Adresse steht im Port-Register", addr_ok);

    // A probe request. In Linux `ieee80211_build_probe_req` builds it, in the
    // upper half, which is `wifid` here. It lives here so the transmit path
    // has something to send; stage 5c supersedes it.
    let mut frame = [0u8; 128];
    let n = build_probe_req(&mut frame, &mac, 1);
    let frame = &frame[..n];

    let mut info = tx::pkt_info_update(frame, vif.mac_id, tx::RTW_BAND_2G);
    let queue = tx::queue_mapping(
        u16::from_le_bytes([frame[0], frame[1]]), &frame[4..10]);
    host::print("  Rahmen ");
    host::print_dec(n as u32);
    host::print(" Bytes · Queue ");
    host::print_dec(queue as u32);
    host::print(" · qsel ");
    host::print_dec(pci::tx_qsel(queue) as u32);
    host::print(" · rate 0x");
    host::print_hex8(info.rate);
    host::print(" · rate_id ");
    host::print_dec(info.rate_id as u32);
    host::print("\n");
    ok &= gate("ein Verwaltungsrahmen geht in die MGMT-Queue",
               queue == tx::RTW_TX_QUEUE_MGMT);

    // Three times, spaced out: a single probe request can collide, and an AP
    // may simply drop it.
    let mut sent = 0u32;
    let mut consumed = 0u32;
    let mut last_us = 0u64;
    for _ in 0..3 {
        if !pci::tx_write(h, trx, mgmt_buf, queue, &mut info, frame) {
            break;
        }
        pci::tx_kick_off_queue(h, trx, queue);
        sent += 1;
        let (done, us, hw) = pci::tx_wait_consumed(h, trx, queue, 50_000);
        if done {
            consumed += 1;
            last_us = us;
        } else {
            host::print("  Deskriptor nicht abgeholt: hw ");
            host::print_dec(hw);
            host::print(" statt ");
            host::print_dec(trx.tx[queue].wp & pci::TRX_BD_IDX_MASK);
            host::print("\n");
        }
        host::sleep_ms(20);
    }
    host::print("  gesendet ");
    host::print_dec(sent);
    host::print(" · vom Chip abgeholt ");
    host::print_dec(consumed);
    host::print(" (zuletzt nach ");
    host::print_dec(last_us as u32);
    host::print(" us)\n");
    ok &= gate("der Chip holt die Sendedeskriptoren ab", consumed == sent
               && sent > 0);

    // Now listen. A probe response is subtype 5.
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF2: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: as in stage 5a: one thread, one caller, the buffer does not
    // leave this function.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF2) };

    let mut resp = 0u32;
    let mut shown = 0u32;
    let t0 = host::now_us();
    while host::now_us() - t0 < 1_000_000 {
        let n = pci::rx_poll(h, trx, 64, buf, dm, path_div,
                             hal.rf_path_num, 0, 1, |st, pkt| {
            if st.crc_err || st.is_c2h {
                return;
            }
            let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                + st.shift as usize;
            if off + 24 > pkt.len() {
                return;
            }
            let fc = u16::from_le_bytes([pkt[off], pkt[off + 1]]);
            // Subtype 5 = probe response, type 0 = management.
            if fc & 0xfc != 0x50 {
                return;
            }
            // `addr1` is our address; the filter would have dropped it
            // otherwise, but check anyway.
            if pkt[off + 4..off + 10] != mac[..] {
                return;
            }
            resp += 1;
            if shown < 6 {
                shown += 1;
                print_probe_resp(&pkt[off..], st);
            }
        });
        if n == 0 {
            host::sleep_ms(1);
        }
    }

    host::print("  Probe Responses an UNSERE Adresse: ");
    host::print_dec(resp);
    host::print("\n");
    if resp == 0 {
        host::print("  [Befund] auf Kanal 1 antwortet gerade niemand. Das ist\n         \x20         eine Aussage ueber die NACHBARSCHAFT, nicht ueber uns —\n         \x20         ein Kanal ist ein Muenzwurf. Der Beweis, dass der Rahmen\n         \x20         die Antenne verlaesst, faellt im Suchlauf ueber DREIZEHN\n         \x20         Kanaele (Stufe 5c).\n");
    }
    ok
}

/// `ieee80211_build_probe_req` in small: a wildcard probe request.
///
/// The sequence number stays zero: `en_hwseq` is set in the descriptor, so
/// the chip assigns it. Only what an AP needs to answer is built: the three
/// addresses, the empty SSID element and the rates.
fn build_probe_req(out: &mut [u8; 128], mac: &[u8; 6], ch: u8) -> usize {
    build_probe_req_to(out, mac, ch, None, &[])
}

/// `ieee80211_build_probe_req` with `IEEE80211_PROBE_FLAG_DIRECTED`
/// (net/mac80211/util.c, called from `ieee80211_ap_probereq_get`,
/// mlme.c:4518-4521).
///
/// A directed probe request asks exactly one AP "are you still there?":
/// receiver and BSSID are its address and the SSID element carries its
/// name instead of zero length. The scan calls it without a target, where
/// the empty SSID asks "who is there?".
fn build_probe_req_to(out: &mut [u8; 128], mac: &[u8; 6], ch: u8,
                      bssid: Option<&[u8; 6]>, ssid: &[u8]) -> usize {
    let bcast = [0xffu8; 6];
    let ziel = bssid.unwrap_or(&bcast);
    out[0..2].copy_from_slice(&0x0040u16.to_le_bytes()); // mgmt, subtype 4
    out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration
    out[4..10].copy_from_slice(ziel); // addr1 = receiver
    out[10..16].copy_from_slice(mac); // addr2 = us
    out[16..22].copy_from_slice(ziel); // addr3 = BSSID
    out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq, see above
    let mut n = 24;
    // SSID element. Length 0 = "any network", otherwise the one name.
    let sl = ssid.len().min(32);
    out[n] = 0;
    out[n + 1] = sl as u8;
    out[n + 2..n + 2 + sl].copy_from_slice(&ssid[..sl]);
    n += 2 + sl;
    // Supported Rates: 1, 2, 5.5, 11, 6, 9, 12, 18 Mbit. The high bit marks
    // a basic rate.
    out[n] = 1;
    out[n + 1] = 8;
    out[n + 2..n + 10]
        .copy_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    n += 10;
    // DS Parameter Set: the channel we probe on.
    out[n] = 3;
    out[n + 1] = 1;
    out[n + 2] = ch;
    n + 3
}

/// The channel load a cell reports itself, in percent.
///
/// Without the element nothing is printed; an invented zero would be a
/// claim.
fn print_last(b: &Bss) {
    if !b.bss_load_seen {
        return;
    }
    host::print("  belegt ");
    host::print_dec(b.bss_load as u32 * 100 / 255);
    host::print("%");
}

/// One line per response: BSSID, signal and the network name from the SSID
/// element.
fn print_probe_resp(f: &[u8], st: &rx::RxPktStat) {
    host::print("    von ");
    for i in 0..6 {
        if i > 0 {
            host::print(":");
        }
        host::print_hex8(f[16 + i]); // addr3 = BSSID
    }
    host::print(" · ");
    print_dbm(st.signal_power);
    host::print(" · SSID \"");
    // 24 header + 12 fixed fields (timestamp, interval, capabilities), then
    // the elements. The SSID element has ID 0.
    let mut i = 36;
    while i + 2 <= f.len() {
        let id = f[i];
        let len = f[i + 1] as usize;
        if i + 2 + len > f.len() {
            break;
        }
        if id == 0 {
            for &c in &f[i + 2..i + 2 + len] {
                let s = [if (0x20..0x7f).contains(&c) { c } else { b'.' }];
                host::print(unsafe { core::str::from_utf8_unchecked(&s) });
            }
            break;
        }
        i += 2 + len;
    }
    host::print("\"\n");
}

/// A discovered cell. Only what follows directly from a beacon or probe
/// response, nothing derived.
#[derive(Clone, Copy)]
struct Bss {
    bssid: [u8; 6],
    ssid: [u8; 32],
    ssid_len: u8,
    channel: u8,
    best: i8,
    beacons: u16,
    resps: u16,
    /// Capability field from beacon/probe response (802.11 §9.4.1.4).
    capability: u16,
    /// The AP's RSN element, raw. The association request picks the AP's
    /// suites from it; otherwise the AP rejects with status 43.
    rsn: [u8; 64],
    rsn_len: u8,
    /// Byte 1 of the HT Operation element (802.11 §9.4.2.56, id 61): bits
    /// 1:0 the secondary channel offset, bit 2 whether the AP allows more
    /// than 20 MHz at all. Without it there is no HT40: only the AP says
    /// which half the wide cell occupies.
    ht_param: u8,
    /// Whether the HT Operation element was present. Otherwise
    /// `ht_param == 0` means either "the AP runs 20 MHz" or "never seen".
    ht_op_seen: bool,
    /// Bytes 0:1 of the HT Capabilities element (id 45). Bit 1 is
    /// `SUP_WIDTH_20_40`: what the AP can do, while HT Operation says what
    /// it currently does.
    ht_cap: u16,
    /// Byte 0 of the VHT Operation element (802.11 §9.4.2.158, id 192): the
    /// width the cell runs. `0` = use the HT information, `1` = 80 MHz (and,
    /// with segment 1, also 160 and 80+80); `2` and `3` are the encodings
    /// for 160 and 80+80 deprecated in 802.11-2016.
    vht_chanwidth: u8,
    /// Byte 1: center channel segment 0. With width `1` this is the center
    /// of the primary 80 MHz, even if the AP runs 160, so an 80-only station
    /// finds its channel here (802.11 table 9-250; mac80211
    /// `ieee80211_chandef_vht_oper`, case `supp_chwidth == 0` → `ccf1 = 0`
    /// → `center_freq1 = cf0`).
    vht_cch0: u8,
    /// Byte 2: center channel segment 1, at 160 MHz the center of the whole
    /// 160. Read for the report only: our VHT capabilities say "no 160", and
    /// then per the same table the answer is segment 0.
    vht_cch1: u8,
    /// Whether the VHT Operation element was present, for the same reason
    /// as `ht_op_seen`.
    vht_op_seen: bool,
    /// The AP's "VHT Capabilities Info" field from its beacon, used only to
    /// trim our own offer.
    ///
    /// `bss_load` below is byte 2 of the BSS Load element (802.11
    /// §9.4.2.26, id 11): the share of time the AP sees its channel busy,
    /// as 0..255. It is the only beacon value that can reveal a range extender,
    /// whose backhaul shares the air and makes its channel look busier.
    /// Reported only, it decides nothing; wpa_supplicant does not use it
    /// either (`scan.c:3425`: `TODO: channel utilization and AP load`).
    ap_vht_cap: u32,
    ap_vht_cap_seen: bool,
    bss_load: u8,
    bss_load_seen: bool,
    /// Whether the AP announces WMM: `bss->wmm_used` in mac80211 (scan.c:139:
    /// `elems->wmm_param || elems->wmm_info`). It decides whether our
    /// association request carries the WMM element, and with it whether the
    /// AP grants VHT: hostapd strips VHT from a station without WMM element
    /// (`copy_sta_vht_capab`, ieee802_11_vht.c:200).
    wmm: bool,
}

const MAX_BSS: usize = 48;

/// What the cell says about its width: the raw bytes from its operation
/// elements, not their interpretation.
///
/// A value of its own so `chan_params` and `switch_channel` get the three
/// bytes in one piece. `CellWidth::default()` means "nothing known about
/// this cell", which is the state during a scan.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct CellWidth {
    /// Byte 1 of the HT Operation element (id 61).
    ht_param: u8,
    /// Byte 0 of the VHT Operation element (id 192).
    vht_chanwidth: u8,
    /// Byte 1 of the VHT Operation element: center channel segment 0.
    vht_cch0: u8,
}

impl Bss {
    fn width(&self) -> CellWidth {
        CellWidth {
            ht_param: self.ht_param,
            vht_chanwidth: self.vht_chanwidth,
            vht_cch0: self.vht_cch0,
        }
    }
}

/// How long to wait for a beacon after a channel switch before turning back.
/// A beacon interval is 102 ms; ten of them are plenty and still shorter
/// than the link watchdog.
const CSA_BEACON_WAIT_MS: u64 = 1000;

/// `IEEE80211_VHT_CHANWIDTH_80MHZ`, the only value a width is derived from.
/// `USE_HT` (0) falls back to HT; `160MHZ` (2) and `80P80MHZ` (3) are the
/// deprecated encodings in which segment 0 holds the center of the whole
/// 160 instead of our 80. No AP sends them since 802.11-2016, so they fall
/// back to HT and are named in the report.
const VHT_CHANWIDTH_80: u8 = 1;

/// The valid center channels of an 80 MHz block in the 5 GHz band. They are
/// fixed in the grid (802.11 annex E), each covering four 20 MHz channels:
/// 36-48, 52-64, 100-112, 116-128, 132-144, 149-161, 165-177.
const CENTERS_80: [u8; 7] = [42, 58, 106, 122, 138, 155, 171];

// ═══════════════════════════════════════════════════════════════
// Channel switch — CSA (802.11 §11.9, mac80211 spectmgmt.c:220-330 and
// mlme.c:2742-3024)
//
// An AP may move and announces it beforehand. On a DFS channel this is
// routine: on detecting radar it must vacate the channel within seconds
// (ETSI EN 301 893). A client that ignores the announcement stays behind on
// the empty channel and notices only when beacons stop.
// ═══════════════════════════════════════════════════════════════

/// What a channel switch announcement says.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Csa {
    /// `mode` (802.11 §9.4.2.19). 1 means: stop transmitting until the
    /// switch is done, the AP is vacating.
    mode: u8,
    /// The new primary channel.
    channel: u8,
    /// Number of beacon intervals until the switch. `0` and `1` both mean
    /// "now" (mlme.c:2991: `(max(count, 1) - 1) * beacon_int`).
    count: u8,
    /// The width after the switch.
    width: CellWidth,
}

/// `ieee80211_parse_ch_switch_ie` (spectmgmt.c:220), reduced to what a
/// beacon carries.
///
/// Four elements are read:
/// * 37 Channel Switch Announcement: `{mode, new channel, count}`
/// * 60 Extended CSA: the same, preceded by the operating class
/// * 62 Secondary Channel Offset: where the secondary channel lies after
/// * 194 Wide Bandwidth Channel Switch: VHT width and center, alone or in
///   wrapper 196
///
/// Deviation: Linux uses the operating class of element 60 to detect a band
/// change (`ieee80211_operating_class_to_band`). We do not track operating
/// classes, so element 37 wins when present and 60 is only a fallback. An
/// announcement into another band falls back on the channel plausibility
/// check that also guards `chan_params`.
fn parse_csa(f: &[u8]) -> Option<Csa> {
    // Beacon: 24 header + 8 timestamp + 2 beacon interval + 2 capabilities,
    // then the elements.
    if f.len() < 36 {
        return None;
    }
    let mut csa = Csa::default();
    let mut aus37 = false;
    let mut sec_offs = 0u8;
    let mut i = 36usize;
    while i + 2 <= f.len() {
        let id = f[i];
        let len = f[i + 1] as usize;
        if i + 2 + len > f.len() {
            break;
        }
        let b = &f[i + 2..i + 2 + len];
        match id {
            37 if len >= 3 => {
                csa.mode = b[0];
                csa.channel = b[1];
                csa.count = b[2];
                aus37 = true;
            }
            60 if len >= 4 && !aus37 => {
                // b[1] is the operating class, which we do not track.
                csa.mode = b[0];
                csa.channel = b[2];
                csa.count = b[3];
            }
            62 if len >= 1 => sec_offs = b[0],
            194 if len >= 3 => {
                csa.width.vht_chanwidth = b[0];
                csa.width.vht_cch0 = b[1];
            }
            // 196 Channel Switch Wrapper, containing 194.
            196 => {
                let mut j = 0usize;
                while j + 2 <= b.len() {
                    let sid = b[j];
                    let slen = b[j + 1] as usize;
                    if j + 2 + slen > b.len() {
                        break;
                    }
                    if sid == 194 && slen >= 3 {
                        csa.width.vht_chanwidth = b[j + 2];
                        csa.width.vht_cch0 = b[j + 3];
                    }
                    j += 2 + slen;
                }
            }
            _ => {}
        }
        i += 2 + len;
    }
    if csa.channel == 0 {
        // spectmgmt.c:278 „nothing here we understand"
        return None;
    }
    // The secondary channel offset is translated into an HT Operation byte
    // so `chan_params` understands it: one computation, one answer. Bit 2
    // (`WIDTH_ANY`) must be set, otherwise 20 MHz applies.
    csa.width.ht_param = match sec_offs {
        1 => 0x05, // secondary channel above
        3 => 0x07, // secondary channel below
        // spectmgmt.c:307-310: without the element the position after the
        // switch is unknown, and 20 MHz is the best we can say.
        _ => 0x00,
    };
    Some(csa)
}

/// main.c:822-867 `rtw_get_channel_params` and main.c:759-792, the part of
/// `rtw_update_channel` that picks the primary channel position, merged
/// because both run the same case analysis and we have no `chandef`, only
/// the AP's HT Operation byte.
///
/// Linux works in frequencies (`primary_freq > center_freq`), we in channel
/// numbers. That is the same statement: one channel step is 5 MHz.
///
/// ```text
///   secondary above (0x1): center = primary + 2, primary is the lower
///   secondary below (0x3): center = primary - 2, primary is the upper
/// ```
///
/// 80 MHz comes from the VHT Operation element. `rtw_get_channel_params`
/// distinguishes four positions of the primary 20 at 80 MHz
/// (main.c:769-791), and the center comes from the element, not from a
/// computation.
///
/// ```text
///   |primary - center| == 2 : primary is an inner quarter
///                             -> RTW_SC_20_UPPER / _LOWER
///   |primary - center| == 6 : it is an outer one
///                             -> RTW_SC_20_UPMOST / _LOWEST
/// ```
///
/// `max_bw` is the limit the caller allows: `0` during a scan (any width
/// above 20 MHz would be a claim about the neighbor channel), otherwise the
/// minimum of card and `bw:` from the config.
fn chan_params(primary: u8, w: CellWidth, max_bw: usize) -> (u8, usize, u8) {
    const SEC_OFFSET: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_OFFSET
    const SEC_ABOVE: u8 = 0x01; // IEEE80211_HT_PARAM_CHA_SEC_ABOVE
    const SEC_BELOW: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_BELOW
    const WIDTH_ANY: u8 = 0x04; // IEEE80211_HT_PARAM_CHAN_WIDTH_ANY

    // Both conditions, not one. An AP may name the secondary channel and
    // still forbid the width (bit 2 clear) while protecting a 20 MHz
    // neighbor; reading only the offset would send 40 MHz into a 20 MHz
    // cell.
    //
    // The bit also applies to 80 MHz: 802.11 calls it "STA Channel Width",
    // meaning anything above 20 MHz is currently forbidden. A VHT Operation
    // element does not change that, so the check precedes the VHT branch.
    if max_bw == 0 || w.ht_param & WIDTH_ANY == 0 {
        return (primary, 0, RTW_SC_DONT_CARE);
    }

    // ── 80 MHz ───────────────────────────────────────────────────
    //
    // Take segment 0 and compute nothing. Our VHT capabilities report
    // `supp_chan_width = 0` (no 160, no 80+80), and for that case 802.11
    // table 9-250 has segment 0 carry the center of our 80 MHz, even at a
    // 160 MHz AP that puts its 160 center into segment 1. mac80211 does the
    // same (`ccf1 = 0` → `center_freq1 = cf0`).
    //
    // Checked like the 40 MHz case because the input is a byte from a
    // foreign beacon: the center must be a center channel of the 80 MHz
    // grid and the primary must be one of its four quarters. Otherwise the
    // 40 MHz path below applies; a narrower width is always allowed, an
    // invented center never.
    if max_bw >= 2 && primary > 14 && w.vht_chanwidth == VHT_CHANWIDTH_80 {
        let c = w.vht_cch0;
        let d = if c > primary { c - primary } else { primary - c };
        if CENTERS_80.contains(&c) && (d == 2 || d == 6) {
            // main.c:769-791 in channels instead of frequencies: 10 MHz are
            // two channel steps, 30 MHz are six.
            let idx = match (primary > c, d) {
                (true, 2) => RTW_SC_20_UPPER,
                (true, _) => RTW_SC_20_UPMOST,
                (false, 2) => RTW_SC_20_LOWER,
                (false, _) => RTW_SC_20_LOWEST,
            };
            return (c, 2, idx);
        }
    }

    // ── 40 MHz ───────────────────────────────────────────────────
    let center = match w.ht_param & SEC_OFFSET {
        SEC_ABOVE => primary.saturating_add(2),
        SEC_BELOW => primary.saturating_sub(2),
        _ => return (primary, 0, RTW_SC_DONT_CARE),
    };

    // An addition over Linux: `rtw88` receives a `cfg80211_chan_def` that
    // cfg80211 already validated (`cfg80211_chandef_valid`). We have no
    // cfg80211; our input is a byte from a foreign beacon, and "secondary
    // above" on channel 13 would compute channel 15, which does not exist
    // and has no TX power entry. So the center channel is checked against
    // the primary's band; if it falls outside, 20 MHz applies.
    let plausibel = if primary <= 14 {
        (1..=13).contains(&center)
    } else {
        (36..=165).contains(&center)
    };
    if !plausibel {
        return (primary, 0, RTW_SC_DONT_CARE);
    }

    // main.c:766-768: a primary above the center is the upper half, so with
    // "secondary above" it is the lower one.
    if center > primary {
        (center, 1, RTW_SC_20_LOWER)
    } else {
        (center, 1, RTW_SC_20_UPPER)
    }
}

/// main.c:880-913 `rtw_set_channel`, the part that fits any channel.
///
/// `primary` is the channel the cell sends beacons on, `w` what it says
/// about its width. The scan calls with `max_bw = 0`: on a channel that is
/// only listened to, any width above 20 MHz is a claim about the neighbor.
///
/// The chip gets the center channel, not the primary (main.c:817
/// `hal->current_channel = center_channel`), so the RF 0x18 cross-check
/// compares against the center too.
fn switch_channel(h: i32, hal: &Hal, e: &efuse::Efuse, t: &txpower::TxPower,
                  primary: u8, w: CellWidth, max_bw: usize) -> bool {
    let (ch, bw, primary_idx) = chan_params(primary, w, max_bw);

    // `rtw_update_channel`: the 20 MHz entry is always the primary channel,
    // the entry of the current width the center (main.c:754-757).
    let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };
    t2.cch_by_bw[0] = primary;
    t2.cch_by_bw[bw] = ch;
    // At 80 MHz the 40 MHz entry must be filled too:
    // `rtw_phy_get_tx_power_limit` takes the minimum over all widths from
    // 20 up to the current one (phy.c:2149-2196) and looks up
    // `cch_by_bw[1]`. A zero there makes `channel_to_idx` find no channel,
    // and the limit would vanish entirely.
    //
    // main.c:777-791: the 40 MHz center lies in the same half of the 80 as
    // the primary, four steps from the 80 MHz center.
    if bw == 2 {
        t2.cch_by_bw[1] = if primary > ch { ch + 4 } else { ch - 4 };
    }

    chip::set_channel(h, ch, bw, primary_idx);

    // Linux calls `rtw_coex_switchband_notify` here, with three reasons by
    // band and scan state. It leads into `rtw_coex_run_coex`, the running
    // coexistence that needs traffic and BT state. Not implemented.

    let band = if ch > 14 { txpower::PHY_BAND_5G } else { txpower::PHY_BAND_2G };
    let mut tbl = [[0u8; txpower::DESC_RATE_MAX]; txpower::RTW_RF_PATH_MAX];
    let idx: [txpower::TxPwrIdx; 4] = [
        txpower::TxPwrIdx(&e.txpwr_idx[0]), txpower::TxPwrIdx(&e.txpwr_idx[1]),
        txpower::TxPwrIdx(&e.txpwr_idx[2]), txpower::TxPwrIdx(&e.txpwr_idx[3]),
    ];
    txpower::set_tx_power_level(&t2, &idx, &mut tbl, hal.rf_path_num, ch, bw,
                                band, e.regd as usize);
    chip::set_tx_power_index(h, hal.rf_path_num, &tbl);

    // `need_rfk` is not set while scanning; that is the point of the
    // `RTW_FLAG_SCANNING` branch: calibrating on every channel takes too
    // long. Calibration belongs before association.

    // Cross-check: RF 0x18 carries band, channel and bandwidth. Two reads
    // that tell whether the chip accepted the channel, which, unlike
    // whether anyone transmits there, is up to the driver.
    let a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);
    let b = phy::read_rf(h, phy::RF_PATH_B, 0x18, phy::RFREG_MASK);
    a & 0xff == ch as u32 && b & 0xff == ch as u32
}

/// Stage 5c: the scan.
///
/// Active on 2.4 GHz, passive on 5 GHz. Active means a probe request, then
/// listen; passive means only listen. Which 5 GHz channels may be
/// transmitted on depends on the regulatory domain, and those rules belong
/// to the upper half (`wifid`/cfg80211), not the driver. Receiving is
/// allowed everywhere, so the scan listens where it may not ask.
#[allow(clippy::too_many_arguments)]
fn stage5c_scan(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
                h2c: &mut fw::H2cState,
                e: &efuse::Efuse, t: &txpower::TxPower, mac: [u8; 6],
                fw_feature: u32, target: &mut Option<Bss>, d: &mut Dev) -> bool {
    // 2.4 GHz: the thirteen channels allowed in Europe. Channel 14 is
    // allowed only in Japan and only with DSSS, so it is left out.
    const ACTIVE_2G: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    // 5 GHz: UNII-1 to UNII-3 as cfg80211 lists them. Listen only.
    const PASSIVE_5G: [u8; 25] = [
        36, 40, 44, 48, 52, 56, 60, 64,
        100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
        149, 153, 157, 161, 165,
    ];
    // A beacon interval is 102.4 ms. Listening shorter misses a network
    // because of the clock, not a weak signal.
    const DWELL_MS: u32 = 130;

    host::print("[rtl8822ce] Stufe 5c: der Suchlauf\n");
    if mgmt_buf < 0 {
        host::print("  kein DMA-Puffer fuer die MGMT-Queue\n");
        return false;
    }

    // `rtw_core_scan_start`. The address is in the port since 5b (mac80211
    // may pass a randomized one here; we use our own). `rtw_leave_lps` and
    // `RTW_FLAG_DIG_DISABLE` have no effect here: there is neither power
    // saving nor running gain control. `rtw_coex_scan_notify` is the same
    // gap as above.
    let notify = fw_feature & FW_FEATURE_NOTIFY_SCAN != 0;
    host::print("  fw feature 0x");
    host::print_hex32(fw_feature);
    host::print(if notify {
        " · NOTIFY_SCAN ja\n"
    } else {
        " · NOTIFY_SCAN nein\n"
    });
    if notify {
        let ok = fw::scan_notify(h, h2c, true);
        host::print("  scan_notify(start) ");
        host::print(if ok { "raus" } else { "FEHLGESCHLAGEN" });
        host::print(" · HMETFR 0x");
        host::print_hex8(fw::hmetfr(h));
        host::print("\n");
    }

    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF3: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: one thread, one caller, the buffer does not leave the function.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF3) };

    let mut found: [Bss; MAX_BSS] = [Bss {
        bssid: [0; 6], ssid: [0; 32], ssid_len: 0,
        channel: 0, best: -128, beacons: 0, resps: 0,
        capability: 0, rsn: [0; 64], rsn_len: 0, ht_param: 0,
        ht_op_seen: false, ht_cap: 0,
        vht_chanwidth: 0, vht_cch0: 0, vht_cch1: 0, vht_op_seen: false,
        ap_vht_cap: 0, ap_vht_cap_seen: false,
        bss_load: 0, bss_load_seen: false, wmm: false,
    }; MAX_BSS];
    let mut n_found = 0usize;
    let mut probes = 0u32;
    let mut overflow = false;

    let mut frame = [0u8; 128];
    let t_start = host::now_us();
    let mut rf_ok = 0u32;
    let mut rf_bad_first = 0u8;
    let mut n_ch = 0u32;

    for (list, active) in [(&ACTIVE_2G[..], true), (&PASSIVE_5G[..], false)] {
        for &ch in list {
            n_ch += 1;
            if switch_channel(h, hal, e, t, ch, CellWidth::default(), 0) {
                rf_ok += 1;
            } else if rf_bad_first == 0 {
                rf_bad_first = ch;
            }

            if active {
                let n = build_probe_req(&mut frame, &mac, ch);
                let f = &frame[..n];
                let mut info =
                    tx::pkt_info_update(f, 0, tx::band_of(ch));
                let q = tx::RTW_TX_QUEUE_MGMT;
                if pci::tx_write(h, trx, mgmt_buf, q, &mut info, f) {
                    pci::tx_kick_off_queue(h, trx, q);
                    probes += 1;
                }
            }

            let t0 = host::now_us();
            while host::now_us() - t0 < DWELL_MS as u64 * 1000 {
                let got = pci::rx_poll(h, trx, 64, buf, dm, path_div,
                                       hal.rf_path_num, 0, ch, |st, pkt| {
                    if st.crc_err || st.is_c2h {
                        return;
                    }
                    let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                        + st.shift as usize;
                    if off + 36 > pkt.len() {
                        return;
                    }
                    let fc = u16::from_le_bytes([pkt[off], pkt[off + 1]]);
                    // Beacon (0x80) and probe response (0x50) share the same
                    // body: 12 fixed bytes, then elements.
                    let is_beacon = fc & 0xfc == 0x80;
                    let is_resp = fc & 0xfc == 0x50;
                    if !is_beacon && !is_resp {
                        return;
                    }
                    if !record_bss(&mut found, &mut n_found, &pkt[off..], ch,
                                   st.signal_power, is_beacon) {
                        overflow = true;
                    }
                });
                if got == 0 {
                    host::sleep_ms(1);
                }
            }
        }
    }

    // `rtw_core_scan_complete`
    if notify {
        let ok = fw::scan_notify(h, h2c, false);
        host::print("  scan_notify(stop) ");
        host::print(if ok { "raus" } else { "FEHLGESCHLAGEN" });
        host::print(" · HMETFR 0x");
        host::print_hex8(fw::hmetfr(h));
        host::print("\n");
    }
    // Back to the channel the earlier stages measured on.
    let _ = switch_channel(h, hal, e, t, 1, CellWidth::default(), 0);

    let dauer = (host::now_us() - t_start) / 1000;
    host::print("  ");
    host::print_dec(ACTIVE_2G.len() as u32);
    host::print(" Kanaele aktiv + ");
    host::print_dec(PASSIVE_5G.len() as u32);
    host::print(" passiv · ");
    host::print_dec(DWELL_MS);
    host::print(" ms je Kanal · ");
    host::print_dec(dauer as u32);
    host::print(" ms · ");
    host::print_dec(probes);
    host::print(" Probe Requests\n");

    let mut n_2g = 0u32;
    let mut n_5g = 0u32;
    let mut n_resp = 0u32;
    for b in found[..n_found].iter() {
        if b.channel > 14 { n_5g += 1 } else { n_2g += 1 }
        n_resp += b.resps as u32;
    }
    host::print("  Probe Responses insgesamt: ");
    host::print_dec(n_resp);
    host::print("\n  gefunden: ");
    host::print_dec(n_found as u32);
    host::print(" Netze (");
    host::print_dec(n_2g);
    host::print(" auf 2,4 GHz, ");
    host::print_dec(n_5g);
    host::print(" auf 5 GHz)\n");
    if overflow {
        host::print("  [Hinweis] mehr als ");
        host::print_dec(MAX_BSS as u32);
        host::print(" Netze — die Liste ist voll, nicht die Luft\n");
    }

    for b in found[..n_found].iter() {
        host::print("    ");
        for i in 0..6 {
            if i > 0 {
                host::print(":");
            }
            host::print_hex8(b.bssid[i]);
        }
        host::print(" · K");
        host::print_dec(b.channel as u32);
        if b.channel < 10 {
            host::print(" ");
        }
        if b.channel < 100 {
            host::print(" ");
        }
        host::print(" · ");
        print_dbm(b.best);
        host::print(" · B");
        host::print_dec(b.beacons as u32);
        host::print("/R");
        host::print_dec(b.resps as u32);
        host::print(" · \"");
        print_ssid(&b.ssid[..b.ssid_len as usize]);
        host::print("\"\n");
    }

    host::print("  RF 0x18 bestaetigt ");
    host::print_dec(rf_ok);
    host::print(" von ");
    host::print_dec(n_ch);
    host::print(" Kanaelen");
    if rf_bad_first != 0 {
        host::print(" (erster Fehlschlag: Kanal ");
        host::print_dec(rf_bad_first as u32);
        host::print(")");
    }
    host::print("\n");

    // The target comes from the SSID filter in `sys/config/wifi`, not from
    // signal strength. docs/spec/WIFI_CLASS_ABI.md gives the reason: without
    // it the driver picks the loudest AP of any network, possibly a
    // neighbor's, for which `wifid` has no PSK; that ends in a silent MIC
    // failure. One file, `key: value` per line, the same one `wifid` and
    // `wifi_ax200` read, so the network cannot diverge from the PSK.
    let mut cfg = [0u8; 512];
    let cn = host::fetch("sys/config/wifi", &mut cfg);
    let want = if cn > 0 {
        cfg_get(&cfg[..cn as usize], b"ssid")
    } else {
        None
    };
    host::print("  sys/config/wifi ssid: ");
    match want {
        Some((a, b)) => {
            host::print("\"");
            print_ssid(&cfg[a..b]);
            host::print("\"");
        }
        None => host::print("nicht gesetzt — der lauteste AP wird genommen"),
    }
    host::print("\n");

    // 5 GHz cells are selectable. The scan covers them passively (see
    // `PASSIVE_5G`), so no probe request goes out there, which is the care
    // a DFS channel requires: listen first, then transmit.
    //
    // `Auto` takes 5 GHz when it is usable, otherwise the strongest overall.
    // Pure strongest-signal selection would be wrong: a 2.4 GHz AP in the
    // same room is almost always louder than its 5 GHz twin.
    let pref = read_band_pref();
    let mut best_all: Option<Bss> = None;
    let mut best_5g: Option<Bss> = None;
    for b in found[..n_found].iter() {
        if b.ssid_len == 0 {
            continue;
        }
        let band_ok = match pref {
            BandPref::Only24 => b.channel <= 14,
            BandPref::Only5 => b.channel > 14,
            BandPref::Auto => true,
        };
        if !band_ok {
            continue;
        }
        let ssid_ok = match want {
            Some((a, c)) => b.ssid_len as usize == c - a
                && b.ssid[..c - a] == cfg[a..c],
            None => true,
        };
        if !ssid_ok {
            continue;
        }
        if best_all.map_or(true, |x| b.best > x.best) {
            best_all = Some(*b);
        }
        if b.channel > 14 && best_5g.map_or(true, |x| b.best > x.best) {
            best_5g = Some(*b);
        }
    }
    host::print("  band: ");
    host::print(match pref {
        BandPref::Auto => "auto (5 GHz ab -70 dBm)",
        BandPref::Only24 => "nur 2,4 GHz",
        BandPref::Only5 => "nur 5 GHz",
    });
    let gewaehlt = match (pref, best_5g) {
        (BandPref::Auto, Some(f)) if f.best >= PREFER_5G_DBM => {
            host::print(" -> 5 GHz genommen\n");
            Some(f)
        }
        (BandPref::Auto, Some(f)) => {
            host::print(" -> 5 GHz zu schwach (");
            print_dbm(f.best);
            host::print("), 2,4 GHz\n");
            best_all
        }
        _ => {
            host::print("\n");
            best_all
        }
    };
    // A scan without a result does not clear the target. The reconnect path
    // calls this scan after two failures, and a `None` there breaks out of
    // the loop and ends the driver. A channel where nobody answers right now
    // does not prove the cell is gone.
    match (gewaehlt, *target) {
        (Some(g), _) => *target = Some(g),
        (None, Some(alt)) => {
            host::print("  nichts gefunden — das letzte bekannte Ziel bleibt: K");
            host::print_dec(alt.channel as u32);
            host::print("\n");
        }
        (None, None) => {}
    }
    // The candidates, by signal. Selection is by signal strength, which
    // assumes the loudest network is also the fastest; a nearby range extender
    // with less width can beat a farther AP with more. The report lists what
    // each cell can do, computed by the same function that later sets the
    // channel.
    if n_found > 0 {
        host::print("  Zellen nach Signal:\n");
        let mut gezeigt = [false; MAX_BSS];
        for _ in 0..n_found.min(8) {
            let mut best: Option<usize> = None;
            for i in 0..n_found {
                if gezeigt[i] || found[i].ssid_len == 0 {
                    continue;
                }
                if best.map_or(true, |k| found[i].best > found[k].best) {
                    best = Some(i);
                }
            }
            let Some(i) = best else { break };
            gezeigt[i] = true;
            let b = &found[i];
            host::print("    ");
            print_dbm(b.best);
            host::print("  K");
            host::print_dec(b.channel as u32);
            if b.channel < 100 {
                host::print(" ");
            }
            host::print("  ");
            let (_, bw, _) = chan_params(b.channel, b.width(), 2);
            host::print(match (bw, b.channel > 14) {
                (2, _) => "VHT80",
                (1, true) => "VHT40",
                (1, false) => " HT40",
                (_, true) => "VHT20",
                _ => " HT20",
            });
            print_last(b);
            host::print("  \"");
            print_ssid(&b.ssid[..b.ssid_len as usize]);
            host::print("\"");
            if target.map_or(false, |t| t.bssid == b.bssid) {
                host::print("   <- gewaehlt");
            }
            host::print("\n");
        }
    }
    // Remember the channels of our SSID as the search space for roaming.
    // Channels only, not levels: those are stale as soon as someone moves.
    if let Some(t) = target {
        let mut n_ch = 0usize;
        for b in found[..n_found].iter() {
            if b.ssid_len != t.ssid_len
                || b.ssid[..b.ssid_len as usize] != t.ssid[..t.ssid_len as usize]
            {
                continue;
            }
            // SAFETY: single-threaded, one writer, and the scan does not run
            // concurrently with the pump.
            unsafe {
                let chs = &mut *core::ptr::addr_of_mut!(ROAM_CHANNELS);
                if !chs[..n_ch].contains(&b.channel) && n_ch < chs.len() {
                    chs[n_ch] = b.channel;
                    n_ch += 1;
                }
            }
        }
        unsafe { N_ROAM_CHANNELS = n_ch };
        host::print("  Roaming-Kanaele:");
        for i in 0..n_ch {
            host::print(" K");
            host::print_dec(unsafe { ROAM_CHANNELS[i] } as u32);
        }
        host::print("\n");
    }
    if let Some(b) = target {
        host::print("  Ziel fuer Stufe 5e: \"");
        print_ssid(&b.ssid[..b.ssid_len as usize]);
        host::print("\" auf K");
        host::print_dec(b.channel as u32);
        host::print(" · ");
        print_dbm(b.best);
        host::print(" · Faehigkeiten 0x");
        host::print_hex16(b.capability);
        host::print(" · RSN ");
        if b.rsn_len > 0 {
            host::print_dec(b.rsn_len as u32);
            host::print(" Bytes");
        } else {
            host::print("keins (offen oder WEP)");
        }
        host::print("\n");
    }

    let mut ok = true;
    // The gate measures us, not the neighborhood. Whether anyone transmits
    // on a channel is not up to the driver; whether the chip accepted the
    // channel is. The number of networks found is a finding, printed above.
    ok &= gate("jeder angefahrene Kanal steht danach im RF", rf_ok == n_ch);
    // This gate belongs here and not in 5b. Only a response proves a frame
    // left the antenna, and whether anyone answers on one channel is a coin
    // toss; over thirteen channels it is not.
    ok &= gate("ein fremder AP antwortet auf unseren Probe Request\n         \x20         (ueber alle aktiven Kanaele)", n_resp > 0);
    ok &= gate("der Suchlauf findet Netze", n_found > 0);
    let mehr_als_einer = {
        let first = found[..n_found].iter().map(|b| b.channel).next()
            .unwrap_or(0);
        found[..n_found].iter().any(|b| b.channel != first)
    };
    ok &= gate("und zwar auf MEHR als dem einen Kanal von vorher",
               mehr_als_einer);
    ok
}

/// Adds a beacon or probe response to the list. Returns `false` when the
/// list is full; a full list is a finding and must not look like an empty
/// channel.
fn record_bss(found: &mut [Bss], n: &mut usize, f: &[u8], ch: u8,
              signal: i8, is_beacon: bool) -> bool {
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&f[16..22]); // addr3

    // Elements are parsed again for every frame, as Linux updates the BSS
    // entry with every beacon. Keeping those of the first frame would let
    // chance (beacon or probe response, early or late) decide the channel
    // width attributed to a cell forever.
    let idx = match found[..*n].iter().position(|b| b.bssid == bssid) {
        Some(i) => {
            if signal > found[i].best {
                found[i].best = signal;
            }
            if is_beacon { found[i].beacons += 1 } else { found[i].resps += 1 }
            i
        }
        None => {
            if *n >= found.len() {
                return false;
            }
            let i = *n;
            *n += 1;
            found[i].bssid = bssid;
            found[i].channel = ch;
            found[i].best = signal;
            found[i].beacons = is_beacon as u16;
            found[i].resps = !is_beacon as u16;
            i
        }
    };
    let e = &mut found[idx];
    // 24 header + 8 timestamp + 2 beacon interval, then the capability
    // field, then the elements.
    if f.len() >= 36 {
        e.capability = u16::from_le_bytes([f[34], f[35]]);
    }
    let mut i = 36;
    while i + 2 <= f.len() {
        let id = f[i];
        let len = f[i + 1] as usize;
        if i + 2 + len > f.len() {
            break;
        }
        match id {
            0 => {
                let take = len.min(32);
                e.ssid[..take].copy_from_slice(&f[i + 2..i + 2 + take]);
                e.ssid_len = take as u8;
            }
            // 48 = RSN (802.11 §9.4.2.24). Kept raw, including the header.
            48 if len + 2 <= 64 => {
                e.rsn[..len + 2].copy_from_slice(&f[i..i + 2 + len]);
                e.rsn_len = (len + 2) as u8;
            }
            // 61 = HT Operation (802.11 §9.4.2.56). Byte 0 is the primary
            // channel, byte 1 the secondary channel offset; only byte 1 is
            // kept, the channel is the one we were on.
            // 45 = HT Capabilities (802.11 §9.4.2.55). Bytes 0:1 are the
            // capability field; bit 1 says whether the AP can do 40 MHz.
            // 11 = BSS Load (802.11 §9.4.2.26). Bytes 0:1 the station count,
            // byte 2 the channel utilization as 0..255.
            11 if len >= 3 => {
                e.bss_load = f[i + 4];
                e.bss_load_seen = true;
            }
            45 if len >= 2 => {
                e.ht_cap = u16::from_le_bytes([f[i + 2], f[i + 3]]);
            }
            61 if len >= 2 => {
                e.ht_param = f[i + 3];
                e.ht_op_seen = true;
            }
            // 191 = VHT Capabilities (802.11 §9.4.2.157). Only the first four
            // bytes, "VHT Capabilities Info": `build_vht_cap_ie` trims our
            // offer to it, as mac80211 does (`ieee80211_add_vht_ie`,
            // mlme.c:1481-1526), because "Some APs apparently get confused
            // if our capabilities are better than theirs."
            191 if len >= 4 => {
                e.ap_vht_cap = u32::from_le_bytes(
                    [f[i + 2], f[i + 3], f[i + 4], f[i + 5]]);
                e.ap_vht_cap_seen = true;
            }
            // 192 = VHT Operation (802.11 §9.4.2.158). Byte 0 is the width,
            // bytes 1 and 2 the two center channel segments. Without it
            // there is no 80 MHz: the center of an 80 is stated nowhere
            // else, and guessing from the primary would be a claim about
            // three neighbor channels.
            // 221 = vendor specific. Microsoft OUI 00:50:f2, type 2 is WMM,
            // subtype 0 the info and 1 the parameter element, the check
            // from mac80211 `parse.c:407-421`.
            221 if len >= 5
                && f[i + 2..i + 5] == [0x00, 0x50, 0xf2]
                && f[i + 5] == 2
                && (f[i + 6] == 0 || f[i + 6] == 1) => {
                e.wmm = true;
            }
            192 if len >= 3 => {
                e.vht_chanwidth = f[i + 2];
                e.vht_cch0 = f[i + 3];
                e.vht_cch1 = f[i + 4];
                e.vht_op_seen = true;
            }
            _ => {}
        }
        i += 2 + len;
    }
    true
}

fn print_ssid(s: &[u8]) {
    if s.is_empty() {
        host::print("<versteckt>");
        return;
    }
    for &c in s {
        let b = [if (0x20..0x7f).contains(&c) { c } else { b'.' }];
        host::print(unsafe { core::str::from_utf8_unchecked(&b) });
    }
}

/// rtw8822c.c:4179-4186 `rtw8822c_phy_calibration`, stage 5d.
///
/// It runs here because Linux runs it here: `rtw_set_channel` only sets
/// `need_rfk = true`; it executes in `rtw_chip_prepare_tx`, which mac80211
/// calls from `mgd_prepare_tx`, i.e. after the scan and before association.
/// Calibrating on every channel while scanning takes too long, as the
/// comment in `main.c` says.
#[allow(clippy::too_many_arguments)]
fn stage5d_calibration(h: i32, hal: &Hal, trx: &mut pci::Trx, h2c_buf: i32,
                       h2c: &mut fw::H2cState,
                       e: &efuse::Efuse, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5d: die RF-Kalibrierung\n");
    if h2c_buf < 0 {
        host::print("  kein DMA-Puffer fuer H2C\n");
        return false;
    }

    // `dm_flags` is written only from debugfs in Linux; at start it is zero,
    // so no calibration is disabled.
    const DM_FLAGS: u32 = 0;

    host::print("  power_track_type ");
    host::print_dec(e.power_track_type as u32);
    host::print(" · thermal_meter ");
    host::print_dec(e.thermal_meter_k as u32);
    host::print(" · HMETFR 0x");
    host::print_hex8(fw::hmetfr(h));
    host::print(" · naechstes Fach ");
    host::print_dec(h2c.last_box_num as u32);
    host::print("\n");

    // Firmware C2H responses arrive on PCIe through the same RX ring as
    // radio frames, separated by `pkt_stat.is_c2h`. Linux reads them
    // continuously; here `rx_poll` runs only in the measurement windows of
    // 5a to 5c, so whatever accumulated since is drained first. Firmware
    // whose output nobody drains may appear not to respond.
    {
        let mut dmx = dm::DmInfo::new();
        let mut pdx = dm::PathDiv::default();
        static mut RXBUF4: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
            [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
        // SAFETY: one thread, one caller, the buffer does not leave the block.
        let b = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF4) };
        let mut c2h = 0u32;
        let mut frames = 0u32;
        let t0 = host::now_us();
        while host::now_us() - t0 < 50_000 {
            let n = pci::rx_poll(h, trx, 64, b, &mut dmx, &mut pdx,
                                 hal.rf_path_num, 0, 1, |st, pkt| {
                if st.is_c2h {
                    c2h += 1;
                    let off = RX_PKT_DESC_SZ as usize
                        + st.drv_info_sz as usize + st.shift as usize;
                    if c2h <= 4 && off + 2 <= pkt.len() {
                        host::print("    c2h id 0x");
                        host::print_hex8(pkt[off]);
                        host::print(" len ");
                        host::print_dec(st.pkt_len as u32);
                        host::print("\n");
                    }
                } else {
                    frames += 1;
                }
            });
            if n == 0 {
                host::sleep_ms(1);
            }
        }
        host::print("  Ring geleert: ");
        host::print_dec(c2h);
        host::print(" C2H, ");
        host::print_dec(frames);
        host::print(" Funkrahmen · HMETFR danach 0x");
        host::print_hex8(fw::hmetfr(h));
        host::print("\n");
    }

    let mut gapk = txgapk::GapkInfo::new();
    let dpkinfo = &mut d.dpk;
    // `rtw_load_rfk_table` wrote the RFK table in stage 3c and sets this
    // flag (phy.c:1847). Without a loaded table there would be no DPK.
    dpkinfo.is_dpk_pwr_on = true;
    let mut bt_iqk_timeout = false;

    let t_all = host::now_us();

    // `rtw8822c_rfk_power_save(rtwdev, false)`
    rfkcal::power_save(h, hal.rf_path_num, false);

    // ── do_gapk ──────────────────────────────────────────────────
    let t0 = host::now_us();
    let (gapk_rpt, hs1, hs2) =
        rfkcal::do_gapk(h, &mut gapk, hal.rf_path_num, DM_FLAGS,
                        e.power_track_type, &mut bt_iqk_timeout, h2c);
    let dt_gapk = host::now_us() - t0;

    host::print("  Handschlag: BT-IQK ");
    if hs1.bt_iqk_timeout {
        host::print("ZEITUEBERSCHREITUNG nach ");
    } else {
        host::print("frei nach ");
    }
    host::print_dec(hs1.bt_iqk_waited_us as u32);
    host::print(" us · Start-Quittung ");
    host::print(if hs1.start_ack { "ja" } else { "NEIN" });
    host::print(" (");
    host::print_dec(hs1.start_ack_us as u32);
    host::print(" us) · Ende-Quittung ");
    host::print(if hs2.finish_ack { "ja" } else { "NEIN" });
    host::print(" (");
    host::print_dec(hs2.finish_ack_us as u32);
    host::print(" us)\n  TXGAPK: ");
    match gapk_rpt {
        txgapk::TxgapkRpt::Ran => {
            host::print("gelaufen, Kanal ");
            host::print_dec(gapk.channel as u32);
            host::print(" · Versatz Pfad A");
            for i in 0..txgapk::RF_HW_OFFSET_NUM_U {
                host::print(if i == 0 { " " } else { "," });
                print_signed(gapk.offset[i][0] as i32);
            }
        }
        txgapk::TxgapkRpt::NoTxGain =>
            host::print("uebersprungen, keine Verstaerkungstabelle gelesen"),
        txgapk::TxgapkRpt::TssiMode(t) => {
            host::print("uebersprungen — TSSI-Modus (power_track_type ");
            host::print_dec(t as u32);
            host::print("), der Chip regelt selbst");
        }
        txgapk::TxgapkRpt::Disabled =>
            host::print("abgeschaltet ueber dm_flags"),
    }
    host::print(" · ");
    host::print_dec((dt_gapk / 1000) as u32);
    host::print(" ms\n");

    // ── do_iqk ───────────────────────────────────────────────────
    let t0 = host::now_us();
    let (iqk_ok, iqk_us, iqk_chk) = rfkcal::do_iqk(h, trx, h2c_buf, h2c);
    let _ = host::now_us() - t0;
    host::print("  IQK (Firmware): ");
    host::print(if iqk_ok { "fertig" } else { "NICHT fertig" });
    host::print(", RPT_CIP 0x");
    host::print_hex8(iqk_chk);
    host::print(" nach ");
    host::print_dec((iqk_us / 1000) as u32);
    host::print(" ms\n");

    // ── do_dpk ───────────────────────────────────────────────────
    let t0 = host::now_us();
    let dpk_rpt = dpk::do_dpk(h, dpkinfo, hal.rf_path_num);
    let dt_dpk = host::now_us() - t0;
    host::print("  DPK: ");
    let mut dpk_paths = 0u8;
    match dpk_rpt {
        dpk::DpkRpt::Ran { path_ok, gs, txagc, coef1_ready } => {
            dpk_paths = path_ok;
            host::print("Pfade ok 0b");
            host::print_dec((path_ok & 1) as u32);
            host::print_dec(((path_ok >> 1) & 1) as u32);
            host::print(" · gs ");
            host::print_dec(gs[0] as u32);
            host::print("/");
            host::print_dec(gs[1] as u32);
            host::print(" · txagc ");
            host::print_dec(txagc[0] as u32);
            host::print("/");
            host::print_dec(txagc[1] as u32);
            host::print(" · coef1 ");
            host::print(if coef1_ready { "fertig" } else { "HAENGT" });
        }
        dpk::DpkRpt::PwrOff => host::print("uebersprungen, DPD-Strom aus"),
        dpk::DpkRpt::Reloaded => host::print("aus dem Zwischenspeicher"),
    }
    host::print(" · ");
    host::print_dec((dt_dpk / 1000) as u32);
    host::print(" ms · Waerme ");
    host::print_dec(dpkinfo.thermal_dpk[0] as u32);
    host::print("/");
    host::print_dec(dpkinfo.thermal_dpk[1] as u32);
    host::print("\n");

    // `rtw8822c_rfk_power_save(rtwdev, true)`
    rfkcal::power_save(h, hal.rf_path_num, true);
    host::print("  gesamt ");
    host::print_dec(((host::now_us() - t_all) / 1000) as u32);
    host::print(" ms\n");

    // ── Does the receiver still hear after calibration? ──────────
    // A calibration that breaks reception is worse than none. Same
    // measurement as stage 4c, so the numbers compare.
    let dm = &mut d.dm;
    chip::false_alarm_statistics(h, dm);
    host::sleep_ms(200);
    chip::false_alarm_statistics(h, dm);
    host::print("  danach: CCA ");
    host::print_dec(dm.total_cca_cnt);
    host::print(" · CRC ok/err cck ");
    host::print_dec(dm.cck_ok_cnt);
    host::print("/");
    host::print_dec(dm.cck_err_cnt);
    host::print(" · ofdm ");
    host::print_dec(dm.ofdm_ok_cnt);
    host::print("/");
    host::print_dec(dm.ofdm_err_cnt);
    host::print("\n");

    let mut ok = true;
    ok &= gate("die Firmware quittiert den RFK-Handschlag",
               hs1.start_ack && hs2.finish_ack);
    ok &= gate("die Firmware meldet die IQK als fertig", iqk_ok);
    ok &= gate("die DPK richtet mindestens einen Pfad ein", dpk_paths != 0);
    ok &= gate("und der Empfaenger hoert danach unveraendert",
               dm.total_cca_cnt != 0);
    ok
}

fn print_signed(v: i32) {
    if v < 0 {
        host::print("-");
        host::print_dec((-v) as u32);
    } else {
        host::print_dec(v as u32);
    }
}

/// Stage 5e: authentication and association.
///
/// Linux order: set the target's channel → `rtw_chip_prepare_tx` (the 5d
/// calibration, since `rtw_set_channel` set `need_rfk`) → `PORT_SET_BSSID`
/// → auth → assoc → on success `net_type = RTW_NET_MGD_LINKED` with the AID
/// in the port, plus `rtw_fw_media_status_report`.
///
/// Not part of this stage:
/// * `rtw_update_sta_info` + `rtw_fw_send_ra_info`: rate adaptation needs
///   the peer's HT/VHT capabilities from the association response (stage
///   5f).
/// * `rtw_fw_download_rsvd_page` + `rtw_send_rsvd_page_h2c`: PS-Poll, null
///   and QoS null frames in the reserved page area. The path exists since
///   stage 2 (`download_firmware` writes there); the content is upper half.
/// * `rtw_fw_default_port`, `rtw_coex_media_status_notify`,
///   `rtw_bf_assoc`, `rtw_set_ampdu_factor`, `rtw_fw_beacon_filter_config`.
/// * The four-way handshake and key store: that is `wifid`, and without it
///   an association ends in a deauth within seconds. This stage's gate is
///   before that.
#[allow(clippy::too_many_arguments)]
fn stage5e_connect(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
                   h2c: &mut fw::H2cState, e: &efuse::Efuse,
                   t: &txpower::TxPower, mac: [u8; 6], bss: &Bss,
                   out_vif: &mut Option<vif::Vif>, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5e: Auth und Assoc mit \"");
    print_ssid(&bss.ssid[..bss.ssid_len as usize]);
    host::print("\" auf K");
    host::print_dec(bss.channel as u32);
    host::print("\n");

    // `rtw_set_channel` to the target's channel, with the width the cell
    // announces, so association request, TX descriptors and PHY agree.
    let max_bw = max_bw_for(e);
    let (cch, bw, _) = chan_params(bss.channel, bss.width(), max_bw);
    if !switch_channel(h, hal, e, t, bss.channel, bss.width(), max_bw) {
        host::print("  RF 0x18 traegt den Zielkanal NICHT\n");
        return false;
    }
    // The width is decided here and only here; stage 5f reads it instead of
    // computing it again.
    d.cur_bw = bw;
    host::print("  Kanal ");
    host::print_dec(bss.channel as u32);
    host::print(" · Breite ");
    host::print(match bw {
        2 => "80 MHz (Mitte K",
        1 => "40 MHz (Mitte K",
        _ => "20 MHz (K",
    });
    host::print_dec(cch as u32);
    host::print(")");
    // The raw byte as well: without it "the AP allows no HT40" looks the
    // same as "we parse the element wrong". Secondary channel in bits 1:0,
    // permission for more than 20 MHz in bit 2.
    host::print(" · HT-Operation 0x");
    host::print_hex8(bss.ht_param);
    host::print(" (");
    host::print(match bss.ht_param & 0x03 {
        1 => "Zweitkanal oben",
        3 => "Zweitkanal unten",
        _ => "kein Zweitkanal",
    });
    host::print(if bss.ht_param & 0x04 != 0 {
        ", Breite erlaubt)"
    } else {
        ", nur 20 MHz)"
    });
    // Can it do 40, or does it only run 20? HT Operation says what the AP
    // currently runs, bit 1 of its HT capabilities what it can. Both
    // together separate its decision from our gap, and a missing element is
    // a third case.
    host::print(" · AP kann 40: ");
    host::print(if bss.ht_cap & 0x0002 != 0 { "JA" } else { "nein" });
    if !bss.ht_op_seen {
        host::print(" · ACHTUNG: HT-Operation-Element war in KEINEM Rahmen dieser Zelle");
    }
    host::print("\n");

    // The same for 80 MHz: `40 MHz` on a 5 GHz AP has three possible causes
    // (the cell really runs 40, it sends no VHT Operation element, or its
    // center failed our check), indistinguishable without the raw bytes.
    if bss.channel > 14 {
        host::print("  VHT-Operation ");
        if bss.vht_op_seen {
            host::print("Breite ");
            host::print_dec(bss.vht_chanwidth as u32);
            host::print(match bss.vht_chanwidth {
                0 => " (wie HT)",
                1 => " (80/160)",
                2 => " (160, abgeschaffte Kodierung)",
                _ => " (80+80, abgeschaffte Kodierung)",
            });
            host::print(" · Mitte K");
            host::print_dec(bss.vht_cch0 as u32);
            if bss.vht_cch1 != 0 {
                // A second segment means the AP runs wider than 80. We still
                // take segment 0, which is what 802.11 puts there for us.
                host::print(" · Segment 1 K");
                host::print_dec(bss.vht_cch1 as u32);
                host::print(" (er faehrt breiter, wir nehmen die primaeren 80)");
            }
            if bss.vht_chanwidth == VHT_CHANWIDTH_80 && bw != 2 {
                host::print(if max_bw < 2 {
                    " · 80 MHz hier nicht erlaubt (bw: oder Karte)"
                } else {
                    " · ABGELEHNT: Mitte oder Viertel passen nicht"
                });
            }
        } else {
            host::print("fehlt — kein 80 MHz aus dieser Zelle");
        }
        host::print("\n");
    }

    // `rtw_chip_prepare_tx`: `need_rfk` is set, so calibrate, on this
    // channel rather than the scan's.
    let mut gapk = txgapk::GapkInfo::new();
    let dpkinfo = &mut d.dpk;
    dpkinfo.is_dpk_pwr_on = true;
    let mut bt_iqk_timeout = false;
    let t0 = host::now_us();
    rfkcal::power_save(h, hal.rf_path_num, false);
    rfkcal::do_gapk(h, &mut gapk, hal.rf_path_num, 0, e.power_track_type,
                    &mut bt_iqk_timeout, h2c);
    rfkcal::do_iqk(h, trx, mgmt_buf, h2c);
    let dpk_rpt = dpk::do_dpk(h, dpkinfo, hal.rf_path_num);
    rfkcal::power_save(h, hal.rf_path_num, true);
    host::print("  auf K");
    host::print_dec(bss.channel as u32);
    host::print(" kalibriert (");
    host::print_dec(((host::now_us() - t0) / 1000) as u32);
    host::print(" ms, DPK ");
    host::print(match dpk_rpt {
        dpk::DpkRpt::Ran { .. } => "gelaufen",
        dpk::DpkRpt::Reloaded => "aus dem Zwischenspeicher",
        dpk::DpkRpt::PwrOff => "aus",
    });
    host::print(")\n");

    // `rtw_ops_bss_info_changed`, branch `BSS_CHANGED_BSSID`. From now on
    // the hardware accepts frames of this cell.
    let mut vifc = vif::add_interface_station(h, mac);
    vifc.bssid = bss.bssid;
    vif::port_config(h, &vifc, PORT_SET_BSSID);

    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF5: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: one thread, one caller, the buffer does not leave the function.
    let rxbuf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF5) };

    let mut frame = [0u8; 256];
    let mut ok = true;

    // ── Authentication (Open System) ─────────────────────────────
    // WPA2 authenticates open too; keys come after association, in the
    // four-way handshake.
    let n = build_auth_req(&mut frame, &mac, &bss.bssid);
    let (auth_ok, auth_status, auth_tries) =
        exchange(h, hal, trx, mgmt_buf, rxbuf, dm, path_div,
                 &frame[..n], &mac, bss.channel, d.cur_bw as u8, 0xb0, |f| {
            // Auth response: algorithm, sequence 2, status.
            if f.len() < 30 {
                return None;
            }
            let seq = u16::from_le_bytes([f[26], f[27]]);
            let status = u16::from_le_bytes([f[28], f[29]]);
            if seq == 2 { Some(status) } else { None }
        });
    host::print("  Auth: ");
    report_exchange(auth_ok, auth_status, auth_tries);
    ok &= gate("der AP authentifiziert uns", auth_ok && auth_status == 0);
    if !(auth_ok && auth_status == 0) {
        return false;
    }

    // ── Association ──────────────────────────────────────────────
    let n = build_assoc_req(&mut frame, &mac, bss, e, hal.rf_path_num);
    host::print("  Anmeldeantrag ");
    host::print_dec(n as u32);
    host::print(" Bytes · HT ja (nss ");
    host::print_dec(e.hw_cap_nss as u32);
    host::print(", bw 0x");
    host::print_hex8(e.hw_cap_bw);
    host::print(")");
    if bss.channel > 14 {
        host::print(" · VHT ja");
    }
    if bss.rsn_len > 0 {
        host::print(" · RSN CCMP/PSK");
    }
    host::print("\n");
    // The raw elements as they go out, to compare against the evaluation.
    // From offset 28: 24 bytes header, then capability info and listen
    // interval, then the elements.
    if n > 28 {
        host::print("  Antrag-Elemente:");
        for (k, byte) in frame[28..n].iter().enumerate() {
            host::print(if k % 16 == 0 { "\n   " } else { " " });
            host::print_hex8(*byte);
        }
        host::print("\n");
    }
    if bss.ap_vht_cap_seen {
        host::print("  AP VHT cap 0x");
        host::print_hex8((bss.ap_vht_cap >> 24) as u8);
        host::print_hex8((bss.ap_vht_cap >> 16) as u8);
        host::print_hex8((bss.ap_vht_cap >> 8) as u8);
        host::print_hex8(bss.ap_vht_cap as u8);
        host::print(if bss.ap_vht_cap & IEEE80211_VHT_CAP_SU_BEAMFORMER_CAPABLE != 0 {
            " · SU-Beamformer ja"
        } else {
            " · SU-Beamformer NEIN -> wir nehmen Beamformee zurueck"
        });
        host::print("\n");
    } else if bss.channel > 14 {
        host::print("  der AP hat in seiner Bake KEIN VHT-Element\n");
    }
    let mut aid = 0u16;
    let (assoc_ok, assoc_status, assoc_tries) =
        exchange(h, hal, trx, mgmt_buf, rxbuf, dm, path_div,
                 &frame[..n], &mac, bss.channel, d.cur_bw as u8, 0x10, |f| {
            // Association response: capabilities, status, AID.
            if f.len() < 30 {
                return None;
            }
            Some(u16::from_le_bytes([f[26], f[27]]))
        });
    if assoc_ok && assoc_status == 0 {
        // The AID is in the same frame as the status, two bytes after it.
        // `exchange` returns only one number, so the reader stores it
        // alongside.
        // SAFETY: single-threaded, one writer, one reader.
        aid = unsafe { LAST_ASSOC_AID };
    }
    host::print("  Assoc: ");
    report_exchange(assoc_ok, assoc_status, assoc_tries);
    if assoc_ok && assoc_status == 0 {
        host::print("  AID ");
        host::print_dec((aid & 0x3fff) as u32);
        host::print("\n");
    }
    ok &= gate("der AP nimmt uns an (Status 0 und eine AID)",
               assoc_ok && assoc_status == 0 && (aid & 0x3fff) != 0);

    if assoc_ok && assoc_status == 0 {
        // `rtw_vif_assoc_changed` + `PORT_SET_NET_TYPE | PORT_SET_AID`
        vifc.aid = (aid & 0x3fff) as u32;
        vifc.net_type = RTW_NET_MGD_LINKED;
        vif::port_config(h, &vifc, PORT_SET_NET_TYPE | PORT_SET_AID);
        // `rtw_fw_media_status_report`
        let msr = fw::media_status_report(h, h2c, vifc.mac_id, true);
        host::print("  Port: net_type MGD_LINKED, AID gesetzt · ");
        host::print(if msr {
            "media_status_report raus\n"
        } else {
            "media_status_report FEHLGESCHLAGEN\n"
        });
        ok &= gate("die Firmware nimmt die Verbindungsmeldung an", msr);
        *out_vif = Some(vifc);
    }

    ok
}

/// The AID of the last association response. It is in the same frame as the
/// status, and `exchange` returns only one number.
static mut LAST_ASSOC_AID: u16 = 0;

/// The whole frame as well: stage 5f reads the AP's capabilities (HT, VHT,
/// rates) from it. In Linux mac80211 builds `ieee80211_sta` from it.
static mut LAST_ASSOC_RESP: [u8; 256] = [0; 256];
static mut LAST_ASSOC_RESP_LEN: usize = 0;

/// Sends a management frame and waits for the response.
///
/// Three times, spaced out: a single frame can collide, and an AP may drop
/// it. Returns (response received, status, attempts).
#[allow(clippy::too_many_arguments)]
fn exchange(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
            rxbuf: &mut [u8], dm: &mut dm::DmInfo,
            path_div: &mut dm::PathDiv, frame: &[u8], mac: &[u8; 6],
            channel: u8, bw: u8, want_fc: u8,
            parse: impl Fn(&[u8]) -> Option<u16>) -> (bool, u16, u32)
{
    let queue = tx::RTW_TX_QUEUE_MGMT;
    for tries in 1..=3u32 {
        let mut info = tx::pkt_info_update(frame, 0, tx::band_of(channel));
        if !pci::tx_write(h, trx, mgmt_buf, queue, &mut info, frame) {
            return (false, 0, tries);
        }
        pci::tx_kick_off_queue(h, trx, queue);
        let (_done, _us, _hw) = pci::tx_wait_consumed(h, trx, queue, 50_000);
        // `rtw_pci_tx_isr`: advance the read pointer, otherwise the ring
        // fills up over several frames.
        pci::tx_isr(h, trx, queue);

        let mut status: Option<u16> = None;
        let t0 = host::now_us();
        while host::now_us() - t0 < 300_000 && status.is_none() {
            let n = pci::rx_poll(h, trx, 64, rxbuf, dm, path_div,
                                 hal.rf_path_num, bw, channel, |st, pkt| {
                if st.crc_err || st.is_c2h || status.is_some() {
                    return;
                }
                let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                    + st.shift as usize;
                if off + 30 > pkt.len() {
                    return;
                }
                let f = &pkt[off..];
                if f[0] != want_fc {
                    return;
                }
                if f[4..10] != mac[..] {
                    return;
                }
                if let Some(v) = parse(f) {
                    // For the association response the same frame carries
                    // the AID.
                    if want_fc == 0x10 && f.len() >= 32 {
                        // SAFETY: single-threaded, one writer.
                        unsafe {
                            LAST_ASSOC_AID =
                                u16::from_le_bytes([f[28], f[29]]);
                            let n = f.len().min(256);
                            LAST_ASSOC_RESP[..n].copy_from_slice(&f[..n]);
                            LAST_ASSOC_RESP_LEN = n;
                        }
                    }
                    status = Some(v);
                }
            });
            if n == 0 {
                host::sleep_ms(1);
            }
        }
        if let Some(v) = status {
            return (true, v, tries);
        }
        host::sleep_ms(50);
    }
    (false, 0, 3)
}

fn report_exchange(got: bool, status: u16, tries: u32) {
    if !got {
        host::print("keine Antwort nach 3 Versuchen\n");
        return;
    }
    host::print("Antwort nach Versuch ");
    host::print_dec(tries);
    host::print(", Status ");
    host::print_dec(status as u32);
    host::print(match status {
        0 => " (angenommen)",
        1 => " (unspezifisch abgelehnt)",
        12 => " (abgelehnt: Vorbedingungen)",
        17 => " (abgelehnt: AP voll)",
        43 => " (abgelehnt: falsches Paarschluessel-Verfahren)",
        _ => "",
    });
    host::print("\n");
}

/// 802.11 §9.3.3.12: authentication frame, Open System, sequence 1.
fn build_auth_req(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6])
    -> usize
{
    mgmt_header(out, 0xb0, mac, bssid);
    out[24..26].copy_from_slice(&0u16.to_le_bytes()); // algorithm 0 = open
    out[26..28].copy_from_slice(&1u16.to_le_bytes()); // sequence 1
    out[28..30].copy_from_slice(&0u16.to_le_bytes()); // Status 0
    30
}

/// 802.11 §9.3.3.6: association request.
///
/// With HT and VHT elements. Without them the AP treats us as a legacy
/// station and omits HT in its response too; a request that offers less
/// gets less.
fn build_assoc_req(out: &mut [u8; 256], mac: &[u8; 6], bss: &Bss,
                   e: &efuse::Efuse, rf_path_num: u8) -> usize {
    mgmt_header(out, 0x00, mac, &bss.bssid);
    // Capabilities: ESS, plus privacy and short preamble as the AP announces
    // them. Claiming more than the AP supports gets rejected.
    //
    // Short preamble only on 2.4 GHz: mac80211 sets SHORT_SLOT and
    // SHORT_PREAMBLE only in the 2.4 GHz band (mlme.c:1771-1774); on 5 GHz
    // the bit is meaningless and Linux omits it.
    let preamble = if bss.channel > 14 { 0 } else { bss.capability & 0x0020 };
    let cap = 0x0001u16 | (bss.capability & 0x0010) | preamble;
    out[24..26].copy_from_slice(&cap.to_le_bytes());
    out[26..28].copy_from_slice(&10u16.to_le_bytes()); // Listen Interval
    let mut n = 28;

    // SSID
    let sl = bss.ssid_len as usize;
    out[n] = 0;
    out[n + 1] = sl as u8;
    out[n + 2..n + 2 + sl].copy_from_slice(&bss.ssid[..sl]);
    n += 2 + sl;

    // ── Rates, which depend on the band ──────────────────────────
    //
    // 1, 2, 5.5 and 11 Mbit are CCK/DSSS rates that do not exist in the
    // 5 GHz band. Linux builds these elements per band from
    // `sband->bitrates` (`ieee80211_assoc_add_rates` ->
    // `ieee80211_put_srates_elem`, mlme.c), intersects them with the AP's
    // rates and sets no basic bits in an association request (`basic_rates`
    // is 0 there), because "some APs don't like getting a superset of their
    // rates in the association request".
    //
    // The 2.4 GHz branch keeps its basic bits, a known deviation from Linux
    // left in place because the path works.
    if bss.channel > 14 {
        // 6, 9, 12, 18, 24, 36, 48, 54: all eight fit into one element, so
        // the extended one is omitted.
        out[n] = 1;
        out[n + 1] = 8;
        out[n + 2..n + 10]
            .copy_from_slice(&[0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c]);
        n += 10;
    } else {
        // Supported Rates: 1, 2, 5.5, 11, 6, 9, 12, 18 Mbit
        out[n] = 1;
        out[n + 1] = 8;
        out[n + 2..n + 10]
            .copy_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
        n += 10;

        // Extended Supported Rates: 24, 36, 48, 54 Mbit
        out[n] = 50;
        out[n + 1] = 4;
        out[n + 2..n + 6].copy_from_slice(&[0x30, 0x48, 0x60, 0x6c]);
        n += 6;
    }

    // Element order follows the specification, not the element IDs: 802.11
    // table 9-34 orders an association request body by table position, so
    // RSN (order 8) precedes HT Capabilities (order 13), and the extended
    // rates (order 5) precede HT although ID 50 is larger than 45. mac80211
    // emits RSN from `ieee80211_add_before_ht_elems` (mlme.c), before HT.
    // The four-way handshake MIC covers the RSN element's content, not its
    // position.

    // RSN: one choice built from what the AP announces.
    if bss.rsn_len > 0 {
        n += build_rsn_ie(&mut out[n..], &bss.rsn[..bss.rsn_len as usize]);
    }

    // HT always: it decides whether we are accepted as an 11n station.
    n += sta::build_ht_cap_ie(&mut out[n..], e.hw_cap_bw, e.hw_cap_nss);

    // VHT only on 5 GHz: it is not allowed on 2.4 GHz, and an AP may reject
    // a request with VHT in the wrong band.
    if bss.channel > 14 {
        let v = sta::build_vht_cap_ie(&mut out[n..], e.hw_cap_ptcl,
                                      e.hw_cap_nss, rf_path_num,
                                      if bss.ap_vht_cap_seen {
                                          Some(bss.ap_vht_cap)
                                      } else {
                                          None
                                      });
        // Record what actually went out, not the condition for it, so the
        // report reflects the wire even if `build_vht_cap_ie` declined.
        // SAFETY: single-threaded, one writer, and the report reads it only
        // after association completed.
        unsafe {
            SENT_VHT = if v >= 14 {
                Some(u32::from_le_bytes([out[n + 2], out[n + 3],
                                         out[n + 4], out[n + 5]]))
            } else {
                None
            };
        }
        n += v;
    } else {
        // SAFETY: as above.
        unsafe { SENT_VHT = None };
    }

    // ── WMM information, as the last element ─────────────────────
    //
    // Without it there is no VHT: hostapd strips VHT from a station whose
    // request carries no valid WMM element (`copy_sta_vht_capab`,
    // ieee802_11_vht.c:200-207: `!(sta->flags & WLAN_STA_WMM)`), and
    // `check_wmm` (ieee802_11.c:5361) sets that flag only from this
    // element.
    //
    // Built like `ieee80211_add_wmm_info_ie` (util.c:4290-4303), placed as in
    // `ieee80211_send_assoc` (mlme.c:2299-2309): after all non-vendor
    // elements, only if the AP announces WMM itself (`assoc_data->wmm` =
    // `bss->wmm_used`), QoS info 0 (no U-APSD).
    if bss.wmm && n + 9 <= out.len() {
        out[n..n + 9].copy_from_slice(&[
            221, 7,             // vendor specific, length
            0x00, 0x50, 0xf2,   // Microsoft OUI
            2,                  // WME
            0,                  // WME info
            1,                  // Version
            0,                  // QoS info: U-APSD not in use
        ]);
        n += 9;
    }
    // SAFETY: as above; single-threaded, read only after association.
    unsafe { SENT_WMM = bss.wmm };
    n
}

/// Whether the last association request carried the WMM element.
static mut SENT_WMM: bool = false;

/// The "VHT Capabilities Info" field the last association request actually
/// carried. `None` = no VHT element went out.
static mut SENT_VHT: Option<u32> = None;

/// 802.11 §9.4.2.24: our RSN element.
///
/// It must be byte-identical to the one in `wifid` (`wasm/src/lib.rs:124`),
/// a contract the ABI does not express: the four-way handshake computes its
/// MIC over exactly the RSN element the station sent in the association
/// request. If ours differs, the AP drops msg2 without saying why.
///
/// CCMP as group and pairwise cipher, PSK as authentication.
const RSN_IE_WPA2_CCMP_PSK: [u8; 22] = [
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f,
    0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
];

/// Writes our RSN element, and reports when the AP announces something
/// other than what we can offer.
///
/// A request chooses, a beacon enumerates: the AP's element lists every
/// suite it supports, ours names exactly one. If the AP cannot do CCMP as
/// group cipher, the handshake fails later, and the reason is printed here.
fn build_rsn_ie(out: &mut [u8], ap: &[u8]) -> usize {
    const CCMP: [u8; 4] = [0x00, 0x0f, 0xac, 0x04];
    if ap.len() >= 8 && ap[4..8] != CCMP {
        host::print("  [Hinweis] der AP nennt eine andere Gruppenchiffre als\n         \x20         CCMP — der Handschlag wird daran scheitern.\n");
    }
    out[..RSN_IE_WPA2_CCMP_PSK.len()].copy_from_slice(&RSN_IE_WPA2_CCMP_PSK);
    RSN_IE_WPA2_CCMP_PSK.len()
}

/// The common 24-byte header of a management frame to an AP. The sequence
/// number stays zero: `en_hwseq` is set in the TX descriptor, so the chip
/// assigns it.
fn mgmt_header(out: &mut [u8; 256], subtype_fc: u8, mac: &[u8; 6],
               bssid: &[u8; 6]) {
    out.fill(0);
    out[0] = subtype_fc;
    out[1] = 0x00;
    out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration
    out[4..10].copy_from_slice(bssid); // addr1 = receiver
    out[10..16].copy_from_slice(mac); // addr2 = us
    out[16..22].copy_from_slice(bssid); // addr3 = BSSID
    out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq
}

/// Stage 5f: rate adaptation.
///
/// The driver sends the firmware a mask, not a rate: which of the 64 rates
/// the peer supports. The firmware picks from it continuously and reports
/// its choice as `C2H_RA_RPT`, which is this stage's gate.
///
/// The mask comes from the association response kept by stage 5e: HT and
/// VHT elements, supported rates. In Linux mac80211 builds
/// `ieee80211_sta` from it; here the parser is in `sta.rs`.
///
/// Not implemented: `rtw_fw_download_rsvd_page` + `rtw_send_rsvd_page_h2c`.
/// The reserved pages hold PS-Poll, null and QoS null frames the firmware
/// sends itself in power save; there is no power save here, so they belong
/// with LPS.
fn stage5f_rates(h: i32, trx: &mut pci::Trx, h2c: &mut fw::H2cState,
                 hal: &Hal, vifc: &vif::Vif, bss: &Bss,
                 out: &mut Option<(sta::PeerCaps, sta::StaInfo)>, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5f: die Ratenanpassung\n");

    // SAFETY: single-threaded, and 5e wrote it before.
    let (resp, len) = unsafe {
        (&*core::ptr::addr_of!(LAST_ASSOC_RESP), LAST_ASSOC_RESP_LEN)
    };
    if len < 30 {
        host::print("  keine Anmeldeantwort aufgehoben\n");
        return false;
    }

    let mut caps = sta::parse_assoc_resp(&resp[..len]);

    // A station's width is the minimum of its capability and the cell
    // (mac80211 `ieee80211_sta_cur_vht_bw`). `parse_assoc_resp` knows only
    // the capability; the cell is known here, so the clamp belongs here. A
    // TX descriptor claiming a different width than the radio runs matters:
    // the firmware picks its rates by it.
    // The cell width comes from 5e and is not recomputed; it decides what
    // goes into every TX descriptor.
    let zellen_bw = d.cur_bw;
    if caps.bandwidth > zellen_bw as u8 {
        caps.bandwidth = zellen_bw as u8;
    }
    host::print("  Gegenueber: HT ");
    host::print(if caps.ht_supported { "ja" } else { "nein" });
    if caps.ht_supported {
        host::print(" (cap 0x");
        host::print_hex16(caps.ht_cap);
        host::print(", MCS ");
        for (i, b) in caps.ht_mcs.iter().enumerate() {
            if i > 0 {
                host::print(":");
            }
            host::print_hex8(*b);
        }
        host::print(")");
    }
    host::print(" · VHT ");
    host::print(if caps.vht_supported { "ja" } else { "nein" });
    if caps.vht_supported {
        host::print(" (cap 0x");
        host::print_hex32(caps.vht_cap);
        host::print(", mcs_map 0x");
        host::print_hex16(caps.vht_mcs_map);
        host::print(")");
    }
    host::print("\n  Raten 0x");
    host::print_hex16(caps.supp_rates);
    host::print(" · Bandbreite ");
    host::print(match caps.bandwidth {
        0 => "20",
        1 => "40",
        _ => "80",
    });
    host::print(" MHz\n");

    let mut si = sta::StaInfo { mac_id: vifc.mac_id, init_ra_lv: 1,
                                ..Default::default() };
    let nss = if hal.rf_2t2r { 2 } else { 1 };
    let wireless_set = sta::update_sta_info(&mut si, &caps, nss,
                                            bss.channel <= 14);
    // main.c:1266/1286: `rtw_update_sta_info` also sets the base set of
    // response rates per band. Our `update_sta_info` holds no `dm`, so this
    // line is here and in the watchdog (`phy::ra_track`). Without it
    // `rrsr_update` would write `0 & mask = 0` to REG_RRSR, the rates the
    // MAC picks its ACKs and block ACKs from.
    d.dm.rrsr_val_init = if bss.channel <= 14 { RRSR_INIT_2G } else { RRSR_INIT_5G };

    host::print("  rate_id ");
    host::print_dec(si.rate_id as u32);
    host::print(" · bw_mode ");
    host::print_dec(si.bw_mode as u32);
    host::print(" · sgi ");
    host::print(if si.sgi_enable { "ja" } else { "nein" });
    host::print(" · vht ");
    host::print(if si.vht_enable { "ja" } else { "nein" });
    host::print(" · wireless_set 0x");
    host::print_hex8(wireless_set as u8);
    host::print("\n  ra_mask 0x");
    host::print_hex32((si.ra_mask >> 32) as u32);
    host::print_hex32(si.ra_mask as u32);
    host::print("\n");

    let mut ok = true;
    ok &= gate("die Anmeldeantwort traegt Raten fuer dieses Gegenueber",
               caps.supp_rates != 0);
    ok &= gate("die Ratenmaske ist nicht leer", si.ra_mask != 0);

    let ra = fw::send_ra_info(h, h2c, &mut si, true);
    let dp = fw::default_port(h, h2c, vifc.port, vifc.mac_id, vifc.net_type);
    host::print("  send_ra_info ");
    host::print(if ra { "raus" } else { "FEHLGESCHLAGEN" });
    host::print(" · default_port ");
    host::print(if dp { "raus" } else { "FEHLGESCHLAGEN" });
    host::print("\n");
    ok &= gate("die Firmware nimmt die Ratenmaske an", ra);

    // ── Now listen to what the firmware makes of it ──────────────
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF6: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: one thread, one caller, the buffer does not leave the function.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF6) };

    let mut ra_rpt = 0u32;
    let mut last_rate = 0u8;
    let mut last_sgi = false;
    let mut last_bw = 0u8;
    let mut c2h_total = 0u32;
    let t0 = host::now_us();
    while host::now_us() - t0 < 2_000_000 {
        let n = pci::rx_poll(h, trx, 64, buf, dm, path_div,
                             hal.rf_path_num, d.cur_bw as u8, bss.channel, |st, pkt| {
            if !st.is_c2h {
                return;
            }
            let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                + st.shift as usize;
            let Some(c) = fw::c2h_parse(&pkt[off..]) else { return };
            c2h_total += 1;
            if c2h_total <= 8 {
                host::print("    c2h 0x");
                host::print_hex8(c.id);
                host::print(" ");
                host::print(fw::c2h_name(c.id));
                host::print("\n");
            }
            // `rtw_fw_ra_report_handle`: rate_sgi, mac_id, …, bw
            if c.id as u32 == C2H_RA_RPT && c.payload.len() >= 7 {
                ra_rpt += 1;
                last_rate = (c.payload[0] as u32 & RTW_C2H_RA_RPT_RATE) as u8;
                last_sgi = c.payload[0] as u32 & RTW_C2H_RA_RPT_SGI != 0;
                last_bw = c.payload[6];
            }
        });
        if n == 0 {
            host::sleep_ms(1);
        }
    }

    host::print("  C2H insgesamt ");
    host::print_dec(c2h_total);
    host::print(", davon RA_RPT ");
    host::print_dec(ra_rpt);
    if ra_rpt > 0 {
        host::print("\n  zuletzt gewaehlt: Rate 0x");
        host::print_hex8(last_rate);
        host::print(" (");
        host::print(rate_name(last_rate));
        host::print("), SGI ");
        host::print(if last_sgi { "ja" } else { "nein" });
        host::print(", bw ");
        host::print_dec(last_bw as u32);
    }
    host::print("\n");
    ok &= gate("die Firmware meldet eine gewaehlte Rate zurueck",
               ra_rpt > 0);
    *out = Some((caps, si));
    ok
}

/// main.h:250-262 `DESC_RATE*` as names, for the report. The exact name,
/// not the class.
fn rate_name(r: u8) -> &'static str {
    const HT: [&str; 16] = [
        "HT MCS0", "HT MCS1", "HT MCS2", "HT MCS3", "HT MCS4", "HT MCS5",
        "HT MCS6", "HT MCS7", "HT MCS8", "HT MCS9", "HT MCS10", "HT MCS11",
        "HT MCS12", "HT MCS13", "HT MCS14", "HT MCS15",
    ];
    if (DESC_RATEMCS0 as u8..=DESC_RATEMCS0 as u8 + 15).contains(&r) {
        return HT[(r - DESC_RATEMCS0 as u8) as usize];
    }
    match r {
        0x00 => "CCK 1M",
        0x01 => "CCK 2M",
        0x02 => "CCK 5,5M",
        0x03 => "CCK 11M",
        0x04 => "OFDM 6M",
        0x05 => "OFDM 9M",
        0x06 => "OFDM 12M",
        0x07 => "OFDM 18M",
        0x08 => "OFDM 24M",
        0x09 => "OFDM 36M",
        0x0a => "OFDM 48M",
        0x0b => "OFDM 54M",
        0x2c..=0x35 => "VHT 1SS",
        0x36..=0x3f => "VHT 2SS",
        _ => "?",
    }
}

/// The link counters. They belong to the link, not the stage: 6b continues
/// where 6a stopped, and a counter that resets in between misreports the
/// link.
#[derive(Clone, Copy)]
struct LinkStats {
    eapol_rx: u32,
    eapol_tx: u32,
    keys_set: u32,
    data_rx: u32,
    data_tx: u32,
    authorized: bool,
    link_up_sent: bool,
    extra_reported: u32,
    llc_miss: u32,
    rx_wd: u32,
    /// The last sequence control field per TID (slot 16 = non-QoS),
    /// `u32::MAX` = none yet. `rx.c:1480` `last_seq_ctrl[seqno_idx]`, sized
    /// `IEEE80211_NUM_TIDS + 1` as in `sta_info.h`: the TID field is four
    /// bits, so any value 0..15 can arrive.
    last_seq_ctrl: [u32; 17],
    /// Last CCMP packet number accepted per TID (16 = non-QoS), for the
    /// pairwise key [0] and the group key [1]: `key->u.ccmp.rx_pn` in
    /// mac80211, checked in `ieee80211_crypto_ccmp_decrypt`. The hardware
    /// decrypts but does not check for replays.
    rx_pn: [[u64; 17]; 2],
    /// Protected frames the hardware did not decrypt or whose ICV failed.
    rx_undecrypted: u32,
    /// Unprotected data frames dropped once the link is keyed.
    rx_plain_dropped: u32,
    /// Frames whose packet number was not above the last one (replays).
    rx_pn_replay: u32,
    /// Dropped 802.11 retransmissions (`dot11FrameDuplicateCount`).
    dup_rx: u32,
    /// Frames with the retry bit set (802.11 §9.2.4.1.8).
    ///
    /// This shows whether the AP has reason to slow down. `dup_rx` does
    /// not: the duplicate check remembers exactly one sequence value per
    /// TID, and a retransmission after an aggregate never hits it. The
    /// retry bit is the sender's statement and needs no memory.
    retry_rx: u32,
    /// Reorder buffer per TID: is a block ack session running?
    ro_on: [bool; RO_TIDS],
    /// Next expected sequence number (12 bits).
    ro_head: [u16; RO_TIDS],
    /// Window slot -> pool index + 1, 0 = empty.
    ro_slot: [[u8; RO_WIN]; RO_TIDS],
    /// How many frames of this TID are currently held.
    ro_held: [u8; RO_TIDS],
    /// When the head was last blocked (ms), for the timeout.
    ro_since: [u32; RO_TIDS],
    /// Counters for the report.
    ro_sorted: u32,
    ro_old: u32,
    ro_timeout: u32,
    ro_full: u32,
    /// A-MSDU: MPDUs with the A-MSDU bit set, the subframes from them, and
    /// how many were dropped whole (`goto purge` in cfg80211).
    amsdu_rx: u32,
    amsdu_sub: u32,
    amsdu_bad: u32,
    /// Group rekeys. Every EAPOL after the handshake is one (msg1 of the
    /// group key sequence, or a whole new four-way), and every answer is
    /// counted next to it. "3 received, 0 answered" points at us; "3/3"
    /// followed by a kick points elsewhere, and the reason code says where.
    rekey_rx: u32,
    rekey_tx: u32,
    /// Every GTK `wifid` has us write into the CAM.
    gtk_set: u32,
    /// What the receive loop saw, handled by the loop afterwards: `(was it
    /// a deauth, reason code)`. The callback only records;
    /// `netdev_set_link` and `EV_LINK_DOWN` do not belong in a callback that
    /// runs while the ring is being drained.
    gone: Option<(bool, u16)>,
    /// How often we were kicked, and the last reason given.
    kicked: u32,
    last_reason: u16,
    /// The firmware's TX report (`rtw_tx_report_*`, tx.c): whether a sent
    /// frame arrived.
    probes: [TxProbe; TX_PROBE_SLOTS],
    probe_sn: u8,
    /// acknowledged · not acknowledged · no firmware report at all
    tx_acked: u32,
    tx_lost: u32,
    tx_no_report: u32,
    /// The firmware declared itself dead.
    fw_crash: u32,
    /// How often we rebuilt the link.
    reconnects: u32,
    /// Census of unhandled C2H IDs. Four slots, each `(id, count)`: this
    /// firmware sends no more distinct ones, and the most frequent is the
    /// interesting one.
    c2h_ids: [(u8, u32); 4],
    /// Management frames of our cell, counted by subtype (16 slots, one per
    /// subtype; the list is closed).
    mgmt_sub: [u32; 16],
    /// For action frames, category/action of the last one and the number of
    /// ADDBA requests.
    addba_req: u32,
    /// And our own requests, in the other direction.
    addba_tx: u32,
    /// AP requests that did not even fit into the staging buffer.
    addba_drop: u32,
    /// `IEEE80211_STA_CONNECTION_POLL`: we are currently probing the AP.
    poll_on: bool,
    /// `ifmgd->probe_send_count`
    probe_send_count: u32,
    /// `ifmgd->probe_timeout`
    probe_timeout_ms: u64,
    /// How often the watchdog fired and how often it was right.
    poll_started: u32,
    poll_recovered: u32,
    /// How often we followed the AP to a new channel, and how often nobody
    /// was there.
    csa_done: u32,
    csa_back: u32,
    /// How many frames were in the ring per kick. It bounds what the
    /// hardware can aggregate: with one on average, the best block ack
    /// session does not help.
    tx_batch_n: u32,
    tx_batch_sum: u32,
    tx_batch_max: u32,
    /// And how much the hardware still had queued at the same moment. This
    /// decides aggregation; `tx_batch_*` only says how much the driver
    /// queued in one pass, which differs once the medium is busy.
    tx_ring_sum: u32,
    tx_ring_max: u32,
    /// Aggregate size in the receive direction, from the descriptor's
    /// `ppdu_cnt`: transmissions and the frames in them.
    rx_ppdu_n: u32,
    rx_data_ppdu_frames: u32,
    last_ppdu: u8,
    /// Interval between two AP transmissions, from the 802.11 clock.
    last_tsf: u32,
    rx_gap_sum: u64,
    rx_gap_n: u32,
    rx_gap_min: u32,
    /// The distribution, not the mean. Buckets per `GAP_BUCKETS`; the fifth
    /// is "over 10 ms" and keeps its sum.
    rx_gap_buckets: [u32; 5],
    rx_gap_big_sum: u64,
    /// And the intervals with no traffic at all.
    rx_gap_idle: u32,
    /// Turnaround time of our own stack: data to the kernel -> frame back
    /// from the kernel.
    last_rx_at: u64,
    turn_sum: u64,
    turn_n: u32,
    turn_max: u64,
    turn_buckets: [u32; 5],
    turn_big_sum: u64,
    last_action: (u8, u8),
    /// How often we agreed, and how often the response did not fit into
    /// the TX ring.
    addba_resp: u32,
    addba_fail: u32,
    /// The window we last granted and the one requested. Shows whether a
    /// changed `ampdu:` line was read; the driver reads it once when the
    /// loop starts.
    addba_win: u16,
    addba_win_req: u16,
    /// Shape of the receive loop: polls with and without frames, the frame
    /// sum, and how often a poll drained the whole batch.
    rx_polls: u32,
    rx_empty: u32,
    rx_frames: u32,
    rx_full: u32,
    /// Wall time in `rx_poll` when it returned frames, and how long the loop
    /// has run overall; their ratio is the receive path load.
    ///
    /// `rate_hist` below is the rate histogram over the whole link. Linux
    /// keeps `cur_pkt_count.num_qry_pkt[rate]` per watchdog tick and shifts
    /// it into `last_pkt_count`, read live via debugfs. Without debugfs, a
    /// report read after a transfer needs a number that outlasts it;
    /// `curr_rx_rate` is the rate of the last frame, which is usually a
    /// beacon at the lowest basic rate.
    rx_us: u64,
    pump_us0: u64,
    /// What corrupts the air, summed: `false_alarm_statistics` reads one
    /// CRC counter per modulation and resets it. A snapshot says nothing;
    /// the sum over the link shows whether the AP keeps retransmitting.
    ht_ok: u64,
    ht_err: u64,
    ofdm_ok: u64,
    ofdm_err: u64,
    rate_hist: [u32; DESC_RATE_MAX],
    /// The width frames actually arrived in: 20/40/80 and a fourth slot for
    /// anything else.
    ///
    /// The only proof of 80 MHz: everything else in the report is our own
    /// setting (descriptor and PHY), while the RX status says what the AP
    /// really sends.
    bw_hist: [u32; 4],
    /// How often the firmware reported its rate choice (`C2H_RA_RPT`). Zero
    /// would mean `dm.tx_rate` is 0 = CCK 1M, which makes
    /// `config_swing_table` pick the CCK curve of TX power tracking.
    ra_rpt_n: u32,
}

impl Default for LinkStats {
    /// By hand because `[u32; 84]` has no `Default` (derive goes up to 32).
    /// `zeroed` would work but needs an `unsafe` for a struct of plain
    /// numbers and `bool`.
    fn default() -> Self {
        LinkStats {
            eapol_rx: 0, eapol_tx: 0, keys_set: 0, data_rx: 0, data_tx: 0,
            authorized: false, link_up_sent: false, extra_reported: 0,
            llc_miss: 0, rx_wd: 0, last_seq_ctrl: [u32::MAX; 17], rx_pn: [[0; 17]; 2], rx_undecrypted: 0, rx_plain_dropped: 0, rx_pn_replay: 0, dup_rx: 0,
            retry_rx: 0,
            ro_on: [false; RO_TIDS], ro_head: [0; RO_TIDS],
            ro_slot: [[0; RO_WIN]; RO_TIDS], ro_held: [0; RO_TIDS],
            ro_since: [0; RO_TIDS], ro_sorted: 0, ro_old: 0,
            ro_timeout: 0, ro_full: 0, amsdu_rx: 0, amsdu_sub: 0, amsdu_bad: 0,
            rekey_rx: 0, rekey_tx: 0, gtk_set: 0,
            gone: None, kicked: 0, last_reason: 0,
            probes: [TxProbe { sn: 0, at_ms: 0, busy: false }; TX_PROBE_SLOTS],
            probe_sn: 0, tx_acked: 0, tx_lost: 0, tx_no_report: 0,
            fw_crash: 0, reconnects: 0, c2h_ids: [(0, 0); 4],
            mgmt_sub: [0; 16], addba_req: 0, addba_tx: 0, addba_drop: 0,
            poll_on: false, probe_send_count: 0, probe_timeout_ms: 0,
            poll_started: 0, poll_recovered: 0, csa_done: 0,
            csa_back: 0,
            tx_batch_n: 0, tx_batch_sum: 0, tx_batch_max: 0,
            tx_ring_sum: 0, tx_ring_max: 0,
            rx_ppdu_n: 0, rx_data_ppdu_frames: 0, last_ppdu: 0xff,
            last_tsf: 0, rx_gap_sum: 0, rx_gap_n: 0, rx_gap_min: 0,
            rx_gap_buckets: [0; 5], rx_gap_big_sum: 0, rx_gap_idle: 0,
            last_rx_at: 0, turn_sum: 0, turn_n: 0, turn_max: 0,
            turn_buckets: [0; 5], turn_big_sum: 0,
            last_action: (0, 0),
            addba_resp: 0, addba_fail: 0,
            addba_win: 0, addba_win_req: 0,
            rx_polls: 0, rx_empty: 0, rx_frames: 0, rx_full: 0,
            rx_us: 0, pump_us0: 0,
            ht_ok: 0, ht_err: 0, ofdm_ok: 0, ofdm_err: 0,
            rate_hist: [0; DESC_RATE_MAX], bw_hist: [0; 4], ra_rpt_n: 0,
        }
    }
}

impl LinkStats {
    /// tx.c:166-211 `rtw_tx_report_enable` + `rtw_tx_report_enqueue` in one:
    /// assign a number and take a slot.
    ///
    /// Returns `None` when all eight slots are busy; the firmware is not
    /// answering then anyway, and a ninth request does not help.
    fn arm_probe(&mut self, now: u64) -> Option<u8> {
        let slot = self.probes.iter().position(|p| !p.busy)?;
        let sn = tx::report_seqnum(&mut self.probe_sn);
        self.probes[slot] = TxProbe { sn, at_ms: now, busy: true };
        Some(sn)
    }

    /// tx.c:229-256 `rtw_tx_report_handle`: match the answer.
    fn settle_probe(&mut self, sn: u8, acked: bool) {
        if let Some(p) = self.probes.iter_mut().find(|p| p.busy && p.sn == sn) {
            p.busy = false;
            if acked {
                self.tx_acked += 1;
            } else {
                self.tx_lost += 1;
            }
        }
    }

    /// Counts a management frame.
    fn note_mgmt(&mut self, subtype: u8, cat: u8, action: u8) {
        self.mgmt_sub[(subtype & 0xf) as usize] += 1;
        if cat != 0xff {
            self.last_action = (cat, action);
            if cat == DOT11_ACTION_CAT_BA && action == DOT11_ACTION_ADDBA_REQ {
                self.addba_req += 1;
            }
        }
    }

    /// Counts an unhandled C2H ID. Four slots, after that only the ones
    /// already present: the census is meant to find the most frequent one.
    fn note_c2h(&mut self, id: u8) {
        if let Some(e) = self.c2h_ids.iter_mut().find(|e| e.1 > 0 && e.0 == id) {
            e.1 += 1;
            return;
        }
        if let Some(e) = self.c2h_ids.iter_mut().find(|e| e.1 == 0) {
            *e = (id, 1);
        }
    }

    /// tx.c:179-194 `rtw_tx_report_purge_timer`: "failed to get tx report
    /// from firmware". A deadline, not a round count.
    fn purge_probes(&mut self, now: u64) {
        for p in self.probes.iter_mut() {
            if p.busy && now.wrapping_sub(p.at_ms) > RTW_TX_PROBE_TIMEOUT_MS {
                p.busy = false;
                self.tx_no_report += 1;
            }
        }
    }

    /// When `purge_probes` gives up the next open report, i.e. when the pump
    /// has to look again.
    fn next_probe_due(&self) -> Option<u64> {
        self.probes.iter()
            .filter(|p| p.busy)
            .map(|p| p.at_ms + RTW_TX_PROBE_TIMEOUT_MS + 1)
            .min()
    }
}

/// tx.c `struct rtw_tx_report`: frames awaiting a TX report.
///
/// Linux queues the `sk_buff`s because it hands them back to mac80211. We
/// only need "did it arrive?", so a sequence number and when it was asked.
/// Eight slots: more open reports mean the firmware is not answering, which
/// `tx_no_report` counts.
#[derive(Clone, Copy, Default)]
struct TxProbe {
    sn: u8,
    at_ms: u64,
    busy: bool,
}

const TX_PROBE_SLOTS: usize = 8;

/// Bucket limits for the gap histogram, in microseconds.
///
/// A mean hides the distribution: an average of 1.9 ms can mean every
/// aggregate arrives after 1.9 ms, or most after 0.3 ms with a long stall
/// every thirtieth. Only the second is a problem.
const GAP_BUCKETS: [u32; 4] = [500, 2_000, 5_000, 10_000];
/// The same for our stack's turnaround, an order of magnitude finer: there
/// a millisecond is already a lot.
const TURN_BUCKETS: [u32; 4] = [200, 1_000, 5_000, 20_000];

/// Beyond this it is no stall but no traffic. Seconds between two downloads
/// do not belong in the same sum as a stall mid-stream.
const GAP_IDLE_US: u32 = 200_000;

fn bucket(us: u32, grenzen: &[u32; 4]) -> usize {
    let mut i = 0;
    while i < 4 {
        if us < grenzen[i] {
            return i;
        }
        i += 1;
    }
    4
}

// ═══════════════════════════════════════════════════════════════
// Roaming: switch AP before the link breaks
//
// The trigger comes from mac80211 (`ieee80211_handle_beacon_sig`,
// mlme.c:6780-6870): an EWMA over beacon signal, only after
// `IEEE80211_SIGNAL_AVE_MIN_COUNT` beacons, with threshold and hysteresis;
// an event fires again only once the level moves past the hysteresis.
// Without that a single bad beacon would trigger a scan.
//
// Candidate selection is a policy choice. In Linux it lives in
// wpa_supplicant (`wpa_scan_result_compar`). The rule here has the same
// shape as `PREFER_5G_DBM`, and the numbers are named constants so they
// can be tuned.
// ═══════════════════════════════════════════════════════════════

/// `roam:` from `sys/config/wifi`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoamMode {
    /// Scan and switch.
    An,
    /// Do not scan at all.
    Aus,
    /// Scan and report, but do not switch, so thresholds can be set from
    /// real numbers.
    NurBericht,
}

/// The pure part, so `framecheck.py` can run it without hardware.
///
/// The default is `NurBericht`, an exception to the rule that an unknown
/// value means the default: a switch interferes with a running link, and
/// until re-association after a switch is reliable, roaming must not touch
/// a working link. It scans and reports what it would do, which costs
/// nothing and yields the numbers for the thresholds.
///
/// `on` arms it, `off` disables it.
pub fn roam_from(v: &[u8]) -> RoamMode {
    if v.starts_with(b"off") || v.starts_with(b"aus") || v == b"0" {
        RoamMode::Aus
    } else if v.starts_with(b"on") || v.starts_with(b"an") || v == b"1" {
        RoamMode::An
    } else {
        RoamMode::NurBericht
    }
}

fn read_roam_mode() -> RoamMode {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return RoamMode::NurBericht;
    }
    match cfg_get(&cfg[..n as usize], b"roam") {
        Some((a, b)) => roam_from(&cfg[a..b]),
        None => RoamMode::NurBericht,
    }
}

/// mlme.c:96 `IEEE80211_SIGNAL_AVE_MIN_COUNT`: below four beacons the
/// smoothed level means nothing.
const SIGNAL_AVE_MIN_COUNT: u32 = 4;

/// Start scanning below this. Policy; the same threshold as
/// `PREFER_5G_DBM`.
const ROAM_THOLD_DBM: i8 = -70;
/// `cqm_rssi_hyst`: how far the level must rise again before the threshold
/// fires again. Policy.
const ROAM_HYST_DB: i8 = 4;
/// Minimum interval between two scans. Policy: each costs latency.
const ROAM_SCAN_GAP_MS: u64 = 10_000;
/// Minimum interval between two switches, the hysteresis against
/// ping-ponging between two equally good cells. Policy.
const ROAM_GAP_MS: u64 = 10_000;
/// How much stronger a candidate must be. Policy.
const ROAM_BETTER_DB: i8 = 8;
/// What halving the bandwidth may cost, in dB. Policy: half the width is
/// half the gross rate, and a few dB more signal on a busy link gives far
/// less than double. Ten dB per step means a candidate must be twenty dB
/// better to go from 80 to 20 MHz.
const ROAM_NARROWER_COST_DB: i8 = 10;
/// ... or the candidate is wider (VHT80 vs. HT40) and at most this much
/// weaker. Policy: a stronger but narrower range extender can deliver half the
/// throughput of a weaker, wider AP.
const ROAM_WIDER_TOLERANCE_DB: i8 = 6;
/// How long to listen per channel. A directed probe request is answered
/// within milliseconds; passive listening would need a full beacon
/// interval (102 ms) per channel.
const ROAM_DWELL_MS: u32 = 25;
/// How many channels to remember from the scan.
const ROAM_CHANNELS_MAX: usize = 6;
/// And how many cells one roam scan can find.
const ROAM_BSS_MAX: usize = 8;

/// The channels on which the initial scan saw cells of our SSID.
///
/// This is what makes a roam scan cheap: the SSID and its channels are
/// already known, as with `bgscan simple` in wpa_supplicant. The stored
/// levels are not used; they are from the initial scan and possibly another
/// place, and a stale level is worse than none. Only where to look is
/// stored.
static mut ROAM_CHANNELS: [u8; ROAM_CHANNELS_MAX] = [0; ROAM_CHANNELS_MAX];
static mut N_ROAM_CHANNELS: usize = 0;

/// Roaming state of a running link.
#[derive(Clone, Copy)]
struct Roam {
    /// `ewma_beacon_signal`, `DECLARE_EWMA(beacon_signal, 4, 4)` (mac80211
    /// ieee80211_i.h:518). Computed on `level + 128` because our `Ewma` is
    /// unsigned.
    ave: dm::Ewma,
    /// `count_beacon_signal`
    count: u32,
    /// `last_cqm_event_signal`; 0 means "never fired".
    last_event: i8,
    last_scan_ms: u64,
    last_roam_ms: u64,
    /// How often we switched and how often we scanned.
    scans: u32,
    roams: u32,
    /// The candidate the caller should switch to.
    to: Option<Bss>,
}

impl Roam {
    const fn new() -> Self {
        Roam { ave: dm::Ewma::new(), count: 0, last_event: 0,
               last_scan_ms: 0, last_roam_ms: 0, scans: 0, roams: 0,
               to: None }
    }
    /// A beacon of our own cell.
    fn note_beacon(&mut self, dbm: i8) {
        self.ave.add((dbm as i32 + 128) as u32, EWMA_BEACON_PRECISION,
                     EWMA_BEACON_WEIGHT_RCP);
        self.count = self.count.saturating_add(1);
    }
    /// The smoothed level in dBm.
    fn dbm(&self) -> i8 {
        (self.ave.read(EWMA_BEACON_PRECISION) as i32 - 128) as i8
    }
}

/// `DECLARE_EWMA(beacon_signal, 4, 4)`: precision 4, weight 1/16.
const EWMA_BEACON_PRECISION: u32 = 4;
const EWMA_BEACON_WEIGHT_RCP: u32 = 16;

/// A null data frame, `ieee80211_send_nullfunc` (mlme.c:2364).
///
/// This makes a roam scan cheap: with the power management bit set it tells
/// the AP "dozing", and the AP buffers our packets instead of sending them
/// to a channel we have left. On return the same with the bit cleared, and
/// the AP flushes the buffer (`ieee80211_offchannel_ps_enable`/`_disable`,
/// offchannel.c:25-81). Without it each scan loses packets; with it, only
/// latency.
fn build_nullfunc(out: &mut [u8; 32], mac: &[u8; 6], bssid: &[u8; 6],
                  powersave: bool) -> usize {
    out.fill(0);
    // Type data (0b10), subtype 4 = null data.
    out[0] = DOT11_FC_TYPE_DATA | (4 << 4);
    // ToDS, plus the power management bit (802.11 §9.2.4.1.7).
    out[1] = 0x01 | if powersave { 0x10 } else { 0x00 };
    out[4..10].copy_from_slice(bssid); // addr1 = receiver
    out[10..16].copy_from_slice(mac); // addr2 = us
    out[16..22].copy_from_slice(bssid); // addr3 = BSSID
    24
}

// ═══════════════════════════════════════════════════════════════
// Link watchdog — mlme.c:4278-4481, 8516-8560
//
// Missing beacons are not link loss. Linux probes the AP first and gives up
// only when that stays silent too. `rtw_sw_beacon_loss_check` feeds
// `d.beacon_loss`.
// ═══════════════════════════════════════════════════════════════

/// mlme.c:58 `max_probe_tries`.
const MAX_PROBE_TRIES: u32 = 5;
/// mlme.c:86 `probe_wait_ms`.
const PROBE_WAIT_MS: u64 = 500;
/// mlme.c:4391 `unicast_limit = max(1, max_probe_tries - 3)`.
///
/// The last three attempts go out as broadcast; per the source comment some
/// APs answer only broadcast probes.
const PROBE_UNICAST_LIMIT: u32 = if MAX_PROBE_TRIES > 4 {
    MAX_PROBE_TRIES - 3
} else {
    1
};

/// The TID our data runs on. Best effort, and the only one: without EDCA
/// from the AP there is no reason for a second queue, and each costs its
/// own block ack session.
const BA_TX_TID: u8 = 0;

/// How long to wait for the ADDBA response before asking again. mac80211:
/// `ADDBA_RESP_INTERVAL` = HZ/5.
const BA_RESP_MS: u64 = 200;
/// How many times. mac80211 gives up after `HT_AGG_MAX_RETRIES` (15), we
/// after three; a link without aggregation is slow, not broken.
const BA_MAX_TRIES: u32 = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum BaState {
    /// Not asked yet, or not to be asked (`txagg: off`).
    Aus,
    /// Asked, response pending.
    Gefragt,
    /// The AP agreed. From here every frame of this TID carries AGG_EN.
    Laeuft,
    /// Rejected, or unanswered after three attempts. No retry: an AP silent
    /// three times stays silent, and a loop on the management path costs
    /// airtime.
    Aufgegeben,
}

/// Our block ack session in the transmit direction.
///
/// In Linux it lives in `tid_ampdu_tx`, driven by
/// `ieee80211_tx_ba_session_handle_start`; the driver only sees
/// `IEEE80211_AMPDU_TX_OPERATIONAL`. Without mac80211 the state machine is
/// here.
#[derive(Clone, Copy)]
struct BaTx {
    state: BaState,
    tid: u8,
    /// The token we asked with. A response with another one belongs to an
    /// earlier request (agg-tx.c:1002).
    token: u8,
    tries: u32,
    at_ms: u64,
    /// What the AP granted, in frames.
    win: u16,
    /// `MAX_AGG_NUM` and `AMPDU_DEN` for the descriptor, from the AP's HT
    /// capabilities.
    factor: u8,
    density: u8,
    /// The status of its rejection, for the report.
    status: u16,
}

impl BaTx {
    const fn new() -> Self {
        BaTx { state: BaState::Aus, tid: BA_TX_TID, token: 0, tries: 0,
               at_ms: 0, win: 0, factor: 0, density: 0, status: 0 }
    }
    fn laeuft(&self, tid: u8) -> bool {
        self.state == BaState::Laeuft && self.tid == tid
    }
}

/// State of a running link, stage 6a.
struct Link {
    bssid: [u8; 6],
    mac: [u8; 6],
    channel: u8,
    /// The cell name. Needed for the directed probe request
    /// (`ieee80211_ap_probereq_get`, mlme.c:4518-4521: the SSID element
    /// carries the one AP's name, not zero length) and to find cells of the
    /// same SSID when roaming.
    ssid: [u8; 32],
    ssid_len: u8,
    si: sta::StaInfo,
    highest_rate: u8,
    /// Running sequence number for data frames. The chip assigns it with
    /// `en_hwseq`, but `pkt_info.seq` is still in the descriptor; Linux fills
    /// it from the frame header.
    seq: u16,
    ptk_installed: bool,
    /// 802.11 §12.5.3.2: the 48-bit packet number of the pairwise key. It
    /// starts at one and increments per frame; the AP drops a repeated
    /// number as a replay.
    tx_pn: u64,
    cam: [sec::CamEntry; 4],
    /// Our block ack session in the transmit direction.
    ba_tx: BaTx,
    /// Roaming: smoothed level, holds, candidate.
    roam: Roam,
    /// The width the link runs in, as the three bytes `chan_params` computes
    /// it from. The return from a roam scan needs them, otherwise the link
    /// silently comes back at 20 MHz.
    ht_param_now: u8,
    vht_chanwidth_now: u8,
    vht_cch0_now: u8,
    /// A pending channel switch announcement: where, how wide, from when.
    /// `None` means none announced.
    csa: Option<Csa>,
    /// When to move (`link->u.mgd.csa.time`, mlme.c:2992).
    csa_at_ms: u64,
    /// Where to return if nobody is on the new channel.
    csa_zurueck: Option<(u8, CellWidth)>,
    csa_frist_ms: u64,
}

/// 802.11 §12.5.3.2: the eight-byte CCMP header.
///
/// The packet number is split in two parts by design: byte 2 is reserved
/// and byte 3 carries the ExtIV bit and key ID, so a legacy WEP receiver
/// recognizes the frame as extended.
fn ccmp_hdr(out: &mut [u8], pn: u64, key_id: u8) {
    out[0] = (pn & 0xff) as u8; // PN0
    out[1] = ((pn >> 8) & 0xff) as u8; // PN1
    out[2] = 0; // reserved
    out[3] = 0x20 | (key_id << 6); // ExtIV | KeyID
    out[4] = ((pn >> 16) & 0xff) as u8; // PN2
    out[5] = ((pn >> 24) & 0xff) as u8; // PN3
    out[6] = ((pn >> 32) & 0xff) as u8; // PN4
    out[7] = ((pn >> 40) & 0xff) as u8; // PN5
}

/// docs/spec/WIFI_CLASS_ABI.md §2b: an Ethernet frame as an 802.11 data
/// frame to the AP.
///
/// 802.3: `[DA 6][SA 6][ethertype 2][payload]`
/// 802.11 ToDS: `[fc 2][dur 2][addr1=BSSID][addr2=SA][addr3=DA][seq 2]`
/// plus LLC/SNAP (RFC 1042) and the ethertype.
///
/// `qos` selects the frame type, and everything depends on it: block ack
/// needs a TID, only a QoS frame carries one, and without block ack every
/// frame goes out alone. An HT station is a QoS station (802.11 §10.2.3),
/// and block ack exists only between QoS stations (§10.24.2).
///
/// `probe` is `IEEE80211_TX_CTL_REQ_TX_STATUS`, which mac80211 sets for
/// frames whose loss costs the link (control port, i.e. EAPOL). Returns the
/// sequence number the firmware will report under.
///
/// EAPOL goes without QoS: the four-way handshake runs before any block ack
/// session exists.
fn tx_8023(h: i32, trx: &mut pci::Trx, mgmt_buf: i32, link: &mut Link,
           eth: &[u8], encrypt: bool, probe: Option<u8>,
           qos: Option<u8>) -> bool {
    if eth.len() < 14 {
        return false;
    }
    let mut frame = [0u8; 2048];
    let payload = &eth[14..];
    let hdrlen = if qos.is_some() { 26 } else { 24 };
    let total = hdrlen + if encrypt { 8 } else { 0 } + 6 + 2 + payload.len();
    if total > frame.len() {
        return false;
    }

    frame[0] = DOT11_FC_TYPE_DATA;
    if qos.is_some() {
        // The subtype is in bits 7:4 and `DOT11_STYPE_QOS` is the nibble
        // value, hence the shift.
        frame[0] |= DOT11_STYPE_QOS << 4;
    }
    frame[1] = 0x01; // ToDS
    if encrypt {
        frame[1] |= DOT11_FC_PROTECTED;
    }
    frame[2..4].copy_from_slice(&0u16.to_le_bytes());
    frame[4..10].copy_from_slice(&link.bssid); // addr1 = receiver
    frame[10..16].copy_from_slice(&link.mac); // addr2 = source
    frame[16..22].copy_from_slice(&eth[0..6]); // addr3 = destination
    frame[22..24].copy_from_slice(&(link.seq << 4).to_le_bytes());
    if let Some(tid) = qos {
        // 802.11 §9.2.4.5 QoS Control: bits 3:0 the TID, bits 6:5 the ack
        // policy (00 = normal, which under block ack is the implicit block
        // ack request), bit 7 A-MSDU: no. Byte 1 is TXOP duration or queue
        // size and belongs to whoever requests it; we request nothing.
        frame[24] = tid & 0x0f;
        frame[25] = 0;
    }

    // The driver writes the CCMP header, not the hardware. `rtw_ops_set_key`
    // sets `IEEE80211_KEY_FLAG_GENERATE_IV`, which in mac80211 means the
    // stack reserves eight bytes and writes the packet number into them
    // (`ccmp_pn2hdr`); the hardware only encrypts. Without it the AP cannot
    // decrypt our frames.
    let ofs = if encrypt {
        ccmp_hdr(&mut frame[hdrlen..hdrlen + 8], link.tx_pn, 0);
        link.tx_pn = link.tx_pn.wrapping_add(1);
        hdrlen + 8
    } else {
        hdrlen
    };
    frame[ofs..ofs + 6].copy_from_slice(&LLC_SNAP_HDR);
    frame[ofs + 6..ofs + 8].copy_from_slice(&eth[12..14]); // ethertype
    frame[ofs + 8..ofs + 8 + payload.len()].copy_from_slice(payload);

    let mut info = tx::TxPktInfo::default();
    // `rtw_tx_pkt_info_update` for a data frame: rate first, then the common
    // fields.
    tx::data_pkt_info_update(&mut info, link.seq, Some(&link.si),
                             link.highest_rate);
    let a1 = &frame[4..10];
    info.bmc = a1.iter().all(|&b| b == 0xff) || a1[0] & 0x01 != 0;
    info.tx_pkt_size = total as u32;
    info.offset = tx::TX_PKT_DESC_SZ as u8;
    info.ls = true;
    info.mac_id = link.si.mac_id;
    // `rtw_tx_pkt_info_update_sec`: with a key installed the descriptor
    // carries the eight bytes of the CCMP header as extra length.
    if encrypt {
        info.sec_type = 0x3; // AES
    }

    link.seq = link.seq.wrapping_add(1) & 0x0fff;

    // tx.c:432-433 `if (info->flags & IEEE80211_TX_CTL_REQ_TX_STATUS)`. The
    // caller assigns the number (`rtw_tx_report_enable`) because it enters
    // it into its open list right after.
    if let Some(sn) = probe {
        info.sn = sn as u16;
        info.report = true;
    }

    // tx.c:361-365: `ampdu_en` follows `IEEE80211_TX_CTL_AMPDU`, which
    // mac80211 sets once a block ack session is open; here that is
    // `BaState::Laeuft` on this TID.
    //
    // The hardware aggregates, not the driver: the chip combines consecutive
    // frames of the same MACID and TID when AGG_EN is set. The driver only
    // says how much is allowed at once, using the limits the AP announced in
    // its HT capabilities, not ours.
    if let Some(tid) = qos {
        if link.ba_tx.laeuft(tid) {
            info.ampdu_en = true;
            info.ampdu_factor = link.ba_tx.factor;
            info.ampdu_density = link.ba_tx.density;
        }
    }

    // No kick here. tx.c:660-676: Linux queues `frame_cnt` frames and calls
    // `rtw_hci_tx_kick_off` once afterwards. That is a precondition for
    // aggregation, not an MMIO saving: the hardware can only combine what
    // is already in the ring when it gets a transmit opportunity.
    let queue = pci::Q_BE;
    pci::tx_write(h, trx, mgmt_buf, queue, &mut info, &frame[..total])
}

/// The 802.11 header length of a data frame, which locates the LLC/SNAP
/// header.
///
/// cfg80211 `ieee80211_hdrlen` (util.c:430-447) for a station's data frame:
/// 24, plus 2 for QoS control with QoS, plus 4 for HT control when the
/// order bit is set (we announce `+HTC-VHT`, so the AP may send it). Address
/// field 4 (FromDS and ToDS) does not occur for a station.
///
/// An encrypted frame carries the eight-byte CCMP header between the 802.11
/// header and the payload, and the hardware does not strip it:
/// `rtw_rx_fill_rx_status` sets `RX_FLAG_DECRYPTED` but not
/// `RX_FLAG_IV_STRIPPED`; in Linux mac80211 removes it. At the end are the
/// FCS (always) and with CCMP the eight-byte MIC, because `WLAN_RCR_CFG`
/// sets APP_FCS and APP_MIC.
fn data_hdrlen(f: &[u8]) -> usize {
    let qos = f[0] & (DOT11_STYPE_QOS << 4) != 0;
    if !qos {
        return 24;
    }
    26 + if f[1] & 0x80 != 0 { 4 } else { 0 }
}

fn llc_offset(f: &[u8], miss: &mut u32) -> Option<(usize, usize)> {
    // The subtype is in bits 7:4. `DOT11_STYPE_QOS` (0x08) without the shift
    // equals `DOT11_FC_TYPE_DATA`, which would mark every data frame as QoS.
    let hdrlen = data_hdrlen(f);
    let prot = f[1] & DOT11_FC_PROTECTED != 0;
    let crypt = if prot { 8usize } else { 0 };
    let trailing = 4 + if prot { 8usize } else { 0 };

    let at = |o: usize| o + 6 <= f.len() && f[o..o + 6] == LLC_SNAP_HDR;
    let want = hdrlen + crypt;
    let found = if at(want) {
        want
    } else if at(hdrlen) {
        if *miss < 3 {
            *miss += 1;
            host::print("    [Befund] LLC/SNAP steht bei ");
            host::print_dec(hdrlen as u32);
            host::print(" statt ");
            host::print_dec(want as u32);
            host::print(" — der CCMP-Kopf fehlt\n");
        }
        hdrlen
    } else if at(hdrlen + 8) {
        if *miss < 3 {
            *miss += 1;
            host::print("    [Befund] LLC/SNAP steht bei ");
            host::print_dec((hdrlen + 8) as u32);
            host::print(" statt ");
            host::print_dec(want as u32);
            host::print("\n");
        }
        hdrlen + 8
    } else {
        return None;
    };
    if found + 8 + trailing > f.len() {
        return Some((found, 0));
    }
    Some((found, trailing))
}

/// Detects a deauthentication or disassociation from our AP.
///
/// `rx_to_8023` filters for data frames in its first line, so a deauth (a
/// management frame) would otherwise pass unnoticed: the link goes silent
/// while the kernel still believes `carrier UP`.
///
/// 802.11 §9.4.1.7: deauthentication (subtype 12) and disassociation
/// (subtype 10) carry a little-endian reason code right after the 24-byte
/// header. Returns `(was it a deauth, reason code)`.
///
/// Only from `addr2 == BSSID`: a deauth of another cell is none of our
/// business.
fn disconnect_reason(f: &[u8], bssid: &[u8; 6]) -> Option<(bool, u16)> {
    // 24 bytes header + 2 bytes reason. Anything shorter is not a valid
    // frame, and guessing would be worse than silence.
    if f.len() < 26 {
        return None;
    }
    let deauth = match f[0] {
        DOT11_FC_DEAUTH => true,
        DOT11_FC_DISASSOC => false,
        _ => return None,
    };
    if f[10..16] != bssid[..] {
        return None;
    }
    Some((deauth, u16::from_le_bytes([f[24], f[25]])))
}

/// 802.11 §9.4.1.7 table 9-49. 15 and 16 point at the four-way handshake
/// and the group rekey respectively, rather than at the air.
fn reason_name(code: u16) -> &'static str {
    match code {
        1 => "unspezifiziert",
        2 => "vorige Authentifizierung ungueltig",
        3 => "die Station verlaesst die Zelle",
        4 => "Inaktivitaet",
        5 => "dem AP gehen die Plaetze aus",
        6 => "Klasse-2-Rahmen von nicht authentifizierter Station",
        7 => "Klasse-3-Rahmen von nicht assoziierter Station",
        8 => "die Station verlaesst die Zelle (Disassoc)",
        9 => "Assoziation ohne vorherige Authentifizierung",
        13 => "ungueltiges Informationselement",
        14 => "MIC-Fehler",
        15 => "Vierwegehandschlag: Zeitueberschreitung",
        16 => "Gruppenschluessel-Handschlag: Zeitueberschreitung",
        17 => "IE weicht vom Anmeldeantrag ab",
        18 => "ungueltige Gruppen-Chiffre",
        19 => "ungueltige Paar-Chiffre",
        20 => "ungueltige AKM",
        23 => "802.1X-Authentifizierung fehlgeschlagen",
        24 => "Chiffre durch Sicherheitsregel abgelehnt",
        34 => "zu schlechte Verbindung (BSS Transition)",
        _ => "unbekannt",
    }
}

/// docs/spec/WIFI_CLASS_ABI.md §2b, demux rule: a received 802.11 data frame
/// becomes 802.3 and goes either to `wifid` as `EAPOL_RX` or to the IP
/// stack.
///
/// Returns the length of the 802.3 frame in `out` and whether it was EAPOL.
fn rx_to_8023(f: &[u8], out: &mut [u8], miss: &mut u32)
    -> Option<(usize, bool)>
{
    if f.len() < 24 || f[0] & 0x0c != DOT11_FC_TYPE_DATA {
        return None;
    }
    // Null and QoS null carry no body.
    if f[0] & DOT11_STYPE_NODATA != 0 {
        return None;
    }
    let (llc, trailing) = llc_offset(f, miss)?;
    let body_end = f.len().saturating_sub(trailing);
    if body_end < llc + 8 {
        return None;
    }
    let et = u16::from_be_bytes([f[llc + 6], f[llc + 7]]);
    let payload = &f[llc + 8..body_end];
    if out.len() < 14 + payload.len() {
        return None;
    }
    // FromDS: addr1 = us, addr2 = BSSID, addr3 = source.
    out[0..6].copy_from_slice(&f[4..10]);
    out[6..12].copy_from_slice(&f[16..22]);
    out[12..14].copy_from_slice(&f[llc + 6..llc + 8]);
    out[14..14 + payload.len()].copy_from_slice(payload);
    Some((14 + payload.len(), et == ETHERTYPE_EAPOL))
}

/// tx.c:367-374, the order in `rtw_tx_data_pkt_info_update`.
#[allow(clippy::too_many_arguments)]
///
/// VHT is checked before HT; checking only `ht_supported` would pick an HT
/// rate (`DESC_RATEMCS15`) on a VHT link.
fn highest_tx_rate(caps: &sta::PeerCaps, hal: &Hal) -> u8 {
    let nss = if hal.rf_2t2r { 2 } else { 1 };
    if caps.vht_supported {
        tx::highest_vht_tx_rate(caps.vht_tx_mcs_map, nss)
    } else if caps.ht_supported {
        tx::highest_ht_tx_rate(&caps.ht_mcs, hal.rf_2t2r)
    } else if caps.supp_rates & 0x000f == caps.supp_rates {
        // tx.c:371 `supp_rates[0] <= 0xf`: only the four CCK bits.
        DESC_RATE11M as u8
    } else {
        DESC_RATE54M as u8
    }
}

/// Sets up the link: registers with the kernel and arms `wifid`.
///
/// Must happen exactly once per link: a second `EV_READY` makes `wifid`
/// build a fresh supplicant waiting for an msg1 the AP never sends again.
fn link_setup(hal: &Hal, bss: &Bss, caps: &sta::PeerCaps, si: sta::StaInfo,
              mac: [u8; 6]) -> Link {
    let link = Link {
        bssid: bss.bssid,
        mac,
        channel: bss.channel,
        ssid: bss.ssid,
        ssid_len: bss.ssid_len,
        si,
        highest_rate: highest_tx_rate(caps, hal),
        seq: 0,
        ptk_installed: false,
        tx_pn: 1,
        cam: [sec::CamEntry::default(); 4],
        ba_tx: BaTx::new(),
        roam: Roam { last_roam_ms: host::now_ms(), ..Roam::new() },
        ht_param_now: bss.ht_param,
        vht_chanwidth_now: bss.vht_chanwidth,
        vht_cch0_now: bss.vht_cch0,
        csa: None,
        csa_at_ms: 0,
        csa_zurueck: None,
        csa_frist_ms: 0,
    };

    // Without registration the IP stack does not see the data channel.
    let reg = host::netdev_register(&mac);
    host::print("  netdev_register ");
    host::print(if reg >= 0 { "ok" } else { "FEHLGESCHLAGEN" });
    host::print("\n");

    // `EV_READY` = [0x83][ap_mac 6][our_mac 6]: `wifid` builds its
    // supplicant for exactly this cell.
    let mut ready = [0u8; 13];
    ready[0] = EV_READY;
    ready[1..7].copy_from_slice(&bss.bssid);
    ready[7..13].copy_from_slice(&mac);
    let sent = host::wifi_send_event(&ready);
    host::print("  EV_READY an wifid ");
    host::print(if sent >= 0 { "raus" } else { "FEHLGESCHLAGEN" });
    host::print(" — der Supplicant wird jetzt scharf gemacht\n");

    link
}

/// Longest duration of each watchdog part, in us.
#[allow(clippy::too_many_arguments)]
///
/// Linux runs the watchdog in a workqueue beside reception; here it runs in
/// the pump loop, and nobody drains the ring while it computes, so a long
/// part can overflow the ring. `do_lck` alone may poll up to 100 ms per
/// Linux. These numbers show which part it was.
static mut WD_MAX: [u32; 8] = [0; 8];
const WD_NAMES: [&str; 8] = ["coex", "statistik", "dig/cck", "ra/rrsr",
                             "pfad/cfo", "dpk", "pwr_track", "adaptivity"];

fn wd_mark(i: usize, tp: &mut u64) {
    let now = host::now_us();
    let d = now.saturating_sub(*tp).min(u32::MAX as u64) as u32;
    // SAFETY: single-threaded, only the pump thread writes, the report reads.
    unsafe {
        let m = &mut *core::ptr::addr_of_mut!(WD_MAX);
        if d > m[i] { m[i] = d; }
    }
    *tp = now;
}

/// The chip's MSI vector, `-1` = polling. Set at start (`rtw_hci_start`),
/// read when the pump loop is idle.
static mut IRQ_VEC: i32 = -1;

/// During a channel switch (CSA) the pump parks at most this long: beacon
/// deadline and switch time are rare and short, so the fine grid stays.
const CSA_WAIT_MS: u64 = 10;

/// The longest pump loop iteration without its sleep, in us, and whether
/// the watchdog ran in it.
static mut ITER_MAX: u32 = 0;
static mut ITER_MAX_WD: bool = false;

/// main.c:224-310 `rtw_watch_dog_work`, every two seconds for the life of a
/// link: crystal tracking, TX power over temperature, DPK tracking, RSSI to
/// the firmware's rate selection.
///
/// Not implemented here:
///
/// * `rtw_leave_lps` / `rtw_enter_lps` / `rtw_recalc_lps`: no power save.
/// * `rtw_hci_dynamic_rx_agg`: `.dynamic_rx_agg = NULL` for PCI
///   (pci.c:1605).
/// * `rtw_dynamic_csi_rate`: returns while the peer has no beamforming role;
///   `bf.c` is not ported.
/// * `rtw_coex_run_coex` (via `wl_status_change_notify`): the coexistence
///   decision tree (L6 of the plan, 111 functions).
///
/// Deviation: `rtw_coex_monitor_bt_enable` is called only from
/// `rtw_coex_run_coex` in Linux. It produces `bt_disabled`, which
/// `rtw8822c_cfo_need_adjust` depends on; without it crystal tracking would
/// stay off forever. So it is called here until L6 exists.
fn watch_dog(h: i32, hal: &Hal, d: &mut Dev, h2c: &mut fw::H2cState,
             e: &efuse::Efuse, link: &mut Link, caps: &sta::PeerCaps,
             fw_feature: u32, linked: bool, beacon_int: u16) {
    let received_beacons = d.dm.cur_pkt_count.num_bcn_pkt;

    // main.c:241-248: the threshold is 100 frames per tick.
    let busy_pre = d.busy_traffic;
    d.busy_traffic = d.stats.tx_cnt > RTW_BUSY_TRAFFIC_THRESHOLD
        || d.stats.rx_cnt > RTW_BUSY_TRAFFIC_THRESHOLD;
    if busy_pre != d.busy_traffic {
        // `rtw_coex_wl_status_change_notify(rtwdev, 0)` -> run_coex (L6)
    }

    // main.c:255-268: bytes per two seconds in Mbit/s, smoothed.
    let tx_mbps = (d.stats.tx_unicast >> RTW_TP_SHIFT) as u32;
    let rx_mbps = (d.stats.rx_unicast >> RTW_TP_SHIFT) as u32;
    d.stats.tx_ewma_tp.add(tx_mbps, dm::EWMA_TP_PRECISION,
                           dm::EWMA_TP_WEIGHT_RCP);
    d.stats.rx_ewma_tp.add(rx_mbps, dm::EWMA_TP_PRECISION,
                           dm::EWMA_TP_WEIGHT_RCP);
    d.stats.tx_throughput = d.stats.tx_ewma_tp.read(dm::EWMA_TP_PRECISION);
    d.stats.rx_throughput = d.stats.rx_ewma_tp.read(dm::EWMA_TP_PRECISION);
    d.stats.tx_peak = d.stats.tx_peak.max(d.stats.tx_throughput);
    d.stats.rx_peak = d.stats.rx_peak.max(d.stats.rx_throughput);
    d.stats.tx_unicast = 0;
    d.stats.rx_unicast = 0;
    d.stats.tx_cnt = 0;
    d.stats.rx_cnt = 0;

    let mut tp = host::now_us();
    // main.c:275-279
    coex::wl_status_check(h, &mut d.cx);
    coex::monitor_bt_enable(h, &mut d.cx);
    coex::active_query_bt_info(h, &mut d.cx);
    wd_mark(0, &mut tp);

    let band_2g = link.channel <= 14;
    // `si->ra_report.desc_rate`: what the firmware last picked, not what we
    // offered. It reports it as C2H `RA_RPT`; until one arrives this holds
    // the initial rate.
    let sta_rate = if linked { Some(link.si.ra_report_desc_rate) } else { None };
    let nss = if hal.rf_2t2r { 2 } else { 1 };
    let fw_adapt = fw_feature & FW_FEATURE_ADAPTIVITY != 0;

    // Two calls, one borrow at a time: `si` and `rssi_si` are the same
    // iterator over the same station in Linux, but here only one mutable
    // reference is allowed. Linux order: `statistics` first (RSSI included),
    // then the rest.
    phy::statistics(h, &mut d.dm, h2c, if linked { Some(&mut link.si) } else { None });
    wd_mark(1, &mut tp);
    phy::dig(h, &mut d.dm, hal.rf_path_num, linked);
    phy::cck_pd(h, &mut d.dm, band_2g, linked);
    wd_mark(2, &mut tp);
    phy::ra_track(h, &mut d.dm, h2c, d.stats.tx_throughput,
                  d.stats.rx_throughput, d.watch_dog_cnt,
                  if linked { Some((&mut link.si, caps, nss, band_2g)) } else { None },
                  sta_rate);
    wd_mark(3, &mut tp);
    phy::tx_path_diversity(h, &mut d.path_div, hal.antenna_tx, hal.antenna_rx,
                           linked);
    chip::cfo_track(h, &mut d.dm, hal.rf_path_num, e.crystal_cap, linked,
                    d.cx.bt_disabled);
    wd_mark(4, &mut tp);
    dpk::track(h, &mut d.dpk);
    wd_mark(5, &mut tp);
    chip::pwr_track(h, &mut d.dm, e.power_track_type, &e.thermal_meter,
                    hal.rf_path_num, link.channel);
    wd_mark(6, &mut tp);
    if fw_adapt {
        fw::adaptivity(h, h2c, &d.dm);
    } else {
        phy::adaptivity(h, &d.dm);
    }
    wd_mark(7, &mut tp);

    // main.c:196-207 `rtw_sw_beacon_loss_check`. Firmware with
    // `FW_FEATURE_BCN_FILTER` does it itself.
    if fw_feature & FW_FEATURE_BCN_FILTER == 0 && beacon_int > 0 {
        // watchdog_delay = 2000000 / 1024 TU
        let watchdog_delay = 2_000_000u32 / 1024;
        let expected = watchdog_delay.div_ceil(beacon_int as u32);
        d.beacon_loss = (received_beacons as u32) < expected / 2;
    }

    d.watch_dog_cnt = d.watch_dog_cnt.wrapping_add(1);
}

/// Why `link_pump` returned.
#[allow(clippy::too_many_arguments)]
#[derive(PartialEq, Clone, Copy)]
enum PumpEnd {
    /// A better AP was found; the candidate is in `link.roam.to`, and the
    /// caller associates there.
    Roam,
    /// Stage 6a's deadline expired, the normal case there.
    Frist,
    /// The cell dropped us (deauth/disassoc) or the firmware declared
    /// itself dead. The caller reconnects.
    LinkLost,
}

// ── Reorder buffer for received A-MPDUs ──────────────────────────
//
// `ieee80211_rx_reorder_ampdu` + `ieee80211_sta_reorder_release`
// (net/mac80211/rx.c), 802.11 §10.24.7 "Receive reordering buffer control".
//
// A frame lost within an A-MPDU arrives in the next burst. Delivered in
// arrival order (6, 7, 8 … 63, then 5), TCP sees a gap and sends duplicate
// ACKs, three of which trigger a needless fast retransmit at the sender.
// Without reordering the receive window cannot be opened wide.

/// How many TIDs can have a session at once. APs commonly open two (TID 0
/// and 6).
const RO_TIDS: usize = 8;
/// The window size we grant in ADDBA. Grant and buffer must match: granting
/// 64 and buffering 32 drops what was accepted.
const RO_WIN: usize = 64;
/// The buffer holds the raw MPDU, not the 802.3 frame. mac80211 reorders
/// MPDUs and unpacks an A-MSDU only afterwards (`ieee80211_rx_h_amsdu`
/// comes after reordering in the chain); one MPDU with an A-MSDU carries
/// many frames under one sequence number. The size is the largest MPDU we
/// announce in the VHT element (`MAX_MPDU_LENGTH_11454`).
const RO_FRAME: usize = 11454;
/// Frames held at once across all TIDs. Normally empty, only while a hole
/// is open. When the pool is full, frames are delivered rather than
/// dropped (`ro_full` counts it).
const RO_POOL: usize = 64;
/// Deadline for a hole, after which frames are released past it.
/// mac80211: `HT_RX_REORDER_BUF_TIMEOUT` = HZ/10.
const RO_TIMEOUT_MS: u32 = 100;

static mut RO_BUF: [[u8; RO_FRAME]; RO_POOL] = [[0; RO_FRAME]; RO_POOL];
static mut RO_LEN: [u16; RO_POOL] = [0; RO_POOL];
static mut RO_USED: [bool; RO_POOL] = [false; RO_POOL];

/// Takes a free pool slot and stores the MPDU in it.
fn ro_take(mpdu: &[u8]) -> Option<usize> {
    if mpdu.len() > RO_FRAME {
        return None;
    }
    // SAFETY: one thread, one caller, the same contract as RXBUF6.
    unsafe {
        let used = &mut *core::ptr::addr_of_mut!(RO_USED);
        let i = used.iter().position(|u| !*u)?;
        used[i] = true;
        let buf = &mut *core::ptr::addr_of_mut!(RO_BUF);
        buf[i][..mpdu.len()].copy_from_slice(mpdu);
        let lens = &mut *core::ptr::addr_of_mut!(RO_LEN);
        lens[i] = mpdu.len() as u16;
        Some(i)
    }
}

/// Delivers the frame in slot `i` and frees the slot.
fn ro_release_slot(ls: &mut LinkStats, i: usize) {
    // Copy the frame first, then deliver: `deliver` takes `&mut LinkStats`,
    // and a borrow of the pool across it would be a second mutable access
    // to the same memory.
    let mut tmp = [0u8; RO_FRAME];
    // SAFETY: as `ro_take`: one thread, one caller. The pointers are bound
    // to references first; `&(*ptr)[i]` inside an expression would be an
    // implicit borrow through a raw pointer.
    let len = unsafe {
        let lens = &*core::ptr::addr_of!(RO_LEN);
        let len = lens[i] as usize;
        let buf = &*core::ptr::addr_of!(RO_BUF);
        tmp[..len].copy_from_slice(&buf[i][..len]);
        let used = &mut *core::ptr::addr_of_mut!(RO_USED);
        used[i] = false;
        len
    };
    ls.ro_sorted += 1;
    deliver_mpdu(ls, &tmp[..len]);
}

/// Releases everything contiguous from the head.
fn ro_release_ready(ls: &mut LinkStats, tid: usize) {
    loop {
        let h = (ls.ro_head[tid] as usize) % RO_WIN;
        let slot = ls.ro_slot[tid][h];
        if slot == 0 {
            return;
        }
        ls.ro_slot[tid][h] = 0;
        ls.ro_held[tid] = ls.ro_held[tid].saturating_sub(1);
        ls.ro_head[tid] = (ls.ro_head[tid] + 1) & 0x0fff;
        ro_release_slot(ls, (slot - 1) as usize);
    }
}

/// Advances the head to `want` and releases everything below it, skipping
/// holes (`ieee80211_sta_reorder_release`).
fn ro_advance_to(ls: &mut LinkStats, tid: usize, want: u16) {
    // A jump beyond the window does not step slot by slot. The distance can
    // be up to 2047 (half the sequence space), which would be 2047 rounds for
    // at most 64 held frames; instead walk the window once and set the head
    // directly.
    let dist = want.wrapping_sub(ls.ro_head[tid]) & 0x0fff;
    if dist as usize > RO_WIN {
        for k in 0..RO_WIN {
            let h = ((ls.ro_head[tid] as usize) + k) % RO_WIN;
            let slot = ls.ro_slot[tid][h];
            if slot != 0 {
                ls.ro_slot[tid][h] = 0;
                ls.ro_held[tid] = ls.ro_held[tid].saturating_sub(1);
                ro_release_slot(ls, (slot - 1) as usize);
            }
        }
        ls.ro_head[tid] = want;
        return;
    }
    while ((want.wrapping_sub(ls.ro_head[tid])) & 0x0fff) != 0 {
        let h = (ls.ro_head[tid] as usize) % RO_WIN;
        let slot = ls.ro_slot[tid][h];
        ls.ro_slot[tid][h] = 0;
        ls.ro_head[tid] = (ls.ro_head[tid] + 1) & 0x0fff;
        if slot != 0 {
            ls.ro_held[tid] = ls.ro_held[tid].saturating_sub(1);
            ro_release_slot(ls, (slot - 1) as usize);
        }
    }
}

/// Ends a session and drops everything still held.
///
/// Called on reconnect. A block ack session belongs to the association:
/// after a switch the buffer would hold frames of the new cell against the
/// sequence numbers of the old one, and pool slots would stay taken. Held
/// frames are dropped, not delivered; they belong to a link that no longer
/// exists, and TCP fetches them again anyway.
fn ro_close(ls: &mut LinkStats, tid: usize) {
    if tid >= RO_TIDS {
        return;
    }
    for h in 0..RO_WIN {
        let slot = ls.ro_slot[tid][h];
        if slot != 0 {
            ls.ro_slot[tid][h] = 0;
            ro_release_slot(ls, (slot - 1) as usize);
        }
    }
    ls.ro_on[tid] = false;
    ls.ro_held[tid] = 0;
    ls.ro_head[tid] = 0;
    ls.ro_since[tid] = 0;
}

/// A session starts: the ADDBA names the starting sequence.
fn ro_open(ls: &mut LinkStats, tid: u8, ssn: u16) {
    let t = tid as usize;
    if t >= RO_TIDS {
        return;
    }
    ls.ro_on[t] = true;
    // Bits 4..15, not the whole field. The block ack starting sequence is a
    // sequence control field whose lower four bits are the fragment number;
    // unshifted, the head would be off by a factor of sixteen.
    ls.ro_head[t] = (ssn >> 4) & 0x0fff;
    ls.ro_held[t] = 0;
    ls.ro_slot[t] = [0; RO_WIN];
}

/// The entry point. Without a session the frame passes unchanged, as
/// before the handshake and for every non-QoS frame.
fn deliver_or_reorder(ls: &mut LinkStats, tid: usize, sn: u16, have_sn: bool,
                      mpdu: &[u8]) {
    if !have_sn || tid >= RO_TIDS || !ls.ro_on[tid] {
        deliver_mpdu(ls, mpdu);
        return;
    }
    let d = sn.wrapping_sub(ls.ro_head[tid]) & 0x0fff;
    // The sequence space is 12 bits, so "older" is the upper half. A frame
    // below the head is too late and was already skipped by a hole or a
    // timeout; delivering it now would break the order just restored.
    if d >= 0x800 {
        ls.ro_old += 1;
        return;
    }
    if d >= RO_WIN as u16 {
        // The sender is ahead of our window: advance the head until `sn`
        // just fits.
        let want = sn.wrapping_sub(RO_WIN as u16 - 1) & 0x0fff;
        ro_advance_to(ls, tid, want);
    }
    if sn == ls.ro_head[tid] && ls.ro_slot[tid][(sn as usize) % RO_WIN] == 0 {
        // The normal case: it fits exactly, nothing is held.
        ls.ro_head[tid] = (ls.ro_head[tid] + 1) & 0x0fff;
        deliver_mpdu(ls, mpdu);
        ro_release_ready(ls, tid);
        return;
    }
    let pos = (sn as usize) % RO_WIN;
    if ls.ro_slot[tid][pos] != 0 {
        // Slot already taken: a duplicate the depth-1 cache missed because
        // other frames came in between.
        ls.ro_old += 1;
        return;
    }
    match ro_take(mpdu) {
        Some(i) => {
            ls.ro_slot[tid][pos] = (i + 1) as u8;
            if ls.ro_held[tid] == 0 {
                ls.ro_since[tid] = host::now_us() as u32 / 1000;
            }
            ls.ro_held[tid] += 1;
        }
        None => {
            // A full pool delivers rather than drops. Order suffers, the data
            // does not, and the counter records it.
            ls.ro_full += 1;
            deliver_mpdu(ls, mpdu);
        }
    }
    ro_release_ready(ls, tid);
}

/// Once per round: a hole open too long is skipped. Otherwise a single lost
/// frame stalls the stream until the sender has sent 64 more.
fn ro_tick(ls: &mut LinkStats) {
    // The cheap check first. This runs in the pump loop on every iteration,
    // and the normal case "nothing held" must cost no host call; `now_us()`
    // is asked only when a hole is open.
    if ls.ro_held.iter().all(|&h| h == 0) {
        return;
    }
    let now = host::now_us() as u32 / 1000;
    for tid in 0..RO_TIDS {
        if ls.ro_held[tid] == 0 {
            continue;
        }
        if now.wrapping_sub(ls.ro_since[tid]) < RO_TIMEOUT_MS {
            continue;
        }
        ls.ro_timeout += 1;
        // Advance the head by one, skipping the hole, then release
        // everything contiguous.
        let want = (ls.ro_head[tid] + 1) & 0x0fff;
        ro_advance_to(ls, tid, want);
        ro_release_ready(ls, tid);
        ls.ro_since[tid] = now;
    }
}

/// In how many milliseconds `ro_tick` skips the next hole; `None` while
/// nothing is held.
fn ro_due_ms(ls: &LinkStats) -> Option<u64> {
    if ls.ro_held.iter().all(|&h| h == 0) {
        return None;
    }
    let now = host::now_us() as u32 / 1000;
    (0..RO_TIDS)
        .filter(|&tid| ls.ro_held[tid] != 0)
        .map(|tid| RO_TIMEOUT_MS.saturating_sub(now.wrapping_sub(ls.ro_since[tid])) as u64)
        .min()
}

/// Delivers an MPDU: convert it, or unpack an A-MSDU.
///
/// The position in the chain is mac80211's: after reordering
/// (`ieee80211_rx_h_amsdu`, rx.c:3114, follows
/// `ieee80211_rx_reorder_ampdu`). Whether the MPDU carries an A-MSDU is
/// decided by bit 7 of the QoS control field
/// (`IEEE80211_QOS_CTL_A_MSDU_PRESENT`, rx.c:914-915) and nothing else.
fn deliver_mpdu(ls: &mut LinkStats, f: &[u8]) {
    let mut out = [0u8; 2048];
    let qos = f.len() >= 26 && f[0] & (DOT11_STYPE_QOS << 4) != 0;
    let protected = f.len() >= 2 && f[1] & DOT11_FC_PROTECTED != 0;
    // After reordering, as in mac80211, so packet numbers arrive in order.
    if protected && !ccmp_pn_ok(ls, f) {
        ls.rx_pn_replay += 1;
        return;
    }
    if qos && f[24] & 0x80 != 0 {
        // `ieee80211_drop_unencrypted`: once keyed, data must be protected.
        if !protected && ls.authorized {
            ls.rx_plain_dropped += 1;
            return;
        }
        amsdu_to_8023s(ls, f, &mut out);
        return;
    }
    if let Some((n, is_eapol)) = rx_to_8023(f, &mut out, &mut ls.llc_miss) {
        // The one unprotected frame a keyed link takes is EAPOL: the AP
        // may send a rekey before the station has the new key in use
        // (rx.c `ieee80211_802_1x_port_control`).
        if !protected && ls.authorized && !is_eapol {
            ls.rx_plain_dropped += 1;
            return;
        }
        deliver(ls, &out[..n], is_eapol);
    }
}

/// The CCMP packet number check of `ieee80211_crypto_ccmp_decrypt`
/// (wpa.c): the 48-bit PN from the CCMP header must be above the last one
/// accepted for this key and TID. A frame repeated by a third party
/// decrypts fine and is caught only here. Updates the counter on success.
fn ccmp_pn_ok(ls: &mut LinkStats, f: &[u8]) -> bool {
    let h = data_hdrlen(f);
    if f.len() < h + 8 || f.len() < 24 {
        return false;
    }
    // PN0 PN1 rsvd keyid PN2 PN3 PN4 PN5 (802.11-2020 §12.5.3.2).
    let c = &f[h..h + 8];
    let pn = c[0] as u64 | (c[1] as u64) << 8 | (c[4] as u64) << 16
        | (c[5] as u64) << 24 | (c[6] as u64) << 32 | (c[7] as u64) << 40;
    let qos = f[0] & (DOT11_STYPE_QOS << 4) != 0;
    let tid = if qos && f.len() >= 26 { (f[24] & 0x0f) as usize } else { 16 };
    let group = f[4] & 0x01 != 0; // addr1 is a group address
    let last = &mut ls.rx_pn[group as usize][tid];
    if pn <= *last {
        return false;
    }
    *last = pn;
    true
}

/// cfg80211 `ieee80211_amsdu_to_8023s` (util.c:842-937) for a station in a
/// non-mesh cell: `iftype` STATION, `mesh_control` 0, and from
/// `__ieee80211_rx_h_amsdu` (rx.c:3028-3058) `check_da` = our address
/// (addr1), `check_sa` = none.
///
/// The body is a sequence of subframes:
///
///     DA(6) SA(6) length(2, big-endian) | MSDU | padding to 4 bytes
///
/// The last one has no padding. If one subframe does not fit the rest, the
/// whole MPDU is dropped (`goto purge`), including subframes that fit, so
/// we validate first and deliver after.
///
/// A-MSDU injection defense (util.c:824-825, CVE-2020-24588): the first
/// subframe's destination address must not be the LLC/SNAP header,
/// otherwise a plain MSDU was reinterpreted as an A-MSDU.
fn amsdu_to_8023s(ls: &mut LinkStats, f: &[u8], out: &mut [u8; 2048]) {
    ls.amsdu_rx += 1;
    // Locate the body: the same header/CCMP/trailer arithmetic as for a
    // single MSDU, without the LLC/SNAP search (an A-MSDU body starts with
    // the first subframe header).
    let hdrlen = data_hdrlen(f);
    let prot = f[1] & DOT11_FC_PROTECTED != 0;
    let start = hdrlen + if prot { 8 } else { 0 };
    let trailing = 4 + if prot { 8 } else { 0 };
    if f.len() < start + trailing {
        ls.amsdu_bad += 1;
        return;
    }
    let body = &f[start..f.len() - trailing];
    let da_ok: [u8; 6] = [f[4], f[5], f[6], f[7], f[8], f[9]];

    // Two passes instead of a list: the first checks all subframes (one bad
    // means all dropped), the second delivers. A fixed list would impose a
    // limit cfg80211 does not have.
    if amsdu_walk(body, &da_ok, &mut |_, _| {}).is_none() {
        ls.amsdu_bad += 1;
        return;
    }
    amsdu_walk(body, &da_ok, &mut |o, len| {
        let hdr = &body[o..o + 14];
        let msdu = &body[o + 14..o + 14 + len];
        // `ieee80211_get_8023_tunnel_proto` (util.c:512-525): with RFC 1042
        // (except AARP and IPX) or bridge tunnel in front, it is removed with
        // its type field and the type goes into the 802.3 header.
        let (proto, payload) = if msdu.len() >= 8 {
            let p = [msdu[6], msdu[7]];
            let rfc1042 = msdu[0..6] == LLC_SNAP_HDR
                && p != [0x80, 0xf3] && p != [0x81, 0x37];
            let bridge = msdu[0..6] == [0xaa, 0xaa, 0x03, 0x00, 0x00, 0xf8];
            if rfc1042 || bridge { (p, &msdu[8..]) } else { ([hdr[12], hdr[13]], msdu) }
        } else {
            ([hdr[12], hdr[13]], msdu)
        };
        if 14 + payload.len() > out.len() {
            return;
        }
        out[0..12].copy_from_slice(&hdr[0..12]);
        out[12..14].copy_from_slice(&proto);
        out[14..14 + payload.len()].copy_from_slice(payload);
        ls.amsdu_sub += 1;
        let is_eapol = u16::from_be_bytes(proto) == ETHERTYPE_EAPOL;
        deliver(ls, &out[..14 + payload.len()], is_eapol);
    });
}

/// The loop of `ieee80211_amsdu_to_8023s` without delivery: walks the
/// subframes, calls `each(start, length)` for each one addressed to us (or
/// broadcast), and returns `None` where cfg80211 does `goto purge`.
fn amsdu_walk(body: &[u8], da_ok: &[u8; 6],
              each: &mut dyn FnMut(usize, usize)) -> Option<()> {
    let mut offset = 0usize;
    let mut last = false;
    while !last {
        let remaining = body.len() - offset;
        if 14 > remaining {
            return None;
        }
        let len = u16::from_be_bytes([body[offset + 12], body[offset + 13]]) as usize;
        let subframe_len = 14 + len;
        let padding = (4 - (subframe_len & 3)) & 3;
        // "the last MSDU has no padding"
        if subframe_len > remaining {
            return None;
        }
        // "mitigate A-MSDU aggregation injection attacks"
        if offset == 0 && body[0..6] == LLC_SNAP_HDR {
            return None;
        }
        last = remaining <= subframe_len + padding;
        let da = &body[offset..offset + 6];
        if da[0] & 0x01 != 0 || da == &da_ok[..] {
            each(offset, len);
        }
        offset += subframe_len + padding;
    }
    Some(())
}

/// The single exit for a received data frame. An in-order frame passes
/// through here at once; one that fills a hole is stored and sent through
/// the same door later, so both paths share one semantics.
fn deliver(ls: &mut LinkStats, eth: &[u8], is_eapol: bool) {
    if eth.len() < 14 {
        return;
    }
    if is_eapol {
        ls.eapol_rx += 1;
        if ls.authorized {
            ls.rekey_rx += 1;
        }
        // `EV_EAPOL_RX` = [0x84][len u16 LE][frame], where the frame is the
        // EAPOL body after the ethertype.
        //
        // Trim to the declared length. The EAPOL header carries it in bytes
        // 2..4 (802.1X, big-endian), and the whole frame is 4 + that number.
        // `WLAN_RCR_CFG` sets APP_FCS, APP_MIC and APP_ICV, so the descriptor
        // delivers more bytes than the frame has. `wifid` computes the MIC
        // over the whole slice it gets (`compute_mic`: `frame.len()`), so
        // extra bytes would make msg3 fail.
        let raw = &eth[14..];
        let body = if raw.len() >= 4 {
            let declared =
                4 + u16::from_be_bytes([raw[2], raw[3]]) as usize;
            if ls.extra_reported < 2 && declared <= raw.len() {
                ls.extra_reported += 1;
                host::print("    EAPOL: ");
                host::print_dec(raw.len() as u32);
                host::print(" Bytes geliefert, ");
                host::print_dec(declared as u32);
                host::print(" angesagt (");
                host::print_dec((raw.len() - declared) as u32);
                host::print(" zu viel)\n");
            }
            &raw[..declared.min(raw.len())]
        } else {
            raw
        };
        let mut ev = [0u8; 600];
        if body.len() + 3 <= ev.len() {
            ev[0] = EV_EAPOL_RX;
            ev[1] = (body.len() & 0xff) as u8;
            ev[2] = (body.len() >> 8) as u8;
            ev[3..3 + body.len()].copy_from_slice(body);
            host::wifi_send_event(&ev[..3 + body.len()]);
        }
    } else if ls.authorized {
        host::netdev_submit_rx(eth);
    }
}

/// The driver loop: drain the RX ring, collect TX reports, execute `wifid`
/// commands, report events upward.
///
/// `wifid` computes the handshake (`tools/wasm/wifid/core/src/eapol.rs`);
/// the driver carries the frames and writes the finished keys. Per
/// `docs/spec/WIFI_CLASS_ABI.md` §1 the driver never sees the PSK.
///
/// `frist_us == 0` means run forever. Link and counters come from the
/// caller so stage 6b continues where 6a stopped.
fn link_pump(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
             link: &mut Link, ls: &mut LinkStats, mac: [u8; 6],
             frist_us: u64, d: &mut Dev, h2c: &mut fw::H2cState,
             e: &efuse::Efuse, t_pwr: &txpower::TxPower,
             caps: &sta::PeerCaps, fw_feature: u32)
    -> PumpEnd {
    chip::read_cck_gi_bnd(h, &mut d.dm);
    static mut RXBUF7: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    static mut ETHBUF: [u8; 2048] = [0; 2048];
    static mut CMDBUF: [u8; 2048] = [0; 2048];
    // SAFETY: single-threaded, one caller each, none leaves this function.
    let (rxbuf, ethbuf, cmdbuf) = unsafe {
        (&mut *core::ptr::addr_of_mut!(RXBUF7),
         &mut *core::ptr::addr_of_mut!(ETHBUF),
         &mut *core::ptr::addr_of_mut!(CMDBUF))
    };


    // Stage 6a passes eight seconds: the handshake takes four frames and
    // completes within milliseconds, so waiting longer means waiting for an
    // error. Stage 6b calls the same loop without a deadline.

    // The callback needs the BSSID to tell a deauth of our own cell from a
    // neighbor's. A copy, so it does not hold `link` while the loop writes
    // to it.
    let bssid = link.bssid;

    let t0 = host::now_us();
    let mut report_ms = host::now_ms();
    let mut rx_silent_ms = host::now_ms();
    let mut watch_dog_ms = host::now_ms();
    // One data frame per watchdog tick requests a TX report.
    let mut probe_due = true;
    let mut leer_in_folge = 0u32;
    if ls.pump_us0 == 0 {
        ls.pump_us0 = host::now_us();
    }
    // The receive aggregation window from `sys/config/wifi`: `ampdu: off`
    // disables it, `ampdu: 16` sets another window.
    let ampdu_buf = read_ampdu_buf();
    // `txagg:` limits only the transmit direction. `ampdu:` is the receive
    // side; the two directions are separate sessions and do not share a
    // switch. Default on.
    let txagg = read_txagg();
    let roam_mode = read_roam_mode();
    // `bss_conf.beacon_int`: 100 TU is what practically every AP announces.
    // It is not read from the beacon yet, and zero would be worse than the
    // normal case (it divides).
    let beacon_int: u16 = 100;
    // The width the PHY is on goes into the RX status: zero would mean
    // 20 MHz, and every frame with `rxsc == 0` (the full width, the normal
    // case) would be reported as 20 MHz.
    let mut cur_bw = d.cur_bw as u8;
    while frist_us == 0 || host::now_us() - t0 < frist_us {
        let now = host::now_ms();
        let t_iter = host::now_us();
        let mut wd_ran = false;

        // ── Receive ──────────────────────────────────────────────
        let mut acc = rx::WdAcc::new(link.si.avg_rssi);
        // Time spent in the ring. Frames per poll near one can mean fast
        // enough or exactly as slow as arrival; only the wall time in the
        // receive path tells them apart. Measured only when something came,
        // so the measurement does not dominate the empty polls.
        // For the callback, which must not touch `d`.
        let messen = d.stats.rx_throughput >= 10;
        let t_rx0 = host::now_us();
        let got = pci::rx_poll(h, trx, 64, rxbuf, &mut d.dm, &mut d.path_div,
                               hal.rf_path_num, cur_bw, link.channel,
                               |st, pkt| {
            if st.crc_err {
                return;
            }
            // The width the frame came in, only when the descriptor answers
            // it. The width field (`GET_RX_DESC_BW`) is filled only on
            // frames carrying a PHY status, one of many in an A-MPDU; empty
            // fields would read as 20 MHz.
            //
            // It precedes the early exit below, so the count covers all
            // frames, not only those processed further.
            if st.phy_status {
                acc.bw_cnt[(st.bw as usize).min(3)] += 1;
            }
            let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                + st.shift as usize;
            if off >= pkt.len() {
                return;
            }
            // C2H: the firmware rate report feeds `config_swing_table` and
            // `rrsr_update`; without it the watchdog computes on the initial
            // rate.
            if st.is_c2h {
                if let Some(c) = fw::c2h_parse(&pkt[off..]) {
                    if c.id as u32 == C2H_RA_RPT
                        && c.payload.len() >= C2H_RA_REPORT_SIZE
                    {
                        acc.ra_rpt = Some((c.payload[0]
                                           & RTW_C2H_RA_RPT_RATE as u8,
                                           c.payload[1]));
                    } else if c.id as u32 == C2H_CCX_TX_RPT
                        || (c.id as u32 == C2H_HALMAC
                            && c.payload.first().copied()
                               == Some(C2H_CCX_RPT as u8))
                    {
                        // Both paths: `C2H_CCX_TX_RPT` carries the report
                        // directly (V0), `C2H_HALMAC` as subcommand 0x0f
                        // (V1), fw.c:93-113.
                        let v1 = c.id as u32 == C2H_HALMAC;
                        if let Some(r) = fw::tx_report_parse(c.payload, v1) {
                            if acc.n_tx_rpt < acc.tx_rpt.len() {
                                acc.tx_rpt[acc.n_tx_rpt] = r;
                                acc.n_tx_rpt += 1;
                            }
                        }
                    } else if acc.n_c2h_seen < acc.c2h_seen.len() {
                        // Count whatever else arrives, so a missing report
                        // shows up with the IDs that came instead.
                        acc.c2h_seen[acc.n_c2h_seen] = c.id;
                        acc.n_c2h_seen += 1;
                    }
                }
                return;
            }
            let f = &pkt[off..];
            // rx.c `ieee80211_rx_h_decrypt`: a protected data frame the
            // hardware did not decrypt, or decrypted with a bad ICV, is not
            // the AP's. rtw88 reports both in the RX descriptor.
            if f.len() >= 2 && f[0] & 0x0c == DOT11_FC_TYPE_DATA
                && f[1] & DOT11_FC_PROTECTED != 0 && (!st.decrypted || st.icv_err)
            {
                ls.rx_undecrypted += 1;
                return;
            }
            // mlme.c:131-145: every frame from the AP resets the watchdog,
            // not only a beacon. `addr2` is the sender, the BSSID for
            // anything from it.
            if f.len() >= 16 && f[10..16] == bssid {
                acc.heard_ap = true;
                // A beacon of this cell can carry a channel switch
                // announcement. Parsed here, executed outside: the switch
                // needs `trx`, which the callback must not hold.
                if f[0] == 0x80 {
                    match parse_csa(f) {
                        Some(c) => acc.csa = Some(c),
                        // mlme.c:2820-2824: `else if (res)
                        // ieee80211_sta_abort_chanswitch(link)`; `res` is 1
                        // when this beacon has no CSA element. An
                        // announcement that disappears is withdrawn.
                        None => acc.beacon_ohne_csa = true,
                    }
                    // mlme.c:6789-6798: the level of every beacon of this
                    // cell goes into the smoothed value.
                    acc.beacon_dbm = Some(st.signal_power);
                }
            }
            // How many frames the AP bundles per transmission. `ppdu_cnt`
            // is two bits in the RX descriptor counting PPDUs (rtw88 reads
            // it and never uses it); a change means a new transmission.
            // `frames / PPDUs` is the real aggregate size in the receive
            // direction, counted by hardware. It is the counterpart to the
            // TX ring numbers and tells whether low efficiency is the link
            // or missing aggregation.
            //
            // Data frames only: a beacon is always its own transmission and
            // would lower the average.
            if f[0] & 0x0c == DOT11_FC_TYPE_DATA {
                if st.ppdu_cnt != ls.last_ppdu {
                    ls.last_ppdu = st.ppdu_cnt;
                    ls.rx_ppdu_n += 1;
                    // Interval between two transmissions, stamped by the
                    // hardware: `tsf_low` is the 802.11 clock in
                    // microseconds, set on reception. It shows how much of
                    // the time the AP transmits at all: an interval of 3 ms
                    // for a 1 ms aggregate means the air is a third busy,
                    // and then the link rate is not the limit.
                    //
                    // The smallest interval is the yardstick: what the link
                    // can do when nothing interferes.
                    let dt = st.tsf_low.wrapping_sub(ls.last_tsf);
                    let d = if messen { dt } else { 0 };
                    // Long intervals are bucketed, not discarded: dropping
                    // them would measure the link only while it runs.
                    if ls.last_tsf != 0 && d > 0 {
                        if d >= GAP_IDLE_US {
                            ls.rx_gap_idle += 1;
                        } else {
                            let i = bucket(d, &GAP_BUCKETS);
                            ls.rx_gap_buckets[i] += 1;
                            if i == 4 {
                                ls.rx_gap_big_sum += d as u64;
                            }
                            if d < 10_000 {
                                ls.rx_gap_sum += d as u64;
                                ls.rx_gap_n += 1;
                                if ls.rx_gap_min == 0 || d < ls.rx_gap_min {
                                    ls.rx_gap_min = d;
                                }
                            }
                        }
                    }
                    ls.last_tsf = st.tsf_low;
                }
                ls.rx_data_ppdu_frames += 1;
                // The moment the kernel gets data, the start of the
                // turnaround measurement below.
                ls.last_rx_at = host::now_us();
            }
            // rx.c:100-133 + phy.c:678-704: what the watchdog needs.
            rx::watchdog_feed(&mut acc, st, f, &mac, &bssid,
                              hal.rf_path_num);
            // Disconnects first: a deauth is a management frame and would
            // not pass `rx_to_8023`. Only observe here, act after draining.
            // The management census runs before and includes it, a deauth
            // is one too.
            if let Some((sub, act)) = rx::mgmt_census(f, &bssid) {
                if acc.n_mgmt < acc.mgmt.len() {
                    acc.mgmt[acc.n_mgmt] = (sub, act.unwrap_or((0xff, 0xff)));
                    acc.n_mgmt += 1;
                }
                // The ADDBA request is only recorded here and answered after
                // draining: a transmission does not belong in a callback that
                // must not hold `trx`.
                if act == Some((DOT11_ACTION_CAT_BA, DOT11_ACTION_ADDBA_REQ))
                {
                    if let Some(r) = sta::parse_addba_req(f) {
                        if acc.n_addba < acc.addba.len() {
                            acc.addba[acc.n_addba] = r;
                            acc.n_addba += 1;
                        } else {
                            acc.addba_drop += 1;
                        }
                    }
                }
                if act == Some((DOT11_ACTION_CAT_BA, DOT11_ACTION_ADDBA_RESP))
                    && acc.addba_resp.is_none()
                {
                    acc.addba_resp = sta::parse_addba_resp(f);
                }
            }
            if let Some(r) = disconnect_reason(f, &bssid) {
                if ls.gone.is_none() {
                    ls.gone = Some(r);
                }
                return;
            }
            // Drop duplicate 802.11 retransmissions:
            // `ieee80211_rx_h_check_dup` (rx.c:1438-1490), 802.11-2012
            // §9.3.2.10 "Duplicate detection and recovery".
            //
            // The AP retransmits at MAC level when our ACK is missing or
            // late; the retransmission carries the same sequence control
            // field and the retry bit. Not dropping it delivers the same
            // bytes to TCP twice.
            //
            // The whole field is compared, not only the sequence number: the
            // lower four bits are the fragment number, and two fragments of
            // the same frame are not duplicates.
            let mut ro_tid = RO_TIDS;
            let mut ro_sn = 0u16;
            let mut ro_have = false;
            if f.len() >= 24 {
                let fc = u16::from_le_bytes([f[0], f[1]]);
                if fc & 0x000c == 0x0008 {
                    let is_qos = fc & 0x0080 != 0;
                    let idx = if is_qos && f.len() >= 26 {
                        (f[24] & 0x0f) as usize
                    } else {
                        16
                    };
                    let sc = u16::from_le_bytes([f[22], f[23]]) as u32;
                    if fc & 0x0800 != 0 {
                        ls.retry_rx += 1;
                    }
                    if fc & 0x0800 != 0 && ls.last_seq_ctrl[idx] == sc {
                        ls.dup_rx += 1;
                        return;
                    }
                    ls.last_seq_ctrl[idx] = sc;
                    // Only whole frames are reordered. The lower four bits
                    // are the fragment number; a fragment belongs to
                    // defragmentation, which mac80211 runs separately and
                    // before the buffer (`ieee80211_rx_h_defragment`).
                    if is_qos && idx < RO_TIDS && sc & 0x000f == 0 {
                        ro_tid = idx;
                        ro_sn = (sc >> 4) as u16;
                        ro_have = true;
                    }
                }
            }
            // Only data frames with a body continue, the same first check as
            // in `rx_to_8023`. Conversion happens on delivery
            // (`deliver_mpdu`), after reordering.
            if f.len() < 24 || f[0] & 0x0c != DOT11_FC_TYPE_DATA
                || f[0] & DOT11_STYPE_NODATA != 0
            {
                return;
            }
            ls.data_rx += 1;
            // rx.c:14-32 `rtw_rx_stats`: only unicast counts, and the length
            // of the 802.11 frame is counted.
            if f[4] & 0x01 == 0 {
                acc.rx_unicast += f.len() as u64;
                acc.rx_cnt += 1;
            }
            deliver_or_reorder(ls, ro_tid, ro_sn, ro_have, f);
        });

        // A hole open too long would stall the whole stream. Once per round
        // is enough; the timeout is 100 ms.
        // Below, the loop shape, measured without an extra host call:
        // frames per poll near one means we look faster than frames arrive
        // (the air is the limit); full batches mean we cannot keep up.
        ro_tick(ls);
        if got > 0 {
            ls.rx_polls += 1;
            ls.rx_frames += got;
            ls.rx_us = ls.rx_us.wrapping_add(host::now_us() - t_rx0);
            if got >= 64 {
                ls.rx_full += 1;
            }
        } else {
            ls.rx_empty += 1;
        }

        // mlme.c:4525-4528 `if (!ifmgd->probe_send_count)
        // ieee80211_reset_ap_probe(sdata)`: the AP answered. Every frame from
        // it counts, not only a reply to our probe: whoever sends data is
        // alive.
        if let Some(dbm) = acc.beacon_dbm.take() {
            link.roam.note_beacon(dbm);
        }
        // Measure only while data flows. A 19 ms gap between two frames means
        // "the link stalled" during a download and "nothing to send" when
        // idle; a sum over both means neither. `rx_throughput` is in Mbit and
        // updated per watchdog tick (two seconds); ten separates background
        // traffic from a transfer.
        let misst = d.stats.rx_throughput >= 10;
        if acc.heard_ap && ls.poll_on {
            ls.poll_on = false;
            ls.probe_send_count = 0;
            ls.poll_recovered += 1;
            host::say("[rtl8822ce] der AP ist wieder da — Verbindung steht\n");
        }

        // Merge what the ring pass collected for the watchdog.
        acc.merge(&mut d.dm, &mut link.si);
        for i in 0..4 {
            ls.bw_hist[i] = ls.bw_hist[i].saturating_add(acc.bw_cnt[i]);
        }
        d.stats.rx_unicast += acc.rx_unicast;
        d.stats.rx_cnt += acc.rx_cnt;
        for i in 0..acc.n_tx_rpt {
            let (sn, acked) = acc.tx_rpt[i];
            ls.settle_probe(sn, acked);
        }
        for i in 0..acc.n_c2h_seen {
            ls.note_c2h(acc.c2h_seen[i]);
        }
        for i in 0..acc.n_mgmt {
            let (sub, (cat, a)) = acc.mgmt[i];
            ls.note_mgmt(sub, cat, a);
        }

        // ── Request aggregation ──────────────────────────────────
        //
        // The branch below answers the AP's request, this one makes ours:
        // the same thing in two directions.
        //
        // Ask only after the four-way handshake: before it there is no key,
        // no data flows, so nothing to aggregate. It is also the moment the
        // sequence number is still idle, and the starting sequence in the
        // request must be the one we send from.
        if let Some(r) = acc.addba_resp.take() {
            if link.ba_tx.state == BaState::Gefragt
                && r.dialog_token == link.ba_tx.token
            {
                link.ba_tx.status = r.status;
                if r.status == 0 && r.tid == link.ba_tx.tid {
                    link.ba_tx.state = BaState::Laeuft;
                    // agg-tx.c:992: the AP may grant less than requested, and
                    // HT cannot do more than 64.
                    link.ba_tx.win = r.buf_size.min(sta::BA_TX_BUF_SIZE);
                    host::loud_begin();
                    host::print("[rtl8822ce] Sende-Aggregation LAEUFT: TID ");
                    host::print_dec(r.tid as u32);
                    host::print(", der AP gibt ");
                    host::print_dec(link.ba_tx.win as u32);
                    host::print(" Rahmen (erbeten ");
                    host::print_dec(sta::BA_TX_BUF_SIZE as u32);
                    host::print("), MAX_AGG_NUM ");
                    host::print_dec(link.ba_tx.factor as u32);
                    host::print(", Abstand ");
                    host::print_dec(link.ba_tx.density as u32);
                    host::print("\n");
                    host::loud_end();
                } else {
                    link.ba_tx.state = BaState::Aufgegeben;
                    host::loud_begin();
                    host::print("[rtl8822ce] Sende-Aggregation ABGELEHNT: Status ");
                    host::print_dec(r.status as u32);
                    host::print(", TID ");
                    host::print_dec(r.tid as u32);
                    host::print(" — die Verbindung laeuft weiter, jeder Rahmen einzeln\n");
                    host::loud_end();
                }
            }
        }
        let ba_faellig = match link.ba_tx.state {
            BaState::Aus => txagg && ls.authorized,
            BaState::Gefragt => now.saturating_sub(link.ba_tx.at_ms)
                                > BA_RESP_MS,
            _ => false,
        };
        if ba_faellig {
            if link.ba_tx.tries >= BA_MAX_TRIES {
                link.ba_tx.state = BaState::Aufgegeben;
                host::say("[rtl8822ce] Sende-Aggregation: der AP hat auf drei ADDBA Requests\n\x20         nicht geantwortet. Jeder Rahmen geht einzeln hinaus.\n");
            } else {
                // The two descriptor values come from the AP's HT
                // capabilities and are fixed once it agrees; computed here
                // because `caps` is at hand.
                link.ba_tx.factor = sta::tx_ampdu_factor(caps.ht_ampdu_factor);
                link.ba_tx.density = sta::tx_ampdu_density(caps.ht_ampdu_density);
                link.ba_tx.token = link.ba_tx.token.wrapping_add(1);
                link.ba_tx.tries += 1;
                link.ba_tx.at_ms = now;
                link.ba_tx.state = BaState::Gefragt;
                let mut req = [0u8; 256];
                let n = sta::build_addba_req(&mut req, &mac, &bssid,
                                             link.ba_tx.tid,
                                             link.ba_tx.token, link.seq,
                                             sta::BA_TX_BUF_SIZE, 0);
                let mut info = tx::pkt_info_update(&req[..n], 0,
                                                  tx::band_of(link.channel));
                let q = tx::RTW_TX_QUEUE_MGMT;
                if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &req[..n]) {
                    pci::tx_kick_off_queue(h, trx, q);
                    ls.addba_tx += 1;
                }
                if link.ba_tx.tries == 1 {
                    host::loud_begin();
                    host::print("[rtl8822ce] ADDBA Request hinaus: TID ");
                    host::print_dec(link.ba_tx.tid as u32);
                    host::print(", ab Folgenummer ");
                    host::print_dec(link.seq as u32);
                    host::print(", Fenster ");
                    host::print_dec(sta::BA_TX_BUF_SIZE as u32);
                    host::print("\n");
                    host::loud_end();
                }
            }
        }
        // Send QoS only once the session is up. A QoS frame without a
        // session would work, but the frame type would change mid-link and
        // the sequence number would be used before the request names its
        // starting sequence.
        let qos_tid = if link.ba_tx.state == BaState::Laeuft {
            Some(link.ba_tx.tid)
        } else {
            None
        };

        // ── Allow aggregation ────────────────────────────────────
        // The AP asks with an ADDBA request and repeats it until answered;
        // it cannot aggregate until then.
        ls.addba_drop += acc.addba_drop;
        for ai in 0..acc.n_addba {
            let req = acc.addba[ai];
            if ampdu_buf > 0 {
                let mut resp = [0u8; 256];
                let n = sta::build_addba_resp(&mut resp, &mac, &bssid, &req,
                                              ampdu_buf);
                let mut info = tx::pkt_info_update(&resp[..n], 0,
                                                   tx::band_of(link.channel));
                let q = tx::RTW_TX_QUEUE_MGMT;
                if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &resp[..n]) {
                    pci::tx_kick_off_queue(h, trx, q);
                    ls.addba_resp += 1;
                    ls.addba_win = ampdu_buf;
                    ls.addba_win_req = req.buf_size;
                    // Only now, with the starting sequence from its request:
                    // from this frame on the AP aggregates and reordering is
                    // needed. Earlier would leave a head without a session,
                    // later a hole at the start.
                    ro_open(ls, req.tid, req.ssn);
                    if ls.addba_resp <= 3 {
                        host::loud_begin();
                        host::print("[rtl8822ce] ADDBA angenommen: TID ");
                        host::print_dec(req.tid as u32);
                        host::print(", der AP wollte ");
                        host::print_dec(req.buf_size as u32);
                        host::print(" offene Rahmen, wir geben ");
                        host::print_dec(ampdu_buf as u32);
                        host::print("\n            (Umsortierpuffer ");
                        host::print_dec(RO_WIN as u32);
                        host::print(" Plaetze, Startsequenz ");
                        host::print_dec((req.ssn >> 4) as u32);
                        host::print(")\n");
                        host::loud_end();
                    }
                } else {
                    ls.addba_fail += 1;
                }
            }
        }
        if let Some((rate, mac_id)) = acc.ra_rpt {
            ls.ra_rpt_n += 1;
            // fw.c:308: `dm_info->tx_rate` regardless of station,
            // `si->ra_report.desc_rate` only for a matching mac_id.
            d.dm.tx_rate = rate;
            if link.si.mac_id == mac_id {
                link.si.ra_report_desc_rate = rate;
            }
        }

        // ── Disconnect, handled ──────────────────────────────────
        // The callback above saw it; here the ring is drained and `link` is
        // free again. The kernel learns first, since until now it believed
        // `carrier UP` and pushed packets into a dead link.
        if let Some((deauth, reason)) = ls.gone.take() {
            ls.kicked += 1;
            ls.last_reason = reason;

            // Every one is counted, the first three printed. An AP tends to
            // send its deauth as a burst, and the counter in the report stays
            // complete.
            if ls.kicked <= 3 {
                host::loud_begin();
                host::print("[rtl8822ce] ");
                host::print(if deauth { "DEAUTH" } else { "DISASSOC" });
                host::print(" vom AP — Grund ");
                host::print_dec(reason as u32);
                host::print(" (");
                host::print(reason_name(reason));
                host::print(")\n            Laufzeit ");
                host::print_dec(((host::now_us() - t0) / 1_000_000) as u32);
                host::print(" s · daten rein/raus ");
                host::print_dec(ls.data_rx);
                host::print("/");
                host::print_dec(ls.data_tx);
                host::print(" · neuschluessel ");
                host::print_dec(ls.rekey_rx);
                host::print("/");
                host::print_dec(ls.rekey_tx);
                host::print(" · gtk ");
                host::print_dec(ls.gtk_set);
                host::print("\n");
                host::loud_end();
            }

            if ls.authorized || ls.link_up_sent {
                host::netdev_set_link(false);
                let down = [EV_LINK_DOWN, LINK_DOWN_DEAUTH];
                host::wifi_send_event(&down);
            }
            ls.authorized = false;
            ls.link_up_sent = false;

            // Return to the caller. Stage 6a continues (a deauth there is a
            // finding for the gate, not a task); in 6b the caller rebuilds
            // the link.
            if frist_us == 0 {
                return PumpEnd::LinkLost;
            }
        }

        // ── Channel switch (CSA) ─────────────────────────────────
        //
        // mlme.c:2980-3010. Every beacon with an announcement recomputes the
        // deadline, `(max(count, 1) - 1) * beacon_int`, so a missed beacon
        // does not shift it. `count` counts down in the beacon; 0 and 1 both
        // mean "now". Withdraw when the announcement disappears
        // (mlme.c:2822), otherwise we would move to a channel the AP never
        // went to.
        if acc.beacon_ohne_csa && acc.csa.is_none() && link.csa.is_some() {
            link.csa = None;
            host::say("[rtl8822ce] Kanalwechsel ZURUECKGENOMMEN — der AP kuendigt ihn nicht mehr an\n");
        }
        if let Some(c) = acc.csa.take() {
            let neu = link.csa.map_or(true, |a| a.channel != c.channel);
            let tu = (c.count.max(1) as u64 - 1) * beacon_int as u64;
            // One TU is 1024 us; we compute in milliseconds.
            link.csa_at_ms = now + (tu * 1024) / 1000;
            link.csa = Some(c);
            if neu {
                host::loud_begin();
                host::print("[rtl8822ce] der AP zieht um: K");
                host::print_dec(link.channel as u32);
                host::print(" -> K");
                host::print_dec(c.channel as u32);
                host::print(", in ");
                host::print_dec(c.count as u32);
                host::print(" Baken (");
                host::print(if c.mode != 0 {
                    "ab jetzt Sendepause"
                } else {
                    "senden erlaubt"
                });
                host::print(")\n");
                host::loud_end();
            }
        }
        if let Some(c) = link.csa {
            if now >= link.csa_at_ms {
                link.csa = None;
                let max_bw = max_bw_for(e);
                let (cch, bw, _) = chan_params(c.channel, c.width, max_bw);
                let ok = switch_channel(h, hal, e, t_pwr, c.channel,
                                        c.width, max_bw);
                host::loud_begin();
                host::print("[rtl8822ce] Kanalwechsel vollzogen: K");
                host::print_dec(c.channel as u32);
                host::print(" · ");
                host::print(match bw {
                    2 => "80 MHz (Mitte K",
                    1 => "40 MHz (Mitte K",
                    _ => "20 MHz (K",
                });
                host::print_dec(cch as u32);
                host::print(")");
                if !ok {
                    host::print("  — RF 0x18 traegt ihn NICHT");
                }
                host::print("\n");
                host::loud_end();
                // The way back if nobody is there. mac80211 waits for a
                // beacon after the switch (`csa.waiting_bcn`, mlme.c:2812).
                // We remember the old channel and return if no beacon of the
                // cell arrives within a second, rather than sitting on an
                // empty channel until the watchdog fires.
                link.csa_zurueck = Some((link.channel, CellWidth {
                    ht_param: link.ht_param_now,
                    vht_chanwidth: link.vht_chanwidth_now,
                    vht_cch0: link.vht_cch0_now,
                }));
                link.csa_frist_ms = now + CSA_BEACON_WAIT_MS;
                link.channel = c.channel;
                link.ht_param_now = c.width.ht_param;
                link.vht_chanwidth_now = c.width.vht_chanwidth;
                link.vht_cch0_now = c.width.vht_cch0;
                d.cur_bw = bw;
                cur_bw = bw as u8;
                ls.csa_done += 1;
                // The link watchdog starts over: no beacon seen on the new
                // channel yet.
                ls.poll_on = false;
                ls.probe_send_count = 0;
            }
        }

        // Did a beacon arrive on the new channel? If not, go back.
        if let Some((alt_ch, alt_w)) = link.csa_zurueck {
            if acc.heard_ap {
                link.csa_zurueck = None;
            } else if now >= link.csa_frist_ms {
                link.csa_zurueck = None;
                host::loud_begin();
                host::print("[rtl8822ce] auf K");
                host::print_dec(link.channel as u32);
                host::print(" ist niemand — zurueck auf K");
                host::print_dec(alt_ch as u32);
                host::print("\n");
                host::loud_end();
                let max_bw = max_bw_for(e);
                let (_, bw, _) = chan_params(alt_ch, alt_w, max_bw);
                let _ = switch_channel(h, hal, e, t_pwr, alt_ch, alt_w,
                                       max_bw);
                link.channel = alt_ch;
                link.ht_param_now = alt_w.ht_param;
                link.vht_chanwidth_now = alt_w.vht_chanwidth;
                link.vht_cch0_now = alt_w.vht_cch0;
                d.cur_bw = bw;
                cur_bw = bw as u8;
                ls.csa_back += 1;
            }
        }

        // ── Link watchdog ────────────────────────────────────────
        //
        // mlme.c:8516-8560, branch `IEEE80211_STA_CONNECTION_POLL`: when the
        // deadline expires and attempts remain, probe again; otherwise the
        // link is lost.
        if ls.poll_on && now >= ls.probe_timeout_ms {
            if ls.probe_send_count >= MAX_PROBE_TRIES {
                host::loud_begin();
                host::print("[rtl8822ce] keine Antwort vom AP nach ");
                host::print_dec(MAX_PROBE_TRIES);
                host::print(" Anstupsern — Verbindung verloren\n");
                host::loud_end();
                ls.poll_on = false;
                if ls.authorized || ls.link_up_sent {
                    host::netdev_set_link(false);
                    let down = [EV_LINK_DOWN, LINK_DOWN_DEAUTH];
                    host::wifi_send_event(&down);
                }
                ls.authorized = false;
                ls.link_up_sent = false;
                if frist_us == 0 {
                    return PumpEnd::LinkLost;
                }
            } else {
                // mlme.c:4391-4396: the last three as broadcast.
                let gerichtet = ls.probe_send_count < PROBE_UNICAST_LIMIT;
                let mut pr = [0u8; 128];
                let n = build_probe_req_to(
                    &mut pr, &mac, link.channel,
                    if gerichtet { Some(&bssid) } else { None },
                    if gerichtet {
                        &link.ssid[..link.ssid_len as usize]
                    } else {
                        &[]
                    });
                let mut info = tx::pkt_info_update(&pr[..n], 0,
                                                   tx::band_of(link.channel));
                let q = tx::RTW_TX_QUEUE_MGMT;
                if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &pr[..n]) {
                    pci::tx_kick_off_queue(h, trx, q);
                }
                ls.probe_send_count += 1;
                ls.probe_timeout_ms = now + PROBE_WAIT_MS;
            }
        }

        // ── Commands from wifid ──────────────────────────────────
        loop {
            let clen = host::wifi_poll_cmd(cmdbuf);
            if clen <= 0 {
                break;
            }
            let cmd = &cmdbuf[..clen as usize];
            match cmd.first().copied() {
                // TX_EAPOL: [op][len u16 LE][frame]
                Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {
                    let len = ((cmd[2] as usize) << 8) | cmd[1] as usize;
                    if cmd.len() >= 3 + len {
                        let mut eth = [0u8; 600];
                        eth[0..6].copy_from_slice(&link.bssid);
                        eth[6..12].copy_from_slice(&mac);
                        eth[12..14]
                            .copy_from_slice(&ETHERTYPE_EAPOL.to_be_bytes());
                        eth[14..14 + len].copy_from_slice(&cmd[3..3 + len]);
                        // Encrypted once the PTK is installed. msg2 and msg4
                        // go out in the clear as required; a group rekey
                        // comes later with the PTK in place, and the AP
                        // expects it protected.
                        let enc = link.ptk_installed;
                        // EAPOL always requests a TX report. In Linux this
                        // comes from mac80211: control port frames carry
                        // `IEEE80211_TX_CTL_REQ_TX_STATUS`, since their loss
                        // costs the link.
                        let sn = ls.arm_probe(now);
                        if tx_8023(h, trx, mgmt_buf, link,
                                   &eth[..14 + len], enc, sn, None) {
                            // A single frame the AP is waiting for: kick at
                            // once.
                            pci::tx_kick_off_queue(h, trx, pci::Q_BE);
                            ls.eapol_tx += 1;
                            if ls.authorized {
                                ls.rekey_tx += 1;
                            }
                        } else {
                            host::say("  EAPOL NICHT GESENDET — der AP\n\
                             \x20         wird es wiederholen und dann\n\
                             \x20         aufgeben\n");
                        }
                    }
                }
                // SET_KEY: [op][key_type][key_idx][cipher][key_len][key][rsc 6]
                Some(CMD_SET_KEY) if cmd.len() >= 5 => {
                    let key_type = cmd[1];
                    let key_idx = cmd[2];
                    let key_len = cmd[4] as usize;
                    if cmd.len() >= 5 + key_len + 6 {
                        let key = &cmd[5..5 + key_len];
                        let group = key_type == 1;
                        // Pairwise key in slot 0, group key at its index, as
                        // mac80211 does.
                        let slot = if group { key_idx.min(3) } else { 0 };
                        let addr = if group { [0xffu8; 6] } else { link.bssid };
                        sec::write_cam(h, &mut link.cam[slot as usize], slot,
                                       RTW_CAM_AES as u8, key_idx, group,
                                       &addr, key);
                        ls.keys_set += 1;
                        // A new key starts its own packet numbers: the
                        // pairwise key from zero, the group key from the
                        // RSC the AP gave with it (`ieee80211_key_alloc`).
                        let rsc = &cmd[5 + key_len..5 + key_len + 6];
                        let start = rsc.iter().rev().fold(0u64, |a, &b| a << 8 | b as u64);
                        ls.rx_pn[group as usize] = [if group { start } else { 0 }; 17];
                        if group {
                            ls.gtk_set += 1;
                        } else {
                            link.ptk_installed = true;
                        }
                        host::print("  Schluessel gesetzt: ");
                        host::print(if group { "GTK" } else { "PTK" });
                        host::print(" Platz ");
                        host::print_dec(slot as u32);
                        host::print(", ");
                        host::print_dec(key_len as u32);
                        host::print(" Bytes\n");
                    }
                }
                // AUTHORIZED: the handshake is done.
                Some(CMD_AUTHORIZED) => {
                    ls.authorized = true;
                    host::netdev_set_link(true);
                    let mut up = [0u8; 7];
                    up[0] = EV_LINK_UP;
                    up[1..7].copy_from_slice(&link.bssid);
                    host::wifi_send_event(&up);
                    ls.link_up_sent = true;
                    host::print("  *** AUTHORIZED — Datenweg offen ***\n");
                }
                _ => {}
            }
        }

        // ── Send what the IP stack wants out ─────────────────────
        //
        // Queue everything first, then kick once (tx.c:660-676), so at its
        // next transmit opportunity the hardware sees the whole batch and
        // can combine it into one A-MPDU.
        //
        // mlme.c:2982 `if (csa_ie.mode) ieee80211_vif_block_queues_csa`:
        // `mode = 1` means stop transmitting. The AP is vacating the channel,
        // on a DFS channel because it detected radar, and every further frame
        // from us is one too many on a frequency that must be cleared.
        let sendesperre = link.csa.map_or(false, |c| c.mode != 0);
        if ls.authorized && !sendesperre {
            let mut gestapelt = 0u32;
            loop {
                let n = host::netdev_poll_tx(ethbuf);
                if n <= 0 {
                    break;
                }
                let enc = link.ptk_installed;
                // One data frame per watchdog tick requests a TX report.
                // Deviation: Linux learns TX success via mac80211 and asks
                // only for control frames. We lack that path entirely, and
                // one report every two seconds answers "does the AP hear me"
                // at no cost.
                let sn = if probe_due { probe_due = false; ls.arm_probe(now) }
                         else { None };
                // Our stack's turnaround. During a download practically
                // every transmitted frame is a TCP ACK triggered by received
                // data, so the time between "data to the kernel" and "frame
                // back from the kernel" is our side's turnaround, which adds
                // directly to the RTT the server measures. It separates "the
                // air" from "us".
                if ls.last_rx_at != 0 {
                    let d = host::now_us().saturating_sub(ls.last_rx_at);
                    if misst && d < GAP_IDLE_US as u64 {
                        ls.turn_sum += d;
                        ls.turn_n += 1;
                        if d > ls.turn_max { ls.turn_max = d; }
                        let i = bucket(d as u32, &TURN_BUCKETS);
                        ls.turn_buckets[i] += 1;
                        if i == 4 {
                            ls.turn_big_sum += d;
                        }
                    }
                    ls.last_rx_at = 0;
                }
                if tx_8023(h, trx, mgmt_buf, link,
                           &ethbuf[..n as usize], enc, sn, qos_tid) {
                    gestapelt += 1;
                    ls.data_tx += 1;
                    // tx.c `rtw_tx`: the same accounting as on receive, so
                    // `tx_throughput` has a value.
                    if ethbuf[0] & 0x01 == 0 {
                        d.stats.tx_unicast += n as u64;
                        d.stats.tx_cnt += 1;
                    }
                }
            }
            if gestapelt > 0 {
                // Measure first, then kick: after the kick the number
                // changes, the hardware starts as soon as the write pointer
                // is set.
                let im_ring = pci::tx_pending(h, trx, pci::Q_BE);
                pci::tx_kick_off_queue(h, trx, pci::Q_BE);
                ls.tx_batch_n += 1;
                ls.tx_batch_sum += gestapelt;
                if gestapelt > ls.tx_batch_max {
                    ls.tx_batch_max = gestapelt;
                }
                ls.tx_ring_sum += im_ring;
                if im_ring > ls.tx_ring_max {
                    ls.tx_ring_max = im_ring;
                }
            }
        }

        // `rtw_pci_tx_isr`: advance the read pointer.
        pci::tx_isr(h, trx, pci::Q_BE);
        pci::tx_isr(h, trx, tx::RTW_TX_QUEUE_MGMT);

        // ── Every two seconds: `rtw_watch_dog_work` ──────────────
        // The period is Linux': `RTW_WATCH_DOG_DELAY_TIME` = HZ * 2. The
        // tracking loops in it compute on what arrived since the last tick;
        // another period would be another controller.
        ls.purge_probes(now);
        if now.wrapping_sub(watch_dog_ms) >= RTW_WATCH_DOG_DELAY_MS {
            watch_dog_ms = now;
            probe_due = true;
            if fw::fw_crashed(h) {
                ls.fw_crash += 1;
                if ls.fw_crash <= 3 {
                    host::loud_begin();
                    host::print("[rtl8822ce] DIE FIRMWARE HAT SICH SELBST FUER\n\
                     \x20         TOT ERKLAERT (REG_MCU_TST_CFG = FW_TRIGGER).\n\
                     \x20         Linux zieht das Geraet hier neu hoch; das\n\
                     \x20         ist gebaut, sobald das Wiederverbinden steht.\n");
                    host::loud_end();
                }
                if frist_us == 0 {
                    return PumpEnd::LinkLost;
                }
            }
            watch_dog(h, hal, d, h2c, e, link, caps, fw_feature,
                      ls.authorized || ls.link_up_sent, beacon_int);
            wd_ran = true;
            // ── Roaming: switch before the link breaks ───────────
            //
            // mlme.c:6800-6828 `ieee80211_handle_beacon_sig`: only after
            // four beacons, then a threshold with hysteresis. The event fires
            // again only once the level moves past the hysteresis; otherwise
            // a single bad beacon triggers a scan, and then the next one.
            // Only after the four-way handshake: during it the trigger would
            // fire in stage 6a, which discards the pump's result.
            if roam_mode != RoamMode::Aus
                && ls.authorized
                // And not right after a setup: `authorized` is set once the
                // handshake completes, but the cell has not yet delivered
                // four beacons and traffic is just starting.
                && now.saturating_sub(link.roam.last_roam_ms) > ROAM_GAP_MS
                && link.roam.count >= SIGNAL_AVE_MIN_COUNT
                && link.csa.is_none()
            {
                let sig = link.roam.dbm();
                let le = link.roam.last_event;
                // The signal level is the only trigger, as in mac80211
                // (`ieee80211_handle_beacon_sig`: threshold with hysteresis
                // on the smoothed beacon level). `curr_rx_rate` is the rate
                // of the last frame, usually a beacon at OFDM 6M, and
                // `ofdm_err/ofdm_ok` come from a hardware CRC32 counter
                // (rtw8822c.c:2855) that counts all OFDM frames on the
                // channel, including foreign ones; neither is a usable
                // trigger. A trigger that never turns off is none.
                let tief = sig < ROAM_THOLD_DBM
                    && (le == 0 || sig < le - ROAM_HYST_DB);
                // Never scan while data flows: a scan then costs throughput
                // for nothing.
                let ruhig = d.stats.rx_throughput < 2 && d.stats.tx_throughput < 2;
                if tief && ruhig
                    && now.saturating_sub(link.roam.last_scan_ms)
                        > ROAM_SCAN_GAP_MS
                {
                    link.roam.last_event = sig;
                    link.roam.last_scan_ms = now;
                    link.roam.scans += 1;
                    host::loud_begin();
                    host::print("[rtl8822ce] Pegel ");
                    print_dbm(sig);
                    host::print(" (Schwelle ");
                    print_dbm(ROAM_THOLD_DBM);
                    host::print(")");
                    host::print(" — die bekannten Kanaele werden abgehorcht\n");
                    host::loud_end();

                    let mut kand = [Bss {
                        bssid: [0; 6], ssid: [0; 32], ssid_len: 0,
                        channel: 0, best: -128, beacons: 0, resps: 0,
                        capability: 0, rsn: [0; 64], rsn_len: 0,
                        ht_param: 0, ht_op_seen: false, ht_cap: 0,
                        vht_chanwidth: 0, vht_cch0: 0, vht_cch1: 0,
                        vht_op_seen: false,
                        ap_vht_cap: 0, ap_vht_cap_seen: false,
                        bss_load: 0, bss_load_seen: false, wmm: false,
                    }; ROAM_BSS_MAX];
                    let mut n_kand = 0usize;
                    let ms = roam_scan(h, hal, trx, mgmt_buf, e, t_pwr, link,
                                       rxbuf, &mut d.dm, &mut d.path_div,
                                       &mut kand, &mut n_kand);
                    let max_bw = max_bw_for(e);
                    let mut best: Option<Bss> = None;
                    let mut selbst_gut: Option<i8> = None;
                    host::loud_begin();
                    host::print("            ");
                    host::print_dec(ms);
                    host::print(" ms weg, ");
                    host::print_dec(n_kand as u32);
                    host::print(" Zellen gehoert\n");
                    for b in kand[..n_kand].iter() {
                        if b.ssid_len != link.ssid_len
                            || b.ssid[..b.ssid_len as usize]
                                != link.ssid[..link.ssid_len as usize]
                        {
                            continue;
                        }
                        let (_, bw, _) = chan_params(b.channel, b.width(),
                                                     max_bw);
                        host::print("            K");
                        host::print_dec(b.channel as u32);
                        host::print(" ");
                        print_dbm(b.best);
                        host::print(" ");
                        host::print(match bw {
                            2 => "80 MHz",
                            1 => "40 MHz",
                            _ => "20 MHz",
                        });
                        print_last(b);
                        if b.bssid == link.bssid {
                            host::print("  (wir)");
                            // We just measured ourselves. The smoothed beacon
                            // level and a fresh probe of the same cell can
                            // disagree widely; the fresh probe is the one to
                            // trust. If it is above the threshold, there is
                            // no reason to leave.
                            if b.best > ROAM_THOLD_DBM {
                                selbst_gut = Some(b.best);
                            }
                        } else if roam_better(sig, d.cur_bw, b, max_bw) {
                            host::print("  BESSER");
                            // Among several good candidates the wider one
                            // wins, then the louder. `roam_better` compares
                            // width against us; here candidates are compared
                            // with each other.
                            let besser = match best {
                                None => true,
                                Some(x) => {
                                    let (_, xbw, _) = chan_params(
                                        x.channel, x.width(), max_bw);
                                    (bw, b.best) > (xbw, x.best)
                                }
                            };
                            if besser {
                                best = Some(*b);
                            }
                        }
                        host::print("\n");
                    }
                    host::loud_end();

                    if let Some(gut) = selbst_gut {
                        host::loud_begin();
                        host::print("[rtl8822ce] kein Wechsel: der Suchlauf hoert UNS mit ");
                        print_dbm(gut);
                        host::print(" (geglaettet ");
                        print_dbm(sig);
                        host::print(") — die frische Probe gilt\n");
                        host::loud_end();
                        // Reseed the smoothed value, otherwise it fires again
                        // on the next tick.
                        link.roam.ave = dm::Ewma::new();
                        link.roam.count = 0;
                        best = None;
                    }
                    if let Some(z) = best {
                        if now.saturating_sub(link.roam.last_roam_ms)
                            > ROAM_GAP_MS
                        {
                            host::loud_begin();
                            host::print("[rtl8822ce] ");
                            host::print(if roam_mode == RoamMode::NurBericht {
                                "WUERDE WECHSELN zu K"
                            } else {
                                "WECHSEL zu K"
                            });
                            host::print_dec(z.channel as u32);
                            host::print(" ");
                            print_dbm(z.best);
                            host::print(" (wir: ");
                            print_dbm(sig);
                            host::print(")\n");
                            host::loud_end();
                            if roam_mode == RoamMode::An {
                                link.roam.last_roam_ms = now;
                                link.roam.roams += 1;
                                link.roam.to = Some(z);
                                // Tear the link down properly, as on the
                                // deauth path: otherwise the kernel keeps its
                                // carrier and pushes data during the whole
                                // re-association, and `wifid` sees no link
                                // down and keeps its old supplicant while we
                                // associate with another cell.
                                if ls.authorized || ls.link_up_sent {
                                    host::netdev_set_link(false);
                                    let down = [EV_LINK_DOWN,
                                                LINK_DOWN_DEAUTH];
                                    host::wifi_send_event(&down);
                                }
                                ls.authorized = false;
                                ls.link_up_sent = false;
                                return PumpEnd::Roam;
                            }
                        }
                    }
                }
            }

            // mlme.c:4427-4480 `ieee80211_mgd_probe_ap(sdata, true)`. Beacon
            // loss is computed by `rtw_sw_beacon_loss_check`; without acting
            // on it a link whose AP vanished would stay up forever.
            if d.beacon_loss && !ls.poll_on {
                ls.poll_on = true;
                ls.probe_send_count = 0;
                ls.probe_timeout_ms = 0; // probe at once
                ls.poll_started += 1;
                host::say("[rtl8822ce] keine Baken mehr vom AP — anstupsen statt aufgeben\n");
            }
            // `rtw_phy_stat_rate_cnt` has just shifted the window into
            // `last_pkt_count`; now and only now it is complete.
            for i in 0..DESC_RATE_MAX {
                ls.rate_hist[i] = ls.rate_hist[i]
                    .saturating_add(d.dm.last_pkt_count.num_qry_pkt[i] as u32);
            }
            // Same place, same reason: `false_alarm_statistics` has just read
            // and reset.
            ls.ht_ok += d.dm.ht_ok_cnt as u64;
            ls.ht_err += d.dm.ht_err_cnt as u64;
            ls.ofdm_ok += d.dm.ofdm_ok_cnt as u64;
            ls.ofdm_err += d.dm.ofdm_err_cnt as u64;
        }

        // ── Once per second: the report for `wlan` ───────────────
        // The air is invisible to the kernel. Rate, counters and key state
        // appear nowhere else; without them a link slow because of a legacy
        // rate cannot be told from one slow because of full queues.
        if now.wrapping_sub(report_ms) >= 1000 {
            report_ms = now;
            publish_report(link, ls, d, e, caps);
        }

        // ── RX silence as a watchdog ─────────────────────────────
        // A live cell always sends something, beacons alone are ten per
        // second. Complete silence means the ring is stuck, not that the air
        // is empty.
        if got > 0 {
            rx_silent_ms = now;
        } else if now.wrapping_sub(rx_silent_ms) > 5_000 {
            rx_silent_ms = now;
            ls.rx_wd += 1;
            if ls.rx_wd <= 4 {
                // A stuck ring is not a stage finding but an error: loud even
                // without `debug: 1`.
                host::loud_begin();
                host::print("[rtl8822ce] RX still seit 5 s — Ringzeiger rp=");
                host::print_dec(trx.rx.rp);
                host::print(", rx_tag ");
                host::print_dec(trx.rx_tag as u32);
                host::print("\n");
                host::loud_end();
            }
        }

        // ── When to let go of the ring ───────────────────────────
        //
        // Between two bursts the ring is briefly empty; sleeping a whole
        // millisecond on the first empty poll would cap polling at a thousand
        // per second and throughput at `1000 x burst size`. Instead the shape
        // Linux calls NAPI: a budget of empty polls, then sleep, and stay
        // parked until something arrives. Under load the counter resets on
        // every burst and we never sleep; when idle the budget runs out
        // within half a millisecond.
        {
            let dt = (host::now_us() - t_iter).min(u32::MAX as u64) as u32;
            // SAFETY: as `WD_MAX`.
            unsafe {
                if dt > ITER_MAX {
                    ITER_MAX = dt;
                    ITER_MAX_WD = wd_ran;
                }
            }
        }
        // SAFETY: only this fiber reads IRQ_VEC.
        let irq = unsafe { IRQ_VEC } >= 0;
        if got == 0 && irq {
            // With MSI: park until the chip signals. The shape of
            // `rtw_pci_napi_poll` when less than the budget arrived: ack HISR
            // (otherwise no new edge), arm HIMR, and look at the ring again,
            // since whatever arrived between the last poll and arming raises
            // no interrupt.
            pci::irq_recognized(h);
            pci::enable_interrupt(h, false);
            // Clear the TX ring once more, after arming. HIMR is zero for the
            // whole round, and with zero the chip sets no HISR bit, so a TX
            // completion (TX-DOK) during the round never raises an
            // interrupt. For receive the ring check below catches that; for
            // transmit this does. Linux keeps the DOK interrupts enabled
            // during RX processing (`rtw_pci_enable_interrupt(..,
            // exclude_rx = true)`); here, whatever completed before this call
            // is reaped by it, whatever completes after wakes us.
            pci::tx_isr(h, trx, pci::Q_BE);
            pci::tx_isr(h, trx, tx::RTW_TX_QUEUE_MGMT);
            if pci::get_hw_rx_ring_nr(h, trx) == 0 {
                let mut mask = host::WAIT_IRQ | host::WAIT_WIFI_CMD;
                // Only if this round also drains the queue; otherwise it
                // signals again at once and the loop spins.
                if ls.authorized && !sendesperre {
                    mask |= host::WAIT_NET_TX;
                }
                // Until the earliest own timer, not a fixed period. Every
                // time-dependent check in this loop reports its deadline
                // here, and we park until the earliest one.
                let jetzt = host::now_ms();
                let mut warte: u64 = 1000;
                let mut faellig = |at: u64| warte = warte.min(at.saturating_sub(jetzt));
                faellig(watch_dog_ms + RTW_WATCH_DOG_DELAY_MS);
                faellig(report_ms + 1000);
                faellig(rx_silent_ms + 5_001);
                if link.ba_tx.state == BaState::Gefragt {
                    faellig(link.ba_tx.at_ms + BA_RESP_MS + 1);
                }
                if ls.poll_on {
                    faellig(ls.probe_timeout_ms);
                }
                if let Some(at) = ls.next_probe_due() {
                    faellig(at);
                }
                if link.csa.is_some() || link.csa_zurueck.is_some() {
                    warte = warte.min(CSA_WAIT_MS);
                }
                if let Some(ms) = ro_due_ms(ls) {
                    warte = warte.min(ms);
                }
                if frist_us != 0 {
                    let rest = (t0 + frist_us).saturating_sub(host::now_us()) / 1000;
                    warte = warte.min(rest);
                }
                // At least 1 ms: `now_ms` counts in 10 ms steps, and a
                // deadline that says "now" while its condition triggers only
                // in the next step would otherwise spin for up to 10 ms.
                host::wait(mask, warte.max(1) as u32);
            }
            // `rtw_pci_interrupt_handler`: HIMR off until the round is done.
            pci::disable_interrupt(h);
        } else if got == 0 {
            if leer_in_folge < RX_SPIN_BUDGET {
                leer_in_folge += 1;
            } else {
                host::sleep_ms(1);
            }
        } else {
            leer_in_folge = 0;
        }
    }

    PumpEnd::Frist
}


/// How many empty ring polls before going to sleep.
///
/// One loop round costs half a dozen host calls, roughly five to ten
/// microseconds, so 64 empty rounds keep us awake for about half a
/// millisecond. Under load the next frame comes much sooner; when idle it
/// is a one-time cost per beacon.
const RX_SPIN_BUDGET: u32 = 64;

/// How long to wait after a failed attempt. A restarting AP needs seconds;
/// asking more often does not help and only fills the log.
const RECONNECT_BACKOFF_MS: u32 = 3000;

/// After how many failed attempts the surroundings are scanned again. Two,
/// not one: a restarting AP is back after three seconds, and a scan would
/// cost more than waiting.
const RESCAN_AFTER_TRIES: u32 = 2;

/// The way back into a running link.
///
/// The same as the way in, stage 5e (auth + assoc) and 5f (rate
/// adaptation), with four differences:
///
/// * No `netdev_register`: the kernel already knows the interface; a second
///   registration would create a second one.
/// * Keys out before renegotiating: an old pairwise key in the CAM would
///   decrypt the first frames of the new link wrongly, which looks like a
///   broken handshake.
/// * `rtw_mac_flush_queues`: whatever is still queued belongs to the old
///   link and would go out with the old key.
/// * The packet number restarts at one (802.11 §12.5.3.2: it belongs to the
///   key, and the key is about to be new).
///
/// And a new `EV_READY`: `wifid` needs a fresh supplicant with a new
/// SNonce. Unlike a stray second `EV_READY`, this one comes with a new
/// association.
#[allow(clippy::too_many_arguments)]
fn reconnect(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
             h2c: &mut fw::H2cState, e: &efuse::Efuse,
             t: &txpower::TxPower, bss: &Bss, link: &mut Link,
             ls: &mut LinkStats, d: &mut Dev,
             linked: &mut Option<vif::Vif>) -> bool {
    ls.reconnects += 1;
    // The cell can be a different one: after two failures the caller scans
    // again, and `bss` may carry another BSSID, channel or name. Without
    // updating here, data frames would still go to the AP just lost.
    if link.bssid != bss.bssid {
        host::loud_begin();
        host::print("[rtl8822ce] andere Zelle: K");
        host::print_dec(bss.channel as u32);
        host::print(" ");
        print_dbm(bss.best);
        host::print("\n");
        host::loud_end();
    }
    let andere = link.bssid != bss.bssid;
    link.bssid = bss.bssid;
    link.channel = bss.channel;
    link.ssid = bss.ssid;
    link.ssid_len = bss.ssid_len;
    // The width of the new cell; without it a roam scan would return at the
    // width of the old one.
    link.ht_param_now = bss.ht_param;
    link.vht_chanwidth_now = bss.vht_chanwidth;
    link.vht_cch0_now = bss.vht_cch0;
    if andere {
        // The smoothed level belongs to the cell, not the link: carrying
        // the old AP's value over would make the next candidate look better
        // at once and cause constant switching. `count` resets with it:
        // until four beacons of the new cell arrive the mean says nothing,
        // and no switch happens (`SIGNAL_AVE_MIN_COUNT`, mlme.c:96).
        link.roam.ave = dm::Ewma::new();
        link.roam.count = 0;
        link.roam.last_event = 0;
        link.roam.last_roam_ms = host::now_ms();
    }
    host::loud_begin();
    host::print("[rtl8822ce] Verbindung weg — Anlauf ");
    host::print_dec(ls.reconnects);
    host::print(" auf \"");
    print_ssid(&bss.ssid[..bss.ssid_len as usize]);
    host::print("\", Kanal ");
    host::print_dec(bss.channel as u32);
    host::print("\n");
    host::loud_end();

    // The kernel must stop pushing into the dead link.
    host::netdev_set_link(false);

    // Old keys out of the CAM. `write_cam` put them in, `clear_cam` takes
    // them out, slot by slot as they were used.
    for slot in 0..link.cam.len() {
        sec::clear_cam(h, &mut link.cam[slot], slot as u8);
    }
    link.ptk_installed = false;
    link.tx_pn = 1;
    link.seq = 0;
    // Block ack sessions belong to the association, not the driver. Left as
    // `Laeuft` from the old cell, every data frame would keep carrying QoS
    // and AGG_EN to an AP we never negotiated a session with. The state
    // machine starts over and asks the new AP.
    link.ba_tx = BaTx::new();
    // The same in the receive direction. The new AP sends its own ADDBA
    // request and `ro_open` resets the TID then, but until that the buffer
    // would hold frames of the new cell against the old sequence numbers.
    for t in 0..RO_TIDS {
        ro_close(ls, t);
    }

    let leer = mac::flush_queues(h);
    if leer < 4 {
        host::loud_begin();
        host::print("  nur ");
        host::print_dec(leer);
        host::print(" von 4 Sendeschlangen wurden leer — der Rest geht
         \x20         mit dem alten Schluessel verloren
");
        host::loud_end();
    }

    // Stage 5e: auth and assoc, on the same channel.
    if !stage5e_connect(h, hal, trx, mgmt_buf, h2c, e, t, e.addr, bss,
                        linked, d) {
        return false;
    }
    let Some(v) = linked.as_ref() else { return false };

    // Stage 5f: the firmware picks the rate again.
    let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;
    if !stage5f_rates(h, trx, h2c, hal, v, bss, &mut rates, d) {
        return false;
    }
    if let Some((caps, si)) = rates {
        link.si = si;
        link.highest_rate = highest_tx_rate(&caps, hal);
    }

    // And `wifid` gets a fresh supplicant.
    let mut ready = [0u8; 13];
    ready[0] = EV_READY;
    ready[1..7].copy_from_slice(&link.bssid);
    ready[7..13].copy_from_slice(&link.mac);
    host::wifi_send_event(&ready);

    host::loud_begin();
    host::print("  wieder angemeldet, AID ");
    host::print_dec(linked.as_ref().map(|v| v.aid).unwrap_or(0));
    host::print(" — der Handschlag faengt von vorn an
");
    host::loud_end();
    true
}

/// Scans without losing the link.
///
/// The sequence is mac80211's (`ieee80211_offchannel_stop_vifs`,
/// offchannel.c:83-131), reduced to what we have:
///
/// 1. tell the AP we doze: null data with the power management bit set,
///    from then on it buffers for us.
/// 2. for each known channel: switch, send a probe request with our SSID,
///    listen briefly.
/// 3. back to our own channel, in our own width.
/// 4. wake up: the same without the bit, and the AP flushes.
///
/// Probes use the broadcast address with our SSID set, so every cell of
/// this network answers, not just one.
#[allow(clippy::too_many_arguments)]
fn roam_scan(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
             e: &efuse::Efuse, t_pwr: &txpower::TxPower, link: &Link,
             rxbuf: &mut [u8], dm: &mut dm::DmInfo,
             path_div: &mut dm::PathDiv,
             found: &mut [Bss; ROAM_BSS_MAX], n_found: &mut usize) -> u32 {
    let mac = link.mac;
    let leer = Bss {
        bssid: [0; 6], ssid: [0; 32], ssid_len: 0, channel: 0, best: -128,
        beacons: 0, resps: 0, capability: 0, rsn: [0; 64], rsn_len: 0,
        ht_param: 0, ht_op_seen: false, ht_cap: 0, vht_chanwidth: 0,
        vht_cch0: 0, vht_cch1: 0, vht_op_seen: false,
        ap_vht_cap: 0, ap_vht_cap_seen: false,
        bss_load: 0, bss_load_seen: false, wmm: false,
    };
    *found = [leer; ROAM_BSS_MAX];
    *n_found = 0;

    // (1) doze
    let mut nf = [0u8; 32];
    let n = build_nullfunc(&mut nf, &mac, &link.bssid, true);
    let mut info = tx::pkt_info_update(&nf[..n], 0, tx::band_of(link.channel));
    let q = tx::RTW_TX_QUEUE_MGMT;
    if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &nf[..n]) {
        pci::tx_kick_off_queue(h, trx, q);
    }
    // Give the frame time to go out; otherwise we switch channel before the
    // AP learns we are gone.
    host::sleep_ms(2);

    let t0 = host::now_us();
    // SAFETY: single-threaded, and the scan does not write while the pump
    // runs.
    let (chs, n_ch) = unsafe {
        (&*core::ptr::addr_of!(ROAM_CHANNELS), N_ROAM_CHANNELS)
    };
    for &ch in chs[..n_ch].iter() {
        // (2) switch, probe, listen
        let _ = switch_channel(h, hal, e, t_pwr, ch, CellWidth::default(), 0);
        let mut pr = [0u8; 128];
        let n = build_probe_req_to(&mut pr, &mac, ch, None,
                                   &link.ssid[..link.ssid_len as usize]);
        let mut info = tx::pkt_info_update(&pr[..n], 0, tx::band_of(ch));
        if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &pr[..n]) {
            pci::tx_kick_off_queue(h, trx, q);
        }
        let t_ch = host::now_us();
        while host::now_us() - t_ch < ROAM_DWELL_MS as u64 * 1000 {
            let got = pci::rx_poll(h, trx, 64, rxbuf, dm, path_div,
                                   hal.rf_path_num, 0, ch, |st, pkt| {
                if st.crc_err || st.is_c2h {
                    return;
                }
                let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                    + st.shift as usize;
                if off + 36 > pkt.len() {
                    return;
                }
                let f = &pkt[off..];
                let fc = f[0];
                if fc & 0xfc != 0x80 && fc & 0xfc != 0x50 {
                    return;
                }
                let _ = record_bss(found, n_found, f, ch, st.signal_power,
                                   fc & 0xfc == 0x80);
            });
            if got == 0 {
                host::sleep_ms(1);
            }
        }
    }

    // (3) back, in the width the link runs
    let heim = CellWidth {
        ht_param: link.ht_param_now,
        vht_chanwidth: link.vht_chanwidth_now,
        vht_cch0: link.vht_cch0_now,
    };
    let _ = switch_channel(h, hal, e, t_pwr, link.channel, heim,
                           max_bw_for(e));

    // (4) wake up
    let n = build_nullfunc(&mut nf, &mac, &link.bssid, false);
    let mut info = tx::pkt_info_update(&nf[..n], 0, tx::band_of(link.channel));
    if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &nf[..n]) {
        pci::tx_kick_off_queue(h, trx, q);
    }
    ((host::now_us() - t0) / 1000) as u32
}

/// Whether the candidate is better than the current cell.
///
/// The rule is a policy, not a port; in Linux it lives in wpa_supplicant.
/// Two ways lead to a switch:
///
/// * it is clearly stronger (`ROAM_BETTER_DB`), or
/// * it is wider and at most `ROAM_WIDER_TOLERANCE_DB` weaker.
///
/// The second covers a stronger but narrower range extender, which wins
/// any pure signal comparison against a weaker, wider AP while delivering
/// less.
fn roam_better(jetzt_dbm: i8, jetzt_bw: usize, kand: &Bss,
               max_bw: usize) -> bool {
    let (_, kand_bw, _) = chan_params(kand.channel, kand.width(), max_bw);
    let d = kand.best as i32 - jetzt_dbm as i32;
    // Width is not traded for signal: every halving of the width must be
    // paid with `ROAM_NARROWER_COST_DB`. From 80 to 20 MHz are two steps, at
    // 10 dB each twenty.
    let schmaler = jetzt_bw.saturating_sub(kand_bw) as i32;
    let preis = schmaler * ROAM_NARROWER_COST_DB as i32;
    if d >= ROAM_BETTER_DB as i32 + preis {
        return true;
    }
    kand_bw > jetzt_bw && d >= -(ROAM_WIDER_TOLERANCE_DB as i32)
}

/// Stage 6a: setup, handshake and the first eight seconds.
#[allow(clippy::too_many_arguments)]
fn stage6a_link(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
                bss: &Bss, caps: &sta::PeerCaps, si: sta::StaInfo,
                mac: [u8; 6], link: &mut Option<Link>,
                ls: &mut LinkStats, d: &mut Dev, h2c: &mut fw::H2cState,
                e: &efuse::Efuse, t_pwr: &txpower::TxPower,
                fw_feature: u32) -> bool {
    host::print("[rtl8822ce] Stufe 6a: der Steuerkanal und der Datenweg\n");

    let mut l = link_setup(hal, bss, caps, si, mac);
    // Eight seconds: the handshake takes four frames and completes within
    // milliseconds; waiting longer means waiting for an error.
    let _ = link_pump(h, hal, trx, mgmt_buf, &mut l, ls, mac, 8_000_000, d,
                      h2c, e, t_pwr, caps, fw_feature);

    host::print("  EAPOL rein/raus ");
    host::print_dec(ls.eapol_rx);
    host::print("/");
    host::print_dec(ls.eapol_tx);
    host::print(" · Schluessel ");
    host::print_dec(ls.keys_set);
    host::print(" · Datenrahmen rein/raus ");
    host::print_dec(ls.data_rx);
    host::print("/");
    host::print_dec(ls.data_tx);
    host::print("\n");

    let mut ok = true;
    ok &= gate("der AP faengt den Vierwegehandschlag an (EAPOL msg1)",
               ls.eapol_rx > 0);
    ok &= gate("wir beantworten ihn", ls.eapol_tx > 0);
    ok &= gate("wifid installiert Paar- und Gruppenschluessel",
               ls.keys_set >= 2);
    ok &= gate("der Handschlag ist durch (AUTHORIZED, LINK_UP)",
               ls.authorized && ls.link_up_sent);
    *link = Some(l);
    ok
}

/// A gate. A failed one is always printed, otherwise a silent run would not
/// say where it stopped.
fn gate(name: &str, ok: bool) -> bool {
    if !ok {
        host::loud_begin();
    }
    host::print(if ok { "  [ JA  ] " } else { "  [NEIN ] " });
    host::print(name);
    host::print("\n");
    if !ok {
        host::loud_end();
    }
    ok
}

/// The occupied slots of the C2H census.
fn d_c2h(ls: &LinkStats) -> impl Iterator<Item = (u8, u32)> + '_ {
    ls.c2h_ids.iter().filter(|e| e.1 > 0).map(|e| (e.0, e.1))
}

/// The one line a silent run prints.
///
/// It is not part of `driver_report`, which goes to `wlan` and is state
/// refreshed every second. This is the event: the link is up, with whom,
/// how fast and how wide.
///
///     [rtl8822ce] verbunden: "MyNet" K7 -49 dBm · HT MCS8-15
///                 (0x1b) · 40 MHz · AID 3
fn report_connected(link: &Link, bss: Option<&Bss>, vif: Option<&vif::Vif>) {
    host::loud_begin();
    host::print("[rtl8822ce] verbunden: ");
    match bss {
        Some(b) => {
            host::print("\"");
            print_ssid(&b.ssid[..b.ssid_len as usize]);
            host::print("\"");
        }
        None => host::print("(ohne Namen)"),
    }
    host::print(" K");
    host::print_dec(link.channel as u32);
    if let Some(b) = bss {
        host::print(" ");
        print_dbm(b.best);
    }
    host::print(" · ");
    host::print(rate_name(link.highest_rate));
    host::print(" (0x");
    host::print_hex8(link.highest_rate);
    host::print(") · ");
    host::print(match link.si.bw_mode {
        0 => "20 MHz",
        1 => "40 MHz",
        2 => "80 MHz",
        _ => "? MHz",
    });
    if let Some(v) = vif {
        host::print(" · AID ");
        host::print_dec(v.aid);
    }
    host::print("\n");
    host::loud_end();
}

/// One line of the final summary. Green is stage output, red is always
/// printed.
fn stage_line(ok: bool, green: &str, red: &str) {
    if ok {
        host::print(green);
    } else {
        host::say(red);
    }
}

/// `debug:` from `sys/config/wifi`. A missing file or line means no: a
/// driver in autostart stays silent until asked.
///
/// Also returns the read's return value: "no `debug:` in the file" and "the
/// file was missing" lead to the same silence but have different fixes.
fn read_debug_flag() -> (bool, i32) {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return (false, n);
    }
    let on = match cfg_get(&cfg[..n as usize], b"debug") {
        Some((a, b)) => cfg_on(&cfg[a..b]),
        None => false,
    };
    (on, n)
}

/// Which band the configuration wants.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BandPref {
    /// Default: 5 GHz when usable, otherwise 2.4.
    Auto,
    /// Only 2.4 GHz, the fallback when 5 GHz causes trouble.
    Only24,
    /// Only 5 GHz, for measurements, so no strong 2.4 GHz neighbor takes
    /// over the decision.
    Only5,
}

/// `band:` from `sys/config/wifi`. The pure part, so `framecheck.py` can run
/// it without hardware.
///
/// An unknown value is `Auto`, not a band. Unlike `aspm` there is no safe
/// side here: a typo should yield the default, not lock into a band the
/// network may not use.
pub fn band_pref_from(v: &[u8]) -> BandPref {
    if v.starts_with(b"5") {
        BandPref::Only5
    } else if v.starts_with(b"2") {
        BandPref::Only24
    } else {
        BandPref::Auto
    }
}

fn read_band_pref() -> BandPref {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return BandPref::Auto;
    }
    match cfg_get(&cfg[..n as usize], b"band") {
        Some((a, b)) => band_pref_from(&cfg[a..b]),
        None => BandPref::Auto,
    }
}

/// From this signal strength on 5 GHz is the better choice (class ABI
/// `WIFI_CLASS_ABI.md`: "prefer 5 GHz from -70 dBm"). Below it 2.4 GHz
/// reaches further, and a weak 5 GHz signal would be a step back.
const PREFER_5G_DBM: i8 = -70;

/// `aspm:` from `sys/config/wifi`: `an` · `aus` · `wie-gefunden`.
///
/// The default is off. The driver never sleeps (no LPS), so link power
/// saving has no counterpart that wakes it, and the card itself reports
/// 64 us to exit L1. It costs idle power instead, so it is a switch rather
/// than silent behavior.
fn read_aspm_pref() -> Option<bool> {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return Some(false);
    }
    match cfg_get(&cfg[..n as usize], b"aspm") {
        Some((a, b)) => aspm_pref_from(&cfg[a..b]),
        None => Some(false),
    }
}

/// The pure part of `read_aspm_pref`, separate so `framecheck.py` can run it
/// without hardware or file system.
///
/// `Some(true)` enable · `Some(false)` disable · `None` leave alone. An
/// unknown value means off, not "leave alone": whoever writes something
/// wants a change, and the safe reading of a typo is the default.
fn aspm_pref_from(v: &[u8]) -> Option<bool> {
    if v.starts_with(b"an") || v.starts_with(b"on") || v == b"1" {
        Some(true)
    } else if v.starts_with(b"wie") || v.starts_with(b"keep") {
        None
    } else {
        Some(false)
    }
}

/// The widest channel we may use, `RTW_CHANNEL_WIDTH_*`, i.e. 0/1/2.
///
/// The minimum of two sources. The card states its capability in the efuse
/// (`hw_cap_bw`, a bit field: bit 0 always, bit 1 for 40, bit 2 for 80; the
/// 8822CE reports 0x07, so 80 but no 160). `bw:` in `sys/config/wifi` caps
/// it by hand, e.g. `bw: 40`.
fn max_bw_for(e: &efuse::Efuse) -> usize {
    let mut bw = 0usize;
    // The highest set bit is the card's capability.
    for i in 0..3usize {
        if e.hw_cap_bw & (1 << i) != 0 {
            bw = i;
        }
    }
    bw.min(read_bw_cap())
}

/// `bw:` from `sys/config/wifi`, the pure part, so `framecheck.py` can run
/// it without hardware.
///
/// An unknown value is the default (80), not the narrowest setting, the
/// same rule as for `band:`. Unlike `aspm:` there is no safe side here:
/// narrow is not safer, only slower.
pub fn bw_cap_from(v: &[u8]) -> usize {
    if v.starts_with(b"20") {
        0
    } else if v.starts_with(b"40") {
        1
    } else {
        2
    }
}

fn read_bw_cap() -> usize {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return 2;
    }
    match cfg_get(&cfg[..n as usize], b"bw") {
        Some((a, b)) => bw_cap_from(&cfg[a..b]),
        None => 2,
    }
}

/// `txagg:` from `sys/config/wifi`, the pure part for `framecheck.py`.
///
/// An unknown value is on, the default, as for `band:` and `bw:`. `off` is
/// the escape hatch.
pub fn txagg_from(v: &[u8]) -> bool {
    !(v.starts_with(b"off") || v.starts_with(b"aus") || v == b"0")
}

fn read_txagg() -> bool {
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return true;
    }
    match cfg_get(&cfg[..n as usize], b"txagg") {
        Some((a, b)) => txagg_from(&cfg[a..b]),
        None => true,
    }
}

/// `ampdu:`: the window we grant the AP for its aggregation. Missing line:
/// the protocol maximum (see `sta::build_addba_resp`); `off`/`0`: no
/// aggregation; a number: exactly that window.
fn read_ampdu_buf() -> u16 {
    const VORGABE: u16 = sta::BA_TX_BUF_SIZE;
    let mut cfg = [0u8; 512];
    let n = host::fetch("sys/config/wifi", &mut cfg);
    if n <= 0 {
        return VORGABE;
    }
    let Some((a, b)) = cfg_get(&cfg[..n as usize], b"ampdu") else {
        return VORGABE;
    };
    let v = &cfg[a..b];
    if v.starts_with(b"off") || v == b"0" {
        return 0;
    }
    let mut num = 0u16;
    let mut any = false;
    for &c in v {
        if c.is_ascii_digit() {
            num = num.saturating_mul(10).saturating_add((c - b'0') as u16);
            any = true;
        } else {
            break;
        }
    }
    // The field is ten bits wide (`ADDBA_PARAM_BUF_SIZE_MASK`), and HT/VHT
    // cannot do more than 64 anyway: the compressed block ack bitmap has 64
    // slots.
    if any { num.clamp(1, sta::BA_TX_BUF_SIZE) } else { VORGABE }
}

/// `on` or `1`, the same rule `wifi_ax200` uses for `ampdu:` and `ps:`. An
/// unknown word is no, not a guess.
fn cfg_on(v: &[u8]) -> bool {
    v.starts_with(b"on") || v.starts_with(b"1")
}

/// `cfg_get` from `wifid`/`wifi_ax200`: one `key: value` line, `#` starts a
/// comment. Returns the bounds of the value, not a slice, because the
/// buffer is used alongside.
fn cfg_get(text: &[u8], key: &[u8]) -> Option<(usize, usize)> {
    let mut start = 0usize;
    while start <= text.len() {
        let end = text[start..].iter().position(|&b| b == b'\n')
            .map(|p| start + p).unwrap_or(text.len());
        let (a, b) = trim(text, start, end);
        if b > a && text[a] != b'#' {
            if let Some(c) = text[a..b].iter().position(|&x| x == b':') {
                let (ka, kb) = trim(text, a, a + c);
                if &text[ka..kb] == key {
                    let (va, vb) = trim(text, a + c + 1, b);
                    return Some((va, vb));
                }
            }
        }
        if end >= text.len() {
            break;
        }
        start = end + 1;
    }
    None
}

/// Strips spaces and tabs at both ends, and a trailing carriage return.
fn trim(t: &[u8], mut a: usize, mut b: usize) -> (usize, usize) {
    while a < b && (t[a] == b' ' || t[a] == b'\t') {
        a += 1;
    }
    while b > a && (t[b - 1] == b' ' || t[b - 1] == b'\t' || t[b - 1] == b'\r') {
        b -= 1;
    }
    (a, b)
}

/// How large our report may be. The kernel accepts up to
/// `drivers::report::REPORT_MAX` = 4096; this leaves the kernel a margin.
/// `put` truncates silently, which the report marks at its end.
const REPORT_CAP: usize = 3584;

/// docs/spec/WIFI_CLASS_ABI.md §3: `npk_driver_report`.
///
/// A plain text block the `wlan` intent prints beside the kernel view. The
/// kernel parses nothing; what is worth reporting is device knowledge.
fn publish_report(link: &Link, ls: &LinkStats, d: &Dev,
                  e: &efuse::Efuse, caps: &sta::PeerCaps) {
    // `put` truncates silently (`s.len().min(b.len() - *n)`); if the buffer
    // is ever too small, the report says so at its end.
    let mut b = [0u8; REPORT_CAP];
    let mut n = 0usize;
    let put = |s: &str, b: &mut [u8; REPORT_CAP], n: &mut usize| {
        let k = s.len().min(b.len() - *n);
        b[*n..*n + k].copy_from_slice(&s.as_bytes()[..k]);
        *n += k;
    };
    let num = |v: u32, b: &mut [u8; REPORT_CAP], n: &mut usize| {
        let mut d = [0u8; 10];
        let mut i = 10;
        let mut v = v;
        if v == 0 {
            i -= 1;
            d[i] = b'0';
        }
        while v > 0 {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        let k = (10 - i).min(b.len() - *n);
        b[*n..*n + k].copy_from_slice(&d[i..i + k]);
        *n += k;
    };

    put("rtl8822ce  ", &mut b, &mut n);
    put(if ls.authorized { "verbunden" } else { "NICHT verbunden" },
        &mut b, &mut n);
    put("  kanal ", &mut b, &mut n);
    num(link.channel as u32, &mut b, &mut n);
    // Three rates. `angeboten` is `link.highest_rate`, computed once at
    // association from the AP's capabilities: a claim about what is
    // possible. `tx` is what the firmware picked (C2H `RA_RPT`), `rx` what
    // each frame's RX descriptor says. Those are the measurements.
    let hex = b"0123456789abcdef";
    let rate_hex = |v: u8, b: &mut [u8; REPORT_CAP], n: &mut usize| {
        if *n + 2 <= b.len() {
            b[*n] = hex[(v >> 4) as usize];
            b[*n + 1] = hex[(v & 0xf) as usize];
            *n += 2;
        }
    };
    // The most frequent data rate over the whole link, not the beacons'
    // rate. A beacon always goes out at the lowest rate, and the longer a
    // link stands, the more surely beacons would win a plain maximum. So
    // counting starts at `DESC_RATEMCS0`; everything below is legacy and on
    // an HT/VHT link management traffic. Without any HT/VHT rate it falls
    // back to the peak over all, because then the link is legacy.
    let mcs0 = DESC_RATEMCS0 as usize;
    let spitze = |von: usize| ls.rate_hist.iter().enumerate().skip(von)
        .fold((0usize, 0u32), |acc, (i, &c)| if c > acc.1 { (i, c) } else { acc });
    let legacy: u32 = ls.rate_hist[..mcs0].iter().sum();
    let vht0 = DESC_RATEVHT1SS_MCS0 as usize;
    let ht_n: u32 = ls.rate_hist[mcs0..vht0].iter().sum();
    let vht_n: u32 = ls.rate_hist[vht0..].iter().sum();
    // When falling back to the peak over all, the denominator must be over
    // all too; otherwise it counts HT/VHT frames, of which there are none.
    let (top_rate, top_cnt, nur_legacy) = match spitze(mcs0) {
        (_, 0) => {
            let (r, c) = spitze(0);
            (r, c, true)
        }
        (r, c) => (r, c, false),
    };
    put("  rx ", &mut b, &mut n);
    put(rate_name(top_rate as u8), &mut b, &mut n);
    put(" (0x", &mut b, &mut n);
    rate_hex(top_rate as u8, &mut b, &mut n);
    put(" in ", &mut b, &mut n);
    num(top_cnt, &mut b, &mut n);
    put(" von ", &mut b, &mut n);
    let ges: u32 = ls.rate_hist.iter().sum();
    if nur_legacy {
        num(ges, &mut b, &mut n);
        put(" (NUR legacy), zuletzt ", &mut b, &mut n);
    } else {
        num(ges - legacy, &mut b, &mut n);
        // HT and VHT are two classes, and the difference is a factor on the
        // link. The peak rate above names one; an HT rate while we
        // associated VHT80 means the AP sends one class below what was
        // negotiated, which no other number shows. `rate_hist` already
        // carries the split.
        put(" ht/vht (HT ", &mut b, &mut n);
        num(ht_n, &mut b, &mut n);
        put(", VHT ", &mut b, &mut n);
        num(vht_n, &mut b, &mut n);
        put("), dazu ", &mut b, &mut n);
        num(legacy, &mut b, &mut n);
        put(" legacy (baken), zuletzt ", &mut b, &mut n);
    }
    put(rate_name(d.dm.curr_rx_rate), &mut b, &mut n);
    put(")  tx ", &mut b, &mut n);
    put(rate_name(d.dm.tx_rate), &mut b, &mut n);
    put(" (0x", &mut b, &mut n);
    rate_hex(d.dm.tx_rate, &mut b, &mut n);
    put(", ", &mut b, &mut n);
    num(ls.ra_rpt_n, &mut b, &mut n);
    put(" meldungen)  angeboten 0x", &mut b, &mut n);
    rate_hex(link.highest_rate, &mut b, &mut n);
    put("  bw ", &mut b, &mut n);
    put(match link.si.bw_mode { 0 => "20", 1 => "40", _ => "80" },
        &mut b, &mut n);
    put(" MHz", &mut b, &mut n);
    // And next to it what actually arrived. The number before is our
    // setting; this one comes from the chip's RX status. If they differ,
    // the AP runs another width than we do.
    put(" (empfangen ", &mut b, &mut n);
    for (i, name) in ["20", "40", "80", "?"].iter().enumerate() {
        if ls.bw_hist[i] == 0 {
            continue;
        }
        if i > 0 && ls.bw_hist[..i].iter().any(|&c| c != 0) {
            put(" ", &mut b, &mut n);
        }
        put(name, &mut b, &mut n);
        put(":", &mut b, &mut n);
        num(ls.bw_hist[i], &mut b, &mut n);
    }
    put(")", &mut b, &mut n);
    // What the AP itself claims it can do, from its own association
    // response (`parse_assoc_resp`). The line above says what it sends
    // with, this one what it could. If they differ, the rate is its
    // decision, not its limit; if VHT is missing here entirely, it did not
    // accept us as a VHT station, which would be on our side.
    let vhtmap = |m: u16, b: &mut [u8; REPORT_CAP], n: &mut usize| {
        let mut nss = 0u32;
        let mut top = 0u32;
        for i in 0..8u16 {
            let v = (m >> (i * 2)) & 3;
            if v == 3 {
                break;
            }
            nss += 1;
            top = 7 + v as u32;
        }
        if nss == 0 {
            put("keins", b, n);
            return;
        }
        num(nss, b, n);
        put("SS MCS0-", b, n);
        num(top, b, n);
    };
    // Our side first, so the line distinguishes "it said no" from "we never
    // asked": `build_vht_cap_ie` declines if the efuse announces anything
    // other than VHT, and then no VHT element goes out.
    put("\n  wir schickten  HT ", &mut b, &mut n);
    num(e.hw_cap_nss as u32, &mut b, &mut n);
    put("SS", &mut b, &mut n);
    // SAFETY: single-threaded; written when building the request, only read
    // here.
    match unsafe { SENT_VHT } {
        Some(cap) => {
            put(" + VHT ", &mut b, &mut n);
            num(e.hw_cap_nss as u32, &mut b, &mut n);
            put("SS MCS0-9, cap 0x", &mut b, &mut n);
            rate_hex((cap >> 24) as u8, &mut b, &mut n);
            rate_hex((cap >> 16) as u8, &mut b, &mut n);
            rate_hex((cap >> 8) as u8, &mut b, &mut n);
            rate_hex(cap as u8, &mut b, &mut n);
        }
        None if link.channel > 14 => {
            put(" + KEIN VHT-Element hinausgegangen (efuse ptcl ", &mut b, &mut n);
            num(e.hw_cap_ptcl as u32, &mut b, &mut n);
            put(")", &mut b, &mut n);
        }
        None => put(" + kein VHT (2,4 GHz)", &mut b, &mut n),
    }
    // SAFETY: as above.
    put(if unsafe { SENT_WMM } { " + WMM" } else { " + KEIN WMM (AP sagt keins an)" },
        &mut b, &mut n);
    // How wide the AP operates its cell, from the association response, not
    // the beacon. Linux reads exactly these elements:
    // `ieee80211_assoc_success` passes the response elements to
    // `ieee80211_config_bw` (mlme.c:7666), and `ieee80211_determine_ap_chan`
    // derives mode and width from them. A mismatch with the beacon shows
    // here.
    put("\n  ap betreibt  ", &mut b, &mut n);
    if caps.vht_op_seen {
        put("VHT-Op breite=", &mut b, &mut n);
        num(caps.vht_op_chanwidth as u32, &mut b, &mut n);
        put(match caps.vht_op_chanwidth {
            0 => " (HT, also 20/40!)",
            1 => " (80)",
            2 => " (160)",
            _ => " (80+80)",
        }, &mut b, &mut n);
        put(" mitte=", &mut b, &mut n);
        num(caps.vht_op_cch0 as u32, &mut b, &mut n);
        put(" basic-mcs=0x", &mut b, &mut n);
        rate_hex((caps.vht_op_basic_mcs >> 8) as u8, &mut b, &mut n);
        rate_hex(caps.vht_op_basic_mcs as u8, &mut b, &mut n);
    } else {
        put("KEIN VHT-Op in der Anmeldeantwort", &mut b, &mut n);
    }
    if caps.ht_op_seen {
        put("  HT-Op sek=", &mut b, &mut n);
        num((caps.ht_op_info & 0x3) as u32, &mut b, &mut n);
        put(if caps.ht_op_info & 0x4 != 0 { " breit" } else { " NUR 20" },
            &mut b, &mut n);
    }
    put("  (wir fahren ", &mut b, &mut n);
    put(match link.si.bw_mode { 0 => "20", 1 => "40", _ => "80" },
        &mut b, &mut n);
    put(")", &mut b, &mut n);
    put("\n  ap kann  ", &mut b, &mut n);
    let hss = caps.ht_mcs.iter().filter(|&&m| m != 0).count() as u32;
    if caps.ht_supported && hss > 0 {
        put("HT ", &mut b, &mut n);
        num(hss, &mut b, &mut n);
        put("SS MCS0-", &mut b, &mut n);
        num(hss * 8 - 1, &mut b, &mut n);
    } else {
        put("kein HT", &mut b, &mut n);
    }
    if caps.vht_supported {
        put("  VHT sendet ", &mut b, &mut n);
        vhtmap(caps.vht_tx_mcs_map, &mut b, &mut n);
        put(", empfaengt ", &mut b, &mut n);
        vhtmap(caps.vht_mcs_map, &mut b, &mut n);
    } else {
        put("  KEIN VHT in der Anmeldeantwort", &mut b, &mut n);
    }
    // The PCIe link. Reported here rather than once at start because ASPM
    // can affect throughput.
    if let Some(l) = pci::link_state() {
        put("\npcie gen", &mut b, &mut n);
        num(l.speed as u32, &mut b, &mut n);
        put(" x", &mut b, &mut n);
        num(l.width as u32, &mut b, &mut n);
        put("  aspm ", &mut b, &mut n);
        put(match l.aspm {
            0 => "aus",
            1 => "L0s",
            2 => "L1 AN",
            _ => "L0s+L1 AN",
        }, &mut b, &mut n);
        if l.aspm & 0x2 != 0 {
            put(" (austritt ", &mut b, &mut n);
            num(1u32 << l.l1_exit.min(6), &mut b, &mut n);
            put(if l.l1_exit >= 7 { "+ us)" } else { " us)" }, &mut b, &mut n);
        }
        put("  clkreq ", &mut b, &mut n);
        put(if l.clkreq { "an" } else { "aus" }, &mut b, &mut n);
    }
    put("\nbssid ", &mut b, &mut n);
    for (i, byte) in link.bssid.iter().enumerate() {
        if i > 0 && n < b.len() {
            b[n] = b':';
            n += 1;
        }
        if n + 2 <= b.len() {
            b[n] = hex[(byte >> 4) as usize];
            b[n + 1] = hex[(byte & 0xf) as usize];
            n += 2;
        }
    }
    put("\ndaten rein/raus ", &mut b, &mut n);
    num(ls.data_rx, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(ls.data_tx, &mut b, &mut n);
    put("  wiederholt ", &mut b, &mut n);
    num(ls.retry_rx, &mut b, &mut n);
    put(" (duplikate ", &mut b, &mut n);
    num(ls.dup_rx, &mut b, &mut n);
    put(")", &mut b, &mut n);
    put("  umsortiert ", &mut b, &mut n);
    num(ls.ro_sorted, &mut b, &mut n);
    put(" (zu spaet ", &mut b, &mut n);
    num(ls.ro_old, &mut b, &mut n);
    put(", Frist ", &mut b, &mut n);
    num(ls.ro_timeout, &mut b, &mut n);
    put(", Pool voll ", &mut b, &mut n);
    num(ls.ro_full, &mut b, &mut n);
    put(")", &mut b, &mut n);
    put("  a-msdu ", &mut b, &mut n);
    num(ls.amsdu_rx, &mut b, &mut n);
    put(" (", &mut b, &mut n);
    num(ls.amsdu_sub, &mut b, &mut n);
    put(" teile, ", &mut b, &mut n);
    num(ls.amsdu_bad, &mut b, &mut n);
    put(" verworfen)", &mut b, &mut n);
    put("  eapol ", &mut b, &mut n);
    num(ls.eapol_rx, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(ls.eapol_tx, &mut b, &mut n);
    put("  schluessel ", &mut b, &mut n);
    num(ls.keys_set, &mut b, &mut n);
    // Link watchdog and roaming: a probe followed by a recovery is a case in
    // which the link would otherwise have been dropped.
    if link.roam.scans > 0 || link.roam.roams > 0 {
        put("  roaming ", &mut b, &mut n);
        num(link.roam.scans, &mut b, &mut n);
        put("x umgehoert, ", &mut b, &mut n);
        num(link.roam.roams, &mut b, &mut n);
        put("x gewechselt (pegel ", &mut b, &mut n);
        let sg = link.roam.dbm();
        if sg < 0 {
            put("-", &mut b, &mut n);
            num((-(sg as i32)) as u32, &mut b, &mut n);
        } else {
            num(sg as u32, &mut b, &mut n);
        }
        put(" dBm geglaettet)", &mut b, &mut n);
    }
    if ls.csa_done > 0 || ls.csa_back > 0 {
        put("  kanalwechsel ", &mut b, &mut n);
        num(ls.csa_done, &mut b, &mut n);
        put("x gefolgt, ", &mut b, &mut n);
        num(ls.csa_back, &mut b, &mut n);
        put("x umgekehrt (dort war niemand)", &mut b, &mut n);
    }
    if ls.poll_started > 0 {
        put("  wache ", &mut b, &mut n);
        num(ls.poll_started, &mut b, &mut n);
        put("x angestupst, ", &mut b, &mut n);
        num(ls.poll_recovered, &mut b, &mut n);
        put("x kam er zurueck", &mut b, &mut n);
        if ls.poll_on {
            put(" (laeuft gerade, versuch ", &mut b, &mut n);
            num(ls.probe_send_count, &mut b, &mut n);
            put(")", &mut b, &mut n);
        }
    }
    put("  rx-wachhund ", &mut b, &mut n);
    num(ls.rx_wd, &mut b, &mut n);
    put("\nrx-schleife ", &mut b, &mut n);
    num(ls.rx_polls, &mut b, &mut n);
    put(" blicke mit beute, ", &mut b, &mut n);
    num(ls.rx_empty, &mut b, &mut n);
    put(" leer, ", &mut b, &mut n);
    // Frames per poll with one decimal, in integers.
    let zehntel = if ls.rx_polls > 0 {
        ls.rx_frames.saturating_mul(10) / ls.rx_polls
    } else {
        0
    };
    num(zehntel / 10, &mut b, &mut n);
    put(",", &mut b, &mut n);
    num(zehntel % 10, &mut b, &mut n);
    put(" rahmen/blick, ", &mut b, &mut n);
    num(ls.rx_full, &mut b, &mut n);
    put(" volle stapel, ", &mut b, &mut n);
    // Load in percent: time in the ring against loop time.
    let lauf = host::now_us().wrapping_sub(ls.pump_us0).max(1);
    num(((ls.rx_us.saturating_mul(100)) / lauf) as u32, &mut b, &mut n);
    put(" % der zeit im empfangspfad (", &mut b, &mut n);
    let je = if ls.rx_frames > 0 { ls.rx_us / ls.rx_frames as u64 } else { 0 };
    num(je as u32, &mut b, &mut n);
    put(" us je rahmen)", &mut b, &mut n);
    // Whether the AP hears us: the TX report counters. A sent count alone
    // only says how many frames went into a ring.
    put("\nsendequittung ", &mut b, &mut n);
    num(ls.tx_acked, &mut b, &mut n);
    put(" ok, ", &mut b, &mut n);
    num(ls.tx_lost, &mut b, &mut n);
    put(" ohne ACK, ", &mut b, &mut n);
    num(ls.tx_no_report, &mut b, &mut n);
    put(" ohne bericht", &mut b, &mut n);
    for (id, cnt) in d_c2h(ls) {
        put("  c2h 0x", &mut b, &mut n);
        if n + 2 <= b.len() {
            b[n] = hex[(id >> 4) as usize];
            b[n + 1] = hex[(id & 0xf) as usize];
            n += 2;
        }
        put(" ", &mut b, &mut n);
        put(fw::c2h_name(id), &mut b, &mut n);
        put(" x", &mut b, &mut n);
        num(cnt, &mut b, &mut n);
    }
    if ls.fw_crash > 0 {
        put("  FIRMWARE-ABSTURZ ", &mut b, &mut n);
        num(ls.fw_crash, &mut b, &mut n);
    }
    // Rekeys and disconnects. A rekey runs minutes after the handshake and
    // leaves no other trace.
    put("\nneuschluessel ", &mut b, &mut n);
    num(ls.rekey_rx, &mut b, &mut n);
    put(" empfangen, ", &mut b, &mut n);
    num(ls.rekey_tx, &mut b, &mut n);
    put(" beantwortet  gtk ", &mut b, &mut n);
    num(ls.gtk_set, &mut b, &mut n);
    put("  rauswurf ", &mut b, &mut n);
    num(ls.kicked, &mut b, &mut n);
    put("  neuverbunden ", &mut b, &mut n);
    num(ls.reconnects, &mut b, &mut n);
    // Management frames by subtype, including whether the AP attempts to set
    // up aggregation (ADDBA requests).
    put("\nmgmt beacon ", &mut b, &mut n);
    num(ls.mgmt_sub[8], &mut b, &mut n);
    put("  action ", &mut b, &mut n);
    num(ls.mgmt_sub[13], &mut b, &mut n);
    put(" (zuletzt kat ", &mut b, &mut n);
    num(ls.last_action.0 as u32, &mut b, &mut n);
    put("/akt ", &mut b, &mut n);
    num(ls.last_action.1 as u32, &mut b, &mut n);
    put(")  ADDBA ", &mut b, &mut n);
    num(ls.addba_req, &mut b, &mut n);
    put(" erbeten, ", &mut b, &mut n);
    num(ls.addba_resp, &mut b, &mut n);
    put(" angenommen", &mut b, &mut n);
    if ls.addba_resp > 0 {
        put(" (fenster ", &mut b, &mut n);
        num(ls.addba_win as u32, &mut b, &mut n);
        put(" von ", &mut b, &mut n);
        num(ls.addba_win_req as u32, &mut b, &mut n);
        put(" erbetenen)", &mut b, &mut n);
    } else if ls.addba_req > 0 {
        put(" — AGGREGATION AUS (`ampdu: off`)", &mut b, &mut n);
    }
    // Aggregation in both directions. The aggregate size in the receive
    // direction, counted by hardware, comes first because it is the larger
    // one when downloading. Next the transmit session and how much was in
    // the ring per kick: the session says what the hardware may aggregate,
    // the ring depth what it can. A one there means the bottleneck is not
    // the session but that never more than one frame is queued.
    if ls.rx_ppdu_n > 0 {
        put("\nempfangsstapel ", &mut b, &mut n);
        num(ls.rx_data_ppdu_frames / ls.rx_ppdu_n, &mut b, &mut n);
        put(" rahmen je sendevorgang des AP (", &mut b, &mut n);
        num(ls.rx_data_ppdu_frames, &mut b, &mut n);
        put(" in ", &mut b, &mut n);
        num(ls.rx_ppdu_n, &mut b, &mut n);
        put(" ppdus)", &mut b, &mut n);
        // And the time in between. The smallest interval is what the link
        // can do, the mean what it does; together they show whether the air
        // is the limit, without any estimated constant.
        if ls.rx_gap_n > 0 {
            put("\n  abstand ", &mut b, &mut n);
            num((ls.rx_gap_sum / ls.rx_gap_n as u64) as u32, &mut b, &mut n);
            put(" us im mittel, kleinster ", &mut b, &mut n);
            num(ls.rx_gap_min, &mut b, &mut n);
            put(" us (", &mut b, &mut n);
            num(ls.rx_gap_n, &mut b, &mut n);
            put(" gemessen, nur bei Verkehr)", &mut b, &mut n);
            // The distribution is the actual finding.
            put("\n  verteilt  <0,5ms ", &mut b, &mut n);
            for (i, name) in ["", "<2ms ", "<5ms ", "<10ms ", ">=10ms "]
                .iter().enumerate()
            {
                if i > 0 {
                    put("  ", &mut b, &mut n);
                    put(name, &mut b, &mut n);
                }
                num(ls.rx_gap_buckets[i], &mut b, &mut n);
            }
            if ls.rx_gap_buckets[4] > 0 {
                put(" (zusammen ", &mut b, &mut n);
                num((ls.rx_gap_big_sum / 1000) as u32, &mut b, &mut n);
                put(" ms STILLSTAND)", &mut b, &mut n);
            }
            if ls.rx_gap_idle > 0 {
                put("  ·  ", &mut b, &mut n);
                num(ls.rx_gap_idle, &mut b, &mut n);
                put(" x kein verkehr", &mut b, &mut n);
            }
        }
    }
    // Our own stack's turnaround, from "data to the kernel" to "frame back
    // from the kernel". When downloading this is the time we take for the
    // TCP ACK, which adds directly to the RTT the server measures.
    if ls.turn_n > 0 {
        put("\nstapelumkehr ", &mut b, &mut n);
        num((ls.turn_sum / ls.turn_n as u64) as u32, &mut b, &mut n);
        put(" us im mittel, groesste ", &mut b, &mut n);
        num(ls.turn_max as u32, &mut b, &mut n);
        put(" us (", &mut b, &mut n);
        num(ls.turn_n, &mut b, &mut n);
        put(" gemessen)", &mut b, &mut n);
        // If the two right buckets hold about as many cases as `>=10ms`
        // above, the AP is waiting for us: the pause on air and the pause in
        // the stack are the same event. If they are zero, the stall comes
        // from elsewhere.
        put("\n  verteilt  <0,2ms ", &mut b, &mut n);
        for (i, name) in ["", "<1ms ", "<5ms ", "<20ms ", ">=20ms "]
            .iter().enumerate()
        {
            if i > 0 {
                put("  ", &mut b, &mut n);
                put(name, &mut b, &mut n);
            }
            num(ls.turn_buckets[i], &mut b, &mut n);
        }
        if ls.turn_big_sum > 0 {
            put(" (zusammen ", &mut b, &mut n);
            num((ls.turn_big_sum / 1000) as u32, &mut b, &mut n);
            put(" ms)", &mut b, &mut n);
        }
    }
    if ls.tx_batch_n > 0 {
        // Two numbers, only the second decides. `eingelegt` is what the
        // driver queued in one pass; `im ring` is what the hardware still had
        // queued at that moment, and that is what gets aggregated. They
        // differ once the medium is busy.
        put("\nsendering ", &mut b, &mut n);
        num(ls.tx_ring_sum / ls.tx_batch_n, &mut b, &mut n);
        put(" deskriptoren beim anstoss im mittel, groesster ",
            &mut b, &mut n);
        num(ls.tx_ring_max, &mut b, &mut n);
        put(" (eingelegt ", &mut b, &mut n);
        num(ls.tx_batch_sum / ls.tx_batch_n, &mut b, &mut n);
        put(" je durchlauf, groesster ", &mut b, &mut n);
        num(ls.tx_batch_max, &mut b, &mut n);
        put(", ", &mut b, &mut n);
        num(ls.tx_batch_n, &mut b, &mut n);
        put(" anstoesse)", &mut b, &mut n);
    }
    put("\n  BA SENDEN ", &mut b, &mut n);
    match link.ba_tx.state {
        BaState::Aus => put("aus (txagg)", &mut b, &mut n),
        BaState::Gefragt => {
            put("gefragt, keine Antwort (", &mut b, &mut n);
            num(link.ba_tx.tries, &mut b, &mut n);
            put(" x)", &mut b, &mut n);
        }
        BaState::Laeuft => {
            put("LAEUFT, fenster ", &mut b, &mut n);
            num(link.ba_tx.win as u32, &mut b, &mut n);
            put(", max_agg ", &mut b, &mut n);
            num(link.ba_tx.factor as u32, &mut b, &mut n);
            put(", abstand ", &mut b, &mut n);
            num(link.ba_tx.density as u32, &mut b, &mut n);
        }
        BaState::Aufgegeben => {
            put("abgelehnt (status ", &mut b, &mut n);
            num(link.ba_tx.status as u32, &mut b, &mut n);
            put(", ", &mut b, &mut n);
            num(ls.addba_tx, &mut b, &mut n);
            put(" fragen)", &mut b, &mut n);
        }
    }
    if ls.addba_drop > 0 {
        put(", ", &mut b, &mut n);
        num(ls.addba_drop, &mut b, &mut n);
        put(" NICHT GESEHEN (Puffer)", &mut b, &mut n);
    }
    if ls.addba_fail > 0 {
        put(", ", &mut b, &mut n);
        num(ls.addba_fail, &mut b, &mut n);
        put(" NICHT GESENDET", &mut b, &mut n);
    }
    let sonst: u32 = ls.mgmt_sub.iter().enumerate()
        .filter(|(i, _)| *i != 8 && *i != 13)
        .map(|(_, v)| *v).sum();
    put("  sonst ", &mut b, &mut n);
    num(sonst, &mut b, &mut n);
    if ls.kicked > 0 {
        put(" (zuletzt Grund ", &mut b, &mut n);
        num(ls.last_reason as u32, &mut b, &mut n);
        put(": ", &mut b, &mut n);
        put(reason_name(ls.last_reason), &mut b, &mut n);
        put(")", &mut b, &mut n);
    }
    // Proof that the watchdog runs: tick, gain control, crystal (against the
    // efuse value) and temperature must move. Crystal at the efuse value
    // with `bt` on means tracking is held off by the coexistence lock, which
    // is Linux' own rule.
    put("\nwatchdog ", &mut b, &mut n);
    num(d.watch_dog_cnt, &mut b, &mut n);
    // SAFETY: single-threaded; the pump thread writes, only read here.
    let (wd, it, it_wd) = unsafe {
        (*core::ptr::addr_of!(WD_MAX), ITER_MAX, ITER_MAX_WD)
    };
    put(" (laengste runde ", &mut b, &mut n);
    num(it, &mut b, &mut n);
    put(if it_wd { " us MIT watchdog; teile max" } else { " us ohne watchdog; teile max" },
        &mut b, &mut n);
    for (i, name) in WD_NAMES.iter().enumerate() {
        put(" ", &mut b, &mut n);
        put(name, &mut b, &mut n);
        put(" ", &mut b, &mut n);
        num(wd[i], &mut b, &mut n);
    }
    put(" us)", &mut b, &mut n);
    put("  igi 0x", &mut b, &mut n);
    if n + 2 <= b.len() {
        b[n] = hex[(d.dm.igi_history[0] >> 4) as usize];
        b[n + 1] = hex[(d.dm.igi_history[0] & 0xf) as usize];
        n += 2;
    }
    put("  fehlalarm ", &mut b, &mut n);
    num(d.dm.total_fa_cnt, &mut b, &mut n);
    // The state of the air in one number. A high share means the AP keeps
    // retransmitting, and then the link is the limit, not the driver.
    put("  crc ht ", &mut b, &mut n);
    num(ls.ht_err as u32, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num((ls.ht_ok + ls.ht_err) as u32, &mut b, &mut n);
    let anteil = if ls.ht_ok + ls.ht_err > 0 {
        ls.ht_err * 100 / (ls.ht_ok + ls.ht_err)
    } else {
        0
    };
    put(" (", &mut b, &mut n);
    num(anteil as u32, &mut b, &mut n);
    put(" %)  ofdm ", &mut b, &mut n);
    num(ls.ofdm_err as u32, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num((ls.ofdm_ok + ls.ofdm_err) as u32, &mut b, &mut n);
    put("  rssi ", &mut b, &mut n);
    num(d.dm.min_rssi as u32, &mut b, &mut n);
    // Why the peer picks what it picks. The SNR per path is in every RX
    // descriptor (`query_phy_status_page1`) and shows whether a low rate is
    // justified.
    put("  snr ", &mut b, &mut n);
    num(d.dm.rx_snr[0].max(0) as u32, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.dm.rx_snr[1].max(0) as u32, &mut b, &mut n);
    // Two numbers that explain each other. The crystal value alone says
    // nothing; only its distance from the efuse shows whether tracking does
    // anything. At the efuse value with `bt` off it ran and found nothing to
    // correct; with `bt` on it is held off.
    put("  quarz ", &mut b, &mut n);
    num(d.dm.cfo_track.crystal_cap as u32, &mut b, &mut n);
    put(" (efuse ", &mut b, &mut n);
    num(e.crystal_cap as u32, &mut b, &mut n);
    put(")", &mut b, &mut n);
    put("  thermo ", &mut b, &mut n);
    num(d.dm.thermal_avg[0] as u32, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.dm.thermal_avg[1] as u32, &mut b, &mut n);
    put("  txidx ", &mut b, &mut n);
    num(d.dm.delta_power_index[0] as i32 as u32, &mut b, &mut n);
    put("  bt ", &mut b, &mut n);
    put(if d.cx.bt_disabled { "aus" } else { "AN (Quarz fest)" },
        &mut b, &mut n);
    // The peak next to it. The smoothed value drops to zero within seconds
    // after a transfer, so the peak is what stays comparable afterwards.
    put("  tp ", &mut b, &mut n);
    num(d.stats.tx_throughput, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.stats.rx_throughput, &mut b, &mut n);
    put(" Mbit (spitze ", &mut b, &mut n);
    num(d.stats.tx_peak, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.stats.rx_peak, &mut b, &mut n);
    put(")\n", &mut b, &mut n);
    // A truncated report must say so; otherwise a cut line reads as a
    // complete one.
    if n == b.len() {
        const MARKE: &[u8] = b"\n*** BERICHT ABGESCHNITTEN ***";
        let a = b.len() - MARKE.len();
        b[a..].copy_from_slice(MARKE);
    }
    host::driver_report(&b[..n]);
}
