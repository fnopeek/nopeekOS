//! wifi_rtl8822ce — Realtek RTL8822CE (Wi-Fi 5, 2T2R, PCIe), WASM-Treiber.
//!
//! Strikte 1:1-Portierung von Linux 6.18.26,
//! `drivers/net/wireless/realtek/rtw88/` (Modul `rtw_8822ce`).
//! Plan: `docs/plan/WIFI_RTL8822CE.md` · Karte:
//! `docs/plan/WIFI_RTL8822CE_LINUX_MAP.md`.
//!
//! **Stufe 0: die Tuer.** PCI binden, Bus-Master, **BAR2**
//! abbilden (pci.c `rtw_pci_io_mapping`: `u8 bar_id = 2` — nicht BAR0), und
//! `REG_SYS_CFG1` lesen wie `rtw_chip_parameter_setup` es tut.
//!
//! **Stufe 0 SCHREIBT NICHTS.** Kein Register wird angefasst, keine
//! Power-Sequenz gefahren. Was hier schiefgeht, kann also nicht an einem
//! Schreibzugriff von uns liegen — und das ist der ganze Sinn einer ersten
//! Stufe, die nur eine Frage stellt.
//!
//! **Die Stufen 0-2a sind Diagnose und KEHREN ZURUECK.** Ein Treiber, der
//! nicht endet, haelt das Terminal, aus dem er gestartet wurde
//! (`spawn_on_worker` setzt `APP_RUNNING`) — und ein Lauf, nach dem man nicht
//! weiterarbeiten kann, ist beim Suchen schlimmer als kein Lauf. Der Chip
//! bleibt dabei AUS zurueck, und der Kernel gibt beim Ende alles DMA frei.
//! Erst wenn die Firmware laeuft und Frames fliessen (ab 2b), wird daraus ein
//! Treiber, der bleiben muss — und dann ist der Startweg die Frage, nicht das
//! Modul.
//!
//! **Ab 2b kommt eine zweite Pflicht dazu, und die stand schon im AX200:**
//! „the kernel frees our DMA buffers on return and a still-running firmware
//! must not DMA into them afterwards". Solange nur der MAC an- und wieder
//! ausgeht, ist das erledigt; sobald eine Firmware laeuft, muss sie VOR dem
//! Zurueckkehren angehalten werden.
//!
//! Gate: `chip_version` plausibel, RF-Typ 2T2R, und der frische 8-Bit-Pfad
//! (`npk_mmio_read8`) liefert byteweise dasselbe wie der 32-Bit-Pfad.
//!
//! **Stufe 1: der Strom.** `rtw_mac_power_on` vollstaendig (`mac.rs`), mit
//! den vier Power-Sequenz-Tabellen aus `pwrseq.rs` — erzeugt aus der
//! C-Quelle, nicht abgetippt. Noch keine Firmware, noch keine Ringe.
//! Gate in BEIDE Richtungen: `REG_CR` verlaesst `0xea` beim Einschalten und
//! kehrt beim Abschalten dorthin zurueck. Nur eine Richtung zu messen hiesse,
//! einen Zustand zu pruefen, den der Chip vielleicht schon hatte.
//!
//! **Stufe 2b: die Firmware.** `rtw_download_firmware` vollstaendig
//! (`mac.rs` + `fw.rs` + `tx.rs`), 202600 Bytes ueber die BCN-Queue und
//! DDMA nach dmem/imem/emem. Gate: `REG_MCUFW_CTRL` liest `FW_READY`.
//!
//! **Stufe 2a: die Ringe.** `rtw_pci_init_trx_ring` + `rtw_pci_reset_buf_desc`
//! (`pci.rs`), in Linux' Reihenfolge — die Ringregister werden programmiert,
//! BEVOR der MAC angeht (`rtw_power_on` ruft `rtw_hci_setup` vor
//! `rtw_mac_power_on`). Gate: jedes Adress- und Anzahlregister gibt zurueck,
//! was hineingeschrieben wurde, einmal mit MAC aus und einmal mit MAC an.

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
mod rx;
mod sec;
mod tables;
mod tx;
mod txpower;
mod regs;
use regs::*;

#[unsafe(link_section = ".npk.app_meta")]
#[used]
static APP_META_BYTES: [u8; include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin")).len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/app_meta.bin"));

/// Nie still sterben: ein `loop {}` ohne Meldung sieht von aussen aus wie
/// „der Chip antwortet nicht" und hat beim AX200 einen Abend gekostet.
/// `Location` ueberlebt `strip = true`, weil es statische Daten sind.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
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

/// Eine Quelle fuer die Version — Banner und Bericht koennen nicht
/// auseinanderlaufen.
const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// rtw8822c.c `.fw_name = "rtw88/rtw8822c_fw.bin"` — mitgeliefert wie der
/// AX200-Blob. Version 9.9.15, und `check_firmware_size` rechnet den Kopf
/// gegen die Dateilaenge nach, bevor ein Byte an den Chip geht.
static FW: &[u8] = include_bytes!("../firmware/rtw8822c_fw.bin");

