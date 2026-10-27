//! NTP — Network Time Protocol (SNTP client)
//!
//! Simple NTP query to get wall-clock time.
//! Uses UDP port 123, parses NTP v4 response.
//!
//! SNTP is unauthenticated, and certificate validity rests on this clock.
//! A reply counts only if it comes from the asked server, is a server reply
//! (mode 4) from a synchronised source (stratum 1..15, leap != 3), and
//! echoes our random transmit timestamp — so an off-path forger has to
//! guess a random port and 64 random bits.

use super::udp;

const NTP_PORT: u16 = 123;
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800; // seconds from 1900 to 1970

/// The time from an NTP server, as Unix seconds. Blocking, 3 s at most.
pub fn query(server_ip: [u8; 4]) -> Option<u64> {
    // Build SNTP request (48 bytes)
    let mut req = [0u8; 48];
    req[0] = 0x23; // LI=0, Version=4, Mode=3 (client)
    // Transmit timestamp: random, not the time. The server copies it into
    // the reply's origin field; it is the nonce that ties the reply to us
    // and says nothing about our clock.
    let nonce = crate::csprng::random_u64().to_be_bytes();
    req[40..48].copy_from_slice(&nonce);

    // Warm the next hop's MAC — see `dns::resolve`: a blind spin pays its
    // timeout even when the entry is already cached.
    let _ = super::arp::resolve(super::ipv4::arp_target_for(server_ip), 10);

    let local_port = udp::listen_ephemeral()?;
    udp::send(server_ip, local_port, NTP_PORT, &req);

    let t0 = crate::interrupts::ticks();
    let mut result = None;

    loop {
        super::poll();
        if let Some((src, port, data)) = udp::recv(local_port) {
            if src == server_ip && port == NTP_PORT {
                if let Some(unix) = parse_reply(&data, &nonce) {
                    result = Some(unix);
                    break;
                }
            }
        }
        if crate::interrupts::ticks() - t0 > 300 { break; } // 3s timeout
        core::hint::spin_loop();
    }

    udp::unlisten(local_port);
    result
}

/// The server's transmit time as Unix seconds, if `data` is a reply to the
/// request that carried `nonce`.
fn parse_reply(data: &[u8], nonce: &[u8; 8]) -> Option<u64> {
    if data.len() < 48 { return None; }
    let leap = data[0] >> 6;
    let mode = data[0] & 0x07;
    let stratum = data[1];
    if mode != 4 || leap == 3 || !(1..=15).contains(&stratum) { return None; }
    if data[24..32] != nonce[..] { return None; }
    // Transmit timestamp at offset 40 (seconds since 1900-01-01)
    let secs = u32::from_be_bytes([data[40], data[41], data[42], data[43]]) as u64;
    let unix = secs.checked_sub(NTP_EPOCH_OFFSET)?;
    // Earlier than this is not believable, and certificate checks would
    // stop being enforced below it.
    (unix >= crate::tls::certstore::CLOCK_SANE_FLOOR).then_some(unix)
}
