//! Intel display codecs of the Tiger Lake model, after Linux 6.18.55
//! `sound/hda/codecs/hdmi/intelhdmi.c` (`probe_i915_tgl_hdmi`) and the
//! parts of `hdmi.c`, `hdmi_chmap.c` and `jack.c` one stereo PCM stream to
//! one pin goes through.
//!
//! The pin is the one whose presence detect the display driver has set
//! (`AUDIO_OUTPUT_ENABLE`). The connection type the audio infoframe needs
//! comes from the ELD in Linux; until the display's ELD reaches this driver
//! it is taken from the pin: DisplayPort when the pin can do it.

use crate::host::*;
use crate::regs::*;
use crate::{codec_cmd, get_param, log, loghex, widget_type};

/// `snd_hda_id_intelhdmi` entries with `MODEL_TGL`.
const TGL_IDS: [u32; 5] = [0x80862812, 0x80862814, 0x80862815, 0x80862816, 0x80862818];

/// `probe_i915_tgl_hdmi`: the vendor nid, and the pin of each display port
/// (the index is the i915 port).
const VENDOR_NID: u32 = 0x02;
const PORT_MAP: [u32; 9] = [0x4, 0x6, 0x8, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];

const INTEL_GET_VENDOR_VERB: u32 = 0xf81;
const INTEL_SET_VENDOR_VERB: u32 = 0x781;
/// Enable DP 1.2 features.
const INTEL_EN_DP12: u32 = 0x02;
/// Enable the 2nd and 3rd pins and converters.
const INTEL_EN_ALL_PIN_CVTS: u32 = 0x01;

const AC_VERB_GET_CONNECT_SEL: u32 = 0xf01;
const AC_VERB_GET_POWER_STATE: u32 = 0xf05;
const AC_VERB_GET_PIN_SENSE: u32 = 0xf09;
const AC_VERB_GET_DIGI_CONVERT_1: u32 = 0xf0d;
const AC_VERB_GET_HDMI_DIP_DATA: u32 = 0xf31;
const AC_VERB_GET_HDMI_DIP_XMIT: u32 = 0xf32;
const AC_VERB_SET_CONNECT_SEL: u32 = 0x701;
const AC_VERB_SET_POWER_STATE: u32 = 0x705;
const AC_VERB_SET_PIN_WIDGET_CONTROL: u32 = 0x707;
const AC_VERB_SET_PIN_SENSE: u32 = 0x709;
const AC_VERB_SET_DIGI_CONVERT_1: u32 = 0x70d;
const AC_VERB_SET_CVT_CHAN_COUNT: u32 = 0x72d;
const AC_VERB_SET_HDMI_DIP_INDEX: u32 = 0x730;
const AC_VERB_SET_HDMI_DIP_DATA: u32 = 0x731;
const AC_VERB_SET_HDMI_DIP_XMIT: u32 = 0x732;
const AC_VERB_SET_HDMI_CHAN_SLOT: u32 = 0x734;
const AC_VERB_SET_DEVICE_SEL: u32 = 0x735;
const AC_VERB_SET_DIGI_CONVERT_3: u32 = 0x73e;

const AC_PAR_VENDOR_ID: u32 = 0x00;
const AC_PAR_DEVLIST_LEN: u32 = 0x15;
const AC_WCAP_DIGITAL: u32 = 1 << 9;
const AC_PINCAP_TRIG_REQ: u32 = 1 << 1;
const AC_PINCAP_HDMI: u32 = 1 << 7;
const AC_PINCAP_DP: u32 = 1 << 24;
const AC_PINSENSE_PRESENCE: u32 = 1 << 31;
const AC_PINSENSE_ELDV: u32 = 1 << 30;
const AC_PWRST_D0: u32 = 0x00;
const AC_PWRST_ACTUAL: u32 = 0xf << 4;
const AC_PWRST_ACTUAL_SHIFT: u32 = 4;
const AC_DIG1_ENABLE: u32 = 1 << 0;
const AC_DIG3_ICT: u32 = 0xf;
const AC_DIPXMIT_DISABLE: u32 = 0x0 << 6;
const AC_DIPXMIT_BEST: u32 = 0x3 << 6;
const PIN_OUT: u32 = 0x40;
/// `AMP_OUT_UNMUTE`: output amp, both channels, mute cleared.
const AMP_OUT_UNMUTE: u16 = 0xb000;

/// `union audio_infoframe`: the HDMI layout is the larger, 11 bytes.
const AUDIO_INFOFRAME_BYTES: usize = 11;

