//! The host-visible window of virtio-gpu (`VIRTIO_GPU_SHM_ID_HOST_VISIBLE`,
//! docs/plan/GPU_STACK.md §4.3): host frames of GPU objects mapped straight
//! into guest-physical space, so the guest maps a GPU buffer without a copy
//! (`RESOURCE_MAP_BLOB`). It is as large as the host's memory, rounded up
//! to a power of two, so it fills no sooner than the memory it maps; a BAR
//! lies at a multiple of its size, so the window does too, from 4 GiB on,
//! above guest RAM and the MMIO hole (`configure`). It spans whole PDPT
//! slots of the EPT or NPT.
//!
//! Mapping only adds entries (not present → present) and needs no flush.
//! Unmapping removes them, and every core that ran a vCPU may still hold the
//! old translation. Like KVM's `KVM_REQ_TLB_FLUSH`: `unmap` bumps a
//! generation and kicks every vCPU that is in the guest; each vCPU compares
//! the generation after it published "in guest" and before it enters
//! (`flush_before_entry`), and flushes on a change. `unmap` returns once
//! every such core flushed or left the guest; a core outside the guest
//! flushes before its next entry.
//!
//! An access to an unmapped window page is not guest RAM: reads return 0
//! and writes are dropped (`Hole`, the unassigned-memory behaviour of
//! QEMU's hostmem region). The window's tables go with the EPT/NPT
//! (`release`); the mapped frames belong to the GPU objects.

use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use super::guest_mem::SecondLevel;

const PAGE: u64 = 4096;
const ONE_GB: u64 = 1 << 30;
const FOUR_GB: u64 = 4 << 30;

/// Where the window lies and how large it is, set at `vm_open`.
static WINDOW_GPA: AtomicU64 = AtomicU64::new(FOUR_GB);
static WINDOW_SIZE: AtomicU64 = AtomicU64::new(ONE_GB);

/// Size and place the window for the next VM: the host's memory top,
/// rounded up to a power of two and at least 1 GiB, at a multiple of that
/// from 4 GiB on, inside the guest's physical address width.
pub fn configure() {
    let phys_bits = crate::microvm::cpu::guest_phys_bits();
    let limit = 1u64 << phys_bits;
    let mut size = crate::memory::memory_top_bytes().max(ONE_GB).next_power_of_two();
    while size > ONE_GB && size.max(FOUR_GB) + size > limit {
        size /= 2;
    }
    WINDOW_SIZE.store(size, Ordering::Release);
    WINDOW_GPA.store(size.max(FOUR_GB), Ordering::Release);
}

/// Guest-physical base of the window.
pub fn gpa() -> u64 {
    WINDOW_GPA.load(Ordering::Acquire)
}

/// Size of the window in bytes.
pub fn size() -> u64 {
    WINDOW_SIZE.load(Ordering::Acquire)
}

/// The PDPT slots the window spans.
pub fn pdpt_slots() -> core::ops::Range<usize> {
    (gpa() / ONE_GB) as usize..((gpa() + size()) / ONE_GB) as usize
}
/// Host cores whose "in guest" state is tracked (`cpu::in_guest`).
const CORES: usize = 32;

/// How a window page is cached.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cache {
    WriteBack,
    WriteCombining,
    Uncached,
}

/// Serializes window changes.
static LOCK: Mutex<()> = Mutex::new(());
/// Bumped by every `unmap` that removed a mapping.
static GEN: AtomicU64 = AtomicU64::new(0);
/// Per host core: the generation its translations reflect.
static SEEN: [AtomicU64; CORES] = [const { AtomicU64::new(0) }; CORES];

pub fn contains(gpa: u64) -> bool {
    (self::gpa()..self::gpa() + size()).contains(&gpa)
}

fn root() -> Option<(SecondLevel, u64)> {
    super::guest_mem::active().map(|gm| gm.second_level())
}

/// Map the host frames `host_phys` (page-aligned) at window offset
/// `gpa_off`, one per page. Fails without changes beyond those already
/// made if a page is mapped already; the caller unmaps what it mapped.
pub fn map(gpa_off: u64, host_phys: &[u64], cache: Cache) -> Result<(), &'static str> {
    let len = host_phys.len() as u64 * PAGE;
    if gpa_off % PAGE != 0 || gpa_off.checked_add(len).is_none_or(|e| e > size()) {
        return Err("outside the host-visible window");
    }
    if host_phys.iter().any(|p| p % PAGE != 0) {
        return Err("host frame not page-aligned");
    }
    let (sl, pml4) = root().ok_or("no microVM running")?;
    let _g = LOCK.lock();
    for (i, &host) in host_phys.iter().enumerate() {
        let gpa = self::gpa() + gpa_off + i as u64 * PAGE;
        match sl {
            SecondLevel::Ept => {
                use crate::microvm::cpu::vmx::ept;
                let t = match cache {
                    Cache::WriteBack => ept::MEMTYPE_WB,
                    Cache::WriteCombining => ept::MEMTYPE_WC,
                    Cache::Uncached => ept::MEMTYPE_UC,
                };
                ept::window_map(pml4, gpa, host, t)?;
            }
            SecondLevel::Npt => {
                use crate::microvm::cpu::svm::npt;
                let c = match cache {
                    Cache::WriteBack => npt::CACHE_WB,
                    Cache::WriteCombining => npt::CACHE_WC,
                    Cache::Uncached => npt::CACHE_UC,
                };
                npt::window_map(pml4, gpa, host, c)?;
            }
        }
    }
    Ok(())
}

