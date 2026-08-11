# BROWSER.md — Native, Sandboxed Web Browser (`beak`)

A **native** web browser for nopeekOS: HTML + CSS + JavaScript rendered
directly through the widget/compositor stack, over the native `net::tcp` +
TLS stack — **no Linux guest, no microVM**. Runs as a single WASM widget
app inside the trust boundary. Untrusted web JS is *doubly contained*.

> Status: **Stage 0 shipped and running on hardware** (beak 0.1.44,
> 2026-07-22). ~13k lines of engine + a 1.3k-line shell render real
> Wikipedia, google.ch and Marginalia articles: DOM, full cascade, external
> stylesheets, block/inline/table/flex/grid layout, positioning, PNG + SVG,
> HTML GET forms. **No JavaScript yet** — that is Stage 1.
>
> This document is both the design skeleton and the running status. Sections
> §1, §2, §4, §7, §9 are the *vision* and have held up unchanged; §3, §5, §6
> and §10 carry the **as-built** state and are updated as things land. The
> per-feature conformance numbers live in `docs/spec/CONFORMANCE.md`.
>
> Relationship to the existing browser: the microVM+LibreWolf stays as the
> **compatibility browser** (full modern web, JS-heavy SPA webapps). `beak`
> is the **native browser** — lighter, capability-gated, principle-pure,
> covering the readable + lightly-dynamic web and growing over time. They
> coexist; they are not a replacement for each other.

---

## 1. Why

The microVM browser is the one place the OS violates its own manifesto:
behind the WASM trust boundary sits a *complete Linux 6.18 + POSIX +
Firefox* — exactly the legacy stack nopeekOS threw out on page 1. It also
drags the whole net-bridge perf saga (`memory/project_browser_net_perf.md`,
`project_netbench_coldstart.md`), 2 GiB guest RAM, 9p, audio RT-promotion.

A native browser instead:

- **Embodies all 5 principles** — capability-gated networking, WASM-
  sandboxed, content rendered through *our* compositor, runtime-driven.
- **Cheap** — a few MB WASM app on the native net stack (which already does
  ~gigabit for OTA), vs. a whole Linux VM.
- **Safer than Firefox** (§4) — the sandbox model falls out for free and is
  *stronger* than a process sandbox.

## 2. Scope — honest boundary

The goal is **"the biggest part of the internet"**, not Servo-completeness.
No small graphical browser does JS-heavy sites *well* — that cannot exist;
the web platform is the largest API in computing. So we draw a line:

**In scope (target: renders correctly):**
- Content + lightly-dynamic sites: news, Wikipedia, blogs, docs, forums,
  HN, GitHub (read), search results, most shop product pages.
- HTML5 tolerant parsing, CSS (box model, **Flexbox + Grid**, position,
  typography, colors/borders/backgrounds, media queries), images.
- JavaScript: language + enough Web Platform (DOM, events, timers, `fetch`)
  that jQuery/light-React/Vue *content* renders and links/forms work.

**Beyond the v1 *target* (NOT a hard wall — the frontier, §9):**
- Heavy SPA webapps (Figma, Google Docs, online IDEs), `<video>`,
  WebGL/WebGPU, WebRTC, Service/Web Workers, extensions, DRM/EME.

These are past the *initial* target, not past what's *possible*. This
project's whole history is "impossible → shortcut" (WLAN via WASM driver,
audio-RT via PID-1 promoter, this browser via double-containment). So §2 is
where v1 *lands*; §9 is the frontier with the known attack vector for each
wall. `beak` grows toward and then past this boundary stage by stage (§6),
and never degrades to *blank* — see the Reader-mode fallback (§9.7).

**A wall that is not technical (found 2026-07-20).** "Search results" is in
the in-scope list above, and rendering them is not the problem — *being
allowed to fetch them* is. Measured against the live engines: Google
`/search` is a JS shell (we render a blank page), DuckDuckGo html returns a
202 anti-bot challenge, Mojeek demands verification, searx.be answers 403.
A full Firefox User-Agent did **not** lift any of these — they fingerprint,
they don't just read the UA. The one engine that answers plainly is
**Marginalia** (built for text/no-JS clients), which is why the address bar's
omnibox points there. Google's homepage, by contrast, renders beautifully.
The lesson generalises: from here on, some failures are policy, not
capability — measure which one before building anything (§8.1 is the same
story for HTTP/1.1).

