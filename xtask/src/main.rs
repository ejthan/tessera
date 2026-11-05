// SPDX-License-Identifier: GPL-3.0-or-later

//! Developer tasks, run with `cargo xtask <task>`.

mod seed;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask seed --url <url> [--key <key>] [--reset]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("seed") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let opts = match seed::Options::parse(&args[1..]) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let run = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|rt| rt.block_on(seed::run(&opts)));
    match run {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
