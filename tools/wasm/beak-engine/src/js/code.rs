//! The instruction set a program is compiled to, and the unit that holds it
//! (`Chunk`).
//!
//! A tree walker keeps its state on the Rust stack, which cannot be
//! suspended; generators and `async`/`await` need state that can be saved
//! mid-expression. Here the state is a plain value stack that can be stored
//! and resumed.
//!
//! No second semantics: every op calls the same helpers as the tree walker
//! (`Interp::binary`, `Interp::call`, `Value::truthy`, `Env`). Only the
//! dispatch differs, and a program is run either fully compiled or fully by
//! the tree walker, never mixed.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::ast::{BinOp, UnaryOp};
use super::value::Value;

/// One instruction. Jump targets are absolute indices into `Chunk::ops`, so
/// `patch` needs no offset arithmetic.
#[derive(Debug, Clone)]
pub enum Op {
    /// Push `constants[i]`.
    Const(u32),
    /// Push the value of `names[i]`.
    LoadVar(u32),
    /// Assign the top to `names[i]`; the value stays on the stack (assignment
    /// is an expression).
    StoreVar(u32),
    /// Bind `names[i]`, popping the value.
    ///
    /// `lexical`: a `let`/`const` binds in exactly the current environment,
    /// a `var` in the nearest function environment (already created by
    /// hoisting, possibly further up). Matters e.g. for `for (const x = 1; …)`
    /// with an outer `x`.
    DeclVar { name: u32, mutable: bool, lexical: bool },
    /// Name a freshly built function after the variable it is bound to
    /// (`var f = function(){}` -> `f.name === "f"`).
    NameFunc(u32),
    /// Convert the top to a property key before the value is evaluated:
    /// `ToPropertyKey` may have side effects and the spec fixes their order.
    ToKey,
    /// `this`.
    This,
    Pop,
    Dup,
    /// Drop the top and keep the value below (`a, b` drops a).
    Swap,
    Un(UnaryOp),
    /// `ToNumeric`: like `Un(Plus)`, but a BigInt stays a BigInt. `x++` on a
    /// BigInt must not go through `+x`, which throws.
    ToNumeric,
    /// Add or subtract one in the value's own type (`true` = add).
    Step(bool),
    Bin(BinOp),
    /// `typeof x` on a name: must not throw ReferenceError for an unbound
    /// name, so it is not `LoadVar` + `Un`.
    TypeofVar(u32),
    Jump(u32),
    /// Jump if the top is truthy; always pops.
    JumpTrue(u32),
    /// Jump if the top is falsy; always pops.
    JumpFalse(u32),
    /// For `&&`/`||`/`??`: jump on falsy/truthy/nullish and leave the value
    /// on the stack as the result of the expression.
    JumpFalseKeep(u32),
    JumpTrueKeep(u32),
    JumpNullishKeep(u32),
    /// `obj[names[i]]`. Stack: obj -> value.
    GetProp(u32),
    /// `obj[key]`. Stack: obj, key -> value.
    GetIndex,
    /// Stack: obj, value -> value (assignment is an expression).
    SetProp(u32),
    /// Stack: obj, key, value -> value.
    SetIndex,
    /// Stack: callee, this, arg0..argN -> result.
    ///
    /// `name` is the callee's name, used only in the error message;
    /// `u32::MAX` means none.
    Call { argc: u16, name: u32 },
    /// Stack: callee, arg0..argN -> result.
    /// `name` is only for the error message ("Intl.PluralRules is not a
    /// constructor"); `u32::MAX` means the callee was an expression.
    New { argc: u16, name: u32 },
    /// Array from the top `n` values. A hole (`[1, , 3]`) is not the same as
    /// `undefined`: `in` does not find it.
    MakeArray(u16),
    /// An empty object with `Object.prototype`.
    NewObject,
    /// Stack: obj, value -> obj. A data property under `names[i]`.
    DefineProp(u32),
    /// `{ __proto__: v }`: sets the prototype of the object on the stack
    /// instead of creating a property.
    SetLiteralProto,
    /// Stack: obj, key, value -> obj. `named` gives an anonymous function
    /// value the key as its name.
    DefinePropComputed { named: bool },
    /// Stack: obj, function -> obj. `get` selects getter or setter; both must
    /// be able to land on the same property.
    DefineAccessor { name: u32, get: bool },
    DefineAccessorComputed { get: bool },
    /// Stack: obj, source -> obj. `{...src}` copies the enumerable own
    /// properties.
    SpreadInto,
    /// Duplicate the top two, for `o[k]++` where object and key are evaluated
    /// once.
    Dup2,
    /// `[a, b, c]` -> `[c, a, b]`. For `o.p++`, where the old value is the
    /// result and must move below the object.
    Rot3,
    /// `[a, b, c, d]` -> `[d, a, b, c]`. The same for `o[k]++`, with object
    /// and key above.
    Rot4,
    /// A regular expression from `names[body]` and `names[flags]`.
    Regex { body: u32, flags: u32 },
    /// The template object of a tagged template (ES 13.2.8.4). Built once per
    /// site: `Interp::template_object` caches it under the address of this
    /// entry.
    TemplateObject(u32),
    /// `for await (… of x)`: get the async iterator (ES 7.4.2 with
    /// `hint: async`). Without `Symbol.asyncIterator` the sync iterator is
    /// used and marked as such.
    IterAllAsync,
    /// Call `next()` and push what must be awaited: the whole result for a
    /// real async iterator, only its `value` for a wrapped sync one (whose
    /// `done` is already known and recorded on the iterator).
    IterNextAsyncCall,
    /// After `Op::Await`: inspect the awaited result and either push the
    /// value or jump to the loop end.
    IterStepAsync(u32),
    /// Concatenate the top `n` values into a string (template literal).
    Concat(u16),
    /// `delete obj[names[i]]` or `delete obj[key]`.
    DeleteProp(u32),
    /// `#x in obj`, the brand check. The name is in the name table, the
    /// object on the stack.
    PrivateIn(u32),
    DeleteIndex,
    /// Build an array from the top `n` entries, each either a value or one to
    /// spread (`spread[k]`).
    MakeArraySpread { n: u16, spread: u32 },
    /// Like `Call`/`New`, but the arguments are one array on the stack, so
    /// `f(...xs)` can use the same call helper.
    CallSpread(u32),
    NewSpread,
    /// Read `super.k`. Stack: -> value. `Interp::super_get`.
    SuperGet(u32),
    /// `super.k` as callee. Stack: -> value, this. The receiver is the own
    /// `this`, the value comes from the parent; that split is what `super`
    /// means.
    SuperCallee(u32),
    /// `super(...)`. Stack: arg0..argN -> undefined. `Interp::super_call`
    /// finds the parent constructor, runs it on this `this` and then creates
    /// the own instance fields.
    SuperCall(u16),
    /// `super(...xs)`. Stack: args array -> result.
    SuperCallSpread,
    /// `import(spec, options)`. Stack: spec options -> promise.
    ImportCall,
    /// `import.meta`. Stack: -> the module's meta object, or `undefined`.
    ImportMeta,
    /// Jump if the top is `null`/`undefined`, leaving the value. Counterpart
    /// of `JumpNullishKeep` for optional chains.
    JumpNullishTo(u32),
    /// Build a function; the index points into `funcs`.
    Closure(u32),
    /// An object literal's method, getter or setter: a closure whose home
    /// object is the literal, so `super.x` inside it works. `under` is how
    /// many values above the object sit on the stack (1 for a computed key).
    Method { f: u32, under: u8 },
    /// Bind a pattern. Stack: value -> (nothing). The index points into
    /// `pats`.
    ///
    /// Like `Op::Class`, this delegates to `bind_pattern`/`declare_pattern`,
    /// the tree walker's helpers. Patterns carry defaults, rest elements,
    /// nesting and non-binding targets (`[a.b] = x`); reimplementing them
    /// would be a second assignment semantics.
    BindPat { pat: u32, mode: BindMode },
    /// Bind the head of a `for..of`/`for..in` loop. Stack: value ->
    /// (nothing). `Interp::for_head_bind` handles the three cases.
    BindHead(u32),
    /// Build a class; the index points into `classes`.
    ///
    /// Delegates to `Interp::eval_class`, the same function the tree walker
    /// uses. Class semantics (implicit constructor, derived pass-through,
    /// non-enumerable methods, getter and setter on one property) must exist
    /// only once.
    ///
    /// Its subexpressions (`extends`, computed keys, static fields) therefore
    /// always run in the tree walker; a class body has exactly one path.
    Class(u32),
    Throw,
    /// Throw the top value: the exit from a `finally` that did not catch.
    Rethrow,
    /// Open a handler. `catch`/`finally` are jump targets, `u32::MAX` means
    /// absent.
    ///
    /// The handler also records stack and environment depth: a throw in the
    /// middle of an expression leaves partial values behind, which must be
    /// cut off before the `catch` block runs.
    ///
    /// `finally` is the single copy of the finalizer. It is entered with a
    /// completion record on the stack, `[value, kind]`, and ends in
    /// `EndFinally`; see `FIN_NORMAL` and friends. A `catch` takes precedence
    /// for a throw; a `return` (also `gen.return()`) always goes to
    /// `finally`.
    TryStart { catch: u32, finally: u32 },
    /// Resume the completion record a finalizer was entered with. Stack:
    /// value, kind -> (nothing). Kinds from `FIN_JUMP` on jump to
    /// `finally_tables[i][kind - FIN_JUMP]`, the continuation of a `break` or
    /// `continue` that left the `try`.
    EndFinally(u32),
    /// Close the innermost handler.
    TryEnd,
    /// Bind the thrown value to `names[i]`: the head of a `catch`.
    BindCatch(u32),
    /// Get the iterator of the top value and store it in the frame.
    ///
    /// Lazy, via `get_iterator`/`iter_next`/`iter_close` like the tree
    /// walker: a body that mutates the source must see it, and an early exit
    /// must call `return()`.
    IterAll,
    /// Push the next value; jump when the iterator is done.
    IterNext(u32),
    /// Collect the keys of a `for…in` and store them in the frame. Stack:
    /// obj -> (nothing).
    ///
    /// Eager, via `Interp::for_in_keys`, so that mutating the object does not
    /// derail the loop. `null`/`undefined` yield an empty list (zero
    /// iterations, no error).
    ForInAll,
    /// Push the next key; jump when the list is empty.
    ForInNext(u32),
    /// Forget the iterator: it is done, calling `return()` would be wrong.
    IterDrop,
    /// Close the iterator (`return()`) and forget it: the path for `break`
    /// and any abrupt exit.
    IterClose,
    /// Return from the frame; the value is on top.
    Ret,
    /// Suspend. The top is the value `next()` returns; the frame stays as is.
    ///
    /// On resume `Vm::send` puts the argument of `next(v)` in the same stack
    /// slot, which is the value of the `yield` expression. A partial
    /// expression below (`a` in `a + (yield 1)`) survives because it lives in
    /// the frame's value stack, not on the Rust stack.
    Yield,
    /// The suspension point of `yield*` in a sync generator: hands out the
    /// inner iterator's result object unchanged, and marks the spot where
    /// `throw()`/`return()` are forwarded instead of unwinding.
    YieldDelegate(bool),
    /// `yield* x`: get the inner iterator (sync or async, depending on the
    /// generator kind) and push the first received value.
    DelegateStart(bool),
    /// Drive the inner iterator with `next`, `throw` or `return`, matching
    /// how the outer generator was resumed. Jumps to the loop end when the
    /// inner iterator has no `return` and the outer one must give up.
    DelegateCall(u32),
    /// Inspect the (possibly awaited) result: done -> jump to the target,
    /// otherwise push the value for the `yield`.
    DelegateStep { end: u32, is_async: bool },
    /// Await. The top is the awaited value; the machine suspends and is
    /// resumed with the promise's settlement.
    ///
    /// The same mechanism as `Yield`, except that a promise sits in front and
    /// resumption comes from the microtask queue instead of `next()`.
    Await,
    /// Open a block environment with the bindings hoisted to it
    /// (`blocks[i]`). Without them `let` would not be in its temporal dead
    /// zone from block start, and a block function declaration would not
    /// exist before its line.
    PushEnv(u32),
    PopEnv,
    /// Replace the innermost environment with a copy of its bindings
    /// (CreatePerIterationEnvironment, ES 14.7.4.4), so a closure from one
    /// iteration of `for (let i …)` keeps that iteration's `i`.
    CopyEnv,
    /// Record the program's completion value (its last expression value;
    /// `eval` and the console use it).
    SetCompletion,
}

