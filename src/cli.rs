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
use crate::installed::{self, InstalledAddon};
use crate::package::Toc;
use crate::remove;

#[derive(Debug, Parser)]
#[command(
    name = "simple-addons",
    version,
    about = "Install World of Warcraft addons from a folder or a zip file, and remove them",
    after_help = "Run with no arguments to open the window and install by drag and drop."
)]
pub struct Args {
    /// Folders or zip files to install. Leave empty to open the window.
    #[arg(value_name = "SOURCE")]
    pub sources: Vec<PathBuf>,

    /// World of Warcraft directory. Found automatically when left out.
    #[arg(short = 'w', long, value_name = "PATH", env = "SIMPLE_ADDONS_WOW_DIR")]
    pub wow_dir: Option<PathBuf>,

    /// Game version to install into or remove from, such as retail or
    /// classic_era.
    #[arg(short, long, value_name = "NAME")]
    pub flavor: Option<String>,

    /// Report what would be installed or removed without changing anything.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Remove these addons, named as their folders are, instead of
    /// installing anything.
    #[arg(
        short,
        long,
        value_name = "ADDON",
        num_args = 1..,
        conflicts_with_all = ["sources", "gui", "list", "list_addons"]
    )]
    pub remove: Vec<String>,

    /// List the World of Warcraft directories that were found, then stop.
    #[arg(long)]
    pub list: bool,

    /// List the addons installed in the game version, then stop.
    #[arg(long, conflicts_with_all = ["sources", "gui", "list"])]
    pub list_addons: bool,

    /// Open the window even when sources are given.
    #[arg(long)]
    pub gui: bool,
}

/// True when the window should open rather than the command line running.
pub fn wants_window(args: &Args) -> bool {
    let has_work =
        !args.sources.is_empty() || !args.remove.is_empty() || args.list || args.list_addons;
    args.gui || !has_work
}

pub fn run(args: &Args) -> ExitCode {
    if args.list {
        return report(list_installs(args.wow_dir.as_deref()));
    }
    if args.list_addons {
        return report(list_addons(args));
    }
    if !args.remove.is_empty() {
        return report(remove_all(args));
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
    let (install, flavor) = resolve_target(args, OnSeveral::TakeFirst)?;
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
    Ok(exit_code(&lines))
}

fn exit_code(lines: &[Line]) -> ExitCode {
    if lines.iter().any(|line| line.failed) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What to do when no `--wow-dir` is given and the search finds several
/// game folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnSeveral {
    /// Use the first, which the report then names. Fine for anything that
    /// can be undone.
    TakeFirst,
    /// Refuse to guess. A removal cannot be undone, so it has to happen in
    /// the folder the user meant.
    Refuse,
}

/// The game folder and version a run works on.
///
/// A `--wow-dir` inside a game version folder names that version, as
/// plainly as it names the install, so it is the one used unless `--flavor`
/// names another.
fn resolve_target(args: &Args, on_several: OnSeveral) -> Result<(WowInstall, Flavor)> {
    let install = match &args.wow_dir {
        Some(dir) => discovery::open(dir)?,
        None => pick_install(discovery::discover(), on_several)?,
    };
    let flavor = match (args.flavor.as_deref(), args.wow_dir.as_deref()) {
        (Some(requested), _) => discovery::select_flavor(&install, Some(requested))?,
        (None, Some(dir)) => match discovery::flavor_containing(&install, dir)? {
            Some(implied) => implied,
            None => discovery::select_flavor(&install, None)?,
        },
        (None, None) => discovery::select_flavor(&install, None)?,
    };
    let flavor = flavor.clone();
    Ok((install, flavor))
}

fn pick_install(found: Vec<WowInstall>, on_several: OnSeveral) -> Result<WowInstall> {
    if on_several == OnSeveral::Refuse && found.len() > 1 {
        let roots: Vec<String> = found
            .iter()
            .map(|install| install.root().display().to_string())
            .collect();
        return Err(Error::SeveralInstallsFound {
            found: roots.join(", "),
        });
    }
    found.into_iter().next().ok_or(Error::NoInstallFound)
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
        name: plan.name.to_string(),
        detail: describe_version(&plan.toc),
        failed: false,
    }
}

