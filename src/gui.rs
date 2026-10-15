//! The window front end: drop a folder or a zip file onto it to install, or
//! tick addons in a list to remove them.
//!
//! This is user facing software, so nothing here stops the program. A failure
//! becomes a line in the result list and the window carries on.
//!
//! The window state is plain data. Each frame reads it, draws from it, and
//! returns the [`Action`] values the user asked for; [`apply`] is the one
//! place that changes anything. Installs and removals run on another thread
//! so that a large archive cannot freeze the window.

mod addon_list;
mod backend;
mod drop_zone;
mod job;
mod results;
mod settings;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eframe::egui;

use crate::discovery::{self, Flavor, WowInstall};
use crate::installed::{self, InstalledAddon};
use addon_list::{Confirmation, ListState};
use backend::Backend;
use job::{Job, Removal};
use results::Row;
use settings::Settings;

const WINDOW_TITLE: &str = "Simple Addons";

/// The name the desktop entry and the window agree on, so that a launcher can
/// tell which window belongs to which icon.
const APP_ID: &str = "simple-addons";

/// A window that would not open, and what to try about it.
///
/// There is no window to report this in, so the caller has to put it on the
/// terminal. The advice is separate from the error because it is about the
/// choice this program made, not about what winit then ran into.
#[derive(Debug)]
pub struct WindowFailed {
    pub error: eframe::Error,
    pub advice: Option<&'static str>,
}

/// Opens the window and runs until it is closed.
///
/// `wow_dir` and `flavor` come from the command line and only set the
/// starting choice; the user can change both in the window. Any `sources`
/// are installed as soon as the window opens, which is what happens when
/// files are dropped on a launcher icon.
pub fn run(
    wow_dir: Option<&Path>,
    flavor: Option<&str>,
    sources: Vec<PathBuf>,
) -> Result<(), WindowFailed> {
    let wow_dir = wow_dir.map(Path::to_path_buf);
    let flavor = flavor.map(str::to_string);
    let chosen = backend::choose(&backend::read_session());
    let drops_arrive = backend::reports_drops(chosen);
    // The window is built inside the closure because the choices stored by
    // the last run only become readable once eframe has opened its storage.
    eframe::run_native(
        WINDOW_TITLE,
        window_options(chosen),
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                wow_dir.as_deref(),
                flavor.as_deref(),
                sources,
                drops_arrive,
                cc.storage,
            )))
        }),
    )
    .map_err(|error| WindowFailed {
        error,
        advice: backend::advice(chosen),
    })
}

fn window_options(chosen: Backend) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_app_id(APP_ID)
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([520.0, 380.0])
            .with_drag_and_drop(true),
        event_loop_builder: backend::force(chosen),
        ..Default::default()
    }
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

/// Everything the window holds.
struct App {
    /// The game folders on offer in the picker.
    installs: Vec<WowInstall>,
    /// The chosen folder and game version, if there is one to choose.
    target: Option<Target>,
    /// What the target already has, as the count next to the picker and the
    /// Installed addons tab show it, or why its folder could not be read.
    listing: Result<Vec<InstalledAddon>, String>,
    job: Job,
    /// Sources dropped while a job was running, installed once it ends.
    queued: Vec<PathBuf>,
    /// A removal confirmed while a job was running, started once it ends.
    queued_removal: Option<Removal>,
    /// Results so far, newest first.
    rows: Vec<Row>,
    /// A message about the game folder, such as a path that turned out not to
    /// be one.
    notice: Option<String>,
    /// Whether dropped files reach this window at all, which depends on the
    /// backend it opened on. False means the drop zone has to send the user
    /// to the buttons instead of inviting a drop that will never land.
    drops_arrive: bool,
    /// The choices stored by the last run, kept so that a run which never had
    /// a target to choose does not wipe them.
    remembered: Settings,
    /// The tab the main area shows.
    view: View,
    /// The filter, the ticks and the dialog of the Installed addons tab.
    list: ListState,
}

/// The tabs of the main area.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    Install,
    Addons,
}

/// Something the user asked for while a frame was drawn.
enum Action {
    AddSources(Vec<PathBuf>),
    SelectInstall(WowInstall),
    SelectFlavor(Flavor),
    ChooseInstallDir,
    ClearResults,
    ShowView(View),
    SetFilter(String),
    Tick { addon: PathBuf, ticked: bool },
    AskToRemove,
    SetRemoveDependents(bool),
    CancelRemoval,
    ConfirmRemoval,
}

