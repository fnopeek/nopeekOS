# `tools/wasm/beak-engine/src/js/interp.rs` @ 5e0102684

## L1-13 · `use alloc::boxed::Box;`

```
//! Die Auswertung: ein Baumlaeufer.
//!
//! **Warum ein Baumlaeufer und kein Bytecode.** Das Gedaechtnis notiert zu
//! Recht, dass die Form der Verteilerschleife eine Entwurfsentscheidung ist
//! und dass wasms `return_call` dort der moderne Hebel waere. Der Hebel bleibt
//! richtig — er wird nur nicht als erstes gezogen: heute gibt es keine Zahl,
//! gegen die er sich messen liesse, und was zuerst gebraucht wird, ist
//! Richtigkeit. Der test262-Lauf ist danach das Netz, mit dem eine Umstellung
//! auf Bytecode ueberhaupt erst verantwortbar ist.
//!
//! Was hier bewusst NICHT steht: Generatoren, async/await, Proxy, Symbole,
//! BigInt. Jedes davon faellt im Lauf als eigene Zeile auf und ist damit
//! gezaehlt statt vergessen.
```

## L26 · `pub enum Abrupt {`

```
/// Ein Abbruch: alles, was nicht „der naechste Ausdruck" ist.
```

## L38-39 · `pub initialized: bool,`

```
/// `let`/`const` vor ihrer Deklaration: der Zugriff wirft. Ohne das ist
/// die zeitliche Totzone unsichtbar und `let` verhaelt sich wie `var`.
```

## L46-47 · `pub this_val: Option<Value>,`

```
/// Nur Funktionsumgebungen tragen `this`; ein Block erbt es. Genau daran
/// haengt, dass ein Pfeil das `this` seiner Umgebung sieht.
```

## L49 · `pub is_func_scope: bool,`

```
/// Ist das die Umgebung einer Funktion (Ziel fuer `var`-Hochziehen)?
```

## L51-55 · `pub home: Option<Gc>,`

```
/// Das „Heimatobjekt" der Methode, in der wir stehen — bei einer Klasse
/// ihr `prototype`. `super.f` sucht auf DESSEN Prototyp, nicht auf dem
/// von `this`: sonst faende eine Methode, die `super.f()` ruft, sich
/// selbst wieder und liefe endlos. Ein Pfeil setzt es nicht und erbt es
/// dadurch ueber die Kette, genau wie `this`.
```

## L57-61 · `pub strict: bool,`

```
/// Streng? Die Strenge steht am CODE (siehe [[ast::Func::strict]]), aber
/// gelesen wird sie zur Laufzeit — an jeder Zuweisung, jedem `delete`,
/// jedem `this`. Deshalb faellt sie beim Anlegen der Umgebung hier
/// hinein und wird VERERBT: ein Block in einer strengen Funktion ist
/// streng, ohne dass jemand die Kette hochlaufen muss.
```

## L63-72 · `pub imports: Option<Box<HashMap<Rc<str>, (Rc<RefCell<Env>>, Rc<str>)>>>,`

```
/// Namen, die aus einem ANDEREN Modul kommen (`import`).
///
/// **Ein Verweis, keine Kopie.** Ein Modulgraph mit Zyklen — und der der
/// Fritzbox-Oberflaeche hat welche (`main` <-> `oldpage` <-> `html2`) —
/// braucht LEBENDE Bindungen: wer im Kreis frueher laeuft, sieht den
/// Wert, den der andere spaeter hineinschreibt. Eine Kopie beim Verbinden
/// saehe dort `undefined`, und zwar still.
///
/// Nur Modulumgebungen tragen die Tabelle; jede andere ein `None`, und
/// das ist eine Nullpruefung im Kettenlauf.
```

## L74-81 · `pub with_obj: Option<Gc>,`

```
/// Das Objekt eines `with (o) { … }`.
///
/// **Eine Umgebung, deren Namen aus einem OBJEKT kommen** — die einzige
/// Sorte, deren Bindungen sich waehrend des Laufs aendern koennen. Sie
/// ist deshalb auch der Grund, warum die Wegweiser (`Chunk::hints`)
/// abgeschaltet werden, sobald eine entsteht: ein Hinweis auf eine Tiefe
/// zeigt an einer Eigenschaft vorbei, die es beim letzten Mal noch nicht
/// gab.
```

## L86-88 · `pub fn new(parent: Option<Rc<RefCell<Env>>>, func_scope: bool) -> Rc<RefCell<Env>> {`

```
/// Erbt die Strenge vom Elter. Ein Funktionsaufruf ueberschreibt sie
/// gleich danach mit der seines eigenen Rumpfes — nur DORT darf sie sich
/// aendern, und nur nach oben.
```

## L98-99 · `pub fn env_strict(env: &Rc<RefCell<Env>>) -> bool { env.borrow().strict }`

```
/// Ist der Code, der gerade laeuft, streng? Ein Feld, kein Kettenlauf — die
/// Vererbung ist beim Anlegen passiert.
```

## L106 · `pub fn env_lookup_depth(env: &Rc<RefCell<Env>>, name: &str)`

```
/// Wie `env_lookup`, sagt aber MIT, wieviele Spruenge es gekostet hat.
```

## L124-128 · `pub fn env_deref(env: &Rc<RefCell<Env>>, name: &str) -> Option<(Rc<RefCell<Env>>, Rc<str>)> {`

```
/// Einem `import` folgen, bis eine echte Bindung dasteht.
///
/// Eine Kette ist moeglich (`export { x } from …` reicht durch), ein KREIS
/// auch — ein Modul, das seinen eigenen Namen wieder einfuehrt. Der Deckel
/// ist deshalb kein Vorsichtsmass, sondern die Abbruchbedingung.
```

## L133-138 · `pub fn env_deref_from(mut e: Rc<RefCell<Env>>, mut n: Rc<str>)`

```
/// Wie `env_deref`, nur mit einem Namen, den der Rufer schon BESITZT.
///
/// Der gewoehnliche Lesepfad geht hierueber und alloziert damit nichts:
/// `Rc::from(name)` kopiert die Zeichenkette auf den Haufen, und das je
/// Variablenzugriff — fuer eine Kette, die in aller Regel gar nicht
/// betreten wird.
```

## L155 · `pub enum Hit {`

```
/// Was EIN Blick in EINE Umgebung ergeben hat.
```

## L157 · `Val(Value),`

```
/// Der Name steht hier, mit diesem Wert.
```

## L159 · `Dead,`

```
/// Er steht hier, ist aber noch nicht initialisiert (zeitliche Totzone).
```

## L161 · `Import(Rc<RefCell<Env>>, Rc<str>),`

```
/// Er kommt aus einem anderen Modul.
```

## L163 · `Up(Option<Rc<RefCell<Env>>>),`

```
/// Nicht hier — weiter beim Elter (`None` = die Kette ist zu Ende).
```

## L167-173 · `pub fn env_peek(env: &Rc<RefCell<Env>>, n: &str) -> Hit {`

```
/// Eine Umgebung EINMAL fragen.
///
/// **Der Punkt ist, dass es einmal ist.** Der Lesepfad lief bis 0.117.0 als
/// `env_lookup` + `env_deref` + zwei `vars.get` — derselbe Name bis zu
/// viermal gehasht und mit `memcmp` verglichen, dazu ein `Rc<str>` auf dem
/// Haufen. Im Profil der Fritzbox-Anmeldung waren das 6,1 % memcmp und
/// 6,3 % hashbrown.
```

## L179-180 · `if let Some(t) = b.imports.as_ref().and_then(|m| m.get(n)) {`

```
// Die Tabelle gibt es nur in Modulumgebungen; sonst ist das hier eine
// Nullpruefung.
```

## L187 · `pub fn env_home(env: &Rc<RefCell<Env>>) -> Option<Gc> {`

```
/// Das Heimatobjekt der naechsten umschliessenden Methode.
```

## L197-201 · `pub fn this_observed(i: &mut Interp, env: &Rc<RefCell<Env>>) -> Value {`

```
/// `this`, PLUS die Frage, ob der Modus daran etwas aendern wuerde.
///
/// Beide Maschinen rufen sie, damit die Sonde nicht in zwei Fassungen
/// auseinanderlaeuft. `undefined`/`null` waere im lockeren Modus `globalThis`,
/// ein Primitiv waere dort eingepackt — beides sieht das Programm.
```

## L212-216 · `pub fn set_env_this(env: &Rc<RefCell<Env>>, v: Value) {`

```
/// `this` NEU binden — in genau der Umgebung, die es traegt.
///
/// Nur `super()` braucht das: bis dahin ist `this` in einer abgeleiteten
/// Klasse noch nicht endgueltig, und ein Elternkonstruktor, der ein Objekt
/// zurueckgibt, entscheidet es.
```

## L235 · `pub struct Realm {`

```
/// Die eingebauten Objekte einer Ausfuehrungseinheit.
```

## L246 · `pub error_ctors: HashMap<&'static str, Gc>,`

```
/// Name -> Prototyp der Fehlerarten, fuer `throw_type` & Co.
```

## L252-254 · `pub event_proto: Gc,`

```
/// `Event.prototype`. Liegt im Realm, weil die Zustellung Ereignisse
/// BAUT und die eingebauten Funktionen Zeiger sind, keine Abschluesse —
/// sie koennen den Prototyp nicht einfangen.
```

## L256-260 · `pub location: Gc,`

```
/// Das EINE `location`-Objekt. Es liegt aus demselben Grund im Realm wie
/// `event_proto`: `window.location = "…"` und `document.location = "…"`
/// sind Setzer, die auf `href` weiterreichen ([PutForwards=href]), und
/// ein eingebauter Setzer ist ein Zeiger — er kann das Objekt nicht
/// einfangen, er muss es nachschlagen koennen.
```

## L267-269 · `pub iterator_proto: Gc,`

```
/// `%IteratorPrototype%` — der gemeinsame Vorfahr aller eingebauten
/// Iteratoren. Er traegt `[Symbol.iterator]() { return this }`, und genau
/// daran haengt, dass ein Iterator selbst wieder iterierbar ist.
```

## L271-274 · `pub generator_proto: Gc,`

```
/// `%GeneratorPrototype%` (`next`/`return`/`throw`) und
/// `%GeneratorFunction.prototype%`. Sie liegen im Realm, weil eingebaute
/// Funktionen Zeiger sind und keine Abschluesse — sie koennen den
/// Prototyp nicht einfangen.
```

## L277-279 · `pub async_iterator_proto: Gc,`

```
/// `%AsyncIteratorPrototype%`, `%AsyncGeneratorPrototype%` und
/// `%AsyncGeneratorFunction.prototype%`. Der erste steht getrennt, weil
/// `for await` ihn auch an einem selbstgebauten async-Iterator findet.
```

## L283-285 · `pub websocket_proto: Gc,`

```
/// `WebSocket.prototype`. Steht im Realm, weil ein `instanceof` gegen
/// einen fehlenden Namen wirft statt `false` zu ergeben — daran ist
/// htmx in 0.140.0 gestorben.
```

## L294-295 · `pub eval_fn: Option<Gc>,`

```
/// Die eingebaute `eval`. Gemerkt, weil ein Aufruf nur dann ein DIREKTER
/// ist, wenn er GENAU sie trifft.
```

## L297-299 · `pub html_element_proto: Gc,`

```
/// Die Schnittstellen-Prototypen der DOM-Bindung. `tag_protos` bildet den
/// Elementnamen auf seine Schnittstelle ab; was nicht darinsteht, ist
/// `HTMLElement`.
```

## L303-306 · `pub event_target_proto: Gc,`

```
/// `EventTarget.prototype` — gebraucht, weil `new EventTarget()` einen
/// LOSGELOESTEN Knoten baut und `wrap` ihm den richtigen Prototyp geben
/// muss. Ueber den globalen Namen zu gehen waere falsch: den darf die
/// Seite ueberschreiben.
```

## L311 · `pub response_proto: Gc,`

```
/// `fetch` und was daran haengt — siehe `fetch.rs`.
```

## L317-319 · `pub mo_proto: Gc,`

```
/// `MutationObserver.prototype` — im Realm, weil der Konstruktor ein
/// Zeiger ist und den Prototyp nicht einfangen kann. Dasselbe gilt fuer
/// die beiden Kasten-Beobachter.
```

## L329 · `pub attr_proto: Gc,`

```
/// `Attr` und `NamedNodeMap` — `el.attributes` haengt beide aneinander.
```

## L335-337 · `pub ta_protos: HashMap<&'static str, Gc>,`

```
/// Die Prototypen der neun Sichten, nach ihrem Namen — `new_typed` haengt
/// eine frische Sicht daran, und eingebaute Funktionen sind Zeiger, die
/// nichts einfangen koennen.
```

## L344 · `#[cfg(feature = "heap-census")]`

```
/// Was ein Zensus der Halde gefunden hat.
```

## L347 · `pub reachable: usize,`

```
/// Objekte, die von den Wurzeln aus zu erreichen sind.
```

## L349-350 · `pub envs: usize,`

```
/// Umgebungen darin — eine Schliessung haelt ihre, und die haelt wieder
/// Schliessungen.
```

## L352-353 · `pub props: usize,`

```
/// Eigenschaften in den erreichbaren Objekten. Sie sind der Preis: beak
/// legt jede einzeln ab, Schluessel als Zeichenkette.
```

## L355-356 · `pub live: usize,`

```
/// Objekte, die insgesamt LEBEN. Nur mit `--features heap-census`, sonst
/// null — und ohne diese Zahl sagt `reachable` nichts.
```

## L361-363 · `fn roots(&self) -> Vec<Gc> {`

```
/// Alles, was der Realm selbst festhaelt. Eine Liste und keine
/// Aufzaehlung von Hand an jeder Stelle: wer ein Feld hinzufuegt, sieht
/// hier, dass es auch abgebaut werden muss.
```

## L393-406 · `pub struct Geometry {`

```
/// Der Kaskadenkontext, den der Wirt einreicht.
/// Die Kaesten des LETZTEN Layouts — was `getBoundingClientRect` und die
/// `offset*`/`client*`-Felder beantworten.
///
/// **Warum das der Wirt einreicht und die Maschine es nicht selbst rechnet:**
/// Geometrie entsteht im Layout, und das Layout ist beaks Sache. Dieselbe
/// Bauart wie `StyleCtx` und `set_media`.
///
/// **Und warum es die Kaesten von VORHIN sind:** ein Skript, das den Baum
/// aendert und sofort misst, bekommt hier den Stand vor seiner Aenderung. Ein
/// Browser legt an dieser Stelle synchron neu aus — auf Wikipedia gemessene
/// 70 ms je Abfrage. Das waere hier machbar und ist bewusst nicht gebaut:
/// erst soll jemand eine Seite zeigen, der es weh tut. Bis dahin ist die
/// letzte echte Geometrie ungleich besser als die Null, die vorher dastand.
```

