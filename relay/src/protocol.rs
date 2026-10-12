// SPDX-License-Identifier: AGPL-3.0-only

//! The JSON payloads the relay sends and the responses it reads (PROTOCOL.md, "Endpoints").
//! Responses may carry fields this version does not know; they are ignored.

use serde::{Deserialize, Serialize};

/// `v` of the hello and poll payloads.
pub const PAYLOAD_VERSION: u32 = 1;

/// One network interface the relay may send on, as reported to the server.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct IfaceReport {
    pub name: String,
    pub mac: Option<String>,
    /// `a.b.c.d/prefix`, the interface's usable IPv4 addresses.
    pub ipv4: Vec<String>,
    pub up: bool,
    pub running: bool,
    /// The interface's default gateway, if it has a default route.
    pub gateway: Option<String>,
    pub gateway_mac: Option<String>,
}

#[derive(Serialize)]
pub struct HelloPayload<'a> {
    pub v: u32,
    pub version: &'a str,
    pub arch: &'a str,
    pub os: &'a str,
    pub kernel: &'a str,
    pub ifaces_hash: &'a str,
    pub ifaces: &'a [IfaceReport],
}

#[derive(Serialize)]
pub struct PollPayload<'a> {
    pub v: u32,
    pub version: &'a str,
    pub arch: &'a str,
    pub os: &'a str,
    pub kernel: &'a str,
    pub wait: u32,
    pub ifaces_hash: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ifaces: Option<&'a [IfaceReport]>,
    pub results: &'a [CommandResult],
}

/// What the relay did with one command.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CommandResult {
    /// Lowercase hex SHA-256 of the command string `c` as received (null only when the entry had
    /// no string `c`).
    pub cmd: Option<String>,
    pub job: Option<String>,
    /// Estimated server time when the last packet went out; null when nothing was sent.
    pub sent_at: Option<i64>,
    pub iface: Option<String>,
    pub packets: u32,
    /// Null when every planned packet went out; otherwise the code of PROTOCOL.md "Results".
    pub error: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
pub struct HelloResponse {
    pub server_time: i64,
    pub state: String,
    pub code: Option<String>,
    pub poll_after: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PollResponse {
    pub server_time: i64,
    pub state: String,
    /// Kept as raw JSON: one malformed entry must not hide the others.
    pub commands: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub send_ifaces: bool,
    pub latest_version: Option<String>,
    pub min_version: Option<String>,
    pub poll_after: Option<u64>,
}

/// The body of a non-200 response, as far as it is JSON.
#[derive(Debug, Default, Deserialize)]
pub struct ErrorBody {
    pub error: Option<String>,
    pub server_time: Option<i64>,
    pub retry_after: Option<u64>,
}