/// `hal.current_band_type` ist beim Download noch 0 — gesetzt wird es erst
/// in `rtw_set_channel`, also lange danach. Das entscheidet in
/// `rtw_tx_pkt_info_update_rate`, welcher Zweig gilt, und wir nehmen
/// denselben wie Linux.
const BAND_AT_FWDL: u8 = 0;

/// Ergebnis von `rtw_chip_parameter_setup` (main.c:1876-1900).
struct Hal {
    chip_version: u32,
    cut_version: u8,
    mp_chip: u8,
    vendor_id: u8,
    rf_2t2r: bool,
    rf_path_num: u8,
    /// main.c:1884-1893 — bei 2T2R beide `BB_PATH_AB`, sonst `BB_PATH_A`.
    antenna_tx: u8,
    antenna_rx: u8,
    /// main.c:2183 die Vorgabe, main.c:1903 das ODER mit `BIT_VHT_DACK`.
    /// `rtw_core_start` schreibt das ins Register, NACHDEM `mac_init` dort
    /// `WLAN_RCR_CFG` hinterlassen hat — „rcr reset after powered on".
    rcr: u32,
}

/// main.c `rtw_chip_parameter_setup` — der Teil, der aus EINEM Register
/// liest. Der Rest der Funktion setzt nur Felder aus `chip`.
fn chip_parameter_setup(h: i32) -> Hal {
    let chip_version = host::r32(h, REG_SYS_CFG1);
    let rf_2t2r = chip_version & BIT_RF_TYPE_ID != 0;
    Hal {
        chip_version,
        cut_version: bit_get_chip_ver(chip_version),
        // main.c:1883 — gesetztes BIT_RTL_ID heisst NICHT mp_chip.
        mp_chip: if chip_version & BIT_RTL_ID != 0 { 0 } else { 1 },
        vendor_id: bit_get_vendor_id(chip_version),
        rf_2t2r,
        rf_path_num: if rf_2t2r { 2 } else { 1 },
        antenna_tx: if rf_2t2r { BB_PATH_AB } else { BB_PATH_A },
        antenna_rx: if rf_2t2r { BB_PATH_AB } else { BB_PATH_A },
        // main.c:2183 „default rx filter setting" plus main.c:1903.
        rcr: BIT_APP_FCS | BIT_APP_MIC | BIT_APP_ICV | BIT_PKTCTL_DLEN
            | BIT_HTC_LOC_CTRL | BIT_APP_PHYSTS | BIT_AB | BIT_AM | BIT_APM
            | BIT_VHT_DACK,
    }
}

