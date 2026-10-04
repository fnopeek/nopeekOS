# `tools/wasm/top/src/lib.rs` @ 5e0102684

## L1-6 · `#![no_std]`

```
//! top — nopeekOS system monitor (WASM module)
//!
//! Live-updating display: per-core CPU usage, frequency, memory, scheduler.
//! Process tracking: running WASM apps with name, CPU%, memory, core.
//! Uses the App Display API: npk_print, npk_clear, npk_input_wait, npk_sys_info.
//! Press 'q' to exit.
```

## L18 · `#[link(wasm_import_module = "env")]`

```
// ── App Display API ──────────────────────────────────────────────
```

## L20-22 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

## L25 · `fn npk_print(ptr: i32, len: i32);`

```
/// Write text to the app's display area.
```

## L27 · `fn npk_clear();`

```
/// Clear the app's display.
```

## L29 · `fn npk_input_wait(timeout_ms: i32) -> i32;`

```
/// Wait for a key press or timeout. Returns key (0-255) or -1 (timeout).
```

## L31 · `fn npk_sys_info(key: i32) -> i64;`

```
/// Query system information.
```

## L70 · `fn unpack_name(val: i64, buf: &mut [u8], offset: usize) -> usize {`

```
/// Decode up to 8 bytes from a packed i64 into a buffer. Returns bytes written.
```

## L106 · `print("\n  nopeekOS top — ");`

```
// Header
```

## L127 · `let proc_count = sys(20);`

```
// Query process table
```

## L130 · `let mut core_has_proc = [false; 16];`

```
// Build core→process map via process table PIDs
```

## L132 · `let mut pids = [0i32; 32]; // cache PIDs for reuse below`

```
// cache PIDs for reuse below
```

## L147 · `print("  CORE  USAGE    MHz  QUEUE  ROLE\n");`

```
// Per-core
```

## L161-162 · `let mhz = sys(12 | ((i as i32) << 8) as i32);`

```
// 0 = the core did not run in the window (kernel 0.423+): no
// frequency to show, it slept.
```

## L186 · `if proc_count > 0 {`

```
// Processes (PID-based iteration)
```

## L199 · `let mut name_buf = [0u8; 16];`

```
// Decode name from packed i64s
```

## L218 · `let kind = sys(29 | (pid << 8));`

```
// Kind
```

## L252 · `print("\n  MEMORY\n  ──────\n");`

```
// Memory
```

## L260 · `print("\n  SCHED  spawned="); print_num(spawned);`

```
// Scheduler
```

## L267 · `let key = unsafe { npk_input_wait(1000) };`

```
// Wait for key or 1-second timeout — instant response to 'q'
```

## L269 · `if key == 0x71 || key == 0x51 { return; } // 'q' or 'Q'`

```
// 'q' or 'Q'
```

