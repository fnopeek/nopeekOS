//! include/uapi/drm/i915_drm.h (Linux 6.18.55): the numbers and layouts of
//! the i915 uAPI a native-context guest speaks, and Mesa's
//! src/intel/dev/virtio/i915_proto.h, the ccmd framing around it.

/// `_IOC(dir, 'd', DRM_COMMAND_BASE + nr, size)`.
const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    dir << 30 | size << 16 | (b'd' as u32) << 8 | (0x40 + nr)
}
const W: u32 = 1;
const R: u32 = 2;
const WR: u32 = 3;
/// `_IOC_SIZE(cmd)`, `IOC_OUT`.
pub const fn ioc_size(cmd: u32) -> u32 {
    (cmd >> 16) & 0x3fff
}
pub const IOC_OUT: u32 = R << 30;

pub const DRM_IOCTL_I915_GEM_SET_DOMAIN: u32 = ioc(W, 0x1f, 12);
pub const DRM_IOCTL_I915_GEM_SET_TILING: u32 = ioc(WR, 0x21, 16);
pub const DRM_IOCTL_I915_GEM_GET_TILING: u32 = ioc(WR, 0x22, 16);
pub const DRM_IOCTL_I915_GEM_GET_APERTURE: u32 = ioc(R, 0x23, 16);
pub const DRM_IOCTL_I915_GEM_CONTEXT_CREATE: u32 = ioc(WR, 0x2d, 8);
pub const DRM_IOCTL_I915_GEM_CONTEXT_DESTROY: u32 = ioc(W, 0x2e, 8);
pub const DRM_IOCTL_I915_REG_READ: u32 = ioc(WR, 0x31, 16);
pub const DRM_IOCTL_I915_GET_RESET_STATS: u32 = ioc(WR, 0x32, 24);
pub const DRM_IOCTL_I915_GEM_CONTEXT_GETPARAM: u32 = ioc(WR, 0x34, 24);
pub const DRM_IOCTL_I915_GEM_CONTEXT_SETPARAM: u32 = ioc(WR, 0x35, 24);
pub const DRM_IOCTL_I915_GEM_VM_CREATE: u32 = ioc(WR, 0x3a, 16);
pub const DRM_IOCTL_I915_GEM_VM_DESTROY: u32 = ioc(W, 0x3b, 16);

// enum i915_ccmd
pub const I915_CCMD_IOCTL_SIMPLE: u32 = 1;
pub const I915_CCMD_GETPARAM: u32 = 2;
pub const I915_CCMD_QUERYPARAM: u32 = 3;
pub const I915_CCMD_GEM_CONTEXT_CREATE: u32 = 6;
pub const I915_CCMD_GEM_EXECBUFFER2: u32 = 7;
pub const I915_CCMD_GEM_BUSY: u32 = 9;
pub const I915_CCMD_GEM_EXECBUFFER2_V2: u32 = 10;
/// nopeekOS' own commands, numbered away from virglrenderer's: what the
/// i915 uAPI passes through pointers and Mesa's protocol cannot carry.
/// `CONTEXT_SETPARAM`/`_GETPARAM` with the parameter's data inline (an
/// engine map followed by its extension chain), and an execbuffer that may
/// name a request of the same client to start with (`I915_EXEC_FENCE_SUBMIT`).
pub const I915_CCMD_NPK_CONTEXT_SETPARAM: u32 = 0x100;
pub const I915_CCMD_NPK_CONTEXT_GETPARAM: u32 = 0x101;
pub const I915_CCMD_NPK_GEM_EXECBUFFER2: u32 = 0x102;
/// `sizeof(struct vdrm_ccmd_req)`: cmd, len, seqno, rsp_off.
pub const VDRM_CCMD_REQ: usize = 16;

