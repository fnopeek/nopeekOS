//! USB mass storage over Bulk-Only Transport, after Linux
//! drivers/usb/storage (transport.c `usb_stor_Bulk_transport`,
//! `usb_stor_Bulk_max_lun`, `usb_stor_Bulk_reset`; protocol.c) and the
//! SCSI commands drivers/scsi/sd.c sends to bring a disk up and move its
//! blocks.
//!
//! The xHCI driver hands over each Bulk-Only interface as a pipe
//! (`attached`); a task scans its LUNs outside the controller lock and
//! registers every disk that answers. Nothing here reads a file system:
//! that is crafted input and belongs in a module.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use spin::Mutex;

use crate::kprintln;
use crate::xhci::{self, MscError, MSC_MAX_XFER};

/// What the xHCI driver knows of a new pipe.
pub struct PipeInfo {
    pub port: String,
    pub name: String,
    pub subclass: u8,
    pub super_speed: bool,
}

/// One disk: a LUN that answered and has a medium.
#[derive(Clone)]
pub struct DiskInfo {
    pub id: u32,
    pub port: String,
    pub name: String,
    pub vendor: String,
    pub product: String,
    pub block_size: u32,
    pub blocks: u64,
    pub removable: bool,
    pub read_only: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The disk is gone.
    Gone,
    /// Transport failure; the device was reset.
    Io,
    /// The device refused the command (sense key, ASC, ASCQ).
    Sense(u8, u8, u8),
    /// Blocks outside the disk, or a buffer of the wrong size.
    Range,
    ReadOnly,
}

// Subclasses (`USB_SC_*`): the command set the interface speaks.
const USB_SC_RBC: u8 = 0x01;
const USB_SC_8020: u8 = 0x02;
const USB_SC_8070: u8 = 0x05;
const USB_SC_SCSI: u8 = 0x06;

// Bulk-Only wrappers (`US_BULK_*`).
const US_BULK_CB_SIGN: u32 = 0x4342_5355;
const US_BULK_CS_SIGN: u32 = 0x5342_5355;
const US_BULK_CB_WRAP_LEN: usize = 31;
const US_BULK_CS_WRAP_LEN: usize = 13;
const US_BULK_FLAG_IN: u8 = 0x80;
const US_BULK_STAT_OK: u8 = 0;
const US_BULK_STAT_FAIL: u8 = 1;
const US_BULK_RESET_REQUEST: u8 = 0xFF;
const US_BULK_GET_MAX_LUN: u8 = 0xFE;

// SCSI opcodes.
const TEST_UNIT_READY: u8 = 0x00;
const REQUEST_SENSE: u8 = 0x03;
const INQUIRY: u8 = 0x12;
const MODE_SENSE: u8 = 0x1A;
const START_STOP: u8 = 0x1B;
const ALLOW_MEDIUM_REMOVAL: u8 = 0x1E;
const READ_CAPACITY: u8 = 0x25;
const READ_10: u8 = 0x28;
const WRITE_10: u8 = 0x2A;
const SYNCHRONIZE_CACHE: u8 = 0x35;
const READ_16: u8 = 0x88;
const WRITE_16: u8 = 0x8A;
const SERVICE_ACTION_IN_16: u8 = 0x9E;
const SAI_READ_CAPACITY_16: u8 = 0x10;

// Sense keys.
const NOT_READY: u8 = 0x02;
const ILLEGAL_REQUEST: u8 = 0x05;
const UNIT_ATTENTION: u8 = 0x06;

/// `SD_TIMEOUT`: 30 s per command.
const SD_TIMEOUT_MS: u64 = 30_000;
/// The wrappers are small; a device that does not take them in 10 s is
/// stuck (`US_BULK` control timeouts are of this order).
const WRAP_TIMEOUT_MS: u64 = 10_000;
/// `delay_use`: a new device gets a second before the first command.
const DELAY_USE_MS: u64 = 1_000;
/// `usb_stor_reset_common` gives a reset device six seconds.
const RESET_RECOVERY_MS: u64 = 6_000;
/// `SD_MAX_RETRIES`.
const SD_MAX_RETRIES: u32 = 5;
/// `max_sectors`: 240 sectors below SuperSpeed, 2048 at it (usb-storage
/// `slave_configure` and scsiglue.c).
const MAX_BYTES_HIGH: usize = 240 * 512;
const MAX_BYTES_SUPER: usize = 2048 * 512;

