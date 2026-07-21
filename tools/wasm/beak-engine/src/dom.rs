//! dom.rs — tolerant HTML tree construction (WHATWG §13 subset).
//!
//! Slice-0 emitted a flat `Vec<Block>`; that threw away the structure both
//! inline flow and the CSS cascade need. This builds a real node tree —
//! elements with attributes + children, and text nodes — with the tolerant
//! recovery real pages rely on (implied `</p>`/`</li>`, unmatched end tags
//! ignored, raw-text `<script>`/`<style>`). Not the full state machine yet;
//! it grows toward it against html5lib-tests (CONFORMANCE.md).
//!
//! The tree is owned (no `Rc`/arena) — enough for a render-only pass. Live
//! mutation (Stage 2, JS-driven) will move to an arena; the shape stays.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A DOM node: an element subtree or a run of text.
pub enum Node {
    Element(Element),
    Text(String),
}

/// An element: lowercased tag, attributes, child nodes.
pub struct Element {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    /// Document-order index, assigned at parse time. A stable identity for one
    /// element across re-layouts of the SAME document — form controls key their
    /// live state (typed value, checked, focus) on it. Tree position can't serve
    /// as the key: layout skips `display:none` subtrees, so any counter kept
    /// during layout would drift from one kept during a plain DOM walk.
    pub seq: u32,
}

impl Element {
    fn new(tag: String, seq: u32) -> Element {
        Element { tag, attrs: Vec::new(), children: Vec::new(), seq }
    }

    /// Value of `name` (case-insensitive, already lowercased at parse time).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The parsed document. `root` is a synthetic container holding the top-level
/// nodes (`<html>`/`<body>`/…); layout starts from `<body>` if present.
pub struct Dom {
    pub root: Element,
}

impl Dom {
    /// The `<body>` element to lay out, or the root if the page has none.
    pub fn body(&self) -> &Element {
        find_tag(&self.root, "body").unwrap_or(&self.root)
    }
}

/// First descendant element with tag `tag` (depth-first).
fn find_tag<'a>(el: &'a Element, tag: &str) -> Option<&'a Element> {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.tag == tag {
                return Some(e);
            }
            if let Some(found) = find_tag(e, tag) {
                return Some(found);
            }
        }
    }
    None
}

// Elements that never have children / an end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "meta", "param",
    "source", "track", "wbr",
];
// Raw-text elements: everything up to the matching close is literal text.
const RAWTEXT: &[&str] = &["script", "style"];
// Block-level starters that imply a `</p>` when a `<p>` is still open.
const BLOCK_STARTERS: &[&str] = &[
    "address", "article", "aside", "blockquote", "details", "div", "dl", "fieldset", "figcaption",
    "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hr", "main", "nav",
    "ol", "p", "pre", "section", "table", "ul",
];

