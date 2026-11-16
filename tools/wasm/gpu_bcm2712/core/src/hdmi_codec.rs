//! `sound/soc/codecs/hdmi-codec.c` behind
//! `drivers/gpu/drm/display/drm_hdmi_audio_helper.c` (Linux rpi-6.18):
//! one HDMI port's playback stream as the codec drives the connector's
//! audio funcs: startup, prepare with the stream's CEA audio infoframe,
//! shutdown. vc4 registers I2S playback only (no S/PDIF, no capture).
//! Not ported: the ALSA controls (ELD, channel map, IEC958) and the jack,
//! and `snd_pcm_hw_constraint_eld`: the audio side chooses a format the
//! ELD allows.

use drm_core::drm_connector::{drm_connector, MAX_ELD_BYTES};
use drm_core::drm_edid::drm_eld_get_spk_alloc;
use drm_core::hdmi::{hdmi_audio_infoframe, hdmi_audio_infoframe_init};
use drm_core::regs::{HDMI_AUDIO_CODING_TYPE_STREAM, HDMI_AUDIO_SAMPLE_FREQUENCY_STREAM, HDMI_AUDIO_SAMPLE_SIZE_STREAM};

use crate::hw::Hw;
use crate::linux::EINVAL;
use crate::vc4_hdmi::Vc4Hdmi;
use crate::vc4_hvs::Hvs;

/// `struct hdmi_codec_params`; of `iec`, only `status[0]` (the driver
/// reads `IEC958_AES0_NONAUDIO` alone).
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct hdmi_codec_params {
    pub cea: hdmi_audio_infoframe,
    pub iec_status0: u8,
    pub sample_rate: u32,
    pub sample_width: u32,
    pub channels: u32,
}

/// `IEC958_AES0_NONAUDIO` (include/sound/asoundef.h).
const IEC958_AES0_NONAUDIO: u8 = 1 << 1;
/// `IEC958_AES0_CON_NOT_COPYRIGHT | IEC958_AES0_CON_EMPHASIS_NONE`: byte 0
/// of `snd_pcm_create_iec958_consumer_default`.
const IEC958_CONSUMER_DEFAULT_0: u8 = 1 << 2;

/// `HDMI_CODEC_CHMAP_IDX_UNKNOWN`.
const HDMI_CODEC_CHMAP_IDX_UNKNOWN: u32 = 0xff;

// enum hdmi_codec_cea_spk_placement: CEA speaker placement for HDMI 1.4.
const FL: u32 = 1 << 0;
const FC: u32 = 1 << 1;
const FR: u32 = 1 << 2;
const FLC: u32 = 1 << 3;
const FRC: u32 = 1 << 4;
const RL: u32 = 1 << 5;
const RC: u32 = 1 << 6;
const RR: u32 = 1 << 7;
const RLC: u32 = 1 << 8;
const RRC: u32 = 1 << 9;
const LFE: u32 = 1 << 10;

/// `struct hdmi_codec_cea_spk_alloc`: `(ca_id, n_ch, mask)`.
type SpkAlloc = (u8, u32, u32);

