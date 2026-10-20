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
    /// A write to the first register sets the second to the value: a
    /// mailbox that answers.
    pub on_write: Vec<(u32, u32, u32)>,
    /// Every write, (register, value), in order.
    pub writes: Vec<(u32, u32)>,
    /// The PSP's C2PMSG_67, the KM ring's write pointer: a write signals
    /// the fence of the frame before it. The ring's address is in
    /// C2PMSG_69/70 and its size in C2PMSG_71, as psp_v12_0 writes them;
    /// GPU addresses are offsets into the VRAM window (VRAM at 0, the
    /// aperture at 0, as the fake card puts them).
    pub psp_wptr: Option<u32>,
}

/// The writes the card has seen.
pub fn fake_card_writes() -> Vec<(u32, u32)> {
    CARD.with(|c| c.borrow().as_ref().map_or(Vec::new(), |c| c.writes.clone()))
}

/// From here on a register never written reads `rule(reg)`, and the
/// PCIe index/data pair (dword numbers) reaches a register space of its
/// own, where an address never written reads `rule(address)`.
pub fn fake_card_unset(rule: fn(u32) -> u32, index_data: (u32, u32)) {
    UNSET.with(|u| *u.borrow_mut() = Some((rule, index_data)));
}

thread_local! {
    static ECHO: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static ECHO_SKIP: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// Registers whose writes the echo leaves out.
pub fn fake_card_echo_skip(regs: &[u32]) {
    ECHO_SKIP.with(|e| *e.borrow_mut() = regs.to_vec());
}

/// From here on every register write is printed as it happens.
pub fn fake_card_echo(on: bool) {
    ECHO.with(|e| e.set(on));
}

/// The registers the card holds, (register, value), by register.
pub fn fake_card_regs() -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> =
        CARD.with(|c| c.borrow().as_ref().map_or(Vec::new(), |c| c.regs.iter().map(|(&r, &v)| (r, v)).collect()));
    v.sort();
    v
}

/// What the card holds behind the PCIe index/data pair, by address.
pub fn fake_card_indirect() -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = INDIRECT.with(|i| i.borrow().iter().map(|(&a, &v)| (a, v)).collect());
    v.sort();
    v
}

/// Forget the writes seen so far.
pub fn fake_card_clear_writes() {
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.writes.clear() });
}

