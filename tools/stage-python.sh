#!/usr/bin/env bash
# Stage the CPython bundle into release/ so the next `build.sh release`
# signs and publishes it.
#
# Python is not built from this repo: `tools/stage-module.sh` compiles a
# Rust crate, and CPython is a C cross-build that needs a wasi sysroot.
# It is built out of tree and lives in
#   ../tools/python-wasi/          (see that directory's README)
# This script only copies.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${1:-$ROOT/../tools/python-wasi}"

[ -f "$SRC/python.wasm" ] || { echo "no python.wasm in $SRC" >&2; exit 1; }

# The version comes from a file next to the artifact, not from grepping
# the binary: OTA compares this string, and it must not shift with the
# byte layout of a build. The zip is named after it (pythonXY.zip), as
# CPython looks for it on its default path.
[ -f "$SRC/VERSION" ] || { echo "no VERSION in $SRC" >&2; exit 1; }
PYVER=$(tr -d '[:space:]' < "$SRC/VERSION")
ZIP="python$(echo "$PYVER" | cut -d. -f1-2 | tr -d .).zip"
[ -f "$SRC/lib/$ZIP" ] || { echo "no lib/$ZIP in $SRC" >&2; exit 1; }

# The zip must be stored, not deflated: this interpreter has no zlib and
# a compressed member raises ZipImportError on the first import.
if ! python3 - "$SRC/lib/$ZIP" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    bad = [i.filename for i in z.infolist() if i.compress_type != zipfile.ZIP_STORED]
sys.exit(1 if bad else 0)
PY
then
    echo "$ZIP has deflated members — rebuild it with ZIP_STORED" >&2
    exit 1
fi

cp "$SRC/python.wasm" "$ROOT/release/modules/python.wasm"
echo "$PYVER" > "$ROOT/release/modules/python.version"
rm -f "$ROOT"/release/assets/python3*.zip "$ROOT"/release/assets/python3*.zip.sig
cp "$SRC/lib/$ZIP" "$ROOT/release/assets/$ZIP"

echo "staged python $PYVER"
echo "  release/modules/python.wasm     $(du -h "$ROOT/release/modules/python.wasm" | cut -f1)"
echo "  release/assets/$ZIP    $(du -h "$ROOT/release/assets/$ZIP" | cut -f1)"
echo "run ./build.sh release to sign + publish"
