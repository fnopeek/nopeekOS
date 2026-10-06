//! Evaluation: the tree walker, and the interpreter state it shares with
//! the bytecode machine (`vm.rs`).
//!
//! The tree walker runs whatever the compiler declines; both must keep the
//! same semantics.

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use hashbrown::{HashMap, HashSet};

use super::ast::*;
use super::value::*;
pub use super::value::Value;

/// An abrupt completion: anything that is not "the next expression".
pub enum Abrupt {
    Throw(Value),
    Return(Value),
    Break(Option<String>),
    Continue(Option<String>),
}
pub type C<T> = Result<T, Abrupt>;

pub struct Binding {
    pub value: Value,
    pub mutable: bool,
    /// `let`/`const` before their declaration: access throws. This is the
    /// temporal dead zone; without it `let` behaves like `var`.
    pub initialized: bool,
}

pub struct Env {
    pub vars: HashMap<Rc<str>, Binding>,
    pub parent: Option<Rc<RefCell<Env>>>,
    /// Only function environments carry `this`; a block inherits it. That is
    /// how an arrow sees the `this` of its surroundings.
    pub this_val: Option<Value>,
    /// Is this a function environment (target for `var` hoisting)?
    pub is_func_scope: bool,
    /// The home object of the enclosing method; for a class its `prototype`.
    /// `super.f` looks up on its prototype, not on `this`'s, or a method calling
    /// `super.f()` would find itself and recurse forever. An arrow does not set
    /// it and inherits it through the chain, like `this`.
    pub home: Option<Gc>,
    /// Strict? Strictness belongs to the code (see `ast::Func::strict`) but is
    /// read at runtime on every assignment, `delete` and `this`. It is copied in
    /// when the environment is created and inherited, so a block in a strict
    /// function is strict without walking the chain.
    pub strict: bool,
    /// Names imported from another module.
    ///
    /// A reference, not a copy: a module graph with cycles needs live bindings,
    /// so a module that runs earlier sees the value the other writes later. A
    /// copy taken when linking would silently show `undefined`.
    ///
    /// Only module environments carry the table; all others hold `None`, which
    /// costs a null check in the chain walk.
    pub imports: Option<Box<HashMap<Rc<str>, (Rc<RefCell<Env>>, Rc<str>)>>>,
    /// The object of a `with (o) { … }`.
    ///
    /// An environment whose names come from an object, the only kind whose
    /// bindings can change while running. That is why the lookup hints
    /// (`Chunk::hints`) are switched off once one exists: a depth hint could
    /// skip a property that did not exist last time.
    pub with_obj: Option<Gc>,
}

impl Env {
    /// Inherits strictness from the parent. A function call overrides it right
    /// after with that of its own body; only there may it change, and only
    /// towards strict.
    pub fn new(parent: Option<Rc<RefCell<Env>>>, func_scope: bool) -> Rc<RefCell<Env>> {
        let strict = parent.as_ref().is_some_and(|p| p.borrow().strict);
        Rc::new(RefCell::new(Env {
            vars: HashMap::new(), parent, this_val: None, is_func_scope: func_scope, home: None,
            strict, imports: None, with_obj: None,
        }))
    }
}

/// Is the running code strict? A field, not a chain walk; inheritance
/// happened at creation.
pub fn env_strict(env: &Rc<RefCell<Env>>) -> bool { env.borrow().strict }

pub fn env_lookup(env: &Rc<RefCell<Env>>, name: &str) -> Option<Rc<RefCell<Env>>> {
    env_lookup_depth(env, name).map(|(e, _)| e)
}

/// Like `env_lookup`, but also returns how many hops it took.
pub fn env_lookup_depth(env: &Rc<RefCell<Env>>, name: &str)
    -> Option<(Rc<RefCell<Env>>, usize)> {
    let mut depth = 0usize;
    let mut cur = env.clone();
    loop {
        {
            let b = cur.borrow();
            if b.vars.contains_key(name) { drop(b); return Some((cur, depth)); }
            if b.imports.as_ref().is_some_and(|m| m.contains_key(name)) {
                drop(b); return Some((cur, depth));
            }
        }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => { cur = p; depth += 1; } None => return None }
    }
}

/// Follow an `import` until a real binding is found.
///
/// A chain is possible (`export { x } from …` passes through), and so is a
/// cycle (a module re-importing its own name). The cap is therefore the
/// termination condition, not a precaution.
pub fn env_deref(env: &Rc<RefCell<Env>>, name: &str) -> Option<(Rc<RefCell<Env>>, Rc<str>)> {
    env_deref_from(env.clone(), Rc::from(name))
}

/// Like `env_deref`, but with a name the caller already owns.
///
/// The ordinary read path uses this and so allocates nothing; `Rc::from(name)`
/// would copy the string to the heap per variable access, for a chain that
/// is almost never entered.
pub fn env_deref_from(mut e: Rc<RefCell<Env>>, mut n: Rc<str>)
    -> Option<(Rc<RefCell<Env>>, Rc<str>)> {
    for _ in 0..64 {
        let next = {
            let b = e.borrow();
            if b.vars.contains_key(&*n) { None }
            else { b.imports.as_ref().and_then(|m| m.get(&n).cloned()) }
        };
        match next {
            None => return Some((e, n)),
            Some((e2, n2)) => { e = e2; n = n2; }
        }
    }
    None
}

/// The result of one look into one environment.
pub enum Hit {
    /// The name is here, with this value.
    Val(Value),
    /// It is here but not yet initialised (temporal dead zone).
    Dead,
    /// It comes from another module.
    Import(Rc<RefCell<Env>>, Rc<str>),
    /// Not here; continue with the parent (`None` = end of chain).
    Up(Option<Rc<RefCell<Env>>>),
}

/// Query an environment once.
///
/// The point is that it is once: one hash and compare per name and
/// environment on the hot read path, no allocation.
pub fn env_peek(env: &Rc<RefCell<Env>>, n: &str) -> Hit {
    let b = env.borrow();
    if let Some(bd) = b.vars.get(n) {
        return if bd.initialized { Hit::Val(bd.value.clone()) } else { Hit::Dead };
    }
    // The table exists only in module environments; otherwise this is a null
    // check.
    if let Some(t) = b.imports.as_ref().and_then(|m| m.get(n)) {
        return Hit::Import(t.0.clone(), t.1.clone());
    }
    Hit::Up(b.parent.clone())
}

/// The home object of the nearest enclosing method.
pub fn env_home(env: &Rc<RefCell<Env>>) -> Option<Gc> {
    let mut cur = env.clone();
    loop {
        if let Some(h) = cur.borrow().home.clone() { return Some(h); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return None }
    }
}

/// `this`, as the program observes it given the mode.
///
/// Both machines call it so the two cannot diverge. In sloppy mode
/// `undefined`/`null` would be `globalThis` and a primitive would be boxed;
/// the program sees both.
pub fn this_observed(i: &mut Interp, env: &Rc<RefCell<Env>>) -> Value {
    let v = env_this(env);
    match &v {
        Value::Undefined | Value::Null => strict_site!(i, 8),
        Value::Obj(_) => {}
        _ => strict_site!(i, 9),
    }
    v
}

/// Rebind `this` in exactly the environment that carries it.
///
/// Only `super()` needs this: until then `this` in a derived class is not
/// final, and a parent constructor returning an object decides it.
pub fn set_env_this(env: &Rc<RefCell<Env>>, v: Value) {
    let mut cur = env.clone();
    loop {
        if cur.borrow().this_val.is_some() { cur.borrow_mut().this_val = Some(v); return; }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return }
    }
}

pub fn env_this(env: &Rc<RefCell<Env>>) -> Value {
    let mut cur = env.clone();
    loop {
        if let Some(t) = &cur.borrow().this_val { return t.clone(); }
        let next = cur.borrow().parent.clone();
        match next { Some(p) => cur = p, None => return Value::Undefined }
    }
}

/// The built-in objects of one execution unit.
pub struct Realm {
    pub global: Gc,
    pub global_env: Rc<RefCell<Env>>,
    pub object_proto: Gc,
    pub function_proto: Gc,
    pub array_proto: Gc,
    pub string_proto: Gc,
    pub number_proto: Gc,
    pub boolean_proto: Gc,
    pub error_proto: Gc,
    /// Name -> prototype of the error kinds, for `throw_type` and friends.
    pub error_ctors: HashMap<&'static str, Gc>,
    pub node_proto: Gc,
    pub element_proto: Gc,
    pub text_proto: Gc,
    pub document_proto: Gc,
    /// `Event.prototype`. Lives in the realm because dispatch builds events, and
    /// built-in functions are pointers, not closures; they cannot capture it.
    pub event_proto: Gc,
    /// The one `location` object. In the realm for the same reason as
    /// `event_proto`: `window.location = "…"` and `document.location = "…"` are
    /// setters forwarding to `href` ([PutForwards=href]), and a built-in setter
    /// is a pointer that must be able to look the object up.
    pub location: Gc,
    pub token_list_proto: Gc,
    pub comment_proto: Gc,
    pub style_proto: Gc,
    pub regexp_proto: Gc,
    pub symbol_proto: Gc,
    /// `%IteratorPrototype%`, the common ancestor of all built-in iterators. It
    /// carries `[Symbol.iterator]() { return this }`, which makes an iterator
    /// itself iterable.
    pub iterator_proto: Gc,
    /// `%GeneratorPrototype%` (`next`/`return`/`throw`) and
    /// `%GeneratorFunction.prototype%`. In the realm because built-in functions
    /// are pointers, not closures, and cannot capture the prototype.
    pub generator_proto: Gc,
    pub generator_func_proto: Gc,
    /// `%AsyncIteratorPrototype%`, `%AsyncGeneratorPrototype%` and
    /// `%AsyncGeneratorFunction.prototype%`. The first is separate because
    /// `for await` also finds it on a hand-written async iterator.
    pub async_iterator_proto: Gc,
    pub async_gen_proto: Gc,
    pub async_gen_func_proto: Gc,
    /// `WebSocket.prototype`. In the realm even when the interface is absent,
    /// because `instanceof` against a missing name throws instead of giving
    /// `false`.
    pub websocket_proto: Gc,
    pub array_iter_proto: Gc,
    pub string_iter_proto: Gc,
    pub promise_proto: Gc,
    pub date_proto: Gc,
    pub bigint_proto: Gc,
    pub iter_helper_proto: Gc,
    pub iter_wrap_proto: Gc,
    /// The built-in `eval`. Remembered because a call is direct only if it hits
    /// exactly this function.
    pub eval_fn: Option<Gc>,
    /// The interface prototypes of the DOM binding. `tag_protos` maps an element
    /// name to its interface; anything not listed is `HTMLElement`.
    pub html_element_proto: Gc,
    pub svg_element_proto: Gc,
    pub fragment_proto: Gc,
    /// `EventTarget.prototype`, needed because `new EventTarget()` builds a
    /// detached node and `wrap` must give it the right prototype. Going through
    /// the global name would be wrong: the page may overwrite it.
    pub event_target_proto: Gc,
    pub tag_protos: HashMap<&'static str, Gc>,
    pub url_proto: Gc,
    pub url_params_proto: Gc,
    /// `fetch` and what belongs to it; see `fetch.rs`.
    pub response_proto: Gc,
    pub headers_proto: Gc,
    pub abort_signal_proto: Gc,
    pub abort_ctrl_proto: Gc,
    pub xhr_proto: Gc,
    /// `MutationObserver.prototype`, in the realm because the constructor is a
    /// pointer and cannot capture the prototype. Same for the two box
    /// observers.
    pub mo_proto: Gc,
    pub ro_proto: Gc,
    pub formdata_proto: Gc,
    pub intl_dtf_proto: Gc,
    pub intl_nf_proto: Gc,
    pub xpath_result_proto: Gc,
    pub xpath_expr_proto: Gc,
    pub xpath_eval_proto: Gc,
    pub io_proto: Gc,
    /// `Attr` and `NamedNodeMap`; `el.attributes` links both.
    pub attr_proto: Gc,
    pub nnm_proto: Gc,
    pub prej_proto: Gc,
    pub text_encoder_proto: Gc,
    pub text_decoder_proto: Gc,
    /// The prototypes of the nine typed array views, by name. `new_typed` hangs
    /// a fresh view on them; built-in functions are pointers that capture
    /// nothing.
    pub ta_protos: HashMap<&'static str, Gc>,
    pub typed_proto: Gc,
    pub buffer_proto: Gc,
    pub dataview_proto: Gc,
}

/// What a heap census found.
#[cfg(feature = "heap-census")]
pub struct Census {
    /// Objects reachable from the roots.
    pub reachable: usize,
    /// Environments among them; a closure holds its environment, which holds
    /// closures again.
    pub envs: usize,
    /// Properties in the reachable objects; each is stored individually, keyed
    /// by string.
    pub props: usize,
    /// Objects alive in total. Only with `--features heap-census`, otherwise
    /// zero; without it `reachable` means nothing.
    pub live: usize,
}

impl Realm {
    /// Everything the realm itself holds. One list rather than enumerating by
    /// hand at each use: adding a field here shows it must be torn down too.
    fn roots(&self) -> Vec<Gc> {
        alloc::vec![
            self.global.clone(), self.object_proto.clone(), self.function_proto.clone(),
            self.array_proto.clone(), self.string_proto.clone(), self.number_proto.clone(),
            self.boolean_proto.clone(), self.error_proto.clone(), self.node_proto.clone(),
            self.element_proto.clone(), self.text_proto.clone(), self.document_proto.clone(),
            self.event_proto.clone(), self.token_list_proto.clone(), self.style_proto.clone(),
            self.comment_proto.clone(), self.regexp_proto.clone(), self.symbol_proto.clone(),
            self.iterator_proto.clone(), self.generator_proto.clone(),
            self.generator_func_proto.clone(), self.async_iterator_proto.clone(),
            self.async_gen_proto.clone(), self.async_gen_func_proto.clone(),
            self.websocket_proto.clone(),
            self.array_iter_proto.clone(),
            self.string_iter_proto.clone(), self.promise_proto.clone(),
            self.typed_proto.clone(), self.buffer_proto.clone(),
            self.dataview_proto.clone(), self.response_proto.clone(),
            self.headers_proto.clone(), self.abort_signal_proto.clone(),
            self.abort_ctrl_proto.clone(), self.xhr_proto.clone(),
            self.html_element_proto.clone(), self.svg_element_proto.clone(),
            self.fragment_proto.clone(), self.url_proto.clone(), self.url_params_proto.clone(),
            self.event_target_proto.clone(),
        ]
    }
}

impl Drop for Interp {
    fn drop(&mut self) { self.teardown(); }
}

/// The boxes of the last layout, answering `getBoundingClientRect` and the
/// `offset*`/`client*` properties.
///
/// Supplied by the host because geometry comes from layout, which belongs to
/// beak; same design as `StyleCtx` and `set_media`.
///
/// They are the boxes of the last frame: a script that mutates the tree and
/// measures at once sees the previous state. Boxes that do not exist yet are
/// laid out on demand via `relayout`.
pub struct Geometry {
    /// One entry per fragment: an inline box across three lines has three.
    /// `getClientRects` lists them, `getBoundingClientRect` returns their union.
    pub boxes: alloc::rc::Rc<alloc::vec::Vec<crate::layout::ElemRect>>,
    /// The window's scroll position. Boxes are in document coordinates,
    /// `getBoundingClientRect` answers in viewport coordinates.
    pub scroll: (i32, i32),
    /// The scrollable area of the document: as far as painted content reaches.
    ///
    /// Not the union of the boxes: a background image, a shadow or overflowing
    /// text keep the page scrollable too. Layout computes it anyway
    /// (`Layout::height`); guessing it here would be a second answer for the same
    /// number.
    pub content: (i32, i32),
}

/// The cascade context the host supplies.
pub struct StyleCtx {
    pub sheet: alloc::rc::Rc<crate::css::Stylesheet>,
    pub theme: crate::layout::Theme,
    pub viewport_w: f32,
}

/// The places where strict mode differs, in counter order. Diagnostics only
/// (`--features strict-probe`).
pub const STRICT_SITE_NAMES: [&str; STRICT_SITES] = [
    "set: Empfaenger ist ein echtes Primitiv (Text/Zahl/Bool/Symbol)",
    "set: eigene Eigenschaft nicht schreibbar",
    "set: geerbte Eigenschaft nicht schreibbar",
    "set: nur Getter, kein Setzer",
    "set: Objekt nicht erweiterbar",
    "set: die Stellvertreter-Falle sagte nein",
    "Zuweisung an einen unbekannten Namen legt eine globale an",
    "delete gab false zurueck",
    "this ist undefined in einem einfachen Aufruf (locker: globalThis)",
    "this bleibt ein Primitiv (locker: eingepackt)",
    "eval legt sein var im Bereich des Aufrufers ab",
    // Kept separate from a real primitive: almost every hit here means the
    // object does not exist in beak at all (`undefined.foo = 1`). Counting both
    // together would measure a different gap.
    "set: Empfaenger ist undefined/null (meist: das Objekt fehlt ganz)",
];
pub const STRICT_SITES: usize = 12;

