# `kernel/src/wasi/forge_glue.rs` @ 5e0102684

## L1-11 · `#![allow(clippy::too_many_arguments)]`

```
//! Die forge-Seite der wasi-ABI.
//!
//! Ein Adapter je Funktion, und jeder tut dasselbe wie sein wasmi-Zwilling in
//! `wasi.rs`: `mem` und `st` beschaffen und `calls::` rufen. Der Helfer dafuer
//! ist derselbe wie auf der npk-Seite — zwei Fassungen wuerden auseinander
//! laufen.
//!
//! `proc_exit` ist von Hand geschrieben und steht unten: es darf nicht
//! zurueckkehren, und genau darin unterscheiden sich die Motoren.
//!
//! DIESE DATEI IST ERZEUGT (bis auf `proc_exit` und `resolve`).
```

## L18 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L24 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L30 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L36 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L42 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L48 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L54 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L60 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L66 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L72 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L78 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L84 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L90 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L96 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L102 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L108 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L114 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L120 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L126 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L132 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L138 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L144 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L150 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L156 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L162 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L168 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L174 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L180 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L186 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L192 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L198 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L204 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L210 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L216 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L222 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L228 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L234 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L240 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L246 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L252 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L258 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L264 · `let (mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L269-275 · `extern "C" fn f_proc_exit(vm: *const u64, code: i32) -> i32 {`

```
/// Der eine Adapter, den kein Generator schreiben kann.
///
/// Ein wasi-Programm verlaesst sich IMMER hierueber, sauberes Ende
/// eingeschlossen. Der Interpreter macht daraus ein `Err` und rollt ab;
/// erzeugter Code nimmt die Trap-Routine des Moduls, die `rsp`/`rbp`
/// wiederherstellt und zum Eintritt zurueckspringt. Der Status wird VORHER
/// hinterlegt — im Trap-Code reist er nicht mit.
```

## L277 · `let (_mem, st) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L280-281 · `unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }`

```
// SAFETY: derselbe vmctx, und der Aufrufer haelt nichts, was aufgeraeumt
// werden muesste. Kehrt nicht zurueck.
```

## L285 · `pub(crate) fn resolve(module: &str, name: &str) -> Option<u64> {`

```
/// Adresse der Routine fuer einen wasi-Import, oder nichts.
```

