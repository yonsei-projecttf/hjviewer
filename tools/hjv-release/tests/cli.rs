// SPDX-License-Identifier: AGPL-3.0-only

//! The commands, run as the operator, an agent and the person at the terminal run them: the
//! binary itself, a pseudo terminal where a passphrase is typed, recorded `gh` answers
//! (tests/gh/) replayed by a script on the PATH, and a temporary git repository with the release
//! tags. Test keys only (src/keys.rs).

mod common;

use common::{mode, run, run_tty, run_with, shows_bytes, temp_dir, Output, CTRL_C};
use hjv_release::{
    draft::Draft,
    envelope::Envelope,
    hbbs::Door2,
    keyfile::SecretKey,
    keys::{test_key, KeySet, Role},
    policy::{Platform, Policy},
    time,
    trust::{check_envelope, hex, sha256},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

const SERVER: &str = "rd.example.com";
/// The passphrase of the key files made here. Not a secret: the keys are the test keys.
const PASS: &str = "test passphrase of the cli tests";

fn vectors() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The CI build number of a time: `date -u +%y%j%H%M`.
fn build_number(at: i64) -> String {
    let days = at.div_euclid(time::DAY);
    let (year, _, _) = time::civil_from_days(days);
    let day_of_year = days - time::days_from_civil(year, 1, 1) + 1;
    let seconds = at.rem_euclid(time::DAY);
    format!(
        "{:02}{day_of_year:03}{:02}{:02}",
        year % 100,
        seconds / 3600,
        seconds % 3600 / 60
    )
}

/// A dev machine for one test: the key file, encrypted key files, a clone with release tags, and
/// a `gh` that replays recorded answers.
struct World {
    dir: PathBuf,
    source_sha: String,
    path: String,
    /// The iOS build number of the releases made here: yesterday's.
    ios_build: String,
}

impl World {
    fn new(name: &str) -> World {
        let dir = temp_dir(name);
        fs::copy(vectors().join("update-keys.env"), dir.join("keys.env")).unwrap();
        for name in ["signing-1", "signing-2", "root-101"] {
            let key = test_key(name).unwrap();
            let file = SecretKey::from_seed(key.role, key.id, &key.seed())
                .encrypt(PASS, 6)
                .unwrap();
            fs::write(dir.join(format!("{name}.key")), file).unwrap();
        }
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "the reviewed commit"],
        );
        for version in ["1.6.0", "1.6.1"] {
            let tag = format!("client-{version}");
            git(
                &repo,
                &["tag", "-a", &tag, "-m", &format!("HJViewer {version}")],
            );
        }
        let source_sha = git(&repo, &["rev-parse", "HEAD"]);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("gh")).unwrap();
        let gh = dir.join("bin/gh");
        fs::write(
            &gh,
            "#!/bin/sh\n\
             # replays recorded answers (tests/gh) and notes what was asked\n\
             echo \"$*\" >> \"$HJV_TEST_GH/calls.txt\"\n\
             case \"$*\" in\n\
             \x20 \"api repos/yonsei-projecttf/hjviewer-private/actions/runs/4242\")\n\
             \x20   exec cat \"$HJV_TEST_GH/run.json\" ;;\n\
             \x20 \"api -H Accept: application/vnd.github.raw+json repos/yonsei-projecttf/hjviewer-private/contents/.github/workflows/build.yml?ref=2222222222222222222222222222222222222222\")\n\
             \x20   exec cat \"$HJV_TEST_GH/build.yml\" ;;\n\
             esac\n\
             echo \"gh: Not Found (HTTP 404)\" >&2\n\
             exit 1\n",
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let world = World {
            path: format!(
                "{}:{}",
                dir.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
            dir,
            source_sha,
            ios_build: build_number(time::now() - time::DAY),
        };
        world.gh("run.json", "build.yml");
        world
    }

    /// Chooses the recorded answers `gh` gives, with this world's source commit put in.
    fn gh(&self, run: &str, build: &str) {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gh");
        for (from, to) in [(run, "run.json"), (build, "build.yml")] {
            let text = fs::read_to_string(fixtures.join(from)).unwrap();
            fs::write(
                self.dir.join("gh").join(to),
                text.replace(&"1".repeat(40), &self.source_sha),
            )
            .unwrap();
        }
    }

    fn file(&self, name: &str) -> String {
        self.dir.join(name).to_str().unwrap().to_owned()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(name)).unwrap()
    }

    fn env(&self) -> Vec<(&str, String)> {
        vec![
            ("PATH", self.path.clone()),
            ("HJV_TEST_GH", self.file("gh")),
        ]
    }

    /// Runs the tool as a script or an agent would: no terminal anywhere.
    fn tool(&self, args: &[&str]) -> Output {
        let env = self.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        run_with(args, &self.dir, &env, "")
    }

    /// Runs the tool as the person at the terminal does.
    fn person(&self, args: &[&str], script: &[(&str, &str)]) -> Output {
        let env = self.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        run_tty(args, &self.dir, &env, script)
    }

    /// The artifacts of a build as downloaded from its run, with release.json as CI writes it.
    fn release(&self, version: &str) -> String {
        let dir = self.dir.join(format!("release-{version}"));
        let android_code: u64 = self.ios_build.parse::<u64>().unwrap() + 2000;
        for (platform, file, build) in [
            ("windows", "HJViewer.exe".to_owned(), format!("{version}.7")),
            (
                "android",
                format!("HJViewer-{version}-1111111-android-aarch64-signed.apk"),
                android_code.to_string(),
            ),
            (
                "ios",
                format!("HJViewer-{version}-1111111-ios.ipa"),
                self.ios_build.clone(),
            ),
        ] {
            let artifact = dir.join(format!("HJViewer-{version}-1111111-{platform}"));
            fs::create_dir_all(&artifact).unwrap();
            let content = format!("{platform} package {version}").into_bytes();
            fs::write(artifact.join(&file), &content).unwrap();
            fs::write(
                artifact.join("release.json"),
                format!(
                    "{{\"platform\":\"{platform}\",\"version\":\"{version}\",\"build\":\"{build}\",\"file\":\"{file}\",\
                     \"size\":{},\"sha256\":\"{}\",\"source_sha\":\"{}\",\"run_id\":4242,\
                     \"workflow\":\".github/workflows/build.yml\"}}\n",
                    content.len(),
                    hex(&sha256(&content)),
                    self.source_sha
                ),
            )
            .unwrap();
            for notice in ["LICENSE.txt", "NOTICE.txt", "SOURCE.txt"] {
                fs::write(
                    artifact.join(notice),
                    format!("{notice} of HJViewer {version}\n"),
                )
                .unwrap();
            }
            fs::write(
                artifact.join("THIRD_PARTY_NOTICES.txt"),
                format!("notices of the {platform} build\n"),
            )
            .unwrap();
        }
        dir.to_str().unwrap().to_owned()
    }

    /// `hjv-release policy` with the options every call here needs.
    fn policy(&self, more: &[&str]) -> Output {
        let (keys, repo) = (self.file("keys.env"), self.file("repo"));
        let mut args = vec![
            "policy",
            "--keys",
            &keys,
            "--allow-test-keys",
            "--repo",
            &repo,
        ];
        args.extend_from_slice(more);
        self.tool(&args)
    }

    /// The first policy's draft: 1.6.0 everywhere, nothing enforced.
    fn first_draft(&self, out: &str) -> Output {
        let release = self.release("1.6.0");
        self.policy(&[
            "--prev",
            "none",
            "--server",
            SERVER,
            "--release-dir",
            &release,
            "--min",
            "all=1.5.0",
            "--grace-days",
            "0",
            "--ios-valid",
            &self.ios_build,
            "--os-min",
            "windows=10",
            "--os-min",
            "android=22",
            "--os-min",
            "ios=13.0",
            "--contact",
            "설치 파일은 관리자에게 받으세요.",
            "--out",
            out,
        ])
    }

    /// Signs as the person does: the passphrase typed at the prompt.
    fn sign(&self, key: &str, draft: &str, out: &str) -> Output {
        let (keys, key) = (self.file("keys.env"), self.file(key));
        self.person(
            &[
                "sign",
                "--key",
                &key,
                "--keys",
                &keys,
                "--allow-test-keys",
                "--out",
                out,
                draft,
            ],
            &[("의 비밀번호: ", PASS)],
        )
    }

    fn checked(&self, name: &str) -> Policy {
        let keys = KeySet::parse(&self.read("keys.env")).unwrap();
        check_envelope(&self.read(name), &keys, SERVER)
            .unwrap()
            .policy
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Nothing a run showed holds a private key of the world or the passphrase, in any form a
/// careless print could give them.
fn assert_no_secret(output: &Output, names: &[&str]) {
    let all = output.all();
    assert!(
        !all.contains(PASS),
        "the passphrase is in the output:\n{all}"
    );
    for name in names {
        let seed = test_key(name).unwrap().seed();
        assert!(
            !shows_bytes(&all, &seed),
            "the private key {name} is in the output:\n{all}"
        );
    }
}

#[test]
fn the_leak_check_knows_the_forms_of_a_leak() {
    let seed = test_key("signing-2").unwrap().seed();
    let hex_plain = hex(&seed);
    let leaks = [
        format!("key: {hex_plain}"),
        format!("key: {}", hex_plain.to_uppercase()),
        format!("{seed:02x?}"),
        format!("{seed:#04x?}"),
        format!("{seed:?}"),
        seed.iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":"),
        {
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
            URL_SAFE_NO_PAD.encode(seed)
        },
        // the seed of signing-1 is printable as it is
        String::from_utf8(test_key("signing-1").unwrap().seed().to_vec()).unwrap(),
    ];
    for (i, leak) in leaks.iter().enumerate() {
        let seed = if i == leaks.len() - 1 {
            test_key("signing-1").unwrap().seed()
        } else {
            seed
        };
        assert!(shows_bytes(&format!("error: {leak}\n"), &seed), "{leak}");
    }
    // the public key, another key and ordinary output are no leak
    let public = test_key("signing-2").unwrap().public().to_string();
    assert!(!shows_bytes(&public, &seed));
    assert!(!shows_bytes(
        &hex(&test_key("root-101").unwrap().seed()),
        &seed
    ));
    assert!(!shows_bytes(
        "서명했습니다: policy.json\nsha256 0123456789abcdef",
        &seed
    ));
}

