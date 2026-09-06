//! Rasteriser — draws a `Layout`'s display list into a BGRA pixel buffer.
//!
//! Glyphs come from fontdue (Inter, the same font the compositor uses); the
//! layout is ours. `paint` renders the visible slice at a scroll offset, so
//! the buffer stays viewport-sized regardless of document length. Pure — the
//! same code paints on nopeekOS (into a `Widget::Canvas`) and on the desktop
//! adapter (into a window framebuffer), see docs/spec/BROWSER.md §10.

use alloc::vec::Vec;
use core::cell::RefCell;
use fontdue::Metrics;
use hashbrown::HashMap;

use crate::fonts::Fonts;
use crate::layout::{DrawOp, Layout, Rgb, Rgba, Theme};
use crate::color::ColorFilter;
use crate::style::{BgPos, BgSize, GradKind, Gradient, ObjectFit};

/// What a pointer move costs — the answer `Engine::set_hover` gives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoverChange {
    /// The pointer is inside exactly the same elements as before.
    Unchanged,
    /// The state changed. `paint_only` means no rule that can gain or lose
    /// here declares anything that moves a box, so the geometry of the whole
    /// page is unchanged and only the look of those elements differs.
    Changed { paint_only: bool },
}

impl HoverChange {
    /// Did anything change at all?
    pub fn is_changed(self) -> bool {
        self != HoverChange::Unchanged
    }
}

pub struct Engine {
    /// Die Schriften. `RefCell`, weil eine Seite ihre eigenen erst NACH dem
    /// ersten Auslegen mitbringt und `layout` nur `&self` hat.
    fonts: core::cell::RefCell<Fonts>,
    /// Schriften, die die Seite verlangt und die noch fehlen: `(Adresse,
    /// Familie, Gewicht, kursiv)`. Der Wirt holt sie und meldet sich mit
    /// `add_font` zurueck — dieselbe Runde wie beim Modulgraphen und den
    /// nachgeladenen Stilblaettern.
    pending_fonts: core::cell::RefCell<alloc::vec::Vec<(alloc::string::String, u32, u16, bool)>>,
    /// Adressen, die schon angefragt wurden — sonst fragt jedes Auslegen neu.
    asked_fonts: core::cell::RefCell<alloc::vec::Vec<alloc::string::String>>,
    /// Rasterised-glyph cache keyed by (char, size-bits, face-id). fontdue's
    /// rasterise is not free; without this every glyph is re-rasterised every
    /// frame, which makes scrolling lag. Bounded by the glyph set the page uses.
    glyphs: RefCell<HashMap<(u32, u32, u32), (Metrics, Vec<u8>)>>,
    /// Page colours (theme-resolved by the shell; dark until then).
    theme: Theme,
    /// Decoded page images keyed by `<img src>`. Fetched ones are handed in by
    /// the shell each nav; a `data:` src carries its own bytes and is decoded
    /// during layout — the same two clocks `css_images` runs on, which is why
    /// this is a `RefCell` and not a plain map.
    images: RefCell<crate::image::ImageMap>,
    /// Decoded CSS images (`background-image`/`mask-image`) keyed by
    /// `css::url_key`. Separate from `images` so a page's `<img src="x">` and
    /// a stylesheet's `url(x)` cannot collide, and because these are resolved
    /// on a different clock: `data:` URIs decode during layout, fetched ones
    /// arrive later from the shell.
    css_images: RefCell<HashMap<u64, alloc::rc::Rc<crate::image::Image>>>,
    /// Decoded-BGRA budget for CSS images, separate from `img_budget` so a
    /// page full of icons cannot starve its `<img>`s (or the reverse).
    css_img_budget: core::cell::Cell<usize>,
    /// Remaining decoded-BGRA budget for the current page (streaming decode).
    img_budget: core::cell::Cell<usize>,
    /// Decoded `<img>` pixels kept ACROSS navigations, keyed by the RESOLVED
    /// url and oldest-first.
    ///
    /// Keyed by url and NOT by the `src` attribute, which is what `images` uses
    /// — `/logo.png` is a different picture on a different host, and a cache
    /// that confused the two would show one site's image on another's page.
    ///
    /// `Rc` is what makes it cheap: an image the next page uses again costs
    /// its pixels ONCE, shared between this cache and the page map. Only
    /// pictures no live page holds are paid for twice, and `IMG_CACHE_BUDGET`
    /// bounds those.
    img_cache: RefCell<Vec<(alloc::string::String, alloc::rc::Rc<crate::image::Image>)>>,
    /// Bytes of BGRA currently in `img_cache`.
    img_cache_bytes: core::cell::Cell<usize>,
    /// The same cache for `background-image`/`mask-image` layers. Separate
    /// from `img_cache` for the reason `css_images` is separate from `images`:
    /// a page's `<img src=x>` and a sheet's `url(x)` must not collide.
    css_cache: RefCell<Vec<(alloc::string::String, alloc::rc::Rc<crate::image::Image>)>>,
    css_cache_bytes: core::cell::Cell<usize>,
    /// Viewport height (px) — the initial containing block's height, which
    /// `top`/`bottom`/`height` percentages on root-level absolutely positioned
    /// boxes resolve against (CSS 2.1 §10.1). Device state like `theme`, not
    /// page content, so it lives here rather than in every layout signature.
    /// `Cell` for the same reason `glyphs` is a `RefCell`: the shell holds the
    /// engine by shared reference across a frame.
    viewport_h: core::cell::Cell<u32>,
    /// When set, `layout` records an `InspectBox` per element box (the dev
    /// tool). Off by default so the label-formatting cost is only paid while the
    /// user is inspecting; the shell toggles it and re-lays-out.
    inspect: core::cell::Cell<bool>,
    /// The pointer state the LAST layout was made with. `repaint_hover` needs
    /// both: the style a box is painted with now, and the one it should be
    /// painted with next.
    hover_prev: RefCell<Vec<u32>>,
    /// `seq`s of the elements the pointer is inside, ascending — what `:hover`
    /// reads. Device state like `theme`, not page content, so it lives here
    /// rather than in every layout signature.
    hover: RefCell<Vec<u32>>,
    /// A tick source lent by the shell, so a layout can report what each phase
    /// cost on the machine that is actually slow. `None` on the host, where
    /// `tests/diag.rs` times the phases from outside.
    clock: core::cell::Cell<Option<fn() -> u64>>,
    /// Why the last pointer repaint gave up — see `Engine::repaint_bail`.
    repaint_bail: core::cell::Cell<&'static str>,
    /// The last parsed DOCUMENT with the fingerprint of the inputs that built
    /// it. Parsing a real page is ~170 ms on the device, and a page is laid out
    /// several times over its life from unchanged bytes — an image landing, a
    /// form key, the pointer entering a link. Every one of those re-parsed the
    /// whole HTML for nothing.
    ///
    /// The width and the palette are part of the identity because
    /// `picture::resolve` BAKES the winning `srcset` candidate into the tree:
    /// the same bytes at a different width are a different document.
    /// Parsed documents, most-recently-used FIRST — **index 0 is the current
    /// page** and every reader below takes that one.
    ///
    /// More than one slot because going back is the most common navigation
    /// there is, and the DOM the reader is returning to was thrown away by the
    /// page in between. Measured on the device: revisiting an article whose
    /// bytes had not changed cost 110 ms parse + 710 ms cascade a second time,
    /// purely because one other page had been visited.
    ///
    /// Keyed by content (plus width and theme), so it can only ever hand back
    /// a document identical to the one that would have been parsed. A page
    /// that answers differently every request — a live front page — misses by
    /// construction, and that is correct rather than unfortunate.
    dom: RefCell<Vec<(u64, crate::dom::Dom)>>,
    /// Ein vom SKRIPT veraenderter Baum. Ist er gesetzt, wird nicht geparst
    /// und nicht zwischengespeichert — er IST das Dokument.
    ///
    /// So herum, weil der Zwischenspeicher auf dem HTML-Fingerabdruck sitzt:
    /// derselbe Quelltext, aber ein anderer Baum, und der Abdruck wuesste
    /// nichts davon.
    scripted: RefCell<Option<crate::dom::Dom>>,
    /// Zaehlt hoch, sobald ein Skript den Baum veraendert hat. Geht in den
    /// SCHLUESSEL des Stilblatts ein — ein Skript kann ein `<style>`
    /// hinzufuegen, und dann waere das zwischengespeicherte Blatt falsch.
    scripted_gen: core::cell::Cell<u64>,
    /// Fuer JEDES Element einen Treffer-Kasten aufzeichnen, nicht nur fuer die
    /// mit `:hover`-Regeln.
    ///
    /// beak schaltet das ein, sobald eine Seite Ereignisbehandler angemeldet
    /// hat: ohne einen Kasten je Element gibt es keinen Weg vom Klickpunkt zum
    /// Knoten. Aus, solange keiner da ist — die Liste ist ein `push` je
    /// Element, und dieser Pfad wurde einmal gemessen und gekuerzt.
    hit_all: core::cell::Cell<bool>,
    /// The last parsed stylesheet with the fingerprint of the inputs that built
    /// it. Parsing a real page's CSS is a third of a layout, and a page is laid
    /// out several times over its life (images landing, a form key, a resize)
    /// from unchanged bytes — so the parse is repeated for nothing.
    /// Collected stylesheets, same shape and same rule as `dom`: index 0 is
    /// the current page's.
    sheet: RefCell<Vec<(u64, crate::css::Stylesheet)>>,
    /// How often a document was really parsed, and a sheet really collected.
    /// The point of the slots above is that these stop counting on a revisit,
    /// and a time is too noisy to assert on.
    docs_parsed: core::cell::Cell<u64>,
    sheets_collected: core::cell::Cell<u64>,
}

