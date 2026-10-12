// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-relay, the HJViewer Wake-on-LAN relay. It runs on an always-on Linux machine inside a PC's
//! LAN (the user's Synology NAS), asks the HJViewer server for wake commands over HTTPS with signed
//! long-poll requests, checks each command's Ed25519 signature and limits, and sends magic packets
//! on its own interface that matches the PC's network. `PROTOCOL.md` next to this crate is the wire
//! contract with the server (hjv-api); `docs/relay-dsm.md` is the install guide.
//!
//! The binary (`src/main.rs`) is the command line; this library holds everything else so that the
//! integration tests can drive the relay loop against a test server and inside network namespaces.

pub mod clock;
pub mod command;
pub mod config;
pub mod envelope;
pub mod https;
pub mod identity;
pub mod log;
pub mod netif;
pub mod protocol;
pub mod relay;
pub mod system;
pub mod wake;
