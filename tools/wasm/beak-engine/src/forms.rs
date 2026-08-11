//! forms.rs — HTML forms (Stage 0: no JS).
//!
//! Three pieces, all host-testable and host-free:
//!   * `collect` — the document's `<form>`s and their controls, in document
//!     order, each keyed by its element `seq` (see `dom::Element::seq`).
//!   * `FormState` — the *user's* edits only (typed text, checked boxes, which
//!     control has focus). Unmodified controls fall back to their attributes,
//!     so a fresh state is exactly the page's defaults.
//!   * `submit` — the successful controls of one form, URL-encoded
//!     (`application/x-www-form-urlencoded`, HTML §4.10.21/22).
//!
//! The shell owns the state and does the navigating; layout only reads state
//! to paint the control. No JS means no `onsubmit`/validation — a GET form is
//! a URL builder, which is all a search box needs.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::dom::{Dom, Element, Node};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlKind {
    Text,
    Password,
    Checkbox,
    Radio,
    Submit,
    Reset,
    Button,
    Hidden,
    TextArea,
    Select,
    File,
}

impl ControlKind {
    /// Does this control hold a text buffer the user types into?
    pub fn is_text(self) -> bool {
        matches!(self, ControlKind::Text | ControlKind::Password | ControlKind::TextArea)
    }
    /// Does activating it submit the form?
    pub fn is_submit(self) -> bool {
        self == ControlKind::Submit
    }
}

pub struct Control {
    pub seq: u32,
    pub kind: ControlKind,
    pub name: String,
    pub default_value: String,
    pub default_checked: bool,
    pub placeholder: String,
    pub disabled: bool,
    pub readonly: bool,
    /// `size` (text) / `cols` (textarea), in characters.
    pub cols: Option<u32>,
    pub rows: Option<u32>,
    /// Owning `<form>` index, if the control is inside one.
    pub form: Option<usize>,
    /// `<select>` choices as (value, label).
    pub options: Vec<(String, String)>,
}

pub struct FormDef {
    pub action: String,
    pub method_get: bool,
}

pub struct Forms {
    pub forms: Vec<FormDef>,
    pub controls: Vec<Control>,
}

impl Forms {
    pub fn get(&self, seq: u32) -> Option<&Control> {
        self.controls.iter().find(|c| c.seq == seq)
    }
    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }
    /// The form's default button — the one an Enter press activates
    /// (HTML §4.10.21.2 implicit submission).
    pub fn default_button(&self, form: usize) -> Option<&Control> {
        self.controls
            .iter()
            .find(|c| c.form == Some(form) && c.kind.is_submit() && !c.disabled)
    }
    /// Text controls in a form, for the "lone text field submits on Enter" rule.
    pub fn text_control_count(&self, form: usize) -> usize {
        self.controls
            .iter()
            .filter(|c| c.form == Some(form) && c.kind.is_text() && !c.disabled)
            .count()
    }
}

/// Is `el` a form control that layout renders as a box?
pub fn kind_of(el: &Element) -> Option<ControlKind> {
    match el.tag.as_str() {
        "input" => {
            let t = el.attr("type").unwrap_or("text").trim().to_ascii_lowercase();
            Some(match t.as_str() {
                "password" => ControlKind::Password,
                "checkbox" => ControlKind::Checkbox,
                "radio" => ControlKind::Radio,
                "submit" | "image" => ControlKind::Submit,
                "reset" => ControlKind::Reset,
                "button" => ControlKind::Button,
                "hidden" => ControlKind::Hidden,
                "file" => ControlKind::File,
                // text, search, email, url, tel, number, date, … all edit text.
                _ => ControlKind::Text,
            })
        }
        "button" => Some(match el.attr("type").unwrap_or("submit").trim().to_ascii_lowercase().as_str() {
            "reset" => ControlKind::Reset,
            "button" => ControlKind::Button,
            _ => ControlKind::Submit, // a <button> defaults to submit
        }),
        "textarea" => Some(ControlKind::TextArea),
        "select" => Some(ControlKind::Select),
        _ => None,
    }
}

/// All text inside an element (used for `<textarea>`/`<button>`/`<option>`).
fn text_of(el: &Element) -> String {
    let mut s = String::new();
    fn walk(el: &Element, s: &mut String) {
        for c in &el.children {
            match c {
                Node::Text(t) => s.push_str(t),
                Node::Element(e) => walk(e, s),
            }
        }
    }
    walk(el, &mut s);
    s
}

fn num_attr(el: &Element, name: &str) -> Option<u32> {
    el.attr(name).and_then(|v| v.trim().parse::<u32>().ok()).filter(|n| *n > 0)
}

pub fn collect(dom: &Dom) -> Forms {
    let mut out = Forms { forms: Vec::new(), controls: Vec::new() };
    walk(&dom.root, None, &mut out);
    out
}

