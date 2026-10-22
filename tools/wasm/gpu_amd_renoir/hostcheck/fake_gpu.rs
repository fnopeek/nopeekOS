// The fake GPU behind the doorbells, include!d by the host run: a command
// processor that runs the PM4 packets the driver's ring and IB tests
// write, an SDMA engine that runs its packets, and an IH that reports EOP
// and trap interrupts. GPU addresses below the VRAM size are VRAM offsets
// (the fake card's VRAM starts at 0); GART addresses go through the PTEs
// of the table the GC hub's context 0 points to.

use std::cell::RefCell;

fn gc(idx: u32, reg: u32) -> u32 {
    tables::GC_BASE[idx as usize] + reg
}

fn oss(idx: u32, reg: u32) -> u32 {
    tables::OSSSYS_BASE[idx as usize] + reg
}

fn sdma(reg: u32) -> u32 {
    tables::SDMA0_BASE[0] + reg
}

fn uvd(idx: u32, reg: u32) -> u32 {
    tables::UVD0_BASE[idx as usize] + reg
}

fn r(reg: u32) -> u32 {
    npk_sys::fake_reg(reg)
}

fn r64(lo: u32, hi: u32) -> u64 {
    r(lo) as u64 | (r(hi) as u64) << 32
}

/// GPU address to the bus address or VRAM offset behind it.
enum Mem {
    Vram(u64),
    Bus(u64),
}

fn translate(addr: u64) -> Mem {
    let vram = (VRAM_MB as u64) << 20;
    if addr < vram {
        return Mem::Vram(addr);
    }
    let start = (r(gc(GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32))
        as u64) << 12
        | (r(gc(GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32))
            as u64) << 44;
    let ptb = r64(gc(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32),
                  gc(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32))
        & 0x0000_ffff_ffff_f000;
    let at = ptb + ((addr - start) >> 12) * 8;
    let pte = npk_sys::vram_r32(at) as u64 | (npk_sys::vram_r32(at + 4) as u64) << 32;
    Mem::Bus((pte & 0x0000_ffff_ffff_f000) | (addr & 0xfff))
}

/// PM4 SET_SH_REG and DISPATCH_DIRECT (soc15d.h), and the SH registers a
/// dispatch reads, from SI_SH_REG_OFFSET in dwords.
const PKT3_SET_SH_REG: u32 = 0x76;
const PKT3_DISPATCH_DIRECT: u32 = 0x15;
const SH_COMPUTE_PGM_LO: u32 = 0x20c;
const SH_COMPUTE_PGM_HI: u32 = 0x20d;
const SH_COMPUTE_USER_DATA_0: u32 = 0x240;

thread_local! {
    /// The compute SH registers the command processor holds.
    static SH: RefCell<std::collections::HashMap<u32, u32>> = RefCell::new(std::collections::HashMap::new());
}

/// A dispatch on the compute ring. The fake runs one program, fill_pattern
/// (recognized by its code through the VM), so the dispatch self-test
/// proves the program address, push constants and grid end to end; any
/// other program is logged and does nothing.
fn run_dispatch(grid: [u32; 3]) {
    let sh = |r: u32| SH.with(|s| s.borrow().get(&r).copied().unwrap_or(0));
    let va = ((sh(SH_COMPUTE_PGM_HI) & 0xff) as u64) << 40 | (sh(SH_COMPUTE_PGM_LO) as u64) << 8;
    let code = &shaders_gen::FILL_PATTERN_CODE;
    let is_fill = (0..code.len() / 4).all(|i| {
        gpu_r32(va + i as u64 * 4) == u32::from_le_bytes(code[i * 4..i * 4 + 4].try_into().unwrap())
    });
    if !is_fill {
        log(format!("pm4 DISPATCH_DIRECT {:?} program {:#x}", grid, va));
        return;
    }
    let k = &shaders_gen::FILL_PATTERN;
    let u = |i: u32| sh(SH_COMPUTE_USER_DATA_0 + k.inline_push_sgpr + i);
    let push = |off: u32| u(off / 4);
    let dst = push(shaders_gen::FILL_PATTERN_PUSH_DST) as u64 | (push(shaders_gen::FILL_PATTERN_PUSH_DST + 4) as u64) << 32;
    let (w, h) = (push(shaders_gen::FILL_PATTERN_PUSH_WIDTH), push(shaders_gen::FILL_PATTERN_PUSH_HEIGHT));
    let (pitch, seed) = (push(shaders_gen::FILL_PATTERN_PUSH_PITCH_PX), push(shaders_gen::FILL_PATTERN_PUSH_SEED));
    for y in 0..(grid[1] * k.num_thread[1]).min(h) {
        for x in 0..(grid[0] * k.num_thread[0]).min(w) {
            gpu_w32(dst + (y as u64 * pitch as u64 + x as u64) * 4, seed << 24 | y << 12 | x);
        }
    }
    log(format!("pm4 DISPATCH_DIRECT {:?} fill_pattern {}x{} at {:#x}", grid, w, h, dst));
}