#[test]
fn usage() {
    let dir = temp_dir("usage");
    let none = run(&[], &dir);
    assert_eq!(none.code, 2);
    assert!(none.stderr.contains("hjv-release") && none.stderr.contains("keygen"));
    let help = run(&["help"], &dir);
    assert_eq!(help.code, 0);
    for command in [
        "keygen",
        "policy",
        "sign",
        "verify",
        "publish-dir",
        "hbbs-conf",
        "floor",
        "check",
        "test-keys",
    ] {
        assert!(help.stdout.contains(command), "{command}");
    }
    let unknown = run(&["signn"], &dir);
    assert_eq!(unknown.code, 2);
    assert!(unknown.stderr.contains("모르는 명령"));
    let bad_option = run(&["verify", "--serverr", "x", "file"], &dir);
    assert_eq!(bad_option.code, 1);
    assert!(bad_option.stderr.contains("모르는 옵션: --serverr"));
    assert!(run(&["version"], &dir).stdout.starts_with("hjv-release "));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keygen_needs_a_terminal() {
    let dir = temp_dir("keygen-no-tty");
    // a script that pipes the passphrase in twice gets nowhere
    let piped = format!("{PASS}\n{PASS}\n");
    let output = run_with(
        &[
            "keygen", "--role", "signing", "--id", "1", "--out", "new.key",
        ],
        &dir,
        &[],
        &piped,
    );
    assert_eq!(output.code, 1);
    assert!(
        output
            .stderr
            .contains("터미널(TTY)이 없어 키를 만들지 않습니다"),
        "{}",
        output.stderr
    );
    assert!(output.stdout.is_empty());
    assert!(!dir.join("new.key").exists());
    // arguments are checked first
    for (args, expected) in [
        (
            vec!["keygen", "--role", "admin", "--id", "1", "--out", "k"],
            "--role",
        ),
        (
            vec!["keygen", "--role", "root", "--id", "0", "--out", "k"],
            "키 번호",
        ),
        (vec!["keygen", "--role", "root", "--id", "101"], "--out"),
        (
            vec![
                "keygen",
                "--role",
                "root",
                "--id",
                "101",
                "--out",
                "k",
                "--passphrase",
                "x",
            ],
            "모르는 옵션",
        ),
    ] {
        let output = run(&args, &dir);
        assert_eq!(output.code, 1, "{args:?}");
        assert!(
            output.stderr.contains(expected),
            "{args:?}: {}",
            output.stderr
        );
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn keygen_writes_an_encrypted_key() {
    let dir = temp_dir("keygen");
    let args = [
        "keygen", "--role", "root", "--id", "101", "--out", "root.key",
    ];
    let output = run_tty(
        &args,
        &dir,
        &[],
        &[("비밀번호: ", PASS), ("비밀번호 다시: ", PASS)],
    );
    assert_eq!(output.code, 0, "{}", output.all());
    // standard output is the public line and nothing else
    let line = output.stdout.strip_suffix('\n').unwrap();
    assert!(!line.contains('\n'), "{line:?}");
    let (id, public) = line.split_once(':').unwrap();
    assert_eq!(id, "101");
    assert_eq!(public.len(), 44);
    assert!(output.tty.contains("새 루트 키(번호 101)를 만듭니다"));
    assert!(output.tty.contains("HJV_UPDATE_ROOT_PUBKEY"));

    // the file: for its owner only, an age file, and the key opens with the passphrase
    let path = dir.join("root.key");
    assert_eq!(mode(&path), 0o600);
    let file = fs::read(&path).unwrap();
    assert!(file.starts_with(b"age-encryption.org/v1\n-> scrypt "));
    let key = SecretKey::decrypt(&file, PASS).unwrap();
    assert_eq!((key.role, key.id), (Role::Root, 101));
    assert_eq!(key.public().to_string(), line);
    assert!(SecretKey::decrypt(&file, "another passphrase").is_err());
    assert!(!hjv_release::keys::is_test_key(&key.verifying_key()));

    // neither the key nor the passphrase is anywhere: not on any output, not in the file
    use base64::{engine::general_purpose::STANDARD, Engine};
    let seed = key.signing_key().to_bytes();
    let (seed_b64, seed_hex) = (STANDARD.encode(seed), hex(&seed));
    let all = output.all();
    assert!(
        !all.contains(PASS),
        "the passphrase is in the output:\n{all}"
    );
    assert!(
        !shows_bytes(&all, &seed),
        "the private key is in the output:\n{all}"
    );
    for needle in [
        &seed[..],
        seed_b64.as_bytes(),
        seed_hex.as_bytes(),
        PASS.as_bytes(),
        b"seed=",
        b"hjv-update-key",
    ] {
        assert!(!file.windows(needle.len()).any(|w| w == needle));
    }

    // an existing file is never overwritten, with or without a terminal
    let again = run_tty(
        &args,
        &dir,
        &[],
        &[("비밀번호: ", PASS), ("비밀번호 다시: ", PASS)],
    );
    assert_eq!(again.code, 1);
    assert!(again.stderr.contains("이미 있습니다"));
    assert_eq!(fs::read(&path).unwrap(), file);

    // two different passphrases, or a short one: no key
    let args = [
        "keygen",
        "--role",
        "signing",
        "--id",
        "1",
        "--out",
        "signing.key",
    ];
    let differ = run_tty(
        &args,
        &dir,
        &[],
        &[
            ("비밀번호: ", PASS),
            ("비밀번호 다시: ", "another passphrase"),
        ],
    );
    assert_eq!(differ.code, 1);
    assert!(differ.stderr.contains("두 비밀번호가 다릅니다"));
    let short = run_tty(&args, &dir, &[], &[("비밀번호: ", "short")]);
    assert_eq!(short.code, 1);
    assert!(short.stderr.contains("12자 이상"));
    assert!(!dir.join("signing.key").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn sign_needs_a_terminal() {
    let world = World::new("sign-no-tty");
    assert_eq!(world.first_draft("draft.json").code, 0);
    let (keys, key) = (world.file("keys.env"), world.file("signing-1.key"));
    let args = [
        "sign",
        "--key",
        &key,
        "--keys",
        &keys,
        "--allow-test-keys",
        "--out",
        "policy.json",
        "draft.json",
    ];
    // what an agent's shell can try: the passphrase on standard input, in the environment
    let piped = format!("{PASS}\nyes\n{PASS}\n");
    let env = [
        ("HJV_PASSPHRASE", PASS),
        ("PASSPHRASE", PASS),
        ("SSH_ASKPASS", "/bin/true"),
    ];
    for output in [world.tool(&args), run_with(&args, &world.dir, &env, &piped)] {
        assert_eq!(output.code, 1);
        assert!(
            output
                .stderr
                .contains("터미널(TTY)이 없어 서명하지 않습니다"),
            "{}",
            output.stderr
        );
        assert!(output.stderr.contains("터미널에서만"));
        assert!(output.stdout.is_empty());
        assert_no_secret(&output, &["signing-1"]);
    }
    assert!(!world.dir.join("policy.json").exists());
    // there is no option that takes a passphrase
    for option in ["--passphrase", "--password", "--passphrase-file", "--stdin"] {
        let mut with = args.to_vec();
        with.extend_from_slice(&[option, PASS]);
        let output = world.tool(&with);
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("모르는 옵션"),
            "{option}: {}",
            output.stderr
        );
    }
    // the same command with a person at a terminal signs
    let output = world.sign("signing-1.key", "draft.json", "policy.json");
    assert_eq!(output.code, 0, "{}", output.all());
    assert!(world.dir.join("policy.json").exists());
}

#[test]
fn a_release_from_the_build_to_the_server() {
    let world = World::new("release");
    // seq 1: the agent makes the draft; provenance is checked with gh and the tag
    let draft = world.first_draft("draft1.json");
    assert_eq!(draft.code, 0, "{}", draft.all());
    assert!(draft.stdout.contains("첫 정책"));
    assert!(draft
        .stdout
        .contains("확인함: windows 1.6.0: 실행 4242 성공"));
    assert!(draft
        .stdout
        .contains("확인함: android 1.6.0: 실행 4242 성공"));
    assert!(draft
        .stdout
        .contains("서명은 사용자가 터미널에서 직접 합니다"));
    // the command line for the person: the same key file, and the same word about test keys
    assert!(draft.stdout.contains(&format!(
        "hjv-release sign --key <서명 키 파일> --keys {} --allow-test-keys --out version-policy.json draft1.json",
        world.file("keys.env")
    )));
    let calls = world.read("gh/calls.txt");
    assert_eq!(
        calls
            .lines()
            .filter(|l| l.contains("actions/runs/4242"))
            .count(),
        2
    );
    assert_eq!(
        calls
            .lines()
            .filter(|l| l.contains("contents/.github/workflows/build.yml?ref="))
            .count(),
        2
    );
    let parsed = Draft::parse(&world.read("draft1.json")).unwrap();
    assert_eq!(
        (parsed.signer, &parsed.root, &parsed.prev),
        (Role::Signing, &None, &None)
    );
    // a draft is not overwritten
    assert!(world
        .first_draft("draft1.json")
        .stderr
        .contains("이미 있습니다"));

    // the person signs: the summary on the terminal, the passphrase unseen
    let signed = world.sign("signing-1.key", "draft1.json", "policy1.json");
    assert_eq!(signed.code, 0, "{}", signed.all());
    assert!(signed.tty.contains("서명할 정책:"));
    assert!(signed.tty.contains("첫 정책"));
    assert!(signed.tty.contains("windows: 최소 1.5.0 · 최신 1.6.0"));
    assert!(signed.tty.contains("서명 키(번호 1)를 열었습니다"));
    assert!(
        !signed.tty.contains("yes를 입력"),
        "nothing dangerous: no question"
    );
    assert_no_secret(&signed, &["signing-1"]);
    let file = world.read("policy1.json");
    assert!(signed
        .stdout
        .contains(&format!("sha256 {}", hex(&sha256(file.as_bytes())))));
    assert!(signed.stdout.contains(&format!(
        "확인: hjv-release verify --keys {} --allow-test-keys --server {SERVER} policy1.json",
        world.file("keys.env")
    )));
    assert!(file.ends_with("\n") && !file.trim_end().contains('\n'));
    assert_eq!(mode(&world.dir.join("policy1.json")), 0o644);
    let policy = world.checked("policy1.json");
    assert_eq!((policy.epoch, policy.seq), (1, 1));
    assert_eq!(
        policy.platforms[&Platform::Windows]
            .package
            .as_ref()
            .unwrap()
            .file,
        "HJViewer-1.6.0-windows-x86_64.exe"
    );
    assert_eq!(
        policy.platforms[&Platform::Ios].testflight_build.as_deref(),
        Some(world.ios_build.as_str())
    );
    // a signed policy is not overwritten either
    let again = world.sign("signing-1.key", "draft1.json", "policy1.json");
    assert_eq!(again.code, 1);
    assert!(again.stderr.contains("이미 있습니다"));

    // verify: anyone, without a key
    let keys = world.file("keys.env");
    let verify = |more: &[&str]| {
        let mut args = vec!["verify", "--keys", &keys, "--allow-test-keys"];
        args.extend_from_slice(more);
        world.tool(&args)
    };
    let ok = verify(&["--server", SERVER, "policy1.json"]);
    assert_eq!(ok.code, 0, "{}", ok.all());
    assert!(ok.stdout.contains("검증됨") && ok.stdout.contains("서명: 서명 키 1"));
    assert!(ok
        .stdout
        .contains(&format!("sha256 {}", hex(&sha256(file.as_bytes())))));
    assert!(ok.stderr.contains("시험 키"));
    let other_server = verify(&["--server", "hjv.test", "policy1.json"]);
    assert_eq!(other_server.code, 1);
    assert!(other_server.stderr.contains("서버"));
    fs::write(
        world.dir.join("changed.json"),
        file.replacen("1.5.0", "1.4.0", 1),
    )
    .unwrap();
    let changed = verify(&["--server", SERVER, "changed.json"]);
    assert_eq!(changed.code, 1);
    assert!(changed.stderr.contains("서명이 맞지 않습니다"));
    // test keys are refused unless the command line says it is a test
    let gated = world.tool(&[
        "verify",
        "--keys",
        &keys,
        "--server",
        SERVER,
        "policy1.json",
    ]);
    assert_eq!(gated.code, 1);
    assert!(gated.stderr.contains("시험 키") && gated.stderr.contains("--allow-test-keys"));
    assert_eq!(verify(&["policy1.json"]).code, 1, "--server is required");

    // the publish directory: first from the draft, then with the signed policy
    let release = world.release("1.6.0");
    let publish = |policy: &str| {
        world.tool(&[
            "publish-dir",
            "--policy",
            policy,
            "--release-dir",
            &release,
            "--out",
            "publish",
            "--keys",
            &keys,
            "--allow-test-keys",
        ])
    };
    let from_draft = publish("draft1.json");
    assert_eq!(from_draft.code, 0, "{}", from_draft.all());
    assert!(from_draft.stdout.contains("아직 없습니다"));
    let with_policy = publish("policy1.json");
    assert_eq!(with_policy.code, 0, "{}", with_policy.all());
    assert_eq!(world.read("publish/version-policy.json"), file);
    assert_eq!(
        world.read("publish/pkg/HJViewer-1.6.0-windows-x86_64.exe"),
        "windows package 1.6.0"
    );
    assert_eq!(
        world.read("publish/pkg/HJViewer-1.6.0-android-arm64.apk"),
        "android package 1.6.0"
    );
    assert!(world
        .read("publish/pkg/notices-1.6.0/THIRD_PARTY_NOTICES.txt")
        .contains("notices of the windows build"));
    let sums = world.read("publish/SHA256SUMS");
    assert_eq!(sums.lines().count(), 7);
    assert!(sums.contains(&format!(
        "{}  version-policy.json",
        hex(&sha256(file.as_bytes()))
    )));
    // sha256sum -c agrees
    let check = Command::new("sha256sum")
        .args(["-c", "--quiet", "SHA256SUMS"])
        .current_dir(world.dir.join("publish"))
        .status()
        .unwrap();
    assert!(check.success());

    // seq 2: every min to 1.6.0 with 14 days of grace
    let enforce = world.policy(&[
        "--prev",
        "policy1.json",
        "--min",
        "all=1.6.0",
        "--grace-days",
        "14",
        "--ios-valid",
        &world.ios_build,
        "--out",
        "draft2.json",
    ]);
    assert_eq!(enforce.code, 0, "{}", enforce.all());
    assert!(enforce.stdout.contains("windows: 최소 1.5.0 → 1.6.0"));
    let signed = world.sign("signing-2.key", "draft2.json", "policy2.json");
    assert_eq!(signed.code, 0, "{}", signed.all());
    let policy2 = world.checked("policy2.json");
    assert_eq!((policy2.epoch, policy2.seq), (1, 2));
    let deadline = policy2.platforms[&Platform::Windows].grace_until.unwrap();
    assert_eq!((deadline + time::KST) % time::DAY, 0, "midnight in Korea");
    let days = (deadline - time::now()) as f64 / time::DAY as f64;
    assert!((13.0..=14.0).contains(&days), "{days} days");
    let ok = verify(&["--server", SERVER, "--prev", "policy1.json", "policy2.json"]);
    assert_eq!(ok.code, 0, "{}", ok.all());
    assert!(ok
        .stdout
        .contains("이전 정책을 가진 앱이 이 정책을 받습니다"));
    // the other way round it is a rollback
    let rollback = verify(&["--server", SERVER, "--prev", "policy2.json", "policy1.json"]);
    assert_eq!(rollback.code, 1);

    // the floor line for the next build
    let floor = world.tool(&[
        "floor",
        "--policy",
        "policy2.json",
        "--keys",
        &keys,
        "--allow-test-keys",
    ]);
    assert_eq!(
        (floor.code, floor.stdout.as_str()),
        (0, "HJV_POLICY_FLOOR=1:2\n")
    );
    assert!(floor.stderr.contains("하한은 1:0"));
}

#[test]
fn relaxing_asks_for_yes_and_the_root_key() {
    let world = World::new("relax");
    assert_eq!(world.first_draft("draft1.json").code, 0);
    assert_eq!(
        world
            .sign("signing-1.key", "draft1.json", "policy1.json")
            .code,
        0
    );
    assert_eq!(
        world
            .policy(&[
                "--prev",
                "policy1.json",
                "--min",
                "all=1.6.0",
                "--grace-days",
                "14",
                "--ios-valid",
                &world.ios_build,
                "--out",
                "draft2.json"
            ])
            .code,
        0
    );
    assert_eq!(
        world
            .sign("signing-1.key", "draft2.json", "policy2.json")
            .code,
        0
    );

    // lowering a min: the tool refuses without --relax, and with it the draft is the root key's
    let refused = world.policy(&[
        "--prev",
        "policy2.json",
        "--min",
        "ios=1.5.0",
        "--out",
        "draft3.json",
    ]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("완화가 들어 있습니다")
            && refused.stderr.contains("ios: min을 내림")
    );
    assert!(!world.dir.join("draft3.json").exists());
    let relaxed = world.policy(&[
        "--prev",
        "policy2.json",
        "--min",
        "ios=1.5.0",
        "--relax",
        "--out",
        "draft3.json",
    ]);
    assert_eq!(relaxed.code, 0, "{}", relaxed.all());
    assert!(relaxed.stdout.contains("서명할 키: 루트 키"));
    assert!(relaxed
        .stdout
        .contains("hjv-release sign --key <루트 키 파일>"));

    let (keys, root, signing) = (
        world.file("keys.env"),
        world.file("root-101.key"),
        world.file("signing-1.key"),
    );
    let sign = |key: &str, script: &[(&str, &str)]| {
        world.person(
            &[
                "sign",
                "--key",
                key,
                "--keys",
                &keys,
                "--allow-test-keys",
                "--out",
                "policy3.json",
                "draft3.json",
            ],
            script,
        )
    };
    // anything but yes ends it, before any passphrase is asked for
    for answer in ["no", "y", "YES", ""] {
        let output = sign(&root, &[("yes를 입력하세요: ", answer)]);
        assert_eq!(output.code, 1, "{answer:?}");
        assert!(
            output.tty.contains("완화: ios: min을 내림"),
            "{}",
            output.tty
        );
        assert!(output.stderr.contains("yes가 아니어서 서명하지 않았습니다"));
        assert!(!output.tty.contains("의 비밀번호"));
    }
    // a signing key cannot sign it, even with yes
    let output = sign(
        &signing,
        &[("yes를 입력하세요: ", "yes"), ("의 비밀번호: ", PASS)],
    );
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("이 정책은 루트 키로 서명합니다"),
        "{}",
        output.stderr
    );
    assert!(!world.dir.join("policy3.json").exists());
    // yes and the root key
    let output = sign(
        &root,
        &[("yes를 입력하세요: ", "yes"), ("의 비밀번호: ", PASS)],
    );
    assert_eq!(output.code, 0, "{}", output.all());
    assert!(output.tty.contains("yes\n"), "the answer is shown as typed");
    assert!(
        output.tty.contains("필요한 키: 루트 키\n"),
        "{}",
        output.tty
    );
    assert!(output.tty.contains("루트 키(번호 101)를 열었습니다"));
    assert_no_secret(&output, &["root-101", "signing-1"]);
    let policy3 = world.checked("policy3.json");
    assert_eq!(policy3.platforms[&Platform::Ios].min.to_string(), "1.5.0");
    assert_eq!(
        Envelope::parse(&world.read("policy3.json"))
            .unwrap()
            .policy
            .k,
        101
    );

    // a revocation with a new epoch: the root key signs the declaration, signing key 2 the policy
    let revoke = world.policy(&[
        "--prev",
        "policy3.json",
        "--revoke-key",
        "1",
        "--new-epoch",
        "--out",
        "draft4.json",
    ]);
    assert_eq!(revoke.code, 0, "{}", revoke.all());
    assert!(revoke
        .stdout
        .contains("hjv-release sign --key <루트 키 파일> --key <서명 키 파일>"));
    let signing2 = world.file("signing-2.key");
    // one key file is not enough, and that is said before any passphrase is asked for
    let output = world.person(
        &[
            "sign",
            "--key",
            &root,
            "--keys",
            &keys,
            "--allow-test-keys",
            "--out",
            "policy4.json",
            "draft4.json",
        ],
        &[("yes를 입력하세요: ", "yes")],
    );
    assert_eq!(output.code, 1, "{}", output.all());
    assert!(
        output.tty.contains("필요한 키: 루트 키, 서명 키"),
        "{}",
        output.tty
    );
    assert!(
        output.stderr.contains("키 파일이 2개 필요합니다"),
        "{}",
        output.stderr
    );
    assert!(!output.tty.contains("의 비밀번호"));
    let output = world.person(
        &[
            "sign",
            "--key",
            &root,
            "--key",
            &signing2,
            "--keys",
            &keys,
            "--allow-test-keys",
            "--out",
            "policy4.json",
            "draft4.json",
        ],
        &[
            ("yes를 입력하세요: ", "yes"),
            ("root-101.key의 비밀번호: ", PASS),
            ("signing-2.key의 비밀번호: ", PASS),
        ],
    );
    assert_eq!(output.code, 0, "{}", output.all());
    assert!(output.tty.contains("서명 키(번호 1)를 폐기합니다"));
    assert!(output.tty.contains("세대(epoch)를 올립니다(1 → 2)"));
    let envelope = Envelope::parse(&world.read("policy4.json")).unwrap();
    assert_eq!(
        (envelope.policy.k, envelope.root.as_ref().unwrap().k),
        (2, 101)
    );
    let policy4 = world.checked("policy4.json");
    assert_eq!((policy4.epoch, policy4.seq), (2, 1));
    // the revoked key signs nothing any more
    assert_eq!(
        world
            .policy(&[
                "--prev",
                "policy4.json",
                "--contact",
                "내선 1234",
                "--out",
                "draft5.json"
            ])
            .code,
        0
    );
    let output = world.sign("signing-1.key", "draft5.json", "policy5.json");
    assert_eq!(output.code, 1);
    assert!(output.stderr.contains("폐기된 키"), "{}", output.stderr);
    assert!(!world.dir.join("policy5.json").exists());
    assert_eq!(
        world
            .sign("signing-2.key", "draft5.json", "policy5.json")
            .code,
        0
    );
    assert_eq!(
        Envelope::parse(&world.read("policy5.json")).unwrap().root,
        envelope.root
    );
}

#[test]
fn a_draft_that_waited_too_long_asks_first() {
    let world = World::new("stale");
    // a draft as `policy` made it ten days ago: a few days of grace for the minimum and a head
    // start before unattended installs, both of which have run out since
    assert_eq!(world.first_draft("fresh.json").code, 0);
    let fresh = Draft::parse(&world.read("fresh.json")).unwrap();
    let now = time::now();
    let mut policy = Policy::parse(&fresh.p).unwrap();
    policy.issued = now - 10 * time::DAY;
    let windows = policy.platforms.get_mut(&Platform::Windows).unwrap();
    windows.grace_until = Some(now - 3 * time::DAY);
    windows.auto_install_after = Some(now - 8 * time::DAY);
    let stale = Draft {
        p: policy.to_text(),
        ..fresh
    };
    fs::write(world.dir.join("stale.json"), stale.to_text()).unwrap();

    let (keys, key) = (world.file("keys.env"), world.file("signing-1.key"));
    let args = [
        "sign",
        "--key",
        &key,
        "--keys",
        &keys,
        "--allow-test-keys",
        "--out",
        "late.json",
        "stale.json",
    ];
    // the person is told, and anything but yes ends it before a passphrase is asked for
    let refused = world.person(&args, &[("yes를 입력하세요: ", "y")]);
    assert_eq!(refused.code, 1, "{}", refused.all());
    assert!(
        refused.tty.contains("windows: 유예 끝(")
            && refused
                .tty
                .contains("서버에 넣는 즉시 1.5.0 미만을 막습니다"),
        "{}",
        refused.tty
    );
    assert!(
        refused.tty.contains("windows: 무인 설치 시작("),
        "{}",
        refused.tty
    );
    assert!(!refused.tty.contains("의 비밀번호"));
    assert!(!world.dir.join("late.json").exists());
    // the fresh draft of the same policy is signed without a question, and says how long is left
    // (two days, or a moment less if the clock moved on between the two commands)
    let fresh = world.sign("signing-1.key", "fresh.json", "fresh-policy.json");
    assert_eq!(fresh.code, 0, "{}", fresh.all());
    assert!(!fresh.tty.contains("yes를 입력하세요"));
    let left = fresh
        .tty
        .split("지금부터 남은 시간:\n    windows: 무인 설치 시작까지 ")
        .nth(1)
        .unwrap_or("");
    assert!(
        left.starts_with("2일\n") || left.starts_with("1일 23시간\n"),
        "{}",
        fresh.tty
    );
    // with yes the person's decision stands
    let signed = world.person(
        &args,
        &[("yes를 입력하세요: ", "yes"), ("의 비밀번호: ", PASS)],
    );
    assert_eq!(signed.code, 0, "{}", signed.all());
    assert_eq!(
        world.checked("late.json").platforms[&Platform::Windows].grace_until,
        Some(now - 3 * time::DAY)
    );
    assert_no_secret(&signed, &["signing-1"]);
}

#[test]
fn a_wrong_passphrase_opens_nothing() {
    let world = World::new("wrong-passphrase");
    assert_eq!(world.first_draft("draft.json").code, 0);
    let (keys, key) = (world.file("keys.env"), world.file("signing-1.key"));
    let args = [
        "sign",
        "--key",
        &key,
        "--keys",
        &keys,
        "--allow-test-keys",
        "--out",
        "policy.json",
        "draft.json",
    ];
    let wrong = "not the passphrase at all";
    let three = [
        ("의 비밀번호: ", wrong),
        ("의 비밀번호: ", wrong),
        ("의 비밀번호: ", wrong),
    ];
    let output = world.person(&args, &three);
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("비밀번호가 맞지 않습니다"),
        "{}",
        output.stderr
    );
    assert_eq!(output.tty.matches("의 비밀번호: ").count(), 3);
    assert!(!world.dir.join("policy.json").exists());
    assert_no_secret(&output, &["signing-1"]);
    assert!(
        !output.all().contains(wrong),
        "a typed passphrase is never shown"
    );
    // the second try may succeed
    let output = world.person(&args, &[("의 비밀번호: ", wrong), ("의 비밀번호: ", PASS)]);
    assert_eq!(output.code, 0, "{}", output.all());

    // a damaged key file, a file that is no key file, a key the builds do not hold
    let mut damaged = fs::read(world.dir.join("signing-1.key")).unwrap();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    fs::write(world.dir.join("damaged.key"), &damaged).unwrap();
    fs::write(world.dir.join("text.key"), "seed=AAAA\n").unwrap();
    let other = test_key("signing-3").unwrap();
    fs::write(
        world.dir.join("other.key"),
        SecretKey::from_seed(other.role, other.id, &other.seed())
            .encrypt(PASS, 6)
            .unwrap(),
    )
    .unwrap();
    for (file, expected) in [
        ("damaged.key", "키 파일이 손상되었습니다"),
        ("text.key", "키 파일이 아닙니다"),
        ("other.key", "update-keys.env에 없습니다"),
        ("missing.key", "읽을 수 없습니다"),
    ] {
        let key = world.file(file);
        let output = world.person(
            &[
                "sign",
                "--key",
                &key,
                "--keys",
                &keys,
                "--allow-test-keys",
                "--out",
                "policy2.json",
                "draft.json",
            ],
            &[("의 비밀번호: ", PASS)],
        );
        assert_eq!(output.code, 1, "{file}");
        assert!(
            output.stderr.contains(expected),
            "{file}: {}",
            output.stderr
        );
        assert!(!world.dir.join("policy2.json").exists());
        assert_no_secret(&output, &["signing-1", "signing-3"]);
    }
    // a draft that was touched after `policy` made it
    // a draft changed into something the rules do not allow is refused before any passphrase
    let draft = world.read("draft.json");
    let touched = draft.replacen("\\\"min\\\":\\\"1.5.0\\\"", "\\\"min\\\":\\\"1.7.0\\\"", 1);
    assert_ne!(touched, draft);
    fs::write(world.dir.join("touched.json"), touched).unwrap();
    let output = world.sign("signing-1.key", "touched.json", "policy3.json");
    assert_eq!(output.code, 1, "{}", output.all());
    assert!(
        output
            .stderr
            .contains("min(1.7.0)이 latest(1.6.0)보다 높습니다"),
        "{}",
        output.stderr
    );
    assert!(
        !output.tty.contains("의 비밀번호"),
        "refused before any passphrase"
    );
    fs::write(
        world.dir.join("envelope-as-draft.json"),
        world.read("policy.json"),
    )
    .unwrap();
    let output = world.sign("signing-1.key", "envelope-as-draft.json", "policy3.json");
    assert!(output.stderr.contains("초안 파일이 아닙니다"));
}

#[test]
fn ctrl_c_at_the_prompt_leaves_the_terminal_usable() {
    let world = World::new("ctrl-c");
    assert_eq!(world.first_draft("draft.json").code, 0);
    let (keys, key) = (world.file("keys.env"), world.file("signing-1.key"));
    let args = [
        "sign",
        "--key",
        &key,
        "--keys",
        &keys,
        "--allow-test-keys",
        "--out",
        "policy.json",
        "draft.json",
    ];
    // the person changes their mind at the passphrase prompt, where the echo is off
    let output = world.person(&args, &[("의 비밀번호: ", CTRL_C)]);
    assert_ne!(output.code, 0);
    assert_ne!(
        output.code,
        common::ASKED_FOR_MORE,
        "the tool ended by itself"
    );
    assert!(output.echo, "the terminal echoes again");
    assert!(!world.dir.join("policy.json").exists());
    // and after an ordinary run as well
    let output = world.sign("signing-1.key", "draft.json", "policy.json");
    assert_eq!(output.code, 0, "{}", output.all());
    assert!(output.echo);
}

#[test]
fn provenance_refusals_from_recorded_gh_output() {
    let world = World::new("provenance");
    let cases = [
        ("run-failed.json", "build.yml", "성공으로 끝나지 않았습니다"),
        (
            "run-other-workflow.json",
            "build.yml",
            "다른 워크플로의 실행입니다",
        ),
        ("run-other-commit.json", "build.yml", "부른 워크플로가"),
        (
            "run.json",
            "build-other-commit.yml",
            "uses가 검토한 커밋으로 고정되어 있지 않습니다",
        ),
        ("run.json", "build-extra-job.yml", "job이 하나가 아닙니다"),
    ];
    for (run, build, expected) in cases {
        world.gh(run, build);
        let output = world.first_draft("draft.json");
        assert_eq!(output.code, 1, "{run} {build}");
        assert!(
            output.stderr.contains("출처 확인 실패(windows 1.6.0)"),
            "{}",
            output.stderr
        );
        assert!(
            output.stderr.contains(expected),
            "{run} {build}: {}",
            output.stderr
        );
        assert!(
            !world.dir.join("draft.json").exists(),
            "no draft without provenance"
        );
    }
    // another commit than the tagged one was built
    world.gh("run.json", "build.yml");
    let release = world.release("1.6.0");
    let json = Path::new(&release).join("HJViewer-1.6.0-1111111-windows/release.json");
    let text = fs::read_to_string(&json).unwrap();
    fs::write(&json, text.replace(&world.source_sha, &"3".repeat(40))).unwrap();
    let output = world.policy(&[
        "--prev",
        "none",
        "--server",
        SERVER,
        "--release-dir",
        &release,
        "--min",
        "all=1.5.0",
        "--grace-days",
        "0",
        "--ios-valid",
        &world.ios_build,
        "--out",
        "draft.json",
    ]);
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("태그 client-1.6.0의 커밋"),
        "{}",
        output.stderr
    );
    fs::write(&json, &text).unwrap();
    // no tag for the version
    git(&world.dir.join("repo"), &["tag", "-d", "client-1.6.0"]);
    let output = world.first_draft("draft.json");
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("태그가 없습니다: client-1.6.0"),
        "{}",
        output.stderr
    );
    git(&world.dir.join("repo"), &["tag", "client-1.6.0"]);
    // gh cannot answer: no network, or no such run
    fs::remove_file(world.dir.join("gh/run.json")).unwrap();
    let output = world.first_draft("draft.json");
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("출처 확인 실패")
            && output.stderr.contains("gh 명령이 실패했습니다"),
        "{}",
        output.stderr
    );
    assert!(!world.dir.join("draft.json").exists());
    // a package that is not what release.json says
    world.gh("run.json", "build.yml");
    let release = world.release("1.6.0");
    fs::write(
        Path::new(&release).join("HJViewer-1.6.0-1111111-windows/HJViewer.exe"),
        "windows package 1.6.X",
    )
    .unwrap();
    let output = world.policy(&[
        "--prev",
        "none",
        "--server",
        SERVER,
        "--release-dir",
        &release,
        "--min",
        "all=1.5.0",
        "--grace-days",
        "0",
        "--ios-valid",
        &world.ios_build,
        "--out",
        "draft.json",
    ]);
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("파일이 release.json과 다릅니다"),
        "{}",
        output.stderr
    );
    assert!(!world.dir.join("draft.json").exists());
}

