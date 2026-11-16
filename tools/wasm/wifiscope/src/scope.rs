//! The radar probe that runs after stage 4c in place of the link stages.
//!
//! An ADC is read through the sample port the DAC calibration uses
//! (`0x2dbc`, path selected in `0x1c3c`, rtw8822c.c:394-470) while path A is
//! in its receive state or in TX mode without a baseband signal, so that A
//! sends its bare carrier (its LO leakage). Received with the same LO the
//! carrier is a constant in baseband: the mean of a block of samples is one
//! complex channel value.
//!
//! B's ADC shows nothing while A transmits, whether B is held in receive or
//! not, so the measurement is monostatic: A's own receiver, which keeps
//! running while A transmits, through A's antenna.
//!
//! Four measurements, each written to npkFS as CSV for host-side analysis:
//!
//! - gate 0: path A's port once, then path B's after each cumulative step
//!   of the setup, plus one raw block of B in normal receive.
//! - matrix: both ports with A in receive and in TX, B under the BB or held.
//! - gate 1: A's carrier off and on in turn on A's port, with the hand
//!   prompts.
//! - gate 2: the same off/on pair across channel switches, for the phase
//!   after a retune and against frequency.
//!
//! The chip is powered off by the caller afterwards; restoring here only
//! ends the transmission and gives the antenna back to coexistence.
//!
//! RF 0x00 is written through the direct window and read back; if the
//! value does not hold, once more over the serial path with that path's
//! 3-wire enabled for the write. Every write and its read-back go to
//! `diag.csv`.

use alloc::vec::Vec;

use crate::chip;
use crate::coex;
use crate::host;
use crate::phy;
use crate::regs::*;

/// Sample port: `[21:12]` I, `[9:0]` Q, ten bits two's complement.
const SAMPLE_PORT: u32 = 0x2dbc;
/// Sample source select; path B raw ADC as in `rtw8822c_dac_cal_adc`.
const SAMPLE_SEL: u32 = 0x1c3c;
const SAMPLE_SEL_B: u32 = 0x80000 + 0x8003;
const SAMPLE_SEL_A: u32 = 0xa0000 + 0x8003;

/// RF 0x00 bits that must read back as written: the mode. The gain bits
/// read back as the RF holds them, not as written.
const RF0_CHECK: u32 = 0xf0000;
/// Lines kept in `diag.csv`.
const DIAG_MAX: usize = 400;

/// RF 0x00 `[19:16]`.
const RF_MODE_TX: u32 = 0x20000;

/// A's RF 0x00 in the BB's normal receive state, the carrier's off state.
/// Standby is not used: with A in standby B receives nothing either.
static A_IDLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// B's RF 0x00 in the BB's normal receive state, held while B's 3-wire is
/// off so the BB cannot gate B while A transmits.
static B_IDLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Whether gates 1 and 2 hold B that way; chosen by the matrix.
static B_HOLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Samples per block.
const BLOCK: usize = 2048;
/// Samples in the raw dump.
const RAW: usize = 4096;

/// TX gain index of the carrier: the lowest.
const TX_LOW: u32 = 0x00;

/// Band 4: the only band where a carrier showed on B.
const CH_START: u8 = 149;
/// Gate 2 stays off the DFS channels 52 to 144.
const CH_SWEEP: [u8; 9] = [36, 40, 44, 48, 149, 153, 157, 161, 165];
const SWEEP_ROUNDS: u32 = 6;

/// Registers the probe changes, restored before the chip goes off.
const BB_SAVE: [u32; 19] = [
    0x180c, 0x1810, 0x410c, 0x4110,
    0x1c3c, 0x1c24, 0x1d70, 0x9b4,
    0x1a00, 0x1a14, 0x1d58, 0x1c38,
    0x1e24, 0x1e28, 0x1860, 0x4160,
    0x1b00, 0x520, REG_RFTXEN_GCK_A,
];
const RF_SAVE: [u32; 1] = [0x00];

static DIAG: npk_sys::cell::Single<(Vec<u8>, usize)> =
    npk_sys::cell::Single::new((Vec::new(), 0));