/// Tolerant HTML → DOM tree. Recovers from the malformation real pages ship;
/// see the module doc for the subset covered.
pub fn parse(html: &str) -> Dom {
    // Open-element stack; index 0 is the synthetic root and is never popped.
    let mut stack: Vec<Element> = Vec::with_capacity(16);
    let mut seq: u32 = 0;
    stack.push(Element::new("#root".to_string(), seq));

    let bytes = html.as_bytes();
    let mut i = 0usize;
    let n = bytes.len();
    let mut text = String::new();

    while i < n {
        if bytes[i] == b'<' && i + 1 < n && is_tagish(bytes[i + 1]) {
            flush_text(&mut stack, &mut text);

            // Comment / doctype / CDATA-ish: `<!...>` (and `<?...>`).
            if bytes[i + 1] == b'!' || bytes[i + 1] == b'?' {
                i = skip_bogus(bytes, i);
                continue;
            }

            let (raw, next) = read_tag(bytes, i);
            i = next;
            let closing = raw.starts_with('/');
            let name = tag_name(&raw);
            if name.is_empty() {
                continue;
            }

            if closing {
                close_tag(&mut stack, &name);
                continue;
            }

            // Start tag. Apply the implied-end-tag recovery rules first.
            apply_implied_end_tags(&mut stack, &name);

            seq += 1;
            let mut el = Element::new(name.clone(), seq);
            parse_attrs(&raw, &mut el.attrs);

            if RAWTEXT.contains(&name.as_str()) {
                let (content, after) = read_rawtext(html, i, &name);
                i = after;
                if !content.is_empty() {
                    el.children.push(Node::Text(content));
                }
                push_child(&mut stack, el);
            } else if VOID.contains(&name.as_str()) || raw.trim_end().ends_with('/') {
                push_child(&mut stack, el);
            } else {
                stack.push(el);
            }
        } else {
            // Text: decode entities as we accumulate; whitespace stays raw and
            // is collapsed at layout (per `white-space: normal`).
            let ch = html[i..].chars().next().unwrap();
            if ch == '&' {
                let (decoded, adv) = decode_entity(&html[i..]);
                text.push_str(&decoded);
                i += adv;
            } else {
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    flush_text(&mut stack, &mut text);
    // Close any elements left open (malformed / truncated document).
    while stack.len() > 1 {
        let el = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(Node::Element(el));
    }
    Dom { root: stack.pop().unwrap() }
}

/// The document `<title>` text, cleaned, if present.
pub fn title(dom: &Dom) -> Option<String> {
    let t = find_tag(&dom.root, "title")?;
    let mut s = String::new();
    for c in &t.children {
        if let Node::Text(txt) = c {
            s.push_str(txt);
        }
    }
    let s = collapse_ws(&s);
    if s.is_empty() { None } else { Some(s) }
}

// ── tree-builder helpers ───────────────────────────────────────────────────

fn flush_text(stack: &mut [Element], text: &mut String) {
    if text.is_empty() {
        return;
    }
    // Keep whitespace-only text too: between inline elements it is a
    // significant space (`</a> <a>` ≠ `</a><a>`); the inline collapser turns
    // it into a single space, and in a block context it collapses to nothing.
    let t = core::mem::take(text);
    stack.last_mut().unwrap().children.push(Node::Text(t));
}

fn push_child(stack: &mut [Element], el: Element) {
    stack.last_mut().unwrap().children.push(Node::Element(el));
}

/// Close the nearest matching open element (tolerant: unmatched → ignored).
fn close_tag(stack: &mut Vec<Element>, name: &str) {
    let target = (1..stack.len()).rev().find(|&idx| stack[idx].tag == name);
    if let Some(idx) = target {
        while stack.len() > idx {
            let el = stack.pop().unwrap();
            stack.last_mut().unwrap().children.push(Node::Element(el));
        }
    }
}

/// HTML's optional-end-tag recovery for the cases content pages hit: a new
/// block closes an open `<p>`; a new `<li>`/`<dt>`/`<dd>` closes its sibling.
fn apply_implied_end_tags(stack: &mut Vec<Element>, starting: &str) {
    let top = stack.last().unwrap().tag.as_str();
    if top == "p" && BLOCK_STARTERS.contains(&starting) {
        close_tag(stack, "p");
    } else if (starting == "li" && top == "li")
        || (matches!(starting, "dt" | "dd") && matches!(top, "dt" | "dd"))
        || (matches!(starting, "td" | "th") && matches!(top, "td" | "th"))
        || (starting == "tr" && top == "tr")
        || (starting == "option" && top == "option")
    {
        let t = top.to_string();
        close_tag(stack, &t);
    }
}

// ── tag / attribute lexing ─────────────────────────────────────────────────

fn is_tagish(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'/' || b == b'!' || b == b'?'
}

/// Read a tag body between `<` and the matching `>`, honoring quotes so a `>`
/// inside an attribute value doesn't end the tag. Returns (body, index-after-`>`).
fn read_tag(bytes: &[u8], start: usize) -> (String, usize) {
    let mut i = start + 1; // skip '<'
    let mut quote = 0u8;
    let body_start = i;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            let body = core::str::from_utf8(&bytes[body_start..i]).unwrap_or("").to_string();
            return (body, i + 1);
        }
        i += 1;
    }
    (core::str::from_utf8(&bytes[body_start..]).unwrap_or("").to_string(), bytes.len())
}

/// Skip a `<!-- comment -->`, `<!doctype …>`, or `<?…?>` bogus construct.
fn skip_bogus(bytes: &[u8], start: usize) -> usize {
    if bytes[start..].starts_with(b"<!--") {
        // to "-->"
        let mut i = start + 4;
        while i + 2 < bytes.len() {
            if &bytes[i..i + 3] == b"-->" {
                return i + 3;
            }
            i += 1;
        }
        return bytes.len();
    }
    // to the next '>'
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Lowercased tag name after an optional leading `/`.
fn tag_name(raw: &str) -> String {
    let raw = raw.trim_start_matches('/').trim_start();
    let mut name = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || (name.is_empty() && ch.is_ascii_alphabetic()) {
            name.push(ch.to_ascii_lowercase());
        } else {
            break;
        }
    }
    name
}

/// Parse `name="v"` / `name='v'` / `name=v` / bare `name` attribute pairs from
/// a tag body. Attribute names are lowercased; values keep their case and are
/// entity-decoded.
fn parse_attrs(raw: &str, out: &mut Vec<(String, String)>) {
    let bytes = raw.as_bytes();
    // Skip the tag name.
    let mut i = 0;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b'/' {
            break;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() && bytes[i] != b'/' {
            i += 1;
        }
        let name = raw[name_start..i].to_ascii_lowercase();
        if name.is_empty() {
            i += 1;
            continue;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let vs = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                let v = &raw[vs..i.min(raw.len())];
                i += 1; // closing quote
                decode_all(v)
            } else {
                let vs = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                decode_all(&raw[vs..i])
            }
        } else {
            String::new()
        };
        out.push((name, value));
    }
}

