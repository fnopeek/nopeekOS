# Root CA anchors shipped as data

Every DER file here is signed by `./build.sh release` and listed in the asset
manifest as `[cert:<filename>]`. The kernel maps that section to
`sys/certs/<filename>` and loads it as a trust anchor — no kernel change, no
reinstall. Users get it with `update`.

**Adding a root CA means the whole system trusts everything it signs.** That is
a security boundary, not a bugfix. Before dropping a file here:

```
openssl x509 -inform DER -in <file> -noout -subject -enddate -fingerprint -sha256
```

and check the fingerprint against the CA's own published value — from their
site, not from wherever the file came from.

The anchors needed to reach the update host are compiled into the kernel
(`kernel/src/crypto/tls/certstore.rs`). That floor cannot be removed from here,
so a mistake in this directory can never lock a machine out of its own updates.

For a one-off private or self-signed CA, use `cert add` on the device instead —
this directory is for anchors that ship to everyone.