/// Cheap content fingerprint (FNV-1a over 8-byte words). Identity by pointer
/// would be wrong here: the shell parses into one static buffer, so a different
/// document can land at the same address with the same length.
fn fingerprint(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut it = b.chunks_exact(8);
    for c in &mut it {
        h ^= u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &x in it.remainder() {
        h ^= x as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ b.len() as u64
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Insert into a cross-navigation cache, oldest-first, evicting from the front
/// until the budget holds.
///
/// An evicted entry's pixels stay alive as long as a page still references them
/// (`Rc`) — eviction bounds the CACHE, not the page. Both caches share this one
/// routine and one budget, so they cannot drift apart.
fn cache_put(
    slots: &RefCell<alloc::vec::Vec<(alloc::string::String, alloc::rc::Rc<crate::image::Image>)>>,
    bytes: &core::cell::Cell<usize>,
    budget: usize,
    url: &str,
    img: alloc::rc::Rc<crate::image::Image>,
) {
    let n = img.bgra.len();
    if n > budget {
        return; // one picture may not evict the whole cache for itself
    }
    {
        let mut c = slots.borrow_mut();
        if let Some(pos) = c.iter().position(|(u, _)| u == url) {
            let (_, old) = c.remove(pos);
            bytes.set(bytes.get() - old.bgra.len());
        }
        c.push((alloc::string::String::from(url), img));
    }
    bytes.set(bytes.get() + n);
    while bytes.get() > budget {
        let mut c = slots.borrow_mut();
        if c.is_empty() {
            break;
        }
        let (_, old) = c.remove(0);
        bytes.set(bytes.get() - old.bgra.len());
    }
}

/// How many parsed documents (and stylesheets) to keep.
///
/// Three, not more: a `Dom` plus its `Stylesheet` for a real article is
/// megabytes, and this shares a 128 MB heap with a 24 MB page image budget and
/// an 8 MB image cache. Three covers what it is for — the page you are on, the
/// one you came from, and the one before that.
const DOC_SLOTS: usize = 3;

/// Move the entry keyed `key` to the front and say whether it was there.
///
/// Front means current: every reader takes index 0, so a hit has to be
/// promoted and not merely found.
/// Der Baum, aus dem das LETZTE Layout gebaut wurde.
///
/// **Der Grund, warum das eine eigene Funktion ist:** `layout_forms` umgeht den
/// Parse-Zwischenspeicher, sobald ein Skript einen Baum zurueckgeschrieben hat
/// (`let dom_hit = scripted || promote(…)`). Damit bleibt `self.dom` auf jeder
/// Seite mit Skripten LEER — und beide Schnellwege, die ihn lasen, gaben
/// stillschweigend auf. Der Hover-Weg (gemessen 0,16 ms gegen 24 ms Layout)
/// und der Steuerelement-Weg liefen auf keiner echten Seite.
///
/// Gemerkt hat es niemand, weil der eine Weg nichts sagte und der andere
/// seinen Grund nur im Fehlerfall meldete
/// ([[feedback-the-fast-path-must-say-it-ran]]).
fn current_dom<'a>(
    scripted: &'a Option<crate::dom::Dom>,
    cached: &'a alloc::vec::Vec<(u64, crate::dom::Dom)>,
) -> Option<&'a crate::dom::Dom> {
    match scripted {
        Some(d) => Some(d),
        None => cached.first().map(|(_, d)| d),
    }
}

fn promote<T>(slots: &mut alloc::vec::Vec<(u64, T)>, key: u64) -> bool {
    match slots.iter().position(|(k, _)| *k == key) {
        Some(0) => true,
        Some(pos) => {
            let e = slots.remove(pos);
            slots.insert(0, e);
            true
        }
        None => false,
    }
}

impl Engine {
    /// Parse the embedded font faces. Cheap enough to build once and reuse
    /// across page loads (the shell keeps one `Engine`).
    pub fn new() -> Engine {
        Engine {
            fonts: core::cell::RefCell::new(Fonts::new()),
            pending_fonts: core::cell::RefCell::new(alloc::vec::Vec::new()),
            asked_fonts: core::cell::RefCell::new(alloc::vec::Vec::new()),
            glyphs: RefCell::new(HashMap::new()),
            theme: Theme::DARK,
            images: RefCell::new(crate::image::ImageMap::new()),
            css_images: RefCell::new(HashMap::new()),
            css_img_budget: core::cell::Cell::new(crate::image::CSS_BUDGET),
            img_budget: core::cell::Cell::new(crate::image::TOTAL_BUDGET),
            img_cache: RefCell::new(Vec::new()),
            img_cache_bytes: core::cell::Cell::new(0),
            css_cache: RefCell::new(Vec::new()),
            css_cache_bytes: core::cell::Cell::new(0),
            // 600 keeps the historical behaviour of the reftest canvas for any
            // caller that never sets it.
            viewport_h: core::cell::Cell::new(600),
            inspect: core::cell::Cell::new(false),
            hover: RefCell::new(Vec::new()),
            hover_prev: RefCell::new(Vec::new()),
            clock: core::cell::Cell::new(None),
            repaint_bail: core::cell::Cell::new(""),
            dom: RefCell::new(Vec::new()),
            scripted: RefCell::new(None),
            scripted_gen: core::cell::Cell::new(0),
            hit_all: core::cell::Cell::new(false),
            sheet: RefCell::new(Vec::new()),
            docs_parsed: core::cell::Cell::new(0),
            sheets_collected: core::cell::Cell::new(0),
        }
    }

    /// Tell the engine how tall the viewport is (see `viewport_h`).
    pub fn set_viewport_h(&self, h: u32) {
        if h > 0 {
            self.viewport_h.set(h);
        }
    }

    /// Enable/disable the inspect dev tool. When on, the next `layout` records
    /// an element box per node into `Layout::inspect`; the shell re-lays-out.
    pub fn set_inspect(&self, on: bool) {
        self.inspect.set(on);
    }

    /// Tell the engine which elements the pointer is inside — `Layout::hover_at`
    /// produces the list from the previous layout. Says what the change COSTS:
    /// a pointer that stayed in the same elements must cost nothing, and one
    /// that entered something which only recolours must not cost a layout.
    pub fn set_hover(&self, seqs: Vec<u32>) -> HoverChange {
        let mut cur = self.hover.borrow_mut();
        if *cur == seqs {
            return HoverChange::Unchanged;
        }
        // Only the elements that GAINED or LOST the state can restyle; one that
        // is in both lists is unaffected by the move.
        let moved: Vec<u32> = cur
            .iter()
            .chain(seqs.iter())
            .filter(|s| (cur.contains(s) as u8 + seqs.contains(s) as u8) == 1)
            .copied()
            .collect();
        *self.hover_prev.borrow_mut() = core::mem::replace(&mut *cur, seqs);
        drop(cur);
        HoverChange::Changed { paint_only: self.hover_is_paint_only(&moved) }
    }

    /// Can the elements whose pointer state just changed only be REPAINTED?
    ///
    /// True when no `:hover` rule that could gain or lose on any of them
    /// declares a property that moves something. Conservative in the direction
    /// that matters: an unknown answer is "no", which costs a layout we might
    /// not have needed — never a stale page.
    fn hover_is_paint_only(&self, moved: &[u32]) -> bool {
        let held = self.sheet.borrow();
        let Some((_, sheet)) = held.first() else { return false };
        // Nothing on the page styles geometry on hover — no lookup needed.
        if sheet.hover_layout_set.is_empty() {
            return true;
        }
        let held_dom = self.dom.borrow();
        let Some((_, dom)) = held_dom.first() else { return false };
        fn walk(el: &crate::dom::Element, want: &[u32], set: &crate::css::HoverSet) -> bool {
            for c in &el.children {
                if let crate::dom::Node::Element(e) = c {
                    if want.contains(&e.seq) && set.may_match(e) {
                        return true;
                    }
                    if walk(e, want, set) {
                        return true;
                    }
                }
            }
            false
        }
        !walk(&dom.root, moved, &sheet.hover_layout_set)
    }

    /// Answer a paint-only pointer change by patching the display list, with
    /// no parse, no cascade over the page and no box arithmetic.
    ///
    /// Only call it after `set_hover` said `paint_only`. `false` means the
    /// patch could not be made with certainty and the caller must lay out —
    /// the layout it was given is then untouched, because every change is
    /// applied only once all of them are known to be possible.
    /// Ein Steuerelement neu malen statt die Seite auszulegen. `false` heisst:
    /// es geht nicht, legt aus.
    ///
    /// Der Anrufer ist jedes Ereignis, das nur den Zustand eines Kastens
    /// aendert — Tastendruck im Feld, Fokus, Fokusverlust, Haekchen. Auf
    /// Wikipedia kostete jedes davon 280 ms.
    pub fn repaint_controls(&self, lay: &mut Layout, state: &crate::forms::FormState) -> bool {
        let held = self.sheet.borrow();
        let scripted = self.scripted.borrow();
        let held_dom = self.dom.borrow();
        let Some((_, sheet)) = held.first() else {
            self.repaint_bail.set("kein Blatt im Zwischenspeicher");
            return false;
        };
        let Some(dom) = current_dom(&scripted, &held_dom) else {
            self.repaint_bail.set("kein Baum im Zwischenspeicher");
            return false;
        };
        // Nur eine `:checked`-Regel kann durch die Kaskade etwas anderes
        // umstylen; `:focus` und Verwandte matchen bei uns nie.
        let may_restyle = |seq: u32| {
            if sheet.checked_set.is_empty() {
                return false;
            }
            fn find(el: &crate::dom::Element, seq: u32) -> Option<&crate::dom::Element> {
                if el.seq == seq {
                    return Some(el);
                }
                el.children.iter().find_map(|c| match c {
                    crate::dom::Node::Element(e) => find(e, seq),
                    _ => None,
                })
            }
            // Kein Element gefunden heisst: nicht entscheidbar, also auslegen.
            find(&dom.root, seq).map_or(true, |el| sheet.checked_set.may_match(el))
        };
        match crate::layout::repaint_controls(lay, &self.fonts.borrow(), &self.theme, state, &may_restyle) {
            Ok(()) => true,
            Err(why) => {
                self.repaint_bail.set(why);
                false
            }
        }
    }

    pub fn repaint_hover(&self, lay: &mut Layout) -> bool {
        match self.try_repaint_hover(lay) {
            Ok(()) => true,
            Err(why) => {
                self.repaint_bail.set(why);
                false
            }
        }
    }

    /// Why the last `repaint_hover` handed the page to a layout. `""` when the
    /// last one succeeded. Worth saying ONCE per page on the device: a browser
    /// that quietly lays out on every pointer move looks like the feature was
    /// never built ([[feedback-log-the-exception-not-the-rule]]).
    pub fn repaint_bail(&self) -> &'static str {
        self.repaint_bail.get()
    }

    fn try_repaint_hover(&self, lay: &mut Layout) -> Result<(), &'static str> {
        let (prev, cur) = (self.hover_prev.borrow(), self.hover.borrow());
        let moved: Vec<u32> = prev
            .iter()
            .chain(cur.iter())
            .filter(|s| (prev.contains(s) as u8 + cur.contains(s) as u8) == 1)
            .copied()
            .collect();
        if moved.is_empty() {
            return Ok(());
        }
        let held = self.sheet.borrow();
        let Some((_, sheet)) = held.first() else { return Err("no sheet") };
        let scripted = self.scripted.borrow();
        let held_dom = self.dom.borrow();
        let Some(dom) = current_dom(&scripted, &held_dom) else {
            return Err("no cached document");
        };
        let w = lay.width;
        let vh = self.viewport_h.get();
        // A rule that restyles a sibling of the element the pointer is in is
        // out of reach for a subtree walk.
        if !sheet.hover_sideways_set.is_empty() {
            let mut hit = false;
            fn walk(el: &crate::dom::Element, want: &[u32], set: &crate::css::HoverSet, hit: &mut bool) {
                for c in &el.children {
                    if let crate::dom::Node::Element(e) = c {
                        if want.contains(&e.seq) && set.may_match(e) {
                            *hit = true;
                            return;
                        }
                        walk(e, want, set, hit);
                    }
                }
            }
            walk(&dom.root, &moved, &sheet.hover_sideways_set, &mut hit);
            if hit {
                return Err("a rule restyles a sibling");
            }
        }
        let mut groups = Vec::new();
        for &seq in &moved {
            // Bounded on purpose: repainting an element at a time only beats a
            // layout while the subtree is small. `nav:hover` over a menu of a
            // few dozen items is worth it; the same rule on `<body>` is not.
            const SUBTREE_CAP: usize = 128;
            let mut kids_off = Vec::new();
            let mut kids_on = Vec::new();
            let one = |hover: &[u32], out: &mut Vec<crate::layout::StyleProbe>| {
                crate::layout::resolve_out_of_band(dom, sheet, &self.theme, w, vh, seq, hover, SUBTREE_CAP, out)
            };
            let (Some(off), Some(on)) = (one(&prev, &mut kids_off), one(&cur, &mut kids_on)) else {
                return Err("the subtree is too big to repaint one box at a time");
            };
            // The two descents visit the same tree in the same order, so a
            // length mismatch would mean one of them stopped early.
            if kids_off.len() != kids_on.len() {
                return Err("the two descents disagree");
            }
            let mut pairs = alloc::vec![(off, on)];
            pairs.extend(kids_off.into_iter().zip(kids_on));
            let mut text = alloc::string::String::new();
            if let Some(el) = crate::layout::find_seq_pub(dom, seq) {
                crate::layout::subtree_text(el, &mut text);
            }
            groups.push(crate::layout::HoverRepaint {
                text,
                boxes: lay.hover_boxes.iter().filter(|b| b.seq == seq).copied().collect(),
                pairs,
            });
        }
        crate::layout::repaint_hover(lay, &self.fonts.borrow(), &groups)
    }

    /// Put the pointer state back to what the last layout was made with —
    /// for a change that could be neither repainted nor afforded.
    pub fn revert_hover(&self) {
        let prev = self.hover_prev.borrow().clone();
        *self.hover.borrow_mut() = prev;
    }

    /// Does the current page style anything on `:hover`? False means pointer
    /// movement can be ignored outright — no hit-test, no layout.
    pub fn page_has_hover(&self) -> bool {
        self.sheet.borrow().first().is_some_and(|(_, s)| !s.hover_set.is_empty())
    }

    /// Open/close the `<details>` owning the `<summary>` at `seq`
    /// (`Layout::hit_toggle` names it). Returns whether anything changed — the
    /// caller re-lays-out only then.
    ///
    /// This EDITS the cached document: the `open` content attribute is what
    /// the state actually is, so `details[open] > summary` in the page's own
    /// stylesheet gets the right answer for free — rustdoc and MDN both style
    /// the open state that way. Keeping the state beside the DOM instead would
    /// have meant two truths and one of them invisible to the cascade.
    ///
    /// It lives as long as the parsed document does: navigating away and back
    /// re-parses and starts closed again, which is what a browser does too.
    pub fn toggle_details(&self, seq: u32) -> bool {
        let mut held = self.dom.borrow_mut();
        let Some((_, dom)) = held.first_mut() else { return false };
        /// The element whose child subtree holds `seq`.
        fn parent_of(el: &mut crate::dom::Element, seq: u32) -> Option<&mut crate::dom::Element> {
            let mine = el.children.iter().any(
                |c| matches!(c, crate::dom::Node::Element(e) if e.seq == seq),
            );
            if mine {
                return Some(el);
            }
            for c in &mut el.children {
                if let crate::dom::Node::Element(e) = c {
                    if let Some(f) = parent_of(e, seq) {
                        return Some(f);
                    }
                }
            }
            None
        }
        let Some(det) = parent_of(&mut dom.root, seq) else { return false };
        if det.tag != "details" {
            return false;
        }
        match det.attrs.iter().position(|(k, _)| k == "open") {
            Some(i) => {
                det.attrs.remove(i);
            }
            None => det.attrs.push((
                alloc::string::String::from("open"),
                alloc::string::String::new(),
            )),
        }
        true
    }

    /// Set the page colours (the shell resolves these from the compositor
    /// palette so the page follows light/dark).
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Die Farben, mit denen die Engine kaskadiert. `getComputedStyle` muss
    /// dieselben nehmen, sonst antwortet es ueber eine andere Seite als die
    /// gemalte.
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// Start a fresh page's image set: clear the previous decode + reset the
    /// per-page budget. The shell then fetches + `add_image`s each `<img>` ONE
    /// AT A TIME (streaming) so the compressed bytes never pile up — decode the
    /// image, keep only its pixels, reuse the same fetch scratch for the next.
    /// `images_begin` clears the PAGE map, never the cross-navigation cache —
    /// that is the whole point of the cache surviving a navigation.
    pub fn images_begin(&mut self) {
        self.images.get_mut().clear();
        self.img_budget.set(crate::image::TOTAL_BUDGET);
        // Drop the previous page's rasterised glyphs. The cache is keyed by
        // (char, size, face) and never evicts, so without this it grows across
        // every navigation (and every distinct font size) until the heap OOMs.
        // Bounding it to one page's working set costs only a lazy re-rasterise
        // of the visible glyphs on the first paint after a nav.
        self.glyphs.get_mut().clear();
    }

    /// Decode ONE image and store it under `src`. The compressed `bytes` are
    /// borrowed (dropped by the caller right after) — only the decoded pixels
    /// are retained. Over-budget / undecodable → skipped (renders a
    /// placeholder). Returns whether the image was stored.
    pub fn add_image(&mut self, src: &str, bytes: &[u8]) -> bool {
        self.store_image(src, bytes)
    }

    /// As [`Self::add_image`], and also keep the pixels under `url` for the
    /// next navigation. The shell resolves the url; the engine never sees a
    /// base to resolve against.
    pub fn add_image_cached(&mut self, src: &str, url: &str, bytes: &[u8]) -> bool {
        if !self.store_image(src, bytes) {
            return false;
        }
        let Some(img) = self.images.borrow().get(src).cloned() else { return true };
        self.cache_put(url, img);
        true
    }

    /// Serve `pairs` of `(src, url)` from the cross-navigation cache.
    ///
    /// Returns the `src`s that were served — the shell drops those from its
    /// fetch queue. Called BEFORE the first layout, which is where it pays
    /// twice: no request, no decode, and the box is definite on the first
    /// layout instead of being guessed and moving the page later.
    pub fn adopt_cached(&mut self, pairs: &[(alloc::string::String, alloc::string::String)])
        -> Vec<alloc::string::String>
    {
        let mut served = Vec::new();
        for (src, url) in pairs {
            let hit = self.img_cache.borrow().iter()
                .find(|(u, _)| u == url)
                .map(|(_, img)| img.clone());
            if let Some(img) = hit {
                // Charged against the page budget like any other image: the
                // pixels are live for this page whether or not they are shared.
                let n = img.bgra.len();
                if n > self.img_budget.get() {
                    continue;
                }
                self.img_budget.set(self.img_budget.get() - n);
                self.images.get_mut().insert(src.clone(), img);
                served.push(src.clone());
            }
        }
        served
    }

    /// Insert into the cross-navigation cache, oldest-first, evicting from the
    /// front until the budget holds. An evicted entry's pixels stay alive as
    /// long as a page still references them (`Rc`) — eviction bounds the
    /// CACHE, not the page.
    fn cache_put(&mut self, url: &str, img: alloc::rc::Rc<crate::image::Image>) {
        cache_put(&self.img_cache, &self.img_cache_bytes,
                  crate::image::IMG_CACHE_BUDGET, url, img);
    }

    /// How many documents this engine has actually parsed, and how many
    /// stylesheets it has actually collected, since it was created.
    /// Den vom Skript veraenderten Baum einreichen. Ab jetzt legt das Layout
    /// IHN aus, nicht mehr das geparste HTML.
    ///
    /// `None` nimmt das zurueck — bei einer Navigation muss das passieren,
    /// sonst zeigt die naechste Seite den Baum der vorigen.
    pub fn set_scripted_dom(&self, dom: Option<crate::dom::Dom>) {
        *self.scripted.borrow_mut() = dom;
        self.scripted_gen.set(self.scripted_gen.get().wrapping_add(1));
    }

    pub fn has_scripted_dom(&self) -> bool { self.scripted.borrow().is_some() }

    /// Welche `@font-face`-Schriften die Seite verlangt und noch nicht hat.
    ///
    /// Nur die ERSTE Quelle je Gesicht: die Liste ist die Rangfolge der Seite,
    /// und beak liest WOFF2 und rohes sfnt — die erste Angabe ist praktisch
    /// immer WOFF2.
    fn note_font_faces(&self, sheet: &crate::css::Stylesheet) {
        for f in &sheet.faces {
            let Some(url) = f.src.first() else { continue };
            if self.asked_fonts.borrow().iter().any(|u| u == url) { continue }
            self.asked_fonts.borrow_mut().push(url.clone());
            self.pending_fonts.borrow_mut().push((url.clone(), f.family, f.weight, f.italic));
        }
    }

    /// Was der Wirt holen soll. Leert die Liste — jede Adresse wird einmal
    /// angefragt.
    pub fn take_pending_fonts(&self) -> alloc::vec::Vec<(alloc::string::String, u32, u16, bool)> {
        core::mem::take(&mut self.pending_fonts.borrow_mut())
    }

    /// Eine geholte Schrift aufnehmen. `bytes` darf WOFF2 oder rohes sfnt
    /// sein; alles andere wird abgelehnt, statt als kaputte Schrift zu enden.
    ///
    /// Liefert false, wenn die Bytes nicht lesbar waren — der Wirt meldet das.
    pub fn add_font(&self, family: u32, weight: u16, italic: bool, bytes: &[u8]) -> bool {
        let owned;
        let sfnt: &[u8] = if bytes.starts_with(b"wOF2") {
            match crate::woff2::to_sfnt(bytes) { Some(v) => { owned = v; &owned } None => return false }
        } else if bytes.starts_with(b"wOFF") {
            // WOFF1 packt mit zlib. Nicht gebaut — die Fassung ist praktisch
            // ausgestorben, und eine halbe Umsetzung waere schlechter als ein
            // ehrliches Nein.
            return false;
        } else {
            bytes
        };
        let ok = self.fonts.borrow_mut().add_web(family, weight, italic, sfnt);
        if ok {
            // Alles, was mit der alten Schrift gemessen wurde, ist ueberholt.
            self.sheet.borrow_mut().clear();
            self.dom.borrow_mut().clear();
            self.glyphs.borrow_mut().clear();
        }
        ok
    }

    pub fn web_font_count(&self) -> usize { self.fonts.borrow().web_count() }

    /// Wie oft der Baum seit dem Start durch Skripte ersetzt wurde.
    ///
    /// Der Wirt braucht die Zahl, um sein FORMULARMODELL nachzuziehen: eine
    /// Seite, die ihre Maske erst per Skript baut, hat sonst Steuerelemente
    /// im Bild, die es fuer `submit` gar nicht gibt.
    pub fn scripted_gen(&self) -> u64 { self.scripted_gen.get() }

    /// Etwas auf dem lebenden Baum ausrechnen — ohne ihn herauszugeben, denn
    /// er gehoert dem Motor.
    pub fn with_scripted<R>(&self, f: impl FnOnce(&crate::dom::Dom) -> R) -> Option<R> {
        self.scripted.borrow().as_ref().map(f)
    }

    /// Treffer-Kaesten fuer alle Elemente aufzeichnen (siehe `hit_all`).
    pub fn set_hit_all(&self, on: bool) { self.hit_all.set(on); }

    pub fn parse_counts(&self) -> (u64, u64) {
        (self.docs_parsed.get(), self.sheets_collected.get())
    }

    /// Entries and bytes BOTH cross-navigation caches hold, for the trace.
    pub fn img_cache_stats(&self) -> (usize, usize) {
        (self.img_cache.borrow().len() + self.css_cache.borrow().len(),
         self.img_cache_bytes.get() + self.css_cache_bytes.get())
    }

    /// The one place `<img>` pixels enter the store, shared by the shell's
    /// fetched bytes and by a `data:` src decoded during layout, so the budget
    /// is honoured on both paths.
    fn store_image(&self, src: &str, bytes: &[u8]) -> bool {
        if let Some(img) = crate::image::decode(bytes) {
            if img.bgra.len() <= self.img_budget.get() {
                self.img_budget.set(self.img_budget.get() - img.bgra.len());
                self.images
                    .borrow_mut()
                    .insert(src.into(), alloc::rc::Rc::new(img));
                return true;
            }
        }
        false
    }

    /// Decode every `data:` `<img src>` in the document. Such a src carries its
    /// own bytes — there is nothing to fetch, and the pixels must exist BEFORE
    /// layout because the intrinsic size decides the box. Mirrors
    /// `resolve_css_images`, which does the same for `url(data:…)`.
    fn resolve_data_uri_images(&self, dom: &crate::dom::Dom) {
        fn walk(el: &crate::dom::Element, eng: &Engine) {
            for c in &el.children {
                if let crate::dom::Node::Element(e) = c {
                    if e.tag == "img" {
                        if let Some(src) = e.attr("src").map(str::trim) {
                            if (src.starts_with("data:") || src.starts_with("DATA:"))
                                && !eng.images.borrow().contains_key(src)
                            {
                                if let Some(bytes) = crate::image::decode_data_uri(src) {
                                    eng.store_image(src, &bytes);
                                }
                            }
                        }
                    }
                    walk(e, eng);
                }
            }
        }
        walk(&dom.root, self);
    }

    /// Decode + store a whole batch at once (holds all compressed bytes) — kept
    /// for tests / non-streaming callers; the shell uses `images_begin` +
    /// `add_image` to avoid hoarding.
    pub fn set_images(&mut self, pairs: &[(alloc::string::String, Vec<u8>)]) {
        self.images_begin();
        for (src, bytes) in pairs {
            self.add_image(src, bytes);
        }
    }

    /// Parse + lay out a document at `width`. Scroll-independent. Collects the
    /// page's `<style>` blocks into the author stylesheet used by the cascade.
    pub fn layout(&self, html: &str, width: u32) -> Layout {
        self.layout_ext(html, "", width)
    }

    /// Like `layout`, but also applies `external_css` — the concatenated bytes
    /// of the page's `<link rel=stylesheet>` files, which the shell fetches
    /// (the engine is host-free) and passes in. External CSS cascades before
    /// inline `<style>` (document/head order).
    /// Lend the engine a monotonic tick source, so `Layout::phase` reports
    /// what parse, cascade and layout each cost. Purely optional — the engine
    /// stays free of host functions either way.
    pub fn set_clock(&self, f: fn() -> u64) {
        self.clock.set(Some(f));
    }

    pub fn layout_ext(&self, html: &str, external_css: &str, width: u32) -> Layout {
        self.layout_forms(html, external_css, width, &crate::forms::FormState::default())
    }

    /// Like `layout_ext`, but paints the page's form controls with the user's
    /// live state (typed text, checked boxes, focus + caret). The shell keeps
    /// one `FormState` per page and re-lays out when it changes.
    pub fn layout_forms(
        &self,
        html: &str,
        external_css: &str,
        width: u32,
        forms: &crate::forms::FormState,
    ) -> Layout {
        let now = || self.clock.get().map_or(0, |f| f());
        let t0 = now();
        let dom_key = fingerprint(html.as_bytes())
            ^ (width as u64) << 40
            ^ (self.theme.is_dark() as u64) << 63;
        let scripted = self.scripted.borrow().is_some();
        let dom_hit = scripted || promote(&mut self.dom.borrow_mut(), dom_key);
        if !dom_hit {
            let mut dom = crate::dom::parse(html);
            // `<picture>`/`srcset` is folded into the `<img>` before anything
            // reads a `src` — layout, the fetch list and the draw op then all
            // see the one URL that actually won.
            crate::picture::resolve(&mut dom, crate::css::Media::new(width as f32, self.theme.is_dark()));
            self.docs_parsed.set(self.docs_parsed.get() + 1);
            let mut held = self.dom.borrow_mut();
            held.insert(0, (dom_key, dom));
            held.truncate(DOC_SLOTS);
        }
        let held_dom = self.dom.borrow();
        let held_scripted = self.scripted.borrow();
        let dom = match held_scripted.as_ref() { Some(d) => d, None => &held_dom[0].1 };
        // The cascade also reads the document's own `<style>` blocks and the
        // viewport width (media queries), so both are part of the identity.
        // The theme is part of the identity too: `prefers-color-scheme` decides
        // which rules apply, and `resolve_vars` BAKES the winning custom
        // properties into the text it hands on — so a light and a dark sheet
        // are different documents, not the same one read differently.
        let t_parse = now();
        let media = crate::css::Media::new(width as f32, self.theme.is_dark());
        // The viewport HEIGHT is part of the identity too, since `resolve_vars`
        // bakes custom properties down and one may hold a `vh` length. Without
        // it a purely vertical window resize would keep the stale sheet.
        let key = fingerprint(html.as_bytes())
            ^ fingerprint(external_css.as_bytes()).rotate_left(17)
            ^ (width as u64) << 40
            ^ (self.viewport_h.get() as u64).rotate_left(23)
            ^ (media.dark as u64) << 63
            ^ self.scripted_gen.get().rotate_left(41);
        let sheet_hit = promote(&mut self.sheet.borrow_mut(), key);
        if !sheet_hit {
            let collected = crate::css::collect_all(dom, external_css, media);
            self.sheets_collected.set(self.sheets_collected.get() + 1);
            let mut held = self.sheet.borrow_mut();
            held.insert(0, (key, collected));
            held.truncate(DOC_SLOTS);
        }
        let t_css = now();
        let held = self.sheet.borrow();
        let sheet = &held[0].1;
        self.note_font_faces(sheet);
        self.resolve_data_uri_images(&dom);
        let mut lay = crate::layout::layout(&self.fonts.borrow(), &dom, sheet, &self.images.borrow(), width, self.viewport_h.get(), &self.theme, forms, self.inspect.get(), &self.hover.borrow(), self.hit_all.get());
        self.resolve_inline_svgs(&dom, &lay);
        self.resolve_css_images(sheet, &mut lay);
        lay.phase = [t_parse.wrapping_sub(t0), t_css.wrapping_sub(t_parse), now().wrapping_sub(t_css)];
        lay
    }

    /// Rasterise the inline `<svg>`s a layout painted, under their store keys.
    ///
    /// This runs AFTER layout on purpose: `currentColor` is the element's
    /// computed `color` and the box is CSS's, so neither is known before the
    /// cascade. The box is definite from the markup either way, so nothing has
    /// to be laid out twice.
    fn resolve_inline_svgs(&self, dom: &crate::dom::Dom, lay: &Layout) {
        if lay.inline_svgs.is_empty() {
            return;
        }
        fn walk(el: &crate::dom::Element, lay: &Layout, eng: &Engine) {
            for c in &el.children {
                let crate::dom::Node::Element(e) = c else { continue };
                if e.tag == "svg" {
                    if let Some(&(_, color, w, h)) =
                        lay.inline_svgs.iter().find(|(seq, ..)| *seq == e.seq)
                    {
                        let key = alloc::format!("svg:{}", e.seq);
                        if !eng.images.borrow().contains_key(key.as_str()) {
                            if let Some(img) =
                                crate::svg::render_element(e, color, Some((w, h)))
                            {
                                if img.bgra.len() <= eng.img_budget.get() {
                                    eng.img_budget.set(eng.img_budget.get() - img.bgra.len());
                                    eng.images.borrow_mut().insert(key, alloc::rc::Rc::new(img));
                                }
                            }
                        }
                    }
                    // An <svg> subtree holds no HTML; nothing below it to visit.
                    continue;
                }
                walk(e, lay, eng);
            }
        }
        walk(&dom.root, lay, self);
    }

    /// Turn the CSS image keys a layout needs back into URLs.
    ///
    /// A `data:` URI carries its own bytes, so the engine decodes it here and
    /// the shell never hears about it; everything else is reported in
    /// `css_image_srcs` for the shell to fetch and hand back via
    /// `add_css_image`. Already-decoded keys are skipped, so this stays cheap
    /// across the several layouts one page runs through.
    fn resolve_css_images(&self, sheet: &crate::css::Stylesheet, lay: &mut Layout) {
        for &key in &lay.css_image_keys {
            if self.css_images.borrow().contains_key(&key) {
                continue;
            }
            let Some(url) = sheet.url(key) else { continue };
            if url.starts_with("data:") || url.starts_with("DATA:") {
                if let Some(bytes) = crate::image::decode_data_uri(url) {
                    self.store_css_image(key, &bytes);
                }
            } else {
                lay.css_image_srcs.push((key, alloc::string::String::from(url)));
            }
        }
    }

    fn store_css_image(&self, key: u64, bytes: &[u8]) -> bool {
        if let Some(img) = crate::image::decode(bytes) {
            let budget = self.css_img_budget.get();
            if img.bgra.len() <= budget {
                self.css_img_budget.set(budget - img.bgra.len());
                self.css_images.borrow_mut().insert(key, alloc::rc::Rc::new(img));
                return true;
            }
        }
        false
    }

    /// Store a CSS image the shell fetched (see `Layout::css_image_srcs`).
    /// Costs a repaint, never a re-layout: a background cannot move a box.
    pub fn add_css_image(&self, key: u64, bytes: &[u8]) -> bool {
        self.store_css_image(key, bytes)
    }

    /// As [`Self::add_css_image`], and keep the pixels under `url` for the next
    /// navigation.
    ///
    /// `url_key` is a hash of the `url()` text as the sheet wrote it, so it is
    /// unique only WITHIN one document — two sites both saying `url(/bg.png)`
    /// share a key. Across navigations the RESOLVED url is the only honest
    /// identity, exactly as for `<img>`.
    pub fn add_css_image_cached(&self, key: u64, url: &str, bytes: &[u8]) -> bool {
        if !self.store_css_image(key, bytes) {
            return false;
        }
        let Some(img) = self.css_images.borrow().get(&key).cloned() else { return true };
        cache_put(&self.css_cache, &self.css_cache_bytes,
                  crate::image::CSS_CACHE_BUDGET, url, img);
        true
    }

    /// Serve one background layer from the cross-navigation cache. True on a
    /// hit — the shell then does not queue it for fetching.
    pub fn adopt_css_cached(&self, key: u64, url: &str) -> bool {
        let hit = self.css_cache.borrow().iter()
            .find(|(u, _)| u == url)
            .map(|(_, img)| img.clone());
        let Some(img) = hit else { return false };
        let n = img.bgra.len();
        if n > self.css_img_budget.get() {
            return false;
        }
        self.css_img_budget.set(self.css_img_budget.get() - n);
        self.css_images.borrow_mut().insert(key, img);
        true
    }

    /// Drop the previous page's CSS images (called on navigation). The
    /// cross-navigation cache is untouched, same as `images_begin`.
    pub fn css_images_begin(&self) {
        self.css_images.borrow_mut().clear();
        self.css_img_budget.set(crate::image::CSS_BUDGET);
    }

    /// Lay out with the UA sheet ONLY — no author `<style>`/`<link>` CSS
    /// (reader mode; docs/spec/BROWSER.md §9.7 "never worse than clean content").
    pub fn layout_ua(&self, html: &str, width: u32) -> Layout {
        self.layout_ua_forms(html, width, &crate::forms::FormState::default())
    }

    /// Reader mode with live form state (see `layout_forms`).
    pub fn layout_ua_forms(&self, html: &str, width: u32, forms: &crate::forms::FormState) -> Layout {
        let mut dom = crate::dom::parse(html);
        crate::picture::resolve(&mut dom, crate::css::Media::new(width as f32, self.theme.is_dark()));
        self.resolve_data_uri_images(&dom);
        crate::layout::layout(
            &self.fonts.borrow(),
            &dom,
            &crate::css::Stylesheet::empty(),
            &self.images.borrow(),
            width,
            self.viewport_h.get(),
            &self.theme,
            forms,
            self.inspect.get(),
            // Reader mode drops the page's sheet, so nothing can style `:hover`.
            &[],
            self.hit_all.get(),
        )
    }

    /// Paint the slice `[scroll_y, scroll_y + h)` into `out` (must be
    /// `w * h * 4` BGRA bytes).
    /// Paint only the viewport rows `y0..y1` — the same picture [`Self::paint`]
    /// would put there, without touching the rest of the buffer.
    ///
    /// This is what makes a scroll cheap. Scrolling does not change the page;
    /// it moves it. The pixels that merely moved are shifted with one
    /// `copy_within`, and only the newly exposed band is drawn — against ~60-80
    /// ms for a full 1902x1000 repaint on the device, which is what every
    /// scroll used to cost.
    ///
    /// No clipping had to be added anywhere for this, and that is the whole
    /// trick: every drawing primitive already clips against `(0, 0, w, h)` of
    /// the buffer it is handed. A band is just a narrower buffer — hand over
    /// those rows' slice, say `h = y1 - y0`, and move the scroll offset down by
    /// `y0` so document coordinates still land where they belong.
    pub fn paint_band(
        &self,
        layout: &Layout,
        w: u32,
        h: u32,
        scroll_y: i32,
        out: &mut [u8],
        y0: u32,
        y1: u32,
    ) {
        let (y0, y1) = (y0.min(h), y1.min(h));
        if y1 <= y0 {
            return;
        }
        let stride = w as usize * 4;
        let band = &mut out[y0 as usize * stride..y1 as usize * stride];
        self.paint(layout, w, y1 - y0, scroll_y + y0 as i32, band);
    }

    pub fn paint(&self, layout: &Layout, w: u32, h: u32, scroll_y: i32, out: &mut [u8]) {
        let (wi, hi) = (w as i32, h as i32);
        // Canvas = the propagated body background (falls back to theme bg).
        fill(out, wi, hi, 0, 0, wi, hi, layout.bg.into());
        for op in &layout.ops {
            match op {
                DrawOp::Rect { x, y, w: rw, h: rh, color } => {
                    fill(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *color);
                }
                DrawOp::RoundRect { x, y, w: rw, h: rh, r, color, ring } => {
                    fill_round(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *r, *color, *ring);
                }
                DrawOp::Shadow { x, y, w: rw, h: rh, blur, color, dx, dy, spread } => {
                    // Der Kasten, der ausgespart bleibt: das Schattenrechteck
                    // zurueckgerechnet auf den Rahmenkasten.
                    let keep = (*x - *dx + *spread, *y - *dy + *spread - scroll_y,
                                *rw - 2 * *spread, *rh - 2 * *spread);
                    fill_shadow(out, wi, hi, *x, *y - scroll_y, *rw, *rh, *blur, *color, keep);
                }
                DrawOp::Text { x, y, size, color, bold, italic, mono, family, sp, text } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + (*size as i32) + 6 < 0 {
                        continue; // fully off-screen line → skip
                    }
                    self.draw_run(out, wi, hi, *x, vy, *size, *color, *bold, *italic, *mono, *family, *sp, text);
                }
                DrawOp::Image { x, y, w: iw, h: ih, src, alt, fit, filter } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *ih < 0 {
                        continue;
                    }
                    // Look the pixels up at PAINT time, so an image that
                    // arrives after layout needs only a repaint. A miss (not
                    // fetched yet, or an undecodable format) draws the
                    // placeholder that layout used to emit as separate ops.
                    match self.images.borrow().get(src) {
                        Some(img) => blit_image(out, wi, hi, *x, vy, *iw, *ih, img, *fit, filt(layout, *filter)),
                        None => self.draw_img_placeholder(out, wi, hi, *x, vy, *iw, *ih, alt),
                    }
                }
                DrawOp::Gradient { x, y, w: gw, h: gh, clip, repeat, pos, size, r, g } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *gh < 0 {
                        continue;
                    }
                    let cl = (clip.0, clip.1 - scroll_y, clip.2, clip.3);
                    fill_gradient(out, wi, hi, *x, vy, *gw, *gh, cl, *repeat, *pos, *size, *r, g);
                }
                DrawOp::BgImage { x, y, w: bw, h: bh, clip, key, repeat, pos, size, tint, filter } => {
                    let vy = *y - scroll_y;
                    if vy > hi || vy + *bh < 0 {
                        continue;
                    }
                    let cl = (clip.0, clip.1 - scroll_y, clip.2, clip.3);
                    // A missing background draws NOTHING — unlike `<img>`,
                    // there is no placeholder for one: the box is styled and
                    // sized either way, so an absent decoration must simply be
                    // absent rather than a grey frame over the content.
                    if let Some(img) = self.css_images.borrow().get(key) {
                        blit_bg(out, wi, hi, *x, vy, *bw, *bh, cl, img, *repeat, *pos, *size, *tint, filt(layout, *filter));
                    }
                }
            }
        }
    }

    /// The box an `<img>` shows while its pixels are missing: a thin frame
    /// plus the alt text. Lives here rather than in layout so that an image
    /// arriving later swaps the placeholder for the picture without the
    /// display list changing at all.
    #[allow(clippy::too_many_arguments)]
    fn draw_img_placeholder(
        &self,
        out: &mut [u8],
        wi: i32,
        hi: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        alt: &str,
    ) {
        let c = Rgba::opaque(self.theme.rule);
        fill(out, wi, hi, x, y, w, 1, c);
        fill(out, wi, hi, x, y + h - 1, w, 1, c);
        fill(out, wi, hi, x, y, 1, h, c);
        fill(out, wi, hi, x + w - 1, y, 1, h, c);
        if !alt.is_empty() && w > 24 {
            self.draw_run(out, wi, hi, x + 4, y + 4, 13.0, self.theme.muted.into(), false, false, false, 0, (0.0, 0.0), alt);
        }
    }

    /// Draw a run at `(x, y=run-top)` in the face selected by `bold`/`italic`/
    /// `mono` (see `Fonts::pick`) — real weight/slant/monospace, no synthesis.
    #[allow(clippy::too_many_arguments)]
    fn draw_run(
        &self,
        out: &mut [u8],
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        size: f32,
        color: Rgba,
        bold: bool,
        italic: bool,
        mono: bool,
        // Streuwert der `font-family` — dieselbe Zahl, mit der das Layout
        // gemessen hat. Ohne sie malte der Rasterer die eingebaute Schrift
        // unter die Breiten einer Seitenschrift.
        family: u32,
        // `(letter-spacing, word-spacing)` — the SAME pair layout measured the
        // run with. Advancing the pen by anything else puts the glyphs somewhere
        // the line box did not reserve.
        sp: (f32, f32),
        text: &str,
    ) {
        let fonts = self.fonts.borrow();
        let font = fonts.pick(bold, italic, mono, family);
        let face = Fonts::face_key(bold, italic, mono, family);
        let ascent = font.horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size);
        let baseline = y + ascent as i32;
        let mut pen = x as f32;
        // One borrow for the whole run instead of three per character.
        let mut cache = self.glyphs.borrow_mut();
        for ch in text.chars() {
            let key = (ch as u32, size.to_bits(), face);
            let (m, cov) = cache.entry(key).or_insert_with(|| font.rasterize(ch, size));
            let gx0 = pen as i32 + m.xmin;
            let gy0 = baseline - m.ymin - m.height as i32;
            pen += m.advance_width + crate::layout::char_spacing(ch, sp);
            // Clip the glyph box against the buffer once; the inner loop then
            // walks a row by offset and never re-tests a bound.
            let (cx0, cx1) = (gx0.max(0), (gx0 + m.width as i32).min(w));
            let (cy0, cy1) = (gy0.max(0), (gy0 + m.height as i32).min(h));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            for py in cy0..cy1 {
                let row = (py - gy0) as usize * m.width + (cx0 - gx0) as usize;
                let mut i = idx(w, cx0, py);
                for gx in 0..(cx1 - cx0) as usize {
                    // Glyph coverage times the colour's own alpha — a
                    // translucent text colour dims the whole run, it does not
                    // sharpen its edges.
                    let a = mul255(cov[row + gx], color.a);
                    if a != 0 {
                        blend_at(out, i, color.c, a);
                    }
                    i += 4;
                }
            }
        }
    }
}

