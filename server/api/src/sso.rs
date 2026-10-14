// SPDX-License-Identifier: AGPL-3.0-only

//! The Yonsei SSO (design v2 §4-1). hjv-api uses the registration of the 행정허브 WordPress plugin
//! on the same server (D2) and follows the contract that plugin's code shows:
//!
//! 1. Token: `POST <token URL>` with `Authorization: Basic <client secret>` (the credential holds
//!    the value exactly as it follows "Basic ") and the form (`application/x-www-form-urlencoded`)
//!    of exactly `username=<portal ID, uppercased>&password=<password>&grant_type=password`.
//!    Answer: `{"success":true,"access_token":"…"}`, the token a string of 1 to 16384 characters
//!    without control characters or whitespace.
//! 2. User: `POST <user-info URL>` with `Authorization: Bearer <access token>` and the form of
//!    exactly `apiUuid=<api uuid>&clientId=<client id>`. The endpoint is dynamic (its fields
//!    depend on the api uuid); for this registration it answers
//!    `{"success":true,"data":[{"pcode":…,"name":…,"email":…,"deptNm":…}]}`.
//!
//! Each answer is HTTP 2xx, at most 1 MiB and JSON nested at most 32 deep, with a top-level
//! `success`: `true`, `1`, `"true"` or `"1"` goes on, `false`, `0`, `"false"` or `"0"` refuses,
//! and anything else, or none, is a format error. `data[0]` must be an object; of it hjv-api reads
//! `pcode` and `name` only: strings without control characters, trimmed, of at most 60 and 250
//! characters, the name not empty. The pcode, uppercased, must pass HJViewer's ID rule (util.rs
//! `staff_id`: Z or Y, not ZA): it is the person's fixed ID, which the account keeps with the name
//! (db.rs `accounts.portal_id`). The e-mail and the department are not read, and the SSO's token
//! goes at once.
//!
//! The outcomes, sorted as v2 §4-1 says (not every client does it this way):
//!
//! - credentials refused (counted against the ID's login limit): the token call refuses, or
//!   answers 400 `{"error":"invalid_grant"}` (the OAuth way);
//! - denied (not counted against the ID; the person is told so, login.rs): the user call refuses
//!   (access denied), or names a pcode outside HJViewer's ID rule. The SSO took the password, but
//!   the person may not use HJViewer;
//! - address refused (not counted against the ID, the operator is alerted): a `msg` containing
//!   both "not allow" and the word "ip" (or "ips"), in any case, from either call. "ip" must stand
//!   as a word of its own ("Not Allowed IP", "IP-address", "IP입니다"), so that "multiple" or
//!   "description" do not turn a refused password into an address error that the ID's limit does
//!   not count;
//! - client refused (not counted against the ID, the operator is alerted): the token call's
//!   400 or 401 `{"error":"invalid_client"}`, which every login gets until the credential
//!   hjviewer-sso-client-secret is fixed;
//! - upstream error (not counted against the ID): everything else, such as 5xx, timeouts, an
//!   answer too large or not JSON, `{}`, no `success` or another value, a token or a `data[0]`
//!   that breaks the rules above.
//!
//! The plugin reads neither `msg` nor `error`: the refused address, the refused client and
//! invalid_grant are hjv-api's own sorting on top of the contract (v2 §4-1), so that the SSO
//! refusing this server neither reads as a wrong password nor goes unnoticed. None of them lets
//! through a login that the contract refuses.
//!
//! A failed login counts against the client's address (login.rs), except a denied one, which
//! counts as a success does. A refused address, a refused client or five upstream errors in a row
//! pause SSO calls for a while (limits.rs); while they are paused every login gets the same 503
//! before the allowlist is looked at (design v3 §4-1), so that SSO trouble neither reads as a
//! wrong password nor tells who is on the list.
//!
//! Each login gets 3 s to connect and 8 s in all (HJV_API_SSO_*_MS). Redirects are not followed,
//! answers over 1 MiB are cut off, and no proxy is used.