/// An update that left files of the old copy behind still installed the new
/// one, but the run fails so that a script notices the files that need
/// deleting by hand.
fn outcome_line(outcome: &Outcome) -> Line {
    match &outcome.result {
        Ok(installed) => Line {
            status: if installed.replaced {
                "updated"
            } else {
                "installed"
            },
            name: outcome.addon.clone(),
            detail: crate::install::describe_installed(&outcome.toc, installed),
            failed: installed.left_behind.is_some(),
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

fn list_addons(args: &Args) -> Result<ExitCode> {
    let (install, flavor) = resolve_target(args, OnSeveral::TakeFirst)?;
    let addons = list_installed(&install.addons_dir(&flavor))?;
    println!("{} ({})", install.root().display(), flavor.label());
    if addons.is_empty() {
        println!("  no addons installed");
    }
    let rows: Vec<[String; 3]> = addons
        .iter()
        .map(|addon| {
            [
                addon.name().to_string(),
                addon.version().unwrap_or_default().to_string(),
                installed::describe(addon),
            ]
        })
        .collect();
    print_columns(&rows);
    Ok(ExitCode::SUCCESS)
}

/// The addons installed in a game version.
///
/// A game version that has never had an addon has no folder for them, which
/// means that nothing is installed rather than that something is wrong.
fn list_installed(addons_dir: &Path) -> Result<Vec<InstalledAddon>> {
    if addons_dir.is_dir() {
        installed::list(addons_dir)
    } else {
        Ok(Vec::new())
    }
}

fn remove_all(args: &Args) -> Result<ExitCode> {
    let (install, flavor) = resolve_target(args, OnSeveral::Refuse)?;
    let addons_dir = install.addons_dir(&flavor);
    let installed = list_installed(&addons_dir)?;
    println!(
        "{} {} ({})",
        if args.dry_run {
            "Would remove from"
        } else {
            "Removing from"
        },
        install.root().display(),
        flavor.label()
    );

    let (chosen, mut unknown) = choose_addons(&installed, &args.remove, &addons_dir);
    let (mut lines, removed) = if args.dry_run {
        (chosen.iter().map(planned_removal_line).collect(), chosen)
    } else {
        remove_chosen(&addons_dir, &chosen)?
    };
    lines.append(&mut unknown);
    print_lines(&lines);
    print_still_needed(&installed::dependents(&installed, &removed), args.dry_run);
    Ok(exit_code(&lines))
}

/// Matches the names given to installed addons. An addon named twice is
/// removed once, and a name that matches nothing becomes a failure line.
fn choose_addons(
    installed: &[InstalledAddon],
    names: &[String],
    addons_dir: &Path,
) -> (Vec<InstalledAddon>, Vec<Line>) {
    let mut chosen: Vec<InstalledAddon> = Vec::new();
    let mut unknown = Vec::new();
    for name in names {
        match installed::find(installed, name, addons_dir) {
            Ok(addon) if !chosen.contains(addon) => chosen.push(addon.clone()),
            Ok(_) => {}
            Err(error) => unknown.push(Line {
                status: "failed",
                name: name.clone(),
                detail: error.to_string(),
                failed: true,
            }),
        }
    }
    (chosen, unknown)
}

/// Removes the chosen addons, and says which of them are gone.
fn remove_chosen(
    addons_dir: &Path,
    chosen: &[InstalledAddon],
) -> Result<(Vec<Line>, Vec<InstalledAddon>)> {
    // Getting ready to remove locks the Interface folder and makes the
    // staging folder in it, which is not worth doing when every name was
    // unknown.
    if chosen.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let outcomes = remove::execute(addons_dir, chosen)?;
    let lines = outcomes.iter().map(removal_line).collect();
    let removed = outcomes
        .into_iter()
        .filter(|outcome| outcome.result.is_ok())
        .map(|outcome| outcome.addon)
        .collect();
    Ok((lines, removed))
}

fn planned_removal_line(addon: &InstalledAddon) -> Line {
    Line {
        status: "remove",
        name: addon.name().to_string(),
        detail: remove::describe(addon),
        failed: false,
    }
}

/// A removed addon whose files could not all be deleted still counts as
/// removed, since the game no longer loads it, but the run fails so that a
/// script notices the files that need deleting by hand.
fn removal_line(outcome: &remove::Outcome) -> Line {
    match &outcome.result {
        Ok(removed) => Line {
            status: "removed",
            name: outcome.addon.name().to_string(),
            detail: remove::describe_removed(&outcome.addon, removed),
            failed: removed.left_behind.is_some(),
        },
        Err(error) => Line {
            status: "failed",
            name: outcome.addon.name().to_string(),
            detail: error.to_string(),
            failed: true,
        },
    }
}

/// Names the addons that are still installed but need one that is gone, so
/// the game will not load them. They are left alone, because whether to
/// remove them too is for the user to say.
fn print_still_needed(still_needed: &[InstalledAddon], dry_run: bool) {
    if still_needed.is_empty() {
        return;
    }
    if dry_run {
        println!(
            "These addons need an addon that would be removed, so the game would not load them:"
        );
    } else {
        println!("These addons need an addon that was removed, so the game will not load them:");
    }
    let rows: Vec<[&str; 2]> = still_needed
        .iter()
        .map(|addon| [addon.name(), addon.version().unwrap_or_default()])
        .collect();
    print_columns(&rows);
    println!("To remove them as well, add their names to --remove.");
}

/// Prints the lines with their names in a column, so a run over several
/// sources still reads as one list.
fn print_lines(lines: &[Line]) {
    let rows: Vec<[&str; 3]> = lines
        .iter()
        .map(|line| [line.status, line.name.as_str(), line.detail.as_str()])
        .collect();
    print_columns(&rows);
}

/// Prints rows indented, with every column but the last padded to its
/// widest entry, so that the entries line up.
fn print_columns<S: AsRef<str>, const N: usize>(rows: &[[S; N]]) {
    let widths: [usize; N] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].as_ref().chars().count())
            .max()
            .unwrap_or(0)
    });
    for row in rows {
        println!("{}", format_row(row, &widths));
    }
}

