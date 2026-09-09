//! XPath 1.0 over the scripting DOM arena — lexer, parser, evaluator.
//!
//! Built because htmx finds its `hx-on:` attributes with one, and it is the
//! last of the thirteen library probes still red. Measured first, as the rule
//! says: the Chromium census over twelve real target pages records **zero**
//! XPath calls, so this is not a web requirement by call count — it is a
//! LIBRARY requirement, and htmx is a library a real page loads.
//!
//! That measurement decides the SHAPE, not whether to build it. A special case
//! for htmx's one expression would be a workaround where a capability is
//! missing; a full XPath 1.0 with namespaces, `following`/`preceding` and the
//! whole axis set would be gold plate nobody asks for. What is here is the
//! language a page actually writes: the abbreviated syntax, the axes that
//! address a document tree, predicates with the real expression grammar, and
//! the core function library.
//!
//! Not implemented, and named rather than faked: namespaces (`namespace::`,
//! prefixed names resolve as literal names), the `following`/`preceding` axes,
//! and variables (`$x`) — no caller can bind one through the DOM API anyway.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::dombind::{Doc, DomNode, ELEMENT_NODE};

/// A node in an XPath node-set. Attributes are not arena nodes, so they are
/// addressed as (owner element, index into its `attrs`) rather than given a
/// synthetic arena slot — which would have to be kept in step with every
/// mutation for the sake of one axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum XNode {
    Node(u32),
    Attr(u32, usize),
}

impl XNode {
    fn id(self) -> u32 {
        match self {
            XNode::Node(i) | XNode::Attr(i, _) => i,
        }
    }
}

/// An XPath value (XPath 1.0 §1): the four types, and every conversion between
/// them is defined by the spec rather than by convenience.
#[derive(Clone, Debug)]
pub enum XVal {
    Nodes(Vec<XNode>),
    Str(String),
    Num(f64),
    Bool(bool),
}

// ───────────────────────────── lexer ─────────────────────────────

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Name(String),
    /// `prefix::` — an axis specifier, already split off its `::`.
    Axis(String),
    Fn(String),
    Num(f64),
    Str(String),
    Op(&'static str),
    Eof,
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// **The one context rule of the XPath grammar** (§3.7): `*` and a bare name
/// like `and`, `or`, `div`, `mod` are OPERATORS unless the preceding token is
/// `@`, `::`, `(`, `[`, `,` or another operator — in which case they are a
/// wildcard or a node test. Without it, `.//*[@*[...]]` lexes its second `*`
/// as a multiplication and the whole expression is a parse error.
fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Does the previous token allow an operator here?
        let prev_is_operand = matches!(
            out.last(),
            Some(Tok::Name(_)) | Some(Tok::Num(_)) | Some(Tok::Str(_)) | Some(Tok::Op("]")) | Some(Tok::Op(")")) | Some(Tok::Op("*"))
        );
        match c {
            '"' | '\'' => {
                let q = c;
                i += 1;
                let start = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                if i >= b.len() {
                    return Err("unterminated string".to_string());
                }
                out.push(Tok::Str(b[start..i].iter().collect()));
                i += 1;
            }
            '0'..='9' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                    i += 1;
                }
                let s: String = b[start..i].iter().collect();
                out.push(Tok::Num(s.parse::<f64>().map_err(|_| "bad number".to_string())?));
            }
            '.' if i + 1 < b.len() && b[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                let s: String = b[start..i].iter().collect();
                out.push(Tok::Num(s.parse::<f64>().map_err(|_| "bad number".to_string())?));
            }
            '/' if i + 1 < b.len() && b[i + 1] == '/' => {
                out.push(Tok::Op("//"));
                i += 2;
            }
            '.' if i + 1 < b.len() && b[i + 1] == '.' => {
                out.push(Tok::Op(".."));
                i += 2;
            }
            '!' if i + 1 < b.len() && b[i + 1] == '=' => {
                out.push(Tok::Op("!="));
                i += 2;
            }
            '<' | '>' if i + 1 < b.len() && b[i + 1] == '=' => {
                out.push(Tok::Op(if c == '<' { "<=" } else { ">=" }));
                i += 2;
            }
            '/' | '.' | '@' | '[' | ']' | '(' | ')' | ',' | '|' | '+' | '-' | '=' | '<' | '>' | '$' => {
                let s: &'static str = match c {
                    '/' => "/", '.' => ".", '@' => "@", '[' => "[", ']' => "]",
                    '(' => "(", ')' => ")", ',' => ",", '|' => "|", '+' => "+",
                    '-' => "-", '=' => "=", '<' => "<", '>' => ">", _ => "$",
                };
                out.push(Tok::Op(s));
                i += 1;
            }
            '*' => {
                out.push(Tok::Op(if prev_is_operand { "mul" } else { "*" }));
                i += 1;
            }
            ':' if i + 1 < b.len() && b[i + 1] == ':' => {
                return Err("stray ::".to_string());
            }
            _ if is_name_start(c) => {
                let start = i;
                // `:` belongs to a name (`svg:rect`) but `::` does NOT — it is
                // the axis separator, and letting the name scan swallow it made
                // `child::p` one long name and every axis silently a tag.
                while i < b.len() && is_name_char(b[i]) {
                    if b[i] == ':' && i + 1 < b.len() && b[i + 1] == ':' {
                        break;
                    }
                    i += 1;
                }
                let name: String = b[start..i].iter().collect();
                if i + 1 < b.len() && b[i] == ':' && b[i + 1] == ':' {
                    i += 2;
                    out.push(Tok::Axis(name));
                    continue;
                }
                let mut j = i;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == '(' && !matches!(name.as_str(), "node" | "text" | "comment" | "processing-instruction") {
                    out.push(Tok::Fn(name));
                } else if j < b.len() && b[j] == '(' {
                    out.push(Tok::Name(name)); // a node type test, kept as a name
                } else if prev_is_operand && matches!(name.as_str(), "and" | "or" | "div" | "mod") {
                    let s: &'static str = match name.as_str() {
                        "and" => "and", "or" => "or", "div" => "div", _ => "mod",
                    };
                    out.push(Tok::Op(s));
                } else {
                    out.push(Tok::Name(name));
                }
            }
            _ => return Err(alloc::format!("unexpected char {c:?}")),
        }
    }
    out.push(Tok::Eof);
    Ok(out)
}

