# UI Refresh — Design-Kontrakt

Quelle: Claude-Design-Projekt `OS UI Refresh.dc.html`
(`claude.ai/design/p/25474c47-a7b7-49b7-ad97-85b582df39a1`).

Dieses Dokument ist die Übersetzung des Mockups in unser Vokabular. Das
Mockup ist CSS; hier steht, welches Token / welcher Modifier das im
`nopeek_widgets`-ABI ist. Apps lesen dieses File, nicht das HTML.

---

## 1. Farbrampen

Das Design hat eine neutral-kühle Graustufenrampe (statt der alten
blau-violetten) plus eine dritte Textstufe. Reihenfolge dunkel → hell:

| Design    | Token             | Dark      | Light     | Rolle                              |
|-----------|-------------------|-----------|-----------|------------------------------------|
| `page`    | `Page`            | `#131517` | `#ffffff` | Inhaltsfläche: Editor, Seite, Term |
| `surface` | `Surface`         | `#17191b` | `#faf9f7` | Fensterkörper                      |
| `chrome`  | `SurfaceElevated` | `#1c1f21` | `#f0f0ec` | Titel-/Menüband, Tabstrip, Panels  |
| `surface-2`| `SurfaceMuted`   | `#1e2124` | `#f1f0ec` | Eingesenkt: Sidebar, Feld, Tab-Bg  |
| `surface-3`| `SurfaceHover`   | `#262a2e` | `#e4e3de` | Hover-Füllung, Chips, Kbd-Badge    |
| `line`    | `Border`          | `#2c3034` | `#dedcd6` | 1px-Trennlinien + Rahmen           |
| `fg`      | `OnSurface`       | `#e6e8ea` | `#1b1d1f` | Primärtext                         |
| `fg-2`    | `OnSurfaceMuted`  | `#a2a8ae` | `#5c6166` | Sekundärtext, Menülabels           |
| `fg-3`    | `OnSurfaceFaint`  | `#6f767c` | `#8a9096` | Sektionsköpfe, Meta, disabled      |
| `ok`      | `Success`         | `#8fbf9f` | `#4d8a63` | Lock-Icon, Erfolg                  |

`chrome` und `chrome-2` des Mockups liegen 3 Stufen auseinander — das ist
unter der Wahrnehmungsschwelle. Beide sind `SurfaceElevated`.

## 1a. Code-Rampe (Syntax)

Die Rampe oben beschreibt **Chrome**. Quelltext braucht eine zweite,
unabhängige — ein Editor zeigt beides gleichzeitig, und solange
Schlüsselwörter auf `Accent` lagen, hing die Syntaxfarbe am Wallpaper und
eine ganze Sprache kam mit drei Farben aus.

Neun Tokens, angehängt ab 17 (ABI: nur anhängen, Werte eingefroren):

| Token          | Wert | Rolle                                              |
|----------------|------|----------------------------------------------------|
| `CodeKeyword`  | 17   | Deklaration/Storage: `fn` `let` `def` `class` `int` |
| `CodeControl`  | 18   | Fluss + Import: `if` `for` `return` `import`        |
| `CodeString`   | 19   | String- und Zeichenliterale, Anführungszeichen mit  |
| `CodeComment`  | 20   | Kommentare                                          |
| `CodeNumber`   | 21   | Zahlliterale                                        |
| `CodeFunction` | 22   | Funktionsnamen, Deklaration und Aufruf              |
| `CodeType`     | 23   | Typ-/Klassennamen, Markup-Tagnamen                  |
| `CodeVariable` | 24   | Attributnamen, JSON-Schlüssel, Dekoratoren          |
| `CodeConstant` | 25   | `true` `None` `null`, Entities, ALLCAPS-Namen       |

Aufgelöst wird **nicht** über den Akzent, sondern über ein benanntes
Schema: `set code.scheme <name>`. Die Werte stammen 1:1 aus VSCodiums
eingebauten Themes, aufgelöst wie TextMate auflöst (spezifischster Scope
gewinnt):

