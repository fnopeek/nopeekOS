//! Address space and mappings for compiled modules.
//!
//! Linear memory gets a reservation of 8 GiB plus a page, of which only the
//! pages that exist are mapped. A wasm address is a u32 and a memory offset is
//! a u32, so no access can reach past that range, which is why the generator
//! emits no bounds check. The spare page is needed: the highest reachable
//! address is `2^33-2`, and an eight-byte access there reaches `2^33+5`.
//!
//! Code is mapped W^X: writable while it is being filled, executable
//! afterwards, never both.
//!
//! Everything lives above the identity-mapped first 64 GB, where nothing else
//! claims addresses.

use crate::mm::paging::{self, PageFlags};
use crate::memory;

/// First address above the identity map. Below this everything is mapped 1:1
/// through 1 GB huge pages, so this is where free address space starts.
const REGION_BASE: u64 = 64 * 1024 * 1024 * 1024;

/// What one instance reserves. A power of two well above `8 GiB + page` keeps
/// the arithmetic to a shift and leaves the next instance nowhere near.
const INSTANCE_STRIDE: u64 = 16 * 1024 * 1024 * 1024;

/// The readable part may never exceed this — the reservation minus the slack
/// an eight-byte access at the very top needs.
pub const MAX_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024 + 0x1000;

/// How much RAM one module may actually occupy.
///
/// `MAX_MEMORY_BYTES` is the address reservation, not a cap on RAM. Without
/// this limit a module could grow until the machine is empty and the kernel
/// fails its own next allocation. A module that wants too much must fail, not
/// the machine: the machine's memory is outside the sandbox. The value is set
/// well above the normal working set of the hungriest module.
pub const MAX_INSTANCE_BYTES: u64 = 1024 * 1024 * 1024;

/// RAM the kernel keeps for itself. Without it, the last frame could go to a
/// module and the kernel could not even log the refusal.
pub const KERNEL_RESERVE_MB: usize = 96;

const PAGE: u64 = 4096;

/// Number of instance slots. A slot is `INSTANCE_STRIDE` wide and code starts
/// above them; slot 1024 would land exactly on `CODE_BASE`.
const MAX_SLOTS: u64 = 1024;

/// High-water mark of handed-out slots; `SLOT_FREE` applies below it.
static NEXT_SLOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Returned slots, one bit per slot (set = free). A bitmap rather than a list
/// because slots are returned in `Drop`, which must not allocate.
static SLOT_FREE: [core::sync::atomic::AtomicU64; (MAX_SLOTS / 64) as usize] =
    [const { core::sync::atomic::AtomicU64::new(0) }; (MAX_SLOTS / 64) as usize];

/// Take a slot: a returned one first, otherwise the next fresh one.
fn take_slot() -> Option<u64> {
    use core::sync::atomic::Ordering::{AcqRel, Relaxed};
    for (w, cell) in SLOT_FREE.iter().enumerate() {
        loop {
            let bits = cell.load(Relaxed);
            if bits == 0 { break; }
            let b = bits.trailing_zeros() as u64;
            if cell.compare_exchange(bits, bits & !(1 << b), AcqRel, Relaxed).is_ok() {
                return Some(w as u64 * 64 + b);
            }
        }
    }
    let s = NEXT_SLOT.fetch_add(1, Relaxed);
    if s >= MAX_SLOTS {
        NEXT_SLOT.store(MAX_SLOTS, Relaxed);
        return None;
    }
    Some(s)
}

/// Return a slot.
fn give_slot(s: u64) {
    if s >= MAX_SLOTS { return; }
    SLOT_FREE[(s / 64) as usize]
        .fetch_or(1 << (s % 64), core::sync::atomic::Ordering::Release);
}

/// Code goes at the top of the region, far from any instance's memory.
const CODE_BASE: u64 = REGION_BASE + 1024 * INSTANCE_STRIDE;
static NEXT_CODE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(CODE_BASE);

