//! amdgpu/vcn_v2_0.c and amdgpu_vcn.c (Linux 6.18.55): the VCN block's
//! early_init, which requests the VCN image.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::{amdgpu_ucode_ip_version_decode, amdgpu_ucode_request};
use crate::regs::*;

/// `amdgpu_vcn_early_init`, instance 0.
pub fn amdgpu_vcn_early_init(adev: &mut amdgpu_device, i: usize) -> i32 {
    adev.vcn.inst[i].inst = i as u32;
    let ucode_prefix = amdgpu_ucode_ip_version_decode(adev, UVD_HWIP);
    if adev.vcn.inst[0].fw.is_some() {
        return 0;
    }
    let (fw, r) = amdgpu_ucode_request(&alloc::format!("amdgpu/{}.bin", ucode_prefix));
    adev.vcn.inst[0].fw = if r == 0 { fw } else { None };
    r
}

/// `vcn_v2_0_early_init`. Bare metal: two encode rings.
pub fn vcn_v2_0_early_init(adev: &mut amdgpu_device) -> i32 {
    let v = &mut adev.vcn.inst[0];
    v.num_enc_rings = 2;
    v.set_pg_state = Some("vcn_v2_0_set_pg_state");
    // vcn_v2_0_set_dec_ring_funcs
    v.ring_dec_funcs = Some("vcn_v2_0_dec_ring_vm_funcs");
    // vcn_v2_0_set_enc_ring_funcs
    for i in 0..v.num_enc_rings as usize {
        v.ring_enc_funcs[i] = Some("vcn_v2_0_enc_ring_vm_funcs");
    }
    // vcn_v2_0_set_irq_funcs
    v.irq.num_types = v.num_enc_rings + 1;
    v.irq.funcs = Some("vcn_v2_0_irq_funcs");

    amdgpu_vcn_early_init(adev, 0)
}
