//! GUI subsystem
//!
//! Graphical rendering, damage-tracked compositing, and login screen.
//! Builds on framebuffer.rs shadow buffer + MMIO blit.

pub mod background;
pub mod color;
pub mod font;
pub mod render;
pub mod login;
