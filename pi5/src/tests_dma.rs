//! `bcm2835_dma` against the control blocks and registers Linux derives
//! for HDMI0's audio channel on the Pi 5 (`bcm2712-rpi.dtsi`).

use crate::bcm2835_dma::*;
use crate::hw::{Block, Hw};
use std::collections::BTreeMap;

/// `&hdmi0 dmas = <&dma40 (10|(1<<30)|(1<<24)|(10<<16)|(15<<20))>`.
const HDMI0_CELL: u32 = 10 | (1 << 30) | (1 << 24) | (10 << 16) | (15 << 20);

#[derive(Default)]
struct Regs {
    vals: BTreeMap<u64, u32>,
    writes: Vec<(Block, u64, u32)>,
}

impl Hw for Regs {
    fn read(&mut self, _: Block, off: u64, _: u8) -> u32 {
        *self.vals.get(&off).unwrap_or(&0)
    }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        self.writes.push((b, off, v));
        self.vals.insert(off, v);
    }
    fn delay_us(&mut self, _: u64) {}
}

#[test]
fn dma40_offers_channels_6_to_10_from_its_rebased_origin() {
    // bcm2712-ds.dtsi: reg <0x10 0x00010600 0 0x600>; bcm2712-rpi.dtsi
    // masks channel 11 for the VPU.
    let (origin, chans) = channels(0x10_0001_0600, 0x600, 0x07c0);
    assert_eq!(origin, 0x10_0001_0000);
    assert_eq!(chans.collect::<Vec<_>>(), vec![6, 7, 8, 9, 10]);
    // dma32's channels are not 40-bit ones.
    let (_, chans) = channels(0x10_0001_0000, 0x600, 0x003f);
    assert_eq!(chans.count(), 0);
}

#[test]
fn a_cyclic_hdmi_transfer_is_one_interrupting_block_per_period_in_a_ring() {
    let c = Chan::new(6, HDMI0_CELL);
    let mut cbs = [[0u32; 8]; 4];
    let cb_bus = 0x1_2345_6000;
    let buf = 0x1_0000_0000;
    let fifo = 0x10_7c72_0020;
    let d = c.prep_cyclic(&mut cbs, cb_bus, buf, 4 * 4096, 4096, fifo).unwrap();
    assert_eq!(d.frames, 4);
    // TI: INTEN | WAIT_RESP | PER_MAP(10) | D_DREQ.
    let ti = 1 | (1 << 2) | (10 << 9) | (1 << 15);
    // SRCI: high address bits | INC | SIZE_128 (wide source) | BURST_LEN(3).
    let srci = 0x1 | (1 << 12) | (2 << 13) | (3 << 8);
    // DSTI: high address bits | BURST_LEN(3); no increment, 32-bit writes.
    let dsti = 0x10 | (3 << 8);
    for (i, cb) in cbs.iter().enumerate() {
        let next = if i == 3 { cb_bus } else { cb_bus + 32 * (i as u64 + 1) };
        assert_eq!(*cb, [ti, (i as u32) * 4096, srci, 0x7c72_0020, dsti, 4096, (next >> 5) as u32, 0]);
    }
}

#[test]
fn a_misaligned_control_block_or_too_little_room_prepares_nothing() {
    let c = Chan::new(6, HDMI0_CELL);
    let mut cbs = [[0u32; 8]; 2];
    assert!(c.prep_cyclic(&mut cbs, 0x1010, 0, 8192, 4096, 0).is_none());
    assert!(c.prep_cyclic(&mut cbs, 0x1000, 0, 3 * 4096, 4096, 0).is_none());
    // A buffer that is not a multiple of the period ends in a short block.
    let d = c.prep_cyclic(&mut cbs, 0x1000, 0, 4096 + 100, 4096, 0).unwrap();
    assert_eq!((d.frames, cbs[1][5]), (2, 100));
}

#[test]
fn start_acknowledge_and_abort_write_what_linux_writes() {
    let c = Chan::new(6, HDMI0_CELL);
    let mut cbs = [[0u32; 8]; 2];
    let d = c.prep_cyclic(&mut cbs, 0x2000, 0x8000, 8192, 4096, 0x10_7c72_0020).unwrap();
    let mut r = Regs::default();
    c.start(&mut r, &d);
    // CB = addr >> 5, then CS = ACTIVE | PROT | QOS(10) | PANIC_QOS(15).
    let cs = 1 | (3 << 8) | (10 << 16) | (15 << 20);
    assert_eq!(r.writes, vec![(Block::Dma, 0x604, 0x2000 >> 5), (Block::Dma, 0x600, cs)]);

    r.writes.clear();
    r.vals.insert(0x600, cs | (1 << 2));
    assert_eq!(c.irq(&mut r), Ok(true));
    assert_eq!(r.writes, vec![(Block::Dma, 0x600, cs | (1 << 2))]);

    // The source halfway into the second period: 2048 bytes left.
    r.vals.insert(0x614, 0x8000 + 4096 + 2048);
    assert_eq!(c.residue(&mut r, &d), 2048);

    r.writes.clear();
    assert!(c.abort(&mut r));
    let w: Vec<(u64, u32)> = r.writes.iter().map(|w| (w.1, w.2)).collect();
    // Only ACTIVE is cleared for the pause; the model still holds INT.
    assert_eq!(w, vec![(0x600, (cs | (1 << 2)) & !1), (0x600, 3 << 8), (0x60c, 1 << 23)]);
}
