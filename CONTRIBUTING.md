# Contributing

Right now nopeekOS is one person and a lot of evenings, built with
[Claude Code](https://claude.com/claude-code) as a pair programmer. Every
change is tested on real hardware, and the numbers in the docs are
measured, not guessed. Help is welcome: questions, reviews, hardware
reports and code.

## Where things go

| You have… | Go to |
|---|---|
| a question, an idea, a design argument | [Discussions](https://github.com/fnopeek/nopeekOS/discussions) |
| a device you want supported | a [hardware request](https://github.com/fnopeek/nopeekOS/issues/new?template=hardware-request.yml); others vote with 👍 |
| a bug | a [bug report](https://github.com/fnopeek/nopeekOS/issues/new?template=bug-report.yml) |
| a security issue | **privately**, see [SECURITY.md](SECURITY.md) |
| code | a pull request from your fork; please ask in Discussions first for anything larger |

## Licensing note (read before a pull request)

nopeekOS is dual-licensed: AGPL-3.0-or-later and a commercial license.
To keep that possible, a contribution can only be merged under a
contributor license agreement (CLA). For anything beyond a typo, ask in
Discussions before you start, so neither of us spends evenings on a
change that cannot be merged.

## How the code is written

The rules live in [CLAUDE.md](CLAUDE.md) and `docs/spec/`. The short form:

- **Architecture:** capabilities, not permissions; intents, not
  commands; content-addressed, not path-addressed; the WASM sandbox is the
  trust boundary. A change that weakens one of these will not be merged.
- **Rust:** `no_std`, nightly, edition 2024, target `x86_64-nopeek`. No
  POSIX, no libc.
- **`unsafe`:** every block carries a `SAFETY` comment.
- **Comments:** English, minimal, no history; see
  `docs/spec/COMMENTS.md`. The pre-commit hook `tools/hooks/pre-commit`
  checks it.
- **Hardware drivers** are ported from Linux and kept as close to the
  original as possible, with Linux as the reference.
- **Security checkpoint:** before every commit, ask: *can a WASM module escape
  its sandbox through this change?* If the answer is not clearly **no**,
  it does not go in.

## Commit messages

The first line says which update path the change needs:

| Prefix | Meaning |
|---|---|
| `kernel-only:` | `update` is enough |
| `module <name>:` | `install <name>` is needed |
| `abi+kernel:` | kernel and all SDK apps, released together |
| `kernel+module <name>:` | both, because they belong together |
| `docs:` | no release |

## Build and test

```bash
./build.sh build   # compile
./build.sh qemu    # build and run in QEMU
```

Prerequisites and the release flow are in the [README](README.md).
Releases are signed with the project's update key and published by the
maintainer only.

## How the maintainer works

Day-to-day work and releases go straight to `main`: OTA updates are
served from `main/release/`, so `main` is never branch-protected. Pull
requests are for outside contributions.
