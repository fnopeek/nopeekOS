#![cfg_attr(not(test), no_std)]
//! beak-engine — portable browser engine core (nopeekOS `beak`).
//!
//! Pure `no_std` + `alloc`, **no host-fn dependencies** → the whole engine
//! builds and unit-tests on any target with no OS in the loop (BROWSER.md
//! §10). Slice 0 is a tolerant HTML → platform-neutral `Block` lowering; the
//! real DOM / CSS / layout / JS grow inside this crate over time.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A platform-neutral piece of rendered content. The engine parses HTML into
/// these; each platform adapter maps them to its own presentation (nopeek →
/// widget tree, desktop → painted boxes). Deliberately host-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: String },
    Para(String),
    ListItem(String),
    Link { text: String, href: String },
    Rule,
}

#[derive(Clone, Copy)]
enum Kind {
    Para,
    Heading(u8),
    ListItem,
}

/// Tolerant HTML → blocks. Strips tags, decodes common entities, collapses
/// whitespace, recognizes `h1`–`h6` / `p` / `li` / `a[href]` / `hr` / `br`,
/// and skips `<script>` / `<style>` content. Not a spec tokenizer yet — the
/// full HTML5 state machine arrives with the real DOM.
pub fn parse(html: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut buf = String::new();
    let mut kind = Kind::Para;
    let mut skip = false; // inside <script>/<style>
    let mut in_link = false;
    let mut link_href = String::new();

    let mut chars = html.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut raw = String::new();
            let mut closed = false;
            for nc in chars.by_ref() {
                if nc == '>' {
                    closed = true;
                    break;
                }
                raw.push(nc);
            }
            if !closed {
                break; // unterminated tag → stop
            }

            // HTML comment `<!-- ... -->`. `raw` is the text up to the first
            // '>'. A comment that contains inner markup (e.g. a commented-out
            // menu `<!-- <li><a>…</a> -->`) has its first '>' *inside* the
            // comment, so `raw` won't end with "--" — keep consuming until we
            // actually reach "-->". (A pure-text comment closes right here.)
            if raw.starts_with("!--") {
                if !raw.ends_with("--") {
                    let mut dashes = 0;
                    for nc in chars.by_ref() {
                        if nc == '-' {
                            dashes += 1;
                        } else if nc == '>' && dashes >= 2 {
                            break;
                        } else {
                            dashes = 0;
                        }
                    }
                }
                continue;
            }

            let closing = raw.starts_with('/');
            let name = tag_name(&raw);

            if name == "script" || name == "style" {
                skip = !closing;
                continue;
            }
            if skip {
                continue;
            }
            // Inside a link, ignore structural tags — keep the link text intact.
            if in_link && name != "a" {
                continue;
            }

            match name.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    flush(&mut blocks, &mut buf, kind);
                    kind = if closing {
                        Kind::Para
                    } else {
                        Kind::Heading(name.as_bytes()[1] - b'0')
                    };
                }
                "a" => {
                    if closing {
                        let t = clean(&buf);
                        buf.clear();
                        if !t.is_empty() {
                            blocks.push(Block::Link {
                                text: t,
                                href: core::mem::take(&mut link_href),
                            });
                        } else {
                            link_href.clear();
                        }
                        in_link = false;
                    } else {
                        flush(&mut blocks, &mut buf, kind);
                        link_href = attr(&raw, "href").unwrap_or_default();
                        in_link = true;
                    }
                }
                "li" => {
                    flush(&mut blocks, &mut buf, kind);
                    kind = if closing { Kind::Para } else { Kind::ListItem };
                }
                "hr" => {
                    flush(&mut blocks, &mut buf, kind);
                    blocks.push(Block::Rule);
                }
                "br" => flush(&mut blocks, &mut buf, kind),
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "ul"
                | "ol" | "blockquote" | "pre" | "tr" | "table" | "nav" | "aside" | "figure"
                | "figcaption" | "title" | "head" | "body" | "html" | "dl" | "dt" | "dd" => {
                    flush(&mut blocks, &mut buf, kind);
                    kind = Kind::Para;
                }
                _ => {} // inline / unknown tag → ignore, keep accumulating text
            }
        } else if !skip {
            buf.push(c);
        }
    }

    // Trailing text.
    if in_link {
        let t = clean(&buf);
        if !t.is_empty() {
            blocks.push(Block::Link { text: t, href: link_href });
        }
    } else {
        flush(&mut blocks, &mut buf, kind);
    }
    blocks
}

/// The document `<title>`, cleaned, if present.
pub fn title(html: &str) -> Option<String> {
    let low = html.to_ascii_lowercase();
    let s = low.find("<title")?;
    let gt = html[s..].find('>')? + s + 1;
    let e = low[gt..].find("</title>")? + gt;
    let t = clean(&html[gt..e]);
    if t.is_empty() { None } else { Some(t) }
}

