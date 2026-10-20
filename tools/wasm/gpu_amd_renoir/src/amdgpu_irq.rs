//! amdgpu/amdgpu_irq.c (Linux 6.18.55): the interrupt source registry.

use crate::amdgpu_device::amdgpu_device;
use crate::gmc_v9_0::amdgpu_irq_src;
use crate::linux::Line;
use crate::regs::*;
use alloc::vec;
use alloc::vec::Vec;

/// `struct amdgpu_irq_client`: its sources by source id, each named by
/// the field of `adev` that holds it.
#[derive(Default)]
pub struct amdgpu_irq_client {
    pub sources: Option<Vec<Option<&'static str>>>,
}

/// `amdgpu_irq_add_id`: `name` is the source's place in `adev`.
pub fn amdgpu_irq_add_id(adev: &mut amdgpu_device, client_id: u32, src_id: u32, source: &amdgpu_irq_src,
                         name: &'static str) -> i32 {
    if client_id >= AMDGPU_IRQ_CLIENTID_MAX {
        return -22;
    }
    if src_id >= AMDGPU_MAX_IRQ_SRC_ID {
        return -22;
    }
    if source.funcs.is_none() {
        return -22;
    }
    if adev.irq.client.is_empty() {
        adev.irq.client = (0..AMDGPU_IRQ_CLIENTID_MAX).map(|_| amdgpu_irq_client::default()).collect();
    }
    let c = &mut adev.irq.client[client_id as usize];
    let sources = c.sources.get_or_insert_with(|| vec![None; AMDGPU_MAX_IRQ_SRC_ID as usize]);
    if sources[src_id as usize].is_some() {
        return -22;
    }
    sources[src_id as usize] = Some(name);
    if source.num_types != 0 && !adev.irq.enabled_types.contains_key(name) {
        adev.irq.enabled_types.insert(name, vec![0; source.num_types as usize]);
    }
    0
}

/// The `struct amdgpu_irq_src` `adev` holds under `name`.
pub fn irq_src(adev: &amdgpu_device, name: &str) -> Option<amdgpu_irq_src> {
    Some(match name {
        "irq.self_irq" => adev.irq.self_irq,
        "gmc.vm_fault" => adev.gmc.vm_fault,
        "gmc.ecc_irq" => adev.gmc.ecc_irq,
        "gfx.eop_irq" => adev.gfx.eop_irq,
        "gfx.priv_reg_irq" => adev.gfx.priv_reg_irq,
        "gfx.bad_op_irq" => adev.gfx.bad_op_irq,
        "gfx.priv_inst_irq" => adev.gfx.priv_inst_irq,
        "gfx.cp_ecc_error_irq" => adev.gfx.cp_ecc_error_irq,
        "sdma.trap_irq" => adev.sdma.trap_irq,
        "sdma.ecc_irq" => adev.sdma.ecc_irq,
        "sdma.illegal_inst_irq" => adev.sdma.illegal_inst_irq,
        "sdma.vm_hole_irq" => adev.sdma.vm_hole_irq,
        "sdma.doorbell_invalid_irq" => adev.sdma.doorbell_invalid_irq,
        "sdma.pool_timeout_irq" => adev.sdma.pool_timeout_irq,
        "sdma.srbm_write_irq" => adev.sdma.srbm_write_irq,
        "vcn.inst[0].irq" => adev.vcn.inst[0].irq,
        "jpeg.inst[0].irq" => adev.jpeg.inst0_irq,
        _ => return None,
    })
}

/// A source's `funcs->set`, by the name of its funcs table; `None` for a
/// table without one.
fn src_set(adev: &mut amdgpu_device, funcs: &str, type_: u32, state: u32) -> Option<i32> {
    Some(match funcs {
        "gmc_v9_0_irq_funcs" => crate::gmc_v9_0::gmc_v9_0_vm_fault_interrupt_state(adev, type_, state),
        "gmc_v9_0_ecc_funcs" => crate::gmc_v9_0::gmc_v9_0_ecc_interrupt_state(adev, type_, state),
        "gfx_v9_0_eop_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_set_eop_interrupt_state(adev, type_, state),
        "gfx_v9_0_priv_reg_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_set_priv_reg_fault_state(adev, type_, state),
        "gfx_v9_0_bad_op_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_set_bad_op_fault_state(adev, type_, state),
        "gfx_v9_0_priv_inst_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_set_priv_inst_fault_state(adev, type_, state),
        "gfx_v9_0_cp_ecc_error_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_set_cp_ecc_error_state(adev, type_, state),
        "sdma_v4_0_trap_irq_funcs" => crate::sdma_v4_0::sdma_v4_0_set_trap_irq_state(adev, type_, state),
        "sdma_v4_0_ecc_irq_funcs" => crate::sdma_v4_0::sdma_v4_0_set_ecc_irq_state(adev, type_, state),
        // vcn_v2_0_set_interrupt_state and jpeg_v2_0_set_interrupt_state
        // return 0 and touch nothing.
        "vcn_v2_0_irq_funcs" | "jpeg_v2_0_irq_funcs" => 0,
        _ => return None,
    })
}

