//! AML table loader: bytes -> namespace. Registers all named objects, defers
//! method bodies, and skips control-flow blocks at scope level (the battery
//! objects are all unconditional definitions).

use crate::value::{obj, seg, Obj, Path, Seg, Value};
use crate::{Namespace, Node};
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};

pub fn load_table(table: &[u8]) -> Result<Namespace, String> {
    let mut ns = Namespace { nodes: BTreeMap::new(), deferred: Vec::new() };
    predefine_root(&mut ns);
    load_into(&mut ns, table)?;
    Ok(ns)
}

/// Einen Ausschnitt roher AML-Bytes als Termliste in einen Scope laden.
///
/// Gebraucht fuer den genommenen Zweig eines `If` auf Scope-Ebene: die
/// BEDINGUNG entscheidet der Interpreter, die DEKLARATIONEN darin gehoeren
/// hierher.
pub fn load_range(ns: &mut Namespace, bytes: &[u8], start: usize, end: usize, scope: Path)
    -> Result<(), String>
{
    let mut ld = Loader { b: bytes, ns };
    ld.term_list(scope, start, end)
}

/// Eine WEITERE Tabelle in denselben Namespace legen.
///
/// Eine Firmware verteilt ihre Deklarationen ueber die DSDT und beliebig
/// viele SSDTs, und sie bilden EINEN Namespace — Linux laedt sie
/// entsprechend alle (`acpi_tb_load_namespace`). Wer nur die DSDT liest,
/// dem fehlen Namen, die woanders stehen: auf einem Lenovo IdeaPad die
/// Basis der Region mit den Freigabebits der I2C-Controller (`FRTB`), und
/// ohne sie meldet `_STA` beider Controller „abgeschaltet".
pub fn load_into(ns: &mut Namespace, table: &[u8]) -> Result<(), String> {
    if table.len() < 36 || &table[0..4] != b"DSDT" && &table[0..4] != b"SSDT" {
        return Err(format!("not a DSDT/SSDT: {:?}", &table[0..4.min(table.len())]));
    }
    let len = u32::from_le_bytes([table[4], table[5], table[6], table[7]]) as usize;
    let end = if len >= 36 && len <= table.len() { len } else { table.len() };
    let mut ld = Loader { b: table, ns };
    ld.term_list(Vec::new(), 36, end)
}

/// Die Namen, die das BETRIEBSSYSTEM mitbringt — nicht die Tabelle.
///
/// ACPICA legt sie in `acpi_ns_root_initialize` (nsaccess.c) an, und das ist
/// kein Schoenheitsdetail: eine Firmware fragt
/// `If (CondRefOf (\_OSI, Local0))`, BEVOR sie `_OSI` benutzt. Wer die
/// Namen nur beim AUFRUF abfaengt, sagt dort Nein — und die Tabelle nimmt
/// dann ihren Pfad fuer ein Betriebssystem von vor 2001. Auf der
/// HP-Tabelle blieb `OSYS` damit auf 0x07D0, und das `_CRS` des Touchpads
/// gab statt Bus + Interrupt nur den Interrupt zurueck.
///
/// Die Scope-Namen (`_SB_`, `_TZ_`, …) legt ACPICA ebenfalls an; die
/// deklariert jede Tabelle selbst, also bleiben sie hier weg.
fn predefine_root(ns: &mut Namespace) {
    // `_OSI` ist bei ACPICA eine Methode; wir fangen den Aufruf in
    // `eval_name` ab, brauchen hier also nur die PRAESENZ.
    ns.nodes.insert(alloc::vec![seg("_OSI")], Node::Other);
    // `_GL_` ist der globale Mutex (ACPI 6.5 §5.7.1). Unser `Acquire` ist
    // ein No-op, aber `super_name` muss den Namen finden.
    ns.nodes.insert(alloc::vec![seg("_GL_")], Node::Other);
    // `_OS_` und `_REV` sind Namen mit Werten. `eval_name` antwortet
    // ohnehin; der Knoten macht sie fuer `CondRefOf` sichtbar.
    ns.nodes.insert(
        alloc::vec![seg("_OS_")],
        Node::Name(obj(Value::Str(alloc::string::String::from("Microsoft Windows NT")))),
    );
    ns.nodes.insert(alloc::vec![seg("_REV")], Node::Name(obj(Value::Int(2))));
}

