use devspace_core::{
    Estimate, ProjectId, RowSource, TimesheetCommit, TimesheetRow, TimesheetSettings,
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::StorageError;

/// Persistence for the monthly timesheet: workspace-wide settings, each
/// month's remarks and each project's task lines with their commit snapshots.
pub struct TimesheetRepo;

impl TimesheetRepo {
    /// The saved settings, or the defaults before the first save.
    pub fn settings(conn: &Connection) -> Result<TimesheetSettings, StorageError> {
        let saved = conn
            .query_row(
                "SELECT name, role, title, authors, min_rows, est_base, est_line_factor, \
                 est_per_extra_commit, est_per_extra_day, est_max_per_day \
                 FROM timesheet_settings WHERE id = 1",
                [],
                |r| {
                    Ok(TimesheetSettings {
                        name: r.get(0)?,
                        role: r.get(1)?,
                        title: r.get(2)?,
                        authors: r.get(3)?,
                        min_rows: usize::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                        estimate: Estimate {
                            base: r.get(5)?,
                            line_factor: r.get(6)?,
                            per_extra_commit: r.get(7)?,
                            per_extra_day: r.get(8)?,
                            max_per_day: r.get(9)?,
                        },
                    })
                },
            )
            .optional()?;
        Ok(saved.unwrap_or_default())
    }

    pub fn save_settings(
        conn: &Connection,
        settings: &TimesheetSettings,
    ) -> Result<(), StorageError> {
        let e = &settings.estimate;
        conn.execute(
            "INSERT INTO timesheet_settings (id, name, role, title, authors, min_rows, est_base, \
             est_line_factor, est_per_extra_commit, est_per_extra_day, est_max_per_day) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(id) DO UPDATE SET name = ?1, role = ?2, title = ?3, authors = ?4, \
             min_rows = ?5, est_base = ?6, est_line_factor = ?7, est_per_extra_commit = ?8, \
             est_per_extra_day = ?9, est_max_per_day = ?10",
            params![
                settings.name,
                settings.role,
                settings.title,
                settings.authors,
                i64::try_from(settings.min_rows).unwrap_or(i64::MAX),
                e.base,
                e.line_factor,
                e.per_extra_commit,
                e.per_extra_day,
                e.max_per_day,
            ],
        )?;
        Ok(())
    }

    /// The month's remarks, or `None` if the month has never been saved.
    pub fn remarks(conn: &Connection, month: &str) -> Result<Option<String>, StorageError> {
        conn.query_row(
            "SELECT remarks FROM timesheet_reports WHERE month = ?1",
            params![month],
            |r| r.get(0),
        )
        .optional()
        .map_err(StorageError::from)
    }

    /// Saves a month's remarks together with every project's lines in one
    /// transaction. A project missing from `tables` keeps its saved lines.
    pub fn save_month(
        conn: &Connection,
        month: &str,
        remarks: &str,
        tables: &[(ProjectId, Vec<TimesheetRow>)],
    ) -> Result<(), StorageError> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO timesheet_reports (month, remarks) VALUES (?1, ?2) \
             ON CONFLICT(month) DO UPDATE SET remarks = ?2",
            params![month, remarks],
        )?;
        for (project, rows) in tables {
            // Commit snapshots go with their rows (ON DELETE CASCADE).
            tx.execute(
                "DELETE FROM timesheet_rows WHERE project_id = ?1 AND month = ?2",
                params![project.0, month],
            )?;
            for (position, row) in rows.iter().enumerate() {
                tx.execute(
                    "INSERT INTO timesheet_rows (project_id, month, key, title, date_from, \
                     date_to, hours, auto, source, position) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        project.0,
                        month,
                        row.key,
                        row.title,
                        row.date_from,
                        row.date_to,
                        row.hours,
                        row.auto,
                        row.source.as_str(),
                        i64::try_from(position).unwrap_or(i64::MAX),
                    ],
                )?;
                let row_id = tx.last_insert_rowid();
                for c in &row.commits {
                    tx.execute(
                        "INSERT OR IGNORE INTO timesheet_commits \
                         (row_id, repo, hash, subject, date, lines) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![row_id, c.repo, c.hash, c.subject, c.date, c.lines],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A project's lines for a month in saved order, commits oldest first.
    pub fn rows(
        conn: &Connection,
        project: ProjectId,
        month: &str,
    ) -> Result<Vec<TimesheetRow>, StorageError> {
        let mut stmt = conn.prepare(
            "SELECT id, key, title, date_from, date_to, hours, auto, source FROM timesheet_rows \
             WHERE project_id = ?1 AND month = ?2 ORDER BY position",
        )?;
        let mut rows = stmt
            .query_map(params![project.0, month], |r| {
                let source: String = r.get(7)?;
                Ok((
                    r.get::<_, i64>(0)?,
                    TimesheetRow {
                        key: r.get(1)?,
                        title: r.get(2)?,
                        date_from: r.get(3)?,
                        date_to: r.get(4)?,
                        hours: r.get(5)?,
                        auto: r.get(6)?,
                        // Only this code writes the column; an unknown value
                        // would mean a hand-edited database, so treat the
                        // line as hand-typed (never overwritten by a sync).
                        source: RowSource::parse(&source).unwrap_or(RowSource::Manual),
                        commits: Vec::new(),
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut commits = conn.prepare(
            "SELECT repo, hash, subject, date, lines FROM timesheet_commits \
             WHERE row_id = ?1 ORDER BY date, repo, hash",
        )?;
        for (id, row) in &mut rows {
            row.commits = commits
                .query_map(params![*id], |r| {
                    Ok(TimesheetCommit {
                        repo: r.get(0)?,
                        hash: r.get(1)?,
                        subject: r.get(2)?,
                        date: r.get(3)?,
                        lines: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{migrations::run_migrations, open_in_memory, projects::ProjectRepo};
    use devspace_core::Project;

    fn setup() -> Connection {
        let mut conn = open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        for id in [1, 2] {
            ProjectRepo::insert(&conn, &Project::new(ProjectId(id), "P", 0).unwrap()).unwrap();
        }
        conn
    }

    fn row(key: &str, hours: Option<f64>, source: RowSource) -> TimesheetRow {
        TimesheetRow {
            key: key.into(),
            title: format!("task {key}"),
            date_from: "2026-10-01".into(),
            date_to: "2026-10-02".into(),
            hours,
            auto: source == RowSource::Git,
            source,
            commits: if source == RowSource::Git {
                vec![
                    TimesheetCommit {
                        repo: "app".into(),
                        hash: "bbb".into(),
                        subject: format!("{key}: second"),
                        date: "2026-10-02".into(),
                        lines: 7,
                    },
                    TimesheetCommit {
                        repo: "app".into(),
                        hash: "aaa".into(),
                        subject: format!("{key}: first"),
                        date: "2026-10-01".into(),
                        lines: 3,
                    },
                ]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn settings_default_until_saved_then_round_trip() {
        let conn = setup();
        assert_eq!(
            TimesheetRepo::settings(&conn).unwrap(),
            TimesheetSettings::default()
        );
        let mut s = TimesheetSettings {
            name: "Ann".into(),
            authors: "ann, ann@x.com".into(),
            min_rows: 5,
            ..TimesheetSettings::default()
        };
        s.estimate.max_per_day = 4.5;
        TimesheetRepo::save_settings(&conn, &s).unwrap();
        assert_eq!(TimesheetRepo::settings(&conn).unwrap(), s);
        s.name = "Bob".into();
        TimesheetRepo::save_settings(&conn, &s).unwrap();
        assert_eq!(TimesheetRepo::settings(&conn).unwrap().name, "Bob");
    }

    #[test]
    fn month_round_trips_rows_commits_and_order() {
        let conn = setup();
        assert_eq!(TimesheetRepo::remarks(&conn, "2026-10").unwrap(), None);
        let rows = vec![
            row("7267", Some(2.5), RowSource::Git),
            row("manual:1", None, RowSource::Manual),
            row("7300", None, RowSource::Git),
        ];
        TimesheetRepo::save_month(&conn, "2026-10", "note", &[(ProjectId(1), rows.clone())])
            .unwrap();

        assert_eq!(
            TimesheetRepo::remarks(&conn, "2026-10").unwrap().as_deref(),
            Some("note")
        );
        let loaded = TimesheetRepo::rows(&conn, ProjectId(1), "2026-10").unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            loaded.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["7267", "manual:1", "7300"]
        );
        assert_eq!(loaded[0].hours, Some(2.5));
        assert_eq!(loaded[1].hours, None);
        assert_eq!(loaded[1].source, RowSource::Manual);
        // Commits come back oldest first whatever order they were saved in.
        assert_eq!(loaded[0].commits[0].hash, "aaa");
        assert_eq!(loaded[0].commits[1].lines, 7);
        // Other projects and months are untouched.
        assert!(TimesheetRepo::rows(&conn, ProjectId(2), "2026-10")
            .unwrap()
            .is_empty());
        assert!(TimesheetRepo::rows(&conn, ProjectId(1), "2026-09")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn saving_again_replaces_that_projects_month_only() {
        let conn = setup();
        TimesheetRepo::save_month(
            &conn,
            "2026-10",
            "",
            &[
                (ProjectId(1), vec![row("1", Some(1.0), RowSource::Git)]),
                (ProjectId(2), vec![row("2", Some(1.0), RowSource::Git)]),
            ],
        )
        .unwrap();
        TimesheetRepo::save_month(
            &conn,
            "2026-10",
            "later",
            &[(ProjectId(1), vec![row("3", Some(2.0), RowSource::Git)])],
        )
        .unwrap();

        let one = TimesheetRepo::rows(&conn, ProjectId(1), "2026-10").unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].key, "3");
        assert_eq!(
            TimesheetRepo::rows(&conn, ProjectId(2), "2026-10")
                .unwrap()
                .len(),
            1,
            "a project missing from the save keeps its lines"
        );
        let commits: i64 = conn
            .query_row("SELECT COUNT(*) FROM timesheet_commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(commits, 4, "replaced rows leave no orphaned commits");
    }

    #[test]
    fn deleting_a_project_removes_its_timesheet_lines() {
        let conn = setup();
        TimesheetRepo::save_month(
            &conn,
            "2026-10",
            "",
            &[(ProjectId(1), vec![row("1", Some(1.0), RowSource::Git)])],
        )
        .unwrap();
        ProjectRepo::delete(&conn, ProjectId(1)).unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM timesheet_rows) + \
                 (SELECT COUNT(*) FROM timesheet_commits)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }
}
