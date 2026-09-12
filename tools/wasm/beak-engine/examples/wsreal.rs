// Der WebSocket-Code gegen einen ECHTEN Server — host-seitig.
//
// Die Rahmen rechnet DIESE Datei, also derselbe Code, den beak faehrt; die
// TLS-Leitung liegt draussen (`tools/wsdrive.py`), weil der Motor kein `std`
// kennt und keinen Strom aufmachen kann. Zeilenprotokoll auf stdin:
//
//     RX <hex>     Bytes von der Leitung hereingeben
//     SEND <text>  einen Textrahmen hinauslegen
//
// Geantwortet wird mit `EV <ereignis>`, `TX <hex>` und `OK`.
use beak_engine::js::websocket::{Event, Socket};
use std::io::{BufRead, Write};

fn hex_to(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()).collect()
}
fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    let url = std::env::args().nth(1).expect("URL");
    let origin = std::env::args().nth(2).expect("Herkunft");
    // Fester Zufall: der Lauf soll wiederholbar sein. Fuer die Maske ist das
    // hier richtig und auf dem Geraet falsch — §5.3 verlangt dort echten.
    let mut sock = Socket::new(1, &url, &origin, true, [0x5a; 16]).expect("URL zerlegen");
    let mut mask = || [0xa1u8, 0xb2, 0xc3, 0xd4];
    let out = std::io::stdout();
    let mut o = out.lock();
    writeln!(o, "TX {}", to_hex(&sock.handshake())).unwrap();
    writeln!(o, "OK").unwrap();
    o.flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let (cmd, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        match cmd {
            "RX" => {
                for ev in sock.feed(&hex_to(rest), &mut mask) {
                    let s = match &ev {
                        Event::Open => String::from("OPEN"),
                        Event::Text(t) => format!("TEXT({}) {:.180}", t.len(), t),
                        Event::Binary(d) => format!("BINARY({})", d.len()),
                        Event::Closed(c, r) => format!("CLOSED {c} {r}"),
                        Event::Error(e) => format!("ERROR {e}"),
                    };
                    writeln!(o, "EV {s}").unwrap();
                }
            }
            "SEND" => { sock.send_text(rest, &mut mask); }
            _ => {}
        }
        if sock.has_out() { writeln!(o, "TX {}", to_hex(&sock.take_out())).unwrap(); }
        writeln!(o, "OK").unwrap();
        o.flush().unwrap();
    }
}
