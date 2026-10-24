//! display/amdgpu_dm/amdgpu_dm.c (Linux 6.18.55): the DM block's
//! early_init, which sizes the display for DCN 2.1 and requests DMCUB, its
//! sw_init, which lays out the DMUB regions in one buffer, and the start of
//! its hw_init: `amdgpu_dm_init` creates Display Core and brings up the
//! DMCUB. Nothing dispatches `dm_hw_init` yet.

use crate::amdgpu_device::amdgpu_device;
use crate::amdgpu_ucode::amdgpu_ucode_request;
use crate::atom::amdgpu_atom_parse_data_header;
use crate::linux::{drm_err, drm_info, Line};
use crate::regs::*;
use crate::dmub_srv::*;
use crate::dc::{dc, dc_create, dc_init_data};
use crate::dc_types::dce_environment;
use crate::amdgpu_discovery::ip_version;
use alloc::boxed::Box;

/// `struct amdgpu_display_manager`, the part sw_init fills.
#[derive(Default)]
pub struct amdgpu_display_manager {
    /// `cgs_device`: amdgpu_cgs_create_device allocates it; nothing in
    /// this port reads through it.
    pub cgs_device: bool,
    pub dmcub_fw_version: u32,
    /// `dmub_srv`: the DMUB service until `dc_dmub_srv_create` moves it
    /// into DC; `dm_dmub_srv` finds it in either place.
    pub dmub_srv: Option<Box<dmub_srv>>,
    pub dmub_bo: Option<usize>,
    pub dmub_bo_gpu_addr: u64,
    pub dmub_bo_cpu_addr: Option<crate::amdgpu_object::amdgpu_kptr>,
    pub dmub_fb_info: Option<dmub_srv_fb_info>,
    /// `dc`: the Display Core instance `amdgpu_dm_init` creates.
    pub dc: Option<Box<dc>>,
    pub aux_hpd_discon_quirk: bool,
    pub edp0_on_dp1_quirk: bool,
    /// `bb_from_dmub`: the DMUB bounding box, DCN 4.0.1 only.
    pub bb_from_dmub: bool,
    /// `irq_handler_list_low_tab` / `irq_handler_list_high_tab`: the
    /// handlers per DAL irq source (amdgpu_dm_irq.c).
    pub irq_handler_list_low_tab: alloc::vec::Vec<alloc::vec::Vec<crate::amdgpu_dm_irq::dm_irq_handler>>,
    pub irq_handler_list_high_tab: alloc::vec::Vec<alloc::vec::Vec<crate::amdgpu_dm_irq::dm_irq_handler>>,
    /// `vupdate_params`: VUPDATE1..VUPDATE6.
    pub vupdate_params: [crate::amdgpu_dm_irq::common_irq_params; 6],
    pub dmub_outbox_params: [crate::amdgpu_dm_irq::common_irq_params; 1],
    pub display_indexes_num: u32,
    /// What DC drives the panel with, once it took it over.
    pub live: Option<dm_live>,
    /// Page flips completed since the driver's loop last took them.
    pub flip_events: alloc::vec::Vec<crate::drm_vblank::drm_flip_event>,
    /// The cursor plane's state, once the kernel gave it an image.
    pub cursor: Option<dm_cursor_plane>,
}

/// The cursor plane as DRM would hold it: its framebuffer (a VRAM buffer
/// of the largest cursor DCN 2.1 shows), the image's size and hotspot, and
/// where the pointer is.
#[derive(Clone, Copy, Debug)]
pub struct dm_cursor_plane {
    pub bo: usize,
    pub address: u64,
    pub kptr: crate::amdgpu_object::amdgpu_kptr,
    /// `crtc_w` / `crtc_h`, and the framebuffer's pitch in pixels.
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub x: i32,
    pub y: i32,
    /// Whether the plane has a framebuffer.
    pub visible: bool,
}

/// `struct amdgpu_crtc` (amdgpu_mode.h) with its `drm_crtc` and DRM vblank
/// state, the members DM reaches.
#[derive(Clone, Copy, Default, Debug)]
pub struct amdgpu_crtc {
    pub crtc_id: u32,
    pub otg_inst: i32,
    /// `base.enabled`.
    pub enabled: bool,
    /// `enum amdgpu_flip_status`.
    pub pflip_status: u32,
    /// `event`: the pending page-flip completion event, by its token.
    pub event: Option<u32>,
    pub dm_irq_params: dm_irq_params,
    pub vblank: crate::drm_vblank::drm_vblank_crtc,
}

/// `struct dm_irq_params` (amdgpu_dm_irq_params.h), the members reached.
#[derive(Clone, Copy, Default, Debug)]
pub struct dm_irq_params {
    pub stream: Option<usize>,
    pub active_planes: u32,
    pub last_flip_vblank: u64,
}

/// `dm_init_microcode`: DCN 2.1.0, Renoir's DMCUB unless the revision is
/// Green Sardine's.
pub fn dm_init_microcode(adev: &mut amdgpu_device) -> i32 {
    let fw_name_dmub = match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => {
            // ASICREV_IS_GREEN_SARDINE
            if adev.external_rev_id >= GREEN_SARDINE_A0 && adev.external_rev_id < 0xFF {
                FIRMWARE_GREEN_SARDINE_DMUB
            } else {
                FIRMWARE_RENOIR_DMUB
            }
        }
        // ASIC doesn't support DMUB.
        _ => return 0,
    };
    let (fw, r) = amdgpu_ucode_request(core::str::from_utf8(fw_name_dmub).unwrap_or(""));
    adev.dm_dmub_fw = fw;
    r
}

/// `dm_early_init`. CHIP_RENOIR takes the default branch of the
/// `asic_type` switch, then DCN 2.1.0.
pub fn dm_early_init(adev: &mut amdgpu_device) -> i32 {
    // if there is no object header, skip DM
    let has_object_header = adev.mode_info.atom_context.as_ref()
        .is_some_and(|ctx| amdgpu_atom_parse_data_header(ctx, DATA_TABLE_Object_Header).is_some());
    if !has_object_header {
        adev.harvest_ip_mask |= AMD_HARVEST_IP_DMU_MASK;
        drm_info("No object header, skipping DM");
        return -2;
    }

    match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => {
            adev.mode_info.num_crtc = 4;
            adev.mode_info.num_hpd = 4;
            adev.mode_info.num_dig = 4;
        }
        v => {
            Line::new().s("Unsupported DCE IP versions: ").hex(v).say();
            return -22;
        }
    }

    if adev.mode_info.funcs.is_none() {
        adev.mode_info.funcs = Some("dm_display_funcs");
    }
    adev.dc_enabled = true;

    dm_init_microcode(adev)
}


fn le32(b: &[u8], at: u32) -> u32 {
    let at = at as usize;
    b.get(at..at + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

// DMUB_WINDOW_0_INST_CONST to DMUB_WINDOW_SHARED_STATE in the frame
// buffer; DMUB_WINDOW_LSDMA_BUFFER is not initialized and so 0, the frame
// buffer as well.
pub static WINDOW_MEMORY_TYPE: [u32; DMUB_WINDOW_TOTAL as usize] = [DMUB_WINDOW_MEMORY_TYPE_FB; DMUB_WINDOW_TOTAL as usize];

/// The middle of `dm_dmub_sw_init`: create the DMUB service and calculate
/// the size of all the regions for it.
pub fn dm_dmub_regions(data: &[u8], vbios_size: u32, dmub_asic: u32, dmub_srv: &mut dmub_srv,
                       region_info: &mut dmub_srv_region_info) -> i32 {
    let inst_const_bytes = le32(data, DMCUB_HDR_INST_CONST_BYTES);
    let bss_data_bytes = le32(data, DMCUB_HDR_BSS_DATA_BYTES);
    let ucode_array_offset = le32(data, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES);
    // Create the DMUB service.
    let status = dmub_srv_create(dmub_srv, dmub_asic, 0);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error creating DMUB service: ").dec(status as u64).say();
        return -22;
    }
    // Calculate the size of all the regions for the DMUB service.
    let blob = |off: u32, len: u32| data.get(off as usize..(off as usize).saturating_add(len as usize));
    let inst_const_size = inst_const_bytes.wrapping_sub(PSP_HEADER_BYTES).wrapping_sub(PSP_FOOTER_BYTES);
    let bss_off = ucode_array_offset.wrapping_add(inst_const_bytes);
    let region_params = dmub_srv_region_params {
        inst_const_size,
        bss_data_size: bss_data_bytes,
        vbios_size,
        fw_bss_data: if bss_data_bytes != 0 { blob(bss_off, bss_data_bytes) } else { None },
        fw_inst_const: blob(ucode_array_offset.wrapping_add(PSP_HEADER_BYTES), inst_const_size),
        window_memory_type: &WINDOW_MEMORY_TYPE,
    };
    let status = dmub_srv_calc_region_info(dmub_srv, &region_params, region_info);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error calculating DMUB region info: ").dec(status as u64).say();
        return -22;
    }
    0
}

