# `tools/wasm/beak-engine/src/values.rs` @ 5e0102684

## L1-9 · `use core::str;`

```
//! values.rs — CSS `<length>` / `<percentage>` / `calc()` resolution.
//!
//! One place that turns a length token into pixels, given the resolution
//! context (font-relative bases, the percentage basis, and the viewport for
//! `vw`/`vh`/`vmin`/`vmax`). Supports absolute + font-relative + viewport units
//! and `calc()` with `+ - * /` and nesting. Host-testable, no OS.
//!
//! Pure `core`/`alloc`, `f32` throughout — the whole thing unit-tests on the
//! dev box with no target in the loop.
```

## L13 · `#[derive(Clone, Copy)]`

```
/// Context for resolving a length to px.
```

## L16 · `pub em: f32,`

```
/// Current element font-size (for `em`).
```

## L18 · `pub rem: f32,`

```
/// Root font-size (for `rem`).
```

## L20 · `pub pct_basis: f32,`

```
/// Basis a `%` resolves against (e.g. containing-block width).
```

## L22 · `pub vw: f32,`

```
/// Viewport width in px (for `vw`/`vmin`/`vmax`).
```

## L24 · `pub vh: f32,`

```
/// Viewport height in px (for `vh`/`vmin`/`vmax`).
```

## L28-34 · `pub fn resolve_length(v: &str, ctx: &LenCtx) -> Option<f32> {`

```
/// Resolve a CSS `<length>`/`<percentage>`/`calc(...)` value to pixels.
/// `None` if the token is unparseable (caller keeps its prior value).
///
/// A top-level value is treated as a `calc()`-style expression, so a bare
/// `16px`, a `50%`, and a full `calc(100% - 3rem)` all go down one path.
/// The whole input must be consumed (trailing garbage → `None`), the result
/// must be finite, and division by zero → `None`.
```

## L47 · `if p.i != p.b.len() {`

```
// Reject trailing garbage: the entire token must be consumed.
```

## L57-59 · `fn resolve_unit(n: f32, unit: &str, ctx: &LenCtx) -> Option<f32> {`

```
/// Turn a numeric magnitude + unit suffix into pixels against `ctx`.
/// Unit is matched case-insensitively; `""`/`"px"` and a bare number are px.
/// Unknown unit → `None`.
```

## L61 · `if unit.is_empty() {`

```
// No unit → px (also covers the `0` case).
```

## L83 · `n * (96.0 / 72.0)`

```
// 1pt = 1/72in, 1in = 96px.
```

## L86 · `n * 16.0`

```
// 1pc = 12pt = 16px.
```

## L95 · `n * (96.0 / 25.4 / 4.0)`

```
// 1Q = 1/4 mm = 96/25.4/4 px ≈ 0.944882px.
```

## L100-101 · `n * crate::style::CH_PER_EM * ctx.em`

```
// NOT the same as `ex`: a `ch` is the "0" advance. Both were 0.5 here,
// which made every `ch` length 26 % too narrow — see `CH_PER_EM`.
```

## L127-142 · `struct Parser<'a> {`

