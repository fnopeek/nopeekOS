# beak conformance

How well `beak-engine` follows the web standards, measured against the
official test suites rather than judged by eye. Every number here comes from
a run on the host; rerun it and replace the number, never edit it by hand.

Measured 2026-10-27, beak 0.194.1.

## WPT reftests (CSS and form rendering)

The vendored reftests under `tools/wasm/beak-engine/tests/wpt/` render the
test and its `<link rel="match">` reference through the engine and compare
the pixels. A test is inconclusive when its reference renders blank.

```
4586 pass / 1072 fail / 128 inconclusive   (of 5786 reftests)
4586 / 5658 = 81.1 %   of the conclusive tests
4586 / 5199 = 88.2 %   without the tests no engine can win here (below)
```

| Suite | Pass | Conclusive | % |
|---|---|---|---|
| `CSS2` | 3044 | 3297 | 92.3 % |
| `css-align` | 12 | 29 | 41.4 % |
| `css-backgrounds` | 83 | 137 | 60.6 % |
| `css-cascade` | 21 | 31 | 67.7 % |
| `css-color` | 269 | 282 | 95.4 % |
| `css-display` | 50 | 87 | 57.5 % |
| `css-flexbox` | 310 | 418 | 74.2 % |
| `css-fonts` | 23 | 42 | 54.8 % |
| `css-grid` | 331 | 744 | 44.5 % |
| `css-position` | 31 | 35 | 88.6 % |
| `css-sizing` | 77 | 102 | 75.5 % |
| `css-text` | 259 | 354 | 73.2 % |
| `css-values` | 56 | 79 | 70.9 % |
| `html-forms` | 20 | 21 | 95.2 % |

**Tests that are not ours to win** (459, counted once each by
`tests/vehicles.py`, by content rather than file name):

| Kind | Tests | Why |
|---|---|---|
| `grid-lanes` (masonry) | 345 | a proposal no engine ships |
| `writing-mode: vertical` | 42 | a second axis model, not built |
| `display: run-in` | 35 | not shipped by any major engine |
| `subgrid` | 19 | not built |
| CSSTest / Ahem fonts | 18 | the test needs fonts that are not installed |

Run, from the repo root:

```
cargo test --release --manifest-path tools/wasm/beak-engine/Cargo.toml --test wpt -- --nocapture
```

| Variable | Effect |
|---|---|
| `WPT_FILTER=<substr>` | run only matching tests |
| `WPT_DUMP=<dir>` | write `<test>-test.bmp` and `<test>-ref.bmp` for each failure |
| `WPT_BLESS=1` | rewrite `tests/wpt-baseline.tsv` after a deliberate change |
| `WPT_JOBS=<n>` | thread count (default: all cores) |
| `WPT_DIR=<dir>` | measure a larger corpus without committing it |

The run compares against `tests/wpt-baseline.tsv` and lists what was gained
and lost, the biggest failing families and the near misses. After
`WPT_BLESS=1`, `python3 tests/vehicles.py` (from `tools/wasm/beak-engine/`)
prints the two denominators above.

Only rendering is measured this way. WPT tests behaviour (form submission,
DOM, events) through `testharness.js`, which this harness does not run.

## test262 (JavaScript)

The test262 corpus is not in the repo; point `TEST262=` at a checkout.

| Run | Result |
|---|---|
| Parse oracle: does the parser accept exactly the valid programs | 88 656 of 91 523 variants = 96.87 % |
| Execution | 66 970 of 79 325 run = 84.42 % |
| V8 on the same corpus | 99.41 % |

The engine's own percentage says little, because test262 runs ahead of the
shipped engines. The gap that counts is a test V8 passes and beak fails.
The execution denominator leaves out Intl, staging, Temporal and syntax
proposals no engine ships.

```
TEST262=<path> cargo test --release \
    --manifest-path tools/wasm/beak-engine/Cargo.toml --test test262 -- --nocapture
```

`T262_FILTER=<substr>` narrows the run, `T262_SHOW=<n>` prints n failures.

## Rules

1. The spec and a test decide, not a page that looks right.
2. A change that moves a number reruns the suite and updates this file with
   the date and beak version.
3. A deliberate deviation from a standard is named in the code where it is
   made.
