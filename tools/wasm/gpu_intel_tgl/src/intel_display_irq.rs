//! display/intel_display_irq.c (Linux 6.18.55): the display engine's
//! interrupt registers and handler, display version 12 with an ICP-class
//! PCH; with the hotplug decoding of display/intel_hotplug_irq.c, the FIFO
//! underrun report of display/intel_fifo_underrun.c and the vblank
//! reference counting of drm_vblank.c the vblank wait needs.
//!
//! The DMC wakelock (`intel_dmc_wl_get/put`) is display version 20 and
//! later. DSI command mode (TE interrupts, `gen11_dsi_configure_te`) is
//! not ported: there is no DSI encoder, and the TE bits are only unmasked
//! when the VBT lists a DSI port.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::{intel_de_read, intel_de_rmw, intel_de_write};
use crate::intel_display_power::{intel_display_power_is_enabled, POWER_DOMAIN_PIPE, POWER_DOMAIN_TRANSCODER};
use crate::intel_display_types::plane_kind;
use crate::linux::Line;
use crate::regs::*;
use crate::tables;

const NUM_PIPES: usize = I915_MAX_PIPES as usize;

/// `display->irq`, `display->hotplug` (the pending pins) and the drm
/// vblank state per pipe.
#[derive(Clone, Copy, Default)]
pub struct intel_display_irq_state {
    /// `display->irq.de_irq_mask[pipe]`.
    pub de_irq_mask: [u32; NUM_PIPES],
    /// `drm_vblank_crtc.count`: vblank interrupts handled per pipe.
    pub vblank_count: [u32; NUM_PIPES],
    /// `drm_vblank_crtc.enabled`: set by `drm_crtc_vblank_on`.
    pub vblank_on: u8,
    /// `drm_vblank_crtc.refcount`.
    pub vblank_refcount: [u8; NUM_PIPES],
    /// `drm_vblank_crtc.enabled` as far as the interrupt goes.
    pub vblank_irq_enabled: u8,
    /// `crtc->cpu_fifo_underrun_disabled`, per pipe.
    pub cpu_fifo_underrun_disabled: u8,
    /// `intel_hpd_irq_handler`: pins that saw a hotplug pulse and those
    /// that were long ones. Hotplug processing (storm detection, the
    /// hotplug work) is not ported; the pins wait here.
    pub hpd_pin_mask: u32,
    pub hpd_long_mask: u32,
    /// `intel_dp_aux_irq_handler` / `intel_gmbus_irq_handler`: wake-ups
    /// for AUX and GMBUS transfers, which poll in this port.
    pub aux_done: bool,
    pub gmbus_done: bool,
}

/// `HAS_DSB`.
const HAS_DSB: bool = TGL_DISPLAY_HAS_DSB != 0;

/// `HAS_PIPEDMC`.
fn has_pipedmc(i915: &drm_i915_private) -> bool {
    i915.display.ip.ver >= 12
}

/// `intel_display_irq_regs_init`.
fn intel_display_irq_regs_init(i915: &mut drm_i915_private, imr: u32, ier: u32, iir: u32, imr_val: u32, ier_val: u32) {
    crate::i915_irq::gen2_irq_init(&mut i915.uncore, imr, ier, iir, imr_val, ier_val);
}

/// `intel_display_irq_regs_reset`.
fn intel_display_irq_regs_reset(i915: &mut drm_i915_private, imr: u32, ier: u32, iir: u32) {
    crate::i915_irq::gen2_irq_reset(&mut i915.uncore, imr, ier, iir);
}

/// `intel_display_irq_regs_assert_irr_is_zero`.
fn intel_display_irq_regs_assert_irr_is_zero(i915: &mut drm_i915_private, reg: u32) {
    crate::i915_irq::gen2_assert_iir_is_zero(&mut i915.uncore, reg);
}

/// The set bits of `mask`, lowest first.
fn bits(mask: u32) -> impl Iterator<Item = u32> {
    (0..32).filter(move |b| mask & (1 << b) != 0)
}

/// `for_each_pipe`.
fn pipes(i915: &drm_i915_private) -> impl Iterator<Item = u32> + use<> {
    bits(i915.display.runtime.pipe_mask)
}

/// `for_each_cpu_transcoder_masked`.
fn transcoders(i915: &drm_i915_private, mask: u32) -> impl Iterator<Item = u32> + use<> {
    bits(i915.display.runtime.cpu_transcoder_mask & mask)
}

/// `intel_crtc_get_plane`.
fn intel_crtc_get_plane(i915: &drm_i915_private, pipe: u32, plane_id: u32) -> Option<usize> {
    (0..i915.display.num_planes).find(|&p| i915.display.planes[p].pipe == pipe && i915.display.planes[p].id == plane_id)
}