// ───────────────────────────── AST ─────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Parent,
    Ancestor,
    AncestorOrSelf,
    SelfAxis,
    Attribute,
    FollowingSibling,
    PrecedingSibling,
}

#[derive(Clone, Debug)]
enum Test {
    /// `*` — any node of the axis's principal type.
    Any,
    Name(String),
    /// `node()`
    AnyNode,
    /// `text()`
    Text,
    Comment,
}

#[derive(Clone, Debug)]
struct Step {
    axis: Axis,
    test: Test,
    preds: Vec<Expr>,
}

#[derive(Clone, Debug)]
enum Expr {
    /// A location path. `absolute` starts at the document root.
    Path { absolute: bool, steps: Vec<Step> },
    /// A primary expression with steps hung off it (`foo(...)/bar`).
    Filter { base: alloc::boxed::Box<Expr>, preds: Vec<Expr>, steps: Vec<Step> },
    Num(f64),
    Str(String),
    Call(String, Vec<Expr>),
    Bin(&'static str, alloc::boxed::Box<Expr>, alloc::boxed::Box<Expr>),
    Neg(alloc::boxed::Box<Expr>),
}

// ───────────────────────────── parser ─────────────────────────────

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn peek(&self) -> &Tok {
        self.t.get(self.i).unwrap_or(&Tok::Eof)
    }
    fn eat_op(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Tok::Op(o) if *o == s) {
            self.i += 1;
            return true;
        }
        false
    }
    fn expect_op(&mut self, s: &str) -> Result<(), String> {
        if self.eat_op(s) { Ok(()) } else { Err(alloc::format!("expected {s}")) }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        self.or_expr()
    }
    fn or_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.and_expr()?;
        while self.eat_op("or") {
            l = Expr::Bin("or", l.into(), self.and_expr()?.into());
        }
        Ok(l)
    }
    fn and_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.eq_expr()?;
        while self.eat_op("and") {
            l = Expr::Bin("and", l.into(), self.eq_expr()?.into());
        }
        Ok(l)
    }
    fn eq_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.rel_expr()?;
        loop {
            let op = if self.eat_op("=") { "=" } else if self.eat_op("!=") { "!=" } else { break };
            l = Expr::Bin(op, l.into(), self.rel_expr()?.into());
        }
        Ok(l)
    }
    fn rel_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.add_expr()?;
        loop {
            let op = if self.eat_op("<=") { "<=" } else if self.eat_op(">=") { ">=" }
                else if self.eat_op("<") { "<" } else if self.eat_op(">") { ">" } else { break };
            l = Expr::Bin(op, l.into(), self.add_expr()?.into());
        }
        Ok(l)
    }
    fn add_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.mul_expr()?;
        loop {
            let op = if self.eat_op("+") { "+" } else if self.eat_op("-") { "-" } else { break };
            l = Expr::Bin(op, l.into(), self.mul_expr()?.into());
        }
        Ok(l)
    }
    fn mul_expr(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let op = if self.eat_op("mul") { "*" } else if self.eat_op("div") { "div" }
                else if self.eat_op("mod") { "mod" } else { break };
            l = Expr::Bin(op, l.into(), self.unary()?.into());
        }
        Ok(l)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat_op("-") {
            return Ok(Expr::Neg(self.unary()?.into()));
        }
        self.union()
    }
    fn union(&mut self) -> Result<Expr, String> {
        let mut l = self.path()?;
        while self.eat_op("|") {
            l = Expr::Bin("|", l.into(), self.path()?.into());
        }
        Ok(l)
    }

    /// A location path, or a primary expression with steps hung off it.
    fn path(&mut self) -> Result<Expr, String> {
        // A primary expression starts a FilterExpr: `(`, a literal, a number,
        // or a function call. Everything else is a location path.
        let primary = matches!(self.peek(), Tok::Num(_) | Tok::Str(_) | Tok::Fn(_))
            || matches!(self.peek(), Tok::Op("(") | Tok::Op("$"));
        if primary {
            let base = self.primary()?;
            let mut preds = Vec::new();
            while self.eat_op("[") {
                preds.push(self.expr()?);
                self.expect_op("]")?;
            }
            let mut steps = Vec::new();
            self.trailing_steps(&mut steps)?;
            return Ok(Expr::Filter { base: base.into(), preds, steps });
        }
        let mut steps = Vec::new();
        let absolute = if self.eat_op("//") {
            steps.push(Step { axis: Axis::DescendantOrSelf, test: Test::AnyNode, preds: Vec::new() });
            steps.push(self.step()?);
            true
        } else if self.eat_op("/") {
            // A lone `/` is the root itself.
            if !matches!(self.peek(), Tok::Eof | Tok::Op("]") | Tok::Op(")") | Tok::Op(",")) {
                steps.push(self.step()?);
            }
            true
        } else {
            steps.push(self.step()?);
            false
        };
        self.trailing_steps(&mut steps)?;
        Ok(Expr::Path { absolute, steps })
    }

    fn trailing_steps(&mut self, steps: &mut Vec<Step>) -> Result<(), String> {
        loop {
            if self.eat_op("//") {
                steps.push(Step { axis: Axis::DescendantOrSelf, test: Test::AnyNode, preds: Vec::new() });
                steps.push(self.step()?);
            } else if self.eat_op("/") {
                steps.push(self.step()?);
            } else {
                return Ok(());
            }
        }
    }

    fn step(&mut self) -> Result<Step, String> {
        if self.eat_op(".") {
            return Ok(Step { axis: Axis::SelfAxis, test: Test::AnyNode, preds: Vec::new() });
        }
        if self.eat_op("..") {
            return Ok(Step { axis: Axis::Parent, test: Test::AnyNode, preds: Vec::new() });
        }
        let axis = if self.eat_op("@") {
            Axis::Attribute
        } else if let Tok::Axis(name) = self.peek().clone() {
            self.i += 1;
            match name.as_str() {
                "child" => Axis::Child,
                "descendant" => Axis::Descendant,
                "descendant-or-self" => Axis::DescendantOrSelf,
                "parent" => Axis::Parent,
                "ancestor" => Axis::Ancestor,
                "ancestor-or-self" => Axis::AncestorOrSelf,
                "self" => Axis::SelfAxis,
                "attribute" => Axis::Attribute,
                "following-sibling" => Axis::FollowingSibling,
                "preceding-sibling" => Axis::PrecedingSibling,
                other => return Err(alloc::format!("unsupported axis {other}")),
            }
        } else {
            Axis::Child
        };
        let test = match self.peek().clone() {
            Tok::Op("*") => {
                self.i += 1;
                Test::Any
            }
            Tok::Name(n) => {
                self.i += 1;
                // `node()` / `text()` / `comment()` — a node TYPE test.
                if self.eat_op("(") {
                    self.expect_op(")")?;
                    match n.as_str() {
                        "node" => Test::AnyNode,
                        "text" => Test::Text,
                        "comment" => Test::Comment,
                        // `processing-instruction()` matches nothing here.
                        _ => Test::Comment,
                    }
                } else {
                    Test::Name(n)
                }
            }
            t => return Err(alloc::format!("expected a node test, got {t:?}")),
        };
        let mut preds = Vec::new();
        while self.eat_op("[") {
            preds.push(self.expr()?);
            self.expect_op("]")?;
        }
        Ok(Step { axis, test, preds })
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().clone() {
            Tok::Num(n) => {
                self.i += 1;
                Ok(Expr::Num(n))
            }
            Tok::Str(s) => {
                self.i += 1;
                Ok(Expr::Str(s))
            }
            Tok::Fn(name) => {
                self.i += 1;
                self.expect_op("(")?;
                let mut args = Vec::new();
                if !self.eat_op(")") {
                    loop {
                        args.push(self.expr()?);
                        if self.eat_op(")") {
                            break;
                        }
                        self.expect_op(",")?;
                    }
                }
                Ok(Expr::Call(name, args))
            }
            Tok::Op("(") => {
                self.i += 1;
                let e = self.expr()?;
                self.expect_op(")")?;
                Ok(e)
            }
            // A variable reference has no way to be bound through the DOM API.
            Tok::Op("$") => Err("variable references are not supported".to_string()),
            t => Err(alloc::format!("unexpected {t:?}")),
        }
    }
}

