//! IEC958 subframes for an HDMI audio FIFO that takes
//! `SNDRV_PCM_FORMAT_IEC958_SUBFRAME_LE` (vc4's HDMI): the consumer
//! channel status of Linux `sound/core/pcm_iec958.c`
//! (`snd_pcm_create_iec958_consumer_hw_params`) and the subframe packing
//! of alsa-lib's `iec958` plugin (`src/pcm/pcm_iec958.c`,
//! `iec958_subframe` and `iec958_encode` for PCM, its default preambles).

/// `IEC958_AES0_CON_NOT_COPYRIGHT | IEC958_AES0_CON_EMPHASIS_NONE`.
const AES0_DEFAULT: u8 = 1 << 2;
/// `IEC958_AES3_CON_CLOCK_1000PPM`.
const AES3_CLOCK_1000PPM: u8 = 0;
/// `IEC958_AES4_CON_WORDLEN_24_20 | IEC958_AES4_CON_MAX_WORDLEN_24`.
const AES4_24BIT: u8 = (5 << 1) | 1;

/// The preambles alsa-lib sets by default: Z (block start), X, Y.
const PREAMBLE_Z: u32 = 0x08;
const PREAMBLE_X: u32 = 0x02;
const PREAMBLE_Y: u32 = 0x04;

/// Frames of one channel status block.
const BLOCK: u32 = 192;

/// `fill_iec958_consumer`'s sample frequency code, None for a rate it
/// does not name.
fn aes3_fs(rate: u32) -> Option<u8> {
    Some(match rate {
        32000 => 3,
        44100 => 0,
        48000 => 2,
        88200 => 8,
        96000 => 10,
        176400 => 12,
        192000 => 14,
        _ => return None,
    })
}

/// `snd_pcm_create_iec958_consumer_hw_params` for 32-bit samples (24 bit
/// used): the 24 channel status bytes, None for an unnamed rate.
pub fn consumer_status(rate: u32) -> Option<[u8; 24]> {
    let mut cs = [0u8; 24];
    cs[0] = AES0_DEFAULT;
    cs[3] = AES3_CLOCK_1000PPM | aes3_fs(rate)?;
    cs[4] = AES4_24BIT;
    Some(cs)
}

/// `iec958_parity`: odd number of ones in bits 4 to 30.
fn parity(data: u32) -> bool {
    ((data >> 4) & 0x07ff_ffff).count_ones() & 1 != 0
}

/// The encoder's state across calls: where in the 192-frame block it is.
pub struct Encoder {
    status: [u8; 24],
    counter: u32,
}

impl Encoder {
    pub fn new(status: [u8; 24]) -> Self {
        Encoder { status, counter: 0 }
    }

    /// `iec958_subframe` for a left-justified 32-bit sample.
    fn subframe(&self, sample: i32, channel: usize) -> u32 {
        let byte = (self.counter >> 3) as usize;
        let mask = 1u8 << (self.counter & 7);
        let mut data = (sample as u32 >> 4) & !0xf;
        if self.status[byte] & mask != 0 {
            data |= 0x4000_0000;
        }
        if parity(data) {
            data |= 0x8000_0000;
        }
        data | if channel != 0 {
            PREAMBLE_Y
        } else if self.counter == 0 {
            PREAMBLE_Z
        } else {
            PREAMBLE_X
        }
    }

    /// `iec958_encode` for interleaved PCM: every 32-bit sample of
    /// `frames` (`channels` per frame) replaced by its subframe in place.
    pub fn encode(&mut self, frames: &mut [u32], channels: usize) {
        if channels == 0 {
            return;
        }
        for frame in frames.chunks_exact_mut(channels) {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = self.subframe(*s as i32, ch);
            }
            self.counter = (self.counter + 1) % BLOCK;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_status_names_48_khz_and_24_bit_as_linux_fills_it() {
        let cs = consumer_status(48000).unwrap();
        assert_eq!(&cs[..5], &[0x04, 0x00, 0x00, 0x02, 0x0b]);
        assert!(cs[5..].iter().all(|&b| b == 0));
        assert!(consumer_status(22050).is_none());
    }

    #[test]
    fn subframes_carry_preamble_status_bit_and_even_parity() {
        let mut e = Encoder::new(consumer_status(48000).unwrap());
        // Frames 0..3 of silence, then a full-scale left sample at frame 3.
        let mut f = [0u32, 0, 0, 0, 0, 0, 0x7fff_ff00, 0];
        e.encode(&mut f, 2);
        // Frame 0: block start Z, then Y.
        assert_eq!(&f[..2], &[0x08, 0x04]);
        // Frame 1: X, status bit 1 of byte 0 is clear.
        assert_eq!(&f[2..4], &[0x02, 0x04]);
        // Frame 3: bit 3 of AES0 is clear; 23 data ones set the parity bit.
        assert_eq!(f[6], 0x8000_0000 | 0x07ff_fff0 | 0x02);
        // Frame 2 sits on AES0 bit 2 (not copyright): C set, so parity set.
        let mut e = Encoder::new(consumer_status(48000).unwrap());
        let mut f = [0u32; 6];
        e.encode(&mut f, 2);
        assert_eq!(&f[4..6], &[0xc000_0002, 0xc000_0004]);
    }

    #[test]
    fn the_block_counter_wraps_after_192_frames() {
        let mut e = Encoder::new([0; 24]);
        let mut f = vec![0u32; 2 * 193];
        e.encode(&mut f, 2);
        assert_eq!((f[0], f[2 * 191], f[2 * 192]), (0x08, 0x02, 0x08));
    }
}