pub const I915_PARAM_IRQ_ACTIVE: u32 = 1;
pub const I915_PARAM_ALLOW_BATCHBUFFER: u32 = 2;
pub const I915_PARAM_LAST_DISPATCH: u32 = 3;
pub const I915_PARAM_CHIPSET_ID: u32 = 4;
pub const I915_PARAM_HAS_GEM: u32 = 5;
pub const I915_PARAM_NUM_FENCES_AVAIL: u32 = 6;
pub const I915_PARAM_HAS_OVERLAY: u32 = 7;
pub const I915_PARAM_HAS_PAGEFLIPPING: u32 = 8;
pub const I915_PARAM_HAS_EXECBUF2: u32 = 9;
pub const I915_PARAM_HAS_BSD: u32 = 10;
pub const I915_PARAM_HAS_BLT: u32 = 11;
pub const I915_PARAM_HAS_RELAXED_FENCING: u32 = 12;
pub const I915_PARAM_HAS_COHERENT_RINGS: u32 = 13;
pub const I915_PARAM_HAS_EXEC_CONSTANTS: u32 = 14;
pub const I915_PARAM_HAS_RELAXED_DELTA: u32 = 15;
pub const I915_PARAM_HAS_GEN7_SOL_RESET: u32 = 16;
pub const I915_PARAM_HAS_LLC: u32 = 17;
pub const I915_PARAM_HAS_ALIASING_PPGTT: u32 = 18;
pub const I915_PARAM_HAS_WAIT_TIMEOUT: u32 = 19;
pub const I915_PARAM_HAS_SEMAPHORES: u32 = 20;
pub const I915_PARAM_HAS_PRIME_VMAP_FLUSH: u32 = 21;
pub const I915_PARAM_HAS_VEBOX: u32 = 22;
pub const I915_PARAM_HAS_SECURE_BATCHES: u32 = 23;
pub const I915_PARAM_HAS_PINNED_BATCHES: u32 = 24;
pub const I915_PARAM_HAS_EXEC_NO_RELOC: u32 = 25;
pub const I915_PARAM_HAS_EXEC_HANDLE_LUT: u32 = 26;
pub const I915_PARAM_HAS_WT: u32 = 27;
pub const I915_PARAM_CMD_PARSER_VERSION: u32 = 28;
pub const I915_PARAM_HAS_COHERENT_PHYS_GTT: u32 = 29;
pub const I915_PARAM_MMAP_VERSION: u32 = 30;
pub const I915_PARAM_HAS_BSD2: u32 = 31;
pub const I915_PARAM_REVISION: u32 = 32;
pub const I915_PARAM_SUBSLICE_TOTAL: u32 = 33;
pub const I915_PARAM_EU_TOTAL: u32 = 34;
pub const I915_PARAM_HAS_GPU_RESET: u32 = 35;
pub const I915_PARAM_HAS_RESOURCE_STREAMER: u32 = 36;
pub const I915_PARAM_HAS_EXEC_SOFTPIN: u32 = 37;
pub const I915_PARAM_HAS_POOLED_EU: u32 = 38;
pub const I915_PARAM_MIN_EU_IN_POOL: u32 = 39;
pub const I915_PARAM_MMAP_GTT_VERSION: u32 = 40;
pub const I915_PARAM_HAS_SCHEDULER: u32 = 41;
pub const I915_PARAM_HUC_STATUS: u32 = 42;
pub const I915_PARAM_HAS_EXEC_ASYNC: u32 = 43;
pub const I915_PARAM_HAS_EXEC_FENCE: u32 = 44;
pub const I915_PARAM_HAS_EXEC_CAPTURE: u32 = 45;
pub const I915_PARAM_SLICE_MASK: u32 = 46;
pub const I915_PARAM_SUBSLICE_MASK: u32 = 47;
pub const I915_PARAM_HAS_EXEC_BATCH_FIRST: u32 = 48;
pub const I915_PARAM_HAS_EXEC_FENCE_ARRAY: u32 = 49;
pub const I915_PARAM_HAS_CONTEXT_ISOLATION: u32 = 50;
pub const I915_PARAM_CS_TIMESTAMP_FREQUENCY: u32 = 51;
pub const I915_PARAM_MMAP_GTT_COHERENT: u32 = 52;
pub const I915_PARAM_HAS_EXEC_SUBMIT_FENCE: u32 = 53;
pub const I915_PARAM_PERF_REVISION: u32 = 54;
pub const I915_PARAM_HAS_EXEC_TIMELINE_FENCES: u32 = 55;
pub const I915_PARAM_HAS_USERPTR_PROBE: u32 = 56;
pub const I915_PARAM_OA_TIMESTAMP_FREQUENCY: u32 = 57;
pub const I915_PARAM_PXP_STATUS: u32 = 58;
pub const I915_PARAM_HAS_CONTEXT_FREQ_HINT: u32 = 59;

