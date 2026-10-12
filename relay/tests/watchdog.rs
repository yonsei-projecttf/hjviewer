// SPDX-License-Identifier: AGPL-3.0-only

//! The DSM boot-up script exactly as docs/relay-dsm.md prints it, run by this machine's POSIX `sh`
//! (paths moved to a temporary folder, the 30-second pause shortened, the SHA-256 placeholder of
//! its first line filled in). Against a stand-in for hjv-relay: it runs only a file whose SHA-256
//! is the first line's, and only the checked copy in the state folder, never the uploaded file;
//! it ends on a file that does not match, also one that changes between two runs; it computes
//! the hash with sha256sum or, without it, with openssl, and with neither runs nothing; it ends on
//! the stop marker, before a run and right after one; it waits for a volume that is not there
//! yet; it restarts the relay after 0 and 1, ends on 2 and 78 and, after a few tries, on 75.
//! Against the real relay (built for a test server): the relay drops the log lines older than
//! 180 days, runs from the checked copy, ends with the loop on the stop marker, and ends on an
//! upload whose SHA-256 the task does not have yet, until the task gets the new value. The
//! guide's other tasks run only the checked copy too.

mod common;

use common::{configured_relay, Reply, TestServer};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn guide() -> String {
    let guide = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/relay-dsm.md");
    fs::read_to_string(guide).unwrap()
}

fn boot_script() -> String {
    let text = guide();
    let start = text.find("```sh\n").expect("no sh block in the guide") + 6;
    let end = start + text[start..].find("```").unwrap();
    text[start..end].to_owned()
}

const PLACEHOLDER: &str = "H=0000000000000000000000000000000000000000000000000000000000000000\n";

