# beak

`beak` is the native web browser of nopeekOS. It fetches HTML, CSS,
JavaScript, images and fonts through the kernel's HTTP client, renders the
page with its own engine into a pixel buffer and shows it in a widget
window. It runs as one WASM module; page JavaScript runs in an interpreter
inside that module.

Conformance numbers are in [`CONFORMANCE.md`](CONFORMANCE.md).

## Where it runs

`beak` is a widget app (`tools/wasm/beak`, a `cdylib` built for WASM). The
kernel runs it under forge or wasmi; both bind the same host-function list
(`kernel/src/wasm/forge_glue.rs`).

It declares `RENDER`, `CANVAS` and `NET` in `.npk.caps`
(`[caps::RENDER | caps::CANVAS, caps::ext::NET]`, `NET` being bit `0x01` of
the second byte); the host function table below shows what each opens.
`beak` holds no `READ` or `WRITE`. Its only storage is its private area
`priv/beak/`, which the kernel opens to the module of that name without a
capability (`wasm::is_own_private`). The cookie jar lives there.
`build.rs` writes the `.npk.app_meta` section.

## Crates

`tools/wasm/beak-engine` is the engine: `no_std` + `alloc`, no host
functions, built and tested on the host. `tools/wasm/beak` is the shell:
host imports, chrome, tabs, input, the fetch loop and the cookie file.

The engine never calls the host. Where it needs something from outside, the
shell lends it a function pointer: the clock (`Engine::set_clock`), the
random source (`js::random::set_source`), the script deadline
(`Interp::deadline`) and the mid-script layout hook (`Interp::relayout`).
Everything the engine wants fetched it puts in a queue the shell drains.

Engine modules (`beak-engine/src`):

| Module | Content |
|---|---|
| `dom` | tolerant HTML tree builder (WHATWG §13 subset), `entities` (the full named-reference table), `charset` (UTF-8 and windows-1252) |
| `css`, `style`, `values`, `vars`, `color`, `media` | stylesheet parser and selector matching, cascade and computed style with the UA sheet as data, lengths and `calc()`, custom properties, CSS Color 4, media and container queries |
| `layout` | block and inline flow, tables, flex, grid, positioning, floats; produces a display list of `DrawOp`s, link and element rects |
| `raster` | `Engine`: holds faces and images, paints a display list into BGRA |
| `fonts`, `gsub`, `woff`, `woff2` | six embedded faces (Inter, Noto Sans Mono), `@font-face` loading, GSUB ligatures |
| `image`, `ico`, `svg`, `webp`, `picture` | PNG, JPEG, ICO, SVG, lossy WebP; `<picture>`/`srcset` selection |
| `forms`, `select` | form controls, submission encoding; text selection and find |
| `cookies`, `site` | the cookie jar; origin, site and the Public Suffix List |
| `js` | the JavaScript engine and the DOM binding (below) |

Third-party crates in the engine: `fontdue` (glyph coverage), `ttf-parser`
(GSUB tables), `hashbrown`, `miniz_oxide` (PNG, WOFF), `libm`, `zune-jpeg`,
`brotli-decompressor` (WOFF2). The VP8 decoder in `webp/` is ported from
`image-webp` with only the `std` parts replaced.

## Loading a page

A navigation is a chain of stages (`NavStage`). Each starts one
asynchronous host request; `nav_pump` collects it on a later turn of the
main loop, which keeps painting and taking keys meanwhile. Nothing is
painted between the document and its stylesheets.

1. **Document.** `nav_begin` reports the target to `npk_net_context`,
   cancels the previous load, drops the JS session and calls
   `npk_http_begin` (GET, or POST for forms) with the jar's cookies. On
   arrival the shell files `Set-Cookie`, decodes to UTF-8 (`Content-Type`
   charset or sniff) and takes the final URL as document URL and base.
2. **Stylesheets.** Every `<link rel=stylesheet>`, resolved and deduplicated,
   goes out as one batch (`npk_http_begin_many[_hdr]`).
3. **`@import`.** Imported sheets are fetched round by round, each resolved
   against its own sheet's URL.
4. **Scripts.** External classic and module scripts are fetched as a batch;
   then `run_scripts` runs them (see JavaScript).
5. **Module graph, `import()`, script-inserted sheets and scripts.** Each is
   fetched in rounds until nothing new is requested or its round cap is hit.
6. **Done.** The page is laid out and painted; images are queued.

