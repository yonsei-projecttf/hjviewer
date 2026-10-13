// SPDX-License-Identifier: AGPL-3.0-only

//! Wake-on-LAN facts and the judgements hjv-api makes of them (design v3 §8-2, §8-4): the wake
//! report a bound PC sends in its sync, as hjv-api reads and keeps it; the readiness verdict and
//! its hints, rules that live here so that they change without a new app; the interfaces a relay
//! reports (relay/PROTOCOL.md §9); and how a relay's networks are matched with a PC's.
//!
//! **The PC's report** (`wol` of the sync's payload, client patch 0035): facts only, with every
//! value a PC did not learn as null:
//!
//! ```text
//! {"v":1,
//!  "adapters":[{"mac":"00:00:5e:00:53:10","ipv4":["192.0.2.67/24"],"gateway":"192.0.2.1",
//!    "gateway_mac":"00:00:5e:00:53:01","desc":"…","bus":"pci","link_mbps":1000,
//!    "driver":{"provider":…,"version":…,"date":…},
//!    "wake":{"magic":"1","pattern":"1","ms_magic":null,"s5":"1","shutdown_speed":"0","eee":"0","green":"0"},
//!    "armed":true}],
//!  "power":{"s3":true,"s4":true,"s5":true,"hiberfile":true,"aoac":false,"fast_startup":false,"on_ac":true},
//!  "uptime_s":3600}
//! ```
//!
//! [`parse_report`] takes what has its form and leaves the rest out: an adapter without a MAC that
//! could be sent to (an individual address, not all zeros) is left out, an address that is not
//! `a.b.c.d/p` is left out, texts are cut to [`MAX_TEXT`] characters, at most [`MAX_ADAPTERS`]
//! adapters with [`MAX_ADDRESSES`] addresses each; `uptime_s` is not kept. A report that is not an
//! object with `"v":1` is not taken at all. The apps never get a MAC: they get the verdict, its
//! hints and the relay's name (wake.rs).
//!
//! **Matching** ([`judge`]): a relay can wake a PC on a network when one of its interfaces is on
//! that network, by these grades (design v3 §8-4):
//!
//! 1. `exact`: the same network and prefix, and the same gateway IP;
//! 2. `overlap`: one network holds the other (a NAS with a /24 on the PC's /22 wire), and the same
//!    gateway IP or the same gateway MAC: the relay's own broadcast reaches the PC (experiment
//!    T10), while the PC's broadcast address would leave the NAS through its gateway (T11);
//! 3. `history`: the relay woke this PC on this network before (a confirmed job) and still has
//!    an address on it, whatever the gateways are now.
//!
//! On a private or shared (CGNAT) network the PC's and the relay's egress addresses must be the
//! same too (an IPv6 address by its /64): such networks repeat from one site to the next. A gateway
//! that is not known matches nothing.

use crate::util::{address_group, clean_text};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::net::{IpAddr, Ipv4Addr};

/// The adapters taken from a report, and the addresses of each.
pub const MAX_ADAPTERS: usize = 4;
pub const MAX_ADDRESSES: usize = 4;
/// The longest text kept of a report's value.
pub const MAX_TEXT: usize = 64;
/// The interfaces taken from a relay's report (PROTOCOL.md §9.1), and the addresses of each.
pub const MAX_IFACES: usize = 16;
pub const MAX_IFACE_ADDRESSES: usize = 8;
/// The prefixes of the addresses a relay uses and reports (PROTOCOL.md §9.1: 8 to 30). A relay
/// reports its interfaces itself, a pending one too: an address with another prefix is not one a
/// relay would send from, and one as wide as `/0` would overlap every PC's network.
pub const IFACE_PREFIXES: std::ops::RangeInclusive<u8> = 8..=30;

// ---- addresses ----

/// An IPv4 address with its prefix, as `192.0.2.67/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net4 {
    addr: u32,
    prefix: u8,
}

fn mask(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

impl Net4 {
    /// `a.b.c.d/p` exactly: dotted decimal and a prefix 0 to 32 in plain decimal digits.
    pub fn parse(text: &str) -> Option<Net4> {
        let (addr, prefix) = text.split_once('/')?;
        if prefix.is_empty() || prefix.len() > 2 || !prefix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let prefix: u8 = prefix.parse().ok().filter(|p| *p <= 32)?;
        let addr: Ipv4Addr = addr.parse().ok()?;
        Some(Net4 {
            addr: u32::from(addr),
            prefix,
        })
    }

    pub fn addr(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.addr)
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }

    /// The network: the address with its host bits zero.
    pub fn network(&self) -> Net4 {
        Net4 {
            addr: self.addr & mask(self.prefix),
            prefix: self.prefix,
        }
    }

    /// `network/prefix`, as commands carry it (PROTOCOL.md §7.1: host bits zero).
    pub fn canonical(&self) -> String {
        self.network().to_string()
    }

    /// The same network and prefix.
    pub fn same_network(&self, other: &Net4) -> bool {
        self.network() == other.network()
    }

    /// One network holds the other (networks that overlap at all do).
    pub fn overlaps(&self, other: &Net4) -> bool {
        let m = mask(self.prefix.min(other.prefix));
        self.addr & m == other.addr & m
    }

    /// A network whose addresses repeat from one site to the next: RFC 1918, the shared address
    /// space of carrier-grade NAT (100.64.0.0/10) and link-local addresses.
    pub fn is_private(&self) -> bool {
        let n = self.network().addr;
        [
            (0x0a00_0000, 8),  // 10.0.0.0/8
            (0xac10_0000, 12), // 172.16.0.0/12
            (0xc0a8_0000, 16), // 192.168.0.0/16
            (0x6440_0000, 10), // 100.64.0.0/10
            (0xa9fe_0000, 16), // 169.254.0.0/16
        ]
        .iter()
        .any(|(block, len)| n & mask(*len) == *block && self.prefix >= *len)
    }
}