/// `amdgpu_irq_enabled`.
pub fn amdgpu_irq_enabled(adev: &amdgpu_device, name: &str, type_: u32) -> bool {
    if !adev.irq.installed {
        return false;
    }
    let Some(src) = irq_src(adev, name) else { return false };
    if type_ >= src.num_types {
        return false;
    }
    match adev.irq.enabled_types.get(name) {
        Some(t) => t[type_ as usize] != 0,
        None => false,
    }
}

/// `amdgpu_irq_update`.
fn amdgpu_irq_update(adev: &mut amdgpu_device, name: &str, type_: u32) -> i32 {
    // We need to determine after taking the lock, otherwise we might
    // disable just enabled interrupts again
    let state = if amdgpu_irq_enabled(adev, name, type_) { AMDGPU_IRQ_STATE_ENABLE } else { AMDGPU_IRQ_STATE_DISABLE };
    let funcs = irq_src(adev, name).and_then(|s| s.funcs).unwrap_or("");
    src_set(adev, funcs, type_, state).unwrap_or(-22)
}

/// Whether a source has `enabled_types` and a `funcs->set`.
fn settable(adev: &mut amdgpu_device, name: &str) -> bool {
    let funcs = irq_src(adev, name).and_then(|s| s.funcs).unwrap_or("");
    adev.irq.enabled_types.contains_key(name) && has_set(funcs)
}

/// Whether the funcs table named has a `set`.
fn has_set(funcs: &str) -> bool {
    matches!(funcs, "gmc_v9_0_irq_funcs" | "gmc_v9_0_ecc_funcs" | "gfx_v9_0_eop_irq_funcs"
        | "gfx_v9_0_priv_reg_irq_funcs" | "gfx_v9_0_bad_op_irq_funcs" | "gfx_v9_0_priv_inst_irq_funcs"
        | "gfx_v9_0_cp_ecc_error_irq_funcs" | "sdma_v4_0_trap_irq_funcs" | "sdma_v4_0_ecc_irq_funcs"
        | "vcn_v2_0_irq_funcs" | "jpeg_v2_0_irq_funcs")
}

/// `amdgpu_irq_get`.
pub fn amdgpu_irq_get(adev: &mut amdgpu_device, name: &'static str, type_: u32) -> i32 {
    if !adev.irq.installed {
        return -2;
    }
    let Some(src) = irq_src(adev, name) else { return -22 };
    if type_ >= src.num_types {
        return -22;
    }
    if !settable(adev, name) {
        return -22;
    }
    let t = adev.irq.enabled_types.get_mut(name).unwrap();
    t[type_ as usize] += 1;
    if t[type_ as usize] == 1 {
        return amdgpu_irq_update(adev, name, type_);
    }
    0
}

/// `amdgpu_irq_put`: no RAS, so never RMA.
pub fn amdgpu_irq_put(adev: &mut amdgpu_device, name: &'static str, type_: u32) -> i32 {
    if !adev.irq.installed {
        return -2;
    }
    let Some(src) = irq_src(adev, name) else { return -22 };
    if type_ >= src.num_types {
        return -22;
    }
    if !settable(adev, name) {
        return -22;
    }
    if !amdgpu_irq_enabled(adev, name, type_) {
        Line::new().s("WARNING: amdgpu_irq_put on ").s(name).s(", which is not enabled").say();
        return -22;
    }
    let t = adev.irq.enabled_types.get_mut(name).unwrap();
    t[type_ as usize] -= 1;
    if t[type_ as usize] == 0 {
        return amdgpu_irq_update(adev, name, type_);
    }
    0
}

