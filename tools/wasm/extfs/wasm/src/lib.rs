//! extfs.wasm: the partition table and file system of one external drive.
//!
//! The kernel starts it in two roles (`storage::ext`). As a probe it sees
//! the whole disk, reads the partition table and reports the partitions.
//! As a volume it sees one partition, mounts the file system on it, names
//! the volume and answers the file requests the kernel routes from
//! `ext/<label>/`. It holds no right: the block handle is all it can reach.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use extfs_core::exfat::Exfat;
use extfs_core::fat::{self, Fat};
use extfs_core::Volume;
use extfs_core::{part, Dev, IoError};

// No rights: the block handle is bound to the instance by the kernel.
#[unsafe(link_section = ".npk.caps")]
#[used]
static NPK_CAPS: [u8; 1] = [0x00];

#[global_allocator]
static ALLOC: nopeek_widgets::heap::Allocator = nopeek_widgets::heap::new();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    npk_sys::log_serial(b"[extfs] panic");
    core::arch::wasm32::unreachable()
}

const ROLE_PROBE: u32 = 1;
const ROLE_VOLUME: u32 = 2;
const OP_LIST: u8 = 1;
const OP_STAT: u8 = 2;
const OP_READ: u8 = 3;
const OP_CREATE: u8 = 4;
const OP_WRITE: u8 = 5;
const OP_MKDIR: u8 = 6;
const OP_DELETE: u8 = 7;
const OP_RENAME: u8 = 8;
const ST_OK: i32 = 0;
const ST_NOT_FOUND: i32 = 1;
const ST_FAILED: i32 = 2;
const ST_EXISTS: i32 = 3;
const ST_NOT_EMPTY: i32 = 4;
const ST_NO_SPACE: i32 = 5;
const REQ_HDR: usize = 24;
/// Largest block transfer per host call.
const MAX_IO: usize = 1 << 20;

struct Handle {
    bs: u32,
    blocks: u64,
}

