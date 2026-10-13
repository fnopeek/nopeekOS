#!/bin/sh
# One line per second of where the system's time goes: CPU split, context
# switches, page faults, reclaim and compaction, the network receive backlog
# and TCP retransmits. In the guest the session starts it when the host passes
# `npkload` and the lines go to /dev/kmsg; on a host, run it as is and they
# go to stdout, so both sides can be compared line for line.
# kmsg lines need a level the console shows (the guest boots with loglevel=5).
pre=; [ "${1:-}" = /dev/kmsg ] && pre="<0>"
[ -n "${1:-}" ] && exec > "$1"
snap() {
  awk '
    function hex(x,  v, k, c) { v = 0; x = tolower(x)
      for (k = 1; k <= length(x); k++) { c = index("0123456789abcdef", substr(x, k, 1)); v = v * 16 + c - 1 }
      return v }
    FILENAME=="/proc/stat" && $1=="cpu" { u=$2+$3; s=$4; i=$5+$6; q=$7+$8; st=$9; t=0; for(k=2;k<=9;k++) t+=$k }
    FILENAME=="/proc/stat" && $1=="ctxt" { cs=$2 }
    FILENAME=="/proc/vmstat" && $1=="pgfault" { pf=$2 }
    FILENAME=="/proc/vmstat" && $1=="pgmajfault" { mj=$2 }
    FILENAME=="/proc/vmstat" && $1 ~ /^allocstall/ { as+=$2 }
    FILENAME=="/proc/vmstat" && $1=="compact_stall" { cp=$2 }
    FILENAME=="/proc/vmstat" && $1=="thp_fault_alloc" { th=$2 }
    FILENAME=="/proc/vmstat" && $1=="pgscan_direct" { sd=$2 }
    FILENAME=="/proc/net/softnet_stat" { np+=hex($1); nd+=hex($2); sq+=hex($3) }
    FILENAME=="/proc/net/snmp" && $1=="Tcp:" && $2 ~ /^[0-9]/ { rt=$13 }
    END { print t, u, s, i, q, st, cs, pf, mj, as, cp, th, sd, np, nd, sq, rt }
  ' /proc/stat /proc/vmstat /proc/net/softnet_stat /proc/net/snmp
}
# Busiest threads since the last sample: "name/tid=percent of one CPU", the
# sum over all threads, and each online CPU's busy share.
threads() {
  read up _ < /proc/uptime; echo "uptime x $up"
  awk '$1 ~ /^cpu[0-9]/ { t = 0; for (k = 2; k <= 9; k++) t += $k; print "cpu", $1, t, t - $5 - $6 }' /proc/stat
  for f in /proc/[0-9]*/task/[0-9]*/stat; do cat "$f" 2>/dev/null; echo; done | awk '
    NF { a = index($0, "("); b = 0; for (k = length($0); k > 0; k--) if (substr($0, k, 1) == ")") { b = k; break }
         if (a == 0 || b == 0) next
         name = substr($0, a + 1, b - a - 1); gsub(/ /, "_", name)
         split(substr($0, b + 2), r, " ")
         print $1, name, r[12] + r[13] }'
}
top_threads() {
  { echo "$1"; echo "--"; echo "$2"; } | awk -v pre="$pre" -v n="$n" '
    $0 == "--" { cur = 1; next }
    $1 == "uptime" { if (cur) t1 = $3; else t0 = $3; next }
    $1 == "cpu" && !cur { ct[$2] = $3; cb[$2] = $4; next }
    $1 == "cpu" { if ($2 in ct) { dt = $3 - ct[$2]; ncpu++; cpus = cpus sprintf(" %d", dt > 0 ? 100 * ($4 - cb[$2]) / dt : 0) } next }
    !cur { old[$1] = $3; next }
    ($1 in old) { d = $3 - old[$1]; if (d > 0) { busy[$1] = d; nm[$1] = $2; sum += d } }
    END { el = t1 - t0; if (el <= 0) el = 1
          line = sprintf(" cpus=%d busy=[%s ] threads=%d%% |", ncpu, cpus, sum / el); for (k = 0; k < 8; k++) { best = ""; for (t in busy) if (best == "" || busy[t] > busy[best]) best = t
            if (best == "" || busy[best] < 5) break
            line = line sprintf(" %s/%s=%d%%", nm[best], best, busy[best] / el); delete busy[best] }
          printf "%s[load] +%ds top:%s\n", pre, n, line }'
}
prev=$(snap)
tprev=$(threads)
n=0
while sleep 1; do
  n=$((n + 1))
  cur=$(snap)
  echo "$prev $cur" | awk -v n="$n" -v pre="$pre" '{
    h=17; for (k=1;k<=h;k++) d[k]=$(k+h)-$k; t=d[1]>0?d[1]:1
    printf "%s[load] +%ds cpu usr=%d%% sys=%d%% irq=%d%% idle=%d%% steal=%d%% | ctxsw=%d | pgfault=%d major=%d thp=%d | allocstall=%d compact=%d scan=%d | rx pkts=%d drop=%d squeeze=%d | retrans=%d\n",
      pre, n, 100*d[2]/t, 100*d[3]/t, 100*d[5]/t, 100*d[4]/t, 100*d[6]/t, d[7], d[8], d[9], d[12], d[10], d[11], d[13], d[14], d[15], d[16], d[17]
  }'
  prev=$cur
  tcur=$(threads)
  top_threads "$tprev" "$tcur"
  tprev=$tcur
done
