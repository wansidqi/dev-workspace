//! Timesheet helpers of `AppState`, in their own file (a child module of
//! `state`, so it can use the private connection) to keep `state.rs` small.

use super::AppState;
use devspace_core::{ProjectId, Repo, TimesheetRow, TimesheetSettings};
use devspace_storage::{RepoRepo, TimesheetRepo};

/// An active project and the repos its timesheet reads commits from.
#[derive(Debug, Clone)]
pub struct TimesheetProject {
    pub id: ProjectId,
    pub name: String,
    pub repos: Vec<Repo>,
}

/// One saved month: remarks plus each active project's lines.
pub struct TimesheetMonth {
    /// False until the month has been saved once; the view syncs from git
    /// automatically the first time it opens an unsaved month.
    pub saved: bool,
    pub remarks: String,
    pub tables: Vec<(ProjectId, Vec<TimesheetRow>)>,
}

impl AppState {
    pub fn timesheet_settings(&self) -> TimesheetSettings {
        TimesheetRepo::settings(&self.conn).unwrap_or_default()
    }

    pub fn save_timesheet_settings(&self, settings: &TimesheetSettings) -> Result<(), String> {
        TimesheetRepo::save_settings(&self.conn, settings).map_err(|e| e.to_string())
    }

    /// Active (unarchived) projects in sidebar order, each with its repos.
    pub fn timesheet_projects(&self) -> Vec<TimesheetProject> {
        self.projects
            .iter()
            .map(|p| TimesheetProject {
                id: p.id,
                name: p.name.clone(),
                repos: RepoRepo::list_for_project(&self.conn, p.id).unwrap_or_default(),
            })
            .collect()
    }

    /// The saved lines of every active project for `month`.
    pub fn load_timesheet_month(&self, month: &str) -> Result<TimesheetMonth, String> {
        let remarks = TimesheetRepo::remarks(&self.conn, month).map_err(|e| e.to_string())?;
        let mut tables = Vec::with_capacity(self.projects.len());
        for project in &self.projects {
            let rows =
                TimesheetRepo::rows(&self.conn, project.id, month).map_err(|e| e.to_string())?;
            tables.push((project.id, rows));
        }
        Ok(TimesheetMonth {
            saved: remarks.is_some(),
            remarks: remarks.unwrap_or_default(),
            tables,
        })
    }

    pub fn save_timesheet_month(
        &self,
        month: &str,
        remarks: &str,
        tables: &[(ProjectId, Vec<TimesheetRow>)],
    ) -> Result<(), String> {
        TimesheetRepo::save_month(&self.conn, month, remarks, tables).map_err(|e| e.to_string())
    }
}
