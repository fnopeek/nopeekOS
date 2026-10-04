# `tools/wasm/bar/src/lib.rs` @ 5e0102684

## L1-9 · `#![no_std]`

```
//! bar — top status status bar, a strut panel rendered via the widget ABI.
//!
//! Declares itself a top-edge strut panel (`npk_window_set_panel`); the
//! compositor positions it into the bar band and draws it with the same
//! translucent-tray blit as the dock. Content is config-driven: segments
//! (`left`/`center`/`right`) listing built-in widgets — workspaces, title,
//! clock, tray, power — from `sys/config/bar`. Live state (clock / focused
//! title / active workspace) is polled from `npk_bar_state`; the tree is
//! only re-committed when that state changes. See docs/spec/PANEL.md.
```

## L27-29 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L53 · `const EDGE_TOP: i32 = 1;`

```
// Panel edge/behavior (see docs/spec/PANEL.md / compositor set_panel).
```

## L57 · `const WS_BASE: u32 = 1;        // workspace i → WS_BASE + i`

```
// ActionId encoding.
```

## L58 · `const WS_BASE: u32 = 1;        // workspace i → WS_BASE + i`

```
// workspace i → WS_BASE + i
```

## L60 · `const SHOT: u32  = 90_001;     // screenshot: left-click = region, right = full`

```
// screenshot: left-click = region, right = full
```

## L61-62 · `const VOL_OPEN: u32 = 90_002;`

```
// Volume: left-click the speaker → launch the `volume` overlay slider;
// right-click → mute.
```

## L65-68 · `const BAND_H: u16 = 24;`

```
// Chrome sizing — docs/spec/UI_REFRESH.md §4 "Panel".
/// Height of the bar's inner content band. The panel height is derived
/// from it, so text and icons are capped to what fits inside: turning
/// them up must never push the bar taller.
```

## L70-71 · `const FONT_DEFAULT: u16 = 15;`

```
/// Text px in the bar. Bigger than `TextStyle::Body` (13) — at bar
/// distance 13 reads small, and 15 still leaves 6 px of band.
```

## L74 · `const FONT_MAX: u16 = 18;      // line height at 18 ≈ 22 px < BAND_H`

```
// line height at 18 ≈ 22 px < BAND_H
```

## L75-83 · `const ICON_DEFAULT: u16 = 16;`

```
/// Icon px in the bar. **Der Atlas fuehrt 16, 24, 32, 48, 64 — sonst
/// nichts** (aus `release/assets/phosphor.atlas` gelesen, nicht geraten).
/// Jede andere Zahl holt die naechstgroessere und laesst sie vom Kasten-
/// filter verkleinern: 18 kam aus dem 24er auf **0,75x**, und eine
/// Flaechenmittelung eines 1,5 px breiten Phosphor-Strichs auf 0,75 IST
/// unscharf. Der Kommentar hier nannte den Weg vorher und zog den Schluss
/// nicht. **16 ist unter 24 das einzige Mass mit 1:1-Blit**, und es macht
/// den Lautsprecher zugleich kleiner (gemessen 17 px Tinte bei 18, die
/// anderen Symbole 15 — das Sprechersymbol ist schlicht breiter gebaut).
```

## L85 · `const ICON_MIN: u16 = 12;      // nicht im Atlas → wird verkleinert`

```
// nicht im Atlas → wird verkleinert
```

## L86 · `const ICON_MAX: u16 = 20;      // 20 + the readout's 4 px padding = BAND_H`

```
// 20 + the readout's 4 px padding = BAND_H
```

## L87-89 · `const CELL_W: u16 = 30;`

```
/// Minimum width of a trailing icon cell. Sie steht NUR in `tray_cell`,
/// darum weitet sie die Symbole rechts, ohne das 40-px-Raster der
/// Desktops (`WS_W` + Zonenabstand) anzufassen.
```

## L91-95 · `const SEP_W: u16 = 16;`