impl App {
    fn new(
        wow_dir: Option<&Path>,
        flavor: Option<&str>,
        sources: Vec<PathBuf>,
        drops_arrive: bool,
        storage: Option<&dyn eframe::Storage>,
    ) -> Self {
        let remembered = settings::load(storage);
        let (installs, notice) = locate_installs(wow_dir, &remembered);
        let (flavor, wow_dir_notice) = match flavor {
            Some(requested) => (Some(requested), None),
            None => match implied_flavor(wow_dir, &installs) {
                Ok(implied) => (implied, None),
                Err(error) => (None, Some(wow_dir_notice(&error))),
            },
        };
        let notice = notice.or(wow_dir_notice);
        let target = initial_target(&installs, flavor, &remembered);
        Self {
            listing: read_listing(target.as_ref()),
            installs,
            target,
            job: Job::Idle,
            queued: sources,
            queued_removal: None,
            rows: Vec::new(),
            notice,
            drops_arrive,
            remembered,
            view: View::default(),
            list: ListState::default(),
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
        if job::finish(&mut self.job, &mut self.rows) {
            refresh_listing(&mut self.listing, &mut self.list, self.target.as_ref());
        }

        // Panels are added outermost first, and the central panel last. The
        // dialog floats above them all.
        let mut actions = drop_zone::dropped_sources(&ctx);
        actions.extend(target_panel(ui, self));
        actions.extend(results::results_panel(ui, &self.rows));
        actions.extend(main_panel(ui, self));
        actions.extend(addon_list::confirmation_dialog(
            &ctx,
            self.list.confirming.as_ref(),
            job::is_busy(&self.job),
        ));
        for action in actions {
            apply(action, self);
        }

        start_queued(self, &ctx);
        if job::is_busy(&self.job) {
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

/// The game version a `--wow-dir` inside one names. The window starts on it
/// when no `--flavor` was given, as the command line does.
///
/// For a path in no game version the window can see, the command line
/// refuses. The window shows which game version it chose before anything is
/// removed, so it opens on its usual choice instead, and says why.
fn implied_flavor<'a>(
    wow_dir: Option<&Path>,
    installs: &'a [WowInstall],
) -> crate::Result<Option<&'a str>> {
    let (Some(dir), Some(install)) = (wow_dir, installs.first()) else {
        return Ok(None);
    };
    Ok(discovery::flavor_containing(install, dir)?.map(Flavor::dir_name))
}

/// What the window says when the game version a `--wow-dir` lies in is not
/// one it can see. The error's own words point at `--flavor`, which is how
/// the command line chooses; in the window, the picker above is.
fn wow_dir_notice(error: &crate::Error) -> String {
    match error {
        crate::Error::NotInGameVersion { path, .. } => format!(
            "{} is not inside a game version found there, so the window chose one. \
             Check the game version above.",
            display_path(path)
        ),
        other => other.to_string(),
    }
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

/// Lists what the target has installed.
///
/// A game version that has never had an addon has no folder for them, which
/// means that nothing is installed rather than that something is wrong.
fn read_listing(target: Option<&Target>) -> Result<Vec<InstalledAddon>, String> {
    let Some(target) = target else {
        return Ok(Vec::new());
    };
    let addons_dir = target.addons_dir();
    if !addons_dir.is_dir() {
        return Ok(Vec::new());
    }
    installed::list(&addons_dir).map_err(|error| error.to_string())
}

/// Reads the target's addons again. The ticks of any that are gone are
/// dropped, and an open dialog is brought up to date.
fn refresh_listing(
    listing: &mut Result<Vec<InstalledAddon>, String>,
    list: &mut ListState,
    target: Option<&Target>,
) {
    *listing = read_listing(target);
    match listing {
        Ok(addons) => {
            addon_list::keep_installed(&mut list.ticked, addons);
            addon_list::refresh_confirmation(&mut list.confirming, addons);
        }
        Err(_) => addon_list::forget_choices(list),
    }
}

fn apply(action: Action, app: &mut App) {
    match action {
        Action::AddSources(paths) => app.queued.extend(paths),
        Action::SelectInstall(install) => select_install(app, install),
        Action::SelectFlavor(flavor) => select_flavor(app, flavor),
        Action::ChooseInstallDir => choose_install_dir(app),
        Action::ClearResults => app.rows.clear(),
        Action::ShowView(view) => {
            app.view = view;
            // Another program or a file manager can change the folder while
            // the window is open, and the list is where that would show.
            if view == View::Addons {
                refresh_listing(&mut app.listing, &mut app.list, app.target.as_ref());
            }
        }
        Action::SetFilter(filter) => app.list.filter = filter,
        Action::Tick { addon, ticked } => tick(&mut app.list.ticked, addon, ticked),
        Action::AskToRemove => {
            app.list.confirming =
                ask_to_remove(&app.listing, &app.list.ticked, app.target.as_ref());
        }
        Action::SetRemoveDependents(on) => {
            if let Some(confirmation) = app.list.confirming.as_mut() {
                confirmation.with_dependents = on;
            }
        }
        Action::CancelRemoval => app.list.confirming = None,
        Action::ConfirmRemoval => confirm_removal(
            &mut app.list,
            &mut app.queued_removal,
            job::is_busy(&app.job),
        ),
    }
}

fn tick(ticked: &mut BTreeSet<PathBuf>, addon: PathBuf, on: bool) {
    if on {
        ticked.insert(addon);
    } else {
        ticked.remove(&addon);
    }
}

fn ask_to_remove(
    listing: &Result<Vec<InstalledAddon>, String>,
    ticked: &BTreeSet<PathBuf>,
    target: Option<&Target>,
) -> Option<Confirmation> {
    let (Some(target), Ok(addons)) = (target, listing) else {
        return None;
    };
    addon_list::confirmation(addons, ticked, target.addons_dir())
}

/// Hands a confirmed removal to the queue, which starts it this same frame,
/// so that jobs only ever start in one place.
///
/// A removal confirmed while a job runs would be built from the list as it
/// was before that job. The dialog's button waits for the job to finish, and
/// this holds to that whatever sends the action: the dialog stays open, and
/// is brought up to date when the job ends.
fn confirm_removal(list: &mut ListState, queued_removal: &mut Option<Removal>, busy: bool) {
    if busy {
        return;
    }
    let Some(confirmation) = list.confirming.take() else {
        return;
    };
    list.ticked.clear();
    *queued_removal = Some(addon_list::removal(&confirmation));
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
    addon_list::forget_choices(&mut app.list);
    refresh_listing(&mut app.listing, &mut app.list, app.target.as_ref());
}

fn select_flavor(app: &mut App, flavor: Flavor) {
    if let Some(target) = app.target.as_mut() {
        target.flavor = flavor;
    }
    addon_list::forget_choices(&mut app.list);
    refresh_listing(&mut app.listing, &mut app.list, app.target.as_ref());
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

/// Starts waiting work when nothing is running.
///
/// A confirmed removal goes first: the user has just seen the list it
/// changes, and an install could change that list before it ran.
fn start_queued(app: &mut App, ctx: &egui::Context) {
    if job::is_busy(&app.job) {
        return;
    }
    if let Some(removal) = app.queued_removal.take() {
        app.job = job::start_removal(removal, ctx.clone());
        return;
    }
    if app.queued.is_empty() {
        return;
    }
    let sources = std::mem::take(&mut app.queued);
    let Some(target) = app.target.as_ref() else {
        let rows = sources
            .iter()
            .map(|source| results::no_target_row(source))
            .collect();
        results::prepend(&mut app.rows, rows);
        return;
    };
    app.job = job::start_install(sources, target.addons_dir(), ctx.clone());
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
                if app.target.is_some()
                    && let Ok(addons) = &app.listing
                {
                    ui.label(egui::RichText::new(describe_count(addons.len())).weak());
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
    format!("{} installed", describe_addons(installed))
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

/// The main area: the tabs, and the one that is open.
///
/// While files are dragged over the window it shows the drop zone whichever
/// tab is open, since that is where they will go.
fn main_panel(ui: &mut egui::Ui, app: &App) -> Vec<Action> {
    let hovering = drop_zone::hovering_count(ui.ctx());
    let shown = if hovering > 0 {
        View::Install
    } else {
        app.view
    };
    egui::CentralPanel::default()
        .show(ui, |ui| {
            let mut actions = tabs(ui, shown);
            ui.add_space(8.0);
            match shown {
                View::Install => actions.extend(drop_zone::show(
                    ui,
                    &app.job,
                    app.target.as_ref(),
                    hovering,
                    app.drops_arrive,
                )),
                View::Addons => actions.extend(addon_list::show(
                    ui,
                    &app.listing,
                    &app.list,
                    app.target.as_ref(),
                    &app.job,
                )),
            }
            actions
        })
        .inner
}

fn tabs(ui: &mut egui::Ui, shown: View) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        for (view, label) in [
            (View::Install, "Install"),
            (View::Addons, "Installed addons"),
        ] {
            if ui.selectable_label(shown == view, label).clicked() && shown != view {
                actions.push(Action::ShowView(view));
            }
        }
    });
    actions
}

fn describe_addons(count: usize) -> String {
    if count == 1 {
        "1 addon".to_string()
    } else {
        format!("{count} addons")
    }
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Builds an install directory holding the given game versions.
    pub(super) fn fake_install(root: &Path, flavors: &[&str]) -> WowInstall {
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

    /// Builds a target to install into, so that the drop zone has somewhere to
    /// name.
    pub(super) fn fake_target(root: &Path) -> Target {
        let install = fake_install(root, &["_retail_"]);
        let flavor = first_flavor(&install);
        Target { install, flavor }
    }

    #[test]
    fn item_counts_read_as_sentences() {
        assert_eq!(describe_items(1), "1 item");
        assert_eq!(describe_items(4), "4 items");
        assert_eq!(describe_count(1), "1 addon installed");
        assert_eq!(describe_count(0), "0 addons installed");
    }

    #[test]
    fn long_paths_are_shortened_to_the_home_folder() {
        let Some(home) = std::env::home_dir() else {
            return;
        };

        assert_eq!(display_path(&home.join("Games/wow")), "~/Games/wow");
        assert_eq!(display_path(Path::new("/opt/wow")), "/opt/wow");
    }

    /// Opens the window's state on a game folder holding these retail
    /// addons, each with the toc header given. The folder is named outright,
    /// so the usual search of the home folder never runs.
    fn app_with(root: &Path, flavors: &[&str], addons: &[(&str, &str)]) -> App {
        let install = fake_install(root, flavors);
        let addons_dir = install.addons_dir(&first_flavor(&install));
        for (name, toc) in addons {
            fs::create_dir_all(addons_dir.join(name)).unwrap();
            fs::write(addons_dir.join(name).join(format!("{name}.toc")), toc).unwrap();
        }
        App::new(Some(root), Some("retail"), Vec::new(), true, None)
    }

    fn names(addons: &[InstalledAddon]) -> Vec<&str> {
        addons.iter().map(InstalledAddon::name).collect()
    }

    /// Ticks a retail addon, as clicking its tick box does.
    fn tick_addon(app: &mut App, root: &Path, name: &str) {
        let addon = root.join("_retail_/Interface/AddOns").join(name);
        apply(
            Action::Tick {
                addon,
                ticked: true,
            },
            app,
        );
    }

    #[test]
    fn the_window_lists_what_is_installed_when_it_opens() {
        let root = tempfile::tempdir().unwrap();

        let app = app_with(
            root.path(),
            &["_retail_"],
            &[("Details", ""), ("WeakAuras", "")],
        );

        assert_eq!(
            names(app.listing.as_ref().unwrap()),
            vec!["Details", "WeakAuras"]
        );
    }

    #[test]
    fn confirming_queues_the_ticked_addons_and_those_that_need_them() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with(
            root.path(),
            &["_retail_"],
            &[
                ("Details", ""),
                ("WeakAuras", ""),
                ("WeakAurasOptions", "## Dependencies: WeakAuras\n"),
            ],
        );

        tick_addon(&mut app, root.path(), "WeakAuras");
        apply(Action::AskToRemove, &mut app);
        let asked = app.list.confirming.clone().unwrap();
        apply(Action::ConfirmRemoval, &mut app);

        assert_eq!(names(&asked.dependents), vec!["WeakAurasOptions"]);
        assert!(app.list.confirming.is_none());
        assert!(app.list.ticked.is_empty());
        let removal = app.queued_removal.as_ref().unwrap();
        assert_eq!(
            names(&removal.addons),
            vec!["WeakAuras", "WeakAurasOptions"]
        );
        assert!(
            root.path()
                .join("_retail_/Interface/AddOns/WeakAuras")
                .exists(),
            "nothing is removed until the queue starts the job"
        );
    }

    #[test]
    fn cancelling_removes_nothing_and_keeps_the_ticks() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with(root.path(), &["_retail_"], &[("WeakAuras", "")]);

        tick_addon(&mut app, root.path(), "WeakAuras");
        apply(Action::AskToRemove, &mut app);
        apply(Action::CancelRemoval, &mut app);

        assert!(app.list.confirming.is_none());
        assert!(app.queued_removal.is_none());
        let ticked = root.path().join("_retail_/Interface/AddOns/WeakAuras");
        assert!(app.list.ticked.contains(&ticked));
    }

    #[test]
    fn ticks_belong_to_the_game_version_they_were_made_in() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with(
            root.path(),
            &["_retail_", "_classic_era_"],
            &[("WeakAuras", "")],
        );
        let era = Flavor::from_dir_name("_classic_era_").unwrap();

        tick_addon(&mut app, root.path(), "WeakAuras");
        apply(Action::SelectFlavor(era), &mut app);

        assert!(app.list.ticked.is_empty());
        assert!(app.listing.as_ref().unwrap().is_empty());
    }

    #[test]
    fn opening_the_list_shows_changes_made_behind_the_window() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with(root.path(), &["_retail_"], &[("WeakAuras", "")]);
        tick_addon(&mut app, root.path(), "WeakAuras");
        fs::remove_dir_all(root.path().join("_retail_/Interface/AddOns/WeakAuras")).unwrap();

        apply(Action::ShowView(View::Addons), &mut app);

        assert!(app.listing.as_ref().unwrap().is_empty());
        assert!(app.list.ticked.is_empty());
    }

