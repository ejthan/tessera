// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::ExitCode;

use tessera_core::cli::{self, Launch};

fn main() -> ExitCode {
    let launch = match cli::utf8_args(std::env::args_os().skip(1))
        .and_then(|args| cli::parse(&args, &|k| std::env::var(k).ok()))
    {
        Ok(Launch::Help) => {
            print!("{}", cli::HELP);
            return ExitCode::SUCCESS;
        }
        Ok(launch) => launch,
        Err(e) => {
            eprintln!("tessera: {e}");
            return ExitCode::from(2);
        }
    };
    // Read before the window opens, so the terminal is still in front.
    match tessera_app::read_stdin_key(launch, &mut std::io::stdin().lock())
        .and_then(tessera_app::run)
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tessera: {e}");
            ExitCode::from(2)
        }
    }
}
