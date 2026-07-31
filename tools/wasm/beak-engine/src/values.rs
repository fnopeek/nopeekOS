//! values.rs — CSS `<length>` / `<percentage>` / `calc()` resolution.
//!
//! One place that turns a length token into pixels, given the resolution
//! context (font-relative bases, the percentage basis, and the viewport for
//! `vw`/`vh`/`vmin`/`vmax`). Supports absolute + font-relative + viewport units
//! and `calc()` with `+ - * /` and nesting. Host-testable, no OS.
//!
//! Pure `core`/`alloc`, `f32` throughout — the whole thing unit-tests on the
//! dev box with no target in the loop.

use core::str;

/// Context for resolving a length to px.
#[derive(Clone, Copy)]
pub struct LenCtx {
    /// Current element font-size (for `em`).
    pub em: f32,
    /// Root font-size (for `rem`).
    pub rem: f32,
    /// Basis a `%` resolves against (e.g. containing-block width).
    pub pct_basis: f32,
    /// Viewport width in px (for `vw`/`vmin`/`vmax`).
    pub vw: f32,
    /// Viewport height in px (for `vh`/`vmin`/`vmax`).
    pub vh: f32,
}

/// Resolve a CSS `<length>`/`<percentage>`/`calc(...)` value to pixels.
/// `None` if the token is unparseable (caller keeps its prior value).
///
/// A top-level value is treated as a `calc()`-style expression, so a bare
/// `16px`, a `50%`, and a full `calc(100% - 3rem)` all go down one path.
/// The whole input must be consumed (trailing garbage → `None`), the result
/// must be finite, and division by zero → `None`.
pub fn resolve_length(v: &str, ctx: &LenCtx) -> Option<f32> {
    let s = v.trim();
    if s.is_empty() {
        return None;
    }
    let mut p = Parser {
        b: s.as_bytes(),
        i: 0,
        ctx,
    };
    let val = p.parse_expr()?;
    p.skip_ws();
    // Reject trailing garbage: the entire token must be consumed.
    if p.i != p.b.len() {
        return None;
    }
    if !val.is_finite() {
        return None;
    }
    Some(val)
}

/// Turn a numeric magnitude + unit suffix into pixels against `ctx`.
/// Unit is matched case-insensitively; `""`/`"px"` and a bare number are px.
/// Unknown unit → `None`.
fn resolve_unit(n: f32, unit: &str, ctx: &LenCtx) -> Option<f32> {
    // No unit → px (also covers the `0` case).
    if unit.is_empty() {
        return Some(n);
    }
    let eq = |u: &str| unit.eq_ignore_ascii_case(u);
    let px = if eq("px") {
        n
    } else if eq("em") {
        n * ctx.em
    } else if eq("rem") {
        n * ctx.rem
    } else if eq("%") {
        n / 100.0 * ctx.pct_basis
    } else if eq("vw") {
        n / 100.0 * ctx.vw
    } else if eq("vh") {
        n / 100.0 * ctx.vh
    } else if eq("vmin") {
        n / 100.0 * fmin(ctx.vw, ctx.vh)
    } else if eq("vmax") {
        n / 100.0 * fmax(ctx.vw, ctx.vh)
    } else if eq("pt") {
        // 1pt = 1/72in, 1in = 96px.
        n * (96.0 / 72.0)
    } else if eq("pc") {
        // 1pc = 12pt = 16px.
        n * 16.0
    } else if eq("in") {
        n * 96.0
    } else if eq("cm") {
        n * (96.0 / 2.54)
    } else if eq("mm") {
        n * (96.0 / 25.4)
    } else if eq("q") {
        // 1Q = 1/4 mm = 96/25.4/4 px ≈ 0.944882px.
        n * (96.0 / 25.4 / 4.0)
    } else if eq("ex") || eq("ch") {
        // Approximation: no glyph metrics here, so ex/ch ≈ 0.5em.
        n * 0.5 * ctx.em
    } else {
        return None;
    };
    Some(px)
}

