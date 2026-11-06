//! SD memory cards on the SDHCI host, after Linux 6.18.60 drivers/mmc/core
//! (core.c, sd.c, sd_ops.c, mmc_ops.c) and the read/write path of block.c.
//! A card is one removable disk in `storage::disk`, ids from `SD_ID_BASE`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use crate::drivers::sdhci::{self as h, Command, Data, Error, Host, Ios, Stop};
use crate::drivers::usb_storage::{DiskInfo, Error as DiskError};
use crate::kprintln;

/// Disk ids of SD cards start here; `storage::disk` routes by range.
pub const SD_ID_BASE: u32 = 0x4000_0000;

// include/linux/mmc/sd.h, mmc.h, card.h
const MMC_GO_IDLE_STATE: u8 = 0;
const MMC_ALL_SEND_CID: u8 = 2;
const SD_SEND_RELATIVE_ADDR: u8 = 3;
const SD_SWITCH: u8 = 6;
const MMC_SELECT_CARD: u8 = 7;
const SD_SEND_IF_COND: u8 = 8;
const MMC_SEND_CSD: u8 = 9;
const SD_SWITCH_VOLTAGE: u8 = 11;
const MMC_SEND_STATUS: u8 = 13;
const MMC_READ_SINGLE_BLOCK: u8 = 17;
const MMC_READ_MULTIPLE_BLOCK: u8 = 18;
const MMC_WRITE_BLOCK: u8 = 24;
const MMC_WRITE_MULTIPLE_BLOCK: u8 = 25;
const MMC_APP_CMD: u8 = 55;
const SD_APP_SET_BUS_WIDTH: u8 = 6;
const SD_APP_OP_COND: u8 = 41;
const SD_APP_SEND_SCR: u8 = 51;

const SD_OCR_S18R: u32 = 1 << 24;
const SD_ROCR_S18A: u32 = SD_OCR_S18R;
const SD_OCR_2T: u32 = 1 << 27;
const SD_OCR_XPC: u32 = 1 << 28;
const SD_OCR_CCS: u32 = 1 << 30;
const MMC_CARD_BUSY: u32 = 0x8000_0000;

const R1_OUT_OF_RANGE: u32 = 1 << 31;
const R1_ADDRESS_ERROR: u32 = 1 << 30;
const R1_BLOCK_LEN_ERROR: u32 = 1 << 29;
const R1_WP_VIOLATION: u32 = 1 << 26;
const R1_CARD_ECC_FAILED: u32 = 1 << 21;
const R1_CC_ERROR: u32 = 1 << 20;
const R1_ERROR: u32 = 1 << 19;
const R1_READY_FOR_DATA: u32 = 1 << 8;
const R1_APP_CMD: u32 = 1 << 5;
const R1_STATE_TRAN: u32 = 4;
/// block.c CMD_ERRORS
const CMD_ERRORS: u32 = R1_OUT_OF_RANGE | R1_ADDRESS_ERROR | R1_BLOCK_LEN_ERROR
    | R1_WP_VIOLATION | R1_CARD_ECC_FAILED | R1_CC_ERROR | R1_ERROR;

const SD_BUS_WIDTH_4: u32 = 2;
const SCR_SPEC_VER_1: u32 = 1;
const SCR_SPEC_VER_2: u32 = 2;
const SD_SCR_BUS_WIDTH_1: u32 = 1 << 0;
const SD_SCR_BUS_WIDTH_4: u32 = 1 << 2;
const SD_SCR_CMD23_SUPPORT: u32 = 1 << 1;
const CCC_SWITCH: u32 = 1 << 10;

const HIGH_SPEED_MAX_DTR: u32 = 50_000_000;
const UHS_SDR104_MAX_DTR: u32 = 208_000_000;
const UHS_SDR50_MAX_DTR: u32 = 100_000_000;
const UHS_DDR50_MAX_DTR: u32 = 50_000_000;
const UHS_SDR25_MAX_DTR: u32 = UHS_DDR50_MAX_DTR;
const UHS_SDR12_MAX_DTR: u32 = 25_000_000;

const UHS_SDR12_BUS_SPEED: u32 = 0;
const HIGH_SPEED_BUS_SPEED: u32 = 1;
const UHS_SDR25_BUS_SPEED: u32 = 1;
const UHS_SDR50_BUS_SPEED: u32 = 2;
const UHS_SDR104_BUS_SPEED: u32 = 3;
const UHS_DDR50_BUS_SPEED: u32 = 4;
const SD_MODE_HIGH_SPEED: u32 = 1 << HIGH_SPEED_BUS_SPEED;
const SD_MODE_UHS_SDR12: u32 = 1 << UHS_SDR12_BUS_SPEED;
const SD_MODE_UHS_SDR25: u32 = 1 << UHS_SDR25_BUS_SPEED;
const SD_MODE_UHS_SDR50: u32 = 1 << UHS_SDR50_BUS_SPEED;
const SD_MODE_UHS_SDR104: u32 = 1 << UHS_SDR104_BUS_SPEED;
const SD_MODE_UHS_DDR50: u32 = 1 << UHS_DDR50_BUS_SPEED;

const SD_SET_CURRENT_LIMIT_200: u32 = 0;
const SD_SET_CURRENT_LIMIT_400: u32 = 1;
const SD_SET_CURRENT_LIMIT_600: u32 = 2;
const SD_SET_CURRENT_LIMIT_800: u32 = 3;
const SD_MAX_CURRENT_200: u32 = 1 << SD_SET_CURRENT_LIMIT_200;
const SD_MAX_CURRENT_400: u32 = 1 << SD_SET_CURRENT_LIMIT_400;
const SD_MAX_CURRENT_600: u32 = 1 << SD_SET_CURRENT_LIMIT_600;
const SD_MAX_CURRENT_800: u32 = 1 << SD_SET_CURRENT_LIMIT_800;

