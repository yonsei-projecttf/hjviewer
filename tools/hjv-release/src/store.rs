// SPDX-License-Identifier: AGPL-3.0-only

//! The stores' side of a policy (auto-deploy design 3-7, 3-8): the Google Play track the store
//! run uploads to, and the release record `store-releases.txt` of the build repository.
//!
//! The record has one line per store action, never changed or removed:
//!
//! ```text
//! # store version build action track source-commit date(UTC) note
//! play 1.9.1 263122249 upload alpha <commit> 2026-10-28 store #12 build-run 4242
//! appstore 1.9.1 263120249 submit - <commit> 2026-10-28 store #13
//! appstore 1.9.1 263120249 release - <commit> 2026-11-02 store #15
//! ```
//!
//! `policy` reads it only for T3: an `appstore … release` line says the App Store has handed out
//! a version, so a policy that changes the iOS entry with `--no-appstore` (without naming the App
//! Store version) is probably a mistake of the kind seq 5 made with `android_play`. The draft
//! says so, and `sign` asks for a typed `yes`. The tool never asks the stores themselves: what
//! the operator confirms (`--play-live`, `--appstore-live`) comes from the store run's `status`.

use crate::{
    provenance::{Settings, Tools},
    version::Version,
};

/// The closed testing track the store run uploads to (owner's decision of 2026-10-26: the
/// default closed track Alpha, API name `alpha`). `--play-live` confirms a release there.
pub const PLAY_TRACK: &str = "alpha";

/// Where the release record is in the build repository (its default branch).
pub const RECORD_PATH: &str = "store-releases.txt";

/// The largest record the tool reads.
const MAX_RECORD_BYTES: usize = 1024 * 1024;

/// The record as the build repository has it, or none when the repository has no such file yet.
///
/// GitHub answers 404 for a missing file, but also for a repository the token cannot see or
/// that does not exist. A 404 is therefore no record only when the repository itself answers:
/// otherwise a wrong `--build-repo` or a token without access would switch T3's question off.
pub fn fetch(settings: &Settings, tools: &dyn Tools) -> Result<Option<String>, String> {
    settings.check()?;
    let repo = format!("repos/{}", settings.build_repo);
    let path = format!("{repo}/contents/{RECORD_PATH}");
    let fail = |e: String| {
        format!(
            "출시 기록({}의 {RECORD_PATH})을 읽지 못했습니다: {e}",
            settings.build_repo
        )
    };
    match tools.gh(&[
        "api",
        "-H",
        "Accept: application/vnd.github.raw+json",
        &path,
    ]) {
        Ok(bytes) if bytes.len() > MAX_RECORD_BYTES => Err(fail("파일이 너무 큽니다".to_owned())),
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| fail("UTF-8 텍스트가 아닙니다".to_owned())),
        // no record yet, if the repository is there for this token
        Err(e) if e.contains("HTTP 404") => match tools.gh(&["api", &repo]) {
            Ok(_) => Ok(None),
            Err(r) => Err(fail(format!(
                "{e}. 저장소도 보이지 않습니다({r}). --build-repo와 gh의 권한을 확인하세요"
            ))),
        },
        Err(e) => Err(fail(e)),
    }
}

/// The stores and actions a line of the record may name (design 3-8).
const STORES: [&str; 2] = ["play", "appstore"];
const ACTIONS: [&str; 3] = ["upload", "submit", "release"];

