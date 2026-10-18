//! display/dmub/src/dmub_srv.c (Linux 6.18.55): the DMUB service, the part
//! dm_dmub_sw_init runs: create, the region layout and its rebase onto the
//! framebuffer.

use crate::regs::*;

const NUM_WINDOWS: usize = DMUB_NUM_WINDOWS as usize;

/// `struct dmub_fw_meta_info`, the fields the region layout reads.
#[derive(Default, Clone, Copy)]
pub struct dmub_fw_meta_info {
    pub magic_value: u32,
    pub fw_region_size: u32,
    pub trace_buffer_size: u32,
    pub fw_version: u32,
    pub shared_state_size: u32,
}

/// `struct dmub_srv`, the part sw_init fills.
#[derive(Default)]
pub struct dmub_srv {
    pub asic: u32,
    pub fw_version: u32,
    pub sw_init: bool,
    pub meta_info: dmub_fw_meta_info,
}

/// `struct dmub_srv_region_params`; the blobs are slices of the firmware
/// image.
pub struct dmub_srv_region_params<'a> {
    pub inst_const_size: u32,
    pub bss_data_size: u32,
    pub vbios_size: u32,
    pub fw_inst_const: Option<&'a [u8]>,
    pub fw_bss_data: Option<&'a [u8]>,
    pub window_memory_type: &'a [u32; NUM_WINDOWS],
}

/// `struct dmub_region`.
#[derive(Default, Clone, Copy)]
pub struct dmub_region {
    pub base: u32,
    pub top: u32,
}

/// `struct dmub_srv_region_info`.
#[derive(Default)]
pub struct dmub_srv_region_info {
    pub fb_size: u32,
    pub gart_size: u32,
    pub num_regions: u8,
    pub regions: [dmub_region; NUM_WINDOWS],
}

/// `struct dmub_fb`: the CPU address is an offset into the buffer it
/// was rebased on.
#[derive(Default, Clone, Copy)]
pub struct dmub_fb {
    pub cpu_off: u64,
    pub gpu_addr: u64,
    pub size: u32,
}

/// `struct dmub_srv_fb_info`.
#[derive(Default)]
pub struct dmub_srv_fb_info {
    pub num_fb: u32,
    pub fb: [dmub_fb; NUM_WINDOWS],
}

fn dmub_align(val: u32, factor: u32) -> u32 {
    (val + factor - 1) / factor * factor
}

/// `dmub_srv_hw_setup`: DCN 2.1 has inbox1 read and write pointers, so the
/// sanity check of `dmub_srv_create` passes.
fn dmub_srv_hw_setup(asic: u32) -> bool {
    asic == DMUB_ASIC_DCN21
}