#[inline]
fn idx(w: i32, x: i32, y: i32) -> usize {
    ((y * w + x) * 4) as usize
}

/// Fill a rect by building ONE row and copying it, rather than storing four
/// bytes per pixel.
///
/// This is the hottest loop in the app. A frame clears the canvas and then
/// paints roughly another viewport of backgrounds on top, so about 3.7 M pixels
/// are written per scroll step — and under the wasmi interpreter every one of
/// those byte stores is an interpreted instruction with its own bounds check.
/// `copy_within` compiles to `memory.copy`, a single instruction the host
/// executes as a native memmove, so an N-pixel row costs log2(N) copies to
/// build plus one copy per further row instead of 4·N·rows stores.
fn fill(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, c: Rgba) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + rw).min(w);
    let y1 = (y + rh).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // A translucent fill has to READ each destination pixel, so none of the
    // row-copy trick below applies — every pixel is its own blend. Kept behind
    // this branch rather than folded into the loop so the opaque case, which is
    // the overwhelming majority and the hottest loop in the app, still costs
    // one `memory.copy` per row.
    if !c.is_opaque() {
        for py in y0..y1 {
            let mut i = idx(w, x0, py);
            for _ in x0..x1 {
                blend_at(out, i, c.c, c.a);
                i += 4;
            }
        }
        return;
    }
    let c = c.c;
    let row = ((x1 - x0) * 4) as usize;
    let first = idx(w, x0, y0);
    out[first] = c.2; // B
    out[first + 1] = c.1; // G
    out[first + 2] = c.0; // R
    out[first + 3] = 255; // A
    let mut done = 4;
    while done < row {
        let n = done.min(row - done);
        out.copy_within(first..first + n, first + done);
        done += n;
    }
    for py in (y0 + 1)..y1 {
        let dst = idx(w, x0, py);
        out.copy_within(first..first + row, dst);
    }
}

