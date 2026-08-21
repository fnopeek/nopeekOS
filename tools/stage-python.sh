#!/usr/bin/env bash
# Stage the CPython bundle into release/ so the next `build.sh release`
# signs and publishes it.
#
# Python is not built from this repo: `tools/stage-module.sh` compiles a
# Rust crate, and CPython is a C cross-build that takes ~20 minutes and
# needs a wasi sysroot. It is built once, out of tree, and lives in
#   ../tools/python-wasi/          (see that directory's README)
# This script only copies. Nothing here compiles anything.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${1:-$ROOT/../tools/python-wasi}"

[ -f "$SRC/python.wasm" ] || { echo "no python.wasm in $SRC" >&2; exit 1; }
[ -f "$SRC/lib/python313.zip" ] || { echo "no lib/python313.zip in $SRC" >&2; exit 1; }

# The zip MUST be stored, not deflated: this interpreter has no zlib and
# a compressed member raises ZipImportError on the first import. Check it
# here rather than discovering it on the device.
if ! python3 - "$SRC/lib/python313.zip" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[1]) as z:
    bad = [i.filename for i in z.infolist() if i.compress_type != zipfile.ZIP_STORED]
sys.exit(1 if bad else 0)
PY
then
    echo "python313.zip has deflated members — rebuild it with ZIP_STORED" >&2
    exit 1
fi

# The version comes from a file next to the artifact, not from grepping
# the binary: OTA compares this string, and a version that shifts because
# a build happened to lay bytes out differently is worse than no version.
[ -f "$SRC/VERSION" ] || { echo "no VERSION in $SRC" >&2; exit 1; }
PYVER=$(tr -d '[:space:]' < "$SRC/VERSION")

cp "$SRC/python.wasm" "$ROOT/release/modules/python.wasm"
echo "$PYVER" > "$ROOT/release/modules/python.version"
cp "$SRC/lib/python313.zip" "$ROOT/release/assets/python313.zip"

echo "staged python $PYVER"
echo "  release/modules/python.wasm     $(du -h "$ROOT/release/modules/python.wasm" | cut -f1)"
echo "  release/assets/python313.zip    $(du -h "$ROOT/release/assets/python313.zip" | cut -f1)"
echo "run ./build.sh release to sign + publish"
