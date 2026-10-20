// The module's import of a guest frame (src/import.rs) against Mesa's
// (shaders/imgdesc.c): the same cases on stdin, the same lines out. Then
// the bounds the module adds on top, which Mesa leaves to the VM.

#![allow(dead_code, non_camel_case_types, non_upper_case_globals)]

#[path = "../src/regs.rs"]
mod regs;
#[path = "../src/import.rs"]
mod import;

use import::refusal;
use std::io::BufRead;

/// The view's address in imgdesc.c.
const VA: u64 = 0x0000_8000_0010_0000;
fn case(pci_id: u16, w: u32, h: u32, tiling: u64, md: &[u32]) {
    println!("begin import {w} {h} {tiling}");
    match import::import(pci_id, w, h, tiling, md) {
        Err(refusal::surface(e)) => println!("case refused surface {e}"),
        Err(refusal::metadata) => println!("case refused metadata"),
        Err(e) => println!("case not modelled: {e:?}"),
        Ok(s) => {
            println!("case surface swizzle {} scanout {} epitch {} surf_pitch {} surf_height {} surf_size {} \
                      surf_align_log2 {} meta_offset {} meta_size {} display_dcc_size {} meta_align_log2 {} \
                      pipe_aligned {} rb_aligned {} num_meta_levels {} tile_swizzle {}",
                     s.swizzle_mode, s.scanout as u32, s.epitch, s.surf_pitch, s.surf_height, s.surf_size,
                     s.surf_alignment_log2, s.meta_offset, s.meta_size, s.display_dcc_size, s.meta_alignment_log2,
                     s.pipe_aligned as u32, s.rb_aligned as u32, s.num_meta_levels, s.tile_swizzle);
            println!("case va {VA}");
            let d = import::descriptor(&s, w, h, VA);
            println!("case desc {}", d.map(|x| x.to_string()).join(" "));
        }
    }
    println!("end");
}

/// The bounds: a valid DCC frame as the guest sends it, then each field
/// pushed past its object.
fn bounds(pci_id: u16) -> bool {
    let word1 = import::ac_get_umd_metadata_word1(pci_id);
    let md = |meta: u64, pipe: u32, rb: u32| -> Vec<u32> {
        let mut m = vec![1, word1, 0, 10485760, 1090110382, 2443185966, 2155864064, 0, 0, 0];
        m[2 + 6] |= 1 << 21;
        m[2 + 7] = (meta >> 8) as u32;
        m[2 + 5] |= (((meta >> 40) as u32 & 0xff) << 17) | pipe << 26 | rb << 27;
        m
    };
    // The guest's running frames: 943x1000 64KB_D_X, DCC at 4.5 MiB, an
    // object of 1168 pages.
    let tiling = 0x0000_087F_E009_001A;
    let obj = 1168 * 4096;
    let cases: [(&str, u32, u32, Vec<u32>, u64, Result<(), refusal>); 8] = [
        ("measured frame", 943, 1000, md(4718592, 1, 1), obj, Ok(())),
        ("display DCC", 943, 1000, md(4718592, 0, 0), obj, Ok(())),
        ("DCC one page short", 943, 1000, md(4718592, 1, 1), obj - 4096, Err(refusal::bounds("DCC"))),
        ("DCC past 2^40", 943, 1000, md(1 << 40, 1, 1), obj, Err(refusal::bounds("DCC"))),
        ("DCC inside the image", 943, 1000, md(65536, 1, 1), obj, Err(refusal::bounds("DCC"))),
        ("DCC unaligned", 943, 1000, md(4718592 + 256, 1, 1), obj + 65536, Err(refusal::bounds("DCC"))),
        ("mixed alignment", 943, 1000, md(4718592, 1, 0), obj, Err(refusal::unmodelled("mixed DCC alignment"))),
        ("image past the object", 1920, 1080, md(4718592, 1, 1), obj, Err(refusal::bounds("image"))),
    ];
    let mut ok = true;
    for (what, w, h, m, size, want) in cases {
        let got = import::import(pci_id, w, h, tiling, &m).and_then(|s| import::check(&s, size));
        if got != want {
            println!("bounds: {what}: {got:?}, expected {want:?}");
            ok = false;
        }
    }
    ok
}

/// `import_check <pci id> [bounds]`: the device's PCI id, as Mesa's
/// metadata word 1 carries it.
fn main() {
    let pci_id = u16::from_str_radix(std::env::args().nth(1).unwrap().trim_start_matches("0x"), 16).unwrap();
    if std::env::args().nth(2).as_deref() == Some("bounds") {
        let ok = bounds(pci_id);
        println!("import bounds: {}", if ok { "every case as expected" } else { "FAILED" });
        std::process::exit(!ok as i32);
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.first() != Some(&"import") {
            continue;
        }
        let n: usize = p[4].parse().unwrap();
        let md: Vec<u32> = p[5..5 + n].iter().map(|x| x.parse::<i64>().unwrap() as u32).collect();
        case(pci_id, p[1].parse().unwrap(), p[2].parse().unwrap(), p[3].parse().unwrap(), &md);
    }
}
