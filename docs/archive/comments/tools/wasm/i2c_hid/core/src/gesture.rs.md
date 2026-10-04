# `tools/wasm/i2c_hid/core/src/gesture.rs` @ 5e0102684

## L1-29 · `pub type Contact = (i32, i32, i32);`

```
//! Aus KONTAKTPUNKTEN wird ein Zeiger und eine Geste.
//!
//! Ein Touchpad meldet Orte, keine Wege, und es meldet keine Gesten: „zwei
//! Finger wandern nach unten" steht in keinem Bericht. Unter Linux macht
//! das libinput, hier dieses Modul — und es steht im Kern und nicht im
//! Wasm-Modul, weil genau diese Logik zweimal falsch ausgeliefert wurde
//! und beide Male erst am Geraet auffiel.
//!
//! Vier Dinge, die nicht offensichtlich sind:
//!
//! 1. **Ein Bild kann ueber mehrere Berichte kommen.** Ein Geraet mit einem
//!    Kontaktplatz schickt je Finger einen Bericht; `Contact Count` steht
//!    nur im ersten. Die Regel ist die von `hid-multitouch.c`: der Bericht,
//!    der eine Kontaktzahl TRAEGT, eroeffnet das Bild.
//! 2. **Die Geste haelt, bis abgehoben wird.** Haengt sie an der Fingerzahl
//!    DIESES Bildes, zappelt sie — faellt ein Folgebericht aus, sieht ein
//!    Bild einen Finger statt zwei. Wer bei jedem Wechsel den Bezugspunkt
//!    wegwirft, rechnet immer die Strecke null.
//! 3. **Wer die Taste durchdrueckt, bewegt den Finger.** Eine Fingerkuppe
//!    verformt sich unter dem Druck, ihr Schwerpunkt wandert — und ohne
//!    Gegenmittel geht diese Wanderung ungefiltert an den Zeiger, genau
//!    dann, wenn man etwas Kleines treffen will. Deshalb werden die Finger
//!    beim Tastendruck FESTGEHALTEN (libinput: `tp_pin_fingers`) und erst
//!    gelöst, wenn sie wirklich weit wandern.
//! 4. **Ein Antippen drueckt sofort und laesst SPAETER los.** Nur so kann
//!    daraus ein Ziehen werden: setzt der Finger im Fenster wieder auf, ist
//!    die Taste noch unten. Der Preis ist ein Zeitgeber, den der Rufer
//!    treten muss (`tick`) — ohne ihn bliebe die Taste gedrueckt, und
//!    genau das war der Fehler von 0.13.0.
```

## L31 · `pub type Contact = (i32, i32, i32);`

```
/// Ein aufliegender Finger: Kennung, Ort.
```

## L34 · `#[derive(Debug, PartialEq, Eq)]`

```
/// Was ein Bericht ergeben hat.
```

## L37 · `Pending,`

```
/// Das Bild ist noch nicht vollstaendig — es fehlen Finger.
```

## L39 · `Frame {`

```
/// Ein vollstaendiges Bild.
```

## L41 · `n: usize,`

```
/// Aufliegende Finger in diesem Bild.
```

## L43 · `gesture: usize,`

```
/// Die Geste, die seit dem Aufsetzen gilt.
```

## L47 · `scroll: i32,`

```
/// Rollrasten, Vorzeichen wie ein Mausrad (positiv = nach oben).
```

## L49-54 · `hscroll: i32,`

```
/// Waagrechte Rollrasten, positiv = nach RECHTS (wie REL_HWHEEL).
///
/// Eigene Achse mit eigenem Speicher und eigenem Schritt: das Pad
/// ist breiter als hoch, ein Schritt aus der Hoehe waere quer zu
/// fein. Beide Achsen laufen unabhaengig, wie bei libinput —
/// eine Achssperre wuerde einen schraegen Wisch halbieren.
```

## L56-61 · `tap: u8,`

```
/// Ein Antippen, das fertig ist: Druck UND Loslassen in einem.
/// 0 = keines, 2 = rechts.
///
/// Nur fuer das, was nicht ziehen kann. Ein Antippen mit EINEM
/// Finger laeuft ueber [`Tracker::hold`], weil daraus noch ein
/// Ziehen werden kann; ein Rechtsklick zieht nichts.
```