## 3. Architecture

Solid arrows are built and running; the dashed JS box is Stage 1.

```
  npk_http_request ──▶ HTML bytes ──▶ [Tokenizer] ──▶ DOM tree (Rust)
   (NET cap, TLS)                                        │  ╎
                                                         │  ╎ mutate
  CSS bytes ──▶ [CSS parser] ──▶ stylesheets ──┐         ▼  ╎
   (<style> + external <link>)                 ▼      ┌ ─ ─ ─ ─ ─ ─ ┐
                                        [Style: cascade,  JS engine  ╎ ◀── <script>
                                         specificity,   │ (interpreter,
                                         inherit, vars] │  no JIT §4)╎
                                               │        └ ─ ─ ─ ─ ─ ─┘
                                               ▼                  ╎ DOM/CSSOM/
                                        [Layout: OUR block+inline, ╎ fetch/timer
                                         table, flex, grid,  ◀ ─ ─ ┘ host objects
                                         box model, position]
                                               │
                                               ▼
                                        [Paint: OUR rasteriser + fontdue]
                                               │
                                               ▼
                                        Widget::Canvas ──▶ compositor
                                        (npk_canvas_commit, raw BGRA)
```

**Layout is ours, not taffy** (§7-D2). `fontdue` is the only third-party
piece in the render path, and only to turn glyph outlines into coverage
bitmaps — the same crate the kernel UI uses.

**The real monster is NOT the JS engine — it's the DOM + Web Platform + the
re-entrant layout loop.** The *language* JS is the small, solved part. Real
sites bottom out in `querySelector`, `element.style.x = …`,
`addEventListener`, and `getBoundingClientRect` / `offsetWidth` — which force
*synchronous* layout mid-script. So the engine must:

- own a **live DOM** (Rust) that JS mutates, which dirties style + layout;
- run the **event loop** (microtasks/macrotasks, `setTimeout`);
- support **synchronous layout-on-read** (a script reads `offsetWidth` →
  layout must flush now);
- dispatch **events** (capture/bubble: click, input, scroll);
- expose **`fetch`/XHR** wired to the native net stack.

The JS engine is a bytecode interpreter bolted to this. The DOM/CSSOM/event-
loop glue + a re-entrant layout engine is where the effort concentrates.

### 3.1 Crate layout (as built)

```
tools/wasm/beak-engine/    # PURE no_std+alloc core — NO host-fn deps (portable, host-testable, §10)
  src/
    lib.rs      380   public API: Engine, stylesheet_links, image_srcs
    dom.rs      599   tolerant HTML tokenizer + tree builder; Element.seq identity
    css.rs     1183   css-syntax-3 subset: rules, selector lists, specificity, @media
    style.rs   2017   ComputedStyle, UA sheet as data, inheritance, cascade incl. !important
    values.rs   458   lengths/units (em, rem, %, calc), Units ctx
    vars.rs     502   CSS custom properties (--x, var()), @media-aware collection
    color.rs    914   CSS Color 4 (named, hex, rgb/hsl/hwb/lab/lch, colour spaces)
    layout.rs  4194   OUR layout: block+inline flow, table, flex, grid, box model, position
    raster.rs   315   display list → BGRA (glyph cache, synthetic bold/italic, image blit)
    fonts.rs     82   embedded subsetted faces (see assets/subset.sh)
    image.rs    429   PNG decode (8-bit RGB/RGBA + palette), decode budget
    forms.rs    507   form/control collection, FormState, successful-control rules, urlencode
    svg.rs     1446   SVG subset: paths, shapes, fill + stroke
  tests/
    wpt.rs            the oracle: WPT reftests → render → pixel-compare
    diag.rs           dev tool (DPAGE/DWIDTHS/DCTRL/…), untracked on purpose

tools/wasm/beak/           # nopeekOS shell (the WASM app), 1258 lines
  src/lib.rs        menu bar, toolbar, address bar/omnibox, history, Canvas body,
                    sub-resource fetching, form focus + submission, scroll, theming
```