thread_local! {
    /// The VMID of the IB running now; 0 is the kernel's context.
    static VMID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// A VRAM physical address (`amdgpu_gmc_vram_mc2pa`) to its VRAM offset.
fn vram_pa_offset(pa: u64) -> u64 {
    pa - ((r(gc(GC_mmMC_VM_FB_OFFSET_BASE_IDX, GC_mmMC_VM_FB_OFFSET)) as u64) << 24)
}

/// A client address of VMID `vmid` through its four-level page table
/// (block size 9), as the GC hub's context registers point to it.
fn translate_vm(vmid: u32, addr: u64) -> Mem {
    let lo = gc(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32)
        + GC_CTX_ADDR_DISTANCE * vmid;
    let hi = gc(GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_HI32)
        + GC_CTX_ADDR_DISTANCE * vmid;
    let mut table = vram_pa_offset(r64(lo, hi) & 0x0000_ffff_ffff_f000);
    let page = (addr & 0x0000_ffff_ffff_ffff) >> 12;
    for shift in [27u32, 18, 9, 0] {
        let at = table + ((page >> shift) & 511) * 8;
        let e = npk_sys::vram_r32(at) as u64 | (npk_sys::vram_r32(at + 4) as u64) << 32;
        if e & 1 == 0 {
            panic!("vmid {vmid} fault at {addr:#x}: entry {e:#x} at level shift {shift}");
        }
        let base = e & 0x0000_ffff_ffff_f000;
        if shift == 0 || e & AMDGPU_PDE_PTE != 0 {
            let pa = base + ((page & ((1u64 << shift) - 1)) << 12) + (addr & 0xfff);
            return if e & AMDGPU_PTE_SYSTEM as u64 != 0 { Mem::Bus(pa) } else { Mem::Vram(vram_pa_offset(pa)) };
        }
        table = vram_pa_offset(base);
    }
    unreachable!()
}

fn gpu_r32(addr: u64) -> u32 {
    let vmid = VMID.with(|v| v.get());
    if vmid != 0 {
        return match translate_vm(vmid, addr) {
            Mem::Vram(o) => npk_sys::vram_r32(o),
            Mem::Bus(b) => npk_sys::bus_r32(b),
        };
    }
    match translate(addr) {
        Mem::Vram(o) => npk_sys::vram_r32(o),
        Mem::Bus(b) => npk_sys::bus_r32(b),
    }
}

fn gpu_w32(addr: u64, v: u32) {
    let vmid = VMID.with(|c| c.get());
    if vmid != 0 {
        match translate_vm(vmid, addr) {
            Mem::Vram(o) => npk_sys::vram_w32(o, v),
            Mem::Bus(b) => npk_sys::bus_w32(b, v),
        }
        return;
    }
    match translate(addr) {
        Mem::Vram(o) => npk_sys::vram_w32(o, v),
        Mem::Bus(b) => npk_sys::bus_w32(b, v),
    }
}

/// A queue the fake GPU knows.
#[derive(Clone, Copy)]
struct Queue {
    kind: Kind,
    base: u64,
    size_dw: u32,
    rptr: u32,
    rptr_report: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Gfx,
    Compute { me: u32, pipe: u32, queue: u32 },
    Sdma,
}

thread_local! {
    /// The KCQs MAP_QUEUES announced, by doorbell index.
    static KCQ: RefCell<Vec<(u32, Queue)>> = const { RefCell::new(Vec::new()) };
    static QUEUES: RefCell<Vec<(u32, Queue)>> = const { RefCell::new(Vec::new()) };
    static IH_WPTR: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Packets run, for the log.
    pub static PACKETS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn field(v: u32, mask: u32, shift: u32) -> u32 {
    (v & mask) >> shift
}

/// The GRBM_GFX_CNTL value that selects a queue's HQD registers.
fn grbm_bank(me: u32, pipe: u32, queue: u32) -> u32 {
    pipe << GC_GRBM_GFX_CNTL__PIPEID__SHIFT | me << GC_GRBM_GFX_CNTL__MEID__SHIFT
        | queue << GC_GRBM_GFX_CNTL__QUEUEID__SHIFT
}

/// The queue a doorbell index rings, as the registers and MAP_QUEUES left it.
fn queue_for(index: u32) -> Option<Queue> {
    if let Some(q) = QUEUES.with(|q| q.borrow().iter().find(|e| e.0 == index).map(|e| e.1)) {
        return Some(q);
    }
    let rb_db = r(gc(GC_mmCP_RB_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_RB_DOORBELL_CONTROL));
    if rb_db & GC_CP_RB_DOORBELL_CONTROL__DOORBELL_EN_MASK != 0
        && field(rb_db, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_OFFSET_MASK, GC_CP_RB_DOORBELL_CONTROL__DOORBELL_OFFSET__SHIFT)
            == index
    {
        let cntl = r(gc(GC_mmCP_RB0_CNTL_BASE_IDX, GC_mmCP_RB0_CNTL));
        let bufsz = field(cntl, GC_CP_RB0_CNTL__RB_BUFSZ_MASK, GC_CP_RB0_CNTL__RB_BUFSZ__SHIFT);
        return Some(Queue {
            kind: Kind::Gfx,
            base: r64(gc(GC_mmCP_RB0_BASE_BASE_IDX, GC_mmCP_RB0_BASE), gc(GC_mmCP_RB0_BASE_HI_BASE_IDX, GC_mmCP_RB0_BASE_HI))
                << 8,
            size_dw: 2 << bufsz,
            rptr: 0,
            rptr_report: r64(gc(GC_mmCP_RB0_RPTR_ADDR_BASE_IDX, GC_mmCP_RB0_RPTR_ADDR),
                             gc(GC_mmCP_RB0_RPTR_ADDR_HI_BASE_IDX, GC_mmCP_RB0_RPTR_ADDR_HI)),
        });
    }
    // A queue the driver programmed through its HQD registers (the KIQ);
    // flat, they are the KIQ's.
    let kiq = grbm_bank(2, 1, 0);
    let banks = if npk_sys::fake_is_banked() { npk_sys::fake_banks() } else { vec![kiq] };
    for bank in banks {
        let br = |idx: u32, reg: u32| {
            if npk_sys::fake_is_banked() { npk_sys::fake_bank_reg(bank, gc(idx, reg)) } else { r(gc(idx, reg)) }
        };
        let hqd_db = br(GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL);
        if br(GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE) & 1 == 0
            || field(hqd_db, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET_MASK,
                     GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET__SHIFT) != index
        {
            continue;
        }
        let cntl = br(GC_mmCP_HQD_PQ_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_CONTROL);
        let qs = field(cntl, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE_MASK, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE__SHIFT);
        let b64 = |lo: (u32, u32), hi: (u32, u32)| br(lo.0, lo.1) as u64 | (br(hi.0, hi.1) as u64) << 32;
        return Some(Queue {
            kind: Kind::Compute {
                me: field(bank, GC_GRBM_GFX_CNTL__MEID_MASK, GC_GRBM_GFX_CNTL__MEID__SHIFT),
                pipe: field(bank, GC_GRBM_GFX_CNTL__PIPEID_MASK, GC_GRBM_GFX_CNTL__PIPEID__SHIFT),
                queue: field(bank, GC_GRBM_GFX_CNTL__QUEUEID_MASK, GC_GRBM_GFX_CNTL__QUEUEID__SHIFT),
            },
            base: b64((GC_mmCP_HQD_PQ_BASE_BASE_IDX, GC_mmCP_HQD_PQ_BASE),
                      (GC_mmCP_HQD_PQ_BASE_HI_BASE_IDX, GC_mmCP_HQD_PQ_BASE_HI)) << 8,
            size_dw: 2 << qs,
            rptr: 0,
            rptr_report: b64((GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR),
                             (GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI)),
        });
    }
    if let Some(q) = KCQ.with(|k| k.borrow().iter().find(|e| e.0 == index).map(|e| e.1)) {
        return Some(q);
    }
    let sd = r(sdma(SDMA0_mmSDMA0_GFX_DOORBELL));
    let off = r(sdma(SDMA0_mmSDMA0_GFX_DOORBELL_OFFSET));
    if sd & SDMA0_SDMA0_GFX_DOORBELL__ENABLE_MASK != 0
        && field(off, SDMA0_SDMA0_GFX_DOORBELL_OFFSET__OFFSET_MASK, SDMA0_SDMA0_GFX_DOORBELL_OFFSET__OFFSET__SHIFT) == index
    {
        let cntl = r(sdma(SDMA0_mmSDMA0_GFX_RB_CNTL));
        let sz = field(cntl, SDMA0_SDMA0_GFX_RB_CNTL__RB_SIZE_MASK, SDMA0_SDMA0_GFX_RB_CNTL__RB_SIZE__SHIFT);
        return Some(Queue {
            kind: Kind::Sdma,
            base: r64(sdma(SDMA0_mmSDMA0_GFX_RB_BASE), sdma(SDMA0_mmSDMA0_GFX_RB_BASE_HI)) << 8,
            size_dw: 1 << sz,
            rptr: 0,
            rptr_report: r64(sdma(SDMA0_mmSDMA0_GFX_RB_RPTR_ADDR_LO), sdma(SDMA0_mmSDMA0_GFX_RB_RPTR_ADDR_HI)),
        });
    }
    None
}

/// One IV into the IH ring 0, then the interrupt.
fn ih_post(client: u32, src: u32, ring_id: u32) {
    let base = r64(oss(OSS_mmIH_RB_BASE_BASE_IDX, OSS_mmIH_RB_BASE), oss(OSS_mmIH_RB_BASE_HI_BASE_IDX, OSS_mmIH_RB_BASE_HI))
        << 8;
    let cntl = r(oss(OSS_mmIH_RB_CNTL_BASE_IDX, OSS_mmIH_RB_CNTL));
    if cntl & OSS_IH_RB_CNTL__RB_ENABLE_MASK == 0 {
        return;
    }
    let size = 4u32 << field(cntl, OSS_IH_RB_CNTL__RB_SIZE_MASK, OSS_IH_RB_CNTL__RB_SIZE__SHIFT);
    let wb = r64(oss(OSS_mmIH_RB_WPTR_ADDR_LO_BASE_IDX, OSS_mmIH_RB_WPTR_ADDR_LO),
                 oss(OSS_mmIH_RB_WPTR_ADDR_HI_BASE_IDX, OSS_mmIH_RB_WPTR_ADDR_HI));
    let w = IH_WPTR.with(|w| w.get());
    let iv = [client | src << 8 | ring_id << 16, 0, 0, 0, 0, 0, 0, 0];
    for (i, v) in iv.iter().enumerate() {
        npk_sys::bus_w32(base + w as u64 + i as u64 * 4, *v);
    }
    let w = (w + 32) % size;
    IH_WPTR.with(|x| x.set(w));
    npk_sys::bus_w32(wb, w);
    npk_sys::fake_irq();
}

fn log(s: String) {
    PACKETS.with(|p| p.borrow_mut().push(s));
}

/// Runs the PM4 packets in `[at, end)` dwords of a buffer read by `rd`;
/// the dword it stalls at, if a client's wait on memory is never met.
fn run_pm4(kind: Kind, rd: &dyn Fn(u32) -> u32, mut at: u32, end: u32, wrap: u32) -> Option<u32> {
    while at != end {
        let hdr = rd(at % wrap);
        let op = (hdr >> 8) & 0xff;
        let count = (hdr >> 16) & 0x3fff;
        if hdr >> 30 != 3 {
            log(format!("pm4 type {} header {:#x}", hdr >> 30, hdr));
            at = (at + 1) % wrap;
            continue;
        }
        let len = if op == PACKET3_NOP && count == 0x3fff { 1 } else { count + 2 };
        let d = |i: u32| rd((at + i) % wrap);
        match op {
            PACKET3_NOP => {}
            PACKET3_SET_UCONFIG_REG => {
                let reg = (d(1) & 0xffff) + PACKET3_SET_UCONFIG_REG_START;
                for i in 0..count {
                    npk_sys::fake_set_reg(reg + i, d(2 + i));
                }
                log(format!("pm4 SET_UCONFIG_REG {:#x} = {:#x}", reg, d(2)));
            }
            PACKET3_WRITE_DATA => {
                let dst = (d(1) >> 8) & 0xf;
                let addr = d(2) as u64 | (d(3) as u64) << 32;
                for i in 0..count - 2 {
                    if dst == 0 {
                        npk_sys::fake_set_reg(d(2) + i, d(4 + i));
                    } else {
                        gpu_w32(addr + i as u64 * 4, d(4 + i));
                    }
                }
                log(format!("pm4 WRITE_DATA dst {} {:#x} = {:#x}", dst, addr, d(4)));
            }
            PACKET3_RELEASE_MEM => {
                let addr = d(3) as u64 | (d(4) as u64) << 32;
                let data_sel = d(2) >> 29;
                let int_sel = (d(2) >> 24) & 7;
                gpu_w32(addr, d(5));
                if data_sel == 2 {
                    gpu_w32(addr + 4, d(6));
                }
                log(format!("pm4 RELEASE_MEM {:#x} = {} int {}", addr, d(5), int_sel));
                if int_sel != 0 {
                    let ring_id = match kind {
                        Kind::Compute { me, pipe, queue } => queue << 4 | me << 2 | pipe,
                        _ => 0,
                    };
                    ih_post(SOC15_IH_CLIENTID_GRBM_CP, GFX_9_0__SRCID__CP_EOP_INTERRUPT, ring_id);
                }
            }
            PKT3_SET_SH_REG => {
                for i in 0..count {
                    SH.with(|s| s.borrow_mut().insert(d(1) + i, d(2 + i)));
                }
            }
            PKT3_DISPATCH_DIRECT => run_dispatch([d(1), d(2), d(3)]),
            PACKET3_WAIT_REG_MEM => {
                let mem = (d(1) >> 4) & 1 != 0;
                if (d(1) >> 6) & 1 != 0 {
                    // write reg0, then wait on reg1
                    npk_sys::fake_set_reg(d(2), d(4));
                    let v = npk_sys::fake_reg(d(3));
                    log(format!("pm4 WAIT_REG_MEM write {:#x} = {:#x}, wait {:#x}{}", d(2), d(4), d(3),
                                if v & d(5) == d(5) & d(4) { "" } else { " (unmet)" }));
                } else {
                    let v = if mem { gpu_r32(d(2) as u64 | (d(3) as u64) << 32) } else { npk_sys::fake_reg(d(2)) };
                    let met = v & d(5) == d(4);
                    if !met && mem && VMID.with(|v| v.get()) != 0 {
                        log(format!("pm4 WAIT_REG_MEM mem {:#x} == {:#x}: the queue hangs", d(2), d(4)));
                        return Some(at);
                    }
                    log(format!("pm4 WAIT_REG_MEM {} {:#x} == {:#x}{}", if mem { "mem" } else { "reg" }, d(2), d(4),
                                if met { "" } else { " (unmet)" }));
                }
            }
            PACKET3_INDIRECT_BUFFER => {
                let ib = d(1) as u64 | (d(2) as u64) << 32;
                let n = d(3) & 0xfffff;
                let vmid = (d(3) >> 24) & 0xf;
                log(format!("pm4 INDIRECT_BUFFER {:#x} {} dw vmid {}", ib, n, vmid));
                let outer = VMID.with(|v| v.replace(vmid));
                let stall = run_pm4(kind, &|i| gpu_r32(ib + i as u64 * 4), 0, n, u32::MAX);
                VMID.with(|v| v.set(outer));
                if stall.is_some() {
                    return Some(at);
                }
            }
            PACKET3_MAP_QUEUES => {
                let doorbell = (d(2) >> 2) & 0x3ffffff;
                let mqd = d(3) as u64 | (d(4) as u64) << 32;
                let m = |off: u32| gpu_r32(mqd + off as u64);
                let qs = field(m(V9_MQD_cp_hqd_pq_control), GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE_MASK,
                               GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE__SHIFT);
                let q = Queue {
                    kind: Kind::Compute { me: if (d(1) >> 18) & 3 == 0 { 1 } else { 2 }, pipe: (d(1) >> 16) & 3,
                                          queue: (d(1) >> 13) & 7 },
                    base: (m(V9_MQD_cp_hqd_pq_base_lo) as u64 | (m(V9_MQD_cp_hqd_pq_base_hi) as u64) << 32) << 8,
                    size_dw: 2 << qs,
                    rptr: 0,
                    rptr_report: m(V9_MQD_cp_hqd_pq_rptr_report_addr_lo) as u64
                        | (m(V9_MQD_cp_hqd_pq_rptr_report_addr_hi) as u64) << 32,
                };
                if let Kind::Compute { me, pipe, queue } = q.kind {
                    npk_sys::fake_bank_set(grbm_bank(me, pipe, queue), gc(GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE), 1);
                }
                KCQ.with(|k| k.borrow_mut().push((doorbell, q)));
                log(format!("pm4 MAP_QUEUES doorbell {:#x}", doorbell));
            }
            PACKET3_UNMAP_QUEUES => {
                let doorbell = (d(2) >> 2) & 0x3ffffff;
                let gone = KCQ.with(|k| k.borrow().iter().find(|e| e.0 == doorbell).map(|e| e.1.kind));
                if let Some(Kind::Compute { me, pipe, queue }) = gone {
                    npk_sys::fake_bank_set(grbm_bank(me, pipe, queue), gc(GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE), 0);
                }
                KCQ.with(|k| k.borrow_mut().retain(|e| e.0 != doorbell));
                QUEUES.with(|q| q.borrow_mut().retain(|e| e.0 != doorbell));
                log(format!("pm4 UNMAP_QUEUES doorbell {:#x}", doorbell));
            }
            other => log(format!("pm4 op {:#x} ({} dw)", other, len)),
        }
        at = if wrap == u32::MAX { at + len } else { (at + len) % wrap };
    }
    None
}

/// Runs the SDMA packets in `[at, end)` dwords.
fn run_sdma(rd: &dyn Fn(u32) -> u32, mut at: u32, end: u32, wrap: u32) {
    while at != end {
        if wrap == u32::MAX && at > end {
            panic!("sdma IB packets run past the IB's end ({at} > {end})");
        }
        let hdr = rd(at % wrap);
        let d = |i: u32| rd((at + i) % wrap);
        let (op, sub) = (hdr & 0xff, (hdr >> 8) & 0xff);
        let len = match (op, sub) {
            (0, _) => 1 + ((hdr >> 16) & 0x3fff),
            (2, 0) => {
                let addr = d(1) as u64 | (d(2) as u64) << 32;
                let n = d(3) + 1;
                for i in 0..n {
                    gpu_w32(addr + i as u64 * 4, d(4 + i));
                }
                log(format!("sdma WRITE_LINEAR {:#x} = {:#x}", addr, d(4)));
                4 + n
            }
            (1, 0) => {
                let bytes = d(1) + 1;
                let src = d(3) as u64 | (d(4) as u64) << 32;
                let dst = d(5) as u64 | (d(6) as u64) << 32;
                for i in 0..bytes.div_ceil(4) as u64 {
                    gpu_w32(dst + i * 4, gpu_r32(src + i * 4));
                }
                log(format!("sdma COPY_LINEAR {:#x} -> {:#x} {} bytes", src, dst, bytes));
                7
            }
            (4, _) => {
                let ib = d(1) as u64 | (d(2) as u64) << 32;
                let n = d(3);
                log(format!("sdma INDIRECT {:#x} {} dw", ib, n));
                run_sdma(&|i| gpu_r32(ib + i as u64 * 4), 0, n, u32::MAX);
                6
            }
            (5, _) => {
                let addr = d(1) as u64 | (d(2) as u64) << 32;
                gpu_w32(addr, d(3));
                log(format!("sdma FENCE {:#x} = {}", addr, d(3)));
                4
            }
            (8, _) => {
                // POLL_REGMEM, function == only
                let mem = (hdr >> 31) & 1 != 0;
                let (a0, a1, refv, mask) = (d(1), d(2), d(3), d(4));
                let v = if mem { gpu_r32(a0 as u64 | (a1 as u64) << 32) } else { npk_sys::fake_reg(a0 >> 2) };
                log(format!("sdma POLL_REGMEM {} {:#x} == {:#x}{}", if mem { "mem" } else { "reg" },
                            if mem { a0 as u64 | (a1 as u64) << 32 } else { (a0 >> 2) as u64 }, refv,
                            if v & mask == refv { "" } else { " (unmet)" }));
                6
            }
            (14, _) => {
                npk_sys::fake_set_reg(d(1), d(2));
                log(format!("sdma SRBM_WRITE {:#x} = {:#x}", d(1), d(2)));
                3
            }
            // CONST_FILL: address, data, byte count - 1.
            (11, _) => {
                let addr = d(1) as u64 | (d(2) as u64) << 32;
                for i in 0..(d(4) as u64 + 1) / 4 {
                    gpu_w32(addr + i * 4, d(3));
                }
                log(format!("sdma const_fill {} bytes at {:#x}", d(4) + 1, addr));
                5
            }
            (12, _) => {
                // PTEPDE: count entries (addr + i * incr) | flags
                let pe = d(1) as u64 | (d(2) as u64) << 32;
                let flags = d(3) as u64 | (d(4) as u64) << 32;
                let addr = d(5) as u64 | (d(6) as u64) << 32;
                let (incr, count) = (d(7) as u64, d(9) + 1);
                for i in 0..count as u64 {
                    let v = (addr + i * incr) | flags;
                    gpu_w32(pe + i * 8, v as u32);
                    gpu_w32(pe + i * 8 + 4, (v >> 32) as u32);
                }
                log(format!("sdma PTEPDE {:#x} x{} = {:#x} | {:#x} + {:#x}", pe, count, addr, flags, incr));
                10
            }
            (6, _) => {
                log("sdma TRAP".to_string());
                ih_post(SOC15_IH_CLIENTID_SDMA0, SDMA0_4_0__SRCID__SDMA_TRAP, 0);
                2
            }
            _ => {
                log(format!("sdma op {} sub {}", op, sub));
                1
            }
        };
        at = if wrap == u32::MAX { at + len } else { (at + len) % wrap };
    }
}

/// VCN's rings by the doorbell range NBIO gives it: the decoder, then the
/// two encode rings two and three dwords on.
fn vcn_ring_for(index: u32) -> Option<u32> {
    let range = r(tables::NBIF0_BASE[mmBIF_MMSCH0_DOORBELL_RANGE_BASE_IDX as usize] + mmBIF_MMSCH0_DOORBELL_RANGE);
    if field(range, BIF_MMSCH0_DOORBELL_RANGE__SIZE_MASK, BIF_MMSCH0_DOORBELL_RANGE__SIZE__SHIFT) == 0 {
        return None;
    }
    let base = field(range, BIF_MMSCH0_DOORBELL_RANGE__OFFSET_MASK, BIF_MMSCH0_DOORBELL_RANGE__OFFSET__SHIFT);
    match index.checked_sub(base)? {
        0 => Some(0),
        2 => Some(1),
        3 => Some(2),
        _ => None,
    }
}

/// A 40-bit address of the VCPU in the 48-bit space of context 0: the hub
/// supplies the top bits of the GART aperture it lies in.
fn vcn_addr(a: u64) -> u64 {
    let start = (r(gc(GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_LO32))
        as u64) << 12
        | (r(gc(GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32_BASE_IDX, GC_mmVM_CONTEXT0_PAGE_TABLE_START_ADDR_HI32))
            as u64) << 44;
    if a >> 32 == (start >> 32) & 0xff { (start & !0xff_ffff_ffff) | a } else { a }
}

/// The VCN decoder's ring from `from` to `wptr` (dwords): PACKET0 register
/// writes to the VCPU's internal registers. The scratch one shows outside,
/// a fence command writes the context id to the address in DATA0/DATA1, a
/// trap raises the decoder's interrupt.
fn run_vcn_dec(base: u64, size_dw: u32, from: u32, wptr: u32) {
    let (mut data0, mut data1, mut ctx) = (0u32, 0u32, 0u32);
    let mut at = from;
    while at != wptr {
        let hdr = gpu_r32(base + at as u64 * 4);
        let v = gpu_r32(base + ((at + 1) % size_dw) as u64 * 4);
        let reg = hdr & 0xffff;
        match reg {
            mmUVD_SCRATCH9_INTERNAL_OFFSET => npk_sys::fake_set_reg(uvd(mmUVD_SCRATCH9_BASE_IDX, mmUVD_SCRATCH9), v),
            mmUVD_CONTEXT_ID_INTERNAL_OFFSET => ctx = v,
            mmUVD_GPCOM_VCPU_DATA0_INTERNAL_OFFSET => data0 = v,
            mmUVD_GPCOM_VCPU_DATA1_INTERNAL_OFFSET => data1 = v,
            mmUVD_GPCOM_VCPU_CMD_INTERNAL_OFFSET if v == VCN_DEC_KMD_CMD | VCN_DEC_CMD_FENCE << 1 => {
                let addr = vcn_addr(data0 as u64 | (data1 as u64) << 32);
                gpu_w32(addr, ctx);
                log(format!("vcn_dec FENCE {:#x} = {}", addr, ctx));
            }
            mmUVD_GPCOM_VCPU_CMD_INTERNAL_OFFSET if v == VCN_DEC_KMD_CMD | VCN_DEC_CMD_TRAP << 1 => {
                log("vcn_dec TRAP".to_string());
                ih_post(SOC15_IH_CLIENTID_VCN, VCN_2_0__SRCID__UVD_SYSTEM_MESSAGE_INTERRUPT, 0);
            }
            _ => {}
        }
        at = (at + 2) % size_dw;
    }
    npk_sys::fake_set_reg(uvd(mmUVD_RBC_RB_RPTR_BASE_IDX, mmUVD_RBC_RB_RPTR), wptr);
}

/// A VCN encode ring from `from` to `wptr`: END, NO_OP, IB (not run),
/// register writes and waits; FENCE writes its sequence number, TRAP raises
/// the ring's interrupt.
fn run_vcn_enc(ring: u32, base: u64, size_dw: u32, from: u32, wptr: u32) {
    let d = |i: u32| gpu_r32(base + (i % size_dw) as u64 * 4);
    let mut at = from;
    while at != wptr {
        let len = match d(at) {
            VCN_ENC_CMD_FENCE => {
                let addr = d(at + 1) as u64 | (d(at + 2) as u64) << 32;
                gpu_w32(addr, d(at + 3));
                log(format!("vcn_enc{} FENCE {:#x} = {}", ring - 1, addr, d(at + 3)));
                4
            }
            VCN_ENC_CMD_TRAP => {
                let src = if ring == 1 { VCN_2_0__SRCID__UVD_ENC_GENERAL_PURPOSE } else { VCN_2_0__SRCID__UVD_ENC_LOW_LATENCY };
                log(format!("vcn_enc{} TRAP", ring - 1));
                ih_post(SOC15_IH_CLIENTID_VCN, src, 0);
                1
            }
            VCN_ENC_CMD_IB => 5,
            VCN_ENC_CMD_REG_WRITE => 3,
            VCN_ENC_CMD_REG_WAIT => 4,
            _ => 1,
        };
        at = (at + len) % size_dw;
    }
    let rptr = if ring == 1 { uvd(mmUVD_RB_RPTR_BASE_IDX, mmUVD_RB_RPTR) } else { uvd(mmUVD_RB_RPTR2_BASE_IDX, mmUVD_RB_RPTR2) };
    npk_sys::fake_set_reg(rptr, wptr);
}

thread_local! {
    /// Where each VCN ring was run to.
    static VCN_RPTR: RefCell<[u32; 3]> = const { RefCell::new([0; 3]) };
}

/// A VCN doorbell: the ring runs to the write pointer it hands over.
fn vcn_doorbell(ring: u32, value: u64) {
    let wptr = value as u32;
    // The doorbell is the ring's write pointer register.
    let wreg = match ring {
        0 => uvd(mmUVD_RBC_RB_WPTR_BASE_IDX, mmUVD_RBC_RB_WPTR),
        1 => uvd(mmUVD_RB_WPTR_BASE_IDX, mmUVD_RB_WPTR),
        _ => uvd(mmUVD_RB_WPTR2_BASE_IDX, mmUVD_RB_WPTR2),
    };
    npk_sys::fake_set_reg(wreg, wptr);
    let from = VCN_RPTR.with(|v| v.borrow()[ring as usize]);
    if ring == 0 {
        let base = r64(uvd(mmUVD_LMI_RBC_RB_64BIT_BAR_LOW_BASE_IDX, mmUVD_LMI_RBC_RB_64BIT_BAR_LOW),
                       uvd(mmUVD_LMI_RBC_RB_64BIT_BAR_HIGH_BASE_IDX, mmUVD_LMI_RBC_RB_64BIT_BAR_HIGH));
        let cntl = r(uvd(mmUVD_RBC_RB_CNTL_BASE_IDX, mmUVD_RBC_RB_CNTL));
        let size_dw = (1 << field(cntl, UVD_RBC_RB_CNTL__RB_BUFSZ_MASK, UVD_RBC_RB_CNTL__RB_BUFSZ__SHIFT)) / 4;
        run_vcn_dec(base, size_dw.max(1), from, wptr % size_dw.max(1));
    } else {
        let (lo, hi, sz) = if ring == 1 {
            (uvd(mmUVD_RB_BASE_LO_BASE_IDX, mmUVD_RB_BASE_LO), uvd(mmUVD_RB_BASE_HI_BASE_IDX, mmUVD_RB_BASE_HI),
             uvd(mmUVD_RB_SIZE_BASE_IDX, mmUVD_RB_SIZE))
        } else {
            (uvd(mmUVD_RB_BASE_LO2_BASE_IDX, mmUVD_RB_BASE_LO2), uvd(mmUVD_RB_BASE_HI2_BASE_IDX, mmUVD_RB_BASE_HI2),
             uvd(mmUVD_RB_SIZE2_BASE_IDX, mmUVD_RB_SIZE2))
        };
        let size_dw = r(sz).max(1);
        run_vcn_enc(ring, r64(lo, hi), size_dw, from, wptr % size_dw);
    }
    VCN_RPTR.with(|v| v.borrow_mut()[ring as usize] = wptr);
}

/// A doorbell rang: run the queue up to the new write pointer.
fn doorbell(index: u32, value: u64) {
    if let Some(ring) = vcn_ring_for(index) {
        return vcn_doorbell(ring, value);
    }
    let Some(mut q) = queue_for(index) else {
        log(format!("doorbell {:#x} = {:#x}: no queue", index, value));
        return;
    };
    let wptr = if q.kind == Kind::Sdma { (value >> 2) as u32 } else { value as u32 } % q.size_dw;
    let base = q.base;
    let rd = move |i: u32| gpu_r32(base + i as u64 * 4);
    let stall = match q.kind {
        Kind::Sdma => {
            run_sdma(&rd, q.rptr, wptr, q.size_dw);
            None
        }
        k => run_pm4(k, &rd, q.rptr, wptr, q.size_dw),
    };
    q.rptr = stall.unwrap_or(wptr);
    gpu_w32(q.rptr_report, if q.kind == Kind::Sdma { q.rptr << 2 } else { q.rptr });
    QUEUES.with(|qs| {
        let mut qs = qs.borrow_mut();
        qs.retain(|e| e.0 != index);
        qs.push((index, q));
    });
}

/// The fake GPU listens from here on.
fn fake_gpu() {
    npk_sys::fake_doorbell_hook(doorbell);
    npk_sys::fake_write_hook(card_write);
}

thread_local! {
    /// The SMU message that is a MODE2 reset (GfxDeviceDriverReset).
    static MODE2_MSG: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
}

/// What VCN's own state machines do with a register write: the power
/// FSM reports the state it was configured to; a DPG pause request is
/// acknowledged with the tiles on, its release with them off again; a
/// ring's read pointer reset starts the fake from there.
fn vcn_write(reg: u32, v: u32) {
    if reg == uvd(mmUVD_PGFSM_CONFIG_BASE_IDX, mmUVD_PGFSM_CONFIG) {
        npk_sys::fake_set_reg(uvd(mmUVD_PGFSM_STATUS_BASE_IDX, mmUVD_PGFSM_STATUS), v);
    } else if reg == uvd(mmUVD_DPG_PAUSE_BASE_IDX, mmUVD_DPG_PAUSE) {
        let ps = uvd(mmUVD_POWER_STATUS_BASE_IDX, mmUVD_POWER_STATUS);
        let st = r(ps) & !UVD_POWER_STATUS__UVD_POWER_STATUS_MASK;
        if v & UVD_DPG_PAUSE__NJ_PAUSE_DPG_REQ_MASK != 0 {
            npk_sys::fake_set_reg(reg, v | UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK);
            npk_sys::fake_set_reg(ps, st | UVD_PGFSM_CONFIG__UVDM_UVDU_PWR_ON);
        } else {
            npk_sys::fake_set_reg(reg, v & !UVD_DPG_PAUSE__NJ_PAUSE_DPG_ACK_MASK);
            npk_sys::fake_set_reg(ps, st | UVD_POWER_STATUS__UVD_POWER_STATUS_TILES_OFF);
        }
    } else if reg == uvd(mmUVD_RBC_RB_RPTR_BASE_IDX, mmUVD_RBC_RB_RPTR) {
        VCN_RPTR.with(|x| x.borrow_mut()[0] = v);
    } else if reg == uvd(mmUVD_RB_RPTR_BASE_IDX, mmUVD_RB_RPTR) {
        VCN_RPTR.with(|x| x.borrow_mut()[1] = v);
    } else if reg == uvd(mmUVD_RB_RPTR2_BASE_IDX, mmUVD_RB_RPTR2) {
        VCN_RPTR.with(|x| x.borrow_mut()[2] = v);
    }
}

/// A MODE2 reset through the SMU mailbox drops every queue the CP had.
fn card_write(reg: u32, v: u32) {
    vcn_write(reg, v);
    if reg == 0x16000 + mmMP1_SMN_C2PMSG_66 && v == MODE2_MSG.with(|m| m.get()) {
        QUEUES.with(|q| q.borrow_mut().clear());
        KCQ.with(|k| k.borrow_mut().clear());
        for bank in npk_sys::fake_banks() {
            npk_sys::fake_bank_set(bank, gc(GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE), 0);
        }
        log("MODE2 reset: the CP forgets its queues".to_string());
    }
}

/// Reset behavior: `mode2` is the SMU's index of GfxDeviceDriverReset.
fn fake_gpu_mode2(mode2: u32) {
    MODE2_MSG.with(|m| m.set(mode2));
    npk_sys::fake_write_hook(card_write);
}

/// The HQD registers once per queue, as GRBM_GFX_CNTL selects them. The
/// register checks leave them flat: Linux's oracle reads them so.
fn fake_gpu_hqd_banks() {
    npk_sys::fake_card_banked(gc(GC_mmGRBM_GFX_CNTL_BASE_IDX, GC_mmGRBM_GFX_CNTL),
                              gc(GC_mmCP_MQD_BASE_ADDR_BASE_IDX, GC_mmCP_MQD_BASE_ADDR),
                              gc(GC_mmCP_HQD_PQ_WPTR_HI_BASE_IDX, GC_mmCP_HQD_PQ_WPTR_HI));
}
