//! The Installed addons tab: what the chosen game version has, with tick
//! boxes to pick addons to remove.
//!
//! Nothing is removed without a dialog that names every addon it will
//! remove. The dialog also names the installed addons that need one of them,
//! since the game stops loading those, and offers to remove them as well.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use eframe::egui;

use super::job::{self, Job, Removal};
use super::{Action, Target, describe_addons, display_path};
use crate::installed::{self, InstalledAddon};

/// What the tab keeps between frames.
#[derive(Debug, Default)]
pub struct ListState {
    /// Text the list is narrowed to.
    pub filter: String,
    /// The folders of the ticked addons. Paths rather than names, since two
    /// folder names that are not valid text can read the same once shown.
    ///
    /// A tick stays when the filter hides its addon. The dialog names every
    /// ticked addon, so nothing the filter hides is removed unseen.
    pub ticked: BTreeSet<PathBuf>,
    /// A removal waiting for the user to confirm it.
    pub confirming: Option<Confirmation>,
}

/// A removal the dialog is asking about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub addons_dir: PathBuf,
    /// The ticked addons.
    pub chosen: Vec<InstalledAddon>,
    /// Installed addons that need one of `chosen` and will not load without
    /// it.
    pub dependents: Vec<InstalledAddon>,
    /// Whether to remove `dependents` too. It starts on, since an addon the
    /// game will not load is of no use to anyone.
    pub with_dependents: bool,
}

/// The removal to ask about, made from what is ticked. `None` when nothing
/// is ticked.
pub fn confirmation(
    installed: &[InstalledAddon],
    ticked: &BTreeSet<PathBuf>,
    addons_dir: PathBuf,
) -> Option<Confirmation> {
    let chosen = ticked_addons(installed, ticked);
    if chosen.is_empty() {
        return None;
    }
    Some(Confirmation {
        dependents: installed::dependents(installed, &chosen),
        chosen,
        addons_dir,
        with_dependents: true,
    })
}

/// Every addon that confirming will remove.
pub fn to_remove(confirmation: &Confirmation) -> Vec<InstalledAddon> {
    let mut addons = confirmation.chosen.clone();
    if confirmation.with_dependents {
        addons.extend(confirmation.dependents.iter().cloned());
    }
    addons
}

pub fn removal(confirmation: &Confirmation) -> Removal {
    Removal {
        addons_dir: confirmation.addons_dir.clone(),
        addons: to_remove(confirmation),
    }
}

/// Keeps only the ticks of addons still installed, so that a tick cannot
/// outlive its addon and be counted in the next removal.
pub fn keep_installed(ticked: &mut BTreeSet<PathBuf>, installed: &[InstalledAddon]) {
    ticked.retain(|path| installed.iter().any(|addon| addon.path() == path));
}

/// Brings an open dialog up to date with a fresh listing.
///
/// An install can finish while the dialog is open, since files dropped
/// meanwhile install at once. It may have added an addon that needs a
/// chosen one, or replaced a chosen one with a new version, and the dialog
/// has to name what will really be removed. It closes when none of the
/// chosen addons is left.
pub fn refresh_confirmation(confirming: &mut Option<Confirmation>, installed: &[InstalledAddon]) {
    let Some(stale) = confirming.take() else {
        return;
    };
    let chosen = stale
        .chosen
        .iter()
        .map(|addon| addon.path().to_path_buf())
        .collect();
    *confirming = confirmation(installed, &chosen, stale.addons_dir).map(|fresh| Confirmation {
        with_dependents: stale.with_dependents,
        ..fresh
    });
}

/// Ticks and a waiting dialog belong to the list of one game version, so a
/// change of target drops them.
pub fn forget_choices(state: &mut ListState) {
    state.ticked.clear();
    state.confirming = None;
}

fn ticked_addons(installed: &[InstalledAddon], ticked: &BTreeSet<PathBuf>) -> Vec<InstalledAddon> {
    installed
        .iter()
        .filter(|addon| ticked.contains(addon.path()))
        .cloned()
        .collect()
}

/// True when the filter matches the addon's folder name or its title,
/// ignoring case. An empty filter matches every addon.
fn matches_filter(addon: &InstalledAddon, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    let title = addon.toc().and_then(|toc| toc.title.as_deref());
    filter.is_empty()
        || addon.name().to_lowercase().contains(&filter)
        || title.is_some_and(|title| title.to_lowercase().contains(&filter))
}

