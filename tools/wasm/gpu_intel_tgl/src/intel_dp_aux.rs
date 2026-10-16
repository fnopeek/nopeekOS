//! display/intel_dp_aux.c `intel_dp_aux_xfer` / `intel_dp_aux_transfer`
//! and the parts of display/drm_dp_helper.c a DPCD read and an
//! I2C-over-AUX transfer go through (Linux 6.18.55), for Tiger Lake.
//!
//! eDP is not ported, so there is no PPS VDD to take; the CPU latency
//! request has nothing to request here. The I2C adapter is the transfer
//! function itself (`drm_dp_i2c_xfer`), called with the messages directly.

use crate::i915_drv::drm_i915_private;
use crate::intel_de::*;
use crate::intel_encoder::*;
use crate::linux::{self, Line};
use crate::regs::*;

const EBUSY: i32 = 16;
const EIO: i32 = 5;
const E2BIG: i32 = 7;
const EINVAL: i32 = 22;
const ENXIO: i32 = 6;
const EPROTO: i32 = 71;
const ETIMEDOUT: i32 = 110;
const EREMOTEIO: i32 = 121;

/// `struct drm_dp_aux_msg`: the buffer is the caller's slice.
pub struct drm_dp_aux_msg<'a> {
    pub address: u32,
    pub request: u8,
    pub reply: u8,
    pub buffer: &'a mut [u8],
    pub size: usize,
}

/// `struct i2c_msg`.
pub struct i2c_msg<'a> {
    pub addr: u32,
    pub flags: u32,
    pub buf: &'a mut [u8],
}

/// The AUX channel of digital port `d` (`struct drm_dp_aux` with what
/// `intel_dp_aux_init` hangs on it).
pub struct drm_dp_aux<'a> {
    pub i915: &'a mut drm_i915_private,
    pub o: &'a mut intel_outputs,
    pub d: usize,
    /// `aux->dpcd_probe_disabled`.
    pub dpcd_probe_disabled: bool,
    pub i2c_nack_count: u32,
    pub i2c_defer_count: u32,
}

/// `intel_dp_aux_pack`.
fn intel_dp_aux_pack(src: &[u8]) -> u32 {
    let mut v = 0u32;
    for (i, &b) in src.iter().take(4).enumerate() {
        v |= (b as u32) << ((3 - i) * 8);
    }
    v
}

/// `intel_dp_aux_unpack`.
fn intel_dp_aux_unpack(src: u32, dst: &mut [u8]) {
    let n = dst.len().min(4);
    for (i, b) in dst[..n].iter_mut().enumerate() {
        *b = (src >> ((3 - i) * 8)) as u8;
    }
}

/// `intel_dp_aux_wait_done`.
fn intel_dp_aux_wait_done(i915: &mut drm_i915_private, ch_ctl: u32) -> u32 {
    const TIMEOUT_MS: u32 = 10;
    let mut status = 0;
    if intel_de_wait_custom(i915, ch_ctl, DP_AUX_CH_CTL_SEND_BUSY, 0, 2, TIMEOUT_MS, Some(&mut status)).is_err() {
        Line::new().s("error: AUX: did not complete or timeout within ").dec(TIMEOUT_MS as u64)
            .s("ms (status ").hex(status).s(")").say();
    }
    status
}

/// `skl_get_aux_clock_divider`: the hardware derives the clock itself.
fn skl_get_aux_clock_divider(index: u32) -> u32 {
    if index != 0 { 0 } else { 1 }
}

/// `intel_dp_aux_sync_len`.
fn intel_dp_aux_sync_len() -> u32 {
    let precharge = 16; // 10-16
    let preamble = 16;
    precharge + preamble
}

/// `intel_dp_aux_fw_sync_len`; no DPCD quirks are known yet.
fn intel_dp_aux_fw_sync_len() -> u32 {
    let precharge = 10; // 10-16
    let preamble = 8;
    precharge + preamble
}

