//! CUBIC congestion control (RFC 9438) with HyStart, after Linux 6.18
//! `tcp_cubic.c`, at HZ = 100.

use super::*;

const BICTCP_BETA_SCALE: u32 = 1024;
const BICTCP_HZ: u32 = 10;
const HZ: u32 = 100;
const HYSTART_MIN_SAMPLES: u8 = 8;
const HYSTART_DELAY_MIN: u32 = 4000;
const HYSTART_DELAY_MAX: u32 = 16000;
/// Module parameters at their defaults.
const BETA: u32 = 717;
const BIC_SCALE: u32 = 41;
const HYSTART_LOW_WINDOW: u32 = 16;
const HYSTART_ACK_DELTA_US: u32 = 2000;
/// Precomputed in `cubictcp_register` for an SRTT of 100 ms.
const BETA_SCALE: u32 = 8 * (BICTCP_BETA_SCALE + BETA) / 3 / (BICTCP_BETA_SCALE - BETA);
const CUBE_RTT_SCALE: u64 = BIC_SCALE as u64 * 10;
const CUBE_FACTOR: u64 = (1u64 << (10 + 3 * BICTCP_HZ)) / (BIC_SCALE as u64 * 10);

/// `struct bictcp`. Times without unit are ticks (`tcp_jiffies32`).
#[derive(Default)]
pub(super) struct Cubic {
    /// Increase cwnd by 1 after this many ACKed packets.
    cnt: u32,
    last_max_cwnd: u32,
    last_cwnd: u32,
    last_time: u32,
    bic_origin_point: u32,
    bic_k: u32,
    /// Minimum delay, in µs.
    delay_min: u32,
    epoch_start: u32,
    ack_cnt: u32,
    /// The window Reno would have.
    tcp_cwnd: u32,
    sample_cnt: u8,
    /// HyStart found the exit point.
    found: bool,
    /// The round, in µs and by sequence number.
    round_start: u32,
    end_seq: u32,
    last_ack: u32,
    /// The minimum RTT of the current round, in µs.
    curr_rtt: u32,
}

fn bictcp_reset(ca: &mut Cubic) {
    *ca = Cubic {
        sample_cnt: ca.sample_cnt,
        round_start: ca.round_start,
        end_seq: ca.end_seq,
        last_ack: ca.last_ack,
        curr_rtt: ca.curr_rtt,
        ..Cubic::default()
    };
}

fn hystart_reset(c: &mut TcpConn, now_us: u64) {
    let ca = &mut c.tx.cubic;
    ca.round_start = now_us as u32;
    ca.last_ack = now_us as u32;
    ca.end_seq = c.snd_nxt;
    ca.curr_rtt = u32::MAX;
    ca.sample_cnt = 0;
}

/// `cubictcp_init`.
pub(super) fn init(c: &mut TcpConn, now_us: u64) {
    bictcp_reset(&mut c.tx.cubic);
    hystart_reset(c, now_us);
}

/// `cubictcp_cwnd_event(CA_EVENT_TX_START)`: after an idle stretch the
/// epoch moves on with it, so the curve does not jump.
pub(super) fn tx_start(c: &mut TcpConn, now: u64) {
    let now = now as u32;
    let ca = &mut c.tx.cubic;
    let delta = now.wrapping_sub(c.tx.lsndtime as u32) as i32;
    if ca.epoch_start != 0 && delta > 0 {
        ca.epoch_start = ca.epoch_start.wrapping_add(delta as u32);
        if (ca.epoch_start.wrapping_sub(now) as i32) > 0 { ca.epoch_start = now; }
    }
}