/// Draws the tab into the main area.
pub fn show(
    ui: &mut egui::Ui,
    listing: &Result<Vec<InstalledAddon>, String>,
    state: &ListState,
    target: Option<&Target>,
    job: &Job,
) -> Vec<Action> {
    if let Job::Working { task, .. } = job {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(job::describe(*task));
        });
        ui.add_space(4.0);
    }
    let Some(target) = target else {
        ui.label(egui::RichText::new("Use Choose... above to point at your game folder.").weak());
        return Vec::new();
    };
    let addons = match listing {
        Ok(addons) => addons,
        Err(message) => {
            let colour = ui.visuals().warn_fg_color;
            ui.colored_label(colour, message.as_str());
            return Vec::new();
        }
    };
    if addons.is_empty() {
        let text = format!(
            "No addons are installed in {}.",
            display_path(&target.addons_dir())
        );
        ui.label(egui::RichText::new(text).weak());
        return Vec::new();
    }
    let mut actions = toolbar(ui, addons, state, job::is_busy(job));
    ui.add_space(4.0);
    actions.extend(addon_rows(ui, addons, state));
    actions
}

/// The filter box, and the button that asks to remove what is ticked.
fn toolbar(
    ui: &mut egui::Ui,
    addons: &[InstalledAddon],
    state: &ListState,
    busy: bool,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let ticked = ticked_addons(addons, &state.ticked).len();
    ui.horizontal(|ui| {
        ui.label("Filter");
        let mut filter = state.filter.clone();
        // A fixed id keeps the box focused while the busy line above it comes
        // and goes, which would otherwise move it and give it a new id.
        let edit = egui::TextEdit::singleline(&mut filter)
            .id_salt("addon-filter")
            .hint_text("name or title")
            .desired_width(220.0);
        if ui.add(edit).changed() {
            actions.push(Action::SetFilter(filter));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui
                .add_enabled(
                    ticked > 0 && !busy,
                    egui::Button::new(remove_button_text(ticked)),
                )
                .on_disabled_hover_text(if busy {
                    "Wait for the running job to finish."
                } else {
                    "Tick the addons to remove first."
                });
            if button.clicked() {
                actions.push(Action::AskToRemove);
            }
        });
    });
    actions
}

fn remove_button_text(ticked: usize) -> String {
    if ticked == 0 {
        "Remove...".to_string()
    } else {
        format!("Remove {}...", describe_addons(ticked))
    }
}

fn addon_rows(ui: &mut egui::Ui, addons: &[InstalledAddon], state: &ListState) -> Vec<Action> {
    let mut actions = Vec::new();
    let shown: Vec<&InstalledAddon> = addons
        .iter()
        .filter(|addon| matches_filter(addon, &state.filter))
        .collect();
    if shown.is_empty() {
        ui.label(egui::RichText::new("No installed addon matches the filter.").weak());
        return actions;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("installed")
                .num_columns(3)
                .striped(true)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for addon in shown {
                        actions.extend(addon_row(ui, addon, &state.ticked));
                        ui.end_row();
                    }
                });
        });
    actions
}

fn addon_row(ui: &mut egui::Ui, addon: &InstalledAddon, ticked: &BTreeSet<PathBuf>) -> Vec<Action> {
    let mut actions = Vec::new();
    let mut is_ticked = ticked.contains(addon.path());
    let name = egui::RichText::new(addon.name()).strong();
    if ui.checkbox(&mut is_ticked, name).changed() {
        actions.push(Action::Tick {
            addon: addon.path().to_path_buf(),
            ticked: is_ticked,
        });
    }
    ui.label(addon.version().unwrap_or_default());
    let note = egui::RichText::new(installed::describe(addon));
    let note = if addon.toc().is_none() {
        note.color(ui.visuals().warn_fg_color)
    } else {
        note.weak()
    };
    ui.add(egui::Label::new(note).truncate());
    actions
}

/// The dialog that asks before anything is removed. Escape, or a click
/// outside the dialog, cancels.
pub fn confirmation_dialog(
    ctx: &egui::Context,
    confirming: Option<&Confirmation>,
    busy: bool,
) -> Vec<Action> {
    let Some(confirmation) = confirming else {
        return Vec::new();
    };
    let response = egui::Modal::new(egui::Id::new("confirm-removal"))
        .show(ctx, |ui| dialog_contents(ui, confirmation, busy));
    let cancelled = response.should_close();
    let mut actions = response.inner;
    if cancelled {
        actions.push(Action::CancelRemoval);
    }
    actions
}