/// Record a site. Without the feature this compiles to nothing.
#[cfg(feature = "strict-probe")]
macro_rules! strict_site {
    ($me:expr, $i:expr) => {{ $me.strict_probe[$i] += 1; }};
}
#[cfg(not(feature = "strict-probe"))]
macro_rules! strict_site {
    ($me:expr, $i:expr) => {{ let _ = &$me; }};
}
pub(crate) use strict_site;

/// What a page requested of the history. An intent, not an action; the host,
/// which owns the history, carries it out.
#[derive(Debug, Clone)]
pub enum HistoryOp {
    /// `pushState(state, title, url)`; `url` is resolved or empty if the page
    /// gave none.
    Push { url: String },
    /// `replaceState(...)`: same entry, new address.
    Replace { url: String },
    /// `go(n)`, `back()` (= `go(-1)`), `forward()` (= `go(1)`).
    Go(i32),
}

/// A real navigation requested via `location`.
///
/// Kept apart from `HistoryOp` on purpose: `pushState` only rewrites the
/// address and keeps the document, `location.replace` discards it and loads
/// a new one. One enum for both would invite confusing them.
#[derive(Debug, Clone)]
pub struct NavRequest {
    /// Already resolved against the current address, i.e. absolute.
    pub url: String,
    /// `replace()` replaces the history entry, `assign()`/`href=` appends.
    pub replace: bool,
    /// `reload()`: the same address again.
    pub reload: bool,
}

/// A registered timer.
///
/// `interval` separates `setInterval` from `setTimeout`: only an interval
/// re-registers after running. `args` are the values after the delay;
/// `setTimeout(f, 0, a, b)` calls `f(a, b)`.
pub struct Timer {
    pub(crate) id: u32,
    pub(crate) cb: Value,
    pub(crate) due: f64,
    pub(crate) interval: Option<f64>,
    pub(crate) args: alloc::vec::Vec<Value>,
}

pub struct Interp {
    pub realm: Realm,
    /// The fetched ES modules, by resolved address. See `modules.rs`; the engine
    /// fetches nothing, it only manages.
    pub modules: HashMap<Rc<str>, Rc<RefCell<super::modules::Module>>>,
    /// Which module threw first. An error from a large graph would otherwise
    /// only name the entry point.
    pub module_fail: Option<Rc<str>>,
    /// Stylesheets inserted by script that still need fetching:
    /// `(node, address as in the attribute)`.
    ///
    /// The host takes them with `take_pending_sheets`, resolves, loads and
    /// reports back with `sheet_done`; only then does `load` or `error` fire on
    /// the `<link>`. Without it a page loading its sheet by script waits forever.
    pub pending_sheets: Vec<(u32, String)>,
    /// Scripts inserted by script that still need fetching:
    /// `(node, address as in the attribute)`.
    ///
    /// Same path as `pending_sheets`, for the same reason. A `<script src=…>`
    /// added via `appendChild` is how code-splitting bundlers load their chunks
    /// (webpack's `__webpack_require__.l`), and frameworks wait on them before
    /// rendering anything.
    pub pending_scripts: Vec<(u32, String)>,
    /// The `<script>` node whose code is running: `document.currentScript`.
    ///
    /// Bundlers use it to locate their chunks (e.g.
    /// `TURBOPACK.push([document.currentScript, …])`).
    ///
    /// A module has none: per HTML §4.12.1 the answer is `null`, and
    /// `import.meta.url` is the way. Setting `None` says exactly that.
    pub current_script: Option<u32>,
    /// The name of the constructor being called, for the error message.
    /// `construct_named` sets and clears it.
    pub(crate) new_name: Option<String>,
    /// Where the next `document.write` of the same script writes:
    /// `(script node, last written node)`.
    ///
    /// The pair invalidates itself: if the first value no longer matches
    /// `current_script`, it does not apply. Without it a second `write` would
    /// land before the first.
    pub(crate) write_point: Option<(u32, u32)>,
    /// Script nodes that have already run: the spec's "already started" flag.
    /// A script inserted again does not run again.
    pub(crate) ran_scripts: Vec<u32>,
    /// Requests from `fetch()` whose response is pending. Same pattern as
    /// `pending_sheets`: the engine fetches nothing, it queues the request; the
    /// host takes it with `take_pending_fetches`, loads and reports back with
    /// `fetch_done`/`fetch_failed`.
    pub pending_fetches: Vec<super::fetch::PendingFetch>,
    /// Who waits for which response. Two kinds of waiter, one channel: `fetch`
    /// holds a promise, `XMLHttpRequest` its own object. The host only reports
    /// an id back; `fetch_done` decides here.
    pub(crate) fetch_waiting: Vec<(u32, super::fetch::Waiter)>,
    /// Requests the host should abort. `controller.abort()` puts the id here;
    /// otherwise the abort would only be a flag and the connection would go on.
    pub aborted_fetches: Vec<u32>,
    pub(crate) next_fetch_id: u32,
    /// Forms the page wants to submit (`form.submit()`), as the `<form>`'s `seq`.
    /// The host takes them with `take_submits` and navigates; the engine knows
    /// neither address nor network. Same pattern as `history_ops`.
    pub submits: Vec<u32>,
    /// Rejected promises with nothing attached (yet).
    pub pending_rejections: Vec<Gc>,
    /// `customElements`: the definitions and their construction state.
    pub custom: super::dombind::CeRegistry,
    /// Call depth. The tree walker uses the Rust stack, so a too-deep JS program
    /// would overflow the host stack, which in the kernel is a crash, not an
    /// error. The limit is mandatory.
    pub depth: usize,
    pub max_depth: usize,
    /// Executed statements. Without a cap a `while(true)` hangs the whole run,
    /// and a test runner stuck on one program measures nothing.
    pub steps: u64,
    pub max_steps: u64,
    /// "May computation continue?" Set by the host, queried by the engine every
    /// 65 536 steps.
    ///
    /// A step cap measures the wrong thing: it stops a page that hangs, but also
    /// one that legitimately computes a lot (e.g. many PBKDF2 rounds). Browsers
    /// measure time, not steps; the engine has no clock, so it asks the host.
    ///
    /// Every 65 536 steps, not each one: the call must not cost the hottest path.
    pub deadline: Option<fn() -> bool>,
    /// A monotonic counter, used as the clock when the host supplies none
    /// (`beak-engine` is host-free and has no clock).
    pub fake_now: f64,
    /// The host's real clock, in milliseconds since page start.
    ///
    /// Without it `performance.now()` is a call counter, which breaks
    /// schedulers that yield after a time slice (React checks
    /// `now() - start >= 5`).
    ///
    /// A function pointer like `deadline`. When absent, the rising counter
    /// remains; better than a constant at which every measurement is zero.
    pub clock: Option<fn() -> f64>,
    /// Milliseconds since the epoch, as set by the host at session start. The
    /// engine has no clock; without this `Date.now()` is stuck at 1970.
    pub epoch_ms: f64,
    /// The document `document` operates on. `None` until one is supplied; then
    /// `document` does not exist at all rather than faking an empty one.
    pub doc: Option<super::dombind::Doc>,
    /// The state of `Math.random`.
    ///
    /// The host seeds it, not the engine: `beak-engine` is host-free and has no
    /// entropy source. A fixed seed stands here until `seed_random` supplies a
    /// real one, which also gives the test runner reproducible runs.
    rng: u64,
    /// The media state for `matchMedia`: width and colour scheme preference.
    /// Like `innerWidth` it belongs to the host; without `set_viewport` there is
    /// no `matchMedia`.
    pub media: Option<(f64, bool)>,
    /// What `getComputedStyle` needs: the stylesheet, the tree the document was
    /// built from, the colour scheme and the viewport width.
    ///
    /// Supplied by the host, like the window size and the cookies. Without it
    /// `getComputedStyle` answers from the inline style only: a partial answer
    /// that keeps the page running instead of a TypeError.
    pub style_ctx: Option<StyleCtx>,
    /// How many programs the bytecode machine ran, as opposed to the tree
    /// walker.
    pub vm_ran: u64,
    pub vm_declined: u64,
    /// Why the compiler declined last time. A name, not a sentence: it is
    /// counted, and counting needs a key.
    pub vm_decline: Option<&'static str>,
    /// Off switch for the bytecode machine. Only for cross-checking: the same
    /// run with and without it shows in a diff which tests it loses.
    pub vm_off: bool,
    /// Compiled function bodies, by the address of their AST node.
    ///
    /// An `Rc<Func>` is a function's identity in the source; the same body is
    /// needed on every call and must be compiled once. `None` means "tried,
    /// cannot", which is also remembered so a loop does not retry each time.
    ///
    /// The `Weak` is what makes the key sound. An address is an identity only
    /// while occupied: once the last `Rc` frees the node, the next function can
    /// land exactly there and would get its predecessor's body. A `Weak` keeps
    /// the allocation occupied without keeping the tree alive.
    pub func_chunks: HashMap<usize, (alloc::rc::Weak<Func>, Option<Rc<super::code::Chunk>>)>,
    /// The template objects of tagged templates (ES 13.2.8.4).
    ///
    /// The same source location must get the same object on every evaluation.
    /// This is observable, and libraries such as lit-html key a `WeakMap` with
    /// the `strings` array.
    ///
    /// Keyed by the node's address. Because an address is only an identity
    /// while occupied, the raw strings are stored alongside and compared on a
    /// hit: if a tree is dropped and another template lands at the same address,
    /// the strings differ and the object is rebuilt. If they are equal, sharing
    /// is unobservable.
    pub templates: HashMap<usize, (Vec<Rc<str>>, Value)>,
    /// The open `WebSocket`s. The engine opens no connection; this holds its
    /// half of the state (handshake, frames, buffers), the host drives the wire.
    /// Same pattern as `pending_fetches`.
    pub sockets: Vec<super::websocket::Socket>,
    /// The JS object per connection, for event dispatch.
    pub socket_objs: HashMap<u32, Gc>,
    /// What the host still has to set up.
    pub pending_sockets: Vec<super::websocket::PendingSocket>,
    pub next_socket_id: u32,
    /// Why the compiler declines a function body, per reason.
    ///
    /// Program-level declines (`vm_decline`) do not see these: a generator or
    /// async body can decline without the program declining.
    pub func_declines: HashMap<&'static str, u64>,
    /// Labels belonging to the next loop.
    ///
    /// `outer: for (…)` is a label around a loop in the tree, but `continue outer`
    /// belongs to the loop, which alone has a continuation point. The label puts
    /// its name here and the loop takes it on entry. Without it a `continue lbl`
    /// would propagate to the top and silently end the program.
    pub pending_labels: Vec<String>,
    /// Instructions the machine actually executed, for measurement. The step
    /// counter counts statements, not instructions.
    pub vm_ops: u64,
    /// May the lookup hints in `Chunk::hints` be used?
    ///
    /// A direct `eval` disables them for the whole session. It is the only thing
    /// that can add a binding to an inner environment after an instruction has
    /// run, and then a hint would skip the shadowing binding. Checking for that
    /// would cost the full chain walk the hint saves.
    pub hints_ok: bool,
    pub vm_calls: u64,
    /// Native, bound and getter calls: they have no bytecode body and never
    /// will, so they are not counted with JS functions the compiler cannot
    /// handle yet.
    pub vm_calls_native: u64,
    pub vm_calls_slow: u64,
    /// See `Geometry`. `None` means the host supplied none; geometry then
    /// answers with zeros.
    pub geometry: Option<Geometry>,
    /// Lay out on demand. The host installs this, like `clock`; without a hook
    /// nothing changes.
    ///
    /// `geometry` is the last frame. An element the script inserted in the same
    /// step is not in it and reports 0, and a page that measures only once (e.g.
    /// in a mount effect) stays wrong forever. Browsers relayout synchronously
    /// here.
    pub relayout: Option<fn(&mut Interp)>,
    /// How often layout was forced since the host's last frame. `set_geometry`
    /// resets it, but only when called by the host, not by the hook itself.
    pub forced_layouts: u32,
    /// Is a forced layout running? Guards against recursion (the hook calls
    /// `set_geometry`, where box observers ask for boxes again) and against
    /// resetting the cap.
    pub in_forced_layout: bool,
    /// The live tree in the form the cascade can read, built from `doc` and
    /// rebuilt only when `doc.version` changes. There is a single tree, so a
    /// script that sets a class and then measures sees its own change.
    pub live_dom: core::cell::RefCell<Option<(u32, alloc::rc::Rc<crate::dom::Dom>)>>,
    /// Only with `--features strict-probe`. Counts, per site, the places where
    /// strict mode would do something different from sloppy mode.
    ///
    /// Test flags (`onlyStrict`) do not answer this: they say how a test starts,
    /// not whether it passes one of these sites. A test without the flag that
    /// writes `"use strict"` in its body hits them just the same.
    #[cfg(feature = "strict-probe")]
    pub strict_probe: [u32; STRICT_SITES],
    /// This page's cookies as `document.cookie` shows them.
    ///
    /// The host supplies them; the engine has no jar. The jar knows domain,
    /// path, `Secure` and `HttpOnly`, and which of them this document may see is
    /// its decision. Empty if nobody supplied any: `document.cookie` still exists
    /// as an empty string, because scripts call `.match` on it. No cookies is a
    /// real answer.
    pub cookies: String,
    /// What the page set with `document.cookie = "…"`, raw and in order. The host
    /// takes it with `take_cookie_sets` and stores it in its jar; the engine does
    /// not decide what applies.
    pub cookie_sets: Vec<String>,
    /// The registered `MutationObserver`s. They live here, not in the document,
    /// because they hold a JS callback: the document is rebuilt on every
    /// navigation, the realm is not.
    pub observers: Vec<super::dombind::MutObs>,
    /// The last parsed XPath expression, keyed by its source.
    ///
    /// `createExpression` exists because a page parses one expression at load
    /// and evaluates it often (htmx does on every `process`). One entry suffices;
    /// more would be a table nobody cleans.
    pub xpath_memo: Option<(alloc::string::String, alloc::rc::Rc<super::xpath::XPath>)>,
    /// The registered `ResizeObserver`s and `IntersectionObserver`s. Both are
    /// evaluated in `set_geometry`, where the host says how the page stands now.
    pub resize_obs: Vec<super::dombind::ResizeObs>,
    pub inter_obs: Vec<super::dombind::InterObs>,
    /// Width and height of the viewport as `set_viewport` knows them; the root
    /// of an `IntersectionObserver` without its own root.
    pub viewport: (f64, f64),
    /// What the page requested via `history.pushState`/`replaceState`/`go`, raw
    /// and in order. The engine does not navigate and keeps no history; it
    /// collects, and the host takes it with `take_history_ops` and decides. Same
    /// pattern as the cookies.
    pub history_ops: Vec<HistoryOp>,
    /// Where the page wants to scroll (`scrollTo`, `scrollIntoView`,
    /// `scrollTop =`). The engine has no window and does not scroll; the host
    /// takes it with `take_scroll`. Per axis, because `scrollTo({ top: 0 })`
    /// must leave the other alone.
    pub scroll_want: Option<(Option<f64>, Option<f64>)>,
    /// The navigation the page requested last (`location.assign`, `.replace`,
    /// `.reload`, `href = …`). Exactly one, and the last wins: in a browser a
    /// second navigation aborts the first, so no queue may run both.
    ///
    /// The engine does not navigate; the host takes it with `take_nav`.
    pub nav: Option<NavRequest>,
    /// The document address, the single source behind `location`.
    ///
    /// `location` is built entirely from accessors for this reason: a page
    /// setting `location.pathname` also changes `href`, and two copies would
    /// diverge immediately.
    pub loc_href: String,
    /// `history.state`, the state the page set last. It belongs to the document,
    /// not to the host's history, and therefore lives here.
    pub history_state: Value,
    /// `history.length`, supplied by the host. Defaults to 1: a freshly loaded
    /// document is always at least one entry.
    pub history_len: f64,
    /// Running number for `Symbol()`.
    pub next_sym: u32,
    /// The global symbol registry behind `Symbol.for`/`Symbol.keyFor`.
    pub sym_registry: HashMap<Rc<str>, Value>,
    /// The microtask queue. It sits beside `timers`, not inside: a microtask
    /// runs before the next timer, which is the whole difference between
    /// `Promise.resolve().then(f)` and `setTimeout(f, 0)`.
    pub jobs: alloc::collections::VecDeque<super::promise::Job>,
    /// Registered timers, run by `run_timers` in due order.
    ///
    /// The delay matters: order is by due time, not registration, and
    /// `clearTimeout` must really remove the entry. Chunk loaders register a long
    /// timeout and clear it in `onload`.
    pub timers: Vec<Timer>,
    /// The clock the timers run on. It does not advance by itself: when nothing
    /// is due and something is pending, it jumps to the next deadline. The order
    /// is always right even without a host clock, and order is what real code
    /// depends on.
    pub(crate) vnow: f64,
    /// The next id. Starts at 1 because 0 is falsy and pages write `if (id)`.
    pub(crate) next_timer: u32,
    /// Is a `new` on a built-in constructor running? A native `this` is the same
    /// (`undefined`) for call and construct, so this flag is needed: `Symbol`
    /// has a `[[Construct]]`, it just throws inside it.
    pub native_new: bool,
    /// The last successful match, only for the Annex B statics `RegExp.$1`,
    /// `RegExp.lastMatch` and friends. They live on the constructor, not on the
    /// expression object, so the state lives here.
    pub last_match: Option<LastMatch>,
    /// What the page wrote to `console`.
    ///
    /// Collected rather than discarded: `beak-engine` has no serial line, but the
    /// host does, and a page reporting its own state is often the only window
    /// into it. Capped so a page cannot fill memory with it; the loss is
    /// reported.
    pub console: Vec<String>,
    /// How many lines the cap discarded. Public, because a silent cap turns
    /// every investigation into measuring the cap.
    pub console_dropped: usize,
}

