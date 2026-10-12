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
//! guide's other tasks run only the checked copy too. Symbolic links that someone who can write
//! to the shared folder puts in place of the log, the status and test outputs, the upload or the
//! stop marker never make root write to, create or run another file: the shell sends nothing
//! into the folder, and the relay opens what it writes there itself.

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

/// A stand-in for hjv-relay that records how it was started ($0 and the arguments), says so in
/// the file of its `--log` (the loop gives it /dev/null for stdout and stderr, as it does the
/// real relay, which opens its log itself), runs `body` and then exits with the next code of
/// `codes`. Returns its SHA-256.
fn fake_relay(nas: &Nas, codes: &[u8], body: &str) -> String {
    fs::create_dir_all(nas.share()).unwrap();
    fs::write(
        nas.dir.join("codes"),
        codes.iter().map(|c| format!("{c}\n")).collect::<String>(),
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\necho \"$0 $*\" >> '{d}/calls'\n\
         log= prev=\nfor a in \"$@\"; do [ \"$prev\" = --log ] && log=$a; prev=$a; done\n\
         echo \"stand-in started: $*\" >> \"${{log:-/dev/null}}\"\n{body}\n\
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

/// The lines of the guide's code blocks, trimmed.
fn code_lines(text: &str) -> Vec<String> {
    let mut in_code = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
        } else if in_code {
            lines.push(line.to_owned());
        }
    }
    lines
}

/// The targets of a shell line's output redirections (`>`, `>>`, `2>`, `2>&1`).
fn redirections(line: &str) -> Vec<String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut targets = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let rest = token.trim_start_matches(|c: char| c.is_ascii_digit());
        if let Some(op) = rest.strip_prefix(">>").or_else(|| rest.strip_prefix('>')) {
            let target = if op.is_empty() {
                tokens.get(i + 1).copied().unwrap_or_default()
            } else {
                op
            };
            targets.push(target.trim_end_matches([';', ')']).to_owned());
        }
    }
    targets
}

