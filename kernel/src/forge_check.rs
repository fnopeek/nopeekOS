//! forge checked where it runs: the embedded modules of `forge_tests`
//! against what the same compiler produced on the host, and the unwind
//! out of a host function.

use crate::kprintln;

/// A 70-byte module that calls one import and then returns 99. The 99 must
/// never come out: the import leaves the run via `forge_rt::host_trap`, so
/// a broken unwind shows up here rather than later in python.
///
/// Hand-assembled (types, one import, export "f", three instructions).
const TRAP_PROBE: &[u8] = &[
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x09, 0x02, 0x60,
    0x00, 0x00, 0x60, 0x01, 0x7f, 0x01, 0x7f, 0x02, 0x1c, 0x01, 0x03, 0x65,
    0x6e, 0x76, 0x14, 0x6e, 0x70, 0x6b, 0x5f, 0x66, 0x6f, 0x72, 0x67, 0x65,
    0x5f, 0x74, 0x72, 0x61, 0x70, 0x5f, 0x70, 0x72, 0x6f, 0x62, 0x65, 0x00,
    0x00, 0x03, 0x02, 0x01, 0x01, 0x07, 0x05, 0x01, 0x01, 0x66, 0x00, 0x01,
    0x0a, 0x08, 0x01, 0x06, 0x00, 0x10, 0x00, 0x41, 0x63, 0x0b,
];

extern "C" fn probe_exit(vm: *const u64) -> i32 {
    // SAFETY: `vm` is the running instance's vmctx, passed by the generator
    // as the first argument. Does not return.
    unsafe { crate::forge_rt::host_trap(vm, forge_core::trap::EXIT) }
}

struct ProbeHost;
impl crate::forge_rt::HostImports for ProbeHost {
    fn ctx_ptr(&self) -> u64 {
        // The stub never touches host state, so there is no pointer to give.
        0
    }
    fn resolve(&self, module: &str, name: &str) -> Option<(u64, crate::forge_rt::HostSig)> {
        let sig = crate::forge_rt::HostSig { params: &[], result: None };
        (module == "env" && name == "npk_forge_trap_probe")
            .then(|| (probe_exit as *const () as u64, sig))
    }
}

/// Can a host function end the run instead of returning?
///
/// The host harness cannot test this: there each run is its own process and
/// `proc_exit` just ends it. In the kernel the run must be unwound by an
/// assembly stub that restores `rsp` and `rbp`.
fn trap_probe() -> bool {
    use crate::forge_rt::Instance;
    let Ok(m) = forge_core::compile(TRAP_PROBE) else {
        kprintln!("[npk] forge: trap probe failed to compile");
        return false;
    };
    let entry = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i)
        .and_then(|i| m.offset_of(i));
    let Some(off) = entry else {
        kprintln!("[npk] forge: trap probe has no f");
        return false;
    };
    let Some(mut inst) = Instance::new_with_host(&m, &ProbeHost) else {
        kprintln!("[npk] forge: trap probe: instance could not be built");
        return false;
    };
    if inst.unresolved_imports() != 0 {
        kprintln!("[npk] forge: trap probe: import not resolved");
        return false;
    }
    inst.set_fuel(i64::MAX / 4);
    let (got, trap) = inst.call(off, 0, 0, 0);
    if trap != forge_core::trap::EXIT {
        kprintln!("[npk] forge: trap probe -> {} instead of EXIT (result {})",
            forge_core::trap::name(trap), got);
        return false;
    }
    // 99 means the host function returned instead of trapping.
    if got == 99 {
        kprintln!("[npk] forge: trap probe RETURNED: the unwind did not take");
        return false;
    }
    true
}

/// Compile the embedded modules, run them, and compare against what the same
/// compiler produced on the host. The expectations in `forge_tests.rs` are
/// generated there and checked against the interpreter, so this asks whether
/// kernel and host agree down to the trap codes.
pub fn selftest() {
    use crate::forge_rt::Instance;
    use crate::forge_tests::CASES;

    let (mut ok, mut bad) = (0u32, 0u32);
    for c in CASES {
        let m = match forge_core::compile(c.wasm) {
            Ok(m) => m,
            Err(e) => {
                kprintln!("[npk] forge: {}: compile: {}", c.name, e);
                bad += 1;
                continue;
            }
        };
        let Some(fidx) = m.plan.exports.iter().find(|(n, _)| n == "f").map(|(_, i)| *i) else {
            kprintln!("[npk] forge: {}: no export f", c.name);
            bad += 1;
            continue;
        };
        let Some(off) = m.offset_of(fidx) else {
            kprintln!("[npk] forge: {}: no address", c.name);
            bad += 1;
            continue;
        };
        let Some(mut inst) = Instance::new(&m) else {
            kprintln!("[npk] forge: {}: instance could not be built", c.name);
            bad += 1;
            continue;
        };
        inst.set_fuel(if c.fuel < 0 { i64::MAX / 4 } else { c.fuel });

        let (got, trap) = inst.call(off, c.arg, 0, 0);
        if got == c.want && trap == c.trap {
            ok += 1;
        } else {
            bad += 1;
            kprintln!(
                "[npk] forge: {} (arg {}) -> {} / {} instead of {} / {}",
                c.name, c.arg, got, forge_core::trap::name(trap), c.want,
                forge_core::trap::name(c.trap)
            );
        }
    }

    kprintln!("[npk] forge selftest: {}/{} same as on the host", ok, ok + bad);
    kprintln!("[npk] forge: host trap (unwind from a host function): {}",
        if trap_probe() { "works" } else { "FAILED" });
    if bad == 0 {
        kprintln!("[npk] forge: guard page, table, division, unreachable and fuel");
        kprintln!("[npk] forge: report the same reason on the device.");
    }
}

