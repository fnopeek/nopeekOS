# `tools/wasm/beak-engine/src/js/dombind.rs` @ 5e0102684

## L1-17 · `use alloc::rc::Rc;`

```
//! Das DOM fuer JavaScript.
//!
//! **Wem gehoert der Baum.** `beak_engine::dom` haelt einen BESITZENDEN Baum
//! (`Element` haelt seine Kinder), und darin kann JavaScript keine Referenz
//! halten: ein Handle muesste einen Knoten ueberleben, den ein Nachbar
//! besitzt. Also wird der Baum in eine **Arena** geflacht — ein `Vec` von
//! Knoten, und ein Handle ist ein Index.
//!
//! Das ist eine ZWEITE Darstellung desselben Dokuments, und das ist eine
//! Schuld, keine Loesung: solange beaks Layout den alten Baum liest und JS die
//! Arena schreibt, wirkt eine DOM-Aenderung NICHT auf das Bild. Die Arena ist
//! aber genau die Form, zu der der alte Baum vereinheitlicht werden muss —
//! Indizes statt Besitz — und dieser Schritt macht sie erst messbar.
//!
//! **Knotenidentitaet ist beobachtbar.** `document.body === document.body`
//! muss wahr sein, also wird das JS-Huellobjekt je Knoten EINMAL gebaut und
//! behalten.
```

## L27 · `fn is_void(tag: &str) -> bool {`

```
/// Elemente ohne Schlusstag.
```

## L57 · `pub js: Option<Gc>,`

```
/// Einmal gebaut, dann behalten — sonst waere `el === el` falsch.
```

## L59 · `pub listeners: Vec<(Rc<str>, Value)>,`

```
/// Angemeldete Behandler, je Ereignisart.
```

## L61-66 · `pub handlers: Vec<(Rc<str>, Value)>,`

```
/// Behandler, die als EIGENSCHAFT gesetzt wurden (`el.onclick = f`).
///
/// Getrennt von `listeners`, weil sie sich anders verhalten: eine zweite
/// Zuweisung ERSETZT die erste, waehrend `addEventListener` anhaengt. Und
/// sie verdraengt das gleichnamige Attribut — im Browser ist es derselbe
/// Platz, nicht zwei.
```

## L68-75 · `pub value: Option<Rc<str>>,`

```
/// Der „schmutzige" Wert eines Steuerelements (HTML §4.10.5.5): was der
/// Benutzer getippt oder ein Skript gesetzt hat.
///
/// **Getrennt vom Attribut, und das ist keine Feinheit.** `el.value = x`
/// aendert im Browser das `value`-ATTRIBUT NICHT — das bleibt der
/// Vorgabewert (`defaultValue`), auf den `form.reset()` zurueckstellt.
/// Wer beides zusammenlegt, hat eine Seite, die nach dem Zuruecksetzen
/// das Getippte wieder hinschreibt, und `getAttribute("value")` luegt.
```

## L77 · `pub checked: Option<bool>,`

```
/// Dasselbe fuer `checked`: `defaultChecked` ist das Attribut.
```

## L79-80 · `pub content: Option<u32>,`

```
/// Nur `<template>`: der Bruchstueck-Knoten, in dem sein Inhalt haengt.
/// Entsteht beim ersten `.content` — siehe dort, warum nicht frueher.
```

## L82-85 · `pub seq: u32,`

```
/// Die `seq` desselben Elements in beaks Baum — die Bruecke zwischen
/// Klickpunkt und Knoten. `to_dom` vergibt sie und schreibt sie HIER
/// zurueck; das Layout gibt sie beim Treffer aus. Ohne diese Brueckenzahl
/// gibt es keinen Weg von „hier wurde geklickt" zu „dieser Knoten".
```

## L87-96 · `pub src_seq: u32,`

```
/// Die `seq` des Elements im BAUM, aus dem dieses Dokument gebaut wurde.
///
/// Die Bruecke fuer `getComputedStyle`: die Kaskade laeuft auf beaks Baum
/// (`crate::dom`), die Maschine arbeitet auf ihrer eigenen Arena. Ohne
/// diesen Verweis gibt es keinen Weg von „dieses JS-Objekt" zu „dieses
/// Element, fuer das die Kaskade gerechnet hat".
///
/// `0` heisst „kein Quellknoten" — ein Element, das ein Skript erst
/// erzeugt hat. Fuer das kann `getComputedStyle` nur den Inline-Stil
/// beantworten, und das ist ehrlicher als eine Zahl aus dem Nichts.
```

