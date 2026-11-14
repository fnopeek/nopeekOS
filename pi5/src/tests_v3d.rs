//! The kernel's share of V3D against Linux's arithmetic and sequences.

use crate::hw::{Block, Hw};
use crate::v3d::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Regs {
    vals: BTreeMap<(String, u64), u32>,
    writes: Vec<(Block, u64, u32)>,
}

impl Hw for Regs {
    fn read(&mut self, b: Block, off: u64, _: u8) -> u32 {
        *self.vals.get(&(format!("{b:?}"), off)).unwrap_or(&0)
    }
    fn write(&mut self, b: Block, off: u64, _: u8, v: u32) {
        self.writes.push((b, off, v));
        // PM_GRAFX powers up at once and repairs its memory.
        let v = if b == Block::Pm && off == 0x10c { v | if v & 1 != 0 { 2 } else { 0 } | if v & 8 != 0 { 16 } else { 0 } } else { v };
        self.vals.insert((format!("{b:?}"), off), v & !0x5a00_0000);
    }
    fn delay_us(&mut self, _: u64) {}
}

#[test]
fn pages_take_the_largest_size_alignment_allows() {
    let mut pt = vec![0u32; 1024];
    // 1 MiB + 64 KiB + 4 KiB from a 1 MiB-aligned frame at a 1 MiB-aligned page.
    assert!(insert_ptes(&mut pt, 256, &[(0x4000_0000, (1 << 20) + (64 << 10) + 4096)]));
    let pfn = 0x4000_0000u32 >> 12;
    assert_eq!(pt[256], (1 << 31) | (1 << 29) | (1 << 28) | pfn);
    assert_eq!(pt[256 + 255], (1 << 31) | (1 << 29) | (1 << 28) | (pfn + 255));
    assert_eq!(pt[512], (1 << 30) | (1 << 29) | (1 << 28) | (pfn + 256));
    assert_eq!(pt[528], (1 << 29) | (1 << 28) | (pfn + 272));
    assert_eq!(pt[529], 0);
    // A frame at a page that is not 64 KiB aligned: 4 KiB pages.
    let mut pt = vec![0u32; 64];
    assert!(insert_ptes(&mut pt, 1, &[(0x1_0000, 64 << 10)]));
    assert!(pt[1..17].iter().all(|p| p & (3 << 30) == 0 && p & (1 << 28) != 0));
    remove_ptes(&mut pt, 1, 16);
    assert!(pt.iter().all(|&p| p == 0));
    // Beyond 24 PFN bits: refused.
    assert!(!insert_ptes(&mut pt, 1, &[(1 << 36, 4096)]));
}

#[test]
fn addresses_are_128k_aligned_and_come_back() {
    let mut va = Va::new(1, 1 << 20);
    let a = va.alloc(3).unwrap();
    assert_eq!(a, GMP_GRANULARITY_PAGES);
    let b = va.alloc(40).unwrap();
    assert_eq!(b % GMP_GRANULARITY_PAGES, 0);
    assert!(b >= a + 3);
    va.free(a, 3);
    assert_eq!(va.alloc(2), Some(a));
    va.free(a, 2);
    va.free(b, 40);
    assert_eq!(va.alloc((1 << 20) - GMP_GRANULARITY_PAGES), Some(GMP_GRANULARITY_PAGES));
}

#[test]
fn grafx_powers_up_as_linux_does() {
    let mut r = Regs::default();
    grafx_power_on(&mut r).unwrap();
    let pm: Vec<u32> = r.writes.iter().filter(|w| w.0 == Block::Pm).map(|w| w.2).collect();
    // POWUP at the lowest inrush first, every write with the password.
    assert_eq!(pm[0], 0x5a00_0000 | 1);
    assert!(pm.iter().all(|v| v & 0xff00_0000 == 0x5a00_0000));
    // ISPOW, MEMREP, ISFUNC in that order last.
    let n = pm.len();
    assert!(pm[n - 3] & 4 != 0 && pm[n - 2] & 8 != 0 && pm[n - 1] & 32 != 0);
    // Already powered by the firmware: nothing written.
    let mut r = Regs::default();
    r.vals.insert(("Pm".into(), 0x10c), 1);
    grafx_power_on(&mut r).unwrap();
    assert!(r.writes.is_empty());
    v3d_domain_on(&mut r);
    assert_eq!(r.writes, vec![(Block::Pm, 0x304, 0x5a00_0000 | (1 << 6))]);
}

