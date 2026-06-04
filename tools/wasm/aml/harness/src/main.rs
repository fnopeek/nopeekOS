//! std dev-harness: load a real DSDT, run the AML battery methods against a
//! mock EC, and print what `_BST`/`_BIF` produce. Proves the interpreter
//! executes real firmware AML before we ship it as the wasm driver.
//!
//!   cargo run -p aml_harness -- ../dev/DSDT.aml

use aml_core::{find_batteries, path_str, read_battery, Ec, Namespace};
use std::collections::HashMap;

/// Mock EC: returns realistic values at the HP Dragonfly battery offsets so the
/// percentage looks sane; everything else reads 0. On real hardware these reads
/// hit ports 0x62/0x66 instead.
struct MockEc {
    mem: HashMap<u8, u8>,
}

impl MockEc {
    fn dragonfly() -> Self {
        let mut mem = HashMap::new();
        // @0x84: ADP(bit0)=AC present, BATP(bits4-7) battery-present mask;
        // bit4 set => battery 0 present.
        mem.insert(0x84, 0x11);
        // BFC @0x8D = 6496 mAh (0x1960) full charge
        mem.insert(0x8D, 0x60);
        mem.insert(0x8E, 0x19);
        // BDC @0x89 = 7300 mAh design
        mem.insert(0x89, 0x84);
        mem.insert(0x8A, 0x1C);
        // BST @0x99 = 0x02 charging
        mem.insert(0x99, 0x02);
        // BRC @0xA1 = 5100 mAh (0x13EC) remaining
        mem.insert(0xA1, 0xEC);
        mem.insert(0xA2, 0x13);
        // BPV @0xA5 = 7700 mV
        mem.insert(0xA5, 0x14);
        mem.insert(0xA6, 0x1E);
        Self { mem }
    }
}

impl Ec for MockEc {
    fn read(&mut self, addr: u8) -> u8 {
        let v = *self.mem.get(&addr).unwrap_or(&0);
        eprintln!("  ec.read [{:#04x}] -> {:#04x}", addr, v);
        v
    }
    fn write(&mut self, addr: u8, val: u8) {
        eprintln!("  ec.write[{:#04x}] <- {:#04x}", addr, val);
        self.mem.insert(addr, val);
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "../dev/DSDT.aml".to_string());
    let table = std::fs::read(&path).expect("read DSDT.aml");
    println!("DSDT: {} bytes", table.len());

    let ns = match Namespace::load(&table) {
        Ok(ns) => ns,
        Err(e) => {
            eprintln!("LOAD ERROR: {e}");
            std::process::exit(1);
        }
    };
    println!("namespace: {} objects", ns.nodes.len());

    let bats = find_batteries(&ns);
    println!("batteries found: {}", bats.len());
    for b in &bats {
        println!("  {}", path_str(b));
    }
    if bats.is_empty() {
        eprintln!("no PNP0C0A battery device found");
        std::process::exit(1);
    }

    for b in &bats {
        println!("\n=== read_battery {} ===", path_str(b));
        let mut ec = MockEc::dragonfly();
        match read_battery(&ns, &mut ec, b) {
            Ok(info) => {
                println!(
                    "  present={} state={:#x} remaining={} mAh full={} mAh -> {}%",
                    info.present, info.state, info.remaining_mah, info.full_charge_mah, info.percent
                );
            }
            Err(e) => println!("  ERROR: {e}"),
        }
    }
}