const MMC_CMD_RETRIES: u32 = 3;
/// mmc_rescan_try_freq: the first of `freqs[]`.
const F_INIT: u32 = 400_000;
/// `ios.power_delay_ms`
const POWER_DELAY_MS: u64 = 10;
/// block.c MMC_BLK_TIMEOUT_MS
const MMC_BLK_TIMEOUT_MS: u64 = 10 * 60 * 1000;

const TRAN_EXP: [u32; 8] = [10000, 100000, 1000000, 10000000, 0, 0, 0, 0];
const TRAN_MANT: [u32; 16] = [0, 10, 12, 13, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 70, 80];
const TAAC_EXP: [u32; 8] = [1, 10, 100, 1000, 10000, 100000, 1000000, 10000000];
const TAAC_MANT: [u32; 16] = [0, 10, 12, 13, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 70, 80];

/// `UNSTUFF_BITS` over a 128-bit response, resp[0] the most significant.
fn unstuff_bits(resp: &[u32; 4], start: u32, size: u32) -> u32 {
    let mask = if size < 32 { (1u32 << size) - 1 } else { u32::MAX };
    let off = 3 - (start / 32) as usize;
    let shft = start & 31;
    let mut res = resp[off] >> shft;
    if size + shft > 32 && off > 0 {
        res |= resp[off - 1] << ((32 - shft) % 32);
    }
    res & mask
}

#[derive(Default)]
struct SwCaps {
    hs_max_dtr: u32,
    uhs_max_dtr: u32,
    sd3_bus_mode: u32,
    sd3_drv_type: u32,
    sd3_curr_limit: u32,
}

#[derive(Default)]
struct Card {
    rca: u32,
    raw_cid: [u32; 4],
    raw_csd: [u32; 4],
    blockaddr: bool,
    readonly: bool,
    /// 512-byte sectors.
    capacity: u64,
    cmdclass: u32,
    max_dtr: u32,
    taac_ns: u32,
    taac_clks: u32,
    r2w_factor: u32,
    sda_vsn: u32,
    sda_spec3: u32,
    sda_spec4: u32,
    bus_widths: u32,
    cmds: u32,
    sw_caps: SwCaps,
    sd_bus_speed: u32,
    hs: bool,
    manfid: u32,
    oemid: u32,
    prod_name: [u8; 5],
}

/// The bound host and, while one is in, its card.
struct Slot {
    host: Host,
    card: Option<Card>,
}

/// None while a caller has it; `CONTROLLER` says whether there is one.
static SLOT: Mutex<Option<Slot>> = Mutex::new(None);
static CONTROLLER: AtomicBool = AtomicBool::new(false);
static DISK: Mutex<Option<DiskInfo>> = Mutex::new(None);
static NEXT_ID: AtomicU32 = AtomicU32::new(SD_ID_BASE);
static DETECTING: AtomicBool = AtomicBool::new(false);
/// The card in the slot was ejected: it stays unused until pulled out.
static EJECTED: AtomicBool = AtomicBool::new(false);
static LAST_POLL: AtomicU64 = AtomicU64::new(0);

/// Take the slot for one operation; waits while another holds it.
fn claim() -> Option<Slot> {
    loop {
        if !CONTROLLER.load(Ordering::Acquire) { return None; }
        if let Some(s) = SLOT.lock().take() { return Some(s); }
        h::mdelay(1);
    }
}

fn release(s: Slot) {
    *SLOT.lock() = Some(s);
}

/// Find the controller and look for a card. Boot, after the NVMe.
pub fn init() -> bool {
    let Some(host) = h::probe() else { return false };
    *SLOT.lock() = Some(Slot { host, card: None });
    CONTROLLER.store(true, Ordering::Release);
    detect_spawn();
    true
}

/// From the shell's idle step: a card inserted or pulled is noticed here,
/// once a second, and handled on a worker.
pub fn poll() {
    if !CONTROLLER.load(Ordering::Acquire) { return; }
    let now = crate::interrupts::rdtsc();
    let last = LAST_POLL.load(Ordering::Relaxed);
    if now.saturating_sub(last) < crate::interrupts::tsc_freq() { return; }
    LAST_POLL.store(now, Ordering::Relaxed);
    let changed = {
        let mut g = SLOT.lock();
        let Some(s) = g.as_mut() else { return };
        let present = s.host.readl(h::SDHCI_PRESENT_STATE) & h::SDHCI_CARD_PRESENT != 0;
        let event = s.host.take_card_event();
        if !present { EJECTED.store(false, Ordering::Release); }
        let ejected = EJECTED.load(Ordering::Acquire);
        event || present != (s.card.is_some() || ejected)
    };
    if changed { detect_spawn(); }
}

fn detect_spawn() {
    if DETECTING.swap(true, Ordering::AcqRel) { return; }
    crate::smp::scheduler::spawn("sd card", detect_task, 0);
}

/// `mmc_rescan`: a known card still answering, or a new one initialised.
fn detect_task(_: u64) {
    let Some(mut s) = claim() else { DETECTING.store(false, Ordering::Release); return };
    if let Some(card) = s.card.as_ref() {
        // mmc_sd_detect: alive (CMD13), else removed.
        let rca = card.rca;
        let alive = s.host.get_cd() && {
            let mut cmd = Command::new(MMC_SEND_STATUS, rca << 16, h::MMC_RSP_R1);
            wait_for_cmd(&mut s.host, &mut cmd, MMC_CMD_RETRIES).is_ok()
        };
        if !alive {
            s.card = None;
            power_off(&mut s.host);
            drop_disk("removed");
        }
    }
    if s.card.is_none() && !EJECTED.load(Ordering::Acquire) && s.host.get_cd() {
        match rescan_try_freq(&mut s.host) {
            Ok(card) => {
                let info = disk_info(&card, &s.host);
                kprintln!("[npk] sd: card {} \"{}\" in {}, {} MB, {} at {} MHz{}", info.id, info.product,
                    s.host.name, card.capacity * 512 / 1_000_000, timing_name(s.host.ios.timing, card.hs),
                    s.host.actual_clock / 1_000_000,
                    if card.readonly { ", write-protected" } else { "" });
                let id = info.id;
                *DISK.lock() = Some(info);
                s.card = Some(card);
                release(s);
                DETECTING.store(false, Ordering::Release);
                crate::ext::disk_added(id);
                return;
            }
            Err(e) => kprintln!("[npk] sd: error {:?} whilst initialising SD card", e),
        }
    }
    release(s);
    DETECTING.store(false, Ordering::Release);
}

