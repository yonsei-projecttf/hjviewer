// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-api, the HJViewer account API (design v2 §9): Yonsei portal login through the university's
//! SSO, the per-account list of Windows PCs that the apps show as "accessible devices", and the
//! admin commands. nginx hands it the account paths only (deploy/nginx-hjviewer-api.conf), over a
//! Unix socket that systemd holds (deploy/systemd/hjviewer-api.socket).
//!
//! Later phases add routes, not rewrites: the password-less connection proof (design v3) and
//! Wake-on-LAN each become a module of their own, dispatched from [`api::route`], with their
//! tables added by a schema migration in [`db`]. README.md lists the places. Design v3 is here:
//! the account key of each login ([`keys`]), its signature on every account request ([`proof`]),
//! the server's names for devices ([`devmodel`]), and the sync of the bound PCs ([`pcsync`]) that
//! hands each one its signed trust list ([`trust`]) in the signed envelope it shares with
//! hjv-relay ([`envelope`]). Its phase 2, Wake-on-LAN through NAS relays: the PCs' wake reports
//! and their judgement ([`wolinfo`]), hjv-relay's hello and long-poll ([`relay`], the server side
//! of relay/PROTOCOL.md), and the app's wake requests with the signed commands ([`wake`]). The
//! minimum version and forced update add the packages of new app versions ([`update`]), which
//! hjv-api streams itself ([`body`]).

#[macro_use]
pub mod log;

pub mod admin;
pub mod alert;
pub mod api;
pub mod backup;
pub mod body;
pub mod check;
pub mod config;
pub mod db;
pub mod devmodel;
pub mod devsig;
pub mod envelope;
pub mod fake_sso;
pub mod hbbs;
pub mod jobs;
pub mod keygen;
pub mod keys;
pub mod limits;
pub mod login;
pub mod oplog;
pub mod overview;
pub mod pcsync;
pub mod proof;
pub mod relay;
pub mod seal;
pub mod selfcheck;
pub mod server;
pub mod sso;
pub mod state;
pub mod tfa;
pub mod trust;
pub mod update;
pub mod util;
pub mod wake;
pub mod wolinfo;
