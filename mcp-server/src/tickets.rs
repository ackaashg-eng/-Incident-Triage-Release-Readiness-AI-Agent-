//! Incident tickets: strict validation and persistent storage in SQLite.
//!
//! IDs come from SQLite's `AUTOINCREMENT` primary key (`INC-` + 1000 + row id),
//! so they are unique even across restarts and concurrent writers, and an ID is
//! never reused after a delete. The database is opened per call: a missing
//! folder is created, and a broken file produces a structured `DATABASE_ERROR`
//! for that call instead of taking the whole server down.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::data::{owner_handles, rfc3339};
use crate::error::{ErrorCode, FieldError, ToolError};

pub const TICKET_SEVERITIES: [&str; 4] = ["LOW", "MEDIUM", "HIGH", "CRITICAL"];
pub const TITLE_LEN: (usize, usize) = (5, 120);
pub const SUMMARY_LEN: (usize, usize) = (20, 4000);
const ID_OFFSET: i64 = 1000;

/// A ticket whose fields all passed validation (only `validate` builds one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTicket {
    title: String,
    severity: &'static str,
    summary: String,
    assignee: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticket {
    pub ticket_id: String,
    pub title: String,
    pub severity: String,
    pub summary: String,
    pub assignee: String,
    pub status: String,
    pub created_at: String,
}

impl NewTicket {
    /// Checks every field and reports *all* problems at once, so a client can
    /// fix them in one retry. Surrounding whitespace is trimmed.
    pub fn validate(
        title: &str,
        severity: &str,
        summary: &str,
        assignee: &str,
    ) -> Result<Self, ToolError> {
        let mut errors = Vec::new();
        let title = title.trim();
        let summary = summary.trim();
        let assignee = assignee.trim();

        check_length(&mut errors, "title", title, TITLE_LEN);
        if title.chars().any(char::is_control) {
            errors.push(field(
                "title",
                "must be a single line without control characters",
            ));
        }

        check_length(&mut errors, "summary", summary, SUMMARY_LEN);
        if summary
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t' && c != '\r')
        {
            errors.push(field(
                "summary",
                "must not contain control characters (newlines and tabs are fine)",
            ));
        }

        let severity = TICKET_SEVERITIES.iter().copied().find(|s| *s == severity);
        if severity.is_none() {
            errors.push(field(
                "severity",
                "must be one of LOW, MEDIUM, HIGH, CRITICAL (upper case)",
            ));
        }

        if !owner_handles().any(|h| h == assignee) {
            errors.push(field(
                "assignee",
                format!(
                    "must be the on-call handle of an owning team: {}",
                    owner_handles().collect::<Vec<_>>().join(", ")
                ),
            ));
        }

        match severity {
            Some(severity) if errors.is_empty() => Ok(Self {
                title: title.to_string(),
                severity,
                summary: summary.to_string(),
                assignee: assignee.to_string(),
            }),
            _ => Err(ToolError::new(
                ErrorCode::InvalidTicketFields,
                format!(
                    "{} ticket field(s) are invalid; no ticket was created",
                    errors.len()
                ),
            )
            .with_hint("Fix every field listed in field_errors and call the tool again.")
            .with_field_errors(errors)),
        }
    }
}

fn field(name: &'static str, problem: impl Into<String>) -> FieldError {
    FieldError {
        field: name,
        problem: problem.into(),
    }
}

fn check_length(
    errors: &mut Vec<FieldError>,
    name: &'static str,
    value: &str,
    (min, max): (usize, usize),
) {
    let len = value.chars().count();
    if len < min || len > max {
        errors.push(field(
            name,
            format!("must be {min}-{max} characters after trimming (got {len})"),
        ));
    }
}

#[derive(Debug, Clone)]
pub struct TicketStore {
    db_path: PathBuf,
}

