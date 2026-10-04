//! AML method evaluator — enough opcodes to run the firmware's battery methods.

use crate::value::{obj, path_str, Obj, Path, Place, Seg, Value};
use crate::{Ec, Namespace, Node};
use alloc::collections::BTreeMap;
use alloc::{format, string::String, vec, vec::Vec};

const MAX_DEPTH: usize = 64;

pub struct Interp<'a> {
    ns: &'a Namespace,
    /// Nodes a method declares at run time (`OpRegion`, `Field` in a body).
    /// They are not in the loaded table: the loader defers method bodies and
    /// sees them only on execution.
    ///
    /// ACPICA creates the node at parse time and evaluates address and length
    /// at execution (`acpi_ds_eval_region_operands`, dsopcode.c); here both
    /// happen at execution, with the same result. Deliberate difference:
    /// ACPICA deletes the nodes when the method exits (owner id); we keep
    /// them until the end of the run. `decode` builds a fresh namespace each
    /// round, so they live no longer than one reading.
    dyn_nodes: BTreeMap<Path, Node>,
    ec: &'a mut dyn Ec,
    depth: usize,
    /// Report every name read.
    ///
    /// For questions like "why does `_STA` return zero?": the answer lies in
    /// the few values the method reads, which are visible nowhere else.
    trace: bool,
    /// In-memory backing for non-EmbeddedControl regions (SystemIO,
    /// SystemMemory, PCI config, ...). Keyed by (region_space, absolute_byte).
    /// Only EmbeddedControl touches real hardware; the firmware's SMI/init
    /// handshakes thus become harmless writes-readable-back here, so their
    /// write-then-poll loops terminate without any real port I/O.
    mem: BTreeMap<(u8, u64), u8>,
}

struct Frame<'a> {
    scope: Path,
    args: Vec<Obj>,
    locals: Vec<Obj>,
    body: &'a [u8],
}

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

type R<T> = Result<T, String>;

// ── public entry points ───────────────────────────────────────────────

pub fn find_batteries(ns: &Namespace) -> Vec<Path> {
    let mut out = Vec::new();
    for (path, node) in ns.nodes.iter() {
        if let Node::Name(v) = node {
            if path.last() == Some(&crate::value::seg("_HID")) {
                if is_pnp0c0a(&v.borrow()) {
                    let mut dev = path.clone();
                    dev.pop(); // drop _HID -> the device path
                    out.push(dev);
                }
            }
        }
    }
    out
}

fn is_pnp0c0a(v: &Value) -> bool {
    // _HID may be an EisaId-encoded integer or a string "PNP0C0A".
    match v {
        Value::Str(s) => s == "PNP0C0A",
        Value::Int(n) => *n == eisa_id("PNP0C0A"),
        _ => false,
    }
}

/// EisaId packing (ACPI): 7-bit compressed mfg + hex product, big-endian dword.
fn eisa_id(s: &str) -> u64 {
    let b = s.as_bytes();
    if b.len() != 7 {
        return 0;
    }
    let c = |x: u8| -> u64 {
        if x.is_ascii_digit() { (x - b'0') as u64 } else { (x - b'A' + 10) as u64 }
    };
    let m0 = (b[0] - b'@') as u64;
    let m1 = (b[1] - b'@') as u64;
    let m2 = (b[2] - b'@') as u64;
    let prod = (c(b[3]) << 12) | (c(b[4]) << 8) | (c(b[5]) << 4) | c(b[6]);
    let swapped = (m0 << 26) | (m1 << 21) | (m2 << 16) | prod;
    // Stored little-endian in the dword -> byte-swap for comparison.
    ((swapped >> 24) & 0xFF)
        | (((swapped >> 16) & 0xFF) << 8)
        | (((swapped >> 8) & 0xFF) << 16)
        | ((swapped & 0xFF) << 24)
}

/// The EC's GPE number: `_GPE` in the scope of an EmbeddedControl region
/// (ACPI 6.5 §12.11, an Integer here; the Package form for a GPE block
/// device is not handled).
pub fn ec_gpe(ns: &Namespace) -> Option<u32> {
    for (path, node) in ns.nodes.iter() {
        if let Node::Region { space: 3, .. } = node {
            let mut g = path.clone();
            g.pop();
            g.push(crate::value::seg("_GPE"));
            if let Some(Node::Name(v)) = ns.nodes.get(&g) {
                if let Value::Int(n) = &*v.borrow() {
                    return Some(*n as u32);
                }
            }
        }
    }
    None
}

pub fn read_battery(ns: &Namespace, ec: &mut dyn Ec, bat: &Path) -> R<crate::BatteryInfo> {
    let mut it = Interp { ns, dyn_nodes: BTreeMap::new(), ec, depth: 0, trace: false, mem: BTreeMap::new() };
    // Order as in ACPICA `acpi_initialize_objects`: enable operation regions
    // first (`_REG`), then start the devices (`_STA`/`_INI`).
    it.run_deferred();
    it.ec.note("[aml]  phase _REG");
    it.register_ec_regions()?;
    it.ec.note("[aml]  phase _INI");
    let ini_ran = it.run_ini_methods();
    it.ec.note_num("[aml]  _INI methods run: ", ini_ran as u64);

    // How many `_Qxx` does this DSDT have?
    //
    // These are the EC's query handlers: the EC signals an event (battery
    // in/out, AC), the OS fetches the event number with QR_EC and calls
    // `_Q<nr>`. Firmware often updates state such as "battery present" only
    // there.
    let mut qcount = 0u64;
    for p in ns.nodes.keys() {
        if let Some(last) = p.last() {
            if last[0] == b'_' && last[1] == b'Q' { qcount += 1; }
        }
    }
    it.ec.note_num("[aml]  _Qxx handlers in DSDT: ", qcount);

    // Fetch pending EC events and run their `_Qxx`.
    //
    // This is `acpi_ec_clear` from Linux `drivers/acpi/ec.c`: the EC
    // collects events (battery inserted, AC, lid), sets bit 5 of its status
    // register and waits for someone to fetch them with `QR_EC`. Only in
    // `_Q<nr>` does the firmware update its state; if nobody fetches them,
    // `_STA` stays at its initial value (e.g. "no battery").
    //
    // Limit of 100 as `ACPI_EC_CLEAR_MAX`; Linux warns when it is hit and
    // treats it as a stuck EC.
    let drained = it.drain_ec_queries();
    it.ec.note_num("[aml]  stale EC events drained: ", drained as u64);

    // The battery device's `_STA`, which an OS queries before `_BST`: bit 0
    // present, bit 3 functional, bit 4 = battery inserted (ACPI 6.5
    // §10.2.1). Without it, `_BST` returning 0xFFFFFFFF is ambiguous.
    let mut sta = bat.clone();
    sta.push(crate::value::seg("_STA"));
    // Presence comes from `_STA` bit 4 (ACPI 6.5 §10.2.1 "Battery is
    // present"), not from `remaining`: 0xFFFFFFFF means "unknown" per the
    // specification, not "absent". A full battery on AC whose remaining
    // capacity the firmware does not state is still present.
    let mut sta_present: Option<bool> = None;
    if it.has(&sta) {
        match it.call_path(&sta, Vec::new()) {
            Ok(v) => {
                let f = v.as_int();
                it.ec.note_num("[aml]  battery _STA=", f);
                sta_present = Some(f & 0x10 != 0);
            }
            Err(_) => it.ec.note("[aml]  battery _STA failed"),
        }
    } else {
        it.ec.note("[aml]  battery has no _STA");
    }
    // Order as in Linux: description first, then state.
    //
    // `drivers/acpi/battery.c` calls `acpi_battery_get_info` (_BIX/_BIF)
    // before `acpi_battery_get_state` (_BST). Some firmware selects the
    // battery or latches readings in `_BIF`, and a `_BST` before it reports
    // "unknown".
    it.ec.note("[aml]  phase _BIF");
    let (full, power_unit) = it.read_full_charge(bat)?;
    it.ec.note_num("[aml]  full charge: ", full as u64);

    it.ec.note("[aml]  phase _BST");

    // _BST -> Package { State, PresentRate, RemainingCapacity, Voltage }
    let mut p = bat.clone();
    p.push(crate::value::seg("_BST"));
    let bst = it.call_path(&p, Vec::new())?;
    let (state, remaining, rate, voltage_mv) = match &bst {
        Value::Package(e) if e.len() >= 4 => {
            // Log the whole package: 0xFFFFFFFF in every field means "no
            // battery", in only one field it means "unknown"; `remaining`
            // alone cannot tell.
            for (i, el) in e.iter().enumerate().take(4) {
                it.ec.note_num(
                    match i { 0 => "[aml]   _BST[0] state=", 1 => "[aml]   _BST[1] rate=",
                              2 => "[aml]   _BST[2] remaining=", _ => "[aml]   _BST[3] voltage=" },
                    el.borrow().as_int());
            }
            (e[0].borrow().as_int() as u32, e[2].borrow().as_int() as u32,
             e[1].borrow().as_int() as u32, e[3].borrow().as_int() as u32)
        }
        _ => return Err(format!("_BST did not return a Package(>=4): got {}", kind(&bst))),
    };

    // Absent batteries report 0xFFFFFFFF in every field.
    //
    // What `_BST` actually returned is passed along, so the caller can
    // tell "firmware reports no battery" from "nothing was read". If `_STA`
    // explicitly says "no battery" there is none; otherwise an unknown
    // `remaining` counts as unknown, not absent.
    if sta_present == Some(false) {
        return Ok(crate::BatteryInfo {
            present: false,
            state,
            remaining_mah: remaining,
            ..Default::default()
        });
    }
    if remaining == 0xFFFF_FFFF && sta_present.is_none() {
        return Ok(crate::BatteryInfo {
            present: false,
            state,
            remaining_mah: remaining,
            ..Default::default()
        });
    }

    // `remaining == 0xFFFFFFFF` means unknown (ACPI 6.5 §10.2.2), and an
    // unknown remaining capacity must not yield a percentage; computed as a
    // measurement it would clamp to a constant 100 %.
    let percent = if full > 0 && remaining != 0xFFFF_FFFF {
        (((remaining as u64) * 100 + (full as u64) / 2) / full as u64).min(100) as u8
    } else {
        0
    };

    Ok(crate::BatteryInfo {
        present: true,
        state,
        remaining_mah: remaining,
        full_charge_mah: full,
        percent,
        rate,
        voltage_mv,
        power_unit,
    })
}

