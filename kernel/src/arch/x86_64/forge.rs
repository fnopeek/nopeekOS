//! forge on x86-64: the module traps the processor raises (#PF from a
//! guard page, #DE from a division) and the trap a host function raises.
//!
//! Generated code keeps the vmctx pinned in r14 the whole time it runs, so
//! one routine per reason serves every module: the IDT stub checks that
//! the faulting rip lies in the code range and returns into the routine,
//! which records the reason and unwinds to the entry. The host trap gets
//! the vmctx in rdi and puts r14 and r15 (the fuel register the resume
//! path writes back) in place first: a host function may have used both.

use forge_core::vmctx;
use crate::forge_rt::CODE_BASE;

core::arch::global_asm!(
    r#"
.globl forge_pf_stub
forge_pf_stub:
    push rax
    mov  rax, {lo}
    cmp  qword ptr [rsp + 16], rax
    jb   1f
    mov  rax, [rip + FORGE_NEXT_CODE]
    cmp  qword ptr [rsp + 16], rax
    jae  1f
    lea  rax, [rip + forge_trap_oob]
    mov  [rsp + 16], rax
    pop  rax
    add  rsp, 8
    iretq
1:  pop  rax
    jmp  page_fault_handler

.globl forge_de_stub
forge_de_stub:
    push rax
    mov  rax, {lo}
    cmp  qword ptr [rsp + 8], rax
    jb   2f
    mov  rax, [rip + FORGE_NEXT_CODE]
    cmp  qword ptr [rsp + 8], rax
    jae  2f
    lea  rax, [rip + forge_trap_div]
    mov  [rsp + 8], rax
    pop  rax
    iretq
2:  pop  rax
    jmp  divide_error_handler

forge_trap_oob:
    mov  eax, {oob}
    jmp  3f
forge_trap_div:
    mov  eax, {div}
3:  mov  [r14 + {tc}], rax
    mov  rbp, [r14 + {rbp}]
    mov  rsp, [r14 + {rsp}]
    jmp  [r14 + {res}]
"#,
    lo = const CODE_BASE,
    oob = const forge_core::trap::MEMORY_OUT_OF_BOUNDS,
    div = const forge_core::trap::DIVIDE_ERROR,
    tc = const vmctx::TRAP_CODE,
    rbp = const vmctx::TRAP_RBP,
    rsp = const vmctx::TRAP_RSP,
    res = const vmctx::TRAP_RESUME,
);

unsafe extern "C" {
    pub fn forge_pf_stub();
    pub fn forge_de_stub();
}

core::arch::global_asm!(
    r#"
.globl forge_host_trap
forge_host_trap:
    mov  r14, rdi
    mov  r15, [rdi + {fuel}]
    mov  [rdi + {tc}], rsi
    mov  rbp, [rdi + {rbp}]
    mov  rsp, [rdi + {rsp}]
    jmp  [rdi + {res}]
"#,
    tc = const vmctx::TRAP_CODE,
    rbp = const vmctx::TRAP_RBP,
    rsp = const vmctx::TRAP_RSP,
    res = const vmctx::TRAP_RESUME,
    fuel = const vmctx::FUEL,
);

unsafe extern "C" {
    /// Leaves the module with `code` as trap reason. Never returns.
    pub fn forge_host_trap(vm: *const u64, code: u64) -> !;
}

/// Code written as data is visible to instruction fetch on x86 without
/// maintenance (coherent instruction cache, the remap serialises).
pub fn sync_code(_base: u64, _len: usize) {}

/// The stack pointer of the caller.
#[inline(always)]
pub fn stack_pointer() -> u64 {
    let rsp: u64;
    // SAFETY: reads the stack pointer, nothing else.
    unsafe { core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags)) };
    rsp
}