impl TicketStore {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    /// Inserts the ticket and returns it with its new unique ID.
    pub fn create(&self, ticket: &NewTicket, now: DateTime<Utc>) -> Result<Ticket, ToolError> {
        let conn = self.open()?;
        let created_at = rfc3339(now);
        conn.execute(
            "INSERT INTO tickets (title, severity, summary, assignee, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'OPEN', ?5)",
            params![
                ticket.title,
                ticket.severity,
                ticket.summary,
                ticket.assignee,
                created_at
            ],
        )
        .map_err(|e| db_error("could not save the ticket", &e))?;
        let ticket_id = format_id(conn.last_insert_rowid());
        tracing::info!(%ticket_id, severity = ticket.severity, assignee = %ticket.assignee, "incident ticket created");

        Ok(Ticket {
            ticket_id,
            title: ticket.title.clone(),
            severity: ticket.severity.to_string(),
            summary: ticket.summary.clone(),
            assignee: ticket.assignee.clone(),
            status: "OPEN".to_string(),
            created_at,
        })
    }

    /// Reads one ticket back (used by tests and for verification).
    pub fn get(&self, ticket_id: &str) -> Result<Option<Ticket>, ToolError> {
        let Some(row_id) = parse_id(ticket_id) else {
            return Ok(None);
        };
        let conn = self.open()?;
        conn.query_row(
            "SELECT id, title, severity, summary, assignee, status, created_at FROM tickets WHERE id = ?1",
            params![row_id],
            |row| {
                Ok(Ticket {
                    ticket_id: format_id(row.get(0)?),
                    title: row.get(1)?,
                    severity: row.get(2)?,
                    summary: row.get(3)?,
                    assignee: row.get(4)?,
                    status: row.get(5)?,
                    created_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| db_error("could not read the ticket", &e))
    }

    pub fn count(&self) -> Result<i64, ToolError> {
        self.open()?
            .query_row("SELECT COUNT(*) FROM tickets", [], |row| row.get(0))
            .map_err(|e| db_error("could not count tickets", &e))
    }

    fn open(&self) -> Result<Connection, ToolError> {
        if let Some(parent) = self.db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| db_error("could not create the database folder", &e))?;
        }
        let conn = Connection::open(&self.db_path)
            .map_err(|e| db_error("could not open the ticket database", &e))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| db_error("could not configure the ticket database", &e))?;
        // The CHECK constraints repeat the validation rules as a last line of defence.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tickets (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 title      TEXT NOT NULL CHECK (length(title) BETWEEN 5 AND 120),
                 severity   TEXT NOT NULL CHECK (severity IN ('LOW', 'MEDIUM', 'HIGH', 'CRITICAL')),
                 summary    TEXT NOT NULL CHECK (length(summary) BETWEEN 20 AND 4000),
                 assignee   TEXT NOT NULL,
                 status     TEXT NOT NULL DEFAULT 'OPEN',
                 created_at TEXT NOT NULL
             );",
        )
        .map_err(|e| db_error("could not prepare the ticket database", &e))?;
        Ok(conn)
    }
}

fn format_id(row_id: i64) -> String {
    format!("INC-{}", row_id + ID_OFFSET)
}

fn parse_id(ticket_id: &str) -> Option<i64> {
    ticket_id
        .strip_prefix("INC-")?
        .parse::<i64>()
        .ok()
        .map(|n| n - ID_OFFSET)
        .filter(|n| *n > 0)
}

