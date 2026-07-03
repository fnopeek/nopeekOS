//! css.rs — author stylesheet parsing (css-syntax-3 subset) + selector match.
//!
//! Slice-0.2 gave us the UA sheet (as data) + inline `style="…"`. This adds the
//! middle cascade layer: author rules from `<style>` blocks. The pipeline is
//! now the real thing —
//!
//! ```text
//!   inherited(parent) → UA sheet → author <style> (specificity) → inline
//! ```
//!
//! Selectors: type / `.class` / `#id` / `*`, compounds (`div.a#b`), descendant
//! (space) + child (`>`) combinators, comma lists. Right-to-left matching with
//! an ancestor stack + `(id, class, type)` specificity. Unsupported bits
//! (pseudo-classes, `[attr]`, `+`/`~` siblings, `@media` bodies) are dropped,
//! not mis-applied — forward-compatible like a browser (CONFORMANCE.md).
//! External `<link>` stylesheets need a sub-resource fetch → later; the parser
//! + cascade here are exactly what that will reuse.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

/// The identity a selector matches against: tag + id + classes.
#[derive(Clone)]
pub struct ElemInfo {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

impl ElemInfo {
    pub fn of(el: &Element) -> ElemInfo {
        let id = el.attr("id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let classes = el
            .attr("class")
            .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        ElemInfo { tag: el.tag.clone(), id, classes }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Comb {
    Descendant,
    Child,
}

/// A compound selector: an optional type + optional id + zero-or-more classes.
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

impl Compound {
    fn matches(&self, e: &ElemInfo) -> bool {
        if let Some(t) = &self.tag {
            if *t != e.tag {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if e.id.as_deref() != Some(id.as_str()) {
                return false;
            }
        }
        self.classes.iter().all(|c| e.classes.iter().any(|x| x == c))
    }
}

/// A complex selector: compounds left→right, with the combinator that precedes
/// each compound after the first (`combs[k]` sits left of `compounds[k+1]`).
pub struct Selector {
    compounds: Vec<Compound>,
    combs: Vec<Comb>,
    spec: u32,
}

impl Selector {
    /// Right-to-left match: the last compound must match `subject`, then earlier
    /// compounds must match ancestors per their combinators. `ancestors` is
    /// root→…→parent order. Descendant matching is nearest-first (no backtrack —
    /// enough for content selectors; noted as a shortcut).
    fn matches(&self, subject: &ElemInfo, ancestors: &[ElemInfo]) -> bool {
        let last = self.compounds.len() - 1;
        if !self.compounds[last].matches(subject) {
            return false;
        }
        let mut anc = ancestors.len() as isize - 1; // immediate parent
        let mut ci = last as isize - 1;
        while ci >= 0 {
            let comb = self.combs[ci as usize];
            let comp = &self.compounds[ci as usize];
            match comb {
                Comb::Child => {
                    if anc < 0 || !comp.matches(&ancestors[anc as usize]) {
                        return false;
                    }
                    anc -= 1;
                }
                Comb::Descendant => {
                    let mut a = anc;
                    let mut found = false;
                    while a >= 0 {
                        if comp.matches(&ancestors[a as usize]) {
                            found = true;
                            break;
                        }
                        a -= 1;
                    }
                    if !found {
                        return false;
                    }
                    anc = a - 1;
                }
            }
            ci -= 1;
        }
        true
    }
}

/// One `selectors { declarations }` rule; `order` is document position (for
/// same-specificity tie-breaking, last wins).
pub struct Rule {
    selectors: Vec<Selector>,
    decls: Vec<(String, String)>,
    order: u32,
}

/// A parsed author stylesheet.
pub struct Stylesheet {
    rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn empty() -> Stylesheet {
        Stylesheet { rules: Vec::new() }
    }
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// All declaration blocks that match `subject` (given its `ancestors`),
    /// each tagged with the winning selector's specificity + document order.
    /// Caller sorts ascending and applies in order (later overrides earlier).
    pub fn matched<'a>(
        &'a self,
        subject: &ElemInfo,
        ancestors: &[ElemInfo],
    ) -> Vec<(u32, u32, &'a [(String, String)])> {
        let mut out = Vec::new();
        for rule in &self.rules {
            let mut best: Option<u32> = None;
            for sel in &rule.selectors {
                if sel.matches(subject, ancestors) {
                    best = Some(best.map_or(sel.spec, |b| b.max(sel.spec)));
                }
            }
            if let Some(spec) = best {
                out.push((spec, rule.order, rule.decls.as_slice()));
            }
        }
        out
    }
}

/// Gather + parse every `<style>` block in the document into one stylesheet.
pub fn collect(dom: &Dom) -> Stylesheet {
    let mut css = String::new();
    gather_style_text(&dom.root, &mut css);
    if css.is_empty() {
        return Stylesheet::empty();
    }
    parse(&css)
}

fn gather_style_text(el: &Element, out: &mut String) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.tag == "style" {
                for cc in &e.children {
                    if let Node::Text(t) = cc {
                        out.push_str(t);
                        out.push('\n');
                    }
                }
            } else {
                gather_style_text(e, out);
            }
        }
    }
}

/// Parse a stylesheet body into rules (css-syntax-3 subset).
pub fn parse(css: &str) -> Stylesheet {
    let css = strip_comments(css);
    let bytes = css.as_bytes();
    let mut rules = Vec::new();
    let mut i = 0;
    let mut order = 0u32;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'@' {
            i = skip_at_rule(bytes, i);
            continue;
        }
        let sel_start = i;
        while i < bytes.len() && bytes[i] != b'{' && bytes[i] != b'}' {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'}' {
            break;
        }
        let sel_text = &css[sel_start..i];
        i += 1; // '{'
        let body_start = i;
        while i < bytes.len() && bytes[i] != b'}' {
            i += 1;
        }
        let body = &css[body_start..i.min(css.len())];
        if i < bytes.len() {
            i += 1; // '}'
        }
        let selectors = parse_selector_list(sel_text);
        let decls = parse_decls(body);
        if !selectors.is_empty() && !decls.is_empty() {
            rules.push(Rule { selectors, decls, order });
            order += 1;
        }
    }
    Stylesheet { rules }
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let b = css.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(css[i..].chars().next().unwrap());
            i += css[i..].chars().next().unwrap().len_utf8();
        }
    }
    out
}