```
/// Breite der Trennstrich-Zelle. Genau die 16 px, die vorher der leere
/// Platz hatte: der Abstand, der einen Fehlklick verhindert, bleibt
/// derselbe — nur steht jetzt ein Strich in seiner MITTE, statt dass er
/// leer ist. Mit `CELL_W` waere Kamera↔Ausschalter von 28 auf 48 px
/// gegangen, und das war nicht gefragt.
```

## L97-98 · `const CELL_RADIUS: u8 = 6;`

```
/// Corner radius of those cells. Bleibt klein: ein Tray-Icon ist
/// quadratisch, und `Pill` machte daraus einen KREIS.
```

## L100-104 · `const WS_W: u16 = 38;`

```
/// Breite einer Desktop-Zelle. Sie ist groesser als `CELL_W`, und das ist
/// der ganze Unterschied zwischen einer Pille und einem Kreis: der
/// Rasterer klemmt `Pill` auf `min(w/2, h/2)`, bei 24 px Bandhoehe also
/// auf 12. Was davon als GERADE Strecke uebrigbleibt, macht die Form —
/// 26 px lassen 2 px (ein Kreis), 38 px lassen 14 px (eine Pille).
```

## L106-111 · `const WS_INDENT: u16 = 6;`

```
// KEIN eigener Rand hier. Der Compositor setzt die Bar selbst als
// schwebende Pille: `set_bar_panel` schreibt `win.x = margin`,
// `win.width = screen_w - 2*margin` mit `shade.bar_margin` (Vorgabe 6) —
// DERSELBE Rand, mit dem die Kacheln liegen. Wer hier noch einmal
// polstert, macht die Bar schmaler als die Fenster; genau das ist in
// 0.9.0 passiert.
```

## L113-121 · `const WS_INDENT: u16 = 6;`

```
/// Einzug VOR der ersten Desktop-Zelle, als unsichtbare Marke.
///
/// Gerechnet, nicht gesetzt: die Karte polstert 4, die Zeile setzt
/// zwischen Marke und erster Zelle ihre eigenen `Spacing::Xxs` = 2, also
/// 4 + 6 + 2 = **12 px** bis zur „1" — dieselben 12, die die Ablage im
/// Dock innen laesst, damit die zwei Pillen gleich aussehen.
///
/// Eine unsichtbare Marke und nicht `PaddingXY`, weil das links UND
/// rechts polstert; hier soll nur links etwas passieren.
```

## L123-125 · `const SEP_H: u16 = 14;`

```
/// Hoehe des Trennstrichs. Seine BREITE ist keine eigene Zahl: er sitzt
/// in einer Zelle, die genau so breit ist wie eine Arbeitsflaeche —
/// siehe das Segment „title".
```

## L127-134 · `const WS_RADIUS: u8 = Radius::Pill as u8;`

```
/// Eckradius der Desktop-Zellen: ganz rund.
///
/// Sie sitzen in der Karte, die selbst eine Pille ist (36 px hoch →
/// Radius 18), mit 4 px Polsterung dazwischen. Konzentrisch waeren
/// 18 − 4 = 14; eine 38x24-Zelle bekommt 12, ist also zwei Pixel ENGER
/// als der Bogen ueber ihr. Das ist die sichere Richtung — zu weit waere
/// der Fall, in dem die Ecke schneidet. Deshalb braucht die Bar keinen
/// waagrechten Zuschlag, anders als die Ablage im Dock.
```

## L137-140 · `const HEAP_SIZE: usize = 256 * 1024;`

```
// ── Bump allocator with a reset mark ─────────────────────────────────
// Config is parsed once (below MARK and kept); the per-frame widget tree
// is rebuilt above MARK and reclaimed each commit by resetting to MARK,
// so re-committing every clock tick never leaks.
```

## L162-163 · `static mut NUMBUF: [u8; 12] = [0; 12];`

