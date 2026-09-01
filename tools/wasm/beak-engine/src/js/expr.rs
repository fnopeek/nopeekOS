//! Ausdruecke auswerten.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::vec;
use core::cell::RefCell;

use super::ast::*;
use super::interp::*;
use super::value::*;

impl Interp {
    pub fn eval(&mut self, e: &Expr, env: &Rc<RefCell<Env>>) -> C<Value> {
        match e {
            Expr::Num(n) => Ok(Value::Num(*n)),
            Expr::Str(s) => Ok(Value::str(s)),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Null => Ok(Value::Null),
            Expr::This => Ok(env_this(env)),
            Expr::BigInt(_) => self.type_err("BigInt is not supported"),
            Expr::Ident(n) => self.load_ident(n, env),
            Expr::Regex { .. } => {
                // Kein RegExp-Motor. Ein Objekt zurueckzugeben, das nicht
                // funktioniert, waere schlimmer als ein klarer Fehler: der
                // Lauf zaehlt ihn, ein stiller Platzhalter waere unsichtbar.
                self.type_err("regular expressions are not supported")
            }
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
            Expr::Func(f) => {
                // Eine benannte Funktions-EXPRESSION sieht ihren eigenen Namen
                // in ihrem Rumpf — `(function e(){ … e … })`. Das ist der Weg,
                // auf dem minifizierter Code rekursiert, und ohne ihn stirbt er
                // an einem einbuchstabigen `ReferenceError`. Eine DEKLARATION
                // bekommt diese Bindung NICHT: dort steht der Name schon
                // aussen, und eine innere wuerde eine Neuzuweisung verdecken.
                if !f.is_arrow {
                    if let Some(n) = &f.name {
                        let inner = Env::new(Some(env.clone()), false);
                        let cl = self.make_closure(f.clone(), &inner, None);
                        inner.borrow_mut().vars.insert(Rc::from(n.as_str()),
                            Binding { value: cl.clone(), mutable: false, initialized: true });
                        return Ok(cl);
                    }
                }
                Ok(self.make_closure(f.clone(), env,
                    if f.is_arrow { Some(env_this(env)) } else { None }))
            }
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
            Expr::TaggedTemplate { .. } => self.type_err("tagged templates are not supported"),
            Expr::Seq(list) => {
                let mut last = Value::Undefined;
                for x in list { last = self.eval(x, env)?; }
                Ok(last)
            }
            Expr::Unary { op, arg } => self.eval_unary(*op, arg, env),
            Expr::Update { op, arg, prefix } => self.eval_update(*op, arg, *prefix, env),
            Expr::Binary { op, left, right } => {
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
                self.construct(&f, &a)
            }
            Expr::Spread(inner) => self.eval(inner, env),
            Expr::Super => Ok(Value::Undefined),
            Expr::MetaProp { .. } => Ok(Value::Undefined),
            Expr::ImportCall(_) => self.type_err("dynamic import is not supported"),
            Expr::Yield { .. } => self.type_err("generators are not supported"),
            Expr::Await(_) => self.type_err("await is not supported"),
        }
    }

    fn load_ident(&mut self, n: &str, env: &Rc<RefCell<Env>>) -> C<Value> {
        if let Some(e) = env_lookup(env, n) {
            let b = e.borrow();
            let bd = b.vars.get(n).unwrap();
            if !bd.initialized {
                drop(b);
                return self.ref_err(&alloc::format!("cannot access '{n}' before initialization"));
            }
            return Ok(bd.value.clone());
        }
        // Nicht in der Kette: das globale Objekt fragen, sonst ReferenceError.
        // Der Unterschied zu `undefined` ist der ganze Sinn der Sache.
        let g = self.realm.global.clone();
        if self.has_property(&g, n) { return self.get(&Value::Obj(g), n); }
        self.ref_err(&alloc::format!("{n} is not defined"))
    }

