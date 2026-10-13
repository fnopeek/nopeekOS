//! display/intel_tc.c (Linux 6.18.55) for Tiger Lake: `tgl_tc_phy_ops`,
//! the ICL PHY handlers it shares, the TypeC mode state machine and the
//! port init with its initial mode readout.
//!
//! The port lock is a flag (the module is single-threaded), and the
//! delayed PHY-disconnect and link-reset works are not queued: nothing
//! runs between two calls of the driver, so the disconnect happens when
//! the next lock finds the link unreferenced, as the work would.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_display_power::{intel_display_power_get, intel_display_power_is_enabled,
                                 intel_display_power_legacy_aux_domain, intel_display_power_put,
                                 intel_display_power_tbt_aux_domain};
use crate::intel_display_types::intel_crtc_state;
use crate::intel_encoder::*;
use crate::linux::{self, Line};
use crate::regs::*;
use crate::tables;

fn tc_port_mode_name(mode: u32) -> &'static str {
    match mode {
        TC_PORT_DISCONNECTED => "disconnected",
        TC_PORT_TBT_ALT => "tbt-alt",
        TC_PORT_DP_ALT => "dp-alt",
        TC_PORT_LEGACY => "legacy",
        _ => "disconnected",
    }
}

fn tc(o: &mut intel_outputs, d: usize) -> &mut intel_tc_port {
    o.dig_ports[d].tc.as_mut().expect("digital port without TypeC state")
}

fn tc_ref(o: &intel_outputs, d: usize) -> &intel_tc_port {
    o.dig_ports[d].tc.as_ref().expect("digital port without TypeC state")
}

fn encoder(o: &intel_outputs, d: usize) -> &intel_encoder {
    &o.encoders[o.dig_ports[d].encoder]
}

/// `tc->port_name`: "%c/TC#%d".
fn port_name_line<'a>(l: &'a mut Line, o: &intel_outputs, d: usize) -> &'a mut Line {
    let e = encoder(o, d);
    let p = port_name(e.port);
    l.s("Port ").s(core::str::from_utf8(&p).unwrap_or("?")).s("/TC#")
        .dec((intel_encoder_to_tc(e) + 1) as u64)
}

fn pin_assignment_name(pin_assignment: u32) -> [u8; 1] {
    if pin_assignment == INTEL_TC_PIN_ASSIGNMENT_NONE {
        return [b'-'];
    }
    [b'A' + (pin_assignment - INTEL_TC_PIN_ASSIGNMENT_A) as u8]
}

fn dflexdpsp(fia: u32) -> u32 {
    [PORT_TX_DFLEXDPSP_FIA1, PORT_TX_DFLEXDPSP_FIA2, PORT_TX_DFLEXDPSP_FIA3][fia as usize]
}

fn dflexdppms(fia: u32) -> u32 {
    [PORT_TX_DFLEXDPPMS_FIA1, PORT_TX_DFLEXDPPMS_FIA2, PORT_TX_DFLEXDPPMS_FIA3][fia as usize]
}

fn dflexdpcsss(fia: u32) -> u32 {
    [PORT_TX_DFLEXDPCSSS_FIA1, PORT_TX_DFLEXDPCSSS_FIA2, PORT_TX_DFLEXDPCSSS_FIA3][fia as usize]
}

fn dflexpa1(fia: u32) -> u32 {
    [PORT_TX_DFLEXPA1_FIA1, PORT_TX_DFLEXPA1_FIA2, PORT_TX_DFLEXPA1_FIA3][fia as usize]
}

/// `intel_tc_port_in_mode`.
fn intel_tc_port_in_mode(o: &intel_outputs, d: usize, mode: u32) -> bool {
    intel_encoder_is_tc(encoder(o, d)) && tc_ref(o, d).mode == mode
}

pub fn intel_tc_port_in_tbt_alt_mode(o: &intel_outputs, d: usize) -> bool {
    intel_tc_port_in_mode(o, d, TC_PORT_TBT_ALT)
}

pub fn intel_tc_port_in_dp_alt_mode(o: &intel_outputs, d: usize) -> bool {
    intel_tc_port_in_mode(o, d, TC_PORT_DP_ALT)
}

pub fn intel_tc_port_in_legacy_mode(o: &intel_outputs, d: usize) -> bool {
    intel_tc_port_in_mode(o, d, TC_PORT_LEGACY)
}

/// `tc_phy_cold_off_domain` → `tgl_tc_phy_cold_off_domain`.
fn tc_phy_cold_off_domain() -> u32 {
    POWER_DOMAIN_TC_COLD_OFF
}