fn skip_at_rule(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && bytes[i] != b';' && bytes[i] != b'{' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'{' {
        let mut depth = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i + 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        i
    } else {
        (i + 1).min(bytes.len())
    }
}

fn parse_selector_list(text: &str) -> Vec<Selector> {
    text.split(',').filter_map(|s| parse_selector(s.trim())).collect()
}

fn parse_selector(text: &str) -> Option<Selector> {
    let mut compounds: Vec<Compound> = Vec::new();
    let mut combs: Vec<Comb> = Vec::new();
    let mut pending_child = false;
    for tok in tokenize_selector(text) {
        if tok == ">" {
            pending_child = true;
            continue;
        }
        let comp = parse_compound(&tok)?;
        if !compounds.is_empty() {
            combs.push(if pending_child { Comb::Child } else { Comb::Descendant });
        }
        compounds.push(comp);
        pending_child = false;
    }
    if compounds.is_empty() {
        return None;
    }
    let spec = specificity(&compounds);
    Some(Selector { compounds, combs, spec })
}

/// Split into compound tokens + `>` tokens on whitespace / `>` boundaries.
fn tokenize_selector(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !cur.is_empty() {
                out.push(core::mem::take(&mut cur));
            }
        } else if ch == '>' {
            if !cur.is_empty() {
                out.push(core::mem::take(&mut cur));
            }
            out.push(">".to_string());
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_compound(tok: &str) -> Option<Compound> {
    if tok.is_empty() || tok.contains([':', '[', ']', '+', '~', '(', ')']) {
        return None; // unsupported selector features → drop the whole selector
    }
    let bytes = tok.as_bytes();
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut i = 0;
    if bytes[0] == b'*' {
        i = 1;
    } else if bytes[0].is_ascii_alphabetic() {
        let s = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
            i += 1;
        }
        tag = Some(tok[s..i].to_ascii_lowercase());
    }
    while i < bytes.len() {
        let marker = bytes[i];
        if marker != b'.' && marker != b'#' {
            return None;
        }
        i += 1;
        let s = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-' || bytes[i] == b'_') {
            i += 1;
        }
        if i == s {
            return None;
        }
        if marker == b'.' {
            classes.push(tok[s..i].to_string());
        } else {
            id = Some(tok[s..i].to_string());
        }
    }
    Some(Compound { tag, id, classes })
}

