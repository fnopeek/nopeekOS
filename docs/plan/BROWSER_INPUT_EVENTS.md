# Die BEDIEN-Haelfte: Ereignisse, die der Wirt nie zustellt

Stand 2026-09-17 · beak 0.185.0. Alle Zahlen gemessen; wo etwas geschlossen
und nicht gemessen ist, steht es dabei.

## Der Befund

**Der Wirt stellt der Seite genau VIER Ereignisse zu:**

```
load · DOMContentLoaded · click · submit
```

Ausgezaehlt in `tools/wasm/beak/src/lib.rs` — mehr `dispatch`-Rufstellen gibt
es nicht. `edit_key` schreibt jeden Tastendruck in `FormState` **und in den
Baumknoten**, und sagt es der Seite nie.

**Und die Engine kennt nur die Basis `Event`.** Kein `UIEvent`,
`KeyboardEvent`, `InputEvent`, `MouseEvent`, `FocusEvent` — null Treffer in
`dombind.rs`. Ein Behandler, der `e.key` oder `e.clientX` liest, bekommt
`undefined`; ein `e instanceof KeyboardEvent` wirft.

Der sichtbare Fall ist DuckDuckGos Vorschlagsliste: tippt man „stansstad",
zeigt ein Browser acht Vorschlaege (`stansstad badi`, `plz`, `kanton`, …).
In beak passiert nichts — React haengt an `onChange`, also an `input`, und
das kommt nie.

## Der Zensus — welche Ereignisse der Korpus WIRKLICH anmeldet

`addEventListener` (und die `onfoo=`-Form) vor dem ersten Seitenskript
umgehaengt und je Typ gezaehlt, ueber die zwoelf Korpusseiten aus
`<tools>/jsscope/corpus.txt` (Chromium, echtes UA). Werkzeug:
`<scratchpad>/evcensus.py` + `cdp.py`.

**Nach Seitenverbreitung, weil die eine Zahl ist, die nicht luegt**
([[feedback_a_call_count_is_not_a_site_count]]):

| Ereignis | Seiten | Anmeldungen | beak |
|---|---:|---:|---|
| `click` | **12** | 4408 | ✅ |
| `keydown` | **11** | 566 | ❌ |
| `change` | **10** | 638 | (nur am Kaestchen) |
| `resize` | **10** | 220 | ❌ |
| `focus` | **10** | 87 | ❌ |
| `keypress` | 9 | 389 | ❌ (veraltet) |
| `scroll` | 9 | 339 | ❌ |
| `visibilitychange` | 9 | 162 | ❌ |
| `pageshow` | 9 | 132 | ❌ |
| `blur` | 9 | 74 | ❌ |
| `load` | 8 | 730 | ✅ |
| `mouseover` | 8 | 704 | ❌ |
| `mousedown` | 8 | 592 | ❌ |
| `keyup` | 8 | 442 | ❌ |
| `focusin` | 8 | 416 | ❌ |
| **`input`** | **8** | **403** | ❌ |
| `submit` | 6 | 396 | ✅ |

**Von den zehn verbreitetsten Ereignissen des Webs stellen wir genau EINES
zu.** Dazu 177 EIGENE Namen (`soft-nav:external:start`, `srf.track.interaction`
…) — das ist der Bibliotheks-Bus einer Seite mit sich selbst, kein
Wirtsereignis; `dispatchEvent` bedient ihn bereits.

**Warnung an die hinteren Zahlen.** `touchstart`/`pointerdown`/`dragstart`/
`composition*` stehen bei 5–7 Seiten mit je ~385 Anmeldungen — das ist EINE
Bibliothek, die eine ganze Matrix auf einmal anmeldet, und nicht sieben
Seiten, die Touch brauchen. Die Spalte „Seiten" traegt, die Spalte
„Anmeldungen" traegt hier nicht.

## Was das mit dem Relayout-Posten zu tun hat

`docs/plan/BROWSER_RELAYOUT_ON_DEMAND.md` hat gemessen: Kastenlesen ist
**0,4 %** der Aufrufe beim Laden und **4,7 %** beim Bedienen. Wir haben den
Haken fuer Messungen gebaut, die es bei uns fast nur beim Laden gibt — weil es
kein Bedienen gibt. **Die beiden Posten gehoeren zusammen**, und dieser hier
ist der, der den anderen einloest.