pub struct Memory {
    /// Slot in the region; returned when the instance goes away.
    slot: u64,
    pub base: u64,
    /// Bytes currently readable.
    pub size: u64,
}

/// Map `bytes` of fresh, zeroed pages at `at`.
fn map_range(at: u64, bytes: u64, flags: PageFlags) -> bool {
    let mut off = 0;
    while off < bytes {
        let Some(frame) = memory::allocate_frame() else {
            // Release the part already mapped, so running out of memory
            // does not leak memory too.
            unmap_range(at, off);
            return false;
        };
        // SAFETY: the frame allocator just handed this out and the first
        // 64 GB are identity-mapped, so the frame is addressable here.
        unsafe { core::ptr::write_bytes(frame as *mut u8, 0, PAGE as usize) };
        if paging::map_page(at + off, frame, flags).is_err() {
            memory::deallocate_frame(frame);
            unmap_range(at, off);
            return false;
        }
        off += PAGE;
    }
    true
}

/// Unmap a range and return its frames. Counterpart of `map_range`.
fn unmap_range(at: u64, bytes: u64) {
    let mut off = 0;
    while off < bytes {
        if let Ok(frame) = paging::unmap_page(at + off) {
            memory::deallocate_frame(frame);
        }
        off += PAGE;
    }
}

impl Memory {
    /// Reserve an instance's address space and map its initial pages.
    ///
    /// Only the mapping is done here — the rest of the 8 GiB stays absent, and
    /// that absence is the bounds check.
    pub fn new(initial_pages: u64) -> Option<Memory> {
        let slot = take_slot()?;
        let base = REGION_BASE + slot * INSTANCE_STRIDE;
        let size = initial_pages * 65536;
        if size > MAX_MEMORY_BYTES {
            give_slot(slot);
            return None;
        }
        let flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        if !map_range(base, size, flags) {
            give_slot(slot);
            return None;
        }
        Some(Memory { base, size, slot })
    }

    // Growth happens only through `forge_rt::grow`, which writes the vmctx;
    // `Drop for Instance` copies the current size back before this drops.

    /// Does `addr` fall inside this instance's reservation? What a page-fault
    /// handler asks to tell a module's mistake from a kernel's.
    pub fn owns(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.base + INSTANCE_STRIDE
    }
}

impl Drop for Memory {
    /// Returns as many pages as `self.size` says. `memory.grow` writes the new
    /// size to the vmctx, not here; `Drop for Instance` brings it up to date.
    fn drop(&mut self) {
        unmap_range(self.base, self.size);
        give_slot(self.slot);
    }
}

/// A module's code, mapped executable and not writable.
pub struct Code {
    pub base: u64,
    pub len: usize,
}

impl Code {
    /// Copy `bytes` into fresh pages and flip them to execute-only-ish
    /// (readable and executable, not writable). Writable and executable are
    /// never true at the same time.
    pub fn map(bytes: &[u8]) -> Option<Code> {
        let len = bytes.len();
        let span = ((len as u64) + PAGE - 1) & !(PAGE - 1);
        let base = NEXT_CODE.fetch_add(span.max(PAGE), core::sync::atomic::Ordering::Relaxed);

        // Writable first, so it can be filled.
        let rw = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
        if !map_range(base, span, rw) {
            return None;
        }
        // SAFETY: `span` bytes were just mapped writable at `base`.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), base as *mut u8, len) };

        // Then executable, and no longer writable.
        let rx = PageFlags::PRESENT;
        let mut off = 0;
        while off < span {
            let Ok(frame) = paging::unmap_page(base + off) else {
                unmap_range(base, span);
                return None;
            };
            if paging::map_page(base + off, frame, rx).is_err() {
                memory::deallocate_frame(frame);
                unmap_range(base, span);
                return None;
            }
            off += PAGE;
        }
        Some(Code { base, len })
    }

    pub fn owns(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.base + self.len as u64
    }

    /// Bytes actually mapped; `map` rounds up to whole pages.
    fn span(&self) -> u64 { ((self.len as u64) + PAGE - 1) & !(PAGE - 1) }
}

