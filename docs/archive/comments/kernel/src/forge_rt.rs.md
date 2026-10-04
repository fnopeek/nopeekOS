# `kernel/src/forge_rt.rs` @ 5e0102684

## L1-17 · `use crate::mm::paging::{self, PageFlags};`

```
//! Address space and mappings for compiled modules.
//!
//! Two kinds of memory, and both are decisions rather than plumbing:
//!
//! **Linear memory** gets a reservation of 8 GiB plus a page, of which only
//! the pages that exist are mapped. A wasm address is a u32 and a memory
//! offset is a u32, so no access can reach past that range — which is why the
//! generator emits no bounds check at all. The spare page is not slack: the
//! highest reachable address is `2^33-2`, and an eight-byte access THERE
//! reaches `2^33+5`.
//!
//! **Code** is mapped W^X — writable while it is being filled, executable
//! afterwards, never both. A kernel that carries a code generator has to be
//! able to say that much.
//!
//! Everything lives above the identity-mapped first 64 GB, where nothing else
//! claims addresses.
```

## L22-23 · `const REGION_BASE: u64 = 64 * 1024 * 1024 * 1024;`

```
/// First address above the identity map. Below this everything is mapped 1:1
/// through 1 GB huge pages, so this is where free address space starts.
```

## L26-27 · `const INSTANCE_STRIDE: u64 = 16 * 1024 * 1024 * 1024;`

```
/// What one instance reserves. A power of two well above `8 GiB + page` keeps
/// the arithmetic to a shift and leaves the next instance nowhere near.
```

## L30-31 · `pub const MAX_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024 + 0x1000;`

```
/// The readable part may never exceed this — the reservation minus the slack
/// an eight-byte access at the very top needs.
```

## L34-53 · `pub const MAX_INSTANCE_BYTES: u64 = 1024 * 1024 * 1024;`

```
/// Wieviel Arbeitsspeicher EIN Modul wirklich belegen darf.
///
/// **`MAX_MEMORY_BYTES` ist die Adressreservierung, kein Deckel** — sie sagt,
/// wie weit ein Platz reicht, nicht wieviel RAM er nehmen darf. Dazwischen
/// stand bis hierher nichts: ein Modul wuchs, bis die MASCHINE leer war, und
/// dann starb der Kernel an seiner eigenen naechsten Allokation. Gemessen am
/// 2026-09-13 an DuckDuckGos Ergebnisseite — beak hielt dort 2 GB, weil `Rc`
/// keine Ringe einsammelt und Reacts Fiberbaum einer ist. Im Log: Seitenfehler
/// auf `0xfffffffffffffffd` (das ist `null - 3`, eine benutzte
/// Fehlallokation), danach „capacity overflow" im Kernel, Halt.
///
/// **Ein Modul, das zuviel will, muss sterben; die Maschine nicht.** Das ist
/// dieselbe Grenze wie jede andere in diesem System: die Sandbox darf nicht
/// nach draussen wirken, und der Arbeitsspeicher der ganzen Maschine ist
/// draussen.
///
/// Die Zahl steht ueber dem gemessenen Normalfall, nicht darunter
/// ([[feedback_a_cap_set_from_a_guess_is_below_the_normal_case]]): eine
/// gewoehnliche Seite haelt in beak 44 bis 90 MiB, und beak ist das
/// hungrigste Modul, das es gibt. Ein Gigabyte ist das Zehnfache davon.
```

## L56-58 · `pub const KERNEL_RESERVE_MB: usize = 96;`

```
/// Was der Kernel fuer sich behaelt. Ohne diese Reserve gibt er den letzten
/// Rahmen an ein Modul und kann danach nicht einmal mehr die Absage
/// aufschreiben — genau das ist am 2026-09-13 passiert.
```

## L63-66 · `const MAX_SLOTS: u64 = 1024;`

```
/// Wieviele Plaetze es GIBT. Ein Platz ist `INSTANCE_STRIDE` breit, und
/// darueber faengt der Code an — Platz 1024 laege GENAU auf `CODE_BASE`.
/// Bis 0.117.0 stand hier kein Deckel: der 1025. Modullauf eines Bootes haette
/// sein lineares Gedaechtnis ueber den Maschinencode gelegt.
```

## L69 · `static NEXT_SLOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);`

```
/// Hoechster je vergebener Platz. Die Marke, unter der `SLOT_FREE` gilt.
```

## L72-74 · `static SLOT_FREE: [core::sync::atomic::AtomicU64; (MAX_SLOTS / 64) as usize] =`

```
/// Zurueckgegebene Plaetze, ein Bit je Platz (gesetzt = frei). Ein Feld statt
/// einer Liste: die Rueckgabe passiert in `Drop`, und dort darf nichts
/// allozieren.
```

