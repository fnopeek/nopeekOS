# `tools/wasm/beak-engine/src/js/xpath.rs` @ 5e0102684

## L1-19 · `use alloc::rc::Rc;`

```
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
```

## L27-30 · `#[derive(Clone, Copy, PartialEq, Eq, Debug)]`

```
/// A node in an XPath node-set. Attributes are not arena nodes, so they are
/// addressed as (owner element, index into its `attrs`) rather than given a
/// synthetic arena slot — which would have to be kept in step with every
/// mutation for the sake of one axis.
```

## L45-46 · `#[derive(Clone, Debug)]`

```
/// An XPath value (XPath 1.0 §1): the four types, and every conversion between
/// them is defined by the spec rather than by convenience.
```

## L55 · `#[derive(Clone, PartialEq, Debug)]`

```
// ───────────────────────────── lexer ─────────────────────────────
```

## L60 · `Axis(String),`

```
/// `prefix::` — an axis specifier, already split off its `::`.
```

## L76-80 · `fn lex(src: &str) -> Result<Vec<Tok>, String> {`

```
/// **The one context rule of the XPath grammar** (§3.7): `*` and a bare name
/// like `and`, `or`, `div`, `mod` are OPERATORS unless the preceding token is
/// `@`, `::`, `(`, `[`, `,` or another operator — in which case they are a
/// wildcard or a node test. Without it, `.//*[@*[...]]` lexes its second `*`
/// as a multiplication and the whole expression is a parse error.
```

## L91 · `let prev_is_operand = matches!(`

```
// Does the previous token allow an operator here?
```

## L161-163 · `while i < b.len() && is_name_char(b[i]) {`

```
// `:` belongs to a name (`svg:rect`) but `::` does NOT — it is
// the axis separator, and letting the name scan swallow it made
// `child::p` one long name and every axis silently a tag.
```

## L183 · `out.push(Tok::Name(name)); // a node type test, kept as a name`

```
// a node type test, kept as a name
```

## L200 · `#[derive(Clone, Copy, PartialEq, Debug)]`

```
// ───────────────────────────── AST ─────────────────────────────
```

## L218 · `Any,`

```
/// `*` — any node of the axis's principal type.
```

## L221 · `AnyNode,`

```
/// `node()`
```

## L223 · `Text,`

```
/// `text()`
```

## L237 · `Path { absolute: bool, steps: Vec<Step> },`

```
/// A location path. `absolute` starts at the document root.
```

## L239 · `Filter { base: alloc::boxed::Box<Expr>, preds: Vec<Expr>, steps: Vec<Step> },`

```
/// A primary expression with steps hung off it (`foo(...)/bar`).
```

## L248 · `struct P {`

```
// ───────────────────────────── parser ─────────────────────────────
```

## L335 · `fn path(&mut self) -> Result<Expr, String> {`

```
/// A location path, or a primary expression with steps hung off it.
```

## L337-338 · `let primary = matches!(self.peek(), Tok::Num(_) | Tok::Str(_) | Tok::Fn(_))`

```
// A primary expression starts a FilterExpr: `(`, a literal, a number,
// or a function call. Everything else is a location path.
```

## L358 · `if !matches!(self.peek(), Tok::Eof | Tok::Op("]") | Tok::Op(")") | Tok::Op(",")) {`

```
// A lone `/` is the root itself.
```

## L418 · `if self.eat_op("(") {`

```
// `node()` / `text()` / `comment()` — a node TYPE test.
```

## L425 · `_ => Test::Comment,`

```
// `processing-instruction()` matches nothing here.
```

## L473 · `Tok::Op("$") => Err("variable references are not supported".to_string()),`

```
// A variable reference has no way to be bound through the DOM API.
```

## L480-482 · `#[derive(Clone, Debug)]`

```
/// A parsed expression, reusable against many context nodes — which is the
/// point of `createExpression`: htmx parses its one selector once at load and
/// evaluates it on every node it processes.
```

## L497 · `struct Ctx<'a> {`

```
// ───────────────────────────── evaluator ─────────────────────────────
```

## L507-509 · `pub fn eval(&self, doc: &Doc, context: XNode) -> XVal {`

```
/// Evaluate against `context`. Node-sets come back in DOCUMENT ORDER,
/// which is what an iterator result promises and what makes
/// `iterateNext()` walk a subtree top-down.
```

## L520 · `fn axis_nodes(doc: &Doc, n: XNode, axis: Axis) -> Vec<XNode> {`

```
/// Every node of `axis` from `n`, in document order.
```

## L541 · `XNode::Attr(o, _) => out.push(XNode::Node(o)),`

```
// An attribute's parent is its owner element.
```

## L563 · `chain.reverse();`

```
// Ancestors are a REVERSE axis; document order puts the root first.
```

## L605-606 · `fn matches(doc: &Doc, n: XNode, axis: Axis, test: &Test) -> bool {`

```
/// The principal node type of an axis decides what `*` matches (§2.3):
/// `attribute::*` is every attribute, every other axis is every ELEMENT.
```

## L609 · `XNode::Attr(..) => f64::NAN, // attributes have no arena kind`

```
// attributes have no arena kind
```

## L630 · `fn node_name(doc: &Doc, n: XNode) -> String {`

```
/// The name of a node, as `name()` reports it.
```

## L644 · `fn node_string(doc: &Doc, n: XNode) -> String {`

```
/// The string-value of a node (§5): an element's is its concatenated text.
```

## L687 · `XVal::Nodes(ns) => ns.first().map(|n| node_string(doc, *n)).unwrap_or_default(),`

```
// A node-set's string-value is that of its FIRST node in document order.
```

## L733 · `other => {`

```
// A non-node-set with steps hung off it selects nothing.
```

## L753-754 · `fn walk(doc: &Doc, start: Vec<XNode>, steps: &[Step]) -> Vec<XNode> {`

```
/// Apply `steps` to a starting node-set, keeping document order and dropping
/// duplicates — a node reached by two paths appears once (§3.3).
```

## L774-775 · `fn filter(doc: &Doc, nodes: Vec<XNode>, pred: &Expr) -> Vec<XNode> {`

```
/// A predicate keeps a node when the expression is true for it — and a NUMBER
/// means "the node at this position" (§3.3), which is why `foo[1]` works.
```

## L834-836 · `fn compare_eq(doc: &Doc, a: &XVal, b: &XVal, negate: bool) -> bool {`

```
/// `=` against a node-set is EXISTENTIAL (§3.4): true when ANY node compares
/// equal, which is why `@class = "x"` is not the same question as
/// `not(@class != "x")`.
```

## L867 · `None => node_string(c.doc, c.node),`

```
// A missing argument defaults to the CONTEXT node's string-value.
```

## L905 · `let src: Vec<char> = s(0).chars().collect();`

```
// 1-based, and rounded — `substring("12345", 1.5, 2.6)` is "234".
```

## L952-953 · `_ => XVal::Bool(false),`

```
// An unknown function is an error in XPath; answering `false` would
// silently change what a predicate selects.
```

## L958 · `pub fn result_nodes(doc: &Doc, v: XVal) -> Vec<XNode> {`

```
/// The node-set of a result, in document order, for the DOM API to iterate.
```

## L969-970 · `fn doc_order(doc: &Doc, n: XNode) -> Vec<u32> {`

```
/// A sort key that puts nodes in document order: the path of child indices
/// from the root, with an attribute after its owner element.
```

## L988 · `pub fn eval_str(doc: &Doc, context: XNode, src: &str) -> Result<XVal, String> {`

```
/// Ergonomic entry point: parse and evaluate in one call.
```

## L1003 · `fn names(d: &Doc, expr: &str) -> Vec<String> {`

```
/// Evaluate against the document element and report the matched names.
```

## L1012-1014 · `#[test]`

```
/// **The expression htmx actually ships.** It is the reason this module
/// exists, so it is the first test: every element carrying any attribute
/// whose name starts with one of four prefixes.
```

## L1044 · `assert_eq!(names(&d, "//div/descendant-or-self::div"), vec!["div"]);`

```
// `descendant-or-self` reaches the node itself.
```

## L1066-1067 · `#[test]`

```
/// The one context rule of the grammar: `*` after `@` is a wildcard, `*`
/// after an operand is a multiplication. Both appear in htmx's selector.
```

## L1076-1077 · `#[test]`

```
/// `=` against a node-set is existential, so it is NOT the negation of
/// `!=` — the distinction decides what a predicate selects.
```