/// How a pattern is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindMode {
    /// A declaration: the binding already exists (created by hoisting) and
    /// is initialized here.
    Init,
    /// Assignment to existing targets, including properties.
    Assign,
    /// The head of a `catch`: the names are created here.
    Declare,
}

/// What a block binds on entry, before its first statement runs.
///
/// Built at compile time by the same two loops as `Interp::hoist`, in the
/// same order and with the same cases. Two numbers per binding instead of
/// carrying the AST of the whole body.
#[derive(Debug, Clone)]
pub enum BlockDecl {
    /// `let`/`const`/`class`: bound but uninitialized (temporal dead zone).
    Tdz { name: u32, mutable: bool },
    /// A function declaration: initialized immediately so it is callable
    /// before its line.
    Func { name: u32, func: u32 },
}

/// Completion kinds of a finalizer's record (`Op::TryStart`).
pub const FIN_NORMAL: u32 = 0;
pub const FIN_THROW: u32 = 1;
pub const FIN_RETURN: u32 = 2;
/// First jump kind; `break`/`continue` exits are numbered from here.
pub const FIN_JUMP: u32 = 3;

/// "No hint yet." Not an `Option`, which would cost a byte per op site on a
/// hot path.
pub const HINT_NONE: u16 = u16::MAX;

