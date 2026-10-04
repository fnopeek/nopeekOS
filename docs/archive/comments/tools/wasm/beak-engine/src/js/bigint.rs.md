# `tools/wasm/beak-engine/src/js/bigint.rs` @ 5e0102684

## L1-14 · `use alloc::string::String;`

```
//! Ganze Zahlen ohne Groessengrenze — der Zahlentyp hinter `BigInt`.
//!
//! **Selbst geschrieben, nicht geholt.** Die Engine ist `no_std` und liegt in
//! einem WASM-Modul; eine Bignum-Kiste dafuer zu ziehen kostet mehr als diese
//! Datei, und gebraucht wird nur, was die Sprache verlangt.
//!
//! Betrag als `Vec<u32>` in aufsteigender Wertigkeit, Vorzeichen daneben.
//! Null hat einen LEEREN Betrag und ist nie negativ — ohne diese Normalform
//! gaebe es zwei Nullen, und `0n === -0n` waere falsch.
//!
//! Die Division ist binaer (schieben und abziehen) statt nach Knuth D. Sie
//! ist damit um einen Faktor langsamer und um zwei Groessenordnungen kuerzer;
//! fuer die Zahlen, die auf einer Seite vorkommen, ist das der richtige
//! Handel.
```

## L23 · `pub mag: Vec<u32>,`

```
/// Aufsteigende Wertigkeit, ohne fuehrende Nullen. Leer = 0.
```

## L49-50 · `pub fn from_f64(v: f64) -> Option<Big> {`

```
/// Aus einem `f64`. Nur GANZE, endliche Zahlen — alles andere ist ein
/// RangeError beim Rufer.
```

## L56-57 · `let mut mag = Vec::new();`

```
// In 32-Bit-Stuecken herunterbrechen. `a / 2^32` ist exakt, solange
// `a` ganz ist: der Exponent sinkt, die Mantisse bleibt.
```

## L73 · `pub fn to_u64_wrap(&self) -> u64 {`

```
/// Passt sie in einen `u64`? Fuer die 64-Bit-Sichten.
```

## L81 · `fn cmp_mag(a: &[u32], b: &[u32]) -> core::cmp::Ordering {`

```
// ── Vergleich ────────────────────────────────────────────────────────
```

## L101 · `fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {`

```
// ── Betragsrechnung ──────────────────────────────────────────────────
```

## L114 · `fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {`

```
/// `a - b`, und `a >= b` wird vorausgesetzt.
```

## L202-203 · `pub fn div_rem(&self, o: &Big) -> Option<(Big, Big)> {`

```
/// Ganzzahlige Division mit Rest, ABGESCHNITTEN zur Null hin — so, wie
/// die Spezifikation es fuer `/` und `%` verlangt.
```

## L209 · `if o.mag.len() == 1 {`

```
// Ein einwortiger Teiler ist der haeufige Fall und geht direkt.
```

## L223 · `let n = self.bits();`

```
// Binaer: von oben nach unten ein Bit anhaengen und abziehen, wo es geht.
```

## L248-250 · `if self.bits() as f64 * n > 1_000_000.0 { return None; }`

```
// Ein Ergebnis jenseits von etwa einer Million Bit ist kein Rechnen
// mehr, sondern ein Aufhaenger. Die Spezifikation erlaubt hier
// ausdruecklich einen RangeError.
```

## L262-266 · `fn twos(&self, words: usize) -> Vec<u32> {`

```
// ── Bitweise: im ZWEIERKOMPLEMENT, unendlich fortgesetzt ─────────────
//
// `-1n & 0xffn` ist 255n, nicht 0n — das geht nur, wenn die negative
// Zahl als unendlich viele Einsen nach oben gedacht wird. Also wird auf
// die gemeinsame Laenge plus ein Wort gerechnet.
```

## L315-316 · `pub fn shr(&self, n: u64) -> Big {`

```
/// Arithmetisches Verschieben nach rechts: bei einer negativen Zahl wird
/// ABGERUNDET, nicht abgeschnitten (`-3n >> 1n` ist `-2n`).
```

## L323 · `let mut lost = false;`

```
// Ging etwas verloren, eins abziehen.
```

## L329 · `pub fn as_n(&self, bits: u64, signed: bool) -> Big {`

```
/// `BigInt.asIntN` / `asUintN`.
```

## L335 · `let extra = (words as u64 * 32 - bits) as u32;`

```
// Die Bits ueber `bits` streichen.
```

## L342 · `let top = ((bits - 1) % 32) as u32;`

```
// Das oberste behaltene Bit ist das Vorzeichen.
```

## L346 · `if extra > 0 { let last = v.len() - 1; v[last] |= !(u32::MAX >> extra); }`

```
// Nach oben mit Einsen auffuellen und als Zweierkomplement lesen.
```

## L354 · `pub fn to_string_radix(&self, radix: u32) -> String {`

```
// ── Text ─────────────────────────────────────────────────────────────
```

## L378-379 · `pub fn parse(t: &str) -> Option<Big> {`

```
/// Aus dem Quelltext (`123n`) oder aus `BigInt("…")`. `None` heisst
/// SyntaxError beim Rufer.
```

## L391 · `if radix != 10 && neg { return None; }`

```
// Ein Vorzeichen vor einer Basis gibt es nicht.
```