## L118-125 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
/// Was sich am Baum geaendert hat — die Rohaufzeichnung fuer
/// `MutationObserver`.
///
/// **Aufgezeichnet wird im BAUM, nicht in der Bindung.** Ein Beobachter, der
/// sich seine Meldungen aus einem Vergleich zweier Momentaufnahmen rechnet,
/// meldet „geaendert" und weiss nicht, was; und er kann nicht sehen, dass ein
/// Knoten weg war und wieder da ist. Die Stellen, an denen sich ein Baum
/// aendert, sind gezaehlt — also sagen sie es selbst.
```

## L132 · `pub target: u32,`

```
/// Bei `ChildList` der ELTER, bei den anderen beiden der Knoten selbst.
```

## L140-147 · `pub hits: u64,`

```
/// **Welche beobachteten Knoten diese Aenderung ZUM ZEITPUNKT DER
/// AENDERUNG umschlossen haben**, als Bitmaske ueber `Doc::observed`.
///
/// Das ist der ganze Grund, warum es diese Maske gibt: die Zugehoerigkeit
/// spaeter zu pruefen liest den Baum von SPAETER. Ein Knoten, der erst
/// gesetzt und dann eingehaengt wird, waere faelschlich dabei; einer, der
/// geaendert und dann entfernt wird, fiele heraus. Beides ist mir genau
/// so passiert, und beides sieht aus wie ein Fehler der Bibliothek.
```

## L151-156 · `pub const MAX_MUTATIONS: usize = 20_000;`

```
/// Deckel fuer die Rohaufzeichnung.
///
/// Ein Skript, das hunderttausend Knoten baut, waehrend ein Beobachter
/// angemeldet ist, darf den Speicher nicht auffressen. Ueberlaeuft es, wird
/// das GEMELDET und nicht still verschluckt — ein Beobachter, dem Meldungen
/// fehlen, ohne dass es jemand sagt, ist schlimmer als gar keiner.
```

## L165-169 · `pub dirty: bool,`

```
/// Hat sich seit dem letzten Zurueckschreiben etwas geaendert?
///
/// Ohne diese Fahne muesste JEDER Klick den Baum neu aufbauen und die
/// Seite neu auslegen — 130 ms auf dem Geraet, fuer nichts, wenn der
/// Behandler nur etwas gezaehlt hat.
```

## L171-172 · `pub has_listeners: bool,`

```
/// Hat die Seite ueberhaupt Behandler? Solange nicht, braucht das Layout
/// keine Treffer-Kaesten aufzuzeichnen.
```

## L174-179 · `pub focused: Option<u32>,`

```
/// Welches Element den Tastaturfokus hat — gesetzt von `focus()`.
///
/// Der Wirt liest es und stellt seinen eigenen Fokus danach
/// (`forms.rs::FormState::focus`); ohne diese Zeile war `el.focus()`
/// „focus is not a function", und die Fritzbox-Anmeldemaske ist genau
/// daran am Ende ihres Aufbaus gescheitert.
```

## L181-187 · `pub version: u32,`

```
/// Zaehlt jede Aenderung am Baum — und wird NIE zurueckgesetzt.
///
/// `dirty` beantwortet „muss ich zurueckschreiben?" und wird beim
/// Zurueckschreiben geloescht. Fuer einen Zwischenspeicher taugt es
/// deshalb nicht: nach dem Loeschen ist „unveraendert seit meinem Stand"
/// von „seither zweimal geaendert" nicht mehr zu unterscheiden. Ein
/// Zaehler, der nur steigt, kann beides.
```

## L189-190 · `pub mutations: Vec<Mutation>,`

```
/// Die Rohaufzeichnung fuer `MutationObserver` — leer, solange niemand
/// zusieht.
```

## L192-193 · `pub observing: bool,`

```
/// Sieht ueberhaupt jemand zu? Ohne diese Fahne zahlte JEDE Seite fuer
/// eine Aufzeichnung, die keiner abholt.
```

## L195 · `pub mut_overflow: bool,`

```
/// Ist die Aufzeichnung uebergelaufen? Wird beim Zustellen gemeldet.
```

## L197-199 · `pub observed: Vec<(u32, bool)>,`

```
/// Die beobachteten Knoten als `(Knoten, mit Nachkommen)`, in der
/// Reihenfolge der Bits in `Mutation::hits`. Der Baum muss NICHT wissen,
/// wer zusieht — nur, wo.
```

## L213-215 · `pub fn from_dom(src: &crate::dom::Dom) -> Doc {`

```
/// Aus beaks geparstem Baum. Der `seq` des Originals wird NICHT
/// uebernommen — die Arena vergibt eigene Indizes, und die Zuordnung
/// zurueck ist die Aufgabe des Schritts, der beide vereinheitlicht.
```

## L223 · `d.dirty = false;`

```
// Der Aufbau selbst ist keine Aenderung.
```

## L241-243 · `if is_handler_attr(k) { self.has_listeners = true; }`

```
// Ein Attribut-Behandler ist genauso ein Behandler wie
// ein angemeldeter: ohne diese Zeile bekaeme die Seite
// keine Treffer-Kaesten, und der Klick fiele ins Leere.
```

## L263 · `pub fn create(&mut self, kind: f64, tag: &str) -> u32 {`

```
/// Frei stehender Knoten, noch ohne Elternteil (`createElement`).
```

## L279-283 · `pub fn hits_for(&self, node: u32) -> u64 {`

```
/// Welche beobachteten Knoten `node` JETZT umschliessen.
///
/// Ein Gang die Elternkette hinauf, genau wie ihn ein echter Motor an
/// dieser Stelle macht — die Kosten sind die Tiefe, nicht die Zahl der
/// Knoten.
```

## L299-300 · `pub fn record(&mut self, mut m: Mutation) {`

```
/// Eine Aenderung notieren — nur, wenn jemand zusieht, und nur, wenn sie
/// ueberhaupt jemanden betrifft.
```

## L309-311 · `fn siblings(&self, parent: u32, at: usize) -> (Option<u32>, Option<u32>) {`

```
/// Die Geschwister eines Knotens, wie sie JETZT stehen. Ein Datensatz
/// nennt sie so, wie sie zum Zeitpunkt der Aenderung waren — danach
/// stimmen sie nicht mehr.
```

## L317-318 · `pub fn detach(&mut self, id: u32) {`

```
/// Aus dem alten Elternteil aushaengen. Muss VOR jedem Einhaengen laufen,
/// sonst steht ein Knoten in zwei Kinderlisten und der Baum ist keiner mehr.
```

## L338-341 · `self.detach(child);`

```
// **Ein Umhaengen ist ein Entfernen UND ein Einfuegen**, und beides
// gehoert gemeldet — die Spezifikation sagt es so, und ein
// Beobachter, der nur das Einfuegen saehe, haette den Knoten
// zweimal im Baum.
```

## L354-360 · `pub fn insert_maybe_fragment(&mut self, parent: u32, child: u32, before: Option<u32>) {`

```
/// Einhaengen — und ein BRUCHSTUECK gibt dabei seine Kinder ab.
///
/// Das ist keine Feinheit, sondern der Sinn der Sache: `ul.appendChild(
/// tpl.content.cloneNode(true))` ist die uebliche Schreibweise, und wer
/// das Bruchstueck selbst einhaengt, bekommt ein `<#fragment>`-Element in
/// den Baum — ein Element, das es im HTML nicht gibt und das jede
/// Formatierung darunter verschiebt.
```

## L389-390 · `pub fn set_attr_at(&mut self, id: u32, k: &str, v: &str) {`

```
/// Ein Attribut setzen — **der eine Weg**, damit die Aenderung EINMAL
/// notiert wird und nicht an zwoelf Stellen vergessen.
```

## L402-403 · `pub fn remove_attr_at(&mut self, id: u32, k: &str) {`

```
/// Ein Attribut entfernen. Ist es gar nicht da, ist es KEINE Aenderung —
/// ein Datensatz dafuer waere eine erfundene Meldung.
```

## L416-418 · `pub fn set_text(&mut self, id: u32, v: Rc<str>) {`

```
/// Den Text eines Text-/Kommentarknotens setzen (`data`, `nodeValue`).
/// NICHT fuer frisch gebaute Knoten: dort gibt es keinen alten Wert und
/// niemanden, der zusieht.
```

## L430 · `pub fn text_of(&self, id: u32) -> String {`

```
/// Der Text eines Teilbaums, aneinandergehaengt.
```

## L446 · `pub fn descendants(&self, from: u32, out: &mut Vec<u32>) {`

```
/// Alle Elemente in Dokumentreihenfolge ab `from`.
```

## L456-469 · `pub fn parse_into(&mut self, parent: u32, html: &str, at: Option<usize>) -> Vec<u32> {`

```
/// Ein HTML-Bruchstueck parsen und unter `parent` einhaengen.
///
/// Das ist der Weg von `innerHTML =` und `insertAdjacentHTML`. Es geht
/// ueber beaks eigenen Parser — ein zweiter, laxerer waere eine zweite
/// Wahrheit darueber, was das Web bedeutet.
/// HTML in einen Knoten parsen — als BRUCHSTUECK, nicht als Dokument.
///
/// `dom::parse` baut immer ein ganzes Dokument: `<li>x</li>` wird zu
/// `<html><body><li>x`. Wer das ungefiltert einhaengt, schiebt bei JEDEM
/// `innerHTML` ein `<html><body>` unter das Element — gefunden beim
/// Bauen von `append`, und es betraf jede Seite, die `innerHTML` setzt.
/// Sichtbar war es nicht, weil `<html>` und `<body>` keinen eigenen
/// Kasten malen; kaputt war es trotzdem: `el.children[0]` war nicht das
/// erste Element des Textes, sondern der Rahmen darum.
```

## L485-491 · `pub fn parse_document(&mut self, src: &str, html_kind: bool) -> u32 {`

```
/// Ein GANZES Dokument parsen — der Weg von
/// `DOMParser.parseFromString`. Anders als `parse_into` bleibt der
/// `<html>`/`<head>`/`<body>`-Rahmen STEHEN: hier ist er nicht der
/// Rahmen um ein Bruchstueck, sondern das Ergebnis.
///
/// Losgeloest wie `createHTMLDocument`: derselbe Knotenspeicher, aber
/// kein Elternteil, also sieht ihn weder das Layout noch der Wirt.
```

## L496-497 · `for c in fragment_nodes(&parsed.root) {`

```
// XML und SVG kennen keinen `<body>`-Rahmen: die Wurzel des
// Textes IST die Wurzel des Dokuments.
```

## L512-515 · `for (k, tag) in ["head", "body"].iter().enumerate() {`

```
// **Der Parser laesst einen leeren `<head>` weg, die Spezifikation
// nicht.** Das Bruchstueck-Parsen baut immer beide Kinder, und eine
// Seite, die `doc.head` liest, bekaeme sonst `null` fuer ein
// Dokument, in dem der Kopf nur leer ist.
```

## L528 · `fn from_src_node(&mut self, n: &crate::dom::Node) -> Option<u32> {`

```
/// Einen geparsten Teilbaum in die Arena legen, noch ohne Elternteil.
```

## L553-555 · `pub fn serialize(&self, id: u32, inner_only: bool) -> String {`

```
/// Ein Knoten als HTML. Fuer `innerHTML`/`outerHTML` — und die
/// Maskierung ist Pflicht, nicht Kosmetik: ein `<` im Text, das
/// unmaskiert herauskommt, macht aus Inhalt Auszeichnung.
```

## L576 · `pub fn clear_children(&mut self, id: u32) {`

```
/// Alle Kinder eines Knotens loesen (fuer `innerHTML =`).
```

## L582-585 · `if self.observing && !old.is_empty() {`

```
// `innerHTML = "…"` raeumt hier ab, bevor es neu baut. Ohne diese
// Meldung saehe ein Beobachter nur das Neue und nie, dass das Alte
// weg ist — und genau daran haengen die Aufraeumroutinen jeder
// Komponentenbibliothek.
```

## L593 · `pub fn clone_node(&mut self, id: u32, deep: bool) -> u32 {`

```
/// Einen Teilbaum kopieren (`cloneNode`).
```

## L612-626 · `pub fn touch(&mut self) {`

```
/// Zurueck in beaks Baum — der Schritt, der eine DOM-Aenderung UEBERHAUPT
/// ERST sichtbar macht.
///
/// Solange nur die Arena veraendert wird, bleibt das Bild stehen: Layout,
/// Kaskade und Formulare lesen `dom::Dom`. Statt das Layout auf die Arena
/// umzubauen — was jede gemessene Zahl dieser Engine aufs Spiel setzen
/// wuerde — wird hier zurueckgeschrieben. Der Preis ist ein voller
/// Neuaufbau des Baums je Skriptlauf, nicht je Aenderung; gegen ein
/// Layout von 150 ms faellt das nicht auf.
///
/// `seq` wird NEU vergeben, in Dokumentreihenfolge — genau wie der Parser
/// es tut. Damit stimmt die Identitaet, an der die Formularzustaende
/// haengen, fuer alles, was der Skriptlauf nicht angefasst hat.
/// Eine Aenderung am Baum vermerken. `dirty` fuer „zurueckschreiben",
/// `version` fuer jeden, der einen Zwischenspeicher darauf haelt.
```

## L632-643 · `pub fn live_dom(&self) -> crate::dom::Dom {`

```
/// Der LEBENDE Baum, so wie die Kaskade ihn braucht — ohne etwas zu
/// aendern.
///
/// Unterschied zu `to_dom`: das hier vergibt keine neuen `seq`. `to_dom`
/// tut das, weil es die Bruecke zum Layout neu spannt; wer es zwischendurch
/// riefe, um nur EINE Frage zu beantworten, wuerde jedem Kasten im
/// fertigen Layout die Nummer unter dem Stuhl wegziehen.
///
/// Die Kennung ist stattdessen der ARENA-INDEX, der sich nie aendert. Sie
/// steht als `seq` im gebauten Element, also findet `find_path` das
/// Element mit derselben Zahl wieder, mit der der Rufer sein JS-Objekt
/// haelt.
```

## L659 · `e.index_attrs();`

```
// MUSS nach den Attributen laufen — siehe `to_node`.
```

## L678 · `pub fn by_seq(&self, seq: u32) -> Option<u32> {`

```
/// Der Arena-Knoten zu einer `seq` aus dem Layout.
```

## L691 · `self.nodes[id as usize].seq = *seq;`

```
// Die Bruecke: dieselbe Zahl steht jetzt hier und im Layout.
```

## L695-696 · `e.index_attrs();`

```
// MUSS nach den Attributen laufen — sonst sind Klassen, id und der
// Bloom-Filter leer und kein Selektor trifft mehr.
```

## L705-706 · `pub enum ScriptRef {`

```
/// Ein Skript der Seite: entweder sein Text oder die Adresse, unter der er
/// steht.
```

## L708-714 · `Inline(String, bool, u32),`

```
/// Quelltext und ob `type="module"` daransteht. Die Fahne gehoert
/// HIERHER und wird nicht spaeter geraten: ein Modul ohne `import` parst
/// auch als Skript, haette dann aber den falschen Bereich und das falsche
/// `this` — und zwar still.
/// Der dritte Wert ist der KNOTEN des `<script>`. Er ist
/// `document.currentScript`, und ohne ihn findet ein Turbopack-Buendel
/// seinen eigenen Pfad nicht.
```

## L719-727 · `fn is_root_element(i: &mut Interp, t: &Value) -> bool {`

```
/// ALLE Skripte der Seite, in Dokumentreihenfolge — eingebettete wie externe.
///
/// Die Reihenfolge ist die des Quelltextes, und beak fuehrt sie auch so aus.
/// Das ist die Bedeutung von `defer` und NICHT die eines blockierenden
/// `<script>` mitten im Koerper: ein Browser wuerde ein klassisches Skript
/// ausfuehren, BEVOR er weiterparst, und `document.write` haengt davon ab.
/// beak hat das Dokument schon fertig, wenn es hier ankommt — also verhaelt
/// sich alles wie `defer`. Bewusst, und es deckt alles ausser `document.write`.
/// Ist `t` das Wurzelelement (`<html>`)?
```

## L733-738 · `fn is_scrolling_root(i: &mut Interp, t: &Value) -> bool {`

```
/// Rollt DIESES Element die Seite? Das Wurzelelement und der `<body>` tun
/// es; jedes andere hat in beak keinen eigenen Rollkasten.
///
/// `document.scrollingElement` nennt dasselbe Element, und beide Antworten
/// muessen dieselbe sein: eine Seite liest `scrollingElement` und schreibt
/// dann dessen `scrollTop`.
```

## L745-749 · `fn scroll_area(i: &Interp, t: &Value) -> Option<(f64, f64)> {`

```
/// Die ROLLFLAECHE eines gewoehnlichen Elements: sein Polsterkasten,
/// vereinigt mit den Rahmenkaesten aller Nachfahren.
///
/// Genau das fragt eine Seite, wenn sie `scrollHeight` liest — „passt der
/// Inhalt in den Kasten?". Ohne die Nachfahren waere die Antwort immer ja.
```

## L755-760 · `let (bl, bt) = (borders.0 / 2.0, borders.1 / 2.0);`

```
// Der Polsterkasten ist der Boden: `scrollHeight` ist nie kleiner als
// `clientHeight`. Gemessen wird von der POLSTERkante aus, also nur den
// Rahmen hinein — und wir fuehren je Achse nur die SUMME, also ist die
// halbe die Schaetzung fuer die eine Seite. Bei gleichen Rahmen ist sie
// exakt; ein Rahmen, der links und rechts verschieden ist, verschiebt sie
// um wenige Pixel. Gegen die 0, die vorher dastand, ist das keine Frage.
```

## L779-781 · `fn scroll_args(i: &mut Interp, a: &[Value]) -> C<(Option<f64>, Option<f64>)> {`

```
/// Die Argumente von `scrollTo`/`scrollBy`: entweder zwei Zahlen oder ein
/// Gegenstand mit `left`/`top`. Was fehlt, bleibt `None` und laesst die
/// Achse in Ruhe — `scrollTo({ top: 0 })` darf nicht waagerecht springen.
```

## L799 · `fn viewport_num(i: &mut Interp, key: &str) -> Value {`

```
/// Ein Fenstermass, so wie `set_viewport` es abgelegt hat.
```

## L805 · `fn viewport_f(i: &mut Interp, key: &str) -> f64 {`

```
/// Dasselbe als Zahl.
```

## L819-828 · `if !module && n.attr("nomodule").is_some() { continue; }`

```
// `nomodule` heisst „nur fuer einen Browser OHNE Module" (HTML
// §4.12.1). beak hat Module, also gehoert dieser Zweig UEBERSPRUNGEN
// — und das ist keine Ersparnis, sondern Richtigkeit: eine Seite
// liefert beide Zweige aus, und wer beide faehrt, laesst zwei
// Fassungen derselben Bibliothek um denselben globalen Namen
// streiten. Auf der DDG-Ergebnisseite sind das 2 MB, auf der
// Startseite ein core-js-Bundle, das beaks eingebaute Zusagen
// ERSETZT ([[feedback_a_polyfill_replaces_what_it_judges_broken]]).
// Auf einem Modulskript wird das Attribut laut Spezifikation
// ignoriert.
```

## L842-843 · `fn script_type_is_js(n: &DomNode) -> bool {`

```
/// Ein `type`, das nicht JavaScript meint (`application/json`,
/// `text/template`), ist Nutzlast und kein Programm.
```

## L854-859 · `pub fn inline_scripts(d: &Doc) -> Vec<String> {`

```
/// Die Inhalte aller `<script>`-Elemente OHNE `src`, in Dokumentreihenfolge.
///
/// Nur die eingebetteten: ein `src` muesste geholt werden, und die Reihenfolge
/// zwischen geholten und eingebetteten Skripten ist eine eigene Frage
/// (`defer`, `async`, und was ein `document.write` dazwischen anrichtet).
/// Bewusst der kleinere, ehrliche Anfang.
```

## L874-881 · `fn matches_simple(d: &Doc, id: u32, sel: &str) -> bool {`

```
// ── Selektoren ──────────────────────────────────────────────────────────────
//
// Eine EIGENE, kleine Auswertung — nicht die aus `css.rs`. Die passt auf
// beaks `Element` und nicht auf die Arena, und sie hierher zu ziehen waere der
// zweite Umbau in einem Schritt. Abgedeckt ist, was echter Code fast immer
// benutzt: `tag`, `#id`, `.class`, `[attr]`, `[attr=wert]`, beliebig
// kombiniert, dazu Nachfahren (Leerzeichen), Kind (`>`) und Listen (`,`).
// Was fehlt, faellt als NICHT GETROFFEN auf, nicht als falscher Treffer.
```

## L888 · `let tag_end = rest.find(['.', '#', '[']).unwrap_or(rest.len());`

```
// Fuehrender Typselektor.
```

## L920-921 · `fn matches_compound(d: &Doc, id: u32, sel: &str) -> bool {`

```
/// Ein zusammengesetzter Selektor, von rechts nach links geprueft — so herum,
/// weil der rechte Teil den Kandidaten schon festlegt.
```

## L968 · `use super::interp::C;`

```
// ── Die JS-Seite ────────────────────────────────────────────────────────────
```

## L972-974 · `const SLOT: &str = "__node";`

```
/// Der Index im Huellobjekt. Nicht aufzaehlbar und nicht konfigurierbar — ein
/// Skript, das ueber die Eigenschaften eines Elements laeuft, darf ihn nicht
/// sehen.
```

## L984-989 · `fn target_node(i: &mut Interp, v: &Value) -> C<u32> {`

```
/// Wie `node_of`, aber `window` zaehlt als das DOKUMENT.
///
/// `window.addEventListener("click", …)` ist die haeufigste Anmeldung
/// ueberhaupt, und im Browser bekommt das Fenster blasende Ereignisse als
/// LETZTES. Der Wurzelknoten steht in jeder Zustellkette genau dort — also
/// ist er die richtige Adresse, nicht eine Naeherung.
```

## L991-1004 · `if matches!(v, Value::Undefined | Value::Null) {`

```
// **Kein `this` heisst das GLOBALE Objekt, nicht „kein Ziel".** WebIDL
// §3.7.4 sagt es woertlich: ist der `this`-Wert null oder undefined,
// tritt das globale Objekt an seine Stelle — und erst danach wird
// geprueft, ob das die Schnittstelle ueberhaupt erfuellt. Fuer
// `EventTarget` erfuellt `window` sie, also traegt genau diese Regel das
// haeufigste Idiom im Web: `addEventListener("resize", f)` OHNE Empfaenger.
// Ein blanker Aufruf uebergibt laut Sprachkern `undefined` (der globale
// Bereich ist ein Umgebungssatz, kein Eigenschaftsbezug), und beak machte
// daraus einen TypeError — auf DuckDuckGos Ergebnisseite starb daran der
// Zeitgeber, der React einhaengt, und die Seite blieb leer.
//
// `node_of` bekommt die Regel NICHT: `window` ist kein `Node`, ein
// blankes `appendChild(x)` muss weiter werfen — auch das steht so in
// derselben Vorschrift.
```

## L1016 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Welches Stueck eines Dokuments gesucht ist.
```

## L1020-1026 · `fn doc_part(i: &mut Interp, this: &Value, part: DocPart) -> C<Option<u32>> {`

```
/// `documentElement` / `head` / `body` — von DIESEM Dokumentknoten aus.
///
/// Fuer das Hauptdokument steht die Antwort gemerkt in `Doc`; fuer jedes
/// andere (`implementation.createHTMLDocument`) wird gelaufen. Zwei Wege und
/// nicht einer, weil der gemerkte auch dann noch stimmt, wenn eine Seite
/// ihren Rumpf umbaut — und weil ein Umbau des Hauptwegs hier nichts
/// gewinnen und alles riskieren wuerde.
```

## L1033 · `let root = d.nodes.get(id as usize)`

```
// Die Wurzel ist das erste Element unter dem Dokumentknoten.
```

## L1044-1060 · `#[derive(Clone, Copy, PartialEq)]`

```
// ── ResizeObserver + IntersectionObserver ───────────────────────────────
//
// **Beide haengen an derselben Sache: der Geometrie nach dem Layout.** Der
// Wirt reicht sie mit `Interp::set_geometry` ein, und genau dort werden die
// Beobachter ausgewertet — nicht in einem Zeitgeber, der raet, wann sich
// etwas bewegt haben koennte.
//
// Im Aufrufzensus stehen sie mit 429 (`IntersectionObserver`) und 353
// (`ResizeObserver`) Aufrufen als P4 und P5. Was sie tragen, ist der halbe
// moderne Web-Werkzeugkasten: verzoegert geladene Bilder, unendliche Listen,
// klebende Kopfzeilen, Diagramme, die sich an ihren Kasten anpassen.
//
// **Gemessen wird beim Beobachten, gemeldet spaeter.** Ein Eintrag haelt die
// Kaesten, wie sie ZUM ZEITPUNKT der Beobachtung standen; ihn beim Zustellen
// neu zu rechnen hiesse, dem Rueckruf Zahlen aus einer anderen Runde zu
// geben — und zwischen Beobachtung und Zustellung liegt ein Rueckruf eines
// anderen Beobachters, der den Baum aendern darf.
```

## L1062 · `#[derive(Clone, Copy, PartialEq)]`

```
/// Welchen Kasten ein `ResizeObserver` meldet.
```

## L1066 · `pub struct ResizeReg {`

```
/// Eine Anmeldung eines `ResizeObserver`.
```

## L1068-1069 · `pub target: u32,`

```
/// Der KNOTEN, nicht sein Layout-`seq`: das Layout vergibt `seq` bei
/// jedem Lauf neu, und eine Anmeldung ueberlebt jedes Layout.
```

## L1072-1075 · `pub last: Option<(f64, f64)>,`

```
/// Zuletzt GEMELDETE Groesse. `None` heisst „noch nie" — und der erste
/// Lauf meldet immer, so wie die Spezifikation es verlangt: wer
/// `observe` ruft, bekommt die aktuelle Groesse, nicht erst die naechste
/// Aenderung.
```

## L1079 · `pub struct ResizeEntry {`

```
/// Was ein `ResizeObserverEntry` sagt — beim Beobachten gerechnet.
```

## L1082 · `pub content: (f64, f64),`

```
/// Inhaltskasten: Breite, Hoehe.
```

## L1084 · `pub border: (f64, f64),`

```
/// Rahmenkasten: Breite, Hoehe.
```

## L1095 · `pub struct InterReg {`

```
/// Eine Anmeldung eines `IntersectionObserver`.
```

## L1098-1101 · `pub band: i32,`

```
/// Der Schwellenindex von letztem Mal: wieviele Schwellen das Verhaeltnis
/// erreicht hat. Gemeldet wird, wenn sich DIESE Zahl aendert — nicht
/// jedes Pixel, sonst waere jeder Bildlauf ein Rueckrufgewitter.
/// `-1` heisst „noch nie gemeldet".
```

## L1105 · `pub struct InterEntry {`

```
/// Was ein `IntersectionObserverEntry` sagt.
```

## L1119-1121 · `pub root: Option<u32>,`

```
/// `None` heisst: das Sichtfeld. Ein anderes Element als Wurzel ist
/// erlaubt und wird hier genauso behandelt — sein Kasten ist dann der
/// Ausschnitt.
```

## L1123-1124 · `pub margin_src: Rc<str>,`

```
/// Der ungelesene `rootMargin`-Text, damit der Getter ihn zurueckgeben
/// kann, ohne aus vier Zahlen wieder Text zu raten.
```

## L1126-1127 · `pub margin: (f64, f64, f64, f64),`

```
/// `rootMargin` in Pixeln, oben/rechts/unten/links. Prozente werden beim
/// Anmelden aufgeloest; die Spezifikation erlaubt beides.
```

## L1134-1135 · `fn node_seq(i: &Interp, id: u32) -> Option<u32> {`

```
/// Der Layout-`seq` eines Knotens — dieselbe Auskunft wie `layout_seq`, nur
/// von der id aus. Die Beobachter kennen ihre Ziele als Knoten.
```

## L1141-1153 · `fn node_box(i: &Interp, id: u32) -> Option<((f64, f64, f64, f64), (f64, f64), (f64, f64))> {`

```
/// Der Rahmenkasten eines Knotens in FENSTERkoordinaten — die Vereinigung
/// seiner Fragmente, genau wie `getBoundingClientRect`, dazu die Rahmen- und
/// Polstersummen des ersten Fragments, GETRENNT.
///
/// Getrennt, weil zwei Fragen daran haengen und sie verschiedene Kanten
/// meinen: `contentRect` will beide abziehen, `scrollHeight` misst ab der
/// POLSTERkante und zieht nur den Rahmen ab. Zusammengefasst waren sie
/// einmal, und der Unterschied war 5 px, die kein Test bemerkt haette, wenn
/// er nicht beides in demselben Kasten gehabt haette.
///
/// Dieselbe Rechnung wie `elem_rect`, nur ohne den Umweg ueber einen
/// JS-Wert. Zwei Rechenwege waeren zwei Wahrheiten ueber denselben Kasten
/// ([[feedback_intrinsic_shared_path]]).
```

## L1176-1178 · `fn parse_root_margin(s: &str, vw: f64, vh: f64) -> (f64, f64, f64, f64) {`

```
/// `rootMargin`: ein bis vier CSS-Laengen, oben/rechts/unten/links wie bei
/// `margin`. Prozente stehen zur AUSSCHNITTgroesse — waagerecht zur Breite,
/// senkrecht zur Hoehe, so wie bei jedem anderen Rand auch.
```

## L1187-1189 · `t.parse::<f64>().ok().filter(|v| *v == 0.0).unwrap_or(0.0)`

```
// Eine blanke Zahl ist KEINE Laenge (nur `0` waere eine), und eine
// Einheit, die wir nicht kennen, auch nicht. Beides wird 0 statt
// geraten.
```

## L1203-1204 · `pub fn eval_box_observers(i: &mut Interp) {`

```
/// Beide Beobachter auswerten. Gerufen aus `Interp::set_geometry` — dem
/// einen Moment, in dem der Wirt sagt „so steht die Seite jetzt".
```

## L1206 · `for n in 0..i.resize_obs.len() {`

```
// ── ResizeObserver ───────────────────────────────────────────────────
```

## L1217-1218 · `if last != Some(seen) {`

```
// Ein Beobachter meldet AENDERUNGEN — und beim ersten Mal die
// Lage, wie sie ist.
```

## L1227 · `let (vw, vh) = i.viewport;`

```
// ── IntersectionObserver ─────────────────────────────────────────────
```

## L1231-1232 · `let root = match i.inter_obs[n].root {`

```
// Der Ausschnitt: das Sichtfeld oder der Kasten der Wurzel, in
// beiden Faellen um `rootMargin` gedehnt.
```

## L1257-1259 · `let inside = tx >= rx0 && tx <= rx1 && ty >= ry0 && ty <= ry1;`

```
// Ein Kasten ohne Flaeche (eine leere Zeile, ein umbrochener
// Inline-Kasten ohne Breite) hat kein Verhaeltnis — er schneidet,
// wenn er im Ausschnitt LIEGT. Sonst waere 0/0 die Antwort.
```

## L1266-1268 · `let hits = i.inter_obs[n].thresholds.iter()`

```
// Wieviele Schwellen erreicht sind. Die Schwelle 0 gilt erst als
// erreicht, wenn ueberhaupt geschnitten wird — sonst waere jedes
// Element von Anfang an „ueber 0".
```

## L1287 · `fn build_resize_entry(i: &mut Interp, e: &ResizeEntry) -> Value {`

```
/// Aus einem `ResizeEntry` das JS-Objekt bauen.
```

## L1291-1294 · `let rect = rect_obj(i, Some((0.0, 0.0, e.content.0, e.content.1)));`

```
// `contentRect` steht im Polsterkasten: x/y sind die Polsterung links
// und oben. Wir fuehren nur die SUMMEN, also die halbe — richtig fuer
// gleichmaessige Polsterung und nie schlechter als die 0, die vorher
// dagestanden haette.
```

## L1300-1303 · `i.new_array(alloc::vec![Value::Obj(s)])`

```
// Eine Liste, weil ein Element in einem fragmentierten Kasten
// mehrere Groessen haette. Wir haben immer genau eine — die Liste
// ist trotzdem die richtige Form, denn Seitencode schreibt
// `entry.contentBoxSize[0].inlineSize`.
```

## L1314-1315 · `b.define(SYM_TO_STRING_TAG, Prop::tag(Value::str("ResizeObserverEntry")));`

```
// Wir kennen kein Geraetepixel-Raster, also ist die Liste leer statt
// erfunden.
```

## L1323 · `fn build_inter_entry(i: &mut Interp, e: &InterEntry) -> Value {`

```
/// Aus einem `InterEntry` das JS-Objekt bauen.
```

## L1343-1347 · `pub fn deliver_box_observers(i: &mut Interp) -> bool {`

```
/// Der Kontrollpunkt: wer etwas in der Schlange hat, wird gerufen. Neben
/// `deliver_mutations` und aus demselben Grund dort — nach jedem
/// Einstiegspunkt, nicht in einem Zeitgeber.
///
/// Liefert true, wenn ein Rueckruf gelaufen ist.
```

## L1379-1380 · `pub struct MutReg {`

```
/// Eine Anmeldung eines `MutationObserver`: WAS er an WELCHEM Knoten sehen
/// will (DOM §4.3.1 „registered observer").
```

## L1383-1384 · `pub slot: usize,`

```
/// Der Platz dieser Anmeldung in `Doc::observed` — das Bit, das eine
/// Aenderung gesetzt hat, wenn sie diesen Knoten betraf.
```

## L1392 · `pub filter: Option<Vec<Rc<str>>>,`

```
/// `attributeFilter` — nur diese Attribute. `None` heisst „alle".
```

## L1396-1399 · `pub struct MutObs {`

```
/// Ein angemeldeter `MutationObserver`.
///
/// Er liegt im `Interp` und nicht im `Doc`, weil er einen JS-Rueckruf haelt:
/// das Dokument wird bei jeder Navigation neu gebaut, der Realm nicht.
```

## L1401-1402 · `pub js: Gc,`

```
/// Das JS-Objekt — die IDENTITAET. Ueber sie findet `observe` den
/// richtigen Eintrag wieder.
```

## L1409 · `fn sync_observing(i: &mut Interp) {`

```
/// Sieht noch jemand zu? Wenn nicht, hoert der Baum auf aufzuzeichnen.
```

## L1411-1412 · `let mut obs: Vec<(u32, bool)> = Vec::new();`

```
// Die Anmeldungen bekommen ihre Plaetze — und der Baum die Liste, die er
// beim Aufzeichnen braucht.
```

## L1420-1422 · `None => { r.slot = usize::MAX; voll = true; }`

```
// **Mehr als 64 verschiedene beobachtete Knoten.** Gesagt
// statt verschluckt: die Maske ist ein `u64`, und eine
// Anmeldung ohne Platz meldet nichts.
```

## L1439-1443 · `fn matches_reg(m: &Mutation, r: &MutReg) -> bool {`

```
/// Passt diese Aenderung zu dieser Anmeldung?
///
/// Die ZUGEHOERIGKEIT steht schon fest — sie wurde beim Aendern gerechnet
/// (`Mutation::hits`). Hier faellt nur noch, was diese Anmeldung inhaltlich
/// nicht sehen will.
```

## L1460-1464 · `pub fn collect_mutations(i: &mut Interp) {`

```
/// Die Rohaufzeichnung des Baums auf die Beobachter verteilen.
///
/// **Erst hier wird gefiltert, nicht beim Aufzeichnen.** Der Baum weiss
/// nicht, wer zusieht, und soll es nicht wissen muessen; und dieselbe
/// Aenderung kann an mehrere Beobachter gehen.
```

## L1478-1480 · `if o.regs.iter().any(|r| matches_reg(m, r)) {`

```
// Der Beobachter bekommt die Aenderung EINMAL, auch wenn zwei
// seiner Anmeldungen passen — sonst saehe eine Seite, die
// Elter und Kind beobachtet, alles doppelt.
```

## L1482-1484 · `let want_old = o.regs.iter().any(|r| matches_reg(m, r) && match m.kind {`

```
// `oldValue` gibt es nur, wenn die Anmeldung danach
// gefragt hat. Ihn immer mitzuliefern waere bequem und
// falsch: Seiten unterscheiden `null` von `""`.
```

## L1502 · `fn build_record(i: &mut Interp, m: &Mutation) -> C<Value> {`

```
/// Aus einer Aenderung ein `MutationRecord` bauen.
```

## L1527-1528 · `b.define("attributeNamespace", Prop::data(Value::Null));`

```
// Ein Namensraum, den wir nicht fuehren, ist `null` — und `null` ist
// hier die richtige Antwort, nicht eine fehlende.
```

## L1537-1540 · `pub fn deliver_mutations(i: &mut Interp) -> bool {`

```
/// Der Kontrollpunkt: einsammeln, und wer etwas hat, wird gerufen.
///
/// Liefert true, wenn ein Rueckruf gelaufen ist — der Rufer muss dann noch
/// einmal vorbeikommen, weil ein Beobachter im Rueckruf den Baum aendern darf.
```

## L1555-1557 · `if let Err(e) = i.call(&cb, this.clone(), &[arr, this]) {`

```
// Wirft der Rueckruf, ist das SEIN Fehler und nicht das Ende der
// Zustellung: die anderen Beobachter bekommen ihre Meldungen
// trotzdem, so wie im Browser.
```

## L1566 · `pub fn wrap(i: &mut Interp, id: u32) -> Value {`

```
/// Das Huellobjekt eines Knotens — einmal gebaut, dann behalten.
```

## L1600-1602 · `fn adjacent_spot(i: &Interp, id: u32, pos: &str) -> Option<(u32, Option<u32>)> {`

```
/// Die vier Stellen von `insertAdjacent*` als `(Elter, davor)`. `None`, wenn
/// die Stellenangabe keine der vier ist — oder wenn `beforebegin`/`afterend`
/// an einem Knoten ohne Elter verlangt wird, wo es nichts einzusetzen gibt.
```

## L1619-1620 · `fn nodes_equal(d: &Doc, x: u32, y: u32) -> bool {`

```
/// `isEqualNode`, rekursiv. Attribute werden als MENGE verglichen: die
/// Spezifikation sagt ausdruecklich, dass ihre Reihenfolge nichts bedeutet.
```

## L1651-1652 · `macro_rules! with_node {`

```
/// Lesender Zugriff auf einen Knoten, ohne die Ausleihe ueber einen Aufruf
/// hinweg zu halten — jede Abfrage kopiert, was sie braucht.
```

## L1662-1664 · `const EV_TYPE: &str = "__evtype";`

```
/// Die Schlitze eines Ereignisses. Nicht aufzaehlbar und mit `__` davor:
/// ein `for (k in e)` einer Seite darf sie nicht sehen, und `e.type` kommt
/// vom Prototyp, nicht von der Instanz.
```

## L1678-1680 · `macro_rules! ev_getter {`

```
/// Ein Getter, das einen festen Schlitz liest. Als Makro, weil ein
/// eingebautes Getter ein FUNKTIONSZEIGER ist: er faengt nichts ein, also
/// muss der Schlitzname im Rumpf stehen und nicht in einer Variablen.
```

## L1690-1696 · `fn event_of_kind(i: &mut Interp, iface: &str, a: &[Value]) -> C<Gc> {`

```
/// Ein Ereignis einer ART bauen, wie `new KeyboardEvent("keydown", {...})` es
/// tut: Prototyp aus dem globalen Namen, Basisfelder aus dem Wörterbuch, und
/// danach die Felder, die genau diese Art fuehrt.
///
/// Ein Woerterbuchfeld, das fehlt, bekommt den Vorgabewert der Spezifikation
/// — nicht `undefined`. Eine Seite, die `e.clientX + 1` rechnet, bekaeme sonst
/// `NaN`, und das sieht aus wie ein Rechenfehler der Seite.
```

## L1711 · `for (k, slot) in [("altKey", "__evalt"), ("ctrlKey", "__evctrl"),`

```
// Die vier Umschalter und `detail`/`view` hat jede Art unter `UIEvent`.
```

## L1772-1774 · `fn build_event(i: &mut Interp, proto: Gc, kind: &str, trusted: bool) -> Gc {`

```
/// Ein Ereignisobjekt mit gesetzten Schlitzen. `trusted` unterscheidet, was
/// beak selbst zustellt, von dem, was die Seite mit `dispatchEvent` schickt —
/// Seiten fragen es ab, und ein festes `true` waere gelogen.
```

## L1796-1801 · `fn init_event_fields(ev: &Gc, kind: &str, bubbles: bool, cancelable: bool) {`

```
/// Die Felder, die `initEvent` setzt — als Rust-Funktion, damit
/// `initCustomEvent` sie NICHT ein zweites Mal aufschreibt
/// ([[feedback_a_copy_is_a_second_semantics_waiting]]).
///
/// DOM §initEvent setzt die Abbruch-Fahnen ausdruecklich ZURUECK: dasselbe
/// Objekt darf ein zweites Mal zugestellt werden.
```

## L1816 · `fn apply_event_init(i: &mut Interp, ev: &Gc, init: &Value) -> C<()> {`

```
/// `new Event(art, {bubbles, cancelable})` — das zweite Argument.
```

## L1829-1830 · `fn same_fn(a: &Value, b: &Value) -> bool {`

```
/// Sind das dieselbe Funktion? Identitaet, nicht Gleichheit — genau das
/// fragt `removeEventListener`.
```

## L1835-1838 · `fn ancestors(i: &Interp, id: u32) -> Vec<u32> {`

```
/// Die Zustellkette fuer einen Knoten: von der Wurzel bis zu ihm.
///
/// Dieselbe Reihenfolge, in der beak sie aus dem LAYOUT baut — aussen zuerst,
/// Ziel zuletzt. Wer das dreht, dreht die Blasenrichtung.
```

## L1851-1856 · `fn style_decls(text: &str) -> Vec<(String, String)> {`

```
/// Die Erklaerungen eines `style`-Attributs, in der Reihenfolge des Textes.
///
/// Ein eigener kleiner Leser und nicht der aus `css`: der hier bekommt genau
/// das zurueckzugeben, was ein Skript hineingeschrieben hat — der Kaskadenleser
/// wirft ungueltige Erklaerungen weg, und dann laese `el.style.foo` etwas
/// anderes als das eben Geschriebene.
```

## L1877-1879 · `const COMPUTED: &str = "__computed";`

```
/// Der Schnappschuss, den `getComputedStyle` hinterlegt. Ist er da, liest die
/// Sicht IHN statt des `style`-Attributs — dieselben Zugriffsfunktionen, zwei
/// Quellen, und keine zweite Maschinerie.
```

## L1882 · `fn style_text(i: &Interp, this: &Value) -> String {`

```
/// Der Deklarationstext hinter einer `CSSStyleDeclaration`.
```

## L1895 · `fn node_of_ref(_i: &Interp, v: &Value) -> Result<u32, ()> {`

```
/// Wie `node_of`, aber ohne zu werfen — ein Schnappschuss hat keinen Knoten.
```

## L1909-1910 · `None => Value::str(""),`

```
// Nicht gesetzt ist die LEERE Zeichenkette, nicht `undefined` — so
// steht es in der Spezifikation, und Seiten pruefen darauf.
```

## L1915-1920 · `fn style_set(i: &mut Interp, id: u32, css: &str, val: &str) {`

```
/// Eine Erklaerung setzen oder (bei leerem Wert) entfernen.
///
/// Das Ergebnis landet im `style`-ATTRIBUT, nicht in einer Nebenablage: die
/// Kaskade liest das Attribut, also wirkt `el.style.display = "none"` damit
/// wirklich — vorher lief die Zuweisung ins Leere und die Seite blieb stehen,
/// wie sie war.
```

## L1933-1935 · `macro_rules! style_prop {`

```
/// Ein Eigenschaftspaar auf `CSSStyleDeclaration.prototype`. Als Makro aus
/// demselben Grund wie `ev_getter`: ein eingebautes Getter ist ein
/// Funktionszeiger und faengt nichts ein.
```

## L1956-1957 · `macro_rules! handler_prop {`

```
/// Ein Behandler als Eigenschaft: `el.onclick`. Als Makro, weil das Getter
/// ein Funktionszeiger ist und die Ereignisart im Rumpf stehen muss.
```

## L1964-1966 · `Ok(inline_handler(i, id, $kind)?.unwrap_or(Value::Null))`

```
// Steht nur das Attribut da, gibt der Browser trotzdem eine
// FUNKTION zurueck — also uebersetzen wir es hier, wie beim
// Ausloesen auch.
```

## L1977-1979 · `d.has_listeners = true;`

```
// Ohne diese Zeile zeichnet das Layout keine
// Treffer-Kaesten auf, und der Klick findet nichts —
// dieselbe Falle wie bei `addEventListener`.
```

## L1990-1992 · `macro_rules! attr_prop {`

```
/// Ein Feld, das auf einem ATTRIBUT sitzt: `a.href`, `img.src`, `el.title`.
/// Lesen gibt die leere Zeichenkette, wenn es das Attribut nicht gibt —
/// nicht `undefined`, denn darauf ruft Seitencode `.indexOf`.
```

## L2010-2013 · `macro_rules! attr_prop_null {`

```
/// Wie `attr_prop!`, aber `null` statt der leeren Zeichenkette, wenn das
/// Attribut fehlt. So spiegeln die ARIA-Felder (ARIA 1.2 §9): sie sind
/// `DOMString?`, und `el.ariaHidden === null` ist die Frage „steht da
/// ueberhaupt etwas".
```

## L2023-2026 · `None | Some(Value::Null) | Some(Value::Undefined) => {`

```
// `el.ariaHidden = null` NIMMT das Attribut weg. Es auf die
// Zeichenkette "null" zu setzen waere die naheliegende
// Bequemlichkeit und ein sichtbarer Fehler: `aria-hidden="null"`
// ist wahr, weil jeder Wert ausser "false" wahr ist.
```

## L2043-2045 · `macro_rules! num_attr_prop {`

```
/// Ein Feld, das eine ZAHL auf einem Attribut ist: `el.tabIndex`. Fehlt das
/// Attribut oder ist es keine Zahl, gilt `default` — bei `tabIndex` ist das
/// nicht 0, sondern -1 fuer alles, was nicht von sich aus anspringbar ist.
```

## L2065-2067 · `macro_rules! bool_attr_prop {`

```
/// Ein Feld, das die ANWESENHEIT eines Attributs ist: `el.hidden`,
/// `script.async`. Der Wert des Attributs zaehlt nicht — `hidden="false"`
/// versteckt trotzdem, so steht es im HTML.
```

## L2088-2089 · `fn fragment_nodes(root: &crate::dom::Element) -> Vec<&crate::dom::Node> {`

```
/// Die Knoten eines geparsten BRUCHSTUECKS: der `<html>`/`<head>`/`<body>`-
/// Rahmen, den der Dokumentparser immer baut, faellt weg.
```

## L2109-2115 · `fn layout_seq(i: &Interp, this: &Value) -> Option<u32> {`

```
/// Die Nummer, unter der das LAYOUT dieses Element kennt.
///
/// Zwei Zahlen kommen in Frage, und welche gilt, haengt am Zeitpunkt:
/// `to_dom` vergibt beim Zurueckschreiben frische `seq`, davor sind sie 0 und
/// das Layout stammt noch aus dem geparsten Baum — dessen Nummern stehen in
/// `src_seq`. „Nicht-null gewinnt" ist damit keine Heuristik, sondern die
/// Frage „hat schon einmal jemand zurueckgeschrieben?".
```

## L2122-2125 · `const FORCED_LAYOUT_CAP: u32 = 4;`

```
/// Der Deckel gegen Layout-Thrashing: so oft darf zwischen zwei Bildern des
/// Wirts erzwungen ausgelegt werden. Eine Seite, die in einer Schleife
/// schreibt und liest, erzwingt sonst je Durchlauf ein volles Layout — auf
/// DDGs Ergebnisseite sind das 51 ms.
```

## L2128-2141 · `fn ensure_box(i: &mut Interp, this: &Value) {`

```
/// **Vor jeder Kastenfrage: hat dieses Element schon einen Kasten?**
///
/// Wenn nicht, der Baum sich seit dem letzten Bild bewegt hat und das Element
/// AM Dokument haengt, dann ist die Null keine Antwort, sondern ein fehlendes
/// Bild — also wird jetzt ausgelegt.
///
/// **Die enge Fassung, und das ist Absicht.** Ein Browser rechnet bei JEDER
/// Lesung auf einem schmutzigen Baum neu. Hier wird nur nachgelegt, wenn gar
/// kein Kasten da ist. Gemessen auf DDGs Ergebnisseite und
/// `sandbox.nopeek.ch` deckt das alle Faelle ab, die heute falsch antworten
/// (4 von 219 bzw. 3 von 77 Lesungen, alle auf Elementen ohne Kasten);
/// Leseschleifen ueber bestehende Elemente kosten damit nichts. Der
/// Unterschied ist benannt, nicht versteckt: ein Element, das sich seit dem
/// letzten Bild BEWEGT hat, meldet weiter den alten Ort.
```

## L2144-2145 · `if let Some(seq) = layout_seq(i, this) {`

```
// Schon ein Kasten da? Dann ist nichts zu tun — der haeufigste Fall, und
// er muss billig bleiben.
```

## L2149-2151 · `if !i.doc.as_ref().is_some_and(|d| d.dirty) { return }`

```
// Nur wenn der Baum sich bewegt hat. Ist er sauber, ist die Null die
// Wahrheit (`display:none`, ein leeres Inline) und ein Layout aendert
// daran nichts.
```

## L2153-2154 · `let Ok(id) = node_of_ref(i, this) else { return };`

```
// Und nur fuer einen Knoten AM Dokument. Ein losgeloester bekaeme auch
// nach dem Auslegen keinen Kasten — wir wuerden je Lesung neu rechnen.
```

## L2170-2172 · `fn force_layout(i: &mut Interp) {`

```
/// Den Haken des Wirts rufen — einmal, unter dem Deckel, und mit einer Zeile
/// auf der Konsole, wenn der Deckel greift. Ein Deckel, der stillschweigend
/// eine falsche Zahl liefert, ist schlimmer als keiner.
```

## L2189-2193 · `fn elem_rect(i: &Interp, this: &Value) -> Option<(f64, f64, f64, f64)> {`

```
/// Der Rahmenkasten in FENSTERkoordinaten: `(x, y, w, h)`.
///
/// Ein Kasten kann in mehrere Fragmente zerfallen (ein Inline-Kasten je
/// Zeile) — `getBoundingClientRect` nennt deren Vereinigung, und das ist
/// genau das, was ein Browser dort auch liefert.
```

## L2209 · `fn elem_inner(i: &Interp, this: &Value) -> Option<(f64, f64)> {`

```
/// Der POLSTERkasten: Breite und Hoehe ohne die Rahmen.
```

## L2214-2215 · `let b = g.boxes.iter().find(|b| b.seq == seq)?;`

```
// Die Rahmen des ERSTEN Fragments: ein ueber Zeilen gebrochener Kasten
// zeichnet sie nur an seinen aeusseren Enden, und dort steht ohnehin 0.
```

## L2220-2222 · `fn rect_obj(i: &Interp, r: Option<(f64, f64, f64, f64)>) -> Gc {`

```
/// Ein `DOMRect`-artiger Gegenstand. `None` heisst „kein Kasten" und wird zu
/// lauter Nullen — dieselbe Antwort, die ein Browser fuer ein Element ohne
/// Kasten (`display:none`) gibt.
```

## L2233 · `enum Where { First, Last, Before, After }`

```
/// Wohin `append` & Co. einhaengen.
```

## L2236-2238 · `fn insert_all(i: &mut Interp, this: &Value, args: &[Value], w: Where) -> C<Value> {`

```
/// Der gemeinsame Rumpf von `append`/`prepend`/`before`/`after`. Ein
/// Argument, das kein Knoten ist, wird zum Textknoten — genau das
/// unterscheidet diese Familie von `appendChild`.
```

## L2268-2269 · `if let Some(d) = &mut i.doc { d.insert_maybe_fragment(parent, id, anchor); d.touch(); }`

```
// Der Anker bleibt derselbe: alles landet DAVOR, also stehen mehrere
// Argumente am Ende in der Reihenfolge, in der sie uebergeben wurden.
```

## L2276-2294 · `fn style_tree(i: &Interp) -> Option<alloc::rc::Rc<crate::dom::Dom>> {`

```
/// Den KASKADIERTEN Stil eines Elements als Deklarationstext.
///
/// Die Kaskade laeuft auf beaks Baum, nicht auf der Arena der Maschine — also
/// wird das Element ueber `src_seq` dort gesucht und die Kette von der Wurzel
/// herunter aufgeloest. Das kostet einen Lauf je Ebene (auf einer echten
/// Seite ein Dutzend), und zwar je Aufruf: `getComputedStyle` ist eine Frage
/// an den JETZIGEN Zustand, und ein Zwischenspeicher muesste wissen, wann er
/// falsch wird.
///
/// `None`, wenn kein Kontext eingereicht wurde oder das Element im Baum nicht
/// vorkommt (ein Skript hat es erst erzeugt) — dann bleibt es beim
/// Inline-Stil.
/// Der Baum, auf dem die Kaskade rechnet: der LEBENDE, aus `doc` gebaut und
/// nur dann neu gebaut, wenn `doc.version` sich bewegt hat.
///
/// Ein Skript, das eine Klasse setzt und dann misst, ist kein Randfall — und
/// aus einem Schnappschuss beantwortet, waeren es zwei Antworten auf dieselbe
/// Frage. Der Zwischenspeicher ist der Preis dafuer: EIN Aufbau je
/// Aenderungsschub, nicht je Abfrage.
```

## L2304-2311 · `fn computed_decls(i: &Interp, node: u32) -> Option<String> {`

```
/// `node` ist der ARENA-INDEX des Elements — dieselbe Zahl, die `live_dom`
/// als `seq` in den Baum schreibt.
/// Die block-artige Entsprechung eines Anzeigewerts (css-display-3 §2.7).
///
/// `list-item` bleibt stehen — es ist schon block-artig, und Chromium meldet
/// an einem `<li>` in einer Flex-Leiste auch `list-item`. Nachgemessen, nicht
/// vermutet: die erste Fassung machte `block` daraus und lag an neunzehn
/// Kaesten der Bootstrap-Galerie falsch.
```

## L2322-2327 · `let mut vars = crate::vars::VarMap::new();`

```
// Die Variablenkarte faehrt MIT. Ohne sie erreicht `:root`s Palette das
// Element nie — und ein Rahmenwerk, das seine ganze Skala ueber
// Variablen fuehrt (Tailwind: `font-size: var(--text-xs)`), bekaeme
// ueberall die Vorgabewerte zurueck. `getComputedStyle` haette dann eine
// andere Antwort gegeben als das Layout gemalt hat, und das ist die
// schlimmste Sorte Fehler: zwei Wahrheiten.
```

## L2331-2333 · `let (prev, count) = match k {`

```
// Geschwister zaehlen, damit `:nth-*` und `:first-child` stimmen —
// sonst haette der gerechnete Stil eine andere Kaskade gesehen als
// das Layout.
```

## L2350-2354 · `if matches!(parent.display, crate::style::Display::Flex`

```
// **Ein Flex- oder Rasterkind ist block-artig** (css-display-3 §2.7),
// und `layout_flex` rechnet auch genau damit. Ohne diese Zeile sagte
// `getComputedStyle` `inline` fuer einen Knopf, den das Layout als
// Block gemalt hat — auf der Bootstrap-Galerie 80 Kaesten, und jedes
// Mal zwei Wahrheiten zu derselben Frage.
```

## L2360-2364 · `if k == 0 {`

```
// `rem` rechnet gegen die WURZEL, und die steht erst fest, wenn sie
// aufgeloest ist. Das Layout setzt das direkt nach dem Wurzellauf;
// ohne die Zeile las `getComputedStyle` jedes `rem` gegen die
// Vorgabegroesse — `font-size: .75rem` kam als 16 px zurueck, waehrend
// das Layout 12 malte. Zwei Antworten auf dieselbe Frage.
```

## L2374 · `fn find_path<'a>(el: &'a crate::dom::Element, seq: u32,`

```
/// Den Weg von der Wurzel zu `seq` sammeln.
```

## L2392-2393 · `fn def_global(realm: &Realm, name: &str, f: NativeFn, len: usize, fp: &Gc) {`

```
/// Eine Funktion auf dem Fenster. `meth` legt sie auf einen Prototyp, hier
/// gehoert sie an den globalen Gegenstand selbst.
```

## L2420-2433 · `fn xpath_compiled(i: &mut Interp, src: &str) -> C<Rc<super::xpath::XPath>> {`

```
// ── XPath ───────────────────────────────────────────────────────────────────
//
// **Warum es das gibt, und warum in dieser Groesse.** htmx sucht seine
// `hx-on:`-Attribute mit einem XPath-Ausdruck, und es war das letzte der
// dreizehn Bibliotheksproben, das rot stand. Der Chromium-Zensus ueber zwoelf
// echte Zielseiten zaehlt dagegen NULL XPath-Aufrufe: das hier ist keine
// Web-Anforderung nach Aufrufzahl, sondern eine BIBLIOTHEKS-Anforderung.
//
// Diese Messung entscheidet die Form, nicht das Ob. Ein Sonderfall fuer htmx'
// einen Ausdruck waere ein Notnagel an der Stelle eines fehlenden Merkmals
// ([[feedback_a_workaround_is_the_wrong_answer_to_a_missing_capability]]);
// volles XPath 1.0 mit Namensraeumen waere Gold, nach dem niemand fragt.
// Gebaut ist die Sprache, die eine Seite wirklich schreibt — `js/xpath.rs`
// sagt, was fehlt.
```

## L2435 · `fn xpath_compiled(i: &mut Interp, src: &str) -> C<Rc<super::xpath::XPath>> {`

```
/// Den Ausdruck holen — einmal geparst, unter seinem Quelltext gemerkt.
```

## L2448-2449 · `Err(e) => Err(i.throw_kind("SyntaxError", &alloc::format!("XPath: {e}"))),`

```
// Ein unlesbarer Ausdruck ist ein FEHLER, keine leere Treffermenge:
// eine leere Menge sieht aus wie „nichts gefunden".
```

## L2454-2455 · `fn wrap_attr(i: &mut Interp, owner: u32, k: usize) -> Value {`

```
/// Ein Attributknoten als `Attr`-Objekt — dieselbe Form, die
/// `element.attributes` liefert.
```

## L2479 · `fn xpath_run(i: &mut Interp, src: &str, ctx: &Value, want: f64) -> C<Value> {`

```
/// `document.evaluate` / `XPathExpression.evaluate` — beide enden hier.
```

## L2485-2486 · `let (nodes, num, string, boolean) = {`

```
// Erst auswerten, dann huellen: die Auswertung leiht `i.doc` aus, das
// Huellen braucht `i` veraenderlich.
```

## L2497-2498 · `let ty = if want == 0.0 {`

```
// `ANY_TYPE` (0) meldet den natuerlichen Typ des Ergebnisses; sonst
// gilt, was der Aufrufer verlangt hat.
```

## L2530-2542 · `fn install_formdata(realm: &mut Realm) {`

```
/// `FormData` — die Buendelung eines Formulars fuer `fetch`.
///
/// **Der Weg, auf dem eine moderne Seite ein Formular abschickt.** Sie faengt
/// `submit` ab, baut `new FormData(form)` und schickt es selbst; ohne den
/// Namen wirft schon die Zeile, und die Seite bleibt stumm stehen.
///
/// Die Paare liegen als Feld auf dem Objekt, in Dokumentreihenfolge — dieselbe
/// Reihenfolge, die `forms::submit` fuer den eigenen Weg baut, und dieselben
/// Regeln fuer den erfolgreichen Wert: benannt, nicht abgeschaltet, und ein
/// Kaestchen nur, wenn es angehakt ist.
///
/// Dateien traegt es nicht: beak hat kein `multipart/form-data` (CONFORMANCE
/// sagt es), und ein `File`, das nichts enthaelt, waere schlechter als keins.
```

## L2563-2564 · `let button = tag == "button" || matches!(ty.as_str(), "submit" | "reset" | "button" | "image");`

```
// Ein Knopf ist nur erfolgreich, wenn er der ist, der
// abgeschickt hat — hier war das keiner.
```

## L2659 · `fn fd_pairs(i: &mut Interp, t: &Value) -> C<Vec<(alloc::string::String, alloc::string::String)>> {`

```
/// Die Paare eines `FormData` als Rust-Werte.
```

## L2681 · `let res_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── XPathResult ──────────────────────────────────────────────────────
```

## L2688-2690 · `for (name, v) in [`

```
// Die zehn Typkonstanten stehen im Browser auf BEIDEN — der Konstruktor
// ist die uebliche Schreibweise (`XPathResult.FIRST_ORDERED_NODE_TYPE`),
// die Instanz die seltenere.
```

## L2712-2713 · `None => Ok(Value::Null),`

```
// Erschoepft: `null`, und darauf endet die `while`-Schleife, mit
// der jeder Aufrufer darueber laeuft.
```

## L2735 · `let expr_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── XPathExpression ──────────────────────────────────────────────────
```

## L2752 · `let ev_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── XPathEvaluator ───────────────────────────────────────────────────
```

## L2763-2765 · `xpath_compiled(i, &src)?;`

```
// JETZT parsen, nicht erst beim Auswerten: `createExpression` ist die
// Stelle, an der ein Browser einen Syntaxfehler meldet, und eine Seite
// die ihren Ausdruck beim Laden baut soll ihn beim Laden hoeren.
```

## L2778-2779 · `meth(&ev_proto, "createNSResolver", |_, _, a| {`

```
// `createNSResolver` gibt es, damit der uebliche Vieraufruf nicht wirft;
// Namensraeume loest es nicht auf, und das steht in `js/xpath.rs`.
```

## L2786 · `pub fn install(realm: &mut Realm) {`

```
/// Baut `Node`/`Element`/`Document`-Prototypen und das globale `document`.
```

## L2789-2792 · `let event_target_proto = new_obj(Some(realm.object_proto.clone()));`

```
// `EventTarget` steht UNTER `Node`: `addEventListener` gehoert dorthin,
// nicht auf den Knoten. Sonst hat `window` es nicht — und `window
// .addEventListener` ist mit 33 360 Aufrufen der haeufigste DOM-Aufruf
// des ganzen Zielkorpus (`tools/jsscope/out/apicensus.json`).
```

## L2798-2799 · `let fragment_proto = new_obj(Some(node_proto.clone()));`

```
// `DocumentFragment` haengt an Node, nicht an Element — hier oben, weil
// die Suchfunktionen weiter unten auch auf ihm sitzen.
```

## L2802 · `getter(&node_proto, "nodeType", |i, t, _| with_node!(i, t, |n| Ok(Value::Num(n.kind))), &fp);`

```
// ── Node ─────────────────────────────────────────────────────────────
```

## L2844-2846 · `let old: Vec<u32> = d.nodes[id as usize].children.clone();`

```
// Alle Kinder weg, ein Textknoten hin. Die alten Knoten bleiben in
// der Arena liegen — sie sind nur nicht mehr verhaengt. Freigeben
// hiesse Indizes verschieben, und ein Handle darf nie verrutschen.
```

## L2857-2862 · `meth(&node_proto, "normalize", |i, t, _| {`

```
// `normalize` — benachbarte Textknoten verschmelzen, leere entfernen.
//
// Die Fritzbox ruft es am Ende JEDES Anhaengens. Ohne die Methode wirft
// dort „normalize is not a function", und zwar nachdem die Kinder schon
// dranhaengen: der Baum ist dann halb gebaut und die Meldung zeigt auf
// die falsche Stelle.
```

## L2914-2918 · `meth(&node_proto, "replaceChild", |i, t, a| {`

```
// `replaceChild(neu, alt)` — im Zensus null Aufrufe, und trotzdem gebaut:
// die Ausfallart ist der Punkt. Eine FEHLENDE Methode wirft und beendet
// das ganze Skript, und **lucide ersetzt damit jedes `<i data-lucide>`
// durch sein `<svg>`**. Auf sandbox.nopeek.ch starb daran die komplette
// Symbolschicht — keine Reiter-Symbole, keine Lupe, kein Themenschalter.
```

## L2927-2928 · `d.insert_maybe_fragment(p, new, Some(old));`

```
// Erst einsetzen, DANN entfernen: umgekehrt waere die Stelle weg,
// an der das Neue stehen soll, und es landete am Ende.
```

## L2935-2937 · `meth(&node_proto, "isEqualNode", |i, t, a| {`

```
// Strukturvergleich (DOM §4.4): gleicher Knotentyp, gleicher Name, dieselben
// Attribute (Menge und Werte, Reihenfolge egal) und dieselben Kinder in
// derselben Reihenfolge. NICHT dieselbe Identitaet — dafuer gibt es `===`.
```

## L2946 · `|i, t, _| with_node!(i, t, |n| Ok(if n.kind == ELEMENT_NODE || n.kind == DOCUMENT_NODE {`

```
// Ein Element HAT keinen Wert — `null` ist die Antwort, nicht "".
```

## L2959-2960 · `meth(&node_proto, "compareDocumentPosition", |i, t, a| {`

```
// Die Bitmaske aus der Spezifikation. Seiten benutzen sie fuer genau eine
// Frage — „liegt A vor B?" — und `& 4` ist die Art, sie zu stellen.
```

## L2969 · `if ax[0] != ay[0] { return Ok(Value::Num(1.0 + 2.0 + 32.0)) }   // DISCONNECTED`

```
// DISCONNECTED
```

## L2970 · `let mut k = 0;`

```
// Der erste Punkt, an dem die Wege sich trennen, entscheidet.
```

## L2973 · `if k == ax.len() { return Ok(Value::Num(16.0 + 4.0)) }          // CONTAINED_BY`

```
// CONTAINED_BY
```

## L2974 · `if k == ay.len() { return Ok(Value::Num(8.0 + 2.0)) }           // CONTAINS`

```
// CONTAINS
```

## L2991-2993 · `meth(&event_target_proto, "addEventListener", |i, t, a| {`

```
// Anmelden, aber noch nicht zustellen. Ein `addEventListener`, das WIRFT,
// beendet das Skript — eins, das die Anmeldung nur aufbewahrt, laesst es
// weiterlaufen. Die Zustellung setzt genau hier an.
```

## L3000 · `d.has_listeners = true;`

```
// Sobald EIN Behandler da ist, braucht das Layout Treffer-Kaesten.
```

## L3005-3007 · `meth(&event_target_proto, "removeEventListener", |i, t, a| {`

```
// `removeEventListener(art, f)` nimmt GENAU f weg, nicht alles dieser
// Art. Vorher fiel mit einem `resize`-Behandler jeder zweite mit ab —
// und eine Seite, die einen von dreien abmeldet, verlor alle drei.
```

## L3017-3019 · `meth(&event_target_proto, "dispatchEvent", |i, t, a| {`

```
// Die Seite loest selbst aus: `el.dispatchEvent(new Event("change"))`.
// Vorher gab das ein festes `true` zurueck, ohne einen Behandler zu
// rufen — eine Antwort, die aussieht wie eine Zustellung.
```

## L3030-3031 · `let chain = if bubbles { ancestors(i, id) } else { alloc::vec![id] };`

```
// Blast es nicht, ist die Kette genau ein Knoten lang — dann laeuft
// nur der Behandler am Ziel, und das ist der ganze Unterschied.
```

## L3037 · `getter(&element_proto, "tagName", |i, t, _| with_node!(i, t, |n| Ok(Value::string(n.tag.to_uppercase()))), &fp);`

```
// ── Element ──────────────────────────────────────────────────────────
```

## L3114-3115 · `let (parent, at) = match pos.as_str() {`

```
// Die vier Stellen der Spezifikation: vor/nach dem Element selbst,
// und ganz vorn/hinten in ihm.
```

## L3129-3131 · `meth(&element_proto, "insertAdjacentElement", |i, t, a| {`

```
// Dieselben vier Stellen, aber mit einem KNOTEN statt einer Zeichenkette.
// Wer `insertAdjacentHTML` hat und diese beiden nicht, hat die Familie
// halb — und die halbe Familie wirft dort, wo die andere Haelfte traegt.
```

## L3167-3178 · `meth(&element_proto, "getBoundingClientRect", |i, t, _| {`

```
// ── Geometrie ────────────────────────────────────────────────────────
//
// Bis 0.75.0 stand hier ueberall eine 0, und der Kommentar begruendete das
// damit, dass beak erst NACH den Skripten auslegt. Der Grund war einmal
// richtig und ist es seit der Ereigniszustellung nicht mehr: ein
// Klickbehandler laeuft auf einer fertig ausgelegten Seite.
//
// **Eine 0 war dabei das teuerste, was hier stehen konnte.** Sie wirft
// nicht, sie steht in keinem Log, sie sieht aus wie eine Antwort — der
// Tooltip landet in der Ecke, die Sichtbarkeitspruefung haelt alles fuer
// sichtbar. Im Aufrufzensus ist `getBoundingClientRect` mit 1125 Aufrufen
// der GROESSTE einzelne Posten, und er stand als „gedeckt" in der Bilanz.
```

## L3184-3185 · `meth(&element_proto, "getClientRects", |i, t, _| {`

```
// Die Fragmente einzeln — ein Inline-Kasten ueber drei Zeilen hat drei
// Rechtecke, und genau deshalb gibt es diese Funktion neben der oberen.
```

## L3207-3210 · `getter(&element_proto, "offsetTop", |i, t, _| {`

```
// `offsetTop`/`offsetLeft` gehen gegen den `offsetParent`, und den gibt es
// hier nicht. Gegen das DOKUMENT ist die naechstbeste Wahrheit und fuer
// die ueblichen Faelle (ein Element in einem nicht positionierten Rumpf)
// dieselbe Zahl. Benannt, damit niemand sie fuer exakt haelt.
```

## L3221-3233 · `getter(&element_proto, "clientWidth", |i, t, _| {`

```
// `clientWidth`/`clientHeight` sind der POLSTERkasten: der Rahmenkasten
// ohne die Rahmen. Die Summen faehrt `HoverBox` mit.
//
// **Ausser am WURZELELEMENT — dort sind sie die Sichtflaeche** (CSSOM
// View §4: „If the element is the root element … return the viewport
// width/height"). Das ist keine Feinheit: Googles Startseite misst damit
// das Fenster und laeuft nur weiter, wenn beide Werte wahr sind —
//
//     h = p.clientWidth; m = p.clientHeight;
//     if (h && m && …) { … "/client_204?…&biw=" + h + "&bih=" + m … }
//
// Mit 0 blieb der Block stehen, das Formular schickte `biw=&bih=`, und
// Google hielt uns fuer einen Browser ohne JavaScript.
```

## L3244-3267 · `getter(&element_proto, "scrollHeight", |i, t, _| {`

```
// ── Die Rollmasse ────────────────────────────────────────────────────
//
// **Sie waren da und antworteten 0** — 582 Aufrufe im Zensus, die
// groesste Position, die keine neue Schnittstelle braucht, sondern eine
// Leitung. Der Kommentar, der hier stand, war ehrlich: die Zahlen gab es
// im Layout nicht. Jetzt gibt es sie.
//
// **beak klemmt nichts ab.** `overflow: auto`/`scroll` schneidet hier
// nicht, es gibt keine Rollkaesten je Element (`layout.rs`: „we have no
// scroll containers"). Das ist keine Ausrede, sondern die Antwort:
//
// * `scrollTop`/`scrollLeft` sind an jedem gewoehnlichen Element **0**,
//   und das ist WAHR, nicht geraten — nichts an ihm ist weggerollt. Am
//   Wurzelelement und am `<body>` sind sie der Rollstand der SEITE, denn
//   die rollt.
// * `scrollHeight`/`scrollWidth` sind die Rollflaeche: der Polsterkasten,
//   vereinigt mit den Rahmenkaesten aller Nachfahren. Ein Element mit
//   `height: 100px` und hoeherem Inhalt meldet den Inhalt — genau
//   deswegen fragt eine Seite ueberhaupt.
// * Am Wurzelelement ist es die Rollflaeche des DOKUMENTS, und die sagt
//   das Layout (`Geometry::content`). Sie aus den Kaesten zu raten waere
//   eine zweite Wahrheit ueber dieselbe Zahl: ein Hintergrund oder ein
//   ueberlaufender Text haelt eine Seite rollbar, ohne einen Kasten zu
//   haben.
```

## L3306-3313 · `getter(&element_proto, "offsetParent", |i, t, _| {`

```
// `offsetParent`: der naechste POSITIONIERTE Vorfahr, sonst der `<body>`
// (CSSOM View §5). Die Ecke „positioniert" faehrt seit dieser Runde im
// Layoutkasten mit — sie sonst zu beantworten hiesse, fuer jeden
// Vorfahren die Kaskade neu aufzuloesen.
//
// `null` an einem Element ohne Kasten, am Wurzelelement und am `<body>`
// selbst. Das ist die Antwort, auf die eine Seite prueft, wenn sie
// fragt, ob ein Element ueberhaupt sichtbar ist.
```

## L3322-3323 · `if node_box(i, id).is_none() { return Ok(Value::Null) }`

```
// Ohne Kasten gibt es keinen Bezug — `display: none`, und genau das
// ist die uebliche Frage.
```

## L3336-3339 · `meth(&element_proto, "scrollTo", |i, t, a| {`

```
// Rollen auf Verlangen. **Die Engine rollt NICHT** — sie merkt sich, was
// die Seite wollte, und der Wirt holt es mit `take_scroll` ab. Dasselbe
// Muster wie bei den Keksen und der Navigation: die Engine hat kein
// Fenster und soll sich keines erfinden.
```

## L3354-3356 · `meth(&element_proto, "scrollIntoView", |i, t, a| {`

```
// `scrollIntoView` rollt so weit, dass die OBERKANTE des Elements oben
// steht — die Vorgabe der Spezifikation (`block: "start"`). Ein Argument
// `false` oder `{ block: "end" }` stellt die Unterkante ans untere Ende.
```

## L3371-3374 · `getter(&element_proto, "classList", |i, t, _| {`

```
// Die Liste selbst arbeitet auf dem Element: sie haelt keine Kopie der
// Klassen, sondern liest und schreibt das Attribut. Frisch je Zugriff —
// `el.classList === el.classList` ist damit falsch, waehrend ein Browser
// dasselbe Objekt liefert. Gemerkt, weil es eines Tages auffaellt.
```

## L3382-3384 · `getter(&element_proto, "style", |i, t, _| {`

```
// `el.style` ist eine SICHT auf das `style`-Attribut dieses Elements —
// sie haelt keinen eigenen Zustand, also koennen Attribut und Sicht nicht
// auseinanderlaufen.
```

## L3393-3396 · `for target in [&element_proto, &fragment_proto] {`

```
// querySelector & Co. auf Element wie auf Document.
// `append`, `prepend`, `before`, `after`, `replaceWith` — die moderne
// Einhaengfamilie. Sie nimmt beliebig viele Argumente, und ein Text wird
// dabei zum TEXTKNOTEN: `el.append("hallo")` haengt keinen String an.
```

## L3410-3411 · `for target in [&element_proto, &document_proto, &fragment_proto] {`

```
// Auch auf dem Bruchstueck: eine Schablone wird gefuellt, indem man in
// ihrem Inhalt sucht — ohne das ist `.content` nur halb gebaut.
```

## L3449-3456 · `getter(&document_proto, "documentElement", |i, this, _| {`

```
// ── Document ─────────────────────────────────────────────────────────
//
// **Die drei lesen `this`, nicht nur das Hauptdokument.** Seit
// `implementation.createHTMLDocument` gibt es einen ZWEITEN
// Dokumentknoten im selben Feld, und ein Getter, der stur
// `i.doc.body` zurueckgibt, haette dessen Rumpf ausgeliefert — jQuery
// haette sein Probestueck in die ECHTE Seite geschrieben. Fuer das
// Hauptdokument bleibt der gemerkte Weg; nur daneben wird gelaufen.
```

## L3467-3476 · `getter(&document_proto, "currentScript", |i, this, _| {`

```
// **`currentScript` ist der Weg, auf dem ein Buendel sich selbst findet.**
// Jedes von Turbopack erzeugte Stueck meldet sich mit
// `TURBOPACK.push([document.currentScript, …])` an und wirft ohne den
// Knoten „chunk path empty but not in a worker" — auf DDGs Startseite
// fielen daran sieben Skripte und ein Inline-Stueck aus. Dasselbe Feld
// sagt `document.write`, WOHIN geschrieben wird.
//
// Nur am Hauptdokument, und nur waehrend ein klassisches Skript laeuft:
// in einem Modul, in einem Rueckruf und in einem zweiten Dokument ist die
// Antwort `null` (HTML §4.12.1).
```

## L3482-3485 · `getter(&document_proto, "scrollingElement", |i, this, _| {`

```
// `scrollingElement` — das Element, dessen `scrollTop` die SEITE rollt.
// Im Standardmodus ist das `documentElement`, und beak parst nichts
// anderes. Eine Seite liest es und schreibt dann darauf; beide Antworten
// muessen zueinander passen (`is_scrolling_root`).
```

## L3489-3525 · `meth(&document_proto, "write", |i, this, a| doc_write(i, &this, a, false), 1, &fp);`

```
// **Vier Felder, die jede Seite liest — und die es bisher nicht gab.**
// Im Zensus stehen `visibilityState`/`hidden` mit 50 und `referrer` mit
// 45 Aufrufen (`docs/plan/WEB_PLATFORM_GAPS.md` P7). Ein FEHLENDES Feld
// ist schlechter als eine richtige Antwort: `document.referrer.indexOf(…)`
// stirbt auf `undefined`, und `if (document.hidden)` nimmt still den
// falschen Zweig.
//
// beak malt genau ein Dokument, und es ist sichtbar, solange es laeuft —
// das ist keine Hoeflichkeit, sondern der Zustand.
//
// **Seit es Tabs gibt (beak 0.163.0) gilt der Satz weiter, aber aus einem
// anderen Grund**, und der gehoert dazu, weil er ablaufen kann: ein Tab im
// Hintergrund ist EINGEFROREN (`docs/plan/BROWSER_TABS.md` §A3 b) — er hat
// gar keine JS-Sitzung, also fragt dort auch niemand. Eine Sitzung, die
// diese drei Zeilen liest, ist die des sichtbaren Tabs.
//
// Zur Luege werden sie an dem Tag, an dem ein Hintergrundtab LEBENDIG
// bleibt (§A3, LRU 2-3). Dann ist „sichtbar" falsch, und zwar in der
// schlimmsten Richtung: eine Seite, der man sagt, sie sei sichtbar,
// pollt weiter. Wer das baut, baut diese drei Getter mit — und
// `visibilitychange`, das es noch gar nicht gibt.
// **`document.write` — die eine Stelle, an der beaks Modell nicht das des
// Browsers ist.** Dort laeuft ein klassisches Skript WAEHREND des Parsens
// und schreibt in den Strom; beak hat den Baum schon fertig, wenn das
// erste Skript laeuft (`page_scripts` sagt es woertlich). Das Ergebnis
// ist trotzdem dasselbe, wenn man dorthin schreibt, wo der Parser stuende:
// unmittelbar HINTER das schreibende `<script>`.
//
// Nach der Spezifikation waere ein `write` ohne Einfuegestelle ein
// `document.open()` — also das Dokument LEEREN. Das tut beak nicht: hier
// hat kein Skript je eine Einfuegestelle im Sinne der Spezifikation, und
// eine leere Seite waere die schlechtere von zwei falschen Antworten.
// Ohne `currentScript` (aus einem Zeitgeber, einem Rueckruf) steht es
// deshalb auf der Konsole und passiert nichts.
//
// DDGs Startseite laedt so ihren Intl-Polyfill nach; ohne die Funktion
// starb das Skript an `write is not a function`.
```

## L3531-3534 · `getter(&document_proto, "referrer", |_, _, _| Ok(Value::str("")), &fp);`

```
// `referrer` ist die LEERE Zeichenkette und keine erfundene Adresse: sie
// ist die richtige Antwort fuer eine Navigation ohne Verweis, und beak
// reicht bisher keinen weiter. Da statt fehlend — und wenn der Wirt ihn
// einmal einreicht, steht die Stelle schon.
```

## L3536-3544 · `accessor(&document_proto, "cookie",`

```
// `document.cookie` — 1852 Aufrufe im Zensus, und auf BEIDEN Wikipedias
// die erste Wand ueberhaupt: das allererste Inline-Skript jeder Seite
// ruft `document.cookie.match(…)`, und auf `undefined` ist das das Ende
// des Skripts.
//
// Die Engine haelt keinen Behaelter. Was dieses Dokument sehen darf,
// haengt an Domain, Pfad, `Secure` und `HttpOnly` — das weiss der Wirt,
// und `Interp::set_cookies` reicht ihm genau die Skript-Sicht ein.
// Gesetztes geht denselben Weg zurueck (`take_cookie_sets`).
```

## L3553-3556 · `let deleting = decl.split(';').skip(1).any(|a| {`

```
// Loeschen erkennt die Engine nur an `Max-Age<=0` — das ist
// taktfrei. Ein `Expires` in der Vergangenheit braucht eine Uhr,
// die sie nicht hat; DER Fall wird erst sichtbar, wenn der Wirt
// die Sicht neu einreicht. Der Behaelter selbst hat beides.
```

## L3571-3576 · `accessor(&document_proto, "title",`

```
// Der Titel steht im Baum, nicht daneben: ein Skript, das ihn setzt,
// aendert das `<title>`-Element, und wer ihn liest, liest denselben
// Knoten. Zwei Kopien waeren zwei Wahrheiten.
// `title` liest und schreibt am EIGENEN Dokumentknoten — sonst gaebe ein
// frisch gebautes `createHTMLDocument("Titel")` den Titel der echten
// Seite zurueck, und ein Setzer daran wuerde ihn ueberschreiben.
```

## L3594-3595 · `None => {`

```
// Kein `<title>`: eins anlegen und in den Kopf haengen. Ein
// stiller Fehlschlag saehe aus wie ein kaputter Setzer.
```

## L3609-3612 · `getter(&node_proto, "ownerDocument", |i, t, _| {          // 6081`

```
// Was der Aufrufzensus (`tools/jsscope/out/apicensus.json`, eine echte
// Chromium-Messung auf denselben zwoelf Seiten) als naechstes verlangt.
// Die Reihenfolge hier IST die Rangfolge dort — nicht die Reihenfolge,
// in der mir etwas eingefallen ist.
```

## L3613 · `getter(&node_proto, "ownerDocument", |i, t, _| {          // 6081`

```
// 6081
```

## L3617 · `if id == root { return Ok(Value::Null) }              // das Dokument selbst: null`

```
// das Dokument selbst: null
```

## L3620 · `meth(&node_proto, "getRootNode", |i, t, _| {              // 421`

```
// 421
```

## L3628 · `getter(&element_proto, "namespaceURI", |i, t, _| {        // 3777`

```
// 3777
```

## L3629-3631 · `with_node!(i, t, |n| Ok(Value::str(`

```
// Nur die zwei, die vorkommen. Ein `foreignObject` in SVG bekaeme
// hier die falsche Antwort — es kommt im Zielkorpus nicht vor, und
// eine erfundene dritte waere schlimmer als eine ehrliche zweite.
```

## L3636 · `meth(&element_proto, "closest", |i, t, a| {               // 6115`

```
// 6115
```

## L3647 · `meth(&element_proto, "getAttributeNames", |i, t, _| {     // 165`

```
// 165
```

## L3652 · `meth(&element_proto, "hasAttributes", |i, t, _| {         // 159`

```
// 159
```

## L3655 · `getter(&element_proto, "dataset", |i, t, _| {             // 3966`

```
// 3966
```

## L3656-3667 · `let id = node_of(i, &t)?;`

```
// **Lebendig beim Schreiben, Momentaufnahme beim Lesen.** Die Werte
// stehen als gewoehnliche Eigenschaften darauf (das Lesen ist der
// Alltagsfall und soll nichts kosten), und `ObjKind::Dataset` traegt
// den Knoten, damit `Interp::set` eine Zuweisung ins ATTRIBUT
// durchreicht. Vorher war es nur die Momentaufnahme, und
// `el.dataset.theme = 'light'` verpuffte — der Theme-Schalter von
// `sandbox.nopeek.ch` genau so.
//
// Offen und benannt: `delete el.dataset.x` entfernt das Attribut
// nicht, und ein Schluessel, den das Element noch nicht hat, wird
// angelegt — das ist richtig — aber das Objekt in der Hand des
// Rufers zeigt Aenderungen von AUSSEN nicht nach.
```

## L3676 · `getter(&document_proto, "defaultView", |i, _, _| {        // 1580`

```
// 1580
```

## L3679-3684 · `getter(&document_proto, "forms", |i, _, _| {`

```
// `document.forms` — die Sammlung, ueber die Seiten ihr Formular finden.
//
// Mit BENANNTEM Zugriff: `document.forms["loginForm"]` sucht ueber `id`
// UND `name`, und genau diese Form nehmen Seiten. Eine Liste ohne
// Namenszugriff waere die halbe Sache — sie gibt `undefined` und sagt
// nicht, warum.
```

## L3706 · `getter(&document_proto, "activeElement", |i, _, _| {      // 1606`

```
// 1606
```

## L3707-3708 · `if let Some(f) = i.doc.as_ref().and_then(|d| d.focused) { return Ok(wrap(i, f)) }`

```
// Was `focus()` gesetzt hat — sonst `body`, die Antwort, die ein
// Browser ohne Fokus auch gibt.
```

## L3713 · `meth(&document_proto, "createComment", |i, _, a| {        // 2398`

```
// 2398
```

## L3720-3732 · `meth(&document_proto, "createEvent", |i, _, a| {`

```
// `document.createEvent` — die Fassung von DOM Level 2, und sie steht
// immer noch in ausgeliefertem Code: die Einwilligungsschicht auf
// arcade.ch baut damit JEDES ihrer Ereignisse (`registerEvent`), und der
// ganze `DOMContentLoaded`-Behandler der Seite starb an diesem EINEN
// fehlenden Aufruf — die Navigation blieb ungestaltet zurueck.
//
// Der Chromium-Zensus zaehlt drei Aufrufe auf zwoelf Zielseiten. Das
// entscheidet die GROESSE, nicht das Ob
// ([[feedback_a_call_count_is_not_a_site_count]]): gebaut wird die
// Namenstabelle der Spezifikation fuer die Schnittstellen, die es hier
// WIRKLICH gibt — und fuer jede andere die Absage, die DOM §createEvent
// dafuer vorsieht, statt einer Huelle, die beim naechsten `init…`-Aufruf
// ohnehin stirbt.
```

## L3736-3737 · `if !custom && !matches!(&*want, "event" | "events" | "htmlevents" | "svgevents") {`

```
// Die Tabelle ist case-insensitiv, und die Namen im Plural sind die
// aelteren Schreibweisen desselben Eintrags.
```

## L3750-3751 · `Ok(Value::Obj(build_event(i, proto, "", false)))`

```
// So gebaut ist es NICHT initialisiert: die Art bleibt leer, bis
// `initEvent` sie setzt. Genau dafuer gibt es die zwei Aufrufe.
```

## L3754 · `meth(&document_proto, "createElementNS", |i, _, a| {      // 180`

```
// 180
```

## L3778-3783 · `let img_ctor = native(Some(fp.clone()), |i, _, a| {`

```
// **`new Image()` ist `document.createElement("img")`** — dieselbe
// Sache, ein anderer Name. Ohne den Konstruktor bricht der Zeitgeber der
// Google-Ergebnisseite mit `Image is not defined` ab; ein Zaehlpixel ist
// der haeufigste Gebrauch, und der besteht genau aus `new Image().src =
// …`. Gebaut wird deshalb ein ECHTES `img`-Element und kein Attrappe:
// was die Seite danach daran tut, tut sie an einem Knoten im Baum.
```

## L3790-3791 · `for (k, n) in [("width", 0usize), ("height", 1usize)] {`

```
// `new Image(w, h)` setzt Breite und Hoehe als ATTRIBUTE, so wie im
// Browser — nicht als Stil.
```

## L3813-3817 · `meth(&document_proto, "importNode", |i, _, a| {`

```
// `importNode` (2134 Aufrufe) und `adoptNode`: beide holen einen Knoten
// in DIESES Dokument. beak hat genau eins — es gibt keinen zweiten Baum,
// aus dem etwas kaeme —, also ist `importNode` eine Kopie und `adoptNode`
// der Knoten selbst. Das ist keine Abkuerzung, sondern was die
// Spezifikation fuer den Ein-Dokument-Fall sagt.
```

## L3829-3840 · `let mo_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── MutationObserver ─────────────────────────────────────────────────
//
// **Warum es das braucht.** Alpine, htmx und jede Bibliothek, die
// nachgeladenes HTML von selbst zum Leben erweckt, meldet sich beim BAUM
// an statt bei einem Ereignis. Ohne `MutationObserver` stirbt Alpine
// schon beim Laden — `ReferenceError`, ausserhalb jedes `try`.
//
// Aufgezeichnet wird im Baum (`Doc::record`), zugestellt am
// Microtask-Kontrollpunkt (`promise::run_jobs`). Die Meldungen werden
// erst beim Zustellen GEBAUT: ein fertiges Objekt je Aenderung, waehrend
// das Skript laeuft, waere Arbeit fuer einen Leser, den es vielleicht
// nie gibt.
```

## L3871-3873 · `let filter = if !has_opts { None } else {`

```
// `attributeFilter` schaltet `attributes` MIT ein, auch wenn niemand
// es hingeschrieben hat — so steht es in der Spezifikation, und
// Bibliothekscode verlaesst sich darauf.
```

## L3891-3892 · `if !child_list && !attrs && !char_data {`

```
// Ohne eines der drei ist nichts zu beobachten — die Spezifikation
// wirft hier, statt still einen Beobachter anzulegen, der nie meldet.
```

## L3902-3903 · `o.regs.retain(|r| r.target != target);`

```
// Ein zweites `observe` auf DENSELBEN Knoten ersetzt die
// Anmeldung, es haengt keine zweite an (DOM §4.3.1).
```

## L3927-3929 · `collect_mutations(i);`

```
// Erst einsammeln, was der Baum seit dem letzten Mal notiert hat —
// sonst gaebe `takeRecords()` unmittelbar nach einer Aenderung eine
// leere Liste, und genau dafuer ruft man es.
```

## L3941-3946 · `let ro_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── ResizeObserver ───────────────────────────────────────────────────
//
// **Warum es das braucht.** Jedes Diagramm, jede Karte und jede
// Bibliothek, die sich an ihren Kasten anpasst, meldet sich hier an statt
// am `resize` des Fensters — der sagt nichts darueber, dass sich EIN
// Kasten geaendert hat, weil daneben etwas eingeklappt wurde.
```

## L3965 · `let kind = match a.get(1) {`

```
// `{ box: "border-box" }` — die Vorgabe ist der Inhaltskasten.
```

## L3980-3983 · `o.regs.retain(|r| r.target != target);`

```
// Ein zweites `observe` auf DENSELBEN Knoten ersetzt die
// Anmeldung (Resize Observer 3.1) — und `last: None` sorgt
// dafuer, dass es DANACH einmal meldet, so wie beim ersten
// Mal.
```

## L3991-3993 · `eval_box_observers(i);`

```
// Sofort auswerten: `observe` liefert die aktuelle Groesse, nicht
// erst die naechste Aenderung. Wer bis zum naechsten Layout wartet,
// laesst eine Seite ohne Groesse dastehen, die sich nie mehr aendert.
```

## L4016-4022 · `let io_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── IntersectionObserver ─────────────────────────────────────────────
//
// **Warum es das braucht.** Verzoegert geladene Bilder, unendliche
// Listen, „im Blick"-Animationen und jede Statistik, die zaehlt, was
// gesehen wurde. Ohne ihn laedt eine Bildergalerie genau ein Bild und
// haelt dann an — nicht mit einem Fehler, sondern mit Ruhe, und das ist
// schlimmer.
```

## L4031 · `let root = if !has { None } else {`

```
// Die Wurzel: ein Element, oder das Sichtfeld.
```

## L4038-4040 · `let (vw, vh) = i.viewport;`

```
// `rootMargin` — die CSS-Kurzform mit ein bis vier Laengen. Prozente
// stehen zur AUSSCHNITTgroesse; ohne eigene Wurzel ist das das
// Sichtfeld.
```

## L4050-4051 · `let mut thresholds: Vec<f64> = Vec::new();`

```
// `threshold` — eine Zahl oder eine Liste. Ohne Angabe: 0, also
// „sobald ein Pixel sichtbar wird".
```

## L4101-4103 · `eval_box_observers(i);`

```
// Wie beim `ResizeObserver`: die erste Meldung kommt sofort, nicht
// erst beim naechsten Bildlauf. Genau darauf verlaesst sich jede
// Liste, die beim Laden schon halb sichtbar ist.
```

## L4135-4136 · `getter(&io_proto, "root", |i, t, _| {`

```
// `root`, `rootMargin` und `thresholds` sind lesbar — Bibliothekscode
// liest sie zurueck, um einen Beobachter wiederzuverwenden.
```

## L4156-4169 · `let impl_obj = new_obj(Some(realm.object_proto.clone()));`

```
// ── document.implementation ──────────────────────────────────────────
//
// **`createHTMLDocument` ist der Grund, warum es das hier gibt.** jQuery
// baut damit ein WEGWERF-Dokument, um fremdes HTML zu zerlegen, ohne
// dass dabei Bilder geladen oder Skripte gefahren werden — und es tut
// das schon beim Laden, ausserhalb jedes `try`. Ohne diese Zeile stirbt
// jQuery an seiner eigenen Merkmalspruefung, und mit ihm die halbe
// Bibliothek der Seite.
//
// Ein zweiter Dokumentknoten IM SELBEN Knotenfeld, nicht ein zweites
// Feld: die Knoten-ids sind Plaetze in genau einem `Vec`, und ein
// zweiter Baum daneben waere ein zweites Adressraum-Modell. Losgeloest
// ist er trotzdem — er haengt an keinem Elter, also sieht ihn weder das
// Layout noch der Wirt.
```

## L4184-4185 · `if let Some(t) = title {`

```
// `createHTMLDocument()` OHNE Argument bekommt keinen Titel — das
// ist etwas anderes als der leere Titel, den `("")` verlangt.
```

## L4195-4196 · `meth(&impl_obj, "hasFeature", |_, _, _| Ok(Value::Bool(true)), 0, &fp);`

```
// `hasFeature` sagt laut Spezifikation IMMER true — sie ist Altlast und
// ausdruecklich so festgeschrieben, damit niemand mehr danach fragt.
```

## L4198-4201 · `meth(&impl_obj, "createDocument", |i, _, a| {`

```
// `createDocument` ist der XML-Zwilling: ein Dokumentknoten mit genau
// einem Wurzelelement, kein `head`, kein `body`. Der Namensraum wird
// GELESEN und fallengelassen — beaks Baum kennt keine Namensraeume, und
// ein erfundener waere schlimmer als keiner.
```

## L4218-4227 · `def_global(realm, "reportError", |i, _, a| {`

```
// `reportError(e)` (HTML §8.1.3.9): einen Fehler so melden, wie es eine
// unabgefangene Ausnahme tut — ans Fenster, und danach auf die Konsole.
//
// **Es ist der Weg, auf dem eine Laufzeitumgebung ueberhaupt SAGT, dass
// etwas schiefging.** React 19 meldet jeden unabgefangenen Renderfehler
// zuerst hierueber; gibt es den Namen nicht, faellt es auf einen
// `ErrorEvent` zurueck, den es auch nicht gibt, und am Ende auf
// `console.error`. Wer die Kette nicht hat, bekommt eine Seite, die
// nichts rendert UND nichts sagt
// ([[feedback_a_silent_failure_hides_every_bug_upstream_of_it]]).
```

## L4247-4248 · `if !handled {`

```
// `preventDefault` heisst „ich habe es behandelt" — dann schweigt die
// Konsole, genau wie im Browser.
```

## L4256-4268 · `let css_obj = new_obj(Some(realm.object_proto.clone()));`

```
// ── CSS ──────────────────────────────────────────────────────────────
//
// **Eine fehlende Merkmalspruefung ist keine neutrale Luecke — sie ist
// ein NEIN.** `CSS.supports` ist die Stelle, an der eine Seite fragt, ob
// sie den modernen Weg nehmen darf. Gibt es das Objekt nicht, nimmt sie
// den alten: DuckDuckGos Ergebnisseite laedt dann `css-vars-ponyfill`
// und laesst es ihre eigenen 1,1 MB Stilblaetter mit verschachtelten
// regulaeren Ausdruecken nachbauen — obwohl beak Custom Properties
// laengst selbst aufloest. Gemessen: der Lauf kam danach in zwanzig
// Minuten nicht zum Ende.
//
// Geantwortet wird aus DERSELBEN Funktion, die `@supports` im Blatt
// auswertet. Zwei Auskuenfte ueber dasselbe waeren zwei Wahrheiten.
```

## L4273-4275 · `Some(v) => {`

```
// Die ZWEIstellige Form nimmt Name und Wert — und sagt fuer eine
// Custom Property ausdruecklich NEIN (css-conditional-3 §6): wer
// `--x` pruefen will, muss die Bedingungsform nehmen.
```

## L4285-4287 · `meth(&css_obj, "escape", |i, _, a| {`

```
// `CSS.escape` (cssom-1 §9): ein Bezeichner, der in einem Selektor stehen
// darf. Bibliotheken bauen damit `#\31 23`-Selektoren aus fremden ids;
// ohne die Funktion wirft der Aufruf und nimmt das ganze Skript mit.
```

## L4295-4297 · `let lead_digit = c.is_ascii_digit()`

```
// Eine Ziffer am Anfang — und nach einem fuehrenden `-` — muss
// als Codepunkt ausgeschrieben werden, sonst liest der Parser
// eine Zahl.
```

## L4314-4328 · `let dp_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── DOMParser ────────────────────────────────────────────────────────
//
// **Der sichere Weg, fremdes HTML zu LESEN.** `innerHTML =` haengt es in
// die Seite; `new DOMParser().parseFromString(s, "text/html")` gibt ein
// Dokument NEBEN der Seite zurueck, aus dem man Text und Struktur holt,
// ohne dass etwas davon gemalt oder gefahren wird. Genau deshalb bauen
// Bibliotheken ihre Bereinigung damit — und genau daran fiel
// DuckDuckGos Ergebnisliste aus: ihre React-Schicht parst jeden
// Trefferauszug so, der `ReferenceError` loeste die Fehlergrenze aus,
// und die Seite blieb mit Kopfleiste und Filtern, aber OHNE Treffer
// stehen.
//
// Ein Bruder von `createHTMLDocument`, kein zweiter Parser: derselbe
// Baum, dieselbe Arena, derselbe Rahmen — nur dass der Text hier
// mitkommt.
```

## L4332-4334 · `let ty = i.to_string(a.get(1).unwrap_or(&Value::Undefined))?;`

```
// Der zweite Parameter ist PFLICHT und eine Aufzaehlung: was nicht
// darin steht, ist ein TypeError (DOM §DOMParser). Parameter hinter
// einem `;` (`text/html;charset=utf-8`) gehoeren nicht zum Namen.
```

## L4339-4344 · `"text/xml" | "application/xml" | "application/xhtml+xml" | "image/svg+xml" => false,`

```
// **XML faehrt durch denselben Parser, und das ist eine
// benannte Naeherung.** Ein echter XML-Lauf muesste bei jedem
// Wohlgeformtheitsfehler ein `parsererror`-Dokument liefern;
// beak hat einen Parser, und ein zweiter waere eine zweite
// Wahrheit darueber, was Auszeichnung bedeutet. Wer hier
// `parsererror` erwartet, bekommt es nicht.
```

## L4365-4367 · `meth(&document_proto, "evaluate", |i, _, a| {`

```
// Ein Dokument IST ein `XPathEvaluator` (DOM 4 §XPathEvaluatorBase), und
// `document.evaluate(...)` ist der Einstieg, den eine Seite schreibt —
// `new XPathEvaluator()` ist der von Bibliotheken.
```

## L4392-4402 · `let html_element_proto = new_obj(Some(element_proto.clone()));`

```
// ── Die Schnittstellen als globale Konstruktoren ─────────────────────
//
// Nicht Zierde: `el instanceof HTMLLinkElement` und
// `class X extends HTMLElement` sind auf DREI der elf Zielseiten die
// ERSTE Wand — vor jeder Sprachluecke. Gezaehlt, nicht vermutet
// (`wallcheck WCPAGE=*`).
//
// Die Kette ist die echte: EventTarget -> Node -> Element -> HTMLElement
// -> HTMLxyzElement. Eine flache Liste taete es fuer `instanceof
// HTMLElement` auch, aber dann waere `link instanceof Element` falsch —
// und genau solche Ketten fragt Bibliothekscode ab.
```

## L4406-4408 · `fn iface(realm: &Realm, name: &str, proto: &Gc) -> Gc {`

```
// `new HTMLElement()` wirft — so wie im Browser. Die KLASSENDEFINITION
// `class X extends HTMLElement {}` laeuft trotzdem durch: sie liest nur
// `HTMLElement.prototype`, gerufen wird der Konstruktor erst bei `new`.
```

## L4413-4418 · `fn iface_with(realm: &Realm, name: &str, proto: &Gc, ctor: NativeFn) -> Gc {`

```
/// Dieselbe Verdrahtung, aber mit einem echten Konstruktor. **Die
/// meisten DOM-Schnittstellen haben keinen** — `new HTMLElement()` wirft
/// im Browser genauso. `EventTarget` HAT einen (DOM §2.7), und das ist
/// keine Feinheit: DuckDuckGo prueft damit, ob es Apples MapKit laden
/// darf (`no_event_target`), und ohne den Konstruktor wird die Karte im
/// Wissenskasten nie auch nur ANGEFORDERT.
```

## L4423-4431 · `proto.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str(name)));`

```
// **`Symbol.toStringTag` traegt den Schnittstellennamen** (WebIDL
// 3.7.3). Ohne ihn meldete `Object.prototype.toString.call(el)`
// `[object Object]` — und genau daran erkennt Bibliothekscode ein
// GEWOEHNLICHES Objekt: jQuerys `isPlainObject` hielt jeden Knoten
// fuer eine Datenstruktur und stieg beim tiefen Kopieren ueber
// `parentNode` in einen Ring, aus dem es keinen Ausgang gibt.
//
// Hier und nicht an 60 Stellen: `iface` ist der EINE Weg, auf dem
// eine DOM-Schnittstelle entsteht.
```

## L4436-4445 · `iface_with(realm, "EventTarget", &event_target_proto, |i, _, _| {`

```
// **Ein eigenstaendiges `EventTarget` ist ein LOSGELOESTER Knoten.**
// Damit tragen `addEventListener`, `removeEventListener` und
// `dispatchEvent` unveraendert: die Zuhoererliste sitzt am Knoten, und
// ohne Elter ist die Blasenkette genau ein Glied lang — was fuer ein
// Ziel ohne Baum richtig ist. Die Alternative waere eine ZWEITE
// Zuhoererverwaltung neben der ersten
// ([[feedback_a_copy_is_a_second_semantics_waiting]]).
//
// Gesehen wird er von niemandem sonst: er haengt an keinem Elter, also
// erreicht ihn weder ein Selektor noch das Auslegen.
```

## L4448-4451 · `let Some(d) = &mut i.doc else {`

```
// Die Zuhoererliste sitzt im Dokument — ohne eines gibt es keine
// Stelle, an der sie stehen koennte. Auf einer Seite gibt es immer
// eins; nur das nackte `jsrun` hat keins, und dort steht es so da,
// statt still ein halbes Ziel zu liefern.
```

## L4459-4460 · `realm.global.borrow_mut().proto = Some(event_target_proto.clone());`

```
// Das Fenster IST ein EventTarget — dadurch hat `window` dieselben drei
// Methoden wie jeder Knoten, ohne sie ein zweites Mal zu definieren.
```

## L4462-4466 · `realm.global.borrow_mut().define(SYM_TO_STRING_TAG, Prop::tag(Value::str("Window")));`

```
// Und das Fenster heisst `Window`, nicht `EventTarget`. Ohne diese Zeile
// erbt es die Marke seines Prototyps, und `toString.call(window)` sagt
// etwas Falsches statt gar nichts. KEIN `iface`: das legte ein
// `constructor` auf das globale Objekt, und dort steht schon alles, was
// die Seite selbst definiert.
```

## L4469-4474 · `for (n, v) in [("ELEMENT_NODE", 1.0), ("ATTRIBUTE_NODE", 2.0), ("TEXT_NODE", 3.0),`

```
// **Die Knotentyp-Konstanten.** Sie stehen laut Spezifikation auf dem
// Konstruktor UND auf dem Prototyp. Ohne sie ist `Node.ELEMENT_NODE`
// `undefined`, und ein `switch(el.nodeType){case Node.ELEMENT_NODE: …}`
// faellt still in den `default`-Zweig: die Fritzbox-Oberflaeche hat
// damit ihre GANZE Anmeldemaske gebaut und dann nicht angehaengt — kein
// Fehler, keine Meldung, nur ein leeres `<body>`.
```

## L4485-4486 · `iface(realm, "HTMLElement", &html_element_proto);`

```
// NACH `iface`: die legt einen „Illegal constructor" auf denselben
// Prototyp, und wer zuletzt schreibt, gewinnt.
```

## L4490-4491 · `let char_data_proto = new_obj(Some(node_proto.clone()));`

```
// `CharacterData` sitzt zwischen Node und Text — 453 Aufrufe im Zensus
// fragen `.data`, und die Kette ist die, die Bibliothekscode abfragt.
```

## L4496-4497 · `let comment_proto = new_obj(Some(char_data_proto.clone()));`

```
// Ein Kommentar ist KEIN HTMLElement — vorher landete er dort, weil
// `wrap` ihn wie ein unbekanntes Tag behandelte.
```

## L4522-4536 · `let attr_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── Attr + NamedNodeMap ──────────────────────────────────────────────
//
// `el.attributes` (40 Aufrufe), `NamedNodeMap.length` (35) und die drei
// `Attr`-Felder (72). Zusammen 147, und sie haengen aneinander: ohne
// `Attr` ist die Karte leer, ohne die Karte ist `attributes` nutzlos.
//
// Die Karte ist eine MOMENTAUFNAHME, keine lebende Sicht. Ein Browser
// gibt eine lebende; wer die Karte haelt und dazwischen ein Attribut
// setzt, saehe hier den alten Stand. Gemerkt, weil es eines Tages
// auffaellt — die 147 Aufrufe lesen alle sofort.
//
// Die Felder sitzen auf dem PROTOTYP, nicht auf jedem Gegenstand — so
// wie im Browser. Sie auf die Instanz zu legen waere kuerzer und fuer
// `attributes[0].name` nicht zu unterscheiden; es faellt erst auf, wenn
// jemand `Attr.prototype` befragt, und genau das tut die Lueckenprobe.
```

## L4539-4540 · `macro_rules! slot_getter {`

```
/// Ein verdecktes Feld lesen. Eingebaute Funktionen sind Zeiger und
/// fangen nichts ein — also steht der Feldname im Rumpf.
```

## L4553-4554 · `for k in ["namespaceURI", "prefix"] {`

```
// Wir fuehren keine Namensraeume. `null` ist die richtige Antwort fuer
// HTML-Attribute, nicht eine fehlende.
```

## L4595-4597 · `m.define(k, Prop::data(Value::Obj(a)));`

```
// Auch unter dem NAMEN: `el.attributes.href` ist die uebliche
// Schreibweise, und die Spezifikation kennt sie (WebIDL
// `[LegacyUnenumerableNamedProperties]`).
```

## L4604-4606 · `meth(&element_proto, "toggleAttribute", |i, t, a| {`

```
// `toggleAttribute(name, force?)` — 56 Aufrufe. Es liefert, ob das
// Attribut DANACH da ist, und darauf verlaesst sich der uebliche Einzeiler
// `el.setAttribute("aria-expanded", el.toggleAttribute("open"))`.
```

## L4623-4628 · `for proto in [&element_proto, &char_data_proto] {`

```
// ── Der Kleinkram aus P7 ─────────────────────────────────────────────
//
// Je Zeile fuenf Zeilen Arbeit, zusammen rund 400 Aufrufe im Zensus. Sie
// stehen hier zusammen, weil sie EINE Sache gemeinsam haben: jede war
// nicht falsch, sondern GAR NICHT da — und ein fehlendes Feld ist ein
// `TypeError` mitten in fremdem Code, kein falscher Wert, den man sieht.
```

## L4630-4632 · `for proto in [&element_proto, &char_data_proto] {`

```
// `nextElementSibling`/`previousElementSibling` gehoeren zu
// `NonDocumentTypeChildNode` — also an Element UND an Text/Kommentar
// (112 + 54 Aufrufe). Der Zensus nennt beide getrennt, und beide gibt es.
```

## L4638-4641 · `meth(proto, "remove", |i, t, _| {`

```
// `remove()` gehoert zu `ChildNode`, also an BEIDE — es sass nur auf
// Element, und `CharacterData.remove` steht mit 18 Aufrufen im
// Zensus. Ein Textknoten, den man nicht loswird, ist genau die Sorte
// Luecke, die man erst am fremden Code merkt.
```

## L4648-4650 · `for proto in [&element_proto, &document_proto, &fragment_proto] {`

```
// `lastElementChild`/`childElementCount` — die Geschwister von
// `firstElementChild`, das es schon gab. Sie einzeln nachzureichen, wenn
// sie das naechste Mal fehlen, waere dreimal derselbe Weg.
```

## L4664-4666 · `meth(proto, "replaceChildren", |i, t, a| {`

```
// `replaceChildren(...)` — alles raus, das Neue rein. 52 Aufrufe, und
// es ist die moderne Schreibweise fuer `innerHTML = ""` plus
// anhaengen; wer sie nicht hat, bekommt eine halb geleerte Liste.
```

## L4675-4676 · `for proto in [&document_proto, &fragment_proto] {`

```
// Auf `document` und `<html>` fehlten `firstElementChild`/`children`
// ebenfalls — sie sitzen bisher nur auf Element.
```

## L4692-4695 · `getter(&node_proto, "isConnected", |i, t, _| {`

```
// `isConnected` (42): haengt dieser Knoten am Dokument? Genau diese Frage
// stellt jede Bibliothek, bevor sie an einem Knoten misst — ein Knoten
// ausserhalb des Baumes hat keinen Kasten, und ihn zu messen liefert
// Nullen, die aussehen wie eine Messung.
```

## L4707-4710 · `let token_list_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── DOMTokenList ─────────────────────────────────────────────────────
//
// `classList` gab es; was fehlte, war der Name — 1381 Aufrufe, und die
// Methoden sassen auf JEDER Liste einzeln statt auf einem Prototyp.
```

## L4713 · `fn set_classes(i: &mut Interp, id: u32, cs: &[Rc<str>]) {`

```
/// Die Klassen eines Knotens schreiben — eine Stelle, ein Format.
```

## L4748-4749 · `let forced = match a.get(1) { None | Some(Value::Undefined) => None, Some(v) => Some(v.truthy()) };`

```
// `toggle(name, kraft)` — das zweite Argument entscheidet statt des
// Zustands, und Seiten benutzen es fuer „setze genau so".
```

## L4803-4812 · `let style_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── CSSStyleDeclaration ──────────────────────────────────────────────
//
// Vorher gab `el.style` bei JEDEM Zugriff ein frisches leeres Objekt:
// ein Schreibzugriff verschwand, und ein Lesen danach fand nichts. Das
// war als ehrlicher Stumpf gemeint, ist aber der haeufigste Eingriff
// ueberhaupt — `el.style.display = "none"` ist Zeigen und Verstecken.
//
// Jetzt ist es eine SICHT auf das `style`-Attribut. Damit wirkt die
// Zuweisung wirklich: die Kaskade liest dasselbe Attribut, und `dirty`
// sagt beak, dass neu ausgelegt werden muss.
```

## L4855-4858 · `style_prop!(style_proto, fp, "display", "display");`

```
// Die benannten Eigenschaften. Die Liste ist bewusst endlich: ohne Proxy
// gibt es keinen Weg, JEDEN Namen abzufangen, und eine Liste, die die
// gebraeuchlichen deckt, ist besser als ein Stumpf, der keinen deckt.
// Was nicht daraufsteht, geht ueber `setProperty`/`getPropertyValue`.
```

## L4963-4974 · `let shadow_root_proto = new_obj(Some(fragment_proto.clone()));`

```
// **`ShadowRoot` gibt es als SCHNITTSTELLE, auch ohne Shadow DOM.**
// htmx fragt `e.parentNode instanceof ShadowRoot`, um einen Elter durch
// eine Schattengrenze zu finden — und ein `instanceof` gegen einen
// fehlenden Namen ist ein `ReferenceError`, der die ganze Bibliothek
// umbringt, statt `false` zu ergeben.
//
// `false` IST hier die wahre Antwort: beak haengt nirgends einen
// Schattenbaum an, also ist der Elter eines Knotens nie einer. So sieht
// ein Browser auf jeder Seite aus, die `attachShadow` nie ruft — das
// Schnittstellenobjekt steht da, eine Instanz gibt es nicht. Shadow DOM
// selbst bleibt gemessen kein Ziel
// ([[feedback_a_call_count_is_not_a_site_count]]).
```

## L4978-4986 · `let event_proto = new_obj(Some(realm.object_proto.clone()));`

```
// ── Event ────────────────────────────────────────────────────────────
//
// Das Ereignisobjekt gab es schon — als flache Huelle mit Datenfeldern.
// Was fehlte, war der NAME: `e instanceof Event` scheitert daran, nicht
// an `e.target`, und im Zensus haengen 1320 Aufrufe daran.
//
// Die Felder liegen jetzt in Schlitzen und die Prototypen lesen sie —
// sonst stuende `target` auf der INSTANZ und `Event.prototype.target`
// waere trotzdem leer, also genau die Abfrage, die scheitert.
```

## L4989-4990 · `ev_getter!(event_proto, fp2, "type", EV_TYPE);`

```
// Ein eingebautes Getter je Feld. Ein Funktionszeiger faengt nichts ein,
// also traegt jedes seinen Schlitznamen im Rumpf — das Makro schreibt sie.
```

## L5002-5003 · `if matches!(i.get(&t, EV_CANCELABLE)?, Value::Bool(true)) {`

```
// Nur ein abbrechbares Ereignis laesst sich abbrechen — sonst meldet
// `defaultPrevented` einen Halt, den niemand beachtet.
```

## L5020-5027 · `meth(&event_proto, "initEvent", |i, t, a| {`

```
// `initEvent(art, blasen, abbrechbar)` — der Partner von `createEvent`.
//
// Zwei Dinge stehen ausdruecklich in DOM §initEvent und sind beide der
// Grund, warum es nicht bloss drei Zuweisungen sind: ein Ereignis, das
// gerade ZUGESTELLT wird, laesst sich nicht mehr umbenennen (sonst
// wechselt es mitten in der Kette die Art), und der Aufruf setzt die
// Abbruch-Fahnen ZURUECK — dasselbe Objekt darf ein zweites Mal benutzt
// werden.
```

## L5030 · `if !matches!(i.get(&t, EV_PHASE)?, Value::Num(0.0)) {`

```
// `eventPhase != NONE` ist die Zustellfahne der Spezifikation.
```

## L5044-5045 · `Ok(nodes_array(i, chain.into_iter().rev().collect()))`

```
// Vom Ziel nach aussen — `ancestors` liefert die Zustellreihenfolge,
// also aussen zuerst.
```

## L5064-5065 · `let custom_proto = new_obj(Some(event_proto.clone()));`

```
// `CustomEvent` ist ein `Event` mit einem Feld — und mit 412 Aufrufen die
// Art, in der Seiten untereinander reden.
```

## L5081-5082 · `meth(&custom_proto, "initCustomEvent", |i, t, a| {`

```
// Der Partner von `createEvent("CustomEvent")` — ohne ihn haette der
// Zweig oben ein Objekt geliefert, das seine `detail` nie bekommt.
```

## L5102-5114 · `let ui_proto = new_obj(Some(event_proto.clone()));`

```
// ── Die Ereignis-ARTEN der Bedienung ────────────────────────────────
//
// **Bis 0.185.0 kannte beak nur die Basis `Event`.** Ein Behandler, der
// `e.key` oder `e.clientX` liest, bekam `undefined`, und ein
// `e instanceof KeyboardEvent` warf — der Name gab es nicht. Gemessen ueber
// die zwoelf Korpusseiten meldet `keydown` auf 11 von 12 an, `input` auf 8
// (`docs/plan/BROWSER_INPUT_EVENTS.md`); das ist die BEDIEN-Haelfte des
// Webs, und sie hing an diesen fuenf Namen.
//
// Die Kette ist die echte: Event -> UIEvent -> {Keyboard, Mouse, Focus,
// Input}Event. Die Felder stehen in Schlitzen und werden ueber
// Prototyp-Zugriffe gelesen, wie bei `Event` seit je — damit sind sie
// nicht aufzaehlbar, genau wie im Browser.
```

## L5137-5139 · `meth(&kbd_proto, "getModifierState", |i, t, a| {`

```
// `getModifierState` liest dieselben vier Schlitze. Seiten fragen damit
// nach Umschaltern, die wir gar nicht fuehren (`CapsLock`), und die
// richtige Antwort darauf ist `false`, nicht ein Wurf.
```

## L5199-5208 · `let prej_proto = new_obj(Some(event_proto.clone()));`

```
// `PromiseRejectionEvent` — die Art, unter der eine unbehandelte
// Ablehnung ans Fenster kommt.
//
// Die Marke ist keine leere Zusage: beak meldet unbehandelte Ablehnungen
// wirklich (`promise::report_rejections` schickt genau dieses Ereignis
// ans Fenster und schreibt danach auf die Konsole). Sie zu HABEN ist
// zugleich das, woran core-js erkennt, ob die Umgebung Versprechen
// browsermaessig behandelt — fiel die Pruefung durch, ersetzte es die
// eingebaute `Promise` durch seine eigene, und die kennt in der Fassung,
// die die Fritzbox ausliefert, kein `allSettled`.
```

## L5236-5254 · `for k in ["scrollX", "pageXOffset"] {`

```
// `getComputedStyle` — 443 Aufrufe im Zensus.
//
// Seit 0.64.0 antwortet es aus der KASKADE: der Wirt reicht mit
// `set_style_context` Blatt, Baum, Thema und Fensterbreite ein, und
// `computed_decls` rechnet damit dieselbe Kaskade wie das Layout — auf
// demselben Baum, mit demselben Blatt. Die Werte kommen in
// CSSOM-Schreibweise heraus (`rgb(0, 0, 0)`, nicht `#000`), also NICHT
// in der des Autors.
//
// Ohne Kontext bleibt es beim Inline-Stil. Das ist eine Teilantwort, aber
// die Funktion ganz wegzulassen hiesse TypeError, und ein TypeError
// beendet das Skript.
// Der Rollstand am Fenster. **Er stand fest auf 0** — `set_viewport` hat
// ihn als Zahl abgelegt, und danach hat ihn nie jemand nachgezogen. Eine
// Seite, die `window.scrollY` liest, um zu entscheiden, ob die Kopfzeile
// kleben soll, bekam ueberall die Antwort „ganz oben".
//
// Jetzt Zugriffsfunktionen auf die Geometrie: dieselbe Quelle, aus der
// `getBoundingClientRect` rechnet, und damit dieselbe Wahrheit.
```

## L5287-5295 · `if let Some(text) = computed_decls(i, id) {`

```
// Der gerechnete Stil, wenn der Wirt einen Kaskadenkontext eingereicht
// hat. Ein SCHNAPPSCHUSS, kein lebender Verweis — genau das ist
// `getComputedStyle` auch im Browser: die Antwort auf die Frage von
// jetzt. Ohne Kontext bleibt es beim Inline-Stil, und das ist eine
// Teilantwort, die die Seite laufen laesst.
// Gerechnet wird auf dem LEBENDEN Baum, und die Kennung ist der
// Arena-Index — dieselbe Zahl, die das JS-Objekt haelt. Ein Element,
// das noch nirgends haengt, findet `find_path` nicht: dafuer gibt es
// keinen gerechneten Stil, und die leere Antwort ist die ehrliche.
```

## L5303-5312 · `handler_prop!(html_element_proto, fp, "onclick", "click");`

```
// ── Behandler als Eigenschaft ────────────────────────────────────────
//
// `el.onclick = f` — 645 Aufrufe im Zensus, und bis hierher gab es davon
// NUR die Attributform. Eine Seite, die den Behandler zuweist statt ihn
// ins HTML zu schreiben, hatte gar keinen.
//
// Zugestellt wird trotzdem nur, was in `DISPATCHED` steht (heute:
// `click`). Die uebrigen Namen anzunehmen ist kein Vortaeuschen — die
// Anmeldung DARF nicht werfen, sonst stirbt die Seite an einer Zeile,
// die im Browser auch nichts tut, solange nichts passiert.
```

## L5319-5325 · `meth(&html_element_proto, "focus", |i, t, _| {`

```
// `focus()` / `blur()` — die Seite bestimmt, wo die Tastatur hinschreibt.
//
// Der Fokus steht im Dokument, nicht in einem Nebenzustand: `document
// .activeElement` liest dieselbe Stelle, und der Wirt uebernimmt sie in
// seinen eigenen (`forms.rs`). Die Ereignisse werden dabei WIRKLICH
// zugestellt — ein `focus()`, das nur einen Wert setzt, waere fuer eine
// Seite mit `onfocus` unsichtbar.
```

## L5356-5360 · `handler_prop!(html_element_proto, fp, "onpointerenter", "pointerenter");`

```
// Die Zeigerereignisse — 150 Aufrufe im Zensus. Zugestellt wird davon
// heute nichts (beak kennt `click`), aber die ANMELDUNG darf nicht
// werfen: eine Seite, die `el.onpointermove = f` schreibt, stirbt sonst
// an einer Zeile, die im Browser auch nichts tut, solange der Zeiger
// stillsteht.
```

## L5373-5374 · `handler_prop!(realm.global, fp, "onclick", "click");`

```
// Dieselben auf dem Fenster: `window.onload = …` ist die aelteste
// Schreibweise ueberhaupt und steht auf fast jeder alten Seite.
```

## L5402-5405 · `attr_prop!(html_element_proto, fp, "title", "title");`

```
// ── Felder, die auf einem Attribut sitzen ────────────────────────────
//
// Alle aus dem Zensus, keins geraten. Sie sind billig, weil das Attribut
// schon da ist — was fehlte, war der NAME, unter dem Seiten es abfragen.
```

## L5412-5413 · `num_attr_prop!(html_element_proto, fp, "tabIndex", "tabindex", -1.0);`

```
// `tabIndex` ist -1, wenn nichts dasteht: „nicht mit der Tabtaste
// erreichbar". Eine 0 hiesse das Gegenteil.
```

## L5416-5423 · `attr_prop!(element_proto, fp, "role", "role");`

```
// ── ARIA-Spiegelung ──────────────────────────────────────────────────
//
// `ariaHidden` steht mit 68 Aufrufen im Zensus — tagesschau.de liest es
// bei jedem Umschalten. Die Nachbarn kommen mit, weil sie dasselbe Muster
// haben und weil die naechste Seite eines davon liest: sie einzeln
// nachzureichen waere achtmal derselbe Weg.
//
// `role` ist die Ausnahme in der Liste: es heisst auch im Attribut so.
```

## L5442-5452 · `if let Some(p) = tag_protos.get("a") {`

```
// Je Schnittstelle das, was auf ihr wirklich abgefragt wird — aus dem
// Aufrufzensus, nicht aus der Spezifikation: `HTMLAnchorElement.href`
// steht dort mit 285 Aufrufen, `HTMLScriptElement.src` mit 144. Was
// niemand ruft, steht hier nicht.
//
// **`href` und `src` sind ROH**, so wie sie im Attribut stehen. Im
// Browser sind sie AUFGELOEST — `a.href` einer relativen Adresse ist die
// absolute. Das braucht die Adresse der Seite und ist eine eigene Zeile;
// bis dahin ist `.href` dasselbe wie `getAttribute("href")`, und das ist
// nachpruefbar falsch statt still falsch.
// HTMLAnchorElement
```

## L5460-5464 · `getter(p, "hash", |i, t, _| {`

```
// `a.hash` — 20 Aufrufe, und es ist das Stueck, an dem eine Seite
// erkennt, ob ein Verweis auf denselben Abschnitt zeigt. Aus dem
// ROHEN `href`, wie die Nachbarn auch: `href` ist hier nicht
// aufgeloest, und `hash` daraus aufzuloesen waere die eine Zeile, die
// aus der Reihe tanzt.
```

## L5470 · `if let Some(p) = tag_protos.get("link") {`

```
// HTMLLinkElement
```

## L5478 · `if let Some(p) = tag_protos.get("script") {`

```
// HTMLScriptElement
```

## L5484 · `if let Some(p) = tag_protos.get("img") {`

```
// HTMLImageElement
```

## L5491-5496 · `getter(p, "currentSrc", |i, t, _| {`

```
// `currentSrc` (28 Aufrufe): WELCHE Quelle wurde wirklich genommen.
// beak waehlt aus `srcset` im Layout (`picture.rs`), und die Engine
// kennt diese Wahl nicht — sie kennt nur das Dokument. Also `src`,
// und das ist die richtige Antwort fuer jedes Bild ohne `srcset`;
// fuer eines MIT ist es die Quelle, die im Markup steht, und nicht
// die leere Zeichenkette, an der Wikipedias Bildcode heute abbricht.
```

## L5502-5512 · `meth(&html_element_proto, "click", |i, t, _| {`

```
// **`el.click()`** — die uebliche Art, ein Steuerelement aus JS
// auszuloesen, und sie fehlte ganz. Ohne sie wirft schon der AUFRUF, und
// der Wurf beendet das Skript; eine Seite, die ihren eigenen Knopf
// programmatisch drueckt, stirbt daran.
//
// Die Reihenfolge ist die der Spezifikation (HTML §4.10.5, „synthetic
// click activation"): bei einem Kaestchen oder Radioknopf wird ZUERST
// umgeschaltet, dann `click` zugestellt, und erst wenn niemand abgebrochen
// hat, folgen `input` und `change`. Bricht jemand ab, wird der Haken
// wieder zurueckgenommen — genau das ist der Unterschied zwischen
// `preventDefault()` an einem Kaestchen und an einem Knopf.
```

## L5539 · `if let Some(p) = tag_protos.get("canvas") {`

```
// HTMLCanvasElement
```

## L5541-5543 · `accessor(p, "width",`

```
// `width`/`height` sind ZAHLEN und liegen als Attribut, mit den
// Vorgaben 300x150 aus der Spezifikation. Eine Seite liest sie, um
// ihre Zeichenflaeche zu bemessen.
```

## L5562-5574 · `meth(p, "getContext", |_, _, _| Ok(Value::Null), 1, &fp);`

```
// **`getContext` antwortet `null`, und das ist die Wahrheit.**
//
// Die Spezifikation sagt fuer einen Kontexttyp, den die Maschine nicht
// anbietet, ausdruecklich `null` — und beak hat keinen 2D-Kontext. Das
// ist etwas anderes als die Methode WEGZULASSEN: eine fehlende Methode
// wirft, und der Wurf beendet das ganze Skript. Auf sandbox.nopeek.ch
// stand `state.setCtx(state.canvas.getContext('2d'))` in `init()`,
// eine Zeile ueber `initEventListeners()` — der Wurf kostete jeden
// Knopf der Seite. Mit `null` laeuft `init()` durch, und der uebliche
// `if (ctx)` davor tut, was er soll.
//
// Ein echter 2D-Kontext ist ein eigenes Stueck Arbeit und steht in
// CONFORMANCE als benannte Luecke, nicht als Attrappe hier.
```

## L5577 · `if let Some(p) = tag_protos.get("input") {`

```
// HTMLInputElement
```

## L5583-5589 · `for (tag, from_text) in [("input", false), ("textarea", true)] {`

```
// ── Der WERT eines Steuerelements ────────────────────────────────────
//
// `value` ist der schmutzige Wert, `defaultValue` das Attribut — die
// Spezifikation trennt beide, und ein Browser aendert beim Setzen von
// `.value` das Attribut nicht. Bis hierher war `input.value` eine
// gewoehnliche Eigenschaft auf der HUELLE: sie las sich zurueck und
// erreichte weder Baum noch Layout noch das abgeschickte Formular.
```

## L5624-5628 · `accessor(p, "indeterminate",`

```
// `indeterminate` ist KEIN Attribut — es lebt nur im Objekt (HTML
// §4.10.5.3), und deshalb liegt es auch hier im Objekt. beak malt
// den dritten Zustand nicht; die Eigenschaft ist trotzdem da, weil
// ein Skript sie setzt und danach LIEST, und ein `undefined` an
// dieser Stelle ist eine falsche Antwort.
```

## L5642-5645 · `for tag in ["input", "select", "textarea", "button"] {`

```
// Die Eigenschaften, die JEDES Steuerelement traegt. Sie fehlten alle vier:
// `disabled`, `readOnly` und `required` liest und schreibt jedes
// Formularskript, und `form`/`labels` sind der Weg vom Feld zu seinem
// Umfeld.
```

## L5661-5662 · `if let Some(p) = tag_protos.get("label") {`

```
// `<label>`: die zwei Eigenschaften, mit denen ein Skript vom Schild zum
// Feld kommt.
```

## L5675-5678 · `for tag in ["input", "textarea"] {`

```
// Textauswahl in einem Feld. beak fuehrt keine Auswahl (CONFORMANCE sagt
// das), also sind das die ehrlichen Antworten: `textLength` misst wirklich,
// `select`/`setSelectionRange` setzen den Fokus und melden keine Auswahl,
// die es nicht gibt.
```

## L5689 · `if let Some(p) = tag_protos.get("form") {`

```
// ── HTMLFormElement: elements, submit, reset ─────────────────────────
```

## L5702-5704 · `meth(p, "submit", |i, t, _| {`

```
// `submit()` schickt OHNE `submit`-Ereignis ab — das ist der
// Unterschied zu `requestSubmit()`, und Seiten verlassen sich darauf
// (ihr eigener `onsubmit` soll nicht ein zweites Mal laufen).
```

## L5732-5742 · `if let Some(p) = tag_protos.get("option") {`

```
// ── HTMLSelectElement / HTMLOptionElement ────────────────────────────
//
// Ein `<select>` hatte weder `value` noch `options` noch `selectedIndex`.
// Auf der Fritzbox-Anmeldeseite stirbt daran der Aufbau des
// Anmeldeformulars — `gUsernameElem.value.length` liest `.length` von
// `undefined`, und der Fehler nennt weder Element noch Zeile.
//
// Die Wahrheit ist der BAUM, nicht ein Nebenzustand: `selected` ist das
// Attribut, `value` faellt auf den Text zurueck. Genau so liest das
// Layout die Auswahl (`forms.rs::collect_options`), also koennen die
// beiden Seiten nicht auseinanderlaufen.
```

## L5824 · `if let Some(p) = tag_protos.get("button") {`

```
// HTMLButtonElement
```

## L5829 · `if let Some(p) = tag_protos.get("form") {`

```
// HTMLFormElement
```

## L5835 · `if let Some(p) = tag_protos.get("textarea") {`

```
// HTMLTextAreaElement
```

## L5840 · `if let Some(p) = tag_protos.get("iframe") {`

```
// HTMLIFrameElement
```

## L5845 · `if let Some(p) = tag_protos.get("source") {`

```
// HTMLSourceElement
```

## L5852 · `if let Some(p) = tag_protos.get("meta") {`

```
// HTMLMetaElement
```

## L5859-5867 · `if let Some(tpl) = tag_protos.get("template") {`

```
// `<template>.content` — 2245 Aufrufe, die groesste einzelne Luecke im
// Zensus. Der Inhalt einer Schablone gehoert laut Spezifikation NICHT in
// den Baum, sondern in ein eigenes Bruchstueck.
//
// Umgehaengt wird erst beim ersten Zugriff. Wer nie `.content` liest,
// behaelt die Kinder im Baum, und `to_dom` schreibt sie zurueck wie
// bisher — gemalt werden sie ohnehin nicht (`style.rs` gibt `<template>`
// kein Kaestchen). Das ist der billige Weg zu spec-treuem Verhalten,
// ohne den Weg zurueck ins Layout anzufassen.
```

## L5881 · `{`

```
// SVG kennt genau eine Unterscheidung, die Seiten wirklich abfragen.
```

## L5902-5908 · `const HTML_IFACES: &[(&str, &[&str])] = &[`

```
/// Welches Element welche Schnittstelle traegt.
///
/// Die Liste ist nicht vollstaendig und soll es nicht sein — sie deckt, was
/// Seiten abfragen. Was nicht daraufsteht, ist `HTMLElement`, und das ist
/// die richtige Antwort: ein unbekanntes Element IST eins, und `instanceof
/// HTMLElement` ist die Abfrage, die wirklich vorkommt. `HTMLUnknownElement`
/// waere formal genauer und praktisch nutzlos.
```

## L5951-5954 · `fn element_sibling(i: &mut Interp, this: &Value, dir: i32) -> C<Value> {`

```
/// Der naechste/vorige ELEMENT-Geschwisterknoten. Wie `sibling`, nur laeuft
/// er weiter, bis ein Element kommt — Textknoten zwischen zwei `<li>` sind
/// der Normalfall, nicht die Ausnahme, und genau deshalb fragt Seitencode
/// nach `nextElementSibling` und nicht nach `nextSibling`.
```

## L5981 · `pub const DISPATCHED: &[&str] = &["click"];`

```
// ── Ereigniszustellung ──────────────────────────────────────────────────────
```

## L5983-5998 · `pub const DISPATCHED: &[&str] = &["click"];`

```
/// Ein Ereignis an `target` zustellen und die Kette hinauf blasen.
///
/// `chain` kommt aus dem Layout: die `seq`-Kette unter dem Zeiger, vom
/// aeussersten zum innersten. Zugestellt wird UMGEKEHRT — vom Ziel nach
/// aussen, so wie ein Browser blaest. Die Einfangphase gibt es nicht; sie
/// braucht ein drittes Argument an `addEventListener`, das kaum eine Seite
/// benutzt, und ohne sie stimmt die Reihenfolge fuer alles Uebrige.
///
/// Liefert true, wenn ein Behandler `preventDefault` gerufen hat — dann
/// unterbleibt, was beak sonst getan haette (einem Link folgen).
/// Welche Ereignisse beak ueberhaupt zustellt.
///
/// Die Liste ist absichtlich kurz und deckungsgleich mit dem, was der Wirt
/// wirklich ausloest. Ein `onload` hier aufzunehmen wuerde jeder Seite
/// Treffer-Kaesten aufzwingen, die nie jemand befragt — Aufwand fuer ein
/// Ereignis, das nie kommt.
```

## L6001 · `fn is_handler_attr(k: &str) -> bool {`

```
/// Ist `k` ein Attribut-Behandler fuer eins davon?
```

## L6006-6015 · `fn inline_handler(i: &mut Interp, node: u32, kind: &str) -> C<Option<Value>> {`

```
/// Den Behandler aus `on<art>` uebersetzen, falls es einen gibt.
///
/// Ein Attribut ist Quelltext, keine Funktion — es wird erst beim Ausloesen
/// uebersetzt. Das kostet je Klick eine Uebersetzung von ein paar Dutzend
/// Zeichen und spart, die halbe Seite beim Laden zu uebersetzen: die meisten
/// dieser Behandler werden nie ausgeloest.
///
/// Ein Attribut, das sich nicht uebersetzen laesst, ist KEIN Fehler der
/// Seite: der Browser laesst es still fallen, sonst haette ein Tippfehler in
/// einem Attribut die ganze Zustellung angehalten.
```

## L6024-6025 · `let mut wrapped = String::from("(function(event){");`

```
// Der Koerper laeuft mit `event` als Namen und `this` am Element — genau
// so ist der Attribut-Behandler definiert.
```

## L6036-6041 · `pub fn dispatch(i: &mut Interp, kind: &str, chain: &[u32]) -> C<bool> {`

```
/// Ein Ereignis, das beak selbst ausloest, ueber die Kette zustellen.
///
/// `chain` ist die Kette aus dem LAYOUT, aussen zuerst — nicht aus dem Baum.
/// Das ist keine Feinheit: der Klickpunkt kennt nur Kaesten, und wer die
/// Kette stattdessen aus dem Baum baut, prueft einen Weg, den beak nie geht
/// ([[feedback_the_test_path_must_be_the_real_path]]).
```

## L6046-6053 · `pub fn dispatch_at(i: &mut Interp, kind: &str, chain: &[u32],`

```
/// Wie `dispatch`, aber mit dem ORT des Zeigers — dann wird daraus ein
/// `MouseEvent`.
///
/// **Ein Klick ohne Koordinaten ist eine falsche Antwort, keine fehlende.**
/// Bis 0.186.0 war jeder Klick eine nackte `Event`, und `e.clientX` war
/// `undefined`: eine Seite, die ihr Menue an den Zeiger legt, rechnete mit
/// `NaN`. `client*` ist FENSTERbezogen, `page*` dokumentbezogen — der Rufer
/// gibt beides, weil nur er den Rollstand kennt.
```

## L6075-6076 · `put("__evbutton", Value::Num(0.0));`

```
// Die linke Taste: `button` zaehlt ab null, `buttons` ist eine
// Bitmaske und bei einem `click` schon wieder leer.
```

## L6083 · `put(EV_DETAIL, Value::Num(1.0));`

```
// `detail` ist bei einem einfachen Klick 1 (UI Events §5.3).
```

## L6088-6096 · `if kind == "click" && !prevented {`

```
// **Ein Klick auf ein `<label>` aktiviert sein Steuerelement** (HTML
// §4.10.4). Das ist keine Feinheit, sondern die Art, wie ein Kaestchen
// bedient wird: die Klickflaeche ist der TEXT daneben, und ohne diesen
// Schritt tut ein Klick darauf gar nichts — genau das Symptom „Checkboxen
// sind nicht sauber".
//
// Nicht, wenn jemand abgebrochen hat, und nicht, wenn das Steuerelement
// selbst schon in der Kette liegt (ein `<label><input></label>` bekaeme
// sonst zwei Klicks und schaltete zweimal um).
```

## L6115-6118 · `super::promise::run_jobs(i);`

```
// Ein Klick ist eine AUFGABE — danach laeuft die Microtask-Schlange, wie
// nach jeder anderen auch. Sonst bliebe ein `.then` aus dem Behandler bis
// zum naechsten Zeitgeber liegen, und auf einer Seite ohne Zeitgeber
// fuer immer.
```

## L6123 · `pub fn label_target(i: &Interp, id: u32) -> Option<(u32, u32)> {`

```
/// Das `<label>` am oder ueber dem getroffenen Knoten und sein Steuerelement.
```

## L6162-6166 · `fn deliver(i: &mut Interp, ev: &Gc, kind: &str, chain: &[u32]) -> C<bool> {`

```
/// Der gemeinsame Kern: ein fertiges Ereignis ueber eine fertige Kette.
///
/// Zwei Wege enden hier — beaks eigener Klick und `el.dispatchEvent(…)` der
/// Seite. Ein zweiter Rumpf waere ein zweiter Satz Regeln, und der eine wuerde
/// gepflegt und der andere nicht.
```

## L6179-6186 · `let prop = i.doc.as_ref().and_then(|d| d.nodes[node as usize].handlers.iter()`

```
// Der Behandler aus dem Attribut oder aus der Eigenschaft zuerst: im
// Quelltext steht er vor jedem `addEventListener`, das ein Skript
// spaeter anmeldet, und die Reihenfolge der Anmeldung ist die
// Reihenfolge des Aufrufs.
//
// ENTWEDER-ODER: `el.onclick = f` ersetzt im Browser den Behandler
// aus dem Attribut, es ist derselbe Platz. Beide laufen zu lassen
// hiesse, dass eine Zuweisung den alten nicht loswird.
```

## L6200-6201 · `set(ev, EV_PHASE, Value::Num(if k + 1 == chain.len() { 2.0 } else { 3.0 }));`

```
// 2 = AT_TARGET, 3 = BUBBLING_PHASE. Eine Fangphase gibt es nicht:
// `addEventListener` nimmt das dritte Argument an und verwirft es.
```

## L6204-6205 · `let r = i.call(&f, this_node.clone(), &[evv.clone()]);`

```
// Ein Behandler, der wirft, darf die naechsten nicht mitnehmen —
// so macht es ein Browser auch.
```

## L6207-6210 · `if let Err(e) = r {`

```
// **Ein Behandler, der wirft, muss es SAGEN.** Der Ausgang wurde
// hier nur auf `false` geprueft und sonst weggeworfen: ein Fehler
// im `onsubmit` einer Seite verschwand spurlos, und was danach
// nicht passierte, sah aus wie ein fehlendes Merkmal.
```

## L6216-6219 · `if matches!(r, Ok(Value::Bool(false))) { set(ev, EV_PREVENTED, Value::Bool(true)); }`

```
// `onclick="return false"` ist die alte Schreibweise fuer
// `preventDefault` und steht auf mehr Seiten als die neue. Sie
// gilt nur fuer den Attribut-Behandler; ein `addEventListener`
// wertet den Rueckgabewert nicht aus.
```

## L6235-6237 · `pub fn camel_to_data_attr(s: &str) -> String {`

```
/// `data-foo-bar` -> `fooBar`.
/// `theme` -> `data-theme`, `myKey` -> `data-my-key`. Die Umkehr von
/// `dash_to_camel`, und der Weg, den `el.dataset.x = v` nimmt.
```

## L6267-6268 · `pub fn boxed(seq: u32, x: i32, y: i32, w: i32, h: i32, bx: i16, by: i16) -> ElemRect {`

```
/// Ein Kasten, wie das Layout ihn aufzeichnet — kurz, weil eine Probe die
/// zwoelf Felder sonst dreimal ausschreibt und nur fuenf davon meint.
```

## L6273-6275 · `pub fn placed(seq: u32, x: i32, y: i32, w: i32, h: i32) -> ElemRect {`

```
/// Derselbe Kasten mit Polsterung — nur die Proben der Beobachter
/// brauchen sie.
/// Und mit `position` — nur `offsetParent` fragt danach.
```

## L6296-6302 · `fn run(html: &str, js: &str) -> alloc::vec::Vec<alloc::string::String> {`

```
/// `getComputedStyle` antwortet aus der KASKADE, nicht aus dem
/// Inline-Stil.
///
/// Der Unterschied ist der ganze Sinn: `.hide{display:none}` steht in
/// einem Blatt, nicht am Element. Bis 0.64.0 gab die Funktion darauf
/// „block" zurueck — eine Auskunft, auf die eine Seite ihren naechsten
/// Schritt baut.
```

## L6338-6344 · `#[test]`

```
/// **`new Image()` ist ein echtes `img`-Element, keine Attrappe.**
///
/// Die Google-Ergebnisseite bricht ihren Zeitgeber sonst mit
/// `Image is not defined` ab — ein Zaehlpixel besteht genau aus
/// `new Image().src = …`. Ein Stummel haette den Fehler weggenommen und
/// den Knoten schuldig geblieben; der Test prueft deshalb, dass das Ding
/// im Baum ankommt und sich wie ein Element verhaelt.
```

## L6361-6368 · `#[test]`

```
/// **Am Wurzelelement ist `clientWidth` die SICHTFLAECHE** (CSSOM View
/// §4), nicht der Polsterkasten. Googles Startseite misst damit das
/// Fenster und laeuft nur weiter, wenn beide Werte wahr sind — mit 0
/// blieb ihr ganzer Messblock stehen und das Formular schickte
/// `biw=&bih=`.
///
/// Eigener Aufbau statt `run`: der Helfer setzt keine Sichtflaeche, und
/// genau die ist hier der Gegenstand.
```

## L6385-6387 · `#[test]`

```
/// Ein Element, das ein Skript erst erzeugt hat, kommt im Baum nicht vor.
/// Dafuer gibt es keinen gerechneten Stil — und die leere Zeichenkette ist
/// die ehrliche Antwort, keine erfundene Zahl.
```

## L6398-6402 · `#[test]`

```
/// Was das SKRIPT an den Inline-Stil schreibt, sieht der gerechnete Stil
/// auch — obwohl der Kaskadenkontext ein Schnappschuss vom Skriptstart
/// ist. Ohne diese Ueberlagerung antwortete `getComputedStyle` mit dem
/// Stand von vorgestern, und ein Skript, das setzt und dann misst, bekam
/// seinen eigenen Wert nicht zurueck.
```

## L6415-6418 · `#[test]`

```
/// **Der Fall, wegen dem der Schnappschuss weg ist.** Ein Skript setzt
/// eine KLASSE und misst dann — die Klasse entscheidet, welche Regeln
/// ueberhaupt treffen, und aus einem Baum vom Skriptstart ist das nicht zu
/// beantworten. Bis 0.74.0 kam die Antwort von vorher.
```

## L6434-6435 · `#[test]`

```
/// Und ein Knoten, den das Skript erst EINHAENGT, bekommt seine Kaskade —
/// samt allem, was ihm der Nachbar oder der Elternteil vererbt.
```

## L6449-6452 · `#[test]`

```
/// Geometrie kommt aus dem LAYOUT, und der Wirt reicht sie ein. Ohne
/// eingereichte Kaesten bleibt es bei Nullen — das ist die Antwort eines
/// Browsers fuer ein Element ohne Kasten und die einzige ehrliche, solange
/// es kein Layout gibt.
```

## L6476 · `"10,60,200,50,210,110",`

```
// y ist um den Rollstand verschoben: 100 - 40.
```

## L6478 · `"200,50,100",`

```
// offsetTop geht gegen das DOKUMENT, also OHNE den Rollstand.
```

## L6480 · `"196,44",`

```
// Polsterkasten = Rahmenkasten ohne die Rahmensummen.
```

## L6486-6488 · `#[test]`

```
/// Ein Kasten ueber mehrere Zeilen hat mehrere Fragmente. `getClientRects`
/// nennt sie einzeln, `getBoundingClientRect` ihre VEREINIGUNG — nicht das
/// erste, was man findet.
```

## L6514-6519 · `#[test]`

```
/// `ResizeObserver` meldet den INHALTSkasten — Rahmen UND Polsterung ab.
///
/// Das ist der Unterschied, der zaehlt: ein Diagramm baut sein Zeichenfeld
/// aus `entry.contentRect.width`, und in einem gepolsterten Kasten waere
/// jede andere Zahl zu gross. Vor dieser Runde fuehrte das Layout die
/// Polsterung gar nicht mit.
```

## L6539 · `i.set_geometry(super::super::interp::Geometry {`

```
// Erst die Kaesten, dann das Skript: `observe` misst SOFORT.
```

## L6542-6543 · `padded(seq, 10, 100, 200, 50, 4, 6, 20, 10),`

```
// 200x50 Rahmenkasten, 4 px Rahmen und 20 px Polsterung
// waagerecht, 6 + 10 senkrecht.
```

## L6550 · `i.run_timers();`

```
// Zugestellt wird am Kontrollpunkt, nicht im `observe`.
```

## L6557-6559 · `#[test]`

```
/// Und er meldet nur, was sich GEAENDERT hat. Ein zweites Layout mit
/// derselben Groesse ist kein Ereignis — sonst waere jeder Bildlauf ein
/// Rueckrufgewitter.
```

## L6580 · `for _ in 0..3 { i.set_geometry(geom(200)); i.run_timers(); }`

```
// Dasselbe noch dreimal — die Groesse steht.
```

## L6582 · `i.set_geometry(geom(300));`

```
// Jetzt bewegt sie sich.
```

## L6590-6592 · `#[test]`

```
/// `IntersectionObserver`: das Verhaeltnis ist die geschnittene Flaeche
/// geteilt durch die des Ziels, und gemeldet wird beim WECHSEL ueber eine
/// Schwelle.
```

## L6602-6603 · `let at = |y: i32| super::super::interp::Geometry {`

```
// Ein 100 hoher Kasten, dessen Oberkante bei y liegt. Bei y = 950
// ragen 50 von 100 ins Sichtfeld: Verhaeltnis 0,5.
```

## L6620 · `i.set_geometry(at(975));`

```
// Ein Viertel herein: ueber 0, unter 0,5.
```

## L6623 · `i.set_geometry(at(950));`

```
// Die Haelfte: die zweite Schwelle faellt.
```

## L6626 · `i.set_geometry(at(2000));`

```
// Wieder hinaus.
```

## L6635-6636 · `#[test]`

```
/// `rootMargin` dehnt den Ausschnitt — genau das macht „lade das Bild,
/// BEVOR es sichtbar wird" moeglich.
```

## L6646 · `i.set_geometry(super::super::interp::Geometry {`

```
// 200 px UNTER dem Sichtfeld.
```

## L6668-6672 · `#[test]`

```
/// Die Rollmasse kommen aus den KAESTEN, nicht aus einer 0.
///
/// `scrollHeight` ist der Polsterkasten, vereinigt mit den Nachfahren —
/// ein Kind, das aus seinem Elter laeuft, macht genau den Unterschied,
/// wegen dem eine Seite ueberhaupt fragt.
```

## L6685 · `padded(a, 0, 0, 200, 60, 4, 4, 10, 10),`

```
// 200x60 Rahmenkasten, 4 px Rahmen und 10 px Polsterung.
```

## L6687 · `boxed(b, 2, 2, 180, 400, 0, 0),`

```
// Das Kind ist 400 hoch und laeuft unten heraus.
```

## L6700 · `"196,56",`

```
// Polsterkasten: 200-4, 60-4.
```

## L6702-6704 · `"196,400",`

```
// Rollflaeche: die Breite passt (das Kind endet bei 182, der
// Polsterkasten ist 196 breit), die Hoehe nicht — das Kind endet
// bei 402, die Polsterkante liegt bei 2.
```

## L6706-6707 · `"0,0",`

```
// beak klemmt nichts ab: an einem gewoehnlichen Element ist
// NICHTS weggerollt, und 0 ist hier wahr statt geraten.
```

## L6712-6713 · `#[test]`

```
/// Am Wurzelelement ist es die Rollflaeche des DOKUMENTS, und die sagt
/// das Layout — nicht die Vereinigung der Kaesten.
```

## L6737-6738 · `#[test]`

```
/// Rollen ist ein WUNSCH. Die Engine hat kein Fenster; sie merkt sich,
/// was die Seite wollte, und der Wirt holt es ab.
```

## L6756-6757 · `let prog = super::super::parse(`

```
// Und `scrollIntoView` rechnet die Dokumentlage aus, nicht die
// Fensterlage: der Kasten steht bei 1200, gerollt ist nichts.
```

## L6762 · `assert_eq!(i.take_scroll(), None);`

```
// Zweimal abholen gibt beim zweiten Mal nichts.
```

## L6766-6768 · `#[test]`

```
/// `offsetParent` ist der naechste POSITIONIERTE Vorfahr — und die Ecke
/// dafuer faehrt im Layoutkasten mit, statt fuer jeden Vorfahren die
/// Kaskade neu aufzuloesen.
```

## L6782 · `placed(a, 0, 0, 300, 300),          // positioniert`

```
// positioniert
```

## L6783 · `boxed(m, 0, 0, 300, 200, 0, 0),     // nicht`

```
// nicht
```

## L6798-6799 · `#[test]`

```
/// Der Kleinkram, in EINEM Lauf: jede dieser Zeilen war vorher ein
/// `TypeError` mitten in fremdem Code.
```

## L6838-6839 · `"[][]http://example.com/p",`

```
// Anmeldedaten faehrt beak nicht — und sagt das, statt zu
// schweigen oder sie in die Adresszeile zu schreiben.
```

## L6844-6846 · `#[test]`

```
/// Der Inline-Stil bleibt eine LEBENDE Sicht — `el.style` ist etwas
/// anderes als `getComputedStyle(el)`, und beide gehen durch dieselben
/// Zugriffsfunktionen.
```

## L6859-6860 · `const TD_FATAL: &str = "\0!tdfatal";`

```
/// Das interne Feld eines `TextDecoder`: verwirft eine ungueltige Folge
/// still, oder wirft? NUL-praefigiert, also fuer jedes Skript unsichtbar.
```

## L6863-6872 · `fn install_text_codec(realm: &mut Realm) {`

```
/// `TextEncoder` und `TextDecoder`.
///
/// **Nur UTF-8, und das ist keine Luecke.** Die Spezifikation laesst dem
/// Encoder gar keine andere Wahl (`new TextEncoder("latin1")` ist trotzdem
/// UTF-8), und der Decoder nimmt zwar Beschriftungen entgegen, aber jede
/// Seite, die eine andere als UTF-8 braucht, braucht auch eine Tabelle, die
/// hier nicht liegt. Eine fremde Beschriftung wird also angenommen und wie
/// UTF-8 behandelt, statt zu werfen — der Fritzbox-Anmeldecode ruft
/// `new TextEncoder("utf-8")`, und ein Wurf dort waere ein Fehler ueber
/// nichts.
```

## L6877 · `let enc_proto = new_obj(Some(op.clone()));`

```
// ── TextEncoder ──────────────────────────────────────────────────────
```

## L6887-6889 · `meth(&enc_proto, "encodeInto", |i, _, a| {`

```
// `encodeInto` schreibt in eine bestehende Sicht und meldet, wie weit es
// gekommen ist. Abgeschnitten wird an einer ZEICHENgrenze — eine halbe
// Folge in den Puffer zu legen waere kaputtes UTF-8.
```

## L6922 · `let dec_proto = new_obj(Some(op));`

```
// ── TextDecoder ──────────────────────────────────────────────────────
```

## L6938-6939 · `Ok(Value::string(lossy_utf8(&bytes)))`

```
// Ohne `fatal` ersetzt die Spezifikation jede ungueltige
// Folge durch U+FFFD, statt zu werfen.
```

## L6961 · `fn bytes_to_u8(i: &mut Interp, b: &[u8]) -> Value {`

```
/// Eine frische `Uint8Array` mit diesen Bytes.
```

## L6968-6969 · `fn read_view(v: &Value) -> Vec<u8> {`

```
/// Die Bytes hinter einer Sicht ODER einem Puffer. Alles andere ist leer —
/// `decode` bekommt in echtem Code nie etwas anderes.
```

## L6972-6974 · `let what = match &o.borrow().kind {`

```
// Erst die Angaben herausholen, dann die Ausleihe fallen lassen: eine
// Sicht auf SICH SELBST gibt es nicht, aber `slice_of` leiht den Puffer
// erneut, und bei `ObjKind::Buffer` waere das dasselbe Objekt.
```

## L6991 · `fn view_len(v: &Value) -> usize {`

```
/// Wieviele BYTES in die Sicht passen.
```

## L7016-7018 · `fn lossy_utf8(b: &[u8]) -> String {`

```
/// UTF-8 mit U+FFFD fuer jede ungueltige Folge. `String::from_utf8_lossy`
/// gibt es in `alloc` — aber nur mit `Cow`, und die Grenze ist hier
/// uninteressant.
```

## L7038-7049 · `fn install_custom_elements(realm: &mut Realm, html_element_proto: &Gc) {`

```
/// `customElements` — die Registratur der eigenen Elemente.
///
/// **Ohne Schattenbaum.** `attachShadow` fehlt weiter; gemessen an der
/// Fritzbox-Oberflaeche benutzen vier von vierzehn Komponenten einen, und
/// keine davon steht auf der Anmeldeseite. Die Trennung ist bewusst: ein
/// halber Schattenbaum waere schlimmer als keiner, weil er das Layout
/// betrifft und nicht nur die Bindung.
///
/// **Was es kann:** anmelden, nachschlagen, und — der eigentliche Punkt —
/// `class X extends HTMLElement` KONSTRUIERBAR machen. `new X()` legt einen
/// echten Knoten mit der angemeldeten Marke an; welche Marke, sagt der
/// Prototyp des gerade gebauten Objekts.
```

## L7053-7055 · `let he = native(Some(fp.clone()), |i, this, _| {`

```
// `HTMLElement` ist ab hier ein echter Konstruktor. Ohne ihn wirft
// `super()` in jeder Komponente „Illegal constructor" — und das ist die
// erste Zeile, die eine Seite mit Web Components ausfuehrt.
```

## L7074-7075 · `if !name.contains('-') {`

```
// Eine Marke ohne Bindestrich ist keine eigene — die Spezifikation
// wirft dort, und eine Seite, die es doch versucht, will es wissen.
```

## L7082-7086 · `let obs = i.get(&ctor, "observedAttributes")?;`

```
// **`observedAttributes` wird HIER gelesen**, nicht erst beim ersten
// Attributwechsel. Die Spezifikation sagt es so, und Bibliotheken
// haengen ihre ganze Einrichtung an diesen Zugriff: der `static get`
// der Fritzbox-Komponenten ruft `finalize()`, und ohne den steht
// spaeter `_wcProperties` auf `undefined`.
```

## L7089-7090 · `let _ = i.iterate(&obs);`

```
// Nur LESEN. Die Liste selbst braucht beak noch nicht — sie wird
// gebraucht, wenn `attributeChangedCallback` kommt.
```

## L7106-7107 · `meth(&ce, "whenDefined", |i, _, a| {`

```
// `whenDefined` wird nur abgewartet. Da alle Anmeldungen beim Laden
// passieren, ist die Antwort immer schon da.
```

## L7116-7118 · `meth(&ce, "upgrade", |_, _, _| Ok(Value::Undefined), 1, &fp);`

```
// `upgrade` tut nichts: beak baut den Baum aus dem HTML, bevor ein
// Skript laeuft, und hebt vorhandene Knoten nicht nachtraeglich in eine
// Klasse. Es ist da, damit ein Aufruf nicht wirft.
```

## L7124-7129 · `fn custom_tag_of(i: &Interp, this: &Value) -> Option<Rc<str>> {`

```
/// Welche angemeldete Marke gehoert zu diesem Objekt?
///
/// Ueber die PROTOTYPKETTE, von innen nach aussen — das gibt automatisch die
/// abgeleitetste Klasse. `new.target` gaebe dieselbe Antwort, aber beak
/// reicht es nicht durch, und die Kette weiss es ohnehin: `construct` hat den
/// Prototyp der gebauten Klasse schon gesetzt, bevor `super()` lief.
```

## L7147-7153 · `pub fn readopt(i: &mut Interp, from: &Gc, to: &Gc) {`

```
/// Den DOM-Knoten von einem Huellenobjekt auf ein anderes umhaengen.
///
/// `super()` kopiert die Felder des eingebauten Ergebnisses in das `this` der
/// abgeleiteten Klasse. Der Knoten zeigt danach aber noch auf die
/// weggeworfene Huelle — und `wrap` gibt jedem, der ihn spaeter aus dem Baum
/// holt, genau die. Diese Zeile ist der Unterschied zwischen „die Komponente
/// IST das Element" und „es gibt sie zweimal".
```

## L7166-7167 · `fn select_options(i: &Interp, sel: u32) -> Vec<u32> {`

```
/// Die `<option>`-Knoten eines `<select>`, in Dokumentreihenfolge und durch
/// `<optgroup>` hindurch — genau wie `forms.rs::collect_options`.
```

## L7180-7181 · `fn option_value(i: &Interp, id: u32) -> String {`

```
/// Der Wert einer Option: das Attribut, sonst ihr Text. Dieselbe Regel wie im
/// Layout — sonst zeigt die Auswahl etwas anderes an, als das Skript liest.
```

## L7190 · `fn owning_select(i: &Interp, id: u32) -> Option<u32> {`

```
/// Zu welchem `<select>` gehoert diese Option?
```

## L7201-7205 · `fn selected_index(i: &Interp, sel: u32) -> f64 {`

```
/// Welche Option ist ausgewaehlt?
///
/// Steht nirgends `selected`, ist es bei einer einfachen Auswahl die ERSTE —
/// so zeigt ein Browser sie an, und ein Skript, das gleich danach `value`
/// liest, bekaeme sonst die leere Zeichenkette.
```

## L7216 · `fn select_index(i: &mut Interp, sel: u32, n: i64) {`

```
/// Die n-te Option auswaehlen und alle anderen abwaehlen. `-1` waehlt nichts.
```

## L7227-7229 · `fn set_text_of(i: &mut Interp, id: u32, s: &str) {`

```
/// Den Inhalt eines Knotens durch EINEN Textknoten ersetzen — dieselbe
/// Regel wie `textContent`, nur als Funktion, weil `option.text` sie auch
/// braucht.
```

## L7243-7244 · `const CE_CONNECTED: &str = "\0!ceconn";`

```
/// Vermerk auf der Huelle: `connectedCallback` ist gelaufen. NUL-praefigiert,
/// also fuer jedes Skript unsichtbar.
```

## L7247 · `fn is_connected(d: &Doc, mut id: u32) -> bool {`

```
/// Haengt dieser Knoten wirklich am Dokument?
```

## L7255 · `fn collect_custom(d: &Doc, id: u32, out: &mut Vec<u32>) {`

```
/// Die eigenen Elemente eines Teilbaums, von aussen nach innen.
```

## L7258-7259 · `if n.kind == ELEMENT_NODE && n.tag.contains('-') { out.push(id); }`

```
// Eine Marke OHNE Bindestrich kann kein eigenes Element sein — die
// Spezifikation verlangt ihn, und die Pruefung kostet ein Byte.
```

## L7264-7276 · `fn doc_write(i: &mut Interp, this: &Value, a: &[Value], line: bool) -> C<Value> {`

```
/// `connectedCallback` fuer alles, was gerade ins Dokument gekommen ist.
///
/// **Einmal je Element**, gemerkt auf der Huelle: die Fritzbox-Komponenten
/// bauen darin ihren Inhalt, und ein zweiter Lauf wuerde ihn verdoppeln.
/// Ein Wurf im Rueckruf beendet NICHT das Einhaengen — so macht es ein
/// Browser auch —, landet aber sichtbar auf der Konsole statt still zu
/// verschwinden.
/// `document.write`/`writeln`: das Bruchstueck hinter das schreibende
/// `<script>` haengen und alles darin anschliessen.
///
/// Angeschlossen heisst hier auch GEHOLT: ein geschriebenes `<script src>`
/// laeuft, anders als eines aus `innerHTML`. Genau das ist der Unterschied
/// zwischen den beiden Wegen, und der Grund, warum eine Seite `write` nimmt.
```

## L7289-7290 · `let after = match i.write_point {`

```
// Die Einfuegestelle: hinter dem zuletzt Geschriebenen DIESES Skripts,
// sonst hinter dem Skript selbst.
```

## L7300-7301 · `for n in made {`

```
// Auch die TIEFER liegenden: `write` schreibt selten einen nackten
// Knoten, und ein `<script>` in einem `<div>` muss genauso laufen.
```

## L7333-7334 · `fn push_hex(out: &mut String, mut code: u32) {`

```
/// Ein Codepunkt als Hexziffern, klein geschrieben — die Form, die
/// `CSS.escape` verlangt.
```

## L7344-7356 · `fn settle_stylesheet(i: &mut Interp, id: u32) {`

```
/// Ein `<link rel="stylesheet">`, das ein SKRIPT einhaengt, wird zum HOLEN
/// angemeldet.
///
/// Die Engine holt nichts — sie legt die Adresse in `pending_sheets`, der
/// Wirt laedt sie und meldet mit `sheet_done` zurueck. Erst dann faellt
/// `load` oder `error` am `<link>`.
///
/// **Warum das eine eigene Runde wert ist.** Eine Seite, die ihre
/// Stilblaetter per Skript nachlaedt, wartet in aller Regel auf deren `load`,
/// bevor sie weiterbaut. Ohne Antwort steht sie fuer immer; mit einer
/// erfundenen `load`-Meldung baut sie weiter und sieht falsch aus, ohne dass
/// es jemand sagt. Beides ist schlechter als die Wahrheit, und die Wahrheit
/// heisst: holen.
```

## L7366 · `if i.pending_sheets.iter().any(|(n, _)| *n == id) { return }`

```
// Zweimal anmelden waere zweimal holen — und zweimal `load`.
```

## L7373 · `pub fn sheet_done(i: &mut Interp, id: u32, ok: bool) {`

```
/// Der Wirt meldet, wie es einem angeforderten Blatt ergangen ist.
```

## L7378-7395 · `fn settle_script(i: &mut Interp, id: u32) {`

```
/// Ein `<script src>`, das ein SKRIPT einhaengt, wird zum Holen angemeldet.
///
/// **Das ist die Art, in der ein geteiltes Buendel seine Stuecke nachlaedt.**
/// webpack baut dafuer ein `<script>`, haengt es an den Kopf und wartet auf
/// sein `onload`; Next.js wartet auf genau dieses Versprechen, BEVOR es
/// React ueberhaupt etwas zu rendern gibt. Kam nie eine Antwort, blieb das
/// Versprechen offen und die Seite leer — ohne eine einzige Fehlermeldung,
/// weil formal nichts schiefgegangen war.
///
/// Drei Grenzen, jede mit Grund:
/// * **Nur mit `src`.** Ein eingehaengtes Skript MIT Text laeuft laut
///   Spezifikation sofort beim Einhaengen; das ist eine eigene Baustelle
///   und hier bewusst nicht angefasst.
/// * **Nur einmal.** Das „already started"-Kennzeichen der Spezifikation:
///   ein Skript, das man wieder einhaengt, laeuft nicht noch einmal.
/// * **Kein Modul.** Ein `type="module"` braucht den Graphen, und den loest
///   der Wirt beim Laden der Seite auf. Es waere ein zweiter Lader — offen
///   und hier benannt statt still falsch gemacht.
```

## L7413-7418 · `pub fn script_done(i: &mut Interp, id: u32, source: Option<&str>) {`

```
/// Der Wirt meldet, was aus einem angeforderten Skript geworden ist.
///
/// `Some(quelle)` heisst geholt: der Text laeuft im Bereich der Seite, und
/// DANACH faellt `load` — die Reihenfolge des Browsers, auf die jeder
/// Nachlader baut. Ein Wurf im Skript beendet nur dieses Skript; `load`
/// faellt trotzdem, denn geladen wurde es ja.
```

## L7426-7428 · `let outer = i.current_script.replace(id);`

```
// `document.currentScript` zeigt auf DIESEN Knoten, solange er
// laeuft — und danach auf den, in dem wir stehen: ein per Skript
// eingehaengtes `<script>` laeuft aus einem anderen heraus.
```

## L7442-7443 · `pub fn dispatch_rejection(i: &mut Interp, reason: Value, promise: Value) -> C<bool> {`

```
/// Eine unbehandelte Ablehnung ans Fenster melden. Liefert true, wenn ein
/// Behandler `preventDefault` gerufen hat — dann unterbleibt die Konsolenzeile.
```

## L7453-7454 · `fn deliver_focus(i: &mut Interp, id: u32, kind: &str) -> C<()> {`

```
/// `focus`/`blur` zustellen. Sie BLUBBERN NICHT — die Kette ist das Element
/// allein; `focusin`/`focusout` waeren die blubbernden Zwillinge.
```

## L7460-7464 · `fn control_value(i: &Interp, id: u32, from_text: bool) -> String {`

```
/// Der WERT eines Steuerelements: der schmutzige, sonst der Vorgabewert.
///
/// Bei `<textarea>` ist der Vorgabewert der TEXTinhalt, bei `<input>` das
/// `value`-Attribut. Beides steht so in der Spezifikation, und beides ist der
/// Grund, warum es hier eine Funktion gibt statt zweier Makroaufrufe.
```

## L7479-7484 · `fn checked_now(i: &Interp, id: u32) -> bool {`

```
/// Die Steuerelemente eines `<form>`, in Dokumentreihenfolge.
///
/// Ueber den BAUM, nicht ueber `form=`: das Attribut, mit dem ein Element
/// ausserhalb seines Formulars stehen kann, liest beak nirgends, und eine
/// halbe Zuordnung waere schlimmer als eine ehrliche.
/// Der Haken, wie er JETZT steht: der „schmutzige" Wert, sonst das Attribut.
```

## L7502-7505 · `fn owning_form(i: &Interp, id: u32) -> Option<u32> {`

```
/// Das Formular, dem ein Steuerelement gehoert: der naechste `<form>`-Vorfahr.
/// Das `form=`-Attribut (ein Control ausserhalb seines Formulars) wird hier
/// NICHT gelesen — dieselbe Grenze, die `forms::collect` hat, und benannt
/// statt still.
```

## L7518-7519 · `fn labels_of(i: &Interp, id: u32) -> Vec<u32> {`

```
/// Die `<label>`, die zu einem Steuerelement gehoeren: die es einwickeln, und
/// die per `for=` auf seine `id` zeigen.
```

## L7541-7542 · `fn label_control(i: &Interp, id: u32) -> Option<u32> {`

```
/// Das Steuerelement, das eine `<label>` benennt: `for=` zuerst, sonst das
/// erste eingewickelte.
```

## L7578-7579 · `if tag != "form" { walk(d, c, out); }`

```
// Ein verschachteltes `<form>` ist ungueltiges HTML; seine
// Elemente gehoeren ihm, nicht uns.
```

## L7588 · `fn tags_of(d: &Doc, from: u32, tag: &str) -> Vec<u32> {`

```
/// Alle Elemente einer Marke im Teilbaum, in Dokumentreihenfolge.
```

## L7602-7609 · `fn iface_proto(i: &mut Interp, iface: &str) -> Gc {`

```
/// Ein Ereignis an das Element mit dieser `seq` zustellen — mit der Kette bis
/// zur Wurzel, also BLUBBERND.
///
/// Der Weg, auf dem der Wirt der Seite etwas meldet, das er selbst ausloest:
/// ein Formular, das abgeschickt wird, ein Element, das den Fokus bekommt.
/// Liefert true, wenn ein Behandler `preventDefault` gerufen hat (oder
/// `false` zurueckgab).
/// Der Prototyp einer Ereignisart, ueber ihren globalen Namen.
```

## L7618-7622 · `fn dispatch_typed(i: &mut Interp, iface: &str, kind: &str, seq: u32,`

```
/// Ein Ereignis einer ART ueber den Baumknoten `seq` zustellen.
///
/// **Der eine Weg fuer alles, was der Wirt schickt.** Gibt `true`, wenn die
/// Seite abgebrochen hat (`preventDefault`) — und genau darauf muss der Rufer
/// hoeren: ein `keydown`, das abgebrochen wurde, darf kein Zeichen einfuegen.
```

## L7645-7649 · `pub fn dispatch_key(i: &mut Interp, kind: &str, seq: u32, key: &str, code: &str,`

```
/// `keydown`/`keyup` an das Steuerelement mit dieser `seq`.
///
/// `key` ist der WERT der Taste (`"a"`, `"Enter"`, `"ArrowLeft"`), `code` ihr
/// Ort auf der Tastatur (`"KeyA"`). `key_code` ist die Altlast, die trotzdem
/// jeder liest. Gibt `true`, wenn die Seite abgebrochen hat.
```

## L7668-7671 · `pub fn dispatch_input_event(i: &mut Interp, kind: &str, seq: u32,`

```
/// `beforeinput`/`input` an das Steuerelement mit dieser `seq`.
///
/// `beforeinput` ist abbrechbar, `input` nicht (UI Events §5.1) — deshalb
/// sagt `cancelable` hier nicht immer dasselbe.
```

## L7683-7687 · `pub fn dispatch_focus(i: &mut Interp, kind: &str, seq: u32, related: Option<u32>) -> bool {`

```
/// `focus`/`blur` (blasen NICHT) und `focusin`/`focusout` (blasen).
///
/// Beide Paare, weil Seiten beide benutzen und das eine das andere nicht
/// ersetzt: `focus` erreicht nur das Element selbst, `focusin` den ganzen Weg
/// nach oben — eine Seite, die am Formular lauscht, hoert nur das zweite.
```

## L7713-7720 · `pub fn push_control_values(doc: &mut Doc, forms: &crate::forms::Forms,`

```
// ── Die Bruecke zwischen den Eingaben des Benutzers und dem Baum ─────────
//
// Zwei Speicher, EINE Regel. Die Eingaben leben im Wirt (`FormState`, nach
// `seq`), weil eine Seite ohne Skripte gar keinen Baum der Maschine hat; der
// schmutzige Wert lebt am Knoten, weil `el.value` ihn dort erwartet. Die
// beiden muessen vor und nach jedem Lauf von Seitencode abgeglichen werden —
// und dafuer gibt es genau diese zwei Funktionen, damit nicht jeder Rufer
// seine eigene Regel bekommt.
```

## L7722-7725 · `pub fn push_control_values(doc: &mut Doc, forms: &crate::forms::Forms,`

```
/// Die Eingaben des Benutzers in den Baum schreiben — VOR jedem Lauf von
/// Seitencode. Uebertragen wird nur, was wirklich bearbeitet wurde: sonst
/// traegt hinterher jedes Feld einen schmutzigen Wert und `form.reset()`
/// haette nichts mehr zurueckzustellen.
```

## L7744 · `pub fn pull_control_values(doc: &Doc, forms: &crate::forms::Forms,`

```
/// Und zurueck: was Seitencode gesetzt hat, gilt fuer Anzeige und Absenden.
```

