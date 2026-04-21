//! Rasterizer implementations.
//!
//! The compositor holds a `Box<dyn Rasterizer>` (trait from
//! `super::abi`). P10.5 ships `CpuRasterizer` — software text +
//! rect + icon into a `RasterTarget`. P10.12+ ships
//! `XeRenderRasterizer` on the Intel Xe render engine.

pub mod cpu;
