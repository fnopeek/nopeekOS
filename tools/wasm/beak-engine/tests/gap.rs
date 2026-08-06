//! CSS gap analysis: for a real page + its real stylesheets, count how many
//! DOM elements each declared property actually WINS on, then split that by
//! whether the engine implements the property.
//!
//! GAPHTML=wiki.html GAPCSS=wiki.css cargo test --release --test gap -- --nocapture
use std::collections::HashMap;
use std::fs;

use beak_engine::css::{self, ElemInfo, Stylesheet};
use beak_engine::dom::{self, Element, Node};

/// Every property `style::apply_one` actually handles, read out of the source
/// at run time. This list used to be maintained by hand and went stale twice —
/// once claiming `background-image` was missing months after it shipped, which
/// put a phantom item at the top of the priority list. Deriving it costs one
/// file read and cannot drift.
fn implemented() -> std::collections::HashSet<String> {
    let src = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/style.rs"),
    )
    .expect("src/style.rs");
    let body = src
        .split_once("fn apply_one")
        .expect("apply_one")
        .1;
    let mut out = std::collections::HashSet::new();
    for line in body.lines() {
        // A match arm head sits at exactly 8 spaces and starts with a string
        // pattern. `"a" | "b" => …` puts several on one line; a pattern broken
        // over lines continues with `| "c"`.
        let ind = line.len() - line.trim_start().len();
        let t = line.trim_start();
        if ind != 8 || !(t.starts_with('"') || t.starts_with("| \"")) {
            continue;
        }
        // Only what precedes `=>` is the pattern; the arm body may hold strings
        // of its own (values, keywords) that are not property names.
        let pat = t.split("=>").next().unwrap_or("");
        let mut rest = pat;
        while let Some(a) = rest.find('"') {
            let Some(b) = rest[a + 1..].find('"') else { break };
            out.insert(rest[a + 1..a + 1 + b].to_string());
            rest = &rest[a + b + 2..];
        }
    }
    assert!(out.len() > 100, "apply_one scrape found only {} arms — did the shape change?", out.len());
    out
}


struct Ctx<'a> {
    ss: &'a Stylesheet,
    w: f32,
    /// property -> (elements it wins on, distinct winning values)
    tally: HashMap<String, (u32, HashMap<String, u32>)>,
    elems: u32,
    /// tag -> count, for sanity
    tags: HashMap<String, u32>,
}

fn walk<'a>(ctx: &mut Ctx, el: &'a Element, ancestors: &mut Vec<ElemInfo<'a>>) {
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

    let mut m = ctx.ss.matched(&ei, ancestors, &[], sib_count, css::Media::new(ctx.w, false));
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

fn walk_with_prev<'a>(ctx: &mut Ctx, el: &'a Element, ancestors: &mut Vec<ElemInfo<'a>>, prev: &[ElemInfo<'a>]) {
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

    let mut m = ctx.ss.matched(&ei, ancestors, prev, prev.len() as u32 + 1, css::Media::new(ctx.w, false));
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
    let ss = css::collect_all(&d, &extern_css, css::Media::new(w, false));

    let mut ctx = Ctx {
        ss: &ss,
        w,
        tally: HashMap::new(),
        elems: 0,
        tags: HashMap::new(),
    };
    let mut anc = Vec::new();
    walk(&mut ctx, &d.root, &mut anc);

    let impl_set = implemented();
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
