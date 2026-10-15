//! The Install tab: the area files are dropped onto to install them, with
//! buttons that do the same job for anyone who would rather not drag
//! anything.

use std::path::PathBuf;

use eframe::egui;

use super::job::{self, Job, Task};
use super::{Action, Target, describe_items, display_path, pick_folder};

/// The paths dropped onto the window this frame.
pub fn dropped_sources(ctx: &egui::Context) -> Vec<Action> {
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
pub fn hovering_count(ctx: &egui::Context) -> usize {
    ctx.input(|input| input.raw.hovered_files.len())
}

/// Draws the tab: the area files are dropped onto, filling the rest of the
/// main area.
pub fn show(
    ui: &mut egui::Ui,
    job: &Job,
    target: Option<&Target>,
    hovering: usize,
    drops_arrive: bool,
) -> Vec<Action> {
    let (heading, hint) = zone_text(job, target, hovering, drops_arrive);
    let mut actions = Vec::new();
    drop_zone(ui, hovering > 0, |ui| {
        ui.add_space(ui.available_height() * 0.25);
        ui.label(egui::RichText::new(heading).size(20.0).strong());
        ui.add_space(8.0);
        ui.label(egui::RichText::new(hint).weak());
        ui.add_space(20.0);
        if job::is_busy(job) {
            ui.spinner();
        } else {
            actions.extend(browse_buttons(ui));
        }
    });
    actions
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
///
/// `drops_arrive` is false on a backend that never reports a drop, and there
/// the zone stops asking to be dropped on and names the buttons instead.
fn zone_text(
    job: &Job,
    target: Option<&Target>,
    hovering: usize,
    drops_arrive: bool,
) -> (String, String) {
    if let Job::Working { task, .. } = job {
        return (job::describe(*task), working_hint(*task).to_string());
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
    let destination = display_path(&target.addons_dir());
    if !drops_arrive {
        return (
            "Choose addon folders or zip files".to_string(),
            format!(
                "This window cannot take drops on Wayland. The buttons below \
                 install into {destination}"
            ),
        );
    }
    (
        "Drop addon folders or zip files here".to_string(),
        format!("They go into {destination}"),
    )
}

/// What happens to a drop while a job runs: it waits for the job to end.
fn working_hint(task: Task) -> &'static str {
    match task {
        Task::Install { .. } => "The window stays usable; anything else dropped will follow.",
        Task::Remove { .. } => "The window stays usable; anything dropped is installed next.",
    }
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
    use std::sync::mpsc;

    use super::super::tests::fake_target;
    use super::*;

    #[test]
    fn the_drop_zone_says_there_is_nowhere_to_install_to() {
        let (heading, hint) = zone_text(&Job::Idle, None, 0, true);

        assert_eq!(heading, "No World of Warcraft folder found");
        assert!(hint.contains("Choose"));
    }

    #[test]
    fn the_drop_zone_counts_what_is_being_dragged() {
        let (heading, _) = zone_text(&Job::Idle, None, 3, true);

        // With nowhere to install to, the count is not the point.
        assert_eq!(heading, "No World of Warcraft folder found");
    }

    #[test]
    fn the_drop_zone_says_what_a_running_removal_is_doing() {
        let root = tempfile::tempdir().unwrap();
        let target = fake_target(root.path());
        let (_sender, results) = mpsc::channel();
        let job = Job::Working {
            task: Task::Remove { addons: 2 },
            results,
        };

        let (heading, hint) = zone_text(&job, Some(&target), 0, true);

        assert_eq!(heading, "Removing 2 addons...");
        assert!(hint.contains("installed next"));
    }

    #[test]
    fn the_drop_zone_invites_a_drop_where_drops_arrive() {
        let root = tempfile::tempdir().unwrap();
        let target = fake_target(root.path());

        let (heading, hint) = zone_text(&Job::Idle, Some(&target), 0, true);

        assert_eq!(heading, "Drop addon folders or zip files here");
        assert!(hint.contains("AddOns"));
    }

    #[test]
    fn the_drop_zone_sends_the_user_to_the_buttons_where_drops_never_arrive() {
        let root = tempfile::tempdir().unwrap();
        let target = fake_target(root.path());

        let (heading, hint) = zone_text(&Job::Idle, Some(&target), 0, false);

        assert_eq!(heading, "Choose addon folders or zip files");
        assert!(hint.contains("The buttons below install into"));
        assert!(hint.contains("AddOns"));
    }
}
