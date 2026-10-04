//! Every class used in a fixture must have a rule in the stylesheet.
//!
//! Tailwind v4 emits only the classes its source uses, so a fixture can use a
//! class the vendored stylesheet lacks. The box then sits wrong and looks
//! exactly like an engine bug. A fixture with a rule-less class is an oracle
//! that reports bugs that do not exist; this test keeps fixture and
//! stylesheet together.

/// Selector text of a class as a stylesheet writes it: Tailwind escapes `:`,
/// `/` and `.` with a backslash.
fn selector(class: &str) -> String {
    let mut out = String::from(".");
    for c in class.chars() {
        if matches!(c, ':' | '/' | '.' | '[' | ']' | '(' | ')' | '%' | '!' | '#') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `sel` occurs in the stylesheet and ends there, rather than being
/// the prefix of a longer name (`.p-1` must not match `.p-10`).
fn defined(css: &str, sel: &str) -> bool {
    let mut from = 0;
    while let Some(i) = css[from..].find(sel) {
        let at = from + i;
        let after = css[at + sel.len()..].chars().next().unwrap_or(' ');
        let ok_after = !matches!(after, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '\\');
        // A selector does not start mid-name: the preceding character must
        // not be a name character (or `.p-1` matches `.grid-cols-1`).
        let before = css[..at].chars().next_back().unwrap_or(' ');
        let ok_before = !matches!(before, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_');
        if ok_after && ok_before {
            return true;
        }
        from = at + 1;
    }
    false
}

fn classes(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("class=\"") {
        rest = &rest[i + 7..];
        let Some(j) = rest.find('"') else { break };
        for c in rest[..j].split_whitespace() {
            if !out.iter().any(|x| x == c) {
                out.push(c.to_string());
            }
        }
        rest = &rest[j + 1..];
    }
    out
}

fn check(name: &str, html: &str, sheet: &str) {
    // The fixture's own <style> block counts too: it defines the frame around
    // each block, which is not part of the vendored stylesheet.
    let own = html.split("<style>").nth(1).and_then(|s| s.split("</style>").next()).unwrap_or("");
    let missing: Vec<String> = classes(html)
        .into_iter()
        .filter(|c| !defined(sheet, &selector(c)) && !defined(own, &selector(c)))
        .collect();
    assert!(
        missing.is_empty(),
        "{name}: {} Klasse(n) ohne Regel im Blatt — die Vorlage zeigt einen Fall, \
         den das Blatt gar nicht beschreibt: {missing:?}",
        missing.len()
    );
}

#[test]
fn every_class_in_the_tailwind_fixture_has_a_rule() {
    check(
        "tailwind.html",
        include_str!("../../../fixtures/tailwind.html"),
        include_str!("../assets/tailwind.css"),
    );
}

#[test]
fn every_class_in_the_bootstrap_fixture_has_a_rule() {
    check(
        "components.html",
        include_str!("../../../fixtures/components.html"),
        include_str!("../assets/bootstrap.min.css"),
    );
}
