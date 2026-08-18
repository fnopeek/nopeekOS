//! Throwaway diagnostic — dump the display list for a WPT reftest + its ref.
//! DIAG=CSS2/height-012.xht cargo test --release --test diag -- --nocapture
use std::fs;
use std::path::Path;
use beak_engine::layout::DrawOp;
use beak_engine::{Engine, Rgb, Theme};

fn light() -> Theme {
    Theme { bg: Rgb(255,255,255), text: Rgb(0,0,0), heading: Rgb(0,0,0),
            link: Rgb(0,0,238), muted: Rgb(96,96,96), rule: Rgb(128,128,128) }
}

fn dump(label: &str, html: &str) {
    let mut eng = Engine::new();
    eng.set_theme(light());
    let lay = eng.layout_ext(html, "", 800);
    eprintln!("--- {label}: {} ops, height={} ---", lay.ops.len(), lay.height);
    for op in &lay.ops {
        match op {
            DrawOp::RoundRect { x, y, w, h, color, .. } | DrawOp::Rect { x, y, w, h, color } =>
                eprintln!("  RECT x={x} y={y} w={w} h={h} color={:?}", color),
            DrawOp::Text { x, y, size, color, text, .. } =>
                eprintln!("  TEXT x={x} y={y} size={size:.0} color={:?} {:?}", color,
                          if text.len()>40 {&text[..40]} else {text}),
            DrawOp::Image { x, y, w, h, .. } =>
                eprintln!("  IMG  x={x} y={y} w={w} h={h}"),
            DrawOp::BgImage { x, y, w, h, key, tint, .. } =>
                eprintln!("  BGIMG x={x} y={y} w={w} h={h} key={key:016x} {}",
                          if tint.is_some() {"MASK"} else {"bg"}),
        }
    }
}

