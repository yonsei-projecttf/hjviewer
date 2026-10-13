// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-release: the operator's tool for HJViewer's signed version policy (minimum version and
//! forced update). README.md describes the commands and the file formats.
//!
//! - what is signed: envelope.rs (the file apps fetch), policy.rs (its texts), keys.rs (the
//!   public keys a build trusts), trust.rs (the rules by which an app accepts an envelope);
//! - private keys: keyfile.rs and age.rs (the encrypted key file), tty.rs (the passphrase, from
//!   the terminal only), signing.rs;
//! - making a policy: release.rs and provenance.rs (the builds and where they came from),
//!   make.rs (the next policy and what it may not do), draft.rs (what goes to the signer);
//! - the server's side: publish.rs (what is sent), hbbs.rs (hbbs's threshold and door 2),
//!   check.rs (the weekly look).

pub mod age;
pub mod args;
pub mod check;
pub mod commands;
pub mod draft;
pub mod envelope;
pub mod hbbs;
pub mod json;
pub mod keyfile;
pub mod keys;
pub mod make;
pub mod policy;
pub mod provenance;
pub mod publish;
pub mod release;
pub mod signing;
pub mod time;
pub mod trust;
pub mod tty;
pub mod version;
