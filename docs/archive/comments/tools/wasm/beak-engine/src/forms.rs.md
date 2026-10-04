# `tools/wasm/beak-engine/src/forms.rs` @ 5e0102684

## L1-14 · `use alloc::collections::BTreeMap;`

```
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
```

## L38 · `pub fn is_text(self) -> bool {`

```
/// Does this control hold a text buffer the user types into?
```

## L42 · `pub fn is_submit(self) -> bool {`

```
/// Does activating it submit the form?
```

## L57 · `pub cols: Option<u32>,`

```
/// `size` (text) / `cols` (textarea), in characters.
```

## L60 · `pub form: Option<usize>,`

```
/// Owning `<form>` index, if the control is inside one.
```

## L62 · `pub options: Vec<(String, String)>,`

```
/// `<select>` choices as (value, label).
```

## L64-71 · `pub label: String,`

```
/// Was auf dem Bedienelement STEHT — der Text eines `<button>`, das
/// `value` eines `<input type=submit>`.
///
/// Getrennt von `default_value`, weil es etwas anderes ist: der Text
/// eines `<button>` ist NICHT sein Wert (HTML §4.10.6). Gebraucht wird er
/// zum Hinsehen — Googles Einwilligungsseite hat vier Formulare auf
/// dieselbe Adresse, und ohne die Beschriftung sind „Alle ablehnen" und
/// „Alle akzeptieren" im Log nicht zu unterscheiden.
```

## L78-80 · `pub seq: u32,`

```
/// Die `seq` des `<form>` selbst. Ein Skript schickt ueber `form.submit()`
/// ein ELEMENT ab, nicht einen Knopf — ohne diese Zahl ist das Formular
/// aus dem Baum nicht wiederzufinden.
```

## L96-97 · `pub fn default_button(&self, form: usize) -> Option<&Control> {`

```
/// The form's default button — the one an Enter press activates
/// (HTML §4.10.21.2 implicit submission).
```

## L103 · `pub fn text_control_count(&self, form: usize) -> usize {`

```
/// Text controls in a form, for the "lone text field submits on Enter" rule.
```

## L112 · `pub fn kind_of(el: &Element) -> Option<ControlKind> {`

```
/// Is `el` a form control that layout renders as a box?
```

## L126 · `_ => ControlKind::Text,`

```
// text, search, email, url, tel, number, date, … all edit text.
```

## L133 · `_ => ControlKind::Submit, // a <button> defaults to submit`

```
// a <button> defaults to submit
```

## L141 · `fn text_of(el: &Element) -> String {`

```
/// All text inside an element (used for `<textarea>`/`<button>`/`<option>`).
```

## L172-173 · `let mut inner = form;`

```
// A nested <form> is invalid HTML; treat the inner one as its own
// (matches how parsers recover) rather than dropping its controls.
```

## L196-197 · `ControlKind::TextArea => text_of(e),`

```
// <textarea>'s content is its default value; a <button>'s label is NOT
// its value (that's the `value` attribute, empty by default).
```

## L209-214 · `default_value = "Absenden".to_string();`

```
// A bare `<input type=submit>` still submits a name=value pair using
// the UA's default label. Only a MISSING `value` gets it: HTML
// §4.10.5.1.20 makes `value=""` an explicit empty label, and pages
// rely on that to put their own icon there by CSS — DDG's search
// button carries a magnifier that way, and stamping "Absenden" into it
// both hid the icon and submitted a value the page never asked for.
```

## L217 · `let mut label = if e.tag == "button" { text_of(e) } else { default_value.clone() };`

```
// Die Beschriftung: bei `<button>` der Text, bei `<input>` sein `value`.
```

## L238-239 · `pub fn options_of(el: &Element) -> (Vec<(String, String)>, Option<String>) {`

```
/// A `<select>`'s options as (value, label) plus the pre-selected value, if
/// any. Layout needs the same view to paint the closed box.
```

## L258 · `collect_options(e, out, selected); // <optgroup>`

```
// <optgroup>
```

## L264-265 · `#[derive(Default, Clone)]`

```
/// The user's edits to a document's controls. Anything untouched falls back to
/// the parsed defaults, so `FormState::default()` renders the page as authored.
```

## L270 · `pub focus: Option<u32>,`

```
/// Control that has keyboard focus (element `seq`).
```

## L272 · `pub caret: usize,`

```
/// Caret position in the focused control's value, as a byte offset.
```

## L274-277 · `pub pending: [u8; 4],`

```
/// Angefangene UTF-8-Folge. **Ein Tastendruck traegt ein BYTE, und `ä`
/// sind zwei** — das erste allein ist noch kein Zeichen und darf nicht in
/// den Wert, sonst stuende dort kein gueltiges UTF-8. Fuer ASCII bleibt
/// der Puffer immer leer.
```

## L283 · `pub fn value_or<'a>(&'a self, seq: u32, default: &'a str) -> &'a str {`

```
/// Current value of the control at `seq`, falling back to `default`.
```

## L287-290 · `pub fn value_set(&self, seq: u32) -> Option<&str> {`

