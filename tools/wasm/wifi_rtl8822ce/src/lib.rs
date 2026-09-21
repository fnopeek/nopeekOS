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

/// Nie still sterben: ein `loop {}` ohne Meldung sieht von aussen aus wie
/// „der Chip antwortet nicht" und hat beim AX200 einen Abend gekostet.
/// `Location` ueberlebt `strip = true`, weil es statische Daten sind.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Eine Panik ist nie Stufenausgabe: laut, auch ohne `debug: 1`, und
    // die Klammer wird nicht mehr geschlossen — danach kommt nichts.
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

/// **`struct rtw_dev` — der Zustand, der so lange lebt wie der Treiber.**
///
/// Bis 0.26.0 legte JEDE Stufe ihr eigenes `DmInfo`, `DpkInfo` und
/// `Coex` an. Solange nur Stufen liefen, war das folgenlos; mit dem
/// Watchdog ist es ein Fehler, und zwar ein stiller:
///
/// * `cfo_track.crystal_cap` kommt aus `rtw_phy_init` (dem Wert der
///   efuse). Ein frisches `DmInfo` traegt dort NULL — die
///   Quarznachfuehrung wuerde von null hochlaufen statt von der
///   Werkseinstellung.
/// * `dpk_info.thermal_dpk` kommt aus der Kalibrierung von Stufe 5d.
///   Ohne sie kehrt `dpk_track` in der ersten Zeile um.
/// * `coex.bt_disabled` entscheidet, ob die Quarznachfuehrung ueberhaupt
///   laufen darf.
///
/// In Linux liegt all das in `rtwdev` und lebt vom Laden bis zum
/// Entladen. Hier jetzt auch.
struct Dev {
    dm: dm::DmInfo,
    path_div: dm::PathDiv,
    dpk: dpk::DpkInfo,
    cx: coex::Coex,
    /// main.h:660-672 `struct rtw_traffic_stats`
    stats: TrafficStats,
    /// `rtwdev->watch_dog_cnt` — `rtw_phy_ra_info_update` laeuft nur auf
    /// jedem vierten.
    watch_dog_cnt: u32,
    /// `RTW_FLAG_BUSY_TRAFFIC`
    busy_traffic: bool,
    /// `rtwdev->beacon_loss`
    beacon_loss: bool,
}

/// main.h:660-672 `struct rtw_traffic_stats`. Die Einheiten stehen dort
/// als Kommentar und sind hier Teil des Namens: Bytes je zwei Sekunden,
/// umgerechnet mit `RTW_TP_SHIFT`.
#[derive(Default, Clone, Copy)]
struct TrafficStats {
    tx_unicast: u64,
    rx_unicast: u64,
    tx_cnt: u64,
    rx_cnt: u64,
    tx_throughput: u32,
    rx_throughput: u32,
    /// Der hoechste je gesehene Wert — der geglaettete faellt nach dem
    /// Ende einer Uebertragung auf null.
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
    // ── Wie laut? ────────────────────────────────────────────────
    // **Zuerst, vor der ersten Zeile.** Im Autostart druckt der Treiber
    // sechs Stufen mit ihren Toren und macht die Konsole unbrauchbar;
    // `debug: 1` in `sys/config/wifi` holt sie zurueck. Die Datei ist
    // dieselbe, aus der Stufe 5c ihr `ssid:` liest — eine zweite Stelle
    // fuer dieselbe Sache driftet.
    let (verbose, cfg_rc) = read_debug_flag();
    host::set_verbose(verbose);

    host::print("[rtl8822ce] Realtek RTL8822CE (rtw88) v");
    host::print(DRIVER_VERSION);
    host::print(" — Stufe 0: binden, BAR2, Chipkennung\n");
    if !verbose {
        // Die eine Zeile, die auch ein stiller Lauf schuldet: dass es
        // den Treiber gibt und wo der Schalter steht.
        //
        // **Und ob die Datei ueberhaupt gelesen wurde.** `wifid`
        // dokumentiert fuer genau dieses Objekt ein Rennen mit dem Rest
        // des Bootvorgangs; ohne diesen Zusatz saehe ein gescheiterter
        // Lesezugriff aus wie ein Schalter, der nicht greift — und das
        // kostet einen ganzen Geraetelauf.
        host::say("[rtl8822ce] v");
        host::say(DRIVER_VERSION);
        if cfg_rc > 0 {
            host::say(" — still (`debug: 1` in sys/config/wifi zeigt die Stufen)\n");
        } else {
            host::say(" — still, und sys/config/wifi war beim Start nicht\n\
             \x20         lesbar: ein `debug: 1` darin greift dann NICHT\n");
        }
    }

    // ── `rtwdev`: der Zustand ueber den ganzen Treiberlauf ───────
    // Gross genug, um nicht auf den Stapel zu gehoeren (die
    // DACK-Sicherungen und die Ratenzaehler machen den Loewenanteil).
    static mut DEV: Dev = Dev {
        dm: dm::DmInfo::new(),
        path_div: dm::PathDiv::new(),
        dpk: dpk::DpkInfo::new(),
        cx: coex::Coex::new(),
        stats: TrafficStats::new(),
        watch_dog_cnt: 0,
        busy_traffic: false,
        beacon_loss: false,
    };
    // SAFETY: einfaedig, genau ein Rufer, und `_start` kehrt erst
    // zurueck, wenn der Treiber endet.
    let rtwdev = unsafe { &mut *core::ptr::addr_of_mut!(DEV) };

    // ── PCI binden ───────────────────────────────────────────────
    // rtw8822ce.c fuehrt zwei Geraete-IDs fuer denselben Chip.
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

    // Rueckgabewert lesen, nicht wegwerfen: ohne Busmaster kann der Chip
    // keinen Deskriptor aus dem Hauptspeicher holen, und das sieht dann aus
    // wie ein Fehler im Treiber statt wie eine fehlende Erlaubnis.
    let bm = host::pci_enable_bus_master();
    if bm != 0 {
        host::say("[rtl8822ce] Bus-Master konnte nicht eingeschaltet werden\n");
    }
    fw::dump_pci_cmd("nach bind");

    // ── BAR2 abbilden ────────────────────────────────────────────
    let h = host::mmio_map_bar(BAR_REG, BAR_PAGES);
    if h < 0 {
        host::say("[rtl8822ce] BAR2 nicht abbildbar — Stufe 0 endet hier\n");
        return;
    }
    host::print("[rtl8822ce] BAR2 abgebildet (handle ");
    host::print_dec(h as u32);
    host::print(", 64 KiB)\n");

    // ── `rtw_pci_phy_cfg` / `rtw_pci_link_cfg` ───────────────────
    // Muss NACH der BAR-Abbildung stehen: der DBI-Weg laeuft ueber MMIO.
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
        host::say("[rtl8822ce] Stufe 0: NEIN — Stufe 1 und 2a werden nicht gefahren\n");
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

    // Die Merkmalsbits der Firmware entscheiden, welche H2C-Kommandos sie
    // ueberhaupt kennt (`rtw_fw_feature_check`). Sie stehen im Kopf des
    // Abbilds, also gibt es sie schon vor dem Download.
    let fw_feature = mac::parse_fw_hdr(FW).feature;

    // `rtwdev->h2c` — EINER fuer das ganze Geraet. Die Reihenfolge der vier
    // Postfaecher ist der Sinn der Sache: der Treiber reicht sie im Kreis
    // weiter, damit die Firmware Zeit hat, das vorige zu leeren. Bis 0.17.0
    // legte jede Stufe einen eigenen an und fing wieder bei Fach 0 an.
    let mut h2c = fw::H2cState::default();