/// `cubic_root`: cbrt(a), from a table and one Newton-Raphson step.
fn cubic_root(a: u64) -> u32 {
    const V: [u8; 64] = [
        0, 54, 54, 54, 118, 118, 118, 118,
        123, 129, 134, 138, 143, 147, 151, 156,
        157, 161, 164, 168, 170, 173, 176, 179,
        181, 185, 187, 190, 192, 194, 197, 199,
        200, 202, 204, 206, 209, 211, 213, 215,
        217, 219, 221, 222, 224, 225, 227, 229,
        231, 232, 234, 236, 237, 239, 240, 242,
        244, 245, 246, 248, 250, 251, 252, 254,
    ];
    let b = 64 - a.leading_zeros();
    if b < 7 {
        return (V[a as usize] as u32 + 35) >> 6;
    }
    let b = ((b * 84) >> 8) - 1;
    let shift = (a >> (b * 3)) as usize;
    let mut x = ((V[shift] as u32 + 10) << b) >> 6;
    x = 2 * x + (a / (x as u64 * (x as u64 - 1))) as u32;
    (x * 341) >> 10
}

/// `bictcp_update`: how many ACKed packets make cwnd one larger.
fn bictcp_update(ca: &mut Cubic, cwnd: u32, acked: u32, now: u32) {
    ca.ack_cnt = ca.ack_cnt.wrapping_add(acked);
    if ca.last_cwnd == cwnd && (now.wrapping_sub(ca.last_time) as i32) <= (HZ / 32) as i32 {
        return;
    }
    // At most once per tick; a reduction clears `epoch_start` and forces it.
    if !(ca.epoch_start != 0 && now == ca.last_time) {
        ca.last_cwnd = cwnd;
        ca.last_time = now;
        if ca.epoch_start == 0 {
            ca.epoch_start = now;
            ca.ack_cnt = acked;
            ca.tcp_cwnd = cwnd;
            if ca.last_max_cwnd <= cwnd {
                ca.bic_k = 0;
                ca.bic_origin_point = cwnd;
            } else {
                ca.bic_k = cubic_root(CUBE_FACTOR * (ca.last_max_cwnd - cwnd) as u64);
                ca.bic_origin_point = ca.last_max_cwnd;
            }
        }
        // c/rtt * (t-K)^3, t in units of 2^-10 s.
        let mut t = now.wrapping_sub(ca.epoch_start) as i32 as i64 as u64;
        t += (ca.delay_min as u64).div_ceil(1_000_000 / HZ as u64);
        t <<= BICTCP_HZ;
        t /= HZ as u64;
        let k = ca.bic_k as u64;
        let offs = t.abs_diff(k);
        let delta = (CUBE_RTT_SCALE.wrapping_mul(offs).wrapping_mul(offs).wrapping_mul(offs)
            >> (10 + 3 * BICTCP_HZ)) as u32;
        let bic_target = if t < k {
            ca.bic_origin_point.wrapping_sub(delta)
        } else {
            ca.bic_origin_point.wrapping_add(delta)
        };
        ca.cnt = if bic_target > cwnd { cwnd / (bic_target - cwnd) } else { 100 * cwnd };
        // The first growth, before any loss told the bandwidth: 5% per RTT.
        if ca.last_max_cwnd == 0 && ca.cnt > 20 { ca.cnt = 20; }
    }

    // TCP friendliness: never slower than Reno would be.
    let delta = (cwnd * BETA_SCALE) >> 3;
    while delta != 0 && ca.ack_cnt > delta {
        ca.ack_cnt -= delta;
        ca.tcp_cwnd += 1;
    }
    if ca.tcp_cwnd > cwnd {
        let max_cnt = cwnd / (ca.tcp_cwnd - cwnd);
        if ca.cnt > max_cnt { ca.cnt = max_cnt; }
    }
    // At most 1.5x per RTT.
    ca.cnt = ca.cnt.max(2);
}

/// `cubictcp_cong_avoid`.
pub(super) fn cong_avoid(c: &mut TcpConn, mut acked: u32, now: u64) {
    if !recovery::is_cwnd_limited(c) { return; }
    if c.snd_cwnd < c.snd_ssthresh {
        acked = recovery::slow_start(c, acked);
        if acked == 0 { return; }
    }
    let cwnd = c.snd_cwnd;
    bictcp_update(&mut c.tx.cubic, cwnd, acked, now as u32);
    let w = c.tx.cubic.cnt;
    recovery::cong_avoid_ai(c, w, acked);
}

