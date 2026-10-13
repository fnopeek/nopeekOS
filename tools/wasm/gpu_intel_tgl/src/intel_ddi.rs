//! display/intel_ddi.c `intel_ddi_init` and the encoder hooks the HW
//! readout calls, intel_display.c `intel_setup_outputs` and intel_hti.c
//! (Linux 6.18.55), for a Tiger Lake display (version 12, PCH TGP), and
//! the encoder configuration readout (`icl_ddi_combo_get_config`,
//! `icl_ddi_tc_get_config`, `intel_ddi_get_config`).

use crate::i915_drv::drm_i915_private;
use crate::intel_bios::{self, intel_vbt_data};
use crate::intel_de::*;
use crate::intel_display;
use crate::intel_display::REG_FIELD_GET;
use crate::intel_display_device::intel_display_runtime_info;
use crate::intel_display_power::{intel_display_power_ddi_io_domain, intel_display_power_ddi_lanes_domain,
                                 intel_display_power_get, intel_display_power_get_if_enabled,
                                 intel_display_power_is_enabled, intel_display_power_put,
                                 POWER_DOMAIN_TRANSCODER};
use crate::intel_dp;
use crate::intel_display_types::{icl_port_dpll, intel_crtc_state};
use crate::intel_dpll_mgr::{icl_tc_port_to_pll_id, intel_dpll_get_freq, intel_dpll_get_hw_state,
                            intel_get_dpll_by_id};
use crate::intel_encoder::*;
use crate::intel_hdmi;
use crate::intel_tc;
use crate::linux::{self, Line};
use crate::regs::*;

// ── intel_hti.c ───────────────────────────────────────────────────────

/// `intel_hti_init`: whether HTI reserved display resources before the
/// outputs are created. Tiger Lake has no HTI, so the state stays 0.
pub fn intel_hti_init(i915: &mut drm_i915_private) -> u32 {
    if TGL_DISPLAY_HAS_HTI != 0 {
        return intel_de_read(i915, HDPORT_STATE);
    }
    0
}

/// `intel_hti_uses_phy`.
fn intel_hti_uses_phy(hti_state: u32, phy: i32) -> bool {
    if phy == PHY_NONE {
        linux::say("warning: HTI check on PHY_NONE");
        return false;
    }
    hti_state & HDPORT_ENABLED != 0 && hti_state & HDPORT_DDI_USED(phy as u32) != 0
}

// ── intel_ddi.c init ─────────────────────────────────────────────────

/// `intel_ddi_max_lanes`: four from display version 11 on.
fn intel_ddi_max_lanes() -> i32 {
    4
}

/// `tgl_hpd_pin`.
fn tgl_hpd_pin(port: u32) -> u32 {
    if port >= PORT_TC1 {
        return HPD_PORT_TC1 + port - PORT_TC1;
    }
    HPD_PORT_A + port - PORT_A
}

/// `intel_ddi_is_tc`.
fn intel_ddi_is_tc(port: u32) -> bool {
    port >= PORT_TC1
}

/// `port_strap_detected`: straps are not used on SKL and later.
fn port_strap_detected(_port: u32) -> bool {
    true
}

/// `need_aux_ch`.
fn need_aux_ch(e: &intel_encoder, init_dp: bool) -> bool {
    init_dp || intel_encoder_is_tc(e)
}

/// `assert_port_valid`.
fn assert_port_valid(rt: &intel_display_runtime_info, port: u32) -> bool {
    if rt.port_mask & (1 << port) == 0 {
        let p = port_name(port);
        Line::new().s("warning: Platform does not support port ").s(core::str::from_utf8(&p).unwrap_or("?")).say();
        return false;
    }
    true
}

/// `port_in_use`.
fn port_in_use(o: &intel_outputs, port: u32) -> bool {
    o.encoders[..o.n_encoders].iter().any(|e| e.port == port)
}

/// `intel_ddi_encoder_name` (display version 12): "DDI %s%c/PHY %s%c".
fn intel_ddi_encoder_name(e: &mut intel_encoder, port: u32, phy: i32) {
    let tc_port = intel_port_to_tc(port);
    let port_part = if port >= PORT_TC1 { [b'1' + (port - PORT_TC1) as u8] } else { port_name(port) };
    let phy_part = if tc_port != TC_PORT_NONE {
        [b'1' + (tc_port - TC_PORT_1 as i32) as u8]
    } else {
        [b'A' + phy as u8]
    };
    e.set_name(&[
        "DDI ",
        if port >= PORT_TC1 { "TC" } else { "" },
        core::str::from_utf8(&port_part).unwrap_or("?"),
        "/PHY ",
        if tc_port != TC_PORT_NONE { "TC" } else { "" },
        core::str::from_utf8(&phy_part).unwrap_or("?"),
    ]);
}

/// `intel_ddi_init_dp_connector`.
fn intel_ddi_init_dp_connector(i915: &mut drm_i915_private, o: &mut intel_outputs,
                               vbt: &intel_bios::intel_bios_display, d: usize, pipe_mask: u32) -> Result<(), i32> {
    let port = o.encoders[o.dig_ports[d].encoder].port;
    o.dig_ports[d].dp.output_reg = DDI_BUF_CTL(port);
    if !intel_dp::intel_dp_init_connector(i915, o, vbt, d, pipe_mask) {
        return Err(-(EINVAL as i32));
    }
    Ok(())
}

/// `intel_ddi_init_hdmi_connector`: a failure (conflicting DDC pin, too
/// few lanes) leaves the port without HDMI but does not fail the DDI.
fn intel_ddi_init_hdmi_connector(o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display, d: usize) -> Result<(), i32> {
    let port = o.encoders[o.dig_ports[d].encoder].port;
    o.dig_ports[d].hdmi.hdmi_reg = DDI_BUF_CTL(port);
    if !intel_hdmi::intel_hdmi_init_connector(o, vbt, d) {
        o.dig_ports[d].hdmi.hdmi_reg = 0;
    }
    Ok(())
}

