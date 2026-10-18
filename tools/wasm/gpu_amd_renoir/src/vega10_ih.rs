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
    pub enabled: bool,
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

/// `REG_SET_FIELD`.
fn field(orig: u32, mask: u32, shift: u32, val: u32) -> u32 {
    (orig & !mask) | (mask & (val << shift))
}

fn rreg(adev: &amdgpu_device, reg: u32) -> u32 {
    adev.rreg32(reg).unwrap_or(0)
}

/// `vega10_ih_toggle_ring_interrupts`, ring 0.
fn vega10_ih_toggle_ring_interrupts(adev: &mut amdgpu_device, enable: bool) {
    let regs = adev.irq.ih.ih_regs;
    let mut tmp = rreg(adev, regs.ih_rb_cntl);
    tmp = field(tmp, OSS_IH_RB_CNTL__RB_ENABLE_MASK, OSS_IH_RB_CNTL__RB_ENABLE__SHIFT, enable as u32);
    tmp = field(tmp, OSS_IH_RB_CNTL__RB_GPU_TS_ENABLE_MASK, OSS_IH_RB_CNTL__RB_GPU_TS_ENABLE__SHIFT, 1);
    // enable_intr field is only valid in ring0
    tmp = field(tmp, OSS_IH_RB_CNTL__ENABLE_INTR_MASK, OSS_IH_RB_CNTL__ENABLE_INTR__SHIFT, enable as u32);
    adev.wreg32(regs.ih_rb_cntl, tmp);
    if enable {
        adev.irq.ih.enabled = true;
    } else {
        // set rptr, wptr to 0
        adev.wreg32(regs.ih_rb_rptr, 0);
        adev.wreg32(regs.ih_rb_wptr, 0);
        adev.irq.ih.enabled = false;
        adev.irq.ih.rptr = 0;
    }
}

/// `vega10_ih_toggle_interrupts`: an APU has ring 0 only.
fn vega10_ih_toggle_interrupts(adev: &mut amdgpu_device, enable: bool) {
    if adev.irq.ih.ring_size != 0 {
        vega10_ih_toggle_ring_interrupts(adev, enable);
    }
}

/// `vega10_ih_rb_cntl`.
fn vega10_ih_rb_cntl(ih: &amdgpu_ih_ring, mut c: u32) -> u32 {
    let rb_bufsz = order_base_2(ih.ring_size / 4);
    c = field(c, OSS_IH_RB_CNTL__MC_SPACE_MASK, OSS_IH_RB_CNTL__MC_SPACE__SHIFT, if ih.use_bus_addr { 1 } else { 4 });
    c = field(c, OSS_IH_RB_CNTL__WPTR_OVERFLOW_CLEAR_MASK, OSS_IH_RB_CNTL__WPTR_OVERFLOW_CLEAR__SHIFT, 1);
    c = field(c, OSS_IH_RB_CNTL__WPTR_OVERFLOW_ENABLE_MASK, OSS_IH_RB_CNTL__WPTR_OVERFLOW_ENABLE__SHIFT, 1);
    c = field(c, OSS_IH_RB_CNTL__RB_SIZE_MASK, OSS_IH_RB_CNTL__RB_SIZE__SHIFT, rb_bufsz);
    // Ring Buffer write pointer writeback. If enabled, IH_RB_WPTR register
    // value is written to memory
    c = field(c, OSS_IH_RB_CNTL__WPTR_WRITEBACK_ENABLE_MASK, OSS_IH_RB_CNTL__WPTR_WRITEBACK_ENABLE__SHIFT, 1);
    c = field(c, OSS_IH_RB_CNTL__MC_SNOOP_MASK, OSS_IH_RB_CNTL__MC_SNOOP__SHIFT, 1);
    c = field(c, OSS_IH_RB_CNTL__MC_RO_MASK, OSS_IH_RB_CNTL__MC_RO__SHIFT, 0);
    c = field(c, OSS_IH_RB_CNTL__MC_VMID_MASK, OSS_IH_RB_CNTL__MC_VMID__SHIFT, 0);
    c
}

