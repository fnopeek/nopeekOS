//! x86_64.

pub mod cpu;

pub use cpu::{halt_forever, irq_disable, irq_restore, irq_save, irqs_enabled, without_interrupts, IrqState};
