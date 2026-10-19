// SPDX-License-Identifier: AGPL-3.0-only

//! The support grant (지원 확인, TV-W6; docs/access-tickets.md 3-7 "지원 확인"): with the ticket a
//! PC trades a support code for, hjv-api signs with its list key which device made the code, for
//! that PC only. The PC keeps it next to the ticket, in memory only, and its connection window
//! marks a login whose same-account proof (`hjv-conn-v1`) is by that device's account key as
//! "연세대 교직원(로그인 확인)". The grant opens nothing: the PC's click approval, password and 2FA
//! decide as they did. It carries no name (the user's decision for 1.9.0: outsiders' PCs see no
//! staff name), so `name` is always "".
//!
//! ```text
//! "staff":{"kid":1,"g":"<standard base64 of the grant bytes>","s":"<standard base64, 88>"}
//! signed: "hjv-support-v1" NUL <grant bytes>          key: the list key `kid` (trust.rs)
//! grant bytes (UTF-8 JSON, in this order, no spaces):
//! {"v":1,"typ":"hjv-support","kid":1,"pc_id":"<PC ID>","pc_pk":"<base64 32>",
//!  "ck":"<base64 32>","ctl_id":"<the maker's device ID, or "">","name":"","iat":…,"exp":…}
//! ```
//!
//! `pc_id` and `pc_pk` are the trade's (checked with the PC's signature), `ck` and `ctl_id` the
//! account key that signed the request that made the code (`ctl_keys.pk`, `ctl_keys.ctl_id`),
//! `iat` the ticket's and `exp` the ticket's too, but at most [`MAX_LIFE`] after `iat` ([`exp_for`]):
//! the apps refuse a longer grant (MAX_GRANT_LIFE), so a ticket made longer than support.rs
//! `TICKET_SECS`'s two hours leaves the grant, and the staff mark, at two hours until the apps'
//! bound and the vector file's `max_life` change in a new app release. The bytes are what is signed, so [`Statement`] fixes their
//! order; the app checks the same bytes in its src/hjviewer_account/grant.rs, and both sides test
//! against tests/data/support-grant-vectors.json.

use crate::{trust::ListKey, util::b64};
use serde::Serialize;
use serde_json::{json, Value};

/// The purpose string of a grant's signature (docs/architecture.md 5-2), never the trust list's.
pub const CONTEXT: &[u8] = b"hjv-support-v1";
/// The longest a grant may live, `exp − iat`: the apps refuse a longer one (their
/// src/hjviewer_account/grant.rs MAX_GRANT_LIFE, the vector file's `max_life`).
pub const MAX_LIFE: i64 = 7200;

/// A grant's `exp` for a ticket of `iat` and `exp`: the ticket's, at most [`MAX_LIFE`] after
/// `iat`.
pub fn exp_for(iat: i64, ticket_exp: i64) -> i64 {
    ticket_exp.min(iat.saturating_add(MAX_LIFE))
}

/// What a grant says, in the order of its bytes.
#[derive(Serialize)]
struct Statement<'a> {
    v: u8,
    typ: &'static str,
    kid: u32,
    pc_id: &'a str,
    pc_pk: String,
    ck: String,
    ctl_id: &'a str,
    /// always "": outsiders' PCs are shown no staff name
    name: &'static str,
    iat: i64,
    exp: i64,
}

/// The PC a grant is for and the device that made the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant<'a> {
    pub pc_id: &'a str,
    pub pc_pk: [u8; 32],
    /// the maker's account key
    pub ck: [u8; 32],
    /// the maker's device ID ("" when its login came from none)
    pub ctl_id: &'a str,
    pub iat: i64,
    pub exp: i64,
}

/// The grant's bytes, for the list key `kid`.
pub fn bytes(kid: u32, grant: &Grant<'_>) -> Option<Vec<u8>> {
    let statement = Statement {
        v: 1,
        typ: "hjv-support",
        kid,
        pc_id: grant.pc_id,
        pc_pk: b64(&grant.pc_pk),
        ck: b64(&grant.ck),
        ctl_id: grant.ctl_id,
        name: "",
        iat: grant.iat,
        exp: grant.exp,
    };
    serde_json::to_vec(&statement).ok()
}

/// The bytes a list key signs for a grant: [`CONTEXT`], NUL and the grant's bytes.
pub fn message(grant: &[u8]) -> Vec<u8> {
    [CONTEXT, &[0u8], grant].concat()
}

/// The `staff` member of a support code's trade: the grant signed with `key` for its own purpose
/// ([`CONTEXT`], trust.rs `ListKey::sign_with`); `None` when it could not be made (the trade then
/// gives the ticket alone).
pub fn staff(key: &ListKey, grant: &Grant<'_>) -> Option<Value> {
    let g = bytes(key.kid(), grant)?;
    let s = key.sign_with(CONTEXT, &g)?;
    Some(json!({ "kid": key.kid(), "g": b64(&g), "s": b64(&s) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust;

    #[test]
    fn the_bytes_are_in_their_order_and_signed_for_their_purpose() {
        let grant = Grant {
            pc_id: "123456789",
            pc_pk: [2; 32],
            ck: [3; 32],
            ctl_id: "",
            iat: 10,
            exp: 7210,
        };
        let g = bytes(1, &grant).unwrap();
        assert_eq!(
            String::from_utf8(g.clone()).unwrap(),
            format!(
                r#"{{"v":1,"typ":"hjv-support","kid":1,"pc_id":"123456789","pc_pk":"{}","ck":"{}","ctl_id":"","name":"","iat":10,"exp":7210}}"#,
                b64(&[2; 32]),
                b64(&[3; 32])
            )
        );
        let key = ListKey::new(1, &[1; 32]);
        let staff = staff(&key, &grant).unwrap();
        assert_eq!(staff.as_object().unwrap().len(), 3);
        assert_eq!(staff["s"].as_str().unwrap().len(), 88);
        // the trust list's signature of the same bytes is another one
        assert_ne!(staff["s"], json!(b64(&key.sign(&g))));
        assert_ne!(message(&g), trust::message(&g));
        assert!(message(&g).starts_with(b"hjv-support-v1\0"));
        assert_eq!(&message(&g)[15..], &g[..]);
        assert!(crate::devsig::verify(
            &key.public_key(),
            &message(&g),
            staff["s"].as_str().unwrap()
        ));
    }

    #[test]
    fn a_grant_lives_the_tickets_time_at_most_two_hours() {
        assert_eq!(exp_for(10, 7210), 7210);
        assert_eq!(exp_for(10, 3610), 3610);
        assert_eq!(exp_for(10, 10 + 8 * 3600), 7210);
    }
}
