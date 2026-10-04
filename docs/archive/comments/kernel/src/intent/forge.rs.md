# `kernel/src/intent/forge.rs` @ 5e0102684

## L1-7 · `use crate::{kprint, kprintln};`

```
//! `forge` — translate a module to machine code, and say what it cost.
//!
//! The compiler runs on the device long before anything it produces is
//! executed there. That order is deliberate: translating exercises the
//! allocator, the parser and the whole generator under the kernel's own
//! `no_std` conditions, and a failure there says something quite different
//! from a failure while running. One thing at a time.
```

## L12 · `fn ms() -> u64 {`

```
/// Milliseconds since boot, from the tick counter.
```

## L17-30 · `const TRAP_PROBE: &[u8] = &[`

```
/// Compile the embedded modules, run them, and compare against what the same
/// compiler produced on the development machine.
///
/// The expectations in `forge_tests.rs` were not written by hand: each one was
/// measured there, by a generator whose output is checked against the
/// interpreter case by case. So this asks a sharper question than "did it
/// work" — it asks whether the device agrees with the host, down to the trap
/// codes.
/// Ein Modul von 70 Bytes, das genau eine Sache tut: einen Import rufen und
/// danach 99 zurueckgeben. Die 99 darf NIE herauskommen — der Import verlaesst
/// den Lauf ueber `forge_rt::host_trap`, und wenn das Abrollen nicht stimmt,
/// sagt es genau hier Bescheid statt spaeter in python.
///
/// Von Hand erzeugt (Typen, ein Import, ein Export "f", drei Instruktionen).
```

## L41-42 · `unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }`

```
// SAFETY: `vm` ist der vmctx der laufenden Instanz, den der Generator als
// erstes Argument uebergibt. Kehrt nicht zurueck.
```

## L49 · `0`

```
// Der Stumpf fasst den Zustand nicht an; ein Zeiger waere gelogen.
```

## L58-63 · `fn trap_probe() -> bool {`

```
/// Kann eine Host-Funktion den Lauf beenden, statt zurueckzukehren?
///
/// Das ist der eine Mechanismus, fuer den der Host-Zwilling keine Antwort hat:
/// dort ist jeder Lauf ein eigener Prozess und `proc_exit` beendet ihn einfach.
/// Im Kernel muss abgerollt werden, und ein Assembler-Stub, der `rsp` und `rbp`
/// wiederherstellt, gehoert geprueft, bevor python davon abhaengt.
```

## L91 · `if got == 99 {`

```
// Kaeme 99 heraus, waere die Host-Funktion zurueckgekehrt statt zu traps.
```

## L161 · `super::python::intent_python_forge(rest.trim_start(), vault, session);`

```
// `forge python -c "..."` — derselbe Lauf, anderer Motor.
```

## L174-176 · `if let Some(rest) = name.strip_prefix("default") {`

```
// Der Schalter, der einen Neustart uebersteht. `dock`, `bar`, `audio_hda`
// und `wifid` startet niemand von Hand — die kommen ueber Autostart und
// den Treiberweg, und nur so lassen sie sich unter forge pruefen.
```

## L240-241 · `let tenths = if instrs > 0 { m.code.len() as u64 * 10 / instrs } else { 0 };`

```
// Tenths, printed with the point where it belongs — the value is around
// eight, and "84" reads like a different number entirely.
```

## L262-265 · `let imports = m.plan.imported_funcs.len();`

```
// Uebersetzen ist das eine, hinauskommen das andere. Ein Import, den die
// Bruecke nicht kennt, behaelt den Trap-Stumpf — das Modul wuerde beim
// ersten Aufruf stehenbleiben, nicht falsch rechnen. Also hier zaehlen,
// wo es noch niemandem weh tut.
```

