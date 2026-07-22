// Generated from RFC 7541 Appendix C. Do not hand-edit; regenerate.
use crate::hpack::{Decoder, Header};

fn hx(s: &str) -> Vec<u8> {
    (0..s.len()/2).map(|i| u8::from_str_radix(&s[i*2..i*2+2], 16).unwrap()).collect()
}

fn h(n: &str, v: &str) -> Header { Header { name: n.into(), value: v.into() } }

#[test]
fn rfc7541_c_2() {
    // Each C.2 example starts from a fresh decoder.
    let mut d = Decoder::with_max(4096);
    // C.2.1 Literal Header Field with Indexing
    let got = d.decode(&hx("400a637573746f6d2d6b65790d637573746f6d2d686561646572")).expect("C.2.1 decode");
    assert_eq!(got, vec![h("custom-key", "custom-header")], "C.2.1");
    let mut d = Decoder::with_max(4096);
    // C.2.2 Literal Header Field without Indexing
    let got = d.decode(&hx("040c2f73616d706c652f70617468")).expect("C.2.2 decode");
    assert_eq!(got, vec![h(":path", "/sample/path")], "C.2.2");
    let mut d = Decoder::with_max(4096);
    // C.2.3 Literal Header Field Never Indexed
    let got = d.decode(&hx("100870617373776f726406736563726574")).expect("C.2.3 decode");
    assert_eq!(got, vec![h("password", "secret")], "C.2.3");
    let mut d = Decoder::with_max(4096);
    // C.2.4 Indexed Header Field
    let got = d.decode(&hx("82")).expect("C.2.4 decode");
    assert_eq!(got, vec![h(":method", "GET")], "C.2.4");
}

#[test]
fn rfc7541_c_3() {
    // Consecutive header lists on ONE connection (table size 4096).
    let mut d = Decoder::with_max(4096);
    // C.3.1 First Request
    let got = d.decode(&hx("828684410f7777772e6578616d706c652e636f6d")).expect("C.3.1 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com")], "C.3.1");
    // C.3.2 Second Request
    let got = d.decode(&hx("828684be58086e6f2d6361636865")).expect("C.3.2 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com"), h("cache-control", "no-cache")], "C.3.2");
    // C.3.3 Third Request
    let got = d.decode(&hx("828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565")).expect("C.3.3 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "https"), h(":path", "/index.html"), h(":authority", "www.example.com"), h("custom-key", "custom-value")], "C.3.3");
}

#[test]
fn rfc7541_c_4() {
    // Consecutive header lists on ONE connection (table size 4096).
    let mut d = Decoder::with_max(4096);
    // C.4.1 First Request
    let got = d.decode(&hx("828684418cf1e3c2e5f23a6ba0ab90f4ff")).expect("C.4.1 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com")], "C.4.1");
    // C.4.2 Second Request
    let got = d.decode(&hx("828684be5886a8eb10649cbf")).expect("C.4.2 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com"), h("cache-control", "no-cache")], "C.4.2");
    // C.4.3 Third Request
    let got = d.decode(&hx("828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf")).expect("C.4.3 decode");
    assert_eq!(got, vec![h(":method", "GET"), h(":scheme", "https"), h(":path", "/index.html"), h(":authority", "www.example.com"), h("custom-key", "custom-value")], "C.4.3");
}

#[test]
fn rfc7541_c_5() {
    // Consecutive header lists on ONE connection (table size 256).
    let mut d = Decoder::with_max(256);
    // C.5.1 First Response
    let got = d.decode(&hx("4803333032580770726976617465611d4d6f6e2c203231204f637420323031332032303a31333a323120474d546e1768747470733a2f2f7777772e6578616d706c652e636f6d")).expect("C.5.1 decode");
    assert_eq!(got, vec![h(":status", "302"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:21 GMT"), h("location", "https://www.example.com")], "C.5.1");
    // C.5.2 Second Response
    let got = d.decode(&hx("4803333037c1c0bf")).expect("C.5.2 decode");
    assert_eq!(got, vec![h(":status", "307"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:21 GMT"), h("location", "https://www.example.com")], "C.5.2");
    // C.5.3 Third Response
    let got = d.decode(&hx("88c1611d4d6f6e2c203231204f637420323031332032303a31333a323220474d54c05a04677a69707738666f6f3d4153444a4b48514b425a584f5157454f50495541585157454f49553b206d61782d6167653d333630303b2076657273696f6e3d31")).expect("C.5.3 decode");
    assert_eq!(got, vec![h(":status", "200"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:22 GMT"), h("location", "https://www.example.com"), h("content-encoding", "gzip"), h("set-cookie", "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1")], "C.5.3");
}

#[test]
fn rfc7541_c_6() {
    // Consecutive header lists on ONE connection (table size 256).
    let mut d = Decoder::with_max(256);
    // C.6.1 First Response
    let got = d.decode(&hx("488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8e9ae82ae43d3")).expect("C.6.1 decode");
    assert_eq!(got, vec![h(":status", "302"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:21 GMT"), h("location", "https://www.example.com")], "C.6.1");
    // C.6.2 Second Response
    let got = d.decode(&hx("4883640effc1c0bf")).expect("C.6.2 decode");
    assert_eq!(got, vec![h(":status", "307"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:21 GMT"), h("location", "https://www.example.com")], "C.6.2");
    // C.6.3 Third Response
    let got = d.decode(&hx("88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b3960d5af27087f3672c1ab270fb5291f9587316065c003ed4ee5b1063d5007")).expect("C.6.3 decode");
    assert_eq!(got, vec![h(":status", "200"), h("cache-control", "private"), h("date", "Mon, 21 Oct 2013 20:13:22 GMT"), h("location", "https://www.example.com"), h("content-encoding", "gzip"), h("set-cookie", "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1")], "C.6.3");
}

