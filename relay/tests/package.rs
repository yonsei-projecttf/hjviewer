// SPDX-License-Identifier: AGPL-3.0-only

//! scripts/package-relay.sh. Its refusals run with every `cargo test` (they stop before building).
//! The packages themselves take a static release build for x86_64-unknown-linux-musl (a minute
//! or two the first time; it needs that Rust target and musl-gcc) and run with
//! `cargo test --test package -- --ignored`: the archive, the unpacked folder and a SHA256SUMS of
//! that package alone in the out folder (build/release-relay by default), a statically linked
//! hjv-relay whose SHA-256 is the one SHA256SUMS gives for the boot-up task, and the field test
//! package without a server.

use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Mutex,
};

/// One build at a time: the script's cargo target folder is shared, and builds of different
/// variables replace each other's binary there.
static BUILDS: Mutex<()> = Mutex::new(());

/// The public test command key of PROTOCOL.md section 13.
const TEST_KEY: &str = "1:Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn script() -> PathBuf {
    root().join("scripts/package-relay.sh")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hjv-relay-package-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the script into `out` with only the given variables of its own.
fn package(out: &Path, vars: &[(&str, &str)]) -> Output {
    let _one = BUILDS.lock().unwrap_or_else(|e| e.into_inner());
    let mut cmd = Command::new("bash");
    cmd.arg(script()).arg(out);
    for var in [
        "HJV_SERVER",
        "HJV_COMMAND_PUBKEY",
        "HJV_RELAY_TEST_BUILD",
        "HJV_RELAY_FIELDTEST",
        "HJV_SKIP_SOURCE_CHECK",
        "HJV_BUILD_COMMIT",
        "CARGO_TARGET_DIR",
    ] {
        cmd.env_remove(var);
    }
    cmd.envs(vars.iter().copied());
    cmd.output().unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn the_default_out_folder_is_the_relays_own() {
    let text = fs::read_to_string(script()).unwrap();
    assert!(text.contains("out=${1:-$ROOT/build/release-relay}"));
}

#[test]
fn builds_that_must_not_reach_a_nas_are_refused_before_building() {
    let out = temp_dir("refused");
    for (vars, says) in [
        // no server or no keys: a relay that cannot run
        (
            &[("HJV_SKIP_SOURCE_CHECK", "1")][..],
            "set HJV_SERVER and HJV_COMMAND_PUBKEY",
        ),
        (
            &[
                ("HJV_SKIP_SOURCE_CHECK", "1"),
                ("HJV_SERVER", "rd.example.com"),
            ],
            "set HJV_SERVER and HJV_COMMAND_PUBKEY",
        ),
        // a placeholder or test server, unless it is a test build
        (
            &[
                ("HJV_SKIP_SOURCE_CHECK", "1"),
                ("HJV_SERVER", "rd.example.com"),
                ("HJV_COMMAND_PUBKEY", "1:AAAA"),
            ],
            "is a placeholder",
        ),
        (
            &[
                ("HJV_SKIP_SOURCE_CHECK", "1"),
                ("HJV_SERVER", "192.0.2.10"),
                ("HJV_COMMAND_PUBKEY", "1:AAAA"),
            ],
            "is a placeholder",
        ),
        // a public test key, whose private key anyone has
        (
            &[
                ("HJV_SKIP_SOURCE_CHECK", "1"),
                ("HJV_SERVER", "relay.no-such-tld-hjviewer"),
                ("HJV_COMMAND_PUBKEY", TEST_KEY),
            ],
            "public test key",
        ),
    ] {
        let result = package(&out, vars);
        assert_eq!(result.status.code(), Some(1), "{vars:?}: {}", text(&result));
        assert!(text(&result).contains(says), "{vars:?}: {}", text(&result));
    }
    // nothing was written
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    fs::remove_dir_all(&out).unwrap();
}

/// The lines of SHA256SUMS as (sum, file).
fn sums(out: &Path) -> Vec<(String, String)> {
    fs::read_to_string(out.join("SHA256SUMS"))
        .unwrap()
        .lines()
        .map(|l| {
            let (sum, file) = l.split_once("  ").unwrap();
            (sum.to_owned(), file.to_owned())
        })
        .collect()
}

#[test]
#[ignore = "a static release build (scripts/package-relay.sh); cargo test --test package -- --ignored"]
fn the_package_and_its_own_sha256sums() {
    let out = temp_dir("release");
    // what else may be in an out folder: another package and its sums
    fs::write(out.join("server-1.6.0.tar.gz"), "server").unwrap();
    fs::write(
        out.join("SHA256SUMS"),
        format!("{}  server-1.6.0.tar.gz\n", sha256_hex(b"server")),
    )
    .unwrap();
    let result = package(
        &out,
        &[
            ("HJV_SKIP_SOURCE_CHECK", "1"),
            ("HJV_RELAY_TEST_BUILD", "1"),
            ("HJV_SERVER", "rd.example.com"),
            ("HJV_COMMAND_PUBKEY", TEST_KEY),
        ],
    );
    let said = text(&result);
    eprintln!("{said}");
    assert!(result.status.success(), "{said}");
    let name = format!(
        "hjviewer-relay-{}-linux-x86_64-test",
        env!("CARGO_PKG_VERSION")
    );
    let archive = format!("{name}.tar.gz");
    let bin = out.join(&name).join("hjv-relay");

    // SHA256SUMS: this package alone, the archive and the hjv-relay it holds
    let listed = sums(&out);
    let files: Vec<&str> = listed.iter().map(|(_, f)| f.as_str()).collect();
    assert_eq!(files, [archive.clone(), format!("{name}/hjv-relay")]);
    assert_eq!(
        listed[0].0,
        sha256_hex(&fs::read(out.join(&archive)).unwrap())
    );
    let bin_sum = sha256_hex(&fs::read(&bin).unwrap());
    assert_eq!(listed[1].0, bin_sum);
    let check = Command::new("sha256sum")
        .args(["-c", "--strict", "SHA256SUMS"])
        .current_dir(&out)
        .output()
        .unwrap();
    assert!(check.status.success(), "{}", text(&check));
    // the value for the boot-up task, said at the end
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.lines().last().unwrap(),
        format!(
            "the SHA-256 of hjv-relay, for the first line of the NAS's boot-up task: {bin_sum}"
        ),
        "{said}"
    );
    // the unpacked folder is the archive's content
    let list = Command::new("tar")
        .args(["-tzf", &archive])
        .current_dir(&out)
        .output()
        .unwrap();
    let mut entries: Vec<String> = String::from_utf8(list.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim_end_matches('/').to_owned())
        .collect();
    entries.sort();
    let mut expected: Vec<String> = [
        "",
        "/LICENSE.txt",
        "/NOTICE.txt",
        "/README-DSM.md",
        "/SOURCE.txt",
        "/THIRD_PARTY_NOTICES.txt",
        "/hjv-relay",
    ]
    .iter()
    .map(|f| format!("{name}{f}"))
    .collect();
    expected.sort();
    assert_eq!(entries, expected);
    assert_eq!(
        fs::read_to_string(out.join(&name).join("README-DSM.md")).unwrap(),
        fs::read_to_string(root().join("docs/relay-dsm.md")).unwrap()
    );
    let source = fs::read_to_string(out.join(&name).join("SOURCE.txt")).unwrap();
    assert!(source.starts_with("UNVERIFIED TEST BUILD"), "{source}");
    assert!(source.contains("Compilers: rustc "), "{source}");

    // statically linked, configured as asked, and it runs with nothing from the system
    let kind = Command::new("file").arg("-b").arg(&bin).output().unwrap();
    let kind = String::from_utf8_lossy(&kind.stdout);
    assert!(
        kind.contains("statically linked") || kind.contains("static-pie linked"),
        "{kind}"
    );
    let version = Command::new(&bin)
        .arg("version")
        .env_clear()
        .output()
        .unwrap();
    let version = String::from_utf8_lossy(&version.stdout).to_string();
    assert!(version.contains("server: rd.example.com"), "{version}");
    assert!(
        version.contains(&format!("command keys: {TEST_KEY}")),
        "{version}"
    );
    assert!(
        version.contains(
            "command key fingerprints: 1:24f6ed6acbfe1009c030d7ca567c33ca4830911498236b5561a6c82abec5de28"
        ),
        "{version}"
    );
    // the other package's files are left alone, and out of these sums
    assert_eq!(
        fs::read_to_string(out.join("server-1.6.0.tar.gz")).unwrap(),
        "server"
    );
    fs::remove_dir_all(&out).unwrap();
}

