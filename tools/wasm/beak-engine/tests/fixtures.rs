//! Jede Klasse in einer Vorlage muss im Blatt auch eine Regel HABEN.
//!
//! Der Grund ist eine Stunde Fehlersuche: die Tailwind-Vorlage benutzte
//! `left-2`, das vendorierte Blatt kennt aber nur `top-*`/`bottom-*`/`inset-*`
//! — Tailwind v4 gibt nur aus, was seine Quelle wirklich benutzt, und diese
//! Quelle war eine andere. Der Kasten stand deshalb bei x=0, und das sah
//! haargenau nach einem Fehler in unserem `position:absolute` aus. Er war
//! keiner: das Blatt sagte nichts, also tat die Maschine nichts.
//!
//! Eine Vorlage, die eine Klasse ohne Regel zeigt, ist ein Orakel, das luegt —
//! und zwar in die teure Richtung: sie meldet einen Fehler, den es nicht gibt.
//! Diese Probe haelt Vorlage und Blatt zusammen, damit das nicht ein zweites
//! Mal eine Runde kostet.

/// Selektor-Text einer Klasse, so wie ein Blatt ihn schreibt: `:` und `/` und
/// `.` werden in Tailwind mit Backslash geschuetzt.
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

/// Steht `sel` im Blatt — und endet dort auch, statt der Anfang eines
/// laengeren Namens zu sein (`.p-1` darf nicht auf `.p-10` passen)?
fn defined(css: &str, sel: &str) -> bool {
    let mut from = 0;
    while let Some(i) = css[from..].find(sel) {
        let at = from + i;
        let after = css[at + sel.len()..].chars().next().unwrap_or(' ');
        let ok_after = !matches!(after, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '\\');
        // Ein Selektor faengt nicht mitten in einem Namen an: das Zeichen davor
        // darf kein Namenszeichen sein (sonst passt `.p-1` auf `.grid-cols-1`).
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
    // Der eigene <style>-Block der Vorlage zaehlt mit: er definiert den Rahmen
    // um jeden Block, und der gehoert nicht ins fremde Blatt.
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