## L408-410 · `pub boxes: alloc::rc::Rc<alloc::vec::Vec<crate::layout::ElemRect>>,`

```
/// Ein Eintrag je FRAGMENT — ein Inline-Kasten ueber drei Zeilen hat drei,
/// und das ist richtig: `getClientRects` nennt sie einzeln,
/// `getBoundingClientRect` ihre Vereinigung.
```

## L412-413 · `pub scroll: (i32, i32),`

```
/// Der Rollstand des Fensters. Die Kaesten stehen in Dokumentkoordinaten,
/// `getBoundingClientRect` antwortet in Fensterkoordinaten.
```

## L415-421 · `pub content: (i32, i32),`

```
/// Die ROLLFLAECHE des Dokuments: so weit, wie der gemalte Inhalt reicht.
///
/// Sie ist nicht die Vereinigung der Kaesten. Ein Hintergrundbild, ein
/// Schatten, ein Text, der aus seinem Kasten laeuft — all das haelt die
/// Seite rollbar, und das Layout rechnet es ohnehin aus
/// (`Layout::height`). Sie hier zu erraten waere eine zweite Wahrheit
/// ueber dieselbe Zahl.
```

## L431-432 · `pub const STRICT_SITE_NAMES: [&str; STRICT_SITES] = [`

```
/// Die Stellen, an denen der strenge Modus abweicht — in der Reihenfolge des
/// Zaehlfeldes. Nur Diagnose (`--features strict-probe`).
```

## L445-449 · `"set: Empfaenger ist undefined/null (meist: das Objekt fehlt ganz)",`

```
// Getrennt vom echten Primitiv, und das ist keine Feinheit: fast jeder
// Treffer hier heisst „das Objekt gibt es in beak gar nicht", also
// `undefined.foo = 1` aus `propertyHelper.js`. Zusammengezaehlt haetten
// die beiden Faelle die Rangliste angefuehrt und dabei eine ganz andere
// Luecke gemessen als die, die sie zu messen vorgeben.
```

## L454 · `#[cfg(feature = "strict-probe")]`

```
/// Eine Stelle melden. Ohne die Fahne ist es nichts — kein Feld, kein Befehl.
```

## L465-466 · `#[derive(Debug, Clone)]`

```
/// Was eine Seite am Verlauf verlangt hat. **Eine Absicht, keine Tat** —
/// ausgefuehrt wird sie vom Wirt, der als einziger einen Verlauf hat.
```

## L469-470 · `Push { url: String },`

```
/// `pushState(state, title, url)` — `url` ist bereits aufgeloest oder
/// leer, wenn die Seite keine angab.
```

## L472 · `Replace { url: String },`

```
/// `replaceState(...)` — derselbe Eintrag, neue Adresse.
```

## L474 · `Go(i32),`

```
/// `go(n)`, `back()` (= `go(-1)`), `forward()` (= `go(1)`).
```

## L478-484 · `#[derive(Debug, Clone)]`

```
/// Was die Seite per `location` verlangt hat: eine ECHTE Navigation.
///
/// **Der Unterschied zu `HistoryOp` ist der ganze Punkt.** `pushState`
/// schreibt nur die Adresse um und laesst das Dokument stehen;
/// `location.replace` wirft es weg und holt ein neues. Beides in einem Enum
/// zu fuehren hiesse, dass ein Leser sie verwechseln kann — und der
/// teurere der beiden Fehler ist still.
```

## L487 · `pub url: String,`

```
/// Schon gegen die aktuelle Adresse aufgeloest, also absolut.
```

## L489 · `pub replace: bool,`

```
/// `replace()` ersetzt den Verlaufseintrag, `assign()`/`href=` haengt an.
```

## L491 · `pub reload: bool,`

```
/// `reload()` — dieselbe Adresse noch einmal.
```

## L495-500 · `pub struct Timer {`

```
/// Ein angemeldeter Zeitgeber.
///
/// `interval` unterscheidet `setInterval` von `setTimeout`: nur ein Intervall
/// meldet sich nach dem Lauf wieder an. `args` sind die Werte HINTER der
/// Verzoegerung — `setTimeout(f, 0, a, b)` ruft `f(a, b)`, und Bibliotheken
/// schreiben das.
```

## L511-512 · `pub modules: HashMap<Rc<str>, Rc<RefCell<super::modules::Module>>>,`

```
/// Die geholten ES-Module, nach AUFGELOESTER Adresse. Siehe `modules.rs`
/// — die Engine holt nichts, sie verwaltet nur.
```

## L514-516 · `pub module_fail: Option<Rc<str>>,`

```
/// Welches Modul zuerst geworfen hat. Ein Fehler aus einem Graphen von
/// sechsundfuenfzig Adressen nennt sonst nur den EINSTIEG, und das ist
/// die eine Auskunft, die man nicht braucht.
```

## L518-524 · `pub pending_sheets: Vec<(u32, String)>,`

```
/// Stilblaetter, die ein SKRIPT eingehaengt hat und die noch geholt
/// werden muessen: `(Knoten, Adresse wie im Attribut)`.
///
/// Der Wirt holt sie mit `take_pending_sheets` ab, loest auf, laedt, und
/// meldet den Ausgang mit `sheet_done` zurueck — dann erst faellt `load`
/// oder `error` am `<link>`. Ohne diesen Weg wartet jede Seite, die ihr
/// Blatt per Skript nachlaedt, fuer immer auf ein Ereignis, das nie kommt.
```

## L526-535 · `pub pending_scripts: Vec<(u32, String)>,`

```
/// Skripte, die ein SKRIPT eingehaengt hat und die noch geholt werden
/// muessen: `(Knoten, Adresse wie im Attribut)`.
///
/// **Derselbe Weg wie `pending_sheets`, und aus demselben Grund.** Ein
/// `<script src=…>`, das per `appendChild` in die Seite kommt, ist keine
/// Randerscheinung: so laedt jeder code-geteilte Bundler seine Stuecke
/// nach (webpacks `__webpack_require__.l`), und so wartet Next.js auf
/// seine Route, BEVOR es ueberhaupt etwas rendert. Ohne Antwort steht
/// die Seite fuer immer — bei DuckDuckGo raeumte React das
/// servergerenderte HTML weg und rendert dann nichts mehr.
```

## L537-546 · `pub current_script: Option<u32>,`

```
/// Der `<script>`-Knoten, dessen Code GERADE laeuft — `document.currentScript`.
///
/// **Ohne ihn faellt jedes Turbopack-Buendel aus.** Ein von Next.js
/// erzeugtes Stueck meldet sich mit
/// `TURBOPACK.push([document.currentScript, …])` an und wirft sonst
/// „chunk path empty but not in a worker"; DDGs Startseite verlor so
/// sieben Stuecke und ein Inline-Skript auf einmal.
///
/// Ein MODUL hat keinen: dort ist die Antwort laut HTML §4.12.1 `null`,
/// und `import.meta.url` ist der Weg. Wer `None` setzt, sagt genau das.
```

## L548-549 · `pub(crate) new_name: Option<String>,`

```
/// Der NAME des gerade gebauten Konstruktors, fuer die Fehlermeldung.
/// `construct_named` stellt ihn und raeumt ihn wieder weg.
```

## L551-556 · `pub(crate) write_point: Option<(u32, u32)>,`

```
/// Wo das naechste `document.write` desselben Skripts hinschreibt:
/// `(Skriptknoten, zuletzt geschriebener Knoten)`.
///
/// Das Paar trennt sich selbst von einem alten Lauf: passt der erste Wert
/// nicht mehr zu `current_script`, gilt es nicht. Ohne die Stelle landete
/// ein zweites `write` VOR dem ersten.
```

## L558-560 · `pub(crate) ran_scripts: Vec<u32>,`

```
/// Welche Skriptknoten schon gelaufen sind. Die Spezifikation nennt es
/// das „already started"-Kennzeichen: ein Skript, das man noch einmal
/// einhaengt, laeuft NICHT noch einmal.
```

## L562-565 · `pub pending_fetches: Vec<super::fetch::PendingFetch>,`

```
/// Anfragen aus `fetch()`, deren Antwort noch aussteht. **Dasselbe Muster
/// wie `pending_sheets`:** die Engine holt nichts, sie legt die Anfrage
/// hin; der Wirt nimmt sie mit `take_pending_fetches`, laedt, und meldet
/// mit `fetch_done`/`fetch_failed` zurueck.
```

## L567-570 · `pub(crate) fetch_waiting: Vec<(u32, super::fetch::Waiter)>,`

```
/// Wer auf welche Antwort wartet. **Zwei Sorten Warter, eine Leitung:**
/// `fetch` haelt ein Versprechen, ein `XMLHttpRequest` sein eigenes
/// Objekt. Der Wirt kennt den Unterschied nicht — er meldet eine id
/// zurueck, und `fetch_done` entscheidet hier.
```

## L572-574 · `pub aborted_fetches: Vec<u32>,`

```
/// Anfragen, die der Wirt ABBRECHEN soll. `controller.abort()` legt die
/// id hier ab — sonst waere der Abbruch nur eine Fahne im Baum und die
/// Verbindung liefe weiter.
```

## L577-580 · `pub submits: Vec<u32>,`

```
/// Formulare, die die SEITE abschicken will (`form.submit()`), als
/// `seq` des `<form>`. Der Wirt holt sie mit `take_submits` ab und
/// navigiert — die Engine kann das nicht, sie kennt weder Adresse noch
/// Netz. Dasselbe Muster wie `history_ops`.
```

## L582 · `pub pending_rejections: Vec<Gc>,`

```
/// Abgelehnte Versprechen, an denen (noch) nichts haengt.
```

## L584-588 · `pub custom: Vec<(Rc<str>, Value)>,`

```
/// `customElements`: Marke -> Konstruktor, in Reihenfolge der Anmeldung.
///
/// Eine LISTE und keine Tabelle: sie wird bei jedem `new` einmal
/// durchlaufen, um aus dem Prototyp die Marke zu finden, und vierzehn
/// Eintraege sind kein Fall fuer eine Hashtabelle.
```

## L590-593 · `pub depth: usize,`

```
/// Aufruftiefe. Ein Baumlaeufer benutzt den RUST-Stapel, also wird ein
/// zu tiefes JS-Programm zum Stapelueberlauf des Wirts — und das ist im
/// Kernel ein Absturz, kein Fehler. Die Grenze ist deshalb Pflicht, nicht
/// Komfort.
```

## L596-598 · `pub steps: u64,`

```
/// Ausgefuehrte Anweisungen. Ohne Deckel haengt ein `while(true)` den
/// ganzen Lauf auf — und ein Testlaeufer, der an EINEM Programm stehen
/// bleibt, misst gar nichts mehr.
```

## L601-611 · `pub deadline: Option<fn() -> bool>,`

```
/// „Darf noch weitergerechnet werden?" — der Wirt setzt sie, die Engine
/// fragt sie alle 65 536 Schritte.
///
/// **Ein Schrittdeckel misst das Falsche.** Er sollte eine Seite stoppen,
/// die sich aufhaengt, aber er trifft genauso eine, die viel RECHNET: die
/// Anmeldung einer Fritzbox rechnet 66 000 PBKDF2-Runden, und die sind
/// keine Endlosschleife. Ein Browser misst deshalb ZEIT, nicht Schritte.
/// Die Engine hat keine Uhr — also fragt sie den Wirt.
///
/// Alle 65 536 Schritte, nicht bei jedem: der Aufruf selbst darf den
/// heissesten Pfad der Maschine nicht kosten.
```

## L613-614 · `pub fake_now: f64,`

```
/// Eine Uhr, die nur steigt. Ersatz, bis beak die echte einreicht —
/// `beak-engine` ist hostfrei und hat keine.
```

## L616-626 · `pub clock: Option<fn() -> f64>,`

```
/// Die ECHTE Uhr des Wirts, in Millisekunden seit dem Seitenanfang.
///
/// **Ohne sie ist `performance.now()` ein Aufrufzaehler**, und das ist
/// nicht bloss ungenau: Reacts Ablaufplaner fragt `now() - start >= 5`,
/// um zu entscheiden, ob er das Bild abgeben soll. Bei einem Zaehler
/// gibt er nach FUENF Fragen ab — er rechnet in Krumen weiter, statt in
/// Scheiben, und eine grosse Seite wird darueber nie fertig.
///
/// Wie `deadline` ein Funktionszeiger: die Engine hat keine Uhr, der
/// Wirt hat eine. Fehlt sie, bleibt der steigende Zaehler — besser als
/// eine stehende Zahl, an der jede Zeitmessung null ergibt.
```

## L628-630 · `pub epoch_ms: f64,`

```
/// Millisekunden seit der Epoche, wie sie der Wirt beim Sitzungsbeginn
/// gesetzt hat. Die Engine selbst hat keine Uhr; ohne diesen Wert steht
/// `Date.now()` bei 1970 — richtig, aber nutzlos.
```

## L632-634 · `pub doc: Option<super::dombind::Doc>,`

```
/// Das Dokument, auf dem `document` arbeitet. `None`, solange keins
/// eingereicht wurde — dann gibt es `document` gar nicht erst, statt eins
/// vorzutaeuschen, das nichts enthaelt.
```

## L636-642 · `rng: u64,`

```
/// Der Zustand von `Math.random`.
///
/// **Der Wirt saet, nicht die Engine.** `beak-engine` ist hostfrei und hat
/// keine Entropiequelle; sich eine auszudenken waere schlimmer als keine
/// zu haben. Also steht hier eine feste Saat, und wer eine echte hat,
/// reicht sie mit `seed_random` ein. Der Testlaeufer bekommt dadurch
/// nebenbei, was er ohnehin braucht: reproduzierbare Laeufe.
```

## L644-646 · `pub media: Option<(f64, bool)>,`

```
/// Die Medienlage fuer `matchMedia` — Breite und Farbschema-Wunsch. Wie
/// `innerWidth` gehoert sie dem Wirt; ohne `set_viewport` gibt es
/// `matchMedia` gar nicht erst.
```

## L648-655 · `pub style_ctx: Option<StyleCtx>,`