struct Loader<'a> {
    b: &'a [u8],
    ns: &'a mut Namespace,
}

/// A parsed NameString.
struct NameRef {
    rooted: bool,
    carets: usize,
    segs: Vec<Seg>,
}

impl<'a> Loader<'a> {
    fn term_list(&mut self, scope: Path, start: usize, end: usize) -> Result<(), String> {
        let mut p = start;
        while p < end {
            p = self.term(&scope, p, end)?;
        }
        Ok(())
    }

    /// Process one scope-level term starting at `p`, return the next position.
    fn term(&mut self, scope: &Path, mut p: usize, end: usize) -> Result<usize, String> {
        let op = self.b[p];
        p += 1;
        match op {
            0x08 => {
                // NameOp NameString DataRefObject
                let (name, np) = self.name_ref(p);
                p = np;
                let target = self.def_path(scope, &name);
                let (val, vp) = self.data_object(p, end)?;
                p = vp;
                self.ns.nodes.insert(target, Node::Name(val));
            }
            0x06 => {
                // AliasOp NameString NameString — register alias target as Other.
                let (_a, p1) = self.name_ref(p);
                let (b, p2) = self.name_ref(p1);
                p = p2;
                let t = self.def_path(scope, &b);
                self.ns.nodes.entry(t).or_insert(Node::Other);
            }
            0x10 => {
                // ScopeOp PkgLength NameString TermList
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let target = self.def_path(scope, &name);
                self.ns.nodes.entry(target.clone()).or_insert(Node::Scope);
                self.term_list(target, p2, pkg_end)?;
                p = pkg_end;
            }
            0x14 => {
                // MethodOp PkgLength NameString MethodFlags TermList(body deferred)
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let flags = self.b[p2];
                let body = self.b[p2 + 1..pkg_end].to_vec();
                let target = self.def_path(scope, &name);
                self.ns.nodes.insert(
                    target,
                    Node::Method { flags, body, scope: scope.clone() },
                );
                p = pkg_end;
            }
            0x15 => {
                // ExternalOp NameString ObjectType ArgCount
                let (name, p1) = self.name_ref(p);
                p = p1 + 2;
                let t = self.def_path(scope, &name);
                self.ns.nodes.entry(t).or_insert(Node::Other);
            }
            0x8A | 0x8B | 0x8C | 0x8D | 0x8F => {
                // CreateDWord/Word/Byte/Bit/QWordField(source, index, NameString)
                let op_start = p - 1;
                let p1 = self.skip_term_arg(scope, p)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                let (name, p3) = self.name_ref(p2);
                p = p3;
                let t = self.def_path(scope, &name);
                // Platzhalter, damit der Name aufloesbar ist, BEVOR die
                // aufgehobene Anweisung laeuft; sie ersetzt ihn dann.
                self.ns.nodes.insert(t, Node::Other);
                let bytes = self.b[op_start..p].to_vec();
                self.ns.deferred.push((scope.clone(), bytes));
            }
            0xA0 | 0xA1 | 0xA2 => {
                // If / Else / While auf SCOPE-Ebene: aufheben und beim
                // Anlauf AUSFUEHREN.
                //
                // Hier stand „skip the whole block (the battery objects are
                // never conditionally defined)". Fuer den Akku stimmte das.
                // Fuer alles andere ist es ein Loch im Namespace: ACPICA
                // FUEHRT die Termliste einer Tabelle beim Laden aus
                // (`acpi_ns_execute_table`), ein `If` ist dort eine
                // Verzweigung und kein Text. Was darin deklariert wird,
                // existiert fuer einen Ueberspringer nicht.
                //
                // Auf Florians IdeaPad haengt daran `FRTB` — die Basis der
                // Region mit den Freigabebits der I2C-Controller —, und auf
                // Intel-Tabellen die `_HID` der I2C-Controller selbst.
                //
                // Ein `Else` gehoert zu seinem `If`: beide zusammen
                // aufheben, sonst entscheidet der Interpreter ohne
                // Gegenstueck.
                let op_start = p - 1;
                let (pkg_end, _p1) = self.pkg_length(p);
                p = pkg_end;
                if self.b[op_start] == 0xA0 && p < end && self.b[p] == 0xA1 {
                    let (else_end, _) = self.pkg_length(p + 1);
                    p = else_end;
                }
                if self.b[op_start] != 0xA1 {
                    let bytes = self.b[op_start..p].to_vec();
                    self.ns.deferred.push((scope.clone(), bytes));
                }
            }
            0x5B => {
                let ext = self.b[p];
                p += 1;
                p = self.ext_term(scope, ext, p, end)?;
            }
            // Bare data tokens shouldn't appear at scope level; if they do,
            // they're harmless no-ops we can skip.
            0x00 | 0x01 | 0xFF => {}
            other => {
                return Err(format!(
                    "unhandled scope opcode {:#04x} at {:#x} (scope {})",
                    other,
                    p - 1,
                    crate::value::path_str(scope)
                ));
            }
        }
        Ok(p)
    }