/// `skl_get_aux_send_ctl` (display version 12).
fn skl_get_aux_send_ctl(o: &intel_outputs, d: usize, send_bytes: u32) -> u32 {
    let mut ret = DP_AUX_CH_CTL_SEND_BUSY
        | DP_AUX_CH_CTL_DONE
        | DP_AUX_CH_CTL_INTERRUPT
        | DP_AUX_CH_CTL_TIME_OUT_ERROR
        | DP_AUX_CH_CTL_TIME_OUT_MAX
        | DP_AUX_CH_CTL_RECEIVE_ERROR
        | DP_AUX_CH_CTL_MESSAGE_SIZE(send_bytes)
        | DP_AUX_CH_CTL_FW_SYNC_PULSE_SKL(intel_dp_aux_fw_sync_len())
        | DP_AUX_CH_CTL_SYNC_PULSE_SKL(intel_dp_aux_sync_len());
    if crate::intel_tc::intel_tc_port_in_tbt_alt_mode(o, d) {
        ret |= DP_AUX_CH_CTL_TBT_IO;
    }
    ret
}

/// `intel_digital_port_lock` / `_unlock`: the TC ports have the lock.
fn intel_digital_port_lock(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) {
    if o.dig_ports[d].tc.is_some() {
        crate::intel_tc::intel_tc_port_lock(i915, o, d);
    }
}

fn intel_digital_port_unlock(o: &mut intel_outputs, d: usize) {
    if o.dig_ports[d].tc.is_some() {
        crate::intel_tc::intel_tc_port_unlock(o, d);
    }
}

/// `intel_digital_port_connected_locked`, for the ports `dig_port->connected`
/// is ported for (the TC ports).
fn intel_digital_port_connected_locked(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    match o.dig_ports[d].connected {
        connected_fn::intel_tc_port_connected => {
            // intel_tc_port_handles_hpd_glitches: a non-legacy TC port does.
            let glitch_free = o.dig_ports[d].tc.is_some_and(|t| !t.legacy_port);
            crate::intel_display_power::intel_display_power_get(i915, POWER_DOMAIN_DISPLAY_CORE);
            let mut is_connected = false;
            let _ = linux::_wait_for(4000, 30, 30, || {
                is_connected = crate::intel_tc::intel_tc_port_connected(i915, o, d);
                is_connected || glitch_free
            });
            crate::intel_display_power::intel_display_power_put(i915, POWER_DOMAIN_DISPLAY_CORE);
            is_connected
        }
        _ => false,
    }
}

/// `intel_digital_port_connected`.
pub fn intel_digital_port_connected(i915: &mut drm_i915_private, o: &mut intel_outputs, d: usize) -> bool {
    intel_digital_port_lock(i915, o, d);
    let ret = intel_digital_port_connected_locked(i915, o, d);
    intel_digital_port_unlock(o, d);
    ret
}

/// `intel_dp_aux_xfer`.
fn intel_dp_aux_xfer(aux: &mut drm_dp_aux, send: &[u8], recv: &mut [u8], aux_send_ctl_flags: u32) -> i32 {
    let d = aux.d;
    let ch_ctl = aux.o.dig_ports[d].dp.aux_ch_ctl_reg;
    let ch_data = aux.o.dig_ports[d].dp.aux_ch_data_reg;

    intel_digital_port_lock(aux.i915, aux.o, d);
    // Abort transfers on a disconnected port as required by DP 1.4a link
    // CTS 4.2.1.5, also avoiding the long AUX timeouts that would
    // otherwise happen.
    if !intel_digital_port_connected_locked(aux.i915, aux.o, d) {
        intel_digital_port_unlock(aux.o, d);
        return -ENXIO;
    }

    let aux_domain = crate::intel_tc::intel_aux_power_domain(aux.o, d);
    crate::intel_display_power::intel_display_power_get(aux.i915, aux_domain);

    let ret = intel_dp_aux_xfer_locked(aux, ch_ctl, &ch_data, send, recv, aux_send_ctl_flags);

    crate::intel_display_power::intel_display_power_put_async(aux.i915, aux_domain);
    intel_digital_port_unlock(aux.o, d);
    ret
}