/// `s` with `YYYY` as the year three years from now: a date that is ahead whenever the tests
/// run (any month and day that every year has).
fn ahead(s: &str) -> String {
    let (year, _, _) = time::civil_from_days(time::now().div_euclid(time::DAY));
    s.replace("YYYY", &(year + 3).to_string())
}

/// A policy for `hjv.test` signed with the test keys, by `test-sign`.
fn test_signed(
    world: &World,
    name: &str,
    seq: u64,
    min: &str,
    grace: Option<&str>,
    key: &str,
) -> String {
    let grace = grace
        .map(|g| format!(",\n  \"grace_until\": \"{g}\""))
        .unwrap_or_default();
    let entry =
        |extra: &str| format!("{{\"min\": \"{min}\", \"latest\": \"1.6.1\"{grace}{extra}}}");
    let body = format!(
        "{{\n \"v\": 1, \"epoch\": 1, \"seq\": {seq}, \"issued\": \"2026-10-0{seq}T03:00:00Z\", \"server\": \"hjv.test\",\n \
         \"platforms\": {{\n  \"windows\": {},\n  \"ios\": {}\n }}\n}}\n",
        entry(", \"package\": {\"file\": \"HJViewer-1.6.1-windows-x86_64.exe\", \"size\": 10, \"sha256\": \"0000000000000000000000000000000000000000000000000000000000000000\", \"build\": \"1.6.1.7\"}"),
        entry(&format!(", \"testflight_build\": \"{}\"", world.ios_build)),
    );
    fs::write(world.dir.join(format!("{name}.body.json")), body).unwrap();
    let (body, out) = (
        world.file(&format!("{name}.body.json")),
        world.file(&format!("{name}.json")),
    );
    let output = world.tool(&["test-sign", "--policy", &body, "--key", key, "--out", &out]);
    assert_eq!(output.code, 0, "{}", output.all());
    assert!(output.stderr.contains("시험 키"));
    out
}