/// How far a rounded rect's left and right edges move inwards on the row whose
/// top is `row_y` (rect-local), in fractional pixels. Radii are `[tl, tr, br,
/// bl]` (CSS corner order) and are treated as circular — CSS allows an ellipse
/// per corner, we take one radius.
fn round_insets(row_y: f32, rh: f32, r: [f32; 4]) -> (f32, f32) {
    let [tl, tr, br, bl] = r;
    let cy = row_y + 0.5; // sample the row's centre
    let inset = |rad: f32, dy: f32| {
        if rad <= 0.0 || dy <= 0.0 {
            0.0
        } else {
            rad - libm::sqrtf((rad * rad - dy * dy).max(0.0))
        }
    };
    let pick = |top: f32, bot: f32| {
        if cy < top {
            inset(top, top - cy)
        } else if cy > rh - bot {
            inset(bot, cy - (rh - bot))
        } else {
            0.0
        }
    };
    (pick(tl, bl), pick(tr, br))
}

/// Fill one row's horizontal span with fractional ends: the interior is a solid
/// run, the two boundary pixels get partial coverage. That antialiasing is what
/// keeps a 2px corner from looking like a chopped pixel.
fn fill_span(out: &mut [u8], w: i32, h: i32, y: i32, xl: f32, xr: f32, c: Rgba) {
    if y < 0 || y >= h || xr <= xl {
        return;
    }
    let (l, rr) = (libm::floorf(xl), libm::ceilf(xr));
    // Solid interior first, then the two fractional edges over it.
    let (i0, i1) = ((l as i32 + 1).max(0), ((rr as i32) - 1).min(w));
    if i1 > i0 {
        fill(out, w, h, i0, y, i1 - i0, 1, c);
    }
    let mut edge = |px: f32, cov: f32| {
        let xi = px as i32;
        if cov > 0.004 && xi >= 0 && xi < w {
            // Two coverages multiply: how much of the pixel the shape covers,
            // and how opaque the colour itself is.
            let a = (cov.min(1.0) * c.a as f32) as u8;
            blend_at(out, idx(w, xi, y), c.c, a);
        }
    };
    // A span narrower than one pixel covers a single pixel partially.
    if rr - l <= 1.0 {
        edge(l, xr - xl);
        return;
    }
    edge(l, 1.0 - (xl - l));
    edge(rr - 1.0, 1.0 - (rr - xr));
}

/// Die Kachelgroesse eines Verlaufs.
///
/// Ein Verlauf hat KEINE eigene Groesse (css-images-3 §4.3): sein Vorgabemass
/// ist die Positionierflaeche selbst. Damit fallen `auto`, `cover` und
/// `contain` alle auf die Flaeche zurueck, und nur eine ausdrueckliche
/// Groesse macht daraus eine Kachel.
fn grad_tile_size(area: (i32, i32), size: BgSize) -> (i32, i32) {
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    match size {
        BgSize::Fixed(fw, fh) => {
            let tw = fw.and_then(|l| l.px(aw)).unwrap_or(aw);
            let th = fh.and_then(|l| l.px(ah)).unwrap_or(ah);
            ((libm::roundf(tw) as i32).max(1), (libm::roundf(th) as i32).max(1))
        }
        _ => (area.0.max(1), area.1.max(1)),
    }
}

/// Einen Farbverlauf ueber die Positionierflaeche `x,y,gw,gh` malen —
/// gekachelt nach `size`/`pos`/`repeat`, beschnitten auf `cl` und auf die
/// Eckenradien `r`.
///
/// Die Kachelung ist dieselbe wie bei einem Bild und aus demselben Grund:
/// `background-image` ist EINE Eigenschaft, und ein Verlauf steht darin an
/// derselben Stelle wie ein `url()`.
#[allow(clippy::too_many_arguments)]
fn fill_gradient(
    out: &mut [u8],
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    gw: i32,
    gh: i32,
    cl: (i32, i32, i32, i32),
    repeat: (bool, bool),
    pos: (BgPos, BgPos),
    size: BgSize,
    r: [f32; 4],
    g: &Gradient,
) {
    if gw <= 0 || gh <= 0 || g.n < 2 {
        return;
    }
    let (tw, th) = grad_tile_size((gw, gh), size);
    let ox = x + bg_offset(pos.0, gw, tw);
    let oy = y + bg_offset(pos.1, gh, th);
    // Wie in `blit_bg`: wie viele Kacheln zurueck und vor, bis der Malbereich
    // verlassen ist. Eine nicht wiederholte Achse hat genau eine.
    let span = |origin: i32, box_lo: i32, box_hi: i32, tile: i32, rep: bool| -> (i32, i32) {
        if !rep {
            return (0, 0);
        }
        let lo = (box_lo - origin).div_euclid(tile).min(0);
        let hi = (box_hi - origin - 1).div_euclid(tile).max(0);
        (lo, hi)
    };
    let (ix0, ix1) = span(ox, cl.0, cl.0 + cl.2, tw, repeat.0);
    let (iy0, iy1) = span(oy, cl.1, cl.1 + cl.3, th, repeat.1);
    for ty in iy0..=iy1 {
        for tx in ix0..=ix1 {
            fill_gradient_tile(out, w, h, ox + tx * tw, oy + ty * th, tw, th, cl, r, g);
        }
    }
}

/// EINE Kachel des Verlaufs.
///
/// Drei Wege, und der Grund ist die Groesse: ein Seitenhintergrund ist
/// 1902x1000 = 1,9 Mio Pixel, und jedes einzeln zu rechnen kostet mehr als
/// alles andere im Bild zusammen. Ein SENKRECHTER Verlauf hat je Zeile genau
/// eine Farbe — eine Zeile ist ein `memory.copy`. Nur der schraege und der
/// radiale laufen wirklich Pixel fuer Pixel.
#[allow(clippy::too_many_arguments)]
fn fill_gradient_tile(
    out: &mut [u8],
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    gw: i32,
    gh: i32,
    cl: (i32, i32, i32, i32),
    r: [f32; 4],
    g: &Gradient,
) {
    // Sichtbarer Bereich: Kachel ∩ Malbereich ∩ Bild.
    let x0 = x.max(cl.0).max(0);
    let y0 = y.max(cl.1).max(0);
    let x1 = (x + gw).min(cl.0 + cl.2).min(w);
    let y1 = (y + gh).min(cl.1 + cl.3).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (fw, fh) = (gw as f32, gh as f32);
    let (cx, cy) = (x as f32 + fw / 2.0, y as f32 + fh / 2.0);
    let rounded = r.iter().any(|&v| v > 0.0);
    // Die Rundung gehoert dem MALBEREICH, nicht der Kachel: mit einem Rahmen
    // sind das zwei verschiedene Rechtecke, und die Ecke, die der Verlauf
    // nicht ueberlaufen darf, ist die des Malbereichs.
    let span = |py: i32| -> (i32, i32) {
        if !rounded {
            return (x0, x1);
        }
        let (li, ri) = round_insets((py - cl.1) as f32, cl.3 as f32, r);
        let l = libm::ceilf(cl.0 as f32 + li) as i32;
        let rgt = libm::floorf((cl.0 + cl.2) as f32 - ri) as i32;
        (l.max(x0), rgt.min(x1))
    };

    if g.kind == GradKind::Radial {
        // Mitte, `farthest-corner`. Eine Ellipse behaelt das Seitenverhaeltnis
        // des Kastens und geht durch die Ecke — das Wurzel-Zwei-Fache der
        // halben Seiten. Ein Kreis hat EINEN Radius: den Abstand zur Ecke.
        let (rx, ry) = if g.circle {
            let rad = libm::sqrtf(fw * fw + fh * fh) / 2.0;
            (rad, rad)
        } else {
            (fw * 0.7071068, fh * 0.7071068)
        };
        let gr = g.resolved(rx);
        for py in y0..y1 {
            let (sx, ex) = span(py);
            let dy = (py as f32 + 0.5 - cy) / ry.max(0.001);
            for px in sx..ex {
                let dx = (px as f32 + 0.5 - cx) / rx.max(0.001);
                let c = gr.at(libm::sqrtf(dx * dx + dy * dy));
                if c.a > 0 {
                    blend_at(out, idx(w, px, py), c.c, c.a);
                }
            }
        }
        return;
    }

    // CSS zaehlt den Winkel im Uhrzeigersinn ab „nach oben"; die Achse zeigt
    // damit nach `(sin, -cos)` in Bildkoordinaten (y waechst nach unten).
    let rad = g.angle_for(fw, fh) * core::f32::consts::PI / 180.0;
    let (sa, ca) = (libm::sinf(rad), libm::cosf(rad));
    let line = libm::fabsf(fw * sa) + libm::fabsf(fh * ca);
    let gr = g.resolved(line);
    let inv = if line > 0.0 { 1.0 / line } else { 0.0 };
    // `t` an einem Pixel: die Projektion auf die Achse, auf 0..1 normiert.
    let t_at = |px: f32, py: f32| 0.5 + ((px - cx) * sa - (py - cy) * ca) * inv;

    if libm::fabsf(sa) < 0.0005 {
        // Senkrecht: eine Farbe je Zeile.
        for py in y0..y1 {
            let (sx, ex) = span(py);
            if ex <= sx {
                continue;
            }
            let c = gr.at(t_at(0.0, py as f32 + 0.5));
            if c.a > 0 {
                fill(out, w, h, sx, py, ex - sx, 1, c);
            }
        }
        return;
    }

    if libm::fabsf(ca) < 0.0005 {
        // Waagrecht: jede Zeile ist dieselbe Farbfolge. Einmal rechnen, dann
        // nur noch schreiben — das nimmt der heissesten Schleife im Bild die
        // Winkelrechnung und die Stoppsuche je Pixel.
        let mut row: Vec<Rgba> = Vec::with_capacity((x1 - x0) as usize);
        for px in x0..x1 {
            row.push(gr.at(t_at(px as f32 + 0.5, 0.0)));
        }
        for py in y0..y1 {
            let (sx, ex) = span(py);
            let mut i = idx(w, sx, py);
            for px in sx..ex {
                let c = row[(px - x0) as usize];
                if c.a > 0 {
                    blend_at(out, i, c.c, c.a);
                }
                i += 4;
            }
        }
        return;
    }

    for py in y0..y1 {
        let (sx, ex) = span(py);
        let fy = py as f32 + 0.5;
        for px in sx..ex {
            let c = gr.at(t_at(px as f32 + 0.5, fy));
            if c.a > 0 {
                blend_at(out, idx(w, px, py), c.c, c.a);
            }
        }
    }
}

