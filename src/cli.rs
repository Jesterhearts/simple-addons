//! The command line front end.
//!
//! This is pipeline software: it runs once, writes what it can, and reports
//! what it did. Anything that stops it from starting is printed and ends the
//! run, rather than being worked around.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use crate::discovery::{self, Flavor, WowInstall};
use crate::error::{Error, Result};
use crate::install::{AddonPlan, Outcome};
use crate::package::Toc;

#[derive(Debug, Parser)]
#[command(
    name = "simple-addons",
    version,
    about = "Install World of Warcraft addons from a folder or a zip file",
    after_help = "Run with no arguments to open the window and install by drag and drop."
)]
pub struct Args {
    /// Folders or zip files to install. Leave empty to open the window.
    #[arg(value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,

    /// World of Warcraft directory. Found automatically when left out.
    #[arg(short = 'w', long, value_name = "PATH", env = "SIMPLE_ADDONS_WOW_DIR")]
    pub wow_dir: Option<PathBuf>,

    /// Game version to install into, such as retail or classic_era.
    #[arg(short, long, value_name = "NAME")]
    pub flavor: Option<String>,

    /// Report what would be installed without writing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// List the World of Warcraft directories that were found, then stop.
    #[arg(long)]
    pub list: bool,

    /// Open the window even when sources are given.
    #[arg(long)]
    pub gui: bool,
}

/// True when the window should open rather than the command line running.
pub fn wants_window(args: &Args) -> bool {
    args.gui || (args.sources.is_empty() && !args.list)
}

pub fn run(args: &Args) -> ExitCode {
    if args.list {
        return report(list_installs(args.wow_dir.as_deref()));
    }
    report(install_all(args))
}

/// Prints whatever went wrong before any work could start, and turns the
/// result into an exit code.
fn report(result: Result<ExitCode>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("simple-addons: {error}");
            ExitCode::FAILURE
        }
    }
}

fn list_installs(wow_dir: Option<&Path>) -> Result<ExitCode> {
    let installs = match wow_dir {
        Some(dir) => vec![discovery::open(dir)?],
        None => discovery::discover(),
    };
    if installs.is_empty() {
        return Err(Error::NoInstallFound);
    }
    for install in &installs {
        println!("{}", install.root().display());
        for flavor in install.flavors() {
            println!(
                "  {:<14} {}",
                flavor.label(),
                describe_addon_count(&install.addons_dir(flavor))
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn describe_addon_count(addons_dir: &Path) -> String {
    match crate::count_installed(addons_dir) {
        Some(1) => "1 addon installed".to_string(),
        Some(count) => format!("{count} addons installed"),
        None => "no addon folder yet".to_string(),
    }
}

fn install_all(args: &Args) -> Result<ExitCode> {
    let (install, flavor) = resolve_target(args)?;
    let addons_dir = install.addons_dir(&flavor);
    println!(
        "{} {} ({})",
        if args.dry_run {
            "Would install to"
        } else {
            "Installing to"
        },
        install.root().display(),
        flavor.label()
    );

    // Every source is reported in one list, so that the columns line up
    // across a run over several of them.
    let lines: Vec<Line> = args
        .sources
        .iter()
        .flat_map(|source| source_lines(source, &addons_dir, args.dry_run))
        .collect();
    print_lines(&lines);

    if lines.iter().any(|line| line.failed) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn resolve_target(args: &Args) -> Result<(WowInstall, Flavor)> {
    let install = match &args.wow_dir {
        Some(dir) => discovery::open(dir)?,
        None => discovery::discover()
            .into_iter()
            .next()
            .ok_or(Error::NoInstallFound)?,
    };
    let flavor = discovery::select_flavor(&install, args.flavor.as_deref())?.clone();
    Ok((install, flavor))
}

/// Reports on one source: a line per addon, or one line saying why the source
/// could not be read at all.
fn source_lines(source: &Path, addons_dir: &Path, dry_run: bool) -> Vec<Line> {
    let lines = if dry_run {
        planned_lines(source, addons_dir)
    } else {
        installed_lines(source, addons_dir)
    };
    lines.unwrap_or_else(|error| vec![failure_line(source, &error)])
}

fn planned_lines(source: &Path, addons_dir: &Path) -> Result<Vec<Line>> {
    let (_staged, plan) = crate::plan_source(source, addons_dir)?;
    Ok(plan.addons.iter().map(planned_line).collect())
}

fn installed_lines(source: &Path, addons_dir: &Path) -> Result<Vec<Line>> {
    Ok(crate::install_source(source, addons_dir)?
        .iter()
        .map(outcome_line)
        .collect())
}

/// One reported addon, before it is laid out in columns.
struct Line {
    status: &'static str,
    name: String,
    detail: String,
    failed: bool,
}

fn planned_line(plan: &AddonPlan) -> Line {
    Line {
        status: if plan.replaces_existing {
            "replace"
        } else {
            "new"
        },
        name: plan.name.clone(),
        detail: describe_version(&plan.toc),
        failed: false,
    }
}

fn outcome_line(outcome: &Outcome) -> Line {
    match &outcome.result {
        Ok(installed) => Line {
            status: if installed.replaced {
                "updated"
            } else {
                "installed"
            },
            name: outcome.addon.clone(),
            detail: describe_version(&outcome.toc),
            failed: false,
        },
        Err(error) => Line {
            status: "failed",
            name: outcome.addon.clone(),
            detail: error.to_string(),
            failed: true,
        },
    }
}

fn failure_line(source: &Path, error: &Error) -> Line {
    Line {
        status: "failed",
        name: crate::fs_util::file_name_lossy(source),
        detail: error.to_string(),
        failed: true,
    }
}

fn describe_version(toc: &Toc) -> String {
    toc.version.clone().unwrap_or_default()
}

/// Prints the lines with their names in a column, so a run over several
/// sources still reads as one list.
fn print_lines(lines: &[Line]) {
    let status_width = lines
        .iter()
        .map(|line| line.status.len())
        .max()
        .unwrap_or(0);
    let name_width = lines
        .iter()
        .map(|line| line.name.chars().count())
        .max()
        .unwrap_or(0);
    for line in lines {
        let padding = " ".repeat(name_width.saturating_sub(line.name.chars().count()));
        println!(
            "  {:<status_width$}  {}{}  {}",
            line.status, line.name, padding, line.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_argument_definition_is_sound() {
        Args::command().debug_assert();
    }

    #[test]
    fn no_arguments_means_open_the_window() {
        let args = Args::parse_from(["simple-addons"]);

        assert!(wants_window(&args));
    }

    #[test]
    fn a_source_means_run_on_the_command_line() {
        let args = Args::parse_from(["simple-addons", "MyAddon.zip"]);

        assert_eq!(args.sources, vec![PathBuf::from("MyAddon.zip")]);
        assert!(!wants_window(&args));
    }

    #[test]
    fn listing_installs_does_not_open_the_window() {
        assert!(!wants_window(&Args::parse_from([
            "simple-addons",
            "--list"
        ])));
    }

    #[test]
    fn the_window_can_be_asked_for_alongside_sources() {
        assert!(wants_window(&Args::parse_from([
            "simple-addons",
            "--gui",
            "MyAddon.zip"
        ])));
    }

    #[test]
    fn reads_the_target_from_options() {
        let args = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            "/games/wow",
            "--flavor",
            "classic_era",
            "--dry-run",
            "MyAddon.zip",
        ]);

        assert_eq!(args.wow_dir, Some(PathBuf::from("/games/wow")));
        assert_eq!(args.flavor, Some("classic_era".to_string()));
        assert!(args.dry_run);
    }
}
