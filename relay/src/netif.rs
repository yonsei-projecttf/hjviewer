// SPDX-License-Identifier: AGPL-3.0-only

//! The relay's network interfaces (PROTOCOL.md, "Interfaces"): which ones it may send on, which one
//! a command's network picks, and what it reports to the server.
//!
//! The rules come from the namespace experiments of the design notes: a magic packet must leave
//! through the interface on the PC's wire, so the relay binds to that interface's address (an
//! unbound socket sends 255.255.255.255 out of the default route's interface, possibly another
//! LAN), and it sends to the interface's own subnet broadcast, never to the PC's (a NAS with a
//! longer prefix than the PC would route the PC's broadcast address to the gateway).

use crate::{protocol::IfaceReport, wake};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, ffi::CStr, fmt, fs, io, net::Ipv4Addr, str::FromStr};

pub const FLAG_UP: u32 = libc::IFF_UP as u32;
pub const FLAG_BROADCAST: u32 = libc::IFF_BROADCAST as u32;
pub const FLAG_LOOPBACK: u32 = libc::IFF_LOOPBACK as u32;
pub const FLAG_POINTOPOINT: u32 = libc::IFF_POINTOPOINT as u32;
pub const FLAG_RUNNING: u32 = libc::IFF_RUNNING as u32;

/// Name prefixes of interfaces that are never a LAN port: container and VM bridges, tunnels and
/// VPNs. (`lo` is matched exactly.) DSM's own names, `eth0`, `bond0` and the Open vSwitch bridge
/// `ovs_eth0` that Virtual Machine Manager adds, are all allowed.
pub const EXCLUDED_PREFIXES: &[&str] = &[
    "docker",
    "br-",
    "veth",
    "virbr",
    "lxcbr",
    "lxdbr",
    "cni",
    "flannel",
    "cali",
    "kube",
    "tun",
    "tap",
    "ppp",
    "wg",
    "tailscale",
    "zt",
    "nebula",
    "dummy",
    "sit",
    "gre",
    "erspan",
    "ip6",
    "vxlan",
];

/// At most this many interfaces and addresses per interface are reported.
const MAX_REPORTED_IFACES: usize = 16;
const MAX_REPORTED_ADDRS: usize = 8;

/// An IPv4 address with a prefix length: an interface address, or a network when the host bits
/// are zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ipv4Net {
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Net {
    pub fn new(addr: Ipv4Addr, prefix: u8) -> Option<Ipv4Net> {
        (prefix <= 32).then_some(Ipv4Net { addr, prefix })
    }

    pub fn mask(&self) -> u32 {
        match self.prefix {
            0 => 0,
            p => u32::MAX << (32 - u32::from(p)),
        }
    }

    pub fn network(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.addr) & self.mask())
    }

    /// Whether the host bits are zero.
    pub fn is_network(&self) -> bool {
        self.addr == self.network()
    }

    /// The subnet broadcast address; none for /31 and /32.
    pub fn broadcast(&self) -> Option<Ipv4Addr> {
        (self.prefix <= 30).then(|| Ipv4Addr::from(u32::from(self.addr) | !self.mask()))
    }

    /// Whether `other` (its network) lies inside this network.
    pub fn contains(&self, other: &Ipv4Net) -> bool {
        self.prefix <= other.prefix
            && u32::from(other.addr) & self.mask() == u32::from(self.network())
    }
}

impl FromStr for Ipv4Net {
    type Err = String;

    /// `a.b.c.d/prefix`, prefix 0-32 in plain decimal digits.
    fn from_str(s: &str) -> Result<Ipv4Net, String> {
        let (addr, prefix) = s
            .split_once('/')
            .ok_or_else(|| format!("{s:?} is not a.b.c.d/prefix"))?;
        let addr: Ipv4Addr = addr
            .parse()
            .map_err(|_| format!("{s:?}: bad IPv4 address"))?;
        let prefix =
            (!prefix.is_empty() && prefix.len() <= 2 && prefix.bytes().all(|b| b.is_ascii_digit()))
                .then(|| prefix.parse::<u8>().ok())
                .flatten()
                .and_then(|p| Ipv4Net::new(addr, p))
                .ok_or_else(|| format!("{s:?}: bad prefix length"))?;
        Ok(prefix)
    }
}

impl fmt::Display for Ipv4Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

