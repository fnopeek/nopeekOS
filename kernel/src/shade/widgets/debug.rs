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
//! ```text
//! Column spacing=8 align=Stretch [Padding(8), Background(Surface)]
//!   Text "Files" style=Title
//!   Row spacing=4 align=Center
//!     Icon Folder size=16
//!     Text "Documents" style=Body
//!     Spacer flex=1
//!     Button "Open" → Action(42)
//!   Divider
//! ```

#![allow(dead_code)]

use alloc::string::String;
use core::fmt::Write;

use super::abi::{Modifier, Transition, Widget};

/// Print the tree to serial via `kprintln!`, one node per line.
pub fn print_tree(root: &Widget) {
    let mut out = String::new();
    let _ = writeln!(out, "[npk] ── widget tree ──────────────────────────");
    write_node(&mut out, root, 0);
    let _ = writeln!(out, "[npk] ─────────────────────────────────────────");
    crate::kprint!("{}", out);
}

fn write_node(out: &mut String, w: &Widget, depth: usize) {
    let indent = "  ".repeat(depth);
    match w {
        Widget::Column { children, spacing, align, modifiers } => {
            let _ = writeln!(out, "[npk] {}Column spacing={} align={:?}{}",
                indent, spacing, align, fmt_mods(modifiers));
            for c in children { write_node(out, c, depth + 1); }
        }
        Widget::Row { children, spacing, align, modifiers } => {
            let _ = writeln!(out, "[npk] {}Row spacing={} align={:?}{}",
                indent, spacing, align, fmt_mods(modifiers));
            for c in children { write_node(out, c, depth + 1); }
        }
        Widget::Stack { children, modifiers } => {
            let _ = writeln!(out, "[npk] {}Stack{}", indent, fmt_mods(modifiers));
            for c in children { write_node(out, c, depth + 1); }
        }
        Widget::Scroll { child, axis, modifiers } => {
            let _ = writeln!(out, "[npk] {}Scroll axis={:?}{}",
                indent, axis, fmt_mods(modifiers));
            write_node(out, child, depth + 1);
        }
        Widget::Text { content, style, modifiers } => {
            let _ = writeln!(out, "[npk] {}Text {:?} style={:?}{}",
                indent, content, style, fmt_mods(modifiers));
        }
        Widget::Icon { id, size, modifiers } => {
            let _ = writeln!(out, "[npk] {}Icon {:?} size={}{}",
                indent, id, size, fmt_mods(modifiers));
        }
        Widget::Button { label, icon, on_click, modifiers } => {
            let _ = writeln!(out, "[npk] {}Button {:?} icon={:?} → Action({}){}",
                indent, label, icon, on_click.0, fmt_mods(modifiers));
        }
        Widget::Input { value, placeholder, on_submit, modifiers } => {
            let _ = writeln!(out, "[npk] {}Input value={:?} placeholder={:?} → Action({}){}",
                indent, value, placeholder, on_submit.0, fmt_mods(modifiers));
        }
        Widget::Checkbox { value, on_toggle, modifiers } => {
            let _ = writeln!(out, "[npk] {}Checkbox {}={} → Action({}){}",
                indent, "value", value, on_toggle.0, fmt_mods(modifiers));
        }
        Widget::Spacer { flex } => {
            let _ = writeln!(out, "[npk] {}Spacer flex={}", indent, flex);
        }
        Widget::Divider => {
            let _ = writeln!(out, "[npk] {}Divider", indent);
        }
        Widget::Canvas { id, width, height, modifiers } => {
            let _ = writeln!(out, "[npk] {}Canvas id={} {}x{}{}",
                indent, id.0, width, height, fmt_mods(modifiers));
        }
        Widget::Popover { anchor, child, modifiers } => {
            let _ = writeln!(out, "[npk] {}Popover anchor={} (RESERVED){}",
                indent, anchor.0, fmt_mods(modifiers));
            write_node(out, child, depth + 1);
        }
        Widget::Tooltip { text, anchor, modifiers } => {
            let _ = writeln!(out, "[npk] {}Tooltip {:?} anchor={} (RESERVED){}",
                indent, text, anchor.0, fmt_mods(modifiers));
        }
        Widget::Menu { items, modifiers } => {
            let _ = writeln!(out, "[npk] {}Menu (RESERVED){}",
                indent, fmt_mods(modifiers));
            for c in items { write_node(out, c, depth + 1); }
        }
        _ => {
            let _ = writeln!(out, "[npk] {}<unknown variant>", indent);
        }
    }
}

fn fmt_mods(mods: &[Modifier]) -> String {
    if mods.is_empty() {
        return String::new();
    }
    let mut s = String::from(" [");
    for (i, m) in mods.iter().enumerate() {
        if i > 0 { s.push_str(", "); }
        match m {
            Modifier::Padding(n)    => { let _ = write!(s, "Padding({})", n); }
            Modifier::Margin(n)     => { let _ = write!(s, "Margin({})", n); }
            Modifier::Background(t) => { let _ = write!(s, "Background({:?})", t); }
            Modifier::Border { token, width, radius } => {
                let _ = write!(s, "Border({:?}, w={}, r={})", token, width, radius);
            }
            Modifier::Opacity(v)    => { let _ = write!(s, "Opacity({})", v); }
            Modifier::Transition(t) => {
                match t {
                    Transition::Spring        => s.push_str("Transition(Spring)"),
                    Transition::Linear { ms } => { let _ = write!(s, "Transition(Linear {}ms)", ms); }
                    _                         => s.push_str("Transition(?)"),
                }
            }
            Modifier::OnClick(a)    => { let _ = write!(s, "OnClick({})", a.0); }
            Modifier::OnHover(a)    => { let _ = write!(s, "OnHover({})", a.0); }
            Modifier::Blur(r)       => { let _ = write!(s, "Blur({})", r); }
            Modifier::Shadow(_)     => s.push_str("Shadow(..)"),
            Modifier::Effect(id)    => { let _ = write!(s, "Effect({:?})", id); }
            Modifier::RoleOverride(r) => { let _ = write!(s, "Role({:?})", r); }
            _                       => s.push_str("<unknown>"),
        }
    }
    s.push(']');
    s
}