struct Pipe {
    id: u32,
    info: PipeInfo,
    tag: u32,
    busy: bool,
    scanned: bool,
}

struct Disk {
    info: DiskInfo,
    pipe: u32,
    lun: u8,
    /// Blocks per READ/WRITE.
    max_blocks: u32,
}

static PIPES: Mutex<Vec<Pipe>> = Mutex::new(Vec::new());
static DISKS: Mutex<Vec<Disk>> = Mutex::new(Vec::new());
static NEXT_DISK: AtomicU32 = AtomicU32::new(1);
static PENDING: AtomicBool = AtomicBool::new(false);
static SCANNING: AtomicBool = AtomicBool::new(false);

/// From the xHCI driver, under its lock: a Bulk-Only interface is ready.
pub fn attached(pipe: u32, info: PipeInfo) {
    PIPES.lock().push(Pipe { id: pipe, info, tag: 0, busy: false, scanned: false });
    PENDING.store(true, Ordering::Release);
}

/// From the xHCI driver, under its lock: the interface is gone.
pub fn detached(pipe: u32) {
    PIPES.lock().retain(|p| p.id != pipe);
    let mut d = DISKS.lock();
    for disk in d.iter().filter(|d| d.pipe == pipe) {
        kprintln!("[npk] usb-storage: disk {} ({}) gone", disk.info.id, disk.info.name);
    }
    d.retain(|d| d.pipe != pipe);
}

/// From the idle step: scan new pipes on a worker.
pub fn kick() {
    if !PENDING.load(Ordering::Acquire) { return; }
    if SCANNING.swap(true, Ordering::AcqRel) { return; }
    crate::smp::scheduler::spawn("usb storage", scan_task, 0);
}

