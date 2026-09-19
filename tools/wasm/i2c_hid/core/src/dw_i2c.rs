//! Synopsys-Designware-I2C — der Bus, an dem das Touchpad haengt.
//!
//! Portiert aus Linux 6.18.26, `drivers/i2c/busses/`:
//! `i2c-designware-core.h` (Register), `-common.c` (Takt, SCL-Zaehler,
//! Abschalten, FIFO-Tiefe) und `-master.c` (Initialisierung und
//! Uebertragung). Die Funktionsnamen sind uebernommen, damit sich beides
//! nebeneinanderlegen laesst.
//!
//! **Eine bewusste Abweichung, und ihr Grund:** Linux fuellt die FIFOs aus
//! `i2c_dw_isr`. Fuer dieses Geraet gibt es bei uns keinen Interrupt —
//! `irq.rs` sagt „no IOAPIC, PIC fully masked", und der FCH-I2C hat kein
//! MSI-X. Also laeuft dieselbe Zustandsmaschine aus einer Warteschleife:
//! `read_clear_intrbits` → `process_transfer` → `xfer_msg`/`read`. Die
//! Funktionen und ihre Reihenfolge bleiben, nur der Ausloeser ist ein
//! anderer. Linux selbst faehrt in `amd_i2c_dw_xfer_quirk` einen Pollpfad,
//! es ist also keine erfundene Bauweise — und die Maskenlogik dafuer
//! (`ACCESS_POLLING`, `sw_mask`) steht im Original.

use alloc::{format, string::String};

// ── Register (i2c-designware-core.h) ─────────────────────────────────
pub const DW_IC_CON: u32 = 0x00;
pub const DW_IC_TAR: u32 = 0x04;
pub const DW_IC_DATA_CMD: u32 = 0x10;
pub const DW_IC_SS_SCL_HCNT: u32 = 0x14;
pub const DW_IC_SS_SCL_LCNT: u32 = 0x18;
pub const DW_IC_FS_SCL_HCNT: u32 = 0x1c;
pub const DW_IC_FS_SCL_LCNT: u32 = 0x20;
pub const DW_IC_INTR_MASK: u32 = 0x30;
pub const DW_IC_RAW_INTR_STAT: u32 = 0x34;
pub const DW_IC_RX_TL: u32 = 0x38;
pub const DW_IC_TX_TL: u32 = 0x3c;
pub const DW_IC_CLR_INTR: u32 = 0x40;
pub const DW_IC_CLR_RX_UNDER: u32 = 0x44;
pub const DW_IC_CLR_RX_OVER: u32 = 0x48;
pub const DW_IC_CLR_TX_OVER: u32 = 0x4c;
pub const DW_IC_CLR_RD_REQ: u32 = 0x50;
pub const DW_IC_CLR_TX_ABRT: u32 = 0x54;
pub const DW_IC_CLR_RX_DONE: u32 = 0x58;
pub const DW_IC_CLR_ACTIVITY: u32 = 0x5c;
pub const DW_IC_CLR_STOP_DET: u32 = 0x60;
pub const DW_IC_CLR_START_DET: u32 = 0x64;
pub const DW_IC_CLR_GEN_CALL: u32 = 0x68;
pub const DW_IC_ENABLE: u32 = 0x6c;
pub const DW_IC_STATUS: u32 = 0x70;
pub const DW_IC_TXFLR: u32 = 0x74;
pub const DW_IC_RXFLR: u32 = 0x78;
pub const DW_IC_SDA_HOLD: u32 = 0x7c;
pub const DW_IC_TX_ABRT_SOURCE: u32 = 0x80;
pub const DW_IC_ENABLE_STATUS: u32 = 0x9c;
pub const DW_IC_SMBUS_INTR_MASK: u32 = 0xcc;
pub const DW_IC_COMP_PARAM_1: u32 = 0xf4;
pub const DW_IC_COMP_VERSION: u32 = 0xf8;
pub const DW_IC_SDA_HOLD_MIN_VERS: u32 = 0x3131312A; // "111*"
pub const DW_IC_COMP_TYPE: u32 = 0xfc;
pub const DW_IC_COMP_TYPE_VALUE: u32 = 0x4457_0140; // "DW" + 0x0140

