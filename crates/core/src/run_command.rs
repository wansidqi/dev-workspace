use crate::{ProjectId, RunCommandId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCommand {
    pub id: RunCommandId,
    pub project_id: ProjectId,
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RunCommandError {
    #[error("run command name must not be empty")]
    EmptyName,
    #[error("run command text must not be empty")]
    EmptyCommand,
}

impl RunCommand {
    pub fn new(
        id: RunCommandId,
        project_id: ProjectId,
        name: impl Into<String>,
        command: impl Into<String>,
    ) -> Result<Self, RunCommandError> {
        let name = name.into();
        let command = command.into();
        if name.trim().is_empty() {
            return Err(RunCommandError::EmptyName);
        }
        if command.trim().is_empty() {
            return Err(RunCommandError::EmptyCommand);
        }
        Ok(Self {
            id,
            project_id,
            name,
            command,
            cwd: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_empty_name_or_command() {
        assert_eq!(
            RunCommand::new(RunCommandId(1), ProjectId(1), "", "cargo build").unwrap_err(),
            RunCommandError::EmptyName
        );
        assert_eq!(
            RunCommand::new(RunCommandId(1), ProjectId(1), "build", "").unwrap_err(),
            RunCommandError::EmptyCommand
        );
    }
}