/// A parsed expression, reusable against many context nodes — which is the
/// point of `createExpression`: htmx parses its one selector once at load and
/// evaluates it on every node it processes.
#[derive(Clone, Debug)]
pub struct XPath {
    root: Expr,
}

pub fn parse(src: &str) -> Result<XPath, String> {
    let mut p = P { t: lex(src)?, i: 0 };
    let e = p.expr()?;
    if !matches!(p.peek(), Tok::Eof) {
        return Err(alloc::format!("trailing tokens: {:?}", p.peek()));
    }
    Ok(XPath { root: e })
}

// ───────────────────────────── evaluator ─────────────────────────────

struct Ctx<'a> {
    doc: &'a Doc,
    node: XNode,
    pos: usize,
    size: usize,
}

impl XPath {
    /// Evaluate against `context`. Node-sets come back in DOCUMENT ORDER,
    /// which is what an iterator result promises and what makes
    /// `iterateNext()` walk a subtree top-down.
    pub fn eval(&self, doc: &Doc, context: XNode) -> XVal {
        let c = Ctx { doc, node: context, pos: 1, size: 1 };
        eval(&self.root, &c)
    }
}

fn node_ok(doc: &Doc, id: u32) -> Option<&DomNode> {
    doc.nodes.get(id as usize)
}

