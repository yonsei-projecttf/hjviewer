// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-release publish-dir`: gathers what goes to the server for a policy into one directory,
//! laid out like the server's (design 4-3 and 4-5):
//!
//! ```text
//! <out>/version-policy.json                        the signed envelope (once it is signed)
//! <out>/pkg/HJViewer-1.6.0-windows-x86_64.exe      the packages, under the names the policy has
//! <out>/pkg/HJViewer-1.6.0-android-arm64.apk
//! <out>/pkg/notices-1.6.0/LICENSE.txt NOTICE.txt SOURCE.txt THIRD_PARTY_NOTICES.txt
//! <out>/SHA256SUMS                                 every file above, for sha256sum -c
//! ```
//!
//! Each package is copied from the release directory and then hashed again where it lies: the
//! copy must have the size and the SHA-256 the policy names. The third-party notices differ by
//! platform, so the one file holds a section for each package of that version.

use crate::{
    policy::{Platform, Policy},
    release::{hash_file, Artifact, NOTICE_FILES, SHA256SUMS},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const POLICY_FILE: &str = "version-policy.json";

fn io(path: &Path, what: &str) -> impl Fn(std::io::Error) -> String {
    let at = path.display().to_string();
    let what = what.to_owned();
    move |e| format!("{at}: {what}({e})")
}

/// Writes `content` to `path`, or checks that the file there already holds exactly that.
fn place(path: &Path, content: &[u8]) -> Result<(), String> {
    match fs::read(path) {
        Ok(existing) if existing == content => Ok(()),
        Ok(_) => Err(format!(
            "{}: 다른 내용의 파일이 이미 있습니다(새 디렉터리를 쓰세요)",
            path.display()
        )),
        Err(_) => fs::write(path, content).map_err(io(path, "쓸 수 없습니다")),
    }
}

/// Every file under `dir`, as paths relative to it with `/`, sorted.
fn files_under(dir: &Path) -> Result<Vec<String>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(io(dir, "읽을 수 없습니다"))? {
            let path = entry.map_err(io(dir, "읽을 수 없습니다"))?.path();
            let meta = fs::symlink_metadata(&path).map_err(io(&path, "읽을 수 없습니다"))?;
            if meta.is_dir() {
                walk(root, &path, out)?;
            } else if meta.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| "경로를 다루지 못했습니다")?;
                let parts: Vec<String> = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.push(parts.join("/"));
            } else {
                return Err(format!("{}: 보통 파일이 아닙니다", path.display()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Fills `out` for `policy`. `envelope` is the signed file, if there is one yet. Returns what
/// was done, for the operator.
pub fn publish(
    policy: &Policy,
    envelope: Option<&str>,
    artifacts: &[Artifact],
    out: &Path,
) -> Result<Vec<String>, String> {
    let pkg = out.join("pkg");
    fs::create_dir_all(&pkg).map_err(io(&pkg, "만들 수 없습니다"))?;
    let mut report = Vec::new();
    let mut expected: Vec<String> = Vec::new();
    // version -> (package file name, the artifact's third-party notices), by package
    let mut third_party: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();

    for platform in [Platform::Windows, Platform::Android] {
        let Some(entry) = policy.platforms.get(&platform) else {
            continue;
        };
        let package = entry
            .package
            .as_ref()
            .ok_or_else(|| format!("{platform}: 정책에 package가 없습니다"))?;
        let artifact = artifacts
            .iter()
            .find(|a| a.info.platform.platform() == platform && a.info.version == entry.latest);
        let Some(artifact) = artifact else {
            report.push(format!(
                "{platform} {}: 릴리스 디렉터리에 없어 넣지 않았습니다. 서버의 pkg/에 이 파일이 이미 있어야 합니다: {}",
                entry.latest, package.file
            ));
            continue;
        };
        if artifact.info.sha256 != package.sha256 || artifact.info.size != package.size {
            return Err(format!(
                "{platform} {}: 릴리스 디렉터리의 패키지가 정책의 해시와 다릅니다(정책 {}, 파일 {})",
                entry.latest, package.sha256, artifact.info.sha256
            ));
        }
        let target = pkg.join(&package.file);
        if !target.exists() {
            fs::copy(artifact.path(), &target).map_err(io(&target, "복사할 수 없습니다"))?;
        }
        // what lies in the directory is what the policy names, whatever the copy did
        let (size, sha256) = hash_file(&target)?;
        if size != package.size || sha256 != package.sha256 {
            return Err(format!(
                "{}: 복사한 파일이 정책과 다릅니다(크기 {size}, sha256 {sha256})",
                target.display()
            ));
        }
        expected.push(format!("pkg/{}", package.file));
        report.push(format!(
            "pkg/{}  {}바이트  sha256 {}",
            package.file, size, sha256
        ));

        let version = entry.latest.to_string();
        let notices = pkg.join(format!("notices-{version}"));
        fs::create_dir_all(&notices).map_err(io(&notices, "만들 수 없습니다"))?;
        for name in NOTICE_FILES {
            let source = artifact.dir.join(name);
            let content = fs::read(&source).map_err(io(&source, "알림 파일을 읽을 수 없습니다"))?;
            if name == "THIRD_PARTY_NOTICES.txt" {
                third_party
                    .entry(version.clone())
                    .or_default()
                    .push((package.file.clone(), content));
            } else {
                place(&notices.join(name), &content).map_err(|e| {
                    format!("{e}. 같은 버전의 다른 플랫폼 것과 다른 알림 파일입니다({name})")
                })?;
                let listed = format!("pkg/notices-{version}/{name}");
                if !expected.contains(&listed) {
                    expected.push(listed);
                }
            }
        }
    }
    for (version, sections) in &third_party {
        let mut text = format!(
            "Third-party notices of HJViewer {version}: one section for each package of this version.\n"
        )
        .into_bytes();
        for (file, content) in sections {
            text.extend_from_slice(format!("\n==== {file} ====\n\n").as_bytes());
            text.extend_from_slice(content);
            if !content.ends_with(b"\n") {
                text.push(b'\n');
            }
        }
        let name = format!("pkg/notices-{version}/THIRD_PARTY_NOTICES.txt");
        place(&out.join(&name), &text)?;
        expected.push(name);
    }
    match envelope {
        Some(envelope) => {
            place(&out.join(POLICY_FILE), envelope.as_bytes())?;
            expected.push(POLICY_FILE.to_owned());
            report.push(format!(
                "{POLICY_FILE}  sha256 {}",
                crate::trust::hex(&crate::trust::sha256(envelope.as_bytes()))
            ));
        }
        None => report.push(format!(
            "{POLICY_FILE}은 아직 없습니다. 서명한 뒤 같은 명령을 서명된 파일로 다시 실행하세요"
        )),
    }

    // nothing else may ride along to the server
    let present = files_under(out)?;
    if let Some(stray) = present
        .iter()
        .find(|f| *f != SHA256SUMS && !expected.contains(f))
    {
        return Err(format!(
            "{}에 이 정책과 관계없는 파일이 있습니다: {stray}(새 디렉터리를 쓰세요)",
            out.display()
        ));
    }
    let mut sums = String::new();
    for file in present.iter().filter(|f| *f != SHA256SUMS) {
        let (_, sha256) = hash_file(&out.join(file))?;
        sums.push_str(&format!("{sha256}  {file}\n"));
    }
    let sums_path: PathBuf = out.join(SHA256SUMS);
    fs::write(&sums_path, &sums).map_err(io(&sums_path, "쓸 수 없습니다"))?;
    report.push(format!(
        "{SHA256SUMS}: 파일 {}개(서버에서 sha256sum -c {SHA256SUMS})",
        sums.lines().count()
    ));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        policy::tests::sample,
        release::{
            scan,
            tests::{temp_dir, write_artifact},
        },
        trust::{hex, sha256},
        version::Version,
    };

    /// The sample policy with packages that are the artifacts written here.
    fn fixture(name: &str) -> (Policy, Vec<Artifact>, PathBuf) {
        let root = temp_dir(name);
        let release = root.join("release-1.6.1");
        write_artifact(
            &release,
            "windows",
            "1.6.1",
            "1.6.1.7",
            b"windows package 1.6.1",
        );
        write_artifact(
            &release,
            "android",
            "1.6.1",
            "263122251",
            b"android package 1.6.1",
        );
        write_artifact(&release, "ios", "1.6.1", "263120249", b"ios package 1.6.1");
        let artifacts = scan(&[release]).unwrap();
        let mut policy = sample();
        for (platform, content) in [
            (Platform::Windows, &b"windows package 1.6.1"[..]),
            (Platform::Android, b"android package 1.6.1"),
        ] {
            let package = policy
                .platforms
                .get_mut(&platform)
                .unwrap()
                .package
                .as_mut()
                .unwrap();
            package.size = content.len() as u64;
            package.sha256 = hex(&sha256(content));
        }
        (policy, artifacts, root)
    }

    #[test]
    fn the_directory() {
        let (policy, artifacts, root) = fixture("publish");
        let out = root.join("out");
        // first with the draft only, then again with the signed file: the same directory
        let report = publish(&policy, None, &artifacts, &out).unwrap();
        assert!(report.iter().any(|l| l.contains("아직 없습니다")));
        assert!(!out.join(POLICY_FILE).exists());
        let report = publish(&policy, Some("{\"v\":1}\n"), &artifacts, &out).unwrap();
        assert!(report
            .iter()
            .any(|l| l.starts_with("version-policy.json  sha256 ")));
        assert_eq!(
            files_under(&out).unwrap(),
            [
                "SHA256SUMS",
                "pkg/HJViewer-1.6.1-android-arm64.apk",
                "pkg/HJViewer-1.6.1-windows-x86_64.exe",
                "pkg/notices-1.6.1/LICENSE.txt",
                "pkg/notices-1.6.1/NOTICE.txt",
                "pkg/notices-1.6.1/SOURCE.txt",
                "pkg/notices-1.6.1/THIRD_PARTY_NOTICES.txt",
                "version-policy.json",
            ]
        );
        assert_eq!(
            fs::read(out.join("pkg/HJViewer-1.6.1-windows-x86_64.exe")).unwrap(),
            b"windows package 1.6.1"
        );
        assert_eq!(fs::read(out.join(POLICY_FILE)).unwrap(), b"{\"v\":1}\n");
        let third =
            fs::read_to_string(out.join("pkg/notices-1.6.1/THIRD_PARTY_NOTICES.txt")).unwrap();
        assert!(third.contains("==== HJViewer-1.6.1-windows-x86_64.exe ====\n\nthird-party notices of the windows build\n"));
        assert!(third.contains("==== HJViewer-1.6.1-android-arm64.apk ====\n\nthird-party notices of the android build\n"));
        assert!(!third.contains("ios"));
        // SHA256SUMS lists every other file with its hash, as sha256sum writes it
        let sums = fs::read_to_string(out.join(SHA256SUMS)).unwrap();
        assert_eq!(sums.lines().count(), 7);
        for line in sums.lines() {
            let (hash, file) = line.split_once("  ").unwrap();
            assert_eq!(
                hash,
                hex(&sha256(&fs::read(out.join(file)).unwrap())),
                "{file}"
            );
        }
        // running again changes nothing
        let before = fs::read(out.join(SHA256SUMS)).unwrap();
        publish(&policy, Some("{\"v\":1}\n"), &artifacts, &out).unwrap();
        assert_eq!(fs::read(out.join(SHA256SUMS)).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn what_does_not_fit_is_refused() {
        let (policy, artifacts, root) = fixture("publish-bad");
        // the policy names another hash than the package has
        let mut other = policy.clone();
        other
            .platforms
            .get_mut(&Platform::Windows)
            .unwrap()
            .package
            .as_mut()
            .unwrap()
            .sha256 = "0".repeat(64);
        assert!(publish(&other, None, &artifacts, &root.join("a"))
            .unwrap_err()
            .contains("정책의 해시와 다릅니다"));
        // a file of another policy is in the directory already
        let out = root.join("b");
        fs::create_dir_all(out.join("pkg")).unwrap();
        fs::write(out.join("pkg/HJViewer-1.6.0-windows-x86_64.exe"), b"old").unwrap();
        assert!(publish(&policy, None, &artifacts, &out)
            .unwrap_err()
            .contains("관계없는 파일"));
        // a package with the right name and other content lies there
        let out = root.join("c");
        fs::create_dir_all(out.join("pkg")).unwrap();
        fs::write(
            out.join("pkg/HJViewer-1.6.1-windows-x86_64.exe"),
            b"windows package 1.6.X",
        )
        .unwrap();
        assert!(publish(&policy, None, &artifacts, &out)
            .unwrap_err()
            .contains("복사한 파일이 정책과 다릅니다"));
        // another envelope than the one already placed
        let out = root.join("d");
        publish(&policy, Some("{\"v\":1}\n"), &artifacts, &out).unwrap();
        assert!(publish(&policy, Some("{\"v\":2}\n"), &artifacts, &out)
            .unwrap_err()
            .contains("다른 내용의 파일"));
        // notices that differ between the platforms of one version
        let out = root.join("e");
        fs::write(artifacts[0].dir.join("NOTICE.txt"), b"another notice\n").unwrap();
        assert!(publish(&policy, None, &artifacts, &out)
            .unwrap_err()
            .contains("다른 플랫폼"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_package_that_is_already_on_the_server() {
        let (mut policy, artifacts, root) = fixture("publish-part");
        // Android stays at 1.6.0, whose package is not in this release directory
        let android = policy.platforms.get_mut(&Platform::Android).unwrap();
        android.latest = Version::parse("1.6.0").unwrap();
        android.min = android.latest;
        android.package.as_mut().unwrap().file = "HJViewer-1.6.0-android-arm64.apk".to_owned();
        let out = root.join("out");
        let report = publish(&policy, None, &artifacts, &out).unwrap();
        assert!(report
            .iter()
            .any(|l| l.contains("android 1.6.0: 릴리스 디렉터리에 없어")));
        let files = files_under(&out).unwrap();
        assert!(files.contains(&"pkg/HJViewer-1.6.1-windows-x86_64.exe".to_owned()));
        assert!(!files.iter().any(|f| f.contains("android")));
        let third =
            fs::read_to_string(out.join("pkg/notices-1.6.1/THIRD_PARTY_NOTICES.txt")).unwrap();
        assert!(third.contains("windows") && !third.contains("android"));
        fs::remove_dir_all(root).unwrap();
    }
}
