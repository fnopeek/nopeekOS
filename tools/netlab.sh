#!/bin/bash
# Internet-like link without root: two user-owned network namespaces joined by
# a veth pair, each end shaped with netem (one-way delay + rate + queue).
#
#   client ns (QEMU, curl)  10.99.0.1 --veth-- 10.99.0.2  server ns (netbench :80)
#
#   netlab.sh up [delay_ms] [rate] [queue_ms]   start (default 10 ms, 1gbit, 5 ms)
#   netlab.sh shell                             shell in the client ns
#   netlab.sh qemu [target]                     ./build.sh <target> in the client ns (qemu-gui)
#   netlab.sh base                              native baseline: curl down/up
#   netlab.sh down                              stop everything
#
# nopeekOS: `set benchhost 10.99.0.2` then `microvm benchvm 1000`.
# The browser VM reaches it too: http://10.99.0.2/get?mb=500
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
STATE=${XDG_RUNTIME_DIR:-/tmp}/netlab

client_pid() { cat "$STATE/client.pid"; }
server_pid() { cat "$STATE/server.pid"; }
in_client() { nsenter --preserve-credentials -U -n -t "$(client_pid)" "$@"; }
in_server() { nsenter --preserve-credentials -U -n -t "$(server_pid)" "$@"; }

shape() { # <ns-runner> <dev> <delay_ms> <rate> <limit_pkts>
    local run=$1 dev=$2 d=$3 r=$4 l=$5
    $run ip link set dev "$dev" gso_max_segs 1 2>/dev/null || true
    $run tc qdisc replace dev "$dev" root netem delay "${d}ms" rate "$r" limit "$l"
}

case "${1:-}" in
up)
    delay=${2:-10}; rate=${3:-1gbit}; qms=${4:-5}
    mkdir -p "$STATE"
    # Client ns: owns the user ns; a sleeper keeps it alive.
    unshare -rn --kill-child sleep infinity & echo $! > "$STATE/client.pid"
    sleep 0.3
    # Server ns: a second net ns inside the same user ns.
    in_client unshare -n --kill-child sleep infinity & sleep 0.3
    spid=$(pgrep -n -f "^unshare -n --kill-child sleep infinity")
    pgrep -P "$spid" sleep > "$STATE/server.pid"
    # Windows big enough for 1 Gbit at 20 ms (BDP 2.5 MB) and more; the
    # namespace defaults would cap TCP itself below the link.
    for run in in_client in_server; do
        $run sh -c 'echo "4096 131072 33554432" > /proc/sys/net/ipv4/tcp_rmem
                    echo "4096 16384 33554432" > /proc/sys/net/ipv4/tcp_wmem'
        $run ip link set lo up
    done
    # slirp forwards the guest's ping through an unprivileged ICMP socket.
    in_client sh -c 'echo "0 0" > /proc/sys/net/ipv4/ping_group_range'
    in_client ip link add veth-c type veth peer name veth-s
    in_client ip link set veth-s netns "$(server_pid)"
    in_client ip addr add 10.99.0.1/24 dev veth-c
    in_server ip addr add 10.99.0.2/24 dev veth-s
    in_client ip link set veth-c up
    in_server ip link set veth-s up
    # Packets in flight one way (BDP) plus a queue of qms at line rate.
    rbits=$(python3 -c "import re,sys;s='$rate'.lower();m=re.match(r'([\d.]+)([kmg]?)bit',s);print(int(float(m[1])*{'':1,'k':1e3,'m':1e6,'g':1e9}[m[2]]))")
    limit=$(( rbits / 8 / 1514 * (delay + qms) / 1000 + 64 ))
    shape in_client veth-c "$delay" "$rate" "$limit"
    shape in_server veth-s "$delay" "$rate" "$limit"
    in_server python3 "$REPO/tools/netbench_server.py" 80 > "$STATE/server.log" 2>&1 &
    echo $! > "$STATE/netbench.pid"
    sleep 0.5
    echo "netlab up: RTT $((2 * delay)) ms, $rate each way, queue ${qms} ms (limit $limit pkts)"
    in_client ping -c 2 -q 10.99.0.2 | tail -1
    ;;
shell) in_client bash ;;
qemu)  in_client env -C "$REPO" ./build.sh "${2:-qemu-gui}" ;;
base)
    mb=${2:-500}
    echo "download ${mb} MiB:"
    in_client curl -s -o /dev/null -w '  %{speed_download} B/s in %{time_total}s\n' "http://10.99.0.2/get?mb=$mb"
    echo "upload ${mb} MiB:"
    head -c $((mb << 20)) /dev/zero | in_client curl -s -X POST --data-binary @- http://10.99.0.2/upload; echo
    ;;
log)  tail -n "${2:-40}" "$STATE/server.log" ;;
down)
    for f in netbench server client; do
        [ -f "$STATE/$f.pid" ] && kill "$(cat "$STATE/$f.pid")" 2>/dev/null || true
    done
    rm -rf "$STATE"; echo "netlab down"
    ;;
*) sed -n '2,15p' "$0"; exit 1 ;;
esac