```
/// Was `getComputedStyle` braucht: das Blatt, der Baum, aus dem das
/// Dokument gebaut wurde, das Farbschema und die Fensterbreite.
///
/// **Der Wirt reicht es ein**, wie die Fenstergroesse und die Kekse. Die
/// Maschine hat kein Stilblatt und soll keins holen; sie bekommt eins,
/// wenn jemand eins hat. Ohne diesen Kontext antwortet
/// `getComputedStyle` weiter aus dem Inline-Stil — eine Teilantwort, die
/// die Seite laufen laesst, statt sie mit einem TypeError zu beenden.
```

## L657-660 · `pub vm_ran: u64,`

```
/// Wieviele Programme die BEFEHLSMASCHINE gefahren hat und wieviele der
/// Baumlaeufer — die Zahl, an der die Umstellung gemessen wird. Sie soll
/// steigen, waehrend die test262-Zahl STEHEN BLEIBT: das eine ist der
/// Fortschritt, das andere das Netz.
```

## L663-664 · `pub vm_decline: Option<&'static str>,`

```
/// Warum der Uebersetzer beim letzten Mal abgesagt hat. Ein Name, kein
/// Satz — er wird gezaehlt, und eine Zaehlung braucht einen Schluessel.
```

## L666-669 · `pub vm_off: bool,`

```
/// Aus. Nur fuer die Gegenprobe: derselbe Lauf einmal MIT und einmal OHNE
/// die Befehlsmaschine sagt in einem Diff, welche Tests sie verliert. Ohne
/// diesen Schalter muesste man den Unterschied erraten, und beim ersten
/// Lauf waren es 62 Tests von 69 194 — die findet man nicht durch Lesen.
```

## L671-685 · `pub func_chunks: HashMap<usize, (alloc::rc::Weak<Func>, Option<Rc<super::code::Chunk>>)>,`

```
/// Uebersetzte Funktionsrumpfe, nach der Adresse ihres AST-Knotens.
///
/// Ein `Rc<Func>` ist die Identitaet einer Funktion im Quelltext; derselbe
/// Rumpf wird bei jedem Aufruf gebraucht und darf nur EINMAL uebersetzt
/// werden. `None` heisst „schon versucht, geht nicht" — auch das gehoert
/// gemerkt, sonst uebersetzt eine Schleife bei jedem Umlauf vergeblich.
///
/// **Der `Weak` ist nicht Zierde, er ist der Schluessel.** Eine Adresse
/// ist nur solange eine Identitaet, wie sie belegt ist: gibt der letzte
/// `Rc` den Knoten frei, kann die naechste Funktion GENAU DORT liegen —
/// und bekam dann den Rumpf ihrer Vorgaengerin. Am Geraet heisst das:
/// die Seite ruft ihre Funktion, und es laeuft der Code einer fremden
/// Bibliothek. Ein `Weak` haelt die Zelle belegt, ohne den Baum
/// festzuhalten — die Adresse bleibt damit unverwechselbar, und der AST
/// darf trotzdem sterben.
```

## L687-701 · `pub templates: HashMap<usize, (Vec<Rc<str>>, Value)>,`

```
/// Die Vorlagen-Gegenstaende getaggter Templates (ES 13.2.8.4).
///
/// **Dieselbe Stelle im Quelltext muss bei JEDER Auswertung denselben
/// Gegenstand bekommen** — das ist beobachtbar, und lit-html und
/// styled-components bauen ihren ganzen Zwischenspeicher darauf: sie
/// schluesseln eine `WeakMap` mit dem `strings`-Feld. Ohne stabile
/// Identitaet baut lit bei JEDEM Rendern das DOM neu.
///
/// Geschluesselt wird mit der ADRESSE des Knotens — und weil eine Adresse
/// nur belegt eine Identitaet ist ([[feedback_an_address_is_only_an_identity_while_it_is_occupied]]),
/// liegen die ROHEN Zeichenketten daneben und werden beim Treffer
/// verglichen. Faellt ein Baum weg und legt der naechste eine andere
/// Vorlage auf dieselbe Zelle, gehen die Zeichenketten auseinander und der
/// Gegenstand wird neu gebaut. Sind sie GLEICH, ist das Teilen
/// unbeobachtbar.
```

## L703-705 · `pub sockets: Vec<super::websocket::Socket>,`

```
/// Die offenen `WebSocket`s. **Die Engine oeffnet keine Verbindung** —
/// hier liegt ihr halber Zustand (Handschlag, Rahmen, Puffer), der Wirt
/// fuehrt die Leitung. Dasselbe Muster wie `pending_fetches`.
```

## L707 · `pub socket_objs: HashMap<u32, Gc>,`

```
/// Der JS-Gegenstand je Verbindung, fuer die Zustellung der Ereignisse.
```

## L709 · `pub pending_sockets: Vec<super::websocket::PendingSocket>,`

```
/// Was der Wirt noch aufbauen soll.
```

## L712-718 · `pub func_declines: HashMap<&'static str, u64>,`

```
/// Woran der Uebersetzer bei einem FUNKTIONSRUMPF absagt, je Grund.
///
/// Die Programm-Absagen (`vm_decline`) waren bis Stufe 4 die ganze
/// Rangliste — und sind es seitdem nicht mehr: ein Generator- oder
/// async-Rumpf sagt ab, ohne dass das Programm darum absagt, und die
/// Absage war damit UNSICHTBAR. Eine Rangfolge, die den halben Korpus
/// nicht sieht, ist keine.
```

## L720-726 · `pub pending_labels: Vec<String>,`

```
/// Die Marken, die zur naechsten Schleife gehoeren.
///
/// `outer: for (…)` ist im Baum eine Marke UM eine Schleife; ein
/// `continue outer` gehoert aber der SCHLEIFE — nur sie hat einen
/// Fortsetzungspunkt. Also legt die Marke den Namen hier ab und die
/// Schleife holt ihn beim Betreten. Ohne das lief ein `continue lbl` dem
/// Baumlaeufer durch bis nach oben und beendete das Programm STILL.
```

## L728-733 · `pub vm_ops: u64,`

```
/// Aufrufe, die als RAHMEN liefen, und solche, die ueber den Rust-Stapel
/// mussten. Die zweite Zahl ist das, was Stufe 4 (Anhalten) noch im Weg
/// steht: was ueber Rust laeuft, kann nicht stehenbleiben.
/// Nur zum MESSEN: wie viele Befehle die Maschine wirklich gefahren hat.
/// Der Schrittzaehler zaehlt Anweisungen, nicht Befehle, und aus ihm laesst
/// sich keine ns-je-Befehl-Zahl bilden.
```

## L735-741 · `pub hints_ok: bool,`

```
/// Duerfen die Wegweiser in `Chunk::hints` benutzt werden?
///
/// **Ein direktes `eval` schaltet sie ab, fuer die ganze Sitzung.** Es
/// ist das Einzige, was eine Bindung in eine INNERE Umgebung legen kann,
/// nachdem eine Befehlsstelle schon gelaufen ist — und dann zeigt der
/// Hinweis an der Verschattung vorbei. Eine Pruefung dagegen waere der
/// volle Kettenlauf, also genau das, was der Hinweis spart.
```

## L744-746 · `pub vm_calls_native: u64,`

```
/// Eingebaute, gebundene, Getter — die haben keinen Rumpf aus Befehlen
/// und werden nie einen haben. Sie gehoeren NICHT in denselben Nenner wie
/// eine JS-Funktion, die der Uebersetzer bloss noch nicht kann.
```

## L749-750 · `pub geometry: Option<Geometry>,`

```
/// Siehe `Geometry`. `None` heisst: der Wirt hat keine eingereicht, und
/// dann antwortet die Geometrie mit Nullen wie eh und je.
```

## L752-758 · `pub relayout: Option<fn(&mut Interp)>,`

```
/// **Neu auslegen auf Verlangen.** Der Wirt haengt hier ein, wie er es
/// bei `clock` tut; ohne Haken bleibt alles wie vorher.
///
/// Warum es den Haken braucht: `geometry` ist das letzte BILD. Ein
/// Element, das ein Skript im selben Schritt eingehaengt hat, steht nicht
/// darin und meldet 0 — und eine Seite, die EINMAL misst, bleibt damit
/// fuer immer falsch. Ein Browser rechnet an dieser Stelle synchron neu.
```

## L760-762 · `pub forced_layouts: u32,`

```
/// Wie oft seit dem letzten Bild des Wirts erzwungen ausgelegt wurde.
/// `set_geometry` setzt es zurueck — aber nur, wenn der Wirt ruft und
/// nicht der Haken selbst.
```

## L764-766 · `pub in_forced_layout: bool,`

```
/// Laeuft gerade ein erzwungenes Auslegen? Sperre gegen Rekursion (der
/// Haken ruft `set_geometry`, und darin fragen die Kastenbeobachter
/// wieder nach Kaesten) und gegen das Zuruecksetzen des Deckels.
```

## L768-774 · `pub live_dom: core::cell::RefCell<Option<(u32, alloc::rc::Rc<crate::dom::Dom>)>>,`

```
/// Der lebende Baum in der Form, die die Kaskade lesen kann — gebaut aus
/// `doc`, und nur neu gebaut, wenn `doc.version` sich bewegt hat.
///
/// Frueher hielt `StyleCtx` einen SCHNAPPSCHUSS vom Skriptstart, und
/// `getComputedStyle` antwortete daraus. Ein Skript, das eine Klasse setzt
/// und dann misst, bekam den Stand von vorher — zwei Antworten auf
/// dieselbe Frage. Jetzt gibt es nur noch einen Baum.
```

## L776-791 · `#[cfg(feature = "strict-probe")]`

```
/// Die Kekse dieser Seite, so wie `document.cookie` sie zeigt.
///
/// **Der Wirt reicht sie ein, die Engine hat keinen Behaelter.** Der
/// Behaelter kennt Domain, Pfad, `Secure` und `HttpOnly`; welche davon
/// dieses Dokument sehen darf, ist eine Frage an ihn und nicht an die
/// Maschine. `None` heisst „niemand hat gefragt" — dann gibt es
/// `document.cookie` trotzdem, als leere Zeichenkette, weil ein Skript
/// darauf `.match` ruft und ein `undefined` es toetet. Das ist keine
/// erfundene Antwort: keine Kekse IST eine Antwort.
/// **Nur mit `--features strict-probe`.** Zaehlt die Stellen, an denen der
/// strenge Modus etwas anderes taete als der lockere — jede fuer sich.
///
/// Die Fahnen der Tests (`onlyStrict`) beantworten die Frage NICHT: sie
/// sagen, wie ein Test GESTARTET wird, nicht, ob er an einer dieser
/// Stellen vorbeikommt. Ein Test ohne Fahne, der in seinem Rumpf
/// `"use strict"` schreibt, haengt genauso daran.
```

## L795-797 · `pub cookie_sets: Vec<String>,`

```
/// Was die Seite mit `document.cookie = "…"` gesetzt hat, roh und in der
/// Reihenfolge. Der Wirt holt es sich mit `take_cookie_sets` und legt es
/// in seinen Behaelter — die Engine entscheidet nicht, was gilt.
```

## L799-806 · `pub observers: Vec<super::dombind::MutObs>,`

```
/// Was die Seite mit `history.pushState`/`replaceState`/`go` verlangt hat
/// — roh und in der Reihenfolge. **Die Engine navigiert nicht**, sie hat
/// keinen Verlauf und soll keinen erfinden; sie sammelt, und der Wirt
/// holt es mit `take_history_ops` ab und entscheidet. Dasselbe Muster
/// wie bei den Keksen.
/// Die angemeldeten `MutationObserver`. Sie liegen hier und nicht im
/// Dokument, weil sie einen JS-Rueckruf halten: das Dokument wird bei
/// jeder Navigation neu gebaut, der Realm nicht.
```

## L808-816 · `pub xpath_memo: Option<(alloc::string::String, alloc::rc::Rc<super::xpath::XPath>)>,`

```
/// Die angemeldeten `ResizeObserver` und `IntersectionObserver`.
/// Beide werden in `set_geometry` ausgewertet — dort, wo der Wirt sagt,
/// wie die Seite JETZT steht.
/// Der zuletzt geparste XPath-Ausdruck, gemerkt unter seinem Quelltext.
///
/// `createExpression` gibt es genau deshalb: eine Seite parst EINEN
/// Ausdruck beim Laden und wertet ihn danach oft aus (htmx tut es bei
/// jedem `process`). Ein Eintrag reicht dafuer vollstaendig, und mehr
/// waere eine Tabelle, die niemand raeumt.
```

## L820-821 · `pub viewport: (f64, f64),`

```
/// Breite und Hoehe des Sichtfelds, wie `set_viewport` sie kennt — der
/// Ausschnitt eines `IntersectionObserver` ohne eigene Wurzel.
```

## L824-827 · `pub scroll_want: Option<(Option<f64>, Option<f64>)>,`

```
/// Wohin die Seite rollen will (`scrollTo`, `scrollIntoView`,
/// `scrollTop =`). **Die Engine rollt nicht** — sie hat kein Fenster; der
/// Wirt holt es mit `take_scroll` ab. Je Achse getrennt, weil
/// `scrollTo({ top: 0 })` die andere in Ruhe lassen muss.
```

## L829-834 · `pub nav: Option<NavRequest>,`

```
/// Die Navigation, die die Seite zuletzt verlangt hat (`location.assign`,
/// `.replace`, `.reload`, `href = …`). **Genau eine, und die LETZTE
/// gewinnt** — im Browser bricht eine zweite Navigation die erste ab,
/// also darf hier keine Warteschlange stehen, die beide faehrt.
///
/// Die Engine navigiert nicht; der Wirt holt es mit `take_nav` ab.
```

## L836-840 · `pub loc_href: String,`

```
/// Die Adresse des Dokuments — die EINE Quelle hinter `location`.
///
/// `location` ist deshalb ganz aus Zugriffsfunktionen gebaut und nicht
/// aus Datenfeldern: eine Seite, die `location.pathname` setzt, aendert
/// damit `href` mit, und zwei Kopien liefen sofort auseinander.
```

## L842-844 · `pub history_state: Value,`

```
/// `history.state` — der Zustand, den die Seite zuletzt gesetzt hat.
/// Er gehoert dem DOKUMENT, nicht dem Verlauf des Wirts, und lebt
/// deshalb hier.
```

## L846-847 · `pub history_len: f64,`

```
/// `history.length`, vom Wirt eingereicht. Ohne ihn steht 1 da — ein
/// frisch geladenes Dokument ist immer mindestens ein Eintrag.
```

## L849 · `pub next_sym: u32,`

```
/// Laufende Nummer fuer `Symbol()`.
```

