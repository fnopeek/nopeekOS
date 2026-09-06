# Was der Plattform noch fehlt — gemessen, nach Aufrufzahl

**Stand 2026-09-06, beak 0.118.0.** Ausgelöst von Florian: *„bau es und für
alles andere sockets etc. was es sonst noch braucht auch gleich einplanen
oder als todo notieren."*

Dieses Papier ist eine **Rangliste, keine Wunschliste.** Jede Zeile hat eine
Zahl, und die Zahl kommt aus dem Aufrufzensus über echte Seiten
(`<tools>/jsscope/apicensus.mjs`, liegt NEBEN dem memory-Verzeichnis), nicht aus einem Gefühl dafür, was modern
klingt.

```
cd tools/wasm/beak-engine
APICENSUS=<tools>/jsscope/out/apicensus.json cargo test --release --test apigap -- --nocapture
```

> **Vor jeder Planungsrunde dieses Kommando laufen lassen.** Die Reihenfolge
> hier ist die von heute; sie verschiebt sich, sobald etwas gebaut ist.
> [[feedback_count_it_dont_sample_it]]

**Deckung heute: 98,5 % von 301 127 Aufrufen** (296 540). Die fehlenden 1,5 %
sind 4 587 Aufrufe, und sie verteilen sich nicht gleichmässig — die Hälfte
steckt in vier Paketen.

---

## 0. Was 0.118.0 geschlossen hat

`fetch`, `Response`, `Headers`, `AbortController`, `AbortSignal`
(`beak-engine/src/js/fetch.rs`, `beak/src/lib.rs::pump_fetches`).
**98,3 % → 98,5 %**, und die Fritzbox-Oberfläche lädt ihr `rest-helper.js`
zum ersten Mal durch (6 von 6 Modulen statt 5 von 6).

**Nur gleiche Herkunft.** Siehe `BROWSER_FETCH_ORIGIN.md` §3.5: gebaut ist
Stufe **C ohne ihre fremde Hälfte**. Eine fremde Herkunft wird abgelehnt und
sagt warum. Der Grund steht im Papier und ist keine Bequemlichkeit: *„Eine
halbe CORS ist gefährlicher als keine."* Ohne Antwortprüfung darf es keine
fremde Antwort zu lesen geben, sonst liest eine öffentliche Seite
`https://192.168.178.1/` aus.

Bewusst nicht gebaut, und im Kopf von `fetch.rs` benannt: `Request`-Objekte,
Rümpfe ausser Text, `response.body` als Strom, `AbortSignal.timeout`.

---

## 1. Die Pakete, nach Gewicht

| # | Paket | Aufrufe | wo | Grösse |
|---|---|---:|---|---|
| **P1** | **Shadow DOM** — `attachShadow`, `shadowRoot`, `assignedSlot`, `ShadowRoot.host`, `adoptedStyleSheets` | **876** | Engine + Kaskade | gross |
| **P2** | **`MessagePort.postMessage`** | **505** | Engine | mittel |
| **P3** | **Echte Scroll-Masse** — `scrollHeight`, `scrollWidth`, `scrollTop`, `scrollLeft`, `offsetParent` | **660** | Layout → Bindung | mittel |
| **P4** | **`IntersectionObserver`** | **429** | Engine + Layout | mittel |
| **P5** | **`ResizeObserver`** | **353** | Engine + Layout | mittel |
| **P6** | **`MutationObserver`** | **116** | Engine | klein |
| **P7** | Kleinkram, je < 70: `ariaHidden` 68, `nextElementSibling` 112, `toggleAttribute` 56, `URL.username/password` 94, `History.state` 46, `document.referrer` 45, `Node.isConnected` 42, `Element.attributes`/`NamedNodeMap` 75, `createTreeWalker` 26, `document.hidden`/`visibilityState` 50, `currentSrc` 28, `DocumentFragment.*` 54 | **~700** | Engine | je klein |

**P3 ist der billigste Gewinn je Aufruf.** Die Eigenschaften sind schon da —
sie antworten nur 0, weil die Bindung die Layoutkästen nicht fragt. Das ist
keine neue Schnittstelle, das ist eine Leitung.

