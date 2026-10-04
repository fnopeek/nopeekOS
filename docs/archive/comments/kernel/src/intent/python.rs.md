# `kernel/src/intent/python.rs` @ 5e0102684

## L1-8 · `use alloc::string::{String, ToString};`

```
//! `python` — run Python on the machine.
//!
//! The interpreter is an ordinary signed WASM module in `sys/wasm/`; the
//! standard library is an ordinary npkFS object in `sys/python/`. Nothing
//! about Python is special-cased in the kernel: it reaches the system
//! through the same `wasi_snapshot_preview1` grant any other wasi binary
//! would get, and the only Python-shaped thing here is which two
//! directories it gets handed.
```

## L18 · `const MODULE: &str = "sys/wasm/python";`

```
/// npkFS path of the interpreter module.
```

## L20 · `const BUNDLE: &str = "sys/python";`

```
/// npkFS directory holding `lib/python313.zip` + `lib/python3.13/os.py`.
```

## L23-30 · `const PYTHON_FUEL: u64 = 600_000_000_000;`

```
/// Fuel for one Python run.
///
/// Measured against this exact interpreter under this exact wasmi: a
/// bare `-c pass` costs 2.6 G, `import json,re,os` 4.0 G, and a
/// million-iteration Python loop 102 G. The 10 G that `run` grants would
/// therefore stop almost any real script mid-sentence. 600 G leaves room
/// for a script that actually computes something while still being a
/// ceiling — roughly a minute of device time, not an afternoon.
```

## L37-38 · `pub fn intent_python_forge(args: &str, vault: &'static spin::Mutex<capability::Vault>, session: capability::CapId) {`

```
/// Dasselbe unter forge. Eigener Eingang statt einer Fahne im python-Aufruf:
/// so laeuft genau EIN Lauf auf dem Compiler und alles andere wie bisher.
```

## L59-64 · `let home = home_dir();`

```
// What the guest will see. Two grants, both named, neither implicit:
//   /      the interpreter's own bundle, READ-ONLY
//   /home  the user's tree, writable
// A script outside home is refused rather than silently granted —
// widening the grant to reach one file is how a boundary stops
// meaning anything.
```

## L71-72 · `kprintln!("[python] usage: python <file.py> [args...]   |   python -c \"code\"");`

```
// No stdin reaches a wasi guest yet, so an interactive prompt
// would just sit there. Say so instead of hanging.
```

## L104-106 · `let env = vec![`

```
// PYTHONHOME because this interpreter was configured with the
// default prefix (/usr/local) and would look for its stdlib there.
// Building it with --prefix=/ would drop this line.
```

## L138-139 · `let ms = crate::interrupts::ticks().saturating_sub(t0) * 10;`

```
// Die Zeit gehoert zum Vergleich, nicht zum Lauf — deshalb steht sie hier
// und nicht im Motor, und beide Motoren werden gleich gemessen.
```

## L143-145 · `kprintln!("[heap] {} allocs / {} frees, Schritte: {} beim Belegen + {} beim Freigeben",`

```
// Die Karte des Allokators fuer genau diesen Lauf. `steps` sind besuchte
// Knoten der Freiliste — die Zahl, die sagt, ob die lineare Suche das
// Problem ist oder nicht.
```