```
// u32 → decimal &str in a static buffer (no alloc — safe in the panic handler
// even when the panic IS an allocation failure).
```

## L188-191 · `core::arch::wasm32::unreachable()`

```
// Trap — do NOT `loop {}`. A wasm `unreachable` makes `_start`'s host call
// return Err, so the kernel tears this instance down and frees its worker
// core. A busy loop pins the cooperative fiber forever → the whole core
// pegs at 100% (the "core spins, never halts" bug). Clean death > spin.
```

## L216-220 · `struct Segments { left: Vec<String>, center: Vec<String>, right: Vec<String> }`

```
// ── Config: segments ─────────────────────────────────────────────────
// `sys/config/bar` lines: "left: a b c", "center: x", "right: y z",
// plus "font: <px>" / "icon: <px>" (see read_sizes).
// Missing / empty → built-in default. Unknown widget names are skipped
// at render time.
```

## L257-260 · `static mut FONT_PX: u16 = FONT_DEFAULT;`

```
// ── Config: sizing ───────────────────────────────────────────────────
// Same file, two more lines: "font: 15", "icon: 18". Kept in statics and
// re-read every few seconds, so tuning them is edit-and-look — no heap
// involved, which is what lets this run above the bump-allocator mark.
```

## L267 · `fn read_sizes() -> bool {`

```
/// Re-read the two size lines. True when either changed.
```

## L292 · `unsafe {`

```
// SAFETY: single-threaded WASM app.
```

## L301-302 · `const STATE_MAX: usize = 256;`

```
// ── Live state ───────────────────────────────────────────────────────
// `npk_bar_state` → "HH:MM\n<ws_count>\n<ws_active>\n<title>".
```

## L307-308 · `static mut BAT: i32 = -1;`

```
// Battery, polled alongside bar_state. -1 = no battery (segment hidden);
// else (status<<8)|percent. Sentinel i32::MIN = never read yet.
```

## L311 · `static mut VOL: i32 = 80;`

```
// Master volume (0..=100), polled each tick (cheap atomic). i32::MIN = unread.
```

## L314 · `static mut PRE_MUTE: i32 = 50; // level restored on un-mute`

```
// level restored on un-mute
```

## L327-330 · `const WINS_CAP: usize = 2048;`

```
// ── Window list ──────────────────────────────────────────────────────
// `npk_window_titles` → "<flags>\t<workspace>\t<title>" per open window.
// Kept in a static buffer: the render loop resets the bump allocator
// before every commit, so heap-derived state would dangle a frame later.
```

## L335 · `fn refresh_windows() -> bool {`

```
/// Re-read the window list; true when it changed since the last read.
```

## L340 · `unsafe {`

```
// SAFETY: single-threaded WASM app.
```

## L352 · `unsafe {`

```
// SAFETY: single-threaded; only refresh_windows writes the buffer.
```

## L359 · `fn workspace_occupied(ws: u8) -> bool {`

```
/// Does workspace `ws` hold at least one window?
```

## L368-370 · `static mut CATALOG: Option<Vec<app_catalog::AppEntry>> = None;`

```
// ── App icons ────────────────────────────────────────────────────────
// Window titles carry the module name; the catalog maps that to the
// app's declared icon. Loaded once at startup (below the heap mark).
```

## L374 · `unsafe { *(&raw mut CATALOG) = Some(app_catalog::load(&[])); }`

```
// SAFETY: single-threaded; called once before the render loop.
```

## L379 · `let cat = unsafe { (*(&raw const CATALOG)).as_ref() };`

```
// SAFETY: single-threaded; written once by load_catalog.
```

## L386 · `fn cell(child: Widget, modifiers: Vec<Modifier>) -> Widget {`

```
// ── Segment → widgets ────────────────────────────────────────────────
```

## L388-390 · `fn cell(child: Widget, modifiers: Vec<Modifier>) -> Widget {`

