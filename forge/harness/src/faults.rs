//! Turning a processor fault into a wasm trap.
//!
//! Two of the traps the generator relies on are not raised by an instruction
//! it emits: an access past the end of linear memory arrives as a page fault,
//! and a bad division as #DE. Both are the CPU telling us something a check
//! would otherwise have had to look for on every single operation — which is
//! exactly the trade the guard page and the bare `idiv` were chosen for.
//!
//! Catching them means pointing the interrupted context at the module's trap
//! routine with the reason in `rax`. Everything after that is the routine's
//! ordinary work. **The kernel's #PF and #DE handlers will do precisely this**,
//! against the same trap routine and the same instance context — the only
//! difference is that they read the interrupted registers out of a trap frame
//! rather than a `ucontext`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// The code region currently able to fault, and where its traps go. One
/// instance runs at a time here, so three words are enough.
static BASE: AtomicU64 = AtomicU64::new(0);
static LEN: AtomicUsize = AtomicUsize::new(0);
static PF_ENTRY: AtomicU64 = AtomicU64::new(0);
static DE_ENTRY: AtomicU64 = AtomicU64::new(0);

/// Was the fault inside generated code? That is the whole test — a fault
/// anywhere else is a real one and must stay real.
fn ours(rip: u64) -> bool {
    let base = BASE.load(Ordering::Relaxed);
    let len = LEN.load(Ordering::Relaxed) as u64;
    base != 0 && rip >= base && rip < base + len
}

extern "C" fn on_fault(sig: i32, _info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    // SAFETY: the third argument of a `SA_SIGINFO` handler is a `ucontext_t`.
    unsafe {
        let uc = uc as *mut libc::ucontext_t;
        let rip = (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as u64;
        if !ours(rip) {
            // Not ours. Put the default back and return, so the fault happens
            // again and ends the process the way it should.
            libc::signal(sig as libc::c_int, libc::SIG_DFL);
            return;
        }
        // Point the interrupted context at the entry for this fault. The
        // entry names the reason itself, so no general register has to be
        // touched — which is what lets the kernel do the same with almost
        // nothing.
        let entry = if sig == libc::SIGFPE {
            DE_ENTRY.load(Ordering::Relaxed)
        } else {
            PF_ENTRY.load(Ordering::Relaxed)
        };
        (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] = entry as i64;
    }
}

/// Install the handlers once.
pub fn install() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: a plain `sigaction` for two signals, with a handler that
        // only reads and writes the interrupted context.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_fault as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGFPE, &sa, std::ptr::null_mut());
        }
    });
}

/// Say which code may fault, and where its traps go.
pub fn arm(base: u64, len: usize, pf: u64, de: u64) {
    install();
    BASE.store(base, Ordering::Relaxed);
    LEN.store(len, Ordering::Relaxed);
    PF_ENTRY.store(pf, Ordering::Relaxed);
    DE_ENTRY.store(de, Ordering::Relaxed);
}