/// `skl_plane_capture_error` / `g4x_cursor_capture_error`: (ctl, surf,
/// surflive).
fn capture_error(i915: &mut drm_i915_private, pipe: u32, p: usize) -> (u32, u32, u32) {
    let plane = i915.display.planes[p];
    match plane.kind {
        plane_kind::skl_universal => (
            intel_de_read(i915, PLANE_CTL(pipe, plane.id)),
            intel_de_read(i915, PLANE_SURF(pipe, plane.id)),
            intel_de_read(i915, PLANE_SURFLIVE(pipe, plane.id)),
        ),
        plane_kind::i9xx_cursor => (
            intel_de_read(i915, CURCNTR(pipe)),
            intel_de_read(i915, CURBASE(pipe)),
            intel_de_read(i915, CURSURFLIVE(pipe)),
        ),
    }
}

/// `handle_plane_fault`.
fn handle_plane_fault(i915: &mut drm_i915_private, pipe: u32, plane_id: u32) -> bool {
    let Some(p) = intel_crtc_get_plane(i915, pipe, plane_id) else {
        return false;
    };
    let (ctl, surf, surflive) = capture_error(i915, pipe, p);
    Line::new().s("error: [CRTC pipe ").dec(pipe as u64).s("][PLANE ").dec(plane_id as u64)
        .s("] fault (CTL=").hex(ctl).s(", SURF=").hex(surf).s(", SURFLIVE=").hex(surflive).s(")").say();
    true
}

/// `handle_pipedmc_fault`.
fn handle_pipedmc_fault(_i915: &mut drm_i915_private, pipe: u32, _plane_id: u32) -> bool {
    Line::new().s("error: [CRTC pipe ").dec(pipe as u64).s("] PIPEDMC fault").say();
    true
}

/// `struct pipe_fault_handler`.
struct pipe_fault_handler {
    fault: u32,
    handle: fn(&mut drm_i915_private, u32, u32) -> bool,
    plane_id: u32,
}

/// `tgl_pipe_fault_handlers`.
const tgl_pipe_fault_handlers: [pipe_fault_handler; 9] = [
    pipe_fault_handler { fault: GEN12_PIPEDMC_FAULT, handle: handle_pipedmc_fault, plane_id: 0 },
    pipe_fault_handler { fault: GEN11_PIPE_PLANE7_FAULT, handle: handle_plane_fault, plane_id: PLANE_7 },
    pipe_fault_handler { fault: GEN11_PIPE_PLANE6_FAULT, handle: handle_plane_fault, plane_id: PLANE_6 },
    pipe_fault_handler { fault: GEN11_PIPE_PLANE5_FAULT, handle: handle_plane_fault, plane_id: PLANE_5 },
    pipe_fault_handler { fault: GEN9_PIPE_PLANE4_FAULT, handle: handle_plane_fault, plane_id: PLANE_4 },
    pipe_fault_handler { fault: GEN9_PIPE_PLANE3_FAULT, handle: handle_plane_fault, plane_id: PLANE_3 },
    pipe_fault_handler { fault: GEN9_PIPE_PLANE2_FAULT, handle: handle_plane_fault, plane_id: PLANE_2 },
    pipe_fault_handler { fault: GEN9_PIPE_PLANE1_FAULT, handle: handle_plane_fault, plane_id: PLANE_1 },
    pipe_fault_handler { fault: GEN9_PIPE_CURSOR_FAULT, handle: handle_plane_fault, plane_id: PLANE_CURSOR },
];

/// `intel_pipe_fault_irq_handler`.
fn intel_pipe_fault_irq_handler(i915: &mut drm_i915_private, pipe: u32, mut fault_errors: u32) {
    for handler in tgl_pipe_fault_handlers.iter() {
        if fault_errors & handler.fault == 0 {
            continue;
        }
        if (handler.handle)(i915, pipe, handler.plane_id) {
            fault_errors &= !handler.fault;
        }
    }
    if fault_errors != 0 {
        Line::new().s("warning: [CRTC pipe ").dec(pipe as u64).s("] unreported faults ").hex(fault_errors).say();
    }
}

/// `intel_handle_vblank` → `drm_crtc_handle_vblank`: counts the vblank
/// while the pipe's vblank is on.
fn intel_handle_vblank(i915: &mut drm_i915_private, pipe: u32) {
    let st = &mut i915.irq.display;
    if st.vblank_on & (1 << pipe) == 0 {
        return;
    }
    st.vblank_count[pipe as usize] = st.vblank_count[pipe as usize].wrapping_add(1);
}

