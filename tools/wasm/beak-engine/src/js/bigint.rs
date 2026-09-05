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

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Big {
    pub neg: bool,
    /// Aufsteigende Wertigkeit, ohne fuehrende Nullen. Leer = 0.
    pub mag: Vec<u32>,
}

impl Big {
    pub fn zero() -> Big { Big { neg: false, mag: Vec::new() } }
    pub fn is_zero(&self) -> bool { self.mag.is_empty() }

    fn norm(mut self) -> Big {
        while self.mag.last() == Some(&0) { self.mag.pop(); }
        if self.mag.is_empty() { self.neg = false; }
        self
    }

    pub fn from_u64(v: u64) -> Big {
        let mut m = vec![(v & 0xffff_ffff) as u32, (v >> 32) as u32];
        while m.last() == Some(&0) { m.pop(); }
        Big { neg: false, mag: m }
    }

    pub fn from_i64(v: i64) -> Big {
        let neg = v < 0;
        let m = Big::from_u64(v.unsigned_abs());
        Big { neg: neg && !m.is_zero(), mag: m.mag }
    }

    /// Aus einem `f64`. Nur GANZE, endliche Zahlen — alles andere ist ein
    /// RangeError beim Rufer.
    pub fn from_f64(v: f64) -> Option<Big> {
        if !v.is_finite() || libm::trunc(v) != v { return None; }
        if v == 0.0 { return Some(Big::zero()); }
        let neg = v < 0.0;
        let mut a = libm::fabs(v);
        // In 32-Bit-Stuecken herunterbrechen. `a / 2^32` ist exakt, solange
        // `a` ganz ist: der Exponent sinkt, die Mantisse bleibt.
        let mut mag = Vec::new();
        while a >= 1.0 {
            let rem = libm::fmod(a, 4294967296.0);
            mag.push(rem as u32);
            a = libm::floor(a / 4294967296.0);
        }
        Some(Big { neg, mag }.norm())
    }

    pub fn to_f64(&self) -> f64 {
        let mut r = 0.0f64;
        for w in self.mag.iter().rev() { r = r * 4294967296.0 + *w as f64; }
        if self.neg { -r } else { r }
    }

    /// Passt sie in einen `u64`? Fuer die 64-Bit-Sichten.
    pub fn to_u64_wrap(&self) -> u64 {
        let lo = *self.mag.first().unwrap_or(&0) as u64;
        let hi = *self.mag.get(1).unwrap_or(&0) as u64;
        let v = lo | (hi << 32);
        if self.neg { v.wrapping_neg() } else { v }
    }

    // ── Vergleich ────────────────────────────────────────────────────────
    fn cmp_mag(a: &[u32], b: &[u32]) -> core::cmp::Ordering {
        use core::cmp::Ordering::*;
        if a.len() != b.len() { return if a.len() < b.len() { Less } else { Greater }; }
        for k in (0..a.len()).rev() {
            if a[k] != b[k] { return if a[k] < b[k] { Less } else { Greater }; }
        }
        Equal
    }

    pub fn cmp(&self, o: &Big) -> core::cmp::Ordering {
        use core::cmp::Ordering::*;
        match (self.neg, o.neg) {
            (false, true) => Greater,
            (true, false) => Less,
            (false, false) => Big::cmp_mag(&self.mag, &o.mag),
            (true, true) => Big::cmp_mag(&o.mag, &self.mag),
        }
    }