```
/// A fixed-size, centred chrome cell — a workspace pill or a tray icon.
/// `prefab::center_box` supplies the flex spacers: `MinWidth` alone
/// widens the box and leaves the glyph pinned to its left edge.
```

## L395-396 · `fn bar_text(content: String, style: TextStyle, mut modifiers: Vec<Modifier>) -> Widget {`

```
/// Bar text: the style picks the face, `FontSize` the size. Every string
/// in the bar goes through here so one config line moves all of them.
```

## L402 · `fn readout(icon: IconId, icon_mods: Vec<Modifier>, value: String,`

```
/// Icon plus a mono value (volume, battery) as one hoverable unit.
```

## L430-431 · `fn tray_cell(icon: IconId, click: Option<ActionId>, tint: Token) -> Widget {`

```
/// Tray icon cell: rest is `OnSurfaceMuted` on no background, hover
/// fills `SurfaceHover` (docs/spec/UI_REFRESH.md §3 `toolbar_button`).
```

## L447-448 · `fn segment_widgets(name: &str, st: &BarState) -> Vec<Widget> {`

```
// No per-widget Padding (the bar is short — the enclosing card supplies
// the inset; uniform Padding here would overflow the band vertically).
```

## L452-455 · `let mut row = Vec::new();`

```
// A rounded cell per workspace. Active = filled Accent; an
// occupied-but-inactive one keeps full-strength text; an empty
// one recedes to OnSurfaceFaint, so the row doubles as an
// at-a-glance map of where your windows are.
```

## L473-474 · `mods.push(Modifier::Hover(alloc::vec![`

```
// Dieselbe Form wie die Zelle: ein Hover-Rechteck mit
// anderem Radius ist der Fall, der auffaellt.
```

## L491-493 · `"title" => {`

```
// The focused app: its catalog icon plus its name. Absent when
// nothing is open, and preceded by a hairline so the divider only
// shows up together with it.
```

## L497-519 · `alloc::vec![`

```
// `1 2 3 4 (5) | (6) Appname` — und das RASTER ist das
// Mittel, nicht ein ausbalanciertes Paar Abstaende.
//
// Zuerst stand hier eine Polsterung von einer Zellbreite.
// Die stimmte im Abstand und nicht in der Lage: sie setzt
// den Strich ans ENDE des Platzes (5) statt in seine
// Mitte, gemessen 19,5 px zu weit rechts — eine halbe
// Zelle.
//
// Jetzt bekommt er die Zelle wirklich: ein Kasten von
// `WS_W`, der Strich darin zwischen zwei Spreizern, also
// mittig. Damit liegt alles auf demselben 40er-Raster wie
// die Ziffern (`WS_W` + Zonenabstand), und keine Zahl
// davon ist von Hand abgestimmt.
//
// Und **weil** der Strich in SEINER Zelle mittig sitzt,
// ist der Abstand nach beiden Seiten von selbst gleich:
// bis zur „4" ist es eine halbe Zelle plus Zonenabstand,
// bis zur Anwendung dasselbe — 21,5 px und 21,5 px. Der
// leere Platz (6) aus Florians erster Skizze stand
// zwischendrin und machte daraus 21,5 gegen 61,5; er ist
// deshalb weg. Symmetrie ist hier eine Eigenschaft der
// Konstruktion und nicht eine abgestimmte Zahl.
```

## L537-559 · `prefab::mark(`