/// `hdmi_codec_channel_alloc`: the speaker configurations, earlier ones
/// preferred.
const HDMI_CODEC_CHANNEL_ALLOC: [SpkAlloc; 43] = [
    (0x00, 2, FL | FR),
    (0x03, 4, FL | FR | LFE | FC),
    (0x02, 4, FL | FR | FC),
    (0x01, 4, FL | FR | LFE),
    (0x0b, 6, FL | FR | LFE | FC | RL | RR),
    (0x0a, 6, FL | FR | FC | RL | RR),
    (0x09, 6, FL | FR | LFE | RL | RR),
    (0x08, 6, FL | FR | RL | RR),
    (0x07, 6, FL | FR | LFE | FC | RC),
    (0x06, 6, FL | FR | FC | RC),
    (0x05, 6, FL | FR | LFE | RC),
    (0x04, 6, FL | FR | RC),
    (0x13, 8, FL | FR | LFE | FC | RL | RR | RLC | RRC),
    (0x1f, 8, FL | FR | LFE | FC | RL | RR | FLC | FRC),
    (0x12, 8, FL | FR | FC | RL | RR | RLC | RRC),
    (0x1e, 8, FL | FR | FC | RL | RR | FLC | FRC),
    (0x11, 8, FL | FR | LFE | RL | RR | RLC | RRC),
    (0x1d, 8, FL | FR | LFE | RL | RR | FLC | FRC),
    (0x10, 8, FL | FR | RL | RR | RLC | RRC),
    (0x1c, 8, FL | FR | RL | RR | FLC | FRC),
    (0x0f, 8, FL | FR | LFE | FC | RL | RR | RC),
    (0x1b, 8, FL | FR | LFE | RC | FC | FLC | FRC),
    (0x0e, 8, FL | FR | FC | RL | RR | RC),
    (0x1a, 8, FL | FR | RC | FC | FLC | FRC),
    (0x0d, 8, FL | FR | LFE | RL | RR | RC),
    (0x19, 8, FL | FR | LFE | RC | FLC | FRC),
    (0x0c, 8, FL | FR | RC | RL | RR),
    (0x18, 8, FL | FR | RC | FLC | FRC),
    (0x17, 8, FL | FR | LFE | FC | FLC | FRC),
    (0x16, 8, FL | FR | FC | FLC | FRC),
    (0x15, 8, FL | FR | LFE | FLC | FRC),
    (0x14, 8, FL | FR | FLC | FRC),
    (0x0b, 8, FL | FR | LFE | FC | RL | RR),
    (0x0a, 8, FL | FR | FC | RL | RR),
    (0x09, 8, FL | FR | LFE | RL | RR),
    (0x08, 8, FL | FR | RL | RR),
    (0x07, 8, FL | FR | LFE | FC | RC),
    (0x06, 8, FL | FR | FC | RC),
    (0x05, 8, FL | FR | LFE | RC),
    (0x04, 8, FL | FR | RC),
    (0x03, 8, FL | FR | LFE | FC),
    (0x02, 8, FL | FR | FC),
    (0x01, 8, FL | FR | LFE),
];

/// `hdmi_codec_spk_mask_from_alloc`.
fn hdmi_codec_spk_mask_from_alloc(spk_alloc: u8) -> u32 {
    const HDMI_CODEC_ELD_SPK_ALLOC_BITS: [u32; 7] = [FL | FR, LFE, FC, RL | RR, RC, FLC | FRC, RLC | RRC];
    let mut spk_mask = 0;

    for (i, bits) in HDMI_CODEC_ELD_SPK_ALLOC_BITS.iter().enumerate() {
        if spk_alloc & (1 << i) != 0 {
            spk_mask |= bits;
        }
    }

    spk_mask
}

/// `struct hdmi_codec_priv`, the fields playback uses.
pub struct HdmiCodec {
    eld: [u8; MAX_ELD_BYTES],
    chmap_idx: u32,
    busy: bool,
    /// `iec_status[0]`, as `hdmi_codec_probe` creates it.
    iec_status0: u8,
}

impl Default for HdmiCodec {
    fn default() -> Self {
        HdmiCodec {
            eld: [0; MAX_ELD_BYTES],
            chmap_idx: HDMI_CODEC_CHMAP_IDX_UNKNOWN,
            busy: false,
            iec_status0: IEC958_CONSUMER_DEFAULT_0,
        }
    }
}