/// `bdw_update_pipe_irq`.
fn bdw_update_pipe_irq(i915: &mut drm_i915_private, pipe: u32, interrupt_mask: u32, enabled_irq_mask: u32) {
    if enabled_irq_mask & !interrupt_mask != 0 {
        crate::linux::say("warning: bdw_update_pipe_irq: enabled bits outside the mask");
    }
    if !i915.irqs_enabled {
        crate::linux::say("warning: bdw_update_pipe_irq with interrupts disabled");
        return;
    }
    let mut new_val = i915.irq.display.de_irq_mask[pipe as usize];
    new_val &= !interrupt_mask;
    new_val |= !enabled_irq_mask & interrupt_mask;

    if new_val != i915.irq.display.de_irq_mask[pipe as usize] {
        i915.irq.display.de_irq_mask[pipe as usize] = new_val;
        intel_de_write(i915, GEN8_DE_PIPE_IMR(pipe), new_val);
        let _ = intel_de_read(i915, GEN8_DE_PIPE_IMR(pipe));
    }
}

/// `bdw_enable_pipe_irq`.
pub fn bdw_enable_pipe_irq(i915: &mut drm_i915_private, pipe: u32, bits: u32) {
    bdw_update_pipe_irq(i915, pipe, bits, bits);
}

/// `bdw_disable_pipe_irq`.
pub fn bdw_disable_pipe_irq(i915: &mut drm_i915_private, pipe: u32, bits: u32) {
    bdw_update_pipe_irq(i915, pipe, bits, 0);
}

/// `bdw_enable_vblank`. `crtc->vblank_psr_notify` is only set for panel
/// replay, and `drm_crtc_vblank_restore` (HAS_PSR) only corrects a
/// hardware-counter based count, which this port does not keep.
fn bdw_enable_vblank(i915: &mut drm_i915_private, pipe: u32) {
    bdw_enable_pipe_irq(i915, pipe, GEN8_PIPE_VBLANK);
}

/// `bdw_disable_vblank`.
fn bdw_disable_vblank(i915: &mut drm_i915_private, pipe: u32) {
    bdw_disable_pipe_irq(i915, pipe, GEN8_PIPE_VBLANK);
}

/// `drm_crtc_vblank_on` (the part `intel_crtc_vblank_on` relies on): the
/// vblank becomes available, and its interrupt comes back on when there
/// are references.
pub fn drm_crtc_vblank_on(i915: &mut drm_i915_private, pipe: u32) {
    let st = &mut i915.irq.display;
    st.vblank_on |= 1 << pipe;
    if st.vblank_refcount[pipe as usize] != 0 && st.vblank_irq_enabled & (1 << pipe) == 0 {
        st.vblank_irq_enabled |= 1 << pipe;
        bdw_enable_vblank(i915, pipe);
    }
}

/// `drm_crtc_vblank_off`.
pub fn drm_crtc_vblank_off(i915: &mut drm_i915_private, pipe: u32) {
    let st = &mut i915.irq.display;
    let irq_on = st.vblank_irq_enabled & (1 << pipe) != 0;
    st.vblank_on &= !(1 << pipe);
    st.vblank_irq_enabled &= !(1 << pipe);
    if irq_on {
        bdw_disable_vblank(i915, pipe);
    }
}

/// `drm_vblank_get`.
fn drm_vblank_get(i915: &mut drm_i915_private, pipe: u32) -> Result<(), i32> {
    const EINVAL: i32 = 22;
    let st = &mut i915.irq.display;
    st.vblank_refcount[pipe as usize] += 1;
    if st.vblank_refcount[pipe as usize] == 1 {
        // drm_vblank_enable
        if st.vblank_on & (1 << pipe) == 0 {
            st.vblank_refcount[pipe as usize] -= 1;
            return Err(-EINVAL);
        }
        if st.vblank_irq_enabled & (1 << pipe) == 0 {
            st.vblank_irq_enabled |= 1 << pipe;
            bdw_enable_vblank(i915, pipe);
        }
    } else if st.vblank_on & (1 << pipe) == 0 {
        st.vblank_refcount[pipe as usize] -= 1;
        return Err(-EINVAL);
    }
    Ok(())
}

/// `drm_vblank_put`: `vblank_disable_immediate` is set
/// (`intel_display_irq_init`), so the last reference turns it off.
fn drm_vblank_put(i915: &mut drm_i915_private, pipe: u32) {
    let st = &mut i915.irq.display;
    if st.vblank_refcount[pipe as usize] == 0 {
        return;
    }
    st.vblank_refcount[pipe as usize] -= 1;
    if st.vblank_refcount[pipe as usize] == 0 && st.vblank_irq_enabled & (1 << pipe) != 0 {
        st.vblank_irq_enabled &= !(1 << pipe);
        bdw_disable_vblank(i915, pipe);
    }
}