/// `dm_dmub_sw_init`.
fn dm_dmub_sw_init(adev: &mut amdgpu_device) -> i32 {
    let dmub_asic = match adev.ip_version(DCE_HWIP, 0) {
        v if v == 2 << 24 | 1 << 16 => DMUB_ASIC_DCN21,
        // ASIC doesn't support DMUB.
        _ => return 0,
    };
    let Some(fw) = adev.dm_dmub_fw.as_ref() else { return -22 };
    let data = &fw.data;
    adev.dm.dmcub_fw_version = le32(data, UCODE_HDR_UCODE_VERSION);
    let inst_const_bytes = le32(data, DMCUB_HDR_INST_CONST_BYTES);

    if adev.firmware.load_type == AMDGPU_FW_LOAD_PSP {
        let info = &mut adev.firmware.ucode[AMDGPU_UCODE_ID_DMCUB as usize];
        info.ucode_id = AMDGPU_UCODE_ID_DMCUB;
        info.fw = Some(crate::amdgpu_ucode::fw_source::dm_dmub);
        adev.firmware.fw_size = adev.firmware.fw_size
            .wrapping_add(crate::linux::align(inst_const_bytes as u64, crate::amdgpu_object::PAGE_SIZE) as u32);
        Line::new().s("Loading DMUB firmware via PSP: version=0x").hexw(adev.dm.dmcub_fw_version, 8).say();
    }

    let mut dmub_srv = dmub_srv::default();
    let mut region_info = dmub_srv_region_info::default();
    if dm_dmub_regions(data, adev.bios.len() as u32, dmub_asic, &mut dmub_srv, &mut region_info) != 0 {
        return -22;
    }
    adev.dm.dmub_srv = Some(Box::new(dmub_srv));

    // Allocate a framebuffer based on the total size of all the regions.
    let h = match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, region_info.fb_size as u64,
        crate::amdgpu_object::PAGE_SIZE, AMDGPU_GEM_DOMAIN_VRAM | AMDGPU_GEM_DOMAIN_GTT, true) {
        Ok(Some(h)) => h,
        Ok(None) => return -12,
        Err(r) => return r,
    };
    adev.dm.dmub_bo = Some(h);
    adev.dm.dmub_bo_gpu_addr = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
    adev.dm.dmub_bo_cpu_addr = crate::amdgpu_object::bo(adev, h).kptr;

    // Rebase the regions on the framebuffer address.
    let mut fb_info = dmub_srv_fb_info::default();
    let status = dmub_srv_calc_mem_info(adev.dm.dmub_srv.as_ref().unwrap(), &region_info, &WINDOW_MEMORY_TYPE,
        (0, adev.dm.dmub_bo_gpu_addr), (0, 0), &mut fb_info);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error calculating DMUB FB info: ").dec(status as u64).say();
        return -22;
    }
    adev.dm.dmub_fb_info = Some(fb_info);
    // dm_dmub_get_vbios_bounding_box: DCN 4.0.1 only.
    0
}

/// `dm_sw_init`. `load_dmcu_fw` returns 0 for DCN 2.1.0: it has no DMCU.
pub fn dm_sw_init(adev: &mut amdgpu_device) -> i32 {
    adev.dm.cgs_device = true;
    dm_dmub_sw_init(adev)
}

/// `retrieve_dmi_info` (amdgpu_dm_quirks.c): the quirks match DMI system
/// vendor and product names of Dell and HP machines. Not implemented: the
/// kernel hands modules no DMI strings, so no entry matches.
fn retrieve_dmi_info(dm: &mut amdgpu_display_manager) {
    dm.aux_hpd_discon_quirk = false;
    dm.edp0_on_dp1_quirk = false;
}

/// `amdgpu_stutter_quirk_list` (amdgpu_dm.c): vendor, device, subsystem
/// vendor, subsystem device, revision.
const AMDGPU_STUTTER_QUIRK_LIST: [(u16, u16, u16, u16, u8); 1] = [(0x1002, 0x15dd, 0x1002, 0x15dd, 0xc8)];

/// `dm_should_disable_stutter`.
fn dm_should_disable_stutter(adev: &amdgpu_device) -> bool {
    AMDGPU_STUTTER_QUIRK_LIST.iter().any(|&(vendor, device, sub_vendor, sub_device, revision)| {
        PCI_VENDOR_ID_ATI as u16 == vendor && adev.device == device && adev.subsystem_vendor == sub_vendor
            && adev.subsystem_device == sub_device && adev.revision == revision
    })
}

/// `dm_get_default_ips_mode`: ASICs older than DCN 3.5 have no IPS.
fn dm_get_default_ips_mode(adev: &amdgpu_device) -> u32 {
    let mut ret = DMUB_IPS_ENABLE;
    match adev.ip_version(DCE_HWIP, 0) {
        v if v == ip_version(3, 5, 0) || v == ip_version(3, 6, 0) || v == ip_version(3, 5, 1) => {
            ret = DMUB_IPS_RCG_IN_ACTIVE_IPS2_IN_OFF;
        }
        v => {
            if v < ip_version(3, 5, 0) {
                ret = DMUB_IPS_DISABLE_ALL;
            }
        }
    }
    ret
}


/// `amdgpu_device_seamless_boot_supported` (amdgpu_device.c).
fn amdgpu_device_seamless_boot_supported(adev: &amdgpu_device) -> bool {
    match crate::tables::amdgpu_seamless[0] {
        -1 => {}
        1 => return true,
        0 => return false,
        _ => {
            drm_err("Invalid value for amdgpu.seamless");
            return false;
        }
    }
    if adev.flags & AMD_IS_APU == 0 {
        return false;
    }
    if adev.mman.keep_stolen_vga_memory {
        return false;
    }
    adev.ip_version(DCE_HWIP, 0) >= ip_version(3, 0, 0)
}

/// `dm_dmub_hw_init`. The DMCUB on DCN 2.1 has no runtime register
/// offsets, no DPIA and no IPS, and DC has no DMCU or ABM.
pub fn dm_dmub_hw_init(adev: &mut amdgpu_device) -> i32 {
    let in_dc = adev.dm.dc.as_deref_mut().and_then(|d| d.ctx.dmub_srv.as_deref_mut()).and_then(|s| s.dmub.take());
    let Some(mut dmub_srv) = in_dc.or_else(|| adev.dm.dmub_srv.take()) else {
        // DMUB isn't supported on the ASIC.
        return 0;
    };
    let r = dm_dmub_hw_init_srv(adev, &mut dmub_srv);

    let dc = adev.dm.dc.as_deref_mut().expect("dm_dmub_hw_init runs after dc_create");
    if let Some(dc_srv) = dc.ctx.dmub_srv.as_deref_mut() {
        dc_srv.dmub = Some(dmub_srv);
    } else if r == Ok(true) {
        dc.ctx.dmub_srv = crate::dc_dmub_srv::dc_dmub_srv_create(dmub_srv);
    } else {
        adev.dm.dmub_srv = Some(dmub_srv);
    }
    match r {
        Ok(false) => 0,
        Err(e) => e,
        Ok(true) => {
            if adev.dm.dc.as_deref().is_none_or(|d| d.ctx.dmub_srv.is_none()) {
                drm_err("Couldn't allocate DC DMUB server!");
                return -12;
            }
            Line::new().s("DMUB hardware initialized: version=0x").hexw(adev.dm.dmcub_fw_version, 8).say();
            // The sanity-check overrides are DCN 3.1.x only.
            0
        }
    }
}

/// The DMUB service wherever it lives: in DC once `dc_dmub_srv_create` ran,
/// in the DM before.
pub fn dm_dmub_srv(adev: &amdgpu_device) -> Option<&dmub_srv> {
    adev.dm.dc.as_deref().and_then(|d| d.ctx.dmub_srv.as_deref()).and_then(|s| s.dmub.as_deref())
        .or(adev.dm.dmub_srv.as_deref())
}

