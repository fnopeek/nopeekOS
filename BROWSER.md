# BROWSER.md — Native, Sandboxed Web Browser (`beak`)

A **native** web browser for nopeekOS: HTML + CSS + JavaScript rendered
directly through the widget/compositor stack, over the native `net::tcp` +
TLS stack — **no Linux guest, no microVM**. Runs as a single WASM widget
app inside the trust boundary. Untrusted web JS is *doubly contained*.

> Status: **spec / vision (2026-07-03).** Decided: build the engine
> ourselves — no wholesale vendored render/JS engines (§7-D0). No code yet.
> This is the design skeleton to react to.
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

## 3. Architecture

```
  npk_http_request ──▶ HTML bytes ──▶ [Tokenizer] ──▶ DOM tree (Rust)
   (NET cap, TLS)                                        │  ▲
                                                         │  │ mutate
  CSS bytes ──▶ [CSS parser] ──▶ stylesheets ──┐         ▼  │
                                               ▼      [JS engine]  ◀── page <script>
                                        [Style: cascade,          (interpreter,
                                         specificity, inherit]     no JIT — §4)
                                               │                     │
                                               ▼                     │ DOM/CSSOM/
                                        [Layout: taffy               │ fetch/timer
                                         Flexbox+Grid+flow]  ◀────────┘ host objects
                                               │
                                               ▼
                                        [Paint] ──▶ Canvas escape-hatch / widget
                                                    primitives ──▶ compositor
```

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

### 3.1 Crate layout (proposed)

```
beak-engine/         # PURE no_std+alloc core — NO host-fn deps (portable, host-testable, §10)
  src/
    platform.rs    # the `Platform` trait: fetch / present / poll_input / now_ms
    html/          # tolerant tokenizer + tree builder → dom
    dom/           # live node tree, CSSOM, event target, mutation hooks
    css/           # parser, cascade, specificity, computed values
    layout.rs      # OUR block/inline flow → Flexbox → Grid (no taffy)
    paint.rs       # box tree → BGRA pixels (OUR text rasterizer)
    js/            # OUR JS engine (parser/bytecode/interp/GC) + DOM bindings
    image.rs       # PNG (have it) + JPEG decode

tools/wasm/beak/     # nopeekOS adapter + shell (the WASM app)
  src/main.rs      # impl Platform via npk_* ; address bar / tabs / scroll chrome

beak-desktop/        # (future) Linux/Windows adapter: winit + softbuffer + rustls
```

Everything is `no_std` + `alloc` against the existing WASM host ABI. That
rules out `html5ever`/Servo (std, huge) — the HTML/CSS/DOM core is hand-
rolled or uses `no_std`+`alloc`-clean crates (see §5).

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

**Missing (build):**
- **HTML** tolerant tokenizer + tree builder (hand-rolled, `no_std`).
- **CSS** parser + cascade/specificity/computed values (subset; possibly
  vendor `no_std`-clean bits of `cssparser`/`selectors`, else hand-roll).
- **Layout: our own** — block + inline flow, then Flexbox, then Grid. Each is
  a precisely-specified CSS algorithm: hard but bounded (weeks per layer, not
  years). taffy/Servo are *references* for the box-tree + measure-fn model;
  we do not vendor them.
- **JS engine** — our own; language grown as a subset (§7-D1).
- **The glue** — DOM/CSSOM host objects, event loop, event dispatch,
  synchronous reflow-on-read, `fetch`/XHR bridge.
- **JPEG** decoder (`no_std` Rust) for the common image case.
- Host fn: `npk_http_request(...)` (NET cap) — request/response over
  `net::tcp`+TLS.

**Verified against the tree (2026-07-03):**
- ABI is browser-ready: `Widget::Canvas` + `npk_canvas_commit` (CANVAS cap
  0x20, raw BGRA upload) = the page-raster target; `Scroll` = viewport;
  `Input` = address bar; `Popover`/`Menu` = context menus; and
  `Role::{Link,Heading,Image,List,ListItem}` already map to HTML semantics.
- Host TLS/HTTP already exists: `intent::http::https_get` +
  `https_get_streaming` (used by OTA) over `net::tcp::connect` + `net::dns`.
  The browser's `fetch` is a thin WASM host-fn wrapper over these — no new
  networking code, just exposure.
- **The one real gap:** WASM apps have NO network host fn (`npk_fetch` is
  npkFS/READ, not HTTP). Add `npk_http_request` in `kernel/src/wasm.rs`
  (`linker.func_wrap("env", …)`) wrapping `https_get_streaming`, gated by a
  new **NET** cap. The 1-byte `.npk.caps` section is **full** (all 8 bits:
  READ/WRITE/EXEC/RENDER/CAPTURE/CANVAS/HARDWARE/NETCTL) → extend it to 2
  bytes in the spawn path. NETCTL (0x80) is WiFi-supplicant control, NOT
  general net — do not reuse it. This is Stage 0's only ABI addition.

## 6. Staged milestones (each a demoable artifact, no cliff)

| Stage | Content | Proves | Needs engine? |
|-------|---------|--------|---------------|
| **0** | fetch HTTPS → HTML parse → CSS subset → taffy layout → paint. **No JS.** | the whole pipeline; de-risks everything | No |
| **1** | Embed JS engine: `console.log`, inline `<script>` against a read-only DOM. | engine runs *in the sandbox* | Yes |
| **2** | The hard glue: live DOM mutation → reflow, event loop + timers, `fetch`/XHR, click/input dispatch. | real dynamic web | Yes |
| **3** | Full Flexbox/Grid (taffy), `position:*`, more CSSOM → React/Vue *content* sites render. | "biggest part of the internet" | Yes |

Stage 0 needs no engine decision — we build the Rust DOM so either engine
binds later. Stage 0 demo target: `example.com` natively, then a Wikipedia
article (headings, paragraphs, clickable links → next page, one image).

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
- **Cookies / sessions / TLS SNI+ALPN** for real sites — v2 in `net.rs`.

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
tech; taffy supports incremental. *Difficulty: medium.*

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

**Bonus 2 — one binary, same sandbox:** on desktop, run the *same* `beak.wasm`
under `wasmtime` with host-fn shims mirroring the nopeek ABI → identical
double-containment (§4) on all three platforms.

**Stays nopeek-specific (does NOT port):** the sandbox/cap model (§4,
`.npk.caps` NET — desktop falls back to OS process sandbox or the wasmtime
route above); the widget-ABI chrome (address bar as `Widget::Input` — native
draws its own); npkFS integration (downloads, `.open-in-loft` — native uses
the real FS). All shell, not engine.

> Slice-0 caveat: the first increment renders via the widget-ABI shell (fast
> path to pixels on nopeek), which is nopeek-specific. The portable
> engine-core / `Platform` split becomes real when page rendering moves to
> Canvas-paint (our own layout + rasterizer). Recorded now so we build toward it.

## Appendix — proposed host fn

```
npk_http_request(req_ptr, req_len, resp_buf, resp_max) -> i32   // NET cap
    // req: method + URL + headers (+ body); over net::tcp + TLS.
    // resp: status + headers + body (streamed for large bodies, cf.
    //       project_streaming_downloads.md). Same-origin/redirect policy
    //       enforced in beak/net.rs above the capability.
```

Name `beak` is tentative (nopeek → the bird's beak). Matches the lowercase
app-naming convention (loft, dock, bar, drun, spell, iris, snap, volume).
