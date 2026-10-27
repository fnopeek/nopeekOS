# docs/

What is here describes nopeekOS as it is built today, written from the code.
Plans, history and measurements are not kept here: open work is in the
issues, history and measurements are in `git log`.

## spec/

Contracts the code follows. A deviation between a spec and the code is a
bug in one of them; the change that moves the code updates the spec too.

| File | What it covers |
|---|---|
| [`COMMENTS.md`](spec/COMMENTS.md) | Rules for code comments, enforced by `tools/comments.py` |
| [`WIDGET_VOCAB.md`](spec/WIDGET_VOCAB.md) | The `nopeek_widgets` vocabulary for building apps |
| [`UI_REFRESH.md`](spec/UI_REFRESH.md) | Design tokens: colours, spacing, type |
| [`PANEL.md`](spec/PANEL.md) | Panels at the screen edge (`deck`), popups |
| [`WIFI_CLASS_ABI.md`](spec/WIFI_CLASS_ABI.md) | Contract between `link`, `wifid` and the vendor WiFi drivers |
| [`AX200_FUNC_MAP.md`](spec/AX200_FUNC_MAP.md) | Which `iwlwifi-mvm` functions the AX200 driver ports |
| [`BROWSER.md`](spec/BROWSER.md) | `beak`, the native browser |
| [`CONFORMANCE.md`](spec/CONFORMANCE.md) | `beak` against the official test suites |

## Rules

1. **Describe what is, not what was planned.** A spec is written from the
   code. What the code does not do yet is an issue, not a paragraph.
2. **One fact, one place.** Versions live in `Cargo.toml`, test counts in
   `spec/CONFORMANCE.md`, history in `git log`. What is written twice drifts.
3. **Code may point at a spec, an upstream source or an issue (`#N`)**, never
   at a plan or a paper.
