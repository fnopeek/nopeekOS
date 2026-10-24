# Security

nopeekOS is experimental and **has not been audited**. Please do not trust
it with real data yet.

## What counts as a vulnerability

Most of all, anything that crosses the boundary the system is built on:

- a WASM module escaping its sandbox, or reaching memory, devices or DMA
  it was not granted
- a capability check that can be bypassed, or rights that grow on delegation
- flaws in the crypto: npkFS encryption, the keyvault, TLS, the passphrase
  keyslots
- anything that gets an unsigned or rolled-back update accepted by `update`

## Reporting

Please report these **privately** via
<https://github.com/fnopeek/nopeekOS/security/advisories/new>, not as a
public issue.

This is one person's project: expect an answer within days, not hours. A
fix ships as a signed OTA release; you will be credited in it unless you
prefer not to be.

Reviews of the sandbox boundary, the capability vault and the crypto are
especially welcome, even without a concrete finding.
