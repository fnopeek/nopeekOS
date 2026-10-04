# `tools/wasm/beak-engine/tests/gap.rs` @ 5e0102684

## L1-5 · `use std::collections::HashMap;`

```
//! CSS gap analysis: for a real page + its real stylesheets, count how many
//! DOM elements each declared property actually WINS on, then split that by
//! whether the engine implements the property.
//!
//! GAPHTML=wiki.html GAPCSS=wiki.css cargo test --release --test gap -- --nocapture
```

## L12-16 · `fn implemented() -> std::collections::HashSet<String> {`

```
/// Every property `style::apply_one` actually handles, read out of the source
/// at run time. This list used to be maintained by hand and went stale twice —
/// once claiming `background-image` was missing months after it shipped, which
/// put a phantom item at the top of the priority list. Deriving it costs one
/// file read and cannot drift.
```

## L28-30 · `let ind = line.len() - line.trim_start().len();`

```
// A match arm head sits at exactly 8 spaces and starts with a string
// pattern. `"a" | "b" => …` puts several on one line; a pattern broken
// over lines continues with `| "c"`.
```

## L36-37 · `let pat = t.split("=>").next().unwrap_or("");`

```
// Only what precedes `=>` is the pattern; the arm body may hold strings
// of its own (values, keywords) that are not property names.
```

## L54 · `tally: HashMap<String, (u32, HashMap<String, u32>)>,`

```
/// property -> (elements it wins on, distinct winning values)
```

## L57 · `tags: HashMap<String, u32>,`

```
/// tag -> count, for sanity
```

## L78-79 · `let mut winner: HashMap<&str, &str> = HashMap::new();`

```
// last writer per property wins (ignoring !important — close enough for a
// frequency census)
```

## L82 · `let decls = decls.iter().chain(imp.iter());`

```
// `!important` wins last — a census that ignored it undercounted.
```

## L88 · `if let Some(style) = el.attr("style") {`

```
// inline style attribute beats every stylesheet rule
```

## L111 · `let _ = &prev;`

```
// prev_siblings is passed as a slice of preceding siblings
```

## L137 · `let decls = decls.iter().chain(imp.iter());`

```
// `!important` wins last — a census that ignored it undercounted.
```