impl<'a> Interp<'a> {
    /// (LastFullChargeCap, power unit) — the unit says whether `_BST`'s
    /// rate and capacities are mW/mWh (0) or mA/mAh (1).
    fn read_full_charge(&mut self, bat: &Path) -> R<(u32, u32)> {
        // _BIF: Package[0]=PowerUnit, [1]=DesignCap, [2]=LastFullChargeCap.
        let mut p = bat.clone();
        p.push(crate::value::seg("_BIF"));
        if self.has(&p) {
            let v = self.call_path(&p, Vec::new())?;
            if let Value::Package(e) = &v {
                if e.len() >= 3 {
                    return Ok((e[2].borrow().as_int() as u32, e[0].borrow().as_int() as u32));
                }
            }
        }
        // _BIX: [0]=Revision, [1]=PowerUnit, [2]=DesignCap, [3]=LastFullChargeCap.
        let mut p = bat.clone();
        p.push(crate::value::seg("_BIX"));
        if self.has(&p) {
            let v = self.call_path(&p, Vec::new())?;
            if let Value::Package(e) = &v {
                if e.len() >= 4 {
                    return Ok((e[3].borrow().as_int() as u32, e[1].borrow().as_int() as u32));
                }
            }
        }
        Err(String::from("neither _BIF nor _BIX usable"))
    }

    /// Run every EmbeddedControl region's parent `_REG(3, 1)` so the firmware
    /// sets its "EC ready" gate (e.g. ECRG = 1). Generic — no name hardcoded.
    fn register_ec_regions(&mut self) -> R<()> {
        // `_REG(space, 1)` for every region space we serve, not only the EC.
        //
        // ACPICA calls `_REG` for every space with an installed handler
        // (`acpi_ev_initialize_op_regions`), telling the firmware "this
        // space is now usable". A DSDT that sets up state in a `_REG` for
        // SystemIO or SystemMemory would otherwise stay half-initialised.
        //
        // Pairs of (parent scope, space), so a scope with two regions gets
        // two calls.
        let mut pairs: Vec<(Path, u8)> = Vec::new();
        for (path, node) in self.ns.nodes.iter() {
            if let Node::Region { space, .. } = node {
                let mut par = path.clone();
                par.pop();
                let e = (par, *space);
                if !pairs.contains(&e) {
                    pairs.push(e);
                }
            }
        }
        // EC last: the other spaces often set up the gate through which the
        // EC answers at all.
        pairs.sort_by_key(|(_, sp)| if *sp == 3 { 1 } else { 0 });
        for (par, space) in pairs {
            let mut reg = par.clone();
            reg.push(crate::value::seg("_REG"));
            if self.has(&reg) {
                let args = vec![obj(Value::Int(space as u64)), obj(Value::Int(1))];
                // A failing `_REG` must not abort the others — same as for
                // `_INI`.
                let _ = self.call_path(&reg, args);
            }
        }
        Ok(())
    }

    /// Does this path exist, in the table or declared at run time?
    fn has(&self, p: &Path) -> bool {
        self.dyn_nodes.contains_key(p) || self.ns.nodes.contains_key(p)
    }

    /// Look up a node; run-time declarations shadow the table.
    fn node(&self, p: &Path) -> Option<&Node> {
        self.dyn_nodes.get(p).or_else(|| self.ns.get(p))
    }

    /// Like `Namespace::resolve`, but over both maps. A field a method has
    /// just created must be found by it.
    fn resolve(&self, scope: &Path, rooted: bool, carets: usize, segs: &[Seg]) -> Option<Path> {
        let mut base: Path = if rooted {
            Vec::new()
        } else {
            let mut b = scope.clone();
            for _ in 0..carets { b.pop(); }
            b
        };
        if !rooted && carets == 0 && segs.len() == 1 {
            loop {
                let mut cand = base.clone();
                cand.push(segs[0]);
                if self.has(&cand) { return Some(cand); }
                if base.is_empty() { return None; }
                base.pop();
            }
        }
        for sg in segs { base.push(*sg); }
        if self.has(&base) { Some(base) } else { None }
    }

    /// Absolute path of a declaration (no upward search), like
    /// `Loader::def_path`.
    fn def_path(&self, scope: &Path, n: &NRef) -> Path {
        let mut base: Path = if n.rooted {
            Vec::new()
        } else {
            let mut b = scope.clone();
            for _ in 0..n.carets { b.pop(); }
            b
        };
        for sg in &n.segs { base.push(*sg); }
        base
    }