/// `intel_crtc_wait_for_next_vblank` = `drm_crtc_wait_one_vblank`: one
/// vblank interrupt, at most 100 ms.
pub fn intel_crtc_wait_for_next_vblank(i915: &mut drm_i915_private, pipe: u32) {
    if let Err(ret) = drm_vblank_get(i915, pipe) {
        Line::new().s("warning: vblank not available on crtc ").dec(pipe as u64).s(", ret=-")
            .dec((-ret) as u64).say();
        return;
    }

    let last = i915.irq.display.vblank_count[pipe as usize];
    let t0 = crate::linux::now_us();
    loop {
        if i915.irq.display.vblank_count[pipe as usize] != last {
            break;
        }
        let elapsed_ms = (crate::linux::now_us() - t0) / 1000;
        if elapsed_ms >= 100 {
            Line::new().s("warning: vblank wait timed out on crtc ").dec(pipe as u64).say();
            break;
        }
        crate::i915_irq::intel_irq_wait(i915, (100 - elapsed_ms) as i32);
    }

    drm_vblank_put(i915, pipe);
}

/// `gen8_de_port_aux_mask`, display version 12.
fn gen8_de_port_aux_mask() -> u32 {
    TGL_DE_PORT_AUX_DDIA | TGL_DE_PORT_AUX_DDIB | TGL_DE_PORT_AUX_DDIC
        | TGL_DE_PORT_AUX_USBC1 | TGL_DE_PORT_AUX_USBC2 | TGL_DE_PORT_AUX_USBC3
        | TGL_DE_PORT_AUX_USBC4 | TGL_DE_PORT_AUX_USBC5 | TGL_DE_PORT_AUX_USBC6
}

/// `gen8_de_pipe_fault_mask`, display version 12 without
/// HAS_D12_PLANE_MINIMIZATION (Rocket Lake, Alder Lake-S).
fn gen8_de_pipe_fault_mask() -> u32 {
    GEN12_PIPEDMC_FAULT | GEN9_PIPE_CURSOR_FAULT | GEN11_PIPE_PLANE7_FAULT | GEN11_PIPE_PLANE6_FAULT
        | GEN11_PIPE_PLANE5_FAULT | GEN9_PIPE_PLANE4_FAULT | GEN9_PIPE_PLANE3_FAULT
        | GEN9_PIPE_PLANE2_FAULT | GEN9_PIPE_PLANE1_FAULT
}

/// `gen8_de_pipe_flip_done_mask`.
fn gen8_de_pipe_flip_done_mask() -> u32 {
    GEN9_PIPE_PLANE1_FLIP_DONE
}

/// `intel_bios_is_dsi_present`.
fn intel_bios_is_dsi_present(i915: &drm_i915_private) -> bool {
    let child = |c: &[u8], f: Bitfield| -> u32 {
        let mut v = 0u32;
        for i in 0..f.width.div_ceil(8) as usize {
            v |= (c[f.byte as usize + i] as u32) << (8 * i);
        }
        (v >> f.shift) & ((1u32 << f.width) - 1)
    };
    for devdata in i915.display.bios.devices() {
        let dvo_port = child(&devdata.child, CHILD_DVO_PORT);
        if child(&devdata.child, CHILD_DEVICE_TYPE) & DEVICE_TYPE_MIPI_OUTPUT == 0 {
            continue;
        }
        // dsi_dvo_port_to_port, display version 11 and later.
        if dvo_port != DVO_PORT_MIPIA && dvo_port != DVO_PORT_MIPIC {
            Line::new().s("VBT has unsupported DSI port ").dec((dvo_port - DVO_PORT_MIPIA) as u64).say();
            continue;
        }
        return true;
    }
    false
}

/// `gen8_de_misc_irq_handler`. `intel_opregion_asle_intr` is not ported;
/// an ASLE request is reported.
fn gen8_de_misc_irq_handler(i915: &mut drm_i915_private, iir: u32) {
    let mut found = false;

    if iir & GEN8_DE_MISC_GSE != 0 {
        crate::linux::say("GSE interrupt: intel_opregion_asle_intr not ported");
        found = true;
    }

    if iir & GEN8_DE_EDP_PSR != 0 {
        // for_each_intel_encoder_with_psr: no encoder here runs PSR.
    }

    if !found {
        Line::new().s("error: Unexpected DE Misc interrupt: ").hex(iir).say();
    }
    let _ = i915;
}

/// `gen11_port_hotplug_long_detect` / `icp_tc_port_hotplug_long_detect`.
fn tc_port_hotplug_long_detect(pin: u32, val: u32, long_detect: fn(u32) -> u32) -> bool {
    if (HPD_PORT_TC1..=HPD_PORT_TC6).contains(&pin) {
        val & long_detect(pin) != 0
    } else {
        false
    }
}

/// `icp_ddi_port_hotplug_long_detect`.
fn icp_ddi_port_hotplug_long_detect(pin: u32, val: u32) -> bool {
    if (HPD_PORT_A..=HPD_PORT_D).contains(&pin) {
        val & SHOTPLUG_CTL_DDI_HPD_LONG_DETECT(pin) != 0
    } else {
        false
    }
}

