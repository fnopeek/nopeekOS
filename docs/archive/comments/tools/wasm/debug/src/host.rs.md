# `tools/wasm/debug/src/host.rs` @ 5e0102684

## L1 · `#[link(wasm_import_module = "env")]`

```
//! Host function bindings for debug.wasm — TCP + terminal mirror + key inject.
```

## L3-5 · `#[link(wasm_import_module = "env")]`

```
// Host functions are WASM imports from the `env` module, resolved by the
// kernel at instantiation. Naming the module explicitly is what makes them
// imports rather than ordinary undefined C symbols, which rust-lld rejects.
```