    /// `_INI` over the whole namespace — the startup the OS performs.
    ///
    /// Modelled on ACPICA `acpi_ns_initialize_devices` /
    /// `acpi_ns_init_one_device` (nsinit.c): top-down, root first, and
    /// `_STA` decides.
    ///
    ///   * no `_STA`  -> present and functional
    ///   * bit 0 set -> present, `_INI` runs
    ///   * neither bit 0 nor bit 3 -> neither present nor functional: the
    ///     whole subtree is skipped ("don't look at the children of such a
    ///     device")
    ///   * absent but functional -> no `_INI`, but children are visited
    ///
    /// Firmware sets up its state in these methods, including the state the
    /// EC battery reporting depends on.
    ///
    /// Errors are swallowed, as in ACPICA: a failing `_INI` must not abort
    /// the startup of the other devices.
    ///
    /// Deliberate deviation: ACPICA runs this once at boot, we run it every
    /// round, since `decode` rebuilds the namespace each round; `_REG` runs
    /// every time for the same reason.
    fn run_ini_methods(&mut self) -> u32 {
        let ini = crate::value::seg("_INI");
        let sta_seg = crate::value::seg("_STA");

        // Root first (ACPICA: \_INI, then \_SB._INI, then the rest).
        let root_ini: Path = vec![ini];
        if self.has(&root_ini) {
            let _ = self.call_path(&root_ini, Vec::new());
        }

        // Walk the `_INI` methods, not nodes that look like devices.
        //
        // The loader files Device, Scope, Processor, PowerRes and
        // ThermalZone alike as `Node::Scope`, and anything filed otherwise
        // would be missed. The parent of an `_INI` is the device, so the
        // walk depends on what we look for rather than on a classification.
        let ini_seg = ini;
        let mut devs: Vec<Path> = self
            .ns
            .nodes
            .keys()
            .filter(|p| p.len() > 1 && p.last() == Some(&ini_seg))
            .map(|p| { let mut d = p.clone(); d.pop(); d })
            .collect();
        self.ec.note_num("[aml]  _INI methods found: ", devs.len() as u64);
        devs.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));

        let mut ran = 0u32;
        let mut pruned: Vec<Path> = Vec::new();
        for dev in devs {
            if pruned
                .iter()
                .any(|d| dev.len() > d.len() && dev.starts_with(d.as_slice()))
            {
                continue;
            }
            let mut sta = dev.clone();
            sta.push(sta_seg);
            let flags: u64 = if self.has(&sta) {
                match self.call_path(&sta, Vec::new()) {
                    Ok(v) => v.as_int(),
                    // If `_STA` fails, treat the device as ACPICA does:
                    // present and functional, so one error does not disable
                    // a whole subtree.
                    Err(_) => 0x0F,
                }
            } else {
                u32::MAX as u64
            };
            let present = flags & 0x01 != 0;
            let functioning = flags & 0x08 != 0;
            if !present && !functioning {
                pruned.push(dev);
                continue;
            }
            if present {
                let mut ip = dev.clone();
                ip.push(ini);
                if self.has(&ip) {
                    // An `_INI` that throws must be logged. It sets up the
                    // device (e.g. a touchpad's `_INI` may set its HID
                    // descriptor address), and swallowing the error makes
                    // every follow-up failure look like a firmware quirk.
                    match self.call_path(&ip, Vec::new()) {
                        Ok(_) => {}
                        Err(e) => {
                            let name = path_str(&ip);
                            self.ec.note(&format!("[aml]  {name} failed: {e}"));
                        }
                    }
                    ran += 1;
                }
            }
        }
        ran
    }

    /// Fetch pending EC queries and run their `_Q<nr>`.
    ///
    /// The name is `_Q` plus the number in hex — Linux reads it with
    /// `sscanf(node_name, "_Q%x", &value)`, i.e. two uppercase digits. It is
    /// looked up in the EC device's scope, the parent of an EmbeddedControl
    /// region.
    fn drain_ec_queries(&mut self) -> u32 {
        // Scopes containing an EC region.
        let mut scopes: Vec<Path> = Vec::new();
        for (path, node) in self.ns.nodes.iter() {
            if let Node::Region { space: 3, .. } = node {
                let mut par = path.clone();
                par.pop();
                if !scopes.contains(&par) { scopes.push(par); }
            }
        }
        if scopes.is_empty() { return 0; }

        let hex = |n: u8| -> Seg {
            let d = |v: u8| if v < 10 { b'0' + v } else { b'A' + (v - 10) };
            [b'_', b'Q', d(n >> 4), d(n & 0xF)]
        };

        let mut done = 0u32;
        for _ in 0..100 {
            let q = match self.ec.query() { Some(q) => q, None => break };
            self.ec.note_num("[aml]   EC query 0x", q as u64);
            let name = hex(q);
            let mut found = false;
            for sc in &scopes {
                let mut h = sc.clone();
                h.push(name);
                if self.has(&h) {
                    let _ = self.call_path(&h, Vec::new());
                    found = true;
                    break;
                }
            }
            if !found {
                // Linux logs this too and continues: an event without a
                // handler is not an error; it has been fetched.
                self.ec.note("[aml]   (no handler for that query)");
            }
            self.ec.ec_event(q, found);
            done += 1;
        }
        done
    }

    fn call_path(&mut self, path: &Path, args: Vec<Obj>) -> R<Value> {
        if self.depth > MAX_DEPTH {
            return Err(String::from("recursion too deep"));
        }
        let (body, scope) = match self.ns.get(path) {
            Some(Node::Method { body, scope, .. }) => (body.as_slice(), scope.clone()),
            Some(Node::Name(v)) => return Ok(v.borrow().clone()),
            Some(Node::Field { .. }) => return self.read_field(path),
            _ => {
                // A field unit declared at run time lives only in
                // `dyn_nodes`. Methods can never be there, so the access
                // above deliberately stays on the table: it borrows `body`,
                // which must outlive the method.
                if let Some(Node::BufferField { buf, bit_offset, bit_width }) =
                    self.dyn_nodes.get(path)
                {
                    let v = buf.borrow();
                    return Ok(match &*v {
                        Value::Buffer(b) => buf_field_read(b, *bit_offset, *bit_width),
                        _ => Value::Int(0),
                    });
                }
                if self.dyn_nodes.contains_key(path) { return self.read_field(path); }
                return Err(format!("call: {} is not a method", path_str(path)));
            }
        };
        // The method's own scope is the path itself (names it creates live here);
        // unqualified lookups search upward from here.
        let mut locals = Vec::with_capacity(8);
        for _ in 0..8 {
            locals.push(obj(Value::Uninit));
        }
        let frame = Frame { scope: path.clone(), args, locals, body };
        let _ = scope; // body uses `path` as its scope anchor
        self.depth += 1;
        let r = self.exec_list(&frame, 0, frame.body.len());
        self.depth -= 1;
        match r {
            Ok(Flow::Return(v)) => Ok(v),
            Ok(_) => Ok(Value::Uninit),
            Err(e) => Err(format!("{} -> {}", path_str(path), e)),
        }
    }

    // ── statement execution ───────────────────────────────────────────

    fn exec_list(&mut self, f: &Frame, start: usize, end: usize) -> R<Flow> {
        let mut p = start;
        while p < end {
            let (flow, np) = self.stmt(f, p, end)?;
            match flow {
                Flow::Normal => p = np,
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn stmt(&mut self, f: &Frame, p: usize, _end: usize) -> R<(Flow, usize)> {
        let b = f.body;
        match b[p] {
            0xA0 => {
                // If
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let (cond, p2) = self.eval(f, p1)?;
                if cond.as_int() != 0 {
                    let flow = self.exec_list(f, p2, pkg_end)?;
                    if !matches!(flow, Flow::Normal) {
                        return Ok((flow, pkg_end));
                    }
                    // fall through past a possible Else
                    return Ok((Flow::Normal, skip_else(b, pkg_end)));
                } else {
                    // skip then-block; run Else if present
                    if pkg_end < b.len() && b[pkg_end] == 0xA1 {
                        let (else_end, e1) = pkg_length(b, pkg_end + 1);
                        let flow = self.exec_list(f, e1, else_end)?;
                        return Ok((flow, else_end));
                    }
                    return Ok((Flow::Normal, pkg_end));
                }
            }
            0xA1 => {
                // Stray Else (then-branch was taken and consumed it via skip_else,
                // so reaching here means skip it).
                let (else_end, _e1) = pkg_length(b, p + 1);
                Ok((Flow::Normal, else_end))
            }
            0xA2 => {
                // While
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let mut guard = 0u32;
                loop {
                    let (cond, p2) = self.eval(f, p1)?;
                    if cond.as_int() == 0 {
                        break;
                    }
                    match self.exec_list(f, p2, pkg_end)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok((Flow::Return(v), pkg_end)),
                        _ => {}
                    }
                    guard += 1;
                    if guard > 1_000_000 {
                        return Err(String::from("While loop runaway"));
                    }
                }
                Ok((Flow::Normal, pkg_end))
            }
            0xA3 => Ok((Flow::Normal, p + 1)), // Noop
            0xA4 => {
                // Return TermArg
                let (v, np) = self.eval(f, p + 1)?;
                Ok((Flow::Return(v), np))
            }
            0xA5 => Ok((Flow::Break, p + 1)),
            0x9F => Ok((Flow::Continue, p + 1)),
            // Name(x, value) in a method body — the most common declaration:
            // a method creates its result package, fills it and returns it.
            0x08 => {
                let (nref, p1) = name_at(b, p + 1);
                let (v, p2) = self.eval(f, p1)?;
                let target = self.def_path(&f.scope, &nref);
                self.dyn_nodes.insert(target, Node::Name(obj(v)));
                Ok((Flow::Normal, p2))
            }
            // Mutex(name, flags) / Event(name) — allowed in a body. Recorded
            // as present; `Acquire`/`Release` have no effect on a single
            // execution path anyway.
            0x5B if b[p + 1] == 0x01 => {
                let (nref, p1) = name_at(b, p + 2);
                let target = self.def_path(&f.scope, &nref);
                self.dyn_nodes.insert(target, Node::Other);
                Ok((Flow::Normal, p1 + 1)) // + SyncFlags
            }
            0x5B if b[p + 1] == 0x02 => {
                let (nref, p1) = name_at(b, p + 2);
                let target = self.def_path(&f.scope, &nref);
                self.dyn_nodes.insert(target, Node::Other);
                Ok((Flow::Normal, p1))
            }
            // Declarations in a method body. Legal and common AML: a method
            // creates its own operation region and fields, typical for
            // multiplexed EC registers. The loader never sees them because
            // it defers method bodies.
            0x5B if b[p + 1] == 0x80 => {
                // OpRegionOp NameString RegionSpace RegionOffset RegionLen.
                // Offset and length are TermArgs and are evaluated here, the
                // step ACPICA performs in `acpi_ds_eval_region_operands`.
                let (nref, p1) = name_at(b, p + 2);
                let space = b[p1];
                let (off, p2) = self.eval(f, p1 + 1)?;
                let (len, p3) = self.eval(f, p2)?;
                let target = self.def_path(&f.scope, &nref);
                self.dyn_nodes.insert(
                    target,
                    Node::Region { space, offset: off.as_int(), len: len.as_int() },
                );
                Ok((Flow::Normal, p3))
            }
            0x5B if b[p + 1] == 0x81 => {
                // FieldOp PkgLength NameString FieldFlags FieldList
                let (pkg_end, p1) = pkg_length(b, p + 2);
                let (rref, p2) = name_at(b, p1);
                let region = self.def_path(&f.scope, &rref);
                self.dyn_field_list(b, &region, p2 + 1, pkg_end);
                Ok((Flow::Normal, pkg_end))
            }
            0x8A | 0x8B | 0x8C | 0x8D | 0x8F => {
                let np = self.create_buffer_field(f, p)?;
                Ok((Flow::Normal, np))
            }
            0x5B if b[p + 1] == 0x13 => {
                let np = self.create_buffer_field(f, p)?;
                Ok((Flow::Normal, np))
            }
            _ => {
                // Expression statement (Store, method call, op with target...).
                let (_v, np) = self.eval(f, p)?;
                Ok((Flow::Normal, np))
            }
        }
    }

    // ── expression evaluation ─────────────────────────────────────────

    fn eval(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x00 => Ok((Value::Int(0), p + 1)),
            0x01 => Ok((Value::Int(1), p + 1)),
            0xFF => Ok((Value::Int(u64::MAX), p + 1)),
            0x0A => Ok((Value::Int(b[p + 1] as u64), p + 2)),
            0x0B => Ok((Value::Int(u16::from_le_bytes([b[p + 1], b[p + 2]]) as u64), p + 3)),
            0x0C => Ok((
                Value::Int(u32::from_le_bytes([b[p + 1], b[p + 2], b[p + 3], b[p + 4]]) as u64),
                p + 5,
            )),
            0x0E => {
                let mut a = [0u8; 8];
                a.copy_from_slice(&b[p + 1..p + 9]);
                Ok((Value::Int(u64::from_le_bytes(a)), p + 9))
            }
            0x0D => {
                let mut q = p + 1;
                let mut s = String::new();
                while q < b.len() && b[q] != 0 {
                    s.push(b[q] as char);
                    q += 1;
                }
                Ok((Value::Str(s), q + 1))
            }
            0x11 => {
                // Buffer
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let (size, p2) = self.eval(f, p1)?;
                let n = size.as_int() as usize;
                let mut buf = Vec::with_capacity(n);
                buf.extend_from_slice(&b[p2..pkg_end]);
                buf.resize(n, 0);
                Ok((Value::Buffer(buf), pkg_end))
            }
            0x12 => {
                // Package
                let (pkg_end, p1) = pkg_length(b, p + 1);
                let num = b[p1] as usize;
                let mut q = p1 + 1;
                let mut elems = Vec::with_capacity(num);
                while q < pkg_end && elems.len() < num {
                    let (e, nq) = self.eval(f, q)?;
                    elems.push(obj(e));
                    q = nq;
                }
                while elems.len() < num {
                    elems.push(obj(Value::Uninit));
                }
                Ok((Value::Package(elems), pkg_end))
            }
            0x60..=0x67 => Ok((f.locals[(op - 0x60) as usize].borrow().clone(), p + 1)),
            0x68..=0x6E => {
                let i = (op - 0x68) as usize;
                let v = if i < f.args.len() { f.args[i].borrow().clone() } else { Value::Uninit };
                Ok((v, p + 1))
            }
            _ => self.eval_op(f, p),
        }
    }

    fn eval_op(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x70 => {
                // Store(src, SuperName)
                let (src, p1) = self.eval(f, p + 1)?;
                let (place, p2) = self.super_name(f, p1)?;
                if let Some(pl) = place {
                    self.store(&pl, src.clone())?;
                }
                Ok((src, p2))
            }
            0x71 => {
                // RefOf(SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let v = match place {
                    Some(Place::Obj(o)) => Value::Ref(Place::Obj(o)),
                    Some(pl) => Value::Ref(pl),
                    None => Value::Uninit,
                };
                Ok((v, p1))
            }
            0x72 | 0x74 | 0x77 | 0x79 | 0x7A | 0x7B | 0x7C | 0x7D | 0x7E | 0x7F => {
                // Binary op: a, b, target
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (tgt, p3) = self.super_name(f, p2)?;
                let x = a.as_int();
                let y = bb.as_int();
                let r = match op {
                    0x72 => x.wrapping_add(y),
                    0x74 => x.wrapping_sub(y),
                    0x77 => x.wrapping_mul(y),
                    0x79 => if y >= 64 { 0 } else { x << y },
                    0x7A => if y >= 64 { 0 } else { x >> y },
                    0x7B => x & y,
                    0x7C => !(x & y),
                    0x7D => x | y,
                    0x7E => !(x | y),
                    0x7F => x ^ y,
                    _ => unreachable!(),
                };
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p3))
            }
            0x78 => {
                // Divide(a, b, remainder_target, quotient_target) -> quotient
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (rem_t, p3) = self.super_name(f, p2)?;
                let (quo_t, p4) = self.super_name(f, p3)?;
                let x = a.as_int();
                let y = bb.as_int();
                if y == 0 {
                    return Err(String::from("Divide by zero"));
                }
                let q = x / y;
                let r = x % y;
                if let Some(pl) = rem_t {
                    self.store(&pl, Value::Int(r))?;
                }
                if let Some(pl) = quo_t {
                    self.store(&pl, Value::Int(q))?;
                }
                Ok((Value::Int(q), p4))
            }
            0x80 => {
                // Not(operand, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = !a.as_int();
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x75 | 0x76 => {
                // Increment / Decrement (SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let pl = place.ok_or_else(|| String::from("Incr/Decr needs target"))?;
                let cur = self.read_place(&pl)?.as_int();
                let r = if op == 0x75 { cur.wrapping_add(1) } else { cur.wrapping_sub(1) };
                self.store(&pl, Value::Int(r))?;
                Ok((Value::Int(r), p1))
            }
            0x90 | 0x91 => {
                // LAnd / LOr
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let r = if op == 0x90 {
                    (a.as_int() != 0) && (bb.as_int() != 0)
                } else {
                    (a.as_int() != 0) || (bb.as_int() != 0)
                };
                Ok((Value::Int(r as u64), p2))
            }
            0x92 => {
                // LNot, or combined LNotEqual/LLessEqual/LGreaterEqual
                let nb = b[p + 1];
                match nb {
                    0x93 | 0x94 | 0x95 => {
                        let (a, p1) = self.eval(f, p + 2)?;
                        let (bb, p2) = self.eval(f, p1)?;
                        let (x, y) = (a.as_int(), bb.as_int());
                        let r = match nb {
                            0x93 => x != y,      // LNotEqual
                            0x94 => !(x > y),    // LLessEqual
                            0x95 => !(x < y),    // LGreaterEqual
                            _ => unreachable!(),
                        };
                        Ok((Value::Int(r as u64), p2))
                    }
                    _ => {
                        let (a, p1) = self.eval(f, p + 1)?;
                        Ok((Value::Int((a.as_int() == 0) as u64), p1))
                    }
                }
            }
            0x93 | 0x94 | 0x95 => {
                // LEqual / LGreater / LLess
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (x, y) = (a.as_int(), bb.as_int());
                let r = match op {
                    0x93 => x == y,
                    0x94 => x > y,
                    0x95 => x < y,
                    _ => unreachable!(),
                };
                Ok((Value::Int(r as u64), p2))
            }
            0x88 => {
                // Index(source, index, target?) -> reference
                let (place, p1) = self.index_place(f, p + 1)?;
                // optional target
                let (tgt, p2) = self.super_name(f, p1)?;
                let refv = Value::Ref(place.clone());
                if let Some(pl) = tgt {
                    self.store(&pl, refv.clone())?;
                }
                Ok((refv, p2))
            }
            0x83 => {
                // DerefOf(operand)
                let (v, p1) = self.eval(f, p + 1)?;
                let out = match v {
                    Value::Ref(pl) => self.read_place(&pl)?,
                    other => other,
                };
                Ok((out, p1))
            }
            0x87 => {
                // SizeOf(SuperName)
                let (place, p1) = self.super_name(f, p + 1)?;
                let sz = match place {
                    Some(pl) => match self.read_place(&pl)? {
                        Value::Buffer(x) => x.len() as u64,
                        Value::Str(s) => s.len() as u64,
                        Value::Package(e) => e.len() as u64,
                        _ => 0,
                    },
                    None => 0,
                };
                Ok((Value::Int(sz), p1))
            }
            0x73 => {
                // Concatenate(a, b, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (tgt, p3) = self.super_name(f, p2)?;
                let r = concat(&a, &bb);
                if let Some(pl) = tgt {
                    self.store(&pl, r.clone())?;
                }
                Ok((r, p3))
            }
            0x84 => {
                // ConcatenateResTemplate(a, b, target) — ACPI 2.0.
                //
                // 1:1 from ACPICA `acpi_ex_concat_template` (exconcat.c):
                // find the end tag in both templates, join the parts before
                // it and append one new end tag with checksum 0 ("ignore").
                // An empty buffer is allowed and counts as a bare end tag.
                //
                // Firmware commonly builds an I2C HID device's `_CRS` from a
                // bus part and a GPIO part this way.
                let (a, p1) = self.eval(f, p + 1)?;
                let (bb, p2) = self.eval(f, p1)?;
                let (tgt, p3) = self.super_name(f, p2)?;
                let r = Value::Buffer(concat_res_template(&a, &bb));
                if let Some(pl) = tgt {
                    self.store(&pl, r.clone())?;
                }
                Ok((r, p3))
            }
            0x99 => {
                // ToInteger(operand, target)
                let (a, p1) = self.eval(f, p + 1)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = Value::Int(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, r.clone())?;
                }
                Ok((r, p2))
            }
            0x86 => {
                // Notify(object, value): nothing acts on it here, but the
                // host hears it — that is where a firmware hotkey surfaces.
                let b = f.body;
                let target = match b.get(p + 1) {
                    Some(0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F) => {
                        let (nref, _) = name_at(b, p + 1);
                        self.resolve(&f.scope, nref.rooted, nref.carets, &nref.segs)
                    }
                    _ => None,
                };
                let (_o, p1) = self.super_name(f, p + 1)?;
                let (v, p2) = self.eval(f, p1)?;
                if let Some(path) = target {
                    self.ec.notify(&path, v.as_int());
                }
                Ok((Value::Uninit, p2))
            }
            0x5B => self.eval_ext(f, p),
            // name-ish first byte -> NameString (method call or name/field read)
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => self.eval_name(f, p),
            other => Err(format!(
                "unhandled eval opcode {:#04x} at body+{:#x}",
                other, p
            )),
        }
    }

    fn eval_ext(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let b = f.body;
        let ext = b[p + 1];
        match ext {
            0x23 => {
                // Acquire(mutex, timeout-u16) -> bool (0 = acquired)
                let (_pl, p1) = self.super_name(f, p + 2)?;
                Ok((Value::Int(0), p1 + 2))
            }
            0x27 => {
                // Release(mutex)
                let (_pl, p1) = self.super_name(f, p + 2)?;
                Ok((Value::Uninit, p1))
            }
            0x12 => {
                // CondRefOf(SuperName, target) -> bool
                let (place, p1) = self.super_name_opt(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let found = place.is_some();
                if let (Some(pl), Some(src)) = (tgt, place) {
                    self.store(&pl, Value::Ref(src))?;
                }
                Ok((Value::Int(found as u64), p2))
            }
            0x28 => {
                // FromBCD(value, target)
                let (a, p1) = self.eval(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = from_bcd(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x29 => {
                // ToBCD(value, target)
                let (a, p1) = self.eval(f, p + 2)?;
                let (tgt, p2) = self.super_name(f, p1)?;
                let r = to_bcd(a.as_int());
                if let Some(pl) = tgt {
                    self.store(&pl, Value::Int(r))?;
                }
                Ok((Value::Int(r), p2))
            }
            0x21 | 0x22 => {
                // Stall(usec) = 0x21, Sleep(msec) = 0x22.
                //
                // Firmware uses these to wait for its own hardware, typically
                // between an EC write and a read. Not waiting reads too early
                // and returns the old value without any error.
                let (a, p1) = self.eval(f, p + 2)?;
                let ms = if b[p + 1] == 0x21 {
                    // Stall is in microseconds; round up so Stall(1) does
                    // not become zero.
                    ((a.as_int() + 999) / 1000) as u32
                } else {
                    a.as_int() as u32
                };
                // Cap: a DSDT must not stall us for minutes.
                if ms > 0 { self.ec.sleep_ms(ms.min(50)); }
                Ok((Value::Uninit, p1))
            }
            0x31 => {
                // DebugObj as a value (rare) — treat as 0.
                Ok((Value::Int(0), p + 2))
            }
            // Not implemented: IndexField needs an index/data register pair
            // and BankField a bank select; both are real semantics, so they
            // fail rather than being skipped.
            0x86 => Err(String::from(
                "IndexField im Methodenrumpf — braucht Index/Daten-Semantik (nicht gebaut)")),
            0x87 => Err(String::from(
                "BankField im Methodenrumpf — nicht gebaut")),
            other => Err(format!("unhandled ext eval opcode 5B {:#04x} at body+{:#x}", other, p)),
        }
    }

    /// Evaluate a NameString as a value: method invocation, name read, or field read.
    fn eval_name(&mut self, f: &Frame, p: usize) -> R<(Value, usize)> {
        let (nref, p1) = name_at(f.body, p);

        // ── Names provided by the operating system ───────────────────────
        //
        // `_OSI`, `_OS` and `_REV` are not in the DSDT (ACPI 6.5 §5.7).
        // Firmware uses them to find out which OS it runs under and branches
        // accordingly; every interpreter (ACPICA, Linux, Windows) provides
        // them. Some firmware calls `_OSI` already in the EC's `_REG`, so
        // without them the EC region is never enabled.
        if nref.carets == 0 && nref.segs.len() == 1 {
            match &nref.segs[0] {
                b"_OSI" => {
                    // Exactly one argument (the queried string).
                    let (arg, q) = self.eval(f, p1)?;
                    let yes = match &arg {
                        Value::Str(s) => osi_supported(s),
                        _ => false,
                    };
                    // ACPI: "Ones" is true, 0 is false.
                    return Ok((Value::Int(if yes { 0xFFFF_FFFF } else { 0 }), q));
                }
                b"_OS_" => {
                    // What Linux reports, deliberately: a DSDT reading
                    // something unknown here takes its oldest path.
                    return Ok((Value::Str(String::from("Microsoft Windows NT")), p1));
                }
                b"_REV" => {
                    // ACPI revision we can evaluate.
                    return Ok((Value::Int(2), p1));
                }
                _ => {}
            }
        }

        let path = self
            .resolve(&f.scope, nref.rooted, nref.carets, &nref.segs)
            .ok_or_else(|| format!("unresolved name {} (scope {})", segs_str(&nref.segs), path_str(&f.scope)))?;
        // Read first, then act: `node()` borrows `self`, and the arms below
        // call `&mut self` methods.
        enum Kind { Method(u8), Name(Value), Field, BufField(Obj, u64, u64), Other }
        let kind = match self.node(&path) {
            Some(Node::Method { flags, .. }) => Kind::Method(*flags),
            Some(Node::Name(v)) => Kind::Name(v.borrow().clone()),
            Some(Node::Field { .. }) => Kind::Field,
            Some(Node::BufferField { buf, bit_offset, bit_width }) => {
                Kind::BufField(buf.clone(), *bit_offset, *bit_width)
            }
            _ => Kind::Other,
        };
        match kind {
            Kind::Method(flags) => {
                let argc = (flags & 0x07) as usize;
                let mut args = Vec::with_capacity(argc);
                let mut q = p1;
                for _ in 0..argc {
                    let (v, nq) = self.eval(f, q)?;
                    args.push(obj(v));
                    q = nq;
                }
                let v = self.call_path(&path, args)?;
                Ok((v, q))
            }
            Kind::Name(v) => {
                if self.trace {
                    let d = describe(&v);
                    let n = path_str(&path);
                    self.ec.note(&format!("[aml]   trace {n} = {d}"));
                }
                Ok((v, p1))
            }
            Kind::Field => {
                let v = self.read_field(&path)?;
                if self.trace {
                    let d = describe(&v);
                    let n = path_str(&path);
                    self.ec.note(&format!("[aml]   trace {n} = {d} (field)"));
                }
                Ok((v, p1))
            }
            Kind::BufField(o, off, w) => {
                let v = o.borrow();
                let r = match &*v {
                    Value::Buffer(b) => buf_field_read(b, off, w),
                    _ => Value::Int(0),
                };
                Ok((r, p1))
            }
            Kind::Other => Ok((Value::Uninit, p1)),
        }
    }

    /// Parse a SuperName / Target. Returns None for the null target (0x00) and
    /// for the Debug object (writes are discarded).
    fn super_name(&mut self, f: &Frame, p: usize) -> R<(Option<Place>, usize)> {
        self.super_name_opt(f, p)
    }

    fn super_name_opt(&mut self, f: &Frame, p: usize) -> R<(Option<Place>, usize)> {
        let b = f.body;
        let op = b[p];
        match op {
            0x00 => Ok((None, p + 1)), // NullName target
            0x60..=0x67 => Ok((Some(Place::Obj(f.locals[(op - 0x60) as usize].clone())), p + 1)),
            0x68..=0x6E => {
                let i = (op - 0x68) as usize;
                let o = if i < f.args.len() { f.args[i].clone() } else { obj(Value::Uninit) };
                Ok((Some(Place::Obj(o)), p + 1))
            }
            0x88 => {
                let (pl, p1) = self.index_place(f, p + 1)?;
                // optional nested target of Index is ignored when used as a target
                let (_t, p2) = self.super_name(f, p1)?;
                Ok((Some(pl), p2))
            }
            0x83 => {
                // DerefOf used as a target -> the referenced place
                let (v, p1) = self.eval(f, p + 1)?;
                match v {
                    Value::Ref(pl) => Ok((Some(pl), p1)),
                    _ => Ok((None, p1)),
                }
            }
            0x5B if b[p + 1] == 0x31 => Ok((None, p + 2)), // DebugObj sink
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {
                let (nref, p1) = name_at(b, p);
                let path = self.resolve(&f.scope, nref.rooted, nref.carets, &nref.segs);
                match path {
                    Some(pp) => match self.node(&pp) {
                        Some(Node::Field { .. }) => Ok((Some(Place::Field(pp)), p1)),
                        Some(Node::Name(o)) => Ok((Some(Place::Obj(o.clone())), p1)),
                        Some(Node::BufferField { buf, bit_offset, bit_width }) => {
                            Ok((Some(Place::BufField(buf.clone(), *bit_offset, *bit_width)), p1))
                        }
                        _ => Ok((Some(Place::Field(pp)), p1)),
                    },
                    None => Ok((None, p1)),
                }
            }
            _ => Err(format!("bad SuperName opcode {:#04x} at body+{:#x}", op, p)),
        }
    }

    /// Parse `Index(source, index)` into a Place (without the optional target).
    fn index_place(&mut self, f: &Frame, p: usize) -> R<(Place, usize)> {
        let (src, p1) = self.eval(f, p)?;
        let (idx, p2) = self.eval(f, p1)?;
        let i = idx.as_int() as usize;
        let src = match src {
            Value::Ref(pl) => self.read_place(&pl)?,
            other => other,
        };
        let place = match src {
            Value::Package(elems) => {
                if i < elems.len() {
                    Place::Obj(elems[i].clone())
                } else {
                    return Err(format!("package index {} out of range {}", i, elems.len()));
                }
            }
            Value::Buffer(_) | Value::Str(_) => {
                // Need the underlying cell for write-back. Best-effort: re-resolve
                // not possible from a value copy, so wrap a throwaway cell.
                Place::BufIndex(obj(src), i)
            }
            _ => return Err(String::from("Index on non-indexable value")),
        };
        Ok((place, p2))
    }

    // ── places ────────────────────────────────────────────────────────

    /// `CreateBitField` / `CreateByteField` / `CreateWordField` /
    /// `CreateDWordField` / `CreateQWordField` / `CreateField`.
    ///
    /// ACPI 6.5 §19.6.20-25: a named buffer field, i.e. a bit range of an
    /// existing buffer with no storage of its own. Writing to it changes the
    /// buffer.
    ///
    /// The source is taken as a SuperName, not with `eval`: `eval` would
    /// return a copy of the buffer, and `INT1 = GNUM (GPDI)` would write
    /// into nothing, leaving e.g. the pin number in a `_CRS` template at 0.
    fn create_buffer_field(&mut self, f: &Frame, p: usize) -> R<usize> {
        let b = f.body;
        let (op, arg0) = if b[p] == 0x5B { (0x13u8, p + 2) } else { (b[p], p + 1) };

        let (src_place, p1) = self.super_name_opt(f, arg0)?;
        let buf = match src_place {
            Some(Place::Obj(o)) => o,
            // A source that is not a plain name (Index(...), a method
            // result) has no buffer to bind to. A throwaway buffer keeps the
            // run going.
            other => {
                let v = match other {
                    Some(pl) => self.read_place(&pl)?,
                    None => Value::Uninit,
                };
                self.ec.note("[aml]  Create*Field on a non-name source — writes will not stick");
                obj(v)
            }
        };

        let (idx, p2) = self.eval(f, p1)?;
        let (bit_offset, bit_width, p3) = match op {
            0x8D => (idx.as_int(), 1u64, p2),                 // CreateBitField
            0x8C => (idx.as_int() * 8, 8, p2),                // CreateByteField
            0x8B => (idx.as_int() * 8, 16, p2),               // CreateWordField
            0x8A => (idx.as_int() * 8, 32, p2),               // CreateDWordField
            0x8F => (idx.as_int() * 8, 64, p2),               // CreateQWordField
            _ => {
                // CreateField(source, bit-index, num-bits, name)
                let (n, q) = self.eval(f, p2)?;
                (idx.as_int(), n.as_int(), q)
            }
        };

        let (nref, p4) = name_at(b, p3);
        let path = self.def_path(&f.scope, &nref);
        self.dyn_nodes.insert(path, Node::BufferField { buf, bit_offset, bit_width });
        Ok(p4)
    }

    /// Run the table's deferred statements.
    ///
    /// ACPICA executes the term list while loading; we run exactly the
    /// statements that need an interpreter for it. Before `_REG` and `_INI`,
    /// since those use them.
    fn run_deferred(&mut self) -> Vec<(Path, Vec<u8>)> {
        let items: Vec<(Path, Vec<u8>)> = self.ns.deferred.clone();
        self.run_deferred_items(items, true)
    }

    /// Execute a list of deferred statements; returns those that failed.
    fn run_deferred_items(&mut self, items: Vec<(Path, Vec<u8>)>, loud: bool)
        -> Vec<(Path, Vec<u8>)>
    {
        let mut failed = Vec::new();
        for (scope, bytes) in items {
            let f = Frame {
                scope: scope.clone(), args: Vec::new(),
                locals: (0..8).map(|_| obj(Value::Uninit)).collect(),
                body: &bytes,
            };
            // Through the same dispatcher as a method body: `Create*Field`
            // and `OpRegion` are already handled there, and a second version
            // would be a second semantics.
            if let Err(e) = self.stmt(&f, 0, bytes.len()) {
                if loud {
                    let name = path_str(&scope);
                    self.ec.note(&format!("[aml]  deferred op in {name} failed: {e}"));
                    // "Unresolvable" is half the answer. The other half is
                    // whether the name exists at all, which separates
                    // "defined elsewhere in the tree" from "in no table we
                    // see"; only the latter is a load error.
                    if let Some(want) = e.strip_prefix("unresolved name ") {
                        let want = want.split(' ').next().unwrap_or("");
                        let seg = crate::value::seg(want);
                        let mut hits = 0usize;
                        for p in self.ns.nodes.keys() {
                            if p.last() == Some(&seg) && hits < 3 {
                                self.ec.note(&format!("[aml]    but {want} exists at {}", path_str(p)));
                                hits += 1;
                            }
                        }
                        if hits == 0 {
                            self.ec.note(&format!(
                                "[aml]    {want} is in NO table we loaded"));
                        }
                    }
                }
                failed.push((scope, bytes));
            }
        }
        failed
    }

    fn read_place(&mut self, pl: &Place) -> R<Value> {
        match pl {
            Place::Obj(o) => Ok(o.borrow().clone()),
            Place::Field(path) => self.read_field(path),
            Place::BufIndex(o, i) => {
                let v = o.borrow();
                match &*v {
                    Value::Buffer(b) => Ok(Value::Int(*b.get(*i).unwrap_or(&0) as u64)),
                    _ => Ok(Value::Int(0)),
                }
            }
            Place::BufField(o, off, w) => {
                let v = o.borrow();
                match &*v {
                    Value::Buffer(b) => Ok(buf_field_read(b, *off, *w)),
                    _ => Ok(Value::Int(0)),
                }
            }
        }
    }

    fn store(&mut self, pl: &Place, val: Value) -> R<()> {
        match pl {
            Place::Obj(o) => {
                *o.borrow_mut() = val;
                Ok(())
            }
            Place::Field(path) => self.write_field(path, val.as_int()),
            Place::BufIndex(o, i) => {
                let mut v = o.borrow_mut();
                if let Value::Buffer(b) = &mut *v {
                    if *i < b.len() {
                        b[*i] = val.as_int() as u8;
                    }
                }
                Ok(())
            }
            Place::BufField(o, off, w) => {
                let mut v = o.borrow_mut();
                if let Value::Buffer(b) = &mut *v {
                    buf_field_write(b, *off, *w, &val);
                }
                Ok(())
            }
        }
    }

    // ── field access via the EC region ────────────────────────────────

    fn region_byte(&mut self, space: u8, addr: u64) -> u8 {
        if space == 3 {
            return self.ec.read(addr as u8);
        }
        // Every other region space — SystemMemory(0), SystemIO(1),
        // PCI config(2), SMBus(4) — is backed by a scratch store and reads
        // 0 where nothing has been written. This keeps firmware handshakes
        // away from real ports, but a made-up 0 is indistinguishable from a
        // real one to the DSDT, so it is logged below.
        //
        // What we wrote takes precedence: firmware handshakes should read
        // back their own value.
        if let Some(v) = self.mem.get(&(space, addr)).copied() {
            return v;
        }
        // SystemMemory(0): ask the real window before making anything up.
        // Some firmware maps its EC into memory (e.g. at 0xFE800008)
        // instead of using the ports.
        if space == 0 {
            if let Some(v) = self.ec.mem_read(addr) {
                // Log with the address: values without their origin do not
                // show whether a contiguous block or one location is read.
                self.ec.note_num("[aml]   sysmem [", addr);
                self.ec.note_num("[aml]        ] -> ", v as u64);
                return v;
            }
        }
        // One line with everything, and only for the first few.
        //
        // A single `note` line, so a host that implements only `note` (not
        // `note_num`) still sees the address. A made-up value is information
        // once and noise afterwards: firmware reads such registers in loops.
        use core::sync::atomic::{AtomicU32, Ordering};
        static INVENTED: AtomicU32 = AtomicU32::new(0);
        let n = INVENTED.fetch_add(1, Ordering::Relaxed);
        if n < 4 {
            self.ec.note(&format!(
                "[aml]   region space={space} addr={addr:#x} not backed -> 0 (invented)"));
        } else if n == 4 {
            self.ec.note("[aml]   (further invented region reads silenced)");
        }
        0
    }

    fn set_region_byte(&mut self, space: u8, addr: u64, val: u8) {
        if space == 3 {
            self.ec.write(addr as u8, val);
            return;
        }
        // A write to a non-EC region goes to the scratch store and does not
        // reach the hardware. This is deliberate (writing arbitrary MMIO
        // could reprogram devices), but if the firmware drives a select
        // register here and then reads, it gets data for the wrong
        // selection, so it is logged.
        self.ec.note_num("[aml]   scratch write space=", space as u64);
        self.ec.note_num("[aml]        addr=", addr);
        self.ec.note_num("[aml]        val=", val as u64);
        self.mem.insert((space, addr), val);
    }

    fn read_field(&mut self, path: &Path) -> R<Value> {
        let (space, base, bit_off, bit_w) = self.field_geom(path)?;
        let mut val: u64 = 0;
        let mut produced = 0u64;
        let mut bit = bit_off;
        while produced < bit_w {
            let addr = base + bit / 8;
            let bit_in = bit % 8;
            let take = core::cmp::min(8 - bit_in, bit_w - produced);
            let raw = self.region_byte(space, addr) as u64;
            let chunk = (raw >> bit_in) & ((1u64 << take) - 1);
            val |= chunk << produced;
            produced += take;
            bit += take;
        }
        Ok(Value::Int(val))
    }

    fn write_field(&mut self, path: &Path, val: u64) -> R<()> {
        let (space, base, bit_off, bit_w) = self.field_geom(path)?;
        let mut written = 0u64;
        let mut bit = bit_off;
        while written < bit_w {
            let addr = base + bit / 8;
            let bit_in = bit % 8;
            let take = core::cmp::min(8 - bit_in, bit_w - written);
            let mask = ((1u64 << take) - 1) << bit_in;
            let chunk = ((val >> written) & ((1u64 << take) - 1)) << bit_in;
            let mut cur = self.region_byte(space, addr) as u64;
            cur = (cur & !mask) | (chunk & mask);
            self.set_region_byte(space, addr, cur as u8);
            written += take;
            bit += take;
        }
        Ok(())
    }

    /// FieldList of a run-time declaration, bit by bit — the same rule as
    /// `Loader::field_list`: field units are siblings of the region, not its
    /// children.
    fn dyn_field_list(&mut self, b: &[u8], region: &Path, start: usize, end: usize) {
        let mut p = start;
        let mut bit: u64 = 0;
        while p < end && p < b.len() {
            match b[p] {
                0x00 => {
                    // ReservedField / Offset(): the PkgLength value is a bit gap.
                    let (pe, p1) = pkg_length(b, p + 1);
                    bit += (pe - (p + 1)) as u64;
                    p = p1;
                }
                0x01 => p += 3,                         // AccessField
                0x02 => { let (_n, p1) = name_at(b, p + 1); p = p1; } // ConnectField
                0x03 => p += 4,                         // ExtendedAccessField
                _ => {
                    if p + 4 > b.len() { return; }
                    let mut sg: Seg = [0; 4];
                    sg.copy_from_slice(&b[p..p + 4]);
                    let (pe, p1) = pkg_length(b, p + 4);
                    let width = (pe - (p + 4)) as u64;
                    let mut fp = region.clone();
                    fp.pop();
                    fp.push(sg);
                    self.dyn_nodes.insert(
                        fp,
                        Node::Field { region: region.clone(), bit_offset: bit, bit_width: width },
                    );
                    bit += width;
                    p = p1;
                }
            }
        }
    }

    /// (region_space, region_byte_base, field_bit_offset, field_bit_width)
    fn field_geom(&self, path: &Path) -> R<(u8, u64, u64, u64)> {
        let (region, bit_offset, bit_width) = match self.node(path) {
            Some(Node::Field { region, bit_offset, bit_width }) => {
                (region.clone(), *bit_offset, *bit_width)
            }
            _ => return Err(format!("{} is not a field", path_str(path))),
        };
        let (space, offset) = match self.node(&region) {
            Some(Node::Region { space, offset, .. }) => (*space, *offset),
            _ => return Err(format!("region {} missing", path_str(&region))),
        };
        Ok((space, offset, bit_offset, bit_width))
    }
}

// ── free helpers ───────────────────────────────────────────────────────

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Uninit => "Uninit",
        Value::Int(_) => "Int",
        Value::Str(_) => "Str",
        Value::Buffer(_) => "Buffer",
        Value::Package(_) => "Package",
        Value::Ref(_) => "Ref",
    }
}

fn concat(a: &Value, b: &Value) -> Value {
    // Strings concatenate as strings; otherwise produce a buffer.
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => {
            let mut s = x.clone();
            s.push_str(y);
            Value::Str(s)
        }
        (Value::Str(x), other) => {
            let mut s = x.clone();
            s.push_str(&val_to_string(other));
            Value::Str(s)
        }
        (other, Value::Str(y)) => {
            let mut s = val_to_string(other);
            s.push_str(y);
            Value::Str(s)
        }
        _ => {
            let mut buf = to_bytes(a);
            buf.extend_from_slice(&to_bytes(b));
            Value::Buffer(buf)
        }
    }
}