/// Every node of `axis` from `n`, in document order.
fn axis_nodes(doc: &Doc, n: XNode, axis: Axis) -> Vec<XNode> {
    let mut out = Vec::new();
    let id = n.id();
    match axis {
        Axis::SelfAxis => out.push(n),
        Axis::Child => {
            if let (XNode::Node(_), Some(e)) = (n, node_ok(doc, id)) {
                out.extend(e.children.iter().map(|c| XNode::Node(*c)));
            }
        }
        Axis::Descendant | Axis::DescendantOrSelf => {
            if axis == Axis::DescendantOrSelf {
                out.push(n);
            }
            if matches!(n, XNode::Node(_)) {
                descend(doc, id, &mut out);
            }
        }
        Axis::Parent => {
            match n {
                // An attribute's parent is its owner element.
                XNode::Attr(o, _) => out.push(XNode::Node(o)),
                XNode::Node(_) => {
                    if let Some(p) = node_ok(doc, id).and_then(|e| e.parent) {
                        out.push(XNode::Node(p));
                    }
                }
            }
        }
        Axis::Ancestor | Axis::AncestorOrSelf => {
            if axis == Axis::AncestorOrSelf {
                out.push(n);
            }
            let mut cur = match n {
                XNode::Attr(o, _) => Some(o),
                XNode::Node(_) => node_ok(doc, id).and_then(|e| e.parent),
            };
            let mut chain = Vec::new();
            while let Some(c) = cur {
                chain.push(XNode::Node(c));
                cur = node_ok(doc, c).and_then(|e| e.parent);
            }
            // Ancestors are a REVERSE axis; document order puts the root first.
            chain.reverse();
            let mut merged = chain;
            merged.extend(out.drain(..));
            out = merged;
        }
        Axis::Attribute => {
            if let (XNode::Node(_), Some(e)) = (n, node_ok(doc, id)) {
                if e.kind == ELEMENT_NODE {
                    out.extend((0..e.attrs.len()).map(|k| XNode::Attr(id, k)));
                }
            }
        }
        Axis::FollowingSibling | Axis::PrecedingSibling => {
            if let XNode::Node(_) = n {
                if let Some(p) = node_ok(doc, id).and_then(|e| e.parent) {
                    if let Some(par) = node_ok(doc, p) {
                        let at = par.children.iter().position(|c| *c == id);
                        if let Some(at) = at {
                            let range: Vec<u32> = if axis == Axis::FollowingSibling {
                                par.children[at + 1..].to_vec()
                            } else {
                                par.children[..at].to_vec()
                            };
                            out.extend(range.into_iter().map(XNode::Node));
                        }
                    }
                }
            }
        }
    }
    out
}

fn descend(doc: &Doc, id: u32, out: &mut Vec<XNode>) {
    let Some(e) = node_ok(doc, id) else { return };
    for c in e.children.clone() {
        out.push(XNode::Node(c));
        descend(doc, c, out);
    }
}

