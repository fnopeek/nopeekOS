//! `JSON.parse` und `JSON.stringify`.
//!
//! Kein Zusatz, sondern Grundausstattung: der Selbsttest fiel darueber, und
//! auf echten Seiten steht die Konfiguration einer Komponente fast immer als
//! JSON in einem `<script type="application/json">` oder in einem
//! `data-`-Attribut. Ohne `JSON` bricht der Startpfad solcher Seiten in der
//! ersten Zeile ab.
//!
//! Eigener Erzeuger und eigener Leser statt „irgendwie ueber die Sprache":
//! JSON ist NICHT die JS-Literalsyntax (keine einfachen Anfuehrungszeichen,
//! kein Komma am Ende, keine Bezeichner als Schluessel), und der Leser haette
//! sonst mehr angenommen als er darf.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::interp::*;
use super::value::*;

/// Wie tief `stringify` und `parse` gehen duerfen. Beide laufen rekursiv auf
/// dem WIRTS-Stapel, und ein zu tiefes Dokument ist im Kernel kein Fehler,
/// sondern ein Absturz — dieselbe Ueberlegung wie bei `MAX_DEPTH`.
const MAX_JSON_DEPTH: usize = 200;

pub fn install(realm: &mut Realm) {
    let fp = realm.function_proto.clone();
    let json = new_obj(Some(realm.object_proto.clone()));
    let st = native(Some(fp.clone()), stringify, "stringify", 3, false);
    let pa = native(Some(fp.clone()), parse, "parse", 2, false);
    json.borrow_mut().define("stringify", Prop::builtin(Value::Obj(st)));
    json.borrow_mut().define("parse", Prop::builtin(Value::Obj(pa)));
    json.borrow_mut().define(SYM_TO_STRING_TAG, Prop::frozen(Value::str("JSON")));
    realm.global.borrow_mut().define("JSON", Prop::builtin(Value::Obj(json)));
}

// ── stringify ────────────────────────────────────────────────────────────

fn stringify(i: &mut Interp, _t: Value, args: &[Value]) -> C<Value> {
    let v = args.first().cloned().unwrap_or(Value::Undefined);
    // Der dritte Parameter ist der Einzug. Eine Zahl heisst „so viele
    // Leerzeichen", ein Text heisst „dieser Text" — beide gedeckelt auf 10,
    // wie die Spezifikation es vorschreibt.
    // Ein Number- oder String-OBJEKT zaehlt wie sein Wert (ES §25.5.2.1, 5).
    // Seit `new Number(5)` wirklich ein Objekt ist, kommt der Fall auch vor.
    let space = match args.get(2) {
        Some(Value::Obj(o)) => match &o.borrow().kind {
            ObjKind::NumWrap(n) => Some(Value::Num(*n)),
            ObjKind::StrWrap(t) => Some(Value::Str(t.clone())),
            _ => None,
        },
        other => other.cloned(),
    };
    let indent = match &space {
        Some(Value::Num(n)) => " ".repeat((*n as usize).min(10)),
        Some(Value::Str(s)) => s.chars().take(10).collect(),
        _ => String::new(),
    };
    let mut seen: Vec<Gc> = Vec::new();
    let mut out = String::new();
    match write_value(i, &v, &indent, "", &mut seen, &mut out, 0)? {
        true => Ok(Value::Str(Rc::from(out.as_str()))),
        // `undefined`, eine Funktion — die haben keine Entsprechung, und die
        // Antwort ist `undefined`, nicht der Text "undefined".
        false => Ok(Value::Undefined),
    }
}