pub const DW_IC_CON_MASTER: u32 = 1 << 0;
pub const DW_IC_CON_SPEED_STD: u32 = 1 << 1;
pub const DW_IC_CON_SPEED_FAST: u32 = 2 << 1;
pub const DW_IC_CON_RESTART_EN: u32 = 1 << 5;
pub const DW_IC_CON_SLAVE_DISABLE: u32 = 1 << 6;

pub const DW_IC_INTR_RX_UNDER: u32 = 1 << 0;
pub const DW_IC_INTR_RX_OVER: u32 = 1 << 1;
pub const DW_IC_INTR_RX_FULL: u32 = 1 << 2;
pub const DW_IC_INTR_TX_OVER: u32 = 1 << 3;
pub const DW_IC_INTR_TX_EMPTY: u32 = 1 << 4;
pub const DW_IC_INTR_RD_REQ: u32 = 1 << 5;
pub const DW_IC_INTR_TX_ABRT: u32 = 1 << 6;
pub const DW_IC_INTR_RX_DONE: u32 = 1 << 7;
pub const DW_IC_INTR_ACTIVITY: u32 = 1 << 8;
pub const DW_IC_INTR_STOP_DET: u32 = 1 << 9;
pub const DW_IC_INTR_START_DET: u32 = 1 << 10;
pub const DW_IC_INTR_GEN_CALL: u32 = 1 << 11;
pub const DW_IC_INTR_MST_ON_HOLD: u32 = 1 << 13;

pub const DW_IC_INTR_DEFAULT_MASK: u32 =
    DW_IC_INTR_RX_FULL | DW_IC_INTR_TX_ABRT | DW_IC_INTR_STOP_DET;
pub const DW_IC_INTR_MASTER_MASK: u32 = DW_IC_INTR_DEFAULT_MASK | DW_IC_INTR_TX_EMPTY;

pub const DW_IC_ENABLE_ENABLE: u32 = 1 << 0;
pub const DW_IC_ENABLE_ABORT: u32 = 1 << 1;

pub const DW_IC_STATUS_ACTIVITY: u32 = 1 << 0;
pub const DW_IC_STATUS_MASTER_HOLD_TX_FIFO_EMPTY: u32 = 1 << 7;

pub const DW_IC_SDA_HOLD_RX_SHIFT: u32 = 16;

const STATUS_WRITE_IN_PROGRESS: u32 = 1 << 1;
const STATUS_READ_IN_PROGRESS: u32 = 1 << 2;
const STATUS_MASK: u32 = 0x7;

/// Abbruchgruende, die einen Namen verdienen (DW_IC_TX_ABRT_SOURCE).
const ABRT_7B_ADDR_NOACK: u32 = 1 << 0;
const ABRT_TXDATA_NOACK: u32 = 1 << 3;
const ARB_LOST: u32 = 1 << 12;

pub const I2C_MAX_STANDARD_MODE_FREQ: u32 = 100_000;
pub const I2C_MAX_FAST_MODE_FREQ: u32 = 400_000;

/// Der Zugang zur Hardware. Der Treiber rechnet, der Rufer greift zu —
/// damit laeuft derselbe Code im Modul und im Pruefstand.
pub trait Bus {
    fn read32(&mut self, off: u32) -> u32;
    fn write32(&mut self, off: u32, val: u32);
    /// Mindestens `us` Mikrosekunden warten.
    fn udelay(&mut self, us: u32);
    /// Monotone Zeit in Mikrosekunden, fuer Zeitablaeufe.
    fn now_us(&mut self) -> u64;
    fn note(&mut self, _s: &str) {}
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Kein Designware-Block an dieser Adresse.
    NotDesignware(u32),
    /// Der Bus wurde nicht frei.
    BusBusy,
    /// Das Geraet hat die Adresse nicht bestaetigt — meist: da ist nichts.
    AddrNack,
    /// Das Geraet hat mitten im Schreiben abgebrochen.
    DataNack,
    /// Arbitrierung verloren.
    ArbLost,
    /// Anderer Abbruch, mit dem rohen Quellregister.
    Abort(u32),
    /// Zeitablauf.
    Timeout,
}