#[test]
fn test_sign_makes_envelopes_for_other_tests() {
    let world = World::new("test-sign");
    let keys = world.file("keys.env");
    let first = test_signed(&world, "p1", 1, "1.5.0", None, "signing-1");
    let verify = |more: &[&str]| {
        let mut args = vec![
            "verify",
            "--keys",
            &keys,
            "--allow-test-keys",
            "--server",
            "hjv.test",
        ];
        args.extend_from_slice(more);
        world.tool(&args)
    };
    assert_eq!(verify(&[&first]).code, 0);
    // the hand-written body was put into the one form the tool writes
    let envelope = Envelope::parse(&fs::read_to_string(&first).unwrap()).unwrap();
    assert!(envelope
        .policy
        .p
        .starts_with("{\"v\":1,\"epoch\":1,\"seq\":1,"));
    assert_eq!(
        Policy::parse(&envelope.policy.p).unwrap().server,
        "hjv.test"
    );
    // a relaxation by a signing key is made without complaint (--raw is not needed: the envelope
    // is fine by itself) and refused by an app that holds the stricter policy; the root key's passes
    let strict = test_signed(&world, "p2", 2, "1.6.1", None, "signing-1");
    let eased = test_signed(&world, "p3", 3, "1.5.0", None, "signing-2");
    let eased_by_root = test_signed(&world, "p3r", 3, "1.5.0", None, "root-101");
    assert_eq!(verify(&["--prev", &first, &strict]).code, 0);
    let refused = verify(&["--prev", &strict, &eased]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("루트 키가 필요"),
        "{}",
        refused.stderr
    );
    assert_eq!(verify(&["--prev", &strict, &eased_by_root]).code, 0);
    // without --out the envelope goes to standard output
    let body = world.file("p1.body.json");
    let output = world.tool(&["test-sign", "--policy", &body]);
    assert_eq!(output.stdout, fs::read_to_string(&first).unwrap());
    // a body that breaks a rule is refused; --raw signs the bytes as they are
    fs::write(
        world.dir.join("bad.json"),
        fs::read_to_string(&body)
            .unwrap()
            .replace("\"min\": \"1.5.0\"", "\"min\": \"1.6\""),
    )
    .unwrap();
    let bad = world.file("bad.json");
    assert_eq!(world.tool(&["test-sign", "--policy", &bad]).code, 1);
    let raw = world.tool(&["test-sign", "--policy", &bad, "--raw", "--out", "raw.json"]);
    assert_eq!(raw.code, 0, "{}", raw.all());
    assert_eq!(verify(&["raw.json"]).code, 1);
    // a key no test build holds needs --raw as well, and only the test keys exist here
    assert_eq!(
        world
            .tool(&["test-sign", "--policy", &body, "--key", "signing-3"])
            .code,
        1
    );
    assert_eq!(
        world
            .tool(&[
                "test-sign",
                "--policy",
                &body,
                "--key",
                "signing-3",
                "--raw"
            ])
            .code,
        0
    );
    let unknown = world.tool(&["test-sign", "--policy", &body, "--key", "release"]);
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.contains("signing-1, signing-2, root-101"));
    // a root declaration goes along
    fs::write(world.dir.join("decl.json"), "{\"v\":1, \"server\":\"hjv.test\", \"epoch\":1, \"revoked\":[2], \"issued\":\"2026-10-04T03:00:00Z\"}\n").unwrap();
    let decl = world.file("decl.json");
    let body4 = world.file("p3.body.json");
    let output = world.tool(&[
        "test-sign",
        "--policy",
        &body4,
        "--root-decl",
        &decl,
        "--key",
        "signing-1",
        "--out",
        "with-decl.json",
    ]);
    assert_eq!(output.code, 0, "{}", output.all());
    let envelope = Envelope::parse(&world.read("with-decl.json")).unwrap();
    assert_eq!(envelope.root.unwrap().k, 101);
    assert_eq!(
        world
            .tool(&[
                "test-sign",
                "--policy",
                &body4,
                "--root-decl",
                &decl,
                "--key",
                "signing-2"
            ])
            .code,
        1,
        "the declaration revokes key 2"
    );
}