/// `intel_tc_cold_requires_aux_pw`.
pub fn intel_tc_cold_requires_aux_pw(o: &intel_outputs, d: usize) -> bool {
    tc_phy_cold_off_domain() == intel_display_power_legacy_aux_domain(o.dig_ports[d].aux_ch as u32)
}

/// `intel_aux_power_domain` (intel_display.c).
pub fn intel_aux_power_domain(o: &intel_outputs, d: usize) -> u32 {
    if intel_tc_port_in_tbt_alt_mode(o, d) {
        return intel_display_power_tbt_aux_domain(o.dig_ports[d].aux_ch as u32);
    }
    intel_display_power_legacy_aux_domain(o.dig_ports[d].aux_ch as u32)
}

/// `__tc_cold_block`.
fn __tc_cold_block(i915: &mut drm_i915_private) -> u32 {
    let domain = tc_phy_cold_off_domain();
    intel_display_power_get(i915, domain);
    domain
}

/// `tc_cold_block`.
fn tc_cold_block(i915: &mut drm_i915_private) -> bool {
    __tc_cold_block(i915);
    true
}

/// `__tc_cold_unblock`.
fn __tc_cold_unblock(i915: &mut drm_i915_private, domain: u32) {
    intel_display_power_put(i915, domain);
}

/// `tc_cold_unblock`.
fn tc_cold_unblock(i915: &mut drm_i915_private, wakeref: bool) {
    if wakeref {
        __tc_cold_unblock(i915, tc_phy_cold_off_domain());
    }
}

/// `assert_tc_cold_blocked`.
fn assert_tc_cold_blocked(i915: &drm_i915_private) {
    if !intel_display_power_is_enabled(i915, tc_phy_cold_off_domain()) {
        linux::say("warning: TC cold not blocked");
    }
}

/// `tc_port_power_domain`.
fn tc_port_power_domain(o: &intel_outputs, d: usize) -> u32 {
    let tc_port = intel_encoder_to_tc(encoder(o, d));
    if tc_port == TC_PORT_NONE {
        return POWER_DOMAIN_INVALID;
    }
    POWER_DOMAIN_PORT_DDI_LANES_TC1 + (tc_port as u32 - TC_PORT_1)
}

/// `assert_tc_port_power_enabled`.
fn assert_tc_port_power_enabled(i915: &drm_i915_private, o: &intel_outputs, d: usize) {
    if !intel_display_power_is_enabled(i915, tc_port_power_domain(o, d)) {
        linux::say("warning: TC port power not enabled");
    }
}

/// `get_lane_mask`.
fn get_lane_mask(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);

    intel_display_power_get(i915, POWER_DOMAIN_DISPLAY_CORE);
    let mut lane_mask = intel_de_read(i915, dflexdpsp(fia));
    intel_display_power_put(i915, POWER_DOMAIN_DISPLAY_CORE);

    if lane_mask == 0xffffffff {
        linux::say("warning: FIA lane assignment reads all ones");
    }
    assert_tc_cold_blocked(i915);

    lane_mask &= DP_LANE_ASSIGNMENT_MASK(idx);
    lane_mask >> DP_LANE_ASSIGNMENT_SHIFT(idx)
}

/// `get_pin_assignment` (display version below 20).
fn get_pin_assignment(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    if tc(o, d).mode == TC_PORT_TBT_ALT {
        return INTEL_TC_PIN_ASSIGNMENT_NONE;
    }
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);
    let reg = dflexpa1(fia);
    let mask = DP_PIN_ASSIGNMENT_MASK(idx);

    intel_display_power_get(i915, POWER_DOMAIN_DISPLAY_CORE);
    let val = intel_de_read(i915, reg);
    intel_display_power_put(i915, POWER_DOMAIN_DISPLAY_CORE);

    if val == 0xffffffff {
        linux::say("warning: FIA pin assignment reads all ones");
    }
    assert_tc_cold_blocked(i915);

    let pin_assignment = (val & mask) >> mask.trailing_zeros();

    match pin_assignment {
        // Assignments A, B and F exist up to display version 11 only.
        INTEL_TC_PIN_ASSIGNMENT_A | INTEL_TC_PIN_ASSIGNMENT_B | INTEL_TC_PIN_ASSIGNMENT_F => {
            linux::say("warning: TC pin assignment A, B or F on display version 12");
        }
        INTEL_TC_PIN_ASSIGNMENT_NONE | INTEL_TC_PIN_ASSIGNMENT_C | INTEL_TC_PIN_ASSIGNMENT_D
        | INTEL_TC_PIN_ASSIGNMENT_E => {}
        _ => {
            Line::new().s("MISSING_CASE pin assignment ").dec(pin_assignment as u64).say();
        }
    }
    pin_assignment
}