/// The body of `dm_dmub_hw_init` up to `dc_dmub_srv_create`: Ok(true) when
/// the DMCUB came up and DC's handle is due, Ok(false) when the ASIC has no
/// DMCUB to bring up.
fn dm_dmub_hw_init_srv(adev: &mut amdgpu_device, dmub_srv: &mut dmub_srv) -> Result<bool, i32> {
    let Some(fb_info) = adev.dm.dmub_fb_info.as_ref() else {
        drm_err("No framebuffer info for DMUB service.");
        return Err(-22);
    };
    let Some(dmub_fw) = adev.dm_dmub_fw.as_ref() else {
        // Firmware required for DMUB support.
        drm_err("No firmware provided for DMUB.");
        return Err(-22);
    };
    let adev_r: &amdgpu_device = adev;

    let mut has_hw_support = false;
    let status = dmub_srv_has_hw_support(adev_r, dmub_srv, &mut has_hw_support);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error checking HW support for DMUB: ").dec(status as u64).say();
        return Err(-22);
    }

    if !has_hw_support {
        drm_info("DMUB unsupported on ASIC");
        return Ok(false);
    }

    // Reset DMCUB if it was previously running - before we overwrite its memory.
    let status = dmub_srv_hw_reset(adev_r, dmub_srv);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error resetting DMUB HW: ").dec(status as u64).say();
    }

    let data = &dmub_fw.data;
    let ucode_array_offset = le32(data, UCODE_HDR_UCODE_ARRAY_OFFSET_BYTES);
    let inst_const_bytes = le32(data, DMCUB_HDR_INST_CONST_BYTES);
    let fw_inst_const = ucode_array_offset.wrapping_add(PSP_HEADER_BYTES);
    let fw_bss_data = ucode_array_offset.wrapping_add(inst_const_bytes);

    // Copy firmware and bios info into FB memory.
    let fw_inst_const_size = inst_const_bytes.wrapping_sub(PSP_HEADER_BYTES).wrapping_sub(PSP_FOOTER_BYTES);
    let fw_bss_data_size = le32(data, DMCUB_HDR_BSS_DATA_BYTES);

    let Some(cpu) = adev_r.dm.dmub_bo_cpu_addr else { return Err(-22) };
    let blob = |off: u32, len: u32| data.get(off as usize..(off as usize).saturating_add(len as usize));
    let fb = |id: u32| fb_info.fb[id as usize];

    // With PSP loading, amdgpu_ucode_init_single_fw puts fw_inst_const in
    // cw0; otherwise this is the backdoor load.
    if adev_r.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        let Some(src) = blob(fw_inst_const, fw_inst_const_size) else { return Err(-22) };
        crate::amdgpu_gart::kptr_write(adev_r, cpu, fb(DMUB_WINDOW_0_INST_CONST).cpu_off, src);
    }

    if fw_bss_data_size != 0 {
        let Some(src) = blob(fw_bss_data, fw_bss_data_size) else { return Err(-22) };
        crate::amdgpu_gart::kptr_write(adev_r, cpu, fb(DMUB_WINDOW_2_BSS_DATA).cpu_off, src);
    }

    // Copy firmware bios info into FB memory.
    crate::amdgpu_gart::kptr_write(adev_r, cpu, fb(DMUB_WINDOW_3_VBIOS).cpu_off, &adev_r.bios);

    // Reset regions that need to be reset.
    for id in [DMUB_WINDOW_4_MAILBOX, DMUB_WINDOW_5_TRACEBUFF, DMUB_WINDOW_6_FW_STATE, DMUB_WINDOW_SHARED_STATE] {
        let w = fb(id);
        crate::amdgpu_gart::kptr_fill32(adev_r, cpu, w.cpu_off, w.size as u64, 0);
    }

    // Initialize hardware.
    let mut hw_params = dmub_srv_hw_params {
        cpu: Some(cpu),
        fb_base: adev_r.gmc.fb_start,
        fb_offset: adev_r.vm_manager.vram_base_offset,
        ..dmub_srv_hw_params::default()
    };

    // backdoor load firmware and trigger dmub running
    if adev_r.firmware.load_type != AMDGPU_FW_LOAD_PSP {
        hw_params.load_inst_const = true;
    }

    // dmcu is NULL on DCN 2.1: psp_version stays 0.
    for i in 0..fb_info.num_fb as usize {
        hw_params.fb[i] = Some(fb_info.fb[i]);
    }
    // DPIA (DCN 3.1.3+) and the IPS/PHY options (DCN 3.5+) do not apply.

    let status = dmub_srv_hw_init(adev_r, dmub_srv, &hw_params);
    if status != DMUB_STATUS_OK {
        Line::new().s("Error initializing DMUB HW: ").dec(status as u64).say();
        return Err(-22);
    }

    // Wait for firmware load to finish.
    let status = dmub_srv_wait_for_auto_load(adev_r, dmub_srv, 100000);
    if status != DMUB_STATUS_OK {
        Line::new().s("Wait for DMUB auto-load failed: ").dec(status as u64).say();
    }

    // Init DMCU and ABM if available: DCN 2.1 has neither.
    Ok(true)
}

/// `amdgpu_dm_init`, up to the DMCUB. `dc_hardware_init` (detect_edp_presence,
/// `hwss.init_hw`, the D0 power-state command) and everything after it —
/// the system context, freesync, the work queues, outbox, the DRM device —
/// are the next stages; until then this returns after `dm_dmub_hw_init`.
pub fn amdgpu_dm_init(adev: &mut amdgpu_device) -> i32 {
    if crate::amdgpu_dm_irq::amdgpu_dm_irq_init(adev) != 0 {
        drm_err("failed to initialize DM IRQ support.");
        return -22;
    }

    let mut init_data = dc_init_data::default();
    init_data.asic_id.chip_family = adev.family;

    init_data.asic_id.pci_revision_id = adev.revision as u32;
    init_data.asic_id.hw_internal_rev = adev.external_rev_id;
    init_data.asic_id.chip_id = adev.device as u32;

    init_data.asic_id.vram_width = adev.gmc.vram_width;
    // vram_type is left unset, as Linux leaves it; atombios_base_address
    // is the VBIOS image, which the port reads through adev.bios.

    // cgs_device was created in dm_sw_init()
    init_data.cgs_device = adev.dm.cgs_device;

    init_data.dce_environment = dce_environment::DCE_ENV_PRODUCTION_DRV;

    match adev.ip_version(DCE_HWIP, 0) {
        v if v == ip_version(2, 1, 0) => {
            init_data.flags.disable_dmcu = !matches!(adev.dm.dmcub_fw_version,
                // development, linux-firmware.git hash 6d9f399, hash 9a0b0f4
                0 | 0x1 | 0x0100_0000);
        }
        v if v == ip_version(2, 0, 3) => init_data.flags.disable_dmcu = true,
        _ => {}
    }

    // APU support S/G display by default except ASICs before Carrizo and
    // RAVEN1 (users reported stability issues).
    let sg_display = crate::tables::amdgpu_sg_display[0];
    if adev.asic_type < CHIP_CARRIZO {
        init_data.flags.gpu_vm_support = false;
    } else if adev.asic_type == CHIP_RAVEN {
        init_data.flags.gpu_vm_support = if adev.apu_flags & AMD_APU_IS_RAVEN != 0 { false } else { sg_display != 0 };
    } else if adev.ip_version(DCE_HWIP, 0) == ip_version(2, 0, 3) {
        init_data.flags.gpu_vm_support = sg_display == 1;
    } else {
        init_data.flags.gpu_vm_support = sg_display != 0 && adev.flags & AMD_IS_APU != 0;
    }

    adev.mode_info.gpu_vm_support = init_data.flags.gpu_vm_support;

    let feature_mask = crate::tables::amdgpu_dc_feature_mask[0];
    if feature_mask & DC_FBC_MASK != 0 {
        init_data.flags.fbc_support = true;
    }
    if feature_mask & DC_MULTI_MON_PP_MCLK_SWITCH_MASK != 0 {
        init_data.flags.multi_mon_pp_mclk_switch = true;
    }
    if feature_mask & DC_DISABLE_FRACTIONAL_PWM_MASK != 0 {
        init_data.flags.disable_fractional_pwm = true;
    }
    if feature_mask & DC_EDP_NO_POWER_SEQUENCING != 0 {
        init_data.flags.edp_no_power_sequencing = true;
    }
    if feature_mask & DC_DISABLE_LTTPR_DP1_4A != 0 {
        init_data.flags.allow_lttpr_non_transparent_mode_dp1_4a = true;
    }
    if feature_mask & DC_DISABLE_LTTPR_DP2_0 != 0 {
        init_data.flags.allow_lttpr_non_transparent_mode_dp2_0 = true;
    }

    init_data.flags.seamless_boot_edp_requested = false;

    if amdgpu_device_seamless_boot_supported(adev) {
        init_data.flags.seamless_boot_edp_requested = true;
        init_data.flags.allow_seamless_boot_optimization = true;
    }

    init_data.flags.enable_mipi_converter_optimization = true;

    init_data.dcn_reg_offsets = adev.reg_offset[DCE_HWIP as usize][0].clone();
    init_data.nbio_reg_offsets = adev.reg_offset[NBIO_HWIP as usize][0].clone();
    init_data.clk_reg_offsets = adev.reg_offset[CLK_HWIP as usize][0].clone();

    let debug_mask = crate::tables::amdgpu_dc_debug_mask[0];
    init_data.flags.disable_ips = if debug_mask & DC_DISABLE_IPS != 0 {
        DMUB_IPS_DISABLE_ALL
    } else if debug_mask & DC_DISABLE_IPS_DYNAMIC != 0 {
        DMUB_IPS_DISABLE_DYNAMIC
    } else if debug_mask & DC_DISABLE_IPS2_DYNAMIC != 0 {
        DMUB_IPS_RCG_IN_ACTIVE_IPS2_IN_OFF
    } else if debug_mask & DC_FORCE_IPS_ENABLE != 0 {
        DMUB_IPS_ENABLE
    } else {
        dm_get_default_ips_mode(adev)
    };

    init_data.flags.disable_ips_in_vpb = 0;

    // DCN35 and above supports dynamic DTBCLK switch
    if adev.ip_version(DCE_HWIP, 0) >= ip_version(3, 5, 0) {
        init_data.flags.allow_0_dtb_clk = true;
    }

    // Enable DWB for tested platforms only
    if adev.ip_version(DCE_HWIP, 0) >= ip_version(3, 0, 0) {
        init_data.num_virtual_links = 1;
    }

    retrieve_dmi_info(&mut adev.dm);
    if adev.dm.edp0_on_dp1_quirk {
        init_data.flags.support_edp0_on_dp1 = true;
    }

    init_data.bb_from_dmub = adev.dm.bb_from_dmub;

    // Display Core create.
    adev.dm.dc = dc_create(adev, &init_data);

    let Some(dc) = adev.dm.dc.as_deref_mut() else {
        Line::new().s("Display Core failed to initialize with v").s(crate::dc_types::DC_VER).say();
        return -22;
    };
    Line::new().s("Display Core v").s(crate::dc_types::DC_VER).s(" initialized on DCN 2.1").say();

    if debug_mask & DC_DISABLE_PIPE_SPLIT != 0 {
        dc.debug.force_single_disp_pipe_split = false;
        dc.debug.pipe_split_policy = crate::dc::pipe_split_policy::MPC_SPLIT_AVOID;
    }

    if adev.asic_type != CHIP_CARRIZO && adev.asic_type != CHIP_STONEY {
        dc.debug.disable_stutter = crate::tables::amdgpu_pp_feature_mask[0] & PP_STUTTER_MODE == 0;
    }
    if dm_should_disable_stutter(adev) {
        adev.dm.dc.as_deref_mut().unwrap().debug.disable_stutter = true;
    }
    let dc = adev.dm.dc.as_deref_mut().unwrap();

    if debug_mask & DC_DISABLE_STUTTER != 0 {
        dc.debug.disable_stutter = true;
    }
    if debug_mask & DC_DISABLE_DSC != 0 {
        dc.debug.disable_dsc = true;
    }
    if debug_mask & DC_DISABLE_CLOCK_GATING != 0 {
        dc.debug.disable_clock_gate = true;
    }
    if debug_mask & DC_FORCE_SUBVP_MCLK_SWITCH != 0 {
        dc.debug.force_subvp_mclk_switch = true;
    }
    if debug_mask & DC_DISABLE_SUBVP_FAMS != 0 {
        dc.debug.force_disable_subvp = true;
        dc.debug.fams2_config_enable = false;
    }
    if debug_mask & DC_ENABLE_DML2 != 0 {
        dc.debug.using_dml2 = true;
        dc.debug.using_dml21 = true;
    }
    if debug_mask & DC_HDCP_LC_FORCE_FW_ENABLE != 0 {
        dc.debug.hdcp_lc_force_fw_enable = true;
    }
    if debug_mask & DC_HDCP_LC_ENABLE_SW_FALLBACK != 0 {
        dc.debug.hdcp_lc_enable_sw_fallback = true;
    }
    if debug_mask & DC_SKIP_DETECTION_LT != 0 {
        dc.debug.skip_detection_link_training = true;
    }

    dc.debug.visual_confirm = crate::tables::amdgpu_dc_visual_confirm[0];

    // TODO: Remove after DP2 receiver gets proper support of Cable ID feature
    dc.debug.ignore_cable_id = true;

    if dc.caps.dp_hdmi21_pcon_support {
        drm_info("DP-HDMI FRL PCON supported");
    }

    let r = dm_dmub_hw_init(adev);
    if r != 0 {
        Line::new().s("DMUB interface failed to initialize: status=").dec(r as i64 as u64).say();
        // error: amdgpu_dm_fini → dc_destroy.
        adev.dm.dc = None;
        return -22;
    }

    0
}

