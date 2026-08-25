//! Differential test: the same function under wasmi and under forge, same
//! arguments, and the results must be identical. wasmi is the oracle because
//! it is what the device runs today — a disagreement is a real disagreement,
//! not a spec argument.
//!
//! The generated code is mapped W^X (write, then flip to execute) the way the
//! kernel will have to map it. Doing it right here keeps the harness honest
//! about what the real thing costs.

use std::ffi::c_void;

/// A mapped, executable code page. Dropping it unmaps.
pub(crate) struct Exec {
    ptr: *mut c_void,
    len: usize,
}

impl Exec {
    pub(crate) fn new(code: &[u8]) -> Option<Exec> {
        if code.is_empty() {
            return None;
        }
        let len = (code.len() + 4095) & !4095;
        // SAFETY: anonymous private mapping, size is page-rounded and nonzero.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        // SAFETY: `ptr` is a fresh writable mapping of at least `code.len()`.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), ptr as *mut u8, code.len()) };
        // W^X: writable while filling, executable afterwards, never both.
        // SAFETY: same mapping, same length.
        if unsafe { libc::mprotect(ptr, len, libc::PROT_READ | libc::PROT_EXEC) } != 0 {
            // SAFETY: unmapping the mapping we just made.
            unsafe { libc::munmap(ptr, len) };
            return None;
        }
        Some(Exec { ptr, len })
    }

    pub(crate) fn base(&self) -> u64 {
        self.ptr as u64
    }

    /// Let the fault handler know this code may trap, and where its traps go.
    pub(crate) fn arm_traps(&self, m: &forge_core::CompiledModule) {
        crate::faults::arm(
            self.base(),
            self.len,
            self.base() + m.pf_entry as u64,
            self.base() + m.de_entry as u64,
        );
    }

    /// Enter the module. Generated functions expect the instance already set
    /// up in their pinned registers, so a native caller goes in through the
    /// module's trampoline rather than jumping at a function directly.
    pub(crate) fn call_entry(
        &self,
        entry: usize,
        off: usize,
        ctx: *const u64,
        a: u32,
        b: u32,
        c: u32,
    ) -> u32 {
        // SAFETY: both offsets come from the module's own tables, and the
        // trampoline's shape is fixed by `codegen::emit_entry`.
        unsafe {
            let base = self.ptr as *const u8;
            let f: extern "C" fn(*const u64, *const u8, u32, u32, u32) -> u32 =
                std::mem::transmute(base.add(entry));
            f(ctx, base.add(off), a, b, c)
        }
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        // SAFETY: our own mapping, unmapped once.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

/// One wasm page.
pub(crate) const PAGE: usize = 64 * 1024;
/// A budget far above anything the cases need, so the run finishes and what
/// it actually cost is what gets reported.
pub(crate) const DEFAULT_FUEL: i64 = i64::MAX / 4;
/// Address space reserved per instance, and the number has to be exact.
///
/// A wasm address is a u32 and a memory offset is a u32, so the highest
/// effective address is `2^32-1 + 2^32-1 = 2^33-2` — just under 8 GiB. But the
/// ACCESS still has a width: an eight-byte load there reaches `2^33+5`. Eight
/// gibibytes on the nose would therefore leave the last few bytes of the
/// widest access hanging outside the reservation, which is precisely the hole
/// an attacker would look for. One spare page covers every access width.
pub(crate) const RESERVATION: usize = 8 * 1024 * 1024 * 1024 + PAGE;

/// Linear memory: 8 GiB of address space, of which only `size` bytes are
/// readable. Costs no physical memory — `PROT_NONE` pages are never backed.
pub(crate) struct Memory {
    pub(crate) base: *mut u8,
    pub(crate) size: usize,
}

impl Memory {
    pub(crate) fn new(pages: usize) -> Option<Memory> {
        let size = pages.max(1) * PAGE;
        // SAFETY: a fresh anonymous reservation, no backing requested.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                RESERVATION,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return None;
        }
        // SAFETY: the first `size` bytes of our own reservation.
        if unsafe { libc::mprotect(base, size, libc::PROT_READ | libc::PROT_WRITE) } != 0 {
            // SAFETY: unmapping the reservation we just made.
            unsafe { libc::munmap(base, RESERVATION) };
            return None;
        }
        Some(Memory { base: base as *mut u8, size })
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: our own reservation, unmapped once.
        unsafe { libc::munmap(self.base as *mut libc::c_void, RESERVATION) };
    }
}

/// The instance context generated code reads through its pinned register,
/// laid out by `forge_core::vmctx`. The backing buffers live here so their
/// addresses stay valid for the call — a `Vec`'s heap block does not move
/// when the `Vec` does, so taking the pointer before the move is sound.
pub(crate) struct Inst {
    ctx: Vec<u64>,
    #[allow(dead_code)]
    globals: Vec<u64>,
    #[allow(dead_code)]
    memory: Memory,
    #[allow(dead_code)]
    table: Vec<u64>,
    #[allow(dead_code)]
    table_sigs: Vec<u32>,
    #[allow(dead_code)]
    host_fns: Vec<u64>,
}

impl Inst {
    /// Fresh per call, so forge starts from the same globals, memory image and
    /// table wasmi does.
    pub(crate) fn new(m: &forge_core::CompiledModule, code_base: u64) -> Option<Inst> {
        use forge_core::vmctx as v;
        let plan = &m.plan;
        // Imported globals hold the low indices and have no initialiser here.
        let imported = plan.global_types.len().saturating_sub(plan.global_init.len());
        let mut globals: Vec<u64> = vec![0; imported];
        globals.extend(plan.global_init.iter().map(|g| g.unwrap_or(0) as u64));
        globals.push(0); // never hand out a null base

        let pages = plan.memory.map(|(min, _)| min as usize).unwrap_or(1);
        let memory = Memory::new(pages)?;
        for (off, bytes) in &plan.data_init {
            let off = *off as usize;
            if off + bytes.len() > memory.size {
                return None;
            }
            // SAFETY: bounds checked against the readable window above.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), memory.base.add(off), bytes.len()) };
        }

        // The table: one code pointer per slot, and the canonical signature id
        // beside it. A slot whose function did not compile points at the trap
        // stub rather than at nothing.
        let slots = plan.table.map(|(min, _)| min as usize).unwrap_or(0);
        let mut table = vec![code_base + m.trap_offset as u64; slots.max(1)];
        let mut table_sigs = vec![u32::MAX; slots.max(1)];
        for (off, funcs) in &plan.elem_init {
            for (i, fi) in funcs.iter().enumerate() {
                let slot = *off as usize + i;
                if slot >= table.len() {
                    return None;
                }
                table[slot] = match m.offset_of(*fi) {
                    Some(o) => code_base + o as u64,
                    None => code_base + m.trap_offset as u64,
                };
                let ti = *plan.func_type_of.get(*fi as usize)? as usize;
                table_sigs[slot] = *plan.sig_id.get(ti)?;
            }
        }

        // Host functions, in import order. An import this harness does not
        // know gets the trap stub, so a wrong name faults instead of calling
        // something arbitrary.
        let host_fns: Vec<u64> = plan
            .imported_funcs
            .iter()
            .map(|(module, name)| {
                host_fn(module, name)
                    .or_else(|| crate::bench::host_for(module, name))
                    .or_else(|| crate::pybench::wasi_host(module, name))
                    .unwrap_or(code_base + m.trap_offset as u64)
            })
            .collect();

        let mut ctx = vec![0u64; v::SIZE / 8];
        ctx[v::MEM_BASE as usize / 8] = memory.base as u64;
        ctx[v::MEM_SIZE as usize / 8] = memory.size as u64;
        ctx[v::GLOBALS as usize / 8] = globals.as_ptr() as u64;
        ctx[v::TABLE as usize / 8] = table.as_ptr() as u64;
        ctx[v::TABLE_LEN as usize / 8] = slots as u64;
        ctx[v::HOST_FNS as usize / 8] = host_fns.as_ptr() as u64;
        ctx[v::TABLE_SIGS as usize / 8] = table_sigs.as_ptr() as u64;
        // wasm's own ceiling when the module declares none is 65536 pages,
        // and the reservation is sized to hold all of it.
        ctx[v::MEM_MAX_PAGES as usize / 8] =
            plan.memory.and_then(|(_, m)| m).unwrap_or(65536);
        ctx[v::BUILTIN_GROW as usize / 8] = h_memory_grow as usize as u64;
        ctx[v::FUEL as usize / 8] = DEFAULT_FUEL as u64;
        Some(Inst {
            ctx,
            globals,
            memory,
            table,
            table_sigs,
            host_fns,
        })
    }

    pub(crate) fn ptr(&self) -> *const u64 {
        self.ctx.as_ptr()
    }

    /// What is left of the budget. The generated code keeps the counter in a
    /// register while it runs and writes it back on the way out, so this is
    /// only meaningful after a call has returned.
    /// What stopped the last call — `trap::NONE` if it returned normally.
    pub(crate) fn trap_code(&self) -> u32 {
        self.ctx[forge_core::vmctx::TRAP_CODE as usize / 8] as u32
    }