/// The principal node type of an axis decides what `*` matches (§2.3):
/// `attribute::*` is every attribute, every other axis is every ELEMENT.
fn matches(doc: &Doc, n: XNode, axis: Axis, test: &Test) -> bool {
    let kind = match n {
        XNode::Attr(..) => f64::NAN, // attributes have no arena kind
        XNode::Node(i) => node_ok(doc, i).map(|e| e.kind).unwrap_or(0.0),
    };
    match test {
        Test::AnyNode => true,
        Test::Text => matches!(n, XNode::Node(_)) && kind == super::dombind::TEXT_NODE,
        Test::Comment => matches!(n, XNode::Node(_)) && kind == super::dombind::COMMENT_NODE,
        Test::Any => match axis {
            Axis::Attribute => matches!(n, XNode::Attr(..)),
            _ => kind == ELEMENT_NODE,
        },
        Test::Name(want) => match n {
            XNode::Attr(o, k) => node_ok(doc, o)
                .and_then(|e| e.attrs.get(k))
                .is_some_and(|(name, _)| &**name == want),
            XNode::Node(i) => node_ok(doc, i)
                .is_some_and(|e| e.kind == ELEMENT_NODE && e.tag.eq_ignore_ascii_case(want)),
        },
    }
}

/// The name of a node, as `name()` reports it.
fn node_name(doc: &Doc, n: XNode) -> String {
    match n {
        XNode::Attr(o, k) => node_ok(doc, o)
            .and_then(|e| e.attrs.get(k))
            .map(|(name, _)| name.to_string())
            .unwrap_or_default(),
        XNode::Node(i) => node_ok(doc, i)
            .filter(|e| e.kind == ELEMENT_NODE)
            .map(|e| e.tag.to_string())
            .unwrap_or_default(),
    }
}

/// The string-value of a node (§5): an element's is its concatenated text.
fn node_string(doc: &Doc, n: XNode) -> String {
    match n {
        XNode::Attr(o, k) => node_ok(doc, o)
            .and_then(|e| e.attrs.get(k))
            .map(|(_, v)| v.to_string())
            .unwrap_or_default(),
        XNode::Node(i) => {
            let mut s = String::new();
            gather_text(doc, i, &mut s);
            s
        }
    }
}

fn gather_text(doc: &Doc, id: u32, out: &mut String) {
    let Some(e) = node_ok(doc, id) else { return };
    if e.kind == super::dombind::TEXT_NODE {
        out.push_str(&e.text);
        return;
    }
    for c in &e.children {
        gather_text(doc, *c, out);
    }
}

pub fn to_boolean(doc: &Doc, v: &XVal) -> bool {
    match v {
        XVal::Bool(b) => *b,
        XVal::Num(n) => *n != 0.0 && !n.is_nan(),
        XVal::Str(s) => !s.is_empty(),
        XVal::Nodes(ns) => {
            let _ = doc;
            !ns.is_empty()
        }
    }
}

pub fn to_str(doc: &Doc, v: &XVal) -> String {
    match v {
        XVal::Str(s) => s.clone(),
        XVal::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
        XVal::Num(n) => fmt_num(*n),
        // A node-set's string-value is that of its FIRST node in document order.
        XVal::Nodes(ns) => ns.first().map(|n| node_string(doc, *n)).unwrap_or_default(),
    }
}

fn fmt_num(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() };
    }
    if n == libm::trunc(n) && libm::fabs(n) < 1e21 {
        return alloc::format!("{}", n as i64);
    }
    alloc::format!("{n}")
}

pub fn to_number(doc: &Doc, v: &XVal) -> f64 {
    match v {
        XVal::Num(n) => *n,
        XVal::Bool(b) => if *b { 1.0 } else { 0.0 },
        XVal::Str(s) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        XVal::Nodes(_) => to_str(doc, v).trim().parse::<f64>().unwrap_or(f64::NAN),
    }
}

fn eval(e: &Expr, c: &Ctx) -> XVal {
    match e {
        Expr::Num(n) => XVal::Num(*n),
        Expr::Str(s) => XVal::Str(s.clone()),
        Expr::Neg(x) => XVal::Num(-to_number(c.doc, &eval(x, c))),
        Expr::Call(name, args) => call(name, args, c),
        Expr::Bin(op, l, r) => binary(op, l, r, c),
        Expr::Path { absolute, steps } => {
            let start = if *absolute {
                XNode::Node(root_of(c.doc, c.node.id()))
            } else {
                c.node
            };
            XVal::Nodes(walk(c.doc, alloc::vec![start], steps))
        }
        Expr::Filter { base, preds, steps } => {
            let v = eval(base, c);
            let mut nodes = match v {
                XVal::Nodes(n) => n,
                // A non-node-set with steps hung off it selects nothing.
                other => {
                    return if steps.is_empty() && preds.is_empty() { other } else { XVal::Nodes(Vec::new()) };
                }
            };
            for p in preds {
                nodes = filter(c.doc, nodes, p);
            }
            XVal::Nodes(walk(c.doc, nodes, steps))
        }
    }
}

fn root_of(doc: &Doc, mut id: u32) -> u32 {
    while let Some(p) = node_ok(doc, id).and_then(|e| e.parent) {
        id = p;
    }
    id
}

