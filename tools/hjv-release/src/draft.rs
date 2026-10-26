// SPDX-License-Identifier: AGPL-3.0-only

//! The draft: what `policy` hands to `sign`. It holds the exact texts that will be signed and
//! what the signer needs to judge them, and no signature of its own.
//!
//! ```json
//! {"hjv_release_draft":1,"signer":"signing","p":"<policy text>",
//!  "root":{"k":101,"p":"…","s":"…"},        the last signed root declaration, attached as it is
//!  "root_p":"<root declaration text>",        or a new one, for the root key to sign
//!  "prev":"<the previous envelope file>",
//!  "appstore_release":"1.9.1",               the store record's App Store release, when the
//!                                             policy changes iOS with --no-appstore (T3)
//!  "notes":["what policy checked", …]}
//! ```
//!
//! An agent may write a draft; only a person signs it. `sign` therefore checks the draft again
//! from its texts and shows what changes, and believes nothing else in it.
//!
//! The notes are the one free text of a draft, and `sign` shows them on the signer's terminal.
//! So a note is only what `policy` itself writes: one line of printable ASCII and Korean letters
//! with single spaces, [`MAX_NOTE_CHARS`] characters at most, [`MAX_NOTES`] of them. A note
//! cannot move the cursor, start a line, hide text or pad itself into a line of its own.

use crate::{
    envelope::Part,
    json::{quote, Json, Writer},
    keys::Role,
    version::Version,
};

pub const DRAFT_VERSION: u64 = 1;
const MAX_DRAFT_BYTES: usize = 256 * 1024;
/// How many notes a draft may carry, and how long each may be.
pub const MAX_NOTES: usize = 20;
pub const MAX_NOTE_CHARS: usize = 300;