    pub(crate) fn fuel_left(&self) -> i64 {
        self.ctx[forge_core::vmctx::FUEL as usize / 8] as i64
    }

    pub(crate) fn fuel_used(&self) -> i64 {
        DEFAULT_FUEL - self.fuel_left()
    }

    /// Set a budget small enough to run out.
    pub(crate) fn set_fuel(&mut self, v: i64) {
        self.ctx[forge_core::vmctx::FUEL as usize / 8] = v as u64;
    }
}

// The host side, defined once and handed to both engines so a disagreement
// can only come from the generated code.
extern "C" fn h_add(_ctx: *const u64, a: u32, b: u32, _c: u32) -> u32 {
    a.wrapping_add(b)
}
extern "C" fn h_double(_ctx: *const u64, a: u32, _b: u32, _c: u32) -> u32 {
    a.wrapping_mul(2)
}
extern "C" fn h_const(_ctx: *const u64, _a: u32, _b: u32, _c: u32) -> u32 {
    12345
}

/// The runtime side of `memory.grow`. The base never moves — that is what the
/// 8 GiB reservation buys — so growing is only ever "make more of it
/// readable". Fresh pages come out zero because the reservation was never
/// written to.
pub(crate) extern "C" fn h_memory_grow(ctx: *mut u64, delta: u32) -> u32 {
    use forge_core::vmctx as v;
    // SAFETY: `ctx` is the instance context this module was called with, laid
    // out by `forge_core::vmctx`.
    unsafe {
        let base = *ctx.add(v::MEM_BASE as usize / 8) as *mut libc::c_void;
        let size = *ctx.add(v::MEM_SIZE as usize / 8) as usize;
        let max_pages = *ctx.add(v::MEM_MAX_PAGES as usize / 8) as usize;
        let old_pages = size / PAGE;
        let Some(new_pages) = old_pages.checked_add(delta as usize) else {
            return u32::MAX;
        };
        if new_pages > max_pages {
            return u32::MAX;
        }
        if delta > 0 {
            let add = delta as usize * PAGE;
            if libc::mprotect(base.add(size), add, libc::PROT_READ | libc::PROT_WRITE) != 0 {
                return u32::MAX;
            }
        }
        *ctx.add(v::MEM_SIZE as usize / 8) = (new_pages * PAGE) as u64;
        old_pages as u32
    }
}

pub(crate) fn host_fn(module: &str, name: &str) -> Option<u64> {
    let f = match (module, name) {
        ("env", "add") => h_add as usize,
        ("env", "double") => h_double as usize,
        ("env", "konst") => h_const as usize,
        _ => return None,
    };
    Some(f as u64)
}

fn under_wasmi(wasm: &[u8], args: &[u32]) -> Option<u32> {
    use wasmi::{Config, Engine, Linker, Module, Store, Val};
    let mut cfg = Config::default();
    cfg.consume_fuel(true);
    let engine = Engine::new(&cfg);
    let module = Module::new(&engine, wasm).ok()?;
    let mut store = Store::new(&engine, ());
    store.set_fuel(u64::MAX / 4).ok()?;
    let mut linker = <Linker<()>>::new(&engine);
    linker
        .func_wrap("env", "add", |a: i32, b: i32| a.wrapping_add(b))
        .ok()?;
    linker
        .func_wrap("env", "double", |a: i32| a.wrapping_mul(2))
        .ok()?;
    linker.func_wrap("env", "konst", || 12345i32).ok()?;
    let instance = linker.instantiate_and_start(&mut store, &module).ok()?;
    let f = instance.get_func(&store, "f")?;
    let vals: Vec<Val> = args.iter().map(|a| Val::I32(*a as i32)).collect();
    let mut out = [Val::I32(0)];
    f.call(&mut store, &vals, &mut out).ok()?;
    match out[0] {
        Val::I32(v) => Some(v as u32),
        _ => None,
    }
}

/// Run `src`'s exported `f` with one argument and report both what it returned
/// and what stopped it.
///
/// Traps used to be checked in a forked child, by watching which signal killed
/// it. They do not need a child any more, and that is the point of this whole
/// round: a trap is a RESULT now. The test can look at the reason instead of a
/// death certificate, and a wrong reason is as visible as a missing one.
pub fn oneshot(wasm: &[u8], arg: u32, fuel: Option<i64>) -> Option<(u32, u32)> {
    let m = forge_core::compile(wasm).ok()?;
    let fidx = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i)?;
    let off = m.offset_of(fidx)?;
    let exec = Exec::new(&m.code)?;
    exec.arm_traps(&m);
    let mut inst = Inst::new(&m, exec.base())?;
    if let Some(v) = fuel {
        inst.set_fuel(v);
    }
    let r = exec.call_entry(m.entry_offset, off, inst.ptr(), arg, 0, 0);
    Some((r, inst.trap_code()))
}

fn run_it(src: &str, arg: u32, fuel: Option<i64>) -> Option<(u32, u32)> {
    let wasm = wat::parse_str(src).ok()?;
    let m = forge_core::compile(&wasm).ok()?;
    let fidx = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i)?;
    let off = m.offset_of(fidx)?;
    let exec = Exec::new(&m.code)?;
    exec.arm_traps(&m);
    let mut inst = Inst::new(&m, exec.base())?;
    if let Some(v) = fuel {
        inst.set_fuel(v);
    }
    let r = exec.call_entry(m.entry_offset, off, inst.ptr(), arg, 0, 0);
    Some((r, inst.trap_code()))
}

/// Every trap the generator can raise, checked for the RIGHT reason.
fn traps_report_themselves() -> bool {
    use forge_core::trap::*;
    let mut ok = true;
    let mut want = |src: &str, arg: u32, fuel: Option<i64>, code: u32, what: &str| {
        match run_it(src, arg, fuel) {
            Some((_, got)) if got == code => {}
            Some((_, got)) => {
                println!("  Trap: {what} meldete {} statt {}", name(got), name(code));
                ok = false;
            }
            None => {
                println!("  Trap: {what} liess sich nicht fahren");
                ok = false;
            }
        }
    };

    // A guard page catches an access past the end of memory — no bounds check
    // is emitted for it, so this is where that decision is proved.
    let mem = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \
        local.get 0 i32.load))";
    want(mem, 0, None, NONE, "Zugriff innerhalb");
    want(mem, 65532, None, NONE, "Zugriff am letzten Wort");
    want(mem, 65536, None, MEMORY_OUT_OF_BOUNDS, "ein Byte hinter dem Speicher");
    want(mem, 1 << 30, None, MEMORY_OUT_OF_BOUNDS, "weit daneben");
    want(mem, 0xFFFF_FFFF, None, MEMORY_OUT_OF_BOUNDS, "an der Adressspitze");

    // The table index is the one bound that has to be checked in code.
    let ind = "(module \
        (type $t (func (param i32) (result i32))) \
        (func $a (type $t) local.get 0 i32.const 5 i32.add) \
        (table 2 funcref) (elem (i32.const 0) $a) \
        (func (export \"f\") (param i32) (result i32) \
           i32.const 7 local.get 0 call_indirect (type $t)))";
    want(ind, 0, None, NONE, "gueltiger indirekter Aufruf");
    want(ind, 1, None, BAD_SIGNATURE, "leerer Tabellenplatz");
    want(ind, 2, None, TABLE_OUT_OF_BOUNDS, "Index genau daneben");
    want(ind, 99, None, TABLE_OUT_OF_BOUNDS, "Index weit daneben");

    let mism = "(module \
        (type $t1 (func (param i32) (result i32))) \
        (type $t2 (func (param i32 i32) (result i32))) \
        (func $b (type $t2) local.get 0 local.get 1 i32.add) \
        (table 1 funcref) (elem (i32.const 0) $b) \
        (func (export \"f\") (param i32) (result i32) \
           i32.const 7 local.get 0 call_indirect (type $t1)))";
    want(mism, 0, None, BAD_SIGNATURE, "falsche Signatur");

    // Bulk memory checks its own range, because the trap has to come before
    // the first byte moves. Zero length still counts.
    let f16 = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \
        local.get 0 i32.const 0 i32.const 16 memory.fill i32.const 12))";
    let f0 = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \
        local.get 0 i32.const 0 i32.const 0 memory.fill i32.const 12))";
    let c16 = "(module (memory 1) (func (export \"f\") (param i32) (result i32) \
        i32.const 0 local.get 0 i32.const 16 memory.copy i32.const 12))";
    want(f16, 65520, None, NONE, "fill endet genau am Ende");
    want(f16, 65521, None, MEMORY_OUT_OF_BOUNDS, "fill ein Byte darueber");
    want(f0, 65536, None, NONE, "leerer fill genau am Ende");
    want(f0, 65537, None, MEMORY_OUT_OF_BOUNDS, "leerer fill ein Byte darueber");
    want(c16, 65521, None, MEMORY_OUT_OF_BOUNDS, "copy-Quelle darueber");

    // Division: the processor raises the fault, and no compare is emitted.
    let by = |op: &str| {
        format!("(module (func (export \"f\") (param i32) (result i32) \
                 i32.const 100 local.get 0 {op}))")
    };
    for op in ["i32.div_s", "i32.div_u", "i32.rem_s", "i32.rem_u"] {
        let src = by(op);
        want(&src, 0, None, DIVIDE_ERROR, op);
        want(&src, 7, None, NONE, op);
    }
    let minmax = |op: &str| {
        format!("(module (func (export \"f\") (param i32) (result i32) \
                 i32.const -2147483648 local.get 0 {op}))")
    };
    want(&minmax("i32.div_s"), 0xFFFF_FFFF, None, DIVIDE_ERROR, "INT_MIN/-1");
    // …and the one that must NOT trap, because wasm wants an answer there.
    want(&minmax("i32.rem_s"), 0xFFFF_FFFF, None, NONE, "INT_MIN%-1 darf NICHT trappen");

    // `unreachable` is a trap the generator raises itself.
    let unr = "(module (func (export \"f\") (param i32) (result i32) \
        local.get 0 if unreachable end i32.const 3))";
    want(unr, 0, None, NONE, "unreachable nicht erreicht");
    want(unr, 1, None, UNREACHABLE, "unreachable erreicht");

    // A resource limit that does not limit is decoration.
    let loopy = "(module (func (export \"f\") (param i32) (result i32) \
        (local i32) \
        (block $done (loop $top \
          local.get 1 i32.const 1 i32.add local.set 1 \
          local.get 0 br_if $top)) \
        local.get 1))";
    want(loopy, 1, Some(100_000), OUT_OF_FUEL, "Endlosschleife");
    want(loopy, 0, Some(100_000), NONE, "endlicher Lauf");
    want(loopy, 0, Some(0), OUT_OF_FUEL, "leeres Budget");
    ok
}

