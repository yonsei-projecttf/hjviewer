// SPDX-License-Identifier: AGPL-3.0-only

//! Real magic packets inside throwaway network namespaces; nothing reaches a real network.
//!
//! The test re-runs itself under `unshare --user --map-root-user --net --mount --fork` (as root,
//! without `--user`), so it needs no sudo where unprivileged user namespaces are allowed. It skips
//! with a message where they are not (Ubuntu 24.04 restricts them through AppArmor:
//! `sysctl kernel.apparmor_restrict_unprivileged_userns=0`, or run the test as root);
//! `HJV_RELAY_NETNS=require` turns the skip into a failure, for CI.
//!
//! The lab of the design notes: bridge br0 is LAN A 10.1.4.0/22 with the NAS (eth0
//! 10.1.5.20/22) and a sleeping PC (MAC 00:00:5e:00:53:01, no IP: it answers nothing, and
//! AF_PACKET sees what reaches its port); br1 is LAN B 192.168.50.0/24 with the NAS's eth1, which
//! has the NAS's default route, and another host. The NAS also has a container bridge docker0 on
//! 10.1.6.1/24, and for a while a port bond0 on LAN A without carrier. (The addresses and MACs are
//! documentation values.) `hjv-relay test-wake` runs in the NAS namespace as an unprivileged user
//! without capabilities (as root: as root, so the privilege drop runs too); then the relay loop itself
//! runs there against a test server, with the real interface listing and sending; then the
//! `run` command of a binary built for that test server (HJV_SERVER=localhost and the test
//! command key, built by this test into target/netns-configured): enrollment, a command, the
//! state folder lock, `status`, and the exit when its executable is replaced.

mod common;

use base64::{engine::general_purpose::STANDARD, Engine};
use common::{command_signing_key, relay_config, signed, unix_now, Reply, TestServer};
use hjv_relay::{
    identity::Identity,
    relay::{Relay, State, SystemEnv},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::{self, File},
    io,
    net::Ipv4Addr,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::PermissionsExt,
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const INNER: &str = "HJV_RELAY_NETNS_INNER";
const CONFIGURED: &str = "HJV_RELAY_NETNS_CONFIGURED_BIN";
const PC_MAC: [u8; 6] = [0x00, 0x00, 0x5e, 0x00, 0x53, 0x01];
const BIN: &str = "/run/hjv-relay-test/hjv-relay";

#[test]
fn packets_on_the_wire() {
    if std::env::var_os(INNER).is_some() {
        inner();
    } else {
        outer();
    }
}

fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

fn outer() {
    let require = std::env::var("HJV_RELAY_NETNS").is_ok_and(|v| v == "require");
    let skip = |why: &str| {
        assert!(!require, "the namespace test cannot run: {why}");
        eprintln!("SKIPPED the namespace test: {why} (HJV_RELAY_NETNS=require makes this fail)");
    };
    let user: &[&str] = if is_root() {
        &[]
    } else {
        &["--user", "--map-root-user"]
    };
    for (tool, arg) in [("unshare", "--version"), ("ip", "-V")] {
        if Command::new(tool).arg(arg).output().is_err() {
            return skip(&format!("{tool} is not installed"));
        }
    }
    let probe = Command::new("unshare")
        .args(user)
        .args(["--net", "--mount", "true"])
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        return skip("user and network namespaces are not available to this user");
    }
    let configured = build_configured();
    let status = Command::new("unshare")
        .args(user)
        .args(["--net", "--mount", "--fork"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "packets_on_the_wire",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER, "1")
        .env(CONFIGURED, configured)
        .status()
        .unwrap();
    assert!(status.success(), "the namespace test failed");
}

/// hjv-relay built the way a release is, but for the test server and the test command key.
fn build_configured() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("target").join("netns-configured");
    let key = STANDARD.encode(command_signing_key().verifying_key().to_bytes());
    let status = Command::new(env!("CARGO"))
        .current_dir(manifest)
        .args(["build", "--offline", "--locked", "--bin", "hjv-relay"])
        .env("CARGO_TARGET_DIR", &target)
        .env("HJV_SERVER", "localhost")
        .env("HJV_COMMAND_PUBKEY", format!("1:{key}"))
        .env_remove("HJV_BUILD_COMMIT")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "building the test-configured hjv-relay failed"
    );
    target.join("debug").join("hjv-relay")
}