#[inline]
fn fmin(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn fmax(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

/// Recursive-descent evaluator over the byte string. Everything resolves to a
/// px `f32` at the leaves, so arithmetic is plain float math (the spec's
/// "one side of `*`/`/` must be a number" rule is relaxed — dimensional
/// checking is not enforced).
///
/// Grammar:
/// ```text
///   expr   := term  (('+' | '-') term)*
///   term   := factor (('*' | '/') factor)*
///   factor := '(' expr ')'
///           | 'calc' '(' expr ')'
///           | ('min' | 'max') '(' expr (',' expr)* ')'
///           | 'clamp' '(' expr ',' expr ',' expr ')'
///           | ('+' | '-') factor          (unary sign)
///           | value                       (number + optional unit)
/// ```
struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    ctx: &'a LenCtx,
}

impl<'a> Parser<'a> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            // CSS whitespace: space, tab, LF, CR, form feed.
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == 0x0c {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn parse_expr(&mut self) -> Option<f32> {
        let mut acc = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    acc += self.parse_term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    acc -= self.parse_term()?;
                }
                _ => break,
            }
        }
        Some(acc)
    }

    fn parse_term(&mut self) -> Option<f32> {
        let mut acc = self.parse_factor()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.i += 1;
                    acc *= self.parse_factor()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let r = self.parse_factor()?;
                    if r == 0.0 {
                        return None; // division by zero
                    }
                    acc /= r;
                }
                _ => break,
            }
        }
        Some(acc)
    }

    fn parse_factor(&mut self) -> Option<f32> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let v = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(b')') {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            Some(b'+') => {
                // Unary plus.
                self.i += 1;
                self.parse_factor()
            }
            Some(b'-') => {
                // Unary minus.
                self.i += 1;
                Some(-self.parse_factor()?)
            }
            _ => {
                // A nested math function, else a numeric value leaf.
                if self.match_fn(b"calc") {
                    let v = self.parse_expr()?;
                    self.close_paren()?;
                    Some(v)
                } else if self.match_fn(b"min") {
                    self.parse_fold(fmin)
                } else if self.match_fn(b"max") {
                    self.parse_fold(fmax)
                } else if self.match_fn(b"clamp") {
                    let lo = self.parse_expr()?;
                    self.comma()?;
                    let val = self.parse_expr()?;
                    self.comma()?;
                    let hi = self.parse_expr()?;
                    self.close_paren()?;
                    // clamp(MIN, VAL, MAX) == max(MIN, min(VAL, MAX)).
                    Some(fmax(lo, fmin(val, hi)))
                } else {
                    self.parse_value()
                }
            }
        }
    }

    /// Fold a comma-separated argument list (already past the opening paren)
    /// through `f`, consuming the closing paren. `min()`/`max()` are variadic.
    fn parse_fold(&mut self, f: fn(f32, f32) -> f32) -> Option<f32> {
        let mut acc = self.parse_expr()?;
        loop {
            self.skip_ws();
            if self.peek() != Some(b',') {
                break;
            }
            self.i += 1;
            acc = f(acc, self.parse_expr()?);
        }
        self.close_paren()?;
        Some(acc)
    }

    fn comma(&mut self) -> Option<()> {
        self.skip_ws();
        if self.peek() != Some(b',') {
            return None;
        }
        self.i += 1;
        Some(())
    }

    fn close_paren(&mut self) -> Option<()> {
        self.skip_ws();
        if self.peek() != Some(b')') {
            return None;
        }
        self.i += 1;
        Some(())
    }

    /// If the input at the cursor is `<name>(` (name case-insensitive),
    /// consume through the opening paren and return true.
    fn match_fn(&mut self, name: &[u8]) -> bool {
        let rest = &self.b[self.i..];
        let n = name.len();
        if rest.len() > n
            && rest[n] == b'('
            && rest[..n]
                .iter()
                .zip(name)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            self.i += n + 1;
            true
        } else {
            false
        }
    }

    /// A number (optional decimal) followed by an optional unit suffix.
    /// Sign is handled by `parse_factor` (unary), so only `+`-less magnitudes
    /// with an optional leading `.` land here.
    fn parse_value(&mut self) -> Option<f32> {
        let start = self.i;
        let mut seen_digit = false;
        let mut seen_dot = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                seen_digit = true;
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if !seen_digit {
            return None;
        }
        // Bytes are all ASCII digits/dot → valid UTF-8, parseable as f32.
        let num_str = str::from_utf8(&self.b[start..self.i]).ok()?;
        let n: f32 = num_str.parse().ok()?;

        // Unit: `%` or a run of ASCII letters.
        let ustart = self.i;
        if self.peek() == Some(b'%') {
            self.i += 1;
        } else {
            while let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() {
                    self.i += 1;
                } else {
                    break;
                }
            }
        }
        let unit = str::from_utf8(&self.b[ustart..self.i]).ok()?;
        resolve_unit(n, unit, self.ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> LenCtx {
        LenCtx {
            em: 16.0,
            rem: 16.0,
            pct_basis: 1000.0,
            vw: 1280.0,
            vh: 800.0,
        }
    }

    /// Assert `resolve_length(v)` ≈ `expect`.
    fn approx(v: &str, expect: f32) {
        let got = resolve_length(v, &ctx())
            .unwrap_or_else(|| panic!("`{v}` resolved to None, expected {expect}"));
        assert!(
            (got - expect).abs() < 0.01,
            "`{v}` = {got}, expected {expect}"
        );
    }

    #[test]
    fn bare_and_px() {
        approx("16px", 16.0);
        approx("0", 0.0);
        approx("42", 42.0);
        approx("  24px  ", 24.0); // trimmed
        approx(".5px", 0.5);
        approx("+8px", 8.0);
        approx("-8px", -8.0);
    }

    #[test]
    fn font_relative() {
        approx("1.5rem", 24.0);
        approx("2em", 32.0);
        approx(".5rem", 8.0);
        approx("1EM", 16.0); // case-insensitive
    }

    #[test]
    fn percentage() {
        approx("50%", 500.0);
        approx("100%", 1000.0);
        approx("0%", 0.0);
        approx("12.5%", 125.0);
    }

    #[test]
    fn viewport() {
        approx("10vw", 128.0); // 10% of 1280
        approx("10vh", 80.0); // 10% of 800
        approx("100vw", 1280.0);
        approx("10vmin", 80.0); // 10% of min(1280,800)=800
        approx("10vmax", 128.0); // 10% of max(1280,800)=1280
        approx("50vmin", 400.0);
    }

    #[test]
    fn absolute_units() {
        approx("12pt", 16.0); // 12 * 96/72
        approx("1in", 96.0);
        approx("1pc", 16.0);
        approx("2.54cm", 96.0);
        approx("25.4mm", 96.0);
        approx("40q", 96.0 / 25.4 / 4.0 * 40.0); // 40Q = 10mm
    }

    #[test]
    fn ex_ch_approx() {
        approx("1ex", 8.0); // 0.5em
        approx("2ch", 16.0); // 2 * 0.5em
    }

    #[test]
    fn calc_add_sub() {
        approx("calc(100% - 3rem)", 952.0); // 1000 - 48
        approx("calc(1.5rem + 2px)", 26.0); // 24 + 2
        approx("calc(10px + 20px + 30px)", 60.0);
        approx("calc(50% - 10%)", 400.0); // 500 - 100
        approx("CALC(16px + 16px)", 32.0); // case-insensitive fn name
    }

    #[test]
    fn calc_mul_div() {
        approx("calc(2 * 10px)", 20.0);
        approx("calc(10px * 2)", 20.0);
        approx("calc(100px / 4)", 25.0);
        approx("calc(3rem / 2)", 24.0); // 48/2
    }

    #[test]
    fn calc_precedence() {
        approx("calc(10px + 2 * 5px)", 20.0); // mul before add
        approx("calc(2 * 5px + 10px)", 20.0);
        approx("calc(20px - 6px / 2)", 17.0); // 20 - 3
    }

    #[test]
    fn calc_parens_and_nesting() {
        approx("calc(2 * (10px + 5px))", 30.0);
        approx("calc((100% - 200px) / 2)", 400.0); // (1000-200)/2
        approx("calc(1px + calc(2px + 3px))", 6.0); // nested calc()
        approx("calc(2 * calc(10px + 5px))", 30.0);
    }

    #[test]
    fn calc_unary_and_whitespace() {
        approx("calc( 100%  -  3rem )", 952.0); // loose inner whitespace
        approx("calc(1rem - -2px)", 18.0); // unary minus operand
        approx("calc(-3rem + 100%)", 952.0); // leading negative
    }

    #[test]
    fn min_max_clamp() {
        // ctx(): em=16, rem=16, pct_basis=1000, vw=800, vh=600.
        approx("min(10px, 2rem)", 10.0);
        approx("max(10px, 2rem)", 32.0);
        approx("min(50%, 100px, 3rem)", 48.0); // variadic
        approx("MAX(1px, 2px)", 2.0); // case-insensitive
        approx("clamp(10px, 5%, 30px)", 30.0); // 50 clamped down to the max
        approx("clamp(10px, 1px, 30px)", 10.0); // below the min
        approx("clamp(10px, 20px, 30px)", 20.0); // inside the range
        // Nested with calc(), and as an operand of one.
        approx("calc(max(calc(1rem + 2px), 10px) * 2)", 36.0);
        approx("min(calc(100% - 900px), 200px)", 100.0);
        approx("max(-3px, -1px)", -1.0);
    }

    #[test]
    fn min_max_clamp_invalid() {
        let c = ctx();
        assert_eq!(resolve_length("min()", &c), None);
        assert_eq!(resolve_length("max(1px,)", &c), None);
        assert_eq!(resolve_length("clamp(1px, 2px)", &c), None); // wrong arity
        assert_eq!(resolve_length("clamp(1px, 2px, 3px, 4px)", &c), None);
        assert_eq!(resolve_length("min(1px", &c), None); // unbalanced
        assert_eq!(resolve_length("min (1px, 2px)", &c), None); // space before paren
    }

    #[test]
    fn invalid_returns_none() {
        let c = ctx();
        assert_eq!(resolve_length("", &c), None);
        assert_eq!(resolve_length("   ", &c), None);
        assert_eq!(resolve_length("auto", &c), None);
        assert_eq!(resolve_length("16pxx", &c), None); // bad unit
        assert_eq!(resolve_length("16 px", &c), None); // space splits number/unit
        assert_eq!(resolve_length("px", &c), None); // no number
        assert_eq!(resolve_length("16px 32px", &c), None); // two values
        assert_eq!(resolve_length("1.2.3px", &c), None); // malformed number
        assert_eq!(resolve_length("calc(1px +)", &c), None); // dangling op
        assert_eq!(resolve_length("calc(1px + 2px", &c), None); // unbalanced paren
        assert_eq!(resolve_length("calc()", &c), None); // empty calc
        assert_eq!(resolve_length("calc(foo)", &c), None); // junk operand
    }

    #[test]
    fn div_by_zero_returns_none() {
        let c = ctx();
        assert_eq!(resolve_length("calc(10px / 0)", &c), None);
        assert_eq!(resolve_length("calc(10px / (2 - 2))", &c), None);
        assert_eq!(resolve_length("calc(5px / 0.0)", &c), None);
    }

    #[test]
    fn does_not_panic_on_garbage() {
        // Never unwrap/panic on adversarial input — just return None or a value.
        let c = ctx();
        let _ = resolve_length("calc(((((", &c);
        let _ = resolve_length(")))))", &c);
        let _ = resolve_length("calc(* / +)", &c);
        let _ = resolve_length("...", &c);
        let _ = resolve_length("%%%%", &c);
        let _ = resolve_length("calc(1px * * 2px)", &c);
        let _ = resolve_length("-", &c);
        let _ = resolve_length("+", &c);
    }
}