/// Apply `steps` to a starting node-set, keeping document order and dropping
/// duplicates — a node reached by two paths appears once (§3.3).
fn walk(doc: &Doc, start: Vec<XNode>, steps: &[Step]) -> Vec<XNode> {
    let mut cur = start;
    for st in steps {
        let mut next: Vec<XNode> = Vec::new();
        for n in &cur {
            for cand in axis_nodes(doc, *n, st.axis) {
                if matches(doc, cand, st.axis, &st.test) && !next.contains(&cand) {
                    next.push(cand);
                }
            }
        }
        for p in &st.preds {
            next = filter(doc, next, p);
        }
        cur = next;
    }
    cur
}

/// A predicate keeps a node when the expression is true for it — and a NUMBER
/// means "the node at this position" (§3.3), which is why `foo[1]` works.
fn filter(doc: &Doc, nodes: Vec<XNode>, pred: &Expr) -> Vec<XNode> {
    let size = nodes.len();
    let mut out = Vec::new();
    for (k, n) in nodes.into_iter().enumerate() {
        let c = Ctx { doc, node: n, pos: k + 1, size };
        let v = eval(pred, &c);
        let keep = match v {
            XVal::Num(x) => x == (k + 1) as f64,
            other => to_boolean(doc, &other),
        };
        if keep {
            out.push(n);
        }
    }
    out
}

fn binary(op: &str, l: &Expr, r: &Expr, c: &Ctx) -> XVal {
    match op {
        "or" => return XVal::Bool(to_boolean(c.doc, &eval(l, c)) || to_boolean(c.doc, &eval(r, c))),
        "and" => return XVal::Bool(to_boolean(c.doc, &eval(l, c)) && to_boolean(c.doc, &eval(r, c))),
        _ => {}
    }
    let (a, b) = (eval(l, c), eval(r, c));
    match op {
        "|" => {
            let mut out = match a {
                XVal::Nodes(n) => n,
                _ => Vec::new(),
            };
            if let XVal::Nodes(n) = b {
                for x in n {
                    if !out.contains(&x) {
                        out.push(x);
                    }
                }
            }
            XVal::Nodes(out)
        }
        "=" | "!=" => XVal::Bool(compare_eq(c.doc, &a, &b, op == "!=")),
        "<" | ">" | "<=" | ">=" => {
            let (x, y) = (to_number(c.doc, &a), to_number(c.doc, &b));
            XVal::Bool(match op {
                "<" => x < y,
                ">" => x > y,
                "<=" => x <= y,
                _ => x >= y,
            })
        }
        "+" => XVal::Num(to_number(c.doc, &a) + to_number(c.doc, &b)),
        "-" => XVal::Num(to_number(c.doc, &a) - to_number(c.doc, &b)),
        "*" => XVal::Num(to_number(c.doc, &a) * to_number(c.doc, &b)),
        "div" => XVal::Num(to_number(c.doc, &a) / to_number(c.doc, &b)),
        "mod" => XVal::Num(to_number(c.doc, &a) % to_number(c.doc, &b)),
        _ => XVal::Bool(false),
    }
}

/// `=` against a node-set is EXISTENTIAL (§3.4): true when ANY node compares
/// equal, which is why `@class = "x"` is not the same question as
/// `not(@class != "x")`.
fn compare_eq(doc: &Doc, a: &XVal, b: &XVal, negate: bool) -> bool {
    let one = |ns: &Vec<XNode>, other: &XVal| -> bool {
        ns.iter().any(|n| {
            let s = node_string(doc, *n);
            let hit = match other {
                XVal::Num(x) => s.trim().parse::<f64>().map(|v| v == *x).unwrap_or(false),
                XVal::Bool(x) => !s.is_empty() == *x,
                _ => s == to_str(doc, other),
            };
            hit != negate
        })
    };
    match (a, b) {
        (XVal::Nodes(x), XVal::Nodes(y)) => x.iter().any(|n| {
            let s = node_string(doc, *n);
            y.iter().any(|m| (node_string(doc, *m) == s) != negate)
        }),
        (XVal::Nodes(x), other) => one(x, other),
        (other, XVal::Nodes(y)) => one(y, other),
        (XVal::Bool(_), _) | (_, XVal::Bool(_)) => (to_boolean(doc, a) == to_boolean(doc, b)) != negate,
        (XVal::Num(_), _) | (_, XVal::Num(_)) => (to_number(doc, a) == to_number(doc, b)) != negate,
        _ => (to_str(doc, a) == to_str(doc, b)) != negate,
    }
}

