// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-release <command> ...: see `hjv-release help` and README.md.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(_) => {
                eprintln!("hjv-release: 인자가 UTF-8이 아닙니다");
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::from(hjv_release::commands::run(&args) as u8)
}
