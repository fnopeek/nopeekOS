//! Selectors for `querySelector`, `matches` and `closest` (Selectors 4),
//! matched on the script's arena.
//!
//! The cascade has its own matcher on `crate::dom`; this one answers on the
//! live arena without rebuilding a tree per call. An invalid selector is an
//! error (the caller throws `SyntaxError`), never a silent non-match.
//!
//! Not implemented: namespaces (`ns|tag` is read as `tag`), `:dir()`,
//! `:indeterminate`, and the user-action states `:hover`/`:active`, which
//! never match.

use alloc::string::String;
use alloc::vec::Vec;

use super::dombind::{Doc, ELEMENT_NODE, TEXT_NODE, UI_MODAL, UI_POPOVER_OPEN};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Comb { Desc, Child, Next, Sub }

#[derive(Clone, Copy, PartialEq, Debug)]
enum AttrOp { Exists, Eq, Word, Dash, Prefix, Suffix, Substr }

#[derive(Clone, Debug)]
enum Simple {
    Id(String),
    Class(String),
    Attr { name: String, op: AttrOp, val: String, ci: bool },
    Not(SelList),
    Is(SelList),
    Has(Vec<(Comb, Complex)>),
    Nth { a: i32, b: i32, last: bool, of_type: bool, of: Option<SelList> },
    Empty,
    Root,
    Scope,
    Checked,
    Disabled,
    Enabled,
    Defined,
    Link,
    Focus,
    FocusWithin,
    Target,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    Open,
    PopoverOpen,
    Modal,
    State(String),
    Lang(String),
    /// A pseudo-element or a state beak never has: valid, never matches.
    Never,
}

#[derive(Clone, Debug)]
struct Compound {
    /// `None` for `*` or no type selector.
    tag: Option<String>,
    items: Vec<Simple>,
}

/// Compounds right to left: `parts[0]` is the subject, `parts[k].0` the
/// combinator between `parts[k]` and `parts[k + 1]`, the compound to its left.
#[derive(Clone, Debug)]
pub struct Complex {
    parts: Vec<(Comb, Compound)>,
}

#[derive(Clone, Debug)]
pub struct SelList(Vec<Complex>);

/// What a match may ask beyond the tree.
pub struct Ctx<'a> {
    pub d: &'a Doc,
    /// `:scope`: the element `querySelector`/`matches` was called on; the root
    /// element for the document.
    pub scope: Option<u32>,
    /// The fragment of the document URL, for `:target`.
    pub target: &'a str,
}

// ── Parsing ─────────────────────────────────────────────────────────────────

struct P<'s> {
    s: &'s [char],
    i: usize,
}

type R<T> = Result<T, String>;

pub fn parse(text: &str) -> R<SelList> {
    let chars: Vec<char> = text.chars().collect();
    let mut p = P { s: &chars, i: 0 };
    let list = p.list(false)?;
    p.ws();
    if p.i < p.s.len() { return Err(alloc::format!("unexpected '{}'", p.s[p.i])) }
    Ok(list)
}

fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

