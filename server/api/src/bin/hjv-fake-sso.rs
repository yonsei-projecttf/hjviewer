// SPDX-License-Identifier: AGPL-3.0-only

//! The fake Yonsei SSO for the E2E (cargo build --features fake-sso): serves the token and user
//! calls of sso.rs on 127.0.0.1 with fixed test credentials and accounts (fake_sso.rs).
//!
//!     hjv-fake-sso [--listen 127.0.0.1:18600]

use hjv_api::fake_sso;
use std::{net::SocketAddr, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let mut listen: SocketAddr = "127.0.0.1:18600".parse().expect("an address");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--listen", Some(v)) => match v.parse::<SocketAddr>() {
                Ok(a) if a.ip().is_loopback() => listen = a,
                _ => {
                    eprintln!("--listen takes a loopback address such as 127.0.0.1:18600");
                    return std::process::ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("usage: hjv-fake-sso [--listen 127.0.0.1:18600]");
                return std::process::ExitCode::from(2);
            }
        }
    }
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{listen}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let fake = fake_sso::start(listener, Duration::from_secs(10));
    println!("fake Yonsei SSO on {}", fake.addr);
    println!("HJV_API_SSO_TOKEN_URL={}", fake.token_url());
    println!("HJV_API_SSO_USERINFO_URL={}", fake.user_url());
    println!("HJV_API_SSO_CLIENT_ID={}", fake_sso::CLIENT_ID);
    println!("credential hjviewer-sso-client-secret: {}", fake_sso::BASIC);
    println!("credential hjviewer-sso-api-uuid: {}", fake_sso::API_UUID);
    let users: Vec<&str> = fake_sso::ACCOUNTS.iter().map(|a| a.username).collect();
    println!(
        "test accounts (passwords in fake_sso.rs): {}",
        users.join(", ")
    );
    println!("failure usernames: {}", fake_sso::SCENARIOS.join(", "));
    let _ = tokio::signal::ctrl_c().await;
    drop(fake);
    std::process::ExitCode::SUCCESS
}