fn db_error(what: &str, err: &dyn std::fmt::Display) -> ToolError {
    // Details (paths, SQLite messages) go to the server log, not to the client.
    tracing::error!(error = %err, "{what}");
    ToolError::new(ErrorCode::DatabaseError, format!("{what}; no ticket was created"))
        .with_hint("This is a server-side problem, not a problem with your request. Do not claim a ticket exists.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const SUMMARY: &str = "Checkout failures since the 2.14.0 deploy at 09:05 UTC.";

    fn valid() -> NewTicket {
        NewTicket::validate(
            "payments-api checkout outage",
            "CRITICAL",
            SUMMARY,
            "payments-oncall",
        )
        .unwrap()
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-23T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn valid_fields_are_trimmed_and_accepted() {
        let t = NewTicket::validate(
            "  Payments outage  ",
            "HIGH",
            &format!("  {SUMMARY}\n"),
            " payments-oncall ",
        )
        .unwrap();
        assert_eq!(t.title, "Payments outage");
        assert_eq!(t.summary, SUMMARY);
        assert_eq!(t.assignee, "payments-oncall");
    }

    #[test]
    fn every_severity_and_owner_is_accepted() {
        for sev in TICKET_SEVERITIES {
            for owner in owner_handles() {
                NewTicket::validate("Valid title", sev, SUMMARY, owner).unwrap();
            }
        }
    }

    #[test]
    fn all_invalid_fields_are_reported_together() {
        let err = NewTicket::validate("hi", "URGENT", "too short", "admin").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidTicketFields);
        let fields: Vec<&str> = err.field_errors.iter().map(|f| f.field).collect();
        assert_eq!(fields, ["title", "summary", "severity", "assignee"]);
    }

    #[test]
    fn individual_rules() {
        let cases = [
            (
                NewTicket::validate("   ", "LOW", SUMMARY, "payments-oncall"),
                "title",
            ),
            (
                NewTicket::validate(&"x".repeat(121), "LOW", SUMMARY, "payments-oncall"),
                "title",
            ),
            (
                NewTicket::validate("Two\nlines", "LOW", SUMMARY, "payments-oncall"),
                "title",
            ),
            (
                NewTicket::validate("Valid title", "LOW", &"x".repeat(4001), "payments-oncall"),
                "summary",
            ),
            (
                NewTicket::validate(
                    "Valid title",
                    "LOW",
                    "bell \u{7} character in the text",
                    "payments-oncall",
                ),
                "summary",
            ),
            (
                NewTicket::validate("Valid title", "critical", SUMMARY, "payments-oncall"),
                "severity",
            ),
            (
                NewTicket::validate("Valid title", "", SUMMARY, "payments-oncall"),
                "severity",
            ),
            (
                NewTicket::validate("Valid title", "LOW", SUMMARY, "admin"),
                "assignee",
            ),
            (
                NewTicket::validate("Valid title", "LOW", SUMMARY, "Payments-Oncall"),
                "assignee",
            ),
            (
                NewTicket::validate("Valid title", "LOW", SUMMARY, ""),
                "assignee",
            ),
        ];
        for (result, expected) in cases {
            let err = result.unwrap_err();
            assert_eq!(err.field_errors.len(), 1, "{err:?}");
            assert_eq!(err.field_errors[0].field, expected);
        }
    }

    #[test]
    fn tickets_persist_across_store_instances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("tickets.db"); // folder does not exist yet
        let created = TicketStore::new(&path).create(&valid(), now()).unwrap();
        assert_eq!(created.ticket_id, "INC-1001");
        assert_eq!(created.created_at, "2026-11-23T10:00:00Z");
        assert_eq!(created.status, "OPEN");

        let reopened = TicketStore::new(&path);
        assert_eq!(reopened.get("INC-1001").unwrap(), Some(created));
        assert_eq!(reopened.count().unwrap(), 1);
        assert_eq!(reopened.get("INC-9999").unwrap(), None);
        assert_eq!(reopened.get("garbage").unwrap(), None);
    }

    #[test]
    fn ids_are_unique_and_sequential() {
        let dir = tempfile::tempdir().unwrap();
        let store = TicketStore::new(dir.path().join("t.db"));
        let ids: Vec<String> = (0..3)
            .map(|_| store.create(&valid(), now()).unwrap().ticket_id)
            .collect();
        assert_eq!(ids, ["INC-1001", "INC-1002", "INC-1003"]);
    }

    #[test]
    fn concurrent_writers_never_share_an_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        TicketStore::new(&path).count().unwrap(); // create the table once up front
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = TicketStore::new(&path);
                std::thread::spawn(move || {
                    (0..5)
                        .map(|_| store.create(&valid(), now()).unwrap().ticket_id)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ids: HashSet<String> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(ids.len(), 40);
        assert_eq!(TicketStore::new(&path).count().unwrap(), 40);
    }

    #[test]
    fn database_errors_are_structured() {
        let dir = tempfile::tempdir().unwrap();
        // 1. The path is a folder, not a file.
        let err = TicketStore::new(dir.path())
            .create(&valid(), now())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::DatabaseError);
        // 2. The file is not a SQLite database.
        let junk = dir.path().join("junk.db");
        std::fs::write(
            &junk,
            "this is not a database, just text that is long enough to matter",
        )
        .unwrap();
        let err = TicketStore::new(&junk).create(&valid(), now()).unwrap_err();
        assert_eq!(err.code, ErrorCode::DatabaseError);
        assert!(
            !err.message.contains("junk.db"),
            "paths stay in the server log"
        );
    }
}