impl<'s> P<'s> {
    fn peek(&self) -> Option<char> { self.s.get(self.i).copied() }
    fn ws(&mut self) -> bool {
        let at = self.i;
        while self.peek().is_some_and(|c| c.is_whitespace()) { self.i += 1; }
        self.i > at
    }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) { self.i += 1; true } else { false }
    }

    /// A comma-separated list. `forgiving` drops invalid entries (`:is`,
    /// `:where`), otherwise one invalid entry fails the whole list.
    fn list(&mut self, forgiving: bool) -> R<SelList> {
        let mut out = Vec::new();
        loop {
            self.ws();
            let at = self.i;
            match self.complex() {
                Ok(c) => out.push(c),
                Err(e) => {
                    if !forgiving { return Err(e) }
                    self.i = at;
                    self.skip_to_comma();
                }
            }
            self.ws();
            if !self.eat(',') { break }
        }
        if out.is_empty() && !forgiving { return Err(String::from("empty selector")) }
        Ok(SelList(out))
    }

    fn skip_to_comma(&mut self) {
        let mut depth = 0i32;
        while let Some(c) = self.peek() {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' if depth == 0 => return,
                ')' | ']' => depth -= 1,
                ',' if depth == 0 => return,
                '"' | '\'' => { let _ = self.string(); continue }
                _ => {}
            }
            self.i += 1;
        }
    }

    fn comb(&mut self) -> Option<Comb> {
        let had_ws = self.ws();
        let c = match self.peek() {
            Some('>') => Comb::Child,
            Some('+') => Comb::Next,
            Some('~') => Comb::Sub,
            Some(',') | Some(')') | None => return None,
            _ => return if had_ws { Some(Comb::Desc) } else { None },
        };
        self.i += 1;
        self.ws();
        Some(c)
    }

    fn complex(&mut self) -> R<Complex> {
        let mut left: Vec<(Comb, Compound)> = Vec::new();
        let mut comb = Comb::Desc;
        loop {
            let c = self.compound()?;
            left.push((comb, c));
            let at = self.i;
            match self.comb() {
                Some(k) => {
                    if matches!(self.peek(), Some(',') | Some(')') | None) {
                        if k == Comb::Desc { self.i = at; break }
                        return Err(String::from("a combinator without a selector after it"));
                    }
                    comb = k;
                }
                None => break,
            }
        }
        // Stored right to left; each entry carries the combinator to its left.
        left.reverse();
        Ok(Complex { parts: left })
    }

    /// A relative selector for `:has()`: an optional leading combinator.
    fn relative(&mut self) -> R<(Comb, Complex)> {
        self.ws();
        let lead = match self.peek() {
            Some('>') => { self.i += 1; Comb::Child }
            Some('+') => { self.i += 1; Comb::Next }
            Some('~') => { self.i += 1; Comb::Sub }
            _ => Comb::Desc,
        };
        self.ws();
        Ok((lead, self.complex()?))
    }

    fn compound(&mut self) -> R<Compound> {
        let mut c = Compound { tag: None, items: Vec::new() };
        let mut any = false;
        if self.eat('*') {
            any = true;
            if self.eat('|') { if !self.eat('*') { self.ident()?; } }
        } else if self.peek().is_some_and(|ch| is_name(ch) || ch == '\\') {
            let mut name = self.ident()?;
            if self.peek() == Some('|') && self.s.get(self.i + 1) != Some(&'=') {
                self.i += 1;
                name = if self.eat('*') { String::new() } else { self.ident()? };
            }
            if !name.is_empty() { c.tag = Some(name.to_ascii_lowercase()); }
            any = true;
        }
        loop {
            match self.peek() {
                Some('#') => {
                    self.i += 1;
                    c.items.push(Simple::Id(self.ident()?));
                }
                Some('.') => {
                    self.i += 1;
                    c.items.push(Simple::Class(self.ident()?));
                }
                Some('[') => { self.i += 1; c.items.push(self.attr()?); }
                Some(':') => { self.i += 1; c.items.push(self.pseudo()?); }
                _ => break,
            }
            any = true;
        }
        if !any {
            return Err(match self.peek() {
                Some(ch) => alloc::format!("unexpected '{ch}'"),
                None => String::from("unexpected end"),
            });
        }
        Ok(c)
    }

    fn ident(&mut self) -> R<String> {
        let mut out = String::new();
        if self.peek() == Some('-') { out.push('-'); self.i += 1; }
        while let Some(c) = self.peek() {
            if c == '\\' {
                self.i += 1;
                out.push(self.escape()?);
            } else if is_name(c) {
                out.push(c);
                self.i += 1;
            } else {
                break;
            }
        }
        let first = out.chars().next();
        if out.is_empty() || out == "-" || first.is_some_and(|c| c.is_ascii_digit())
            || (out.starts_with('-') && out.chars().nth(1).is_some_and(|c| c.is_ascii_digit())) {
            return Err(String::from("expected an identifier"));
        }
        Ok(out)
    }

    /// After a backslash (css-syntax-3 §4.3.7).
    fn escape(&mut self) -> R<char> {
        let Some(c) = self.peek() else { return Ok('\u{FFFD}') };
        if c.is_ascii_hexdigit() {
            let mut v = 0u32;
            let mut n = 0;
            while n < 6 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                v = v * 16 + self.peek().unwrap().to_digit(16).unwrap();
                self.i += 1;
                n += 1;
            }
            if self.peek().is_some_and(|c| c.is_whitespace()) { self.i += 1; }
            return Ok(match char::from_u32(v) { Some(c) if v != 0 => c, _ => '\u{FFFD}' });
        }
        if c == '\n' { return Err(String::from("invalid escape")) }
        self.i += 1;
        Ok(c)
    }

    fn string(&mut self) -> R<String> {
        let q = self.peek().unwrap_or('"');
        self.i += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Ok(out),
                Some(c) if c == q => { self.i += 1; return Ok(out) }
                Some('\\') => {
                    self.i += 1;
                    if self.peek() == Some('\n') { self.i += 1; continue }
                    out.push(self.escape()?);
                }
                Some('\n') => return Err(String::from("newline in string")),
                Some(c) => { out.push(c); self.i += 1; }
            }
        }
    }

    fn attr(&mut self) -> R<Simple> {
        self.ws();
        let mut name = if self.eat('*') { String::new() } else { self.ident()? };
        if self.peek() == Some('|') && self.s.get(self.i + 1) != Some(&'=') {
            self.i += 1;
            name = self.ident()?;
        }
        if name.is_empty() { return Err(String::from("expected an attribute name")) }
        self.ws();
        let op = match self.peek() {
            Some(']') => { self.i += 1; return Ok(Simple::Attr {
                name: name.to_ascii_lowercase(), op: AttrOp::Exists, val: String::new(), ci: false }) }
            Some('=') => { self.i += 1; AttrOp::Eq }
            Some(c @ ('~' | '|' | '^' | '$' | '*')) => {
                self.i += 1;
                if !self.eat('=') { return Err(String::from("expected '='")) }
                match c { '~' => AttrOp::Word, '|' => AttrOp::Dash, '^' => AttrOp::Prefix,
                          '$' => AttrOp::Suffix, _ => AttrOp::Substr }
            }
            _ => return Err(String::from("invalid attribute selector")),
        };
        self.ws();
        let val = match self.peek() {
            Some('"') | Some('\'') => self.string()?,
            _ => self.ident()?,
        };
        self.ws();
        let mut ci = false;
        if let Some(c) = self.peek() {
            if c == 'i' || c == 'I' { ci = true; self.i += 1; self.ws(); }
            else if c == 's' || c == 'S' { self.i += 1; self.ws(); }
        }
        if !self.eat(']') { return Err(String::from("expected ']'")) }
        Ok(Simple::Attr { name: name.to_ascii_lowercase(), op, val, ci })
    }

    fn args_end(&mut self) -> R<()> {
        self.ws();
        if self.eat(')') { Ok(()) } else { Err(String::from("expected ')'")) }
    }

    fn pseudo(&mut self) -> R<Simple> {
        if self.eat(':') {
            // A pseudo-element: valid, and never an element of the tree.
            let _ = self.ident()?;
            if self.eat('(') { self.skip_args()?; }
            return Ok(Simple::Never);
        }
        let name = self.ident()?.to_ascii_lowercase();
        if self.eat('(') {
            let r = match name.as_str() {
                "not" => Simple::Not(self.list(false)?),
                "is" | "matches" | "where" | "-webkit-any" => Simple::Is(self.list(true)?),
                "has" => {
                    let mut rel = Vec::new();
                    loop {
                        rel.push(self.relative()?);
                        self.ws();
                        if !self.eat(',') { break }
                    }
                    Simple::Has(rel)
                }
                "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                    let (a, b) = self.anb()?;
                    let mut of = None;
                    self.ws();
                    if name.ends_with("child") && self.s[self.i..].starts_with(&['o', 'f']) {
                        self.i += 2;
                        of = Some(self.list(false)?);
                    }
                    Simple::Nth { a, b, last: name.contains("last"), of_type: name.ends_with("type"), of }
                }
                "lang" => {
                    self.ws();
                    let v = match self.peek() {
                        Some('"') | Some('\'') => self.string()?,
                        _ => self.ident()?,
                    };
                    Simple::Lang(v.to_ascii_lowercase())
                }
                "state" => { self.ws(); Simple::State(self.ident()?) }
                "dir" | "host" | "host-context" => { self.skip_args()?; return Ok(Simple::Never) }
                _ => return Err(alloc::format!("unknown pseudo-class ':{name}()'")),
            };
            self.args_end()?;
            return Ok(r);
        }
        Ok(match name.as_str() {
            "first-child" => Simple::Nth { a: 0, b: 1, last: false, of_type: false, of: None },
            "last-child" => Simple::Nth { a: 0, b: 1, last: true, of_type: false, of: None },
            "only-child" => Simple::Is(SelList(alloc::vec![
                Complex { parts: alloc::vec![(Comb::Desc, Compound { tag: None, items: alloc::vec![
                    Simple::Nth { a: 0, b: 1, last: false, of_type: false, of: None },
                    Simple::Nth { a: 0, b: 1, last: true, of_type: false, of: None }] })] }])),
            "first-of-type" => Simple::Nth { a: 0, b: 1, last: false, of_type: true, of: None },
            "last-of-type" => Simple::Nth { a: 0, b: 1, last: true, of_type: true, of: None },
            "only-of-type" => Simple::Is(SelList(alloc::vec![
                Complex { parts: alloc::vec![(Comb::Desc, Compound { tag: None, items: alloc::vec![
                    Simple::Nth { a: 0, b: 1, last: false, of_type: true, of: None },
                    Simple::Nth { a: 0, b: 1, last: true, of_type: true, of: None }] })] }])),
            "empty" => Simple::Empty,
            "root" => Simple::Root,
            "scope" => Simple::Scope,
            "checked" => Simple::Checked,
            "disabled" => Simple::Disabled,
            "enabled" => Simple::Enabled,
            "defined" => Simple::Defined,
            "link" | "any-link" | "-webkit-any-link" => Simple::Link,
            "focus" => Simple::Focus,
            "focus-within" => Simple::FocusWithin,
            "target" => Simple::Target,
            "required" => Simple::Required,
            "optional" => Simple::Optional,
            "read-only" => Simple::ReadOnly,
            "read-write" => Simple::ReadWrite,
            "placeholder-shown" => Simple::PlaceholderShown,
            "open" => Simple::Open,
            "popover-open" => Simple::PopoverOpen,
            "modal" => Simple::Modal,
            "before" | "after" | "first-line" | "first-letter" => Simple::Never,
            "hover" | "active" | "visited" | "focus-visible" | "target-within" | "indeterminate"
            | "default" | "valid" | "invalid" | "in-range" | "out-of-range" | "user-invalid"
            | "user-valid" | "autofill" | "-webkit-autofill" | "playing" | "paused" | "fullscreen"
            | "picture-in-picture" | "host" | "blank" | "current" | "past" | "future"
            | "local-link" => Simple::Never,
            _ => return Err(alloc::format!("unknown pseudo-class ':{name}'")),
        })
    }

    fn skip_args(&mut self) -> R<()> {
        let mut depth = 1;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                '(' => depth += 1,
                ')' => { depth -= 1; if depth == 0 { return Ok(()) } }
                _ => {}
            }
        }
        Err(String::from("expected ')'"))
    }

    /// `an+b` (css-syntax-3 §6.2), including `odd` and `even`.
    fn anb(&mut self) -> R<(i32, i32)> {
        self.ws();
        let start = self.i;
        let mut t = String::new();
        while let Some(c) = self.peek() {
            if c == ')' { break }
            if (c == 'o' || c == 'O') && !t.trim().is_empty() && t.trim_end().len() < t.len() { break }
            t.push(c);
            self.i += 1;
        }
        // `of S` follows after whitespace; give it back.
        let lower = t.to_ascii_lowercase();
        let (expr, back) = match lower.find(" of ") {
            Some(k) => (&lower[..k], lower.len() - k - 1),
            None => (lower.trim_end(), 0),
        };
        if back > 0 { self.i = start + t[..t.len() - back].chars().count(); }
        let e: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
        match e.as_str() {
            "odd" => return Ok((2, 1)),
            "even" => return Ok((2, 0)),
            _ => {}
        }
        let bad = || String::from("invalid an+b");
        if let Some(k) = e.find('n') {
            let (an, rest) = (&e[..k], &e[k + 1..]);
            let a = match an { "" | "+" => 1, "-" => -1, x => x.parse::<i32>().map_err(|_| bad())? };
            let b = if rest.is_empty() { 0 } else {
                if !rest.starts_with(['+', '-']) { return Err(bad()) }
                rest.parse::<i32>().map_err(|_| bad())?
            };
            Ok((a, b))
        } else {
            Ok((0, e.parse::<i32>().map_err(|_| bad())?))
        }
    }
}

