// SPDX-License-Identifier: AGPL-3.0-only

//! What a build left behind: the artifact directories under `build/release-<version>/`, each
//! with its `release.json` (written by CI, design 4-5 and 6-4), and the package files in them.
//!
//! ```json
//! {"platform":"windows","version":"1.6.0","build":"1.6.0.37","file":"HJViewer.exe",
//!  "size":25329664,"sha256":"<64 hex>","source_sha":"<40 hex>","run_id":"38106290530",
//!  "workflow":".github/workflows/build.yml"}
//! ```
//!
//! `release.json` is a claim, not proof: the tool hashes the file itself, and provenance.rs asks
//! GitHub about the run. `build` and `run_id` may be JSON strings or numbers; fields the tool does
//! not know are ignored, since CI may add some.

use crate::{
    json::Json,
    policy::{check_sha256, Platform},
    trust::hex,
    version::Version,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub const RELEASE_JSON: &str = "release.json";
pub const SHA256SUMS: &str = "SHA256SUMS";
/// The notice files CI puts next to every package (AGPL: licence, notices, where the source is).
pub const NOTICE_FILES: [&str; 4] = [
    "LICENSE.txt",
    "NOTICE.txt",
    "SOURCE.txt",
    "THIRD_PARTY_NOTICES.txt",
];

/// The platforms CI builds. Google Play gets the Android build's bundle and has no artifact here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Built {
    Windows,
    Android,
    Ios,
}

impl Built {
    pub fn name(self) -> &'static str {
        match self {
            Built::Windows => "windows",
            Built::Android => "android",
            Built::Ios => "ios",
        }
    }

    pub fn platform(self) -> Platform {
        match self {
            Built::Windows => Platform::Windows,
            Built::Android => Platform::Android,
            Built::Ios => Platform::Ios,
        }
    }
}

/// `release.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub platform: Built,
    pub version: Version,
    /// The Windows file version, the Android versionCode or the iOS CFBundleVersion.
    pub build: String,
    /// The package's name inside the artifact directory.
    pub file: String,
    pub size: u64,
    pub sha256: String,
    /// The commit of the public repository that was built.
    pub source_sha: String,
    /// The run of the build repository.
    pub run_id: String,
    /// The workflow of the build repository the run belongs to.
    pub workflow: String,
}

/// A package on disk whose size and hash were found to match its `release.json`.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub dir: PathBuf,
    pub info: ReleaseInfo,
}

impl Artifact {
    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.info.file)
    }
}

pub fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn digits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
}

impl ReleaseInfo {
    pub fn parse(text: &str) -> Result<ReleaseInfo, String> {
        let value = Json::parse(text).map_err(|e| format!("JSON이 아닙니다({e})"))?;
        let mut f = value.fields(RELEASE_JSON)?;
        // a string, or a number where CI wrote one
        let mut text_or_number = |name: &str| -> Result<String, String> {
            match f.take(name) {
                Some(Json::Str(s)) => Ok(s.clone()),
                Some(Json::Int(n)) => Ok(n.to_string()),
                _ => Err(format!("{name} 필드가 없거나 문자열·숫자가 아닙니다")),
            }
        };
        let build = text_or_number("build")?;
        let run_id = text_or_number("run_id")?;
        let platform = match f.str("platform")? {
            "windows" => Built::Windows,
            "android" => Built::Android,
            "ios" => Built::Ios,
            other => return Err(format!("모르는 platform {other:?}(windows, android, ios)")),
        };
        let info = ReleaseInfo {
            platform,
            version: Version::parse(f.str("version")?)?,
            build,
            file: f.str("file")?.to_owned(),
            size: f.int("size")?,
            sha256: f.str("sha256")?.to_owned(),
            source_sha: f.str("source_sha")?.to_owned(),
            run_id,
            workflow: f.str("workflow")?.to_owned(),
        };
        check_sha256(&info.sha256)?;
        if !is_commit(&info.source_sha) {
            return Err(format!(
                "source_sha는 40자리 커밋이어야 합니다: {:?}",
                info.source_sha
            ));
        }
        if !digits(&info.run_id, 20) {
            return Err(format!("run_id는 숫자여야 합니다: {:?}", info.run_id));
        }
        let b = info.build.as_bytes();
        if b.is_empty() || b.len() > 32 || !b.iter().all(|c| c.is_ascii_digit() || *c == b'.') {
            return Err(format!("build는 숫자와 점이어야 합니다: {:?}", info.build));
        }
        let name_ok = !info.file.is_empty()
            && info.file.len() <= 200
            && !info.file.starts_with('.')
            && info
                .file
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !name_ok {
            return Err(format!(
                "file은 디렉터리 없는 파일 이름이어야 합니다: {:?}",
                info.file
            ));
        }
        if info.workflow.is_empty()
            || info.workflow.len() > 300
            || info.workflow.chars().any(char::is_control)
        {
            return Err("workflow 필드가 맞지 않습니다".to_owned());
        }
        Ok(info)
    }
}

