# Settings

System settings have one path: `kernel/src/settings.rs`. The loop's `set`,
`theme` and `mouse` and the apps' hostcalls all validate, store and apply
a key there, so a key means the same thing wherever it is changed.

## The table

A key in the table has one check (the values it takes) and one apply step
(what changes at once). A key outside the table is a debug or driver knob:
the loop stores it as typed, an app can neither read nor write it.

| Key | Values | Apps | Takes effect |
|---|---|---|---|
| `theme` | `dark`, `light`, `auto` | read, write | scenes re-rasterized, redraw |
| `accent` | `auto`, `rose`, `sage`, `blue`, `amber`, `#RRGGBB` | read, write | as `theme` |
| `wallpaper` | a file name in `pictures/wallpapers`, extension optional | read, write | decoded by the `wallpaper` module, stored once it worked |
| `shade.scale` | `1.0` to `3.0` | read, write | rescale, redraw |
| `code.scheme` | `auto` or a known scheme | read, write | as `theme` |
| `brightness` | `0` to `100` | read, write | the GPU module sets the backlight; not stored |
| `keyboard` | `de_CH`, `de_DE`, `us` | read, write | next key |
| `mouse_speed` | `25` to `600` | read, write | next movement |
| `mouse_size` | `50` to `300` | read, write | next frame |
| `lang` | `en`, `de` | read, write | apps that ask for the locale |
| `timezone` | a zone name, a POSIX TZ string or an offset, up to 64 characters | read, write | clock, ESP copy |
| `net.allow_plain_http` | as `config` lists | read | loop only |
| `net.lan_devices` | as `config` lists | read | loop only |

Keys that lower a security guarantee are read-only for apps. A value or
key holding a control character is refused for everyone (the config is
stored as `key=value` lines).

`wallpaper` and `brightness` apply as a kernel task: the call returns once
the value is checked, the change follows. A newer value replaces one still
queued, so a dragged slider costs one apply.

## Calls

| Call | Right | Contract |
|---|---|---|
| `npk_setting_get(key, buf) -> len` | RENDER | The value; 0 when it has none yet (brightness before its first read), `-(needed + 2)` for a short buffer, -1 for a key outside the table. |
| `npk_setting_set(key, value) -> 0` | SETTINGS | -1 without the right or for a key apps cannot write, -2 for a value the key does not take. Keys up to 64, values up to 256 bytes. |

`SETTINGS` is bit `0x20` of the second `.npk.caps` byte. Only the settings
app declares it.