/// Fill a rounded rect, or — when `ring > 0` — only a border of that thickness
/// along its inside edge. Radii are in px, `[tl, tr, br, bl]`.
///
/// A solid fill only walks rows inside the corner bands; everything between
/// them is ONE `fill` call. So a page-tall background with a 2px radius still
/// costs one `memory.copy` per row instead of a per-pixel loop over millions
/// of pixels.
#[allow(clippy::too_many_arguments)]
fn fill_round(out: &mut [u8], w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32, r: [f32; 4], c: Rgba, ring: f32) {
    if rw <= 0 || rh <= 0 {
        return;
    }
    if r.iter().all(|&v| v <= 0.0) && ring <= 0.0 {
        fill(out, w, h, x, y, rw, rh, c);
        return;
    }
    let (fx, fy, fw, fh) = (x as f32, y as f32, rw as f32, rh as f32);
    // Erst jeden Radius auf die Kastenseite deckeln, DANN die Paare summieren.
    //
    // Sonst laeuft die Summe ueber: Tailwind schreibt seine Pille als
    // `border-radius: 3.40282e38px` — und das ist f32::MAX, also ist
    // `r[0] + r[1]` unendlich, `extent / sum` wird 0, und der Faktor unten
    // setzt ALLE Radien auf null. Die Pille kam als Rechteck heraus.
    // Deckeln aendert die Form nicht: ein Radius groesser als die Seite ist
    // ohnehin nicht darstellbar, und die Verhaeltnisse zwischen den Ecken
    // bleiben, weil danach immer noch EIN gemeinsamer Faktor wirkt.
    let cap = fw.max(fh);
    let r = [r[0].min(cap), r[1].min(cap), r[2].min(cap), r[3].min(cap)];
    // A radius may not exceed half the box, and CSS scales ALL of them by one
    // factor when any pair overflows its side (css-backgrounds-3 §5.5) —
    // clamping each corner on its own would change the shape.
    let mut scale = 1.0f32;
    for (sum, extent) in [(r[0] + r[1], fw), (r[3] + r[2], fw), (r[0] + r[3], fh), (r[1] + r[2], fh)] {
        if sum > extent && sum > 0.0 {
            scale = scale.min(extent / sum);
        }
    }
    let r = [r[0] * scale, r[1] * scale, r[2] * scale, r[3] * scale];
    let span = |out: &mut [u8], py: i32| {
        let (li, ri) = round_insets(py as f32 - fy, fh, r);
        fill_span(out, w, h, py, fx + li, fx + fw - ri, c);
    };

    if ring <= 0.0 {
        let top = ceil_f(r[0].max(r[1])).min(rh);
        let bot = ceil_f(r[2].max(r[3])).min(rh - top);
        for py in y..(y + top) {
            span(out, py);
        }
        fill(out, w, h, x, y + top, rw, rh - top - bot, c);
        for py in (y + rh - bot)..(y + rh) {
            span(out, py);
        }
        return;
    }

    // Ring: the hole's radii shrink with the border but never go negative — a
    // border thicker than the radius leaves a square inner corner, as browsers
    // do. Rows above and below the hole are border across their whole span.
    let inner = [
        (r[0] - ring).max(0.0),
        (r[1] - ring).max(0.0),
        (r[2] - ring).max(0.0),
        (r[3] - ring).max(0.0),
    ];
    let (iy0, iy1, ih) = (fy + ring, fy + fh - ring, fh - 2.0 * ring);
    for py in y.max(0)..(y + rh).min(h) {
        let cy = py as f32 + 0.5;
        if cy < iy0 || cy > iy1 || ih <= 0.0 {
            span(out, py);
            continue;
        }
        let (li, ri) = round_insets(py as f32 - fy, fh, r);
        let (ili, iri) = round_insets(cy - iy0 - 0.5, ih, inner);
        fill_span(out, w, h, py, fx + li, fx + ring + ili, c);
        fill_span(out, w, h, py, fx + fw - ring - iri, fx + fw - ri, c);
    }
}

/// `ceil` as an i32 — `core` has no `f32::ceil` in `no_std`.
fn ceil_f(v: f32) -> i32 {
    libm::ceilf(v.max(0.0)) as i32
}

/// Two 0..255 coverages multiplied, rounded — `255 * x == x`, so an opaque
/// colour leaves a coverage untouched and the antialiasing is bit-identical to
/// what it was before alpha existed.
#[inline]
fn mul255(x: u8, y: u8) -> u8 {
    ((x as u32 * y as u32 + 127) / 255) as u8
}

/// Blend `c` at `a`/255 coverage over the pixel starting at byte `i`. Takes the
/// offset rather than (x, y) so the caller can walk a row by adding 4 instead of
/// recomputing `y * w + x` for every pixel it touches.
#[inline]
/// Ein weichgezeichneter Schlagschatten.
///
/// **Die Deckung eines gaussisch weichgezeichneten Rechtecks ist trennbar:**
///
///     a(x,y) = A * S(x; links, rechts) * S(y; oben, unten)
///     S(t; a, b) = Phi((t-a)/sigma) - Phi((t-b)/sigma)
///
/// Das ist keine Naeherung, sondern exakt — die Faltung eines Rechtecks mit
/// einem trennbaren Kern zerfaellt in zwei eindimensionale. Damit kostet ein
/// Pixel EINE Multiplikation statt einer Faltung, und das Waagrechte wird
/// einmal je Schatten gerechnet statt einmal je Zeile.
///
/// `sigma = blur / 2`, wie CSS Backgrounds 3 §7.1.1 es vorschreibt: der
/// Radius spannt zwei Standardabweichungen.
///
/// **Was hier NICHT drin ist:** die Ecken folgen keinem `border-radius`. Bei
/// den Radien, mit denen echte Seiten arbeiten (6 px) und den Weichzeichnungen
/// (16 px) liegt der Unterschied unter der Sichtbarkeitsschwelle; bei einem
/// Kreis waere er sichtbar. Benannt statt still.
#[allow(clippy::too_many_arguments)]
fn fill_shadow(out: &mut [u8], cw: i32, ch: i32, x: i32, y: i32, w: i32, h: i32,
               blur: f32, color: Rgba, keep: (i32, i32, i32, i32)) {
    if w <= 0 || h <= 0 || color.a == 0 {
        return;
    }
    let sigma = (blur * 0.5).max(0.01);
    // Jenseits von drei Sigma ist die Deckung unter einem halben Prozent.
    let pad = libm::ceilf(sigma * 3.0) as i32;
    let (x0, y0) = ((x - pad).max(0), (y - pad).max(0));
    let (x1, y1) = ((x + w + pad).min(cw), (y + h + pad).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // Das waagrechte Profil einmal, nicht je Zeile.
    let mut fx: alloc::vec::Vec<f32> = alloc::vec::Vec::with_capacity((x1 - x0) as usize);
    for px in x0..x1 {
        fx.push(span(px as f32 + 0.5, x as f32, (x + w) as f32, sigma));
    }
    for py in y0..y1 {
        let fy = span(py as f32 + 0.5, y as f32, (y + h) as f32, sigma);
        if fy <= 0.002 {
            continue;
        }
        let inside_y = py >= keep.1 && py < keep.1 + keep.3;
        let row = (py * cw) as usize * 4;
        for (k, px) in (x0..x1).enumerate() {
            // Ein AEUSSERER Schatten wird nicht in den RAHMENkasten gemalt
            // (CSS Backgrounds 3 §7.1.1). Unter einem deckenden Kasten faellt
            // das nicht auf; unter einem durchsichtigen ist es der Ring, den
            // ein Browser dort auch zeigt.
            //
            // Ausgespart wird der RAHMENkasten, nicht das Schattenrechteck.
            // Dieselben sind die beiden nur ohne Versatz und ohne Spread —
            // Tailwinds `shadow-lg` hat beides (`0 10px 15px -3px`), und die
            // falsche Aussparung schnitt einen weissen Zapfen genau in den
            // Streifen unter dem Kasten, wo der Schatten am dunkelsten ist.
            if inside_y && px >= keep.0 && px < keep.0 + keep.2 {
                continue;
            }
            let a = fx[k] * fy * (color.a as f32);
            if a < 0.5 {
                continue;
            }
            blend_at(out, row + px as usize * 4, color.c, a as u8);
        }
    }
}

/// Wieviel einer gaussisch verschmierten Kante liegt bei `t` noch im Band
/// `[a, b]`? `Phi` ueber die Fehlerfunktion — `libm` hat sie, also braucht es
/// hier keine Naeherung, die man spaeter erklaeren muss.
fn span(t: f32, a: f32, b: f32, sigma: f32) -> f32 {
    let k = 1.0 / (sigma * core::f32::consts::SQRT_2);
    let phi = |z: f32| 0.5 * (1.0 + libm::erff(z));
    (phi((t - a) * k) - phi((t - b) * k)).clamp(0.0, 1.0)
}

fn blend_at(out: &mut [u8], i: usize, c: Rgb, a: u8) {
    if a == 255 {
        out[i] = c.2;
        out[i + 1] = c.1;
        out[i + 2] = c.0;
        out[i + 3] = 255;
        return;
    }
    let a = a as u32;
    let ia = 255 - a;
    out[i] = ((c.2 as u32 * a + out[i] as u32 * ia) / 255) as u8; // B
    out[i + 1] = ((c.1 as u32 * a + out[i + 1] as u32 * ia) / 255) as u8; // G
    out[i + 2] = ((c.0 as u32 * a + out[i + 2] as u32 * ia) / 255) as u8; // R
    out[i + 3] = 255;
}

/// Nearest-neighbour scale a decoded `img` (BGRA) into a `dw`×`dh` box at
/// (dx, dy), alpha-blending over `out`. Clipped to the buffer.
/// Resolve `background-size` against the positioning area (css-backgrounds-3
/// §3.9). `auto` on one axis keeps the intrinsic aspect ratio.
fn bg_tile_size(area: (i32, i32), img: (u32, u32), size: BgSize) -> (i32, i32) {
    let (aw, ah) = (area.0 as f32, area.1 as f32);
    let (iw, ih) = (img.0 as f32, img.1 as f32);
    let ratio = iw / ih;
    let (tw, th) = match size {
        BgSize::Auto => (iw, ih),
        BgSize::Cover | BgSize::Contain => {
            let s = if matches!(size, BgSize::Cover) {
                (aw / iw).max(ah / ih)
            } else {
                (aw / iw).min(ah / ih)
            };
            (iw * s, ih * s)
        }
        BgSize::Fixed(fw, fh) => {
            let rw = fw.and_then(|l| l.px(aw));
            let rh = fh.and_then(|l| l.px(ah));
            match (rw, rh) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, a / ratio),
                (None, Some(b)) => (b * ratio, b),
                (None, None) => (iw, ih),
            }
        }
    };
    ((libm::roundf(tw) as i32).max(1), (libm::roundf(th) as i32).max(1))
}

fn bg_offset(p: BgPos, area: i32, tile: i32) -> i32 {
    match p {
        BgPos::Px(v) => libm::roundf(v) as i32,
        BgPos::Pct(f) => libm::roundf((area - tile) as f32 * f) as i32,
    }
}

/// Paint one `background-image`/`mask-image` layer into the box `dx,dy,dw,dh`.
/// Everything is clipped to that box; repeating axes tile outward from the
/// positioned origin.
#[allow(clippy::too_many_arguments)]
fn blit_bg(
    out: &mut [u8],
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    // The painting area (`background-clip`). `d*` above is the POSITIONING
    // area, which is what the tile grid is anchored to and measured against.
    clip: (i32, i32, i32, i32),
    img: &crate::image::Image,
    repeat: (bool, bool),
    pos: (BgPos, BgPos),
    size: BgSize,
    tint: Option<crate::layout::Rgba>,
    filter: Option<ColorFilter>,
) {
    if dw <= 0 || dh <= 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let (tw, th) = bg_tile_size((dw, dh), (img.w, img.h), size);
    let ox = dx + bg_offset(pos.0, dw, tw);
    let oy = dy + bg_offset(pos.1, dh, th);
    // Tile range: how many steps back from the origin before leaving the box,
    // and how many forward. A non-repeating axis is the single tile.
    let span = |origin: i32, box_lo: i32, box_hi: i32, tile: i32, rep: bool| -> (i32, i32) {
        if !rep {
            return (0, 0);
        }
        let lo = (box_lo - origin).div_euclid(tile).min(0);
        let hi = (box_hi - origin - 1).div_euclid(tile).max(0);
        (lo, hi)
    };
    let (ix0, ix1) = span(ox, clip.0, clip.0 + clip.2, tw, repeat.0);
    let (iy0, iy1) = span(oy, clip.1, clip.1 + clip.3, th, repeat.1);
    // Clip to the painting area AND to the surface in one rect, so the inner
    // loop never tests bounds per pixel.
    let (cx0, cx1) = (clip.0.max(0), (clip.0 + clip.2).min(w));
    let (cy0, cy1) = (clip.1.max(0), (clip.1 + clip.3).min(h));
    if cx1 <= cx0 || cy1 <= cy0 {
        return;
    }
    for ty in iy0..=iy1 {
        let ty0 = oy + ty * th;
        let (y0, y1) = (ty0.max(cy0), (ty0 + th).min(cy1));
        if y1 <= y0 {
            continue;
        }
        for tx in ix0..=ix1 {
            let tx0 = ox + tx * tw;
            let (x0, x1) = (tx0.max(cx0), (tx0 + tw).min(cx1));
            if x1 <= x0 {
                continue;
            }
            // Source column per destination column, resolved once per tile
            // rather than a multiply+divide per pixel (the interpreter charges
            // ~150× for a per-pixel loop — see the wasmi hot-loop note).
            let cols: Vec<usize> = (x0..x1)
                .map(|px| ((px - tx0) * img.w as i32 / tw).clamp(0, img.w as i32 - 1) as usize * 4)
                .collect();
            for py in y0..y1 {
                let sy = ((py - ty0) * img.h as i32 / th).clamp(0, img.h as i32 - 1);
                let srow = (sy * img.w as i32) as usize * 4;
                let mut di = idx(w, x0, py);
                for &sx in &cols {
                    let si = srow + sx;
                    // A translucent tint multiplies into the mask's own
                    // alpha, so a 50 %-opaque tint through a solid stencil is
                    // half-covered rather than solid.
                    let a = match tint {
                        Some(c) => mul255(img.bgra[si + 3], c.a) as u32,
                        None => img.bgra[si + 3] as u32,
                    };
                    if a != 0 {
                        // A mask takes only the alpha and paints the tint
                        // through it; a background image paints its own pixels.
                        let src = match (tint, filter) {
                            // A mask's tint was already filtered at layout.
                            (Some(c), _) => [c.c.2, c.c.1, c.c.0],
                            (None, None) => [img.bgra[si], img.bgra[si + 1], img.bgra[si + 2]],
                            (None, Some(f)) => {
                                let p = f.apply_bgra([img.bgra[si], img.bgra[si + 1], img.bgra[si + 2], img.bgra[si + 3]]);
                                [p[0], p[1], p[2]]
                            }
                        };
                        if a == 255 {
                            out[di] = src[0];
                            out[di + 1] = src[1];
                            out[di + 2] = src[2];
                            out[di + 3] = 255;
                        } else {
                            let ia = 255 - a;
                            for c in 0..3 {
                                out[di + c] = ((src[c] as u32 * a + out[di + c] as u32 * ia) / 255) as u8;
                            }
                            out[di + 3] = 255;
                        }
                    }
                    di += 4;
                }
            }
        }
    }
}