fn val_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(n) => {
            // single-char if it's a small ASCII code (ISTR builds from NIST chars)
            if *n >= 0x20 && *n < 0x7f {
                let mut s = String::new();
                s.push(*n as u8 as char);
                s
            } else {
                let mut s = String::new();
                let mut x = *n;
                if x == 0 {
                    s.push('0');
                } else {
                    let mut tmp = [0u8; 20];
                    let mut i = 0;
                    while x > 0 {
                        tmp[i] = b'0' + (x % 10) as u8;
                        x /= 10;
                        i += 1;
                    }
                    while i > 0 {
                        i -= 1;
                        s.push(tmp[i] as char);
                    }
                }
                s
            }
        }
        Value::Buffer(b) => {
            let mut s = String::new();
            for &c in b {
                if c == 0 {
                    break;
                }
                s.push(c as char);
            }
            s
        }
        _ => String::new(),
    }
}

fn to_bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::Buffer(b) => b.clone(),
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(n) => n.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

fn to_bcd(mut n: u64) -> u64 {
    let mut r = 0u64;
    let mut shift = 0;
    while n > 0 {
        r |= (n % 10) << (shift * 4);
        n /= 10;
        shift += 1;
    }
    r
}

fn from_bcd(n: u64) -> u64 {
    let mut r = 0u64;
    let mut mul = 1u64;
    let mut x = n;
    while x > 0 {
        r += (x & 0x0F) * mul;
        x >>= 4;
        mul *= 10;
    }
    r
}