/// One network interface (a device; alias labels like `eth0:1` are folded into their device).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub flags: u32,
    pub mac: Option<[u8; 6]>,
    pub addrs: Vec<Ipv4Net>,
}

impl Iface {
    pub fn up(&self) -> bool {
        self.flags & FLAG_UP != 0
    }

    pub fn running(&self) -> bool {
        self.flags & FLAG_RUNNING != 0
    }
}

pub fn excluded_name(name: &str) -> bool {
    name == "lo" || EXCLUDED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Addresses the relay may bind to and broadcast from: prefix 8-30, not loopback, link-local,
/// multicast or otherwise special.
pub fn usable_addr(a: &Ipv4Net) -> bool {
    (8..=30).contains(&a.prefix)
        && !a.addr.is_loopback()
        && !a.addr.is_link_local()
        && !a.addr.is_unspecified()
        && !a.addr.is_multicast()
        && !a.addr.is_broadcast()
        && a.broadcast() != Some(a.addr)
        && !a.is_network()
}

/// Why an interface is not used, or `None` when the relay may send on it.
pub fn ineligible_reason(iface: &Iface) -> Option<&'static str> {
    if excluded_name(&iface.name) {
        Some("name")
    } else if iface.flags & (FLAG_LOOPBACK | FLAG_POINTOPOINT) != 0 {
        Some("loopback/point-to-point")
    } else if !iface.up() {
        Some("down")
    } else if iface.flags & FLAG_BROADCAST == 0 {
        Some("no broadcast")
    } else if !iface.addrs.iter().any(usable_addr) {
        Some("no IPv4")
    } else {
        None
    }
}

pub fn eligible(iface: &Iface) -> bool {
    ineligible_reason(iface).is_none()
}

/// How an interface address relates to a command's network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    /// Same network and prefix.
    Exact,
    /// The interface's network contains the command's (the interface has the shorter prefix).
    Contains,
    /// The interface's network lies inside the command's (the interface has the longer prefix,
    /// e.g. a NAS with /24 on a PC's /22 wire).
    Within,
}

impl Match {
    pub fn name(self) -> &'static str {
        match self {
            Match::Exact => "exact",
            Match::Contains => "contains",
            Match::Within => "within",
        }
    }
}

/// An interface and the address on it to send from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub iface: String,
    pub addr: Ipv4Net,
}

/// The interface address to wake a PC on `net` from: of the eligible interfaces' usable addresses
/// that overlap `net`, those on an interface with carrier (a port without a cable sends into
/// nothing, though the kernel takes the packets), then an exact match first, then one that
/// contains `net`, then one inside it; among equals the closest prefix length, then the
/// interface name.
pub fn select(ifaces: &[Iface], net: &Ipv4Net) -> Option<(Target, Match)> {
    let mut best: Option<(bool, Match, u8, &str, Ipv4Net)> = None;
    for iface in ifaces.iter().filter(|i| eligible(i)) {
        for addr in iface.addrs.iter().filter(|a| usable_addr(a)) {
            let matched = if addr.prefix == net.prefix && addr.network() == net.network() {
                Match::Exact
            } else if addr.contains(net) {
                Match::Contains
            } else if net.contains(addr) {
                Match::Within
            } else {
                continue;
            };
            let key = (
                !iface.running(),
                matched,
                addr.prefix.abs_diff(net.prefix),
                iface.name.as_str(),
                *addr,
            );
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
    }
    best.map(|(_, matched, _, name, addr)| {
        (
            Target {
                iface: name.to_owned(),
                addr,
            },
            matched,
        )
    })
}

/// Every eligible interface with its first usable address (test-wake without `--net`).
pub fn all_targets(ifaces: &[Iface]) -> Vec<Target> {
    ifaces
        .iter()
        .filter(|i| eligible(i))
        .filter_map(|i| {
            i.addrs.iter().find(|a| usable_addr(a)).map(|addr| Target {
                iface: i.name.clone(),
                addr: *addr,
            })
        })
        .collect()
}

/// The device's interfaces, from getifaddrs(3) (netlink, so it reflects the calling thread's
/// network namespace).
pub fn list() -> io::Result<Vec<Iface>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we free below.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut map: BTreeMap<String, Iface> = BTreeMap::new();
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list getifaddrs returned, which stays valid until freed.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_name.is_null() {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string.
        let label = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_string_lossy();
        let device = label.split(':').next().unwrap_or_default().to_owned();
        if device.is_empty() {
            continue;
        }
        let iface = map.entry(device.clone()).or_insert_with(|| Iface {
            name: device,
            ..Iface::default()
        });
        iface.flags |= ifa.ifa_flags;
        if ifa.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: ifa_addr points to a sockaddr whose family says which struct it is.
        match i32::from(unsafe { (*ifa.ifa_addr).sa_family }) {
            libc::AF_INET => {
                // SAFETY: an AF_INET address is a sockaddr_in.
                let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                let addr = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let prefix = if ifa.ifa_netmask.is_null() {
                    None
                } else {
                    // SAFETY: the netmask of an AF_INET address is a sockaddr_in too.
                    let mask = unsafe { &*(ifa.ifa_netmask as *const libc::sockaddr_in) };
                    prefix_of(u32::from_be(mask.sin_addr.s_addr))
                };
                if let Some(net) = prefix.and_then(|p| Ipv4Net::new(addr, p)) {
                    if !iface.addrs.contains(&net) {
                        iface.addrs.push(net);
                    }
                }
            }
            libc::AF_PACKET => {
                // SAFETY: an AF_PACKET address is a sockaddr_ll.
                let ll = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_ll) };
                if ll.sll_halen == 6 {
                    let mut mac = [0u8; 6];
                    mac.copy_from_slice(&ll.sll_addr[..6]);
                    if mac != [0; 6] {
                        iface.mac = Some(mac);
                    }
                }
            }
            _ => {}
        }
    }
    // SAFETY: `head` came from getifaddrs and is freed once.
    unsafe { libc::freeifaddrs(head) };
    Ok(map.into_values().collect())
}