## L78 · `fn take_slot() -> Option<u64> {`

```
/// Einen Platz nehmen — erst einen zurueckgegebenen, sonst den naechsten.
```

## L99 · `fn give_slot(s: u64) {`

```
/// Einen Platz zurueckgeben.
```

## L106 · `const CODE_BASE: u64 = REGION_BASE + 1024 * INSTANCE_STRIDE;`

```
/// Code goes at the top of the region, far from any instance's memory.
```

## L111 · `slot: u64,`

```
/// Der Platz in der Region — gehoert zurueckgegeben, wenn die Instanz geht.
```

## L114 · `pub size: u64,`

```
/// Bytes currently readable.
```

## L118 · `fn map_range(at: u64, bytes: u64, flags: PageFlags) -> bool {`

```
/// Map `bytes` of fresh, zeroed pages at `at`.
```

## L123-124 · `unmap_range(at, off);`

```
// Die Haelfte, die schon steht, geht zurueck. Sonst kostet gerade
// der Fall „kein Speicher mehr" noch einmal Speicher.
```

## L128-129 · `unsafe { core::ptr::write_bytes(frame as *mut u8, 0, PAGE as usize) };`

```
// SAFETY: the frame allocator just handed this out and the first
// 64 GB are identity-mapped, so the frame is addressable here.
```

## L141-147 · `fn unmap_range(at: u64, bytes: u64) {`

```
/// Abbildung loesen und die Rahmen zurueckgeben.
///
/// **Das Gegenstueck zu `map_range`, und bis 0.117.0 gab es keins.** Jeder
/// Modullauf behielt sein lineares Gedaechtnis und seinen Maschinencode bis
/// zum Neustart — bei beak 20 MB beim Start plus alles, was seine Halde
/// waehrend eines Laufes dazunahm. Ein zweiter Start fand danach keine Rahmen
/// mehr, und das Log sagte nur „Instanz liess sich nicht bauen".
```

## L159-162 · `pub fn new(initial_pages: u64) -> Option<Memory> {`

```
/// Reserve an instance's address space and map its initial pages.
///
/// Only the mapping is done here — the rest of the 8 GiB stays absent, and
/// that absence IS the bounds check.
```

## L179-183 · `pub fn owns(&self, addr: u64) -> bool {`

```
// **Hier stand ein zweites `grow`.** Es hielt `self.size` richtig und
// wurde von NIEMANDEM gerufen; gewachsen ist die Instanz ueber
// `forge_rt::grow`, das nur den vmctx schreibt. Zwei Wege fuer dieselbe
// Groesse, und gepflegt hat sie der tote — deshalb gibt es jetzt nur noch
// einen, und `Drop for Instance` holt die Groesse dort ab, wo sie steht.
```

## L185-186 · `pub fn owns(&self, addr: u64) -> bool {`

```
/// Does `addr` fall inside this instance's reservation? What a page-fault
/// handler asks to tell a module's mistake from a kernel's.
```

## L193-198 · `fn drop(&mut self) {`

```
/// **Die Seiten gehen zurueck — so viele, wie `self.size` sagt.**
///
/// Und `self.size` ist NICHT von selbst der Stand von jetzt: `memory.grow`
/// laeuft im generierten Code und schreibt die neue Groesse in den vmctx,
/// nicht hierher. Wer sie aktuell haelt, ist `Drop for Instance` — dort
/// steht auch, was es gekostet hat, dass es die Zeile nicht gab.
```

## L205 · `pub struct Code {`

```
/// A module's code, mapped executable and not writable.
```

## L212-214 · `pub fn map(bytes: &[u8]) -> Option<Code> {`

```
/// Copy `bytes` into fresh pages and flip them to execute-only-ish
/// (readable and executable, not writable). Writable and executable are
/// never true at the same time.
```

## L220 · `let rw = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;`

```
// Writable first, so it can be filled.
```

## L225 · `unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), base as *mut u8, len) };`

```
// SAFETY: `span` bytes were just mapped writable at `base`.
```

## L228 · `let rx = PageFlags::PRESENT;`

```
// Then executable, and no longer writable.
```

## L250 · `fn span(&self) -> u64 { ((self.len as u64) + PAGE - 1) & !(PAGE - 1) }`

```
/// So viele Bytes stehen wirklich — `map` rundet auf ganze Seiten auf.
```

## L255-257 · `fn drop(&mut self) {`

```
/// Auch der Maschinencode ist geliehen. Der Adressraum darueber wird
/// NICHT zurueckgedreht (`NEXT_CODE` laeuft weiter) — er ist 48 Bit breit
/// und kostet nichts; die Rahmen kosten.
```