/// `intel_ddi_init`.
fn intel_ddi_init(i915: &mut drm_i915_private, o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display,
                  rt: &intel_display_runtime_info, hti_state: u32, devdata_idx: usize) {
    let devdata = vbt.devices()[devdata_idx];

    let port = intel_bios::intel_bios_encoder_port(vbt, &devdata);
    if port == PORT_NONE {
        return;
    }
    let port = port as u32;
    let pn = port_name(port);
    let pname = core::str::from_utf8(&pn).unwrap_or("?");

    if !port_strap_detected(port) {
        Line::new().s("Port ").s(pname).s(" strap not detected").say();
        return;
    }

    if !assert_port_valid(rt, port) {
        return;
    }

    if port_in_use(o, port) {
        Line::new().s("Port ").s(pname).s(" already claimed").say();
        return;
    }

    if intel_bios::intel_bios_encoder_supports_dsi(&devdata) {
        // Tiger Lake has ICL DSI; icl_dsi_init is not ported (no DSI here).
        Line::new().s("Port ").s(pname).s(": DSI in the VBT, icl_dsi_init is not ported").say();
        return;
    }

    let phy = intel_port_to_phy(port);

    // HTI (HDPORT) may have taken PHYs over at boot.
    if intel_hti_uses_phy(hti_state, phy) {
        Line::new().s("PORT ").s(pname).s(" reserved by HTI").say();
        return;
    }

    let mut init_hdmi = intel_bios::intel_bios_encoder_supports_dvi(&devdata)
        || intel_bios::intel_bios_encoder_supports_hdmi(&devdata);
    let mut init_dp = intel_bios::intel_bios_encoder_supports_dp(&devdata);

    if intel_bios::intel_bios_encoder_is_lspcon(vbt, &devdata) {
        // An LSPCON is driven as DP with its own detection sequence.
        init_dp = true;
        init_hdmi = false;
        Line::new().s("VBT says port ").s(pname).s(" has lspcon").say();
    }

    if !init_dp && !init_hdmi {
        Line::new().s("VBT says port ").s(pname).s(" is not DVI/HDMI/DP compatible, respect it").say();
        return;
    }

    // intel_dig_port_alloc
    if o.n_dig_ports == MAX_DIG_PORTS {
        linux::drm_err("digital port list is full");
        return;
    }
    let d = o.n_dig_ports;
    o.dig_ports[d] = intel_digital_port { aux_ch: AUX_CH_NONE, ..Default::default() };

    let mut e = intel_encoder {
        kind: encoder_kind::ddi(d),
        devdata: devdata_idx,
        type_: INTEL_OUTPUT_DDI,
        power_domain: intel_display_power_ddi_lanes_domain(port),
        port,
        cloneable: 0,
        pipe_mask: !0,
        ..Default::default()
    };
    intel_ddi_encoder_name(&mut e, port, phy);

    // Display version 12, not DG1/RKL/ADL-S/JSL/EHL.
    e.clock_ops = if intel_ddi_is_tc(port) { ddi_clock_ops::icl_ddi_tc } else { ddi_clock_ops::icl_ddi_combo };
    e.phy_ops = if intel_encoder_is_combo(&e) { ddi_phy_ops::tgl_combo } else { ddi_phy_ops::tgl_dkl };

    e.hpd_pin = tgl_hpd_pin(port);

    let Some(enc) = o.add_encoder(e) else {
        return;
    };
    o.dig_ports[d].encoder = enc;
    o.n_dig_ports += 1;

    let ddi_buf_ctl = intel_de_read(i915, DDI_BUF_CTL(port));

    o.dig_ports[d].lane_reversal =
        intel_bios::intel_bios_encoder_lane_reversal(&devdata) || ddi_buf_ctl & DDI_BUF_PORT_REVERSAL != 0;
    o.dig_ports[d].ddi_a_4_lanes = false;
    o.dig_ports[d].max_lanes = intel_ddi_max_lanes();

    if need_aux_ch(&o.encoders[enc], init_dp) {
        let aux_ch = intel_dp::intel_dp_aux_ch(o, vbt, enc);
        o.dig_ports[d].aux_ch = aux_ch;
        if aux_ch == AUX_CH_NONE {
            intel_ddi_init_err(o, d, enc);
            return;
        }
    }

    if intel_encoder_is_tc(&o.encoders[enc]) {
        let mut is_legacy = !intel_bios::intel_bios_encoder_supports_typec_usb(vbt, &devdata)
            && !intel_bios::intel_bios_encoder_supports_tbt(vbt, &devdata);

        if !is_legacy && init_hdmi {
            is_legacy = !init_dp;
            Line::new().s("VBT says port ").s(pname).s(" is non-legacy TC and has HDMI (with DP: ")
                .s(if init_dp { "yes" } else { "no" }).s("), assume it's ")
                .s(if is_legacy { "legacy" } else { "non-legacy" }).say();
        }

        if intel_tc::intel_tc_port_init(i915, o, d, is_legacy).is_err() {
            intel_ddi_init_err(o, d, enc);
            return;
        }
    }

    o.dig_ports[d].ddi_io_power_domain = intel_display_power_ddi_io_domain(port);

    o.dig_ports[d].connected = if intel_encoder_is_tc(&o.encoders[enc]) {
        connected_fn::intel_tc_port_connected
    } else {
        connected_fn::lpt_digital_port_connected
    };

    intel_hdmi::intel_infoframe_init(o, vbt, d);

    if init_dp {
        if intel_ddi_init_dp_connector(i915, o, vbt, d, rt.pipe_mask).is_err() {
            intel_ddi_init_err(o, d, enc);
            return;
        }
        if o.dig_ports[d].dp.mso_link_count != 0 {
            linux::say("eDP MSO splitter pipe mask is not ported");
        }
    }

    // The type check guards against a bad VBT.
    if o.encoders[enc].type_ != INTEL_OUTPUT_EDP && init_hdmi {
        let _ = intel_ddi_init_hdmi_connector(o, vbt, d);
    }
}

/// The `err:` path of `intel_ddi_init`: `drm_encoder_cleanup` + `kfree`.
/// The encoder and digital port are the last added, so they come off the
/// end of the lists; a TC port's state goes with them.
fn intel_ddi_init_err(o: &mut intel_outputs, d: usize, enc: usize) {
    if enc + 1 == o.n_encoders {
        o.n_encoders -= 1;
    }
    if d + 1 == o.n_dig_ports {
        o.n_dig_ports -= 1;
    }
}