fn diag(what: &str, path: usize, want: u32, got: u32, how: &str) {
    DIAG.with(|(out, lines)| {
        if *lines >= DIAG_MAX {
            return;
        }
        *lines += 1;
        put_i(out, host::now_ms() as i64);
        out.push(b',');
        put(out, what);
        out.push(b',');
        put(out, if path == phy::RF_PATH_A { "A" } else { "B" });
        put(out, ",0x");
        put_hex(out, want);
        put(out, ",0x");
        put_hex(out, got);
        out.push(b',');
        put(out, how);
        out.push(b'\n');
    });
}

fn put_hex(out: &mut Vec<u8>, v: u32) {
    for k in (0..5).rev() {
        out.push(b"0123456789abcdef"[((v >> (k * 4)) & 0xf) as usize]);
    }
}

fn rf0_read(h: i32, path: usize) -> u32 {
    phy::read_rf(h, path, 0x00, phy::RFREG_MASK)
}

/// Write RF 0x00 of `path` and confirm it by reading back.
fn rf0(h: i32, path: usize, val: u32, what: &str) -> bool {
    phy::write_rf_reg(h, path, 0x00, phy::RFREG_MASK, val);
    host::delay_us(5);
    let got = rf0_read(h, path);
    if got & RF0_CHECK == val & RF0_CHECK {
        diag(what, path, val, got, "direct");
        return true;
    }
    diag(what, path, val, got, "direct-failed");
    let reg = if path == phy::RF_PATH_A { REG_3WIRE } else { REG_3WIRE2 };
    let old = host::r32(h, reg);
    host::w32_mask(h, reg, BIT_3WIRE_EN, 0x3);
    phy::write_rf_reg_mix(h, path, 0x00, phy::RFREG_MASK, val);
    host::delay_us(20);
    host::w32(h, reg, old);
    let got = rf0_read(h, path);
    let ok = got & RF0_CHECK == val & RF0_CHECK;
    diag(what, path, val, got, if ok { "sipi" } else { "sipi-failed" });
    ok
}

struct Saved {
    bb: [u32; BB_SAVE.len()],
    rf: [[u32; 2]; RF_SAVE.len()],
    bcc: u8,
    gnt_ctl: u32,
    gnt_val: u32,
}

#[derive(Clone, Copy, Default)]
struct Stat {
    n: u32,
    si: i64,
    sq: i64,
    sii: i64,
    sqq: i64,
    siq: i64,
    min_i: i32,
    max_i: i32,
    min_q: i32,
    max_q: i32,
    dt_us: u64,
}

fn s10(v: u32) -> i32 {
    let v = (v & 0x3ff) as i32;
    if v >= 0x200 { v - 0x400 } else { v }
}

fn sample(h: i32) -> (i32, i32) {
    let w = host::r32_mask(h, SAMPLE_PORT, 0x3fffff);
    (s10(w >> 12), s10(w))
}

/// Sums of `n` samples: (I, Q).
fn probe(h: i32, n: usize) -> (i64, i64) {
    let (mut si, mut sq) = (0i64, 0i64);
    for _ in 0..n {
        let (i, q) = sample(h);
        si += i as i64;
        sq += q as i64;
    }
    (si, sq)
}

/// Samples in the check that the carrier came on.
const PROBE: usize = 256;
/// The carrier counts as on when the mean moves by this much.
const CARRIER_MIN: i64 = 10;
/// Switch-ons before a pair is taken as it is.
const TRIES: u32 = 4;

/// One off/on pair. The carrier does not come on at every switch-on; a
/// short probe checks it and switches again, up to `TRIES` times.
fn pair(h: i32, txg: u32) -> (Stat, Stat, u32) {
    carrier(h, None);
    let off = block(h);
    let n = off.n as i64;
    let mut tries = 0;
    loop {
        tries += 1;
        carrier(h, Some(txg));
        let (pi, pq) = probe(h, PROBE);
        let di = pi * n - off.si * PROBE as i64;
        let dq = pq * n - off.sq * PROBE as i64;
        let lim = CARRIER_MIN * PROBE as i64 * n;
        if di * di + dq * dq >= lim * lim || tries >= TRIES {
            break;
        }
        carrier(h, None);
        host::delay_us(500);
    }
    let on = block(h);
    (off, on, tries)
}