/// The highest version an `appstore … release` line of the record names. Every other line but
/// comments and blank lines must have the record's form too: a line written by hand with a typo
/// (`Release`, `app-store`) would otherwise pass for no release.
pub fn appstore_released(record: &str) -> Result<Option<Version>, String> {
    let mut highest: Option<Version> = None;
    for (n, line) in record.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') {
            continue;
        }
        let bad = || format!("출시 기록 {RECORD_PATH}의 {}번째 줄을 읽지 못합니다", n + 1);
        let [store, version, build, action, ..] = fields[..] else {
            return Err(bad());
        };
        let version = Version::parse(version).map_err(|_| bad())?;
        if !STORES.contains(&store)
            || !ACTIONS.contains(&action)
            || build.is_empty()
            || !build.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(bad());
        }
        if store == "appstore" && action == "release" {
            highest = highest.max(Some(version));
        }
    }
    Ok(highest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::tests::v;

    #[test]
    fn the_release_lines_of_the_app_store() {
        let record = "\
# store version build action track source-commit date(UTC) note
play 1.9.0 262932441 upload alpha 3333333333333333333333333333333333333333 2026-10-21 by hand
play 1.9.2 263122249 upload alpha 1111111111111111111111111111111111111111 2026-11-01 store #3
appstore 1.9.1 263120249 submit - 1111111111111111111111111111111111111111 2026-10-28 store #4
";
        // a submission is no release; Play's lines do not count
        assert_eq!(appstore_released(record).unwrap(), None);
        let released = format!(
            "{record}appstore 1.9.1 263120249 release - 1111111111111111111111111111111111111111 2026-11-02 store #5\n"
        );
        assert_eq!(appstore_released(&released).unwrap(), Some(v("1.9.1")));
        let later = format!(
            "{released}appstore 1.9.3 263200100 release - 2222222222222222222222222222222222222222 2026-11-20 store #9\n\n"
        );
        assert_eq!(appstore_released(&later).unwrap(), Some(v("1.9.3")));
        assert_eq!(appstore_released("").unwrap(), None);
        // a line the tool cannot read is not passed over, whatever its store or action
        for bad in [
            "appstore 1.9.1 263120249\n",
            "appstore 1.9 263120249 release -\n",
            "appstore 1.9.1 263120249 Release - x 2026-11-02 store #5\n",
            "app-store 1.9.1 263120249 release - x 2026-11-02 store #5\n",
            "play 1.9.1 263122249 uploaded alpha x 2026-11-02 store #5\n",
            "appstore 1.9.1 build release - x 2026-11-02 store #5\n",
            "{\"message\":\"Not Found\"}\n",
        ] {
            assert!(
                appstore_released(bad).unwrap_err().contains("1번째 줄"),
                "{bad}"
            );
        }
    }

    /// `gh` answers from a list: (the arguments joined, the answer).
    struct Gh(Vec<(&'static str, Result<Vec<u8>, String>)>);

    impl Tools for Gh {
        fn gh(&self, args: &[&str]) -> Result<Vec<u8>, String> {
            let asked = args.join(" ");
            self.0
                .iter()
                .find(|(a, _)| *a == asked)
                .map(|(_, answer)| answer.clone())
                .unwrap_or_else(|| Err(format!("gh: HTTP 404 ({asked})")))
        }
        fn git(&self, _: &std::path::Path, _: &[&str]) -> Result<Vec<u8>, String> {
            unreachable!()
        }
    }

    const RECORD: &str = "api -H Accept: application/vnd.github.raw+json \
                          repos/yonsei-projecttf/hjviewer-private/contents/store-releases.txt";
    const REPO: &str = "api repos/yonsei-projecttf/hjviewer-private";

    #[test]
    fn reading_the_record() {
        let settings = Settings::new(".".into());
        let gh = |answers: Vec<(&'static str, Result<Vec<u8>, String>)>| Gh(answers);
        let line = "appstore 1.9.1 263120249 release - x 2026-11-02 store #5\n";
        assert_eq!(
            fetch(&settings, &gh(vec![(RECORD, Ok(line.into()))])).unwrap(),
            Some(line.to_owned())
        );
        // a 404 is no record only when the repository itself answers
        assert_eq!(
            fetch(&settings, &gh(vec![(REPO, Ok(b"{}".to_vec()))])).unwrap(),
            None
        );
        let err = fetch(&settings, &gh(Vec::new())).unwrap_err();
        assert!(err.contains("저장소도 보이지 않습니다"), "{err}");
        // any other failure, an oversized record and one that is no text are refused
        for (answer, why) in [
            (Err("gh: HTTP 500".to_owned()), "HTTP 500"),
            (Ok(vec![b'#'; MAX_RECORD_BYTES + 1]), "너무 큽니다"),
            (Ok(vec![0xff, 0xfe]), "UTF-8"),
        ] {
            let tools = gh(vec![(RECORD, answer), (REPO, Ok(Vec::new()))]);
            let err = fetch(&settings, &tools).unwrap_err();
            assert!(err.contains(why), "{err}");
        }
        // the repository name goes into the path only in the form the settings allow
        let mut odd = Settings::new(".".into());
        odd.build_repo = "owner/name/../other".to_owned();
        assert!(fetch(&odd, &gh(Vec::new()))
            .unwrap_err()
            .contains("owner/name"));
    }
}