/// Compiled code together with everything its ops refer to.
pub struct Chunk {
    pub ops: Vec<Op>,
    /// Per op site, the environment depth at which the name was found last
    /// time. Lexically fixed: the same site sees the same chain shape. A
    /// miss falls back to the full lookup; it is a hint, not an answer.
    pub hints: Vec<core::cell::Cell<u16>>,
    pub constants: Vec<Value>,
    /// Names (identifiers and properties), stored once instead of per op.
    pub names: Vec<Rc<str>>,
    pub funcs: Vec<Rc<super::ast::Func>>,
    /// The parts of each tagged template. Stored here, not in the AST,
    /// because a `Chunk` outlives the AST, and the address of the entry is
    /// the key under which the template object is cached.
    pub templates: Vec<Vec<super::ast::TemplateElement>>,
    pub classes: Vec<Rc<super::ast::Class>>,
    pub pats: Vec<super::ast::Pat>,
    pub heads: Vec<super::ast::ForHead>,
    pub blocks: Vec<Vec<BlockDecl>>,
    /// One mask per `MakeArraySpread`: which entries were `...x`.
    pub blocks_spread: Vec<Vec<bool>>,
    /// Per `try … finally`, the continuations of its jump exits.
    pub finally_tables: Vec<Vec<u32>>,
}