impl std::fmt::Display for Net4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.addr(), self.prefix)
    }
}

/// A MAC address written `00:00:5e:00:53:01` (or with `-`, any case).
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = text.split([':', '-']).collect();
    if parts.len() != 6 || text.contains(':') && text.contains('-') {
        return None;
    }
    let mut mac = [0u8; 6];
    for (byte, part) in mac.iter_mut().zip(&parts) {
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

pub fn mac_text(mac: &[u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A MAC that a magic packet can be meant for (PROTOCOL.md §7.1): an individual address (the
/// lowest bit of the first byte clear), not all zeros.
pub fn wakeable(mac: &[u8; 6]) -> bool {
    mac[0] & 1 == 0 && *mac != [0; 6]
}

/// A MAC as the admin output shows it: its last two bytes only (`**:**:**:**:53:01`).
pub fn mask_mac(text: &str) -> String {
    match parse_mac(text) {
        Some(mac) => format!("**:**:**:**:{:02x}:{:02x}", mac[4], mac[5]),
        None => "-".to_owned(),
    }
}

/// How two egress addresses compare: an IPv4 address by itself, an IPv6 address by its /64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    Same,
    Different,
    /// one is not known, or one is IPv4 and the other IPv6
    Unknown,
}

pub fn egress(a: Option<&str>, b: Option<&str>) -> Egress {
    let parse = |ip: Option<&str>| ip.and_then(|ip| ip.trim().parse::<IpAddr>().ok());
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) if x.is_ipv4() == y.is_ipv4() => {
            if address_group(&x.to_string()) == address_group(&y.to_string()) {
                Egress::Same
            } else {
                Egress::Different
            }
        }
        _ => Egress::Unknown,
    }
}

// ---- the PC's report ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub v: u32,
    pub adapters: Vec<Adapter>,
    pub power: Power,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adapter {
    /// lowercase, `:` between the bytes
    pub mac: String,
    /// the card's addresses as `a.b.c.d/p`
    pub ipv4: Vec<String>,
    pub gateway: Option<String>,
    pub gateway_mac: Option<String>,
    pub desc: String,
    /// the bus of the card's device ID ("pci", "usb"), when the PC said
    pub bus: Option<String>,
    pub link_mbps: Option<u64>,
    pub driver: Driver,
    pub wake: Wake,
    /// whether Windows lets the card wake the PC (Device Manager), when the PC said
    pub armed: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Driver {
    pub provider: Option<String>,
    pub version: Option<String>,
    pub date: Option<String>,
}

/// The driver's wake keywords as the registry has them ("1" on, "0" off).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    pub magic: Option<String>,
    pub pattern: Option<String>,
    pub ms_magic: Option<String>,
    pub s5: Option<String>,
    pub shutdown_speed: Option<String>,
    pub eee: Option<String>,
    pub green: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Power {
    pub s3: Option<bool>,
    pub s4: Option<bool>,
    pub s5: Option<bool>,
    pub hiberfile: Option<bool>,
    pub aoac: Option<bool>,
    pub fast_startup: Option<bool>,
    pub on_ac: Option<bool>,
}

fn text_of(object: &Map<String, Value>, name: &str, max: usize) -> Option<String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(|t| clean_text(t, max))
}

fn bool_of(object: &Map<String, Value>, name: &str) -> Option<bool> {
    object.get(name).and_then(Value::as_bool)
}

fn object_of<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Map<String, Value>> {
    object.get(name).and_then(Value::as_object)
}

/// A report as hjv-api keeps it; `None` when it is not an object with `"v":1`.
pub fn parse_report(value: &Value) -> Option<Report> {
    let object = value.as_object()?;
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let empty = Map::new();
    let adapters = object
        .get("adapters")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| adapter(item.as_object()?))
                .take(MAX_ADAPTERS)
                .collect()
        })
        .unwrap_or_default();
    let power = object_of(object, "power").unwrap_or(&empty);
    Some(Report {
        v: 1,
        adapters,
        power: Power {
            s3: bool_of(power, "s3"),
            s4: bool_of(power, "s4"),
            s5: bool_of(power, "s5"),
            hiberfile: bool_of(power, "hiberfile"),
            aoac: bool_of(power, "aoac"),
            fast_startup: bool_of(power, "fast_startup"),
            on_ac: bool_of(power, "on_ac"),
        },
    })
}

fn adapter(object: &Map<String, Value>) -> Option<Adapter> {
    let mac = object
        .get("mac")
        .and_then(Value::as_str)
        .and_then(parse_mac)
        .filter(wakeable)?;
    let ipv4: Vec<String> = object
        .get("ipv4")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|a| Net4::parse(a.as_str()?))
                .filter(|n| (8..=32).contains(&n.prefix()))
                .map(|n| n.to_string())
                .take(MAX_ADDRESSES)
                .collect()
        })
        .unwrap_or_default();
    let empty = Map::new();
    let driver = object_of(object, "driver").unwrap_or(&empty);
    let wake = object_of(object, "wake").unwrap_or(&empty);
    let keyword = |name: &str| text_of(wake, name, 16);
    Some(Adapter {
        mac: mac_text(&mac),
        ipv4,
        gateway: object
            .get("gateway")
            .and_then(Value::as_str)
            .and_then(|g| g.parse::<Ipv4Addr>().ok())
            .map(|g| g.to_string()),
        gateway_mac: object
            .get("gateway_mac")
            .and_then(Value::as_str)
            .and_then(parse_mac)
            .filter(|m| *m != [0; 6])
            .map(|m| mac_text(&m)),
        desc: text_of(object, "desc", MAX_TEXT).unwrap_or_default(),
        bus: object
            .get("bus")
            .and_then(Value::as_str)
            .filter(|b| (1..=16).contains(&b.len()) && b.bytes().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_ascii_lowercase),
        link_mbps: object.get("link_mbps").and_then(Value::as_u64),
        driver: Driver {
            provider: text_of(driver, "provider", MAX_TEXT),
            version: text_of(driver, "version", MAX_TEXT),
            date: text_of(driver, "date", MAX_TEXT),
        },
        wake: Wake {
            magic: keyword("magic"),
            pattern: keyword("pattern"),
            ms_magic: keyword("ms_magic"),
            s5: keyword("s5"),
            shutdown_speed: keyword("shutdown_speed"),
            eee: keyword("eee"),
            green: keyword("green"),
        },
        armed: bool_of(object, "armed"),
    })
}