/// Auf ganze Zahlen gerundete Division — Linux `DIV_ROUND_CLOSEST_ULL`.
fn div_round_closest(n: u64, d: u64) -> u64 {
    (n + d / 2) / d
}

/// `i2c_dw_scl_hcnt` (common.c).
///
/// `IC_[FS]S_SCL_HCNT + 3 >= IC_CLK * (tHD;STA + tf)`. Der Takt kommt in
/// **kHz** herein, die Zeiten in **ns** — so rechnet Linux, und `MICRO`
/// ist der Nenner, der beides zusammenbringt.
pub fn scl_hcnt(ic_clk_khz: u32, tsymbol_ns: u32, tf_ns: u32, offset: i32) -> u32 {
    let n = ic_clk_khz as u64 * (tsymbol_ns + tf_ns) as u64;
    let v = div_round_closest(n, 1_000_000) as i64 - 3 + offset as i64;
    if v < 0 { 0 } else { v as u32 }
}

/// `i2c_dw_scl_lcnt` (common.c).
///
/// `IC_[FS]S_SCL_LCNT + 1 >= IC_CLK * (tLOW + tf)`. Die Fallzeit zaehlt
/// mit, weil der Block ab dem Ziehen der Leitung zaehlt.
pub fn scl_lcnt(ic_clk_khz: u32, tlow_ns: u32, tf_ns: u32, offset: i32) -> u32 {
    let n = ic_clk_khz as u64 * (tlow_ns + tf_ns) as u64;
    let v = div_round_closest(n, 1_000_000) as i64 - 1 + offset as i64;
    if v < 0 { 0 } else { v as u32 }
}

/// Der eingestellte Controller.
pub struct Dw {
    pub bus_freq_hz: u32,
    pub tx_fifo_depth: u32,
    pub rx_fifo_depth: u32,
    pub master_cfg: u32,
    pub ss_hcnt: u32,
    pub ss_lcnt: u32,
    pub fs_hcnt: u32,
    pub fs_lcnt: u32,
    pub sda_hold_time: u32,
}

