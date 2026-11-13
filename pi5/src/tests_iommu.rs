//! The IOMMU against Linux: `testdata/linux_iommu_d0.log` is run K of the
//! oracle, the unmodified bcm2712-iommu.c and bcm2712-iommu-cache.c on the
//! board's DEBUG_INFO, with the default aperture (40 GiB, 2 GiB, bypass
//! below). Init, five maps with their `iotlb_sync_map`, five unmaps with
//! `iotlb_sync` and a domain-wide flush must make Linux's register
//! accesses one for one and leave Linux's PTEs. Linux's tables sit in its
//! 16 KiB pages and the port's in 4 KiB ones, which changes how many
//! top-level entries one allocation fills, not the accesses or the PTEs.

use super::iommu::*;
use super::tests::{parse, Ev, Strict};
use std::collections::BTreeMap;

const LOG: &str = include_str!("../testdata/linux_iommu_d0.log");

/// Table pages handed out at the addresses Linux's run used for the ones
/// whose address reaches a register (top table, default page), then any.
struct Mem {
    next: Vec<u64>,
    pages: BTreeMap<u64, Vec<u32>>,
}

impl Pages for Mem {
    fn alloc(&mut self) -> Option<u64> {
        let p = if self.next.is_empty() {
            0x5000_0000 + self.pages.len() as u64 * 0x1000
        } else {
            self.next.remove(0)
        };
        self.pages.insert(p, vec![0; 1024]);
        Some(p)
    }
    fn free(&mut self, phys: u64) { self.pages.remove(&phys); }
    fn read(&self, phys: u64, i: usize) -> u32 { self.pages[&phys][i] }
    fn write(&mut self, phys: u64, i: usize, v: u32) { self.pages.get_mut(&phys).unwrap()[i] = v; }
    fn sync_for_cpu(&mut self, _: u64) {}
    fn sync_for_device(&mut self, _: u64) {}
}

fn hex(s: &str) -> u64 {
    u64::from_str_radix(s.trim_end_matches([',', ')']).trim_start_matches("0x"), 16).expect(s)
}

/// `key 0x...` in a note line.
fn after(line: &str, key: &str) -> u64 {
    let rest = &line[line.find(key).expect(key) + key.len()..];
    let rest = rest.trim_start_matches("0x");
    hex(rest.split(|c: char| !c.is_ascii_hexdigit()).next().unwrap())
}

#[test]
fn iommu_is_linux() {
    let evs = parse(LOG);
    let accesses: Vec<Ev> = evs.iter().filter(|e| matches!(e, Ev::Read { .. } | Ev::Write { .. })).cloned().collect();
    let notes: Vec<String> = evs.iter().filter_map(|e| if let Ev::Note(n) = e { Some(n.clone()) } else { None }).collect();
    let mut hw = Strict::new(accesses);
    let mut mem = Mem { next: vec![0x4000_0000, 0x4000_4000], pages: BTreeMap::new() };
    let mut mmu = Iommu::init(&mut hw, &mut mem, Config::LINUX_DEFAULT, true).expect("init");

    let mut i = 0;
    while i < notes.len() {
        let n = &notes[i];
        if n.starts_with("# step map") {
            let (iova, pa, size) = (after(n, "iova 0x"), after(n, "phys 0x"), after(n, "size 0x"));
            let write = n.contains("prot RW");
            mmu.map(&mut mem, iova, pa, size, write).expect(n);
            mmu.sync_range(&mut hw, iova, size);
            // Linux's PTEs and iova_to_phys after the map.
            for m in notes[i + 1..].iter().take_while(|m| !m.starts_with("# step")) {
                if let Some(rest) = m.strip_prefix("# table: pte iova ") {
                    let first = hex(rest.split_whitespace().next().unwrap());
                    let count: u64 = rest.split(" x").nth(1).unwrap().split(':').next().unwrap().trim().parse().unwrap();
                    let new = after(m, "-> ") as u32;
                    let step = if m.contains("+1 each") { 1 } else { 0 };
                    for k in 0..count {
                        let pte_iova = first + k * IOMMU_PAGE_SIZE - mmu.aperture_base;
                        let p = (pte_iova >> IOMMU_PAGE_SHIFT) as usize;
                        let tbl_phys = mmu_table(&mmu, &mem, p);
                        assert_eq!(mem.read(tbl_phys, p & 1023), new + (k as u32) * step, "{m}, page {k}");
                    }
                }
                if m.starts_with("# iommu_map ret 0, iova_to_phys(iova + ") {
                    let off = after(m, "iova + ");
                    let want = hex(m.rsplit(' ').next().unwrap());
                    assert_eq!(mmu.iova_to_phys(&mem, iova + off), want, "{m}");
                }
            }
        } else if n.starts_with("# step unmap") {
            let (iova, size) = (after(n, "iova 0x"), after(n, "size 0x"));
            assert_eq!(mmu.unmap(&mut mem, iova, size), size);
            mmu.sync_range(&mut hw, iova, size);
        } else if n.starts_with("# step f") {
            mmu.sync_all(&mut hw);
        }
        i += 1;
    }
    hw.done();
    assert_eq!(mmu.mapped_pages(), 0);
}