#[test]
fn diag() {
    // DCSSIMG=<html> DCSS=<css> [DW=w] — for every element that WINS a
    // background-image or mask-image, report what would stop us painting it:
    // the display type (an inline box has no box decoration) and whether there
    // is a background-colour for a mask to stencil.
    if let Ok(hp) = std::env::var("DCSSIMG") {
        use beak_engine::css::{self, ElemInfo};
        use beak_engine::dom::{self, Element, Node};
        use beak_engine::style::{self, ComputedStyle};
        let csspath = std::env::var("DCSS").unwrap_or_default();
        let css = fs::read_to_string(&csspath).unwrap_or_default();
        let w: f32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1900.0);
        let html = fs::read_to_string(&hp).expect("html");
        let dom = dom::parse(&html);
        let ss = css::collect_all(&dom, &css, css::Media::new(w, false));
        let theme = light();
        let mut tally: std::collections::BTreeMap<String, u32> = Default::default();
        #[allow(clippy::too_many_arguments)]
        fn walk<'a>(el: &'a Element, parent: &ComputedStyle, ss: &css::Stylesheet, theme: &Theme,
                w: f32, anc: &mut Vec<ElemInfo<'a>>, tally: &mut std::collections::BTreeMap<String, u32>,
                dead: bool) {
            let kids: Vec<&Element> = el.children.iter()
                .filter_map(|n| match n { Node::Element(e) => Some(e), _ => None }).collect();
            let n = kids.len() as u32;
            let mut prev: Vec<ElemInfo> = Vec::new();
            for k in &kids {
                let st = style::resolve(&beak_engine::css::ElemInfo::of(k), parent, theme, ss, anc, &prev, n, w);
                let which = if st.mask_layer.image.is_some() { Some("mask") }
                            else if st.bg_layer.image.is_some() { Some("bg") } else { None };
                if let Some(kind) = which {
                    let blocked = if dead { "ancestor display:none" }
                        else if st.display == style::Display::None { "display:none" }
                        else if kind == "mask" && st.bg.is_none() { "mask without background-color" }
                        else { "paints" };
                    let blocked = &format!("{blocked} [{:?}]", st.display);
                    let cls = k.attr("class").unwrap_or("");
                    *tally.entry(format!("{kind:4} {blocked:28} {} .{}", k.tag,
                        cls.split_whitespace().take(2).collect::<Vec<_>>().join("."))).or_insert(0) += 1;
                }
                anc.push(ElemInfo::of(k));
                walk(k, &st, ss, theme, w, anc, tally, dead || st.display == style::Display::None);
                anc.pop();
                prev.push(ElemInfo::of(k));
            }
        }
        let root = ComputedStyle::root(&theme);
        walk(&dom.root, &root, &ss, &theme, w, &mut Vec::new(), &mut tally, false);
        let total: u32 = tally.values().sum();
        println!("elements winning a CSS image: {total}");
        let mut v: Vec<_> = tally.into_iter().collect();
        v.sort_by_key(|(_, c)| -(*c as i64));
        for (k, c) in v { println!("  {c:>3}x  {k}"); }
        return;
    }
    // DJPEG=<file> — decode one JPEG and print the decoder's own error.
    if let Ok(fp) = std::env::var("DJPEG") {
        use zune_core::bytestream::ZCursor;
        use zune_core::colorspace::ColorSpace;
        use zune_core::options::DecoderOptions;
        use zune_jpeg::JpegDecoder;
        let bytes = fs::read(&fp).expect("file");
        let opts = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::RGB)
            .set_max_width(8192)
            .set_max_height(8192);
        let mut dec = JpegDecoder::new_with_options(ZCursor::new(&bytes[..]), opts);
        match dec.decode_headers() {
            Err(e) => println!("headers: ERR {e:?}"),
            Ok(()) => println!("headers: ok, dims={:?}", dec.dimensions()),
        }
        match dec.decode() {
            Err(e) => println!("decode:  ERR {e:?}"),
            Ok(v) => println!("decode:  ok, {} bytes", v.len()),
        }
        return;
    }
    // DPOS=<css> [DW=w] — dump the position/height/display/visibility cascade
    // for a `.vector-dropdown-content` div nested under a `.vector-dropdown`,
    // with a realistic ancestor chain (html.client-js …).
    if let Ok(cp) = std::env::var("DPOS") {
        use beak_engine::css::{self, ElemInfo};
        let css = fs::read_to_string(&cp).expect("css");
        let w: f32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400.0);
        let ss = css::parse(&css);
        // ElemInfo borrows a live element now (so a selector can see children
        // and element state), so the chain is built by parsing a snippet with
        // the shape we want rather than by hand-filling a struct.
        let dom = beak_engine::dom::parse(
            "<html class='client-js vector-feature-language-in-header-enabled'>\
             <body class='skin-vector-2022'>\
             <div id='vector-header-start' class='vector-header-start'>\
             <nav class='vector-main-menu-landmark'>\
             <div id='vector-main-menu-dropdown' class='vector-dropdown vector-main-menu-dropdown'>\
             <input id='vector-main-menu-dropdown-checkbox' class='vector-dropdown-checkbox'>\
             <label id='vector-main-menu-dropdown-label' class='vector-dropdown-label'></label>\
             <div class='vector-dropdown-content'></div>\
             </div></nav></div></body></html>",
        );
        // root → … → parent, then the subject and its preceding siblings.
        let mut chain: Vec<&beak_engine::dom::Element> = Vec::new();
        let mut cur = &dom.root;
        loop {
            let next = cur.children.iter().find_map(|n| match n {
                beak_engine::dom::Node::Element(e) => Some(e),
                _ => None,
            });
            match next {
                Some(e) => { chain.push(e); cur = e; }
                None => break,
            }
        }
        let dropdown = chain.iter().find(|e| e.attr("id") == Some("vector-main-menu-dropdown")).expect("dropdown");
        let kids: Vec<&beak_engine::dom::Element> = dropdown.children.iter().filter_map(|n| match n {
            beak_engine::dom::Node::Element(e) => Some(e),
            _ => None,
        }).collect();
        let ancestors: Vec<ElemInfo> = chain.iter().take_while(|e| e.attr("id") != Some("vector-main-menu-dropdown"))
            .chain(core::iter::once(dropdown)).map(|e| ElemInfo::of(e)).collect();
        let el = ElemInfo::of(kids.iter().find(|e| e.tag == "div").expect("content div"));
        let prev: Vec<ElemInfo> = kids.iter().take(2).map(|e| ElemInfo::of(e)).collect();
        let m = ss.matched(&el, &ancestors, &prev, 3, beak_engine::css::Media::new(w, false));
        for prop in ["position", "height", "display", "visibility", "opacity", "overflow"] {
            let mut all: Vec<(u32,u32,String)> = Vec::new();
            for (spec, order, decls, _imp) in &m {
                for (p,v) in decls.iter() { if beak_engine::css::prop_name(*p) == prop { all.push((*spec,*order,v.clone())); } }
            }
            all.sort_by_key(|(s,o,_)| (*s,*o));
            let winner = all.last().map(|(_,_,v)| v.clone()).unwrap_or_else(|| "<none>".into());
            eprintln!("{prop:>12} = {winner}   ({} rule(s))", all.len());
            for (s,o,v) in &all { eprintln!("               spec={s} order={o}: {v}"); }
        }
        return;
    }
    // DOPS=<html> DCSS=<css> DW=<w> — the WHOLE display list, in paint order.
    // Use when a widget is visibly wrong and you need to see which rect is the
    // stray one, not just where the text landed.
    if let Ok(hp) = std::env::var("DOPS") {
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400);
        let html = fs::read_to_string(&hp).expect("html");
        let mut eng = Engine::new();
        eng.set_theme(light());
        let lay = eng.layout_ext(&html, &css, w);
        eprintln!("--- {} ops, page height {} ---", lay.ops.len(), lay.height);
        for (i, op) in lay.ops.iter().enumerate() {
            match op {
                DrawOp::Rect { x, y, w, h, color } =>
                    eprintln!("{i:>3} RECT   x={x:>5} y={y:>5} w={w:>5} h={h:>4}  {color:?}"),
                DrawOp::RoundRect { x, y, w, h, color, .. } =>
                    eprintln!("{i:>3} RRECT  x={x:>5} y={y:>5} w={w:>5} h={h:>4}  {color:?}"),
                DrawOp::Text { x, y, size, text, .. } =>
                    eprintln!("{i:>3} TEXT   x={x:>5} y={y:>5} size={size:.0} {:?}",
                              if text.len()>40 {&text[..40]} else {text}),
                DrawOp::Image { x, y, w, h, .. } =>
                    eprintln!("{i:>3} IMG    x={x:>5} y={y:>5} w={w:>5} h={h:>4}"),
                DrawOp::BgImage { x, y, w, h, key, tint, .. } =>
                    eprintln!("{i:>3} BGIMG  x={x:>5} y={y:>5} w={w:>5} h={h:>4} key={key:016x} {}",
                              if tint.is_some() {"MASK"} else {"bg"}),
            }
        }
        return;
    }
    // DDUMP=<html> DCSS=<css> DW=<w> — dump every TEXT op with its y, plus the
    // page height. Tells you where a marker text lands (push-down debugging).
    if let Ok(hp) = std::env::var("DDUMP") {
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400);
        let html = fs::read_to_string(&hp).expect("html");
        let mut eng = Engine::new();
        eng.set_theme(light());
        let lay = eng.layout_ext(&html, &css, w);
        eprintln!("page height = {}", lay.height);
        for op in &lay.ops {
            if let DrawOp::Text { x, y, text, .. } = op {
                if !text.trim().is_empty() {
                    eprintln!("  TEXT y={y:>6} x={x:>5} {:?}", if text.len()>50 {&text[..50]} else {text});
                }
            }
        }
        return;
    }
    // DWIDTHS=<html> DCSS=<css> DW=<w> — trace box widths / overflow.
    if let Ok(hp) = std::env::var("DWIDTHS") {
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400);
        let html = fs::read_to_string(&hp).expect("html");
        let mut eng = Engine::new();
        eng.set_theme(light());
        let lay = eng.layout_ext(&html, &css, w);
        let mut max_right = 0i32;
        let mut rects: Vec<(i32, i32, i32)> = vec![];
        for op in &lay.ops {
            match op {
                DrawOp::RoundRect { x, y, w, .. } | DrawOp::Rect { x, y, w, .. } => { max_right = max_right.max(x + w); rects.push((*x, *w, *y)); }
                DrawOp::Text { x, .. } => { max_right = max_right.max(*x); }
                DrawOp::Image { x, y: _, w, .. } | DrawOp::BgImage { x, y: _, w, .. } => { max_right = max_right.max(x + w); }
            }
        }
        eprintln!("viewport={w}  max_right_edge={max_right}  OVERFLOW={}", max_right - w as i32);
        rects.sort_by_key(|(_, wd, _)| -wd);
        eprintln!("widest {} rects (x .. right, w, y):", rects.len().min(18));
        for (x, wd, y) in rects.iter().take(18) {
            eprintln!("  x={x:>5} right={:>5} w={wd:>5} y={y}", x + wd);
        }
        return;
    }
    // Parse a CSS file and report whether a given class's declarations survive.
    // DGRID=<css> [DCLASS=mw-page-container-inner] [DW=1200] cargo test --test diag
    if let Ok(cp) = std::env::var("DVARS") {
        let css = fs::read_to_string(&cp).expect("css");
        let out = beak_engine::vars::resolve_vars(&css, beak_engine::css::Media::new(std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1200.0), false), &[]);
        fs::write("resolved.css", &out).expect("write");
        eprintln!("resolved {} -> {} bytes (resolved.css)", css.len(), out.len());
        // report the biggest bare numbers in the output
        let mut nums: Vec<f64> = Vec::new();
        let b = out.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i].is_ascii_digit() {
                let s = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') { i += 1; }
                if let Ok(n) = out[s..i].parse::<f64>() { if n > 5000.0 { nums.push(n); } }
            } else { i += 1; }
        }
        nums.sort_by(|a, c| c.partial_cmp(a).unwrap());
        nums.dedup();
        eprintln!("big numbers (>5000) in resolved css: {:?}", &nums[..nums.len().min(15)]);
        return;
    }
    if let Ok(cp) = std::env::var("DGRID") {
        use beak_engine::css::{self, ElemInfo};
        let css = fs::read_to_string(&cp).expect("css");
        let class = std::env::var("DCLASS").unwrap_or_else(|_| "mw-page-container-inner".into());
        let w: f32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1200.0);
        let ss = css::parse(&css);
        let tag = std::env::var("DTAG").unwrap_or_else(|_| "div".into());
        let prop = std::env::var("DPROP").ok();
        // ElemInfo borrows a live element — build one instead of filling a
        // struct by hand.
        let cls = class.split(',').map(|s| s.trim()).collect::<Vec<_>>().join(" ");
        let dom = beak_engine::dom::parse(&format!("<{tag} class=\"{cls}\"></{tag}>"));
        fn first<'x>(el: &'x beak_engine::dom::Element, tag: &str) -> Option<&'x beak_engine::dom::Element> {
            if el.tag == tag { return Some(el); }
            el.children.iter().find_map(|n| match n {
                beak_engine::dom::Node::Element(e) => first(e, tag),
                _ => None,
            })
        }
        let el = ElemInfo::of(first(&dom.root, &tag).expect("element"));
        let m = ss.matched(&el, &[], &[], 1, beak_engine::css::Media::new(w, false));
        if let Some(pr) = &prop {
            let mut all: Vec<(u32,u32,String)> = Vec::new();
            for (spec, order, decls, _imp) in &m {
                for (p,v) in decls.iter() { if beak_engine::css::prop_name(*p) == pr.as_str() { all.push((*spec,*order,v.clone())); } }
            }
            all.sort_by_key(|(s,o,_)| (*s,*o));
            eprintln!("=== <{tag}>.{class} '{pr}' cascade (winner last) ===");
            for (s,o,v) in &all { eprintln!("  spec={s} order={o}: {pr}: {v}"); }
            return;
        }
        eprintln!("=== matched rules for .{class} @ vw={w}: {} ===", m.len());
        let mut saw_grid = false;
        for (spec, order, decls, _imp) in &m {
            let has = decls.iter().any(|(p, _)| { let n = beak_engine::css::prop_name(*p); n == "display" || n.starts_with("grid") });
            if has {
                eprintln!("  spec={spec} order={order}:");
                for (p, v) in decls.iter() {
                    let n = beak_engine::css::prop_name(*p);
                    if n == "display" || n.starts_with("grid") || n == "column-gap" {
                        eprintln!("      {n}: {v}");
                        if n == "display" && v.contains("grid") { saw_grid = true; }
                    }
                }
            }
        }
        eprintln!("=== display:grid present for .{class}: {saw_grid} ===");
        return;
    }
    // Render a real fetched page (HTML file + concatenated CSS file) to a BMP.
    // DCTRL=<html> DCSS=<css> DW=<w> — list the page's form controls (rect,
    // kind, and the text painted inside each) + what a submit would send.
    if let Ok(hp) = std::env::var("DCTRL") {
        let html = fs::read_to_string(&hp).expect("html");
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
        let eng = Engine::new();
        let lay = eng.layout_ext(&html, &css, w);
        eprintln!("controls: {}", lay.controls.len());
        for c in &lay.controls {
            let inside: Vec<&str> = lay.ops.iter().filter_map(|o| match o {
                beak_engine::layout::DrawOp::Text { x, y, text, .. }
                    if *x >= c.x - 2 && *x < c.x + c.w && *y >= c.y - 2 && *y < c.y + c.h => Some(text.as_str()),
                _ => None,
            }).collect();
            eprintln!("  seq={} {:?} at ({},{}) {}x{}  text={:?}", c.seq, c.kind, c.x, c.y, c.w, c.h, inside);
        }
        let dom = beak_engine::dom::parse(&html);
        let forms = beak_engine::forms::collect(&dom);
        for (i, f) in forms.forms.iter().enumerate() {
            eprintln!("  form[{i}] action={:?} get={}", f.action, f.method_get);
        }
        return;
    }

    // DIMG=<html> DCSS=<css> DIMGDIR=<dir> DW=<w> DOUT=<bmp> — render a real
    // page WITH its images, to check that decoded pixels actually reach the
    // canvas (the device showed grey placeholder boxes). Files in DIMGDIR are
    // named after the src with '/' and ':' replaced by '_'.
    if let Ok(hp) = std::env::var("DIMG") {
        let html = fs::read_to_string(&hp).expect("html");
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let dir = std::env::var("DIMGDIR").expect("DIMGDIR");
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400);
        let mut eng = Engine::new();
        // DTHEME=dark reproduces what the device does when the compositor
        // palette is dark: the PAGE stays whatever colour its CSS says, but
        // anything we derive from the theme (form-control chrome, placeholder
        // text, the default text colour) flips. Device-only colour reports are
        // otherwise impossible to reproduce here.
        eng.set_theme(if std::env::var("DTHEME").as_deref() == Ok("dark") {
            Theme::DARK
        } else {
            Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                    link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) }
        });
        eng.images_begin();
        let mut ok = 0; let mut miss = 0; let mut undecodable = 0;
        for src in beak_engine::image_srcs(&html, w) {
            let fname = src.replace('/', "_").replace(':', "_");
            match fs::read(format!("{dir}/{fname}")) {
                Ok(bytes) => {
                    if eng.add_image(&src, &bytes) { ok += 1; }
                    else { undecodable += 1; println!("  UNDECODABLE {} ({} B)", src, bytes.len()); }
                }
                Err(_) => { miss += 1; println!("  NO FILE      {}", src); }
            }
        }
        println!("images: {ok} decoded, {undecodable} undecodable, {miss} not fetched");
        let want_inspect = std::env::var("DINSPECT").is_ok();
        if want_inspect { eng.set_inspect(true); }
        let lay = eng.layout_ext(&html, &css, w);
        if want_inspect {
            let filt = std::env::var("DFILTER").unwrap_or_default();
            println!("=== inspect boxes ({} total){} ===", lay.inspect.len(),
                     if filt.is_empty() { String::new() } else { format!(", filter '{filt}'") });
            for b in &lay.inspect {
                if filt.is_empty() || b.label.contains(&filt) {
                    println!("  x={:>5} y={:>5} w={:>5} h={:>5} depth={:>2}  {}", b.x, b.y, b.w, b.h, b.depth, b.label);
                }
            }
        }
        let painted = lay.ops.iter().filter(|o| matches!(o, beak_engine::layout::DrawOp::Image { .. })).count();
        println!("Image draw-ops in the display list: {painted}");
        for op in &lay.ops {
            if let beak_engine::layout::DrawOp::Image { x, y, w, h, src, .. } = op {
                println!("   IMG x={x:>5} y={y:>5} {w:>4}x{h:<4} {}", &src[src.len().saturating_sub(60)..]);
            }
        }
        println!("guessed (need relayout): {:?}", lay.guessed_image_srcs);
        // CSS images (background-image / mask-image), split by how they are
        // sourced: data: URIs need no network, the rest is the fetch backlog.
        let masks = lay.ops.iter().filter(|o| matches!(o, beak_engine::layout::DrawOp::BgImage { tint: Some(_), .. })).count();
        let bgs = lay.ops.iter().filter(|o| matches!(o, beak_engine::layout::DrawOp::BgImage { tint: None, .. })).count();
        println!("CSS image ops: {masks} mask + {bgs} background   (keys used: {}, still to fetch: {})",
                 lay.css_image_keys.len(), lay.css_image_srcs.len());
        // Feed the CSS-image backlog from DIMGDIR too, the way the shell would.
        // A background cannot move a box, so this needs no relayout — the ops
        // are already in the list, only their pixels were missing.
        let mut css_ok = 0;
        for (key, u) in &lay.css_image_srcs {
            let fname = u.replace('/', "_").replace(':', "_");
            match fs::read(format!("{dir}/{fname}")) {
                Ok(bytes) if eng.add_css_image(*key, &bytes) => css_ok += 1,
                _ => println!("   FETCH {}", if u.len() > 110 { &u[..110] } else { u }),
            }
        }
        println!("CSS images loaded from disk: {css_ok}");
        // The tallest filled rects — an enormous empty box shows up here.
        let mut rects: Vec<(i32,i32,i32,i32,beak_engine::Rgb)> = lay.ops.iter().filter_map(|o| match o {
            beak_engine::layout::DrawOp::RoundRect { x, y, w, h, color, .. }
            | beak_engine::layout::DrawOp::Rect { x, y, w, h, color } => Some((*h,*y,*x,*w,*color)),
            _ => None,
        }).collect();
        rects.sort_by_key(|r| -r.0);
        println!("hoechste Rects (h, y, x, w, farbe):");
        for (h,y,x,w,c) in rects.iter().take(10) {
            println!("   h={h:>6} y={y:>6} x={x:>5} w={w:>5}  #{:02x}{:02x}{:02x}", c.0, c.1, c.2);
        }
        if let Ok(out_path) = std::env::var("DOUT") {
            let h = lay.height.clamp(1, 12000);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            eng.paint(&lay, w, h, 0, &mut buf);
            let row = (w * 3 + 3) & !3;
            let mut bmp = Vec::with_capacity(54 + (row * h) as usize);
            let fsz = 54 + row * h;
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&fsz.to_le_bytes());
            bmp.extend_from_slice(&[0;4]);
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&40u32.to_le_bytes());
            bmp.extend_from_slice(&w.to_le_bytes());
            bmp.extend_from_slice(&h.to_le_bytes());
            bmp.extend_from_slice(&1u16.to_le_bytes());
            bmp.extend_from_slice(&24u16.to_le_bytes());
            bmp.extend_from_slice(&[0;24]);
            for y in (0..h).rev() {
                let mut n = 0u32;
                for x in 0..w {
                    let o = ((y*w+x)*4) as usize;
                    bmp.extend_from_slice(&[buf[o], buf[o+1], buf[o+2]]);
                    n += 3;
                }
                while n < row { bmp.push(0); n += 1; }
            }
            fs::write(&out_path, &bmp).expect("write bmp");
            println!("wrote {out_path} ({}x{})", w, h);
        }
        return;
    }

    // DTIME=<html> DCSS=<css> DW=<width> [DN=<runs>] — how long do parse+
    // cascade+layout and paint actually take? Native, so it is a LOWER bound
    // for the device, which runs the same code under the wasmi interpreter.

    if let Ok(hp) = std::env::var("DTIME") {
        let html = fs::read_to_string(&hp).expect("html");
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1400);
        let n: u32 = std::env::var("DN").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
        let mut eng = Engine::new();
        eng.set_theme(Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                              link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) });
        println!("html {} B, css {} B, width {}", html.len(), css.len(), w);
        for i in 0..n {
            // Break the "layout" number into its three real phases — on the
            // device this whole call is 13 s, so knowing WHICH part decides
            // what to fix.
            let t_dom = std::time::Instant::now();
            let dom = beak_engine::dom::parse(&html);
            let d_dom = t_dom.elapsed();
            let t_css = std::time::Instant::now();
            let sheet = beak_engine::css::collect_all(&dom, &css, beak_engine::css::Media::new(w as f32, false));
            let d_css = t_css.elapsed();
            let t_sl = std::time::Instant::now();
            let links = beak_engine::stylesheet_links(&html);
            let d_sl = t_sl.elapsed();
            let t_is = std::time::Instant::now();
            let imgs = beak_engine::image_srcs(&html, w);
            let d_is = t_is.elapsed();
            println!("       dom::parse {:>7.1} ms   css::collect_all {:>7.1} ms   stylesheet_links {:>7.1} ms ({})   image_srcs {:>7.1} ms ({})",
                d_dom.as_secs_f64()*1000.0, d_css.as_secs_f64()*1000.0,
                d_sl.as_secs_f64()*1000.0, links.len(),
                d_is.as_secs_f64()*1000.0, imgs.len());
            let t0 = std::time::Instant::now();
            let lay = eng.layout_ext(&html, &css, w);
            let t_lay = t0.elapsed();
            let h = lay.height.clamp(1, 6000);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let t1 = std::time::Instant::now();
            eng.paint(&lay, w, h, 0, &mut buf);
            let t_paint = t1.elapsed();
            println!("run {}: layout {:>7.1} ms   paint {:>7.1} ms   (page height {})",
                i, t_lay.as_secs_f64()*1000.0, t_paint.as_secs_f64()*1000.0, lay.height);
        }
        return;
    }

    // DPAINT=<html> DCSS=<css> DW=<width> DVH=<viewport height> — measure the
    // paint the DEVICE actually does: one viewport-sized buffer, repainted at a
    // series of scroll offsets, which is what a scroll costs. DTIME paints the
    // whole document once, so it hides both the per-frame canvas clear and the
    // fact that a scrolled frame still walks the entire display list.
    if let Ok(hp) = std::env::var("DPAINT") {
        let html = fs::read_to_string(&hp).expect("html");
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1900);
        let vh: u32 = std::env::var("DVH").ok().and_then(|s| s.parse().ok()).unwrap_or(950);
        let mut eng = Engine::new();
        eng.set_theme(light());
        eng.set_viewport_h(vh);
        let lay = eng.layout_ext(&html, &css, w);
        // What the display list asks the rasteriser to touch, per frame: how
        // many ops it walks, and how many pixels the rects alone cover once
        // clipped to the viewport. Overdraw > 1 viewport means the same pixel
        // is written several times before the text lands on top.
        let (mut nr, mut nt, mut ni, mut glyphs) = (0u64, 0u64, 0u64, 0u64);
        for op in &lay.ops {
            match op {
                DrawOp::Rect { .. } | DrawOp::RoundRect { .. } => nr += 1,
                DrawOp::Text { text, .. } => { nt += 1; glyphs += text.chars().count() as u64 }
                DrawOp::Image { .. } | DrawOp::BgImage { .. } => ni += 1,
            }
        }
        println!("ops {} (rect {nr}, text {nt} / {glyphs} glyphs, img {ni})   page height {}   viewport {w}x{vh}",
                 lay.ops.len(), lay.height);
        // Every guessed src costs a FULL re-layout when its pixels land.
        println!("guessed image boxes: {}", lay.guessed_image_srcs.len());
        for s in &lay.guessed_image_srcs {
            println!("   {s}");
        }
        let mut buf = vec![0u8; (w * vh * 4) as usize];
        for scroll in [0i32, 1000, 2000, 3000, 4000] {
            // Rect pixels this frame would write, clipped to the viewport.
            let mut px = 0i64;
            for op in &lay.ops {
                if let DrawOp::Rect { x, y, w: rw, h: rh, .. } = op {
                    let (x0, y0) = ((*x).max(0), (*y - scroll).max(0));
                    let (x1, y1) = ((*x + *rw).min(w as i32), (*y - scroll + *rh).min(vh as i32));
                    px += ((x1 - x0).max(0) as i64) * ((y1 - y0).max(0) as i64);
                }
            }
            // Warm the glyph cache first: the device keeps it across frames, so
            // the steady-state scroll cost is what matters, not the first paint.
            eng.paint(&lay, w, vh, scroll, &mut buf);
            let t = std::time::Instant::now();
            for _ in 0..5 {
                eng.paint(&lay, w, vh, scroll, &mut buf);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / 5.0;
            println!("  scroll {scroll:>5}: paint {ms:>6.2} ms   rect px {:>10}  (= {:.2} viewports)",
                     px, px as f64 / (w as f64 * vh as f64));
        }
        return;
    }

    // DPAGE=<html> DCSS=<css> DW=<width> DOUT=<bmp> cargo test --test diag
    if let Ok(hp) = std::env::var("DPAGE") {
        let html = fs::read_to_string(&hp).expect("html");
        let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
        let mut eng = Engine::new();
        // DDARK=1 renders on the DARK palette — the device default, and the one
        // difference that makes a page look fine here and black there.
        eng.set_theme(if std::env::var("DDARK").is_ok() {
            Theme::DARK
        } else {
            Theme { bg: Rgb(255,255,255), text: Rgb(33,37,41), heading: Rgb(33,37,41),
                    link: Rgb(13,110,253), muted: Rgb(108,117,125), rule: Rgb(222,226,230) }
        });
        let lay = eng.layout_ext(&html, &css, w);
        let h = lay.height.clamp(1, 6000);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        eng.paint(&lay, w, h, 0, &mut buf);
        // BMP (24-bit, bottom-up) — same writer as lib.rs demos.
        let row = (w * 3 + 3) & !3;
        let mut bmp = Vec::with_capacity(54 + (row * h) as usize);
        let fsz = 54 + row * h;
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&fsz.to_le_bytes());
        bmp.extend_from_slice(&[0;4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&w.to_le_bytes());
        bmp.extend_from_slice(&h.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0;24]);
        for y in (0..h).rev() {
            let mut n = 0u32;
            for x in 0..w {
                let o = ((y*w+x)*4) as usize;
                bmp.extend_from_slice(&[buf[o], buf[o+1], buf[o+2]]);
                n += 3;
            }
            while n < row { bmp.push(0); n += 1; }
        }
        let out = std::env::var("DOUT").unwrap_or_else(|_| "page.bmp".into());
        fs::write(&out, &bmp).expect("write bmp");
        eprintln!("page render: {}x{} px, {} ops, {} links → {out}", w, h, lay.ops.len(), lay.links.len());
        return;
    }
    if let Ok(h) = std::env::var("DIAGHTML") {
        dump("LITERAL", &h);
        return;
    }
    let Ok(rel) = std::env::var("DIAG") else { eprintln!("set DIAG=<rel path>"); return };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt");
    let tp = root.join(&rel);
    let html = fs::read_to_string(&tp).expect("test file");
    dump("TEST", &html);
    // ref via rel=match
    if let Some(i) = html.find("rel=\"match\"").or_else(|| html.find("rel='match'")).or_else(|| html.find("rel=match")) {
        let tag = &html[..];
        if let Some(h) = tag[i..].find("href=") {
            let s = i + h + 5;
            let q = tag.as_bytes()[s] as char;
            let start = if q=='"'||q=='\'' { s+1 } else { s };
            let end = start + tag[start..].find(|c| c=='"'||c=='\''||c=='>'||c==' ').unwrap_or(0);
            let href = &tag[start..end];
            let rp = tp.parent().unwrap().join(href);
            if let Ok(rh) = fs::read_to_string(&rp) { dump(&format!("REF {href}"), &rh); }
        }
    }
}

