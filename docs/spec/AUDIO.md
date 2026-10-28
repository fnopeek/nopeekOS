# Audio

The kernel's audio mailbox and mixer (`kernel/src/audio.rs`) and the host
calls around it. The kernel knows no audio hardware; a driver module
(`audio_hda`) pulls one mixed stream and plays it.

## Streams (sources)

| Call | Contract |
|---|---|
| `npk_audio_open() -> slot` | A new stream of the caller, S16LE, 48 kHz, stereo. -1 when the caller already holds 8 streams (a safety cap, logged). The number of streams is otherwise unbounded. |
| `npk_audio_format(slot, rate, encoding, channels) -> 0 / -1` | The stream's format from now on; empties its ring. `encoding`: 0 = S16LE, 1 = S32LE, 2 = F32LE (full scale is ±1.0). `rate`: 8000 to 384000 Hz. `channels`: 1 or 2; mono plays on both front channels. |
| `npk_audio_submit(slot, ptr, len) -> bytes` | Appends interleaved PCM in the stream's format. Takes fewer bytes when the ring is full; the caller retries. The ring holds about 1.36 s of the stream's format. |
| `npk_audio_buffered(slot) -> bytes` | Bytes not yet mixed: the play clock. |
| `npk_audio_close(slot) -> 0 / -1` | Ends the stream and frees its ring. Streams of a module that ends are closed for it. |

Only the module that opened a stream may use it.

## Device (driver)

| Call | Contract |
|---|---|
| `npk_audio_device_format(rate, encoding, channels) -> 0 / -1` | What `npk_audio_poll_mix` produces from now on; S16LE, 48 kHz, stereo until set. `channels` 1 to 8. Needs `Rights::HARDWARE`. |
| `npk_audio_poll_mix(ptr, max) -> bytes` | Fills whole frames of the device format: every stream converted to it and summed, scaled by the master volume, clamped. Silence where no stream has data. Needs `Rights::HARDWARE`. |

Conversion: samples become full-scale 32-bit, then the rate is converted by
linear interpolation between neighbouring source frames (as ALSA's
`linear` rate plugin). A stereo source fills device channels 0 and 1, the
others stay silent; a mono device gets the average of left and right. A
device encoding wider than 16 bits gets the sample in the most significant
bits (HDA's 20 and 24 bit containers).

## HDA driver

`audio_hda` probes every codec on the link (STATESTS), parses each codec's
pins and widget graph as Linux `snd-hda-codec-generic` does, and drives
the first codec with an analog output, else the first with any output
(Intel display codecs through `intelhdmi.rs`). The device format is the
best the chosen converters take: 48 kHz if supported, 16 bits if
supported, stereo unless the converter is mono.

## Display audio (component)

What Linux' audio component carries between the display driver and the
audio driver. Every call needs `Rights::HARDWARE`; the display side is the
driver of a PCI class 03 device, the audio side of a class 04 device.

| Call | Side | Contract |
|---|---|---|
| `npk_display_audio_set(port, ptr, len) -> 0 / -1` | display | The ELD of `port`'s sink, `len` 0 for none (`pin_eld_notify`). |
| `npk_display_audio_get(port, ptr, max) -> len` | audio | The ELD of `port`, 0 for no sink, `-(needed + 2)` for a short buffer (`get_eld`). |
| `npk_display_audio_generation() -> n` | both | 0 until the display side reported once, then a count that moves with every report. |
| `npk_display_audio_serve(done) -> want` | display | Binds the component and says request `done` is carried out; returns `(number << 2) \| bits` of what is wanted now. Bit 0: hold the display's audio power (`get_power` / `put_power`). Bit 1: force the codec wake signal (`codec_wake_override`). The display driver calls it on every pass of its service loop, and again after carrying out a new number. |
| `npk_display_audio_request(bits) -> number` | audio | Asks for `bits` (as above) instead of what was asked before, and wakes the display driver. -2 while no display driver is bound, -3 when the bus has no Intel display device, so none will bind (`i915_gfx_present`); -1 refused. |
| `npk_display_audio_done() -> number` | audio | The last request carried out; -2 while no display driver is bound. |

What an audio module asked for is given back when it ends. When the
display driver ends, nothing it carried out holds; a new one carries out
what is still wanted. `audio_hda` waits up to 60 s for the component to
bind, as `snd_hdac_i915_init` does, then holds the power and forces the
codec wake signal around the controller reset (`hda_intel_init_chip`), so
the display codec is present on the link however the boot went.