/// The prefix length of a contiguous netmask.
fn prefix_of(mask: u32) -> Option<u8> {
    let ones = mask.leading_ones();
    (ones + mask.trailing_zeros() == 32 || mask == u32::MAX).then_some(ones as u8)
}

/// An interface's default gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gateway {
    pub iface: String,
    pub ip: Ipv4Addr,
    pub mac: Option<[u8; 6]>,
}

/// Default routes in /proc/net/route text: (interface, gateway, metric).
pub fn parse_default_routes(text: &str) -> Vec<(String, Ipv4Addr, u32)> {
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let (Some(dest), Some(gw), Some(flags), Some(mask)) =
            (hex(f[1]), hex(f[2]), hex(f[3]), hex(f[7]))
        else {
            continue;
        };
        // RTF_UP | RTF_GATEWAY on 0.0.0.0/0
        if dest != 0 || mask != 0 || flags & 0x3 != 0x3 {
            continue;
        }
        let metric = f[6].parse().unwrap_or(u32::MAX);
        // the kernel prints the address's bytes as a host-order integer
        out.push((f[0].to_owned(), Ipv4Addr::from(gw.to_ne_bytes()), metric));
    }
    out
}

/// Complete entries of /proc/net/arp text: (address, MAC, interface).
pub fn parse_arp(text: &str) -> Vec<(Ipv4Addr, [u8; 6], String)> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 6 {
            continue;
        }
        let flags = u32::from_str_radix(f[2].trim_start_matches("0x"), 16).unwrap_or(0);
        // ATF_COM: the entry is complete
        if flags & 0x2 == 0 {
            continue;
        }
        if let (Ok(ip), Some(mac)) = (f[0].parse(), wake::parse_mac(f[3])) {
            if mac != [0; 6] {
                out.push((ip, mac, f[5].to_owned()));
            }
        }
    }
    out
}

/// Each interface's default gateway (the lowest metric) with its MAC when the ARP cache has it.
pub fn gateways(route: &str, arp: &str) -> Vec<Gateway> {
    let mut best: BTreeMap<String, (u32, Ipv4Addr)> = BTreeMap::new();
    for (iface, ip, metric) in parse_default_routes(route) {
        let e = best.entry(iface).or_insert((metric, ip));
        if metric < e.0 {
            *e = (metric, ip);
        }
    }
    let arp = parse_arp(arp);
    best.into_iter()
        .map(|(iface, (_, ip))| Gateway {
            mac: arp
                .iter()
                .find(|(a, _, dev)| *a == ip && *dev == iface)
                .map(|(_, mac, _)| *mac),
            iface,
            ip,
        })
        .collect()
}

