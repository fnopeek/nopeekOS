//! aml_core — a minimal ACPI AML interpreter, enough to evaluate the
//! Control-Method-Battery objects (`_BST`, `_BIF`, `_BIX`) on any laptop by
//! running the firmware's own AML, exactly the way ACPICA/Linux do — no
//! per-device hardcoded EC offsets.
//!
//! no_std + alloc. The same crate compiles for the std dev-harness and for the
//! wasm32 battery driver. Hardware access (EmbeddedControl region reads/writes)
//! is abstracted behind the [`Ec`] trait.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod value;
mod load;
mod interp;

pub use value::{path_str, seg, Obj, Path, Place, Seg, Value};

use alloc::{collections::BTreeMap, string::String, vec::Vec};

/// Embedded-controller access (ACPI RegionSpace 3). The only hardware the
/// battery path needs. On real hardware this drives ports 0x62/0x66; in the
/// dev-harness it is a mock.
pub trait Ec {
    fn read(&mut self, addr: u8) -> u8;
    fn write(&mut self, addr: u8, val: u8);
}

/// A namespace object.
pub enum Node {
    /// Pure container (Scope / Device / Processor / PowerRes / ThermalZone).
    Scope,
    /// A named data object with a mutable value cell.
    Name(Obj),
    /// A control method: flags + deferred body bytes + the scope it lives in.
    Method { flags: u8, body: Vec<u8>, scope: Path },
    /// OperationRegion.
    Region { space: u8, offset: u64, len: u64 },
    /// A field unit inside a region.
    Field { region: Path, bit_offset: u64, bit_width: u64 },
    /// Mutex / Event / External declaration — presence only.
    Other,
}

/// The loaded ACPI namespace.
pub struct Namespace {
    pub nodes: BTreeMap<Path, Node>,
}

impl Namespace {
    /// Parse a DSDT/SSDT table (with its 36-byte ACPI header) into a namespace.
    pub fn load(table: &[u8]) -> Result<Namespace, String> {
        load::load_table(table)
    }

    pub fn get(&self, p: &Path) -> Option<&Node> {
        self.nodes.get(p)
    }

    /// Resolve a name reference using the ACPI search rules, relative to
    /// `scope`. Single unqualified NameSegs search upward to the root.
    pub fn resolve(&self, scope: &Path, rooted: bool, carets: usize, segs: &[Seg]) -> Option<Path> {
        let mut base: Path = if rooted {
            Vec::new()
        } else {
            let mut b = scope.clone();
            for _ in 0..carets {
                b.pop();
            }
            b
        };

        if !rooted && carets == 0 && segs.len() == 1 {
            // Upward search: try scope, then each parent, down to root.
            loop {
                let mut cand = base.clone();
                cand.push(segs[0]);
                if self.nodes.contains_key(&cand) {
                    return Some(cand);
                }
                if base.is_empty() {
                    return None;
                }
                base.pop();
            }
        }

        for s in segs {
            base.push(*s);
        }
        if self.nodes.contains_key(&base) {
            Some(base)
        } else {
            None
        }
    }
}

/// Result of `_BST` + `_BIF`, decoded for the bar.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatteryInfo {
    pub present: bool,
    /// State bits from _BST[0]: bit0 = discharging, bit1 = charging.
    pub state: u32,
    pub remaining_mah: u32,
    pub full_charge_mah: u32,
    /// 0..100, computed remaining/full.
    pub percent: u8,
}

/// Evaluate `\_SB...BAT0._BST` and `_BIF`/`_BIX` for the battery device whose
/// absolute path is given (e.g. resolved by scanning for `_HID == PNP0C0A`),
/// and decode the result.
pub fn read_battery(ns: &Namespace, ec: &mut dyn Ec, bat: &Path) -> Result<BatteryInfo, String> {
    interp::read_battery(ns, ec, bat)
}

/// Find every Control-Method-Battery device (`_HID == "PNP0C0A"`).
pub fn find_batteries(ns: &Namespace) -> Vec<Path> {
    interp::find_batteries(ns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    struct MockEc(BTreeMap<u8, u8>);
    impl Ec for MockEc {
        fn read(&mut self, a: u8) -> u8 {
            *self.0.get(&a).unwrap_or(&0)
        }
        fn write(&mut self, a: u8, v: u8) {
            self.0.insert(a, v);
        }
    }

    /// Real HP Elite Dragonfly G1 DSDT + injected EC battery values; the
    /// firmware AML must compute 5100/6496 = 79% with no hardcoded offsets.
    #[test]
    fn dragonfly_battery() {
        let table = include_bytes!("../../dev/DSDT.aml");
        let ns = Namespace::load(table).expect("load");
        let bats = find_batteries(&ns);
        assert_eq!(bats.len(), 2);

        let mut m = BTreeMap::new();
        m.insert(0x84u8, 0x11u8); // ADP + BATP[0]
        m.insert(0x89, 0x84);
        m.insert(0x8A, 0x1C); // BDC 7300
        m.insert(0x8D, 0x60);
        m.insert(0x8E, 0x19); // BFC 6496
        m.insert(0x99, 0x02); // BST charging
        m.insert(0xA1, 0xEC);
        m.insert(0xA2, 0x13); // BRC 5100
        let mut ec = MockEc(m);

        let info = read_battery(&ns, &mut ec, &bats[0]).expect("read");
        assert!(info.present);
        assert_eq!(info.full_charge_mah, 6496);
        assert_eq!(info.remaining_mah, 5100);
        assert_eq!(info.percent, 79);
        assert_eq!(info.state, 0x2);

        // Second battery slot is empty on this machine.
        let info1 = read_battery(&ns, &mut ec, &bats[1]).expect("read1");
        assert!(!info1.present);
    }
}