// ── NameString parsing (mirrors the loader) ─────────────────────────────

pub struct NRef {
    pub rooted: bool,
    pub carets: usize,
    pub segs: Vec<Seg>,
}

fn name_at(b: &[u8], mut p: usize) -> (NRef, usize) {
    let mut rooted = false;
    let mut carets = 0;
    if b[p] == 0x5C {
        rooted = true;
        p += 1;
    } else {
        while b[p] == 0x5E {
            carets += 1;
            p += 1;
        }
    }
    let mut segs: Vec<Seg> = Vec::new();
    match b[p] {
        0x00 => p += 1,
        0x2E => {
            p += 1;
            segs.push(seg_at(b, p));
            segs.push(seg_at(b, p + 4));
            p += 8;
        }
        0x2F => {
            p += 1;
            let count = b[p] as usize;
            p += 1;
            for i in 0..count {
                segs.push(seg_at(b, p + i * 4));
            }
            p += count * 4;
        }
        _ => {
            segs.push(seg_at(b, p));
            p += 4;
        }
    }
    (NRef { rooted, carets, segs }, p)
}

fn seg_at(b: &[u8], p: usize) -> Seg {
    let mut s: Seg = [0; 4];
    s.copy_from_slice(&b[p..p + 4]);
    s
}

/// Answer to `_OSI("…")`.
///
/// True for the Windows strings: almost all firmware asks for them, and
/// answering no selects the DSDT's oldest path or none at all. Everything
/// else is false, in particular "Linux": Linux itself dropped it because
/// firmware then takes broken special paths.
///
/// Deliberately without an upper bound on the year; a fixed list would be
/// outdated by the next firmware.
fn osi_supported(s: &str) -> bool {
    s.starts_with("Windows ")
}