fn walk(el: &Element, form: Option<usize>, out: &mut Forms) {
    for child in &el.children {
        let e = match child {
            Node::Element(e) => e,
            Node::Text(_) => continue,
        };
        // A nested <form> is invalid HTML; treat the inner one as its own
        // (matches how parsers recover) rather than dropping its controls.
        let mut inner = form;
        if e.tag == "form" {
            out.forms.push(FormDef {
                action: e.attr("action").unwrap_or("").trim().to_string(),
                method_get: !e
                    .attr("method")
                    .map(|m| m.trim().eq_ignore_ascii_case("post"))
                    .unwrap_or(false),
            });
            inner = Some(out.forms.len() - 1);
        }
        if let Some(kind) = kind_of(e) {
            out.controls.push(control_of(e, kind, inner));
        }
        walk(e, inner, out);
    }
}

fn control_of(e: &Element, kind: ControlKind, form: Option<usize>) -> Control {
    let mut options = Vec::new();
    let mut default_value = match kind {
        // <textarea>'s content is its default value; a <button>'s label is NOT
        // its value (that's the `value` attribute, empty by default).
        ControlKind::TextArea => text_of(e),
        _ => e.attr("value").unwrap_or("").to_string(),
    };
    if kind == ControlKind::Select {
        let mut selected: Option<String> = None;
        collect_options(e, &mut options, &mut selected);
        default_value = selected
            .or_else(|| options.first().map(|(v, _)| v.clone()))
            .unwrap_or_default();
    }
    if kind.is_submit() && e.tag == "input" && e.attr("value").is_none() {
        // A bare `<input type=submit>` still submits a name=value pair using
        // the UA's default label. Only a MISSING `value` gets it: HTML
        // §4.10.5.1.20 makes `value=""` an explicit empty label, and pages
        // rely on that to put their own icon there by CSS — DDG's search
        // button carries a magnifier that way, and stamping "Absenden" into it
        // both hid the icon and submitted a value the page never asked for.
        default_value = "Absenden".to_string();
    }
    Control {
        seq: e.seq,
        kind,
        name: e.attr("name").unwrap_or("").trim().to_string(),
        default_value,
        default_checked: e.attr("checked").is_some(),
        placeholder: e.attr("placeholder").unwrap_or("").to_string(),
        disabled: e.attr("disabled").is_some(),
        readonly: e.attr("readonly").is_some(),
        cols: num_attr(e, "size").or_else(|| num_attr(e, "cols")),
        rows: num_attr(e, "rows"),
        form,
        options,
    }
}

/// A `<select>`'s options as (value, label) plus the pre-selected value, if
/// any. Layout needs the same view to paint the closed box.
pub fn options_of(el: &Element) -> (Vec<(String, String)>, Option<String>) {
    let mut opts = Vec::new();
    let mut sel = None;
    collect_options(el, &mut opts, &mut sel);
    (opts, sel)
}

fn collect_options(el: &Element, out: &mut Vec<(String, String)>, selected: &mut Option<String>) {
    for c in &el.children {
        if let Node::Element(e) = c {
            if e.tag == "option" {
                let label = text_of(e).trim().to_string();
                let value = e.attr("value").map(|v| v.to_string()).unwrap_or_else(|| label.clone());
                if e.attr("selected").is_some() {
                    *selected = Some(value.clone());
                }
                out.push((value, label));
            } else {
                collect_options(e, out, selected); // <optgroup>
            }
        }
    }
}

/// The user's edits to a document's controls. Anything untouched falls back to
/// the parsed defaults, so `FormState::default()` renders the page as authored.
#[derive(Default)]
pub struct FormState {
    values: BTreeMap<u32, String>,
    checked: BTreeMap<u32, bool>,
    /// Control that has keyboard focus (element `seq`).
    pub focus: Option<u32>,
    /// Caret position in the focused control's value, as a byte offset.
    pub caret: usize,
}

