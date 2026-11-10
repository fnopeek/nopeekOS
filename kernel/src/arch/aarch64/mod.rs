//! AArch64.

pub mod boot;
pub mod cores;
pub mod cpu;
pub mod dma;
pub mod exceptions;
pub mod gic;
pub mod mmu;
pub mod paging;
pub mod time;
pub mod timer;

core::arch::global_asm!(include_str!("boot.s"));

pub use cpu::{current_core, exception_level, halt_forever, irq_disable, irq_restore, irq_save, irqs_enabled, stop_other_cores, without_interrupts, IrqState};
pub use mmu::IDENTITY_MAX;
pub use time::{calibrate_counter, counter, counter_hz};