thread_local! {
    static UNSET: RefCell<Option<(fn(u32) -> u32, (u32, u32))>> = const { RefCell::new(None) };
    static INDIRECT: RefCell<HashMap<u32, u32>> = RefCell::new(HashMap::new());
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

thread_local! {
    /// Memory windows: Mmio(1..) and DMA buffers, by handle - 1.
    static WINDOWS: RefCell<Vec<(u64, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

fn window(bus: u64, len: usize) -> i32 {
    WINDOWS.with(|w| {
        let mut w = w.borrow_mut();
        w.push((bus, vec![0; len]));
        w.len() as i32
    })
}

fn win_r32(h: i32, off: u32) -> u32 {
    WINDOWS.with(|w| {
        let w = w.borrow();
        let b = &w[h as usize - 1].1;
        b.get(off as usize..off as usize + 4).map_or(0, |x| u32::from_le_bytes(x.try_into().unwrap()))
    })
}

fn win_w32(h: i32, off: u32, v: u32) {
    WINDOWS.with(|w| {
        let mut w = w.borrow_mut();
        let b = &mut w[h as usize - 1].1;
        if let Some(x) = b.get_mut(off as usize..off as usize + 4) {
            x.copy_from_slice(&v.to_le_bytes());
        }
    })
}

/// The bytes of a memory window, for a check that reads what was written.
pub fn window_bytes(h: i32) -> Vec<u8> {
    WINDOWS.with(|w| w.borrow()[h as usize - 1].1.clone())
}

impl Mmio {
    /// `pages` pages of physical memory at `phys`: a window of zeroes.
    pub fn map_phys(phys: u64, pages: i32) -> Option<Mmio> {
        (pages > 0).then(|| Mmio(window(phys, pages as usize * 4096)))
    }

    /// BAR `bar` of the card: a window of zeroes. BAR2's is the doorbell
    /// page the fake GPU listens on.
    pub fn map_bar(bar: i32, pages: i32) -> Option<Mmio> {
        if pages <= 0 {
            return None;
        }
        let h = window(0, pages as usize * 4096);
        if bar == 2 {
            DOORBELL.with(|d| d.set(h));
        }
        Some(Mmio(h))
    }

    pub fn writeq(self, off: u32, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
        if self.0 != 0 && DOORBELL.with(|d| d.get()) == self.0 {
            if let Some(f) = HOOK.with(|h| h.get()) {
                f(off / 4, v);
            }
        }
    }

    pub fn r32(self, off: u32) -> u32 {
        if self.0 != 0 {
            return win_r32(self.0, off);
        }
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
            let unset = UNSET.with(|u| *u.borrow());
            if let Some((rule, (index, data))) = unset {
                if reg == data {
                    let a = c.regs.get(&index).copied().unwrap_or_else(|| rule(index));
                    return INDIRECT.with(|i| i.borrow().get(&a).copied().unwrap_or_else(|| rule(a)));
                }
                return c.regs.get(&reg).copied().unwrap_or_else(|| rule(reg));
            }
            if let Some((index, data)) = PCIE.with(|p| p.get()) {
                if reg == data {
                    let a = c.regs.get(&index).copied().unwrap_or(0);
                    return INDIRECT.with(|i| i.borrow().get(&a).copied().unwrap_or(0));
                }
            }
            c.regs.get(&reg).copied().unwrap_or(0)
        })
    }

    pub fn w32(self, off: u32, v: u32) {
        if self.0 != 0 {
            return win_w32(self.0, off, v);
        }
        CARD.with(|c| {
            if let Some(c) = c.borrow_mut().as_mut() {
                let reg = off / 4;
                c.writes.push((reg, v));
                if ECHO.with(|e| e.get()) && !ECHO_SKIP.with(|e| e.borrow().contains(&reg)) {
                    println!("write reg {:#x} = {:#x}", reg, v);
                }
                if let Some((rule, (index, data))) = UNSET.with(|u| *u.borrow()) {
                    if reg == data {
                        let a = c.regs.get(&index).copied().unwrap_or_else(|| rule(index));
                        INDIRECT.with(|i| i.borrow_mut().insert(a, v));
                        return;
                    }
                }
                if let Some((index, data)) = PCIE.with(|p| p.get()) {
                    if reg == data {
                        let a = c.regs.get(&index).copied().unwrap_or(0);
                        INDIRECT.with(|i| i.borrow_mut().insert(a, v));
                        return;
                    }
                }
                c.regs.insert(reg, v);
                let answers: Vec<(u32, u32)> =
                    c.on_write.iter().filter(|w| w.0 == reg).map(|w| (w.1, w.2)).collect();
                for (r, val) in answers {
                    c.regs.insert(r, val);
                }
                if c.psp_wptr == Some(reg) {
                    let base = c.regs.get(&(reg + 2)).copied().unwrap_or(0) as u64
                        | (c.regs.get(&(reg + 3)).copied().unwrap_or(0) as u64) << 32;
                    let size_dw = c.regs.get(&(reg + 4)).copied().unwrap_or(0x1000) / 4;
                    let frame = (v + size_dw - 16) % size_dw;
                    fake_psp_fence(base + frame as u64 * 4);
                }
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

/// `npk_config_get`: the environment variable named like the key with
/// `.` as `__` (NPK has no environment; the host check sets it).
pub fn config_get(key: &[u8], buf: &mut [u8]) -> i32 {
    let Ok(key) = std::str::from_utf8(key) else { return -1 };
    match std::env::var(key.replace('.', "__")) {
        Ok(v) if v.len() <= buf.len() => {
            buf[..v.len()].copy_from_slice(v.as_bytes());
            v.len() as i32
        }
        Ok(_) => -1,
        Err(_) => 0,
    }
}

thread_local! {
    static QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether `log` prints; the previous setting.
pub fn quiet(on: bool) -> bool {
    QUIET.with(|q| q.replace(on))
}

pub fn log(b: &[u8]) {
    if QUIET.with(|q| q.get()) {
        return;
    }
    let s = String::from_utf8_lossy(b);
    println!("{}", s.strip_prefix("[gpu_amd_renoir] ").unwrap_or(&s));
}

/// `npk_sys_info`: key 2 is the RAM in MB, as the IdeaPad has it.
pub fn sys_info(key: i32) -> i64 {
    if key == 2 { 15_360 } else { 0 }
}

/// A DMA buffer: a memory window with a made-up bus address.
#[derive(Clone, Copy, Debug)]
pub struct Dma {
    pub handle: i32,
    pub phys: u64,
}

thread_local! {
    static NEXT_BUS: std::cell::Cell<u64> = const { std::cell::Cell::new(0x1_0000_0000) };
}

impl Dma {
    pub fn alloc(pages: i32) -> Option<Dma> {
        if pages <= 0 {
            return None;
        }
        let phys = NEXT_BUS.with(|n| {
            let p = n.get();
            n.set(p + pages as u64 * 4096);
            p
        });
        Some(Dma { handle: window(phys, pages as usize * 4096), phys })
    }

    pub fn free(self) {}

    pub fn r32(self, off: u32) -> u32 {
        win_r32(self.handle, off)
    }

    pub fn w32(self, off: u32, v: u32) {
        win_w32(self.handle, off, v)
    }

    pub fn write(self, off: u32, data: &[u8]) -> bool {
        for (i, c) in data.chunks(4).enumerate() {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            let at = off + i as u32 * 4;
            if c.len() < 4 {
                let old = win_r32(self.handle, at).to_le_bytes();
                w[c.len()..].copy_from_slice(&old[c.len()..]);
            }
            win_w32(self.handle, at, u32::from_le_bytes(w));
        }
        true
    }

    pub fn read(self, off: u32, buf: &mut [u8]) -> bool {
        for (i, c) in buf.chunks_mut(4).enumerate() {
            let w = win_r32(self.handle, off + i as u32 * 4).to_le_bytes();
            let n = c.len();
            c.copy_from_slice(&w[..n]);
        }
        true
    }
}

pub fn now_us() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64)
}

pub fn sleep(_ms: i32) -> i32 {
    0
}

/// `npk_dma_set_mask`: accepted, as the kernel does for 32..=64 bits.
pub fn dma_set_mask(bits: i32) -> i32 {
    if (32..=64).contains(&bits) { 0 } else { -1 }
}

/// `npk_irq_register`: vector 48 for the first entry, as the kernel hands
/// out its first device vector.
pub fn irq_register(_entry: i32) -> i32 {
    48
}

/// `npk_irq_arm`: the fired count, which `irq_wait` compares against.
pub fn irq_arm(_vector: i32) -> i64 {
    IRQ_FIRED.with(|f| f.get())
}

/// `npk_irq_wait`: 1 when the fake card raised an interrupt since `since`;
/// otherwise the wait passes (in an instant: the clock is the host's).
pub fn irq_wait(_vector: i32, since: i64, _timeout_ms: i32) -> i32 {
    if IRQ_FIRED.with(|f| f.get()) != since { 1 } else { 0 }
}

thread_local! {
    static IRQ_FIRED: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
    static DOORBELL: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static HOOK: std::cell::Cell<Option<fn(u32, u64)>> = const { std::cell::Cell::new(None) };
    static PCIE: std::cell::Cell<Option<(u32, u32)>> = const { std::cell::Cell::new(None) };
}

/// The fake GPU: called with the doorbell index and the 64-bit value of
/// every doorbell write.
pub fn fake_doorbell_hook(f: fn(u32, u64)) {
    HOOK.with(|h| h.set(Some(f)));
}

/// The fake GPU raises its interrupt.
pub fn fake_irq() {
    IRQ_FIRED.with(|f| f.set(f.get() + 1));
}

/// The PCIe index/data pair (dword numbers) reaches a register space of
/// its own, preset with `presets`; reads of other addresses there are 0.
pub fn fake_card_pcie(index: u32, data: u32, presets: &[(u32, u32)]) {
    PCIE.with(|p| p.set(Some((index, data))));
    INDIRECT.with(|i| i.borrow_mut().extend(presets.iter().copied()));
}

/// A register as the card holds it, without recording a driver write.
pub fn fake_reg(reg: u32) -> u32 {
    CARD.with(|c| c.borrow().as_ref().and_then(|c| c.regs.get(&reg).copied()).unwrap_or(0))
}

/// The card sets one of its own registers.
pub fn fake_set_reg(reg: u32, v: u32) {
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.regs.insert(reg, v); });
}

/// The DMA window holding bus address `bus`, and the offset into it.
fn bus_window(bus: u64) -> Option<(i32, u32)> {
    WINDOWS.with(|w| {
        let w = w.borrow();
        w.iter().enumerate().find(|(_, (b, v))| *b != 0 && bus >= *b && bus < *b + v.len() as u64)
            .map(|(i, (b, _))| (i as i32 + 1, (bus - b) as u32))
    })
}

/// A dword of system memory, by bus address.
pub fn bus_r32(bus: u64) -> u32 {
    bus_window(bus).map_or(0, |(h, o)| win_r32(h, o))
}

pub fn bus_w32(bus: u64, v: u32) {
    if let Some((h, o)) = bus_window(bus) {
        win_w32(h, o, v);
    }
}

/// A dword of VRAM, by offset.
pub fn vram_r32(off: u64) -> u32 {
    vram_window(off).map_or(0, |h| win_r32(h, off as u32))
}

pub fn vram_w32(off: u64, v: u32) {
    if let Some(h) = vram_window(off) {
        win_w32(h, off as u32, v);
    }
}

/// `npk_pci_enable_bus_master`: granted.
pub fn pci_enable_bus_master() -> i32 {
    0
}

/// The window holding GPU address `addr` (see `FakeCard::psp_wptr`): the
/// largest at bus address 0, the VRAM aperture.
fn vram_window(addr: u64) -> Option<i32> {
    WINDOWS.with(|w| {
        let w = w.borrow();
        w.iter().enumerate().filter(|(_, (bus, b))| *bus == 0 && (addr as usize) < b.len())
            .max_by_key(|(_, (_, b))| b.len()).map(|(i, _)| i as i32 + 1)
    })
}

/// The PSP answering one KM ring frame: the fence value to the fence
/// address. `psp_gfx_rb_frame`: cmd_buf_addr_lo at 0x00, _hi at 0x04,
/// fence_addr_lo at 0x0c, _hi at 0x10, fence_value at 0x14.
fn fake_psp_fence(frame: u64) {
    let Some(h) = vram_window(frame) else { return };
    let dw: Vec<u32> = (0..16).map(|i| win_r32(h, frame as u32 + i * 4)).collect();
    let cmd = dw[0] as u64 | (dw[1] as u64) << 32;
    let cmd_dw: Vec<u32> = match vram_window(cmd) {
        Some(c) => (0..0x100).map(|i| win_r32(c, cmd as u32 + i * 4)).collect(),
        None => Vec::new(),
    };
    PSP_LOG.with(|l| l.borrow_mut().push((dw.clone(), cmd_dw)));
    let lo = dw[3] as u64;
    let hi = dw[4] as u64;
    let value = dw[5];
    let fence = lo | hi << 32;
    if let Some(f) = vram_window(fence) {
        win_w32(f, fence as u32, value);
    }
}

thread_local! {
    static PSP_LOG: RefCell<Vec<(Vec<u32>, Vec<u32>)>> = const { RefCell::new(Vec::new()) };
}

/// Every KM ring frame the fake PSP answered, with the first 1 KiB of the
/// command buffer it names (`struct psp_gfx_cmd_resp`), in order.
pub fn fake_psp_log() -> Vec<(Vec<u32>, Vec<u32>)> {
    PSP_LOG.with(|l| l.borrow().clone())
}

/// The kernel's side of the GPU service on the host: a framebuffer at
/// bus address `fb.0` in the VRAM aperture, two shadows in DMA windows and
/// the copy jobs a check queues. `poll` gives one job a call and, once
/// none is left, -1, as the kernel does for a module that no longer serves.
pub struct FakeGpudev {
    /// Bus address, width, height, pitch.
    pub fb: (u64, u32, u32, u32),
    /// Buffers 1 and 2, the shadows, and 3, a scene source to add.
    pub shadows: [Option<Dma>; 3],
    /// Op, arguments and payload of each job.
    pub jobs: std::collections::VecDeque<(u32, [u32; 6], Vec<u8>)>,
    pub seq: u32,
    pub payloads: std::collections::HashMap<u32, Vec<u8>>,
}

thread_local! {
    static GPUDEV: RefCell<Option<FakeGpudev>> = const { RefCell::new(None) };
}

/// The service's buffers and jobs, before the run.
pub fn fake_gpudev(fb: (u64, u32, u32, u32), jobs: Vec<(u32, [u32; 6], Vec<u8>)>) {
    let pages = ((fb.3 * fb.2) as u64).div_ceil(4096) as i32;
    let shadows = [Dma::alloc(pages), Dma::alloc(pages), Dma::alloc(pages)];
    GPUDEV.with(|g| *g.borrow_mut() = Some(FakeGpudev { fb, shadows, jobs: jobs.into(), seq: 0,
                                                        payloads: Default::default() }));
}

/// Shadow `i` (1 or 2), for a check to fill or read.
pub fn fake_gpudev_shadow(i: usize) -> Option<Dma> {
    GPUDEV.with(|g| g.borrow().as_ref().and_then(|s| s.shadows[i - 1]))
}

pub fn gpudev_register(version: i32) -> i32 {
    if version != 2 {
        return -2;
    }
    if GPUDEV.with(|g| g.borrow().is_some()) { 0 } else { -1 }
}

pub fn gpudev_bo_info(id: i32, out: &mut [u8; 40]) -> i32 {
    GPUDEV.with(|g| {
        let g = g.borrow();
        let Some(s) = g.as_ref() else { return -1 };
        let (fbp, w, h, pitch) = s.fb;
        let size = pitch as u64 * h as u64;
        let phys = match id {
            0 => fbp,
            1..=3 => match s.shadows[id as usize - 1] {
                Some(d) => d.phys,
                None => return -1,
            },
            _ => return -1,
        };
        out[0..8].copy_from_slice(&phys.to_le_bytes());
        out[8..16].copy_from_slice(&size.to_le_bytes());
        let pages = size.div_ceil(4096) as u32;
        for (i, v) in [pitch, w, h, 0x3432_5258, 0, pages].iter().enumerate() {
            out[16 + i * 4..20 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        0
    })
}

pub fn gpudev_bo_pages(id: i32, first: i32, out: &mut [u64]) -> i32 {
    if id > 1000 {
        return TTS.with(|t| {
            let t = t.borrow();
            let Some(Some((_, d, pages))) = t.get((id - 1001) as usize) else { return -1 };
            let n = (*pages - first).clamp(0, out.len() as i32);
            for i in 0..n {
                out[i as usize] = d.phys + (first + i) as u64 * 4096;
            }
            n
        });
    }
    GPUDEV.with(|g| {
        let g = g.borrow();
        let Some(s) = g.as_ref() else { return -1 };
        if !(1..=3).contains(&id) {
            return -1;
        }
        let Some(d) = s.shadows[id as usize - 1] else { return -1 };
        let pages = ((s.fb.3 * s.fb.2) as u64).div_ceil(4096) as i32;
        let mut n = 0;
        for p in first..pages {
            if n as usize == out.len() {
                break;
            }
            out[n as usize] = d.phys + p as u64 * 4096;
            n += 1;
        }
        n
    })
}

pub fn gpudev_bo_bound(id: i32, gpu_addr: i64) -> i32 {
    println!("gpudev bo {} bound at {:#x}", id, gpu_addr);
    0
}

pub fn gpudev_ready(caps: i32) -> i32 {
    println!("gpudev ready caps {}", caps);
    0
}

pub fn gpudev_poll(buf: &mut [u8]) -> i32 {
    GPUDEV.with(|g| {
        let mut g = g.borrow_mut();
        let Some(s) = g.as_mut() else { return -1 };
        let Some((op, args, payload)) = s.jobs.pop_front() else { return -1 };
        s.seq += 1;
        buf[0..4].copy_from_slice(&s.seq.to_le_bytes());
        buf[4..8].copy_from_slice(&op.to_le_bytes());
        for (i, v) in args.iter().enumerate() {
            buf[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        let seq = s.seq;
        s.payloads.insert(seq, payload);
        1
    })
}

/// `npk_gpudev_job_data`: bytes of a job's payload.
pub fn gpudev_job_data(seq: i32, off: i32, out: &mut [u8]) -> i32 {
    GPUDEV.with(|g| {
        let g = g.borrow();
        let Some(p) = g.as_ref().and_then(|s| s.payloads.get(&(seq as u32))) else { return -1 };
        let off = off as usize;
        if off + out.len() > p.len() {
            return -1;
        }
        out.copy_from_slice(&p[off..off + out.len()]);
        out.len() as i32
    })
}

/// `npk_gpudev_tt_alloc`: a guest object's pages, from the fake's DMA.
pub fn gpudev_tt_alloc(client: i32, pages: i32) -> i32 {
    let Some(d) = Dma::alloc(pages) else { return -1 };
    TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.push(Some((client, d, pages)));
        t.len() as i32
    })
}

/// `npk_gpudev_tt_buffer`: the fake reads tt pages under the tt's id.
pub fn gpudev_tt_buffer(tt: i32) -> i32 {
    TTS.with(|t| if t.borrow().get(tt as usize - 1).is_some_and(|x| x.is_some()) { 1000 + tt } else { -1 })
}

pub fn gpudev_tt_free(tt: i32) -> i32 {
    TTS.with(|t| match t.borrow_mut().get_mut(tt as usize - 1).and_then(|x| x.take()) {
        Some((_, d, _)) => {
            d.free();
            0
        }
        None => -1,
    })
}

thread_local! {
    static TTS: core::cell::RefCell<Vec<Option<(i32, Dma, i32)>>> = const { core::cell::RefCell::new(Vec::new()) };
}

/// `npk_gpudev_capset`: the words the guest's Mesa reads.
pub fn gpudev_capset(data: &[u8]) -> i32 {
    let w = |at: usize| u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
    let name: Vec<u8> = data[0x1d0..0x1d0 + 128].iter().copied().take_while(|&b| b != 0).collect();
    println!("gpudev capset {} bytes: drm {}.{}.{}, context type {}, address32_hi {:#x}, vm_always_valid {}, \
              alignments {:#x}/{:#x}, asic {:#x} family {} gb_addr_cfg {:#x}, \"{}\"",
             data.len(), w(4), w(8), w(12), w(16), w(0x18), w(0x1c) & 1, w(0x20), w(0x28), w(0x30), w(0x3c),
             w(0x30 + 0x60), String::from_utf8_lossy(&name));
    0
}

/// `npk_gpudev_reply`: the reply head and the response's ret.
pub fn gpudev_reply(seq: i32, data: &[u8]) -> i32 {
    let w = |at: usize| data.get(at..at + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
    println!("gpudev reply {}: {} bytes, async_error {}, flags {}, rsp len {} ret {} word {:#x}", seq, data.len(), w(0),
             w(4), w(8), w(12) as i32, w(16));
    0
}

pub fn gpudev_complete(seq: i32, status: i32) -> i32 {
    println!("gpudev job {} done: {}", seq, status);
    0
}

/// `npk_wait`: everything the host run waits for is there already.
pub fn wait(_mask: i32, _timeout_ms: i32) -> i32 {
    0
}