impl Drop for Code {
    /// Frees the frames. The address range is not reused (`NEXT_CODE` only
    /// moves forward); address space is plentiful, frames are not.
    fn drop(&mut self) {
        unmap_range(self.base, self.span());
    }
}

// ── faults ────────────────────────────────────────────────────────────
//
// A page fault from a guard page and a divide fault are how two of the traps
// arrive: they are the processor telling us what a check on every single
// access would otherwise have had to look for. Catching them means pointing
// the interrupted instruction pointer at the module's entry for that reason —
// and because the entry names the reason itself, no general register has to be
// touched. That is the whole reason the entries exist per reason.
//
// The stubs below are the smallest thing that can do it: one register saved,
// two comparisons, and either a redirect or a jump to the handler that was
// there before. A fault outside a module's code is still a kernel fault.

#[unsafe(no_mangle)]
pub static FORGE_CODE_LO: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(u64::MAX);
#[unsafe(no_mangle)]
pub static FORGE_CODE_HI: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static FORGE_PF_ENTRY: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static FORGE_DE_ENTRY: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Which code may fault, and where its faults go. Cleared with `disarm`.
pub fn arm_faults(code: &Code, pf_entry: usize, de_entry: usize) {
    use core::sync::atomic::Ordering;
    FORGE_PF_ENTRY.store(code.base + pf_entry as u64, Ordering::SeqCst);
    FORGE_DE_ENTRY.store(code.base + de_entry as u64, Ordering::SeqCst);
    FORGE_CODE_LO.store(code.base, Ordering::SeqCst);
    FORGE_CODE_HI.store(code.base + code.len as u64, Ordering::SeqCst);
}

pub fn disarm_faults() {
    use core::sync::atomic::Ordering;
    FORGE_CODE_LO.store(u64::MAX, Ordering::SeqCst);
    FORGE_CODE_HI.store(0, Ordering::SeqCst);
}

// The saved instruction pointer sits one word above our own push, plus another
// word for #PF's error code. Nothing but `rax` is touched, and `iretq` puts
// the flags back from the frame.
core::arch::global_asm!(
    r#"
.globl forge_pf_stub
forge_pf_stub:
    push rax
    mov  rax, [rip + FORGE_CODE_LO]
    cmp  qword ptr [rsp + 16], rax
    jb   1f
    mov  rax, [rip + FORGE_CODE_HI]
    cmp  qword ptr [rsp + 16], rax
    jae  1f
    mov  rax, [rip + FORGE_PF_ENTRY]
    mov  [rsp + 16], rax
    pop  rax
    add  rsp, 8
    iretq
1:  pop  rax
    jmp  page_fault_handler

.globl forge_de_stub
forge_de_stub:
    push rax
    mov  rax, [rip + FORGE_CODE_LO]
    cmp  qword ptr [rsp + 8], rax
    jb   2f
    mov  rax, [rip + FORGE_CODE_HI]
    cmp  qword ptr [rsp + 8], rax
    jae  2f
    mov  rax, [rip + FORGE_DE_ENTRY]
    mov  [rsp + 8], rax
    pop  rax
    iretq
2:  pop  rax
    jmp  divide_error_handler
"#
);

unsafe extern "C" {
    pub fn forge_pf_stub();
    pub fn forge_de_stub();
}

// ── trap from inside a host function ──────────────────────────────────
//
// A wasi function like `proc_exit` must not return, even on a clean exit.
// Generated code unwinds the same way its own traps do: restore `rsp`/`rbp`
// from the vmctx and jump back to the entry, dropping any depth of wasm
// frames. Only the vmctx pointer is needed, and every host function has it in
// `rdi`, so r14 is not touched.
core::arch::global_asm!(
    r#"
.globl forge_host_trap
forge_host_trap:
    mov  [rdi + {tc}], rsi
    mov  rbp, [rdi + {rbp}]
    mov  rsp, [rdi + {rsp}]
    jmp  [rdi + {res}]
"#,
    tc = const vmctx::TRAP_CODE,
    rbp = const vmctx::TRAP_RBP,
    rsp = const vmctx::TRAP_RSP,
    res = const vmctx::TRAP_RESUME,
);