    fn member_key2(&mut self, p: &MemberProp, env: &Rc<RefCell<Env>>) -> C<Rc<str>> {
        Ok(match p {
            MemberProp::Ident(n) => Rc::from(n.as_str()),
            MemberProp::Private(n) => Rc::from(alloc::format!("#{n}").as_str()),
            MemberProp::Computed(e) => { let v = self.eval(e, env)?; self.to_string(&v)? }
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
        // `a.b()` bindet `this` an `a` — deshalb wird der Empfaenger hier
        // getrennt geholt und nicht ueber `eval(callee)`, das ihn verlieren
        // wuerde.
        let (this_val, f) = match callee {
            Expr::Member { obj, prop, optional: mopt } => {
                let base = self.eval(obj, env)?;
                if *mopt && matches!(base, Value::Undefined | Value::Null) {
                    return Ok(Value::Undefined);
                }
                let key = self.member_key2(prop, env)?;
                let f = self.get(&base, &key)?;
                if !self.is_callable(&f) && !optional {
                    return self.type_err(&alloc::format!("{key} is not a function"));
                }
                (base, f)
            }
            _ => (Value::Undefined, self.eval(callee, env)?),
        };
        if optional && matches!(f, Value::Undefined | Value::Null) { return Ok(Value::Undefined); }
        let a = self.eval_args(args, env)?;
        if !self.is_callable(&f) { return self.type_err("value is not a function"); }
        self.call(&f, this_val, &a)
    }

    pub fn construct(&mut self, f: &Value, args: &[Value]) -> C<Value> {
        let Value::Obj(fo) = f else { return self.type_err("value is not a constructor") };
        // Ein nativer Konstruktor baut sein Objekt selbst; ein Pfeil ist keiner.
        if let ObjKind::Native(n) = &fo.borrow().kind {
            if !n.ctor { return self.type_err("value is not a constructor"); }
            let nf = n.clone();
            return (nf.func)(self, Value::Undefined, args);
        }
        if let ObjKind::Function(d) = &fo.borrow().kind {
            if d.node.is_arrow { return self.type_err("arrow functions are not constructors"); }
        }
        if !self.is_callable(f) { return self.type_err("value is not a constructor"); }
        let proto = match self.get(f, "prototype")? {
            Value::Obj(p) => Some(p),
            _ => Some(self.realm.object_proto.clone()),
        };
        let obj = new_obj(proto);
        let r = self.call(f, Value::Obj(obj.clone()), args)?;
        // Gibt der Konstruktor ein OBJEKT zurueck, gewinnt es; alles andere
        // wird verworfen und das frische Objekt gewinnt.
        Ok(match r { Value::Obj(_) => r, _ => Value::Obj(obj) })
    }

    fn eval_object(&mut self, props: &[ObjProp], env: &Rc<RefCell<Env>>) -> C<Value> {
        let g = new_obj(Some(self.realm.object_proto.clone()));
        for p in props {
            match &p.value {
                ObjPropValue::Spread(e) => {
                    let v = self.eval(e, env)?;
                    if let Value::Obj(src) = &v {
                        for k in src.borrow().own_keys() {
                            let enumerable = src.borrow().get_own(&k).map(|x| x.enumerable).unwrap_or(false);
                            if !enumerable { continue; }
                            let val = self.get(&v, &k)?;
                            g.borrow_mut().set_prop(k, Prop::data(val));
                        }
                    }
                }
                ObjPropValue::Init(e) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.eval(e, env)?;
                    g.borrow_mut().set_prop(key, Prop::data(v));
                }
                ObjPropValue::Method(f) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.make_closure(f.clone(), env, None);
                    g.borrow_mut().set_prop(key, Prop::data(v));
                }
                ObjPropValue::Get(f) | ObjPropValue::Set(f) => {
                    let key = self.prop_key(&p.key, env)?;
                    let v = self.make_closure(f.clone(), env, None);
                    let is_get = matches!(p.value, ObjPropValue::Get(_));
                    let mut existing = g.borrow().get_own(&key).cloned().unwrap_or(Prop {
                        value: None, get: None, set: None,
                        writable: false, enumerable: true, configurable: true });
                    existing.value = None;
                    if is_get { existing.get = Some(v); } else { existing.set = Some(v); }
                    g.borrow_mut().set_prop(key, existing);
                }
            }
        }
        Ok(Value::Obj(g))
    }