    fn ext_term(&mut self, scope: &Path, ext: u8, mut p: usize, _end: usize) -> Result<usize, String> {
        match ext {
            0x01 => {
                // MutexOp NameString SyncFlags
                let (name, p1) = self.name_ref(p);
                p = p1 + 1;
                let t = self.def_path(scope, &name);
                self.ns.nodes.insert(t, Node::Other);
            }
            0x02 => {
                // EventOp NameString
                let (name, p1) = self.name_ref(p);
                p = p1;
                let t = self.def_path(scope, &name);
                self.ns.nodes.insert(t, Node::Other);
            }
            0x80 => {
                // OpRegionOp NameString RegionSpace RegionOffset RegionLen.
                // Offset/Len are TermArgs (not PkgLength-delimited). For the
                // battery we only need EmbeddedControl regions, whose offset is
                // a constant; other regions may have computed offsets we just
                // skip past.
                let op_start = p - 2; // 0x5B 0x80
                let (name, p1) = self.name_ref(p);
                let space = self.b[p1];
                let (offset, p2, off_lit) = self.region_arg(scope, p1 + 1)?;
                let (len, p3, len_lit) = self.region_arg(scope, p2)?;
                p = p3;
                let t = self.def_path(scope, &name);
                self.ns.nodes.insert(t, Node::Region { space, offset, len });
                if !off_lit || !len_lit {
                    // GERECHNETE Basis. Hier stand frueher schlicht 0, und
                    // damit lasen alle Felder der Region ab Adresse null.
                    // Auf Florians IdeaPad sind das `IC0E`/`IC3E` — die
                    // Freigabebits der I2C-Controller —, und ihre erfundene
                    // 0 liess das `_STA` beider Controller „abgeschaltet"
                    // melden, obwohl beide laufen.
                    //
                    // ACPICA wertet Adresse und Laenge erst beim AUSFUEHREN
                    // aus (`acpi_ds_eval_region_operands`). Unser
                    // Interpreter tut das im Methodenrumpf laengst; hier
                    // wird die Anweisung dafuer aufgehoben.
                    let bytes = self.b[op_start..p].to_vec();
                    self.ns.deferred.push((scope.clone(), bytes));
                }
            }
            0x81 => {
                // FieldOp PkgLength NameString FieldFlags FieldList
                let (pkg_end, p1) = self.pkg_length(p);
                let (region_name, p2) = self.name_ref(p1);
                let region = self.def_path(scope, &region_name);
                let _flags = self.b[p2];
                self.field_list(scope, &region, p2 + 1, pkg_end)?;
                p = pkg_end;
            }
            0x82 => {
                // DeviceOp PkgLength NameString TermList
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let target = self.def_path(scope, &name);
                self.ns.nodes.entry(target.clone()).or_insert(Node::Scope);
                self.term_list(target, p2, pkg_end)?;
                p = pkg_end;
            }
            0x83 => {
                // ProcessorOp PkgLength NameString ProcID PblkAddr PblkLen TermList
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let target = self.def_path(scope, &name);
                self.ns.nodes.entry(target.clone()).or_insert(Node::Scope);
                // ProcID(1) + PblkAddr(4) + PblkLen(1) = 6 bytes
                self.term_list(target, p2 + 6, pkg_end)?;
                p = pkg_end;
            }
            0x84 => {
                // PowerResOp PkgLength NameString SystemLevel ResourceOrder TermList
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let target = self.def_path(scope, &name);
                self.ns.nodes.entry(target.clone()).or_insert(Node::Scope);
                // SystemLevel(1) + ResourceOrder(2) = 3 bytes
                self.term_list(target, p2 + 3, pkg_end)?;
                p = pkg_end;
            }
            0x85 => {
                // ThermalZoneOp PkgLength NameString TermList
                let (pkg_end, p1) = self.pkg_length(p);
                let (name, p2) = self.name_ref(p1);
                let target = self.def_path(scope, &name);
                self.ns.nodes.entry(target.clone()).or_insert(Node::Scope);
                self.term_list(target, p2, pkg_end)?;
                p = pkg_end;
            }
            0x86 => {
                // IndexFieldOp PkgLength NameString NameString FieldFlags FieldList
                // Not needed for the battery (flat fields); skip the block.
                let (pkg_end, _p1) = self.pkg_length(p);
                p = pkg_end;
            }
            0x87 => {
                // BankFieldOp PkgLength ... — skip.
                let (pkg_end, _p1) = self.pkg_length(p);
                p = pkg_end;
            }
            0x13 => {
                // CreateFieldOp source bit-index num-bits NameString
                let op_start = p - 2; // 0x5B 0x13
                let p1 = self.skip_term_arg(scope, p)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                let p3 = self.skip_term_arg(scope, p2)?;
                let (name, p4) = self.name_ref(p3);
                p = p4;
                let t = self.def_path(scope, &name);
                self.ns.nodes.insert(t, Node::Other);
                let bytes = self.b[op_start..p].to_vec();
                self.ns.deferred.push((scope.clone(), bytes));
            }
            other => {
                return Err(format!("unhandled ext opcode 5B {:#04x} at {:#x}", other, p - 1));
            }
        }
        Ok(p)
    }