Everything is `no_std` + `alloc`. That rules out `html5ever`/Servo (std,
huge) — the HTML/CSS/DOM core is hand-rolled (§7-D0). The only third-party
crates in the engine are `fontdue` (glyph rasterisation) and `miniz_oxide`
(PNG inflate).

**Not built as designed:** there is no `platform.rs` / `Platform` trait yet,
and no separate `beak-desktop` adapter. The split the trait was meant to
enforce happened anyway — the engine crate has zero host-fn dependencies and
runs natively on the dev box (§10) — but the shell calls `npk_*` directly
rather than through an abstraction. A desktop port would add the trait then;
the cost of retrofitting it turned out to be low, because the seam held.

## 4. Security model — the crown jewel

`beak` fetches untrusted HTML/CSS/JS and runs that JS in an interpreter that
*itself* runs inside the WASM trust boundary. Untrusted web JS is **doubly
contained**:

- **Box 1** — page JS runs in the JS interpreter → no native code gen, no
  memory outside the engine's own heap objects.
- **Box 2** — the interpreter runs inside `beak`'s WASM boundary → formally
  bounded, capability-gated, no syscalls except our host fns.

Malicious page JS can do *only* what `beak` exposes to it (DOM ops + `fetch`
through the capability-gated NET host fn). No path to native code, no memory
outside linear memory, no host access beyond declared caps. This is
*stronger* isolation than a browser process sandbox — and it's free.

**The invariant that keeps it airtight: interpreter, NEVER a JIT.** A JS JIT
needs host-granted W^X executable memory; granting that punches a hole in
the sandbox. So "no JIT" is a **security invariant**, not a perf compromise.
(A future WASM-emitting JIT that asks the host to instantiate modules is a
deliberate, separately-gated escalation surface — *not* v1.)

**Capabilities.** `beak` declares a new **NET** bit in its `.npk.caps`
section (analogous to HARDWARE 0x40, CAPTURE, CANVAS —
`memory/project_widget_app_caps.md`). It gets NET + RENDER + CANVAS. It does
**not** get WRITE / filesystem — page content never touches npkFS unless a
download is explicitly, separately capability-granted (v2). Same-origin
policy + no local-file scheme enforced in `net.rs`, above the capability.

**Security checkpoint (per CLAUDE.md):** "Can page JS escape through this?"
The whole design answers *No* by construction: page JS never sees anything
but DOM host objects and a scoped `fetch`.

## 5. What we already have vs. what's missing

**Have (reuse directly):**
- Net: `net::tcp` + TLS chain enforcement + DNS (OTA does ~gigabit natively).
- Render: compositor, alpha-compositing, glyph atlas/fonts, **Canvas escape-
  hatch**, PNG decode (wallpaper module).
- Browser-shell primitives (built for Spell/loft): `TextArea` (address bar),
  `Scroll` + clip-rect (page viewport), `Span` runs (styled/clickable text →
  links!), `Popover`, mouse selection + copy/paste (v0.227.0),
  `Modifier::Tint/Scale/NodeId`.
- WASM app platform: caps, singleton routing, `npk_open`/launch-args, tabs.

**Built since (was the "missing" list):**
- **HTML** tolerant tokenizer + tree builder (`dom.rs`) — implied end tags,
  raw-text `<script>`/`<style>`, entities, implicit `<html>`/`<body>`.
- **CSS** parser + cascade (`css.rs`/`style.rs`) — hand-rolled, no vendored
  `cssparser`/`selectors`. Full order: inherited → UA sheet → author
  (specificity + doc order) → inline, then a second `!important` pass.
  Plus custom properties (`vars.rs`), CSS Color 4 (`color.rs`), `@media`.
