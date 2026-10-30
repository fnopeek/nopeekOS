# Root CA anchors shipped as data

The `.der` files here are Mozilla's TLS root CAs, written by
`tools/certs/mozilla.py` from NSS `certdata.txt` as Firefox release ships it.
Do not edit them by hand: run the script, review the diff, release.

`./build.sh release` signs every file and lists it in the asset manifest as
`[cert:<filename>]`. The kernel stores it as `sys/certs/<filename>` and trusts
it after `update`; a root that leaves Mozilla's list is removed from devices by
the next `update`. The installer bundles the same list.

The roots needed to reach the update host are compiled into the kernel
(`kernel/certs/`, `kernel/src/crypto/tls/certstore.rs`), so nothing here can
lock a machine out of its own updates. `./build.sh release` refuses to sign
while an update host no longer chains to one of them.

A private or company CA belongs on the device: `cert add` puts it in
`sys/certs/local/`, which updates never touch.
