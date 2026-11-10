// SPDX-License-Identifier: AGPL-3.0-only

//! The operator's files that end (1.9.3): the App Review account's (HJV_API_REVIEW_FILE,
//! review.rs), the access password's (HJV_API_GUEST_FILE, guest.rs) and door 2's
//! (HJV_API_DOOR2_FILE, update.rs). Each carries an `UNTIL=`, at most 90 days ahead by the tools
//! that write it, and past it each is a filter per request that tells nobody: the review login
//! fails, the access password is "closed" and counted like a wrong one, door 2's link is 404. A
//! file that does not parse is treated as absent, with one err line in the journal when a request
//! first reads it.
//!
//! So `health` (check.rs) carries each configured file's state, [`OpFile`], and the five-minute
//! check finds a file that ends within [`ENDING_SOON`] (`file_ending`), one whose `UNTIL` has
//! passed while it is still installed (`file_ended`) and one that is refused (`file_refused`).
//! `admin report` prints the three with their end and the days left, and the start logs door 2's
//! state as review.rs and guest.rs log theirs. No line names more than the file and its time: no
//! ID, code, hash or address.

use crate::{
    check::Finding,
    state::AppState,
    update::Seen,
    util::{datetime, DAY},
};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

/// A file that ends within this long is a finding.
pub const ENDING_SOON: i64 = 7 * DAY;

pub const ABSENT: &str = "absent";
pub const ACTIVE: &str = "active";
pub const ENDED: &str = "ended";
pub const REFUSED: &str = "refused";

/// A configured file's state as `health` says it: its name, state and end, and nothing of what
/// it holds, so that no finding or line made of it can show an ID, a code or a hash.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct OpFile {
    /// "review", "guest" or "door2"
    pub name: String,
    /// [`ABSENT`], [`ACTIVE`] (until `until`), [`ENDED`] (at `until`, and still there) or
    /// [`REFUSED`] (there, but it cannot be reached or read, is no file or does not parse)
    pub state: String,
    /// its UNTIL, when it is active or ended
    #[serde(default)]
    pub until: Option<i64>,
}

impl OpFile {
    /// The state at `at` of what `watched` saw of the file `name`.
    pub(crate) fn of<T>(name: &str, seen: Seen<T>, until: impl Fn(&T) -> i64, at: i64) -> OpFile {
        let (state, until) = match seen {
            Seen::Absent => (ABSENT, None),
            Seen::Refused => (REFUSED, None),
            Seen::Taken(value) => {
                let until = until(&value);
                (if at < until { ACTIVE } else { ENDED }, Some(until))
            }
        };
        OpFile {
            name: name.to_owned(),
            state: state.to_owned(),
            until,
        }
    }
}

/// What a file is, for the lines about it.
struct Kind {
    name: &'static str,
    setting: &'static str,
    what: &'static str,
    /// what goes wrong once it has ended or while it is refused
    without: &'static str,
}

const KINDS: [Kind; 3] = [
    Kind {
        name: "review",
        setting: "HJV_API_REVIEW_FILE",
        what: "the App Review account's file",
        without: "the App Review account cannot log in, and its logins end",
    },
    Kind {
        name: "guest",
        setting: "HJV_API_GUEST_FILE",
        what: "the access password's file",
        without: "every try of the access password is answered as closed and counted against its address",
    },
    Kind {
        name: "door2",
        setting: "HJV_API_DOOR2_FILE",
        what: "door 2's file",
        without: "door 2's link answers 404, also where hbbs's text below the minimum version still carries it",
    },
];

fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name == name)
}

/// The configured files' states at `at`, in the order review, guest, door 2 (it reads files: not
/// for an async thread).
pub fn states(state: &AppState, at: i64) -> Vec<OpFile> {
    let mut files = Vec::new();
    if state.config.review_file.is_some() {
        files.push(state.review.state(&state.config, &state.tags, at));
    }
    if state.config.guest_file.is_some() {
        files.push(state.guest.state(&state.config, at));
    }
    files.push(state.updates.door2_state(&state.config, &state.tags, at));
    files
}

/// "75 days left", "less than a day left".
fn left(until: i64, at: i64) -> String {
    match (until - at) / DAY {
        0 => "less than a day left".to_owned(),
        1 => "1 day left".to_owned(),
        n => format!("{n} days left"),
    }
}

/// "less than a day ago", "3 days ago".
fn ago(until: i64, at: i64) -> String {
    match (at - until) / DAY {
        0 => "less than a day ago".to_owned(),
        1 => "1 day ago".to_owned(),
        n => format!("{n} days ago"),
    }
}