/// `icl_get_max_lane_count`.
fn icl_get_max_lane_count(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let lane_mask = get_lane_mask(i915, o, d);
    match lane_mask {
        0x1 | 0x2 | 0x4 | 0x8 => 1,
        0x3 | 0xc => 2,
        0xf => 4,
        _ => {
            Line::new().s("MISSING_CASE lane mask ").hex(lane_mask).say();
            1
        }
    }
}

/// `get_max_lane_count` (display version below 14).
fn get_max_lane_count(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    if tc(o, d).mode != TC_PORT_DP_ALT {
        return 4;
    }
    icl_get_max_lane_count(i915, o, d)
}

/// `read_pin_configuration`.
fn read_pin_configuration(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    let pin = get_pin_assignment(i915, o, d);
    tc(o, d).pin_assignment = pin;
    let lanes = get_max_lane_count(i915, o, d);
    tc(o, d).max_lane_count = lanes;
}

/// `intel_tc_port_max_lane_count`.
pub fn intel_tc_port_max_lane_count(o: &intel_outputs, d: usize) -> u32 {
    if !intel_encoder_is_tc(encoder(o, d)) {
        return 4;
    }
    tc_ref(o, d).max_lane_count
}

/// `intel_tc_port_get_pin_assignment`.
pub fn intel_tc_port_get_pin_assignment(o: &intel_outputs, d: usize) -> u32 {
    if !intel_encoder_is_tc(encoder(o, d)) {
        return INTEL_TC_PIN_ASSIGNMENT_NONE;
    }
    tc_ref(o, d).pin_assignment
}

/// `tc_port_fixup_legacy_flag`.
fn tc_port_fixup_legacy_flag(o: &mut intel_outputs, d: usize, live_status_mask: u32) {
    if tc(o, d).mode != TC_PORT_DISCONNECTED {
        linux::say("warning: legacy flag fixup on a connected TC port");
    }
    if live_status_mask.count_ones() != 1 {
        return;
    }
    let valid_hpd_mask = if tc(o, d).legacy_port {
        1 << TC_PORT_LEGACY
    } else {
        (1 << TC_PORT_DP_ALT) | (1 << TC_PORT_TBT_ALT)
    };
    if live_status_mask & !valid_hpd_mask == 0 {
        return;
    }

    // If live status mismatches the VBT flag, trust the live status.
    let mut l = Line::new();
    port_name_line(&mut l, o, d).s(": live status ").hex(live_status_mask)
        .s(" mismatches the legacy port flag ").hex(valid_hpd_mask).s(", fixing flag").say();
    let t = tc(o, d);
    t.legacy_port = !t.legacy_port;
}

/// `tc_phy_load_fia_params`: a modular FIA instance houses two TC ports.
fn tc_phy_load_fia_params(o: &mut intel_outputs, d: usize, modular_fia: bool) {
    let tc_port = intel_encoder_to_tc(encoder(o, d)) as u32;
    let t = tc(o, d);
    if modular_fia {
        t.phy_fia = tc_port / 2;
        t.phy_fia_idx = tc_port % 2;
    } else {
        t.phy_fia = FIA1;
        t.phy_fia_idx = tc_port;
    }
}

// ── ICL TC PHY handlers, shared by tgl_tc_phy_ops ─────────────────────

/// `icl_tc_phy_hpd_live_status`.
fn icl_tc_phy_hpd_live_status(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let isr_bit = tables::hpd_icp[encoder(o, d).hpd_pin as usize];
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);
    let mut mask = 0u32;

    let domain = tc_phy_cold_off_domain();
    intel_display_power_get(i915, domain);
    let fia_isr = intel_de_read(i915, dflexdpsp(fia));
    let pch_isr = intel_de_read(i915, SDEISR);
    intel_display_power_put(i915, domain);

    if fia_isr == 0xffffffff {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY in TCCOLD, nothing connected").say();
        return mask;
    }

    if fia_isr & TC_LIVE_STATE_TBT(idx) != 0 {
        mask |= 1 << TC_PORT_TBT_ALT;
    }
    if fia_isr & TC_LIVE_STATE_TC(idx) != 0 {
        mask |= 1 << TC_PORT_DP_ALT;
    }
    if pch_isr & isr_bit != 0 {
        mask |= 1 << TC_PORT_LEGACY;
    }
    mask
}