unsafe extern "C" {
    /// Leaves the module with `code` as trap reason. Never returns.
    fn forge_host_trap(vm: *const u64, code: u64) -> !;
}

/// End the module run from inside a host function.
///
/// # Safety
/// `vm` must be the vmctx of the instance currently running, i.e. the pointer
/// the adapter received as its first argument; any other vmctx jumps into a
/// frame that no longer exists.
///
/// The caller must hold nothing that needs cleanup: no Rust frame is unwound
/// and no `Drop` runs. The `#PF` stub relies on the same guarantee.
pub unsafe fn host_trap(vm: *const u64, code: u32) -> ! {
    // SAFETY: forwarded to the caller, see above.
    unsafe { forge_host_trap(vm, code as u64) }
}



// ── instances ─────────────────────────────────────────────────────────

use alloc::vec::Vec;
use forge_core::{vmctx, CompiledModule};

/// `memory.grow`, the one runtime routine generated code calls. It never moves
/// the base — the reservation is already there, and only its readable part
/// changes. Generated code depends on that, which is why nothing reloads the
/// memory register after a call.
extern "C" fn grow(ctx: *mut u64, delta: u32) -> u32 {
    // SAFETY: `ctx` is the instance context of the module doing the call,
    // laid out by `forge_core::vmctx`.
    unsafe {
        let base = *ctx.add(vmctx::MEM_BASE as usize / 8);
        let size = *ctx.add(vmctx::MEM_SIZE as usize / 8);
        let max_pages = *ctx.add(vmctx::MEM_MAX_PAGES as usize / 8);
        let old_pages = size / 65536;
        let Some(new_pages) = old_pages.checked_add(delta as u64) else {
            return u32::MAX;
        };
        if new_pages > max_pages || new_pages * 65536 > MAX_MEMORY_BYTES {
            crate::kprintln!(
                "[npk] forge: memory.grow abgelehnt — Deckel erreicht ({} von hoechstens {} Seiten)",
                new_pages, max_pages);
            return u32::MAX;
        }
        // The per-module cap, then the machine-wide reserve. Both refuse the
        // module rather than panic the kernel.
        if new_pages * 65536 > MAX_INSTANCE_BYTES {
            crate::kprintln!(
                "[npk] forge: memory.grow abgelehnt — Modul-Deckel erreicht ({} MB von hoechstens {} MB)",
                new_pages * 65536 / (1024 * 1024), MAX_INSTANCE_BYTES / (1024 * 1024));
            return u32::MAX;
        }
        let (_, free_mb) = crate::memory::stats();
        let want_mb = (delta as usize * 65536).div_ceil(1024 * 1024);
        if free_mb < KERNEL_RESERVE_MB + want_mb {
            crate::kprintln!(
                "[npk] forge: memory.grow abgelehnt — Maschine fast leer ({} MB frei, {} MB Reserve, {} MB gefragt)",
                free_mb, KERNEL_RESERVE_MB, want_mb);
            return u32::MAX;
        }
        if delta > 0 {
            let flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
            if !map_range(base + size, delta as u64 * 65536, flags) {
                // The module only sees that `memory.grow` failed; whether the
                // machine is out of frames or the mapping failed is only
                // visible here, so log it.
                let (frames, mb) = crate::memory::stats();
                crate::kprintln!(
                    "[npk] forge: memory.grow abgelehnt — {} Seiten gefragt, {} MB frei ({} Rahmen)",
                    delta, mb, frames);
                return u32::MAX;
            }
        }
        *ctx.add(vmctx::MEM_SIZE as usize / 8) = new_pages * 65536;
        old_pages as u32
    }
}


