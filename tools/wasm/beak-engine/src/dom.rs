//! dom.rs — tolerant HTML tree construction (WHATWG §13 subset).
//!
//! Builds a node tree (elements with attributes and children, text nodes)
//! with the tolerant recovery real pages rely on: implied `</p>`/`</li>`,
//! unmatched end tags ignored, raw-text `<script>`/`<style>`. Not the full
//! state machine; it grows toward it against html5lib-tests
//! (docs/spec/CONFORMANCE.md).
//!
//! The tree is owned (no `Rc`/arena); scripting works on its own arena and
//! writes back through `Element::bare`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::entities;

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
    /// Document-order index, assigned at parse time: a stable identity for one
    /// element across re-layouts of the same document. Form controls key their
    /// live state (typed value, checked, focus) on it. Tree position cannot
    /// serve: layout skips `display:none` subtrees, so a layout-time counter
    /// would drift from one kept during a plain DOM walk.
    pub seq: u32,
    /// Derived from `attrs` once, by `index_attrs`. Anything that later changes
    /// class, id or tag must re-run it. Cached because selector matching reads
    /// these on every element of every layout.
    pub classes: Vec<String>,
    pub id: Option<String>,
    pub bloom: crate::css::Bloom,
    /// What the source said. Live form state (a box the reader ticked) travels
    /// separately in `ElemState`.
    pub checked_attr: bool,
    pub disabled_attr: bool,
}

impl Element {
    /// Build an element from outside, the way back from the JS arena
    /// (`js::dombind::Doc::to_dom`). The derived fields are empty; the caller
    /// must run `index_attrs`, or selector matching sees neither classes nor id.
    pub fn bare(tag: String, seq: u32) -> Element { Element::new(tag, seq) }

    fn new(tag: String, seq: u32) -> Element {
        Element {
            tag,
            attrs: Vec::new(),
            children: Vec::new(),
            seq,
            classes: Vec::new(),
            id: None,
            bloom: [0; 4],
            checked_attr: false,
            disabled_attr: false,
        }
    }

