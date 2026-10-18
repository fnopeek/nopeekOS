//! The VRAM manager against Linux: reads the requests oracle/main_vram.c
//! reads (stdin) and prints the port's trace of them.

extern crate alloc;

include!("modules.rs");

use std::io::BufRead;

fn field<'a>(words: &[&'a str], key: &str) -> &'a str {
    let i = words.iter().position(|w| *w == key).unwrap();
    words[i + 1]
}

fn num(s: &str) -> u64 {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).unwrap(),
        None => s.parse().unwrap(),
    }
}

fn main() {
    let mc_vram_size = num(&std::env::args().nth(1).unwrap());
    let visible_vram_size = num(&std::env::args().nth(2).unwrap());
    let mut mgr = amdgpu_vram_mgr::amdgpu_vram_mgr::init(mc_vram_size).unwrap();
    mgr.trace = true;
    let mut live: std::collections::HashMap<u32, ttm::ttm_resource> = Default::default();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let w: Vec<&str> = line.split_whitespace().collect();
        if line.starts_with("vram new") {
            let range: Vec<&str> = field(&w, "place").split('-').collect();
            let place = ttm::ttm_place {
                fpfn: num(range[0]) as u32,
                lpfn: num(range[1]) as u32,
                mem_type: regs::TTM_PL_VRAM,
                flags: num(w[w.iter().position(|x| *x == "place").unwrap() + 2]) as u32,
            };
            let r = amdgpu_vram_mgr::vram_request {
                size: num(w[2]) << 12,
                page_alignment: num(field(&w, "align")) as u32,
                bo_type: num(field(&w, "type")) as u32,
                bo_flags: num(field(&w, "flags")),
                mc_vram_size,
                visible_vram_size,
            };
            if let Ok(res) = mgr.new(&r, &place) {
                if let ttm::ttm_resource_priv::vram { id, .. } = &res.priv_ {
                    live.insert(*id, res.clone());
                }
            }
        } else if line.starts_with("vram del") {
            let id = num(field(&w, "id")) as u32;
            match live.remove(&id) {
                Some(mut res) => mgr.del(&mut res, visible_vram_size),
                None => println!("vram del id {} none", id),
            }
        }
    }
}