    /// Parse a FieldList, registering each NamedField at its accumulated bit
    /// offset. Reserved/Offset/Access entries advance or annotate the cursor.
    fn field_list(&mut self, _scope: &Path, region: &Path, start: usize, end: usize) -> Result<(), String> {
        let mut p = start;
        let mut bit: u64 = 0;
        while p < end {
            match self.b[p] {
                0x00 => {
                    // ReservedField (also how Offset() is encoded): 0x00
                    // PkgLength, where the PkgLength *value* is a bit gap.
                    let (_pe, p1) = self.pkg_length(p + 1);
                    bit += self.pkg_value(p + 1);
                    p = p1;
                }
                0x01 => {
                    // AccessField: 0x01 AccessType AccessAttrib
                    p += 3;
                }
                0x02 => {
                    // ConnectField: 0x02 (NameString | BufferData) — rare; skip a namestring.
                    let (_n, p1) = self.name_ref(p + 1);
                    p = p1;
                }
                0x03 => {
                    // ExtendedAccessField: 0x03 AccessType AccessAttrib AccessLength
                    p += 4;
                }
                _ => {
                    // NamedField: NameSeg PkgLength(bitwidth)
                    let mut sg: Seg = [0; 4];
                    sg.copy_from_slice(&self.b[p..p + 4]);
                    let width = self.pkg_value(p + 4);
                    let (_pe, p1) = self.pkg_length(p + 4);
                    let mut fp = region.clone();
                    // Field units live in the region's parent scope (siblings of
                    // the region), addressed by their NameSeg.
                    fp.pop();
                    fp.push(sg);
                    self.ns.nodes.insert(
                        fp,
                        Node::Field { region: region.clone(), bit_offset: bit, bit_width: width },
                    );
                    bit += width;
                    p = p1;
                }
            }
        }
        Ok(())
    }