/// `encoders_cloneable`: the masks may be asymmetric.
fn encoders_cloneable(o: &intel_outputs, a: usize, b: usize) -> bool {
    let (ea, eb) = (&o.encoders[a], &o.encoders[b]);
    a == b || (ea.cloneable & (1 << eb.type_) != 0 && eb.cloneable & (1 << ea.type_) != 0)
}

/// `intel_encoder_possible_clones`: `drm_encoder_mask` is the bit of the
/// encoder's index in the list.
fn intel_encoder_possible_clones(o: &intel_outputs, encoder: usize) -> u32 {
    let mut possible_clones = 0;
    for source in 0..o.n_encoders {
        if encoders_cloneable(o, encoder, source) {
            possible_clones |= 1 << source;
        }
    }
    possible_clones
}

/// `intel_encoder_possible_crtcs`: a crtc is created per pipe of
/// `pipe_mask`, in pipe order, so `drm_crtc_mask` is the bit of its rank.
fn intel_encoder_possible_crtcs(o: &intel_outputs, encoder: usize, pipe_mask: u32) -> u32 {
    let mut possible_crtcs = 0;
    let mut index = 0;
    for pipe in 0..I915_MAX_PIPES {
        if pipe_mask & (1 << pipe) == 0 {
            continue;
        }
        if o.encoders[encoder].pipe_mask & (1 << pipe) != 0 {
            possible_crtcs |= 1 << index;
        }
        index += 1;
    }
    possible_crtcs
}

/// `intel_setup_outputs` (HAS_DDI). `intel_pps_unlock_regs_wa` returns at
/// once on a DDI platform, `intel_ddi_crt_present` is false from display
/// version 9 on, and `intel_init_pch_refclk` has nothing to do for PCH TGP.
pub fn intel_setup_outputs(i915: &mut drm_i915_private, vbt: &intel_bios::intel_bios_display,
                           rt: &intel_display_runtime_info) {
    if !i915.display.has_display {
        return;
    }
    let hti_state = intel_hti_init(i915);

    let mut o = i915.display.outputs;

    // intel_bios_for_each_encoder(display, intel_ddi_init)
    for devdata in 0..vbt.devices().len() {
        intel_ddi_init(i915, &mut o, vbt, rt, hti_state, devdata);
    }

    for e in 0..o.n_encoders {
        o.encoders[e].possible_crtcs = intel_encoder_possible_crtcs(&o, e, rt.pipe_mask);
        o.encoders[e].possible_clones = intel_encoder_possible_clones(&o, e);
    }

    i915.display.outputs = o;
}

// ── encoder and connector hooks for the HW readout ───────────────────

/// `TRANS_DDI_FUNC_CTL(display, transcoder)` for transcoders A-D.
fn trans_ddi_func_ctl(transcoder: u32) -> u32 {
    [TRANS_DDI_FUNC_CTL_A, TRANS_DDI_FUNC_CTL_B, TRANS_DDI_FUNC_CTL_C, TRANS_DDI_FUNC_CTL_D][transcoder as usize]
}

/// `POWER_DOMAIN_TRANSCODER(transcoder)` for transcoders A-D.
fn power_domain_transcoder(transcoder: u32) -> u32 {
    [POWER_DOMAIN_TRANSCODER_A_, POWER_DOMAIN_TRANSCODER_B_, POWER_DOMAIN_TRANSCODER_C_,
     POWER_DOMAIN_TRANSCODER_D_][transcoder as usize]
}

/// `intel_ddi_get_encoder_pipes`: the pipes whose transcoder selects this
/// port, and whether it runs MST. There is no eDP transcoder on display
/// version 12, and no 128b/132b (HAS_DP20 is false).
pub fn intel_ddi_get_encoder_pipes(i915: &mut drm_i915_private, o: &intel_outputs, encoder: usize,
                                   pipes: u32) -> (u8, bool) {
    let e = &o.encoders[encoder];
    let port = e.port;
    let mut pipe_mask = 0u8;
    let mut is_dp_mst = false;
    let mut mst_pipe_mask = 0u8;

    if !intel_display_power_get_if_enabled(i915, e.power_domain) {
        return (pipe_mask, is_dp_mst);
    }

    let tmp = intel_de_read(i915, DDI_BUF_CTL(port));
    if tmp & DDI_BUF_CTL_ENABLE != 0 {
        for p in 0..I915_MAX_PIPES {
            if pipes & (1 << p) == 0 {
                continue;
            }
            let cpu_transcoder = p;
            if !intel_display_power_get_if_enabled(i915, power_domain_transcoder(cpu_transcoder)) {
                continue;
            }
            let port_mask = TGL_TRANS_DDI_PORT_MASK;
            let ddi_select = TGL_TRANS_DDI_SELECT_PORT(port);

            let tmp = intel_de_read(i915, trans_ddi_func_ctl(cpu_transcoder));
            intel_display_power_put(i915, power_domain_transcoder(cpu_transcoder));

            if tmp & port_mask != ddi_select {
                continue;
            }

            let ddi_mode = tmp & TRANS_DDI_MODE_SELECT_MASK;
            if ddi_mode == TRANS_DDI_MODE_SELECT_DP_MST {
                mst_pipe_mask |= 1 << p;
            }
            pipe_mask |= 1 << p;
        }

        if pipe_mask == 0 {
            Line::new().s("No pipe for [ENCODER:").s(e.name()).s("] found").say();
        }

        if mst_pipe_mask == 0 && pipe_mask.count_ones() > 1 {
            Line::new().s("Multiple pipes for [ENCODER:").s(e.name()).s("] (pipe_mask ").hex(pipe_mask as u32)
                .s(")").say();
            pipe_mask = 1 << pipe_mask.trailing_zeros();
        }

        if mst_pipe_mask != 0 && mst_pipe_mask != pipe_mask {
            Line::new().s("Conflicting MST and non-MST state for [ENCODER:").s(e.name()).s("] (pipe masks: all ")
                .hex(pipe_mask as u32).s(", MST ").hex(mst_pipe_mask as u32).s(", 128b/132b 0)").say();
        } else {
            is_dp_mst = mst_pipe_mask != 0;
        }
    }

    intel_display_power_put(i915, e.power_domain);
    (pipe_mask, is_dp_mst)
}

