// SPDX-License-Identifier: AGPL-3.0-only

//! Magic packets and how the relay sends them (PROTOCOL.md, "Sending"). One UDP socket per command
//! or test, bound to the chosen interface's IPv4 address, with SO_BROADCAST (without it Linux
//! refuses broadcasts) and SO_BINDTODEVICE where the kernel lets an unprivileged process set it
//! (Linux 5.7 and later; elsewhere the address bind alone picks the interface). Destinations are
//! 255.255.255.255 and the interface's own subnet broadcast; ports 9 and/or 7; `repeat` rounds
//! `gap` apart. No unicast, no other ports, no other content.

use crate::netif::Target;
use std::{
    ffi::CString,
    fmt::Write as _,
    io,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    os::fd::AsRawFd,
    time::Duration,
};

pub const PACKET_LEN: usize = 102;

/// Six 0xff bytes and the MAC sixteen times.
pub fn magic_packet(mac: &[u8; 6]) -> [u8; PACKET_LEN] {
    let mut packet = [0xff; PACKET_LEN];
    for i in 0..16 {
        packet[6 + 6 * i..12 + 6 * i].copy_from_slice(mac);
    }
    packet
}

/// `aa:bb:cc:dd:ee:ff` (either case), the form commands and /proc use.
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let bytes = s.as_bytes();
    if bytes.len() != 17 || (2..17).step_by(3).any(|i| bytes[i] != b':') {
        return None;
    }
    parse_hex_groups(s.split(':'))
}

/// Also `aa-bb-cc-dd-ee-ff` (Windows `getmac`) and `aabbccddeeff`, for the command line.
pub fn parse_mac_lenient(s: &str) -> Option<[u8; 6]> {
    let s = s.trim();
    if let Some(mac) = parse_mac(s) {
        return Some(mac);
    }
    let bytes = s.as_bytes();
    if bytes.len() == 17 && (2..17).step_by(3).all(|i| bytes[i] == b'-') {
        return parse_hex_groups(s.split('-'));
    }
    if bytes.len() == 12 && s.is_ascii() {
        return parse_hex_groups((0..12).step_by(2).map(|i| &s[i..i + 2]));
    }
    None
}

fn parse_hex_groups<'a>(groups: impl Iterator<Item = &'a str>) -> Option<[u8; 6]> {
    let mut mac = [0u8; 6];
    let mut n = 0;
    for group in groups {
        if n == 6 || group.len() != 2 || !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        mac[n] = u8::from_str_radix(group, 16).ok()?;
        n += 1;
    }
    (n == 6).then_some(mac)
}