impl FormState {
    /// Current value of the control at `seq`, falling back to `default`.
    pub fn value_or<'a>(&'a self, seq: u32, default: &'a str) -> &'a str {
        self.values.get(&seq).map(|s| s.as_str()).unwrap_or(default)
    }
    pub fn checked_or(&self, seq: u32, default: bool) -> bool {
        self.checked.get(&seq).copied().unwrap_or(default)
    }
    pub fn value<'a>(&'a self, c: &'a Control) -> &'a str {
        self.value_or(c.seq, &c.default_value)
    }
    pub fn is_checked(&self, c: &Control) -> bool {
        self.checked_or(c.seq, c.default_checked)
    }
    pub fn set_value(&mut self, seq: u32, v: String) {
        self.values.insert(seq, v);
    }
    /// Toggle a checkbox, or select one radio out of its name-group.
    pub fn toggle(&mut self, forms: &Forms, seq: u32) {
        let c = match forms.get(seq) {
            Some(c) => c,
            None => return,
        };
        if c.kind == ControlKind::Radio {
            for other in forms.controls.iter().filter(|o| {
                o.kind == ControlKind::Radio && o.form == c.form && o.name == c.name
            }) {
                self.checked.insert(other.seq, other.seq == seq);
            }
        } else {
            let now = self.is_checked(c);
            self.checked.insert(seq, !now);
        }
    }
    /// Advance a `<select>` to its next option (we have no dropdown popover
    /// inside the canvas yet — clicking cycles, which is enough to pick).
    pub fn cycle_select(&mut self, forms: &Forms, seq: u32) {
        let c = match forms.get(seq) {
            Some(c) if c.kind == ControlKind::Select && !c.options.is_empty() => c,
            _ => return,
        };
        let cur = self.value(c);
        let i = c.options.iter().position(|(v, _)| v == cur).map(|i| i + 1).unwrap_or(0);
        let (v, _) = &c.options[i % c.options.len()];
        self.values.insert(seq, v.clone());
    }
    /// Drop every edit (a navigation loads a different document).
    pub fn reset(&mut self) {
        self.values.clear();
        self.checked.clear();
        self.focus = None;
        self.caret = 0;
    }
}

pub struct Submission {
    /// The form's `action`, as authored (the shell resolves it against the
    /// page URL — the engine has no notion of a base URL).
    pub action: String,
    pub method_get: bool,
    /// `a=1&b=2`, percent-encoded.
    pub query: String,
}

/// Build the submission for the form owning `activated` (a submit button) or,
/// with none, the form owning the focused control. Returns `None` if there is
/// no owning form.
pub fn submit(forms: &Forms, state: &FormState, activated: Option<u32>) -> Option<Submission> {
    let origin = activated.or(state.focus)?;
    let form = forms.get(origin)?.form?;
    let def = forms.forms.get(form)?;

    let mut query = String::new();
    for c in forms.controls.iter().filter(|c| c.form == Some(form)) {
        // "Successful control" (HTML §4.10.22.4): named, enabled, and — for
        // buttons — only the one that was actually activated.
        if c.name.is_empty() || c.disabled {
            continue;
        }
        match c.kind {
            ControlKind::Reset | ControlKind::Button | ControlKind::File => continue,
            ControlKind::Submit if Some(c.seq) != activated => continue,
            ControlKind::Checkbox | ControlKind::Radio if !state.is_checked(c) => continue,
            _ => {}
        }
        let value = match c.kind {
            // A checked box with no value submits "on".
            ControlKind::Checkbox | ControlKind::Radio if state.value(c).is_empty() => "on",
            _ => state.value(c),
        };
        if !query.is_empty() {
            query.push('&');
        }
        encode_query_value(&c.name, &mut query);
        query.push('=');
        encode_query_value(value, &mut query);
    }
    Some(Submission { action: def.action.clone(), method_get: def.method_get, query })
}

