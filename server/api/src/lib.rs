// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-api, the HJViewer account API (design v2 §9): Yonsei portal login through the university's
//! SSO, the per-account list of Windows PCs that the apps show as "accessible devices", and the
//! admin commands. nginx hands it the account paths only (deploy/nginx-hjviewer-api.conf), over a
//! Unix socket that systemd holds (deploy/systemd/hjviewer-api.socket).
//!
//! Later phases add routes, not rewrites: the password-less connection proof (design v3) and
//! Wake-on-LAN (v2 §7) each become a module of their own, dispatched from [`api::route`], with
//! their tables added by a schema migration in [`db`]. README.md lists the places.

#[macro_use]
pub mod log;

pub mod admin;
pub mod api;
pub mod config;
pub mod db;
pub mod devsig;
pub mod fake_sso;
pub mod hbbs;
pub mod jobs;
pub mod keygen;
pub mod limits;
pub mod login;
pub mod oplog;
pub mod seal;
pub mod selfcheck;
pub mod server;
pub mod sso;
pub mod state;
pub mod tfa;
pub mod util;
