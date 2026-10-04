# Code Comments

A comment explains the code that stands next to it, to a reader who has
only the code. Everything else has a better home.

## Language and form

- **English only.** Code, comments and doc comments.
- Plain prose. No bold, no CAPS for emphasis, no exclamation marks.
- Short: one line is the norm, a paragraph is the exception, a page is a
  design doc and belongs in `docs/`.
- `//!` at the top of a module: what the module is for and its main
  invariant, in a few lines. `///` on items: contract, not implementation.

## What belongs in a comment

- **`SAFETY:`** on every `unsafe` block — the invariant that makes it sound.
- **Invariants and contracts** the type system does not express: units,
  ranges, ownership, ordering, "caller holds X", "must not allocate".
- **The non-obvious why**, in one to three sentences: why this order, why
  this constant, why not the obvious alternative. Write the reason, not
  the story of how it was found.
- **Spec references**: `css-flexbox-1 §4.5`, `RFC 6455 §5.3`,
  `ES2024 15.5.5`, `HTML §4.10.11`.
- **Upstream references** for ported code: `rtw88 phy.c:1221 rtw_phy_...`,
  `Linux i2c-designware-core.c`. Drivers follow Linux 1:1, and the
  reference is how a reader checks that.
- **Hardware facts** tied to the code: "the 8822C ignores this bit",
  "N100 reports a zero here". Name the chip, not our machine.
- **Known limits**: `Not implemented: X.` / `Assumes Y.` — one line.

## What does not belong in a comment

| Content | Where it goes instead |
|---|---|
| Version numbers ("since 0.185.0", "until kernel 0.405.2") | commit message |
| Dates, "measured on …" | commit message, `docs/plan/` |
| Measurements, benchmarks, A/B results | commit message, `docs/plan/` |
| Our devices, network, setup (IdeaPad, repeater, "on the device") | memory, `docs/plan/` |
| People ("Florian said", "per request") | nowhere, or the commit message |
| Debugging stories, "previously this was X", "the bug was" | commit message |
| Design discussion, alternatives weighed, roadmap | `docs/plan/`, `docs/spec/` |
| TODO lists longer than one line | `docs/plan/` |

The test: **would this sentence still be true and useful if the git
history were deleted?** If it only makes sense as history, it is history.

## Examples

```rust
// Bad
// **0.185.0: die Geometrie war das letzte BILD.** DDG misst seinen Kasten
// einmal (gemessen am Geraet, 4 von 219 Lesungen) ...

// Good
// Lay out on demand when script reads geometry of a box that does not
// exist yet; the page may read it only once (e.g. in a mount effect).
```

```rust
// Bad
// Florian's IdeaPad liefert hier 0 — deshalb seit 0.24.0 der Rueckfall.

// Good
// Some firmware reports 0; fall back to the timing Linux computes in
// i2c_dw_acpi_params.
```

## Tooling

`tools/comments.py` (Rust, Python, shell and TOML; `tools/wasm/vendor/` excluded;
for shell and TOML only full-line `#` comments, for Python also docstrings):

- `lint --staged` — runs in the pre-commit hook (`tools/hooks/pre-commit`);
  fails on comment blocks touched by the commit that break these rules.
  Product names (NUC, IdeaPad) are reported as info only.
- `lint --rev HEAD --warn` — the same against the working tree; run by
  `./build.sh build`, never fails the build.
- `audit [--md FILE]` — every violation in the tree, per file.
- `verify [REV]` — proves that a change touched comments only: the code
  token stream must be identical to `REV`.
- `archive DIR` — dumps every comment block with its location.

The pre-cleanup comments are archived in `docs/archive/comments/` and
under the git tag `comments-archive-2026-10`.