// ── Matching ────────────────────────────────────────────────────────────────

fn is_el(d: &Doc, id: u32) -> bool {
    let n = &d.nodes[id as usize];
    n.kind == ELEMENT_NODE && !n.tag.starts_with('#')
}

fn parent_el(d: &Doc, id: u32) -> Option<u32> {
    let p = d.nodes[id as usize].parent?;
    if is_el(d, p) { Some(p) } else { None }
}

fn prev_el(d: &Doc, id: u32) -> Option<u32> {
    let p = d.nodes[id as usize].parent?;
    let kids = &d.nodes[p as usize].children;
    let at = kids.iter().position(|&c| c == id)?;
    kids[..at].iter().rev().copied().find(|&c| is_el(d, c))
}

impl SelList {
    pub fn matches(&self, cx: &Ctx, el: u32) -> bool {
        is_el(cx.d, el) && self.0.iter().any(|c| match_complex(cx, el, &c.parts, None))
    }
}

/// Match `parts` with `el` as the subject. `anchor` (for `:has`) requires the
/// leftmost compound to stand in the given relation to that element.
fn match_complex(cx: &Ctx, el: u32, parts: &[(Comb, Compound)], anchor: Option<(Comb, u32)>) -> bool {
    let Some(((comb, c), rest)) = parts.split_first() else { return false };
    if !match_compound(cx, el, c) { return false }
    if rest.is_empty() {
        return match anchor {
            None => true,
            Some((k, a)) => related(cx.d, el, k, a),
        };
    }
    let d = cx.d;
    match *comb {
        Comb::Child => parent_el(d, el).is_some_and(|p| match_complex(cx, p, rest, anchor)),
        Comb::Desc => {
            let mut cur = parent_el(d, el);
            while let Some(p) = cur {
                if match_complex(cx, p, rest, anchor) { return true }
                cur = parent_el(d, p);
            }
            false
        }
        Comb::Next => prev_el(d, el).is_some_and(|p| match_complex(cx, p, rest, anchor)),
        Comb::Sub => {
            let mut cur = prev_el(d, el);
            while let Some(p) = cur {
                if match_complex(cx, p, rest, anchor) { return true }
                cur = prev_el(d, p);
            }
            false
        }
    }
}

