//! Running installs and removals on another thread, so that a large archive
//! or a large addon folder cannot freeze the window.
//!
//! One job runs at a time. Both kinds empty the staging folder beside
//! `AddOns` before they start, so two at once would delete each other's work.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

use super::results::{self, Row};
use super::{describe_addons, describe_items};
use crate::installed::InstalledAddon;
use crate::remove;

/// Whether a job is running. A job owns the channel its thread reports on.
pub enum Job {
    Idle,
    Working {
        task: Task,
        results: Receiver<Vec<Row>>,
    },
}

/// What a running job is doing, for the window to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Install { items: usize },
    Remove { addons: usize },
}

/// Addons to remove, and the `AddOns` folder they were listed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub addons_dir: PathBuf,
    pub addons: Vec<InstalledAddon>,
}

pub fn start_install(sources: Vec<PathBuf>, addons_dir: PathBuf, ctx: egui::Context) -> Job {
    let task = Task::Install {
        items: sources.len(),
    };
    spawn(task, ctx, move || {
        sources
            .iter()
            .flat_map(|source| {
                results::install_rows(source, crate::install_source(source, &addons_dir))
            })
            .collect()
    })
}

pub fn start_removal(removal: Removal, ctx: egui::Context) -> Job {
    let task = Task::Remove {
        addons: removal.addons.len(),
    };
    spawn(task, ctx, move || {
        let result = remove::execute(&removal.addons_dir, &removal.addons);
        results::removal_rows(&removal.addons, result)
    })
}

/// Runs `work` on a thread of its own. Its rows come back over the job's
/// channel, and the window is woken to show them.
fn spawn(task: Task, ctx: egui::Context, work: impl FnOnce() -> Vec<Row> + Send + 'static) -> Job {
    let (sender, results) = mpsc::channel();
    thread::spawn(move || {
        let rows = work();
        // A closed channel means the window has gone, which is not a problem.
        let _ = sender.send(rows);
        ctx.request_repaint();
    });
    Job::Working { task, results }
}

/// Moves a finished job's rows into the list.
///
/// Reports whether a job ended, so the caller can refresh what it says about
/// the game folder.
pub fn finish(job: &mut Job, rows: &mut Vec<Row>) -> bool {
    let Job::Working { task, results } = job else {
        return false;
    };
    match results.try_recv() {
        Ok(finished) => {
            results::prepend(rows, finished);
            *job = Job::Idle;
            true
        }
        Err(TryRecvError::Empty) => false,
        // The thread ended without reporting, which takes a panic. Nothing
        // more will arrive, so say so rather than waiting for ever.
        Err(TryRecvError::Disconnected) => {
            results::prepend(rows, vec![unexpected_stop_row(*task)]);
            *job = Job::Idle;
            true
        }
    }
}

pub fn is_busy(job: &Job) -> bool {
    matches!(job, Job::Working { .. })
}

/// What a running job is doing, in words.
pub fn describe(task: Task) -> String {
    match task {
        Task::Install { items } => format!("Installing {}...", describe_items(items)),
        Task::Remove { addons } => format!("Removing {}...", describe_addons(addons)),
    }
}

fn unexpected_stop_row(task: Task) -> Row {
    match task {
        Task::Install { .. } => results::failed_row("Install", "the install stopped unexpectedly"),
        Task::Remove { .. } => results::failed_row("Removal", "the removal stopped unexpectedly"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::gui::results::{Done, Status};
    use crate::installed;

    /// Waits for a job's thread to report, as the window would over several
    /// frames.
    fn wait_for(job: Job) -> Vec<Row> {
        let Job::Working { results, .. } = job else {
            panic!("the job never started");
        };
        results.recv().unwrap()
    }

    #[test]
    fn a_removal_runs_on_its_own_thread_and_reports_each_addon() {
        let root = tempfile::tempdir().unwrap();
        let addons_dir = root.path().join("Interface").join("AddOns");
        for name in ["Details", "WeakAuras"] {
            fs::create_dir_all(addons_dir.join(name)).unwrap();
            fs::write(addons_dir.join(name).join(format!("{name}.toc")), "").unwrap();
        }
        let addons = installed::list(&addons_dir).unwrap();

        let job = start_removal(
            Removal {
                addons_dir: addons_dir.clone(),
                addons,
            },
            egui::Context::default(),
        );
        let rows = wait_for(job);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| {
            row.status
                == Status::Done {
                    what: Done::Removed,
                    warning: false,
                }
        }));
        assert!(installed::list(&addons_dir).unwrap().is_empty());
    }

    #[test]
    fn a_job_says_what_it_is_doing() {
        assert_eq!(describe(Task::Install { items: 1 }), "Installing 1 item...");
        assert_eq!(describe(Task::Remove { addons: 3 }), "Removing 3 addons...");
    }

    #[test]
    fn a_job_that_stops_without_reporting_says_so() {
        let (sender, results) = mpsc::channel();
        drop(sender);
        let mut job = Job::Working {
            task: Task::Remove { addons: 2 },
            results,
        };
        let mut rows = Vec::new();

        assert!(finish(&mut job, &mut rows));

        assert!(!is_busy(&job));
        assert_eq!(rows[0].addon, "Removal");
        assert_eq!(rows[0].status, Status::Failed);
    }
}