fn format_row<S: AsRef<str>, const N: usize>(row: &[S; N], widths: &[usize; N]) -> String {
    let mut line = String::new();
    for (column, cell) in row.iter().enumerate() {
        let cell = cell.as_ref();
        line.push_str("  ");
        line.push_str(cell);
        if column + 1 < N {
            line.push_str(&" ".repeat(widths[column].saturating_sub(cell.chars().count())));
        }
    }
    // An empty last column, such as an addon with no version, would leave
    // the padding of the one before it hanging off the end.
    line.trim_end().to_string()
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

    #[test]
    fn removing_takes_several_names_and_does_not_open_the_window() {
        let args = Args::parse_from([
            "simple-addons",
            "--remove",
            "WeakAuras",
            "Details",
            "--dry-run",
        ]);

        assert_eq!(args.remove, vec!["WeakAuras", "Details"]);
        assert!(args.dry_run);
        assert!(!wants_window(&args));
    }

    #[test]
    fn listing_addons_does_not_open_the_window() {
        assert!(!wants_window(&Args::parse_from([
            "simple-addons",
            "--list-addons"
        ])));
    }

    #[test]
    fn one_run_either_installs_or_removes() {
        let both = Args::try_parse_from(["simple-addons", "MyAddon.zip", "--remove", "Other"]);
        let in_the_window = Args::try_parse_from(["simple-addons", "--gui", "--remove", "Other"]);

        assert!(both.is_err());
        assert!(in_the_window.is_err());
    }

    #[test]
    fn columns_line_up_and_leave_no_space_at_the_end() {
        let widths = [7, 9, 0];

        assert_eq!(
            format_row(&["removed", "WeakAuras", "5.19.1"], &widths),
            "  removed  WeakAuras  5.19.1"
        );
        assert_eq!(
            format_row(&["remove", "Details", ""], &widths),
            "  remove   Details"
        );
    }

    /// Builds a World of Warcraft directory with these addons installed in
    /// retail, each with the toc header given.
    fn fake_wow(root: &Path, addons: &[(&str, &str)]) -> PathBuf {
        let addons_dir = root.join("_retail_").join("Interface").join("AddOns");
        for (name, toc) in addons {
            std::fs::create_dir_all(addons_dir.join(name)).unwrap();
            std::fs::write(addons_dir.join(name).join(format!("{name}.toc")), toc).unwrap();
        }
        std::fs::create_dir_all(&addons_dir).unwrap();
        addons_dir
    }

    #[test]
    fn a_name_given_twice_is_removed_once_and_an_unknown_one_fails() {
        let root = tempfile::tempdir().unwrap();
        let addons_dir = fake_wow(root.path(), &[("WeakAuras", "")]);
        let installed = installed::list(&addons_dir).unwrap();
        let names = ["WeakAuras", "weakauras", "Details"].map(String::from);

        let (chosen, unknown) = choose_addons(&installed, &names, &addons_dir);

        assert_eq!(chosen.len(), 1);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].name, "Details");
        assert!(unknown[0].failed);
    }

    #[test]
    fn removes_named_addons_from_the_chosen_game_version() {
        let root = tempfile::tempdir().unwrap();
        let addons_dir = fake_wow(
            root.path(),
            &[
                ("WeakAuras", ""),
                ("WeakAurasOptions", "## Dependencies: WeakAuras\n"),
            ],
        );
        let args = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            root.path().to_str().unwrap(),
            "--remove",
            "weakauras",
        ]);

        let code = remove_all(&args).unwrap();

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!addons_dir.join("WeakAuras").exists());
        assert!(
            addons_dir.join("WeakAurasOptions").exists(),
            "an addon that needs a removed one is named, not removed"
        );
    }

    #[test]
    fn a_wow_dir_inside_a_game_version_removes_from_that_game_version() {
        let root = tempfile::tempdir().unwrap();
        let retail = fake_wow(root.path(), &[("Questie", "")]);
        let era = root.path().join("_classic_era_/Interface/AddOns");
        std::fs::create_dir_all(era.join("Questie")).unwrap();
        std::fs::write(era.join("Questie/Questie.toc"), "").unwrap();
        let args = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            era.to_str().unwrap(),
            "--remove",
            "Questie",
        ]);

        let code = remove_all(&args).unwrap();

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!era.join("Questie").exists());
        assert!(retail.join("Questie").exists());
    }

    #[test]
    fn a_wow_dir_in_a_game_version_it_cannot_see_removes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let retail = fake_wow(root.path(), &[("Questie", "")]);
        // A game version folder without its Interface folder is not one the
        // search recognises.
        let unseen = root.path().join("_classic_era_");
        std::fs::create_dir_all(&unseen).unwrap();
        let args = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            unseen.to_str().unwrap(),
            "--remove",
            "Questie",
        ]);

        let error = remove_all(&args).unwrap_err();

        assert!(matches!(error, Error::NotInGameVersion { .. }));
        assert!(retail.join("Questie").exists());
    }

    #[test]
    fn a_flavor_given_outright_beats_the_one_the_wow_dir_lies_in() {
        let root = tempfile::tempdir().unwrap();
        fake_wow(root.path(), &[]);
        let era = root.path().join("_classic_era_/Interface/AddOns");
        std::fs::create_dir_all(&era).unwrap();
        let in_era = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            era.to_str().unwrap(),
            "--list-addons",
        ]);
        let retail = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            era.to_str().unwrap(),
            "--flavor",
            "retail",
            "--list-addons",
        ]);

        let (_, implied) = resolve_target(&in_era, OnSeveral::TakeFirst).unwrap();
        let (_, given) = resolve_target(&retail, OnSeveral::TakeFirst).unwrap();

        assert_eq!(implied.dir_name(), "_classic_era_");
        assert!(given.is_retail());
    }

    #[test]
    fn a_removal_will_not_guess_between_several_game_folders() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        fake_wow(one.path(), &[]);
        fake_wow(two.path(), &[]);
        let found = || {
            [one.path(), two.path()]
                .map(|root| discovery::read_install(root).unwrap())
                .to_vec()
        };

        let taken = pick_install(found(), OnSeveral::TakeFirst).unwrap();
        let refused = pick_install(found(), OnSeveral::Refuse);

        assert_eq!(taken.root(), one.path());
        assert!(matches!(refused, Err(Error::SeveralInstallsFound { .. })));
        assert!(pick_install(found()[..1].to_vec(), OnSeveral::Refuse).is_ok());
    }

    #[test]
    fn a_dry_run_removes_nothing_and_an_unknown_name_fails_the_run() {
        let root = tempfile::tempdir().unwrap();
        let addons_dir = fake_wow(root.path(), &[("WeakAuras", "")]);
        let args = Args::parse_from([
            "simple-addons",
            "--wow-dir",
            root.path().to_str().unwrap(),
            "--dry-run",
            "--remove",
            "WeakAuras",
            "Details",
        ]);

        let code = remove_all(&args).unwrap();

        assert_eq!(code, ExitCode::FAILURE);
        assert!(addons_dir.join("WeakAuras").exists());
    }
}