/// DRECT=<html> DW=<w> — dump every RECT op (backgrounds, borders, stripes).
#[test]
fn diag_rects() {
    let Ok(hp) = std::env::var("DRECT") else { return };
    let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(800);
    let html = fs::read_to_string(&hp).expect("html");
    let mut eng = Engine::new();
    eng.set_theme(light());
    let lay = eng.layout_ext(&html, "", w);
    eprintln!("page height = {}", lay.height);
    for op in &lay.ops {
        if let DrawOp::Rect { x, y, w, h, color } = op {
            eprintln!("  RECT x={x:>5} y={y:>5} w={w:>5} h={h:>4} {:?}", color);
        }
    }
}

#[test]
fn sizeof_style() {
    println!("sizeof(ComputedStyle) = {} B", std::mem::size_of::<beak_engine::style::ComputedStyle>());
}

/// DPHASE=<html> DCSS=<css> [DW=w] [DH=h] — split the one "parse+cascade+
/// layout" number the device reports into its three phases, and time what a
/// pure viewport-HEIGHT change actually costs. The dock bar shifting beak by a
/// few pixels re-ran all three on device (~6.4 s each, twice per hover).
#[test]
fn phase() {
    let Ok(hp) = std::env::var("DPHASE") else { return };
    let html = fs::read_to_string(&hp).expect("html");
    let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1880);
    let h: u32 = std::env::var("DH").ok().and_then(|s| s.parse().ok()).unwrap_or(1000);
    println!("html {} KiB, css {} KiB, {w}x{h}", html.len() / 1024, css.len() / 1024);

    let t = std::time::Instant::now();
    let dom = beak_engine::dom::parse(&html);
    let t_parse = t.elapsed();

    let media = beak_engine::css::Media::new(w as f32, false);
    let t = std::time::Instant::now();
    let sheet = beak_engine::css::collect_all(&dom, &css, media);
    let t_css = t.elapsed();

    // Whole-pipeline runs through the public entry, the way the app calls it.
    let mut eng = Engine::new();
    eng.set_theme(light());
    eng.set_viewport_h(h);
    let t = std::time::Instant::now();
    let lay = eng.layout_ext(&html, &css, w);
    let t_first = t.elapsed();
    let vh_used = lay.viewport_h_used;

    // Same size again: everything cached that can be.
    let t = std::time::Instant::now();
    let _ = eng.layout_ext(&html, &css, w);
    let t_same = t.elapsed();

    // ONLY the viewport height changes — the dock-hover case.
    eng.set_viewport_h(h - 40);
    let t = std::time::Instant::now();
    let _ = eng.layout_ext(&html, &css, w);
    let t_hchange = t.elapsed();

    println!("  dom::parse        {:>7.0} ms", t_parse.as_secs_f64() * 1000.0);
    println!("  css::collect_all  {:>7.0} ms  ({})", t_css.as_secs_f64() * 1000.0, if sheet.is_empty() { "empty" } else { "non-empty" });
    println!("  full layout_ext   {:>7.0} ms  (first)", t_first.as_secs_f64() * 1000.0);
    println!("  full layout_ext   {:>7.0} ms  (same size again)", t_same.as_secs_f64() * 1000.0);
    println!("  full layout_ext   {:>7.0} ms  (HEIGHT changed only)", t_hchange.as_secs_f64() * 1000.0);
    println!("  viewport_h_used = {vh_used}   -> height-only resize {}",
             if vh_used { "MUST re-lay-out" } else { "can reuse the layout (repaint only)" });
}