/// CSS specificity packed as (id<<20)|(class<<10)|type — enough headroom.
fn specificity(compounds: &[Compound]) -> u32 {
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for comp in compounds {
        if comp.id.is_some() {
            a += 1;
        }
        b += comp.classes.len() as u32;
        if comp.tag.is_some() {
            c += 1;
        }
    }
    (a << 20) | (b << 10) | c
}

fn parse_decls(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for decl in body.split(';') {
        let mut it = decl.splitn(2, ':');
        let (p, v) = match (it.next(), it.next()) {
            (Some(p), Some(v)) => (p.trim(), v.trim()),
            _ => continue,
        };
        if !p.is_empty() && !v.is_empty() {
            out.push((p.to_ascii_lowercase(), v.to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn info(tag: &str, id: Option<&str>, classes: &[&str]) -> ElemInfo {
        ElemInfo {
            tag: tag.to_string(),
            id: id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parses_and_matches_type_class_id() {
        let ss = parse("p { color: red } .lead { font-weight: bold } #main { color: blue }");
        assert!(!ss.matched(&info("p", None, &[]), &[]).is_empty());
        assert!(!ss.matched(&info("div", None, &["lead"]), &[]).is_empty());
        assert!(!ss.matched(&info("div", Some("main"), &[]), &[]).is_empty());
        assert!(ss.matched(&info("span", None, &[]), &[]).is_empty());
    }

    #[test]
    fn descendant_and_child_combinators() {
        let ss = parse("nav a { color: green } ul > li { color: teal }");
        let nav = info("nav", None, &[]);
        let div = info("div", None, &[]);
        let ul = info("ul", None, &[]);
        // nav a: matches an <a> with <nav> anywhere above
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone()]).is_empty());
        assert!(!ss.matched(&info("a", None, &[]), &[nav.clone(), div.clone()]).is_empty());
        assert!(ss.matched(&info("a", None, &[]), &[div.clone()]).is_empty());
        // ul > li: <li> whose IMMEDIATE parent is <ul>
        assert!(!ss.matched(&info("li", None, &[]), &[ul.clone()]).is_empty());
        assert!(ss.matched(&info("li", None, &[]), &[ul.clone(), div.clone()]).is_empty());
    }

    #[test]
    fn specificity_ranks_id_over_class_over_type() {
        let ss = parse("p { color: a } p.x { color: b } #p { color: c }");
        let e = info("p", Some("p"), &["x"]);
        let mut m = ss.matched(&e, &[]);
        m.sort_by_key(|(spec, order, _)| (*spec, *order));
        // ascending: type(p) < class(p.x) < id(#p)
        assert_eq!(m.len(), 3);
        assert!(m[0].0 < m[1].0 && m[1].0 < m[2].0);
    }

    #[test]
    fn comments_at_rules_and_bad_selectors_are_tolerated() {
        let ss = parse(
            "/* c */ @media screen { p { color: x } } \
             a:hover { color: y } input[type=text] { color: z } \
             h1, h2 { color: ok }",
        );
        // @media body dropped, :hover + [attr] selectors dropped …
        assert!(ss.matched(&info("a", None, &[]), &[]).is_empty());
        assert!(ss.matched(&info("input", None, &[]), &[]).is_empty());
        // … but the plain "h1, h2" list still parsed.
        assert!(!ss.matched(&info("h2", None, &[]), &[]).is_empty());
    }

    #[test]
    fn collects_style_blocks_from_dom() {
        let dom = dom::parse("<html><head><style>p{color:red}</style></head>\
            <body><style>.x{color:blue}</style><p>hi</p></body></html>");
        let ss = collect(&dom);
        assert!(!ss.matched(&info("p", None, &[]), &[]).is_empty());
        assert!(!ss.matched(&info("span", None, &["x"]), &[]).is_empty());
    }
}
