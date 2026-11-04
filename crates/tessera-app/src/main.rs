// SPDX-License-Identifier: GPL-3.0-or-later

use std::process::ExitCode;

use tessera_core::cli::{self, Launch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match cli::parse(&args, &|k| std::env::var(k).ok()) {
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