fn intel_dp_aux_xfer_locked(aux: &mut drm_dp_aux, ch_ctl: u32, ch_data: &[u32; 5], send: &[u8], recv: &mut [u8],
                            aux_send_ctl_flags: u32) -> i32 {
    let i915 = &mut *aux.i915;

    // Try to wait for any previous AUX channel activity.
    let mut status = 0;
    let mut tries = 0;
    while tries < 3 {
        status = intel_de_read(i915, ch_ctl);
        if status & DP_AUX_CH_CTL_SEND_BUSY == 0 {
            break;
        }
        npk_sys::sleep(1);
        tries += 1;
    }
    if tries == 3 {
        let status = intel_de_read(i915, ch_ctl);
        Line::new().s("warning: AUX: not started (status ").hex(status).s(")").say();
        return -EBUSY;
    }

    // Only 5 data registers!
    if send.len() > 20 || recv.len() > 20 {
        linux::say("warning: AUX transfer over 20 bytes");
        return -E2BIG;
    }

    let mut clock = 0;
    let mut done = false;
    'outer: loop {
        let aux_clock_divider = skl_get_aux_clock_divider(clock);
        clock += 1;
        if aux_clock_divider == 0 {
            break;
        }
        let send_ctl = skl_get_aux_send_ctl(aux.o, aux.d, send.len() as u32) | aux_send_ctl_flags;

        // Must try at least 3 times according to DP spec.
        for _ in 0..5 {
            // Load the send data into the aux channel data registers.
            let mut i = 0;
            while i < send.len() {
                intel_de_write(i915, ch_data[i >> 2], intel_dp_aux_pack(&send[i..]));
                i += 4;
            }

            // Send the command and wait for it to complete.
            intel_de_write(i915, ch_ctl, send_ctl);

            status = intel_dp_aux_wait_done(i915, ch_ctl);

            // Clear done status and any errors.
            intel_de_write(i915, ch_ctl,
                           status | DP_AUX_CH_CTL_DONE | DP_AUX_CH_CTL_TIME_OUT_ERROR | DP_AUX_CH_CTL_RECEIVE_ERROR);

            // DP CTS 1.2 Core Rev 1.1, 4.2.1.1 & 4.2.1.2: 400us delay
            // required for errors and timeouts. Timeout errors from the
            // hardware already meet this requirement.
            if status & DP_AUX_CH_CTL_TIME_OUT_ERROR != 0 {
                continue;
            }
            if status & DP_AUX_CH_CTL_RECEIVE_ERROR != 0 {
                linux::udelay(400);
                continue;
            }
            if status & DP_AUX_CH_CTL_DONE != 0 {
                done = true;
                break 'outer;
            }
        }
    }

    if !done && status & DP_AUX_CH_CTL_DONE == 0 {
        Line::new().s("error: AUX: not done (status ").hex(status).s(")").say();
        return -EBUSY;
    }

    // Check for timeout or receive error. Timeouts occur when the sink is
    // not connected.
    if status & DP_AUX_CH_CTL_RECEIVE_ERROR != 0 {
        Line::new().s("error: AUX: receive error (status ").hex(status).s(")").say();
        return -EIO;
    }
    if status & DP_AUX_CH_CTL_TIME_OUT_ERROR != 0 {
        return -ETIMEDOUT;
    }

    // Unload any bytes sent back from the other side.
    let mut recv_bytes = crate::intel_gt::reg_field_get(DP_AUX_CH_CTL_MESSAGE_SIZE_MASK, status) as usize;

    // By BSpec: "Message sizes of 0 or >20 are not allowed."
    if recv_bytes == 0 || recv_bytes > 20 {
        return -EBUSY;
    }
    if recv_bytes > recv.len() {
        recv_bytes = recv.len();
    }

    let mut i = 0;
    while i < recv_bytes {
        let end = recv_bytes.min(i + 4);
        intel_dp_aux_unpack(intel_de_read(i915, ch_data[i >> 2]), &mut recv[i..end]);
        i += 4;
    }
    recv_bytes as i32
}

