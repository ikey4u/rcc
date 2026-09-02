mod cache;
mod cli;
mod dispatch;
mod engine;
mod payload;
mod provider;
mod toolchain;

use clap::Parser;

fn main() {
    if let Some(result) = dispatch::try_run_from_environment() {
        match result {
            Ok(code) => std::process::exit(code),
            Err(error) => {
                eprintln!("rcc-tool: {error:#}");
                std::process::exit(dispatch::DISPATCH_ERROR_EXIT);
            }
        }
    }

    let cli = cli::Cli::parse();
    match cli::run(cli) {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("rcc: {error:#}");
            std::process::exit(1);
        }
    }
}
