// SPDX-License-Identifier: AGPL-3.0-only

//! The DSM boot-up script exactly as docs/relay-dsm.md prints it, run by this machine's POSIX `sh`
//! against a stand-in for hjv-relay (paths moved to a temporary folder, the 30-second pause
//! shortened): it starts the relay with `run --state`, starts it again after it exits, stops for
//! good on 2 (command line), 75 (already running) and 78 (configuration), waits while the file is
//! not executable and says so once, waits for a volume that is not there yet, appends the
//! relay's output to the log and rotates a log over 1 MB once.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn boot_script() -> String {
    let guide = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/relay-dsm.md");
    let text = fs::read_to_string(guide).unwrap();
    let start = text.find("```sh\n").expect("no sh block in the guide") + 6;
    let end = start + text[start..].find("```").unwrap();
    text[start..end].to_owned()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hjv-relay-watchdog-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A stand-in that records its arguments and exits with the next code of `codes`.
fn fake_relay(dir: &Path, codes: &[u8]) {
    fs::create_dir_all(dir.join("share")).unwrap();
    fs::write(
        dir.join("codes"),
        codes.iter().map(|c| format!("{c}\n")).collect::<String>(),
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{d}/calls'\necho \"stand-in started: $*\"\n\
         code=$(head -n 1 '{d}/codes'); sed -i 1d '{d}/codes'\nexit \"${{code:-0}}\"\n",
        d = dir.display()
    );
    let bin = dir.join("share/hjv-relay");
    fs::write(&bin, script).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
}

fn calls(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn log(dir: &Path) -> String {
    fs::read_to_string(dir.join("share/hjv-relay.log")).unwrap_or_default()
}

/// Runs the boot-up script the way the task does; it must end at once, successfully.
fn start(dir: &Path) {
    let script = boot_script()
        .replace(
            "/volume1/hjv-relay",
            &dir.join("share").display().to_string(),
        )
        .replace(
            "/volume1/@hjv-relay",
            &dir.join("state").display().to_string(),
        )
        .replace("sleep 30", "sleep 0.2");
    let started = Instant::now();
    let out = Command::new("sh").arg("-c").arg(script).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_boot_script_of_the_guide() {
    let script = boot_script();
    assert!(script.contains("run --state"), "{script}");
    let dir = temp_dir("main");

    // exits 0 (a new executable) and 1 (an error) are restarted; 75 ends the loop
    let old_log = "x".repeat(1_048_577);
    fs::create_dir_all(dir.join("share")).unwrap();
    fs::write(dir.join("share/hjv-relay.log"), &old_log).unwrap();
    fake_relay(&dir, &[0, 1, 75]);
    start(&dir);
    wait_for("three runs", || calls(&dir).len() >= 3);
    thread::sleep(Duration::from_millis(800));
    let state = dir.join("state").display().to_string();
    assert_eq!(calls(&dir), vec![format!("run --state {state}"); 3]);
    // the big log was rotated, and the new one has the relay's output
    assert_eq!(
        fs::read_to_string(dir.join("share/hjv-relay.log.1")).unwrap(),
        old_log
    );
    assert_eq!(
        log(&dir).matches("stand-in started: run --state").count(),
        3,
        "{}",
        log(&dir)
    );

    // 78 (configuration) and 2 (command line) also end the loop; a small log is not rotated
    for code in [78, 2] {
        fs::remove_file(dir.join("calls")).unwrap();
        fake_relay(&dir, &[code]);
        start(&dir);
        wait_for("one run", || calls(&dir).len() == 1);
        thread::sleep(Duration::from_millis(800));
        assert_eq!(calls(&dir).len(), 1, "exit {code}");
    }
    assert_eq!(
        fs::read_to_string(dir.join("share/hjv-relay.log.1")).unwrap(),
        old_log
    );

    // a file that is not executable (an upload in progress, or one that lost the bit) is waited
    // for, and the log says so once
    fs::remove_file(dir.join("calls")).unwrap();
    fake_relay(&dir, &[75]);
    let bin = dir.join("share/hjv-relay");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o644)).unwrap();
    let before = log(&dir);
    start(&dir);
    thread::sleep(Duration::from_millis(1000));
    assert!(calls(&dir).is_empty());
    let said = log(&dir)[before.len()..].to_owned();
    assert_eq!(
        said.matches("실행 권한이 없어 기다립니다").count(),
        1,
        "{said}"
    );
    assert_eq!(said.lines().count(), 1, "{said}");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    wait_for("the run after chmod", || calls(&dir).len() == 1);
    thread::sleep(Duration::from_millis(600));
    assert_eq!(calls(&dir).len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_volume_that_appears_after_the_boot_up_task() {
    // the task runs before the volume (an encrypted one waiting to be unlocked) or the shared
    // folder is there: the loop must still be waiting when they appear
    let dir = temp_dir("late");
    start(&dir);
    thread::sleep(Duration::from_millis(500));
    assert!(!dir.join("share").exists());
    fake_relay(&dir, &[75]);
    wait_for("the run once the folder is there", || {
        calls(&dir).len() == 1
    });
    let state = dir.join("state").display().to_string();
    assert_eq!(calls(&dir), [format!("run --state {state}")]);
    wait_for("the relay's output in the log", || {
        log(&dir).contains("stand-in started: run --state")
    });
    thread::sleep(Duration::from_millis(600));
    assert_eq!(calls(&dir).len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}