After the first layout the shell fetches images in batches of four, CSS
background images the layout names, and the page's `@font-face` fonts
(at most 12 per round). Only what lands in the visible band triggers a
repaint.

A failed document fetch shows a local error page (`neterror.rs`) from
`npk_http_last_error`'s `kind\tmessage`: inline HTML, escaped, loading
nothing. `beak:selftest` (also `about:selftest`, `beak:test`) is compiled
into the binary (`selftest.html`) and takes the normal path after the fetch;
so is `beak:about` (also `about:beak`, Help > About), name, version and
licence.

Address bar input (`typed_to_url`): `http://` and `https://` stay as typed,
something that looks like a host gets `https://`, anything else becomes a
search on `https://marginalia-search.com/search?query=`.

Shell limits per page: document 3 MiB; 64 linked sheets, 8 MiB together;
32 external scripts, 8 MiB together; module graph 256 URLs in 24 rounds;
512 images; 6 `fetch`/XHR in flight, 2 MiB per response.

## Rendering

**Style.** Cascade: inherited values, the UA sheet, author rules by
specificity and order, inline style, then `!important`. At-rules handled
are `@media`, `@container`, `@supports`, `@font-face`, `@property`,
`@layer` and `@import`; unsupported selectors and at-rules are dropped.
UA colours come from a `Theme`; the shell sets a fixed light page palette
(white canvas, black text, `#0000EE` links), independent of the desktop
theme.

**Layout.** `Layout` is computed once per (content generation, width) and
reused for scrolling; the viewport height counts only when the page's
geometry depends on it. Display types: block, inline, inline-block,
inline-flex, list-item, flex, grid, the table types and `contents`.
Positioning: static, relative, absolute, fixed, sticky. `direction: rtl`
sets the base direction; there is no bidi reordering.

**Paint.** `Engine::paint` draws the display list into a viewport-sized
BGRA buffer. On a pure scroll the shell shifts the buffer and paints only
the band that came into view.

**Compositing.** The chrome is a widget tree sent with `npk_scene_commit`:
menu bar, tab strip, toolbar with back, forward, reload/stop and the address
field, the page canvas (`Widget::Canvas`, id 1), and when open the find bar
and the inspect status line. The page buffer goes to that canvas with
`npk_canvas_commit`; `npk_canvas_rect` gives its size and position for
layout width and hit testing. The address field shows the registrable
domain (Public Suffix List) at full strength and the rest muted.

The View menu has reload, a site-CSS toggle (UA sheet only) and inspect
mode (a click outlines and logs the element box).

## JavaScript

The engine is in `beak-engine/src/js`, written from scratch.

- **Lexer and parser.** Recursive descent with precedence climbing, ESTree
  AST, cover grammar for arrow parameters, scripts and modules. Early errors
  are incomplete.
- **Two machines, one semantics.** `compile` turns a program or a function
  body into bytecode for `vm`; what the compiler declines runs whole in the
  tree walker (`eval`, `expr`). A unit is never split between them, and both
  call the same helpers in `interp`. Generators and async functions run on
  their own `Vm`, which can be suspended.
- **Memory.** Values are `Rc<RefCell<Object>>` with full property
  descriptors. There is no cycle collector: cycles live until the
  interpreter is dropped, which happens on every navigation and tab switch.
- **Strings** index Unicode code points, not UTF-16 code units
  (`"\u{1F600}".length` is 1). RegExp is a backtracking matcher with a step
  cap, `\p{…}` tables generated from the UCD.
- **Built-ins** include Proxy, Reflect, BigInt (own bignum), typed arrays,
  `DataView`, Map/Set/WeakMap/WeakSet/WeakRef, iterator helpers, JSON,
  `structuredClone`, `URL`/`URLSearchParams`, `Date` (UTC only,
  `getTimezoneOffset()` is 0) and `Intl` with `DateTimeFormat`,
  `NumberFormat`, `PluralRules`, `RelativeTimeFormat` and `Segmenter`
  (English and German data). There is no `WebAssembly`, `SharedArrayBuffer`
  or `Atomics`.

**DOM binding** (`dombind.rs` with `dombind/extra.rs`, `dombind/surface.rs`).
The parsed tree is flattened into an arena (`Doc`); a JS node is an index,
wrapped once so identity holds. `Doc::to_dom` writes the arena back, and the
shell hands it to the engine (`set_scripted_dom`) whenever a script round
left it dirty. Layout reads (`getBoundingClientRect` and similar) answer
from the boxes of the last layout; the `relayout` hook lays the page out
again mid-script when the tree changed. `getComputedStyle` and `matchMedia`
use the same cascade and media state as layout.