/// `intel_get_hpd_pins`.
fn intel_get_hpd_pins(pin_mask: &mut u32, long_mask: &mut u32, hotplug_trigger: u32, dig_hotplug_reg: u32,
                      hpd: &[u32], long_pulse_detect: &dyn Fn(u32, u32) -> bool) {
    for pin in 0..HPD_NUM_PINS {
        if hpd[pin as usize] & hotplug_trigger == 0 {
            continue;
        }
        *pin_mask |= 1 << pin;
        if long_pulse_detect(pin, dig_hotplug_reg) {
            *long_mask |= 1 << pin;
        }
    }
    Line::new().s("hotplug event received, stat ").hex(hotplug_trigger).s(", dig ").hex(dig_hotplug_reg)
        .s(", pins ").hex(*pin_mask).s(", long ").hex(*long_mask).say();
}

/// `intel_hpd_irq_handler`: the pins are kept for the hotplug side.
fn intel_hpd_irq_handler(i915: &mut drm_i915_private, pin_mask: u32, long_mask: u32) {
    i915.irq.display.hpd_pin_mask |= pin_mask;
    i915.irq.display.hpd_long_mask |= long_mask;
}

/// `gen11_hpd_irq_handler` (`display->hotplug.hpd` is `hpd_gen11`).
fn gen11_hpd_irq_handler(i915: &mut drm_i915_private, iir: u32) {
    let mut pin_mask = 0;
    let mut long_mask = 0;
    let trigger_tc = iir & GEN11_DE_TC_HOTPLUG_MASK;
    let trigger_tbt = iir & GEN11_DE_TBT_HOTPLUG_MASK;
    let detect = |pin, val| tc_port_hotplug_long_detect(pin, val, GEN11_HOTPLUG_CTL_LONG_DETECT);

    if trigger_tc != 0 {
        let dig_hotplug_reg = intel_de_rmw(i915, GEN11_TC_HOTPLUG_CTL, 0, 0);
        intel_get_hpd_pins(&mut pin_mask, &mut long_mask, trigger_tc, dig_hotplug_reg, &tables::hpd_gen11, &detect);
    }

    if trigger_tbt != 0 {
        let dig_hotplug_reg = intel_de_rmw(i915, GEN11_TBT_HOTPLUG_CTL, 0, 0);
        intel_get_hpd_pins(&mut pin_mask, &mut long_mask, trigger_tbt, dig_hotplug_reg, &tables::hpd_gen11, &detect);
    }

    if pin_mask != 0 {
        intel_hpd_irq_handler(i915, pin_mask, long_mask);
    } else {
        Line::new().s("error: Unexpected DE HPD interrupt ").hex(iir).say();
    }
}

/// `icp_irq_handler` (`display->hotplug.pch_hpd` is `hpd_icp`).
fn icp_irq_handler(i915: &mut drm_i915_private, pch_iir: u32) {
    let ddi_hotplug_trigger = pch_iir & SDE_DDI_HOTPLUG_MASK_ICP;
    let tc_hotplug_trigger = pch_iir & SDE_TC_HOTPLUG_MASK_ICP;
    let mut pin_mask = 0;
    let mut long_mask = 0;

    if ddi_hotplug_trigger != 0 {
        let dig_hotplug_reg = intel_de_rmw(i915, SHOTPLUG_CTL_DDI, 0, 0);
        intel_get_hpd_pins(&mut pin_mask, &mut long_mask, ddi_hotplug_trigger, dig_hotplug_reg,
                           &tables::hpd_icp, &icp_ddi_port_hotplug_long_detect);
    }

    if tc_hotplug_trigger != 0 {
        let dig_hotplug_reg = intel_de_rmw(i915, SHOTPLUG_CTL_TC, 0, 0);
        let detect = |pin, val| tc_port_hotplug_long_detect(pin, val, ICP_TC_HPD_LONG_DETECT);
        intel_get_hpd_pins(&mut pin_mask, &mut long_mask, tc_hotplug_trigger, dig_hotplug_reg,
                           &tables::hpd_icp, &detect);
    }

    if pin_mask != 0 {
        intel_hpd_irq_handler(i915, pin_mask, long_mask);
    }

    if pch_iir & SDE_GMBUS_ICP != 0 {
        i915.irq.display.gmbus_done = true;
    }
}

/// `gen8_read_and_ack_pch_irqs`. The PICA interrupt is a Meteor Lake PCH
/// source; on this PCH the bit is reported, not serviced.
fn gen8_read_and_ack_pch_irqs(i915: &mut drm_i915_private) -> u32 {
    let pch_iir = intel_de_read(i915, SDEIIR);
    if pch_iir == 0 {
        return 0;
    }
    if pch_iir & SDE_PICAINTERRUPT != 0 {
        crate::linux::say("warning: PICA interrupt on a PCH older than MTL");
    }
    intel_de_write(i915, SDEIIR, pch_iir);
    pch_iir
}