/// DHOVER=<html> DCSS=<css> [DW=] — the census that decides how `:hover`
/// should invalidate: lay the page out at rest, then with the pointer on each
/// of a few real links, and count how many elements actually get a DIFFERENT
/// computed style. If that is a handful, targeted invalidation is the answer;
/// if it is hundreds, only a cheaper layout is.
#[test]
fn hover_cost_census() {
    let Ok(hp) = std::env::var("DHOVER") else { return };
    let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1880);
    let html = fs::read_to_string(&hp).expect("DHOVER");

    let eng = beak_engine::Engine::new();
    eng.set_viewport_h(1000);

    let t = std::time::Instant::now();
    let rest = eng.layout_ext(&html, &css, w);
    let first = t.elapsed();
    let t = std::time::Instant::now();
    let rest2 = eng.layout_ext(&html, &css, w);
    let second = t.elapsed();
    println!("layout: first {first:?}  second (sheet cached) {second:?}");
    println!("hover_boxes: {}  ops: {}", rest.hover_boxes.len(), rest.ops.len());
    drop(rest2);

    if rest.hover_boxes.is_empty() {
        println!("page has no :hover rules — nothing to measure");
        return;
    }

    // Probe a grid over the VISIBLE area — a box below the fold cannot change
    // a pixel, and the first census wasted every probe that way.
    let (vw, vh) = (w, 1000u32);
    let mut probes: Vec<(i32, i32)> = Vec::new();
    for gy in (20..vh as i32).step_by(37) {
        for gx in (20..vw as i32).step_by(53) {
            probes.push((gx, gy));
        }
    }
    println!("probing {} points over the visible area", probes.len());

    let (mut hits, mut sum_px, mut sum_ms) = (0usize, 0usize, 0u128);
    let mut worst = (0usize, (0, 0, 0, 0), (0, 0));
    for (px, py) in probes {
        let hovered = rest.hover_at(px, py);
        if hovered.is_empty() {
            continue;
        }
        eng.set_hover(hovered.clone());
        let t = std::time::Instant::now();
        let hot = eng.layout_ext(&html, &css, w);
        let relayout = t.elapsed();
        let (px_changed, _total, bbox) = pixels_differ(&eng, &rest, &hot, vw, vh);
        eng.set_hover(Vec::new());
        if px_changed == 0 {
            continue;
        }
        hits += 1;
        sum_px += px_changed;
        sum_ms += relayout.as_millis();
        if px_changed > worst.0 {
            worst = (px_changed, bbox, (px, py));
        }
    }
    let total_px = (vw * vh) as usize;
    println!("--- {hits} of the probed points change ANY pixel ---");
    if hits > 0 {
        println!(
            "average dirty area {:.4} % of the viewport, average relayout {} ms",
            100.0 * (sum_px as f32 / hits as f32) / total_px as f32,
            sum_ms / hits as u128,
        );
        println!(
            "worst point {:?}: {} px ({:.4} %), dirty rect {:?}",
            worst.2,
            worst.0,
            100.0 * worst.0 as f32 / total_px as f32,
            worst.1,
        );
    }
}