/// Read raw-text-element content up to `</name>` (case-insensitive). Returns
/// (decoded-content, index-just-after-the-close-tag).
fn read_rawtext(html: &str, start: usize, name: &str) -> (String, usize) {
    let bytes = html.as_bytes();
    let close = name.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            let rest = &bytes[i + 2..];
            if rest.len() >= close.len()
                && rest[..close.len()].eq_ignore_ascii_case(close)
                && rest.get(close.len()).is_none_or(|&b| b == b'>' || b.is_ascii_whitespace())
            {
                let content = html[start..i].to_string();
                // advance past "</name ... >"
                let mut j = i + 2 + close.len();
                while j < bytes.len() && bytes[j] != b'>' {
                    j += 1;
                }
                return (content, (j + 1).min(bytes.len()));
            }
        }
        i += 1;
    }
    (html[start..].to_string(), bytes.len())
}

// ── entities + whitespace ──────────────────────────────────────────────────

/// Decode every entity in `s` (used for attribute values / title text).
fn decode_all(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let ch = s[i..].chars().next().unwrap();
        if ch == '&' {
            let (d, adv) = decode_entity(&s[i..]);
            out.push_str(&d);
            i += adv;
        } else {
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Decode one entity starting at `&…`. Returns (text, bytes-consumed). On no
/// match, consumes just the `&` and returns it literally.
fn decode_entity(s: &str) -> (String, usize) {
    debug_assert!(s.starts_with('&'));
    let mut name = String::new();
    let mut consumed = 1; // the '&'
    for ch in s[1..].chars() {
        if ch == ';' {
            consumed += 1;
            let out = match name.as_str() {
                "amp" => "&".into(),
                "lt" => "<".into(),
                "gt" => ">".into(),
                "quot" => "\"".into(),
                "apos" => "'".into(),
                "nbsp" => "\u{00A0}".into(),
                "mdash" => "\u{2014}".into(),
                "ndash" => "\u{2013}".into(),
                "hellip" => "\u{2026}".into(),
                "copy" => "\u{00A9}".into(),
                "reg" => "\u{00AE}".into(),
                "trade" => "\u{2122}".into(),
                "raquo" => "\u{00BB}".into(),
                "laquo" => "\u{00AB}".into(),
                "quot;" => "\"".into(),
                _ => {
                    if let Some(rest) = name.strip_prefix('#') {
                        let cp = if let Some(h) = rest.strip_prefix(['x', 'X']) {
                            u32::from_str_radix(h, 16).ok()
                        } else {
                            rest.parse::<u32>().ok()
                        };
                        cp.and_then(char::from_u32).map(String::from).unwrap_or_default()
                    } else {
                        // unknown named entity → keep literal
                        let mut lit = String::from("&");
                        lit.push_str(&name);
                        lit.push(';');
                        lit
                    }
                }
            };
            return (out, consumed);
        }
        if ch == '&' || ch.is_whitespace() || name.len() > 30 {
            break;
        }
        name.push(ch);
        consumed += ch.len_utf8();
    }
    ("&".into(), 1)
}

/// Collapse runs of whitespace to a single space, trimming the ends.
pub fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child_tags(el: &Element) -> Vec<&str> {
        el.children
            .iter()
            .filter_map(|c| match c {
                Node::Element(e) => Some(e.tag.as_str()),
                _ => None,
            })
            .collect()
    }
    fn text_of(el: &Element) -> String {
        let mut s = String::new();
        for c in &el.children {
            match c {
                Node::Text(t) => s.push_str(t),
                Node::Element(e) => s.push_str(&text_of(e)),
            }
        }
        collapse_ws(&s)
    }

    #[test]
    fn builds_a_tree_not_a_flat_list() {
        let dom = parse("<html><head><title>Hi &amp; Bye</title></head><body>\
            <h1>Head</h1><p>Hello <a href=\"/next\">Next <b>page</b></a> now</p></body></html>");
        assert_eq!(title(&dom), Some("Hi & Bye".to_string()));
        let body = dom.body();
        assert_eq!(child_tags(body), ["h1", "p"]);
        // The <p> keeps its inline structure: text, an <a> element, text.
        let p = match &body.children[1] {
            Node::Element(e) => e,
            _ => panic!(),
        };
        assert_eq!(child_tags(p), ["a"]);
        let a = match &p.children[1] {
            Node::Element(e) => e,
            _ => panic!("expected <a> as p's 2nd child"),
        };
        assert_eq!(a.attr("href"), Some("/next"));
        assert_eq!(text_of(a), "Next page");
        assert_eq!(text_of(p), "Hello Next page now");
    }

    #[test]
    fn implied_end_tags_recover_unclosed_p_and_li() {
        let dom = parse("<body><ul><li>one<li>two<li>three</ul><p>a<p>b</body>");
        let body = dom.body();
        assert_eq!(child_tags(body), ["ul", "p", "p"]);
        let ul = match &body.children[0] {
            Node::Element(e) => e,
            _ => panic!(),
        };
        assert_eq!(child_tags(ul), ["li", "li", "li"]);
    }

    #[test]
    fn script_and_style_are_raw_and_dont_leak() {
        let dom = parse("<body><style>a{color:red} b>c{x:1}</style>\
            <p>keep</p><script>if (a < b) evil('</p>')</script></body>");
        let body = dom.body();
        assert_eq!(child_tags(body), ["style", "p", "script"]);
        // The '<' inside the script did not break out into new elements.
        assert_eq!(text_of(&as_el(&body.children[1])), "keep");
    }

    #[test]
    fn comments_skip_even_with_inner_markup() {
        let dom = parse("<body><p>before</p><!-- <li><a>x</a> --><p>after</p></body>");
        assert_eq!(child_tags(dom.body()), ["p", "p"]);
    }

    fn as_el(n: &Node) -> &Element {
        match n {
            Node::Element(e) => e,
            _ => panic!("expected element"),
        }
    }
}