    #[test]
    fn a_wow_dir_in_no_game_version_it_can_see_is_named_in_a_notice() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_"]);
        let unseen = root.path().join("_classic_era_");
        fs::create_dir_all(&unseen).unwrap();

        let app = App::new(Some(&unseen), None, Vec::new(), true, None);

        assert!(app.target.unwrap().flavor.is_retail());
        assert!(app.notice.unwrap().contains("Check the game version above"));
    }

    #[test]
    fn a_wow_dir_inside_a_game_version_opens_the_window_on_it() {
        let root = tempfile::tempdir().unwrap();
        fake_install(root.path(), &["_retail_", "_classic_era_"]);
        let era = root.path().join("_classic_era_/Interface/AddOns");

        let app = App::new(Some(&era), None, Vec::new(), true, None);

        assert_eq!(app.target.unwrap().flavor.dir_name(), "_classic_era_");
    }

    #[test]
    fn a_removal_waits_for_a_running_job_then_goes_before_queued_drops() {
        let root = tempfile::tempdir().unwrap();
        let mut app = app_with(root.path(), &["_retail_"], &[("WeakAuras", "")]);
        let ctx = egui::Context::default();
        tick_addon(&mut app, root.path(), "WeakAuras");
        apply(Action::AskToRemove, &mut app);
        // Files dropped while the dialog is open start an install, and more
        // dropped after that wait for it.
        let (install_ends, results) = std::sync::mpsc::channel();
        app.job = Job::Working {
            task: job::Task::Install { items: 1 },
            results,
        };
        app.queued.push(root.path().join("Dropped.zip"));

        apply(Action::ConfirmRemoval, &mut app);
        start_queued(&mut app, &ctx);

        assert!(app.list.confirming.is_some(), "the dialog stays open");
        assert!(app.queued_removal.is_none());
        assert_eq!(app.queued.len(), 1);

        install_ends.send(Vec::new()).unwrap();
        assert!(job::finish(&mut app.job, &mut app.rows));
        apply(Action::ConfirmRemoval, &mut app);
        start_queued(&mut app, &ctx);

        assert!(app.list.confirming.is_none());
        assert_eq!(app.queued.len(), 1, "the drop waits for the removal");
        let Job::Working { task, results } = std::mem::replace(&mut app.job, Job::Idle) else {
            panic!("the removal did not start");
        };
        assert_eq!(task, job::Task::Remove { addons: 1 });
        results.recv().unwrap();
        let removed = root.path().join("_retail_/Interface/AddOns/WeakAuras");
        assert!(!removed.exists());
    }
}