- **Layout, ours** (`layout.rs`) — block + inline flow, tables, flexbox,
  grid, the box model, positioning. No taffy, as decided in §7-D2.
- **Paint, ours** (`raster.rs`) — display list → BGRA, glyph cache,
  synthetic bold/italic, image blit.
- **PNG** decode + an **SVG** subset (`svg.rs`, fill + stroke).
- **HTML GET forms** (`forms.rs`) — inputs, buttons, checkboxes, selects,
  textareas; successful-control rules; submit → `GET action?field=val`.
- Host fns: **`npk_http_request`** (NET cap) and **`npk_http_final_url`**
  (the URL the body actually came from, after redirects — a browser needs it
  to resolve relative sub-resources correctly).
- ABI additions that landed for beak: `npk_canvas_rect` (canvas viewport →
  1:1 paint + click coords), `Event::Wheel`, `npk_theme_token`, and a
  press into a `Widget::Canvas` releasing the compositor's text focus
  (without which the app never sees a key).

**Still missing:**
- **JS engine** — our own; language grown as a subset (§7-D1). Stage 1.
- **The glue** — DOM/CSSOM host objects, event loop, event dispatch,
  synchronous reflow-on-read, `fetch`/XHR bridge. Stage 2.
- **JPEG** decoder — most web photos are JPEG and currently render as
  placeholders. The single biggest remaining *image* gap.
- **HTTP/2** in the kernel — see §8, this is not a nicety: parts of the web
  now rate-limit HTTP/1.1 clients outright.
- `@import` and relative `url()` *inside* CSS are not resolved.
- Text selection in form fields, non-ASCII keyboard input.
- Cookies on SUB-RESOURCES: only the document request carries them, so an
  image or stylesheet behind a login still comes back unauthenticated.
  `npk_http_request_many` (the multiplexed sub-resource path) takes no
  headers yet.
- Cookie PERSISTENCE: the jar is session-only, see §5.1.

**Verified against the tree (2026-07-03, all since confirmed by use):**
- ABI is browser-ready: `Widget::Canvas` + `npk_canvas_commit` (CANVAS cap
  0x20, raw BGRA upload) = the page-raster target; `Scroll` = viewport;
  `Input` = address bar; `Popover`/`Menu` = context menus.
- Host TLS/HTTP already existed: `intent::http::https_get` +
  `https_get_streaming` (used by OTA) over `net::tcp::connect` + `net::dns`.
  The browser's fetch is a thin host-fn wrapper over these.
- The `.npk.caps` section was **full** at 1 byte (READ/WRITE/EXEC/RENDER/
  CAPTURE/CANVAS/HARDWARE/NETCTL) → extended to 2 bytes, with **NET** as
  bit 11. NETCTL (0x80) is WiFi-supplicant control, NOT general net — it was
  deliberately not reused.

## 6. Staged milestones (each a demoable artifact, no cliff)

| Stage | Content | Proves | State |
|-------|---------|--------|-------|
| **0** | fetch HTTPS → HTML parse → CSS → **our** layout → paint. **No JS.** | the whole pipeline; de-risks everything | ✅ **shipped** (0.1.44) |
| **3′** | Flexbox, Grid, `position:*`, box model, tables, external CSS, images, forms. | "biggest part of the internet", pre-JS | ✅ **shipped** — pulled forward, see below |
| **1** | Embed JS engine: `console.log`, inline `<script>` against a read-only DOM. | engine runs *in the sandbox* | next |
| **2** | The hard glue: live DOM mutation → reflow, event loop + timers, `fetch`/XHR, click/input dispatch. | real dynamic web | after 1 |

**The order changed, deliberately.** Stage 3's layout work turned out to be
the thing standing between us and real sites, not JS — a Wikipedia article,
google.ch's homepage and Marginalia's results all render *without a line of
JavaScript*. So flexbox/grid/position/tables/forms were pulled forward ahead
of the JS engine, and Stage 3 is largely done before Stage 1 starts. What is
left for the original Stage 3 is CSSOM, which needs JS anyway.