/// Does `x` stand in relation `k` to the anchor `a` (`a k x`)?
fn related(d: &Doc, x: u32, k: Comb, a: u32) -> bool {
    match k {
        Comb::Child => d.nodes[x as usize].parent == Some(a),
        Comb::Desc => {
            let mut cur = d.nodes[x as usize].parent;
            while let Some(p) = cur {
                if p == a { return true }
                cur = d.nodes[p as usize].parent;
            }
            false
        }
        Comb::Next => prev_el(d, x) == Some(a),
        Comb::Sub => {
            let mut cur = prev_el(d, x);
            while let Some(p) = cur {
                if p == a { return true }
                cur = prev_el(d, p);
            }
            false
        }
    }
}

fn match_compound(cx: &Ctx, el: u32, c: &Compound) -> bool {
    let n = &cx.d.nodes[el as usize];
    if let Some(t) = &c.tag {
        if !n.tag.eq_ignore_ascii_case(t) { return false }
    }
    c.items.iter().all(|s| match_simple(cx, el, s))
}

fn attr_match(have: &str, op: AttrOp, want: &str, ci: bool) -> bool {
    let eq = |a: &str, b: &str| if ci { a.eq_ignore_ascii_case(b) } else { a == b };
    match op {
        AttrOp::Exists => true,
        AttrOp::Eq => eq(have, want),
        AttrOp::Word => !want.is_empty() && !want.contains(char::is_whitespace)
            && have.split_ascii_whitespace().any(|w| eq(w, want)),
        AttrOp::Dash => eq(have, want) || (have.len() > want.len() && have.is_char_boundary(want.len())
            && eq(&have[..want.len()], want) && have.as_bytes()[want.len()] == b'-'),
        AttrOp::Prefix => !want.is_empty() && have.len() >= want.len()
            && have.is_char_boundary(want.len()) && eq(&have[..want.len()], want),
        AttrOp::Suffix => !want.is_empty() && have.len() >= want.len()
            && have.is_char_boundary(have.len() - want.len()) && eq(&have[have.len() - want.len()..], want),
        AttrOp::Substr => !want.is_empty() && if ci {
            have.to_ascii_lowercase().contains(&want.to_ascii_lowercase())
        } else { have.contains(want) },
    }
}