/// `vega10_ih_doorbell_rptr`.
fn vega10_ih_doorbell_rptr(ih: &amdgpu_ih_ring) -> u32 {
    if ih.use_doorbell {
        let r = field(0, OSS_IH_DOORBELL_RPTR__OFFSET_MASK, OSS_IH_DOORBELL_RPTR__OFFSET__SHIFT, ih.doorbell_index);
        field(r, OSS_IH_DOORBELL_RPTR__ENABLE_MASK, OSS_IH_DOORBELL_RPTR__ENABLE__SHIFT, 1)
    } else {
        field(0, OSS_IH_DOORBELL_RPTR__ENABLE_MASK, OSS_IH_DOORBELL_RPTR__ENABLE__SHIFT, 0)
    }
}

/// `vega10_ih_enable_ring`, ring 0, bare metal.
fn vega10_ih_enable_ring(adev: &mut amdgpu_device) {
    let ih = adev.irq.ih;
    let regs = ih.ih_regs;
    // Ring Buffer base. [39:8] of 40-bit address of the beginning of the
    // ring buffer
    adev.wreg32(regs.ih_rb_base, (ih.gpu_addr >> 8) as u32);
    adev.wreg32(regs.ih_rb_base_hi, ((ih.gpu_addr >> 40) & 0xff) as u32);
    let mut tmp = rreg(adev, regs.ih_rb_cntl);
    tmp = vega10_ih_rb_cntl(&ih, tmp);
    tmp = field(tmp, OSS_IH_RB_CNTL__RPTR_REARM_MASK, OSS_IH_RB_CNTL__RPTR_REARM__SHIFT, adev.irq.msi_enabled as u32);
    adev.wreg32(regs.ih_rb_cntl, tmp);
    // set the ih ring 0 writeback address whether it's enabled or not
    adev.wreg32(regs.ih_rb_wptr_addr_lo, ih.wptr_addr as u32);
    adev.wreg32(regs.ih_rb_wptr_addr_hi, ((ih.wptr_addr >> 32) & 0xFFFF) as u32);
    // set rptr, wptr to 0
    adev.wreg32(regs.ih_rb_wptr, 0);
    adev.wreg32(regs.ih_rb_rptr, 0);
    adev.wreg32(regs.ih_doorbell_rptr, vega10_ih_doorbell_rptr(&ih));
}

/// `vega10_ih_irq_init` (`vega10_ih_hw_init`). `pci_set_master` is where
/// the kernel puts the card behind the IOMMU.
pub fn vega10_ih_hw_init(adev: &mut amdgpu_device) -> i32 {
    // disable irqs
    vega10_ih_toggle_interrupts(adev, false);
    crate::nbio_v7_0::nbio_v7_0_ih_control(adev);
    if adev.asic_type == CHIP_RENOIR {
        let reg = oss(adev, OSS_mmIH_CHICKEN_BASE_IDX, OSS_mmIH_CHICKEN);
        let mut ih_chicken = rreg(adev, reg);
        if adev.irq.ih.use_bus_addr {
            ih_chicken = field(ih_chicken, OSS_IH_CHICKEN__MC_SPACE_GPA_ENABLE_MASK,
                OSS_IH_CHICKEN__MC_SPACE_GPA_ENABLE__SHIFT, 1);
        }
        adev.wreg32(reg, ih_chicken);
    }
    if adev.irq.ih.ring_size != 0 {
        vega10_ih_enable_ring(adev);
    }
    let (use_doorbell, doorbell_index) = (adev.irq.ih.use_doorbell, adev.irq.ih.doorbell_index);
    crate::nbio_v7_0::nbio_v7_0_ih_doorbell_range(adev, use_doorbell, doorbell_index);

    // pci_set_master
    if npk_sys::pci_enable_bus_master() < 0 {
        crate::linux::Line::new().s("pci_set_master: the kernel refused bus mastering").say();
        return -5;
    }

    // enable interrupts
    vega10_ih_toggle_interrupts(adev, true);
    if adev.irq.ih_soft.ring_size != 0 {
        adev.irq.ih_soft.enabled = true;
    }
    0
}