fn flush(blocks: &mut Vec<Block>, buf: &mut String, kind: Kind) {
    let t = clean(buf);
    buf.clear();
    if t.is_empty() {
        return;
    }
    match kind {
        Kind::Para => blocks.push(Block::Para(t)),
        Kind::Heading(l) => blocks.push(Block::Heading { level: l, text: t }),
        Kind::ListItem => blocks.push(Block::ListItem(t)),
    }
}

/// Lowercased tag name (leading alphanumerics after an optional `/`).
fn tag_name(raw: &str) -> String {
    let raw = raw.trim_start_matches('/').trim_start();
    let mut name = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else {
            break;
        }
    }
    name
}

/// Value of a (quoted or bare) attribute inside a tag body. Case-insensitive
/// on the attribute name; `ascii_lowercase` preserves byte length so the
/// found index is valid in the original.
fn attr(raw: &str, key: &str) -> Option<String> {
    let low = raw.to_ascii_lowercase();
    let mut from = 0;
    loop {
        let pos = low[from..].find(key)? + from;
        let after = raw[pos + key.len()..].trim_start();
        if let Some(after) = after.strip_prefix('=') {
            let after = after.trim_start();
            let val = if let Some(rest) = after.strip_prefix('"') {
                rest.split('"').next().unwrap_or("")
            } else if let Some(rest) = after.strip_prefix('\'') {
                rest.split('\'').next().unwrap_or("")
            } else {
                after.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("")
            };
            return Some(val.to_string());
        }
        from = pos + key.len();
    }
}

/// Decode common HTML entities then collapse runs of whitespace to one space.
fn clean(s: &str) -> String {
    let decoded = decode_entities(s);
    let mut out = String::new();
    let mut prev_space = false;
    for ch in decoded.chars() {
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

fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let mut ent = String::new();
        let mut found = false;
        for _ in 0..12 {
            match chars.peek() {
                Some(';') => {
                    chars.next();
                    found = true;
                    break;
                }
                Some(&ch) if ch == '&' || ch.is_whitespace() => break,
                Some(&ch) => {
                    ent.push(ch);
                    chars.next();
                }
                None => break,
            }
        }
        if !found {
            out.push('&');
            out.push_str(&ent);
            continue;
        }
        match ent.as_str() {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            "nbsp" => out.push(' '),
            "mdash" => out.push('\u{2014}'),
            "ndash" => out.push('\u{2013}'),
            "hellip" => out.push('\u{2026}'),
            "copy" => out.push('\u{00A9}'),
            _ => {
                if let Some(rest) = ent.strip_prefix('#') {
                    let cp = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        rest.parse::<u32>().ok()
                    };
                    if let Some(ch) = cp.and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                // unknown named entity → dropped
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn strips_tags_and_builds_blocks() {
        let html = "<html><head><title>Hi &amp; Bye</title><style>x{color:red}</style>\
                    </head><body><h1>Head</h1><p>Hello   world &lt;3</p>\
                    <ul><li>one</li><li>two</li></ul>\
                    <a href=\"/next\">Next page</a>\
                    <script>evil()</script></body></html>";
        let blocks = parse(html);

        assert_eq!(title(html), Some("Hi & Bye".to_string()));
        assert!(blocks.contains(&Block::Heading { level: 1, text: "Head".to_string() }));
        assert!(blocks.contains(&Block::Para("Hello world <3".to_string())));
        assert!(blocks.contains(&Block::ListItem("one".to_string())));
        assert!(blocks.iter().any(|b| matches!(b, Block::Link { href, text }
            if href == "/next" && text == "Next page")));
        // script/style bodies never leak into content
        assert!(!blocks.iter().any(|b| matches!(b, Block::Para(t) if t.contains("evil"))));
        assert!(!blocks.iter().any(|b| matches!(b, Block::Para(t) if t.contains("color"))));
    }

    #[test]
    fn skips_comments_even_with_inner_markup() {
        let html = "<p>before</p><!-- <li><a>Über uns</a> Referenzen \
                    Lab -->\n<p>after</p><!-- plain comment -->";
        let blocks = parse(html);
        assert!(blocks.contains(&Block::Para("before".to_string())));
        assert!(blocks.contains(&Block::Para("after".to_string())));
        assert!(!blocks.iter().any(|b| matches!(b, Block::Para(t)
            if t.contains("Über uns") || t.contains("Referenzen") || t.contains("--"))));
        assert!(!blocks.iter().any(|b| matches!(b, Block::Link { text, .. }
            if text.contains("Über uns"))));
    }

    #[test]
    fn numeric_entities_and_whitespace() {
        let blocks = parse("<p>a&#38;b   c&#x2014;d</p>");
        assert_eq!(blocks, alloc::vec![Block::Para("a&b c\u{2014}d".to_string())]);
    }
}