impl Report {
    /// The networks of its cards, canonical, each once, in the report's order.
    pub fn nets(&self) -> Vec<String> {
        let mut nets: Vec<String> = Vec::new();
        for net in self.addresses().map(|(_, n)| n.canonical()) {
            if !nets.contains(&net) {
                nets.push(net);
            }
        }
        nets
    }

    /// Every address of every card, with its card.
    pub fn addresses(&self) -> impl Iterator<Item = (&Adapter, Net4)> {
        self.adapters.iter().flat_map(|a| {
            a.ipv4
                .iter()
                .filter_map(|text| Net4::parse(text))
                .map(move |n| (a, n))
        })
    }

    /// The MACs of the cards with an address on `net` (canonical), each once, at most four
    /// (PROTOCOL.md §7.1).
    pub fn macs_on(&self, net: &Net4) -> Vec<String> {
        let mut macs: Vec<String> = Vec::new();
        for (adapter, address) in self.addresses() {
            if address.same_network(net) && !macs.contains(&adapter.mac) {
                macs.push(adapter.mac.clone());
            }
        }
        macs.truncate(4);
        macs
    }
}

// ---- readiness (design v3 §8-4, "준비 판정 안내") ----

/// Whether the PC can be woken, as far as its report tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ready {
    Ok,
    Warn,
    No,
    /// no report yet
    Unknown,
}

impl Ready {
    pub fn as_str(self) -> &'static str {
        match self {
            Ready::Ok => "ok",
            Ready::Warn => "warn",
            Ready::No => "no",
            Ready::Unknown => "unknown",
        }
    }
}

/// A hint: what keeps the PC from waking, or may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hint {
    pub code: &'static str,
    /// No: it cannot be woken (no wired card); Warn: it may not wake, or not from every state
    pub level: Ready,
    pub text: &'static str,
}

pub const HINT_NO_WIRED: Hint = Hint {
    code: "no_wired",
    level: Ready::No,
    text: "이 PC에는 켜기에 쓸 유선 랜카드가 없습니다. 원격 켜기는 유선으로 연결된 PC만 됩니다.",
};
pub const HINT_MAGIC_OFF: Hint = Hint {
    code: "magic_off",
    level: Ready::Warn,
    text: "랜카드의 매직 패킷 깨우기가 꺼져 있습니다. 장치 관리자에서 랜카드 속성의 '고급' 탭에 있는 'Wake on Magic Packet'을 켜 주세요.",
};
pub const HINT_MAGIC_OFF_SOME: Hint = Hint {
    code: "magic_off_some",
    level: Ready::Warn,
    text: "랜카드 가운데 매직 패킷 깨우기가 꺼진 것이 있습니다. 장치 관리자에서 랜카드 속성의 '고급' 탭에 있는 'Wake on Magic Packet'을 켜 주세요.",
};
pub const HINT_MAGIC_UNKNOWN: Hint = Hint {
    code: "magic_unknown",
    level: Ready::Warn,
    text: "랜카드 드라이버가 매직 패킷 깨우기 설정을 알려 주지 않아 켜질지 확인하지 못했습니다. 켜지지 않으면 장치 관리자에서 랜카드 속성의 '고급' 탭을 확인해 주세요.",
};
pub const HINT_NOT_ARMED: Hint = Hint {
    code: "not_armed",
    level: Ready::Warn,
    text: "장치 관리자에서 랜카드 속성의 '전원 관리' 탭에 있는 '이 장치를 사용하여 컴퓨터의 대기 모드를 해제할 수 있음'을 켜 주세요.",
};
pub const HINT_FAST_STARTUP: Hint = Hint {
    code: "fast_startup",
    level: Ready::Warn,
    text: "빠른 시작이 켜져 있어 '종료' 상태에서는 켜지지 않습니다(절전은 됩니다).",
};
pub const HINT_ON_BATTERY: Hint = Hint {
    code: "on_battery",
    level: Ready::Warn,
    text: "배터리로 동작하는 중에는 켜지지 않을 수 있습니다. 전원 어댑터를 연결해 두세요.",
};
pub const HINT_USB: Hint = Hint {
    code: "usb",
    level: Ready::Warn,
    text: "USB 랜카드는 대개 PC를 켜지 못합니다.",
};
pub const HINT_MODERN_STANDBY: Hint = Hint {
    code: "modern_standby",
    level: Ready::Warn,
    text: "모던 스탠바이 PC입니다. 랜카드의 모던 스탠바이용 매직 패킷 깨우기가 켜져 있어야 절전에서 켜집니다.",
};