Stage 0's demo target (`example.com`, then a Wikipedia article with
headings, links and an image) was met on 2026-07-03 and has been the daily
regression check since.

**Choosing test targets:** does the content ship *in the delivered HTML*
(server-rendered → fair game), or does the page hand back an empty
`<div id="root">` (SPA → structurally unreachable before Stage 2)? Wikipedia
qualifies and is the primary target: 1,878 visible words are in the served
HTML and `<html class="client-nojs">` is MediaWiki's officially supported
branch, with only 65 CSS rules gated on `.client-js`.

## 7. Decisions

**D0 — Own the engine (decided).** No wholesale vendoring of foreign render
or JS engines (taffy / Servo / Blitz / QuickJS / Boa). We build the engine
ourselves, clean-slate — "how would you build this *today*, without dragging
millions of lines of legacy." Adapt good *ideas* freely; do not copy code.
Keeps control, minimizes external deps, and — the real engineering payoff —
lets us design DOM ↔ style ↔ layout ↔ paint ↔ JS-host-objects as *one*
coherent model with unified dirty-tracking, instead of impedance-matching
foreign libraries with mismatched memory models. The trickiest seam
(reflow-on-read, §8) gets designed once, coherently.

**D1 — JS engine: our own, language as a growing subset.** Like the CSS
subset (§2): implement the ES surface real content sites actually use, grow
it demand-driven. Clean-slate architecture (parser → bytecode → interpreter
→ GC), *informed by* studying Boa (pure-Rust design) and QuickJS (compact
interpreter design) — ideas, not code. Honest caveat (§8): this is the one
true mountain — the ES spec is genuinely huge, and "no legacy" shrinks it
less here because the *spec itself* is the weight. Mitigated by sequencing
(Stage 0 ships a useful owned browser first) and by growing the language as
a subset, not chasing conformance.

**D2 — Layout: our own (decided, no taffy).** Hand-roll block + inline flow,
then Flexbox, then Grid — each a precisely-specified CSS algorithm, hard but
bounded (weeks per layer, not years). taffy stays a *reference* for its
box-tree + measure-fn model; we don't vendor it.

### 7.1 How the decisions held up (2026-07-22)

- **D0/D2 were right, and the estimate was optimistic in a useful way.** Each
  layout layer took days, not weeks — but `layout.rs` is 4,194 lines and is
  now the piece that most often needs care. The predicted payoff (one coherent
  model, no impedance-matching) showed up in an unglamorous way: features slot
  in without rework *because* we own every layer, and the cascade seam designed
  at the UA-sheet step absorbed author CSS, external sheets, `@media`, custom
  properties and `!important` in turn, each time without restructuring.
- **The order was wrong, and correcting it was the biggest single win.** The
  plan put JS at Stage 1 and "full flexbox/grid" at Stage 3. Reality: real
  sites are unreadable without layout and perfectly readable without JS. §6
  now reflects the corrected order.
- **The thing that actually accelerated everything was §10's testability
  bonus**, not any decision in §7 — see §10.
- **Two recurring bug patterns, both from owning the whole stack.** (1) *A
  feature must go into all three walks* — block flow, inline flow **and**
  `layout_box` (flex/grid items, table cells). Adding `<img>` to only the
  block walk made Wikipedia's `<a><img>` thumbnails vanish; adding form
  controls to only two walks made real search boxes render nothing, because
  they sit in `display:flex`. (2) *Measurement passes must roll back what they
  collect* — the height-measuring passes discard ops but once kept controls,
  so the same control was recorded four times at stale positions.

## 8. Risks / open questions

- **JS engine = bounded grunt work, NOT a research risk.** ECMA-262 defines
  *every* behavior precisely, and ships **test262** (~50k executable
  conformance tests) as an oracle — so we never *guess* conformance, we run
  it (fits `feedback_test_on_hw.md` / messen-nicht-raten). The surface is
  large (RegExp, Promises, generators, async, Proxy, typed arrays, the
  numeric tower) but fully *mapped*; the only constraint is throughput, which
  keeps dropping (AI code-gen). "Alpha Centauri" framing: start now, grow the
  language as a subset (§2), let the vehicle get faster mid-flight. Stage 0
  ships value with *no* JS meanwhile.