/// `intel_ddi_get_hw_state`: the pipe driving the port in SST mode.
pub fn intel_ddi_get_hw_state(i915: &mut drm_i915_private, o: &intel_outputs, encoder: usize,
                              pipes: u32) -> Option<u32> {
    let (pipe_mask, is_mst) = intel_ddi_get_encoder_pipes(i915, o, encoder, pipes);
    if is_mst || pipe_mask == 0 {
        return None;
    }
    Some(pipe_mask.trailing_zeros())
}

/// `mst_stream_get_hw_state`: a fake MST encoder is on when it has a
/// connector, and only an MST topology probe creates one, which nothing
/// here does.
pub fn mst_stream_get_hw_state(_o: &intel_outputs, _encoder: usize) -> Option<u32> {
    None
}

/// `encoder->get_hw_state` for either kind of encoder.
pub fn intel_encoder_get_hw_state(i915: &mut drm_i915_private, o: &intel_outputs, encoder: usize,
                                  pipes: u32) -> Option<u32> {
    match o.encoders[encoder].kind {
        encoder_kind::ddi(_) => intel_ddi_get_hw_state(i915, o, encoder, pipes),
        encoder_kind::dp_mst(_, _) => mst_stream_get_hw_state(o, encoder),
        encoder_kind::none => None,
    }
}

/// `intel_ddi_connector_get_hw_state`.
pub fn intel_ddi_connector_get_hw_state(i915: &mut drm_i915_private, o: &intel_outputs, connector: usize,
                                        pipes: u32) -> bool {
    let c = &o.connectors[connector];
    let encoder = c.encoder;
    let type_ = c.connector_type;
    let power_domain = o.encoders[encoder].power_domain;

    if !intel_display_power_get_if_enabled(i915, power_domain) {
        return false;
    }

    // This is false for DP MST primary encoders.
    let ret = match intel_encoder_get_hw_state(i915, o, encoder, pipes) {
        None => false,
        Some(pipe) => {
            let cpu_transcoder = pipe;
            let ddi_mode = intel_de_read(i915, trans_ddi_func_ctl(cpu_transcoder)) & TRANS_DDI_MODE_SELECT_MASK;
            if ddi_mode == TRANS_DDI_MODE_SELECT_HDMI || ddi_mode == TRANS_DDI_MODE_SELECT_DVI {
                type_ == DRM_MODE_CONNECTOR_HDMIA
            } else if ddi_mode == TRANS_DDI_MODE_SELECT_FDI_OR_128B132B {
                type_ == DRM_MODE_CONNECTOR_VGA
            } else if ddi_mode == TRANS_DDI_MODE_SELECT_DP_SST {
                type_ == DRM_MODE_CONNECTOR_eDP || type_ == DRM_MODE_CONNECTOR_DisplayPort
            } else {
                if ddi_mode == TRANS_DDI_MODE_SELECT_DP_MST {
                    linux::say("warning: MST transcoder behind an SST encoder");
                }
                false
            }
        }
    };

    intel_display_power_put(i915, power_domain);
    ret
}

/// `intel_ddi_main_link_aux_domain` (display version 12; PSR needs AUX IO
/// power on eDP only).
fn intel_ddi_main_link_aux_domain(o: &intel_outputs, d: usize, cs: &intel_crtc_state) -> u32 {
    let e = &o.encoders[o.dig_ports[d].encoder];
    let psr_aux_io = cs.has_type(INTEL_OUTPUT_EDP);
    if psr_aux_io {
        return crate::intel_display_power::intel_display_power_aux_io_domain(o.dig_ports[d].aux_ch as u32);
    } else if cs.has_dp_encoder() || intel_encoder_is_tc(e) {
        return intel_tc::intel_aux_power_domain(o, d);
    }
    POWER_DOMAIN_INVALID
}

/// `main_link_aux_power_domain_get`.
fn main_link_aux_power_domain_get(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize,
                                  cs: &intel_crtc_state) {
    let domain = intel_ddi_main_link_aux_domain(o, d, cs);
    if o.dig_ports[d].aux_wakeref.is_some() {
        linux::say("warning: AUX wakeref already held");
    }
    if domain == POWER_DOMAIN_INVALID {
        return;
    }
    intel_display_power_get(i915, domain);
    o.dig_ports[d].aux_wakeref = Some(domain);
}

/// `intel_ddi_get_power_domains`: fake MST encoders do not set the hook.
pub fn intel_ddi_get_power_domains(i915: &mut drm_i915_private, o: &mut intel_outputs, encoder: usize,
                                   cs: &intel_crtc_state) {
    if cs.has_type(INTEL_OUTPUT_DP_MST) {
        linux::say("warning: DDI power domains for an MST crtc state");
        return;
    }
    let Some(d) = o.dig_port_of(encoder) else {
        return;
    };

    if !intel_tc::intel_tc_port_in_tbt_alt_mode(o, d) {
        if o.dig_ports[d].ddi_io_wakeref {
            linux::say("warning: DDI IO wakeref already held");
        }
        intel_display_power_get(i915, o.dig_ports[d].ddi_io_power_domain);
        o.dig_ports[d].ddi_io_wakeref = true;
    }

    main_link_aux_power_domain_get(i915, o, d, cs);
}

/// `icl_ddi_tc_port_pll_type` / `intel_ddi_port_pll_type`.
pub fn intel_ddi_port_pll_type(o: &intel_outputs, encoder: usize, cs: &intel_crtc_state) -> u32 {
    if o.encoders[encoder].clock_ops != ddi_clock_ops::icl_ddi_tc {
        return ICL_PORT_DPLL_DEFAULT;
    }
    match cs.pll_is_tbt() {
        None => {
            linux::say("warning: TC port PLL type without a PLL");
            ICL_PORT_DPLL_DEFAULT
        }
        Some(true) => ICL_PORT_DPLL_DEFAULT,
        Some(false) => ICL_PORT_DPLL_MG_PHY,
    }
}

/// `intel_ddi_sync_state`.
pub fn intel_ddi_sync_state(i915: &mut drm_i915_private, o: &mut intel_outputs, vbt: &intel_bios::intel_bios_display,
                            encoder: usize, crtc_state: Option<&intel_crtc_state>) {
    if intel_encoder_is_tc(&o.encoders[encoder]) {
        if let Some(d) = o.dig_port_of(encoder) {
            intel_tc::intel_tc_port_sanitize_mode(i915, o, d, crtc_state);
        }
    }

    if crtc_state.is_some_and(|cs| cs.has_dp_encoder()) || (crtc_state.is_none() && intel_encoder_is_dp(o, encoder)) {
        intel_dp::intel_dp_sync_state(o, vbt, encoder, crtc_state);
    }
}