/// What an embedder has to answer so a compiled module can call out.
///
/// Deliberately two questions and no types: `forge_rt` stays free of anything
/// npk-specific, and the table it fills is plain addresses. The npk side of
/// this lives in `wasm::forge_glue`.
pub trait HostImports {
    /// The embedder state a host function will be handed, as a raw address.
    /// Parked in the vmctx, so two modules on two cores never share one.
    fn ctx_ptr(&self) -> u64;
    /// Address of the routine for one import, or `None` to leave it trapping.
    fn resolve(&self, module: &str, name: &str) -> Option<u64>;
}

/// A module made ready to run: its code mapped, its memory reserved, and the
/// context generated code reaches everything else through.
pub struct Instance {
    /// Imports still pointing at the trap stub. Zero means the module can
    /// actually run; anything else means it will stop at the first call out.
    unresolved: u32,
    ctx: Vec<u64>,
    _globals: Vec<u64>,
    _table: Vec<u64>,
    _table_sigs: Vec<u32>,
    _host_fns: Vec<u64>,
    memory: Option<Memory>,
    code: Code,
    entry: usize,
    pf_entry: usize,
    de_entry: usize,
}

impl Instance {
    /// Without a host: every import lands on the trap stub. That is what the
    /// selftest cases want — they import nothing.
    pub fn new(m: &CompiledModule) -> Option<Instance> {
        Self::build(m, None)
    }

    /// With a host: imports the embedder knows get its addresses, the rest
    /// keep trapping. A module is never half-wired without saying so —
    /// `unresolved_imports` counts what stayed on the stub.
    ///
    pub fn new_with_host(m: &CompiledModule, host: &dyn HostImports) -> Option<Instance> {
        Self::build(m, Some(host))
    }