    let stage_buf = host::dma_alloc_below(
        (pci::RSVD_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    // Der H2C-Ring braucht einen EIGENEN Zwischenpuffer: der oben ist fuer
    // den Firmware-Download gedacht und genau ein Stueck gross.
    let h2c_buf = host::dma_alloc_below(
        (pci::H2C_STAGE_BYTES.div_ceil(4096)) as u16, 1024);
    // Und die MGMT-Queue einen dritten: ihre Rahmen sind bis 2 KB gross,
    // das H2C-Raster von 128 Bytes traegt keinen einzigen davon.
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

    // ── Stufe 4b: rtw_chip_board_info_setup ──────────────────────
    // Sie steht VOR Stufe 3, weil sie in Linux vor `rtw_power_on` steht:
    // `rtw_chip_info_setup` = parameter_setup -> efuse_info_setup ->
    // board_info_setup. Die Nummer 4b ist die Reihenfolge, in der wir
    // gebaut haben, nicht die, in der gelaufen wird.
    let (stage4b, _txpwr) = match efuse.as_ref() {
        Some(e) if stage2c => stage4b_board_info_setup(e.rfe_option),
        _ => {
            host::say("[rtl8822ce] Stufe 4b: uebersprungen, 2c steht nicht\n");
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

    // ── Stufe 4a: der Rest von rtw_power_on und rtw_core_start ───
    let stage4a = match (stage3c, efuse.as_ref()) {
        (true, Some(e)) => stage4a_power_on_tail(h, &hal, &mut trx, h2c_buf, &mut h2c,
                                                 &mut fifo, e, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 4a: uebersprungen, 3c steht nicht\n");
            false
        }
    };

    // ── Stufe 4c: rtw_set_channel ────────────────────────────────
    let stage4c = match (stage4a && stage4b, efuse.as_ref(), _txpwr.as_ref()) {
        (true, Some(e), Some(t)) => stage4c_set_channel(h, &hal, e, t, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 4c: uebersprungen, 4a/4b stehen nicht\n");
            false
        }
    };

    // ── Stufe 5a: der Empfangsweg ────────────────────────────────
    let stage5a = if stage4c {
        stage5a_rx(h, &hal, &mut trx, rtwdev)
    } else {
        host::say("[rtl8822ce] Stufe 5a: uebersprungen, 4c steht nicht\n");
        false
    };

    // ── Stufe 5b: der Sendeweg ───────────────────────────────────
    let stage5b = match (stage5a, efuse.as_ref()) {
        (true, Some(e)) => stage5b_tx(h, &hal, &mut trx, mgmt_buf, e.addr, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5b: uebersprungen, 5a steht nicht\n");
            false
        }
    };

    // ── Stufe 5c: der Suchlauf ───────────────────────────────────
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

    // ── Stufe 5d: die RF-Kalibrierung ────────────────────────────
    let stage5d = match (stage5c, efuse.as_ref()) {
        (true, Some(e)) => stage5d_calibration(h, &hal, &mut trx, h2c_buf, &mut h2c, e, rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5d: uebersprungen, 5c steht nicht\n");
            false
        }
    };

    // ── Stufe 5e: Auth und Assoc ─────────────────────────────────
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

    // ── Stufe 5f: die Ratenanpassung ─────────────────────────────
    let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;
    let stage5f = match (stage5e, linked.as_ref(), target.as_ref(),
                         efuse.as_ref()) {
        (true, Some(v), Some(b), Some(ef)) =>
            stage5f_rates(h, &mut trx, &mut h2c, &hal, ef, v, b, &mut rates,
                          rtwdev),
        _ => {
            host::say("[rtl8822ce] Stufe 5f: uebersprungen, 5e steht nicht\n");
            false
        }
    };

    // ── Stufe 6a: Steuerkanal, Handschlag, Datenweg ──────────────
    let mut link: Option<Link> = None;
    let mut lstats = LinkStats::default();
    let stage6a = match (stage5f, linked.as_ref(), target.as_ref(),
                         rates.as_ref(), efuse.as_ref()) {
        (true, Some(_v), Some(b), Some((caps, si)), Some(e)) =>
            stage6a_link(h, &hal, &mut trx, mgmt_buf, b, caps, *si,
                         e.addr, &mut link, &mut lstats, rtwdev, &mut h2c, e,
                         fw_feature),
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

    // ── Stufe 6b: stehenbleiben ──────────────────────────────────
    //
    // **Hier kehrt der Treiber nicht mehr zurueck.** Bis 6a lief eine
    // Stufenkette, und am Ende schaltete `mac_power_off` den Chip ab — die
    // Verbindung stand, die DHCP-Adresse kam, und Sekunden spaeter war
    // beides weg. Ein Treiber, der seine Arbeit beendet, ist kein Treiber.
    //
    // Die Zusammenfassung steht deshalb DAVOR: wer nie zurueckkehrt, kann
    // sie hinterher nicht mehr drucken.
    if stage6a {
        if let (Some(e), Some(l)) = (efuse.as_ref(), link.as_mut()) {
            host::print("[rtl8822ce] Stufe 6b: der Treiber bleibt stehen —\n             \x20         Bericht je Sekunde, RX-Wachhund, kein\n             \x20         Abschalten mehr\n");
            // **Die eine Zeile eines stillen Laufs.** Sie steht hier und
            // nicht bei AUTHORIZED: erst hinter den Toren von 6a ist sie
            // eine Aussage ueber eine VERBINDUNG und nicht ueber einen
            // Zwischenstand. Wer sie liest, muss nichts weiter fragen.
            report_connected(l, target.as_ref(), linked.as_ref());
            // **Dieselbe Schleife, derselbe Link, dieselben Zaehler.**
            // 6b setzt fort, statt neu anzufangen — ein zweites
            // `EV_READY` liesse `wifid` einen frischen Supplicant bauen,
            // der auf ein msg1 wartet, das der AP nie wieder schickt.
            let caps = rates.as_ref().map(|(c, _)| *c).unwrap_or_default();
            // **Ab hier laeuft die Verbindung, und sie kann enden.**
            // Bis 0.26.0 kehrte `link_pump` nie zurueck: ein Rauswurf
            // hinterliess eine tote Leitung, bis jemand neu bootete.
            // Jetzt ist der Weg zurueck derselbe wie der Weg hin —
            // Stufe 5e und 5f, ohne den Kernel noch einmal anzumelden.
            loop {
                let end = link_pump(h, &hal, &mut trx, mgmt_buf, l,
                                    &mut lstats, e.addr, 0, rtwdev,
                                    &mut h2c, e, &caps, fw_feature);
                if end != PumpEnd::LinkLost {
                    break;
                }
                let (Some(t), Some(b)) = (_txpwr.as_ref(), target.as_ref())
                else {
                    break;
                };
                if !reconnect(h, &hal, &mut trx, mgmt_buf, &mut h2c, e, t, b,
                              l, &mut lstats, rtwdev, &mut linked) {
                    // Nicht aufgeben, aber auch nicht im Kreis rennen:
                    // ein AP, der gerade neu startet, braucht Sekunden.
                    host::sleep_ms(RECONNECT_BACKOFF_MS);
                }
            }
        }
    }

    // Ab hier nur noch, wenn eine Stufe NICHT steht: der Kernel gibt
    // gleich die DMA-Puffer frei, in die eine laufende Firmware sonst
    // weiterschriebe.
    mac::mac_power_off(h, hal.cut_version);


    // Zurueckkehren, nicht schlafen. Der Kernel raeumt danach auf: DMA
    // freigeben, PCI loesen, und den Bericht loeschen — letzteres mit der
    // ausdruecklichen Begruendung, dass die Zahlen eines toten Treibers nicht
    // wie lebende aussehen duerfen. Das Ergebnis dieser Stufen steht deshalb
    // HIER im Terminal und nicht in `wlan`.
    // Der Chip ist hier bereits aus (Stufe 1 schaltet ihn zuletzt ab), also
    // kann niemand mehr in die gleich freigegebenen Puffer schreiben.
    // **Hierher kommt nur ein Lauf, der NICHT steht** — mit Verbindung
    // kehrt 6b nie zurueck. Also laut, auch ohne `debug: 1`.
    host::say("[rtl8822ce] fertig — Chip ist aus, Geraet freigegeben\n");
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
        host::say("  rtw_mac_power_on fehlgeschlagen\n");
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
                         h2c: &mut fw::H2cState,
                         fifo: &mut mac::Fifo, e: &efuse::Efuse, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 4a: rtw_power_on (Rest) + rtw_core_start\n");

    if h2c_buf < 0 {
        host::print("  kein DMA fuer den H2C-Zwischenpuffer\n");
        return false;
    }

    let cx = &mut d.cx;

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
    coex::power_on_setting(h, cx, h2c, e.share_ant, e.rfe_option);
    let scbd_poweron = coex::read_scbd_raw(h);
    coex::init_hw_config(h, cx, h2c, e.share_ant, wifi_only,
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
                       t: &txpower::TxPower, d: &mut Dev) -> bool {
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
fn stage5a_rx(h: i32, hal: &Hal, trx: &mut pci::Trx, d: &mut Dev) -> bool {
    /// Derselbe Kanal wie in Stufe 4c — `hal.current_channel`.
    const CH_5A: u8 = 1;

    host::print("[rtl8822ce] Stufe 5a: der Empfangsweg\n");

    // `dm_info` traegt die CCK-Verstaerkungsgrenzen, an denen ein
    // CCK-Paket seine Signalstaerke bekommt. Sie stehen in der Hardware,
    // seit `phy_set_param` sie dort gelesen hat.
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
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

/// Stufe 5b — der Sendeweg, und die Antwort darauf.
///
/// Der Port bekommt seine Adresse (`rtw_ops_add_interface`), dann geht ein
/// Probe Request hinaus und wir hoeren zu. **Das Gate ist die ANTWORT**,
/// nicht der verbrauchte Deskriptor: dass der Chip einen Deskriptor abholt,
/// sagt nur, dass DMA laeuft — dass ein fremder AP antwortet, sagt, dass
/// der Rahmen die Antenne verlassen hat und richtig gebaut war.
///
/// **Kalibriert wird hier bewusst nicht.** `rtw_set_channel` setzt am Ende
/// `need_rfk = true`, und `rtw_chip_prepare_tx` fuehrt GAPK/IQK/DPK erst
/// aus, wenn mac80211 `mgd_prepare_tx` ruft — also VOR dem Anmelden, nicht
/// beim Kanalwechsel. Linux' Kommentar nennt den Grund: waehrend eines
/// Scans auf jedem Kanal zu kalibrieren dauert zu lange. Ein Probe Request
/// geht in Linux genauso unkalibriert hinaus wie hier.
fn stage5b_tx(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
              mac: [u8; 6], d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5b: der Sendeweg\n");
    if mgmt_buf < 0 {
        host::print("  kein DMA-Puffer fuer die MGMT-Queue\n");
        return false;
    }

    // `rtw_ops_add_interface`, Zweig STATION. Ohne diesen Schritt steht im
    // Port-Register keine Adresse, und `BIT_APM` im RCR laesst dann nur
    // Broadcast durch — eine Probe Response ist an UNS gerichtet.
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

    // Ein Probe Request. Das baut in Linux `ieee80211_build_probe_req` —
    // die OBERE Haelfte, die hier `wifid` wird. Er steht hier, weil der
    // Sendeweg sonst nichts zu senden haette; 5c loest ihn ab.
    let mut frame = [0u8; 64];
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

    // Dreimal, mit Abstand: ein einzelner Probe Request kann kollidieren,
    // und ein AP darf ihn auch schlicht verwerfen.
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

    // Und jetzt zuhoeren. Eine Probe Response ist Subtyp 5.
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF2: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: wie in Stufe 5a — ein Faden, ein Rufer, der Puffer verlaesst
    // diese Funktion nicht.
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
            // Subtyp 5 = Probe Response, Typ 0 = Verwaltung.
            if fc & 0xfc != 0x50 {
                return;
            }
            // `addr1` ist unsere Adresse — sonst haette der Filter ihn
            // gar nicht durchgelassen, aber gemessen ist besser.
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

/// `ieee80211_build_probe_req` in klein: ein Wildcard-Probe-Request.
///
/// Die Sequenznummer bleibt null — `en_hwseq` steht im Deskriptor, also
/// vergibt sie der Chip. Gebaut wird nur, was ein AP zum Antworten
/// braucht: die drei Adressen, das leere SSID-Element und die Raten.
fn build_probe_req(out: &mut [u8; 64], mac: &[u8; 6], ch: u8) -> usize {
    let bcast = [0xffu8; 6];
    out[0..2].copy_from_slice(&0x0040u16.to_le_bytes()); // Verwaltung, Subtyp 4
    out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration
    out[4..10].copy_from_slice(&bcast); // addr1 = Empfaenger
    out[10..16].copy_from_slice(mac); // addr2 = wir
    out[16..22].copy_from_slice(&bcast); // addr3 = BSSID
    out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq, siehe oben
    let mut n = 24;
    // SSID-Element, Laenge 0 = „jedes Netz"
    out[n] = 0;
    out[n + 1] = 0;
    n += 2;
    // Supported Rates: 1, 2, 5.5, 11, 6, 9, 12, 18 Mbit. Das hohe Bit
    // markiert eine GRUNDrate.
    out[n] = 1;
    out[n + 1] = 8;
    out[n + 2..n + 10]
        .copy_from_slice(&[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    n += 10;
    // DS Parameter Set: der Kanal, auf dem wir fragen.
    out[n] = 3;
    out[n + 1] = 1;
    out[n + 2] = ch;
    n + 3
}

/// Eine Zeile je Antwort: BSSID, Signal und der Netzname aus dem
/// SSID-Element. Ein Name macht aus „ein Rahmen kam" ein „wir sehen X".
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
    // 24 Kopf + 12 feste Felder (Zeitstempel, Intervall, Faehigkeiten),
    // dann die Elemente. Das SSID-Element hat die Kennung 0.
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

/// Eine gefundene Funkzelle. Nur das, was aus Beacon oder Probe Response
/// sicher herausfaellt — nichts Abgeleitetes.
#[derive(Clone, Copy)]
struct Bss {
    bssid: [u8; 6],
    ssid: [u8; 32],
    ssid_len: u8,
    channel: u8,
    best: i8,
    beacons: u16,
    resps: u16,
    /// Faehigkeitsfeld aus Beacon/Probe Response (802.11 §9.4.1.4).
    capability: u16,
    /// Das RSN-Element des AP, roh. Daraus waehlt der Anmeldeantrag
    /// SEINE Verfahren — ein AP lehnt sonst mit Status 43 ab.
    rsn: [u8; 64],
    rsn_len: u8,
    /// Byte 1 des HT-Operation-Elements (802.11 §9.4.2.56, id 61):
    /// Bit 1:0 die Lage des Zweitkanals, Bit 2 ob der AP ueberhaupt
    /// breiter als 20 MHz zulaesst. **Ohne dieses Byte gibt es kein
    /// HT40** — welche HAELFTE die breite Zelle belegt, sagt allein der
    /// AP, und eine geratene Haelfte ist ein anderer Kanal.
    ht_param: u8,
}

const MAX_BSS: usize = 48;

/// main.c:822-867 `rtw_get_channel_params` und main.c:759-792, der Teil von
/// `rtw_update_channel`, der die Unterkanallage waehlt — zusammengezogen,
/// weil beide dieselbe Fallunterscheidung fahren und wir keine `chandef`
/// haben, sondern das HT-Operation-Byte des AP.
///
/// Linux rechnet in FREQUENZEN (`primary_freq > center_freq`), wir in
/// Kanalnummern. Das ist dieselbe Aussage: ein Kanalschritt sind 5 MHz,
/// und der Vergleich dreht sich mit. Ausgeschrieben, damit es nachpruefbar
/// ist statt geglaubt:
///
/// ```text
///   Zweitkanal OBEN  (0x1): Mitte = primaer + 2, primaer ist die UNTERE
///   Zweitkanal UNTEN (0x3): Mitte = primaer - 2, primaer ist die OBERE
/// ```
///
/// 80 MHz fehlt hier mit Absicht: dafuer braucht es das
/// VHT-Operation-Element, und ohne 5 GHz gibt es keins.
fn chan_params(primary: u8, ht_param: u8, allow_40: bool) -> (u8, usize, u8) {
    const SEC_OFFSET: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_OFFSET
    const SEC_ABOVE: u8 = 0x01; // IEEE80211_HT_PARAM_CHA_SEC_ABOVE
    const SEC_BELOW: u8 = 0x03; // IEEE80211_HT_PARAM_CHA_SEC_BELOW
    const WIDTH_ANY: u8 = 0x04; // IEEE80211_HT_PARAM_CHAN_WIDTH_ANY

    // **Beide Bedingungen, nicht eine.** Ein AP darf den Zweitkanal nennen
    // und die Breite trotzdem verbieten (Bit 2 aus), waehrend er gerade
    // einen 20-MHz-Nachbarn schuetzt. Wer nur den Versatz liest, sendet
    // dann 40 MHz in eine Zelle, die 20 erwartet.
    if !allow_40 || ht_param & WIDTH_ANY == 0 {
        return (primary, 0, RTW_SC_DONT_CARE);
    }
    let center = match ht_param & SEC_OFFSET {
        SEC_ABOVE => primary.saturating_add(2),
        SEC_BELOW => primary.saturating_sub(2),
        _ => return (primary, 0, RTW_SC_DONT_CARE),
    };

    // **Eine Zutat gegenueber Linux, und hier ist der Grund.** `rtw88`
    // bekommt eine `cfg80211_chan_def`, die cfg80211 vorher geprueft hat
    // (`cfg80211_chandef_valid`); es RECHNET nur noch. Wir haben kein
    // cfg80211 — unsere Eingabe ist ein Byte aus einem fremden Beacon,
    // und wenn das „Zweitkanal oben" auf Kanal 13 sagt, faehrt die
    // Rechnung auf Kanal 15. Den gibt es nicht, seine Sendeleistung steht
    // in keiner Tabelle, und in Europa ist er nicht zugelassen.
    //
    // Geprueft wird deshalb der MITTENkanal gegen das Band, aus dem der
    // primaere kommt. Faellt er heraus, gilt 20 MHz — ein schmaler Kanal
    // ist immer erlaubt, ein erfundener nie.
    let plausibel = if primary <= 14 {
        (1..=13).contains(&center)
    } else {
        (36..=165).contains(&center)
    };
    if !plausibel {
        return (primary, 0, RTW_SC_DONT_CARE);
    }

    // main.c:766-768: liegt der primaere UEBER der Mitte, ist er die obere
    // Haelfte. Bei „Zweitkanal oben" ist er also die untere.
    if center > primary {
        (center, 1, RTW_SC_20_LOWER)
    } else {
        (center, 1, RTW_SC_20_UPPER)
    }
}

/// main.c:880-913 `rtw_set_channel` — der Teil, der auf JEDEN Kanal passt.
///
/// `primary` ist der Kanal, auf dem die Zelle ihre Beacons sendet;
/// `ht_param` das Byte aus ihrem HT-Operation-Element. Der Suchlauf ruft
/// mit `allow_40 = false` — auf einem Kanal, den man nur abhorcht, ist
/// jede Breite ueber 20 MHz eine Behauptung ueber den Nachbarkanal.
///
/// **Was an den Chip geht, ist der MITTENkanal**, nicht der primaere
/// (main.c:817 `hal->current_channel = center_channel`) — und deshalb
/// prueft auch die Gegenprobe an RF 0x18 gegen die Mitte.
fn switch_channel(h: i32, hal: &Hal, e: &efuse::Efuse, t: &txpower::TxPower,
                  primary: u8, ht_param: u8, allow_40: bool) -> bool {
    let (ch, bw, primary_idx) = chan_params(primary, ht_param, allow_40);

    // `rtw_update_channel`: der 20-MHz-Eintrag ist IMMER der primaere
    // Kanal, der Eintrag der laufenden Breite die Mitte (main.c:754-757).
    let mut t2 = txpower::TxPower { cch_by_bw: t.cch_by_bw, ..*t };
    t2.cch_by_bw[0] = primary;
    t2.cch_by_bw[bw] = ch;

    chip::set_channel(h, ch, bw, primary_idx);

    // `rtw_coex_switchband_notify` steht hier in Linux, mit drei
    // verschiedenen Gruenden je nach Band und Suchlauf. Er muendet in
    // `rtw_coex_run_coex` — die LAUFENDE Koexistenz, die den Verkehrs- und
    // BT-Zustand braucht. Benannt und nicht gebaut, seit Stufe 4c.

    let band = if ch > 14 { txpower::PHY_BAND_5G } else { txpower::PHY_BAND_2G };
    let mut tbl = [[0u8; txpower::DESC_RATE_MAX]; txpower::RTW_RF_PATH_MAX];
    let idx: [txpower::TxPwrIdx; 4] = [
        txpower::TxPwrIdx(&e.txpwr_idx[0]), txpower::TxPwrIdx(&e.txpwr_idx[1]),
        txpower::TxPwrIdx(&e.txpwr_idx[2]), txpower::TxPwrIdx(&e.txpwr_idx[3]),
    ];
    txpower::set_tx_power_level(&t2, &idx, &mut tbl, hal.rf_path_num, ch, bw,
                                band, e.regd as usize);
    chip::set_tx_power_index(h, hal.rf_path_num, &tbl);

    // `need_rfk` wird beim Suchen NICHT gesetzt — genau das ist der Sinn des
    // `RTW_FLAG_SCANNING`-Zweigs: auf jedem Kanal zu kalibrieren dauert zu
    // lange. Die Kalibrierung gehoert vor das Anmelden.

    // Und die Gegenprobe: RF 0x18 traegt Band, Kanal und Bandbreite. Sie
    // kostet zwei Lesezugriffe und sagt etwas ueber UNS statt ueber die
    // Nachbarschaft — ob ein Netz auf einem Kanal funkt, entscheidet nicht
    // der Treiber, ob der Chip den Kanal angenommen hat schon.
    let a = phy::read_rf(h, phy::RF_PATH_A, 0x18, phy::RFREG_MASK);
    let b = phy::read_rf(h, phy::RF_PATH_B, 0x18, phy::RFREG_MASK);
    a & 0xff == ch as u32 && b & 0xff == ch as u32
}

/// Stufe 5c — der Suchlauf.
///
/// **Aktiv auf 2,4 GHz, passiv auf 5 GHz.** Aktiv heisst: ein Probe Request
/// hinaus, dann zuhoeren. Passiv heisst: nur zuhoeren. Der Unterschied ist
/// keine Bequemlichkeit — auf welchen 5-GHz-Kanaelen gesendet werden DARF,
/// entscheidet die Zulassungszone, und die Regeln dafuer gehoeren der
/// oberen Haelfte (`wifid`/cfg80211), nicht dem Treiber. Empfangen ist
/// ueberall erlaubt, also hoert der Suchlauf dort, wo er nicht fragen darf.
#[allow(clippy::too_many_arguments)]
fn stage5c_scan(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
                h2c: &mut fw::H2cState,
                e: &efuse::Efuse, t: &txpower::TxPower, mac: [u8; 6],
                fw_feature: u32, target: &mut Option<Bss>, d: &mut Dev) -> bool {
    // 2,4 GHz: die dreizehn Kanaele, die es in Europa gibt. Kanal 14 ist
    // nur in Japan und nur mit DSSS zugelassen — er steht bewusst nicht da.
    const ACTIVE_2G: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
    // 5 GHz: UNII-1 bis UNII-3, wie cfg80211 sie fuehrt. Nur zum Hoeren.
    const PASSIVE_5G: [u8; 25] = [
        36, 40, 44, 48, 52, 56, 60, 64,
        100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
        149, 153, 157, 161, 165,
    ];
    // Ein Beacon-Intervall sind 102,4 ms. Wer kuerzer horcht, verpasst ein
    // Netz nicht wegen schwachem Signal, sondern wegen der Uhr.
    const DWELL_MS: u32 = 130;

    host::print("[rtl8822ce] Stufe 5c: der Suchlauf\n");
    if mgmt_buf < 0 {
        host::print("  kein DMA-Puffer fuer die MGMT-Queue\n");
        return false;
    }

    // `rtw_core_scan_start`. Die Adresse steht seit 5b im Port (mac80211
    // reicht hier eine ggf. gewuerfelte durch; wir nehmen unsere eigene).
    // `rtw_leave_lps` und `RTW_FLAG_DIG_DISABLE` sind bei uns wirkungslos —
    // es gibt weder Stromsparen noch eine laufende Verstaerkungsregelung.
    // `rtw_coex_scan_notify` ist dieselbe benannte Luecke wie oben.
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
    // SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF3) };

    let mut found: [Bss; MAX_BSS] = [Bss {
        bssid: [0; 6], ssid: [0; 32], ssid_len: 0,
        channel: 0, best: -128, beacons: 0, resps: 0,
        capability: 0, rsn: [0; 64], rsn_len: 0, ht_param: 0,
    }; MAX_BSS];
    let mut n_found = 0usize;
    let mut probes = 0u32;
    let mut overflow = false;

    let mut frame = [0u8; 64];
    let t_start = host::now_us();
    let mut rf_ok = 0u32;
    let mut rf_bad_first = 0u8;
    let mut n_ch = 0u32;

    for (list, active) in [(&ACTIVE_2G[..], true), (&PASSIVE_5G[..], false)] {
        for &ch in list {
            n_ch += 1;
            if switch_channel(h, hal, e, t, ch, 0, false) {
                rf_ok += 1;
            } else if rf_bad_first == 0 {
                rf_bad_first = ch;
            }

            if active {
                let n = build_probe_req(&mut frame, &mac, ch);
                let f = &frame[..n];
                let mut info =
                    tx::pkt_info_update(f, 0, tx::RTW_BAND_2G);
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
                    // Beacon (0x80) und Probe Response (0x50) tragen
                    // beide denselben Rumpf: 12 feste Bytes, dann Elemente.
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
    // Zurueck auf den Kanal, auf dem die Stufen davor gemessen haben.
    let _ = switch_channel(h, hal, e, t, 1, 0, false);

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

    // **Das Ziel kommt aus `sys/config/wifi_ssid`, nicht aus der
    // Lautstaerke.** docs/spec/WIFI_CLASS_ABI.md sagt warum: ohne
    // SSID-Filter nimmt der Treiber den lautesten AP IRGENDEINES Netzes,
    // auch den des Nachbarn — und fuer den hat `wifid` keinen PSK. Das
    // endet in einem stillen MIC-Fehlschlag, und niemand sieht, woran.
    // Nur 2,4 GHz: dort duerfen wir senden, auf 5 GHz haben wir nur
    // gehorcht.
    // **Eine Datei, `key: value` je Zeile — dieselbe, die `wifid` und
    // `wifi_ax200` lesen.** Bis 0.20.0 stand hier `sys/config/wifi_ssid`;
    // das steht so in einem veralteten Absatz der Spec, und es waere eine
    // ZWEITE Stelle gewesen, die dasselbe konfiguriert. Zwei Stellen
    // driften auseinander, und dann assoziiert der Treiber zu einem Netz,
    // fuer das `wifid` keinen PSK hat.
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

    *target = found[..n_found].iter()
        .filter(|b| b.channel <= 14 && b.ssid_len > 0)
        .filter(|bss| match want {
            Some((a, b)) => bss.ssid_len as usize == b - a
                && bss.ssid[..b - a] == cfg[a..b],
            None => true,
        })
        .max_by_key(|b| b.best)
        .copied();
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
    // **Das Gate misst UNS, nicht die Nachbarschaft.** Ob auf einem Kanal
    // jemand funkt, entscheidet nicht der Treiber — ob der Chip den Kanal
    // angenommen hat, schon. Die Zahl der gefundenen Netze ist ein BEFUND
    // und steht oben.
    ok &= gate("jeder angefahrene Kanal steht danach im RF", rf_ok == n_ch);
    // **Hier gehoert dieses Tor hin und nicht in 5b.** Dass ein Rahmen die
    // Antenne verlaesst, beweist nur eine ANTWORT — und ob auf EINEM Kanal
    // gerade jemand antwortet, ist ein Muenzwurf. Ueber dreizehn Kanaele
    // ist es keiner mehr.
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

/// Einen Beacon oder eine Probe Response in die Liste aufnehmen. Gibt
/// `false` zurueck, wenn kein Platz mehr ist — eine volle Liste ist ein
/// BEFUND und darf nicht wie ein leerer Kanal aussehen.
fn record_bss(found: &mut [Bss; MAX_BSS], n: &mut usize, f: &[u8], ch: u8,
              signal: i8, is_beacon: bool) -> bool {
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&f[16..22]); // addr3

    for b in found[..*n].iter_mut() {
        if b.bssid == bssid {
            if signal > b.best {
                b.best = signal;
            }
            if is_beacon { b.beacons += 1 } else { b.resps += 1 }
            return true;
        }
    }
    if *n >= MAX_BSS {
        return false;
    }
    let e = &mut found[*n];
    e.bssid = bssid;
    e.channel = ch;
    e.best = signal;
    e.beacons = is_beacon as u16;
    e.resps = !is_beacon as u16;
    // 24 Kopf + 8 Zeitstempel + 2 Beacon-Intervall, dann das
    // Faehigkeitsfeld, dann die Elemente.
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
            // 48 = RSN (802.11 §9.4.2.24). Roh behalten, samt Kopf.
            48 if len + 2 <= 64 => {
                e.rsn[..len + 2].copy_from_slice(&f[i..i + 2 + len]);
                e.rsn_len = (len + 2) as u8;
            }
            // 61 = HT Operation (802.11 §9.4.2.56). Byte 0 ist der
            // primaere Kanal, Byte 1 traegt die Lage des Zweitkanals.
            // Wir behalten nur Byte 1 — den Kanal wissen wir, wir
            // standen darauf, als der Beacon hereinkam.
            61 if len >= 2 => {
                e.ht_param = f[i + 3];
            }
            _ => {}
        }
        i += 2 + len;
    }
    *n += 1;
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

/// rtw8822c.c:4179-4186 `rtw8822c_phy_calibration` — Stufe 5d.
///
/// **Sie laeuft hier, weil Linux sie hier laufen laesst.** `rtw_set_channel`
/// setzt nur `need_rfk = true`; ausgefuehrt wird sie in
/// `rtw_chip_prepare_tx`, das mac80211 aus `mgd_prepare_tx` ruft — also
/// nach dem Suchlauf und VOR dem Anmelden. Waehrend des Suchens auf jedem
/// Kanal zu kalibrieren dauert zu lange, und genau das sagt der Kommentar
/// in `main.c`.
#[allow(clippy::too_many_arguments)]
fn stage5d_calibration(h: i32, hal: &Hal, trx: &mut pci::Trx, h2c_buf: i32,
                       h2c: &mut fw::H2cState,
                       e: &efuse::Efuse, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5d: die RF-Kalibrierung\n");
    if h2c_buf < 0 {
        host::print("  kein DMA-Puffer fuer H2C\n");
        return false;
    }

    // `dm_flags` wird in Linux NUR aus debugfs beschrieben; beim Start ist
    // es null, und damit ist keine Kalibrierung abgeschaltet.
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

    // **Die C2H-Antworten der Firmware holt bisher niemand ab.** Auf PCIe
    // kommen sie durch DENSELBEN Empfangsring wie die Funkrahmen, an
    // `pkt_stat.is_c2h` getrennt. Linux liest sie fortwaehrend; bei uns
    // laeuft `rx_poll` nur in den Messfenstern von 5a bis 5c. Was seit dem
    // letzten Fenster aufgelaufen ist, wird hier zuerst geleert — eine
    // Firmware, deren Ausgang keiner leert, ist ein Verdaechtiger fuer
    // jedes „die Firmware antwortet nicht".
    {
        let mut dmx = dm::DmInfo::new();
        let mut pdx = dm::PathDiv::default();
        static mut RXBUF4: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
            [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
        // SAFETY: ein Faden, ein Rufer, der Puffer verlaesst den Block nicht.
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
    // `rtw_load_rfk_table` hat die RFK-Tabelle in Stufe 3c geschrieben und
    // setzt dabei dieses Flag (phy.c:1847). Ohne geladene Tabelle gaebe es
    // keine DPK — die Reihenfolge ist der Grund, nicht ein Sonderfall.
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

    // ── Und die Frage, die zaehlt: hoert der Empfaenger danach noch? ──
    // Eine Kalibrierung, die den Empfang kaputtmacht, ist schlimmer als
    // keine. Dieselbe Messung wie in Stufe 4c, damit die Zahlen
    // vergleichbar sind.
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

/// Stufe 5e — Authentifizierung und Anmeldung.
///
/// **Die Reihenfolge ist Linux' Reihenfolge**: Kanal des Ziels setzen →
/// `rtw_chip_prepare_tx` (die Kalibrierung aus 5d, denn `rtw_set_channel`
/// hat `need_rfk` gesetzt) → `PORT_SET_BSSID` → Auth → Assoc → und bei
/// Erfolg `net_type = RTW_NET_MGD_LINKED` mit der AID in den Port, dazu
/// `rtw_fw_media_status_report`.
///
/// **Was daneben steht und hier NICHT gebaut ist, namentlich:**
/// * `rtw_update_sta_info` + `rtw_fw_send_ra_info` — die Ratenanpassung
///   braucht die HT/VHT-Faehigkeiten des Gegenuebers aus der
///   Anmeldeantwort. Das ist ein Elementeparser der OBEREN Haelfte, und
///   ohne ihn waere jede Ratenmaske geraten.
/// * `rtw_fw_download_rsvd_page` + `rtw_send_rsvd_page_h2c` — PS-Poll,
///   Null- und QoS-Null-Rahmen in den reservierten Seitenbereich. Der Weg
///   dahin steht seit Stufe 2 (`download_firmware` schreibt dort), der
///   INHALT ist obere Haelfte.
/// * `rtw_fw_default_port`, `rtw_coex_media_status_notify`,
///   `rtw_bf_assoc`, `rtw_set_ampdu_factor`, `rtw_fw_beacon_filter_config`.
/// * Der Vierwegehandschlag und der Schluesselspeicher — ab da ist es
///   `wifid`, und eine Anmeldung ohne ihn endet nach wenigen Sekunden in
///   einem Deauth. Das Tor dieser Stufe steht DAVOR.
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

    // `rtw_set_channel` auf den Kanal des Ziels — und **hier zum ersten
    // Mal mit der Breite, die die Zelle ansagt.** Bis 0.32.x stand die
    // Breite auf 20 MHz genagelt, waehrend der Anmeldeantrag
    // `SUP_WIDTH_20_40` versprach und jeder Sendedeskriptor 40 MHz
    // eintrug: drei Stellen, drei Antworten.
    let (cch, bw, _) = chan_params(bss.channel, bss.ht_param, true);
    if !switch_channel(h, hal, e, t, bss.channel, bss.ht_param, true) {
        host::print("  RF 0x18 traegt den Zielkanal NICHT\n");
        return false;
    }
    host::print("  Kanal ");
    host::print_dec(bss.channel as u32);
    host::print(" · Breite ");
    host::print(if bw == 1 { "40 MHz (Mitte K" } else { "20 MHz (K" });
    host::print_dec(cch as u32);
    host::print(")");
    // **Das rohe Byte dazu.** Ohne es sieht „der AP erlaubt kein HT40"
    // genauso aus wie „wir lesen das Element falsch" — und beides endet
    // in derselben Zeile `20 MHz`. Der Zweitkanal steht in Bit 1:0, die
    // Erlaubnis fuer mehr als 20 MHz in Bit 2.
    host::print(" · HT-Operation 0x");
    host::print_hex8(bss.ht_param);
    host::print(" (");
    host::print(match bss.ht_param & 0x03 {
        1 => "Zweitkanal oben",
        3 => "Zweitkanal unten",
        _ => "kein Zweitkanal",
    });
    host::print(if bss.ht_param & 0x04 != 0 {
        ", Breite erlaubt)\n"
    } else {
        ", AP erlaubt nur 20 MHz)\n"
    });

    // `rtw_chip_prepare_tx`: `need_rfk` steht, also wird kalibriert — und
    // zwar auf DIESEM Kanal, nicht auf dem des Suchlaufs.
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

    // `rtw_ops_bss_info_changed`, Zweig `BSS_CHANGED_BSSID`. Ab jetzt
    // nimmt die Hardware Rahmen dieser Zelle an.
    let mut vifc = vif::add_interface_station(h, mac);
    vifc.bssid = bss.bssid;
    vif::port_config(h, &vifc, PORT_SET_BSSID);

    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF5: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
    let rxbuf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF5) };

    let mut frame = [0u8; 256];
    let mut ok = true;

    // ── Authentifizierung (Open System) ──────────────────────────
    // Auch WPA2 authentifiziert OFFEN; die Schluessel kommen erst nach
    // der Anmeldung, im Vierwegehandschlag.
    let n = build_auth_req(&mut frame, &mac, &bss.bssid);
    let (auth_ok, auth_status, auth_tries) =
        exchange(h, hal, trx, mgmt_buf, rxbuf, dm, path_div,
                 &frame[..n], &mac, bss.channel, 0xb0, |f| {
            // Auth-Antwort: Algorithmus, Folge 2, Status.
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

    // ── Anmeldung ────────────────────────────────────────────────
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
    let mut aid = 0u16;
    let (assoc_ok, assoc_status, assoc_tries) =
        exchange(h, hal, trx, mgmt_buf, rxbuf, dm, path_div,
                 &frame[..n], &mac, bss.channel, 0x10, |f| {
            // Anmeldeantwort: Faehigkeiten, Status, AID.
            if f.len() < 30 {
                return None;
            }
            Some(u16::from_le_bytes([f[26], f[27]]))
        });
    if assoc_ok && assoc_status == 0 {
        // Die AID steht im SELBEN Rahmen wie der Status, zwei Byte
        // dahinter. `exchange` traegt nur eine Zahl zurueck, also legt
        // der Leser sie daneben ab.
        // SAFETY: einfaedig, ein Schreiber, ein Leser.
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

/// Die AID der letzten Anmeldeantwort. Sie steht im selben Rahmen wie der
/// Status, und der Rueckgabeweg von `exchange` traegt nur EINE Zahl.
static mut LAST_ASSOC_AID: u16 = 0;

/// Und der ganze Rahmen dazu: Stufe 5f liest daraus die Faehigkeiten des
/// AP (HT, VHT, Raten). In Linux baut mac80211 daraus `ieee80211_sta`.
static mut LAST_ASSOC_RESP: [u8; 256] = [0; 256];
static mut LAST_ASSOC_RESP_LEN: usize = 0;

/// Einen Verwaltungsrahmen senden und auf die Antwort warten.
///
/// Dreimal, mit Abstand: ein einzelner Rahmen kann kollidieren, und ein AP
/// darf ihn verwerfen. Gibt (Antwort gekommen, Status, Versuche) zurueck.
#[allow(clippy::too_many_arguments)]
fn exchange(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
            rxbuf: &mut [u8], dm: &mut dm::DmInfo,
            path_div: &mut dm::PathDiv, frame: &[u8], mac: &[u8; 6],
            channel: u8, want_fc: u8,
            parse: impl Fn(&[u8]) -> Option<u16>) -> (bool, u16, u32)
{
    let queue = tx::RTW_TX_QUEUE_MGMT;
    for tries in 1..=3u32 {
        let mut info = tx::pkt_info_update(frame, 0, tx::RTW_BAND_2G);
        if !pci::tx_write(h, trx, mgmt_buf, queue, &mut info, frame) {
            return (false, 0, tries);
        }
        pci::tx_kick_off_queue(h, trx, queue);
        let (_done, _us, _hw) = pci::tx_wait_consumed(h, trx, queue, 50_000);
        // `rtw_pci_tx_isr` — den Lesezeiger nachziehen, sonst zaehlt der
        // Ring sich ueber mehrere Rahmen voll.
        pci::tx_isr(h, trx, queue);

        let mut status: Option<u16> = None;
        let t0 = host::now_us();
        while host::now_us() - t0 < 300_000 && status.is_none() {
            let n = pci::rx_poll(h, trx, 64, rxbuf, dm, path_div,
                                 hal.rf_path_num, 0, channel, |st, pkt| {
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
                    // Bei der Anmeldeantwort traegt derselbe Rahmen die AID.
                    if want_fc == 0x10 && f.len() >= 32 {
                        // SAFETY: einfaedig, ein Schreiber.
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

/// 802.11 §9.3.3.12 — Authentifizierungsrahmen, Open System, Folge 1.
fn build_auth_req(out: &mut [u8; 256], mac: &[u8; 6], bssid: &[u8; 6])
    -> usize
{
    mgmt_header(out, 0xb0, mac, bssid);
    out[24..26].copy_from_slice(&0u16.to_le_bytes()); // Algorithmus 0 = offen
    out[26..28].copy_from_slice(&1u16.to_le_bytes()); // Folge 1
    out[28..30].copy_from_slice(&0u16.to_le_bytes()); // Status 0
    30
}

/// 802.11 §9.3.3.6 — Anmeldeantrag.
///
/// **Mit HT- und VHT-Element.** Ohne sie nimmt der AP uns als
/// LEGACY-Station an und laesst HT auch in seiner Antwort weg — in 0.19.0
/// kam genau das heraus: `ra_mask 0x0ff5`, keine MCS-Bits, Deckel bei
/// OFDM 54M. Ein Antrag, der weniger anbietet, bekommt weniger.
fn build_assoc_req(out: &mut [u8; 256], mac: &[u8; 6], bss: &Bss,
                   e: &efuse::Efuse, rf_path_num: u8) -> usize {
    mgmt_header(out, 0x00, mac, &bss.bssid);
    // Faehigkeiten: ESS, dazu Privacy und Short Preamble so, wie der AP
    // sie ansagt. Wer hier mehr behauptet, als der AP kann, wird abgelehnt.
    let cap = 0x0001u16 | (bss.capability & 0x0030);
    out[24..26].copy_from_slice(&cap.to_le_bytes());
    out[26..28].copy_from_slice(&10u16.to_le_bytes()); // Listen Interval
    let mut n = 28;

    // SSID
    let sl = bss.ssid_len as usize;
    out[n] = 0;
    out[n + 1] = sl as u8;
    out[n + 2..n + 2 + sl].copy_from_slice(&bss.ssid[..sl]);
    n += 2 + sl;

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

    // Die Elemente stehen in AUFSTEIGENDER Kennung: 45 HT, 48 RSN,
    // 191 VHT. 802.11 verlangt es nicht, aber es kostet nichts, und
    // manche APs sind darin eigen.

    // HT — immer. Es entscheidet, ob wir als 11n-Station angenommen werden.
    n += sta::build_ht_cap_ie(&mut out[n..], e.hw_cap_bw, e.hw_cap_nss);

    // RSN — aus dem, was der AP ansagt, EINE Wahl gebaut.
    if bss.rsn_len > 0 {
        n += build_rsn_ie(&mut out[n..], &bss.rsn[..bss.rsn_len as usize]);
    }

    // VHT nur auf 5 GHz: auf 2,4 GHz ist es nicht zugelassen, und ein AP
    // darf einen Antrag mit VHT im falschen Band ablehnen.
    if bss.channel > 14 {
        n += sta::build_vht_cap_ie(&mut out[n..], e.hw_cap_ptcl,
                                   e.hw_cap_nss, rf_path_num);
    }
    n
}

/// 802.11 §9.4.2.24 — unser RSN-Element.
///
/// **Es ist BYTE-GLEICH mit dem in `wifid`** (`wasm/src/lib.rs:124`), und
/// das ist kein Zufall, sondern ein Vertrag, den die ABI nicht ausdrueckt:
/// der Vierwegehandschlag rechnet seinen MIC ueber GENAU das RSN-Element,
/// das die Station im Anmeldeantrag geschickt hat. Weicht unseres ab,
/// verwirft der AP msg2 — und sagt nicht warum.
///
/// CCMP als Gruppen- und Paarschluessel, PSK als Authentifizierung.
const RSN_IE_WPA2_CCMP_PSK: [u8; 22] = [
    0x30, 0x14, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x04, 0x01, 0x00, 0x00, 0x0f,
    0xac, 0x04, 0x01, 0x00, 0x00, 0x0f, 0xac, 0x02, 0x00, 0x00,
];

/// Unser RSN-Element schreiben — und MELDEN, wenn der AP etwas anderes
/// ansagt, als wir anbieten koennen.
///
/// **Ein Antrag waehlt, ein Beacon zaehlt auf**: das Element des AP nennt
/// alle Verfahren, die er kann; unseres nennt genau eines. Kann er CCMP
/// nicht als Gruppenchiffre, scheitert die Verbindung spaeter im
/// Handschlag — und dann soll hier schon stehen, warum.
fn build_rsn_ie(out: &mut [u8], ap: &[u8]) -> usize {
    const CCMP: [u8; 4] = [0x00, 0x0f, 0xac, 0x04];
    if ap.len() >= 8 && ap[4..8] != CCMP {
        host::print("  [Hinweis] der AP nennt eine andere Gruppenchiffre als\n         \x20         CCMP — der Handschlag wird daran scheitern.\n");
    }
    out[..RSN_IE_WPA2_CCMP_PSK.len()].copy_from_slice(&RSN_IE_WPA2_CCMP_PSK);
    RSN_IE_WPA2_CCMP_PSK.len()
}

/// Der gemeinsame 24-Byte-Kopf eines Verwaltungsrahmens an einen AP.
/// Die Folgenummer bleibt null — `en_hwseq` steht im Sendedeskriptor,
/// also vergibt sie der Chip.
fn mgmt_header(out: &mut [u8; 256], subtype_fc: u8, mac: &[u8; 6],
               bssid: &[u8; 6]) {
    out.fill(0);
    out[0] = subtype_fc;
    out[1] = 0x00;
    out[2..4].copy_from_slice(&0u16.to_le_bytes()); // duration
    out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger
    out[10..16].copy_from_slice(mac); // addr2 = wir
    out[16..22].copy_from_slice(bssid); // addr3 = BSSID
    out[22..24].copy_from_slice(&0u16.to_le_bytes()); // seq
}

/// Stufe 5f — die Ratenanpassung.
///
/// Der Treiber schickt der Firmware KEINE Rate, sondern eine MASKE:
/// welche der 64 Raten dieses Gegenueber kann. Die Firmware waehlt daraus
/// laufend und meldet ihre Wahl als `C2H_RA_RPT` zurueck — und genau das
/// ist das Tor dieser Stufe. Eine Maske, die niemand beantwortet, ist eine
/// Behauptung.
///
/// **Die Maske kommt aus der Anmeldeantwort**, die Stufe 5e aufgehoben
/// hat: HT- und VHT-Element, unterstuetzte Raten. In Linux baut mac80211
/// daraus `ieee80211_sta`; hier steht der Parser in `sta.rs` und gehoert
/// spaeter `wifid`.
///
/// **Nicht gebaut und namentlich:** `rtw_fw_download_rsvd_page` +
/// `rtw_send_rsvd_page_h2c`. Die reservierten Seiten tragen PS-Poll, Null-
/// und QoS-Null-Rahmen, die die FIRMWARE im Stromsparbetrieb selbst
/// sendet. Stromsparen gibt es hier nicht, also wuerden die Seiten
/// geschrieben und nie gelesen. Sie gehoeren zu LPS, nicht hierher.
#[allow(clippy::too_many_arguments)]
fn stage5f_rates(h: i32, trx: &mut pci::Trx, h2c: &mut fw::H2cState,
                 hal: &Hal, e: &efuse::Efuse, vifc: &vif::Vif, bss: &Bss,
                 out: &mut Option<(sta::PeerCaps, sta::StaInfo)>, d: &mut Dev) -> bool {
    host::print("[rtl8822ce] Stufe 5f: die Ratenanpassung\n");

    // SAFETY: einfaedig, und 5e hat vorher geschrieben.
    let (resp, len) = unsafe {
        (&*core::ptr::addr_of!(LAST_ASSOC_RESP), LAST_ASSOC_RESP_LEN)
    };
    if len < 30 {
        host::print("  keine Anmeldeantwort aufgehoben\n");
        return false;
    }

    let mut caps = sta::parse_assoc_resp(&resp[..len]);

    // **Die Breite einer Station ist das MINIMUM aus ihrem Koennen und der
    // Zelle** (mac80211 `ieee80211_sta_cur_vht_bw`). `parse_assoc_resp`
    // kennt nur das Koennen — der Kommentar dort sagt es woertlich: „Ohne
    // die Zelle bleibt das, was das Gegenueber kann." Hier haben wir die
    // Zelle, also gehoert die Klemme hierher.
    //
    // Ohne sie trug jeder Sendedeskriptor 40 MHz, waehrend die PHY auf 20
    // stand. Ein Deskriptor, der eine andere Breite behauptet als das
    // Funkteil fuehrt, ist kein Schoenheitsfehler: die Firmware waehlt
    // ihre Raten danach.
    let (_, zellen_bw, _) = chan_params(bss.channel, bss.ht_param, true);
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
    // **Die Stroemezahl hat EINE Quelle, und das ist die efuse.** Linux
    // fragt an beiden Stellen `efuse->hw_cap.nss` (main.c:1248 fuer die
    // Ratenmaske, main.c:1313 fuer `tx_num`); hier stand
    // `if hal.rf_2t2r { 2 }`. Auf diesem Chip ist das meist dasselbe --
    // aber es ist dieselbe Bauart wie die Bandbreite, die an drei Stellen
    // stand und dreimal etwas anderes sagte, und das hat uns einen Faktor
    // drei gekostet. `rf_2t2r` sagt, was der Funkteil HAT; `hw_cap.nss`
    // sagt, was diese KARTE fuehren darf, und nur das zweite steht auch im
    // Anmeldeantrag (`build_ht_cap_ie` nimmt es seit je).
    let nss = e.hw_cap_nss;
    let wireless_set = sta::update_sta_info(&mut si, &caps, nss,
                                            bss.channel <= 14);

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

    // ── Und jetzt zuhoeren, was die Firmware daraus macht ────────
    let dm = &mut d.dm;
    let path_div = &mut d.path_div;
    chip::read_cck_gi_bnd(h, dm);
    static mut RXBUF6: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    // SAFETY: ein Faden, ein Rufer, der Puffer verlaesst die Funktion nicht.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RXBUF6) };

    let mut ra_rpt = 0u32;
    let mut last_rate = 0u8;
    let mut last_sgi = false;
    let mut last_bw = 0u8;
    let mut c2h_total = 0u32;
    let t0 = host::now_us();
    while host::now_us() - t0 < 2_000_000 {
        let n = pci::rx_poll(h, trx, 64, buf, dm, path_div,
                             hal.rf_path_num, 0, bss.channel, |st, pkt| {
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

/// main.h:250-262 `DESC_RATE*` als Namen — fuer den Bericht.
/// **Der genaue Name, nicht die Klasse.** Bis 0.31.0 stand hier
/// „HT MCS8-15" fuer acht verschiedene Raten — fuer die Frage „mit
/// welcher Rate laeuft die Leitung wirklich" ist das keine Antwort.
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

/// Die Zaehler der Verbindung. Sie gehoeren dem LINK, nicht der Stufe —
/// 6b setzt fort, wo 6a aufgehoert hat, und ein Zaehler, der dabei auf
/// null springt, ist eine Luege ueber die Leitung.
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
    /// **Der Gruppen-Neuschluessel, gezaehlt statt vermutet.** Jedes
    /// EAPOL NACH dem Handschlag ist einer (msg1 der
    /// Gruppenschluessel-Sequenz, oder ein ganz neues Vierwege), und
    /// jede Antwort darauf zaehlt daneben. „3 empfangen, 0 beantwortet"
    /// heisst an uns; „3/3" und trotzdem Rauswurf heisst woanders — und
    /// der Grundcode sagt dann wo.
    rekey_rx: u32,
    rekey_tx: u32,
    /// Jede GTK, die `wifid` uns ins CAM schreiben laesst.
    gtk_set: u32,
    /// Was die Empfangsschleife gesehen hat und die Schleife DANACH
    /// behandelt: `(war es ein Deauth, Grundcode)`. Im Rueckruf steht
    /// nur das Sehen — `netdev_set_link` und `EV_LINK_DOWN` gehoeren
    /// nicht in einen Rueckruf, der mitten im Ringleeren laeuft.
    gone: Option<(bool, u16)>,
    /// Wie oft wir hinausgeworfen wurden, und womit zuletzt begruendet.
    kicked: u32,
    last_reason: u16,
    /// **Die Sendequittung der Firmware** (`rtw_tx_report_*`, tx.c).
    /// Bis 0.26.0 wussten wir von KEINEM gesendeten Rahmen, ob er
    /// ankam — genau der Beobachter, der bei zwei Fehlern hintereinander
    /// gefehlt hat.
    probes: [TxProbe; TX_PROBE_SLOTS],
    probe_sn: u8,
    /// quittiert · nicht quittiert · gar keine Antwort der Firmware
    tx_acked: u32,
    tx_lost: u32,
    tx_no_report: u32,
    /// Die Firmware hat sich selbst fuer tot erklaert.
    fw_crash: u32,
    /// Wie oft wir die Verbindung neu aufgebaut haben.
    reconnects: u32,
    /// **Der Zensus der unbehandelten C2H-Kennungen.** Vier Plaetze,
    /// jeder `(Kennung, Anzahl)` — mehr verschiedene schickt diese
    /// Firmware nicht, und die haeufigste ist die interessante. Bis
    /// 0.27.2 wurden sie verworfen, und die ausbleibende Sendequittung
    /// war dadurch eine Null ohne Hinweis.
    c2h_ids: [(u8, u32); 4],
    /// Verwaltungsrahmen unserer Zelle, nach Subtyp gezaehlt (16
    /// Plaetze, einer je Subtyp — die Liste ist abgeschlossen).
    mgmt_sub: [u32; 16],
    /// Und fuer Action-Rahmen die Kategorie/Aktion des letzten sowie
    /// die Zahl der **ADDBA Requests** — die Frage dieser Runde.
    addba_req: u32,
    last_action: (u8, u8),
    /// Wie oft wir zugestimmt haben — und wie oft die Antwort nicht in
    /// den Sendering passte.
    addba_resp: u32,
    addba_fail: u32,
    /// Und die GEGENrichtung: wie oft wir selbst gefragt haben, ob der
    /// Block steht, und mit welchem Status der AP abgelehnt hat.
    /// **`addba_req_sent > 0` bei `tx_ba_ok == false` heisst: wir haben
    /// gefragt und keine Antwort bekommen** — ein anderer Zustand als
    /// „nie gefragt", und ohne die zwei Zahlen sehen beide gleich aus.
    addba_req_sent: u32,
    tx_ba_ok: bool,
    tx_ba_status: u16,
    /// Das Fenster, das wir zuletzt zugestanden haben, und das, um das
    /// gebeten wurde. **Ohne die Zahl im Bericht ist nicht zu sehen, ob
    /// eine geaenderte `ampdu:`-Zeile ueberhaupt gelesen wurde** — der
    /// Treiber liest sie einmal beim Start der Schleife.
    addba_win: u16,
    addba_win_req: u16,
    /// Die Form der Empfangsschleife: Bliecke mit und ohne Beute, die
    /// Summe der Rahmen, und wie oft ein Blick den Stapel voll
    /// ausschoepfte.
    rx_polls: u32,
    rx_empty: u32,
    rx_frames: u32,
    rx_full: u32,
    /// **Das Ratenhistogramm ueber die GANZE Verbindung.**
    ///
    /// Linux fuehrt `cur_pkt_count.num_qry_pkt[rate]` je Watchdog-Takt
    /// und schiebt es nach `last_pkt_count`; debugfs liest es LAUFEND
    /// mit. Wir haben kein debugfs — ein Bericht, den jemand NACH einer
    /// Uebertragung liest, braucht eine Zahl, die sie ueberlebt.
    ///
    /// Und er braucht sie dringend: `curr_rx_rate` ist die Rate des
    /// LETZTEN Rahmens, und eine halbe Sekunde nach einem Download ist
    /// das ein Beacon — die gehen auf der niedrigsten Grundrate. Der
    /// Bericht zeigte deshalb „OFDM 6M", waehrend die Daten mit etwas
    /// ganz anderem kamen.
    /// Wanduhrzeit in `rx_poll`, wenn es etwas brachte, und wie lange
    /// die Schleife insgesamt laeuft. Ihr Verhaeltnis ist die
    /// Auslastung des Empfangspfades.
    rx_us: u64,
    pump_us0: u64,
    /// **Was die Luft kaputt macht**, aufsummiert: `false_alarm_statistics`
    /// liest je Modulation einen CRC-Zaehler und SETZT IHN ZURUECK. Eine
    /// Momentaufnahme sagt darueber nichts; die Summe ueber die
    /// Verbindung sagt, ob der AP staendig wiederholen muss.
    ht_ok: u64,
    ht_err: u64,
    ofdm_ok: u64,
    ofdm_err: u64,
    rate_hist: [u32; DESC_RATE_MAX],
    /// Wie oft die Firmware ihre Ratenwahl gemeldet hat (`C2H_RA_RPT`).
    /// **Null hiesse: `dm.tx_rate` steht auf 0 = CCK 1M**, und damit
    /// waehlt `config_swing_table` die CCK-Kurve der
    /// Sendeleistungs-Nachfuehrung.
    ra_rpt_n: u32,
}

impl Default for LinkStats {
    /// **Von Hand, weil `[u32; 84]` kein `Default` hat** (die Ableitung
    /// reicht nur bis 32). `zeroed` waere hier richtig und trotzdem
    /// falsch: ein `unsafe` fuer eine Struktur aus lauter Zahlen und
    /// `bool` spart nichts und verpflichtet den naechsten Leser.
    fn default() -> Self {
        LinkStats {
            eapol_rx: 0, eapol_tx: 0, keys_set: 0, data_rx: 0, data_tx: 0,
            authorized: false, link_up_sent: false, extra_reported: 0,
            llc_miss: 0, rx_wd: 0, rekey_rx: 0, rekey_tx: 0, gtk_set: 0,
            gone: None, kicked: 0, last_reason: 0,
            probes: [TxProbe { sn: 0, at_ms: 0, busy: false }; TX_PROBE_SLOTS],
            probe_sn: 0, tx_acked: 0, tx_lost: 0, tx_no_report: 0,
            fw_crash: 0, reconnects: 0, c2h_ids: [(0, 0); 4],
            mgmt_sub: [0; 16], addba_req: 0, last_action: (0, 0),
            addba_resp: 0, addba_fail: 0,
            addba_req_sent: 0, tx_ba_ok: false, tx_ba_status: 0,
            addba_win: 0, addba_win_req: 0,
            rx_polls: 0, rx_empty: 0, rx_frames: 0, rx_full: 0,
            rx_us: 0, pump_us0: 0,
            ht_ok: 0, ht_err: 0, ofdm_ok: 0, ofdm_err: 0,
            rate_hist: [0; DESC_RATE_MAX], ra_rpt_n: 0,
        }
    }
}

impl LinkStats {
    /// tx.c:166-211 `rtw_tx_report_enable` + `rtw_tx_report_enqueue` in
    /// einem: Nummer vergeben und Platz belegen.
    ///
    /// Gibt `None`, wenn alle acht Plaetze belegt sind — dann antwortet
    /// die Firmware ohnehin nicht, und eine neunte Frage macht es nicht
    /// besser.
    fn arm_probe(&mut self, now: u64) -> Option<u8> {
        let slot = self.probes.iter().position(|p| !p.busy)?;
        let sn = tx::report_seqnum(&mut self.probe_sn);
        self.probes[slot] = TxProbe { sn, at_ms: now, busy: true };
        Some(sn)
    }

    /// tx.c:229-256 `rtw_tx_report_handle` — die Antwort zuordnen.
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

    /// Einen Verwaltungsrahmen zaehlen.
    fn note_mgmt(&mut self, subtype: u8, cat: u8, action: u8) {
        self.mgmt_sub[(subtype & 0xf) as usize] += 1;
        if cat != 0xff {
            self.last_action = (cat, action);
            if cat == DOT11_ACTION_CAT_BA && action == DOT11_ACTION_ADDBA_REQ {
                self.addba_req += 1;
            }
        }
    }

    /// Eine unbehandelte C2H-Kennung zaehlen. Vier Plaetze, danach nur
    /// noch die, die schon dastehen — der Zensus soll die haeufigste
    /// finden, nicht jede einzelne.
    fn note_c2h(&mut self, id: u8) {
        if let Some(e) = self.c2h_ids.iter_mut().find(|e| e.1 > 0 && e.0 == id) {
            e.1 += 1;
            return;
        }
        if let Some(e) = self.c2h_ids.iter_mut().find(|e| e.1 == 0) {
            *e = (id, 1);
        }
    }

    /// tx.c:179-194 `rtw_tx_report_purge_timer` — „failed to get tx
    /// report from firmware". Eine Frist, keine Rundenzahl.
    fn purge_probes(&mut self, now: u64) {
        for p in self.probes.iter_mut() {
            if p.busy && now.wrapping_sub(p.at_ms) > RTW_TX_PROBE_TIMEOUT_MS {
                p.busy = false;
                self.tx_no_report += 1;
            }
        }
    }
}

/// tx.c `struct rtw_tx_report` — die Rahmen, deren Quittung aussteht.
///
/// **Linux haengt dafuer die `sk_buff`s in eine Warteschlange**, weil es
/// sie danach an mac80211 zurueckgibt. Wir brauchen den Rahmen nicht
/// mehr, nur die Frage „ist er angekommen?" — also eine Folgenummer und
/// wann gefragt wurde. Acht Plaetze: mehr als acht offene Quittungen
/// hiesse, dass die Firmware gar nicht antwortet, und dann sagt das der
/// Zaehler `tx_no_report`.
#[derive(Clone, Copy, Default)]
struct TxProbe {
    sn: u8,
    at_ms: u64,
    busy: bool,
}

const TX_PROBE_SLOTS: usize = 8;

/// Der Zustand einer stehenden Verbindung — Stufe 6a.
struct Link {
    bssid: [u8; 6],
    mac: [u8; 6],
    channel: u8,
    si: sta::StaInfo,
    highest_rate: u8,
    /// Laufende Folgenummer fuer Datenrahmen. Der Chip vergibt sie bei
    /// `en_hwseq` selbst, aber `pkt_info.seq` steht trotzdem im
    /// Deskriptor — Linux fuellt es aus dem Rahmenkopf.
    seq: u16,
    ptk_installed: bool,
    /// **Der SENDE-Block, TID 0.** `None` = nicht ausgehandelt, also
    /// einzelne Rahmen. `Some(fenster)` = der AP hat mit Status 0
    /// geantwortet, und ab dann traegt jeder Datenrahmen `ampdu_en`.
    ///
    /// rtw88 fuehrt das als Flagbit `RTW_TXQ_AMPDU` je Sendeschlange
    /// (mac80211.c `rtw_ops_ampdu_action`, Zweig `TX_OPERATIONAL`) — wir
    /// haben keine Schlangen je TID, also steht es hier.
    tx_ampdu: Option<u16>,
    /// Ob der AP HT kann, und sein A-MPDU-Parameterbyte. Beides aus
    /// seinem HT-Element; ohne HT gibt es keinen Block, und die zwei
    /// Werte im Deskriptor sagen, wie LANG und wie DICHT er ihn vertraegt.
    peer_ht: bool,
    peer_ampdu_param: u8,
    /// **Ob wir QoS-Datenrahmen senden — die Voraussetzung fuer jede
    /// Sende-Aggregation.** Ein HT-AP ist per Definition ein QoS-AP
    /// (802.11 §10.1: eine HT-STA ist eine QoS-STA), also entscheidet
    /// dasselbe Element ueber beides. Gegen einen AP ohne HT bleibt es
    /// beim einfachen Datenrahmen, und dann gibt es auch keinen Block.
    tx_qos: bool,
    /// Wie oft wir gefragt haben, und wann zuletzt. Ein AP, der schweigt,
    /// darf uns nicht in eine Endlosschleife schicken.
    addba_tries: u8,
    addba_last_ms: u64,
    addba_token: u8,
    /// 802.11 §12.5.3.2 — die 48-Bit-Paketnummer des Paarschluessels.
    /// Sie faengt bei eins an und zaehlt je Rahmen hoch; eine wiederholte
    /// Nummer verwirft der AP als Wiedereinspielung.
    tx_pn: u64,
    cam: [sec::CamEntry; 4],
}

/// 802.11 §12.5.3.2 — der acht Byte lange CCMP-Kopf.
///
/// Die Paketnummer steht in zwei Stuecken, und das ist kein Versehen der
/// Spezifikation: Byte 2 ist reserviert und Byte 3 traegt das ExtIV-Bit
/// und die Schluesselnummer, damit ein alter WEP-Empfaenger den Rahmen
/// als erweitert erkennt.
fn ccmp_hdr(out: &mut [u8], pn: u64, key_id: u8) {
    out[0] = (pn & 0xff) as u8; // PN0
    out[1] = ((pn >> 8) & 0xff) as u8; // PN1
    out[2] = 0; // reserviert
    out[3] = 0x20 | (key_id << 6); // ExtIV | KeyID
    out[4] = ((pn >> 16) & 0xff) as u8; // PN2
    out[5] = ((pn >> 24) & 0xff) as u8; // PN3
    out[6] = ((pn >> 32) & 0xff) as u8; // PN4
    out[7] = ((pn >> 40) & 0xff) as u8; // PN5
}

/// Den 802.11-Datenrahmen BAUEN — rein, ohne Chip, ohne `Link`.
///
/// **Herausgeloest, weil 0.36.0 genau hier scheiterte und kein Test es
/// sehen konnte.** Der Fehler war ein fehlendes FELD in einem Rahmen, den
/// wir selbst bauen, und `datapath.py` zaehlt fehlende FUNKTIONEN. Was
/// eine Funktion zusammensetzt, muss sich Byte fuer Byte nachrechnen
/// lassen; deshalb steht das Bauen hier und das Senden daneben.
///
/// **QoS oder nicht, und warum das mehr ist als zwei Byte:** ein
/// Block-Ack gilt je TID (802.11 §11.5.1.1), und einen TID traegt nur ein
/// QoS-Datenrahmen. Ohne `qos` darf es keine Sende-Aggregation geben.
/// Mit `qos` waechst der Kopf auf 26 Byte, und alles dahinter — CCMP-Kopf,
/// LLC/SNAP, Nutzlast — rueckt mit.
///
///     ohne QoS, klar:          [fc 2][dur 2][a1 6][a2 6][a3 6][seq 2]
///     mit QoS:                 ... [seq 2][qos 2]
///     verschluesselt:          ... + [ccmp 8]
///     dann immer:              [LLC/SNAP 6][ethertyp 2][nutzlast]
///
/// Die Ack-Politik im QoS-Feld ist 0 (Normal Ack) und der A-MSDU-Bit ist
/// null — wir fassen keine MSDUs zusammen, nur MPDUs.
#[allow(clippy::too_many_arguments)]
fn build_data_frame(out: &mut [u8; 2048], eth: &[u8], bssid: &[u8; 6],
                    mac: &[u8; 6], seq: u16, encrypt: bool, pn: u64,
                    qos: bool) -> Option<usize> {
    if eth.len() < 14 {
        return None;
    }
    let payload = &eth[14..];
    let hdr = 24 + if qos { 2 } else { 0 };
    let total = hdr + if encrypt { 8 } else { 0 } + 6 + 2 + payload.len();
    if total > out.len() {
        return None;
    }

    out[0] = DOT11_FC_TYPE_DATA | if qos { DOT11_FC0_QOS } else { 0 };
    out[1] = 0x01; // ToDS
    if encrypt {
        out[1] |= DOT11_FC_PROTECTED;
    }
    out[2..4].copy_from_slice(&0u16.to_le_bytes());
    out[4..10].copy_from_slice(bssid); // addr1 = Empfaenger
    out[10..16].copy_from_slice(mac); // addr2 = Quelle
    out[16..22].copy_from_slice(&eth[0..6]); // addr3 = Ziel
    out[22..24].copy_from_slice(&((seq & 0x0fff) << 4).to_le_bytes());
    if qos {
        // TID 0 (Best Effort), Normal Ack, kein EOSP, kein A-MSDU.
        out[24..26].copy_from_slice(&0u16.to_le_bytes());
    }

    // **Der CCMP-Kopf wird vom TREIBER geschrieben, nicht von der
    // Hardware.** `rtw_ops_set_key` setzt `IEEE80211_KEY_FLAG_GENERATE_IV`,
    // und das heisst in mac80211: der Stapel macht acht Byte Platz und
    // schreibt die Paketnummer hinein (`ccmp_pn2hdr`), die Hardware
    // verschluesselt nur. Ohne ihn stehen unsere Rahmen fuer den AP nicht
    // zur Entschluesselung bereit — und das sieht aus wie eine Leitung,
    // auf der nichts zurueckkommt.
    let ofs = if encrypt {
        ccmp_hdr(&mut out[hdr..hdr + 8], pn, 0);
        hdr + 8
    } else {
        hdr
    };
    out[ofs..ofs + 6].copy_from_slice(&LLC_SNAP_HDR);
    out[ofs + 6..ofs + 8].copy_from_slice(&eth[12..14]); // Ethertyp
    out[ofs + 8..ofs + 8 + payload.len()].copy_from_slice(payload);
    Some(total)
}

/// docs/spec/WIFI_CLASS_ABI.md §2b — ein Ethernet-Rahmen als
/// 802.11-Datenrahmen an den AP.
///
/// 802.3: `[DA 6][SA 6][ethertype 2][Nutzlast]`
/// 802.11 ToDS: `[fc 2][dur 2][addr1=BSSID][addr2=SA][addr3=DA][seq 2]`
/// plus LLC/SNAP (RFC 1042) und den Ethertyp.
///
/// **Gewoehnliche Daten, kein QoS.** Unser Anmeldeantrag trug kein
/// WMM-Element, also hat der AP uns als Nicht-QoS-Station angenommen —
/// ein QoS-Rahmen waere jetzt falsch. Das haengt zusammen: wer WMM
/// anbietet, muss QoS senden, und wer es nicht anbietet, darf nicht.
/// `probe` ist `IEEE80211_TX_CTL_REQ_TX_STATUS` — Linux setzt es aus
/// mac80211 fuer die Rahmen, deren Verlust die Verbindung kostet
/// (Steuerport, also EAPOL). Gibt die Folgenummer zurueck, unter der
/// die Firmware antworten wird.
fn tx_8023(h: i32, trx: &mut pci::Trx, mgmt_buf: i32, link: &mut Link,
           eth: &[u8], encrypt: bool, probe: Option<u8>) -> bool {
    if eth.len() < 14 {
        return false;
    }
    let mut frame = [0u8; 2048];
    let Some(total) = build_data_frame(&mut frame, eth, &link.bssid,
                                       &link.mac, link.seq, encrypt,
                                       link.tx_pn, link.tx_qos)
    else {
        return false;
    };
    if encrypt {
        link.tx_pn = link.tx_pn.wrapping_add(1);
    }

    let mut info = tx::TxPktInfo::default();
    // `rtw_tx_pkt_info_update` fuer einen Datenrahmen: erst die Rate,
    // dann die gemeinsamen Felder.
    tx::data_pkt_info_update(&mut info, link.seq, Some(&link.si),
                             link.highest_rate, link.tx_ampdu,
                             link.peer_ampdu_param);
    let a1 = &frame[4..10];
    info.bmc = a1.iter().all(|&b| b == 0xff) || a1[0] & 0x01 != 0;
    info.tx_pkt_size = total as u32;
    info.offset = tx::TX_PKT_DESC_SZ as u8;
    info.ls = true;
    info.mac_id = link.si.mac_id;
    // `rtw_tx_pkt_info_update_sec`: mit installiertem Schluessel traegt der
    // Deskriptor die acht Byte des CCMP-Kopfes als zusaetzliche Laenge.
    if encrypt {
        info.sec_type = 0x3; // AES
    }

    link.seq = link.seq.wrapping_add(1) & 0x0fff;

    // tx.c:432-433 `if (info->flags & IEEE80211_TX_CTL_REQ_TX_STATUS)`.
    // Die Nummer vergibt der Rufer (`rtw_tx_report_enable`), weil er sie
    // gleich darauf in seine offene Liste eintraegt.
    if let Some(sn) = probe {
        info.sn = sn as u16;
        info.report = true;
    }

    let queue = pci::Q_BE;
    if !pci::tx_write(h, trx, mgmt_buf, queue, &mut info, &frame[..total]) {
        return false;
    }
    pci::tx_kick_off_queue(h, trx, queue);
    true
}

/// Wo der LLC/SNAP-Kopf eines Datenrahmens steht, und wieviel hinten
/// nicht dazugehoert.
///
/// **Ein verschluesselter Rahmen traegt acht Byte CCMP-Kopf zwischen dem
/// 802.11-Kopf und den Nutzdaten**, und die Hardware entfernt ihn NICHT:
/// `rtw_rx_fill_rx_status` setzt `RX_FLAG_DECRYPTED`, aber nicht
/// `RX_FLAG_IV_STRIPPED` — in Linux raeumt mac80211 ihn weg. Hinten haengen
/// die Pruefsumme (immer) und bei CCMP der acht Byte lange MIC, beides
/// weil `WLAN_RCR_CFG` APP_FCS und APP_MIC gesetzt hat.
///
/// Findet sich LLC/SNAP nicht an der gerechneten Stelle, wird an den zwei
/// anderen moeglichen gesucht und das GEMELDET. Ein stiller Fehlgriff
/// hier verwirft jeden Rahmen und sieht aus wie eine tote Leitung.
fn llc_offset(f: &[u8], miss: &mut u32) -> Option<(usize, usize)> {
    let qos = f[0] & DOT11_FC0_QOS != 0;
    let hdrlen = 24 + if qos { 2 } else { 0 };
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

/// **Der Rauswurf, und warum er bisher unsichtbar war.**
///
/// `rx_to_8023` filtert in seiner ERSTEN Zeile auf Datenrahmen. Ein
/// Deauth ist ein VERWALTUNGSrahmen und faellt dort lautlos durch: aus
/// Treibersicht stirbt die Verbindung nicht, sie wird nur still — und
/// genau deshalb wirkt ein Rauswurf zufaellig. Der Kernel glaubt
/// derweil weiter an `carrier UP` und schiebt Pakete in eine tote
/// Leitung.
///
/// 802.11 §9.4.1.7: Deauthentication (Subtyp 12) und Disassociation
/// (Subtyp 10) tragen einen Grundcode, little-endian, direkt hinter dem
/// 24 Byte langen Kopf. Gibt `(war es ein Deauth, Grundcode)` zurueck.
///
/// **Nur von `addr2 == BSSID`.** Die Luft ist voll; der Deauth einer
/// fremden Zelle geht uns nichts an, und ein Treiber, der auf ihn
/// hoert, legt seine eigene Verbindung wegen des Nachbarn nieder.
fn disconnect_reason(f: &[u8], bssid: &[u8; 6]) -> Option<(bool, u16)> {
    // 24 Byte Kopf + 2 Byte Grund. Kuerzer ist kein gueltiger Rahmen,
    // und raten waere hier schlimmer als schweigen.
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

/// 802.11 §9.4.1.7 Tabelle 9-49. **Die 15 und die 16 sind die Frage
/// dieser Runde**: sie waeren die Bestaetigung, dass es am Handschlag
/// bzw. am Gruppen-Neuschluessel haengt und nicht an der Luft.
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

/// docs/spec/WIFI_CLASS_ABI.md §2b, Demux-Regel: ein empfangener
/// 802.11-Datenrahmen wird zu 802.3 und geht dann entweder als `EAPOL_RX`
/// an `wifid` oder in den IP-Stapel.
///
/// Gibt die Laenge des 802.3-Rahmens in `out` zurueck und ob es EAPOL war.
fn rx_to_8023(f: &[u8], out: &mut [u8], miss: &mut u32)
    -> Option<(usize, bool)>
{
    if f.len() < 24 || f[0] & 0x0c != DOT11_FC_TYPE_DATA {
        return None;
    }
    // Null und QoS-Null tragen keinen Rumpf.
    if f[0] & DOT11_FC0_NODATA != 0 {
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
    // FromDS: addr1 = wir, addr2 = BSSID, addr3 = Quelle.
    out[0..6].copy_from_slice(&f[4..10]);
    out[6..12].copy_from_slice(&f[16..22]);
    out[12..14].copy_from_slice(&f[llc + 6..llc + 8]);
    out[14..14 + payload.len()].copy_from_slice(payload);
    Some((14 + payload.len(), et == ETHERTYPE_EAPOL))
}

/// PLATZ
///
/// **Hier hoert der Stufentest auf und der Treiber faengt an.** Bis 5f
/// arbeitete `main` eine Kette ab und schaltete den Chip aus; hier laeuft
/// eine Schleife: Empfangsring leeren, Sendequittungen einsammeln,
/// Kommandos von `wifid` ausfuehren, Ereignisse hinaufmelden.
///
/// **Den Handschlag rechnet `wifid`, nicht wir** — er ist
/// herstellerunabhaengig und steht einmal da
/// (`tools/wasm/wifid/core/src/eapol.rs`). Der Treiber transportiert die
/// Rahmen und schreibt die fertigen Schluessel in den Speicher. Genau so
/// steht es in `docs/spec/WIFI_CLASS_ABI.md` §1: der Treiber sieht nie
/// den PSK.
#[allow(clippy::too_many_arguments)]
/// Den Link aufbauen: beim Kernel anmelden und `wifid` scharf machen.
///
/// **Das darf genau EINMAL geschehen.** Ein zweites `EV_READY` laesst
/// `wifid` einen frischen Supplicant bauen, der auf ein msg1 wartet, das
/// der AP nie wieder schickt — genau das ist in 0.23.0 passiert, weil
/// Stufe 6b die Funktion von 6a ein zweites Mal rief.
fn link_setup(hal: &Hal, bss: &Bss, caps: &sta::PeerCaps, si: sta::StaInfo,
              mac: [u8; 6]) -> Link {
    let link = Link {
        bssid: bss.bssid,
        mac,
        channel: bss.channel,
        si,
        highest_rate: if caps.ht_supported {
            tx::highest_ht_tx_rate(&caps.ht_mcs, hal.rf_2t2r)
        } else {
            0x0b // DESC_RATE54M
        },
        seq: 0,
        ptk_installed: false,
        tx_ampdu: None,
        peer_ht: caps.ht_supported,
        peer_ampdu_param: caps.ht_ampdu_param,
        tx_qos: caps.ht_supported,
        addba_tries: 0,
        addba_last_ms: 0,
        addba_token: 0x10,
        tx_pn: 1,
        cam: [sec::CamEntry::default(); 4],
    };

    // Der Datenkanal existiert seit Kernel 0.205.0; ohne Anmeldung sieht
    // ihn der IP-Stapel nicht.
    let reg = host::netdev_register(&mac);
    host::print("  netdev_register ");
    host::print(if reg >= 0 { "ok" } else { "FEHLGESCHLAGEN" });
    host::print("\n");

    // `EV_READY` = [0x83][ap_mac 6][our_mac 6] — damit baut `wifid` seinen
    // Supplicant fuer GENAU diese Zelle.
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

/// main.c:224-310 `rtw_watch_dog_work` — **alle zwei Sekunden, das
/// ganze Leben einer Verbindung lang.**
///
/// Bis 0.26.0 gab es sie nicht. Gebaut war der Aufbau, und danach blieb
/// der Chip sich selbst ueberlassen: kein Quarz-Nachziehen, keine
/// Sendeleistung ueber die Temperatur, keine Vorverzerrungs-Nachfuehrung,
/// keine RSSI an die Ratenwahl der Firmware. Drei davon sind SENDEseite,
/// und ein Empfaenger rastet sich an jeder Praeambel neu ein — ein
/// Sender nicht. Das ist die Form, in der eine Leitung einseitig wird,
/// ohne dass irgendwo ein Fehler steht.
///
/// **Vier Posten aus Linux stehen hier NICHT, und jeder hat seinen
/// Grund:**
///
/// * `rtw_leave_lps` / `rtw_enter_lps` / `rtw_recalc_lps` — wir fahren
///   kein Power-Save (offener Posten im Plan).
/// * `rtw_hci_dynamic_rx_agg` — `.dynamic_rx_agg = NULL` fuer PCI
///   (pci.c:1605), also auf unserem Bus ein Nichts.
/// * `rtw_dynamic_csi_rate` — kehrt um, solange die Gegenstelle keine
///   Beamforming-Rolle hat; wir bauen `bf.c` nicht.
/// * `rtw_coex_run_coex` (ueber `wl_status_change_notify`) — der
///   Entscheidungsbaum der Koexistenz ist L6 des Plans, 111 Funktionen,
///   eigene Stufe.
///
/// **Eine Abweichung, die hier stehen MUSS:** `rtw_coex_monitor_bt_enable`
/// wird in Linux nur aus `rtw_coex_run_coex` gerufen. Sie erzeugt
/// `bt_disabled`, und daran haengt `rtw8822c_cfo_need_adjust`. Ohne sie
/// bliebe die Zahl auf ihrem Anfangswert stehen, der Riegel zu und die
/// Quarznachfuehrung fuer immer aus — ein Tor, das nie aufgeht. Also
/// wird sie hier gerufen, bis L6 steht.
#[allow(clippy::too_many_arguments)]
fn watch_dog(h: i32, hal: &Hal, d: &mut Dev, h2c: &mut fw::H2cState,
             e: &efuse::Efuse, link: &mut Link, caps: &sta::PeerCaps,
             fw_feature: u32, linked: bool, beacon_int: u16) {
    let received_beacons = d.dm.cur_pkt_count.num_bcn_pkt;

    // main.c:241-248 — die Schwelle ist 100 Rahmen je Takt.
    let busy_pre = d.busy_traffic;
    d.busy_traffic = d.stats.tx_cnt > RTW_BUSY_TRAFFIC_THRESHOLD
        || d.stats.rx_cnt > RTW_BUSY_TRAFFIC_THRESHOLD;
    if busy_pre != d.busy_traffic {
        // `rtw_coex_wl_status_change_notify(rtwdev, 0)` -> run_coex (L6)
    }

    // main.c:255-268 — Bytes je zwei Sekunden in Mbit/s, geglaettet.
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

    // main.c:275-279
    coex::wl_status_check(h, &mut d.cx);
    coex::monitor_bt_enable(h, &mut d.cx);
    coex::active_query_bt_info(h, &mut d.cx);

    let band_2g = link.channel <= 14;
    // `si->ra_report.desc_rate` — was die FIRMWARE zuletzt gewaehlt hat,
    // nicht was wir angeboten haben. Sie meldet es als C2H `RA_RPT`;
    // solange keiner kam, steht dort die Anfangsrate.
    let sta_rate = if linked { Some(link.si.ra_report_desc_rate) } else { None };
    let nss = if hal.rf_2t2r { 2 } else { 1 };
    let fw_adapt = fw_feature & FW_FEATURE_ADAPTIVITY != 0;

    // **Eine Ausleihe, zwei Rufe.** `si` und `rssi_si` sind in Linux
    // derselbe Iterator ueber dieselbe Station; hier muessen sie
    // nacheinander gehen, weil der Ausleiher nur eine mutable Referenz
    // zulaesst. Die Reihenfolge ist Linux': erst `statistics` (und darin
    // der RSSI), dann der Rest.
    phy::statistics(h, &mut d.dm, h2c, if linked { Some(&mut link.si) } else { None });
    phy::dig(h, &mut d.dm, hal.rf_path_num, linked);
    phy::cck_pd(h, &mut d.dm, band_2g, linked);
    phy::ra_track(h, &mut d.dm, h2c, d.stats.tx_throughput,
                  d.stats.rx_throughput, d.watch_dog_cnt,
                  if linked { Some((&mut link.si, caps, nss, band_2g)) } else { None },
                  sta_rate);
    phy::tx_path_diversity(h, &mut d.path_div, hal.antenna_tx, hal.antenna_rx,
                           linked);
    chip::cfo_track(h, &mut d.dm, hal.rf_path_num, e.crystal_cap, linked,
                    d.cx.bt_disabled);
    dpk::track(h, &mut d.dpk);
    chip::pwr_track(h, &mut d.dm, e.power_track_type, &e.thermal_meter,
                    hal.rf_path_num, link.channel);
    if fw_adapt {
        fw::adaptivity(h, h2c, &d.dm);
    } else {
        phy::adaptivity(h, &d.dm);
    }

    // main.c:196-207 `rtw_sw_beacon_loss_check`. Die Firmware mit
    // `FW_FEATURE_BCN_FILTER` macht es selbst.
    if fw_feature & FW_FEATURE_BCN_FILTER == 0 && beacon_int > 0 {
        // watchdog_delay = 2000000 / 1024 TU
        let watchdog_delay = 2_000_000u32 / 1024;
        let expected = watchdog_delay.div_ceil(beacon_int as u32);
        d.beacon_loss = (received_beacons as u32) < expected / 2;
    }

    d.watch_dog_cnt = d.watch_dog_cnt.wrapping_add(1);
}

/// Die Schleife des Treibers. `frist_us == 0` heisst: nicht mehr aufhoeren.
///
/// Sie bekommt Link UND Zaehler von aussen, damit Stufe 6b dort fortsetzt,
/// wo 6a aufgehoert hat.
#[allow(clippy::too_many_arguments)]
#[derive(PartialEq, Clone, Copy)]
enum PumpEnd {
    /// Die Frist von Stufe 6a ist abgelaufen — der Normalfall dort.
    Frist,
    /// Die Zelle hat uns verloren (Deauth/Disassoc) oder die Firmware
    /// hat sich fuer tot erklaert. Der Rufer verbindet neu.
    LinkLost,
}

fn link_pump(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
             link: &mut Link, ls: &mut LinkStats, mac: [u8; 6],
             frist_us: u64, d: &mut Dev, h2c: &mut fw::H2cState,
             e: &efuse::Efuse, caps: &sta::PeerCaps, fw_feature: u32)
    -> PumpEnd {
    chip::read_cck_gi_bnd(h, &mut d.dm);
    static mut RXBUF7: [u8; pci::RTK_PCI_RX_BUF_SIZE as usize] =
        [0; pci::RTK_PCI_RX_BUF_SIZE as usize];
    static mut ETHBUF: [u8; 2048] = [0; 2048];
    static mut CMDBUF: [u8; 2048] = [0; 2048];
    // SAFETY: einfaedig, je ein Rufer, keiner verlaesst diese Funktion.
    let (rxbuf, ethbuf, cmdbuf) = unsafe {
        (&mut *core::ptr::addr_of_mut!(RXBUF7),
         &mut *core::ptr::addr_of_mut!(ETHBUF),
         &mut *core::ptr::addr_of_mut!(CMDBUF))
    };


    // `frist_us == 0` heisst: nicht mehr aufhoeren. Stufe 6a gibt acht
    // Sekunden vor — der Handschlag braucht vier Rahmen und ist in
    // Millisekunden durch, wer laenger wartet, wartet auf einen Fehler.
    // Stufe 6b ruft dieselbe Schleife ohne Frist.

    // Der Rueckruf braucht die BSSID, um einen Deauth der EIGENEN Zelle
    // von dem des Nachbarn zu unterscheiden. Als Kopie, damit er `link`
    // nicht festhalten muss, waehrend die Schleife darauf schreibt.
    let bssid = link.bssid;

    let t0 = host::now_us();
    let mut report_ms = host::now_ms();
    let mut rx_silent_ms = host::now_ms();
    let mut watch_dog_ms = host::now_ms();
    // Ein Datenrahmen je Watchdog-Takt bekommt eine Quittung.
    let mut probe_due = true;
    let mut leer_in_folge = 0u32;
    if ls.pump_us0 == 0 {
        ls.pump_us0 = host::now_us();
    }
    // **Das Empfangsfenster der Aggregation, aus `sys/config/wifi`.**
    // `ampdu: off` schaltet sie ab, `ampdu: 16` gibt ein anderes
    // Fenster. Die Vorgabe ist klein und der Grund steht bei
    // `build_addba_resp`: es gibt keinen Umsortierpuffer.
    let ampdu_buf = read_ampdu_buf();
    // **`ampdu: off` ist der Notausgang und schaltet BEIDES ab** — die
    // Aggregation und den QoS-Rahmen darunter. Ein QoS-Datenrahmen ist
    // fuer sich richtig und braucht keinen Block; nach der Regression von
    // 0.36.0 ist mir aber ein Schalter lieber, der auf EINEN bekannten
    // Zustand zurueckfaellt, als drei halbe.
    if ampdu_buf == 0 {
        link.tx_qos = false;
    }
    // `bss_conf.beacon_int` — 100 TU ist der Wert, den praktisch jeder AP
    // ansagt; aus dem Beacon gelesen wird er noch nicht, und eine Null
    // waere hier schlimmer als der Normalfall (sie teilt).
    let beacon_int: u16 = 100;
    while frist_us == 0 || host::now_us() - t0 < frist_us {
        let now = host::now_ms();

        // ── Empfangen ────────────────────────────────────────────
        let mut acc = rx::WdAcc::new(link.si.avg_rssi);
        // **Die Zeit IM Ring, und warum sie hier gemessen wird.**
        // `rahmen/blick 1,2` kann zweierlei heissen: schnell genug, oder
        // exakt so langsam wie die Ankunft. Zwischen beidem entscheidet
        // nur, wieviel Wanduhrzeit im Empfangspfad steckt. Gemessen
        // wird NUR, wenn etwas kam (rund 1400 Mal je Sekunde, also
        // 2800 Wirtsaufrufe) — bei den 65 000 leeren Bliecken waere es
        // die Messung, die den Zustand erzeugt.
        let t_rx0 = host::now_us();
        let got = pci::rx_poll(h, trx, 64, rxbuf, &mut d.dm, &mut d.path_div,
                               hal.rf_path_num, 0, link.channel,
                               |st, pkt| {
            if st.crc_err {
                return;
            }
            let off = RX_PKT_DESC_SZ as usize + st.drv_info_sz as usize
                + st.shift as usize;
            if off >= pkt.len() {
                return;
            }
            // **C2H wurde bis 0.26.0 verworfen.** Der Ratenbericht der
            // Firmware ist die Eingabe von `config_swing_table` und
            // `rrsr_update`; ohne ihn rechnet der Watchdog auf der
            // Anfangsrate.
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
                        // **Beide Wege.** `C2H_CCX_TX_RPT` traegt die
                        // Quittung selbst (V0), `C2H_HALMAC` traegt sie
                        // als Unterkommando 0x0f (V1) — fw.c:93-113.
                        let v1 = c.id as u32 == C2H_HALMAC;
                        if let Some(r) = fw::tx_report_parse(c.payload, v1) {
                            if acc.n_tx_rpt < acc.tx_rpt.len() {
                                acc.tx_rpt[acc.n_tx_rpt] = r;
                                acc.n_tx_rpt += 1;
                            }
                        }
                    } else if acc.n_c2h_seen < acc.c2h_seen.len() {
                        // **Und was sonst hereinkommt, wird gezaehlt.**
                        // Dass die Quittung ausblieb, war in 0.26.0 eine
                        // Null ohne Hinweis; ein Zensus der Kennungen
                        // haette die Frage in EINEM Lauf beantwortet.
                        acc.c2h_seen[acc.n_c2h_seen] = c.id;
                        acc.n_c2h_seen += 1;
                    }
                }
                return;
            }
            let f = &pkt[off..];
            // rx.c:100-133 + phy.c:678-704 — was der Watchdog braucht.
            rx::watchdog_feed(&mut acc, st, f, &mac, &bssid,
                              hal.rf_path_num);
            // **Zuerst der Rauswurf.** Er ist ein Verwaltungsrahmen und
            // kaeme durch `rx_to_8023` nicht hindurch. Nur SEHEN hier —
            // gehandelt wird nach dem Ringleeren.
            // Der Zensus der Verwaltungsrahmen laeuft VOR dem Rauswurf
            // und schliesst ihn ein — ein Deauth ist auch einer.
            if let Some((sub, act)) = rx::mgmt_census(f, &bssid) {
                if acc.n_mgmt < acc.mgmt.len() {
                    acc.mgmt[acc.n_mgmt] = (sub, act.unwrap_or((0xff, 0xff)));
                    acc.n_mgmt += 1;
                }
                // **Der ADDBA Request wird nur GESEHEN**, beantwortet
                // wird er nach dem Ringleeren — ein Sendevorgang gehoert
                // nicht in einen Rueckruf, der `trx` nicht halten darf.
                if act == Some((DOT11_ACTION_CAT_BA, DOT11_ACTION_ADDBA_REQ))
                    && acc.addba.is_none()
                {
                    acc.addba = sta::parse_addba_req(f);
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
            let Some((n, is_eapol)) = rx_to_8023(f, ethbuf, &mut ls.llc_miss)
            else {
                return;
            };
            ls.data_rx += 1;
            // rx.c:14-32 `rtw_rx_stats` — nur UNICAST zaehlt, und
            // gezaehlt wird die Laenge des 802.11-Rahmens.
            if f[4] & 0x01 == 0 {
                acc.rx_unicast += f.len() as u64;
                acc.rx_cnt += 1;
            }
            if is_eapol {
                ls.eapol_rx += 1;
                if ls.authorized {
                    ls.rekey_rx += 1;
                }
                // `EV_EAPOL_RX` = [0x84][len u16 LE][Rahmen] — und der
                // Rahmen ist der EAPOL-RUMPF hinter dem Ethertyp.
                //
                // **Auf die ANGESAGTE Laenge kuerzen.** Der EAPOL-Kopf
                // traegt sie in den Bytes 2..4 (802.1X, gross-endig), und
                // der ganze Rahmen ist 4 + diese Zahl. Was die Hardware
                // dahinter anhaengt, gehoert nicht dazu: `WLAN_RCR_CFG`
                // hat APP_FCS, APP_MIC und APP_ICV gesetzt, also liefert
                // der Deskriptor mehr Bytes, als der Rahmen lang ist.
                //
                // **Das ist nicht kosmetisch.** `wifid` rechnet den MIC
                // ueber die GANZE Scheibe, die es bekommt
                // (`compute_mic`: `frame.len()`). Vier Bytes zu viel, und
                // msg3 schlaegt fehl — msg1 nicht, denn das traegt gar
                // keinen MIC. Genau dieses Muster stand im Geraetelauf.
                let raw = &ethbuf[14..n];
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
                host::netdev_submit_rx(&ethbuf[..n]);
            }
        });

        // **Die Form der Schleife, ohne einen einzigen zusaetzlichen
        // Wirtsaufruf gemessen.** Wieviele Rahmen ein Blick bringt sagt,
        // auf welcher Seite der Deckel liegt: knapp ueber eins heisst,
        // wir sehen schneller nach als etwas kommt (die Luft ist die
        // Grenze); volle Stapel heissen, wir kommen nicht nach.
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

        // Was der Ringdurchlauf dem Watchdog zugetragen hat, eintragen.
        acc.merge(&mut d.dm, &mut link.si);
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

        // ── Die Aggregation zulassen ─────────────────────────────
        // ── Der SENDE-Block: wir fragen, der AP antwortet ────────
        //
        // **Die Richtung, die bis 0.35.0 ganz fehlte.** In Linux stoesst
        // `rtw_txq_check_agg` (tx.c) `ba_work` an, und das ruft
        // `ieee80211_start_tx_ba_session` — genau diesen Rahmen. Ohne ihn
        // bleibt `ampdu_en` fuer immer falsch, und jeder Datenrahmen geht
        // einzeln mit eigener Quittung raus.
        //
        // Bedingungen: der Paarschluessel muss stehen (vorher sind wir
        // nicht autorisiert), der AP muss HT koennen, und das Fenster aus
        // der Konfiguration muss ueberhaupt Aggregation erlauben.
        if let Some(r) = acc.addba_resp.take() {
            if r.dialog_token == link.addba_token && r.tid == 0 {
                if r.status == 0 {
                    // **Das Fenster des AP, nicht unseres.** Er darf
                    // kleiner antworten, als wir gefragt haben, und dann
                    // gilt seins (802.11 §11.5.3).
                    let f = r.buf_size.clamp(1, 64);
                    link.tx_ampdu = Some(f);
                    ls.tx_ba_ok = true;
                    host::print("[rtl8822ce] Sende-Block steht: TID 0, Fenster ");
                    host::print_dec(f as u32);
                    host::print("\n");
                } else {
                    // Eine Absage ist eine ANTWORT. Nicht weiterfragen.
                    link.addba_tries = 255;
                    ls.tx_ba_status = r.status;
                    host::print("[rtl8822ce] Sende-Block abgelehnt, Status ");
                    host::print_dec(r.status as u32);
                    host::print("\n");
                }
            }
        }
        if link.tx_qos
            && link.tx_ampdu.is_none() && link.ptk_installed && ampdu_buf > 0
            && link.peer_ht && link.addba_tries < 4
            && now.wrapping_sub(link.addba_last_ms) >= 500
        {
            link.addba_tries += 1;
            link.addba_last_ms = now;
            link.addba_token = link.addba_token.wrapping_add(1);
            let mut req = [0u8; 256];
            // SSN: die naechste Folgenummer, die wir senden werden.
            let n = sta::build_addba_req(&mut req, &mac, &bssid, 0, 64,
                                         link.addba_token, link.seq, 0);
            let mut info = tx::pkt_info_update(&req[..n], 0, tx::RTW_BAND_2G);
            let q = tx::RTW_TX_QUEUE_MGMT;
            if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &req[..n]) {
                pci::tx_kick_off_queue(h, trx, q);
                ls.addba_req_sent += 1;
            }
        }

        // Der AP bittet mit einem ADDBA Request und wiederholt ihn,
        // solange keine Antwort kommt — im Geraetelauf 180 Mal, und
        // genau so lange konnte er nicht aggregieren.
        if let Some(req) = acc.addba.take() {
            if ampdu_buf > 0 {
                let mut resp = [0u8; 256];
                let n = sta::build_addba_resp(&mut resp, &mac, &bssid, &req,
                                              ampdu_buf);
                let mut info = tx::pkt_info_update(&resp[..n], 0,
                                                   tx::RTW_BAND_2G);
                let q = tx::RTW_TX_QUEUE_MGMT;
                if pci::tx_write(h, trx, mgmt_buf, q, &mut info, &resp[..n]) {
                    pci::tx_kick_off_queue(h, trx, q);
                    ls.addba_resp += 1;
                    ls.addba_win = ampdu_buf;
                    ls.addba_win_req = req.buf_size;
                    if ls.addba_resp <= 3 {
                        host::loud_begin();
                        host::print("[rtl8822ce] ADDBA angenommen: TID ");
                        host::print_dec(req.tid as u32);
                        host::print(", der AP wollte ");
                        host::print_dec(req.buf_size as u32);
                        host::print(" offene Rahmen, wir geben ");
                        host::print_dec(ampdu_buf as u32);
                        host::print("\n            (kein Umsortierpuffer, deshalb klein)\n");
                        host::loud_end();
                    }
                } else {
                    ls.addba_fail += 1;
                }
            }
        }
        if let Some((rate, mac_id)) = acc.ra_rpt {
            ls.ra_rpt_n += 1;
            // fw.c:308 — `dm_info->tx_rate` unabhaengig von der Station,
            // `si->ra_report.desc_rate` nur bei passender mac_id.
            d.dm.tx_rate = rate;
            if link.si.mac_id == mac_id {
                link.si.ra_report_desc_rate = rate;
            }
        }

        // ── Der Rauswurf, gehandelt ──────────────────────────────
        // Gesehen hat ihn der Rueckruf oben; hier ist der Ring leer und
        // `link` wieder frei. **Der Kernel erfaehrt es als erster** —
        // bis hierher glaubte er an `carrier UP` und schob Pakete in
        // eine tote Leitung.
        if let Some((deauth, reason)) = ls.gone.take() {
            ls.kicked += 1;
            ls.last_reason = reason;

            // **Gezaehlt wird jeder, gedruckt die ersten drei.** Ein AP
            // schickt seinen Rauswurf gern als Salve; die vierte Zeile
            // sagt nichts, was die erste nicht sagte, und der Zaehler im
            // Bericht bleibt vollstaendig.
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

            // **Und zurueck zum Rufer.** Stufe 6a laeuft weiter (dort
            // ist ein Rauswurf ein Befund fuer das Tor, keine Aufgabe);
            // in 6b baut der Rufer die Verbindung neu auf.
            if frist_us == 0 {
                return PumpEnd::LinkLost;
            }
        }

        // ── Kommandos von wifid ──────────────────────────────────
        loop {
            let clen = host::wifi_poll_cmd(cmdbuf);
            if clen <= 0 {
                break;
            }
            let cmd = &cmdbuf[..clen as usize];
            match cmd.first().copied() {
                // TX_EAPOL: [op][len u16 LE][Rahmen]
                Some(CMD_TX_EAPOL) if cmd.len() >= 3 => {
                    let len = ((cmd[2] as usize) << 8) | cmd[1] as usize;
                    if cmd.len() >= 3 + len {
                        let mut eth = [0u8; 600];
                        eth[0..6].copy_from_slice(&link.bssid);
                        eth[6..12].copy_from_slice(&mac);
                        eth[12..14]
                            .copy_from_slice(&ETHERTYPE_EAPOL.to_be_bytes());
                        eth[14..14 + len].copy_from_slice(&cmd[3..3 + len]);
                        // **Verschluesselt, sobald die PTK steht.** msg2 und
                        // msg4 gehen im Klartext hinaus, wie sie muessen —
                        // ein GRUPPEN-Neuschluessel kommt Minuten spaeter,
                        // mit der PTK laengst im Speicher, und der AP
                        // erwartet ihn geschuetzt.
                        let enc = link.ptk_installed;
                        // **EAPOL bekommt IMMER eine Quittung.** In Linux
                        // kommt das aus mac80211: Rahmen des Steuerports
                        // tragen `IEEE80211_TX_CTL_REQ_TX_STATUS`. Es
                        // sind die Rahmen, deren Verlust die Verbindung
                        // kostet — und beim letzten Fehler genau die, von
                        // denen der AP keinen einzigen hoerte.
                        let sn = ls.arm_probe(now);
                        if tx_8023(h, trx, mgmt_buf, link,
                                   &eth[..14 + len], enc, sn) {
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
                        // Paarschluessel auf Platz 0, Gruppenschluessel auf
                        // seinen Index — so haelt es auch mac80211.
                        let slot = if group { key_idx.min(3) } else { 0 };
                        let addr = if group { [0xffu8; 6] } else { link.bssid };
                        sec::write_cam(h, &mut link.cam[slot as usize], slot,
                                       RTW_CAM_AES as u8, key_idx, group,
                                       &addr, key);
                        ls.keys_set += 1;
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
                // AUTHORIZED: der Handschlag ist durch.
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

        // ── Senden, was der IP-Stapel loswerden will ─────────────
        if ls.authorized {
            loop {
                let n = host::netdev_poll_tx(ethbuf);
                if n <= 0 {
                    break;
                }
                let enc = link.ptk_installed;
                // **Ein Datenrahmen je Watchdog-Takt wird quittiert.**
                // Das ist eine benannte Abweichung: Linux erfaehrt den
                // Sendeerfolg ueber mac80211 und fragt deshalb nur fuer
                // Steuerrahmen nach. Uns fehlt dieser Weg ganz, und eine
                // Leitung, auf der NICHTS quittiert wird, war zweimal der
                // Fehler. Einer je zwei Sekunden kostet nichts und
                // beantwortet „hoert der AP mich ueberhaupt".
                let sn = if probe_due { probe_due = false; ls.arm_probe(now) }
                         else { None };
                if tx_8023(h, trx, mgmt_buf, link,
                           &ethbuf[..n as usize], enc, sn) {
                    ls.data_tx += 1;
                    // tx.c `rtw_tx` — dieselbe Buchfuehrung wie beim
                    // Empfang, damit `tx_throughput` eine Zahl hat.
                    if ethbuf[0] & 0x01 == 0 {
                        d.stats.tx_unicast += n as u64;
                        d.stats.tx_cnt += 1;
                    }
                }
            }
        }

        // `rtw_pci_tx_isr` — den Lesezeiger nachziehen.
        pci::tx_isr(h, trx, pci::Q_BE);
        pci::tx_isr(h, trx, tx::RTW_TX_QUEUE_MGMT);

        // ── Alle zwei Sekunden: `rtw_watch_dog_work` ─────────────
        // **Der Takt ist Linux'**: `RTW_WATCH_DOG_DELAY_TIME` = HZ * 2.
        // Die Nachfuehrungen darin rechnen auf dem, was seit dem letzten
        // Takt hereinkam — ein anderer Takt waere ein anderer Regler.
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
            // `rtw_phy_stat_rate_cnt` hat das Fenster gerade nach
            // `last_pkt_count` geschoben — jetzt und nur jetzt steht es
            // vollstaendig da.
            for i in 0..DESC_RATE_MAX {
                ls.rate_hist[i] = ls.rate_hist[i]
                    .saturating_add(d.dm.last_pkt_count.num_qry_pkt[i] as u32);
            }
            // Dieselbe Stelle, derselbe Grund: `false_alarm_statistics`
            // hat gerade gelesen UND zurueckgesetzt.
            ls.ht_ok += d.dm.ht_ok_cnt as u64;
            ls.ht_err += d.dm.ht_err_cnt as u64;
            ls.ofdm_ok += d.dm.ofdm_ok_cnt as u64;
            ls.ofdm_err += d.dm.ofdm_err_cnt as u64;
        }

        // ── Einmal je Sekunde: der Bericht fuer `wlan` ───────────
        // Die Luft ist fuer den Kernel unsichtbar. Rate, Zaehler und
        // Schluesselzustand stehen nirgends sonst — ohne sie ist eine
        // Leitung, die wegen einer Legacy-Rate langsam ist, nicht von
        // einer zu unterscheiden, die wegen voller Schlangen langsam ist.
        if now.wrapping_sub(report_ms) >= 1000 {
            report_ms = now;
            publish_report(link, ls, d, e);
        }

        // ── RX-Stille als Wachhund ───────────────────────────────
        // Auf einer lebenden Zelle kommt IMMER etwas: Beacons allein sind
        // zehn je Sekunde. Voelliges Schweigen heisst, dass der Ring
        // steht, nicht dass die Luft leer ist.
        if got > 0 {
            rx_silent_ms = now;
        } else if now.wrapping_sub(rx_silent_ms) > 5_000 {
            rx_silent_ms = now;
            ls.rx_wd += 1;
            if ls.rx_wd <= 4 {
                // Ein stehender Ring ist kein Stufenbefund, sondern ein
                // Fehler: laut, auch ohne `debug: 1`.
                host::loud_begin();
                host::print("[rtl8822ce] RX still seit 5 s — Ringzeiger rp=");
                host::print_dec(trx.rx.rp);
                host::print(", rx_tag ");
                host::print_dec(trx.rx_tag as u32);
                host::print("\n");
                host::loud_end();
            }
        }

        // ── Wann wir die Hand vom Ring nehmen ────────────────────
        //
        // **Hier stand `if got == 0 { sleep_ms(1) }`, und das war der
        // Durchsatzdeckel.** Zwischen zwei Buendeln ist der Ring einen
        // Moment leer — beim ersten leeren Blick eine ganze Millisekunde
        // zu schlafen heisst, hoechstens tausend Mal je Sekunde
        // nachzusehen. Gemessen: 1375 Rahmen/s bei 1,37 Rahmen je Blick,
        // also genau `1000 x Buendelgroesse`. Die Aggregation aus 0.29.0
        // machte die Buendel groesser und brachte deshalb nur +26 %
        // statt eines Vielfachen.
        //
        // Jetzt die Form, die Linux NAPI nennt: **ein Budget leerer
        // Blicke, dann erst schlafen** — und liegend bleiben, bis wieder
        // etwas kommt. Unter Last faellt der Zaehler bei jedem Buendel
        // auf null und wir schlafen nie; im Leerlauf ist das Budget nach
        // einem knappen halben Millisekunde aufgebraucht und wir
        // schlafen wie vorher.
        if got == 0 {
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


/// Wieviele leere Blicke auf den Ring, bevor wir uns schlafen legen.
///
/// Eine Schleifenrunde kostet ein halbes Dutzend Wirtsaufrufe, also
/// grob fuenf bis zehn Mikrosekunden. 64 leere Runden sind damit knapp
/// eine halbe Millisekunde Wachbleiben — unter Last kommt der naechste
/// Rahmen lange vorher, im Leerlauf ist es ein einmaliger Preis je
/// Beacon.
const RX_SPIN_BUDGET: u32 = 64;

/// Wie lange gewartet wird, wenn ein Anlauf scheitert. Ein AP, der
/// gerade neu startet, braucht Sekunden; oefter zu fragen hilft nicht
/// und fuellt nur den Log.
const RECONNECT_BACKOFF_MS: u32 = 3000;

/// **Der Weg zurueck in eine stehende Verbindung.**
///
/// Er ist derselbe wie der Weg hin — Stufe 5e (Auth + Assoc) und 5f
/// (Ratenanpassung) —, mit vier Unterschieden, und jeder hat einen
/// Grund:
///
/// * **Kein `netdev_register`.** Der Kernel kennt die Schnittstelle
///   schon; ein zweites Anmelden gaebe eine zweite.
/// * **Die Schluessel raus, BEVOR neu verhandelt wird.** Ein alter
///   Paarschluessel im CAM entschluesselt die ersten Rahmen der neuen
///   Verbindung falsch, und das sieht aus wie ein kaputter Handschlag.
/// * **`rtw_mac_flush_queues`** — was noch in den Sendeschlangen liegt,
///   gehoert zur alten Verbindung und wuerde mit dem alten Schluessel
///   hinausgehen.
/// * **Die Paketnummer faengt wieder bei eins an** (802.11 §12.5.3.2:
///   sie gehoert zum SCHLUESSEL, und der ist gleich ein neuer).
///
/// Und ein neues `EV_READY`: `wifid` braucht einen frischen Supplicant
/// mit neuem SNonce. **Das ist der feine Unterschied zu 0.23.0** — dort
/// kam das zweite `EV_READY` ohne neue Verbindung, hier gehoert es dazu.
#[allow(clippy::too_many_arguments)]
fn reconnect(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
             h2c: &mut fw::H2cState, e: &efuse::Efuse,
             t: &txpower::TxPower, bss: &Bss, link: &mut Link,
             ls: &mut LinkStats, d: &mut Dev,
             linked: &mut Option<vif::Vif>) -> bool {
    ls.reconnects += 1;
    host::loud_begin();
    host::print("[rtl8822ce] Verbindung weg — Anlauf ");
    host::print_dec(ls.reconnects);
    host::print(" auf \"");
    print_ssid(&bss.ssid[..bss.ssid_len as usize]);
    host::print("\", Kanal ");
    host::print_dec(bss.channel as u32);
    host::print("\n");
    host::loud_end();

    // Der Kernel soll nichts mehr in die tote Leitung schieben.
    host::netdev_set_link(false);

    // Alte Schluessel aus dem CAM. `write_cam` hat sie hineingelegt,
    // `clear_cam` nimmt sie heraus — Platz fuer Platz, wie sie belegt
    // wurden.
    for slot in 0..link.cam.len() {
        sec::clear_cam(h, &mut link.cam[slot], slot as u8);
    }
    link.ptk_installed = false;
    link.tx_pn = 1;
    link.seq = 0;

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

    // Stufe 5e: Auth und Assoc, auf demselben Kanal.
    if !stage5e_connect(h, hal, trx, mgmt_buf, h2c, e, t, e.addr, bss,
                        linked, d) {
        return false;
    }
    let Some(v) = linked.as_ref() else { return false };

    // Stufe 5f: die Firmware waehlt wieder die Rate.
    let mut rates: Option<(sta::PeerCaps, sta::StaInfo)> = None;
    if !stage5f_rates(h, trx, h2c, hal, e, v, bss, &mut rates, d) {
        return false;
    }
    if let Some((caps, si)) = rates {
        link.si = si;
        link.highest_rate = if caps.ht_supported {
            tx::highest_ht_tx_rate(&caps.ht_mcs, hal.rf_2t2r)
        } else {
            DESC_RATE54M as u8
        };
    }

    // Und `wifid` bekommt einen frischen Supplicant.
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

/// Stufe 6a — Aufbau, Handschlag und die ersten acht Sekunden.
#[allow(clippy::too_many_arguments)]
fn stage6a_link(h: i32, hal: &Hal, trx: &mut pci::Trx, mgmt_buf: i32,
                bss: &Bss, caps: &sta::PeerCaps, si: sta::StaInfo,
                mac: [u8; 6], link: &mut Option<Link>,
                ls: &mut LinkStats, d: &mut Dev, h2c: &mut fw::H2cState,
                e: &efuse::Efuse, fw_feature: u32) -> bool {
    host::print("[rtl8822ce] Stufe 6a: der Steuerkanal und der Datenweg\n");

    let mut l = link_setup(hal, bss, caps, si, mac);
    // Acht Sekunden: der Handschlag braucht vier Rahmen und ist in
    // Millisekunden durch; wer laenger wartet, wartet auf einen Fehler.
    let _ = link_pump(h, hal, trx, mgmt_buf, &mut l, ls, mac, 8_000_000, d,
                      h2c, e, caps, fw_feature);

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

/// Ein Tor. **Ein gefallenes geht immer hinaus** — sonst sagt ein
/// stiller Lauf nicht, wo er stehengeblieben ist, und das waere genau
/// der Zustand, aus dem die sechs Stufen herausfuehren sollten.
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

/// Die belegten Plaetze des C2H-Zensus.
fn d_c2h(ls: &LinkStats) -> impl Iterator<Item = (u8, u32)> + '_ {
    ls.c2h_ids.iter().filter(|e| e.1 > 0).map(|e| (e.0, e.1))
}

/// **Die eine Zeile, die auch ein stiller Lauf druckt.**
///
/// Sie steht nicht im `driver_report` — der landet in `wlan` und ist
/// Zustand, der sich je Sekunde erneuert. Hier steht das EREIGNIS: die
/// Verbindung ist zustande gekommen, mit wem, wie schnell und wie breit.
///
///     [rtl8822ce] verbunden: "IvyPie_New" K7 -49 dBm · HT MCS8-15
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

/// Eine Zeile der Schlusszusammenfassung. Gruen ist Stufenausgabe, rot
/// geht immer hinaus.
fn stage_line(ok: bool, green: &str, red: &str) {
    if ok {
        host::print(green);
    } else {
        host::say(red);
    }
}

/// `debug:` aus `sys/config/wifi`. Fehlt die Datei oder die Zeile, ist
/// die Antwort NEIN: ein Treiber im Autostart schweigt, bis jemand
/// danach fragt.
///
/// **Gibt den Rueckgabewert des Lesezugriffs mit zurueck**, und das ist
/// kein Beiwerk: „kein `debug:` in der Datei" und „die Datei war nicht
/// da" fuehren zum selben Schweigen und haben verschiedene Heilungen.
/// Ein NEIN aus einem gescheiterten Lesezugriff ist keine Antwort auf
/// die gestellte Frage.
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

/// `ampdu:` aus `sys/config/wifi` — das Empfangsfenster der
/// Aggregation.
///
/// **Drei Faelle, und alle drei absichtlich:** die Zeile fehlt → die
/// Vorgabe (8, siehe `build_addba_resp`); `off`/`0` → gar keine
/// Aggregation, der Zustand bis 0.28.0; eine ZAHL → genau dieses
/// Fenster. So kann der naechste Lauf 8 gegen 32 messen, ohne dass
/// jemand neu uebersetzt — und eine Messung schlaegt eine Vermutung
/// darueber, wieviel Umsortierung TCP hier vertraegt.
/// `aspm:` aus `sys/config/wifi` — `an` · `aus` · `wie-gefunden`.
///
/// **Vorgabe ist AUS, und das ist eine Entscheidung mit zwei Seiten.** Der
/// Treiber schlaeft nie (kein LPS, §6 des Plans), also hat das Stromsparen
/// des Links bei uns keinen Gegenpart, der es wieder aufweckt — und die
/// Karte sagt selbst, dass sie 64 us braucht, um aus L1 herauszukommen.
/// Dafuer kostet es Leerlaufstrom, und genau daran haengt ein anderer
/// offener Posten (`project_idle_power_21w`). Deshalb ein Schalter und
/// kein stilles Verhalten: `aspm: an` faehrt die Gegenprobe.
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

/// Der reine Teil von `read_aspm_pref` — getrennt, damit `framecheck.py`
/// ihn ohne Geraet und ohne Dateisystem fahren kann.
///
/// `Some(true)` anschalten · `Some(false)` ausschalten · `None` nicht
/// anfassen. **Ein unverstandener Wert heisst AUS, nicht „nicht
/// anfassen"** — wer etwas hinschreibt, will etwas aendern, und die
/// sichere Auslegung eines Tippfehlers ist die Vorgabe, nicht das
/// Gegenteil davon.
fn aspm_pref_from(v: &[u8]) -> Option<bool> {
    if v.starts_with(b"an") || v.starts_with(b"on") || v == b"1" {
        Some(true)
    } else if v.starts_with(b"wie") || v.starts_with(b"keep") {
        None
    } else {
        Some(false)
    }
}

fn read_ampdu_buf() -> u16 {
    const VORGABE: u16 = 8;
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
    // Das Feld ist zehn Bit breit (`ADDBA_PARAM_BUF_SIZE_MASK`), und
    // mehr als 64 kann HT ohnehin nicht.
    if any { num.clamp(1, 64) } else { VORGABE }
}

/// `on` oder `1` — dieselbe Regel, die `wifi_ax200` fuer `ampdu:` und
/// `ps:` fuehrt. Ein unbekanntes Wort ist ein NEIN und keine Vermutung.
fn cfg_on(v: &[u8]) -> bool {
    v.starts_with(b"on") || v.starts_with(b"1")
}

/// `cfg_get` aus `wifid`/`wifi_ax200` — eine Zeile `key: value`, `#` ist
/// ein Kommentar. Gibt die GRENZEN des Wertes zurueck, nicht eine
/// Scheibe: der Puffer wird daneben weiterbenutzt.
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

/// Leerzeichen und Wagenruecklauf an beiden Enden weg.
fn trim(t: &[u8], mut a: usize, mut b: usize) -> (usize, usize) {
    while a < b && (t[a] == b' ' || t[a] == b'\t') {
        a += 1;
    }
    while b > a && (t[b - 1] == b' ' || t[b - 1] == b'\t' || t[b - 1] == b'\r') {
        b -= 1;
    }
    (a, b)
}

/// docs/spec/WIFI_CLASS_ABI.md §3 — `npk_driver_report`.
///
/// Ein Klartextblock, den das Intent `wlan` neben die Kernelsicht druckt.
/// Der Kernel parst nichts; was berichtenswert ist, ist Geraetewissen.
fn publish_report(link: &Link, ls: &LinkStats, d: &Dev,
                  e: &efuse::Efuse) {
    let mut b = [0u8; 896];
    let mut n = 0usize;
    let put = |s: &str, b: &mut [u8; 896], n: &mut usize| {
        let k = s.len().min(b.len() - *n);
        b[*n..*n + k].copy_from_slice(&s.as_bytes()[..k]);
        *n += k;
    };
    let num = |v: u32, b: &mut [u8; 896], n: &mut usize| {
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
    // **Drei Raten, und nur eine davon war bisher zu sehen.**
    //
    // `angeboten` ist `link.highest_rate` — einmal bei der Anmeldung aus
    // den Faehigkeiten des AP gerechnet. Das ist eine BEHAUPTUNG ueber
    // das Moegliche, und sie stand hier bis 0.31.0 allein als „rate".
    //
    // `tx` ist, was die FIRMWARE gewaehlt hat (C2H `RA_RPT`), `rx` was
    // im Empfangsdeskriptor JEDES Rahmens steht. Das sind die Messungen.
    let hex = b"0123456789abcdef";
    let rate_hex = |v: u8, b: &mut [u8; 896], n: &mut usize| {
        if *n + 2 <= b.len() {
            b[*n] = hex[(v >> 4) as usize];
            b[*n + 1] = hex[(v & 0xf) as usize];
            *n += 2;
        }
    };
    // **Die haeufigste Empfangsrate der ganzen Verbindung**, nicht die
    // des letzten Rahmens. Bei einem Download sind 65 000 Datenrahmen
    // gegen 900 Beacons kein Zweifelsfall.
    let (top_rate, top_cnt) = ls.rate_hist.iter().enumerate()
        .fold((0usize, 0u32), |acc, (i, &c)| if c > acc.1 { (i, c) } else { acc });
    put("  rx ", &mut b, &mut n);
    put(rate_name(top_rate as u8), &mut b, &mut n);
    put(" (0x", &mut b, &mut n);
    rate_hex(top_rate as u8, &mut b, &mut n);
    put(" in ", &mut b, &mut n);
    num(top_cnt, &mut b, &mut n);
    put(" von ", &mut b, &mut n);
    let ges: u32 = ls.rate_hist.iter().sum();
    num(ges, &mut b, &mut n);
    put(", zuletzt ", &mut b, &mut n);
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
    // Die PCIe-Strecke. Steht hier und nicht einmalig beim Start, weil
    // ASPM ein Verdaechtiger fuer den Durchsatz ist und ein Verdaechtiger
    // in DEN Bericht gehoert, den Florian einschickt.
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
    // **Die Stroemezahl, und zwar BEIDE.** `rx HT MCS7` ist die Spitze des
    // Ein-Strom-Bereichs — dort festzusitzen heisst entweder, dass der AP
    // so waehlt, oder dass wir nur einen Strom ANGEBOTEN haben. Der
    // Unterschied steht in der efuse, und ohne ihn im Bericht raet man.
    put("  nss ", &mut b, &mut n);
    num(e.hw_cap_nss as u32, &mut b, &mut n);
    put(" angeboten (antennen ", &mut b, &mut n);
    num(e.hw_cap_ant_num as u32, &mut b, &mut n);
    put(", efuse-bw 0x", &mut b, &mut n);
    rate_hex(e.hw_cap_bw, &mut b, &mut n);
    put(")", &mut b, &mut n);
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
    put("  eapol ", &mut b, &mut n);
    num(ls.eapol_rx, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(ls.eapol_tx, &mut b, &mut n);
    put("  schluessel ", &mut b, &mut n);
    num(ls.keys_set, &mut b, &mut n);
    put("  rx-wachhund ", &mut b, &mut n);
    num(ls.rx_wd, &mut b, &mut n);
    put("\nrx-schleife ", &mut b, &mut n);
    num(ls.rx_polls, &mut b, &mut n);
    put(" blicke mit beute, ", &mut b, &mut n);
    num(ls.rx_empty, &mut b, &mut n);
    put(" leer, ", &mut b, &mut n);
    // Rahmen je Blick mit einer Nachkommastelle, in ganzen Zahlen.
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
    // Auslastung in Prozent: Zeit im Ring gegen Zeit der Schleife.
    let lauf = host::now_us().wrapping_sub(ls.pump_us0).max(1);
    num(((ls.rx_us.saturating_mul(100)) / lauf) as u32, &mut b, &mut n);
    put(" % der zeit im empfangspfad (", &mut b, &mut n);
    let je = if ls.rx_frames > 0 { ls.rx_us / ls.rx_frames as u64 } else { 0 };
    num(je as u32, &mut b, &mut n);
    put(" us je rahmen)", &mut b, &mut n);
    // **Die Zeile, die sagt, ob der AP uns HOERT.** Bis 0.26.0 stand
    // hier nichts dergleichen: „raus 360" hiess nur, dass wir 360 Rahmen
    // in einen Ring gelegt haben.
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
    // **Die Zeile, die diese Runde beantwortet.** Ein Neuschluessel
    // laeuft Minuten nach dem Handschlag und hinterlaesst sonst keine
    // Spur; ein Rauswurf war bis hierher gar nicht sichtbar.
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
    // **Die Frage dieser Runde, in einer Zahl.** Versucht der AP
    // ueberhaupt, eine Aggregation aufzubauen? Er tut das mit einem
    // ADDBA Request, und wir verwerfen bis heute jeden
    // Verwaltungsrahmen ausser Deauth und Disassoc.
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
    // **Die GEGENrichtung, und sie hat drei Zustaende, nicht zwei.**
    // „nie gefragt" · „gefragt, keine Antwort" · „abgelehnt mit Status N"
    // sehen ohne diese Zeile alle gleich aus, naemlich nach „keine
    // Aggregation".
    put("  senden: ", &mut b, &mut n);
    if ls.tx_ba_ok {
        put("Block steht (fenster ", &mut b, &mut n);
        num(link.tx_ampdu.unwrap_or(0) as u32, &mut b, &mut n);
        put(", faktor ", &mut b, &mut n);
        num(sta::tx_ampdu_factor(link.peer_ampdu_param) as u32, &mut b, &mut n);
        put(", dichte ", &mut b, &mut n);
        num(sta::tx_ampdu_density(link.peer_ampdu_param) as u32, &mut b, &mut n);
        put(")", &mut b, &mut n);
    } else if ls.tx_ba_status != 0 {
        put("ABGELEHNT, status ", &mut b, &mut n);
        num(ls.tx_ba_status as u32, &mut b, &mut n);
    } else if ls.addba_req_sent > 0 {
        num(ls.addba_req_sent, &mut b, &mut n);
        put("x gefragt, KEINE Antwort", &mut b, &mut n);
    } else {
        put("nicht gefragt", &mut b, &mut n);
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
    // **Die Zeile, die beweist, dass der Watchdog laeuft.** Vier Zahlen,
    // die sich bewegen muessen: der Takt, die Verstaerkungsregelung, der
    // Quarz (gegen den Wert der efuse) und die Temperatur. Steht der
    // Quarz auf dem efuse-Wert und `bt` auf „an", ist die Nachfuehrung
    // durch den Koexistenz-Riegel abgestellt — das ist kein Fehler,
    // sondern Linux' eigene Regel, und man sieht es hier.
    put("\nwatchdog ", &mut b, &mut n);
    num(d.watch_dog_cnt, &mut b, &mut n);
    put("  igi 0x", &mut b, &mut n);
    if n + 2 <= b.len() {
        b[n] = hex[(d.dm.igi_history[0] >> 4) as usize];
        b[n + 1] = hex[(d.dm.igi_history[0] & 0xf) as usize];
        n += 2;
    }
    put("  fehlalarm ", &mut b, &mut n);
    num(d.dm.total_fa_cnt, &mut b, &mut n);
    // **Der Zustand der Luft in einer Zahl.** Ein hoher Anteil heisst:
    // der AP muss staendig wiederholen, und dann ist der Deckel die
    // Strecke und nicht der Treiber.
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
    // **Warum die Gegenseite waehlt, was sie waehlt.** Der
    // Stoerabstand je Pfad steht in jedem Empfangsdeskriptor
    // (`query_phy_status_page1`) und sagt, ob eine niedrige Rate
    // berechtigt ist oder ob jemand unter Wert faehrt.
    put("  snr ", &mut b, &mut n);
    num(d.dm.rx_snr[0].max(0) as u32, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.dm.rx_snr[1].max(0) as u32, &mut b, &mut n);
    // **Zwei Zahlen, die sich gegenseitig aufloesen.** Allein sagt der
    // Quarzwert nichts: erst der Abstand zur efuse sagt, ob die
    // Nachfuehrung ueberhaupt etwas tut. Steht er auf dem efuse-Wert und
    // `bt` auf „aus", dann hat sie gelaufen und nichts zu korrigieren
    // gefunden — steht er darauf und `bt` auf „AN", ist sie abgestellt.
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
    // **Der Spitzenwert daneben.** Der geglaettete Wert faellt nach dem
    // Ende einer Uebertragung binnen Sekunden auf null — wer danach
    // `wlan` tippt, sieht `0/0` und kann ihn mit nichts vergleichen. Der
    // Hoechststand bleibt stehen und ist die Zahl, die neben der von
    // `netbench` steht.
    put("  tp ", &mut b, &mut n);
    num(d.stats.tx_throughput, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.stats.rx_throughput, &mut b, &mut n);
    put(" Mbit (spitze ", &mut b, &mut n);
    num(d.stats.tx_peak, &mut b, &mut n);
    put("/", &mut b, &mut n);
    num(d.stats.rx_peak, &mut b, &mut n);
    put(")\n", &mut b, &mut n);
    host::driver_report(&b[..n]);
}