impl Dev for Handle {
    fn block_size(&self) -> u32 { self.bs }
    fn blocks(&self) -> u64 { self.blocks }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let bs = self.bs as usize;
        let mut at = 0;
        while at < buf.len() {
            let n = (buf.len() - at).min(MAX_IO / bs * bs);
            if npk_sys::blk_read(lba + (at / bs) as u64, &mut buf[at..at + n]) != 0 { return Err(IoError); }
            at += n;
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let bs = self.bs as usize;
        let mut at = 0;
        while at < buf.len() {
            let n = (buf.len() - at).min(MAX_IO / bs * bs);
            if npk_sys::blk_write(lba + (at / bs) as u64, &buf[at..at + n]) != 0 { return Err(IoError); }
            at += n;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), IoError> {
        if npk_sys::blk_flush() == 0 { Ok(()) } else { Err(IoError) }
    }
}

fn status_of(e: fat::Error) -> i32 {
    match e {
        fat::Error::NotFound => ST_NOT_FOUND,
        fat::Error::Exists => ST_EXISTS,
        fat::Error::NotEmpty => ST_NOT_EMPTY,
        fat::Error::NoSpace => ST_NO_SPACE,
        _ => ST_FAILED,
    }
}

fn log(msg: &str) {
    npk_sys::log_serial(alloc::format!("[extfs] {}", msg).as_bytes());
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let Some((role, bs, blocks, _ro)) = npk_sys::blk_info() else { return };
    let dev = Handle { bs, blocks };
    match role {
        ROLE_PROBE => probe(dev),
        ROLE_VOLUME => serve(dev),
        _ => {}
    }
}

fn probe(mut dev: Handle) {
    let parts = match part::scan(&mut dev) {
        Ok(p) => p,
        Err(_) => { log("partition table not readable"); return; }
    };
    let mut b = Vec::with_capacity(parts.len() * 16);
    for p in &parts {
        b.extend_from_slice(&p.start.to_le_bytes());
        b.extend_from_slice(&p.count.to_le_bytes());
    }
    npk_sys::ext_parts(&b);
}

/// exFAT carries its own name in the boot sector, so it is tried first;
/// a volume neither format takes is not shown.
fn serve(dev: Handle) {
    let (bs, blocks) = (dev.bs, dev.blocks);
    if let Ok(fs) = Exfat::mount(dev) { return run(fs); }
    if let Ok(fs) = Fat::mount(Handle { bs, blocks }) { run(fs); }
}

fn run<V: Volume>(mut fs: V) {
    if fs.dirty() { log("volume was not cleanly unmounted"); }
    let label = alloc::string::String::from(fs.label());
    if npk_sys::ext_mount(label.as_bytes(), fs.total_bytes(), fs.free_bytes()) != 0 { return; }

    // Header, two paths and a write's payload.
    let mut req = alloc::vec![0u8; REQ_HDR + 2 * 4096 + MAX_IO];
    let mut out: Vec<u8> = Vec::new();
    // The node of the last path asked for: reads of one file come in many
    // requests.
    let mut last: Option<(Vec<u8>, V::Node)> = None;
    loop {
        let n = npk_sys::ext_next(&mut req, 5000);
        if n == -2 || n < 0 { return; }
        if n == 0 { continue; }
        let r = &req[..n as usize];
        if r.len() < REQ_HDR { continue; }
        let seq = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        let op = r[4];
        let off = u64::from_le_bytes(r[8..16].try_into().unwrap_or([0; 8]));
        let len = u32::from_le_bytes([r[16], r[17], r[18], r[19]]) as usize;
        let plen = u16::from_le_bytes([r[20], r[21]]) as usize;
        let Some(path) = r.get(REQ_HDR..REQ_HDR + plen) else { continue };
        let Ok(path_str) = core::str::from_utf8(path) else {
            npk_sys::ext_reply(seq, ST_FAILED, &[]);
            continue;
        };
        if matches!(op, OP_CREATE | OP_WRITE | OP_MKDIR | OP_DELETE | OP_RENAME) {
            last = None;
            let Some(payload) = r.get(REQ_HDR + plen..REQ_HDR + plen + if op == OP_CREATE || op == OP_MKDIR || op == OP_DELETE { 0 } else { len }) else {
                npk_sys::ext_reply(seq, ST_FAILED, &[]);
                continue;
            };
            fs.set_now(npk_sys::unix_time().max(0) as u64);
            let done = match op {
                OP_CREATE => fs.create(path_str).and_then(|_| fs.sync()),
                OP_WRITE => fs.append(path_str, off, payload),
                OP_MKDIR => fs.mkdir(path_str).and_then(|_| fs.sync()),
                OP_DELETE => fs.remove(path_str).and_then(|_| fs.sync()),
                _ => match core::str::from_utf8(payload) {
                    Ok(to) => fs.rename(path_str, to, off == 1).and_then(|_| fs.sync()),
                    Err(_) => Err(fat::Error::BadName),
                },
            };
            match done {
                Ok(()) => npk_sys::ext_reply(seq, ST_OK, &fs.free_bytes().to_le_bytes()),
                Err(e) => {
                    // A failed change may have marked the volume: settle it.
                    let _ = fs.sync();
                    npk_sys::ext_reply(seq, status_of(e), &[])
                }
            };
            continue;
        }
        let node = match &last {
            Some((p, n)) if p == path => Ok(Some(*n)),
            _ => fs.lookup(path_str),
        };
        let node = match node {
            Ok(Some(n)) => { last = Some((Vec::from(path), n)); n }
            Ok(None) => { npk_sys::ext_reply(seq, ST_NOT_FOUND, &[]); continue; }
            Err(_) => { npk_sys::ext_reply(seq, ST_FAILED, &[]); continue; }
        };
        out.clear();
        let status = match op {
            OP_LIST => list(&mut fs, &node, &mut out),
            OP_STAT => {
                let (is_dir, size, mtime) = V::info(&node);
                out.push(is_dir as u8);
                out.extend_from_slice(&size.to_le_bytes());
                out.extend_from_slice(&mtime.to_le_bytes());
                ST_OK
            }
            OP_READ if !V::info(&node).0 => {
                out.resize(len.min(MAX_IO), 0);
                match fs.read(&node, off, &mut out) {
                    Ok(k) => { out.truncate(k); ST_OK }
                    Err(_) => { out.clear(); ST_FAILED }
                }
            }
            _ => ST_FAILED,
        };
        npk_sys::ext_reply(seq, status, &out);
    }
}

/// A folder's entries in the reply format; names that cannot be one path
/// segment are left out.
fn list<V: Volume>(fs: &mut V, node: &V::Node, out: &mut Vec<u8>) -> i32 {
    if !V::info(node).0 { return ST_NOT_FOUND; }
    let Ok(entries) = fs.list(node) else { return ST_FAILED };
    for (name_s, n) in entries {
        let name = name_s.as_bytes();
        if name.is_empty() || name.len() > 1024 || name_s == "." || name_s == ".."
            || name_s.chars().any(|c| c == '/' || c.is_control())
        {
            continue;
        }
        let (is_dir, size, mtime) = V::info(&n);
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(is_dir as u8);
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&mtime.to_le_bytes());
        out.extend_from_slice(name);
    }
    ST_OK
}
