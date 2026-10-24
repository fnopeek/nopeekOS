//! display/dmub/src/dmub_srv.c + dmub_srv.h (Linux 6.18.55): the DMUB
//! service — create, the region layout and its rebase onto the framebuffer
//! (dm_dmub_sw_init), and the hardware bring-up of the DMCUB
//! (dm_dmub_hw_init): reset, windows, mailboxes, release, auto-load wait.
//! The hardware functions are those of `dmub_dcn20.rs`.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_object::amdgpu_kptr;
use crate::dmub_dcn20::*;
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

/// `struct dmub_rb` (dmub_cmd.h). `base_address` is a CPU offset into the
/// buffer the windows were rebased on.
#[derive(Default, Clone, Copy)]
pub struct dmub_rb {
    pub base_address: u64,
    pub rptr: u32,
    pub wrpt: u32,
    pub capacity: u32,
}

/// `struct dmub_rb_init_params` (dmub_cmd.h).
#[derive(Default, Clone, Copy)]
pub struct dmub_rb_init_params {
    pub base_address: u64,
    pub capacity: u32,
    pub read_ptr: u32,
    pub write_ptr: u32,
}

/// `dmub_rb_init` (dmub_cmd.h).
pub fn dmub_rb_init(rb: &mut dmub_rb, init_params: &dmub_rb_init_params) {
    rb.base_address = init_params.base_address;
    rb.capacity = init_params.capacity;
    rb.rptr = init_params.read_ptr;
    rb.wrpt = init_params.write_ptr;
}

/// `struct dmub_srv_inbox` (dmub_srv.h): the frame-buffer mailbox ring and
/// the register mailbox's pending flag of its union.
#[derive(Default, Clone, Copy)]
pub struct dmub_srv_inbox {
    pub num_submitted: u64,
    pub num_reported: u64,
    pub rb: dmub_rb,
    pub is_pending: bool,
    pub is_multi_pending: bool,
}

/// `union dmub_rb_cmd` (dmub_cmd.h): one 64-byte ring entry. The header
/// (`struct dmub_cmd_header`) is its first word: type 7:0, sub_type 15:8,
/// ret_status 16, multi_cmd_pending 17, is_reg_based 18, payload_bytes 29:24.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct dmub_rb_cmd(pub [u8; DMUB_RB_CMD_SIZE as usize]);

impl Default for dmub_rb_cmd {
    fn default() -> Self {
        dmub_rb_cmd([0; DMUB_RB_CMD_SIZE as usize])
    }
}