/// `dm_hw_init`: the DAL display manager. `amdgpu_dm_hpd_init` and the OEM
/// i2c bus follow `amdgpu_dm_init`'s later stages.
pub fn dm_hw_init(adev: &mut amdgpu_device) -> i32 {
    amdgpu_dm_init(adev)
}

/// Whether `set gpu_amd_renoir.dc on` (or `live`) asks for Display Core on
/// this boot.
pub fn dc_requested() -> bool {
    let mut buf = [0u8; 8];
    let n = npk_sys::config_get(b"gpu_amd_renoir.dc", &mut buf);
    n > 0 && matches!(&buf[..n as usize], b"on" | b"1" | b"live")
}

/// What `dm_hw_init` built and found, one line per object, for reading
/// the bring-up off a device's log.
pub fn dm_report(adev: &amdgpu_device) {
    let Some(dc) = adev.dm.dc.as_deref() else {
        Line::new().s("dc: no Display Core").say();
        return;
    };
    let pool = dc.res_pool.as_deref();
    Line::new().s("dc: links ").dec(dc.link_count as u64).s(", streams ").dec(dc.caps.max_streams as u64)
        .s(", audios ").dec(dc.caps.max_audios as u64).s(", pipes ").dec(pool.map_or(0, |p| p.pipe_count) as u64)
        .s(", internal displays ").dec(dc.caps.num_of_internal_disp as u64).say();
    for l in &dc.links {
        Line::new().s("dc: link ").dec(l.link_index as u64).s(" connector ").dec(l.link_id.id as u64)
            .s(" signal 0x").hex(l.connector_signal).s(" internal ").dec(l.is_internal_display as u64)
            .s(" eng ").dec(l.eng_id as u64).s(" hpd ").dec(l.hpd_src as u64).s(" ddc ").dec(l.ddc_hw_inst as u64)
            .s(" panel_cntl ").dec(l.panel_cntl.is_some() as u64).say();
    }
    if let Some(c) = dc.clk_mgr.as_deref() {
        let b = &c.base.boot_snapshot;
        Line::new().s("dc: clk smu_ver 0x").hex(c.smu_ver as u32).s(" vco ").dec(c.base.dentist_vco_freq_khz as u64)
            .s(" kHz dprefclk ").dec(c.base.dprefclk_khz as u64).s(" kHz smu ").dec(c.smu_present as u64)
            .s(" dpm ").dec(c.dpm_present as u64).s(" retraining_off ").dec(c.periodic_retraining_disabled as u64)
            .say();
        Line::new().s("dc: boot clocks dispclk ").dec(b.dispclk as u64).s(" dppclk ").dec(b.dppclk as u64)
            .s(" dcfclk ").dec(b.dcfclk as u64).s(" dprefclk ").dec(b.dprefclk as u64).s(" fclk ").dec(b.fclk as u64)
            .say();
    }
    if let Some(d) = dm_dmub_srv(adev) {
        let r = &crate::dmub_dcn20::dmub_srv_dcn21_regs.offset;
        Line::new().s("dmub: hw_init ").dec(d.hw_init as u64).s(" fw_version 0x").hexw(d.fw_version, 8)
            .s(" DMCUB_CNTL 0x").hexw(crate::dc_helper::dm_read_reg(adev, r.DMCUB_CNTL), 8)
            .s(" boot status 0x").hexw(crate::dc_helper::dm_read_reg(adev, r.DMCUB_SCRATCH0), 8)
            .s(" dc handle ").dec(dc.ctx.dmub_srv.is_some() as u64).say();
    }
}

/// `dm_execute_dmub_cmd` (amdgpu_dm.c).
pub fn dm_execute_dmub_cmd(adev: &amdgpu_device, dc_srv: &mut crate::dc_dmub_srv::dc_dmub_srv,
                           debug: &crate::dc::dc_debug_options, cmd: &mut dmub_rb_cmd, wait_type: u32) -> bool {
    crate::dc_dmub_srv::dc_dmub_srv_cmd_run(adev, dc_srv, debug, cmd, wait_type)
}

/// `dm_execute_dmub_cmd_list` (amdgpu_dm.c).
pub fn dm_execute_dmub_cmd_list(adev: &amdgpu_device, dc_srv: &mut crate::dc_dmub_srv::dc_dmub_srv,
                                debug: &crate::dc::dc_debug_options, cmd: &mut [dmub_rb_cmd], wait_type: u32) -> bool {
    crate::dc_dmub_srv::dc_dmub_srv_cmd_run_list(adev, dc_srv, debug, cmd, wait_type)
}

/// `mmhub_read_system_context` (amdgpu_dm.c): the address space DC programs
/// into the DCHUB — the system aperture over the frame buffer (and AGP when
/// it is on) and the GART as page table. Raven2, Renoir and Green Sardine
/// cannot use VRAM past MC_VM_SYSTEM_APERTURE_HIGH_ADDR, so the aperture
/// reaches one unit further (a VM fault and hang otherwise).
pub fn mmhub_read_system_context(adev: &amdgpu_device) -> crate::dc::dc_phy_addr_space_config {
    let mut pa_config = crate::dc::dc_phy_addr_space_config::default();
    let gmc = &adev.gmc;
    let wa = adev.apu_flags & (AMD_APU_IS_RAVEN2 | AMD_APU_IS_RENOIR | AMD_APU_IS_GREEN_SARDINE) != 0;

    let agp_base: u32 = 0;
    let agp_bot = (gmc.agp_start >> 24) as u32;
    let agp_top = (gmc.agp_end >> 24) as u32;

    let (logical_addr_low, logical_addr_high): (u32, u32);
    // AGP aperture is disabled
    if agp_bot > agp_top {
        logical_addr_low = (gmc.fb_start >> 18) as u32;
        logical_addr_high = if wa { ((gmc.fb_end >> 18) + 0x1) as u32 } else { (gmc.fb_end >> 18) as u32 };
    } else {
        logical_addr_low = (gmc.fb_start.min(gmc.agp_start) >> 18) as u32;
        logical_addr_high = if wa {
            ((gmc.fb_end >> 18) + 0x1).max(gmc.agp_end >> 18) as u32
        } else {
            (gmc.fb_end.max(gmc.agp_end) >> 18) as u32
        };
    }

    let pt_base = crate::amdgpu_gmc::amdgpu_gmc_pd_addr(adev, adev.gart.bo.expect("GART table"));

    // PHYSICAL_ADDRESS_LOC: the 32-bit halves put together again.
    let page_table_start = gmc.gart_start >> AMDGPU_GPU_PAGE_SHIFT;
    let page_table_end = gmc.gart_end >> AMDGPU_GPU_PAGE_SHIFT;
    let page_table_base = pt_base;

    let a = &mut pa_config.system_aperture;
    a.start_addr = (logical_addr_low as u64) << 18;
    a.end_addr = (logical_addr_high as u64) << 18;

    a.agp_base = (agp_base as u64) << 24;
    a.agp_bot = (agp_bot as u64) << 24;
    a.agp_top = (agp_top as u64) << 24;

    a.fb_base = gmc.fb_start;
    a.fb_offset = adev.vm_manager.vram_base_offset;
    a.fb_top = gmc.fb_end;

    pa_config.gart_config.page_table_start_addr = page_table_start << 12;
    pa_config.gart_config.page_table_end_addr = page_table_end << 12;
    pa_config.gart_config.page_table_base_addr = page_table_base;

    pa_config.is_hvm_enabled = adev.mode_info.gpu_vm_support;

    pa_config
}