/// A short verb: 12-bit id and an 8-bit payload.
fn verb(id: u32, payload: u32) -> u32 {
    (id << 8) | (payload & 0xff)
}

fn read(mmio: i32, cad: u32, nid: u32, id: u32, payload: u32) -> u32 {
    codec_cmd(mmio, cad, nid, verb(id, payload)).unwrap_or(u32::MAX)
}

fn write(mmio: i32, cad: u32, nid: u32, id: u32, payload: u32) {
    codec_cmd(mmio, cad, nid, verb(id, payload));
}

/// Is this a codec `intelhdmi_probe` takes with the Tiger Lake model?
pub fn is_tgl_hdmi(mmio: i32, cad: u32) -> bool {
    TGL_IDS.contains(&get_param(mmio, cad, 0, AC_PAR_VENDOR_ID))
}

/// `intel_haswell_enable_all_pins`.
fn intel_haswell_enable_all_pins(mmio: i32, cad: u32) {
    let vendor_param = read(mmio, cad, VENDOR_NID, INTEL_GET_VENDOR_VERB, 0);
    if vendor_param == u32::MAX || vendor_param & INTEL_EN_ALL_PIN_CVTS != 0 {
        return;
    }
    read(mmio, cad, VENDOR_NID, INTEL_SET_VENDOR_VERB, vendor_param | INTEL_EN_ALL_PIN_CVTS);
}

/// `intel_haswell_fixup_enable_dp12`.
fn intel_haswell_fixup_enable_dp12(mmio: i32, cad: u32) {
    let vendor_param = read(mmio, cad, VENDOR_NID, INTEL_GET_VENDOR_VERB, 0);
    if vendor_param == u32::MAX || vendor_param & INTEL_EN_DP12 != 0 {
        return;
    }
    write(mmio, cad, VENDOR_NID, INTEL_SET_VENDOR_VERB, vendor_param | INTEL_EN_DP12);
}

/// `read_pin_sense` (jack.c).
fn read_pin_sense(mmio: i32, cad: u32, nid: u32, dev_id: u32) -> u32 {
    if get_param(mmio, cad, nid, PARAM_PIN_CAP) & AC_PINCAP_TRIG_REQ != 0 {
        read(mmio, cad, nid, AC_VERB_SET_PIN_SENSE, 0);
    }
    read(mmio, cad, nid, AC_VERB_GET_PIN_SENSE, dev_id)
}

/// `snd_hda_set_dev_select`: only on a pin with more than one device entry.
fn snd_hda_set_dev_select(mmio: i32, cad: u32, nid: u32, dev_id: u32) {
    let num_devices = (get_param(mmio, cad, nid, AC_PAR_DEVLIST_LEN) & 0x3f) + 1;
    if num_devices == 1 || num_devices <= dev_id {
        return;
    }
    write(mmio, cad, nid, AC_VERB_SET_DEVICE_SEL, dev_id);
}

/// `haswell_verify_D0`: a converter may stay in D3 after boot.
fn haswell_verify_d0(mmio: i32, cad: u32, cvt_nid: u32, nid: u32) {
    let d0 = |n: u32| (read(mmio, cad, n, AC_VERB_GET_POWER_STATE, 0) & AC_PWRST_ACTUAL) >> AC_PWRST_ACTUAL_SHIFT
        == AC_PWRST_D0;
    if !d0(cvt_nid) {
        write(mmio, cad, cvt_nid, AC_VERB_SET_POWER_STATE, AC_PWRST_D0);
    }
    if !d0(nid) {
        write(mmio, cad, nid, AC_VERB_SET_POWER_STATE, AC_PWRST_D0);
        sleep_ms(40);
    }
}

/// The digital audio output converters, in codec order.
fn converters(mmio: i32, cad: u32, w_start: u32, w_count: u32, out: &mut [u32; 8]) -> usize {
    let mut n = 0;
    for nid in w_start..w_start + w_count {
        if widget_type(mmio, cad, nid) == WTYPE_DAC
            && get_param(mmio, cad, nid, PARAM_AUDIO_WIDGET_CAP) & AC_WCAP_DIGITAL != 0
            && n < out.len()
        {
            out[n] = nid;
            n += 1;
        }
    }
    n
}