/// Paint both layouts and count differing pixels, plus their bounding box —
/// exactly what a damage-driven repaint would have to redraw.
fn pixels_differ(
    eng: &beak_engine::Engine,
    a: &beak_engine::Layout,
    b: &beak_engine::Layout,
    w: u32,
    h: u32,
) -> (usize, usize, (i32, i32, i32, i32)) {
    let mut pa = vec![0u8; (w * h * 4) as usize];
    let mut pb = vec![0u8; (w * h * 4) as usize];
    eng.paint(a, w, h, 0, &mut pa);
    eng.paint(b, w, h, 0, &mut pb);
    let (mut n, mut x0, mut y0, mut x1, mut y1) = (0usize, i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let i = ((y as u32 * w + x as u32) * 4) as usize;
            if pa[i..i + 3] != pb[i..i + 3] {
                n += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    let bbox = if n == 0 { (0, 0, 0, 0) } else { (x0, y0, x1 - x0 + 1, y1 - y0 + 1) };
    (n, (w * h) as usize, bbox)
}

// ── DHOPS: the op-level hover census ───────────────────────────────────────
// The pixel census (DHOVER) said HOW MUCH changes. This says WHAT changes in
// the display list, which is what decides the shape of a paint-only path:
// if the op COUNT is stable and only colour fields move, a patch is enough;
// if ops appear/disappear, the list has to be re-emitted.
//
//   DHOPS=<html> DCSS=<css> [DW=1880] [DN=40]
//     cargo test --release --test diag hover_op_census -- --nocapture

/// Everything about an op that the rasteriser reads, as text — so two ops
/// compare field-by-field without the engine needing `PartialEq`.
fn op_full(op: &DrawOp) -> String {
    match op {
        DrawOp::Text { x, y, size, color, bold, italic, mono, text } => format!(
            "T x={x} y={y} s={size:.2} c={color:?} b={bold} i={italic} m={mono} {text:?}"
        ),
        DrawOp::Rect { x, y, w, h, color } => format!("R x={x} y={y} w={w} h={h} c={color:?}"),
        DrawOp::RoundRect { x, y, w, h, r, color, ring } => {
            format!("Q x={x} y={y} w={w} h={h} r={r:?} c={color:?} ring={ring:.2}")
        }
        DrawOp::Image { x, y, w, h, src, alt } => {
            format!("I x={x} y={y} w={w} h={h} {src:?} {alt:?}")
        }
        DrawOp::BgImage { x, y, w, h, key, repeat, pos, size, tint } => format!(
            "B x={x} y={y} w={w} h={h} k={key} rep={repeat:?} p={pos:?} sz={size:?} t={tint:?}"
        ),
    }
}

/// The part of an op that a paint-only change must NOT be able to move: kind
/// plus geometry. Two ops with the same shape differ only in appearance.
fn op_shape(op: &DrawOp) -> String {
    match op {
        DrawOp::Text { x, y, size, text, .. } => format!("T x={x} y={y} s={size:.2} n={}", text.len()),
        DrawOp::Rect { x, y, w, h, .. } => format!("R x={x} y={y} w={w} h={h}"),
        DrawOp::RoundRect { x, y, w, h, .. } => format!("Q x={x} y={y} w={w} h={h}"),
        DrawOp::Image { x, y, w, h, .. } => format!("I x={x} y={y} w={w} h={h}"),
        DrawOp::BgImage { x, y, w, h, .. } => format!("B x={x} y={y} w={w} h={h}"),
    }
}

#[test]
fn hover_op_census() {
    let Ok(hp) = std::env::var("DHOPS") else { return };
    let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1880);
    let want: usize = std::env::var("DN").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let html = fs::read_to_string(&hp).expect("DHOPS");

    // What do the sheet's `:hover` rules even DECLARE? A text census over the
    // whole sheet is an upper bound (not every rule matches), but it is the
    // cheap half of the answer and it names the properties to classify first.
    {
        let mut props: std::collections::BTreeMap<String, usize> = Default::default();
        let mut blocks = 0usize;
        let b = css.as_bytes();
        let mut i = 0usize;
        while let Some(p) = css[i..].find(":hover") {
            let at = i + p;
            // the selector this compound belongs to ends at the next `{`
            let Some(open) = css[at..].find('{') else { break };
            let open = at + open;
            // …but only if no `}` or `;` intervenes (else the `:hover` was in
            // a value or a comment, not a selector)
            if css[at..open].contains('}') || css[at..open].contains(';') {
                i = at + 6;
                continue;
            }
            let Some(close) = css[open..].find('}') else { break };
            let close = open + close;
            blocks += 1;
            for decl in css[open + 1..close].split(';') {
                if let Some(c) = decl.find(':') {
                    let name = decl[..c].trim().to_ascii_lowercase();
                    if !name.is_empty() && name.len() < 40 {
                        *props.entry(name).or_default() += 1;
                    }
                }
            }
            i = close.min(b.len());
        }
        let mut v: Vec<_> = props.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        println!("--- sheet census: {blocks} `:hover` blocks declare ---");
        for (name, n) in v.iter().take(25) {
            println!("  {n:>4}x  {name}");
        }
        if v.len() > 25 {
            println!("  … {} more property names", v.len() - 25);
        }
    }

    let eng = Engine::new();
    eng.set_viewport_h(1000);
    let rest = eng.layout_ext(&html, &css, w);
    println!("\nrest: {} ops, {} hover_boxes, height {}", rest.ops.len(), rest.hover_boxes.len(), rest.height);
    if rest.hover_boxes.is_empty() {
        println!("page has no :hover rules — nothing to measure");
        return;
    }
    let rest_full: Vec<String> = rest.ops.iter().map(op_full).collect();
    let rest_shape: Vec<String> = rest.ops.iter().map(op_shape).collect();
    {
        let mut kinds: std::collections::BTreeMap<char, usize> = Default::default();
        for f in &rest_full {
            *kinds.entry(f.as_bytes()[0] as char).or_default() += 1;
        }
        println!("  op kinds {kinds:?}   links {}", rest.links.len());
    }

    // Probe the visible area, same grid as DHOVER, but stop after DN hits.
    let (vw, vh) = (w, 1000u32);
    let mut hits = 0usize;
    let (mut geom_stable, mut sum_repaint, mut sum_ins, mut sum_del) = (0usize, 0usize, 0usize, 0usize);
    let (mut said_paint_only, mut wrong, mut missed) = (0usize, 0usize, 0usize);
    let (mut tried, mut exact, mut wrong_patch) = (0usize, 0usize, 0usize);
    let (mut sum_layout_us, mut sum_patch_us) = (0u128, 0u128);
    let (mut worst_repaint, mut worst_ins) = (0usize, 0usize);
    let mut seen: std::collections::HashSet<Vec<u32>> = Default::default();
    let mut examples: Vec<String> = Vec::new();
    'probe: for gy in (20..vh as i32).step_by(17) {
        for gx in (20..vw as i32).step_by(23) {
            let hovered = rest.hover_at(gx, gy);
            if hovered.is_empty() || !seen.insert(hovered.clone()) {
                continue;
            }
            // Lay the page out at rest, then hot, then PATCH the resting one
            // and see whether it came out the same. That is the whole claim.
            eng.set_hover(Vec::new());
            let mut base = eng.layout_ext(&html, &css, w);
            let verdict = eng.set_hover(hovered.clone());
            let t = std::time::Instant::now();
            let hot = eng.layout_ext(&html, &css, w);
            sum_layout_us += t.elapsed().as_micros();
            let claims_paint_only =
                matches!(verdict, beak_engine::raster::HoverChange::Changed { paint_only: true });
            let t = std::time::Instant::now();
            let patched = claims_paint_only && eng.repaint_hover(&mut base);
            let patch_us = t.elapsed().as_micros();
            if patched {
                sum_patch_us += patch_us;
            }
            if patched {
                tried += 1;
                let got: Vec<String> = base.ops.iter().map(op_full).collect();
                let want: Vec<String> = hot.ops.iter().map(op_full).collect();
                if got == want {
                    exact += 1;
                } else {
                    if wrong_patch < 3 {
                        println!("  PATCH != LAYOUT for seqs {hovered:?}");
                        for (i, (a, b)) in got.iter().zip(&want).enumerate().filter(|(_, (a, b))| a != b).take(3) {
                            println!("      #{i} patched {a}\n      #{i} layout  {b}");
                        }
                        if got.len() != want.len() {
                            println!("      op count {} vs {}", got.len(), want.len());
                        }
                    }
                    wrong_patch += 1;
                }
            }
            eng.set_hover(Vec::new());
            let hot_full: Vec<String> = hot.ops.iter().map(op_full).collect();
            if hot_full == rest_full {
                continue; // this element styles nothing on hover
            }
            hits += 1;
            let hot_shape: Vec<String> = hot.ops.iter().map(op_shape).collect();
            // Align on GEOMETRY: ops that keep their kind+rect are the same box
            // painted again. What is left over is a true insert or delete.
            let al = lcs(&rest_shape, &hot_shape);
            let repaint = al.iter().filter(|&&(i, j)| rest_full[i] != hot_full[j]).count();
            let del = rest_full.len() - al.len();
            let ins = hot_full.len() - al.len();
            if ins == 0 && del == 0 {
                geom_stable += 1;
            }
            // The engine's verdict against what the pixels actually did. An
            // op that MOVED would prove the "paint only" claim wrong; ops
            // added or removed at unchanged rects would not.
            let moved = al.iter().any(|&(i, j)| rest_shape[i] != hot_shape[j]);
            if claims_paint_only {
                said_paint_only += 1;
                if moved {
                    wrong += 1;
                    println!("  !!! claimed paint-only but geometry MOVED: seqs {hovered:?}");
                }
            } else if !moved {
                missed += 1;
            }
            sum_repaint += repaint;
            sum_ins += ins;
            sum_del += del;
            worst_repaint = worst_repaint.max(repaint);
            worst_ins = worst_ins.max(ins);
            if examples.len() < 8 {
                let mut e = format!(
                    "  seqs {:?} at ({gx},{gy}): {} -> {} ops | {repaint} repainted, {ins} added, {del} removed",
                    if hovered.len() > 6 { &hovered[..6] } else { &hovered[..] },
                    rest_full.len(), hot_full.len(),
                );
                for &(i, j) in al.iter().filter(|&&(i, j)| rest_full[i] != hot_full[j]).take(2) {
                    e.push_str(&format!("\n      was {}\n      now {}", rest_full[i], hot_full[j]));
                }
                let mut k = 0usize;
                let inserted: Vec<usize> = {
                    let keep: std::collections::HashSet<usize> = al.iter().map(|&(_, j)| j).collect();
                    (0..hot_full.len()).filter(|j| !keep.contains(j)).collect()
                };
                for j in inserted.iter().take(2) {
                    e.push_str(&format!("\n      ADDED {}", hot_full[*j]));
                    k += 1;
                }
                let _ = k;
                examples.push(e);
            }
            if hits >= want {
                break 'probe;
            }
        }
    }

    println!("\n--- {hits} distinct hover targets that change the display list ---");
    if hits == 0 {
        return;
    }
    println!("  geometry FULLY stable:  {geom_stable}/{hits}  (no op added or removed)");
    println!("  repainted ops: avg {:.1}, worst {worst_repaint}  of {} total ({:.3} %)",
             sum_repaint as f32 / hits as f32, rest_full.len(),
             100.0 * (sum_repaint as f32 / hits as f32) / rest_full.len() as f32);
    println!("  added ops:     avg {:.1}, worst {worst_ins}", sum_ins as f32 / hits as f32);
    println!("\n  engine verdict `paint only`: {said_paint_only}/{hits}");
    println!("    of those WRONG (something moved): {wrong}");
    println!("    said relayout though nothing moved: {missed}");
    println!("\n  REPAINT statt Layout: {tried}/{hits} gepatcht");
    println!("    davon byte-gleich mit dem vollen Layout: {exact}");
    println!("    davon ABWEICHEND: {wrong_patch}");
    if tried > 0 {
        println!(
            "\n  Kosten je Zeigerwechsel: volles Layout {:.2} ms  vs  Repaint {:.3} ms  = {:.0}x",
            sum_layout_us as f64 / hits as f64 / 1000.0,
            sum_patch_us as f64 / tried as f64 / 1000.0,
            (sum_layout_us as f64 / hits as f64) / (sum_patch_us as f64 / tried as f64),
        );
    }
    println!("  removed ops:   avg {:.1}", sum_del as f32 / hits as f32);
    println!("\nexamples:");
    for e in &examples {
        println!("{e}");
    }
}

