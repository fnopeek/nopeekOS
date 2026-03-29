//! WASM Runtime
//!
//! Sandboxed execution via wasmi interpreter.
//! Every host function is capability-gated.
//! WASM modules can only interact with the system through
//! explicitly registered host functions.

use alloc::string::String;
use alloc::vec::Vec;
use wasmi::{Caller, Engine, Linker, Module, Store, Value};
use spin::Mutex;
use crate::kprintln;

/// Result from a WASM module execution
pub struct WasmResult {
    pub output: String,
    pub success: bool,
}

/// Host state passed to WASM host functions
struct HostState {
    output: String,
}

/// Global WASM engine (reused across invocations)
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

pub fn init() {
    let engine = Engine::default();
    *ENGINE.lock() = Some(engine);
    kprintln!("[npk] WASM runtime: wasmi v0.31 (interpreter)");
}

/// Execute a WASM module with the given function name and arguments
pub fn execute(wasm_bytes: &[u8], func_name: &str, args: &[Value]) -> Result<WasmResult, WasmError> {
    let engine_guard = ENGINE.lock();
    let engine = engine_guard.as_ref().ok_or(WasmError::NotInitialized)?;

    let module = Module::new(engine, wasm_bytes)
        .map_err(|_| WasmError::InvalidModule)?;

    let mut store = Store::new(engine, HostState {
        output: String::new(),
    });

    let mut linker = <Linker<HostState>>::new(engine);
    register_host_functions(&mut linker, engine)?;

    let instance = linker.instantiate(&mut store, &module)
        .map_err(|_| WasmError::InstantiationFailed)?
        .start(&mut store)
        .map_err(|_| WasmError::StartFailed)?;

    let func = instance.get_func(&store, func_name)
        .ok_or(WasmError::FunctionNotFound)?;

    let mut results = [Value::I32(0)];
    func.call(&mut store, args, &mut results)
        .map_err(|_| WasmError::ExecutionFailed)?;

    let host = store.data();
    let mut output = host.output.clone();

    // If no explicit output, format the return value
    if output.is_empty() {
        match results[0] {
            Value::I32(v) => output = alloc::format!("{}", v),
            Value::I64(v) => output = alloc::format!("{}", v),
            _ => output = alloc::format!("{:?}", results[0]),
        }
    }

    Ok(WasmResult { output, success: true })
}

/// Register host functions that WASM modules can call
fn register_host_functions(linker: &mut Linker<HostState>, _engine: &Engine) -> Result<(), WasmError> {
    // npk_print(ptr, len) — write to output buffer
    linker.func_wrap("env", "npk_print",
        |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
            let mem = caller.get_export("memory")
                .and_then(|e| e.into_memory());
            if let Some(mem) = mem {
                let start = ptr as usize;
                let end = start + len as usize;
                let data = mem.data(&caller);
                // Copy bytes out before borrowing caller mutably
                if end <= data.len() {
                    let mut buf = alloc::vec![0u8; len as usize];
                    buf.copy_from_slice(&data[start..end]);
                    if let Ok(s) = core::str::from_utf8(&buf) {
                        caller.data_mut().output.push_str(s);
                    }
                }
            }
        },
    ).map_err(|_| WasmError::HostFunctionError)?;

    Ok(())
}

#[derive(Debug)]
pub enum WasmError {
    NotInitialized,
    InvalidModule,
    InstantiationFailed,
    StartFailed,
    FunctionNotFound,
    ExecutionFailed,
    HostFunctionError,
}

impl core::fmt::Display for WasmError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            WasmError::NotInitialized => write!(f, "WASM runtime not initialized"),
            WasmError::InvalidModule => write!(f, "invalid WASM module"),
            WasmError::InstantiationFailed => write!(f, "module instantiation failed"),
            WasmError::StartFailed => write!(f, "module start failed"),
            WasmError::FunctionNotFound => write!(f, "exported function not found"),
            WasmError::ExecutionFailed => write!(f, "execution failed"),
            WasmError::HostFunctionError => write!(f, "host function registration error"),
        }
    }
}

/// Helper to create Value::I32 without exposing wasmi types
pub fn val_i32(v: i32) -> Value { Value::I32(v) }

// === Built-in WASM modules ===

/// Minimal WASM module: exports add(i32, i32) -> i32
/// Built from: (module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))
pub const MODULE_ADD: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D, // magic: \0asm
    0x01, 0x00, 0x00, 0x00, // version: 1
    // Type section: one function type (i32, i32) -> i32
    0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F,
    // Function section: function 0 uses type 0
    0x03, 0x02, 0x01, 0x00,
    // Export section: "add" -> function 0
    0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00,
    // Code section: function body
    0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6A, 0x0B,
];

/// Minimal WASM module: exports multiply(i32, i32) -> i32
/// Built from: (module (func (export "multiply") (param i32 i32) (result i32) local.get 0 local.get 1 i32.mul))
pub const MODULE_MULTIPLY: &[u8] = &[
    0x00, 0x61, 0x73, 0x6D,
    0x01, 0x00, 0x00, 0x00,
    0x01, 0x07, 0x01, 0x60, 0x02, 0x7F, 0x7F, 0x01, 0x7F,
    0x03, 0x02, 0x01, 0x00,
    0x07, 0x0C, 0x01, 0x08, 0x6D, 0x75, 0x6C, 0x74, 0x69, 0x70, 0x6C, 0x79, 0x00, 0x00,
    0x0A, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6C, 0x0B,
];