/// What the firmware left on the screen before DC takes over: the OTG's
/// timing and the surface its HUBP scans out.
#[derive(Clone, Copy, Default, Debug)]
pub struct firmware_scanout {
    pub link: usize,
    pub tg_inst: u32,
    pub timing: crate::dc_hw_types::dc_crtc_timing,
    pub address: u64,
    pub pitch: i32,
    pub width: i32,
    pub height: i32,
    /// `enum surface_pixel_format`.
    pub format: u32,
}

/// The eDP the firmware lit (DIG enabled, front end and OTG as
/// `dc_validate_boot_timing` finds them) and the HUBP feeding that OTG.
/// Read before `dc_hardware_init`, whose `init_pipes` blanks every pipe.
pub fn dm_read_firmware_scanout(adev: &amdgpu_device, dc: &crate::dc::dc) -> Option<firmware_scanout> {
    use crate::dc_helper::{reg_get, reg_get_n};
    use crate::dcn21_link_encoder::ENGINE_ID_UNKNOWN;
    let li = dc.links.iter().position(|l| l.connector_signal == LF_SIGNAL_TYPE_EDP)?;
    let enc = dc.links[li].link_enc.as_ref()?;
    if !(enc.funcs().is_dig_enabled.unwrap())(adev, enc) {
        return None;
    }
    let enc_inst = (enc.funcs().get_dig_frontend.unwrap())(adev, enc);
    if enc_inst == ENGINE_ID_UNKNOWN {
        return None;
    }
    let pool = dc.res_pool.as_deref()?;
    let se = (0..pool.stream_enc_count as usize).find(|&i| pool.stream_enc[i].as_ref().unwrap().base().id as u32 == enc_inst)?;
    let e = pool.stream_enc[se].as_ref().unwrap();
    let tg_inst = (e.funcs().dig_source_otg.unwrap())(adev, e);
    if tg_inst >= pool.timing_generator_count {
        return None;
    }
    let tg = pool.timing_generators[tg_inst as usize].as_deref()?;

    let mut timing = crate::dc_hw_types::dc_crtc_timing::default();
    (tg.funcs().get_hw_timing.unwrap())(adev, tg, &mut timing);
    let mut s = crate::dcn20_optc::dcn_otg_state::default();
    crate::dcn20_optc::optc1_read_otg_state(adev, tg, &mut s);
    // optc1_program_timing writes polarity 0 for a positive sync.
    timing.flags.HSYNC_POSITIVE_POLARITY = s.h_sync_a_pol == 0;
    timing.flags.VSYNC_POSITIVE_POLARITY = s.v_sync_a_pol == 0;
    let dp = pool.dp_clock_source.as_deref()?;
    (dp.funcs().get_pixel_clk_frequency_100hz.unwrap())(adev, dp, tg_inst, &mut timing.pix_clk_100hz);
    (e.funcs().dp_get_pixel_format.unwrap())(adev, e, &mut timing.pixel_encoding, &mut timing.display_color_depth);
    timing.scan_type = SCANNING_TYPE_NODATA;

    for h in pool.hubps.iter().take(pool.pipe_count as usize).flatten() {
        let c = h.rsm();
        let [vtg, blank] = reg_get_n!(adev, c, DCHUBP_CNTL, HUBP_VTG_SEL, HUBP_BLANK_EN);
        if vtg != tg_inst || blank != 0 {
            continue;
        }
        let lo = reg_get!(adev, c, DCSURF_PRIMARY_SURFACE_ADDRESS, PRIMARY_SURFACE_ADDRESS);
        let hi = reg_get!(adev, c, DCSURF_PRIMARY_SURFACE_ADDRESS_HIGH, PRIMARY_SURFACE_ADDRESS_HIGH);
        let pitch = reg_get!(adev, c, DCSURF_SURFACE_PITCH, PITCH) as i32 + 1;
        let [width, height] = reg_get_n!(adev, c, DCSURF_PRI_VIEWPORT_DIMENSION, PRI_VIEWPORT_WIDTH, PRI_VIEWPORT_HEIGHT);
        let pf = reg_get!(adev, c, DCSURF_SURFACE_CONFIG, SURFACE_PIXEL_FORMAT);
        let blue_bar = reg_get!(adev, c, HUBPRET_CONTROL, CROSSBAR_SRC_CB_B);
        // The mapping of hubp1_program_pixel_format, 32-bit formats only.
        let format = match (pf, blue_bar) {
            (8, 2) => SURFACE_PIXEL_FORMAT_GRPH_ARGB8888,
            (8, _) => SURFACE_PIXEL_FORMAT_GRPH_ABGR8888,
            (10, 2) => SURFACE_PIXEL_FORMAT_GRPH_ARGB2101010,
            (10, _) => SURFACE_PIXEL_FORMAT_GRPH_ABGR2101010,
            _ => return None,
        };
        return Some(firmware_scanout {
            link: li,
            tg_inst,
            timing,
            address: (hi as u64) << 32 | lo as u64,
            pitch,
            width: width as i32,
            height: height as i32,
            format,
        });
    }
    None
}

/// Whether `set gpu_amd_renoir.dc live` asks DC to drive the display.
pub fn dc_live_requested() -> bool {
    let mut buf = [0u8; 8];
    let n = npk_sys::config_get(b"gpu_amd_renoir.dc", &mut buf);
    n > 0 && &buf[..n as usize] == b"live"
}

/// The framebuffer the kernel draws into, as a DRM framebuffer gives it to
/// `fill_plane_buffer_attributes`: MC address, pitch in pixels, size.
#[derive(Clone, Copy, Debug)]
pub struct dm_framebuffer {
    pub address: u64,
    pub pitch: i32,
    pub width: i32,
    pub height: i32,
}

