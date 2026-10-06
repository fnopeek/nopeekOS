//! JavaScript: lexer, syntax tree, parser, compiler, VM and runtime.
//!
//! Hand-written rather than ported: the established JS parsers (swc, oxc,
//! boa) are large, `std`-bound and arena-based, and their trees are shaped
//! for their own engines. Conformance is checked against test262.

pub mod ast;
/// The instruction set; `compile` translates the AST into it.
pub mod code;
pub mod compile;
pub mod vm;
pub mod bigint;
pub mod builtins;
pub mod date;
pub mod iterhelp;
pub mod dombind;
pub mod eval;
pub mod json;
pub mod modules;
pub mod expr;
pub mod fetch;
pub mod generator;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod promise;
pub mod qsel;
pub mod random;
pub mod proxy;
pub mod test262;
pub mod ws_crypto;
pub mod websocket;
pub mod url;
pub mod xpath;
pub mod regexp;
pub mod value;

pub use ast::Program;
pub use parser::{parse, ParseError};
pub use interp::TEST_STEPS;
pub use interp::{STRICT_SITES, STRICT_SITE_NAMES};

/// Run a program. An uncaught `throw` comes back as the thrown value's
/// `name: message`, not as a Rust error.
pub fn run(src: &str, module: bool) -> Result<(), alloc::string::String> {
    run_capped(src, module, u64::MAX)
}

/// Like `run`, with a step limit for test runners.
pub fn run_capped(src: &str, module: bool, max_steps: u64) -> Result<(), alloc::string::String> {
    use alloc::string::ToString;
    let prog = parse(src, module).map_err(|e| alloc::format!("SyntaxError: {} @{}", e.msg, e.at))?;
    let mut i = interp::Interp::new();
    i.max_steps = max_steps;
    match i.run_program(&prog) {
        Ok(_) => Ok(()),
        Err(interp::Abrupt::Throw(v)) => {
            let name = i.get(&v, "name").ok().and_then(|n| i.to_string(&n).ok());
            let msg = i.get(&v, "message").ok().and_then(|m| i.to_string(&m).ok());
            Err(match (name, msg) {
                (Some(n), Some(m)) if !m.is_empty() => alloc::format!("{n}: {m}"),
                (Some(n), _) => n.to_string(),
                _ => "uncaught exception".to_string(),
            })
        }
        Err(_) => Err("illegal completion".to_string()),
    }
}

/// An execution context in which several programs run one after another.
///
/// Lets a test runner parse its harness (`assert.js`, `sta.js`) once and
/// only execute it before each test.
pub struct Session {
    pub interp: interp::Interp,
}

impl Session {
    pub fn new(max_steps: u64) -> Session {
        let mut interp = interp::Interp::new();
        interp.max_steps = max_steps;
        Session { interp }
    }

    /// A session that runs on the tree-walker instead of the VM, for
    /// cross-checking the two.
    pub fn new_without_vm(max_steps: u64) -> Session {
        let mut s = Session::new(max_steps);
        s.interp.vm_off = true;
        s
    }

    /// Run an already parsed program.
    pub fn run(&mut self, prog: &Program) -> Result<(), alloc::string::String> {
        match self.interp.run_program(prog) {
            Ok(_) => Ok(()),
            Err(interp::Abrupt::Throw(v)) => Err(self.describe(v)),
            Err(_) => Err(alloc::string::String::from("illegal completion")),
        }
    }

    fn describe(&mut self, v: value::Value) -> alloc::string::String {
        use alloc::string::ToString;
        let name = self.interp.get(&v, "name").ok().and_then(|n| self.interp.to_string(&n).ok());
        let msg = self.interp.get(&v, "message").ok().and_then(|m| self.interp.to_string(&m).ok());
        match (name, msg) {
            (Some(n), Some(m)) if !m.is_empty() => alloc::format!("{n}: {m}"),
            (Some(n), _) if !n.is_empty() => n.to_string(),
            _ => self.interp.to_string(&v).map(|s| s.to_string())
                    .unwrap_or_else(|_| "uncaught exception".to_string()),
        }
    }
}

/// Check only whether the source parses, without keeping the tree.
pub fn parses(src: &str, module: bool) -> Result<(), ParseError> {
    parse(src, module).map(|_| ())
}