/// A /proc/net file of the calling thread's network namespace (falls back to the process's).
pub fn read_proc_net(name: &str) -> String {
    fs::read_to_string(format!("/proc/thread-self/net/{name}"))
        .or_else(|_| fs::read_to_string(format!("/proc/net/{name}")))
        .unwrap_or_default()
}

pub fn system_gateways() -> Vec<Gateway> {
    gateways(&read_proc_net("route"), &read_proc_net("arp"))
}

/// The interfaces the relay may send on, as reported to the server, sorted by name.
pub fn reports(ifaces: &[Iface], gateways: &[Gateway]) -> Vec<IfaceReport> {
    let mut out: Vec<IfaceReport> = ifaces
        .iter()
        .filter(|i| eligible(i))
        .map(|i| {
            let mut addrs: Vec<Ipv4Net> = i.addrs.iter().copied().filter(usable_addr).collect();
            addrs.sort();
            addrs.truncate(MAX_REPORTED_ADDRS);
            let gw = gateways.iter().find(|g| g.iface == i.name);
            IfaceReport {
                name: i.name.clone(),
                mac: i.mac.map(|m| wake::format_mac(&m)),
                ipv4: addrs.iter().map(Ipv4Net::to_string).collect(),
                up: i.up(),
                running: i.running(),
                gateway: gw.map(|g| g.ip.to_string()),
                gateway_mac: gw.and_then(|g| g.mac).map(|m| wake::format_mac(&m)),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(MAX_REPORTED_IFACES);
    out
}

/// Lowercase hex SHA-256 of the reports' JSON, so the server can tell when they changed.
pub fn reports_hash(reports: &[IfaceReport]) -> String {
    let json = serde_json::to_string(reports).unwrap_or_default();
    hex(&Sha256::digest(json.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> Ipv4Net {
        s.parse().unwrap()
    }

    fn iface(name: &str, addrs: &[&str]) -> Iface {
        Iface {
            name: name.to_owned(),
            flags: FLAG_UP | FLAG_BROADCAST | FLAG_RUNNING,
            mac: Some([0x02, 0, 0, 0, 0, 1]),
            addrs: addrs.iter().map(|a| net(a)).collect(),
        }
    }

    #[test]
    fn nets() {
        let n = net("10.1.5.20/22");
        assert_eq!(n.network(), Ipv4Addr::new(10, 1, 4, 0));
        assert_eq!(n.broadcast(), Some(Ipv4Addr::new(10, 1, 7, 255)));
        assert!(!n.is_network());
        assert!(net("10.1.4.0/22").contains(&net("10.1.5.0/24")));
        assert!(net("10.1.4.0/22").contains(&n));
        assert!(!net("10.1.5.0/24").contains(&net("10.1.4.0/22")));
        assert!(!net("10.0.0.0/8").contains(&net("11.0.0.0/8")));
        assert_eq!(net("10.1.2.3/31").broadcast(), None);
        assert_eq!(net("0.0.0.0/0").mask(), 0);
        assert_eq!(net("1.2.3.4/32").mask(), u32::MAX);
        for bad in [
            "10.0.0.0",
            "10.0.0.0/33",
            "10.0.0.0/+8",
            "10.0.0/8",
            "10.0.0.0/",
            "10.0.0.0/008",
            "x/8",
        ] {
            assert!(bad.parse::<Ipv4Net>().is_err(), "{bad:?} parsed");
        }
        assert_eq!(prefix_of(0xffff_fc00), Some(22));
        assert_eq!(prefix_of(0), Some(0));
        assert_eq!(prefix_of(u32::MAX), Some(32));
        assert_eq!(prefix_of(0xff00_ff00), None);
    }

    #[test]
    fn eligibility() {
        assert!(eligible(&iface("eth0", &["10.1.5.20/22"])));
        assert!(eligible(&iface("ovs_eth0", &["10.1.5.20/24"])));
        assert!(eligible(&iface("bond0", &["10.1.5.20/22"])));
        for name in [
            "lo",
            "docker0",
            "docker-1a2b",
            "br-1a2b3c",
            "veth12",
            "tun1000",
            "tailscale0",
            "wg0",
            "ppp200",
            "sit0",
        ] {
            assert_eq!(
                ineligible_reason(&iface(name, &["10.1.5.20/22"])),
                Some("name"),
                "{name}"
            );
        }
        let mut down = iface("eth1", &["10.0.0.5/24"]);
        down.flags &= !FLAG_UP;
        assert_eq!(ineligible_reason(&down), Some("down"));
        let mut p2p = iface("eth1", &["10.0.0.5/24"]);
        p2p.flags |= FLAG_POINTOPOINT;
        assert!(!eligible(&p2p));
        let mut nobcast = iface("eth1", &["10.0.0.5/24"]);
        nobcast.flags &= !FLAG_BROADCAST;
        assert_eq!(ineligible_reason(&nobcast), Some("no broadcast"));
        for addrs in [
            &[][..],
            &["169.254.3.4/16"],
            &["10.0.0.5/32"],
            &["10.0.0.5/31"],
            &["10.0.0.0/24"],
            &["10.0.0.255/24"],
            &["127.0.0.1/8"],
            &["10.1.2.3/7"],
        ] {
            assert_eq!(
                ineligible_reason(&iface("eth1", addrs)),
                Some("no IPv4"),
                "{addrs:?}"
            );
        }
        // running is reported, not required
        let mut nocarrier = iface("eth1", &["10.0.0.5/24"]);
        nocarrier.flags &= !FLAG_RUNNING;
        assert!(eligible(&nocarrier));
    }

    #[test]
    fn selection_prefers_exact_then_contains_then_within() {
        let lan = iface("eth0", &["10.1.5.20/22"]);
        let other = iface("eth1", &["192.168.50.10/24"]);
        let pc = net("10.1.4.0/22");
        let (t, m) = select(&[other.clone(), lan.clone()], &pc).unwrap();
        assert_eq!(
            (t.iface.as_str(), t.addr, m),
            ("eth0", net("10.1.5.20/22"), Match::Exact)
        );
        // the NAS has /24 on the PC's /22 wire (ovs_eth0 when VMM is installed)
        let ovs = iface("ovs_eth0", &["10.1.5.20/24"]);
        let (t, m) = select(&[other.clone(), ovs.clone()], &pc).unwrap();
        assert_eq!((t.iface.as_str(), m), ("ovs_eth0", Match::Within));
        // the PC has a longer prefix than the NAS
        let (t, m) = select(std::slice::from_ref(&lan), &net("10.1.5.0/24")).unwrap();
        assert_eq!((t.iface.as_str(), m), ("eth0", Match::Contains));
        // exact beats contains beats within, whatever the order
        let wide = iface("eth2", &["10.1.0.9/16"]);
        let narrow = iface("eth3", &["10.1.5.5/25"]);
        for list in [
            vec![wide.clone(), narrow.clone(), lan.clone()],
            vec![lan.clone(), narrow.clone(), wide.clone()],
        ] {
            assert_eq!(select(&list, &pc).unwrap().0.iface, "eth0");
        }
        assert_eq!(
            select(&[narrow.clone(), wide.clone()], &pc).unwrap().1,
            Match::Contains
        );
        assert_eq!(
            select(std::slice::from_ref(&narrow), &pc).unwrap().1,
            Match::Within
        );
        // nothing on the network
        assert_eq!(select(std::slice::from_ref(&other), &pc), None);
        assert_eq!(
            select(std::slice::from_ref(&lan), &net("172.16.0.0/12")),
            None
        );
        // a container bridge on the very network is never used
        let docker = iface("docker0", &["10.1.4.2/22"]);
        assert_eq!(select(std::slice::from_ref(&docker), &pc), None);
        assert_eq!(select(&[docker, other, lan], &pc).unwrap().0.iface, "eth0");
        // one interface, several addresses: the matching one is used
        let multi = iface("eth0", &["192.168.1.2/24", "10.1.5.20/22"]);
        assert_eq!(select(&[multi], &pc).unwrap().0.addr, net("10.1.5.20/22"));
    }

    #[test]
    fn selection_prefers_a_port_with_carrier() {
        // two ports on the PC's network; eth0 lost its cable but kept its static address
        let mut unplugged = iface("eth0", &["10.1.5.20/22"]);
        unplugged.flags &= !FLAG_RUNNING;
        let plugged = iface("eth1", &["10.1.5.21/22"]);
        let pc = net("10.1.4.0/22");
        let (t, m) = select(&[unplugged.clone(), plugged.clone()], &pc).unwrap();
        assert_eq!((t.iface.as_str(), m), ("eth1", Match::Exact));
        // even over a better match
        let within = iface("eth1", &["10.1.5.21/24"]);
        let (t, m) = select(&[unplugged.clone(), within], &pc).unwrap();
        assert_eq!((t.iface.as_str(), m), ("eth1", Match::Within));
        // and when it is the only one, it is still used
        let (t, _) = select(std::slice::from_ref(&unplugged), &pc).unwrap();
        assert_eq!(t.iface, "eth0");
    }

    #[test]
    fn all_targets_takes_each_eligible_interface_once() {
        let list = [
            iface("eth0", &["10.1.5.20/22", "10.0.0.2/24"]),
            iface("docker0", &["172.17.0.1/16"]),
            iface("eth1", &["192.168.50.10/24"]),
            iface("eth2", &[]),
        ];
        let targets = all_targets(&list);
        let names: Vec<_> = targets
            .iter()
            .map(|t| (t.iface.as_str(), t.addr.to_string()))
            .collect();
        assert_eq!(
            names,
            [
                ("eth0", "10.1.5.20/22".to_owned()),
                ("eth1", "192.168.50.10/24".to_owned())
            ]
        );
    }

    const ROUTE: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
eth0\t00000000\t0104A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
eth1\t00000000\t0132A8C0\t0003\t0\t0\t50\t00000000\t0\t0\t0
eth0\t00000000\t0204A8C0\t0003\t0\t0\t10\t00000000\t0\t0\t0
eth0\t0004A8C0\t00000000\t0001\t0\t0\t0\t00FCFFFF\t0\t0\t0
eth2\t00000000\t0164A8C0\t0001\t0\t0\t0\t00000000\t0\t0\t0
";
    const ARP: &str =
        "IP address       HW type     Flags       HW address            Mask     Device
192.168.4.2      0x1         0x2         02:00:00:aa:bb:cc     *        eth0
192.168.50.1     0x1         0x0         00:00:00:00:00:00     *        eth1
192.168.4.1      0x1         0x2         02:00:00:00:00:01     *        eth1
";

    #[test]
    fn proc_net_parsing() {
        if cfg!(target_endian = "big") {
            return;
        }
        let routes = parse_default_routes(ROUTE);
        assert_eq!(routes.len(), 3);
        assert_eq!(
            routes[0],
            ("eth0".to_owned(), Ipv4Addr::new(192, 168, 4, 1), 100)
        );
        let gws = gateways(ROUTE, ARP);
        assert_eq!(
            gws,
            [
                Gateway {
                    iface: "eth0".to_owned(),
                    ip: Ipv4Addr::new(192, 168, 4, 2),
                    mac: Some([0x02, 0x00, 0x00, 0xaa, 0xbb, 0xcc]),
                },
                Gateway {
                    iface: "eth1".to_owned(),
                    ip: Ipv4Addr::new(192, 168, 50, 1),
                    mac: None,
                },
            ]
        );
        assert!(parse_arp("").is_empty());
        assert!(parse_default_routes("garbage\nmore garbage").is_empty());
    }

    #[test]
    fn reports_list_eligible_interfaces_with_gateways() {
        let list = [
            iface("eth1", &["192.168.50.10/24"]),
            iface("eth0", &["192.168.5.20/22", "169.254.1.1/16"]),
            iface("docker0", &["172.17.0.1/16"]),
        ];
        let gws = [Gateway {
            iface: "eth0".to_owned(),
            ip: Ipv4Addr::new(192, 168, 4, 1),
            mac: Some([0x02, 0x00, 0x00, 0, 0, 0xfe]),
        }];
        let r = reports(&list, &gws);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].name, "eth0");
        assert_eq!(r[0].ipv4, ["192.168.5.20/22"]);
        assert_eq!(r[0].mac.as_deref(), Some("02:00:00:00:00:01"));
        assert_eq!(r[0].gateway.as_deref(), Some("192.168.4.1"));
        assert_eq!(r[0].gateway_mac.as_deref(), Some("02:00:00:00:00:fe"));
        assert_eq!(r[1].gateway, None);
        let h = reports_hash(&r);
        assert_eq!(h.len(), 64);
        assert_eq!(h, reports_hash(&reports(&list, &gws)));
        assert_ne!(h, reports_hash(&reports(&list[..1], &gws)));
    }

    #[test]
    fn lists_this_machines_interfaces() {
        let list = list().unwrap();
        assert!(list.iter().any(|i| i.name == "lo"));
        assert!(list.iter().all(|i| !i.name.contains(':')));
    }
}
