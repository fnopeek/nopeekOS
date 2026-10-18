//! amdgpu/jpeg_v2_0.c (Linux 6.18.55): the JPEG block's early_init.

use crate::amdgpu_device::amdgpu_device;

/// `jpeg_v2_0_early_init`.
pub fn jpeg_v2_0_early_init(adev: &mut amdgpu_device) -> i32 {
    adev.jpeg.num_jpeg_inst = 1;
    adev.jpeg.num_jpeg_rings = 1;
    // jpeg_v2_0_set_dec_ring_funcs
    adev.jpeg.inst0_ring_dec_funcs = Some("jpeg_v2_0_dec_ring_vm_funcs");
    // jpeg_v2_0_set_irq_funcs
    adev.jpeg.inst0_irq.num_types = 1;
    adev.jpeg.inst0_irq.funcs = Some("jpeg_v2_0_irq_funcs");
    0
}
