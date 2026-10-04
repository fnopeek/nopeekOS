//! Rasterizer implementations.
//!
//! The compositor holds a `Box<dyn Rasterizer>` (trait from
//! `super::abi`). `CpuRasterizer` draws text + rect + icon into a
//! `RasterTarget` in software; a GPU backend would implement the same
//! trait.

pub mod cpu;