fn drop_disk(why: &str) {
    let gone = DISK.lock().take();
    if let Some(d) = gone {
        kprintln!("[npk] sd: card {} {}", d.id, why);
        crate::ext::disk_gone(d.id);
    }
}

fn timing_name(t: u8, hs: bool) -> String {
    let name = match t {
        h::MMC_TIMING_UHS_SDR104 => "UHS-I SDR104",
        h::MMC_TIMING_UHS_SDR50 => "UHS-I SDR50",
        h::MMC_TIMING_UHS_DDR50 => "UHS-I DDR50",
        h::MMC_TIMING_UHS_SDR25 => "UHS-I SDR25",
        h::MMC_TIMING_UHS_SDR12 => "UHS-I SDR12",
        _ if hs => "high speed",
        _ => "default speed",
    };
    String::from(name)
}

fn disk_info(card: &Card, host: &Host) -> DiskInfo {
    let name: String = card.prod_name.iter()
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|&b| b as char).collect();
    let name = String::from(name.trim());
    DiskInfo {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        port: String::from("sd"),
        name: alloc::format!("SD card {}", name),
        vendor: alloc::format!("{:02x}/{:04x}", card.manfid, card.oemid),
        product: if name.is_empty() { String::from("SD card") } else { name },
        block_size: 512,
        blocks: card.capacity,
        removable: true,
        read_only: card.readonly || host.get_ro(),
    }
}

// ── core.c: ios, power ─────────────────────────────────────────────

fn set_ios(host: &mut Host) {
    let ios: Ios = host.ios;
    host.set_ios(&ios);
}

fn set_clock(host: &mut Host, mut hz: u32) {
    if hz > host.f_max { hz = host.f_max; }
    host.ios.clock = hz;
    set_ios(host);
}

fn set_bus_width(host: &mut Host, width: u8) {
    host.ios.bus_width = width;
    set_ios(host);
}

fn set_timing(host: &mut Host, timing: u8) {
    host.ios.timing = timing;
    set_ios(host);
}

fn set_initial_state(host: &mut Host) {
    host.ios.bus_width = h::MMC_BUS_WIDTH_1;
    host.ios.timing = h::MMC_TIMING_LEGACY;
    host.ios.drv_type = 0;
    set_ios(host);
}

fn set_signal_voltage(host: &mut Host, v: u8) -> Result<(), Error> {
    let old = host.ios.signal_voltage;
    host.ios.signal_voltage = v;
    let r = host.start_signal_voltage_switch(v);
    if r.is_err() { host.ios.signal_voltage = old; }
    r
}

fn fls(x: u32) -> u32 { 32 - x.leading_zeros() }

fn power_up(host: &mut Host, ocr: u32) {
    if host.ios.power_mode == h::MMC_POWER_ON { return; }
    host.ios.vdd = (fls(ocr) - 1) as u8;
    host.ios.power_mode = h::MMC_POWER_UP;
    set_initial_state(host);
    // mmc_set_initial_signal_voltage
    if set_signal_voltage(host, h::MMC_SIGNAL_VOLTAGE_330).is_err() {
        let _ = set_signal_voltage(host, h::MMC_SIGNAL_VOLTAGE_180);
    }
    h::mdelay(POWER_DELAY_MS);
    host.ios.clock = F_INIT;
    host.ios.power_mode = h::MMC_POWER_ON;
    set_ios(host);
    h::mdelay(POWER_DELAY_MS);
}

fn power_off(host: &mut Host) {
    if host.ios.power_mode == h::MMC_POWER_OFF { return; }
    host.ios.clock = 0;
    host.ios.vdd = 0;
    host.ios.power_mode = h::MMC_POWER_OFF;
    set_initial_state(host);
    h::mdelay(1);
}

fn power_cycle(host: &mut Host, ocr: u32) {
    power_off(host);
    h::mdelay(1);
    power_up(host, ocr);
}

fn host_can_uhs(host: &Host) -> bool {
    host.mmc_caps & h::MMC_CAP_UHS != 0
}

fn wait_for_cmd(host: &mut Host, cmd: &mut Command, retries: u32) -> Result<(), Error> {
    let mut r = host.request(cmd, None, Stop::None);
    let mut left = retries;
    while r.is_err() && left > 0 && r != Err(Error::NoMedium) {
        left -= 1;
        cmd.resp = [0; 4];
        r = host.request(cmd, None, Stop::None);
    }
    r
}

/// `mmc_host_set_uhs_voltage`
fn host_set_uhs_voltage(host: &mut Host) -> Result<(), Error> {
    let clock = host.ios.clock;
    host.ios.clock = 0;
    set_ios(host);
    if set_signal_voltage(host, h::MMC_SIGNAL_VOLTAGE_180).is_err() {
        return Err(Error::Again);
    }
    h::mdelay(10);
    host.ios.clock = clock;
    set_ios(host);
    Ok(())
}

/// `mmc_set_uhs_voltage`: CMD11 and the switch to 1.8 V signalling.
fn set_uhs_voltage(host: &mut Host, ocr: u32) -> Result<(), Error> {
    let mut cmd = Command::new(SD_SWITCH_VOLTAGE, 0, h::MMC_RSP_R1);
    let mut err = wait_for_cmd(host, &mut cmd, 0);
    if err.is_ok() {
        if cmd.resp[0] & R1_ERROR != 0 {
            return Err(Error::Io);
        }
        h::mdelay(1);
        if !host.card_busy() {
            err = Err(Error::Again);
        } else if host_set_uhs_voltage(host).is_err() {
            err = Err(Error::Again);
        } else {
            h::mdelay(1);
            if host.card_busy() {
                err = Err(Error::Again);
            }
        }
    }
    if err.is_err() {
        power_cycle(host, ocr);
    }
    err
}