/// Longest common subsequence as index pairs. The op lists are in document
/// order and edits are local, so this aligns "the same box, painted again"
/// against "an op that genuinely appeared" — an index-wise diff cannot, it
/// reports every op after an insertion as changed.
fn lcs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * (m + 1) + j] = if a[i] == b[j] {
                dp[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                dp[(i + 1) * (m + 1) + j].max(dp[i * (m + 1) + j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0usize, 0usize, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * (m + 1) + j] >= dp[i * (m + 1) + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// DHBOX=<html> DCSS=<css> [DW=] [DX= DY=] — dump the hover boxes that contain
/// a point, with their rects. A box whose rect contains the point but whose
/// paint is elsewhere means the hit-test geometry is wrong.
#[test]
fn hover_box_dump() {
    let Ok(hp) = std::env::var("DHBOX") else { return };
    let css = std::env::var("DCSS").ok().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let w: u32 = std::env::var("DW").ok().and_then(|s| s.parse().ok()).unwrap_or(1880);
    let x: i32 = std::env::var("DX").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let y: i32 = std::env::var("DY").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let html = fs::read_to_string(&hp).expect("DHBOX");
    let eng = Engine::new();
    eng.set_viewport_h(1000);
    let lay = eng.layout_ext(&html, &css, w);
    println!("{} hover_boxes; those containing ({x},{y}):", lay.hover_boxes.len());
    for b in lay.hover_boxes.iter().filter(|b| x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h) {
        println!("  seq {:>5}  x={:<5} y={:<5} w={:<5} h={:<4}", b.seq, b.x, b.y, b.w, b.h);
    }
    // …and the same seqs' OTHER boxes, if any (an inline box spanning lines).
    let hit: Vec<u32> = lay.hover_at(x, y);
    println!("hover_at -> {hit:?}");
    for s in hit.iter().take(8) {
        for b in lay.hover_boxes.iter().filter(|b| b.seq == *s) {
            println!("  seq {s} box x={} y={} w={} h={}", b.x, b.y, b.w, b.h);
        }
    }
}
