use std::{
    fs::File,
    io::{BufReader, Read},
    path::Path,
    time::Duration,
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Preflight,
    Warmup,
    Measuring,
    Normalizing,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Preflight => "preflight",
            Self::Warmup => "warmup",
            Self::Measuring => "measuring",
            Self::Normalizing => "normalizing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "queued" => Ok(Self::Queued),
            "preflight" => Ok(Self::Preflight),
            "warmup" => Ok(Self::Warmup),
            "measuring" => Ok(Self::Measuring),
            "normalizing" => Ok(Self::Normalizing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StoreError::InvalidState(other.to_owned())),
        }
    }

    const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (
                Self::Queued,
                Self::Preflight | Self::Failed | Self::Cancelled
            ) | (
                Self::Preflight,
                Self::Warmup | Self::Failed | Self::Cancelled
            ) | (
                Self::Warmup,
                Self::Measuring | Self::Failed | Self::Cancelled
            ) | (
                Self::Measuring,
                Self::Normalizing | Self::Failed | Self::Cancelled
            ) | (
                Self::Normalizing,
                Self::Completed | Self::Failed | Self::Cancelled
            )
        )
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: JobState,
    pub request: Value,
    pub result_path: Option<String>,
    pub error: Option<String>,
    pub worker_pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub id: i64,
    pub job_id: String,
    pub created_at: DateTime<Utc>,
    pub phase: JobState,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub job_id: String,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactIssue {
    pub artifact_id: String,
    pub path: String,
    pub reason: String,
}

pub struct Store {
    connection: Connection,
}

const DATABASE_SCHEMA_VERSION: i64 = 2;
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Time(#[from] chrono::ParseError),
    #[error("unknown job: {0}")]
    UnknownJob(String),
    #[error("unknown artifact: {0}")]
    UnknownArtifact(String),
    #[error("invalid stored job state: {0}")]
    InvalidState(String),
    #[error("invalid job transition from {from:?} to {to:?}")]
    InvalidTransition { from: JobState, to: JobState },
    #[error(
        "database schema version {found} is newer than this Reactor build supports ({supported})"
    )]
    UnsupportedSchema { found: i64, supported: i64 },
}