/// Schreibt `v` nach `out`. `false` heisst „hat keine Entsprechung" — der
/// Aufrufer entscheidet dann, ob das ein `null` (im Array) oder ein
/// weggelassenes Feld (im Objekt) wird.
fn write_value(i: &mut Interp, v: &Value, indent: &str, cur: &str, seen: &mut Vec<Gc>,
                out: &mut String, depth: usize) -> C<bool> {
    i.tick()?;
    if depth > MAX_JSON_DEPTH {
        return Err(i.throw_kind("TypeError", "JSON.stringify: structure too deep"));
    }
    // `toJSON` gewinnt ueber alles andere — daran haengt, dass ein Datum als
    // Zeichenkette herauskommt statt als leeres Objekt.
    let v = match v {
        Value::Obj(o) => {
            let f = i.get(&Value::Obj(o.clone()), "toJSON")?;
            if i.is_callable(&f) { i.call(&f, v.clone(), &[])? } else { v.clone() }
        }
        _ => v.clone(),
    };
    // Die HUELLE eines Primitivs zaehlt wie das Primitiv: `JSON.stringify(
    // Object(0n))` muss werfen, `Object(1)` wird zu `1`. Ohne diesen Schritt
    // faellt eine Huelle in den Objektzweig und wird `{}`.
    let v = match &v {
        Value::Obj(o) => match &o.borrow().kind {
            ObjKind::BigWrap(b) => Value::BigInt(b.clone()),
            _ => v.clone(),
        },
        _ => v.clone(),
    };
    match &v {
        Value::Null => { out.push_str("null"); Ok(true) }
        Value::Bool(b) => { out.push_str(if *b { "true" } else { "false" }); Ok(true) }
        Value::Num(n) => {
            // NaN und Unendlich sind kein JSON. `null` ist die vorgeschriebene
            // Ersatzform, nicht ein Fehler.
            if n.is_finite() { out.push_str(&num_to_string(*n)); } else { out.push_str("null"); }
            Ok(true)
        }
        Value::Str(s) => { write_string(s, out); Ok(true) }
        // JSON kennt keine grossen Zahlen, und stillschweigend zu kuerzen
        // waere Datenverlust — die Spezifikation schreibt hier einen Fehler vor.
        Value::BigInt(_) => Err(i.throw_kind("TypeError", "Do not know how to serialize a BigInt")),
        // Wie `undefined`: faellt aus dem Objekt heraus, ist im Array `null`.
        // Ein Fehler waere falsch — JSON kennt Symbole schlicht nicht.
        Value::Undefined | Value::Sym(_) => Ok(false),
        Value::Obj(o) => {
            if i.is_callable(&v) { return Ok(false); }
            // Ein Zyklus ist der eine Fall, in dem `stringify` werfen MUSS.
            // Ohne die Pruefung laeuft er, bis der Stapel reisst.
            if seen.iter().any(|s| Rc::ptr_eq(s, o)) {
                return Err(i.throw_kind("TypeError", "Converting circular structure to JSON"));
            }
            seen.push(o.clone());
            let r = write_obj(i, o, indent, cur, seen, out, depth);
            seen.pop();
            r
        }
    }
}

fn write_obj(i: &mut Interp, o: &Gc, indent: &str, cur: &str, seen: &mut Vec<Gc>,
             out: &mut String, depth: usize) -> C<bool> {
    let is_array = matches!(o.borrow().kind, ObjKind::Array);
    let inner = { let mut s = String::from(cur); s.push_str(indent); s };
    let (open, close) = if is_array { ('[', ']') } else { ('{', '}') };
    let nl = if indent.is_empty() { String::new() } else { format!("\n{inner}") };
    let nl_end = if indent.is_empty() { String::new() } else { format!("\n{cur}") };

    out.push(open);
    let mut n = 0usize;
    if is_array {
        let len = match i.get(&Value::Obj(o.clone()), "length")? {
            Value::Num(x) if x.is_finite() && x >= 0.0 => x as usize,
            _ => 0,
        };
        for idx in 0..len {
            i.tick()?;
            if n > 0 { out.push(','); }
            out.push_str(&nl);
            let e = i.get(&Value::Obj(o.clone()), &idx.to_string())?;
            // Im Array wird eine Luecke zu `null` — weglassen wuerde die
            // Laenge aendern, und die traegt hier Bedeutung.
            if !write_value(i, &e, indent, &inner, seen, out, depth + 1)? {
                out.push_str("null");
            }
            n += 1;
        }
    } else {
        let keys = o.borrow().own_keys();
        for k in keys {
            i.tick()?;
            let enumerable = o.borrow().is_enumerable(&k);
            if !enumerable { continue; }
            let e = i.get(&Value::Obj(o.clone()), &k)?;
            let mut piece = String::new();
            if !write_value(i, &e, indent, &inner, seen, &mut piece, depth + 1)? { continue; }
            if n > 0 { out.push(','); }
            out.push_str(&nl);
            write_string(&k, out);
            out.push(':');
            if !indent.is_empty() { out.push(' '); }
            out.push_str(&piece);
            n += 1;
        }
    }
    if n > 0 { out.push_str(&nl_end); }
    out.push(close);
    Ok(true)
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ── parse ────────────────────────────────────────────────────────────────

struct P<'a> { b: &'a [u8], p: usize }

/// `JSON.parse` fuer einen Wert, den der Rufer schon HAT.
///
/// `Response.json()` geht hierueber. Ein eigener Leser dort waere eine
/// zweite Semantik — und die faellt zuerst bei etwas Kleinem auseinander,
/// etwa was ein nacktes `NaN` im Text bedeutet.
pub(crate) fn parse_value(i: &mut Interp, v: &Value) -> C<Value> {
    parse(i, Value::Undefined, core::slice::from_ref(v))
}