/// `icl_ddi_combo_is_clock_enabled` / `icl_ddi_tc_is_clock_enabled`.
fn intel_ddi_is_clock_enabled(i915: &mut drm_i915_private, e: &intel_encoder) -> bool {
    match e.clock_ops {
        ddi_clock_ops::icl_ddi_combo => {
            let phy = intel_encoder_to_phy(e) as u32;
            intel_de_read(i915, ICL_DPCLKA_CFGCR0) & ICL_DPCLKA_CFGCR0_DDI_CLK_OFF(phy) == 0
        }
        ddi_clock_ops::icl_ddi_tc => {
            let tmp = intel_de_read(i915, DDI_CLK_SEL(e.port));
            if tmp & DDI_CLK_SEL_MASK == DDI_CLK_SEL_NONE {
                return false;
            }
            let tmp = intel_de_read(i915, ICL_DPCLKA_CFGCR0);
            tmp & tc_clk_off(intel_encoder_to_tc(e)) == 0
        }
        ddi_clock_ops::none => false,
    }
}

/// `ICL_DPCLKA_CFGCR0_TC_CLK_OFF(tc_port)`.
fn tc_clk_off(tc_port: i32) -> u32 {
    [ICL_DPCLKA_CFGCR0_TC_CLK_OFF_0, ICL_DPCLKA_CFGCR0_TC_CLK_OFF_1, ICL_DPCLKA_CFGCR0_TC_CLK_OFF_2,
     ICL_DPCLKA_CFGCR0_TC_CLK_OFF_3, ICL_DPCLKA_CFGCR0_TC_CLK_OFF_4, ICL_DPCLKA_CFGCR0_TC_CLK_OFF_5][tc_port as usize]
}

/// `icl_ddi_combo_disable_clock` / `icl_ddi_tc_disable_clock`.
fn intel_ddi_disable_clock(i915: &mut drm_i915_private, e: &intel_encoder) {
    match e.clock_ops {
        ddi_clock_ops::icl_ddi_combo => {
            let phy = intel_encoder_to_phy(e) as u32;
            intel_de_rmw(i915, ICL_DPCLKA_CFGCR0, 0, ICL_DPCLKA_CFGCR0_DDI_CLK_OFF(phy));
        }
        ddi_clock_ops::icl_ddi_tc => {
            intel_de_rmw(i915, ICL_DPCLKA_CFGCR0, 0, tc_clk_off(intel_encoder_to_tc(e)));
            intel_de_write(i915, DDI_CLK_SEL(e.port), DDI_CLK_SEL_NONE);
        }
        ddi_clock_ops::none => {}
    }
}

/// `intel_ddi_sanitize_encoder_pll_mapping`: gate the DDI clock of an
/// encoder no crtc uses. `has_crtc` is `encoder->base.crtc != NULL`.
pub fn intel_ddi_sanitize_encoder_pll_mapping(i915: &mut drm_i915_private, o: &intel_outputs, encoder: usize,
                                              has_crtc: bool, pipes: u32) {
    let e = o.encoders[encoder];

    // For DP MST only the primary encoder is sanitized.
    if e.type_ == INTEL_OUTPUT_DP_MST {
        return;
    }

    if !has_crtc && intel_encoder_is_dp(o, encoder) {
        let (_pipe_mask, is_mst) = intel_ddi_get_encoder_pipes(i915, o, encoder, pipes);
        // A BIOS-enabled MST output: the MST readout is incomplete.
        if is_mst {
            linux::say("warning: BIOS left DP in MST mode");
            return;
        }
    }

    let ddi_clk_needed = has_crtc;

    if ddi_clk_needed || e.clock_ops == ddi_clock_ops::none || !intel_ddi_is_clock_enabled(i915, &e) {
        return;
    }

    Line::new().s("[ENCODER:").s(e.name()).s("] is disabled/in DSI mode with an ungated DDI clock, gate it").say();
    intel_ddi_disable_clock(i915, &e);
}

// ── encoder configuration readout (`encoder->get_config`) ────────────

/// `_icl_ddi_get_pll` → `icl_ddi_combo_get_pll`.
fn icl_ddi_combo_get_pll(i915: &mut drm_i915_private, e: &intel_encoder) -> Option<usize> {
    let phy = intel_encoder_to_phy(e) as u32;
    let id = (intel_de_read(i915, ICL_DPCLKA_CFGCR0) & ICL_DPCLKA_CFGCR0_DDI_CLK_SEL_MASK(phy))
        >> ICL_DPCLKA_CFGCR0_DDI_CLK_SEL_SHIFT(phy);
    intel_get_dpll_by_id(i915, id)
}

/// `icl_ddi_tc_get_pll`.
fn icl_ddi_tc_get_pll(i915: &mut drm_i915_private, e: &intel_encoder) -> Option<usize> {
    let tc_port = intel_encoder_to_tc(e);
    let tmp = intel_de_read(i915, DDI_CLK_SEL(e.port));

    let id = match tmp & DDI_CLK_SEL_MASK {
        DDI_CLK_SEL_TBT_162 | DDI_CLK_SEL_TBT_270 | DDI_CLK_SEL_TBT_540 | DDI_CLK_SEL_TBT_810 => DPLL_ID_ICL_TBTPLL,
        DDI_CLK_SEL_MG => icl_tc_port_to_pll_id(tc_port as u32),
        DDI_CLK_SEL_NONE => return None,
        _ => {
            Line::new().s("MISSING_CASE DDI_CLK_SEL ").hex(tmp).say();
            return None;
        }
    };
    intel_get_dpll_by_id(i915, id)
}

/// `icl_calc_tbt_pll_link`.
fn icl_calc_tbt_pll_link(i915: &mut drm_i915_private, port: u32) -> i32 {
    let val = intel_de_read(i915, DDI_CLK_SEL(port)) & DDI_CLK_SEL_MASK;
    match val {
        DDI_CLK_SEL_NONE => 0,
        DDI_CLK_SEL_TBT_162 => 162000,
        DDI_CLK_SEL_TBT_270 => 270000,
        DDI_CLK_SEL_TBT_540 => 540000,
        DDI_CLK_SEL_TBT_810 => 810000,
        _ => {
            Line::new().s("MISSING_CASE TBT clock select ").hex(val).say();
            0
        }
    }
}