/// `icl_tc_phy_is_ready`: the IOM firmware sets the flag when a DP-alt or
/// legacy sink can be handed to display; it stays clear for TBT-alt.
fn icl_tc_phy_is_ready(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);
    assert_tc_cold_blocked(i915);

    let val = intel_de_read(i915, dflexdppms(fia));
    if val == 0xffffffff {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY in TCCOLD, assuming not ready").say();
        return false;
    }
    val & DP_PHY_MODE_STATUS_COMPLETED(idx) != 0
}

/// `icl_tc_phy_take_ownership`.
fn icl_tc_phy_take_ownership(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize, take: bool) -> bool {
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);
    assert_tc_cold_blocked(i915);

    let mut val = intel_de_read(i915, dflexdpcsss(fia));
    if val == 0xffffffff {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY in TCCOLD, can't ")
            .s(if take { "take" } else { "release" }).s(" ownership").say();
        return false;
    }

    val &= !DP_PHY_MODE_STATUS_NOT_SAFE(idx);
    if take {
        val |= DP_PHY_MODE_STATUS_NOT_SAFE(idx);
    }
    intel_de_write(i915, dflexdpcsss(fia), val);
    true
}

/// `icl_tc_phy_is_owned`.
fn icl_tc_phy_is_owned(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    let (fia, idx) = (tc(o, d).phy_fia, tc(o, d).phy_fia_idx);
    assert_tc_cold_blocked(i915);

    let val = intel_de_read(i915, dflexdpcsss(fia));
    if val == 0xffffffff {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY in TCCOLD, assume not owned").say();
        return false;
    }
    val & DP_PHY_MODE_STATUS_NOT_SAFE(idx) != 0
}

/// `icl_tc_phy_get_hw_state`.
fn icl_tc_phy_get_hw_state(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    let domain = __tc_cold_block(i915);

    let mode = tc_phy_get_current_mode(i915, o, d);
    tc(o, d).mode = mode;
    if mode != TC_PORT_DISCONNECTED {
        tc(o, d).lock_wakeref = tc_cold_block(i915);
        read_pin_configuration(i915, o, d);
    }

    __tc_cold_unblock(i915, domain);
}

/// `tc_phy_verify_legacy_or_dp_alt_mode`.
fn tc_phy_verify_legacy_or_dp_alt_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                                       required_lanes: u32) -> bool {
    let max_lanes = intel_tc_port_max_lane_count(o, d);
    if tc(o, d).mode == TC_PORT_LEGACY {
        if max_lanes != 4 {
            linux::say("warning: legacy TC port with fewer than 4 lanes");
        }
        return true;
    }

    if tc(o, d).mode != TC_PORT_DP_ALT {
        linux::say("warning: TC port neither legacy nor DP-alt");
    }

    // The port may have been disconnected meanwhile; legacy mode needs no
    // second look.
    if tc_phy_hpd_live_status(i915, o, d) & (1 << TC_PORT_DP_ALT) == 0 {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY sudden disconnect").say();
        return false;
    }

    if max_lanes < required_lanes {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY max lanes ").dec(max_lanes as u64)
            .s(" < required lanes ").dec(required_lanes as u64).say();
        return false;
    }
    true
}

/// `icl_tc_phy_connect`: the first part of the Connect Flow; the PHY is
/// shared with USB and TBT, so ownership is handed over through the FIA.
fn icl_tc_phy_connect(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize, required_lanes: u32) -> bool {
    tc(o, d).lock_wakeref = tc_cold_block(i915);

    if tc(o, d).mode == TC_PORT_TBT_ALT {
        read_pin_configuration(i915, o, d);
        return true;
    }

    let usable = icl_tc_phy_is_ready(i915, o, d) && icl_tc_phy_take_ownership(i915, o, d, true);
    if !usable {
        if tc(o, d).mode == TC_PORT_LEGACY {
            linux::say("warning: legacy TC port PHY not ready or not taken, proceeding");
        } else {
            let ready = icl_tc_phy_is_ready(i915, o, d);
            let mut l = Line::new();
            port_name_line(&mut l, o, d).s(": can't take PHY ownership (ready ")
                .s(if ready { "yes" } else { "no" }).s(")").say();
            let w = core::mem::take(&mut tc(o, d).lock_wakeref);
            tc_cold_unblock(i915, w);
            return false;
        }
    }

    read_pin_configuration(i915, o, d);

    if !tc_phy_verify_legacy_or_dp_alt_mode(i915, o, d, required_lanes) {
        icl_tc_phy_take_ownership(i915, o, d, false);
        let w = core::mem::take(&mut tc(o, d).lock_wakeref);
        tc_cold_unblock(i915, w);
        return false;
    }
    true
}