/// `intel_set_cpu_fifo_underrun_reporting` (display version 8 and later):
/// the old state.
fn intel_set_cpu_fifo_underrun_reporting(i915: &mut drm_i915_private, pipe: u32, enable: bool) -> bool {
    let old = i915.irq.display.cpu_fifo_underrun_disabled & (1 << pipe) == 0;
    if enable {
        i915.irq.display.cpu_fifo_underrun_disabled &= !(1 << pipe);
        bdw_enable_pipe_irq(i915, pipe, GEN8_PIPE_FIFO_UNDERRUN);
    } else {
        i915.irq.display.cpu_fifo_underrun_disabled |= 1 << pipe;
        bdw_disable_pipe_irq(i915, pipe, GEN8_PIPE_FIFO_UNDERRUN);
    }
    old
}

/// `intel_cpu_fifo_underrun_irq_handler`. `intel_fbc_handle_fifo_underrun_irq`
/// acts only on an FBC instance with a plane, and this port never
/// activates FBC.
fn intel_cpu_fifo_underrun_irq_handler(i915: &mut drm_i915_private, pipe: u32) {
    if crate::intel_display::intel_crtc_for_pipe(i915, pipe).is_none() {
        return;
    }
    if intel_set_cpu_fifo_underrun_reporting(i915, pipe, false) {
        Line::new().s("error: CPU pipe ").dec(pipe as u64).s(" FIFO underrun").say();
    }
}

/// `gen8_de_irq_handler`.
fn gen8_de_irq_handler(i915: &mut drm_i915_private, master_ctl: u32) {
    if master_ctl & GEN8_DE_MISC_IRQ != 0 {
        let iir = intel_de_read(i915, GEN8_DE_MISC_IIR);
        if iir != 0 {
            intel_de_write(i915, GEN8_DE_MISC_IIR, iir);
            gen8_de_misc_irq_handler(i915, iir);
        } else {
            crate::linux::say("error: The master control interrupt lied (DE MISC)!");
        }
    }

    if master_ctl & GEN11_DE_HPD_IRQ != 0 {
        let iir = intel_de_read(i915, GEN11_DE_HPD_IIR);
        if iir != 0 {
            intel_de_write(i915, GEN11_DE_HPD_IIR, iir);
            gen11_hpd_irq_handler(i915, iir);
        } else {
            crate::linux::say("error: The master control interrupt lied, (DE HPD)!");
        }
    }

    if master_ctl & GEN8_DE_PORT_IRQ != 0 {
        let iir = intel_de_read(i915, GEN8_DE_PORT_IIR);
        if iir != 0 {
            let mut found = false;
            intel_de_write(i915, GEN8_DE_PORT_IIR, iir);

            if iir & gen8_de_port_aux_mask() != 0 {
                i915.irq.display.aux_done = true;
                found = true;
            }

            let te_trigger = iir & (DSI0_TE | DSI1_TE);
            if te_trigger != 0 {
                Line::new().s("DSI TE interrupt ").hex(te_trigger).s(": gen11_dsi_te_interrupt_handler not ported").say();
                found = true;
            }

            if !found {
                crate::linux::say("error: Unexpected DE Port interrupt");
            }
        } else {
            crate::linux::say("error: The master control interrupt lied (DE PORT)!");
        }
    }

    for pipe in pipes(i915) {
        if master_ctl & GEN8_DE_PIPE_IRQ(pipe) == 0 {
            continue;
        }

        let iir = intel_de_read(i915, GEN8_DE_PIPE_IIR(pipe));
        if iir == 0 {
            Line::new().s("error: The master control interrupt lied (DE PIPE ").dec(pipe as u64).s(")!").say();
            continue;
        }

        intel_de_write(i915, GEN8_DE_PIPE_IIR(pipe), iir);

        if iir & GEN8_PIPE_VBLANK != 0 {
            intel_handle_vblank(i915, pipe);
        }

        // flip_done_handler: no commit here sends a flip-done event.

        if HAS_DSB {
            for dsb in [INTEL_DSB_0, INTEL_DSB_1, INTEL_DSB_2] {
                if iir & GEN12_DSB_INT(dsb) != 0 {
                    Line::new().s("pipe ").dec(pipe as u64).s(" DSB ").dec(dsb as u64)
                        .s(" interrupt: intel_dsb_irq_handler not ported").say();
                }
            }
        }

        if has_pipedmc(i915) && iir & GEN12_PIPEDMC_INTERRUPT != 0 {
            Line::new().s("pipe ").dec(pipe as u64).s(" PIPEDMC interrupt: intel_pipedmc_irq_handler not ported").say();
        }

        if iir & GEN8_PIPE_CDCLK_CRC_DONE != 0 {
            // hsw_pipe_crc_irq_handler; no CRC source is ever set up, so
            // display_pipe_crc_irq_handler has no consumer.
            let _ = intel_de_read(i915, PIPE_CRC_RES_HSW(pipe));
        }

        if iir & GEN8_PIPE_FIFO_UNDERRUN != 0 {
            intel_cpu_fifo_underrun_irq_handler(i915, pipe);
        }

        let fault_errors = iir & gen8_de_pipe_fault_mask();
        if fault_errors != 0 {
            intel_pipe_fault_irq_handler(i915, pipe, fault_errors);
        }
    }

    // HAS_PCH_SPLIT && !HAS_PCH_NOP; the PCH is ICP-class.
    if master_ctl & GEN8_DE_PCH_IRQ != 0 {
        let iir = gen8_read_and_ack_pch_irqs(i915);
        if iir != 0 {
            icp_irq_handler(i915, iir);
        }
    }
}

