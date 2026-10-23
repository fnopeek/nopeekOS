//! drivers/gpu/drm/display/drm_dp_helper.c (Linux 6.18.55): native DPCD
//! access with its retries, and I2C over AUX as an I2C adapter, on top of a
//! driver's `transfer`. Linux's `transfer` finds its driver state through
//! `container_of`; the port hands the DM's context (`dm_ctx`) down instead.
//! The CEC, CRC and MST halves of the file are not on the detection path.
#![allow(dead_code)]

use crate::amdgpu_dm_helpers::dm_ctx;
use crate::i2c::{i2c_adapter, i2c_algo, i2c_msg, I2C_M_RD, I2C_M_STOP};
use crate::linux::{usleep_range, EBUSY, EIO, EPROTO, EREMOTEIO, ETIMEDOUT};
use crate::regs::*;
use alloc::string::String;

/// `struct drm_dp_aux_msg`. `buffer` is an offset into the caller's slice
/// with `size` bytes: Linux moves the pointer, the port the offset.
#[derive(Clone, Copy, Default)]
pub struct drm_dp_aux_msg {
    pub address: u32,
    pub request: u32,
    pub reply: u8,
    pub buffer: usize,
    pub size: usize,
}

/// `aux->transfer`: the message, and the bytes it reads or writes.
pub type drm_dp_aux_transfer = fn(&mut drm_dp_aux, &mut dm_ctx, &mut drm_dp_aux_msg, &mut [u8]) -> isize;

/// `struct drm_dp_aux`, the members the SST path reaches.
#[derive(Default)]
pub struct drm_dp_aux {
    pub name: String,
    pub ddc: i2c_adapter,
    pub transfer: Option<drm_dp_aux_transfer>,
    pub i2c_nack_count: u32,
    pub i2c_defer_count: u32,
    pub is_remote: bool,
    pub powered_down: bool,
    pub no_zero_sized: bool,
    pub dpcd_probe_disabled: bool,
}

const AUX_RETRY_INTERVAL: u64 = 500; // us

/// `drm_dp_dump_access`: a drm_dbg_dp line, off in the module.
fn drm_dp_dump_access(_aux: &drm_dp_aux, _request: u32, _offset: u32, _ret: isize) {}

/// `drm_dp_dpcd_access`.
fn drm_dp_dpcd_access(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, request: u32, offset: u32, buffer: &mut [u8]) -> isize {
    let size = buffer.len();
    let mut msg = drm_dp_aux_msg { address: offset, request, reply: 0, buffer: 0, size };
    let mut err: isize = 0;
    let mut ret: isize = 0;

    // If the device attached to the aux bus is powered down then there's
    // no reason to attempt a transfer. Error out immediately.
    if aux.powered_down {
        return -EBUSY;
    }

    // The specification doesn't give any recommendation on how often to
    // retry native transactions. We used to retry 7 times like for
    // aux i2c transactions but real world devices this wasn't
    // sufficient, bump to 32 which makes Dell 4k monitors happier.
    for _retry in 0..32 {
        if ret != 0 && ret != -ETIMEDOUT {
            usleep_range(AUX_RETRY_INTERVAL, AUX_RETRY_INTERVAL + 100);
        }

        let transfer = aux.transfer.unwrap();
        ret = transfer(aux, ctx, &mut msg, buffer);
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

        // We want the error we return to be the error we received on
        // the first transaction, since we may get a different error the
        // next time we retry
        if err == 0 {
            err = ret;
        }
    }

    err
}

/// `drm_dp_dpcd_probe`.
pub fn drm_dp_dpcd_probe(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, offset: u32) -> isize {
    let mut buffer = [0u8; 1];

    let ret = drm_dp_dpcd_access(aux, ctx, DP_AUX_NATIVE_READ, offset, &mut buffer);

    drm_dp_dump_access(aux, DP_AUX_NATIVE_READ, offset, ret);

    if ret < 0 { ret } else { 0 }
}

/// `drm_dp_dpcd_set_probe`.
pub fn drm_dp_dpcd_set_probe(aux: &mut drm_dp_aux, enable: bool) {
    aux.dpcd_probe_disabled = !enable;
}

