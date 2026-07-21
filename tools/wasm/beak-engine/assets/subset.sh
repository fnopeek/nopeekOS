#!/usr/bin/env bash
# Re-subset the six embedded faces. Run from this directory, pointing at the
# upstream originals (Inter, Noto Sans Mono) — the checked-in .ttf files here
# are ALREADY subsetted output, so re-running this on them is a no-op.
#
# Why: fontdue outlines every cmap-reachable glyph at Font::from_bytes (it has
# no lazy path), so every glyph in the file is startup cost and wasm size. The
# upstream files carried 19516 glyphs, ~5000 of which no web page can reach:
# Inter's 863 private-use icon glyphs (U+E000-E2DC, U+EE01-EEE1), astral-plane
# symbols, historic Cyrillic/Latin (U+A640-A7FF), variation selectors.
#
# The range below keeps everything a page CAN render: Latin (incl. Extended
# Additional/Vietnamese), IPA, Greek incl. polytonic, Cyrillic, all punctuation,
# currency, arrows, math, box drawing, geometric shapes and misc symbols, plus
# the fi/fl ligatures and U+FFFD. Neither font ever had CJK, Arabic, Hebrew or
# Indic, so nothing renderable was lost. 3271 KB -> 1322 KB, 19516 -> 14524.
#
# GSUB/GPOS are dropped: beak rasterises by char, never by glyph index, so
# fontdue's substitution pass (FontSettings::load_substitutions) has nothing to
# contribute — see fonts.rs.
set -euo pipefail

SRC="${1:?usage: subset.sh <dir-with-upstream-ttfs>}"
UNICODES="U+0000-2E7F,U+FB00-FB4F,U+FEFF,U+FFFC-FFFD"

for f in inter inter-bold inter-italic inter-bolditalic mono mono-bold; do
    pyftsubset "$SRC/$f.ttf" \
        --unicodes="$UNICODES" \
        --layout-features='' \
        --drop-tables+=GSUB,GPOS \
        --no-hinting \
        --output-file="$f.ttf"
    printf '%-18s %s\n' "$f.ttf" "$(du -h "$f.ttf" | cut -f1)"
done