## L851 · `pub sym_registry: HashMap<Rc<str>, Value>,`

```
/// Die globale Symbolregistrierung hinter `Symbol.for`/`Symbol.keyFor`.
```

## L853-856 · `pub jobs: alloc::collections::VecDeque<super::promise::Job>,`

```
/// Die Microtask-Schlange. Sie steht NEBEN `timers`, nicht darin: eine
/// Microtask laeuft vor dem naechsten Zeitgeber, nicht danach — das ist
/// der ganze Unterschied zwischen `Promise.resolve().then(f)` und
/// `setTimeout(f, 0)`.
```

## L858-868 · `pub timers: Vec<Timer>,`

```
/// Angemeldete Zeitgeber. Noch laeuft niemand sie; sie zu HALTEN kostet
/// nichts und ist die Stelle, an der beaks Schleife ansetzt.
///
/// **Die Verzoegerung ist keine Zierde.** Bis 0.174.0 stand hier ein
/// `Vec<Value>`: jeder Rueckruf lief in der Runde nach seiner Anmeldung,
/// in ANMELDEreihenfolge, und `clearTimeout` war ein Nichts. Damit lief
/// ein `setTimeout(f, 120000)` VOR einem `setTimeout(g, 0)` daneben —
/// und genau darauf steht webpacks Nachlader: er meldet einen Zeitgeber
/// als Zeitueberschreitung an und loescht ihn im `onload` des Stuecks.
/// Beides ging schief, also meldete jedes nachgeladene Stueck
/// „ChunkLoadError: timeout", obwohl es angekommen war.
```

## L870-874 · `pub(crate) vnow: f64,`

```
/// Die Uhr, an der die Zeitgeber haengen. Sie laeuft nicht von selbst:
/// ist nichts faellig und liegt noch etwas an, springt sie auf den
/// naechsten Termin. Damit stimmt die REIHENFOLGE immer, auch wenn der
/// Wirt keine Uhr einreicht — und die Reihenfolge ist das, woran echter
/// Code haengt.
```

## L876-877 · `pub(crate) next_timer: u32,`

```
/// Die naechste Kennung. Sie faengt bei 1 an, weil 0 in JavaScript
/// falsch ist und Seiten `if (id)` schreiben.
```

## L879-893 · `pub native_new: bool,`

```
/// Was die Seite auf `console` geschrieben hat.
///
/// Gesammelt statt weggeworfen: `beak-engine` hat keine Serienleitung,
/// aber der Wirt hat eine, und eine Seite, die ihren eigenen Zustand
/// meldet, ist bei einer Ferndiagnose oft das einzige Fenster hinein.
/// Gedeckelt, weil eine fremde Seite sonst den Speicher damit fuellt —
/// und der Verlust wird gemeldet, nicht verschwiegen.
/// Der letzte erfolgreiche Treffer — allein fuer die annexB-Statiken
/// `RegExp.$1`, `RegExp.lastMatch` & Co. Sie stehen NICHT am
/// Ausdrucksobjekt, sondern am Konstruktor, also muss der Zustand hier
/// liegen und nicht dort.
/// Laeuft gerade ein `new` auf einem eingebauten Konstruktor? Ein
/// natives `this` ist bei Aufruf und Bau dasselbe (`undefined`), also
/// braucht es diese Fahne — `Symbol` HAT ein `[[Construct]]`, es wirft
/// nur darin.
```

## L897-898 · `pub console_dropped: usize,`

```
/// Wie viele Zeilen der Deckel verworfen hat. OEFFENTLICH, weil ein
/// stiller Deckel jede Fehlersuche zur Messung des Deckels macht.
```

## L902-904 · `pub struct LastMatch {`

```
/// Wie viele Zeilen `console` haelt, und wie lang eine werden darf.
/// Was die neun `RegExp.$n` und ihre vier Nachbarn brauchen. Fertige
/// Zeichenketten statt Bereiche: die Quelle darf danach verschwinden.
```

## L910 · `pub caps: Vec<String>,`

```
/// `$1` bis `$9`; eine Gruppe ohne Treffer ist die leere Zeichenkette.
```

## L920-927 · `pub const MAX_PROTO_CHAIN: usize = 1000;`

```
/// Wie weit eine Prototypkette laufen darf.
///
/// **Ein Sicherungsnetz, keine Regel der Sprache.** Eine Kette kann einen
/// Zyklus enthalten (`Object.setPrototypeOf` muss ihn zwar ablehnen, aber
/// darauf allein soll sich hier nichts verlassen), und dann laeuft jeder
/// Eigenschaftszugriff fuer immer — in NATIVEM Code, an der Schrittgrenze
/// vorbei. Eine fremde Seite haette damit drei Zeilen gebraucht, um beak
/// aufzuhaengen. Echte Ketten sind ein Dutzend Glieder tief.
```

## L930-937 · `pub const MAX_BUFFER_BYTES: usize = 64 << 20;`

```
/// Wieviele Bytes ein einzelner `ArrayBuffer` haben darf.
///
/// Keine erfundene Grenze, sondern die Antwort auf eine echte: der Lauf ist
/// an `new ArrayBuffer(2**53)` gestorben, und in einem Kernel gibt es keinen
/// Prozess, der dabei alleine stirbt. 64 MB sind mehr, als jede Seite im
/// Zielkorpus je in einem Stueck belegt, und der Fehlschlag ist ein
/// `RangeError` — genau der, den ein echter Motor bei gescheiterter Zuteilung
/// gibt.
```

## L940-950 · `pub const TEST_STEPS: u64 = 200_000;`

```
/// Was ein Testlaeufer setzt.
///
/// Gegen eine Messung gesetzt, nicht gegen ein Gefuehl: ein gewoehnlicher
/// test262-Test kostet **1,9 µs**, also grob hundert Schritte. 2 Mio. waren
/// das Zwanzigtausendfache — und weil diese Maschine rund 11 Mio. Schritte je
/// Sekunde schafft, kostete JEDER Test, der absichtlich mit einer absurden
/// Array-Laenge arbeitet, 180 ms. Davon gibt es in `built-ins/Array` Tausende.
///
/// 200 000 sind immer noch hundertfache Reserve und decken hoechstens 18 ms.
/// Was darueber faellt, verschwindet nicht still: „step budget exhausted"
/// steht als eigene Zeile in der Fehlerkarte.
```

## L969-973 · `super::websocket::install(&mut me);`

```
// **`WebSocket` erscheint nur, wenn es echten Zufall gibt** — es
// braucht eine Maske je Rahmen (RFC 6455 §5.3), und eine
// vorhersagbare waere schlechter als eine fehlende Schnittstelle.
// Dieselbe Regel wie bei `crypto`, und sie steht hier, weil `install`
// den fertigen Interp braucht und nicht nur den Realm.
```

## L1014-1026 · `pub fn box_observations_pending(&self) -> bool {`

```
/// Die angemeldeten Zeitgeber EINMAL durchlaufen.
///
/// Einmal, nicht bis die Schlange leer ist: ein `setTimeout`, das sich
/// selbst neu anmeldet, ist ein voellig normales Muster (Abfrageschleifen,
/// Animationen) und wuerde die Schleife sonst nie verlassen. Was waehrend
/// des Laufs dazukommt, ist beim naechsten Mal dran.
/// Wartet eine Beobachtung auf ihren Rueckruf?
///
/// **Der Wirt fragt das nach jedem Bild.** `set_geometry` MISST, aber
/// zustellen darf nur ein Einstiegspunkt — und ohne diese Frage haette
/// eine Seite, die weder Zeitgeber noch Ereignisse hat, ihre Beobachter
/// angemeldet und nie etwas gehoert. Billig, wenn niemand beobachtet:
/// zwei leere Listen.
```

## L1033-1036 · `super::promise::run_jobs(self);`

```
// Erst die Microtasks, dann die Zeitgeber — das IST die Rangfolge.
// Und ohne diese Zeile bliebe ein `Promise.resolve().then(f)` aus
// einem Ereignisbehandler liegen, bis zufaellig ein Zeitgeber faellig
// wird: `run_timers` kaeme bei leerer Zeitgeberliste gar nicht dazu.
```

## L1039-1048 · `let waiting = !self.pending_scripts.is_empty()`

```
// Ist nichts faellig, springt die Uhr auf den naechsten Termin. Das
// ist keine echte Zeit — aber es ist die richtige REIHENFOLGE, und
// ein Rueckruf, den niemand mehr abbestellt, muss auch laufen.
//
// **Nicht, solange etwas unterwegs ist.** Wer auf eine Antwort
// wartet, darf die Uhr nicht vorstellen: webpack meldet neben jedem
// nachgeladenen Stueck einen Zeitgeber auf 120 SEKUNDEN an und
// loescht ihn im `onload`. Springt die Uhr, waehrend das Stueck noch
// geholt wird, faellt die Zeitueberschreitung VOR der Zustellung —
// und die Seite meldet einen Fehler fuer etwas, das ankommt.
```

## L1062 · `due.sort_by(|a, b| a.due.partial_cmp(&b.due).unwrap_or(core::cmp::Ordering::Equal)`

```
// Gleicher Termin heisst: in der Reihenfolge der Anmeldung.
```

## L1067-1068 · `if let Some(iv) = t.interval {`

```
// Ein `setInterval` meldet sich selbst wieder an — VOR dem Lauf,
// damit ein `clearInterval` im Rueckruf ihn auch erwischt.
```

## L1075-1078 · `if let Err(e) = self.call(&f, Value::Undefined, &args) {`

```
// Ein Zeitgeber, der wirft, muss es SAGEN. Der Ausgang wurde hier
// weggeworfen: ein Fehler in einem `setTimeout`-Rueckruf war
// unsichtbar, und was danach nicht passierte, sah aus wie ein
// fehlendes Merkmal — dieselbe Falle wie beim Ereignisbehandler.
```

## L1083-1085 · `super::promise::run_jobs(self);`

```
// Nach JEDEM Zeitgeber, nicht erst nach allen: Microtasks laufen
// zwischen den Aufgaben, und ein `then`, das der erste Zeitgeber
// anlegt, gehoert vor den zweiten.
```

## L1091-1128 · `#[cfg(feature = "heap-census")]`

```
/// Ein Dokument einreichen und `document` global sichtbar machen.
///
/// Den Realm abbauen und dabei die Ringe brechen.
///
/// **Gemessen 2026-09-04: ein Realm kostet 973 KB und wurde NIE frei.**
/// Zweihundert erzeugte und wieder fallengelassene Maschinen liessen
/// 191 MB liegen. Der Grund ist kein Fehler in einer Zeile, sondern die
/// Bauart: `Rc` zaehlt, und die Form eines JS-Realms ist ringfoermig —
/// `proto.constructor` zeigt auf den Konstruktor, `ctor.prototype`
/// zurueck auf den Prototyp; `globalThis` zeigt auf sich selbst; jede
/// Schliessung haelt ihre Umgebung, und die globale Umgebung haelt die
/// Schliessung. Ein Zaehler kommt aus einem Ring nie auf null.
///
/// Der test262-Lauf hat es aufgedeckt: 69 194 Tests x 973 KB = 67 GB, und
/// der OOM-Killer hat den Lauf erschossen. Bei 519 KB (0.56.0) passte es
/// gerade noch — die Luecke war also schon lange da, nur nicht sichtbar.
///
/// Kein Sammler, sondern ein ABBAU an den Wurzeln: alles, was vom
/// globalen Gegenstand und den Prototypen aus erreichbar ist, wird
/// einmal besucht und geleert. Danach zeigt kein Ring mehr auf sich
/// selbst, und `Rc` raeumt den Rest.
///
/// ⚠ Wer nach dem Fallenlassen noch einen `Value` aus dieser Maschine
/// haelt, haelt danach ein LEERES Objekt. Das ist sicher (der Speicher
/// lebt, solange der `Rc` lebt), aber es ist nicht mehr dasselbe Objekt.
/// Von den Wurzeln aus zaehlen, was erreichbar ist.
///
/// **Der Gang, den `teardown` schon laeuft — nur zaehlend statt
/// abbauend.** `Rc` sammelt keine Ringe ein (`value.rs` sagt es im Kopf),
/// und Reacts Fiberbaum IST einer: `return`, `child`, `sibling`,
/// `alternate` zeigen aufeinander. Die Frage, die vor jedem Sammler
/// steht, ist deshalb nicht „wieviel haelt die Seite", sondern „wieviel
/// davon haelt SIE noch, und wieviel haelt nur sich selbst".
///
/// Die Wurzeln sind ALLE, die der Interpreter hat — nicht nur die des
/// Realms: ein Zeitgeber, ein Beobachter, ein offener `fetch` und jeder
/// Behandler am Baum halten genauso. Wer eine auslaesst, zaehlt
/// lebendigen Bestand als Muell.
```

## L1145 · `#[cfg(feature = "heap-census")]`

```
/// Nur die Markierung — was `collect_cycles` behalten muss.
```

## L1149-1154 · `#[cfg(feature = "heap-census")]`

```
/// Der Gang selbst: von allen Wurzeln aus, ohne etwas anzufassen.
///
/// **Bleibt aus dem ausgelieferten Modul heraus**, solange es keinen
/// Sammler gibt, der ihn braucht: Diagnose darf nicht stoeren, und 279
/// Bytes im Bild sind 279 Bytes fuer niemanden
/// ([[feedback_diagnostics_must_not_disturb]]).
```

## L1197-1199 · `if let Some(d) = &self.doc {`

```
// **Der Baum haelt mit.** Jede Huelle, jeder Behandler und jeder
// `on…`-Wert ist eine Wurzel — und genau daran haengt bei einer
// Anwendung der groesste Teil.
```

## L1234-1236 · `ObjKind::Promise(d) => {`

```
// **Ein Versprechen haelt seine Behandler.** `teardown` laesst
// sie aus; fuer den Zensus waeren sie sonst Muell, obwohl sie
// gebraucht werden.
```

## L1262-1263 · `while let Some(o) = objs.pop() {`

```
// Was ueber die Bindungen dazukam, muss noch durch den
// Objektgang — sonst fehlen dessen Umgebungen.
```

## L1284-1298 · `#[cfg(feature = "heap-census")]`