/// `gen11_gu_misc_irq_ack`.
pub fn gen11_gu_misc_irq_ack(i915: &mut drm_i915_private, master_ctl: u32) -> u32 {
    if master_ctl & GEN11_GU_MISC_IRQ == 0 {
        return 0;
    }
    let iir = intel_de_read(i915, GEN11_GU_MISC_IIR);
    if iir != 0 {
        intel_de_write(i915, GEN11_GU_MISC_IIR, iir);
    }
    iir
}

/// `gen11_gu_misc_irq_handler`: `intel_opregion_asle_intr` is not ported.
pub fn gen11_gu_misc_irq_handler(_i915: &mut drm_i915_private, iir: u32) {
    if iir & GEN11_GU_MISC_GSE != 0 {
        crate::linux::say("GSE interrupt: intel_opregion_asle_intr not ported");
    }
}

/// `gen11_display_irq_handler`.
pub fn gen11_display_irq_handler(i915: &mut drm_i915_private) {
    // GEN11_DISPLAY_INT_CTL has the display bits of GEN8_MASTER_IRQ.
    let disp_ctl = intel_de_read(i915, GEN11_DISPLAY_INT_CTL);

    intel_de_write(i915, GEN11_DISPLAY_INT_CTL, 0);
    gen8_de_irq_handler(i915, disp_ctl);
    intel_de_write(i915, GEN11_DISPLAY_INT_CTL, GEN11_DISPLAY_IRQ_ENABLE as u32);
}

/// `gen11_display_irq_reset`.
pub fn gen11_display_irq_reset(i915: &mut drm_i915_private) {
    let trans_mask = (1 << TRANSCODER_A) | (1 << TRANSCODER_B) | (1 << TRANSCODER_C) | (1 << TRANSCODER_D);

    if !i915.display.has_display {
        return;
    }

    intel_de_write(i915, GEN11_DISPLAY_INT_CTL, 0);

    for trans in transcoders(i915, trans_mask) {
        if !intel_display_power_is_enabled(i915, POWER_DOMAIN_TRANSCODER(trans)) {
            continue;
        }
        intel_de_write(i915, TRANS_PSR_IMR(trans), 0xffffffff);
        intel_de_write(i915, TRANS_PSR_IIR(trans), 0xffffffff);
    }

    for pipe in pipes(i915) {
        if intel_display_power_is_enabled(i915, POWER_DOMAIN_PIPE(pipe)) {
            intel_display_irq_regs_reset(i915, GEN8_DE_PIPE_IMR(pipe), GEN8_DE_PIPE_IER(pipe), GEN8_DE_PIPE_IIR(pipe));
        }
    }

    intel_display_irq_regs_reset(i915, GEN8_DE_PORT_IMR, GEN8_DE_PORT_IER, GEN8_DE_PORT_IIR);
    intel_display_irq_regs_reset(i915, GEN8_DE_MISC_IMR, GEN8_DE_MISC_IER, GEN8_DE_MISC_IIR);
    intel_display_irq_regs_reset(i915, GEN11_DE_HPD_IMR, GEN11_DE_HPD_IER, GEN11_DE_HPD_IIR);

    if i915.display.bios.pch_type >= PCH_ICP as i32 {
        intel_display_irq_regs_reset(i915, SDEIMR, SDEIER, SDEIIR);
    }
}

/// `icp_irq_postinstall`.
fn icp_irq_postinstall(i915: &mut drm_i915_private) {
    let mask = SDE_GMBUS_ICP;
    intel_display_irq_regs_init(i915, SDEIMR, SDEIER, SDEIIR, !mask, 0xffffffff);
}

