//! display/intel_ddi.c `intel_ddi_init` and the encoder hooks the HW
//! readout calls, intel_display.c `intel_setup_outputs` and intel_hti.c
//! (Linux 6.18.55), for a Tiger Lake display (version 12, PCH TGP).
//!
//! `intel_ddi_get_config` is not here: it fills the crtc state.

use crate::i915_drv::drm_i915_private;
use crate::intel_bios;
use crate::intel_de::*;
use crate::intel_display_device::intel_display_runtime_info;
use crate::intel_display_power::{intel_display_power_ddi_io_domain, intel_display_power_ddi_lanes_domain,
                                 intel_display_power_get, intel_display_power_get_if_enabled,
                                 intel_display_power_put};
use crate::intel_dp;
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
                               vbt: &intel_bios::intel_display, d: usize, pipe_mask: u32) -> Result<(), i32> {
    let port = o.encoders[o.dig_ports[d].encoder].port;
    o.dig_ports[d].dp.output_reg = DDI_BUF_CTL(port);
    if !intel_dp::intel_dp_init_connector(i915, o, vbt, d, pipe_mask) {
        return Err(-(EINVAL as i32));
    }
    Ok(())
}

/// `intel_ddi_init_hdmi_connector`: a failure (conflicting DDC pin, too
/// few lanes) leaves the port without HDMI but does not fail the DDI.
fn intel_ddi_init_hdmi_connector(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) -> Result<(), i32> {
    let port = o.encoders[o.dig_ports[d].encoder].port;
    o.dig_ports[d].hdmi.hdmi_reg = DDI_BUF_CTL(port);
    if !intel_hdmi::intel_hdmi_init_connector(o, vbt, d) {
        o.dig_ports[d].hdmi.hdmi_reg = 0;
    }
    Ok(())
}

/// `intel_ddi_init`.
fn intel_ddi_init(i915: &mut drm_i915_private, o: &mut intel_outputs, vbt: &intel_bios::intel_display,
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
pub fn intel_setup_outputs(i915: &mut drm_i915_private, vbt: &intel_bios::intel_display,
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
fn intel_ddi_main_link_aux_domain(o: &intel_outputs, d: usize, cs: &encoder_crtc_state) -> u32 {
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
                                  cs: &encoder_crtc_state) {
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
                                   cs: &encoder_crtc_state) {
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
pub fn intel_ddi_port_pll_type(o: &intel_outputs, encoder: usize, cs: &encoder_crtc_state) -> u32 {
    if o.encoders[encoder].clock_ops != ddi_clock_ops::icl_ddi_tc {
        return ICL_PORT_DPLL_DEFAULT;
    }
    match cs.pll_is_tbt {
        None => {
            linux::say("warning: TC port PLL type without a PLL");
            ICL_PORT_DPLL_DEFAULT
        }
        Some(true) => ICL_PORT_DPLL_DEFAULT,
        Some(false) => ICL_PORT_DPLL_MG_PHY,
    }
}

/// `intel_ddi_sync_state`.
pub fn intel_ddi_sync_state(i915: &mut drm_i915_private, o: &mut intel_outputs, vbt: &intel_bios::intel_display,
                            encoder: usize, crtc_state: Option<&encoder_crtc_state>) {
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