```
/// Die Ringe brechen, die von keiner Wurzel aus zu erreichen sind.
///
/// **Markieren und kehren, mit `Rc` als Kehrblech.** Der Gang ist derselbe
/// wie in `heap_census`; was er nicht gefunden hat, wird geleert — Werte
/// weg, Prototyp weg, Art auf `Plain`. Damit zeigt kein Ring mehr auf sich
/// selbst, und der Zaehler kommt von allein auf null.
///
/// Gekehrt wird ueber `value::ALL_OBJECTS`: **ohne Verzeichnis kann
/// niemand kehren** — ein Sammler muss aufzaehlen koennen, was es gibt.
///
/// Liefert `(geleert, uebrig)`.
///
/// ⚠ Noch eine MESSUNG, kein Sammler im Betrieb: wer danach noch einen
/// `Value` aus einem geleerten Objekt haelt, haelt ein leeres. Dieselbe
/// Warnung wie bei `teardown`, und derselbe Grund, sie ernst zu nehmen.
```

## L1324-1326 · `self.templates.clear();`

```
// Die Vorlagen-Gegenstaende haengen an keiner Wurzel — der Gang unten
// findet sie nicht. Sie zeigen nur auf Zeichenketten und auf
// `array_proto`, also reicht Loslassen.
```

## L1328-1329 · `self.sockets.clear();`

```
// Eine offene Verbindung haelt ihren JS-Gegenstand, und der haelt
// seine Behandler — das ist ein Ring, den der Gang unten nicht findet.
```

## L1337-1339 · `objs.extend(self.realm.roots());`

```
// Die Prototypen stehen im Realm und sind nicht immer vom globalen
// Gegenstand aus erreichbar (`event_proto` etwa haengt am
// Konstruktor, aber der Umweg ist nicht garantiert).
```

## L1359-1362 · `ObjKind::Generator(g) => {`

```
// Ein angehaltener Generator haelt seine Umgebungen und
// halbfertige Werte in seiner Maschine fest. Sie stehen
// sonst in keiner Eigenschaft und in keiner Bindung — wer
// sie hier auslaesst, laesst einen Rc-Ring stehen.
```

## L1376-1377 · `let mut all_envs: Vec<Rc<RefCell<Env>>> = Vec::new();`

```
// Die Umgebungen dazu: eine Schliessung haelt ihre, und die haelt
// ueber ihre Bindungen wieder Schliessungen.
```

## L1387-1388 · `if let ObjKind::Function(d) = &x.borrow().kind { env_stack.push(d.env.clone()); }`

```
// Objekte aus Umgebungen koennen selbst Umgebungen
// halten — deshalb dieselbe Behandlung.
```

## L1410-1411 · `pub fn set_document(&mut self, doc: super::dombind::Doc) {`

```
/// Erst hier entsteht `document` — vorher gibt es den Namen nicht, und ein
/// Skript, das ihn prueft, bekommt die Wahrheit statt eine leere Huelle.
```

## L1419 · `pub fn console_push(&mut self, line: String) {`

```
/// Eine Zeile von der Seite entgegennehmen.
```

## L1433-1435 · `pub fn take_console(&mut self) -> Vec<String> {`

```
/// Die gesammelten Zeilen herausnehmen. Wurde etwas verworfen, sagt die
/// letzte Zeile es — sonst laese sich eine gedeckelte Ausgabe wie eine
/// vollstaendige.
```

## L1445-1453 · `pub fn seed_random(&mut self, seed: u64) {`

```
/// Die Fenstergroesse einreichen.
///
/// `beak-engine` hat keine — sie gehoert dem Wirt. Vorher gab es
/// `innerWidth` deshalb GAR NICHT, und eine Seite, die ihr Layout danach
/// waehlt, fiel mit `ReferenceError` aus, statt die schmale Fassung zu
/// nehmen. Eine erfundene Zahl waere schlimmer gewesen: sie haette
/// ausgesehen wie eine Messung ([[feedback_invented_fallback_hides_the_fault]]).
/// Eine echte Saat vom Wirt. Ohne sie liefert `Math.random` jedes Mal
/// dieselbe Folge — sichtbar deterministisch statt unsichtbar schlecht.
```

## L1458-1460 · `pub fn next_random(&mut self) -> f64 {`

```
/// xorshift64*. Eine Zahl in [0,1), wie die Spezifikation sie verlangt.
/// Kein Kryptozufall und nicht als solcher gedacht — `crypto.getRandomValues`
/// waere eine eigene Frage und haengt am Wirt.
```

## L1465 · `((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 53) as f64`

```
// Die oberen 53 Bits: genau die Genauigkeit eines f64-Bruchs.
```

## L1469-1473 · `pub fn set_cookies(&mut self, jar: String) {`

```
/// Die Kekse dieser Seite einreichen — was `document.cookie` LIEST.
///
/// Der Wirt gibt die Skript-Sicht (`cookies::script_header_for`), nicht
/// den `Cookie:`-Kopf: ein `HttpOnly`-Keks reist auf der Anfrage mit und
/// darf trotzdem nie in einem Skript stehen.
```

## L1478-1481 · `pub fn take_pending_sheets(&mut self) -> Vec<(u32, String)> {`

```
/// Was die Seite gesetzt hat, herausnehmen. Rohe Erklaerungen
/// (`name=wert; Path=/; Max-Age=…`) — die Regeln kennt der Behaelter.
/// Was die Seite am Verlauf tun WOLLTE. Der Wirt holt es ab und
/// entscheidet; die Liste ist danach leer.
```

## L1486-1487 · `pub fn take_pending_scripts(&mut self) -> Vec<(u32, String)> {`

```
/// Was an eingehaengten Skripten geholt werden will. Der Wirt holt es ab
/// und meldet mit `dombind::script_done` zurueck.
```

## L1492-1493 · `pub fn take_pending_fetches(&mut self) -> Vec<super::fetch::PendingFetch> {`

```
/// Was `fetch()` losschicken will. Der Wirt holt es ab; die Liste ist
/// danach leer.
```

## L1498 · `pub fn take_aborted_fetches(&mut self) -> Vec<u32> {`

```
/// Welche Anfragen abgebrochen gehoeren.
```

## L1511-1512 · `pub fn take_nav(&mut self) -> Option<NavRequest> {`

```
/// Die verlangte Navigation abholen. Der Wirt ist der einzige, der sie
/// ausfuehren kann — er hat Netz und Verlauf.
```

## L1517-1518 · `pub fn set_history(&mut self, len: f64, state: Value) {`

```
/// Der Wirt reicht ein, wie lang sein Verlauf ist und welchen Zustand
/// der aktuelle Eintrag traegt — beim Laden und nach jedem Sprung.
```

## L1524-1526 · `pub fn want_scroll(&mut self, x: Option<f64>, y: Option<f64>) {`

```
/// Einen Rollwunsch merken. Der LETZTE gewinnt, so wie bei der
/// Navigation: im Browser bricht ein zweiter Sprung den ersten ab. Eine
/// Achse ohne Wunsch behaelt den Wunsch von vorhin.
```

## L1532 · `pub fn take_scroll(&mut self) -> Option<(Option<f64>, Option<f64>)> {`

```
/// Was die Seite verlangt hat, und danach ist es weg.
```

## L1541-1542 · `pub fn set_style_context(&mut self, ctx: StyleCtx) {`

```
/// Den Kaskadenkontext einreichen — damit `getComputedStyle` echte Werte
/// liefern kann statt nur des Inline-Stils.
```

## L1547-1549 · `pub fn set_geometry(&mut self, g: Geometry) {`

```
/// Die Kaesten des letzten Layouts einreichen — siehe `Geometry`.
/// Der Rollstand aendert sich ohne Layout, also gehoert er MIT hinein und
/// wird bei jedem Einreichen nachgezogen.
```

## L1552-1553 · `if !self.in_forced_layout { self.forced_layouts = 0; }`

```
// Ein Bild des Wirts gibt das Budget zurueck; ein erzwungenes
// Auslegen nicht — sonst waere der Deckel keiner.
```

## L1555-1559 · `super::dombind::eval_box_observers(self);`

```
// **Hier und nirgends sonst.** `ResizeObserver` und
// `IntersectionObserver` fragen nicht nach der Zeit, sondern nach
// dem Kasten — und der steht genau jetzt fest. Ein Zeitgeber, der
// raet, wann sich etwas bewegt haben koennte, waere die falsche
// Frage und die teurere dazu.
```

## L1563-1567 · `pub fn set_location(&mut self, url: &str) {`

```
/// Die Adresse der Seite einreichen. Fuellt `location` und `document.URL`.
///
/// Vorher stand dort `about:blank` — eine Zahl, die aussieht wie eine
/// Messung: ein Skript, das seinen Pfad prueft, nahm den falschen Zweig
/// und meldete keinen Fehler dabei.
```

## L1574-1577 · `self.loc_href = p.href();`

```
// **Nur eine Zeile schreibt die Adresse.** `location` ist ganz aus
// Zugriffsfunktionen gebaut, die alle aus `loc_href` lesen — wer die
// Teile hier als Datenfelder mitschreiben wuerde, haette eine zweite
// Wahrheit, die beim ersten `location.pathname = …` auseinanderlaeuft.
```

## L1587-1589 · `let g = super::value::native(Some(fp.clone()),`

```
// `document.location` ist DASSELBE Objekt wie `window.location`, und
// eine Zuweisung darauf navigiert, statt es zu ersetzen — dieselbe
// Regel wie am Fenster ([PutForwards=href]).
```

## L1598-1600 · `pub fn set_media(&mut self, w: f64, h: f64, dark: bool) {`

```
/// Wie `set_viewport`, aber mit dem Farbschema dazu. `matchMedia` braucht
/// beides, und ein `prefers-color-scheme`, das immer hell sagt, waere eine
/// erfundene Antwort.
```

## L1628-1634 · `pub fn tick(&mut self) -> C<()> {`

```
/// Ein Arbeitsschritt in einer EINGEBAUTEN Schleife.
///
/// Der Deckel in `exec` zaehlt nur Anweisungen — die Schleifen in
/// `Array.prototype.*` und `iterate` laufen daran vorbei. `new
/// Array(2**32-1).join()` haengt damit unbegrenzt, und genau das hat den
/// ersten Ausfuehrungslauf ueber den Zeitdeckel getragen. Also zaehlen
/// diese Schleifen mit.
```

## L1644-1655 · `#[inline]`

```
/// Die UHR des Wirts — alle 65 536 Schritte.
///
/// **Sie stand bis 0.117.0 nur in `tick`, und `tick` ruft nur, wer in
/// einem EINGEBAUTEN schleift.** Reines JS kam nie vorbei: eine
/// Anmeldung, die ihren HMAC selbst rechnet, lief vier Minuten ohne
/// einen Herzschlag, und das Zeitbudget war fuer genau den Fall
/// unwirksam, fuer den es gedacht ist. Eine echte Endlosschleife hing
/// bis zum Schrittdeckel — bei 20 Mrd. Schritten anderthalb Stunden.
///
/// Sie gehoert deshalb dorthin, wo BEIDE Maschinen ihre Schritte
/// zaehlen. Der heisse Pfad zahlt eine Maske und einen Sprung; der
/// Aufruf selbst kommt alle 65 536 Schritte, das sind rund 60 ms.
```

## L1657-1658 · `pub fn now_ms(&mut self) -> f64 {`

```
/// Millisekunden seit dem Seitenanfang — echt, wenn der Wirt eine Uhr
/// eingereicht hat, sonst der steigende Zaehler.
```

## L1675 · `pub fn throw_kind(&mut self, kind: &'static str, msg: &str) -> Abrupt {`

```
// ── Fehler ───────────────────────────────────────────────────────────
```

## L1685-1705 · `pub fn not_a_constructor(&mut self, f: &Value) -> Abrupt {`

```
/// „x is not a function" — mit dem NAMEN, wo es einen gibt.
///
/// „value is not a function" war im Zielkorpus mit 46 Fehlschlaegen der
/// haeufigste Grund ueberhaupt und sagte ueber keinen einzigen, WAS fehlt
/// ([[feedback_print_the_identifier_not_just_the_event]]). Eigene
/// Funktion, weil beide Maschinen sie brauchen: als `switch` und `for..in`
/// uebersetzbar wurden, wanderten zwei Korpusskripte auf die Maschine —
/// und verloren dabei still ihren Namen in der Meldung. Die Prozentzahl
/// hat das nicht gesehen, der Wandvergleich schon.
/// `on` ist der EMPFAENGER, auf dem gesucht wurde. Der Name allein sagt
/// bei einem haeufigen Wort wie `render` nicht, WESSEN `render` fehlt —
/// und in einem minifizierten Buendel steht kein zweiter Hinweis
/// daneben ([[feedback_a_runtime_error_without_a_position_costs_an_hour]]).
/// „value is not a constructor" sagte ueber den Wert GAR NICHTS — und in
/// einem minifizierten Buendel steht daneben kein zweiter Hinweis
/// ([[feedback_a_runtime_error_without_a_position_costs_an_hour]]).
///
/// Die drei Antworten, die den Fall entscheiden: ein `undefined` heisst
/// fehlender Name oder fehlender Import, ein Pfeil/eine Methode/ein
/// Generator heisst falsche Bauart, und ein eingebauter ohne `new` heisst
/// Absicht der Spezifikation.
```

## L1707 · `let art = match f {`

```
// Die BAUART, kurz: sie sagt, warum es keiner ist.
```

## L1722-1724 · `if let Some(n) = self.new_name.clone() {`

```
// Der Name der RUFSTELLE entscheidet den Fall: „Intl.PluralRules is
// not a constructor (undefined)" nennt eine fehlende Schnittstelle,
// „undefined is not a constructor" nur einen Zustand.
```

## L1729-1730 · `let own = match f {`

```
// Ohne Namen an der Rufstelle: der eigene Name des Werts, wenn er
// einen hat.
```

## L1749-1751 · `let ctor = self.get(v, "constructor").ok()`

```
// Der Name der Bauart zuerst: „auf einer Instanz von Foo"
// sagt in einem minifizierten Buendel mehr als jede
// Eigenschaftsliste.
```

## L1760-1761 · `if let Some(pr) = b.proto.clone() {`

```
// Was die BAUART kann, sagt mehr als was die Instanz traegt:
// bei einer Komponente ohne `render` ist die Frage „welche".
```

## L1787-1790 · `pub fn to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {`

```
// ── Umwandlungen ─────────────────────────────────────────────────────
/// `ToPrimitive`. `hint_string` waehlt die Reihenfolge von `toString` und
/// `valueOf` — das ist der ganze Unterschied zwischen `"" + obj` und
/// `1 * obj`.
```

## L1795-1798 · `pub fn to_primitive_hint(&mut self, v: &Value, hint: &str) -> C<Value> {`

