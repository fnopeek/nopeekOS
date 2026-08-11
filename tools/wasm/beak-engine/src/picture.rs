//! Responsive images: `<picture>`/`<source>` selection and `srcset` (HTML
//! §4.8.4.3).
//!
//! Resolved as a DOM pass, once, right after parsing: the winning candidate is
//! folded into the `<img>`'s own `src`/`width`/`height`. Everything downstream
//! — `image_srcs` (what the shell fetches), `img_box` (how big the box is),
//! the `DrawOp::Image` that carries the src — keeps reading a plain `<img
//! src>` and needs no changes. It also guarantees the shell fetches exactly
//! the URL layout will ask for, which two independent selection sites could
//! not.

use crate::css::Media;
use crate::dom::{Dom, Element, Node};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Fold every `<picture>`'s active `<source>` — and any bare `<img srcset>` —
/// into the `<img>` element itself.
pub fn resolve(dom: &mut Dom, media: Media) {
    walk(&mut dom.root, media);
}

fn walk(el: &mut Element, media: Media) {
    if el.tag == "picture" {
        apply_picture(el, media);
    }
    for c in &mut el.children {
        if let Node::Element(e) = c {
            walk(e, media);
        }
    }
}

/// A `<picture>`: the first `<source>` whose `media` matches and whose `type`
/// we can actually decode wins; the `<img>` is the fallback. Nothing matching
/// leaves the `<img>` exactly as authored.
fn apply_picture(pic: &mut Element, media: Media) {
    let mut chosen: Option<(String, Option<String>, Option<String>)> = None;
    for c in &pic.children {
        let Node::Element(e) = c else { continue };
        if e.tag != "source" {
            continue;
        }
        // A `type` we cannot decode has to be SKIPPED, not taken and then
        // failed: taking an `image/webp` source would replace a picture that
        // renders today with one that renders nothing.
        if let Some(t) = e.attr("type") {
            if !decodable_type(t) {
                continue;
            }
        }
        if let Some(q) = e.attr("media") {
            if !crate::css::media_matches(q, media) {
                continue;
            }
        }
        let Some(set) = e.attr("srcset") else { continue };
        let Some(url) = pick(set, e.attr("sizes"), media.width) else { continue };
        chosen = Some((
            url.to_string(),
            e.attr("width").map(ToString::to_string),
            e.attr("height").map(ToString::to_string),
        ));
        break;
    }
    let Some((url, w, h)) = chosen else {
        // No `<source>` won — the `<img>` may still carry its own `srcset`.
        for c in &mut pic.children {
            if let Node::Element(e) = c {
                if e.tag == "img" {
                    apply_img_srcset(e, media);
                }
            }
        }
        return;
    };
    for c in &mut pic.children {
        let Node::Element(e) = c else { continue };
        if e.tag != "img" {
            continue;
        }
        set_attr(e, "src", &url);
        // A `<source>`'s own `width`/`height` are the image's dimensions when
        // that source is used — that is the whole point of the wide-viewport
        // variant (Wikipedia's footer swaps a 25×25 icon for an 84×29 button).
        if let Some(w) = &w {
            set_attr(e, "width", w);
        }
        if let Some(h) = &h {
            set_attr(e, "height", h);
        }
    }
}

/// `srcset` on a bare `<img>`: only used when there is no `src` to fall back
/// on, or when the chosen candidate is a different URL at 1x. Density
/// candidates above 1x are deliberately NOT taken — we render at 1x, and
/// fetching the 2x asset would double the bytes for no visible gain.
fn apply_img_srcset(img: &mut Element, media: Media) {
    let Some(set) = img.attr("srcset").map(ToString::to_string) else { return };
    let sizes = img.attr("sizes").map(ToString::to_string);
    if img.attr("src").is_some_and(|s| !s.trim().is_empty()) {
        return;
    }
    if let Some(url) = pick(&set, sizes.as_deref(), media.width) {
        let url = url.to_string();
        set_attr(img, "src", &url);
    }
}

fn set_attr(el: &mut Element, name: &str, value: &str) {
    match el.attrs.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) => *v = value.to_string(),
        None => el.attrs.push((name.to_string(), value.to_string())),
    }
}

