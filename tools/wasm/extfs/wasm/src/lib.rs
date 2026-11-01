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
use extfs_core::fat::{Fat, Node};
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
const ST_OK: i32 = 0;
const ST_NOT_FOUND: i32 = 1;
const ST_FAILED: i32 = 2;
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

fn serve(dev: Handle) {
    let mut fs = match Fat::mount(dev) {
        Ok(fs) => fs,
        // Other formats come as further ports; this volume is not shown.
        Err(_) => return,
    };
    if fs.dirty() { log("volume was not cleanly unmounted"); }
    let label = alloc::string::String::from(fs.label());
    if npk_sys::ext_mount(label.as_bytes(), fs.total_bytes(), fs.free_bytes()) != 0 { return; }

    let mut req = alloc::vec![0u8; REQ_HDR + 4096];
    let mut out: Vec<u8> = Vec::new();
    // The node of the last path asked for: reads of one file come in many
    // requests.
    let mut last: Option<(Vec<u8>, Node)> = None;
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
                out.push(node.is_dir as u8);
                out.extend_from_slice(&node.size.to_le_bytes());
                out.extend_from_slice(&node.mtime.to_le_bytes());
                ST_OK
            }
            OP_READ if !node.is_dir => {
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
fn list<D: Dev>(fs: &mut Fat<D>, node: &Node, out: &mut Vec<u8>) -> i32 {
    if !node.is_dir { return ST_NOT_FOUND; }
    let Ok(entries) = fs.read_dir(node) else { return ST_FAILED };
    for e in entries {
        let name = e.name.as_bytes();
        if name.is_empty() || name.len() > 1024 || e.name == "." || e.name == ".."
            || e.name.chars().any(|c| c == '/' || c.is_control())
        {
            continue;
        }
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(e.node.is_dir as u8);
        out.extend_from_slice(&e.node.size.to_le_bytes());
        out.extend_from_slice(&e.node.mtime.to_le_bytes());
        out.extend_from_slice(name);
    }
    ST_OK
}