/// `amdgpu_irq_disable_all`.
pub fn amdgpu_irq_disable_all(adev: &mut amdgpu_device) {
    for i in 0..adev.irq.client.len() {
        let Some(sources) = adev.irq.client[i].sources.clone() else { continue };
        for name in sources.into_iter().flatten() {
            let Some(src) = irq_src(adev, name) else { continue };
            let Some(funcs) = src.funcs else { continue };
            if !has_set(funcs) || src.num_types == 0 {
                continue;
            }
            for k in 0..src.num_types {
                let r = src_set(adev, funcs, k, AMDGPU_IRQ_STATE_DISABLE).unwrap_or(0);
                if r != 0 {
                    Line::new().s("error disabling interrupt (").dec((-r) as u64).s(")").say();
                }
            }
        }
    }
}

/// `struct amdgpu_iv_entry`.
#[derive(Default, Clone, Copy)]
pub struct amdgpu_iv_entry {
    pub client_id: u32,
    pub src_id: u32,
    pub ring_id: u32,
    pub vmid: u32,
    pub vmid_src: u32,
    pub timestamp: u64,
    pub timestamp_src: u32,
    pub pasid: u32,
    pub node_id: u32,
    pub src_data: [u32; AMDGPU_IRQ_SRC_DATA_MAX_SIZE_DW as usize],
    /// The IV's dwords as the ring held them.
    pub iv_entry: [u32; 8],
}

/// The IH ring `soft` names: the hardware ring 0 or the software ring.
fn ih_ring(adev: &mut amdgpu_device, soft: bool) -> &mut crate::vega10_ih::amdgpu_ih_ring {
    if soft { &mut adev.irq.ih_soft } else { &mut adev.irq.ih }
}

/// `amdgpu_ih_decode_iv_helper`.
fn amdgpu_ih_decode_iv_helper(adev: &mut amdgpu_device, soft: bool, entry: &mut amdgpu_iv_entry) {
    let ih = ih_ring(adev, soft);
    // wptr/rptr are in bytes!
    let Some(ring) = ih.ring else { return };
    let mut dw = [0u32; 8];
    for (k, d) in dw.iter_mut().enumerate() {
        *d = ring.r32(ih.rptr + k as u32 * 4);
    }
    entry.iv_entry = dw;
    entry.client_id = dw[0] & 0xff;
    entry.src_id = (dw[0] >> 8) & 0xff;
    entry.ring_id = (dw[0] >> 16) & 0xff;
    entry.vmid = (dw[0] >> 24) & 0xf;
    entry.vmid_src = dw[0] >> 31;
    entry.timestamp = dw[1] as u64 | ((dw[2] & 0xffff) as u64) << 32;
    entry.timestamp_src = dw[2] >> 31;
    entry.pasid = dw[3] & 0xffff;
    entry.node_id = (dw[3] >> 16) & 0xff;
    entry.src_data[0] = dw[4];
    entry.src_data[1] = dw[5];
    entry.src_data[2] = dw[6];
    entry.src_data[3] = dw[7];
    // wptr/rptr are in bytes!
    ih.rptr += 32;
}

/// A source's `funcs->process`.
fn src_process(adev: &mut amdgpu_device, funcs: &str, entry: &amdgpu_iv_entry, soft: bool) -> i32 {
    match funcs {
        "gmc_v9_0_irq_funcs" => crate::gmc_v9_0::gmc_v9_0_process_interrupt(adev, entry, soft),
        "gfx_v9_0_eop_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_eop_irq(adev, entry),
        "gfx_v9_0_priv_reg_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_priv_reg_irq(adev, entry),
        "gfx_v9_0_bad_op_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_bad_op_irq(adev, entry),
        "gfx_v9_0_priv_inst_irq_funcs" => crate::gfx_v9_0::gfx_v9_0_priv_inst_irq(adev, entry),
        "sdma_v4_0_trap_irq_funcs" => crate::sdma_v4_0::sdma_v4_0_process_trap_irq(adev, entry),
        // vega10_ih_self_irq schedules the ih1/ih2 work an APU does not
        // have.
        "vega10_ih_self_irq_funcs" => 0,
        _ => {
            Line::new().s("interrupt ").dec(entry.client_id as u64).s(":").dec(entry.src_id as u64)
                .s(" for ").s(funcs).s(": its process is not ported").say();
            0
        }
    }
}

/// The IVs dispatched so far.
pub fn amdgpu_irq_counts(adev: &amdgpu_device) -> u32 {
    adev.irq.dispatched
}

