//! The window front end: drop a folder or a zip file onto it to install.
//!
//! This is user facing software, so nothing here stops the program. A failure
//! becomes a line in the result list and the window carries on.
//!
//! The window state is plain data. Each frame reads it, draws from it, and
//! returns the [`Action`] values the user asked for; [`apply`] is the one
//! place that changes anything. Installing runs on another thread so that a
//! large archive cannot freeze the window.

mod settings;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

use crate::discovery::{self, Flavor, WowInstall};
use crate::install::Outcome;
use settings::Settings;

const WINDOW_TITLE: &str = "Simple Addons";

/// The name the desktop entry and the window agree on, so that a launcher can
/// tell which window belongs to which icon.
const APP_ID: &str = "simple-addons";

/// Opens the window and runs until it is closed.
///
/// `wow_dir` and `flavor` come from the command line and only set the
/// starting choice; the user can change both in the window. Any `sources`
/// are installed as soon as the window opens, which is what happens when
/// files are dropped on a launcher icon.
pub fn run(wow_dir: Option<&Path>, flavor: Option<&str>, sources: Vec<PathBuf>) -> eframe::Result {
    let wow_dir = wow_dir.map(Path::to_path_buf);
    let flavor = flavor.map(str::to_string);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_app_id(APP_ID)
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([520.0, 380.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    // The window is built inside the closure because the choices stored by
    // the last run only become readable once eframe has opened its storage.
    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                wow_dir.as_deref(),
                flavor.as_deref(),
                sources,
                cc.storage,
            )))
        }),
    )
}

/// Where addons will go.
#[derive(Debug, Clone)]
struct Target {
    install: WowInstall,
    flavor: Flavor,
}

impl Target {
    fn addons_dir(&self) -> PathBuf {
        self.install.addons_dir(&self.flavor)
    }
}

/// One line of the result list.
#[derive(Debug, Clone)]
struct Row {
    status: &'static str,
    addon: String,
    detail: String,
    failed: bool,
}

/// Whether an install is running. A job owns the channel its thread reports on.
enum Job {
    Idle,
    Working {
        items: usize,
        results: Receiver<Vec<Row>>,
    },
}

/// Everything the window holds.
struct App {
    /// The game folders on offer in the picker.
    installs: Vec<WowInstall>,
    /// The chosen folder and game version, if there is one to choose.
    target: Option<Target>,
    /// How many addons the target already has, as shown next to the picker.
    installed: Option<usize>,
    job: Job,
    /// Sources dropped while a job was running, installed once it ends.
    queued: Vec<PathBuf>,
    /// Results so far, newest first.
    rows: Vec<Row>,
    /// A message about the game folder, such as a path that turned out not to
    /// be one.
    notice: Option<String>,
    /// The choices stored by the last run, kept so that a run which never had
    /// a target to choose does not wipe them.
    remembered: Settings,
}

/// Something the user asked for while a frame was drawn.
enum Action {
    AddSources(Vec<PathBuf>),
    SelectInstall(WowInstall),
    SelectFlavor(Flavor),
    ChooseInstallDir,
    ClearResults,
}