/// A MAC a magic packet may name: an individual address (the multicast bit clear), not all zeros.
pub fn wakeable(mac: &[u8; 6]) -> bool {
    mac[0] & 1 == 0 && *mac != [0; 6]
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    let mut s = String::with_capacity(17);
    for (i, b) in mac.iter().enumerate() {
        if i > 0 {
            s.push(':');
        }
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The MAC as logs show it: only the last two bytes.
pub fn mask_mac(mac: &[u8; 6]) -> String {
    format!("**:**:**:**:{:02x}:{:02x}", mac[4], mac[5])
}

/// Everything one send does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendPlan {
    pub iface: String,
    pub source: Ipv4Addr,
    /// The source address's prefix length (for the log).
    pub prefix: u8,
    pub dests: Vec<Ipv4Addr>,
    pub macs: Vec<[u8; 6]>,
    pub ports: Vec<u16>,
    pub repeat: u32,
    pub gap: Duration,
}

impl SendPlan {
    pub fn new(
        target: &Target,
        macs: &[[u8; 6]],
        ports: &[u16],
        repeat: u32,
        gap: Duration,
    ) -> SendPlan {
        let mut dests = vec![Ipv4Addr::BROADCAST];
        if let Some(b) = target
            .addr
            .broadcast()
            .filter(|b| *b != Ipv4Addr::BROADCAST)
        {
            dests.push(b);
        }
        SendPlan {
            iface: target.iface.clone(),
            source: target.addr.addr,
            prefix: target.addr.prefix,
            dests,
            macs: macs.to_vec(),
            ports: ports.to_vec(),
            repeat,
            gap,
        }
    }

    pub fn packets(&self) -> u32 {
        let per_round = self.macs.len() * self.ports.len() * self.dests.len();
        self.repeat
            .saturating_mul(u32::try_from(per_round).unwrap_or(u32::MAX))
    }

    /// `eth0 10.1.5.20/22 -> 255.255.255.255, 10.1.7.255 포트 9,7 ×3`
    pub fn describe(&self) -> String {
        let dests: Vec<String> = self.dests.iter().map(Ipv4Addr::to_string).collect();
        let ports: Vec<String> = self.ports.iter().map(u16::to_string).collect();
        format!(
            "{} {}/{} -> {} 포트 {} ×{}",
            self.iface,
            self.source,
            self.prefix,
            dests.join(", "),
            ports.join(","),
            self.repeat
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SendOutcome {
    pub planned: u32,
    pub sent: u32,
    /// Whether SO_BINDTODEVICE took; if not, why (the address bind still picks the interface).
    pub device_error: Option<String>,
    /// The first few errors.
    pub errors: Vec<String>,
}

/// Sends `plan` and reports how many packets went out. Blocks for `(repeat - 1) * gap`.
pub fn send(plan: &SendPlan) -> SendOutcome {
    let mut out = SendOutcome {
        planned: plan.packets(),
        ..SendOutcome::default()
    };
    let socket = match UdpSocket::bind(SocketAddrV4::new(plan.source, 0)) {
        Ok(s) => s,
        Err(e) => {
            out.errors.push(format!("bind {}: {e}", plan.source));
            return out;
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        out.errors.push(format!("SO_BROADCAST: {e}"));
        return out;
    }
    if let Err(e) = bind_to_device(&socket, &plan.iface) {
        out.device_error = Some(e.to_string());
    }
    for round in 0..plan.repeat {
        if round > 0 {
            std::thread::sleep(plan.gap);
        }
        for mac in &plan.macs {
            let packet = magic_packet(mac);
            for port in &plan.ports {
                for dest in &plan.dests {
                    match socket.send_to(&packet, SocketAddrV4::new(*dest, *port)) {
                        Ok(n) if n == PACKET_LEN => out.sent += 1,
                        Ok(n) => push_error(&mut out, format!("{dest}:{port}: sent {n} bytes")),
                        Err(e) => push_error(&mut out, format!("{dest}:{port}: {e}")),
                    }
                }
            }
        }
    }
    out
}

fn push_error(out: &mut SendOutcome, e: String) {
    if out.errors.len() < 3 {
        out.errors.push(e);
    }
}

fn bind_to_device(socket: &UdpSocket, iface: &str) -> io::Result<()> {
    let name = CString::new(iface).map_err(io::Error::other)?;
    let len =
        libc::socklen_t::try_from(name.as_bytes_with_nul().len()).map_err(io::Error::other)?;
    // SAFETY: the option value is a NUL-terminated interface name of `len` bytes.
    let rc = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            len,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netif::Ipv4Net;

    #[test]
    fn packet_layout() {
        let mac = [0x00, 0x00, 0x5e, 0x00, 0x53, 0x01];
        let p = magic_packet(&mac);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xff; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + 6 * i..12 + 6 * i], &mac);
        }
    }

    #[test]
    fn macs() {
        let mac = Some([0xaa, 0xbb, 0xcc, 0x0d, 0x0e, 0x0f]);
        assert_eq!(parse_mac("aa:bb:cc:0d:0e:0f"), mac);
        assert_eq!(parse_mac("AA:BB:CC:0D:0E:0F"), mac);
        for bad in [
            "aa-bb-cc-0d-0e-0f",
            "aabbcc0d0e0f",
            "aa:bb:cc:0d:0e",
            "aa:bb:cc:0d:0e:0f:00",
            "aa:bb:cc:0d:0e:0g",
            "+a:bb:cc:0d:0e:0f",
            "aa:bb:cc:0d:0e:0f ",
        ] {
            assert_eq!(parse_mac(bad), None, "{bad:?}");
        }
        assert_eq!(parse_mac_lenient("AA-BB-CC-0D-0E-0F"), mac);
        assert_eq!(parse_mac_lenient(" aabbcc0d0e0f "), mac);
        assert_eq!(parse_mac_lenient("aa:bb:cc:0d:0e:0f"), mac);
        assert_eq!(parse_mac_lenient("aa-bb:cc-0d-0e-0f"), None);
        assert_eq!(parse_mac_lenient("aabbcc0d0e0"), None);
        // twelve bytes that are not twelve ASCII characters
        assert_eq!(parse_mac_lenient("aéaaaaaaaaa"), None);
        assert_eq!(parse_mac_lenient("가나다라"), None);
        assert!(wakeable(&[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]));
        assert!(!wakeable(&[0x01, 0, 0x5e, 0, 0, 1]));
        assert!(!wakeable(&[0xff; 6]));
        assert!(!wakeable(&[0; 6]));
        assert_eq!(
            format_mac(&[0xaa, 0xbb, 0xcc, 0x0d, 0x0e, 0x0f]),
            "aa:bb:cc:0d:0e:0f"
        );
        assert_eq!(
            mask_mac(&[0xaa, 0xbb, 0xcc, 0x0d, 0x53, 0x01]),
            "**:**:**:**:53:01"
        );
    }

    #[test]
    fn plans() {
        let target = Target {
            iface: "eth0".to_owned(),
            addr: "10.1.5.20/22".parse::<Ipv4Net>().unwrap(),
        };
        let plan = SendPlan::new(
            &target,
            &[[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]],
            &[9, 7],
            3,
            Duration::from_millis(200),
        );
        assert_eq!(
            plan.dests,
            [Ipv4Addr::BROADCAST, Ipv4Addr::new(10, 1, 7, 255)]
        );
        assert_eq!(plan.source, Ipv4Addr::new(10, 1, 5, 20));
        assert_eq!(plan.packets(), 12);
        assert_eq!(
            plan.describe(),
            "eth0 10.1.5.20/22 -> 255.255.255.255, 10.1.7.255 포트 9,7 ×3"
        );
        let two = SendPlan::new(
            &target,
            &[[2; 6], [4; 6]],
            &[9],
            5,
            Duration::from_millis(100),
        );
        assert_eq!(two.packets(), 20);
    }

    #[test]
    fn sends_on_loopback() {
        // only to 127.0.0.1: nothing leaves this machine (broadcasts are tested in namespaces)
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let target = Target {
            iface: "lo".to_owned(),
            addr: Ipv4Net::new(Ipv4Addr::LOCALHOST, 8).unwrap(),
        };
        let mac = [0x00, 0x00, 0x5e, 0x00, 0x53, 0x01];
        let mut plan = SendPlan::new(&target, &[mac], &[port], 2, Duration::from_millis(10));
        plan.dests = vec![Ipv4Addr::LOCALHOST];
        let out = send(&plan);
        assert_eq!((out.planned, out.sent), (2, 2), "{out:?}");
        assert!(out.errors.is_empty());
        for _ in 0..2 {
            let mut buf = [0u8; 200];
            let (n, from) = listener.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], &magic_packet(&mac)[..]);
            assert_eq!(from.ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        // a source address this machine does not have: nothing is sent, and it says why
        plan.source = Ipv4Addr::new(192, 0, 2, 1);
        let out = send(&plan);
        assert_eq!(out.sent, 0);
        assert_eq!(out.errors.len(), 1);
    }
}