    // ── Betragsrechnung ──────────────────────────────────────────────────
    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for k in 0..a.len().max(b.len()) {
            let s = carry + *a.get(k).unwrap_or(&0) as u64 + *b.get(k).unwrap_or(&0) as u64;
            out.push((s & 0xffff_ffff) as u32);
            carry = s >> 32;
        }
        if carry > 0 { out.push(carry as u32); }
        out
    }

    /// `a - b`, und `a >= b` wird vorausgesetzt.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for k in 0..a.len() {
            let mut d = a[k] as i64 - borrow - *b.get(k).unwrap_or(&0) as i64;
            if d < 0 { d += 1 << 32; borrow = 1; } else { borrow = 0; }
            out.push(d as u32);
        }
        out
    }

    pub fn add(&self, o: &Big) -> Big {
        if self.neg == o.neg {
            Big { neg: self.neg, mag: Big::add_mag(&self.mag, &o.mag) }.norm()
        } else if Big::cmp_mag(&self.mag, &o.mag) == core::cmp::Ordering::Less {
            Big { neg: o.neg, mag: Big::sub_mag(&o.mag, &self.mag) }.norm()
        } else {
            Big { neg: self.neg, mag: Big::sub_mag(&self.mag, &o.mag) }.norm()
        }
    }

    pub fn negate(&self) -> Big {
        if self.is_zero() { return Big::zero(); }
        Big { neg: !self.neg, mag: self.mag.clone() }
    }

    pub fn sub(&self, o: &Big) -> Big { self.add(&o.negate()) }

    pub fn mul(&self, o: &Big) -> Big {
        if self.is_zero() || o.is_zero() { return Big::zero(); }
        let mut out = vec![0u32; self.mag.len() + o.mag.len()];
        for (i, &x) in self.mag.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &y) in o.mag.iter().enumerate() {
                let t = out[i + j] as u64 + x as u64 * y as u64 + carry;
                out[i + j] = (t & 0xffff_ffff) as u32;
                carry = t >> 32;
            }
            let mut k = i + o.mag.len();
            while carry > 0 {
                let t = out[k] as u64 + carry;
                out[k] = (t & 0xffff_ffff) as u32;
                carry = t >> 32;
                k += 1;
            }
        }
        Big { neg: self.neg != o.neg, mag: out }.norm()
    }

    pub fn bits(&self) -> usize {
        match self.mag.last() {
            None => 0,
            Some(w) => (self.mag.len() - 1) * 32 + (32 - w.leading_zeros() as usize),
        }
    }

    fn bit(&self, n: usize) -> bool {
        let w = n / 32;
        w < self.mag.len() && (self.mag[w] >> (n % 32)) & 1 == 1
    }

    fn shl_words_bits(mag: &[u32], n: usize) -> Vec<u32> {
        if mag.is_empty() { return Vec::new(); }
        let (w, b) = (n / 32, n % 32);
        let mut out = vec![0u32; w];
        let mut carry = 0u32;
        for &x in mag {
            out.push((x << b) | carry);
            carry = if b == 0 { 0 } else { x >> (32 - b) };
        }
        if carry > 0 { out.push(carry); }
        out
    }

    fn shr_words_bits(mag: &[u32], n: usize) -> Vec<u32> {
        let (w, b) = (n / 32, n % 32);
        if w >= mag.len() { return Vec::new(); }
        let src = &mag[w..];
        let mut out = Vec::with_capacity(src.len());
        for k in 0..src.len() {
            let lo = src[k] >> b;
            let hi = if b == 0 { 0 } else { src.get(k + 1).map(|x| x << (32 - b)).unwrap_or(0) };
            out.push(lo | hi);
        }
        out
    }

    /// Ganzzahlige Division mit Rest, ABGESCHNITTEN zur Null hin — so, wie
    /// die Spezifikation es fuer `/` und `%` verlangt.
    pub fn div_rem(&self, o: &Big) -> Option<(Big, Big)> {
        if o.is_zero() { return None; }
        if Big::cmp_mag(&self.mag, &o.mag) == core::cmp::Ordering::Less {
            return Some((Big::zero(), self.clone()));
        }
        // Ein einwortiger Teiler ist der haeufige Fall und geht direkt.
        if o.mag.len() == 1 {
            let d = o.mag[0] as u64;
            let mut q = vec![0u32; self.mag.len()];
            let mut rem = 0u64;
            for k in (0..self.mag.len()).rev() {
                let cur = (rem << 32) | self.mag[k] as u64;
                q[k] = (cur / d) as u32;
                rem = cur % d;
            }
            let quo = Big { neg: self.neg != o.neg, mag: q }.norm();
            let r = Big { neg: self.neg, mag: if rem == 0 { Vec::new() } else { vec![rem as u32] } }.norm();
            return Some((quo, r));
        }
        // Binaer: von oben nach unten ein Bit anhaengen und abziehen, wo es geht.
        let n = self.bits();
        let mut q = vec![0u32; self.mag.len()];
        let mut rem = Big::zero();
        for k in (0..n).rev() {
            rem.mag = Big::shl_words_bits(&rem.mag, 1);
            if self.bit(k) {
                if rem.mag.is_empty() { rem.mag.push(1); }
                else { rem.mag[0] |= 1; }
            }
            rem = rem.norm();
            if Big::cmp_mag(&rem.mag, &o.mag) != core::cmp::Ordering::Less {
                rem.mag = Big::sub_mag(&rem.mag, &o.mag);
                rem = rem.norm();
                q[k / 32] |= 1 << (k % 32);
            }
        }
        let quo = Big { neg: self.neg != o.neg, mag: q }.norm();
        let r = Big { neg: self.neg, mag: rem.mag }.norm();
        Some((quo, r))
    }

    pub fn pow(&self, e: &Big) -> Option<Big> {
        if e.neg { return None; }
        let mut n = e.to_f64();
        // Ein Ergebnis jenseits von etwa einer Million Bit ist kein Rechnen
        // mehr, sondern ein Aufhaenger. Die Spezifikation erlaubt hier
        // ausdruecklich einen RangeError.
        if self.bits() as f64 * n > 1_000_000.0 { return None; }
        let mut base = self.clone();
        let mut acc = Big::from_u64(1);
        while n > 0.0 {
            if libm::fmod(n, 2.0) == 1.0 { acc = acc.mul(&base); }
            n = libm::floor(n / 2.0);
            if n > 0.0 { base = base.mul(&base); }
        }
        Some(acc)
    }

    // ── Bitweise: im ZWEIERKOMPLEMENT, unendlich fortgesetzt ─────────────
    //
    // `-1n & 0xffn` ist 255n, nicht 0n — das geht nur, wenn die negative
    // Zahl als unendlich viele Einsen nach oben gedacht wird. Also wird auf
    // die gemeinsame Laenge plus ein Wort gerechnet.
    fn twos(&self, words: usize) -> Vec<u32> {
        let mut v = vec![0u32; words];
        for k in 0..words.min(self.mag.len()) { v[k] = self.mag[k]; }
        if self.neg {
            for x in v.iter_mut() { *x = !*x; }
            let mut carry = 1u64;
            for x in v.iter_mut() {
                let t = *x as u64 + carry;
                *x = (t & 0xffff_ffff) as u32;
                carry = t >> 32;
                if carry == 0 { break }
            }
        }
        v
    }

    fn from_twos(v: Vec<u32>) -> Big {
        let neg = v.last().map(|w| w >> 31 == 1).unwrap_or(false);
        if !neg { return Big { neg: false, mag: v }.norm(); }
        let mut m = v;
        for x in m.iter_mut() { *x = !*x; }
        let mut carry = 1u64;
        for x in m.iter_mut() {
            let t = *x as u64 + carry;
            *x = (t & 0xffff_ffff) as u32;
            carry = t >> 32;
            if carry == 0 { break }
        }
        Big { neg: true, mag: m }.norm()
    }

    pub fn bitop(&self, o: &Big, op: u8) -> Big {
        let w = self.mag.len().max(o.mag.len()) + 1;
        let a = self.twos(w);
        let b = o.twos(w);
        let mut out = Vec::with_capacity(w);
        for k in 0..w {
            out.push(match op { 0 => a[k] & b[k], 1 => a[k] | b[k], _ => a[k] ^ b[k] });
        }
        Big::from_twos(out)
    }

    pub fn not(&self) -> Big { self.negate().sub(&Big::from_u64(1)) }

    pub fn shl(&self, n: u64) -> Big {
        Big { neg: self.neg, mag: Big::shl_words_bits(&self.mag, n as usize) }.norm()
    }

    /// Arithmetisches Verschieben nach rechts: bei einer negativen Zahl wird
    /// ABGERUNDET, nicht abgeschnitten (`-3n >> 1n` ist `-2n`).
    pub fn shr(&self, n: u64) -> Big {
        let n = n as usize;
        let m = Big::shl_words_bits(&[], 0);
        let _ = m;
        let r = Big { neg: self.neg, mag: Big::shr_words_bits(&self.mag, n) }.norm();
        if !self.neg { return r; }
        // Ging etwas verloren, eins abziehen.
        let mut lost = false;
        for k in 0..n.min(self.bits()) { if self.bit(k) { lost = true; break } }
        if lost { r.sub(&Big::from_u64(1)) } else { r }
    }

    /// `BigInt.asIntN` / `asUintN`.
    pub fn as_n(&self, bits: u64, signed: bool) -> Big {
        if bits == 0 { return Big::zero(); }
        let words = ((bits + 31) / 32) as usize;
        let mut v = self.twos(words + 1);
        v.truncate(words);
        // Die Bits ueber `bits` streichen.
        let extra = (words as u64 * 32 - bits) as u32;
        if extra > 0 {
            let last = v.len() - 1;
            v[last] &= u32::MAX >> extra;
        }
        if !signed { return Big { neg: false, mag: v }.norm(); }
        // Das oberste behaltene Bit ist das Vorzeichen.
        let top = ((bits - 1) % 32) as u32;
        let widx = ((bits - 1) / 32) as usize;
        if (v[widx] >> top) & 1 == 1 {
            // Nach oben mit Einsen auffuellen und als Zweierkomplement lesen.
            if extra > 0 { let last = v.len() - 1; v[last] |= !(u32::MAX >> extra); }
            v.push(u32::MAX);
            return Big::from_twos(v);
        }
        Big { neg: false, mag: v }.norm()
    }

    // ── Text ─────────────────────────────────────────────────────────────
    pub fn to_string_radix(&self, radix: u32) -> String {
        if self.is_zero() { return String::from("0"); }
        const DIG: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut digits = Vec::new();
        let mut cur = self.mag.clone();
        while !cur.is_empty() {
            let mut rem = 0u64;
            let mut q = vec![0u32; cur.len()];
            for k in (0..cur.len()).rev() {
                let v = (rem << 32) | cur[k] as u64;
                q[k] = (v / radix as u64) as u32;
                rem = v % radix as u64;
            }
            digits.push(DIG[rem as usize]);
            while q.last() == Some(&0) { q.pop(); }
            cur = q;
        }
        let mut s = String::new();
        if self.neg { s.push('-'); }
        for d in digits.iter().rev() { s.push(*d as char); }
        s
    }

    /// Aus dem Quelltext (`123n`) oder aus `BigInt("…")`. `None` heisst
    /// SyntaxError beim Rufer.
    pub fn parse(t: &str) -> Option<Big> {
        let t = t.trim();
        if t.is_empty() { return Some(Big::zero()); }
        let (neg, body) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let (radix, digits) = if let Some(r) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) { (16, r) }
            else if let Some(r) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) { (8, r) }
            else if let Some(r) = body.strip_prefix("0b").or_else(|| body.strip_prefix("0B")) { (2, r) }
            else { (10, body) };
        // Ein Vorzeichen vor einer Basis gibt es nicht.
        if radix != 10 && neg { return None; }
        if digits.is_empty() { return None; }
        let mut acc = Big::zero();
        let base = Big::from_u64(radix as u64);
        for c in digits.chars() {
            let d = c.to_digit(radix)?;
            acc = acc.mul(&base).add(&Big::from_u64(d as u64));
        }
        Some(if neg { acc.negate() } else { acc })
    }
}
