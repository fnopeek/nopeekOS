//! Synopsys DesignWare I2C — the bus the touchpad sits on.
//!
//! Ported from Linux `drivers/i2c/busses/`: `i2c-designware-core.h`
//! (registers), `-common.c` (clock, SCL counts, disable, FIFO depth) and
//! `-master.c` (init and transfer). Function names are kept so both can be
//! read side by side.
//!
//! One deliberate deviation: Linux services the FIFOs from `i2c_dw_isr`.
//! This module may run without that interrupt, so the same state machine
//! is driven from a polling loop: `read_clear_intrbits` →
//! `process_transfer` → `xfer_msg`/`read`. Functions and order are
//! unchanged, only the trigger differs. Linux itself polls in
//! `amd_i2c_dw_xfer_quirk`, and the masking logic for it (`ACCESS_POLLING`,
//! `sw_mask`) is from the original.

use alloc::{format, string::String};

// ── Registers (i2c-designware-core.h) ────────────────────────────────
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

/// Abort sources that deserve a name (DW_IC_TX_ABRT_SOURCE).
const ABRT_7B_ADDR_NOACK: u32 = 1 << 0;
const ABRT_TXDATA_NOACK: u32 = 1 << 3;
const ARB_LOST: u32 = 1 << 12;

pub const I2C_MAX_STANDARD_MODE_FREQ: u32 = 100_000;
pub const I2C_MAX_FAST_MODE_FREQ: u32 = 400_000;

/// Hardware access. The driver computes, the caller accesses — so the same
/// code runs in the module and in the harness.
pub trait Bus {
    fn read32(&mut self, off: u32) -> u32;
    fn write32(&mut self, off: u32, val: u32);
    /// Wait at least `us` microseconds.
    fn udelay(&mut self, us: u32);
    /// Monotonic time in microseconds, for timeouts.
    fn now_us(&mut self) -> u64;
    fn note(&mut self, _s: &str) {}
}

#[derive(Debug, PartialEq)]
pub enum Error {
    /// No DesignWare block at this address.
    NotDesignware(u32),
    /// The bus did not become idle.
    BusBusy,
    /// The device did not acknowledge the address — usually: nothing there.
    AddrNack,
    /// The device aborted in the middle of a write.
    DataNack,
    /// Arbitration lost.
    ArbLost,
    /// Other abort, with the raw source register.
    Abort(u32),
    /// Timeout.
    Timeout,
}

/// Division rounded to nearest — Linux `DIV_ROUND_CLOSEST_ULL`.
fn div_round_closest(n: u64, d: u64) -> u64 {
    (n + d / 2) / d
}

/// `i2c_dw_scl_hcnt` (common.c).
///
/// `IC_[FS]S_SCL_HCNT + 3 >= IC_CLK * (tHD;STA + tf)`. The clock is in kHz,
/// the times in ns, as in Linux; `MICRO` is the denominator that joins
/// them.
pub fn scl_hcnt(ic_clk_khz: u32, tsymbol_ns: u32, tf_ns: u32, offset: i32) -> u32 {
    let n = ic_clk_khz as u64 * (tsymbol_ns + tf_ns) as u64;
    let v = div_round_closest(n, 1_000_000) as i64 - 3 + offset as i64;
    if v < 0 { 0 } else { v as u32 }
}

/// `i2c_dw_scl_lcnt` (common.c).
///
/// `IC_[FS]S_SCL_LCNT + 1 >= IC_CLK * (tLOW + tf)`. The fall time counts
/// because the block starts counting when the line is pulled.
pub fn scl_lcnt(ic_clk_khz: u32, tlow_ns: u32, tf_ns: u32, offset: i32) -> u32 {
    let n = ic_clk_khz as u64 * (tlow_ns + tf_ns) as u64;
    let v = div_round_closest(n, 1_000_000) as i64 - 1 + offset as i64;
    if v < 0 { 0 } else { v as u32 }
}