#[test]
#[ignore = "a static release build (scripts/package-relay.sh); cargo test --test package -- --ignored"]
fn the_field_test_package_has_no_server() {
    let out = temp_dir("fieldtest");
    let result = package(
        &out,
        &[("HJV_SKIP_SOURCE_CHECK", "1"), ("HJV_RELAY_FIELDTEST", "1")],
    );
    let said = text(&result);
    assert!(result.status.success(), "{said}");
    let name = format!(
        "hjviewer-relay-{}-linux-x86_64-fieldtest-test",
        env!("CARGO_PKG_VERSION")
    );
    let files: Vec<String> = sums(&out).into_iter().map(|(_, f)| f).collect();
    assert_eq!(
        files,
        [format!("{name}.tar.gz"), format!("{name}/hjv-relay")]
    );
    let bin = out.join(&name).join("hjv-relay");
    let version = Command::new(&bin).arg("version").output().unwrap();
    let version = String::from_utf8_lossy(&version.stdout).to_string();
    assert!(version.contains("server: none"), "{version}");
    assert!(version.contains("command keys: none"), "{version}");
    let source = fs::read_to_string(out.join(&name).join("SOURCE.txt")).unwrap();
    assert!(source.contains("FIELD TEST BUILD"), "{source}");
    // it refuses run, before it touches anything
    let state = out.join("state");
    let run = Command::new(&bin)
        .args(["run", "--state", state.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(78));
    assert!(text(&run).contains("현장 시험용 묶음이면 정상입니다"));
    assert!(!state.exists());
    fs::remove_dir_all(&out).unwrap();
}