// ── mmc_ops.c / sd_ops.c ───────────────────────────────────────────

fn go_idle(host: &mut Host) -> Result<(), Error> {
    h::mdelay(1);
    let mut cmd = Command::new(MMC_GO_IDLE_STATE, 0, h::MMC_RSP_NONE);
    let r = wait_for_cmd(host, &mut cmd, 0);
    h::mdelay(1);
    h::mdelay(1);
    r
}

fn send_if_cond(host: &mut Host, ocr: u32) -> Result<(), Error> {
    let arg = (((ocr & 0xFF8000) != 0) as u32) << 8 | 0xAA;
    let mut cmd = Command::new(SD_SEND_IF_COND, arg, h::MMC_RSP_R7);
    wait_for_cmd(host, &mut cmd, 0)?;
    if cmd.resp[0] & 0xFF != 0xAA {
        return Err(Error::Io);
    }
    Ok(())
}

fn app_cmd(host: &mut Host, rca: Option<u32>) -> Result<(), Error> {
    let mut cmd = Command::new(MMC_APP_CMD, rca.map_or(0, |r| r << 16), h::MMC_RSP_R1);
    wait_for_cmd(host, &mut cmd, 0)?;
    if cmd.resp[0] & R1_APP_CMD == 0 {
        return Err(Error::Invalid);
    }
    Ok(())
}

fn wait_for_app_cmd(host: &mut Host, rca: Option<u32>, cmd: &mut Command) -> Result<(), Error> {
    let mut err = Err(Error::Io);
    for _ in 0..=MMC_CMD_RETRIES {
        if let Err(e) = app_cmd(host, rca) {
            if e == Error::NoMedium { return Err(e); }
            err = Err(e);
            continue;
        }
        cmd.resp = [0; 4];
        err = host.request(cmd, None, Stop::None);
        if err.is_ok() { break; }
    }
    err
}

/// `mmc_send_app_op_cond`: ACMD41 until the card leaves busy, at most 2 s.
fn send_app_op_cond(host: &mut Host, ocr: u32) -> Result<u32, Error> {
    let deadline = crate::interrupts::rdtsc() + crate::interrupts::tsc_freq() * 2;
    let mut cmd = Command::new(SD_APP_OP_COND, ocr, h::MMC_RSP_R3);
    loop {
        wait_for_app_cmd(host, None, &mut cmd)?;
        if ocr == 0 || cmd.resp[0] & MMC_CARD_BUSY != 0 {
            return Ok(cmd.resp[0]);
        }
        if crate::interrupts::rdtsc() > deadline {
            return Err(Error::Timeout);
        }
        h::mdelay(10);
    }
}

fn send_cid(host: &mut Host) -> Result<[u32; 4], Error> {
    let mut cmd = Command::new(MMC_ALL_SEND_CID, 0, h::MMC_RSP_R2);
    wait_for_cmd(host, &mut cmd, MMC_CMD_RETRIES)?;
    Ok(cmd.resp)
}

fn send_relative_addr(host: &mut Host) -> Result<u32, Error> {
    let mut cmd = Command::new(SD_SEND_RELATIVE_ADDR, 0, h::MMC_RSP_R6);
    wait_for_cmd(host, &mut cmd, MMC_CMD_RETRIES)?;
    Ok(cmd.resp[0] >> 16)
}

fn send_csd(host: &mut Host, rca: u32) -> Result<[u32; 4], Error> {
    let mut cmd = Command::new(MMC_SEND_CSD, rca << 16, h::MMC_RSP_R2);
    wait_for_cmd(host, &mut cmd, MMC_CMD_RETRIES)?;
    Ok(cmd.resp)
}

fn select_card(host: &mut Host, rca: u32) -> Result<(), Error> {
    let mut cmd = Command::new(MMC_SELECT_CARD, rca << 16, h::MMC_RSP_R1);
    wait_for_cmd(host, &mut cmd, MMC_CMD_RETRIES)
}

fn send_status(host: &mut Host, card: &Card) -> Result<u32, Error> {
    let mut cmd = Command::new(MMC_SEND_STATUS, card.rca << 16, h::MMC_RSP_R1);
    wait_for_cmd(host, &mut cmd, MMC_CMD_RETRIES)?;
    Ok(cmd.resp[0])
}

fn app_set_bus_width(host: &mut Host, card: &Card) -> Result<(), Error> {
    let mut cmd = Command::new(SD_APP_SET_BUS_WIDTH, SD_BUS_WIDTH_4, h::MMC_RSP_R1);
    wait_for_app_cmd(host, Some(card.rca), &mut cmd)
}

/// `mmc_set_data_timeout` for an SD card.
fn data_timeout(card: &Card, host: &Host, write: bool) -> (u32, u32) {
    let mut mult: u32 = 100;
    if write { mult <<= card.r2w_factor.min(7); }
    let mut ns = card.taac_ns.saturating_mul(mult);
    let mut clks = card.taac_clks.saturating_mul(mult);
    let mut us = ns / 1000;
    if host.ios.clock != 0 {
        us = us.saturating_add(clks.saturating_mul(1000) / (host.ios.clock / 1000).max(1));
    }
    let limit_us = if write { 3_000_000 } else { 100_000 };
    if us > limit_us {
        ns = limit_us * 1000;
        clks = 0;
    }
    if us == 0 {
        ns = limit_us * 1000;
    }
    (ns, clks)
}

/// A one-block read command into the start of the host buffer
/// (`mmc_send_adtc_data`, `mmc_app_send_scr`).
fn read_small(host: &mut Host, card: &Card, opcode: u8, arg: u32, len: u32, app: bool, out: &mut [u8]) -> Result<(), Error> {
    if app { app_cmd(host, Some(card.rca))?; }
    let (ns, clks) = data_timeout(card, host, false);
    let data = Data { blksz: len, blocks: 1, write: false, timeout_ns: ns, timeout_clks: clks };
    let mut cmd = Command::new(opcode, arg, h::MMC_RSP_R1);
    host.request(&mut cmd, Some(&data), Stop::None)?;
    let n = out.len().min(len as usize);
    host.buf.copy_out(0u64, &mut out[..n]);
    Ok(())
}