    // ── primitives ─────────────────────────────────────────────────────

    /// Decode a PkgLength, returning (absolute_end_position, position_after_pkglength_bytes).
    /// The returned end is `pkglength_start + value`.
    fn pkg_length(&self, p: usize) -> (usize, usize) {
        let lead = self.b[p];
        let extra = (lead >> 6) as usize;
        if extra == 0 {
            let len = (lead & 0x3F) as usize;
            (p + len, p + 1)
        } else {
            let mut len = (lead & 0x0F) as usize;
            for i in 0..extra {
                len |= (self.b[p + 1 + i] as usize) << (4 + i * 8);
            }
            (p + len, p + 1 + extra)
        }
    }

    /// The raw PkgLength *value* (used for field bit widths / offsets).
    fn pkg_value(&self, p: usize) -> u64 {
        let lead = self.b[p];
        let extra = (lead >> 6) as usize;
        if extra == 0 {
            (lead & 0x3F) as u64
        } else {
            let mut len = (lead & 0x0F) as u64;
            for i in 0..extra {
                len |= (self.b[p + 1 + i] as u64) << (4 + i * 8);
            }
            len
        }
    }

    fn name_ref(&self, mut p: usize) -> (NameRef, usize) {
        let mut rooted = false;
        let mut carets = 0;
        if self.b[p] == 0x5C {
            rooted = true;
            p += 1;
        } else {
            while self.b[p] == 0x5E {
                carets += 1;
                p += 1;
            }
        }
        let mut segs: Vec<Seg> = Vec::new();
        match self.b[p] {
            0x00 => {
                p += 1; // NullName
            }
            0x2E => {
                // DualNamePrefix
                p += 1;
                segs.push(self.seg_at(p));
                segs.push(self.seg_at(p + 4));
                p += 8;
            }
            0x2F => {
                // MultiNamePrefix SegCount
                p += 1;
                let count = self.b[p] as usize;
                p += 1;
                for i in 0..count {
                    segs.push(self.seg_at(p + i * 4));
                }
                p += count * 4;
            }
            _ => {
                segs.push(self.seg_at(p));
                p += 4;
            }
        }
        (NameRef { rooted, carets, segs }, p)
    }

    fn seg_at(&self, p: usize) -> Seg {
        let mut s: Seg = [0; 4];
        s.copy_from_slice(&self.b[p..p + 4]);
        s
    }

    /// Absolute path of a *definition* (no upward search).
    fn def_path(&self, scope: &Path, n: &NameRef) -> Path {
        let mut base: Path = if n.rooted {
            Vec::new()
        } else {
            let mut b = scope.clone();
            for _ in 0..n.carets {
                b.pop();
            }
            b
        };
        for s in &n.segs {
            base.push(*s);
        }
        base
    }

    /// A RegionOffset/RegionLen TermArg: a constant value if it is one,
    /// otherwise 0 after skipping the (computed) expression.
    /// Basis oder Laenge einer Operationsregion. Der dritte Rueckgabewert
    /// sagt, ob es eine ECHTE Zahl war — sonst muss die Anweisung
    /// nachgeholt werden.
    fn region_arg(&mut self, scope: &Path, p: usize) -> Result<(u64, usize, bool), String> {
        match self.b[p] {
            0x00 | 0x01 | 0xFF | 0x0A | 0x0B | 0x0C | 0x0E => {
                let (v, np) = self.data_object(p, self.b.len())?;
                let n = v.borrow().as_int();
                Ok((n, np, true))
            }
            _ => {
                let np = self.skip_term_arg(scope, p)?;
                Ok((0, np, false))
            }
        }
    }

    /// Method arity (arg count) for a name reference resolved from `scope`, or 0
    /// if it is not a (yet-loaded) method.
    fn arity(&self, scope: &Path, n: &NameRef) -> usize {
        self.ns
            .resolve(scope, n.rooted, n.carets, &n.segs)
            .and_then(|path| self.ns.nodes.get(&path))
            .map(|node| match node {
                Node::Method { flags, .. } => (*flags & 0x07) as usize,
                _ => 0,
            })
            .unwrap_or(0)
    }

