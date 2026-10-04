# `kernel/src/process.rs` @ 5e0102684

## L1-4 · `use alloc::collections::BTreeMap;`

```
//! Process Table — dynamic, PID-based process tracking.
//!
//! Tracks all running tasks: intents on workers, WASM apps, system tasks.
//! Independent of terminals. Foundation for kill, monitoring, resource limits.
```

## L10 · `pub const KIND_INTENT: u8 = 0;`

```
// Process kinds
```

## L20 · `pub terminal_idx: u8, // 255 = no terminal`

```
// 255 = no terminal
```

## L24 · `pub memory: u32, // bytes`

```
// bytes
```

## L25 · `last_busy: u64,`

```
// Delta computation for CPU%
```

## L28 · `pub cpu_pct: u32, // 0-100`

```
// 0-100
```

## L34 · `pub fn spawn(name: &str, kind: u8, terminal_idx: u8, core_id: u8) -> u32 {`

```
/// Register a new process. Returns the assigned PID.
```

## L60 · `pub fn exit(pid: u32) {`

```
/// Deregister a process.
```

## L65 · `pub fn count() -> usize {`

```
/// Number of active processes.
```

## L70-71 · `pub fn pid_at_index(idx: usize) -> u32 {`

```
/// Get PID at table index (for iteration by top).
/// BTreeMap is ordered by PID, so iteration is deterministic.
```

## L76 · `pub fn add_busy_tsc(pid: u32, cycles: u64) {`

```
/// Accumulate CPU busy cycles for a process.
```

## L83 · `pub fn set_memory(pid: u32, bytes: u32) {`

```
/// Update WASM linear memory size.
```

## L90 · `fn update_usage(pid: u32, procs: &mut BTreeMap<u32, Process>) {`

```
/// Compute CPU usage % from delta busy/total TSC (on-demand).
```

## L111 · `pub fn sys_info(key: i32) -> i64 {`

```
// ── npk_sys_info query interface ──────────────────────────
```

## L113-115 · `pub fn sys_info(key: i32) -> i64 {`

```
/// Query process info for npk_sys_info. Key encoding:
/// - low byte (key & 0xFF): info type
/// - high bits (key >> 8): index for key 20-21, PID for keys 22-29
```

## L121 · `20 => count() as i64,`

```
// 20: process count
```

## L124 · `21 => pid_at_index(param as usize) as i64,`

```
// 21: PID at index (for iteration)
```

## L127 · `22 => {`

```
// 22: CPU% for PID
```

## L134 · `23 => {`

```
// 23: memory in KB for PID
```

## L141 · `24 => {`

```
// 24: core_id for PID
```

## L148 · `25 => {`

```
// 25: name bytes 0-7 as i64 (little-endian packed)
```

## L158 · `26 => {`

```
// 26: name bytes 8-15 as i64
```

## L169 · `27 => {`

```
// 27: uptime in seconds for PID
```

## L178 · `28 => {`

```
// 28: terminal_idx for PID (255 = no terminal)
```

## L185 · `29 => {`

```
// 29: kind for PID (0=Intent, 1=WASM, 2=System)
```