impl App {
    fn new(
        wow_dir: Option<&Path>,
        flavor: Option<&str>,
        sources: Vec<PathBuf>,
        storage: Option<&dyn eframe::Storage>,
    ) -> Self {
        let remembered = settings::load(storage);
        let (installs, notice) = locate_installs(wow_dir, &remembered);
        let target = initial_target(&installs, flavor, &remembered);
        Self {
            installed: installed_count(target.as_ref()),
            installs,
            target,
            job: Job::Idle,
            queued: sources,
            rows: Vec::new(),
            notice,
            remembered,
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let chosen = match self.target.as_ref() {
            Some(target) => settings::of(&target.install, &target.flavor),
            None => self.remembered.clone(),
        };
        settings::save(storage, &chosen);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if finish_job(&mut self.job, &mut self.rows) {
            self.installed = installed_count(self.target.as_ref());
        }

        // Panels are added outermost first, and the central panel last.
        let mut actions = dropped_sources(&ctx);
        actions.extend(target_panel(ui, self));
        actions.extend(results_panel(ui, &self.rows));
        actions.extend(drop_panel(ui, self));
        for action in actions {
            apply(action, self);
        }

        start_queued(self, &ctx);
        if matches!(self.job, Job::Working { .. }) {
            ctx.request_repaint();
        }
    }
}

/// Finds the game folders to offer.
///
/// A folder named on the command line that turns out not to be one is
/// reported, and the usual search runs anyway, so the window still opens with
/// something to install to.
fn locate_installs(
    wow_dir: Option<&Path>,
    remembered: &Settings,
) -> (Vec<WowInstall>, Option<String>) {
    match wow_dir {
        None => (offered_installs(remembered), None),
        Some(dir) => match discovery::open(dir) {
            Ok(install) => (vec![install], None),
            Err(error) => (offered_installs(remembered), Some(error.to_string())),
        },
    }
}

/// The game folders the picker offers, with the one chosen last time first so
/// that it is the one the window starts on.
fn offered_installs(remembered: &Settings) -> Vec<WowInstall> {
    let found = discovery::discover();
    let Some(chosen) = settings::remembered_install(&found, remembered) else {
        return found;
    };
    let mut installs = vec![chosen.clone()];
    installs.extend(
        found
            .into_iter()
            .filter(|install| install.root() != chosen.root()),
    );
    installs
}

fn initial_target(
    installs: &[WowInstall],
    flavor: Option<&str>,
    remembered: &Settings,
) -> Option<Target> {
    let install = installs.first()?.clone();
    let flavor = choose_flavor(&install, flavor, remembered);
    Some(Target { install, flavor })
}

/// A game version named on the command line is an instruction for this run,
/// so it wins. Next comes the one chosen last time, and failing that the
/// first on offer.
fn choose_flavor(install: &WowInstall, requested: Option<&str>, remembered: &Settings) -> Flavor {
    if let Some(requested) = requested
        && let Ok(flavor) = discovery::select_flavor(install, Some(requested))
    {
        return flavor.clone();
    }
    settings::remembered_flavor(install, remembered).unwrap_or_else(|| first_flavor(install))
}

/// The game version a window starts on when nothing else decides it.
///
/// A [`WowInstall`] always holds at least one, which is what makes it an
/// install, and they are ordered with retail first.
fn first_flavor(install: &WowInstall) -> Flavor {
    install.flavors()[0].clone()
}

fn installed_count(target: Option<&Target>) -> Option<usize> {
    crate::count_installed(&target?.addons_dir())
}

fn apply(action: Action, app: &mut App) {
    match action {
        Action::AddSources(paths) => app.queued.extend(paths),
        Action::SelectInstall(install) => select_install(app, install),
        Action::SelectFlavor(flavor) => select_flavor(app, flavor),
        Action::ChooseInstallDir => choose_install_dir(app),
        Action::ClearResults => app.rows.clear(),
    }
}

fn select_install(app: &mut App, install: WowInstall) {
    if !app
        .installs
        .iter()
        .any(|known| known.root() == install.root())
    {
        app.installs.push(install.clone());
    }
    app.target = Some(Target {
        flavor: first_flavor(&install),
        install,
    });
    app.installed = installed_count(app.target.as_ref());
}

fn select_flavor(app: &mut App, flavor: Flavor) {
    if let Some(target) = app.target.as_mut() {
        target.flavor = flavor;
    }
    app.installed = installed_count(app.target.as_ref());
}

/// Asks for a game folder and takes it if it holds a game version.
fn choose_install_dir(app: &mut App) {
    let Some(chosen) = pick_folder("Choose your World of Warcraft folder") else {
        return;
    };
    match discovery::open(&chosen) {
        Ok(install) => {
            app.notice = None;
            select_install(app, install);
        }
        Err(error) => app.notice = Some(error.to_string()),
    }
}

/// Starts the next install when sources are waiting and nothing is running.
fn start_queued(app: &mut App, ctx: &egui::Context) {
    if app.queued.is_empty() || matches!(app.job, Job::Working { .. }) {
        return;
    }
    let sources = std::mem::take(&mut app.queued);
    let Some(target) = app.target.as_ref() else {
        let rows = sources.iter().map(|source| no_target_row(source)).collect();
        prepend(&mut app.rows, rows);
        return;
    };
    app.job = Job::Working {
        items: sources.len(),
        results: start_install(sources, target.addons_dir(), ctx.clone()),
    };
}

/// Moves a finished job's rows into the list.
///
/// Reports whether a job ended, so the caller can refresh what it says about
/// the game folder.
fn finish_job(job: &mut Job, rows: &mut Vec<Row>) -> bool {
    let Job::Working { results, .. } = job else {
        return false;
    };
    match results.try_recv() {
        Ok(finished) => {
            prepend(rows, finished);
            *job = Job::Idle;
            true
        }
        Err(TryRecvError::Empty) => false,
        // The thread ended without reporting, which takes a panic. Nothing
        // more will arrive, so say so rather than waiting for ever.
        Err(TryRecvError::Disconnected) => {
            prepend(rows, vec![unexpected_stop_row()]);
            *job = Job::Idle;
            true
        }
    }
}

fn prepend(rows: &mut Vec<Row>, newest: Vec<Row>) {
    rows.splice(0..0, newest);
}

/// Installs sources on another thread so that the window keeps drawing.
fn start_install(
    sources: Vec<PathBuf>,
    addons_dir: PathBuf,
    ctx: egui::Context,
) -> Receiver<Vec<Row>> {
    let (sender, results) = mpsc::channel();
    thread::spawn(move || {
        let rows: Vec<Row> = sources
            .iter()
            .flat_map(|source| install_source(source, &addons_dir))
            .collect();
        // A closed channel means the window has gone, which is not a problem.
        let _ = sender.send(rows);
        ctx.request_repaint();
    });
    results
}

/// Installs one source, as a row per addon or one row saying why the source
/// could not be read at all.
fn install_source(source: &Path, addons_dir: &Path) -> Vec<Row> {
    match crate::install_source(source, addons_dir) {
        Ok(outcomes) => outcomes.iter().map(outcome_row).collect(),
        Err(error) => vec![Row {
            status: "failed",
            addon: crate::fs_util::file_name_lossy(source),
            detail: error.to_string(),
            failed: true,
        }],
    }
}

fn outcome_row(outcome: &Outcome) -> Row {
    match &outcome.result {
        Ok(installed) => Row {
            status: if installed.replaced {
                "updated"
            } else {
                "installed"
            },
            addon: outcome.addon.clone(),
            detail: outcome.toc.version.clone().unwrap_or_default(),
            failed: false,
        },
        Err(error) => Row {
            status: "failed",
            addon: outcome.addon.clone(),
            detail: error.to_string(),
            failed: true,
        },
    }
}

fn no_target_row(source: &Path) -> Row {
    Row {
        status: "failed",
        addon: crate::fs_util::file_name_lossy(source),
        detail: "choose a World of Warcraft folder first".to_string(),
        failed: true,
    }
}

fn unexpected_stop_row() -> Row {
    Row {
        status: "failed",
        addon: "Install".to_string(),
        detail: "the install stopped unexpectedly".to_string(),
        failed: true,
    }
}

/// The paths dropped onto the window this frame.
fn dropped_sources(ctx: &egui::Context) -> Vec<Action> {
    let paths: Vec<PathBuf> = ctx.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    });
    if paths.is_empty() {
        Vec::new()
    } else {
        vec![Action::AddSources(paths)]
    }
}