impl dmub_rb_cmd {
    fn header(&self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
    /// Sets the header's type, sub_type and payload_bytes.
    pub fn set_header(&mut self, type_: u32, sub_type: u32, payload_bytes: u32) {
        let h = (self.header() & !0x3f00_ffff) | (type_ & 0xff) | (sub_type & 0xff) << 8 | (payload_bytes & 0x3f) << 24;
        self.0[..4].copy_from_slice(&h.to_le_bytes());
    }
    pub fn multi_cmd_pending(&self) -> bool {
        self.header() & 1 << 17 != 0
    }
}

/// `struct dmub_timeout_info` (dmub_srv.h).
#[derive(Default, Clone, Copy)]
pub struct dmub_timeout_info {
    pub timeout_occured: bool,
    pub timeout_cmd: dmub_rb_cmd,
    pub timestamp: u64,
}

/// `struct dmub_diagnostic_data` (dmub_srv.h).
#[derive(Default, Clone, Copy)]
pub struct dmub_diagnostic_data {
    pub dmcub_version: u32,
    pub scratch: [u32; 17],
    pub pc: [u32; DMUB_PC_SNAPSHOT_COUNT as usize],
    pub undefined_address_fault_addr: u32,
    pub inst_fetch_fault_addr: u32,
    pub data_write_fault_addr: u32,
    pub inbox1_rptr: u32,
    pub inbox1_wptr: u32,
    pub inbox1_size: u32,
    pub inbox0_rptr: u32,
    pub inbox0_wptr: u32,
    pub inbox0_size: u32,
    pub outbox1_rptr: u32,
    pub outbox1_wptr: u32,
    pub outbox1_size: u32,
    pub gpint_datain0: u32,
    pub timeout_info: dmub_timeout_info,
    pub is_dmcub_enabled: bool,
    pub is_dmcub_soft_reset: bool,
    pub is_dmcub_secure_reset: bool,
    pub is_traceport_en: bool,
    pub is_cw0_enabled: bool,
    pub is_cw6_enabled: bool,
    pub is_pwait: bool,
}

/// `struct dmub_srv_hw_funcs` (dmub_srv.h), the members `dmub_srv_hw_setup`
/// binds for the DCN 2.0 family. A `None` is a function the family does
/// not have.
#[derive(Default, Clone, Copy)]
pub struct dmub_srv_hw_funcs {
    pub reset: Option<fn(&amdgpu_device, &mut dmub_srv)>,
    pub reset_release: Option<fn(&amdgpu_device, &mut dmub_srv)>,
    pub backdoor_load: Option<fn(&amdgpu_device, &mut dmub_srv, &dmub_window, &dmub_window)>,
    pub setup_windows: Option<fn(&amdgpu_device, &mut dmub_srv, &dmub_window, &dmub_window, &dmub_window,
                                 &dmub_window, &dmub_window, &dmub_window)>,
    pub setup_mailbox: Option<fn(&amdgpu_device, &mut dmub_srv, &dmub_region)>,
    pub get_inbox1_wptr: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub get_inbox1_rptr: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub set_inbox1_wptr: Option<fn(&amdgpu_device, &dmub_srv, u32)>,
    pub is_supported: Option<fn(&amdgpu_device, &dmub_srv) -> bool>,
    pub is_hw_init: Option<fn(&amdgpu_device, &dmub_srv) -> bool>,
    pub set_gpint: Option<fn(&amdgpu_device, &dmub_srv, dmub_gpint_data_register)>,
    pub is_gpint_acked: Option<fn(&amdgpu_device, &dmub_srv, dmub_gpint_data_register) -> bool>,
    pub get_gpint_response: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub get_fw_status: Option<fn(&amdgpu_device, &dmub_srv) -> dmub_fw_boot_status>,
    pub enable_dmub_boot_options: Option<fn(&amdgpu_device, &dmub_srv, &dmub_srv_hw_params)>,
    pub skip_dmub_panel_power_sequence: Option<fn(&amdgpu_device, &dmub_srv, bool)>,
    pub get_current_time: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub setup_out_mailbox: Option<fn(&amdgpu_device, &mut dmub_srv, &dmub_region)>,
    pub get_outbox1_wptr: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub set_outbox1_rptr: Option<fn(&amdgpu_device, &dmub_srv, u32)>,
    pub setup_outbox0: Option<fn(&amdgpu_device, &mut dmub_srv, &dmub_region)>,
    pub get_outbox0_wptr: Option<fn(&amdgpu_device, &dmub_srv) -> u32>,
    pub set_outbox0_rptr: Option<fn(&amdgpu_device, &dmub_srv, u32)>,
    pub get_diagnostic_data: Option<fn(&amdgpu_device, &mut dmub_srv)>,
}

/// `struct dmub_srv` (dmub_srv.h). `fw_state`, `shared_state` and the
/// rings' `base_address` are CPU offsets into `cpu`, the buffer the windows
/// were rebased on (Linux holds pointers).
#[derive(Default)]
pub struct dmub_srv {
    pub cpu: Option<amdgpu_kptr>,
    pub asic: u32,
    pub fw_version: u32,
    pub is_virtual: bool,
    pub scratch_mem_fb: dmub_fb,
    pub ib_mem_gart: dmub_fb,
    pub shared_state: Option<u64>,
    pub fw_state: Option<u64>,
    pub regs: Option<&'static dmub_srv_common_regs>,
    pub hw_funcs: dmub_srv_hw_funcs,
    pub inbox1: dmub_srv_inbox,
    pub inbox1_last_wptr: u32,
    pub reg_inbox0: dmub_srv_inbox,
    pub outbox1_rb: dmub_rb,
    pub outbox0_rb: dmub_rb,
    pub sw_init: bool,
    pub hw_init: bool,
    pub dpia_supported: bool,
    pub fb_base: u64,
    pub fb_offset: u64,
    pub psp_version: u32,
    pub meta_info: dmub_fw_meta_info,
    pub inbox_type: u32,
    pub power_state: u32,
    pub debug: dmub_diagnostic_data,
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

/// `struct dmub_window`: `offset` is the `union dmub_addr` quad part.
#[derive(Default, Clone, Copy)]
pub struct dmub_window {
    pub offset: u64,
    pub region: dmub_region,
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

/// `struct dmub_srv_hw_params` (dmub_srv.h). `fb` holds copies of the
/// `fb_info` entries Linux points at; `cpu` is the buffer their CPU
/// offsets index.
#[derive(Default)]
pub struct dmub_srv_hw_params {
    pub fb: [Option<dmub_fb>; NUM_WINDOWS],
    pub cpu: Option<amdgpu_kptr>,
    pub fb_base: u64,
    pub fb_offset: u64,
    pub psp_version: u32,
    pub load_inst_const: bool,
    pub skip_panel_power_sequence: bool,
    pub disable_z10: bool,
    pub power_optimization: bool,
    pub dpia_supported: bool,
    pub disable_dpia: bool,
    pub usb4_cm_version: bool,
    pub fw_in_system_memory: bool,
    pub dpia_hpd_int_enable_supported: bool,
    pub disable_clock_gate: bool,
    pub disallow_dispclk_dppclk_ds: bool,
    pub ips_sequential_ono: bool,
    pub mem_access_type: u32,
    pub disable_ips: u32,
    pub disallow_phy_access: bool,
    pub disable_sldo_opt: bool,
    pub enable_non_transparent_setconfig: bool,
    pub lower_hbr3_phy_ssc: bool,
    pub override_hbr3_pll_vco: bool,
}

fn dmub_align(val: u32, factor: u32) -> u32 {
    (val + factor - 1) / factor * factor
}

/// `dmub_flush_buffer_mem`: read the window back in 64-byte chunks so CPU
/// writes reach memory before the DMCUB fetches it.
fn dmub_flush_buffer_mem(adev: &amdgpu_device, cpu: Option<amdgpu_kptr>, fb: &dmub_fb) {
    let Some(base) = cpu else { return };
    let mut buf = [0u8; 64];
    let end = fb.size / 64 * 64;
    let mut pos = 0;
    while pos < end {
        crate::amdgpu_gart::kptr_read(adev, base, fb.cpu_off + pos as u64, &mut buf);
        pos += 64;
    }
    // Read anything leftover into the buffer.
    if end < fb.size {
        crate::amdgpu_gart::kptr_read(adev, base, fb.cpu_off + pos as u64, &mut buf[..(fb.size - end) as usize]);
    }
}

/// `dmub_srv_hw_setup`: the DCN 2.0 family's functions on DCN 2.1's
/// registers.
fn dmub_srv_hw_setup(dmub: &mut dmub_srv, asic: u32) -> bool {
    if asic != DMUB_ASIC_DCN21 {
        return false;
    }
    let funcs = &mut dmub.hw_funcs;
    funcs.reset = Some(dmub_dcn20_reset);
    funcs.reset_release = Some(dmub_dcn20_reset_release);
    funcs.backdoor_load = Some(dmub_dcn20_backdoor_load);
    funcs.setup_windows = Some(dmub_dcn20_setup_windows);
    funcs.setup_mailbox = Some(dmub_dcn20_setup_mailbox);
    funcs.get_inbox1_wptr = Some(dmub_dcn20_get_inbox1_wptr);
    funcs.get_inbox1_rptr = Some(dmub_dcn20_get_inbox1_rptr);
    funcs.set_inbox1_wptr = Some(dmub_dcn20_set_inbox1_wptr);
    funcs.is_supported = Some(dmub_dcn20_is_supported);
    funcs.is_hw_init = Some(dmub_dcn20_is_hw_init);
    funcs.set_gpint = Some(dmub_dcn20_set_gpint);
    funcs.is_gpint_acked = Some(dmub_dcn20_is_gpint_acked);
    funcs.get_gpint_response = Some(dmub_dcn20_get_gpint_response);
    funcs.get_fw_status = Some(dmub_dcn20_get_fw_boot_status);
    funcs.enable_dmub_boot_options = Some(dmub_dcn20_enable_dmub_boot_options);
    funcs.skip_dmub_panel_power_sequence = Some(dmub_dcn20_skip_dmub_panel_power_sequence);
    funcs.get_current_time = Some(dmub_dcn20_get_current_time);

    // Out mailbox register access functions for RN and above
    funcs.setup_out_mailbox = Some(dmub_dcn20_setup_out_mailbox);
    funcs.get_outbox1_wptr = Some(dmub_dcn20_get_outbox1_wptr);
    funcs.set_outbox1_rptr = Some(dmub_dcn20_set_outbox1_rptr);

    // outbox0 call stacks
    funcs.setup_outbox0 = Some(dmub_dcn20_setup_outbox0);
    funcs.get_outbox0_wptr = Some(dmub_dcn20_get_outbox0_wptr);
    funcs.set_outbox0_rptr = Some(dmub_dcn20_set_outbox0_rptr);

    funcs.get_diagnostic_data = Some(dmub_dcn20_get_diagnostic_data);

    dmub.regs = Some(&dmub_srv_dcn21_regs);
    true
}

/// `dmub_srv_create`.
pub fn dmub_srv_create(dmub: &mut dmub_srv, asic: u32, fw_version: u32) -> u32 {
    *dmub = dmub_srv { asic, fw_version, ..dmub_srv::default() };
    // Setup asic dependent hardware funcs.
    if !dmub_srv_hw_setup(dmub, asic) {
        *dmub = dmub_srv::default();
        return DMUB_STATUS_INVALID;
    }
    // Sanity checks for required hw func pointers.
    if dmub.hw_funcs.get_inbox1_rptr.is_none() || dmub.hw_funcs.set_inbox1_wptr.is_none() {
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

/// `dmub_srv_has_hw_support`.
pub fn dmub_srv_has_hw_support(adev: &amdgpu_device, dmub: &dmub_srv, is_supported: &mut bool) -> u32 {
    *is_supported = false;
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }
    if let Some(f) = dmub.hw_funcs.is_supported {
        *is_supported = f(adev, dmub);
    }
    DMUB_STATUS_OK
}

/// `dmub_srv_is_hw_init`.
pub fn dmub_srv_is_hw_init(adev: &amdgpu_device, dmub: &dmub_srv, is_hw_init: &mut bool) -> u32 {
    *is_hw_init = false;
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }
    if !dmub.hw_init {
        return DMUB_STATUS_OK;
    }
    if let Some(f) = dmub.hw_funcs.is_hw_init {
        *is_hw_init = f(adev, dmub);
    }
    DMUB_STATUS_OK
}

/// `dmub_srv_hw_init`. The DCN 2.0 family has no system-memory firmware
/// mode and no inbox0/outbox0 interrupt enables.
pub fn dmub_srv_hw_init(adev: &amdgpu_device, dmub: &mut dmub_srv, params: &dmub_srv_hw_params) -> u32 {
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }

    let w = |id: u32| params.fb[id as usize];
    let (Some(inst_fb), Some(stack_fb), Some(data_fb), Some(bios_fb), Some(mail_fb), Some(tracebuff_fb),
         Some(fw_state_fb), Some(scratch_mem_fb), Some(ib_mem_gart)) =
        (w(DMUB_WINDOW_0_INST_CONST), w(DMUB_WINDOW_1_STACK), w(DMUB_WINDOW_2_BSS_DATA), w(DMUB_WINDOW_3_VBIOS),
         w(DMUB_WINDOW_4_MAILBOX), w(DMUB_WINDOW_5_TRACEBUFF), w(DMUB_WINDOW_6_FW_STATE),
         w(DMUB_WINDOW_7_SCRATCH_MEM), w(DMUB_WINDOW_IB_MEM))
    else {
        return DMUB_STATUS_INVALID;
    };
    // Linux reads the shared-state window's address unchecked.
    let shared_state_fb = w(DMUB_WINDOW_SHARED_STATE).unwrap_or_default();

    dmub.cpu = params.cpu;
    dmub.fb_base = params.fb_base;
    dmub.fb_offset = params.fb_offset;
    dmub.psp_version = params.psp_version;

    if let Some(f) = dmub.hw_funcs.reset {
        f(adev, dmub);
    }