/// `icl_set_active_port_dpll`.
pub fn icl_set_active_port_dpll(crtc_state: &mut intel_crtc_state, port_dpll_id: u32) {
    let port_dpll = crtc_state.icl_port_dplls[port_dpll_id as usize];
    crtc_state.intel_dpll = port_dpll.pll;
    crtc_state.dpll_hw_state = port_dpll.hw_state;
}

/// The PLL readout shared by `intel_ddi_get_clock` and
/// `icl_ddi_tc_get_clock`: fill the port DPLL slot and make it active.
fn read_port_dpll(i915: &mut drm_i915_private, vbt: &intel_vbt_data, crtc_state: &mut intel_crtc_state,
                  pll: usize, port_dpll_id: u32) {
    let p = i915.display.dpll.dplls[pll];
    let mut hw_state = crtc_state.icl_port_dplls[port_dpll_id as usize].hw_state;
    let pll_active = intel_dpll_get_hw_state(i915, vbt, &p, &mut hw_state);
    if !pll_active {
        linux::say("warning: the encoder's PLL is not active");
    }
    crtc_state.icl_port_dplls[port_dpll_id as usize] = icl_port_dpll { pll: Some(pll), hw_state };
    icl_set_active_port_dpll(crtc_state, port_dpll_id);
}

/// `intel_ddi_get_clock`.
fn intel_ddi_get_clock(i915: &mut drm_i915_private, vbt: &intel_vbt_data, crtc_state: &mut intel_crtc_state,
                       pll: Option<usize>) {
    let Some(pll) = pll else {
        linux::say("warning: no PLL for the encoder");
        return;
    };
    read_port_dpll(i915, vbt, crtc_state, pll, ICL_PORT_DPLL_DEFAULT);
    let p = i915.display.dpll.dplls[crtc_state.intel_dpll.unwrap_or(pll)];
    crtc_state.port_clock = intel_dpll_get_freq(i915, &p, &crtc_state.dpll_hw_state) as i32;
}

/// `icl_ddi_tc_get_clock`.
fn icl_ddi_tc_get_clock(i915: &mut drm_i915_private, vbt: &intel_vbt_data, e: &intel_encoder,
                        crtc_state: &mut intel_crtc_state, pll: Option<usize>) {
    let Some(pll) = pll else {
        linux::say("warning: no PLL for the encoder");
        return;
    };
    let is_tbt = crate::tables::tgl_plls[pll].id == DPLL_ID_ICL_TBTPLL;
    let port_dpll_id = if is_tbt { ICL_PORT_DPLL_DEFAULT } else { ICL_PORT_DPLL_MG_PHY };
    read_port_dpll(i915, vbt, crtc_state, pll, port_dpll_id);

    if crtc_state.pll_is_tbt() == Some(true) {
        crtc_state.port_clock = icl_calc_tbt_pll_link(i915, e.port);
    } else {
        let p = i915.display.dpll.dplls[crtc_state.intel_dpll.unwrap_or(pll)];
        crtc_state.port_clock = intel_dpll_get_freq(i915, &p, &crtc_state.dpll_hw_state) as i32;
    }
}

/// `intel_ddi_is_audio_enabled` (no eDP transcoder on display version 12).
fn intel_ddi_is_audio_enabled(i915: &mut drm_i915_private, cpu_transcoder: u32) -> bool {
    if cpu_transcoder == TRANSCODER_EDP {
        return false;
    }
    if !intel_display_power_is_enabled(i915, POWER_DOMAIN_AUDIO_MMIO) {
        return false;
    }
    intel_de_read(i915, HSW_AUD_PIN_ELD_CP_VLD) & AUDIO_OUTPUT_ENABLE(cpu_transcoder) != 0
}

/// `intel_ddi_compute_min_voltage_level` → `tgl_ddi_min_voltage_level`.
pub fn intel_ddi_compute_min_voltage_level(crtc_state: &mut intel_crtc_state) {
    crtc_state.min_voltage_level = if crtc_state.port_clock > 594000 { 2 } else { 0 };
}

/// `bdw_transcoder_master_readout` (display version 11+).
fn bdw_transcoder_master_readout(i915: &mut drm_i915_private, cpu_transcoder: u32) -> u32 {
    let ctl2 = intel_de_read(i915, TRANS_DDI_FUNC_CTL2(cpu_transcoder));
    if ctl2 & PORT_SYNC_MODE_ENABLE == 0 {
        return INVALID_TRANSCODER as u32;
    }
    let master_select = REG_FIELD_GET(PORT_SYNC_MODE_MASTER_SELECT_MASK, ctl2);
    if master_select == 0 {
        TRANSCODER_EDP
    } else {
        master_select - 1
    }
}

/// `bdw_get_trans_port_sync_config`.
fn bdw_get_trans_port_sync_config(i915: &mut drm_i915_private, crtc_state: &mut intel_crtc_state) {
    let transcoders = (1 << TRANSCODER_A) | (1 << TRANSCODER_B) | (1 << TRANSCODER_C) | (1 << TRANSCODER_D);

    crtc_state.master_transcoder = bdw_transcoder_master_readout(i915, crtc_state.cpu_transcoder);

    for cpu_transcoder in 0..I915_MAX_TRANSCODERS {
        if transcoders & (1 << cpu_transcoder) == 0
            || i915.display.runtime.cpu_transcoder_mask & (1 << cpu_transcoder) == 0 {
            continue;
        }
        let power_domain = POWER_DOMAIN_TRANSCODER(cpu_transcoder);
        if !intel_display_power_get_if_enabled(i915, power_domain) {
            continue;
        }
        if bdw_transcoder_master_readout(i915, cpu_transcoder) == crtc_state.cpu_transcoder {
            crtc_state.sync_mode_slaves_mask |= 1 << cpu_transcoder;
        }
        intel_display_power_put(i915, power_domain);
    }

    if crtc_state.master_transcoder != INVALID_TRANSCODER as u32 && crtc_state.sync_mode_slaves_mask != 0 {
        linux::say("warning: transcoder is both a port sync master and a slave");
    }
}