fn sha256_hex(path: &Path) -> String {
    Sha256::digest(fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A temporary NAS: `share` stands for /volume1/hjv-relay, `state` for /volume1/@hjv-relay. When
/// it goes, so do the boot-up loops and relays it started (also when a test fails).
struct Nas {
    dir: PathBuf,
}

impl Nas {
    fn new(name: &str) -> Nas {
        let dir =
            std::env::temp_dir().join(format!("hjv-relay-watchdog-{name}-{}", std::process::id()));
        if dir.exists() {
            kill_processes_of(&dir);
            let _ = fs::remove_dir_all(&dir);
        }
        fs::create_dir_all(&dir).unwrap();
        Nas { dir }
    }

    fn share(&self) -> PathBuf {
        self.dir.join("share")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn bin(&self) -> PathBuf {
        self.share().join("hjv-relay")
    }

    fn marker(&self) -> PathBuf {
        self.share().join("hjv-relay.stop")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.share().join("hjv-relay.log")).unwrap_or_default()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The boot-up script for this NAS, with `hash` as its first line's value; `edit` changes it
    /// further.
    fn script(&self, hash: &str, edit: impl Fn(String) -> String) -> String {
        let script = boot_script();
        assert!(script.starts_with(PLACEHOLDER), "{script}");
        edit(script)
            .replacen(PLACEHOLDER, &format!("H={hash}\n"), 1)
            .replace("/volume1/@hjv-relay", &self.state().display().to_string())
            .replace("/volume1/hjv-relay", &self.share().display().to_string())
            .replace("sleep 30", "sleep 0.2")
    }

    /// Runs the boot-up script the way the task does; it must end at once, successfully.
    fn start(&self, hash: &str) {
        self.start_script(&self.script(hash, |s| s), None);
    }

    fn start_script(&self, script: &str, path: Option<&Path>) {
        let started = Instant::now();
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        if let Some(path) = path {
            cmd.env("PATH", path);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Processes (the loop's sh and what it started) that name this NAS's folder.
    fn processes(&self) -> Vec<i32> {
        processes_of(&self.dir)
    }

    fn wait_for_the_loop_to_end(&self) {
        wait_for("the loop to end", || self.processes().is_empty());
    }
}

impl Drop for Nas {
    fn drop(&mut self) {
        kill_processes_of(&self.dir);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn processes_of(dir: &Path) -> Vec<i32> {
    let needle = dir.as_os_str().as_encoded_bytes().to_vec();
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() as i32 {
            continue;
        }
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if cmdline.windows(needle.len()).any(|w| w == needle) {
            found.push(pid);
        }
    }
    found
}

fn kill_processes_of(dir: &Path) {
    for pid in processes_of(dir) {
        // SAFETY: a signal to a process of this test.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A stand-in for hjv-relay that records how it was started ($0 and the arguments), says so on
/// stdout (the log), runs `body` and then exits with the next code of `codes`. Returns its
/// SHA-256.
fn fake_relay(nas: &Nas, codes: &[u8], body: &str) -> String {
    fs::create_dir_all(nas.share()).unwrap();
    fs::write(
        nas.dir.join("codes"),
        codes.iter().map(|c| format!("{c}\n")).collect::<String>(),
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\necho \"$0 $*\" >> '{d}/calls'\necho \"stand-in started: $*\"\n{body}\n\
         code=$(head -n 1 '{d}/codes'); sed -i 1d '{d}/codes'\nexit \"${{code:-0}}\"\n",
        d = nas.dir.display()
    );
    fs::write(nas.bin(), script).unwrap();
    // an upload through File Station may come without the execute bit: the copy gets it
    fs::set_permissions(nas.bin(), fs::Permissions::from_mode(0o644)).unwrap();
    sha256_hex(&nas.bin())
}

/// How the loop starts the relay: the checked copy, with the guide's arguments.
fn expected_call(nas: &Nas) -> String {
    let (share, state) = (nas.share(), nas.state());
    format!(
        "{} run --state {} --log {} --watch {} --stop {}",
        state.join("hjv-relay").display(),
        state.display(),
        share.join("hjv-relay.log").display(),
        share.join("hjv-relay").display(),
        share.join("hjv-relay.stop").display()
    )
}

#[test]
fn the_boot_script_of_the_guide() {
    let script = boot_script();
    assert!(script.starts_with(PLACEHOLDER), "{script}");
    assert!(
        guide().contains("**첫 줄의 `0000…0000`(64자)을 3장 4번의 값으로 바꿉니다.**"),
        "the guide must say what goes into the first line"
    );
    let nas = Nas::new("main");

    // exits 0 (a new executable) and 1 (an error) are restarted; 75 (another relay runs) is tried
    // four times; each run is the checked copy with the guide's arguments
    let hash = fake_relay(&nas, &[0, 1, 75, 75, 75, 75], "");
    nas.start(&hash);
    wait_for("six runs", || nas.calls().len() >= 6);
    nas.wait_for_the_loop_to_end();
    assert_eq!(nas.calls(), vec![expected_call(&nas); 6]);
    assert_eq!(
        nas.log().matches("stand-in started: run --state").count(),
        6
    );
    // the copy: in a folder only its owner can use, executable by its owner only, the same bytes
    let copy = nas.state().join("hjv-relay");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&nas.state()), 0o700);
    assert_eq!(mode(&copy), 0o700);
    assert_eq!(fs::read(&copy).unwrap(), fs::read(nas.bin()).unwrap());
    // no temporary copies are left behind
    let names: Vec<String> = fs::read_dir(nas.state())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["hjv-relay"]);

    // 78 (configuration) and 2 (command line) end the loop at once
    for code in [78, 2] {
        fs::remove_file(nas.dir.join("calls")).unwrap();
        let hash = fake_relay(&nas, &[code], "");
        nas.start(&hash);
        nas.wait_for_the_loop_to_end();
        assert_eq!(nas.calls(), [expected_call(&nas)], "exit {code}");
    }

    // the loop no longer rotates the log: the relay keeps it (--log)
    fs::remove_file(nas.dir.join("calls")).unwrap();
    let big = "x".repeat(3 * 1024 * 1024);
    fs::write(nas.share().join("hjv-relay.log"), &big).unwrap();
    let hash = fake_relay(&nas, &[78], "");
    nas.start(&hash);
    nas.wait_for_the_loop_to_end();
    assert!(nas.log().starts_with(&big));
    assert!(!nas.share().join("hjv-relay.log.1").exists());
}

#[test]
fn a_file_that_does_not_match_is_never_run() {
    let nas = Nas::new("mismatch");
    let pwned = nas.dir.join("pwned");

    // the task has the value of another file: nothing runs, the loop ends and says so with the
    // first 8 digits of what it found
    let hash = fake_relay(&nas, &[0], "");
    let other = "1".repeat(64);
    nas.start(&other);
    nas.wait_for_the_loop_to_end();
    assert!(nas.calls().is_empty());
    let line = format!(
        "hjv-relay의 SHA-256({}…)이 작업 스크립트 첫 줄의 값과 달라 실행하지 않고 감시 루프를 끝냅니다.",
        &hash[..8]
    );
    assert!(nas.log().contains(&line), "{}", nas.log());
    assert!(!nas.log().contains(&hash), "the full value is never shown");
    assert!(!nas.state().join("hjv-relay").exists());

    // a matching file that is swapped for another while it runs (someone who can write the
    // shared folder): the loop checks again before the next run and ends
    fs::remove_file(nas.share().join("hjv-relay.log")).unwrap();
    let evil = nas.dir.join("evil");
    fs::write(&evil, format!("#!/bin/sh\ntouch '{}'\n", pwned.display())).unwrap();
    fs::set_permissions(&evil, fs::Permissions::from_mode(0o755)).unwrap();
    let body = format!(
        "cp '{evil}' '{bin}.new' && mv '{bin}.new' '{bin}'",
        evil = evil.display(),
        bin = nas.bin().display()
    );
    let hash = fake_relay(&nas, &[0, 0], &body);
    nas.start(&hash);
    nas.wait_for_the_loop_to_end();
    assert_eq!(nas.calls(), [expected_call(&nas)]);
    assert!(!pwned.exists());
    assert!(
        nas.log()
            .contains("이 작업 스크립트 첫 줄의 값과 달라 실행하지 않고"),
        "{}",
        nas.log()
    );
    // the checked copy stayed what it was
    assert_eq!(sha256_hex(&nas.state().join("hjv-relay")), hash);

    // a copy in the state folder that is not this version (an older one, or damaged) is
    // replaced before anything runs
    fs::remove_file(nas.dir.join("calls")).unwrap();
    let hash = fake_relay(&nas, &[78], "");
    fs::write(
        nas.state().join("hjv-relay"),
        format!("#!/bin/sh\ntouch '{}'\n", pwned.display()),
    )
    .unwrap();
    nas.start(&hash);
    nas.wait_for_the_loop_to_end();
    assert_eq!(nas.calls(), [expected_call(&nas)]);
    assert!(!pwned.exists());
    assert_eq!(sha256_hex(&nas.state().join("hjv-relay")), hash);
}

/// A folder of links to this machine's tools, without the ones in `without`, for PATH.
fn tools(nas: &Nas, without: &[&str]) -> PathBuf {
    let bin = nas.dir.join(format!("bin-{}", without.join("-")));
    fs::create_dir_all(&bin).unwrap();
    for tool in [
        "sh",
        "nohup",
        "date",
        "cp",
        "chmod",
        "mv",
        "rm",
        "mkdir",
        "sleep",
        "head",
        "sed",
        "sha256sum",
        "openssl",
        "touch",
    ] {
        if without.contains(&tool) {
            continue;
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {tool}"))
            .output()
            .unwrap();
        let path = String::from_utf8(out.stdout).unwrap();
        assert!(out.status.success(), "{tool} is not installed");
        std::os::unix::fs::symlink(path.trim(), bin.join(tool)).unwrap();
    }
    bin
}

#[test]
fn the_hash_comes_from_sha256sum_or_openssl() {
    let nas = Nas::new("tools");
    // DSM without sha256sum: openssl dgst gives the same value
    let hash = fake_relay(&nas, &[78], "");
    let path = tools(&nas, &["sha256sum"]);
    nas.start_script(&nas.script(&hash, |s| s), Some(&path));
    nas.wait_for_the_loop_to_end();
    assert_eq!(nas.calls(), [expected_call(&nas)]);

    // neither: nothing runs
    fs::remove_file(nas.dir.join("calls")).unwrap();
    fs::remove_dir_all(nas.state()).unwrap();
    let hash = fake_relay(&nas, &[78], "");
    let path = tools(&nas, &["sha256sum", "openssl"]);
    nas.start_script(&nas.script(&hash, |s| s), Some(&path));
    nas.wait_for_the_loop_to_end();
    assert!(nas.calls().is_empty());
    assert!(
        nas.log().contains("hjv-relay의 SHA-256(?…)이"),
        "{}",
        nas.log()
    );
}

#[test]
fn the_stop_marker_ends_the_loop_without_a_reboot() {
    let nas = Nas::new("stop");
    let said = "hjv-relay.stop이 있어 감시 루프를 끝냅니다.";

    // there before the start (a folder made in File Station): nothing runs
    let hash = fake_relay(&nas, &[0], "");
    fs::create_dir(nas.marker()).unwrap();
    nas.start(&hash);
    nas.wait_for_the_loop_to_end();
    assert!(nas.calls().is_empty());
    assert_eq!(nas.log().matches(said).count(), 1, "{}", nas.log());

    // put there while the relay runs: the relay ends on it (--stop, here a stand-in waiting for
    // it), and the loop ends right after, without its pause
    fs::remove_dir(nas.marker()).unwrap();
    let wait = format!(
        "while [ ! -e '{}' ]; do sleep 0.05; done",
        nas.marker().display()
    );
    let hash = fake_relay(&nas, &[0, 0], &wait);
    // a pause long enough to tell whether the loop takes it
    let script = nas.script(&hash, |s| s.replace("sleep 30", "sleep 20"));
    nas.start_script(&script, None);
    wait_for("the run", || nas.calls().len() == 1);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(nas.processes().len(), 2, "the loop and the relay");
    fs::write(nas.marker(), "").unwrap();
    let stopped = Instant::now();
    nas.wait_for_the_loop_to_end();
    assert!(stopped.elapsed() < Duration::from_secs(5));
    assert_eq!(nas.log().matches(said).count(), 2, "{}", nas.log());
    // removing the marker brings nothing back: only the task's Run (or a reboot) does
    fs::remove_file(nas.marker()).unwrap();
    thread::sleep(Duration::from_millis(600));
    assert_eq!(nas.calls().len(), 1);
}

#[test]
fn a_volume_that_appears_after_the_boot_up_task() {
    // the task runs before the volume (an encrypted one waiting to be unlocked) or the shared
    // folder is there: the loop must still be waiting when they appear, and makes the state
    // folder only then
    let nas = Nas::new("late");
    let hash = fake_relay(&nas, &[78], "");
    let content = fs::read(nas.bin()).unwrap();
    fs::remove_dir_all(nas.share()).unwrap();
    nas.start(&hash);
    thread::sleep(Duration::from_millis(500));
    assert!(!nas.share().exists());
    assert!(!nas.state().exists());
    assert_eq!(nas.processes().len(), 1, "the loop waits");
    fs::create_dir(nas.share()).unwrap();
    fs::write(nas.bin(), content).unwrap();
    nas.wait_for_the_loop_to_end();
    assert_eq!(nas.calls(), [expected_call(&nas)]);
    assert!(nas.log().contains("stand-in started: run --state"));
}

#[test]
fn the_tasks_of_the_guide_run_only_the_checked_copy() {
    // the field test and the status task are root tasks too: they must not run the file anyone
    // with write access to the shared folder can replace
    let text = guide();
    let mut in_code = false;
    let mut commands = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if !in_code {
            continue;
        }
        assert!(
            !line.starts_with("/volume1/hjv-relay/hjv-relay"),
            "a task runs the uploaded file: {line}"
        );
        for command in ["test-wake", "status", "run", "init"] {
            if line.contains(&format!("hjv-relay {command} ")) {
                assert!(
                    line.starts_with("/volume1/@hjv-relay/hjv-relay ")
                        || line.starts_with("\"$X\" run "),
                    "{line}"
                );
                commands += 1;
            }
        }
    }
    assert!(commands >= 2, "the guide's test-wake and status tasks");
    // the boot-up loop starts only $X, the copy it checked
    let script = boot_script();
    assert!(script.contains("X=$S/hjv-relay "));
    assert_eq!(script.matches("\"$X\" run ").count(), 1);
}

/// The first log line with `text` and its position.
fn position(log: &str, text: &str) -> usize {
    log.find(text)
        .unwrap_or_else(|| panic!("{text:?} is not in the log:\n{log}"))
}

#[test]
fn the_real_relay_under_the_boot_script() {
    let relay = configured_relay();
    let nas = Nas::new("real");
    fs::create_dir_all(nas.share()).unwrap();
    fs::copy(&relay, nas.bin()).unwrap();
    fs::set_permissions(nas.bin(), fs::Permissions::from_mode(0o644)).unwrap();
    let hash = sha256_hex(&nas.bin());

    // a log with lines older and younger than 180 days, in the relay's and the loop's form
    let now = hjv_relay::clock::system_now();
    let stamp = |days: i64| hjv_relay::system::local_timestamp(now - days * 86_400);
    fs::write(
        nas.share().join("hjv-relay.log"),
        format!(
            "{} old line\n{} older than 180 days\n{} kept line\n{} recent line\n",
            stamp(200),
            stamp(181),
            stamp(179),
            stamp(1)
        ),
    )
    .unwrap();

    let server = TestServer::start(|seen, now| {
        let _ = seen;
        Reply::ok(json!({"server_time": now, "state": "active"}))
    });
    let ca = nas.dir.join("ca.pem");
    fs::write(&ca, &server.ca_pem).unwrap();
    let status = nas.dir.join("status");
    let test_args = format!(
        "run --state \"$S\" --server localhost:{} --ca {} --status-file {}",
        server.port,
        ca.display(),
        status.display()
    );
    let script = |hash: &str| nas.script(hash, |s| s.replace("run --state \"$S\"", &test_args));
    let hellos = || server.seen().iter().filter(|s| s.op == "hello").count();

    nas.start_script(&script(&hash), None);
    wait_for("hello and a poll", || {
        server.seen().iter().any(|s| s.op == "poll")
    });
    let log = nas.log();
    assert!(
        !log.contains("old line") && !log.contains("older than 180 days"),
        "{log}"
    );
    assert!(position(&log, "kept line") < position(&log, "recent line"));
    assert!(position(&log, "recent line") < position(&log, "hjv-relay 1.6.0 시작"));
    assert!(server.refused().is_empty(), "{:?}", server.refused());
    // it runs from the checked copy in the state folder (its command line: the relay makes itself
    // non-dumpable, so /proc/<pid>/exe is closed to this test)
    let copy = nas.state().join("hjv-relay");
    let started_as: Vec<Vec<String>> = nas
        .processes()
        .iter()
        .filter_map(|pid| fs::read(format!("/proc/{pid}/cmdline")).ok())
        .map(|c| {
            String::from_utf8_lossy(&c)
                .split('\0')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|args| args.get(1).is_some_and(|a| a == "run"))
        .collect();
    assert_eq!(started_as.len(), 1, "{started_as:?}");
    assert_eq!(started_as[0][0], copy.display().to_string());

    // the stop marker: the relay and its loop end, nothing is left running
    fs::create_dir(nas.marker()).unwrap();
    nas.wait_for_the_loop_to_end();
    let log = nas.log();
    let relay_said = format!("멈춤 표지 {}이(가) 있어 끝냅니다.", nas.marker().display());
    assert!(
        position(&log, &relay_said) < position(&log, "hjv-relay.stop이 있어 감시 루프를 끝냅니다."),
        "{log}"
    );

    // the task's Run again, then an upload of another version: the relay notices it and ends,
    // and the loop does not run a file whose SHA-256 the task does not have
    fs::remove_dir(nas.marker()).unwrap();
    let before = hellos();
    nas.start_script(&script(&hash), None);
    wait_for("the relay to start again", || hellos() > before);
    let mut newer = fs::read(&relay).unwrap();
    newer.extend_from_slice(b"another build");
    let upload = nas.share().join("upload.tmp");
    fs::write(&upload, &newer).unwrap();
    fs::rename(&upload, nas.bin()).unwrap();
    let new_hash = sha256_hex(&nas.bin());
    nas.wait_for_the_loop_to_end();
    let log = nas.log();
    let changed = position(&log, "실행 파일이 바뀌었습니다(");
    let refused = position(
        &log,
        &format!(
            "hjv-relay의 SHA-256({}…)이 작업 스크립트 첫 줄의 값과 달라",
            &new_hash[..8]
        ),
    );
    assert!(changed < refused, "{log}");
    assert_eq!(sha256_hex(&copy), hash, "the old copy was not replaced");

    // the task with the new value: the new version runs
    let before = hellos();
    nas.start_script(&script(&new_hash), None);
    wait_for("the new version", || hellos() > before);
    assert_eq!(sha256_hex(&copy), new_hash);
    fs::create_dir(nas.marker()).unwrap();
    nas.wait_for_the_loop_to_end();
    assert!(server.refused().is_empty(), "{:?}", server.refused());
    eprint!("{}", nas.log());
}
