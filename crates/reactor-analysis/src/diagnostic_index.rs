//! Persistent, bounded diagnostic index and viewport-oriented queries.

#![allow(
    clippy::collapsible_if,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnecessary_wraps
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use crate::{ClockMapping, ClockQuality, ClockSyncPoint};
use reactor_protocol::{ArtifactIntegrity, ArtifactRef, CollectorStatus, NormalizedResult};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_TIMELINE_EVENTS: u64 = 500_000;
pub const MAX_CPU_SAMPLES: u64 = 2_000_000;
pub const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_DIAGNOSTIC_INPUT_BYTES: u64 = 256 * 1024 * 1024;
const SCHEMA_VERSION: u32 = 2;
const QUERY_LIMIT: u32 = 20_000;

#[derive(Debug, Error)]
pub enum DiagnosticIndexError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid diagnostic range: start must be finite, non-negative, and before end")]
    InvalidRange,
    #[error("diagnostic artifact path is unsafe: {0}")]
    UnsafeArtifactPath(String),
    #[error("diagnostic artifact integrity is not complete: {0}")]
    IncompleteArtifact(String),
    #[error("diagnostic artifact integrity check failed for {path}: {reason}")]
    ArtifactIntegrity { path: String, reason: String },
    #[error("diagnostic input limit ({MAX_DIAGNOSTIC_INPUT_BYTES} bytes) exceeded")]
    InputLimit,
    #[error("diagnostic index limit ({MAX_INDEX_BYTES} bytes) exceeded")]
    IndexLimit,
    #[error("diagnostic index belongs to run {found}, not {expected}")]
    RunMismatch { expected: String, found: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAvailability {
    pub state: String,
    pub reason: Option<String>,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticTruncation {
    pub truncated: bool,
    pub event_limit_reached: bool,
    pub sample_limit_reached: bool,
    pub byte_limit_reached: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub index_path: String,
    pub start_ms: Option<f64>,
    pub end_ms: Option<f64>,
    pub event_count: u64,
    pub sample_count: u64,
    pub index_bytes: u64,
    pub truncation: DiagnosticTruncation,
    pub availability: BTreeMap<String, DiagnosticAvailability>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineTrack {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub start_ms: Option<f64>,
    pub end_ms: Option<f64>,
    pub item_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineOverview {
    pub manifest: DiagnosticManifest,
    pub tracks: Vec<TimelineTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineItem {
    pub id: i64,
    pub track_id: i64,
    pub item_type: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub label: String,
    pub severity: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineWindow {
    pub start_ms: f64,
    pub end_ms: f64,
    pub items: Vec<TimelineItem>,
    pub returned_count: u64,
    pub clipped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionAnalysis {
    pub start_ms: f64,
    pub end_ms: f64,
    pub event_count: u64,
    pub frame_count: u64,
    pub slow_frame_count: u64,
    pub react_commit_count: u64,
    pub cpu_sample_count: u64,
    pub top_functions: Vec<RankedValue>,
    pub top_components: Vec<RankedValue>,
    pub availability: BTreeMap<String, DiagnosticAvailability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedValue {
    pub name: String,
    pub value: f64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameDrilldown {
    pub available: bool,
    pub reason: Option<String>,
    pub frame: Option<TimelineItem>,
    pub overlapping_events: Vec<TimelineItem>,
    pub react_commits: Vec<TimelineItem>,
    pub cpu_samples: Vec<RankedValue>,
    pub correlations: Vec<TimelineItem>,
}

pub struct DiagnosticIndex {
    path: PathBuf,
    connection: Connection,
}

impl DiagnosticIndex {
    /// Builds the per-run index if absent, or opens the existing complete index.
    pub fn open_or_build(
        path: &Path,
        result_base: &Path,
        result: &NormalizedResult,
    ) -> Result<Self, DiagnosticIndexError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        create_schema(&connection)?;
        let schema_matches = metadata(&connection, "schema_version")?
            .is_some_and(|version| version == SCHEMA_VERSION.to_string());
        let indexed_run = metadata(&connection, "run_id")?;
        let complete =
            metadata(&connection, "build_complete")?.as_deref() == Some("true") && schema_matches;
        if let Some(found) = indexed_run {
            if found != result.run_id {
                return Err(DiagnosticIndexError::RunMismatch {
                    expected: result.run_id.clone(),
                    found,
                });
            }
        }
        if !complete {
            if let Err(error) = rebuild(&mut connection, path, result_base, result) {
                drop(connection);
                if matches!(error, DiagnosticIndexError::IndexLimit) {
                    remove_index_files(path)?;
                }
                return Err(error);
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            connection,
        })
    }

    pub fn manifest(&self) -> Result<DiagnosticManifest, DiagnosticIndexError> {
        manifest(&self.connection, &self.path)
    }

    pub fn overview(&self) -> Result<TimelineOverview, DiagnosticIndexError> {
        let mut statement = self.connection.prepare(
            "SELECT t.id, t.kind, t.name, MIN(e.start_ms), MAX(e.end_ms), COUNT(e.id)\n             FROM tracks t LEFT JOIN timeline_events e ON e.track_id=t.id\n             GROUP BY t.id ORDER BY t.sort_order, t.id",
        )?;
        let tracks = statement
            .query_map([], |row| {
                Ok(TimelineTrack {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    name: row.get(2)?,
                    start_ms: row.get(3)?,
                    end_ms: row.get(4)?,
                    item_count: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TimelineOverview {
            manifest: self.manifest()?,
            tracks,
        })
    }

    pub fn timeline_window(
        &self,
        start_ms: f64,
        end_ms: f64,
        track_ids: &[i64],
        limit: Option<u32>,
    ) -> Result<TimelineWindow, DiagnosticIndexError> {
        validate_range(start_ms, end_ms)?;
        let limit = limit.unwrap_or(5_000).clamp(1, QUERY_LIMIT);
        let mut sql = "SELECT id, track_id, item_type, start_ms, end_ms, label, severity, data_json FROM timeline_events WHERE start_ms < ?1 AND end_ms >= ?2".to_owned();
        if !track_ids.is_empty() {
            sql.push_str(" AND track_id IN (");
            sql.push_str(
                &track_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
            sql.push(')');
        }
        sql.push_str(" ORDER BY start_ms, id LIMIT ?3");
        let mut statement = self.connection.prepare(&sql)?;
        let mut items = statement
            .query_map(params![end_ms, start_ms, limit + 1], timeline_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let clipped = items.len() > limit as usize;
        items.truncate(limit as usize);
        Ok(TimelineWindow {
            start_ms,
            end_ms,
            returned_count: items.len() as u64,
            items,
            clipped,
        })
    }

    pub fn analyze_selection(
        &self,
        start_ms: f64,
        end_ms: f64,
    ) -> Result<SelectionAnalysis, DiagnosticIndexError> {
        validate_range(start_ms, end_ms)?;
        let event_count = self.connection.query_row(
            "SELECT COUNT(*) FROM timeline_events e JOIN tracks t ON t.id=e.track_id WHERE t.kind IN ('events','flow','frames','correlations') AND e.start_ms < ?1 AND e.end_ms >= ?2",
            params![end_ms, start_ms],
            |row| row.get(0),
        )?;
        let slow_frame_count = self.connection.query_row(
            "SELECT COUNT(*) FROM frames WHERE start_ms < ?1 AND end_ms >= ?2 AND is_slow=1",
            params![end_ms, start_ms],
            |row| row.get(0),
        )?;
        let cpu_sample_count = 0;
        Ok(SelectionAnalysis {
            start_ms,
            end_ms,
            event_count,
            frame_count: self.connection.query_row(
                "SELECT COUNT(*) FROM frames WHERE start_ms < ?1 AND end_ms >= ?2",
                params![end_ms, start_ms],
                |row| row.get(0),
            )?,
            slow_frame_count,
            react_commit_count: 0,
            cpu_sample_count,
            top_functions: Vec::new(),
            top_components: Vec::new(),
            availability: availability(&self.connection)?,
        })
    }

    pub fn frame_drilldown(&self, frame_id: i64) -> Result<FrameDrilldown, DiagnosticIndexError> {
        let frame = self.connection.query_row(
            "SELECT e.id, e.track_id, e.item_type, e.start_ms, e.end_ms, e.label, e.severity, e.data_json FROM frames f JOIN timeline_events e ON e.id=f.timeline_event_id WHERE f.id=?1",
            [frame_id], timeline_row,
        ).optional()?;
        let Some(frame) = frame else {
            return Ok(FrameDrilldown {
                available: false,
                reason: Some("frame evidence is unavailable for this run or frame id".to_owned()),
                frame: None,
                overlapping_events: Vec::new(),
                react_commits: Vec::new(),
                cpu_samples: Vec::new(),
                correlations: Vec::new(),
            });
        };
        let safe_track_ids = self
            .connection
            .prepare(
                "SELECT id FROM tracks WHERE kind IN ('events','flow','frames','correlations')",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        let window = self.timeline_window(
            frame.start_ms,
            frame.end_ms.max(frame.start_ms + 0.001),
            &safe_track_ids,
            Some(2_000),
        )?;
        let react_commits = Vec::new();
        let overlapping_events = window
            .items
            .iter()
            .filter(|item| {
                item.item_type != "frame"
                    && item.item_type != "react_commit"
                    && item.item_type != "correlation"
            })
            .cloned()
            .collect();
        let correlations = window
            .items
            .iter()
            .filter(|item| item.item_type == "correlation")
            .cloned()
            .collect();
        let cpu_samples = Vec::new();
        Ok(FrameDrilldown {
            available: true,
            reason: None,
            frame: Some(frame),
            overlapping_events,
            react_commits,
            cpu_samples,
            correlations,
        })
    }
}

fn create_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;\n         CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);\n         CREATE TABLE IF NOT EXISTS clock_mappings(id INTEGER PRIMARY KEY, source_clock TEXT NOT NULL, target_clock TEXT NOT NULL, source_start REAL, source_end REAL, slope REAL NOT NULL, intercept_ms REAL NOT NULL, quality TEXT NOT NULL, details_json TEXT NOT NULL DEFAULT '{}');\n         CREATE TABLE IF NOT EXISTS tracks(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, sort_order INTEGER NOT NULL, availability TEXT NOT NULL, reason TEXT);\n         CREATE TABLE IF NOT EXISTS timeline_events(id INTEGER PRIMARY KEY, track_id INTEGER NOT NULL REFERENCES tracks(id), item_type TEXT NOT NULL, start_ms REAL NOT NULL, end_ms REAL NOT NULL, label TEXT NOT NULL, severity TEXT, data_json TEXT NOT NULL DEFAULT '{}');\n         CREATE INDEX IF NOT EXISTS timeline_range ON timeline_events(start_ms, end_ms, track_id);\n         CREATE TABLE IF NOT EXISTS frames(id INTEGER PRIMARY KEY, timeline_event_id INTEGER NOT NULL UNIQUE REFERENCES timeline_events(id), start_ms REAL NOT NULL, end_ms REAL NOT NULL, duration_ms REAL NOT NULL, is_slow INTEGER NOT NULL, source TEXT NOT NULL);\n         CREATE INDEX IF NOT EXISTS frames_range ON frames(start_ms, end_ms);\n         CREATE TABLE IF NOT EXISTS react_commits(id INTEGER PRIMARY KEY, timeline_event_id INTEGER NOT NULL UNIQUE REFERENCES timeline_events(id), root_id TEXT NOT NULL, start_ms REAL NOT NULL, end_ms REAL NOT NULL, duration_ms REAL NOT NULL);\n         CREATE INDEX IF NOT EXISTS commits_range ON react_commits(start_ms, end_ms);\n         CREATE TABLE IF NOT EXISTS commit_components(id INTEGER PRIMARY KEY, commit_id INTEGER NOT NULL REFERENCES react_commits(id), component_id TEXT NOT NULL, component_name TEXT NOT NULL, actual_duration_ms REAL NOT NULL, self_duration_ms REAL);\n         CREATE INDEX IF NOT EXISTS components_by_commit ON commit_components(commit_id);\n         CREATE TABLE IF NOT EXISTS stack_frames(id INTEGER PRIMARY KEY, external_id TEXT NOT NULL UNIQUE, parent_id INTEGER REFERENCES stack_frames(id), function_name TEXT NOT NULL, url TEXT, line INTEGER, column INTEGER);\n         CREATE TABLE IF NOT EXISTS cpu_samples(id INTEGER PRIMARY KEY, timestamp_ms REAL NOT NULL, stack_frame_id INTEGER NOT NULL REFERENCES stack_frames(id), weight_ms REAL NOT NULL);\n         CREATE INDEX IF NOT EXISTS samples_range ON cpu_samples(timestamp_ms);\n         CREATE TABLE IF NOT EXISTS correlations(id INTEGER PRIMARY KEY, timeline_event_id INTEGER NOT NULL UNIQUE REFERENCES timeline_events(id), left_type TEXT NOT NULL, left_id INTEGER NOT NULL, right_type TEXT NOT NULL, right_id INTEGER NOT NULL, confidence TEXT NOT NULL, relation TEXT NOT NULL);"
    )?;
    Ok(())
}

fn rebuild(
    connection: &mut Connection,
    path: &Path,
    base: &Path,
    result: &NormalizedResult,
) -> Result<(), DiagnosticIndexError> {
    let tx = connection.transaction()?;
    tx.execute_batch("DELETE FROM correlations; DELETE FROM cpu_samples; DELETE FROM stack_frames; DELETE FROM commit_components; DELETE FROM react_commits; DELETE FROM frames; DELETE FROM timeline_events; DELETE FROM tracks; DELETE FROM clock_mappings; DELETE FROM metadata;")?;
    set_metadata(&tx, "schema_version", &SCHEMA_VERSION.to_string())?;
    set_metadata(&tx, "run_id", &result.run_id)?;
    set_metadata(&tx, "build_complete", "false")?;
    set_metadata(&tx, "event_limit_reached", "false")?;
    set_metadata(&tx, "sample_limit_reached", "false")?;
    set_metadata(&tx, "byte_limit_reached", "false")?;
    set_metadata(&tx, "warnings", "[]")?;
    tx.execute("INSERT INTO clock_mappings(source_clock,target_clock,slope,intercept_ms,quality,details_json) VALUES('artifact','timeline',1.0,0.0,'unknown','{}')", [])?;

    let artifacts = collect_artifacts(result);
    let mut warnings = Vec::new();
    let mut budget = Budget::default();
    let event_track = add_track(&tx, "events", "Diagnostic events (elapsed realtime)", 10)?;
    let wall_event_track = add_track(
        &tx,
        "events_wall",
        "Diagnostic events (unmapped wall clock)",
        11,
    )?;
    let flow_track = add_track(&tx, "flow", "Flow steps and iterations", 15)?;
    let commit_track = add_track(&tx, "react", "React commits (React profiler clock)", 20)?;
    let frame_track = add_track(&tx, "frames", "Frames (elapsed realtime)", 30)?;
    let cpu_track = add_track(&tx, "cpu", "CPU samples (Hermes profiler clock)", 40)?;
    let correlation_track = add_track(&tx, "correlations", "Correlations", 50)?;
    mark_separate_clock(
        &tx,
        commit_track,
        "React profiler timestamps have no validated mapping to elapsed realtime",
    )?;
    mark_separate_clock(
        &tx,
        cpu_track,
        "Hermes profiler timestamps have no validated mapping to elapsed realtime",
    )?;
    let mut imported_paths = BTreeSet::new();

    for artifact in artifacts {
        let resolved = match validate_artifact(base, &artifact, &mut budget) {
            Ok(path) => path,
            Err(error) => {
                warnings.push(format!("rejected artifact {}: {error}", artifact.path));
                continue;
            }
        };
        if !imported_paths.insert(resolved.clone()) {
            continue;
        }
        let format = artifact.format.to_ascii_lowercase();
        let result = if format.contains("rn-events")
            || resolved.extension().and_then(|v| v.to_str()) == Some("ndjson")
        {
            import_ndjson(
                &tx,
                &resolved,
                event_track,
                wall_event_track,
                flow_track,
                &mut budget,
            )
        } else if format.contains("react-devtools")
            || resolved
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.contains("rn-profile"))
        {
            import_react_profile(&tx, &resolved, commit_track, &mut budget)
        } else if format.contains("cpu-profile")
            || format.contains("cpuprofile")
            || format.contains("hermes-sampling-chrome-trace-json")
            || resolved.extension().and_then(|v| v.to_str()) == Some("cpuprofile")
        {
            import_cpu_profile(&tx, &resolved, cpu_track, &mut budget)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            if matches!(error, DiagnosticIndexError::InputLimit) {
                budget.byte_limit = true;
            }
            warnings.push(format!("failed to index {}: {error}", artifact.path));
        }
        if budget.byte_limit {
            break;
        }
    }
    update_track_availability(
        &tx,
        event_track,
        "timeline_events",
        Some("track_id"),
        "no supported elapsed-realtime event artifact was available",
    )?;
    update_track_availability(
        &tx,
        wall_event_track,
        "timeline_events",
        Some("track_id"),
        "no unmapped wall-clock events were present",
    )?;
    update_track_availability(
        &tx,
        flow_track,
        "timeline_events",
        Some("track_id"),
        "no paired host-observed Flow marker boundaries were available",
    )?;
    update_track_availability(
        &tx,
        commit_track,
        "react_commits",
        None,
        "no React profiler artifact was available",
    )?;
    update_track_availability(
        &tx,
        frame_track,
        "frames",
        None,
        "raw Perfetto/xctrace frame extraction is not available in this indexer",
    )?;
    update_track_availability(
        &tx,
        cpu_track,
        "cpu_samples",
        None,
        "no supported Hermes CPU profile was available",
    )?;
    update_track_availability(
        &tx,
        correlation_track,
        "correlations",
        None,
        "correlation requires indexed frames and overlapping evidence",
    )?;
    set_metadata(&tx, "event_limit_reached", &budget.event_limit.to_string())?;
    set_metadata(
        &tx,
        "sample_limit_reached",
        &budget.sample_limit.to_string(),
    )?;
    set_metadata(&tx, "byte_limit_reached", &budget.byte_limit.to_string())?;
    set_metadata(&tx, "warnings", &serde_json::to_string(&warnings)?)?;
    set_metadata(&tx, "build_complete", "true")?;
    tx.commit()?;
    checkpoint_and_bound(connection, path)?;
    Ok(())
}

#[derive(Default)]
struct Budget {
    events: u64,
    samples: u64,
    event_limit: bool,
    sample_limit: bool,
    byte_limit: bool,
    input_bytes: u64,
}

fn reserve_event(budget: &mut Budget) -> bool {
    if budget.events >= MAX_TIMELINE_EVENTS {
        budget.event_limit = true;
        false
    } else {
        budget.events += 1;
        true
    }
}
fn reserve_sample(budget: &mut Budget) -> bool {
    if budget.samples >= MAX_CPU_SAMPLES {
        budget.sample_limit = true;
        false
    } else {
        budget.samples += 1;
        true
    }
}

fn collect_artifacts(result: &NormalizedResult) -> Vec<ArtifactRef> {
    let mut artifacts = result.artifacts.clone();
    if let Some(diagnostics) = &result.framework_diagnostics {
        if let Some(rn) = &diagnostics.react_native {
            for collector in rn
                .collectors
                .values()
                .filter(|collector| collector.status == CollectorStatus::Collected)
            {
                artifacts.extend(collector.artifacts.clone());
            }
        }
    }
    artifacts
}

fn validate_artifact(
    base: &Path,
    artifact: &ArtifactRef,
    budget: &mut Budget,
) -> Result<PathBuf, DiagnosticIndexError> {
    if artifact.integrity != ArtifactIntegrity::Complete {
        return Err(DiagnosticIndexError::IncompleteArtifact(
            artifact.path.clone(),
        ));
    }
    let relative = Path::new(&artifact.path);
    if relative.is_absolute()
        || relative.as_os_str().is_empty()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(DiagnosticIndexError::UnsafeArtifactPath(
            artifact.path.clone(),
        ));
    }
    let canonical_base = base.canonicalize()?;
    let resolved = canonical_base.join(relative).canonicalize()?;
    if !resolved.starts_with(&canonical_base) || !resolved.is_file() {
        return Err(DiagnosticIndexError::UnsafeArtifactPath(
            artifact.path.clone(),
        ));
    }
    let actual_size = std::fs::metadata(&resolved)?.len();
    if actual_size != artifact.size_bytes {
        return Err(DiagnosticIndexError::ArtifactIntegrity {
            path: artifact.path.clone(),
            reason: format!(
                "declared sizeBytes {} does not match {actual_size}",
                artifact.size_bytes
            ),
        });
    }
    if actual_size > MAX_DIAGNOSTIC_INPUT_BYTES
        || budget.input_bytes.saturating_add(actual_size) > MAX_DIAGNOSTIC_INPUT_BYTES
    {
        budget.byte_limit = true;
        return Err(DiagnosticIndexError::InputLimit);
    }
    if artifact.sha256.len() != 64 || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DiagnosticIndexError::ArtifactIntegrity {
            path: artifact.path.clone(),
            reason: "declared SHA-256 is missing or malformed".to_owned(),
        });
    }
    let mut file = File::open(&resolved)?;
    let mut hasher = Sha256::new();
    let copied = std::io::copy(&mut file.by_ref().take(actual_size + 1), &mut hasher)?;
    if copied != actual_size {
        return Err(DiagnosticIndexError::ArtifactIntegrity {
            path: artifact.path.clone(),
            reason: "artifact changed while verifying size".to_owned(),
        });
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if !actual_hash.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(DiagnosticIndexError::ArtifactIntegrity {
            path: artifact.path.clone(),
            reason: "SHA-256 mismatch".to_owned(),
        });
    }
    budget.input_bytes += actual_size;
    Ok(resolved)
}

fn bounded_json(path: &Path) -> Result<Value, DiagnosticIndexError> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_DIAGNOSTIC_INPUT_BYTES {
        return Err(DiagnosticIndexError::InputLimit);
    }
    let reader = File::open(path)?.take(size + 1);
    Ok(serde_json::from_reader(reader)?)
}

fn import_ndjson(
    tx: &Transaction<'_>,
    path: &Path,
    event_track: i64,
    wall_event_track: i64,
    flow_track: i64,
    budget: &mut Budget,
) -> Result<(), DiagnosticIndexError> {
    let mut sync_points = Vec::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)?;
        if let (Some(source_time), Some(target_nanos)) = (
            number(&value, &["timestampMs"]),
            number(&value, &["elapsedRealtimeNanos"]),
        ) {
            sync_points.push(ClockSyncPoint {
                source_time,
                target_time: target_nanos / 1_000_000.0,
                uncertainty: 1.0,
            });
        }
    }

    sync_points.sort_by(|left, right| left.source_time.total_cmp(&right.source_time));
    sync_points.dedup_by(|left, right| left.source_time.total_cmp(&right.source_time).is_eq());
    let mapping = if sync_points.len() >= 2 {
        ClockMapping::fit("unix_epoch_ms", "elapsed_realtime_ms", &sync_points, false).ok()
    } else {
        None
    };
    store_clock_mapping(tx, mapping.as_ref(), sync_points.len())?;
    let usable_mapping = mapping
        .as_ref()
        .filter(|mapping| mapping.quality != ClockQuality::Poor);
    let mut open = BTreeMap::<(String, String, String), (f64, Value)>::new();
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)?;
        let raw_timestamp = number(&value, &["timestampMs", "timestamp", "ts"]);
        let elapsed_timestamp =
            number(&value, &["elapsedRealtimeNanos"]).map(|value| value / 1_000_000.0);
        let (timestamp, clock, uncertainty) = if let Some(timestamp) = elapsed_timestamp {
            (timestamp, "elapsed_realtime_ms", 0.001)
        } else if let (Some(timestamp), Some(mapping)) = (raw_timestamp, usable_mapping) {
            let mapped = mapping.map(timestamp);
            (
                mapped.target_time,
                "elapsed_realtime_ms",
                mapped.uncertainty,
            )
        } else if let Some(timestamp) = raw_timestamp {
            (timestamp, "unix_epoch_ms_unmapped", 1.0)
        } else {
            continue;
        };
        let kind = value.get("kind").and_then(Value::as_str).unwrap_or("event");
        let mut payload = value.get("payload").cloned().unwrap_or(Value::Null);
        if let Some(object) = payload.as_object_mut() {
            object.insert("clock".to_owned(), Value::String(clock.to_owned()));
            object.insert(
                "clockUncertaintyMs".to_owned(),
                serde_json::json!(uncertainty),
            );
        }
        if kind == "flow_marker" {
            let boundary = payload
                .get("boundary")
                .and_then(Value::as_str)
                .unwrap_or("");
            let entity_type = payload
                .get("entityType")
                .and_then(Value::as_str)
                .unwrap_or("step");
            let entity_id = payload
                .get("stepId")
                .or_else(|| payload.get("iterationId"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            let key = (entity_type.to_owned(), entity_id.clone(), clock.to_owned());
            if boundary == "start" {
                open.insert(key, (timestamp, payload));
                continue;
            }
            if matches!(boundary, "end" | "failed" | "cancelled") {
                let (start, mut start_payload) = open
                    .remove(&key)
                    .unwrap_or_else(|| (timestamp, serde_json::json!({})));
                if let (Some(start), Some(end)) =
                    (start_payload.as_object_mut(), payload.as_object())
                {
                    for (key, value) in end {
                        start.insert(key.clone(), value.clone());
                    }
                }
                let state = if boundary == "end" {
                    "completed"
                } else {
                    boundary
                };
                if let Some(object) = start_payload.as_object_mut() {
                    object.insert("state".to_owned(), Value::String(state.to_owned()));
                }
                if !reserve_event(budget) {
                    break;
                }
                insert_event(
                    tx,
                    flow_track,
                    &format!("flow_{entity_type}"),
                    start,
                    timestamp,
                    &entity_id,
                    (state != "completed").then_some(state),
                    &start_payload,
                )?;
                continue;
            }
        }
        if !reserve_event(budget) {
            break;
        }
        let duration = number(&payload, &["durationMs", "duration", "actualDuration"])
            .unwrap_or(0.0)
            .max(0.0);
        let target_track = if clock == "elapsed_realtime_ms" {
            event_track
        } else {
            wall_event_track
        };
        insert_event(
            tx,
            target_track,
            kind,
            timestamp,
            timestamp + duration,
            kind,
            None,
            &payload,
        )?;
    }
    for ((entity_type, entity_id, _clock), (start, mut payload)) in open {
        if !reserve_event(budget) {
            break;
        }
        if let Some(object) = payload.as_object_mut() {
            object.insert("state".to_owned(), Value::String("open".to_owned()));
        }
        insert_event(
            tx,
            flow_track,
            &format!("flow_{entity_type}"),
            start,
            start,
            &entity_id,
            Some("open"),
            &payload,
        )?;
    }
    Ok(())
}

fn store_clock_mapping(
    tx: &Transaction<'_>,
    mapping: Option<&ClockMapping>,
    point_count: usize,
) -> Result<(), DiagnosticIndexError> {
    tx.execute("DELETE FROM clock_mappings WHERE source_clock='unix_epoch_ms' AND target_clock='elapsed_realtime_ms'", [])?;
    if let Some(mapping) = mapping {
        tx.execute(
            "INSERT INTO clock_mappings(source_clock,target_clock,source_start,source_end,slope,intercept_ms,quality,details_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![mapping.source_clock, mapping.target_clock, mapping.segments.first().map(|v| v.source_start), mapping.segments.last().map(|v| v.source_end), mapping.scale, mapping.offset, format!("{:?}", mapping.quality).to_ascii_lowercase(), serde_json::to_string(mapping)?],
        )?;
    } else {
        tx.execute(
            "INSERT INTO clock_mappings(source_clock,target_clock,slope,intercept_ms,quality,details_json) VALUES('unix_epoch_ms','elapsed_realtime_ms',1.0,0.0,'unavailable',?1)",
            [serde_json::json!({"syncPointCount": point_count, "reason": "at least two distinct timestamp pairs are required"}).to_string()],
        )?;
    }
    Ok(())
}

fn import_react_profile(
    tx: &Transaction<'_>,
    path: &Path,
    track: i64,
    budget: &mut Budget,
) -> Result<(), DiagnosticIndexError> {
    let value = bounded_json(path)?;
    for root in value
        .get("dataForRoots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let root_id = root
            .get("rootID")
            .map(Value::to_string)
            .unwrap_or_else(|| "root".to_owned());
        let names = snapshot_names(root);
        for commit in root
            .get("commitData")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !reserve_event(budget) {
                return Ok(());
            }
            let start = number(commit, &["timestamp"]).unwrap_or(0.0);
            let duration = number(commit, &["duration"]).unwrap_or(0.0).max(0.0);
            let timeline_id = insert_event(
                tx,
                track,
                "react_commit",
                start,
                start + duration,
                "React commit",
                None,
                commit,
            )?;
            tx.execute("INSERT INTO react_commits(timeline_event_id,root_id,start_ms,end_ms,duration_ms) VALUES(?1,?2,?3,?4,?5)", params![timeline_id, root_id, start, start + duration, duration])?;
            let commit_id = tx.last_insert_rowid();
            let self_durations = duration_map(commit.get("fiberSelfDurations"));
            for (component_id, actual) in duration_map(commit.get("fiberActualDurations")) {
                tx.execute("INSERT INTO commit_components(commit_id,component_id,component_name,actual_duration_ms,self_duration_ms) VALUES(?1,?2,?3,?4,?5)", params![commit_id, component_id, names.get(&component_id).unwrap_or(&component_id), actual, self_durations.get(&component_id)])?;
            }
        }
    }
    Ok(())
}

fn import_cpu_profile(
    tx: &Transaction<'_>,
    path: &Path,
    _track: i64,
    budget: &mut Budget,
) -> Result<(), DiagnosticIndexError> {
    let value = bounded_json(path)?;
    if let Some(events) = value.get("traceEvents").and_then(Value::as_array) {
        return import_hermes_trace_events(tx, events, budget);
    }
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for node in &nodes {
        let external = node.get("id").map(Value::to_string).unwrap_or_default();
        let frame = node.get("callFrame").unwrap_or(&Value::Null);
        tx.execute("INSERT OR IGNORE INTO stack_frames(external_id,function_name,url,line,column) VALUES(?1,?2,?3,?4,?5)", params![external, frame.get("functionName").and_then(Value::as_str).unwrap_or("(anonymous)"), frame.get("url").and_then(Value::as_str), frame.get("lineNumber").and_then(Value::as_i64), frame.get("columnNumber").and_then(Value::as_i64)])?;
    }
    let samples = value
        .get("samples")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let deltas = value
        .get("timeDeltas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut timestamp = value
        .get("startTime")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        / 1_000.0;
    for (index, sample) in samples.iter().enumerate() {
        if !reserve_sample(budget) {
            break;
        }
        let weight = deltas.get(index).and_then(Value::as_f64).unwrap_or(0.0) / 1_000.0;
        timestamp += weight;
        let external = sample.to_string();
        let frame_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM stack_frames WHERE external_id=?1",
                [&external],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(frame_id) = frame_id {
            tx.execute(
                "INSERT INTO cpu_samples(timestamp_ms,stack_frame_id,weight_ms) VALUES(?1,?2,?3)",
                params![timestamp, frame_id, weight],
            )?;
        }
    }
    Ok(())
}

fn import_hermes_trace_events(
    tx: &Transaction<'_>,
    events: &[Value],
    budget: &mut Budget,
) -> Result<(), DiagnosticIndexError> {
    for (index, event) in events.iter().enumerate() {
        if event.get("ph").and_then(Value::as_str) != Some("X") {
            continue;
        }
        let Some(timestamp_us) = number(event, &["ts"]) else {
            continue;
        };
        if !reserve_sample(budget) {
            break;
        }
        let duration_ms = number(event, &["dur"]).unwrap_or(0.0).max(0.0) / 1_000.0;
        let args = event.get("args").unwrap_or(&Value::Null);
        let name = event
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("(anonymous)");
        let external = format!("trace:{index}");
        tx.execute(
            "INSERT INTO stack_frames(external_id,function_name,url,line,column) VALUES(?1,?2,?3,?4,?5)",
            params![
                external,
                name,
                args.get("url").and_then(Value::as_str),
                args.get("line").or_else(|| args.get("lineNumber")).and_then(Value::as_i64),
                args.get("column").or_else(|| args.get("columnNumber")).and_then(Value::as_i64)
            ],
        )?;
        tx.execute(
            "INSERT INTO cpu_samples(timestamp_ms,stack_frame_id,weight_ms) VALUES(?1,?2,?3)",
            params![timestamp_us / 1_000.0, tx.last_insert_rowid(), duration_ms],
        )?;
    }
    Ok(())
}

fn snapshot_names(root: &Value) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for entry in root
        .get("snapshots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(pair) = entry.as_array().filter(|pair| pair.len() >= 2) {
            let id = pair[0].to_string();
            let node = &pair[1];
            names.insert(
                id,
                node.get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
            );
        } else if let Some(node) = entry.as_object() {
            let id = node.get("id").map(Value::to_string).unwrap_or_default();
            names.insert(
                id,
                node.get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("Unknown")
                    .to_owned(),
            );
        }
    }
    names
}
fn duration_map(value: Option<&Value>) -> BTreeMap<String, f64> {
    let mut values = BTreeMap::new();
    for pair in value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        if pair.len() >= 2 {
            if let Some(duration) = pair[1].as_f64().filter(|v| v.is_finite()) {
                values.insert(pair[0].to_string(), duration);
            }
        }
    }
    values
}
fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    })
}

