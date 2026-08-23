# Stufe 0 — Aufklärung vor der JS-Runde (2026-08-23)

Zweck: den JS-Aufwand für beak **beziffern statt schätzen**, bevor eine Zeile
Engine entsteht. Gemessen, nicht erinnert — Rohdaten und Skripte entstanden in
einer Session, die Zahlen unten sind reproduzierbar mit `curl` + `acorn`.

## 1. Zielwahl: claude.ai fällt aus, die Klasse nicht

    GET https://claude.ai/  ->  HTTP/2 403
    cf-mitigated: challenge

claude.ai liegt hinter einer Cloudflare Managed Challenge (Turnstile: Client
Hints, obfuskiertes JS mit `unsafe-eval`, iframe auf challenges.cloudflare.com,
Worker aus blob:). Das ist **kein Rendering-Problem** und wird nicht dadurch
gelöst, dass beak besser wird. Eine Umgehung wäre Bot-Detection-Umgehung und
widerspricht ausserdem `feedback_no_ua_impersonation`.

Bemerkenswert: die Challenge ist **bedingt**. Am Gerät hat beak am selben Tag
einmal 403 und einmal 200 auf `/login` bekommen; curl bekommt konstant 403.

Gemessen wurde deshalb an zwei erreichbaren SPAs derselben Klasse:
`excalidraw.com` (React/Vite, 2,4 KB Hülle) und `app.netlify.com`
(React-Dashboard, 10,5 KB Hülle). Beide liefern `<div id="root">` und sonst
nichts — strukturell identisch zu claude.ai.

## 2. Transport (an claude.ai gemessen, Asset-Host ist offen)

Dasselbe Stylesheet, drei Kodierungen:

| Accept-Encoding | Bytes     | Faktor |
|-----------------|-----------|--------|
| identity        | 1'033'647 | 1,00   |
| gzip            |   134'499 | 7,68   |
| br              |   130'108 | 7,94   |

**gzip ist Pflicht, brotli bringt darüber 3,3 %** → brotli von der Liste
streichen.

Weiter: HTTP/2 bestätigt, `alt-svc: h3`. **HTTP 103 Early Hints** wird
ausgeliefert — beak muss 1xx mindestens überlesen können. COOP `same-origin` +
COEP `require-corp`.

## 3. Die JS-Klippe — GEMESSEN, nicht geschätzt

acorn-Bisektion über `ecmaVersion` pro Bundle, niedrigste Version die parst:

| Ziel | Bundles | komprimiert | entpackt | **Klippe** |
|------|---------|-------------|----------|------------|
| excalidraw.com | 2 | 853 KB | **2,76 MB** | **ES2020** |
| app.netlify.com | 19 | 2,28 MB | **9,37 MB** | **ES2022** |

Bei netlify verteilt es sich: 9 Bundles ES2015, dann einzeln ES2018/2019/2020/
2021 und **drei Bundles ES2022** (0,92 + 0,27 + 1,09 MB). Ein einziges
ES2022-Bundle setzt die Klippe für die ganze App.

**Konsequenz für die Subset-Strategie:** anders als CSS degradiert JS nicht.
Ein fehlendes Feature wirft, und alles danach im selben Script läuft nie. Es
gibt also keine sanfte Rampe — **ES2022 muss nahezu vollständig stehen**, bevor
irgendein reales Bundle durchläuft. Danach wird es inkrementell.

## 4. Web-API-Zensus

Statischer Zensus über alle Bundles (globale Namen und DOM-Properties überleben
die Minifizierung). **Vorkommen, nicht Startaufrufe** — ein Treffer heisst
"referenziert", nicht "beim Start ausgeführt".

Zahlen = excalidraw / netlify.