```
/// Recursive-descent evaluator over the byte string. Everything resolves to a
/// px `f32` at the leaves, so arithmetic is plain float math (the spec's
/// "one side of `*`/`/` must be a number" rule is relaxed — dimensional
/// checking is not enforced).
///
/// Grammar:
/// `​``text
///   expr   := term  (('+' | '-') term)*
///   term   := factor (('*' | '/') factor)*
///   factor := '(' expr ')'
///           | 'calc' '(' expr ')'
///           | ('min' | 'max') '(' expr (',' expr)* ')'
///           | 'clamp' '(' expr ',' expr ',' expr ')'
///           | ('+' | '-') factor          (unary sign)
///           | value                       (number + optional unit)
/// `​``
```

## L157 · `if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == 0x0c {`

```
// CSS whitespace: space, tab, LF, CR, form feed.
```

## L198 · `return None; // division by zero`

```
// division by zero
```

## L222 · `self.i += 1;`

```
// Unary plus.
```

## L227 · `self.i += 1;`

```
// Unary minus.
```

## L232 · `if self.match_fn(b"calc") {`

```
// A nested math function, else a numeric value leaf.
```

## L248 · `Some(fmax(lo, fmin(val, hi)))`

```
// clamp(MIN, VAL, MAX) == max(MIN, min(VAL, MAX)).
```

## L257-258 · `fn parse_fold(&mut self, f: fn(f32, f32) -> f32) -> Option<f32> {`

```
/// Fold a comma-separated argument list (already past the opening paren)
/// through `f`, consuming the closing paren. `min()`/`max()` are variadic.
```

## L291-292 · `fn match_fn(&mut self, name: &[u8]) -> bool {`

```
/// If the input at the cursor is `<name>(` (name case-insensitive),
/// consume through the opening paren and return true.
```

## L310-312 · `fn parse_value(&mut self) -> Option<f32> {`

```
/// A number (optional decimal) followed by an optional unit suffix.
/// Sign is handled by `parse_factor` (unary), so only `+`-less magnitudes
/// with an optional leading `.` land here.
```

## L331 · `let num_str = str::from_utf8(&self.b[start..self.i]).ok()?;`

```
// Bytes are all ASCII digits/dot → valid UTF-8, parseable as f32.
```

## L335 · `let ustart = self.i;`

```
// Unit: `%` or a run of ASCII letters.
```

## L367 · `fn approx(v: &str, expect: f32) {`

```
/// Assert `resolve_length(v)` ≈ `expect`.
```

## L382 · `approx("  24px  ", 24.0); // trimmed`

```
// trimmed
```

## L393 · `approx("1EM", 16.0); // case-insensitive`

```
// case-insensitive
```

## L406 · `approx("10vw", 128.0); // 10% of 1280`

```
// 10% of 1280
```

## L407 · `approx("10vh", 80.0); // 10% of 800`

```
// 10% of 800
```

## L409 · `approx("10vmin", 80.0); // 10% of min(1280,800)=800`

```
// 10% of min(1280,800)=800
```

## L410 · `approx("10vmax", 128.0); // 10% of max(1280,800)=1280`

```
// 10% of max(1280,800)=1280
```

## L416 · `approx("12pt", 16.0); // 12 * 96/72`

```
// 12 * 96/72
```

## L421 · `approx("40q", 96.0 / 25.4 / 4.0 * 40.0); // 40Q = 10mm`

```
// 40Q = 10mm
```

## L425-428 · `fn ex_and_ch_use_the_real_font_metrics() {`

```
/// `ex` and `ch` are DIFFERENT: the x-height and the "0" advance. Both sat
/// at 0.5em, which made every `ch` length 26 % too narrow — a `width: 60ch`
/// text column came out at 47 characters. The factors are measured off our
/// own font, see `style::CH_PER_EM`.
```

## L437 · `approx("calc(100% - 3rem)", 952.0); // 1000 - 48`

```
// 1000 - 48
```

## L438 · `approx("calc(1.5rem + 2px)", 26.0); // 24 + 2`

```
// 24 + 2
```

## L440 · `approx("calc(50% - 10%)", 400.0); // 500 - 100`

```
// 500 - 100
```

## L441 · `approx("CALC(16px + 16px)", 32.0); // case-insensitive fn name`

```
// case-insensitive fn name
```

## L449 · `approx("calc(3rem / 2)", 24.0); // 48/2`

```
// 48/2
```

## L454 · `approx("calc(10px + 2 * 5px)", 20.0); // mul before add`

```
// mul before add
```

## L456 · `approx("calc(20px - 6px / 2)", 17.0); // 20 - 3`

```
// 20 - 3
```

## L462 · `approx("calc((100% - 200px) / 2)", 400.0); // (1000-200)/2`

```
// (1000-200)/2
```

## L463 · `approx("calc(1px + calc(2px + 3px))", 6.0); // nested calc()`

```
// nested calc()
```

## L469 · `approx("calc( 100%  -  3rem )", 952.0); // loose inner whitespace`

```
// loose inner whitespace
```

## L470 · `approx("calc(1rem - -2px)", 18.0); // unary minus operand`

```
// unary minus operand
```

## L471 · `approx("calc(-3rem + 100%)", 952.0); // leading negative`

```
// leading negative
```

## L476 · `approx("min(10px, 2rem)", 10.0);`

```
// ctx(): em=16, rem=16, pct_basis=1000, vw=800, vh=600.
```

## L479 · `approx("min(50%, 100px, 3rem)", 48.0); // variadic`

```
// variadic
```

## L480 · `approx("MAX(1px, 2px)", 2.0); // case-insensitive`

```
// case-insensitive
```

## L481 · `approx("clamp(10px, 5%, 30px)", 30.0); // 50 clamped down to the max`

```
// 50 clamped down to the max
```

## L482 · `approx("clamp(10px, 1px, 30px)", 10.0); // below the min`

```
// below the min
```

## L483 · `approx("clamp(10px, 20px, 30px)", 20.0); // inside the range`

```
// inside the range
```

## L484 · `approx("calc(max(calc(1rem + 2px), 10px) * 2)", 36.0);`

```
// Nested with calc(), and as an operand of one.
```

## L495 · `assert_eq!(resolve_length("clamp(1px, 2px)", &c), None); // wrong arity`

```
// wrong arity
```

## L497 · `assert_eq!(resolve_length("min(1px", &c), None); // unbalanced`

```
// unbalanced
```

## L498 · `assert_eq!(resolve_length("min (1px, 2px)", &c), None); // space before paren`

```
// space before paren
```

## L507 · `assert_eq!(resolve_length("16pxx", &c), None); // bad unit`

```
// bad unit
```

## L508 · `assert_eq!(resolve_length("16 px", &c), None); // space splits number/unit`

```
// space splits number/unit
```

## L509 · `assert_eq!(resolve_length("px", &c), None); // no number`

```
// no number
```

## L510 · `assert_eq!(resolve_length("16px 32px", &c), None); // two values`

```
// two values
```

## L511 · `assert_eq!(resolve_length("1.2.3px", &c), None); // malformed number`

```
// malformed number
```

## L512 · `assert_eq!(resolve_length("calc(1px +)", &c), None); // dangling op`

```
// dangling op
```

## L513 · `assert_eq!(resolve_length("calc(1px + 2px", &c), None); // unbalanced paren`

```
// unbalanced paren
```

## L514 · `assert_eq!(resolve_length("calc()", &c), None); // empty calc`

```
// empty calc
```

## L515 · `assert_eq!(resolve_length("calc(foo)", &c), None); // junk operand`

```
// junk operand
```

## L528 · `let c = ctx();`

```
// Never unwrap/panic on adversarial input — just return None or a value.
```

