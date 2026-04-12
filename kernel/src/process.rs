//! Process Table — dynamic, PID-based process tracking.
//!
//! Tracks all running tasks: intents on workers, WASM apps, system tasks.
//! Independent of terminals. Foundation for kill, monitoring, resource limits.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

// Process kinds
pub const KIND_INTENT: u8 = 0;
pub const KIND_WASM: u8 = 1;
pub const KIND_SYSTEM: u8 = 2;

pub struct Process {
    pub pid: u32,
    pub name: [u8; 32],
    pub name_len: u8,
    pub kind: u8,
    pub terminal_idx: u8, // 255 = no terminal
    pub core_id: u8,
    pub start_tsc: u64,
    pub busy_tsc: u64,
    pub memory: u32, // bytes
    // Delta computation for CPU%
    last_busy: u64,
    last_tsc: u64,
    pub cpu_pct: u32, // 0-100
}

static PROCS: Mutex<BTreeMap<u32, Process>> = Mutex::new(BTreeMap::new());
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// Register a new process. Returns the assigned PID.
pub fn spawn(name: &str, kind: u8, terminal_idx: u8, core_id: u8) -> u32 {
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let mut proc_name = [0u8; 32];
    let len = name.len().min(32);
    proc_name[..len].copy_from_slice(&name.as_bytes()[..len]);

    let process = Process {
        pid,
        name: proc_name,
        name_len: len as u8,
        kind,
        terminal_idx,
        core_id,
        start_tsc: crate::interrupts::rdtsc(),
        busy_tsc: 0,
        memory: 0,
        last_busy: 0,
        last_tsc: 0,
        cpu_pct: 0,
    };

    PROCS.lock().insert(pid, process);
    pid
}

/// Deregister a process.
pub fn exit(pid: u32) {
    PROCS.lock().remove(&pid);
}

/// Number of active processes.
pub fn count() -> usize {
    PROCS.lock().len()
}

/// Get PID at table index (for iteration by top).
/// BTreeMap is ordered by PID, so iteration is deterministic.
pub fn pid_at_index(idx: usize) -> u32 {
    PROCS.lock().values().nth(idx).map(|p| p.pid).unwrap_or(0)
}

/// Accumulate CPU busy cycles for a process.
pub fn add_busy_tsc(pid: u32, cycles: u64) {
    if let Some(p) = PROCS.lock().get_mut(&pid) {
        p.busy_tsc += cycles;
    }
}

/// Update WASM linear memory size.
pub fn set_memory(pid: u32, bytes: u32) {
    if let Some(p) = PROCS.lock().get_mut(&pid) {
        p.memory = bytes;
    }
}

/// Compute CPU usage % from delta busy/total TSC (on-demand).
fn update_usage(pid: u32, procs: &mut BTreeMap<u32, Process>) {
    let tsc = crate::interrupts::rdtsc();
    if let Some(p) = procs.get_mut(&pid) {
        let prev_busy = p.last_busy;
        let prev_tsc = p.last_tsc;
        p.last_busy = p.busy_tsc;
        p.last_tsc = tsc;

        if prev_tsc == 0 { return; }

        let delta_busy = p.busy_tsc.wrapping_sub(prev_busy);
        let delta_tsc = tsc.wrapping_sub(prev_tsc);

        if delta_tsc > 0 {
            let pct = (delta_busy * 100).checked_div(delta_tsc).unwrap_or(0);
            p.cpu_pct = pct.min(100) as u32;
        }
    }
}

// ── npk_sys_info query interface ──────────────────────────

/// Query process info for npk_sys_info. Key encoding:
/// - low byte (key & 0xFF): info type
/// - high bits (key >> 8): index for key 20-21, PID for keys 22-29
pub fn sys_info(key: i32) -> i64 {
    let info = key & 0xFF;
    let param = (key >> 8) as u32;

    match info {
        // 20: process count
        20 => count() as i64,

        // 21: PID at index (for iteration)
        21 => pid_at_index(param as usize) as i64,

        // 22: CPU% for PID
        22 => {
            let mut procs = PROCS.lock();
            update_usage(param, &mut procs);
            procs.get(&param).map(|p| p.cpu_pct as i64).unwrap_or(0)
        }

        // 23: memory in KB for PID
        23 => {
            PROCS.lock().get(&param)
                .map(|p| (p.memory / 1024) as i64)
                .unwrap_or(0)
        }

        // 24: core_id for PID
        24 => {
            PROCS.lock().get(&param)
                .map(|p| p.core_id as i64)
                .unwrap_or(-1)
        }

        // 25: name bytes 0-7 as i64 (little-endian packed)
        25 => {
            PROCS.lock().get(&param).map(|p| {
                let mut val = 0u64;
                let len = (p.name_len as usize).min(8);
                for i in 0..len { val |= (p.name[i] as u64) << (i * 8); }
                val as i64
            }).unwrap_or(0)
        }

        // 26: name bytes 8-15 as i64
        26 => {
            PROCS.lock().get(&param).map(|p| {
                if p.name_len <= 8 { return 0i64; }
                let mut val = 0u64;
                let len = (p.name_len as usize).min(16);
                for i in 8..len { val |= (p.name[i] as u64) << ((i - 8) * 8); }
                val as i64
            }).unwrap_or(0)
        }

        // 27: uptime in seconds for PID
        27 => {
            PROCS.lock().get(&param).map(|p| {
                let freq = crate::interrupts::tsc_freq();
                if freq == 0 || p.start_tsc == 0 { return 0i64; }
                ((crate::interrupts::rdtsc() - p.start_tsc) / freq) as i64
            }).unwrap_or(0)
        }

        // 28: terminal_idx for PID (255 = no terminal)
        28 => {
            PROCS.lock().get(&param)
                .map(|p| p.terminal_idx as i64)
                .unwrap_or(255)
        }

        // 29: kind for PID (0=Intent, 1=WASM, 2=System)
        29 => {
            PROCS.lock().get(&param)
                .map(|p| p.kind as i64)
                .unwrap_or(-1)
        }

        _ => -1,
    }
}