/// `cubictcp_recalc_ssthresh`: beta 0.7, and with fast convergence a lower
/// Wmax when the last maximum was not reached again.
pub(super) fn recalc_ssthresh(c: &mut TcpConn) -> u32 {
    let cwnd = c.snd_cwnd;
    let ca = &mut c.tx.cubic;
    ca.epoch_start = 0;
    ca.last_max_cwnd = if cwnd < ca.last_max_cwnd {
        (cwnd * (BICTCP_BETA_SCALE + BETA)) / (2 * BICTCP_BETA_SCALE)
    } else {
        cwnd
    };
    ((cwnd * BETA) / BICTCP_BETA_SCALE).max(2)
}

/// `cubictcp_state(TCP_CA_Loss)`.
pub(super) fn enter_loss(c: &mut TcpConn, now_us: u64) {
    bictcp_reset(&mut c.tx.cubic);
    hystart_reset(c, now_us);
}

fn hystart_update(c: &mut TcpConn, delay: u32, now_us: u64) {
    if (c.snd_una.wrapping_sub(c.tx.cubic.end_seq) as i32) > 0 {
        hystart_reset(c, now_us);
    }
    if c.snd_cwnd < HYSTART_LOW_WINDOW { return; }
    let cwnd = c.snd_cwnd;
    let ca = &mut c.tx.cubic;

    // ACK train: ACKs closely spaced for longer than half the minimum
    // delay (no pacing, no TSO cushion).
    let now = now_us as u32;
    if (now.wrapping_sub(ca.last_ack) as i32) <= HYSTART_ACK_DELTA_US as i32 {
        ca.last_ack = now;
        let threshold = ca.delay_min >> 1;
        if (now.wrapping_sub(ca.round_start) as i32) > threshold as i32 {
            ca.found = true;
            c.snd_ssthresh = cwnd;
        }
    }

    // Delay: the round's minimum RTT rose by an eighth of the minimum.
    let ca = &mut c.tx.cubic;
    if ca.curr_rtt > delay { ca.curr_rtt = delay; }
    if ca.sample_cnt < HYSTART_MIN_SAMPLES {
        ca.sample_cnt += 1;
    } else if ca.curr_rtt > ca.delay_min + (ca.delay_min >> 3).clamp(HYSTART_DELAY_MIN, HYSTART_DELAY_MAX) {
        ca.found = true;
        c.snd_ssthresh = cwnd;
    }
}

/// `cubictcp_acked`: the RTT of the last packet an ACK delivered.
pub(super) fn acked(c: &mut TcpConn, rtt_us: Option<u64>, now: u64, now_us: u64) {
    let Some(rtt) = rtt_us else { return };
    let ca = &mut c.tx.cubic;
    // Not right after a reduction.
    if ca.epoch_start != 0 && ((now as u32).wrapping_sub(ca.epoch_start) as i32) < HZ as i32 {
        return;
    }
    let delay = rtt.clamp(1, u32::MAX as u64) as u32;
    if ca.delay_min == 0 || ca.delay_min > delay { ca.delay_min = delay; }
    if !ca.found && c.snd_cwnd < c.snd_ssthresh {
        hystart_update(c, delay, now_us);
    }
}

#[cfg(test)]
mod tests {
    use super::cubic_root;

    #[test]
    fn cubic_root_is_close() {
        for a in [0u64, 1, 7, 8, 63, 64, 1000, 1 << 30, 123_456_789, 1 << 50] {
            let want = (a as f64).cbrt();
            let got = cubic_root(a) as f64;
            assert!((got - want).abs() <= want / 32.0 + 1.0, "cbrt({a}) = {got}, want {want}");
        }
    }
}
