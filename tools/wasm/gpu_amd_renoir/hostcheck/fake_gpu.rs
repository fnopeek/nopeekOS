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

fn gpu_r32(addr: u64) -> u32 {
    match translate(addr) {
        Mem::Vram(o) => npk_sys::vram_r32(o),
        Mem::Bus(b) => npk_sys::bus_r32(b),
    }
}

fn gpu_w32(addr: u64, v: u32) {
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
    let hqd_db = r(gc(GC_mmCP_HQD_PQ_DOORBELL_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_DOORBELL_CONTROL));
    if r(gc(GC_mmCP_HQD_ACTIVE_BASE_IDX, GC_mmCP_HQD_ACTIVE)) & 1 != 0
        && field(hqd_db, GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET_MASK,
                 GC_CP_HQD_PQ_DOORBELL_CONTROL__DOORBELL_OFFSET__SHIFT) == index
    {
        let cntl = r(gc(GC_mmCP_HQD_PQ_CONTROL_BASE_IDX, GC_mmCP_HQD_PQ_CONTROL));
        let qs = field(cntl, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE_MASK, GC_CP_HQD_PQ_CONTROL__QUEUE_SIZE__SHIFT);
        let gfx_cntl = r(gc(GC_mmGRBM_GFX_CNTL_BASE_IDX, GC_mmGRBM_GFX_CNTL));
        let _ = gfx_cntl;
        return Some(Queue {
            kind: Kind::Compute { me: 2, pipe: 1, queue: 0 },
            base: r64(gc(GC_mmCP_HQD_PQ_BASE_BASE_IDX, GC_mmCP_HQD_PQ_BASE),
                      gc(GC_mmCP_HQD_PQ_BASE_HI_BASE_IDX, GC_mmCP_HQD_PQ_BASE_HI)) << 8,
            size_dw: 2 << qs,
            rptr: 0,
            rptr_report: r64(gc(GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR),
                             gc(GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI_BASE_IDX, GC_mmCP_HQD_PQ_RPTR_REPORT_ADDR_HI)),
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

/// Runs the PM4 packets in `[at, end)` dwords of a buffer read by `rd`.
fn run_pm4(kind: Kind, rd: &dyn Fn(u32) -> u32, mut at: u32, end: u32, wrap: u32) {
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
                    log(format!("pm4 WAIT_REG_MEM {} {:#x} == {:#x}{}", if mem { "mem" } else { "reg" }, d(2), d(4),
                                if v & d(5) == d(4) { "" } else { " (unmet)" }));
                }
            }
            PACKET3_INDIRECT_BUFFER => {
                let ib = d(1) as u64 | (d(2) as u64) << 32;
                let n = d(3) & 0xfffff;
                log(format!("pm4 INDIRECT_BUFFER {:#x} {} dw", ib, n));
                run_pm4(kind, &|i| gpu_r32(ib + i as u64 * 4), 0, n, u32::MAX);
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
                KCQ.with(|k| k.borrow_mut().push((doorbell, q)));
                log(format!("pm4 MAP_QUEUES doorbell {:#x}", doorbell));
            }
            PACKET3_UNMAP_QUEUES => {
                let doorbell = (d(2) >> 2) & 0x3ffffff;
                KCQ.with(|k| k.borrow_mut().retain(|e| e.0 != doorbell));
                QUEUES.with(|q| q.borrow_mut().retain(|e| e.0 != doorbell));
                log(format!("pm4 UNMAP_QUEUES doorbell {:#x}", doorbell));
            }
            other => log(format!("pm4 op {:#x} ({} dw)", other, len)),
        }
        at = if wrap == u32::MAX { at + len } else { (at + len) % wrap };
    }
}

/// Runs the SDMA packets in `[at, end)` dwords.
fn run_sdma(rd: &dyn Fn(u32) -> u32, mut at: u32, end: u32, wrap: u32) {
    while at != end {
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

/// A doorbell rang: run the queue up to the new write pointer.
fn doorbell(index: u32, value: u64) {
    let Some(mut q) = queue_for(index) else {
        log(format!("doorbell {:#x} = {:#x}: no queue", index, value));
        return;
    };
    let wptr = if q.kind == Kind::Sdma { (value >> 2) as u32 } else { value as u32 } % q.size_dw;
    let base = q.base;
    let rd = move |i: u32| gpu_r32(base + i as u64 * 4);
    match q.kind {
        Kind::Sdma => run_sdma(&rd, q.rptr, wptr, q.size_dw),
        k => run_pm4(k, &rd, q.rptr, wptr, q.size_dw),
    }
    q.rptr = wptr;
    gpu_w32(q.rptr_report, if q.kind == Kind::Sdma { wptr << 2 } else { wptr });
    QUEUES.with(|qs| {
        let mut qs = qs.borrow_mut();
        qs.retain(|e| e.0 != index);
        qs.push((index, q));
    });
}

/// The fake GPU listens from here on.
fn fake_gpu() {
    npk_sys::fake_doorbell_hook(doorbell);
}
