//! What the DRM core made of a connector, as the `# kms:` lines the pi5
//! oracle prints at the end of run G (oracle `src/kmodes.c`): the tests
//! compare a port's lines with Linux's.

use crate::drm_connector::drm_connector;
use crate::drm_modes::drm_display_mode;
use crate::regs::*;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One mode, `what` being `mode` or `chosen`.
pub fn mode_line(what: &str, m: &drm_display_mode) -> String {
    format!(
        "# kms: {} \"{}\": {} {} {} {} {} {} {} {} {} {} {} flags {:#x} type {:#x} aspect {}",
        what, m.name, m.clock, m.hdisplay, m.hsync_start, m.hsync_end, m.htotal, m.hskew, m.vdisplay,
        m.vsync_start, m.vsync_end, m.vtotal, m.vscan, m.flags, m.r#type, m.picture_aspect_ratio
    )
}

/// `%#x` as C prints it: `0x0` for zero too.
fn x(v: u32) -> String {
    format!("0x{:x}", v)
}

/// The connector's lines: status, display info, HDMI info, VICs, ELD and
/// its `modes` list.
pub fn connector_lines(c: &drm_connector) -> Vec<String> {
    let i = &c.display_info;
    let h = &i.hdmi;
    let mut o = Vec::new();

    o.push(format!("# kms: connector {} status {}", c.name, c.status as i32));
    o.push(format!(
        "# kms: info {}x{} mm bpc {} color_formats {} is_hdmi {} has_audio {} max_tmds_clock {} dvi_dual {} \
         cea_rev {} non_desktop {} quirks {} spa {} rgb_qrs {} dc_rgb444 {} dc_ycbcr444 {}",
        i.width_mm, i.height_mm, i.bpc, x(i.color_formats), i.is_hdmi as i32, i.has_audio as i32,
        i.max_tmds_clock, i.dvi_dual as i32, i.cea_rev, i.non_desktop as i32, x(i.quirks),
        x(i.source_physical_address as u32), i.rgb_quant_range_selectable as i32,
        x(i.edid_hdmi_rgb444_dc_modes as u32), x(i.edid_hdmi_ycbcr444_dc_modes as u32)
    ));
    o.push(format!(
        "# kms: hdmi scdc {} read_request {} scrambling {} low_rates {} y420_dc {} frl {} lanes {}",
        h.scdc.supported as i32, h.scdc.read_request as i32, h.scdc.scrambling.supported as i32,
        h.scdc.scrambling.low_rates as i32, x(h.y420_dc_modes as u32), h.max_frl_rate_per_lane, h.max_lanes
    ));
    let mut l = String::from("# kms: vics");
    for v in &i.vics {
        l += &format!(" {}", v);
    }
    o.push(l);
    let size = DRM_ELD_HEADER_BLOCK_SIZE as usize + c.eld[DRM_ELD_BASELINE_ELD_LEN as usize] as usize * 4;
    let mut l = String::from("# kms: eld");
    for b in &c.eld[..size.min(c.eld.len())] {
        l += &format!(" {:02x}", b);
    }
    o.push(l);
    for m in &c.modes {
        o.push(mode_line("mode", m));
    }
    o
}

/// The client's modesets: CRTC, offset, connectors (the first by name)
/// and the chosen mode.
pub fn modeset_lines(client: &crate::drm_client_modeset::drm_client_dev, connectors: &[drm_connector])
    -> Vec<String> {
    let mut o = Vec::new();
    for ms in &client.modesets {
        let first = ms.connectors.first().map(|&i| connectors[i].name.as_str());
        o.push(format!(
            "# kms: modeset crtc {} x {} y {} connectors {}{}{}",
            ms.crtc, ms.x, ms.y, ms.connectors.len(),
            if first.is_some() { " " } else { "" },
            first.unwrap_or("")
        ));
        if let Some(m) = &ms.mode {
            o.push(mode_line("chosen", m));
        }
    }
    o
}