## Stand: S1–S5 sind gebaut (beak 0.186.0, 2026-09-17)

* **S1 — die Arten.** `UIEvent` → `KeyboardEvent` / `InputEvent` /
  `FocusEvent` / `MouseEvent`, echte Konstruktoren, echte Kette, echter
  `Symbol.toStringTag`. Gemessen im Selbsttest:
  `kd:x/KeyX/88/88/shift/kbd/ui/ev/mod` — `key`, `code`, `keyCode`, `which`,
  die Umschalter, `getModifierState`, alle drei `instanceof` und
  `preventDefault` auf einem abbrechbaren Ereignis.
* **S2 — die Tastatur.** `edit_key` faehrt jetzt
  `keydown` → (Abbruch? Ende) → Wert → `input` → `keyup`. Auf einer Vorlage
  gemessen: getippt „axb" in ein Feld, dessen `keydown` bei `x` abbricht →
  im Feld steht `ab`. **Der Abbruch traegt.**
* **S3 — der Fokus.** Ein Weg (`set_focus`) statt sieben Zuweisungen, mit
  `blur`/`focusout` und `focus`/`focusin` — und `change` faellt beim
  VERLASSEN, nur wenn sich der Wert seit dem Fokussieren geaendert hat.
* **An DDGs echtem Suchfeld gemessen** (eingefrorene Startseite, `pagerun
  TYPE=qbox=st`): die Ereignisse kommen an den Knoten, blasen bis `document`
  und tragen den wachsenden Wert (`data=s val=s`, `data=t val=st`).
  **Nicht bewiesen:** dass DDGs eigener Behandler daraufhin die Vorschlagsliste
  holt — auf dem Spiegel gibt es die `/ac/`-Antwort nicht, und der Lauf zeigt
  keine `fetch`-Zeile. Das sagt der Geraetelauf.
* **S4 — die Maus.** Der Klick, den der Wirt schon zustellte, ist jetzt ein
  echtes `MouseEvent` mit `clientX/Y` (fensterbezogen) und `pageX/Y`
  (dokumentbezogen), `button`, `detail`. Dazu `mousedown` vor dem Klick und
  `mouseup` beim Loslassen — und `mouseup` faellt UNABHAENGIG davon, ob ein
  Link haengt; ein Schieberegler liegt auf keinem `<a>`. Selbsttestzeile
  `mausev`.
* **S5 — Fenster und Rollen.** `scroll` und `resize` nach dem Bild, mit
  eigenem Gedaechtnis (`told_scroll`/`told_vp`): ohne das faellt das Ereignis
  je Bild oder nie. `pageshow` einmal nach `load`. **`visibilitychange` wird
  NICHT gebaut** — `document.visibilityState` ist bei uns konstant `"visible"`
  (das gibt es seit je), also aendert sich nichts, und ein Ereignis dafuer
  waere erfunden.
* **Offen: `mouseover`/`mouseout`.** Die Zeigerspur gibt es (`HoverChange`),
  aber sie ist der teuerste Pfad im Wirt — das gehoert gemessen, bevor dort
  ein Skriptlauf je Bewegung dazukommt.

## Der Bauplan

Die Maschinerie steht groesstenteils: `build_event`, `deliver` (Fang-, Ziel-
und Blasenphase), `dispatch_plain`, `dispatch_seq`. `dispatch_plain(i,
"input", …)` wird sogar schon benutzt — beim Klick auf ein `<label>` eines
Kaestchens. Was fehlt, ist der WEG von der Tastatur dorthin und die
Ereignis-ARTEN.

### S1 — die Ereignis-Schnittstellen

`UIEvent` → `KeyboardEvent` / `MouseEvent` / `FocusEvent`, und `InputEvent`
unter `UIEvent`. Konstruktoren echt (wie `EventTarget` seit 0.178.0), Kette
echt, damit `instanceof` traegt. Felder, die Seiten wirklich lesen:

* `KeyboardEvent`: `key`, `code`, `keyCode`/`charCode`/`which` (veraltet und
  ueberall gelesen), `altKey`/`ctrlKey`/`shiftKey`/`metaKey`, `repeat`,
  `isComposing`, `location`, `getModifierState()`