impl Dw {
    /// `i2c_dw_set_timings_master` + `i2c_dw_set_fifo_size`.
    ///
    /// `sscn`/`fmcn` sind die Werte aus der Firmware (`SSCN`/`FMCN`), wenn
    /// sie welche liefert — **zuerst die Firmung fragen, erst dann
    /// rechnen**, wie `i2c_dw_acpi_params` es tut. Florians IdeaPad liefert
    /// keine; dann zaehlt `ic_clk_khz`.
    pub fn setup(
        bus: &mut dyn Bus,
        bus_freq_hz: u32,
        ic_clk_khz: u32,
        sscn: Option<(u16, u16, u32)>,
        fmcn: Option<(u16, u16, u32)>,
    ) -> Result<Dw, Error> {
        // Ist da ueberhaupt ein Designware-Block? Die Kennung ist die
        // billigste Probe, dass Abbildung und Adresse stimmen — und sie
        // sagt es, bevor irgendein Schreibzugriff ins Leere geht.
        let comp = bus.read32(DW_IC_COMP_TYPE);
        if comp != DW_IC_COMP_TYPE_VALUE {
            return Err(Error::NotDesignware(comp));
        }

        // Fallzeiten: Linux nimmt 300 ns, wenn die Plattform schweigt.
        let sda_fall = 300u32;
        let scl_fall = 300u32;

        let (mut ss_hcnt, mut ss_lcnt) = match sscn {
            Some((h, l, _)) if h != 0 && l != 0 => (h as u32, l as u32),
            _ => (
                scl_hcnt(ic_clk_khz, 4000, sda_fall, 0), // tHD;STA = tHIGH = 4.0 us
                scl_lcnt(ic_clk_khz, 4700, scl_fall, 0), // tLOW = 4.7 us
            ),
        };
        let (mut fs_hcnt, mut fs_lcnt) = match fmcn {
            Some((h, l, _)) if h != 0 && l != 0 => (h as u32, l as u32),
            _ => (
                scl_hcnt(ic_clk_khz, 600, sda_fall, 0),  // tHD;STA = tHIGH = 0.6 us
                scl_lcnt(ic_clk_khz, 1300, scl_fall, 0), // tLOW = 1.3 us
            ),
        };
        // Ohne Takt UND ohne Firmwarewerte gibt es nichts zu rechnen.
        if ic_clk_khz == 0 && (sscn.is_none() || fmcn.is_none()) {
            if sscn.is_none() { ss_hcnt = 0; ss_lcnt = 0; }
            if fmcn.is_none() { fs_hcnt = 0; fs_lcnt = 0; }
        }

        // `i2c_dw_set_sda_hold`: nur ab Version 1.11a, sonst gibt es das
        // Register nicht.
        let ver = bus.read32(DW_IC_COMP_VERSION);
        let sda_hold_time = match (sscn, fmcn, bus_freq_hz) {
            _ if ver < DW_IC_SDA_HOLD_MIN_VERS => 0,
            (_, Some((_, _, ht)), f) if f > I2C_MAX_STANDARD_MODE_FREQ => ht,
            (Some((_, _, ht)), _, _) => ht,
            _ => 0,
        };

        // `i2c_dw_set_fifo_size`: Tiefe steht in COMP_PARAM_1.
        let param = bus.read32(DW_IC_COMP_PARAM_1);
        let tx_fifo_depth = ((param >> 16) & 0xFF) + 1;
        let rx_fifo_depth = ((param >> 8) & 0xFF) + 1;

        // `i2c_dw_configure_master`.
        let speed_bits = if bus_freq_hz <= I2C_MAX_STANDARD_MODE_FREQ {
            DW_IC_CON_SPEED_STD
        } else {
            DW_IC_CON_SPEED_FAST
        };
        let master_cfg =
            DW_IC_CON_MASTER | DW_IC_CON_SLAVE_DISABLE | DW_IC_CON_RESTART_EN | speed_bits;

        Ok(Dw {
            bus_freq_hz,
            tx_fifo_depth,
            rx_fifo_depth,
            master_cfg,
            ss_hcnt,
            ss_lcnt,
            fs_hcnt,
            fs_lcnt,
            sda_hold_time,
        })
    }

    pub fn describe(&self) -> String {
        format!(
            "dw-i2c: {} Hz, fifo tx={} rx={}, SS {}:{}, FS {}:{}, sda_hold={}",
            self.bus_freq_hz, self.tx_fifo_depth, self.rx_fifo_depth,
            self.ss_hcnt, self.ss_lcnt, self.fs_hcnt, self.fs_lcnt, self.sda_hold_time
        )
    }
}

fn enable_nowait(bus: &mut dyn Bus, on: bool) {
    bus.write32(DW_IC_ENABLE, if on { DW_IC_ENABLE_ENABLE } else { 0 });
}

/// `__i2c_dw_disable` (common.c) — samt dem Abbruch, den die Databook-
/// Anmerkung verlangt, wenn der Block noch auf der Leitung haelt.
pub fn disable(bus: &mut dyn Bus, bus_freq_hz: u32) {
    let raw = bus.read32(DW_IC_RAW_INTR_STAT);
    let stat = bus.read32(DW_IC_STATUS);
    let mut enable = bus.read32(DW_IC_ENABLE);

    let abort_needed = (raw & DW_IC_INTR_MST_ON_HOLD) != 0
        || (stat & DW_IC_STATUS_MASTER_HOLD_TX_FIFO_EMPTY) != 0;
    if abort_needed {
        if enable & DW_IC_ENABLE_ENABLE == 0 {
            bus.write32(DW_IC_ENABLE, DW_IC_ENABLE_ENABLE);
            // Zehn Taktperioden warten, damit ENABLE wirklich steht —
            // bei 400 kHz sind das 25 us.
            let us = div_round_closest(10 * 1_000_000, bus_freq_hz.max(1) as u64) as u32;
            bus.udelay(us);
            enable |= DW_IC_ENABLE_ENABLE;
        }
        bus.write32(DW_IC_ENABLE, enable | DW_IC_ENABLE_ABORT);
        for _ in 0..10 {
            if bus.read32(DW_IC_ENABLE) & DW_IC_ENABLE_ABORT == 0 { break; }
            bus.udelay(10);
        }
    }

    for _ in 0..100 {
        enable_nowait(bus, false);
        // Das Statusregister darf fehlen; dann liest es 0 und wir sind fertig.
        if bus.read32(DW_IC_ENABLE_STATUS) & 1 == 0 { return; }
        bus.udelay(25);
    }
    bus.note("dw-i2c: timeout disabling adapter");
}

