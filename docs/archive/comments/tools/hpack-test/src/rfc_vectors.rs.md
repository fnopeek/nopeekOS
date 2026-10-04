# `tools/hpack-test/src/rfc_vectors.rs` @ 5e0102684

## L1 · `use crate::hpack::{Decoder, Header};`

```
// Generated from RFC 7541 Appendix C. Do not hand-edit; regenerate.
```

## L12 · `let mut d = Decoder::with_max(4096);`

```
// Each C.2 example starts from a fresh decoder.
```

## L14 · `let got = d.decode(&hx("400a637573746f6d2d6b65790d637573746f6d2d686561646572")).expect("C.2.1 decode");`

```
// C.2.1 Literal Header Field with Indexing
```

## L18 · `let got = d.decode(&hx("040c2f73616d706c652f70617468")).expect("C.2.2 decode");`

```
// C.2.2 Literal Header Field without Indexing
```

## L22 · `let got = d.decode(&hx("100870617373776f726406736563726574")).expect("C.2.3 decode");`

```
// C.2.3 Literal Header Field Never Indexed
```

## L26 · `let got = d.decode(&hx("82")).expect("C.2.4 decode");`

```
// C.2.4 Indexed Header Field
```

## L33 · `let mut d = Decoder::with_max(4096);`

```
// Consecutive header lists on ONE connection (table size 4096).
```

## L35 · `let got = d.decode(&hx("828684410f7777772e6578616d706c652e636f6d")).expect("C.3.1 decode");`

```
// C.3.1 First Request
```

## L38 · `let got = d.decode(&hx("828684be58086e6f2d6361636865")).expect("C.3.2 decode");`

```
// C.3.2 Second Request
```

## L41 · `let got = d.decode(&hx("828785bf400a637573746f6d2d6b65790c637573746f6d2d76616c7565")).expect("C.3.3 decode");`

```
// C.3.3 Third Request
```

## L48 · `let mut d = Decoder::with_max(4096);`

```
// Consecutive header lists on ONE connection (table size 4096).
```

## L50 · `let got = d.decode(&hx("828684418cf1e3c2e5f23a6ba0ab90f4ff")).expect("C.4.1 decode");`

```
// C.4.1 First Request
```

## L53 · `let got = d.decode(&hx("828684be5886a8eb10649cbf")).expect("C.4.2 decode");`

```
// C.4.2 Second Request
```

## L56 · `let got = d.decode(&hx("828785bf408825a849e95ba97d7f8925a849e95bb8e8b4bf")).expect("C.4.3 decode");`

```
// C.4.3 Third Request
```

## L63 · `let mut d = Decoder::with_max(256);`

```
// Consecutive header lists on ONE connection (table size 256).
```

## L65 · `let got = d.decode(&hx("4803333032580770726976617465611d4d6f6e2c203231204f637420323031332032303a31333a323120474d546e1768`

```
// C.5.1 First Response
```

## L68 · `let got = d.decode(&hx("4803333037c1c0bf")).expect("C.5.2 decode");`

```
// C.5.2 Second Response
```

## L71 · `let got = d.decode(&hx("88c1611d4d6f6e2c203231204f637420323031332032303a31333a323220474d54c05a04677a69707738666f6f3d4153`

```
// C.5.3 Third Response
```

## L78 · `let mut d = Decoder::with_max(256);`

```
// Consecutive header lists on ONE connection (table size 256).
```

## L80 · `let got = d.decode(&hx("488264025885aec3771a4b6196d07abe941054d444a8200595040b8166e082a62d1bff6e919d29ad171863c78f0b97c8`

```
// C.6.1 First Response
```

## L83 · `let got = d.decode(&hx("4883640effc1c0bf")).expect("C.6.2 decode");`

```
// C.6.2 Second Response
```

## L86 · `let got = d.decode(&hx("88c16196d07abe941054d444a8200595040b8166e084a62d1bffc05a839bd9ab77ad94e7821dd7f2e6c7b335dfdfcd5b`

```
// C.6.3 Third Response
```

