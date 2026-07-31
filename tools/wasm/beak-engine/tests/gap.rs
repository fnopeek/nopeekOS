//! CSS gap analysis: for a real page + its real stylesheets, count how many
//! DOM elements each declared property actually WINS on, then split that by
//! whether the engine implements the property.
//!
//! GAPHTML=wiki.html GAPCSS=wiki.css cargo test --release --test gap -- --nocapture
use std::collections::HashMap;
use std::fs;

use beak_engine::css::{self, ElemInfo, Stylesheet};
use beak_engine::dom::{self, Element, Node};

/// Properties `style::apply_one` actually handles (kept in sync by hand from
/// the top-level `match prop` arms).
const IMPLEMENTED: &[&str] = &[
    "align-items","align-self","background","background-color","border","border-bottom",
    "border-bottom-color","border-bottom-style","border-bottom-width","border-color",
    "border-left","border-left-color","border-left-style","border-left-width","border-right",
    "border-right-color","border-right-style","border-right-width","border-style","border-top",
    "border-top-color","border-top-style","border-top-width","border-width","bottom",
    "box-sizing","clear","clip","color","column-gap","contain","contain-intrinsic-size",
    "display","flex","flex-basis","flex-direction","flex-flow","flex-grow","flex-shrink",
    "flex-wrap","float","font-family","font-size","font-style","font-weight","gap","grid",
    "grid-area","grid-auto-rows","grid-column","grid-column-start","grid-gap","grid-row",
    "grid-row-start","grid-template","grid-template-areas","grid-template-columns",
    "grid-template-rows","height","justify-content","justify-items","justify-self","left",
    "margin","margin-bottom","margin-left","margin-right","margin-top","max-height",
    "max-width","min-height","min-width","order","padding","padding-bottom","padding-left",
    "padding-right","padding-top","place-items","place-self","position","right","row-gap",
    "table-layout","top","white-space","width","z-index",
    "text-align","text-align-last","text-transform","list-style","list-style-type",
    "line-height","direction","font",
    "margin-inline","margin-inline-start","margin-inline-end","margin-block",
    "margin-block-start","margin-block-end","padding-inline","padding-inline-start",
    "padding-inline-end","padding-block","padding-block-start","padding-block-end",
    "border-collapse","border-spacing","empty-cells","counter-reset","counter-increment",
    "opacity","visibility","content",
    "text-decoration","text-decoration-line","caption-side","vertical-align",
    "border-radius","border-top-left-radius","border-top-right-radius",
    "border-bottom-right-radius","border-bottom-left-radius",
    "overflow","overflow-wrap","word-wrap","word-break",
];

struct Ctx<'a> {
    ss: &'a Stylesheet,
    w: f32,
    /// property -> (elements it wins on, distinct winning values)
    tally: HashMap<String, (u32, HashMap<String, u32>)>,
    elems: u32,
    /// tag -> count, for sanity
    tags: HashMap<String, u32>,
}

fn walk(ctx: &mut Ctx, el: &Element, ancestors: &mut Vec<ElemInfo>) {
    let ei = ElemInfo::of(el);
    ctx.elems += 1;
    *ctx.tags.entry(el.tag.clone()).or_insert(0) += 1;

    let kids: Vec<&Element> = el
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    let sib_count = kids.len() as u32;

    let mut m = ctx.ss.matched(&ei, ancestors, &[], sib_count, ctx.w);
    m.sort_by_key(|(spec, ord, _)| (*spec, *ord));
    // last writer per property wins (ignoring !important — close enough for a
    // frequency census)
    let mut winner: HashMap<&str, &str> = HashMap::new();
    for (_, _, decls) in &m {
        for (p, v) in decls.iter() {
            winner.insert(p.as_str(), v.as_str());
        }
    }
    // inline style attribute beats every stylesheet rule
    if let Some(style) = el.attr("style") {
        for d in style.split(';') {
            if let Some((p, v)) = d.split_once(':') {
                winner.insert(p.trim(), v.trim());
            }
        }
    }
    for (p, v) in winner {
        if p.starts_with("--") {
            continue;
        }
        let e = ctx
            .tally
            .entry(p.to_string())
            .or_insert((0, HashMap::new()));
        e.0 += 1;
        *e.1.entry(v.to_string()).or_insert(0) += 1;
    }

    ancestors.push(ei);
    let mut prev: Vec<ElemInfo> = Vec::new();
    for k in kids {
        // prev_siblings is passed as a slice of preceding siblings
        let _ = &prev;
        walk_with_prev(ctx, k, ancestors, &prev);
        prev.push(ElemInfo::of(k));
    }
    ancestors.pop();
}

