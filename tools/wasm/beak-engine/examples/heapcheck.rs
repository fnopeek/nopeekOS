//! Wieviel Halde kostet was — GEHALTEN und als SPITZE, getrennt.
//!
//! Ausgeloest von einem Absturz am Geraet: beak gab beim Beenden 79 MB zurueck,
//! davon 59 MB gewachsen, und `beakbench` zeigte, dass das Schriftrastern
//! allein von 11 auf 89 MiB treibt. Was `beakbench` NICHT trennen kann, ist
//! „braucht der Browser das dauerhaft" von „das war eine Spitze beim Parsen,
//! und talc gibt Seiten nie zurueck" — die Antwort entscheidet, ob hier etwas
//! zu holen ist.
//!
//!     cargo run --release --example heapcheck
//!
//! Der Allokator zaehlt Bytes, nicht Seiten: eine Wasm-Halde ist immer
//! groesser als die Summe der lebenden Allokationen (Verschnitt, Bins), aber
//! das VERHAELTNIS von gehalten zu Spitze ist dasselbe.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

// SAFETY: reicht jede Anfrage an `System` durch und zaehlt nur mit.
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

    // Die sechs eingebauten Gesichter — der Posten, um den es geht.
    let f = beak_engine::fonts::Fonts::new();
    zeile("Fonts::new() (6 Gesichter)", start);
    let nach_fonts = LIVE.load(Relaxed);

    // Je Gesicht einzeln — ueber `add_web`, das denselben Parser fuehrt.
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

    // **Und jetzt die Frage, die zaehlt: wieviele Gesichter fasst eine ECHTE
    // Seite an?** Faul zu laden hilft nur, wenn eine Seite nicht ohnehin alle
    // sechs beruehrt.
    for p in std::env::args().skip(1) {
        let Ok(html) = std::fs::read_to_string(&p) else { continue };
        let css = std::fs::read_to_string(p.replace(".html", ".css")).unwrap_or_default();
        reset();
        let vor = LIVE.load(Relaxed);
        let fonts = beak_engine::fonts::Fonts::new();
        let dom = beak_engine::dom::parse(&html);
        // Mit dem externen Blatt, sonst misst man eine Seite ohne ihr CSS —
        // und die fasst weniger Gesichter an als die echte.
        let sheet = beak_engine::css::collect_all(&dom, &css, beak_engine::css::Media::new(1902.0, false));
        let lay = beak_engine::layout::layout(
            &fonts, &dom, &sheet, &beak_engine::image::ImageMap::new(),
            1902, 993, &beak_engine::Theme::DARK,
            &beak_engine::forms::FormState::default(), false, &[], false);
        let name = p.rsplit('/').next().unwrap_or(&p);
        // **Die SPITZE bestimmt die Halde, nicht das Gehaltene.** Ein
        // Wasm-Heap waechst auf den groessten gleichzeitigen Stand und gibt
        // nie zurueck; was danach frei wird, senkt ihn nicht mehr.
        println!("  {:<24} {} von 6 Gesichtern   gehalten {:>6.1} MB   SPITZE {:>6.1} MB   ({} px hoch)",
                 name, fonts.loaded_faces(), mb(LIVE.load(Relaxed).saturating_sub(vor)),
                 mb(PEAK.load(Relaxed).saturating_sub(vor)), lay.height);
    }
    println!("\n  {:<28} gehalten {:>7.1} MB   nach ALLEM\n",
             "", mb(LIVE.load(Relaxed)));
    let _ = nach_fonts;
}