/// `dpcd_access_needs_probe`: the HP ZR24w throw-away read.
fn dpcd_access_needs_probe(aux: &drm_dp_aux) -> bool {
    !aux.is_remote && !aux.dpcd_probe_disabled
}

/// `drm_dp_dpcd_read`. An MST port (`is_remote`) is not on this path.
pub fn drm_dp_dpcd_read(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, offset: u32, buffer: &mut [u8]) -> isize {
    if dpcd_access_needs_probe(aux) {
        let ret = drm_dp_dpcd_probe(aux, ctx, DP_TRAINING_PATTERN_SET);
        if ret < 0 {
            return ret;
        }
    }

    let ret = drm_dp_dpcd_access(aux, ctx, DP_AUX_NATIVE_READ, offset, buffer);

    drm_dp_dump_access(aux, DP_AUX_NATIVE_READ, offset, ret);
    ret
}

/// `drm_dp_dpcd_write`.
pub fn drm_dp_dpcd_write(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, offset: u32, buffer: &mut [u8]) -> isize {
    let ret = drm_dp_dpcd_access(aux, ctx, DP_AUX_NATIVE_WRITE, offset, buffer);

    drm_dp_dump_access(aux, DP_AUX_NATIVE_WRITE, offset, ret);
    ret
}

/// `drm_dp_i2c_msg_write_status_update`.
fn drm_dp_i2c_msg_write_status_update(msg: &mut drm_dp_aux_msg) {
    // In case of i2c defer or short i2c ack reply to a write,
    // we need to switch to WRITE_STATUS_UPDATE to drain the
    // rest of the message
    if (msg.request & !DP_AUX_I2C_MOT) == DP_AUX_I2C_WRITE {
        msg.request &= DP_AUX_I2C_MOT;
        msg.request |= DP_AUX_I2C_WRITE_STATUS_UPDATE;
    }
}

const AUX_PRECHARGE_LEN: i32 = 10; // 10 to 16
const AUX_SYNC_LEN: i32 = 16 + 4; // preamble + AUX_SYNC_END
const AUX_STOP_LEN: i32 = 4;
const AUX_CMD_LEN: i32 = 4;
const AUX_ADDRESS_LEN: i32 = 20;
const AUX_REPLY_PAD_LEN: i32 = 4;
const AUX_LENGTH_LEN: i32 = 8;

/// `drm_dp_aux_req_duration`: the "best" case of the request, in us.
fn drm_dp_aux_req_duration(msg: &drm_dp_aux_msg) -> i32 {
    let mut len = AUX_PRECHARGE_LEN + AUX_SYNC_LEN + AUX_STOP_LEN + AUX_CMD_LEN + AUX_ADDRESS_LEN + AUX_LENGTH_LEN;

    if msg.request & DP_AUX_I2C_READ == 0 {
        len += msg.size as i32 * 8;
    }

    len
}

/// `drm_dp_aux_reply_duration`.
fn drm_dp_aux_reply_duration(msg: &drm_dp_aux_msg) -> i32 {
    let mut len = AUX_PRECHARGE_LEN + AUX_SYNC_LEN + AUX_STOP_LEN + AUX_CMD_LEN + AUX_REPLY_PAD_LEN;

    // For read we expect what was asked. For writes there will
    // be 0 or 1 data bytes. Assume 0 for the "best" case.
    if msg.request & DP_AUX_I2C_READ != 0 {
        len += msg.size as i32 * 8;
    }

    len
}

const I2C_START_LEN: i32 = 1;
const I2C_STOP_LEN: i32 = 1;
const I2C_ADDR_LEN: i32 = 9; // ADDRESS + R/W + ACK/NACK
const I2C_DATA_LEN: i32 = 9; // DATA + ACK/NACK

fn div_round_up(n: i32, d: i32) -> i32 {
    (n + d - 1) / d
}

/// `drm_dp_i2c_msg_duration`: the "worst" case of the I2C transfer, in us.
fn drm_dp_i2c_msg_duration(msg: &drm_dp_aux_msg, i2c_speed_khz: i32) -> i32 {
    // AUX bitrate is 1MHz, i2c bitrate as specified
    div_round_up((I2C_START_LEN + I2C_ADDR_LEN + msg.size as i32 * I2C_DATA_LEN + I2C_STOP_LEN) * 1000, i2c_speed_khz)
}