/// What the nine `RegExp.$n` and their four neighbours need. Finished
/// strings rather than ranges, so the source may go away.
pub struct LastMatch {
    pub input: String,
    pub matched: String,
    pub left: String,
    pub right: String,
    /// `$1` to `$9`; a group without a match is the empty string.
    pub caps: Vec<String>,
    pub last_paren: String,
}

/// How many lines `console` keeps, and how long one may be.
pub const MAX_CONSOLE_LINES: usize = 200;
pub const MAX_CONSOLE_LEN: usize = 512;

pub const MAX_DEPTH: usize = 400;

/// How far a prototype chain may be walked.
///
/// A safety net, not a language rule. A chain could contain a cycle
/// (`Object.setPrototypeOf` must refuse it, but nothing here relies on that
/// alone), and then every property access would loop forever in native code,
/// past the step limit. Real chains are a dozen links deep.
pub const MAX_PROTO_CHAIN: usize = 1000;

/// Maximum size of a single `ArrayBuffer`.
///
/// In a kernel there is no process that dies alone on `new ArrayBuffer(2**53)`.
/// Exceeding the limit is a `RangeError`, as a real engine gives when
/// allocation fails.
pub const MAX_BUFFER_BYTES: usize = 64 << 20;

/// Step budget a test runner sets.
///
/// An ordinary test262 test needs on the order of a hundred steps; this
/// leaves large headroom while keeping tests that deliberately use huge
/// array lengths cheap. Anything exceeding it shows up as "step budget
/// exhausted" in the error map.
pub const TEST_STEPS: u64 = 200_000;

impl Interp {
    pub fn new() -> Interp {
        let mut realm = super::builtins::make_realm();
        super::dombind::install(&mut realm);
        super::regexp::install(&mut realm);
        super::json::install(&mut realm);
        super::promise::install(&mut realm);
        super::date::install(&mut realm);
        super::iterhelp::install(&mut realm);
        super::proxy::install(&mut realm);
        realm.eval_fn = match realm.global.borrow().get_own("eval").and_then(|p| p.value.clone()) {
            Some(Value::Obj(o)) => Some(o), _ => None,
        };
        super::url::install(&mut realm);
        super::fetch::install(&mut realm);
        let mut me = Interp::with_realm(realm);
        // `WebSocket` only appears with real randomness: each frame needs a mask
        // (RFC 6455 §5.3), and a predictable one would be worse than a missing
        // interface. Same rule as `crypto`; it is here because `install` needs the
        // finished interpreter, not just the realm.
        super::websocket::install(&mut me);
        me
    }

    fn with_realm(realm: Realm) -> Interp {
        Interp { realm, modules: HashMap::new(), module_fail: None, submits: Vec::new(),
                 deadline: None,
                 pending_sheets: Vec::new(),
                 pending_scripts: Vec::new(),
                 current_script: None,
                 new_name: None,
                 write_point: None,
                 ran_scripts: Vec::new(),
                 pending_fetches: Vec::new(), fetch_waiting: Vec::new(),
                 aborted_fetches: Vec::new(), next_fetch_id: 1,
                 pending_rejections: Vec::new(), custom: Default::default(), depth: 0, max_depth: MAX_DEPTH, steps: 0, max_steps: u64::MAX,
                 fake_now: 0.0, clock: None, epoch_ms: 0.0, doc: None, next_sym: 0, sym_registry: HashMap::new(),
                 #[cfg(feature = "strict-probe")]
                 strict_probe: [0; STRICT_SITES],
                 cookies: String::new(), cookie_sets: Vec::new(), style_ctx: None,
                 history_ops: Vec::new(), history_state: Value::Null, history_len: 1.0,
                 nav: None, loc_href: String::from("about:blank"),
                 observers: Vec::new(), xpath_memo: None,
                 resize_obs: Vec::new(), inter_obs: Vec::new(),
                 viewport: (0.0, 0.0), scroll_want: None,
                 vm_ran: 0, vm_declined: 0, vm_decline: None, vm_off: false,
                 func_chunks: HashMap::new(), templates: HashMap::new(),
                 sockets: Vec::new(), socket_objs: HashMap::new(),
                 pending_sockets: Vec::new(), next_socket_id: 1,
                 func_declines: HashMap::new(), pending_labels: Vec::new(), vm_ops: 0, hints_ok: true, vm_calls: 0, vm_calls_native: 0, vm_calls_slow: 0,
                 geometry: None,
                 relayout: None,
                 forced_layouts: 0,
                 in_forced_layout: false,
                 live_dom: core::cell::RefCell::new(None),
                 jobs: alloc::collections::VecDeque::new(),
                 rng: 0x2545_F491_4F6C_DD1D, media: None,
                 timers: Vec::new(), vnow: 0.0, next_timer: 1, native_new: false, last_match: None, console: Vec::new(), console_dropped: 0 }
    }

    /// Is an observation waiting for its callback?
    ///
    /// The host asks after every frame. `set_geometry` measures, but only an
    /// entry point may deliver; without this a page with neither timers nor
    /// events would register observers and never hear back. Cheap when nobody
    /// observes: two empty lists.
    pub fn box_observations_pending(&self) -> bool {
        self.resize_obs.iter().any(|o| !o.queue.is_empty())
            || self.inter_obs.iter().any(|o| !o.queue.is_empty())
    }

    /// Run the registered timers once.
    ///
    /// Once, not until the queue is empty: a `setTimeout` that re-registers
    /// itself is a normal pattern (polling, animation) and would never leave
    /// the loop. Whatever is added during the run is due next time.
    pub fn run_timers(&mut self) -> usize {
        // Microtasks first, then timers; that is the priority. Without this a
        // `Promise.resolve().then(f)` from an event handler would wait until some
        // timer happened to be due: `run_timers` would not get there with an empty
        // timer list.
        super::promise::run_jobs(self);
        if self.timers.is_empty() { return 0 }
        // If nothing is due, the clock jumps to the next deadline. Not real time,
        // but the right order, and a callback nobody cancels must still run.
        //
        // Not while something is in flight: a waiter must not advance the clock.
        // Chunk loaders register a long timeout next to each fetch and clear it in
        // `onload`; jumping the clock while the chunk is still loading would fire the
        // timeout before delivery.
        let waiting = !self.pending_scripts.is_empty()
            || !self.pending_sheets.is_empty()
            || !self.pending_fetches.is_empty();
        let first = self.timers.iter().map(|t| t.due).fold(f64::INFINITY, f64::min);
        if !waiting && first > self.vnow { self.vnow = first; }
        if self.timers.iter().all(|t| t.due > self.vnow) { return 0 }
        let now = self.vnow;
        let mut due: Vec<Timer> = Vec::new();
        let mut keep: Vec<Timer> = Vec::new();
        for t in core::mem::take(&mut self.timers) {
            if t.due <= now { due.push(t) } else { keep.push(t) }
        }
        self.timers = keep;
        // Equal due time means registration order.
        due.sort_by(|a, b| a.due.partial_cmp(&b.due).unwrap_or(core::cmp::Ordering::Equal)
                            .then(a.id.cmp(&b.id)));
        let n = due.len();
        for t in due {
            // A `setInterval` re-registers itself before running, so a `clearInterval`
            // in the callback catches it.
            if let Some(iv) = t.interval {
                self.timers.push(Timer { id: t.id, cb: t.cb.clone(), due: now + iv.max(1.0),
                                         interval: Some(iv), args: t.args.clone() });
            }
            let f = t.cb;
            let args = t.args;
            // A timer that throws must report it; otherwise a failure in a callback
            // looks like a missing feature.
            if let Err(e) = self.call(&f, Value::Undefined, &args) {
                let msg = super::modules::describe(self, e);
                self.console_push(alloc::format!("Fehler im Zeitgeber: {msg}"));
            }
            // After every timer, not after all: microtasks run between tasks, and a
            // `then` created by the first timer belongs before the second.
            super::promise::run_jobs(self);
        }
        n
    }

    /// Count what is reachable from the roots.
    ///
    /// The same walk `teardown` does, counting instead of tearing down. `Rc`
    /// does not collect cycles (see the head of `value.rs`), and framework data
    /// structures such as React's fiber tree are cycles. The question before any
    /// collector is how much the page still holds, and how much only holds
    /// itself.
    ///
    /// The roots are all of the interpreter's, not just the realm's: a timer, an
    /// observer, a pending `fetch` and every handler on the tree hold objects
    /// too. Omitting one counts live data as garbage.
    #[cfg(feature = "heap-census")]
    pub fn heap_census(&self) -> Census {
        let (seen, envs, props) = self.walk_roots();
        Census {
            reachable: seen.len(),
            envs,
            props,
            live: {
                #[cfg(feature = "heap-census")]
                { unsafe { (&raw const super::value::LIVE_OBJECTS).read() } }
                #[cfg(not(feature = "heap-census"))]
                { 0 }
            },
        }
    }

    /// Mark only: what `collect_cycles` must keep.
    #[cfg(feature = "heap-census")]
    fn mark_reachable(&self) -> HashSet<usize> { self.walk_roots().0 }

    /// The walk itself: from all roots, touching nothing.
    ///
    /// Stays out of the shipped module until a collector needs it: diagnostics
    /// must not affect the product.
    #[cfg(feature = "heap-census")]
    fn walk_roots(&self) -> (HashSet<usize>, usize, usize) {
        fn add(v: &Value, objs: &mut Vec<Gc>) {
            if let Value::Obj(o) = v { objs.push(o.clone()); }
        }
        let mut objs: Vec<Gc> = alloc::vec![self.realm.global.clone()];
        objs.extend(self.realm.roots());
        let mut env_stack: Vec<Rc<RefCell<Env>>> = alloc::vec![self.realm.global_env.clone()];

        add(&self.history_state, &mut objs);
        for v in self.sym_registry.values() { add(v, &mut objs); }
        for v in self.custom.values() { add(&v, &mut objs); }
        for (_, (_, v)) in &self.templates { add(v, &mut objs); }
        objs.extend(self.pending_rejections.iter().cloned());
        objs.extend(self.socket_objs.values().cloned());
        for o in &self.observers { objs.push(o.js.clone()); add(&o.cb, &mut objs); }
        for o in &self.resize_obs { objs.push(o.js.clone()); add(&o.cb, &mut objs); }
        for o in &self.inter_obs { objs.push(o.js.clone()); add(&o.cb, &mut objs); }
        for t in &self.timers {
            add(&t.cb, &mut objs);
            for a in &t.args { add(a, &mut objs); }
        }
        for j in &self.jobs {
            match j {
                super::promise::Job::React { r, arg, .. } => {
                    if let Some(h) = &r.handler { add(h, &mut objs); }
                    objs.push(r.derived.clone());
                    if let Some((a, b)) = &r.cap { add(a, &mut objs); add(b, &mut objs); }
                    add(arg, &mut objs);
                }
                super::promise::Job::Adopt { thenable, then, target } => {
                    add(thenable, &mut objs); add(then, &mut objs); objs.push(target.clone());
                }
            }
        }
        for (_, w) in &self.fetch_waiting {
            match w {
                super::fetch::Waiter::Promise(g) | super::fetch::Waiter::Xhr(g) =>
                    objs.push(g.clone()),
            }
        }
        for m in self.modules.values() { env_stack.push(m.borrow().env.clone()); }
        // The tree holds too: every wrapper, handler and `on…` value is a root, and
        // in an application most data hangs there.
        if let Some(d) = &self.doc {
            for n in &d.nodes {
                if let Some(g) = &n.js { objs.push(g.clone()); }
                for (_, v) in &n.listeners { add(v, &mut objs); }
                for (_, v) in &n.handlers { add(v, &mut objs); }
            }
        }

        let mut seen: HashSet<usize> = HashSet::new();
        let mut envs: HashSet<usize> = HashSet::new();
        let mut props = 0usize;
        while let Some(o) = objs.pop() {
            if !seen.insert(Rc::as_ptr(&o) as usize) { continue }
            let b = o.borrow();
            if let Some(p) = &b.proto { objs.push(p.clone()); }
            for k in b.own_keys() {
                props += 1;
                let Some(pr) = b.get_own(&k) else { continue };
                for v in [&pr.value, &pr.get, &pr.set].into_iter().flatten() {
                    add(v, &mut objs);
                }
            }
            match &b.kind {
                ObjKind::Function(d) => {
                    env_stack.push(d.env.clone());
                    if let Some(Value::Obj(x)) = &d.this_val { objs.push(x.clone()); }
                    if let Some(h) = &d.home_object { objs.push(h.clone()); }
                }
                ObjKind::Generator(g) => g.roots(&mut objs, &mut env_stack),
                ObjKind::Bound { target, this_val, args } => {
                    objs.push(target.clone());
                    add(this_val, &mut objs);
                    for a in args { add(a, &mut objs); }
                }
                // A promise holds its handlers. `teardown` skips them; for the census they
                // would otherwise count as garbage though they are needed.
                ObjKind::Promise(d) => {
                    let d = d.borrow();
                    for r in d.on_ok.iter().chain(d.on_err.iter()) {
                        if let Some(h) = &r.handler { add(h, &mut objs); }
                        objs.push(r.derived.clone());
                        if let Some((a, c)) = &r.cap { add(a, &mut objs); add(c, &mut objs); }
                    }
                }
                ObjKind::Proxy(c) => {
                    if let Some((t, h)) = c.borrow().as_ref() {
                        objs.push(t.clone()); objs.push(h.clone());
                    }
                }
                _ => {}
            }
        }
        while let Some(e) = env_stack.pop() {
            if !envs.insert(Rc::as_ptr(&e) as usize) { continue }
            let b = e.borrow();
            if let Some(p) = &b.parent { env_stack.push(p.clone()); }
            for v in b.vars.values() {
                if let Value::Obj(x) = &v.value {
                    if seen.insert(Rc::as_ptr(x) as usize) { objs.push(x.clone()); }
                }
            }
            // What the bindings added must still go through the object walk, or its
            // environments would be missing.
            while let Some(o) = objs.pop() {
                let b2 = o.borrow();
                for k in b2.own_keys() {
                    props += 1;
                    let Some(pr) = b2.get_own(&k) else { continue };
                    for v in [&pr.value, &pr.get, &pr.set].into_iter().flatten() {
                        if let Value::Obj(x) = v {
                            if seen.insert(Rc::as_ptr(x) as usize) { objs.push(x.clone()); }
                        }
                    }
                }
                if let Some(p) = &b2.proto {
                    if seen.insert(Rc::as_ptr(p) as usize) { objs.push(p.clone()); }
                }
                if let ObjKind::Function(d) = &b2.kind { env_stack.push(d.env.clone()); }
            }
        }
        (seen, envs.len(), props)
    }

    /// Break the cycles not reachable from any root.
    ///
    /// Mark and sweep, with `Rc` doing the freeing. The walk is the one in
    /// `heap_census`; whatever it did not find is cleared (values, prototype,
    /// kind set to `Plain`). Then no cycle refers to itself and the counts reach
    /// zero on their own.
    ///
    /// The sweep goes over `value::ALL_OBJECTS`: a collector must be able to
    /// enumerate what exists.
    ///
    /// Returns `(cleared, remaining)`.
    ///
    /// Measurement only, not a collector in use: anyone still holding a `Value`
    /// of a cleared object holds an empty one. Same warning as `teardown`.
    #[cfg(feature = "heap-census")]
    pub fn collect_cycles(&mut self) -> (usize, usize) {
        let keep = self.mark_reachable();
        let dead: alloc::vec::Vec<Gc> = unsafe {
            (&mut *(&raw mut super::value::ALL_OBJECTS)).iter()
                .filter_map(|w| w.upgrade())
                .filter(|g| !keep.contains(&(Rc::as_ptr(g) as usize)))
                .collect()
        };
        let n = dead.len();
        for o in &dead {
            let mut b = o.borrow_mut();
            b.clear_props();
            b.proto = None;
            b.kind = ObjKind::Plain;
        }
        drop(dead);
        unsafe {
            let all = &mut *(&raw mut super::value::ALL_OBJECTS);
            all.retain(|w| w.strong_count() > 0);
            (n, all.len())
        }
    }

