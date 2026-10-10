//! Entry point.
//!
//! Sources on the command line mean a single run that reports on the
//! terminal. No sources means the window, where addons are installed by drag
//! and drop.

use std::process::ExitCode;

use clap::Parser;

use simple_addons::cli::{self, Args};
use simple_addons::gui;

fn main() -> ExitCode {
    let args = Args::parse();
    if cli::wants_window(&args) {
        return open_window(&args);
    }
    cli::run(&args)
}

/// A window that will not open has to be reported on the terminal, since
/// there is no window to report it in.
fn open_window(args: &Args) -> ExitCode {
    match gui::run(
        args.wow_dir.as_deref(),
        args.flavor.as_deref(),
        args.sources.clone(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("simple-addons: cannot open the window: {error}");
            ExitCode::FAILURE
        }
    }
}