/// `drm_dp_i2c_retry_count`.
fn drm_dp_i2c_retry_count(msg: &drm_dp_aux_msg, i2c_speed_khz: i32) -> i32 {
    let aux_time_us = drm_dp_aux_req_duration(msg) + drm_dp_aux_reply_duration(msg);
    let i2c_time_us = drm_dp_i2c_msg_duration(msg, i2c_speed_khz);

    div_round_up(i2c_time_us, aux_time_us + AUX_RETRY_INTERVAL as i32)
}

/// `dp_aux_i2c_speed_khz` (module parameter, default 10).
const DP_AUX_I2C_SPEED_KHZ: i32 = 10;

/// `drm_dp_i2c_do_msg`: one I2C-over-AUX message, retried as the replies
/// ask. Bytes transferred, or a negative error.
fn drm_dp_i2c_do_msg(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, msg: &mut drm_dp_aux_msg, buf: &mut [u8]) -> isize {
    // DP1.2 sections 2.7.7.1.5.6.1 and 2.7.7.1.6.6.1: A DP Source device
    // is required to retry at least seven times upon receiving AUX_DEFER
    // before giving up the AUX transaction.
    //
    // We also try to account for the i2c bus speed.
    let max_retries = 7.max(drm_dp_i2c_retry_count(msg, DP_AUX_I2C_SPEED_KHZ)) as u32;
    let mut retry = 0;
    let mut defer_i2c = 0;

    while retry < max_retries + defer_i2c {
        retry += 1;
        let transfer = aux.transfer.unwrap();
        let ret = transfer(aux, ctx, msg, buf);
        if ret < 0 {
            if ret == -EBUSY {
                continue;
            }

            // While timeouts can be errors, they're usually normal
            // behavior (for instance, when a driver tries to
            // communicate with a non-existent DisplayPort device).
            // Avoid spamming the kernel log with timeout errors.
            return ret;
        }

        match msg.reply as u32 & DP_AUX_NATIVE_REPLY_MASK {
            DP_AUX_NATIVE_REPLY_ACK => {
                // For I2C-over-AUX transactions this isn't enough, we
                // need to check for the I2C ACK reply.
            }
            DP_AUX_NATIVE_REPLY_NACK => return -EREMOTEIO,
            DP_AUX_NATIVE_REPLY_DEFER => {
                // We could check for I2C bit rate capabilities and if
                // available adjust this interval. We could also be
                // more careful with DP-to-legacy adapters where a
                // long legacy cable may force very low I2C bit rates.
                //
                // For now just defer for long enough to hopefully be
                // safe for all use-cases.
                usleep_range(AUX_RETRY_INTERVAL, AUX_RETRY_INTERVAL + 100);
                continue;
            }
            _ => {
                crate::linux::Line::new().s(&aux.name).s(": invalid native reply ").hex(msg.reply as u32).say();
                return -EREMOTEIO;
            }
        }

        match msg.reply as u32 & DP_AUX_I2C_REPLY_MASK {
            DP_AUX_I2C_REPLY_ACK => {
                // Both native ACK and I2C ACK replies received. We
                // can assume the transfer was successful.
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
                // DP Compliance Test 4.2.2.5 Requirement:
                // Must have at least 7 retries for I2C defers on the
                // transaction to pass this test
                aux.i2c_defer_count += 1;
                if defer_i2c < 7 {
                    defer_i2c += 1;
                }
                usleep_range(AUX_RETRY_INTERVAL, AUX_RETRY_INTERVAL + 100);
                drm_dp_i2c_msg_write_status_update(msg);

                continue;
            }
            _ => {
                crate::linux::Line::new().s(&aux.name).s(": invalid I2C reply ").hex(msg.reply as u32).say();
                return -EREMOTEIO;
            }
        }
    }

    -EREMOTEIO
}

/// `drm_dp_i2c_msg_set_request`.
fn drm_dp_i2c_msg_set_request(msg: &mut drm_dp_aux_msg, i2c_msg: &i2c_msg) {
    msg.request = if i2c_msg.flags & I2C_M_RD != 0 { DP_AUX_I2C_READ } else { DP_AUX_I2C_WRITE };
    if i2c_msg.flags & I2C_M_STOP == 0 {
        msg.request |= DP_AUX_I2C_MOT;
    }
}