/// Where a replaced element's pixels land inside the content box the layout
/// gave it — `object-fit` (css-images-3 §5.5). Returns the rectangle the
/// picture is DRAWN into, which for `cover`/`none` may be larger than the box
/// (the caller clips) and for `contain`/`scale-down` smaller (it letterboxes).
///
/// `object-position` is not implemented; the picture is centred, which is that
/// property's initial value (`50% 50%`) and what every use of `object-fit` on
/// the two vendored sheets asks for.
fn object_rect(fit: ObjectFit, dx: i32, dy: i32, dw: i32, dh: i32, iw: i32, ih: i32) -> (i32, i32, i32, i32) {
    let (bw, bh, sw, sh) = (dw as f32, dh as f32, iw as f32, ih as f32);
    let scale = match fit {
        // The initial value: stretch to the box on both axes, aspect ignored.
        ObjectFit::Fill => return (dx, dy, dw, dh),
        ObjectFit::Contain => (bw / sw).min(bh / sh),
        ObjectFit::Cover => (bw / sw).max(bh / sh),
        ObjectFit::None => 1.0,
        // `scale-down` is `none` and `contain`, whichever comes out smaller —
        // an image that already fits keeps its own size instead of growing.
        ObjectFit::ScaleDown => (bw / sw).min(bh / sh).min(1.0),
    };
    let (rw, rh) = ((sw * scale + 0.5).max(1.0) as i32, (sh * scale + 0.5).max(1.0) as i32);
    (dx + (dw - rw) / 2, dy + (dh - rh) / 2, rw, rh)
}

/// An image op's `filter` index resolved against the layout's side table.
/// 0 is "none", so the unfiltered path never touches the table at all.
fn filt(layout: &Layout, idx: u16) -> Option<ColorFilter> {
    layout.filters.get((idx as usize).checked_sub(1)?).copied()
}

