# `kernel/src/security/csprng.rs` @ 5e0102684

## L1-5 · `use spin::Mutex;`

```
//! Cryptographically Secure PRNG (ChaCha20-based)
//!
//! Replaces xorshift128+ for capability tokens and all security-sensitive randomness.
//! Seeded from RDRAND (hardware RNG) if available, TSC fallback.
//! Re-keys every 64 blocks for forward secrecy.
```

## L23 · `rng.refill();`

```
// Discard first block (defense against weak seeds)
```

## L33 · `if self.counter % 64 == 0 {`

```
// Re-key every 64 blocks for forward secrecy
```

## L57-62 · `fn fill(&mut self, out: &mut [u8]) {`

```
/// Beliebig viele Bytes am Stueck — der Strom, nicht acht Bytes davon.
///
/// Blockweise aus demselben Puffer wie `next_u64`, ohne den Umweg ueber
/// 64-Bit-Worte: `crypto.getRandomValues` darf bis 65 536 Bytes auf
/// einmal verlangen, und die Wortschleife waere dafuer achtmal so viele
/// Grenzpruefungen.
```

## L84 · `fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {`

```
// === ChaCha20 core (RFC 7539) ===
```

## L96 · `s[0] = 0x61707865; s[1] = 0x3320646e;`

```
// "expand 32-byte k"
```

## L113 · `quarter_round(&mut s, 0, 4,  8, 12);`

```
// Column rounds
```

## L118 · `quarter_round(&mut s, 0, 5, 10, 15);`

```
// Diagonal rounds
```

## L133 · `fn has_rdrand() -> bool {`

```
// === Seeding ===
```

## L137-138 · `unsafe {`

```
// SAFETY: CPUID is always available on x86_64. rbx is saved/restored
// because LLVM uses it internally.
```

## L156 · `unsafe {`

```
// SAFETY: RDRAND is available (checked by has_rdrand)
```

## L179 · `for i in 0..4 {`

```
// Hardware RNG: best entropy source
```

## L181 · `for _ in 0..10 {`

```
// Retry up to 10 times per word
```

## L190 · `let t1 = rdtsc();`

```
// Fallback: TSC + constants (NOT ideal but better than nothing)
```

## L204 · `pub fn init() {`

```
// === Public API ===
```

## L227 · `pub fn random_256() -> [u8; 32] {`

```
/// Generate a 256-bit random value for capability tokens (post-quantum safe).
```

## L237-244 · `pub fn fill(out: &mut [u8]) {`

```
/// Einen Puffer mit Zufall fuellen — die Quelle hinter
/// `crypto.getRandomValues` einer Seite.
///
/// **Derselbe ChaCha20-Strom wie fuer Kapabilitaetsmarken**, aus RDRAND
/// geseedet und alle 64 Bloecke neu verschluesselt. Eine zweite, schwaechere
/// Quelle daneben waere genau die Falle: eine Seite baut daraus
/// Sitzungsmarken, und „reicht schon" ist dort keine Aussage, die jemand
/// nachpruefen kann.
```