fn insert_event(
    tx: &Transaction<'_>,
    track: i64,
    item_type: &str,
    start: f64,
    end: f64,
    label: &str,
    severity: Option<&str>,
    data: &Value,
) -> Result<i64, DiagnosticIndexError> {
    tx.execute("INSERT INTO timeline_events(track_id,item_type,start_ms,end_ms,label,severity,data_json) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![track, item_type, start.max(0.0), end.max(start).max(0.0), label, severity, serde_json::to_string(data)?])?;
    Ok(tx.last_insert_rowid())
}
fn add_track(
    tx: &Transaction<'_>,
    kind: &str,
    name: &str,
    sort: i64,
) -> Result<i64, rusqlite::Error> {
    tx.execute(
        "INSERT INTO tracks(kind,name,sort_order,availability) VALUES(?1,?2,?3,'available')",
        params![kind, name, sort],
    )?;
    Ok(tx.last_insert_rowid())
}
fn mark_separate_clock(
    tx: &Transaction<'_>,
    track_id: i64,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    tx.execute(
        "UPDATE tracks SET reason=?2 WHERE id=?1",
        params![track_id, reason],
    )?;
    Ok(())
}

fn update_track_availability(
    tx: &Transaction<'_>,
    id: i64,
    table: &str,
    filter: Option<&str>,
    reason: &str,
) -> Result<(), rusqlite::Error> {
    let sql = if let Some(column) = filter {
        format!("SELECT COUNT(*) FROM {table} WHERE {column}=?1")
    } else {
        format!("SELECT COUNT(*) FROM {table}")
    };
    let count: u64 = if filter.is_some() {
        tx.query_row(&sql, [id], |row| row.get(0))?
    } else {
        tx.query_row(&sql, [], |row| row.get(0))?
    };
    if count == 0 {
        tx.execute(
            "UPDATE tracks SET availability='unavailable', reason=?2 WHERE id=?1",
            params![id, reason],
        )?;
    }
    Ok(())
}
fn set_metadata(tx: &Transaction<'_>, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,?2)",
        params![key, value],
    )?;
    Ok(())
}
fn metadata(connection: &Connection, key: &str) -> Result<Option<String>, rusqlite::Error> {
    connection
        .query_row("SELECT value FROM metadata WHERE key=?1", [key], |row| {
            row.get(0)
        })
        .optional()
}