fn block(h: i32) -> Stat {
    let mut s = Stat { min_i: i32::MAX, max_i: i32::MIN, min_q: i32::MAX,
                       max_q: i32::MIN, ..Stat::default() };
    let t0 = host::now_us();
    for _ in 0..BLOCK {
        let (i, q) = sample(h);
        s.n += 1;
        s.si += i as i64;
        s.sq += q as i64;
        s.sii += (i * i) as i64;
        s.sqq += (q * q) as i64;
        s.siq += (i * q) as i64;
        s.min_i = s.min_i.min(i);
        s.max_i = s.max_i.max(i);
        s.min_q = s.min_q.min(q);
        s.max_q = s.max_q.max(q);
    }
    s.dt_us = host::now_us() - t0;
    s
}

// ── CSV ──────────────────────────────────────────────────────────

fn put(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
}

fn put_i(out: &mut Vec<u8>, v: i64) {
    if v < 0 {
        out.push(b'-');
    }
    let mut u = v.unsigned_abs();
    let mut d = [0u8; 20];
    let mut k = d.len();
    loop {
        k -= 1;
        d[k] = b'0' + (u % 10) as u8;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    out.extend_from_slice(&d[k..]);
}

const STAT_HEAD: &str = "n,sum_i,sum_q,sum_ii,sum_qq,sum_iq,min_i,max_i,min_q,max_q,dt_us\n";

fn put_stat(out: &mut Vec<u8>, s: &Stat) {
    for v in [s.n as i64, s.si, s.sq, s.sii, s.sqq, s.siq,
              s.min_i as i64, s.max_i as i64, s.min_q as i64, s.max_q as i64] {
        put_i(out, v);
        out.push(b',');
    }
    put_i(out, s.dt_us as i64);
    out.push(b'\n');
}

fn head(out: &mut Vec<u8>, what: &str) {
    put(out, "# wifiscope ");
    put(out, crate::DRIVER_VERSION);
    put(out, " ");
    put(out, what);
    put(out, "\n# block ");
    put_i(out, BLOCK as i64);
    put(out, " samples, 10-bit I/Q, carrier from path A\n");
}

fn save(name: &str, out: &[u8]) {
    let mut path = Vec::with_capacity(32);
    path.extend_from_slice(b"wifiscope/");
    path.extend_from_slice(name.as_bytes());
    let rc = npk_sys::store(&path, out);
    host::say("[wifiscope] ");
    host::say(name);
    host::say(if rc == 0 { " written\n" } else { " NOT written\n" });
}

// ── Chip state ───────────────────────────────────────────────────

fn save_state(h: i32) -> Saved {
    let mut s = Saved {
        bb: [0; BB_SAVE.len()],
        rf: [[0; 2]; RF_SAVE.len()],
        bcc: host::r8(h, 0x1bcc),
        gnt_ctl: host::r32(h, 0x70),
        gnt_val: coex::read_indirect_reg(h, 0x38),
    };
    for (k, &r) in BB_SAVE.iter().enumerate() {
        s.bb[k] = host::r32(h, r);
    }
    for (k, &r) in RF_SAVE.iter().enumerate() {
        s.rf[k][0] = phy::read_rf(h, phy::RF_PATH_A, r, phy::RFREG_MASK);
        s.rf[k][1] = phy::read_rf(h, phy::RF_PATH_B, r, phy::RFREG_MASK);
    }
    s
}

fn restore_state(h: i32, s: &Saved) {
    carrier(h, None);
    host::w8(h, 0x1bcc, s.bcc);
    for (k, &r) in BB_SAVE.iter().enumerate() {
        host::w32(h, r, s.bb[k]);
    }
    // 3-wire is back on: RF 0x00 takes the serial path again.
    for (k, &r) in RF_SAVE.iter().enumerate() {
        phy::write_rf_reg_mix(h, phy::RF_PATH_A, r, phy::RFREG_MASK, s.rf[k][0]);
        phy::write_rf_reg_mix(h, phy::RF_PATH_B, r, phy::RFREG_MASK, s.rf[k][1]);
    }
    coex::write_indirect_reg(h, 0x38, MASKDWORD, s.gnt_val);
    host::w32(h, 0x70, s.gnt_ctl);
}

/// The setup, one cumulative step at a time. Gate 0 measures B after each;
/// the first `MINIMAL` steps are what gates 1 and 2 run with, leaving B's
/// receive path to the BB. The rest is the DAC calibration's BB setting
/// (`rtw8822c_dac_bb_setting`), under which B has been seen to go silent.
const STEPS: [&str; 14] = [
    "normal", "gnt_wl", "tx_pause", "sharerx_txgat_off", "txen_gck_a",
    "3wire_a_off",
    "r1d58", "r1a00", "r1a14", "igi_7e", "3wire_b_off", "nctl_1b00_1bcc",
    "r1e24", "r1e28",
];
const MINIMAL: usize = 6;

fn step(h: i32, k: usize) {
    match k {
        1 => {
            host::w32_mask(h, 0x70, 1 << 26, 0x1);
            coex::write_indirect_reg(h, 0x38, MASKBYTE1, 0x77);
        }
        2 => host::w8(h, 0x522, 0xff),
        3 => {
            host::w32_mask(h, REG_3WIRE, BIT_DIS_SHARERX_TXGAT, 0x1);
            host::w32_mask(h, REG_3WIRE2, BIT_DIS_SHARERX_TXGAT, 0x1);
        }
        4 => host::w32_mask(h, REG_RFTXEN_GCK_A, BIT_RFTXEN_GCK_FORCE_ON, 0x1),
        5 => host::w32_mask(h, REG_3WIRE, BIT_3WIRE_EN, 0x0),
        6 => host::w32_mask(h, 0x1d58, 0xff8, 0x1ff),
        7 => host::w32_mask(h, 0x1a00, 0x3, 0x2),
        8 => host::w32_mask(h, 0x1a14, 0x300, 0x3),
        9 => host::w32(h, 0x1d70, 0x7e7e7e7e),
        10 => host::w32_mask(h, REG_3WIRE2, BIT_3WIRE_EN, 0x0),
        11 => {
            host::w32(h, 0x1b00, 0x0000_0008);
            host::w8(h, 0x1bcc, 0x3f);
            host::w32(h, 0x1b00, 0x0000_000a);
            host::w8(h, 0x1bcc, 0x3f);
        }
        12 => host::w32_mask(h, 0x1e24, 1 << 31, 0x0),
        13 => host::w32_mask(h, 0x1e28, 0xf, 0x3),
        _ => {}
    }
}

fn sample_b(h: i32) {
    host::w32(h, 0x1c24, 0x0001_0002);
    host::w32(h, SAMPLE_SEL, SAMPLE_SEL_B);
}

/// The setup gates 1 and 2 run with, on A's port.
fn setup_minimal(h: i32) {
    for k in 1..MINIMAL {
        step(h, k);
    }
    host::w32(h, 0x1c24, 0x0001_0002);
    host::w32(h, SAMPLE_SEL, SAMPLE_SEL_A);
}

/// A's carrier at TX gain index `gain`, or standby.
fn carrier(h: i32, gain: Option<u32>) {
    let (v, what) = match gain {
        Some(g) => (RF_MODE_TX | g, "carrier_on"),
        None => (A_IDLE.load(core::sync::atomic::Ordering::Relaxed), "carrier_off"),
    };
    rf0(h, phy::RF_PATH_A, v, what);
    host::delay_us(100);
}

fn tune(h: i32, ch: u8) {
    chip::set_channel(h, ch, 0, RTW_SC_DONT_CARE);
    setup_minimal(h);
    b_hold(h, B_HOLD.load(core::sync::atomic::Ordering::Relaxed));
    carrier(h, None);
    host::delay_us(2000);
}

/// B under the BB (3-wire on) or held in its receive state (3-wire off).
fn b_hold(h: i32, hold: bool) {
    if hold {
        host::w32_mask(h, REG_3WIRE2, BIT_3WIRE_EN, 0x0);
        rf0(h, phy::RF_PATH_B, B_IDLE.load(core::sync::atomic::Ordering::Relaxed), "b_hold");
    } else {
        host::w32_mask(h, REG_3WIRE2, BIT_3WIRE_EN, 0x3);
    }
    host::delay_us(200);
}

/// Which port shows a receiver in which state: B held or not, A in
/// receive or TX, port A or B. Three rounds of three blocks each. Returns
/// whether holding B keeps it receiving while A transmits.
fn matrix(h: i32) -> bool {
    host::say("[wifiscope] matrix: which path hears while A transmits\n");
    let mut out = Vec::new();
    head(&mut out, "matrix");
    put(&mut out, "round,b_hold,a_tx,port,rf0_a,rf0_b,");
    put(&mut out, STAT_HEAD);
    let mut alive = [0u32; 2];
    for round in 0..3 {
        for hold in [false, true] {
            b_hold(h, hold);
            for a_tx in [false, true] {
                carrier(h, if a_tx { Some(TX_LOW) } else { None });
                for port in [phy::RF_PATH_A, phy::RF_PATH_B] {
                    host::w32(h, SAMPLE_SEL,
                              if port == phy::RF_PATH_A { SAMPLE_SEL_A } else { SAMPLE_SEL_B });
                    host::delay_us(50);
                    for _ in 0..3 {
                        let s = block(h);
                        let n = s.n as i64;
                        let var = (s.sii + s.sqq) - (s.si * s.si + s.sq * s.sq) / n;
                        if a_tx && port == phy::RF_PATH_B && var > 25 * n {
                            alive[hold as usize] += 1;
                        }
                        put_i(&mut out, round);
                        out.push(b',');
                        put_i(&mut out, hold as i64);
                        out.push(b',');
                        put_i(&mut out, a_tx as i64);
                        out.push(b',');
                        put(&mut out, if port == phy::RF_PATH_A { "A," } else { "B," });
                        put_i(&mut out, rf0_read(h, phy::RF_PATH_A) as i64);
                        out.push(b',');
                        put_i(&mut out, rf0_read(h, phy::RF_PATH_B) as i64);
                        out.push(b',');
                        put_stat(&mut out, &s);
                    }
                }
            }
            carrier(h, None);
        }
    }
    let hold = alive[1] > alive[0];
    put(&mut out, if hold { "# chosen: b_hold 1\n" } else { "# chosen: b_hold 0\n" });
    save("matrix.csv", &out);
    b_hold(h, hold);
    sample_b(h);
    hold
}

// ── Gates ────────────────────────────────────────────────────────

fn gate0_rows(h: i32, out: &mut Vec<u8>, src: &str, reps: i64) {
    for rep in 0..reps {
        let s = block(h);
        put(out, src);
        put(out, ",-1,");
        put_i(out, rep);
        out.push(b',');
        put_stat(out, &s);
    }
}

fn raw_dump(h: i32) {
    let mut raw = Vec::with_capacity(RAW * 16);
    head(&mut raw, "gate0 raw, path B, BB normal");
    put(&mut raw, "t_us,i,q\n");
    let t0 = host::now_us();
    for _ in 0..RAW {
        let (i, q) = sample(h);
        put_i(&mut raw, (host::now_us() - t0) as i64);
        raw.push(b',');
        put_i(&mut raw, i as i64);
        raw.push(b',');
        put_i(&mut raw, q as i64);
        raw.push(b'\n');
    }
    save("gate0_raw.csv", &raw);
}

fn gate0(h: i32, saved: &Saved) {
    host::say("[wifiscope] gate 0: path B after each setup step\n");
    let mut out = Vec::new();
    head(&mut out, "gate0");
    put(&mut out, "src,rx_gain,rep,");
    put(&mut out, STAT_HEAD);

    diag("live", phy::RF_PATH_A, 0, rf0_read(h, phy::RF_PATH_A), "read");
    diag("live", phy::RF_PATH_B, 0, rf0_read(h, phy::RF_PATH_B), "read");
    host::w32(h, 0x1c24, 0x0001_0002);
    host::w32(h, SAMPLE_SEL, SAMPLE_SEL_A);
    gate0_rows(h, &mut out, "live_A", 5);
    sample_b(h);
    raw_dump(h);

    for (k, name) in STEPS.iter().enumerate() {
        step(h, k);
        host::delay_us(500);
        diag(name, phy::RF_PATH_B, 0, rf0_read(h, phy::RF_PATH_B), "read");
        gate0_rows(h, &mut out, name, 5);
    }
    save("gate0.csv", &out);
    restore_state(h, saved);
}

fn gate1(h: i32) {
    // (prompt, seconds, TX gain index)
    const PHASES: [(&str, u64, u32); 5] = [
        ("keep hands away from the screen edge", 8, TX_LOW),
        ("hold a hand flat in front of the top screen edge, about 10 cm", 8, TX_LOW),
        ("hands away again", 8, TX_LOW),
        ("walk slowly past the laptop at 1 to 3 m, back and forth", 15, TX_LOW),
        ("step away and stand still, out of the room if you can", 10, TX_LOW),
    ];
    host::say("[wifiscope] gate 1: carrier on A, measured on B\n");
    let mut out = Vec::new();
    head(&mut out, "gate1");
    put(&mut out, "# port A, monostatic\nphase,tx_gain,tx_on,t_ms,rf0_a,rf0_b,tries,");
    put(&mut out, STAT_HEAD);
    let t_start = host::now_ms();
    for (p, &(prompt, secs, txg)) in PHASES.iter().enumerate() {
        host::say("[wifiscope] now: ");
        host::say(prompt);
        host::say("\n");
        host::sleep_ms(2000);
        let end = host::now_ms() + secs * 1000;
        while host::now_ms() < end {
            let (off, on_s, tries) = pair(h, txg);
            for (on, s) in [(false, off), (true, on_s)] {
                put_i(&mut out, p as i64);
                out.push(b',');
                put_i(&mut out, txg as i64);
                out.push(b',');
                put_i(&mut out, on as i64);
                out.push(b',');
                put_i(&mut out, (host::now_ms() - t_start) as i64);
                out.push(b',');
                put_i(&mut out, rf0_read(h, phy::RF_PATH_A) as i64);
                out.push(b',');
                put_i(&mut out, rf0_read(h, phy::RF_PATH_B) as i64);
                out.push(b',');
                put_i(&mut out, tries as i64);
                out.push(b',');
                put_stat(&mut out, &s);
            }
            carrier(h, None);
            host::sleep_ms(20);
        }
    }
    carrier(h, None);
    host::say("[wifiscope] gate 1 done, carrier off\n");
    save("gate1.csv", &out);
}

fn gate2(h: i32) {
    host::say("[wifiscope] gate 2: channel switches, hands away\n");
    let mut out = Vec::new();
    head(&mut out, "gate2");
    put(&mut out, "# port A, monostatic\nround,channel,tx_on,rf0_a,rf0_b,tries,");
    put(&mut out, STAT_HEAD);
    for round in 0..SWEEP_ROUNDS {
        for &ch in CH_SWEEP.iter() {
            tune(h, ch);
            let (off, on_s, tries) = pair(h, TX_LOW);
            for (on, s) in [(false, off), (true, on_s)] {
                put_i(&mut out, round as i64);
                out.push(b',');
                put_i(&mut out, ch as i64);
                out.push(b',');
                put_i(&mut out, on as i64);
                out.push(b',');
                put_i(&mut out, rf0_read(h, phy::RF_PATH_A) as i64);
                out.push(b',');
                put_i(&mut out, rf0_read(h, phy::RF_PATH_B) as i64);
                out.push(b',');
                put_i(&mut out, tries as i64);
                out.push(b',');
                put_stat(&mut out, &s);
            }
            carrier(h, None);
        }
    }
    save("gate2.csv", &out);
}

/// Runs the three measurements on the chip stage 4c left on channel 1.
pub fn run(h: i32) {
    host::say("[wifiscope] probe: channel 149, lowest TX gain, about a minute\n");
    chip::set_channel(h, CH_START, 0, RTW_SC_DONT_CARE);
    host::sleep_ms(50);
    let saved = save_state(h);
    A_IDLE.store(saved.rf[0][0], core::sync::atomic::Ordering::Relaxed);
    B_IDLE.store(saved.rf[0][1], core::sync::atomic::Ordering::Relaxed);

    gate0(h, &saved);
    setup_minimal(h);
    carrier(h, None);
    let hold = matrix(h);
    host::w32(h, SAMPLE_SEL, SAMPLE_SEL_A);
    B_HOLD.store(hold, core::sync::atomic::Ordering::Relaxed);
    host::say(if hold { "[wifiscope] B is held in receive for gates 1 and 2\n" }
              else { "[wifiscope] B stays under the BB for gates 1 and 2\n" });
    // The carrier's level drifts for the first seconds of switching.
    let end = host::now_ms() + 5000;
    while host::now_ms() < end {
        let _ = pair(h, TX_LOW);
    }
    carrier(h, None);
    gate1(h);
    gate2(h);

    restore_state(h, &saved);
    DIAG.with(|(d, _)| {
        let mut out = Vec::new();
        head(&mut out, "diag: RF 0x00 writes and read-backs");
        put(&mut out, "t_ms,what,path,want,got,method\n");
        out.extend_from_slice(d);
        save("diag.csv", &out);
    });
    host::say("[wifiscope] probe done, files in wifiscope/\n");
}
