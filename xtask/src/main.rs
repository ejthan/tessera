// SPDX-License-Identifier: GPL-3.0-or-later

//! Developer tasks, run with `cargo xtask <task>`.

mod nf3;
mod seed;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask seed --url <url> [--key <key>] [--reset]
       cargo xtask nf3 --url <url> [--key <key>] [--docs <n>] [--format jsonl|json|csv]
                       [--app-baseline-mb <n>] [--limit-mb <n>] [--keep-file <path>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("seed") => seed_task(&args[1..]),
        Some("nf3") => nf3_task(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn seed_task(args: &[String]) -> ExitCode {
    let opts = match seed::Options::parse(args) {
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

fn nf3_task(args: &[String]) -> ExitCode {
    let opts = match nf3::Options::parse(args) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match nf3::run(&opts) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