/// `gen8_de_irq_postinstall`, display version 12 with an ICP-class PCH.
fn gen8_de_irq_postinstall(i915: &mut drm_i915_private) {
    let mut de_pipe_masked = gen8_de_pipe_fault_mask() | GEN8_PIPE_CDCLK_CRC_DONE;
    let mut de_port_masked = gen8_de_port_aux_mask();
    let de_misc_masked = GEN8_DE_EDP_PSR;
    let trans_mask = (1 << TRANSCODER_A) | (1 << TRANSCODER_B) | (1 << TRANSCODER_C) | (1 << TRANSCODER_D);

    if !i915.display.has_display {
        return;
    }

    if i915.display.bios.pch_type >= PCH_ICP as i32 {
        icp_irq_postinstall(i915);
    }

    if intel_bios_is_dsi_present(i915) {
        de_port_masked |= DSI0_TE | DSI1_TE;
    }

    if HAS_DSB {
        de_pipe_masked |= GEN12_DSB_INT(INTEL_DSB_0) | GEN12_DSB_INT(INTEL_DSB_1) | GEN12_DSB_INT(INTEL_DSB_2);
    }

    let de_pipe_enables = de_pipe_masked | GEN8_PIPE_VBLANK | GEN8_PIPE_FIFO_UNDERRUN | gen8_de_pipe_flip_done_mask();

    let de_port_enables = de_port_masked;

    for trans in transcoders(i915, trans_mask) {
        if !intel_display_power_is_enabled(i915, POWER_DOMAIN_TRANSCODER(trans)) {
            continue;
        }
        intel_display_irq_regs_assert_irr_is_zero(i915, TRANS_PSR_IIR(trans));
    }

    for pipe in pipes(i915) {
        i915.irq.display.de_irq_mask[pipe as usize] = !de_pipe_masked;

        if intel_display_power_is_enabled(i915, POWER_DOMAIN_PIPE(pipe)) {
            let imr = i915.irq.display.de_irq_mask[pipe as usize];
            intel_display_irq_regs_init(i915, GEN8_DE_PIPE_IMR(pipe), GEN8_DE_PIPE_IER(pipe),
                                        GEN8_DE_PIPE_IIR(pipe), imr, de_pipe_enables);
        }
    }

    intel_display_irq_regs_init(i915, GEN8_DE_PORT_IMR, GEN8_DE_PORT_IER, GEN8_DE_PORT_IIR,
                                !de_port_masked, de_port_enables);
    intel_display_irq_regs_init(i915, GEN8_DE_MISC_IMR, GEN8_DE_MISC_IER, GEN8_DE_MISC_IIR,
                                !de_misc_masked, de_misc_masked);

    // IS_DISPLAY_VER(display, 11, 13)
    let de_hpd_masked = 0u32;
    let de_hpd_enables = GEN11_DE_TC_HOTPLUG_MASK | GEN11_DE_TBT_HOTPLUG_MASK;
    intel_display_irq_regs_init(i915, GEN11_DE_HPD_IMR, GEN11_DE_HPD_IER, GEN11_DE_HPD_IIR,
                                !de_hpd_masked, de_hpd_enables);
}

/// `gen11_de_irq_postinstall`.
pub fn gen11_de_irq_postinstall(i915: &mut drm_i915_private) {
    if !i915.display.has_display {
        return;
    }
    gen8_de_irq_postinstall(i915);
    intel_de_write(i915, GEN11_DISPLAY_INT_CTL, GEN11_DISPLAY_IRQ_ENABLE as u32);
}

/// `gen8_irq_power_well_post_enable`: pipe interrupt registers come back
/// with their power well.
pub fn gen8_irq_power_well_post_enable(i915: &mut drm_i915_private, pipe_mask: u8) {
    let extra_ier = GEN8_PIPE_VBLANK | GEN8_PIPE_FIFO_UNDERRUN | gen8_de_pipe_flip_done_mask();
    if !i915.irqs_enabled {
        return;
    }
    for pipe in pipes(i915) {
        if pipe_mask & (1 << pipe) == 0 {
            continue;
        }
        let imr = i915.irq.display.de_irq_mask[pipe as usize];
        intel_display_irq_regs_init(i915, GEN8_DE_PIPE_IMR(pipe), GEN8_DE_PIPE_IER(pipe),
                                    GEN8_DE_PIPE_IIR(pipe), imr, !imr | extra_ier);
    }
}

/// `gen8_irq_power_well_pre_disable`. `intel_synchronize_irq` has nothing
/// to wait for: the handler only runs inside `intel_irq_wait`.
pub fn gen8_irq_power_well_pre_disable(i915: &mut drm_i915_private, pipe_mask: u8) {
    if !i915.irqs_enabled {
        return;
    }
    for pipe in pipes(i915) {
        if pipe_mask & (1 << pipe) == 0 {
            continue;
        }
        intel_display_irq_regs_reset(i915, GEN8_DE_PIPE_IMR(pipe), GEN8_DE_PIPE_IER(pipe), GEN8_DE_PIPE_IIR(pipe));
    }
}