    /// Advance past one TermArg expression without evaluating it. Enough for
    /// computed region offsets (arithmetic / method calls on names + constants).
    fn skip_term_arg(&self, scope: &Path, p: usize) -> Result<usize, String> {
        let op = self.b[p];
        match op {
            0x00 | 0x01 | 0xFF => Ok(p + 1),
            0x0A => Ok(p + 2),
            0x0B => Ok(p + 3),
            0x0C => Ok(p + 5),
            0x0E => Ok(p + 9),
            0x0D => {
                let mut q = p + 1;
                while q < self.b.len() && self.b[q] != 0 {
                    q += 1;
                }
                Ok(q + 1)
            }
            0x11 | 0x12 | 0x13 => {
                let (end, _p1) = self.pkg_length(p + 1);
                Ok(end)
            }
            0x60..=0x6E => Ok(p + 1), // Local0-7 / Arg0-6
            // binary ops: operand operand target
            0x72 | 0x74 | 0x77 | 0x79 | 0x7A | 0x7B | 0x7C | 0x7D | 0x7E | 0x7F => {
                let p1 = self.skip_term_arg(scope, p + 1)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                self.skip_super_name(scope, p2)
            }
            0x78 => {
                // Divide: a b rem-target quo-target
                let p1 = self.skip_term_arg(scope, p + 1)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                let p3 = self.skip_super_name(scope, p2)?;
                self.skip_super_name(scope, p3)
            }
            0x80 => {
                // Not: operand target
                let p1 = self.skip_term_arg(scope, p + 1)?;
                self.skip_super_name(scope, p1)
            }
            0x90 | 0x91 | 0x93 | 0x94 | 0x95 => {
                let p1 = self.skip_term_arg(scope, p + 1)?;
                self.skip_term_arg(scope, p1)
            }
            0x92 => {
                if matches!(self.b[p + 1], 0x93 | 0x94 | 0x95) {
                    let p1 = self.skip_term_arg(scope, p + 2)?;
                    self.skip_term_arg(scope, p1)
                } else {
                    self.skip_term_arg(scope, p + 1)
                }
            }
            0x83 => self.skip_term_arg(scope, p + 1), // DerefOf
            0x87 => self.skip_super_name(scope, p + 1), // SizeOf
            0x88 => {
                // Index: source index target
                let p1 = self.skip_term_arg(scope, p + 1)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                self.skip_super_name(scope, p2)
            }
            0x5B => match self.b[p + 1] {
                0x30 => Ok(p + 2), // Revision
                _ => Err(format!("skip: unhandled ext op 5B {:#04x} at {:#x}", self.b[p + 1], p)),
            },
            // NameString: may be a method invocation — consume its args.
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {
                let (n, np) = self.name_ref(p);
                let argc = self.arity(scope, &n);
                let mut q = np;
                for _ in 0..argc {
                    q = self.skip_term_arg(scope, q)?;
                }
                Ok(q)
            }
            other => Err(format!("skip: unhandled term-arg op {:#04x} at {:#x}", other, p)),
        }
    }

    fn skip_super_name(&self, scope: &Path, p: usize) -> Result<usize, String> {
        match self.b[p] {
            0x00 => Ok(p + 1), // null target
            0x60..=0x6E => Ok(p + 1),
            0x83 => self.skip_term_arg(scope, p + 1),
            0x88 => {
                let p1 = self.skip_term_arg(scope, p + 1)?;
                let p2 = self.skip_term_arg(scope, p1)?;
                self.skip_super_name(scope, p2)
            }
            0x5B if self.b[p + 1] == 0x31 => Ok(p + 2), // Debug
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {
                let (_n, np) = self.name_ref(p);
                Ok(np)
            }
            other => Err(format!("skip super-name op {:#04x} at {:#x}", other, p)),
        }
    }

