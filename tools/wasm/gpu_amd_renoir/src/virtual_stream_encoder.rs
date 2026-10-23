//! display/dc/virtual/virtual_stream_encoder.c (Linux 6.18.55): the virtual
//! stream encoder. A virtual link has no real DIG, so its encoder is a bare
//! `stream_encoder` base carrying only the virtual funcs identity and
//! ENGINE_ID_VIRTUAL. `resource_construct` appends `num_virtual_links` of these
//! after the real DIG encoders.
#![allow(dead_code)]

use crate::dc_types::engine_id;
use crate::dcn20_stream_encoder::{stream_encoder, stream_encoder_funcs};
use alloc::boxed::Box;

/// `virtual_stream_encoder_construct` (virtual_stream_encoder.c): set the
/// virtual funcs and the virtual engine id. Linux also stores ctx and bp; the
/// port does not keep those (see `clock_source`).
pub fn virtual_stream_encoder_construct(enc: &mut stream_encoder) {
    enc.funcs = Some(stream_encoder_funcs::virtual_str_enc_funcs);
    enc.id = engine_id::ENGINE_ID_VIRTUAL;
}

/// `virtual_stream_encoder_create` (virtual_stream_encoder.c).
pub fn virtual_stream_encoder_create() -> Box<stream_encoder> {
    let mut enc = Box::new(stream_encoder::default());
    virtual_stream_encoder_construct(&mut enc);
    enc
}