fn scan_task(_: u64) {
    loop {
        PENDING.store(false, Ordering::Release);
        loop {
            let next = PIPES.lock().iter_mut().find(|p| !p.scanned).map(|p| {
                p.scanned = true;
                p.id
            });
            let Some(pipe) = next else { break };
            sleep_ms(DELAY_USE_MS);
            scan_pipe(pipe);
        }
        SCANNING.store(false, Ordering::Release);
        if !PENDING.load(Ordering::Acquire) || SCANNING.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

fn sleep_ms(ms: u64) {
    if !crate::smp::fiber::yield_sleep(ms) {
        crate::interrupts::delay_ms(ms);
    }
}

/// Every disk there is now.
pub fn disks() -> Vec<DiskInfo> {
    DISKS.lock().iter().map(|d| d.info.clone()).collect()
}

// === Pipe ownership ===

/// One command at a time per pipe, as Bulk-Only requires; the guard holds
/// the pipe and the tag of its command.
struct Claim {
    pipe: u32,
    subclass: u8,
}

impl Claim {
    fn take(pipe: u32) -> Result<Claim, Error> {
        loop {
            {
                let mut g = PIPES.lock();
                let p = g.iter_mut().find(|p| p.id == pipe).ok_or(Error::Gone)?;
                if !p.busy {
                    p.busy = true;
                    return Ok(Claim { pipe, subclass: p.info.subclass });
                }
            }
            crate::smp::fiber::spin_or_yield();
        }
    }

    fn next_tag(&self) -> u32 {
        let mut g = PIPES.lock();
        match g.iter_mut().find(|p| p.id == self.pipe) {
            Some(p) => { p.tag = p.tag.wrapping_add(1); p.tag }
            None => 0,
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(p) = PIPES.lock().iter_mut().find(|p| p.id == self.pipe) {
            p.busy = false;
        }
    }
}

// === Bulk-Only Transport ===

enum Data<'a> {
    None,
    In(&'a mut [u8]),
    Out(&'a [u8]),
}

impl Data<'_> {
    fn len(&self) -> usize {
        match self { Data::None => 0, Data::In(b) => b.len(), Data::Out(b) => b.len() }
    }
}

/// A transport failure: the device needs `reset_recovery`.
struct Broken;

fn moved(r: Result<usize, MscError>) -> Result<Result<usize, MscError>, Broken> {
    match r {
        Err(MscError::Gone) => Err(Broken),
        r => Ok(r),
    }
}

/// `usb_stor_Bulk_transport` plus the auto-sense of
/// `usb_stor_invoke_transport`: CBW, data, CSW; a failed command fetches
/// its sense data. Returns the bytes moved.
fn command(c: &Claim, lun: u8, cdb: &[u8], mut data: Data<'_>) -> Result<usize, Error> {
    match transport(c, lun, cdb, &mut data) {
        Ok(Ok(n)) => Ok(n),
        Ok(Err(())) => Err(request_sense(c, lun)),
        Err(Broken) => {
            reset_recovery(c);
            Err(if xhci::msc_subclass(c.pipe).is_none() { Error::Gone } else { Error::Io })
        }
    }
}

/// `Ok(Ok(n))`: good status; `Ok(Err(()))`: the device reported a
/// failed command; `Err(Broken)`: the transport broke.
fn transport(c: &Claim, lun: u8, cdb: &[u8], data: &mut Data<'_>) -> Result<Result<usize, ()>, Broken> {
    let pipe = c.pipe;
    let len = data.len();
    if len > MSC_MAX_XFER || cdb.len() > 16 { return Err(Broken); }
    // `usb_stor_pad12_command`: ATAPI and SFF-8070i take 12-byte commands.
    let cmd_len = match c.subclass {
        USB_SC_8020 | USB_SC_8070 => 12,
        _ => cdb.len(),
    };
    let tag = c.next_tag();
    let mut cbw = [0u8; US_BULK_CB_WRAP_LEN];
    cbw[0..4].copy_from_slice(&US_BULK_CB_SIGN.to_le_bytes());
    cbw[4..8].copy_from_slice(&tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    cbw[12] = if matches!(data, Data::In(_)) { US_BULK_FLAG_IN } else { 0 };
    cbw[13] = lun;
    cbw[14] = cmd_len as u8;
    cbw[15..15 + cdb.len()].copy_from_slice(cdb);
    xhci::msc_buf_write(pipe, 0, &cbw).map_err(|_| Broken)?;
    match moved(xhci::msc_transfer(pipe, false, US_BULK_CB_WRAP_LEN, WRAP_TIMEOUT_MS))? {
        Ok(US_BULK_CB_WRAP_LEN) => {}
        _ => return Err(Broken),
    }

    // Data stage. A stall ends it early; the CSW still follows.
    let mut done = 0usize;
    if len > 0 {
        let dir_in = matches!(data, Data::In(_));
        if let Data::Out(b) = data {
            xhci::msc_buf_write(pipe, 0, b).map_err(|_| Broken)?;
        }
        match moved(xhci::msc_transfer(pipe, dir_in, len, SD_TIMEOUT_MS))? {
            Ok(n) => done = n,
            Err(MscError::Stall) => {
                if !xhci::msc_clear_halt(pipe, dir_in) { return Err(Broken); }
            }
            Err(_) => return Err(Broken),
        }
        if let Data::In(b) = data {
            xhci::msc_buf_read(pipe, 0, &mut b[..done]).map_err(|_| Broken)?;
        }
    }

    // Status: a stall here is cleared and the CSW read once more.
    let mut csw = [0u8; US_BULK_CS_WRAP_LEN];
    let mut got = moved(xhci::msc_transfer(pipe, true, US_BULK_CS_WRAP_LEN, WRAP_TIMEOUT_MS))?;
    if got == Err(MscError::Stall) {
        if !xhci::msc_clear_halt(pipe, true) { return Err(Broken); }
        got = moved(xhci::msc_transfer(pipe, true, US_BULK_CS_WRAP_LEN, WRAP_TIMEOUT_MS))?;
    }
    if got != Ok(US_BULK_CS_WRAP_LEN) { return Err(Broken); }
    xhci::msc_buf_read(pipe, 0, &mut csw).map_err(|_| Broken)?;
    let sign = u32::from_le_bytes([csw[0], csw[1], csw[2], csw[3]]);
    let ctag = u32::from_le_bytes([csw[4], csw[5], csw[6], csw[7]]);
    if sign != US_BULK_CS_SIGN || ctag != tag { return Err(Broken); }
    match csw[12] {
        US_BULK_STAT_OK => Ok(Ok(done)),
        US_BULK_STAT_FAIL => Ok(Err(())),
        // Phase error or anything else.
        _ => Err(Broken),
    }
}

/// REQUEST SENSE after a failed command.
fn request_sense(c: &Claim, lun: u8) -> Error {
    let mut b = [0u8; 18];
    let cdb = [REQUEST_SENSE, 0, 0, 0, b.len() as u8, 0];
    match transport(c, lun, &cdb, &mut Data::In(&mut b)) {
        Ok(Ok(n)) if n >= 14 => Error::Sense(b[2] & 0x0F, b[12], b[13]),
        Ok(Ok(n)) if n >= 3 => Error::Sense(b[2] & 0x0F, 0, 0),
        Ok(_) => Error::Io,
        Err(Broken) => {
            reset_recovery(c);
            Error::Io
        }
    }
}

/// `usb_stor_Bulk_reset`: Bulk-Only Mass Storage Reset, a pause, then both
/// bulk endpoints cleared.
fn reset_recovery(c: &Claim) {
    kprintln!("[npk] usb-storage: pipe {} reset", c.pipe);
    if !xhci::msc_class_request(c.pipe, 0x21, US_BULK_RESET_REQUEST, 0, &mut []) {
        return;
    }
    sleep_ms(RESET_RECOVERY_MS);
    xhci::msc_clear_halt(c.pipe, true);
    xhci::msc_clear_halt(c.pipe, false);
}

// === Bring-up: `usb_stor_scan_dwork`, sd_probe / sd_revalidate_disk ===

fn scan_pipe(pipe: u32) {
    let Ok(c) = Claim::take(pipe) else { return };
    if !matches!(c.subclass, USB_SC_RBC | USB_SC_8020 | USB_SC_8070 | USB_SC_SCSI) {
        kprintln!("[npk] usb-storage: pipe {} subclass {:02x} not supported", pipe, c.subclass);
        return;
    }
    // `usb_stor_Bulk_max_lun`: a failed or invalid answer means one LUN.
    let mut b = [0u8; 1];
    let max_lun = if xhci::msc_class_request(pipe, 0xA1, US_BULK_GET_MAX_LUN, 0, &mut b) && b[0] < 16 {
        b[0]
    } else {
        0
    };
    let (port, name, sup) = match PIPES.lock().iter().find(|p| p.id == pipe) {
        Some(p) => (p.info.port.clone(), p.info.name.clone(), p.info.super_speed),
        None => return,
    };
    for lun in 0..=max_lun {
        match probe_lun(&c, lun, sup) {
            Ok(Some((mut info, max_blocks))) => {
                info.port = port.clone();
                if info.name.is_empty() { info.name = name.clone(); }
                kprintln!("[npk] usb-storage: disk {} on port {}: {} ({} blocks of {} B, {} MB{}{})",
                    info.id, port, info.name, info.blocks, info.block_size,
                    info.blocks.saturating_mul(info.block_size as u64) / 1_000_000,
                    if info.removable { ", removable" } else { "" },
                    if info.read_only { ", read-only" } else { "" });
                if PIPES.lock().iter().any(|p| p.id == pipe) {
                    DISKS.lock().push(Disk { info, pipe, lun, max_blocks });
                }
            }
            Ok(None) => {}
            Err(e) => kprintln!("[npk] usb-storage: port {} LUN {}: {:?}", port, lun, e),
        }
    }
}

/// INQUIRY, spin-up, capacity and write protection of one LUN; `None` for
/// a LUN that is no disk or has no medium.
fn probe_lun(c: &Claim, lun: u8, sup: bool) -> Result<Option<(DiskInfo, u32)>, Error> {
    let mut inq = [0u8; 36];
    let n = retry(|| command(c, lun, &[INQUIRY, 0, 0, 0, 36, 0], Data::In(&mut inq)))?;
    if n < 5 { return Ok(None); }
    // Direct access, CD/DVD, optical memory, RBC.
    let ptype = inq[0] & 0x1F;
    if inq[0] >> 5 != 0 || !matches!(ptype, 0x00 | 0x05 | 0x07 | 0x0E) { return Ok(None); }
    let removable = inq[1] & 0x80 != 0;
    let field = |a: usize, b: usize| -> String {
        let s: String = inq[a..b.min(n.max(a))].iter()
            .map(|&ch| if (0x20..0x7F).contains(&ch) { ch as char } else { ' ' })
            .collect();
        String::from(s.trim())
    };
    let (vendor, product) = (field(8, 16), field(16, 32));

    if !spin_up(c, lun)? { return Ok(None); }
    let (blocks, block_size) = read_capacity(c, lun)?;
    if blocks == 0 { return Ok(None); }
    let read_only = ptype == 0x05 || write_protected(c, lun);
    let max_bytes = if sup { MAX_BYTES_SUPER } else { MAX_BYTES_HIGH }.min(MSC_MAX_XFER);
    let max_blocks = (max_bytes / block_size as usize).max(1) as u32;
    let name = match (vendor.is_empty(), product.is_empty()) {
        (false, false) => alloc::format!("{} {}", vendor, product),
        (true, false) => product.clone(),
        _ => vendor.clone(),
    };
    Ok(Some((DiskInfo {
        id: NEXT_DISK.fetch_add(1, Ordering::Relaxed), port: String::new(), name,
        vendor, product, block_size, blocks, removable, read_only,
    }, max_blocks)))
}

/// `sd_spinup_disk`: TEST UNIT READY until the medium is ready, START UNIT
/// when the device asks for it. False when no medium is present.
fn spin_up(c: &Claim, lun: u8) -> Result<bool, Error> {
    let mut started = false;
    for _ in 0..30 {
        match command(c, lun, &[TEST_UNIT_READY, 0, 0, 0, 0, 0], Data::None) {
            Ok(_) => return Ok(true),
            Err(Error::Sense(UNIT_ATTENTION, ..)) => continue,
            // Medium not present.
            Err(Error::Sense(NOT_READY, 0x3A, _)) => return Ok(false),
            // Initializing command required.
            Err(Error::Sense(NOT_READY, 0x04, 0x02)) if !started => {
                started = true;
                let _ = command(c, lun, &[START_STOP, 0, 0, 0, 0x01, 0], Data::None);
            }
            Err(Error::Sense(NOT_READY, ..)) => sleep_ms(1_000),
            Err(Error::Sense(..)) => return Ok(true),
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// `read_capacity_10`, and `read_capacity_16` when the disk is larger than
/// 32-bit LBAs reach. Returns (blocks, block size).
fn read_capacity(c: &Claim, lun: u8) -> Result<(u64, u32), Error> {
    let mut b = [0u8; 8];
    let n = retry(|| command(c, lun, &[READ_CAPACITY, 0, 0, 0, 0, 0, 0, 0, 0, 0], Data::In(&mut b)))?;
    if n < 8 { return Err(Error::Io); }
    let last = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut size = u32::from_be_bytes([b[4], b[5], b[6], b[7]]);
    let mut blocks = last as u64 + 1;
    if last == u32::MAX {
        let mut b = [0u8; 32];
        let mut cdb = [0u8; 16];
        cdb[0] = SERVICE_ACTION_IN_16;
        cdb[1] = SAI_READ_CAPACITY_16;
        cdb[13] = b.len() as u8;
        let n = retry(|| command(c, lun, &cdb, Data::In(&mut b)))?;
        if n < 12 { return Err(Error::Io); }
        let last = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        size = u32::from_be_bytes([b[8], b[9], b[10], b[11]]);
        blocks = last.checked_add(1).ok_or(Error::Io)?;
    }
    // `sd_read_capacity`: sector sizes from 512 to 4096, powers of two.
    if !(512..=4096).contains(&size) || !size.is_power_of_two() {
        kprintln!("[npk] usb-storage: unsupported block size {}", size);
        return Err(Error::Io);
    }
    Ok((blocks, size))
}

/// `sd_read_write_protect_flag` with the 192-byte page 0x3F usb-storage
/// asks for; a device that does not answer is taken as writable.
fn write_protected(c: &Claim, lun: u8) -> bool {
    let mut b = [0u8; 192];
    match command(c, lun, &[MODE_SENSE, 0, 0x3F, 0, 192, 0], Data::In(&mut b)) {
        Ok(n) if n >= 4 => b[2] & 0x80 != 0,
        _ => false,
    }
}

/// `SD_MAX_RETRIES` for commands that may meet a unit attention or a
/// transport reset.
fn retry<T>(mut f: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let mut last = Error::Io;
    for _ in 0..SD_MAX_RETRIES {
        match f() {
            Ok(v) => return Ok(v),
            Err(e @ (Error::Sense(UNIT_ATTENTION, ..) | Error::Io)) => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

// === Block access ===

fn disk(id: u32) -> Result<(u32, u8, u32, u64, u32, bool), Error> {
    let g = DISKS.lock();
    let d = g.iter().find(|d| d.info.id == id).ok_or(Error::Gone)?;
    Ok((d.pipe, d.lun, d.info.block_size, d.info.blocks, d.max_blocks, d.info.read_only))
}

/// READ(10)/WRITE(10), or the 16-byte forms past 32-bit LBAs (sd.c
/// `sd_setup_rw10_cmnd`, `sd_setup_rw16_cmnd`).
fn rw_cdb(write: bool, lba: u64, count: u32) -> ([u8; 16], usize) {
    let mut cdb = [0u8; 16];
    if lba + count as u64 - 1 <= u32::MAX as u64 && count <= u16::MAX as u32 {
        cdb[0] = if write { WRITE_10 } else { READ_10 };
        cdb[2..6].copy_from_slice(&(lba as u32).to_be_bytes());
        cdb[7..9].copy_from_slice(&(count as u16).to_be_bytes());
        (cdb, 10)
    } else {
        cdb[0] = if write { WRITE_16 } else { READ_16 };
        cdb[2..10].copy_from_slice(&lba.to_be_bytes());
        cdb[10..14].copy_from_slice(&count.to_be_bytes());
        (cdb, 16)
    }
}

/// Read `out.len() / block_size` blocks from `lba`.
pub fn read(id: u32, lba: u64, out: &mut [u8]) -> Result<(), Error> {
    let (pipe, lun, bs, blocks, max, _) = disk(id)?;
    let count = check_range(out.len(), bs, lba, blocks)?;
    let c = Claim::take(pipe)?;
    let mut at = 0u64;
    while at < count {
        let n = (count - at).min(max as u64) as u32;
        let (cdb, l) = rw_cdb(false, lba + at, n);
        let off = (at * bs as u64) as usize;
        let buf = &mut out[off..off + n as usize * bs as usize];
        let got = retry(|| command(&c, lun, &cdb[..l], Data::In(&mut *buf)))?;
        if got != buf.len() { return Err(Error::Io); }
        at += n as u64;
    }
    Ok(())
}

/// Write `data.len() / block_size` blocks at `lba`.
pub fn write(id: u32, lba: u64, data: &[u8]) -> Result<(), Error> {
    let (pipe, lun, bs, blocks, max, ro) = disk(id)?;
    if ro { return Err(Error::ReadOnly); }
    let count = check_range(data.len(), bs, lba, blocks)?;
    let c = Claim::take(pipe)?;
    let mut at = 0u64;
    while at < count {
        let n = (count - at).min(max as u64) as u32;
        let (cdb, l) = rw_cdb(true, lba + at, n);
        let off = (at * bs as u64) as usize;
        let buf = &data[off..off + n as usize * bs as usize];
        let got = retry(|| command(&c, lun, &cdb[..l], Data::Out(buf)))?;
        if got != buf.len() { return Err(Error::Io); }
        at += n as u64;
    }
    Ok(())
}

fn check_range(len: usize, bs: u32, lba: u64, blocks: u64) -> Result<u64, Error> {
    if len == 0 || len % bs as usize != 0 { return Err(Error::Range); }
    let count = (len / bs as usize) as u64;
    match lba.checked_add(count) {
        Some(end) if end <= blocks => Ok(count),
        _ => Err(Error::Range),
    }
}

/// SYNCHRONIZE CACHE; a device without a cache refuses it, which is fine
/// (`sd_sync_cache`).
pub fn flush(id: u32) -> Result<(), Error> {
    let (pipe, lun, ..) = disk(id)?;
    let c = Claim::take(pipe)?;
    match retry(|| command(&c, lun, &[SYNCHRONIZE_CACHE, 0, 0, 0, 0, 0, 0, 0, 0, 0], Data::None)) {
        Ok(_) | Err(Error::Sense(ILLEGAL_REQUEST, ..)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Flush, allow removal, stop the unit and unload the medium; the disk is
/// then gone from the list (`sd_shutdown`, `scsi_eject`).
pub fn eject(id: u32) -> Result<(), Error> {
    flush(id)?;
    let (pipe, lun, ..) = disk(id)?;
    {
        let c = Claim::take(pipe)?;
        let _ = command(&c, lun, &[ALLOW_MEDIUM_REMOVAL, 0, 0, 0, 0, 0], Data::None);
        let _ = command(&c, lun, &[START_STOP, 0, 0, 0, 0x02, 0], Data::None);
    }
    DISKS.lock().retain(|d| d.info.id != id);
    kprintln!("[npk] usb-storage: disk {} ejected, safe to remove", id);
    Ok(())
}