/// The L2 table holding IOMMU page `p`, found through the top table as
/// the hardware walks it.
fn mmu_table(mmu: &Iommu, mem: &Mem, p: usize) -> u64 {
    let _ = mmu;
    let top = 0x4000_0000;
    let e = mem.read(top, p >> 10);
    assert!(e & MMMU_PTE_VALID != 0, "no L2 table for page {p:#x}");
    ((e & 0x0FFF_FFFF) as u64) << IOMMU_PAGE_SHIFT
}

/// The port's own configuration: no bypass, the aperture at IOVA 0 and 4
/// GiB. Linux takes the same values from `aperture-base = 0` /
/// `aperture-size`; the registers say so.
#[test]
fn no_bypass_config() {
    struct Regs(BTreeMap<u64, u32>);
    impl crate::hw::Hw for Regs {
        fn read(&mut self, b: crate::hw::Block, off: u64, _: u8) -> u32 {
            if b == crate::hw::Block::Iommu && off == MMMU_DEBUG_INFO_OFFSET { 0x2080_4774 } else { *self.0.get(&off).unwrap_or(&0) }
        }
        fn write(&mut self, b: crate::hw::Block, off: u64, _: u8, v: u32) {
            if b == crate::hw::Block::Iommu { self.0.insert(off, v); }
        }
        fn delay_us(&mut self, _: u64) {}
    }
    let mut hw = Regs(BTreeMap::new());
    let mut mem = Mem { next: vec![0x4000_0000, 0x4000_4000], pages: BTreeMap::new() };
    let mut mmu = Iommu::init(&mut hw, &mut mem, Config::NO_BYPASS, true).unwrap();
    assert_eq!(hw.0[&MMMU_BYPASS_END_OFFSET], 0, "no pass-through range");
    assert_eq!(hw.0[&MMMU_ADDR_CAP_OFFSET], MMMU_ADDR_CAP_ENABLE + 15, "cap at 4 GiB");
    assert_eq!(hw.0[&MMMU_PT_PA_BASE_OFFSET], 0x40000, "top table at IOVA 0");
    assert_eq!((mmu.aperture_base, mmu.aperture_end), (0, 4 << 30));
    mmu.map(&mut mem, 0x3f40_0000, 0x3f40_0000, 0x10_0000, false).unwrap();
    assert_eq!(mmu.iova_to_phys(&mem, 0x3f40_0123), 0x3f40_0123);
    assert_eq!(mmu.iova_to_phys(&mem, 0x3f50_0000), 0, "nothing past the mapping");
    assert!(mmu.map(&mut mem, 4 << 30, 0, 0x1000, false).is_err(), "nothing past the aperture");
}
