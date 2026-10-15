//! The result list at the bottom of the window, and the rows it shows.
//!
//! Installs and removals both report as rows, so the list does not need to
//! know which kind of job a row came from.

use std::path::Path;

use eframe::egui;

use super::Action;
use crate::install;
use crate::installed::InstalledAddon;
use crate::remove;

/// What happened to the addon on a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The work was done. `warning` is true when something still needs the
    /// user's attention, such as files that could not be deleted.
    Done {
        what: Done,
        warning: bool,
    },
    Failed,
}

/// The kinds of work a row can report as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Installed,
    Updated,
    Removed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Done { what, .. } => match what {
                Done::Installed => "installed",
                Done::Updated => "updated",
                Done::Removed => "removed",
            },
            Self::Failed => "failed",
        }
    }
}

/// One line of the result list.
#[derive(Debug, Clone)]
pub struct Row {
    pub status: Status,
    pub addon: String,
    pub detail: String,
}

pub fn failed_row(addon: &str, detail: &str) -> Row {
    Row {
        status: Status::Failed,
        addon: addon.to_string(),
        detail: detail.to_string(),
    }
}

pub fn prepend(rows: &mut Vec<Row>, newest: Vec<Row>) {
    rows.splice(0..0, newest);
}

/// The rows for one source: a row per addon, or one row saying why the
/// source could not be read at all.
pub fn install_rows(source: &Path, result: crate::Result<Vec<install::Outcome>>) -> Vec<Row> {
    match result {
        Ok(outcomes) => outcomes.iter().map(install_row).collect(),
        Err(error) => vec![failed_row(
            &crate::fs_util::file_name_lossy(source),
            &error.to_string(),
        )],
    }
}

fn install_row(outcome: &install::Outcome) -> Row {
    match &outcome.result {
        Ok(installed) => Row {
            status: Status::Done {
                what: if installed.replaced {
                    Done::Updated
                } else {
                    Done::Installed
                },
                warning: installed.left_behind.is_some(),
            },
            addon: outcome.addon.clone(),
            detail: install::describe_installed(&outcome.toc, installed),
        },
        Err(error) => failed_row(&outcome.addon, &error.to_string()),
    }
}

/// The rows for one removal, a row per addon. A removal that could not start
/// at all removed none of them, so each gets a row saying why.
pub fn removal_rows(
    addons: &[InstalledAddon],
    result: crate::Result<Vec<remove::Outcome>>,
) -> Vec<Row> {
    match result {
        Ok(outcomes) => outcomes.iter().map(removal_row).collect(),
        Err(error) => addons
            .iter()
            .map(|addon| failed_row(addon.name(), &error.to_string()))
            .collect(),
    }
}

fn removal_row(outcome: &remove::Outcome) -> Row {
    match &outcome.result {
        Ok(removed) => Row {
            status: Status::Done {
                what: Done::Removed,
                warning: removed.left_behind.is_some(),
            },
            addon: outcome.addon.name().to_string(),
            detail: remove::describe_removed(&outcome.addon, removed),
        },
        Err(error) => failed_row(outcome.addon.name(), &error.to_string()),
    }
}

pub fn no_target_row(source: &Path) -> Row {
    failed_row(
        &crate::fs_util::file_name_lossy(source),
        "choose a World of Warcraft folder first",
    )
}

/// Bottom panel: what the last jobs did. Hidden until there is something to
/// say.
pub fn results_panel(ui: &mut egui::Ui, rows: &[Row]) -> Vec<Action> {
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
                let colour = status_colour(ui.visuals(), row.status);
                ui.colored_label(colour, row.status.label());
                ui.label(egui::RichText::new(row.addon.as_str()).strong());
                ui.label(row.detail.as_str());
                ui.end_row();
            }
        });
}

fn status_colour(visuals: &egui::Visuals, status: Status) -> egui::Color32 {
    match status {
        Status::Failed => visuals.error_fg_color,
        Status::Done { warning: true, .. } => visuals.warn_fg_color,
        Status::Done { .. } if visuals.dark_mode => egui::Color32::from_rgb(0x7a, 0xd1, 0x8c),
        Status::Done { .. } => egui::Color32::from_rgb(0x1c, 0x7a, 0x3e),
    }
}

/// Counts the rows by what happened, leaving out what did not happen at all.
fn summarise(rows: &[Row]) -> String {
    ["installed", "removed", "failed"]
        .iter()
        .map(|word| {
            let count = rows
                .iter()
                .filter(|row| summary_word(row.status) == *word)
                .count();
            (count, word)
        })
        .filter(|(count, _)| *count > 0)
        .map(|(count, word)| format!("{count} {word}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The word a row counts under in the summary. An update counts as an
/// install.
fn summary_word(status: Status) -> &'static str {
    match status {
        Status::Done {
            what: Done::Installed | Done::Updated,
            ..
        } => "installed",
        Status::Done {
            what: Done::Removed,
            ..
        } => "removed",
        Status::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: Status) -> Row {
        Row {
            status,
            addon: "MyAddon".to_string(),
            detail: String::new(),
        }
    }

    fn done(what: Done) -> Row {
        row(Status::Done {
            what,
            warning: false,
        })
    }

    #[test]
    fn summarises_what_a_run_did() {
        let removed_with_leftovers = row(Status::Done {
            what: Done::Removed,
            warning: true,
        });

        assert_eq!(summarise(&[done(Done::Installed)]), "1 installed");
        assert_eq!(
            summarise(&[done(Done::Updated), row(Status::Failed)]),
            "1 installed, 1 failed"
        );
        assert_eq!(
            summarise(&[
                done(Done::Removed),
                done(Done::Installed),
                removed_with_leftovers
            ]),
            "1 installed, 2 removed"
        );
        assert_eq!(summarise(&[row(Status::Failed)]), "1 failed");
    }

    #[test]
    fn newest_results_are_listed_first() {
        let mut rows = vec![done(Done::Installed)];

        prepend(&mut rows, vec![row(Status::Failed)]);

        assert_eq!(rows[0].status, Status::Failed);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_source_dropped_with_no_target_says_what_is_missing() {
        let row = no_target_row(Path::new("/tmp/MyAddon.zip"));

        assert_eq!(row.addon, "MyAddon.zip");
        assert_eq!(row.status, Status::Failed);
    }

    #[test]
    fn a_removal_that_could_not_start_fails_every_addon_it_held() {
        let root = tempfile::tempdir().unwrap();
        for name in ["One", "Two"] {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
        }
        let addons = crate::installed::list(root.path()).unwrap();
        let error = crate::Error::NoInstallFound;

        let rows = removal_rows(&addons, Err(error));

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.status == Status::Failed));
    }
}