/// `i2c_dw_init_master` (master.c) — Schreibreihenfolge 1:1.
pub fn init_master(bus: &mut dyn Bus, dw: &Dw) {
    disable(bus, dw.bus_freq_hz);

    // SMBus-Interrupts stummschalten: eine Firmware, die IC_SMBUS=1
    // stehen laesst, erzeugt sonst einen Sturm, den niemand bedient.
    bus.write32(DW_IC_SMBUS_INTR_MASK, 0);

    bus.write32(DW_IC_SS_SCL_HCNT, dw.ss_hcnt);
    bus.write32(DW_IC_SS_SCL_LCNT, dw.ss_lcnt);
    bus.write32(DW_IC_FS_SCL_HCNT, dw.fs_hcnt);
    bus.write32(DW_IC_FS_SCL_LCNT, dw.fs_lcnt);

    if dw.sda_hold_time != 0 {
        bus.write32(DW_IC_SDA_HOLD, dw.sda_hold_time);
    }

    // `i2c_dw_configure_fifo_master`
    bus.write32(DW_IC_TX_TL, dw.tx_fifo_depth / 2);
    bus.write32(DW_IC_RX_TL, 0);
    bus.write32(DW_IC_CON, dw.master_cfg);
}

/// `i2c_dw_wait_bus_not_busy` — 20 ms, wie Linux.
fn wait_bus_not_busy(bus: &mut dyn Bus) -> Result<(), Error> {
    let deadline = bus.now_us() + 20_000;
    loop {
        if bus.read32(DW_IC_STATUS) & DW_IC_STATUS_ACTIVITY == 0 { return Ok(()); }
        if bus.now_us() >= deadline { return Err(Error::BusBusy); }
        bus.udelay(1100);
    }
}

/// Eine Nachricht auf dem Bus.
pub enum Msg<'a> {
    Write(&'a [u8]),
    Read(&'a mut [u8]),
}

impl Msg<'_> {
    fn is_read(&self) -> bool { matches!(self, Msg::Read(_)) }
    fn len(&self) -> usize {
        match self { Msg::Write(b) => b.len(), Msg::Read(b) => b.len() }
    }
}

/// Der Stand einer laufenden Uebertragung — die Felder aus `dw_i2c_dev`,
/// die Linux' Zustandsmaschine fuehrt.
struct Xfer {
    msg_write_idx: usize,
    msg_read_idx: usize,
    tx_pos: usize,
    rx_pos: usize,
    rx_outstanding: u32,
    status: u32,
    abort_source: u32,
    sw_mask: u32,
    aborted: bool,
}