fn segs_str(segs: &[Seg]) -> String {
    let mut s = String::new();
    for (i, sg) in segs.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        for &c in sg {
            s.push(c as char);
        }
    }
    s
}

fn pkg_length(b: &[u8], p: usize) -> (usize, usize) {
    let lead = b[p];
    let extra = (lead >> 6) as usize;
    if extra == 0 {
        ((p + (lead & 0x3F) as usize), p + 1)
    } else {
        let mut len = (lead & 0x0F) as usize;
        for i in 0..extra {
            len |= (b[p + 1 + i] as usize) << (4 + i * 8);
        }
        (p + len, p + 1 + extra)
    }
}

/// After a taken If-then block, skip a trailing Else block if present.
fn skip_else(b: &[u8], p: usize) -> usize {
    if p < b.len() && b[p] == 0xA1 {
        let (end, _p1) = pkg_length(b, p + 1);
        end
    } else {
        p
    }
}

/// Length of a resource template up to its end tag.
///
/// ACPICA `acpi_ut_get_resource_end_tag`: walk the descriptors and stop at
/// small type 0x0F. An empty buffer counts as a template with only an end
/// tag, i.e. length 0.
fn resource_body_len(b: &[u8]) -> usize {
    let mut i = 0usize;
    while i < b.len() {
        let tag = b[i];
        if tag & 0x80 == 0 {
            if (tag >> 3) & 0x0F == 0x0F {
                return i; // End tag: the length is everything before it
            }
            i += 1 + (tag & 0x07) as usize;
        } else {
            if i + 3 > b.len() {
                break;
            }
            let len = (b[i + 1] as usize) | ((b[i + 2] as usize) << 8);
            i += 3 + len;
        }
    }
    // No end tag found: everything counts as the body.
    b.len().min(i)
}