```
/// Mit dem DRITTEN Wunsch: `"default"`. Er unterscheidet sich fuer
/// gewoehnliche Objekte in nichts von `"number"` — aber `Symbol.
/// toPrimitive` bekommt ihn zu sehen, und ein `Date` macht daraus Text.
/// Ohne ihn waere `date + ""` eine Zahl.
```

## L1801-1802 · `let exotic = self.get(v, SYM_TO_PRIMITIVE)?;`

```
// `Symbol.toPrimitive` geht VOR `valueOf`/`toString` — es ist der
// einzige Weg, auf dem ein Objekt beide ueberstimmen kann.
```

## L1813-1816 · `pub fn ordinary_to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {`

```
/// `OrdinaryToPrimitive` — `valueOf` und `toString` in der Reihenfolge,
/// die der Wunsch vorgibt. Herausgezogen, weil `Date.prototype[Symbol.
/// toPrimitive]` sie ruft: sonst waere sie dort ein zweites Mal
/// geschrieben.
```

## L1838-1839 · `Value::BigInt(_) => return self.type_err("cannot convert a BigInt value to a number"),`

```
// Eine grosse Zahl wird NICHT still zu einer kleinen. Das ist der
// ganze Sinn des Typs: `1n + 1` ist ein Fehler, keine 2.
```

## L1845-1847 · `pub fn to_bigint(&mut self, v: &Value) -> C<super::bigint::Big> {`

```
/// `ToBigInt` — die Umwandlung, die eine 64-Bit-Sicht beim Schreiben
/// verlangt. Eine gewoehnliche Zahl wirft: der Uebergang muss im
/// Quelltext stehen.
```

## L1861-1863 · `pub fn to_numeric(&mut self, v: &Value) -> C<Value> {`

```
/// `ToNumeric` — eine grosse Zahl bleibt gross, alles andere wird eine
/// gewoehnliche. Der Unterschied zu `to_number` ist genau der Grund,
/// warum `x++` auf einem BigInt nicht `+x` sein darf.
```

## L1870 · `pub fn step_numeric(&mut self, v: &Value, up: bool) -> C<Value> {`

```
/// Eins dazu oder eins weg, im TYP des Wertes.
```

## L1888-1890 · `Value::Sym(_) => return self.type_err("cannot convert a Symbol value to a string"),`

```
// Absichtlich ein Fehler, kein Text. `"" + sym` ist fast immer ein
// Versehen; `String(sym)` und `sym.toString()` gehen weiterhin,
// die rufen `sym_to_display` statt hier durch.
```

## L1897-1900 · `pub fn to_prop_key(&mut self, v: &Value) -> C<Rc<str>> {`

```
/// `ToPropertyKey`. Der EINE Punkt, an dem ein Symbol zum
/// Eigenschaftsnamen wird — jeder berechnete Zugriff (`o[k]`,
/// Objektliteral, Klassenglied, `in`, `defineProperty`) laeuft hier
/// durch, und nur hier.
```

## L1908-1909 · `pub fn sym_to_display(sd: &SymData) -> Rc<str> {`

```
/// Wie ein Symbol GESCHRIEBEN aussieht: `Symbol(desc)`. Nicht `to_string`
/// — das wirft mit Absicht.
```

## L1917-1923 · `pub fn new_symbol(&mut self, desc: Option<Rc<str>>) -> Value {`

```
/// Ein frisches Symbol. Die laufende Nummer macht den Schluessel einmalig
/// — zwei `Symbol("x")` sind damit verschieden, wie die Spezifikation es
/// verlangt.
/// Die Beschreibung steht MIT im Schluessel — `Object.getOwnPropertySymbols`
/// bekommt nur ihn zu sehen und muss das Symbol daraus wieder aufbauen
/// koennen ([[sym_from_key]]). Die laufende Nummer davor haelt ihn
/// einmalig, damit zwei `Symbol("x")` verschieden bleiben.
```

## L1934-1935 · `pub fn to_object(&mut self, v: &Value) -> C<Gc> {`

```
/// `ToObject`: Primitive bekommen ihre Huelle. Das ist der Weg, ueber den
/// `"abc".length` funktioniert.
```

## L1967-1968 · `ObjKind::Proxy(c) => match c.borrow().clone() {`

```
// Ein Stellvertreter ist aufrufbar, wenn sein ZIEL es ist —
// `typeof new Proxy(f, {})` ist "function".
```

## L1977-1984 · `pub fn is_constructor(&self, v: &Value) -> bool {`

```
/// Darf `new` darauf? Ein Pfeil, eine Methode, eine async-Funktion und
/// ein Generator sind KEINE Konstruktoren — und `Reflect.construct` mit
/// einem solchen als `newTarget` muss werfen. Genau daran haengt der
/// `isConstructor`-Helfer von test262, den ein paar hundert Tests rufen.
///
/// Benannt statt verschwiegen: eine Methodenkurzform (`{ m(){} }`) sieht
/// in unserem Baum aus wie eine gewoehnliche Funktion und gilt hier
/// deshalb faelschlich als Konstruktor.
```

## L2001 · `pub fn get(&mut self, base: &Value, key: &str) -> C<Value> {`

```
// ── Eigenschaften ────────────────────────────────────────────────────
```

## L2004-2007 · `if let Value::Str(s) = base {`

```
// Primitive bekommen KEINE Huelle fuer einen blossen Lesezugriff —
// ausser bei Zeichenketten, wo Laenge und Index direkt beantwortet
// werden. Eine Huelle je Zugriff waere sonst der teuerste Weg zu
// `s.length`.
```

## L2017-2032 · `let start = match base {`

```
// **Ein Primitiv bekommt KEINE Huelle fuers Lesen** — der Kommentar
// oben sagte das schon, der Code tat es nicht: er rief `to_object`.
//
// Und das ist bei einer Zeichenkette kein kleiner Umweg.
// `to_object` legt dort JEDES ZEICHEN als eigene Eigenschaft an —
// ein `String` je Zeichen, ein `num_to_string` je Index, ein
// Hashtabellen-Eintrag je Zeichen. Bei 89 KB sind das 89 000
// Eintraege, gebaut fuer EINEN Aufruf von `s.indexOf(...)`, dessen
// Antwort auf dem PROTOTYP liegt. In einer Schleife ueber eine lange
// Zeichenkette ist das quadratisch.
//
// Gemessen auf einer Google-Suchseite: `to_object` war **22 % der
// Laufzeit**, und die wachsende Hashtabelle darunter noch einmal
// 6,6 %. Eigene Eigenschaften hat ein Primitiv hier keine — `length`
// und die Indizes beantwortet der Schnellweg darueber —, also faengt
// die Kette gleich beim Prototyp an.
```

## L2044-2048 · `if let Some(v) = ta_read(&start, key) { return Ok(v) }`

```
// **Eine SICHT beantwortet ihre Indizes selbst, und zwar ENDGUELTIG.**
// Die Prototypenkette wird dabei NICHT gelaufen: `ta[99]` gibt
// `undefined`, auch wenn `Object.prototype[99]` existiert. Das ist
// kein Detail — es ist der Unterschied zwischen einer Sicht und einem
// gewoehnlichen Objekt mit Zahlen als Schluesseln.
```

## L2050-2056 · `if let super::value::ObjKind::ModuleNs(url) = &start.borrow().kind {`

```
// **Ein Modul-Namensraum liest die BINDUNG, nicht ihren Wert von
// damals.** `export let x` plus ein Setter heisst, dass `ns.x` sich
// aendert, ohne dass jemand `ns` anfasst; eine Momentaufnahme in der
// Eigenschaftstabelle zeigt fuer immer den Anfangswert. Genau daran
// starb sandbox.nopeek.chs `init()`: `state.canvas` blieb `null`,
// `getContext` warf, und die Zeile darunter — die alle Behandler
// anmeldet — lief nie.
```

## L2063-2064 · `if super::proxy::parts(&start).is_some() {`

```
// Ein Stellvertreter beantwortet JEDEN Zugriff selbst — die
// Prototypenkette darunter wird nicht gelaufen.
```

## L2074-2075 · `let mut cur = Some(start);`

```
// Array-`length` lebt in der Eigenschaftstabelle wie alles andere;
// nur die Kette darunter wird hier gelaufen.
```

## L2097-2118 · `pub fn private_in(&mut self, name: &str, base: &Value) -> C<Value> {`

```
/// `Set(O, P, V, Throw)` (ES §7.3.4).
///
/// **Die Wurf-Fahne ist ein ARGUMENT, kein Modus.** Das ist der Punkt, den
/// beak bis 0.98.0 nicht hatte: nicht nur strenger Code will einen Fehler,
/// wenn ein Schreiben scheitert, sondern auch fast jede eingebaute
/// Funktion — `[].push` auf einem eingefrorenen Feld muss in BEIDEN Modi
/// werfen. Deshalb steht die Fahne hier und wird an jeder Rufstelle
/// entschieden; ein Vorgabewert haette genau die Haelfte still falsch
/// gelassen.
/// **Die Markenpruefung** (ES §7.3.28 `PrivateGet`/`PrivateSet`).
///
/// Ein privates Feld ist keine Eigenschaft, die man anlegen kann: es
/// entsteht im Konstruktor und nirgends sonst. Ein Zugriff auf ein
/// Objekt, das es nicht hat, ist ein TypeError — nicht `undefined`, und
/// erst recht kein stilles Anlegen. Ohne diese Pruefung war
/// `fremdesObjekt.#f` schlicht `undefined`, und Schreiben legte das Feld
/// an: die Kapselung war eine Verabredung, keine Grenze.
/// `#x in obj` — die Markenpruefung als AUSDRUCK (ES §13.10.1).
///
/// Sie WIRFT nicht, sie antwortet mit ja/nein; das ist ihr ganzer Zweck.
/// Der Parser macht daraus einen Bezeichner `#x` — ein echter Name kann
/// nie mit `#` anfangen, also ist das eindeutig.
```

## L2140-2142 · `if matches!(base, Value::Undefined | Value::Null) { strict_site!(self, 11); }`

```
// Zuweisung an eine Eigenschaft eines Primitivs verpufft still
// (im lockeren Modus). Der strenge Modus wuerde werfen — das
// gehoert zu den Dingen, die der Lauf als offen ausweist.
```

## L2146-2147 · `return self.type_err(&alloc::format!(`

```
// `undefined.x = 1` wirft ohnehin schon beim Lesen der Basis;
// hier geht es um `"abc".x = 1` im strengen Modus.
```

## L2153-2154 · `if let Some(t) = ta_of(o) {`

```
// Dieselbe Endgueltigkeit beim Schreiben: ausserhalb der Sicht
// verpufft es, und ein Setzer in der Kette bekommt es nie zu sehen.
```

## L2157-2158 · `if t.kind.is_big() {`

```
// Die Umwandlung laeuft AUCH, wenn der Index draussen liegt —
// sie ist beobachtbar.
```

## L2179-2180 · `let ds = match &o.borrow().kind { ObjKind::Dataset(id) => Some(*id), _ => None };`

```
// `el.dataset.x = v` schreibt das ATTRIBUT — sonst ist es eine
// Zuweisung an eine Kopie, und die Seite wundert sich.
```

## L2186-2188 · `o.borrow_mut().define(key, Prop::data(Value::Str(text)));`

```
// Und dieselbe Momentaufnahme mitfuehren: wer sich das Objekt in
// einer Variablen haelt, soll seinen eigenen Schreibvorgang lesen
// koennen.
```

## L2209 · `let mut cur = Some(o.clone());`

```
// Ein Setzer irgendwo in der Kette gewinnt vor dem eigenen Feld.
```

## L2223 · `if p.is_accessor() {`

```
// Nur ein Getter, kein Setzer: das Schreiben scheitert.
```

## L2264-2266 · `fn fix_array_length(&mut self, o: &Gc, key: &str) {`

```
/// Ein Array haelt `length` selbst nach: eine Zuweisung an einen Index
/// jenseits der Laenge schiebt sie nach. Ohne das ist `push` gebaut, aber
/// `a[0]=1; a.length` bleibt 0.
```

## L2281-2283 · `if super::proxy::parts(o).is_some() {`

```
// Ein Stellvertreter kann hier WERFEN; `has_property` gibt aber nur
// ein Ja/Nein. Der geworfene Wert geht dabei verloren — benannt statt
// verschwiegen, `has_prop` daneben reicht ihn durch, und `in` ruft die.
```

## L2304 · `pub fn call(&mut self, callee: &Value, this_val: Value, args: &[Value]) -> C<Value> {`

```
// ── Aufrufen ─────────────────────────────────────────────────────────
```

## L2320-2321 · `if super::proxy::parts(f).is_some() {`

```
// Die `apply`-Falle. Ohne sie liefe ein Aufruf auf einen
// Stellvertreter am Behandler vorbei ans Ziel.
```

## L2347-2351 · `if d.node.is_generator && !d.node.is_async {`

```
// **Ein Generator laeuft seinen Rumpf hier NICHT.** Der Aufruf
// baut ein Objekt, und der Rumpf faengt erst beim ersten
// `next()` an — auf einer eigenen Maschine. Das ist die
// einzige Stelle, an der ein Generatorobjekt entsteht; die
// Befehlsmaschine schickt ihre Aufrufe absichtlich hierher.
```

## L2357-2360 · `if d.node.is_async && !d.node.is_generator {`

```
// Und eine async-Funktion gibt ein VERSPRECHEN zurueck. Ihr
// Rumpf laeuft bis zum ersten `await` sofort weiter, dann
// haelt er an — dieselbe Maschine, nur wirft ihn die
// Microtask-Schlange wieder an.
```

## L2366-2368 · `if d.node.is_async && d.node.is_generator {`

```
// Und ein async-Generator ist beides: er gibt ein Objekt
// zurueck wie ein Generator, und jedes `next()` daran gibt ein
// Versprechen wie eine async-Funktion.
```

## L2379-2386 · `pub fn call_env(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value])`

```
/// Die Umgebung, in der ein Aufruf laeuft — alles, was VOR dem ersten
/// Schritt des Rumpfes passiert: `this`, `arguments`, das Heimatobjekt,
/// die Parameter.
///
/// Eigene Funktion, weil die Befehlsmaschine sie braucht: sie legt danach
/// einen RAHMEN an, statt den Rumpf ueber den Rust-Stapel zu fahren. Zwei
/// Umsetzungen dieses Vorspanns waeren zwei verschiedene Aufrufsemantiken,
/// und das ist die teuerste Sorte Unterschied.
```

## L2390-2391 · `env.borrow_mut().home = d.home_object.clone();`

```
// Ein Pfeil bekommt KEIN eigenes `this` — dadurch findet `this_of`
// das der umgebenden Funktion.
```

## L2393-2397 · `if d.node.strict { env.borrow_mut().strict = true; }`

```
// Die Strenge des RUMPFES, nicht die des Rufers: eine strenge
// Funktion bleibt streng, egal wer sie ruft, und eine lockere bleibt
// locker, auch wenn strenger Code sie aufruft. `Env::new` hat gerade
// die des Definitionsortes geerbt; ein `"use strict"` im Rumpf legt
// hier drauf.
```

## L2408-2411 · `if let Some(c) = &d.class {`

```
// **Instanzfelder einer BASISklasse stehen, bevor der Rumpf laeuft.**
// Eine abgeleitete legt sie erst nach `super()` an: vorher hat sie
// zwar schon ein Objekt, aber ein Initialisierer darf ein Feld der
// Elternklasse sehen, und das gibt es erst danach.
```

## L2421-2428 · `fn bind_this(&mut self, t: Value, strict: bool) -> C<Value> {`

```
/// `OrdinaryCallBindThis` (ES §10.2.1.2) — was `this` im Rumpf WIRKLICH ist.
///
/// **Der Unterschied ist der Modus, und beide Seiten waren falsch.** Eine
/// strenge Funktion bekommt den Wert unveraendert: `f()` sieht `undefined`.
/// Eine lockere sieht dort `globalThis`, und ein Primitiv wird ihr
/// EINGEPACKT — `(7).f()` sieht ein `Number`-Objekt, kein `7`. beak gab
/// bis 0.98.0 immer den strengen Wert, in beiden Modi; das war der
/// groesste einzelne Posten der Messung (257 Varianten, 191 davon locker).
```

## L2438-2443 · `pub fn init_fields(&mut self, d: &Rc<FuncData>, this_val: &Value) -> C<()> {`

```
/// Die Instanzfelder einer Klasse auf ein frisches `this` legen.
///
/// Jeder Initialisierer ist ein eigener kleiner Funktionsbereich mit
/// `this` auf der Instanz und `home` der Klasse — ein Pfeil darin faengt
/// die INSTANZ ein, und `super.x` darin trifft die Elternklasse. Der
/// Bereich darum ist der der KLASSE (`d.env`), nicht der des Aufrufers.
```

## L2459-2460 · `self.name_function(&val, &k);`

```
// `x = function(){}` gibt der Funktion den Feldnamen —
// dieselbe Regel wie bei `var f = function(){}`.
```

## L2471-2477 · `pub fn run_js_body(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value]) -> C<Value> {`

```
/// Einen Funktionsrumpf mit dem BAUMLAEUFER fahren — der Weg, den ein
/// Aufruf nimmt, wenn der Uebersetzer den Rumpf nicht kann.
///
/// Eigene Funktion, weil `generator.rs` sie fuer denselben Fall braucht:
/// eine async-Funktion mit unuebersetzbarem Rumpf muss trotzdem ein
/// VERSPRECHEN zurueckgeben, und dafuer muss jemand den Rumpf fahren und
/// den Ausgang einsammeln.
```

## L2480-2488 · `if !d.node.is_generator && !d.node.is_async {`

```
// **Der Rumpf gehoert auf die Maschine, auch wenn der Ruf nicht von
// ihr kommt.** Ein Aufruf aus einem eingebauten Rueckruf, aus der
// Microtask-Schlange oder aus einem Generator landete hier — und weil
// der Baumlaeufer seine eigenen Aufrufe wieder hierher schickt, blieb
// ALLES darunter bei ihm. Gemessen an der Fritzbox-Anmeldung: 320 721
// Schritte, davon 4 286 auf der Maschine.
//
// Generator und async bleiben aussen vor: ihre Rumpfe halten an, und
// dafuer gibt es `generator.rs` mit einer eigenen Maschine je Aufruf.
```

## L2494 · `Ok(Value::Undefined) => Ok(implicit),`

```
// Ein Konstruktor ohne `return` liefert sein `this`.
```

## L2501-2504 · `let implicit = || if d.class.is_some() { env_this(&env) } else { Value::Undefined };`

```
// Ein KONSTRUKTOR ohne `return` liefert sein `this` — und das kann
// `super()` inzwischen umgehaengt haben. Bei einer Basisklasse ist es
// dasselbe Objekt, das `construct` ohnehin nimmt; bei einer
// abgeleiteten ist es der Unterschied.
```

## L2513-2514 · `pub fn hoist_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {`

```
/// Das Hochziehen eines Funktionsrumpfes — `var` und Funktionen nach vorn.
/// Fuer die Maschine, die den Rumpf danach als Befehle faehrt.
```

## L2519-2520 · `pub fn func_chunk(&mut self, f: &Rc<Func>) -> Option<Rc<super::code::Chunk>> {`

```
/// Der uebersetzte Rumpf dieser Funktion, oder `None`, wenn der
/// Uebersetzer absagt. Einmal je Funktion, dann gemerkt.
```

## L2540-2546 · `pub fn template_object(&mut self, quasis: &[super::ast::TemplateElement]) -> Value {`

```
/// `GetTemplateObject` (ES 13.2.8.4) — der Gegenstand, den ein getaggtes
/// Template seiner Marke als erstes Argument reicht.
///
/// Ein eingefrorenes Feld der GEKOCHTEN Zeichenketten (`undefined`, wo die
/// Fluchtfolge ungueltig war — genau dafuer ist `cooked` ein `Option`),
/// mit einem ebenfalls eingefrorenen `raw` daneben. Beides nicht
/// schreibbar, nicht aufzaehlbar, nicht loeschbar.
```

## L2555-2557 · `let frozen = |o: &Gc| {`

```
// Einfrieren wie `Object.freeze` — und mit derselben Vorsicht: die
// Ausleihe muss VOR dem Schreiben enden, sonst paniked das
// `borrow_mut` darin.
```

## L2560-2563 · `let keys = o.borrow().own_keys();`

```
// Die Schluessel ZUERST in eine eigene Liste — als
// `for k in o.borrow().own_keys()` geschrieben lebt die Leihgabe
// bis zum Ende des Rumpfes, und das `borrow_mut` darin paniked.
// Wortgleich mit `Object.freeze`, und aus demselben Grund.
```

## L2593-2595 · `{`

```
// `arguments` ist iterierbar — `[...arguments]` und `for (a of
// arguments)` sind gewoehnliche Schreibweisen, und beide gehen ueber
// dieselbe Funktion wie `Array.prototype.values`.
```

## L2611-2623 · `for p in params {`

```
// **Erst ALLE Parameternamen HIER anlegen, dann binden.**
//
// `bind_pattern(…, true)` bindet ueber `init_binding`, und das laeuft
// die Umgebungskette HOCH: ein Parameter, der so heisst wie eine
// Variable weiter aussen, schrieb in DIESE. Minifizierter Code
// benutzt ueberall dieselben kurzen Namen, also traf das jedes
// Bundle — auf der Fritzbox-Anmeldeseite hat `o=(o,a)=>{…}` beim
// ersten Aufruf das aeussere `o` mit einer 0 ueberschrieben, und der
// naechste Zugriff darauf war „bind is not a function".
//
// Vor dem Binden, nicht je Parameter: `function f(a = b, b)` ist ein
// ReferenceError und darf nicht das aeussere `b` finden
// (`FunctionDeclarationInstantiation`, ES §10.2.11 Schritt 21).
```

## L2647 · `pub fn run_program(&mut self, prog: &Program) -> C<Value> {`

```
// ── Programm ─────────────────────────────────────────────────────────
```

## L2650-2653 · `env.borrow_mut().strict = prog.strict;`

```
// Die globale Umgebung gehoert der EINHEIT, die gerade laeuft: ein
// Skript mit `"use strict"` faerbt sie streng, das naechste ohne
// wieder locker. Beide Faelle kommen im test262-Lauf vor — Vorspann
// locker, Testkoerper streng.
```

## L2655-2656 · `self.hoist(&prog.body, &env, &env)?;`

```
// Hochziehen ist fuer BEIDE Maschinen dasselbe: es arbeitet auf der
// Umgebung, nicht auf dem Code.
```

## L2658-2661 · `let r = match if self.vm_off { Err(super::code::Unsupported("off")) }`

```
// **Ganz oder gar nicht.** Was der Uebersetzer kann, faehrt die
// Befehlsmaschine; sagt er irgendwo nein, faehrt der Baumlaeufer das
// GANZE Programm. Eine Mischung waere ein zweiter Semantikpfad im
// selben Lauf, und solche laufen still auseinander.
```

## L2682-2683 · `super::promise::run_jobs(self);`

```
// Auch wenn das Programm geworfen hat: die Schlange gehoert geleert.
// Ein `.then`, das vor dem Fehler angelegt wurde, ist angemeldet.
```

## L2688-2694 · `pub fn perform_eval(&mut self, code: &Value, caller: Option<Rc<RefCell<Env>>>) -> C<Value> {`

```
/// `eval`. Der Unterschied zwischen DIREKT und indirekt ist der Bereich:
/// `eval(s)` als blosser Aufruf laeuft im Bereich des Rufers und sieht
/// dessen Namen, `(0,eval)(s)` laeuft global.
///
/// **Kein strenger Modus.** Die Engine kennt ihn zur Laufzeit nicht
/// (siehe [[project-beak-js-language-gap]]), also legt auch ein
/// `"use strict"` im Quelltext keinen eigenen Bereich fuer `var` an.
```

## L2696-2697 · `let Value::Str(src) = code else { return Ok(code.clone()) };`

```
// Alles, was keine Zeichenkette ist, kommt unveraendert zurueck —
// `eval(42)` ist 42, kein Programm.
```

## L2699-2701 · `self.depth += 1;`

```
// Ein `eval` legt einen RUST-Rahmen an (Parser + eigene Maschine) und
// laeuft sonst ohne Grenze im Kreis: `eval("eval('…')")`. Also zaehlt
// er wie ein Aufruf mit.
```

## L2717-2718 · `let inherited = caller.as_ref().is_some_and(|e| e.borrow().strict);`

```
// Ein DIREKTES eval erbt die Strenge seines Rufers; die eigene
// Direktive legt drauf. Ein indirektes faengt locker an.
```

## L2722-2724 · `let scope = Env::new(Some(base.clone()), false);`

```
// `let`/`const` bekommen einen EIGENEN Bereich, `var` und
// Funktionsdeklarationen steigen bis zur naechsten Funktionsgrenze
// des RUFERS — genau deshalb ist der Bereich hier kein Funktionsbereich.
```

## L2727-2730 · `let var_env = if strict {`

```
// **Strenger eval behaelt sein `var` bei sich.** Sonst steigt es bis
// zur Funktionsgrenze des Rufers und legt dort einen Namen an, den
// der Rufer nie geschrieben hat — genau der Unterschied, an dem
// `eval` in 0.92.0 fuenf Tests gekostet hat.
```

## L2745-2746 · `match if self.vm_off { Err(super::code::Unsupported("off")) }`

```
// Dieselbe Wahl wie beim Programm: kann der Uebersetzer alles, faehrt
// die Maschine; sonst der Baumlaeufer. Nie eine Mischung.
```

## L2766-2768 · `pub fn is_eval_fn(&self, v: &Value) -> bool {`

```
/// Ist dieser Wert die eingebaute `eval`? Nur dann ist ein Aufruf
/// `eval(...)` ein DIREKTER — jede andere Funktion unter dem Namen ist
/// ein gewoehnlicher Aufruf.
```

## L2780-2785 · `fn hoist(&mut self, body: &[Stmt], block: &Rc<RefCell<Env>>, func: &Rc<RefCell<Env>>) -> C<()> {`

```
/// `var` und Funktionsdeklarationen nach vorn ziehen.
///
/// `var` steigt bis zur naechsten FUNKTIONSGRENZE, `let`/`const`/`class`
/// bleiben im Block und stehen bis zur Deklaration auf „nicht bereit" —
/// das ist die zeitliche Totzone, und ohne sie ist `let` nur ein `var`
/// mit anderem Namen.
```

## L2789-2792 · `let st = super::modules::unexport(st).unwrap_or(st);`

```
// `export function f(){}` ist eine Deklaration mit einem Wort
// davor — sie wird genauso hochgezogen. Ohne diese Zeile stuende
// eine exportierte Funktion erst da, wenn ihre Zeile lief, und
// ein Zyklus im Modulgraphen saehe sie nie.
```

## L2798-2801 · `if Rc::ptr_eq(block, &self.realm.global_env) {`

```
// Dieselbe Regel wie fuer `var`: eine
// Funktionsdeklaration auf oberster Ebene eines
// Skripts IST eine Eigenschaft des globalen Objekts.
// In einem Block oder einer Funktion nicht.
```

## L2823-2827 · `Stmt::ExportDefault(d) => {`

```
// `export default` legt seinen Wert unter einem Namen ab,
// den kein Skript schreiben kann. Er wird HIER angelegt,
// damit ein Zyklus, der ihn zu frueh liest, „nicht bereit"
// sagt statt „gibt es nicht" — und damit eine
// Funktionsdeklaration auch dahinter hochgezogen wird.
```

## L2854-2855 · `fn hoist_vars(&mut self, st: &Stmt, func: &Rc<RefCell<Env>>) {`

```
/// `var` durch Bloecke und Schleifen hindurch einsammeln — aber NICHT
/// durch Funktionen: dort faengt ein neuer Bereich an.
```

## L2858-2873 · `let gobj = Rc::ptr_eq(func, &self.realm.global_env)`

```
// **Ein `var` auf oberster Ebene eines SKRIPTS ist eine Eigenschaft
// des globalen Objekts** (ES §CreateGlobalVarBinding) — und zwar die
// einzige Ablage dafuer, nicht eine zweite neben der Bindungskette
// ([[feedback_a_copy_is_a_second_semantics_waiting]]). Lesen und
// Schreiben finden sie: `assign_ident_depth` und die Namensaufloesung
// fallen beide auf das globale Objekt zurueck, wenn die Kette den
// Namen nicht hat.
//
// Ohne diese Zeile ist `window.X` fuer ein `var X` **undefined** — und
// genau so exponiert JEDES UMD-Buendel sich selbst. Auf arcade.ch
// starb daran der Einwilligungsbanner: `window.cookieconsent
// .openPreferencesCenter = …` auf einem Wert, den es nicht gab.
// `let`/`const` gehoeren NICHT dorthin (sie stehen im deklarativen
// Teil), und ein MODUL hat seine eigene Umgebung — beides faellt hier
// schon dadurch heraus, dass nur `var` und nur die globale Umgebung
// diesen Weg nehmen.
```

## L2879-2880 · `if g.borrow().get_own(n.as_str()).is_none() {`

```
// Schon da? Dann steht dort ein Wert, den ein frueheres
// `var` oder das Wirtsobjekt gesetzt hat — nicht ueberschreiben.
```

## L2882-2883 · `g.borrow_mut().define(n.as_str(), Prop {`

```
// Nicht loeschbar (ES: D = false fuer ein Skript-`var`),
// aber aufzaehlbar und schreibbar wie jede Seiteneigenschaft.
```

## L2944 · `pub fn to_prop_desc(&mut self, d: &Value) -> C<Desc> {`

```
// ── Der Iteratorvertrag ──────────────────────────────────────────────
```

## L2946-2961 · `pub fn to_prop_desc(&mut self, d: &Value) -> C<Desc> {`

```
/// `{ value, done }` — das Ergebnis eines `next()`.
/// Ein Eigenschaftsbeschreiber (`{value, writable, …}`) → `Prop`.
///
/// Eigene Funktion, weil sie fuenf Rufer hat: `Object.defineProperty`,
/// `Object.defineProperties`, `Object.create` mit zweitem Argument und
/// `Reflect.defineProperty`. Fuenf Kopien dieser Regeln waeren fuenf
/// Gelegenheiten, sie auseinanderlaufen zu lassen.
/// `ToPropertyDescriptor` (ES §6.2.6.5).
///
/// **Gefragt wird nach ANWESENHEIT, nicht nach Wahrheit.** Die alte
/// Fassung las `writable` mit `.truthy()`, und ein fehlendes Feld wurde
/// damit zu `false` — beim ANLEGEN richtig, beim AENDERN falsch. Der
/// Unterschied ist der ganze Sinn eines partiellen Beschreibers.
///
/// Gefragt wird ueber die PROTOTYPKETTE (`HasProperty`, nicht `has_own`):
/// ein Beschreiber, der `writable` erbt, zaehlt.
```

## L2999 · `pub fn from_prop_desc(&mut self, p: &Prop) -> Value {`

```
/// Und zurueck: `Prop` → Beschreiberobjekt.
```

## L3017-3018 · `pub fn define_props_from(&mut self, target: &Gc, props: &Value) -> C<()> {`

```
/// Die Eigenschaften aus einem `{k: beschreiber, …}` auf ein Objekt
/// legen — `Object.defineProperties` und `Object.create(p, props)`.
```

## L3026-3028 · `let mut pending = Vec::new();`

```
// ERST alle Beschreiber lesen, DANN alle anwenden (ES §20.1.2.3.1).
// Die Reihenfolge ist beobachtbar: wirft der dritte Beschreiber,
// duerfen die ersten beiden nicht schon gelegt sein.
```

## L3040-3047 · `pub fn new_buffer(&mut self, n: usize) -> Value {`

```
/// Ein frischer `ArrayBuffer` mit `n` Nullbytes.
///
/// **Ueber `MAX_BUFFER_BYTES` gibt es einen leeren Puffer** — die Rufer
/// pruefen vorher und werfen einen `RangeError`, so wie ein echter Motor
/// es tut, wenn die Zuteilung scheitert. Ohne die Schranke hat ein
/// test262-Fall 7 Petabyte angefordert und den Lauf mit SIGABRT beendet;
/// in einem Kernel waere das kein Absturz des Testlaeufers, sondern der
/// des Systems.
```

## L3057 · `pub fn new_view(&mut self, kind: ElemKind, buf: Gc, offset: usize, len: usize) -> Value {`

```
/// Eine SICHT auf einen bestehenden Puffer — kein neuer Speicher.
```

## L3065 · `pub fn new_typed(&mut self, kind: ElemKind, len: usize) -> Value {`

```
/// Eine Sicht MIT eigenem Puffer — der gewoehnliche `new Uint8Array(n)`.
```

## L3083-3084 · `pub fn array_iter(&mut self, target: Value, kind: u8) -> C<Value> {`

```
/// Ein Array-Iterator ueber `target`. `kind`: 0 Werte, 1 Schluessel,
/// 2 Paare.
```

## L3086 · `let t = self.to_object(&target)?;`

```
// `ToObject` zuerst: `Array.prototype.values.call("ab")` muss gehen.
```

## L3098-3099 · `pub fn get_iterator(&mut self, v: &Value) -> C<Value> {`

```
/// `GetIterator`. Wirft, wenn der Wert keinen `Symbol.iterator` hat —
/// genau das verlangt `for..of`, und der Fehlertext nennt den Grund.
```

## L3113-3114 · `pub fn has_prop(&mut self, o: &Gc, key: &str) -> C<bool> {`

```
/// Wie `has_property`, aber ein Wurf aus einer Stellvertreterfalle kommt
/// durch. `in` und `Reflect.has` rufen diese.
```

## L3129 · `pub fn own_keys_of(&mut self, o: &Gc) -> C<Vec<PropName>> {`

```
/// Die EIGENEN Schluessel — durch einen Stellvertreter hindurch.
```

## L3146 · `pub fn get_own_desc(&mut self, o: &Gc, key: &str) -> C<Option<Prop>> {`

```
/// Der EIGENE Beschreiber — durch einen Stellvertreter hindurch.
```

## L3157-3159 · `Ok(Some(self.to_prop_desc(&r)?.into_new_prop()))`

```
// Die Falle liefert einen partiellen Beschreiber; eine
// ABGELEGTE Eigenschaft ist immer vollstaendig, also
// bekommen die fehlenden Felder hier ihre Vorgabewerte.
```

## L3168-3174 · `pub fn define_own(&mut self, o: &Gc, key: &str, d: Desc) -> C<bool> {`

```
/// Eine Eigenschaft festlegen — durch einen Stellvertreter hindurch.
/// `[[DefineOwnProperty]]` — MIT der Pruefung (ES §10.1.6).
///
/// Bis 0.99.0 legte diese Funktion einfach ab, was man ihr gab. Eine
/// nicht konfigurierbare Eigenschaft liess sich damit umdefinieren,
/// `Object.freeze` war eine Bitte, und `Object.defineProperty` gab immer
/// `true`. 368 test262-Varianten haengen daran.
```

## L3190-3191 · `if !extensible { return Ok(false); }`

```
// Neu. Nur die Erweiterbarkeit steht dem im Weg; die fehlenden
// Felder bekommen HIER ihre Vorgabewerte.
```

## L3199-3200 · `if !cur.configurable {`

```
// `ValidateAndApplyPropertyDescriptor`, Schritt 4: was eine nicht
// konfigurierbare Eigenschaft NICHT erlaubt.
```

## L3221 · `let mut np = cur.clone();`

```
// Angewendet wird FELDWEISE: was der Beschreiber nicht nennt, bleibt.
```

## L3224-3225 · `np = Prop { value: None, get: None, set: None, writable: false,`

```
// Art gewechselt — die Felder der alten Art fallen auf ihre
// Vorgabewerte zurueck, `enumerable`/`configurable` bleiben.
```

## L3240-3241 · `pub fn desc_to_object(&mut self, d: &Desc) -> Value {`

```
/// `FromPropertyDescriptor` fuer einen PARTIELLEN Beschreiber — was die
/// Stellvertreter-Falle zu sehen bekommt. Nur die Felder, die dastehen.
```

## L3256 · `pub fn define_or_throw(&mut self, o: &Gc, key: &str, d: Desc) -> C<()> {`

```
/// `DefinePropertyOrThrow` (ES §7.3.8) — was die Eingebauten benutzen.
```

## L3262 · `pub fn proto_of(&mut self, o: &Gc) -> C<Option<Gc>> {`

```
/// Der Prototyp — durch einen Stellvertreter hindurch.
```

## L3276-3286 · `pub fn get_async_iterator(&mut self, v: &Value) -> C<(Value, bool)> {`

```
/// Ein Schritt. `None` heisst fertig.
/// `GetIterator(obj, async)` (ES 7.4.2). Gibt den Iterator und die
/// Auskunft, ob er ein ECHTER async-Iterator ist.
///
/// **Ohne `Symbol.asyncIterator` wird der synchrone genommen.** Die
/// Spezifikation huellt ihn dafuer in einen `%AsyncFromSyncIterator%`; wir
/// merken uns stattdessen `is_async = false` am Iterator und warten in
/// `Op::IterStepAsync` nur seinen `value` ab. Das ist dieselbe
/// beobachtbare Semantik ohne ein Objekt, das kein Skript je zu sehen
/// bekommt — was fehlt, ist allein `%AsyncFromSyncIteratorPrototype%` als
/// benannte Schnittstelle, und die zaehlt der Zensus nicht.
```

## L3318-3323 · `pub fn iter_close(&mut self, it: &Value) {`

```
/// `IteratorClose` — beim vorzeitigen Verlassen (`break`, `return`, ein
/// Fehler im Rumpf). Ein Generator raeumt hier auf; wer das auslaesst,
/// laesst `finally`-Bloecke in fremdem Code liegen.
///
/// Ein Fehler AUS `return()` wird geschluckt: der Grund fuers Verlassen
/// steht schon fest, und ihn zu ueberschreiben verbirgt ihn.
```

## L3330-3336 · `pub fn iterate(&mut self, v: &Value) -> C<Vec<Value>> {`

```
/// Alles auf einmal — fuer Streuung, `Array.from`, `new Map(…)`.
///
/// Eifrig, und das ist hier richtig: alle drei Aufrufer BRAUCHEN die
/// vollstaendige Liste. `for..of` laeuft nicht hier durch, sondern
/// schrittweise ([[exec_for_of]]) — sonst haenge ein unendlicher
/// Iterator die Seite auf, obwohl der Rumpf im ersten Durchlauf
/// abbricht.
```

## L3350-3363 · `pub fn for_in_keys(&mut self, v: &Value) -> C<Vec<Rc<str>>> {`

```
/// Die Schluessel, ueber die ein `for..in` laeuft: aufzaehlbar, die ganze
/// Prototypenkette hoch, ohne Doppelte.
///
/// **Die Liste wird VORHER gebaut.** Eine Aenderung am Objekt waehrend der
/// Schleife darf sie nicht ins Rutschen bringen — das ist der Unterschied
/// zu `for..of`, das faul sein MUSS. Ein Schluessel, der zwischendurch
/// verschwindet, wird trotzdem uebersprungen: das macht `get` von selbst.
///
/// `undefined`/`null` geben eine LEERE Liste, keinen Fehler: `for (k in
/// null)` laeuft null Mal, statt zu werfen.
///
/// Eigene Funktion, weil die Befehlsmaschine sie braucht. Sie dort
/// nachzubauen waere eine zweite Aufzaehlungsreihenfolge, und die faellt
/// erst auf, wenn ein Skript sich darauf verlaesst.
```

## L3387-3394 · `pub fn elems(&mut self, v: &Value) -> C<Vec<Value>> {`

```
/// `CreateListFromArrayLike`: `length` und Indizes, OHNE den
/// Iteratorvertrag.
///
/// Das ist kein Rueckfall, sondern eine eigene Spezifikationsoperation.
/// `Function.prototype.apply` und die Array-Methoden benutzen sie —
/// `apply` mit einem Objekt ohne `Symbol.iterator` muss gehen, `for..of`
/// damit muss werfen. Wer beides in eine Funktion legt, verliert genau
/// diesen Unterschied.
```

## L3415-3431 · `pub fn set_literal_proto(&mut self, o: &Gc, v: &Value) {`

```
/// Ein Feld MIT LOECHERN: nur die genannten Plaetze werden belegt, die
/// Laenge steht trotzdem fest.
///
/// Ein Loch ist nicht `undefined`. `new Array(3).concat("x").map(f)` ruft
/// `f` genau EINMAL — d3 baut damit seine Farbtabellen, und ein Loch, das
/// als Wert durchgereicht wird, kommt dort als `undefined.length` an.
/// `{ __proto__: v }` im Objektliteral SETZT den Prototyp (ES 13.2.5.5,
/// PropertyDefinitionEvaluation) — es legt KEINE Eigenschaft an.
///
/// Der Unterschied ist nicht akademisch: chart.js beginnt mit
/// `Object.freeze({__proto__:null, get Colors(){…}, …})` und laeuft
/// danach ueber die eigenen Schluessel dieses Namensraums. Als
/// Eigenschaft gelesen steht `__proto__` mit dem Wert `null` mit in der
/// Liste — und der naechste Zugriff ist `null.prototype`.
///
/// Nur ein Objekt oder `null` wirken; alles andere wird still verworfen,
/// so wie es die Spezifikation sagt.
```

## L3473-3474 · `pub fn make_method(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>,`

```
/// Dasselbe, aber mit Heimatobjekt — das ist der einzige Unterschied
/// zwischen einer Funktion und einer Methode, und `super` haengt daran.
```

## L3477-3479 · `let fproto = if f.is_generator {`

```
// Eine Generatorfunktion haengt unter `%GeneratorFunction.prototype%`,
// nicht unter `Function.prototype` — daran haengen `f.constructor` und
// der `toStringTag`, und beides wird gemessen.
```

## L3499-3505 · `if !f.is_arrow && !(f.is_async && !f.is_generator) {`

```
// Ein Pfeil hat kein `prototype` — er kann nicht als Konstruktor
// dienen, und ein vorhandenes `prototype` waere ein sichtbarer
// Unterschied zu jedem echten Motor.
// Eine async-Funktion hat KEIN `prototype` — sie ist kein Konstruktor,
// und ein vorhandenes waere ein sichtbarer Unterschied zu jedem echten
// Motor. Ein async-GENERATOR hat eins, obwohl auch er keiner ist: von
// dort erbt sein Objekt `next`/`return`/`throw`.
```

## L3507-3510 · `let is_gen = f.is_generator;`

```
// Das `prototype` einer Generatorfunktion haengt unter
// `%GeneratorPrototype%` und traegt KEIN `constructor` — von dort
// erbt das Generatorobjekt `next`/`return`/`throw`. Eine
// gewoehnliche Funktion bekommt das gewohnte Paar.
```

## L3532 · `pub fn ta_of(o: &Gc) -> Option<Rc<TaData>> {`

```
/// Die Sicht hinter einem Objekt, wenn es eine ist.
```

## L3540-3542 · `pub fn ta_read(o: &Gc, key: &str) -> Option<Value> {`

```
/// Ein Element einer Sicht lesen. `None` heisst „das ist keine Sicht oder
/// kein Index" — dann laeuft der gewoehnliche Weg weiter. `Some(Undefined)`
/// heisst „Sicht, aber ausserhalb", und das ist eine ANTWORT, kein Durchfall.
```