/// The size and the SHA-256 (lowercase hex) of a file, read in pieces.
pub fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("{}: 열 수 없습니다({e})", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: 읽을 수 없습니다({e})", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

/// The hash `SHA256SUMS` (the output of sha256sum) lists for `name`, if any.
pub fn listed_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(' ')?;
        // "<hash>  <name>" in text mode, "<hash> *<name>" in binary mode
        let file = file.strip_prefix(' ').or_else(|| file.strip_prefix('*'))?;
        (file == name || file.strip_prefix("./") == Some(name)).then(|| hash.to_owned())
    })
}

fn load(dir: &Path) -> Result<Artifact, String> {
    let at = |e: String| format!("{}: {e}", dir.join(RELEASE_JSON).display());
    let text = fs::read_to_string(dir.join(RELEASE_JSON)).map_err(|e| at(e.to_string()))?;
    let info = ReleaseInfo::parse(&text).map_err(at)?;
    let artifact = Artifact {
        dir: dir.to_owned(),
        info,
    };
    let path = artifact.path();
    let (size, sha256) = hash_file(&path)?;
    if size != artifact.info.size || sha256 != artifact.info.sha256 {
        return Err(format!(
            "{}: 파일이 release.json과 다릅니다(크기 {size}, sha256 {sha256})",
            path.display()
        ));
    }
    if let Ok(sums) = fs::read_to_string(dir.join(SHA256SUMS)) {
        match listed_hash(&sums, &artifact.info.file) {
            Some(listed) if listed == sha256 => {}
            Some(_) => {
                return Err(format!(
                    "{}: SHA256SUMS의 해시가 파일과 다릅니다",
                    path.display()
                ))
            }
            None => {
                return Err(format!(
                    "{}: SHA256SUMS에 이 파일의 줄이 없습니다({})",
                    dir.display(),
                    artifact.info.file
                ))
            }
        }
    }
    Ok(artifact)
}

/// Finds the artifacts of `dirs`: a directory with a `release.json`, or one whose direct
/// subdirectories have them (how `gh run download` unpacks a run). Every package is hashed and
/// compared with its `release.json`; a platform may appear once.
pub fn scan(dirs: &[PathBuf]) -> Result<Vec<Artifact>, String> {
    let mut found: Vec<Artifact> = Vec::new();
    for dir in dirs {
        let mut candidates = Vec::new();
        if dir.join(RELEASE_JSON).is_file() {
            candidates.push(dir.clone());
        } else {
            let entries = fs::read_dir(dir)
                .map_err(|e| format!("{}: 읽을 수 없습니다({e})", dir.display()))?;
            for entry in entries {
                let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
                if path.join(RELEASE_JSON).is_file() {
                    candidates.push(path);
                }
            }
            candidates.sort();
        }
        if candidates.is_empty() {
            return Err(format!(
                "{}: release.json이 있는 디렉터리가 없습니다(1.6.0부터 CI가 산출물마다 넣습니다)",
                dir.display()
            ));
        }
        for candidate in candidates {
            let artifact = load(&candidate)?;
            if let Some(other) = found
                .iter()
                .find(|a| a.info.platform == artifact.info.platform)
            {
                return Err(format!(
                    "{} 산출물이 둘입니다: {}, {}",
                    artifact.info.platform.name(),
                    other.dir.display(),
                    artifact.dir.display()
                ));
            }
            found.push(artifact);
        }
    }
    Ok(found)
}

