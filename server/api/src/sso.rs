// SPDX-License-Identifier: AGPL-3.0-only

//! The Yonsei SSO (design v2 §4-1). The contract, as the university's other password-grant
//! clients use it, among them the WordPress plugin with the same credentials (D2):
//!
//! 1. Token: `POST <token URL>` with `Authorization: Basic <client secret>` (the credential holds
//!    the value exactly as it follows "Basic ") and the form
//!    `username=<portal ID>&password=<password>&grant_type=password`. Answer: JSON
//!    `{"success":true,"access_token":"…"}`. A refused password comes back as HTTP 200
//!    `{"success":false,"msg":"…"}`, or the OAuth way as 400 `{"error":"invalid_grant"}`.
//! 2. User: `POST <user-info URL>` with `Authorization: Bearer <access token>` and the form
//!    `apiUuid=<api uuid>&clientId=<client id>`. Answer: `{"data":[{"perslNo":…,"name":…,…}]}`.
//!
//! hjv-api keeps perslNo and the name, drops the SSO token at once and reads nothing else. The
//! outcomes are sorted as v2 §4-1 says (not every client does it this way):
//!
//! - credentials refused (counted against the ID's login limit): `success:false` with a message
//!   that is not an address error, or 400 `invalid_grant`;
//! - address refused (not counted against the ID, the operator is alerted): a message containing
//!   both "not allow" and the word "ip" (or "ips"), in any case, from either call. "ip" must stand
//!   as a word of its own ("Not Allowed IP", "IP-address", "IP입니다"), so that "multiple" or
//!   "description" do not turn a refused password into an address error that the ID's limit does
//!   not count;
//! - upstream error (not counted against the ID): everything else, such as 5xx, timeouts, a body
//!   that is not a JSON object, `{}`, no `success`, `success:true` without a token,
//!   `success:false` without a message, a failed user call, no perslNo.
//!
//! All three count against the client's address, as any failed login does (login.rs), and a
//! refused address or five upstream errors in a row pause SSO calls for a while (limits.rs).
//!
//! Each login gets 3 s to connect and 8 s in all (HJV_API_SSO_*_MS). Redirects are not followed,
//! answers over 64 KiB are cut off, and no proxy is used.

use crate::{config::SsoSettings, util::clean_text};
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