- **Reflow-on-read** (sync layout mid-script) is the trickiest engine seam —
  get the dirty-bit + flush model right early (Stage 2), it colors the DOM API.
- **Text/inline layout** — line-breaking (UAX #14) + bidi + shaping. Latin +
  simple scripts are ours to build; full complex-script shaping (Arabic,
  Indic — HarfBuzz territory) is a later frontier, degrade until then.
- **Fonts** — enough coverage (CJK, emoji) for real pages, or a v2
  fallback-box story?
- **Perf** — a bytecode interpreter is fine for content sites; the JS→WASM
  AOT tier (§9.1) is the lever if app-like sites demand it. Measure on HW
  (`memory/feedback_test_on_hw.md`), don't guess.
- **Cookie persistence** — the jar exists (v0.263.0) but dies with the
  process. A session cookie is a credential, and a credential at rest is its
  own decision: where it lives, who else can read it, whether it is
  encrypted. Closing beak logs you out until that is answered.

### 8.1 HTTP/1.1 is now a walled garden (measured 2026-07-22)

This one was filed under "SNI+ALPN, v2" and turned out to be a *blocker*,
not a nicety. Fetching a Wikipedia article's images produced a wall of
`HTTP 429` after the fourth one. Reproduced host-side and bisected — same
IP, same User-Agent, same headers, runs back to back:

| Variant | Result |
|---|---|
| HTTP/1.1, one connection | 4× 200, then **429 throughout** |
| HTTP/1.1 + `Accept-Encoding` / `Referer` | unchanged |
| HTTP/1.1, fresh connection per request + 300 ms pacing | almost all 429 |
| **HTTP/2** | **20 of 20 = 200** |

HTTP/2 was run *while* the IP was already being limited and still got clean
200s; HTTP/1.1 immediately afterwards hit the wall again. So it is not IP
reputation, not the UA, not missing headers — **Wikimedia's Varnish
front-end throttles HTTP/1.1 clients**, because every real browser speaks
h2. Sustainable HTTP/1.1 rate: **~0.5 requests/second**.

Two consequences worth writing down, because both are the obvious move and
both are wrong:

- **`Retry-After` backoff does not help.** The header says `retry-after: 1`,
  but waiting 2 s still returns 429. It would be HTTP-correct and still take
  40+ seconds per page.
- **Parallel HTTP/1.1 connections make it worse.** The limiter counts per
  IP, not per connection — six connections just reach the wall sooner.

**So HTTP/2 is the answer, and it is the same work as parallelism:**
multiplexing many requests over one connection is what the browsers we are
being compared against actually do. It collapses three problems at once —
the 429s, the missing concurrency, and the ~8 serial round-trips before the
first paint. Groundwork: our TLS 1.3 client sent no ALPN extension at all
(we never offered h2); `tls_connect_alpn` + `TlsSession::alpn()` now exist.
The rest is frames + HPACK + flow control + streams (RFC 9113 / 7541).
Planned simplification: advertise `SETTINGS_HEADER_TABLE_SIZE = 0` so the
server may not use HPACK's dynamic table and our decoder needs only the
static one — fully spec-compliant, and it removes a whole moving part.

Details and the repro recipe: `memory/project_beak_http_fetch.md`.

---

## 9. Frontier — moving the boundary (the shortcuts)

§2's "beyond v1" list is a set of walls, each with a known attack vector.
None is a principle violation; all are "hard but crackable" — the project's
signature move. Ordered by leverage.

**9.1 — JS too slow (interpreter, no JIT).** The biggest wall for app-like
sites. *Shortcut:* **JS→WASM AOT of hot functions.** The engine already
lowers to bytecode; lower hot functions further to **WASM**, and have the
*host* instantiate them as a child module with a `beak`-controlled import
table. Critically this **preserves the no-JIT security invariant (§4)** —
the emitted code is WASM, verified by the same trusted validator that runs
`beak` itself, so it's as contained as `beak`. Not a native JIT; a WASM-
emitting tier. Precedent: Javy (QuickJS→WASM), Porffor (JS→WASM AOT).
Profile-guided, hot 10% only. New host capability: "instantiate child
module" — gate hard, bound by fuel/memory (a malicious page must not force
unbounded compilation). *Difficulty: research-y, Stage 4+.*

