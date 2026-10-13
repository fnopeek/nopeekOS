//! gt/intel_engine.h (Linux 6.18.55): engine-mask helpers.

/// `__HAS_ENGINE(engine_mask, id)`.
pub fn has_engine(engine_mask: u64, id: u32) -> bool {
    engine_mask & (1u64 << id) != 0
}