const BARE_ADDRESS_SIZE: usize = 3;
const HEADER_SIZE: usize = BARE_ADDRESS_SIZE + 1;

/// `intel_dp_aux_header`.
fn intel_dp_aux_header(txbuf: &mut [u8], msg: &drm_dp_aux_msg) {
    txbuf[0] = (msg.request << 4) | ((msg.address >> 16) & 0xf) as u8;
    txbuf[1] = (msg.address >> 8) as u8;
    txbuf[2] = msg.address as u8;
    txbuf[3] = msg.size.wrapping_sub(1) as u8;
}

/// `intel_dp_aux_transfer`. No HDCP Aksv is ever sent, so no transfer flags.
fn intel_dp_aux_transfer(aux: &mut drm_dp_aux, msg: &mut drm_dp_aux_msg) -> i32 {
    let mut txbuf = [0u8; 20];
    let mut rxbuf = [0u8; 20];

    intel_dp_aux_header(&mut txbuf, msg);

    match (msg.request & !DP_AUX_I2C_MOT as u8) as u32 {
        DP_AUX_NATIVE_WRITE | DP_AUX_I2C_WRITE | DP_AUX_I2C_WRITE_STATUS_UPDATE => {
            let txsize = if msg.size != 0 { HEADER_SIZE + msg.size } else { BARE_ADDRESS_SIZE };
            let rxsize = 2; // 0 or 1 data bytes
            if txsize > 20 {
                linux::say("warning: AUX write over 20 bytes");
                return -E2BIG;
            }
            txbuf[HEADER_SIZE..HEADER_SIZE + msg.size].copy_from_slice(&msg.buffer[..msg.size]);
            let ret = intel_dp_aux_xfer(aux, &txbuf[..txsize], &mut rxbuf[..rxsize], 0);
            if ret > 0 {
                msg.reply = rxbuf[0] >> 4;
                if ret > 1 {
                    // Number of bytes written in a short write.
                    (rxbuf[1] as i32).clamp(0, msg.size as i32)
                } else {
                    // Return payload size.
                    msg.size as i32
                }
            } else {
                ret
            }
        }
        DP_AUX_NATIVE_READ | DP_AUX_I2C_READ => {
            let txsize = if msg.size != 0 { HEADER_SIZE } else { BARE_ADDRESS_SIZE };
            let rxsize = msg.size + 1;
            if rxsize > 20 {
                linux::say("warning: AUX read over 20 bytes");
                return -E2BIG;
            }
            let ret = intel_dp_aux_xfer(aux, &txbuf[..txsize], &mut rxbuf[..rxsize], 0);
            if ret > 0 {
                msg.reply = rxbuf[0] >> 4;
                // Assume happy day, and copy the data. The caller is
                // expected to check msg.reply before touching it.
                let n = (ret - 1) as usize;
                msg.buffer[..n].copy_from_slice(&rxbuf[1..1 + n]);
                n as i32
            } else {
                ret
            }
        }
        _ => -EINVAL,
    }
}

/// `drm_dp_dpcd_access`.
fn drm_dp_dpcd_access(aux: &mut drm_dp_aux, request: u8, offset: u32, buffer: &mut [u8]) -> i32 {
    let size = buffer.len();
    let mut msg = drm_dp_aux_msg { address: offset, request, reply: 0, buffer, size };
    let mut err = 0;
    let mut ret = 0;

    // The specification doesn't give any recommendation on how often to
    // retry native transactions; 32 makes Dell 4k monitors happier.
    for _ in 0..32 {
        if ret != 0 && ret != -ETIMEDOUT {
            linux::udelay(AUX_RETRY_INTERVAL as u64);
        }

        ret = intel_dp_aux_transfer(aux, &mut msg);
        if ret >= 0 {
            let native_reply = msg.reply as u32 & DP_AUX_NATIVE_REPLY_MASK;
            if native_reply == DP_AUX_NATIVE_REPLY_ACK {
                if ret as usize == size {
                    return ret;
                }
                ret = -EPROTO;
            } else {
                ret = -EIO;
            }
        }

        // We want the error we return to be the error we received on the
        // first transaction.
        if err == 0 {
            err = ret;
        }
    }
    err
}

