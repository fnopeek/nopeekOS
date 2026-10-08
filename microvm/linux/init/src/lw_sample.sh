#!/bin/sh
# Samples the app process ($1) from its start: threads, page faults, CPU
# time, and time run vs. waiting for a CPU, as [lw-proc] lines on
# /dev/kmsg. Started by the session only when the host passes npkstats.
p=$1
t=0
for d in 0.1 0.1 0.2 0.4 0.4; do
  sleep $d
  t=$((t + ${d#0.}00))
  [ -r /proc/$p/stat ] || exit 0
  set -- $(cat /proc/$p/stat)
  comm=$2; minflt=${10}; majflt=${12}; ut=${14}; st=${15}
  set -- $(cat /proc/$p/schedstat 2>/dev/null)
  run=$(($1 / 1000000)); wait=$(($2 / 1000000))
  thr=$(grep -m1 '^Threads:' /proc/$p/status | tr -dc 0-9)
  echo "<0>[lw-proc] +${t}ms $comm threads=$thr minflt=$minflt majflt=$majflt user=$((ut * 10))ms sys=$((st * 10))ms run=${run}ms runqueue=${wait}ms" > /dev/kmsg
done