/// The formats `image::decode` handles. Anything else must not be selected.
fn decodable_type(t: &str) -> bool {
    let t = t.trim().to_ascii_lowercase();
    matches!(t.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/svg+xml")
}

/// Split a `srcset` into `(url, descriptor)` candidates — HTML "parse a srcset
/// attribute".
///
/// The separator is WHITESPACE, not the comma: a URL is a run of non-whitespace
/// characters, and only a comma that ends that run (or follows the descriptor)
/// starts the next candidate. That is what makes a `data:` URI work, since its
/// commas sit inside an unbroken run — splitting on ',' cuts it in half and
/// hands the tail on as a URL of its own.
fn srcset_candidates(srcset: &str) -> Vec<(&str, Option<&str>)> {
    let mut out = Vec::new();
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',');
        if rest.is_empty() {
            return out;
        }
        // The URL runs to the next whitespace.
        let end = rest
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let (url, tail) = rest.split_at(end);
        // A URL ending in commas takes no descriptor (spec step 5).
        let trimmed = url.trim_end_matches(',');
        if trimmed.len() != url.len() {
            out.push((trimmed, None));
            rest = tail;
            continue;
        }
        // Otherwise the descriptors run to the next comma.
        let dend = tail.find(',').unwrap_or(tail.len());
        let (desc, after) = tail.split_at(dend);
        let desc = desc.trim();
        out.push((url, if desc.is_empty() { None } else { Some(desc) }));
        rest = after;
    }
}

/// Pick one candidate out of a `srcset`. Width (`Nw`) candidates resolve
/// against `sizes` — defaulting to the viewport, as the spec does — and the
/// narrowest one that still covers it wins. Density (`Nx`) candidates resolve
/// at 1x.
fn pick<'a>(srcset: &'a str, sizes: Option<&str>, viewport: f32) -> Option<&'a str> {
    let mut widths: Vec<(f32, &str)> = Vec::new();
    let mut densities: Vec<(f32, &str)> = Vec::new();
    for (url, desc) in srcset_candidates(srcset) {
        match desc {
            Some(d) if d.ends_with('w') => {
                if let Ok(v) = d[..d.len() - 1].parse::<f32>() {
                    widths.push((v, url));
                }
            }
            Some(d) if d.ends_with('x') => {
                if let Ok(v) = d[..d.len() - 1].parse::<f32>() {
                    densities.push((v, url));
                }
            }
            _ => densities.push((1.0, url)),
        }
    }
    if !widths.is_empty() {
        let target = sizes.and_then(|s| sizes_px(s, viewport)).unwrap_or(viewport);
        let mut best: Option<(f32, &str)> = None;
        for (w, u) in &widths {
            if *w >= target && best.is_none_or(|(bw, _)| *w < bw) {
                best = Some((*w, u));
            }
        }
        // Nothing covers the target → the largest available is the closest.
        return Some(best.map(|(_, u)| u).unwrap_or_else(|| {
            widths.iter().fold(widths[0], |a, b| if b.0 > a.0 { *b } else { a }).1
        }));
    }
    // Exactly 1x if it exists, else the smallest above it, else the largest.
    let mut best: Option<(f32, &str)> = None;
    for (d, u) in &densities {
        let better = match best {
            None => true,
            Some((bd, _)) if bd < 1.0 => *d > bd,
            Some((bd, _)) => *d >= 1.0 && *d < bd,
        };
        if better {
            best = Some((*d, u));
        }
    }
    best.map(|(_, u)| u)
}

