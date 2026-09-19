//! Pruefstand: eine echte DSDT laden und berichten, was an HID-over-I2C
//! darin steht. Kein Geraet noetig — genau der Gang, den das Modul am
//! Notebook fahren wird.
//!
//!     cargo run -p i2c_hid_harness -- <DSDT.aml>

use aml_core::{Ec, Machine, Namespace, Value};

fn show(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("{n:#x} ({n})"),
        Value::Str(s) => format!("\"{s}\""),
        Value::Buffer(b) => format!("Buffer[{}] {:02x?}", b.len(), &b[..b.len().min(48)]),
        Value::Package(e) => format!("Package[{}]", e.len()),
        Value::Uninit => String::from("Uninit"),
        Value::Ref(_) => String::from("Ref"),
    }
}

/// Der Pruefstand hat keinen EC; jede Lesung meldet 0 und jede Notiz geht
/// auf die Ausgabe, damit der Gang sichtbar ist.
struct NoEc {
    verbose: bool,
}

impl Ec for NoEc {
    fn read(&mut self, _a: u8) -> u8 { 0 }
    fn write(&mut self, _a: u8, _v: u8) {}
    fn note(&mut self, s: &str) {
        if self.verbose { println!("{s}"); }
    }
    fn note_num(&mut self, s: &str, v: u64) {
        if self.verbose { println!("{s}{v}"); }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: i2c_hid_harness <DSDT.aml> [-v]");
        std::process::exit(2);
    });
    let verbose = std::env::args().any(|a| a == "-v");
    let table = std::fs::read(&path).expect("read table");

    let ns = Namespace::load(&table).expect("load DSDT");
    println!("namespace: {} nodes", ns.nodes.len());

    let mut ec = NoEc { verbose };
    let mut m = Machine::new(&ns, &mut ec);
    m.init();

    if std::env::args().any(|a| a == "-a") {
        // Jedes Geraet mit einer Kennung — die Gegenprobe, wenn die Suche
        // nichts findet: steht das Geraet ueberhaupt im Namespace, und wie
        // heisst es dort?
        let devs = aml_core::devices_with_ids(&ns);
        println!("{} devices carrying _HID/_CID", devs.len());
        for d in &devs {
            let ids = m.device_ids(d);
            println!("  {} -> {:?}", aml_core::path_str(d), ids);
        }
        println!();
    }

    // `-e <pfad>`: ein beliebiges Objekt auswerten. Ein Name gibt seinen
    // Inhalt, eine Methode wird ausgefuehrt — damit laesst sich eine Frage
    // an die Firmware stellen, statt ihre Antwort zu erraten.
    let argv: Vec<String> = std::env::args().collect();
    if let Some(i) = argv.iter().position(|a| a == "-e") {
        if let Some(want) = argv.get(i + 1) {
            match i2c_hid_core::discover::parse_acpi_path(&ns, &Vec::new(), want) {
                Some(p) => match m.value_of(&p) {
                    Ok(v) => println!("{} = {}", aml_core::path_str(&p), show(&v)),
                    Err(e) => println!("{} failed: {e}", aml_core::path_str(&p)),
                },
                None => println!("no such path: {want}"),
            }
            return;
        }
    }

    let found = i2c_hid_core::discover::find(&ns, &mut m);
    println!("\n{} HID-over-I2C device(s)\n", found.len());
    for d in &found {
        for line in i2c_hid_core::discover::report(d) {
            println!("{line}");
        }
        println!();
    }
}