`auto` (Vorgabe, folgt dem Theme) · `dark-plus` · `light-plus` ·
`monokai` · `solarized-dark` · `solarized-light` · `abyss` ·
`kimbie-dark` · `quiet-light`

**Ein Schema liefert nur diese neun Farben.** Leinwand bleibt `Page`,
Fließtext bleibt `OnSurface`. Das eigene Hintergrundbild eines Schemas
mitzunehmen würde gegen die Glasflächen arbeiten — und ein dunkles Schema
unter hellem Theme malte dann dunkel auf weiß. Präferenz und Leinwand
sind zwei Dinge; dieser Schalter bewegt nur die Präferenz. Ein explizit
gesetzter Widerspruch wird ausgeführt, aber `set` sagt es an.

Ohne Deckung durch eine Span rendert ein Byte in `OnSurface` — das ist
der Normalfall und der Grund, warum die Rampe klein bleiben kann.

## 2. Akzent

Vier Presets aus dem Design, plus `auto` (Wallpaper-Extraktion, wie
bisher). Wählbar über `set accent <name>`:

| Preset  | Accent    | Ink (Text auf Accent, dark) |
|---------|-----------|------------------------------|
| `rose`  | `#e39bab` | `#1c1315`                    |
| `sage`  | `#8fbf9f` | `#111a14`                    |
| `blue`  | `#9fb8e0` | `#111620`                    |
| `amber` | `#e0b877` | `#1d1710`                    |

Im Light-Mode ist Ink immer `#ffffff`.

Abgeleitete Tokens — alle **vorgemischt undurchsichtig** über `Surface`,
weil der Rasterizer die Alpha-Bytes der Tokens ignoriert (die Deckkraft
kommt aus `bg_alpha`):

| Design         | Token         | Mischung             | Rolle                          |
|----------------|---------------|----------------------|--------------------------------|
| `accent-soft`  | `AccentMuted` | 15 % Accent          | Selektionsfüllung, Active-Bg   |
| `accent-ring`  | `AccentRing`  | 22 % Accent          | 3px-Fokusring                  |
| `accent-line`  | `AccentLine`  | 45 % Accent          | Rahmen des fokussierten Tiles  |
| `accent-ink`   | `OnAccent`    | —                    | Text auf Accent-Füllung        |

## 3. Widget-Zustände (verbindlich)

Aus der `WIDGET-ZUSTÄNDE`-Sektion des Mockups. Das ist der Teil, den jede
App gleich umsetzen muss.

### `toolbar_button` — 30×30, Radius 7

| Zustand  | Umsetzung                                        |
|----------|--------------------------------------------------|
| rest     | kein Hintergrund, Icon `OnSurface`               |
| hover    | `Background(SurfaceHover)`                       |
| active   | `Background(AccentMuted)` + `Tint(Accent)`       |
| focus    | `Ring { token: Accent, width: 2 }`               |
| disabled | `Tint(OnSurfaceFaint)` + `Opacity(115)`          |

### `text_field` — Höhe 30, Radius 8

| Zustand | Umsetzung                                                        |
|---------|------------------------------------------------------------------|
| rest    | `Background(Surface)` + `Border { Border, 1, 8 }`                 |
| focus   | `Border { Accent, 1, 8 }` + `Ring { token: AccentRing, width: 3 }`|

### `list_row` — Höhe 34

Selektion ist **Tint + 2px-Kante links**, kein voller Balken und kein
umlaufender Rahmen.

| Zustand  | Umsetzung                                                       |
|----------|-----------------------------------------------------------------|
| rest     | transparent, 2px transparente Kante links (hält das Layout)     |
| hover    | `Background(SurfaceHover)`                                      |
| selected | `Background(AccentMuted)` + 2px-Kante `Accent` + `Tint(Accent)` auf dem Icon + Text `Strong` |

### `tab` — Höhe 30, Radius `8 8 0 0`, feste Breite, Ellipsis

- Der **aktive Tab trägt die Farbe des Inhalts darunter** — im Editor
  `Page`, im Browser `Surface`. Der Tabstrip selbst ist `SurfaceElevated`.