## L66-70 · `const TAP_MS: u64 = 180;`

```
/// Ein Antippen dauert hoechstens so lange. libinput nimmt denselben
/// Wert; darueber ist es ein Halten und kein Tippen.
///
/// Dieselbe Spanne gilt danach noch einmal als FENSTER: wer darin wieder
/// aufsetzt, zieht (oder tippt ein zweites Mal), statt neu anzufangen.
```

## L73-79 · `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`

```
/// Wo die Tipp-Maschine gerade steht.
///
/// Die Zustaende von libinput (`evdev-mt-touchpad-tap.c`) auf das
/// reduziert, was ein Finger und zwei Finger brauchen. Der mittlere ist
/// der, um den es geht: nach einem Antippen ist noch NICHT entschieden,
/// ob daraus ein Ziehen oder ein zweites Antippen wird — das sagt erst,
/// was der Finger danach tut.
```

## L82 · `Idle,`

```
/// Nichts laeuft.
```

## L84-85 · `Held { since: u64 },`

```
/// Ein Antippen ist zu Ende, die Taste ist GEDRUECKT, das Fenster
/// laeuft ab `since`.
```

## L87 · `DragOrTap,`

```
/// Im Fenster hat ein Finger wieder aufgesetzt. Offen, was es wird.
```

## L89 · `Dragging,`

```
/// Es ist ein Ziehen: die Taste bleibt, bis abgehoben wird.
```

## L94-95 · `step: i32,`

```
/// Geraeteeinheiten je Rollraste, aus dem logischen Bereich des
/// Geraets hergeleitet.
```

## L97 · `hstep: i32,`

```
/// Dasselbe fuer die waagrechte Achse — aus der BREITE.
```

## L99 · `tap_move: i32,`

```
/// Soweit darf ein Finger wandern und es bleibt ein Tippen.
```

## L101-102 · `pin_move: i32,`

```
/// Soweit darf ein Finger unter einer GEDRUECKTEN Taste wandern,
/// bevor der Zeiger ihm wieder folgt.
```

## L104 · `down_ms: u64,`

```
/// Wann hat die laufende Beruehrung angefangen?
```

## L106-113 · `tap_dx: i32,`

```
/// Weg des VERFOLGTEN Fingers seit dem Aufsetzen.
///
/// Nicht der Abstand zu einem gemerkten Punkt: der Punkt gehoerte
/// `frame[0]`, und welcher Finger das ist, entscheidet das Geraet neu
/// in jedem Bericht. Tauschte es die Reihenfolge, sprang die Strecke
/// um den FINGERABSTAND und jedes Antippen mit zwei Fingern fiel aus.
/// Aufsummiert wird deshalb dieselbe Strecke, die auch der Zeiger
/// bekommt — und die ist beim Fingerwechsel null.
```

## L117-118 · `btn_seen: bool,`

```
/// Lag waehrend DIESER Beruehrung eine physische Taste an? Dann war
/// sie kein Antippen — sonst gaebe jeder Druck zwei Klicks.
```

## L121 · `pinned: bool,`

```
/// Die Finger sind festgehalten: ihr Weg geht nicht an den Zeiger.
```

## L174 · `pub fn frame(&self) -> &[Contact] {`

```
/// Die Finger des zuletzt vollstaendigen Bildes — fuers Log.
```

## L179-182 · `pub fn hold(&self) -> u8 {`

```
/// Die Taste, die ein Antippen gerade GEDRUECKT haelt. 0 = keine.
///
/// Ein PEGEL, keine Flanke: der Rufer legt ihn neben die physischen
/// Tasten und meldet die Lage, wenn sie sich aendert.
```

## L187-189 · `pub fn next_deadline(&self) -> Option<u64> {`

```
/// Wann `tick` wieder laufen muss — solange ein Antippen die Taste
/// haelt. None: kein Zeitgeber offen, der Treiber darf auf den naechsten
/// Bericht warten.
```