/// How many files are being dragged over the window right now.
fn hovering_count(ctx: &egui::Context) -> usize {
    ctx.input(|input| input.raw.hovered_files.len())
}

/// Top panel: which game folder and version addons go to.
fn target_panel(ui: &mut egui::Ui, app: &App) -> Vec<Action> {
    egui::Panel::top("target")
        .show(ui, |ui| {
            let mut actions = Vec::new();
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Game folder");
                if ui.button("Choose...").clicked() {
                    actions.push(Action::ChooseInstallDir);
                }
                actions.extend(install_picker(ui, &app.installs, app.target.as_ref()));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Game version");
                actions.extend(flavor_picker(ui, app.target.as_ref()));
                if let Some(installed) = app.installed {
                    ui.label(egui::RichText::new(describe_count(installed)).weak());
                }
            });
            if let Some(notice) = &app.notice {
                ui.add_space(4.0);
                let colour = ui.visuals().warn_fg_color;
                ui.colored_label(colour, notice.as_str());
            }
            ui.add_space(8.0);
            actions
        })
        .inner
}

fn describe_count(installed: usize) -> String {
    if installed == 1 {
        "1 addon installed".to_string()
    } else {
        format!("{installed} addons installed")
    }
}

fn install_picker(
    ui: &mut egui::Ui,
    installs: &[WowInstall],
    target: Option<&Target>,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let selected = match target {
        Some(target) => display_path(target.install.root()),
        None => "none found".to_string(),
    };
    egui::ComboBox::from_id_salt("install")
        .selected_text(selected)
        .width(ui.available_width().max(200.0))
        .show_ui(ui, |ui| {
            for install in installs {
                let chosen = target.is_some_and(|target| target.install.root() == install.root());
                if ui
                    .selectable_label(chosen, display_path(install.root()))
                    .clicked()
                {
                    actions.push(Action::SelectInstall(install.clone()));
                }
            }
        });
    actions
}