- hover: `Background(SurfaceMuted)`
- inaktiv: transparent, Text `OnSurfaceMuted`
- Das `×` erscheint nur auf aktivem/gehovertem Tab. Sonst zeigt der Slot
  bei ungespeicherten Änderungen einen 8px-Punkt in `Accent`.

### `dock` — Reveal

**Nicht nachbauen:** im Mockup schiebt sich hinter dem Dock ein
rechteckiger Balken nach oben in die Kacheln. Das ist ein Renderfehler
des Entwurfs, kein Gestaltungsmittel. Das Dock ist ein freistehender
Tray mit Abstand zum unteren Rand; darüber liegt nichts.


4px-Hot-Zone · 180 ms rein, 400 ms raus · Slide+Fade 160 ms · die Kacheln
reflowen mit. Drei Zustände: `0 aus` (nichts) · `1 hint` (34×4-Streifen
`OnSurfaceFaint` am unteren Rand) · `2 offen` (Tray).

Icon-Zellen 34×34, Radius 9. Laufindikator unter der Zelle:
laufend+fokussiert = 12×2 `Accent` + Zelle `Background(SurfaceHover)`;
laufend = 3×2 `OnSurfaceFaint`; nicht laufend = nichts.

## 3a. Fokus und Texteingabe (verbindlich, systemweit)

Vorher war das pro Widget-Art verschieden — der Browser fühlte sich
richtig an, der Dateimanager sperrte die Tastatur im Suchfeld ein. Eine
Regel für alle:

| Ereignis | Verhalten |
|----------|-----------|
| Fenster erscheint | Fokus liegt **nirgends**, außer ein Textfeld trägt `Modifier::Autofocus`. Das ist der Launcher- und Dialogfall, nicht der Normalfall. |
| Klick **in** ein `Input`/`TextArea` | Fokus wandert dorthin. |
| Klick **irgendwo sonst** im Fenster | Fokus wird freigegeben — auch auf Knöpfe, Listenzeilen und leere Fläche. Es gibt immer einen Weg heraus. |
| Fokus verlässt ein Feld | Der getippte Puffer wird **geparkt**, nicht verworfen. |
| Fokus kehrt in dasselbe Feld zurück | Der geparkte Puffer lebt weiter. |

Der letzte Punkt ist keine Bequemlichkeit, sondern Notwendigkeit: der
Editorpuffer des Compositors läuft dem Baum der App **konstruktionsbedingt
eine Runde voraus**. Wer ihn beim Fokusverlust wegwirft, verliert alles,
was seit dem letzten Commit der App getippt wurde — das Feld wirkte
geleert und füllte sich erst bei Enter wieder.

**Folge, bewusst in Kauf genommen:** ein Klick ins Menüband nimmt auch
dem Editor in spell den Cursor. Das ist der Preis für eine Regel ohne
Ausnahmen; die Alternative wäre eine Sonderbehandlung für Menüs, und
genau solche Sonderfälle haben den Zustand vorher unvorhersehbar gemacht.

## 4. Fenster-Chrome (Compositor)

- Kachel-Gap **6 px**, Radius **10 px**.
- Fokussiertes Fenster: 1px `AccentLine`. Unfokussiert: 1px `Border`.
- Panel (Bar): Höhe 36, Radius 8, 6px Rand ringsum, 1px `Border`.
- Dock-Tray: Radius 12.

## 5. Fensteraufbau der Apps

Von oben nach unten, alle drei Apps teilen das Raster:

1. **Menüband** — 36 px, `SurfaceElevated`. App-Icon (`Accent` wenn
   fokussiert, sonst `OnSurfaceFaint`), dann die Menülabels in
   `OnSurfaceMuted`, rechts das Plattform-`×`.
2. **Tabstrip** — 36 px, `SurfaceElevated`, Tabs unten bündig (nur beak
   und spell).
3. **Toolbar** — 44 px, `Surface`, unten 1px `Border`.
4. **Inhalt** — `Page`.