/// The configured controller.
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
    /// `sscn`/`fmcn` are the firmware values (`SSCN`/`FMCN`) if it provides
    /// any. As in `i2c_dw_acpi_params`, they take precedence; otherwise the
    /// counts are computed from `ic_clk_khz`.
    pub fn setup(
        bus: &mut dyn Bus,
        bus_freq_hz: u32,
        ic_clk_khz: u32,
        sscn: Option<(u16, u16, u32)>,
        fmcn: Option<(u16, u16, u32)>,
    ) -> Result<Dw, Error> {
        // Is there a DesignWare block at all? The component type is the
        // cheapest proof that mapping and address are right, and it answers
        // before any write goes nowhere.
        let comp = bus.read32(DW_IC_COMP_TYPE);
        if comp != DW_IC_COMP_TYPE_VALUE {
            return Err(Error::NotDesignware(comp));
        }

        // Fall times: Linux uses 300 ns when the platform says nothing.
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
        // Without a clock and without firmware values there is nothing to compute.
        if ic_clk_khz == 0 && (sscn.is_none() || fmcn.is_none()) {
            if sscn.is_none() { ss_hcnt = 0; ss_lcnt = 0; }
            if fmcn.is_none() { fs_hcnt = 0; fs_lcnt = 0; }
        }

        // `i2c_dw_set_sda_hold`: only from version 1.11a; the register does
        // not exist before that.
        let ver = bus.read32(DW_IC_COMP_VERSION);
        let sda_hold_time = match (sscn, fmcn, bus_freq_hz) {
            _ if ver < DW_IC_SDA_HOLD_MIN_VERS => 0,
            (_, Some((_, _, ht)), f) if f > I2C_MAX_STANDARD_MODE_FREQ => ht,
            (Some((_, _, ht)), _, _) => ht,
            _ => 0,
        };

        // `i2c_dw_set_fifo_size`: depth is in COMP_PARAM_1.
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

/// `__i2c_dw_disable` (common.c) — including the abort the databook note
/// requires when the block is still holding the line.
pub fn disable(bus: &mut dyn Bus, bus_freq_hz: u32) {
    let raw = bus.read32(DW_IC_RAW_INTR_STAT);
    let stat = bus.read32(DW_IC_STATUS);
    let mut enable = bus.read32(DW_IC_ENABLE);

    let abort_needed = (raw & DW_IC_INTR_MST_ON_HOLD) != 0
        || (stat & DW_IC_STATUS_MASTER_HOLD_TX_FIFO_EMPTY) != 0;
    if abort_needed {
        if enable & DW_IC_ENABLE_ENABLE == 0 {
            bus.write32(DW_IC_ENABLE, DW_IC_ENABLE_ENABLE);
            // Wait ten clock periods so ENABLE really settles; 25 us at
            // 400 kHz.
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
        // The status register may be absent; it then reads 0 and we are done.
        if bus.read32(DW_IC_ENABLE_STATUS) & 1 == 0 { return; }
        bus.udelay(25);
    }
    bus.note("dw-i2c: timeout disabling adapter");
}

/// `i2c_dw_init_master` (master.c) — write order 1:1.
pub fn init_master(bus: &mut dyn Bus, dw: &Dw) {
    disable(bus, dw.bus_freq_hz);

    // Mask SMBus interrupts: firmware that leaves IC_SMBUS=1 would
    // otherwise cause a storm nobody services.
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

/// `i2c_dw_wait_bus_not_busy` — 20 ms, as in Linux.
fn wait_bus_not_busy(bus: &mut dyn Bus) -> Result<(), Error> {
    let deadline = bus.now_us() + 20_000;
    loop {
        if bus.read32(DW_IC_STATUS) & DW_IC_STATUS_ACTIVITY == 0 { return Ok(()); }
        if bus.now_us() >= deadline { return Err(Error::BusBusy); }
        bus.udelay(1100);
    }
}

/// One message on the bus.
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

/// State of a running transfer — the `dw_i2c_dev` fields that Linux' state
/// machine tracks.
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

/// `i2c_dw_read_clear_intrbits`, polling variant.
///
/// When polling, the hardware mask stays 0 (otherwise the block would raise
/// interrupts nobody collects) and the driver keeps its own. So read the
/// raw status and mask in software, as Linux does under `ACCESS_POLLING`.
fn read_clear_intrbits(bus: &mut dyn Bus, x: &mut Xfer) -> u32 {
    let stat = bus.read32(DW_IC_RAW_INTR_STAT) & x.sw_mask;

    // Do not clear via IC_CLR_INTR: events arriving between read and clear
    // would be lost. One clear register per bit.
    if stat & DW_IC_INTR_RX_UNDER != 0 { bus.read32(DW_IC_CLR_RX_UNDER); }
    if stat & DW_IC_INTR_RX_OVER != 0 { bus.read32(DW_IC_CLR_RX_OVER); }
    if stat & DW_IC_INTR_TX_OVER != 0 { bus.read32(DW_IC_CLR_TX_OVER); }
    if stat & DW_IC_INTR_RD_REQ != 0 { bus.read32(DW_IC_CLR_RD_REQ); }
    if stat & DW_IC_INTR_TX_ABRT != 0 {
        // The source is cleared by reading CLR_TX_ABRT; save it first or
        // the reason is gone.
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

    // Explicitly disable interrupts (hardware bug in some versions), then
    // enable.
    bus.write32(DW_IC_INTR_MASK, 0);
    enable_nowait(bus, true);

    // Dummy read: on Bay Trail the register otherwise gets stuck.
    let _ = bus.read32(DW_IC_ENABLE_STATUS);
    let _ = bus.read32(DW_IC_CLR_INTR);

    // When polling the hardware mask stays 0; `sw_mask` is used instead.
    bus.write32(DW_IC_INTR_MASK, 0);
    x.sw_mask = DW_IC_INTR_MASTER_MASK;
    let _ = dw;
}

/// `i2c_dw_xfer_msg` (master.c) — fill the FIFOs.
fn xfer_msg(bus: &mut dyn Bus, dw: &Dw, msgs: &mut [Msg], x: &mut Xfer) {
    let mut intr_mask = DW_IC_INTR_MASTER_MASK;
    let mut need_restart = false;

    while x.msg_write_idx < msgs.len() {
        let is_read = msgs[x.msg_write_idx].is_read();
        let total = msgs[x.msg_write_idx].len();

        if x.status & STATUS_WRITE_IN_PROGRESS == 0 {
            x.tx_pos = 0;
            // With EMPTYFIFO_HOLD_MASTER_EN and RESTART_EN set, the restart
            // bit between messages must be issued explicitly.
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
            // The stop bit cannot be read back from the registers, so it is
            // always set on the last byte of the last message.
            if x.msg_write_idx == msgs.len() - 1 && total - x.tx_pos == 1 {
                cmd |= 1 << 9;
            }
            if need_restart {
                cmd |= 1 << 10;
                need_restart = false;
            }
            if is_read {
                // Avoid overflowing the receive buffer.
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

    // Once all messages are queued, TX_EMPTY is no longer needed.
    if x.msg_write_idx == msgs.len() {
        intr_mask &= !DW_IC_INTR_TX_EMPTY;
    }
    if x.aborted {
        intr_mask = 0;
    }
    x.sw_mask = intr_mask;
}

/// `i2c_dw_read` (master.c) — drain what is in the FIFO.
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

/// Name an abort source. `7B_ADDR_NOACK` is the normal "nothing there" case,
/// not a failure.
fn abort_error(src: u32) -> Error {
    if src & ABRT_7B_ADDR_NOACK != 0 { return Error::AddrNack; }
    if src & ABRT_TXDATA_NOACK != 0 { return Error::DataNack; }
    if src & ARB_LOST != 0 { return Error::ArbLost; }
    Error::Abort(src)
}

/// Transfer a sequence of messages to `addr`.
///
/// The same state machine as Linux, pumped from a loop instead of the ISR:
/// `read_clear_intrbits` → (`read_fifo` / `xfer_msg`) → done on `STOP_DET`
/// with no reads outstanding.
pub fn xfer(bus: &mut dyn Bus, dw: &Dw, addr: u16, msgs: &mut [Msg]) -> Result<(), Error> {
    wait_bus_not_busy(bus)?;

    let mut x = Xfer {
        msg_write_idx: 0, msg_read_idx: 0, tx_pos: 0, rx_pos: 0,
        rx_outstanding: 0, status: 0, abort_source: 0,
        sw_mask: DW_IC_INTR_MASTER_MASK, aborted: false,
    };
    xfer_init(bus, dw, addr, &mut x);

    // Linux gives a transfer 1 s (adapter.timeout = HZ); same here.
    //
    // Linux sleeps that second (`wait_for_completion_timeout`); we poll it,
    // so once the transfer stalls we yield instead of spinning.
    //
    // DW_IC_RX_TL is 0, so the block reports every received byte, and a
    // byte takes at most 90 us even at 100 kHz. Two milliseconds without
    // any event means nothing is in flight.
    //
    // Overflow while sleeping is impossible: `xfer_msg` never queues more
    // reads than the RX FIFO is deep (`rx_outstanding >= rx_fifo_depth`
    // stops it), so whatever arrives always fits. A late wakeup only adds
    // latency.
    let deadline = bus.now_us() + 1_000_000;
    const STALLED_US: u64 = 2_000;
    let mut last_progress = bus.now_us();
    loop {
        let stat = read_clear_intrbits(bus, &mut x);
        if stat != 0 { last_progress = bus.now_us(); }

        // `i2c_dw_process_transfer`
        if stat & DW_IC_INTR_TX_ABRT != 0 {
            x.aborted = true;
            x.status &= !STATUS_MASK;
            x.rx_outstanding = 0;
            // After an abort both FIFOs are flushed; queue nothing more.
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
            // Drain bytes still in the FIFO before disabling.
            read_fifo(bus, msgs, &mut x);
            break;
        }

        let now = bus.now_us();
        if now >= deadline {
            disable(bus, dw.bus_freq_hz);
            return Err(Error::Timeout);
        }
        if now.saturating_sub(last_progress) > STALLED_US {
            // When stalled, yield instead of spinning. `udelay` of 1 ms or
            // more yields to the scheduler; below that it busy-waits.
            bus.udelay(1000);
        } else {
            bus.udelay(10);
        }
    }

    // Linux waits here until the block is no longer active and then
    // disables it (`i2c_dw_xfer` → `__i2c_dw_disable`).
    disable(bus, dw.bus_freq_hz);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts for `AMDI0010` at 150 MHz driving a 400 kHz touchpad, with no
    /// `FMCN` from the firmware.
    ///
    /// Checked via the period rather than the number: `hcnt + lcnt` clocks
    /// at 150 MHz must give roughly one bus cycle. A wrong formula fails
    /// here; a copied constant would not.
    #[test]
    fn ideapad_fast_mode_counts() {
        let clk_khz = 150_000; // 150 MHz from acpi_apd.c
        let h = scl_hcnt(clk_khz, 600, 300, 0);
        let l = scl_lcnt(clk_khz, 1300, 300, 0);
        assert_eq!(h, 132);
        assert_eq!(l, 239);

        // Period: (hcnt + lcnt) / 150 MHz, in ns.
        let period_ns = (h + l) as u64 * 1_000_000_000 / 150_000_000;
        // 400 kHz is 2500 ns. The block adds a few clocks, so slightly
        // below is fine, but not far off.
        assert!(period_ns > 2_300 && period_ns < 2_600, "period {period_ns} ns");
    }

    /// Standard mode, 100 kHz = 10 000 ns.
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

    /// A digitizer at 1 MHz on the same controller: same clock, different
    /// counts. Checks that nothing is hardwired.
    #[test]
    fn one_megahertz_is_faster_than_four_hundred_kilohertz() {
        let clk_khz = 150_000;
        let fast = scl_hcnt(clk_khz, 600, 300, 0) + scl_lcnt(clk_khz, 1300, 300, 0);
        // Fast-mode plus: tHIGH 260 ns, tLOW 500 ns.
        let fmp = scl_hcnt(clk_khz, 260, 300, 0) + scl_lcnt(clk_khz, 500, 300, 0);
        assert!(fmp < fast, "1 MHz muss weniger Takte je Zyklus brauchen");
    }
}
