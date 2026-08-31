pub mod app;
pub mod cli;
pub mod config;
pub mod domain;
pub mod error;
pub mod filesystem;
pub mod naming;
pub mod storage;
pub mod tmdb;
pub mod ui;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let exit_code = match cli::execute(cli) {
        Ok(outcome) => outcome.exit_code(),
        Err(error) => {
            eprintln!("Error: {error}");
            error.exit_code()
        }
    };

    std::process::exit(exit_code);
}