/// `mmc_sd_switch`: CMD6 with its 64-byte status.
fn sd_switch(host: &mut Host, card: &Card, mode: bool, group: u32, value: u32, status: &mut [u8; 64]) -> Result<(), Error> {
    let value = value & 0xF;
    let mut arg = (mode as u32) << 31 | 0x00FF_FFFF;
    arg &= !(0xF << (group * 4));
    arg |= value << (group * 4);
    read_small(host, card, SD_SWITCH, arg, 64, false, status)
}

// ── sd.c ───────────────────────────────────────────────────────────

fn decode_cid(card: &mut Card) {
    let r = &card.raw_cid;
    card.manfid = unstuff_bits(r, 120, 8);
    card.oemid = unstuff_bits(r, 104, 16);
    card.prod_name = [
        unstuff_bits(r, 96, 8) as u8, unstuff_bits(r, 88, 8) as u8, unstuff_bits(r, 80, 8) as u8,
        unstuff_bits(r, 72, 8) as u8, unstuff_bits(r, 64, 8) as u8,
    ];
}

fn decode_csd(card: &mut Card) -> Result<(), Error> {
    let r = card.raw_csd;
    let csd_struct = unstuff_bits(&r, 126, 2);
    match csd_struct {
        0 => {
            let m = unstuff_bits(&r, 115, 4) as usize;
            let e = unstuff_bits(&r, 112, 3) as usize;
            card.taac_ns = (TAAC_EXP[e] * TAAC_MANT[m] + 9) / 10;
            card.taac_clks = unstuff_bits(&r, 104, 8) * 100;
            let m = unstuff_bits(&r, 99, 4) as usize;
            let e = unstuff_bits(&r, 96, 3) as usize;
            card.max_dtr = TRAN_EXP[e] * TRAN_MANT[m];
            card.cmdclass = unstuff_bits(&r, 84, 12);
            let e = unstuff_bits(&r, 47, 3);
            let m = unstuff_bits(&r, 62, 12) as u64;
            let read_blkbits = unstuff_bits(&r, 80, 4);
            // capacity in read_blkbits units, as 512-byte sectors.
            let blocks = (1 + m) << (e + 2);
            card.capacity = if read_blkbits >= 9 { blocks << (read_blkbits - 9) } else { blocks };
            card.r2w_factor = unstuff_bits(&r, 26, 3);
            if unstuff_bits(&r, 13, 1) != 0 { card.readonly = true; }
        }
        1 | 2 => {
            card.blockaddr = true;
            card.taac_ns = 0;
            card.taac_clks = 0;
            let m = unstuff_bits(&r, 99, 4) as usize;
            let e = unstuff_bits(&r, 96, 3) as usize;
            card.max_dtr = TRAN_EXP[e] * TRAN_MANT[m];
            card.cmdclass = unstuff_bits(&r, 84, 12);
            let m = if csd_struct == 1 { unstuff_bits(&r, 48, 22) } else { unstuff_bits(&r, 48, 28) };
            card.capacity = (1 + m as u64) << 10;
            // Above 2 TiB (SDUC) block addresses need CMD22 first.
            if card.capacity > 1 << 32 {
                kprintln!("[npk] sd: SDUC cards are not driven");
                return Err(Error::Invalid);
            }
            card.r2w_factor = 4;
            if unstuff_bits(&r, 13, 1) != 0 { card.readonly = true; }
        }
        _ => {
            kprintln!("[npk] sd: unrecognised CSD structure version {}", csd_struct);
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

fn decode_scr(card: &mut Card, raw: [u32; 2]) -> Result<(), Error> {
    let resp = [0, 0, raw[0], raw[1]];
    let scr_struct = unstuff_bits(&resp, 60, 4);
    if scr_struct != 0 {
        kprintln!("[npk] sd: unrecognised SCR structure version {}", scr_struct);
        return Err(Error::Invalid);
    }
    card.sda_vsn = unstuff_bits(&resp, 56, 4);
    card.bus_widths = unstuff_bits(&resp, 48, 4);
    if card.sda_vsn == SCR_SPEC_VER_2 {
        card.sda_spec3 = unstuff_bits(&resp, 47, 1);
    }
    if card.sda_spec3 != 0 {
        card.sda_spec4 = unstuff_bits(&resp, 42, 1);
    }
    if card.sda_spec4 != 0 {
        card.cmds = unstuff_bits(&resp, 32, 4);
    } else if card.sda_spec3 != 0 {
        card.cmds = unstuff_bits(&resp, 32, 2);
    }
    if card.bus_widths & SD_SCR_BUS_WIDTH_1 == 0 || card.bus_widths & SD_SCR_BUS_WIDTH_4 == 0 {
        kprintln!("[npk] sd: invalid bus width");
        return Err(Error::Invalid);
    }
    Ok(())
}

fn app_send_scr(host: &mut Host, card: &mut Card) -> Result<[u32; 2], Error> {
    let mut b = [0u8; 8];
    read_small(host, card, SD_APP_SEND_SCR, 0, 8, true, &mut b)?;
    Ok([u32::from_be_bytes([b[0], b[1], b[2], b[3]]), u32::from_be_bytes([b[4], b[5], b[6], b[7]])])
}

fn read_switch(host: &mut Host, card: &mut Card) -> Result<(), Error> {
    if card.sda_vsn < SCR_SPEC_VER_1 { return Ok(()); }
    if card.cmdclass & CCC_SWITCH == 0 {
        kprintln!("[npk] sd: card lacks mandatory switch function, performance might suffer");
        return Ok(());
    }
    let mut status = [0u8; 64];
    match sd_switch(host, card, false, 0, 0, &mut status) {
        Ok(()) => {}
        Err(Error::Invalid) => {
            kprintln!("[npk] sd: problem reading Bus Speed modes");
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    if status[13] as u32 & SD_MODE_HIGH_SPEED != 0 {
        card.sw_caps.hs_max_dtr = HIGH_SPEED_MAX_DTR;
    }
    if card.sda_spec3 != 0 {
        card.sw_caps.sd3_bus_mode = status[13] as u32;
        card.sw_caps.sd3_drv_type = status[9] as u32;
        card.sw_caps.sd3_curr_limit = status[7] as u32 | (status[6] as u32) << 8;
    }
    Ok(())
}

/// `mmc_sd_switch_hs`: 1 when switched, 0 when not.
fn sd_switch_hs(host: &mut Host, card: &mut Card) -> Result<bool, Error> {
    if card.sda_vsn < SCR_SPEC_VER_1 || card.cmdclass & CCC_SWITCH == 0
        || host.mmc_caps & h::MMC_CAP_SD_HIGHSPEED == 0 || card.sw_caps.hs_max_dtr == 0
    {
        return Ok(false);
    }
    let mut status = [0u8; 64];
    sd_switch(host, card, true, 0, HIGH_SPEED_BUS_SPEED, &mut status)?;
    if status[16] as u32 & 0xF != HIGH_SPEED_BUS_SPEED {
        kprintln!("[npk] sd: problem switching card into high-speed mode");
        return Ok(false);
    }
    Ok(true)
}

fn sd_update_bus_speed_mode(host: &Host, card: &mut Card) {
    let caps = host.mmc_caps;
    let mode = card.sw_caps.sd3_bus_mode;
    card.sd_bus_speed = if !host_can_uhs(host) {
        0
    } else if caps & h::MMC_CAP_UHS_SDR104 != 0 && mode & SD_MODE_UHS_SDR104 != 0 {
        UHS_SDR104_BUS_SPEED
    } else if caps & h::MMC_CAP_UHS_DDR50 != 0 && mode & SD_MODE_UHS_DDR50 != 0 {
        UHS_DDR50_BUS_SPEED
    } else if caps & (h::MMC_CAP_UHS_SDR104 | h::MMC_CAP_UHS_SDR50) != 0 && mode & SD_MODE_UHS_SDR50 != 0 {
        UHS_SDR50_BUS_SPEED
    } else if caps & (h::MMC_CAP_UHS_SDR104 | h::MMC_CAP_UHS_SDR50 | h::MMC_CAP_UHS_SDR25) != 0
        && mode & SD_MODE_UHS_SDR25 != 0
    {
        UHS_SDR25_BUS_SPEED
    } else if mode & SD_MODE_UHS_SDR12 != 0 {
        UHS_SDR12_BUS_SPEED
    } else {
        card.sd_bus_speed
    };
}

fn sd_set_bus_speed_mode(host: &mut Host, card: &mut Card, status: &mut [u8; 64]) -> Result<(), Error> {
    let (timing, dtr) = match card.sd_bus_speed {
        UHS_SDR104_BUS_SPEED => (h::MMC_TIMING_UHS_SDR104, UHS_SDR104_MAX_DTR),
        UHS_DDR50_BUS_SPEED => (h::MMC_TIMING_UHS_DDR50, UHS_DDR50_MAX_DTR),
        UHS_SDR50_BUS_SPEED => (h::MMC_TIMING_UHS_SDR50, UHS_SDR50_MAX_DTR),
        UHS_SDR25_BUS_SPEED => (h::MMC_TIMING_UHS_SDR25, UHS_SDR25_MAX_DTR),
        UHS_SDR12_BUS_SPEED => (h::MMC_TIMING_UHS_SDR12, UHS_SDR12_MAX_DTR),
        _ => return Ok(()),
    };
    card.sw_caps.uhs_max_dtr = dtr;
    sd_switch(host, card, true, 0, card.sd_bus_speed, status)?;
    if status[16] as u32 & 0xF != card.sd_bus_speed {
        kprintln!("[npk] sd: problem setting bus speed mode");
    } else {
        set_timing(host, timing);
        set_clock(host, dtr);
    }
    Ok(())
}

fn sd_get_host_max_current(host: &Host) -> u32 {
    match 1u32.checked_shl(host.ios.vdd as u32).unwrap_or(0) {
        h::MMC_VDD_165_195 => host.max_current_180,
        h::MMC_VDD_29_30 | h::MMC_VDD_30_31 => host.max_current_300,
        h::MMC_VDD_32_33 | h::MMC_VDD_33_34 => host.max_current_330,
        _ => 0,
    }
}

fn sd_set_current_limit(host: &mut Host, card: &mut Card, status: &mut [u8; 64]) -> Result<(), Error> {
    if !matches!(card.sd_bus_speed, UHS_SDR50_BUS_SPEED | UHS_SDR104_BUS_SPEED | UHS_DDR50_BUS_SPEED) {
        return Ok(());
    }
    let max_current = sd_get_host_max_current(host);
    let lim = card.sw_caps.sd3_curr_limit;
    let current_limit = if max_current >= 800 && lim & SD_MAX_CURRENT_800 != 0 {
        Some(SD_SET_CURRENT_LIMIT_800)
    } else if max_current >= 600 && lim & SD_MAX_CURRENT_600 != 0 {
        Some(SD_SET_CURRENT_LIMIT_600)
    } else if max_current >= 400 && lim & SD_MAX_CURRENT_400 != 0 {
        Some(SD_SET_CURRENT_LIMIT_400)
    } else if max_current >= 200 && lim & SD_MAX_CURRENT_200 != 0 {
        Some(SD_SET_CURRENT_LIMIT_200)
    } else {
        None
    };
    if let Some(cl) = current_limit {
        sd_switch(host, card, true, 3, cl, status)?;
        if (status[15] as u32 >> 4) & 0x0F != cl {
            kprintln!("[npk] sd: problem setting current limit");
        }
    }
    Ok(())
}

fn sd_use_tuning(host: &Host) -> bool {
    matches!(host.ios.timing, h::MMC_TIMING_UHS_SDR50 | h::MMC_TIMING_UHS_SDR104 | h::MMC_TIMING_UHS_DDR50)
}

/// `mmc_sd_init_uhs_card`
fn sd_init_uhs_card(host: &mut Host, card: &mut Card) -> Result<(), Error> {
    if card.cmdclass & CCC_SWITCH == 0 { return Ok(()); }
    let mut status = [0u8; 64];
    app_set_bus_width(host, card)?;
    set_bus_width(host, h::MMC_BUS_WIDTH_4);
    sd_update_bus_speed_mode(host, card);
    // sd_select_driver_type: the host has no select_drive_strength, so type B.
    sd_set_current_limit(host, card, &mut status)?;
    sd_set_bus_speed_mode(host, card, &mut status)?;
    if sd_use_tuning(host) {
        let r = host.execute_tuning(h::MMC_SEND_TUNING_BLOCK);
        if let Err(e) = r {
            if host.ios.timing == h::MMC_TIMING_UHS_DDR50 {
                kprintln!("[npk] sd: ddr50 tuning failed");
            } else {
                kprintln!("[npk] sd: tuning execution failed: {:?}", e);
                return Err(e);
            }
        }
    }
    Ok(())
}

/// `mmc_sd_get_cid`
fn sd_get_cid(host: &mut Host, mut ocr: u32) -> Result<([u32; 4], u32), Error> {
    let mut retries = 10;
    let pocr = ocr;
    loop {
        if retries == 0 {
            ocr &= !SD_OCR_S18R;
            kprintln!("[npk] sd: skipping voltage switch");
        }
        go_idle(host)?;
        if send_if_cond(host, ocr).is_ok() {
            ocr |= SD_OCR_CCS | SD_OCR_2T;
        }
        if retries != 0 && host_can_uhs(host) {
            ocr |= SD_OCR_S18R;
        }
        if sd_get_host_max_current(host) > 150 {
            ocr |= SD_OCR_XPC;
        }
        let rocr = send_app_op_cond(host, ocr)?;
        if ocr & SD_OCR_S18R != 0 && rocr & SD_ROCR_S18A != 0 {
            match set_uhs_voltage(host, pocr) {
                Err(Error::Again) => { retries -= 1; continue; }
                Err(_) => { retries = 0; continue; }
                Ok(()) => {}
            }
        }
        let cid = send_cid(host)?;
        return Ok((cid, rocr));
    }
}

fn sd_card_using_v18(card: &Card) -> bool {
    card.sw_caps.sd3_bus_mode & (SD_MODE_UHS_SDR50 | SD_MODE_UHS_SDR104 | SD_MODE_UHS_DDR50) != 0
}

/// `mmc_sd_init_card`
fn sd_init_card(host: &mut Host, ocr: u32) -> Result<Card, Error> {
    let mut v18_fixup_failed = false;
    'retry: loop {
        let (cid, rocr) = sd_get_cid(host, ocr)?;
        let mut card = Card { raw_cid: cid, ..Card::default() };
        card.rca = send_relative_addr(host)?;
        card.raw_csd = send_csd(host, card.rca)?;
        decode_csd(&mut card)?;
        decode_cid(&mut card);
        select_card(host, card.rca)?;
        // mmc_sd_setup_card
        let raw_scr = app_send_scr(host, &mut card)?;
        decode_scr(&mut card, raw_scr)?;
        read_switch(host, &mut card)?;
        if host.get_ro() { card.readonly = true; }

        if !v18_fixup_failed && host_can_uhs(host) && sd_card_using_v18(&card)
            && host.ios.signal_voltage != h::MMC_SIGNAL_VOLTAGE_180
        {
            if host_set_uhs_voltage(host).is_err() || sd_init_uhs_card(host, &mut card).is_err() {
                v18_fixup_failed = true;
                power_cycle(host, ocr);
                continue 'retry;
            }
            return Ok(card);
        }

        if rocr & SD_ROCR_S18A != 0 && host_can_uhs(host) {
            sd_init_uhs_card(host, &mut card)?;
        } else {
            if sd_switch_hs(host, &mut card)? {
                card.hs = true;
                set_timing(host, h::MMC_TIMING_SD_HS);
            }
            let max_dtr = if card.hs { card.sw_caps.hs_max_dtr } else { card.max_dtr };
            set_clock(host, max_dtr);
            if host.mmc_caps & h::MMC_CAP_4_BIT_DATA != 0 && card.bus_widths & SD_SCR_BUS_WIDTH_4 != 0 {
                app_set_bus_width(host, &card)?;
                set_bus_width(host, h::MMC_BUS_WIDTH_4);
            }
        }
        return Ok(card);
    }
}

/// `mmc_attach_sd`
fn attach_sd(host: &mut Host) -> Result<Card, Error> {
    let ocr = send_app_op_cond(host, 0)?;
    let ocr = ocr & !0x7FFF;
    // mmc_select_voltage
    let mut o = ocr & host.ocr_avail;
    if o == 0 {
        kprintln!("[npk] sd: no support for card's volts");
        return Err(Error::Invalid);
    }
    let bit = fls(o) - 1;
    o &= 3 << (bit - 1);
    sd_init_card(host, o)
}

/// `mmc_rescan_try_freq` at 400 kHz.
fn rescan_try_freq(host: &mut Host) -> Result<Card, Error> {
    let ocr = host.ocr_avail;
    power_up(host, ocr);
    let r = go_idle(host).and_then(|()| {
        // mmc_send_if_cond_pcie without SD Express.
        let _ = send_if_cond(host, ocr);
        attach_sd(host)
    });
    if r.is_err() { power_off(host); }
    r
}

// ── block.c: reads and writes ──────────────────────────────────────

fn current_disk(id: u32) -> Result<DiskInfo, DiskError> {
    DISK.lock().as_ref().filter(|d| d.id == id).cloned().ok_or(DiskError::Gone)
}

fn check_range(len: usize, lba: u64, blocks: u64) -> Result<u32, DiskError> {
    if len == 0 || len % 512 != 0 { return Err(DiskError::Range); }
    let count = (len / 512) as u64;
    match lba.checked_add(count) {
        Some(end) if end <= blocks => Ok(count as u32),
        _ => Err(DiskError::Range),
    }
}

fn map_err(e: Error) -> DiskError {
    if e == Error::NoMedium { DiskError::Gone } else { DiskError::Io }
}

fn stop_for(host_flags_auto12: bool, host_auto23: bool, card: &Card, blocks: u32) -> Stop {
    if blocks <= 1 { return Stop::None; }
    if card.cmds & SD_SCR_CMD23_SUPPORT != 0 {
        if host_auto23 { Stop::AutoCmd23 } else { Stop::ManualCmd23 }
    } else if host_flags_auto12 {
        Stop::AutoCmd12
    } else {
        Stop::ManualCmd12
    }
}

/// `mmc_blk_card_busy`: CMD13 until the card is ready for data in TRAN.
fn card_busy_wait(host: &mut Host, card: &Card) -> Result<(), Error> {
    let deadline = crate::interrupts::rdtsc()
        + crate::interrupts::tsc_freq() / 1000 * MMC_BLK_TIMEOUT_MS;
    loop {
        let st = send_status(host, card)?;
        if st & R1_READY_FOR_DATA != 0 && (st & 0x1E00) >> 9 == R1_STATE_TRAN {
            return Ok(());
        }
        if crate::interrupts::rdtsc() > deadline {
            kprintln!("[npk] sd: card stuck in programming state");
            return Err(Error::Timeout);
        }
        h::mdelay(1);
    }
}

fn transfer(s: &mut Slot, lba: u64, count: u32, write: bool) -> Result<(), Error> {
    let host = &mut s.host;
    let card = s.card.as_ref().ok_or(Error::NoMedium)?;
    let multi = count > 1;
    let opcode = match (write, multi) {
        (false, false) => MMC_READ_SINGLE_BLOCK,
        (false, true) => MMC_READ_MULTIPLE_BLOCK,
        (true, false) => MMC_WRITE_BLOCK,
        (true, true) => MMC_WRITE_MULTIPLE_BLOCK,
    };
    let arg = if card.blockaddr { lba as u32 } else { (lba as u32) << 9 };
    let (ns, clks) = data_timeout(card, host, write);
    let data = Data { blksz: 512, blocks: count, write, timeout_ns: ns, timeout_clks: clks };
    let stop = stop_for(host.has_auto_cmd12(), host.has_auto_cmd23(), card, count);
    let mut cmd = Command::new(opcode, arg, h::MMC_RSP_R1);
    host.request(&mut cmd, Some(&data), stop)?;
    if cmd.resp[0] & CMD_ERRORS != 0 {
        kprintln!("[npk] sd: CMD{} status {:#010x}", opcode, cmd.resp[0]);
        return Err(Error::Io);
    }
    if write {
        card_busy_wait(host, card)?;
    }
    Ok(())
}

/// One chunk with block.c's recovery: a CRC error in a tuned mode
/// re-tunes, then the request is retried.
fn transfer_retry(s: &mut Slot, lba: u64, count: u32, write: bool) -> Result<(), Error> {
    let mut tries = 0;
    loop {
        match transfer(s, lba, count, write) {
            Ok(()) => return Ok(()),
            Err(Error::NoMedium) => return Err(Error::NoMedium),
            Err(e) if tries < 2 => {
                tries += 1;
                kprintln!("[npk] sd: {} of {} blocks at {} failed ({:?}), retrying",
                    if write { "write" } else { "read" }, count, lba, e);
                if e == Error::Ilseq && sd_use_tuning(&s.host) {
                    let _ = s.host.execute_tuning(h::MMC_SEND_TUNING_BLOCK);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn disks() -> Vec<DiskInfo> {
    DISK.lock().iter().cloned().collect()
}

pub fn read(id: u32, lba: u64, out: &mut [u8]) -> Result<(), DiskError> {
    let d = current_disk(id)?;
    check_range(out.len(), lba, d.blocks)?;
    let mut s = claim().ok_or(DiskError::Gone)?;
    let r = (|| {
        current_disk(id)?;
        let mut done = 0usize;
        while done < out.len() {
            let n = (out.len() - done).min(h::MAX_REQ);
            transfer_retry(&mut s, lba + (done / 512) as u64, (n / 512) as u32, false).map_err(map_err)?;
            s.host.buf.copy_out(0u64, &mut out[done..done + n]);
            done += n;
        }
        Ok(())
    })();
    release(s);
    r
}

pub fn write(id: u32, lba: u64, data: &[u8]) -> Result<(), DiskError> {
    let d = current_disk(id)?;
    if d.read_only { return Err(DiskError::ReadOnly); }
    check_range(data.len(), lba, d.blocks)?;
    let mut s = claim().ok_or(DiskError::Gone)?;
    let r = (|| {
        current_disk(id)?;
        let mut done = 0usize;
        while done < data.len() {
            let n = (data.len() - done).min(h::MAX_REQ);
            s.host.buf.copy_in(0u64, &data[done..done + n]);
            transfer_retry(&mut s, lba + (done / 512) as u64, (n / 512) as u32, true).map_err(map_err)?;
            done += n;
        }
        Ok(())
    })();
    release(s);
    r
}

/// Every write waits for the card's programming, and the card's cache is
/// never enabled, so a flush only checks the card is there.
pub fn flush(id: u32) -> Result<(), DiskError> {
    current_disk(id).map(|_| ())
}

/// Stop the card: power it off and drop the disk.
pub fn eject(id: u32) -> Result<(), DiskError> {
    crate::ext::disk_gone(id);
    current_disk(id)?;
    let mut s = claim().ok_or(DiskError::Gone)?;
    s.card = None;
    power_off(&mut s.host);
    release(s);
    EJECTED.store(true, Ordering::Release);
    let _ = DISK.lock().take();
    kprintln!("[npk] sd: card {} ejected", id);
    Ok(())
}
