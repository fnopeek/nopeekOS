//! display/intel_hdmi.c `intel_hdmi_init_connector`, `intel_hdmi_ddc_pin`
//! and `intel_infoframe_init` (Linux 6.18.55) for a Tiger Lake display
//! with PCH TGP.
//!
//! Properties, HDCP and the CEC notifier only register with DRM and are
//! left out.

use crate::intel_bios;
use crate::intel_encoder::*;
use crate::linux::Line;
use crate::regs::*;

/// `intel_infoframe_init` (DDI platform).
pub fn intel_infoframe_init(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) {
    let e = &o.encoders[o.dig_ports[d].encoder];
    let devdata = &vbt.devices()[e.devdata];
    o.dig_ports[d].infoframes = if intel_bios::intel_bios_encoder_is_lspcon(vbt, devdata) {
        infoframe_ops::lspcon
    } else {
        infoframe_ops::hsw
    };
}

/// `icl_encoder_to_ddc_pin`.
fn icl_encoder_to_ddc_pin(e: &intel_encoder) -> u8 {
    if intel_encoder_is_combo(e) {
        return (GMBUS_PIN_1_BXT + e.port) as u8;
    } else if intel_encoder_is_tc(e) {
        return (GMBUS_PIN_9_TC1_ICP as i32 + intel_encoder_to_tc(e)) as u8;
    }
    let p = port_name(e.port);
    Line::new().s("warning: Unknown port:").s(core::str::from_utf8(&p).unwrap_or("?")).say();
    GMBUS_PIN_2_BXT as u8
}

/// `intel_hdmi_default_ddc_pin`: PCH TGP is ICP or later and below DG1.
fn intel_hdmi_default_ddc_pin(e: &intel_encoder) -> u8 {
    icl_encoder_to_ddc_pin(e)
}

/// `get_encoder_by_ddc_pin`: `intel_gmbus_get_adapter` is one adapter per
/// pin, so the adapters compare equal exactly when the pins do.
fn get_encoder_by_ddc_pin(o: &intel_outputs, encoder: usize, ddc_pin: u8) -> Option<usize> {
    (0..o.n_encoders).find(|&other| {
        if other == encoder {
            return false;
        }
        let Some(d) = o.dig_port_of(other) else {
            return false;
        };
        o.dig_ports[d].hdmi.attached_connector.is_some_and(|c| o.connectors[c].ddc_pin == ddc_pin)
    })
}

/// `intel_hdmi_ddc_pin`.
fn intel_hdmi_ddc_pin(o: &intel_outputs, vbt: &intel_bios::intel_display, encoder: usize) -> u8 {
    let e = &o.encoders[encoder];
    let mut ddc_pin = intel_bios::intel_bios_hdmi_ddc_pin(vbt, &vbt.devices()[e.devdata]) as u8;
    let mut source = "VBT";

    if ddc_pin == 0 {
        ddc_pin = intel_hdmi_default_ddc_pin(e);
        source = "platform default";
    }

    if !intel_bios::intel_gmbus_is_valid_pin(vbt, ddc_pin as u32) {
        Line::new().s("[ENCODER:").s(e.name()).s("] Invalid DDC pin ").dec(ddc_pin as u64).say();
        return 0;
    }

    if let Some(other) = get_encoder_by_ddc_pin(o, encoder, ddc_pin) {
        Line::new().s("[ENCODER:").s(e.name()).s("] DDC pin ").dec(ddc_pin as u64)
            .s(" already claimed by [ENCODER:").s(o.encoders[other].name()).s("]").say();
        return 0;
    }

    Line::new().s("[ENCODER:").s(e.name()).s("] Using DDC pin ").hex(ddc_pin as u32)
        .s(" (").s(source).s(")").say();
    ddc_pin
}

/// `intel_hdmi_init_connector`.
pub fn intel_hdmi_init_connector(o: &mut intel_outputs, vbt: &intel_bios::intel_display, d: usize) -> bool {
    let enc = o.dig_ports[d].encoder;

    Line::new().s("Adding HDMI connector on [ENCODER:").s(o.encoders[enc].name()).s("]").say();

    if o.dig_ports[d].max_lanes < 4 {
        Line::new().s("Not enough lanes (").dec(o.dig_ports[d].max_lanes as u64).s(") for HDMI on [ENCODER:")
            .s(o.encoders[enc].name()).s("]").say();
        return false;
    }

    let ddc_pin = intel_hdmi_ddc_pin(o, vbt, enc);
    if ddc_pin == 0 {
        return false;
    }

    let Some(c) = o.add_connector(intel_connector {
        connector_type: DRM_MODE_CONNECTOR_HDMIA,
        encoder: enc,
        polled: DRM_CONNECTOR_POLL_HPD,
        ddc_pin,
    }) else {
        return false;
    };
    o.dig_ports[d].hdmi.attached_connector = Some(c);
    true
}
