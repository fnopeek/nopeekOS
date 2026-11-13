//! The VideoCore firmware's property channel, after Linux
//! `drivers/mailbox/bcm2835-mailbox.c` (the register side) and
//! `drivers/firmware/raspberrypi.c` (`rpi_firmware_transaction`,
//! `rpi_firmware_property_list`, `rpi_firmware_property`): a request is a
//! buffer of tags in memory the firmware reads and answers in place; its
//! bus address goes to the firmware through mailbox 1, the answer comes
//! back through mailbox 0.

use crate::hw::{Block, Hw};

pub const ARM_0_MAIL0: u64 = 0x00;
pub const ARM_0_MAIL1: u64 = 0x20;
pub const MAIL0_RD: u64 = ARM_0_MAIL0;
pub const MAIL0_POL: u64 = ARM_0_MAIL0 + 0x10;
pub const MAIL0_STA: u64 = ARM_0_MAIL0 + 0x18;
pub const MAIL0_CNF: u64 = ARM_0_MAIL0 + 0x1C;
pub const MAIL1_WRT: u64 = ARM_0_MAIL1;
pub const MAIL1_STA: u64 = ARM_0_MAIL1 + 0x18;

pub const ARM_MS_FULL: u32 = 1 << 31;
pub const ARM_MS_EMPTY: u32 = 1 << 30;
pub const ARM_MC_IHAVEDATAIRQEN: u32 = 1 << 0;

pub const MBOX_CHAN_PROPERTY: u32 = 8;

pub const RPI_FIRMWARE_STATUS_REQUEST: u32 = 0;
pub const RPI_FIRMWARE_STATUS_SUCCESS: u32 = 0x8000_0000;
pub const RPI_FIRMWARE_PROPERTY_END: u32 = 0;

pub const RPI_FIRMWARE_GET_FIRMWARE_REVISION: u32 = 0x0000_0001;
pub const RPI_FIRMWARE_GET_FIRMWARE_VARIANT: u32 = 0x0000_0002;
pub const RPI_FIRMWARE_GET_FIRMWARE_HASH: u32 = 0x0000_0003;
pub const RPI_FIRMWARE_GET_BOARD_REVISION: u32 = 0x0001_0002;
pub const RPI_FIRMWARE_GET_CLOCK_STATE: u32 = 0x0003_0001;
pub const RPI_FIRMWARE_GET_CLOCK_RATE: u32 = 0x0003_0002;
pub const RPI_FIRMWARE_GET_MAX_CLOCK_RATE: u32 = 0x0003_0004;
pub const RPI_FIRMWARE_GET_MIN_CLOCK_RATE: u32 = 0x0003_0007;
pub const RPI_FIRMWARE_GET_THROTTLED: u32 = 0x0003_0046;
pub const RPI_FIRMWARE_GET_CLOCK_MEASURED: u32 = 0x0003_0047;
pub const RPI_FIRMWARE_NOTIFY_REBOOT: u32 = 0x0003_0048;
pub const RPI_FIRMWARE_NOTIFY_DISPLAY_DONE: u32 = 0x0003_0066;
pub const RPI_FIRMWARE_SET_CLOCK_STATE: u32 = 0x0003_8001;
pub const RPI_FIRMWARE_SET_CLOCK_RATE: u32 = 0x0003_8002;

/// `clk-raspberrypi.c`: the clock-state word's on bit.
pub const RPI_FIRMWARE_STATE_ENABLE_BIT: u32 = 1 << 0;

/// `enum rpi_firmware_clk_id`, the clocks the display uses.
pub const RPI_FIRMWARE_CORE_CLK_ID: u32 = 4;
pub const RPI_FIRMWARE_V3D_CLK_ID: u32 = 5;
pub const RPI_FIRMWARE_M2MC_CLK_ID: u32 = 13;
pub const RPI_FIRMWARE_PIXEL_BVB_CLK_ID: u32 = 14;
pub const RPI_FIRMWARE_DISP_CLK_ID: u32 = 16;

/// `MBOX_MSG`.
pub fn mbox_msg(chan: u32, data28: u32) -> u32 {
    (data28 & !0xf) | (chan & 0xf)
}

/// `bcm2835_startup`: interrupt on data for us in mailbox 0.
pub fn startup(hw: &mut impl Hw) {
    hw.w32(Block::Mbox, MAIL0_CNF, ARM_MC_IHAVEDATAIRQEN);
}