/// DC takes the panel over from the firmware: the rest of `amdgpu_dm_init`
/// DC needs (`dc_hardware_init`, the system context,
/// `amdgpu_dm_initialize_drm_device` with the boot detection and the
/// interrupt handlers), then what an atomic commit asks of DC for one
/// stream and one plane — the stream in the timing the firmware set, the
/// plane on `fb`, so the picture the kernel draws stays. DCN 2.1 has no
/// seamless boot: the panel goes dark between `dc_hardware_init` and the
/// commit.
pub fn dm_take_over_display(adev: &mut amdgpu_device, fb: &dm_framebuffer) -> bool {
    use crate::{dc, dc_state, dc_stream};
    let Some(mut dcb) = adev.dm.dc.take() else { return false };
    let fw = dm_init_hw(adev, &mut dcb, fb);
    adev.dm.dc = Some(dcb);
    let Some(fw) = fw else { return false };

    if amdgpu_dm_initialize_drm_device(adev) != 0 {
        return false;
    }
    // drm_vblank_init: one vblank per display index.
    for c in adev.mode_info.crtcs.iter_mut().flatten() {
        c.vblank = Default::default();
    }

    let Some(mut dcb) = adev.dm.dc.take() else { return false };
    let committed = dm_commit(adev, &mut dcb, &fw, fb);
    let status = committed.and_then(|(stream, _)| dc::dc_stream_get_status(&dcb, stream))
        .map(|i| dcb.current_state.as_deref().unwrap().stream_status[i].primary_otg_inst);
    adev.dm.dc = Some(dcb);
    let (Some((stream, plane)), Some(otg)) = (committed, status) else { return false };

    // amdgpu_dm_commit_streams: the CRTC drives the stream's OTG.
    if let Some(acrtc) = adev.mode_info.crtcs[0].as_mut() {
        acrtc.otg_inst = otg;
        acrtc.enabled = true;
        acrtc.dm_irq_params.stream = Some(stream);
        acrtc.dm_irq_params.active_planes = 1;
    }
    adev.dm.live = Some(dm_live { crtc: 0, stream, plane });
    manage_dm_interrupts(adev, 0, true);
    return true;

    /// The rest of `amdgpu_dm_init` before the DRM device: the firmware's
    /// scanout read first, as `dc_hardware_init` blanks it.
    fn dm_init_hw(adev: &amdgpu_device, dc: &mut dc::dc, fb: &dm_framebuffer) -> Option<firmware_scanout> {
        let Some(fw) = dm_read_firmware_scanout(adev, dc) else {
            Line::new().s("dc live: no firmware scanout on an eDP, staying with the firmware").say();
            return None;
        };
        let t = &fw.timing;
        Line::new().s("dc live: firmware scanout otg ").dec(fw.tg_inst as u64).s(" ").dec(t.h_addressable as u64)
            .s("x").dec(t.v_addressable as u64).s(" total ").dec(t.h_total as u64).s("x").dec(t.v_total as u64)
            .s(" pclk ").dec(t.pix_clk_100hz as u64).s("00 Hz surface ").hex64(fw.address).s(" pitch ")
            .dec(fw.pitch as u64).s(" ").dec(fw.width as u64).s("x").dec(fw.height as u64).s(" format ")
            .dec(fw.format as u64).say();
        Line::new().s("dc live: framebuffer ").hex64(fb.address).s(" pitch ").dec(fb.pitch as u64).s(" ")
            .dec(fb.width as u64).s("x").dec(fb.height as u64).say();

        dc::dc_hardware_init(adev, dc);

        if adev.flags & AMD_IS_APU != 0 && adev.asic_type >= CHIP_CARRIZO {
            let pa_config = mmhub_read_system_context(adev);
            dc::dc_setup_system_context(adev, dc, &pa_config);
        }
        Some(fw)
    }

    /// The atomic commit of the firmware's mode on `fb`: the stream and
    /// plane, or None.
    fn dm_commit(adev: &amdgpu_device, dc: &mut dc::dc, fw: &firmware_scanout, fb: &dm_framebuffer)
                 -> Option<(usize, usize)> {
        let t = &fw.timing;
        if dc.links[fw.link].local_sink.is_none() {
            Line::new().s("dc live: no sink on the eDP link").say();
            return None;
        }

        let stream = dc_stream::dc_create_stream_for_sink(dc, fw.link)?;
        {
            let st = dc.streams[stream].as_deref_mut().unwrap();
            st.timing = fw.timing;
            st.src = crate::dc_hw_types::rect { x: 0, y: 0, width: t.h_addressable as i32, height: t.v_addressable as i32 };
            st.dst = st.src;
            st.output_color_space = COLOR_SPACE_SRGB;
            // amdgpu_dm_set_atomic_regamma without a gamma LUT: bypass.
            st.out_transfer_func.r#type = TF_TYPE_BYPASS;
            st.out_transfer_func.tf = TRANSFER_FUNCTION_LINEAR;
        }

        let plane = dc_stream::dc_create_plane_state(dc);
        {
            let pl = dc.planes[plane].as_deref_mut().unwrap();
            pl.format = fw.format;
            pl.plane_size.surface_size = crate::dc_hw_types::rect { x: 0, y: 0, width: fb.width, height: fb.height };
            pl.plane_size.surface_pitch = fb.pitch;
            pl.src_rect = pl.plane_size.surface_size;
            pl.dst_rect = crate::dc_hw_types::rect { x: 0, y: 0, width: t.h_addressable as i32, height: t.v_addressable as i32 };
            pl.clip_rect = pl.dst_rect;
            pl.tiling_info.swizzle = DC_SW_LINEAR;
            pl.rotation = ROTATION_ANGLE_0;
            pl.visible = true;
            pl.color_space = COLOR_SPACE_SRGB;
            pl.address.r#type = PLN_ADDR_TYPE_GRAPHICS;
            pl.address.u[0] = fb.address;
            pl.global_alpha_value = 0xff;
            // amdgpu_dm_update_plane_color_mgmt without a degamma: bypass.
            pl.in_transfer_func.r#type = TF_TYPE_BYPASS;
            pl.in_transfer_func.tf = TRANSFER_FUNCTION_LINEAR;
        }

        let mut context = dc_state::dc_state_create_current_copy(dc);
        let added = dc_state::dc_state_add_stream(dc, &mut context, stream);
        let plane_added = dc_state::dc_state_add_plane(dc, stream, plane, &mut context);
        let valid = crate::dc_resource::dc_validate_global_state(dc, &mut context, DC_VALIDATE_MODE_AND_PROGRAMMING);
        dc_state::dc_state_release(dc, context);
        Line::new().s("dc live: add stream ").dec(added as u64).s(", add plane ").dec(plane_added as u64)
            .s(", validate ").dec(valid as u64).say();
        if added != crate::dc_types::dc_status::DC_OK || !plane_added || valid != crate::dc_types::dc_status::DC_OK {
            return None;
        }

        let streams = [stream];
        let params = dc::dc_commit_streams_params {
            streams: &streams,
            power_source: dc_state::dc_power_source_type::DC_POWER_SOURCE_AC,
        };
        let committed = dc::dc_commit_streams(adev, dc, &params);
        let mut srf = [dc::dc_surface_update { surface: plane, ..Default::default() }];
        let updated = dc::dc_update_planes_and_stream(adev, dc, &mut srf, stream, None);
        Line::new().s("dc live: commit streams ").dec(committed as u64).s(", update planes ").dec(updated as u64).say();
        (committed == crate::dc_types::dc_status::DC_OK && updated).then_some((stream, plane))
    }
}

/// The stream and plane DC drives the panel with, and the CRTC showing
/// them.
#[derive(Clone, Copy, Debug)]
pub struct dm_live {
    pub crtc: usize,
    pub stream: usize,
    pub plane: usize,
}

/// `amdgpu_dm_initialize_drm_device`, the part without DRM objects: the
/// CRTCs, the DMUB outbox handler, the boot detection of every link, and
/// the DCN interrupt handlers. Planes, encoders and connectors are DRM's;
/// PSR and Replay caps join with their stages.
fn amdgpu_dm_initialize_drm_device(adev: &mut amdgpu_device) -> i32 {
    use crate::link_detection;
    let max_streams = adev.dm.dc.as_deref().unwrap().caps.max_streams;

    adev.dm.display_indexes_num = max_streams;
    // Update the actual used number of crtc
    adev.mode_info.num_crtc = adev.dm.display_indexes_num;

    crate::amdgpu_dm_irq::amdgpu_dm_set_irq_funcs(adev);

    for i in 0..max_streams {
        amdgpu_dm_crtc_init(adev, i);
    }

    // DCN 2.1 has the DMUB outbox.
    if register_outbox_irq_handlers(adev) != 0 {
        drm_err("DM: Failed to initialize IRQ");
        return -22;
    }

    let mut dcb = adev.dm.dc.take().unwrap();
    let dc = &mut *dcb;
    for l in 0..dc.link_count as usize {
        if dc.links[l].connector_signal == LF_SIGNAL_TYPE_VIRTUAL {
            continue;
        }
        crate::amdgpu_dm_helpers::amdgpu_dm_connector_init(dc, l);
        let mut new_connection_type = 0;
        if !link_detection::dc_link_detect_connection_type(adev, dc, l, &mut new_connection_type) {
            drm_err("KMS: Failed to detect connector");
        }
        link_detection::dc_link_detect(adev, dc, l, link_detection::dc_detect_reason::DETECT_REASON_BOOT);
    }
    adev.dm.dc = Some(dcb);

    // Software is initialized. Now we can register interrupt handlers.
    if dcn10_register_irq_handlers(adev) != 0 {
        drm_err("DM: Failed to initialize IRQ");
        return -22;
    }

    0
}

/// `amdgpu_dm_crtc_init` (amdgpu_dm_crtc.c), without the DRM CRTC: no OTG
/// until a commit binds one.
fn amdgpu_dm_crtc_init(adev: &mut amdgpu_device, crtc_index: u32) {
    let acrtc = amdgpu_crtc { crtc_id: crtc_index, otg_inst: -1, ..Default::default() };
    let i = crtc_index as usize;
    if adev.mode_info.crtcs.len() <= i {
        adev.mode_info.crtcs.resize(i + 1, None);
    }
    adev.mode_info.crtcs[i] = Some(acrtc);
}

/// `register_outbox_irq_handlers`.
fn register_outbox_irq_handlers(adev: &mut amdgpu_device) -> i32 {
    use crate::amdgpu_dm_irq::*;
    let src = adev.dmub_outbox_irq;
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_DCE,
        DCN_1_0__SRCID__DMCUB_OUTBOX_LOW_PRIORITY_READY_INT, &src, "dmub_outbox_irq");
    if r != 0 {
        drm_err("Failed to add outbox irq id!");
        return r;
    }

    let dc = adev.dm.dc.as_deref().unwrap();
    if dc.ctx.dmub_srv.is_some() {
        let i = DCN_1_0__SRCID__DMCUB_OUTBOX_LOW_PRIORITY_READY_INT;
        let int_params = dc_interrupt_params {
            int_context: dc_interrupt_context::INTERRUPT_LOW_IRQ_CONTEXT,
            irq_source: crate::dc::dc_interrupt_to_irq_source(dc, i, 0),
        };

        adev.dm.dmub_outbox_params[0].irq_src = int_params.irq_source;

        if !amdgpu_dm_irq_register_interrupt(adev, &int_params, dm_irq_handler::dm_dmub_outbox1_low_irq) {
            return -12;
        }
    }

    0
}

/// `dcn10_register_irq_handlers`: VUPDATE_NO_LOCK of each CRTC, the one OTG
/// interrupt DCN delivers vblank and page-flip completion from, and HPD.
/// The OTG vertical line interrupt is CONFIG_DRM_AMD_SECURE_DISPLAY's.
fn dcn10_register_irq_handlers(adev: &mut amdgpu_device) -> i32 {
    use crate::amdgpu_dm_irq::*;

    // Use VUPDATE_NO_LOCK interrupt on DCN, which seems to correspond to the
    // regular VUPDATE interrupt on DCE. We want DC_IRQ_SOURCE_VUPDATEx to
    // trigger at end of each vblank, regardless of state of the lock,
    // matching DCE behaviour.
    let first = DCN_1_0__SRCID__OTG0_IHC_V_UPDATE_NO_LOCK_INTERRUPT;
    for i in first..first + adev.mode_info.num_crtc {
        let src = adev.vupdate_irq;
        let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_DCE, i, &src, "vupdate_irq");
        if r != 0 {
            drm_err("Failed to add vupdate irq id!");
            return r;
        }

        let irq_source = crate::dc::dc_interrupt_to_irq_source(adev.dm.dc.as_deref().unwrap(), i, 0);
        let int_params = dc_interrupt_params { int_context: dc_interrupt_context::INTERRUPT_HIGH_IRQ_CONTEXT, irq_source };

        if irq_source == DC_IRQ_SOURCE_INVALID || irq_source < DC_IRQ_SOURCE_VUPDATE1
            || irq_source > DC_IRQ_SOURCE_VUPDATE6
        {
            drm_err("Failed to register vupdate irq!");
            return -22;
        }

        let params = (irq_source - DC_IRQ_SOURCE_VUPDATE1) as usize;
        adev.dm.vupdate_params[params].irq_src = irq_source;

        if !amdgpu_dm_irq_register_interrupt(adev, &int_params, dm_irq_handler::dm_vupdate_high_irq { params }) {
            return -12;
        }
    }

    // HPD
    let src = adev.hpd_irq;
    let r = crate::amdgpu_irq::amdgpu_irq_add_id(adev, SOC15_IH_CLIENTID_DCE, DCN_1_0__SRCID__DC_HPD1_INT, &src,
                                                 "hpd_irq");
    if r != 0 {
        drm_err("Failed to add hpd irq id!");
        return r;
    }

    register_hpd_handlers(adev)
}

