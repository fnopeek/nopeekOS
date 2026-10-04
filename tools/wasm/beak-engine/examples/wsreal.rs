// Runs the WebSocket framing code against a real server, host-side.
//
// This file computes the frames with the same code beak runs; the TLS link
// lives outside (`tools/wsdrive.py`) because the engine has no `std` and
// cannot open a stream. Line protocol on stdin:
//
//     RX <hex>     feed bytes from the wire
//     SEND <text>  send a text frame
//
// Replies are `EV <event>`, `TX <hex>` and `OK`.
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
    // Fixed randomness so runs are repeatable. Acceptable for a probe; real
    // use needs true randomness for the mask (RFC 6455 §5.3).
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