Web APIs present on the global object: `document` (with `write`), `window`,
`self`, `location`, `history`, `navigator`, `console`, `crypto`
(`getRandomValues`, `randomUUID`, no `subtle`), `localStorage` and
`sessionStorage` (in memory only), timers, `requestAnimationFrame`,
`requestIdleCallback`, `queueMicrotask`, `performance`, `fetch` with
`Request`/`Response`/`Headers`/`AbortController`, `XMLHttpRequest`,
`WebSocket`, `FormData`, `TextEncoder`/`TextDecoder`, `DOMParser`, XPath,
`MutationObserver`, `ResizeObserver`, `IntersectionObserver`,
`PerformanceObserver`, `customElements`, `ShadowRoot`, `CSS`,
`atob`/`btoa`, the `HTML*Element` interfaces and the event constructors.
There is no `Blob`, `FileReader`, `Worker`, `postMessage`, `indexedDB` or
service worker. `navigator.userAgent` is `Mozilla/5.0 (nopeekOS) beak`;
`crypto` exists only when the host lent a random source
(`npk_random_bytes` in beak).

**Script round** (`run_scripts`). After the stylesheets, one session is
built from the fully parsed document. Classic scripts run in document order
(inline and external, `nomodule` skipped, non-JS `type`s skipped, a script
that fails to parse as a script is retried as a module); module scripts are
linked and evaluated after them. One failing script does not stop the next.
Then `DOMContentLoaded`; `load` and `pageshow` fire once the first layout
exists.

**Event loop.** `run_timers` drains microtasks, then runs due timers. The
timer clock is virtual: when nothing is due and no fetch is pending, it
jumps to the earliest deadline. The shell calls it after every entry point
(load, each dispatched UI event, an arriving `fetch` or WebSocket message,
observer delivery, the module and sheet rounds); after each entry point it
also applies cookie writes, `history` operations, scroll requests and
`location` navigations. `requestAnimationFrame` callbacks are due at once.

**Budget.** Each run of page code gets a wall-clock deadline of 900 s
(checked every 65 536 steps), a step cap of 2·10¹⁰ and a call-depth limit.
A run blocks the shell loop while it computes.

## Networking

All network I/O is the kernel's (`kernel/src/intent/http.rs`, `http2/`,
`crypto/tls`), reached through the host functions below.

- **TLS 1.3** with certificate verification in the kernel. A failure has a
  stable kind (`cert.untrusted`, `cert.expired`, `net.connect`, …) for the
  error page.
- **HTTP/2** is offered through ALPN on every browser request and used when
  the server agrees, else HTTP/1.1 with a keep-alive pool. Batches are
  multiplexed per host. The kernel asks for gzip and inflates it.
- **Plain `http://`** is refused unless `net.allow_plain_http` is `1` and
  the host is a literal private or loopback address; it is GET only.
- **Redirects** (301/302/303/307/308) are followed by the kernel, at most
  three per request. Caller headers are dropped once a hop changes host;
  301/302/303 after a non-GET become GET without body; TLS never downgrades
  (a plaintext hop may upgrade). Each hop's
  response headers are returned under a `:hop <url>` line, which no server
  can forge because the kernel drops header lines that start with `:`.
- **Header hygiene** (`header_line_is_safe`, `method_is_safe`): a method is
  1 to 16 uppercase ASCII letters; a header line has no control characters
  except tab, a graphic-ASCII name and at most 2048 bytes; `Host`,
  `Content-Length`, `Transfer-Encoding`, `Connection` and `Accept-Encoding`
  are the kernel's. A bad line refuses the request; a bad per-URL cookie
  line in a batch is dropped instead.
- **Asynchronous jobs** (`intent/fetch.rs`): `begin` returns a handle; one
  worker fiber runs the synchronous client, preferably not on core 0, the
  microVM core or the caller's. Limits: 16 jobs, 4 per process, 64 MiB
  reserved, 64 URLs per batch. `cancel` acts at the next chunk; `take`
  frees the job and alone fills the response getters (cleared on failure,
  untouched by a batch).