/// `icl_tc_phy_disconnect`: the Disconnect Flow.
fn icl_tc_phy_disconnect(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    match tc(o, d).mode {
        TC_PORT_LEGACY | TC_PORT_DP_ALT => {
            icl_tc_phy_take_ownership(i915, o, d, false);
            let w = core::mem::take(&mut tc(o, d).lock_wakeref);
            tc_cold_unblock(i915, w);
        }
        TC_PORT_TBT_ALT => {
            let w = core::mem::take(&mut tc(o, d).lock_wakeref);
            tc_cold_unblock(i915, w);
        }
        m => {
            Line::new().s("MISSING_CASE TC mode ").dec(m as u64).say();
        }
    }
}

/// `tgl_tc_phy_init`.
fn tgl_tc_phy_init(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    let domain = tc_phy_cold_off_domain();
    intel_display_power_get(i915, domain);
    let val = intel_de_read(i915, dflexdpsp(FIA1));
    intel_display_power_put(i915, domain);

    if val == 0xffffffff {
        linux::say("warning: FIA1 reads all ones at TC PHY init");
    }
    tc_phy_load_fia_params(o, d, val & MODULAR_FIA_MASK != 0);
}

// ── the PHY ops dispatch (tgl_tc_phy_ops) ─────────────────────────────

/// `tc_phy_hpd_live_status`: a sink is connected in a single mode only.
fn tc_phy_hpd_live_status(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let mask = icl_tc_phy_hpd_live_status(i915, o, d);
    if mask.count_ones() > 1 {
        linux::say("warning: TC sink live in more than one mode");
    }
    mask
}

fn tc_phy_is_ready(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    icl_tc_phy_is_ready(i915, o, d)
}

fn tc_phy_is_owned(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    icl_tc_phy_is_owned(i915, o, d)
}

fn tc_phy_get_hw_state(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    icl_tc_phy_get_hw_state(i915, o, d);
}

/// `tc_phy_owned_by_display` (display version below 20): legacy or DP-alt.
fn tc_phy_owned_by_display(phy_is_ready: bool, phy_is_owned: bool) -> bool {
    if phy_is_owned && !phy_is_ready {
        linux::say("warning: TC PHY owned but not ready");
    }
    phy_is_ready && phy_is_owned
}

/// `tc_phy_is_connected`.
pub fn tc_phy_is_connected(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize, port_pll_type: u32) -> bool {
    let phy_is_ready = tc_phy_is_ready(i915, o, d);
    let phy_is_owned = tc_phy_is_owned(i915, o, d);

    let is_connected = if tc_phy_owned_by_display(phy_is_ready, phy_is_owned) {
        port_pll_type == ICL_PORT_DPLL_MG_PHY
    } else {
        port_pll_type == ICL_PORT_DPLL_DEFAULT
    };

    let yn = |b: bool| if b { "yes" } else { "no" };
    let mut l = Line::new();
    port_name_line(&mut l, o, d).s(": PHY connected: ").s(yn(is_connected)).s(" (ready: ")
        .s(yn(phy_is_ready)).s(", owned: ").s(yn(phy_is_owned)).s(", pll_type: ")
        .s(if port_pll_type == ICL_PORT_DPLL_DEFAULT { "tbt" } else { "non-tbt" }).s(")").say();
    is_connected
}

/// `tc_phy_wait_for_ready`: poll_timeout_us(…, 1000, 500 * 1000).
fn tc_phy_wait_for_ready(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    let ret = linux::_wait_for(500 * 1000, 1000, 1000, || tc_phy_is_ready(i915, o, d));
    if ret.is_err() {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": timeout waiting for PHY ready").say();
        return false;
    }
    true
}

/// `hpd_mask_to_tc_mode`.
fn hpd_mask_to_tc_mode(live_status_mask: u32) -> u32 {
    if live_status_mask != 0 {
        return 31 - live_status_mask.leading_zeros();
    }
    TC_PORT_DISCONNECTED
}

/// `tc_phy_hpd_live_mode`.
fn tc_phy_hpd_live_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let live_status_mask = tc_phy_hpd_live_status(i915, o, d);
    hpd_mask_to_tc_mode(live_status_mask)
}

/// `get_tc_mode_in_phy_owned_state`.
fn get_tc_mode_in_phy_owned_state(t: &intel_tc_port, live_mode: u32) -> u32 {
    match live_mode {
        TC_PORT_LEGACY | TC_PORT_DP_ALT => live_mode,
        _ => {
            if live_mode != TC_PORT_TBT_ALT && live_mode != TC_PORT_DISCONNECTED {
                Line::new().s("MISSING_CASE live mode ").dec(live_mode as u64).say();
            }
            if t.legacy_port { TC_PORT_LEGACY } else { TC_PORT_DP_ALT }
        }
    }
}