/// The first length in a `sizes` list, which is the value that applies when no
/// media condition precedes it. `vw` resolves against the viewport.
fn sizes_px(sizes: &str, viewport: f32) -> Option<f32> {
    for part in sizes.split(',') {
        let last = part.trim().rsplit([')', ' ']).next()?.trim();
        if let Some(v) = last.strip_suffix("vw").and_then(|n| n.parse::<f32>().ok()) {
            return Some(v / 100.0 * viewport);
        }
        if let Some(v) = last.strip_suffix("px").and_then(|n| n.parse::<f32>().ok()) {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(w: f32) -> Media {
        Media::new(w, false)
    }

    #[test]
    fn a_source_replaces_the_img_src_and_its_dimensions() {
        // Wikipedia's footer: a 25×25 icon below 500px, an 84×29 button above.
        let html = "<body><picture>\
            <source media=\"(min-width: 500px)\" srcset=\"/wide.svg\" width=\"84\" height=\"29\">\
            <img src=\"/small.svg\" width=\"25\" height=\"25\"></picture></body>";
        let mut dom = crate::dom::parse(html);
        resolve(&mut dom, media(800.0));
        let img = find_img(&dom.root).expect("img");
        assert_eq!(img.attr("src"), Some("/wide.svg"));
        assert_eq!(img.attr("width"), Some("84"));
        assert_eq!(img.attr("height"), Some("29"));

        // Below the breakpoint the fallback stands untouched.
        let mut dom = crate::dom::parse(html);
        resolve(&mut dom, media(400.0));
        let img = find_img(&dom.root).unwrap();
        assert_eq!(img.attr("src"), Some("/small.svg"));
        assert_eq!(img.attr("width"), Some("25"));
    }

    #[test]
    fn an_undecodable_type_is_skipped_not_taken() {
        // Taking the webp would replace a picture that renders with one that
        // renders nothing at all.
        let html = "<body><picture>\
            <source type=\"image/webp\" srcset=\"/x.webp\">\
            <source type=\"image/png\" srcset=\"/x.png\">\
            <img src=\"/x.gif\"></picture></body>";
        let mut dom = crate::dom::parse(html);
        resolve(&mut dom, media(800.0));
        assert_eq!(find_img(&dom.root).unwrap().attr("src"), Some("/x.png"));
    }

    #[test]
    fn density_candidates_resolve_at_1x() {
        assert_eq!(pick("/a.png 1x, /b.png 2x", None, 800.0), Some("/a.png"));
        assert_eq!(pick("/a.png, /b.png 2x", None, 800.0), Some("/a.png"));
        // No 1x at all → the smallest above it.
        assert_eq!(pick("/b.png 2x, /c.png 3x", None, 800.0), Some("/b.png"));
    }

    #[test]
    fn a_data_uri_survives_the_srcset_split() {
        // DuckDuckGo's home page ships its logo as
        // `<picture><source srcSet="data:image/svg+xml;base64,…">`. The commas
        // inside a data: URI are not candidate separators — splitting on them
        // handed the base64 tail on as a URL of its own, which no fetch can
        // ever satisfy.
        let uri = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iYSIvPg==";
        assert_eq!(pick(uri, None, 800.0), Some(uri));
        assert_eq!(pick(&alloc::format!("{uri} 2x"), None, 800.0), Some(uri));
        // Still one candidate among several.
        let set = alloc::format!("/a.png 1x, {uri} 2x");
        assert_eq!(pick(&set, None, 800.0), Some("/a.png"));
        let set = alloc::format!("{uri} 1x, /b.png 2x");
        assert_eq!(pick(&set, None, 800.0), Some(uri));
    }

    #[test]
    fn a_comma_only_separates_when_it_ends_the_url_run() {
        // Trailing commas end the candidate and leave it descriptor-less …
        assert_eq!(pick("/a.png,, /b.png 2x", None, 800.0), Some("/a.png"));
        // … while a comma INSIDE the run is just part of the URL, which is the
        // whole reason a data: URI survives.
        assert_eq!(pick("/a,b.png 2x", None, 800.0), Some("/a,b.png"));
    }

    #[test]
    fn width_candidates_resolve_against_sizes() {
        let set = "/s.jpg 320w, /m.jpg 640w, /l.jpg 1280w";
        // Narrowest that still covers the target.
        assert_eq!(pick(set, Some("600px"), 1600.0), Some("/m.jpg"));
        assert_eq!(pick(set, Some("100vw"), 300.0), Some("/s.jpg"));
        // Nothing covers it → the largest.
        assert_eq!(pick(set, Some("2000px"), 1600.0), Some("/l.jpg"));
        // No `sizes` → the viewport is the target, as the spec defaults.
        assert_eq!(pick(set, None, 640.0), Some("/m.jpg"));
    }

    fn find_img(el: &Element) -> Option<&Element> {
        if el.tag == "img" {
            return Some(el);
        }
        el.children.iter().find_map(|c| match c {
            Node::Element(e) => find_img(e),
            _ => None,
        })
    }
}
