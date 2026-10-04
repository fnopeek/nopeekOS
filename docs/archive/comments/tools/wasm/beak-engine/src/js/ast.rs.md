# `tools/wasm/beak-engine/src/js/ast.rs` @ 5e0102684

## L1-10 · `use alloc::boxed::Box;`

```
//! Der Syntaxbaum, in ESTree-Form.
//!
//! ESTree, weil es die Form ist, die jedes Werkzeug der Welt spricht — acorn,
//! esprima, babel. Das ist keine Bequemlichkeit: es macht den Baum gegen einen
//! zweiten Parser vergleichbar, und ein Orakel, das man nicht selbst geschrieben
//! hat, ist das einzige, das einen eigenen Fehler findet.
//!
//! Kein Arena, keine Ids: `Box` und `Vec`. Der Baum wird einmal gebaut und dann
//! gelesen; eine Arena spart Allokationen, die hier niemand zaehlt, und kostet
//! Lesbarkeit an jeder Stelle.
```

## L20-21 · `pub module: bool,`

```
/// `module` heisst: `import`/`export` erlaubt, immer streng, `await` oben
/// erlaubt. Das ist kein Schalter am Parser, es ist eine andere Grammatik.
```

## L23-26 · `pub strict: bool,`

```
/// Ob dieses Programm STRENG ist. Der Parser wusste das immer und hat es
/// weggeworfen; die Laufzeit war dadurch blind, und der Modus entschied
/// nur noch ueber Fruehfehler. Die Strenge steht am CODE, nicht am
/// Zustand des Laufs — deshalb hier und nicht im Interpreter.
```

## L83-86 · `#[derive(Debug, Clone, PartialEq)]`

```
/// Ein Bindungsmuster. Dass Muster und Ausdruecke sich ueberlappen (`[a, b]`
/// ist beides, bis das `=` kommt) ist die zentrale Schwierigkeit der Grammatik;
/// der Parser liest deshalb erst einen Ausdruck und biegt ihn um
/// (`expr_to_pattern`), statt vorauszuschauen.
```

## L94-95 · `Expr(Box<Expr>),`

```
/// `[a.b] = c` — ein Ziel, das kein Bezeichner ist. In einer Deklaration
/// verboten, in einer Zuweisung erlaubt.
```

## L112-113 · `pub is_arrow: bool,`

```
/// Ein Pfeil mit Ausdruckskoerper (`x => x*2`). Der Koerper steht dann als
/// einzelnes `Stmt::Return` in `body`, damit alles darunter EINEN Fall hat.
```

## L116-118 · `pub strict: bool,`

```
/// Streng? Entweder weil der Rumpf mit `"use strict"` beginnt, oder weil
/// die Funktion in strengem Code steht (auch: in einem Klassenkoerper,
/// der immer streng ist). Der Parser rechnet es ohnehin aus.
```

## L178-179 · `Chain(Box<Expr>),`

```
/// Die Kette um ein `?.`, damit ein Kurzschluss die GANZE Kette abbricht
/// und nicht nur das eine Glied.
```

## L185 · `MetaProp { meta: String, prop: String },`

```
/// `new.target` / `import.meta`
```