fn is_form_control(tag: &str) -> bool {
    matches!(tag, "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset")
}

fn disabled(d: &Doc, el: u32) -> bool {
    let n = &d.nodes[el as usize];
    if !is_form_control(&n.tag) { return false }
    if n.attr("disabled").is_some() { return true }
    if &*n.tag == "option" {
        return parent_el(d, el).is_some_and(|p| &*d.nodes[p as usize].tag == "optgroup"
            && d.nodes[p as usize].attr("disabled").is_some());
    }
    // Inside a disabled `<fieldset>`, except in its first `<legend>`.
    let mut child = el;
    let mut cur = parent_el(d, el);
    while let Some(p) = cur {
        let pn = &d.nodes[p as usize];
        if &*pn.tag == "fieldset" && pn.attr("disabled").is_some() {
            let first_legend = pn.children.iter().copied()
                .find(|&c| &*d.nodes[c as usize].tag == "legend");
            if first_legend != Some(child) { return true }
        }
        child = p;
        cur = parent_el(d, p);
    }
    false
}

fn text_input(d: &Doc, el: u32) -> bool {
    let n = &d.nodes[el as usize];
    match &*n.tag {
        "textarea" => true,
        "input" => !matches!(n.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(),
            Some("checkbox" | "radio" | "button" | "submit" | "reset" | "image" | "file"
                 | "hidden" | "range" | "color")),
        _ => false,
    }
}