/// `application/x-www-form-urlencoded` (HTML §4.10.21.6): space → `+`, the
/// unreserved set verbatim, everything else percent-encoded per UTF-8 byte.
/// Public because the shell encodes omnibox search terms the same way.
pub fn encode_query_value(s: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 0xF) as usize] as char);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom;

    fn forms_of(html: &str) -> Forms {
        collect(&dom::parse(html))
    }

    #[test]
    fn collects_a_search_form() {
        let f = forms_of(
            "<body><form action=\"/search\"><input name=q value=hi size=30>\
             <input type=submit name=go value=Go></form></body>",
        );
        assert_eq!(f.forms.len(), 1);
        assert_eq!(f.forms[0].action, "/search");
        assert!(f.forms[0].method_get);
        assert_eq!(f.controls.len(), 2);
        assert_eq!(f.controls[0].kind, ControlKind::Text);
        assert_eq!(f.controls[0].name, "q");
        assert_eq!(f.controls[0].default_value, "hi");
        assert_eq!(f.controls[0].cols, Some(30));
        assert!(f.controls[1].kind.is_submit());
        // Distinct, stable identities from the DOM.
        assert_ne!(f.controls[0].seq, f.controls[1].seq);
    }

    #[test]
    fn submit_builds_an_encoded_query() {
        let f = forms_of(
            "<body><form action=\"/search\"><input name=q value=\"hello world & more\">\
             <input type=hidden name=lang value=de>\
             <input type=submit name=go value=Go><input type=submit name=other value=X>\
             </form></body>",
        );
        let mut st = FormState::default();
        st.focus = Some(f.controls[0].seq);
        let go = f.controls.iter().find(|c| c.name == "go").unwrap();
        let s = submit(&f, &st, Some(go.seq)).unwrap();
        // Only the activated button is successful; hidden fields are.
        assert_eq!(s.query, "q=hello+world+%26+more&lang=de&go=Go");
        assert!(s.method_get);
    }

    #[test]
    fn typed_value_overrides_the_default() {
        let f = forms_of("<body><form action=/s><input name=q value=old></form></body>");
        let seq = f.controls[0].seq;
        let mut st = FormState::default();
        assert_eq!(st.value(&f.controls[0]), "old");
        st.set_value(seq, "neu".to_string());
        st.focus = Some(seq);
        assert_eq!(st.value(&f.controls[0]), "neu");
        assert_eq!(submit(&f, &st, None).unwrap().query, "q=neu");
    }

    #[test]
    fn checkboxes_radios_and_selects() {
        let f = forms_of(
            "<body><form action=/s>\
             <input type=checkbox name=a><input type=checkbox name=b checked>\
             <input type=radio name=r value=1><input type=radio name=r value=2 checked>\
             <select name=s><option value=x>X<option value=y selected>Y</select>\
             </form></body>",
        );
        let mut st = FormState::default();
        st.focus = Some(f.controls[0].seq);
        // Defaults: only the checked box + checked radio + selected option.
        assert_eq!(submit(&f, &st, None).unwrap().query, "b=on&r=2&s=y");
        // Toggle the first box on, move the radio to the first choice.
        st.toggle(&f, f.controls[0].seq);
        st.toggle(&f, f.controls[2].seq);
        st.cycle_select(&f, f.controls[4].seq);
        assert_eq!(submit(&f, &st, None).unwrap().query, "a=on&b=on&r=1&s=x");
    }

    #[test]
    fn post_method_and_textarea_content() {
        let f = forms_of(
            "<body><form action=/p method=POST><textarea name=t>hallo</textarea>\
             <button name=send value=1>Senden</button></form></body>",
        );
        assert!(!f.forms[0].method_get);
        assert_eq!(f.controls[0].kind, ControlKind::TextArea);
        assert_eq!(f.controls[0].default_value, "hallo");
        assert!(f.controls[1].kind.is_submit()); // <button> defaults to submit
        let st = FormState::default();
        let s = submit(&f, &st, Some(f.controls[1].seq)).unwrap();
        assert_eq!(s.query, "t=hallo&send=1");
        assert!(!s.method_get);
    }

    #[test]
    fn real_world_search_forms() {
        // Markup as shipped by the two search boxes we actually target. Both
        // carry hidden fields that must ride along, and a submit button with
        // no `name` (not a successful control).
        let marginalia = "<body><form id=\"search-form\" action=\"/search\" method=\"get\">\
             <input type=\"hidden\" name=\"profile\" value=\"corpo\">\
             <input type=\"text\" value=\"\" placeholder=\"Search the web!\" name=\"query\" id=\"searchInput\" />\
             <button class=\"px-4 py-2\">Search</button></form></body>";
        let f = forms_of(marginalia);
        let q = f.controls.iter().find(|c| c.name == "query").unwrap();
        let mut st = FormState::default();
        st.set_value(q.seq, "nopeekos wasm".to_string());
        st.focus = Some(q.seq);
        // Enter in the field activates the form's default button.
        let btn = f.default_button(q.form.unwrap()).unwrap();
        let s = submit(&f, &st, Some(btn.seq)).unwrap();
        assert_eq!(s.action, "/search");
        assert_eq!(s.query, "profile=corpo&query=nopeekos+wasm");

        let wikipedia = "<body><form action=\"/w/index.php\" id=\"searchform\">\
             <input type=\"search\" name=\"search\" placeholder=\"Wikipedia durchsuchen\">\
             <input type=\"hidden\" name=\"title\" value=\"Spezial:Suche\">\
             <input type=\"submit\" name=\"fulltext\" value=\"Suchen\"></form></body>";
        let f = forms_of(wikipedia);
        let s0 = f.controls.iter().find(|c| c.name == "search").unwrap();
        let mut st = FormState::default();
        st.set_value(s0.seq, "Stansstad".to_string());
        st.focus = Some(s0.seq);
        let btn = f.default_button(s0.form.unwrap()).unwrap();
        let s = submit(&f, &st, Some(btn.seq)).unwrap();
        assert_eq!(s.query, "search=Stansstad&title=Spezial%3ASuche&fulltext=Suchen");
    }

    #[test]
    fn control_outside_a_form_has_no_submission() {
        let f = forms_of("<body><input name=q></body>");
        let mut st = FormState::default();
        st.focus = Some(f.controls[0].seq);
        assert!(submit(&f, &st, None).is_none());
    }
}