/// While a job runs, the Remove button waits. Files dropped while the dialog
/// is open install at once, and only once that install has finished does the
/// dialog show what removing will really take with it.
fn dialog_contents(ui: &mut egui::Ui, confirmation: &Confirmation, busy: bool) -> Vec<Action> {
    let mut actions = Vec::new();
    let removing = to_remove(confirmation);
    ui.set_max_width(440.0);
    ui.heading(dialog_heading(&confirmation.chosen));
    ui.add_space(8.0);
    if confirmation.chosen.len() > 1 {
        name_list(ui, "chosen", &confirmation.chosen);
        ui.add_space(8.0);
    }
    if !confirmation.dependents.is_empty() {
        ui.label(dependents_text(confirmation));
        name_list(ui, "dependents", &confirmation.dependents);
        let mut with_dependents = confirmation.with_dependents;
        if ui
            .checkbox(&mut with_dependents, "Remove these as well")
            .changed()
        {
            actions.push(Action::SetRemoveDependents(with_dependents));
        }
        ui.add_space(8.0);
    }
    ui.label(egui::RichText::new(consequence_text(&confirmation.addons_dir)).weak());
    if let Some(text) = link_text(&removing) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(text).weak());
    }
    ui.add_space(12.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let remove = format!("Remove {}", describe_addons(removing.len()));
        if ui
            .add_enabled(!busy, egui::Button::new(remove))
            .on_disabled_hover_text("Wait for the running job to finish.")
            .clicked()
        {
            actions.push(Action::ConfirmRemoval);
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::CancelRemoval);
        }
        if busy {
            ui.spinner();
        }
    });
    actions
}

/// The names in a dialog, with their versions. Long lists scroll, so that a
/// dialog about many addons still fits in the window.
fn name_list(ui: &mut egui::Ui, id: &str, addons: &[InstalledAddon]) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(140.0)
        .show(ui, |ui| {
            egui::Grid::new(id)
                .num_columns(2)
                .spacing([16.0, 2.0])
                .show(ui, |ui| {
                    for addon in addons {
                        ui.label(egui::RichText::new(addon.name()).strong());
                        ui.label(egui::RichText::new(addon.version().unwrap_or_default()).weak());
                        ui.end_row();
                    }
                });
        });
}

fn dialog_heading(chosen: &[InstalledAddon]) -> String {
    match chosen {
        [only] => format!("Remove {}?", only.name()),
        _ => format!("Remove {}?", describe_addons(chosen.len())),
    }
}

fn dependents_text(confirmation: &Confirmation) -> String {
    let them = if confirmation.dependents.len() == 1 {
        "this addon"
    } else {
        "these addons"
    };
    let needed = match confirmation.chosen.as_slice() {
        [only] => only.name(),
        _ => "the ones above",
    };
    format!("The game will not load {them} without {needed}:")
}

fn consequence_text(addons_dir: &Path) -> String {
    format!(
        "The folders are deleted from {}. The settings the game saved for \
         them stay in WTF, so installing an addon again brings its settings \
         back.",
        display_path(addons_dir)
    )
}