fn match_simple(cx: &Ctx, el: u32, s: &Simple) -> bool {
    let d = cx.d;
    let n = &d.nodes[el as usize];
    match s {
        Simple::Id(v) => n.attr("id").is_some_and(|x| **x == **v),
        Simple::Class(v) => n.attr("class").is_some_and(|c| c.split_ascii_whitespace().any(|k| k == v)),
        Simple::Attr { name, op, val, ci } => n.attrs.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .is_some_and(|(_, v)| attr_match(v, *op, val, *ci)),
        Simple::Not(l) => !l.matches(cx, el),
        Simple::Is(l) => l.matches(cx, el),
        Simple::Has(rel) => rel.iter().any(|(lead, c)| has(cx, el, *lead, c)),
        Simple::Nth { a, b, last, of_type, of } => {
            let Some(p) = d.nodes[el as usize].parent else { return false };
            let kids = &d.nodes[p as usize].children;
            let sibs: Vec<u32> = kids.iter().copied().filter(|&c| is_el(d, c)
                && (!*of_type || d.nodes[c as usize].tag == n.tag)
                && of.as_ref().is_none_or(|l| l.matches(cx, c))).collect();
            let Some(pos) = sibs.iter().position(|&c| c == el) else { return false };
            let idx = if *last { sibs.len() - pos } else { pos + 1 } as i32;
            if *a == 0 { idx == *b } else { (idx - b) % a == 0 && (idx - b) / a >= 0 }
        }
        Simple::Empty => n.children.iter().all(|&c| {
            let k = &d.nodes[c as usize];
            !(k.kind == ELEMENT_NODE || (k.kind == TEXT_NODE && !k.text.is_empty()))
        }),
        Simple::Root => d.html == Some(el) || n.parent == Some(d.doc),
        Simple::Scope => match cx.scope { Some(s) => s == el, None => d.html == Some(el) },
        Simple::Checked => match &*n.tag {
            "input" => n.checked.unwrap_or_else(|| n.attr("checked").is_some())
                && matches!(n.attr("type").map(|t| t.to_ascii_lowercase()).as_deref(),
                            Some("checkbox" | "radio")),
            "option" => n.attr("selected").is_some(),
            _ => false,
        },
        Simple::Disabled => disabled(d, el),
        Simple::Enabled => is_form_control(&n.tag) && !disabled(d, el),
        Simple::Defined => super::dombind::ce_defined(n),
        Simple::Link => matches!(&*n.tag, "a" | "area") && n.attr("href").is_some(),
        Simple::Focus => d.focused == Some(el),
        Simple::FocusWithin => {
            let mut cur = d.focused;
            while let Some(f) = cur {
                if f == el { return true }
                cur = d.nodes[f as usize].parent;
            }
            false
        }
        Simple::Target => !cx.target.is_empty() && n.attr("id").is_some_and(|x| **x == *cx.target),
        Simple::Required => matches!(&*n.tag, "input" | "select" | "textarea") && n.attr("required").is_some(),
        Simple::Optional => matches!(&*n.tag, "input" | "select" | "textarea") && n.attr("required").is_none(),
        Simple::ReadWrite => (text_input(d, el) && n.attr("readonly").is_none() && !disabled(d, el))
            || n.attr("contenteditable").is_some_and(|v| !v.eq_ignore_ascii_case("false")),
        Simple::ReadOnly => !match_simple(cx, el, &Simple::ReadWrite),
        Simple::PlaceholderShown => text_input(d, el) && n.attr("placeholder").is_some()
            && n.value.as_deref().map(|v| v.is_empty())
                .unwrap_or_else(|| &*n.tag == "textarea" && d.text_of(el).is_empty()
                                   || &*n.tag == "input" && n.attr("value").is_none_or(|v| v.is_empty())),
        Simple::Open => matches!(&*n.tag, "details" | "dialog") && n.attr("open").is_some()
            || n.ui & UI_POPOVER_OPEN != 0,
        Simple::PopoverOpen => n.ui & UI_POPOVER_OPEN != 0,
        Simple::Modal => n.ui & UI_MODAL != 0,
        Simple::State(name) => n.states.iter().any(|x| **x == **name),
        Simple::Lang(want) => {
            let mut cur = Some(el);
            while let Some(x) = cur {
                if let Some(l) = d.nodes[x as usize].attr("lang") {
                    let l = l.to_ascii_lowercase();
                    return l == *want || (l.starts_with(want.as_str()) && l.as_bytes().get(want.len()) == Some(&b'-'));
                }
                cur = d.nodes[x as usize].parent;
            }
            false
        }
        Simple::Never => false,
    }
}

