# `kernel/src/net/eth.rs` @ 5e0102684

## L1 · `use crate::netdev;`

```
//! Ethernet frame handling
```

## L17 · `_ => {} // ignore unknown`

```
// ignore unknown
```

## L21 · `pub fn send_frame(dst: &[u8; 6], ethertype: u16, payload: &[u8]) -> Result<(), crate::virtio_net::NetError> {`

```
/// Build and send an Ethernet frame
```

