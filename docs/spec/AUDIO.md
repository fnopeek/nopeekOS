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