/// `get_tc_mode_in_phy_not_owned_state`.
fn get_tc_mode_in_phy_not_owned_state(t: &intel_tc_port, live_mode: u32) -> u32 {
    match live_mode {
        TC_PORT_LEGACY => TC_PORT_DISCONNECTED,
        TC_PORT_DP_ALT | TC_PORT_TBT_ALT => TC_PORT_TBT_ALT,
        _ => {
            if live_mode != TC_PORT_DISCONNECTED {
                Line::new().s("MISSING_CASE live mode ").dec(live_mode as u64).say();
            }
            if t.legacy_port { TC_PORT_DISCONNECTED } else { TC_PORT_TBT_ALT }
        }
    }
}

/// `tc_phy_get_current_mode`.
fn tc_phy_get_current_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let live_mode = tc_phy_hpd_live_mode(i915, o, d);

    // The IOM firmware initializes a legacy port's PHY at boot and resume
    // whether or not a sink is connected; wait for it.
    if tc(o, d).legacy_port {
        tc_phy_wait_for_ready(i915, o, d);
    }

    let phy_is_ready = tc_phy_is_ready(i915, o, d);
    let phy_is_owned = tc_phy_is_owned(i915, o, d);

    let mode = if !tc_phy_owned_by_display(phy_is_ready, phy_is_owned) {
        get_tc_mode_in_phy_not_owned_state(tc_ref(o, d), live_mode)
    } else {
        if live_mode == TC_PORT_TBT_ALT {
            linux::say("warning: TBT-alt live on a PHY owned by display");
        }
        get_tc_mode_in_phy_owned_state(tc_ref(o, d), live_mode)
    };

    let yn = |b: bool| if b { "yes" } else { "no" };
    let mut l = Line::new();
    port_name_line(&mut l, o, d).s(": PHY mode: ").s(tc_port_mode_name(mode)).s(" (ready: ")
        .s(yn(phy_is_ready)).s(", owned: ").s(yn(phy_is_owned)).s(", HPD: ")
        .s(tc_port_mode_name(live_mode)).s(")").say();
    mode
}

/// `default_tc_mode`.
fn default_tc_mode(t: &intel_tc_port) -> u32 {
    if t.legacy_port {
        return TC_PORT_LEGACY;
    }
    TC_PORT_TBT_ALT
}

/// `hpd_mask_to_target_mode`.
fn hpd_mask_to_target_mode(t: &intel_tc_port, live_status_mask: u32) -> u32 {
    let mode = hpd_mask_to_tc_mode(live_status_mask);
    if mode != TC_PORT_DISCONNECTED {
        return mode;
    }
    default_tc_mode(t)
}

/// `tc_phy_get_target_mode`.
fn tc_phy_get_target_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> u32 {
    let live_status_mask = tc_phy_hpd_live_status(i915, o, d);
    hpd_mask_to_target_mode(tc_ref(o, d), live_status_mask)
}

/// `tc_phy_connect`.
fn tc_phy_connect(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize, required_lanes: u32) {
    let live_status_mask = tc_phy_hpd_live_status(i915, o, d);

    tc_port_fixup_legacy_flag(o, d, live_status_mask);

    let mode = hpd_mask_to_target_mode(tc_ref(o, d), live_status_mask);
    tc(o, d).mode = mode;

    let mut connected = icl_tc_phy_connect(i915, o, d, required_lanes);
    let default = default_tc_mode(tc_ref(o, d));
    if !connected && tc(o, d).mode != default {
        tc(o, d).mode = default;
        connected = icl_tc_phy_connect(i915, o, d, required_lanes);
    }

    if !connected {
        linux::say("warning: TC PHY connect failed in the default mode too");
    }
}

/// `tc_phy_disconnect`.
fn tc_phy_disconnect(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    if tc(o, d).mode != TC_PORT_DISCONNECTED {
        icl_tc_phy_disconnect(i915, o, d);
        tc(o, d).mode = TC_PORT_DISCONNECTED;
    }
}

/// `tc_phy_init`.
fn tc_phy_init(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    tgl_tc_phy_init(i915, o, d);
}

