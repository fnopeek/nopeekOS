//! amdgpu/vega10_ih.c (Linux 6.18.55): the IH block's early_init.

use crate::amdgpu_device::amdgpu_device;

/// `vega10_ih_early_init`.
pub fn vega10_ih_early_init(adev: &mut amdgpu_device) -> i32 {
    // vega10_ih_set_interrupt_funcs
    adev.irq.ih_funcs = Some("vega10_ih_funcs");
    // vega10_ih_set_self_irq_funcs
    adev.irq.self_irq.num_types = 0;
    adev.irq.self_irq.funcs = Some("vega10_ih_self_irq_funcs");
    0
}