impl Chunk {
    pub fn new() -> Chunk {
        Chunk { ops: Vec::new(), hints: Vec::new(), constants: Vec::new(), names: Vec::new(),
                funcs: Vec::new(), templates: Vec::new(), classes: Vec::new(), pats: Vec::new(),
                heads: Vec::new(), blocks: Vec::new(), blocks_spread: Vec::new(),
                finally_tables: Vec::new() }
    }

    pub fn emit(&mut self, op: Op) -> usize {
        self.ops.push(op);
        // Keeps `hints` in step with `ops`; `ops` grows nowhere else.
        self.hints.push(core::cell::Cell::new(HINT_NONE));
        self.ops.len() - 1
    }

    /// Emit a jump with an unknown target and return its position.
    pub fn emit_jump(&mut self, make: fn(u32) -> Op) -> usize {
        self.emit(make(u32::MAX))
    }

    /// Set the target of a pending jump to the current position.
    pub fn patch(&mut self, at: usize) {
        let here = self.ops.len() as u32;
        match &mut self.ops[at] {
            Op::Jump(t) | Op::JumpFalse(t) | Op::JumpTrue(t) | Op::JumpFalseKeep(t)
            | Op::JumpTrueKeep(t) | Op::JumpNullishKeep(t) | Op::JumpNullishTo(t)
            | Op::IterNext(t) | Op::ForInNext(t) | Op::IterStepAsync(t)
            | Op::DelegateCall(t) | Op::DelegateStep { end: t, .. } => *t = here,
            other => panic!("patch auf {other:?}"),
        }
    }

    pub fn konst(&mut self, v: Value) -> u32 {
        self.constants.push(v);
        (self.constants.len() - 1) as u32
    }

    /// Names are deduplicated.
    pub fn name(&mut self, s: &str) -> u32 {
        if let Some(i) = self.names.iter().position(|n| &**n == s) {
            return i as u32;
        }
        self.names.push(Rc::from(s));
        (self.names.len() - 1) as u32
    }

    pub fn block(&mut self, d: Vec<BlockDecl>) -> u32 {
        self.blocks.push(d);
        (self.blocks.len() - 1) as u32
    }

    pub fn spread_mask(&mut self, m: Vec<bool>) -> u32 {
        self.blocks_spread.push(m);
        (self.blocks_spread.len() - 1) as u32
    }

    pub fn func(&mut self, f: Rc<super::ast::Func>) -> u32 {
        self.funcs.push(f);
        (self.funcs.len() - 1) as u32
    }

    pub fn template(&mut self, q: Vec<super::ast::TemplateElement>) -> u32 {
        self.templates.push(q);
        (self.templates.len() - 1) as u32
    }

    pub fn class(&mut self, c: Rc<super::ast::Class>) -> u32 {
        self.classes.push(c);
        (self.classes.len() - 1) as u32
    }

    pub fn pat(&mut self, p: super::ast::Pat) -> u32 {
        self.pats.push(p);
        (self.pats.len() - 1) as u32
    }

    pub fn head(&mut self, h: super::ast::ForHead) -> u32 {
        self.heads.push(h);
        (self.heads.len() - 1) as u32
    }

    pub fn here(&self) -> u32 {
        self.ops.len() as u32
    }
}

/// A construct the compiler does not support. Not an error but a decline:
/// the caller then runs the whole program in the tree walker, never half.
///
/// The text is the construct's name, not a sentence, so it can serve as a
/// key when declines are counted.
#[derive(Debug)]
pub struct Unsupported(pub &'static str);

pub type CompileResult<T> = Result<T, Unsupported>;

/// Prints the bare key, for counting declines across runs.
impl core::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.write_str(self.0)
    }
}

impl From<Unsupported> for String {
    fn from(u: Unsupported) -> String {
        String::from(u.0)
    }
}