## L263-274 · `#[unsafe(no_mangle)]`

```
// ── faults ────────────────────────────────────────────────────────────
//
// A page fault from a guard page and a divide fault are how two of the traps
// arrive: they are the processor telling us what a check on every single
// access would otherwise have had to look for. Catching them means pointing
// the interrupted instruction pointer at the module's entry for that reason —
// and because the entry names the reason itself, no general register has to be
// touched. That is the whole reason the entries exist per reason.
//
// The stubs below are the smallest thing that can do it: one register saved,
// two comparisons, and either a redirect or a jump to the handler that was
// there before. A fault outside a module's code is still a kernel fault.
```

## L289 · `pub fn arm_faults(code: &Code, pf_entry: usize, de_entry: usize) {`

```
/// Which code may fault, and where its faults go. Cleared with `disarm`.
```

## L304-306 · `core::arch::global_asm!(`

```
// The saved instruction pointer sits one word above our own push, plus another
// word for #PF's error code. Nothing but `rax` is touched, and `iretq` puts
// the flags back from the frame.
```

## L349-362 · `core::arch::global_asm!(`

```
// ── ein Trap aus einer Host-Funktion heraus ───────────────────────────
//
// Eine wasi-Funktion wie `proc_exit` DARF nicht zurueckkehren — auch bei
// sauberem Ende nicht. Der Interpreter macht daraus ein `Err` und rollt ab;
// erzeugter Code braucht denselben Weg, und den gibt es: die Trap-Routine des
// Moduls stellt `rsp`/`rbp` wieder her und springt zum Eintritt zurueck,
// wodurch jede Tiefe von wasm-Rahmen in vier Befehlen verschwindet.
//
// Sie braucht dafuer nur den vmctx-Zeiger — und den hat jede Host-Funktion in
// `rdi`. Deshalb genuegt hier dieselbe Sequenz, statt r14 anzufassen.
//
// Der Host-Zwilling in `forge/harness` kann das NICHT: dort ist jeder Lauf ein
// eigener Prozess, und `proc_exit` beendet ihn einfach. Im Kernel gibt es
// nichts zu beenden, also muss abgerollt werden.
```

## L379 · `fn forge_host_trap(vm: *const u64, code: u64) -> !;`

```
/// Verlaesst das Modul mit `code` als Trap-Grund. Kehrt nie zurueck.
```

## L383-392 · `pub unsafe fn host_trap(vm: *const u64, code: u32) -> ! {`