    // reset the cache of the last wptr as well now that hw is reset
    dmub.inbox1_last_wptr = 0;

    let cw0 = dmub_window {
        offset: inst_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW0_BASE, top: DMUB_CW0_BASE + inst_fb.size - 1 },
    };
    let cw1 = dmub_window {
        offset: stack_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW1_BASE, top: DMUB_CW1_BASE + stack_fb.size - 1 },
    };

    if params.load_inst_const {
        if let Some(f) = dmub.hw_funcs.backdoor_load {
            // Read back all the instruction memory so we don't hang the
            // DMCUB when backdoor loading if the write from x86 hasn't been
            // flushed yet.
            if params.mem_access_type == DMUB_MEMORY_ACCESS_CPU {
                dmub_flush_buffer_mem(adev, params.cpu, &inst_fb);
            }
            f(adev, dmub, &cw0, &cw1);
        }
    }

    let cw2 = dmub_window {
        offset: data_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW0_BASE + inst_fb.size, top: DMUB_CW0_BASE + inst_fb.size + data_fb.size },
    };
    let cw3 = dmub_window {
        offset: bios_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW3_BASE, top: DMUB_CW3_BASE + bios_fb.size },
    };
    let cw4 = dmub_window {
        offset: mail_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW4_BASE, top: DMUB_CW4_BASE + mail_fb.size },
    };

    // The mailbox window holds the inbox and then the outbox, DMUB_RB_SIZE
    // each; setup_out_mailbox's uncached outbox address assumes this split.
    let inbox1 = dmub_region { base: cw4.region.base, top: cw4.region.base + DMUB_RB_SIZE };
    let outbox1 = dmub_region { base: inbox1.top, top: inbox1.top + DMUB_RB_SIZE };

    let cw5 = dmub_window {
        offset: tracebuff_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW5_BASE, top: DMUB_CW5_BASE + tracebuff_fb.size },
    };

    let outbox0_base = DMUB_REGION5_BASE + TRACE_BUFFER_ENTRY_OFFSET;
    let outbox0 = dmub_region {
        base: outbox0_base,
        top: outbox0_base.wrapping_add(tracebuff_fb.size).wrapping_sub(TRACE_BUFFER_ENTRY_OFFSET),
    };

    let cw6 = dmub_window {
        offset: fw_state_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW6_BASE, top: DMUB_CW6_BASE + fw_state_fb.size },
    };

    dmub.fw_state = Some(fw_state_fb.cpu_off + DMUB_DEBUG_FW_STATE_OFFSET as u64);

    let region6 = dmub_window {
        offset: shared_state_fb.gpu_addr,
        region: dmub_region { base: DMUB_CW6_BASE, top: DMUB_CW6_BASE + shared_state_fb.size },
    };

    dmub.shared_state = Some(shared_state_fb.cpu_off);

    dmub.scratch_mem_fb = scratch_mem_fb;

    dmub.ib_mem_gart = ib_mem_gart;

    let hw = dmub.hw_funcs;
    if let Some(f) = hw.setup_windows {
        f(adev, dmub, &cw2, &cw3, &cw4, &cw5, &cw6, &region6);
    }
    if let Some(f) = hw.setup_outbox0 {
        f(adev, dmub, &outbox0);
    }
    if let Some(f) = hw.setup_mailbox {
        f(adev, dmub, &inbox1);
    }
    if let Some(f) = hw.setup_out_mailbox {
        f(adev, dmub, &outbox1);
    }

    let mut rb_params = dmub_rb_init_params { base_address: mail_fb.cpu_off, capacity: DMUB_RB_SIZE,
        ..dmub_rb_init_params::default() };
    dmub_rb_init(&mut dmub.inbox1.rb, &rb_params);

    // Initialize outbox1 ring buffer
    rb_params.base_address = mail_fb.cpu_off + DMUB_RB_SIZE as u64;
    rb_params.capacity = DMUB_RB_SIZE;
    dmub_rb_init(&mut dmub.outbox1_rb, &rb_params);

    let outbox0_rb_params = dmub_rb_init_params {
        base_address: tracebuff_fb.cpu_off + TRACE_BUFFER_ENTRY_OFFSET as u64,
        capacity: tracebuff_fb.size.wrapping_sub(dmub_align(TRACE_BUFFER_ENTRY_OFFSET, 64)),
        ..dmub_rb_init_params::default()
    };
    dmub_rb_init(&mut dmub.outbox0_rb, &outbox0_rb_params);

    // Report to DMUB what features are supported by current driver
    if let Some(f) = hw.enable_dmub_boot_options {
        f(adev, dmub, params);
    }

    if !dmub.is_virtual {
        if let Some(f) = hw.skip_dmub_panel_power_sequence {
            f(adev, dmub, params.skip_panel_power_sequence);
        }
    }

    if !dmub.is_virtual {
        if let Some(f) = hw.reset_release {
            f(adev, dmub);
        }
    }

    dmub.hw_init = true;
    dmub.power_state = DMUB_POWER_STATE_D0;

    DMUB_STATUS_OK
}

