#!/bin/sh
# With `npkgdb` on the guest command line: gdb attaches to every media
# decoder (RDD) process of the browser. When one stops on a fatal signal,
# its backtrace and the load addresses of the libraries in it go to
# /dev/kmsg as `[gdb]` lines; an address in a stripped library is then
# `pc - from + .text address` in that file.
say() { while IFS= read -r l; do echo "<0>[gdb] $l"; done > /dev/kmsg; }
watch_one() {
  pid=$1 out=/tmp/gdb.$1
  gdb -p "$pid" -batch -nx \
    -ex 'set pagination off' -ex 'set confirm off' -ex 'set width 0' \
    -ex 'handle all nostop noprint pass' \
    -ex 'handle SIGABRT stop print pass' -ex 'handle SIGSEGV stop print pass' \
    -ex 'handle SIGBUS stop print pass' -ex 'handle SIGILL stop print pass' \
    -ex continue -ex 'bt 48' -ex 'info sharedlibrary' > "$out" 2>&1
  if grep -q '^#0 ' "$out"; then
    echo "rdd $pid stopped:" | say
    grep -E 'received signal|^#[0-9]' "$out" | say
    for lib in $(grep -E '^#[0-9]' "$out" | sed -n 's/.* from \(\/[^ ]*\).*/\1/p' | sort -u); do
      grep -F "$lib" "$out" | grep -E '^0x' | sed "s|^|lib |" | say
    done
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
