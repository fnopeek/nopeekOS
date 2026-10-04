# `tools/wasm/dock/build.rs` @ 5e0102684

## L1-3 · `use nopeek_widgets::app_meta::{encode, AppMeta, IconRef};`

```
//! Emit the dock's AppMeta blob under $OUT_DIR for the `.npk.app_meta`
//! custom section. (The dock excludes itself from its own catalog, so
//! this mainly identifies it to other launchers.)
```

