//! Stage 6's VCN block against Linux: the port runs to the end of
//! `amdgpu_device_fw_loading` as on the card, the SMU comes up as phase 2
//! has it, then from here on registers never written read SEED(reg) by the
//! rule the fourth argument names, and the block's life runs as
//! oracle/main_vcn.c runs it: hw_init, the idle work, a use, hw_fini.
//! Printed are every register write and every doorbell with the ring
//! dwords it hands over, the state the oracle prints, and the DPG SRAM the
//! PSP loads. The lines up to the last `ring` are the oracle's stdin. The
//! SMU's and the PSP's mailboxes are their own oracles'.

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
    /// Per doorbell index: the ring's buffer, its dword mask, and where the
    /// last doorbell left it.
    static RINGS: RefCell<Vec<(u32, amdgpu_object::amdgpu_kptr, u32, u64)>> = const { RefCell::new(Vec::new()) };
    /// The CPU's window onto VRAM, for a ring that lies there.
    static APER: std::cell::Cell<Option<npk_sys::Mmio>> = const { std::cell::Cell::new(None) };
}

/// Dword `i` of a ring's buffer, as `kptr_r32` reads it.
fn ring_r32(k: amdgpu_object::amdgpu_kptr, i: u64) -> u32 {
    match k {
        amdgpu_object::amdgpu_kptr::gtt(h) => npk_sys::Dma { handle: h, phys: 0 }.r32((i * 4) as u32),
        amdgpu_object::amdgpu_kptr::vram(base) => APER.with(|a| a.get().unwrap().r32((base + i * 4) as u32)),
        amdgpu_object::amdgpu_kptr::doorbell(_) => 0,
    }
}

/// A doorbell: the dwords from the last one on this ring to `value`, then
/// the fake GPU runs them.
fn print_doorbell(index: u32, value: u64) {
    let hit = RINGS.with(|r| r.borrow().iter().find(|e| e.0 == index).map(|e| (e.1, e.2, e.3)));
    match hit {
        Some((k, mask, from)) => {
            let mut l = format!("doorbell {:#x} = {:#x}:", index, value);
            let mut at = from;
            while at != value {
                l += &format!(" {:08x}", ring_r32(k, at & mask as u64));
                at += 1;
            }
            println!("{}", l);
            RINGS.with(|r| {
                for e in r.borrow_mut().iter_mut().filter(|e| e.0 == index) {
                    e.3 = value;
                }
            });
        }
        None => println!("doorbell {:#x} = {:#x}: no ring", index, value),
    }
    doorbell(index, value);
}

/// The ready flags and write pointers, the power and pause state, and the
/// firmware's three queue modes.
fn print_state(adev: &amdgpu_device::amdgpu_device, order: &[usize]) {
    for &r in order {
        let g = &adev.rings[r];
        println!("ready {} {} wptr {:#x}", g.name, g.sched.ready as u32, g.wptr);
    }
    let v = &adev.vcn.inst[0];
    let k = v.cpu_addr.unwrap();
    let byte = |off: u32| {
        let at = v.fw_shared.cpu_off + off as u64;
        (amdgpu_gart::kptr_r32(adev, k, at & !3) >> ((at & 3) * 8)) & 0xff
    };
    println!("state {} pause {} queues {} {} {}", v.cur_state, v.pause_state, byte(FW_SHARED_DECODE_QUEUE_MODE),
             byte(FW_SHARED_ENCODE_GENERALPURPOSE_QUEUE_MODE), byte(FW_SHARED_ENCODE_LOWLATENCY_QUEUE_MODE));
}