use crate::{
    config::SsoSettings,
    util::{clean_text, staff_id},
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::Bytes,
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT},
    Request, Uri,
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::{Map, Value};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// The longest answer read (the plugin's limit).
pub const MAX_ANSWER: usize = 1024 * 1024;
/// How deep an answer's arrays and objects may nest (PHP json_decode's depth, as the plugin
/// calls it).
pub const MAX_DEPTH: usize = 32;
/// The longest `access_token`, in characters.
pub const MAX_TOKEN: usize = 16384;
/// The longest `pcode` of `data[0]`, in characters.
pub const MAX_PCODE: usize = 60;
/// The longest `name` of `data[0]`, in characters.
pub const MAX_NAME: usize = 250;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoUser {
    /// the pcode the SSO names for the person, uppercased: it passed HJViewer's ID rule
    pub pcode: String,
    /// as the SSO sent it, trimmed
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Credentials,
    /// the SSO took the password, but the person may not use HJViewer: the user call refused, or
    /// its pcode is outside HJViewer's ID rule
    Denied,
    AddressRefused,
    /// the SSO refuses this server's client credentials (`invalid_client`)
    ClientRefused,
    Upstream,
}

#[derive(Debug, Clone)]
pub struct SsoError {
    pub failure: Failure,
    /// what went wrong, for the journal: no user data
    pub detail: String,
    /// the SSO's `msg`, which login.rs logs the first time it appears (with the ID masked)
    pub message: Option<String>,
}

impl SsoError {
    fn new(failure: Failure, detail: impl Into<String>, message: Option<String>) -> Self {
        SsoError {
            failure,
            detail: detail.into(),
            message,
        }
    }

    fn upstream(detail: impl Into<String>) -> Self {
        SsoError::new(Failure::Upstream, detail, None)
    }
}

/// An address error, by the SSO's message (design v2 §4-1): "not allow" and the word "ip".
pub fn is_address_error(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("not allow")
        && m.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == "ip" || word == "ips")
}

/// How deep arrays and objects nest in `value` (a scalar: 0), as json_decode counts it.
fn depth(value: &Value) -> usize {
    match value {
        Value::Array(a) => 1 + a.iter().map(depth).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// The answer as a JSON object, if it is one that nests at most [`MAX_DEPTH`] deep.
fn json_object(body: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(body) {
        Ok(value) if depth(&value) <= MAX_DEPTH => match value {
            Value::Object(map) => Some(map),
            _ => None,
        },
        _ => None,
    }
}

fn message(obj: Option<&Map<String, Value>>) -> Option<String> {
    obj.and_then(|o| o.get("msg"))
        .and_then(Value::as_str)
        .map(|m| clean_text(m, 200))
}

/// What a top-level `success` says: `Ok(true)` goes on, `Ok(false)` refuses, `Err` is a format
/// error. Only these forms count, compared exactly (no `"TRUE"`, `1.0` or `"yes"`).
fn success(obj: &Map<String, Value>) -> Result<bool, &'static str> {
    match obj.get("success") {
        None => Err("no success field"),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::Number(n)) if n.as_u64() == Some(1) => Ok(true),
        Some(Value::Number(n)) if n.as_u64() == Some(0) => Ok(false),
        Some(Value::String(s)) if s == "true" || s == "1" => Ok(true),
        Some(Value::String(s)) if s == "false" || s == "0" => Ok(false),
        Some(_) => Err("success is none of true, 1, \"true\", \"1\", false, 0, \"false\", \"0\""),
    }
}

/// The access token as the contract takes it.
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().count() <= MAX_TOKEN
        && !token.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// A member of `data[0]` as the contract reads it: a string of at most `max` characters without a
/// control character, trimmed of its spaces.
fn field(row: &Map<String, Value>, key: &str, max: usize) -> Result<String, SsoError> {
    let Some(Value::String(value)) = row.get(key) else {
        return Err(SsoError::upstream(format!(
            "user call: data[0].{key} is not a string"
        )));
    };
    if value.chars().any(char::is_control) {
        return Err(SsoError::upstream(format!(
            "user call: data[0].{key} has a control character"
        )));
    }
    if value.chars().count() > max {
        return Err(SsoError::upstream(format!(
            "user call: data[0].{key} is longer than {max} characters"
        )));
    }
    Ok(value.trim_matches(' ').to_owned())
}

