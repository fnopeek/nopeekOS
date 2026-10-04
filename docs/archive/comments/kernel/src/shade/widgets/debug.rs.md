# `kernel/src/shade/widgets/debug.rs` @ 5e0102684

## L1-21 · `#![allow(dead_code)]`

```
//! Serial pretty-printer for deserialized widget trees.
//!
//! P10.2 deliverable: when an app commits a tree, the compositor
//! prints it to serial so we can eyeball the round-trip. Later phases
//! (layout, rasterization) reduce the need for this, but the formatter
//! stays around as a debug probe.
//!
//! Output style — indented tree, one line per node, showing the
//! variant + the fields that carry user-visible data. Modifiers are
//! listed inline in square brackets so the structure stays legible.
//!
//! `​``text
//! Column spacing=8 align=Stretch [Padding(8), Background(Surface)]
//!   Text "Files" style=Title
//!   Row spacing=4 align=Center
//!     Icon Folder size=16
//!     Text "Documents" style=Body
//!     Spacer flex=1
//!     Button "Open" → Action(42)
//!   Divider
//! `​``
```

## L31-33 · `pub fn print_layout(root_widget: &Widget, root_layout: &LayoutNode) {`

```
/// Print the layout tree to serial — each node on one line with its
/// absolute rect and baseline. Lockstepped with `print_tree`: pass the
/// same widget + layout pair and rows line up.
```

## L54-55 · `for (cw, cl) in widget_children(w).iter().zip(n.children.iter()) {`

```
// Recurse into the children — both trees share the same structural
// shape (layout mirrors widget). Mismatch = bug in layout.rs.
```

## L61 · `fn widget_label(w: &Widget) -> String {`

```
/// Short, one-line label of a widget variant for the layout dump.
```

## L87-89 · `fn widget_children(w: &Widget) -> alloc::vec::Vec<&Widget> {`

```
/// Iterator-free peek at a widget's children — empty slice for leaves.
/// Kept as a helper so `write_layout_node` doesn't duplicate variant
/// matching logic against layout's `children`.
```

## L107 · `pub fn print_tree(root: &Widget) {`

```
/// Print the tree to serial via `kprintln!`, one node per line.
```

## L221 · `Modifier::Hover(inner)        => { let _ = write!(s, "Hover{}", fmt_mods(inner)); }`

```
// Vocab v2 (Tailwind-style additions)
```

