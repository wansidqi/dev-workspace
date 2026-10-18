//! Run-command helpers of `AppState`, kept in their own file (a child module
//! of `state`, so it can use the private fields) to keep `state.rs` small.

use super::AppState;
use devspace_core::{ProjectId, RunCommand, RunCommandId};
use devspace_storage::RunCommandRepo;
use std::path::PathBuf;

impl AppState {
    /// (Re)loads the selected project's run commands. Called from
    /// `select_project`.
    pub(super) fn reload_run_commands(&mut self) {
        let Some(project) = self.selected_project else {
            self.run_commands.clear();
            return;
        };
        self.run_commands =
            RunCommandRepo::list_for_project(&self.conn, project).unwrap_or_default();
        if let Some(max) = self.run_commands.iter().map(|c| c.id.0).max() {
            self.next_run_command_id = self.next_run_command_id.max(max + 1);
        }
    }

    /// Builds a validated command: name and command text are trimmed (the
    /// core type rejects empty ones) and a blank working directory means
    /// "use the default".
    fn build_run_command(
        id: RunCommandId,
        project: ProjectId,
        name: &str,
        command: &str,
        cwd: &str,
    ) -> Result<RunCommand, String> {
        let mut cmd =
            RunCommand::new(id, project, name.trim(), command.trim()).map_err(|e| e.to_string())?;
        let cwd = cwd.trim();
        cmd.cwd = (!cwd.is_empty()).then(|| cwd.to_string());
        Ok(cmd)
    }

    /// Saves a new run command for the selected project.
    pub fn create_run_command(
        &mut self,
        name: &str,
        command: &str,
        cwd: &str,
    ) -> Result<RunCommandId, String> {
        let project = self.selected_project.ok_or("no project selected")?;
        let id = RunCommandId(Self::next_id(
            &mut self.next_run_command_id,
            self.run_commands.iter().map(|c| c.id.0).max().unwrap_or(0),
        ));
        let cmd = Self::build_run_command(id, project, name, command, cwd)?;
        RunCommandRepo::insert(&self.conn, &cmd).map_err(|e| e.to_string())?;
        self.run_commands.push(cmd);
        Ok(id)
    }

    /// Edits an existing run command of the selected project.
    pub fn update_run_command(
        &mut self,
        id: RunCommandId,
        name: &str,
        command: &str,
        cwd: &str,
    ) -> Result<(), String> {
        let project = self.selected_project.ok_or("no project selected")?;
        let pos = self
            .run_commands
            .iter()
            .position(|c| c.id == id)
            .ok_or("run command not found")?;
        let cmd = Self::build_run_command(id, project, name, command, cwd)?;
        RunCommandRepo::update(&self.conn, &cmd).map_err(|e| e.to_string())?;
        self.run_commands[pos] = cmd;
        Ok(())
    }

    /// Deletes a run command of the selected project.
    pub fn delete_run_command(&mut self, id: RunCommandId) {
        if RunCommandRepo::delete(&self.conn, id).is_ok() {
            self.run_commands.retain(|c| c.id != id);
        }
    }

    /// Where a run command starts: its own working directory if set
    /// (relative paths are resolved against the default), otherwise the
    /// default shared with new terminals (first linked repo, else home).
    pub fn run_command_cwd(&self, cmd: &RunCommand) -> PathBuf {
        let default = self.terminal_cwd();
        match cmd.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(custom) => {
                let path = PathBuf::from(custom);
                if path.is_absolute() {
                    path
                } else {
                    default.join(path)
                }
            }
            None => default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devspace_storage::{open_in_memory, run_migrations, WorkspacePaths};

    fn state() -> AppState {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        // Run commands never touch the workspace files, so this path is
        // never created.
        let root = std::env::temp_dir().join("devspace-run-commands-test-unused");
        let mut state = AppState::new(conn, WorkspacePaths::new(root));
        state.create_project("P");
        state
    }

    #[test]
    fn create_trims_validates_and_survives_reselect() {
        let mut state = state();
        assert!(state.create_run_command("  ", "cargo run", "").is_err());
        assert!(state.create_run_command("run", "   ", "").is_err());
        let id = state
            .create_run_command("  run  ", "  cargo run  ", "  ")
            .unwrap();
        let cmd = &state.run_commands[0];
        assert_eq!(
            (cmd.name.as_str(), cmd.command.as_str()),
            ("run", "cargo run")
        );
        assert_eq!(cmd.cwd, None);
        let project = state.selected_project.unwrap();
        state.select_project(project);
        assert_eq!(state.run_commands.len(), 1);
        assert_eq!(state.run_commands[0].id, id);
    }

    #[test]
    fn update_and_delete_persist_and_are_per_project() {
        let mut state = state();
        let id = state.create_run_command("a", "echo a", "").unwrap();
        state.update_run_command(id, "b", "echo b", " /x ").unwrap();
        assert!(state.update_run_command(id, "", "echo b", "").is_err());
        let first = state.selected_project.unwrap();
        state.select_project(first);
        assert_eq!(state.run_commands[0].name, "b");
        assert_eq!(state.run_commands[0].cwd.as_deref(), Some("/x"));

        state.create_project("Q");
        assert!(state.run_commands.is_empty(), "other project starts empty");
        state.select_project(first);
        state.delete_run_command(id);
        assert!(state.run_commands.is_empty());
        state.select_project(first);
        assert!(state.run_commands.is_empty());
    }

    #[test]
    fn cwd_defaults_to_the_terminal_directory_and_resolves_relative_paths() {
        let mut state = state();
        let id = state.create_run_command("a", "echo a", "").unwrap();
        let cmd = state.run_commands.iter().find(|c| c.id == id).unwrap();
        assert_eq!(state.run_command_cwd(cmd), state.terminal_cwd());
        let id = state.create_run_command("b", "echo b", "sub").unwrap();
        let cmd = state.run_commands.iter().find(|c| c.id == id).unwrap();
        assert_eq!(state.run_command_cwd(cmd), state.terminal_cwd().join("sub"));
        let abs = std::env::temp_dir();
        let id = state
            .create_run_command("c", "echo c", abs.to_str().unwrap())
            .unwrap();
        let cmd = state.run_commands.iter().find(|c| c.id == id).unwrap();
        assert_eq!(state.run_command_cwd(cmd), abs);
    }
}