fn parse(i: &mut Interp, _t: Value, args: &[Value]) -> C<Value> {
    let text = match args.first() {
        Some(v) => i.to_string(v)?,
        None => Rc::from("undefined"),
    };
    let mut p = P { b: text.as_bytes(), p: 0 };
    p.ws();
    let v = read(i, &mut p, 0)?;
    p.ws();
    if p.p != p.b.len() {
        return Err(i.throw_kind("SyntaxError",
            &format!("Unexpected token in JSON at position {}", p.p)));
    }
    // Der Wiederhersteller laeuft ueber das FERTIGE Ergebnis, von innen nach
    // aussen. Er darf Werte ersetzen und weglassen; das ist der Grund, warum
    // er nicht schon beim Lesen greifen kann.
    match args.get(1) {
        Some(f) if i.is_callable(f) => {
            let holder = new_obj(Some(i.realm.object_proto.clone()));
            holder.borrow_mut().define("", Prop::data(v));
            revive(i, &Value::Obj(holder), "", f, 0)
        }
        _ => Ok(v),
    }
}

fn revive(i: &mut Interp, holder: &Value, key: &str, f: &Value, depth: usize) -> C<Value> {
    i.tick()?;
    if depth > MAX_JSON_DEPTH { return Ok(Value::Undefined); }
    let val = i.get(holder, key)?;
    if let Value::Obj(o) = &val {
        let keys = o.borrow().own_keys();
        for k in keys {
            let nv = revive(i, &val, &k, f, depth + 1)?;
            if matches!(nv, Value::Undefined) {
                o.borrow_mut().remove(&k);
            } else {
                i.set(&val, &k, nv, true)?;
            }
        }
    }
    i.call(f, holder.clone(), &[Value::str(key), val])
}

impl P<'_> {
    fn ws(&mut self) {
        while self.p < self.b.len() && matches!(self.b[self.p], b' ' | b'\t' | b'\n' | b'\r') {
            self.p += 1;
        }
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.b[self.p..].starts_with(s.as_bytes()) { self.p += s.len(); true } else { false }
    }
}

fn read(i: &mut Interp, p: &mut P, depth: usize) -> C<Value> {
    i.tick()?;
    if depth > MAX_JSON_DEPTH {
        return Err(i.throw_kind("SyntaxError", "JSON.parse: structure too deep"));
    }
    p.ws();
    if p.p >= p.b.len() {
        return Err(i.throw_kind("SyntaxError", "Unexpected end of JSON input"));
    }
    let c = p.b[p.p];
    if c == b'n' || c == b't' || c == b'f' {
        if p.eat("null") { return Ok(Value::Null); }
        if p.eat("true") { return Ok(Value::Bool(true)); }
        if p.eat("false") { return Ok(Value::Bool(false)); }
        return Err(i.throw_kind("SyntaxError",
            &format!("Unexpected token in JSON at position {}", p.p)));
    }
    match c {
        b'"' => { let s = read_str(i, p)?; Ok(Value::Str(Rc::from(s.as_str()))) }
        b'[' => {
            p.p += 1;
            let mut items = Vec::new();
            p.ws();
            if p.p < p.b.len() && p.b[p.p] == b']' { p.p += 1; return Ok(i.new_array(items)); }
            loop {
                items.push(read(i, p, depth + 1)?);
                p.ws();
                match p.b.get(p.p) {
                    Some(b',') => { p.p += 1; }
                    Some(b']') => { p.p += 1; break }
                    _ => return Err(i.throw_kind("SyntaxError",
                        &format!("Expected ',' or ']' at position {}", p.p))),
                }
            }
            Ok(i.new_array(items))
        }
        b'{' => {
            p.p += 1;
            let o = new_obj(Some(i.realm.object_proto.clone()));
            p.ws();
            if p.p < p.b.len() && p.b[p.p] == b'}' { p.p += 1; return Ok(Value::Obj(o)); }
            loop {
                p.ws();
                if p.b.get(p.p) != Some(&b'"') {
                    return Err(i.throw_kind("SyntaxError",
                        &format!("Expected string key at position {}", p.p)));
                }
                let k = read_str(i, p)?;
                p.ws();
                if p.b.get(p.p) != Some(&b':') {
                    return Err(i.throw_kind("SyntaxError",
                        &format!("Expected ':' at position {}", p.p)));
                }
                p.p += 1;
                let v = read(i, p, depth + 1)?;
                o.borrow_mut().define(&k, Prop::data(v));
                p.ws();
                match p.b.get(p.p) {
                    Some(b',') => { p.p += 1; }
                    Some(b'}') => { p.p += 1; break }
                    _ => return Err(i.throw_kind("SyntaxError",
                        &format!("Expected ',' or '}}' at position {}", p.p))),
                }
            }
            Ok(Value::Obj(o))
        }
        _ => read_num(i, p),
    }
}