fn migrate(connection: &mut Connection, path: &Path) -> Result<(), StoreError> {
    let current_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version > DATABASE_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema {
            found: current_version,
            supported: DATABASE_SCHEMA_VERSION,
        });
    }
    if current_version >= DATABASE_SCHEMA_VERSION {
        return Ok(());
    }

    if path != Path::new(":memory:") {
        let journal_mode: String =
            connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current_version: i64 =
        transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if current_version < 1 {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
               id TEXT PRIMARY KEY,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL,
               state TEXT NOT NULL,
               request_json TEXT NOT NULL,
               result_path TEXT,
               error TEXT,
               worker_pid INTEGER
             );
             CREATE TABLE IF NOT EXISTS job_events (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
               created_at TEXT NOT NULL,
               phase TEXT NOT NULL,
               message TEXT NOT NULL,
               data_json TEXT
             );
             CREATE INDEX IF NOT EXISTS job_events_cursor ON job_events(job_id, id);",
        )?;
        let has_worker_pid = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('jobs') WHERE name='worker_pid')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !has_worker_pid {
            transaction.execute("ALTER TABLE jobs ADD COLUMN worker_pid INTEGER", [])?;
        }
    }

    if current_version < 2 {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS artifacts (
               id TEXT PRIMARY KEY,
               job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
               created_at TEXT NOT NULL,
               kind TEXT NOT NULL,
               path TEXT NOT NULL,
               size_bytes INTEGER NOT NULL,
               sha256 TEXT NOT NULL,
               UNIQUE(job_id, path)
             );
             CREATE INDEX IF NOT EXISTS artifacts_by_job ON artifacts(job_id, created_at);
             CREATE TABLE IF NOT EXISTS devices (
               id TEXT PRIMARY KEY,
               platform TEXT NOT NULL,
               physical INTEGER NOT NULL,
               metadata_json TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS results (
               run_id TEXT PRIMARY KEY,
               job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
               device_id TEXT,
               payload_json TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS results_by_job ON results(job_id, created_at);",
        )?;
    }

    transaction.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

impl Store {
    /// Opens the database and applies versioned migrations only when required.
    ///
    /// # Errors
    ///
    /// Returns an error when the database cannot be opened or migrated.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                StoreError::Sql(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
            })?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(DATABASE_BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut connection, path)?;
        Ok(Self { connection })
    }

    /// Returns the current on-disk schema version.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the schema metadata.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// Returns the finite `SQLite` lock wait used by this Reactor connection.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot read the connection setting.
    pub fn busy_timeout(&self) -> Result<Duration, StoreError> {
        let milliseconds = self
            .connection
            .pragma_query_value(None, "busy_timeout", |row| row.get::<_, u64>(0))?;
        Ok(Duration::from_millis(milliseconds))
    }

    /// Creates a queued job.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be serialized or persisted.
    pub fn create_job(&self, request: &Value) -> Result<Job, StoreError> {
        let now = Utc::now();
        let job = Job {
            id: Uuid::new_v4().to_string(),
            created_at: now,
            updated_at: now,
            state: JobState::Queued,
            request: request.clone(),
            result_path: None,
            error: None,
            worker_pid: None,
        };
        self.connection.execute(
            "INSERT INTO jobs(id, created_at, updated_at, state, request_json) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                job.id,
                job.created_at.to_rfc3339(),
                job.updated_at.to_rfc3339(),
                job.state.as_str(),
                serde_json::to_string(request)?
            ],
        )?;
        self.append_event(&job.id, JobState::Queued, "Job queued", None)?;
        Ok(job)
    }

    /// Moves a job to a valid next state and records an event atomically enough for recovery.
    ///
    /// # Errors
    ///
    /// Returns an error for missing jobs, invalid transitions, or persistence failures.
    pub fn transition(
        &self,
        job_id: &str,
        next: JobState,
        message: &str,
        result_path: Option<&str>,
        error: Option<&str>,
    ) -> Result<Job, StoreError> {
        let current = self
            .get_job(job_id)?
            .ok_or_else(|| StoreError::UnknownJob(job_id.to_owned()))?;
        if !current.state.can_transition_to(next) {
            return Err(StoreError::InvalidTransition {
                from: current.state,
                to: next,
            });
        }
        let now = Utc::now();
        let transaction = self.connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE jobs SET updated_at=?2, state=?3, result_path=COALESCE(?4, result_path), error=?5, worker_pid=CASE WHEN ?6 THEN NULL ELSE worker_pid END WHERE id=?1 AND state=?7",
            params![
                job_id,
                now.to_rfc3339(),
                next.as_str(),
                result_path,
                error,
                next.is_terminal(),
                current.state.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidTransition {
                from: current.state,
                to: next,
            });
        }
        transaction.execute(
            "INSERT INTO job_events(job_id, created_at, phase, message, data_json) VALUES(?1, ?2, ?3, ?4, NULL)",
            params![job_id, now.to_rfc3339(), next.as_str(), message],
        )?;
        transaction.commit()?;
        self.get_job(job_id)?
            .ok_or_else(|| StoreError::UnknownJob(job_id.to_owned()))
    }

    /// Records a terminal failure from any active phase. Repeated calls are idempotent.
    ///
    /// # Errors
    ///
    /// Returns an error if the job is missing or persistence fails.
    pub fn fail(&self, job_id: &str, error: &str) -> Result<Job, StoreError> {
        let current = self
            .get_job(job_id)?
            .ok_or_else(|| StoreError::UnknownJob(job_id.to_owned()))?;
        if current.state == JobState::Failed {
            return Ok(current);
        }
        if current.state.is_terminal() {
            return Err(StoreError::InvalidTransition {
                from: current.state,
                to: JobState::Failed,
            });
        }
        self.transition(job_id, JobState::Failed, "Job failed", None, Some(error))
    }

    /// Associates a detached worker process with a queued or active job.
    ///
    /// # Errors
    ///
    /// Returns an error when the job is missing, terminal, or persistence fails.
    pub fn set_worker_pid(&self, job_id: &str, worker_pid: u32) -> Result<Job, StoreError> {
        let current = self
            .get_job(job_id)?
            .ok_or_else(|| StoreError::UnknownJob(job_id.to_owned()))?;
        if current.state.is_terminal() {
            return Err(StoreError::InvalidTransition {
                from: current.state,
                to: current.state,
            });
        }
        self.connection.execute(
            "UPDATE jobs SET updated_at=?2, worker_pid=?3 WHERE id=?1",
            params![job_id, Utc::now().to_rfc3339(), worker_pid],
        )?;
        self.get_job(job_id)?
            .ok_or_else(|| StoreError::UnknownJob(job_id.to_owned()))
    }

    /// Reads one job.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn get_job(&self, job_id: &str) -> Result<Option<Job>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, created_at, updated_at, state, request_json, result_path, error, worker_pid FROM jobs WHERE id=?1",
                [job_id],
                row_to_job,
            )
            .optional()?
            .map(parse_job)
            .transpose()
    }

    /// Lists newest jobs first.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn list_jobs(&self, limit: u32) -> Result<Vec<Job>, StoreError> {
        self.list_jobs_page(limit, 0)
    }

    /// Lists one stable newest-first job page.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn list_jobs_page(&self, limit: u32, offset: u32) -> Result<Vec<Job>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, created_at, updated_at, state, request_json, result_path, error, worker_pid
             FROM jobs ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2",
        )?;
        statement
            .query_map(params![limit, offset], row_to_job)?
            .map(|row| row.map_err(StoreError::from).and_then(parse_job))
            .collect()
    }

    /// Counts all persisted jobs for history pagination.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query fails.
    pub fn job_count(&self) -> Result<u64, StoreError> {
        self.connection
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .map_err(StoreError::from)
    }

    /// Returns whether any persisted job is still non-terminal without applying a page limit.
    ///
    /// # Errors
    ///
    /// Returns an error when the job index cannot be queried.
    pub fn has_active_jobs(&self) -> Result<bool, StoreError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE state NOT IN ('completed', 'failed', 'cancelled'))",
            [],
            |row| row.get(0),
        )?)
    }

    /// Appends a progress event.
    ///
    /// # Errors
    ///
    /// Returns an error if optional event data cannot be serialized or persisted.
    pub fn append_event(
        &self,
        job_id: &str,
        phase: JobState,
        message: &str,
        data: Option<&Value>,
    ) -> Result<(), StoreError> {
        let data = data.map(serde_json::to_string).transpose()?;
        self.connection.execute(
            "INSERT INTO job_events(job_id, created_at, phase, message, data_json) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![job_id, Utc::now().to_rfc3339(), phase.as_str(), message, data],
        )?;
        Ok(())
    }

    /// Reads progress events after a stable cursor.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn events_after(&self, job_id: &str, cursor: i64) -> Result<Vec<JobEvent>, StoreError> {
        query_events(
            &self.connection,
            "SELECT id, job_id, created_at, phase, message, data_json FROM job_events WHERE job_id=?1 AND id>?2 ORDER BY id ASC",
            params![job_id, cursor],
        )
    }

    /// Reads at most `limit` progress events after a stable cursor.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn events_after_page(
        &self,
        job_id: &str,
        cursor: i64,
        limit: u32,
    ) -> Result<Vec<JobEvent>, StoreError> {
        query_events(
            &self.connection,
            "SELECT id, job_id, created_at, phase, message, data_json
             FROM job_events WHERE job_id=?1 AND id>?2 ORDER BY id ASC LIMIT ?3",
            params![job_id, cursor, limit],
        )
    }

    /// Reads the page immediately before `before`, retaining chronological order.
    /// Passing `i64::MAX` returns the newest page.
    ///
    /// # Errors
    ///
    /// Returns an error when stored data is invalid or the query fails.
    pub fn events_before_page(
        &self,
        job_id: &str,
        before: i64,
        limit: u32,
    ) -> Result<Vec<JobEvent>, StoreError> {
        query_events(
            &self.connection,
            "SELECT id, job_id, created_at, phase, message, data_json FROM (
               SELECT id, job_id, created_at, phase, message, data_json
               FROM job_events WHERE job_id=?1 AND id<?2 ORDER BY id DESC LIMIT ?3
             ) ORDER BY id ASC",
            params![job_id, before, limit],
        )
    }

    /// Hashes and indexes an artifact that already exists on disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the job is missing, the file cannot be read, or persistence fails.
    pub fn register_artifact(
        &self,
        job_id: &str,
        kind: &str,
        path: &Path,
    ) -> Result<Artifact, StoreError> {
        if self.get_job(job_id)?.is_none() {
            return Err(StoreError::UnknownJob(job_id.to_owned()));
        }
        let metadata = std::fs::metadata(path).map_err(to_sql_error)?;
        let sha256 = hash_file(path)?;
        let artifact = Artifact {
            id: Uuid::new_v4().to_string(),
            job_id: job_id.to_owned(),
            created_at: Utc::now(),
            kind: kind.to_owned(),
            path: path.display().to_string(),
            size_bytes: metadata.len(),
            sha256,
        };
        self.connection.execute(
            "INSERT INTO artifacts(id, job_id, created_at, kind, path, size_bytes, sha256)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(job_id, path) DO UPDATE SET
               created_at=excluded.created_at, kind=excluded.kind,
               size_bytes=excluded.size_bytes, sha256=excluded.sha256",
            params![
                artifact.id,
                artifact.job_id,
                artifact.created_at.to_rfc3339(),
                artifact.kind,
                artifact.path,
                artifact.size_bytes,
                artifact.sha256,
            ],
        )?;
        self.artifact_by_path(job_id, path)?
            .ok_or_else(|| StoreError::UnknownArtifact(path.display().to_string()))
    }

    /// Lists artifact metadata for a job without reading artifact bodies.
    ///
    /// # Errors
    ///
    /// Returns an error when artifact rows cannot be read.
    pub fn list_artifacts(&self, job_id: &str) -> Result<Vec<Artifact>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, created_at, kind, path, size_bytes, sha256
             FROM artifacts WHERE job_id=?1 ORDER BY created_at ASC",
        )?;
        statement
            .query_map([job_id], row_to_artifact)?
            .map(|row| row.map_err(StoreError::from).and_then(parse_artifact))
            .collect()
    }

    /// Verifies that every indexed artifact still exists and matches its recorded size and hash.
    ///
    /// # Errors
    ///
    /// Returns an error only when the index cannot be read. Missing or changed files are returned
    /// as issues so callers can present all integrity failures together.
    pub fn verify_artifacts(&self, job_id: &str) -> Result<Vec<ArtifactIssue>, StoreError> {
        let mut issues = Vec::new();
        for artifact in self.list_artifacts(job_id)? {
            let path = Path::new(&artifact.path);
            let reason = match std::fs::metadata(path) {
                Err(error) => Some(format!("unreadable: {error}")),
                Ok(metadata) if metadata.len() != artifact.size_bytes => Some(format!(
                    "size changed: expected {}, found {}",
                    artifact.size_bytes,
                    metadata.len()
                )),
                Ok(_) => match hash_file(path) {
                    Ok(hash) if hash == artifact.sha256 => None,
                    Ok(_) => Some("sha256 changed".to_owned()),
                    Err(error) => Some(format!("unreadable: {error}")),
                },
            };
            if let Some(reason) = reason {
                issues.push(ArtifactIssue {
                    artifact_id: artifact.id,
                    path: artifact.path,
                    reason,
                });
            }
        }
        Ok(issues)
    }

    /// Upserts device metadata used by result history filters.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata serialization or persistence fails.
    pub fn upsert_device(
        &self,
        id: &str,
        platform: &str,
        physical: bool,
        metadata: &Value,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO devices(id, platform, physical, metadata_json, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET platform=excluded.platform,
               physical=excluded.physical, metadata_json=excluded.metadata_json,
               updated_at=excluded.updated_at",
            params![
                id,
                platform,
                physical,
                serde_json::to_string(metadata)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Indexes a normalized result payload for history and recovery.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or persistence fails.
    pub fn index_result(
        &self,
        job_id: &str,
        run_id: &str,
        device_id: Option<&str>,
        payload: &Value,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO results(run_id, job_id, device_id, payload_json, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(run_id) DO UPDATE SET payload_json=excluded.payload_json,
               device_id=excluded.device_id",
            params![
                run_id,
                job_id,
                device_id,
                serde_json::to_string(payload)?,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn artifact_by_path(&self, job_id: &str, path: &Path) -> Result<Option<Artifact>, StoreError> {
        self.connection
            .query_row(
                "SELECT id, job_id, created_at, kind, path, size_bytes, sha256
                 FROM artifacts WHERE job_id=?1 AND path=?2",
                params![job_id, path.display().to_string()],
                row_to_artifact,
            )
            .optional()?
            .map(parse_artifact)
            .transpose()
    }
}

type EventRow = (i64, String, String, String, String, Option<String>);

fn query_events<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<JobEvent>, StoreError> {
    let mut statement = connection.prepare(sql)?;
    statement
        .query_map(params, |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .map(|row: Result<EventRow, rusqlite::Error>| {
            let (id, job_id, created_at, phase, message, data) = row?;
            Ok(JobEvent {
                id,
                job_id,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                phase: JobState::parse(&phase)?,
                message,
                data: data.map(|value| serde_json::from_str(&value)).transpose()?,
            })
        })
        .collect()
}

fn hash_file(path: &Path) -> Result<String, StoreError> {
    let file = File::open(path).map_err(to_sql_error)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(to_sql_error)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn to_sql_error(error: std::io::Error) -> StoreError {
    StoreError::Sql(rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

type JobRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<u32>,
);

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn parse_job(row: JobRow) -> Result<Job, StoreError> {
    Ok(Job {
        id: row.0,
        created_at: DateTime::parse_from_rfc3339(&row.1)?.with_timezone(&Utc),
        updated_at: DateTime::parse_from_rfc3339(&row.2)?.with_timezone(&Utc),
        state: JobState::parse(&row.3)?,
        request: serde_json::from_str(&row.4)?,
        result_path: row.5,
        error: row.6,
        worker_pid: row.7,
    })
}

type ArtifactRow = (String, String, String, String, String, u64, String);

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn parse_artifact(row: ArtifactRow) -> Result<Artifact, StoreError> {
    Ok(Artifact {
        id: row.0,
        job_id: row.1,
        created_at: DateTime::parse_from_rfc3339(&row.2)?.with_timezone(&Utc),
        kind: row.3,
        path: row.4,
        size_bytes: row.5,
        sha256: row.6,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_versioned_schema_and_finite_busy_timeout() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(store.schema_version().unwrap(), DATABASE_SCHEMA_VERSION);
        assert_eq!(store.busy_timeout().unwrap(), DATABASE_BUSY_TIMEOUT);
    }

    #[test]
    fn rejects_a_database_created_by_a_newer_reactor_without_modifying_it() {
        let directory = std::env::temp_dir().join(format!("reactor-future-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE future_data(value TEXT NOT NULL);\
                 INSERT INTO future_data(value) VALUES('preserve-me');\
                 PRAGMA user_version = 999;",
            )
            .unwrap();
        drop(connection);

        let Err(error) = Store::open(&database) else {
            panic!("future schema must be rejected");
        };
        assert!(matches!(
            error,
            StoreError::UnsupportedSchema {
                found: 999,
                supported: DATABASE_SCHEMA_VERSION
            }
        ));
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT value FROM future_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve-me"
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            999
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_migration_rolls_back_and_preserves_existing_history() {
        let directory = std::env::temp_dir().join(format!("reactor-rollback-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs (\
                   id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,\
                   state TEXT NOT NULL, request_json TEXT NOT NULL, result_path TEXT,\
                   error TEXT, worker_pid INTEGER\
                 );\
                 CREATE TABLE job_events (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL,\
                   created_at TEXT NOT NULL, phase TEXT NOT NULL, message TEXT NOT NULL,\
                   data_json TEXT\
                 );\
                 CREATE VIEW artifacts AS SELECT id FROM jobs;\
                 INSERT INTO jobs(\
                   id, created_at, updated_at, state, request_json\
                 ) VALUES(\
                   'history-1', '2026-08-18T00:00:00Z', '2026-08-18T00:00:00Z',\
                   'completed', '{}'
                 );\
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        assert!(Store::open(&database).is_err());
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT id FROM jobs", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "history-1"
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='devices'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        drop(connection);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn version_one_history_survives_upgrade_to_the_current_schema() {
        let directory = std::env::temp_dir().join(format!("reactor-upgrade-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs (\
                   id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,\
                   state TEXT NOT NULL, request_json TEXT NOT NULL, result_path TEXT,\
                   error TEXT, worker_pid INTEGER\
                 );\
                 CREATE TABLE job_events (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL,\
                   created_at TEXT NOT NULL, phase TEXT NOT NULL, message TEXT NOT NULL,\
                   data_json TEXT\
                 );\
                 INSERT INTO jobs(\
                   id, created_at, updated_at, state, request_json\
                 ) VALUES(\
                   'history-1', '2026-08-18T00:00:00Z', '2026-08-18T00:00:00Z',\
                   'completed', '{\"mode\":\"legacy\"}'\
                 );\
                 INSERT INTO job_events(\
                   job_id, created_at, phase, message\
                 ) VALUES(\
                   'history-1', '2026-08-18T00:00:00Z', 'completed', 'legacy complete'\
                 );\
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        drop(connection);

        let store = Store::open(&database).unwrap();
        assert_eq!(store.schema_version().unwrap(), DATABASE_SCHEMA_VERSION);
        let job = store.get_job("history-1").unwrap().unwrap();
        assert_eq!(job.request["mode"], "legacy");
        assert_eq!(store.events_after("history-1", 0).unwrap().len(), 1);
        assert!(store.list_artifacts("history-1").unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persists_job_lifecycle() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "demo" }))
            .unwrap();
        let running = store
            .transition(&job.id, JobState::Preflight, "Checking", None, None)
            .unwrap();
        assert_eq!(running.state, JobState::Preflight);
        assert_eq!(store.events_after(&job.id, 0).unwrap().len(), 2);
    }

    #[test]
    fn paginates_job_history_newest_first() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let first = store
            .create_job(&serde_json::json!({ "sequence": 1 }))
            .unwrap();
        let second = store
            .create_job(&serde_json::json!({ "sequence": 2 }))
            .unwrap();
        let third = store
            .create_job(&serde_json::json!({ "sequence": 3 }))
            .unwrap();

        assert_eq!(store.job_count().unwrap(), 3);
        assert_eq!(
            store
                .list_jobs_page(2, 0)
                .unwrap()
                .into_iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            vec![third.id, second.id]
        );
        assert_eq!(store.list_jobs_page(2, 2).unwrap()[0].id, first.id);
    }

    #[test]
    fn pages_one_hundred_thousand_events_without_loading_the_history() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "event-scale" }))
            .unwrap();
        store
            .connection
            .execute(
                "WITH RECURSIVE counter(value) AS (
                   VALUES(1) UNION ALL SELECT value + 1 FROM counter WHERE value < 100000
                 )
                 INSERT INTO job_events(job_id, created_at, phase, message, data_json)
                 SELECT ?1, ?2, 'queued', printf('event %d', value), NULL FROM counter",
                params![job.id, Utc::now().to_rfc3339()],
            )
            .unwrap();

        let started = std::time::Instant::now();
        let latest = store.events_before_page(&job.id, i64::MAX, 101).unwrap();
        assert_eq!(latest.len(), 101);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "indexed event paging exceeded the desktop responsiveness budget"
        );
        let previous = store
            .events_before_page(&job.id, latest[0].id, 101)
            .unwrap();
        assert_eq!(previous.len(), 101);
        assert!(previous.last().unwrap().id < latest[0].id);
        assert_eq!(store.events_after_page(&job.id, 0, 100).unwrap().len(), 100);
    }

    #[test]
    fn records_failure_from_queue() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "test" }))
            .unwrap();
        assert!(store.has_active_jobs().unwrap());
        let failed = store.fail(&job.id, "boom").unwrap();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.error.as_deref(), Some("boom"));
        assert!(!store.has_active_jobs().unwrap());
    }

    #[test]
    fn tracks_and_clears_worker_pid() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "worker" }))
            .unwrap();
        assert_eq!(
            store.set_worker_pid(&job.id, 42).unwrap().worker_pid,
            Some(42)
        );
        let failed = store.fail(&job.id, "stopped").unwrap();
        assert_eq!(failed.worker_pid, None);
    }

    #[test]
    fn detects_modified_artifact() {
        let directory = std::env::temp_dir().join(format!("reactor-store-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let artifact_path = directory.join("trace.json");
        std::fs::write(&artifact_path, b"original").unwrap();
        let store = Store::open(&database).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "integrity" }))
            .unwrap();
        store
            .register_artifact(&job.id, "raw_trace", &artifact_path)
            .unwrap();
        assert!(store.verify_artifacts(&job.id).unwrap().is_empty());
        std::fs::write(&artifact_path, b"changed").unwrap();
        assert_eq!(store.verify_artifacts(&job.id).unwrap().len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn only_one_worker_can_claim_a_queued_job() {
        let directory = std::env::temp_dir().join(format!("reactor-claim-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let store = Store::open(&database).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "claim" }))
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let database = database.clone();
                let job_id = job.id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let store = Store::open(&database).unwrap();
                    barrier.wait();
                    store.transition(&job_id, JobState::Preflight, "claimed", None, None)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let successes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap().is_ok())
            .filter(|succeeded| *succeeded)
            .count();
        assert_eq!(successes, 1);
        let claimed_events = store
            .events_after(&job.id, 0)
            .unwrap()
            .into_iter()
            .filter(|event| event.phase == JobState::Preflight)
            .count();
        assert_eq!(claimed_events, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reopen_does_not_rerun_migrations_while_writer_is_active() {
        let directory = std::env::temp_dir().join(format!("reactor-open-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("store.sqlite3");
        let initialized = Store::open(&database).unwrap();
        assert_eq!(
            initialized.schema_version().unwrap(),
            DATABASE_SCHEMA_VERSION
        );
        drop(initialized);

        let writer = Connection::open(&database).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        let reader = Store::open(&database).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "opening an initialized store waited on an unrelated writer"
        );
        assert_eq!(reader.schema_version().unwrap(), DATABASE_SCHEMA_VERSION);
        writer.execute_batch("ROLLBACK").unwrap();
        drop(reader);
        drop(writer);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