/// `intel_tc_port_reset_mode`. Nothing is put asynchronously, so there is
/// no power work to flush.
fn intel_tc_port_reset_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                            required_lanes: u32, force_disconnect: bool) {
    let old_tc_mode = tc(o, d).mode;

    if !intel_tc_cold_requires_aux_pw(o, d) {
        let aux_domain = intel_aux_power_domain(o, d);
        if intel_display_power_is_enabled(i915, aux_domain) {
            let mut l = Line::new();
            port_name_line(&mut l, o, d).s(": AUX unexpectedly powered").say();
        }
    }

    tc_phy_disconnect(i915, o, d);
    if !force_disconnect {
        tc_phy_connect(i915, o, d, required_lanes);
    }

    let t = *tc_ref(o, d);
    let pin = pin_assignment_name(t.pin_assignment);
    let mut l = Line::new();
    port_name_line(&mut l, o, d).s(": TC port mode reset (").s(tc_port_mode_name(old_tc_mode))
        .s(" -> ").s(tc_port_mode_name(t.mode)).s(") pin assignment: ")
        .s(core::str::from_utf8(&pin).unwrap_or("?")).s(" max lanes: ").dec(t.max_lane_count as u64).say();
}

/// `intel_tc_port_needs_reset`.
fn intel_tc_port_needs_reset(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    tc_phy_get_target_mode(i915, o, d) != tc(o, d).mode
}

/// `intel_tc_port_link_needs_reset` → `__intel_tc_port_link_needs_reset`.
pub fn intel_tc_port_link_needs_reset(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    if !intel_encoder_is_tc(&o.encoders[o.dig_ports[d].encoder]) {
        return false;
    }
    tc(o, d).locked = true;
    let ret = tc(o, d).link_refcount != 0 && tc(o, d).mode == TC_PORT_DP_ALT && intel_tc_port_needs_reset(i915, o, d);
    tc(o, d).locked = false;
    ret
}

/// `intel_tc_port_update_mode`.
fn intel_tc_port_update_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                             required_lanes: u32, force_disconnect: bool) {
    if force_disconnect || intel_tc_port_needs_reset(i915, o, d) {
        intel_tc_port_reset_mode(i915, o, d, required_lanes, force_disconnect);
    }
}

fn __intel_tc_port_get_link(o: &mut intel_outputs, d: usize) {
    tc(o, d).link_refcount += 1;
}

fn __intel_tc_port_put_link(o: &mut intel_outputs, d: usize) {
    tc(o, d).link_refcount -= 1;
}

/// `tc_port_is_enabled`.
fn tc_port_is_enabled(i915: &mut drm_i915_private, o: &intel_outputs, d: usize) -> bool {
    assert_tc_port_power_enabled(i915, o, d);
    intel_de_read(i915, DDI_BUF_CTL(encoder(o, d).port)) & DDI_BUF_CTL_ENABLE != 0
}

/// `intel_tc_port_init_mode`: read out the HW state and set the TypeC
/// mode, which then stays locked until `intel_tc_port_sanitize_mode`.
pub fn intel_tc_port_init_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    let mut update_mode = false;
    tc(o, d).locked = true;

    {
        let t = tc_ref(o, d);
        if t.mode != TC_PORT_DISCONNECTED || t.lock_wakeref || t.link_refcount != 0 {
            linux::say("warning: TC port init_mode on a port already in use");
        }
    }

    tc_phy_get_hw_state(i915, o, d);
    // Kept for the state check in intel_tc_port_sanitize_mode().
    let mode = tc(o, d).mode;
    tc(o, d).init_mode = mode;

    // AUX needs a connected PHY during readout, but the mode can only
    // change on a disabled port. The exception is a legacy port the BIOS
    // left disconnected while enabled: the IOM/TCSS firmware ignores the
    // ownership there, so connecting it is harmless.
    if !tc_port_is_enabled(i915, o, d) {
        update_mode = true;
    } else if mode == TC_PORT_DISCONNECTED {
        if !tc(o, d).legacy_port {
            linux::say("warning: non-legacy TC port enabled with the PHY disconnected");
        }
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY disconnected on enabled port, connecting it").say();
        update_mode = true;
    }

    if update_mode {
        intel_tc_port_update_mode(i915, o, d, 1, false);
    }

    // Prevent changing tc->mode until intel_tc_port_sanitize_mode() is called.
    __intel_tc_port_get_link(o, d);

    tc(o, d).locked = false;
}

