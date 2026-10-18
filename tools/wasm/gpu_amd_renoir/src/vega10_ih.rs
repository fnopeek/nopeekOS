//! amdgpu/vega10_ih.c (Linux 6.18.55): the IH block's early_init.

use crate::amdgpu_device::amdgpu_device;
use crate::regs::*;

/// `vega10_ih_early_init`.
pub fn vega10_ih_early_init(adev: &mut amdgpu_device) -> i32 {
    // vega10_ih_set_interrupt_funcs
    adev.irq.ih_funcs = Some("vega10_ih_funcs");
    // vega10_ih_set_self_irq_funcs
    adev.irq.self_irq.num_types = 0;
    adev.irq.self_irq.funcs = Some("vega10_ih_self_irq_funcs");
    0
}

/// `struct amdgpu_ih_regs`.
#[derive(Default, Clone, Copy, Debug)]
pub struct amdgpu_ih_regs {
    pub ih_rb_base: u32,
    pub ih_rb_base_hi: u32,
    pub ih_rb_cntl: u32,
    pub ih_rb_wptr: u32,
    pub ih_rb_rptr: u32,
    pub ih_doorbell_rptr: u32,
    pub ih_rb_wptr_addr_lo: u32,
    pub ih_rb_wptr_addr_hi: u32,
    pub psp_reg_id: u32,
}

/// `struct amdgpu_ih_ring`: a bus-address ring, its rptr/wptr shadows the
/// 8 bytes after it in the same DMA buffer.
#[derive(Default, Clone, Copy, Debug)]
pub struct amdgpu_ih_ring {
    pub ring_size: u32,
    pub ptr_mask: u32,
    pub rptr: u32,
    pub use_bus_addr: bool,
    pub ring: Option<npk_sys::Dma>,
    pub gpu_addr: u64,
    pub wptr_addr: u64,
    pub rptr_addr: u64,
    /// Byte offsets of the shadows in `ring`.
    pub wptr_cpu: u32,
    pub rptr_cpu: u32,
    pub use_doorbell: bool,
    pub doorbell_index: u32,
    pub ih_regs: amdgpu_ih_regs,
}

/// `order_base_2`.
fn order_base_2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

/// `amdgpu_ih_ring_init` (amdgpu_ih.c), the bus-address case.
pub fn amdgpu_ih_ring_init(ih: &mut amdgpu_ih_ring, ring_size: u32, use_bus_addr: bool) -> i32 {
    // Align ring size
    let rb_bufsz = order_base_2(ring_size / 4);
    let ring_size = (1 << rb_bufsz) * 4;
    ih.ring_size = ring_size;
    ih.ptr_mask = ih.ring_size - 1;
    ih.rptr = 0;
    ih.use_bus_addr = use_bus_addr;
    if !use_bus_addr {
        crate::linux::Line::new().s("an IH ring in GTT: not Vega10's").say();
        return -22;
    }
    if ih.ring.is_some() {
        return 0;
    }
    // add 8 bytes for the rptr/wptr shadows and add them to the end of the
    // ring allocation: dma_alloc_coherent.
    let pages = (ih.ring_size as i32 + 8 + 4095) / 4096;
    let Some(d) = npk_sys::Dma::alloc(pages) else { return -12 };
    ih.ring = Some(d);
    ih.gpu_addr = d.phys;
    ih.wptr_addr = d.phys + ih.ring_size as u64;
    ih.wptr_cpu = ih.ring_size;
    ih.rptr_addr = d.phys + ih.ring_size as u64 + 4;
    ih.rptr_cpu = ih.ring_size + 4;
    0
}

/// `SOC15_REG_OFFSET(OSSSYS, 0, reg)`.
fn oss(adev: &amdgpu_device, idx: u32, reg: u32) -> u32 {
    adev.reg_offset[OSSSYS_HWIP as usize][0].get(idx as usize).copied().unwrap_or(0) + reg
}

/// `vega10_ih_init_register_offset`: an APU has only ring 0.
fn vega10_ih_init_register_offset(adev: &mut amdgpu_device) {
    if adev.irq.ih.ring_size != 0 {
        let r = amdgpu_ih_regs {
            ih_rb_base: oss(adev, OSS_mmIH_RB_BASE_BASE_IDX, OSS_mmIH_RB_BASE),
            ih_rb_base_hi: oss(adev, OSS_mmIH_RB_BASE_HI_BASE_IDX, OSS_mmIH_RB_BASE_HI),
            ih_rb_cntl: oss(adev, OSS_mmIH_RB_CNTL_BASE_IDX, OSS_mmIH_RB_CNTL),
            ih_rb_wptr: oss(adev, OSS_mmIH_RB_WPTR_BASE_IDX, OSS_mmIH_RB_WPTR),
            ih_rb_rptr: oss(adev, OSS_mmIH_RB_RPTR_BASE_IDX, OSS_mmIH_RB_RPTR),
            ih_doorbell_rptr: oss(adev, OSS_mmIH_DOORBELL_RPTR_BASE_IDX, OSS_mmIH_DOORBELL_RPTR),
            ih_rb_wptr_addr_lo: oss(adev, OSS_mmIH_RB_WPTR_ADDR_LO_BASE_IDX, OSS_mmIH_RB_WPTR_ADDR_LO),
            ih_rb_wptr_addr_hi: oss(adev, OSS_mmIH_RB_WPTR_ADDR_HI_BASE_IDX, OSS_mmIH_RB_WPTR_ADDR_HI),
            psp_reg_id: PSP_REG_IH_RB_CNTL,
        };
        adev.irq.ih.ih_regs = r;
    }
}

/// `amdgpu_irq_init` (amdgpu_irq.c): one vector, MSI or MSI-X
/// (`pci_alloc_irq_vectors`, `request_irq`).
fn amdgpu_irq_init(adev: &mut amdgpu_device) -> i32 {
    // Enable MSI if not disabled by module parameter: amdgpu_msi_ok.
    adev.irq.msi_enabled = false;
    let msi_ok = crate::tables::amdgpu_msi[0] != 0;
    let v = npk_sys::irq_register(0);
    if v < 0 {
        crate::linux::Line::new().s("Failed to alloc msi vectors").say();
        return -28;
    }
    if msi_ok {
        adev.irq.msi_enabled = true;
    }
    adev.irq.installed = true;
    adev.irq.irq = v as u32;
    crate::linux::Line::new().s("amdgpu: irq initialized (vector ").dec(v as u64).s(").").dbg();
    0
}

/// `vega10_ih_sw_init`: an APU has no ih1/ih2.
pub fn vega10_ih_sw_init(adev: &mut amdgpu_device) -> i32 {
    let self_irq = adev.irq.self_irq;
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_IH, 0, &self_irq, "irq.self_irq");
    if r != 0 {
        return r;
    }
    let r = amdgpu_ih_ring_init(&mut adev.irq.ih, IH_RING_SIZE, true);
    if r != 0 {
        return r;
    }
    adev.irq.ih.use_doorbell = true;
    adev.irq.ih.doorbell_index = adev.doorbell_index.ih << 1;
    // initialize ih control registers offset
    vega10_ih_init_register_offset(adev);
    let r = amdgpu_ih_ring_init(&mut adev.irq.ih_soft, IH_SW_RING_SIZE, true);
    if r != 0 {
        return r;
    }
    amdgpu_irq_init(adev)
}
