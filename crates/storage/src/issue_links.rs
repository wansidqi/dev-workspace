use devspace_core::{IssueLink, ProjectId, RepoId, TaskId};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

/// Which tasks are GitHub issues (`task_issue_links`). Tasks work without a
/// row here; a row appears when a task is published or an issue imported.
pub struct IssueLinkRepo;

impl IssueLinkRepo {
    /// Links a task to an issue, replacing the task's earlier link. The
    /// database refuses a repo from another project and a second task for
    /// the same issue.
    pub fn set(conn: &Connection, link: &IssueLink) -> Result<(), StorageError> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM task_issue_links WHERE task_id = ?1",
            params![link.task_id.0],
        )?;
        tx.execute(
            "INSERT INTO task_issue_links (task_id, repo_id, number, issue_id, remote_updated)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                link.task_id.0,
                link.repo_id.0,
                link.number as i64,
                link.issue_id as i64,
                link.remote_updated
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Remembers the issue's `updated_at` after a pull or push.
    pub fn set_remote_updated(
        conn: &Connection,
        task: TaskId,
        remote_updated: &str,
    ) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE task_issue_links SET remote_updated = ?2 WHERE task_id = ?1",
            params![task.0, remote_updated],
        )?;
        Ok(())
    }

    pub fn remove(conn: &Connection, task: TaskId) -> Result<(), StorageError> {
        conn.execute(
            "DELETE FROM task_issue_links WHERE task_id = ?1",
            params![task.0],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, task: TaskId) -> Result<Option<IssueLink>, StorageError> {
        conn.query_row(
            "SELECT task_id, repo_id, number, issue_id, remote_updated
             FROM task_issue_links WHERE task_id = ?1",
            params![task.0],
            Self::from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    /// Every link of a project's tasks, in one query.
    pub fn list_for_project(
        conn: &Connection,
        project: ProjectId,
    ) -> Result<Vec<IssueLink>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT l.task_id, l.repo_id, l.number, l.issue_id, l.remote_updated
             FROM task_issue_links l JOIN tasks t ON t.id = l.task_id
             WHERE t.project_id = ?1 ORDER BY l.repo_id, l.number",
        )?;
        let rows = stmt.query_map(params![project.0], Self::from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    fn from_row(row: &rusqlite::Row) -> rusqlite::Result<IssueLink> {
        Ok(IssueLink {
            task_id: TaskId(row.get(0)?),
            repo_id: RepoId(row.get(1)?),
            number: row.get::<_, i64>(2)? as u64,
            issue_id: row.get::<_, i64>(3)? as u64,
            remote_updated: row.get(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migrations::run_migrations, open_in_memory, projects::ProjectRepo, tasks::TaskRepo,
    };
    use devspace_core::{Project, Task};

    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for p in [1, 2] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(p), "P", 0).unwrap()).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO repos (id, project_id, path, name) VALUES (1, 1, '/a', 'a'), (2, 2, '/b', 'b');",
        )
        .unwrap();
        for (id, project) in [(1, 1), (2, 1), (3, 2)] {
            TaskRepo::insert(
                &conn,
                &Task::new(TaskId(id), ProjectId(project), "t", 0).unwrap(),
            )
            .unwrap();
        }
        conn
    }

    fn link(task: i64, repo: i64, number: u64) -> IssueLink {
        IssueLink {
            task_id: TaskId(task),
            repo_id: RepoId(repo),
            number,
            issue_id: number * 1000,
            remote_updated: "t0".into(),
        }
    }

    #[test]
    fn set_get_and_replace_round_trip() {
        let conn = setup();
        assert_eq!(IssueLinkRepo::get(&conn, TaskId(1)).unwrap(), None);
        IssueLinkRepo::set(&conn, &link(1, 1, 7)).unwrap();
        assert_eq!(
            IssueLinkRepo::get(&conn, TaskId(1)).unwrap(),
            Some(link(1, 1, 7))
        );
        IssueLinkRepo::set(&conn, &link(1, 1, 9)).unwrap();
        assert_eq!(
            IssueLinkRepo::get(&conn, TaskId(1))
                .unwrap()
                .unwrap()
                .number,
            9
        );

        IssueLinkRepo::set_remote_updated(&conn, TaskId(1), "t1").unwrap();
        assert_eq!(
            IssueLinkRepo::get(&conn, TaskId(1))
                .unwrap()
                .unwrap()
                .remote_updated,
            "t1"
        );
        IssueLinkRepo::remove(&conn, TaskId(1)).unwrap();
        assert_eq!(IssueLinkRepo::get(&conn, TaskId(1)).unwrap(), None);
    }

    #[test]
    fn an_issue_belongs_to_one_task_and_a_repo_to_its_own_project() {
        let conn = setup();
        IssueLinkRepo::set(&conn, &link(1, 1, 7)).unwrap();
        assert!(
            IssueLinkRepo::set(&conn, &link(2, 1, 7)).is_err(),
            "same issue, second task"
        );
        assert!(
            IssueLinkRepo::set(&conn, &link(3, 1, 8)).is_err(),
            "task of project 2 against a repo of project 1"
        );
        IssueLinkRepo::set(&conn, &link(3, 2, 7)).unwrap();
        assert_eq!(
            IssueLinkRepo::list_for_project(&conn, ProjectId(1)).unwrap(),
            vec![link(1, 1, 7)]
        );
    }

    #[test]
    fn deleting_the_task_or_the_repo_removes_the_link() {
        let conn = setup();
        IssueLinkRepo::set(&conn, &link(1, 1, 7)).unwrap();
        IssueLinkRepo::set(&conn, &link(2, 1, 8)).unwrap();
        TaskRepo::delete(&conn, TaskId(1)).unwrap();
        assert_eq!(IssueLinkRepo::get(&conn, TaskId(1)).unwrap(), None);
        conn.execute("DELETE FROM repos WHERE id = 1", []).unwrap();
        assert_eq!(IssueLinkRepo::get(&conn, TaskId(2)).unwrap(), None);
        assert!(TaskRepo::get(&conn, TaskId(2)).unwrap().is_some());
    }
}