#[allow(clippy::too_many_arguments)]
fn blit_image(out: &mut [u8], w: i32, h: i32, dx: i32, dy: i32, dw: i32, dh: i32, img: &crate::image::Image, fit: ObjectFit, filter: Option<ColorFilter>) {
    if dw <= 0 || dh <= 0 || img.w == 0 || img.h == 0 {
        return;
    }
    let (iw, ih) = (img.w as i32, img.h as i32);
    // Two rectangles once `object-fit` is not `fill`: the picture is scaled
    // into `p*` and painted only where that meets the box, so `cover` crops
    // instead of overflowing and `contain` leaves the rest of the box alone.
    let (ox, oy, ow, oh) = object_rect(fit, dx, dy, dw, dh, iw, ih);
    let x0 = ox.max(dx).max(0);
    let x1 = (ox + ow).min(dx + dw).min(w);
    let y0 = oy.max(dy).max(0);
    let y1 = (oy + oh).min(dy + dh).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    // The source column for each destination column, resolved once for the
    // whole blit instead of a multiply, divide and clamp per pixel.
    let cols: Vec<usize> = (x0..x1).map(|px| ((px - ox) * iw / ow).clamp(0, iw - 1) as usize * 4).collect();
    for py in y0..y1 {
        let sy = ((py - oy) * ih / oh).clamp(0, ih - 1);
        let srow = (sy * iw) as usize * 4;
        let mut di = idx(w, x0, py);
        for &sx in &cols {
            let si = srow + sx;
            // `filter` recolours the SOURCE pixel. Read through a local copy
            // only when there is one, so the unfiltered blit keeps indexing
            // straight into the decoded buffer.
            let px = match filter {
                None => [img.bgra[si], img.bgra[si + 1], img.bgra[si + 2], img.bgra[si + 3]],
                Some(f) => f.apply_bgra([img.bgra[si], img.bgra[si + 1], img.bgra[si + 2], img.bgra[si + 3]]),
            };
            let a = px[3];
            if a == 255 {
                out[di..di + 4].copy_from_slice(&px);
            } else if a != 0 {
                let (a, ia) = (a as u32, 255 - a as u32);
                out[di] = ((px[0] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                out[di + 1] = ((px[1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                out[di + 2] = ((px[2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
                out[di + 3] = 255;
            }
            di += 4;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rgb, Theme};

    /// A decodable image in plain bytes — SVG is one of the formats
    /// `image::decode` accepts, so a test needs no binary fixture.
    const RED_10: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#;

    #[test]
    fn a_band_paints_exactly_what_a_full_frame_would() {
        // The claim behind the cheap scroll: painting rows y0..y1 into a slice
        // produces the same bytes a whole frame would have in those rows.
        // Deliberately with boxes and text that STRADDLE the band edges — an
        // op crossing a boundary is where a missing clip would show.
        let eng = Engine::new();
        let html = "<body style=\"background:#123\">\
            <div style=\"height:80px;background:#c00\">Erste Zeile mit Text</div>\
            <p style=\"margin:0;padding:20px\">Ein Absatz, der ueber mehrere Zeilen laeuft \
             und dabei genau die Kante zwischen zwei Streifen kreuzt.</p>\
            <div style=\"height:400px;background:#0a0;border:6px solid #00f\">tief unten</div>\
            </body>";
        let lay = eng.layout(html, 400);
        let (w, h) = (400u32, 300u32);
        let px = (w * h * 4) as usize;

        let mut full = alloc::vec![0u8; px];
        eng.paint(&lay, w, h, 40, &mut full);

        // Bands chosen to cut through content, not between elements.
        let mut banded = alloc::vec![0u8; px];
        for (y0, y1) in [(0u32, 37u32), (37, 111), (111, 298), (298, 300)] {
            eng.paint_band(&lay, w, h, 40, &mut banded, y0, y1);
        }
        assert!(full == banded, "a band must be byte-identical to the full frame");

        // And a band outside the viewport is a no-op rather than a panic.
        let before = banded.clone();
        eng.paint_band(&lay, w, h, 40, &mut banded, 400, 500);
        eng.paint_band(&lay, w, h, 40, &mut banded, 200, 200);
        assert!(before == banded, "an empty or out-of-range band draws nothing");
    }

    #[test]
    fn a_document_survives_a_visit_to_another_page() {
        let eng = Engine::new();
        let (a, b) = ("<body><p>page A</p></body>", "<body><p>page B</p></body>");
        eng.layout(a, 800);
        eng.layout(b, 800);
        assert_eq!(eng.parse_counts().0, 2, "two pages, two parses");
        eng.layout(a, 800); // back
        eng.layout(b, 800); // forward
        assert_eq!(eng.parse_counts().0, 2, "back and forward re-parse nothing");
        assert_eq!(eng.parse_counts().1, 2, "and neither re-collects a sheet");
    }

    #[test]
    fn the_oldest_document_is_the_one_that_goes() {
        // DOC_SLOTS = 3. A fourth page pushes the least recently used out —
        // and the two still held stay free.
        let eng = Engine::new();
        let pages = ["<p>1</p>", "<p>2</p>", "<p>3</p>", "<p>4</p>"];
        for p in pages {
            eng.layout(p, 800);
        }
        assert_eq!(eng.parse_counts().0, 4);
        eng.layout(pages[3], 800);
        eng.layout(pages[2], 800);
        assert_eq!(eng.parse_counts().0, 4, "the two newest are still held");
        eng.layout(pages[0], 800);
        assert_eq!(eng.parse_counts().0, 5, "the oldest had to go");
    }

    #[test]
    fn the_image_cache_is_keyed_by_url_not_by_src() {
        let mut eng = Engine::new();
        assert!(eng.add_image_cached("logo.png", "https://a.example/logo.png", RED_10));
        eng.images_begin(); // a navigation: the page map goes, the cache stays

        // The SAME src string on ANOTHER host is another picture. Serving it
        // from the cache would put one site's image on another site's page.
        let miss = eng.adopt_cached(&[(
            alloc::string::String::from("logo.png"),
            alloc::string::String::from("https://b.example/logo.png"),
        )]);
        assert!(miss.is_empty(), "same src, other host -> miss");

        // The same URL under a different src is the same picture.
        let hit = eng.adopt_cached(&[(
            alloc::string::String::from("assets/l.png"),
            alloc::string::String::from("https://a.example/logo.png"),
        )]);
        assert_eq!(hit.len(), 1, "same url -> hit");
        assert_eq!(hit[0], "assets/l.png");
    }

    #[test]
    fn the_css_image_cache_is_keyed_by_url_too() {
        let eng = Engine::new();
        // `url_key` 7 is whatever site A's sheet hashed `url(/bg.png)` to.
        assert!(eng.add_css_image_cached(7, "https://a.example/bg.png", RED_10));
        eng.css_images_begin(); // a navigation

        // Another site writing the SAME `url(/bg.png)` hashes to the same key
        // and is a different picture. The key alone would have served it.
        assert!(!eng.adopt_css_cached(7, "https://b.example/bg.png"),
            "same url_key, other host -> miss");
        // The same picture under another key (a sheet that wrote it absolute).
        assert!(eng.adopt_css_cached(99, "https://a.example/bg.png"),
            "same url -> hit");
    }

    #[test]
    fn an_adopted_image_makes_the_box_definite() {
        // The point of adopting BEFORE the first layout: with the pixels
        // already here the box is not guessed, so the arriving image never
        // moves the page — which is the full re-layout that cost 1110-1710 ms
        // on the device.
        let mut eng = Engine::new();
        eng.add_image_cached("/x.svg", "https://a.example/x.svg", RED_10);
        eng.images_begin();
        eng.adopt_cached(&[(
            alloc::string::String::from("/x.svg"),
            alloc::string::String::from("https://a.example/x.svg"),
        )]);
        let l = eng.layout("<body><img src=\"/x.svg\"></body>", 800);
        assert!(l.guessed_image_srcs.is_empty(), "cached pixels -> definite box, no re-layout");

        // Without the cache the same markup has to guess.
        let mut cold = Engine::new();
        let l2 = cold.layout("<body><img src=\"/x.svg\"></body>", 800);
        assert_eq!(l2.guessed_image_srcs.len(), 1, "no pixels -> guessed box");
    }

    /// An inline SVG `data:` URI, quote-safe: the SVG's own attribute quotes
    /// are percent-encoded, so the URI survives being nested inside a CSS
    /// string inside an HTML attribute (which is how real pages ship them).
    const MASK_LEFT_HALF: &str = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
        %20width=%2220%22%20height=%2220%22%20viewBox=%220%200%2020%2020%22%3E\
        %3Cpath%20d=%22M0%200%20H10%20V20%20H0%20Z%22%20fill=%22%23000%22/%3E%3C/svg%3E";

    fn light() -> Theme {
        Theme {
            bg: Rgb(255, 255, 255),
            text: Rgb(0, 0, 0),
            heading: Rgb(0, 0, 0),
            link: Rgb(0, 0, 238),
            muted: Rgb(96, 96, 96),
            rule: Rgb(128, 128, 128),
        }
    }

    const PAD: u32 = 8; // the UA `body { margin }` — where page content starts

    /// Paint one page and read a pixel back as (r, g, b). `x`/`y` are relative
    /// to the document's top-left content corner, i.e. past the page padding.
    fn pixel_at(html: &str, x: u32, y: u32) -> (u8, u8, u8) {
        page(html, 40, 40)(x, y)
    }

    /// `pixel_at` over a content box of a given size — inline content needs a
    /// line's worth of width. Returns a reader so one paint answers many probes.
    fn page(html: &str, cw: u32, ch: u32) -> impl Fn(u32, u32) -> (u8, u8, u8) {
        let (w, h) = (PAD * 2 + cw, PAD * 2 + ch);
        let mut eng = Engine::new();
        eng.set_theme(light());
        let lay = eng.layout(html, w);
        let mut buf = alloc::vec![0u8; (w * h * 4) as usize];
        eng.paint(&lay, w, h, 0, &mut buf);
        move |x: u32, y: u32| {
            let i = (((y + PAD) * w + x + PAD) * 4) as usize;
            (buf[i + 2], buf[i + 1], buf[i])
        }
    }

    /// A 4x1 PNG — red, green, blue, yellow — as a `data:` URI, so a test can
    /// say where the picture landed without a fetch. Four columns and one row
    /// make the aspect ratio (4:1) unmistakable against a square box.
    const STRIPES_4X1: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAABCAIAAAB2Xpia\
        AAAAEklEQVR42mP4z8DAAMb//zMAABzwBPxjoz6tAAAAAElFTkSuQmCC";

    fn stripes(fit: &str) -> alloc::string::String {
        alloc::format!(
            "<img src='{STRIPES_4X1}' style='display:block;width:20px;height:20px;object-fit:{fit}'>"
        )
    }

    /// `object-fit` decides how a replaced element's pixels fill the box the
    /// layout gave it — the box itself is 20x20 in every one of these.
    #[test]
    fn object_fit_places_the_picture_inside_the_box() {
        // `fill` is the initial value: stretched to the box, aspect ignored,
        // so the four source columns become four 5px stripes.
        let html = stripes("fill");
        let p = page(&html, 40, 40);
        assert_eq!(p(2, 10), (255, 0, 0), "fill: stretched, first stripe at the left edge");
        assert_eq!(p(17, 10), (255, 255, 0), "fill: fourth stripe at the right edge");
        assert_eq!(p(2, 2), (255, 0, 0), "fill: the box is full top to bottom");

        // `contain`: scaled down to 20x5 and centred — letterboxed above/below.
        let html = stripes("contain");
        let p = page(&html, 40, 40);
        assert_eq!(p(2, 10), (255, 0, 0), "contain: full width, all four stripes");
        assert_eq!(p(17, 10), (255, 255, 0));
        assert_eq!(p(2, 2), (255, 255, 255), "contain: letterbox above the picture");

        // `cover`: scaled UP to 80x20 and cropped to the box — only the two
        // middle stripes survive, and nothing spills outside the box.
        let html = stripes("cover");
        let p = page(&html, 40, 40);
        assert_eq!(p(4, 10), (0, 255, 0), "cover: cropped to the middle two stripes");
        assert_eq!(p(16, 10), (0, 0, 255));
        assert_eq!(p(2, 2), (0, 255, 0), "cover: fills the box top to bottom");
        assert_eq!(p(25, 10), (255, 255, 255), "cover: crops, never overflows the box");

        // `none`: the intrinsic 4x1, centred. `scale-down` is the smaller of
        // that and `contain`, which here is the same 4x1.
        for fit in ["none", "scale-down"] {
            let html = stripes(fit);
            let p = page(&html, 40, 40);
            assert_eq!(p(8, 9), (255, 0, 0), "{fit}: intrinsic size, centred");
            assert_eq!(p(11, 9), (255, 255, 0));
            assert_eq!(p(2, 10), (255, 255, 255), "{fit}: nothing outside the 4x1");
        }
    }

    /// Bootstrap ships `object-fit` only behind Opera's prefix in its utility
    /// classes, so the unprefixed name alone would leave `.object-fit-cover`
    /// doing nothing on a real page.
    #[test]
    fn the_opera_prefix_is_the_same_property() {
        let html = alloc::format!(
            "<img src='{STRIPES_4X1}' style='display:block;width:20px;height:20px;\
             -o-object-fit:cover'>"
        );
        let p = page(&html, 40, 40);
        assert_eq!(p(4, 10), (0, 255, 0), "-o-object-fit: cover crops like the plain name");
    }

    /// A mask paints the element's own background-colour through the image's
    /// alpha — it does NOT paint the image. This SVG is opaque on its left half
    /// only, so the box must be red on the left and untouched on the right.
    #[test]
    fn mask_image_stencils_the_background_colour() {
        // A `data:` URI needs no fetch: the engine decodes it during layout.
        let html = alloc::format!(
            "<div style=\"width:20px;height:20px;background-color:#ff0000;\
             mask-image:url('{MASK_LEFT_HALF}');mask-size:contain;mask-repeat:no-repeat\"></div>"
        );
        assert_eq!(pixel_at(&html, 4, 10), (255, 0, 0), "left half is stencilled red");
        assert_eq!(pixel_at(&html, 16, 10), (255, 255, 255), "right half stays clear");
    }

    /// An `outline` is drawn OUTSIDE the border box and takes no space — that
    /// is the whole reason the property exists separately from `border`: a
    /// focus ring has to appear without moving the page under the reader.
    #[test]
    fn an_outline_rings_the_box_from_outside_and_moves_nothing() {
        let boxed = "<div style='width:20px;height:20px;background:#0000ff'></div>";
        let ringed = "<div style='width:20px;height:20px;background:#0000ff;\
                      outline:2px solid #ff0000'></div>";

        // The box itself is untouched, and so is everything after it.
        assert_eq!(pixel_at(boxed, 10, 10), (0, 0, 255));
        assert_eq!(pixel_at(ringed, 10, 10), (0, 0, 255), "the box keeps its own paint");

        // The ring sits in the two pixels OUTSIDE the border box.
        assert_eq!(pixel_at(ringed, 10, 25), (255, 255, 255), "no ring below at rest…");
        let off = "<div style='width:20px;height:20px;background:#0000ff;\
                   outline:2px solid #ff0000;outline-offset:4px'></div>";
        assert_eq!(pixel_at(off, 10, 25), (255, 0, 0), "…but there once offset pushes it out");

        // It takes no space: a following box sits at exactly the same y.
        let after = |css: &str| {
            let html = alloc::format!(
                "<div style='width:20px;height:20px;{css}'></div>\
                 <div style='width:20px;height:6px;background:#00ff00'></div>"
            );
            let p = page(&html, 40, 40);
            (0..40).find(|&y| p(2, y) == (0, 255, 0))
        };
        assert_eq!(after(""), after("outline:3px solid #f00"), "an outline must not shift the flow");
    }

    /// `:hover` all the way to a pixel: lay out once to learn the geometry, ask
    /// the layout what the pointer is inside, hand that back to the engine, lay
    /// out again — which is exactly the loop the shell runs on `MouseMove`.
    ///
    /// A cascade test would pass on a rule that resolves and never reaches the
    /// screen; only the pixel says the feature works.
    #[test]
    fn hover_repaints_the_element_under_the_pointer() {
        let html = "<style>div:hover{background:#ff0000}</style>\
                    <div style='width:20px;height:20px'></div>";
        let (w, h) = (PAD * 2 + 40, PAD * 2 + 40);
        let mut eng = Engine::new();
        eng.set_theme(light());

        let rest = eng.layout(html, w);
        let probe = |lay: &Layout| {
            let mut buf = alloc::vec![0u8; (w * h * 4) as usize];
            eng.paint(lay, w, h, 0, &mut buf);
            let i = (((10 + PAD) * w + 10 + PAD) * 4) as usize;
            (buf[i + 2], buf[i + 1], buf[i])
        };
        assert_eq!(probe(&rest), (255, 255, 255), "at rest the div is unpainted");

        // The box is at the content origin; probe its middle in document space.
        let hovered = rest.hover_at((PAD + 10) as i32, (PAD + 10) as i32);
        assert!(!hovered.is_empty(), "the pointer is inside the div");
        assert!(eng.set_hover(hovered.clone()).is_changed(), "a first hover is a change");
        assert!(!eng.set_hover(hovered).is_changed(), "the same list twice is not — no relayout");

        assert_eq!(probe(&eng.layout(html, w)), (255, 0, 0), "hovering paints it red");

        // Leaving the element takes the colour away again.
        assert!(eng.set_hover(alloc::vec![]).is_changed());
        assert_eq!(probe(&eng.layout(html, w)), (255, 255, 255));
    }

    /// The whole point: a pointer change that only recolours is answered by
    /// PATCHING the display list, and the result has to be indistinguishable
    /// from having laid the page out again.
    ///
    /// Measured on Wikipedia's Main_Page: 0.16 ms against 24 ms, and 55 of the
    /// 64 hover targets on the page take this path — the rest fall back, which
    /// is what every guard in `repaint_hover` exists to do.
    #[test]
    fn a_repaint_answers_a_hover_exactly_as_a_layout_would() {
        let html = "<style>a{color:#0000ee}a:hover{color:#ff0000;text-decoration:underline}</style>\
                    <p>text <a href=\"/x\">link</a> more</p>";
        let w = 400;
        let eng = Engine::new();
        eng.set_hover(alloc::vec![]);
        let mut base = eng.layout(html, w);
        let hovered = base
            .hover_boxes
            .first()
            .map(|b| base.hover_at(b.x + b.w / 2, b.y + b.h / 2))
            .expect("the link is hit-testable");
        assert!(!hovered.is_empty());

        let change = eng.set_hover(hovered);
        assert_eq!(change, HoverChange::Changed { paint_only: true }, "colour + underline move nothing");
        let full = eng.layout(html, w);
        assert!(eng.repaint_hover(&mut base), "the patch has to be possible here");
        assert_eq!(dump_ops(&base), dump_ops(&full), "patched vs laid out");
        // …and it really did something.
        assert!(dump_ops(&full).contains("Rgb(255, 0, 0), a: 255"), "the link is red now");
    }

    /// A background that only exists under the pointer has nothing to replace
    /// — it has to be INSERTED, and where it goes is the box's own insertion
    /// point. The finished display list no longer holds an index for that, so
    /// each hit rect carries the op that sits there, by content.
    ///
    /// The rect it is painted at is not the hit rect: an inline box's
    /// background covers its font's ascent + descent, not the line box, and
    /// getting that wrong makes the patch one pixel taller than the layout.
    #[test]
    fn a_background_that_appears_under_the_pointer_is_inserted_in_the_right_place() {
        for body in [
            // inline: painted at the font's box, and split across lines draws
            // only the outer borders
            "<p>text <a href=\"/x\">link</a> more</p>",
            // block: painted at the border box
            "<a href=\"/x\" style=\"display:block;width:60px;height:20px\">link</a>",
        ] {
            // Only paint-class properties: a `border-left` would ADVANCE the
            // inline flow, which is a layout and `set_hover` says so.
            let html = alloc::format!(
                "<style>a{{color:#00e}}a:hover{{background:#f00;border-bottom-color:#0f0}}</style>{body}"
            );
            let eng = Engine::new();
            eng.set_hover(alloc::vec![]);
            let mut base = eng.layout(&html, 400);
            let b = base.hover_boxes.first().copied().expect("hit-testable");
            let hovered = base.hover_at(b.x + b.w / 2, b.y + b.h / 2);
            eng.set_hover(hovered);
            let full = eng.layout(&html, 400);
            assert!(eng.repaint_hover(&mut base), "{body}: {}", eng.repaint_bail());
            assert_eq!(dump_ops(&base), dump_ops(&full), "{body}");
            assert!(dump_ops(&full).contains("Rgb(255, 0, 0), a: 255"), "{body}: the background is there");
        }
    }

    /// A hover rule that can MOVE something is not a repaint, and the engine
    /// has to say so before anyone tries.
    #[test]
    fn a_hover_that_moves_a_box_is_not_a_repaint() {
        let probe = |rule: &str| {
            let html = alloc::format!("<style>{rule}</style><p><a href=\"/x\">link</a></p>");
            let eng = Engine::new();
            eng.set_hover(alloc::vec![]);
            let lay = eng.layout(&html, 400);
            let b = lay.hover_boxes.first().copied().expect("hit-testable");
            eng.set_hover(lay.hover_at(b.x + b.w / 2, b.y + b.h / 2))
        };
        assert_eq!(probe("a:hover{color:#f00}"), HoverChange::Changed { paint_only: true });
        assert_eq!(probe("a:hover{padding-left:20px}"), HoverChange::Changed { paint_only: false });
        // A property we do not implement cannot move anything — and MediaWiki
        // writes `cursor:pointer` into a third of its hover rules.
        assert_eq!(probe("a:hover{cursor:pointer;color:#f00}"), HoverChange::Changed { paint_only: true });
        // …but one layout property in the same rule is enough.
        assert_eq!(probe("a:hover{cursor:pointer;display:block}"), HoverChange::Changed { paint_only: false });
    }

    /// The guards. Each of these is a real page shape that a colour patch
    /// cannot reproduce, and each has to end in "lay it out" rather than in a
    /// wrong picture.
    #[test]
    fn a_repaint_gives_up_rather_than_get_it_wrong() {
        let gives_up = |rule: &str, body: &str| {
            let html = alloc::format!("<style>a{{color:#00e}}{rule}</style>{body}");
            let eng = Engine::new();
            eng.set_hover(alloc::vec![]);
            let mut base = eng.layout(&html, 400);
            let b = base.hover_boxes.first().copied().expect("hit-testable");
            let hovered = base.hover_at(b.x + b.w / 2, b.y + b.h / 2);
            assert!(!hovered.is_empty());
            eng.set_hover(hovered);
            let full = eng.layout(&html, 400);
            let ok = eng.repaint_hover(&mut base);
            // Whatever it decided, it must never leave a page that disagrees
            // with what a layout would have produced.
            if ok {
                assert_eq!(dump_ops(&base), dump_ops(&full), "{rule}: patched but wrong");
            }
            !ok
        };
        // A pseudo-element with TEXT of its own: `content` is not part of what
        // the element says, so its run cannot be identified.
        assert!(gives_up(
            "a:hover::after{content:'x';color:#f00}",
            "<p><a href=\"/x\">link</a></p>"
        ));
        // A rule that reaches sideways restyles something outside the subtree.
        assert!(gives_up(
            "li:hover + li a{color:#f00}",
            "<ul><li><a href=\"/x\">one</a></li><li><a href=\"/y\">two</a></li></ul>"
        ));
    }

    /// Ein aeusserer Schatten wird aus dem RAHMENkasten ausgespart, nicht aus
    /// seinem eigenen Rechteck. Bei `0 10px 15px -3px` sind das zwei
    /// verschiedene Rechtecke, und die falsche Aussparung liess genau den
    /// Streifen unter dem Kasten weiss — dort, wo der Schatten am
    /// dunkelsten ist.
    #[test]
    fn an_offset_shadow_is_cut_out_of_the_border_box_not_of_itself() {
        let html = "<div style='margin:20px;width:60px;height:40px;background:#fff;\
                    box-shadow:0 10px 15px -3px rgba(0,0,0,.4)'></div>";
        let p = page(html, 140, 90);
        // Der Kasten selbst bleibt weiss.
        assert_eq!(p(50, 40), (255, 255, 255), "im Kasten kein Schatten");
        // Direkt darunter steht der dunkelste Teil.
        let under = p(50, 62).0;
        assert!(under < 235, "unter dem Kasten muss es dunkel sein, ist {under}");
        // Und er wird nach unten hin heller.
        assert!(p(50, 75).0 > under, "der Schatten laeuft aus");
    }

    /// `filter` recolours the element AND its whole subtree — the box's own
    /// background, the text inside it, and an image's pixels, which are only
    /// looked up at paint time and so travel as an index instead of a colour.
    #[test]
    fn filter_recolours_the_box_and_its_subtree() {
        // `invert(100%)` on yellow is blue — the css-color reftest's case.
        let inv = "<div style='width:20px;height:20px;background:#ffff00;filter:invert(100%)'></div>";
        assert_eq!(pixel_at(inv, 10, 10), (0, 0, 255));

        // …and it reaches a descendant's background, which a page cannot
        // cancel from inside the subtree.
        let nested = "<div style='filter:invert(100%)'>\
                      <div style='width:20px;height:20px;background:#ffff00'></div></div>";
        assert_eq!(pixel_at(nested, 10, 10), (0, 0, 255));

        // An image's pixels: the 4x1 stripes, first column red → cyan.
        let html = alloc::format!(
            "<img src='{STRIPES_4X1}' style='display:block;width:20px;height:20px;filter:invert(1)'>"
        );
        let p = page(&html, 40, 40);
        assert_eq!(p(2, 10), (0, 255, 255), "red inverts to cyan");

        // A chain composes into ONE transform: inverting twice is identity.
        let twice = "<div style='width:20px;height:20px;background:#ffff00;\
                     filter:invert(1) invert(1)'></div>";
        assert_eq!(pixel_at(twice, 10, 10), (255, 255, 0));

        // `grayscale(100)` — Bootstrap writes the amount as a bare number and
        // means 1. Luma of pure red is 0.213 → 54.
        let gray = "<div style='width:20px;height:20px;background:#ff0000;filter:grayscale(100)'></div>";
        assert_eq!(pixel_at(gray, 10, 10), (54, 54, 54));

        // `blur` cannot be a matrix, so the whole declaration is dropped
        // rather than half-applied — the box keeps its own colour.
        let blur = "<div style='width:20px;height:20px;background:#ffff00;\
                    filter:invert(1) blur(2px)'></div>";
        assert_eq!(pixel_at(blur, 10, 10), (255, 255, 0));
    }

    /// `display: contents` generates no box: no border, and the children take
    /// the place the box would have had. Inline-level content joins the line
    /// its parent is building — which is the half a transparent block cannot
    /// do, and the half every one of these tests turns on.
    #[test]
    fn display_contents_generates_no_box() {
        let eng = Engine::new();
        let unboxed = "<div>P<span style='display:contents;border:10px solid #f00'>A</span>SS</div>";
        let plain = "<div>PASS</div>";
        let dump = dump_ops(&eng.layout(unboxed, 400));
        assert!(!dump.contains("Rgb(255, 0, 0)"), "no border is painted: {dump}");
        // One line, laid out at the same y as if the span were not there.
        let (a, b) = (dump_ops(&eng.layout(unboxed, 400)), dump_ops(&eng.layout(plain, 400)));
        let y = |d: &str| d.lines().find(|l| l.starts_with('T')).map(|l| l.split(' ').nth(1).unwrap().to_string());
        assert_eq!(y(&a), y(&b), "the content sits where it would without the wrapper");

        // A block-level child still gets a block: the wrapper is transparent,
        // not a licence to flatten a paragraph into the line above it.
        let blocks = "<div>P<div style='display:contents'><div>A</div><div>S</div></div></div>";
        let rows: alloc::vec::Vec<_> =
            dump_ops(&eng.layout(blocks, 400)).lines().filter(|l| l.starts_with('T')).map(|l| l.to_string()).collect();
        assert_eq!(rows.len(), 3, "three line boxes, one per block: {rows:?}");
    }

    /// `text-overflow: ellipsis` — Bootstrap's `.text-truncate` idiom. The box
    /// keeps the width it was given; only what is painted inside it changes.
    #[test]
    fn text_overflow_ends_a_clipped_line_in_an_ellipsis() {
        let truncate = "<div style='width:60px;overflow:hidden;white-space:nowrap;\
                        text-overflow:ellipsis'>a long line that will not fit</div>";
        let eng = Engine::new();
        let dump = dump_ops(&eng.layout(truncate, 400));
        let run = dump.lines().find(|l| l.starts_with('T')).expect("one text run");
        assert!(run.ends_with("\u{2026}\"", ), "the run ends in an ellipsis: {run}");
        assert!(!run.contains("not fit"), "and the tail it replaced is gone: {run}");

        // `clip` is the initial value, and the same box under it keeps the
        // whole run — the difference is the property, not the overflow.
        let clipped = truncate.replace("text-overflow:ellipsis", "text-overflow:clip");
        let dump = dump_ops(&eng.layout(&clipped, 400));
        assert!(dump.contains("not fit"), "text-overflow:clip changes nothing");
    }

    /// A hover rule reaches a pseudo-element, and MediaWiki underlines the
    /// article tabs with exactly that: an absolutely positioned `::after` that
    /// is 2 px tall, transparent at rest and coloured under the pointer.
    ///
    /// Its box is generated during layout and paints NOTHING at rest, so there
    /// is no op to replace and none inside it to insert ahead of. What it can
    /// name is its predecessor — everything its element painted comes first.
    #[test]
    fn a_pseudo_element_that_lights_up_is_repainted_too() {
        // The real shape: a tab link is itself a flex box (that is how Vector
        // centres its label), and the underline hangs off it as an absolutely
        // positioned `::after`.
        let html = "<style>a{display:flex;position:relative;color:#00e;width:80px;height:24px}\
                    a::after{content:'';display:block;position:absolute;left:0;bottom:0;width:100%;height:2px}\
                    a:hover::after{background-color:#f00}</style>\
                    <a href=\"/x\"><span>link</span></a>";
        let eng = Engine::new();
        eng.set_hover(alloc::vec![]);
        let mut base = eng.layout(html, 400);
        let b = base.hover_boxes.first().copied().expect("hit-testable");
        let hovered = base.hover_at(b.x + b.w / 2, b.y + b.h / 2);
        assert!(!hovered.is_empty());
        eng.set_hover(hovered);
        let full = eng.layout(html, 400);
        assert!(eng.repaint_hover(&mut base), "{}", eng.repaint_bail());
        assert_eq!(dump_ops(&base), dump_ops(&full));
        assert!(dump_ops(&full).contains("Rgb(255, 0, 0), a: 255"), "the underline is there");
    }

    /// A pseudo-element's box is for repainting, not for hit-testing: it must
    /// not widen where the pointer counts as being inside the element.
    #[test]
    fn a_pseudo_elements_box_does_not_widen_the_pointer_target() {
        let html = "<style>a{display:flex;position:relative;color:#00e;width:80px;height:24px}\
                    a::after{content:'';display:block;position:absolute;left:0;top:40px;width:100%;height:20px;\
                    background:#0f0}a:hover{color:#f00}</style>\
                    <a href=\"/x\"><span>link</span></a>";
        let eng = Engine::new();
        eng.set_hover(alloc::vec![]);
        let lay = eng.layout(html, 400);
        // The `::after` sits 40 px below the link. A point inside IT is not
        // inside the link.
        let b = lay.hover_boxes.iter().find(|b| b.pseudo == crate::css::PseudoElem::None).copied().expect("own box");
        assert!(lay.hover_at(b.x + 2, b.y + 50).is_empty(), "the pseudo must not be a target");
        assert!(!lay.hover_at(b.x + 2, b.y + b.h / 2).is_empty(), "the link itself is one");
    }

    /// Everything a layout draws must survive being written down and read back
    /// — the comparison the repaint tests lean on.
    /// **Das Gate fuer den Schnellweg: neu gemalt muss BYTEGLEICH sein zu neu
    /// ausgelegt.** Alles andere waere ein zweiter Renderpfad, der langsam
    /// auseinanderlaeuft ([[feedback-byte-identical-render-gate]]).
    #[test]
    fn repainting_a_control_equals_laying_the_page_out_again() {
        let html = "<style>body{margin:0}input{width:200px}</style>\
                    <p>Text davor</p><input id=q placeholder=Suche><p>und danach</p>\
                    <input type=checkbox id=c><label for=c>Haken</label>";
        let eng = Engine::new();
        let mut state = crate::forms::FormState::default();
        let mut lay = eng.layout_forms(html, "", 400, &state);
        let seq = lay.controls.first().map(|c| c.seq).expect("ein Feld");
        let cb = lay.controls.get(1).map(|c| c.seq).expect("ein Kaestchen");

        // Fokus + getippter Wert + Haken — alles, was ein Klick und eine Taste
        // auslesen.
        state.focus = Some(seq);
        state.set_value(seq, alloc::string::String::from("Hallo"));
        state.caret = 5;
        state.set_checked(cb, true);

        assert!(eng.repaint_controls(&mut lay, &state), "Schnellweg: {}", eng.repaint_bail());
        let fresh = eng.layout_forms(html, "", 400, &state);
        assert_eq!(dump_ops(&lay), dump_ops(&fresh), "neu gemalt != neu ausgelegt");
    }

    /// **Derselbe Vergleich, aber mit einem Kasten, der einen HINTERGRUND
    /// hat.** Der wird UNTER den schon gemalten Inhalt geschoben, also vor
    /// jedes Feld darin — und die notierte Befehlsspanne des Feldes blieb
    /// dabei stehen. Der Schnellweg ersetzte danach fremde Befehle: getippter
    /// Text lag unter dem alten Kasten (unsichtbar), und der Text daneben
    /// rutschte in der Malreihenfolge nach hinten. Am Geraet sah das aus wie
    /// „das Feld zeigt erst beim Verlassen etwas an und blendet dabei den
    /// Text daneben weg".
    ///
    /// Ein Verlauf am Vorfahren macht es schlimmer (ein Befehl mehr je
    /// Kasten), deshalb steht er hier mit drin.
    #[test]
    fn a_background_above_the_control_does_not_move_its_op_range() {
        let html = "<style>body{margin:0}                    .card{background:#eee;background-image:linear-gradient(90deg,#fff,#eee);padding:8px}                    .row{background:#ddd;padding:4px}input{width:200px}</style>                    <div class=card><span>Beschriftung</span>                    <div class=row><input id=q><button id=b>Los</button></div>                    <p>und danach</p></div>";
        let eng = Engine::new();
        let mut state = crate::forms::FormState::default();
        let mut lay = eng.layout_forms(html, "", 400, &state);
        let seq = lay.controls.first().map(|c| c.seq).expect("ein Feld");
        state.focus = Some(seq);
        state.set_value(seq, alloc::string::String::from("Hallo"));
        state.caret = 5;
        assert!(eng.repaint_controls(&mut lay, &state), "Schnellweg: {}", eng.repaint_bail());
        let fresh = eng.layout_forms(html, "", 400, &state);
        assert_eq!(dump_ops(&lay), dump_ops(&fresh), "neu gemalt != neu ausgelegt");
    }

    /// Und wenn eine `:checked`-Regel Kaesten bewegen kann, gibt der Schnellweg
    /// auf, statt ein Menue zu, das sich haette oeffnen muessen.
    #[test]
    fn a_checked_rule_sends_the_page_to_a_layout() {
        let html = "<style>input:checked ~ .menu{display:block}.menu{display:none}</style>\
                    <input type=checkbox id=c><div class=menu>Menue</div>";
        let eng = Engine::new();
        let mut state = crate::forms::FormState::default();
        let mut lay = eng.layout_forms(html, "", 400, &state);
        let cb = lay.controls.first().map(|c| c.seq).expect("ein Kaestchen");
        state.set_checked(cb, true);
        assert!(!eng.repaint_controls(&mut lay, &state), "muss auslegen");
    }

    fn dump_ops(l: &Layout) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for op in &l.ops {
            match op {
                DrawOp::Text { x, y, size, color, bold, italic, mono, family, sp, text } => {
                    let _ = write!(s, "T {x},{y} {size:.2} c={color:?} {bold}{italic}{mono} {sp:?} {text:?}\n");
                }
                DrawOp::Rect { x, y, w, h, color } => {
                    let _ = write!(s, "R {x},{y} {w}x{h} c={color:?}\n");
                }
                DrawOp::RoundRect { x, y, w, h, r, color, ring } => {
                    let _ = write!(s, "Q {x},{y} {w}x{h} {r:?} c={color:?} {ring:.2}\n");
                }
                DrawOp::Shadow { x, y, w, h, blur, color, .. } => {
                    let _ = write!(s, "S {x},{y} {w}x{h} b={blur:.1} c={color:?}\n");
                }
                DrawOp::Image { x, y, w, h, src, alt, .. } => {
                    let _ = write!(s, "I {x},{y} {w}x{h} {src} {alt}\n");
                }
                DrawOp::BgImage { x, y, w, h, key, .. } => {
                    let _ = write!(s, "B {x},{y} {w}x{h} {key}\n");
                }
                DrawOp::Gradient { x, y, w, h, clip, repeat, pos, size, r, g } => {
                    let _ = write!(s, "G {x},{y} {w}x{h} {clip:?} {repeat:?} {pos:?} {size:?} {r:?} \
                                       {:?} {} {} {:.1} {:?}\n",
                                   g.kind, g.repeating, g.circle, g.angle_for(*w as f32, *h as f32),
                                   g.stops());
                }
            }
        }
        s
    }

    /// Without a mask the same box is a plain filled rect — the guard that the
    /// mask path is what changed, not background painting in general.
    #[test]
    fn a_plain_background_colour_still_fills_the_whole_box() {
        let html = "<div style='width:20px;height:20px;background-color:#ff0000'></div>";
        assert_eq!(pixel_at(html, 4, 10), (255, 0, 0));
        assert_eq!(pixel_at(html, 16, 10), (255, 0, 0));
    }

    /// `no-repeat` must leave the rest of the box alone, and the tile must sit
    /// where `background-position` puts it.
    #[test]
    fn background_image_honours_no_repeat_and_position() {
        let svg = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
                   %20width=%224%22%20height=%224%22%20viewBox=%220%200%204%204%22%3E\
                   %3Cpath%20d=%22M0%200%20H4%20V4%20H0%20Z%22%20fill=%22%230000ff%22/%3E%3C/svg%3E";
        let html = alloc::format!(
            "<div style=\"width:20px;height:20px;background-image:url('{svg}');\
             background-repeat:no-repeat;background-position:right top\"></div>"
        );
        assert_eq!(pixel_at(&html, 18, 2), (0, 0, 255), "tile sits at the right edge");
        assert_eq!(pixel_at(&html, 2, 2), (255, 255, 255), "and nowhere else");
    }

    /// The spelling MediaWiki actually ships: a double-quoted `url()` whose
    /// payload carries BACKSLASH-ESCAPED quotes. Stopping at the first inner
    /// quote truncates the URI into something that still parses as a URL and
    /// then silently decodes to nothing — so this is a paint test, not a
    /// parse test.
    #[test]
    fn a_data_uri_with_escaped_quotes_still_paints() {
        let svg = "data:image/svg+xml;utf8,<svg xmlns=\\\"http://www.w3.org/2000/svg\\\" \
                   width=\\\"20\\\" height=\\\"20\\\" viewBox=\\\"0 0 20 20\\\">\
                   <path d=\\\"M0 0 H10 V20 H0 Z\\\" fill=\\\"%23000\\\"/></svg>";
        let html = alloc::format!(
            "<style>div{{width:20px;height:20px;background-color:#ff0000;\
             mask-image:url(\"{svg}\");mask-size:contain;mask-repeat:no-repeat}}</style><div></div>"
        );
        assert_eq!(pixel_at(&html, 4, 10), (255, 0, 0), "left half is stencilled red");
        assert_eq!(pixel_at(&html, 16, 10), (255, 255, 255), "right half stays clear");
    }

    /// An inline box has no block geometry — only the fragments it leaves in
    /// line boxes. It still paints a background over them, and its horizontal
    /// padding is part of that background AND advances the text after it.
    #[test]
    fn an_inline_box_paints_its_own_background() {
        let at = page("<span style='background-color:#ff0000;padding-left:10px'>l</span>", 120, 40);
        assert_eq!(at(2, 8), (255, 0, 0), "the left padding is background too");
        assert_eq!(at(2, 34), (255, 255, 255), "and stops below the box");
    }

    /// The `a.external` shape: the icon lives in the padding the box reserves
    /// past its text, so nothing shows unless BOTH the padding advances the
    /// flow and the fragment paints its background image.
    #[test]
    fn an_inline_background_image_lands_in_the_padding() {
        let svg = "data:image/svg+xml,%3Csvg%20xmlns=%22http://www.w3.org/2000/svg%22\
                   %20width=%224%22%20height=%224%22%20viewBox=%220%200%204%204%22%3E\
                   %3Cpath%20d=%22M0%200%20H4%20V4%20H0%20Z%22%20fill=%22%230000ff%22/%3E%3C/svg%3E";
        let html = alloc::format!(
            "<span style=\"padding-left:10px;background-image:url('{svg}');\
             background-repeat:no-repeat;background-position:left top\">l</span>"
        );
        let at = page(&html, 120, 40);
        assert_eq!(at(1, 1), (0, 0, 255), "the tile sits in the box's own padding");
        assert_eq!(at(8, 8), (255, 255, 255), "no-repeat leaves the rest alone");
    }

    /// A box that only wraps an icon has no text at all. Its padding still
    /// keeps the line box alive (CSS 2.1 §9.4.2) — otherwise the whole
    /// `.vector-icon` pattern paints nothing.
    #[test]
    fn an_empty_inline_box_still_gets_a_line_to_paint_on() {
        let at = page("<span style='background-color:#ff0000;padding-left:12px'></span>", 120, 40);
        assert_eq!(at(4, 8), (255, 0, 0));
    }

    /// Broken over two lines, an inline box leaves one rectangle per line —
    /// and only the first carries its left border (`box-decoration-break:
    /// slice`, the default).
    #[test]
    fn an_inline_box_broken_over_two_lines_paints_both_fragments() {
        let at = page(
            "<span style='background-color:#ff0000;border-left:4px solid #0000ff'>llllllll llllllll</span>",
            40,
            60,
        );
        assert_eq!(at(1, 1), (0, 0, 255), "the left border opens the first fragment");
        assert_eq!(at(6, 1), (255, 0, 0), "which the background follows");
        assert_eq!(at(6, 21), (255, 0, 0), "the second line carries the background too");
        assert_eq!(at(1, 21), (255, 0, 0), "but not the left border a second time");
    }

    /// Clicking a `<summary>` opens its section and closing it puts the page
    /// back. The state is the `open` CONTENT attribute, so the page's own
    /// `details[open]` rules see it — rustdoc and MDN both style that state.
    #[test]
    fn toggling_a_details_opens_and_closes_it() {
        let html = "<body><details><summary>head</summary><p>body text</p></details></body>";
        let eng = Engine::new();
        let shut = eng.layout_ext(html, "", 800);
        let seq = shut
            .hover_boxes
            .iter()
            .find(|b| b.toggle)
            .expect("the summary is clickable")
            .seq;

        assert!(eng.toggle_details(seq), "the click changed something");
        let open = eng.layout_ext(html, "", 800);
        assert!(open.height > shut.height, "{} !> {}", open.height, shut.height);

        assert!(eng.toggle_details(seq));
        let shut2 = eng.layout_ext(html, "", 800);
        assert_eq!(shut2.height, shut.height, "closing must return to where it was");

        // A seq that is not a summary changes nothing — an unknown one too.
        assert!(!eng.toggle_details(u32::MAX), "no such element");
    }
}