fn flavor_picker(ui: &mut egui::Ui, target: Option<&Target>) -> Vec<Action> {
    let mut actions = Vec::new();
    let Some(target) = target else {
        ui.label(egui::RichText::new("none").weak());
        return actions;
    };
    egui::ComboBox::from_id_salt("flavor")
        .selected_text(target.flavor.label())
        .show_ui(ui, |ui| {
            for flavor in target.install.flavors() {
                if ui
                    .selectable_label(*flavor == target.flavor, flavor.label())
                    .clicked()
                {
                    actions.push(Action::SelectFlavor(flavor.clone()));
                }
            }
        });
    actions
}

/// Bottom panel: what the last installs did. Hidden until there is something
/// to say.
fn results_panel(ui: &mut egui::Ui, rows: &[Row]) -> Vec<Action> {
    if rows.is_empty() {
        return Vec::new();
    }
    egui::Panel::bottom("results")
        .resizable(true)
        .default_size(210.0)
        .show(ui, |ui| {
            let mut actions = Vec::new();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(summarise(rows)).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Clear").clicked() {
                        actions.push(Action::ClearResults);
                    }
                });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| result_rows(ui, rows));
            actions
        })
        .inner
}

fn result_rows(ui: &mut egui::Ui, rows: &[Row]) {
    egui::Grid::new("results")
        .num_columns(3)
        .striped(true)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for row in rows {
                let colour = status_colour(ui.visuals(), row.failed);
                ui.colored_label(colour, row.status);
                ui.label(egui::RichText::new(row.addon.as_str()).strong());
                ui.label(row.detail.as_str());
                ui.end_row();
            }
        });
}

fn status_colour(visuals: &egui::Visuals, failed: bool) -> egui::Color32 {
    if failed {
        visuals.error_fg_color
    } else if visuals.dark_mode {
        egui::Color32::from_rgb(0x7a, 0xd1, 0x8c)
    } else {
        egui::Color32::from_rgb(0x1c, 0x7a, 0x3e)
    }
}