/// `tgl_dp_tp_transcoder`.
fn tgl_dp_tp_transcoder(crtc_state: &intel_crtc_state) -> u32 {
    if crtc_state.has_type(INTEL_OUTPUT_DP_MST) {
        crtc_state.mst_master_transcoder
    } else {
        crtc_state.cpu_transcoder
    }
}

/// `dp_tp_ctl_reg` (display version 12).
fn dp_tp_ctl_reg(crtc_state: &intel_crtc_state) -> u32 {
    TGL_DP_TP_CTL(tgl_dp_tp_transcoder(crtc_state))
}

/// `intel_ddi_read_func_ctl_dvi` (display version below 14: four lanes).
fn intel_ddi_read_func_ctl_dvi(crtc_state: &mut intel_crtc_state) {
    crtc_state.output_types |= 1 << INTEL_OUTPUT_HDMI;
    crtc_state.lane_count = 4;
}

/// `intel_ddi_read_func_ctl_hdmi`.
fn intel_ddi_read_func_ctl_hdmi(i915: &mut drm_i915_private, crtc_state: &mut intel_crtc_state, ddi_func_ctl: u32) {
    crtc_state.has_hdmi_sink = true;
    crtc_state.infoframes.enable |= intel_hdmi::intel_hdmi_infoframes_enabled(i915, crtc_state.cpu_transcoder);
    if crtc_state.infoframes.enable != 0 {
        crtc_state.has_infoframe = true;
    }
    if ddi_func_ctl & TRANS_DDI_HDMI_SCRAMBLING != 0 {
        crtc_state.hdmi_scrambling = true;
    }
    if ddi_func_ctl & TRANS_DDI_HIGH_TMDS_CHAR_RATE != 0 {
        crtc_state.hdmi_high_tmds_clock_ratio = true;
    }
    intel_ddi_read_func_ctl_dvi(crtc_state);
}

/// `intel_ddi_read_func_ctl_fdi`.
fn intel_ddi_read_func_ctl_fdi(i915: &mut drm_i915_private, crtc_state: &mut intel_crtc_state) {
    crtc_state.output_types |= 1 << INTEL_OUTPUT_ANALOG;
    crtc_state.enhanced_framing = intel_de_read(i915, dp_tp_ctl_reg(crtc_state)) & DP_TP_CTL_ENHANCED_FRAME_ENABLE != 0;
}

/// `intel_ddi_read_func_ctl_dp_sst`. LSPCON is not in this VBT's ports
/// (`intel_lspcon_active` is false), and `dp_m2_n2` exists only on
/// display versions 5-7.
fn intel_ddi_read_func_ctl_dp_sst(i915: &mut drm_i915_private, e: &intel_encoder,
                                  crtc_state: &mut intel_crtc_state, ddi_func_ctl: u32) {
    let cpu_transcoder = crtc_state.cpu_transcoder;

    if e.type_ == INTEL_OUTPUT_EDP {
        crtc_state.output_types |= 1 << INTEL_OUTPUT_EDP;
    } else {
        crtc_state.output_types |= 1 << INTEL_OUTPUT_DP;
    }
    crtc_state.lane_count = (((ddi_func_ctl & DDI_PORT_WIDTH_MASK) >> DDI_PORT_WIDTH_SHIFT) + 1) as i32;

    if ddi_func_ctl & TRANS_DDI_MODE_SELECT_MASK == TRANS_DDI_MODE_SELECT_FDI_OR_128B132B {
        crtc_state.mst_master_transcoder = REG_FIELD_GET(TRANS_DDI_MST_TRANSPORT_SELECT_MASK, ddi_func_ctl);
    }

    crtc_state.dp_m_n = intel_display::intel_cpu_transcoder_get_m1_n1(i915, cpu_transcoder);

    let tp_ctl = intel_de_read(i915, dp_tp_ctl_reg(crtc_state));
    crtc_state.enhanced_framing = tp_ctl & DP_TP_CTL_ENHANCED_FRAME_ENABLE != 0;
    crtc_state.fec_enable = intel_de_read(i915, dp_tp_ctl_reg(crtc_state)) & DP_TP_CTL_FEC_ENABLE != 0;

    crtc_state.infoframes.enable |= intel_hdmi::intel_hdmi_infoframes_enabled(i915, crtc_state.cpu_transcoder);
}

/// `intel_ddi_read_func_ctl_dp_mst`.
fn intel_ddi_read_func_ctl_dp_mst(i915: &mut drm_i915_private, crtc_state: &mut intel_crtc_state, ddi_func_ctl: u32) {
    let cpu_transcoder = crtc_state.cpu_transcoder;

    crtc_state.output_types |= 1 << INTEL_OUTPUT_DP_MST;
    crtc_state.lane_count = (((ddi_func_ctl & DDI_PORT_WIDTH_MASK) >> DDI_PORT_WIDTH_SHIFT) + 1) as i32;
    crtc_state.mst_master_transcoder = REG_FIELD_GET(TRANS_DDI_MST_TRANSPORT_SELECT_MASK, ddi_func_ctl);

    crtc_state.dp_m_n = intel_display::intel_cpu_transcoder_get_m1_n1(i915, cpu_transcoder);
    crtc_state.fec_enable = intel_de_read(i915, dp_tp_ctl_reg(crtc_state)) & DP_TP_CTL_FEC_ENABLE != 0;
    crtc_state.infoframes.enable |= intel_hdmi::intel_hdmi_infoframes_enabled(i915, crtc_state.cpu_transcoder);
}