**P1 ist das grösste und das teuerste.** Shadow DOM ist nicht nur eine
Baum-Erweiterung: die Kaskade muss Grenzen kennen (`:host`, `::slotted`,
`adoptedStyleSheets`), und die Ereigniszustellung muss neu ziehen.

---

## 2. Sockets

**Der Kernel bietet rohes TCP an Module schon an** — kein neuer Slot nötig,
kein ABI-Umbau:

```
npk_tcp_connect(ip_packed, port) -> handle     kernel/src/wasm/forge_glue.rs:898
npk_tcp_status(handle)
npk_tcp_send / npk_tcp_recv
npk_tcp_close
```

**Aber zwei Dinge fehlen dafür, und beide gehören genannt, bevor jemand
anfängt:**

1. **Kein TLS.** `npk_tcp_connect` nimmt eine **gepackte IPv4 und einen
   Port** — Klartext-TCP. `wss://` braucht TLS, und der Kernel hat einen
   TLS-Stapel (er fährt `https://` über `npk_http_*`), aber er ist nicht als
   Stromsocket herausgeführt. Das ist ein **Kernel-Posten**: ein
   TLS-fähiger Stromsocket als neue Hostfunktion (anhängend,
   [[feedback_abi_append_only]]).
2. **Kein DNS an dieser Stelle.** Die Adresse kommt gepackt herein, also muss
   der Rufer schon aufgelöst haben.

**Und die Herkunftsfrage gilt hier genauso.** Ein `WebSocket` ist ein Kanal,
den keine CORS-Antwortprüfung schützt — das Web löst das über den
`Origin`-Kopf und die Zustimmung des Servers im Handschlag. Wer `WebSocket`
baut, baut die Regel mit, oder er baut ein Loch. Bis dahin gilt dieselbe
Antwort wie bei `fetch`: **gleiche Herkunft**.

Reihenfolge, wenn es drankommt:

| | was | wo |
|---|---|---|
| **S1** | TLS-Stromsocket als Hostfunktion | Kernel |
| **S2** | `WebSocket` (Handschlag, Rahmen, `close`), gleiche Herkunft | Engine + beak |
| **S3** | `Origin`-Kopf + fremde Herkunft nach Serverzustimmung | Engine |

`EventSource` (SSE) ist danach fast geschenkt — es ist eine lange HTTP-Antwort
und braucht **P8** (strömender Antwortkörper), nicht S1.

---

## 3. Was `fetch` selbst noch offen hat

Aus `BROWSER_FETCH_ORIGIN.md` §3.5, mit dem Stand von heute:

| | was | Stand |
|---|---|---|
| A | Herkunft/Site-Begriff + `SameSite` | **offen** — `cookies.rs` kennt die Site nicht |
| B | Reichweiten-Riegel im Kernel (V2) | **offen** — `https://192.168.x.x` ist heute unbeschränkt |
| C | `fetch` gleiche Herkunft mit Keks | **gebaut (0.118.0)** |
| C′ | fremd einfach + CORS-Antwortprüfung | **offen** |
| D | Vorabanfrage (`OPTIONS`) + same-site-Kekse cross-origin | offen |
| E | Kekse auf Unterressourcen | offen, braucht A |
| **P8** | **Strömender Antwortkörper** (`response.body`) | offen — Voraussetzung für SSE und für grosse Antworten |

**B ist unabhängig von allem anderen und schliesst eine Lücke, die HEUTE
offen ist** (§1.4 des Papiers). Es wartet auf nichts.

---

## 4. Regeln für dieses Papier

* **Zahl vor Zeile.** Wer hier etwas einträgt, trägt die gemessene Aufrufzahl
  mit ein. Ohne Zahl gehört es nicht in die Rangliste, sondern in einen Satz
  darunter.
* **Was gebaut ist, wandert nach §0 und bekommt die neue Deckungszahl.**
* Eine Schnittstelle, die halb da ist, ist gefährlicher als eine, die fehlt —
  das gilt für CORS (§3.1 des Papiers) und für alles, was eine Vollmacht
  einführt.