/// `register_hpd_handlers`: hot plug and the HPD RX (short pulse) handlers
/// of each connector join with external displays.
fn register_hpd_handlers(_adev: &mut amdgpu_device) -> i32 {
    Line::new().s("register_hpd_handlers: handle_hpd_irq, handle_hpd_rx_irq not ported").say();
    0
}

/// `manage_dm_interrupts`: a CRTC's vblank on, with the disable delay of
/// DCN before 3.5 on an APU without PSR (two frames), or off.
fn manage_dm_interrupts(adev: &mut amdgpu_device, crtc: usize, enable: bool) {
    if enable {
        let Some(stream) = adev.mode_info.crtcs[crtc].unwrap().dm_irq_params.stream else { return };
        let dc = adev.dm.dc.as_deref().unwrap();
        let s = dc.streams[stream].as_deref().unwrap();
        let timing = &s.timing;
        let mut config = crate::drm_vblank::drm_vblank_crtc_config::default();

        if dc.links[s.link].psr_settings.psr_version < DC_PSR_VERSION_UNSUPPORTED {
            let offdelay = (5u64 * 3 * 10 * timing.v_total as u64 * timing.h_total as u64)
                .div_ceil(timing.pix_clk_100hz as u64);
            config.offdelay_ms = if offdelay != 0 { offdelay as i32 } else { 30 };
        } else {
            // Older HW and DGPU have issues with instant off; use a 2 frame
            // offdelay.
            let offdelay = (20u64 * timing.v_total as u64 * timing.h_total as u64)
                .div_ceil(timing.pix_clk_100hz as u64);
            config.offdelay_ms = if offdelay != 0 { offdelay as i32 } else { 30 };
        }

        crate::drm_vblank::drm_crtc_vblank_on_config(adev, crtc, &config);
        // The extra vupdate reference is DCN 3.0 and 3.2's.
    } else {
        Line::new().s("manage_dm_interrupts: drm_crtc_vblank_off not ported").say();
    }
}

/// `amdgpu_dm_crtc_set_vblank` (amdgpu_dm_crtc.c): on DCN, VUPDATE_NO_LOCK
/// delivers vblank and page-flip completion, so it follows vblank. The
/// vblank control worker (PSR, idle optimizations) is not ported.
pub fn amdgpu_dm_crtc_set_vblank(adev: &mut amdgpu_device, crtc: usize, enable: bool) -> i32 {
    let acrtc = adev.mode_info.crtcs[crtc].unwrap();

    if enable && !acrtc.enabled {
        return -22;
    }

    let irq_type = amdgpu_display_crtc_idx_to_irq_type(adev, acrtc.crtc_id);

    // drm_crtc_vblank_restore is for IPS (DCN 3.5).

    // On DCN, VUPDATE_NO_LOCK is the single OTG interrupt used to deliver
    // vblank and pageflip completion events, so enable it whenever vblank is
    // enabled.
    let rc = if enable {
        crate::amdgpu_irq::amdgpu_irq_get(adev, "vupdate_irq", irq_type)
    } else {
        crate::amdgpu_irq::amdgpu_irq_put(adev, "vupdate_irq", irq_type)
    };
    if rc != 0 {
        return rc;
    }

    // VLINE0 is CONFIG_DRM_AMD_SECURE_DISPLAY's.

    if adev.in_gpu_reset {
        return 0;
    }

    0
}

/// `amdgpu_display_crtc_idx_to_irq_type` (amdgpu_display.c).
fn amdgpu_display_crtc_idx_to_irq_type(_adev: &amdgpu_device, crtc: u32) -> u32 {
    match crtc {
        0 => AMDGPU_CRTC_IRQ_VBLANK1,
        1 => AMDGPU_CRTC_IRQ_VBLANK2,
        2 => AMDGPU_CRTC_IRQ_VBLANK3,
        3 => AMDGPU_CRTC_IRQ_VBLANK4,
        4 => AMDGPU_CRTC_IRQ_VBLANK5,
        5 => AMDGPU_CRTC_IRQ_VBLANK6,
        _ => AMDGPU_CRTC_IRQ_NONE,
    }
}

/// `get_crtc_by_otg_inst`.
fn get_crtc_by_otg_inst(adev: &amdgpu_device, otg_inst: i32) -> Option<usize> {
    if otg_inst == -1 {
        Line::new().s("WARNING: get_crtc_by_otg_inst for otg_inst -1").say();
        return adev.mode_info.crtcs.first().and_then(|c| c.map(|_| 0));
    }

    adev.mode_info.crtcs.iter().position(|c| c.is_some_and(|c| c.otg_inst == otg_inst))
}

/// `dm_vupdate_high_irq`: on DCN the single OTG interrupt that delivers
/// vblank and page-flip completion.
pub fn dm_vupdate_high_irq(adev: &mut amdgpu_device, params: usize) {
    let irq_params = adev.dm.vupdate_params[params];
    let Some(acrtc) = get_crtc_by_otg_inst(adev, (irq_params.irq_src - IRQ_TYPE_VUPDATE) as i32) else { return };

    // The refresh rate trace of the frame duration is a tracepoint.
    adev.dm.vupdate_params[params].previous_timestamp = adev.mode_info.crtcs[acrtc].unwrap().vblank.time;

    // On DCN, VUPDATE_NO_LOCK is the single OTG interrupt used to deliver
    // vblank and pageflip completion events; VSTARTUP and GRPH_PFLIP are not
    // used. Run the full handler here.
    dm_crtc_high_irq_handler(adev, acrtc);
}

/// `dm_crtc_high_irq_handler`: the common OTG vblank and flip event
/// handling, DCN's path. No writeback connector, CRC capture or FreeSync
/// is set up.
fn dm_crtc_high_irq_handler(adev: &mut amdgpu_device, acrtc: usize) {
    // Core vblank handling. On DCN this handler runs at VUPDATE_NO_LOCK, the
    // register latch point, which is the correct place to timestamp both VRR
    // and non-VRR vblanks.
    crate::drm_vblank::drm_crtc_handle_vblank(adev, acrtc);

    // Deliver pageflip completion events (DCN only). Since GRPH_PFLIP is not
    // used, VUPDATE_NO_LOCK is the flip latch point. Deliver any pending
    // pageflip completion event from here, once HW has consumed the new
    // address (the OTG no longer reports a pending flip).
    let c = adev.mode_info.crtcs[acrtc].unwrap();
    if c.pflip_status == AMDGPU_FLIP_SUBMITTED && c.event.is_some() {
        let pending = crate::dc::dc_get_flip_pending_on_otg(adev, adev.dm.dc.as_deref().unwrap(), c.otg_inst as u32);
        if !pending {
            crate::drm_vblank::drm_crtc_send_vblank_event(adev, acrtc, c.event.unwrap());
            let c = adev.mode_info.crtcs[acrtc].as_mut().unwrap();
            c.event = None;
            crate::drm_vblank::drm_crtc_vblank_put(adev, acrtc);
            adev.mode_info.crtcs[acrtc].as_mut().unwrap().pflip_status = AMDGPU_FLIP_NONE;
        }
        // If the flip is still pending, leave it armed and retry on the next
        // vupdate.
    } else if c.pflip_status == AMDGPU_FLIP_SUBMITTED && c.dm_irq_params.active_planes == 0 {
        if let Some(e) = c.event {
            crate::drm_vblank::drm_crtc_send_vblank_event(adev, acrtc, e);
            adev.mode_info.crtcs[acrtc].as_mut().unwrap().event = None;
            crate::drm_vblank::drm_crtc_vblank_put(adev, acrtc);
        }
        adev.mode_info.crtcs[acrtc].as_mut().unwrap().pflip_status = AMDGPU_FLIP_NONE;
    }
}