**Cookies** (`beak-engine/src/cookies.rs`; policy is beak's, the kernel
only carries bytes). The jar holds 256 cookies, dropping the oldest. It handles `Domain`, `Path`, `Secure`, `HttpOnly`, `Expires` (both
date spellings), `Max-Age` and the `__Secure-`/`__Host-` prefixes. A
`Domain` must be a parent of the host and not a public suffix (Public Suffix
List, `site`); a `Secure` cookie is accepted only from an `https` response
or hop. A cookie is replaced on (name, domain, path) and an expired one
deletes. Selection
for a request is by domain, path, `Secure` and expiry. Every request
carries the matching cookies: the document, each URL of a sub-resource
batch (one line per URL) and same-origin `fetch`/XHR. Script sees and sets
only non-`HttpOnly` cookies and cannot overwrite an `HttpOnly` one.
`Set-Cookie` is filed against the hop that sent it. Persistent cookies are
written to `priv/beak/cookies` when the jar changes and restored at start;
session cookies live until beak quits.

## Security model

Page code runs in beak's interpreter, which emits no native code, inside
beak's WASM sandbox, which reaches only the host functions below. Page code
has no path to a host function: every request it makes is a queue entry the
shell decides on.

**Enforced by beak** (inside the module, so only against the page):

- `fetch`, `XMLHttpRequest` and `sendBeacon` go to the document's origin
  only; a foreign URL rejects or fails before any request
  (`same_origin_url` in `js/fetch.rs`), and a response whose final URL
  (after the kernel's redirects) is on another origin fails as a network
  error. `<img>`, stylesheets, scripts and
  fonts load cross-origin but never hand bytes to page code.
- `WebSocket` connects only to the page's own origin (scheme, host and
  port, `wss` for `https`); `ws://` from an `https` page is refused, and
  the shell drives `wss://` only.
- `history.pushState`/`replaceState` may not change the origin.
- Script navigation is limited to `http(s)` and the self-test page, eight in
  a row; `javascript:` and `data:` are rejected by the engine.
- The `$262` test hook exists only when a test runner enables it.

**Enforced by the kernel** (holds even against beak itself):

- Every network call checks `NET`; handles (`npk_http_*`, `npk_tls_*`) are
  answered only to the process that opened them, and a process's jobs and
  sockets are released when it ends.
- **Reach** (`kernel/src/intent/reach.rs`). Addresses are classed `Local`
  (0/8, 127/8, 169.254/16), `Private` (10/8, 172.16/12, 192.168/16,
  100.64/10) or `Public`. A request may go to the same or a more open
  class than the document, never to a more closed one. beak reports the
  document URL with `npk_net_context`; the kernel resolves it and keeps
  only the class, defaulting to `Public` when it cannot. The check runs on
  the resolved address of every hop of `npk_http_begin`, every host of a
  batch and every `npk_tls_connect`, and the connection then goes to that
  checked address (a pooled one only if its peer is that address). beak sets the context to the target
  before a navigation, so a navigation may go anywhere while the page's own
  requests stay within its class.
- Header and method hygiene, redirect rules and plaintext policy as under
  Networking.
- Storage: without `READ`/`WRITE`, beak reads only `priv/beak/` and writes
  only there and to its own settings file `sys/config/beak`.

## Tabs

Up to 10 tabs. One `Engine` and one set of fetch buffers serve the active
tab; a background tab is frozen to what restores it (URL, address text,
history, scroll position, title) and refetched when it becomes active
again. Freezing cancels its requests and drops its JS session, so script
state does not survive a switch. Each tab has its own back/forward history
of up to 64 entries. `npk_open` on a running beak opens a new tab.

## Input and events

| Input | Effect |
|---|---|
| Ctrl+L | focus the address field |
| Ctrl+T, Ctrl+W, Ctrl+1 to 8, Ctrl+9 | new tab, close tab, that tab, the last tab |
| Ctrl+F | find bar (Enter next, Escape close) |
| Ctrl+C | copy the page selection (`npk_clipboard_set`) |
| wheel; arrows, Page Up/Down, Space, Home, End | scroll; the keys only when no page control has focus |
| left click | page handlers first; then a control, a link (followed on release), `<summary>`, or the start of a text selection |
| middle click on a link | open it in a background tab |

Page events dispatched by the shell: `mousedown`, `mouseup`, `click`,
`keydown`, `input`, `change`, `focus`/`blur`/`focusin`/`focusout`, `submit`,
`scroll`, `resize`, `DOMContentLoaded`, `load`, `pageshow`. Listeners
support capture, `once`, `passive` and `signal`; `preventDefault` on a
click stops beak from following the link. Pointer, scroll and resize
events are dispatched only when the page has registered a listener.

