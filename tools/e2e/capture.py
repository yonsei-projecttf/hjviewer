#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Prints the Wake-on-LAN magic packets that reach a network interface.

    python3 -I capture.py <interface> <MAC aa:bb:cc:dd:ee:ff, or any> [<seconds>]

A magic packet is six bytes ff and then the card's MAC sixteen times, anywhere in an Ethernet
frame: that is all a sleeping card looks for, so this looks for the same, on a packet socket,
which also sees the frames of an interface without an IP address (a PC that sleeps). It prints
"listening <interface>" once it sees every frame from then on, one line for each frame with the
magic packet of <MAC> (with `any`, of any MAC: six bytes ff and then one six bytes sixteen
times), and the count at the end, after <seconds> or when it gets SIGTERM or SIGINT:

    listening eth0
    1760000000.123 magic eth_dst=ff:ff:ff:ff:ff:ff src=192.0.2.20:40001 dst=255.255.255.255:9 udp_len=102
    1 magic packet(s)

`udp_len` is the UDP payload's length, 102 when the datagram is the magic packet and nothing else.
An IPv4 frame that is not UDP shows its `ip_proto` instead of the ports, and any other frame its
`ethertype` alone.

It needs CAP_NET_RAW: root, or root of the user namespace that owns the interface's network
namespace. The WOL E2E (tools/e2e/wol-e2e.sh) runs it on the sleeping PC's card and in the other
LAN. On a Linux machine in a PC's LAN, `sudo python3 -I capture.py eth0 <the PC's MAC>` shows
whether a relay's packets arrive there (frames this machine sends itself are left out).
"""
import signal
import socket
import struct
import sys
import time

ETH_P_ALL = 0x0003
ETH_P_IP = 0x0800
ETH_P_8021Q = 0x8100
UDP = 17


def describe(frame):
    """The frame's destination MAC and, for IPv4 UDP, its addresses and payload length."""
    out = ["eth_dst=" + frame[0:6].hex(":")]
    ethertype = struct.unpack("!H", frame[12:14])[0]
    ip = 14
    if ethertype == ETH_P_8021Q and len(frame) >= 18:
        ethertype = struct.unpack("!H", frame[16:18])[0]
        ip = 18
    header = (frame[ip] & 0x0F) * 4 if len(frame) > ip else 0
    udp = ip + header
    if ethertype != ETH_P_IP or header < 20 or len(frame) < udp + 8:
        out.append("ethertype=0x%04x" % ethertype)
        return " ".join(out)
    src = socket.inet_ntoa(frame[ip + 12:ip + 16])
    dst = socket.inet_ntoa(frame[ip + 16:ip + 20])
    if frame[ip + 9] == UDP:
        sport, dport, length = struct.unpack("!HHH", frame[udp:udp + 6])
        out += ["src=%s:%d" % (src, sport), "dst=%s:%d" % (dst, dport), "udp_len=%d" % (length - 8)]
    else:
        # such as the ICMP answer of a host that is up, which quotes the datagram it refused
        out += ["src=%s" % src, "dst=%s" % dst, "ip_proto=%d" % frame[ip + 9]]
    return " ".join(out)


def has_magic(frame, mac):
    """Whether the frame holds the magic packet of `mac`, or of any MAC when it is None."""
    if mac is not None:
        return b"\xff" * 6 + mac * 16 in frame
    start = 0
    while True:
        i = frame.find(b"\xff" * 6, start)
        if i < 0 or len(frame) < i + 102:
            return False
        if frame[i + 6:i + 102] == frame[i + 6:i + 12] * 16:
            return True
        start = i + 1


def main(argv):
    if len(argv) not in (3, 4):
        sys.stderr.write(__doc__)
        return 2
    ifname = argv[1]
    try:
        mac = None if argv[2] == "any" else bytes.fromhex(argv[2].replace(":", "").replace("-", ""))
        seconds = float(argv[3]) if len(argv) == 4 else None
    except ValueError:
        mac, seconds = b"", None
    if mac is not None and len(mac) != 6:
        sys.stderr.write("capture.py: the MAC is six bytes, as aa:bb:cc:dd:ee:ff (or any), and the time a number\n")
        return 2
    done = []
    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, lambda *_: done.append(True))
    try:
        sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
        sock.bind((ifname, 0))
    except OSError as e:
        sys.stderr.write("capture.py: a packet socket on %s: %s (it needs CAP_NET_RAW: root)\n" % (ifname, e))
        return 1
    sock.settimeout(0.1)
    print("listening " + ifname, flush=True)
    end = None if seconds is None else time.monotonic() + seconds
    seen = 0
    while not done and (end is None or time.monotonic() < end):
        try:
            frame, address = sock.recvfrom(65535)
        except socket.timeout:
            continue
        except OSError:
            # the interface is down for now (ENETDOWN): it may come up again
            time.sleep(0.1)
            continue
        if address[2] == socket.PACKET_OUTGOING or len(frame) < 14 or not has_magic(frame, mac):
            continue
        seen += 1
        print("%.3f magic %s" % (time.time(), describe(frame)), flush=True)
    print("%d magic packet(s)" % seen, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
