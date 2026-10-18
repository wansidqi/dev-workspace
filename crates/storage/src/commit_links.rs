use devspace_core::{
    check_same_project, CommitLink, CommitLinkError, CommitLinkId, CommitLinkStatus, ProjectId,
    RepoId, TaskId,
};
use rusqlite::{ffi, params, Connection, ErrorCode, OptionalExtension};

use crate::StorageError;

const COLUMNS: &str = "id, task_id, repo_id, hash, patch_id, status, note, created_at";

/// Persistence for task <-> commit links. A commit is identified by
/// `(repo_id, hash)`; links are many-to-many (one task has many commits, one
/// commit may be mapped to several tasks) and strictly project-scoped.
pub struct CommitLinkRepo;

impl CommitLinkRepo {
    /// Links a commit to a task. Fails with `ProjectMismatch` if the task
    /// and the repo belong to different projects (the task may sit at any
    /// depth of the subtask tree), and `AlreadyLinked` if this exact
    /// `(task, repo, hash)` link exists.
    pub fn insert(conn: &Connection, link: &CommitLink) -> Result<(), StorageError> {
        let task_project =
            project_of(conn, "tasks", link.task_id.0)?.ok_or(CommitLinkError::TaskNotFound)?;
        let repo_project =
            project_of(conn, "repos", link.repo_id.0)?.ok_or(CommitLinkError::RepoNotFound)?;
        check_same_project(task_project, repo_project)?;

        let result = conn.execute(
            "INSERT INTO commit_links (id, task_id, repo_id, hash, patch_id, status, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                link.id.0,
                link.task_id.0,
                link.repo_id.0,
                link.hash,
                link.patch_id,
                link.status.as_str(),
                link.note,
                link.created_at,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(map_constraint_error(e)),
        }
    }

    /// Tags many commits at once (the commit browser's multi-select and
    /// range select). Links that already exist are skipped rather than
    /// failing the batch; any other error rolls the whole batch back.
    /// Returns how many new links were created.
    pub fn insert_skipping_existing(
        conn: &Connection,
        links: &[CommitLink],
    ) -> Result<usize, StorageError> {
        let tx = conn.unchecked_transaction()?;
        let mut created = 0;
        for link in links {
            match Self::insert(&tx, link) {
                Ok(()) => created += 1,
                Err(StorageError::CommitLink(CommitLinkError::AlreadyLinked)) => {}
                Err(e) => return Err(e),
            }
        }
        tx.commit()?;
        Ok(created)
    }

    pub fn get(conn: &Connection, id: CommitLinkId) -> Result<Option<CommitLink>, StorageError> {
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM commit_links WHERE id = ?1"),
            params![id.0],
            from_row,
        )
        .optional()
        .map_err(StorageError::from)
    }

    /// A task's linked commits, oldest link first (the cherry-pick order).
    pub fn list_for_task(conn: &Connection, task: TaskId) -> Result<Vec<CommitLink>, StorageError> {
        query_all(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM commit_links WHERE task_id = ?1 ORDER BY created_at, id"
            ),
            params![task.0],
        )
    }

    /// Every task a given commit is mapped to (for the commit's badges).
    pub fn list_for_commit(
        conn: &Connection,
        repo: RepoId,
        hash: &str,
    ) -> Result<Vec<CommitLink>, StorageError> {
        query_all(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM commit_links WHERE repo_id = ?1 AND hash = ?2 ORDER BY id"
            ),
            params![repo.0, hash],
        )
    }

    /// All links in a project, via its tasks. Drives the "unassigned
    /// commits" view and the commit list's badges.
    pub fn list_for_project(
        conn: &Connection,
        project: ProjectId,
    ) -> Result<Vec<CommitLink>, StorageError> {
        query_all(
            conn,
            "SELECT l.id, l.task_id, l.repo_id, l.hash, l.patch_id, l.status, l.note, l.created_at
             FROM commit_links l JOIN tasks t ON t.id = l.task_id
             WHERE t.project_id = ?1 ORDER BY l.created_at, l.id",
            params![project.0],
        )
    }

    /// Sets a link's status and note (needs migration, cherry-picked,
    /// deployed, ...).
    pub fn update_status_and_note(
        conn: &Connection,
        id: CommitLinkId,
        status: CommitLinkStatus,
        note: Option<&str>,
    ) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE commit_links SET status = ?2, note = ?3 WHERE id = ?1",
            params![id.0, status.as_str(), note],
        )?;
        Ok(())
    }

    pub fn delete(conn: &Connection, id: CommitLinkId) -> Result<(), StorageError> {
        conn.execute("DELETE FROM commit_links WHERE id = ?1", params![id.0])?;
        Ok(())
    }

    /// How many links point at a repo. Unlinking a repo cascades to its
    /// links, so the UI must call this first and warn rather than letting
    /// them vanish silently (CLAUDE.md storage rules).
    pub fn count_for_repo(conn: &Connection, repo: RepoId) -> Result<usize, StorageError> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commit_links WHERE repo_id = ?1",
            params![repo.0],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }
}