/// `amdgpu_irq_dispatch`: no KFD to forward an unhandled IV to.
fn amdgpu_irq_dispatch(adev: &mut amdgpu_device, soft: bool) {
    let mut entry = amdgpu_iv_entry::default();
    amdgpu_ih_decode_iv_helper(adev, soft, &mut entry);
    adev.irq.dispatched += 1;
    let (client_id, src_id) = (entry.client_id, entry.src_id);
    if client_id >= AMDGPU_IRQ_CLIENTID_MAX {
        Line::new().s("Invalid client_id in IV: ").dec(client_id as u64).dbg();
    } else if src_id >= AMDGPU_MAX_IRQ_SRC_ID {
        Line::new().s("Invalid src_id in IV: ").dec(src_id as u64).dbg();
    } else if adev.irq.client.get(client_id as usize).is_none_or(|c| c.sources.is_none()) {
        Line::new().s("Unregistered interrupt client_id: ").dec(client_id as u64).s(" src_id: ").dec(src_id as u64)
            .dbg();
    } else if let Some(name) = adev.irq.client[client_id as usize].sources.as_ref().unwrap()[src_id as usize] {
        let funcs = irq_src(adev, name).and_then(|s| s.funcs).unwrap_or("");
        let r = src_process(adev, funcs, &entry, soft);
        if r < 0 {
            Line::new().s("error processing interrupt (").dec((-r) as u64).s(")").say();
        }
    } else {
        Line::new().s("Unregistered interrupt src_id: ").dec(src_id as u64).s(" of client_id:")
            .dec(client_id as u64).dbg();
    }
    // amdgpu_ih_ts_after(ih->processed_timestamp, entry.timestamp)
    let ih = ih_ring(adev, soft);
    let (ts, p) = (entry.timestamp, ih.processed_timestamp);
    if ((ts << 16) as i64).wrapping_sub((p << 16) as i64) > 0 {
        ih.processed_timestamp = ts;
    }
}

/// `amdgpu_ih_process` on IH ring 0 or the software ring.
pub fn amdgpu_ih_process(adev: &mut amdgpu_device, soft: bool) -> bool {
    if !ih_ring(adev, soft).enabled || adev.shutdown {
        return false;
    }
    let mut wptr = crate::vega10_ih::vega10_ih_get_wptr(adev, soft);
    loop {
        let mut count = AMDGPU_IH_MAX_NUM_IVS;
        // Order reading of wptr vs. reading of IH ring data
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        while ih_ring(adev, soft).rptr != wptr {
            count -= 1;
            if count == 0 {
                break;
            }
            amdgpu_irq_dispatch(adev, soft);
            let ih = ih_ring(adev, soft);
            ih.rptr &= ih.ptr_mask;
        }
        crate::vega10_ih::vega10_ih_set_rptr(adev, soft);
        // make sure wptr hasn't changed while processing
        wptr = crate::vega10_ih::vega10_ih_get_wptr(adev, soft);
        if wptr == ih_ring(adev, soft).rptr {
            break;
        }
    }
    true
}

/// `amdgpu_ih_ring_write` into the software ring.
fn amdgpu_ih_ring_write(adev: &mut amdgpu_device, iv: &[u32]) {
    let ih = &adev.irq.ih_soft;
    let Some(ring) = ih.ring else { return };
    let mut wptr = ring.r32(ih.wptr_cpu) >> 2;
    for &v in iv {
        ring.w32(wptr * 4, v);
        wptr += 1;
    }
    wptr <<= 2;
    wptr &= ih.ptr_mask;
    // Only commit the new wptr if we don't overflow
    if wptr != ih.rptr {
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        ring.w32(ih.wptr_cpu, wptr);
    }
}

/// `amdgpu_irq_delegate`: `ih_soft_work` processes the software ring.
pub fn amdgpu_irq_delegate(adev: &mut amdgpu_device, entry: &amdgpu_iv_entry, num_dw: usize) {
    amdgpu_ih_ring_write(adev, &entry.iv_entry[..num_dw]);
    crate::workqueue::queue_work(adev, crate::workqueue::work::ih_soft);
}

/// `amdgpu_irq_handle_ih_soft`.
pub fn amdgpu_irq_handle_ih_soft(adev: &mut amdgpu_device) {
    amdgpu_ih_process(adev, true);
}

/// The MSI vector fired, or `timeout_ms` passed: then the IH ring is
/// processed. This is where `amdgpu_irq_handler` runs; the module has no
/// interrupt context of its own.
pub fn amdgpu_irq_wait(adev: &mut amdgpu_device, timeout_ms: i32) -> bool {
    let vector = adev.irq.irq as i32;
    let since = npk_sys::irq_arm(vector);
    let fired = npk_sys::irq_wait(vector, since, timeout_ms) == 1;
    amdgpu_ih_process(adev, false);
    fired
}