/// What `amdgpu_dm_commit_planes` does for a page flip of the one plane DC
/// drives: the plane's unchanged plane and scaling info with the new
/// address, the vblank reference, the update, and the event armed after
/// programming (sent at once when the hardware latched already, else from
/// the VUPDATE interrupt). Flips are not throttled to the vblank of the
/// last one (amdgpu_display_get_crtc_scanoutpos is not ported): the caller
/// flips again only after the event. -EBUSY while a flip is pending.
pub fn dm_page_flip(adev: &mut amdgpu_device, address: u64, event: u32) -> i32 {
    use crate::dc;
    let Some(live) = adev.dm.live else { return -19 };
    let acrtc = live.crtc;
    let c = adev.mode_info.crtcs[acrtc].unwrap();
    if c.pflip_status != AMDGPU_FLIP_NONE || c.event.is_some() {
        return -16;
    }

    let (pi, si, mut fa) = {
        let pl = adev.dm.dc.as_deref().unwrap().planes[live.plane].as_deref().unwrap();
        // amdgpu_dm_plane_fill_dc_scaling_info and fill_dc_plane_info_and_addr
        // of an unchanged framebuffer: what the plane has.
        let pi = dc::dc_plane_info {
            plane_size: pl.plane_size, tiling_info: pl.tiling_info, dcc: pl.dcc, format: pl.format,
            rotation: pl.rotation, stereo_format: pl.stereo_format, color_space: pl.color_space,
            horizontal_mirror: pl.horizontal_mirror, visible: pl.visible, per_pixel_alpha: pl.per_pixel_alpha,
            pre_multiplied_alpha: pl.pre_multiplied_alpha, global_alpha: pl.global_alpha,
            global_alpha_value: pl.global_alpha_value, input_csc_enabled: pl.input_csc_color_matrix.enable_adjustment,
            layer_index: pl.layer_index,
        };
        let si = dc::dc_scaling_info { src_rect: pl.src_rect, dst_rect: pl.dst_rect, clip_rect: pl.clip_rect,
                                       scaling_quality: pl.scaling_quality };
        (pi, si, dc::dc_flip_addrs { address: pl.address, ..Default::default() })
    };
    fa.address.u[0] = address;
    fa.flip_immediate = false;
    fa.flip_timestamp_in_us = crate::linux::now_us();

    // dm_arm_vblank_event_pre_programming
    if c.dm_irq_params.active_planes != 0 {
        crate::drm_vblank::drm_crtc_vblank_get(adev, acrtc);
    }

    let mut dcb = adev.dm.dc.take().unwrap();
    let mut srf = [dc::dc_surface_update { surface: live.plane, flip_addr: Some(fa), plane_info: Some(pi),
                                           scaling_info: Some(si), ..Default::default() }];
    let mut su = dc::dc_stream_update { stream: live.stream, ..Default::default() };
    let updated = dc::dc_update_planes_and_stream(adev, &mut dcb, &mut srf, live.stream, Some(&mut su));
    adev.dm.dc = Some(dcb);

    // DCN: arm the event after programming, and deliver it at once if the
    // hardware latched during programming.
    let flip_latched_during_prog = updated
        && !dc::dc_get_flip_pending_on_otg(adev, adev.dm.dc.as_deref().unwrap(), c.otg_inst as u32);

    // dm_arm_vblank_event
    if c.dm_irq_params.active_planes != 0 {
        let a = adev.mode_info.crtcs[acrtc].as_mut().unwrap();
        a.pflip_status = AMDGPU_FLIP_SUBMITTED;
        a.event = Some(event);
    }

    let a = adev.mode_info.crtcs[acrtc].unwrap();
    if flip_latched_during_prog && a.pflip_status == AMDGPU_FLIP_SUBMITTED && a.event.is_some() {
        crate::drm_vblank::drm_crtc_send_vblank_event(adev, acrtc, event);
        adev.mode_info.crtcs[acrtc].as_mut().unwrap().event = None;
        crate::drm_vblank::drm_crtc_vblank_put(adev, acrtc);
        adev.mode_info.crtcs[acrtc].as_mut().unwrap().pflip_status = AMDGPU_FLIP_NONE;
    }

    if updated { 0 } else { -5 }
}

/// `amdgpu_dm_plane_get_cursor_position` (amdgpu_dm_plane.c): the position
/// of the cursor plane's top left corner on the CRTC; a corner left of or
/// above the CRTC moves into the hotspot.
fn amdgpu_dm_plane_get_cursor_position(dc: &crate::dc::dc, c: &dm_cursor_plane,
                                       position: &mut crate::dc_hw_types::dc_cursor_position) -> i32 {
    let max_cursor_width = dc.caps.max_cursor_size as i32;
    let max_cursor_height = dc.caps.max_cursor_size as i32;
    let (mut xorigin, mut yorigin) = (0, 0);

    if !c.visible {
        return 0;
    }

    if c.width as i32 > max_cursor_width || c.height as i32 > max_cursor_height {
        Line::new().s("amdgpu_dm_plane_get_cursor_position: bad cursor width or height ").dec(c.width as u64)
            .s(" x ").dec(c.height as u64).say();
        return -22;
    }

    let mut x = c.x - c.hot_x;
    let mut y = c.y - c.hot_y;

    if x <= -max_cursor_width || y <= -max_cursor_height {
        return 0;
    }

    if x < 0 {
        xorigin = (-x).min(max_cursor_width - 1);
        x = 0;
    }
    if y < 0 {
        yorigin = (-y).min(max_cursor_height - 1);
        y = 0;
    }
    position.enable = true;
    position.x = x as u32;
    position.y = y as u32;
    position.x_hotspot = xorigin as u32;
    position.y_hotspot = yorigin as u32;

    // DCE before 4.0.1
    position.translate_by_source = true;

    0
}

/// `amdgpu_dm_plane_handle_cursor_update` (amdgpu_dm_plane.c): the cursor
/// plane's state to DC, attributes and position, or the cursor off.
fn amdgpu_dm_plane_handle_cursor_update(adev: &mut amdgpu_device) {
    let (Some(c), Some(live)) = (adev.dm.cursor, adev.dm.live) else { return };
    let mut position = crate::dc_hw_types::dc_cursor_position::default();

    let mut dcb = adev.dm.dc.take().unwrap();
    let dc = &mut *dcb;
    let ret = amdgpu_dm_plane_get_cursor_position(dc, &c, &mut position);
    if ret == 0 {
        if !position.enable {
            // turn off cursor
            crate::dc::dc_stream_program_cursor_position(adev, dc, live.stream, &position);
        } else {
            let attributes = crate::dc_hw_types::dc_cursor_attributes {
                address: c.address,
                width: c.width,
                height: c.height,
                color_format: CURSOR_MODE_COLOR_PRE_MULTIPLIED_ALPHA,
                rotation_angle: 0,
                attribute_flags: 0,
                pitch: c.pitch,
                sdr_white_level: 0,
            };

            if !crate::dc::dc_stream_program_cursor_attributes(adev, dc, live.stream, &attributes) {
                drm_err("DC failed to set cursor attributes");
            }

            if !crate::dc::dc_stream_program_cursor_position(adev, dc, live.stream, &position) {
                drm_err("DC failed to set cursor position");
            }
        }
    }
    adev.dm.dc = Some(dcb);
}

/// The kernel's cursor image, premultiplied ARGB8888 `width` x `height`
/// with its hotspot, into the cursor plane's framebuffer. The buffer is
/// the largest cursor DCN 2.1 shows (`caps.max_cursor_size`), allocated
/// once in VRAM, where amdgpu_dm places cursor framebuffers.
pub fn dm_cursor_set_image(adev: &mut amdgpu_device, pixels: &[u8], width: u32, height: u32, hot_x: i32,
                           hot_y: i32) -> i32 {
    let Some(dc) = adev.dm.dc.as_deref() else { return -19 };
    let max = dc.caps.max_cursor_size;
    if width == 0 || height == 0 || width > max || height > max || pixels.len() != (width * height * 4) as usize {
        return -22;
    }
    let pitch = match width {
        0..=64 => 64,
        65..=128 => 128,
        _ => 256,
    };

    if adev.dm.cursor.is_none() {
        let size = (max * max * 4) as u64;
        let h = match crate::amdgpu_object::amdgpu_bo_create_kernel(adev, size, crate::amdgpu_object::PAGE_SIZE,
                                                                    AMDGPU_GEM_DOMAIN_VRAM, true) {
            Ok(Some(h)) => h,
            Ok(None) => return -12,
            Err(r) => return r,
        };
        let Some(kptr) = crate::amdgpu_object::bo(adev, h).kptr else { return -12 };
        let address = crate::amdgpu_object::amdgpu_bo_gpu_offset(adev, h);
        adev.dm.cursor = Some(dm_cursor_plane { bo: h, address, kptr, width: 0, height: 0, pitch: 0, hot_x: 0,
                                                hot_y: 0, x: 0, y: 0, visible: false });
    }
    let c = adev.dm.cursor.as_mut().unwrap();
    let (kptr, row_bytes) = (c.kptr, (pitch * 4) as usize);
    c.width = width;
    c.height = height;
    c.pitch = pitch;
    c.hot_x = hot_x;
    c.hot_y = hot_y;

    let mut row = alloc::vec![0u8; row_bytes];
    for y in 0..height as usize {
        let src = &pixels[y * width as usize * 4..(y + 1) * width as usize * 4];
        row[..src.len()].copy_from_slice(src);
        row[src.len()..].fill(0);
        crate::amdgpu_gart::kptr_write(adev, kptr, (y * row_bytes) as u64, &row);
    }

    amdgpu_dm_plane_handle_cursor_update(adev);
    0
}

/// The pointer moved to `x`, `y` on the CRTC, or the cursor shows or hides.
pub fn dm_cursor_move(adev: &mut amdgpu_device, x: i32, y: i32, visible: bool) -> i32 {
    let Some(c) = adev.dm.cursor.as_mut() else { return -19 };
    c.x = x;
    c.y = y;
    c.visible = visible;
    amdgpu_dm_plane_handle_cursor_update(adev);
    0
}
