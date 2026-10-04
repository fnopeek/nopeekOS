# `tools/wasm/beak-engine/src/picture.rs` @ 5e0102684

## L1-10 · `use crate::css::Media;`

```
//! Responsive images: `<picture>`/`<source>` selection and `srcset` (HTML
//! §4.8.4.3).
//!
//! Resolved as a DOM pass, once, right after parsing: the winning candidate is
//! folded into the `<img>`'s own `src`/`width`/`height`. Everything downstream
//! — `image_srcs` (what the shell fetches), `img_box` (how big the box is),
//! the `DrawOp::Image` that carries the src — keeps reading a plain `<img
//! src>` and needs no changes. It also guarantees the shell fetches exactly
//! the URL layout will ask for, which two independent selection sites could
//! not.
```

## L17-18 · `pub fn resolve(dom: &mut Dom, media: Media) {`

```
/// Fold every `<picture>`'s active `<source>` — and any bare `<img srcset>` —
/// into the `<img>` element itself.
```

## L34-36 · `fn apply_picture(pic: &mut Element, media: Media) {`

```
/// A `<picture>`: the first `<source>` whose `media` matches and whose `type`
/// we can actually decode wins; the `<img>` is the fallback. Nothing matching
/// leaves the `<img>` exactly as authored.
```

## L44-46 · `if let Some(t) = e.attr("type") {`

```
// A `type` we cannot decode has to be SKIPPED, not taken and then
// failed: taking an `image/webp` source would replace a picture that
// renders today with one that renders nothing.
```

## L67 · `for c in &mut pic.children {`

```
// No `<source>` won — the `<img>` may still carry its own `srcset`.
```

## L83-85 · `if let Some(w) = &w {`

```
// A `<source>`'s own `width`/`height` are the image's dimensions when
// that source is used — that is the whole point of the wide-viewport
// variant (Wikipedia's footer swaps a 25×25 icon for an 84×29 button).
```

## L95-98 · `fn apply_img_srcset(img: &mut Element, media: Media) {`

```
/// `srcset` on a bare `<img>`: only used when there is no `src` to fall back
/// on, or when the chosen candidate is a different URL at 1x. Density
/// candidates above 1x are deliberately NOT taken — we render at 1x, and
/// fetching the 2x asset would double the bytes for no visible gain.
```

## L118 · `fn decodable_type(t: &str) -> bool {`

```
/// The formats `image::decode` handles. Anything else must not be selected.
```

## L121-135 · `matches!(t.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/svg+xml")`

```
// `image/webp` is deliberately NOT here, even though `webp::decode` exists
// since 0.40.0. Two measured reasons, both against taking it:
//
// 1. The type says nothing about lossy vs lossless. We decode `VP8 ` only,
//    so an `image/webp` source could still be a `VP8L` we have to decline —
//    and by then the `<img>` fallback is already gone. That is the exact
//    trade the skip below was written for.
// 2. It costs more. Under wasmi the same picture is 597 instructions per
//    pixel as WebP against 220 as JPEG (beakbench, 2026-08-25). The
//    `<picture>` markup on the corpus pairs every webp source with a JPEG
//    one-for-one (236 : 236), so declining loses no image at all — it just
//    takes the cheaper of two encodings of the same photo.
//
// The decoder earns its keep on the OTHER case: a bare `<img src="…webp">`
// with no fallback, which is 87 of 118 images on srf.ch.
```

## L139-146 · `fn srcset_candidates(srcset: &str) -> Vec<(&str, Option<&str>)> {`

```
/// Split a `srcset` into `(url, descriptor)` candidates — HTML "parse a srcset
/// attribute".
///
/// The separator is WHITESPACE, not the comma: a URL is a run of non-whitespace
/// characters, and only a comma that ends that run (or follows the descriptor)
/// starts the next candidate. That is what makes a `data:` URI work, since its
/// commas sit inside an unbroken run — splitting on ',' cuts it in half and
/// hands the tail on as a URL of its own.
```

## L155 · `let end = rest`

```
// The URL runs to the next whitespace.
```

## L160 · `let trimmed = url.trim_end_matches(',');`

```
// A URL ending in commas takes no descriptor (spec step 5).
```

## L167 · `let dend = tail.find(',').unwrap_or(tail.len());`

```
// Otherwise the descriptors run to the next comma.
```

## L176-179 · `fn pick<'a>(srcset: &'a str, sizes: Option<&str>, viewport: f32) -> Option<&'a str> {`

```
/// Pick one candidate out of a `srcset`. Width (`Nw`) candidates resolve
/// against `sizes` — defaulting to the viewport, as the spec does — and the
/// narrowest one that still covers it wins. Density (`Nx`) candidates resolve
/// at 1x.
```

## L206 · `return Some(best.map(|(_, u)| u).unwrap_or_else(|| {`

```
// Nothing covers the target → the largest available is the closest.
```

## L211 · `let mut best: Option<(f32, &str)> = None;`

```
// Exactly 1x if it exists, else the smallest above it, else the largest.
```

## L226-227 · `fn sizes_px(sizes: &str, viewport: f32) -> Option<f32> {`

```
/// The first length in a `sizes` list, which is the value that applies when no
/// media condition precedes it. `vw` resolves against the viewport.
```

## L251 · `let html = "<body><picture>\`

```
// Wikipedia's footer: a 25×25 icon below 500px, an 84×29 button above.
```

## L262 · `let mut dom = crate::dom::parse(html);`

```
// Below the breakpoint the fallback stands untouched.
```

## L272-273 · `let html = "<body><picture>\`

```
// Taking the webp would replace a picture that renders with one that
// renders nothing at all.
```

## L287 · `assert_eq!(pick("/b.png 2x, /c.png 3x", None, 800.0), Some("/b.png"));`

```
// No 1x at all → the smallest above it.
```

## L293-297 · `let uri = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iYSIvPg==";`

```
// DuckDuckGo's home page ships its logo as
// `<picture><source srcSet="data:image/svg+xml;base64,…">`. The commas
// inside a data: URI are not candidate separators — splitting on them
// handed the base64 tail on as a URL of its own, which no fetch can
// ever satisfy.
```

## L301 · `let set = alloc::format!("/a.png 1x, {uri} 2x");`

```
// Still one candidate among several.
```

## L310 · `assert_eq!(pick("/a.png,, /b.png 2x", None, 800.0), Some("/a.png"));`

```
// Trailing commas end the candidate and leave it descriptor-less …
```

## L312-313 · `assert_eq!(pick("/a,b.png 2x", None, 800.0), Some("/a,b.png"));`

```
// … while a comma INSIDE the run is just part of the URL, which is the
// whole reason a data: URI survives.
```

## L320 · `assert_eq!(pick(set, Some("600px"), 1600.0), Some("/m.jpg"));`

```
// Narrowest that still covers the target.
```

## L323 · `assert_eq!(pick(set, Some("2000px"), 1600.0), Some("/l.jpg"));`

```
// Nothing covers it → the largest.
```

## L325 · `assert_eq!(pick(set, None, 640.0), Some("/m.jpg"));`

```
// No `sizes` → the viewport is the target, as the spec defaults.
```