    pub fn eval_class(&mut self, c: &Class, env: &Rc<RefCell<Env>>) -> C<Value> {
        let parent_proto = match &c.super_class {
            Some(e) => {
                let sv = self.eval(e, env)?;
                match self.get(&sv, "prototype")? {
                    Value::Obj(p) => Some(p),
                    _ => Some(self.realm.object_proto.clone()),
                }
            }
            None => Some(self.realm.object_proto.clone()),
        };
        let proto = new_obj(parent_proto);

        // Der Konstruktor IST die Klasse. Fehlt er, wird ein leerer erzeugt —
        // sonst haette die Klasse keinen aufrufbaren Koerper.
        let ctor_node = c.body.iter().find_map(|m| match m {
            ClassMember::Method { func, kind: MethodKind::Constructor, .. } => Some(func.clone()),
            _ => None,
        });
        let ctor_fn = match ctor_node {
            Some(f) => f,
            None => Rc::new(Func {
                name: c.name.clone(), params: Vec::new(), body: Vec::new(),
                is_async: false, is_generator: false, is_arrow: false, expr_body: false,
            }),
        };
        let ctor = self.make_closure(ctor_fn, env, None);
        if let Value::Obj(co) = &ctor {
            co.borrow_mut().define("prototype", Prop {
                value: Some(Value::Obj(proto.clone())), get: None, set: None,
                writable: false, enumerable: false, configurable: false });
            co.borrow_mut().define("name", Prop {
                value: Some(Value::str(c.name.as_deref().unwrap_or(""))), get: None, set: None,
                writable: false, enumerable: false, configurable: true });
        }
        proto.borrow_mut().define("constructor", Prop::builtin(ctor.clone()));

        for m in &c.body {
            match m {
                ClassMember::Method { key, func, kind, is_static, .. } => {
                    if *kind == MethodKind::Constructor { continue; }
                    let target = if *is_static { ctor.as_obj().unwrap().clone() } else { proto.clone() };
                    let k = self.prop_key(key, env)?;
                    let v = self.make_closure(func.clone(), env, None);
                    match kind {
                        MethodKind::Get | MethodKind::Set => {
                            let mut p = target.borrow().get_own(&k).cloned().unwrap_or(Prop {
                                value: None, get: None, set: None,
                                writable: false, enumerable: false, configurable: true });
                            p.value = None;
                            if *kind == MethodKind::Get { p.get = Some(v); } else { p.set = Some(v); }
                            target.borrow_mut().set_prop(k, p);
                        }
                        // Methoden einer Klasse sind NICHT aufzaehlbar — anders
                        // als die eines Objektliterals. Ein `for..in` ueber eine
                        // Instanz darf sie nicht sehen.
                        _ => { target.borrow_mut().set_prop(k, Prop::builtin(v)); }
                    }
                }
                ClassMember::Field { key, value, is_static, .. } if *is_static => {
                    let k = self.prop_key(key, env)?;
                    let v = match value { Some(e) => self.eval(e, env)?, None => Value::Undefined };
                    ctor.as_obj().unwrap().borrow_mut().set_prop(k, Prop::data(v));
                }
                // Felder je Instanz brauchen einen Haken im Konstruktor; der
                // fehlt noch und faellt im Lauf auf.
                _ => {}
            }
        }
        Ok(ctor)
    }