fn walk_with_prev(ctx: &mut Ctx, el: &Element, ancestors: &mut Vec<ElemInfo>, prev: &[ElemInfo]) {
    let ei = ElemInfo::of(el);
    ctx.elems += 1;
    *ctx.tags.entry(el.tag.clone()).or_insert(0) += 1;

    let kids: Vec<&Element> = el
        .children
        .iter()
        .filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
        .collect();

    let mut m = ctx.ss.matched(&ei, ancestors, prev, prev.len() as u32 + 1, ctx.w);
    m.sort_by_key(|(spec, ord, _)| (*spec, *ord));
    let mut winner: HashMap<&str, &str> = HashMap::new();
    for (_, _, decls) in &m {
        for (p, v) in decls.iter() {
            winner.insert(p.as_str(), v.as_str());
        }
    }
    if let Some(style) = el.attr("style") {
        for d in style.split(';') {
            if let Some((p, v)) = d.split_once(':') {
                winner.insert(p.trim(), v.trim());
            }
        }
    }
    for (p, v) in winner {
        if p.starts_with("--") {
            continue;
        }
        let e = ctx.tally.entry(p.to_string()).or_insert((0, HashMap::new()));
        e.0 += 1;
        *e.1.entry(v.to_string()).or_insert(0) += 1;
    }

    ancestors.push(ei);
    let mut p2: Vec<ElemInfo> = Vec::new();
    for k in kids {
        walk_with_prev(ctx, k, ancestors, &p2);
        p2.push(ElemInfo::of(k));
    }
    ancestors.pop();
}

#[test]
fn gap() {
    let hp = match std::env::var("GAPHTML") {
        Ok(p) => p,
        Err(_) => return,
    };
    let cp = std::env::var("GAPCSS").unwrap_or_default();
    let w: f32 = std::env::var("GAPW")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1400.0);

    let html = fs::read_to_string(&hp).expect("html");
    let extern_css = fs::read_to_string(&cp).unwrap_or_default();
    let d = dom::parse(&html);
    let ss = css::collect_all(&d, &extern_css, w);

    let mut ctx = Ctx {
        ss: &ss,
        w,
        tally: HashMap::new(),
        elems: 0,
        tags: HashMap::new(),
    };
    let mut anc = Vec::new();
    walk(&mut ctx, &d.root, &mut anc);

    let impl_set: std::collections::HashSet<&str> = IMPLEMENTED.iter().copied().collect();
    let mut rows: Vec<(u32, String, bool, Vec<(String, u32)>)> = ctx
        .tally
        .into_iter()
        .map(|(p, (n, vals))| {
            let mut v: Vec<(String, u32)> = vals.into_iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            v.truncate(4);
            let ok = impl_set.contains(p.as_str());
            (n, p, ok, v)
        })
        .collect();
    rows.sort_by_key(|(n, p, _, _)| (std::cmp::Reverse(*n), p.clone()));

    eprintln!("elements walked: {}", ctx.elems);
    eprintln!("distinct properties applied: {}", rows.len());
    let missing: u32 = rows.iter().filter(|r| !r.2).map(|r| r.0).sum();
    let have: u32 = rows.iter().filter(|r| r.2).map(|r| r.0).sum();
    eprintln!("property applications: {have} implemented / {missing} unimplemented");

    eprintln!("\n===== UNIMPLEMENTED, by elements affected =====");
    for (n, p, ok, vals) in &rows {
        if *ok {
            continue;
        }
        let vs: Vec<String> = vals
            .iter()
            .map(|(v, c)| format!("{}×{}", c, if v.len() > 28 { &v[..28] } else { v }))
            .collect();
        eprintln!("{n:6}  {p:<34} {}", vs.join("  "));
    }
    eprintln!("\n===== IMPLEMENTED, by elements affected =====");
    for (n, p, ok, _) in &rows {
        if !*ok {
            continue;
        }
        eprintln!("{n:6}  {p}");
    }

    let mut tags: Vec<(String, u32)> = ctx.tags.into_iter().collect();
    tags.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    eprintln!("\n===== TAGS =====");
    for (t, c) in tags.iter().take(30) {
        eprint!("{t}×{c}  ");
    }
    eprintln!();
}