/// The verdict and its hints for a PC's report (design v3 §8-4); `None`: no report, unknown.
pub fn readiness(report: Option<&Report>) -> (Ready, Vec<Hint>) {
    let Some(report) = report else {
        return (Ready::Unknown, Vec::new());
    };
    let cards: Vec<&Adapter> = report
        .adapters
        .iter()
        .filter(|a| !a.ipv4.is_empty())
        .collect();
    if cards.is_empty() {
        return (Ready::No, vec![HINT_NO_WIRED]);
    }
    let mut hints = Vec::new();
    // Only a PC without a wired card cannot be woken at all (design v3 §8-4: "불가"); every other
    // rule is a hint, which the app shows when a wake was not confirmed, so the PC stays one the
    // user may try (a driver's keyword does not always say what the card does when it is off).
    // The design's `magic≠1`: off as the driver says it ("0"), or not told at all (a driver
    // without the keyword), which is not known to be off.
    let magic = |a: &&Adapter, value: &str| a.wake.magic.as_deref() == Some(value);
    if cards.iter().all(|a| magic(a, "0")) {
        hints.push(HINT_MAGIC_OFF);
    } else if cards.iter().any(|a| magic(a, "0")) {
        hints.push(HINT_MAGIC_OFF_SOME);
    } else if !cards.iter().any(|a| magic(a, "1")) {
        hints.push(HINT_MAGIC_UNKNOWN);
    }
    if cards.iter().all(|a| a.armed == Some(false)) {
        hints.push(HINT_NOT_ARMED);
    }
    if report.power.fast_startup == Some(true) {
        hints.push(HINT_FAST_STARTUP);
    }
    if report.power.on_ac == Some(false) {
        hints.push(HINT_ON_BATTERY);
    }
    if cards.iter().all(|a| a.bus.as_deref() == Some("usb")) {
        hints.push(HINT_USB);
    }
    if report.power.aoac == Some(true)
        && cards
            .iter()
            .all(|a| a.wake.ms_magic.as_deref() != Some("1"))
    {
        hints.push(HINT_MODERN_STANDBY);
    }
    let ready = hints.iter().map(|h| h.level).max().unwrap_or(Ready::Ok);
    (ready, hints)
}

/// The hints' texts, as the apps show them.
pub fn hint_texts(hints: &[Hint]) -> Vec<&'static str> {
    hints.iter().map(|h| h.text).collect()
}

// ---- the relay's interfaces (PROTOCOL.md §9.1) ----

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayIface {
    pub name: String,
    pub mac: Option<String>,
    pub ipv4: Vec<String>,
    pub up: bool,
    pub running: bool,
    pub gateway: Option<String>,
    pub gateway_mac: Option<String>,
}

/// A relay's `ifaces` as hjv-api keeps them; `None` when it is not an array of objects with a
/// name and an address list (PROTOCOL.md §4.3 step 7). Addresses that are not `a.b.c.d/p` with a
/// prefix a relay can use ([`IFACE_PREFIXES`]) are left out, and anything past [`MAX_IFACES`]
/// interfaces and [`MAX_IFACE_ADDRESSES`] addresses.
pub fn parse_ifaces(value: &Value) -> Option<Vec<RelayIface>> {
    let items = value.as_array()?;
    let mut ifaces = Vec::new();
    for item in items {
        let object = item.as_object()?;
        let name = object.get("name")?.as_str()?;
        let addresses = object.get("ipv4")?.as_array()?;
        if ifaces.len() == MAX_IFACES {
            continue;
        }
        ifaces.push(RelayIface {
            name: clean_text(name, 32),
            mac: object
                .get("mac")
                .and_then(Value::as_str)
                .and_then(parse_mac)
                .map(|m| mac_text(&m)),
            ipv4: addresses
                .iter()
                .filter_map(|a| Net4::parse(a.as_str()?))
                .filter(|n| IFACE_PREFIXES.contains(&n.prefix()))
                .map(|n| n.to_string())
                .take(MAX_IFACE_ADDRESSES)
                .collect(),
            up: object.get("up").and_then(Value::as_bool).unwrap_or(false),
            running: object
                .get("running")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            gateway: object
                .get("gateway")
                .and_then(Value::as_str)
                .and_then(|g| g.parse::<Ipv4Addr>().ok())
                .map(|g| g.to_string()),
            gateway_mac: object
                .get("gateway_mac")
                .and_then(Value::as_str)
                .and_then(parse_mac)
                .filter(|m| *m != [0; 6])
                .map(|m| mac_text(&m)),
        });
    }
    Some(ifaces)
}

// ---- matching ----

/// How a relay matches a PC (design v3 §8-4), best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    Exact,
    Overlap,
    History,
    /// none of the above: the user asked to try all relays
    Any,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Grade::Exact => "exact",
            Grade::Overlap => "overlap",
            Grade::History => "history",
            Grade::Any => "any",
        }
    }

    pub fn parse(text: &str) -> Option<Grade> {
        Some(match text {
            "exact" => Grade::Exact,
            "overlap" => Grade::Overlap,
            "history" => Grade::History,
            "any" => Grade::Any,
            _ => return None,
        })
    }
}

/// What [`judge`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// the best grade, if the relay matches
    pub grade: Option<Grade>,
    /// the PC's network a command would name (canonical), with the grade's, or else that of the
    /// first overlap
    pub net: Option<Net4>,
    /// one of the relay's networks overlaps one of the PC's (whatever the gateways)
    pub overlap: bool,
    /// the egress addresses, and whether the matched (or overlapping) network is a private one
    pub egress: Egress,
    pub private: bool,
}

impl Verdict {
    /// The relay is on the PC's network as far as hjv-api can tell (an exact or overlapping
    /// match; history is not about the network).
    pub fn same_network(&self) -> bool {
        matches!(self.grade, Some(Grade::Exact | Grade::Overlap))
    }