impl HdmiCodec {
    /// `hdmi_codec_get_ch_alloc_table_idx`.
    fn get_ch_alloc_table_idx(&self, channels: u32) -> Result<usize, i32> {
        // Index 0 is CA 0x00, which is valid for any sink
        if channels <= 2 {
            return Ok(0);
        }

        let spk_alloc = drm_eld_get_spk_alloc(&self.eld);
        let spk_mask = hdmi_codec_spk_mask_from_alloc(spk_alloc);

        for (i, &(ca_id, n_ch, mask)) in HDMI_CODEC_CHANNEL_ALLOC.iter().enumerate() {
            // If spk_alloc == 0, HDMI is unplugged return stereo config
            if spk_alloc == 0 && ca_id == 0 {
                return Ok(i);
            }
            if n_ch != channels {
                continue;
            }
            if mask != spk_mask & mask {
                continue;
            }
            return Ok(i);
        }

        Err(-EINVAL)
    }

    /// `hdmi_codec_fill_codec_params`.
    fn fill_codec_params(&mut self, sample_width: u32, sample_rate: u32, channels: u32)
        -> Result<hdmi_codec_params, i32>
    {
        let mut ca_id = 0;
        let pcm_audio = self.iec_status0 & IEC958_AES0_NONAUDIO == 0;

        if pcm_audio {
            // Select a channel allocation that matches with ELD and pcm
            // channels
            match self.get_ch_alloc_table_idx(channels) {
                Ok(idx) => ca_id = HDMI_CODEC_CHANNEL_ALLOC[idx].0,
                Err(e) => {
                    drm_core::linux::Line::new()
                        .s(&alloc::format!("Not able to map channels to speakers ({})", e)).say();
                    self.chmap_idx = HDMI_CODEC_CHMAP_IDX_UNKNOWN;
                    return Err(e);
                }
            }
        }

        let mut hp = hdmi_codec_params::default();

        hdmi_audio_infoframe_init(&mut hp.cea);

        hp.cea.channels = if pcm_audio { channels as u8 } else { 0 };

        hp.cea.coding_type = HDMI_AUDIO_CODING_TYPE_STREAM;
        hp.cea.sample_size = HDMI_AUDIO_SAMPLE_SIZE_STREAM;
        hp.cea.sample_frequency = HDMI_AUDIO_SAMPLE_FREQUENCY_STREAM;
        hp.cea.channel_allocation = ca_id;

        hp.sample_width = sample_width;
        hp.sample_rate = sample_rate;
        hp.channels = channels;

        self.chmap_idx = if pcm_audio { ca_id as u32 } else { HDMI_CODEC_CHMAP_IDX_UNKNOWN };

        Ok(hp)
    }

    /// `hdmi_codec_startup` for playback: the connector's `startup`, then
    /// its ELD (`get_eld`).
    pub fn startup(&mut self, hdmi: &mut Vc4Hdmi, hw: &mut impl Hw, connector: &drm_connector) -> i32 {
        if self.busy {
            drm_core::linux::Line::new().s("Only one simultaneous stream supported!").say();
            return -EINVAL;
        }

        let ret = hdmi.audio_startup(hw, connector);
        if ret != 0 {
            return ret;
        }

        // drm_connector_hdmi_audio_get_eld
        self.eld.copy_from_slice(&connector.eld);

        self.busy = true;

        0
    }

    /// `hdmi_codec_prepare`: the stream's parameters, then the
    /// connector's `prepare`. `snd_pcm_fill_iec958_consumer` sets only
    /// bytes 3 and 4 of the channel status, which the driver does not read.
    pub fn prepare(&mut self, hdmi: &mut Vc4Hdmi, hw: &mut impl Hw, connector: &mut drm_connector, hvs: &Hvs,
        sample_width: u32, rate: u32, channels: u32) -> i32
    {
        let mut hp = match self.fill_codec_params(sample_width, rate, channels) {
            Ok(hp) => hp,
            Err(e) => return e,
        };

        hp.iec_status0 = self.iec_status0;

        hdmi.audio_prepare(hw, connector, hvs, &hp)
    }

    /// `hdmi_codec_shutdown` for playback.
    pub fn shutdown(&mut self, hdmi: &mut Vc4Hdmi, hw: &mut impl Hw) {
        self.chmap_idx = HDMI_CODEC_CHMAP_IDX_UNKNOWN;
        hdmi.audio_shutdown(hw);

        self.busy = false;
    }
}