/// `drm_dp_dpcd_probe`.
fn drm_dp_dpcd_probe(aux: &mut drm_dp_aux, offset: u32) -> i32 {
    let mut buffer = [0u8; 1];
    let ret = drm_dp_dpcd_access(aux, DP_AUX_NATIVE_READ as u8, offset, &mut buffer);
    if ret < 0 { ret } else { 0 }
}

/// `drm_dp_dpcd_read`.
pub fn drm_dp_dpcd_read(aux: &mut drm_dp_aux, offset: u32, buffer: &mut [u8]) -> i32 {
    // dpcd_access_needs_probe: no remote (MST) AUX here.
    if !aux.dpcd_probe_disabled {
        let ret = drm_dp_dpcd_probe(aux, DP_TRAINING_PATTERN_SET);
        if ret < 0 {
            return ret;
        }
    }
    drm_dp_dpcd_access(aux, DP_AUX_NATIVE_READ as u8, offset, buffer)
}

/// `drm_dp_dpcd_read_data`.
pub fn drm_dp_dpcd_read_data(aux: &mut drm_dp_aux, offset: u32, buffer: &mut [u8]) -> i32 {
    let size = buffer.len() as i32;
    let ret = drm_dp_dpcd_read(aux, offset, buffer);
    if ret < 0 {
        return ret;
    }
    if ret < size {
        return -EPROTO;
    }
    0
}

/// `drm_dp_i2c_msg_write_status_update`.
fn drm_dp_i2c_msg_write_status_update(msg: &mut drm_dp_aux_msg) {
    // In case of i2c defer or short i2c ack reply to a write, we need to
    // switch to WRITE_STATUS_UPDATE to drain the rest of the message.
    if (msg.request & !(DP_AUX_I2C_MOT as u8)) as u32 == DP_AUX_I2C_WRITE {
        msg.request &= DP_AUX_I2C_MOT as u8;
        msg.request |= DP_AUX_I2C_WRITE_STATUS_UPDATE as u8;
    }
}

const AUX_PRECHARGE_LEN: i32 = 10; // 10 to 16
const AUX_SYNC_LEN: i32 = 16 + 4; // preamble + AUX_SYNC_END
const AUX_STOP_LEN: i32 = 4;
const AUX_CMD_LEN: i32 = 4;
const AUX_ADDRESS_LEN: i32 = 20;
const AUX_REPLY_PAD_LEN: i32 = 4;
const AUX_LENGTH_LEN: i32 = 8;

/// `drm_dp_aux_req_duration`.
fn drm_dp_aux_req_duration(msg: &drm_dp_aux_msg) -> i32 {
    let mut len = AUX_PRECHARGE_LEN + AUX_SYNC_LEN + AUX_STOP_LEN + AUX_CMD_LEN + AUX_ADDRESS_LEN + AUX_LENGTH_LEN;
    if msg.request as u32 & DP_AUX_I2C_READ == 0 {
        len += msg.size as i32 * 8;
    }
    len
}

/// `drm_dp_aux_reply_duration`.
fn drm_dp_aux_reply_duration(msg: &drm_dp_aux_msg) -> i32 {
    let mut len = AUX_PRECHARGE_LEN + AUX_SYNC_LEN + AUX_STOP_LEN + AUX_CMD_LEN + AUX_REPLY_PAD_LEN;
    if msg.request as u32 & DP_AUX_I2C_READ != 0 {
        len += msg.size as i32 * 8;
    }
    len
}

const I2C_START_LEN: i32 = 1;
const I2C_STOP_LEN: i32 = 1;
const I2C_ADDR_LEN: i32 = 9; // ADDRESS + R/W + ACK/NACK
const I2C_DATA_LEN: i32 = 9; // DATA + ACK/NACK