    fn build(m: &CompiledModule, host: Option<&dyn HostImports>) -> Option<Instance> {
        let code = Code::map(&m.code)?;
        let trap_stub = code.base + m.trap_offset as u64;

        let pages = m.plan.memory.map(|(min, _)| min).unwrap_or(0);
        let memory = if m.plan.memory.is_some() {
            Some(Memory::new(pages)?)
        } else {
            None
        };
        if let Some(mem) = &memory {
            for (off, bytes) in &m.plan.data_init {
                let end = *off as u64 + bytes.len() as u64;
                if end > mem.size {
                    return None;
                }
                // SAFETY: the range was just checked against the mapped size.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (mem.base + *off as u64) as *mut u8,
                        bytes.len(),
                    )
                };
            }
        }

        let imported_globals = m
            .plan
            .global_types
            .len()
            .saturating_sub(m.plan.global_init.len());
        let mut globals: Vec<u64> = Vec::new();
        globals.resize(imported_globals, 0);
        for g in &m.plan.global_init {
            globals.push(g.unwrap_or(0) as u64);
        }
        globals.push(0); // never hand out a null base

        let slots = m.plan.table.map(|(min, _)| min as usize).unwrap_or(0);
        let mut table: Vec<u64> = Vec::new();
        let mut table_sigs: Vec<u32> = Vec::new();
        table.resize(slots.max(1), trap_stub);
        table_sigs.resize(slots.max(1), u32::MAX);
        for (off, funcs) in &m.plan.elem_init {
            for (i, fi) in funcs.iter().enumerate() {
                let slot = *off as usize + i;
                if slot >= table.len() {
                    return None;
                }
                table[slot] = match m.offset_of(*fi) {
                    Some(o) => code.base + o as u64,
                    None => trap_stub,
                };
                let ti = *m.plan.func_type_of.get(*fi as usize)? as usize;
                table_sigs[slot] = *m.plan.sig_id.get(ti)?;
            }
        }

        // An import the embedder does not know keeps the trap stub, so a module
        // that needs one says so instead of jumping somewhere arbitrary.
        let mut host_fns: Vec<u64> = Vec::new();
        host_fns.resize(m.plan.imported_funcs.len().max(1), trap_stub);
        let mut unresolved = 0u32;
        for (i, (module, name)) in m.plan.imported_funcs.iter().enumerate() {
            match host.and_then(|h| h.resolve(module, name)) {
                Some(addr) => host_fns[i] = addr,
                None => unresolved += 1,
            }
        }

        let mut ctx: Vec<u64> = Vec::new();
        ctx.resize(vmctx::SIZE / 8, 0);
        if let Some(mem) = &memory {
            ctx[vmctx::MEM_BASE as usize / 8] = mem.base;
            ctx[vmctx::MEM_SIZE as usize / 8] = mem.size;
        }
        ctx[vmctx::MEM_MAX_PAGES as usize / 8] =
            m.plan.memory.and_then(|(_, mx)| mx).unwrap_or(65536);
        ctx[vmctx::GLOBALS as usize / 8] = globals.as_ptr() as u64;
        ctx[vmctx::TABLE as usize / 8] = table.as_ptr() as u64;
        ctx[vmctx::TABLE_LEN as usize / 8] = slots as u64;
        ctx[vmctx::TABLE_SIGS as usize / 8] = table_sigs.as_ptr() as u64;
        ctx[vmctx::HOST_FNS as usize / 8] = host_fns.as_ptr() as u64;
        ctx[vmctx::BUILTIN_GROW as usize / 8] = grow as usize as u64;
        ctx[vmctx::HOST_CTX as usize / 8] = host.map(|h| h.ctx_ptr()).unwrap_or(0);

        Some(Instance {
            unresolved,
            ctx,
            _globals: globals,
            _table: table,
            _table_sigs: table_sigs,
            _host_fns: host_fns,
            memory,
            code,
            entry: m.entry_offset,
            pf_entry: m.pf_entry,
            de_entry: m.de_entry,
        })
    }

    pub fn unresolved_imports(&self) -> u32 {
        self.unresolved
    }

    /// Current linear memory size in bytes, as held in the vmctx. Changes with
    /// `memory.grow`.
    pub fn memory_size(&self) -> u64 {
        self.ctx[vmctx::MEM_SIZE as usize / 8]
    }

    pub fn set_fuel(&mut self, v: i64) {
        self.ctx[vmctx::FUEL as usize / 8] = v as u64;
    }

    pub fn trap_code(&self) -> u32 {
        self.ctx[vmctx::TRAP_CODE as usize / 8] as u32
    }

    /// Enter the module at `off` and come back with what it produced and what
    /// stopped it. Faults from this code are claimed for the duration and
    /// released again — outside that window a page fault is the kernel's own.
    pub fn call(&mut self, off: usize, a: u32, b: u32, c: u32) -> (u32, u32) {
        arm_faults(&self.code, self.pf_entry, self.de_entry);
        let entry = self.code.base + self.entry as u64;
        let target = self.code.base + off as u64;
        // SAFETY: both addresses come from the module's own tables, the code
        // is mapped executable, and the trampoline's shape is fixed by
        // `forge_core::codegen::emit_entry`.
        let r = unsafe {
            let f: extern "C" fn(*const u64, *const u8, u32, u32, u32) -> u32 =
                core::mem::transmute(entry as *const ());
            f(self.ctx.as_ptr(), target as *const u8, a, b, c)
        };
        disarm_faults();
        (r, self.trap_code())
    }
}

impl Drop for Instance {
    /// Pages added by `memory.grow` must be freed too. Generated code grows
    /// through `forge_rt::grow`, which updates only the vmctx, so the current
    /// size is copied into `Memory` before it drops.
    fn drop(&mut self) {
        if let Some(m) = &mut self.memory {
            // Never below the initial size and never past the reservation:
            // `unmap_range` walks page by page, and too large a value would
            // reach into the next instance's slot.
            let now = self.ctx[vmctx::MEM_SIZE as usize / 8];
            let start = m.size;
            m.size = now.clamp(start, MAX_MEMORY_BYTES);
            // Log the rare grown case once, with sizes.
            if m.size > start {
                crate::kdebug!("[npk] forge: instance returns {} MB ({} MB grown)",
                                 m.size / (1024 * 1024), (m.size - start) / (1024 * 1024));
            }
        }
    }
}