    /// The networks overlap, but on a private network the egress addresses differ: another site.
    pub fn elsewhere(&self) -> bool {
        self.overlap && self.private && self.egress == Egress::Different
    }
}

fn same_text(a: &Option<String>, b: &Option<String>) -> bool {
    matches!((a, b), (Some(x), Some(y)) if x == y)
}

/// How a relay with `ifaces` and the egress address `relay_egress` matches a PC with `report`
/// and the egress address `pc_egress`; `woken_on` is the networks (canonical) on which this relay
/// woke this PC before.
pub fn judge(
    ifaces: &[RelayIface],
    relay_egress: Option<&str>,
    report: &Report,
    pc_egress: Option<&str>,
    woken_on: &[String],
) -> Verdict {
    let egress = egress(pc_egress, relay_egress);
    let mut best: Option<(Grade, Net4)> = None;
    let mut first_overlap: Option<Net4> = None;
    for (adapter, pc_net) in report.addresses() {
        // on a network that repeats between sites, the same egress address shows the same site
        let site_ok = !pc_net.is_private() || egress == Egress::Same;
        let mut grade = None;
        let mut overlapped = false;
        for iface in ifaces {
            for relay_net in iface.ipv4.iter().filter_map(|a| Net4::parse(a)) {
                if !relay_net.overlaps(&pc_net) {
                    continue;
                }
                overlapped = true;
                first_overlap.get_or_insert(pc_net);
                let same_gateway = same_text(&iface.gateway, &adapter.gateway);
                let found = if relay_net.same_network(&pc_net) && same_gateway {
                    Grade::Exact
                } else if same_gateway || same_text(&iface.gateway_mac, &adapter.gateway_mac) {
                    Grade::Overlap
                } else {
                    continue;
                };
                grade = Some(grade.map_or(found, |g: Grade| g.min(found)));
            }
        }
        // it woke this PC here before, and is still here: without an address on this network
        // it would only answer `no_iface`
        if grade.is_none() && overlapped && woken_on.contains(&pc_net.canonical()) {
            grade = Some(Grade::History);
        }
        if let (Some(g), true) = (grade, site_ok) {
            if best.is_none_or(|(b, _)| g < b) {
                best = Some((g, pc_net.network()));
            }
        }
    }
    let net = best.map(|(_, n)| n).or(first_overlap.map(|n| n.network()));
    Verdict {
        grade: best.map(|(g, _)| g),
        overlap: first_overlap.is_some(),
        private: net.is_some_and(|n| n.is_private()),
        net,
        egress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn net(t: &str) -> Net4 {
        Net4::parse(t).unwrap()
    }

    #[test]
    fn networks() {
        let pc = net("192.0.2.67/24");
        assert_eq!(pc.canonical(), "192.0.2.0/24");
        assert_eq!(pc.to_string(), "192.0.2.67/24");
        assert!(pc.same_network(&net("192.0.2.1/24")));
        assert!(!pc.same_network(&net("192.0.2.1/25")));
        // a /22 wire with a NAS on a /24 of it: they overlap either way
        assert!(net("10.1.5.20/22").overlaps(&net("10.1.6.7/24")));
        assert!(net("10.1.6.7/24").overlaps(&net("10.1.5.20/22")));
        assert!(!net("10.1.5.20/24").overlaps(&net("10.1.6.7/24")));
        assert_eq!(net("10.1.7.255/22").canonical(), "10.1.4.0/22");
        assert_eq!(net("0.0.0.0/0").canonical(), "0.0.0.0/0");
        assert_eq!(net("192.0.2.1/32").canonical(), "192.0.2.1/32");
        for private in [
            "10.1.4.0/22",
            "172.20.1.1/16",
            "192.168.50.10/24",
            "100.64.1.1/16",
        ] {
            assert!(net(private).is_private(), "{private}");
        }
        for public in [
            "192.0.2.0/24",
            "172.32.0.1/16",
            "100.128.0.1/16",
            "203.0.113.9/24",
        ] {
            assert!(!net(public).is_private(), "{public}");
        }
        for bad in [
            "192.0.2.1",
            "192.0.2.1/33",
            "192.0.2.1/",
            "192.0.2.1/+8",
            "192.0.2.1/008",
            "192.0.2/24",
            "2001:db8::1/64",
            " 192.0.2.1/24",
        ] {
            assert_eq!(Net4::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn macs() {
        assert_eq!(
            parse_mac("00:00:5E:00:53:01")
                .map(|m| mac_text(&m))
                .as_deref(),
            Some("00:00:5e:00:53:01")
        );
        assert_eq!(
            parse_mac("00-00-5e-00-53-01"),
            parse_mac("00:00:5e:00:53:01")
        );
        for bad in [
            "00:00:5e:00:53",
            "00:00:5e:00:53:01:02",
            "00:00-5e:00:53:01",
            "0:00:5e:00:53:01",
            "zz:00:5e:00:53:01",
        ] {
            assert_eq!(parse_mac(bad), None, "{bad}");
        }
        assert!(wakeable(&[0, 0, 0x5e, 0, 0x53, 1]));
        assert!(!wakeable(&[1, 0, 0x5e, 0, 0x53, 1]) && !wakeable(&[0; 6]));
        assert!(!wakeable(&[0xff; 6]));
        assert_eq!(mask_mac("00:00:5e:00:53:01"), "**:**:**:**:53:01");
        assert_eq!(mask_mac("x"), "-");
    }

    #[test]
    fn egress_addresses() {
        assert_eq!(egress(Some("192.0.2.4"), Some("192.0.2.4")), Egress::Same);
        assert_eq!(
            egress(Some("192.0.2.4"), Some("192.0.2.5")),
            Egress::Different
        );
        assert_eq!(
            egress(Some("2001:db8:1:2::1"), Some("2001:db8:1:2::9")),
            Egress::Same
        );
        assert_eq!(
            egress(Some("2001:db8:1:2::1"), Some("2001:db8:1:3::1")),
            Egress::Different
        );
        assert_eq!(
            egress(Some("2001:db8:1:2::1"), Some("192.0.2.4")),
            Egress::Unknown
        );
        assert_eq!(egress(None, Some("192.0.2.4")), Egress::Unknown);
        assert_eq!(egress(Some("invalid"), Some("192.0.2.4")), Egress::Unknown);
    }

    /// design v3 §8-2's example (documentation values only)
    pub fn example() -> Value {
        json!({"v":1,
            "adapters":[{"mac":"00:00:5e:00:53:10","ipv4":["192.0.2.67/24"],"gateway":"192.0.2.1",
                "gateway_mac":"00:00:5e:00:53:01","desc":"Example PCIe GbE Controller","bus":"pci",
                "link_mbps":1000,"driver":{"provider":"Example","version":"1.0.0.0","date":"1-1-2024"},
                "wake":{"magic":"1","pattern":"1","ms_magic":null,"s5":"1","shutdown_speed":"0","eee":"0","green":"0"},
                "armed":true}],
            "power":{"s3":true,"s4":true,"s5":true,"hiberfile":true,"aoac":false,"fast_startup":false,"on_ac":true},
            "uptime_s":3600})
    }

    #[test]
    fn reports() {
        let r = parse_report(&example()).unwrap();
        assert_eq!(r.adapters.len(), 1);
        let a = &r.adapters[0];
        assert_eq!(a.mac, "00:00:5e:00:53:10");
        assert_eq!(a.ipv4, vec!["192.0.2.67/24"]);
        assert_eq!(a.gateway.as_deref(), Some("192.0.2.1"));
        assert_eq!(a.bus.as_deref(), Some("pci"));
        assert_eq!(a.wake.magic.as_deref(), Some("1"));
        assert_eq!(a.wake.ms_magic, None);
        assert_eq!(r.power.fast_startup, Some(false));
        assert_eq!(r.nets(), vec!["192.0.2.0/24"]);
        assert_eq!(readiness(Some(&r)), (Ready::Ok, vec![]));
        // kept without the uptime, and read back the same
        let kept = serde_json::to_string(&r).unwrap();
        assert!(!kept.contains("uptime"));
        assert_eq!(serde_json::from_str::<Report>(&kept).unwrap(), r);
        // what has no form is left out, and what is not a report is not taken
        let mut odd = example();
        odd["adapters"] = json!([
            {"mac": "01:00:5e:00:00:01", "ipv4": ["192.0.2.9/24"]},
            {"mac": "00:00:00:00:00:00", "ipv4": ["192.0.2.9/24"]},
            {"mac": "nonsense"},
            {"mac": "00:00:5e:00:53:11", "ipv4": ["192.0.2.68/24", "bad", "192.0.2.68/4", 7],
             "gateway": "x", "gateway_mac": 5, "desc": "a\u{7}b".repeat(40), "bus": "PCI Express",
             "link_mbps": -1, "driver": "x", "wake": {"magic": 1}, "armed": "yes"},
            "not an object",
        ]);
        odd["power"] = json!({"s3": "yes", "on_ac": false});
        let r = parse_report(&odd).unwrap();
        assert_eq!(r.adapters.len(), 1);
        let a = &r.adapters[0];
        assert_eq!(a.ipv4, vec!["192.0.2.68/24"]);
        assert_eq!(
            (a.gateway.clone(), a.gateway_mac.clone(), a.bus.clone()),
            (None, None, None)
        );
        assert_eq!(a.desc.chars().count(), MAX_TEXT);
        assert_eq!(
            (a.link_mbps, a.wake.magic.clone(), a.armed),
            (None, None, None)
        );
        assert_eq!((r.power.s3, r.power.on_ac), (None, Some(false)));
        let many: Vec<Value> = (0..9)
            .map(|i| json!({"mac": format!("00:00:5e:00:53:{i:02x}"), "ipv4": vec!["192.0.2.1/24"; 9]}))
            .collect();
        let r = parse_report(&json!({"v": 1, "adapters": many})).unwrap();
        assert_eq!(r.adapters.len(), MAX_ADAPTERS);
        assert!(r.adapters.iter().all(|a| a.ipv4.len() == MAX_ADDRESSES));
        for bad in [
            json!({"v": 2}),
            json!([]),
            json!("x"),
            json!({"adapters": []}),
        ] {
            assert_eq!(parse_report(&bad), None, "{bad}");
        }
    }

    #[test]
    fn hints() {
        let base = parse_report(&example()).unwrap();
        let with = |f: &dyn Fn(&mut Report)| {
            let mut r = base.clone();
            f(&mut r);
            let (ready, hints) = readiness(Some(&r));
            (ready, hints.iter().map(|h| h.code).collect::<Vec<_>>())
        };
        assert_eq!(readiness(None), (Ready::Unknown, vec![]));
        assert_eq!(with(&|r| r.adapters.clear()), (Ready::No, vec!["no_wired"]));
        assert_eq!(
            with(&|r| r.adapters[0].ipv4.clear()),
            (Ready::No, vec!["no_wired"])
        );
        // off: a hint, shown when a wake was not confirmed; the user may still try
        assert_eq!(
            with(&|r| r.adapters[0].wake.magic = Some("0".into())),
            (Ready::Warn, vec!["magic_off"])
        );
        // a driver that does not name the keyword may still wake: not known, which is not off
        assert_eq!(
            with(&|r| r.adapters[0].wake.magic = None),
            (Ready::Warn, vec!["magic_unknown"])
        );
        assert_eq!(
            with(&|r| r.adapters[0].wake.magic = Some("2".into())),
            (Ready::Warn, vec!["magic_unknown"])
        );
        // one card that is known to be on is enough
        assert_eq!(
            with(&|r| {
                let mut second = r.adapters[0].clone();
                second.mac = "00:00:5e:00:53:20".into();
                second.wake.magic = None;
                r.adapters.push(second);
            }),
            (Ready::Ok, vec![])
        );
        // off and not known: the one that is off is what the user can change
        assert_eq!(
            with(&|r| {
                let mut second = r.adapters[0].clone();
                second.mac = "00:00:5e:00:53:20".into();
                second.wake.magic = Some("0".into());
                r.adapters[0].wake.magic = None;
                r.adapters.push(second);
            }),
            (Ready::Warn, vec!["magic_off_some"])
        );
        assert_eq!(
            with(&|r| {
                let mut second = r.adapters[0].clone();
                second.mac = "00:00:5e:00:53:20".into();
                second.wake.magic = Some("0".into());
                r.adapters.push(second);
            }),
            (Ready::Warn, vec!["magic_off_some"])
        );
        assert_eq!(
            with(&|r| r.adapters[0].armed = Some(false)),
            (Ready::Warn, vec!["not_armed"])
        );
        assert_eq!(with(&|r| r.adapters[0].armed = None), (Ready::Ok, vec![]));
        assert_eq!(
            with(&|r| r.power.fast_startup = Some(true)),
            (Ready::Warn, vec!["fast_startup"])
        );
        assert_eq!(
            with(&|r| r.power.on_ac = Some(false)),
            (Ready::Warn, vec!["on_battery"])
        );
        assert_eq!(
            with(&|r| r.adapters[0].bus = Some("usb".into())),
            (Ready::Warn, vec!["usb"])
        );
        assert_eq!(
            with(&|r| r.power.aoac = Some(true)),
            (Ready::Warn, vec!["modern_standby"])
        );
        assert_eq!(
            with(&|r| {
                r.power.aoac = Some(true);
                r.adapters[0].wake.ms_magic = Some("1".into());
            }),
            (Ready::Ok, vec![])
        );
        assert_eq!(
            with(&|r| {
                r.adapters[0].wake.magic = Some("0".into());
                r.power.fast_startup = Some(true);
            }),
            (Ready::Warn, vec!["magic_off", "fast_startup"])
        );
        // only a PC without a wired card is a "no"
        for hint in [
            HINT_MAGIC_OFF,
            HINT_MAGIC_OFF_SOME,
            HINT_MAGIC_UNKNOWN,
            HINT_NOT_ARMED,
            HINT_FAST_STARTUP,
            HINT_ON_BATTERY,
            HINT_USB,
            HINT_MODERN_STANDBY,
        ] {
            assert_eq!(hint.level, Ready::Warn, "{}", hint.code);
        }
        assert_eq!(HINT_NO_WIRED.level, Ready::No);
        // the texts never carry the word the app takes for a logout
        for hint in [
            HINT_NO_WIRED,
            HINT_MAGIC_OFF,
            HINT_MAGIC_OFF_SOME,
            HINT_MAGIC_UNKNOWN,
            HINT_NOT_ARMED,
            HINT_FAST_STARTUP,
            HINT_ON_BATTERY,
            HINT_USB,
            HINT_MODERN_STANDBY,
        ] {
            assert!(
                !hint.text.to_lowercase().contains("disabled"),
                "{}",
                hint.code
            );
        }
    }

    #[test]
    fn relay_interfaces() {
        let ifaces = parse_ifaces(&json!([
            {"name": "eth0", "mac": "02:00:00:00:00:01", "ipv4": ["10.1.5.20/22", "x"], "up": true,
             "running": true, "gateway": null, "gateway_mac": null},
            {"name": "eth1", "mac": null, "ipv4": [], "up": true, "running": false,
             "gateway": "192.168.50.1", "gateway_mac": "02:00:00:00:00:FE", "extra": 1},
        ]))
        .unwrap();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].ipv4, vec!["10.1.5.20/22"]);
        assert_eq!(ifaces[1].gateway_mac.as_deref(), Some("02:00:00:00:00:fe"));
        assert!(ifaces[0].running && !ifaces[1].running);
        // only the prefixes a relay uses (PROTOCOL.md §9.1): one as wide as /0 would overlap every
        // PC's network, and a /31 or /32 has no broadcast address to send to
        let odd = parse_ifaces(&json!([{"name": "eth0", "ipv4": [
            "10.9.9.9/0", "192.0.0.1/7", "192.0.2.200/31", "192.0.2.200/32", "192.0.0.1/8",
            "192.0.2.200/30"
        ]}]))
        .unwrap();
        assert_eq!(odd[0].ipv4, vec!["192.0.0.1/8", "192.0.2.200/30"]);
        assert_eq!(parse_ifaces(&json!([])), Some(vec![]));
        for bad in [
            json!({}),
            json!([{"ipv4": []}]),
            json!([{"name": "eth0"}]),
            json!([{"name": "eth0", "ipv4": "10.1.5.20/22"}]),
            json!(["eth0"]),
        ] {
            assert_eq!(parse_ifaces(&bad), None, "{bad}");
        }
        let many: Vec<Value> = (0..20)
            .map(|i| json!({"name": format!("eth{i}"), "ipv4": vec!["10.1.5.20/22"; 10]}))
            .collect();
        let ifaces = parse_ifaces(&Value::Array(many)).unwrap();
        assert_eq!(ifaces.len(), MAX_IFACES);
        assert!(ifaces.iter().all(|i| i.ipv4.len() == MAX_IFACE_ADDRESSES));
    }

    fn relay(addr: &str, gateway: Option<&str>, gateway_mac: Option<&str>) -> Vec<RelayIface> {
        vec![RelayIface {
            name: "eth0".into(),
            ipv4: vec![addr.into()],
            up: true,
            running: true,
            gateway: gateway.map(Into::into),
            gateway_mac: gateway_mac.map(Into::into),
            ..RelayIface::default()
        }]
    }

    fn pc(addr: &str, gateway: Option<&str>, gateway_mac: Option<&str>) -> Report {
        Report {
            v: 1,
            adapters: vec![Adapter {
                mac: "00:00:5e:00:53:10".into(),
                ipv4: vec![addr.into()],
                gateway: gateway.map(Into::into),
                gateway_mac: gateway_mac.map(Into::into),
                ..Adapter::default()
            }],
            power: Power::default(),
        }
    }

    #[test]
    fn grades() {
        let gw = Some("192.0.2.1");
        let gw_mac = Some("00:00:5e:00:53:01");
        let grade = |r: &[RelayIface], p: &Report, woken: &[String]| {
            judge(r, Some("198.51.100.7"), p, Some("198.51.100.7"), woken).grade
        };
        // ① the same network and gateway
        let p = pc("192.0.2.67/24", gw, None);
        assert_eq!(
            grade(&relay("192.0.2.10/24", gw, None), &p, &[]),
            Some(Grade::Exact)
        );
        // ② a NAS with another prefix on the same wire: the same gateway IP, or gateway MAC
        let wide = pc("192.0.2.67/23", gw, gw_mac);
        assert_eq!(
            grade(&relay("192.0.2.10/24", gw, None), &wide, &[]),
            Some(Grade::Overlap)
        );
        assert_eq!(
            grade(
                &relay("192.0.2.10/24", Some("192.0.2.254"), gw_mac),
                &wide,
                &[]
            ),
            Some(Grade::Overlap)
        );
        // the same network with another gateway, an unknown gateway: no match
        assert_eq!(
            grade(&relay("192.0.2.10/24", Some("192.0.2.254"), None), &p, &[]),
            None
        );
        assert_eq!(
            grade(
                &relay("192.0.2.10/24", None, None),
                &pc("192.0.2.67/24", None, None),
                &[]
            ),
            None
        );
        // ③ it woke this PC on this network before
        assert_eq!(
            grade(
                &relay("192.0.2.10/24", None, None),
                &p,
                &["192.0.2.0/24".into()]
            ),
            Some(Grade::History)
        );
        // history is of a network the relay is still on: moved elsewhere, or without an
        // interface to report, it has no grade (it could only answer no_iface)
        let woken = ["192.0.2.0/24".to_owned()];
        assert_eq!(grade(&relay("203.0.113.10/24", gw, None), &p, &woken), None);
        assert_eq!(grade(&[], &p, &woken), None);
        let mut no_address = relay("192.0.2.10/24", gw, None);
        no_address[0].ipv4.clear();
        assert_eq!(grade(&no_address, &p, &woken), None);
        // another network
        let v = judge(&relay("198.51.100.10/24", gw, None), None, &p, None, &[]);
        assert_eq!((v.grade, v.overlap, v.net), (None, false, None));
        // the best of several addresses and grades, with the network a command names
        let mut two = pc("203.0.113.5/24", Some("203.0.113.1"), None);
        two.adapters[0].ipv4.push("192.0.2.67/24".into());
        let v = judge(&relay("192.0.2.10/24", gw, None), None, &two, None, &[]);
        assert_eq!(v.grade, None);
        assert!(v.overlap && !v.same_network());
        two.adapters[0].gateway = gw.map(Into::into);
        let v = judge(&relay("192.0.2.10/24", gw, None), None, &two, None, &[]);
        assert_eq!(
            (v.grade, v.net.map(|n| n.to_string())),
            (Some(Grade::Exact), Some("192.0.2.0/24".to_owned()))
        );
    }

    #[test]
    fn private_networks_need_the_same_egress() {
        let gw = Some("10.1.4.1");
        let (r, p) = (relay("10.1.5.20/22", gw, None), pc("10.1.6.7/22", gw, None));
        let at = |pc_ip: Option<&str>, relay_ip: Option<&str>| judge(&r, relay_ip, &p, pc_ip, &[]);
        let same = at(Some("203.0.113.9"), Some("203.0.113.9"));
        assert_eq!(
            (same.grade, same.private, same.egress),
            (Some(Grade::Exact), true, Egress::Same)
        );
        let other = at(Some("203.0.113.9"), Some("198.51.100.3"));
        assert_eq!(other.grade, None);
        assert!(other.overlap && other.elsewhere());
        let unknown = at(None, Some("198.51.100.3"));
        assert_eq!(unknown.grade, None);
        assert!(unknown.overlap && !unknown.elsewhere());
        // history does not bring back another site either
        let v = judge(
            &relay("10.1.5.20/22", None, None),
            Some("198.51.100.3"),
            &p,
            Some("203.0.113.9"),
            &["10.1.4.0/22".into()],
        );
        assert_eq!(v.grade, None);
        // a public network needs no egress
        let v = judge(
            &relay("192.0.2.10/24", Some("192.0.2.1"), None),
            Some("198.51.100.3"),
            &pc("192.0.2.67/24", Some("192.0.2.1"), None),
            Some("203.0.113.9"),
            &[],
        );
        assert_eq!(v.grade, Some(Grade::Exact));
    }
}
