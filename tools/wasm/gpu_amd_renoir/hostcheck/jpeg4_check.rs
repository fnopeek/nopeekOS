//! Stage 6's JPEG block against Linux: the port runs to the end of
//! `amdgpu_device_fw_loading` as on the card, the SMU comes up as phase 2
//! has it, then from here on registers never written read SEED(reg) by the
//! rule the fourth argument names, and the block's life runs as
//! oracle/main_jpeg.c runs it: hw_init, the idle work, a use, hw_fini.
//! Printed are every register write and every doorbell with the ring
//! dwords it hands over, and the state the oracle prints. Then both judge
//! the same command streams with `amdgpu_jpeg_dec_parse_cs`: a few at the
//! edges of what it allows, and many made of headers near them. The lines
//! up to the first `---` are the oracle's stdin. The SMU's mailbox is the
//! SMU oracle's.

extern crate alloc;

include!("modules.rs");
#[path = "../src/stage1.rs"]
mod stage1;

use regs::*;

include!("fake_renoir.rs");
include!("fake_gpu.rs");

fn ones(_reg: u32) -> u32 {
    !0
}

fn zeros(_reg: u32) -> u32 {
    0
}

fn hash(reg: u32) -> u32 {
    reg.wrapping_mul(0x9e37_79b1) ^ 0xa5a5_a5a5
}

thread_local! {
    /// The ring's buffer, its dword mask, its doorbell, and where the last
    /// doorbell left it.
    static RING: RefCell<Option<(amdgpu_object::amdgpu_kptr, u32, u32, u64)>> = const { RefCell::new(None) };
    static APER: std::cell::Cell<Option<npk_sys::Mmio>> = const { std::cell::Cell::new(None) };
}

/// Dword `i` of the ring's buffer, as `kptr_r32` reads it.
fn ring_r32(k: amdgpu_object::amdgpu_kptr, i: u64) -> u32 {
    match k {
        amdgpu_object::amdgpu_kptr::gtt(h) => npk_sys::Dma { handle: h, phys: 0 }.r32((i * 4) as u32),
        amdgpu_object::amdgpu_kptr::vram(base) => APER.with(|a| a.get().unwrap().r32((base + i * 4) as u32)),
        amdgpu_object::amdgpu_kptr::doorbell(_) => 0,
    }
}

/// A doorbell: the dwords from the last one to `value`, then the fake GPU
/// runs them.
fn print_doorbell(index: u32, value: u64) {
    match RING.with(|r| *r.borrow()) {
        Some((k, mask, db, from)) if db == index => {
            let mut l = format!("doorbell {:#x} = {:#x}:", index, value);
            let mut at = from;
            while at != value {
                l += &format!(" {:08x}", ring_r32(k, at & mask as u64));
                at += 1;
            }
            println!("{}", l);
            RING.with(|r| r.borrow_mut().as_mut().unwrap().3 = value);
        }
        _ => println!("doorbell {:#x} = {:#x}: no ring", index, value),
    }
    doorbell(index, value);
}

/// What the fake JPEG engine does with a read pointer reset, for the dump.
fn reset_rung(reg: u32, v: u32) {
    if reg == tables::UVD0_BASE[mmUVD_JRBC_RB_RPTR_BASE_IDX as usize] + mmUVD_JRBC_RB_RPTR {
        RING.with(|r| {
            if let Some(e) = r.borrow_mut().as_mut() {
                e.3 = v as u64;
            }
        });
    }
    card_write(reg, v);
}

fn print_state(adev: &amdgpu_device::amdgpu_device, r: usize) {
    let g = &adev.rings[r];
    println!("ready {} {} wptr {:#x}", g.name, g.sched.ready as u32, g.wptr);
    println!("state {} submissions {}", adev.jpeg.cur_state, adev.jpeg.total_submission_cnt);
}

/// `PACKETJ(reg, r, cond, type)`.
fn pj(reg: u32, r: u32, cond: u32, type_: u32) -> u32 {
    (reg & 0x3FFFF) | (r & 0x3F) << 18 | (cond & 0xF) << 24 | (type_ & 0xF) << 28
}

