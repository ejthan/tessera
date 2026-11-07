// SPDX-License-Identifier: GPL-3.0-or-later

//! Developer tasks, run with `cargo xtask <task>`.

mod i18n;
mod nf3;
mod seed;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask seed --url <url> [--key <key>] [--reset]
       cargo xtask nf3 --url <url> [--key <key>] [--docs <n>] [--format jsonl|json|csv]
                       [--app-baseline-mb <n>] [--limit-mb <n>] [--keep-file <path>]
       cargo xtask i18n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("seed") => seed_task(&args[1..]),
        Some("nf3") => nf3_task(&args[1..]),
        Some("i18n") => i18n_task(),
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

/// The workspace root, whatever folder the task runs from.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
}

fn i18n_task() -> ExitCode {
    let findings = match i18n::run(root()) {
        Ok(findings) => findings,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    for f in &findings {
        eprintln!("{}:{}: {}", f.file.display(), f.line, f.message);
    }
    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
