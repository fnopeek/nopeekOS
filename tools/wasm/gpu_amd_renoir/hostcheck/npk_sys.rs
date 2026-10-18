//! The part of npk_sys the host check needs, on the host. `log` prints the
//! line without the module's prefix. The card is absent unless `fake_card`
//! puts one there: a register file of a few registers and a VRAM image
//! read through MM_INDEX/MM_INDEX_HI/MM_DATA, as `amdgpu_device_mm_access`
//! reads it.

use std::cell::RefCell;
use std::collections::HashMap;

pub struct FakeCard {
    /// Register (dword index) -> value, for plain reads.
    pub regs: HashMap<u32, u32>,
    pub mm_index: u32,
    pub mm_index_hi: u32,
    pub mm_data: u32,
    /// VRAM bytes at `vram_base`.
    pub vram_base: u64,
    pub vram: Vec<u8>,
}

thread_local! {
    static CARD: RefCell<Option<FakeCard>> = const { RefCell::new(None) };
    static ACPI: RefCell<Vec<([u8; 4], Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

/// An ACPI table `acpi_table` hands out.
pub fn fake_acpi(sig: [u8; 4], table: Vec<u8>) {
    ACPI.with(|a| a.borrow_mut().push((sig, table)));
}

/// `npk_acpi_table`: the length, and the bytes into `buf` when it is large
/// enough; -1 when there is no such table.
pub fn acpi_table(sig: [u8; 4], index: i32, buf: &mut [u8]) -> i32 {
    ACPI.with(|a| {
        let a = a.borrow();
        let Some((_, t)) = a.iter().filter(|e| e.0 == sig).nth(index as usize) else { return -1 };
        if buf.len() >= t.len() {
            buf[..t.len()].copy_from_slice(t);
        }
        t.len() as i32
    })
}

pub fn fake_card(card: FakeCard) {
    CARD.with(|c| *c.borrow_mut() = Some(card));
}

#[derive(Clone, Copy, Debug)]
pub struct Mmio(pub i32);

impl Mmio {
    pub fn r32(self, off: u32) -> u32 {
        CARD.with(|c| {
            let c = c.borrow();
            let Some(c) = c.as_ref() else { return 0 };
            let reg = off / 4;
            if reg == c.mm_data {
                let lo = c.regs.get(&c.mm_index).copied().unwrap_or(0) & 0x7fff_ffff;
                let hi = c.regs.get(&c.mm_index_hi).copied().unwrap_or(0) as u64;
                let pos = (hi << 31 | lo as u64).wrapping_sub(c.vram_base) as usize;
                return c.vram.get(pos..pos + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
            }
            c.regs.get(&reg).copied().unwrap_or(0)
        })
    }

    pub fn w32(self, off: u32, v: u32) {
        CARD.with(|c| {
            if let Some(c) = c.borrow_mut().as_mut() {
                c.regs.insert(off / 4, v);
            }
        })
    }
}

thread_local! {
    static FIRMWARE_DIR: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

/// Where `sys/firmware/<path>` is read from: a staged `<module>.fw/`.
pub fn fake_firmware_dir(dir: &str) {
    FIRMWARE_DIR.with(|d| *d.borrow_mut() = Some(std::path::PathBuf::from(dir)));
}

fn host_path(name: &[u8]) -> Option<std::path::PathBuf> {
    let name = std::str::from_utf8(name).ok()?;
    let rel = name.strip_prefix("sys/firmware/")?;
    FIRMWARE_DIR.with(|d| d.borrow().as_ref().map(|d| d.join(rel)))
}

/// `npk_fs_stat`: size, directory flag, mtime; 0 when absent.
pub fn fs_stat(name: &[u8], out: &mut [u8; 17]) -> i32 {
    let Some(m) = host_path(name).and_then(|p| std::fs::metadata(p).ok()) else { return 0 };
    out[..8].copy_from_slice(&m.len().to_le_bytes());
    out[8] = m.is_dir() as u8;
    17
}

/// `npk_fetch`: the bytes into `buf`, their count; -1 when absent.
pub fn fetch(name: &[u8], buf: &mut [u8]) -> i32 {
    let Some(data) = host_path(name).and_then(|p| std::fs::read(p).ok()) else { return -1 };
    let n = data.len().min(buf.len());
    buf[..n].copy_from_slice(&data[..n]);
    n as i32
}

/// `npk_pci_read_config_at`: no device answers on the host.
pub fn pci_read_config_at(_bdf: i32, _offset: i32) -> i32 {
    -1
}

pub fn log(b: &[u8]) {
    let s = String::from_utf8_lossy(b);
    println!("{}", s.strip_prefix("[gpu_amd_renoir] ").unwrap_or(&s));
}

pub fn sys_info(_key: i32) -> i64 {
    0
}

pub fn now_us() -> i64 {
    0
}

pub fn sleep(_ms: i32) -> i32 {
    0
}