* `InputEvent`: `data`, `inputType`, `isComposing`
* `FocusEvent`: `relatedTarget`
* `MouseEvent`: `clientX/Y`, `pageX/Y`, `screenX/Y`, `offsetX/Y`, `button`,
  `buttons`, die vier Umschalter, `relatedTarget`
* `UIEvent`: `detail`, `view`

### S2 — die Tastatur (der Posten, der die Vorschlagsliste aufmacht)

`edit_key` hat Knoten und Wert schon in der Hand. Die Reihenfolge ist die der
Spezifikation (UI Events §5.4), und sie ist der ganze Vertrag:

```
keydown  → abgebrochen? dann NICHTS weiter (kein Zeichen, kein input)
         → beforeinput → Wert aendern → input
keyup
```

**`preventDefault()` auf `keydown` muss den Tastendruck verschlucken** — das
ist die Art, wie jedes Eingabefeld der Welt Zeichen filtert; ohne das ist die
Zustellung schlimmer als keine, weil die Seite glaubt, sie haette verhindert.

`change` gehoert NICHT hierher: bei einem Textfeld faellt es beim Verlassen
(Commit), nicht je Zeichen.

### S3 — Fokus

`focus`/`blur` (blasen nicht) und `focusin`/`focusout` (blasen). Hier faellt
auch `change` an. `page.state.focus` gibt es schon; es fehlt die Meldung.

### S4 — die Maus

Erst der Klick, den wir SCHON zustellen, als echtes `MouseEvent` mit
Koordinaten — heute ist es eine nackte `Event`, und `e.clientX` ist
`undefined`. Danach `mousedown`/`mouseup` und `mouseover`/`mouseout`
(die Zeigerspur gibt es, `HoverChange`).

### S5 — die billigen mit hoher Verbreitung

`scroll` (9 Seiten; der Rollstand faehrt je Bild ohnehin mit),
`resize` (10), `visibilitychange` + `pageshow` (9). Jedes davon ist eine
Meldung an einer Stelle, die es schon gibt.

### Was NICHT gebaut wird — benannt, nicht stillschweigend uebergangen

`touch*` (kein Beruehrungsgeraet), `pointer*` (dasselbe, plus eine eigene
Zeiger-ID-Verwaltung), `drag*`/`drop` (eigener Datentransfer-Vertrag),
`composition*` (kein IME). Sie stehen im Zensus weit oben, aber ihre Zahl ist
eine Bibliotheksmatrix, und ohne die Geraeteklasse dahinter waere die
Zustellung eine Erfindung.

## Reihenfolge

1. **S1 + S2** — Schnittstellen und Tastatur. Das ist der Posten mit dem
   sichtbaren Ergebnis (`keydown` 11/12, `input` 8/12).
2. **S3** — Fokus, faellt fast mit ab, und bringt `change` (10/12).
3. **S4** — `MouseEvent` fuer den vorhandenen Klick zuerst; das ist eine
   Erweiterung, kein neuer Weg.
4. **S5** — `scroll`/`resize`/`visibilitychange`/`pageshow`.

## Tore

Jede Stufe braucht eine Zeile in `beak:selftest`, sonst sagt ein gruener Lauf
nur, dass nichts kaputt ist ([[feedback_the_test_path_must_be_the_real_path]]).
Fuer S2: in ein Feld tippen, und die Seite muss `keydown`, `beforeinput`,
`input`, `keyup` in dieser Reihenfolge mit dem richtigen `key` sehen — und ein
`preventDefault()` auf `keydown` muss das Zeichen verschlucken. Dazu die
bestehenden Tore unveraendert: 468 Tests, WPT, die vier Galerien, die zwoelf
Render-Hashes.

## Womit gemessen wurde

`<scratchpad>/cdp.py` — ein minimaler CDP-Treiber ueber rohe Sockets, weil
Chromium headless zwar Bilder machen, aber nicht TIPPEN kann und die
Vorschlagsliste erst bei Eingabe erscheint. `<scratchpad>/evcensus.py` haengt
`EventTarget.prototype.addEventListener` und die `onfoo=`-Setter um, und zwar
ueber `Page.addScriptToEvaluateOnNewDocument` — also VOR dem ersten
Seitenskript, sonst sind die fruehesten Anmeldungen schon durch.