/// `bcm2835_last_tx_done`: mailbox 1 has room.
pub fn last_tx_done(hw: &mut impl Hw) -> bool {
    hw.r32(Block::Mbox, MAIL1_STA) & ARM_MS_FULL == 0
}

/// `bcm2835_send_data`.
pub fn send_data(hw: &mut impl Hw, msg: u32) {
    hw.w32(Block::Mbox, MAIL1_WRT, msg);
}

/// `bcm2835_mbox_irq`: every message waiting in mailbox 0, to `f`.
pub fn drain(hw: &mut impl Hw, mut f: impl FnMut(u32)) {
    while hw.r32(Block::Mbox, MAIL0_STA) & ARM_MS_EMPTY == 0 {
        f(hw.r32(Block::Mbox, MAIL0_RD));
    }
}

/// `rpi_firmware_property` + `rpi_firmware_property_list`'s buffer: the
/// size, the request code, one tag header (tag, value buffer size,
/// request size 0), the value, the end tag. Returns the words used, or
/// None when `buf` is too small. `value` must be a whole number of words.
pub fn build_property(buf: &mut [u32], tag: u32, value: &[u32]) -> Option<usize> {
    let tag_size = 12 + value.len() * 4;
    let size = tag_size + 12;
    let words = size / 4;
    if buf.len() < words { return None; }
    buf[0] = size as u32;
    buf[1] = RPI_FIRMWARE_STATUS_REQUEST;
    buf[2] = tag;
    buf[3] = (value.len() * 4) as u32;
    buf[4] = 0;
    buf[5..5 + value.len()].copy_from_slice(value);
    buf[words - 1] = RPI_FIRMWARE_PROPERTY_END;
    Some(words)
}

/// The answer to `build_property`'s request: the value buffer, if the
/// firmware reported success.
pub fn property_answer(buf: &[u32], value_words: usize) -> Option<&[u32]> {
    if buf.len() < 5 + value_words || buf[1] != RPI_FIRMWARE_STATUS_SUCCESS {
        return None;
    }
    Some(&buf[5..5 + value_words])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Mailbox registers as the firmware drives them: a queue of answers
    /// in mailbox 0, every access logged.
    struct Fake { replies: Vec<u32>, log: Vec<(bool, u64, u32)> }

    impl Hw for Fake {
        fn read(&mut self, _b: Block, off: u64, _w: u8) -> u32 {
            let v = match off {
                MAIL0_STA => if self.replies.is_empty() { ARM_MS_EMPTY } else { 0 },
                MAIL0_RD => self.replies.remove(0),
                MAIL1_STA => 0,
                _ => 0,
            };
            self.log.push((false, off, v));
            v
        }
        fn write(&mut self, _b: Block, off: u64, _w: u8, v: u32) { self.log.push((true, off, v)); }
        fn delay_us(&mut self, _us: u64) {}
    }

    #[test]
    fn property_buffer_is_linux_layout() {
        // rpi_firmware_clk_get_max_rate(fw, CORE): one 8-byte
        // rpi_firmware_clk_rate_request.
        let mut buf = [0xdead_beefu32; 16];
        let n = build_property(&mut buf, RPI_FIRMWARE_GET_MAX_CLOCK_RATE, &[RPI_FIRMWARE_CORE_CLK_ID, 0]).unwrap();
        assert_eq!(&buf[..n], &[32, 0, 0x0003_0004, 8, 0, 4, 0, 0]);
        assert!(property_answer(&buf, 2).is_none());
        buf[1] = RPI_FIRMWARE_STATUS_SUCCESS;
        buf[6] = 910_000_000;
        assert_eq!(property_answer(&buf, 2), Some(&[4, 910_000_000][..]));
    }

    #[test]
    fn mailbox_path_is_linux() {
        let mut hw = Fake { replies: Vec::new(), log: Vec::new() };
        startup(&mut hw);
        assert!(last_tx_done(&mut hw));
        send_data(&mut hw, mbox_msg(MBOX_CHAN_PROPERTY, 0x3f00_0000));
        hw.replies.push(0x3f00_0008);
        let mut got = Vec::new();
        drain(&mut hw, |m| got.push(m));
        assert_eq!(got, [0x3f00_0008]);
        assert_eq!(hw.log, [
            (true, MAIL0_CNF, ARM_MC_IHAVEDATAIRQEN),
            (false, MAIL1_STA, 0),
            (true, MAIL1_WRT, 0x3f00_0008),
            (false, MAIL0_STA, 0),
            (false, MAIL0_RD, 0x3f00_0008),
            (false, MAIL0_STA, ARM_MS_EMPTY),
        ]);
    }
}
