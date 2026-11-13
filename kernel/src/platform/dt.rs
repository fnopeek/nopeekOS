//! The flattened device tree (devicetree specification v0.4, chapter 5):
//! how a board without ACPI describes its memory and devices.
//!
//! Read in place, without the heap: `early_mmu` walks it with the MMU and
//! the data cache still off. Every read is an aligned 32-bit or byte load
//! (the format keeps tokens and property values 4-byte aligned), so the
//! walk never makes an unaligned access, which faults on Device memory.

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

/// Deepest node nesting the walk follows.
const MAX_DEPTH: usize = 16;

/// A device tree in memory.
#[derive(Clone, Copy)]
pub struct Fdt {
    base: usize,
    total: usize,
    structs: usize,
    structs_len: usize,
    strings: usize,
    strings_len: usize,
    rsvmap: usize,
}

/// A node: where its BEGIN_NODE token is, and the cells its parent uses
/// for `reg`.
#[derive(Clone, Copy)]
pub struct Node {
    pub offset: usize,
    pub depth: usize,
    pub addr_cells: u32,
    pub size_cells: u32,
    /// The BEGIN_NODE offsets of its ancestors, root first.
    path: [usize; MAX_DEPTH],
}

impl Node {
    /// The ancestors, root first, as node offsets.
    pub fn ancestors(&self) -> &[usize] {
        &self.path[..self.depth]
    }
}

/// The tree the firmware booted the kernel with; its memory is kept out
/// of the frame allocator (`boot::holes`).
static BOOT: spin::Once<Fdt> = spin::Once::new();

/// Keep `fdt` as the boot tree, for parts that look at devices after boot.
pub fn set_boot(fdt: Fdt) {
    BOOT.call_once(|| fdt);
}

pub fn boot() -> Option<Fdt> {
    BOOT.get().copied()
}

impl Fdt {
    /// # Safety
    /// `addr` must point at a device tree the firmware left in memory that
    /// nothing overwrites while this value is used.
    pub unsafe fn new(addr: usize) -> Option<Fdt> {
        if addr == 0 || addr % 8 != 0 { return None; }
        // SAFETY: the caller's contract; the header is 40 bytes.
        let r = |o: usize| u32::from_be(unsafe { core::ptr::read_volatile((addr + o) as *const u32) });
        if r(0) != FDT_MAGIC { return None; }
        let fdt = Fdt {
            base: addr,
            total: r(4) as usize,
            structs: r(8) as usize,
            strings: r(12) as usize,
            rsvmap: r(16) as usize,
            strings_len: r(32) as usize,
            structs_len: r(36) as usize,
        };
        if fdt.structs + fdt.structs_len > fdt.total || fdt.strings + fdt.strings_len > fdt.total {
            return None;
        }
        Some(fdt)
    }

    pub fn addr(&self) -> usize { self.base }
    pub fn size(&self) -> usize { self.total }

    fn u32_at(&self, off: usize) -> u32 {
        if off + 4 > self.total || off % 4 != 0 { return 0; }
        // SAFETY: inside the tree (`new`'s contract), 4-byte aligned.
        u32::from_be(unsafe { core::ptr::read_volatile((self.base + off) as *const u32) })
    }

    fn byte_at(&self, off: usize) -> u8 {
        if off >= self.total { return 0; }
        // SAFETY: inside the tree (`new`'s contract).
        unsafe { core::ptr::read_volatile((self.base + off) as *const u8) }
    }

    /// The memory reservation block: `(address, size)` pairs.
    pub fn reservations(&self, mut f: impl FnMut(u64, u64)) {
        let mut o = self.rsvmap;
        for _ in 0..256 {
            let a = (self.u32_at(o) as u64) << 32 | self.u32_at(o + 4) as u64;
            let s = (self.u32_at(o + 8) as u64) << 32 | self.u32_at(o + 12) as u64;
            if a == 0 && s == 0 { return; }
            f(a, s);
            o += 16;
        }
    }

    /// Length of the NUL-terminated string at `off`.
    fn str_len(&self, off: usize) -> usize {
        let mut n = 0;
        while off + n < self.total && self.byte_at(off + n) != 0 { n += 1; }
        n
    }

    fn str_eq(&self, off: usize, s: &[u8]) -> bool {
        if self.str_len(off) != s.len() { return false; }
        s.iter().enumerate().all(|(i, &b)| self.byte_at(off + i) == b)
    }

    /// The node's name (unit address included), as a byte range.
    pub fn name(&self, node: &Node) -> (usize, usize) {
        let o = node.offset + 4;
        (o, self.str_len(o))
    }

    /// Does the node's name equal `name`, or `name` followed by `@unit`?
    pub fn name_is(&self, node: &Node, name: &[u8]) -> bool {
        let (o, len) = self.name(node);
        if len < name.len() { return false; }
        if !name.iter().enumerate().all(|(i, &b)| self.byte_at(o + i) == b) { return false; }
        len == name.len() || self.byte_at(o + name.len()) == b'@'
    }

