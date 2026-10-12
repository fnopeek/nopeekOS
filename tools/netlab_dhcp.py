#!/usr/bin/env python3
"""Minimal DHCP server for netlab's tap: one client, one fixed lease.

  netlab_dhcp.py <iface> <server_ip> <client_ip> <netmask> [dns_ip]

Answers DISCOVER with OFFER and every REQUEST with ACK for `client_ip`
(router = `server_ip`, DNS = `dns_ip` or `server_ip`), broadcast on `iface`. Enough for nopeekOS,
which needs no lease renewal inside a measurement run.
"""
import socket
import struct
import sys

SO_BINDTODEVICE = 25
MAGIC = b"\x63\x82\x53\x63"


def options(msg_type, server, mask, dns):
    o = bytes([53, 1, msg_type])
    o += bytes([54, 4]) + server
    o += bytes([1, 4]) + mask
    o += bytes([3, 4]) + server
    o += bytes([6, 4]) + dns
    o += bytes([51, 4]) + struct.pack(">I", 86400)
    return o + b"\xff"


def msg_type_of(pkt):
    i = 240
    while i + 1 < len(pkt):
        opt = pkt[i]
        if opt == 255:
            break
        if opt == 0:
            i += 1
            continue
        ln = pkt[i + 1]
        if opt == 53 and ln >= 1:
            return pkt[i + 2]
        i += 2 + ln
    return 0


def main():
    iface, server, client, mask = sys.argv[1:5]
    dns_b = socket.inet_aton(sys.argv[5] if len(sys.argv) > 5 else server)
    server_b = socket.inet_aton(server)
    client_b = socket.inet_aton(client)
    mask_b = socket.inet_aton(mask)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.setsockopt(socket.SOL_SOCKET, SO_BINDTODEVICE, iface.encode())
    s.bind(("0.0.0.0", 67))
    print(f"[dhcp] {iface}: lease {client} via {server}", flush=True)
    while True:
        pkt, _ = s.recvfrom(2048)
        if len(pkt) < 240 or pkt[0] != 1 or pkt[236:240] != MAGIC:
            continue
        t = msg_type_of(pkt)
        reply_type = {1: 2, 3: 5}.get(t)  # DISCOVER→OFFER, REQUEST→ACK
        if reply_type is None:
            continue
        rep = bytearray(240)
        rep[0] = 2                      # BOOTREPLY
        rep[1:3] = pkt[1:3]             # htype, hlen
        rep[4:8] = pkt[4:8]             # xid
        rep[10:12] = pkt[10:12]         # flags
        rep[16:20] = client_b           # yiaddr
        rep[20:24] = server_b           # siaddr
        rep[28:44] = pkt[28:44]         # chaddr
        rep[236:240] = MAGIC
        s.sendto(bytes(rep) + options(reply_type, server_b, mask_b, dns_b),
                 ("255.255.255.255", 68))
        print(f"[dhcp] {'OFFER' if reply_type == 2 else 'ACK'} {client}", flush=True)


if __name__ == "__main__":
    main()