/// `dmub_srv_hw_reset`.
pub fn dmub_srv_hw_reset(adev: &amdgpu_device, dmub: &mut dmub_srv) -> u32 {
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }

    if let Some(f) = dmub.hw_funcs.reset {
        f(adev, dmub);
    }

    // mailboxes have been reset in hw, so reset the sw state as well
    dmub.inbox1_last_wptr = 0;
    dmub.inbox1.rb.wrpt = 0;
    dmub.inbox1.rb.rptr = 0;
    dmub.inbox1.num_reported = 0;
    dmub.inbox1.num_submitted = 0;
    dmub.reg_inbox0.num_reported = 0;
    dmub.reg_inbox0.num_submitted = 0;
    dmub.reg_inbox0.is_pending = false;
    dmub.outbox0_rb.wrpt = 0;
    dmub.outbox0_rb.rptr = 0;
    dmub.outbox1_rb.wrpt = 0;
    dmub.outbox1_rb.rptr = 0;

    dmub.hw_init = false;

    DMUB_STATUS_OK
}

/// `dmub_srv_wait_for_auto_load`. The DCN 2.0 family has no
/// `is_hw_powered_up`, so the hardware counts as on.
pub fn dmub_srv_wait_for_auto_load(adev: &amdgpu_device, dmub: &dmub_srv, timeout_us: u32) -> u32 {
    let hw_on = true;

    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }

    let mut i = 0;
    while i <= timeout_us {
        let status = dmub.hw_funcs.get_fw_status.map_or(dmub_fw_boot_status::default(), |f| f(adev, dmub));

        if status.dal_fw() && status.mailbox_rdy() && hw_on {
            return DMUB_STATUS_OK;
        }

        crate::linux::udelay(100);
        i += 100;
    }

    DMUB_STATUS_TIMEOUT
}