fn run(program: &str, args: &[&str]) -> Output {
    let out = Command::new(program).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn ip(args: &[&str]) {
    run("ip", args);
}

fn ns_ip(ns: &str, args: &[&str]) {
    let mut all = vec!["-n", ns];
    all.extend_from_slice(args);
    run("ip", &all);
}

/// A veth from bridge `br` to `ifname` in namespace `ns` (created if needed).
fn attach(ns: &str, ifname: &str, br: &str, mac: Option<&str>) {
    if !Path::new("/run/netns").join(ns).exists() {
        ip(&["netns", "add", ns]);
        ns_ip(ns, &["link", "set", "lo", "up"]);
    }
    let outside = format!("v-{ns}-{ifname}");
    ip(&[
        "link", "add", &outside, "type", "veth", "peer", "name", ifname, "netns", ns,
    ]);
    ip(&["link", "set", &outside, "master", br, "up"]);
    if let Some(mac) = mac {
        ns_ip(ns, &["link", "set", ifname, "address", mac]);
    }
    ns_ip(ns, &["link", "set", ifname, "up"]);
}

fn wait_forwarding(ports: &[&str]) {
    let end = Instant::now() + Duration::from_secs(10);
    for port in ports {
        loop {
            let out = run("ip", &["-d", "link", "show", "dev", port]);
            if String::from_utf8_lossy(&out.stdout).contains("state forwarding") {
                break;
            }
            assert!(Instant::now() < end, "{port} does not forward");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn enter(ns: &str) {
    let file = File::open(format!("/run/netns/{ns}")).unwrap();
    // SAFETY: setns on an open namespace file; affects this thread only.
    let rc = unsafe { libc::setns(file.as_raw_fd(), libc::CLONE_NEWNET) };
    assert_eq!(rc, 0, "setns {ns}: {}", io::Error::last_os_error());
}

/// A magic packet for the PC as it arrived on a port.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Frame {
    dst_mac: [u8; 6],
    src: Ipv4Addr,
    dst: Ipv4Addr,
    port: u16,
}

struct Capture {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Vec<Frame>>,
}

impl Capture {
    fn start(ns: &'static str, ifname: &'static str) -> Capture {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            enter(ns);
            let proto = (libc::ETH_P_ALL as u16).to_be();
            // SAFETY: plain socket calls on a descriptor this thread owns.
            let fd = unsafe {
                libc::socket(
                    libc::AF_PACKET,
                    libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                    i32::from(proto),
                )
            };
            assert!(fd >= 0, "AF_PACKET socket: {}", io::Error::last_os_error());
            // SAFETY: fd is a new descriptor owned here.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            let name = CString::new(ifname).unwrap();
            // SAFETY: a NUL-terminated name.
            let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
            assert!(index > 0, "{ns}/{ifname} not found");
            // SAFETY: zeroed sockaddr_ll is valid; bind with its size.
            let mut sll: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_protocol = proto;
            sll.sll_ifindex = index as i32;
            let rc = unsafe {
                libc::bind(
                    fd.as_raw_fd(),
                    (&sll as *const libc::sockaddr_ll).cast(),
                    std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
                )
            };
            assert_eq!(rc, 0, "bind {ns}/{ifname}: {}", io::Error::last_os_error());
            let tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 50_000,
            };
            // SAFETY: SO_RCVTIMEO with a timeval.
            unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    (&tv as *const libc::timeval).cast(),
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                )
            };
            ready_tx.send(()).unwrap();
            let magic = hjv_relay::wake::magic_packet(&PC_MAC);
            let mut frames = Vec::new();
            let mut buf = [0u8; 2048];
            while !flag.load(Ordering::SeqCst) {
                // SAFETY: zeroed sockaddr_ll is valid for recvfrom to fill.
                let mut from: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
                let mut len = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
                // SAFETY: the buffer and address are valid for their sizes.
                let n = unsafe {
                    libc::recvfrom(
                        fd.as_raw_fd(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        0,
                        (&mut from as *mut libc::sockaddr_ll).cast(),
                        &mut len,
                    )
                };
                if n <= 0 || from.sll_pkttype == libc::PACKET_OUTGOING {
                    continue;
                }
                if let Some(frame) = parse_frame(&buf[..n as usize], &magic) {
                    frames.push(frame);
                }
            }
            frames
        });
        ready_rx.recv().unwrap();
        Capture { stop, thread }
    }

    fn finish(self) -> Vec<Frame> {
        thread::sleep(Duration::from_millis(300));
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

/// An Ethernet/IPv4/UDP frame whose payload is exactly the PC's magic packet.
fn parse_frame(f: &[u8], magic: &[u8]) -> Option<Frame> {
    if f.len() < 14 + 20 + 8 || f[12..14] != [0x08, 0x00] || f[23] != 17 {
        return None;
    }
    let ihl = usize::from(f[14] & 0x0f) * 4;
    let udp = 14 + ihl;
    let payload = f.get(udp + 8..)?;
    if !payload.starts_with(magic) {
        return None;
    }
    let mut dst_mac = [0u8; 6];
    dst_mac.copy_from_slice(&f[..6]);
    Some(Frame {
        dst_mac,
        src: Ipv4Addr::new(f[26], f[27], f[28], f[29]),
        dst: Ipv4Addr::new(f[30], f[31], f[32], f[33]),
        port: u16::from_be_bytes([f[udp + 2], f[udp + 3]]),
    })
}

/// Checks the frames: all broadcast at the Ethernet level, from `src`, and `rounds` times each
/// destination and port.
fn expect(frames: &[Frame], src: &str, dests: &[&str], ports: &[u16], rounds: usize, what: &str) {
    let mut counts: BTreeMap<(Ipv4Addr, u16), usize> = BTreeMap::new();
    for f in frames {
        assert_eq!(f.dst_mac, [0xff; 6], "{what}: {f:?}");
        assert_eq!(f.src, src.parse::<Ipv4Addr>().unwrap(), "{what}: {f:?}");
        *counts.entry((f.dst, f.port)).or_default() += 1;
    }
    let mut expected = BTreeMap::new();
    for d in dests {
        for p in ports {
            expected.insert((d.parse::<Ipv4Addr>().unwrap(), *p), rounds);
        }
    }
    assert_eq!(counts, expected, "{what}");
}

/// Runs `hjv-relay test-wake` in the nas namespace and captures on the PC and the other host.
fn test_wake(args: &[&str]) -> (Output, Vec<Frame>, Vec<Frame>) {
    let pc = Capture::start("pc", "eth0");
    let other = Capture::start("other", "eth0");
    let mac = hjv_relay::wake::format_mac(&PC_MAC);
    let mut cmd = Command::new("ip");
    cmd.args(["netns", "exec", "nas"]);
    if !is_root_outside() {
        // an unmapped user in a nested user namespace: uid 65534, no capabilities
        cmd.args(["unshare", "--user"]);
    }
    let out = cmd
        .arg(BIN)
        .args(["test-wake", &mac])
        .args(args)
        .output()
        .unwrap();
    eprint!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out, pc.finish(), other.finish())
}