pub const I915_SCHEDULER_CAP_ENABLED: i32 = 1 << 0;
pub const I915_SCHEDULER_CAP_PRIORITY: i32 = 1 << 1;

pub const DRM_I915_QUERY_TOPOLOGY_INFO: u32 = 1;
pub const DRM_I915_QUERY_ENGINE_INFO: u32 = 2;
pub const DRM_I915_QUERY_PERF_CONFIG: u32 = 3;
pub const DRM_I915_QUERY_MEMORY_REGIONS: u32 = 4;
pub const DRM_I915_QUERY_HWCONFIG_BLOB: u32 = 5;
pub const DRM_I915_QUERY_GEOMETRY_SUBSLICES: u32 = 6;
pub const DRM_I915_QUERY_GUC_SUBMISSION_VERSION: u32 = 7;
pub const I915_ENGINE_INFO_HAS_LOGICAL_INSTANCE: u64 = 1 << 0;
pub const INTEL_MEMORY_SYSTEM: u16 = 0;

// enum drm_i915_gem_engine_class
pub const I915_ENGINE_CLASS_RENDER: u16 = 0;
pub const I915_ENGINE_CLASS_COPY: u16 = 1;
pub const I915_ENGINE_CLASS_VIDEO: u16 = 2;
pub const I915_ENGINE_CLASS_VIDEO_ENHANCE: u16 = 3;
pub const I915_ENGINE_CLASS_INVALID: u16 = 0xffff;
pub const I915_ENGINE_CLASS_INVALID_NONE: u16 = 0xffff;
pub const I915_ENGINE_CLASS_INVALID_VIRTUAL: u16 = 0xfffe;

// The extensions of I915_CONTEXT_PARAM_ENGINES, and their sizes before
// the trailing engine list (`struct i915_context_engines_*`).
pub const I915_CONTEXT_ENGINES_EXT_LOAD_BALANCE: u32 = 0;
pub const I915_CONTEXT_ENGINES_EXT_BOND: u32 = 1;
pub const I915_CONTEXT_ENGINES_EXT_PARALLEL_SUBMIT: u32 = 2;
pub const LOAD_BALANCE_BYTES: usize = 48;
pub const BOND_BYTES: usize = 80;
pub const PARALLEL_SUBMIT_BYTES: usize = 72;
/// `sizeof(struct i915_user_extension)`.
pub const USER_EXTENSION_BYTES: usize = 32;
/// `sizeof(struct drm_i915_gem_context_param_sseu)`.
pub const PARAM_SSEU_BYTES: usize = 32;
pub const I915_CONTEXT_SSEU_FLAG_ENGINE_INDEX: u32 = 1 << 0;

pub const I915_CONTEXT_CREATE_FLAGS_USE_EXTENSIONS: u32 = 1 << 0;
pub const I915_CONTEXT_CREATE_FLAGS_SINGLE_TIMELINE: u32 = 1 << 1;
pub const I915_CONTEXT_PARAM_GTT_SIZE: u64 = 0x3;
pub const I915_CONTEXT_PARAM_NO_ERROR_CAPTURE: u64 = 0x4;
pub const I915_CONTEXT_PARAM_BANNABLE: u64 = 0x5;
pub const I915_CONTEXT_PARAM_PRIORITY: u64 = 0x6;
pub const I915_CONTEXT_PARAM_SSEU: u64 = 0x7;
pub const I915_CONTEXT_PARAM_RECOVERABLE: u64 = 0x8;
pub const I915_CONTEXT_PARAM_VM: u64 = 0x9;
pub const I915_CONTEXT_PARAM_ENGINES: u64 = 0xa;
pub const I915_CONTEXT_PARAM_PERSISTENCE: u64 = 0xb;
pub const I915_CONTEXT_PARAM_RINGSIZE: u64 = 0xc;
pub const I915_CONTEXT_PARAM_PROTECTED_CONTENT: u64 = 0xd;
pub const I915_CONTEXT_PARAM_LOW_LATENCY: u64 = 0xe;
pub const I915_CONTEXT_PARAM_CONTEXT_IMAGE: u64 = 0xf;
pub const I915_CONTEXT_PARAM_BAN_PERIOD: u64 = 0x1;
pub const I915_CONTEXT_PARAM_NO_ZEROMAP: u64 = 0x2;
pub const I915_CONTEXT_DEFAULT_PRIORITY: i64 = 0;
pub const I915_CONTEXT_MAX_USER_PRIORITY: i64 = 1023;
pub const I915_CONTEXT_MIN_USER_PRIORITY: i64 = -1023;
/// `sizeof(struct drm_i915_gem_context_create_ext_setparam)`: the
/// `i915_user_extension` (32 bytes) and the `drm_i915_gem_context_param`.
pub const SETPARAM_BYTES: usize = 56;

