//! Runs a JS file and prints what it wrote to the console.
//!
//! `js::run` collects the console and discards it; for a probe the console is
//! the result. `CAP=1` sets the step limit so an endless loop ends as a
//! `RangeError` instead of hanging.
//!
//!   cargo run --release --example jsrun -- probe.js
/// Randomness for host tools, from `/dev/urandom`. Without it there is no
/// `crypto`, and the tool would measure a different platform than beak.
fn host_random(out: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut f) => f.read_exact(out).is_ok(),
        Err(_) => false,
    }
}

fn main() {
    beak_engine::js::random::set_source(host_random);
    let arg = std::env::args().nth(1).unwrap_or_default();
    let script = std::fs::read_to_string(&arg).unwrap_or(arg);
    // `MODULE=1` parses as a module. beak tries both, since the file does not
    // say which it is, so the probe must be able to as well.
    let module = std::env::var("MODULE").is_ok();
    let prog = match beak_engine::js::parse(&script, module) {
        Ok(p) => p,
        Err(e) => {
            let at = e.at.min(script.len());
            let head = script[..at].rsplit('\n').next().unwrap_or("");
            let tail = &script[at..(at + 60).min(script.len())];
            println!("SyntaxError: {} @{} ...{}<<HIER>>{}...",
                     e.msg, e.at,
                     &head[head.len().saturating_sub(60)..],
                     tail.split('\n').next().unwrap_or(""));
            return;
        }
    };
    let mut i = beak_engine::js::interp::Interp::new();
    // `NOVM=1` runs the same file without the bytecode machine. Diffing the
    // two outputs checks that both machines have the same semantics.
    if std::env::var("NOVM").is_ok() { i.vm_off = true; }
    // `HTML=<file|text>` attaches a document. Without it there is no `document`
    // at all; that is by design of the engine.
    if let Ok(h) = std::env::var("HTML") {
        let html = std::fs::read_to_string(&h).unwrap_or(h);
        let dom = beak_engine::dom::parse(&html);
        i.set_document(beak_engine::js::dombind::Doc::from_dom(&dom));
        // Submit the same cascade context beak submits, or `getComputedStyle`
        // answers differently here than in beak.
        let media = beak_engine::css::Media::new(1024.0, std::env::var("DARK").is_ok());
        let ext = std::env::var("CSS").ok().and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let sheet = beak_engine::css::collect_all(&dom, &ext, media);
        i.set_style_context(beak_engine::js::interp::StyleCtx {
            sheet: std::rc::Rc::new(sheet),
            theme: beak_engine::layout::Theme {
                bg: beak_engine::layout::Rgb(255, 255, 255),
                text: beak_engine::layout::Rgb(33, 37, 41),
                heading: beak_engine::layout::Rgb(33, 37, 41),
                link: beak_engine::layout::Rgb(13, 110, 253),
                muted: beak_engine::layout::Rgb(108, 117, 125),
                rule: beak_engine::layout::Rgb(222, 226, 230),
            },
            viewport_w: 1024.0,
        });
        // A window too, or there is no `matchMedia`. `DARK=1` flips the colour
        // scheme.
        i.set_media(1024.0, 768.0, std::env::var("DARK").is_ok());
    }
    if std::env::var("CAP").is_ok() { i.max_steps = 2_000_000; }
    let r = i.run_program(&prog);
    // Drain timers and microtasks; a probe that ends on `setTimeout` would
    // otherwise have no result.
    for _ in 0..64 { if i.run_timers() == 0 { break } }
    for l in &i.console { println!("{l}"); }
    if let Err(beak_engine::js::interp::Abrupt::Throw(v)) = r {
        let m = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
        println!("UNCAUGHT: {}", m.as_deref().unwrap_or("?"));
    }
}