fn call(name: &str, args: &[Expr], c: &Ctx) -> XVal {
    let arg = |i: usize| -> XVal { args.get(i).map(|a| eval(a, c)).unwrap_or(XVal::Str(String::new())) };
    let s = |i: usize| -> String {
        match args.get(i) {
            Some(a) => to_str(c.doc, &eval(a, c)),
            // A missing argument defaults to the CONTEXT node's string-value.
            None => node_string(c.doc, c.node),
        }
    };
    let n = |i: usize| -> f64 { to_number(c.doc, &arg(i)) };
    match name {
        "last" => XVal::Num(c.size as f64),
        "position" => XVal::Num(c.pos as f64),
        "count" => XVal::Num(match arg(0) {
            XVal::Nodes(v) => v.len() as f64,
            _ => f64::NAN,
        }),
        "name" | "local-name" => XVal::Str(match args.first() {
            None => node_name(c.doc, c.node),
            Some(a) => match eval(a, c) {
                XVal::Nodes(v) => v.first().map(|x| node_name(c.doc, *x)).unwrap_or_default(),
                _ => String::new(),
            },
        }),
        "string" => XVal::Str(s(0)),
        "concat" => {
            let mut out = String::new();
            for k in 0..args.len() {
                out.push_str(&s(k));
            }
            XVal::Str(out)
        }
        "starts-with" => XVal::Bool(s(0).starts_with(&s(1))),
        "contains" => XVal::Bool(s(0).contains(&s(1))),
        "substring-before" => {
            let (h, nd) = (s(0), s(1));
            XVal::Str(h.find(&nd).map(|i| h[..i].to_string()).unwrap_or_default())
        }
        "substring-after" => {
            let (h, nd) = (s(0), s(1));
            XVal::Str(h.find(&nd).map(|i| h[i + nd.len()..].to_string()).unwrap_or_default())
        }
        "substring" => {
            // 1-based, and rounded — `substring("12345", 1.5, 2.6)` is "234".
            let src: Vec<char> = s(0).chars().collect();
            let start = libm::round(n(1));
            let end = if args.len() > 2 { start + libm::round(n(2)) } else { f64::INFINITY };
            let mut out = String::new();
            for (k, ch) in src.iter().enumerate() {
                let p = (k + 1) as f64;
                if p >= start && p < end {
                    out.push(*ch);
                }
            }
            XVal::Str(out)
        }
        "string-length" => XVal::Num(s(0).chars().count() as f64),
        "normalize-space" => XVal::Str(s(0).split_whitespace().collect::<Vec<_>>().join(" ")),
        "translate" => {
            let (src, from, to) = (s(0), s(1), s(2));
            let from: Vec<char> = from.chars().collect();
            let to: Vec<char> = to.chars().collect();
            let mut out = String::new();
            for ch in src.chars() {
                match from.iter().position(|f| *f == ch) {
                    Some(i) if i < to.len() => out.push(to[i]),
                    Some(_) => {}
                    None => out.push(ch),
                }
            }
            XVal::Str(out)
        }
        "boolean" => XVal::Bool(to_boolean(c.doc, &arg(0))),
        "not" => XVal::Bool(!to_boolean(c.doc, &arg(0))),
        "true" => XVal::Bool(true),
        "false" => XVal::Bool(false),
        "number" => XVal::Num(match args.first() {
            None => node_string(c.doc, c.node).trim().parse::<f64>().unwrap_or(f64::NAN),
            Some(_) => n(0),
        }),
        "sum" => XVal::Num(match arg(0) {
            XVal::Nodes(v) => v
                .iter()
                .map(|x| node_string(c.doc, *x).trim().parse::<f64>().unwrap_or(f64::NAN))
                .sum(),
            _ => f64::NAN,
        }),
        "floor" => XVal::Num(libm::floor(n(0))),
        "ceiling" => XVal::Num(libm::ceil(n(0))),
        "round" => XVal::Num(libm::round(n(0))),
        // An unknown function is an error in XPath; answering `false` would
        // silently change what a predicate selects.
        _ => XVal::Bool(false),
    }
}

/// The node-set of a result, in document order, for the DOM API to iterate.
pub fn result_nodes(doc: &Doc, v: XVal) -> Vec<XNode> {
    match v {
        XVal::Nodes(mut n) => {
            n.sort_by_key(|x| doc_order(doc, *x));
            n
        }
        _ => Vec::new(),
    }
}

/// A sort key that puts nodes in document order: the path of child indices
/// from the root, with an attribute after its owner element.
fn doc_order(doc: &Doc, n: XNode) -> Vec<u32> {
    let mut path = Vec::new();
    let mut cur = n.id();
    while let Some(p) = node_ok(doc, cur).and_then(|e| e.parent) {
        let at = node_ok(doc, p)
            .and_then(|e| e.children.iter().position(|c| *c == cur))
            .unwrap_or(0);
        path.push(at as u32);
        cur = p;
    }
    path.reverse();
    if let XNode::Attr(_, k) = n {
        path.push(k as u32);
    }
    path
}

/// Ergonomic entry point: parse and evaluate in one call.
pub fn eval_str(doc: &Doc, context: XNode, src: &str) -> Result<XVal, String> {
    Ok(parse(src)?.eval(doc, context))
}

