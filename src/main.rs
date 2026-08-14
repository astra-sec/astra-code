mod adapter;
mod cli;
mod docker;
mod model;
mod protocol;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    match real_main() {
        Ok(code) if (0..=255).contains(&code) => ExitCode::from(code as u8),
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("astra-code: {error}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<i32, String> {
    match cli::parse()? {
        cli::Command::Run(options) => docker::run(*options),
        cli::Command::Doctor { image } => docker::doctor(&image),
        cli::Command::Harnesses => {
            cli::print_harnesses();
            Ok(0)
        }
        cli::Command::Shim => adapter::run_shim(),
        cli::Command::Help => {
            cli::print_help();
            Ok(0)
        }
        cli::Command::Printed => Ok(0),
        cli::Command::Version => {
            println!("astra-code {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
    }
}