/// `DIV_ROUND_UP` for the positive durations here.
fn DIV_ROUND_UP(n: i32, d: i32) -> i32 {
    (n + d - 1) / d
}

/// `drm_dp_i2c_msg_duration`.
fn drm_dp_i2c_msg_duration(msg: &drm_dp_aux_msg, i2c_speed_khz: i32) -> i32 {
    // AUX bitrate is 1MHz, i2c bitrate as specified.
    DIV_ROUND_UP((I2C_START_LEN + I2C_ADDR_LEN + msg.size as i32 * I2C_DATA_LEN + I2C_STOP_LEN) * 1000, i2c_speed_khz)
}

/// `drm_dp_i2c_retry_count`.
fn drm_dp_i2c_retry_count(msg: &drm_dp_aux_msg, i2c_speed_khz: i32) -> i32 {
    let aux_time_us = drm_dp_aux_req_duration(msg) + drm_dp_aux_reply_duration(msg);
    let i2c_time_us = drm_dp_i2c_msg_duration(msg, i2c_speed_khz);
    DIV_ROUND_UP(i2c_time_us, aux_time_us + AUX_RETRY_INTERVAL as i32)
}

/// `dp_aux_i2c_speed_khz`.
const DP_AUX_I2C_SPEED_KHZ: i32 = 10;

/// `drm_dp_i2c_do_msg`: one I2C-over-AUX message, retried as the DP spec
/// asks. Bytes transferred, or a negative error.
fn drm_dp_i2c_do_msg(aux: &mut drm_dp_aux, msg: &mut drm_dp_aux_msg) -> i32 {
    // DP1.2 sections 2.7.7.1.5.6.1 and 2.7.7.1.6.6.1: retry at least seven
    // times upon AUX_DEFER, and account for the i2c bus speed.
    let max_retries = 7.max(drm_dp_i2c_retry_count(msg, DP_AUX_I2C_SPEED_KHZ));
    let mut defer_i2c = 0;
    let mut retry = 0;
    while retry < max_retries + defer_i2c {
        retry += 1;
        let ret = intel_dp_aux_transfer(aux, msg);
        if ret < 0 {
            if ret == -EBUSY {
                continue;
            }
            return ret;
        }

        match msg.reply as u32 & DP_AUX_NATIVE_REPLY_MASK {
            DP_AUX_NATIVE_REPLY_ACK => {}
            DP_AUX_NATIVE_REPLY_NACK => return -EREMOTEIO,
            DP_AUX_NATIVE_REPLY_DEFER => {
                linux::udelay(AUX_RETRY_INTERVAL as u64);
                continue;
            }
            _ => {
                Line::new().s("error: AUX: invalid native reply ").hex(msg.reply as u32).say();
                return -EREMOTEIO;
            }
        }

        match msg.reply as u32 & DP_AUX_I2C_REPLY_MASK {
            DP_AUX_I2C_REPLY_ACK => {
                // Both native ACK and I2C ACK replies received.
                if ret as usize != msg.size {
                    drm_dp_i2c_msg_write_status_update(msg);
                }
                return ret;
            }
            DP_AUX_I2C_REPLY_NACK => {
                aux.i2c_nack_count += 1;
                return -EREMOTEIO;
            }
            DP_AUX_I2C_REPLY_DEFER => {
                // DP Compliance Test 4.2.2.5: at least 7 retries for I2C
                // defers.
                aux.i2c_defer_count += 1;
                if defer_i2c < 7 {
                    defer_i2c += 1;
                }
                linux::udelay(AUX_RETRY_INTERVAL as u64);
                drm_dp_i2c_msg_write_status_update(msg);
                continue;
            }
            _ => {
                Line::new().s("error: AUX: invalid I2C reply ").hex(msg.reply as u32).say();
                return -EREMOTEIO;
            }
        }
    }
    -EREMOTEIO
}