/// The rule of the notes (see the top of this file). The errors name the note by its number
/// and never repeat it: they are shown where the note itself must not be.
pub fn check_notes(notes: &[String]) -> Result<(), String> {
    if notes.len() > MAX_NOTES {
        return Err(format!(
            "초안: notes는 {MAX_NOTES}줄까지입니다({}줄)",
            notes.len()
        ));
    }
    for (n, note) in notes.iter().enumerate() {
        let at = |why: &str| format!("초안: notes의 {}번째 줄{why}", n + 1);
        let chars = note.chars().count();
        if chars == 0 || chars > MAX_NOTE_CHARS {
            return Err(at(&format!(
                "은 1자 이상 {MAX_NOTE_CHARS}자 이하여야 합니다({chars}자)"
            )));
        }
        // printable ASCII and Hangul syllables: no control character (a line feed is one), no
        // character that shows nothing or turns the text around, no other kind of space
        if !note
            .chars()
            .all(|c| matches!(c, ' '..='~' | '\u{ac00}'..='\u{d7a3}'))
        {
            return Err(at(
                "에 쓸 수 없는 문자가 있습니다(제어 문자, 줄바꿈, 보이지 않는 문자 등). \
                 policy가 쓰는 줄은 한 줄이고 한글과 ASCII 글자뿐입니다",
            ));
        }
        if note.starts_with(' ') || note.ends_with(' ') || note.contains("  ") {
            return Err(at("에 앞뒤 공백이나 잇단 공백이 있습니다"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The policy text to sign.
    pub p: String,
    /// The role of the key that must sign the policy: the root key if the policy relaxes.
    pub signer: Role,
    /// The root declaration every envelope carries from now on, already signed.
    pub root: Option<Part>,
    /// A new root declaration (a revocation, a new epoch), not signed yet.
    pub root_p: Option<String>,
    /// The envelope this policy follows, byte for byte.
    pub prev: Option<String>,
    /// The App Store version the store record says was released, when this policy changes the
    /// iOS entry without naming the App Store (`--no-appstore`, design 3-7 T3): `sign` asks for
    /// a typed `yes`. It can only make signing stricter.
    pub appstore_release: Option<String>,
    /// What `policy` checked, for the person who signs.
    pub notes: Vec<String>,
}

impl Draft {
    /// What a draft must keep whoever wrote it, beyond the rules of its texts (those are
    /// signing.rs's): the notes are fit to be shown, and there is one root declaration at most.
    pub fn check(&self) -> Result<(), String> {
        check_notes(&self.notes)?;
        if self.root.is_some() && self.root_p.is_some() {
            return Err("초안: 서명된 루트 선언과 새 루트 선언이 함께 있습니다".to_owned());
        }
        if let Some(version) = &self.appstore_release {
            Version::parse(version)
                .map_err(|_| "초안: appstore_release는 버전입니다".to_owned())?;
        }
        Ok(())
    }

    pub fn to_text(&self) -> String {
        let notes: Vec<String> = self.notes.iter().map(|n| quote(n)).collect();
        let mut w = Writer::new();
        w.int("hjv_release_draft", DRAFT_VERSION)
            .str("signer", self.signer.name())
            .str("p", &self.p);
        if let Some(root) = &self.root {
            w.raw("root", &root.to_text());
        }
        w.opt_str("root_p", self.root_p.as_deref())
            .opt_str("prev", self.prev.as_deref())
            .opt_str("appstore_release", self.appstore_release.as_deref())
            .raw("notes", &format!("[{}]", notes.join(",")));
        let mut text = w.finish();
        text.push('\n');
        text
    }

    pub fn parse(text: &str) -> Result<Draft, String> {
        if text.len() > MAX_DRAFT_BYTES {
            return Err("초안 파일이 너무 큽니다".to_owned());
        }
        let value = Json::parse(text).map_err(|e| format!("초안이 JSON이 아닙니다({e})"))?;
        let mut f = value.fields("초안")?;
        match f.take("hjv_release_draft") {
            Some(Json::Int(DRAFT_VERSION)) => {}
            Some(_) => return Err("이 도구가 모르는 초안 버전입니다".to_owned()),
            None => {
                return Err(
                    "초안 파일이 아닙니다(hjv-release policy가 만든 파일을 주세요)".to_owned(),
                )
            }
        }
        let signer =
            Role::parse(f.str("signer")?).ok_or("초안: signer는 signing 또는 root입니다")?;
        let p = f.str("p")?.to_owned();
        let root = f
            .take("root")
            .map(|root| match root {
                Json::Object(_) => Part::parse(&json_text(root), "초안의 루트 선언"),
                _ => Err("초안: root는 객체여야 합니다".to_owned()),
            })
            .transpose()?;
        let root_p = f.opt_str("root_p")?.map(str::to_owned);
        let prev = f.opt_str("prev")?.map(str::to_owned);
        let appstore_release = f.opt_str("appstore_release")?.map(str::to_owned);
        let notes = match f.take("notes") {
            Some(Json::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Json::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<String>>>()
                .ok_or("초안: notes는 문자열 목록입니다")?,
            _ => return Err("초안: notes 목록이 없습니다".to_owned()),
        };
        f.done()?;
        let draft = Draft {
            p,
            signer,
            root,
            root_p,
            prev,
            appstore_release,
            notes,
        };
        draft.check()?;
        Ok(draft)
    }
}

/// Writes a parsed value back as JSON text (for a nested object read again by another parser).
fn json_text(value: &Json) -> String {
    match value {
        Json::Null => "null".to_owned(),
        Json::Bool(b) => b.to_string(),
        Json::Int(n) => n.to_string(),
        Json::Str(s) => quote(s),
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(json_text).collect();
            format!("[{}]", items.join(","))
        }
        Json::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{}:{}", quote(k), json_text(v)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{envelope::ROOT_DOMAIN, keys::test_key};
    use ed25519_dalek::SigningKey;

    #[test]
    fn round_trip() {
        let root_key = SigningKey::from_bytes(&test_key("root-101").unwrap().seed());
        let plain = Draft {
            p: "{\"v\":1,\"text\":\"한글 \\\"따옴표\\\"\"}".to_owned(),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
            appstore_release: None,
            notes: Vec::new(),
        };
        let text = plain.to_text();
        assert!(text.starts_with("{\"hjv_release_draft\":1,\"signer\":\"signing\",\"p\":\""));
        assert!(text.ends_with(",\"notes\":[]}\n"));
        assert_eq!(Draft::parse(&text).unwrap(), plain);

        let full = Draft {
            signer: Role::Root,
            root: Some(Part::sign(&root_key, 101, ROOT_DOMAIN, "{\"v\":1}")),
            prev: Some("{\"v\":1,\"k\":1}\n".to_owned()),
            notes: vec![
                "windows 1.6.1: 실행 4242 성공(.github/workflows/build.yml @ 222222222222)"
                    .to_owned(),
                "ios 1.6.1: 운영자가 TestFlight 빌드(263120249)를 확인함(--ios-valid, 올린 지 0일)"
                    .to_owned(),
            ],
            ..plain.clone()
        };
        assert_eq!(Draft::parse(&full.to_text()).unwrap(), full);
        let new_root = Draft {
            root_p: Some("{\"v\":1,\"epoch\":2}".to_owned()),
            ..plain.clone()
        };
        assert_eq!(Draft::parse(&new_root.to_text()).unwrap(), new_root);
        // the store record's App Store release (T3), a version only
        let released = Draft {
            appstore_release: Some("1.9.1".to_owned()),
            ..plain.clone()
        };
        let text = released.to_text();
        assert!(text.contains(r#","appstore_release":"1.9.1","notes":[]"#));
        assert_eq!(Draft::parse(&text).unwrap(), released);
        assert!(Draft::parse(&text.replace("1.9.1", "1.9"))
            .unwrap_err()
            .contains("appstore_release"));
    }

    #[test]
    fn refuses_other_files() {
        let good = Draft {
            p: "{}".to_owned(),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
            appstore_release: None,
            notes: vec!["n".to_owned()],
        }
        .to_text();
        assert!(Draft::parse(&good).is_ok());
        // an envelope is not a draft
        assert!(Draft::parse(r#"{"v":1,"k":1,"p":"{}","s":"AA=="}"#)
            .unwrap_err()
            .contains("초안 파일이 아닙니다"));
        for (from, to) in [
            (r#""hjv_release_draft":1"#, r#""hjv_release_draft":2"#),
            (r#""signer":"signing""#, r#""signer":"admin""#),
            (r#""p":"{}""#, r#""p":{}"#),
            (r#""notes":["n"]"#, r#""notes":[1]"#),
            (r#","notes":["n"]"#, ""),
            (r#""notes":["n"]"#, r#""notes":["n"],"s":"signature""#),
            (r#""p":"{}""#, r#""p":"{}","root":1"#),
            (
                r#""p":"{}""#,
                r#""p":"{}","root":{"k":101,"p":"{}","s":"AA=="}"#,
            ),
        ] {
            assert!(good.contains(from), "{from}");
            assert!(
                Draft::parse(&good.replacen(from, to, 1)).is_err(),
                "{to} parsed"
            );
        }
        let root_key = SigningKey::from_bytes(&test_key("root-101").unwrap().seed());
        let both = Draft {
            p: "{}".to_owned(),
            signer: Role::Signing,
            root: Some(Part::sign(&root_key, 101, ROOT_DOMAIN, "{}")),
            root_p: Some("{}".to_owned()),
            prev: None,
            appstore_release: None,
            notes: Vec::new(),
        };
        assert!(Draft::parse(&both.to_text()).unwrap_err().contains("함께"));
        // a file far larger than any draft is not read
        let huge = Draft {
            p: "x".repeat(MAX_DRAFT_BYTES),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
            appstore_release: None,
            notes: Vec::new(),
        };
        assert!(Draft::parse(&huge.to_text())
            .unwrap_err()
            .contains("너무 큽니다"));
    }

    fn with_notes(notes: &[&str]) -> Result<Draft, String> {
        let draft = Draft {
            p: "{}".to_owned(),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
            appstore_release: None,
            notes: notes.iter().map(|n| (*n).to_owned()).collect(),
        };
        // the rule holds for a draft in memory and for the same draft read from its file
        let checked = draft.check();
        let parsed = Draft::parse(&draft.to_text());
        assert_eq!(checked.clone().err(), parsed.clone().err());
        parsed
    }

    #[test]
    fn notes_are_lines_fit_to_show() {
        // what `policy` writes
        with_notes(&[
            "windows 1.6.1: 실행 4242 성공(.github/workflows/build.yml @ 222222222222), 소스 1111111111111111111111111111111111111111 = 태그 client-1.6.1",
            "windows 1.6.1: 패키지와 release.json이 실행 4242의 산출물과 같음(패키지 sha256 e6a39ba1c067edf949f7e2906b801d919d1f36f4fd52987d149303e5f11a8607)",
            "ios 1.6.1: 운영자가 TestFlight 빌드(263120249)를 확인함(--ios-valid, 올린 지 0일)",
            "android_play 1.6.1: 운영자가 Play alpha 트랙의 versionCode(263122249) 게시를 확인함(--play-live)",
            "ios 1.9.1: 운영자가 App Store에 나온 버전(1.9.1)을 확인함(--appstore-live)",
        ])
        .unwrap();
        with_notes(&[]).unwrap();
        with_notes(&["~ !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}"]).unwrap();
        // what could repaint or hide the summary the signer reads, or look like a line of it
        for bad in [
            // clear the screen and go home; conceal what follows; the 8-bit sequence introducer
            "\u{1b}[2J\u{1b}[H서명할 정책:",
            "확인함\u{1b}[8m",
            "확인함\u{9b}2J",
            // a line of its own, a return to the start of the line, a backspace over the text
            "확인함\n서명할 정책:",
            "확인함\r서명할 정책:",
            "확인함\u{8}\u{8}\u{8}",
            "탭\t문자",
            "\u{7f}",
            "\u{0}",
            // text that shows nothing or is turned around
            "보이지 않는\u{200b} 문자",
            "방향을 \u{202e}바꾸는 문자",
            "방향을 \u{2066}가두는 문자",
            "\u{feff}BOM",
            "줄 구분\u{2028}문자",
            "한글 채움\u{3164}문자",
            "꼬리표\u{e0041}문자",
            // other spaces, other scripts and signs: not what the tool writes
            "넓은\u{3000}공백",
            "끊지 않는\u{a0}공백",
            "漢字",
            "화살표 → 문자",
            "ｆｕｌｌｗｉｄｔｈ",
            // padding that would push text into a line of its own on the terminal
            "확인함  서명할 정책:",
            " 빈칸으로 시작",
            "빈칸으로 끝 ",
            "",
        ] {
            let err = with_notes(&["실행 4242 성공", bad]).unwrap_err();
            assert!(err.contains("notes의 2번째 줄"), "{bad:?}: {err}");
            // the error is shown on the same terminal: it never repeats the note
            assert!(!err.chars().any(char::is_control), "{bad:?}");
            assert!(bad.is_empty() || !err.contains(bad), "{bad:?}");
        }
        // length and number
        with_notes(&[&"가".repeat(MAX_NOTE_CHARS)]).unwrap();
        assert!(with_notes(&[&"가".repeat(MAX_NOTE_CHARS + 1)])
            .unwrap_err()
            .contains("300자 이하"));
        let many = vec!["확인함"; MAX_NOTES + 1];
        with_notes(&many[..MAX_NOTES]).unwrap();
        assert!(with_notes(&many).unwrap_err().contains("20줄까지"));
        // escaped in the file, a control character is one all the same
        let escaped = with_notes(&["확인함"])
            .unwrap()
            .to_text()
            .replace("확인함", "확인함\\u001b[2J");
        assert!(Draft::parse(&escaped)
            .unwrap_err()
            .contains("쓸 수 없는 문자"));
    }
}