/// `dmub_rb_empty` (dmub_cmd.h).
pub fn dmub_rb_empty(rb: &dmub_rb) -> bool {
    rb.wrpt == rb.rptr
}

/// The bytes between read and write pointer.
fn dmub_rb_data_count(rb: &dmub_rb) -> u32 {
    if rb.wrpt >= rb.rptr { rb.wrpt - rb.rptr } else { rb.capacity - (rb.rptr - rb.wrpt) }
}

/// `dmub_rb_num_outstanding` (dmub_cmd.h).
pub fn dmub_rb_num_outstanding(rb: &dmub_rb) -> u32 {
    dmub_rb_data_count(rb) / DMUB_RB_CMD_SIZE
}

/// `dmub_rb_num_free` (dmub_cmd.h): one entry is always unusable.
pub fn dmub_rb_num_free(rb: &dmub_rb) -> u32 {
    let data_count = dmub_rb_data_count(rb) + DMUB_RB_CMD_SIZE;
    rb.capacity.wrapping_sub(data_count) / DMUB_RB_CMD_SIZE
}

/// `dmub_rb_full` (dmub_cmd.h): one entry is always unusable.
pub fn dmub_rb_full(rb: &dmub_rb) -> bool {
    dmub_rb_data_count(rb) == rb.capacity.wrapping_sub(DMUB_RB_CMD_SIZE)
}

/// `dmub_rb_push_front` (dmub_cmd.h).
pub fn dmub_rb_push_front(adev: &amdgpu_device, cpu: Option<amdgpu_kptr>, rb: &mut dmub_rb, cmd: &dmub_rb_cmd) -> bool {
    if rb.capacity == 0 {
        return false;
    }
    if dmub_rb_full(rb) {
        return false;
    }
    let Some(base) = cpu else { return false };
    crate::amdgpu_gart::kptr_write(adev, base, rb.base_address + rb.wrpt as u64, &cmd.0);
    rb.wrpt += DMUB_RB_CMD_SIZE;
    if rb.wrpt >= rb.capacity {
        rb.wrpt %= rb.capacity;
    }
    true
}

/// `dmub_rb_flush_pending` (dmub_cmd.h): read the pending entries back so
/// the CPU's writes have reached memory before the DMCUB reads them.
pub fn dmub_rb_flush_pending(adev: &amdgpu_device, cpu: Option<amdgpu_kptr>, rb: &dmub_rb) {
    let mut rptr = rb.rptr;
    let wptr = rb.wrpt;
    if rb.capacity == 0 {
        return;
    }
    let Some(base) = cpu else { return };
    let mut entry = [0u8; DMUB_RB_CMD_SIZE as usize];
    while rptr != wptr {
        crate::amdgpu_gart::kptr_read(adev, base, rb.base_address + rptr as u64, &mut entry);
        rptr += DMUB_RB_CMD_SIZE;
        if rptr >= rb.capacity {
            rptr %= rb.capacity;
        }
    }
}

/// `dmub_rb_get_return_data` (dmub_cmd.h): the entry before the read
/// pointer, which the DMCUB has answered in place.
pub fn dmub_rb_get_return_data(adev: &amdgpu_device, cpu: Option<amdgpu_kptr>, rb: &dmub_rb, cmd: &mut dmub_rb_cmd) {
    let Some(base) = cpu else { return };
    let rd = if rb.rptr == 0 { rb.capacity - DMUB_RB_CMD_SIZE } else { rb.rptr - DMUB_RB_CMD_SIZE };
    crate::amdgpu_gart::kptr_read(adev, base, rb.base_address + rd as u64, &mut cmd.0);
}