**9.2 — Web platform too broad (thousands of APIs).** A *coverage* wall, not
speed. *Shortcut — highest leverage:* **push platform breadth into
injectable JS polyfills.** A real browser can't (it *is* the platform);
`beak` can. The native Rust core implements only the *irreducible*
primitives (DOM nodes, layout-read, timers, `fetch`, rAF, events);
everything else (IntersectionObserver, ResizeObserver, URL, structured
clone, …) ships as our own `beak-runtime.js` prelude (ideas adapted from
core-js — not its code). Moves surface from Rust (hard) to JS (easy,
rewritable, fully ours). *Difficulty: medium, huge payoff.*

**9.3 — DOM / reflow thrashing.** SPAs do thousands of mutations + layout
reads. *Shortcut:* batch mutations, dirty-region layout, flush once per
frame unless a script forces a sync read (`offsetWidth`). Standard browser
tech. Our layout is currently whole-document per invalidation, with a cache
so scrolling doesn't re-lay-out; incrementality is a Stage-2 concern.
*Difficulty: medium.*

**9.4 — Fancy CSS (transforms, gradients, animations, opacity).** *Shortcut:*
the Canvas escape-hatch already does arbitrary 2D — transforms/gradients/
compositing are raster ops we can already emit; `requestAnimationFrame` maps
to the compositor frame tick. Skip 3D-transforms/filters/blend-modes first,
degrade gracefully. *Difficulty: low–medium.*

**9.5 — `<video>`.** *Shortcut:* software WASM codec (dav1d for AV1, or
software H.264) decoding frames painted to a canvas surface; audio via the
existing HDA path (`project_audio_hda.md`). Low-res first. Not forbidden by
any principle — just CPU. *Difficulty: medium–hard.*

**9.6 — WebGL / WebGPU.** *Shortcut (far-future):* map a minimal WebGL onto
the native GPU work (BCS blitter + modeset, `project_gpu_4k_display.md`).
*Difficulty: hard, later.*

**9.7 — Universal safety net: Reader-mode extraction.** When a site won't
render cleanly, run a Readability-style pass over the DOM to extract the
article content and render *that* cleanly. **Degrade to content, never to
blank.** Makes "the biggest part of the internet" *readable* even where it's
not pixel-perfect — and it's a small, high-value win. *Difficulty: low.*
*Partly built:* the shell has a **Site-CSS on/off toggle** (Ansicht menu) that
falls back to UA-only styling, which is the cheap half of this and doubles as
the A/B tool when a site's own CSS makes things worse. The Readability-style
content *extraction* is not built.

## 10. Portability — engine core vs platform port (Linux/Windows too)

The engine — HTML/CSS/JS/layout/paint — is pure, platform-agnostic
computation; only the *shell* is nopeekOS-specific. Structure `beak` as a
pure **engine core** behind a narrow **`Platform` trait** (ports & adapters),
with per-OS adapters:

    trait Platform {
        fn fetch(&self, url: &str) -> Result<Vec<u8>, Error>;  // nopeek: npk_http_request | native: rustls
        fn present(&mut self, bgra: &[u8], w: u32, h: u32);    // nopeek: npk_canvas_commit | native: window blit
        fn poll_input(&mut self) -> Option<InputEvent>;        // nopeek: npk_event_poll   | native: winit
        fn now_ms(&self) -> u64;                                // JS timers / rAF
    }

The engine paints the page into its own pixel buffer (OUR text rasterizer,
not the compositor glyph atlas), so it needs the host only for *surface +
input + fetch + clock* — nothing OS-bound. ~90% (the engine) is portable;
only the thin adapter differs per platform.