/// `tc_port_has_active_streams`. MST readout is not done in Linux either.
fn tc_port_has_active_streams(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                              crtc_state: Option<&intel_crtc_state>) -> u32 {
    let mut pll_type = ICL_PORT_DPLL_DEFAULT;
    let mut active_streams = 0;

    if o.dig_ports[d].dp.is_mst {
        active_streams = 0;
    } else if let Some(cs) = crtc_state.filter(|cs| cs.hw.active) {
        pll_type = crate::intel_ddi::intel_ddi_port_pll_type(o, o.dig_ports[d].encoder, cs);
        active_streams = 1;
    }

    if active_streams != 0 && !tc_phy_is_connected(i915, o, d, pll_type) {
        let mut l = Line::new();
        port_name_line(&mut l, o, d).s(": PHY disconnected with ").dec(active_streams as u64)
            .s(" active stream(s)").say();
    }
    active_streams
}

/// `intel_tc_port_sanitize_mode`: an enabled encoder keeps the mode
/// locked until it is disabled; a disabled one gets its PHY disconnected.
/// `crtc_state` is the attached crtc's state when the port is enabled.
pub fn intel_tc_port_sanitize_mode(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                                   crtc_state: Option<&intel_crtc_state>) {
    tc(o, d).locked = true;

    if tc(o, d).link_refcount != 1 {
        linux::say("warning: TC port link refcount is not 1 at sanitize");
    }
    if tc_port_has_active_streams(i915, o, d, crtc_state) == 0 {
        // TBT-alt is the default whenever display does not own the PHY,
        // so leaving it says nothing.
        let init_mode = tc(o, d).init_mode;
        if init_mode != TC_PORT_TBT_ALT && init_mode != TC_PORT_DISCONNECTED {
            let mut l = Line::new();
            port_name_line(&mut l, o, d).s(": PHY left in ").s(tc_port_mode_name(init_mode))
                .s(" mode on disabled port, disconnecting it").say();
        }
        tc_phy_disconnect(i915, o, d);
        __intel_tc_port_put_link(o, d);
    }

    let t = *tc_ref(o, d);
    let pin = pin_assignment_name(t.pin_assignment);
    let mut l = Line::new();
    port_name_line(&mut l, o, d).s(": sanitize mode (").s(tc_port_mode_name(t.mode))
        .s(") pin assignment: ").s(core::str::from_utf8(&pin).unwrap_or("?"))
        .s(" max lanes: ").dec(t.max_lane_count as u64).say();

    tc(o, d).locked = false;
}

/// `intel_tc_port_ref_held`.
pub fn intel_tc_port_ref_held(o: &intel_outputs, d: usize) -> bool {
    let t = tc_ref(o, d);
    t.locked || t.link_refcount != 0
}

/// `intel_tc_port_connected`: connected and usable by display.
pub fn intel_tc_port_connected(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    if !intel_tc_port_ref_held(o, d) {
        linux::say("warning: TC port connected check without a reference");
    }
    let mut mask = !0u32;
    if tc(o, d).mode != TC_PORT_DISCONNECTED {
        mask = 1 << tc(o, d).mode;
    }
    tc_phy_hpd_live_status(i915, o, d) & mask != 0
}

/// `intel_tc_port_lock` (`__intel_tc_port_lock` with one lane).
pub fn intel_tc_port_lock(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    tc(o, d).locked = true;

    if tc(o, d).link_refcount == 0 {
        intel_tc_port_update_mode(i915, o, d, 1, false);
    }

    if tc(o, d).mode == TC_PORT_DISCONNECTED {
        linux::say("warning: TC port locked while disconnected");
    }
    if tc(o, d).mode != TC_PORT_TBT_ALT && !tc_phy_is_owned(i915, o, d) {
        linux::say("warning: TC port locked without PHY ownership");
    }
}

/// `intel_tc_port_unlock`. Linux queues the PHY disconnect a second later;
/// here the next lock does it if the link is still unreferenced.
pub fn intel_tc_port_unlock(o: &mut intel_outputs, d: usize) {
    tc(o, d).locked = false;
}

/// `intel_tc_port_init`.
pub fn intel_tc_port_init(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize, is_legacy: bool) -> Result<(), i32> {
    let tc_port = intel_encoder_to_tc(encoder(o, d));
    if tc_port == TC_PORT_NONE {
        linux::say("warning: TC port init on a port without TC");
        return Err(-(EINVAL as i32));
    }

    o.dig_ports[d].tc = Some(intel_tc_port {
        legacy_port: is_legacy,
        mode: TC_PORT_DISCONNECTED,
        link_refcount: 0,
        ..Default::default()
    });

    tc_phy_init(i915, o, d);

    intel_tc_port_init_mode(i915, o, d);
    Ok(())
}

/// The TypeC mode, for logging.
pub fn intel_tc_port_mode_name(o: &intel_outputs, d: usize) -> &'static str {
    tc_port_mode_name(tc_ref(o, d).mode)
}