```
// Das Symbol steht MITTIG in seiner Zelle, wie
// eine Ziffer in ihrer.
//
// Ohne das klebt es am Trennstrich, und zwar
// sichtbar: die Kaesten sind zwar symmetrisch
// (21,5 px links wie rechts), aber die „4" ist
// eine 6-px-Ziffer in der MITTE einer 38-px-
// Zelle — rechts von ihr stehen 17 px leere
// Zelle. Das Symbol dagegen faengt sofort an
// seiner Kastenkante an. Gemessen: 38,5 px
// Tinte-zu-Tinte auf der einen Seite, 21,5 auf
// der anderen.
//
// Mit dem Vorlauf sitzt sein Mittelpunkt auf
// dem Mittelpunkt von Zelle 6, und damit
// stehen alle im selben 40er-Takt:
// 36,5 · 76,5 · 116,5 · 156,5 · 196,5 · 236,5.
//
// Die Breite ist gerechnet, nicht gesetzt: die
// halbe Restzelle minus den Abstand, den die
// Zeile ohnehin zwischen Marke und Symbol
// setzt. Sie folgt damit `icon_px()`, das aus
// der Konfiguration kommt.
```

## L580-582 · `"battery" => {`

```
// Battery: hidden entirely when no smart battery responds (bat < 0,
// e.g. desktops/QEMU). Icon picked by charge level; charging shows
// the bolt; a near-empty pack tints Danger.
```

## L586 · `let status = (st.bat >> 8) & 0xFF; // 0=discharge 1=charge 2=full 3=plugged-idle`

```
// 0=discharge 1=charge 2=full 3=plugged-idle
```

## L588 · `1 => IconId::BatteryCharging, // actively charging → bolt`

```
// actively charging → bolt
```

## L590 · `3 => IconId::Plug,            // on AC, held (not charging) → plug`

```
// on AC, held (not charging) → plug
```

## L609-615 · `"sep" => alloc::vec![Widget::Row {`

```
// Trennstrich vor dem Ausschalter, in SEINER eigenen Zelle —
// dieselbe Bauweise wie der Strich zwischen Desktops und Anwendung
// (0.9.3). Weil er in der Zelle mittig sitzt, ist der Abstand zur
// Kamera und zum Ausschalter von selbst gleich; keine der Zahlen
// ist von Hand abgestimmt. Die Zelle haelt zugleich den Abstand,
// den vorher der leere Platz hielt: ein Klick auf die Kamera darf
// nicht auf dem Ausschalter landen.
```

## L630-632 · `"gap" => alloc::vec![Widget::Text {`

```
// Leerer Platz fester Breite. Steht nicht mehr in der Vorgabe —
// "sep" hat ihn abgeloest —, bleibt aber, damit eine bestehende
// `sys/config/bar` ihn weiter nennen darf.
```

## L641-643 · `"volume" => {`

```
// Volume: speaker icon + level. Left-click either → launch the
// `volume` overlay slider; right-click → mute. Reflects the kernel
// master volume (also moved by the slider / apps).
```

## L656-659 · `fn zone(names: &[String], st: &BarState) -> Widget {`

```
/// A zone → a plain Row of its segments. The chrome is no longer per
/// zone: the design has ONE bar card holding all three (docs/spec/UI_REFRESH.md
/// §4). Empty zones collapse to a zero spacer so the left/center/right
/// structure stays intact.
```

## L670-673 · `modifiers: alloc::vec![`

```
// Pin the zone to the band. Without this a single over-tall
// segment makes the zone taller than the bar's content box, the
// centring offset saturates to zero, and everything in that zone
// rides out of the bottom edge.
```

## L682-685 · `let sides = Widget::Row {`

```
// Two overlaid full-width layers so the clock is centred on the SCREEN,
// not between the (asymmetric) side groups: the sides layer pins left to
// the start + right to the end; the centre layer centres the clock with
// equal flex spacers.
```

## L706-709 · `Widget::Stack {`

```
// One continuous card. In the bar's transparent scene the background
// is filled at chrome alpha with anti-aliased corners and the
// compositor composites it by per-pixel alpha — translucent panel,
// crisp glyphs, no halo.
```

## L714-720 · `Modifier::Border { token: Token::Border, width: 1, radius: Radius::Pill.as_u8() },`