    /// Tear down the realm and break its cycles.
    ///
    /// `Rc` counts references, and a JS realm is full of cycles:
    /// `proto.constructor` points to the constructor, `ctor.prototype` back;
    /// `globalThis` points to itself; every closure holds its environment,
    /// and the global environment holds the closure. A count never reaches
    /// zero inside a cycle.
    ///
    /// Not a collector but a teardown from the roots: everything reachable
    /// from the global object and the prototypes is visited once and cleared.
    /// Then no cycle refers to itself, and `Rc` frees the rest.
    ///
    /// Anyone still holding a `Value` from this machine after the drop holds
    /// an empty object. That is memory-safe (the allocation lives as long as
    /// the `Rc`), but it is no longer the same object.
    fn teardown(&mut self) {
        // Template objects hang on no root, so the walk below does not find them.
        // They only refer to strings and `array_proto`; dropping them suffices.
        self.templates.clear();
        // An open connection holds its JS object, which holds its handlers; a cycle
        // the walk below does not find.
        self.sockets.clear();
        self.socket_objs.clear();
        self.pending_sockets.clear();
        let mut seen: HashSet<usize> = HashSet::new();
        let mut envs: HashSet<usize> = HashSet::new();
        let mut objs: Vec<Gc> = alloc::vec![self.realm.global.clone()];
        let mut env_stack: Vec<Rc<RefCell<Env>>> = alloc::vec![self.realm.global_env.clone()];
        // The prototypes live in the realm and are not always reachable from the
        // global object (`event_proto` hangs on its constructor, but that path is
        // not guaranteed).
        objs.extend(self.realm.roots());
        let mut all: Vec<Gc> = Vec::new();
        while let Some(o) = objs.pop() {
            if !seen.insert(Rc::as_ptr(&o) as usize) { continue }
            {
                let b = o.borrow();
                if let Some(p) = &b.proto { objs.push(p.clone()); }
                for k in b.own_keys() {
                    let Some(pr) = b.get_own(&k) else { continue };
                    for v in [&pr.value, &pr.get, &pr.set].into_iter().flatten() {
                        if let Value::Obj(x) = v { objs.push(x.clone()); }
                    }
                }
                match &b.kind {
                    ObjKind::Function(d) => {
                        env_stack.push(d.env.clone());
                        if let Some(Value::Obj(x)) = &d.this_val { objs.push(x.clone()); }
                        if let Some(h) = &d.home_object { objs.push(h.clone()); }
                    }
                    // A suspended generator holds environments and half-finished values in its
                    // machine. They are in no property and no binding; skipping them here
                    // leaves an Rc cycle.
                    ObjKind::Generator(g) => {
                        g.roots(&mut objs, &mut env_stack);
                    }
                    ObjKind::Bound { target, this_val, args } => {
                        objs.push(target.clone());
                        if let Value::Obj(x) = this_val { objs.push(x.clone()); }
                        for a in args { if let Value::Obj(x) = a { objs.push(x.clone()); } }
                    }
                    _ => {}
                }
            }
            all.push(o);
        }
        // The environments too: a closure holds its own, which holds closures again
        // through its bindings.
        let mut all_envs: Vec<Rc<RefCell<Env>>> = Vec::new();
        while let Some(e) = env_stack.pop() {
            if !envs.insert(Rc::as_ptr(&e) as usize) { continue }
            {
                let b = e.borrow();
                if let Some(p) = &b.parent { env_stack.push(p.clone()); }
                for v in b.vars.values() {
                    if let Value::Obj(x) = &v.value {
                        if seen.insert(Rc::as_ptr(x) as usize) { all.push(x.clone()); }
                        // Objects from environments can hold environments themselves; same
                        // treatment.
                        if let ObjKind::Function(d) = &x.borrow().kind { env_stack.push(d.env.clone()); }
                    }
                }
            }
            all_envs.push(e);
        }
        for o in &all {
            let mut b = o.borrow_mut();
            b.clear_props();
            b.proto = None;
            b.kind = ObjKind::Plain;
        }
        for e in &all_envs {
            let mut b = e.borrow_mut();
            b.vars.clear();
            b.parent = None;
            b.this_val = None;
            b.home = None;
        }
    }

    /// Supply a document and make `document` globally visible.
    ///
    /// `document` only appears here; before that the name does not exist, and a
    /// script checking for it gets the truth rather than an empty shell.
    pub fn set_document(&mut self, doc: super::dombind::Doc) {
        let root = doc.doc;
        self.doc = Some(doc);
        let v = super::dombind::wrap(self, root);
        self.realm.global.borrow_mut().define("document", Prop::builtin(v));
    }

    /// Accept a line from the page.
    pub fn console_push(&mut self, line: String) {
        if self.console.len() >= MAX_CONSOLE_LINES {
            self.console_dropped += 1;
            return;
        }
        let mut l = line;
        if l.len() > MAX_CONSOLE_LEN {
            l.truncate(MAX_CONSOLE_LEN);
            l.push_str(" …");
        }
        self.console.push(l);
    }

    /// Take the collected lines. If anything was discarded, the last line says
    /// so; otherwise capped output would read as complete.
    pub fn take_console(&mut self) -> Vec<String> {
        let mut out = core::mem::take(&mut self.console);
        if self.console_dropped > 0 {
            out.push(alloc::format!("… {} weitere Zeilen verworfen", self.console_dropped));
            self.console_dropped = 0;
        }
        out
    }

    /// A real seed from the host. Without it `Math.random` yields the same
    /// sequence every time: visibly deterministic rather than invisibly bad.
    pub fn seed_random(&mut self, seed: u64) {
        self.rng = seed | 1;
    }