/// What the dialog says about links, which a developer is likely to have
/// made to a working copy they would not want deleted.
fn link_text(removing: &[InstalledAddon]) -> Option<String> {
    let links: Vec<&str> = removing
        .iter()
        .filter(|addon| addon.is_link())
        .map(InstalledAddon::name)
        .collect();
    match links.as_slice() {
        [] => None,
        [only] => Some(format!(
            "{only} is a link, so only the link is removed. The folder it \
             leads to stays."
        )),
        several => Some(format!(
            "{} are links, so only the links are removed. The folders they \
             lead to stay.",
            several.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    fn put_addon(addons: &Path, name: &str, toc: &str) {
        fs::create_dir_all(addons.join(name)).unwrap();
        fs::write(addons.join(name).join(format!("{name}.toc")), toc).unwrap();
    }

    fn names(addons: &[InstalledAddon]) -> Vec<&str> {
        addons.iter().map(InstalledAddon::name).collect()
    }

    fn ticks(root: &Path, names: &[&str]) -> BTreeSet<PathBuf> {
        names.iter().map(|name| root.join(name)).collect()
    }

    /// A WeakAuras install as its download leaves it, beside an addon that
    /// has nothing to do with it.
    fn weakauras(root: &Path) -> Vec<InstalledAddon> {
        put_addon(
            root,
            "WeakAuras",
            "## Title: WeakAuras\n## Version: 5.19.1\n",
        );
        put_addon(
            root,
            "WeakAurasOptions",
            "## Title: WeakAuras Options\n## Dependencies: WeakAuras\n",
        );
        put_addon(root, "Details", "## Title: Details! Damage Meter\n");
        installed::list(root).unwrap()
    }

    #[test]
    fn nothing_ticked_means_nothing_to_ask_about() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());

        assert_eq!(
            confirmation(&installed, &BTreeSet::new(), root.path().to_path_buf()),
            None
        );
    }

    #[test]
    fn the_dialog_offers_the_addons_that_need_a_ticked_one() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());

        let asked = confirmation(
            &installed,
            &ticks(root.path(), &["WeakAuras"]),
            root.path().to_path_buf(),
        )
        .unwrap();

        assert_eq!(names(&asked.chosen), vec!["WeakAuras"]);
        assert_eq!(names(&asked.dependents), vec!["WeakAurasOptions"]);
        assert!(asked.with_dependents);
        assert_eq!(
            names(&removal(&asked).addons),
            vec!["WeakAuras", "WeakAurasOptions"]
        );
    }

    #[test]
    fn the_addons_that_need_a_ticked_one_can_be_kept() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let mut asked = confirmation(
            &installed,
            &ticks(root.path(), &["WeakAuras"]),
            root.path().to_path_buf(),
        )
        .unwrap();

        asked.with_dependents = false;

        assert_eq!(names(&to_remove(&asked)), vec!["WeakAuras"]);
    }

    #[test]
    fn ticks_of_addons_that_are_gone_are_dropped() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let mut ticked = ticks(root.path(), &["WeakAuras", "Removed"]);

        keep_installed(&mut ticked, &installed);

        assert_eq!(ticked, ticks(root.path(), &["WeakAuras"]));
    }

    #[test]
    fn the_filter_matches_names_and_titles_ignoring_case() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let shown = |filter: &str| -> Vec<&str> {
            installed
                .iter()
                .filter(|addon| matches_filter(addon, filter))
                .map(InstalledAddon::name)
                .collect()
        };

        assert_eq!(shown("weak"), vec!["WeakAuras", "WeakAurasOptions"]);
        assert_eq!(shown("DAMAGE"), vec!["Details"]);
        assert_eq!(
            shown("  "),
            vec!["Details", "WeakAuras", "WeakAurasOptions"]
        );
        assert!(shown("nothing like it").is_empty());
    }

    #[test]
    fn the_dialog_says_that_only_a_link_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        put_addon(&checkout, "DevAddon", "");
        put_addon(&checkout, "Other", "");
        let addons = root.path().join("AddOns");
        put_addon(&addons, "Plain", "");
        symlink(checkout.join("DevAddon"), addons.join("DevAddon")).unwrap();
        let installed = installed::list(&addons).unwrap();

        assert_eq!(
            link_text(&installed).unwrap(),
            "DevAddon is a link, so only the link is removed. The folder it leads to stays."
        );
        symlink(checkout.join("Other"), addons.join("Other")).unwrap();
        let installed = installed::list(&addons).unwrap();
        assert!(
            link_text(&installed)
                .unwrap()
                .starts_with("DevAddon, Other are links")
        );
        let plain: Vec<InstalledAddon> = installed
            .into_iter()
            .filter(|addon| addon.name() == "Plain")
            .collect();
        assert_eq!(link_text(&plain), None);
    }

    #[test]
    fn an_open_dialog_learns_of_an_addon_installed_under_it() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let mut confirming = confirmation(
            &installed,
            &ticks(root.path(), &["WeakAuras"]),
            root.path().to_path_buf(),
        );
        confirming.as_mut().unwrap().with_dependents = false;
        put_addon(
            root.path(),
            "WeakAurasArchive",
            "## RequiredDeps: WeakAuras\n",
        );

        refresh_confirmation(&mut confirming, &installed::list(root.path()).unwrap());

        let fresh = confirming.unwrap();
        assert_eq!(
            names(&fresh.dependents),
            vec!["WeakAurasArchive", "WeakAurasOptions"]
        );
        assert!(!fresh.with_dependents, "the user's choice is kept");
    }

    #[test]
    fn an_open_dialog_closes_when_its_addons_are_gone() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let mut confirming = confirmation(
            &installed,
            &ticks(root.path(), &["Details"]),
            root.path().to_path_buf(),
        );
        fs::remove_dir_all(root.path().join("Details")).unwrap();

        refresh_confirmation(&mut confirming, &installed::list(root.path()).unwrap());

        assert_eq!(confirming, None);
    }

    #[test]
    fn the_dialog_reads_naturally_for_one_addon_or_several() {
        let root = tempfile::tempdir().unwrap();
        let installed = weakauras(root.path());
        let one = confirmation(
            &installed,
            &ticks(root.path(), &["WeakAuras"]),
            root.path().to_path_buf(),
        )
        .unwrap();
        let two = confirmation(
            &installed,
            &ticks(root.path(), &["WeakAuras", "Details"]),
            root.path().to_path_buf(),
        )
        .unwrap();

        assert_eq!(dialog_heading(&one.chosen), "Remove WeakAuras?");
        assert_eq!(dialog_heading(&two.chosen), "Remove 2 addons?");
        assert_eq!(
            dependents_text(&one),
            "The game will not load this addon without WeakAuras:"
        );
        assert_eq!(
            dependents_text(&two),
            "The game will not load this addon without the ones above:"
        );
        assert_eq!(remove_button_text(0), "Remove...");
        assert_eq!(remove_button_text(2), "Remove 2 addons...");
    }
}
