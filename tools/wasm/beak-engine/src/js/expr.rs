//! Expression evaluation (tree walker).

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::ast::*;
use super::interp::*;
use super::value::*;

impl Interp {
    pub fn eval(&mut self, e: &Expr, env: &Rc<RefCell<Env>>) -> C<Value> {
        match e {
            Expr::Num(n) => Ok(Value::Num(*n)),
            Expr::BigInt(t) => match crate::js::bigint::Big::parse(t) {
                Some(b) => Ok(Value::BigInt(Rc::new(b))),
                None => Err(self.throw_kind("SyntaxError", "invalid BigInt literal")),
            },
            Expr::Str(s) => Ok(Value::str(s)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::This => Ok(super::interp::this_observed(self, env)),
            Expr::Ident(n) => self.load_ident(n, env),
            Expr::Regex { body, flags } => super::regexp::make(self, body, flags),
            Expr::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    match it {
                        None => out.push(Value::Undefined),
                        Some(Expr::Spread(inner)) => {
                            let v = self.eval(inner, env)?;
                            out.extend(self.iterate(&v)?);
                        }
                        Some(x) => out.push(self.eval(x, env)?),
                    }
                }
                Ok(self.new_array(out))
            }
            Expr::Object(props) => self.eval_object(props, env),
            Expr::Func(f) => Ok(self.func_value(f.clone(), env)),
            Expr::Class(c) => self.eval_class(c, env),
            Expr::Template { quasis, exprs } => {
                let mut s = String::new();
                for (i, q) in quasis.iter().enumerate() {
                    s.push_str(q.cooked.as_deref().unwrap_or(""));
                    if let Some(x) = exprs.get(i) {
                        let v = self.eval(x, env)?;
                        s.push_str(&self.to_string(&v)?);
                    }
                }
                Ok(Value::string(s))
            }
            Expr::Seq(list) => {
                let mut last = Value::Undefined;
                for x in list { last = self.eval(x, env)?; }
                Ok(last)
            }
            Expr::Unary { op, arg } => self.eval_unary(*op, arg, env),
            Expr::Update { op, arg, prefix } => self.eval_update(*op, arg, *prefix, env),
            Expr::Binary { op, left, right } => {
                // `#x in obj` checks the private brand, not a property; the
                // left side is not a value to evaluate.
                if *op == BinOp::In {
                    if let Expr::Ident(n) = &**left {
                        if let Some(name) = n.strip_prefix('#') {
                            let name = alloc::string::String::from(name);
                            let r = self.eval(right, env)?;
                            return self.private_in(&name, &r);
                        }
                    }
                }
                let l = self.eval(left, env)?;
                let r = self.eval(right, env)?;
                self.binary(*op, l, r)
            }
            Expr::Logical { op, left, right } => {
                let l = self.eval(left, env)?;
                match op {
                    LogicalOp::And => if l.truthy() { self.eval(right, env) } else { Ok(l) },
                    LogicalOp::Or => if l.truthy() { Ok(l) } else { self.eval(right, env) },
                    LogicalOp::Nullish =>
                        if matches!(l, Value::Undefined | Value::Null) { self.eval(right, env) } else { Ok(l) },
                }
            }
            Expr::Cond { test, cons, alt } => {
                if self.eval(test, env)?.truthy() { self.eval(cons, env) } else { self.eval(alt, env) }
            }
            Expr::Assign { op, left, right } => self.eval_assign(*op, left, right, env),
            Expr::Member { obj, prop, optional } => {
                if matches!(**obj, Expr::Super) {
                    let key = self.member_key2(prop, env)?;
                    let (f, _) = self.super_lookup(&key, env)?;
                    return Ok(f);
                }
                let base = self.eval(obj, env)?;
                if *optional && matches!(base, Value::Undefined | Value::Null) {
                    return Ok(Value::Undefined);
                }
                let key = self.member_key2(prop, env)?;
                self.get(&base, &key)
            }
            Expr::Chain(inner) => self.eval(inner, env),
            Expr::Call { callee, args, optional } => self.eval_call(callee, args, *optional, env),
            Expr::New { callee, args } => {
                let f = self.eval(callee, env)?;
                let a = self.eval_args(args, env)?;
                // Same name as the bytecode VM passes, so both engines report
                // the same error message.
                let name = super::compile::dotted_name(callee);
                self.construct_named(&f, &a, name.as_deref())
            }
            Expr::Spread(inner) => self.eval(inner, env),
            Expr::Super => Ok(Value::Undefined),
            // `import.meta` is a binding in the module environment; the scope
            // chain finds the right one, even from a callback. Outside a
            // module there is none, so `undefined`, like `new.target`.
            Expr::MetaProp { meta, prop } => {
                if meta == "import" && prop == "meta" {
                    let n = super::modules::META_LOCAL;
                    if let Some(e) = env_lookup(env, n) {
                        if let Some(b) = e.borrow().vars.get(n) { return Ok(b.value.clone()); }
                    }
                }
                Ok(Value::Undefined)
            }
            Expr::ImportCall(_) => self.type_err("dynamic import is not supported"),
            // `tag`a${x}b`` (ES 13.2.8.6): the tag receives the template
            // object first, then the substitutions. The receiver counts:
            // `o.tag`x`` calls with `o` as `this`, like an ordinary call.
            Expr::TaggedTemplate { tag, quasis, exprs } => {
                let (this_val, f) = match &**tag {
                    Expr::Member { obj, prop, optional } => {
                        let base = self.eval(obj, env)?;
                        if *optional && matches!(base, Value::Undefined | Value::Null) {
                            return Ok(Value::Undefined);
                        }
                        let key = self.member_key2(prop, env)?;
                        let f = self.get(&base, &key)?;
                        if !self.is_callable(&f) {
                            return Err(self.not_a_function(Some(&key), Some(&base)));
                        }
                        (base, f)
                    }
                    other => (Value::Undefined, self.eval(other, env)?),
                };
                if !self.is_callable(&f) {
                    return self.type_err("tag is not a function");
                }
                let mut a = alloc::vec![self.template_object(quasis)];
                for x in exprs { let v = self.eval(x, env)?; a.push(v); }
                self.call(&f, this_val, &a)
            }
            Expr::Yield { .. } => self.type_err("generators are not supported"),
            Expr::Await(_) => self.type_err("await is not supported"),
        }
    }

    fn load_ident(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {
        Ok(self.load_ident_depth(n, env)?.0)
    }

    /// Does the object environment of a `with` have this name?
    ///
    /// Not just `HasProperty`: `Symbol.unscopables` can hide a present name,
    /// so `with([]) { keys }` does not find `Array.prototype.keys`.
    pub(crate) fn with_has_pub(&mut self, o: &Gc, n: &str) -> C<bool> { self.with_has(o, n) }

    fn with_has(&mut self, o: &Gc, n: &str) -> C<bool> {
        if !self.has_property(o, n) { return Ok(false) }
        let un = self.get(&Value::Obj(o.clone()), super::value::SYM_UNSCOPABLES)?;
        if let Value::Obj(_) = un {
            if self.get(&un, n)?.truthy() { return Ok(false) }
        }
        Ok(true)
    }

    /// Like `load_ident`, but also returns the depth at which the name was
    /// found; `None` means "not in the chain" (global object or `with`
    /// object). Only the lookup hints need it; see `Chunk::hints`.
    fn load_ident_depth(&mut self, n: &str, env: &Rc<RefCell<Env>>)
        -> C<(Value, Option<usize>)> {
        // One walk up the chain, hashing the name once per scope.
        let mut cur = env.clone();
        let mut depth = 0usize;
        loop {
            // The object environment of a `with` first. `None` is only a null
            // check; it costs something only where a `with` exists.
            let wo = cur.borrow().with_obj.clone();
            if let Some(o) = wo {
                if self.with_has(&o, n)? {
                    // No depth: a lookup hint must not point at a binding
                    // that comes from an object.
                    return Ok((self.get(&Value::Obj(o), n)?, None));
                }
            }
            match super::interp::env_peek(&cur, n) {
                Hit::Val(v) => return Ok((v, Some(depth))),
                Hit::Dead =>
                    return self.ref_err(
                        &alloc::format!("cannot access '{n}' before initialization")),
                // An imported name lives in its source module and is read
                // from there on every access (live binding).
                Hit::Import(e, n2) => return Ok((self.load_import(e, n2, n)?, None)),
                Hit::Up(Some(p)) => { cur = p; depth += 1; }
                Hit::Up(None) => break,
            }
        }
        // Not in the chain: ask the global object, else ReferenceError (not
        // `undefined`).
        let g = self.realm.global.clone();
        if self.has_property(&g, n) { return Ok((self.get(&Value::Obj(g), n)?, None)); }
        self.ref_err(&alloc::format!("{n} is not defined"))
    }

    /// Follow an `import` to the actual binding. The rare path; it may
    /// allocate an `Rc<str>`.
    fn load_import(&mut self, e: Rc<RefCell<Env>>, n2: Rc<str>, n: &str) -> C<Value> {
        let Some((e, n2)) = super::interp::env_deref_from(e, n2) else {
            return self.ref_err(&alloc::format!("circular import binding for '{n}'"));
        };
        let b = e.borrow();
        let Some(bd) = b.vars.get(&*n2) else {
            drop(b);
            return self.ref_err(&alloc::format!("{n} is not exported"));
        };
        if !bd.initialized {
            drop(b);
            return self.ref_err(&alloc::format!("cannot access '{n}' before initialization"));
        }
        Ok(bd.value.clone())
    }

    /// `super(...)`: run the parent constructor on this `this`.
    ///
    /// The object already exists; `construct` created it before the body
    /// ran. Public because the bytecode VM calls it, and instance fields are
    /// initialized here.
    pub fn super_call(&mut self, args: &[Value], env: &Rc<RefCell<Env>>) -> C<Value> {
        let this_val = env_this(env);
        let parent = self.super_parent(env)?;
        let ctor = self.get(&Value::Obj(parent), "constructor")?;
        if !self.is_callable(&ctor) {
            return self.type_err("super: the parent class has no constructor");
        }
        // A builtin parent constructor builds its own object and cannot fill
        // `this` (e.g. `class E extends Error {}`). So the built object is
        // adopted: its kind and own properties move over, while the derived
        // class's prototype stays.
        let native_parent = matches!(&ctor, Value::Obj(o)
            if matches!(o.borrow().kind, ObjKind::Native(_)));
        if native_parent {
            // The builtin receives the freshly created `this` as receiver.
            // It still builds its own object, but this tells it which class
            // is being constructed; `HTMLElement` reads the registered name
            // from the prototype chain.
            let built = self.construct_on(&ctor, this_val.clone(), args)?;
            // A builtin that honoured the new target already returns the finished
            // object (`HTMLElement` hands out the element itself, which an upgrade
            // must keep); it becomes `this` as is.
            if let (Value::Obj(src), Value::Obj(dst)) = (&built, &this_val) {
                let same = match (&src.borrow().proto, &dst.borrow().proto) {
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                };
                if same && !Rc::ptr_eq(src, dst) {
                    set_env_this(env, built.clone());
                    return self.finish_super(env, built);
                }
            }
            if let (Value::Obj(src), Value::Obj(dst)) = (&built, &this_val) {
                let keys = src.borrow().raw_keys();
                for k in keys {
                    let p = src.borrow().get_own(&k).cloned();
                    if let Some(p) = p { dst.borrow_mut().set_prop(k, p); }
                }
                let kind = core::mem::replace(&mut src.borrow_mut().kind, ObjKind::Plain);
                dst.borrow_mut().kind = kind;
                // Update back-references too: a builtin constructor may have
                // registered its object elsewhere (`HTMLElement` attaches it
                // to the DOM node). Otherwise tree and instance would refer to
                // two different objects.
                super::dombind::readopt(self, src, dst);
            }
        } else {
            let r = self.call(&ctor, this_val.clone(), args)?;
            // If the parent constructor returns an object, that becomes
            // `this` (`class Base { constructor(o) { return o } }`).
            if let Value::Obj(o) = &r {
                if !matches!(&this_val, Value::Obj(t) if Rc::ptr_eq(t, o)) {
                    set_env_this(env, r.clone());
                    return self.finish_super(env, r);
                }
            }
        }
        self.finish_super(env, this_val)
    }

    /// Initialize the class's own instance fields, after the parent
    /// constructor as the spec requires (an initializer may read a parent
    /// field). The home object's `constructor` identifies the class.
    fn finish_super(&mut self, env: &Rc<RefCell<Env>>, this_val: Value) -> C<Value> {
        if let Some(home) = env_home(env) {
            let own = self.get(&Value::Obj(home), "constructor")?;
            if let Value::Obj(co) = &own {
                let d = match &co.borrow().kind {
                    ObjKind::Function(d) => Some(d.clone()),
                    _ => None,
                };
                if let Some(d) = d { self.init_fields(&d, &this_val)?; }
            }
        }
        Ok(Value::Undefined)
    }

    /// `super.k`: the value comes from the parent, `this` stays the current
    /// one. Public because both engines call it.
    pub fn super_get(&mut self, key: &str, env: &Rc<RefCell<Env>>) -> C<(Value, Value)> {
        self.super_lookup(key, env)
    }

    /// The prototype `super` looks up on: that of the home object.
    fn super_parent(&mut self, env: &Rc<RefCell<Env>>) -> C<Gc> {
        let Some(home) = env_home(env) else {
            return self.type_err("'super' outside of a method");
        };
        let proto = home.borrow().proto.clone();
        match proto {
            Some(p) => Ok(p),
            None => self.type_err("'super' in a class without a base"),
        }
    }

    /// Resolve `super.k`: the value comes from the parent prototype, `this`
    /// stays the current receiver.
    fn super_lookup(&mut self, key: &str, env: &Rc<RefCell<Env>>) -> C<(Value, Value)> {
        let parent = self.super_parent(env)?;
        let this_val = env_this(env);
        let f = self.get(&Value::Obj(parent), key)?;
        Ok((f, this_val))
    }

    /// The key of a member access. Keep this the only implementation;
    /// private names must map to the same key (`private_key`) everywhere.
    pub fn member_key2(&mut self, p: &MemberProp, env: &Rc<RefCell<Env>>) -> C<Rc<str>> {
        Ok(match p {
            MemberProp::Ident(n) => Rc::from(n.as_str()),
            MemberProp::Private(n) => private_key(n),
            MemberProp::Computed(e) => { let v = self.eval(e, env)?; self.to_prop_key(&v)? }
        })
    }

    fn eval_args(&mut self, args: &[Arg], env: &Rc<RefCell<Env>>) -> C<Vec<Value>> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            match a {
                Arg::Expr(e) => out.push(self.eval(e, env)?),
                Arg::Spread(e) => { let v = self.eval(e, env)?; out.extend(self.iterate(&v)?); }
            }
        }
        Ok(out)
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Arg], optional: bool,
                 env: &Rc<RefCell<Env>>) -> C<Value> {
        // `a.b()` binds `this` to `a`, so the receiver is fetched separately
        // rather than via `eval(callee)`, which would lose it.
        // `super(...)` calls the parent constructor on the existing `this`.
        if matches!(callee, Expr::Super) {
            let a = self.eval_args(args, env)?;
            return self.super_call(&a, env);
        }
        let (this_val, f) = match callee {
            Expr::Member { obj, prop, optional: mopt } if matches!(**obj, Expr::Super) => {
                let key = self.member_key2(prop, env)?;
                let _ = mopt;
                let (f, this_val) = self.super_lookup(&key, env)?;
                if !self.is_callable(&f) {
                    return self.type_err(&alloc::format!("super.{key} is not a function"));
                }
                (this_val, f)
            }
            Expr::Member { obj, prop, optional: mopt } => {
                let base = self.eval(obj, env)?;
                if *mopt && matches!(base, Value::Undefined | Value::Null) {
                    return Ok(Value::Undefined);
                }
                let key = self.member_key2(prop, env)?;
                let f = self.get(&base, &key)?;
                if !self.is_callable(&f) && !optional {
                    return Err(self.not_a_function(Some(&key), Some(&base)));
                }
                (base, f)
            }
            // `with (o) { m() }` calls `m` with `o` as receiver
            // (ES 9.1.1.2.4, WithBaseObject).
            Expr::Ident(n) => match self.with_target(n, env)? {
                Some(o) => (Value::Obj(o), self.eval(callee, env)?),
                None => (Value::Undefined, self.eval(callee, env)?),
            },
            _ => (Value::Undefined, self.eval(callee, env)?),
        };
        if optional && matches!(f, Value::Undefined | Value::Null) { return Ok(Value::Undefined); }
        let a = self.eval_args(args, env)?;
        // Direct `eval`: recognized by name and by identity. Both engines do
        // the same, see `Op::Call`.
        if matches!(callee, Expr::Ident(n) if n == "eval") && self.is_eval_fn(&f) {
            self.hints_ok = false;
            let c = a.first().cloned().unwrap_or(Value::Undefined);
            return self.perform_eval(&c, Some(env.clone()));
        }
        if !self.is_callable(&f) {
            // Name the callee, same as the bytecode VM; see
            // `Interp::not_a_function`.
            return Err(self.not_a_function(match callee {
                Expr::Ident(n) => Some(n.as_str()),
                _ => None,
            }, Some(&this_val)));
        }
        self.call(&f, this_val, &a)
    }

    /// Construct, passing the callee's name for the error message.
    pub fn construct_named(&mut self, f: &Value, args: &[Value], name: Option<&str>) -> C<Value> {
        let was = core::mem::replace(&mut self.new_name, name.map(alloc::string::String::from));
        let r = self.construct(f, args);
        self.new_name = was;
        r
    }

    pub fn construct(&mut self, f: &Value, args: &[Value]) -> C<Value> {
        self.construct_on(f, Value::Undefined, args)
    }

    /// `new` with a separate new target, as in
    /// `Reflect.construct(target, args, newTarget)`.
    ///
    /// The new target determines the prototype. Babel/SWC compile
    /// `class X extends Y` to `Reflect.construct(Y, args, X)`, so the
    /// instance gets `X.prototype` rather than `Y.prototype`.
    pub fn construct_target(&mut self, f: &Value, args: &[Value], nt: &Value) -> C<Value> {
        self.construct_full(f, Value::Undefined, args, Some(nt))
    }

    /// Like `construct`, with a receiver for the builtin case.
    ///
    /// Only `super()` passes one: the fresh `this` already carries the
    /// derived class's prototype, the only hint of what is being built.
    /// Plain `new` passes `undefined`.
    pub fn construct_on(&mut self, f: &Value, recv: Value, args: &[Value]) -> C<Value> {
        self.construct_full(f, recv, args, None)
    }

    /// The single construction path. `nt` is the new target when it differs
    /// from the called constructor (`Reflect.construct`).
    fn construct_full(&mut self, f: &Value, recv: Value, args: &[Value], nt: Option<&Value>)
        -> C<Value> {
        let Value::Obj(fo) = f else { return Err(self.not_a_constructor(f)) };
        // The `construct` trap.
        if super::proxy::parts(fo).is_some() {
            return match super::proxy::trap(self, fo, "construct")? {
                Some((fn_, hv, t)) => {
                    let arr = self.new_array(args.to_vec());
                    let ntv = nt.cloned().unwrap_or_else(|| f.clone());
                    let r = self.call(&fn_, hv, &[t, arr, ntv])?;
                    if !matches!(r, Value::Obj(_)) {
                        return self.type_err("construct trap did not return an object");
                    }
                    Ok(r)
                }
                None => { let t = super::proxy::target(self, fo)?;
                          self.construct_full(&Value::Obj(t), Value::Undefined, args, nt) }
            };
        }
        // A native constructor builds its own object; an arrow is none.
        if let ObjKind::Native(n) = &fo.borrow().kind {
            if !n.ctor { return Err(self.not_a_constructor(f)); }
            let nf = n.clone();
            // With a separate new target the builtin learns the class through the
            // receiver, as with `super()`, and the result gets its prototype
            // (GetPrototypeFromConstructor).
            let nt_proto = match nt {
                Some(ntv) => match self.get(ntv, "prototype")? { Value::Obj(p) => Some(p), _ => None },
                None => None,
            };
            let recv = match (&recv, &nt_proto) {
                (Value::Undefined, Some(p)) => Value::Obj(new_obj(Some(p.clone()))),
                _ => recv,
            };
            let was = self.native_new;
            self.native_new = true;
            let r = (nf.func)(self, recv, args);
            self.native_new = was;
            if let (Ok(Value::Obj(o)), Some(p)) = (&r, &nt_proto) {
                o.borrow_mut().proto = Some(p.clone());
            }
            return r;
        }
        if !self.is_constructor(f) { return Err(self.not_a_constructor(f)); }
        // The prototype comes from the new target, not the called
        // constructor. Without a new target both are the same.
        let proto_from = nt.unwrap_or(f).clone();
        let proto = match self.get(&proto_from, "prototype")? {
            Value::Obj(p) => Some(p),
            _ => Some(self.realm.object_proto.clone()),
        };
        let obj = new_obj(proto);
        let r = self.call(f, Value::Obj(obj.clone()), args)?;
        // If the constructor returns an object, it wins; otherwise the fresh
        // object does.
        Ok(match r { Value::Obj(_) => r, _ => Value::Obj(obj) })
    }

    /// Install a getter or setter on a property. Both must land on the same
    /// property, or the second hides the first. Public for the bytecode VM.
    pub fn define_accessor(&mut self, g: &Gc, key: Rc<str>, f: Value, is_get: bool) {
        let mut existing = g.borrow().get_own(&key).cloned().unwrap_or(Prop {
            value: None, get: None, set: None,
            writable: false, enumerable: true, configurable: true });
        existing.value = None;
        if is_get { existing.get = Some(f); } else { existing.set = Some(f); }
        g.borrow_mut().set_prop(key, existing);
    }

    /// `{...src}`: copy the enumerable own properties.
    pub fn spread_into(&mut self, g: &Gc, src: &Value) -> C<()> {
        if let Value::Obj(o) = src {
            // Keys and enumerability go through proxy traps.
            for k in self.own_keys_of(o)? {
                if !self.get_own_desc(o, &k)?.is_some_and(|p| p.enumerable) { continue }
                let val = self.get(src, &k)?;
                g.borrow_mut().set_prop(k, Prop::data(val));
            }
        }
        Ok(())
    }

    fn eval_object(&mut self, props: &[ObjProp], env: &Rc<RefCell<Env>>) -> C<Value> {
        let g = new_obj(Some(self.realm.object_proto.clone()));
        for p in props {
            match &p.value {
                ObjPropValue::Spread(e) => {
                    let v = self.eval(e, env)?;
                    self.spread_into(&g, &v)?;
                }
                ObjPropValue::Init(e) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.eval(e, env)?;
                    // `__proto__: v` sets the prototype only when written as a
                    // literal key, neither computed nor shorthand; `{[k]: v}`
                    // and `{__proto__}` are ordinary properties (Annex B).
                    if !p.computed && !p.shorthand && &*key == "__proto__" {
                        self.set_literal_proto(&g, &v);
                        continue;
                    }
                    self.name_function(&v, &key);
                    g.borrow_mut().set_prop(key, Prop::data(v));
                }
                ObjPropValue::Method(f) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.make_closure(f.clone(), env, None);
                    self.name_function(&v, &key);
                    g.borrow_mut().set_prop(key, Prop::data(v));
                }
                ObjPropValue::Get(f) | ObjPropValue::Set(f) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.make_closure(f.clone(), env, None);
                    let is_get = matches!(p.value, ObjPropValue::Get(_));
                    // An accessor is named `"get x"`/`"set x"`, not `"x"`.
                    let show = alloc::format!("{} {key}", if is_get { "get" } else { "set" });
                    self.name_function(&v, &show);
                    self.define_accessor(&g, key, v, is_get);
                }
            }
        }
        Ok(Value::Obj(g))
    }

    pub fn eval_class(&mut self, c: &Rc<Class>, env: &Rc<RefCell<Env>>) -> C<Value> {
        // Two chains: instances inherit from `Parent.prototype`, the
        // constructor itself from the parent class.
        let (parent_proto, parent_ctor) = match &c.super_class {
            Some(e) => {
                let sv = self.eval(e, env)?;
                match sv {
                    // `class X extends null` is allowed (ES 15.7.14 step
                    // 8.d.i): the instance prototype has no parent, the
                    // constructor inherits from `%Function.prototype%`.
                    Value::Null => (None, None),
                    // A non-constructor is reported with the class name.
                    _ if !self.is_constructor(&sv) => {
                        let n = c.name.clone().unwrap_or_else(|| String::from("(anonym)"));
                        let was = sv.type_of();
                        return self.type_err(&alloc::format!(
                            "class {n}: extends value is {was}, not a constructor or null"));
                    }
                    _ => {
                        let pp = match self.get(&sv, "prototype")? {
                            Value::Obj(p) => Some(p),
                            Value::Null => None,
                            _ => {
                                let n = c.name.clone().unwrap_or_else(|| String::from("(anonym)"));
                                return self.type_err(&alloc::format!(
                                    "class {n}: superclass prototype is not an object or null"));
                            }
                        };
                        (pp, sv.as_obj().cloned())
                    }
                }
            }
            None => (Some(self.realm.object_proto.clone()), None),
        };
        // A named class is bound in its own body (ES 15.7.14), so
        // `class C { ... new C() ... }` works even if the outer name is
        // reassigned. Same scope as for a named function expression.
        //
        // The superclass is evaluated in the outer scope, where this binding
        // does not exist yet.
        let cenv = match &c.name {
            Some(_) => Env::new(Some(env.clone()), false),
            None => env.clone(),
        };
        let env = &cenv;
        let proto = new_obj(parent_proto);

        // The constructor is the class. If absent, a default one is created.
        let ctor_node = c.body.iter().find_map(|m| match m {
            ClassMember::Method { func, kind: MethodKind::Constructor, .. } => Some(func.clone()),
            _ => None,
        });
        let ctor_fn = match ctor_node {
            Some(f) => f,
            // A missing constructor in a derived class is not empty: it
            // forwards its arguments (`constructor(...a) { super(...a) }`).
            None if c.super_class.is_some() => Rc::new(Func {
                name: c.name.clone(),
                params: alloc::vec![Pat::Rest(Box::new(Pat::Ident(String::from("args"))))],
                body: alloc::vec![Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::Super),
                    args: alloc::vec![Arg::Spread(Expr::Ident(String::from("args")))],
                    optional: false,
                })],
                is_async: false, is_generator: false, is_arrow: false, expr_body: false,
                strict: true,
            }),
            None => Rc::new(Func {
                name: c.name.clone(), params: Vec::new(), body: Vec::new(),
                is_async: false, is_generator: false, is_arrow: false, expr_body: false,
                strict: true,
            }),
        };
        let ctor = self.make_method(ctor_fn, env, None, Some(proto.clone()));
        // Every class constructor carries its class, not only those with
        // instance fields: it also marks the function as a constructor, so a
        // body without `return` yields `this`. Without fields `init_fields`
        // is a no-op.
        {
            if let Value::Obj(co) = &ctor {
                let d = match &co.borrow().kind {
                    ObjKind::Function(d) => Some(d.clone()),
                    _ => None,
                };
                if let Some(d) = d {
                    co.borrow_mut().kind = ObjKind::Function(Rc::new(FuncData {
                        node: d.node.clone(), env: d.env.clone(),
                        this_val: d.this_val.clone(), home_object: d.home_object.clone(),
                        class: Some(c.clone()),
                    }));
                }
            }
        }
        if let Value::Obj(co) = &ctor {
            co.borrow_mut().define("prototype", Prop {
                value: Some(Value::Obj(proto.clone())), get: None, set: None,
                writable: false, enumerable: false, configurable: false });
            co.borrow_mut().define("name", Prop {
                value: Some(Value::str(c.name.as_deref().unwrap_or(""))), get: None, set: None,
                writable: false, enumerable: false, configurable: true });
        }
        // Bind now: a static field is evaluated within this function and may
        // already refer to the class.
        if let Some(n) = &c.name {
            cenv.borrow_mut().vars.insert(Rc::from(n.as_str()),
                Binding { value: ctor.clone(), mutable: false, initialized: true });
        }
        // Static inheritance: the constructor's prototype is the parent
        // class, so `B.create()` finds `static create` of `A` and
        // `Object.getPrototypeOf(B) === A`.
        if let (Some(p), Value::Obj(co)) = (&parent_ctor, &ctor) {
            co.borrow_mut().proto = Some(p.clone());
        }
        proto.borrow_mut().define("constructor", Prop::builtin(ctor.clone()));

        for m in &c.body {
            match m {
                ClassMember::Method { key, func, kind, is_static, .. } => {
                    if *kind == MethodKind::Constructor { continue; }
                    let target = if *is_static { ctor.as_obj().unwrap().clone() } else { proto.clone() };
                    let k = self.prop_key(key, env)?;
                    let v = self.make_method(func.clone(), env, None, Some(target.clone()));
                    let show = match kind {
                        MethodKind::Get => alloc::format!("get {k}"),
                        MethodKind::Set => alloc::format!("set {k}"),
                        _ => alloc::string::String::from(&*k),
                    };
                    self.name_function(&v, &show);
                    match kind {
                        MethodKind::Get | MethodKind::Set => {
                            let mut p = target.borrow().get_own(&k).cloned().unwrap_or(Prop {
                                value: None, get: None, set: None,
                                writable: false, enumerable: false, configurable: true });
                            p.value = None;
                            if *kind == MethodKind::Get { p.get = Some(v); } else { p.set = Some(v); }
                            target.borrow_mut().set_prop(k, p);
                        }
                        // Class methods are not enumerable, unlike object
                        // literal methods.
                        _ => { target.borrow_mut().set_prop(k, Prop::builtin(v)); }
                    }
                }
                ClassMember::Field { key, value, is_static, .. } if *is_static => {
                    let k = self.prop_key(key, env)?;
                    let v = match value { Some(e) => self.eval(e, env)?, None => Value::Undefined };
                    ctor.as_obj().unwrap().borrow_mut().set_prop(k, Prop::data(v));
                }
                // Per-instance fields are initialized in the constructor; see
                // `Interp::init_fields`.
                _ => {}
            }
        }
        Ok(ctor)
    }

    fn eval_unary(&mut self, op: UnaryOp, arg: &Expr, env: &Rc<RefCell<Env>>) -> C<Value> {
        // `typeof x` on an unknown name does not throw; it is the classic
        // way to probe for a global.
        if op == UnaryOp::Typeof {
            if let Expr::Ident(n) = arg {
                // A `with` object provides the name just like a binding.
                if self.with_target(n, env)?.is_none() && env_lookup(env, n).is_none() {
                    let g = self.realm.global.clone();
                    if !self.has_property(&g, n) { return Ok(Value::str("undefined")); }
                }
            }
        }
        if op == UnaryOp::Delete {
            return Ok(match arg {
                Expr::Member { obj, prop, .. } => {
                    let base = self.eval(obj, env)?;
                    let key = self.member_key2(prop, env)?;
                    let ok = self.delete_key(&base, &key)?;
                    if !ok {
                        super::interp::strict_site!(self, 7);
                        // Strict code throws on a failed `delete`
                        // (ES §13.5.1.2).
                        if super::interp::env_strict(env) {
                            return self.type_err(&alloc::format!(
                                "cannot delete property '{key}'"));
                        }
                    }
                    Value::Bool(ok)
                }
                // `delete x` on a plain name is `true` and a no-op, except
                // inside a `with`, where it deletes the object's property
                // (ES 13.5.1.2 step 5).
                Expr::Ident(n) => match self.with_target(n, env)? {
                    Some(o) => {
                        let k: Rc<str> = Rc::from(&**n);
                        Value::Bool(self.delete_key(&Value::Obj(o), &k)?)
                    }
                    None => Value::Bool(true),
                },
                _ => Value::Bool(true),
            });
        }
        let v = self.eval(arg, env)?;
        self.unary_val(op, v)
    }

    /// The value of a function expression.
    ///
    /// A named function expression sees its own name in its body
    /// (`(function e(){ ... e ... })`); minified code recurses that way. A
    /// declaration does not get this inner binding: its name is already
    /// bound outside, and an inner one would shadow reassignments.
    ///
    /// Public because the bytecode VM calls it.
    pub fn func_value(&mut self, f: Rc<Func>, env: &Rc<RefCell<Env>>) -> Value {
        if !f.is_arrow {
            if let Some(n) = f.name.clone() {
                let inner = Env::new(Some(env.clone()), false);
                let cl = self.make_closure(f, &inner, None);
                inner.borrow_mut().vars.insert(Rc::from(n.as_str()),
                    Binding { value: cl.clone(), mutable: false, initialized: true });
                return cl;
            }
        }
        let this_val = if f.is_arrow { Some(env_this(env)) } else { None };
        self.make_closure(f, env, this_val)
    }

    /// `delete obj[key]`: `false` only if the property exists and is not
    /// configurable. A missing property gives `true`.
    pub fn delete_key(&mut self, base: &Value, key: &str) -> C<bool> {
        if let Value::Obj(o) = base {
            if super::proxy::parts(o).is_some() {
                return match super::proxy::trap(self, o, "deleteProperty")? {
                    Some((f, h, t)) => {
                        let kv = super::proxy::key_value(key);
                        let r = self.call(&f, h, &[t, kv])?;
                        Ok(r.truthy())
                    }
                    None => { let t = super::proxy::target(self, o)?;
                              self.delete_key(&Value::Obj(t), key) }
                };
            }
        }
        Ok(match base {
            Value::Obj(o) => {
                let cfg = o.borrow().get_own(key).map(|p| p.configurable);
                match cfg {
                    Some(false) => false,
                    Some(true) => { o.borrow_mut().remove(key); true }
                    None => true,
                }
            }
            _ => true,
        })
    }

    /// `DeletePropertyOrThrow` (ES §7.3.9), for builtins. The `delete`
    /// operator returns `false` (outside strict mode); a builtin must throw.
    pub fn delete_or_throw(&mut self, base: &Value, key: &str) -> C<()> {
        if self.delete_key(base, key)? { return Ok(()); }
        self.type_err(&alloc::format!("cannot delete property '{key}'"))
    }

    /// The value part of a prefix operator: everything except `delete` and
    /// `typeof` on an unknown name, which need the expression itself.
    /// Shared with the bytecode VM so both use one implementation.
    pub fn unary_val(&mut self, op: UnaryOp, v: Value) -> C<Value> {
        Ok(match op {
            UnaryOp::Minus => {
                let p = self.to_primitive(&v, false)?;
                if let Value::BigInt(b) = &p { return Ok(Value::BigInt(Rc::new(b.negate()))); }
                Value::Num(-self.to_number(&p)?)
            }
            // `+x` throws on a BigInt instead of converting to `f64`.
            UnaryOp::Plus => Value::Num(self.to_number(&v)?),
            UnaryOp::Bang => Value::Bool(!v.truthy()),
            UnaryOp::Tilde => {
                let p = self.to_primitive(&v, false)?;
                if let Value::BigInt(b) = &p { return Ok(Value::BigInt(Rc::new(b.not()))); }
                Value::Num(!to_int32(self.to_number(&p)?) as f64)
            }
            UnaryOp::Typeof => Value::str(v.type_of()),
            UnaryOp::Void => Value::Undefined,
            UnaryOp::Delete => unreachable!(),
        })
    }

    /// `typeof <name>`: does not throw for an unknown name.
    pub fn typeof_ident(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {
        // A `with` object may provide the name.
        if self.with_target(n, env)?.is_some() {
            let v = self.load_ident(n, env)?;
            return self.unary_val(UnaryOp::Typeof, v);
        }
        if env_lookup(env, n).is_none() {
            let g = self.realm.global.clone();
            if !self.has_property(&g, n) {
                return Ok(Value::str("undefined"));
            }
        }
        let v = self.load_ident(n, env)?;
        self.unary_val(UnaryOp::Typeof, v)
    }

    /// Read a name (`x`) for the bytecode VM.
    pub fn vm_load(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {
        self.load_ident(n, env)
    }

    /// `LoadVar` using the scope depth cached from the last run; see
    /// `Chunk::hints`.
    ///
    /// On a hit the access is one table lookup. On a miss (or no hint) the
    /// full path runs and updates the hint. A hint can only become wrong if
    /// a binding appears further in after the fact, which only direct
    /// `eval` can do; it therefore disables hints.
    pub fn vm_load_at(&mut self, n: &str, env: &Rc<RefCell<Env>>,
                      hint: &core::cell::Cell<u16>) -> C<Value> {
        let h = hint.get();
        if self.hints_ok && h != super::code::HINT_NONE {
            // Walk raw pointers instead of cloning `Rc`s per hop.
            //
            // SAFETY: the chain hangs off `env`, which lives for this call;
            // each environment holds its parent as `Rc`, so the whole chain
            // lives too. Access is read-only, and no foreign code runs inside
            // this loop that could rewire it.
            let mut cur: *const RefCell<Env> = Rc::as_ptr(env);
            let mut reached = true;
            for _ in 0..h {
                let next = unsafe { (*cur).borrow().parent.as_ref().map(Rc::as_ptr) };
                match next { Some(p) => cur = p, None => { reached = false; break } }
            }
            // Only a live hit counts. "Present but uninitialized" and imports
            // take the full path, so error messages and import handling stay
            // the same.
            if reached {
                // SAFETY: `cur` comes from the same chain; see above.
                let b = unsafe { (*cur).borrow() };
                if let Some(bd) = b.vars.get(n) {
                    if bd.initialized { return Ok(bd.value.clone()) }
                }
            }
        }
        let (v, d) = self.load_ident_depth(n, env)?;
        match d {
            Some(d) if d < super::code::HINT_NONE as usize => hint.set(d as u16),
            _ => hint.set(super::code::HINT_NONE),
        }
        Ok(v)
    }

    /// Assign a name (`x = v`) for the bytecode VM, directly rather than via a
    /// constructed AST node.
    pub fn vm_store(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        self.assign_ident(n, v, env)
    }

    /// `StoreVar` using the cached scope depth; same reasoning and same
    /// caveat as `vm_load_at` (direct `eval` disables it).
    pub fn vm_store_at(&mut self, n: &str, v: Value, env: &Rc<RefCell<Env>>,
                       hint: &core::cell::Cell<u16>) -> C<()> {
        let h = hint.get();
        if self.hints_ok && h != super::code::HINT_NONE {
            // SAFETY: as in `vm_load_at`: the chain hangs off `env`, is only
            // walked, and no foreign code runs in between.
            let mut cur: *const RefCell<Env> = Rc::as_ptr(env);
            let mut reached = true;
            for _ in 0..h {
                let next = unsafe { (*cur).borrow().parent.as_ref().map(Rc::as_ptr) };
                match next { Some(p) => cur = p, None => { reached = false; break } }
            }
            if reached {
                // SAFETY: `cur` comes from the same chain. `borrow_mut` is
                // fine because no other borrow is open on this path: the loop
                // dropped its borrows and the caller holds only the `Rc`.
                let mut b = unsafe { (*cur).borrow_mut() };
                // Fast path only when there is no import table at all;
                // otherwise we would have to decide whether the name is
                // imported, and assigning to an import is an error.
                if b.imports.is_none() {
                    if let Some(bd) = b.vars.get_mut(n) {
                        if bd.initialized && bd.mutable {
                            bd.value = v;
                            return Ok(());
                        }
                    }
                }
            }
        }
        match self.assign_ident_depth(n, v, env)? {
            Some(d) if d < super::code::HINT_NONE as usize => hint.set(d as u16),
            _ => hint.set(super::code::HINT_NONE),
        }
        Ok(())
    }

    fn eval_update(&mut self, op: UpdateOp, arg: &Expr, prefix: bool,
                   env: &Rc<RefCell<Env>>) -> C<Value> {
        let old = self.eval(arg, env)?;
        let n = self.to_numeric(&old)?;
        let new = self.step_numeric(&n, op == UpdateOp::Inc)?;
        self.store(arg, new.clone(), env)?;
        Ok(if prefix { new } else { n })
    }

    fn store(&mut self, target: &Expr, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        match target {
            // Single implementation, in `eval.rs`.
            Expr::Ident(n) => self.assign_ident(n, v, env),
            Expr::Member { obj, prop, .. } => {
                let base = self.eval(obj, env)?;
                let key = self.member_key2(prop, env)?;
                let throw = super::interp::env_strict(env);
                self.set(&base, &key, v, throw)
            }
            _ => self.ref_err("invalid assignment target"),
        }
    }

    fn eval_assign(&mut self, op: AssignOp, left: &Pat, right: &Expr,
                   env: &Rc<RefCell<Env>>) -> C<Value> {
        if op == AssignOp::Assign {
            let v = self.eval(right, env)?;
            self.bind_pattern(left, v.clone(), env, false)?;
            return Ok(v);
        }
        let Pat::Expr(target) = left else {
            return self.ref_err("invalid compound assignment target");
        };
        // The short-circuit forms evaluate the right side only when needed:
        // `a ||= b` must not touch `b` if `a` is truthy.
        if matches!(op, AssignOp::And | AssignOp::Or | AssignOp::Nullish) {
            let cur = self.eval(target, env)?;
            let need = match op {
                AssignOp::And => cur.truthy(),
                AssignOp::Or => !cur.truthy(),
                _ => matches!(cur, Value::Undefined | Value::Null),
            };
            if !need { return Ok(cur); }
            let v = self.eval(right, env)?;
            self.store(target, v.clone(), env)?;
            return Ok(v);
        }
        let cur = self.eval(target, env)?;
        let r = self.eval(right, env)?;
        let bop = match op {
            AssignOp::Add => BinOp::Add, AssignOp::Sub => BinOp::Sub,
            AssignOp::Mul => BinOp::Mul, AssignOp::Div => BinOp::Div,
            AssignOp::Mod => BinOp::Mod, AssignOp::Exp => BinOp::Exp,
            AssignOp::Shl => BinOp::Shl, AssignOp::Shr => BinOp::Shr,
            AssignOp::UShr => BinOp::UShr, AssignOp::BitAnd => BinOp::BitAnd,
            AssignOp::BitOr => BinOp::BitOr, AssignOp::BitXor => BinOp::BitXor,
            _ => unreachable!(),
        };
        let v = self.binary(bop, cur, r)?;
        self.store(target, v.clone(), env)?;
        Ok(v)
    }

    /// The tail of every numeric arm: operator fixed, both sides converted.
    fn num_done(&mut self, op: BinOp, a: f64, b: f64) -> C<Value> {
        match num_bin(op, a, b) {
            Some(v) => Ok(v),
            // `in`/`instanceof` have their own arms and never get here.
            None => self.type_err("not a numeric operator"),
        }
    }

    pub fn binary(&mut self, op: BinOp, l: Value, r: Value) -> C<Value> {
        use BinOp::*;
        // Fast path: both sides are already numbers. Every conversion is then
        // the identity and unobservable (no `valueOf`, no
        // `Symbol.toPrimitive`, no throw, no ordering), so skip the result
        // plumbing of the general path.
        if let (Value::Num(a), Value::Num(b)) = (&l, &r) {
            if let Some(v) = num_bin(op, *a, *b) { return Ok(v); }
        }
        Ok(match op {
            Add => {
                // `+` is the only operator that also means concatenation.
                // Both sides are converted to primitives first, then the
                // decision is made; the order is observable via `valueOf`.
                let lp = self.to_primitive_hint(&l, "default")?;
                let rp = self.to_primitive_hint(&r, "default")?;
                if matches!(lp, Value::Str(_)) || matches!(rp, Value::Str(_)) {
                    let a = self.to_string(&lp)?;
                    let b = self.to_string(&rp)?;
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(&a); s.push_str(&b);
                    Value::string(s)
                } else if let (Value::BigInt(a), Value::BigInt(b)) = (&lp, &rp) {
                    Value::BigInt(Rc::new(a.add(b)))
                } else if matches!(lp, Value::BigInt(_)) || matches!(rp, Value::BigInt(_)) {
                    return self.type_err("cannot mix BigInt and other types, use explicit conversions");
                } else {
                    let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                    return self.num_done(Add, a, b);
                }
            }
            Sub | Mul | Div | Mod | Exp => {
                // `ToNumeric(lhs)` completely, then `ToNumeric(rhs)`. The
                // order is observable: if `lhs.valueOf` returns a symbol,
                // `rhs.valueOf` must not run. Two BigInts compute as BigInt,
                // two Numbers as Number, mixed throws.
                let lp = self.to_numeric(&l)?;
                let rp = self.to_numeric(&r)?;
                if let (Value::BigInt(a), Value::BigInt(b)) = (&lp, &rp) {
                    return self.big_arith(op, a, b);
                }
                if matches!(lp, Value::BigInt(_)) || matches!(rp, Value::BigInt(_)) {
                    return self.type_err("cannot mix BigInt and other types, use explicit conversions");
                }
                let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                return self.num_done(op, a, b);
            }
            EqEqEq => Value::Bool(l.strict_eq(&r)),
            NotEqEq => Value::Bool(!l.strict_eq(&r)),
            EqEq => Value::Bool(self.loose_eq(&l, &r)?),
            NotEq => Value::Bool(!self.loose_eq(&l, &r)?),
            Lt | Gt | LtEq | GtEq => {
                let lp = self.to_primitive(&l, false)?;
                let rp = self.to_primitive(&r, false)?;
                if let (Value::Str(a), Value::Str(b)) = (&lp, &rp) {
                    Value::Bool(match op { Lt => a < b, Gt => a > b, LtEq => a <= b, _ => a >= b })
                } else if matches!(lp, Value::BigInt(_)) || matches!(rp, Value::BigInt(_)) {
                    // BigInt and Number may be compared, just not mixed in
                    // arithmetic.
                    match self.big_cmp(&lp, &rp)? {
                        None => Value::Bool(false),
                        Some(o) => Value::Bool(match op {
                            Lt => o.is_lt(), Gt => o.is_gt(), LtEq => o.is_le(), _ => o.is_ge() }),
                    }
                } else {
                    let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                    return self.num_done(op, a, b);
                }
            }
            Shl | Shr | UShr => {
                let lp = self.to_numeric(&l)?;
                let rp = self.to_numeric(&r)?;
                if let (Value::BigInt(a), Value::BigInt(b)) = (&lp, &rp) {
                    // `>>>` does not exist for BigInts: it presumes a fixed
                    // width.
                    if matches!(op, UShr) {
                        return self.type_err("BigInts have no unsigned right shift");
                    }
                    let n = b.to_f64();
                    if !n.is_finite() || libm::fabs(n) > 1e6 {
                        return self.range_err("BigInt shift count is too large");
                    }
                    let left = matches!(op, Shl) == (n >= 0.0);
                    let k = crate::js::value::f64_to_usize(libm::fabs(n)) as u64;
                    return Ok(Value::BigInt(Rc::new(if left { a.shl(k) } else { a.shr(k) })));
                }
                if matches!(lp, Value::BigInt(_)) || matches!(rp, Value::BigInt(_)) {
                    return self.type_err("cannot mix BigInt and other types, use explicit conversions");
                }
                let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                return self.num_done(op, a, b);
            }
            BitAnd | BitOr | BitXor => {
                let lp = self.to_numeric(&l)?;
                let rp = self.to_numeric(&r)?;
                if let (Value::BigInt(a), Value::BigInt(b)) = (&lp, &rp) {
                    let k = match op { BitAnd => 0u8, BitOr => 1, _ => 2 };
                    return Ok(Value::BigInt(Rc::new(a.bitop(b, k))));
                }
                if matches!(lp, Value::BigInt(_)) || matches!(rp, Value::BigInt(_)) {
                    return self.type_err("cannot mix BigInt and other types, use explicit conversions");
                }
                let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                return self.num_done(op, a, b);
            }
            In => {
                let Value::Obj(o) = &r else { return self.type_err("'in' needs an object on the right") };
                let k = self.to_prop_key(&l)?;
                let o = o.clone();
                Value::Bool(self.has_prop(&o, &k)?)
            }
            Instanceof => {
                let Value::Obj(_) = &r else { return self.type_err("right side of 'instanceof' is not callable") };
                // `Symbol.hasInstance` overrides the prototype chain walk.
                let hi = self.get(&r, SYM_HAS_INSTANCE)?;
                if self.is_callable(&hi) {
                    let res = self.call(&hi, r.clone(), &[l.clone()])?;
                    return Ok(Value::Bool(res.truthy()));
                }
                if !self.is_callable(&r) { return self.type_err("right side of 'instanceof' is not callable"); }
                let proto = self.get(&r, "prototype")?;
                let Value::Obj(p) = proto else { return self.type_err("prototype is not an object") };
                let mut cur = l.as_obj().and_then(|o| o.borrow().proto.clone());
                let mut found = false;
                while let Some(c) = cur {
                    if Rc::ptr_eq(&c, &p) { found = true; break; }
                    let next = c.borrow().proto.clone();
                    cur = next;
                }
                Value::Bool(found)
            }
        })
    }

    /// `==` (IsLooselyEqual).
    fn loose_eq(&mut self, l: &Value, r: &Value) -> C<bool> {
        use Value::*;
        Ok(match (l, r) {
            (Undefined | Null, Undefined | Null) => true,
            (Undefined | Null, _) | (_, Undefined | Null) => false,
            (Num(_), Num(_)) | (Str(_), Str(_)) | (Bool(_), Bool(_)) | (Obj(_), Obj(_))
            | (Sym(_), Sym(_)) | (BigInt(_), BigInt(_)) => l.strict_eq(r),
            // `1n == 1` is true, `1n === 1` false. Compared by mathematical
            // value, not by conversion.
            (BigInt(a), Num(n)) | (Num(n), BigInt(a)) =>
                n.is_finite() && libm::trunc(*n) == *n
                    && crate::js::bigint::Big::from_f64(*n).map(|b| b == **a).unwrap_or(false),
            (BigInt(a), Str(t)) | (Str(t), BigInt(a)) =>
                crate::js::bigint::Big::parse(t).map(|b| b == **a).unwrap_or(false),
            (BigInt(_), Sym(_)) | (Sym(_), BigInt(_)) => false,
            // A symbol only equals itself; `==` does not convert it. Against
            // an object, the object's `ToPrimitive` decides.
            (Sym(_), Num(_) | Str(_) | Bool(_)) | (Num(_) | Str(_) | Bool(_), Sym(_)) => false,
            (Bool(b), _) => { let n = Num(if *b { 1.0 } else { 0.0 }); self.loose_eq(&n, r)? }
            (_, Bool(b)) => { let n = Num(if *b { 1.0 } else { 0.0 }); self.loose_eq(l, &n)? }
            (Num(a), Str(_)) => *a == self.to_number(r)?,
            (Str(_), Num(b)) => self.to_number(l)? == *b,
            (Obj(_), _) => { let p = self.to_primitive_hint(l, "default")?; self.loose_eq(&p, r)? }
            (_, Obj(_)) => { let p = self.to_primitive_hint(r, "default")?; self.loose_eq(l, &p)? }
        })
    }
}

impl Interp {
    /// The arithmetic operators on two BigInts.
    fn big_arith(&mut self, op: BinOp, a: &Rc<crate::js::bigint::Big>,
                 b: &Rc<crate::js::bigint::Big>) -> C<Value> {
        use BinOp::*;
        let r = match op {
            Sub => a.sub(b),
            Mul => a.mul(b),
            Div | Mod => {
                let Some((q, m)) = a.div_rem(b) else {
                    return Err(self.throw_kind("RangeError", "division by zero"));
                };
                if matches!(op, Div) { q } else { m }
            }
            _ => match a.pow(b) {
                Some(x) => x,
                None => return Err(self.throw_kind("RangeError",
                    "BigInt exponent is negative or the result is too large")),
            },
        };
        Ok(Value::BigInt(Rc::new(r)))
    }

    /// A comparison where at least one side is a BigInt. `None` means
    /// "unordered" (NaN on the other side).
    fn big_cmp(&mut self, l: &Value, r: &Value) -> C<Option<core::cmp::Ordering>> {
        use crate::js::bigint::Big;
        let to_big = |v: &Value| -> Option<Big> {
            match v {
                Value::BigInt(b) => Some((**b).clone()),
                Value::Str(s) => Big::parse(s),
                _ => None,
            }
        };
        // Two BigInts, or a BigInt against a string: compare exactly.
        if let (Some(a), Some(b)) = (to_big(l), to_big(r)) { return Ok(Some(a.cmp(&b))); }
        // BigInt against Number: via `f64`. Imprecise for very large values.
        let (a, b) = match (l, r) {
            (Value::BigInt(x), other) => (x.to_f64(), self.to_number(other)?),
            (other, Value::BigInt(y)) => (self.to_number(other)?, y.to_f64()),
            _ => return Ok(None),
        };
        if a.is_nan() || b.is_nan() { return Ok(None); }
        Ok(Some(if a < b { core::cmp::Ordering::Less }
                else if a > b { core::cmp::Ordering::Greater }
                else { core::cmp::Ordering::Equal }))
    }
}

/// The numeric core of the binary operators: both sides are plain numbers.
///
/// Called from both the fast path in `binary` and the tail of every slow
/// arm, so the semantics live in one place (`%` and `a ** b` have special
/// cases, and any comparison with NaN is false, including `>=`).
///
/// `None` only for `in` and `instanceof`, which have their own arms.
fn num_bin(op: BinOp, a: f64, b: f64) -> Option<Value> {
    use BinOp::*;
    Some(match op {
        Add => Value::Num(a + b),
        Sub => Value::Num(a - b),
        Mul => Value::Num(a * b),
        Div => Value::Num(a / b),
        Mod => Value::Num(if b == 0.0 || a.is_nan() || b.is_nan() || a.is_infinite() { f64::NAN }
                          else if b.is_infinite() { a } else { a % b }),
        Exp => Value::Num(powf(a, b)),
        Shl => Value::Num(((to_int32(a)) << (to_uint32(b) & 31)) as f64),
        Shr => Value::Num(((to_int32(a)) >> (to_uint32(b) & 31)) as f64),
        UShr => Value::Num(((to_uint32(a)) >> (to_uint32(b) & 31)) as f64),
        BitAnd => Value::Num((to_int32(a) & to_int32(b)) as f64),
        BitOr => Value::Num((to_int32(a) | to_int32(b)) as f64),
        BitXor => Value::Num((to_int32(a) ^ to_int32(b)) as f64),
        Lt | Gt | LtEq | GtEq => {
            if a.is_nan() || b.is_nan() { Value::Bool(false) }
            else { Value::Bool(match op { Lt => a < b, Gt => a > b, LtEq => a <= b, _ => a >= b }) }
        }
        // Two numbers: `==` is `===`.
        EqEq | EqEqEq => Value::Bool(a == b),
        NotEq | NotEqEq => Value::Bool(a != b),
        In | Instanceof => return None,
    })
}

/// Exponentiation, `a ** b`.
fn powf(a: f64, b: f64) -> f64 {
    if b == 0.0 { return 1.0; }
    if a.is_nan() || b.is_nan() { return f64::NAN; }
    libm::pow(a, b)
}