#[test]
fn the_page_table_goes_in_as_linux_programs_it() {
    let mut r = Regs::default();
    mmu_set_page_table(&mut r, 0x1_2340_0000, 0x5000).unwrap();
    let w: Vec<(u64, u32)> = r.writes.iter().map(|w| (w.1, w.2)).collect();
    assert_eq!(w[0], (0x1204, 0x123400));
    // ENABLE, PT_INVALID_{ENABLE,INT,ABORT}, WRITE_VIOLATION_{INT,ABORT}, CAP_EXCEEDED_{INT,ABORT}.
    assert_eq!(w[1], (0x1200, 0x060d_0c01));
    assert_eq!(w[2], (0x1230, 5 | (1 << 31)));
    assert_eq!(w[3], (0x1000, 1));
    assert_eq!(w[4], (0x1000, 3));
}

#[test]
fn grafx_and_v3d_go_down_as_linux_takes_them_down() {
    let mut r = Regs::default();
    r.vals.insert(("Pm".into(), 0x10c), 1 | 2 | 4 | 32);
    r.vals.insert(("Pm".into(), 0x304), 1 << 6);
    v3d_domain_off(&mut r);
    grafx_power_off(&mut r);
    let pm: Vec<(u64, u32)> = r.writes.iter().filter(|w| w.0 == Block::Pm).map(|w| (w.1, w.2)).collect();
    // bcm2835_asb_power_off: the V3D reset asserted.
    assert_eq!(pm[0], (0x304, 0x5a00_0000));
    // bcm2835_power_power_off: ISFUNC, then ISPOW, then POWUP cleared.
    assert_eq!(pm[1].1 & 32, 0);
    assert!(pm[1].1 & 4 != 0 && pm[1].1 & 1 != 0);
    assert_eq!(pm[2].1 & (4 | 32), 0);
    assert!(pm[2].1 & 1 != 0);
    assert_eq!(pm[3].1 & (1 | 4 | 32), 0);
    // bcm2835_reset_reset: off, then on.
    let mut r = Regs::default();
    r.vals.insert(("Pm".into(), 0x304), 1 << 6);
    reset_v3d_domain(&mut r);
    let pm: Vec<u32> = r.writes.iter().map(|w| w.2).collect();
    assert_eq!(pm, vec![0x5a00_0000, 0x5a00_0000 | (1 << 6)]);
}

#[test]
fn the_sms_powers_off_and_the_gmp_idles_as_linux_waits_for_them() {
    let mut r = Regs::default();
    // The model keeps what was written: POWER_OFF (bit 30) reads back with
    // state 0, not POWER_OFF_STATE.
    assert_eq!(suspend_sms(&mut r), Err(Error::SmsOff));
    assert_eq!(r.writes[0], (Block::V3dSms, 0x400, 1 << 30));
    let mut r = Regs::default();
    r.vals.insert(("V3dSms".into(), 0x400), 0xd);
    r.writes.clear();
    // A register that reads POWER_OFF_STATE after the write.
    struct Sms(Regs);
    impl Hw for Sms {
        fn read(&mut self, b: Block, off: u64, w: u8) -> u32 {
            if b == Block::V3dSms && off == 0x400 { 0xd } else { self.0.read(b, off, w) }
        }
        fn write(&mut self, b: Block, off: u64, w: u8, v: u32) { self.0.write(b, off, w, v) }
        fn delay_us(&mut self, _: u64) {}
    }
    let mut s = Sms(r);
    assert_eq!(suspend_sms(&mut s), Ok(()));
    // idle_axi: STOP_REQ at GMP_CFG, then GMP_STATUS read until idle.
    let mut r = Regs::default();
    assert!(idle_axi(&mut r));
    assert_eq!(r.writes, vec![(Block::V3dHub, 0x604, 1 << 1)]);
    gmp_resume(&mut r);
    assert_eq!(r.writes[1], (Block::V3dHub, 0x604, 0));
}

#[test]
fn an_mmu_fault_is_read_and_cleared_as_v3d_hub_irq_does() {
    let mut r = Regs::default();
    r.vals.insert(("V3dHub".into(), 0x122c), 0x3a);
    r.vals.insert(("V3dHub".into(), 0x1234), 0x0012_3456);
    r.vals.insert(("V3dHub".into(), 0x1200), 0x060d_0c01 | (1 << 12));
    let f = mmu_fault(&mut r, 36);
    assert_eq!(f, MmuFault { axi_id: 0x3a, addr: 0x0012_3456 << 4 });
    assert_eq!(r.writes, vec![(Block::V3dHub, 0x1200, 0x060d_0c01 | (1 << 12))]);
}