## L197-203 · `pub fn tick(&mut self, now_ms: u64) -> u8 {`

```
/// Den Zeitgeber weiterdrehen, auch wenn kein Bericht kam.
///
/// **Muss laufen, solange der Treiber laeuft.** Ein Antippen drueckt
/// sofort und wartet mit dem Loslassen auf das Fenster; kommt danach
/// nie wieder ein Bericht — und ein Touchpad, das niemand beruehrt,
/// schickt keinen —, bliebe die Taste ohne diesen Tritt fuer immer
/// unten.
```

## L209 · `fn expire(&mut self, now_ms: u64) {`

```
/// Das Fenster nach einem Antippen ablaufen lassen.
```

## L219-226 · `pub fn feed(`

```
/// Einen Bericht einspeisen.
///
/// `cc` ist die gemeldete Kontaktzahl (negativ: das Geraet fuehrt gar
/// keine), `present` sind die Plaetze dieses Berichts, deren Tip-Switch
/// gesetzt ist, `slots` ist die Zahl der Plaetze im Bericht —
/// gezaehlt wird gegen sie, nicht gegen die aufliegenden, sonst
/// endet ein Bild mit einem abgehobenen Finger nie — und `btn` sagt,
/// ob eine physische Taste dieses Berichts anliegt.
```

## L252-254 · `if btn {`

```
// Die Taste zuerst. Sie entscheidet zweierlei: dass die Finger
// festgehalten werden, und dass aus dieser Beruehrung kein
// Antippen mehr werden kann.
```

## L269-270 · `let quick = !self.moved`

```
// Alle Finger weg: erst JETZT endet die Geste — und erst hier
// steht fest, ob sie ein Antippen war.
```

## L278-280 · `self.hold = 0;`

```
// Zweimal kurz getippt: die gehaltene Taste geht auf,
// und der zweite Klick kommt ganz. Zusammen mit dem
// ersten sind das zwei — ein Doppelklick.
```

## L286 · `self.hold = 0;`

```
// Gezogen und losgelassen.
```

## L291-292 · `self.hold = 1;`

```
// Ein Antippen: DRUECKEN. Das Loslassen wartet auf das
// Fenster, sonst koennte daraus nie ein Ziehen werden.
```

## L297-298 · `tap = 2;`

```
// Zwei Finger: ein Rechtsklick zieht nichts, also
// Druck und Loslassen in einem Stueck.
```

## L314 · `self.down_ms = now_ms;`

```
// Aufsetzen: hier faengt ein moegliches Antippen an.
```

## L321-322 · `self.tap_state = Tap::DragOrTap;`

```
// Im Fenster wieder aufgesetzt. Die Taste bleibt unten;
// was es wird, sagen die naechsten Bilder.
```

## L330-332 · `let mut pick = 0usize;`

```
// Den Weg aus DEMSELBEN Finger rechnen: dem mit der kleinsten
// Kennung im Bild. Ohne das springt die Strecke um den
// Fingerabstand, sobald das Geraet die Reihenfolge tauscht.
```

## L343-344 · `(0, 0)`

```
// Erste Beruehrung oder anderer Finger: kein Weg, sonst
// spraenge der Zeiger dorthin, wo aufgesetzt wurde.
```

## L352-355 · `self.tap_dx += ddx;`

```
// Erst messen, dann festhalten: ob ein Finger gewandert ist,
// entscheidet die ECHTE Strecke. Wer das nach dem Festhalten
// fragt, bekommt immer null — und meldete nach jedem Tastendruck
// obendrein ein Antippen.
```

## L366-368 · `self.pinned = false;`

```
// Weit genug — der Zeiger folgt wieder. Die aufgelaufene
// Strecke bleibt liegen: sie NACHZUREICHEN waere ein
// Sprung um genau die Schwelle.
```

## L375 · `if self.tap_state == Tap::DragOrTap`

```
// Ein Finger, der liegen bleibt oder wandert, zieht.
```

## L393-395 · `Out::Frame {`