/// Unmap `pages` pages at window offset `gpa_off`. When it returns, no vCPU
/// can reach the frames through the window any more.
pub fn unmap(gpa_off: u64, pages: usize) {
    let Some((sl, pml4)) = root() else { return };
    let _g = LOCK.lock();
    let mut removed = false;
    for i in 0..pages as u64 {
        let off = gpa_off + i * PAGE;
        if off >= size() {
            break;
        }
        let gpa = self::gpa() + off;
        removed |= match sl {
            SecondLevel::Ept => crate::microvm::cpu::vmx::ept::window_unmap(pml4, gpa),
            SecondLevel::Npt => crate::microvm::cpu::svm::npt::window_unmap(pml4, gpa),
        };
    }
    if removed {
        flush_all_vcpus(true);
    }
}

/// Whether window page `gpa` is mapped (a violation that raced a `map`).
pub fn is_mapped(gpa: u64) -> bool {
    let Some((sl, pml4)) = root() else { return false };
    let _g = LOCK.lock();
    let page = gpa & !(PAGE - 1);
    match sl {
        SecondLevel::Ept => crate::microvm::cpu::vmx::ept::window_mapped(pml4, page),
        SecondLevel::Npt => crate::microvm::cpu::svm::npt::window_mapped(pml4, page),
    }
}

/// After second-level entries of guest RAM were removed: no vCPU uses
/// them any more once this returns (`flush_all_vcpus`). Waits without
/// yielding, for callers that hold the VM's device state.
pub fn flush_guest_translations() {
    flush_all_vcpus(false);
}

/// Bump the generation and wait until every core in the guest has flushed
/// or left it. Store, then look (`cpu::entry_irqs_off` stores "in guest",
/// then looks here): either the vCPU sees the new generation before it
/// enters, or this sees it in the guest and kicks it out. A kicked vCPU
/// leaves within microseconds; `yielding` lets the core's other fibers run
/// meanwhile.
fn flush_all_vcpus(yielding: bool) {
    let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let hz = crate::interrupts::tsc_freq();
    let dl_warn = crate::interrupts::rdtsc() + hz;
    let mut warned = false;
    // A core is kicked once per millisecond it stays in the guest.
    let mut kicked = [0u64; CORES];
    loop {
        let mut pending = false;
        let now = crate::interrupts::rdtsc();
        for c in 0..CORES {
            if crate::microvm::cpu::in_guest(c) && SEEN[c].load(Ordering::SeqCst) < generation {
                pending = true;
                if now >= kicked[c] {
                    kicked[c] = now + hz / 1000;
                    crate::smp::kick_host_core(c);
                }
            }
        }
        if !pending {
            return;
        }
        if !warned && crate::interrupts::rdtsc() > dl_warn {
            warned = true;
            crate::kprintln!("[microvm] hostmem: a vCPU has not left the guest for 1 s after an unmap");
        }
        if yielding {
            crate::smp::fiber::spin_or_yield();
        } else {
            core::hint::spin_loop();
        }
    }
}

/// Called by a vCPU after it published "in guest", with interrupts off,
/// before it enters: true when the window lost mappings since this core's
/// last flush, and the caller must flush the guest's second-level
/// translations on this core before entry (VMX: INVEPT, SVM: TLB_CONTROL).
pub fn flush_before_entry(host_core: usize) -> bool {
    let generation = GEN.load(Ordering::SeqCst);
    if generation == 0 {
        return false;
    }
    let Some(seen) = SEEN.get(host_core) else { return true };
    if seen.load(Ordering::Relaxed) == generation {
        return false;
    }
    seen.store(generation, Ordering::SeqCst);
    true
}

/// An unmapped window page: reads return 0, writes are dropped.
pub struct Hole;

impl super::MmioDevice for Hole {
    fn bar0_base(&self) -> u64 { gpa() }
    fn mmio_read(&mut self, _off: u32, _width: u8) -> u64 { 0 }
    fn mmio_write(&mut self, _off: u32, _width: u8, _value: u64) {}
    fn take_pending_kick(&mut self) -> Option<u16> { None }
    fn service_queues(&mut self, _queue: u16, _mem: &super::guest_mem::GuestMem) -> bool { false }
    fn irq_line(&self) -> u8 { 0 }
}