struct Case {
    name: &'static str,
    /// Module-level text placed before the function — globals, mostly.
    decl: &'static str,
    body: &'static str,
    params: usize,
}

const ARGS: &[[u32; 3]] = &[
    [0, 0, 0],
    [1, 1, 1],
    [7, 3, 2],
    [3, 7, 2],
    [0xFFFF_FFFF, 1, 0],
    [0x8000_0000, 0xFFFF_FFFF, 5],
    [0x7FFF_FFFF, 2, 0],
    [123456, 654321, 42],
];

/// Bodies are written against `local.get 0..n`. Two parameters unless the
/// case says otherwise.
const CASES: &[Case] = &[
    Case { name: "add", decl: "", body: "local.get 0 local.get 1 i32.add", params: 2 },
    Case { name: "sub", decl: "", body: "local.get 0 local.get 1 i32.sub", params: 2 },
    Case { name: "mul", decl: "", body: "local.get 0 local.get 1 i32.mul", params: 2 },
    Case { name: "and", decl: "", body: "local.get 0 local.get 1 i32.and", params: 2 },
    Case { name: "or", decl: "",  body: "local.get 0 local.get 1 i32.or",  params: 2 },
    Case { name: "xor", decl: "", body: "local.get 0 local.get 1 i32.xor", params: 2 },
    Case { name: "eq", decl: "",  body: "local.get 0 local.get 1 i32.eq",  params: 2 },
    Case { name: "ne", decl: "",  body: "local.get 0 local.get 1 i32.ne",  params: 2 },
    Case { name: "lt_s", decl: "", body: "local.get 0 local.get 1 i32.lt_s", params: 2 },
    Case { name: "lt_u", decl: "", body: "local.get 0 local.get 1 i32.lt_u", params: 2 },
    Case { name: "gt_s", decl: "", body: "local.get 0 local.get 1 i32.gt_s", params: 2 },
    Case { name: "gt_u", decl: "", body: "local.get 0 local.get 1 i32.gt_u", params: 2 },
    Case { name: "le_s", decl: "", body: "local.get 0 local.get 1 i32.le_s", params: 2 },
    Case { name: "le_u", decl: "", body: "local.get 0 local.get 1 i32.le_u", params: 2 },
    Case { name: "ge_s", decl: "", body: "local.get 0 local.get 1 i32.ge_s", params: 2 },
    Case { name: "ge_u", decl: "", body: "local.get 0 local.get 1 i32.ge_u", params: 2 },
    Case { name: "eqz", decl: "", body: "local.get 0 i32.eqz", params: 1 },
    Case { name: "const", decl: "", body: "i32.const 305419896", params: 0 },
    Case { name: "const_neg", decl: "", body: "i32.const -1", params: 0 },
    Case { name: "drop", decl: "", body: "local.get 0 local.get 1 drop", params: 2 },
    Case { name: "nop", decl: "", body: "local.get 0 nop", params: 1 },
    Case { name: "ret_early", decl: "", body: "local.get 0 return", params: 1 },
    Case { name: "local_set", decl: "", body: "(local i32) local.get 1 local.set 2 local.get 2 local.get 0 i32.add", params: 2 },
    Case { name: "local_tee", decl: "", body: "(local i32) local.get 0 local.tee 1 local.get 1 i32.add", params: 1 },
    Case { name: "local_zero", decl: "", body: "(local i32) local.get 1", params: 1 },
    Case { name: "deep", decl: "", body: "local.get 0 local.get 1 local.get 2 i32.add i32.add local.get 0 i32.xor", params: 3 },
    Case { name: "chain", decl: "", body:
        "local.get 0 local.get 1 i32.add local.get 1 i32.sub local.get 0 i32.mul local.get 1 i32.and",
        params: 2 },

    Case { name: "shl", decl: "", body: "local.get 0 local.get 1 i32.shl", params: 2 },
    Case { name: "shr_u", decl: "", body: "local.get 0 local.get 1 i32.shr_u", params: 2 },
    Case { name: "shr_s", decl: "", body: "local.get 0 local.get 1 i32.shr_s", params: 2 },
    Case { name: "rotl", decl: "", body: "local.get 0 local.get 1 i32.rotl", params: 2 },
    Case { name: "rotr", decl: "", body: "local.get 0 local.get 1 i32.rotr", params: 2 },
    // A count above 31 is taken mod 32 by both wasm and x86 — worth pinning
    // down rather than assuming.
    Case { name: "shl_big", decl: "", body: "local.get 0 i32.const 33 i32.shl", params: 1 },
    Case { name: "shr_s_big", decl: "", body: "local.get 0 i32.const 63 i32.shr_s", params: 1 },

    // --- structured control flow ---
    Case { name: "block_empty", decl: "", body: "(block) local.get 0", params: 1 },
    Case { name: "block_res", decl: "", body:
        "(block (result i32) local.get 0 local.get 1 i32.add)", params: 2 },
    Case { name: "br_out", decl: "", body:
        "(block (result i32) local.get 0 br 0 drop i32.const 999)", params: 1 },
    Case { name: "br_skip", decl: "", body:
        "(local i32) (block local.get 0 local.set 1 br 0) local.get 1", params: 1 },
    Case { name: "if_else", decl: "", body:
        "local.get 0 if (result i32) local.get 1 else i32.const 99 end", params: 2 },
    Case { name: "if_noelse", decl: "", body:
        "(local i32) local.get 0 if local.get 1 local.set 2 end local.get 2", params: 2 },
    Case { name: "if_both_br", decl: "", body:
        "(block (result i32) local.get 0 if (result i32) local.get 1 else local.get 0 end)",
        params: 2 },
    // The value a taken br_if carries must not overwrite an operand that is
    // still live on the fall-through path.
    Case { name: "brif_value", decl: "", body:
        "(block (result i32) local.get 0 local.get 1 local.get 0 br_if 0 i32.add)", params: 2 },
    Case { name: "brif_void", decl: "", body:
        "(local i32) (block local.get 0 br_if 0 local.get 1 local.set 2) local.get 2", params: 2 },
    Case { name: "nested_br1", decl: "", body:
        "(block (result i32) (block local.get 0 br_if 0 i32.const 42 br 1) local.get 1)",
        params: 2 },
    Case { name: "loop_sum", decl: "", body:
        "(local i32) (local i32) \
         local.get 0 i32.const 7 i32.and local.set 0 \
         block loop \
           local.get 1 local.get 0 i32.ge_u br_if 1 \
           local.get 2 local.get 1 i32.add local.set 2 \
           local.get 1 i32.const 1 i32.add local.set 1 \
           br 0 \
         end end local.get 2", params: 1 },
    Case { name: "loop_if", decl: "", body:
        "(local i32) (local i32) \
         local.get 0 i32.const 15 i32.and local.set 0 \
         block loop \
           local.get 1 local.get 0 i32.ge_u br_if 1 \
           local.get 1 i32.const 1 i32.and if local.get 2 local.get 1 i32.add local.set 2 end \
           local.get 1 i32.const 1 i32.add local.set 1 \
           br 0 \
         end end local.get 2", params: 1 },
    Case { name: "ret_nested", decl: "", body:
        "(block (block local.get 0 return)) i32.const 7", params: 1 },
    Case { name: "unreach_tail", decl: "", body:
        "local.get 0 return unreachable", params: 1 },
    Case { name: "dead_block", decl: "", body:
        "local.get 0 return (block (loop br 0)) i32.const 3", params: 1 },

    // --- globals ---
    Case { name: "global_get", decl: "(global $g (mut i32) (i32.const 1234))", body:
        "global.get $g local.get 0 i32.add", params: 1 },
    Case { name: "global_set", decl: "(global $g (mut i32) (i32.const 1234))", body:
        "local.get 0 global.set $g global.get $g", params: 1 },
    Case { name: "global_const", decl: "(global $c i32 (i32.const 7))", body:
        "global.get $c local.get 0 i32.mul", params: 1 },
    Case { name: "global_two", decl:
        "(global $a (mut i32) (i32.const 100)) (global $b (mut i32) (i32.const 200))", body:
        "local.get 0 global.set $a global.get $a global.get $b i32.add", params: 1 },
    // --- linear memory ---
    // Every address is masked so the case stays inside the mapped window; the
    // guard-page check below is what proves the rest of the reservation bites.
    Case { name: "mem_st_ld", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load", params: 1 },
    Case { name: "mem_offset", decl: "(memory 1)", body:
        "i32.const 4 local.get 0 i32.store offset=8 i32.const 12 i32.load", params: 1 },
    Case { name: "mem_addr", decl: "(memory 1)", body:
        "(local i32) \
         local.get 0 i32.const 1023 i32.and i32.const 4 i32.mul local.tee 2 \
         local.get 1 i32.store \
         local.get 2 i32.load", params: 2 },
    Case { name: "mem_unalign", decl: "(memory 1)", body:
        "i32.const 1 local.get 0 i32.store i32.const 1 i32.load", params: 1 },
    Case { name: "mem_8u", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load8_u", params: 1 },
    Case { name: "mem_8u_1", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load8_u offset=1", params: 1 },
    Case { name: "mem_8s", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load8_s", params: 1 },
    Case { name: "mem_8s_3", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 3 i32.load8_s", params: 1 },
    Case { name: "mem_16u", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load16_u", params: 1 },
    Case { name: "mem_16s", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 0 i32.load16_s", params: 1 },
    Case { name: "mem_16s_2", decl: "(memory 1)", body:
        "i32.const 0 local.get 0 i32.store i32.const 2 i32.load16_s", params: 1 },
    Case { name: "mem_store8", decl: "(memory 1)", body:
        "i32.const 0 i32.const 0 i32.store i32.const 0 local.get 0 i32.store8 i32.const 0 i32.load",
        params: 1 },
    Case { name: "mem_store16", decl: "(memory 1)", body:
        "i32.const 0 i32.const 0 i32.store i32.const 0 local.get 0 i32.store16 i32.const 0 i32.load",
        params: 1 },
    Case { name: "mem_st8_off", decl: "(memory 1)", body:
        "i32.const 0 i32.const 0 i32.store i32.const 0 local.get 0 i32.store8 offset=2 \
         i32.const 0 i32.load", params: 1 },
    Case { name: "mem_data", decl: "(memory 1) (data (i32.const 16) \"\\01\\02\\03\\04\")", body:
        "i32.const 16 i32.load local.get 0 i32.add", params: 1 },
    Case { name: "mem_data_8", decl: "(memory 1) (data (i32.const 16) \"\\ff\\02\\03\\04\")", body:
        "i32.const 16 i32.load8_s local.get 0 i32.add", params: 1 },
    // A loop that walks memory — the shape every real module has.
    Case { name: "mem_loop", decl: "(memory 1)", body:
        "(local i32) (local i32) \
         local.get 0 i32.const 15 i32.and local.set 0 \
         block loop \
           local.get 1 local.get 0 i32.ge_u br_if 1 \
           local.get 1 i32.const 4 i32.mul local.get 1 i32.store \
           local.get 1 i32.const 1 i32.add local.set 1 \
           br 0 \
         end end \
         i32.const 0 local.set 1 i32.const 0 local.set 2 \
         block loop \
           local.get 1 local.get 0 i32.ge_u br_if 1 \
           local.get 2 local.get 1 i32.const 4 i32.mul i32.load i32.add local.set 2 \
           local.get 1 i32.const 1 i32.add local.set 1 \
           br 0 \
         end end local.get 2", params: 1 },

    // --- calls ---
    Case { name: "call_1", decl: "(func $g (param i32) (result i32) local.get 0 i32.const 3 i32.mul)",
        body: "local.get 0 call $g", params: 1 },
    Case { name: "call_2", decl: "(func $g (param i32 i32) (result i32) local.get 0 local.get 1 i32.sub)",
        body: "local.get 0 local.get 1 call $g", params: 2 },
    Case { name: "call_0", decl: "(func $g (result i32) i32.const 777)",
        body: "call $g local.get 0 i32.add", params: 1 },
    Case { name: "call_void", decl:
        "(memory 1) (func $g (param i32) i32.const 0 local.get 0 i32.store)",
        body: "local.get 0 call $g i32.const 0 i32.load", params: 1 },
    Case { name: "call_chain", decl:
        "(func $h (param i32) (result i32) local.get 0 i32.const 1 i32.add) \
         (func $g (param i32) (result i32) local.get 0 call $h call $h)",
        body: "local.get 0 call $g", params: 1 },
    // Recursion: the callee's frame must not disturb the caller's, and the
    // pinned registers have to survive the whole way down and back.
    Case { name: "call_rec", decl:
        "(func $r (param i32) (result i32) \
           local.get 0 i32.eqz if (result i32) i32.const 1 else \
             local.get 0 local.get 0 i32.const 1 i32.sub call $r i32.mul end)",
        body: "local.get 0 i32.const 7 i32.and call $r", params: 1 },
    // An operand sits BELOW the arguments: they must be read from the right
    // slots, not from the bottom of the stack.
    Case { name: "call_under", decl:
        "(func $g (param i32 i32) (result i32) local.get 0 local.get 1 i32.xor)",
        body: "local.get 0 local.get 0 local.get 1 call $g i32.add", params: 2 },
    Case { name: "call_5", decl:
        "(func $g (param i32 i32 i32 i32 i32) (result i32) \
           local.get 0 local.get 1 i32.add local.get 2 i32.add local.get 3 i32.add local.get 4 i32.add)",
        body: "local.get 0 local.get 1 local.get 2 i32.const 10 i32.const 20 call $g", params: 3 },
    // The callee writes memory the caller then reads — the pinned memory base
    // has to be right on both sides of the call.
    Case { name: "call_mem", decl:
        "(memory 1) (func $g (param i32) (result i32) \
           i32.const 32 local.get 0 i32.store i32.const 32 i32.load i32.const 1 i32.add)",
        body: "local.get 0 call $g i32.const 32 i32.load i32.add", params: 1 },

    // --- host imports ---
    Case { name: "host_add", decl:
        "(import \"env\" \"add\" (func $add (param i32 i32) (result i32)))",
        body: "local.get 0 local.get 1 call $add", params: 2 },
    Case { name: "host_double", decl:
        "(import \"env\" \"double\" (func $d (param i32) (result i32)))",
        body: "local.get 0 call $d local.get 1 i32.add", params: 2 },
    Case { name: "host_const", decl:
        "(import \"env\" \"konst\" (func $k (result i32)))",
        body: "call $k local.get 0 i32.add", params: 1 },
    Case { name: "host_mixed", decl:
        "(import \"env\" \"add\" (func $add (param i32 i32) (result i32))) \
         (func $g (param i32) (result i32) local.get 0 i32.const 2 i32.mul)",
        body: "local.get 0 call $g local.get 1 call $add", params: 2 },

    // --- call_indirect ---
    Case { name: "call_ind", decl:
        "(type $t (func (param i32) (result i32))) \
         (func $a (type $t) local.get 0 i32.const 1 i32.add) \
         (func $b (type $t) local.get 0 i32.const 100 i32.mul) \
         (table 2 funcref) (elem (i32.const 0) $a $b)",
        body: "local.get 0 local.get 1 i32.const 1 i32.and call_indirect (type $t)", params: 2 },
    Case { name: "call_ind_2", decl:
        "(type $t1 (func (param i32) (result i32))) \
         (type $t2 (func (param i32 i32) (result i32))) \
         (func $a (type $t1) local.get 0 i32.const 7 i32.xor) \
         (func $b (type $t2) local.get 0 local.get 1 i32.add) \
         (table 2 funcref) (elem (i32.const 0) $a $b)",
        body: "local.get 0 i32.const 0 call_indirect (type $t1) \
               local.get 0 local.get 1 i32.const 1 call_indirect (type $t2) i32.add", params: 2 },
    // Two DISTINCT type indices with the same shape. wasm compares types
    // structurally, so this must pass — comparing raw type indices would
    // reject it, and nothing else in the suite would notice.
    Case { name: "call_ind_same", decl:
        "(type $t1 (func (param i32) (result i32))) \
         (type $t2 (func (param i32) (result i32))) \
         (func $a (type $t1) local.get 0 i32.const 5 i32.add) \
         (table 1 funcref) (elem (i32.const 0) $a) \
         (func $unused (type $t2) local.get 0)",
        body: "local.get 0 i32.const 0 call_indirect (type $t2)", params: 1 },

    // --- select ---
    Case { name: "select", decl: "", body:
        "local.get 0 local.get 1 local.get 0 select", params: 2 },
    Case { name: "select_c0", decl: "", body:
        "local.get 0 local.get 1 i32.const 0 select", params: 2 },
    Case { name: "select_c1", decl: "", body:
        "local.get 0 local.get 1 i32.const 1 select", params: 2 },
    Case { name: "select_deep", decl: "", body:
        "local.get 0 local.get 0 local.get 1 local.get 1 select i32.add", params: 2 },

    // --- br_table ---
    // Named labels throughout: hand-counted branch depths are how a test file
    // grows an infinite loop that looks like a compiler bug.
    Case { name: "brt_3", decl: "", body:
        "(local i32) \
         (block $done \
           (block $c2 (block $c1 (block $c0 \
             local.get 0 i32.const 3 i32.and \
             br_table $c0 $c1 $c2 $done) \
             i32.const 10 local.set 1 br $done) \
             i32.const 20 local.set 1 br $done) \
             i32.const 30 local.set 1 br $done) \
         local.get 1", params: 1 },
    // An index past the last target really has to reach the default.
    Case { name: "brt_default", decl: "", body:
        "(local i32) \
         (block $done \
           (block $c1 (block $c0 \
             local.get 0 i32.const 7 i32.and \
             br_table $c0 $c1 $done) \
             i32.const 100 local.set 1 br $done) \
             i32.const 200 local.set 1 br $done) \
         local.get 1", params: 1 },
    // A br_table carrying a value, to labels at DIFFERENT depths.
    Case { name: "brt_value", decl: "", body:
        "(block $b1 (result i32) \
           (block $b0 (result i32) \
             i32.const 5 \
             local.get 0 i32.const 1 i32.and \
             br_table $b0 $b1) \
           i32.const 100 i32.add)", params: 1 },
    // Duplicate targets: two table entries pointing at the same label.
    Case { name: "brt_dup", decl: "", body:
        "(local i32) \
         (block $done (block $c0 \
           local.get 0 i32.const 3 i32.and \
           br_table $c0 $c0 $done $c0) \
           i32.const 7 local.set 1 br $done) \
         local.get 1", params: 1 },
    Case { name: "brt_wide", decl: "", body:
        "(local i32) \
         (block $done \
           (block $c4 (block $c3 (block $c2 (block $c1 (block $c0 \
             local.get 0 i32.const 7 i32.and \
             br_table $c0 $c1 $c2 $c3 $c4 $done) \
             i32.const 1 local.set 1 br $done) \
             i32.const 2 local.set 1 br $done) \
             i32.const 3 local.set 1 br $done) \
             i32.const 4 local.set 1 br $done) \
             i32.const 5 local.set 1 br $done) \
         local.get 1", params: 1 },
    // A table inside a loop — the back edge must stay a plain jump.
    Case { name: "brt_loop", decl: "", body:
        "(local i32) (local i32) \
         local.get 0 i32.const 15 i32.and local.set 0 \
         (block $exit (loop $top \
           local.get 1 local.get 0 i32.ge_u br_if $exit \
           local.get 1 i32.const 1 i32.add local.set 1 \
           (block $odd (block $even \
             local.get 1 i32.const 1 i32.and \
             br_table $even $odd) \
             local.get 2 i32.const 1 i32.add local.set 2 br $top) \
           local.get 2 i32.const 10 i32.add local.set 2 \
           br $top)) \
         local.get 2", params: 1 },

    // --- more than five arguments ---
    Case { name: "call_6", decl:
        "(func $g (param i32 i32 i32 i32 i32 i32) (result i32) \
           local.get 0 local.get 1 i32.add local.get 2 i32.add local.get 3 i32.add \
           local.get 4 i32.add local.get 5 i32.add)",
        body: "local.get 0 local.get 1 local.get 2 i32.const 1 i32.const 2 i32.const 3 call $g",
        params: 3 },
    Case { name: "call_9", decl:
        "(func $g (param i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32) \
           local.get 8 local.get 7 i32.sub local.get 6 i32.add local.get 5 i32.xor \
           local.get 4 i32.add local.get 3 i32.add local.get 2 i32.add local.get 1 i32.add \
           local.get 0 i32.add)",
        body: "local.get 0 local.get 1 local.get 2 i32.const 4 i32.const 5 i32.const 6 \
               i32.const 7 i32.const 8 i32.const 9 call $g", params: 3 },
    Case { name: "call_arg_last", decl:
        "(func $g (param i32 i32 i32) (result i32) local.get 2)",
        body: "local.get 0 local.get 1 local.get 2 call $g \
               local.get 0 i32.add", params: 3 },
    // An indirect call with stack arguments.
    Case { name: "call_ind_7", decl:
        "(type $t (func (param i32 i32 i32 i32 i32 i32 i32) (result i32))) \
         (func $a (type $t) local.get 6 local.get 5 i32.add local.get 0 i32.add) \
         (table 1 funcref) (elem (i32.const 0) $a)",
        body: "local.get 0 local.get 1 i32.const 1 i32.const 2 i32.const 3 i32.const 4 \
               i32.const 5 i32.const 0 call_indirect (type $t)", params: 2 },
    // A call with stack arguments inside a loop: `rsp` has to come back every
    // single time round, or the frame walks away.
    Case { name: "call_6_loop", decl:
        "(func $g (param i32 i32 i32 i32 i32 i32) (result i32) \
           local.get 0 local.get 5 i32.add)",
        body: "(local i32) (local i32) \
         local.get 0 i32.const 7 i32.and local.set 0 \
         block loop \
           local.get 1 local.get 0 i32.ge_u br_if 1 \
           local.get 2 local.get 1 i32.const 1 i32.const 2 i32.const 3 i32.const 4 i32.const 5 \
           call $g i32.add local.set 2 \
           local.get 1 i32.const 1 i32.add local.set 1 \
           br 0 \
         end end local.get 2", params: 1 },

    // --- i64 ---
    //
    // The exported function keeps its i32 shape; i64 flows through locals,
    // globals, memory and inner calls. Two idioms recur:
    //   MK   builds a 64-bit value with arg0 in the high half and arg1 in the
    //        low half, so both halves carry real data
    //   FOLD xors the two halves back into an i32, so a wrong upper half
    //        cannot hide behind a truncating result
    // FOLD names local 2, so every case using it declares two parameters —
    // even where the second one is not read.
    Case { name: "i64_add", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.add \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_sub", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.sub \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mul", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.mul \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_and", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.and \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_or", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.or \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_xor", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_s i64.xor \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_shl", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_u i64.shl \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_shr_u", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_u i64.shr_u \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_shr_s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_u i64.shr_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_rotl", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_u i64.rotl \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_rotr", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.or local.get 1 i64.extend_i32_u i64.rotr \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },

    // Comparisons: i64 in, i32 out.
    Case { name: "i64_eq", decl: "", body:
        "local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         local.get 1 i64.extend_i32_s i64.eq", params: 2 },
    Case { name: "i64_ne", decl: "", body:
        "local.get 0 i64.extend_i32_s local.get 1 i64.extend_i32_s i64.ne", params: 2 },
    Case { name: "i64_lt_s", decl: "", body:
        "local.get 0 i64.extend_i32_s local.get 1 i64.extend_i32_s i64.lt_s", params: 2 },
    Case { name: "i64_lt_u", decl: "", body:
        "local.get 0 i64.extend_i32_u local.get 1 i64.extend_i32_u i64.lt_u", params: 2 },
    Case { name: "i64_gt_s", decl: "", body:
        "local.get 0 i64.extend_i32_s local.get 1 i64.extend_i32_s i64.gt_s", params: 2 },
    Case { name: "i64_gt_u", decl: "", body:
        "local.get 0 i64.extend_i32_u local.get 1 i64.extend_i32_u i64.gt_u", params: 2 },
    Case { name: "i64_le_s", decl: "", body:
        "local.get 0 i64.extend_i32_s local.get 1 i64.extend_i32_s i64.le_s", params: 2 },
    Case { name: "i64_le_u", decl: "", body:
        "local.get 0 i64.extend_i32_u local.get 1 i64.extend_i32_u i64.le_u", params: 2 },
    Case { name: "i64_ge_s", decl: "", body:
        "local.get 0 i64.extend_i32_s local.get 1 i64.extend_i32_s i64.ge_s", params: 2 },
    Case { name: "i64_ge_u", decl: "", body:
        "local.get 0 i64.extend_i32_u local.get 1 i64.extend_i32_u i64.ge_u", params: 2 },
    Case { name: "i64_eqz", decl: "", body:
        "local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         i64.eqz", params: 2 },

    // Bit counting. The zero input is the case `bsr`/`bsf` leave undefined,
    // so it gets its own case rather than riding on the argument table.
    Case { name: "i64_clz", decl: "", body:
        "local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         i64.clz i32.wrap_i64", params: 2 },
    Case { name: "i64_ctz", decl: "", body:
        "local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         i64.ctz i32.wrap_i64", params: 2 },
    Case { name: "i64_clz_0", decl: "", body: "i64.const 0 i64.clz i32.wrap_i64", params: 0 },
    Case { name: "i64_ctz_0", decl: "", body: "i64.const 0 i64.ctz i32.wrap_i64", params: 0 },
    Case { name: "i32_clz", decl: "", body: "local.get 0 i32.clz", params: 1 },
    Case { name: "i32_ctz", decl: "", body: "local.get 0 i32.ctz", params: 1 },
    Case { name: "i32_clz_0", decl: "", body: "i32.const 0 i32.clz", params: 0 },
    Case { name: "i32_ctz_0", decl: "", body: "i32.const 0 i32.ctz", params: 0 },
    Case { name: "i32_popcnt", decl: "", body: "local.get 0 i32.popcnt", params: 1 },

    // Width conversions, both directions and both signednesses.
    Case { name: "i64_ext_u", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_ext_s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_wrap", decl: "", body:
        "local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         i32.wrap_i64", params: 2 },
    Case { name: "i32_ext8s", decl: "", body: "local.get 0 i32.extend8_s", params: 1 },
    Case { name: "i32_ext16s", decl: "", body: "local.get 0 i32.extend16_s", params: 1 },
    Case { name: "i64_ext8s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.extend8_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_ext16s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.extend16_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_ext32s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.extend32_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_const_big", decl: "", body:
        "(local i64) i64.const 0x0123456789abcdef \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor \
         local.get 0 i32.add", params: 2 },

    // --- i64 in memory ---
    Case { name: "i64_mem", decl: "(memory 1)", body:
        "(local i64) i32.const 8 \
         local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or \
         i64.store i32.const 8 i64.load \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mem_8u", decl: "(memory 1)", body:
        "i32.const 8 local.get 0 i64.extend_i32_u i64.store i32.const 8 i64.load8_u i32.wrap_i64",
        params: 1 },
    Case { name: "i64_mem_8s", decl: "(memory 1)", body:
        "(local i64) i32.const 8 local.get 0 i64.extend_i32_u i64.store \
         i32.const 8 i64.load8_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mem_16s", decl: "(memory 1)", body:
        "(local i64) i32.const 8 local.get 0 i64.extend_i32_u i64.store \
         i32.const 8 i64.load16_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mem_32u", decl: "(memory 1)", body:
        "(local i64) i32.const 8 local.get 0 i64.extend_i32_u i64.store \
         i32.const 8 i64.load32_u \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mem_32s", decl: "(memory 1)", body:
        "(local i64) i32.const 8 local.get 0 i64.extend_i32_u i64.store \
         i32.const 8 i64.load32_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_mem_st8", decl: "(memory 1)", body:
        "i32.const 8 i64.const 0 i64.store \
         i32.const 8 local.get 0 i64.extend_i32_u i64.store8 i32.const 8 i64.load i32.wrap_i64",
        params: 1 },
    Case { name: "i64_mem_st32", decl: "(memory 1)", body:
        "(local i64) i32.const 8 i64.const -1 i64.store \
         i32.const 8 local.get 0 i64.extend_i32_u i64.store32 i32.const 8 i64.load \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },

    // --- i64 through globals, select, blocks and calls ---
    Case { name: "i64_global", decl: "(global $g (mut i64) (i64.const 0x0123456789abcdef))", body:
        "(local i64) global.get $g local.get 0 i64.extend_i32_u i64.add global.set $g \
         global.get $g \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_select", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_s local.get 0 select \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_block", decl: "", body:
        "(local i64) (block (result i64) \
           local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or) \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_br", decl: "", body:
        "(local i64) (block $b (result i64) \
           local.get 0 i64.extend_i32_s br $b) \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_call", decl:
        "(func $g (param i64) (result i64) local.get 0 i64.const 3 i64.mul)",
        body: "(local i64) local.get 0 i64.extend_i32_s call $g \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    // Six i64 parameters: the sixth goes on the stack, at full width.
    Case { name: "i64_call_6", decl:
        "(func $g (param i64 i64 i64 i64 i64 i64) (result i64) \
           local.get 0 local.get 5 i64.add local.get 4 i64.xor)",
        body: "(local i64) local.get 0 i64.extend_i32_s i64.const 1 i64.const 2 i64.const 3 \
         i64.const 4 local.get 1 i64.extend_i32_u i64.const 32 i64.shl call $g \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    // Mixed widths in one signature, across the register/stack boundary.
    Case { name: "i64_call_mix", decl:
        "(func $g (param i32 i64 i32 i64 i32 i64 i32) (result i64) \
           local.get 1 local.get 3 i64.add local.get 5 i64.xor \
           local.get 0 i64.extend_i32_u i64.add \
           local.get 6 i64.extend_i32_u i64.add)",
        body: "(local i64) local.get 0 local.get 1 i64.extend_i32_s local.get 1 \
         i64.const 7 i32.const 9 local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 call $g \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },

    // --- bulk memory ---
    //
    // The memory is seeded from a data segment with 64 distinguishable bytes,
    // and the check folds position-dependently (`acc*31 + byte`) over a window
    // wider than anything the cases touch. A plain sum would let a copy that
    // ran in the wrong direction pass.
    Case { name: "mem_fill", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) local.get 0 i32.const 31 i32.and local.get 1 local.get 0 i32.const 15 i32.and memory.fill i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_fill_0", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) local.get 0 i32.const 31 i32.and local.get 1 i32.const 0 memory.fill i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_fill_wide", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) i32.const 3 local.get 1 i32.const 100 memory.fill i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    // Destination below the source: copying upwards is safe.
    Case { name: "mem_cp_fwd", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) i32.const 0 local.get 0 i32.const 15 i32.and i32.const 24 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    // Destination ABOVE the source and overlapping: this is the case a plain
    // forward copy gets wrong, and the only one the direction flag is for.
    Case { name: "mem_cp_bwd", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) local.get 0 i32.const 15 i32.and i32.const 0 i32.const 24 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_cp_same", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) local.get 0 i32.const 15 i32.and local.get 0 i32.const 15 i32.and i32.const 24 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_cp_0", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) local.get 0 i32.const 31 i32.and i32.const 0 i32.const 0 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_cp_far", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) i32.const 100 i32.const 0 i32.const 32 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_cp_touch", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) i32.const 32 i32.const 0 i32.const 32 memory.copy i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },
    Case { name: "mem_cp_then_fill", decl: "(memory 1) (data (i32.const 0) \"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/\")", body:
        "(local i32) (local i32) i32.const 8 i32.const 0 i32.const 40 memory.copy i32.const 16 local.get 1 i32.const 8 memory.fill i32.const 0 local.set 2 i32.const 0 local.set 3 block loop local.get 2 i32.const 192 i32.ge_u br_if 1 local.get 3 i32.const 31 i32.mul local.get 2 i32.load8_u i32.add local.set 3 local.get 2 i32.const 1 i32.add local.set 2 br 0 end end local.get 3", params: 2 },

    // --- division ---
    // The divisor is forced non-zero, and for the signed quotients also away
    // from -1, so the ordinary cases can share the argument table with
    // everything else. The trapping combinations are checked in a child.
    Case { name: "i32_div_s", decl: "", body:
        "local.get 0 local.get 1 i32.const 0x7fffffff i32.and i32.const 1 i32.or i32.div_s",
        params: 2 },
    Case { name: "i32_div_u", decl: "", body:
        "local.get 0 local.get 1 i32.const 0x7fffffff i32.and i32.const 1 i32.or i32.div_u",
        params: 2 },
    // Here the divisor MAY be -1: the argument table has 0xFFFFFFFF paired
    // with INT_MIN, which is exactly the shortcut's reason to exist.
    Case { name: "i32_rem_s", decl: "", body:
        "local.get 0 local.get 1 i32.const 1 i32.or i32.rem_s", params: 2 },
    Case { name: "i32_rem_u", decl: "", body:
        "local.get 0 local.get 1 i32.const 1 i32.or i32.rem_u", params: 2 },
    Case { name: "i32_rem_min", decl: "", body:
        "i32.const -2147483648 i32.const -1 i32.rem_s local.get 0 i32.add", params: 2 },
    // Truncation runs toward zero and the remainder keeps the dividend's
    // sign — the two places where a hand-written lowering usually drifts.
    Case { name: "i32_div_trunc", decl: "", body:
        "i32.const -7 i32.const 2 i32.div_s local.get 0 i32.add", params: 2 },
    Case { name: "i32_rem_sign", decl: "", body:
        "i32.const -7 i32.const 3 i32.rem_s local.get 0 i32.add", params: 2 },
    Case { name: "i32_rem_sign2", decl: "", body:
        "i32.const 7 i32.const -3 i32.rem_s local.get 0 i32.add", params: 2 },

    Case { name: "i64_div_s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_s \
         local.get 1 i64.extend_i32_u i64.const 0x7fffffff i64.and i64.const 1 i64.or i64.div_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_div_u", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.const 0x7fffffff i64.and i64.const 1 i64.or i64.div_u \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_rem_s", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_s \
         local.get 1 i64.extend_i32_s i64.const 1 i64.or i64.rem_s \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_rem_u", decl: "", body:
        "(local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl \
         local.get 1 i64.extend_i32_u i64.const 1 i64.or i64.rem_u \
         local.tee 2 i64.const 32 i64.shr_u i32.wrap_i64 local.get 2 i32.wrap_i64 i32.xor",
        params: 2 },
    Case { name: "i64_rem_min", decl: "", body:
        "i64.const -9223372036854775808 i64.const -1 i64.rem_s i32.wrap_i64 local.get 0 i32.add",
        params: 2 },

    // --- floating point ---
    //
    // A float result is NOT compared bit for bit: wasm leaves NaN payloads to
    // the implementation, so forge and wasmi may legitimately differ there.
    // The fold below asks whether the result IS a NaN — the part the spec does
    // fix — and folds a non-NaN result down through its exact bits, so signed
    // zero and the last mantissa bit still count.
    Case { name: "f64_add", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.add local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_sub", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.sub local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_mul", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.mul local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_div", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.div local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_add", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.add local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_sub", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.sub local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_mul", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.mul local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_div", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.div local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    // Inputs built from raw bits, so infinities, NaNs and denormals get in.
    Case { name: "f64_add_bits", decl: "", body:
        "(local f64) (local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or f64.reinterpret_i64 local.get 1 f64.convert_i32_s f64.add  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_add_bits", decl: "", body:
        "(local f32) local.get 0 f32.reinterpret_i32 local.get 1 f32.convert_i32_s f32.add  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    // min/max: the two places the hardware instruction disagrees with wasm.
    Case { name: "f64_min", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.min local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_max", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.max local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_min", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.min local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_max", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.max local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_min_zero", decl: "", body:
        "(local f64) (local i64) f64.const 0.0 f64.const -0.0 f64.min  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_max_zero", decl: "", body:
        "(local f64) (local i64) f64.const 0.0 f64.const -0.0 f64.max  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_min_zero2", decl: "", body:
        "(local f64) (local i64) f64.const -0.0 f64.const 0.0 f64.min  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_max_zero2", decl: "", body:
        "(local f64) (local i64) f64.const -0.0 f64.const 0.0 f64.max  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_min_nan", decl: "", body:
        "(local f64) (local i64) f64.const nan f64.const 1.0 f64.min  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_max_nan", decl: "", body:
        "(local f64) (local i64) f64.const 1.0 f64.const nan f64.max  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_min_zero", decl: "", body:
        "(local f32) f32.const 0.0 f32.const -0.0 f32.min  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    // Rounding. `nearest` rounds halves to EVEN, which is the one a
    // hand-written lowering usually gets wrong in both directions.
    Case { name: "f64_floor", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 4.0 f64.div f64.floor  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_ceil", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 4.0 f64.div f64.ceil  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_trunc", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 4.0 f64.div f64.trunc  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_nearest", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 4.0 f64.div f64.nearest  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_near_2_5", decl: "", body:
        "(local f64) (local i64) f64.const 2.5 f64.nearest  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_near_3_5", decl: "", body:
        "(local f64) (local i64) f64.const 3.5 f64.nearest  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_near_m2_5", decl: "", body:
        "(local f64) (local i64) f64.const -2.5 f64.nearest  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_trunc", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s f32.const 4.0 f32.div f32.trunc  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_sqrt", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_u f64.sqrt  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_sqrt_neg", decl: "", body:
        "(local f64) (local i64) f64.const -1.0 f64.sqrt  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    // Sign work, where the answer is a bit pattern and signed zero decides.
    Case { name: "f64_abs", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.sub f64.abs  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_neg", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.sub f64.neg  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_abs_m0", decl: "", body:
        "(local f64) (local i64) f64.const -0.0 f64.abs  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_neg_p0", decl: "", body:
        "(local f64) (local i64) f64.const 0.0 f64.neg  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_copysign", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.copysign  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_copysign_m0", decl: "", body:
        "(local f64) (local i64) f64.const 1.0 f64.const -0.0 f64.copysign  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_abs_m0", decl: "", body:
        "(local f32) f32.const -0.0 f32.abs  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_copysign", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.copysign  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    // Comparisons return i32, so they need no fold — and an unordered pair
    // is where `setb` would call a NaN "less than".
    Case { name: "f64_cmp_eq", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.eq", params: 2 },
    Case { name: "f64_cmp_ne", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.ne", params: 2 },
    Case { name: "f64_cmp_lt", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.lt", params: 2 },
    Case { name: "f64_cmp_gt", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.gt", params: 2 },
    Case { name: "f64_cmp_le", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.le", params: 2 },
    Case { name: "f64_cmp_ge", decl: "", body:
        "local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.ge", params: 2 },
    Case { name: "f32_cmp_eq", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.eq", params: 2 },
    Case { name: "f32_cmp_ne", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.ne", params: 2 },
    Case { name: "f32_cmp_lt", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.lt", params: 2 },
    Case { name: "f32_cmp_gt", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.gt", params: 2 },
    Case { name: "f32_cmp_le", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.le", params: 2 },
    Case { name: "f32_cmp_ge", decl: "", body:
        "local.get 0 f32.convert_i32_s local.get 1 f32.convert_i32_s f32.ge", params: 2 },
    Case { name: "f64_nan_eq", decl: "", body:
        "f64.const nan f64.const 1.0 f64.eq local.get 0 i32.add ", params: 2 },
    Case { name: "f64_nan_ne", decl: "", body:
        "f64.const nan f64.const 1.0 f64.ne local.get 0 i32.add ", params: 2 },
    Case { name: "f64_nan_lt", decl: "", body:
        "f64.const nan f64.const 1.0 f64.lt local.get 0 i32.add ", params: 2 },
    Case { name: "f64_nan_gt", decl: "", body:
        "f64.const nan f64.const 1.0 f64.gt local.get 0 i32.add ", params: 2 },
    Case { name: "f64_nan_le", decl: "", body:
        "f64.const nan f64.const 1.0 f64.le local.get 0 i32.add ", params: 2 },
    Case { name: "f64_nan_ge", decl: "", body:
        "f64.const nan f64.const 1.0 f64.ge local.get 0 i32.add ", params: 2 },
    Case { name: "f64_eq_zero", decl: "", body:
        "f64.const 0.0 f64.const -0.0 f64.eq local.get 0 i32.add ", params: 2 },
    // Conversions in both directions and both signednesses.
    Case { name: "f64_cv_i32s", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_cv_i32u", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_u  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_cv_i32s", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_s  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_cv_i32u", decl: "", body:
        "(local f32) local.get 0 f32.convert_i32_u  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_cv_i64s", decl: "", body:
        "(local f64) (local i64) local.get 0 i64.extend_i32_s f64.convert_i64_s  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_cv_i64u", decl: "", body:
        "(local f64) (local i64) local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or f64.convert_i64_u  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_cv_i64u", decl: "", body:
        "(local f32) local.get 0 i64.extend_i32_u i64.const 32 i64.shl local.get 1 i64.extend_i32_u i64.or f32.convert_i64_u  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_cv_i64u_neg", decl: "", body:
        "(local f64) (local i64) i64.const -1 f64.convert_i64_u  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_cv_i64u_neg", decl: "", body:
        "(local f32) i64.const -1 f32.convert_i64_u  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f32_demote", decl: "", body:
        "(local f32) local.get 0 f64.convert_i32_s f64.const 3.0 f64.div f32.demote_f64  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_promote", decl: "", body:
        "(local f64) (local i64) local.get 0 f32.convert_i32_s f32.const 3.0 f32.div f64.promote_f32  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_reinterp", decl: "", body:
        "(local f32) local.get 0 f32.reinterpret_i32  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "i32_reinterp", decl: "", body:
        "local.get 0 f32.reinterpret_i32 i32.reinterpret_f32 ", params: 2 },
    Case { name: "i64_reinterp", decl: "", body:
        "local.get 0 i64.extend_i32_u f64.reinterpret_i64 i64.reinterpret_f64 i32.wrap_i64 ", params: 2 },
    // Saturating truncation: the hardware answers "indefinite" for a NaN, for
    // either overflow AND for a legitimate minimum, so every one of those
    // gets its own case.
    Case { name: "ts_i32f64s", decl: "", body:
        "local.get 0 f64.convert_i32_s f64.const 3.0 f64.div i32.trunc_sat_f64_s ", params: 2 },
    Case { name: "ts_i32f64s_nan", decl: "", body:
        "f64.const nan i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64s_big", decl: "", body:
        "f64.const 1e300 i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64s_neg", decl: "", body:
        "f64.const -1e300 i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64s_min", decl: "", body:
        "f64.const -2147483648.0 i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64s_edge", decl: "", body:
        "f64.const 2147483648.0 i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64s_inf", decl: "", body:
        "f64.const inf i32.trunc_sat_f64_s local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64u", decl: "", body:
        "local.get 0 f64.convert_i32_u f64.const 3.0 f64.div i32.trunc_sat_f64_u ", params: 2 },
    Case { name: "ts_i32f64u_nan", decl: "", body:
        "f64.const nan i32.trunc_sat_f64_u local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64u_neg", decl: "", body:
        "f64.const -1.5 i32.trunc_sat_f64_u local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64u_big", decl: "", body:
        "f64.const 4294967296.0 i32.trunc_sat_f64_u local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f64u_max", decl: "", body:
        "f64.const 4294967295.0 i32.trunc_sat_f64_u local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i32f32s", decl: "", body:
        "local.get 0 f32.convert_i32_s f32.const 3.0 f32.div i32.trunc_sat_f32_s ", params: 2 },
    Case { name: "ts_i32f32u_big", decl: "", body:
        "f32.const 1e30 i32.trunc_sat_f32_u local.get 0 i32.add ", params: 2 },
    Case { name: "ts_i64f64s", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 3.0 f64.div i64.trunc_sat_f64_s local.set 3 local.get 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor", params: 2 },
    Case { name: "ts_i64f64s_big", decl: "", body:
        "(local f64) (local i64) f64.const 1e300 i64.trunc_sat_f64_s local.set 3 local.get 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor", params: 2 },
    Case { name: "ts_i64f64s_nan", decl: "", body:
        "(local f64) (local i64) f64.const nan i64.trunc_sat_f64_s local.set 3 local.get 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor", params: 2 },
    Case { name: "ts_i64f32s_neg", decl: "", body:
        "(local f64) (local i64) f32.const -1e30 i64.trunc_sat_f32_s local.set 3 local.get 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor", params: 2 },
    // Floats through memory, globals, calls and select.
    Case { name: "f64_mem", decl: "(memory 1)", body:
        "(local f64) (local i64) i32.const 8 local.get 0 f64.convert_i32_s f64.store i32.const 8 f64.load  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f32_mem", decl: "(memory 1)", body:
        "(local f32) i32.const 8 local.get 0 f32.convert_i32_s f32.store i32.const 8 f32.load  local.set 2 local.get 2 local.get 2 f32.ne if (result i32) i32.const 31337 else local.get 2 i32.reinterpret_f32 end", params: 2 },
    Case { name: "f64_global", decl: "(global $g (mut f64) (f64.const 1.25))", body:
        "(local f64) (local i64) global.get $g local.get 0 f64.convert_i32_s f64.add global.set $g global.get $g  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_call", decl: "(func $g (param f64 f64) (result f64) local.get 0 local.get 1 f64.div)", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s call $g  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_call_mix", decl: "(func $g (param i32 f64 i64 f32 f64) (result f64) local.get 1 local.get 4 f64.add local.get 3 f64.promote_f32 f64.add local.get 0 f64.convert_i32_s f64.add)", body:
        "(local f64) (local i64) local.get 0 local.get 0 f64.convert_i32_s local.get 1 i64.extend_i32_s local.get 1 f32.convert_i32_s local.get 1 f64.convert_i32_u call $g  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_call_9", decl: "(func $g (param f64 f64 f64 f64 f64 f64 f64 f64 f64) (result f64) local.get 8 local.get 0 f64.sub)", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s f64.const 1.0 f64.const 2.0 f64.const 3.0 f64.const 4.0 f64.const 5.0 f64.const 6.0 f64.const 7.0 local.get 1 f64.convert_i32_s call $g  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_select", decl: "", body:
        "(local f64) (local i64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s local.get 0 select  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "f64_block", decl: "", body:
        "(local f64) (local i64) (block (result f64) local.get 0 f64.convert_i32_s local.get 1 f64.convert_i32_s f64.mul)  local.set 2 local.get 2 local.get 2 f64.ne if (result i32) i32.const 31337 else local.get 2 i64.reinterpret_f64 local.tee 3 i64.const 32 i64.shr_u i32.wrap_i64 local.get 3 i32.wrap_i64 i32.xor end", params: 2 },
    Case { name: "mem_size", decl: "(memory 3)", body:
        "memory.size local.get 0 i32.add", params: 2 },

    // --- memory.grow ---
    // The base must not move across a grow, and the new pages must read zero.
    Case { name: "grow_size", decl: "(memory 1 4)", body:
        "i32.const 2 memory.grow drop memory.size local.get 0 i32.add", params: 2 },
    Case { name: "grow_ret", decl: "(memory 1 4)", body:
        "i32.const 2 memory.grow local.get 0 i32.add", params: 2 },
    Case { name: "grow_over", decl: "(memory 1 4)", body:
        "i32.const 10 memory.grow local.get 0 i32.add", params: 2 },
    Case { name: "grow_zero", decl: "(memory 2 4)", body:
        "i32.const 0 memory.grow local.get 0 i32.add", params: 2 },
    // Write below the old end, grow, then read it back — the pinned base has
    // to still be right afterwards.
    Case { name: "grow_keeps", decl: "(memory 1 4)", body:
        "i32.const 16 local.get 0 i32.store \
         i32.const 1 memory.grow drop \
         i32.const 16 i32.load local.get 1 i32.add", params: 2 },
    // Fresh pages read zero, and are writable.
    Case { name: "grow_fresh", decl: "(memory 1 4)", body:
        "i32.const 1 memory.grow drop \
         i32.const 65536 i32.load \
         i32.const 70000 local.get 0 i32.store \
         i32.const 70000 i32.load i32.add local.get 1 i32.add", params: 2 },

    // The shape python leans on: a stack pointer taken down and put back.
    Case { name: "global_sp", decl: "(global $sp (mut i32) (i32.const 65536))", body:
        "(local i32) \
         global.get $sp i32.const 16 i32.sub local.tee 1 global.set $sp \
         local.get 1 local.get 0 i32.add \
         local.get 1 i32.const 16 i32.add global.set $sp", params: 1 },
];

pub fn run() -> bool {
    let mut fails = 0u32;
    let mut checks = 0u32;

    for c in CASES {
        let ps = "(param i32)".repeat(c.params);
        let src = format!(
            "(module {} (func (export \"f\") {ps} (result i32) {}))",
            c.decl, c.body
        );
        let wasm = match wat::parse_str(&src) {
            Ok(w) => w,
            Err(e) => {
                println!("  {:<12} wat: {e}", c.name);
                fails += 1;
                continue;
            }
        };

        let m = match forge_core::compile(&wasm) {
            Ok(m) => m,
            Err(e) => {
                println!("  {:<12} forge lehnt das MODUL ab: {e}", c.name);
                fails += 1;
                continue;
            }
        };
        // Which function is `f`, and did it compile? With calls in the picture
        // a module is more than one function, so both questions are real.
        let Some(&(_, fidx)) = m.plan.exports.iter().find(|(n, _)| n == "f") else {
            println!("  {:<12} kein Export f", c.name);
            fails += 1;
            continue;
        };
        let def = fidx as usize - m.plan.imported_funcs.len();
        if let Some(forge_core::codegen::Outcome::Unsupported(op)) = m.funcs.get(def) {
            println!("  {:<12} noch nicht gebaut: {op}", c.name);
            fails += 1;
            continue;
        }
        let Some(off) = m.offset_of(fidx) else {
            println!("  {:<12} keine Adresse fuer f", c.name);
            fails += 1;
            continue;
        };
        let Some(exec) = Exec::new(&m.code) else {
            println!("  {:<12} mmap fehlgeschlagen", c.name);
            fails += 1;
            continue;
        };
        exec.arm_traps(&m);
        let code_len = m.code.len();

        let mut bad = 0u32;
        for a in ARGS {
            let args = &a[..c.params];
            let want = match under_wasmi(&wasm, args) {
                Some(v) => v,
                None => {
                    println!("  {:<12} wasmi konnte nicht laufen", c.name);
                    bad += 1;
                    break;
                }
            };
            let Some(inst) = Inst::new(&m, exec.base()) else {
                println!("  {:<12} Instanz liess sich nicht bauen", c.name);
                bad += 1;
                break;
            };
            let got = exec.call_entry(m.entry_offset, off, inst.ptr(), a[0], a[1], a[2]);
            checks += 1;
            if inst.trap_code() != forge_core::trap::NONE {
                println!(
                    "  {:<12} args {args:?}: unerwarteter Trap ({})",
                    c.name,
                    forge_core::trap::name(inst.trap_code())
                );
                bad += 1;
                continue;
            }
            if got != want {
                println!(
                    "  {:<12} args {args:?}: wasmi {want} (0x{want:08x}) != forge {got} (0x{got:08x})",
                    c.name
                );
                bad += 1;
            }
        }
        if bad == 0 {
            println!("  {:<12} ok  ({code_len} B Code)", c.name);
        } else {
            fails += 1;
        }
    }

    let traps = traps_report_themselves();
    println!(
        "\n{} Faelle, {checks} Vergleiche, {fails} abweichend\n\
         Traps: {} — Wachseite, Tabelle, Signatur, Bulk-Raender, Division, \
         unreachable und Fuel melden sich mit dem RICHTIGEN Grund",
        CASES.len(),
        if traps { "melden sich" } else { "MELDEN SICH NICHT" }
    );
    fails == 0 && traps
}