    /// Copy the node's name into `out`; returns the length.
    pub fn name_into(&self, node: &Node, out: &mut [u8]) -> usize {
        let (o, len) = self.name(node);
        let n = len.min(out.len());
        for (i, b) in out[..n].iter_mut().enumerate() { *b = self.byte_at(o + i); }
        n
    }

    /// Offset of the first token after the node's name.
    fn after_name(&self, node_off: usize) -> usize {
        let name = node_off + 4;
        (name + self.str_len(name) + 1 + 3) & !3
    }

    /// Visit every node in document order; stop when `f` returns false.
    pub fn walk(&self, mut f: impl FnMut(&Node) -> bool) {
        let mut o = self.structs;
        let end = self.structs + self.structs_len;
        let mut path = [0usize; MAX_DEPTH];
        // Cells each open node gives its children, by depth.
        let mut cells = [(2u32, 1u32); MAX_DEPTH + 1];
        let mut depth = 0usize;
        while o < end {
            match self.u32_at(o) {
                FDT_BEGIN_NODE => {
                    if depth >= MAX_DEPTH { return; }
                    let (ac, sc) = cells[depth];
                    let node = Node { offset: o, depth, addr_cells: ac, size_cells: sc, path };
                    // This node's own #address-cells/#size-cells, for its children.
                    let own_ac = self.prop_u32(&node, b"#address-cells").unwrap_or(2);
                    let own_sc = self.prop_u32(&node, b"#size-cells").unwrap_or(1);
                    cells[depth + 1] = (own_ac, own_sc);
                    if !f(&node) { return; }
                    path[depth] = o;
                    depth += 1;
                    o = self.after_name(o);
                }
                FDT_END_NODE => {
                    depth = depth.saturating_sub(1);
                    o += 4;
                }
                FDT_PROP => {
                    let len = self.u32_at(o + 4) as usize;
                    o = (o + 12 + len + 3) & !3;
                }
                FDT_NOP => o += 4,
                FDT_END | _ => return,
            }
        }
    }

    /// The property `name` of the node at `node_off`: `(value offset, length)`.
    fn prop_at(&self, node_off: usize, name: &[u8]) -> Option<(usize, usize)> {
        let mut o = self.after_name(node_off);
        let end = self.structs + self.structs_len;
        while o < end {
            match self.u32_at(o) {
                FDT_PROP => {
                    let len = self.u32_at(o + 4) as usize;
                    let nameoff = self.u32_at(o + 8) as usize;
                    if nameoff < self.strings_len && self.str_eq(self.strings + nameoff, name) {
                        return Some((o + 12, len));
                    }
                    o = (o + 12 + len + 3) & !3;
                }
                FDT_NOP => o += 4,
                _ => return None,
            }
        }
        None
    }

    pub fn prop(&self, node: &Node, name: &[u8]) -> Option<(usize, usize)> {
        self.prop_at(node.offset, name)
    }

    /// The big-endian cell at `off`, an offset `prop` returned.
    pub fn cell_u32(&self, off: usize) -> u32 {
        self.u32_at(off)
    }

    pub fn prop_u32(&self, node: &Node, name: &[u8]) -> Option<u32> {
        let (o, len) = self.prop(node, name)?;
        if len < 4 { return None; }
        Some(self.u32_at(o))
    }

    /// Read `cells` cells at `off` as one number.
    fn cells(&self, off: usize, cells: u32) -> u64 {
        (0..cells as usize).fold(0u64, |v, i| v << 32 | self.u32_at(off + 4 * i) as u64)
    }

    /// Does the string-list property `name` contain `s`?
    pub fn prop_has_str(&self, node: &Node, name: &[u8], s: &[u8]) -> bool {
        let Some((o, len)) = self.prop(node, name) else { return false };
        let mut i = 0;
        while i < len {
            let l = self.str_len(o + i);
            if l == s.len() && s.iter().enumerate().all(|(k, &b)| self.byte_at(o + i + k) == b) {
                return true;
            }
            i += l + 1;
        }
        false
    }

    /// Copy a string property into `out`; returns its length.
    pub fn prop_str(&self, node: &Node, name: &[u8], out: &mut [u8]) -> Option<usize> {
        let (o, len) = self.prop(node, name)?;
        let n = self.str_len(o).min(len).min(out.len());
        for (i, b) in out[..n].iter_mut().enumerate() { *b = self.byte_at(o + i); }
        Some(n)
    }

    /// Is the node enabled (`status` absent, "okay" or "ok")?
    pub fn is_enabled(&self, node: &Node) -> bool {
        match self.prop(node, b"status") {
            None => true,
            Some(_) => self.prop_has_str(node, b"status", b"okay") || self.prop_has_str(node, b"status", b"ok"),
        }
    }