#[test]
fn hbbs_conf_and_door_2() {
    let world = World::new("hbbs");
    let keys = world.file("keys.env");
    // the deadlines of this test, in a year that is ahead whenever the test runs
    let (grace_1, grace_2) = (ahead("YYYY-01-14T15:00:00Z"), ahead("YYYY-03-01T15:00:00Z"));
    let (until, some_day) = (ahead("YYYY-02-15"), ahead("YYYY-01-01"));
    let none = test_signed(&world, "p1", 1, "1.5.0", None, "signing-1");
    let enforced = test_signed(&world, "p2", 2, "1.6.0", Some(&grace_1), "signing-1");
    let conf = |more: &[&str]| {
        let mut args = vec!["hbbs-conf", "--keys", &keys, "--allow-test-keys"];
        args.extend_from_slice(more);
        world.tool(&args)
    };

    // M5: door 2 opens, hbbs enforces nothing yet
    let door_only = conf(&[
        "--door2-out",
        "door",
        "--door2-only",
        "--door2-until",
        &until,
    ]);
    assert_eq!(door_only.code, 0, "{}", door_only.all());
    assert!(door_only.stdout.contains("새 코드"));
    assert_eq!(mode(&world.dir.join("door")), 0o700);
    assert_eq!(mode(&world.dir.join("door/door2.conf")), 0o600);
    let door = Door2::parse(&world.read("door/door2.conf")).unwrap();
    assert_eq!(time::format_utc(door.until), ahead("YYYY-02-14T15:00:00Z"));
    assert!(!world.dir.join("door/min-controller.conf").exists());
    assert!(
        !door_only.all().contains(&door.code),
        "the code is never shown"
    );

    // M7: the minimum is enforced; the text names door 2 with the code the door already has
    let made = conf(&[
        "--policy",
        &enforced,
        "--prev-policy",
        &none,
        "--current",
        "none",
        "--door2-out",
        "door",
    ]);
    assert_eq!(made.code, 0, "{}", made.all());
    assert!(
        made.stdout
            .contains(&format!("hbbs 파일을 바꿈:\nMIN=1.6.0\nFROM={grace_1}\n")),
        "{}",
        made.stdout
    );
    assert!(made.stdout.contains(&ahead(
        "YYYY-01-15 00:00 한국 시각 · 기준 항목: windows, ios"
    )));
    assert!(made.stdout.contains("코드 그대로"));
    assert!(!made.all().contains(&door.code), "the code is never shown");
    assert_eq!(Door2::parse(&world.read("door/door2.conf")).unwrap(), door);
    let file = world.read("door/min-controller.conf");
    assert_eq!(mode(&world.dir.join("door/min-controller.conf")), 0o600);
    assert!(file.contains("\nMIN=1.6.0\n") && file.contains(&format!("\nFROM={grace_1}\n")));
    assert!(file.contains(&format!(
        " https://hjv.test/api/hjv/get/{}/ 에서",
        door.code
    )));
    assert!(file.trim_end().ends_with("(version not allowed)"));
    assert!(file.contains("%MIN%"));

    // one platform alone raises its min: the file stays as it is
    let body = world
        .read("p2.body.json")
        .replacen("\"min\": \"1.6.0\"", "\"min\": \"1.6.1\"", 1)
        .replacen("\"seq\": 2", "\"seq\": 3", 1)
        .replacen(&grace_1, &grace_2, 1);
    fs::write(world.dir.join("p3.body.json"), body).unwrap();
    let body = world.file("p3.body.json");
    assert_eq!(
        world
            .tool(&["test-sign", "--policy", &body, "--out", "p3.json"])
            .code,
        0
    );
    let current = world.file("door/min-controller.conf");
    let same = conf(&[
        "--policy",
        "p3.json",
        "--prev-policy",
        &enforced,
        "--current",
        &current,
    ]);
    assert_eq!(same.code, 0, "{}", same.all());
    assert!(
        same.stdout.contains("hbbs 파일은 바꾸지 않음"),
        "{}",
        same.stdout
    );
    assert!(!same.stdout.contains(&format!("FROM={grace_2}")));

    // every min above MIN: a new FROM. An earlier one than announced is refused
    let later = test_signed(&world, "p4", 4, "1.6.1", Some(&grace_2), "signing-1");
    let raised = conf(&[
        "--policy",
        &later,
        "--prev-policy",
        "p3.json",
        "--current",
        &current,
    ]);
    assert_eq!(raised.code, 0, "{}", raised.all());
    assert!(
        raised
            .stdout
            .contains(&format!("MIN=1.6.1\nFROM={grace_2}\n")),
        "{}",
        raised.stdout
    );
    let earlier = test_signed(
        &world,
        "p5",
        5,
        "1.6.1",
        Some(&ahead("YYYY-01-10T15:00:00Z")),
        "signing-1",
    );
    let back = conf(&[
        "--policy",
        &earlier,
        "--prev-policy",
        &enforced,
        "--current",
        &current,
    ]);
    assert_eq!(back.code, 1);
    assert!(
        back.stderr.contains("FROM이 뒤로 갑니다"),
        "{}",
        back.stderr
    );

    // door 2 closed: the text without a link, to a file of its own; a rotated code
    let closed = conf(&[
        "--policy",
        &enforced,
        "--current",
        "none",
        "--text",
        "s2",
        "--out",
        "closed.conf",
    ]);
    assert_eq!(closed.code, 0, "{}", closed.all());
    let text = world.read("closed.conf");
    assert!(text.contains("관리자에게 새 버전을 받으세요") && !text.contains("http"));
    let rotated = conf(&[
        "--policy",
        &enforced,
        "--current",
        &current,
        "--door2-out",
        "door",
        "--door2-rotate",
    ]);
    assert_eq!(rotated.code, 0, "{}", rotated.all());
    let new_door = Door2::parse(&world.read("door/door2.conf")).unwrap();
    assert_ne!(new_door.code, door.code);
    assert_eq!(new_door.until, door.until);
    assert!(world
        .read("door/min-controller.conf")
        .contains(&new_door.code));
    assert!(!rotated.all().contains(&new_door.code) && !rotated.all().contains(&door.code));

    // mistakes
    for (args, expected) in [
        (vec!["--policy", enforced.as_str()], "--current"),
        (
            vec![
                "--policy",
                enforced.as_str(),
                "--current",
                "none",
                "--text",
                "s1",
                "--out",
                "x.conf",
            ],
            "--door2-out",
        ),
        (
            vec![
                "--policy",
                enforced.as_str(),
                "--current",
                "none",
                "--door2-out",
                "door2",
                "--door2-only",
                "--door2-rotate",
            ],
            "--door2-until",
        ),
        (
            vec![
                "--policy",
                enforced.as_str(),
                "--current",
                "none",
                "--door2-until",
                some_day.as_str(),
            ],
            "--door2-out과 함께",
        ),
        (
            vec![
                "--policy",
                enforced.as_str(),
                "--current",
                "none",
                "--door2-out",
                "door",
                "--door2-until",
                "2020-01-01",
            ],
            "이미 지났습니다",
        ),
    ] {
        let output = conf(&args);
        assert_eq!(output.code, 1, "{args:?}");
        assert!(
            output.stderr.contains(expected),
            "{args:?}: {}",
            output.stderr
        );
    }
    // a door2.conf that cannot be read, or is damaged, is never replaced by a new code
    fs::create_dir(world.dir.join("damaged")).unwrap();
    fs::set_permissions(world.dir.join("damaged"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        world.dir.join("damaged/door2.conf"),
        ahead("CODE=not a code\nUNTIL=YYYY-02-14T15:00:00Z\n"),
    )
    .unwrap();
    let damaged = conf(&[
        "--door2-out",
        "damaged",
        "--door2-only",
        "--door2-until",
        &until,
    ]);
    assert_eq!(damaged.code, 1);
    assert!(
        damaged.stderr.contains("CODE가 32자리 hex가 아닙니다"),
        "{}",
        damaged.stderr
    );
    assert!(world.read("damaged/door2.conf").contains("not a code"));
    // a directory others can read is no place for the code
    fs::create_dir(world.dir.join("open")).unwrap();
    fs::set_permissions(world.dir.join("open"), fs::Permissions::from_mode(0o755)).unwrap();
    let open = conf(&[
        "--door2-out",
        "open",
        "--door2-only",
        "--door2-until",
        &until,
    ]);
    assert_eq!(open.code, 1);
    assert!(open.stderr.contains("chmod 700"));
    // a custom text must keep the rules of texts for old apps
    fs::write(world.dir.join("text.txt"), "새 버전으로 업데이트하세요.\n").unwrap();
    let bad_text = conf(&[
        "--policy",
        &enforced,
        "--current",
        "none",
        "--text-file",
        "text.txt",
        "--out",
        "y.conf",
    ]);
    assert_eq!(bad_text.code, 1);
    assert!(bad_text.stderr.contains("version not allowed"));
}

#[test]
fn the_weekly_check() {
    let world = World::new("check");
    let keys = world.file("keys.env");
    let policy = test_signed(&world, "p1", 1, "1.5.0", None, "signing-1");
    let check = |more: &[&str]| {
        let mut args = vec!["check", "--keys", &keys, "--policy", &policy];
        args.extend_from_slice(more);
        world.tool(&args)
    };
    // the test keys are a warning by themselves: exit code 3
    let plain = check(&[]);
    assert_eq!(plain.code, 3, "{}", plain.all());
    assert!(plain
        .stdout
        .contains("경고: 키 파일에 이 저장소의 시험 키가 있습니다"));
    assert!(plain
        .stdout
        .contains(&format!("TestFlight 빌드 {}: 올린 지 1일", world.ios_build)));
    assert!(plain.stdout.contains("경고 1건"));
    // door 2 about to close, and the server handing out another policy
    fs::write(
        world.dir.join("door2.conf"),
        format!(
            "CODE=0123456789abcdef0123456789abcdef\nUNTIL={}\n",
            time::format_utc(time::now() + 3 * time::DAY)
        ),
    )
    .unwrap();
    let other = test_signed(
        &world,
        "p0",
        1,
        "1.5.0",
        Some(&ahead("YYYY-01-01T00:00:00Z")),
        "signing-1",
    );
    let output = check(&["--door2", "door2.conf", "--live", &other]);
    assert_eq!(output.code, 3);
    assert!(output.stdout.contains("경고: 문 2가"));
    assert!(output.stdout.contains("같은 위치"));
    assert!(output.stdout.contains("경고 3건"));
    assert!(!output.all().contains("0123456789abcdef"));
    let same = check(&["--live", &policy]);
    assert!(same
        .stdout
        .contains("서버가 내주는 정책이 이 PC의 정책과 같습니다"));
    // an old TestFlight build
    let old_build = build_number(time::now() - 76 * time::DAY);
    let body = world
        .read("p1.body.json")
        .replace(&world.ios_build, &old_build);
    fs::write(world.dir.join("old.body.json"), body).unwrap();
    let body = world.file("old.body.json");
    assert_eq!(
        world
            .tool(&["test-sign", "--policy", &body, "--out", "old.json"])
            .code,
        0
    );
    let output = world.tool(&["check", "--keys", &keys, "--policy", "old.json"]);
    assert_eq!(output.code, 3);
    assert!(
        output.stdout.contains("새 TestFlight 빌드를 올리세요"),
        "{}",
        output.stdout
    );
    // a policy that does not verify is an error, not a warning
    fs::write(world.dir.join("bad.json"), "{}").unwrap();
    assert_eq!(
        world
            .tool(&["check", "--keys", &keys, "--policy", "bad.json"])
            .code,
        1
    );
}

#[test]
fn test_keys_and_key_files() {
    let world = World::new("keys");
    // the list CI and the packaging scripts refuse
    let list = world.tool(&["test-keys"]);
    assert_eq!(list.code, 0);
    for known in [
        "JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0=  # signing key 1",
        "zRSzf5VulTGU/3+3Oz2B3MVh1hp1OAlLfD4aZD7l86o=  # signing key 2",
        "T9CZzNR9eJPf6ewkQU7LDZtUICMqrTDZHEZb4zy+ZcQ=  # root key 101",
        "F0VTtFbd38aQjsqxwQH+arIeK6oGF3lbfUOmNIKZP9U=  # the device key of the research vector",
        "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=  # relay/PROTOCOL.md 13, command key",
    ] {
        assert!(list.stdout.contains(known), "{known}\n{}", list.stdout);
    }
    // the key file of test builds is the key file of the vectors
    let env = world.tool(&["test-keys", "--env"]);
    let vectors_file = fs::read_to_string(vectors().join("update-keys.env")).unwrap();
    let lines: Vec<&str> = vectors_file
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect();
    assert_eq!(env.stdout.lines().collect::<Vec<_>>(), lines);

    // `keys`: what a key file holds; --production refuses test keys
    let keys = world.file("keys.env");
    let shown = world.tool(&["keys", "--keys", &keys, "--server", SERVER]);
    assert_eq!(shown.code, 0, "{}", shown.all());
    assert!(shown
        .stdout
        .contains("signing 1:JUO5L/EJVRFHatyDadtt3JM2ZaEZeN2hQE7hBmypVZ0=  (시험 키)"));
    assert!(shown.stdout.contains("root 101:"));
    assert!(shown.stdout.contains("floor 1:0"));
    let parsed = KeySet::parse(&world.read("keys.env")).unwrap();
    assert!(shown
        .stdout
        .contains(&format!("fingerprint {}", hex(&parsed.fingerprint(SERVER)))));
    let production = world.tool(&["keys", "--keys", &keys, "--production"]);
    assert_eq!(production.code, 1);
    assert!(production.stderr.contains("실서버 빌드에 쓸 수 없습니다"));
    // fresh keys pass
    let fresh = |role, id| SecretKey::generate(role, id).unwrap().public().to_string();
    fs::write(
        world.dir.join("real.env"),
        format!(
            "HJV_UPDATE_PUBKEY={},{}\nHJV_UPDATE_ROOT_PUBKEY={}\nHJV_POLICY_FLOOR=1:0\n",
            fresh(Role::Signing, 1),
            fresh(Role::Signing, 2),
            fresh(Role::Root, 101)
        ),
    )
    .unwrap();
    let real = world.file("real.env");
    let production = world.tool(&["keys", "--keys", &real, "--production"]);
    assert_eq!(production.code, 0, "{}", production.all());
    assert!(!production.stdout.contains("시험 키") && production.stderr.is_empty());
    // the placeholder file of a build without update keys, and a broken one
    fs::write(
        world.dir.join("empty.env"),
        "# no update keys yet\nHJV_UPDATE_PUBKEY=\nHJV_UPDATE_ROOT_PUBKEY=\nHJV_POLICY_FLOOR=\n",
    )
    .unwrap();
    let empty = world.tool(&["keys", "--keys", "empty.env", "--production"]);
    assert_eq!(empty.code, 1);
    assert!(empty.stderr.contains("비어 있습니다"));
    fs::write(
        world.dir.join("broken.env"),
        world
            .read("real.env")
            .replace("HJV_UPDATE_ROOT_PUBKEY=101:", "HJV_UPDATE_ROOT_PUBKEY=1:"),
    )
    .unwrap();
    assert_eq!(world.tool(&["keys", "--keys", "broken.env"]).code, 1);
    // a policy made with real keys is not verified by test keys, and the other way round
    let policy = test_signed(&world, "p1", 1, "1.5.0", None, "signing-1");
    let output = world.tool(&["verify", "--keys", &real, "--server", "hjv.test", &policy]);
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.contains("서명이 맞지 않습니다"),
        "{}",
        output.stderr
    );
}

#[test]
fn pubkey_shows_the_public_line() {
    let world = World::new("pubkey");
    let key = world.file("root-101.key");
    let output = world.person(&["pubkey", "--key", &key], &[("의 비밀번호: ", PASS)]);
    assert_eq!(output.code, 0, "{}", output.all());
    assert_eq!(
        output.stdout.trim_end(),
        test_key("root-101").unwrap().public().to_string()
    );
    assert!(output.tty.contains("루트 키 101의 공개 줄입니다"));
    assert_no_secret(&output, &["root-101"]);
    let no_tty = world.tool(&["pubkey", "--key", &key]);
    assert_eq!(no_tty.code, 1);
    assert!(no_tty.stderr.contains("터미널(TTY)이 없어"));
}