/// `i2c_dw_read_clear_intrbits`, Pollfassung.
///
/// Im Pollbetrieb steht die HARDWARE-Maske auf 0 (sonst meldete der Block
/// Interrupts, die niemand abholt), und der Treiber fuehrt seine eigene.
/// Also den ROHEN Status lesen und selbst maskieren — genau das tut Linux
/// unter `ACCESS_POLLING`.
fn read_clear_intrbits(bus: &mut dyn Bus, x: &mut Xfer) -> u32 {
    let stat = bus.read32(DW_IC_RAW_INTR_STAT) & x.sw_mask;

    // NICHT ueber IC_CLR_INTR loeschen — zwischen Lesen und Loeschen
    // eingetroffene Meldungen gingen dabei verloren. Je Bit sein Register.
    if stat & DW_IC_INTR_RX_UNDER != 0 { bus.read32(DW_IC_CLR_RX_UNDER); }
    if stat & DW_IC_INTR_RX_OVER != 0 { bus.read32(DW_IC_CLR_RX_OVER); }
    if stat & DW_IC_INTR_TX_OVER != 0 { bus.read32(DW_IC_CLR_TX_OVER); }
    if stat & DW_IC_INTR_RD_REQ != 0 { bus.read32(DW_IC_CLR_RD_REQ); }
    if stat & DW_IC_INTR_TX_ABRT != 0 {
        // Die Quelle wird beim Lesen von CLR_TX_ABRT geloescht — vorher
        // sichern, sonst ist der Grund weg.
        x.abort_source = bus.read32(DW_IC_TX_ABRT_SOURCE);
        bus.read32(DW_IC_CLR_TX_ABRT);
    }
    if stat & DW_IC_INTR_RX_DONE != 0 { bus.read32(DW_IC_CLR_RX_DONE); }
    if stat & DW_IC_INTR_ACTIVITY != 0 { bus.read32(DW_IC_CLR_ACTIVITY); }
    if stat & DW_IC_INTR_STOP_DET != 0
        && (x.rx_outstanding == 0 || stat & DW_IC_INTR_RX_FULL != 0)
    {
        bus.read32(DW_IC_CLR_STOP_DET);
    }
    if stat & DW_IC_INTR_START_DET != 0 { bus.read32(DW_IC_CLR_START_DET); }
    if stat & DW_IC_INTR_GEN_CALL != 0 { bus.read32(DW_IC_CLR_GEN_CALL); }
    stat
}

/// `i2c_dw_xfer_init` (master.c).
fn xfer_init(bus: &mut dyn Bus, dw: &Dw, addr: u16, x: &mut Xfer) {
    enable_nowait(bus, false);
    bus.write32(DW_IC_TAR, addr as u32);

    // Interrupts ausdruecklich aus (Hardwarefehler in manchen Fassungen),
    // dann einschalten.
    bus.write32(DW_IC_INTR_MASK, 0);
    enable_nowait(bus, true);

    // Blindlesen: auf Bay Trail bleibt das Register sonst haengen.
    let _ = bus.read32(DW_IC_ENABLE_STATUS);
    let _ = bus.read32(DW_IC_CLR_INTR);

    // Im Pollbetrieb bleibt die Hardwaremaske auf 0; gefuehrt wird `sw_mask`.
    bus.write32(DW_IC_INTR_MASK, 0);
    x.sw_mask = DW_IC_INTR_MASTER_MASK;
    let _ = dw;
}

/// `i2c_dw_xfer_msg` (master.c) — die FIFOs fuellen.
fn xfer_msg(bus: &mut dyn Bus, dw: &Dw, msgs: &mut [Msg], x: &mut Xfer) {
    let mut intr_mask = DW_IC_INTR_MASTER_MASK;
    let mut need_restart = false;

    while x.msg_write_idx < msgs.len() {
        let is_read = msgs[x.msg_write_idx].is_read();
        let total = msgs[x.msg_write_idx].len();

        if x.status & STATUS_WRITE_IN_PROGRESS == 0 {
            x.tx_pos = 0;
            // Sind EMPTYFIFO_HOLD_MASTER_EN und RESTART_EN gesetzt, muss
            // das Restart-Bit zwischen Nachrichten von Hand kommen.
            if dw.master_cfg & DW_IC_CON_RESTART_EN != 0 && x.msg_write_idx > 0 {
                need_restart = true;
            }
        }

        let flr = bus.read32(DW_IC_TXFLR);
        let mut tx_limit = dw.tx_fifo_depth.saturating_sub(flr);
        let flr = bus.read32(DW_IC_RXFLR);
        let mut rx_limit = dw.rx_fifo_depth.saturating_sub(flr);

        while x.tx_pos < total && tx_limit > 0 && rx_limit > 0 {
            let mut cmd = 0u32;
            // Das Stop-Bit laesst sich aus den Registern nicht ablesen,
            // also wird es beim letzten Byte der letzten Nachricht immer
            // gesetzt.
            if x.msg_write_idx == msgs.len() - 1 && total - x.tx_pos == 1 {
                cmd |= 1 << 9;
            }
            if need_restart {
                cmd |= 1 << 10;
                need_restart = false;
            }
            if is_read {
                // Ueberlauf des Empfangspuffers vermeiden.
                if x.rx_outstanding >= dw.rx_fifo_depth { break; }
                bus.write32(DW_IC_DATA_CMD, cmd | 0x100);
                rx_limit -= 1;
                x.rx_outstanding += 1;
            } else {
                let byte = match &msgs[x.msg_write_idx] {
                    Msg::Write(b) => b[x.tx_pos] as u32,
                    Msg::Read(_) => 0,
                };
                bus.write32(DW_IC_DATA_CMD, cmd | byte);
            }
            tx_limit -= 1;
            x.tx_pos += 1;
        }

        if x.tx_pos < total {
            x.status |= STATUS_WRITE_IN_PROGRESS;
            break;
        }
        x.status &= !STATUS_WRITE_IN_PROGRESS;
        x.msg_write_idx += 1;
    }

    // Sind alle Nachmittel eingestellt, braucht es TX_EMPTY nicht mehr.
    if x.msg_write_idx == msgs.len() {
        intr_mask &= !DW_IC_INTR_TX_EMPTY;
    }
    if x.aborted {
        intr_mask = 0;
    }
    x.sw_mask = intr_mask;
}