    /// Fill the derived fields. Must run after `attrs` is final; for foreign
    /// content that is after the SVG name fixups.
    pub fn index_attrs(&mut self) {
        let classes: Vec<String> = self
            .attr("class")
            .map(|c| c.split_whitespace().map(String::from).collect())
            .unwrap_or_default();
        let id = self.attr("id").map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        let checked = self.attr("checked").is_some();
        let disabled = self.attr("disabled").is_some();
        self.bloom = crate::css::bloom_of(id.as_deref(), &classes);
        self.classes = classes;
        self.id = id;
        self.checked_attr = checked;
        self.disabled_attr = disabled;
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

    /// The document's root element (`<html>`), or the synthetic container when
    /// the page has none. Its style must be resolved although it is never
    /// painted: every `rem` resolves against `html { font-size }`, and the
    /// `62.5%` "1rem = 10px" idiom is common.
    pub fn root_element(&self) -> &Element {
        find_tag(&self.root, "html").unwrap_or(&self.root)
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
//
// `noscript` is raw text because scripting is enabled: HTML §13.2.5 then
// reads its content as text and builds no elements from it. Parsed as
// markup, a `<noscript>` holding `<style>div{display:none}</style>` or a
// `<meta http-equiv=refresh>` would hide or navigate away the page.
const RAWTEXT: &[&str] = &["script", "style", "noscript"];
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
    // Set once an <svg> start tag is seen; gates the foreign-content check.
    let mut saw_svg = false;

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
                // The end tag is lowercased like every other, so inside
                // foreign content it has to be adjusted the same way or it
                // never matches the start tag it closes.
                let name = if saw_svg && stack.iter().any(|e| e.tag == "svg") {
                    crate::svg::adjust_tag_name(&name)
                } else {
                    &name
                };
                close_tag(&mut stack, name);
                continue;
            }

            // Start tag. Apply the implied-end-tag recovery rules first.
            apply_implied_end_tags(&mut stack, &name);

            seq += 1;
            let mut el = Element::new(name.clone(), seq);
            parse_attrs(&raw, &mut el.attrs);
            // Foreign content keeps its case (see `svg::adjust_tag_name`). The
            // stack walk only runs on a document that has an <svg> in it at
            // all, so the common page pays nothing.
            if name == "svg" {
                saw_svg = true;
            }
            if saw_svg && (name == "svg" || stack.iter().any(|e| e.tag == "svg")) {
                el.tag = crate::svg::adjust_tag_name(&el.tag).to_string();
                for (k, _) in el.attrs.iter_mut() {
                    let fixed = crate::svg::adjust_attr_name(k);
                    if fixed != k {
                        *k = fixed.to_string();
                    }
                }
            }
            el.index_attrs();

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
    let mut root = stack.pop().unwrap();
    imply_html_body(&mut root);
    imply_details_summary(&mut root, &mut seq);
    Dom { root }
}

/// "If there is no child summary element, the user agent should provide its
/// own legend" (HTML §4.11.1). Without one a closed `<details>` renders as
/// nothing and its contents are unreachable, so the control is not optional.
///
/// A browser puts this in the shadow tree; we insert it into the DOM, which
/// shifts the `<details>`' own children by one in `:nth-child`. Summary-less
/// `<details>` selected by index are rare enough to accept that.
fn imply_details_summary(root: &mut Element, seq: &mut u32) {
    fn walk(el: &mut Element, seq: &mut u32) {
        if el.tag == "details"
            && !el.children.iter().any(|c| matches!(c, Node::Element(e) if e.tag == "summary"))
        {
            *seq += 1;
            let mut s = Element::new(String::from("summary"), *seq);
            s.children.push(Node::Text(String::from("Details")));
            el.children.insert(0, Node::Element(s));
        }
        for c in &mut el.children {
            if let Node::Element(e) = c {
                walk(e, seq);
            }
        }
    }
    walk(root, seq);
}

/// HTML's tree construction inserts `<html>` and `<body>` even when the
/// source omits the tags (HTML §13.2.6), and both are ordinary elements the
/// cascade can target; without them `html { … }` / `body { … }` match
/// nothing. `seq` stays 0 on the implied elements: they carry no form state,
/// and every parsed element keeps its index.
fn imply_html_body(root: &mut Element) {
    fn make(tag: &str, children: Vec<Node>) -> Element {
        let mut el = Element::new(String::from(tag), 0);
        el.children = children;
        el
    }
    fn is_tag(n: &Node, tag: &str) -> bool {
        matches!(n, Node::Element(e) if e.tag == tag)
    }
    // Metadata stays a child of <html>, as the parser would place it in <head>.
    fn is_metadata(n: &Node) -> bool {
        matches!(n, Node::Element(e) if matches!(e.tag.as_str(),
            "head" | "title" | "meta" | "link" | "base" | "style" | "script" | "noscript"))
    }

    if !root.children.iter().any(|c| is_tag(c, "html")) {
        let kids = core::mem::take(&mut root.children);
        root.children.push(Node::Element(make("html", kids)));
    }
    for c in &mut root.children {
        if let Node::Element(html) = c {
            if html.tag != "html" || html.children.iter().any(|k| is_tag(k, "body")) {
                continue;
            }
            let (meta, rest): (Vec<Node>, Vec<Node>) =
                core::mem::take(&mut html.children).into_iter().partition(is_metadata);
            html.children = meta;
            html.children.push(Node::Element(make("body", rest)));
        }
    }
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
            let (d, adv) = decode_entity_at(&s[i..], true);
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
fn decode_entity(s: &str) -> (String, usize) { decode_entity_at(s, false) }

/// Like `decode_entity`, but knowing whether we are inside an attribute.
///
/// Uses the full named table (`entities::NAMED`). Two rules from WHATWG
/// §13.2.5.72 f.:
///
/// * Without `;` only the legacy names apply (the 106 HTML still accepts
///   without a semicolon), longest match first.
/// * Inside an attribute a legacy name is not replaced when `=` or an
///   alphanumeric follows, so `?a&copy=1` stays a query string.
fn decode_entity_at(s: &str, in_attr: bool) -> (String, usize) {
    debug_assert!(s.starts_with('&'));
    let rest = &s[1..];

    // Numeric: `&#123;` / `&#x7b;`. The reader must not run past the digits,
    // or a `&#` without `;` would swallow the following text.
    if let Some(num) = rest.strip_prefix('#') {
        let hex = num.starts_with(['x', 'X']);
        let digits = &num[usize::from(hex)..];
        let n = digits.chars()
            .take_while(|c| if hex { c.is_ascii_hexdigit() } else { c.is_ascii_digit() })
            .count();
        if n > 0 {
            let body = &digits[..n];
            let cp = if hex { u32::from_str_radix(body, 16).ok() } else { body.parse::<u32>().ok() };
            // Include the `;` if present; without it the reference is still valid
            // (with a parse error).
            let semi = usize::from(digits[n..].starts_with(';'));
            let adv = 1 + 1 + usize::from(hex) + n + semi;
            return (cp.and_then(char::from_u32).map(String::from).unwrap_or_default(), adv);
        }
        return (String::from("&"), 1);
    }

    // Named. The name runs to the first character that cannot be part of
    // one, capped at the longest name in the table.
    let name_len = rest.char_indices()
        .take_while(|(k, c)| *k < entities::MAX_NAME && c.is_ascii_alphanumeric())
        .map(|(k, c)| k + c.len_utf8())
        .last().unwrap_or(0);
    let name = &rest[..name_len];

    if rest[name_len..].starts_with(';') {
        if let Some(c) = entities::lookup(name) {
            return (String::from(c), 1 + name_len + 1);
        }
        // Unknown name: leave it literal.
        let mut lit = String::from("&");
        lit.push_str(name);
        lit.push(';');
        return (lit, 1 + name_len + 1);
    }

    // No semicolon: legacy names only, and inside an attribute only when
    // neither `=` nor an alphanumeric follows.
    if name_len > 0 {
        if let Some((c, n)) = entities::longest_legacy(name) {
            let next = rest[n..].chars().next();
            let blocked = in_attr
                && next.is_some_and(|c| c == '=' || c.is_ascii_alphanumeric());
            if !blocked {
                return (String::from(c), 1 + n);
            }
        }
    }
    (String::from("&"), 1)
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

    /// Every named reference is replaced, not only a handful.
    #[test]
    fn jeder_benannte_verweis_wird_ersetzt() {
        let dom = parse("<p>a&ZeroWidthSpace;b &times; 3&deg; &middot; &shy;x &nixda;</p>");
        assert_eq!(text_of(dom.body()),
                   "a\u{200b}b \u{d7} 3\u{b0} \u{b7} \u{ad}x &nixda;");
    }

    /// Without `;` only legacy names apply, and inside an attribute not even
    /// those when `=` or a letter follows (WHATWG §13.2.5.73).
    #[test]
    fn eine_altlast_ohne_semikolon_zerstoert_keinen_link() {
        let dom = parse("<a href=\"/x?a&copy=1&amp;b=2\" title=\"&copy 2026\">&copy 2026</a>");
        let a = match &dom.body().children[0] { Node::Element(e) => e, _ => panic!() };
        assert_eq!(a.attr("href"), Some("/x?a&copy=1&b=2"));
        // No `=` after it: replaced, in the attribute as in text.
        assert_eq!(a.attr("title"), Some("\u{a9} 2026"));
        assert_eq!(text_of(a), "\u{a9} 2026");
    }

    /// The numeric reader must not run past its digits.
    #[test]
    fn ein_numerischer_verweis_endet_an_seinen_ziffern() {
        let dom = parse("<p>&#65;&#x42;&#67 D &# E</p>");
        // `&#67` without `;` is a parse error but still replaced
        // (WHATWG §13.2.5.80), so the C joins the B.
        assert_eq!(text_of(dom.body()), "ABC D &# E");
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
    fn foreign_content_keeps_its_case() {
        // SVG is case-sensitive; the HTML tokenizer is not. Without the spec's
        // adjustment an inline icon arrives with `viewbox` and has no
        // coordinate system at all.
        let dom = parse(
            "<body><svg viewBox=\"0 0 16 16\" width=\"16\"><clipPath id=\"c\">\
             <path d=\"M0 0\"/></clipPath><lineargradient gradientUnits=\"userSpaceOnUse\"/>\
             </svg><div viewBox=\"x\">after</div></body>",
        );
        let svg = as_el(&dom.body().children[0]);
        assert_eq!(svg.attr("viewBox"), Some("0 0 16 16"));
        assert_eq!(svg.attr("width"), Some("16"), "plain names are untouched");
        assert_eq!(child_tags(svg), ["clipPath", "linearGradient"]);
        // The end tag is lowercased too — if it were not adjusted, </clipPath>
        // would not close its start tag and the tree would nest wrongly.
        assert_eq!(child_tags(as_el(&svg.children[0])), ["path"]);
        assert_eq!(
            as_el(&svg.children[1]).attr("gradientUnits"),
            Some("userSpaceOnUse")
        );
        // Outside foreign content nothing changes.
        let div = as_el(&dom.body().children[1]);
        assert_eq!(div.attr("viewbox"), Some("x"));
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