```
/// Aus einer Host-Funktion heraus den Lauf beenden.
///
/// # Safety
/// `vm` muss der vmctx der GERADE laufenden Instanz sein — der Zeiger, den der
/// Adapter als erstes Argument bekommen hat. Mit einem fremden vmctx springt
/// das in einen Rahmen, den es nicht mehr gibt.
///
/// Der Aufrufer darf nichts halten, was aufgeraeumt werden muss: hier wird
/// kein Rust-Rahmen abgewickelt, kein `Drop` laeuft. Das ist dieselbe
/// Zusicherung, unter der auch der `#PF`-Vorschalter arbeitet.
```

## L394 · `unsafe { forge_host_trap(vm, code as u64) }`

```
// SAFETY: an den Aufrufer weitergereicht, siehe oben.
```

## L400 · `use alloc::vec::Vec;`

```
// ── instances ─────────────────────────────────────────────────────────
```

## L405-408 · `extern "C" fn grow(ctx: *mut u64, delta: u32) -> u32 {`

```
/// `memory.grow`, the one runtime routine generated code calls. It never moves
/// the base — the reservation is already there, and only its readable part
/// changes. Generated code depends on that, which is why nothing reloads the
/// memory register after a call.
```

## L410-411 · `unsafe {`

```
// SAFETY: `ctx` is the instance context of the module doing the call,
// laid out by `forge_core::vmctx`.
```

## L426-427 · `if new_pages * 65536 > MAX_INSTANCE_BYTES {`

```
// Der Deckel des MODULS, und darunter die Reserve der MASCHINE.
// Beides ist eine Absage an das Modul, keine Panik im Kernel.
```

## L445-448 · `let (frames, mb) = crate::memory::stats();`

```
// **Ein „nein" ohne Grund kostet eine Stunde.** Das Modul
// meldet nur, dass `memory.grow` abgelehnt hat; ob die
// MASCHINE leer ist oder die Abbildung an dieser Adresse
// scheiterte, sieht man nur von hier aus.
```

## L462-466 · `pub trait HostImports {`

```
/// What an embedder has to answer so a compiled module can call out.
///
/// Deliberately two questions and no types: `forge_rt` stays free of anything
/// npk-specific, and the table it fills is plain addresses. The npk side of
/// this lives in `wasm::forge_glue`.
```

## L468-469 · `fn ctx_ptr(&self) -> u64;`

```
/// The embedder state a host function will be handed, as a raw address.
/// Parked in the vmctx, so two modules on two cores never share one.
```

## L471 · `fn resolve(&self, module: &str, name: &str) -> Option<u64>;`

```
/// Address of the routine for one import, or `None` to leave it trapping.
```

## L475-476 · `pub struct Instance {`

```
/// A module made ready to run: its code mapped, its memory reserved, and the
/// context generated code reaches everything else through.
```

## L478-479 · `unresolved: u32,`

```
/// Imports still pointing at the trap stub. Zero means the module can
/// actually run; anything else means it will stop at the first call out.
```

## L494-495 · `pub fn new(m: &CompiledModule) -> Option<Instance> {`

```
/// Without a host: every import lands on the trap stub. That is what the
/// selftest cases want — they import nothing.
```

## L500-503 · `pub fn new_with_host(m: &CompiledModule, host: &dyn HostImports) -> Option<Instance> {`

```
/// With a host: imports the embedder knows get its addresses, the rest
/// keep trapping. A module is never half-wired without saying so —
/// `unresolved_imports` counts what stayed on the stub.
///
```

## L524 · `unsafe {`

```
// SAFETY: the range was just checked against the mapped size.
```

## L545 · `globals.push(0); // never hand out a null base`

```
// never hand out a null base
```

## L567-568 · `let mut host_fns: Vec<u64> = Vec::new();`

```
// An import the embedder does not know keeps the trap stub, so a module
// that needs one says so instead of jumping somewhere arbitrary.
```

## L614-615 · `pub fn memory_size(&self) -> u64 {`

```
/// Groesse der linearen Speichers in Bytes, wie sie gerade im vmctx steht.
/// Waechst mit `memory.grow`, also nach dem Lauf ein anderer Wert als davor.
```

## L628-630 · `pub fn call(&mut self, off: usize, a: u32, b: u32, c: u32) -> (u32, u32) {`

```
/// Enter the module at `off` and come back with what it produced and what
/// stopped it. Faults from this code are claimed for the duration and
/// released again — outside that window a page fault is the kernel's own.
```

## L635-637 · `let r = unsafe {`

```
// SAFETY: both addresses come from the module's own tables, the code
// is mapped executable, and the trampoline's shape is fixed by
// `forge_core::codegen::emit_entry`.
```

## L649-668 · `fn drop(&mut self) {`

```
/// **Was `memory.grow` dazugelegt hat, gehoert mit zurueck.**
///
/// Der Wachstumspfad ist der GENERIERTE Code: er ruft `forge_rt::grow`,
/// und die schreibt die neue Groesse in den vmctx — nicht in das
/// `Memory`, das die Seiten spaeter wieder freigibt. `Memory::drop`
/// loeste deshalb genau die Abbildung, die beim START stand, und alles,
/// was die Halde des Moduls waehrend des Laufs dazunahm, blieb bis zum
/// Neustart liegen.
///
/// Bei beak sind das je Sitzung schnell hundert Megabyte. Nach ein paar
/// besuchten Seiten fand das naechste `memory.grow` keine Rahmen mehr —
/// und beak starb an seiner ERSTEN Erweiterung, mit einer Halde, die noch
/// auf ihrer Startgroesse von 318 Seiten stand. Genau diese Zahl im Log
/// war der Hinweis: sie ist die Startgroesse aus dem Binaerbild, also war
/// nicht diese Sitzung zu gross, sondern die vorigen waren nicht weg.
///
/// **Zwei Wege fuer dieselbe Groesse, und gepflegt hat sie der TOTE.**
/// `Memory::grow` hielt `self.size` richtig und wurde von niemandem
/// gerufen; er ist deshalb weg. Jetzt gibt es einen Weg zu wachsen und
/// eine Stelle, die die Groesse zurueckholt.
```

## L671-673 · `let now = self.ctx[vmctx::MEM_SIZE as usize / 8];`

```
// Nie kleiner als beim Start und nie ueber die Reservierung
// hinaus: `unmap_range` laeuft Seite fuer Seite, und ein zu
// grosser Wert griffe in den Platz der naechsten Instanz.
```

## L677-679 · `if m.size > start {`

```
// Gewachsen ist der seltene Fall — und der, der frueher liegen
// blieb. Er gehoert EINMAL ins Log, mit Zahlen: ohne das ist
// „die Rahmen kommen zurueck" eine Behauptung.
```