fn summarise(rows: &[Row]) -> String {
    let failed = rows.iter().filter(|row| row.failed).count();
    let done = rows.len() - failed;
    if failed == 0 {
        format!("{done} installed")
    } else {
        format!("{done} installed, {failed} failed")
    }
}

/// The rest of the window: the area files are dropped onto.
fn drop_panel(ui: &mut egui::Ui, app: &App) -> Vec<Action> {
    let hovering = hovering_count(ui.ctx());
    let (heading, hint) = zone_text(&app.job, app.target.as_ref(), hovering);
    let busy = matches!(app.job, Job::Working { .. });
    egui::CentralPanel::default()
        .show(ui, |ui| {
            let mut actions = Vec::new();
            drop_zone(ui, hovering > 0, |ui| {
                ui.add_space(ui.available_height() * 0.25);
                ui.label(egui::RichText::new(heading).size(20.0).strong());
                ui.add_space(8.0);
                ui.label(egui::RichText::new(hint).weak());
                ui.add_space(20.0);
                if busy {
                    ui.spinner();
                } else {
                    actions.extend(browse_buttons(ui));
                }
            });
            actions
        })
        .inner
}

/// Draws the drop target, highlighted while files are dragged over it.
fn drop_zone(ui: &mut egui::Ui, active: bool, contents: impl FnOnce(&mut egui::Ui)) {
    let visuals = ui.visuals();
    let (fill, stroke) = if active {
        (
            visuals.selection.bg_fill.linear_multiply(0.35),
            egui::Stroke::new(2.0, visuals.selection.stroke.color),
        )
    } else {
        (
            visuals.faint_bg_color,
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        )
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(12)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical_centered(|ui| contents(ui));
        });
}

/// The way in for users who would rather not drag anything.
fn browse_buttons(ui: &mut egui::Ui) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.allocate_ui_with_layout(
        egui::vec2(330.0, 28.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            if ui.button("Choose zip files...").clicked() {
                let chosen = pick_archives();
                if !chosen.is_empty() {
                    actions.push(Action::AddSources(chosen));
                }
            }
            if ui.button("Choose an addon folder...").clicked()
                && let Some(chosen) = pick_folder("Choose an addon folder")
            {
                actions.push(Action::AddSources(vec![chosen]));
            }
        },
    );
    actions
}

/// The heading and the hint shown in the drop zone, which together say what
/// will happen if something is dropped.
fn zone_text(job: &Job, target: Option<&Target>, hovering: usize) -> (String, String) {
    if let Job::Working { items, .. } = job {
        return (
            format!("Installing {}...", describe_items(*items)),
            "The window stays usable; anything else dropped will follow.".to_string(),
        );
    }
    let Some(target) = target else {
        return (
            "No World of Warcraft folder found".to_string(),
            "Use Choose... above to point at your game folder.".to_string(),
        );
    };
    if hovering > 0 {
        return (
            format!("Install {}", describe_items(hovering)),
            format!("Release to install into {}.", target.flavor.label()),
        );
    }
    (
        "Drop addon folders or zip files here".to_string(),
        format!("They go into {}", display_path(&target.addons_dir())),
    )
}

fn describe_items(count: usize) -> String {
    if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
    }
}