/// The check's findings about the files of `health` at `at`; a state it does not know (a newer
/// service's) is none.
pub fn findings(files: &[OpFile], at: i64) -> Vec<Finding> {
    let mut found = Vec::new();
    for file in files {
        let Some(k) = kind(&file.name) else {
            continue;
        };
        let (what, setting, without) = (k.what, k.setting, k.without);
        match (file.state.as_str(), file.until) {
            (ACTIVE, Some(until)) if until - at <= ENDING_SOON => found.push(Finding {
                code: "file_ending",
                text: format!(
                    "{what} ({setting}) ends at {}, {}: then {without}; put in a new file with a later UNTIL before then, or delete it once it has ended",
                    datetime(until),
                    left(until, at)
                ),
            }),
            (ENDED, Some(until)) => found.push(Finding {
                code: "file_ended",
                text: format!(
                    "{what} ({setting}) ended at {} ({}) and is still installed: {without}; put in a new file or delete it",
                    datetime(until),
                    ago(until, at)
                ),
            }),
            (REFUSED, _) => found.push(Finding {
                code: "file_refused",
                text: format!(
                    "{what} ({setting}) is there but not used: it cannot be read or does not parse (the journal's err line says why), so {without}; correct the file or delete it"
                ),
            }),
            _ => {}
        }
    }
    found
}

/// `admin report`'s lines: each of the three files, whether it is set, its state, its end and the
/// days left (it reads files: not for an async thread).
pub fn report(state: &AppState, at: i64) -> String {
    let files = states(state, at);
    let mut out = String::new();
    for k in &KINDS {
        let file = files.iter().find(|f| f.name == k.name);
        let text = match file.map(|f| (f.state.as_str(), f.until)) {
            None => "not set".to_owned(),
            Some((ACTIVE, Some(until))) => {
                format!("active until {}, {}", datetime(until), left(until, at))
            }
            Some((ENDED, Some(until))) => format!(
                "ended at {}, {}, still installed",
                datetime(until),
                ago(until, at)
            ),
            Some((REFUSED, _)) => "there but not used (the journal says why)".to_owned(),
            Some(_) => "not there".to_owned(),
        };
        let _ = writeln!(out, "operator file {} ({}): {text}", k.name, k.setting);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_794_268_800; // 2026-11-10T00:00:00Z

    fn file(name: &str, state: &str, until: Option<i64>) -> OpFile {
        OpFile {
            name: name.into(),
            state: state.into(),
            until,
        }
    }

    fn codes(found: &[Finding]) -> Vec<&'static str> {
        found.iter().map(|f| f.code).collect()
    }

    #[test]
    fn a_file_that_ends_within_a_week_has_ended_or_is_refused_is_a_finding() {
        let ends = |secs| file("guest", ACTIVE, Some(AT + secs));
        assert!(findings(&[ends(ENDING_SOON + 1)], AT).is_empty());
        let found = findings(&[ends(ENDING_SOON)], AT);
        assert_eq!(codes(&found), ["file_ending"]);
        assert_eq!(
            found[0].text,
            "the access password's file (HJV_API_GUEST_FILE) ends at 2026-11-17T00:00:00Z, 7 days left: then every try of the access password is answered as closed and counted against its address; put in a new file with a later UNTIL before then, or delete it once it has ended"
        );
        assert!(findings(&[ends(DAY + 5)], AT)[0]
            .text
            .contains(", 1 day left: "));
        assert!(findings(&[ends(60)], AT)[0]
            .text
            .contains(", less than a day left: "));
        let found = findings(&[file("door2", ENDED, Some(AT - 3 * DAY - 7))], AT);
        assert_eq!(codes(&found), ["file_ended"]);
        assert_eq!(
            found[0].text,
            "door 2's file (HJV_API_DOOR2_FILE) ended at 2026-11-06T23:59:53Z (3 days ago) and is still installed: door 2's link answers 404, also where hbbs's text below the minimum version still carries it; put in a new file or delete it"
        );
        // ended 23 hours ago: the day before by the calendar, so not "today"
        assert!(
            findings(&[file("door2", ENDED, Some(AT - DAY + 3600))], AT)[0]
                .text
                .contains(" (less than a day ago) ")
        );
        let found = findings(&[file("review", REFUSED, None)], AT);
        assert_eq!(codes(&found), ["file_refused"]);
        assert!(found[0].text.starts_with(
            "the App Review account's file (HJV_API_REVIEW_FILE) is there but not used: "
        ));
        // a file that is not there is no finding: not having one is the operator's choice
        assert!(findings(&[file("door2", ABSENT, None)], AT).is_empty());
        // each file is a line of its own, in the order health gives them
        let all = [
            file("review", ACTIVE, Some(AT + 75 * DAY)),
            file("guest", ACTIVE, Some(AT + 2 * DAY)),
            file("door2", ENDED, Some(AT - 1)),
        ];
        assert_eq!(codes(&findings(&all, AT)), ["file_ending", "file_ended"]);
        // what a newer service may say is no finding here
        let newer = [
            file("door3", REFUSED, None),
            file("guest", "paused", Some(AT)),
            file("guest", ACTIVE, None),
        ];
        assert!(findings(&newer, AT).is_empty());
    }

    #[test]
    fn the_state_a_service_gives_reads_in_a_check_of_another_version() {
        let json = serde_json::to_value(file("door2", ACTIVE, Some(AT))).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"name": "door2", "state": "active", "until": AT})
        );
        let bare: OpFile =
            serde_json::from_value(serde_json::json!({"name": "guest", "state": "absent"}))
                .unwrap();
        assert_eq!(bare, file("guest", ABSENT, None));
        let more: OpFile = serde_json::from_value(
            serde_json::json!({"name": "guest", "state": "absent", "why": "later"}),
        )
        .unwrap();
        assert_eq!(more, file("guest", ABSENT, None));
    }
}
