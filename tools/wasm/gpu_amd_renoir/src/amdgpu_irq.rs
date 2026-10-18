//! amdgpu/amdgpu_irq.c (Linux 6.18.55): the interrupt source registry.

use crate::amdgpu_device::amdgpu_device;
use crate::gmc_v9_0::amdgpu_irq_src;
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
    // The enabled_types counters are allocated with the source in
    // amdgpu_irq_src.
    sources[src_id as usize] = Some(name);
    0
}
