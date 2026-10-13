#!/bin/sh
# One line per second of where the system's time goes: CPU split, context
# switches, page faults, reclaim and compaction, the network receive backlog
# and TCP retransmits. In the guest the session starts it when the host passes
# `npkload` and the lines go to /dev/kmsg; on a host, run it as is and they
# go to stdout, so both sides can be compared line for line.
out=${1:-/dev/stdout}
# kmsg lines need a level the console shows (the guest boots with loglevel=5).
pre=; [ "$out" = /dev/kmsg ] && pre="<0>"
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
prev=$(snap)
n=0
while sleep 1; do
  n=$((n + 1))
  cur=$(snap)
  echo "$prev $cur" | awk -v n="$n" -v pre="$pre" '{
    h=17; for (k=1;k<=h;k++) d[k]=$(k+h)-$k; t=d[1]>0?d[1]:1
    printf "%s[load] +%ds cpu usr=%d%% sys=%d%% irq=%d%% idle=%d%% steal=%d%% | ctxsw=%d | pgfault=%d major=%d thp=%d | allocstall=%d compact=%d scan=%d | rx pkts=%d drop=%d squeeze=%d | retrans=%d\n",
      pre, n, 100*d[2]/t, 100*d[3]/t, 100*d[5]/t, 100*d[4]/t, 100*d[6]/t, d[7], d[8], d[9], d[12], d[10], d[11], d[13], d[14], d[15], d[16], d[17]
  }' > "$out"
  prev=$cur
done