| Gruppe | Befund |
|---|---|
| DOM-Kern | createElement 136/710, setAttribute 213/158, querySelector 132/159, classList 42/63, innerHTML 29/32 |
| Events | addEventListener 163/331, preventDefault 146/189, keydown 31/56, focusin 14/28, **beforeinput 2/2**, compositionstart 4/8 |
| Layout-Lesen | getBoundingClientRect 43/71, offsetWidth 9/29, scrollTop 15/61, getComputedStyle 27/36 |
| Zeit + Planung | setTimeout 99/201, requestAnimationFrame 20/62, queueMicrotask 7/11, requestIdleCallback –/15 |
| Observer | ResizeObserver 9/30, MutationObserver 2/26, IntersectionObserver 3/8, PerformanceObserver –/8 |
| Netz | fetch 13/130, XMLHttpRequest 11/32, AbortController 6/12, WebSocket 2/18, EventSource 1/4, ReadableStream 2/10 |
| Speicher | localStorage 44/69, sessionStorage 7/23, indexedDB 8/10 |
| Routing | pushState 3/18, replaceState 8/11, popstate 3/14 |
| Editieren | getSelection 6/27, Range( 11/49, contentEditable 3/6 |
| Worker | Worker 22/51, postMessage 12/48, SharedWorker 1/2 |
| Krypto + URL | URLSearchParams 9/129, matchMedia 10/30, getRandomValues 9/15, crypto.subtle 8/4 |

**Keine Gruppe ist leer.** Drei Posten, die vorher unterschätzt waren:

- **Web Worker** (22/51) — ein zweiter Ausführungskontext, nicht optional
- **Selection/Range** (11/49) — auch in Apps ohne Chat-Composer
- **XMLHttpRequest** (11/32) — lebt neben `fetch` weiter, nicht ersetzt

## 5. CSS — Zensus an claude.ai (1,08 MB Tailwind v4)

- 15'651 Deklarationen, davon 3'634 Custom Properties (1'351 distinct `--x`)
- 12'017 echte Deklarationen, 302 distinct Eigenschaften
- **beak deckt heute 82,3 %** (`css_props!`-Tabelle, 187 Einträge)

Die fehlenden 2'132: Animation+Transition 740 (ein **System**, keine
Eigenschaftsliste), At-Rule-Deskriptoren 418, Einzel-Transforms 255,
Interaktion 180, -webkit- 127, Compositing 86, SVG 41, scroll-* 29.

**Das eigentliche Gap sind die At-Rules**, nicht die Eigenschaften:

| At-Rule | claude.ai | beak |
|---|---|---|
| @media | 423 | ja |
| @keyframes | 105 | **nein** |
| @property | 100 | **nein** |
| @container | 60 | **nein** |
| @font-face | 15 | **nein** |
| @supports | 12 | ja |
| @starting-style | 11 | **nein** |
| @layer | 6 | ja |
| @position-try | 3 | **nein** |
| @scope | 1 | **nein** |

Selektorlast: `[data-*]` 4'129, `:where()` 2'785, `:is()` 1'177, `:has()` 213,
`calc()` 1'900, `color-mix()` 796, `oklab` 559, `var()` 11'580.

## 6. Befund am Rande, der die Reihenfolge betrifft

Gerätemessung, claude.ai/login (leere SPA-Hülle) gegen Wikipedia:

| | DOM | Stylesheet | dom::parse | css::cascade | box layout |
|---|---|---|---|---|---|
| Wikipedia | gross | klein | 100 ms | 600 ms | 1180 ms |
| claude.ai | winzig | 1 MB | 10 ms | **2210 ms** | 30 ms |

**2210 ms Cascade auf einem DOM, der in 10 ms parst.** Die Kosten hängen an der
Regelzahl, nicht am Dokument — Signatur von "jede Regel gegen jedes Element",
also kein Index. Der benannte Mechanismus dagegen ist Standard: Regeln nach dem
rechtesten einfachen Selektor in Eimer sortieren, plus Bloom-Filter über die
Vorfahren für Abstammungs-Selektoren.

**Die Skalierungswand steht damit vor der JS-Runde, nicht dahinter** — sichtbar
ohne eine Zeile Skript, und mit ~28x Interpreter-Zoll darüber.

## 7. Was noch offen ist

Braucht einen echten Browser (Extension war nicht verbunden):
- Ob claude.ai's Composer `contenteditable` oder `textarea` ist
- SSE oder WebSocket für das Streaming
- Service Worker ja/nein
- Welche APIs beim **Start** tatsächlich laufen (statt nur referenziert zu sein)