/// `dmub_srv_fb_cmd_queue`.
pub fn dmub_srv_fb_cmd_queue(adev: &amdgpu_device, dmub: &mut dmub_srv, cmd: &dmub_rb_cmd) -> u32 {
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    if dmub.power_state != DMUB_POWER_STATE_D0 {
        return DMUB_STATUS_POWER_STATE_D3;
    }
    if dmub.inbox1.rb.rptr > dmub.inbox1.rb.capacity || dmub.inbox1.rb.wrpt > dmub.inbox1.rb.capacity {
        return DMUB_STATUS_HW_FAILURE;
    }
    if dmub_rb_push_front(adev, dmub.cpu, &mut dmub.inbox1.rb, cmd) {
        dmub.inbox1.num_submitted += 1;
        return DMUB_STATUS_OK;
    }
    DMUB_STATUS_QUEUE_FULL
}

/// `dmub_srv_fb_cmd_execute`.
pub fn dmub_srv_fb_cmd_execute(adev: &amdgpu_device, dmub: &mut dmub_srv) -> u32 {
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    if dmub.power_state != DMUB_POWER_STATE_D0 {
        return DMUB_STATUS_POWER_STATE_D3;
    }
    // Read back all the queued commands so the DMCUB reads neither stale
    // nor partial entries.
    let mut flush_rb = dmub.inbox1.rb;
    flush_rb.rptr = dmub.inbox1_last_wptr;
    dmub_rb_flush_pending(adev, dmub.cpu, &flush_rb);

    if let Some(f) = dmub.hw_funcs.set_inbox1_wptr {
        f(adev, dmub, dmub.inbox1.rb.wrpt);
    }
    dmub.inbox1_last_wptr = dmub.inbox1.rb.wrpt;
    DMUB_STATUS_OK
}

/// `dmub_srv_get_fw_boot_status`.
pub fn dmub_srv_get_fw_boot_status(adev: &amdgpu_device, dmub: &dmub_srv, status: &mut dmub_fw_boot_status) -> u32 {
    status.all = 0;
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }
    if let Some(f) = dmub.hw_funcs.get_fw_status {
        *status = f(adev, dmub);
    }
    DMUB_STATUS_OK
}

/// `dmub_srv_update_reg_inbox0_status`: the DCN 2.0 family has no register
/// inbox0 response interrupt, so a pending command clears at once.
fn dmub_srv_update_reg_inbox0_status(dmub: &mut dmub_srv) {
    if dmub.reg_inbox0.is_pending {
        dmub.reg_inbox0.is_pending = false;
        // only update the reported count if commands aren't being batched
        if !dmub.reg_inbox0.is_multi_pending {
            dmub.reg_inbox0.num_reported = dmub.reg_inbox0.num_submitted;
        }
    }
}

/// `dmub_srv_update_inbox_status`.
pub fn dmub_srv_update_inbox_status(adev: &amdgpu_device, dmub: &mut dmub_srv) -> u32 {
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    if dmub.power_state != DMUB_POWER_STATE_D0 {
        return DMUB_STATUS_POWER_STATE_D3;
    }
    // update inbox1 state
    let rptr = dmub.hw_funcs.get_inbox1_rptr.map_or(0, |f| f(adev, dmub));
    if rptr > dmub.inbox1.rb.capacity {
        return DMUB_STATUS_HW_FAILURE;
    }
    let rb = &mut dmub.inbox1.rb;
    let advanced = if rb.rptr > rptr { rptr + rb.capacity - rb.rptr } else { rptr - rb.rptr };
    dmub.inbox1.num_reported += (advanced / DMUB_RB_CMD_SIZE) as u64;
    dmub.inbox1.rb.rptr = rptr;

    // update reg_inbox0
    dmub_srv_update_reg_inbox0_status(dmub);
    DMUB_STATUS_OK
}

/// `dmub_srv_wait_for_idle`.
pub fn dmub_srv_wait_for_idle(adev: &amdgpu_device, dmub: &mut dmub_srv, timeout_us: u32) -> u32 {
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    let mut i = 0;
    while i < timeout_us {
        let status = dmub_srv_update_inbox_status(adev, dmub);
        if status != DMUB_STATUS_OK {
            return status;
        }
        // check for idle
        if dmub_rb_empty(&dmub.inbox1.rb) && !dmub.reg_inbox0.is_pending {
            return DMUB_STATUS_OK;
        }
        crate::linux::udelay(1);
        i += 1;
    }
    DMUB_STATUS_TIMEOUT
}