/// `drm_dp_i2c_drain_msg`: until all of the message is transferred. An
/// error, or the recommended transfer size.
fn drm_dp_i2c_drain_msg(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, orig_msg: &drm_dp_aux_msg, buf: &mut [u8]) -> isize {
    let mut ret = orig_msg.size as isize;
    let mut msg = *orig_msg;

    while msg.size > 0 {
        let err = drm_dp_i2c_do_msg(aux, ctx, &mut msg, buf);
        if err <= 0 {
            return if err == 0 { -EPROTO } else { err };
        }

        if (err as usize) < msg.size && err < ret {
            ret = err;
        }

        msg.size -= err as usize;
        msg.buffer += err as usize;
    }

    ret
}

/// `dp_aux_i2c_transfer_size` (module parameter, default
/// DP_AUX_MAX_PAYLOAD_BYTES).
const DP_AUX_I2C_TRANSFER_SIZE: u32 = DP_AUX_MAX_PAYLOAD_BYTES;

/// `drm_dp_i2c_xfer`: the `master_xfer` of the AUX channel's adapter.
pub fn drm_dp_i2c_xfer(aux: &mut drm_dp_aux, ctx: &mut dm_ctx, msgs: &mut [i2c_msg]) -> i32 {
    let num = msgs.len();
    let mut err: isize = 0;
    let mut msg = drm_dp_aux_msg::default();
    let mut none: [u8; 0] = [];

    if aux.powered_down {
        return -EBUSY as i32;
    }

    let dp_aux_i2c_transfer_size = DP_AUX_I2C_TRANSFER_SIZE.clamp(1, DP_AUX_MAX_PAYLOAD_BYTES);

    for m in msgs.iter_mut() {
        msg.address = m.addr as u32;

        if !aux.no_zero_sized {
            drm_dp_i2c_msg_set_request(&mut msg, m);
            // Send a bare address packet to start the transaction.
            // Zero sized messages specify an address only (bare
            // address) transaction.
            msg.buffer = 0;
            msg.size = 0;
            err = drm_dp_i2c_do_msg(aux, ctx, &mut msg, &mut none);
        }

        // Reset msg.request in case in case it got
        // changed into a WRITE_STATUS_UPDATE.
        drm_dp_i2c_msg_set_request(&mut msg, m);

        if err < 0 {
            break;
        }
        // We want each transaction to be as large as possible, but
        // we'll go to smaller sizes if the hardware gives us a
        // short reply.
        let mut transfer_size = dp_aux_i2c_transfer_size as usize;
        let len = m.len as usize;
        let mut j = 0;
        while j < len {
            msg.buffer = j;
            msg.size = transfer_size.min(len - j);

            if j + msg.size == len && aux.no_zero_sized {
                msg.request &= !DP_AUX_I2C_MOT;
            }
            err = drm_dp_i2c_drain_msg(aux, ctx, &msg, m.buf);

            // Reset msg.request in case in case it got
            // changed into a WRITE_STATUS_UPDATE.
            drm_dp_i2c_msg_set_request(&mut msg, m);

            if err < 0 {
                break;
            }
            transfer_size = err as usize;
            j += msg.size;
        }
        if err < 0 {
            break;
        }
    }
    if err >= 0 {
        err = num as isize;
    }

    if !aux.no_zero_sized {
        // Send a bare address packet to close out the transaction.
        // Zero sized messages specify an address only (bare
        // address) transaction.
        msg.request &= !DP_AUX_I2C_MOT;
        msg.buffer = 0;
        msg.size = 0;
        let _ = drm_dp_i2c_do_msg(aux, ctx, &mut msg, &mut none);
    }

    err as i32
}

/// `drm_dp_aux_init`: the AUX channel's I2C adapter (`drm_dp_i2c_algo`,
/// three retries).
pub fn drm_dp_aux_init(aux: &mut drm_dp_aux) {
    aux.ddc.algo = Some(i2c_algo::drm_dp_i2c_algo);
    aux.ddc.retries = 3;
}
