#!/bin/sh
# With `npkgdb` on the guest command line: gdb attaches to every media
# decoder (RDD) process of the browser. When one stops on a fatal signal,
# its backtrace and the load addresses of the libraries in it go to
# /dev/kmsg as `[gdb]` lines; an address in a stripped library is then
# `pc - from + .text address` in that file. With `npkgdb=SECONDS`, a
# process still running after that long is stopped once and the stacks of
# its media threads are written the same way, for a decoder that hangs.
say() { while IFS= read -r l; do echo "<0>[gdb] $l"; done > /dev/kmsg; }
AFTER=$(tr ' ' '\n' < /proc/cmdline | sed -n 's/^npkgdb=\([0-9][0-9]*\)$/\1/p' | head -1)
libs() {
  for lib in $(grep -E '^#[0-9]' "$1" | sed -n 's/.* from \(\/[^ ]*\).*/\1/p' | sort -u); do
    grep -F "$lib" "$1" | grep -E '^0x' | sed "s|^|lib |" | say
  done
}
watch_one() {
  pid=$1 out=/tmp/gdb.$1
  gdb -p "$pid" -batch -nx \
    -ex 'set pagination off' -ex 'set confirm off' -ex 'set width 0' \
    -ex 'handle all nostop noprint pass' \
    -ex 'handle SIGABRT stop print pass' -ex 'handle SIGSEGV stop print pass' \
    -ex 'handle SIGBUS stop print pass' -ex 'handle SIGILL stop print pass' \
    -ex continue -ex 'bt 48' -ex 'echo ===threads===\n' -ex 'thread apply all bt 24' \
    -ex 'info sharedlibrary' > "$out" 2>&1 &
  g=$!
  if [ -n "$AFTER" ]; then
    ( sleep "$AFTER"; kill -INT "$g" 2>/dev/null ) &
  fi
  wait "$g"
  if grep -qE 'received signal (SIGABRT|SIGSEGV|SIGBUS|SIGILL)' "$out"; then
    echo "rdd $pid stopped:" | say
    sed '/^===threads===/,$d' "$out" | grep -E 'received signal|^#[0-9]' | say
    libs "$out"
  elif grep -q 'received signal SIGINT' "$out"; then
    echo "rdd $pid after ${AFTER} s, media threads:" | say
    sed -n '/^===threads===/,$p' "$out" | awk '
      /^Thread [0-9]+ / { keep = ($0 ~ /Media|RemVid|VideoDec/) ; if (keep) print; next }
      /^#[0-9]/ { if (keep) print }' | say
    libs "$out"
  fi
  rm -f "$out"
}
seen=" "
while :; do
  for pid in $(pgrep -f -- '-contentproc.* rdd( |$)'); do
    case "$seen" in *" $pid "*) continue ;; esac
    seen="$seen$pid "
    echo "attaching to rdd $pid" | say
    watch_one "$pid" &
  done
  sleep 0.2
done