/// `dmub_srv_create`.
pub fn dmub_srv_create(dmub: &mut dmub_srv, asic: u32, fw_version: u32) -> u32 {
    *dmub = dmub_srv { asic, fw_version, ..dmub_srv::default() };
    // Setup asic dependent hardware funcs.
    if !dmub_srv_hw_setup(asic) {
        *dmub = dmub_srv::default();
        return DMUB_STATUS_INVALID;
    }
    dmub.sw_init = true;
    DMUB_STATUS_OK
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// `dmub_get_fw_meta_info_from_blob`.
fn dmub_get_fw_meta_info_from_blob(blob: Option<&[u8]>, blob_size: u32, meta_offset: u32) -> Option<dmub_fw_meta_info> {
    let blob = blob?;
    if blob_size == 0 {
        return None;
    }
    if (blob_size as u64) < SIZEOF_dmub_fw_meta as u64 + meta_offset as u64 {
        return None;
    }
    let at = (blob_size - meta_offset - SIZEOF_dmub_fw_meta) as usize;
    if at + SIZEOF_dmub_fw_meta as usize > blob.len() {
        return None;
    }
    let m = &blob[at..];
    if le32(m, FWMETA_MAGIC_VALUE as usize) != DMUB_FW_META_MAGIC {
        return None;
    }
    Some(dmub_fw_meta_info {
        magic_value: le32(m, FWMETA_MAGIC_VALUE as usize),
        fw_region_size: le32(m, FWMETA_FW_REGION_SIZE as usize),
        trace_buffer_size: le32(m, FWMETA_TRACE_BUFFER_SIZE as usize),
        fw_version: le32(m, FWMETA_FW_VERSION as usize),
        shared_state_size: le32(m, FWMETA_SHARED_STATE_SIZE as usize),
    })
}

/// `dmub_get_fw_meta_info`.
fn dmub_get_fw_meta_info(params: &dmub_srv_region_params) -> Option<dmub_fw_meta_info> {
    if params.fw_bss_data.is_some() && params.bss_data_size != 0 {
        // Legacy metadata region.
        dmub_get_fw_meta_info_from_blob(params.fw_bss_data, params.bss_data_size, DMUB_FW_META_OFFSET)
    } else if params.fw_inst_const.is_some() && params.inst_const_size != 0 {
        // Combined metadata region - can be aligned to 16-bytes.
        (0..16).find_map(|i| dmub_get_fw_meta_info_from_blob(params.fw_inst_const, params.inst_const_size, i))
    } else {
        None
    }
}

/// `dmub_srv_calc_regions_for_memory_type`.
fn dmub_srv_calc_regions_for_memory_type(
    params: &dmub_srv_region_params,
    out: &mut dmub_srv_region_info,
    window_sizes: &[u32; NUM_WINDOWS],
    memory_type: u32,
) -> u32 {
    let mut top = 0;
    for i in 0..NUM_WINDOWS {
        if params.window_memory_type[i] == memory_type {
            let region = &mut out.regions[i];
            region.base = dmub_align(top, 256);
            region.top = region.base + dmub_align(window_sizes[i], 64);
            top = region.top;
        }
    }
    dmub_align(top, 4096)
}

/// `dmub_srv_calc_region_info`.
pub fn dmub_srv_calc_region_info(dmub: &mut dmub_srv, params: &dmub_srv_region_params, out: &mut dmub_srv_region_info) -> u32 {
    let mut fw_state_size = DMUB_FW_STATE_SIZE;
    let mut trace_buffer_size = DMUB_TRACE_BUFFER_SIZE;
    let mut shared_state_size = DMUB_FW_HEADER_SHARED_STATE_SIZE;
    let mut window_sizes = [0u32; NUM_WINDOWS];
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }
    *out = dmub_srv_region_info::default();
    out.num_regions = DMUB_NUM_WINDOWS as u8;

    if let Some(fw_info) = dmub_get_fw_meta_info(params) {
        dmub.meta_info = fw_info;
        fw_state_size = fw_info.fw_region_size;
        trace_buffer_size = fw_info.trace_buffer_size;
        shared_state_size = fw_info.shared_state_size;
        // If DM didn't fill in a version, then fill it in based on the
        // firmware meta now that we have it.
        if dmub.fw_version == 0 {
            dmub.fw_version = fw_info.fw_version;
        }
    }

    window_sizes[DMUB_WINDOW_0_INST_CONST as usize] = params.inst_const_size;
    window_sizes[DMUB_WINDOW_1_STACK as usize] = DMUB_STACK_SIZE + DMUB_CONTEXT_SIZE;
    window_sizes[DMUB_WINDOW_2_BSS_DATA as usize] = params.bss_data_size;
    window_sizes[DMUB_WINDOW_3_VBIOS as usize] = params.vbios_size;
    window_sizes[DMUB_WINDOW_4_MAILBOX as usize] = DMUB_MAILBOX_SIZE;
    window_sizes[DMUB_WINDOW_5_TRACEBUFF as usize] = trace_buffer_size;
    window_sizes[DMUB_WINDOW_6_FW_STATE as usize] = fw_state_size;
    window_sizes[DMUB_WINDOW_7_SCRATCH_MEM as usize] = DMUB_SCRATCH_MEM_SIZE;
    window_sizes[DMUB_WINDOW_IB_MEM as usize] = DMUB_IB_MEM_SIZE;
    window_sizes[DMUB_WINDOW_SHARED_STATE as usize] = DMUB_FW_HEADER_SHARED_STATE_SIZE.max(shared_state_size);
    window_sizes[DMUB_WINDOW_LSDMA_BUFFER as usize] = DMUB_LSDMA_RB_SIZE;

    out.fb_size = dmub_srv_calc_regions_for_memory_type(params, out, &window_sizes, DMUB_WINDOW_MEMORY_TYPE_FB);
    out.gart_size = dmub_srv_calc_regions_for_memory_type(params, out, &window_sizes, DMUB_WINDOW_MEMORY_TYPE_GART);
    DMUB_STATUS_OK
}

/// `dmub_srv_calc_mem_info`: the framebuffer and the GART window as
/// (CPU offset, GPU address) pairs.
pub fn dmub_srv_calc_mem_info(
    dmub: &dmub_srv,
    region_info: &dmub_srv_region_info,
    window_memory_type: &[u32; NUM_WINDOWS],
    fb: (u64, u64),
    gart: (u64, u64),
    out: &mut dmub_srv_fb_info,
) -> u32 {
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }
    *out = dmub_srv_fb_info::default();
    if region_info.num_regions as u32 != DMUB_NUM_WINDOWS {
        return DMUB_STATUS_INVALID;
    }
    for i in 0..NUM_WINDOWS {
        let reg = &region_info.regions[i];
        let (cpu, gpu) = if window_memory_type[i] == DMUB_WINDOW_MEMORY_TYPE_GART { gart } else { fb };
        out.fb[i].cpu_off = cpu + reg.base as u64;
        out.fb[i].gpu_addr = gpu + reg.base as u64;
        out.fb[i].size = reg.top - reg.base;
    }
    out.num_fb = DMUB_NUM_WINDOWS;
    DMUB_STATUS_OK
}