/// `i2c_dw_read` (master.c) — was im FIFO steht, herausholen.
fn read_fifo(bus: &mut dyn Bus, msgs: &mut [Msg], x: &mut Xfer) {
    while x.msg_read_idx < msgs.len() {
        if !msgs[x.msg_read_idx].is_read() {
            x.msg_read_idx += 1;
            continue;
        }
        if x.status & STATUS_READ_IN_PROGRESS == 0 {
            x.rx_pos = 0;
        }
        let total = msgs[x.msg_read_idx].len();
        let mut rx_valid = bus.read32(DW_IC_RXFLR);

        while x.rx_pos < total && rx_valid > 0 {
            let v = (bus.read32(DW_IC_DATA_CMD) & 0xFF) as u8;
            if let Msg::Read(b) = &mut msgs[x.msg_read_idx] {
                b[x.rx_pos] = v;
            }
            x.rx_pos += 1;
            rx_valid -= 1;
            if x.rx_outstanding > 0 { x.rx_outstanding -= 1; }
        }

        if x.rx_pos < total {
            x.status |= STATUS_READ_IN_PROGRESS;
            return;
        }
        x.status &= !STATUS_READ_IN_PROGRESS;
        x.msg_read_idx += 1;
    }
}

/// Einen Abbruchgrund benennen. `7B_ADDR_NOACK` ist der Normalfall „da ist
/// nichts" und kein Unglueck.
fn abort_error(src: u32) -> Error {
    if src & ABRT_7B_ADDR_NOACK != 0 { return Error::AddrNack; }
    if src & ABRT_TXDATA_NOACK != 0 { return Error::DataNack; }
    if src & ARB_LOST != 0 { return Error::ArbLost; }
    Error::Abort(src)
}