/// Whether the test was started by the real root (then hjv-relay runs as root and drops its
/// privileges itself).
fn is_root_outside() -> bool {
    fs::read_to_string("/proc/self/uid_map")
        .is_ok_and(|m| m.split_whitespace().collect::<Vec<_>>() == ["0", "0", "4294967295"])
}

fn inner() {
    let _log = common::capture_log();
    run("mount", &["-t", "tmpfs", "tmpfs", "/run"]);
    fs::create_dir_all("/run/netns").unwrap();
    let dir = Path::new(BIN).parent().unwrap();
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_hjv-relay"), BIN).unwrap();
    fs::set_permissions(BIN, fs::Permissions::from_mode(0o755)).unwrap();

    ip(&["link", "set", "lo", "up"]);
    for br in ["br0", "br1"] {
        ip(&["link", "add", br, "type", "bridge"]);
        ip(&["link", "set", br, "up"]);
    }
    attach("nas", "eth0", "br0", None);
    attach("nas", "eth1", "br1", None);
    attach("pc", "eth0", "br0", Some("00:00:5e:00:53:01"));
    attach("other", "eth0", "br1", Some("02:00:00:00:02:20"));
    ns_ip("nas", &["addr", "add", "10.1.5.20/22", "dev", "eth0"]);
    ns_ip("nas", &["addr", "add", "192.168.50.10/24", "dev", "eth1"]);
    ns_ip(
        "nas",
        &[
            "route",
            "add",
            "default",
            "via",
            "192.168.50.1",
            "dev",
            "eth1",
        ],
    );
    ns_ip("nas", &["link", "add", "docker0", "type", "bridge"]);
    ns_ip("nas", &["addr", "add", "10.1.6.1/24", "dev", "docker0"]);
    ns_ip("nas", &["link", "set", "docker0", "up"]);
    ns_ip("other", &["addr", "add", "192.168.50.20/24", "dev", "eth0"]);
    wait_forwarding(&["v-nas-eth0", "v-nas-eth1", "v-pc-eth0", "v-other-eth0"]);
    let both = [9u16, 7];

    // the PC's network, exactly: eth0, its own broadcast and 255.255.255.255, ports 9 and 7
    let (out, pc, other) = test_wake(&["--net", "10.1.4.0/22", "--count", "2"]);
    assert!(out.status.success());
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &both,
        2,
        "exact",
    );
    assert!(other.is_empty(), "{other:?}");

    // the PC's own address with its prefix works the same
    let (out, pc, other) = test_wake(&["--net", "10.1.7.33/22", "--count", "1"]);
    assert!(out.status.success());
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &both,
        1,
        "address",
    );
    assert!(other.is_empty());

    // docker0 matches 10.1.6.0/24 exactly but is never used: eth0 contains the network
    let (out, pc, other) = test_wake(&["--net", "10.1.6.0/24", "--count", "1"]);
    assert!(out.status.success());
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &both,
        1,
        "contains",
    );
    assert!(other.is_empty());

    // no interface on the network: nothing at all is sent
    let (out, pc, other) = test_wake(&["--net", "10.99.0.0/16"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("보낼 랜카드가 없습니다"));
    assert!(pc.is_empty() && other.is_empty());

    // without --net, every LAN interface: the PC's LAN from eth0, LAN B from eth1, not docker0
    let (out, pc, other) = test_wake(&["--count", "1"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("docker0"), "{stdout}");
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &both,
        1,
        "all, LAN A",
    );
    expect(
        &other,
        "192.168.50.10",
        &["255.255.255.255", "192.168.50.255"],
        &both,
        1,
        "all, LAN B",
    );

    // one named interface
    let (out, pc, other) = test_wake(&["--iface", "eth1", "--count", "1"]);
    assert!(out.status.success());
    assert!(pc.is_empty());
    expect(
        &other,
        "192.168.50.10",
        &["255.255.255.255", "192.168.50.255"],
        &both,
        1,
        "iface",
    );

    // a second port on the PC's network that lost its cable but kept its address, with a name
    // that sorts first: the port with carrier is used, and the unplugged one, named, says so
    ip(&[
        "link",
        "add",
        "v-nas-bond0",
        "type",
        "veth",
        "peer",
        "name",
        "bond0",
        "netns",
        "nas",
    ]);
    // the switch side stays down: bond0 has no carrier
    ip(&["link", "set", "v-nas-bond0", "master", "br0"]);
    ns_ip("nas", &["link", "set", "bond0", "up"]);
    ns_ip("nas", &["addr", "add", "10.1.5.19/22", "dev", "bond0"]);
    let (out, pc, other) = test_wake(&["--net", "10.1.4.0/22", "--count", "1"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("eth0 10.1.5.20/22") && !stdout.contains("bond0"),
        "{stdout}"
    );
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &both,
        1,
        "carrier",
    );
    assert!(other.is_empty());
    let (out, pc, _) = test_wake(&["--iface", "bond0", "--count", "1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout
            .contains("bond0 10.1.5.19/22 -> 255.255.255.255, 10.1.7.255 포트 9,7 ×1 (연결 없음)"),
        "{stdout}"
    );
    assert!(pc.is_empty(), "{pc:?}");
    ip(&["link", "del", "v-nas-bond0"]);

    relay_loop_in_the_nas_namespace();
    configured_binary_in_the_nas_namespace();

    // DSM with Virtual Machine Manager: the address is on the Open vSwitch bridge ovs_eth0, and
    // with a /24 while the PC has /22. The relay sends to its own /24 broadcast, which reaches
    // the PC on the same wire, and never to the PC's 10.1.7.255, which the /24 would route
    // to the gateway.
    ns_ip("nas", &["addr", "del", "10.1.5.20/22", "dev", "eth0"]);
    ns_ip("nas", &["link", "add", "ovs_eth0", "type", "bridge"]);
    ns_ip("nas", &["link", "set", "eth0", "master", "ovs_eth0"]);
    ns_ip("nas", &["link", "set", "ovs_eth0", "up"]);
    ns_ip("nas", &["addr", "add", "10.1.5.20/24", "dev", "ovs_eth0"]);
    let end = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(
        &run("ip", &["-n", "nas", "-d", "link", "show", "dev", "eth0"]).stdout,
    )
    .contains("state forwarding")
    {
        assert!(Instant::now() < end, "ovs_eth0 does not forward");
        thread::sleep(Duration::from_millis(20));
    }
    let (out, pc, other) = test_wake(&["--net", "10.1.4.0/22", "--count", "1"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("ovs_eth0"));
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.5.255"],
        &both,
        1,
        "within",
    );
    assert!(other.is_empty());
}

/// The relay loop with the real interface listing and sending, run from a thread inside the nas
/// namespace against a test server on that namespace's loopback.
fn relay_loop_in_the_nas_namespace() {
    let pc = Capture::start("pc", "eth0");
    let other = Capture::start("other", "eth0");
    let worker = thread::spawn(|| {
        let _log = common::capture_log();
        enter("nas");
        let id = Identity::from_seed(&common::RELAY_SEED).id;
        let mut queued = Some(signed(
            &json!({
                "v": 1, "job": "00000000000000000000000000000abc", "relay": id,
                "iat": unix_now(), "exp": unix_now() + 60, "net": "10.1.4.0/22",
                "macs": [common::PC_MAC], "ports": [9, 7], "repeat": 3, "gap_ms": 100
            }),
            &command_signing_key(),
            1,
        ));
        let server = TestServer::start(move |seen, now| {
            if seen.op == "hello" {
                return Reply::ok(json!({"server_time": now, "state": "active"}));
            }
            let commands: Vec<Value> = queued.take().into_iter().collect();
            Reply::ok(json!({"server_time": now, "state": "active", "commands": commands}))
        });
        let env = SystemEnv {
            exe: None,
            status: None,
        };
        let mut relay = Relay::new(relay_config(&server), env);
        relay.step();
        assert_eq!(relay.state(), State::Active);
        relay.step();
        relay.step();
        assert!(server.refused().is_empty(), "{:?}", server.refused());
        server.seen()
    });
    let seen = worker.join().unwrap();
    let pc = pc.finish();
    let other = other.finish();
    // what the relay reported about the nas, from getifaddrs and /proc in that namespace
    let hello = &seen[0].payload;
    let names: Vec<&str> = hello["ifaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["eth0", "eth1"]);
    assert_eq!(hello["ifaces"][0]["ipv4"], json!(["10.1.5.20/22"]));
    assert_eq!(hello["ifaces"][1]["gateway"], "192.168.50.1");
    let result = &seen.last().unwrap().payload["results"][0];
    assert_eq!(result["job"], "00000000000000000000000000000abc");
    assert_eq!(result["iface"], "eth0");
    assert_eq!(result["packets"], 12);
    assert_eq!(result["error"], Value::Null);
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &[9, 7],
        3,
        "relay loop",
    );
    assert!(other.is_empty(), "{other:?}");
}

/// Runs `relay` in the nas namespace: through `unshare --user` as an unmapped user without
/// capabilities, or as root when the test runs as root.
fn in_nas(relay: &str) -> Command {
    let mut cmd = Command::new("ip");
    cmd.args(["netns", "exec", "nas"]);
    if !is_root_outside() {
        cmd.args(["unshare", "--user"]);
    }
    cmd.arg(relay);
    cmd
}

/// The `run` command of a binary built for the test server, as the DSM boot-up task runs it.
fn configured_binary_in_the_nas_namespace() {
    let dir = Path::new("/run/hjv-relay-test/run");
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    let bin = dir.join("hjv-relay");
    fs::copy(std::env::var(CONFIGURED).unwrap(), &bin).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command_sent = false;
    let server = thread::spawn(move || {
        enter("nas");
        TestServer::start(move |seen, now| {
            if seen.op == "hello" {
                return Reply::ok(json!({"server_time": now, "state": "active"}));
            }
            if command_sent {
                return Reply::ok(json!({"server_time": now, "state": "active"}));
            }
            command_sent = true;
            let command = signed(
                &json!({
                    "v": 1, "job": "00000000000000000000000000000def", "relay": seen.id,
                    "iat": now, "exp": now + 60, "net": "10.1.4.0/22",
                    "macs": [common::PC_MAC], "ports": [9, 7], "repeat": 2, "gap_ms": 100
                }),
                &command_signing_key(),
                1,
            );
            Reply::ok(json!({"server_time": now, "state": "active", "commands": [command]}))
        })
    })
    .join()
    .unwrap();
    let ca = dir.join("ca.pem");
    fs::write(&ca, &server.ca_pem).unwrap();
    fs::set_permissions(&ca, fs::Permissions::from_mode(0o644)).unwrap();
    let (state, status, log) = (dir.join("state"), dir.join("status"), dir.join("relay.log"));
    let server_arg = format!("localhost:{}", server.port);
    let run_args = [
        "run".as_ref(),
        "--state".as_ref(),
        state.as_os_str(),
        "--server".as_ref(),
        server_arg.as_ref(),
        "--ca".as_ref(),
        ca.as_os_str(),
        "--status-file".as_ref(),
        status.as_os_str(),
    ];
    let bin = bin.to_str().unwrap().to_owned();

    let pc = Capture::start("pc", "eth0");
    let other = Capture::start("other", "eth0");
    let out = File::create(&log).unwrap();
    let mut child = in_nas(&bin)
        .args(run_args)
        .stdout(Stdio::from(out.try_clone().unwrap()))
        .stderr(Stdio::from(out))
        .spawn()
        .unwrap();
    common::wait_until("the relay's result", Duration::from_secs(30), || {
        server.seen().iter().any(|s| {
            s.payload["results"]
                .as_array()
                .is_some_and(|r| !r.is_empty())
        })
    });
    let (pc, other) = (pc.finish(), other.finish());
    expect(
        &pc,
        "10.1.5.20",
        &["255.255.255.255", "10.1.7.255"],
        &[9, 7],
        2,
        "run",
    );
    assert!(other.is_empty(), "{other:?}");
    let seen = server.seen();
    let result = seen
        .iter()
        .find_map(|s| {
            s.payload["results"]
                .as_array()
                .and_then(|r| r.first().cloned())
        })
        .unwrap();
    assert_eq!(result["job"], "00000000000000000000000000000def");
    assert_eq!(
        (result["packets"].as_u64(), result["error"].is_null()),
        (Some(8), true)
    );
    assert_eq!(seen[0].op, "hello");
    assert!(server.refused().is_empty(), "{:?}", server.refused());

    // a second relay on the same state folder ends at once, and the boot-up loop with it
    let second = in_nas(&bin).args(run_args).output().unwrap();
    assert_eq!(second.status.code(), Some(75), "{second:?}");

    let report = in_nas(&bin)
        .args(["status".as_ref(), "--state".as_ref(), state.as_os_str()])
        .args(["--status-file".as_ref(), status.as_os_str()])
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&report.stdout).to_string();
    eprint!("{report}");
    assert!(report.contains("실행 중: 예"), "{report}");
    assert!(report.contains("등록 상태: active"), "{report}");
    assert!(
        report.contains(&format!("중계기 ID: {}", seen[0].id)),
        "{report}"
    );
    assert!(
        report.contains("job 00000000000000000000000000000def 랜카드 eth0 패킷 8 결과 ok"),
        "{report}"
    );

    // a new executable uploaded over the old one: the relay ends with 0 for the loop to restart it
    let new = dir.join("hjv-relay.upload");
    fs::copy(&bin, &new).unwrap();
    fs::rename(&new, &bin).unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        assert!(
            Instant::now() < end,
            "the relay did not notice its new executable"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let text = fs::read_to_string(&log).unwrap();
    eprint!("{text}");
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("실행 파일이 바뀌었습니다"), "{text}");
    assert!(text.contains("등록 코드"), "{text}");
    if is_root_outside() {
        assert!(
            text.contains("권한을 uid 65534 gid 65534로 낮췄습니다."),
            "{text}"
        );
    } else {
        assert!(text.contains("root가 아니어서"), "{text}");
    }
    // the key stays in the state folder, readable only by its owner
    let key = state.join("relay.key");
    assert_eq!(
        fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