/// Der 8-Bit-Pfad ist neu im Kernel. Bevor irgendetwas darauf aufbaut, wird
/// er GEGEN den bewaehrten 32-Bit-Pfad gehalten: vier Bytes einzeln gelesen
/// muessen dasselbe Wort ergeben. Dasselbe fuer 16 Bit.
///
/// Das ist billig und es ist read-only — und es beantwortet in einer Zeile
/// die Frage, die sonst erst in Stufe 1 mitten in der Power-Sequenz auffaellt,
/// wo zehn andere Dinge gleichzeitig neu sind.
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
    host::print("[rtl8822ce] Realtek RTL8822CE (rtw88) v");
    host::print(DRIVER_VERSION);
    host::print(" — Stufe 0: binden, BAR2, Chipkennung\n");

    // ── PCI binden ───────────────────────────────────────────────
    // rtw8822ce.c fuehrt zwei Geraete-IDs fuer denselben Chip.
    let mut dev = RTL8822CE_DEVICE;
    let mut rc = host::pci_bind(RTL_VENDOR, dev);
    if rc != 0 {
        dev = RTL8822CE_DEVICE_ALT;
        rc = host::pci_bind(RTL_VENDOR, dev);
    }
    if rc != 0 {
        host::print("[rtl8822ce] PCI-Bind fehlgeschlagen (");
        match rc {
            -1 => host::print("nicht gefunden"),
            -2 => host::print("abgelehnt"),
            _ => host::print("unbekannter Fehler"),
        }
        host::print(") — erwartet 10ec:c822 oder 10ec:c82f\n");
        return;
    }
    host::print("[rtl8822ce] gebunden: 10ec:");
    host::print_hex16(dev);
    host::print("\n");

    // Rueckgabewert lesen, nicht wegwerfen: ohne Busmaster kann der Chip
    // keinen Deskriptor aus dem Hauptspeicher holen, und das sieht dann aus
    // wie ein Fehler im Treiber statt wie eine fehlende Erlaubnis.
    let bm = host::pci_enable_bus_master();
    if bm != 0 {
        host::print("[rtl8822ce] Bus-Master konnte nicht eingeschaltet werden\n");
    }
    fw::dump_pci_cmd("nach bind");

    // ── BAR2 abbilden ────────────────────────────────────────────
    let h = host::mmio_map_bar(BAR_REG, BAR_PAGES);
    if h < 0 {
        host::print("[rtl8822ce] BAR2 nicht abbildbar — Stufe 0 endet hier\n");
        return;
    }
    host::print("[rtl8822ce] BAR2 abgebildet (handle ");
    host::print_dec(h as u32);
    host::print(", 64 KiB)\n");

    // ── Kennung lesen (rtw_chip_parameter_setup) ─────────────────
    let hal = chip_parameter_setup(h);

    // Ein Fenster, das nur Einsen liefert, ist keine Antwort des Chips,
    // sondern die Antwort des Busses auf eine Adresse, an der niemand ist.
    let dead = hal.chip_version == 0xFFFF_FFFF || hal.chip_version == 0;

    host::print("[rtl8822ce] Register:\n");
    host::log_reg32("SYS_CFG1 ", hal.chip_version);
    host::log_reg32("SYS_CFG2 ", host::r32(h, REG_SYS_CFG2));
    host::log_reg32("SYS_PWCTL", host::r32(h, REG_SYS_PW_CTRL));
    host::log_reg32("SYS_STAT1", host::r32(h, REG_SYS_STATUS1));

    // Die zwei Werte, die Linux SELBST als Klartext vergleicht — und der
    // erste ist ausdruecklich ein 8-Bit-Lesezugriff (mac.c
    // `rtw_mac_power_switch`: `rtw_read8(rtwdev, REG_CR) == 0xea`).
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

    // ── Auswertung ───────────────────────────────────────────────
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
        host::print("[rtl8822ce] Stufe 0: NEIN — Stufe 1 und 2a werden nicht gefahren\n");
        return;
    }
    host::print("[rtl8822ce] Stufe 0: GRUEN\n");

    // ── Stufe 2a: die Ringe (rtw_pci_setup_resource) ─────────────
    // Die Reihenfolge ist Linux': rtw_power_on ruft rtw_hci_setup — und
    // damit rtw_pci_setup — VOR rtw_mac_power_on. Die Ringregister liegen
    // im PCIe-Block und leben unabhaengig vom MAC.
    let mut trx = match pci::init_trx_ring() {
        Some(t) => t,
        None => {
            host::print("[rtl8822ce] DMA reicht nicht fuer die Ringe — Stufe 2a aus\n");
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

    // ── Stufe 1: der Strom ───────────────────────────────────────
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
        host::print(match e {
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

    // Dieselbe Pruefung mit laufendem MAC. Linux programmiert die Ringe
    // nach dem Firmware-Download NOCH EINMAL (`rtw_hci_setup` in
    // `__rtw_download_firmware`, Kommentar: „reset desc and index") — also
    // ist die Frage, ob dazwischen etwas verlorengeht, berechtigt und
    // billig zu beantworten.
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

    // ── Stufe 2b: die Firmware (rtw_download_firmware) ───────────
    // Reihenfolge wie `rtw_power_on`: hci_setup, mac_power_on, DANN der
    // Download. Vorher gibt es keinen laufenden MAC, durch dessen BCN-Queue
    // die Seiten gehen koennten.
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

    // `rtwdev->fifo` lebt in Linux ueber den ganzen Treiber und ist bis zum
    // ersten `rtw_mac_init` NULL. Der Download liest daraus `rsvd_boundary`
    // — hier also noch 0, genau wie dort.
    let mut fifo = mac::Fifo::default();

    let stage_buf = host::dma_alloc_below(
        (pci::RSVD_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    // Der H2C-Ring braucht einen EIGENEN Zwischenpuffer: der oben ist fuer
    // den Firmware-Download gedacht und genau ein Stueck gross.
    let h2c_buf = host::dma_alloc_below(
        (pci::H2C_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
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

    // ── Stufe 2c: efuse und hw_feature ───────────────────────────
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

            // main.c: is_valid_ether_addr — nicht null, nicht multicast.
            let valid = e.addr != [0u8; 6]
                && e.addr != [0xffu8; 6]
                && e.addr[0] & 0x01 == 0;
            stage2c = gate("MAC-Adresse aus der efuse ist gueltig", valid);
            efuse = Some(e);
        } else {
            let _ = gate("MAC-Adresse aus der efuse ist gueltig", false);
        }
    }

    // Wie Linux es in rtw_chip_efuse_info_setup tut: wieder ausschalten.
    // Ab hier ist das PFLICHT und nicht Kosmetik — eine laufende Firmware
    // darf nicht mehr in Puffer schreiben, die der Kernel beim Zurueckkehren
    // freigibt (dieselbe Begruendung steht am Ende von wifi_ax200).
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
    host::print(if stage1 {
        "[rtl8822ce] Stufe 1: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 1: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage2a {
        "[rtl8822ce] Stufe 2a: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 2a: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage2b {
        "[rtl8822ce] Stufe 2b: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 2b: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage2c {
        "[rtl8822ce] Stufe 2c: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 2c: NEIN — nicht weiterbauen, bevor das steht\n"
    });

    // ── Stufe 4b: rtw_chip_board_info_setup ──────────────────────
    // Sie steht VOR Stufe 3, weil sie in Linux vor `rtw_power_on` steht:
    // `rtw_chip_info_setup` = parameter_setup -> efuse_info_setup ->
    // board_info_setup. Die Nummer 4b ist die Reihenfolge, in der wir
    // gebaut haben, nicht die, in der gelaufen wird.
    let (stage4b, _txpwr) = match efuse.as_ref() {
        Some(e) if stage2c => stage4b_board_info_setup(e.rfe_option),
        _ => {
            host::print("[rtl8822ce] Stufe 4b: uebersprungen, 2c steht nicht\n");
            (false, None)
        }
    };

    // ── Stufe 3a: rtw_power_on, bis rtw_mac_init ─────────────────
    //
    // Alles davor war `rtw_chip_info_setup` — in Linux die Probe-Zeit, die
    // den Chip anschaltet, NUR um die efuse zu lesen, und ihn danach wieder
    // ausschaltet. Das hier ist der ZWEITE Zyklus, `rtw_power_on`
    // (main.c:1374), und er faengt wieder ganz vorne an:
    //
    //     rtw_hci_setup -> rtw_mac_power_on -> rtw_download_firmware
    //                   -> rtw_mac_init
    //
    // Die Firmware wird also ein zweites Mal geladen. Das ist keine
    // Verschwendung aus Unachtsamkeit, sondern was Linux tut: zwischen den
    // Zyklen war der MAC aus, und ein ausgeschalteter MAC hat keine
    // Firmware mehr.
    let mut stage3b = false;
    let mut stage3c = false;
    let stage3a = match (stage2c, efuse.as_ref()) {
        (true, Some(e)) => {
            host::print("[rtl8822ce] Stufe 3a: rtw_power_on (zweiter Zyklus) + rtw_mac_init\n");
            let ok = power_on_and_mac_init(h, &hal, &mut trx, stage_buf, &mut fifo);
            if ok {
                let (b, c) = phy_set_param_and_check(h, &hal, e);
                stage3b = b;
                stage3c = c;
            } else {
                host::print("[rtl8822ce] Stufe 3b/3c: uebersprungen, 3a steht nicht\n");
            }
            ok
        }
        _ => {
            host::print("[rtl8822ce] Stufe 3a: uebersprungen, 2c steht nicht\n");
            false
        }
    };

    // ── Stufe 4a: der Rest von rtw_power_on und rtw_core_start ───
    let stage4a = match (stage3c, efuse.as_ref()) {
        (true, Some(e)) => stage4a_power_on_tail(h, &hal, &mut trx, h2c_buf,
                                                 &mut fifo, e),
        _ => {
            host::print("[rtl8822ce] Stufe 4a: uebersprungen, 3c steht nicht\n");
            false
        }
    };

    // ── Stufe 4c: rtw_set_channel ────────────────────────────────
    let stage4c = match (stage4a && stage4b, efuse.as_ref(), _txpwr.as_ref()) {
        (true, Some(e), Some(t)) => stage4c_set_channel(h, &hal, e, t),
        _ => {
            host::print("[rtl8822ce] Stufe 4c: uebersprungen, 4a/4b stehen nicht\n");
            false
        }
    };

    // ── Stufe 5a: der Empfangsweg ────────────────────────────────
    let stage5a = if stage4c {
        stage5a_rx(h, &hal, &mut trx)
    } else {
        host::print("[rtl8822ce] Stufe 5a: uebersprungen, 4c steht nicht\n");
        false
    };

    // Und wieder aus, aus demselben Grund wie oben: der Kernel gibt gleich
    // die DMA-Puffer frei, in die eine laufende Firmware sonst weiterschriebe.
    mac::mac_power_off(h, hal.cut_version);

    host::print(if stage3a {
        "[rtl8822ce] Stufe 3a: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 3a: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage3b {
        "[rtl8822ce] Stufe 3b: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 3b: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage3c {
        "[rtl8822ce] Stufe 3c: GRUEN — BB und RF stehen\n"
    } else {
        "[rtl8822ce] Stufe 3c: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage4a {
        "[rtl8822ce] Stufe 4a: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 4a: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage4b {
        "[rtl8822ce] Stufe 4b: GRUEN\n"
    } else {
        "[rtl8822ce] Stufe 4b: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage4c {
        "[rtl8822ce] Stufe 4c: GRUEN — DER EMPFAENGER HOERT\n"
    } else {
        "[rtl8822ce] Stufe 4c: NEIN — nicht weiterbauen, bevor das steht\n"
    });
    host::print(if stage5a {
        "[rtl8822ce] Stufe 5a: GRUEN — DIE PAKETE KOMMEN AN\n"
    } else {
        "[rtl8822ce] Stufe 5a: NEIN — nicht weiterbauen, bevor das steht\n"
    });


    // Zurueckkehren, nicht schlafen. Der Kernel raeumt danach auf: DMA
    // freigeben, PCI loesen, und den Bericht loeschen — letzteres mit der
    // ausdruecklichen Begruendung, dass die Zahlen eines toten Treibers nicht
    // wie lebende aussehen duerfen. Das Ergebnis dieser Stufen steht deshalb
    // HIER im Terminal und nicht in `wlan`.
    // Der Chip ist hier bereits aus (Stufe 1 schaltet ihn zuletzt ab), also
    // kann niemand mehr in die gleich freigegebenen Puffer schreiben.
    host::print("[rtl8822ce] fertig — Chip ist aus, Geraet freigegeben\n");
}

/// main.c:2064-2081 `rtw_chip_board_info_setup` — Stufe 4b.
///
/// **Sie steht hier und nicht spaeter, weil sie in Linux hier steht:**
/// `rtw_chip_info_setup` ruft `parameter_setup`, dann `efuse_info_setup`,
/// dann `board_info_setup` — alles zur Probe-Zeit, VOR `rtw_power_on`. Sie
/// fasst kein Register an; sie fuellt die Tabellen, aus denen
/// `rtw_set_channel` spaeter die Sendeleistung rechnet.
///
/// Das Gate braucht deshalb kein Geraet: `gen_tables.py` rechnet dieselbe
/// Kette in Python nach und legt Pruefsummen ueber die 25 KiB abgeleiteten
/// Zustand ab. Stimmt eine nicht, ist ein einzelnes Byte anders — und das
/// faellt hier auf statt als schiefe Sendeleistung auf einem Kanal.
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

    // Eine Zahl zum Anfassen: die Grenze fuer FCC, 20 MHz, CCK, Kanal 1.
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

/// main.c:1374-1411 `rtw_power_on`, bis einschliesslich `rtw_mac_init`.
///
/// Was danach kommt (`phy_set_param`, `mac_postinit`, `hci_start`, die
/// H2C-Nachrichten und die Koexistenz) ist Stufe 3b und 3c — es steht hier
/// bewusst NICHT als Platzhalter, damit niemand eine halbe Kette fuer eine
/// ganze haelt.
fn power_on_and_mac_init(
    h: i32, hal: &Hal, trx: &mut pci::Trx, stage_buf: i32, fifo: &mut mac::Fifo,
) -> bool {
    // rtw_hci_setup
    pci::setup(h, trx, false);

    // rtw_mac_power_on
    let t0 = host::now_us();
    if mac::mac_power_on(h, hal.cut_version).is_err() {
        host::print("  rtw_mac_power_on fehlgeschlagen\n");
        return false;
    }
    host::print("  MAC an nach ");
    host::print_dec((host::now_us() - t0) as u32);
    host::print(" us, CR = 0x");
    host::print_hex8(host::r8(h, REG_CR));
    host::print("\n");

    // rtw_wait_firmware_completion entfaellt: unsere Firmware liegt im
    // Binaerbild, es gibt kein asynchrones Nachladen, auf das zu warten waere.

    // rtw_download_firmware
    if stage_buf < 0 {
        host::print("  kein DMA fuer den Zwischenpuffer\n");
        return false;
    }
    let t0 = host::now_us();
    if !mac::download_firmware(h, trx, stage_buf, FW, BAND_AT_FWDL,
                               fifo.rsvd_boundary) {
        host::print("  zweiter Firmware-Download fehlgeschlagen\n");
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

    // Der Seitenplan im Klartext. Er ist die Zahl, an der ab jetzt jede
    // Reserved Page haengt — und er steht nirgendwo sonst.
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

    // Die Gates der Stufe: die zwei Quittungen der Hardware und die zwei
    // Zahlen, die aus Linux' eigener Rechnung fallen.
    let llt = host::r8(h, REG_AUTO_LLT_V1) & BIT_AUTO_INIT_LLT_V1 as u8;
    let mut ok = true;
    ok &= gate("Link-List-Tabelle gebaut (AUTO_INIT_LLT_V1 geloescht)", llt == 0);
    ok &= gate("rsvd_boundary == 1938 (2048 Seiten minus 110 reservierte)",
               f.rsvd_boundary == 1938);
    // rtw_pci_interface_cfg auf cut >= D.
    let mix = host::r32(h, REG_HCI_MIX_CFG);
    ok &= gate("PCIE_EMAC_PDN_AUX_TO_FAST_CLK steht (cut D)",
               mix & BIT_PCIE_EMAC_PDN_AUX_TO_FAST_CLK != 0);
    // Der MAC laeuft: REG_CR traegt alle acht TRX-Bits.
    let cr = host::r8(h, REG_CR);
    ok &= gate("REG_CR traegt MAC_TRX_ENABLE", cr & MAC_TRX_ENABLE == MAC_TRX_ENABLE);
    ok
}

/// main.c:1413 `chip->ops->phy_set_param` — Stufe 3b (Tabellen) und
/// 3c (BB/RF-Aufbau) in einem Zug, weil `rtw_phy_load_tables` MITTEN in
/// `rtw8822c_phy_set_param` steht und nicht daneben.
///
/// Die zwei Gates sind getrennt, weil sie verschiedene Fragen stellen:
/// **3b** — kommt aus den Tabellen ueberhaupt etwas an? Gemessen wird das
/// am RF-Register 0x00 BEIDER Pfade: es traegt nach dem Laden den Wert, den
/// die Tabelle hineingeschrieben hat, und ist weder 0 noch 0xfffff.
/// **3c** — hoert der Empfaenger? Gemessen an `false_alarm_statistics`:
/// die CCA-Zaehler des Chips laufen nur, wenn die BB arbeitet.
fn phy_set_param_and_check(h: i32, hal: &Hal, e: &efuse::Efuse) -> (bool, bool) {
    host::print("[rtl8822ce] Stufe 3b/3c: rtw8822c_phy_set_param\n");

    let mut dm = dm::DmInfo::new();
    let mut path_div = dm::PathDiv::default();

    let t0 = host::now_us();
    let (tables_ok, dack_ok) = chip::phy_set_param(h, &mut dm, &mut path_div, e,
                                                   hal.cut_version, hal.rf_path_num,
                                                   hal.antenna_tx, hal.antenna_rx);
    host::print("  phy_set_param fertig in ");
    host::print_dec((host::now_us() - t0) as u32);
    host::print(" us\n");

    // ── Gate 3b ──────────────────────────────────────────────────
    // `rtw_phy_read_rf` geht ueber das direkte Fenster; ein Pfad, der nicht
    // antwortet, liefert 0xfffff (alle Bits) oder 0.
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
    // **Das Gate stand in 0.10.0 an der falschen Stelle**, und in 0.10.1
    // stand es an der zweiten falschen Stelle. Beide Fehler sind derselbe:
    // eine Bedingung erfinden, die Linux nicht hat.
    //
    // 0.10.0 fragte nach CCA-Ereignissen. Die kann es hier nicht geben,
    // auch in Linux nicht: `false_alarm_statistics` laeuft dort erst im
    // Wachhund (main.c:280), also NACH `rtw_coex_power_on_setting` (das die
    // gemeinsame Antenne umlegt) und NACH `rtw_set_channel` (das AGC,
    // CCA-Maske und RX-Filter programmiert). Beides ist Stufe 4.
    //
    // 0.10.1 fragte nach der Konvergenz der DAC-Kalibrierung. **Linux
    // prueft das nirgends** — `rtw8822c_rf_dac_cal` laeuft zehnmal und geht
    // weiter, ob der Restversatz unter 5 faellt oder nicht. Wir haben kein
    // Linux auf diesem Geraet, koennen also nicht wissen, was dort
    // herauskaeme. Ein Gate ohne Vergleichsmass ist eine Meinung
    // ([[feedback_a_test_of_a_state_must_say_when]]).
    //
    // Was Stufe 3c beantworten KANN: lief `phy_set_param` durch und
    // antworten beide RF-Pfade mit dem, was die Tabellen hineingeschrieben
    // haben. Die Konvergenz steht als BEFUND darunter, mit Zahlen.
    let stage3c = gate("phy_set_param lief durch, beide RF-Pfade antworten",
                       rf_ok);

    host::print(if dack_ok {
        "  [Befund] die DAC-Kalibrierung konvergiert auf beiden Pfaden\n"
    } else {
        "  [Befund] die DAC-Kalibrierung konvergiert NICHT auf beiden Pfaden.\n                    Linux prueft das nicht, wir haben kein Vergleichsmass —\n                    benannt und offen, siehe docs/plan/WIFI_RTL8822CE.md\n"
    });

    // Gemessen, aber NICHT gewertet: die Zaehler stehen hier
    // erwartungsgemaess auf 0. Sie stehen trotzdem im Log, weil sie ab
    // Stufe 4 das Gate sind und man dann die Ausgangslage kennen will.
    chip::false_alarm_statistics(h, &mut dm);
    host::sleep_ms(50);
    chip::false_alarm_statistics(h, &mut dm);

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

/// main.c:1413-1434 der Rest von `rtw_power_on`, dann main.c:1517-1533
/// `rtw_core_start` bis zum RCR-Schreibzugriff.
///
///     rtw_mac_postinit        beim 8822C NULL (rtw8822c.c:4967) -> nichts
///     rtw_hci_start           = rtw_pci_start: schaltet NUR Interrupts
///                               frei. Wir pollen -> BENANNTE ABWEICHUNG,
///                               siehe docs/plan/WIFI_RTL8822CE.md
///     rtw_fw_send_general_info    H2C-PAKET durch die H2C-Queue
///     rtw_fw_send_phydm_info      dito
///     rtw_coex_power_on_setting   Antenne auf BT
///     rtw_coex_init_hw_config     danach auf INIT
///     rtw_sec_enable_sec_engine
///     rtw_write32(REG_RCR, hal->rcr)
fn stage4a_power_on_tail(h: i32, hal: &Hal, trx: &mut pci::Trx, h2c_buf: i32,
                         fifo: &mut mac::Fifo, e: &efuse::Efuse) -> bool {
    host::print("[rtl8822ce] Stufe 4a: rtw_power_on (Rest) + rtw_core_start\n");

    if h2c_buf < 0 {
        host::print("  kein DMA fuer den H2C-Zwischenpuffer\n");
        return false;
    }

    let mut h2c = fw::H2cState::default();
    let mut cx = coex::Coex::new();

    // `rtw_mac_postinit`: `chip->ops->mac_postinit` ist beim 8822C NULL,
    // die Funktion kehrt ohne einen Registerzugriff zurueck.

    // `rtw_hci_start` = `rtw_pci_start`: setzt `rtwpci->running` und ruft
    // `rtw_pci_enable_interrupt`. Wir fahren den Chip im Abfragebetrieb,
    // es gibt keine Interruptleitung zu diesem Modul — die einzige
    // Abweichung dieser Stufe, und sie steht im Papier.

    // ── Die zwei H2C-PAKETE ──────────────────────────────────────
    // Sie gehen durch die H2C-QUEUE, nicht durch die Mailbox. Der Ring
    // dafuer steht seit Stufe 3a (`init_h2c`).
    let wp_before = trx.tx[pci::Q_H2C].wp;
    let gi = fw::send_general_info(h, trx, h2c_buf, &mut h2c, fifo);
    let pi = fw::send_phydm_info(h, trx, h2c_buf, &mut h2c, e.rfe_option,
                                 hal.rf_2t2r, hal.cut_version,
                                 hal.antenna_rx, hal.antenna_tx);

    host::print("  H2C-Pakete: general_info ");
    host::print(if gi { "ok" } else { "FEHLER" });
    host::print(" (fw_tx_boundary ");
    host::print_dec((fifo.rsvd_fw_txbuf_addr - fifo.rsvd_boundary) as u32);
    host::print("), phydm_info ");
    host::print(if pi { "ok" } else { "FEHLER" });
    host::print("\n");

    // Der Chip holt die Eintraege selbst ab: die oberen zwoelf Bit des
    // Indexregisters sind SEIN Lesezeiger. **Gewartet, nicht gestochert** —
    // in 0.11.0 stand er auf 1, waehrend unserer schon auf 2 stand, und das
    // war nur die Zeit zwischen Anstoss und Lesung.
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

    // ── Die Koexistenz, und damit die ANTENNE ────────────────────
    // `wifi_only = !efuse->btcoex` (main.c:1431). Unsere efuse sagt
    // btcoex JA, also ist es false und der INIT-Zweig gilt.
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
    coex::power_on_setting(h, &mut cx, &mut h2c, e.share_ant, e.rfe_option);
    let scbd_poweron = coex::read_scbd_raw(h);
    coex::init_hw_config(h, &mut cx, &mut h2c, e.share_ant, wifi_only,
                         e.rfe_option);
    let scbd_after = coex::read_scbd_raw(h);
    let dt = host::now_us() - t0;

    // ROH, ohne die Maske von `read_scbd` — sonst laesst sich eine 0 nicht
    // von „unser eigener Schreibzugriff kam nie an" unterscheiden.
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

    // **Das ist die Sache, um die es geht.** Wo steht die Antenne?
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

    // **Das Score-Board ist KEIN Gate.** Es ist ein gemeinsames Postfach:
    // die Bits, die uns interessieren wuerden, schreibt der BT-Kern. Laeuft
    // der nicht, steht dort 0 — und das ist dann wahr, nicht falsch. Linux
    // prueft es nirgends. Was wir pruefen koennen, ist die Wirkung:
    //
    // Bei `bt_disabled` nimmt `set_ant_path(COEX_SET_ANT_INIT)` den Zweig
    // GNT_BT = SW_LOW (1), GNT_WL = SW_HIGH (3) — die Antenne geht an
    // WLAN. Laeuft BT, ist es umgekehrt, und dann teilt die PTA sie.
    let (want_wl, want_bt) = if cx.bt_disabled {
        (COEX_GNT_SET_SW_HIGH, COEX_GNT_SET_SW_LOW)
    } else {
        (COEX_GNT_SET_SW_LOW, COEX_GNT_SET_SW_HIGH)
    };
    ok &= gate("GNT_WL/GNT_BT stehen so, wie set_ant_path(INIT) sie setzt",
               ant.gnt_wl == want_wl && ant.gnt_bt == want_bt);
    ok &= gate("der Pfadbesitzer ist WLAN", ant.wifi_owns_path);
    ok &= gate("RCR steht auf hal->rcr", rcr == hal.rcr);

    // ── Und die Frage, fuer die 4a da ist ────────────────────────
    let mut dm = dm::DmInfo::new();
    chip::false_alarm_statistics(h, &mut dm);
    host::sleep_ms(50);
    chip::false_alarm_statistics(h, &mut dm);
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

/// main.c:1440-1476 `rtw_set_channel` — Stufe 4c.
///
///     rtw_get_channel_params      aus der Kanalwahl werden Mittenkanal,
///                                 Bandbreite und Unterkanallage
///     rtw_update_channel          derselbe Zustand in `hal`
///     chip->ops->set_channel      = rtw8822c_set_channel: BB, MAC, RF,
///                                   toggle_igi
///     rtw_coex_switchband_notify  BENANNTE ABWEICHUNG, siehe unten
///     rtw_phy_set_tx_power_level  aus den Tabellen von 4b wird ein
///                                 Leistungsindex je Rate und Pfad
///
/// **Der Kanal kommt hier von uns, nicht von mac80211.**
/// `rtw_get_channel_params` liest in Linux eine `cfg80211_chan_def`; die
/// gibt es ohne obere Haelfte nicht. Fuer 20 MHz ist das Ergebnis dieser
/// Funktion genau `center = primary = Kanal`, und das ist, was hier
/// eingesetzt wird — die Rechnung fuer 40 und 80 MHz kommt mit der Stufe,
/// die eine Bandbreite auswaehlt.
fn stage4c_set_channel(h: i32, hal: &Hal, e: &efuse::Efuse,
                       t: &txpower::TxPower) -> bool {
    // Kanal 1, 20 MHz. Der niedrigste 2,4-GHz-Kanal ist der, auf dem am
    // ehesten jemand funkt — und genau darum geht es beim Messen.
    const CH: u8 = 1;
    const BW: usize = 0; // RTW_CHANNEL_WIDTH_20
    const PRIMARY_IDX: u8 = RTW_SC_DONT_CARE;

    host::print("[rtl8822ce] Stufe 4c: rtw_set_channel (Kanal ");
    host::print_dec(CH as u32);
    host::print(", 20 MHz)\n");

    // `rtw_update_channel` — bei 20 MHz ist der Mittenkanal der primaere,
    // und `cch_by_bw[20M]` traegt ihn. Der Rest von `hal` (sar_band,
    // current_band_*) wird hier als lokale Groesse gefuehrt.
    let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };
    t2.cch_by_bw[0] = CH;

    let t0 = host::now_us();
    chip::set_channel(h, CH, BW, PRIMARY_IDX);
    let dt_ch = host::now_us() - t0;

    // `rtw_coex_switchband_notify` gehoert zur laufenden Koexistenz
    // (`rtw_coex_run_coex` mit COEX_RSN_2GSWITCHBAND) und braucht den
    // Verkehrszustand, den erst eine Verbindung hat. BENANNT UND NICHT
    // GEBAUT — er entscheidet nicht, ob der Empfaenger hoert.

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

    // RF 0x18 traegt jetzt Band, Kanal und Bandbreite — zurueckgelesen.
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
    // 20 MHz ist RF18_BW_20M = BIT(13)|BIT(12), also 0x3 im Feld.
    ok &= gate("RF 0x18 traegt die Bandbreite 20 MHz",
               (rf18_a >> 12) & 0x3 == 0x3);

    // ── Das Gate, das seit 0.10.0 auf seine Stufe gewartet hat ───
    let mut dm = dm::DmInfo::new();
    chip::false_alarm_statistics(h, &mut dm);
    host::sleep_ms(200);
    chip::false_alarm_statistics(h, &mut dm);

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

/// Wieviele der 54 Kommandos auf UNSEREM Geraet ueberhaupt laufen. Eine
/// Zahl, die man sonst erst beim Suchen vermisst.
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

/// Stufe 5a — der WIRT um `pci::rx_poll` herum, nicht der Port selbst.
///
/// Der Port ist `pci::rx_poll` (= `rtw_pci_rx_napi`); hier steht nur, wie
/// lange gefragt wird und was gemeldet wird. Linux laeuft dort aus dem
/// Interrupt in NAPI; wir haben keine Geraete-Interrupts (benannte
/// Abweichung seit Stufe 2), also wird der Schreibzeiger gelesen.
fn stage5a_rx(h: i32, hal: &Hal, trx: &mut pci::Trx) -> bool {
    /// Derselbe Kanal wie in Stufe 4c — `hal.current_channel`.
    const CH_5A: u8 = 1;

    host::print("[rtl8822ce] Stufe 5a: der Empfangsweg\n");

    // `dm_info` traegt die CCK-Verstaerkungsgrenzen, an denen ein
    // CCK-Paket seine Signalstaerke bekommt. Sie stehen in der Hardware,
    // seit `phy_set_param` sie dort gelesen hat.
    let mut dm = dm::DmInfo::new();
    let mut path_div = dm::PathDiv::default();
    chip::read_cck_gi_bnd(h, &mut dm);
    host::print("  cck_gi Grenzen u/l ");
    host::print_dec(dm.cck_gi_u_bnd as u32);
    host::print("/");
    host::print_dec(dm.cck_gi_l_bnd as u32);
    host::print("\n");

    // Ein ganzer Empfangspuffer. Er liegt statisch, weil dieser Treiber
    // keinen Allokator hat und 11 KB auf dem Stapel nicht stehen.
    static mut RXBUF: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: Einfaeden, ein Rufer, und der Puffer verlaesst diese
    // Funktion nicht. Es gibt in diesem Treiber keinen zweiten Pfad, der
    // ihn anfasst.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF) };

    let mut total = 0u32;
    let mut c2h = 0u32;
    let mut crc = 0u32;
    let mut shown = 0u32;
    let mut best: i8 = -128;
    let mut rounds = 0u32;

    // 2000 ms. Ein Beacon-Intervall sind 102,4 ms, also kommt in dieser
    // Zeit von JEDEM erreichbaren Netz mehr als ein Rahmen — wenn der Weg
    // traegt. Kommt in zwei Sekunden nichts, ist es kein Timing-Problem.
    let t0 = host::now_us();
    while host::now_us() - t0 < 2_000_000 {
        rounds += 1;
        let n = pci::rx_poll(h, trx, 64, buf, &mut dm, &mut path_div,
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
            // Die ersten acht ganz, damit man SIEHT, was ankommt.
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

/// Eine Zeile je Paket: Laenge, Rate, Bandbreite, Kanal, Signal — und die
/// ersten Bytes des Rahmens, denn an `frame_control` sieht man, ob es ein
/// Beacon ist.
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
        // 802.11: Typ in Bits 3:2, Subtyp in 7:4. 0x80 = Beacon.
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

fn gate(name: &str, ok: bool) -> bool {
    host::print(if ok { "  [ JA  ] " } else { "  [NEIN ] " });
    host::print(name);
    host::print("\n");
    ok
}