/// `intel_ddi_read_func_ctl` (no 128b/132b: HAS_DP20 is false).
fn intel_ddi_read_func_ctl(i915: &mut drm_i915_private, e: &intel_encoder, crtc_state: &mut intel_crtc_state) {
    let cpu_transcoder = crtc_state.cpu_transcoder;
    let ddi_func_ctl = intel_de_read(i915, TRANS_DDI_FUNC_CTL(cpu_transcoder));
    let mut flags = 0;

    flags |= if ddi_func_ctl & TRANS_DDI_PHSYNC != 0 { DRM_MODE_FLAG_PHSYNC } else { DRM_MODE_FLAG_NHSYNC };
    flags |= if ddi_func_ctl & TRANS_DDI_PVSYNC != 0 { DRM_MODE_FLAG_PVSYNC } else { DRM_MODE_FLAG_NVSYNC };
    crtc_state.hw.adjusted_mode.flags |= flags;

    match ddi_func_ctl & TRANS_DDI_BPC_MASK {
        TRANS_DDI_BPC_6 => crtc_state.pipe_bpp = 18,
        TRANS_DDI_BPC_8 => crtc_state.pipe_bpp = 24,
        TRANS_DDI_BPC_10 => crtc_state.pipe_bpp = 30,
        TRANS_DDI_BPC_12 => crtc_state.pipe_bpp = 36,
        _ => {}
    }

    let ddi_mode = ddi_func_ctl & TRANS_DDI_MODE_SELECT_MASK;
    if ddi_mode == TRANS_DDI_MODE_SELECT_HDMI {
        intel_ddi_read_func_ctl_hdmi(i915, crtc_state, ddi_func_ctl);
    } else if ddi_mode == TRANS_DDI_MODE_SELECT_DVI {
        intel_ddi_read_func_ctl_dvi(crtc_state);
    } else if ddi_mode == TRANS_DDI_MODE_SELECT_FDI_OR_128B132B {
        intel_ddi_read_func_ctl_fdi(i915, crtc_state);
    } else if ddi_mode == TRANS_DDI_MODE_SELECT_DP_SST {
        intel_ddi_read_func_ctl_dp_sst(i915, e, crtc_state, ddi_func_ctl);
    } else if ddi_mode == TRANS_DDI_MODE_SELECT_DP_MST {
        intel_ddi_read_func_ctl_dp_mst(i915, crtc_state, ddi_func_ctl);
    }
}

/// `intel_ddi_mso_get_config` (HAS_MSO; only pipe A may split).
fn intel_ddi_mso_get_config(i915: &mut drm_i915_private, pipe: u32, crtc_state: &mut intel_crtc_state) {
    let dss1 = intel_de_read(i915, crate::intel_vdsc::ICL_PIPE_DSS_CTL1_of(pipe));

    crtc_state.splitter.enable = dss1 & SPLITTER_ENABLE as u32 != 0;
    if !crtc_state.splitter.enable {
        return;
    }
    if pipe != PIPE_A {
        linux::say("warning: splitter enabled on a pipe that cannot split");
        crtc_state.splitter.enable = false;
        return;
    }

    crtc_state.splitter.link_count = match dss1 & SPLITTER_CONFIGURATION_MASK {
        SPLITTER_CONFIGURATION_4_SEGMENT => 4,
        SPLITTER_CONFIGURATION_2_SEGMENT => 2,
        _ => {
            Line::new().s("warning: invalid splitter configuration, dss1=").hex(dss1).say();
            2
        }
    };
    crtc_state.splitter.pixel_overlap = REG_FIELD_GET(OVERLAP_PIXELS_MASK, dss1) as i32;
}

/// `ddi_dotclock_get`.
fn ddi_dotclock_get(crtc_state: &mut intel_crtc_state) {
    // CRT dotclock is determined via other means
    if crtc_state.has_pch_encoder {
        return;
    }
    crtc_state.hw.adjusted_mode.crtc_clock = intel_display::intel_crtc_dotclock(crtc_state);
}

/// `intel_ddi_get_config`. The DIP payloads (`intel_read_infoframe`,
/// `intel_read_dp_sdp`) need the HDMI/DP infoframe unpackers, and the
/// audio codec readout the ELD; both only feed the state checker and are
/// left out, with a line when one would have data. PSR is never on: it
/// needs the sink's DPCD (`CAN_PSR`), which is not read.
fn intel_ddi_get_config(i915: &mut drm_i915_private, e: &intel_encoder, pipe: u32, crtc_state: &mut intel_crtc_state) {
    let cpu_transcoder = crtc_state.cpu_transcoder;

    // XXX: DSI transcoder paranoia
    if intel_display::transcoder_is_dsi(cpu_transcoder) {
        linux::say("warning: DDI encoder on a DSI transcoder");
        return;
    }

    intel_ddi_read_func_ctl(i915, e, crtc_state);

    intel_ddi_mso_get_config(i915, pipe, crtc_state);

    crtc_state.has_audio = intel_ddi_is_audio_enabled(i915, cpu_transcoder);

    ddi_dotclock_get(crtc_state);

    intel_ddi_compute_min_voltage_level(crtc_state);

    intel_hdmi::intel_hdmi_read_gcp_infoframe(i915, crtc_state);

    let payloads = crtc_state.infoframes.enable
        & !intel_hdmi::intel_hdmi_infoframe_enable(HDMI_PACKET_TYPE_GENERAL_CONTROL);
    if payloads != 0 {
        Line::new().s("[ENCODER:").s(e.name()).s("] infoframe/SDP payloads ").hex(payloads)
            .s(" not read back").say();
    }

    bdw_get_trans_port_sync_config(i915, crtc_state);

    if crtc_state.has_audio {
        Line::new().s("[ENCODER:").s(e.name()).s("] audio on, codec config not read back").say();
    }
}

/// `encoder->get_config` (`icl_ddi_combo_get_config` /
/// `icl_ddi_tc_get_config`), then `intel_crtc_readout_derived_state`, as
/// `intel_encoder_get_config` does. Fake MST encoders are never active
/// here (no topology is probed).
pub fn intel_encoder_get_config(i915: &mut drm_i915_private, encoder: usize, c: usize) {
    let o = i915.display.outputs;
    let e = o.encoders[encoder];
    let vbt = i915.display.bios.vbt;
    let pipe = i915.display.crtcs[c].pipe;
    let mut cs = i915.display.crtcs[c].state;

    match e.clock_ops {
        ddi_clock_ops::icl_ddi_combo => {
            let pll = icl_ddi_combo_get_pll(i915, &e);
            intel_ddi_get_clock(i915, &vbt, &mut cs, pll);
        }
        ddi_clock_ops::icl_ddi_tc => {
            let pll = icl_ddi_tc_get_pll(i915, &e);
            icl_ddi_tc_get_clock(i915, &vbt, &e, &mut cs, pll);
        }
        ddi_clock_ops::none => {
            Line::new().s("[ENCODER:").s(e.name()).s("] has no get_config").say();
            return;
        }
    }
    intel_ddi_get_config(i915, &e, pipe, &mut cs);

    intel_display::intel_crtc_readout_derived_state(&mut cs, pipe);
    i915.display.crtcs[c].state = cs;
}