/// Eine Folge von Nachrichten an `addr` uebertragen.
///
/// Dieselbe Zustandsmaschine wie Linux, nur aus einer Schleife gepumpt
/// statt aus der ISB: `read_clear_intrbits` → (`read_fifo` /
/// `xfer_msg`) → fertig bei `STOP_DET` und leerem Empfangskonto.
pub fn xfer(bus: &mut dyn Bus, dw: &Dw, addr: u16, msgs: &mut [Msg]) -> Result<(), Error> {
    wait_bus_not_busy(bus)?;

    let mut x = Xfer {
        msg_write_idx: 0, msg_read_idx: 0, tx_pos: 0, rx_pos: 0,
        rx_outstanding: 0, status: 0, abort_source: 0,
        sw_mask: DW_IC_INTR_MASTER_MASK, aborted: false,
    };
    xfer_init(bus, dw, addr, &mut x);

    // Linux gibt einer Uebertragung 1 s (adapter.timeout = HZ). Dasselbe.
    let deadline = bus.now_us() + 1_000_000;
    loop {
        let stat = read_clear_intrbits(bus, &mut x);

        // `i2c_dw_process_transfer`
        if stat & DW_IC_INTR_TX_ABRT != 0 {
            x.aborted = true;
            x.status &= !STATUS_MASK;
            x.rx_outstanding = 0;
            // Nach einem Abbruch sind beide FIFOs geleert — nichts mehr
            // nachschieben.
            bus.write32(DW_IC_INTR_MASK, 0);
            x.sw_mask = 0;
            disable(bus, dw.bus_freq_hz);
            return Err(abort_error(x.abort_source));
        }
        if stat & DW_IC_INTR_RX_FULL != 0 {
            read_fifo(bus, msgs, &mut x);
        }
        if stat & DW_IC_INTR_TX_EMPTY != 0 {
            xfer_msg(bus, dw, msgs, &mut x);
        }
        if stat & DW_IC_INTR_STOP_DET != 0 && x.rx_outstanding == 0 {
            // Noch im FIFO stehende Bytes holen, bevor abgeschaltet wird.
            read_fifo(bus, msgs, &mut x);
            break;
        }

        if bus.now_us() >= deadline {
            disable(bus, dw.bus_freq_hz);
            return Err(Error::Timeout);
        }
        bus.udelay(10);
    }

    // Linux wartet hier, bis der Block nicht mehr aktiv ist, und schaltet
    // dann ab (`i2c_dw_xfer` → `__i2c_dw_disable`).
    disable(bus, dw.bus_freq_hz);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Zaehler fuer Florians IdeaPad: `AMDI0010` faehrt mit 150 MHz,
    /// das Touchpad mit 400 kHz, und die Firmware liefert kein `FMCN`.
    ///
    /// Gegenprobe ueber die PERIODE statt ueber die Zahl: `hcnt + lcnt`
    /// Takte bei 150 MHz muessen ungefaehr einen Buszyklus ergeben. Eine
    /// Formel, die sich vertut, faellt hier auf — eine abgeschriebene Zahl
    /// nicht.
    #[test]
    fn ideapad_fast_mode_counts() {
        let clk_khz = 150_000; // 150 MHz aus acpi_apd.c
        let h = scl_hcnt(clk_khz, 600, 300, 0);
        let l = scl_lcnt(clk_khz, 1300, 300, 0);
        assert_eq!(h, 132);
        assert_eq!(l, 239);

        // Periode: (hcnt + lcnt) / 150 MHz, in ns.
        let period_ns = (h + l) as u64 * 1_000_000_000 / 150_000_000;
        // 400 kHz sind 2500 ns. Der Block legt noch ein paar Takte drauf,
        // also darf es knapp darunter liegen — aber nicht daneben.
        assert!(period_ns > 2_300 && period_ns < 2_600, "period {period_ns} ns");
    }

    /// Standardmodus, 100 kHz = 10 000 ns.
    #[test]
    fn standard_mode_counts() {
        let clk_khz = 150_000;
        let h = scl_hcnt(clk_khz, 4000, 300, 0);
        let l = scl_lcnt(clk_khz, 4700, 300, 0);
        assert_eq!(h, 642);
        assert_eq!(l, 749);
        let period_ns = (h + l) as u64 * 1_000_000_000 / 150_000_000;
        assert!(period_ns > 9_000 && period_ns < 10_200, "period {period_ns} ns");
    }

    /// Der Wacom-Digitizer auf demselben Notebook faehrt 1 MHz — derselbe
    /// Takt, andere Zahlen. Das prueft, dass nichts festverdrahtet ist.
    #[test]
    fn one_megahertz_is_faster_than_four_hundred_kilohertz() {
        let clk_khz = 150_000;
        let fast = scl_hcnt(clk_khz, 600, 300, 0) + scl_lcnt(clk_khz, 1300, 300, 0);
        // Fast-Mode-Plus: tHIGH 260 ns, tLOW 500 ns (css: i2c-designware).
        let fmp = scl_hcnt(clk_khz, 260, 300, 0) + scl_lcnt(clk_khz, 500, 300, 0);
        assert!(fmp < fast, "1 MHz muss weniger Takte je Zyklus brauchen");
    }
}