    /// xorshift64*. A number in [0,1), as the spec requires. Not cryptographic
    /// randomness and not meant as such; `crypto.getRandomValues` is separate
    /// and depends on the host.
    pub fn next_random(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.rng = x;
        // The top 53 bits: exactly the precision of an f64 fraction.
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D)) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Supply this page's cookies: what `document.cookie` reads.
    ///
    /// The host passes the script view (`cookies::script_header_for`), not the
    /// `Cookie:` header: an `HttpOnly` cookie travels with requests but must
    /// never appear in a script.
    pub fn set_cookies(&mut self, jar: String) {
        self.cookies = jar;
    }

    /// Stylesheets inserted by script that the host should fetch; the list is
    /// empty afterwards.
    pub fn take_pending_sheets(&mut self) -> Vec<(u32, String)> {
        core::mem::take(&mut self.pending_sheets)
    }

    /// Inserted scripts waiting to be fetched. The host takes them and reports
    /// back with `dombind::script_done`.
    pub fn take_pending_scripts(&mut self) -> Vec<(u32, String)> {
        core::mem::take(&mut self.pending_scripts)
    }

    /// What `fetch()` wants to send. The host takes it; the list is empty
    /// afterwards.
    pub fn take_pending_fetches(&mut self) -> Vec<super::fetch::PendingFetch> {
        core::mem::take(&mut self.pending_fetches)
    }

    /// Which requests should be aborted.
    pub fn take_aborted_fetches(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.aborted_fetches)
    }

    pub fn take_submits(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.submits)
    }

    /// What the page wanted to do with the history. The host takes it and
    /// decides; the list is empty afterwards.
    pub fn take_history_ops(&mut self) -> Vec<HistoryOp> {
        core::mem::take(&mut self.history_ops)
    }

    /// Take the requested navigation. Only the host can carry it out; it has
    /// network and history.
    pub fn take_nav(&mut self) -> Option<NavRequest> {
        self.nav.take()
    }

    /// The host supplies how long its history is and which state the current
    /// entry carries, on load and after every traversal.
    pub fn set_history(&mut self, len: f64, state: Value) {
        self.history_len = len;
        self.history_state = state;
    }

    /// Remember a scroll request. The last one wins, as with navigation: in a
    /// browser a second jump aborts the first. An axis without a request keeps
    /// the earlier one.
    pub fn want_scroll(&mut self, x: Option<f64>, y: Option<f64>) {
        let (px, py) = self.scroll_want.unwrap_or((None, None));
        self.scroll_want = Some((x.or(px), y.or(py)));
    }

    /// What the page requested; consumed by the call.
    pub fn take_scroll(&mut self) -> Option<(Option<f64>, Option<f64>)> {
        self.scroll_want.take()
    }

    /// Take what the page set. Raw declarations (`name=value; Path=/;
    /// Max-Age=…`); the jar knows the rules.
    pub fn take_cookie_sets(&mut self) -> Vec<String> {
        core::mem::take(&mut self.cookie_sets)
    }

    /// Supply the cascade context so `getComputedStyle` can return real values
    /// instead of only the inline style.
    pub fn set_style_context(&mut self, ctx: StyleCtx) {
        self.style_ctx = Some(ctx);
    }

    /// Supply the boxes of the last layout; see `Geometry`. The scroll position
    /// changes without layout, so it is part of it and updated on every call.
    pub fn set_geometry(&mut self, g: Geometry) {
        self.geometry = Some(g);
        // A host frame restores the budget; a forced layout does not, or the cap
        // would be none.
        if !self.in_forced_layout { self.forced_layouts = 0; }
        // Here and nowhere else: `ResizeObserver` and `IntersectionObserver` depend
        // on the box, not on time, and the box is final exactly now.
        super::dombind::eval_box_observers(self);
    }

    /// Supply the page address. Fills `location` and `document.URL`.
    pub fn set_location(&mut self, url: &str) {
        let p = super::url::parse_abs(url).unwrap_or_else(|| super::url::Parts {
            scheme: alloc::string::String::from("about"),
            path: alloc::string::String::from("blank"),
            ..Default::default()
        });
        // Only one line writes the address. `location` is built entirely from
        // accessors reading `loc_href`; writing the parts here as data fields would
        // create a second copy that diverges at the first `location.pathname = …`.
        self.loc_href = p.href();
        let href = p.href();
        self.realm.global.borrow_mut().define("origin", Prop::builtin(Value::str(&p.origin())));
        let Some(Value::Obj(d)) = self.realm.global.borrow().get_own("document").and_then(|p| p.value.clone())
        else { return };
        let fp = self.realm.function_proto.clone();
        let mut d = d.borrow_mut();
        d.define("URL", Prop::builtin(Value::str(&href)));
        d.define("documentURI", Prop::builtin(Value::str(&href)));
        // `document.location` is the same object as `window.location`, and assigning
        // to it navigates instead of replacing it; same rule as on the window
        // ([PutForwards=href]).
        let g = super::value::native(Some(fp.clone()),
            |i, _, _| Ok(Value::Obj(i.realm.location.clone())), "location", 0, false);
        let st = super::value::native(Some(fp),
            |i, _, a| super::builtins::loc_put_forwards(i, a.first()), "location", 1, false);
        d.define("location", Prop { value: None, get: Some(Value::Obj(g)),
            set: Some(Value::Obj(st)), writable: false, enumerable: true, configurable: false });
    }

    /// Like `set_viewport`, plus the colour scheme. `matchMedia` needs both, and a
    /// `prefers-color-scheme` that always says light would be an invented answer.
    pub fn set_media(&mut self, w: f64, h: f64, dark: bool) {
        self.set_viewport(w, h);
        self.media = Some((w, dark));
    }

    /// Supply the window size.
    ///
    /// `beak-engine` has none; it belongs to the host. An invented number
    /// would be worse than none: it would look like a measurement.
    pub fn set_viewport(&mut self, w: f64, h: f64) {
        if self.media.is_none() { self.media = Some((w, false)); }
        self.viewport = (w, h);
        let g = self.realm.global.clone();
        let mut o = g.borrow_mut();
        for (k, v) in [("innerWidth", w), ("innerHeight", h),
                       ("outerWidth", w), ("outerHeight", h),
                       ("devicePixelRatio", 1.0)] {
            o.define(k, Prop::builtin(Value::Num(v)));
        }
        let screen = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut sc = screen.borrow_mut();
            for (k, v) in [("width", w), ("height", h), ("availWidth", w), ("availHeight", h)] {
                sc.define(k, Prop::builtin(Value::Num(v)));
            }
            sc.define("colorDepth", Prop::builtin(Value::Num(24.0)));
            sc.define("pixelDepth", Prop::builtin(Value::Num(24.0)));
        }
        o.define("screen", Prop::builtin(Value::Obj(screen)));
    }

    /// One unit of work in a built-in loop.
    ///
    /// The cap in `exec` counts statements only; the loops in `Array.prototype.*`
    /// and `iterate` bypass it, so `new Array(2**32-1).join()` would hang without
    /// these loops counting too.
    pub fn tick(&mut self) -> C<()> {
        self.steps += 1;
        if self.steps > self.max_steps {
            return Err(self.throw_kind("RangeError", "step budget exhausted"));
        }
        if self.steps & 0xFFFF == 0 { self.check_deadline()?; }
        Ok(())
    }

    /// The host's clock, every 65 536 steps.
    ///
    /// Called from where both machines count their steps, so pure JS loops are
    /// covered too, not only loops inside built-ins. The hot path pays a mask and
    /// a branch; the call itself comes every 65 536 steps.
    #[inline]
    /// Milliseconds since page start: real if the host supplied a clock,
    /// otherwise the rising counter.
    pub fn now_ms(&mut self) -> f64 {
        match self.clock {
            Some(f) => f(),
            None => { self.fake_now += 1.0; self.fake_now }
        }
    }

    pub fn check_deadline(&mut self) -> C<()> {
        if let Some(f) = self.deadline {
            if !f() {
                return Err(self.throw_kind("RangeError", "script ran too long"));
            }
        }
        Ok(())
    }

    // ── Errors ───────────────────────────────────────────────────────────
    pub fn throw_kind(&mut self, kind: &'static str, msg: &str) -> Abrupt {
        let proto = self.realm.error_ctors.get(kind).cloned()
            .unwrap_or_else(|| self.realm.error_proto.clone());
        let e = new_kind(Some(proto), ObjKind::Error);
        e.borrow_mut().define("message", Prop::builtin(Value::str(msg)));
        Abrupt::Throw(Value::Obj(e))
    }
    pub fn type_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("TypeError", msg)) }

    /// "x is not a constructor", naming the call site and the kind of value.
    ///
    /// Three answers decide the case: `undefined` means a missing name or import,
    /// an arrow/method/generator means the wrong kind of function, and a built-in
    /// without `[[Construct]]` is intended by the spec. A minified bundle offers
    /// no other hint.
    pub fn not_a_constructor(&mut self, f: &Value) -> Abrupt {
        // The kind, briefly: it says why it is not a constructor.
        let art = match f {
            Value::Obj(o) => match &o.borrow().kind {
                ObjKind::Function(d) => {
                    let n = &d.node;
                    if n.is_arrow { "eine Pfeilfunktion" }
                    else if n.is_generator { "ein Generator" }
                    else if n.is_async { "eine async-Funktion" }
                    else { "eine Methode" }
                }
                ObjKind::Native(_) => "eingebaut, aber nicht mit new",
                _ => "ein gewoehnliches Objekt",
            },
            v => v.type_of(),
        };
        // The call-site name decides the case: "Intl.PluralRules is not a
        // constructor (undefined)" names a missing interface, "undefined is not a
        // constructor" only a state.
        if let Some(n) = self.new_name.clone() {
            return self.throw_kind("TypeError",
                &alloc::format!("{n} is not a constructor ({art})"));
        }
        // Without a call-site name: the value's own name, if it has one.
        let own = match f {
            Value::Obj(_) => self.get(f, "name").ok()
                .and_then(|n| self.to_string(&n).ok())
                .map(|n| alloc::string::String::from(&*n))
                .filter(|n| !n.is_empty()),
            _ => None,
        };
        match own {
            Some(n) => self.throw_kind("TypeError",
                &alloc::format!("{n} is not a constructor ({art})")),
            None => self.throw_kind("TypeError",
                &alloc::format!("{art} is not a constructor")),
        }
    }

    /// "x is not a function", with the name where there is one.
    ///
    /// `on` is the receiver the lookup ran on. A common name such as `render`
    /// alone does not say whose `render` is missing, and a minified bundle
    /// offers no other hint. Shared by both machines so neither loses the
    /// name in the message.
    pub fn not_a_function(&mut self, name: Option<&str>, on: Option<&Value>) -> Abrupt {
        let where_ = match on {
            Some(v @ Value::Obj(o)) => {
                // The constructor's name first: "on an instance of Foo" says more in a
                // minified bundle than any property list.
                let ctor = self.get(v, "constructor").ok()
                    .and_then(|c| self.get(&c, "name").ok())
                    .and_then(|n| self.to_string(&n).ok())
                    .map(|n| alloc::string::String::from(&*n))
                    .filter(|n| !n.is_empty() && n != "Object");
                let b = o.borrow();
                let mut keys: alloc::vec::Vec<alloc::string::String> = b.own_keys().into_iter()
                    .take(6).map(|k| alloc::string::String::from(&*k)).collect();
                // What the prototype offers says more than what the instance carries: for a
                // component without `render` the question is which component.
                if let Some(pr) = b.proto.clone() {
                    let pk: alloc::vec::Vec<alloc::string::String> = pr.borrow().own_keys()
                        .into_iter().take(6).map(|k| alloc::string::String::from(&*k)).collect();
                    if !pk.is_empty() { keys.push(alloc::format!("| Bauart: {}", pk.join(","))); }
                }
                drop(b);
                match (ctor, keys.is_empty()) {
                    (Some(c), true) => alloc::format!(" (auf einer Instanz von {c})"),
                    (Some(c), false) => alloc::format!(" (auf einer Instanz von {c} mit {})", keys.join(",")),
                    (None, true) => alloc::string::String::from(" (auf einem Objekt ohne Eigenschaften)"),
                    (None, false) => alloc::format!(" (auf einem Objekt mit {})", keys.join(",")),
                }
            }
            Some(v) if !matches!(v, Value::Undefined) =>
                alloc::format!(" (auf {})", v.type_of()),
            _ => alloc::string::String::new(),
        };
        match name {
            Some(n) => self.throw_kind("TypeError", &alloc::format!("{n} is not a function{where_}")),
            None => self.throw_kind("TypeError", &alloc::format!("value is not a function{where_}")),
        }
    }
    pub fn range_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("RangeError", msg)) }
    pub fn ref_err<T>(&mut self, msg: &str) -> C<T> { Err(self.throw_kind("ReferenceError", msg)) }

    // ── Conversions ──────────────────────────────────────────────────────
    /// `ToPrimitive`. `hint_string` chooses the order of `toString` and `valueOf`;
    /// that is the whole difference between `"" + obj` and `1 * obj`.
    pub fn to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {
        self.to_primitive_hint(v, if hint_string { "string" } else { "number" })
    }

    /// With the third hint, `"default"`. For ordinary objects it is the same as
    /// `"number"`, but `Symbol.toPrimitive` sees it, and a `Date` turns it into
    /// text. Without it `date + ""` would be a number.
    pub fn to_primitive_hint(&mut self, v: &Value, hint: &str) -> C<Value> {
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        // `Symbol.toPrimitive` comes before `valueOf`/`toString`; it is the only way
        // an object can override both.
        let exotic = self.get(v, SYM_TO_PRIMITIVE)?;
        if self.is_callable(&exotic) {
            let r = self.call(&exotic, v.clone(), &[Value::str(hint)])?;
            if !matches!(r, Value::Obj(_)) { return Ok(r); }
            return self.type_err("Symbol.toPrimitive returned an object");
        }
        let _ = o;
        self.ordinary_to_primitive(v, hint == "string")
    }

    /// `OrdinaryToPrimitive`: `valueOf` and `toString` in the order the hint
    /// dictates. Separate because `Date.prototype[Symbol.toPrimitive]` calls it.
    pub fn ordinary_to_primitive(&mut self, v: &Value, hint_string: bool) -> C<Value> {
        let Value::Obj(o) = v else { return Ok(v.clone()) };
        let order: [&str; 2] = if hint_string { ["toString", "valueOf"] } else { ["valueOf", "toString"] };
        for m in order {
            let f = self.get(&Value::Obj(o.clone()), m)?;
            if self.is_callable(&f) {
                let r = self.call(&f, v.clone(), &[])?;
                if !matches!(r, Value::Obj(_)) { return Ok(r); }
            }
        }
        self.type_err("cannot convert object to primitive value")
    }

    pub fn to_number(&mut self, v: &Value) -> C<f64> {
        Ok(match v {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            Value::Num(n) => *n,
            Value::Str(s) => string_to_num(s),
            Value::Sym(_) => return self.type_err("cannot convert a Symbol value to a number"),
            // A BigInt never silently becomes a Number: `1n + 1` is an error, not 2.
            Value::BigInt(_) => return self.type_err("cannot convert a BigInt value to a number"),
            Value::Obj(_) => { let p = self.to_primitive(v, false)?; self.to_number(&p)? }
        })
    }

    /// `ToBigInt`, the conversion a 64-bit view requires on write. A Number
    /// throws: the transition must be explicit in the source.
    pub fn to_bigint(&mut self, v: &Value) -> C<super::bigint::Big> {
        let p = self.to_primitive(v, false)?;
        Ok(match &p {
            Value::BigInt(b) => (**b).clone(),
            Value::Bool(b) => super::bigint::Big::from_u64(if *b { 1 } else { 0 }),
            Value::Str(t) => match super::bigint::Big::parse(t) {
                Some(b) => b,
                None => return Err(self.throw_kind("SyntaxError", "cannot convert string to a BigInt")),
            },
            _ => return self.type_err("cannot convert value to a BigInt"),
        })
    }

    /// `ToNumeric`: a BigInt stays a BigInt, anything else becomes a Number. This
    /// difference from `to_number` is why `x++` on a BigInt must not be `+x`.
    pub fn to_numeric(&mut self, v: &Value) -> C<Value> {
        let p = self.to_primitive(v, false)?;
        if matches!(p, Value::BigInt(_)) { return Ok(p); }
        Ok(Value::Num(self.to_number(&p)?))
    }

    /// Add or subtract one, in the value's own type.
    pub fn step_numeric(&mut self, v: &Value, up: bool) -> C<Value> {
        Ok(match v {
            Value::BigInt(b) => {
                let one = super::bigint::Big::from_u64(1);
                Value::BigInt(Rc::new(if up { b.add(&one) } else { b.sub(&one) }))
            }
            _ => { let n = self.to_number(v)?; Value::Num(if up { n + 1.0 } else { n - 1.0 }) }
        })
    }

    pub fn to_string(&mut self, v: &Value) -> C<Rc<str>> {
        Ok(match v {
            Value::Undefined => Rc::from("undefined"),
            Value::Null => Rc::from("null"),
            Value::Bool(b) => Rc::from(if *b { "true" } else { "false" }),
            Value::Num(n) => Rc::from(num_to_string(*n).as_str()),
            Value::Str(s) => s.clone(),
            // Deliberately an error, not text: `"" + sym` is almost always a mistake.
            // `String(sym)` and `sym.toString()` still work; they call
            // `sym_to_display` instead.
            Value::Sym(_) => return self.type_err("cannot convert a Symbol value to a string"),
            Value::BigInt(b) => Rc::from(b.to_string_radix(10).as_str()),
            Value::Obj(_) => { let p = self.to_primitive(v, true)?; self.to_string(&p)? }
        })
    }

    /// `ToPropertyKey`. The one place where a symbol becomes a property name;
    /// every computed access (`o[k]`, object literal, class member, `in`,
    /// `defineProperty`) goes through here.
    pub fn to_prop_key(&mut self, v: &Value) -> C<Rc<str>> {
        match v {
            Value::Sym(sd) => Ok(sd.key.clone()),
            _ => self.to_string(v),
        }
    }

    /// How a symbol looks when written: `Symbol(desc)`. Not `to_string`, which
    /// throws on purpose.
    pub fn sym_to_display(sd: &SymData) -> Rc<str> {
        match &sd.desc {
            Some(d) => Rc::from(alloc::format!("Symbol({d})").as_str()),
            None => Rc::from("Symbol()"),
        }
    }

    /// A fresh symbol. The description is part of the key, because
    /// `Object.getOwnPropertySymbols` only sees the key and must rebuild the
    /// symbol from it (`sym_from_key`). The running number in front keeps the key
    /// unique, so two `Symbol("x")` stay distinct as the spec requires.
    pub fn new_symbol(&mut self, desc: Option<Rc<str>>) -> Value {
        self.next_sym += 1;
        let n = self.next_sym;
        let key: Rc<str> = Rc::from(match &desc {
            Some(d) => alloc::format!("\0#{n}:{d}"),
            None => alloc::format!("\0#{n}"),
        }.as_str());
        Value::Sym(Rc::new(SymData { desc, key, registered: None }))
    }

    /// `ToObject`: primitives get their wrapper. This is how `"abc".length`
    /// works.
    pub fn to_object(&mut self, v: &Value) -> C<Gc> {
        match v {
            Value::Obj(o) => Ok(o.clone()),
            Value::Str(s) => {
                let g = new_kind(Some(self.realm.string_proto.clone()), ObjKind::StrWrap(s.clone()));
                {
                    let mut b = g.borrow_mut();
                    b.define("length", Prop::frozen(Value::Num(s.chars().count() as f64)));
                    for (i, c) in s.chars().enumerate() {
                        let mut t = String::new(); t.push(c);
                        b.define(&num_to_string(i as f64), Prop {
                            value: Some(Value::string(t)), get: None, set: None,
                            writable: false, enumerable: true, configurable: false });
                    }
                }
                Ok(g)
            }
            Value::Sym(sd) => Ok(new_kind(Some(self.realm.symbol_proto.clone()), ObjKind::SymWrap(sd.clone()))),
            Value::Num(n) => Ok(new_kind(Some(self.realm.number_proto.clone()), ObjKind::NumWrap(*n))),
            Value::Bool(b) => Ok(new_kind(Some(self.realm.boolean_proto.clone()), ObjKind::BoolWrap(*b))),
            Value::BigInt(b) => Ok(new_kind(Some(self.realm.bigint_proto.clone()), ObjKind::BigWrap(b.clone()))),
            Value::Undefined | Value::Null =>
                self.type_err("cannot convert undefined or null to object"),
        }
    }

    pub fn is_callable(&self, v: &Value) -> bool {
        let Value::Obj(o) = v else { return false };
        let kind = &o.borrow().kind;
        match kind {
            ObjKind::Function(_) | ObjKind::Native(_) | ObjKind::Bound { .. } => true,
            // A proxy is callable if its target is: `typeof new Proxy(f, {})` is
            // "function".
            ObjKind::Proxy(c) => match c.borrow().clone() {
                Some((t, _)) => self.is_callable(&Value::Obj(t)),
                None => false,
            },
            _ => false,
        }
    }

    /// May `new` be used on it? Arrows, methods, async functions and generators
    /// are not constructors, and `Reflect.construct` with one as `newTarget` must
    /// throw (test262's `isConstructor` helper relies on it).
    ///
    /// Not implemented: a method shorthand (`{ m(){} }`) looks like an ordinary
    /// function in our tree and is wrongly treated as a constructor.
    pub fn is_constructor(&self, v: &Value) -> bool {
        let Value::Obj(o) = v else { return false };
        let kind = &o.borrow().kind;
        match kind {
            ObjKind::Native(n) => n.ctor,
            ObjKind::Function(d) =>
                !d.node.is_arrow && !d.node.is_async && !d.node.is_generator,
            ObjKind::Bound { target, .. } => self.is_constructor(&Value::Obj(target.clone())),
            ObjKind::Proxy(c) => match c.borrow().clone() {
                Some((t, _)) => self.is_constructor(&Value::Obj(t)),
                None => false,
            },
            _ => false,
        }
    }

    // ── Properties ───────────────────────────────────────────────────────
    pub fn get(&mut self, base: &Value, key: &str) -> C<Value> {
        self.private_brand(base, key)?;
        // Primitives get no wrapper for a plain read, except strings, where length
        // and index are answered directly. A wrapper per access would be the most
        // expensive way to `s.length`.
        if let Value::Str(s) = base {
            if key == "length" { return Ok(Value::Num(s.chars().count() as f64)); }
            if let Some(i) = array_index(key) {
                return Ok(match s.chars().nth(i as usize) {
                    Some(c) => { let mut t = String::new(); t.push(c); Value::string(t) }
                    None => Value::Undefined,
                });
            }
        }
        // A primitive gets no wrapper for reading. For a string, `to_object` would
        // create a property per character just to find a method that lives on the
        // prototype, making loops over long strings quadratic. A primitive has no
        // own properties here (`length` and indices are handled by the fast path
        // above), so the chain starts at the prototype.
        let start = match base {
            Value::Obj(o) => o.clone(),
            Value::Undefined | Value::Null =>
                return self.type_err(&alloc::format!("cannot read '{key}' of {}",
                    if matches!(base, Value::Null) { "null" } else { "undefined" })),
            Value::Str(_) => self.realm.string_proto.clone(),
            Value::Num(_) => self.realm.number_proto.clone(),
            Value::Bool(_) => self.realm.boolean_proto.clone(),
            Value::BigInt(_) => self.realm.bigint_proto.clone(),
            Value::Sym(_) => self.realm.symbol_proto.clone(),
        };
        // A typed array view answers its indices itself, and finally: the prototype
        // chain is not walked, so `ta[99]` is `undefined` even if
        // `Object.prototype[99]` exists. That is the difference between a view and
        // an ordinary object with numeric keys.
        if let Some(v) = ta_read(&start, key) { return Ok(v) }
        // A module namespace reads the binding, not its value at link time:
        // `export let x` plus a setter means `ns.x` changes without anyone touching
        // `ns`; a snapshot in the property table would show the initial value
        // forever.
        if let super::value::ObjKind::ModuleNs(url) = &start.borrow().kind {
            let url = url.clone();
            if let Some(v) = self.ns_live(&url, key) {
                return Ok(v);
            }
        }
        // A proxy answers every access itself; the prototype chain underneath is not
        // walked.
        if super::proxy::parts(&start).is_some() {
            return match super::proxy::trap(self, &start, "get")? {
                Some((f, h, t)) => {
                    let kv = super::proxy::key_value(key);
                    self.call(&f, h, &[t, kv, base.clone()])
                }
                None => { let t = super::proxy::target(self, &start)?; self.get(&Value::Obj(t), key) }
            };
        }
        // Array `length` lives in the property table like everything else; only the
        // chain underneath is walked here.
        let mut cur = Some(start);
        let mut hops = 0;
        while let Some(o) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = o.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(g) = &p.get {
                    if !matches!(g, Value::Undefined) {
                        return self.call(&g.clone(), base.clone(), &[]);
                    }
                }
                if p.is_accessor() { return Ok(Value::Undefined); }
                return Ok(p.value.clone().unwrap_or(Value::Undefined));
            }
            let next = o.borrow().proto.clone();
            cur = next;
        }
        Ok(Value::Undefined)
    }

    /// `#x in obj`: the brand check as an expression (ES §13.10.1).
    ///
    /// It does not throw; it answers yes or no. The parser turns it into an
    /// identifier `#x`; a real name can never start with `#`, so this is
    /// unambiguous.
    pub fn private_in(&mut self, name: &str, base: &Value) -> C<Value> {
        let Value::Obj(o) = base else {
            return self.type_err("the right side of 'in' must be an object");
        };
        let key = super::value::private_key(name);
        let o = o.clone();
        Ok(Value::Bool(self.has_property(&o, &key)))
    }

    /// The brand check (ES §7.3.28 `PrivateGet`/`PrivateSet`).
    ///
    /// A private field is not a property one can create: it comes into being
    /// in the constructor and nowhere else. Access on an object that lacks it
    /// is a TypeError, not `undefined` and certainly not a silent creation.
    fn private_brand(&mut self, base: &Value, key: &str) -> C<()> {
        if !key.starts_with(super::value::PRIVATE_PREFIX) { return Ok(()); }
        let ok = matches!(base, Value::Obj(o) if self.has_property(o, key));
        if ok { return Ok(()); }
        self.type_err(&alloc::format!(
            "cannot read private member #{} from an object whose class did not declare it",
            super::value::private_name(key)))
    }

    /// `Set(O, P, V, Throw)` (ES §7.3.4).
    ///
    /// The throw flag is an argument, not a mode: not only strict code wants
    /// an error when a write fails, but almost every built-in too
    /// (`[].push` on a frozen array must throw in both modes). Each call site
    /// decides; a default would leave half of them silently wrong.
    pub fn set(&mut self, base: &Value, key: &str, val: Value, throw: bool) -> C<()> {
        self.private_brand(base, key)?;
        let Value::Obj(o) = base else {
            // Assigning a property on a primitive does nothing in sloppy mode; with
            // `throw` it is a TypeError.
            if matches!(base, Value::Undefined | Value::Null) { strict_site!(self, 11); }
            else { strict_site!(self, 0); }
            if throw {
                // `undefined.x = 1` already throws when reading the base; this is about
                // `"abc".x = 1` in strict mode.
                return self.type_err(&alloc::format!(
                    "cannot create property '{key}' on a primitive value"));
            }
            return Ok(());
        };
        // Same finality on write: outside the view the write is dropped, and a
        // setter in the chain never sees it.
        if let Some(t) = ta_of(o) {
            if let Some(k) = array_index(key) {
                // The conversion runs even if the index is out of range; it is observable.
                if t.kind.is_big() {
                    let big = self.to_bigint(&val)?;
                    let live = t.live_len();
                    if (k as usize) < live {
                        let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Ok(()) };
                        let at = t.offset + (k as usize) * t.kind.size();
                        t.kind.write_big(&mut b.bytes.borrow_mut(), at, &big);
                    }
                    return Ok(());
                }
                let n = self.to_number(&val)?;
                let live = t.live_len();
                if (k as usize) < live {
                    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Ok(()) };
                    let at = t.offset + (k as usize) * t.kind.size();
                    t.kind.write(&mut b.bytes.borrow_mut(), at, n);
                }
                return Ok(());
            }
        }
        // `el.dataset.x = v` writes the attribute; otherwise it would assign to a
        // copy.
        let ds = match &o.borrow().kind { ObjKind::Dataset(id) => Some(*id), _ => None };
        if let Some(id) = ds {
            let text = self.to_string(&val)?;
            let attr = super::dombind::camel_to_data_attr(key);
            if let Some(d) = &mut self.doc { d.set_attr_at(id, &attr, &text); }
            // Keep the snapshot in step as well, so code holding the object in a
            // variable reads its own write.
            o.borrow_mut().define(key, Prop::data(Value::Str(text)));
            return Ok(());
        }
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "set")? {
                Some((f, h, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, h, &[t, kv, val, base.clone()])?;
                    if !r.truthy() {
                        strict_site!(self, 5);
                        if throw {
                            return self.type_err(&alloc::format!(
                                "'set' on proxy: trap returned falsish for property '{key}'"));
                        }
                    }
                    Ok(())
                }
                None => { let t = super::proxy::target(self, o)?; self.set(&Value::Obj(t), key, val, throw) }
            };
        }
        // A setter anywhere in the chain wins over the own field.
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return self.type_err("prototype chain too long (cycle?)"); }
            let found = c.borrow().get_own(key).cloned();
            if let Some(p) = found {
                if let Some(st) = &p.set {
                    if !matches!(st, Value::Undefined) {
                        self.call(&st.clone(), base.clone(), &[val])?;
                        return Ok(());
                    }
                }
                // A getter without a setter: the write fails.
                if p.is_accessor() {
                    strict_site!(self, 3);
                    if throw { return self.type_err(&alloc::format!(
                        "cannot set property '{key}' of #<Object> which has only a getter")); }
                    return Ok(());
                }
                if Rc::ptr_eq(&c, o) {
                    if !p.writable {
                        strict_site!(self, 1);
                        if throw { return self.type_err(&alloc::format!(
                            "cannot assign to read only property '{key}'")); }
                        return Ok(());
                    }
                    let mut np = p.clone();
                    np.value = Some(val);
                    o.borrow_mut().set_prop(Rc::from(key), np);
                    return Ok(());
                }
                if !p.writable {
                    strict_site!(self, 2);
                    if throw { return self.type_err(&alloc::format!(
                        "cannot assign to read only property '{key}'")); }
                    return Ok(());
                }
                break;
            }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        if !o.borrow().extensible {
            strict_site!(self, 4);
            if throw { return self.type_err(&alloc::format!(
                "cannot add property {key}, object is not extensible")); }
            return Ok(());
        }
        o.borrow_mut().set_prop(Rc::from(key), Prop::data(val));
        self.fix_array_length(o, key);
        Ok(())
    }

    /// An array keeps `length` in step: assigning an index beyond the length
    /// extends it, so `a[0]=1; a.length` is 1.
    fn fix_array_length(&mut self, o: &Gc, key: &str) {
        if !matches!(o.borrow().kind, ObjKind::Array) { return; }
        if let Some(i) = array_index(key) {
            let cur = o.borrow().get_own("length").and_then(|p| p.value.clone());
            let n = match cur { Some(Value::Num(n)) => n, _ => 0.0 };
            if (i as f64) >= n {
                o.borrow_mut().define("length", Prop {
                    value: Some(Value::Num(i as f64 + 1.0)), get: None, set: None,
                    writable: true, enumerable: false, configurable: false });
            }
        }
    }

    pub fn has_property(&mut self, o: &Gc, key: &str) -> bool {
        // A proxy can throw here, but `has_property` only returns yes/no, so the
        // thrown value is lost. Known limit; `has_prop` passes it through, and `in`
        // calls that.
        if super::proxy::parts(o).is_some() {
            return self.has_prop(o, key).unwrap_or(false);
        }
        if let Some(t) = ta_of(o) {
            if let Some(k) = array_index(key) {
                return (k as usize) < t.live_len();
            }
        }
        let mut cur = Some(o.clone());
        let mut hops = 0;
        while let Some(c) = cur {
            hops += 1;
            if hops > MAX_PROTO_CHAIN { return false; }
            if c.borrow().has_own(key) { return true; }
            let next = c.borrow().proto.clone();
            cur = next;
        }
        false
    }

    // ── Calling ──────────────────────────────────────────────────────────
    pub fn call(&mut self, callee: &Value, this_val: Value, args: &[Value]) -> C<Value> {
        let Value::Obj(f) = callee else {
            return self.type_err("value is not a function");
        };
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return self.range_err("Maximum call stack size exceeded");
        }
        let r = self.call_inner(f, this_val, args);
        self.depth -= 1;
        r
    }

    fn call_inner(&mut self, f: &Gc, this_val: Value, args: &[Value]) -> C<Value> {
        // The `apply` trap. Without it a call on a proxy would bypass the handler
        // and reach the target.
        if super::proxy::parts(f).is_some() {
            return match super::proxy::trap(self, f, "apply")? {
                Some((fn_, hv, t)) => {
                    let arr = self.new_array(args.to_vec());
                    self.call(&fn_, hv, &[t, this_val, arr])
                }
                None => { let t = super::proxy::target(self, f)?;
                          self.call(&Value::Obj(t), this_val, args) }
            };
        }
        enum Which { Native(Rc<NativeData>), Js(Rc<FuncData>), Bound(Gc, Value, Vec<Value>) }
        let which = match &f.borrow().kind {
            ObjKind::Native(n) => Which::Native(n.clone()),
            ObjKind::Function(d) => Which::Js(d.clone()),
            ObjKind::Bound { target, this_val, args } =>
                Which::Bound(target.clone(), this_val.clone(), args.clone()),
            _ => return self.type_err("value is not a function"),
        };
        match which {
            Which::Native(n) => {
                let r = (n.func)(self, this_val, args);
                // CEReactions: custom element callbacks queued by a DOM operation run
                // before control returns to script.
                if self.doc.as_ref().is_some_and(|d| !d.ce_queue.is_empty()) {
                    super::dombind::ce_flush(self);
                }
                r
            }
            Which::Bound(t, bt, mut ba) => {
                ba.extend_from_slice(args);
                self.call(&Value::Obj(t), bt, &ba)
            }
            Which::Js(d) => {
                // A generator does not run its body here. The call builds an object, and the
                // body starts at the first `next()`, on its own machine. This is the only
                // place a generator object is created; the bytecode machine routes its calls
                // here on purpose.
                if d.node.is_generator && !d.node.is_async {
                    if let Some(v) = super::generator::make(self, f, &d, this_val.clone(), args)? {
                        return Ok(v);
                    }
                }
                // An async function returns a promise. Its body runs synchronously up to the
                // first `await`, then suspends; the microtask queue resumes it.
                if d.node.is_async && !d.node.is_generator {
                    if let Some(v) = super::generator::make_async(self, &d, this_val.clone(), args)? {
                        return Ok(v);
                    }
                }
                // An async generator is both: it returns an object like a generator, and
                // each `next()` returns a promise like an async function.
                if d.node.is_async && d.node.is_generator {
                    if let Some(v) = super::generator::make_async_gen(self, f, &d, this_val.clone(), args)? {
                        return Ok(v);
                    }
                }
                self.run_js_body(&d, this_val, args)
            }
        }
    }

    /// The environment a call runs in: everything before the body's first step
    /// (`this`, `arguments`, home object, parameters).
    ///
    /// Separate because the bytecode machine needs it before pushing a frame;
    /// two implementations of this prologue would be two call semantics.
    pub fn call_env(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value])
        -> C<Rc<RefCell<Env>>> {
        let env = Env::new(Some(d.env.clone()), true);
        // An arrow gets no `this` of its own, so `this_of` finds the enclosing one.
        env.borrow_mut().home = d.home_object.clone();
        // The strictness of the body, not of the caller: a strict function stays
        // strict whoever calls it, and a sloppy one stays sloppy even when called
        // from strict code. `Env::new` inherited the definition site's mode; a
        // `"use strict"` in the body adds to it here.
        if d.node.strict { env.borrow_mut().strict = true; }
        if !d.node.is_arrow {
            let t = d.this_val.clone().unwrap_or(this_val);
            let t = self.bind_this(t, d.node.strict)?;
            env.borrow_mut().this_val = Some(t);
            let ao = self.make_arguments(args);
            env.borrow_mut().vars.insert(Rc::from("arguments"),
                Binding { value: ao, mutable: true, initialized: true });
        }
        self.bind_params(&d.node.params, args, &env)?;
        // Instance fields of a base class exist before the body runs. A derived
        // class adds them only after `super()`: an initializer may see a parent
        // field, which exists only from then on.
        if let Some(c) = &d.class {
            if c.super_class.is_none() {
                let t = super::interp::env_this(&env);
                self.init_fields(d, &t)?;
            }
        }
        Ok(env)
    }

    /// `OrdinaryCallBindThis` (ES §10.2.1.2): what `this` really is in the body.
    ///
    /// A strict function gets the value unchanged: `f()` sees `undefined`. A
    /// sloppy one sees `globalThis` there, and a primitive is boxed: `(7).f()`
    /// sees a `Number` object, not `7`.
    fn bind_this(&mut self, t: Value, strict: bool) -> C<Value> {
        if strict { return Ok(t); }
        match t {
            Value::Undefined | Value::Null => Ok(Value::Obj(self.realm.global.clone())),
            Value::Obj(_) => Ok(t),
            other => Ok(Value::Obj(self.to_object(&other)?)),
        }
    }

    /// Put a class's instance fields on a fresh `this`.
    ///
    /// Each initializer is its own small function scope with `this` on the
    /// instance and the class as `home`: an arrow inside captures the instance,
    /// and `super.x` hits the parent class. The surrounding scope is the class's
    /// (`d.env`), not the caller's.
    pub fn init_fields(&mut self, d: &Rc<FuncData>, this_val: &Value) -> C<()> {
        let Some(c) = d.class.clone() else { return Ok(()) };
        let Value::Obj(o) = this_val else { return Ok(()) };
        for m in &c.body {
            let ClassMember::Field { key, value, is_static: false, .. } = m else { continue };
            let fenv = Env::new(Some(d.env.clone()), true);
            {
                let mut b = fenv.borrow_mut();
                b.this_val = Some(this_val.clone());
                b.home = d.home_object.clone();
            }
            let k = self.prop_key(key, &fenv)?;
            let v = match value {
                Some(e) => {
                    let val = self.eval(e, &fenv)?;
                    // `x = function(){}` gives the function the field name; same rule as
                    // `var f = function(){}`.
                    self.name_function(&val, &k);
                    val
                }
                None => Value::Undefined,
            };
            o.borrow_mut().set_prop(k, Prop::data(v));
        }
        Ok(())
    }

    /// Run a function body with the tree walker: the path a call takes when the
    /// compiler cannot handle the body.
    ///
    /// Separate because `generator.rs` needs it for the same case: an async
    /// function with an uncompilable body must still return a promise, so
    /// someone has to run the body and collect the outcome.
    pub fn run_js_body(&mut self, d: &Rc<FuncData>, this_val: Value, args: &[Value]) -> C<Value> {
        let env = self.call_env(d, this_val, args)?;
        // The body belongs on the bytecode machine even when the call does not come
        // from it (a built-in callback, the microtask queue, a generator); otherwise
        // everything below such a call would stay on the tree walker.
        //
        // Generators and async functions are excluded: their bodies suspend, and
        // `generator.rs` gives each call its own machine.
        if !d.node.is_generator && !d.node.is_async {
            if let Some(chunk) = self.func_chunk(&d.node) {
                self.hoist_body(&d.node.body, &env)?;
                return match super::vm::Vm::run_function(self, chunk, &env) {
                    // A constructor without `return` yields its `this`, which `super()`
                    // may have replaced.
                    Ok(Value::Undefined) => Ok(if d.class.is_some() { env_this(&env) } else { Value::Undefined }),
                    Ok(v) => Ok(v),
                    Err(e) => Err(e),
                };
            }
        }
        // A constructor without `return` yields its `this`, which `super()` may
        // have replaced. For a base class it is the object `construct` uses anyway;
        // for a derived class it is the difference.
        let implicit = || if d.class.is_some() { env_this(&env) } else { Value::Undefined };
        match self.run_body(&d.node.body, &env) {
            Ok(()) | Err(Abrupt::Return(Value::Undefined)) => Ok(implicit()),
            Err(Abrupt::Return(v)) => Ok(v),
            Err(e) => Err(e),
        }
    }

    /// Hoisting for a function body (`var` and functions to the front), for the
    /// machine that then runs the body as bytecode.
    pub fn hoist_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        self.hoist(body, env, env)
    }

    /// The compiled body of this function, or `None` if the compiler declines.
    /// Once per function, then remembered.
    pub fn func_chunk(&mut self, f: &Rc<Func>) -> Option<Rc<super::code::Chunk>> {
        if self.vm_off {
            return None;
        }
        let key = Rc::as_ptr(f) as usize;
        if let Some((_, c)) = self.func_chunks.get(&key) {
            return c.clone();
        }
        let c = match super::compile::function(f) {
            Ok(ch) => Some(Rc::new(ch)),
            Err(u) => {
                *self.func_declines.entry(u.0).or_insert(0) += 1;
                None
            }
        };
        self.func_chunks.insert(key, (Rc::downgrade(f), c.clone()));
        c
    }

    /// `GetTemplateObject` (ES 13.2.8.4): the object a tagged template passes to
    /// its tag as the first argument.
    ///
    /// A frozen array of the cooked strings (`undefined` where the escape was
    /// invalid, which is why `cooked` is an `Option`), with a frozen `raw`
    /// beside it. Neither writable, enumerable nor configurable.
    pub fn template_object(&mut self, quasis: &[super::ast::TemplateElement]) -> Value {
        let key = quasis.as_ptr() as usize;
        let raws: Vec<Rc<str>> = quasis.iter().map(|q| Rc::from(q.raw.as_str())).collect();
        if let Some((old, v)) = self.templates.get(&key) {
            if old.len() == raws.len() && old.iter().zip(&raws).all(|(a, b)| a == b) {
                return v.clone();
            }
        }
        // Freeze like `Object.freeze`, with the same care: the borrow must end
        // before the write, or the inner `borrow_mut` panics.
        let frozen = |o: &Gc| {
            o.borrow_mut().extensible = false;
            // Collect the keys into a list first: written as
            // `for k in o.borrow().own_keys()`, the borrow lives to the end of the body
            // and the inner `borrow_mut` panics. Same as in `Object.freeze`.
            let keys = o.borrow().own_keys();
            for k in keys {
                let existing = o.borrow().get_own(&k).cloned();
                if let Some(mut p) = existing {
                    p.writable = false;
                    p.configurable = false;
                    o.borrow_mut().set_prop(k, p);
                }
            }
        };
        let raw_arr = self.new_array(raws.iter().map(|r| Value::Str(r.clone())).collect());
        let cooked = self.new_array(quasis.iter()
            .map(|q| q.cooked.as_deref().map_or(Value::Undefined, Value::str))
            .collect());
        let (Value::Obj(ro), Value::Obj(co)) = (&raw_arr, &cooked) else {
            return cooked;
        };
        frozen(ro);
        co.borrow_mut().define("raw", Prop {
            value: Some(raw_arr.clone()), get: None, set: None,
            writable: false, enumerable: false, configurable: false,
        });
        frozen(co);
        self.templates.insert(key, (raws, cooked.clone()));
        cooked
    }

    fn make_arguments(&mut self, args: &[Value]) -> Value {
        let g = new_kind(Some(self.realm.object_proto.clone()), ObjKind::Arguments);
        // `arguments` is iterable: `[...arguments]` and `for (a of arguments)` use
        // the same function as `Array.prototype.values`.
        {
            let vals = self.get(&Value::Obj(self.realm.array_proto.clone()), "values");
            if let Ok(v) = vals { g.borrow_mut().define(SYM_ITERATOR, Prop::builtin(v)); }
        }
        {
            let mut o = g.borrow_mut();
            for (i, a) in args.iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(a.clone()));
            }
            o.define("length", Prop::builtin(Value::Num(args.len() as f64)));
        }
        Value::Obj(g)
    }

    fn bind_params(&mut self, params: &[Pat], args: &[Value], env: &Rc<RefCell<Env>>) -> C<()> {
        // Create all parameter names here first, then bind.
        //
        // `bind_pattern(…, true)` binds via `init_binding`, which walks up the
        // chain: a parameter named like an outer variable would write to the outer
        // one. Minified code reuses short names everywhere, so this matters.
        //
        // Before binding, not per parameter: `function f(a = b, b)` is a
        // ReferenceError and must not find an outer `b`
        // (`FunctionDeclarationInstantiation`, ES §10.2.11 step 21).
        for p in params {
            let mut names = Vec::new();
            super::eval::names_of(p, &mut names);
            for n in names {
                env.borrow_mut().vars.insert(Rc::from(n.as_str()),
                    Binding { value: Value::Undefined, mutable: true, initialized: false });
            }
        }
        let mut i = 0;
        for p in params {
            if let Pat::Rest(inner) = p {
                let rest: Vec<Value> = args.iter().skip(i).cloned().collect();
                let arr = self.new_array(rest);
                self.bind_pattern(inner, arr, env, true)?;
                break;
            }
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            self.bind_pattern(p, v, env, true)?;
            i += 1;
        }
        Ok(())
    }

    // ── Program ──────────────────────────────────────────────────────────
    pub fn run_program(&mut self, prog: &Program) -> C<Value> {
        let env = self.realm.global_env.clone();
        // The global environment belongs to the unit currently running: a script
        // with `"use strict"` makes it strict, the next one without makes it sloppy
        // again (e.g. a sloppy test262 prelude followed by a strict test body).
        env.borrow_mut().strict = prog.strict;
        // Hoisting is the same for both machines: it works on the environment, not
        // on the code.
        self.hoist(&prog.body, &env, &env)?;
        // All or nothing: what the compiler can handle runs on the bytecode
        // machine; if it declines anywhere, the tree walker runs the whole program.
        // Mixing would be a second semantic path in the same run.
        let r = match if self.vm_off { Err(super::code::Unsupported("off")) }
                      else { super::compile::program(prog) } {
            Ok(chunk) => {
                self.vm_ran += 1;
                self.vm_decline = None;
                let mut vm = super::vm::Vm::new();
                vm.run(self, Rc::new(chunk), &env)
            }
            Err(u) => {
                self.vm_declined += 1;
                self.vm_decline = Some(u.0);
                (|| -> C<Value> {
                    let mut last = Value::Undefined;
                    for st in &prog.body {
                        if let Some(v) = self.exec(st, &env)? { last = v; }
                    }
                    Ok(last)
                })()
            }
        };
        // Even if the program threw, the queue must be drained: a `.then` attached
        // before the error is registered.
        super::promise::run_jobs(self);
        r
    }

    /// `eval`. Direct and indirect differ in scope: `eval(s)` as a plain call
    /// runs in the caller's scope and sees its names, `(0,eval)(s)` runs global.
    pub fn perform_eval(&mut self, code: &Value, caller: Option<Rc<RefCell<Env>>>) -> C<Value> {
        // Anything that is not a string comes back unchanged: `eval(42)` is 42, not
        // a program.
        let Value::Str(src) = code else { return Ok(code.clone()) };
        // An `eval` creates a Rust frame (parser + its own machine) and could
        // otherwise recurse without bound (`eval("eval('…')")`), so it counts as a
        // call.
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(self.throw_kind("RangeError", "maximum call stack size exceeded"));
        }
        let r = self.eval_inner(src, caller);
        self.depth -= 1;
        r
    }

    fn eval_inner(&mut self, src: &Rc<str>, caller: Option<Rc<RefCell<Env>>>) -> C<Value> {
        let prog = match super::parser::parse(src, false) {
            Ok(p) => p,
            Err(e) => return Err(self.throw_kind("SyntaxError", &e.msg)),
        };
        // A direct eval inherits its caller's strictness; its own directive adds to
        // it. An indirect one starts sloppy.
        let inherited = caller.as_ref().is_some_and(|e| e.borrow().strict);
        let strict = prog.strict || inherited;
        let base = caller.unwrap_or_else(|| self.realm.global_env.clone());
        // `let`/`const` get their own scope; `var` and function declarations rise
        // to the caller's next function boundary, which is why this scope is not a
        // function scope.
        let scope = Env::new(Some(base.clone()), false);
        scope.borrow_mut().strict = strict;
        // A strict eval keeps its `var` to itself; otherwise it would rise to the
        // caller's function boundary and create a name the caller never wrote.
        let var_env = if strict {
            scope.borrow_mut().is_func_scope = true;
            scope.clone()
        } else {
            let mut ve = base;
            loop {
                let is_fn = ve.borrow().is_func_scope;
                if is_fn { break }
                let up = ve.borrow().parent.clone();
                match up { Some(p) => ve = p, None => break }
            }
            ve
        };
        self.hoist(&prog.body, &scope, &var_env)?;
        // Same choice as for programs: if the compiler can handle everything, the
        // machine runs it; otherwise the tree walker. Never a mix.
        match if self.vm_off { Err(super::code::Unsupported("off")) }
              else { super::compile::program(&prog) } {
            Ok(chunk) => {
                self.vm_ran += 1;
                let mut vm = super::vm::Vm::new();
                vm.run(self, Rc::new(chunk), &scope)
            }
            Err(u) => {
                self.vm_declined += 1;
                self.vm_decline = Some(u.0);
                let mut last = Value::Undefined;
                for st in &prog.body {
                    if let Some(v) = self.exec(st, &scope)? { last = v; }
                }
                Ok(last)
            }
        }
    }

    /// Is this value the built-in `eval`? Only then is `eval(...)` a direct call;
    /// any other function under that name is an ordinary call.
    pub fn is_eval_fn(&self, v: &Value) -> bool {
        let Some(want) = self.realm.eval_fn.as_ref() else { return false };
        matches!(v, Value::Obj(o) if Rc::ptr_eq(o, want))
    }

    fn run_body(&mut self, body: &[Stmt], env: &Rc<RefCell<Env>>) -> C<()> {
        self.hoist(body, env, env)?;
        for st in body { self.exec(st, env)?; }
        Ok(())
    }

    /// Hoist `var` and function declarations.
    ///
    /// `var` rises to the next function boundary; `let`/`const`/`class` stay in
    /// the block and are "not ready" until their declaration. That is the
    /// temporal dead zone; without it `let` is just `var` by another name.
    fn hoist(&mut self, body: &[Stmt], block: &Rc<RefCell<Env>>, func: &Rc<RefCell<Env>>) -> C<()> {
        for st in body { self.hoist_vars(st, func); }
        for st in body {
            // `export function f(){}` is a declaration with a keyword in front and is
            // hoisted the same way. Otherwise an exported function would only exist once
            // its line ran, and a cycle in the module graph would never see it.
            let st = super::modules::unexport(st).unwrap_or(st);
            match st {
                Stmt::Func(f) => {
                    if let Some(n) = &f.name {
                        let v = self.make_closure(f.clone(), block, None);
                        // Same rule as for `var`: a top-level function declaration in a script is a
                        // property of the global object. In a block or function it is not.
                        if Rc::ptr_eq(block, &self.realm.global_env) {
                            self.realm.global.borrow_mut().define(n.as_str(), Prop {
                                value: Some(v), get: None, set: None,
                                writable: true, enumerable: true, configurable: false });
                        } else {
                            block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                                Binding { value: v, mutable: true, initialized: true });
                        }
                    }
                }
                Stmt::VarDecl(d) if d.kind != VarKind::Var => {
                    let mut names = Vec::new();
                    for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                    for n in names {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()), Binding {
                            value: Value::Undefined,
                            mutable: d.kind != VarKind::Const,
                            initialized: false,
                        });
                    }
                }
                // `export default` stores its value under a name no script can write. It is
                // created here so that a cycle reading it too early gets "not ready" rather
                // than "does not exist", and so that a function declaration behind it is
                // hoisted.
                Stmt::ExportDefault(d) => {
                    let (v, init, own) = match &**d {
                        ExportDefault::Func(f) =>
                            (self.make_closure(f.clone(), block, None), true, f.name.clone()),
                        ExportDefault::Class(c) => (Value::Undefined, false, c.name.clone()),
                        ExportDefault::Expr(_) => (Value::Undefined, false, None),
                    };
                    if let Some(n) = own {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: v.clone(), mutable: true, initialized: init });
                    }
                    block.borrow_mut().vars.insert(Rc::from(super::modules::DEFAULT_LOCAL),
                        Binding { value: v, mutable: true, initialized: init });
                }
                Stmt::Class(c) => {
                    if let Some(n) = &c.name {
                        block.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: Value::Undefined, mutable: true, initialized: false });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Collect `var` through blocks and loops, but not through functions: a new
    /// scope starts there.
    fn hoist_vars(&mut self, st: &Stmt, func: &Rc<RefCell<Env>>) {
        let st = super::modules::unexport(st).unwrap_or(st);
        // A top-level `var` in a script is a property of the global object
        // (ES CreateGlobalVarBinding), and that is its only storage, not a second
        // copy beside the binding chain. Reads and writes find it:
        // `assign_ident_depth` and name resolution both fall back to the global
        // object when the chain lacks the name. UMD bundles expose themselves this
        // way (`window.X` for `var X`).
        //
        // `let`/`const` do not belong there (they live in the declarative part), and
        // a module has its own environment; both are excluded because only `var` and
        // only the global environment take this path.
        let gobj = Rc::ptr_eq(func, &self.realm.global_env)
            .then(|| self.realm.global.clone());
        let mut put = |names: Vec<String>| {
            for n in names {
                if let Some(g) = &gobj {
                    // Already there? Then an earlier `var` or the host object set a value; do
                    // not overwrite it.
                    if g.borrow().get_own(n.as_str()).is_none() {
                        // Not deletable (ES: D = false for a script `var`), but enumerable and
                        // writable like any page property.
                        g.borrow_mut().define(n.as_str(), Prop {
                            value: Some(Value::Undefined), get: None, set: None,
                            writable: true, enumerable: true, configurable: false });
                    }
                    continue;
                }
                let key: Rc<str> = Rc::from(n.as_str());
                if !func.borrow().vars.contains_key(&key) {
                    func.borrow_mut().vars.insert(key,
                        Binding { value: Value::Undefined, mutable: true, initialized: true });
                }
            }
        };
        match st {
            Stmt::VarDecl(d) if d.kind == VarKind::Var => {
                let mut names = Vec::new();
                for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                put(names);
            }
            Stmt::Block(b) => for s in b { self.hoist_vars(s, func) },
            Stmt::If { cons, alt, .. } => {
                self.hoist_vars(cons, func);
                if let Some(a) = alt { self.hoist_vars(a, func); }
            }
            Stmt::For { init, body, .. } => {
                if let Some(i) = init {
                    if let ForInit::VarDecl(d) = &**i {
                        if d.kind == VarKind::Var {
                            let mut names = Vec::new();
                            for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                            put(names);
                        }
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::ForIn { left, body, .. } | Stmt::ForOf { left, body, .. } => {
                if let ForHead::VarDecl(d) = &**left {
                    if d.kind == VarKind::Var {
                        let mut names = Vec::new();
                        for dec in &d.decls { super::eval::names_of(&dec.id, &mut names); }
                        put(names);
                    }
                }
                self.hoist_vars(body, func);
            }
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. }
            | Stmt::Labeled { body, .. } | Stmt::With { body, .. } => self.hoist_vars(body, func),
            Stmt::Try { block, handler, finalizer } => {
                for s in block { self.hoist_vars(s, func); }
                if let Some(h) = handler { for s in &h.body { self.hoist_vars(s, func); } }
                if let Some(f) = finalizer { for s in f { self.hoist_vars(s, func); } }
            }
            Stmt::Switch { cases, .. } => {
                for c in cases { for s in &c.body { self.hoist_vars(s, func); } }
            }
            _ => {}
        }
    }

    // ── The iterator protocol ────────────────────────────────────────────

    /// `ToPropertyDescriptor` (ES §6.2.6.5): a descriptor object to `Desc`.
    ///
    /// Checks presence, not truthiness: a missing field must stay absent, which
    /// is the point of a partial descriptor (correct for redefining, not only
    /// for creating).
    ///
    /// Presence is checked through the prototype chain (`HasProperty`, not
    /// `has_own`): a descriptor that inherits `writable` counts.
    ///
    /// One function for all callers (`Object.defineProperty`,
    /// `Object.defineProperties`, `Object.create` with a second argument,
    /// `Reflect.defineProperty`) so their rules cannot diverge.
    pub fn to_prop_desc(&mut self, d: &Value) -> C<Desc> {
        let Value::Obj(dd) = d else {
            return self.type_err("property descriptor must be an object");
        };
        let dd = dd.clone();
        let mut out = Desc::default();
        if self.has_property(&dd, "enumerable") {
            out.enumerable = Some(self.get(d, "enumerable")?.truthy());
        }
        if self.has_property(&dd, "configurable") {
            out.configurable = Some(self.get(d, "configurable")?.truthy());
        }
        if self.has_property(&dd, "value") { out.value = Some(self.get(d, "value")?); }
        if self.has_property(&dd, "writable") {
            out.writable = Some(self.get(d, "writable")?.truthy());
        }
        if self.has_property(&dd, "get") {
            let g = self.get(d, "get")?;
            if !self.is_callable(&g) && !matches!(g, Value::Undefined) {
                return self.type_err("getter must be a function");
            }
            out.get = Some(g);
        }
        if self.has_property(&dd, "set") {
            let st = self.get(d, "set")?;
            if !self.is_callable(&st) && !matches!(st, Value::Undefined) {
                return self.type_err("setter must be a function");
            }
            out.set = Some(st);
        }
        if out.is_accessor() && out.is_data() {
            return self.type_err(
                "property descriptor cannot be both an accessor and a data descriptor");
        }
        Ok(out)
    }

    /// And back: `Prop` -> descriptor object.
    pub fn from_prop_desc(&mut self, p: &Prop) -> Value {
        let d = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut b = d.borrow_mut();
            if p.is_accessor() {
                b.define("get", Prop::data(p.get.clone().unwrap_or(Value::Undefined)));
                b.define("set", Prop::data(p.set.clone().unwrap_or(Value::Undefined)));
            } else {
                b.define("value", Prop::data(p.value.clone().unwrap_or(Value::Undefined)));
                b.define("writable", Prop::data(Value::Bool(p.writable)));
            }
            b.define("enumerable", Prop::data(Value::Bool(p.enumerable)));
            b.define("configurable", Prop::data(Value::Bool(p.configurable)));
        }
        Value::Obj(d)
    }

    /// Put the properties of a `{k: descriptor, …}` on an object, for
    /// `Object.defineProperties` and `Object.create(p, props)`.
    pub fn define_props_from(&mut self, target: &Gc, props: &Value) -> C<()> {
        let Value::Obj(src) = props else {
            return self.type_err("properties must be an object");
        };
        let keys: Vec<Rc<str>> = src.borrow().own_keys().into_iter()
            .filter(|k| src.borrow().is_enumerable(k))
            .collect();
        // Read all descriptors first, then apply all (ES §20.1.2.3.1). The order is
        // observable: if the third descriptor throws, the first two must not have
        // been applied.
        let mut pending = Vec::new();
        for k in keys {
            let d = self.get(props, &k)?;
            pending.push((k, self.to_prop_desc(&d)?));
        }
        for (k, d) in pending {
            self.define_or_throw(target, &k, d)?;
        }
        Ok(())
    }

    /// A fresh `ArrayBuffer` of `n` zero bytes.
    ///
    /// Above `MAX_BUFFER_BYTES` the buffer is empty; callers check beforehand and
    /// throw a `RangeError`, as a real engine does when allocation fails. In a
    /// kernel an unchecked huge allocation would take down the system, not just
    /// the script.
    pub fn new_buffer(&mut self, n: usize) -> Value {
        if n > MAX_BUFFER_BYTES { return self.new_buffer(0) }
        Value::Obj(new_kind(Some(self.realm.buffer_proto.clone()),
            ObjKind::Buffer(Rc::new(BufData {
                bytes: RefCell::new(alloc::vec![0u8; n]),
                detached: core::cell::Cell::new(false),
            }))))
    }

    /// A view on an existing buffer; no new memory.
    pub fn new_view(&mut self, kind: ElemKind, buf: Gc, offset: usize, len: usize) -> Value {
        let proto = self.realm.ta_protos.get(kind.name()).cloned()
            .unwrap_or_else(|| self.realm.typed_proto.clone());
        Value::Obj(new_kind(Some(proto),
            ObjKind::TypedArray(Rc::new(TaData { buf, kind, offset, len }))))
    }

    /// A view with its own buffer: the ordinary `new Uint8Array(n)`.
    pub fn new_typed(&mut self, kind: ElemKind, len: usize) -> Value {
        let Some(bytes) = len.checked_mul(kind.size()) else { return Value::Undefined };
        let b = self.new_buffer(bytes);
        let Value::Obj(bo) = b else { return Value::Undefined };
        self.new_view(kind, bo, 0, len)
    }

    /// `{ value, done }`, the result of a `next()`.
    pub fn iter_result(&mut self, value: Value, done: bool) -> Value {
        let g = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut o = g.borrow_mut();
            o.define("value", Prop::data(value));
            o.define("done", Prop::data(Value::Bool(done)));
        }
        Value::Obj(g)
    }

    /// An array iterator over `target`. `kind`: 0 values, 1 keys, 2 pairs.
    pub fn array_iter(&mut self, target: Value, kind: u8) -> C<Value> {
        // `ToObject` first: `Array.prototype.values.call("ab")` must work.
        let t = self.to_object(&target)?;
        let g = new_obj(Some(self.realm.array_iter_proto.clone()));
        {
            let mut o = g.borrow_mut();
            o.define(IT_TARGET, Prop::data(Value::Obj(t)));
            o.define(IT_INDEX, Prop::data(Value::Num(0.0)));
            o.define(IT_KIND, Prop::frozen(Value::Num(kind as f64)));
        }
        Ok(Value::Obj(g))
    }

    /// `GetIterator`. Throws if the value has no `Symbol.iterator`, as `for..of`
    /// requires; the message names the reason.
    pub fn get_iterator(&mut self, v: &Value) -> C<Value> {
        if matches!(v, Value::Undefined | Value::Null) {
            return self.type_err("value is not iterable");
        }
        let m = self.get(v, SYM_ITERATOR)?;
        if !self.is_callable(&m) { return self.type_err("value is not iterable"); }
        let it = self.call(&m, v.clone(), &[])?;
        if !matches!(it, Value::Obj(_)) {
            return self.type_err("Symbol.iterator did not return an object");
        }
        Ok(it)
    }

    /// Like `has_property`, but a throw from a proxy trap propagates. `in` and
    /// `Reflect.has` call this.
    pub fn has_prop(&mut self, o: &Gc, key: &str) -> C<bool> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "has")? {
                Some((f, h, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, h, &[t, kv])?;
                    Ok(r.truthy())
                }
                None => { let t = super::proxy::target(self, o)?; self.has_prop(&t, key) }
            };
        }
        Ok(self.has_property(o, key))
    }

    /// The own keys, through a proxy.
    pub fn own_keys_of(&mut self, o: &Gc) -> C<Vec<PropName>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "ownKeys")? {
                Some((f, h, t)) => {
                    let r = self.call(&f, h, &[t])?;
                    let items = self.elems(&r)?;
                    let mut out = Vec::with_capacity(items.len());
                    for v in items { out.push(PropName::from(&*self.to_prop_key(&v)?)); }
                    Ok(out)
                }
                None => { let t = super::proxy::target(self, o)?; self.own_keys_of(&t) }
            };
        }
        Ok(o.borrow().own_keys())
    }

    /// The own descriptor, through a proxy.
    pub fn get_own_desc(&mut self, o: &Gc, key: &str) -> C<Option<Prop>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "getOwnPropertyDescriptor")? {
                Some((f, h, t)) => {
                    let kv = super::proxy::key_value(key);
                    let r = self.call(&f, h, &[t, kv])?;
                    if matches!(r, Value::Undefined) { return Ok(None); }
                    if !matches!(r, Value::Obj(_)) {
                        return self.type_err("getOwnPropertyDescriptor trap did not return an object");
                    }
                    // The trap returns a partial descriptor; a stored property is always
                    // complete, so the missing fields get their defaults here.
                    Ok(Some(self.to_prop_desc(&r)?.into_new_prop()))
                }
                None => { let t = super::proxy::target(self, o)?; self.get_own_desc(&t, key) }
            };
        }
        Ok(o.borrow().get_own(key).cloned())
    }

    /// `[[DefineOwnProperty]]` with validation (ES §10.1.6), through a proxy.
    ///
    /// Non-configurable properties cannot be redefined, `Object.freeze` holds,
    /// and the result reports whether the definition was allowed.
    pub fn define_own(&mut self, o: &Gc, key: &str, d: Desc) -> C<bool> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "defineProperty")? {
                Some((f, h, t)) => {
                    let kv = super::proxy::key_value(key);
                    let dv = self.desc_to_object(&d);
                    let r = self.call(&f, h, &[t, kv, dv])?;
                    Ok(r.truthy())
                }
                None => { let t = super::proxy::target(self, o)?; self.define_own(&t, key, d) }
            };
        }
        let cur = o.borrow().get_own(key).cloned();
        let extensible = o.borrow().extensible;
        let Some(cur) = cur else {
            // New. Only extensibility stands in the way; the missing fields get their
            // defaults here.
            if !extensible { return Ok(false); }
            let np = d.into_new_prop();
            o.borrow_mut().define(key, np);
            self.fix_array_length(o, key);
            return Ok(true);
        };
        if d.is_empty() { return Ok(true); }
        // `ValidateAndApplyPropertyDescriptor`, step 4: what a non-configurable
        // property does not allow.
        if !cur.configurable {
            if d.configurable == Some(true) { return Ok(false); }
            if let Some(e) = d.enumerable { if e != cur.enumerable { return Ok(false); } }
            if !d.is_generic() && d.is_accessor() != cur.is_accessor() { return Ok(false); }
            if cur.is_accessor() {
                let same = |a: &Option<Value>, b: &Option<Value>| {
                    let av = a.clone().unwrap_or(Value::Undefined);
                    let bv = b.clone().unwrap_or(Value::Undefined);
                    av.same_value(&bv)
                };
                if d.get.is_some() && !same(&d.get, &cur.get) { return Ok(false); }
                if d.set.is_some() && !same(&d.set, &cur.set) { return Ok(false); }
            } else if !cur.writable {
                if d.writable == Some(true) { return Ok(false); }
                if let Some(v) = &d.value {
                    let cv = cur.value.clone().unwrap_or(Value::Undefined);
                    if !v.same_value(&cv) { return Ok(false); }
                }
            }
        }
        // Applied field by field: what the descriptor does not name stays.
        let mut np = cur.clone();
        if !d.is_generic() && d.is_accessor() != cur.is_accessor() {
            // Kind changed: the fields of the old kind fall back to their defaults,
            // `enumerable`/`configurable` stay.
            np = Prop { value: None, get: None, set: None, writable: false,
                        enumerable: cur.enumerable, configurable: cur.configurable };
        }
        if let Some(v) = d.value { np.value = Some(v); np.get = None; np.set = None; }
        if let Some(w) = d.writable { np.writable = w; }
        if let Some(g) = d.get { np.get = Some(g); np.value = None; }
        if let Some(st) = d.set { np.set = Some(st); np.value = None; }
        if let Some(e) = d.enumerable { np.enumerable = e; }
        if let Some(c) = d.configurable { np.configurable = c; }
        o.borrow_mut().define(key, np);
        self.fix_array_length(o, key);
        Ok(true)
    }

    /// `FromPropertyDescriptor` for a partial descriptor, as a proxy trap sees
    /// it. Only the fields that are present.
    pub fn desc_to_object(&mut self, d: &Desc) -> Value {
        let o = new_obj(Some(self.realm.object_proto.clone()));
        {
            let mut b = o.borrow_mut();
            if let Some(v) = &d.value { b.define("value", Prop::data(v.clone())); }
            if let Some(w) = d.writable { b.define("writable", Prop::data(Value::Bool(w))); }
            if let Some(g) = &d.get { b.define("get", Prop::data(g.clone())); }
            if let Some(s) = &d.set { b.define("set", Prop::data(s.clone())); }
            if let Some(e) = d.enumerable { b.define("enumerable", Prop::data(Value::Bool(e))); }
            if let Some(c) = d.configurable { b.define("configurable", Prop::data(Value::Bool(c))); }
        }
        Value::Obj(o)
    }

    /// `DefinePropertyOrThrow` (ES §7.3.8), used by the built-ins.
    pub fn define_or_throw(&mut self, o: &Gc, key: &str, d: Desc) -> C<()> {
        if self.define_own(o, key, d)? { return Ok(()); }
        self.type_err(&alloc::format!("cannot redefine property: {key}"))
    }

    /// The prototype, through a proxy.
    pub fn proto_of(&mut self, o: &Gc) -> C<Option<Gc>> {
        if super::proxy::parts(o).is_some() {
            return match super::proxy::trap(self, o, "getPrototypeOf")? {
                Some((f, h, t)) => {
                    let r = self.call(&f, h, &[t])?;
                    Ok(match r { Value::Obj(x) => Some(x), _ => None })
                }
                None => { let t = super::proxy::target(self, o)?; self.proto_of(&t) }
            };
        }
        Ok(o.borrow().proto.clone())
    }

    /// `GetIterator(obj, async)` (ES 7.4.2). Returns the iterator and whether it
    /// is a real async iterator.
    ///
    /// Without `Symbol.asyncIterator` the sync iterator is used. The spec wraps
    /// it in a `%AsyncFromSyncIterator%`; instead we record `is_async = false`
    /// and `Op::IterStepAsync` awaits only its `value`. Same observable
    /// semantics without an object no script ever sees.
    pub fn get_async_iterator(&mut self, v: &Value) -> C<(Value, bool)> {
        if matches!(v, Value::Undefined | Value::Null) {
            return self.type_err("value is not async iterable");
        }
        let m = self.get(v, super::value::SYM_ASYNC_ITERATOR)?;
        if self.is_callable(&m) {
            let it = self.call(&m, v.clone(), &[])?;
            if !matches!(it, Value::Obj(_)) {
                return self.type_err("Symbol.asyncIterator did not return an object");
            }
            return Ok((it, true));
        }
        if !matches!(m, Value::Undefined | Value::Null) {
            return self.type_err("Symbol.asyncIterator is not a function");
        }
        Ok((self.get_iterator(v)?, false))
    }

    /// One step. `None` means done.
    pub fn iter_next(&mut self, it: &Value) -> C<Option<Value>> {
        self.tick()?;
        let f = self.get(it, "next")?;
        if !self.is_callable(&f) { return self.type_err("iterator has no next method"); }
        let r = self.call(&f, it.clone(), &[])?;
        if !matches!(r, Value::Obj(_)) {
            return self.type_err("iterator result is not an object");
        }
        let done = self.get(&r, "done")?;
        if done.truthy() { return Ok(None); }
        Ok(Some(self.get(&r, "value")?))
    }

    /// `IteratorClose`, on early exit (`break`, `return`, an error in the body).
    /// A generator cleans up here; skipping it would leave `finally` blocks in
    /// foreign code unrun.
    ///
    /// An error from `return()` is swallowed: the reason for leaving is already
    /// set, and overwriting it would hide it.
    pub fn iter_close(&mut self, it: &Value) {
        let Ok(f) = self.get(it, "return") else { return };
        if !self.is_callable(&f) { return; }
        let _ = self.call(&f, it.clone(), &[]);
    }

    /// Everything at once, for spread, `Array.from`, `new Map(…)`.
    ///
    /// Eager, which is right here: all callers need the full list. `for..of`
    /// does not go through here but steps lazily (`exec_for_of`); otherwise an
    /// infinite iterator would hang the page even if the body breaks on the
    /// first round.
    pub fn iterate(&mut self, v: &Value) -> C<Vec<Value>> {
        let it = self.get_iterator(v)?;
        let mut out = Vec::new();
        loop {
            match self.iter_next(&it) {
                Ok(Some(x)) => out.push(x),
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// The keys a `for..in` iterates: enumerable, up the whole prototype chain,
    /// without duplicates.
    ///
    /// The list is built up front, so changes to the object during the loop do
    /// not shift it (unlike `for..of`, which must be lazy). A key that disappears
    /// meanwhile is still skipped; `get` handles that.
    ///
    /// `undefined`/`null` give an empty list, not an error: `for (k in null)`
    /// runs zero times.
    ///
    /// Separate because the bytecode machine needs it; a second implementation
    /// would be a second enumeration order.
    pub fn for_in_keys(&mut self, v: &Value) -> C<Vec<Rc<str>>> {
        if matches!(v, Value::Undefined | Value::Null) { return Ok(Vec::new()) }
        let o = self.to_object(v)?;
        let mut keys: Vec<Rc<str>> = Vec::new();
        let mut cur = Some(o);
        let mut hops = 0;
        while let Some(c) = cur {
            if hops > MAX_PROTO_CHAIN { break }
            hops += 1;
            let own = self.own_keys_of(&c)?;
            for k in own {
                if is_sym_key(&k) { continue }
                let enumerable = if super::proxy::parts(&c).is_some() {
                    matches!(self.get_own_desc(&c, &k)?, Some(p) if p.enumerable)
                } else { c.borrow().is_enumerable(&k) };
                if enumerable && !keys.iter().any(|x| *x == k) { keys.push(k); }
            }
            let next = self.proto_of(&c)?;
            cur = next;
        }
        Ok(keys)
    }

    /// `CreateListFromArrayLike`: `length` and indices, without the iterator
    /// protocol.
    ///
    /// Its own spec operation, not a fallback. `Function.prototype.apply` and
    /// the array methods use it: `apply` with an object lacking
    /// `Symbol.iterator` must work, `for..of` on it must throw.
    pub fn elems(&mut self, v: &Value) -> C<Vec<Value>> {
        match v {
            Value::Str(s) => Ok(s.chars().map(|c| {
                let mut t = String::new(); t.push(c); Value::string(t)
            }).collect()),
            Value::Obj(_) => {
                let len = self.get(v, "length")?;
                let n = self.to_number(&len)?;
                let n = if n.is_finite() && n > 0.0 { n as usize } else { 0 };
                let mut out = Vec::with_capacity(n.min(1 << 16));
                for i in 0..n {
                    self.tick()?;
                    out.push(self.get(v, &num_to_string(i as f64))?);
                }
                Ok(out)
            }
            _ => self.type_err("value is not array-like"),
        }
    }

    /// `{ __proto__: v }` in an object literal sets the prototype (ES 13.2.5.5,
    /// PropertyDefinitionEvaluation); it creates no property. Read as a property,
    /// `__proto__` would show up among the own keys.
    ///
    /// Only an object or `null` has an effect; anything else is silently
    /// ignored, as the spec says.
    pub fn set_literal_proto(&mut self, o: &Gc, v: &Value) {
        match v {
            Value::Obj(p) => o.borrow_mut().proto = Some(p.clone()),
            Value::Null => o.borrow_mut().proto = None,
            _ => {}
        }
    }

    /// An array with holes: only the given slots are filled, the length is
    /// fixed regardless.
    ///
    /// A hole is not `undefined`: `new Array(3).concat("x").map(f)` calls `f`
    /// exactly once.
    pub fn new_sparse_array(&mut self, len: usize, items: Vec<(usize, Value)>) -> Value {
        let g = new_kind(Some(self.realm.array_proto.clone()), ObjKind::Array);
        {
            let mut o = g.borrow_mut();
            for (k, v) in items {
                o.define(&num_to_string(k as f64), Prop::data(v));
            }
            o.define("length", Prop {
                value: Some(Value::Num(len as f64)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }

    pub fn new_array(&mut self, items: Vec<Value>) -> Value {
        let g = new_kind(Some(self.realm.array_proto.clone()), ObjKind::Array);
        {
            let mut o = g.borrow_mut();
            let n = items.len();
            for (i, v) in items.into_iter().enumerate() {
                o.define(&num_to_string(i as f64), Prop::data(v));
            }
            o.define("length", Prop {
                value: Some(Value::Num(n as f64)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }

    pub fn make_closure(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>) -> Value {
        self.make_method(f, env, this_val, None)
    }

    /// The same, but with a home object; that is the only difference between a
    /// function and a method, and `super` depends on it.
    pub fn make_method(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>, this_val: Option<Value>,
                       home: Option<Gc>) -> Value {
        // A generator function hangs under `%GeneratorFunction.prototype%`, not
        // `Function.prototype`; `f.constructor` and the `toStringTag` depend on it.
        let fproto = if f.is_generator {
            if f.is_async { self.realm.async_gen_func_proto.clone() }
            else { self.realm.generator_func_proto.clone() }
        } else {
            self.realm.function_proto.clone()
        };
        let g = new_kind(Some(fproto),
            ObjKind::Function(Rc::new(FuncData {
                node: f.clone(), env: env.clone(), this_val, home_object: home,
                class: None,
            })));
        {
            let mut o = g.borrow_mut();
            let len = f.params.iter().take_while(|p| matches!(p, Pat::Ident(_))).count();
            o.define("length", Prop { value: Some(Value::Num(len as f64)), get: None, set: None,
                writable: false, enumerable: false, configurable: true });
            o.define("name", Prop { value: Some(Value::str(f.name.as_deref().unwrap_or(""))),
                get: None, set: None, writable: false, enumerable: false, configurable: true });
        }
        // Arrows and async functions have no `prototype`: they are not constructors,
        // and having one would be a visible difference from every engine. An async
        // generator has one although it is not a constructor either: its objects
        // inherit `next`/`return`/`throw` from there.
        if !f.is_arrow && !(f.is_async && !f.is_generator) {
            // The `prototype` of a generator function hangs under `%GeneratorPrototype%`
            // and has no `constructor`; the generator object inherits
            // `next`/`return`/`throw` from it. An ordinary function gets the usual pair.
            let is_gen = f.is_generator;
            let proto = new_obj(Some(if !is_gen {
                self.realm.object_proto.clone()
            } else if f.is_async {
                self.realm.async_gen_proto.clone()
            } else {
                self.realm.generator_proto.clone()
            }));
            if !is_gen {
                proto.borrow_mut().define("constructor", Prop::builtin(Value::Obj(g.clone())));
            }
            g.borrow_mut().define("prototype", Prop {
                value: Some(Value::Obj(proto)), get: None, set: None,
                writable: true, enumerable: false, configurable: false });
        }
        Value::Obj(g)
    }
}



/// The typed array view behind an object, if it is one.
pub fn ta_of(o: &Gc) -> Option<Rc<TaData>> {
    match &o.borrow().kind {
        ObjKind::TypedArray(t) => Some(t.clone()),
        _ => None,
    }
}

/// Read an element of a view. `None` means "not a view or not an index",
/// and the ordinary path continues. `Some(Undefined)` means "view, but out of
/// range", which is an answer, not a fall-through.
pub fn ta_read(o: &Gc, key: &str) -> Option<Value> {
    let t = ta_of(o)?;
    let k = array_index(key)? as usize;
    if k >= t.live_len() { return Some(Value::Undefined) }
    let ObjKind::Buffer(b) = &t.buf.borrow().kind else { return Some(Value::Undefined) };
    let at = t.offset + k * t.kind.size();
    Some(t.kind.read_v(&b.bytes.borrow(), at))
}
