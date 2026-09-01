// Trennen die beiden Fragen wirklich? `hit_all` darf die TREFFER-Kaesten
// mehren, ohne dass ein Element dadurch `:hover`-faehig wird — sonst gilt
// jede Mausbewegung als Stilwechsel und kostet ein volles Layout.
fn main() {
    use beak_engine::{Engine, Rgb, Theme};
    let theme = Theme { bg: Rgb(255,255,255), text: Rgb(0,0,0), heading: Rgb(0,0,0),
                        link: Rgb(0,0,238), muted: Rgb(96,96,96), rule: Rgb(128,128,128) };
    // EIN Element mit :hover-Regel, eins ohne.
    let html = r##"<html><head><style>
        .h:hover { color: red }
        div { width: 300px; height: 20px }
      </style></head><body><div class="h">mit</div><div class="n">ohne</div></body></html>"##;
    let mut e = Engine::new();
    e.set_theme(theme);
    for hit_all in [false, true] {
        e.set_hit_all(hit_all);
        let l = e.layout_forms(html, "", 400, &Default::default());
        // Punkt im ZWEITEN div (ohne :hover-Regel).
        let (x, y) = (10, 30);
        println!("  hit_all={hit_all:5}  hover_at={:?}  element_chain={:?}",
            l.hover_at(x, y), l.element_chain(x, y));
    }
    println!("\n  hover_at darf sich NICHT aendern, element_chain schon.");
}