const MAX_ANSWER: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsoUser {
    /// as the SSO sent it (case kept, design v2 §4-1)
    pub persl_no: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Credentials,
    AddressRefused,
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

fn json_object(body: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(body) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn message(obj: Option<&Map<String, Value>>) -> Option<String> {
    obj.and_then(|o| o.get("msg"))
        .and_then(Value::as_str)
        .map(|m| clean_text(m, 200))
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
    let Some(obj) = obj else {
        return Err(SsoError::upstream(format!(
            "token call: HTTP {status}, not a JSON object"
        )));
    };
    let oauth_error = obj.get("error").and_then(Value::as_str);
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
    match obj.get("success") {
        Some(Value::Bool(true)) => match obj.get("access_token").and_then(Value::as_str) {
            Some(token) if !token.trim().is_empty() => Ok(Zeroizing::new(token.to_owned())),
            _ => Err(SsoError::new(
                Failure::Upstream,
                "token call: success without access_token",
                msg,
            )),
        },
        Some(Value::Bool(false)) => match msg {
            Some(m) if !m.is_empty() => Err(SsoError::new(
                Failure::Credentials,
                "token call: success:false",
                Some(m),
            )),
            _ => Err(SsoError::upstream(
                "token call: success:false without a message",
            )),
        },
        Some(_) => Err(SsoError::new(
            Failure::Upstream,
            "token call: success is not true or false",
            msg,
        )),
        None => Err(SsoError::new(
            Failure::Upstream,
            "token call: no success field",
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
        return Err(SsoError::upstream("user call: not a JSON object"));
    };
    if obj.get("success") == Some(&Value::Bool(false)) {
        return Err(SsoError::new(
            Failure::Upstream,
            "user call: success:false",
            msg,
        ));
    }
    let first = match obj.get("data") {
        Some(Value::Array(data)) => match data.first() {
            Some(Value::Object(first)) => first,
            Some(_) => return Err(SsoError::upstream("user call: data[0] is not an object")),
            None => return Err(SsoError::upstream("user call: empty data")),
        },
        _ => return Err(SsoError::upstream("user call: no data list")),
    };
    let persl_no = match first.get("perslNo") {
        Some(Value::String(s)) => clean_text(s, 64),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    if persl_no.is_empty() {
        return Err(SsoError::upstream("user call: no perslNo"));
    }
    let name = first
        .get("name")
        .and_then(Value::as_str)
        .map(|n| clean_text(n, 64))
        .unwrap_or_default();
    Ok(SsoUser { persl_no, name })
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

    /// Checks `username` and `password` with the SSO.
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
                .append_pair("username", username)
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

    #[test]
    fn token_outcomes() {
        use Failure::*;
        assert_eq!(
            token(200, r#"{"success":true,"access_token":"abc"}"#),
            Ok("abc".to_owned())
        );
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
        // everything else is the SSO's problem, not the user's
        for (status, body) in [
            (400, r#"{"error":"invalid_client"}"#),
            (401, r#"{"error":"invalid_client"}"#),
            (400, r#"{"error":"invalid_request"}"#),
            (500, r#"{"success":false,"msg":"x"}"#),
            (502, "<html>bad gateway</html>"),
            (200, "<html></html>"),
            (200, ""),
            (200, "[]"),
            (200, "{}"),
            (200, r#"{"access_token":"abc"}"#),
            (200, r#"{"success":true}"#),
            (200, r#"{"success":true,"access_token":""}"#),
            (200, r#"{"success":true,"access_token":5}"#),
            (200, r#"{"success":false}"#),
            (200, r#"{"success":false,"msg":""}"#),
            (200, r#"{"success":"false","msg":"x"}"#),
            (302, r#"{"success":true,"access_token":"abc"}"#),
        ] {
            assert_eq!(token(status, body), Err(Upstream), "{status} {body}");
        }
    }

    #[test]
    fn user_outcomes() {
        use Failure::*;
        assert_eq!(
            user(
                200,
                r#"{"success":true,"data":[{"perslNo":"Y1234","name":"홍길동","email":"x@y"}]}"#
            ),
            Ok(SsoUser {
                persl_no: "Y1234".into(),
                name: "홍길동".into()
            })
        );
        assert_eq!(
            user(200, r#"{"data":[{"perslNo":2026123456}]}"#),
            Ok(SsoUser {
                persl_no: "2026123456".into(),
                name: String::new()
            })
        );
        assert_eq!(
            user(200, r#"{"success":false,"msg":"Not allowed IP"}"#),
            Err(AddressRefused)
        );
        for (status, body) in [
            (500, r#"{"data":[{"perslNo":"1"}]}"#),
            (200, r#"{"success":false,"msg":"no such api"}"#),
            (200, "not json"),
            (200, "{}"),
            (200, r#"{"data":[]}"#),
            (200, r#"{"data":["x"]}"#),
            (200, r#"{"data":{"perslNo":"1"}}"#),
            (200, r#"{"data":[{"name":"x"}]}"#),
            (200, r#"{"data":[{"perslNo":"  "}]}"#),
            (200, r#"{"data":[{"perslNo":null}]}"#),
        ] {
            assert_eq!(user(status, body), Err(Upstream), "{status} {body}");
        }
    }

    #[test]
    fn messages_are_kept_for_the_journal() {
        let e = classify_token(200, br#"{"success":false,"msg":"bad\npassword"}"#).unwrap_err();
        assert_eq!(e.message.as_deref(), Some("badpassword"));
        assert!(!e.detail.contains("bad"));
    }
}
