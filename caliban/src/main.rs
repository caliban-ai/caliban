//! caliban — agent harness binary entrypoint.

use std::env;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NAME: &str = env!("CARGO_PKG_NAME");

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("--version" | "-V") => {
            println!("{NAME} {VERSION}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument: {other}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("caliban: no command given (this is a Layer-0 stub)");
            ExitCode::from(2)
        }
    }
}