Form controls are painted into the canvas and edited by beak itself
(`FormState` holds the user's edits). Submission encodes the successful
controls as `application/x-www-form-urlencoded`: GET replaces the action's
query, POST sends them as the body.

`:hover` restyles the page when its last full layout took at most 250 ms;
a slower page gets no hover.

## Host functions

| Function | Gate |
|---|---|
| `npk_scene_commit`, `npk_event_poll`, `npk_canvas_rect`, `npk_close_widget` | `RENDER` |
| `npk_canvas_commit` | `CANVAS` |
| `npk_clipboard_set` | `RENDER` and window focus |
| `npk_http_begin` | `NET`; method and header checks; reach; -2 when the caller's jobs fill the table (beak queues the fetch for the next round) |
| `npk_http_begin_many`, `npk_http_begin_many_hdr` | `NET`; cookie line check; reach |
| `npk_http_poll`, `npk_http_take`, `npk_http_take_many`, `npk_http_cancel` | owning process |
| `npk_http_status`, `npk_http_response_headers`, `npk_http_final_url`, `npk_http_content_type`, `npk_http_last_error`, `npk_net_context` | `NET` |
| `npk_tls_connect`, `npk_tls_send`, `npk_tls_recv`, `npk_tls_close` | `NET`; owning process; reach on connect |
| `npk_random_bytes` | none; at most 64 KiB per call |
| `npk_fetch`, `npk_store` | own private area `priv/beak/` (`npk_store` also `sys/config/beak`) |
| `npk_launch_arg`, `npk_sleep`, `npk_ticks`, `npk_unix_time`, `npk_log_serial` | none checked in `host_core.rs` |

`npk_http_take` returns the body for any status (`npk_http_status` has
it). `npk_tls_connect` blocks for the handshake; `npk_tls_recv` returns at
once (0 = nothing yet, -1 = closed).

## Testing on the host

`cargo test` in `tools/wasm/beak-engine` runs the unit tests, including
the kernel's `reach.rs`, which the crate mounts into its test tree. Suite
runs and their numbers: [`CONFORMANCE.md`](CONFORMANCE.md).

`tests/`:

- `wpt.rs`: renders the vendored reftests in `tests/wpt/` and their
  references, compares pixels against `wpt-baseline.tsv`.
- `test262.rs`: parse oracle and execution run (`TEST262=<checkout>`).
- `vehicles.py`: splits the WPT result into its two denominators.
- `apigap.rs`: DOM members real pages call that the engine lacks, ranked
  (`APICENSUS=<file>`).
- `gap.rs`: which declared properties of a real page win, and whether the
  engine implements them.
- `fixtures.rs`: every class a fixture uses has a rule in its stylesheet.
- `diag.rs`: display list of one reftest and its reference (`DIAG=`).

`examples/` (`cargo run --release --example <name>`):

- Scripts: `selftest` (the shipped self-test page), `jsrun` (one JS file,
  prints the console), `pagerun` (a page's script round), `wallcheck`
  (where shipped scripts die first), `firstfail` (first failure in a script
  corpus), `predict2` (do scripts change the tree visibly).
- DOM and events: `domtest` (binding), `e2e` (tree change reaches the
  picture), `evt` (click handler gets painted), `hitchk` (click finds its
  target), `hoverchk` (hit boxes vs. `:hover`), `ctlpaint` (control
  repaint equals full layout).
- Layout and paint: `pagedump`, `opdump` (draw commands of a file, a
  snippet), `boxprobe` (boxes), `pagerender` (page to BMP), `cmpcheck`
  (per Bootstrap component).
- Cost: `laytime`, `gcstime` (layout, `getComputedStyle`), `heapcheck`
  (heap per component), `realmcost` (realm size and release).
- Decoders and protocols: `imgcheck` (why an image was dropped),
  `woffcheck` (WOFF against TTF), `ligcheck` (ligature icons), `wsreal`
  (WebSocket framing, live), `bigmath` (bignum vs. node), `rejs`, `retest`
  (RegExp with and without the engine).

Dev-only features of `beak-engine`: `diag-boxes`, `strict-probe`,
`heap-census` (needs `--test-threads=1`), `miss-census`.