/// `:has(lead complex)` on `el`.
fn has(cx: &Ctx, el: u32, lead: Comb, c: &Complex) -> bool {
    let d = cx.d;
    let mut cands = Vec::new();
    match lead {
        Comb::Desc | Comb::Child => d.descendants(el, &mut cands),
        Comb::Next | Comb::Sub => {
            let Some(p) = d.nodes[el as usize].parent else { return false };
            let kids = &d.nodes[p as usize].children;
            let at = kids.iter().position(|&k| k == el).unwrap_or(kids.len());
            for &k in &kids[at + 1..] {
                if is_el(d, k) { cands.push(k); d.descendants(k, &mut cands); }
            }
        }
    }
    cands.into_iter().any(|x| is_el(d, x) && match_complex(cx, x, &c.parts, Some((lead, el))))
}

/// All elements below `from` matching `sel`, in tree order.
pub fn query_all(cx: &Ctx, from: u32, sel: &SelList, first_only: bool) -> Vec<u32> {
    let mut cands = Vec::new();
    cx.d.descendants(from, &mut cands);
    let mut out = Vec::new();
    for c in cands {
        if sel.matches(cx, c) {
            out.push(c);
            if first_only { break }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const HTML: &str = "<html><body>\
        <div id=a class='x y' data-target='react-partial.embeddedData other' lang=en-US>\
          <p id=p1>one</p><p id=p2 hidden>two</p><span id=s1></span><p id=p3 class=last>three</p>\
        </div>\
        <ul id=u><li id=l1>a</li><li id=l2>b</li><li id=l3>c</li><li id=l4>d</li><li id=l5>e</li></ul>\
        <form><fieldset disabled><legend><input id=in1></legend><input id=in2></fieldset>\
          <input id=c1 type=checkbox checked><input id=t1 placeholder=q required></form>\
        <a id=lk href='/x'>x</a><a id=nolink>y</a><x-el id=xe></x-el>\
        </body></html>";

    fn ids(sel: &str) -> Vec<String> {
        let d = super::super::dombind::Doc::from_dom(&crate::dom::parse(HTML));
        let l = parse(sel).unwrap_or_else(|e| panic!("{sel}: {e}"));
        let cx = Ctx { d: &d, scope: d.html, target: "p2" };
        query_all(&cx, d.doc, &l, false).iter()
            .map(|&k| d.nodes[k as usize].attr("id").map(|v| v.to_string()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn attribute_operators() {
        assert_eq!(ids("[data-target~=\"react-partial.embeddedData\"]"), ["a"]);
        assert_eq!(ids("[data-target^=react]"), ["a"]);
        assert_eq!(ids("[data-target$=other]"), ["a"]);
        assert_eq!(ids("[data-target*='Data o']"), ["a"]);
        assert_eq!(ids("[lang|=en]"), ["a"]);
        assert_eq!(ids("[ID=P1 i]"), ["p1"]);
        assert_eq!(ids("[href]:not([href=''])"), ["lk"]);
    }

    #[test]
    fn structure_and_combinators() {
        assert_eq!(ids("div > p:first-child"), ["p1"]);
        assert_eq!(ids("#a p:last-of-type"), ["p3"]);
        assert_eq!(ids("li:nth-child(2n+1)"), ["l1", "l3", "l5"]);
        assert_eq!(ids("li:nth-last-child(-n+2)"), ["l4", "l5"]);
        assert_eq!(ids("li:nth-child(even of :not(#l2))"), ["l3", "l5"]);
        assert_eq!(ids("#p1 + p"), ["p2"]);
        assert_eq!(ids("#p1 ~ p"), ["p2", "p3"]);
        assert_eq!(ids("span:empty, #u > :nth-child(3)"), ["s1", "l3"]);
        assert_eq!(ids("div:has(> .last)"), ["a"]);
        assert_eq!(ids("p:has(+ span)"), ["p2"]);
        assert_eq!(ids(":is(ul, div) > :where(#l2, #p2)"), ["p2", "l2"]);
        assert_eq!(ids(":scope > body > ul"), ["u"]);
        assert_eq!(ids("p:target"), ["p2"]);
    }

    #[test]
    fn states() {
        assert_eq!(ids("input:disabled"), ["in2"]);
        assert_eq!(ids("input:enabled"), ["in1", "c1", "t1"]);
        assert_eq!(ids(":checked"), ["c1"]);
        assert_eq!(ids("input:required:placeholder-shown"), ["t1"]);
        assert_eq!(ids(":any-link"), ["lk"]);
        assert_eq!(ids("x-el:not(:defined)"), ["xe"]);
        assert_eq!(ids("p:lang(en)").len(), 3);
        assert_eq!(ids("p::before"), Vec::<String>::new());
        assert_eq!(ids("#\\61"), ["a"]);
    }

    #[test]
    fn invalid_selectors_are_errors() {
        for bad in ["", "div >", "[a=", ":nosuchthing", "p,,a", "a:not()", "#1x", "!"] {
            assert!(parse(bad).is_err(), "{bad} should not parse");
        }
        assert!(parse(":is(p, :nosuchthing)").is_ok(), ":is is forgiving");
    }
}