/// `dmub_srv_wait_for_inbox_free`.
pub fn dmub_srv_wait_for_inbox_free(adev: &amdgpu_device, dmub: &mut dmub_srv, timeout_us: u32,
                                    num_free_required: u32) -> u32 {
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    let mut i = 0;
    while i < timeout_us {
        let status = dmub_srv_update_inbox_status(adev, dmub);
        if status != DMUB_STATUS_OK {
            return status;
        }
        // check for space in inbox1
        if dmub_rb_num_free(&dmub.inbox1.rb) >= num_free_required {
            return DMUB_STATUS_OK;
        }
        crate::linux::udelay(1);
        i += 1;
    }
    DMUB_STATUS_TIMEOUT
}

/// `dmub_srv_wait_for_pending`: the DCN 2.0 family has no register inbox0
/// response interrupt, so a pending register command counts as done.
pub fn dmub_srv_wait_for_pending(adev: &amdgpu_device, dmub: &dmub_srv, timeout_us: u32) -> u32 {
    let mut scratch_reg_inbox0 = dmub.reg_inbox0;
    let mut scratch_inbox1 = dmub.inbox1;
    let (Some(get_wptr), Some(get_rptr)) = (dmub.hw_funcs.get_inbox1_wptr, dmub.hw_funcs.get_inbox1_rptr) else {
        return DMUB_STATUS_INVALID;
    };
    if !dmub.hw_init {
        return DMUB_STATUS_INVALID;
    }
    let mut i = 0;
    while i <= timeout_us {
        scratch_inbox1.rb.wrpt = get_wptr(adev, dmub);
        scratch_inbox1.rb.rptr = get_rptr(adev, dmub);
        scratch_reg_inbox0.is_pending = false;
        if scratch_inbox1.rb.rptr > dmub.inbox1.rb.capacity {
            return DMUB_STATUS_HW_FAILURE;
        }
        // check current HW state first, but use command submission vs reported as a fallback
        if (dmub_rb_empty(&scratch_inbox1.rb) || dmub.inbox1.num_reported >= scratch_inbox1.num_submitted)
            && (!scratch_reg_inbox0.is_pending || dmub.reg_inbox0.num_reported >= scratch_reg_inbox0.num_submitted)
        {
            return DMUB_STATUS_OK;
        }
        crate::linux::udelay(1);
        i += 1;
    }
    DMUB_STATUS_TIMEOUT
}

/// `dmub_srv_cmd_get_response`: DCN 2.0 has no register inbox0, so the
/// answer is the ring entry the DMCUB wrote back.
pub fn dmub_srv_cmd_get_response(adev: &amdgpu_device, dmub: &dmub_srv, cmd_rsp: &mut dmub_rb_cmd) {
    dmub_rb_get_return_data(adev, dmub.cpu, &dmub.inbox1.rb, cmd_rsp);
}

/// `dmub_srv_get_diagnostic_data`.
pub fn dmub_srv_get_diagnostic_data(adev: &amdgpu_device, dmub: &mut dmub_srv) -> bool {
    let Some(f) = dmub.hw_funcs.get_diagnostic_data else { return false };
    f(adev, dmub);
    true
}

/// `dmub_srv_send_gpint_command` (dmub_srv.c): a general purpose interrupt
/// to the firmware, waited for up to `timeout_us`.
pub fn dmub_srv_send_gpint_command(adev: &amdgpu_device, dmub: &dmub_srv, command_code: u32, param: u16,
                                   timeout_us: u32) -> u32 {
    if !dmub.sw_init {
        return DMUB_STATUS_INVALID;
    }

    let (Some(set_gpint), Some(is_gpint_acked)) = (dmub.hw_funcs.set_gpint, dmub.hw_funcs.is_gpint_acked) else {
        return DMUB_STATUS_INVALID;
    };

    let reg = crate::dmub_dcn20::dmub_gpint_data_register::new(1, command_code, param as u32);

    set_gpint(adev, dmub, reg);

    for _ in 0..timeout_us {
        crate::linux::udelay(1);

        if is_gpint_acked(adev, dmub, reg) {
            return DMUB_STATUS_OK;
        }
    }

    DMUB_STATUS_TIMEOUT
}
