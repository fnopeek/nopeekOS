//! Undo and redo for a text widget the compositor edits.
//!
//! The compositor owns the buffer and reports every change as the whole
//! value (`Event::InputChange`) and every caret move as `Event::Caret`.
//! The app feeds both in; this module keeps the difference of each change
//! and, on undo or redo, hands back the text to commit together with a
//! `Modifier::Select` for the caret.
//!
//! Steps group the way editors do: typing a word is one step, a space,
//! a line break, a paste or a caret jump starts the next one.
//!
//! The history lives in the module's own heap and is bounded by
//! [`History::with_cap`]; the oldest steps fall off first.

use alloc::string::String;
use alloc::vec::Vec;

/// Default bound on the bytes a history keeps.
pub const DEFAULT_CAP: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    /// Byte offset of the change.
    at: usize,
    removed: String,
    inserted: String,
    /// `(anchor, cursor)` before and after.
    before: (u32, u32),
    after: (u32, u32),
}

impl Step {
    fn bytes(&self) -> usize { self.removed.len() + self.inserted.len() }
}

/// The text and caret an undo or redo leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    pub text: String,
    pub anchor: u32,
    pub cursor: u32,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    bytes: usize,
    cap: usize,
    /// The last step may still take the next keystroke.
    open: bool,
    /// The caret as last reported.
    caret: (u32, u32),
}

impl Default for History {
    fn default() -> Self { Self::with_cap(DEFAULT_CAP) }
}

impl History {
    pub fn with_cap(cap: usize) -> Self {
        History { undo: Vec::new(), redo: Vec::new(), bytes: 0, cap, open: false, caret: (0, 0) }
    }

    /// Forget everything, e.g. after loading another file.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.open = false;
    }

    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }

    /// The caret as last reported, `(anchor, cursor)`.
    pub fn caret(&self) -> (u32, u32) { self.caret }

    /// An `Event::Caret`. A caret that is not where the last change left it
    /// closes the open step.
    pub fn caret_moved(&mut self, anchor: u32, cursor: u32) {
        if let Some(last) = self.undo.last() {
            if last.after != (anchor, cursor) { self.open = false; }
        }
        self.caret = (anchor, cursor);
    }

    /// Close the open step, so the next change starts a new one.
    pub fn seal(&mut self) { self.open = false; }

    /// An `Event::InputChange`: the value went from `old` to `new`.
    pub fn changed(&mut self, old: &str, new: &str) {
        let Some((at, removed, inserted)) = diff(old, new) else { return };
        let after = (at + inserted.len()) as u32;
        let step = Step {
            at,
            removed: String::from(removed),
            inserted: String::from(inserted),
            before: self.caret,
            after: (after, after),
        };
        self.redo.clear();
        if self.open && self.merge(&step) {
            self.caret = step.after;
            self.trim();
            return;
        }
        self.bytes += step.bytes();
        self.open = continues_word(&step) || single_erase(&step);
        self.caret = step.after;
        self.undo.push(step);
        self.trim();
    }

    /// Take a keystroke into the last step: one more word character typed
    /// behind it, or one more character deleted in front of it.
    fn merge(&mut self, step: &Step) -> bool {
        let Some(last) = self.undo.last_mut() else { return false };
        let typed = step.removed.is_empty() && last.removed.is_empty()
            && step.at == last.at + last.inserted.len()
            && continues_word(step);
        let erased = step.inserted.is_empty() && last.inserted.is_empty()
            && step.at + step.removed.len() == last.at
            && single_erase(step);
        if typed {
            last.inserted.push_str(&step.inserted);
        } else if erased {
            last.removed.insert_str(0, &step.removed);
            last.at = step.at;
        } else {
            return false;
        }
        last.after = step.after;
        self.bytes += step.bytes();
        true
    }

    fn trim(&mut self) {
        while self.bytes > self.cap && self.undo.len() > 1 {
            let s = self.undo.remove(0);
            self.bytes -= s.bytes();
        }
    }

    /// Undo the last step on `text`, the current value.
    pub fn undo(&mut self, text: &str) -> Option<Restore> {
        let step = self.undo.pop()?;
        self.bytes -= step.bytes();
        self.open = false;
        let out = apply(text, step.at, step.inserted.len(), &step.removed)?;
        let r = Restore { text: out, anchor: step.before.0, cursor: step.before.1 };
        self.caret = step.before;
        self.redo.push(step);
        Some(r)
    }

    /// Redo the last undone step on `text`.
    pub fn redo(&mut self, text: &str) -> Option<Restore> {
        let step = self.redo.pop()?;
        self.open = false;
        let out = apply(text, step.at, step.removed.len(), &step.inserted)?;
        let r = Restore { text: out, anchor: step.after.0, cursor: step.after.1 };
        self.caret = step.after;
        self.bytes += step.bytes();
        self.undo.push(step);
        Some(r)
    }

    /// Record a change the app made itself (replace, reformat), as one
    /// step that ends with the caret at `(anchor, cursor)`.
    pub fn replaced(&mut self, old: &str, new: &str, anchor: u32, cursor: u32) {
        self.open = false;
        self.changed(old, new);
        self.open = false;
        if let Some(last) = self.undo.last_mut() { last.after = (anchor, cursor); }
        self.caret = (anchor, cursor);
    }
}