/// Sorts the answer of the token call.
pub fn classify_token(status: u16, body: &[u8]) -> Result<Zeroizing<String>, SsoError> {
    let obj = json_object(body);
    let msg = message(obj.as_ref());
    if msg.as_deref().is_some_and(is_address_error) {
        return Err(SsoError::new(
            Failure::AddressRefused,
            format!("token call: HTTP {status}, address not allowed"),
            msg,
        ));
    }
    let oauth_error = obj
        .as_ref()
        .and_then(|o| o.get("error"))
        .and_then(Value::as_str);
    if matches!(status, 400 | 401) && oauth_error == Some("invalid_client") {
        return Err(SsoError::new(
            Failure::ClientRefused,
            format!("token call: HTTP {status}, invalid_client"),
            msg,
        ));
    }
    if status == 400 && oauth_error == Some("invalid_grant") {
        return Err(SsoError::new(
            Failure::Credentials,
            "token call: invalid_grant",
            msg,
        ));
    }
    if !(200..300).contains(&status) {
        let error = oauth_error.map_or_else(|| "-".to_owned(), |e| clean_text(e, 40));
        return Err(SsoError::new(
            Failure::Upstream,
            format!("token call: HTTP {status}, error={error}"),
            msg,
        ));
    }
    let Some(obj) = obj else {
        return Err(SsoError::upstream(format!(
            "token call: HTTP {status}, not a JSON object of at most {MAX_DEPTH} levels"
        )));
    };
    match success(&obj) {
        Ok(true) => match obj.get("access_token") {
            Some(Value::String(token)) if valid_token(token) => {
                Ok(Zeroizing::new(token.to_owned()))
            }
            _ => Err(SsoError::new(
                Failure::Upstream,
                "token call: success without a valid access_token",
                msg,
            )),
        },
        Ok(false) => Err(SsoError::new(
            Failure::Credentials,
            "token call: refused",
            msg,
        )),
        Err(what) => Err(SsoError::new(
            Failure::Upstream,
            format!("token call: {what}"),
            msg,
        )),
    }
}

/// Sorts the answer of the user call.
pub fn classify_user(status: u16, body: &[u8]) -> Result<SsoUser, SsoError> {
    let obj = json_object(body);
    let msg = message(obj.as_ref());
    if msg.as_deref().is_some_and(is_address_error) {
        return Err(SsoError::new(
            Failure::AddressRefused,
            format!("user call: HTTP {status}, address not allowed"),
            msg,
        ));
    }
    if !(200..300).contains(&status) {
        return Err(SsoError::new(
            Failure::Upstream,
            format!("user call: HTTP {status}"),
            msg,
        ));
    }
    let Some(obj) = obj else {
        return Err(SsoError::upstream(format!(
            "user call: not a JSON object of at most {MAX_DEPTH} levels"
        )));
    };
    match success(&obj) {
        Ok(true) => {}
        Ok(false) => {
            return Err(SsoError::new(
                Failure::Denied,
                "user call: refused (access denied)",
                msg,
            ))
        }
        Err(what) => {
            return Err(SsoError::new(
                Failure::Upstream,
                format!("user call: {what}"),
                msg,
            ))
        }
    }
    // data[0] as PHP reads it: the first of a list, or the member "0" of an object
    let first = match obj.get("data") {
        Some(Value::Array(list)) => list.first(),
        Some(Value::Object(map)) => map.get("0"),
        _ => None,
    };
    let Some(Value::Object(first)) = first else {
        return Err(SsoError::upstream("user call: data[0] is not an object"));
    };
    let pcode = field(first, "pcode", MAX_PCODE)?;
    let name = field(first, "name", MAX_NAME)?;
    if name.is_empty() {
        return Err(SsoError::upstream("user call: data[0].name is empty"));
    }
    match staff_id(&pcode) {
        Some(pcode) => Ok(SsoUser { pcode, name }),
        None => Err(SsoError::new(
            Failure::Denied,
            "user call: a pcode outside HJViewer's ID rule",
            None,
        )),
    }
}