/// Read bits `[off, off+width)` from a buffer, LSB first within each byte
/// (ACPI 6.5 §19.6.20 ff.). Up to 64 bits the result is an integer, above
/// that a buffer.
fn buf_field_read(b: &[u8], off: u64, width: u64) -> Value {
    if width == 0 {
        return Value::Int(0);
    }
    if width <= 64 {
        let mut v = 0u64;
        for i in 0..width {
            let bit = off + i;
            let byte = (bit / 8) as usize;
            let cur = b.get(byte).copied().unwrap_or(0);
            if (cur >> (bit % 8)) & 1 != 0 {
                v |= 1u64 << i;
            }
        }
        return Value::Int(v);
    }
    let n = ((width + 7) / 8) as usize;
    let mut out = vec![0u8; n];
    for i in 0..width {
        let bit = off + i;
        let byte = (bit / 8) as usize;
        let cur = b.get(byte).copied().unwrap_or(0);
        if (cur >> (bit % 8)) & 1 != 0 {
            out[(i / 8) as usize] |= 1u8 << (i % 8);
        }
    }
    Value::Buffer(out)
}

/// Write the same bits. The buffer does not grow; whatever lies outside is
/// dropped, as in ACPICA.
fn buf_field_write(b: &mut [u8], off: u64, width: u64, val: &Value) {
    let src: Vec<u8> = match val {
        Value::Buffer(v) => v.clone(),
        Value::Str(s) => s.as_bytes().to_vec(),
        other => other.as_int().to_le_bytes().to_vec(),
    };
    for i in 0..width {
        let bit = off + i;
        let byte = (bit / 8) as usize;
        if byte >= b.len() {
            break;
        }
        let sbyte = (i / 8) as usize;
        let sbit = if sbyte < src.len() { (src[sbyte] >> (i % 8)) & 1 } else { 0 };
        let mask = 1u8 << (bit % 8);
        if sbit != 0 {
            b[byte] |= mask;
        } else {
            b[byte] &= !mask;
        }
    }
}

/// A short name for a value, for the trace.
fn describe(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("{n:#x}"),
        Value::Str(s) => format!("\"{s}\""),
        Value::Buffer(b) => format!("Buffer[{}]", b.len()),
        Value::Package(e) => format!("Package[{}]", e.len()),
        Value::Uninit => String::from("Uninit"),
        Value::Ref(_) => String::from("Ref"),
    }
}

fn concat_res_template(a: &Value, b: &Value) -> Vec<u8> {
    let empty: Vec<u8> = Vec::new();
    let ab = match a { Value::Buffer(v) => v, _ => &empty };
    let bb = match b { Value::Buffer(v) => v, _ => &empty };
    let l0 = resource_body_len(ab);
    let l1 = resource_body_len(bb);
    let mut out = Vec::with_capacity(l0 + l1 + 2);
    out.extend_from_slice(&ab[..l0]);
    out.extend_from_slice(&bb[..l1]);
    out.push(0x79); // ACPI_RESOURCE_NAME_END_TAG | 1
    out.push(0x00); // checksum 0 = "ignore"
    out
}