    /// Const-evaluate a DataRefObject (Name initializer, region offset/len,
    /// package element). Returns a fresh Obj cell.
    fn data_object(&mut self, p: usize, end: usize) -> Result<(Obj, usize), String> {
        let op = self.b[p];
        match op {
            0x00 => Ok((obj(Value::Int(0)), p + 1)),       // Zero
            0x01 => Ok((obj(Value::Int(1)), p + 1)),       // One
            0xFF => Ok((obj(Value::Int(u64::MAX)), p + 1)), // Ones
            0x0A => Ok((obj(Value::Int(self.b[p + 1] as u64)), p + 2)),
            0x0B => {
                let v = u16::from_le_bytes([self.b[p + 1], self.b[p + 2]]) as u64;
                Ok((obj(Value::Int(v)), p + 3))
            }
            0x0C => {
                let v = u32::from_le_bytes([
                    self.b[p + 1], self.b[p + 2], self.b[p + 3], self.b[p + 4],
                ]) as u64;
                Ok((obj(Value::Int(v)), p + 5))
            }
            0x0E => {
                let mut a = [0u8; 8];
                a.copy_from_slice(&self.b[p + 1..p + 9]);
                Ok((obj(Value::Int(u64::from_le_bytes(a))), p + 9))
            }
            0x0D => {
                // String: bytes until NUL
                let mut q = p + 1;
                let mut s = String::new();
                while q < end && self.b[q] != 0 {
                    s.push(self.b[q] as char);
                    q += 1;
                }
                Ok((obj(Value::Str(s)), q + 1))
            }
            0x11 => {
                // Buffer PkgLength BufferSize ByteList
                let (pkg_end, p1) = self.pkg_length(p + 1);
                let (size, p2) = self.data_object(p1, pkg_end)?;
                let n = size.borrow().as_int() as usize;
                let mut buf = Vec::with_capacity(n);
                buf.extend_from_slice(&self.b[p2..pkg_end]);
                buf.resize(n, 0);
                Ok((obj(Value::Buffer(buf)), pkg_end))
            }
            0x12 => {
                // Package PkgLength NumElements PackageElementList
                let (pkg_end, p1) = self.pkg_length(p + 1);
                let num = self.b[p1] as usize;
                let mut q = p1 + 1;
                let mut elems: Vec<Obj> = Vec::with_capacity(num);
                while q < pkg_end && elems.len() < num {
                    let (e, nq) = self.package_element(q, pkg_end)?;
                    elems.push(e);
                    q = nq;
                }
                while elems.len() < num {
                    elems.push(obj(Value::Uninit));
                }
                Ok((obj(Value::Package(elems)), pkg_end))
            }
            0x13 => {
                // VarPackage PkgLength NumElements(TermArg) PackageElementList
                let (pkg_end, p1) = self.pkg_length(p + 1);
                let (num_v, p2) = self.data_object(p1, pkg_end)?;
                let num = num_v.borrow().as_int() as usize;
                let mut q = p2;
                let mut elems: Vec<Obj> = Vec::new();
                while q < pkg_end && elems.len() < num {
                    let (e, nq) = self.package_element(q, pkg_end)?;
                    elems.push(e);
                    q = nq;
                }
                while elems.len() < num {
                    elems.push(obj(Value::Uninit));
                }
                Ok((obj(Value::Package(elems)), pkg_end))
            }
            // RevisionOp
            0x5B if self.b[p + 1] == 0x30 => Ok((obj(Value::Int(2)), p + 2)),
            // ObjectReference initializer (a NameString) — skip it; we don't
            // need referenced-name initializers for the battery path.
            0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F => {
                let (_n, np) = self.name_ref(p);
                Ok((obj(Value::Uninit), np))
            }
            _ => Err(format!("non-const Name/data initializer op {:#04x} at {:#x}", op, p)),
        }
    }

    /// A package element is a DataRefObject or a NameString reference. We only
    /// need the data ones for the battery; name references become Uninit.
    fn package_element(&mut self, p: usize, end: usize) -> Result<(Obj, usize), String> {
        let op = self.b[p];
        let is_name = matches!(op, 0x5C | 0x5E | 0x2E | 0x2F | 0x41..=0x5A | 0x5F);
        if is_name {
            let (_n, np) = self.name_ref(p);
            Ok((obj(Value::Uninit), np))
        } else {
            self.data_object(p, end)
        }
    }
}