/// The parser's cases: the edges of the register range, the atomic hole,
/// each packet type with its own and a wrong condition, a reserved field,
/// odd lengths; then headers drawn near those edges.
fn cases() -> Vec<Vec<u32>> {
    let (lo, hi, alo, ahi) = (JPEG_REG_RANGE_START, JPEG_REG_RANGE_END, JPEG_ATOMIC_RANGE_START, JPEG_ATOMIC_RANGE_END);
    let regs = [lo - 1, lo, lo + 1, alo - 1, alo, alo + 5, ahi, ahi + 1, hi - 1, hi, hi + 1, 0, 0x3FFFF,
                mmUVD_JPEG_PITCH_INTERNAL_OFFSET, JRBC_DEC_EXTERNAL_REG_WRITE_ADDR];
    let mut v: Vec<Vec<u32>> = vec![
        vec![],
        vec![CP_PACKETJ_NOP, 0],
        vec![CP_PACKETJ_NOP | 1, 0],
        vec![pj(lo, 0, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE0)],
        vec![pj(lo, 1, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE0), 0],
        vec![pj(0, 0, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE2), 0],
        vec![pj(0, 0, 0, PACKETJ_TYPE7), 0],
        vec![pj(mmUVD_JPEG_PITCH_INTERNAL_OFFSET, 0, PACKETJ_CONDITION_CHECK0, PACKETJ_TYPE0), 0xdeadbeef,
             CP_PACKETJ_NOP, 0, pj(hi, 0, PACKETJ_CONDITION_CHECK3, PACKETJ_TYPE3), 2],
    ];
    for &reg in &regs {
        for type_ in [PACKETJ_TYPE0, PACKETJ_TYPE3, PACKETJ_TYPE6, PACKETJ_TYPE4] {
            for cond in [PACKETJ_CONDITION_CHECK0, PACKETJ_CONDITION_CHECK3, 1] {
                v.push(vec![pj(reg, 0, cond, type_), 0x1234_5678]);
            }
        }
    }
    let mut x = 0x2545_f491u32;
    for _ in 0..400 {
        let n = 1 + (x % 12) as usize;
        let mut w = Vec::new();
        for i in 0..n {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if i % 2 == 1 {
                w.push(x);
                continue;
            }
            let reg = regs[(x % regs.len() as u32) as usize];
            let type_ = [PACKETJ_TYPE0, PACKETJ_TYPE3, PACKETJ_TYPE6, PACKETJ_TYPE2, PACKETJ_TYPE7][((x >> 8) % 5) as usize];
            let cond = [PACKETJ_CONDITION_CHECK0, PACKETJ_CONDITION_CHECK3, (x >> 12) & 0xF][((x >> 16) % 3) as usize];
            let res = if (x >> 20) % 16 == 0 { 1 } else { 0 };
            w.push(if type_ == PACKETJ_TYPE6 && (x >> 24) % 2 == 0 { CP_PACKETJ_NOP } else { pj(reg, res, cond, type_) });
        }
        v.push(w);
    }
    v
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    npk_sys::fake_doorbell_hook(print_doorbell);
    npk_sys::fake_write_hook(reset_rung);
    let quiet = npk_sys::quiet(true);
    assert!(stage1::up_to_fw_loading(&mut adev, BDF, (0xfc00_0000, 2 << 20)), "the run did not reach fw_loading");
    assert_eq!(amdgpu_smu::smu_hw_init(&mut adev), 0, "smu_hw_init");

    let a = &adev;
    let hw = [UVD_HWIP, NBIO_HWIP];
    let n: usize = hw.iter().map(|&h| a.reg_offset[h as usize][0].len()).sum();
    let mut l = format!("inputs bases {}", n);
    for h in hw {
        for (seg, v) in a.reg_offset[h as usize][0].iter().enumerate() {
            l += &format!(" {} {} {:x}", h, seg, v);
        }
    }
    l += &format!(" cg {:x} pg {:x} usec {} dpm {} gb {:x} vcn_ring0_1 {:x}", a.cg_flags as u32, a.pg_flags,
                  a.usec_timeout, a.pm.dpm_enabled as u32, a.gfx.config.gb_addr_config, a.doorbell_index.vcn_ring0_1);
    println!("{}", l);
    let r = a.jpeg.inst0_ring_dec.unwrap();
    let g = &a.rings[r];
    println!("ring {} {} {} {} {:#x}", g.name, g.doorbell_index, g.ring_size, g.max_dw, g.gpu_addr);
    RING.with(|x| *x.borrow_mut() = Some((g.ring.unwrap(), g.buf_mask, g.doorbell_index, 0)));
    let cs = cases();
    println!("cases {}", cs.len());
    for c in &cs {
        let mut l = format!("case {}", c.len());
        for w in c {
            l += &format!(" {:x}", w);
        }
        println!("{}", l);
    }

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    // The SMU's JPEG power gating is the SMU oracle's.
    let mp1 = |idx: u32, reg: u32| adev.reg_offset[MP1_HWIP as usize][0][idx as usize] + reg;
    npk_sys::fake_card_echo_skip(&[mp1(mmMP1_SMN_C2PMSG_66_BASE_IDX, mmMP1_SMN_C2PMSG_66),
                                   mp1(mmMP1_SMN_C2PMSG_82_BASE_IDX, mmMP1_SMN_C2PMSG_82),
                                   mp1(mmMP1_SMN_C2PMSG_90_BASE_IDX, mmMP1_SMN_C2PMSG_90)]);
    npk_sys::fake_card_echo(true);
    APER.with(|x| x.set(adev.mman.aper_base_kaddr));

    println!("--- jpeg hw_init");
    println!("jpeg_v2_0_hw_init {}", jpeg_v2_0::jpeg_v2_0_hw_init(&mut adev));
    print_state(&adev, r);
    println!("--- jpeg idle");
    jpeg_v2_0::amdgpu_jpeg_idle_work_handler(&mut adev);
    print_state(&adev, r);
    println!("--- jpeg use");
    jpeg_v2_0::amdgpu_jpeg_ring_begin_use(&mut adev, r);
    jpeg_v2_0::amdgpu_jpeg_ring_end_use(&mut adev, r);
    print_state(&adev, r);
    println!("--- jpeg hw_fini");
    println!("jpeg_v2_0_hw_fini {}", jpeg_v2_0::jpeg_v2_0_hw_fini(&mut adev));
    print_state(&adev, r);
    npk_sys::fake_card_echo(false);
    println!("--- jpeg parse_cs");
    let quiet2 = npk_sys::quiet(true);
    let res: Vec<i32> = cs.iter().map(|c| jpeg_v2_0::amdgpu_jpeg_dec_parse_cs(c)).collect();
    npk_sys::quiet(quiet2);
    for (i, r) in res.iter().enumerate() {
        println!("case {}: {}", i, r);
    }
    npk_sys::quiet(quiet);
}