**Decide it NOW:** free at scaffold time, expensive to retrofit. A `Platform`
trait from day one → the native port is "add a second impl"; reaching for
`npk_*` directly everywhere means untangling it later.

**Bonus 1 — native-host testability (big):** the pure engine crate runs on
the Linux dev box with a headless adapter, so layout tests and **test262**
run at native speed *without booting nopeekOS* — accelerates the JS climb (§8).
This paid off more than anything else in the design: **5,786 WPT reftests**
are vendored and run in ~5 minutes on the dev box (**3,626 passing** as of
2026-07-22), and `tests/diag.rs` renders any real page to a BMP, so a
rendering defect can be reproduced, bisected and fixed without hardware in
the loop. Numbers and how to run it: `docs/spec/CONFORMANCE.md`.

**Bonus 2 — one binary, same sandbox:** on desktop, run the *same* `beak.wasm`
under `wasmtime` with host-fn shims mirroring the nopeek ABI → identical
double-containment (§4) on all three platforms.

**Stays nopeek-specific (does NOT port):** the sandbox/cap model (§4,
`.npk.caps` NET — desktop falls back to OS process sandbox or the wasmtime
route above); the widget-ABI chrome (address bar as `Widget::Input` — native
draws its own); npkFS integration (downloads, `.open-in-loft` — native uses
the real FS). All shell, not engine.

**Status (2026-07-22): the portability bonus is real and load-bearing, the
`Platform` trait is not.** Page rendering does go through our own layout +
rasteriser into a Canvas, so the engine crate genuinely has no host
dependency — it compiles and runs on the Linux dev box, which is where the
WPT oracle and the `diag.rs` page renderer live. That, not the trait, is
what made the last weeks of work possible: nearly every rendering bug was
found and fixed **without booting nopeekOS**, by rendering the real page to
a BMP on the dev box.

What was *not* built is the trait itself — the shell calls `npk_*` directly
instead of implementing `Platform`. The §10 warning ("decide it NOW, free at
scaffold time, expensive to retrofit") turned out to be too pessimistic in
this case, because the engine/shell boundary held on its own: the engine
never grew a host call to hide behind an abstraction. A desktop port would
introduce the trait at that point, over a small surface (fetch, present,
input, clock).

## Appendix — host fns as built

```
npk_http_request(url_ptr, url_len, buf_ptr, buf_max) -> bytes | -1   // NET cap
    // GET only. Follows redirects; returns the BODY, not status+headers.
    // Runs over net::tcp + TLS with the kernel's keep-alive pool.

npk_http_final_url(buf_ptr, buf_max) -> len | -1                     // NET cap
    // The URL the last request's body actually came from, after redirects
    // (RFC 3986 §5.1.3). Without it, relative sub-resources resolve against
    // the URL we ASKED for and every one of them repeats the document's own
    // redirect. Cleared on error so a stale URL can't leak into the next page.
```

The proposal was a full request/response fn (method, headers, body, status).
What shipped first was narrower — GET, body only — which is all Stage 0
needed. **v0.263.0 built the rest of it**, because the two things that
narrowing cost turned out to be the same two things a person needs to log in
anywhere: `npk_http_send` (method, caller headers, request body) plus
`npk_http_response_headers` / `npk_http_status`.

Two rules the boundary enforces, both about the fact that a header block is
just text a sandboxed app hands us:
- **CR, LF and every other control character are refused** in a method or a
  header line. A newline in a header VALUE ends the block early and what
  follows is read as a second request — one check is what stops an app from
  smuggling one through us.
- **`Host`, `Content-Length`, `Transfer-Encoding` and `Connection` are ours**,
  never the caller's. `Host` decides which virtual host answers; the other
  three frame the message, and a body that disagrees with its announced
  length is exactly the shape of a request-smuggling bug.

Cookie POLICY stays out of the kernel: which cookie belongs on which request
is RFC 6265, and that is the browser's job (`beak_engine::cookies`).

The name `beak` (nopeek → the bird's beak) stuck, and follows the lowercase
app-naming convention (loft, dock, bar, drun, spell, iris, snap, volume).