fn read_str(i: &mut Interp, p: &mut P) -> C<String> {
    p.p += 1; // "
    let mut s = String::new();
    loop {
        let Some(&c) = p.b.get(p.p) else {
            return Err(i.throw_kind("SyntaxError", "Unterminated string in JSON"));
        };
        p.p += 1;
        match c {
            b'"' => return Ok(s),
            // Ein rohes Steuerzeichen ist in JSON verboten — anders als in
            // einem JS-Literal. Wer das durchlaesst, nimmt mehr an als er darf.
            0..=0x1f => return Err(i.throw_kind("SyntaxError",
                "Bad control character in JSON string")),
            b'\\' => {
                let Some(&e) = p.b.get(p.p) else {
                    return Err(i.throw_kind("SyntaxError", "Unterminated escape in JSON"));
                };
                p.p += 1;
                match e {
                    b'"' => s.push('"'), b'\\' => s.push('\\'), b'/' => s.push('/'),
                    b'b' => s.push('\u{8}'), b'f' => s.push('\u{c}'),
                    b'n' => s.push('\n'), b'r' => s.push('\r'), b't' => s.push('\t'),
                    b'u' => {
                        let cp = hex4(i, p)?;
                        // Ein hohes Ersatzzeichen sucht sein Gegenstueck; ein
                        // einzelnes bleibt als Ersatzzeichen stehen, statt den
                        // Lauf abzubrechen — genau so macht es ein Browser.
                        let ch = if (0xd800..0xdc00).contains(&cp)
                            && p.b.get(p.p) == Some(&b'\\') && p.b.get(p.p + 1) == Some(&b'u') {
                            p.p += 2;
                            let lo = hex4(i, p)?;
                            if (0xdc00..0xe000).contains(&lo) {
                                0x10000 + ((cp - 0xd800) << 10) + (lo - 0xdc00)
                            } else { 0xfffd }
                        } else { cp };
                        s.push(char::from_u32(ch).unwrap_or('\u{fffd}'));
                    }
                    _ => return Err(i.throw_kind("SyntaxError", "Bad escape in JSON string")),
                }
            }
            // Mehrbytefolgen wandern unveraendert durch; die Quelle war ein
            // gueltiger Rust-`str`, also ist jede davon gueltiges UTF-8.
            c if c < 0x80 => s.push(c as char),
            _ => {
                let start = p.p - 1;
                let mut end = p.p;
                while end < p.b.len() && (p.b[end] & 0xc0) == 0x80 { end += 1; }
                s.push_str(core::str::from_utf8(&p.b[start..end]).unwrap_or("\u{fffd}"));
                p.p = end;
            }
        }
    }
}

fn hex4(i: &mut Interp, p: &mut P) -> C<u32> {
    if p.p + 4 > p.b.len() {
        return Err(i.throw_kind("SyntaxError", "Bad Unicode escape in JSON"));
    }
    let mut n = 0u32;
    for k in 0..4 {
        let d = (p.b[p.p + k] as char).to_digit(16)
            .ok_or_else(|| i.throw_kind("SyntaxError", "Bad Unicode escape in JSON"))?;
        n = n * 16 + d;
    }
    p.p += 4;
    Ok(n)
}

fn read_num(i: &mut Interp, p: &mut P) -> C<Value> {
    let start = p.p;
    if p.b.get(p.p) == Some(&b'-') { p.p += 1; }
    while matches!(p.b.get(p.p), Some(b'0'..=b'9')) { p.p += 1; }
    if p.b.get(p.p) == Some(&b'.') {
        p.p += 1;
        while matches!(p.b.get(p.p), Some(b'0'..=b'9')) { p.p += 1; }
    }
    if matches!(p.b.get(p.p), Some(b'e') | Some(b'E')) {
        p.p += 1;
        if matches!(p.b.get(p.p), Some(b'+') | Some(b'-')) { p.p += 1; }
        while matches!(p.b.get(p.p), Some(b'0'..=b'9')) { p.p += 1; }
    }
    let s = core::str::from_utf8(&p.b[start..p.p]).unwrap_or("");
    match s.parse::<f64>() {
        Ok(n) => Ok(Value::Num(n)),
        Err(_) => Err(i.throw_kind("SyntaxError",
            &format!("Unexpected token in JSON at position {start}"))),
    }
}
