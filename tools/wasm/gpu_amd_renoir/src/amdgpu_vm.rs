//! amdgpu/amdgpu_vm.c (Linux 6.18.55): the VM size GMC derives.

use crate::amdgpu_device::amdgpu_device;
use crate::linux::{fls64, ilog2, roundup_pow_of_two, Line};
use crate::regs::*;
use crate::tables;

/// `amdgpu_vm_get_block_size`.
fn amdgpu_vm_get_block_size(vm_size: u64) -> u32 {
    // Total bits covered by PD + PTs
    let bits = ilog2(vm_size) + 18;
    // Make sure the PD is 4K in size up to 8GB address space. Above that
    // split equal between PD and PTs
    if vm_size <= 8 { bits - 9 } else { (bits + 3) / 2 }
}

/// `amdgpu_vm_adjust_size`.
pub fn amdgpu_vm_adjust_size(adev: &mut amdgpu_device, min_vm_size: u32, fragment_size_default: u32,
                             max_level: u32, max_bits: u32) {
    let max_size: u32 = 1 << (max_bits - 30);
    let vm_size: u32;
    // adjust vm size first
    if tables::amdgpu_vm_size[0] != -1 {
        let mut v = tables::amdgpu_vm_size[0] as u32;
        if v > max_size {
            Line::new().s("VM size (").dec(tables::amdgpu_vm_size[0] as u64).s(") too large, max is ")
                .dec(max_size as u64).s(" GB").say();
            v = max_size;
        }
        vm_size = v;
    } else {
        // si_meminfo: the RAM the kernel manages.
        let total_mb = npk_sys::sys_info(2).max(0) as u64;
        let phys_ram_gb = ((total_mb << 20) + (1 << 30) - 1) >> 30;
        let clamped = (phys_ram_gb * 3).clamp(min_vm_size as u64, max_size as u64);
        vm_size = roundup_pow_of_two(clamped) as u32;
    }

    adev.vm_manager.max_pfn = (vm_size as u64) << 18;

    let mut tmp = roundup_pow_of_two(adev.vm_manager.max_pfn);
    if tables::amdgpu_vm_block_size[0] != -1 {
        tmp >>= tables::amdgpu_vm_block_size[0] as u32 - 9;
    }
    let tmp = ((fls64(tmp) - 1) as u64).div_ceil(9) - 1;
    adev.vm_manager.num_level = max_level.min(tmp as u32);
    match adev.vm_manager.num_level {
        3 => adev.vm_manager.root_level = AMDGPU_VM_PDB2,
        2 => adev.vm_manager.root_level = AMDGPU_VM_PDB1,
        1 => adev.vm_manager.root_level = AMDGPU_VM_PDB0,
        _ => Line::new().s("VMPT only supports 2~4+1 levels").say(),
    }
    // block size depends on vm size and hw setup
    adev.vm_manager.block_size = if tables::amdgpu_vm_block_size[0] != -1 {
        (tables::amdgpu_vm_block_size[0] as u32)
            .min(max_bits - AMDGPU_GPU_PAGE_SHIFT - 9 * adev.vm_manager.num_level)
    } else if adev.vm_manager.num_level > 1 {
        9
    } else {
        amdgpu_vm_get_block_size(tmp)
    };
    adev.vm_manager.fragment_size = if tables::amdgpu_vm_fragment_size[0] == -1 {
        fragment_size_default
    } else {
        tables::amdgpu_vm_fragment_size[0] as u32
    };
    Line::new().s("vm size is ").dec(vm_size as u64).s(" GB, ").dec(adev.vm_manager.num_level as u64 + 1)
        .s(" levels, block size is ").dec(adev.vm_manager.block_size as u64).s("-bit, fragment size is ")
        .dec(adev.vm_manager.fragment_size as u64).s("-bit").say();
}