/// `drm_dp_i2c_msg_set_request`.
fn drm_dp_i2c_msg_set_request(request: &mut u8, flags: u32) {
    *request = if flags & I2C_M_RD != 0 { DP_AUX_I2C_READ as u8 } else { DP_AUX_I2C_WRITE as u8 };
    if flags & I2C_M_STOP == 0 {
        *request |= DP_AUX_I2C_MOT as u8;
    }
}

/// `drm_dp_i2c_drain_msg`: retries until all of `buffer` is transferred.
/// A recommended transfer size, or a negative error.
fn drm_dp_i2c_drain_msg(aux: &mut drm_dp_aux, address: u32, request: &mut u8, buffer: &mut [u8]) -> i32 {
    let mut ret = buffer.len() as i32;
    let mut at = 0;
    while at < buffer.len() {
        let size = buffer.len() - at;
        let mut msg = drm_dp_aux_msg { address, request: *request, reply: 0, buffer: &mut buffer[at..], size };
        let err = drm_dp_i2c_do_msg(aux, &mut msg);
        *request = msg.request;
        if err <= 0 {
            return if err == 0 { -EPROTO } else { err };
        }
        if (err as usize) < size && err < ret {
            ret = err;
        }
        at += err as usize;
    }
    ret
}

/// `drm_dp_i2c_xfer`: the I2C adapter's transfer. `num` on success.
pub fn drm_dp_i2c_xfer(aux: &mut drm_dp_aux, msgs: &mut [i2c_msg]) -> i32 {
    // dp_aux_i2c_transfer_size: the maximum, clamped as Linux does.
    let transfer_max = DP_AUX_MAX_PAYLOAD_BYTES as usize;
    let mut err = 0;
    let mut request = 0u8;
    let mut address = 0u32;
    let mut empty = [0u8; 0];

    for m in msgs.iter_mut() {
        address = m.addr;

        // Send a bare address packet to start the transaction.
        drm_dp_i2c_msg_set_request(&mut request, m.flags);
        let mut msg = drm_dp_aux_msg { address, request, reply: 0, buffer: &mut empty, size: 0 };
        err = drm_dp_i2c_do_msg(aux, &mut msg);

        // Reset the request in case it got changed into a
        // WRITE_STATUS_UPDATE.
        drm_dp_i2c_msg_set_request(&mut request, m.flags);
        if err < 0 {
            break;
        }

        // Each transaction as large as possible, smaller if the hardware
        // gives a short reply.
        let mut transfer_size = transfer_max;
        let mut j = 0;
        while j < m.buf.len() {
            let n = transfer_size.min(m.buf.len() - j);
            err = drm_dp_i2c_drain_msg(aux, address, &mut request, &mut m.buf[j..j + n]);
            drm_dp_i2c_msg_set_request(&mut request, m.flags);
            if err < 0 {
                break;
            }
            transfer_size = err as usize;
            j += n;
        }
        if err < 0 {
            break;
        }
    }
    if err >= 0 {
        err = msgs.len() as i32;
    }

    // Send a bare address packet to close out the transaction.
    request &= !(DP_AUX_I2C_MOT as u8);
    let mut msg = drm_dp_aux_msg { address, request, reply: 0, buffer: &mut empty, size: 0 };
    let _ = drm_dp_i2c_do_msg(aux, &mut msg);
    err
}

/// `drm_dp_is_branch`.
pub fn drm_dp_is_branch(dpcd: &[u8]) -> bool {
    dpcd[DP_DOWNSTREAMPORT_PRESENT as usize] & DP_DWN_STRM_PORT_PRESENT as u8 != 0
}

