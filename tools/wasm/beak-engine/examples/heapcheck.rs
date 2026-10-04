//! Heap cost per component, held and peak separately: "the browser needs this
//! permanently" versus "a peak while parsing, and talc never returns pages".
//!
//!     cargo run --release --example heapcheck
//!
//! The allocator counts bytes, not pages: a Wasm heap is always larger than
//! the sum of live allocations (fragmentation, bins), but the ratio of held
//! to peak is the same.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: forwards every request to `System` and only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() { bump(l.size() as isize) }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        bump(-(l.size() as isize));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() { bump(new as isize - l.size() as isize) }
        q
    }
}

fn bump(d: isize) {
    let now = if d >= 0 {
        LIVE.fetch_add(d as usize, Relaxed) + d as usize
    } else {
        LIVE.fetch_sub((-d) as usize, Relaxed) - (-d) as usize
    };
    PEAK.fetch_max(now, Relaxed);
}

#[global_allocator]
static A: Counting = Counting;

fn mb(b: usize) -> f64 { b as f64 / (1024.0 * 1024.0) }

fn reset() { PEAK.store(LIVE.load(Relaxed), Relaxed); }

fn zeile(was: &str, vorher: usize) {
    let live = LIVE.load(Relaxed);
    let peak = PEAK.load(Relaxed);
    println!("  {:<28} gehalten {:>7.1} MB   (+{:>6.1} MB)   Spitze {:>7.1} MB",
             was, mb(live), mb(live.saturating_sub(vorher)), mb(peak));
}

fn main() {
    println!("\n  === Halde: gehalten vs. Spitze ===\n");
    let start = LIVE.load(Relaxed);
    reset();

    // The six built-in faces.
    let f = beak_engine::fonts::Fonts::new();
    zeile("Fonts::new() (6 Gesichter)", start);
    let nach_fonts = LIVE.load(Relaxed);

    // Each face on its own, via `add_web`, which runs the same parser.
    println!();
    let mut g = beak_engine::fonts::Fonts::new();
    for (n, name) in ["inter.ttf", "inter-bold.ttf", "inter-italic.ttf",
                      "inter-bolditalic.ttf", "mono.ttf", "mono-bold.ttf"].iter().enumerate() {
        let bytes = std::fs::read(format!("assets/{name}")).expect("Schrift lesbar");
        reset();
        let vor = LIVE.load(Relaxed);
        assert!(g.add_web(1000 + n as u32, 400, false, &bytes));
        zeile(name, vor);
    }
    drop(g);

    drop(f);

    // How many faces does a real page touch? Lazy loading only helps if a
    // page does not touch all six anyway.
    for p in std::env::args().skip(1) {
        let Ok(html) = std::fs::read_to_string(&p) else { continue };
        let css = std::fs::read_to_string(p.replace(".html", ".css")).unwrap_or_default();
        reset();
        let vor = LIVE.load(Relaxed);
        let fonts = beak_engine::fonts::Fonts::new();
        let dom = beak_engine::dom::parse(&html);
        // With the external stylesheet; a page without its CSS touches fewer
        // faces than the real one.
        let sheet = beak_engine::css::collect_all(&dom, &css, beak_engine::css::Media::new(1902.0, false));
        let lay = beak_engine::layout::layout(
            &fonts, &dom, &sheet, &beak_engine::image::ImageMap::new(),
            1902, 993, &beak_engine::Theme::DARK,
            &beak_engine::forms::FormState::default(), false, &[], false);
        let name = p.rsplit('/').next().unwrap_or(&p);
        // The peak sizes the heap, not what is held: a Wasm heap grows to the
        // largest simultaneous use and never shrinks.
        println!("  {:<24} {} von 6 Gesichtern   gehalten {:>6.1} MB   SPITZE {:>6.1} MB   ({} px hoch)",
                 name, fonts.loaded_faces(), mb(LIVE.load(Relaxed).saturating_sub(vor)),
                 mb(PEAK.load(Relaxed).saturating_sub(vor)), lay.height);
    }
    println!("\n  {:<28} gehalten {:>7.1} MB   nach ALLEM\n",
             "", mb(LIVE.load(Relaxed)));
    let _ = nach_fonts;
}
