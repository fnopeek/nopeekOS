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
mod fw;
mod mac;
mod pci;
mod pwrseq;
mod tx;
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

    host::pci_enable_bus_master();

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
    host::print("[rtl8822ce] Stufe 2a: Ringe belegt — ");
    host::print_dec(trx.dma_pages);
    host::print(" Seiten (");
    host::print_dec(trx.dma_pages * 4 / 1024);
    host::print(" MiB) in ");
    host::print_dec(trx.dma_allocs);
    host::print(" Stuecken, von 2048 Seiten / 1024 Stuecken\n");

    pci::setup(h, &mut trx); // = rtw_hci_setup: reset_trx_ring + dma_reset
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

    let fw_ok = match host::dma_alloc(
        (pci::RSVD_STAGE_BYTES.div_ceil(4096)) as u16) {
        st if st >= 0 => {
            let ok = mac::download_firmware(h, &mut trx, st, FW, BAND_AT_FWDL);
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
        "[rtl8822ce] Stufe 2b: GRUEN — weiter mit 2c (efuse + hw_feature)\n"
    } else {
        "[rtl8822ce] Stufe 2b: NEIN — nicht weiterbauen, bevor das steht\n"
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

fn gate(name: &str, ok: bool) -> bool {
    host::print(if ok { "  [ JA  ] " } else { "  [NEIN ] " });
    host::print(name);
    host::print("\n");
    ok
}
