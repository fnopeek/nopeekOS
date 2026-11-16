//! A flattened device tree writer (Devicetree Specification v0.4, ch. 5),
//! as libfdt's sequential-write API (`fdt_begin_node`, `fdt_property`,
//! `fdt_end_node`, `fdt_finish`) builds one.

use alloc::vec::Vec;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_END: u32 = 9;
const HEADER: usize = 40;
const RSVMAP: usize = 16;

pub struct FdtWriter {
    structs: Vec<u8>,
    strings: Vec<u8>,
    depth: usize,
}

impl FdtWriter {
    pub fn new() -> Self {
        Self { structs: Vec::new(), strings: Vec::new(), depth: 0 }
    }

    fn u32(&mut self, v: u32) {
        self.structs.extend_from_slice(&v.to_be_bytes());
    }

    fn pad(&mut self) {
        while self.structs.len() % 4 != 0 {
            self.structs.push(0);
        }
    }

    fn string_off(&mut self, name: &str) -> u32 {
        let b = name.as_bytes();
        let mut i = 0;
        while i < self.strings.len() {
            let end = self.strings[i..].iter().position(|&c| c == 0).map_or(self.strings.len(), |p| i + p);
            if &self.strings[i..end] == b {
                return i as u32;
            }
            i = end + 1;
        }
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(b);
        self.strings.push(0);
        off
    }

    pub fn begin(&mut self, name: &str) {
        self.u32(FDT_BEGIN_NODE);
        self.structs.extend_from_slice(name.as_bytes());
        self.structs.push(0);
        self.pad();
        self.depth += 1;
    }

    pub fn end(&mut self) {
        self.u32(FDT_END_NODE);
        self.depth -= 1;
    }

    pub fn prop(&mut self, name: &str, data: &[u8]) {
        let off = self.string_off(name);
        self.u32(FDT_PROP);
        self.u32(data.len() as u32);
        self.u32(off);
        self.structs.extend_from_slice(data);
        self.pad();
    }

    pub fn prop_empty(&mut self, name: &str) {
        self.prop(name, &[]);
    }

    pub fn prop_u32(&mut self, name: &str, v: u32) {
        self.prop(name, &v.to_be_bytes());
    }

    pub fn prop_cells(&mut self, name: &str, cells: &[u32]) {
        let mut d = Vec::with_capacity(cells.len() * 4);
        for c in cells {
            d.extend_from_slice(&c.to_be_bytes());
        }
        self.prop(name, &d);
    }

    pub fn prop_u64s(&mut self, name: &str, vals: &[u64]) {
        let mut d = Vec::with_capacity(vals.len() * 8);
        for v in vals {
            d.extend_from_slice(&v.to_be_bytes());
        }
        self.prop(name, &d);
    }

    pub fn prop_str(&mut self, name: &str, s: &str) {
        let mut d = Vec::with_capacity(s.len() + 1);
        d.extend_from_slice(s.as_bytes());
        d.push(0);
        self.prop(name, &d);
    }

    /// A string list: each entry NUL-terminated.
    pub fn prop_strs(&mut self, name: &str, list: &[&str]) {
        let mut d = Vec::new();
        for s in list {
            d.extend_from_slice(s.as_bytes());
            d.push(0);
        }
        self.prop(name, &d);
    }

    /// The finished blob, with an empty reservation map.
    pub fn finish(mut self) -> Vec<u8> {
        debug_assert_eq!(self.depth, 0);
        self.u32(FDT_END);
        let off_struct = HEADER + RSVMAP;
        let off_strings = off_struct + self.structs.len();
        let total = off_strings + self.strings.len();
        let mut out = Vec::with_capacity(total);
        for v in [
            FDT_MAGIC,
            total as u32,
            off_struct as u32,
            off_strings as u32,
            HEADER as u32,
            17,
            16,
            0,
            self.strings.len() as u32,
            self.structs.len() as u32,
        ] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&[0u8; RSVMAP]);
        out.extend_from_slice(&self.structs);
        out.extend_from_slice(&self.strings);
        out
    }
}