#[test]
fn the_tasks_of_the_guide_run_only_the_checked_copy() {
    // the field test and the status task are root tasks too: they must not run the file anyone
    // with write access to the shared folder can replace
    let text = guide();
    let mut commands = 0;
    for line in code_lines(&text) {
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

    // none of the root tasks sends output into the shared folder through the shell, which would
    // follow a symbolic link put there: the relay opens those files itself (--log, --out)
    let tasks: Vec<String> = code_lines(&text)
        .into_iter()
        .filter(|l| l.starts_with("/volume1/"))
        .collect();
    assert_eq!(tasks.len(), 2, "{tasks:?}");
    for line in tasks.iter().map(String::as_str).chain(script.lines()) {
        for target in redirections(line) {
            assert!(
                ["/dev/null", "&1", "\"$L\""].contains(&target.as_str()),
                "{target}: {line}"
            );
        }
    }
    for task in &tasks {
        assert!(task.contains(" --out /volume1/hjv-relay/"), "{task}");
    }
    // the loop's own lines go to the log only after it checked that the log is no link
    let writes: Vec<&str> = script.lines().filter(|l| l.contains("\"$L\";")).collect();
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert!(
        writes[0].contains("if [ ! -L \"$L\" ] && { [ -f \"$L\" ] || [ ! -e \"$L\" ]; }; then"),
        "{}",
        writes[0]
    );
}

/// The guide's task line that runs `hjv-relay <command> `, with the NAS's paths moved to `nas`.
fn task_line(nas: &Nas, command: &str) -> String {
    code_lines(&guide())
        .into_iter()
        .find(|l| l.starts_with("/volume1/") && l.contains(&format!("hjv-relay {command} ")))
        .unwrap_or_else(|| panic!("no {command} task in the guide"))
        .replace("/volume1/@hjv-relay", &nas.state().display().to_string())
        .replace("/volume1/hjv-relay", &nas.share().display().to_string())
}

/// Runs a task line as the scheduler does; returns its exit code.
fn run_task(line: &str) -> i32 {
    let out = Command::new("sh").arg("-c").arg(line).output().unwrap();
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    out.status.code().unwrap()
}

#[test]
fn links_in_the_shared_folder_are_never_followed() {
    // someone who can write to the shared folder, but is no DSM administrator, puts symbolic
    // links where root writes: their targets, outside the folder, stay as they are, and nothing
    // is created through a link that points nowhere
    let nas = Nas::new("links");
    let outside = nas.dir.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("victim");
    let nowhere = outside.join("new");
    fs::write(&victim, "keep\n").unwrap();
    let put = |name: &str, what: Option<&Path>| {
        let path = nas.share().join(name);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&path);
        if let Some(target) = what {
            std::os::unix::fs::symlink(target, &path).unwrap();
        }
    };
    let untouched = || {
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep\n");
        assert!(!nowhere.exists());
    };
    // the real relay with an unknown option: it opens its log (only a plain file), says so there
    // and ends with a configuration error, whatever this build is configured with
    let relay = PathBuf::from(env!("CARGO_BIN_EXE_hjv-relay"));
    fs::create_dir_all(nas.share()).unwrap();
    fs::copy(&relay, nas.bin()).unwrap();
    let hash = sha256_hex(&nas.bin());
    let bogus = nas.script(&hash, |s| {
        s.replace("run --state \"$S\"", "run --state \"$S\" --bogus")
    });
    nas.start_script(&bogus, None);
    nas.wait_for_the_loop_to_end();
    assert!(
        nas.log().contains("설정 오류: run: unknown option --bogus"),
        "{}",
        nas.log()
    );
    assert_eq!(sha256_hex(&nas.state().join("hjv-relay")), hash);

    // the log a link: the relay does not write through it (nor anywhere else: the loop gives it
    // /dev/null) and does not run, and neither does the loop's own line on the stop marker or on
    // a file that does not match
    for target in [&victim, &nowhere] {
        put("hjv-relay.log", Some(target));
        nas.start_script(&bogus, None);
        nas.wait_for_the_loop_to_end();
        untouched();
        nas.start(&hash);
        nas.wait_for_the_loop_to_end();
        untouched();
        nas.start(&"1".repeat(64));
        nas.wait_for_the_loop_to_end();
        untouched();
        fs::create_dir(nas.marker()).unwrap();
        nas.start(&hash);
        nas.wait_for_the_loop_to_end();
        untouched();
        fs::remove_dir(nas.marker()).unwrap();
    }
    // a folder where the log goes (made in File Station by mistake): nothing runs, and the loop
    // ends instead of waiting on it
    put("hjv-relay.log", None);
    fs::create_dir(nas.share().join("hjv-relay.log")).unwrap();
    nas.start(&hash);
    nas.wait_for_the_loop_to_end();

    // the status task: never through a link or into a hard-linked file, and it says what is at
    // the log's place
    let status = task_line(&nas, "status");
    assert!(status.ends_with("> /dev/null 2>&1"), "{status}");
    for target in [&victim, &nowhere] {
        put("status.txt", Some(target));
        assert_eq!(run_task(&status), 78);
        untouched();
    }
    put("status.txt", None);
    fs::hard_link(&victim, nas.share().join("status.txt")).unwrap();
    assert_eq!(run_task(&status), 78);
    untouched();
    put("status.txt", None);
    let report = || fs::read_to_string(nas.share().join("status.txt")).unwrap();
    // a longer earlier report is replaced, not written over
    fs::write(nas.share().join("status.txt"), "x".repeat(100_000)).unwrap();
    assert_eq!(run_task(&status), 0);
    assert!(report().starts_with("hjv-relay "), "{}", report());
    assert!(!report().contains(&"x".repeat(100)));
    let log_line = |r: &str| {
        r.lines()
            .find(|l| l.starts_with("로그 파일: "))
            .unwrap_or_default()
            .to_owned()
    };
    assert!(log_line(&report()).contains("(폴더라 "), "{}", report());
    put("hjv-relay.log", Some(&victim));
    assert_eq!(run_task(&status), 0);
    assert!(
        log_line(&report()).contains("(바로 가기(심볼릭 링크)라 "),
        "{}",
        report()
    );
    put("hjv-relay.log", None);
    fs::write(nas.share().join("hjv-relay.log"), "").unwrap();
    assert_eq!(run_task(&status), 0);
    assert!(
        log_line(&report()).contains("(일반 파일, 0바이트)"),
        "{}",
        report()
    );

    // the field test task, kept from sending anything here by an interface that does not exist
    let wake = task_line(&nas, "test-wake").replace("--count 3", "--count 3 --iface hjv-none0");
    for target in [&victim, &nowhere] {
        put("test.log", Some(target));
        assert_eq!(run_task(&wake), 78);
        untouched();
    }
    put("test.log", None);
    assert_eq!(run_task(&wake), 1);
    assert_eq!(run_task(&wake), 1);
    let record = fs::read_to_string(nas.share().join("test.log")).unwrap();
    assert_eq!(
        record.matches("보낼 랜카드가 없습니다").count(),
        2,
        "appended: {record}"
    );
    untouched();
}

#[test]
fn an_uploaded_link_is_not_run() {
    // the upload a link to a file that matches: not hashed, copied or run; the loop waits as for
    // a file that is not there yet. A stop marker that is a link counts, as it does for the relay
    let nas = Nas::new("upload-link");
    let hash = fake_relay(&nas, &[78], "");
    let real = nas.dir.join("elsewhere");
    fs::rename(nas.bin(), &real).unwrap();
    std::os::unix::fs::symlink(&real, nas.bin()).unwrap();
    nas.start(&hash);
    thread::sleep(Duration::from_millis(500));
    assert!(nas.calls().is_empty());
    assert!(!nas.state().join("hjv-relay").exists());
    assert_eq!(nas.processes().len(), 1, "the loop waits");
    std::os::unix::fs::symlink(nas.dir.join("nowhere"), nas.marker()).unwrap();
    nas.wait_for_the_loop_to_end();
    assert!(nas.calls().is_empty());
    assert!(
        nas.log()
            .contains("hjv-relay.stop이 있어 감시 루프를 끝냅니다."),
        "{}",
        nas.log()
    );
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
