// SPDX-License-Identifier: AGPL-3.0-only

//! The draft: what `policy` hands to `sign`. It holds the exact texts that will be signed and
//! what the signer needs to judge them, and no signature of its own.
//!
//! ```json
//! {"hjv_release_draft":1,"signer":"signing","p":"<policy text>",
//!  "root":{"k":101,"p":"…","s":"…"},        the last signed root declaration, attached as it is
//!  "root_p":"<root declaration text>",        or a new one, for the root key to sign
//!  "prev":"<the previous envelope file>","notes":["what policy checked", …]}
//! ```
//!
//! An agent may write a draft; only a person signs it. `sign` therefore checks the draft again
//! from its texts and shows what changes, and believes nothing else in it.

use crate::{
    envelope::Part,
    json::{quote, Json, Writer},
    keys::Role,
};

pub const DRAFT_VERSION: u64 = 1;
const MAX_DRAFT_BYTES: usize = 256 * 1024;

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
    /// What `policy` checked, for the person who signs.
    pub notes: Vec<String>,
}

impl Draft {
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
        if root.is_some() && root_p.is_some() {
            return Err("초안: 서명된 루트 선언과 새 루트 선언이 함께 있습니다".to_owned());
        }
        Ok(Draft {
            p,
            signer,
            root,
            root_p,
            prev,
            notes,
        })
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
            notes: vec!["출처 확인: 실행 4242".to_owned(), "줄\n바꿈".to_owned()],
            ..plain.clone()
        };
        assert_eq!(Draft::parse(&full.to_text()).unwrap(), full);
        let new_root = Draft {
            root_p: Some("{\"v\":1,\"epoch\":2}".to_owned()),
            ..plain.clone()
        };
        assert_eq!(Draft::parse(&new_root.to_text()).unwrap(), new_root);
    }

    #[test]
    fn refuses_other_files() {
        let good = Draft {
            p: "{}".to_owned(),
            signer: Role::Signing,
            root: None,
            root_p: None,
            prev: None,
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
            notes: Vec::new(),
        };
        assert!(Draft::parse(&both.to_text()).unwrap_err().contains("함께"));
    }
}