```
/// Der Wert, den der Benutzer gesetzt hat — oder `None`, wenn er das Feld
/// nie angefasst hat. `value_or` kann das nicht sagen: dort ist „nie
/// getippt" von „leer getippt" nicht zu unterscheiden, und ein Neumalen
/// muss den Unterschied kennen (leer heisst Platzhalter).
```

## L294-295 · `pub fn checked_set(&self, seq: u32) -> Option<bool> {`

```
/// Wurde dieses Kaestchen ueberhaupt angefasst? `None` heisst: der
/// Vorgabewert aus dem Attribut gilt noch.
```

## L311-312 · `pub fn set_checked(&mut self, seq: u32, on: bool) {`

```
/// Ein Haekchen direkt setzen. Der Weg fuer ein Skript und fuer eine
/// Probe; der Benutzerweg ist `toggle`, der die Radiogruppe mitfuehrt.
```

## L316 · `pub fn toggle(&mut self, forms: &Forms, seq: u32) {`

```
/// Toggle a checkbox, or select one radio out of its name-group.
```

## L333-334 · `pub fn cycle_select(&mut self, forms: &Forms, seq: u32) {`

```
/// Advance a `<select>` to its next option (we have no dropdown popover
/// inside the canvas yet — clicking cycles, which is enough to pick).
```

## L345 · `pub fn reset(&mut self) {`

```
/// Drop every edit (a navigation loads a different document).
```

## L355-356 · `pub action: String,`

```
/// The form's `action`, as authored (the shell resolves it against the
/// page URL — the engine has no notion of a base URL).
```

## L359 · `pub query: String,`

```
/// `a=1&b=2`, percent-encoded.
```

## L363-365 · `pub fn submit(forms: &Forms, state: &FormState, activated: Option<u32>) -> Option<Submission> {`

```
/// Build the submission for the form owning `activated` (a submit button) or,
/// with none, the form owning the focused control. Returns `None` if there is
/// no owning form.
```

## L372-375 · `pub fn submit_form(forms: &Forms, state: &FormState, form_seq: u32) -> Option<Submission> {`

```
/// Wie `submit`, aber fuer ein `<form>`-ELEMENT — das ist der Weg, den ein
/// Skript nimmt (`form.submit()`). Kein Knopf ist dabei aktiviert, also
/// traegt auch keiner seinen Namen bei; genau das schreibt die Spezifikation
/// fuer den skriptgesteuerten Fall vor.
```

## L386-387 · `if c.name.is_empty() || c.disabled {`

```
// "Successful control" (HTML §4.10.22.4): named, enabled, and — for
// buttons — only the one that was actually activated.
```

## L398 · `ControlKind::Checkbox | ControlKind::Radio if state.value(c).is_empty() => "on",`

```
// A checked box with no value submits "on".
```

## L412-414 · `pub fn encode_query_value(s: &str, out: &mut String) {`

```
/// `application/x-www-form-urlencoded` (HTML §4.10.21.6): space → `+`, the
/// unreserved set verbatim, everything else percent-encoded per UTF-8 byte.
/// Public because the shell encodes omnibox search terms the same way.
```

## L456 · `assert_ne!(f.controls[0].seq, f.controls[1].seq);`

```
// Distinct, stable identities from the DOM.
```

## L472 · `assert_eq!(s.query, "q=hello+world+%26+more&lang=de&go=Go");`

```
// Only the activated button is successful; hidden fields are.
```

## L500 · `assert_eq!(submit(&f, &st, None).unwrap().query, "b=on&r=2&s=y");`

```
// Defaults: only the checked box + checked radio + selected option.
```

## L502 · `st.toggle(&f, f.controls[0].seq);`

```
// Toggle the first box on, move the radio to the first choice.
```

## L518 · `assert!(f.controls[1].kind.is_submit()); // <button> defaults to submit`

```
// <button> defaults to submit
```

## L527-529 · `let marginalia = "<body><form id=\"search-form\" action=\"/search\" method=\"get\">\`

```
// Markup as shipped by the two search boxes we actually target. Both
// carry hidden fields that must ride along, and a submit button with
// no `name` (not a successful control).
```

## L539 · `let btn = f.default_button(q.form.unwrap()).unwrap();`

```
// Enter in the field activates the form's default button.
```

## L567-575 · `#[test]`

```
/// **Die Beschriftung ist nicht der Wert.** Der Text eines `<button>`
/// ist laut HTML §4.10.6 nicht sein `value`, und `collect` speichert ihn
/// deshalb getrennt.
///
/// Gebraucht wird er zum Hinsehen: Googles Einwilligungsseite traegt vier
/// Formulare auf dieselbe Adresse, und im Log sahen sie alle gleich aus —
/// „Alle ablehnen" und „Alle akzeptieren" waren nicht zu unterscheiden.
/// Wer da blind das erste nimmt, wirft eine Muenze ueber eine
/// Entscheidung des Benutzers.
```

## L587 · `("Alle ablehnen", "0"),`

```
// Text steht in `label`, das `value` bleibt das `value`.
```

## L589 · `("Alle akzeptieren", ""),`

```
// Mehrfache Leerzeichen werden zusammengezogen.
```

## L591 · `("Los", "Los"),`

```
// Bei `<input>` ist die Beschriftung sein `value` …
```

## L593 · `("Absenden", "Absenden"),`

```
// … und ohne `value` die Vorgabe des Wirts.
```