    /// The node's `reg` entries as CPU physical `(address, size)`, translated
    /// through every parent bus's `ranges`.
    pub fn reg(&self, node: &Node, mut f: impl FnMut(u64, u64)) {
        let Some((o, len)) = self.prop(node, b"reg") else { return };
        let (ac, sc) = (node.addr_cells, node.size_cells);
        let entry = 4 * (ac + sc) as usize;
        if entry == 0 { return; }
        let mut i = 0;
        while i + entry <= len {
            let a = self.cells(o + i, ac);
            let s = self.cells(o + i + 4 * ac as usize, sc);
            if let Some(pa) = self.translate(node, a) { f(pa, s); }
            i += entry;
        }
    }

    /// The node's `reg` entries as written, untranslated (an address a
    /// device uses, like an MSI target on its bus).
    pub fn reg_raw(&self, node: &Node, mut f: impl FnMut(u64, u64)) {
        let Some((o, len)) = self.prop(node, b"reg") else { return };
        let (ac, sc) = (node.addr_cells, node.size_cells);
        let entry = 4 * (ac + sc) as usize;
        if entry == 0 { return; }
        let mut i = 0;
        while i + entry <= len {
            f(self.cells(o + i, ac), self.cells(o + i + 4 * ac as usize, sc));
            i += entry;
        }
    }

    /// A `ranges`-shaped property of `node` (`ranges`, `dma-ranges`), per
    /// entry: the child bus's flags cell (PCI, three address cells; 0
    /// otherwise), the child address, the parent address translated to the
    /// CPU, the size.
    pub fn ranges(&self, node: &Node, name: &[u8], mut f: impl FnMut(u32, u64, u64, u64)) {
        let Some((o, len)) = self.prop(node, name) else { return };
        let (child_ac, child_sc) = self.own_cells(node.offset);
        let parent_ac = node.addr_cells;
        let entry = 4 * (child_ac + parent_ac + child_sc) as usize;
        if entry == 0 { return; }
        let mut i = 0;
        while i + entry <= len {
            let (flags, child) = if child_ac == 3 {
                (self.u32_at(o + i), self.cells(o + i + 4, 2))
            } else {
                (0, self.cells(o + i, child_ac))
            };
            let parent = self.cells(o + i + 4 * child_ac as usize, parent_ac);
            let size = self.cells(o + i + 4 * (child_ac + parent_ac) as usize, child_sc);
            if let Some(cpu) = self.translate(node, parent) { f(flags, child, cpu, size); }
            i += entry;
        }
    }

    /// The node whose `phandle` is `ph`.
    pub fn find_phandle(&self, ph: u32) -> Option<Node> {
        let mut found = None;
        self.walk(|n| {
            if self.prop_u32(n, b"phandle") == Some(ph) {
                found = Some(*n);
                return false;
            }
            true
        });
        found
    }

    /// The `i`th cell of a property.
    pub fn prop_cell(&self, node: &Node, name: &[u8], i: usize) -> Option<u32> {
        let (o, len) = self.prop(node, name)?;
        if 4 * (i + 1) > len { return None; }
        Some(self.u32_at(o + 4 * i))
    }

    /// The raw bytes of a property into `out` (`of_property_read_u8_array`);
    /// None unless it holds exactly `out.len()` bytes or more.
    pub fn prop_bytes(&self, node: &Node, name: &[u8], out: &mut [u8]) -> Option<()> {
        let (o, len) = self.prop(node, name)?;
        if len < out.len() { return None; }
        for (i, b) in out.iter_mut().enumerate() { *b = self.byte_at(o + i); }
        Some(())
    }

    /// The position of `s` in the string-list property `name`.
    pub fn prop_str_index(&self, node: &Node, name: &[u8], s: &[u8]) -> Option<usize> {
        let (o, len) = self.prop(node, name)?;
        let (mut i, mut idx) = (0, 0);
        while i < len {
            let l = self.str_len(o + i);
            if l == s.len() && s.iter().enumerate().all(|(k, &b)| self.byte_at(o + i + k) == b) {
                return Some(idx);
            }
            i += l + 1;
            idx += 1;
        }
        None
    }

    /// The direct children of `node`.
    pub fn children(&self, node: &Node, mut f: impl FnMut(&Node) -> bool) {
        self.walk(|n| {
            if n.ancestors().last() == Some(&node.offset) { f(n) } else { true }
        });
    }

    /// The first `reg` entry.
    pub fn reg0(&self, node: &Node) -> Option<(u64, u64)> {
        let mut r = None;
        self.reg(node, |a, s| if r.is_none() { r = Some((a, s)) });
        r
    }