fn checkpoint_and_bound(connection: &Connection, path: &Path) -> Result<(), DiagnosticIndexError> {
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    if std::fs::metadata(path)?.len() > MAX_INDEX_BYTES {
        return Err(DiagnosticIndexError::IndexLimit);
    }
    Ok(())
}

fn remove_index_files(path: &Path) -> Result<(), std::io::Error> {
    for candidate in [
        path.to_path_buf(),
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ] {
        match std::fs::remove_file(candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn manifest(
    connection: &Connection,
    path: &Path,
) -> Result<DiagnosticManifest, DiagnosticIndexError> {
    let range = connection.query_row(
        "SELECT MIN(start_ms),MAX(end_ms) FROM timeline_events",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let event_count =
        connection.query_row("SELECT COUNT(*) FROM timeline_events", [], |row| row.get(0))?;
    let sample_count =
        connection.query_row("SELECT COUNT(*) FROM cpu_samples", [], |row| row.get(0))?;
    let event_limit_reached =
        metadata(connection, "event_limit_reached")?.as_deref() == Some("true");
    let sample_limit_reached =
        metadata(connection, "sample_limit_reached")?.as_deref() == Some("true");
    let byte_limit_reached = metadata(connection, "byte_limit_reached")?.as_deref() == Some("true");
    let mut reasons = Vec::new();
    if event_limit_reached {
        reasons.push(format!(
            "timeline event limit ({MAX_TIMELINE_EVENTS}) reached"
        ));
    }
    if sample_limit_reached {
        reasons.push(format!("CPU sample limit ({MAX_CPU_SAMPLES}) reached"));
    }
    if byte_limit_reached {
        reasons.push(format!("index byte limit ({MAX_INDEX_BYTES}) reached"));
    }
    Ok(DiagnosticManifest {
        schema_version: SCHEMA_VERSION,
        run_id: metadata(connection, "run_id")?.unwrap_or_default(),
        index_path: path.display().to_string(),
        start_ms: range.0,
        end_ms: range.1,
        event_count,
        sample_count,
        index_bytes: std::fs::metadata(path).map_or(0, |m| m.len()),
        truncation: DiagnosticTruncation {
            truncated: !reasons.is_empty(),
            event_limit_reached,
            sample_limit_reached,
            byte_limit_reached,
            reasons,
        },
        availability: availability(connection)?,
        warnings: serde_json::from_str(
            &metadata(connection, "warnings")?.unwrap_or_else(|| "[]".to_owned()),
        )?,
    })
}

fn availability(
    connection: &Connection,
) -> Result<BTreeMap<String, DiagnosticAvailability>, DiagnosticIndexError> {
    let mut statement =
        connection.prepare("SELECT kind,availability,reason,id FROM tracks ORDER BY sort_order")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut result = BTreeMap::new();
    for (kind, state, reason, id) in rows {
        let count = connection.query_row(
            "SELECT COUNT(*) FROM timeline_events WHERE track_id=?1",
            [id],
            |row| row.get(0),
        )?;
        result.insert(
            kind,
            DiagnosticAvailability {
                state,
                reason,
                item_count: count,
            },
        );
    }
    Ok(result)
}
fn timeline_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TimelineItem> {
    let json: String = row.get(7)?;
    Ok(TimelineItem {
        id: row.get(0)?,
        track_id: row.get(1)?,
        item_type: row.get(2)?,
        start_ms: row.get(3)?,
        end_ms: row.get(4)?,
        label: row.get(5)?,
        severity: row.get(6)?,
        data: serde_json::from_str(&json).unwrap_or(Value::Null),
    })
}
fn validate_range(start: f64, end: f64) -> Result<(), DiagnosticIndexError> {
    if start.is_finite() && end.is_finite() && start >= 0.0 && end > start {
        Ok(())
    } else {
        Err(DiagnosticIndexError::InvalidRange)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reactor_protocol::NormalizedResult;

    fn fixture_result() -> NormalizedResult {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/result-v1-diagnostics.json"
        ))
        .unwrap()
    }
    fn temp(label: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("reactor-index-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn creates_required_schema_and_explicit_unavailability() {
        let dir = temp("schema");
        let mut result = fixture_result();
        result.artifacts.clear();
        result.framework_diagnostics = None;
        let index =
            DiagnosticIndex::open_or_build(&dir.join("diagnostic-index.sqlite"), &dir, &result)
                .unwrap();
        let names = index
            .connection
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<BTreeSet<_>, _>>()
            .unwrap();
        for required in [
            "metadata",
            "clock_mappings",
            "tracks",
            "timeline_events",
            "frames",
            "react_commits",
            "commit_components",
            "stack_frames",
            "cpu_samples",
            "correlations",
        ] {
            assert!(names.contains(required), "missing {required}");
        }
        let manifest = index.manifest().unwrap();
        assert_eq!(manifest.availability["frames"].state, "unavailable");
        assert!(manifest.availability["frames"].reason.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_profiles_and_queries_only_requested_range() {
        let dir = temp("range");
        std::fs::copy(
            "../../tests/fixtures/hermes-cpu-profile.json",
            dir.join("cpu.cpuprofile"),
        )
        .unwrap();
        std::fs::copy(
            "../../tests/fixtures/react-profiler-baseline.json",
            dir.join("react.json"),
        )
        .unwrap();
        let mut result = fixture_result();
        result.artifacts = vec![
            artifact(&dir, "cpu.cpuprofile", "hermes-cpu-profile"),
            artifact(&dir, "react.json", "react-devtools-profile"),
        ];
        result.framework_diagnostics = None;
        let index =
            DiagnosticIndex::open_or_build(&dir.join("diagnostic-index.sqlite"), &dir, &result)
                .unwrap();
        let window = index.timeline_window(15.0, 17.0, &[], Some(100)).unwrap();
        assert_eq!(
            window
                .items
                .iter()
                .filter(|item| item.item_type == "react_commit")
                .count(),
            1
        );
        let analysis = index.analyze_selection(0.0, 20.0).unwrap();
        assert_eq!(analysis.react_commit_count, 0);
        assert!(analysis.top_components.is_empty());
        assert_eq!(analysis.cpu_sample_count, 0);
        assert!(analysis.top_functions.is_empty());
        assert!(index.timeline_window(2.0, 1.0, &[], None).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persists_truncation_state_at_event_limit() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let track = add_track(&tx, "events", "events", 0).unwrap();
        let flow_track = add_track(&tx, "flow", "flow", 1).unwrap();
        let path = temp("limits").join("events.ndjson");
        std::fs::write(
            &path,
            "{\"timestampMs\":1,\"kind\":\"one\"}\n{\"timestampMs\":2,\"kind\":\"two\"}\n",
        )
        .unwrap();
        let mut budget = Budget {
            events: MAX_TIMELINE_EVENTS,
            ..Budget::default()
        };
        import_ndjson(&tx, &path, track, track, flow_track, &mut budget).unwrap();
        assert!(budget.event_limit);
        assert_eq!(budget.events, MAX_TIMELINE_EVENTS);
        tx.rollback().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn pairs_flow_markers_and_maps_rn_wall_clock() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let event_track = add_track(&tx, "events", "events", 0).unwrap();
        let flow_track = add_track(&tx, "flow", "flow", 1).unwrap();
        let path = temp("markers").join("events.ndjson");
        std::fs::write(
            &path,
            concat!(
                "{\"timestampMs\":10000,\"elapsedRealtimeNanos\":1000000000,\"kind\":\"flow_marker\",\"payload\":{\"boundary\":\"start\",\"entityType\":\"step\",\"iterationId\":\"iteration:1\",\"stepId\":\"flow-step:measured[0]\",\"stepPath\":\"measured[0]\",\"source\":\"runner_host_observed\",\"uncertaintyMs\":5}}\n",
                "{\"timestampMs\":10020,\"elapsedRealtimeNanos\":1020000000,\"kind\":\"network\",\"payload\":{\"event\":\"start\"}}\n",
                "{\"timestampMs\":10050,\"elapsedRealtimeNanos\":1050000000,\"kind\":\"flow_marker\",\"payload\":{\"boundary\":\"failed\",\"entityType\":\"step\",\"iterationId\":\"iteration:1\",\"stepId\":\"flow-step:measured[0]\"}}\n",
                "{\"timestampMs\":10100,\"elapsedRealtimeNanos\":1100000000,\"kind\":\"flow_marker\",\"payload\":{\"boundary\":\"start\",\"entityType\":\"step\",\"iterationId\":\"iteration:1\",\"stepId\":\"flow-step:measured[1]\"}}\n"
            ),
        )
        .unwrap();
        let mut budget = Budget::default();
        import_ndjson(
            &tx,
            &path,
            event_track,
            event_track,
            flow_track,
            &mut budget,
        )
        .unwrap();
        let (start, end, severity, state): (f64, f64, String, String) = tx
            .query_row(
                "SELECT start_ms,end_ms,severity,json_extract(data_json,'$.state') FROM timeline_events WHERE label='flow-step:measured[0]'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!((start, end), (1000.0, 1050.0));
        assert_eq!(severity, "failed");
        assert_eq!(state, "failed");
        let open: String = tx
            .query_row(
                "SELECT json_extract(data_json,'$.state') FROM timeline_events WHERE label='flow-step:measured[1]'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(open, "open");
        let quality: String = tx
            .query_row(
                "SELECT quality FROM clock_mappings WHERE source_clock='unix_epoch_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(quality, "good");
        tx.rollback().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn poor_clock_mapping_does_not_remap_wall_only_events() {
        let connection = Connection::open_in_memory().unwrap();
        create_schema(&connection).unwrap();
        let tx = connection.unchecked_transaction().unwrap();
        let event_track = add_track(&tx, "events", "events", 0).unwrap();
        let flow_track = add_track(&tx, "flow", "flow", 1).unwrap();
        let path = temp("poor-clock").join("events.ndjson");
        std::fs::write(
            &path,
            concat!(
                "{\"timestampMs\":1000,\"elapsedRealtimeNanos\":1000000000,\"kind\":\"sync\",\"payload\":{}}\n",
                "{\"timestampMs\":2000,\"elapsedRealtimeNanos\":9000000000,\"kind\":\"sync\",\"payload\":{}}\n",
                "{\"timestampMs\":1500,\"kind\":\"wall_only\",\"payload\":{}}\n"
            ),
        )
        .unwrap();
        import_ndjson(
            &tx,
            &path,
            event_track,
            event_track,
            flow_track,
            &mut Budget::default(),
        )
        .unwrap();
        let (timestamp, clock): (f64, String) = tx
            .query_row(
                "SELECT start_ms,json_extract(data_json,'$.clock') FROM timeline_events WHERE item_type='wall_only'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((timestamp - 1500.0).abs() < f64::EPSILON);
        assert_eq!(clock, "unix_epoch_ms_unmapped");
        tx.rollback().unwrap();
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn rejects_traversal_symlink_escape_and_tampered_artifacts() {
        let dir = temp("artifact-security");
        let outside = dir
            .parent()
            .unwrap()
            .join(format!("outside-{}", std::process::id()));
        std::fs::write(&outside, "{}").unwrap();
        let mut budget = Budget::default();
        let traversal = ArtifactRef {
            path: format!("../{}", outside.file_name().unwrap().to_string_lossy()),
            format: "react-devtools-profile".to_owned(),
            size_bytes: 2,
            sha256: format!("{:x}", Sha256::digest(b"{}")),
            producer: "test".to_owned(),
            producer_version: "1".to_owned(),
            capture_method: "test".to_owned(),
            integrity: ArtifactIntegrity::Complete,
            time_range: None,
        };
        assert!(matches!(
            validate_artifact(&dir, &traversal, &mut budget),
            Err(DiagnosticIndexError::UnsafeArtifactPath(_))
        ));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, dir.join("escape.json")).unwrap();
            let mut escape = traversal.clone();
            escape.path = "escape.json".to_owned();
            assert!(matches!(
                validate_artifact(&dir, &escape, &mut budget),
                Err(DiagnosticIndexError::UnsafeArtifactPath(_))
            ));
        }
        std::fs::write(dir.join("tampered.json"), "tampered").unwrap();
        let mut tampered = artifact(&dir, "tampered.json", "react-devtools-profile");
        tampered.sha256 = "0".repeat(64);
        assert!(matches!(
            validate_artifact(&dir, &tampered, &mut budget),
            Err(DiagnosticIndexError::ArtifactIntegrity { .. })
        ));
        tampered.integrity = ArtifactIntegrity::Partial;
        assert!(matches!(
            validate_artifact(&dir, &tampered, &mut budget),
            Err(DiagnosticIndexError::IncompleteArtifact(_))
        ));
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn imports_actual_hermes_trace_shape_on_separate_clock() {
        let dir = temp("hermes-trace");
        std::fs::write(
            dir.join("hermes.json"),
            r#"{"traceEvents":[{"pid":1,"tid":2,"ph":"X","name":"renderList","ts":12000,"dur":2500,"args":{"url":"index.bundle","line":7,"column":3}},{"ph":"M","name":"thread_name","ts":0}]}"#,
        )
        .unwrap();
        let mut result = fixture_result();
        result.framework_diagnostics = None;
        result.artifacts = vec![artifact(
            &dir,
            "hermes.json",
            "hermes-sampling-chrome-trace-json",
        )];
        let index =
            DiagnosticIndex::open_or_build(&dir.join("index.sqlite"), &dir, &result).unwrap();
        assert_eq!(index.manifest().unwrap().sample_count, 1);
        let (timestamp, name): (f64, String) = index
            .connection
            .query_row(
                "SELECT cs.timestamp_ms,sf.function_name FROM cpu_samples cs JOIN stack_frames sf ON sf.id=cs.stack_frame_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((timestamp - 12.0).abs() < f64::EPSILON);
        assert_eq!(name, "renderList");
        assert!(
            index.overview().unwrap().tracks.iter().any(|track| {
                track.kind == "cpu" && track.name.contains("Hermes profiler clock")
            })
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn enforces_input_budget_before_reading() {
        let dir = temp("input-bound");
        std::fs::write(dir.join("small.json"), "{}").unwrap();
        let artifact = artifact(&dir, "small.json", "react-devtools-profile");
        let mut budget = Budget {
            input_bytes: MAX_DIAGNOSTIC_INPUT_BYTES - 1,
            ..Budget::default()
        };
        assert!(matches!(
            validate_artifact(&dir, &artifact, &mut budget),
            Err(DiagnosticIndexError::InputLimit)
        ));
        assert!(budget.byte_limit);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn artifact(base: &Path, path: &str, format: &str) -> ArtifactRef {
        let bytes = std::fs::read(base.join(path)).unwrap();
        ArtifactRef {
            path: path.to_owned(),
            format: format.to_owned(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            producer: "test".to_owned(),
            producer_version: "1".to_owned(),
            capture_method: "fixture".to_owned(),
            integrity: reactor_protocol::ArtifactIntegrity::Complete,
            time_range: None,
        }
    }
}