/// A single typed word character, which the next one may join.
fn continues_word(step: &Step) -> bool {
    let mut it = step.inserted.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => step.removed.is_empty() && !c.is_whitespace(),
        _ => false,
    }
}

/// A single deleted character, which the next Backspace may join.
fn single_erase(step: &Step) -> bool {
    step.inserted.is_empty() && step.removed.chars().count() == 1
}

/// `text` with `len` bytes at `at` replaced by `with`, or None if the
/// history no longer fits the text.
fn apply(text: &str, at: usize, len: usize, with: &str) -> Option<String> {
    let end = at.checked_add(len)?;
    if end > text.len() || !text.is_char_boundary(at) || !text.is_char_boundary(end) {
        return None;
    }
    let mut out = String::with_capacity(text.len() - len + with.len());
    out.push_str(&text[..at]);
    out.push_str(with);
    out.push_str(&text[end..]);
    Some(out)
}

/// The one changed range between `old` and `new`: offset, removed text,
/// inserted text. None if equal.
fn diff<'a>(old: &'a str, new: &'a str) -> Option<(usize, &'a str, &'a str)> {
    if old == new { return None; }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut pre = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(pre) || !new.is_char_boundary(pre) { pre -= 1; }
    let max_suf = (ob.len() - pre).min(nb.len() - pre);
    let mut suf = ob.iter().rev().zip(nb.iter().rev()).take(max_suf)
        .take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(ob.len() - suf) || !new.is_char_boundary(nb.len() - suf) {
        suf -= 1;
    }
    Some((pre, &old[pre..ob.len() - suf], &new[pre..nb.len() - suf]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(h: &mut History, text: &mut String, s: &str) {
        for c in s.chars() {
            let old = text.clone();
            let at = h.caret().1 as usize;
            text.insert(at, c);
            h.changed(&old, text);
            let p = (at + c.len_utf8()) as u32;
            h.caret_moved(p, p);
        }
    }

    #[test]
    fn words_group_and_undo_back() {
        let mut h = History::default();
        let mut t = String::new();
        typed(&mut h, &mut t, "hello world");
        assert_eq!(t, "hello world");
        let r = h.undo(&t).unwrap();
        assert_eq!(r.text, "hello ");
        let r2 = h.undo(&r.text).unwrap();
        assert_eq!(r2.text, "hello");
        let r3 = h.undo(&r2.text).unwrap();
        assert_eq!(r3.text, "");
        assert_eq!((r3.anchor, r3.cursor), (0, 0));
        assert!(h.undo(&r3.text).is_none());
        let f = h.redo(&r3.text).unwrap();
        assert_eq!(f.text, "hello");
        assert_eq!(f.cursor, 5);
    }

    #[test]
    fn caret_jump_seals() {
        let mut h = History::default();
        let mut t = String::new();
        typed(&mut h, &mut t, "ab");
        h.caret_moved(0, 0);
        typed(&mut h, &mut t, "x");
        assert_eq!(t, "xab");
        assert_eq!(h.undo(&t).unwrap().text, "ab");
    }

    #[test]
    fn backspaces_group() {
        let mut h = History::default();
        let mut t = String::from("abcd");
        h.caret_moved(4, 4);
        for _ in 0..3 {
            let old = t.clone();
            let c = h.caret().1 as usize;
            t.remove(c - 1);
            h.changed(&old, &t);
            h.caret_moved((c - 1) as u32, (c - 1) as u32);
        }
        assert_eq!(t, "a");
        let r = h.undo(&t).unwrap();
        assert_eq!(r.text, "abcd");
        assert_eq!(r.cursor, 4);
    }

    #[test]
    fn utf8_diff_stays_on_boundaries() {
        assert_eq!(diff("aü", "aö"), Some((1, "ü", "ö")));
        assert_eq!(diff("äb", "b"), Some((0, "ä", "")));
        let mut h = History::default();
        h.changed("Grüße", "Grüsse");
        assert_eq!(h.undo("Grüsse").unwrap().text, "Grüße");
    }

    #[test]
    fn new_edit_drops_redo_and_cap_trims() {
        let mut h = History::with_cap(4);
        h.changed("", "aaaa ");
        h.changed("aaaa ", "aaaa bbbb ");
        assert_eq!(h.undo.len(), 1);
        let r = h.undo("aaaa bbbb ").unwrap();
        assert!(h.can_redo());
        h.changed(&r.text, "aaaa c");
        assert!(!h.can_redo());
    }

    #[test]
    fn stale_history_does_not_panic() {
        let mut h = History::default();
        h.changed("", "hello world");
        assert!(h.undo("hi").is_none());
    }
}