pub const I915_EXEC_RING_MASK: u64 = 0x3f;
pub const I915_EXEC_DEFAULT: u64 = 0;
pub const I915_EXEC_RENDER: u64 = 1;
pub const I915_EXEC_BSD: u64 = 2;
pub const I915_EXEC_BLT: u64 = 3;
pub const I915_EXEC_VEBOX: u64 = 4;
pub const I915_EXEC_SECURE: u64 = 1 << 9;
pub const I915_EXEC_NO_RELOC: u64 = 1 << 11;
pub const I915_EXEC_BSD_SHIFT: u32 = 13;
pub const I915_EXEC_BSD_MASK: u64 = 3 << I915_EXEC_BSD_SHIFT;
pub const I915_EXEC_BSD_DEFAULT: u64 = 0;
pub const I915_EXEC_BSD_RING1: u64 = 1 << I915_EXEC_BSD_SHIFT;
pub const I915_EXEC_BSD_RING2: u64 = 2 << I915_EXEC_BSD_SHIFT;
pub const I915_EXEC_FENCE_IN: u64 = 1 << 16;
pub const I915_EXEC_FENCE_OUT: u64 = 1 << 17;
pub const I915_EXEC_BATCH_FIRST: u64 = 1 << 18;
pub const I915_EXEC_FENCE_ARRAY: u64 = 1 << 19;
pub const I915_EXEC_FENCE_SUBMIT: u64 = 1 << 20;
pub const I915_EXEC_USE_EXTENSIONS: u64 = 1 << 21;
/// `__I915_EXEC_UNKNOWN_FLAGS`.
pub const I915_EXEC_UNKNOWN_FLAGS: u64 = !((I915_EXEC_USE_EXTENSIONS << 1) - 1);

pub const EXEC_OBJECT_WRITE: u64 = 1 << 2;
pub const EXEC_OBJECT_PINNED: u64 = 1 << 4;
pub const EXEC_OBJECT_PAD_TO_SIZE: u64 = 1 << 5;
pub const EXEC_OBJECT_ASYNC: u64 = 1 << 6;
pub const EXEC_OBJECT_CAPTURE: u64 = 1 << 7;
/// `__EXEC_OBJECT_UNKNOWN_FLAGS`.
pub const EXEC_OBJECT_UNKNOWN_FLAGS: u64 = !((EXEC_OBJECT_CAPTURE << 1) - 1);
/// `sizeof(struct drm_i915_gem_exec_object2)`,
/// `sizeof(struct drm_i915_gem_relocation_entry)`.
pub const EXEC_OBJECT2_BYTES: usize = 56;
pub const RELOCATION_ENTRY_BYTES: usize = 32;

pub const I915_TILING_NONE: u32 = 0;
pub const I915_TILING_X: u32 = 1;
pub const I915_TILING_Y: u32 = 2;
pub const I915_BIT_6_SWIZZLE_NONE: u32 = 0;
pub const I915_REG_READ_8B_WA: u64 = 1 << 0;

pub const EINVAL: i32 = 22;
pub const ENOENT: i32 = 2;
pub const ENODEV: i32 = 19;
pub const ENOMEM: i32 = 12;
pub const ENOSPC: i32 = 28;
pub const EPERM: i32 = 1;
pub const EIO: i32 = 5;
pub const EAGAIN: i32 = 11;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;

/// Little-endian fields of a request or response.
pub fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub fn rd64(b: &[u8], o: usize) -> u64 {
    rd32(b, o) as u64 | (rd32(b, o + 4) as u64) << 32
}
pub fn wr32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn wr64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
