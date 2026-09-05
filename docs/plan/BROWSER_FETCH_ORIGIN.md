# `fetch` und die Herkunftsfrage — Entscheidungspapier

**Stand 2026-09-05, beak 0.100.0.** Ausgelöst von Florian: *„fang mit fetch
an — erst das Papier zur Same-Origin-Frage."*

> **ENTSCHIEDEN, 2026-09-05.** Florian: *„schlussendlich müssen wir es so
> bauen, dass es moderne Websites unterstützt — das ganze sicher und
> schnell. Wie obliegt bei dir."*
>
> Damit ist §5.1 beantwortet: **moderne Seiten heisst angemeldete Seiten,
> also müssen Anmeldedaten möglich sein.** Der erste Entwurf dieses Papiers
> schlug vor, sie ganz wegzulassen (Regel R1 „nie Kekse"). Das war die
> sichere Antwort auf eine andere Frage — sie hätte claude.ai und jede
> andere angemeldete Seite dauerhaft ausgeschlossen.
>
> **Die getroffene Entscheidung steht in §3.** Die Analyse in §1 und §2
> bleibt gültig und trägt sie; nur die Schlussfolgerung ist eine andere,
> weil die Anforderung eine andere ist. Der verworfene Entwurf steht in
> §3.0, damit die Begründung nachvollziehbar bleibt.

Ein Skript in beak kann heute **nichts** aus dem Netz holen: `fetch`,
`XMLHttpRequest`, `WebSocket` und `EventSource` fehlen alle vier. Das ist der
Riegel vor jeder dynamischen Seite. Bevor eine Zeile davon gebaut wird, muss
entschieden sein, **wer wen wovor schützt** — sonst wird die Regel später
nachgerüstet, und nachgerüstete Grenzen sind keine.

`docs/spec/BROWSER.md` §4 sagt bereits: *„Same-origin policy + no local-file
scheme enforced in `net.rs`, above the capability."* Das ist eine Absicht,
kein Entwurf. Dieses Papier soll sie zu einem machen.

---

## 1. Was heute WIRKLICH gilt — gemessen, nicht angenommen

Bevor man eine Grenze zieht, muss man wissen, wo die alte liegt. Alles hier
ist am Code nachgelesen (`tools/wasm/beak/src/lib.rs`,
`kernel/src/intent/http.rs`), nicht erinnert.

| Weg | fremde Herkunft? | Kekse dabei? | Antwort für Skript lesbar? |
|---|---|---|---|
| Navigation (Adresszeile, Link) | ja | **ja** (`cookies::header_for`, Zeile 932) | — |
| `<link rel=stylesheet>` | ja, ungeprüft | **nein** | nein |
| `<script src>` | ja, ungeprüft | **nein** | — (läuft aber) |
| `<img src>`, CSS-`url()` | ja, ungeprüft | **nein** | nein |
| Formular-Absenden | ja, ungeprüft | ja (Navigation) | — |
| **`fetch` aus einem Skript** | **gibt es nicht** | — | — |

Daraus folgen vier Feststellungen, und die dritte ist die wichtige:

1. **Eine Seite kann beak heute schon zu GETs an beliebige Hosts bringen.**
   `<img src="https://fremd/?daten">` ist ein Ausleitungskanal, und er ist
   offen. `fetch` erfindet ihn nicht.
2. **`<script src>` von fremder Herkunft wird geholt UND ausgeführt**, in
   derselben Umgebung wie das Seitenskript. Das ist die grösste bestehende
   Vertrauensfläche, grösser als alles, was `fetch` hinzufügt.
3. **Unterressourcen fahren OHNE Kekse.** Nur die Navigation hängt sie an.
   beak ist damit heute **strenger als jeder normale Browser** — es gibt
   keine mitreisende Vollmacht (*ambient authority*) auf Unterressourcen.
   **Das ist der Grund, warum wir die Regel des Webs nicht übernehmen
   müssen** (§2).
4. **Es gibt gar keinen Schutz vor dem eigenen Netz.** Private Adressen
   kommen im Kernel an genau einer Stelle vor — und dort als *Erlaubnis*,
   nicht als Sperre: `plain_http_allowed()` (`http.rs:2129`) lässt
   Klartext-HTTP **nur** an 10/8, 172.16/12, 192.168/16, 127/8 und
   169.254/16 durch, und auch das nur mit dem Schalter
   `net.allow_plain_http=1`. Für `https://` wird die Funktion nie gefragt
   (einziger Aufruf: `http.rs:2094`, im `http://`-Zweig).
   **`https://192.168.1.1` ist damit heute völlig unbeschränkt**, auch als
   Unterressource einer fremden Seite. Der Kommentar bei der Funktion
   benennt das Restrisiko für den Klartextfall bereits — für den
   TLS-Fall steht es nirgends. Offene Lücke, unabhängig von `fetch`,
   eigener Posten in §5.

---

## 2. Warum Same-Origin nicht einfach importiert wird

Die Regel des Webs ist keine Naturkonstante, sie ist die Antwort auf eine
Eigenschaft, die Browser haben und wir nicht:

> Ein Browser hängt Anmeldedaten **automatisch** an jede Anfrage an ihre
> Herkunft. Damit ist jede Anfrage eine Handlung *im Namen des angemeldeten
> Nutzers* — und deshalb muss der Browser einschränken, wer die ANTWORT
> lesen darf. Sonst liest `evil.com` per Skript das Postfach mit.

Same-Origin ist also die Reparatur einer mitreisenden Vollmacht. Heute haben
wir diese Vollmacht auf Unterressourcen gar nicht (§1.3) — **mit der
Entscheidung für angemeldete Seiten führen wir sie ein.** Damit gilt: wer
die Ursache baut, muss die Reparatur mitbauen. Ganz.

Der Wert dieser Analyse liegt nicht darin, CORS zu vermeiden, sondern darin,
**zu wissen, wogegen jeder einzelne Teil davon schützt** — und was er NICHT
abdeckt. Same-Origin vermischt drei Schutzziele:

| | wovor | wer deckt es ab |
|---|---|---|
| **A — fremde Daten lesen** | Skript liest eine Antwort, die nur wegen der Vollmacht des Nutzers zustande kam | **CORS** — und ab jetzt trifft es uns, weil wir Kekse anhängen |
| **B — Reichweite** | Seite erreicht Ziele, die ihr Absender selbst nicht erreicht: Intranet, `localhost`, Gerätedienste | **niemand.** CORS tut hier NICHTS. Browser haben es als *Private Network Access* nachgerüstet, unvollständig |
| **C — Ausleitung** | Seite schickt Daten hinaus | niemand — und schon heute offen (§1.1) |

Zwei Schlüsse, und beide gehen in §3 ein:

1. **A ist ab jetzt real, also wird CORS vollständig gebaut.** Halb gebaut
   wäre schlimmer als gar nicht: die Vollmacht da, die Reparatur nur zum
   Teil.
2. **B ist die, die wirklich weh tut, und CORS beantwortet sie nicht.** Das
   ist unsere eigene Zutat, und sie ist der Ort, an dem Architekturprinzip 1
   greift: Reichweite ist eine *Kapabilität*, keine Berechtigung — sie wird
   erteilt, nicht erfragt, und sie reist nicht von selbst mit.

---

## 3. Die Entscheidung

### 3.0 Was verworfen wurde, und warum es aufgeschrieben bleibt

Der erste Entwurf war: **`fetch` schickt nie Kekse.** Damit fällt
Schutzziel A weg und mit ihm fast die ganze CORS-Maschinerie — kein
Preflight, kein `Allow-Credentials`. Sehr klein, sehr prüfbar, sehr sicher.

**Und unbrauchbar für das erklärte Ziel.** Jede angemeldete Seite spricht
per Skript mit ihrer eigenen API; ohne Keks ist das kein „eingeschränktes
Anmelden", sondern gar keines. Die Regel hätte moderne Seiten nicht
begrenzt, sondern ausgeschlossen.

Die Lehre, die bleibt: *ein Modell, das nur deshalb einfach ist, weil es
die Anforderung nicht erfüllt, ist nicht einfach — es ist unfertig.*

### 3.1 Das Modell: das echte Web-Modell, VOLLSTÄNDIG, plus eine Grenze, die das Web nicht hat

Moderne Seiten setzen das Web-Modell voraus. Davon abzuweichen heisst, sie
zu brechen. Also wird es gebaut wie es ist — **und zwar ganz**:

> **Eine halbe CORS ist gefährlicher als keine.** Wer Kekse anhängt, aber
> die Antwortprüfung nur teilweise baut, hat die Vollmacht eingeführt und
> die Reparatur weggelassen. Entweder beides oder keins.

Dazu kommen drei Verschärfungen, die **keine** moderne Seite brechen, weil
das Web sie ohnehin schon geht oder gar nicht braucht:

| # | Regel | bricht Seiten? |
|---|---|---|
| **V1** | **Keine fremden Kekse (cross-*site*).** Ein Keks fährt nur mit, wenn Anfrageziel und Dokument dieselbe *Site* sind. | nein — Safari und Firefox machen das seit Jahren, Seiten kommen damit klar |
| **V2** | **Keine Reichweite ins eigene Netz.** Eine öffentliche Herkunft erreicht nie 10/8, 172.16/12, 192.168/16, 127/8, 169.254/16, `::1`, `fc00::/7` — geprüft an der AUFGELÖSTEN Adresse, bei JEDEM Umleitungssprung. | nein — öffentliche Seiten tun das nicht |
| **V3** | **Kekse bleiben sitzungsgebunden** (steht schon so in `cookies.rs`). | nein — kostet nur einen Login je Sitzung |

V1 ist die wichtigste und sie ist zugleich die, die am besten zum Projekt
passt: sie schneidet die gesamte Verfolgungsfläche des Webs ab, und sie
macht den gefährlichsten CORS-Fall (`credentials: include` über Site-Grenzen)
schlicht unerreichbar. **Der Fall, den moderne Seiten wirklich brauchen —
`app.example.com` ruft `api.example.com` — ist same-*site* und bleibt
erlaubt.** Cross-Origin, aber nicht cross-Site.

V2 ist unsere eigene Zutat. Browser haben sie als *Private Network Access*
nachgerüstet und bis heute nicht vollständig. Wir bauen sie von Anfang an,
und sie schliesst eine Lücke, die §1.4 **heute schon** offen findet.

### 3.2 Was daraus folgt — die Regeln

**K1 — Herkunft und Site sind zwei Begriffe, und beide müssen existieren.**
Herkunft = Schema + Host + Port. Site = Schema + registrierbare Domain.
`SameSite` braucht den zweiten; `cookies.rs` sagt selbst, dass er fehlt und
„mit dem Skripting ankommt". Er kommt jetzt.

**K2 — `SameSite`, mit `Lax` als Vorgabe.** Wie in jedem heutigen Browser.
Ein Keks ohne Angabe fährt nicht auf einer fremden Site mit. `None` gilt nur
zusammen mit `Secure` — und wird von V1 ohnehin gestrichen.

**K3 — Kekse an `fetch`:**
* gleiche Herkunft → mit (Vorgabe `same-origin`, wie im Web)
* gleiche Site, fremde Herkunft → nur mit `credentials: "include"` **und**
  `Access-Control-Allow-Credentials: true` **und** einer namentlichen
  (nicht `*`) `Allow-Origin`
* fremde Site → **nie**, `credentials: "include"` wird abgelehnt und der
  Lauf sagt es

**K4 — CORS vollständig, für alles Fremde:** einfache Anfragen direkt;
alles andere mit Vorabanfrage (`OPTIONS`, `Allow-Methods`, `Allow-Headers`,
`Max-Age`-Zwischenspeicher). Antwort undurchsichtig ohne passendes
`Allow-Origin`; lesbare Kopfzeilen nur laut `Expose-Headers`.

**K5 — Kekse auch auf Unterressourcen**, nach denselben Regeln. Heute
fahren sie dort gar nicht (§1.3); moderne Seiten brauchen es, und mit K2/V1
ist es sicher. **Erst K2, dann K5** — die Reihenfolge ist keine Vorliebe,
sondern die Bedingung.

**K6 — Deckel, und sie melden sich.** Gleichzeitige Anfragen je Dokument,
Gesamtbytes, Umleitungstiefe. Jeder erreichte Deckel wird geloggt.

### 3.3 Wo die Regeln liegen

`SameSite`, Herkunft/Site und CORS sind **Browserregeln** und gehören zu
`cookies.rs`/beak — genau da, wo der Kopf von `cookies.rs` sie verortet
(„Policy lives here, in the browser, not in the kernel").

**V2 gehört in den Kernel.** Reichweite ist keine Browserregel, sie ist eine
Kapabilität — und eine Grenze, die das Sandkastenmodul selbst zieht, ist
keine ([[feedback-sandbox-protects-kernel-not-decision]]).

### 3.4 Schnell — was das konkret heisst

„Sicher und schnell" ist hier kein Zielkonflikt, weil die Klempnerei schon
richtig geformt ist:

* `npk_http_begin/poll/take/cancel` ist bereits asynchron — genau die Form,
  die ein `Promise` braucht. Kein Umbau.
* HTTP/2 samt Coalescing läuft (`kernel/src/intent/http.rs`).
* Der **Vorabanfragen-Zwischenspeicher** (`Max-Age`) ist nicht Beiwerk,
  sondern der Unterschied zwischen einer und zwei Rundreisen je Aufruf.
* Offen und schon bekannt: der Verbindungspool wirft am Gerät Verbindungen
  weg und verbindet neu ([[project-http-connection-reuse]]). Das trifft
  `fetch` doppelt, weil eine SPA viele kleine Anfragen macht.

## 3.5 Die Reihenfolge

Jede Stufe ist für sich prüfbar und lässt beak in einem stimmigen Zustand.

| | was | warum hier |
|---|---|---|
| **A** | Herkunft/Site-Begriff + `SameSite` (K1, K2) | Der Unterbau. Ohne ihn ist alles danach unsicher. Nach aussen unsichtbar. |
| **B** | Reichweiten-Riegel im Kernel (V2) | Schliesst eine Lücke, die **heute** offen ist. Unabhängig von `fetch`, deshalb nicht warten. |
| **C** | `fetch` — gleiche Herkunft mit Keks, fremd einfach + CORS-Antwortprüfung (K3 teilweise, K4 einfach, K6) | Der erste Punkt, an dem eine Seite etwas kann. Ohne Vorabanfrage: nicht-einfache fremde Anfragen werden mit klarer Meldung abgelehnt. |
| **D** | Vorabanfrage + same-site-Kekse cross-origin (K4 ganz, K3 ganz) | Vervollständigt CORS. Ab hier ist das Modell ganz. |
| **E** | Kekse auf Unterressourcen (K5) | Braucht K2, also A. |
| **F** | Strömender Antwortkörper | Der eigentliche Bauaufwand, und die Voraussetzung fürs Chatten. |

## 4. Was das für die drei Ziele heisst

* **github.com** — `fetch` ist hier nicht die Wand; es fehlen Custom
  Elements, echte `scrollHeight/Width`, `MutationObserver`, `pushState`.
  R1 stört nicht, weil die Darstellung ohne angemeldete API-Abfragen
  auskommt.
* **google.ch** — unberührt. Gemessen 2026-09-05: die Sperrseite kommt auch
  mit voller Chrome-Kennung, es ist keine Funktionslücke
  (`memory/project_beak_search_engines.md`).
* **claude.ai** — `fetch` mit strömendem Körper ist die erste
  Voraussetzung, aber **R1 verhindert das Anmelden**. Ohne die
  Keks-Kapabilität aus §5 bleibt es bei „lädt und rendert", nicht
  „chattet". Das ist eine bewusste Reihenfolge: erst die Grenze, dann die
  Tür.

---

## 5. Was noch offen ist

§5.1 des ersten Entwurfs („gibt es je eine Keks-Kapabilität?") ist
**entschieden** — siehe Kopf und §3. Offen bleibt:

1. **Fremde `<script src>` laufen ungeprüft** (§1.2). Das ist die grösste
   bestehende Vertrauensfläche, und sie macht jede `fetch`-Regel teilweise
   symbolisch: was ein Skript von fremder Herkunft ohnehin tun darf, muss
   es nicht über `fetch` tun. **Eigenes Thema, aber es gehört auf die
   Liste** — und es ist der Grund, warum V2 (Reichweite) im Kernel liegt
   und nicht in beak: sie ist die einzige der drei Grenzen, die auch dann
   noch hält, wenn Seitencode die Oberhand hat.
2. **Kekse auf Platte?** `cookies.rs` sagt heute bewusst nein („a credential
   at rest is a separate decision"). V3 hält das fest. Wenn ein Login je
   Sitzung zu lästig wird, ist das die Diskussion — mit Verschlüsselung und
   der Frage, wer sonst lesen darf.
3. ~~Registrierbare Domain ohne Public-Suffix-Liste.~~ **Ausgezählt und
   entschieden, 2026-09-05.** Im Zielkorpus stehen 691 verschiedene Hosts;
   bei **7** wäre „die letzten zwei Bestandteile" falsch — und zwar in die
   gefährliche Richtung: fünf `*.github.io`, `github-cloud.s3.amazonaws.com`
   und `pajhome.org.uk` würden als *dieselbe* Site gelten wie jede andere
   Seite unter derselben Endung. Kekse flössen zwischen fremden Nutzern.

   Statt eine Kurzliste zu raten wurde die echte gemessen:

       Public Suffix List   10 321 Regeln   144 KB roh   44 KB gzip
       davon ICANN 6 949, privat 3 372

   Der private Abschnitt ist genau der, der `github.io` und
   `s3.amazonaws.com` enthält — also der, der unseren Fall rettet. Gegen
   3,86 MB `beak.wasm`, 9 MB Python-Stdlib und 12 MB Kernelabbild sind
   144 KB nichts.

   **Entscheidung: die echte Liste wird eingebettet** (`include_str!`,
   unkomprimiert — kein Startaufwand, kein neuer Vertrauenspfad). Sie
   aktualisiert sich mit jedem beak-Release. Eine Sicherheitsgrenze
   approximiert man nicht, wenn die genaue Antwort 144 KB kostet.

---

## 6. Sicherheitsprüfung (CLAUDE.md)

> „Kann ein WASM-Modul durch diese Änderung aus seinem Sandkasten
> ausbrechen?"

**Nein — und die Prüfung greift diesmal eine Ebene tiefer.** Die Sandkasten-
grenze bewegt sich nicht: `fetch` ist eine Bindung auf bestehende
Wirtsaufrufe (`npk_http_begin/poll/take/cancel`), kein neuer Weg nach
draussen. Die Frage, die dieses Papier stellt, ist die zweite:
**kann SEITENcode etwas erreichen, was er nicht soll?** Dafür stehen R1–R5,
und sie stehen im Kernel, weil eine Grenze, die das Modul selbst zieht,
keine ist ([[feedback-sandbox-protects-kernel-not-decision]]).

---

## 7. Wie geprüft wird, bevor gebaut wird

Kein Fix vor der Karte. Was vor der ersten Zeile steht:

* eine Probe je Regel, gegen einen echten Motor gegengeprüft, wo es geht
* die R2-Liste **an aufgelösten Adressen** gefahren, nicht an Namen
* eine Prüfseite in `beak:selftest`, die die Absagen als *bestandene* Zeilen
  zählt — eine verweigerte Anfrage muss grün sein, nicht still

---

*Wenn §5.1 entschieden ist, kann gebaut werden. Vorher nicht: die
Keks-Frage bestimmt, ob R3 die kleine oder die grosse CORS-Maschinerie
braucht, und das ist kein Detail, das man nachträglich umdreht.*
