//! x86_64.

pub mod cpu;
pub mod fiber;
pub mod msr;
pub mod port;
pub mod time;

pub use cpu::{current_core, halt_forever, irq_disable, irq_restore, irq_save, irqs_enabled, stop_other_cores, without_interrupts, IrqState};
pub use time::{calibrate_counter, counter, counter_hz};
pub use port::{Port, PortRange};
pub use fiber::{fiber_switch, initial_frame, inherit_flags};
pub use crate::mm::paging::IDENTITY_MAX;