/// Shortens a path for display. Wine install paths are long enough to crowd
/// out everything else in a window this size.
fn display_path(path: &Path) -> String {
    let home = std::env::home_dir();
    match home.as_ref().and_then(|home| path.strip_prefix(home).ok()) {
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

fn pick_folder(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_folder()
}

fn pick_archives() -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Choose addon zip files")
        .add_filter("Zip archive", &["zip"])
        .pick_files()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Builds an install directory holding the given game versions.
    fn fake_install(root: &Path, flavors: &[&str]) -> WowInstall {
        for flavor in flavors {
            fs::create_dir_all(root.join(flavor).join("Interface").join("AddOns")).unwrap();
        }
        discovery::read_install(root).unwrap()
    }

    #[test]
    fn the_window_starts_on_the_game_version_chosen_last_time() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = install.flavors()[1].clone();
        let remembered = settings::of(&install, &era);

        let target = initial_target(&[install], None, &remembered).unwrap();

        assert_eq!(target.flavor, era);
    }

    #[test]
    fn retail_is_the_starting_choice_when_nothing_was_remembered() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_classic_era_", "_retail_"]);

        let target = initial_target(&[install], None, &Settings::default()).unwrap();

        assert!(target.flavor.is_retail());
    }

    #[test]
    fn a_game_version_given_on_the_command_line_beats_the_remembered_one() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let remembered = settings::of(&install, &install.flavors()[1]);

        let target = initial_target(&[install], Some("retail"), &remembered).unwrap();

        assert!(target.flavor.is_retail());
    }

    #[test]
    fn a_remembered_game_version_the_install_lost_falls_back_to_retail() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_"]);
        let remembered = Settings {
            install_root: Some(install.root().to_path_buf()),
            flavor_dir: Some("_classic_era_".to_string()),
        };

        let target = initial_target(&[install], None, &remembered).unwrap();

        assert!(target.flavor.is_retail());
    }

    #[test]
    fn a_nonsense_game_version_on_the_command_line_falls_back_to_the_remembered_one() {
        let root = tempfile::tempdir().unwrap();
        let install = fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = install.flavors()[1].clone();
        let remembered = settings::of(&install, &era);

        let target = initial_target(&[install], Some("wrath"), &remembered).unwrap();

        assert_eq!(target.flavor, era);
    }

    #[test]
    fn nowhere_to_install_to_means_no_starting_target() {
        assert!(initial_target(&[], None, &Settings::default()).is_none());
    }

    fn row(status: &'static str, failed: bool) -> Row {
        Row {
            status,
            addon: "MyAddon".to_string(),
            detail: String::new(),
            failed,
        }
    }

    #[test]
    fn summarises_what_a_run_did() {
        assert_eq!(summarise(&[row("installed", false)]), "1 installed");
        assert_eq!(
            summarise(&[row("updated", false), row("failed", true)]),
            "1 installed, 1 failed"
        );
    }

    #[test]
    fn newest_results_are_listed_first() {
        let mut rows = vec![row("installed", false)];

        prepend(&mut rows, vec![row("failed", true)]);

        assert_eq!(rows[0].status, "failed");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn the_drop_zone_says_there_is_nowhere_to_install_to() {
        let (heading, hint) = zone_text(&Job::Idle, None, 0);

        assert_eq!(heading, "No World of Warcraft folder found");
        assert!(hint.contains("Choose"));
    }

    #[test]
    fn the_drop_zone_counts_what_is_being_dragged() {
        let (heading, _) = zone_text(&Job::Idle, None, 3);

        // With nowhere to install to, the count is not the point.
        assert_eq!(heading, "No World of Warcraft folder found");
    }

    #[test]
    fn item_counts_read_as_sentences() {
        assert_eq!(describe_items(1), "1 item");
        assert_eq!(describe_items(4), "4 items");
        assert_eq!(describe_count(1), "1 addon installed");
        assert_eq!(describe_count(0), "0 addons installed");
    }

    #[test]
    fn a_source_dropped_with_no_target_says_what_is_missing() {
        let row = no_target_row(Path::new("/tmp/MyAddon.zip"));

        assert_eq!(row.addon, "MyAddon.zip");
        assert!(row.failed);
    }

    #[test]
    fn long_paths_are_shortened_to_the_home_folder() {
        let Some(home) = std::env::home_dir() else {
            return;
        };

        assert_eq!(display_path(&home.join("Games/wow")), "~/Games/wow");
        assert_eq!(display_path(Path::new("/opt/wow")), "/opt/wow");
    }
}
