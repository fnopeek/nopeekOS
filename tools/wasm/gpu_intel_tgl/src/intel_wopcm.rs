//! gt/intel_wopcm.c (Linux 6.18.55): the WOPCM layout. Without GuC
//! firmware (enable_guc is 0 on Tiger Lake) there is nothing to partition.

use crate::regs::*;

/// `struct intel_wopcm`.
#[derive(Clone, Copy, Default)]
pub struct intel_wopcm {
    pub size: u32,
    /// `wopcm->guc.base`, `wopcm->guc.size`.
    pub guc_base: u32,
    pub guc_size: u32,
}

/// `intel_wopcm_init_early`: HAS_GT_UC, graphics version 11 and later.
pub fn intel_wopcm_init_early(wopcm: &mut intel_wopcm) {
    wopcm.size = GEN11_WOPCM_SIZE;
    crate::linux::Line::new().s("WOPCM: ").dec(wopcm.size as u64 / 1024).s("K").say();
}

/// `intel_wopcm_init`: it returns at once when the GuC upload size is 0,
/// which it is with no firmware fetched. The partitioning after that
/// point is not ported.
pub fn intel_wopcm_init(_wopcm: &mut intel_wopcm) {}

/// `intel_wopcm_guc_size`.
pub fn intel_wopcm_guc_size(wopcm: &intel_wopcm) -> u32 {
    wopcm.guc_size
}