    /// Translate a child bus address of `node` up to the CPU's address
    /// space (devicetree spec 2.3.8). A bus without `ranges` does not map
    /// its children; an empty `ranges` is an identity map.
    fn translate(&self, node: &Node, mut addr: u64) -> Option<u64> {
        let anc = node.ancestors();
        // Walk from the node's parent up to the root's children.
        for d in (1..anc.len()).rev() {
            let bus = anc[d];
            let (child_ac, child_sc) = self.own_cells(bus);
            let (parent_ac, _) = self.own_cells(anc[d - 1]);
            let (ro, rlen) = self.prop_at(bus, b"ranges")?;
            if rlen == 0 { continue; }
            let entry = 4 * (child_ac + parent_ac + child_sc) as usize;
            if entry == 0 { return None; }
            let mut hit = None;
            let mut i = 0;
            while i + entry <= rlen {
                let cb = self.cells(ro + i, child_ac);
                let pb = self.cells(ro + i + 4 * child_ac as usize, parent_ac);
                let sz = self.cells(ro + i + 4 * (child_ac + parent_ac) as usize, child_sc);
                if addr >= cb && addr - cb < sz {
                    hit = Some(pb + (addr - cb));
                    break;
                }
                i += entry;
            }
            addr = hit?;
        }
        Some(addr)
    }

    /// `#address-cells` and `#size-cells` of the node at `off`.
    fn own_cells(&self, off: usize) -> (u32, u32) {
        let ac = self.prop_at(off, b"#address-cells").map(|(o, _)| self.u32_at(o)).unwrap_or(2);
        let sc = self.prop_at(off, b"#size-cells").map(|(o, _)| self.u32_at(o)).unwrap_or(1);
        (ac, sc)
    }

    /// The node at the absolute `path` ("/chosen", "/soc/serial@7d001000").
    pub fn find_path(&self, path: &[u8]) -> Option<Node> {
        let mut parts = [(0usize, 0usize); MAX_DEPTH];
        let mut n = 0;
        let mut start = 0;
        for i in 0..=path.len() {
            if i == path.len() || path[i] == b'/' {
                if i > start && n < MAX_DEPTH {
                    parts[n] = (start, i);
                    n += 1;
                }
                start = i + 1;
            }
        }
        let mut found = None;
        self.walk(|node| {
            let d = node.depth;
            if d == 0 {
                if n == 0 { found = Some(*node); return false; }
                return true;
            }
            if d > n { return true; }
            let (s, e) = parts[d - 1];
            let want = &path[s..e];
            let (o, len) = self.name(node);
            let exact = len == want.len() && want.iter().enumerate().all(|(i, &b)| self.byte_at(o + i) == b);
            if !exact && !(want.iter().all(|&b| b != b'@') && self.name_is(node, want)) {
                return true;
            }
            // Every ancestor must have matched too: the walk only reaches
            // this depth below a matching parent if we track it.
            if !self.ancestors_match(node, &parts[..d - 1], path) { return true; }
            if d == n { found = Some(*node); return false; }
            true
        });
        found
    }

    fn ancestors_match(&self, node: &Node, parts: &[(usize, usize)], path: &[u8]) -> bool {
        let anc = node.ancestors();
        for (i, &(s, e)) in parts.iter().enumerate() {
            let Some(&off) = anc.get(i + 1) else { return false };
            let want = &path[s..e];
            let o = off + 4;
            let len = self.str_len(o);
            let exact = len == want.len() && want.iter().enumerate().all(|(k, &b)| self.byte_at(o + k) == b);
            let base = want.iter().all(|&b| b != b'@') && len > want.len()
                && want.iter().enumerate().all(|(k, &b)| self.byte_at(o + k) == b)
                && self.byte_at(o + want.len()) == b'@';
            if !exact && !base { return false; }
        }
        true
    }

    /// The node `stdout-path` in /chosen names, following an alias.
    pub fn stdout(&self) -> Option<Node> {
        let chosen = self.find_path(b"/chosen")?;
        let mut buf = [0u8; 128];
        let n = self.prop_str(&chosen, b"stdout-path", &mut buf)?;
        // Options after ':' (baud rate) are not part of the path.
        let n = buf[..n].iter().position(|&b| b == b':').unwrap_or(n);
        if buf.first() == Some(&b'/') {
            return self.find_path(&buf[..n]);
        }
        let aliases = self.find_path(b"/aliases")?;
        let mut path = [0u8; 128];
        let m = self.prop_str(&aliases, &buf[..n], &mut path)?;
        self.find_path(&path[..m])
    }

    /// The first enabled node compatible with `compat`.
    pub fn find_compatible(&self, compat: &[u8]) -> Option<Node> {
        let mut found = None;
        self.walk(|node| {
            if self.prop_has_str(node, b"compatible", compat) && self.is_enabled(node) {
                found = Some(*node);
                return false;
            }
            true
        });
        found
    }
}