```
// Y waechst nach UNTEN, ein Rad zaehlt nach OBEN — deshalb
// dreht die senkrechte Achse ihr Vorzeichen. Quer nicht: X
// waechst nach rechts, und REL_HWHEEL zaehlt auch nach rechts.
```

## L413-414 · `fn tr() -> Tracker { Tracker::new(50, 50, 40, 80) }`

```
/// Ein Tracker mit den Massen, mit denen fast alle Tests rechnen:
/// Raste 50, Tippweg 40, Festhalteweg 80.
```

## L417-418 · `fn two_finger(t: &mut Tracker, a: Contact, b: Contact) -> Out {`

```
/// Ein Geraet mit EINEM Platz: zwei Finger sind zwei Berichte, und die
/// Kontaktzahl steht nur im ersten.
```

## L446 · `let out = two_finger(&mut t, (0, 100, 250), (1, 400, 260));`

```
// Beide Finger 50 Einheiten nach unten: genau eine Raste.
```

## L451-454 · `#[test]`

```
/// Der Fehler, an dem 0.17.0 gescheitert ist: faellt ein Bild einmal
/// auf EINEN Finger zurueck, darf daraus kein Zeigen werden — und vor
/// allem darf der Bezugspunkt nicht verloren gehen, sonst ist die
/// Strecke in JEDEM Bild null und es rollt nie.
```

## L459 · `let out = t.feed(1, &[(0, 100, 250)], 1, 0, false);`

```
// Zwischendurch sieht ein Bild nur einen Finger.
```

## L463 · `let out = two_finger(&mut t, (0, 100, 300), (1, 400, 310));`

```
// Und danach wieder zwei.
```

## L468-469 · `#[test]`

```
/// Ein verlorener Folgebericht darf den Treiber nicht fuer immer
/// warten lassen. Der naechste Bericht MIT Kontaktzahl eroeffnet.
```

## L474 · `assert_eq!(t.feed(2, &[(0, 100, 210)], 1, 0, false), Out::Pending);`

```
// Der zweite Bericht geht verloren; das naechste Bild faengt an.
```

## L483 · `assert_eq!(t.feed(1, &[(0, 100, 200)], 1, 0, false),`

```
// Aufsetzen: KEIN Sprung.
```

## L490-491 · `#[test]`

```
/// Abheben beendet die Geste — sonst rollte die naechste Beruehrung
/// mit EINEM Finger weiter.
```

## L496 · `assert_eq!(t.feed(0, &[], 1, 500, false),`

```
// Spaet genug abgehoben, dass es kein Antippen ist.
```

## L506-508 · `#[test]`

```
/// ANTIPPEN mit einem Finger: die Taste geht SOFORT runter und wartet
/// mit dem Loslassen auf das Fenster — sonst koennte daraus nie ein
/// Ziehen werden.
```

## L520-522 · `#[test]`

```
/// … und geht wieder auf, wenn das Fenster durch ist. **Ohne `tick`
/// bliebe sie fuer immer unten** — ein Touchpad, das niemand beruehrt,
/// schickt keinen Bericht mehr.
```

## L533-535 · `#[test]`

```
/// TIPPEN UND ZIEHEN: antippen, sofort wieder auflegen, wandern — die
/// Taste bleibt unten, bis abgehoben wird. Das ist der „sanfte Klick",
/// mit dem man ein Fenster verschiebt, ohne durchzudruecken.
```

## L542 · `t.feed(1, &[(0, 100, 200)], 1, 120, false);`

```
// Im Fenster wieder aufgesetzt und gewandert.
```

## L548 · `assert_eq!(t.tick(900), 1, "ein Ziehen laeuft nicht ab");`

```
// Der Zeitgeber darf das Ziehen NICHT beenden.
```

## L554-555 · `#[test]`

```
/// Und der Gegenfall: zweimal kurz getippt sind ZWEI Klicks, kein
/// Ziehen. Die gehaltene Taste geht auf, der zweite Klick kommt ganz.
```

## L569-570 · `#[test]`

```
/// Wer im Fenster wieder aufsetzt und LIEGEN bleibt, zieht auch —
/// ohne einen einzigen Punkt Bewegung.
```

## L577 · `t.feed(1, &[(0, 100, 200)], 1, 400, false);`

