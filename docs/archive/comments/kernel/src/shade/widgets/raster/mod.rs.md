# `kernel/src/shade/widgets/raster/mod.rs` @ 5e0102684

## L1-6 · `pub mod cpu;`

```
//! Rasterizer implementations.
//!
//! The compositor holds a `Box<dyn Rasterizer>` (trait from
//! `super::abi`). P10.5 ships `CpuRasterizer` — software text +
//! rect + icon into a `RasterTarget`. P10.12+ ships
//! `XeRenderRasterizer` on the Intel Xe render engine.
```