/// The name a package has in the server's package directory (design 4-3). hjv-api gives
/// browsers other names; this one neither ends in `install.exe` nor holds `-qs-`.
pub fn server_file_name(platform: Built, version: Version) -> Option<String> {
    match platform {
        Built::Windows => Some(format!("HJViewer-{version}-windows-x86_64.exe")),
        Built::Android => Some(format!("HJViewer-{version}-android-arm64.apk")),
        Built::Ios => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const SOURCE_SHA: &str = "1111111111111111111111111111111111111111";

    pub(crate) fn release_json(
        platform: &str,
        version: &str,
        build: &str,
        file: &str,
        content: &[u8],
    ) -> String {
        format!(
            r#"{{"platform":"{platform}","version":"{version}","build":"{build}","file":"{file}","size":{},"sha256":"{}","source_sha":"{SOURCE_SHA}","run_id":"4242","workflow":".github/workflows/build.yml"}}"#,
            content.len(),
            hex(&Sha256::digest(content))
        )
    }

    /// Writes an artifact directory as CI's upload looks after a download.
    pub(crate) fn write_artifact(
        root: &Path,
        platform: &str,
        version: &str,
        build: &str,
        content: &[u8],
    ) -> PathBuf {
        let (dir, file) = match platform {
            "windows" => (
                format!("HJViewer-{version}-1111111-windows-x86_64"),
                "HJViewer.exe".to_owned(),
            ),
            "android" => (
                format!("HJViewer-{version}-1111111-android-aarch64"),
                format!("HJViewer-{version}-1111111-android-aarch64-signed.apk"),
            ),
            _ => (
                format!("HJViewer-{version}-1111111-ios"),
                format!("HJViewer-{version}-1111111-ios.ipa"),
            ),
        };
        let dir = root.join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(&file), content).unwrap();
        fs::write(
            dir.join(RELEASE_JSON),
            release_json(platform, version, build, &file, content),
        )
        .unwrap();
        for notice in NOTICE_FILES {
            let text = match notice {
                "THIRD_PARTY_NOTICES.txt" => {
                    format!("third-party notices of the {platform} build\n")
                }
                _ => format!("{notice} of HJViewer {version}\n"),
            };
            fs::write(dir.join(notice), text).unwrap();
        }
        dir
    }

    pub(crate) fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hjv-release-test-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn release_json_fields() {
        let text = release_json("windows", "1.6.0", "1.6.0.37", "HJViewer.exe", b"exe");
        let info = ReleaseInfo::parse(&text).unwrap();
        assert_eq!(info.platform, Built::Windows);
        assert_eq!(info.version.to_string(), "1.6.0");
        assert_eq!((info.size, info.run_id.as_str()), (3, "4242"));
        // numbers where CI wrote numbers, and fields the tool does not know
        let numbers = text
            .replacen(r#""run_id":"4242""#, r#""run_id":4242,"attempt":2"#, 1)
            .replacen(r#""build":"1.6.0.37""#, r#""build":263122251"#, 1);
        let info = ReleaseInfo::parse(&numbers).unwrap();
        assert_eq!(
            (info.run_id.as_str(), info.build.as_str()),
            ("4242", "263122251")
        );
        for (from, to) in [
            (r#""platform":"windows""#, r#""platform":"linux""#),
            (r#""version":"1.6.0""#, r#""version":"1.6""#),
            (r#""file":"HJViewer.exe""#, r#""file":"../HJViewer.exe""#),
            (r#""file":"HJViewer.exe""#, r#""file":"sub/HJViewer.exe""#),
            (r#""file":"HJViewer.exe""#, r#""file":".hidden""#),
            (r#""size":3"#, r#""size":"3""#),
            (r#""run_id":"4242""#, r#""run_id":"4242/../1""#),
            (r#""run_id":"4242""#, r#""run_id":"""#),
            (r#""build":"1.6.0.37""#, r#""build":"1.6.0-37""#),
            (SOURCE_SHA, "1111111"),
            (SOURCE_SHA, "111111111111111111111111111111111111111G"),
            (r#","workflow":".github/workflows/build.yml""#, ""),
        ] {
            assert!(text.contains(from), "{from}");
            assert!(
                ReleaseInfo::parse(&text.replacen(from, to, 1)).is_err(),
                "{to} parsed"
            );
        }
    }

    #[test]
    fn scanning() {
        let root = temp_dir("scan");
        let win = write_artifact(&root, "windows", "1.6.0", "1.6.0.37", b"windows package");
        write_artifact(&root, "android", "1.6.0", "263122251", b"android package");
        fs::create_dir_all(root.join("bridge-artifact")).unwrap();
        let found = scan(std::slice::from_ref(&root)).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].info.platform, Built::Android);
        assert!(found[1].path().ends_with("HJViewer.exe"));
        // one artifact directory by itself
        assert_eq!(scan(std::slice::from_ref(&win)).unwrap().len(), 1);
        // the same platform twice
        assert!(scan(&[root.clone(), win.clone()])
            .unwrap_err()
            .contains("둘"));
        // a directory without artifacts
        assert!(scan(&[root.join("bridge-artifact")])
            .unwrap_err()
            .contains("release.json"));
        assert!(scan(&[root.join("missing")]).is_err());

        // SHA256SUMS, when CI wrote one, must agree
        let sum = hex(&Sha256::digest(b"windows package"));
        fs::write(win.join(SHA256SUMS), format!("{sum}  HJViewer.exe\n")).unwrap();
        assert!(scan(std::slice::from_ref(&win)).is_ok());
        fs::write(win.join(SHA256SUMS), format!("{sum} *HJViewer.exe\n")).unwrap();
        assert!(scan(std::slice::from_ref(&win)).is_ok());
        fs::write(
            win.join(SHA256SUMS),
            format!("{}  HJViewer.exe\n", "0".repeat(64)),
        )
        .unwrap();
        assert!(scan(std::slice::from_ref(&win))
            .unwrap_err()
            .contains("SHA256SUMS"));
        fs::write(win.join(SHA256SUMS), format!("{sum}  other.exe\n")).unwrap();
        assert!(scan(std::slice::from_ref(&win))
            .unwrap_err()
            .contains("SHA256SUMS"));
        fs::remove_file(win.join(SHA256SUMS)).unwrap();

        // a changed package: same size, other bytes; then another size
        fs::write(win.join("HJViewer.exe"), b"windows packagE").unwrap();
        assert!(scan(std::slice::from_ref(&win))
            .unwrap_err()
            .contains("다릅니다"));
        fs::write(win.join("HJViewer.exe"), b"windows package and more").unwrap();
        assert!(scan(std::slice::from_ref(&win))
            .unwrap_err()
            .contains("다릅니다"));
        fs::remove_file(win.join("HJViewer.exe")).unwrap();
        assert!(scan(&[win]).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_and_hashes() {
        let v = Version::parse("1.6.1").unwrap();
        assert_eq!(
            server_file_name(Built::Windows, v).unwrap(),
            "HJViewer-1.6.1-windows-x86_64.exe"
        );
        assert_eq!(
            server_file_name(Built::Android, v).unwrap(),
            "HJViewer-1.6.1-android-arm64.apk"
        );
        assert_eq!(server_file_name(Built::Ios, v), None);
        for built in [Built::Windows, Built::Android] {
            let name = server_file_name(built, v).unwrap();
            crate::policy::check_package_file(built.platform(), &name).unwrap();
        }
        assert!(is_commit(SOURCE_SHA));
        assert!(!is_commit("1111111") && !is_commit(&"A".repeat(40)));
        let sums = "aa  one\nbb *two\ncc  ./three\n";
        assert_eq!(listed_hash(sums, "one").unwrap(), "aa");
        assert_eq!(listed_hash(sums, "two").unwrap(), "bb");
        assert_eq!(listed_hash(sums, "three").unwrap(), "cc");
        assert_eq!(listed_hash(sums, "four"), None);
        // sha256sum of the three bytes "abc"
        let dir = temp_dir("hash");
        fs::write(dir.join("f"), b"abc").unwrap();
        assert_eq!(
            hash_file(&dir.join("f")).unwrap(),
            (
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned()
            )
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