```
// Liegen bleiben, bis das Fenster des zweiten Kontakts durch ist.
```

## L594-597 · `#[test]`

```
/// Der Bug, den `frame[0]` versteckt hat: tauscht das Geraet zwischen
/// den Bildern die Reihenfolge der Finger, sprang der gemerkte
/// Startpunkt um den FINGERABSTAND — und aus dem Antippen mit zwei
/// Fingern wurde eine Bewegung. Damit fiel der Rechtsklick aus.
```

## L602 · `two_finger_at(&mut t, (1, 400, 900), (0, 100, 200), 40);`

```
// Dasselbe Bild, andere Reihenfolge, kein Finger hat sich bewegt.
```

## L609-610 · `#[test]`

```
/// Wer den Finger bewegt hat, wollte zeigen und nicht klicken — sonst
/// klickt jedes kurze Wischen.
```

## L621 · `#[test]`

```
/// Wer liegen bleibt, haelt — und ein Halten ist kein Tippen.
```

## L631 · `#[test]`

```
/// Eine Rollgeste endet nie als Klick.
```

## L641-644 · `#[test]`

```
/// FESTHALTEN. Wer das Pad durchdrueckt, verformt die Fingerkuppe —
/// ihr Schwerpunkt wandert, und ohne dieses Tor ginge die Wanderung
/// ungefiltert an den Zeiger. Genau dann, wenn man etwas Kleines
/// treffen will.
```

## L649 · `let out = t.feed(1, &[(0, 130, 220)], 1, 20, true);`

```
// Taste runter, und der Finger rutscht dabei 30 Einheiten.
```

## L655-656 · `#[test]`

```
/// Aber ein ZIEHEN mit gedrueckter Taste muss gehen: wandert der
/// Finger weit genug, folgt der Zeiger wieder.
```

## L662-663 · `let out = t.feed(1, &[(0, 100, 290)], 1, 40, true);`

```
// 90 > pin_move (80): das Tor geht auf, aber DIESES Bild bleibt
// still — die aufgelaufene Strecke nachzureichen waere ein Sprung.
```

## L671 · `#[test]`

```
/// Das Festhalten endet mit der Taste, nicht mit dem Finger.
```

## L682-684 · `#[test]`

```
/// Und der Fehler, den das Festhalten sonst einbaut: unter der Taste
/// misst der Zeiger null Weg — wer das Antippen DANACH fragt, haelt
/// jeden physischen Klick fuer ein Tippen und gibt ihn doppelt aus.
```

## L695-696 · `#[test]`

```
/// Zwei Finger nach RECHTS rollen nach rechts — und die senkrechte
/// Achse bleibt dabei still.
```

## L706-707 · `#[test]`

```
/// Ein schraeger Wisch traegt BEIDE Achsen. Eine Achssperre wuerde
/// ihn halbieren, und libinput sperrt beim Zweifinger-Rollen nicht.
```

## L717-718 · `#[test]`

```
/// Die quere Achse hat ihren EIGENEN Schritt: ein Pad ist breiter als
/// hoch, und ein Schritt aus der Hoehe waere quer zu fein.
```

## L723 · `let out = two_finger(&mut t, (0, 150, 200), (1, 450, 210));`

```
// 50 quer ist unter dem queren Schritt — noch keine Raste.
```

## L732-733 · `#[test]`

```
/// Tauscht das Geraet die Reihenfolge der Finger, darf die Strecke
/// nicht um den Fingerabstand springen.
```

## L738 · `let out = two_finger(&mut t, (1, 400, 950), (0, 100, 250));`

```
// Dasselbe Bild, andere Reihenfolge, beide 50 nach unten.
```

## L743-744 · `#[test]`

```
/// Teilstrecken laufen auf, bis sie eine Raste ergeben — sonst kaeme
/// bei jedem Bild eine und das Rollen waere unbrauchbar schnell.
```

## L759-760 · `#[test]`

```
/// Ein Geraet mit ZWEI Plaetzen im Bericht meldet beide Finger auf
/// einmal — dann gibt es keine Folgeberichte.
```