    fn eval_unary(&mut self, op: UnaryOp, arg: &Expr, env: &Rc<RefCell<Env>>) -> C<Value> {
        // `typeof x` auf einen UNBEKANNTEN Namen wirft nicht — das ist der
        // klassische Weg, ein globales Objekt zu pruefen, und der Vorspann
        // benutzt ihn (`typeof JSON !== "undefined"`).
        if op == UnaryOp::Typeof {
            if let Expr::Ident(n) = arg {
                if env_lookup(env, n).is_none() {
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
                    match base {
                        Value::Obj(o) => {
                            let cfg = o.borrow().get_own(&key).map(|p| p.configurable);
                            match cfg {
                                Some(false) => Value::Bool(false),
                                Some(true) => { o.borrow_mut().remove(&key); Value::Bool(true) }
                                None => Value::Bool(true),
                            }
                        }
                        _ => Value::Bool(true),
                    }
                }
                _ => Value::Bool(true),
            });
        }
        let v = self.eval(arg, env)?;
        Ok(match op {
            UnaryOp::Minus => Value::Num(-self.to_number(&v)?),
            UnaryOp::Plus => Value::Num(self.to_number(&v)?),
            UnaryOp::Bang => Value::Bool(!v.truthy()),
            UnaryOp::Tilde => Value::Num(!to_int32(self.to_number(&v)?) as f64),
            UnaryOp::Typeof => Value::str(v.type_of()),
            UnaryOp::Void => Value::Undefined,
            UnaryOp::Delete => unreachable!(),
        })
    }

    fn eval_update(&mut self, op: UpdateOp, arg: &Expr, prefix: bool,
                   env: &Rc<RefCell<Env>>) -> C<Value> {
        let old = self.eval(arg, env)?;
        let n = self.to_number(&old)?;
        let new = if op == UpdateOp::Inc { n + 1.0 } else { n - 1.0 };
        self.store(arg, Value::Num(new), env)?;
        Ok(Value::Num(if prefix { new } else { n }))
    }

    fn store(&mut self, target: &Expr, v: Value, env: &Rc<RefCell<Env>>) -> C<()> {
        match target {
            Expr::Ident(n) => {
                if let Some(e) = env_lookup(env, n.as_str()) {
                    let (mutable, init) = {
                        let b = e.borrow();
                        let bd = b.vars.get(n.as_str()).unwrap();
                        (bd.mutable, bd.initialized)
                    };
                    if !init { return self.ref_err(&alloc::format!("cannot access '{n}' before initialization")); }
                    if !mutable { return self.type_err("assignment to constant variable"); }
                    e.borrow_mut().vars.get_mut(n.as_str()).unwrap().value = v;
                    return Ok(());
                }
                let g = Value::Obj(self.realm.global.clone());
                self.set(&g, n, v)
            }
            Expr::Member { obj, prop, .. } => {
                let base = self.eval(obj, env)?;
                let key = self.member_key2(prop, env)?;
                self.set(&base, &key, v)
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
        // Die kurzschliessenden Formen werten die Rechte NUR aus, wenn sie
        // gebraucht wird — `a ||= b` darf `b` nicht anfassen, wenn `a` wahr ist.
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

    pub fn binary(&mut self, op: BinOp, l: Value, r: Value) -> C<Value> {
        use BinOp::*;
        Ok(match op {
            Add => {
                // `+` ist der einzige Operator, der auch Text meint. Beide
                // Seiten werden ZUERST primitiv gemacht, DANN entschieden —
                // die Reihenfolge ist sichtbar, wenn `valueOf` Nebenwirkungen
                // hat.
                let lp = self.to_primitive(&l, false)?;
                let rp = self.to_primitive(&r, false)?;
                if matches!(lp, Value::Str(_)) || matches!(rp, Value::Str(_)) {
                    let a = self.to_string(&lp)?;
                    let b = self.to_string(&rp)?;
                    let mut s = String::with_capacity(a.len() + b.len());
                    s.push_str(&a); s.push_str(&b);
                    Value::string(s)
                } else {
                    Value::Num(self.to_number(&lp)? + self.to_number(&rp)?)
                }
            }
            Sub => Value::Num(self.to_number(&l)? - self.to_number(&r)?),
            Mul => Value::Num(self.to_number(&l)? * self.to_number(&r)?),
            Div => Value::Num(self.to_number(&l)? / self.to_number(&r)?),
            Mod => {
                let a = self.to_number(&l)?; let b = self.to_number(&r)?;
                Value::Num(if b == 0.0 || a.is_nan() || b.is_nan() || a.is_infinite() { f64::NAN }
                           else if b.is_infinite() { a } else { a % b })
            }
            Exp => Value::Num(powf(self.to_number(&l)?, self.to_number(&r)?)),
            EqEqEq => Value::Bool(l.strict_eq(&r)),
            NotEqEq => Value::Bool(!l.strict_eq(&r)),
            EqEq => Value::Bool(self.loose_eq(&l, &r)?),
            NotEq => Value::Bool(!self.loose_eq(&l, &r)?),
            Lt | Gt | LtEq | GtEq => {
                let lp = self.to_primitive(&l, false)?;
                let rp = self.to_primitive(&r, false)?;
                if let (Value::Str(a), Value::Str(b)) = (&lp, &rp) {
                    Value::Bool(match op { Lt => a < b, Gt => a > b, LtEq => a <= b, _ => a >= b })
                } else {
                    let a = self.to_number(&lp)?; let b = self.to_number(&rp)?;
                    if a.is_nan() || b.is_nan() { Value::Bool(false) }
                    else { Value::Bool(match op { Lt => a < b, Gt => a > b, LtEq => a <= b, _ => a >= b }) }
                }
            }
            Shl => Value::Num(((to_int32(self.to_number(&l)?)) << (to_uint32(self.to_number(&r)?) & 31)) as f64),
            Shr => Value::Num(((to_int32(self.to_number(&l)?)) >> (to_uint32(self.to_number(&r)?) & 31)) as f64),
            UShr => Value::Num(((to_uint32(self.to_number(&l)?)) >> (to_uint32(self.to_number(&r)?) & 31)) as f64),
            BitAnd => Value::Num((to_int32(self.to_number(&l)?) & to_int32(self.to_number(&r)?)) as f64),
            BitOr => Value::Num((to_int32(self.to_number(&l)?) | to_int32(self.to_number(&r)?)) as f64),
            BitXor => Value::Num((to_int32(self.to_number(&l)?) ^ to_int32(self.to_number(&r)?)) as f64),
            In => {
                let Value::Obj(o) = &r else { return self.type_err("'in' needs an object on the right") };
                let k = self.to_string(&l)?;
                Value::Bool(self.has_property(o, &k))
            }
            Instanceof => {
                let Value::Obj(_) = &r else { return self.type_err("right side of 'instanceof' is not callable") };
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

    /// `==`. Die Regel, die niemand mag, aber echter Code benutzt sie.
    fn loose_eq(&mut self, l: &Value, r: &Value) -> C<bool> {
        use Value::*;
        Ok(match (l, r) {
            (Undefined | Null, Undefined | Null) => true,
            (Undefined | Null, _) | (_, Undefined | Null) => false,
            (Num(_), Num(_)) | (Str(_), Str(_)) | (Bool(_), Bool(_)) | (Obj(_), Obj(_)) =>
                l.strict_eq(r),
            (Bool(b), _) => { let n = Num(if *b { 1.0 } else { 0.0 }); self.loose_eq(&n, r)? }
            (_, Bool(b)) => { let n = Num(if *b { 1.0 } else { 0.0 }); self.loose_eq(l, &n)? }
            (Num(a), Str(_)) => *a == self.to_number(r)?,
            (Str(_), Num(b)) => self.to_number(l)? == *b,
            (Obj(_), _) => { let p = self.to_primitive(l, false)?; self.loose_eq(&p, r)? }
            (_, Obj(_)) => { let p = self.to_primitive(r, false)?; self.loose_eq(l, &p)? }
        })
    }
}

/// `**`. `libm` ist schon Abhaengigkeit der Engine (CSS Color 4), also wird
/// hier nichts Neues hereingezogen.
fn powf(a: f64, b: f64) -> f64 {
    if b == 0.0 { return 1.0; }
    if a.is_nan() || b.is_nan() { return f64::NAN; }
    libm::pow(a, b)
}