/// TLS for the SSO and selfcheck: the system's CA certificates, ring as the crypto provider.
pub fn tls_config() -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| format!("TLS setup: {e}"))?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub struct SsoClient {
    http: HttpClient,
    token_url: Uri,
    user_url: Uri,
    client_id: String,
    basic: Zeroizing<String>,
    api_uuid: Zeroizing<String>,
    total_timeout: Duration,
}

impl SsoClient {
    pub fn new(
        settings: &SsoSettings,
        basic: Zeroizing<String>,
        api_uuid: Zeroizing<String>,
    ) -> Result<SsoClient, String> {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_nodelay(true);
        http.set_connect_timeout(Some(settings.connect_timeout));
        let https = HttpsConnectorBuilder::new()
            .with_tls_config(tls_config()?)
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(30))
            .build(https);
        HeaderValue::from_str(&format!("Basic {}", basic.as_str()))
            .map_err(|_| "the SSO client secret cannot go into a header".to_owned())?;
        Ok(SsoClient {
            http: client,
            token_url: settings.token_url.clone(),
            user_url: settings.userinfo_url.clone(),
            client_id: settings.client_id.clone(),
            basic,
            api_uuid,
            total_timeout: settings.total_timeout,
        })
    }

    /// Checks `username` (the portal ID typed, which the token call sends uppercased) and
    /// `password` with the SSO.
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<SsoUser, SsoError> {
        match tokio::time::timeout(self.total_timeout, self.exchange(username, password)).await {
            Ok(result) => result,
            Err(_) => Err(SsoError::upstream(format!(
                "no answer within {} ms",
                self.total_timeout.as_millis()
            ))),
        }
    }

    async fn exchange(&self, username: &str, password: &str) -> Result<SsoUser, SsoError> {
        let form = Zeroizing::new(
            form_urlencoded::Serializer::new(String::new())
                .append_pair("username", &username.to_ascii_uppercase())
                .append_pair("password", password)
                .append_pair("grant_type", "password")
                .finish(),
        );
        let basic = Zeroizing::new(format!("Basic {}", self.basic.as_str()));
        let (status, body) = self
            .post(&self.token_url, &basic, form)
            .await
            .map_err(|e| SsoError::upstream(format!("token call: {e}")))?;
        let token = classify_token(status, &body)?;
        let form = Zeroizing::new(
            form_urlencoded::Serializer::new(String::new())
                .append_pair("apiUuid", &self.api_uuid)
                .append_pair("clientId", &self.client_id)
                .finish(),
        );
        let bearer = Zeroizing::new(format!("Bearer {}", token.as_str()));
        let (status, body) = self
            .post(&self.user_url, &bearer, form)
            .await
            .map_err(|e| SsoError::upstream(format!("user call: {e}")))?;
        classify_user(status, &body)
    }

    async fn post(
        &self,
        url: &Uri,
        authorization: &str,
        form: Zeroizing<String>,
    ) -> Result<(u16, Bytes), String> {
        let mut auth = HeaderValue::from_str(authorization)
            .map_err(|_| "the SSO token cannot go into a header".to_owned())?;
        auth.set_sensitive(true);
        let request = Request::post(url.clone())
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("hjv-api/", env!("CARGO_PKG_VERSION")))
            .body(Full::new(Bytes::copy_from_slice(form.as_bytes())))
            .map_err(|_| "cannot build the request".to_owned())?;
        let response = self.http.request(request).await.map_err(|e| describe(&e))?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), MAX_ANSWER)
            .collect()
            .await
            .map_err(|_| "answer cut off or too large".to_owned())?
            .to_bytes();
        Ok((status, body))
    }
}