/// `intel_not_share_assigned_cvt`: no other pin may select the converter
/// the chosen pin uses; each one found is moved to the next one.
fn intel_not_share_assigned_cvt(mmio: i32, cad: u32, pin_nid: u32, mux_idx: u32, num_cvts: usize) {
    for &nid in PORT_MAP.iter() {
        if nid == pin_nid || widget_type(mmio, cad, nid) != WTYPE_PIN {
            continue;
        }
        if read(mmio, cad, nid, AC_VERB_GET_CONNECT_SEL, 0) != mux_idx {
            continue;
        }
        if let Some(other) = (0..num_cvts as u32).find(|&i| i != mux_idx) {
            write(mmio, cad, nid, AC_VERB_SET_CONNECT_SEL, other);
        }
    }
}

/// `hdmi_init_pin`: unmute, and the pin out on (no dynamic pin out on
/// Intel).
fn hdmi_init_pin(mmio: i32, cad: u32, pin_nid: u32) {
    if get_param(mmio, cad, pin_nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_OUT_AMP != 0 {
        codec_cmd(mmio, cad, pin_nid, vset_amp(AMP_OUT_UNMUTE));
    }
    write(mmio, cad, pin_nid, AC_VERB_SET_PIN_WIDGET_CONTROL, PIN_OUT);
}

/// `hdmi_std_setup_channel_mapping` for CA 0 (FL, FR): slots 0 and 1 carry
/// channels 0 and 1, the other six none.
fn hdmi_setup_channel_mapping(mmio: i32, cad: u32, pin_nid: u32) {
    for slot in 0..8u32 {
        let channel = if slot < 2 { slot } else { 0xf };
        write(mmio, cad, pin_nid, AC_VERB_SET_HDMI_CHAN_SLOT, (channel << 4) | slot);
    }
}

fn hdmi_set_dip_index(mmio: i32, cad: u32, pin_nid: u32, packet_index: u32, byte_index: u32) {
    write(mmio, cad, pin_nid, AC_VERB_SET_HDMI_DIP_INDEX, (packet_index << 5) | (byte_index & 0x1f));
}

/// `hdmi_infoframe_uptodate`.
fn hdmi_infoframe_uptodate(mmio: i32, cad: u32, pin_nid: u32, dip: &[u8]) -> bool {
    hdmi_set_dip_index(mmio, cad, pin_nid, 0, 0);
    if read(mmio, cad, pin_nid, AC_VERB_GET_HDMI_DIP_XMIT, 0) != AC_DIPXMIT_BEST {
        return false;
    }
    dip.iter().all(|&b| read(mmio, cad, pin_nid, AC_VERB_GET_HDMI_DIP_DATA, 0) as u8 == b)
}

/// `hdmi_pin_setup_infoframe` for a stereo PCM stream (CA 0).
fn hdmi_pin_setup_infoframe(mmio: i32, cad: u32, pin_nid: u32, dev_id: u32, dp: bool) {
    let mut ai = [0u8; AUDIO_INFOFRAME_BYTES];
    let active_channels = 2u8;
    if dp {
        // struct dp_audio_infoframe
        ai[0] = 0x84;
        ai[1] = 0x1b;
        ai[2] = 0x11 << 2;
        ai[3] = active_channels - 1;
        ai[6] = 0; // CA
    } else {
        // struct hdmi_audio_infoframe, then hdmi_checksum_audio_infoframe
        ai[0] = 0x84;
        ai[1] = 0x01;
        ai[2] = 0x0a;
        ai[4] = active_channels - 1;
        ai[7] = 0; // CA
        let sum = ai.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        ai[3] = 0u8.wrapping_sub(sum);
    }

    snd_hda_set_dev_select(mmio, cad, pin_nid, dev_id);
    if hdmi_infoframe_uptodate(mmio, cad, pin_nid, &ai) {
        return;
    }
    // hdmi_stop_infoframe_trans, hdmi_fill_audio_infoframe,
    // hdmi_start_infoframe_trans
    hdmi_set_dip_index(mmio, cad, pin_nid, 0, 0);
    write(mmio, cad, pin_nid, AC_VERB_SET_HDMI_DIP_XMIT, AC_DIPXMIT_DISABLE);
    hdmi_set_dip_index(mmio, cad, pin_nid, 0, 0);
    for &b in ai.iter() {
        write(mmio, cad, pin_nid, AC_VERB_SET_HDMI_DIP_DATA, b as u32);
    }
    hdmi_set_dip_index(mmio, cad, pin_nid, 0, 0);
    write(mmio, cad, pin_nid, AC_VERB_SET_HDMI_DIP_XMIT, AC_DIPXMIT_BEST);
}

/// Set up the codec for one stereo stream to the pin a display reports
/// present: the Tiger Lake probe (all pins and converters, DP 1.2), the
/// pin, its converter (`i915_pin_cvt_fixup`), the stream
/// (`i915_hsw_setup_stream`) and the audio infoframe. Returns the
/// converter, 0 when no pin reports a display.
pub fn setup(mmio: i32, cad: u32, afg: u32, stream_tag: u32, format: u16) -> u32 {
    intel_haswell_enable_all_pins(mmio, cad);
    intel_haswell_fixup_enable_dp12(mmio, cad);

    // snd_hda_codec_update_widgets: the enable made more widgets appear.
    let w = get_param(mmio, cad, afg, PARAM_SUB_NODE_COUNT);
    let (w_start, w_count) = ((w >> 16) & 0xff, w & 0xff);

    let mut cvts = [0u32; 8];
    let num_cvts = converters(mmio, cad, w_start, w_count, &mut cvts);
    if num_cvts == 0 {
        log("[audio_hda] HDMI codec without converters\n");
        return 0;
    }

    // The display driver's presence detect names the pin; non-MST, device 0.
    let dev_id = 0;
    let mut pin_nid = 0;
    for (port, &nid) in PORT_MAP.iter().enumerate() {
        if widget_type(mmio, cad, nid) != WTYPE_PIN {
            continue;
        }
        let sense = read_pin_sense(mmio, cad, nid, dev_id);
        if sense & AC_PINSENSE_PRESENCE != 0 && pin_nid == 0 {
            pin_nid = nid;
            loghex("[audio_hda] display on port ", port as u32);
            loghex(", pin 0x", nid);
            log(if sense & AC_PINSENSE_ELDV != 0 { ", ELD valid" } else { ", no ELD" });
            log("\n");
        }
    }
    if pin_nid == 0 {
        log("[audio_hda] no display pin reports a sink\n");
        return 0;
    }

    let cvt_nid = cvts[0];
    let mux_idx = 0;
    let pincap = get_param(mmio, cad, pin_nid, PARAM_PIN_CAP);
    let dp = pincap & AC_PINCAP_DP != 0;

    hdmi_init_pin(mmio, cad, pin_nid);

    // i915_pin_cvt_fixup
    haswell_verify_d0(mmio, cad, cvt_nid, pin_nid);
    snd_hda_set_dev_select(mmio, cad, pin_nid, dev_id);
    if read(mmio, cad, pin_nid, AC_VERB_GET_CONNECT_SEL, 0) != mux_idx {
        write(mmio, cad, pin_nid, AC_VERB_SET_CONNECT_SEL, mux_idx);
    }
    intel_not_share_assigned_cvt(mmio, cad, pin_nid, mux_idx, num_cvts);

    // snd_hda_hdmi_setup_stream, intel_hsw_fixup: the IEC coding type
    // cleared (no HBR), then the stream.
    let param = (read(mmio, cad, cvt_nid, AC_VERB_GET_DIGI_CONVERT_1, 0) >> 16) & !AC_DIG3_ICT;
    write(mmio, cad, cvt_nid, AC_VERB_SET_DIGI_CONVERT_3, param);
    codec_cmd(mmio, cad, cvt_nid, vset_stream_chan(stream_tag << 4));
    codec_cmd(mmio, cad, cvt_nid, vset_format(format));
    // The IEC958 switch on (`snd_hda_spdif_out_switch_put`).
    let dig1 = read(mmio, cad, cvt_nid, AC_VERB_GET_DIGI_CONVERT_1, 0) & 0xff;
    write(mmio, cad, cvt_nid, AC_VERB_SET_DIGI_CONVERT_1, dig1 | AC_DIG1_ENABLE);

    // snd_hda_hdmi_setup_audio_infoframe: amp, channel count, mapping,
    // infoframe.
    snd_hda_set_dev_select(mmio, cad, pin_nid, dev_id);
    if get_param(mmio, cad, pin_nid, PARAM_AUDIO_WIDGET_CAP) & WCAP_OUT_AMP != 0 {
        codec_cmd(mmio, cad, pin_nid, vset_amp(AMP_OUT_UNMUTE));
    }
    write(mmio, cad, cvt_nid, AC_VERB_SET_CVT_CHAN_COUNT, 2 - 1);
    hdmi_setup_channel_mapping(mmio, cad, pin_nid);
    hdmi_pin_setup_infoframe(mmio, cad, pin_nid, dev_id, dp);

    loghex("[audio_hda] HDMI/DP: converter 0x", cvt_nid);
    loghex(" -> pin 0x", pin_nid);
    log(if dp { " (DisplayPort infoframe)" } else if pincap & AC_PINCAP_HDMI != 0 { " (HDMI infoframe)" } else { "" });
    log("\n");
    cvt_nid
}