// ── General access: find devices and evaluate methods ─────────────────
//
// What a bus driver needs: search devices by `_HID`/`_CID`, query `_STA`,
// evaluate `_CRS`/`_DSM`.

/// An initialised interpreter a caller can query repeatedly.
///
/// `read_battery` builds its own and runs a fixed sequence; a device search
/// needs one that persists, since a fresh interpreter per evaluation would
/// rerun `_REG`/`_INI` for every query.
pub struct Machine<'a> {
    it: Interp<'a>,
}

impl<'a> Machine<'a> {
    pub fn new(ns: &'a Namespace, ec: &'a mut dyn Ec) -> Machine<'a> {
        Machine {
            it: Interp { ns, dyn_nodes: BTreeMap::new(), ec, depth: 0, trace: false, mem: BTreeMap::new() },
        }
    }

    /// Evaluate the predicate of a deferred `If` block and return which
    /// branch applies, as a byte range within `bytes`.
    ///
    /// The interpreter does nothing more here. The branch contains
    /// declarations (`Method`, `Name`, `Device`, `OperationRegion`), which
    /// belong to the loader; handling them here too would be a second
    /// version of the same semantics.
    pub fn taken_branch(&mut self, scope: &Path, bytes: &[u8]) -> Option<(usize, usize)> {
        if bytes.first() != Some(&0xA0) { return None; }
        let f = Frame {
            scope: scope.clone(), args: Vec::new(),
            locals: (0..8).map(|_| obj(Value::Uninit)).collect(),
            body: bytes,
        };
        let (pkg_end, p1) = pkg_length(bytes, 1);
        let (cond, p2) = match self.it.eval(&f, p1) {
            Ok(v) => v,
            Err(e) => {
                let n = path_str(scope);
                self.it.ec.note(&format!("[aml]  scope-If in {n}: {e}"));
                return None;
            }
        };
        if cond.as_int() != 0 {
            return Some((p2, pkg_end));
        }
        // Otherwise the Else branch, if there is one.
        if pkg_end < bytes.len() && bytes[pkg_end] == 0xA1 {
            let (else_end, e1) = pkg_length(bytes, pkg_end + 1);
            return Some((e1, else_end.min(bytes.len())));
        }
        None
    }

    /// `_REG`, then `_INI` — the order of ACPICA's
    /// `acpi_initialize_objects`. Without it, firmware whose regions are not
    /// yet enabled answers with its initial values.
    pub fn init(&mut self) {
        self.it.ec.note("[aml]  phase deferred table ops");
        let failed = self.it.run_deferred();
        self.it.ec.note("[aml]  phase _REG");
        let _ = self.it.register_ec_regions();
        self.it.ec.note("[aml]  phase _INI");
        let n = self.it.run_ini_methods();
        self.it.ec.note_num("[aml]  _INI methods run: ", n as u64);

        // Retry what failed the first time.
        //
        // ACPICA evaluates an operation region's operands on first access
        // (`acpi_ds_eval_region_operands`), i.e. after `_REG` and `_INI` at
        // the earliest. We run them at startup, which is too early when the
        // base is a name only `_INI` sets. A second attempt afterwards costs
        // nothing and covers that case.
        if !failed.is_empty() {
            let n = failed.len();
            let still = self.it.run_deferred_items(failed, true);
            self.it.ec.note(&format!(
                "[aml]  deferred retry after _INI: {} of {n} now ok",
                n - still.len()));
        }
    }

    pub fn has(&self, p: &Path) -> bool {
        self.it.has(p)
    }

    /// Diagnostic channel — same path as the interpreter's notes.
    pub fn note(&mut self, s: &str) {
        self.it.ec.note(s);
    }

    pub fn call(&mut self, p: &Path, args: Vec<Obj>) -> R<Value> {
        self.it.call_path(p, args)
    }

    /// Get a node's value: a `Name` yields its content, a `Method` is
    /// executed.
    ///
    /// `_HID` may be a method (e.g. `Method (_HID) { Return ("SYNA30A1") }`),
    /// so every node kind must be handled.
    pub fn value_of(&mut self, p: &Path) -> R<Value> {
        match self.it.ns.get(p) {
            Some(Node::Name(o)) => Ok(o.borrow().clone()),
            Some(Node::Method { .. }) => self.it.call_path(p, Vec::new()),
            Some(_) => Err(String::from("node is not a value")),
            None => Err(String::from("no such node")),
        }
    }

    /// Evaluate a child of the device, e.g. `_CRS`.
    pub fn eval_child(&mut self, dev: &Path, name: &str) -> R<Value> {
        let mut p = dev.clone();
        p.push(crate::value::seg(name));
        if !self.has(&p) {
            return Err(String::from("absent"));
        }
        self.value_of(&p)
    }

    /// `_STA` per ACPI 6.5 §6.3.7: without the method the device counts as
    /// present. Otherwise bit 0 = present, bit 3 = functional.
    ///
    /// Or, not and: Linux `acpi_device_is_present` (scan.c) uses
    /// `adev->status.present || adev->status.functional`. An and would be
    /// stricter than the reference and exclude devices the firmware
    /// reports as usable.
    pub fn device_present(&mut self, dev: &Path) -> bool {
        self.device_status(dev).map(|f| f & 0x01 != 0 || f & 0x08 != 0).unwrap_or(true)
    }

    /// The same method again, but traced: every name read and its value go
    /// to the log.
    ///
    /// For answers that cannot be right: a computed zero says nothing, the
    /// values it came from say everything.
    pub fn call_traced(&mut self, p: &Path, args: Vec<Obj>) -> R<Value> {
        self.it.trace = true;
        let r = self.it.call_path(p, args);
        self.it.trace = false;
        r
    }

    /// The raw `_STA` value, or `None` if there is none.
    ///
    /// "Absent" alone does not say whether the firmware meant 0 or the
    /// interpreter supplied one.
    pub fn device_status(&mut self, dev: &Path) -> Option<u64> {
        let mut p = dev.clone();
        p.push(crate::value::seg("_STA"));
        if !self.has(&p) {
            return None;
        }
        match self.it.call_path(&p, Vec::new()) {
            Ok(v) => Some(v.as_int()),
            Err(_) => Some(0),
        }
    }

    /// All IDs of a device: `_HID` first, then each from `_CID` (which may
    /// be a package of several).
    pub fn device_ids(&mut self, dev: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for name in ["_HID", "_CID"] {
            if let Ok(v) = self.eval_child(dev, name) {
                push_ids(&v, &mut out);
            }
        }
        // Drop duplicates: a device may carry the same ID in `_HID` and
        // `_CID` (the AMD GPIO block does), and reporting it twice looks
        // like two devices.
        out.dedup();
        let mut uniq: Vec<String> = Vec::new();
        for id in out {
            if !uniq.contains(&id) { uniq.push(id); }
        }
        uniq
    }
}

/// An ID can be a string, an EisaId integer, or a package of both
/// (ACPI 6.5 §6.1.2 `_CID`).
fn push_ids(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Str(s) => out.push(s.clone()),
        Value::Int(n) => {
            let s = eisa_str(*n);
            if !s.is_empty() {
                out.push(s);
            }
        }
        Value::Package(e) => {
            for o in e {
                push_ids(&o.borrow(), out);
            }
        }
        _ => {}
    }
}

/// Unpack an EisaId — the inverse of [`eisa_id`].
///
/// Decoding instead of comparing against candidates means every ID is
/// available as a name and can be reported, including ones nobody looked
/// for.
pub fn eisa_str(n: u64) -> String {
    let n = n & 0xFFFF_FFFF;
    // Reverse byte swap (eisa_id stores little-endian).
    let s = ((n >> 24) & 0xFF) | (((n >> 16) & 0xFF) << 8) | (((n >> 8) & 0xFF) << 16) | ((n & 0xFF) << 24);
    let m = |sh: u32| -> u8 { (((s >> sh) & 0x1F) as u8) + b'@' };
    let (m0, m1, m2) = (m(26), m(21), m(16));
    if !(m0.is_ascii_uppercase() && m1.is_ascii_uppercase() && m2.is_ascii_uppercase()) {
        return String::new();
    }
    let hex = |v: u64| -> char {
        let v = (v & 0xF) as u8;
        if v < 10 { (b'0' + v) as char } else { (b'A' + v - 10) as char }
    };
    let mut out = String::new();
    out.push(m0 as char);
    out.push(m1 as char);
    out.push(m2 as char);
    out.push(hex(s >> 12));
    out.push(hex(s >> 8));
    out.push(hex(s >> 4));
    out.push(hex(s));
    out
}

/// Every device in the namespace that carries an ID at all.
///
/// A "device" here is the parent of an `_HID` or `_CID` node. The IDs are
/// evaluated only when queried: running `_HID` as a method costs, and most
/// tables have dozens.
pub fn devices_with_ids(ns: &Namespace) -> Vec<Path> {
    let mut out: Vec<Path> = Vec::new();
    for path in ns.nodes.keys() {
        let last = match path.last() {
            Some(s) => *s,
            None => continue,
        };
        if last != crate::value::seg("_HID") && last != crate::value::seg("_CID") {
            continue;
        }
        let mut dev = path.clone();
        dev.pop();
        if !out.contains(&dev) {
            out.push(dev);
        }
    }
    out
}