/// `drm_dp_read_extended_dpcd_caps`.
fn drm_dp_read_extended_dpcd_caps(aux: &mut drm_dp_aux, dpcd: &mut [u8; DP_RECEIVER_CAP_SIZE as usize]) -> i32 {
    // Prior to DP1.3 the bit was reserved; if set, DP_DPCD_REV at 0000h
    // can be less than the true capability, found at 2200h.
    if dpcd[DP_TRAINING_AUX_RD_INTERVAL as usize] & DP_EXTENDED_RECEIVER_CAP_FIELD_PRESENT as u8 == 0 {
        return 0;
    }
    let mut dpcd_ext = [0u8; DP_RECEIVER_CAP_SIZE as usize];
    let ret = drm_dp_dpcd_read_data(aux, DP_DP13_DPCD_REV, &mut dpcd_ext);
    if ret < 0 {
        return ret;
    }
    if dpcd[DP_DPCD_REV as usize] > dpcd_ext[DP_DPCD_REV as usize] {
        return 0;
    }
    *dpcd = dpcd_ext;
    0
}

/// `drm_dp_read_dpcd_caps`.
pub fn drm_dp_read_dpcd_caps(aux: &mut drm_dp_aux, dpcd: &mut [u8; DP_RECEIVER_CAP_SIZE as usize]) -> i32 {
    let ret = drm_dp_dpcd_read_data(aux, DP_DPCD_REV, dpcd);
    if ret < 0 {
        return ret;
    }
    if dpcd[DP_DPCD_REV as usize] == 0 {
        return -EIO;
    }
    drm_dp_read_extended_dpcd_caps(aux, dpcd)
}

/// `drm_dp_downstream_port_count`.
fn drm_dp_downstream_port_count(dpcd: &[u8]) -> u8 {
    let mut port_count = dpcd[DP_DOWN_STREAM_PORT_COUNT as usize] & DP_PORT_COUNT_MASK as u8;
    if dpcd[DP_DOWNSTREAMPORT_PRESENT as usize] & DP_DETAILED_CAP_INFO_AVAILABLE as u8 != 0 && port_count > 4 {
        port_count = 4;
    }
    port_count
}

/// `drm_dp_read_downstream_info`.
pub fn drm_dp_read_downstream_info(aux: &mut drm_dp_aux, dpcd: &[u8],
                                   downstream_ports: &mut [u8; DP_MAX_DOWNSTREAM_PORTS as usize]) -> i32 {
    downstream_ports.fill(0);

    // No downstream info to read.
    if !drm_dp_is_branch(dpcd) || dpcd[DP_DPCD_REV as usize] as u32 == DP_DPCD_REV_10 {
        return 0;
    }
    let mut len = drm_dp_downstream_port_count(dpcd) as usize;
    if len == 0 {
        return 0;
    }
    if dpcd[DP_DOWNSTREAMPORT_PRESENT as usize] & DP_DETAILED_CAP_INFO_AVAILABLE as u8 != 0 {
        len *= 4;
    }
    drm_dp_dpcd_read_data(aux, DP_DOWNSTREAM_PORT_0, &mut downstream_ports[..len])
}

/// `DP_GET_SINK_COUNT` (drm_dp.h): bit 7 moves down to bit 6.
fn DP_GET_SINK_COUNT(x: u32) -> u32 {
    ((x & 0x80) >> 1) | (x & 0x3f)
}

/// `drm_dp_read_sink_count`.
pub fn drm_dp_read_sink_count(aux: &mut drm_dp_aux) -> i32 {
    let mut count = [0u8; 1];
    let ret = drm_dp_dpcd_read_data(aux, DP_SINK_COUNT, &mut count);
    if ret < 0 {
        return ret;
    }
    DP_GET_SINK_COUNT(count[0] as u32) as i32
}

/// `drm_dp_read_sink_count_cap`; no DPCD quirks are known.
pub fn drm_dp_read_sink_count_cap(connector_type: u32, dpcd: &[u8]) -> bool {
    // Some eDP panels don't set a valid value for the sink count.
    connector_type != DRM_MODE_CONNECTOR_eDP
        && dpcd[DP_DPCD_REV as usize] as u32 >= DP_DPCD_REV_11
        && dpcd[DP_DOWNSTREAMPORT_PRESENT as usize] & DP_DWN_STRM_PORT_PRESENT as u8 != 0
}
