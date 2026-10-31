# CLAUDE.md

Guide for working on nopeekOS with Claude Code. Human contributors: see
[CONTRIBUTING.md](CONTRIBUTING.md) first; this file is the same rules,
written for the agent.

## What nopeekOS is

An operating system rethought from scratch. Not a Unix clone, not POSIX,
no legacy. The vision and architecture are in [README.md](README.md).

## Architecture principles (do not violate)

1. **Capabilities, not permissions.** No chmod, no ACLs, no root.
2. **Intents, not commands.** Express an intention, not instructions.
3. **Content-addressed, not path-addressed.** No filesystem tree.
4. **Runtime-generated, not pre-installed.** Tools are built on demand.
5. **Formally bounded.** The WASM sandbox is the trust boundary.

## Repository layout

| Path | Content |
|---|---|
| `kernel/` | the kernel (`no_std` Rust) |
| `forge/` | WASM to x86-64 compiler, host-testable |
| `tools/wasm/` | WASM modules: drivers, apps, the `beak` browser, the SDK |
| `tools/` | build, staging, codegen and test scripts |
| `targets/` | `x86_64-nopeek.json`, the kernel's own target |
| `microvm*/` | the Linux microVM (guest kernel, userspace) |
| `release/` | the signed OTA tree that devices update from |
| `docs/spec/` | living contracts; the code follows them, a deviation is a bug |

## Code rules

- Rust, `no_std`, nightly (pinned in `rust-toolchain.toml`), edition 2024.
- Target `x86_64-nopeek`: bare metal with SSE, AVX2 and AES-NI. Do not go
  back to overriding features on `x86_64-unknown-none`; that contradicts
  its softfloat ABI and rustc is turning it into a hard error.
- No POSIX, no libc, no std in the kernel.
- Every resource is capability-gated.
- A panic is a kernel panic and halts; there is no recovery path.
- Every `unsafe` block has a `SAFETY` comment.
- Serial is the primary I/O, not the framebuffer.
- Comments are English, minimal and carry no history. Rules in
  `docs/spec/COMMENTS.md`, enforced by `tools/comments.py` through the
  pre-commit hook `tools/hooks/pre-commit` (link it to
  `.git/hooks/pre-commit`).
- Hardware drivers are ported from the Linux source and stay as close to
  it as possible. Port completely instead of debugging a partial port.
- Security checkpoint before every commit: *can a WASM module escape its
  sandbox through this change?* If the answer is not clearly no, do not
  commit.

## Build and run

```bash
./build.sh build          # compile kernel + bootable ISO
./build.sh qemu           # build + QEMU (KVM, serial)
./build.sh qemu-gui       # build + QEMU with framebuffer
./build.sh debug          # build + QEMU with GDB stub on :1234
./build.sh qemu-intel     # TCG with an Intel CPU model (also qemu-amd)
./build.sh installer      # two-pass installer build with bundled assets
./build.sh usb /dev/sdX   # installer on a USB stick
```

The full list is in the header of `build.sh`.

WASM modules are built and staged with `tools/stage-module.sh <mod>...`,
which also writes the module's `.version`. Host-side tests run with
`cargo test` inside the crate (e.g. `tools/wasm/beak-engine`,
`forge/core`).

## Commit messages

The first line says which update path a device needs:

| Prefix | Meaning |
|---|---|
| `kernel-only:` | `update` is enough |
| `module <name>:` | `install <name>` is needed |
| `abi+kernel:` | kernel and all SDK apps, released together |
| `kernel+module <name>:` | both, because they belong together |
| `docs:` | no release |

## Releases (maintainer only)

Devices update over the air from `main/release/`, so releases go straight
to `main` and `main` is never branch-protected. A source change without a
signed release means every `update` silently delivers the last signed
release instead.

```bash
# bump the version, then sync the lock (builds run --locked)
cargo update --offline -p nopeekos-kernel
./build.sh build
git commit -m "..."                 # the source change
./build.sh release                  # sign kernel + modules into release/
git add release/ && git commit -m "release: sign + publish vX.Y.Z"
git push
```

Which number to bump (kernel and modules alike):

| Bump | When | Example |
|---|---|---|
| `Y` (0.794.0 to 0.795.0) | something new: a feature, a new optimisation step, a security fix | locals in registers |
| `Z` (0.794.0 to 0.794.1) | a follow-up to the last release that adds nothing new: a bug in it, a side effect, a forgotten piece | the new step allocated too much |

A follow-up and a new step go out as two releases, so a device
measurement can tell which change did what.

`./build.sh release` does not compile WASM: stage changed modules first.
For a module-only change, `./build.sh sign-modules` re-signs only the
modules whose hash changed and leaves the kernel untouched.

## Tracking bugs, security findings and docs

One finding, one ticket. Nothing open lives only in a document.

| What | Where | Closed by |
|---|---|---|
| bug | public issue, label `bug` | the fix commit with `Fixes #N` |
| hardening (crafted input only, not reachable from a module) | public issue, label `hardening` | the fix commit with `Fixes #N` |
| vulnerability (sandbox, capabilities, crypto, update path) | **private** draft security advisory, never a public issue | publishing the advisory after the fix is released |
| contract the code follows | `docs/spec/` | kept up to date |
| current state, history, measurements | commit messages, `git log` | |

A security fix:

1. Draft advisory with impact, location and fix plan.
2. Fix and release as usual. The commit message names the change, not
   how to exploit it, and references the advisory ID.
3. Set the patched version in the advisory and publish it.

A review produces tickets, not a paper: each confirmed finding becomes an
issue or an advisory as above. Code comments may reference a spec, an
upstream source or an issue (`#N`); never an internal document.

## Where the state lives

- `git log` is the history and the current state; there is no status
  section here.
- Open work is in the issues; open vulnerabilities in the security
  advisories.
- `docs/spec/` holds the contracts that code refers to.