```
// Ganz rund. Der Rasterer klemmt `Pill` auf `min(w/2, h/2)`.
// Die Karte fuellt das Fenster, und dessen Hoehe setzt der
// Compositor auf `pill_h` = das `h` aus `set_panel`, also 36
// → Radius 18, echte Halbkreise an beiden Enden.
//
// Rahmen und Fuellung tragen DENSELBEN Wert. Stuenden dort
// zwei, liefe der 1-px-Strich neben seiner eigenen Flaeche.
```

## L728-729 · `fn state_changed() -> Option<usize> {`

```
/// Read live state into STATE_BUF; return Some(len) if it changed since
/// the last commit (and update LAST_BUF), else None.
```

## L735-740 · `let bat = unsafe { npk_battery() };`

```
// Poll battery too — it changes slowly, so throttle the SMBus reads to
// roughly every 5 s (loop tick is 300 ms) and fold the result into the
// same change-gate (a % or charge-state flip forces a re-commit).
// The bar only wakes when something changed (or the minute turned), so
// no throttle: on a machine with the AML driver this is a cached value,
// and the kernel wakes us when it moves.
```

## L751-753 · `let wins_changed = refresh_windows();`

```
// The window list feeds the occupied-workspace hints, and a window
// opening on ANOTHER workspace leaves bar_state untouched — so it
// needs its own vote in the change gate.
```

## L769 · `unsafe { HEAP_POS = HEAP_MARK; }`

```
// Reclaim the previous frame's tree; config (below the mark) survives.
```

## L794-795 · `Event::Action(ActionId(id)) => {`

```
// Left-click. Screenshot icon → region select (slice ③; falls
// back to full for now). Power → off. Otherwise a workspace pill.
```

## L802 · `launch("volume", "");`

```
// Open the slider as its own centred overlay (drun-style).
```

## L808 · `Event::ContextAction(ActionId(id)) => {`

```
// Right-click: screenshot icon → full-screen capture; speaker → mute.
```

## L829 · `load_catalog();                    // title → app icon, resolved once`

```
// title → app icon, resolved once
```

## L830 · `unsafe { HEAP_MARK = HEAP_POS; }   // freeze config; tree rebuilds above this`

```
// freeze config; tree rebuilds above this
```

## L832-834 · `unsafe { let _ = npk_window_set_panel(EDGE_TOP, BEHAVIOR_STRUT, 1920, 36); }`

```
// Declare the top strut panel. `h` is the band height we need (room for
// 24px tray icons + pill padding); the compositor reserves `margin + h`
// and sizes our window to it. `w` is advisory.
```

## L837 · `if let Some(len) = state_changed() {`

```
// First commit.
```

## L842-848 · `const WAIT_INPUT: i32 = 1;`

```
// **Wait for a change, don't poll for one.** Until 0.10.0 the bar woke
// every 300 ms and asked whether the clock, the window list, the battery
// or the volume had moved — 200 times a minute for one minute change.
// Now the kernel wakes it: WAIT_INPUT for a click, WAIT_STATE when a
// watched topic changes (windows, battery, volume, a config file), and
// the deadline is the next full minute for the clock.
// docs/plan/CORES_AND_EVENTS.md, Stufe 2d.
```

## L854 · `loop {`

```
// Drain any pending click events first.
```

## L862-863 · `let resized = fired & WAIT_STATE != 0 && read_sizes();`

```
// Sizes come from `sys/config/bar` and are tuned by eye, so an edit
// must show without a restart — a config write is a watched topic.
```

## L865-866 · `match state_changed() {`

```
// Re-render only when the live state changed (clock minute / title
// / active workspace), so the tree isn't rebuilt every wake.
```

## L869-870 · `None if resized => {`

```
// State is unchanged but the sizes moved — repaint at the last
// committed length (STATE_BUF still holds that same state).
```

## L879-880 · `let now = unsafe { npk_unix_time() }.max(0);`

```
// Until the next full minute, plus a little so the minute has
// surely turned when we read the clock.
```