fn main() {
    let mut adev = fake_renoir();
    fake_gpu();
    npk_sys::fake_doorbell_hook(print_doorbell);
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
    let v = &a.vcn.inst[0];
    let u = a.firmware.ucode[AMDGPU_UCODE_ID_VCN as usize];
    l += &format!(" cg {:x} pg {:x} usec {} dpm {} gb {:x} fw {} tmr {:x} {:x} vcpu {:#x} shared {:#x} sram {:#x} \
                   indirect {} enc {}",
                  a.cg_flags as u32, a.pg_flags, a.usec_timeout, a.pm.dpm_enabled as u32, a.gfx.config.gb_addr_config,
                  v.fw.as_ref().map_or(0, |f| f.data.len()), u.tmr_mc_addr_lo, u.tmr_mc_addr_hi, v.gpu_addr,
                  v.fw_shared.gpu_addr, v.dpg_sram_gpu_addr, v.indirect_sram as u32, v.num_enc_rings);
    println!("{}", l);
    let mut order = vec![v.ring_dec.unwrap()];
    order.extend(v.ring_enc[..v.num_enc_rings as usize].iter().flatten());
    for &r in &order {
        let g = &a.rings[r];
        println!("ring {} {} {} {} {:#x}", g.name, g.doorbell_index, g.ring_size, g.max_dw, g.gpu_addr);
        RINGS.with(|x| x.borrow_mut().push((g.doorbell_index, g.ring.unwrap(), g.buf_mask, 0)));
    }

    let index = nbio_v7_0::nbio_v7_0_get_pcie_index_offset(&adev).unwrap();
    let data = nbio_v7_0::nbio_v7_0_get_pcie_data_offset(&adev).unwrap();
    let rule = match std::env::args().nth(4).as_deref() {
        Some("ones") => ones,
        Some("zeros") => zeros,
        _ => hash,
    };
    npk_sys::fake_card_unset(rule, (index, data));
    // The SMU's VCN power gating and workload profile, and the PSP's ring
    // that loads the DPG SRAM, are the SMU's and the PSP's oracles'.
    let mp1 = |idx: u32, reg: u32| adev.reg_offset[MP1_HWIP as usize][0][idx as usize] + reg;
    let mp0 = |idx: u32, reg: u32| adev.reg_offset[MP0_HWIP as usize][0][idx as usize] + reg;
    npk_sys::fake_card_echo_skip(&[mp1(mmMP1_SMN_C2PMSG_66_BASE_IDX, mmMP1_SMN_C2PMSG_66),
                                   mp1(mmMP1_SMN_C2PMSG_82_BASE_IDX, mmMP1_SMN_C2PMSG_82),
                                   mp1(mmMP1_SMN_C2PMSG_90_BASE_IDX, mmMP1_SMN_C2PMSG_90),
                                   mp0(MP0_mmMP0_SMN_C2PMSG_67_BASE_IDX, MP0_mmMP0_SMN_C2PMSG_67)]);
    npk_sys::fake_card_echo(true);
    APER.with(|x| x.set(adev.mman.aper_base_kaddr));

    println!("--- vcn hw_init");
    println!("vcn_v2_0_hw_init {}", vcn_v2_0::vcn_v2_0_hw_init(&mut adev));
    print_state(&adev, &order);
    println!("--- vcn idle");
    vcn_v2_0::amdgpu_vcn_idle_work_handler(&mut adev, 0);
    print_state(&adev, &order);
    println!("--- vcn use");
    let dec = order[0];
    vcn_v2_0::amdgpu_vcn_ring_begin_use(&mut adev, dec);
    vcn_v2_0::amdgpu_vcn_ring_end_use(&mut adev, dec);
    print_state(&adev, &order);
    println!("--- vcn hw_fini");
    println!("vcn_v2_0_hw_fini {}", vcn_v2_0::vcn_v2_0_hw_fini(&mut adev));
    print_state(&adev, &order);
    let v = &adev.vcn.inst[0];
    let mut l = format!("sram {} bytes:", v.dpg_sram_curr);
    for i in 0..v.dpg_sram_curr / 4 {
        l += &format!(" {:08x}", amdgpu_gart::kptr_r32(&adev, v.dpg_sram_cpu_addr.unwrap(), i * 4));
    }
    println!("{}", l);
    npk_sys::quiet(quiet);
}