/// What went wrong with a request, for the journal: the error and its causes (DNS, TCP, TLS),
/// which name no user data.
fn describe(e: &hyper_util::client::legacy::Error) -> String {
    let mut text = if e.is_connect() {
        "cannot connect".to_owned()
    } else {
        "request failed".to_owned()
    };
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    clean_text(&text, 200)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token(status: u16, body: &str) -> Result<String, Failure> {
        classify_token(status, body.as_bytes())
            .map(|t| t.as_str().to_owned())
            .map_err(|e| e.failure)
    }

    fn user(status: u16, body: &str) -> Result<SsoUser, Failure> {
        classify_user(status, body.as_bytes()).map_err(|e| e.failure)
    }

    /// A user answer of the 행정허브 shape with `row` as `data[0]`.
    fn user_row(row: Value) -> Result<SsoUser, Failure> {
        user(200, &json!({ "success": true, "data": [row] }).to_string())
    }

    fn hong(pcode: &str) -> Result<SsoUser, Failure> {
        user_row(json!({
            "pcode": pcode, "name": "홍길동", "email": "fake@example.com", "deptNm": "가짜학과"
        }))
    }

    fn found(pcode: &str, name: &str) -> Result<SsoUser, Failure> {
        Ok(SsoUser {
            pcode: pcode.into(),
            name: name.into(),
        })
    }

    /// The forms of `success` the contract takes, in both calls: four that go on, four that
    /// refuse, and everything else a format error, also no `success` at all.
    #[test]
    fn the_success_forms() {
        use Failure::*;
        let yes = [json!(true), json!(1), json!("true"), json!("1")];
        let no = [json!(false), json!(0), json!("false"), json!("0")];
        let other = [
            json!(null),
            json!("TRUE"),
            json!("True"),
            json!("yes"),
            json!(" true"),
            json!("1 "),
            json!(""),
            json!(2),
            json!(-1),
            json!(1.0),
            json!(0.0),
            json!([]),
            json!([true]),
            json!({}),
            json!("success"),
        ];
        let row = json!([{ "pcode": "ZZTEST01", "name": "홍길동" }]);
        for value in &yes {
            let t = json!({ "success": value, "access_token": "abc" }).to_string();
            assert_eq!(token(200, &t), Ok("abc".to_owned()), "{t}");
            let u = json!({ "success": value, "data": row }).to_string();
            assert_eq!(user(200, &u), found("ZZTEST01", "홍길동"), "{u}");
        }
        for value in &no {
            // a refused token call is a wrong password, a refused user call a denial
            let t = json!({ "success": value, "access_token": "abc" }).to_string();
            assert_eq!(token(200, &t), Err(Credentials), "{t}");
            let u = json!({ "success": value, "data": row }).to_string();
            assert_eq!(user(200, &u), Err(Denied), "{u}");
        }
        for value in &other {
            let t = json!({ "success": value, "access_token": "abc" }).to_string();
            assert_eq!(token(200, &t), Err(Upstream), "{t}");
            let u = json!({ "success": value, "data": row }).to_string();
            assert_eq!(user(200, &u), Err(Upstream), "{u}");
        }
        // no success: a format error, whatever else the answer has
        assert_eq!(token(200, r#"{"access_token":"abc"}"#), Err(Upstream));
        assert_eq!(
            token(200, r#"{"Success":true,"access_token":"abc"}"#),
            Err(Upstream)
        );
        assert_eq!(
            user(200, &json!({ "data": row }).to_string()),
            Err(Upstream)
        );
        // a refusal needs no message
        assert_eq!(token(200, r#"{"success":false}"#), Err(Credentials));
        assert_eq!(user(200, r#"{"success":false}"#), Err(Denied));
    }

    #[test]
    fn token_outcomes() {
        use Failure::*;
        // the refused password as the Yonsei SSO sends it, and the OAuth way
        assert_eq!(
            token(
                200,
                r#"{"success":false,"msg":"아이디 또는 비밀번호가 틀립니다"}"#
            ),
            Err(Credentials)
        );
        assert_eq!(token(400, r#"{"error":"invalid_grant"}"#), Err(Credentials));
        // address errors in any case, from any status
        for body in [
            r#"{"success":false,"msg":"Not Allowed IP"}"#,
            r#"{"success":false,"msg":"ip address is NOT ALLOWED"}"#,
            r#"{"error":"x","msg":"not allowed ip"}"#,
        ] {
            assert_eq!(token(200, body), Err(AddressRefused), "{body}");
            assert_eq!(token(403, body), Err(AddressRefused), "{body}");
        }
        // "ip" alone or "not allow" alone is not an address error
        assert_eq!(
            token(200, r#"{"success":false,"msg":"invalid description"}"#),
            Err(Credentials)
        );
        assert_eq!(
            token(200, r#"{"success":false,"msg":"user not allowed"}"#),
            Err(Credentials)
        );
        // nor are "ip" inside words: a refused password stays counted
        for msg in [
            "Login not allowed: multiple failed attempts",
            "not allowed for this recipient",
            "Not allowed: see the description",
            "not allowed (zip code)",
        ] {
            let body = json!({"success": false, "msg": msg}).to_string();
            assert_eq!(token(200, &body), Err(Credentials), "{msg}");
        }
        for msg in [
            "IP-address not allowed",
            "not allowed IPs",
            "접근이 허용되지 않은 IP입니다 (not allowed)",
            "Not allowed [ip: 192.0.2.1]",
        ] {
            assert!(is_address_error(msg), "{msg}");
        }
        // the SSO refuses this server's client credentials
        for status in [400, 401] {
            assert_eq!(
                token(status, r#"{"error":"invalid_client"}"#),
                Err(ClientRefused)
            );
        }
        assert_eq!(token(500, r#"{"error":"invalid_client"}"#), Err(Upstream));
        // everything else is the SSO's problem, not the user's: not 2xx (whatever the body
        // says), not a JSON object, no or another success, no valid token
        for (status, body) in [
            (403, r#"{"error":"invalid_client"}"#),
            (400, r#"{"error":"invalid_request"}"#),
            (500, r#"{"success":false,"msg":"x"}"#),
            (500, r#"{"success":true,"access_token":"abc"}"#),
            (502, "<html>bad gateway</html>"),
            (302, r#"{"success":true,"access_token":"abc"}"#),
            (200, "<html></html>"),
            (200, ""),
            (200, "[]"),
            (200, r#"[{"success":true,"access_token":"abc"}]"#),
            (200, "{}"),
            (200, r#"{"success":true}"#),
            (200, r#"{"success":true,"access_token":""}"#),
            (200, r#"{"success":true,"access_token":5}"#),
            (200, r#"{"success":true,"access_token":null}"#),
            (200, r#"{"success":true,"access_token":["abc"]}"#),
        ] {
            assert_eq!(token(status, body), Err(Upstream), "{status} {body}");
        }
        // any 2xx goes on
        assert_eq!(
            token(201, r#"{"success":"1","access_token":"abc"}"#),
            Ok("abc".to_owned())
        );
    }

    /// The access token: 1 to 16384 characters, no control characters, no whitespace.
    #[test]
    fn the_access_token() {
        let answer = |t: &str| json!({ "success": true, "access_token": t }).to_string();
        let longest = "a".repeat(MAX_TOKEN);
        assert_eq!(token(200, &answer(&longest)), Ok(longest.clone()));
        let tok = "eyJhbGciOi.J9-_+/=~";
        assert_eq!(token(200, &answer(tok)), Ok(tok.to_owned()));
        for bad in [
            "a".repeat(MAX_TOKEN + 1),
            String::new(),
            " ".into(),
            " abc".into(),
            "abc ".into(),
            "a bc".into(),
            "abc\n".into(),
            "a\tbc".into(),
            "abc\u{0}".into(),
            "abc\u{7f}".into(),
            "abc\u{85}".into(),
            "abc\u{a0}".into(),
            "abc\u{3000}".into(),
        ] {
            assert_eq!(token(200, &answer(&bad)), Err(Failure::Upstream), "{bad:?}");
        }
    }

    #[test]
    fn user_outcomes() {
        use Failure::*;
        // the 행정허브 shape: pcode and name read; e-mail and department neither read nor kept
        assert_eq!(hong("ZZTEST01"), found("ZZTEST01", "홍길동"));
        // with or without e-mail and department, also null or of another type: never read
        for row in [
            json!({ "pcode": "ZZTEST01", "name": "홍길동" }),
            json!({ "pcode": "ZZTEST01", "name": "홍길동", "email": null, "deptNm": null }),
            json!({ "pcode": "ZZTEST01", "name": "홍길동", "email": 5, "deptNm": ["x"] }),
            json!({ "pcode": "ZZTEST01", "name": "홍길동", "email": "not an address" }),
        ] {
            assert_eq!(user_row(row.clone()), found("ZZTEST01", "홍길동"), "{row}");
        }
        // data[0] of an object, as PHP reads it too
        assert_eq!(
            user(
                200,
                r#"{"success":true,"data":{"0":{"pcode":"ZZTEST01","name":"홍길동"}}}"#
            ),
            found("ZZTEST01", "홍길동")
        );
        // perslNo is not read: without a pcode the answer is a format error
        assert_eq!(
            user_row(json!({ "perslNo": "ZZTEST01", "name": "홍길동" })),
            Err(Upstream)
        );
        assert_eq!(
            user(200, r#"{"success":false,"msg":"Not allowed IP"}"#),
            Err(AddressRefused)
        );
        assert_eq!(
            user(403, r#"{"success":false,"msg":"Not allowed IP"}"#),
            Err(AddressRefused)
        );
        // the user call refuses: access denied, not a wrong password
        assert_eq!(
            user(200, r#"{"success":false,"msg":"no such api"}"#),
            Err(Denied)
        );
        for (status, body) in [
            (
                500,
                r#"{"success":true,"data":[{"pcode":"ZZTEST01","name":"x"}]}"#,
            ),
            (401, r#"{"success":false}"#),
            (200, "not json"),
            (200, "{}"),
            (200, r#"{"success":true}"#),
            (200, r#"{"success":true,"data":[]}"#),
            (200, r#"{"success":true,"data":{}}"#),
            (
                200,
                r#"{"success":true,"data":{"1":{"pcode":"ZZTEST01","name":"x"}}}"#,
            ),
            (200, r#"{"success":true,"data":"ZZTEST01"}"#),
            (200, r#"{"success":true,"data":null}"#),
            // data[0] that is not an object
            (200, r#"{"success":true,"data":["ZZTEST01"]}"#),
            (200, r#"{"success":true,"data":[7]}"#),
            (200, r#"{"success":true,"data":[null]}"#),
            (
                200,
                r#"{"success":true,"data":[[{"pcode":"ZZTEST01","name":"x"}]]}"#,
            ),
            (
                200,
                r#"{"success":true,"data":[true,{"pcode":"ZZTEST01","name":"x"}]}"#,
            ),
        ] {
            assert_eq!(user(status, body), Err(Upstream), "{status} {body}");
        }
    }

    /// pcode and name: strings without control characters, trimmed of spaces, at most 60 and 250
    /// characters, the name not empty; else a format error.
    #[test]
    fn pcode_and_name() {
        use Failure::*;
        let row = |pcode: Value, name: Value| user_row(json!({ "pcode": pcode, "name": name }));
        // missing or not a string
        assert_eq!(user_row(json!({ "name": "홍길동" })), Err(Upstream));
        assert_eq!(user_row(json!({ "pcode": "ZZTEST01" })), Err(Upstream));
        for bad in [json!(null), json!(12345678), json!(["ZZTEST01"]), json!({})] {
            assert_eq!(row(bad.clone(), json!("홍길동")), Err(Upstream), "{bad}");
            assert_eq!(row(json!("ZZTEST01"), bad.clone()), Err(Upstream), "{bad}");
        }
        // trimmed of spaces; the name not empty
        assert_eq!(
            row(json!("  ZZTEST01 "), json!(" 홍 길동  ")),
            found("ZZTEST01", "홍 길동")
        );
        for name in ["", " ", "   "] {
            assert_eq!(
                row(json!("ZZTEST01"), json!(name)),
                Err(Upstream),
                "{name:?}"
            );
        }
        // the longest of each, and one character more (in characters, not bytes)
        let pcode = format!("Z{}", "9".repeat(MAX_PCODE - 1));
        let name = "가".repeat(MAX_NAME);
        assert_eq!(row(json!(pcode), json!(name)), found(&pcode, &name));
        assert_eq!(
            row(json!(format!("{pcode}9")), json!("홍길동")),
            Err(Upstream)
        );
        assert_eq!(
            row(json!("ZZTEST01"), json!(format!("{name}가"))),
            Err(Upstream)
        );
        // control characters anywhere, also at the ends that trimming would take
        for bad in [
            "\n", "\t", "\r", "\u{0}", "\u{b}", "\u{1b}", "\u{7f}", "\u{85}", "\u{9f}",
        ] {
            for (pcode, name) in [
                (format!("ZZTEST01{bad}"), "홍길동".to_owned()),
                (format!("{bad}ZZTEST01"), "홍길동".to_owned()),
                ("ZZTEST01".to_owned(), format!("홍{bad}길동")),
                ("ZZTEST01".to_owned(), format!("홍길동{bad}")),
            ] {
                assert_eq!(
                    row(json!(pcode), json!(name)),
                    Err(Upstream),
                    "{pcode:?} {name:?}"
                );
            }
        }
    }

    /// The pcode is the person's fixed ID: uppercased, it must pass HJViewer's ID rule (Z or Y,
    /// not ZA); else the person is denied, whatever ID was typed.
    #[test]
    fn the_pcode_rule() {
        use Failure::*;
        // Z and Y codes, uppercased
        assert_eq!(hong("ZZTEST01"), found("ZZTEST01", "홍길동"));
        assert_eq!(hong("YZTEST02"), found("YZTEST02", "홍길동"));
        assert_eq!(hong("zztest01"), found("ZZTEST01", "홍길동"));
        assert_eq!(hong("yZtest_02-x"), found("YZTEST_02-X", "홍길동"));
        // ZA, B (which the 행정허브 plugin takes), a student-like number, others: denied
        for pcode in [
            "ZATEST01",
            "zatest01",
            "BZTEST01",
            "1234567890",
            "AZTEST01",
            "Z",
            "ZZ TEST",
            "ZZ.TEST",
            "ＺＺTEST01",
            "",
            " ",
        ] {
            assert_eq!(hong(pcode), Err(Denied), "{pcode:?}");
        }
        // a denial carries no user data
        let e = classify_user(
            200,
            json!({ "success": true, "data": [{ "pcode": "ZATEST01", "name": "홍길동" }] })
                .to_string()
                .as_bytes(),
        )
        .unwrap_err();
        assert_eq!(e.failure, Denied);
        assert!(!e.detail.contains("ZATEST01") && !e.detail.contains("홍길동"));
        assert_eq!(e.message, None);
    }

    /// Answers nested deeper than json_decode's 32 levels are a format error, in both calls.
    #[test]
    fn nesting_depth() {
        let nest = |levels: usize| format!("{}{}", "[".repeat(levels - 1), "]".repeat(levels - 1));
        // the object is level 1: its members may nest 31 deep
        let t = |levels| {
            format!(
                r#"{{"success":true,"access_token":"abc","x":{}}}"#,
                nest(levels)
            )
        };
        let u = |levels| {
            format!(
                r#"{{"success":true,"data":[{{"pcode":"ZZTEST01","name":"홍길동","x":{}}}]}}"#,
                nest(levels)
            )
        };
        assert_eq!(
            depth(&serde_json::from_str(&t(MAX_DEPTH)).unwrap()),
            MAX_DEPTH
        );
        assert_eq!(token(200, &t(MAX_DEPTH)), Ok("abc".to_owned()));
        assert_eq!(token(200, &t(MAX_DEPTH + 1)), Err(Failure::Upstream));
        // data and data[0] take two levels more
        assert_eq!(user(200, &u(MAX_DEPTH - 2)), found("ZZTEST01", "홍길동"));
        assert_eq!(user(200, &u(MAX_DEPTH - 1)), Err(Failure::Upstream));
    }

    #[test]
    fn messages_are_kept_for_the_journal() {
        let e = classify_token(200, br#"{"success":false,"msg":"bad\npassword"}"#).unwrap_err();
        assert_eq!(e.failure, Failure::Credentials);
        assert_eq!(e.message.as_deref(), Some("badpassword"));
        assert!(!e.detail.contains("bad"));
        let e = classify_user(200, br#"{"success":"0","msg":"denied"}"#).unwrap_err();
        assert_eq!(e.failure, Failure::Denied);
        assert_eq!(e.message.as_deref(), Some("denied"));
    }
}