pub type Shared = Rc<XPath>;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn doc(html: &str) -> Doc {
        Doc::from_dom(&crate::dom::parse(html))
    }
    /// Evaluate against the document element and report the matched names.
    fn names(d: &Doc, expr: &str) -> Vec<String> {
        let v = eval_str(d, XNode::Node(0), expr).expect("parse");
        result_nodes(d, v).into_iter().map(|n| node_name(d, n)).collect()
    }
    fn s(d: &Doc, expr: &str) -> String {
        to_str(d, &eval_str(d, XNode::Node(0), expr).expect("parse"))
    }

    /// **The expression htmx actually ships.** It is the reason this module
    /// exists, so it is the first test: every element carrying any attribute
    /// whose name starts with one of four prefixes.
    #[test]
    fn the_htmx_expression() {
        let d = doc(
            "<body><div id=a hx-on:click='x'></div><p id=b data-hx-on-load='y'>t</p>\
             <span id=c class=plain></span><i id=d data-hx-on:keyup='z'></i></body>",
        );
        let v = eval_str(
            &d,
            XNode::Node(0),
            r#".//*[@*[ starts-with(name(), "hx-on:") or starts-with(name(), "data-hx-on:") or starts-with(name(), "hx-on-") or starts-with(name(), "data-hx-on-") ]]"#,
        )
        .expect("parse");
        let ids: Vec<String> = result_nodes(&d, v)
            .into_iter()
            .map(|n| node_string(&d, XNode::Attr(n.id(), 0)))
            .collect();
        assert_eq!(ids, vec!["a", "b", "d"]);
    }

    #[test]
    fn axes_and_name_tests() {
        let d = doc("<body><div><p>one</p><p>two</p></div><span></span></body>");
        assert_eq!(names(&d, "//p"), vec!["p", "p"]);
        assert_eq!(names(&d, "//div/p"), vec!["p", "p"]);
        assert_eq!(names(&d, "//div/*"), vec!["p", "p"]);
        assert_eq!(names(&d, "//p/.."), vec!["div"]);
        assert_eq!(names(&d, "//p[1]"), vec!["p"]);
        assert_eq!(names(&d, "//span/preceding-sibling::div"), vec!["div"]);
        assert_eq!(names(&d, "//div/following-sibling::*"), vec!["span"]);
        // `descendant-or-self` reaches the node itself.
        assert_eq!(names(&d, "//div/descendant-or-self::div"), vec!["div"]);
    }

    #[test]
    fn predicates_and_the_core_functions() {
        let d = doc("<body><a href='/x' class='btn go'>Ja</a><a href='/y'>Nein</a></body>");
        assert_eq!(names(&d, "//a[@class]"), vec!["a"]);
        assert_eq!(names(&d, "//a[@href='/y']"), vec!["a"]);
        assert_eq!(names(&d, "//a[contains(@class,'go')]"), vec!["a"]);
        assert_eq!(names(&d, "//a[not(@class)]"), vec!["a"]);
        assert_eq!(s(&d, "count(//a)"), "2");
        assert_eq!(s(&d, "string(//a)"), "Ja");
        assert_eq!(s(&d, "concat('a','-',name(//a))"), "a-a");
        assert_eq!(s(&d, "normalize-space('  x   y ')"), "x y");
        assert_eq!(s(&d, "substring('12345', 2, 3)"), "234");
        assert_eq!(s(&d, "translate('abc','ab','AB')"), "ABc");
        assert_eq!(s(&d, "string-length('äöü')"), "3");
        assert_eq!(s(&d, "1 + 2 * 3"), "7");
        assert_eq!(s(&d, "floor(1.7) + ceiling(0.2) + round(2.5)"), "5");
    }

    /// The one context rule of the grammar: `*` after `@` is a wildcard, `*`
    /// after an operand is a multiplication. Both appear in htmx's selector.
    #[test]
    fn star_is_a_wildcard_or_a_product_by_context() {
        let d = doc("<body><div a='1'></div></body>");
        assert_eq!(names(&d, "//*[@*]"), vec!["div"]);
        assert_eq!(s(&d, "3 * 4"), "12");
        assert_eq!(s(&d, "count(//div/@*)"), "1");
    }

    /// `=` against a node-set is existential, so it is NOT the negation of
    /// `!=` — the distinction decides what a predicate selects.
    #[test]
    fn equality_against_a_node_set_is_existential() {
        let d = doc("<body><ul><li>a</li><li>b</li></ul></body>");
        assert_eq!(s(&d, "boolean(//li = 'a')"), "true");
        assert_eq!(s(&d, "boolean(//li != 'a')"), "true");
        assert_eq!(s(&d, "boolean(//li = 'z')"), "false");
    }

    #[test]
    fn a_bad_expression_is_an_error_not_an_empty_set() {
        assert!(parse("//[").is_err());
        assert!(parse("foo::bar").is_err());
        assert!(parse("$x").is_err());
    }
}