fn project_of(conn: &Connection, table: &str, id: i64) -> Result<Option<ProjectId>, StorageError> {
    // `table` is only ever one of two string literals from this module.
    conn.query_row(
        &format!("SELECT project_id FROM {table} WHERE id = ?1"),
        params![id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|p| p.map(ProjectId))
    .map_err(StorageError::from)
}

/// Turns the database's own constraint failures into domain errors, so the
/// unique index and project triggers surface the same way as the checks in
/// `insert` (they also guard writes that bypass this repository).
fn map_constraint_error(e: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(err, _) = &e {
        if err.code == ErrorCode::ConstraintViolation {
            match err.extended_code {
                ffi::SQLITE_CONSTRAINT_UNIQUE => return CommitLinkError::AlreadyLinked.into(),
                ffi::SQLITE_CONSTRAINT_TRIGGER => return CommitLinkError::ProjectMismatch.into(),
                _ => {}
            }
        }
    }
    e.into()
}

fn query_all(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<CommitLink>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}

fn from_row(row: &rusqlite::Row) -> rusqlite::Result<CommitLink> {
    let status: String = row.get(5)?;
    let status = CommitLinkStatus::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            format!("unknown commit link status {status:?}").into(),
        )
    })?;
    Ok(CommitLink {
        id: CommitLinkId(row.get(0)?),
        task_id: TaskId(row.get(1)?),
        repo_id: RepoId(row.get(2)?),
        hash: row.get(3)?,
        patch_id: row.get(4)?,
        status,
        note: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        migrations::run_migrations, open_in_memory, projects::ProjectRepo, repos::RepoRepo,
        tasks::TaskRepo,
    };
    use devspace_core::{Project, Repo, Task};

    /// Project 1: repos 1 and 2, tasks 1 -> 2 -> 3 (three levels deep).
    /// Project 2: repo 3, task 4.
    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for p in [1, 2] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(p), "P", 0).unwrap()).unwrap();
        }
        for (id, project) in [(1, 1), (2, 1), (3, 2)] {
            let repo = Repo::new(RepoId(id), ProjectId(project), "/r", format!("r{id}")).unwrap();
            RepoRepo::insert(&conn, &repo).unwrap();
        }
        let mut t1 = Task::new(TaskId(1), ProjectId(1), "root", 0).unwrap();
        let mut t2 = Task::new(TaskId(2), ProjectId(1), "child", 0).unwrap();
        let mut t3 = Task::new(TaskId(3), ProjectId(1), "grandchild", 0).unwrap();
        let t4 = Task::new(TaskId(4), ProjectId(2), "other project", 0).unwrap();
        t2.parent_id = Some(TaskId(1));
        t3.parent_id = Some(TaskId(2));
        t1.parent_id = None;
        for t in [&t1, &t2, &t3, &t4] {
            TaskRepo::insert(&conn, t).unwrap();
        }
        conn
    }

    fn link(id: i64, task: i64, repo: i64, hash: &str, at: i64) -> CommitLink {
        CommitLink::new(CommitLinkId(id), TaskId(task), RepoId(repo), hash, at).unwrap()
    }

    fn err(result: Result<(), StorageError>) -> CommitLinkError {
        match result.unwrap_err() {
            StorageError::CommitLink(e) => e,
            other => panic!("expected a commit link error, got {other:?}"),
        }
    }

    #[test]
    fn insert_then_get_round_trips() {
        let conn = setup();
        let mut l = link(1, 1, 1, "abc123", 5);
        l.patch_id = Some("pid".into());
        l.note = Some("run migration".into());
        l.status = CommitLinkStatus::NeedsMigration;
        CommitLinkRepo::insert(&conn, &l).unwrap();
        assert_eq!(
            CommitLinkRepo::get(&conn, CommitLinkId(1)).unwrap(),
            Some(l)
        );
    }

    #[test]
    fn links_to_tasks_at_any_depth_in_the_same_project() {
        let conn = setup();
        for (id, task) in [(1, 1), (2, 2), (3, 3)] {
            CommitLinkRepo::insert(&conn, &link(id, task, 1, "abc", id)).unwrap();
        }
        let all = CommitLinkRepo::list_for_project(&conn, ProjectId(1)).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn rejects_a_task_from_another_project() {
        let conn = setup();
        // Task 4 is in project 2, repo 1 is in project 1; and the reverse.
        assert_eq!(
            err(CommitLinkRepo::insert(&conn, &link(1, 4, 1, "abc", 0))),
            CommitLinkError::ProjectMismatch
        );
        assert_eq!(
            err(CommitLinkRepo::insert(&conn, &link(2, 1, 3, "abc", 0))),
            CommitLinkError::ProjectMismatch
        );
        assert!(CommitLinkRepo::list_for_project(&conn, ProjectId(1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn database_itself_rejects_cross_project_links() {
        // Bypasses the repository entirely: the trigger is the backstop.
        let conn = setup();
        let insert = conn.execute(
            "INSERT INTO commit_links (id, task_id, repo_id, hash, status, created_at)
             VALUES (1, 4, 1, 'abc', 'linked', 0)",
            [],
        );
        assert!(insert.is_err());

        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "abc", 0)).unwrap();
        let update = conn.execute("UPDATE commit_links SET task_id = 4 WHERE id = 1", []);
        assert!(
            update.is_err(),
            "re-pointing a link across projects is blocked"
        );
    }

    #[test]
    fn same_commit_may_link_to_several_tasks_but_not_twice_to_one() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "abc", 0)).unwrap();
        CommitLinkRepo::insert(&conn, &link(2, 2, 1, "abc", 0)).unwrap();
        assert_eq!(
            err(CommitLinkRepo::insert(&conn, &link(3, 1, 1, "abc", 0))),
            CommitLinkError::AlreadyLinked
        );
        let tasks: Vec<_> = CommitLinkRepo::list_for_commit(&conn, RepoId(1), "abc")
            .unwrap()
            .iter()
            .map(|l| l.task_id)
            .collect();
        assert_eq!(tasks, vec![TaskId(1), TaskId(2)]);
    }

    #[test]
    fn same_hash_in_different_repos_is_a_different_commit() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "abc1234", 0)).unwrap();
        // Same task, same hash text, other repo of the same project: allowed.
        CommitLinkRepo::insert(&conn, &link(2, 1, 2, "abc1234", 0)).unwrap();
        assert_eq!(
            CommitLinkRepo::list_for_commit(&conn, RepoId(1), "abc1234")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            CommitLinkRepo::list_for_task(&conn, TaskId(1))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn unknown_task_or_repo_is_reported() {
        let conn = setup();
        assert_eq!(
            err(CommitLinkRepo::insert(&conn, &link(1, 99, 1, "a", 0))),
            CommitLinkError::TaskNotFound
        );
        assert_eq!(
            err(CommitLinkRepo::insert(&conn, &link(2, 1, 99, "a", 0))),
            CommitLinkError::RepoNotFound
        );
    }

    #[test]
    fn bulk_insert_skips_existing_and_is_atomic_on_real_errors() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "a", 0)).unwrap();

        let created = CommitLinkRepo::insert_skipping_existing(
            &conn,
            &[
                link(2, 1, 1, "a", 0),
                link(3, 1, 1, "b", 0),
                link(4, 1, 1, "c", 0),
            ],
        )
        .unwrap();
        assert_eq!(created, 2, "existing (task, repo, hash) skipped");

        // A cross-project link in the batch rolls back the valid ones too.
        let result = CommitLinkRepo::insert_skipping_existing(
            &conn,
            &[link(5, 1, 1, "d", 0), link(6, 4, 1, "e", 0)],
        );
        assert!(result.is_err());
        assert_eq!(
            CommitLinkRepo::list_for_task(&conn, TaskId(1))
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn list_for_task_is_in_link_order_and_scoped_to_the_task() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "late", 20)).unwrap();
        CommitLinkRepo::insert(&conn, &link(2, 1, 1, "early", 10)).unwrap();
        CommitLinkRepo::insert(&conn, &link(3, 2, 1, "other", 5)).unwrap();
        let hashes: Vec<_> = CommitLinkRepo::list_for_task(&conn, TaskId(1))
            .unwrap()
            .into_iter()
            .map(|l| l.hash)
            .collect();
        assert_eq!(hashes, vec!["early", "late"]);
    }

    #[test]
    fn update_status_and_delete() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "a", 0)).unwrap();
        CommitLinkRepo::update_status_and_note(
            &conn,
            CommitLinkId(1),
            CommitLinkStatus::Deployed,
            Some("shipped"),
        )
        .unwrap();
        let l = CommitLinkRepo::get(&conn, CommitLinkId(1))
            .unwrap()
            .unwrap();
        assert_eq!(l.status, CommitLinkStatus::Deployed);
        assert_eq!(l.note.as_deref(), Some("shipped"));

        CommitLinkRepo::delete(&conn, CommitLinkId(1)).unwrap();
        assert!(CommitLinkRepo::get(&conn, CommitLinkId(1))
            .unwrap()
            .is_none());
    }

    #[test]
    fn deleting_a_task_cascades_and_unlinking_a_repo_can_be_counted_first() {
        let conn = setup();
        CommitLinkRepo::insert(&conn, &link(1, 1, 1, "a", 0)).unwrap();
        CommitLinkRepo::insert(&conn, &link(2, 2, 1, "a", 0)).unwrap();
        CommitLinkRepo::insert(&conn, &link(3, 2, 2, "z", 0)).unwrap();

        assert_eq!(CommitLinkRepo::count_for_repo(&conn, RepoId(1)).unwrap(), 2);
        assert_eq!(CommitLinkRepo::count_for_repo(&conn, RepoId(2)).unwrap(), 1);

        // Deleting task 2 (cascade) removes its links but not task 1's.
        conn.execute("DELETE FROM tasks WHERE id = 2", []).unwrap();
        assert_eq!(CommitLinkRepo::count_for_repo(&conn, RepoId(1)).unwrap(), 1);

        // Unlinking the repo cascades too; the count is what the UI warns with.
        RepoRepo::delete(&conn, RepoId(1)).unwrap();
        assert_eq!(CommitLinkRepo::count_for_repo(&conn, RepoId(1)).unwrap(), 0);
    }

    #[test]
    fn migration_two_applies_on_top_of_an_existing_v1_database() {
        // A database created before migration 2 must upgrade cleanly.
        let mut conn = open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY);")
            .unwrap();
        run_migrations(&mut conn).unwrap();
        let versions: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(versions, 2);
    }
}
