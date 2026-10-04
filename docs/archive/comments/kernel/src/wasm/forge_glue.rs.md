# `kernel/src/wasm/forge_glue.rs` @ 5e0102684

## L1-14 · `use super::host_core;`

```
//! Die forge-Seite der Host-Bruecke.
//!
//! Ein Adapter je Host-Funktion, und jeder tut genau zwei Dinge: `mem` und
//! `ctx` aus dem vmctx holen und `host_core` rufen. Der wasmi-Adapter in
//! `wasm.rs` holt dieselben zwei anders. Alles darunter ist EINE
//! Implementierung — zwei Host-Schichten zu vergleichen wuerde die
//! Host-Schichten messen, nicht die Compiler.
//!
//! Die Aufrufform gibt der Generator vor: `rdi` = vmctx, ganzzahlige
//! Argumente ab `rsi`, Rueckgabe in `rax`. Das ist SysV, also passt
//! `extern "C"` ohne Zutun — auch fuer die drei mit mehr als fuenf
//! Argumenten, die dadurch ueber den Stapel gehen.
//!
//! DIESE DATEI IST ERZEUGT. Sie folgt den Signaturen in `host_core.rs`.
```

## L20-26 · `unsafe fn ctx_of<'a>(vm: *const u64) -> &'a mut HostState {`

```
/// Der Zustand, den diese Instanz gehoert. Der Zeiger steht im vmctx, nicht in
/// einem `static` — zwei Module auf zwei Kernen haetten sich einen `static`
/// geteilt.
///
/// # Safety
/// `vm` muss der vmctx einer Instanz sein, die ueber `NpkHost` gebaut wurde;
/// nur dann ist `HOST_CTX` ein gueltiger `HostState`.
```

## L31-36 · `pub(crate) unsafe fn parts<'a>(vm: *const u64) -> (&'a mut [u8], &'a mut HostState) {`

```
/// Gastspeicher und Zustand. Beide werden bei JEDEM Aufruf neu gelesen: die
/// Basis bewegt sich nie, aber `memory.grow` verschiebt das Ende.
///
/// # Safety
/// Wie `ctx_of`, und `MEM_BASE`/`MEM_SIZE` muessen die Reservierung dieser
/// Instanz beschreiben.
```

## L41-43 · `let mem = if base == 0 {`

```
// Ein Modul ohne Speicher bekommt eine LEERE Scheibe, keine mit
// Nullzeiger: `read_str` und die anderen antworten darauf schon mit
// "ausserhalb", also braucht kein Adapter einen Sonderfall.
```

## L53-56 · `#[allow(dead_code)]`

```
/// Was `forge_rt` fragen muss, um die Tabelle zu fuellen.
///
/// Steht bereit, faehrt aber noch niemand: den Ausfuehrungspfad gibt es erst,
/// wenn `install` uebersetzt und den Codeblob ablegt.
```

## L65-67 · `resolve(module, name).or_else(|| crate::wasi::forge_glue::resolve(module, name))`

```
// Ein Modul kann beide ABIs importieren — beak nur `env`, python nur
// wasi. Der Host beantwortet deshalb beide, statt dass der Aufrufer
// sich einen aussuchen muesste.
```

## L73 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L79 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L85 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L91 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L97 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L103 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L109 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L115 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L121 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L127 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L133 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L139 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L145 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L151 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L157 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L163 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L169 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L175 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L181 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L187 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L193 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L201 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L207 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L213 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L219 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L225 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L232 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L238 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L244 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L250 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L256 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L262 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L268 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L274 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L280 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L286 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L292 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L298 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L304 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L310 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L316 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L322 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L328 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L334 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L340 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L346 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L352 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L358 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L365 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L371 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L377 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L383 · `let (_mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L389 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L395 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L401 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L407 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L413 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L419 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L425 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L431 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L437 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L443 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L449 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L455 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L461 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L467 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L473 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L479 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L485 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L491 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L497 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L503 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L509 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L515 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L521 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L527 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L533 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L539 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L545 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L551 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L557 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L563 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L569 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L575 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L581 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L587 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L593 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L599 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L605 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L611 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L617 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L623 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L629 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L635 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L641 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L647 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L653 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L661 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L668 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L674 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L680 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L686 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L692 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L698 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L704 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L710 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L716 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L722 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L728 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L734 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L740 · `let (_mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L746 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L752 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L758 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L764 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L770 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L776 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L782 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L788 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L794 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L800 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L806 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L812 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L818 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L824 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L830 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L836 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L842 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L848 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L854 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L861 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L867 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L873 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L879 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L885 · `let ctx = unsafe { ctx_of(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L891 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L897 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L903 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L909 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L915 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L921 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L927 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L933 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L939 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L945 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L951 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L957 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L963 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L969 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L975 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L981 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L987 · `let (mem, ctx) = unsafe { parts(vm) };`

```
// SAFETY: `vm` ist der vmctx des rufenden Moduls.
```

## L992-993 · `pub(crate) fn resolve(module: &str, name: &str) -> Option<u64> {`

```
/// Adresse der Routine fuer einen Import, oder nichts — dann behaelt der
/// Schlitz den Trap-Stumpf und das Modul sagt beim ersten Aufruf Bescheid.
```

