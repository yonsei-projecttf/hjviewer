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

/// The record as the build repository has it, or none when it has no such file yet.
pub fn fetch(settings: &Settings, tools: &dyn Tools) -> Result<Option<String>, String> {
    let path = format!("repos/{}/contents/{RECORD_PATH}", settings.build_repo);
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
        // no record yet: nothing was released through the store run
        Err(e) if e.contains("HTTP 404") => Ok(None),
        Err(e) => Err(fail(e)),
    }
}

/// The highest version an `appstore … release` line of the record names.
pub fn appstore_released(record: &str) -> Result<Option<Version>, String> {
    let mut highest: Option<Version> = None;
    for (n, line) in record.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() || fields[0].starts_with('#') || fields[0] != "appstore" {
            continue;
        }
        let bad = || format!("출시 기록 {RECORD_PATH}의 {}번째 줄을 읽지 못합니다", n + 1);
        if fields.len() < 4 {
            return Err(bad());
        }
        if fields[3] != "release" {
            continue;
        }
        let version = Version::parse(fields[1]).map_err(|_| bad())?;
        highest = highest.max(Some(version));
    }
    Ok(highest)
}

/// The App Store version the record says was released, read through `gh`.
pub fn appstore_release(settings: &Settings, tools: &dyn Tools) -> Result<Option<Version>, String> {
    match fetch(settings, tools)? {
        Some(record) => appstore_released(&record),
        None => Ok(None),
    }
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
        // a line of the App Store the tool cannot read is not passed over
        for bad in [
            "appstore 1.9.1 263120249\n",
            "appstore 1.9 263120249 release -\n",
        ] {
            assert!(
                appstore_released(bad).unwrap_err().contains("1번째 줄"),
                "{bad}"
            );
        }
    }
}
