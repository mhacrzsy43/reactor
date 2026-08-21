#![allow(clippy::cast_precision_loss)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use chrono::Utc;
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use reactor_core::{
    CompiledInputBinding, aggregate_iterations, compile_maestro, mean, percentile,
    render_html_report,
};
use reactor_protocol::{
    AndroidMemoryCheckpoint, AndroidMemoryLeakReport, AndroidNativeMetrics, ArtifactIntegrity,
    ArtifactRef, BuildIdentityV1, CollectorDiagnosticV1, CollectorStatus, Coordinate,
    DeviceMetadata, DiagnosticPlanV1, Flow, FlowLock, FlowTrialEvidence, FlowValidationError,
    FrameworkDiagnosticsV1, InputValue, IosMetricAvailability, IosNativeMetrics, IterationMetrics,
    NormalizedResult, ReactNativeDiagnosticEvent, ReactNativeDiagnosticsSummary,
    ReactNativeFrameworkDiagnosticsV1, ResultSource, RunMode, Step, SwipeDirection, TrialMode,
    canonical_flow_hash,
};
use reactor_store::{ArtifactIssue, Job, JobEvent, JobState, Store};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::Sha1;
use thiserror::Error;
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use walkdir::WalkDir;
use zeroize::Zeroizing;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowMarkerFoundation {
    pub schema_version: u32,
    pub clock: String,
    pub source: String,
    pub uncertainty_ms: Option<f64>,
    pub iteration_boundaries: String,
    pub step_boundaries: String,
    pub reason: String,
    pub steps: Vec<reactor_protocol::ExpandedFlowStep>,
}

fn flow_marker_foundation(flow: &Flow) -> FlowMarkerFoundation {
    FlowMarkerFoundation {
        schema_version: 1,
        clock: "host_monotonic".to_owned(),
        source: "runner_host_observed".to_owned(),
        uncertainty_ms: None,
        iteration_boundaries: "unavailable".to_owned(),
        step_boundaries: "unavailable".to_owned(),
        reason: "Flashlight owns measured iteration invocation and Maestro executes each YAML as an opaque process; Reactor cannot place exact iteration or per-step boundaries without claiming device-side precision".to_owned(),
        steps: flow.expanded_steps(),
    }
}

async fn write_flow_marker_foundation(
    artifact_dir: &Path,
    flow: &Flow,
) -> Result<PathBuf, RunnerError> {
    let path = artifact_dir.join("flow-marker-foundation.json");
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&flow_marker_foundation(flow))?
        ),
    )
    .await?;
    Ok(path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPaths {
    pub maestro: Option<PathBuf>,
    pub flashlight: Option<PathBuf>,
    pub trace_processor: Option<PathBuf>,
    pub java: Option<PathBuf>,
    pub adb: Option<PathBuf>,
    pub manifest: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub id: String,
    pub label: String,
    pub available: bool,
    pub managed: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub id: String,
    pub state: String,
    pub platform: String,
    pub name: Option<String>,
    pub physical: bool,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialFailureEvidence {
    pub artifact_dir: String,
    pub error_path: String,
    pub ui_tree_path: Option<String>,
    pub screenshot_path: Option<String>,
    pub ui_tree: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidRunRequest {
    pub workspace: PathBuf,
    pub flow_lock: PathBuf,
    pub framework: String,
    pub scenario: String,
    pub device_id: String,
    pub duration_ms: u64,
    pub iteration_count: u32,
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_plan: Option<DiagnosticPlanV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leak_test: Option<AndroidLeakTestPlan>,
    /// Records a user-driven session instead of executing the locked Flow. The lock still binds
    /// the app identity and provenance, while the scenario prevents comparison with Flow runs.
    #[serde(default)]
    pub manual_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidLeakTestPlan {
    pub cycles: u32,
    pub checkpoint_every: u32,
    pub warmup_cycles: u32,
    pub stabilization_ms: u64,
    pub cooldown_ms: u64,
    pub threshold_mb_per_cycle: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosRunRequest {
    pub workspace: PathBuf,
    pub flow_lock: PathBuf,
    pub framework: String,
    pub scenario: String,
    pub device_id: String,
    pub duration_ms: u64,
}

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Compile(#[from] reactor_core::CompileError),
    #[error(transparent)]
    Store(#[from] reactor_store::StoreError),
    #[error(transparent)]
    Protocol(#[from] FlowValidationError),
    #[error("managed tool is missing: {0}")]
    MissingTool(&'static str),
    #[error("command failed: {command}\n{output}")]
    CommandFailed { command: String, output: String },
    #[error("command timed out after {seconds}s: {command}")]
    CommandTimedOut { command: String, seconds: u64 },
    #[error("Flashlight result is missing iterations[]")]
    InvalidFlashlight,
    #[error("locked Flow has no successful trial on Android target {0}")]
    MissingAndroidTrial(String),
    #[error("locked Flow has no successful trial on iOS Simulator {0}")]
    MissingIosTrial(String),
    #[error("invalid or unsupported Perfetto trace: {0}")]
    InvalidPerfetto(String),
    #[error(
        "insufficient space at {location}: {available_bytes} bytes available, {required_bytes} required"
    )]
    InsufficientSpace {
        location: String,
        available_bytes: u64,
        required_bytes: u64,
    },
    #[error("invalid or unsupported xctrace output: {0}")]
    InvalidXctrace(String),
    #[error("invalid Flow input reference: {0}")]
    InvalidInputReference(String),
    #[error("{path}: required {kind} value is unavailable")]
    MissingInputValue { path: String, kind: &'static str },
    #[error("{path}: promptRef requires an interactive value before replay")]
    InteractiveInputRequired { path: String },
    #[error("{path}: TOTP secret is not valid Base32")]
    InvalidTotpSecret { path: String },
    #[error("Flow secret store is unavailable: {0}")]
    SecretStore(String),
    #[error("invalid Android leak test plan: {0}")]
    InvalidLeakTestPlan(String),
    #[error("invalid diagnostic plan: {0}")]
    InvalidDiagnosticPlan(String),
    #[error("invalid Android package id: {0}")]
    InvalidAndroidPackageId(String),
}

const FLOW_SECRET_SERVICE: &str = "com.reactor.performance.flow-secret";
const FLOW_SECRET_INDEX_ACCOUNT: &str = "__reactor_secret_index_v1__";

/// Saves a named Flow secret in the OS credential store. The value is never returned by list or
/// diagnostic APIs and is kept separate from AI provider credentials.
///
/// # Errors
///
/// Returns an error for an invalid reference or unavailable credential store.
pub fn save_flow_secret(reference: &str, value: &str) -> Result<(), RunnerError> {
    validate_input_reference(reference)?;
    if value.is_empty() {
        return Err(RunnerError::InvalidInputReference(
            "secret value must not be empty".to_owned(),
        ));
    }
    let mut references = load_flow_secret_index()?;
    references.insert(reference.to_owned());
    save_flow_secret_index(&references)?;
    keyring::Entry::new(FLOW_SECRET_SERVICE, reference)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))?
        .set_password(value)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))
}

/// Reports only whether a named Flow secret exists; it never exposes the stored value.
///
/// # Errors
///
/// Returns an error for an invalid reference or unavailable credential store.
pub fn has_flow_secret(reference: &str) -> Result<bool, RunnerError> {
    Ok(load_flow_secret(reference)?.is_some())
}

/// Deletes a named Flow secret. Missing entries are treated as success.
///
/// # Errors
///
/// Returns an error for an invalid reference or unavailable credential store.
pub fn delete_flow_secret(reference: &str) -> Result<(), RunnerError> {
    validate_input_reference(reference)?;
    let entry = keyring::Entry::new(FLOW_SECRET_SERVICE, reference)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            let mut references = load_flow_secret_index()?;
            references.remove(reference);
            save_flow_secret_index(&references)
        }
        Err(error) => Err(RunnerError::SecretStore(error.to_string())),
    }
}

/// Deletes every Flow secret registered by Reactor without exposing its value or reference list to
/// the UI. Missing credentials are tolerated so a previously interrupted erase can be resumed.
///
/// # Errors
///
/// Returns an error when the OS credential store cannot be read or updated.
pub fn delete_all_flow_secrets() -> Result<(), RunnerError> {
    for reference in load_flow_secret_index()? {
        let entry = keyring::Entry::new(FLOW_SECRET_SERVICE, &reference)
            .map_err(|error| RunnerError::SecretStore(error.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(RunnerError::SecretStore(error.to_string())),
        }
    }
    let index = flow_secret_index_entry()?;
    match index.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(RunnerError::SecretStore(error.to_string())),
    }
}

fn flow_secret_index_entry() -> Result<keyring::Entry, RunnerError> {
    keyring::Entry::new(FLOW_SECRET_SERVICE, FLOW_SECRET_INDEX_ACCOUNT)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))
}

fn load_flow_secret_index() -> Result<BTreeSet<String>, RunnerError> {
    match flow_secret_index_entry()?.get_password() {
        Ok(value) => serde_json::from_str(&value)
            .map_err(|error| RunnerError::SecretStore(format!("invalid secret index: {error}"))),
        Err(keyring::Error::NoEntry) => Ok(BTreeSet::new()),
        Err(error) => Err(RunnerError::SecretStore(error.to_string())),
    }
}

fn save_flow_secret_index(references: &BTreeSet<String>) -> Result<(), RunnerError> {
    if references.is_empty() {
        return match flow_secret_index_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(RunnerError::SecretStore(error.to_string())),
        };
    }
    let value = serde_json::to_string(references)?;
    flow_secret_index_entry()?
        .set_password(&value)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))
}

fn validate_input_reference(reference: &str) -> Result<(), RunnerError> {
    if reference.is_empty()
        || reference.len() > 128
        || reference.trim() != reference
        || !reference
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(RunnerError::InvalidInputReference(
            "use 1-128 ASCII letters, numbers, dot, underscore, or hyphen".to_owned(),
        ));
    }
    Ok(())
}

fn load_flow_secret(reference: &str) -> Result<Option<Zeroizing<String>>, RunnerError> {
    validate_input_reference(reference)?;
    let entry = keyring::Entry::new(FLOW_SECRET_SERVICE, reference)
        .map_err(|error| RunnerError::SecretStore(error.to_string()))?;
    match entry.get_password() {
        Ok(value) => Ok(Some(Zeroizing::new(value))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(RunnerError::SecretStore(error.to_string())),
    }
}

fn resolve_input_environment(
    bindings: &[CompiledInputBinding],
    prompts: Option<&BTreeMap<String, Zeroizing<String>>>,
) -> Result<Vec<(String, Zeroizing<String>)>, RunnerError> {
    bindings
        .iter()
        .map(|binding| {
            let resolved = match &binding.value {
                InputValue::Literal(value) => Zeroizing::new(value.clone()),
                InputValue::VariableRef(reference) => {
                    validate_input_reference(&reference.variable_ref)?;
                    Zeroizing::new(std::env::var(&reference.variable_ref).map_err(|_| {
                        RunnerError::MissingInputValue {
                            path: binding.path.clone(),
                            kind: "variableRef",
                        }
                    })?)
                }
                InputValue::SecretRef(reference) => load_flow_secret(&reference.secret_ref)?
                    .ok_or_else(|| RunnerError::MissingInputValue {
                        path: binding.path.clone(),
                        kind: "secretRef",
                    })?,
                InputValue::PromptRef(reference) => {
                    let value = prompts
                        .and_then(|values| values.get(&reference.prompt_ref))
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| RunnerError::InteractiveInputRequired {
                            path: binding.path.clone(),
                        })?;
                    Zeroizing::new(value.to_string())
                }
                InputValue::TotpRef(reference) => {
                    let secret = load_flow_secret(&reference.totp_ref)?.ok_or_else(|| {
                        RunnerError::MissingInputValue {
                            path: binding.path.clone(),
                            kind: "totpRef",
                        }
                    })?;
                    Zeroizing::new(generate_totp(&secret, &binding.path)?)
                }
            };
            Ok((binding.environment_key.clone(), resolved))
        })
        .collect()
}

fn generate_totp(secret: &str, path: &str) -> Result<String, RunnerError> {
    let normalized = secret
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && *character != '-')
        .flat_map(char::to_uppercase)
        .collect::<String>();
    let bytes =
        BASE32_NOPAD
            .decode(normalized.as_bytes())
            .map_err(|_| RunnerError::InvalidTotpSecret {
                path: path.to_owned(),
            })?;
    let counter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 30;
    generate_totp_at_counter(&bytes, counter).ok_or_else(|| RunnerError::InvalidTotpSecret {
        path: path.to_owned(),
    })
}

fn generate_totp_at_counter(secret: &[u8], counter: u64) -> Option<String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let binary = (u32::from(digest[offset] & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    Some(format!("{:06}", binary % 1_000_000))
}

struct PerfettoSession {
    pid: u32,
    remote_trace: String,
}

#[derive(Debug)]
struct FrameTimelineMetrics {
    frame_count: u64,
    frame_time_mean_ms: Option<f64>,
    frame_time_p50_ms: Option<f64>,
    frame_time_p95_ms: Option<f64>,
    frame_time_p99_ms: Option<f64>,
    jank_frame_count: u64,
    over_budget_frame_pct: Option<f64>,
}

#[derive(Debug)]
struct XctraceProfileMetrics {
    duration_ms: f64,
    cpu_sample_count: u64,
    cpu_mean_pct: Option<f64>,
    xctrace_version: String,
    device_name: Option<String>,
    os_version: Option<String>,
}

#[derive(Debug, Default)]
struct AndroidTargetMetadata {
    name: Option<String>,
    os_version: Option<String>,
    app_version: Option<String>,
}

fn open_store(workspace: &Path) -> Result<Store, RunnerError> {
    Ok(Store::open(
        &workspace.join(".reactor/runtime/reactor.sqlite3"),
    )?)
}

/// Lists persisted jobs for desktop/CLI reconnection.
///
/// # Errors
///
/// Returns an error when the local runtime database cannot be read.
pub fn list_jobs(workspace: &Path, limit: u32) -> Result<Vec<Job>, RunnerError> {
    Ok(open_store(workspace)?.list_jobs(limit)?)
}

/// Reads one persisted job and its events after a stable cursor.
///
/// # Errors
///
/// Returns an error when the job is missing or the local runtime database cannot be read.
pub fn get_job(
    workspace: &Path,
    job_id: &str,
    cursor: i64,
) -> Result<(Job, Vec<JobEvent>), RunnerError> {
    let store = open_store(workspace)?;
    let job = store
        .get_job(job_id)?
        .ok_or_else(|| reactor_store::StoreError::UnknownJob(job_id.to_owned()))?;
    let events = store.events_after(job_id, cursor)?;
    Ok((job, events))
}

/// Cancels an active detached worker and its entire process group. Repeated cancellation is
/// idempotent and returns the existing terminal job.
///
/// # Errors
///
/// Returns an error when the job is missing or its state cannot be persisted.
pub fn cancel_persisted_job(workspace: &Path, job_id: &str) -> Result<Job, RunnerError> {
    let store = open_store(workspace)?;
    let current = store
        .get_job(job_id)?
        .ok_or_else(|| reactor_store::StoreError::UnknownJob(job_id.to_owned()))?;
    if current.state.is_terminal() {
        return Ok(current);
    }
    if let Some(pid) = current.worker_pid {
        terminate_worker_group(pid);
    }
    Ok(store.transition(
        job_id,
        JobState::Cancelled,
        "用户已取消任务",
        None,
        Some("cancelled by user"),
    )?)
}

/// Requests a graceful end to a user-driven Android diagnostic recording. Unlike cancellation,
/// the detached worker remains alive long enough to close collectors and persist final evidence.
///
/// # Errors
///
/// Returns an error for unknown, non-manual, or malformed jobs and for an unwritable stop marker.
pub fn request_android_manual_stop(workspace: &Path, job_id: &str) -> Result<Job, RunnerError> {
    uuid::Uuid::parse_str(job_id).map_err(|_| {
        RunnerError::InvalidDiagnosticPlan("manual recording job id is invalid".to_owned())
    })?;
    let store = open_store(workspace)?;
    let current = store
        .get_job(job_id)?
        .ok_or_else(|| reactor_store::StoreError::UnknownJob(job_id.to_owned()))?;
    if current.state.is_terminal() {
        return Ok(current);
    }
    if current
        .request
        .get("manualSession")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(RunnerError::InvalidDiagnosticPlan(
            "only a manual diagnostic recording can be stopped gracefully".to_owned(),
        ));
    }
    let directory = workspace.join("results/runs").join(job_id);
    std::fs::create_dir_all(&directory)?;
    let stop_path = directory.join("manual-stop.request");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stop_path)
    {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(b"stop\n")?;
            file.sync_all()?;
            store.append_event(
                job_id,
                current.state,
                "已请求停止手动录制；正在关闭采集器并保存证据",
                Some(&serde_json::json!({ "kind": "manual_stop_requested" })),
            )?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ok(store
        .get_job(job_id)?
        .ok_or_else(|| reactor_store::StoreError::UnknownJob(job_id.to_owned()))?)
}

/// Marks active jobs with missing/dead workers as failed while retaining their indexed artifacts.
///
/// # Errors
///
/// Returns an error when the local runtime database cannot be read.
pub fn recover_orphaned_jobs(workspace: &Path) -> Result<Vec<Job>, RunnerError> {
    let store = open_store(workspace)?;
    let mut recovered = Vec::new();
    for job in store
        .list_jobs(500)?
        .into_iter()
        .filter(|job| !job.state.is_terminal())
    {
        let orphaned = job.worker_pid.is_none_or(|pid| !worker_is_alive(pid));
        if orphaned {
            let issues = store.verify_artifacts_from(&job.id, workspace)?;
            let message = if issues.is_empty() {
                "后台 Runner 意外退出；已保留并验证现有 artifact".to_owned()
            } else {
                format!(
                    "后台 Runner 意外退出；已保留 artifact，但发现 {} 个完整性问题",
                    issues.len()
                )
            };
            recovered.push(store.fail(&job.id, &message)?);
        }
    }
    Ok(recovered)
}

/// Verifies all indexed artifacts for one job.
///
/// # Errors
///
/// Returns an error when the local runtime database cannot be read.
pub fn verify_job_artifacts(
    workspace: &Path,
    job_id: &str,
) -> Result<Vec<ArtifactIssue>, RunnerError> {
    Ok(open_store(workspace)?.verify_artifacts_from(job_id, workspace)?)
}

#[cfg(unix)]
fn worker_is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(unix))]
const fn worker_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(unix)]
fn terminate_worker_group(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn terminate_worker_group(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

fn transition_job(
    workspace: &Path,
    job_id: &str,
    next: JobState,
    message: &str,
    result_path: Option<&str>,
) -> Result<Job, RunnerError> {
    Ok(open_store(workspace)?.transition(job_id, next, message, result_path, None)?)
}

fn register_artifact(
    workspace: &Path,
    job_id: &str,
    kind: &str,
    path: &Path,
) -> Result<(), RunnerError> {
    open_store(workspace)?.register_artifact(job_id, kind, path)?;
    Ok(())
}

fn validate_android_package_id(app_id: &str) -> Result<(), RunnerError> {
    let segment = |value: &str| {
        !value.is_empty()
            && value.len() <= 63
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    if app_id.len() > 255 || app_id.split('.').count() < 2 || !app_id.split('.').all(segment) {
        return Err(RunnerError::InvalidAndroidPackageId(app_id.to_owned()));
    }
    Ok(())
}

fn validate_android_request(request: &AndroidRunRequest) -> Result<(), RunnerError> {
    if request.duration_ms == 0 || request.iteration_count == 0 {
        return Err(RunnerError::InvalidDiagnosticPlan(
            "durationMs and iterationCount must be non-zero".to_owned(),
        ));
    }
    if request.manual_session
        && (request.run_mode != RunMode::Diagnose
            || request.iteration_count != 1
            || request.leak_test.is_some())
    {
        return Err(RunnerError::InvalidDiagnosticPlan(
            "manual recording requires diagnose mode, one iteration, and no leak plan".to_owned(),
        ));
    }
    match (request.run_mode, request.diagnostic_plan.as_ref()) {
        (RunMode::Diagnose, Some(plan)) => {
            plan.validate()
                .map_err(RunnerError::InvalidDiagnosticPlan)?;
            let cumulative_duration = request
                .duration_ms
                .checked_mul(u64::from(request.iteration_count))
                .ok_or_else(|| {
                    RunnerError::InvalidDiagnosticPlan(
                        "diagnostic cumulative duration overflowed".to_owned(),
                    )
                })?;
            if cumulative_duration > plan.resource_limits.max_duration_ms {
                return Err(RunnerError::InvalidDiagnosticPlan(format!(
                    "planned cumulative duration {cumulative_duration}ms exceeds maxDurationMs {}",
                    plan.resource_limits.max_duration_ms
                )));
            }
        }
        (RunMode::Diagnose, None) => {
            return Err(RunnerError::InvalidDiagnosticPlan(
                "diagnose mode requires diagnosticPlan".to_owned(),
            ));
        }
        (RunMode::Benchmark, Some(_)) => {
            return Err(RunnerError::InvalidDiagnosticPlan(
                "benchmark mode must not include diagnosticPlan".to_owned(),
            ));
        }
        (RunMode::Benchmark, None) => {}
    }
    Ok(())
}

/// Persists an Android job before a detached worker starts it.
///
/// # Errors
///
/// Returns an error when the queue database cannot be updated.
pub fn enqueue_android(request: &AndroidRunRequest) -> Result<Job, RunnerError> {
    validate_android_request(request)?;
    let store = open_store(&request.workspace)?;
    Ok(store.create_job(&serde_json::to_value(request)?)?)
}

/// Persists an iOS Simulator job before a detached worker starts it.
///
/// # Errors
///
/// Returns an error when the queue database cannot be updated.
pub fn enqueue_ios(request: &IosRunRequest) -> Result<Job, RunnerError> {
    let store = open_store(&request.workspace)?;
    Ok(store.create_job(&serde_json::to_value(request)?)?)
}

/// Persists a product-tour job before a detached worker starts it.
///
/// # Errors
///
/// Returns an error when validation or queue persistence fails.
pub fn enqueue_demo(workspace: &Path, flow: &FlowLock) -> Result<Job, RunnerError> {
    flow.verify()?;
    let request = serde_json::json!({ "mode": "demo", "flowHash": flow.flow_hash });
    Ok(open_store(workspace)?.create_job(&request)?)
}

#[must_use]
pub fn resolve_tools(workspace: &Path) -> ToolPaths {
    let root = workspace.join(".reactor/tools");
    let maestro_override = std::env::var_os("REACTOR_MAESTRO_OVERRIDE")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    ToolPaths {
        maestro: maestro_override.or_else(|| find_executable(&root, &["maestro", "maestro.bat"])),
        flashlight: find_executable(
            &root,
            &[
                "flashlight",
                "flashlight-macos",
                "flashlight-linux",
                "flashlight-win.exe",
            ],
        ),
        trace_processor: find_executable(
            &root,
            &["trace_processor_shell", "trace_processor_shell.exe"],
        ),
        java: find_executable(&root, &["java", "java.exe"]),
        adb: find_executable(&root, &["adb", "adb.exe"]),
        manifest: [root.join("manifest-v2.json"), root.join("manifest.json")]
            .into_iter()
            .find(|path| path.is_file()),
    }
}

#[must_use]
pub fn doctor(workspace: &Path) -> DoctorReport {
    let tools = resolve_tools(workspace);
    let checks = [
        ("maestro", "自动化引擎", tools.maestro),
        ("flashlight", "Android 兼容采集器", tools.flashlight),
        (
            "trace_processor",
            "Perfetto Trace Processor",
            tools.trace_processor,
        ),
        ("java", "内置 Java 运行时", tools.java),
        ("adb", "Android 设备桥", tools.adb),
    ]
    .into_iter()
    .map(|(id, label, path)| DoctorCheck {
        id: id.to_owned(),
        label: label.to_owned(),
        available: path.is_some(),
        managed: true,
        detail: path.map(|value| value.display().to_string()),
    })
    .collect::<Vec<_>>();
    DoctorReport {
        ready: checks.iter().all(|check| check.available),
        checks,
    }
}

/// Discovers Android devices through Reactor's managed ADB.
///
/// # Errors
///
/// Returns an error when ADB is missing or fails.
pub async fn discover_android_devices(
    workspace: &Path,
) -> Result<Vec<DiscoveredDevice>, RunnerError> {
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let output = device_discovery_output(
        Command::new(&adb).args(["devices", "-l"]),
        &format!("{} devices -l", adb.display()),
    )
    .await?;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: adb.display().to_string(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let mut devices = parse_adb_devices(&String::from_utf8_lossy(&output.stdout));
    for device in devices.iter_mut().filter(|device| device.state == "device") {
        for (key, command) in [
            (
                "osVersion",
                vec!["shell", "getprop", "ro.build.version.release"],
            ),
            ("sdk", vec!["shell", "getprop", "ro.build.version.sdk"]),
            (
                "manufacturer",
                vec!["shell", "getprop", "ro.product.manufacturer"],
            ),
            (
                "refreshRate",
                vec!["shell", "settings", "get", "system", "peak_refresh_rate"],
            ),
        ] {
            if let Some(value) = adb_value(&adb, &device.id, &command).await {
                device.metadata.insert(key.to_owned(), value);
            }
        }
    }
    Ok(devices)
}

async fn adb_value(adb: &Path, device_id: &str, command: &[&str]) -> Option<String> {
    let mut args = vec!["-s", device_id];
    args.extend_from_slice(command);
    let output = tokio::time::timeout(
        Duration::from_secs(3),
        Command::new(adb).args(args).output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty() && value != "null")
}

/// Launches an Android application on the selected target and returns its current accessibility
/// hierarchy for local preview/AI Flow preparation. The hierarchy is not persisted or uploaded by
/// this function.
///
/// # Errors
///
/// Returns an error when managed ADB is missing, the application cannot be launched, or the
/// hierarchy cannot be captured.
pub async fn capture_android_ui_tree(
    workspace: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<String, RunnerError> {
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    start_android_launcher_activity(&adb, device_id, app_id).await?;
    tokio::time::sleep(Duration::from_millis(800)).await;

    capture_android_current_ui_tree(workspace, device_id).await
}

/// Returns the accessibility hierarchy for the screen currently visible on Android without
/// relaunching or otherwise changing application state.
///
/// # Errors
///
/// Returns an error when managed ADB is missing or the hierarchy cannot be captured.
pub async fn capture_android_current_ui_tree(
    workspace: &Path,
    device_id: &str,
) -> Result<String, RunnerError> {
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let adb = adb.to_string_lossy().into_owned();

    let remote_tree = format!("/sdcard/reactor-context-{}.xml", uuid::Uuid::new_v4());
    command_text(
        &adb,
        &[
            "-s",
            device_id,
            "shell",
            "uiautomator",
            "dump",
            &remote_tree,
        ],
        "capture Android UI context",
        Duration::from_secs(15),
    )
    .await?;
    let tree = command_text(
        &adb,
        &["-s", device_id, "exec-out", "cat", &remote_tree],
        "read Android UI context",
        Duration::from_secs(10),
    )
    .await;
    let _ = command_text(
        &adb,
        &["-s", device_id, "shell", "rm", &remote_tree],
        "remove Android UI context",
        Duration::from_secs(5),
    )
    .await;
    let tree = tree?;
    if tree.trim().is_empty() {
        return Err(RunnerError::CommandFailed {
            command: "read Android UI context".to_owned(),
            output: "captured hierarchy is empty".to_owned(),
        });
    }
    Ok(tree)
}

/// Captures the currently visible Android screen as an in-memory PNG for the local Flow Explorer.
/// The image is never persisted or uploaded by this function.
///
/// # Errors
///
/// Returns an error when managed ADB is unavailable, capture fails, or the image exceeds the
/// inspector safety limit.
pub async fn capture_android_screenshot(
    workspace: &Path,
    device_id: &str,
) -> Result<Vec<u8>, RunnerError> {
    const MAX_SCREENSHOT_BYTES: usize = 12 * 1024 * 1024;
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(adb)
            .args(["-s", device_id, "exec-out", "screencap", "-p"])
            .output(),
    )
    .await
    .map_err(|_| RunnerError::CommandTimedOut {
        command: "capture Android inspector screenshot".to_owned(),
        seconds: 10,
    })??;
    validate_screenshot_output(
        output,
        "capture Android inspector screenshot",
        MAX_SCREENSHOT_BYTES,
    )
}

/// Launches an application on a booted iOS Simulator and returns Maestro's compact accessibility
/// hierarchy for local preview/AI Flow preparation.
///
/// # Errors
///
/// Returns an error when the managed Maestro/Java runtime is unavailable, the application cannot
/// be launched, or the hierarchy cannot be captured.
pub async fn capture_ios_ui_tree(
    workspace: &Path,
    simulator_id: &str,
    app_id: &str,
) -> Result<String, RunnerError> {
    ensure_ios_app_running(simulator_id, app_id).await?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    capture_ios_current_ui_tree(workspace, simulator_id).await
}

/// Returns the compact hierarchy for the screen currently visible on an iOS Simulator without
/// relaunching the app.
///
/// # Errors
///
/// Returns an error when managed Maestro/Java is unavailable or hierarchy capture fails.
pub async fn capture_ios_current_ui_tree(
    workspace: &Path,
    simulator_id: &str,
) -> Result<String, RunnerError> {
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let mut command = Command::new(&maestro);
    command.args([
        "--udid",
        simulator_id,
        "hierarchy",
        "--no-ansi",
        "--compact",
    ]);
    configure_java_environment(&mut command, &java);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .map_err(|_| RunnerError::CommandTimedOut {
            command: "capture iOS UI context".to_owned(),
            seconds: 20,
        })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "capture iOS UI context".to_owned(),
            output: format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    let tree = String::from_utf8_lossy(&output.stdout).into_owned();
    if tree.trim().is_empty() {
        return Err(RunnerError::CommandFailed {
            command: "capture iOS UI context".to_owned(),
            output: "captured hierarchy is empty".to_owned(),
        });
    }
    Ok(tree)
}

/// Captures a booted iOS Simulator screen as an in-memory PNG for the local Flow Explorer.
/// The temporary file is removed immediately after it is read.
///
/// # Errors
///
/// Returns an error when `simctl` capture fails or the image exceeds the inspector safety limit.
pub async fn capture_ios_screenshot(
    workspace: &Path,
    simulator_id: &str,
) -> Result<Vec<u8>, RunnerError> {
    const MAX_SCREENSHOT_BYTES: usize = 12 * 1024 * 1024;
    let directory = workspace.join(".reactor/runtime/inspector");
    fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{}.png", uuid::Uuid::new_v4()));
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("xcrun")
            .args([
                "simctl",
                "io",
                simulator_id,
                "screenshot",
                "--type=png",
                &path.display().to_string(),
            ])
            .output(),
    )
    .await
    .map_err(|_| RunnerError::CommandTimedOut {
        command: "capture iOS inspector screenshot".to_owned(),
        seconds: 15,
    })??;
    if !output.status.success() {
        let _ = fs::remove_file(&path).await;
        return Err(RunnerError::CommandFailed {
            command: "capture iOS inspector screenshot".to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let bytes = fs::read(&path).await;
    let _ = fs::remove_file(&path).await;
    let bytes = bytes?;
    validate_png_bytes(
        bytes,
        "capture iOS inspector screenshot",
        MAX_SCREENSHOT_BYTES,
    )
}

/// Executes one reviewed Flow Explorer step against the current screen. Launch is handled by the
/// platform bridge, low-latency Android gestures use ADB, and semantic/assertion steps use an
/// ephemeral Maestro document that is removed after execution and never indexed as benchmark
/// evidence. Clearing app data remains whole-flow only because it is destructive.
///
/// # Errors
///
/// Returns an error when the step clears application data, validation fails, the managed
/// automation runtime is unavailable, or the device command fails.
#[allow(clippy::too_many_arguments)]
pub async fn execute_explorer_step(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    app_id: &str,
    step: Step,
    execution_point: Option<Coordinate>,
    viewport_size: Option<(f64, f64)>,
    prompt_values: Option<BTreeMap<String, Zeroizing<String>>>,
) -> Result<(), RunnerError> {
    validate_explorer_single_step(&step)?;
    if matches!(&step, Step::LaunchApp) {
        return relaunch_explorer_app(workspace, platform, device_id, app_id).await;
    }
    if platform == reactor_protocol::Platform::Android
        && matches!(
            &step,
            Step::Tap { .. } | Step::Swipe { .. } | Step::Pause { .. }
        )
    {
        return execute_android_explorer_step(
            workspace,
            device_id,
            &step,
            execution_point,
            viewport_size,
        )
        .await;
    }
    if platform == reactor_protocol::Platform::Android
        && let Step::InputText {
            value,
            clear_before,
            ..
        } = &step
    {
        let resolved = resolve_explorer_input_value(value, prompt_values.as_ref())?;
        if android_fast_text_supported(&resolved) {
            return execute_android_explorer_text(
                workspace,
                device_id,
                execution_point,
                &resolved,
                *clear_before,
            )
            .await;
        }
    }
    let flow = Flow {
        schema_version: 1,
        id: "flow-explorer-step".to_owned(),
        name: "Flow Explorer reviewed step".to_owned(),
        app_id: app_id.to_owned(),
        platform,
        intent: None,
        setup: vec![step],
        // The protocol requires a measured section, but single-step replay executes only setup.
        measured: vec![Step::Pause { duration_ms: 1 }],
        teardown: vec![],
    };
    let compiled = compile_maestro(&flow)?;
    let input_environment =
        resolve_input_environment(&compiled.input_bindings, prompt_values.as_ref())?;
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let directory = workspace
        .join(".reactor/runtime/explorer")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory).await?;
    let path = directory.join("step.yaml");
    fs::write(&path, compiled.setup).await?;
    let result = match platform {
        reactor_protocol::Platform::Android => {
            let adb = tools.adb.ok_or(RunnerError::MissingTool("adb"))?;
            run_maestro_with_inputs(
                &maestro,
                &java,
                &adb,
                &path,
                Some(device_id),
                &input_environment,
            )
            .await
        }
        reactor_protocol::Platform::Ios => {
            run_maestro_ios_with_inputs(&maestro, &java, &path, device_id, &input_environment).await
        }
    };
    let _ = fs::remove_dir_all(&directory).await;
    result
}

fn resolve_explorer_input_value(
    value: &InputValue,
    prompts: Option<&BTreeMap<String, Zeroizing<String>>>,
) -> Result<Zeroizing<String>, RunnerError> {
    match value {
        InputValue::Literal(value) => Ok(Zeroizing::new(value.clone())),
        InputValue::VariableRef(reference) => {
            validate_input_reference(&reference.variable_ref)?;
            std::env::var(&reference.variable_ref)
                .map(Zeroizing::new)
                .map_err(|_| RunnerError::MissingInputValue {
                    path: "setup[0]".to_owned(),
                    kind: "variableRef",
                })
        }
        InputValue::SecretRef(reference) => {
            load_flow_secret(&reference.secret_ref)?.ok_or_else(|| RunnerError::MissingInputValue {
                path: "setup[0]".to_owned(),
                kind: "secretRef",
            })
        }
        InputValue::PromptRef(reference) => prompts
            .and_then(|values| values.get(&reference.prompt_ref))
            .filter(|value| !value.is_empty())
            .map(|value| Zeroizing::new(value.to_string()))
            .ok_or_else(|| RunnerError::InteractiveInputRequired {
                path: "setup[0]".to_owned(),
            }),
        InputValue::TotpRef(reference) => {
            let secret = load_flow_secret(&reference.totp_ref)?.ok_or_else(|| {
                RunnerError::MissingInputValue {
                    path: "setup[0]".to_owned(),
                    kind: "totpRef",
                }
            })?;
            Ok(Zeroizing::new(generate_totp(&secret, "setup[0]")?))
        }
    }
}

fn android_fast_text_supported(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || " ._@+-".contains(character))
}

async fn execute_android_explorer_text(
    workspace: &Path,
    device_id: &str,
    execution_point: Option<Coordinate>,
    value: &str,
    clear_before: bool,
) -> Result<(), RunnerError> {
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let adb = adb.to_string_lossy();
    let tap_args = android_explorer_input_args(
        device_id,
        &Step::Tap {
            target: reactor_protocol::Selector::default(),
        },
        execution_point,
        None,
    )?;
    let tap_args = tap_args.iter().map(String::as_str).collect::<Vec<_>>();
    command_text(
        &adb,
        &tap_args,
        "Android Flow Explorer focus input",
        Duration::from_secs(5),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(60)).await;
    if clear_before {
        command_text(
            &adb,
            &[
                "-s",
                device_id,
                "shell",
                "input",
                "keycombination",
                "KEYCODE_CTRL_LEFT",
                "KEYCODE_A",
            ],
            "Android Flow Explorer select input text",
            Duration::from_secs(5),
        )
        .await?;
        command_text(
            &adb,
            &["-s", device_id, "shell", "input", "keyevent", "KEYCODE_DEL"],
            "Android Flow Explorer clear input text",
            Duration::from_secs(5),
        )
        .await?;
    }
    let encoded = value.replace(' ', "%s");
    command_text(
        &adb,
        &["-s", device_id, "shell", "input", "text", &encoded],
        "Android Flow Explorer enter reviewed text",
        Duration::from_secs(5),
    )
    .await?;
    Ok(())
}

async fn relaunch_explorer_app(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    app_id: &str,
) -> Result<(), RunnerError> {
    match platform {
        reactor_protocol::Platform::Android => {
            let adb = resolve_tools(workspace)
                .adb
                .ok_or(RunnerError::MissingTool("adb"))?;
            android_shell_text(
                &adb,
                device_id,
                &["am", "force-stop", app_id],
                "stop Android app for Flow Explorer relaunch",
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(250)).await;
            start_android_launcher_activity(&adb, device_id, app_id)
                .await
                .map(|_| ())
        }
        reactor_protocol::Platform::Ios => {
            let _ = command_text(
                "xcrun",
                &["simctl", "terminate", device_id, app_id],
                "terminate iOS app for Flow Explorer relaunch",
                Duration::from_secs(10),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(250)).await;
            ensure_ios_app_running(device_id, app_id).await
        }
    }
}

/// Clears application state before a user explicitly starts a new trusted recording. This is
/// deliberately separate from single-step replay because clearing data is destructive and must
/// never happen from an ordinary recorded-step click.
///
/// # Errors
///
/// Returns an error when the target cannot be cleared with the platform's deterministic Flow
/// mechanism.
pub async fn reset_explorer_app_state(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    app_id: &str,
) -> Result<(), RunnerError> {
    match platform {
        reactor_protocol::Platform::Android => {
            validate_android_package_id(app_id)?;
            let adb = resolve_tools(workspace)
                .adb
                .ok_or(RunnerError::MissingTool("adb"))?;
            let output = android_shell_text(
                &adb,
                device_id,
                &["pm", "clear", app_id],
                "clear Android app state for trusted recording",
            )
            .await?;
            if output.lines().any(|line| line.trim() == "Success") {
                Ok(())
            } else {
                Err(RunnerError::CommandFailed {
                    command: "clear Android app state for trusted recording".to_owned(),
                    output,
                })
            }
        }
        reactor_protocol::Platform::Ios => {
            let reset_flow = Flow {
                schema_version: 1,
                id: "reactor-trusted-recording-reset".to_owned(),
                name: "Trusted recording reset".to_owned(),
                app_id: app_id.to_owned(),
                platform,
                intent: None,
                setup: vec![Step::ResetAppState],
                measured: vec![Step::Pause { duration_ms: 1 }],
                teardown: vec![],
            };
            replay_explorer_flow(workspace, platform, device_id, &reset_flow, None).await
        }
    }
}

fn validate_explorer_single_step(step: &Step) -> Result<(), RunnerError> {
    if matches!(step, Step::ResetAppState) {
        return Err(RunnerError::CommandFailed {
            command: "Flow Explorer single-step replay".to_owned(),
            output:
                "reset_app_state clears application data and requires explicit whole-flow replay"
                    .to_owned(),
        });
    }
    Ok(())
}

/// Replays a complete edited Flow outside the measurement window with one Maestro process. The
/// three Flow sections remain separate YAML documents but are passed to the same invocation in
/// setup → measured → teardown order.
///
/// # Errors
///
/// Returns an error when the Flow is invalid, an input reference is unresolved, or Maestro fails.
pub async fn replay_explorer_flow(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    flow: &Flow,
    prompt_values: Option<BTreeMap<String, Zeroizing<String>>>,
) -> Result<(), RunnerError> {
    replay_explorer_flow_with_progress(workspace, platform, device_id, flow, prompt_values, None)
        .await
}

/// Replays an Explorer Flow and reports the zero-based command currently printed by Maestro.
/// Progress is observational only and does not alter the single-process replay semantics.
///
/// # Errors
///
/// Returns an error when validation, input resolution, managed tool lookup, or Maestro execution
/// fails.
pub async fn replay_explorer_flow_with_progress(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    flow: &Flow,
    prompt_values: Option<BTreeMap<String, Zeroizing<String>>>,
    progress: Option<tokio::sync::mpsc::UnboundedSender<usize>>,
) -> Result<(), RunnerError> {
    let compiled = compile_maestro(flow)?;
    let (maestro_progress, progress_forwarder) = if let Some(progress) = progress {
        let top_level_steps = maestro_progress_top_level_steps(flow);
        let (raw_progress, mut raw_progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let forwarder = tokio::spawn(async move {
            while let Some(completed_command_index) = raw_progress_rx.recv().await {
                let Some(&completed_top_level_step) = top_level_steps.get(completed_command_index)
                else {
                    continue;
                };
                if top_level_steps.get(completed_command_index + 1).copied()
                    != Some(completed_top_level_step)
                {
                    let _ = progress.send(completed_top_level_step);
                }
            }
        });
        (Some(raw_progress), Some(forwarder))
    } else {
        (None, None)
    };
    let input_environment =
        resolve_input_environment(&compiled.input_bindings, prompt_values.as_ref())?;
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let directory = workspace
        .join(".reactor/runtime")
        .join(format!("explorer-replay-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory).await?;
    let sections = [
        ("setup.yaml", &flow.setup, compiled.setup),
        ("measured.yaml", &flow.measured, compiled.measured),
        ("teardown.yaml", &flow.teardown, compiled.teardown),
    ];
    let mut paths = Vec::new();
    for (name, steps, yaml) in sections {
        if !steps.is_empty() {
            let path = directory.join(name);
            fs::write(&path, yaml).await?;
            paths.push(path);
        }
    }
    let result = match platform {
        reactor_protocol::Platform::Android => {
            let adb = tools.adb.ok_or(RunnerError::MissingTool("adb"))?;
            run_maestro_paths_with_inputs_progress(
                &maestro,
                &java,
                &adb,
                &paths,
                Some(device_id),
                &input_environment,
                maestro_progress,
            )
            .await
        }
        reactor_protocol::Platform::Ios => {
            run_maestro_ios_paths_with_inputs_progress(
                &maestro,
                &java,
                &paths,
                device_id,
                &input_environment,
                maestro_progress,
            )
            .await
        }
    };
    if let Some(progress_forwarder) = progress_forwarder {
        let _ = progress_forwarder.await;
    }
    let _ = fs::remove_dir_all(&directory).await;
    result
}

fn maestro_progress_top_level_steps(flow: &Flow) -> Vec<usize> {
    flow.setup
        .iter()
        .chain(&flow.measured)
        .chain(&flow.teardown)
        .enumerate()
        .flat_map(|(top_level_index, step)| {
            std::iter::repeat_n(top_level_index, maestro_observable_completion_count(step))
        })
        .collect()
}

fn maestro_observable_completion_count(step: &Step) -> usize {
    match step {
        Step::InputText { clear_before, .. } => usize::from(*clear_before) + 2,
        Step::Repeat { steps, .. } => {
            1 + steps
                .iter()
                .map(maestro_observable_completion_count)
                .sum::<usize>()
        }
        _ => 1,
    }
}

async fn execute_android_explorer_step(
    workspace: &Path,
    device_id: &str,
    step: &Step,
    execution_point: Option<Coordinate>,
    viewport_size: Option<(f64, f64)>,
) -> Result<(), RunnerError> {
    if let Step::Pause { duration_ms } = step {
        tokio::time::sleep(Duration::from_millis(*duration_ms)).await;
        return Ok(());
    }
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let args = android_explorer_input_args(device_id, step, execution_point, viewport_size)?;
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    command_text(
        &adb.to_string_lossy(),
        &borrowed,
        "Android Flow Explorer low-latency interaction",
        Duration::from_secs(5),
    )
    .await?;
    Ok(())
}

fn android_explorer_input_args(
    device_id: &str,
    step: &Step,
    execution_point: Option<Coordinate>,
    viewport_size: Option<(f64, f64)>,
) -> Result<Vec<String>, RunnerError> {
    let valid_dimension = |value: f64| value.is_finite() && (1.0..=20_000.0).contains(&value);
    let rounded = |value: f64| format!("{:.0}", value.round());
    let mut args = vec![
        "-s".to_owned(),
        device_id.to_owned(),
        "shell".to_owned(),
        "input".to_owned(),
    ];
    match step {
        Step::Tap { .. } => {
            let point = execution_point.filter(|point| {
                point.x.is_finite()
                    && point.y.is_finite()
                    && point.x >= 0.0
                    && point.y >= 0.0
                    && point.x <= 20_000.0
                    && point.y <= 20_000.0
            });
            let Some(point) = point else {
                return Err(RunnerError::CommandFailed {
                    command: "Android Flow Explorer tap".to_owned(),
                    output: "a reviewed on-screen execution point is required".to_owned(),
                });
            };
            args.extend(["tap".to_owned(), rounded(point.x), rounded(point.y)]);
        }
        Step::Swipe {
            direction,
            duration_ms,
        } => {
            let Some((width, height)) = viewport_size
                .filter(|(width, height)| valid_dimension(*width) && valid_dimension(*height))
            else {
                return Err(RunnerError::CommandFailed {
                    command: "Android Flow Explorer swipe".to_owned(),
                    output: "a valid captured viewport is required".to_owned(),
                });
            };
            let (start_x, start_y, end_x, end_y) = match direction {
                SwipeDirection::Up => (width * 0.5, height * 0.75, width * 0.5, height * 0.25),
                SwipeDirection::Down => (width * 0.5, height * 0.25, width * 0.5, height * 0.75),
                SwipeDirection::Left => (width * 0.75, height * 0.5, width * 0.25, height * 0.5),
                SwipeDirection::Right => (width * 0.25, height * 0.5, width * 0.75, height * 0.5),
            };
            args.extend([
                "swipe".to_owned(),
                rounded(start_x),
                rounded(start_y),
                rounded(end_x),
                rounded(end_y),
                duration_ms.to_string(),
            ]);
        }
        _ => {
            return Err(RunnerError::CommandFailed {
                command: "Android Flow Explorer interaction".to_owned(),
                output: "only tap, swipe, and pause use the low-latency input driver".to_owned(),
            });
        }
    }
    Ok(args)
}

fn validate_screenshot_output(
    output: std::process::Output,
    command: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, RunnerError> {
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: command.to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    validate_png_bytes(output.stdout, command, max_bytes)
}

fn validate_png_bytes(
    bytes: Vec<u8>,
    command: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, RunnerError> {
    if bytes.len() > max_bytes {
        return Err(RunnerError::CommandFailed {
            command: command.to_owned(),
            output: format!("screenshot exceeds {max_bytes} byte safety limit"),
        });
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(RunnerError::CommandFailed {
            command: command.to_owned(),
            output: "capture did not return a PNG image".to_owned(),
        });
    }
    Ok(bytes)
}

/// Captures local-only Android screenshot/UI-tree evidence after a failed dry run. The returned
/// paths are not uploaded by this function.
///
/// # Errors
///
/// Returns an error when the managed ADB is unavailable or the local evidence directory cannot be
/// created. Individual screenshot/UI-tree failures are represented as missing optional paths.
pub async fn capture_android_trial_failure(
    workspace: &Path,
    device_id: &str,
    error: &str,
) -> Result<TrialFailureEvidence, RunnerError> {
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let artifact_dir = workspace
        .join("results/trials")
        .join(uuid::Uuid::new_v4().to_string())
        .join("failure");
    fs::create_dir_all(&artifact_dir).await?;
    let error_path = artifact_dir.join("error.txt");
    fs::write(&error_path, error).await?;

    let screenshot_path = artifact_dir.join("screenshot.png");
    let screenshot = Command::new(&adb)
        .args(["-s", device_id, "exec-out", "screencap", "-p"])
        .output()
        .await?;
    let screenshot_path = if screenshot.status.success() && !screenshot.stdout.is_empty() {
        fs::write(&screenshot_path, screenshot.stdout).await?;
        Some(screenshot_path.display().to_string())
    } else {
        None
    };

    let remote_tree = format!("/sdcard/reactor-{}.xml", uuid::Uuid::new_v4());
    let dumped = Command::new(&adb)
        .args([
            "-s",
            device_id,
            "shell",
            "uiautomator",
            "dump",
            &remote_tree,
        ])
        .output()
        .await?;
    let tree = if dumped.status.success() {
        let output = Command::new(&adb)
            .args(["-s", device_id, "exec-out", "cat", &remote_tree])
            .output()
            .await?;
        let _ = Command::new(&adb)
            .args(["-s", device_id, "shell", "rm", &remote_tree])
            .output()
            .await;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
            .filter(|value| !value.trim().is_empty())
    } else {
        None
    };
    let ui_tree_path = if let Some(tree) = &tree {
        let path = artifact_dir.join("ui-tree.xml");
        fs::write(&path, tree).await?;
        Some(path.display().to_string())
    } else {
        None
    };
    Ok(TrialFailureEvidence {
        artifact_dir: artifact_dir.display().to_string(),
        error_path: error_path.display().to_string(),
        ui_tree_path,
        screenshot_path,
        ui_tree: tree,
    })
}

/// Captures local-only iOS Simulator screenshot and Maestro hierarchy evidence after a failed
/// dry run. The returned paths are not uploaded by this function.
///
/// # Errors
///
/// Returns an error only when the local evidence directory or error file cannot be created.
/// Screenshot and hierarchy failures are represented as missing optional paths.
pub async fn capture_ios_trial_failure(
    workspace: &Path,
    simulator_id: &str,
    error: &str,
) -> Result<TrialFailureEvidence, RunnerError> {
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let artifact_dir = workspace
        .join("results/trials")
        .join(uuid::Uuid::new_v4().to_string())
        .join("failure");
    fs::create_dir_all(&artifact_dir).await?;
    let error_path = artifact_dir.join("error.txt");
    fs::write(&error_path, error).await?;

    let screenshot_path = artifact_dir.join("screenshot.png");
    let screenshot = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new("xcrun")
            .args([
                "simctl",
                "io",
                simulator_id,
                "screenshot",
                &screenshot_path.display().to_string(),
            ])
            .output(),
    )
    .await
    .ok()
    .and_then(Result::ok);
    let screenshot_path = screenshot
        .filter(|output| output.status.success() && screenshot_path.is_file())
        .map(|_| screenshot_path.display().to_string());

    let mut hierarchy_command = Command::new(&maestro);
    hierarchy_command.args([
        "--udid",
        simulator_id,
        "hierarchy",
        "--no-ansi",
        "--compact",
    ]);
    configure_java_environment(&mut hierarchy_command, &java);
    let hierarchy = tokio::time::timeout(Duration::from_secs(20), hierarchy_command.output())
        .await
        .ok()
        .and_then(Result::ok);
    let tree = hierarchy
        .filter(|output| output.status.success() && !output.stdout.is_empty())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let ui_tree_path = if let Some(tree) = &tree {
        let path = artifact_dir.join("ui-hierarchy.csv");
        fs::write(&path, tree).await?;
        Some(path.display().to_string())
    } else {
        None
    };
    Ok(TrialFailureEvidence {
        artifact_dir: artifact_dir.display().to_string(),
        error_path: error_path.display().to_string(),
        ui_tree_path,
        screenshot_path,
        ui_tree: tree,
    })
}

/// Discovers currently booted iOS Simulators. Shutdown simulators are intentionally hidden so a
/// measurement never boots an arbitrary target without the user choosing it in Xcode/Simulator.
///
/// # Errors
///
/// Returns an error when `simctl` is unavailable or returns invalid data.
pub async fn discover_ios_simulators() -> Result<Vec<DiscoveredDevice>, RunnerError> {
    if !cfg!(target_os = "macos") {
        return Ok(vec![]);
    }
    let output = device_discovery_output(
        Command::new("xcrun").args(["simctl", "list", "devices", "available", "--json"]),
        "xcrun simctl list devices",
    )
    .await?;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "xcrun simctl list devices".to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    parse_ios_simulators(&serde_json::from_slice(&output.stdout)?)
}

async fn device_discovery_output(
    command: &mut Command,
    description: &str,
) -> Result<std::process::Output, RunnerError> {
    const DEVICE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
    tokio::time::timeout(DEVICE_DISCOVERY_TIMEOUT, command.output())
        .await
        .map_err(|_| RunnerError::CommandTimedOut {
            command: description.to_owned(),
            seconds: DEVICE_DISCOVERY_TIMEOUT.as_secs(),
        })?
        .map_err(RunnerError::from)
}

/// Parses `simctl list --json` output into Reactor's shared target representation.
///
/// # Errors
///
/// Returns an error if the payload does not contain the expected devices object.
pub fn parse_ios_simulators(value: &Value) -> Result<Vec<DiscoveredDevice>, RunnerError> {
    let runtimes = value
        .get("devices")
        .and_then(Value::as_object)
        .ok_or_else(|| RunnerError::CommandFailed {
            command: "parse simctl devices".to_owned(),
            output: "missing devices object".to_owned(),
        })?;
    let mut devices = runtimes
        .iter()
        .flat_map(|(runtime, devices)| {
            devices
                .as_array()
                .into_iter()
                .flatten()
                .filter(|device| device.get("state").and_then(Value::as_str) == Some("Booted"))
                .filter_map(move |device| {
                    let id = device.get("udid")?.as_str()?.to_owned();
                    let name = device.get("name")?.as_str()?.to_owned();
                    let mut metadata = BTreeMap::new();
                    metadata.insert("runtime".to_owned(), runtime.clone());
                    if let Some(os_version) = ios_runtime_version(runtime) {
                        metadata.insert("osVersion".to_owned(), os_version);
                    }
                    if let Some(kind) = device.get("deviceTypeIdentifier").and_then(Value::as_str) {
                        metadata.insert("deviceType".to_owned(), kind.to_owned());
                    }
                    Some(DiscoveredDevice {
                        id,
                        state: "device".to_owned(),
                        platform: "ios".to_owned(),
                        name: Some(name),
                        physical: false,
                        metadata,
                    })
                })
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(devices)
}

fn ios_runtime_version(runtime: &str) -> Option<String> {
    let version = runtime
        .rsplit_once("iOS-")
        .map_or(runtime, |(_, version)| version)
        .trim();
    (!version.is_empty()).then(|| version.replace('-', "."))
}

#[must_use]
pub fn parse_adb_devices(output: &str) -> Vec<DiscoveredDevice> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 2 {
                return None;
            }
            let metadata = fields[2..]
                .iter()
                .filter_map(|field| field.split_once(':'))
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect::<BTreeMap<_, _>>();
            let physical = is_physical_android_target(fields[0], &metadata);
            Some(DiscoveredDevice {
                id: fields[0].to_owned(),
                state: fields[1].to_owned(),
                platform: "android".to_owned(),
                name: metadata
                    .get("model")
                    .or_else(|| metadata.get("device"))
                    .cloned(),
                physical,
                metadata,
            })
        })
        .collect()
}

fn perfetto_config(app_id: &str, duration_ms: u64) -> String {
    let app_id = app_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"buffers {{
  size_kb: 32768
  fill_policy: RING_BUFFER
}}
data_sources {{
  config {{
    name: "linux.ftrace"
    target_buffer: 0
    ftrace_config {{
      ftrace_events: "sched/sched_switch"
      ftrace_events: "sched/sched_waking"
      ftrace_events: "power/cpu_frequency"
      ftrace_events: "power/cpu_idle"
      atrace_categories: "am"
      atrace_categories: "gfx"
      atrace_categories: "view"
      atrace_categories: "wm"
      atrace_apps: "{app_id}"
      symbolize_ksyms: false
      compact_sched {{ enabled: true }}
    }}
  }}
}}
data_sources {{
  config {{
    name: "android.surfaceflinger.frametimeline"
    target_buffer: 0
  }}
}}
data_sources {{
  config {{
    name: "linux.process_stats"
    target_buffer: 0
    process_stats_config {{
      scan_all_processes_on_start: true
      proc_stats_poll_ms: 250
    }}
  }}
}}
duration_ms: {duration_ms}
flush_period_ms: 2000
incremental_state_config {{ clear_period_ms: 5000 }}
"#
    )
}

fn heapprofd_config(app_id: &str, duration_ms: u64) -> String {
    let app_id = app_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"buffers {{ size_kb: 65536 fill_policy: RING_BUFFER }}
data_sources {{
  config {{
    name: "android.heapprofd"
    target_buffer: 0
    heapprofd_config {{
      sampling_interval_bytes: 4096
      process_cmdline: "{app_id}"
      continuous_dump_config {{ dump_phase_ms: 1000 dump_interval_ms: 3000 }}
    }}
  }}
}}
duration_ms: {duration_ms}
flush_period_ms: 2000
"#
    )
}

async fn ensure_android_trace_space(
    adb: &Path,
    device_id: &str,
    artifact_dir: &Path,
) -> Result<(), RunnerError> {
    const LOCAL_REQUIRED: u64 = 128 * 1024 * 1024;
    const DEVICE_REQUIRED: u64 = 64 * 1024 * 1024;
    let local_available = fs2::available_space(artifact_dir)?;
    require_available_space(
        &artifact_dir.display().to_string(),
        local_available,
        LOCAL_REQUIRED,
    )?;
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(adb)
            .args([
                "-s",
                device_id,
                "shell",
                "df",
                "-Pk",
                "/data/misc/perfetto-traces",
            ])
            .output(),
    )
    .await
    .map_err(|_| RunnerError::CommandFailed {
        command: "adb df".to_owned(),
        output: "timed out after 5 seconds".to_owned(),
    })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "adb df".to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let available =
        parse_df_available_bytes(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
            RunnerError::CommandFailed {
                command: "adb df".to_owned(),
                output: "unable to parse available device space".to_owned(),
            }
        })?;
    require_available_space(
        &format!("{device_id}:/data/misc/perfetto-traces"),
        available,
        DEVICE_REQUIRED,
    )?;
    Ok(())
}

fn require_available_space(
    location: &str,
    available_bytes: u64,
    required_bytes: u64,
) -> Result<(), RunnerError> {
    if available_bytes < required_bytes {
        return Err(RunnerError::InsufficientSpace {
            location: location.to_owned(),
            available_bytes,
            required_bytes,
        });
    }
    Ok(())
}

fn parse_df_available_bytes(output: &str) -> Option<u64> {
    output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.starts_with("Filesystem"))
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kilobytes| kilobytes * 1024)
}

async fn start_perfetto(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    job_id: &str,
    duration_ms: u64,
) -> Result<PerfettoSession, RunnerError> {
    start_perfetto_with_config(
        adb,
        device_id,
        &format!("reactor-{job_id}"),
        &perfetto_config(app_id, duration_ms),
    )
    .await
}

async fn start_perfetto_with_config(
    adb: &Path,
    device_id: &str,
    trace_name: &str,
    config: &str,
) -> Result<PerfettoSession, RunnerError> {
    let remote_trace = format!("/data/misc/perfetto-traces/{trace_name}.pftrace");
    let mut cleanup = Command::new(adb);
    cleanup.args(["-s", device_id, "shell", "rm", "-f", &remote_trace]);
    let _ = run_command_with_timeout(cleanup, "perfetto-cleanup", Duration::from_secs(5)).await;

    let mut command = Command::new(adb);
    command
        .args([
            "-s",
            device_id,
            "shell",
            "perfetto",
            "--background-wait",
            "--txt",
            "-c",
            "-",
            "-o",
            &remote_trace,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    let mut child = command.spawn()?;
    let child_id = child.id();
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| RunnerError::CommandFailed {
            command: "perfetto-start".to_owned(),
            output: "failed to open Perfetto config stream".to_owned(),
        })?;
    stdin.write_all(config.as_bytes()).await?;
    drop(stdin);
    let output = if let Ok(output) =
        tokio::time::timeout(Duration::from_secs(35), child.wait_with_output()).await
    {
        output?
    } else {
        if let Some(child_id) = child_id {
            terminate_worker_group(child_id);
        }
        return Err(RunnerError::CommandFailed {
            command: "perfetto-start".to_owned(),
            output: "timed out waiting for data sources".to_owned(),
        });
    };
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "perfetto-start".to_owned(),
            output: format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    let pid = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|error| RunnerError::InvalidPerfetto(format!("invalid session PID: {error}")))?;
    Ok(PerfettoSession { pid, remote_trace })
}

async fn stop_perfetto(
    adb: &Path,
    device_id: &str,
    session: &PerfettoSession,
    local_trace: &Path,
) -> Result<(), RunnerError> {
    let mut stop = Command::new(adb);
    stop.args([
        "-s",
        device_id,
        "shell",
        "kill",
        "-TERM",
        &session.pid.to_string(),
    ]);
    let _ = run_command_with_timeout(stop, "perfetto-stop", Duration::from_secs(10)).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut pull = Command::new(adb);
    pull.args([
        "-s",
        device_id,
        "pull",
        &session.remote_trace,
        &local_trace.display().to_string(),
    ]);
    let pull_result =
        run_command_with_timeout(pull, "perfetto-pull", Duration::from_secs(30)).await;
    let mut cleanup = Command::new(adb);
    cleanup.args(["-s", device_id, "shell", "rm", "-f", &session.remote_trace]);
    let _ = run_command_with_timeout(cleanup, "perfetto-cleanup", Duration::from_secs(5)).await;
    pull_result?;
    if !local_trace.is_file() || std::fs::metadata(local_trace)?.len() == 0 {
        return Err(RunnerError::InvalidPerfetto(
            "trace file is empty after collection".to_owned(),
        ));
    }
    Ok(())
}

async fn parse_perfetto_frames(
    trace_processor: &Path,
    trace_path: &Path,
    app_id: &str,
    refresh_rate: f64,
) -> Result<FrameTimelineMetrics, RunnerError> {
    let app_id = app_id.replace('\'', "''");
    let frame_budget_ms = 1_000.0 / refresh_rate.max(1.0);
    let query = format!(
        r"select
          count(*) as frame_count,
          round(avg(dur)/1e6,6) as frame_time_mean_ms,
          round(percentile(dur/1e6,50),6) as frame_time_p50_ms,
          round(percentile(dur/1e6,95),6) as frame_time_p95_ms,
          round(percentile(dur/1e6,99),6) as frame_time_p99_ms,
          sum(case
            when jank_type not in ('None','Unknown Jank','Prediction Error')
              and jank_type is not null then 1 else 0
          end) as jank_frame_count,
          round(
            100.0 * sum(case when dur/1e6>{frame_budget_ms} then 1 else 0 end)
              / nullif(count(*),0),
            6
          ) as over_budget_frame_pct
        from actual_frame_timeline_slice
        where instr(layer_name,'{app_id}/')>0
          and instr(layer_name,'animation-leash')=0
          and instr(layer_name,'Splash Screen')=0
          and dur>0"
    );
    let mut command = Command::new(trace_processor);
    command.args(["query", &trace_path.display().to_string(), &query]);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .map_err(|_| RunnerError::CommandFailed {
            command: "trace-processor".to_owned(),
            output: "timed out after 60 seconds".to_owned(),
        })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "trace-processor".to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    parse_frame_metrics_csv(&String::from_utf8_lossy(&output.stdout))
}

async fn analyze_heapprofd_trace(
    trace_processor: &Path,
    trace_path: &Path,
) -> Result<(i64, i64), RunnerError> {
    let query = "select coalesce(sum(size),0) as retained_bytes, \
                 coalesce(sum(count),0) as retained_allocation_count \
                 from heap_profile_allocation";
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new(trace_processor)
            .args(["query", &trace_path.display().to_string(), query])
            .output(),
    )
    .await
    .map_err(|_| RunnerError::CommandFailed {
        command: "trace-processor-heapprofd".to_owned(),
        output: "timed out after 60 seconds".to_owned(),
    })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: "trace-processor-heapprofd".to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    parse_heapprofd_csv(&String::from_utf8_lossy(&output.stdout))
}

fn parse_heapprofd_csv(output: &str) -> Result<(i64, i64), RunnerError> {
    let row = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.starts_with("retained_bytes"))
        .ok_or_else(|| {
            RunnerError::InvalidPerfetto("heapprofd query returned no row".to_owned())
        })?;
    let values = row
        .split(',')
        .map(|value| value.trim().trim_matches('"'))
        .collect::<Vec<_>>();
    if values.len() != 2 {
        return Err(RunnerError::InvalidPerfetto(
            "heapprofd query returned an invalid row".to_owned(),
        ));
    }
    let retained_bytes = values[0]
        .parse::<i64>()
        .map_err(|error| RunnerError::InvalidPerfetto(error.to_string()))?;
    let retained_count = values[1]
        .parse::<i64>()
        .map_err(|error| RunnerError::InvalidPerfetto(error.to_string()))?;
    Ok((retained_bytes, retained_count))
}

fn parse_frame_metrics_csv(output: &str) -> Result<FrameTimelineMetrics, RunnerError> {
    let row = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .nth(1)
        .ok_or_else(|| RunnerError::InvalidPerfetto("missing metric row".to_owned()))?;
    let values = row
        .split(',')
        .map(|value| value.trim().trim_matches('"'))
        .collect::<Vec<_>>();
    if values.len() != 7 {
        return Err(RunnerError::InvalidPerfetto(format!(
            "expected 7 frame metric columns, received {}",
            values.len()
        )));
    }
    let parse_u64 = |value: &str| {
        value.parse::<u64>().map_err(|error| {
            RunnerError::InvalidPerfetto(format!("invalid integer metric {value}: {error}"))
        })
    };
    let parse_optional = |value: &str| {
        if value == "[NULL]" || value.is_empty() {
            Ok(None)
        } else {
            value.parse::<f64>().map(Some).map_err(|error| {
                RunnerError::InvalidPerfetto(format!("invalid decimal metric {value}: {error}"))
            })
        }
    };
    Ok(FrameTimelineMetrics {
        frame_count: parse_u64(values[0])?,
        frame_time_mean_ms: parse_optional(values[1])?,
        frame_time_p50_ms: parse_optional(values[2])?,
        frame_time_p95_ms: parse_optional(values[3])?,
        frame_time_p99_ms: parse_optional(values[4])?,
        jank_frame_count: parse_u64(values[5])?,
        over_budget_frame_pct: parse_optional(values[6])?,
    })
}

async fn android_shell_text(
    adb: &Path,
    device_id: &str,
    args: &[&str],
    label: &str,
) -> Result<String, RunnerError> {
    let mut command_args = vec!["-s", device_id, "shell"];
    command_args.extend_from_slice(args);
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(adb).args(command_args).output(),
    )
    .await
    .map_err(|_| RunnerError::CommandFailed {
        command: label.to_owned(),
        output: "timed out after 10 seconds".to_owned(),
    })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: label.to_owned(),
            output: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

const RN_CONTROLLER_ACK_FILE: &str = "rn-controller-ack.json";
const RN_HERMES_CPU_FILE: &str = "rn-hermes-cpu.trace.json";
const RN_REMOTE_DIAGNOSTIC_FILES: &[&str] = &[
    RN_CONTROLLER_ACK_FILE,
    RN_HERMES_CPU_FILE,
    "rn-diagnostics.ndjson",
    "rn-react-devtools-profile.json",
    "rn-hermes-heap-stats.ndjson",
    "rn-hermes.heapsnapshot",
    "rn-java.hprof",
];

async fn delete_android_diagnostic_files(adb: &Path, device_id: &str, app_id: &str) {
    if validate_android_package_id(app_id).is_err() {
        return;
    }
    let root = format!("/sdcard/Android/data/{app_id}/files/reactor");
    for name in RN_REMOTE_DIAGNOSTIC_FILES {
        let remote = format!("{root}/{name}");
        let _ = android_shell_text(
            adb,
            device_id,
            &["rm", "-f", &remote],
            "rn-diagnostics-cleanup",
        )
        .await;
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AndroidDiagnosticControllerAck {
    token: String,
    command: String,
    status: String,
    elapsed_realtime_nanos: u64,
    #[serde(default)]
    sdk_version: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    diagnostic_build: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug)]
struct AndroidHermesCpuCapture {
    token: String,
    start_elapsed_realtime_nanos: u64,
    sdk_version: Option<String>,
    capabilities: Vec<String>,
    diagnostic_build: bool,
}

fn result_relative_artifact_path(artifact_dir: &Path, path: &Path) -> Result<String, RunnerError> {
    let relative = path
        .strip_prefix(artifact_dir)
        .map_err(|_| RunnerError::CommandFailed {
            command: "artifact-path".to_owned(),
            output: format!("{} is outside result directory", path.display()),
        })?;
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err(RunnerError::CommandFailed {
            command: "artifact-path".to_owned(),
            output: format!(
                "{} is not a normalized result-relative path",
                path.display()
            ),
        });
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn enforce_diagnostic_artifacts(
    artifact_dir: &Path,
    paths: impl IntoIterator<Item = PathBuf>,
    max_bytes: u64,
) -> Result<(), RunnerError> {
    let mut unique = BTreeSet::new();
    let mut total = 0_u64;
    for path in paths {
        result_relative_artifact_path(artifact_dir, &path)?;
        if unique.insert(path.clone()) {
            total = total.saturating_add(std::fs::metadata(path)?.len());
        }
    }
    if total > max_bytes {
        return Err(RunnerError::CommandFailed {
            command: "diagnostic-artifact-budget".to_owned(),
            output: format!(
                "cumulative diagnostic artifacts are {total} bytes, above maxArtifactBytes {max_bytes}"
            ),
        });
    }
    Ok(())
}

fn enforce_json_sample_limit(path: &Path, max_samples: u64) -> Result<(), RunnerError> {
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let samples = value
        .get("traceEvents")
        .and_then(Value::as_array)
        .map_or(0, |events| u64::try_from(events.len()).unwrap_or(u64::MAX));
    if samples > max_samples {
        return Err(RunnerError::CommandFailed {
            command: "diagnostic-sample-limit".to_owned(),
            output: format!(
                "available Hermes trace event count {samples} exceeds maxSamples {max_samples}"
            ),
        });
    }
    Ok(())
}

fn requested_collector(request: &AndroidRunRequest, collector: &str) -> Option<bool> {
    if request.run_mode != RunMode::Diagnose {
        return None;
    }
    request.diagnostic_plan.as_ref().and_then(|plan| {
        plan.collectors
            .iter()
            .find(|item| item.collector == collector)
            .map(|item| item.required)
    })
}

async fn send_android_diagnostic_command(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    command: &str,
    token: &str,
    lease_ms: Option<u64>,
) -> Result<AndroidDiagnosticControllerAck, RunnerError> {
    validate_android_package_id(app_id)?;
    let component = format!("{app_id}/.ReactorDiagnosticsController");
    let action = format!("{app_id}.DIAGNOSTICS");
    let lease_ms = lease_ms.unwrap_or_default().to_string();
    android_shell_text(
        adb,
        device_id,
        &[
            "am",
            "broadcast",
            "-a",
            &action,
            "-n",
            &component,
            "--es",
            "command",
            command,
            "--es",
            "token",
            token,
            "--el",
            "leaseMs",
            &lease_ms,
        ],
        "rn-diagnostics-controller",
    )
    .await?;

    let remote = format!("/sdcard/Android/data/{app_id}/files/reactor/{RN_CONTROLLER_ACK_FILE}");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(output) =
            android_shell_text(adb, device_id, &["cat", &remote], "rn-diagnostics-ack").await
            && let Ok(ack) = serde_json::from_str::<AndroidDiagnosticControllerAck>(&output)
            && ack.token == token
            && ack.command == command
        {
            if ack.status == "collected" {
                return Ok(ack);
            }
            return Err(RunnerError::CommandFailed {
                command: format!("rn-diagnostics-controller {command}"),
                output: ack
                    .error
                    .unwrap_or_else(|| format!("collector status: {}", ack.status)),
            });
        }
        if Instant::now() >= deadline {
            return Err(RunnerError::CommandTimedOut {
                command: format!("rn-diagnostics-controller {command}"),
                seconds: 15,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn start_android_hermes_cpu(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    lease_ms: u64,
) -> Result<AndroidHermesCpuCapture, RunnerError> {
    let token = format!("reactor-{}", uuid::Uuid::new_v4().simple());
    let ack =
        send_android_diagnostic_command(adb, device_id, app_id, "start", &token, Some(lease_ms))
            .await?;
    Ok(AndroidHermesCpuCapture {
        token,
        start_elapsed_realtime_nanos: ack.elapsed_realtime_nanos,
        sdk_version: ack.sdk_version,
        capabilities: ack.capabilities,
        diagnostic_build: ack.diagnostic_build,
    })
}

async fn finish_android_hermes_cpu(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    capture: &AndroidHermesCpuCapture,
    artifact_dir: &Path,
    max_bytes: u64,
) -> Result<(PathBuf, u64), RunnerError> {
    let ack = send_android_diagnostic_command(
        adb,
        device_id,
        app_id,
        "stopAndDump",
        &capture.token,
        None,
    )
    .await?;
    let remote = format!("/sdcard/Android/data/{app_id}/files/reactor/{RN_HERMES_CPU_FILE}");
    let local = artifact_dir.join(RN_HERMES_CPU_FILE);
    let pulled = pull_optional_android_artifact(
        adb,
        device_id,
        &remote,
        &local,
        max_bytes.min(256 * 1024 * 1024),
    )
    .await;
    let _ = android_shell_text(
        adb,
        device_id,
        &["rm", "-f", &remote],
        "rn-hermes-cpu-cleanup",
    )
    .await;
    let pulled = pulled?.ok_or_else(|| RunnerError::CommandFailed {
        command: "rn-hermes-cpu-artifact".to_owned(),
        output: "controller acknowledged capture but artifact is missing".to_owned(),
    })?;
    Ok((PathBuf::from(pulled), ack.elapsed_realtime_nanos))
}

async fn abort_android_hermes_cpu(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    capture: &AndroidHermesCpuCapture,
) {
    let _ = send_android_diagnostic_command(adb, device_id, app_id, "abort", &capture.token, None)
        .await;
    delete_android_diagnostic_files(adb, device_id, app_id).await;
}

async fn inspect_android_target(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> AndroidTargetMetadata {
    let manufacturer = adb_value(
        adb,
        device_id,
        &["shell", "getprop", "ro.product.manufacturer"],
    )
    .await;
    let model = adb_value(adb, device_id, &["shell", "getprop", "ro.product.model"]).await;
    let name = match (manufacturer, model) {
        (Some(manufacturer), Some(model)) if !model.starts_with(&manufacturer) => {
            Some(format!("{manufacturer} {model}"))
        }
        (Some(_), Some(model)) => Some(model),
        (Some(manufacturer), None) => Some(manufacturer),
        (None, model) => model,
    };
    let os_version = adb_value(
        adb,
        device_id,
        &["shell", "getprop", "ro.build.version.release"],
    )
    .await;
    let package = adb_value(adb, device_id, &["shell", "dumpsys", "package", app_id]).await;
    AndroidTargetMetadata {
        name,
        os_version,
        app_version: package.as_deref().and_then(parse_android_app_version),
    }
}

fn parse_android_app_version(output: &str) -> Option<String> {
    let version_name = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("versionName="))
        .filter(|value| !value.is_empty() && *value != "null");
    let version_code = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("versionCode="))
        .and_then(|value| value.split_whitespace().next())
        .filter(|value| !value.is_empty());
    match (version_name, version_code) {
        (Some(name), Some(code)) => Some(format!("{name} ({code})")),
        (Some(name), None) => Some(name.to_owned()),
        (None, Some(code)) => Some(code.to_owned()),
        (None, None) => None,
    }
}

async fn measure_android_startup(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<Option<f64>, RunnerError> {
    let component = resolve_android_launcher_component(adb, device_id, app_id).await?;
    android_shell_text(adb, device_id, &["am", "force-stop", app_id], "force-stop").await?;
    let output = android_shell_text(
        adb,
        device_id,
        &["am", "start", "-W", "-n", &component],
        "am-start",
    )
    .await?;
    Ok(parse_startup_total_time(&output))
}

async fn resolve_android_launcher_component(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<String, RunnerError> {
    let resolved = android_shell_text(
        adb,
        device_id,
        &["cmd", "package", "resolve-activity", "--brief", app_id],
        "resolve Android launcher activity",
    )
    .await?;
    parse_android_launcher_component(&resolved).ok_or_else(|| RunnerError::CommandFailed {
        command: "resolve Android launcher activity".to_owned(),
        output: format!("package {app_id} has no resolvable Launcher Activity"),
    })
}

fn parse_android_launcher_component(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .rev()
        .find(|line| {
            let Some((package, activity)) = line.split_once('/') else {
                return false;
            };
            !package.is_empty() && !activity.is_empty() && !line.chars().any(char::is_whitespace)
        })
        .map(ToOwned::to_owned)
}

async fn start_android_launcher_activity(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<String, RunnerError> {
    let component = resolve_android_launcher_component(adb, device_id, app_id).await?;
    android_shell_text(
        adb,
        device_id,
        &["am", "start", "-W", "-n", &component],
        "start Android launcher activity",
    )
    .await
}

fn parse_startup_total_time(output: &str) -> Option<f64> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("TotalTime:"))
        .and_then(|value| value.trim().parse::<f64>().ok())
}

async fn sample_android_thermal(adb: &Path, device_id: &str) -> Result<Option<u32>, RunnerError> {
    Ok(parse_thermal_status(
        &android_shell_text(adb, device_id, &["dumpsys", "thermalservice"], "thermal").await?,
    ))
}

fn parse_thermal_status(output: &str) -> Option<u32> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Thermal Status:"))
        .and_then(|value| value.trim().parse().ok())
}

async fn sample_android_memory_pss(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<Option<f64>, RunnerError> {
    let output =
        android_shell_text(adb, device_id, &["dumpsys", "meminfo", app_id], "meminfo").await?;
    Ok(parse_memory_pss_mb(&output))
}

async fn sample_android_memory_checkpoint(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    kind: &str,
    cycle: u32,
    started_at: Instant,
) -> Result<AndroidMemoryCheckpoint, RunnerError> {
    let output =
        android_shell_text(adb, device_id, &["dumpsys", "meminfo", app_id], "meminfo").await?;
    let mut checkpoint = parse_memory_checkpoint(
        &output,
        kind,
        cycle,
        u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    checkpoint.cpu_pct = sample_android_cpu_pct(adb, device_id, app_id)
        .await
        .ok()
        .flatten();
    Ok(checkpoint)
}

/// Collects one lightweight Android CPU/memory sample outside the formal measurement window.
///
/// This is used by interactive Flow trials so the desktop can provide immediate feedback without
/// turning trial observations into benchmark evidence.
///
/// # Errors
///
/// Returns an error when the package id is invalid, managed ADB is unavailable, or the device
/// cannot provide its current process metrics.
pub async fn sample_android_live_performance(
    workspace: &Path,
    device_id: &str,
    app_id: &str,
    elapsed_ms: u64,
) -> Result<Value, RunnerError> {
    validate_android_package_id(app_id)?;
    let adb = resolve_tools(workspace)
        .adb
        .ok_or(RunnerError::MissingTool("adb"))?;
    let output = android_shell_text(
        &adb,
        device_id,
        &["dumpsys", "meminfo", app_id],
        "trial-live-meminfo",
    )
    .await?;
    let mut checkpoint = parse_memory_checkpoint(&output, "trial", 0, elapsed_ms);
    checkpoint.cpu_pct = sample_android_cpu_pct(&adb, device_id, app_id)
        .await
        .ok()
        .flatten();
    let remote = format!("/sdcard/Android/data/{app_id}/files/reactor/rn-diagnostics.ndjson");
    let rn = android_shell_text(
        &adb,
        device_id,
        &["tail", "-n", "1000", &remote],
        "trial-live-rn-diagnostics",
    )
    .await
    .ok()
    .map(|output| summarize_live_rn_events(&output));
    Ok(serde_json::json!({
        "kind": "live_telemetry",
        "source": "trial_observer",
        "elapsedMs": checkpoint.elapsed_ms,
        "cpuPct": checkpoint.cpu_pct,
        "pssMb": checkpoint.pss_mb,
        "rssMb": checkpoint.rss_mb,
        "javaHeapMb": checkpoint.java_heap_mb,
        "nativeHeapMb": checkpoint.native_heap_mb,
        "rn": rn,
        "officialMetric": false,
    }))
}

async fn run_android_live_observer(
    workspace: PathBuf,
    job_id: String,
    adb: PathBuf,
    device_id: String,
    app_id: String,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    const SAMPLE_INTERVAL: Duration = Duration::from_millis(2_000);
    let started_at = Instant::now();
    if let Ok(store) = open_store(&workspace) {
        let _ = store.append_event(
            &job_id,
            JobState::Measuring,
            "已开启 Flow 同屏性能观察",
            Some(&serde_json::json!({
                "kind": "live_observer_status",
                "active": true,
                "samplingIntervalMs": SAMPLE_INTERVAL.as_millis(),
                "officialMetric": false,
            })),
        );
    }
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
            () = tokio::time::sleep(SAMPLE_INTERVAL) => {
                let checkpoint = sample_android_memory_checkpoint(
                    &adb,
                    &device_id,
                    &app_id,
                    "live",
                    0,
                    started_at,
                ).await;
                let remote = format!(
                    "/sdcard/Android/data/{app_id}/files/reactor/rn-diagnostics.ndjson"
                );
                let rn = android_shell_text(
                    &adb,
                    &device_id,
                    &["tail", "-n", "1000", &remote],
                    "live-rn-diagnostics",
                )
                .await
                .ok()
                .map(|output| summarize_live_rn_events(&output));
                if let Ok(checkpoint) = checkpoint
                    && let Ok(store) = open_store(&workspace)
                {
                    let _ = store.append_event(
                        &job_id,
                        JobState::Measuring,
                        "Flow 执行中 · 实时观察样本",
                        Some(&serde_json::json!({
                            "kind": "live_telemetry",
                            "source": "flow_observer",
                            "elapsedMs": checkpoint.elapsed_ms,
                            "cpuPct": checkpoint.cpu_pct,
                            "pssMb": checkpoint.pss_mb,
                            "rssMb": checkpoint.rss_mb,
                            "javaHeapMb": checkpoint.java_heap_mb,
                            "nativeHeapMb": checkpoint.native_heap_mb,
                            "rn": rn,
                            "officialMetric": false,
                        })),
                    );
                }
            }
        }
    }
    if let Ok(store) = open_store(&workspace) {
        let _ = store.append_event(
            &job_id,
            JobState::Measuring,
            "Flow 同屏性能观察结束，开始归一化正式证据",
            Some(&serde_json::json!({
                "kind": "live_observer_status",
                "active": false,
                "officialMetric": false,
            })),
        );
    }
}

fn summarize_live_rn_events(output: &str) -> Value {
    let mut total = 0_u64;
    let mut renders = 0_u64;
    let mut tree_commits = 0_u64;
    let mut commits = 0_u64;
    let mut console = 0_u64;
    let mut network = 0_u64;
    let mut hermes_heap = 0_u64;
    let mut duplicate_renders = 0_u64;
    let mut rendered_components = BTreeSet::new();
    let mut component_render_window_ms = (None, None);
    // Live diagnostics are deliberately a bounded observation window. Keep the
    // per-component breakdown alongside the totals so the UI can explain a
    // repeated render without inventing Profiler timings for release builds.
    let mut components = BTreeMap::<String, (u64, u64, u64, Option<f64>)>::new();
    let mut slowest_commit_ms = None::<f64>;
    let mut slowest_commit_name = None::<String>;
    let mut latest_kind = None;
    let mut latest_name = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        total = total.saturating_add(1);
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let event_name = event
            .pointer("/payload/name")
            .or_else(|| event.pointer("/payload/id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        match kind {
            "component_render" => {
                renders = renders.saturating_add(1);
                update_observed_time_window(&mut component_render_window_ms, &event);
                if let Some(name) = event_name.as_ref() {
                    let component = components.entry(name.clone()).or_default();
                    component.0 = component.0.saturating_add(1);
                    if !rendered_components.insert(name.clone()) {
                        duplicate_renders = duplicate_renders.saturating_add(1);
                        component.1 = component.1.saturating_add(1);
                    }
                }
            }
            "component_tree" => tree_commits = tree_commits.saturating_add(1),
            "react_profile" => {
                commits = commits.saturating_add(1);
                let duration = event
                    .pointer("/payload/actualDuration")
                    .or_else(|| event.pointer("/payload/duration"))
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite() && *value >= 0.0);
                if let Some(duration) = duration
                    && slowest_commit_ms.is_none_or(|current| duration > current)
                {
                    slowest_commit_ms = Some(duration);
                    slowest_commit_name = event
                        .pointer("/payload/id")
                        .or_else(|| event.pointer("/payload/name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if let Some(name) = event_name.as_ref() {
                    let component = components.entry(name.clone()).or_default();
                    component.2 = component.2.saturating_add(1);
                    if let Some(duration) = duration
                        && component.3.is_none_or(|current| duration > current)
                    {
                        component.3 = Some(duration);
                    }
                }
            }
            "console" => console = console.saturating_add(1),
            "network" => network = network.saturating_add(1),
            "hermes_heap" => hermes_heap = hermes_heap.saturating_add(1),
            _ => {}
        }
        latest_kind = Some(kind.to_owned());
        latest_name = event_name;
    }
    let component_rows = live_component_rows(components);
    serde_json::json!({
        "sampledEventCount": total,
        "componentRenderCount": renders,
        "duplicateComponentRenderCount": duplicate_renders,
        "componentRenderWindowStartMs": component_render_window_ms.0,
        "componentRenderWindowEndMs": component_render_window_ms.1,
        "componentRenderWindowDurationMs": component_render_window_ms.0.zip(component_render_window_ms.1).map(|(start, end)| end.saturating_sub(start)),
        "componentTreeCommitCount": tree_commits,
        "profileCommitCount": commits,
        "slowestCommitMs": slowest_commit_ms,
        "slowestCommitName": slowest_commit_name,
        "components": component_rows,
        "consoleEventCount": console,
        "networkEventCount": network,
        "hermesHeapSampleCount": hermes_heap,
        "latestKind": latest_kind,
        "latestName": latest_name,
        "windowLimit": 1000,
    })
}

fn update_observed_time_window(window: &mut (Option<u64>, Option<u64>), event: &Value) {
    let Some(timestamp_ms) = event.get("timestampMs").and_then(Value::as_u64) else {
        return;
    };
    window.0 = Some(window.0.map_or(timestamp_ms, |start| start.min(timestamp_ms)));
    window.1 = Some(window.1.map_or(timestamp_ms, |end| end.max(timestamp_ms)));
}

fn live_component_rows(components: BTreeMap<String, (u64, u64, u64, Option<f64>)>) -> Vec<Value> {
    let mut rows = components
        .into_iter()
        .map(|(name, (render_count, duplicate_render_count, profile_commit_count, max_commit_ms))| {
            serde_json::json!({
                "name": name,
                "renderCount": render_count,
                "duplicateRenderCount": duplicate_render_count,
                "profileCommitCount": profile_commit_count,
                "maxCommitMs": max_commit_ms,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        let left_duplicate = left["duplicateRenderCount"].as_u64().unwrap_or_default();
        let right_duplicate = right["duplicateRenderCount"].as_u64().unwrap_or_default();
        let left_renders = left["renderCount"].as_u64().unwrap_or_default();
        let right_renders = right["renderCount"].as_u64().unwrap_or_default();
        right_duplicate
            .cmp(&left_duplicate)
            .then_with(|| right_renders.cmp(&left_renders))
            .then_with(|| left["name"].as_str().cmp(&right["name"].as_str()))
    });
    rows
}

async fn sample_android_cpu_pct(
    adb: &Path,
    device_id: &str,
    app_id: &str,
) -> Result<Option<f64>, RunnerError> {
    let output = android_shell_text(adb, device_id, &["dumpsys", "cpuinfo"], "cpuinfo").await?;
    Ok(parse_android_cpu_pct(&output, app_id))
}

fn parse_android_cpu_pct(output: &str, app_id: &str) -> Option<f64> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        (trimmed.contains(app_id))
            .then(|| trimmed.split_whitespace().next())
            .flatten()
            .and_then(|value| value.strip_suffix('%'))
            .and_then(|value| value.parse::<f64>().ok())
    })
}

fn parse_memory_checkpoint(
    output: &str,
    kind: &str,
    cycle: u32,
    elapsed_ms: u64,
) -> AndroidMemoryCheckpoint {
    let summary_kb = |label: &str| {
        output.lines().find_map(|line| {
            line.trim()
                .strip_prefix(label)
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<f64>().ok())
        })
    };
    let proportional_kb = summary_kb("TOTAL PSS:");
    let resident_kb = output.lines().find_map(|line| {
        line.split_once("TOTAL RSS:")
            .and_then(|(_, value)| value.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok())
    });
    AndroidMemoryCheckpoint {
        kind: kind.to_owned(),
        cycle,
        elapsed_ms,
        cpu_pct: None,
        pss_mb: proportional_kb.map(|value| value / 1024.0),
        rss_mb: resident_kb.map(|value| value / 1024.0),
        java_heap_mb: summary_kb("Java Heap:").map(|value| value / 1024.0),
        native_heap_mb: summary_kb("Native Heap:").map(|value| value / 1024.0),
    }
}

fn validate_leak_test_plan(plan: &AndroidLeakTestPlan) -> Result<(), RunnerError> {
    if !(3..=500).contains(&plan.cycles) {
        return Err(RunnerError::InvalidLeakTestPlan(
            "cycles must be between 3 and 500".to_owned(),
        ));
    }
    if plan.checkpoint_every == 0 || plan.checkpoint_every > plan.cycles {
        return Err(RunnerError::InvalidLeakTestPlan(
            "checkpointEvery must be between 1 and cycles".to_owned(),
        ));
    }
    if plan.warmup_cycles >= plan.cycles {
        return Err(RunnerError::InvalidLeakTestPlan(
            "warmupCycles must be lower than cycles".to_owned(),
        ));
    }
    if plan.stabilization_ms > 60_000 || plan.cooldown_ms > 300_000 {
        return Err(RunnerError::InvalidLeakTestPlan(
            "stabilization/cooldown exceeds the bounded limit".to_owned(),
        ));
    }
    if !plan.threshold_mb_per_cycle.is_finite() || plan.threshold_mb_per_cycle <= 0.0 {
        return Err(RunnerError::InvalidLeakTestPlan(
            "thresholdMbPerCycle must be a positive finite number".to_owned(),
        ));
    }
    Ok(())
}

fn linear_slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / count;
    let denominator = points
        .iter()
        .map(|(x, _)| (x - mean_x).powi(2))
        .sum::<f64>();
    (denominator > f64::EPSILON).then(|| {
        points
            .iter()
            .map(|(x, y)| (x - mean_x) * (y - mean_y))
            .sum::<f64>()
            / denominator
    })
}

fn analyze_memory_leak(
    plan: &AndroidLeakTestPlan,
    checkpoints: Vec<AndroidMemoryCheckpoint>,
) -> AndroidMemoryLeakReport {
    let cycle_points = checkpoints
        .iter()
        .filter(|point| point.kind == "cycle" && point.cycle > plan.warmup_cycles)
        .filter_map(|point| point.pss_mb.map(|pss| (f64::from(point.cycle), pss)))
        .collect::<Vec<_>>();
    let slope_mb_per_cycle = linear_slope(&cycle_points);
    let end_delta_mb = cycle_points
        .first()
        .zip(cycle_points.last())
        .map(|(first, last)| last.1 - first.1);
    let monotonic_growth_pct = (cycle_points.len() > 1).then(|| {
        let growing = cycle_points
            .windows(2)
            .filter(|pair| pair[1].1 >= pair[0].1)
            .count();
        growing as f64 / (cycle_points.len() - 1) as f64 * 100.0
    });
    let last_cycle_pss = checkpoints
        .iter()
        .rev()
        .find(|point| point.kind == "cycle")
        .and_then(|point| point.pss_mb);
    let cooldown_pss = checkpoints
        .iter()
        .rev()
        .find(|point| point.kind == "cooldown")
        .and_then(|point| point.pss_mb);
    let cooldown_recovery_mb = last_cycle_pss
        .zip(cooldown_pss)
        .map(|(before, after)| before - after);
    let enough_points = cycle_points.len() >= 3;
    let sustained = slope_mb_per_cycle.is_some_and(|slope| slope >= plan.threshold_mb_per_cycle)
        && monotonic_growth_pct.is_some_and(|growth| growth >= 60.0)
        && end_delta_mb.is_some_and(|delta| {
            delta
                >= plan.threshold_mb_per_cycle
                    * f64::from(plan.cycles.saturating_sub(plan.warmup_cycles))
                    * 0.5
        });
    let verdict = if !enough_points {
        "insufficient_evidence"
    } else if sustained {
        "suspected_leak"
    } else {
        "stable"
    };
    let confidence = if !enough_points {
        "low"
    } else if cycle_points.len() >= 6 && cooldown_recovery_mb.is_some() {
        "high"
    } else {
        "medium"
    };
    let mut warnings =
        vec!["进程内存趋势只能判定疑似泄漏；没有堆对象保留链时不得宣称确认泄漏。".to_owned()];
    if checkpoints.iter().any(|point| point.pss_mb.is_none()) {
        warnings.push("部分检查点缺少 TOTAL PSS，判定置信度可能降低。".to_owned());
    }
    AndroidMemoryLeakReport {
        schema_version: 1,
        definitions_version: "android-memory-leak-v2".to_owned(),
        collector: "adb-dumpsys-meminfo-checkpoints-v1".to_owned(),
        cycles: plan.cycles,
        checkpoint_every: plan.checkpoint_every,
        warmup_cycles: plan.warmup_cycles,
        stabilization_ms: plan.stabilization_ms,
        cooldown_ms: plan.cooldown_ms,
        slope_mb_per_cycle,
        end_delta_mb,
        monotonic_growth_pct,
        cooldown_recovery_mb,
        threshold_mb_per_cycle: plan.threshold_mb_per_cycle,
        verdict: verdict.to_owned(),
        confidence: confidence.to_owned(),
        native_heap_trace_file: None,
        native_retained_bytes: None,
        native_retained_allocation_count: None,
        retention_evidence: None,
        managed_retained_object_count: None,
        managed_retained_bytes: None,
        checkpoints,
        warnings,
    }
}

fn reconcile_memory_leak_with_rn_diagnostics(
    report: &mut AndroidMemoryLeakReport,
    diagnostics: Option<&ReactNativeDiagnosticsSummary>,
) {
    let Some(diagnostics) = diagnostics else {
        return;
    };
    if diagnostics.retained_object_count == 0 || diagnostics.retained_bytes == 0 {
        return;
    }
    report.retention_evidence = Some("reactor-rn-sdk-object-lifecycle-v1".to_owned());
    report.managed_retained_object_count = Some(diagnostics.retained_object_count);
    report.managed_retained_bytes = Some(diagnostics.retained_bytes);
    let effective_cycles = report.cycles.saturating_sub(report.warmup_cycles);
    let sustained = report
        .slope_mb_per_cycle
        .is_some_and(|slope| slope >= report.threshold_mb_per_cycle)
        && report
            .monotonic_growth_pct
            .is_some_and(|growth| growth >= 60.0)
        && report.end_delta_mb.is_some_and(|delta| {
            delta >= report.threshold_mb_per_cycle * f64::from(effective_cycles) * 0.5
        });
    report.warnings.retain(|warning| {
        warning != "进程内存趋势只能判定疑似泄漏；没有堆对象保留链时不得宣称确认泄漏。"
    });
    if sustained {
        "confirmed_leak".clone_into(&mut report.verdict);
        if diagnostics.hermes_heap_snapshot_file.is_some() {
            "high".clone_into(&mut report.confidence);
        } else {
            "medium".clone_into(&mut report.confidence);
        }
        report.warnings.push(format!(
            "增长趋势与 RN 对象保留证据同时成立：{} 个对象、{} bytes 在 Flow 结束后仍被保留。",
            diagnostics.retained_object_count, diagnostics.retained_bytes
        ));
    } else {
        "suspected_leak".clone_into(&mut report.verdict);
        "medium".clone_into(&mut report.confidence);
        report.warnings.push(format!(
            "检测到 {} 个受管对象仍被保留，但进程增长趋势尚不足以确认泄漏。",
            diagnostics.retained_object_count
        ));
    }
}

async fn pull_optional_android_artifact(
    adb: &Path,
    device_id: &str,
    remote: &str,
    local: &Path,
    max_bytes: u64,
) -> Result<Option<String>, RunnerError> {
    let size = match android_shell_text(
        adb,
        device_id,
        &["stat", "-c", "%s", remote],
        "artifact-stat",
    )
    .await
    {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map_err(|error| RunnerError::CommandFailed {
                command: "artifact-stat".to_owned(),
                output: format!("invalid size for {remote}: {error}"),
            })?,
        Err(RunnerError::CommandFailed { output, .. })
            if output.contains("No such file") || output.contains("cannot stat") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if size == 0 {
        return Ok(None);
    }
    if size > max_bytes {
        return Err(RunnerError::CommandFailed {
            command: "artifact-size-gate".to_owned(),
            output: format!("{remote} is {size} bytes, above the {max_bytes} byte limit"),
        });
    }
    let mut pull = Command::new(adb);
    pull.args([
        "-s",
        device_id,
        "pull",
        remote,
        &local.display().to_string(),
    ]);
    run_command_with_timeout(pull, "artifact-pull", Duration::from_secs(60)).await?;
    Ok(Some(local.display().to_string()))
}

#[allow(clippy::too_many_lines)]
async fn capture_react_native_diagnostics(
    adb: &Path,
    device_id: &str,
    app_id: &str,
    artifact_dir: &Path,
    max_events: u64,
    max_artifact_bytes: u64,
) -> Result<Option<ReactNativeDiagnosticsSummary>, RunnerError> {
    const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024 * 1024;
    let max_diagnostic_bytes = max_artifact_bytes.min(MAX_DIAGNOSTIC_BYTES as u64);
    let remote = format!("/sdcard/Android/data/{app_id}/files/reactor/rn-diagnostics.ndjson");
    let output = match android_shell_text(adb, device_id, &["cat", &remote], "rn-diagnostics").await
    {
        Ok(output) => output,
        Err(RunnerError::CommandFailed { output, .. })
            if output.contains("No such file") || output.contains("Permission denied") =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if output.is_empty() {
        return Ok(None);
    }
    if u64::try_from(output.len()).unwrap_or(u64::MAX) > max_diagnostic_bytes {
        return Err(RunnerError::CommandFailed {
            command: "rn-diagnostics".to_owned(),
            output: format!(
                "RN diagnostic evidence exceeds the {max_diagnostic_bytes} byte diagnostic limit"
            ),
        });
    }
    let mut component_names = BTreeSet::new();
    let mut retained = BTreeMap::<String, u64>::new();
    let mut event_count = 0_u64;
    let mut component_render_count = 0_u64;
    let mut component_tree_commit_count = 0_u64;
    let mut profile_commit_count = 0_u64;
    let mut console_event_count = 0_u64;
    let mut network_event_count = 0_u64;
    let mut hermes_heap_sample_count = 0_u64;
    let mut allocated = BTreeSet::new();
    let mut benchmark_mode = None;
    let mut recent_events = Vec::new();
    let mut latest_component_tree = None;
    for (index, line) in output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let event: Value =
            serde_json::from_str(line).map_err(|error| RunnerError::CommandFailed {
                command: "parse rn-diagnostics".to_owned(),
                output: format!("invalid NDJSON event at line {}: {error}", index + 1),
            })?;
        event_count = event_count.saturating_add(1);
        if event_count > max_events {
            return Err(RunnerError::CommandFailed {
                command: "rn-diagnostics-event-limit".to_owned(),
                output: format!("event count exceeds maxEvents {max_events}"),
            });
        }
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = event.get("payload").unwrap_or(&Value::Null);
        let diagnostic_event = ReactNativeDiagnosticEvent {
            timestamp_ms: event
                .get("timestampMs")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            kind: kind.to_owned(),
            payload: payload.clone(),
        };
        if kind == "component_tree" {
            latest_component_tree = Some(diagnostic_event);
        } else if recent_events.len() < 1_999 {
            recent_events.push(diagnostic_event);
        }
        match kind {
            "benchmark_mode" => {
                benchmark_mode = payload
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            "component_render" => {
                component_render_count = component_render_count.saturating_add(1);
                if let Some(name) = payload.get("name").and_then(Value::as_str) {
                    component_names.insert(name.to_owned());
                }
            }
            "component_tree" => {
                component_tree_commit_count = component_tree_commit_count.saturating_add(1);
                for node in payload
                    .get("nodes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = node.get("name").and_then(Value::as_str) {
                        component_names.insert(name.to_owned());
                    }
                }
            }
            "react_profile" => {
                profile_commit_count = profile_commit_count.saturating_add(1);
                if let Some(name) = payload.get("id").and_then(Value::as_str) {
                    component_names.insert(name.to_owned());
                }
            }
            "console" => console_event_count = console_event_count.saturating_add(1),
            "network" => network_event_count = network_event_count.saturating_add(1),
            "hermes_heap" => hermes_heap_sample_count = hermes_heap_sample_count.saturating_add(1),
            "object_lifecycle" => {
                let object_id = payload
                    .get("objectId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let bytes = payload.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                match payload.get("action").and_then(Value::as_str) {
                    Some("allocate") if !object_id.is_empty() => {
                        allocated.insert(object_id.to_owned());
                    }
                    Some("retain") if !object_id.is_empty() => {
                        retained.insert(object_id.to_owned(), bytes);
                    }
                    Some("release") if !object_id.is_empty() => {
                        retained.remove(object_id);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if let Some(latest_component_tree) = latest_component_tree {
        recent_events.push(latest_component_tree);
    }
    let path = artifact_dir.join("rn-diagnostics.ndjson");
    fs::write(&path, output).await?;
    let remote_profile =
        format!("/sdcard/Android/data/{app_id}/files/reactor/rn-react-devtools-profile.json");
    let devtools_profile = android_shell_text(
        adb,
        device_id,
        &["cat", &remote_profile],
        "rn-devtools-profile",
    )
    .await
    .ok()
    .filter(|profile| u64::try_from(profile.len()).unwrap_or(u64::MAX) <= max_diagnostic_bytes)
    .and_then(|profile| serde_json::from_str::<Value>(&profile).ok())
    .filter(|profile| {
        profile
            .get("dataForRoots")
            .and_then(Value::as_array)
            .is_some()
    });
    let profile = devtools_profile
        .map(|profile| enrich_react_profile_source_locations(profile, &recent_events))
        .or_else(|| build_managed_react_profile(&recent_events));
    let profile_file = if let Some(profile) = profile {
        let profile_path = artifact_dir.join("rn-profile.json");
        fs::write(&profile_path, serde_json::to_vec_pretty(&profile)?).await?;
        Some(profile_path.display().to_string())
    } else {
        None
    };
    let remote_root = format!("/sdcard/Android/data/{app_id}/files/reactor");
    let mut warnings = Vec::new();
    let hermes_heap_stats_file = match pull_optional_android_artifact(
        adb,
        device_id,
        &format!("{remote_root}/rn-hermes-heap-stats.ndjson"),
        &artifact_dir.join("rn-hermes-heap-stats.ndjson"),
        max_diagnostic_bytes,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("Hermes Heap 统计未保存：{error}"));
            None
        }
    };
    let hermes_heap_snapshot_file = match pull_optional_android_artifact(
        adb,
        device_id,
        &format!("{remote_root}/rn-hermes.heapsnapshot"),
        &artifact_dir.join("rn-hermes.heapsnapshot"),
        max_artifact_bytes,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("Hermes Heap Snapshot 未保存：{error}"));
            None
        }
    };
    let java_heap_dump_file = match pull_optional_android_artifact(
        adb,
        device_id,
        &format!("{remote_root}/rn-java.hprof"),
        &artifact_dir.join("rn-java.hprof"),
        max_artifact_bytes,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("Java HPROF 未保存：{error}"));
            None
        }
    };
    if hermes_heap_snapshot_file.is_none() {
        warnings.push(
            "通用 JS 对象保留确认需要使用 Reactor 诊断构建采集 Hermes Heap Snapshot。".to_owned(),
        );
    }
    if java_heap_dump_file.is_none() {
        warnings
            .push("Java HPROF 只在独立诊断构建中采集，不用于 Release Benchmark 数值。".to_owned());
    }
    let retained_bytes = retained.values().copied().sum();
    Ok(Some(ReactNativeDiagnosticsSummary {
        schema_version: 1,
        collector: "reactor-rn-sdk-v1".to_owned(),
        benchmark_mode,
        event_file: path.display().to_string(),
        event_count,
        component_names: component_names.into_iter().collect(),
        component_render_count,
        component_tree_commit_count,
        profile_commit_count,
        console_event_count,
        network_event_count,
        hermes_heap_sample_count,
        allocated_object_count: u64::try_from(allocated.len()).unwrap_or(u64::MAX),
        retained_object_count: u64::try_from(retained.len()).unwrap_or(u64::MAX),
        retained_bytes,
        profile_file,
        hermes_heap_stats_file,
        hermes_heap_snapshot_file,
        java_heap_dump_file,
        recent_events,
        warnings,
    }))
}

fn build_managed_react_profile(events: &[ReactNativeDiagnosticEvent]) -> Option<Value> {
    let mut parents = BTreeMap::<String, Option<String>>::new();
    let mut source_locations = BTreeMap::<String, Value>::new();
    let mut commits = Vec::new();
    for event in events {
        match event.kind.as_str() {
            "component_render" => {
                if let Some(name) = event.payload.get("name").and_then(Value::as_str) {
                    parents.insert(
                        name.to_owned(),
                        event
                            .payload
                            .get("parent")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    );
                    if let Some(file) = event.payload.get("sourceFile").and_then(Value::as_str) {
                        source_locations.insert(
                            name.to_owned(),
                            serde_json::json!({
                                "file": file,
                                "line": event.payload.get("sourceLine").and_then(Value::as_u64),
                                "column": event.payload.get("sourceColumn").and_then(Value::as_u64),
                            }),
                        );
                    }
                }
            }
            "react_profile" => {
                let Some(id) = event.payload.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let duration = event
                    .payload
                    .get("actualDuration")
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite())
                    .unwrap_or_default();
                let self_duration = event
                    .payload
                    .get("baseDuration")
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite())
                    .unwrap_or(duration);
                parents.entry(id.to_owned()).or_default();
                commits.push(serde_json::json!({
                    "timestamp": event.payload.get("commitTime").and_then(Value::as_f64),
                    "duration": duration,
                    "fiberActualDurations": [[id, duration]],
                    "fiberSelfDurations": [[id, self_duration]],
                    "changeDescriptions": {}
                }));
            }
            _ => {}
        }
    }
    if commits.is_empty() {
        return None;
    }
    let names = parents.keys().cloned().collect::<BTreeSet<_>>();
    let snapshots = parents
        .keys()
        .map(|name| {
            let children = parents
                .iter()
                .filter_map(|(child, candidate_parent)| {
                    (candidate_parent.as_deref() == Some(name.as_str())).then_some(child)
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "id": name,
                "displayName": name,
                "children": children,
            })
        })
        .collect::<Vec<_>>();
    let roots = parents
        .iter()
        .filter_map(|(name, parent)| {
            parent
                .as_ref()
                .is_none_or(|parent| !names.contains(parent))
                .then_some(name)
        })
        .cloned()
        .collect::<Vec<_>>();
    Some(serde_json::json!({
        "version": 5,
        "source": "reactor-rn-sdk-v1",
        "sourceLocations": source_locations,
        "dataForRoots": [{
            "rootID": roots.first().cloned().unwrap_or_else(|| "reactor-root".to_owned()),
            "snapshots": snapshots,
            "commitData": commits,
        }]
    }))
}

fn enrich_react_profile_source_locations(
    mut profile: Value,
    events: &[ReactNativeDiagnosticEvent],
) -> Value {
    let locations_by_name = events
        .iter()
        .filter(|event| event.kind == "component_render")
        .filter_map(|event| {
            let name = event.payload.get("name").and_then(Value::as_str)?;
            let file = event.payload.get("sourceFile").and_then(Value::as_str)?;
            Some((
                name.to_owned(),
                serde_json::json!({
                    "file": file,
                    "line": event.payload.get("sourceLine").and_then(Value::as_u64),
                    "column": event.payload.get("sourceColumn").and_then(Value::as_u64),
                }),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    if locations_by_name.is_empty() {
        return profile;
    }

    let mut source_locations = profile
        .get("sourceLocations")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for root in profile
        .get("dataForRoots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for snapshot in root
            .get("snapshots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some((id, node)) = react_profile_snapshot_entry(snapshot) else {
                continue;
            };
            let Some(location) = node
                .get("displayName")
                .and_then(Value::as_str)
                .and_then(|name| locations_by_name.get(name))
            else {
                continue;
            };
            source_locations.insert(id, location.clone());
        }
    }
    if let Some(object) = profile.as_object_mut() {
        object.insert(
            "sourceLocations".to_owned(),
            Value::Object(source_locations),
        );
    }
    profile
}

fn react_profile_snapshot_entry(value: &Value) -> Option<(String, &Value)> {
    if let Some(pair) = value.as_array() {
        let id = pair.first().and_then(|id| match id {
            Value::String(id) => Some(id.clone()),
            Value::Number(id) => Some(id.to_string()),
            _ => None,
        })?;
        return Some((id, pair.get(1)?));
    }
    let id = value.get("id").and_then(|id| match id {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    })?;
    Some((id, value))
}

#[allow(clippy::too_many_arguments)]
async fn run_android_measured_cycle_with_progress(
    workspace: &Path,
    job_id: &str,
    cycle: u32,
    total_cycles: u32,
    maestro: &Path,
    java: &Path,
    adb: &Path,
    device_id: &str,
    measured_path: &Path,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<usize>();
    let event_workspace = workspace.to_path_buf();
    let event_job_id = job_id.to_owned();
    let event_task = tokio::spawn(async move {
        while let Some(command_index) = receiver.recv().await {
            let command_number = command_index.saturating_add(1);
            if let Ok(store) = open_store(&event_workspace) {
                let _ = store.append_event(
                    &event_job_id,
                    JobState::Measuring,
                    &format!("Flow 循环 {cycle}/{total_cycles} · 命令 {command_number} 已完成"),
                    Some(&serde_json::json!({
                        "kind": "flow_progress",
                        "cycle": cycle,
                        "totalCycles": total_cycles,
                        "commandIndex": command_index,
                        "commandNumber": command_number
                    })),
                );
            }
        }
    });
    let flows = [measured_path.to_path_buf()];
    let result = run_maestro_paths_with_inputs_progress(
        maestro,
        java,
        adb,
        &flows,
        Some(device_id),
        input_environment,
        Some(sender),
    )
    .await;
    let _ = event_task.await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_android_memory_leak_test(
    plan: &AndroidLeakTestPlan,
    workspace: &Path,
    job_id: &str,
    maestro: &Path,
    java: &Path,
    adb: &Path,
    device_id: &str,
    app_id: &str,
    setup_path: Option<&Path>,
    measured_path: &Path,
    teardown_path: Option<&Path>,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<AndroidMemoryLeakReport, RunnerError> {
    validate_leak_test_plan(plan)?;
    if let Some(setup_path) = setup_path {
        run_maestro_with_inputs(
            maestro,
            java,
            adb,
            setup_path,
            Some(device_id),
            input_environment,
        )
        .await?;
    }
    let started_at = Instant::now();
    let mut checkpoints = Vec::new();
    for cycle in 1..=plan.cycles {
        run_android_measured_cycle_with_progress(
            workspace,
            job_id,
            cycle,
            plan.cycles,
            maestro,
            java,
            adb,
            device_id,
            measured_path,
            input_environment,
        )
        .await?;
        if cycle % plan.checkpoint_every == 0 || cycle == plan.cycles {
            tokio::time::sleep(Duration::from_millis(plan.stabilization_ms)).await;
            let checkpoint = sample_android_memory_checkpoint(
                adb, device_id, app_id, "cycle", cycle, started_at,
            )
            .await?;
            open_store(workspace)?.append_event(
                job_id,
                JobState::Measuring,
                &format!("内存循环 {cycle}/{}", plan.cycles),
                Some(&serde_json::json!({
                    "kind": "live_telemetry",
                    "source": "memory_checkpoint",
                    "cycle": cycle,
                    "totalCycles": plan.cycles,
                    "elapsedMs": checkpoint.elapsed_ms,
                    "cpuPct": checkpoint.cpu_pct,
                    "pssMb": checkpoint.pss_mb,
                    "rssMb": checkpoint.rss_mb,
                    "javaHeapMb": checkpoint.java_heap_mb,
                    "nativeHeapMb": checkpoint.native_heap_mb,
                    "officialMetric": false
                })),
            )?;
            checkpoints.push(checkpoint);
        }
    }
    tokio::time::sleep(Duration::from_millis(plan.cooldown_ms)).await;
    checkpoints.push(
        sample_android_memory_checkpoint(
            adb,
            device_id,
            app_id,
            "cooldown",
            plan.cycles,
            started_at,
        )
        .await?,
    );
    if let Some(teardown_path) = teardown_path {
        run_maestro_with_inputs(
            maestro,
            java,
            adb,
            teardown_path,
            Some(device_id),
            input_environment,
        )
        .await?;
    }
    Ok(analyze_memory_leak(plan, checkpoints))
}

fn parse_memory_pss_mb(output: &str) -> Option<f64> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("TOTAL PSS:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<f64>().ok())
            .map(|kilobytes| kilobytes / 1024.0)
    })
}

/// Runs one locked Flow through managed Maestro and Flashlight.
///
/// # Errors
///
/// Returns an error when tools, automation, collection, normalization, or storage fails.
pub async fn run_android(
    request: &AndroidRunRequest,
) -> Result<(Job, NormalizedResult), RunnerError> {
    let job = enqueue_android(request)?;
    execute_android_job(request, &job.id).await
}

/// Executes an already queued Android job. This entry point is safe to call from a detached
/// worker because all progress and output locations are persisted before it returns.
///
/// # Errors
///
/// Returns an error after also recording the terminal failed state in `SQLite`.
pub async fn execute_android_job(
    request: &AndroidRunRequest,
    job_id: &str,
) -> Result<(Job, NormalizedResult), RunnerError> {
    let result = execute_android_job_inner(request, job_id).await;
    if let Err(error) = &result {
        if let Ok(lock_bytes) = fs::read(&request.flow_lock).await
            && let Ok(lock) = serde_json::from_slice::<FlowLock>(&lock_bytes)
            && validate_android_package_id(&lock.flow.app_id).is_ok()
            && let Some(adb) = resolve_tools(&request.workspace).adb
        {
            delete_android_diagnostic_files(&adb, &request.device_id, &lock.flow.app_id).await;
        }
        if let Ok(store) = open_store(&request.workspace) {
            let _ = store.fail(job_id, &error.to_string());
        }
    }
    result
}

#[allow(clippy::too_many_lines)]
async fn execute_android_job_inner(
    request: &AndroidRunRequest,
    job_id: &str,
) -> Result<(Job, NormalizedResult), RunnerError> {
    transition_job(
        &request.workspace,
        job_id,
        JobState::Preflight,
        "验证 Flow 与受管工具",
        None,
    )?;

    let lock: FlowLock = serde_json::from_slice(&fs::read(&request.flow_lock).await?)?;
    lock.verify()?;
    validate_android_package_id(&lock.flow.app_id)?;
    if !lock.has_android_trial(&request.device_id) {
        return Err(RunnerError::MissingAndroidTrial(request.device_id.clone()));
    }
    let compiled = compile_maestro(&lock.flow)?;
    let input_environment = if request.manual_session {
        Vec::new()
    } else {
        resolve_input_environment(&compiled.input_bindings, None)?
    };
    let tools = resolve_tools(&request.workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let flashlight = tools
        .flashlight
        .ok_or(RunnerError::MissingTool("flashlight"))?;
    let trace_processor = tools
        .trace_processor
        .ok_or(RunnerError::MissingTool("trace_processor"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let adb = tools.adb.ok_or(RunnerError::MissingTool("adb"))?;
    let target_metadata = inspect_android_target(&adb, &request.device_id, &lock.flow.app_id).await;

    let artifact_dir = request.workspace.join("results/runs").join(job_id);
    fs::create_dir_all(&artifact_dir).await?;
    let marker_foundation_path = write_flow_marker_foundation(&artifact_dir, &lock.flow).await?;
    register_artifact(
        &request.workspace,
        job_id,
        "flow_marker_foundation",
        &marker_foundation_path,
    )?;
    ensure_android_trace_space(&adb, &request.device_id, &artifact_dir).await?;
    let setup_path = artifact_dir.join("setup.yaml");
    let measured_path = artifact_dir.join("measured.yaml");
    let teardown_path = artifact_dir.join("teardown.yaml");
    fs::write(&setup_path, compiled.setup).await?;
    fs::write(&measured_path, compiled.measured).await?;
    fs::write(&teardown_path, compiled.teardown).await?;
    register_artifact(&request.workspace, job_id, "flow_setup", &setup_path)?;
    register_artifact(&request.workspace, job_id, "flow_measured", &measured_path)?;
    register_artifact(&request.workspace, job_id, "flow_teardown", &teardown_path)?;
    let ai_audit_path = write_measurement_ai_audit(&artifact_dir, job_id).await?;
    register_artifact(
        &request.workspace,
        job_id,
        "measurement_ai_audit",
        &ai_audit_path,
    )?;

    transition_job(
        &request.workspace,
        job_id,
        JobState::Warmup,
        if request.manual_session {
            "准备手动诊断录制"
        } else {
            "执行非计分预热"
        },
        None,
    )?;
    if !request.manual_session && !lock.flow.setup.is_empty() {
        run_maestro_with_inputs(
            &maestro,
            &java,
            &adb,
            &setup_path,
            Some(&request.device_id),
            &input_environment,
        )
        .await?;
    }
    if !request.manual_session {
        run_maestro_with_inputs(
            &maestro,
            &java,
            &adb,
            &measured_path,
            Some(&request.device_id),
            &input_environment,
        )
        .await?;
    }
    if !request.manual_session && !lock.flow.teardown.is_empty() {
        run_maestro_with_inputs(
            &maestro,
            &java,
            &adb,
            &teardown_path,
            Some(&request.device_id),
            &input_environment,
        )
        .await?;
    }

    transition_job(
        &request.workspace,
        job_id,
        JobState::Measuring,
        if request.manual_session {
            "手动诊断录制已开始；请在 App 中自由操作"
        } else {
            "执行锁定 Flow 并采集原生指标"
        },
        None,
    )?;
    let mut native_warnings = Vec::new();
    let startup_time_ms =
        match measure_android_startup(&adb, &request.device_id, &lock.flow.app_id).await {
            Ok(value) => value,
            Err(error) => {
                native_warnings.push(format!("启动时间不可用：{error}"));
                None
            }
        };
    let thermal_status_before = match sample_android_thermal(&adb, &request.device_id).await {
        Ok(value) => value,
        Err(error) => {
            native_warnings.push(format!("测量前热状态不可用：{error}"));
            None
        }
    };
    let raw_path = artifact_dir.join("flashlight.json");
    let perfetto_path = artifact_dir.join("perfetto.pftrace");
    let expected_trace_ms = request
        .duration_ms
        .saturating_mul(u64::from(request.iteration_count))
        .saturating_add(180_000)
        .min(30 * 60 * 1_000);
    let perfetto = start_perfetto(
        &adb,
        &request.device_id,
        &lock.flow.app_id,
        job_id,
        expected_trace_ms,
    )
    .await?;
    let mut rn_framework_collectors = BTreeMap::new();
    let hermes_cpu_requested = requested_collector(request, "hermes-cpu");
    let hermes_cpu_capture = if let Some(required) = hermes_cpu_requested {
        let lease_ms = request
            .diagnostic_plan
            .as_ref()
            .map_or(60_000, |plan| plan.resource_limits.max_duration_ms);
        match start_android_hermes_cpu(&adb, &request.device_id, &lock.flow.app_id, lease_ms).await
        {
            Ok(capture) => Some(capture),
            Err(error) if required => {
                let _ = stop_perfetto(&adb, &request.device_id, &perfetto, &perfetto_path).await;
                return Err(error);
            }
            Err(error) => {
                native_warnings.push(format!("Hermes CPU 采集未启动：{error}"));
                rn_framework_collectors.insert(
                    "hermes-cpu".to_owned(),
                    CollectorDiagnosticV1 {
                        status: CollectorStatus::Failed,
                        artifacts: Vec::new(),
                        reason: Some(error.to_string()),
                    },
                );
                None
            }
        }
    } else {
        None
    };
    let (observer_stop, observer_stop_rx) = tokio::sync::watch::channel(false);
    let observer_task = tokio::spawn(run_android_live_observer(
        request.workspace.clone(),
        job_id.to_owned(),
        adb.clone(),
        request.device_id.clone(),
        lock.flow.app_id.clone(),
        observer_stop_rx,
    ));
    let flashlight_result = if request.manual_session {
        run_flashlight_manual(
            &flashlight,
            &java,
            &adb,
            &raw_path,
            &lock.flow.app_id,
            request,
            job_id,
        )
        .await
    } else {
        run_flashlight(
            &flashlight,
            &maestro,
            &java,
            &adb,
            &setup_path,
            &measured_path,
            (!lock.flow.teardown.is_empty()).then_some(teardown_path.as_path()),
            &raw_path,
            &lock.flow.app_id,
            request,
            &input_environment,
        )
        .await
    };
    let _ = observer_stop.send(true);
    let _ = observer_task.await;
    let hermes_cpu_result = if let Some(capture) = &hermes_cpu_capture {
        Some(
            finish_android_hermes_cpu(
                &adb,
                &request.device_id,
                &lock.flow.app_id,
                capture,
                &artifact_dir,
                request
                    .diagnostic_plan
                    .as_ref()
                    .map_or(256 * 1024 * 1024, |plan| {
                        plan.resource_limits.max_artifact_bytes
                    }),
            )
            .await,
        )
    } else {
        None
    };
    let perfetto_result = stop_perfetto(&adb, &request.device_id, &perfetto, &perfetto_path).await;
    if perfetto_path.is_file() {
        register_artifact(&request.workspace, job_id, "perfetto_trace", &perfetto_path)?;
    }
    if let Err(error) = flashlight_result {
        if let Some(capture) = &hermes_cpu_capture
            && hermes_cpu_result.as_ref().is_some_and(Result::is_err)
        {
            abort_android_hermes_cpu(&adb, &request.device_id, &lock.flow.app_id, capture).await;
        }
        let _ = perfetto_result;
        return Err(error);
    }
    perfetto_result?;
    if let Some(result) = hermes_cpu_result {
        match result {
            Ok((path, end_elapsed_realtime_nanos)) => {
                let limits = &request
                    .diagnostic_plan
                    .as_ref()
                    .expect("Hermes diagnostic capture requires a validated plan")
                    .resource_limits;
                enforce_json_sample_limit(&path, limits.max_samples)?;
                enforce_diagnostic_artifacts(
                    &artifact_dir,
                    std::iter::once(path.clone()),
                    limits.max_artifact_bytes,
                )?;
                let artifact = open_store(&request.workspace)?.register_artifact(
                    job_id,
                    "react_native_hermes_cpu",
                    &path,
                )?;
                let capture = hermes_cpu_capture
                    .as_ref()
                    .expect("Hermes result requires a capture session");
                rn_framework_collectors.insert(
                    "hermes-cpu".to_owned(),
                    CollectorDiagnosticV1 {
                        status: CollectorStatus::Collected,
                        artifacts: vec![ArtifactRef {
                            path: path.file_name().map_or_else(
                                || RN_HERMES_CPU_FILE.to_owned(),
                                |name| name.to_string_lossy().into_owned(),
                            ),
                            format: "hermes-sampling-chrome-trace-json".to_owned(),
                            size_bytes: artifact.size_bytes,
                            sha256: artifact.sha256,
                            producer: "reactor-rn-sdk".to_owned(),
                            producer_version: "1.0.0".to_owned(),
                            capture_method: "official-public-hermes-sampling-profiler".to_owned(),
                            integrity: ArtifactIntegrity::Complete,
                            time_range: Some(reactor_protocol::ArtifactTimeRangeV1 {
                                start_ns: capture.start_elapsed_realtime_nanos,
                                end_ns: end_elapsed_realtime_nanos,
                                clock: "android_elapsed_realtime".to_owned(),
                            }),
                        }],
                        reason: None,
                    },
                );
            }
            Err(error) if hermes_cpu_requested == Some(true) => {
                delete_android_diagnostic_files(&adb, &request.device_id, &lock.flow.app_id).await;
                return Err(error);
            }
            Err(error) => {
                native_warnings.push(format!("Hermes CPU 采集未保存：{error}"));
                rn_framework_collectors.insert(
                    "hermes-cpu".to_owned(),
                    CollectorDiagnosticV1 {
                        status: CollectorStatus::Failed,
                        artifacts: Vec::new(),
                        reason: Some(error.to_string()),
                    },
                );
            }
        }
    }
    register_artifact(&request.workspace, job_id, "flashlight_raw", &raw_path)?;
    let mut memory_leak = if let Some(plan) = &request.leak_test {
        open_store(&request.workspace)?.append_event(
            job_id,
            JobState::Measuring,
            "同一进程循环 Flow 并采集内存检查点",
            None,
        )?;
        let heap_duration_ms = u64::from(plan.cycles)
            .saturating_mul(30_000)
            .saturating_add(plan.stabilization_ms.saturating_mul(u64::from(plan.cycles)))
            .saturating_add(plan.cooldown_ms)
            .saturating_add(120_000)
            .min(30 * 60 * 1_000);
        let heap_config = heapprofd_config(&lock.flow.app_id, heap_duration_ms);
        let heap_session = start_perfetto_with_config(
            &adb,
            &request.device_id,
            &format!("reactor-{job_id}-heapprofd"),
            &heap_config,
        )
        .await;
        let leak_result = run_android_memory_leak_test(
            plan,
            &request.workspace,
            job_id,
            &maestro,
            &java,
            &adb,
            &request.device_id,
            &lock.flow.app_id,
            (!lock.flow.setup.is_empty()).then_some(setup_path.as_path()),
            &measured_path,
            (!lock.flow.teardown.is_empty()).then_some(teardown_path.as_path()),
            &input_environment,
        )
        .await;
        let native_heap_path = artifact_dir.join("android-native-heap.pftrace");
        let native_heap_result = match heap_session {
            Ok(session) => {
                stop_perfetto(&adb, &request.device_id, &session, &native_heap_path).await
            }
            Err(error) => Err(error),
        };
        let mut report = leak_result?;
        match native_heap_result {
            Ok(()) => {
                register_artifact(
                    &request.workspace,
                    job_id,
                    "android_native_heap_trace",
                    &native_heap_path,
                )?;
                report.native_heap_trace_file = Some(native_heap_path.display().to_string());
                match analyze_heapprofd_trace(&trace_processor, &native_heap_path).await {
                    Ok((bytes, count)) => {
                        report.native_retained_bytes = Some(bytes);
                        report.native_retained_allocation_count = Some(count);
                    }
                    Err(error) => report
                        .warnings
                        .push(format!("Native Heap 保留量解析失败：{error}")),
                }
            }
            Err(error) => report
                .warnings
                .push(format!("Perfetto heapprofd 不可用：{error}")),
        }
        Some(report)
    } else {
        None
    };
    let rn_diagnostics = if request.framework == "react-native" {
        let limits = request
            .diagnostic_plan
            .as_ref()
            .map(|plan| &plan.resource_limits);
        match capture_react_native_diagnostics(
            &adb,
            &request.device_id,
            &lock.flow.app_id,
            &artifact_dir,
            limits.map_or(500_000, |limits| limits.max_events),
            limits.map_or(256 * 1024 * 1024, |limits| limits.max_artifact_bytes),
        )
        .await
        {
            Ok(Some(summary)) => {
                let event_path = Path::new(&summary.event_file);
                let event_artifact = open_store(&request.workspace)?.register_artifact(
                    job_id,
                    "react_native_diagnostics",
                    event_path,
                )?;
                let mut runtime_artifacts = vec![ArtifactRef {
                    path: event_path.file_name().map_or_else(
                        || summary.event_file.clone(),
                        |name| name.to_string_lossy().into_owned(),
                    ),
                    format: "reactor-rn-events-ndjson".to_owned(),
                    size_bytes: event_artifact.size_bytes,
                    sha256: event_artifact.sha256,
                    producer: summary.collector.clone(),
                    producer_version: format!("schema-{}", summary.schema_version),
                    capture_method: "reactor-owned-versioned-bridge".to_owned(),
                    integrity: ArtifactIntegrity::Complete,
                    time_range: None,
                }];
                if let Some(profile_file) = &summary.profile_file {
                    let profile_path = Path::new(profile_file);
                    let profile_artifact = open_store(&request.workspace)?.register_artifact(
                        job_id,
                        "react_native_profile",
                        profile_path,
                    )?;
                    runtime_artifacts.push(ArtifactRef {
                        path: profile_path.file_name().map_or_else(
                            || profile_file.clone(),
                            |name| name.to_string_lossy().into_owned(),
                        ),
                        format: "react-devtools-profile-json".to_owned(),
                        size_bytes: profile_artifact.size_bytes,
                        sha256: profile_artifact.sha256,
                        producer: summary.collector.clone(),
                        producer_version: format!("schema-{}", summary.schema_version),
                        capture_method: "react-profiler-managed-export".to_owned(),
                        integrity: ArtifactIntegrity::Complete,
                        time_range: None,
                    });
                }
                rn_framework_collectors.insert(
                    "react-runtime".to_owned(),
                    CollectorDiagnosticV1 {
                        status: CollectorStatus::Collected,
                        artifacts: runtime_artifacts,
                        reason: None,
                    },
                );
                for (kind, path) in [
                    (
                        "react_native_hermes_heap_stats",
                        &summary.hermes_heap_stats_file,
                    ),
                    (
                        "react_native_hermes_heap_snapshot",
                        &summary.hermes_heap_snapshot_file,
                    ),
                    ("react_native_java_heap_dump", &summary.java_heap_dump_file),
                ] {
                    if let Some(path) = path {
                        register_artifact(&request.workspace, job_id, kind, Path::new(path))?;
                    }
                }
                Some(summary)
            }
            Ok(None) => {
                native_warnings.push("目标 App 未提供 Reactor RN SDK 诊断证据。".to_owned());
                None
            }
            Err(error) => {
                native_warnings.push(format!("RN SDK 诊断证据不可用：{error}"));
                None
            }
        }
    } else {
        None
    };
    if let Some(limits) = request
        .diagnostic_plan
        .as_ref()
        .map(|plan| &plan.resource_limits)
    {
        let mut diagnostic_paths = Vec::new();
        if artifact_dir.join(RN_HERMES_CPU_FILE).is_file() {
            diagnostic_paths.push(artifact_dir.join(RN_HERMES_CPU_FILE));
        }
        if let Some(summary) = &rn_diagnostics {
            diagnostic_paths.push(PathBuf::from(&summary.event_file));
            diagnostic_paths.extend(
                [
                    &summary.profile_file,
                    &summary.hermes_heap_stats_file,
                    &summary.hermes_heap_snapshot_file,
                    &summary.java_heap_dump_file,
                ]
                .into_iter()
                .flatten()
                .map(PathBuf::from),
            );
        }
        enforce_diagnostic_artifacts(&artifact_dir, diagnostic_paths, limits.max_artifact_bytes)?;
    }
    delete_android_diagnostic_files(&adb, &request.device_id, &lock.flow.app_id).await;
    if let Some(report) = memory_leak.as_mut() {
        reconcile_memory_leak_with_rn_diagnostics(report, rn_diagnostics.as_ref());
        let path = artifact_dir.join("android-memory-leak.json");
        fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(report)?),
        )
        .await?;
        register_artifact(&request.workspace, job_id, "android_memory_leak", &path)?;
    }
    transition_job(
        &request.workspace,
        job_id,
        JobState::Normalizing,
        "归一化指标并保留原始证据",
        None,
    )?;
    let raw: Value = serde_json::from_slice(&fs::read(&raw_path).await?)?;
    let mut result =
        normalize_flashlight(&raw, &lock, request, job_id, &raw_path, &target_metadata)?;
    let frame_metrics = parse_perfetto_frames(
        &trace_processor,
        &perfetto_path,
        &lock.flow.app_id,
        result.device.refresh_rate,
    )
    .await?;
    if frame_metrics.frame_count == 0 {
        native_warnings.push(
            "Perfetto FrameTimeline 未找到应用 Surface 帧；保留 trace 但拒绝推导帧结论。"
                .to_owned(),
        );
    }
    let thermal_status_after = match sample_android_thermal(&adb, &request.device_id).await {
        Ok(value) => value,
        Err(error) => {
            native_warnings.push(format!("测量后热状态不可用：{error}"));
            None
        }
    };
    let memory_pss_mb =
        match sample_android_memory_pss(&adb, &request.device_id, &lock.flow.app_id).await {
            Ok(value) => value,
            Err(error) => {
                native_warnings.push(format!("原生 PSS 内存不可用：{error}"));
                None
            }
        };
    let jank_frame_pct = (frame_metrics.frame_count > 0)
        .then(|| frame_metrics.jank_frame_count as f64 / frame_metrics.frame_count as f64 * 100.0);
    let native_metrics = AndroidNativeMetrics {
        schema_version: 1,
        definitions_version: "android-native-v1".to_owned(),
        collector: "perfetto-frametimeline-v1".to_owned(),
        trace_processor_version: "57.2".to_owned(),
        perfetto_trace_file: perfetto_path.display().to_string(),
        frame_count: frame_metrics.frame_count,
        frame_time_mean_ms: frame_metrics.frame_time_mean_ms,
        frame_time_p50_ms: frame_metrics.frame_time_p50_ms,
        frame_time_p95_ms: frame_metrics.frame_time_p95_ms,
        frame_time_p99_ms: frame_metrics.frame_time_p99_ms,
        jank_frame_count: frame_metrics.jank_frame_count,
        jank_frame_pct,
        over_budget_frame_pct: frame_metrics.over_budget_frame_pct,
        startup_time_ms,
        memory_pss_mb,
        thermal_status_before,
        thermal_status_after,
        memory_leak,
        rn_diagnostics,
        warnings: native_warnings,
    };
    let native_metrics_path = artifact_dir.join("android-native-metrics.json");
    fs::write(
        &native_metrics_path,
        format!("{}\n", serde_json::to_string_pretty(&native_metrics)?),
    )
    .await?;
    register_artifact(
        &request.workspace,
        job_id,
        "android_native_metrics",
        &native_metrics_path,
    )?;
    result.android_native = Some(native_metrics);
    result.run_mode = request.run_mode;
    result.diagnostic_plan = request.diagnostic_plan.clone();
    if let Some(capture) = &hermes_cpu_capture {
        let mut capabilities = capture.capabilities.clone();
        capabilities.sort();
        capabilities.dedup();
        let app_version = target_metadata
            .app_version
            .clone()
            .unwrap_or_else(|| "unknown".to_owned());
        let variant = if capture.diagnostic_build {
            "diagnostic"
        } else {
            "unknown"
        };
        result.build_identity = Some(BuildIdentityV1 {
            schema_version: 1,
            app_version: app_version.clone(),
            react_native_version: None,
            react_version: None,
            hermes_version: None,
            reactor_sdk_version: capture.sdk_version.clone(),
            variant: variant.to_owned(),
            optimization_mode: "release-optimized".to_owned(),
            bundle_sha256: None,
            binary_sha256: None,
            capabilities: capabilities.clone(),
            fingerprint: format!(
                "android:{app_version}:{variant}:{}:{}",
                capture.sdk_version.as_deref().unwrap_or("unknown"),
                capabilities.join(",")
            ),
        });
    }
    if request.framework == "react-native" && !rn_framework_collectors.is_empty() {
        result.framework_diagnostics = Some(FrameworkDiagnosticsV1 {
            react_native: Some(ReactNativeFrameworkDiagnosticsV1 {
                collectors: rn_framework_collectors,
            }),
        });
    }
    let result_path = artifact_dir.join("result.json");
    fs::write(
        &result_path,
        format!("{}\n", serde_json::to_string_pretty(&result)?),
    )
    .await?;
    let report_path = artifact_dir.join("report.html");
    fs::write(
        &report_path,
        render_html_report(
            &format!("{} · {}", request.framework, request.scenario),
            std::slice::from_ref(&result),
        ),
    )
    .await?;
    let store = open_store(&request.workspace)?;
    store.register_artifact(job_id, "normalized_result", &result_path)?;
    store.register_artifact(job_id, "html_report", &report_path)?;
    store.upsert_device(
        &request.device_id,
        "android",
        !request.device_id.starts_with("emulator-"),
        &serde_json::to_value(&result.device)?,
    )?;
    store.index_result(
        job_id,
        &result.run_id,
        Some(&request.device_id),
        &serde_json::to_value(&result)?,
    )?;
    let completed = transition_job(
        &request.workspace,
        job_id,
        JobState::Completed,
        "性能结果已生成",
        Some(&result_path.display().to_string()),
    )?;
    Ok((completed, result))
}

/// Runs one locked Flow on an iOS Simulator while collecting a native xctrace Time Profiler
/// recording. Unsupported Simulator metrics are preserved as explicit availability states rather
/// than fabricated placeholders.
///
/// # Errors
///
/// Returns an error when the Flow trial, Simulator app, xctrace recording, export, parsing, or
/// artifact persistence fails.
pub async fn run_ios(request: &IosRunRequest) -> Result<(Job, NormalizedResult), RunnerError> {
    let job = enqueue_ios(request)?;
    execute_ios_job(request, &job.id).await
}

/// Executes an already queued iOS Simulator job from a detached worker.
///
/// # Errors
///
/// Returns an error after recording a terminal failed state in `SQLite`.
pub async fn execute_ios_job(
    request: &IosRunRequest,
    job_id: &str,
) -> Result<(Job, NormalizedResult), RunnerError> {
    let result = execute_ios_job_inner(request, job_id).await;
    if let Err(error) = &result
        && let Ok(store) = open_store(&request.workspace)
    {
        let _ = store.fail(job_id, &error.to_string());
    }
    result
}

#[allow(clippy::too_many_lines)]
async fn execute_ios_job_inner(
    request: &IosRunRequest,
    job_id: &str,
) -> Result<(Job, NormalizedResult), RunnerError> {
    transition_job(
        &request.workspace,
        job_id,
        JobState::Preflight,
        "验证 iOS Flow、Simulator 与 xctrace",
        None,
    )?;
    let lock: FlowLock = serde_json::from_slice(&fs::read(&request.flow_lock).await?)?;
    lock.verify()?;
    if !lock.has_ios_simulator_trial(&request.device_id) {
        return Err(RunnerError::MissingIosTrial(request.device_id.clone()));
    }
    let compiled = compile_maestro(&lock.flow)?;
    let input_environment = resolve_input_environment(&compiled.input_bindings, None)?;
    let tools = resolve_tools(&request.workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let executable = ios_app_executable_name(&request.device_id, &lock.flow.app_id).await?;

    let artifact_dir = request.workspace.join("results/runs").join(job_id);
    fs::create_dir_all(&artifact_dir).await?;
    let marker_foundation_path = write_flow_marker_foundation(&artifact_dir, &lock.flow).await?;
    register_artifact(
        &request.workspace,
        job_id,
        "flow_marker_foundation",
        &marker_foundation_path,
    )?;
    let setup_path = artifact_dir.join("setup.yaml");
    let measured_path = artifact_dir.join("measured.yaml");
    let teardown_path = artifact_dir.join("teardown.yaml");
    fs::write(&setup_path, compiled.setup).await?;
    fs::write(&measured_path, compiled.measured).await?;
    fs::write(&teardown_path, compiled.teardown).await?;
    register_artifact(&request.workspace, job_id, "flow_setup", &setup_path)?;
    register_artifact(&request.workspace, job_id, "flow_measured", &measured_path)?;
    register_artifact(&request.workspace, job_id, "flow_teardown", &teardown_path)?;
    let ai_audit_path = write_measurement_ai_audit(&artifact_dir, job_id).await?;
    register_artifact(
        &request.workspace,
        job_id,
        "measurement_ai_audit",
        &ai_audit_path,
    )?;

    transition_job(
        &request.workspace,
        job_id,
        JobState::Warmup,
        "执行 iOS Simulator 非计分预热",
        None,
    )?;
    if !lock.flow.setup.is_empty() {
        run_maestro_ios_with_inputs(
            &maestro,
            &java,
            &setup_path,
            &request.device_id,
            &input_environment,
        )
        .await?;
    }
    run_maestro_ios_with_inputs(
        &maestro,
        &java,
        &measured_path,
        &request.device_id,
        &input_environment,
    )
    .await?;
    if !lock.flow.teardown.is_empty() {
        run_maestro_ios_with_inputs(
            &maestro,
            &java,
            &teardown_path,
            &request.device_id,
            &input_environment,
        )
        .await?;
    }

    transition_job(
        &request.workspace,
        job_id,
        JobState::Measuring,
        "执行锁定 Flow 并录制 xctrace Time Profiler",
        None,
    )?;
    ensure_ios_app_running(&request.device_id, &lock.flow.app_id).await?;
    let trace_path = artifact_dir.join("time-profiler.trace");
    let mut xctrace = start_xctrace_time_profiler(
        &request.device_id,
        &executable,
        request.duration_ms,
        &trace_path,
    )?;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let measured_result = run_maestro_ios_with_inputs(
        &maestro,
        &java,
        &measured_path,
        &request.device_id,
        &input_environment,
    )
    .await;
    let trace_result = stop_xctrace(&mut xctrace, &trace_path).await;
    measured_result?;
    trace_result?;
    if !lock.flow.teardown.is_empty() {
        run_maestro_ios_with_inputs(
            &maestro,
            &java,
            &teardown_path,
            &request.device_id,
            &input_environment,
        )
        .await?;
    }

    transition_job(
        &request.workspace,
        job_id,
        JobState::Normalizing,
        "导出 xctrace 并生成版本化 iOS 指标",
        None,
    )?;
    let trace_archive = artifact_dir.join("time-profiler.trace.zip");
    archive_xctrace(&trace_path, &trace_archive).await?;
    let toc_path = artifact_dir.join("xctrace-toc.xml");
    let profile_path = artifact_dir.join("xctrace-time-profile.xml");
    export_xctrace(&trace_path, None, &toc_path).await?;
    export_xctrace(
        &trace_path,
        Some("/trace-toc/run[@number=\"1\"]/data/table[@schema=\"time-profile\"]"),
        &profile_path,
    )
    .await?;
    let toc = fs::read_to_string(&toc_path).await?;
    let profile = fs::read_to_string(&profile_path).await?;
    let parsed = parse_xctrace_profile(&toc, &profile)?;
    let cpu_status = if parsed.cpu_mean_pct.is_some() {
        "available_time_profiler_sampling"
    } else {
        "unavailable_no_running_samples"
    };
    let mut warnings = vec![
        "iOS Simulator 不提供可与物理设备等价的帧、内存或能耗指标；Reactor 已硬拒绝占位值。"
            .to_owned(),
    ];
    if parsed.cpu_mean_pct.is_none() {
        warnings.push("Time Profiler 未捕获 Running 样本，CPU 指标不可用。".to_owned());
    }
    let native = IosNativeMetrics {
        schema_version: 1,
        definitions_version: "ios-native-v1".to_owned(),
        collector: "xctrace-time-profiler-v1".to_owned(),
        xctrace_version: parsed.xctrace_version.clone(),
        template: "Time Profiler".to_owned(),
        trace_file: trace_path.display().to_string(),
        trace_archive_file: trace_archive.display().to_string(),
        toc_export_file: toc_path.display().to_string(),
        profile_export_file: profile_path.display().to_string(),
        recording_duration_ms: parsed.duration_ms,
        cpu_sample_count: parsed.cpu_sample_count,
        cpu_mean_pct: parsed.cpu_mean_pct,
        frame_time_p95_ms: None,
        startup_time_ms: None,
        memory_peak_mb: None,
        energy_impact: None,
        availability: IosMetricAvailability {
            cpu: cpu_status.to_owned(),
            frames: "unsupported_on_ios_simulator".to_owned(),
            startup: "not_claimed_without_app_ready_evidence".to_owned(),
            memory: "unsupported_on_ios_simulator_activity_monitor".to_owned(),
            energy: "unsupported_on_ios_simulator".to_owned(),
        },
        warnings: warnings.clone(),
    };
    let iteration = IterationMetrics {
        status: "SUCCESS".to_owned(),
        duration_ms: parsed.duration_ms,
        sample_count: parsed.cpu_sample_count,
        fps_mean: None,
        fps_p10: None,
        low_fps_sample_pct: None,
        ram_mean_mb: None,
        ram_peak_mb: None,
        cpu_mean_pct: parsed.cpu_mean_pct,
        ui_cpu_mean_pct: None,
        js_cpu_mean_pct: None,
    };
    let iterations = vec![iteration];
    let summary = aggregate_iterations(&iterations);
    let result = NormalizedResult {
        schema_version: 1,
        run_id: job_id.to_owned(),
        created_at: Utc::now(),
        framework: request.framework.clone(),
        platform: "ios".to_owned(),
        scenario: request.scenario.clone(),
        adapter: "xctrace-ios".to_owned(),
        build_mode: "release".to_owned(),
        flow_hash: lock.flow_hash,
        run_mode: reactor_protocol::RunMode::Benchmark,
        diagnostic_plan: None,
        build_identity: None,
        artifacts: vec![],
        framework_diagnostics: None,
        app_id: Some(lock.flow.app_id),
        app_version: None,
        device: DeviceMetadata {
            id: Some(request.device_id.clone()),
            name: parsed.device_name,
            os_version: parsed.os_version,
            refresh_rate: 60.0,
            physical: Some(false),
        },
        source: ResultSource {
            name: Some(format!(
                "{} · {} · Reactor",
                request.framework, request.scenario
            )),
            status: Some("SUCCESS".to_owned()),
            raw_file: Some(trace_archive.display().to_string()),
            synthetic: false,
        },
        android_native: None,
        ios_native: Some(native.clone()),
        iterations,
        summary,
        warnings,
    };
    let native_path = artifact_dir.join("ios-native-metrics.json");
    fs::write(
        &native_path,
        format!("{}\n", serde_json::to_string_pretty(&native)?),
    )
    .await?;
    let result_path = artifact_dir.join("result.json");
    fs::write(
        &result_path,
        format!("{}\n", serde_json::to_string_pretty(&result)?),
    )
    .await?;
    let report_path = artifact_dir.join("report.html");
    fs::write(
        &report_path,
        render_html_report(
            &format!("{} · {}", request.framework, request.scenario),
            std::slice::from_ref(&result),
        ),
    )
    .await?;
    for (kind, path) in [
        ("xctrace_archive", trace_archive.as_path()),
        ("xctrace_toc", toc_path.as_path()),
        ("xctrace_profile", profile_path.as_path()),
        ("ios_native_metrics", native_path.as_path()),
        ("normalized_result", result_path.as_path()),
        ("html_report", report_path.as_path()),
    ] {
        register_artifact(&request.workspace, job_id, kind, path)?;
    }
    let store = open_store(&request.workspace)?;
    store.upsert_device(
        &request.device_id,
        "ios",
        false,
        &serde_json::to_value(&result.device)?,
    )?;
    store.index_result(
        job_id,
        &result.run_id,
        Some(&request.device_id),
        &serde_json::to_value(&result)?,
    )?;
    let completed = transition_job(
        &request.workspace,
        job_id,
        JobState::Completed,
        "iOS xctrace 结果已生成",
        Some(&result_path.display().to_string()),
    )?;
    Ok((completed, result))
}

async fn ios_app_executable_name(device_id: &str, app_id: &str) -> Result<String, RunnerError> {
    let container = command_text(
        "xcrun",
        &["simctl", "get_app_container", device_id, app_id, "app"],
        "simctl get_app_container",
        Duration::from_secs(15),
    )
    .await?;
    let info = Path::new(container.trim()).join("Info.plist");
    Ok(command_text(
        "plutil",
        &[
            "-extract",
            "CFBundleExecutable",
            "raw",
            "-o",
            "-",
            &info.display().to_string(),
        ],
        "plutil CFBundleExecutable",
        Duration::from_secs(10),
    )
    .await?
    .trim()
    .to_owned())
}

async fn ensure_ios_app_running(device_id: &str, app_id: &str) -> Result<(), RunnerError> {
    command_text(
        "xcrun",
        &["simctl", "launch", device_id, app_id],
        "simctl launch",
        Duration::from_secs(20),
    )
    .await?;
    Ok(())
}

async fn command_text(
    executable: &str,
    args: &[&str],
    label: &str,
    timeout: Duration,
) -> Result<String, RunnerError> {
    let output = tokio::time::timeout(timeout, Command::new(executable).args(args).output())
        .await
        .map_err(|_| RunnerError::CommandFailed {
            command: label.to_owned(),
            output: format!("timed out after {} seconds", timeout.as_secs()),
        })??;
    if !output.status.success() {
        return Err(RunnerError::CommandFailed {
            command: label.to_owned(),
            output: format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn start_xctrace_time_profiler(
    device_id: &str,
    executable: &str,
    duration_ms: u64,
    trace_path: &Path,
) -> Result<tokio::process::Child, RunnerError> {
    let time_limit = format!("{}ms", duration_ms.saturating_add(120_000));
    let mut command = Command::new("xcrun");
    command.args([
        "xctrace",
        "record",
        "--template",
        "Time Profiler",
        "--device",
        device_id,
        "--attach",
        executable,
        "--time-limit",
        &time_limit,
        "--no-prompt",
        "--output",
        &trace_path.display().to_string(),
    ]);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    Ok(command.spawn()?)
}

async fn stop_xctrace(
    child: &mut tokio::process::Child,
    trace_path: &Path,
) -> Result<(), RunnerError> {
    let child_id = child
        .id()
        .ok_or_else(|| RunnerError::InvalidXctrace("xctrace process has no PID".to_owned()))?;
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-INT", &child_id.to_string()])
            .output()
            .await;
    }
    #[cfg(not(unix))]
    child.start_kill()?;
    let status = tokio::time::timeout(Duration::from_secs(90), child.wait())
        .await
        .map_err(|_| RunnerError::CommandFailed {
            command: "xctrace record".to_owned(),
            output: "timed out while saving trace".to_owned(),
        })??;
    if !status.success() {
        return Err(RunnerError::CommandFailed {
            command: "xctrace record".to_owned(),
            output: format!("xctrace exited with {status}"),
        });
    }
    if !trace_path.is_dir() {
        return Err(RunnerError::InvalidXctrace(
            "recording did not create a .trace bundle".to_owned(),
        ));
    }
    Ok(())
}

async fn archive_xctrace(trace_path: &Path, archive_path: &Path) -> Result<(), RunnerError> {
    let mut command = Command::new("/usr/bin/ditto");
    command.args([
        "-c",
        "-k",
        "--keepParent",
        &trace_path.display().to_string(),
        &archive_path.display().to_string(),
    ]);
    run_command_with_timeout(command, "archive-xctrace", Duration::from_secs(90)).await
}

async fn export_xctrace(
    trace_path: &Path,
    xpath: Option<&str>,
    output_path: &Path,
) -> Result<(), RunnerError> {
    let mut command = Command::new("xcrun");
    command.args([
        "xctrace",
        "export",
        "--input",
        &trace_path.display().to_string(),
    ]);
    if let Some(xpath) = xpath {
        command.args(["--xpath", xpath]);
    } else {
        command.arg("--toc");
    }
    command.args(["--output", &output_path.display().to_string()]);
    run_command_with_timeout(command, "xctrace-export", Duration::from_secs(120)).await?;
    if !output_path.is_file() || std::fs::metadata(output_path)?.len() == 0 {
        return Err(RunnerError::InvalidXctrace(format!(
            "export is missing or empty: {}",
            output_path.display()
        )));
    }
    Ok(())
}

fn parse_xctrace_profile(toc: &str, profile: &str) -> Result<XctraceProfileMetrics, RunnerError> {
    let duration_seconds = capture_xml_text(toc, "duration")?
        .parse::<f64>()
        .map_err(|error| RunnerError::InvalidXctrace(format!("invalid duration: {error}")))?;
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Err(RunnerError::InvalidXctrace(
            "recording duration must be positive".to_owned(),
        ));
    }
    let xctrace_version = capture_xml_text(toc, "instruments-version")?.to_owned();
    let device_tag = Regex::new(r#"<device\s+([^>]*platform=\"iOS Simulator\"[^>]*)/>"#)
        .expect("static device regex")
        .captures(toc)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str());
    let device_name = device_tag.and_then(|tag| capture_xml_attribute(tag, "name"));
    let os_version = device_tag.and_then(|tag| capture_xml_attribute(tag, "os-version"));

    let definition = Regex::new(r#"<weight\s+id=\"([^\"]+)\"[^>]*>([0-9]+)</weight>"#)
        .expect("static weight definition regex");
    let reference =
        Regex::new(r#"<weight\s+ref=\"([^\"]+)\"\s*/>"#).expect("static weight reference regex");
    let mut weights = BTreeMap::new();
    let mut total_weight_ns = 0_u64;
    let mut sample_count = 0_u64;
    for captures in definition.captures_iter(profile) {
        let id = captures.get(1).map_or("", |value| value.as_str());
        let value = captures
            .get(2)
            .map_or("", |value| value.as_str())
            .parse::<u64>()
            .map_err(|error| RunnerError::InvalidXctrace(format!("invalid weight: {error}")))?;
        weights.insert(id.to_owned(), value);
        total_weight_ns = total_weight_ns.saturating_add(value);
        sample_count = sample_count.saturating_add(1);
    }
    for captures in reference.captures_iter(profile) {
        let id = captures.get(1).map_or("", |value| value.as_str());
        let value = weights.get(id).ok_or_else(|| {
            RunnerError::InvalidXctrace(format!("unresolved xctrace weight ref {id}"))
        })?;
        total_weight_ns = total_weight_ns.saturating_add(*value);
        sample_count = sample_count.saturating_add(1);
    }
    let duration_ms = duration_seconds * 1_000.0;
    let cpu_mean_pct =
        (sample_count > 0).then(|| total_weight_ns as f64 / (duration_ms * 1_000_000.0) * 100.0);
    Ok(XctraceProfileMetrics {
        duration_ms,
        cpu_sample_count: sample_count,
        cpu_mean_pct,
        xctrace_version,
        device_name,
        os_version,
    })
}

fn capture_xml_text<'a>(xml: &'a str, tag: &str) -> Result<&'a str, RunnerError> {
    let expression = Regex::new(&format!(r"<{tag}>([^<]+)</{tag}>"))
        .map_err(|error| RunnerError::InvalidXctrace(error.to_string()))?;
    expression
        .captures(xml)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str())
        .ok_or_else(|| RunnerError::InvalidXctrace(format!("missing <{tag}>")))
}

fn capture_xml_attribute(tag: &str, attribute: &str) -> Option<String> {
    Regex::new(&format!(r#"{attribute}=\"([^\"]*)\""#))
        .ok()?
        .captures(tag)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned())
}

/// Compiles and executes a Flow once outside the measurement window, producing hash-bound
/// evidence required by real Android runs.
///
/// # Errors
///
/// Returns an error when validation, compilation, setup, execution, or teardown fails.
pub async fn trial_android(
    workspace: &Path,
    flow: &Flow,
    device_id: &str,
    prompt_values: Option<BTreeMap<String, Zeroizing<String>>>,
) -> Result<FlowTrialEvidence, RunnerError> {
    let flow_hash = canonical_flow_hash(flow)?;
    let compiled = compile_maestro(flow)?;
    let input_environment =
        resolve_input_environment(&compiled.input_bindings, prompt_values.as_ref())?;
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let adb = tools.adb.ok_or(RunnerError::MissingTool("adb"))?;
    let artifact_dir = workspace
        .join("results/trials")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&artifact_dir).await?;
    let setup_path = artifact_dir.join("setup.yaml");
    let measured_path = artifact_dir.join("measured.yaml");
    let teardown_path = artifact_dir.join("teardown.yaml");
    fs::write(&setup_path, compiled.setup).await?;
    fs::write(&measured_path, compiled.measured).await?;
    fs::write(&teardown_path, compiled.teardown).await?;

    if !flow.setup.is_empty() {
        run_maestro_with_inputs(
            &maestro,
            &java,
            &adb,
            &setup_path,
            Some(device_id),
            &input_environment,
        )
        .await?;
    }
    let measured = run_maestro_with_inputs(
        &maestro,
        &java,
        &adb,
        &measured_path,
        Some(device_id),
        &input_environment,
    )
    .await;
    measured?;
    if let Ok(tree) = capture_android_current_ui_tree(workspace, device_id).await {
        fs::write(artifact_dir.join("destination-ui-tree.xml"), tree).await?;
    }
    let teardown = if flow.teardown.is_empty() {
        Ok(())
    } else {
        run_maestro_with_inputs(
            &maestro,
            &java,
            &adb,
            &teardown_path,
            Some(device_id),
            &input_environment,
        )
        .await
    };
    teardown?;

    Ok(FlowTrialEvidence {
        schema_version: 1,
        mode: TrialMode::AndroidTarget,
        passed: true,
        flow_hash,
        executed_at: Utc::now(),
        device_id: Some(device_id.to_owned()),
        artifact_dir: Some(artifact_dir.display().to_string()),
        synthetic: false,
    })
}

/// Executes a Flow once on a booted iOS Simulator and returns hash-bound evidence.
///
/// # Errors
///
/// Returns an error when validation, compilation, Maestro execution, or persistence fails.
pub async fn trial_ios_simulator(
    workspace: &Path,
    flow: &Flow,
    simulator_id: &str,
    prompt_values: Option<BTreeMap<String, Zeroizing<String>>>,
) -> Result<FlowTrialEvidence, RunnerError> {
    let flow_hash = canonical_flow_hash(flow)?;
    let compiled = compile_maestro(flow)?;
    let input_environment =
        resolve_input_environment(&compiled.input_bindings, prompt_values.as_ref())?;
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let artifact_dir = workspace
        .join("results/trials")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&artifact_dir).await?;
    let setup_path = artifact_dir.join("setup.yaml");
    let measured_path = artifact_dir.join("measured.yaml");
    let teardown_path = artifact_dir.join("teardown.yaml");
    fs::write(&setup_path, compiled.setup).await?;
    fs::write(&measured_path, compiled.measured).await?;
    fs::write(&teardown_path, compiled.teardown).await?;

    if !flow.setup.is_empty() {
        run_maestro_ios_with_inputs(
            &maestro,
            &java,
            &setup_path,
            simulator_id,
            &input_environment,
        )
        .await?;
    }
    let measured = run_maestro_ios_with_inputs(
        &maestro,
        &java,
        &measured_path,
        simulator_id,
        &input_environment,
    )
    .await;
    measured?;
    if let Ok(tree) = capture_ios_current_ui_tree(workspace, simulator_id).await {
        fs::write(artifact_dir.join("destination-ui-tree.csv"), tree).await?;
    }
    let teardown = if flow.teardown.is_empty() {
        Ok(())
    } else {
        run_maestro_ios_with_inputs(
            &maestro,
            &java,
            &teardown_path,
            simulator_id,
            &input_environment,
        )
        .await
    };
    teardown?;

    Ok(FlowTrialEvidence {
        schema_version: 1,
        mode: TrialMode::IosSimulator,
        passed: true,
        flow_hash,
        executed_at: Utc::now(),
        device_id: Some(simulator_id.to_owned()),
        artifact_dir: Some(artifact_dir.display().to_string()),
        synthetic: false,
    })
}

/// Produces clearly synthetic compile/validation evidence for the no-device product tour.
/// This evidence can unlock the tour but is rejected by real device measurement.
///
/// # Errors
///
/// Returns an error if the Flow cannot be validated, compiled, or persisted.
pub async fn validate_product_tour_flow(
    workspace: &Path,
    flow: &Flow,
) -> Result<FlowTrialEvidence, RunnerError> {
    let flow_hash = canonical_flow_hash(flow)?;
    let compiled = compile_maestro(flow)?;
    let artifact_dir = workspace
        .join("results/trials")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&artifact_dir).await?;
    fs::write(artifact_dir.join("setup.yaml"), compiled.setup).await?;
    fs::write(artifact_dir.join("measured.yaml"), compiled.measured).await?;
    fs::write(artifact_dir.join("teardown.yaml"), compiled.teardown).await?;
    Ok(FlowTrialEvidence {
        schema_version: 1,
        mode: TrialMode::ProductTourValidation,
        passed: true,
        flow_hash,
        executed_at: Utc::now(),
        device_id: None,
        artifact_dir: Some(artifact_dir.display().to_string()),
        synthetic: true,
    })
}

/// Creates clearly-labelled synthetic results for the no-device product tour.
///
/// # Errors
///
/// Returns an error when the demo job or result cannot be persisted.
pub async fn run_demo_suite(
    workspace: &Path,
    flow: &FlowLock,
) -> Result<(Job, Vec<NormalizedResult>), RunnerError> {
    let job = enqueue_demo(workspace, flow)?;
    execute_demo_job(workspace, flow, &job.id).await
}

/// Executes a queued product-tour job in an independent worker.
///
/// # Errors
///
/// Returns an error after recording failure in `SQLite`.
pub async fn execute_demo_job(
    workspace: &Path,
    flow: &FlowLock,
    job_id: &str,
) -> Result<(Job, Vec<NormalizedResult>), RunnerError> {
    let result = execute_demo_job_inner(workspace, flow, job_id).await;
    if let Err(error) = &result
        && let Ok(store) = open_store(workspace)
    {
        let _ = store.fail(job_id, &error.to_string());
    }
    result
}

async fn execute_demo_job_inner(
    workspace: &Path,
    flow: &FlowLock,
    job_id: &str,
) -> Result<(Job, Vec<NormalizedResult>), RunnerError> {
    flow.verify()?;
    transition_job(
        workspace,
        job_id,
        JobState::Preflight,
        "验证锁定 Flow",
        None,
    )?;
    transition_job(
        workspace,
        job_id,
        JobState::Warmup,
        "准备产品导览数据",
        None,
    )?;
    transition_job(
        workspace,
        job_id,
        JobState::Measuring,
        "模拟三框架运行，仅用于界面体验",
        None,
    )?;
    transition_job(
        workspace,
        job_id,
        JobState::Normalizing,
        "生成带模拟标记的结果",
        None,
    )?;

    let artifact_dir = workspace.join("results/demo").join(job_id);
    fs::create_dir_all(&artifact_dir).await?;
    let ai_audit_path = write_measurement_ai_audit(&artifact_dir, job_id).await?;
    let scenario = flow.flow.id.split('-').next().unwrap_or("custom");
    let results = ["react-native", "flutter", "lynx"]
        .into_iter()
        .enumerate()
        .map(|(index, framework)| synthetic_result(flow, job_id, framework, scenario, index))
        .collect::<Vec<_>>();
    let result_path = artifact_dir.join("results.json");
    fs::write(
        &result_path,
        format!("{}\n", serde_json::to_string_pretty(&results)?),
    )
    .await?;
    let report_path = artifact_dir.join("report.html");
    fs::write(
        &report_path,
        render_html_report("Reactor 三框架产品导览", &results),
    )
    .await?;
    let store = open_store(workspace)?;
    store.register_artifact(job_id, "measurement_ai_audit", &ai_audit_path)?;
    store.register_artifact(job_id, "normalized_results", &result_path)?;
    store.register_artifact(job_id, "html_report", &report_path)?;
    for result in &results {
        store.index_result(job_id, &result.run_id, None, &serde_json::to_value(result)?)?;
    }
    let completed = transition_job(
        workspace,
        job_id,
        JobState::Completed,
        "产品导览结果已生成（模拟数据）",
        Some(&result_path.display().to_string()),
    )?;
    Ok((completed, results))
}

async fn write_measurement_ai_audit(
    artifact_dir: &Path,
    job_id: &str,
) -> Result<PathBuf, RunnerError> {
    let path = artifact_dir.join("ai-audit.json");
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "jobId": job_id,
        "processRole": "reactor-runner",
        "aiProviderLinked": false,
        "modelCalls": {
            "preflight": 0,
            "warmup": 0,
            "measurement": 0,
            "normalizing": 0
        },
        "measurementWindowModelCalls": 0,
        "policy": "AI providers are not linked into the measurement runner"
    });
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&payload)?),
    )
    .await?;
    Ok(path)
}

fn synthetic_result(
    flow: &FlowLock,
    job_id: &str,
    framework: &str,
    scenario: &str,
    framework_index: usize,
) -> NormalizedResult {
    let baseline = match framework {
        "react-native" => (55.8, 47.2, 42.4, 168.0),
        "flutter" => (58.7, 52.8, 36.1, 184.0),
        _ => (57.5, 50.6, 33.8, 151.0),
    };
    let iterations = (0..10)
        .map(|index| {
            let phase = u32::try_from((index * 17 + framework_index * 7) % 9)
                .expect("phase is always below nine");
            let wave = f64::from(phase) - 4.0;
            IterationMetrics {
                status: "SUCCESS".to_owned(),
                duration_ms: 8_000.0 + wave * 13.0,
                sample_count: 80,
                fps_mean: Some(baseline.0 + wave * 0.25),
                fps_p10: Some(baseline.1 + wave * 0.3),
                low_fps_sample_pct: Some((60.0 - baseline.0) * 1.7 + wave.abs() * 0.25),
                ram_mean_mb: Some(baseline.3 - 8.0 + wave),
                ram_peak_mb: Some(baseline.3 + wave * 1.5),
                cpu_mean_pct: Some(baseline.2 + wave * 0.4),
                ui_cpu_mean_pct: Some(baseline.2 * 0.62 + wave * 0.2),
                js_cpu_mean_pct: Some(if framework == "flutter" {
                    0.0
                } else {
                    baseline.2 * 0.24
                }),
            }
        })
        .collect::<Vec<_>>();
    let summary = aggregate_iterations(&iterations);
    NormalizedResult {
        schema_version: 1,
        run_id: format!("{job_id}-{framework}"),
        created_at: Utc::now(),
        framework: framework.to_owned(),
        platform: "android".to_owned(),
        scenario: scenario.to_owned(),
        adapter: "reactor-synthetic-tour".to_owned(),
        build_mode: "release".to_owned(),
        flow_hash: flow.flow_hash.clone(),
        run_mode: reactor_protocol::RunMode::Benchmark,
        diagnostic_plan: None,
        build_identity: None,
        artifacts: vec![],
        framework_diagnostics: None,
        app_id: Some(flow.flow.app_id.clone()),
        app_version: None,
        device: DeviceMetadata {
            id: None,
            name: Some("Reactor product tour".to_owned()),
            os_version: None,
            refresh_rate: 60.0,
            physical: None,
        },
        source: ResultSource {
            name: Some("product-tour".to_owned()),
            status: Some("SYNTHETIC".to_owned()),
            raw_file: None,
            synthetic: true,
        },
        android_native: None,
        ios_native: None,
        iterations,
        summary,
        warnings: vec!["模拟数据仅用于体验 Reactor 工作流，不得用于框架性能结论。".to_owned()],
    }
}

async fn run_maestro_with_inputs(
    maestro: &Path,
    java: &Path,
    adb: &Path,
    flow: &Path,
    device_id: Option<&str>,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    let flows = [flow.to_path_buf()];
    run_maestro_paths_with_inputs(maestro, java, adb, &flows, device_id, input_environment).await
}

async fn run_maestro_paths_with_inputs(
    maestro: &Path,
    java: &Path,
    adb: &Path,
    flows: &[PathBuf],
    device_id: Option<&str>,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    run_maestro_paths_with_inputs_progress(
        maestro,
        java,
        adb,
        flows,
        device_id,
        input_environment,
        None,
    )
    .await
}

async fn run_maestro_paths_with_inputs_progress(
    maestro: &Path,
    java: &Path,
    adb: &Path,
    flows: &[PathBuf],
    device_id: Option<&str>,
    input_environment: &[(String, Zeroizing<String>)],
    progress: Option<tokio::sync::mpsc::UnboundedSender<usize>>,
) -> Result<(), RunnerError> {
    let mut command = maestro_test_command(maestro, progress.is_some());
    command.arg("test").args(flows).arg("--no-ansi");
    if let Some(device_id) = device_id {
        command.args(["--udid", device_id]);
    }
    configure_environment(&mut command, java, adb, device_id);
    apply_input_environment(&mut command, input_environment);
    run_maestro_command_with_progress(command, "maestro", input_environment, progress).await
}

async fn run_maestro_ios_with_inputs(
    maestro: &Path,
    java: &Path,
    flow: &Path,
    simulator_id: &str,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    let flows = [flow.to_path_buf()];
    run_maestro_ios_paths_with_inputs(maestro, java, &flows, simulator_id, input_environment).await
}

async fn run_maestro_ios_paths_with_inputs(
    maestro: &Path,
    java: &Path,
    flows: &[PathBuf],
    simulator_id: &str,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    run_maestro_ios_paths_with_inputs_progress(
        maestro,
        java,
        flows,
        simulator_id,
        input_environment,
        None,
    )
    .await
}

async fn run_maestro_ios_paths_with_inputs_progress(
    maestro: &Path,
    java: &Path,
    flows: &[PathBuf],
    simulator_id: &str,
    input_environment: &[(String, Zeroizing<String>)],
    progress: Option<tokio::sync::mpsc::UnboundedSender<usize>>,
) -> Result<(), RunnerError> {
    let mut command = maestro_test_command(maestro, progress.is_some());
    command
        .arg("test")
        .args(flows)
        .args(["--no-ansi", "--udid", simulator_id]);
    configure_java_environment(&mut command, java);
    apply_input_environment(&mut command, input_environment);
    run_maestro_command_with_progress(
        command,
        "maestro-ios-simulator",
        input_environment,
        progress,
    )
    .await
}

fn maestro_test_command(maestro: &Path, live_progress: bool) -> Command {
    #[cfg(target_os = "macos")]
    if live_progress {
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "exec 3>&1; exec /usr/bin/script -q -F /dev/fd/3 \"$@\" >/dev/null",
                "reactor-maestro",
            ])
            .arg(maestro);
        return command;
    }
    let _ = live_progress;
    Command::new(maestro)
}

async fn run_maestro_command_with_progress(
    mut command: Command,
    label: &str,
    input_environment: &[(String, Zeroizing<String>)],
    progress: Option<tokio::sync::mpsc::UnboundedSender<usize>>,
) -> Result<(), RunnerError> {
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let child_id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RunnerError::CommandFailed {
            command: label.to_owned(),
            output: "Maestro stdout unavailable".to_owned(),
        })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| RunnerError::CommandFailed {
            command: label.to_owned(),
            output: "Maestro stderr unavailable".to_owned(),
        })?;
    let stdout_task = tokio::spawn(capture_maestro_stdout(stdout, progress));
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let status =
        if let Ok(status) = tokio::time::timeout(Duration::from_secs(120), child.wait()).await {
            status?
        } else {
            if let Some(child_id) = child_id {
                terminate_worker_group(child_id);
            }
            return Err(RunnerError::CommandFailed {
                command: label.to_owned(),
                output: "timed out after 120 seconds".to_owned(),
            });
        };
    let stdout = stdout_task
        .await
        .map_err(|error| RunnerError::CommandFailed {
            command: label.to_owned(),
            output: error.to_string(),
        })??;
    let stderr = stderr_task
        .await
        .map_err(|error| RunnerError::CommandFailed {
            command: label.to_owned(),
            output: error.to_string(),
        })??;
    if status.success() {
        return Ok(());
    }
    let mut captured = format!(
        "{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    for (_, value) in input_environment {
        if !value.is_empty() {
            captured = captured.replace(value.as_str(), "[REDACTED_INPUT]");
        }
    }
    Err(RunnerError::CommandFailed {
        command: label.to_owned(),
        output: captured,
    })
}

async fn capture_maestro_stdout(
    mut stdout: tokio::process::ChildStdout,
    progress: Option<tokio::sync::mpsc::UnboundedSender<usize>>,
) -> Result<Vec<u8>, std::io::Error> {
    let mut reader = BufReader::new(&mut stdout);
    let mut bytes = Vec::new();
    let mut line = Vec::new();
    let mut completed = 0;
    loop {
        line.clear();
        let count = reader.read_until(b'\n', &mut line).await?;
        if count == 0 {
            break;
        }
        if maestro_line_reports_completion(&line) {
            if let Some(sender) = &progress {
                let _ = sender.send(completed);
            }
            completed += 1;
        }
        bytes.extend_from_slice(&line);
    }
    Ok(bytes)
}

fn maestro_line_reports_completion(line: &[u8]) -> bool {
    line.windows(9).any(|window| window == b"COMPLETED")
        || line.windows(6).any(|window| window == b"FAILED")
}

fn apply_input_environment(
    command: &mut Command,
    input_environment: &[(String, Zeroizing<String>)],
) {
    for (key, value) in input_environment {
        command.env(key, value.as_str());
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_flashlight(
    flashlight: &Path,
    maestro: &Path,
    java: &Path,
    adb: &Path,
    setup_flow: &Path,
    measured_flow: &Path,
    teardown_flow: Option<&Path>,
    raw_path: &Path,
    app_id: &str,
    request: &AndroidRunRequest,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    let maestro_executable = quote_shell(&maestro.display().to_string());
    let test_command = format!(
        "{maestro_executable} test {} --no-ansi --udid {}",
        quote_shell(&measured_flow.display().to_string()),
        quote_shell(&request.device_id),
    );
    let before_each = format!(
        "{maestro_executable} test {} --no-ansi --udid {}",
        quote_shell(&setup_flow.display().to_string()),
        quote_shell(&request.device_id),
    );
    let mut command = Command::new(flashlight);
    let duration = request.duration_ms.to_string();
    let iteration_count = request.iteration_count.to_string();
    let title = format!("{} · {} · Reactor", request.framework, request.scenario);
    command.args([
        "test",
        "--bundleId",
        app_id,
        "--testCommand",
        &test_command,
        "--beforeEachCommand",
        &before_each,
        "--duration",
        &duration,
        "--iterationCount",
        &iteration_count,
        "--resultsTitle",
        &title,
        "--resultsFilePath",
        &raw_path.display().to_string(),
    ]);
    if let Some(teardown_flow) = teardown_flow {
        let after_each = format!(
            "{maestro_executable} test {} --no-ansi --udid {}",
            quote_shell(&teardown_flow.display().to_string()),
            quote_shell(&request.device_id),
        );
        command.args(["--afterEachCommand", &after_each]);
    }
    configure_environment(&mut command, java, adb, Some(&request.device_id));
    apply_input_environment(&mut command, input_environment);
    run_command_with_timeout_redacted(
        command,
        "flashlight",
        flashlight_timeout(request),
        input_environment,
    )
    .await
}

async fn run_flashlight_manual(
    flashlight: &Path,
    java: &Path,
    adb: &Path,
    raw_path: &Path,
    app_id: &str,
    request: &AndroidRunRequest,
    job_id: &str,
) -> Result<(), RunnerError> {
    let artifact_dir = raw_path.parent().ok_or_else(|| {
        RunnerError::InvalidDiagnosticPlan("manual recording artifact path is invalid".to_owned())
    })?;
    let stop_path = artifact_dir.join("manual-stop.request");
    let wait_script = artifact_dir.join("manual-recording-wait.sh");
    let tick_count = request.duration_ms.div_ceil(250).clamp(1, 1_200);
    let script = format!(
        "#!/bin/sh\ni=0\nwhile [ ! -f {} ] && [ \"$i\" -lt {tick_count} ]; do\n  sleep 0.25\n  i=$((i + 1))\ndone\n",
        quote_shell(&stop_path.display().to_string()),
    );
    fs::write(&wait_script, script).await?;
    register_artifact(
        &request.workspace,
        job_id,
        "manual_recording_control",
        &wait_script,
    )?;
    let test_command = format!("sh {}", quote_shell(&wait_script.display().to_string()));
    let mut command = Command::new(flashlight);
    let title = format!("{} · manual-diagnose · Reactor", request.framework);
    command.args([
        "test",
        "--bundleId",
        app_id,
        "--testCommand",
        &test_command,
        "--iterationCount",
        "1",
        "--maxRetries",
        "0",
        "--skipRestart",
        "--resultsTitle",
        &title,
        "--resultsFilePath",
        &raw_path.display().to_string(),
    ]);
    configure_environment(&mut command, java, adb, Some(&request.device_id));
    run_command_with_timeout(
        command,
        "flashlight-manual-recording",
        Duration::from_millis(request.duration_ms.saturating_add(120_000)),
    )
    .await
}

fn flashlight_timeout(request: &AndroidRunRequest) -> Duration {
    // Flashlight's duration only covers metric sampling. Maestro setup, Flow execution and
    // teardown happen around every iteration and need their own bounded allowance.
    let iterations = u64::from(request.iteration_count.max(1));
    let timeout_ms = request
        .duration_ms
        .saturating_mul(iterations)
        .saturating_add(60_000_u64.saturating_mul(iterations))
        .saturating_add(120_000);
    Duration::from_millis(timeout_ms)
}

fn configure_environment(command: &mut Command, java: &Path, adb: &Path, device_id: Option<&str>) {
    configure_java_environment(command, java);
    let adb_bin = adb.parent().unwrap_or_else(|| Path::new(""));
    let existing_path = command
        .as_std()
        .get_envs()
        .find_map(|(key, value)| (key == "PATH").then_some(value).flatten())
        .map(std::ffi::OsString::from)
        .unwrap_or_default();
    let mut paths = vec![adb_bin.to_path_buf()];
    paths.extend(std::env::split_paths(&existing_path));
    let joined = std::env::join_paths(paths).unwrap_or(existing_path);
    command.env("PATH", joined);
    if let Some(device_id) = device_id {
        command.env("ANDROID_SERIAL", device_id);
    }
}

fn configure_java_environment(command: &mut Command, java: &Path) {
    let java_home = java
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(""));
    let java_bin = java.parent().unwrap_or_else(|| Path::new(""));
    let existing_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![java_bin.to_path_buf()];
    paths.extend(std::env::split_paths(&existing_path));
    let joined = std::env::join_paths(paths).unwrap_or(existing_path);
    command
        .env("JAVA_HOME", java_home)
        .env("PATH", joined)
        .env("MAESTRO_CLI_NO_ANALYTICS", "1")
        .env("MAESTRO_CLI_ANALYSIS_NOTIFICATION_DISABLED", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

async fn run_command_with_timeout(
    command: Command,
    label: &str,
    timeout: Duration,
) -> Result<(), RunnerError> {
    run_command_with_timeout_redacted(command, label, timeout, &[]).await
}

async fn run_command_with_timeout_redacted(
    mut command: Command,
    label: &str,
    timeout: Duration,
    input_environment: &[(String, Zeroizing<String>)],
) -> Result<(), RunnerError> {
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn()?;
    let child_id = child.id();
    let output = if let Ok(output) = tokio::time::timeout(timeout, child.wait_with_output()).await {
        output?
    } else {
        if let Some(child_id) = child_id {
            terminate_worker_group(child_id);
        }
        return Err(RunnerError::CommandFailed {
            command: label.to_owned(),
            output: format!("timed out after {} seconds", timeout.as_secs()),
        });
    };
    if output.status.success() {
        Ok(())
    } else {
        let mut captured = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for (_, value) in input_environment {
            if !value.is_empty() {
                captured = captured.replace(value.as_str(), "[REDACTED_INPUT]");
            }
        }
        Err(RunnerError::CommandFailed {
            command: label.to_owned(),
            output: captured,
        })
    }
}

fn normalize_flashlight(
    raw: &Value,
    flow: &FlowLock,
    request: &AndroidRunRequest,
    job_id: &str,
    raw_path: &Path,
    target_metadata: &AndroidTargetMetadata,
) -> Result<NormalizedResult, RunnerError> {
    let physical = is_physical_android_target(&request.device_id, &BTreeMap::new());
    let raw_iterations = raw
        .get("iterations")
        .and_then(Value::as_array)
        .ok_or(RunnerError::InvalidFlashlight)?;
    let refresh_rate = raw
        .pointer("/specs/refreshRate")
        .and_then(Value::as_f64)
        .unwrap_or(60.0);
    let iterations = raw_iterations
        .iter()
        .map(|iteration| normalize_iteration(iteration, refresh_rate))
        .collect::<Vec<_>>();
    let summary = aggregate_iterations(&iterations);
    Ok(NormalizedResult {
        schema_version: 1,
        run_id: job_id.to_owned(),
        created_at: Utc::now(),
        framework: request.framework.clone(),
        platform: "android".to_owned(),
        scenario: request.scenario.clone(),
        adapter: "flashlight-android".to_owned(),
        build_mode: "release".to_owned(),
        flow_hash: flow.flow_hash.clone(),
        run_mode: reactor_protocol::RunMode::Benchmark,
        diagnostic_plan: None,
        build_identity: None,
        artifacts: vec![],
        framework_diagnostics: None,
        app_id: Some(flow.flow.app_id.clone()),
        app_version: target_metadata.app_version.clone(),
        device: DeviceMetadata {
            id: Some(request.device_id.clone()),
            name: target_metadata.name.clone(),
            os_version: target_metadata.os_version.clone(),
            refresh_rate,
            physical: Some(physical),
        },
        source: ResultSource {
            name: raw.get("name").and_then(Value::as_str).map(str::to_owned),
            status: raw.get("status").and_then(Value::as_str).map(str::to_owned),
            raw_file: Some(raw_path.display().to_string()),
            synthetic: false,
        },
        android_native: None,
        ios_native: None,
        iterations,
        summary,
        warnings: if physical {
            vec![]
        } else {
            vec!["Android 模拟器结果只适合同一主机的开发回归，不得与物理真机结果混排。".to_owned()]
        },
    })
}

fn normalize_iteration(iteration: &Value, refresh_rate: f64) -> IterationMetrics {
    let measures = iteration
        .get("measures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let fps = numbers(&measures, |measure| {
        measure.get("fps").and_then(Value::as_f64)
    });
    let ram = numbers(&measures, |measure| {
        measure.get("ram").and_then(Value::as_f64)
    });
    let cpu = numbers(&measures, |measure| cpu_sum(measure, None));
    let ui_cpu = numbers(&measures, |measure| {
        cpu_sum(measure, Some(&["UI Thread", "Main Thread", "1.ui"]))
    });
    let js_cpu = numbers(&measures, |measure| {
        cpu_sum(
            measure,
            Some(&[
                "mqt_js",
                "mqt_v_js",
                "com.facebook.react.JavaScript",
                "Lynx_JS",
                "LynxJS",
            ]),
        )
    });
    IterationMetrics {
        status: iteration
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN")
            .to_owned(),
        duration_ms: iteration.get("time").and_then(Value::as_f64).unwrap_or(0.0),
        sample_count: measures.len() as u64,
        fps_mean: mean(&fps),
        fps_p10: percentile(&fps, 0.1),
        low_fps_sample_pct: (!fps.is_empty()).then(|| {
            fps.iter()
                .filter(|value| **value < refresh_rate * 0.9)
                .count() as f64
                / fps.len() as f64
                * 100.0
        }),
        ram_mean_mb: mean(&ram),
        ram_peak_mb: ram.iter().copied().reduce(f64::max),
        cpu_mean_pct: mean(&cpu),
        ui_cpu_mean_pct: mean(&ui_cpu),
        js_cpu_mean_pct: mean(&js_cpu),
    }
}

fn numbers(measures: &[Value], get: impl Fn(&Value) -> Option<f64>) -> Vec<f64> {
    measures
        .iter()
        .filter_map(get)
        .filter(|value| value.is_finite())
        .collect()
}

fn cpu_sum(measure: &Value, names: Option<&[&str]>) -> Option<f64> {
    let entries = measure.pointer("/cpu/perName")?.as_object()?;
    Some(
        entries
            .iter()
            .filter(|(name, _)| names.is_none_or(|names| names.contains(&name.as_str())))
            .filter_map(|(_, value)| value.as_f64())
            .sum(),
    )
}

fn find_executable(root: &Path, names: &[&str]) -> Option<PathBuf> {
    if !root.exists() {
        return None;
    }
    WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_file()
                && names.iter().any(|name| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(name)
                })
        })
        .map(walkdir::DirEntry::into_path)
}

fn is_physical_android_target(id: &str, metadata: &BTreeMap<String, String>) -> bool {
    if id.starts_with("emulator-") {
        return false;
    }
    !metadata.values().any(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("emulator") || value.contains("sdk_gphone") || value.contains("generic")
    })
}

fn quote_shell(value: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_maestro_repeat_completions_back_to_the_parent_flow_step() {
        let flow = Flow {
            schema_version: 1,
            id: "progress-repeat".to_owned(),
            name: "Progress repeat".to_owned(),
            app_id: "com.example.app".to_owned(),
            platform: reactor_protocol::Platform::Android,
            intent: None,
            setup: vec![Step::LaunchApp],
            measured: vec![
                Step::Repeat {
                    times: 100,
                    steps: vec![
                        Step::Pause { duration_ms: 50 },
                        Step::Pause { duration_ms: 50 },
                    ],
                },
                Step::Swipe {
                    direction: reactor_protocol::SwipeDirection::Up,
                    duration_ms: 500,
                },
            ],
            teardown: vec![],
        };

        assert_eq!(maestro_progress_top_level_steps(&flow), [0, 1, 1, 1, 2]);
    }

    #[test]
    fn recognizes_tty_completion_lines_with_terminal_control_bytes() {
        assert!(maestro_line_reports_completion(
            b"Wait for animation...\x0c\x1b[2K COMPLETED\r\n"
        ));
        assert!(maestro_line_reports_completion(
            b"Tap on Sign in... FAILED\r\n"
        ));
        assert!(!maestro_line_reports_completion(b"Repeat 100 times...\r\n"));
    }

    #[test]
    fn single_step_replay_allows_launch_and_assertion_but_rejects_data_reset() {
        assert!(validate_explorer_single_step(&Step::LaunchApp).is_ok());
        assert!(
            validate_explorer_single_step(&Step::AssertVisible {
                target: reactor_protocol::Selector {
                    text: Some("List ready".to_owned()),
                    ..reactor_protocol::Selector::default()
                },
            })
            .is_ok()
        );
        assert!(validate_explorer_single_step(&Step::ResetAppState).is_err());
    }

    #[test]
    fn builds_bounded_android_explorer_input_commands() {
        let tap = android_explorer_input_args(
            "emulator-5554",
            &Step::Tap {
                target: reactor_protocol::Selector::default(),
            },
            Some(Coordinate { x: 719.6, y: 522.4 }),
            Some((1440.0, 3120.0)),
        )
        .unwrap();
        assert_eq!(
            tap,
            ["-s", "emulator-5554", "shell", "input", "tap", "720", "522"]
        );

        let swipe = android_explorer_input_args(
            "emulator-5554",
            &Step::Swipe {
                direction: SwipeDirection::Up,
                duration_ms: 500,
            },
            None,
            Some((1440.0, 3120.0)),
        )
        .unwrap();
        assert_eq!(
            swipe,
            [
                "-s",
                "emulator-5554",
                "shell",
                "input",
                "swipe",
                "720",
                "2340",
                "720",
                "780",
                "500",
            ]
        );

        assert!(
            android_explorer_input_args(
                "emulator-5554",
                &Step::Tap {
                    target: reactor_protocol::Selector::default(),
                },
                None,
                Some((1440.0, 3120.0)),
            )
            .is_err()
        );
    }

    #[test]
    fn android_fast_text_path_is_conservative() {
        assert!(android_fast_text_supported("test user+1@example.com"));
        assert!(android_fast_text_supported("123456"));
        assert!(!android_fast_text_supported("中文输入"));
        assert!(!android_fast_text_supported("line\nbreak"));
        assert!(!android_fast_text_supported(""));
    }

    #[test]
    fn generates_rfc6238_six_digit_totp_without_exposing_the_seed() {
        assert_eq!(
            generate_totp_at_counter(b"12345678901234567890", 1).as_deref(),
            Some("287082")
        );
    }

    #[test]
    fn resolves_prompt_inputs_only_from_explicit_interactive_values() {
        let binding = CompiledInputBinding {
            path: "setup[0]".to_owned(),
            environment_key: "MAESTRO_REACTOR_INPUT_SETUP_0_".to_owned(),
            value: InputValue::PromptRef(reactor_protocol::PromptInputReference {
                prompt_ref: "login.otp".to_owned(),
            }),
        };
        assert!(matches!(
            resolve_input_environment(std::slice::from_ref(&binding), None),
            Err(RunnerError::InteractiveInputRequired { .. })
        ));
        let prompts =
            BTreeMap::from([("login.otp".to_owned(), Zeroizing::new("123456".to_owned()))]);
        let resolved = resolve_input_environment(&[binding], Some(&prompts)).unwrap();
        assert_eq!(resolved[0].0, "MAESTRO_REACTOR_INPUT_SETUP_0_");
        assert_eq!(resolved[0].1.as_str(), "123456");
    }

    #[test]
    fn missing_variable_input_fails_without_falling_back_to_plaintext() {
        let binding = CompiledInputBinding {
            path: "setup[0]".to_owned(),
            environment_key: "MAESTRO_REACTOR_INPUT_SETUP_0_".to_owned(),
            value: InputValue::VariableRef(reactor_protocol::VariableInputReference {
                variable_ref: "REACTOR_TEST_VARIABLE_THAT_DOES_NOT_EXIST".to_owned(),
            }),
        };
        assert!(matches!(
            resolve_input_environment(&[binding], None),
            Err(RunnerError::MissingInputValue {
                kind: "variableRef",
                ..
            })
        ));
    }

    #[test]
    fn parses_versioned_perfetto_metric_fixture() {
        let fixture = include_str!("../../../tests/fixtures/perfetto-frame-metrics.csv");
        let metrics = parse_frame_metrics_csv(fixture).unwrap();
        assert_eq!(metrics.frame_count, 120);
        assert_eq!(metrics.frame_time_p95_ms, Some(22.0));
        assert_eq!(metrics.jank_frame_count, 7);
        assert_eq!(metrics.over_budget_frame_pct, Some(8.333_333));
    }

    #[test]
    fn parses_fixed_xctrace_exports_and_preserves_simulator_metadata() {
        let toc = include_str!("../../../tests/fixtures/xctrace-time-profiler-toc.xml");
        let profile = include_str!("../../../tests/fixtures/xctrace-time-profile.xml");
        let metrics = parse_xctrace_profile(toc, profile).unwrap();
        assert!((metrics.duration_ms - 2_500.0).abs() < f64::EPSILON);
        assert_eq!(metrics.cpu_sample_count, 3);
        assert!(
            metrics
                .cpu_mean_pct
                .is_some_and(|value| (value - 0.12).abs() < f64::EPSILON)
        );
        assert_eq!(metrics.xctrace_version, "26.0 (17C529)");
        assert_eq!(metrics.device_name.as_deref(), Some("iPhone 15 Pro"));
        assert_eq!(metrics.os_version.as_deref(), Some("17.5 (21F79)"));
    }

    #[test]
    fn rejects_incomplete_xctrace_toc() {
        let error = parse_xctrace_profile("<trace-toc/>", "<trace-query-result/>").unwrap_err();
        assert!(error.to_string().contains("missing <duration>"));
    }

    #[test]
    fn rejects_malformed_perfetto_metric_output() {
        let missing_row = parse_frame_metrics_csv("frame_count,p95\n").unwrap_err();
        assert!(missing_row.to_string().contains("missing metric row"));
        let bad_columns = parse_frame_metrics_csv("a,b,c,d,e,f,g\n1,2,3\n").unwrap_err();
        assert!(
            bad_columns
                .to_string()
                .contains("expected 7 frame metric columns")
        );
        let invalid_number =
            parse_frame_metrics_csv("a,b,c,d,e,f,g\nnot-a-count,2,3,4,5,6,7\n").unwrap_err();
        assert!(
            invalid_number
                .to_string()
                .contains("invalid integer metric")
        );
    }

    #[test]
    fn live_rn_summary_counts_repeated_component_renders() {
        let summary = summarize_live_rn_events(
            r#"{"kind":"component_render","timestampMs":1000,"payload":{"name":"ListRow"}}
{"kind":"component_render","timestampMs":3200,"payload":{"name":"ListRow"}}
{"kind":"component_render","timestampMs":4300,"payload":{"name":"Header"}}
{"kind":"react_profile","payload":{"id":"List","actualDuration":1.5}}
{"kind":"react_profile","payload":{"id":"Header","actualDuration":3.25}}"#,
        );
        assert_eq!(summary["componentRenderCount"], 3);
        assert_eq!(summary["duplicateComponentRenderCount"], 1);
        assert_eq!(summary["componentRenderWindowStartMs"], 1000);
        assert_eq!(summary["componentRenderWindowEndMs"], 4300);
        assert_eq!(summary["componentRenderWindowDurationMs"], 3300);
        assert_eq!(summary["profileCommitCount"], 2);
        assert_eq!(summary["slowestCommitName"], "Header");
        assert_eq!(summary["slowestCommitMs"], 3.25);
        assert_eq!(summary["components"][0]["name"], "ListRow");
        assert_eq!(summary["components"][0]["renderCount"], 2);
        assert_eq!(summary["components"][0]["duplicateRenderCount"], 1);
        assert_eq!(summary["components"][1]["name"], "Header");
        assert_eq!(summary["components"][1]["profileCommitCount"], 1);
        assert_eq!(summary["components"][1]["maxCommitMs"], 3.25);
    }

    #[tokio::test]
    #[ignore = "requires the Reactor-managed Trace Processor"]
    async fn replays_fixed_perfetto_trace_and_rejects_corrupt_trace() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let trace_processor = resolve_tools(&workspace)
            .trace_processor
            .expect("run `reactor setup` before the managed trace replay test");
        let trace = workspace.join("tests/fixtures/perfetto-react-native-list.pftrace");
        let metrics = parse_perfetto_frames(
            &trace_processor,
            &trace,
            "com.reactor.bench.reactnative",
            60.0,
        )
        .await
        .unwrap();
        assert_eq!(metrics.frame_count, 498);
        assert_eq!(metrics.jank_frame_count, 98);
        assert_eq!(metrics.frame_time_p95_ms, Some(20.140_619));
        assert_eq!(metrics.frame_time_p99_ms, Some(45.129_122));
        assert_eq!(metrics.over_budget_frame_pct, Some(36.546_185));

        let corrupt = std::env::temp_dir().join(format!(
            "reactor-corrupt-perfetto-{}.pftrace",
            uuid::Uuid::new_v4()
        ));
        fs::write(&corrupt, b"not a perfetto trace").await.unwrap();
        let error = parse_perfetto_frames(
            &trace_processor,
            &corrupt,
            "com.reactor.bench.reactnative",
            60.0,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("trace-processor"));
        fs::remove_file(corrupt).await.unwrap();
    }

    #[test]
    fn parses_android_native_auxiliary_metrics() {
        assert_eq!(
            parse_startup_total_time("Status: ok\nTotalTime: 278\n"),
            Some(278.0)
        );
        assert_eq!(
            parse_thermal_status("HAL Ready: true\nThermal Status: 3\n"),
            Some(3)
        );
        assert_eq!(
            parse_memory_pss_mb("TOTAL PSS:    52915 TOTAL RSS: 135848"),
            Some(52_915.0 / 1024.0)
        );
        assert_eq!(
            parse_android_app_version("versionCode=42 minSdk=23 targetSdk=35\nversionName=1.2.3\n")
                .as_deref(),
            Some("1.2.3 (42)")
        );
        assert_eq!(parse_android_app_version("versionName=null\n"), None);
        assert_eq!(
            parse_df_available_bytes(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev 100000 1 65536 1% /data\n"
            ),
            Some(65_536 * 1024)
        );
    }

    #[test]
    fn parses_android_memory_checkpoint_breakdown() {
        let checkpoint = parse_memory_checkpoint(
            "Java Heap: 12288 0 0\nNative Heap: 24576 0 0\nTOTAL PSS: 65536 TOTAL RSS: 98304\n",
            "cycle",
            4,
            1_250,
        );
        assert_eq!(checkpoint.kind, "cycle");
        assert_eq!(checkpoint.cycle, 4);
        assert_eq!(checkpoint.elapsed_ms, 1_250);
        assert_eq!(checkpoint.pss_mb, Some(64.0));
        assert_eq!(checkpoint.rss_mb, Some(96.0));
        assert_eq!(checkpoint.java_heap_mb, Some(12.0));
        assert_eq!(checkpoint.native_heap_mb, Some(24.0));
        assert_eq!(
            parse_android_cpu_pct(
                "  3.7% 1234/com.reactor.bench.reactnative: 2.4% user + 1.3% kernel\n",
                "com.reactor.bench.reactnative",
            ),
            Some(3.7)
        );
    }

    #[test]
    fn leak_analysis_distinguishes_sustained_growth_from_stable_memory() {
        let plan = AndroidLeakTestPlan {
            cycles: 10,
            checkpoint_every: 2,
            warmup_cycles: 2,
            stabilization_ms: 0,
            cooldown_ms: 0,
            threshold_mb_per_cycle: 0.25,
        };
        let points = |values: &[f64]| {
            values
                .iter()
                .enumerate()
                .map(|(index, value)| AndroidMemoryCheckpoint {
                    kind: "cycle".to_owned(),
                    cycle: u32::try_from((index + 1) * 2).unwrap(),
                    elapsed_ms: u64::try_from(index).unwrap() * 100,
                    cpu_pct: None,
                    pss_mb: Some(*value),
                    rss_mb: None,
                    java_heap_mb: None,
                    native_heap_mb: None,
                })
                .chain(std::iter::once(AndroidMemoryCheckpoint {
                    kind: "cooldown".to_owned(),
                    cycle: 10,
                    elapsed_ms: 600,
                    cpu_pct: None,
                    pss_mb: Some(*values.last().unwrap()),
                    rss_mb: None,
                    java_heap_mb: None,
                    native_heap_mb: None,
                }))
                .collect::<Vec<_>>()
        };
        let leaking = analyze_memory_leak(&plan, points(&[50.0, 52.0, 54.0, 56.0, 58.0]));
        assert_eq!(leaking.verdict, "suspected_leak");
        assert!(leaking.slope_mb_per_cycle.is_some_and(|slope| slope > 0.9));

        let stable = analyze_memory_leak(&plan, points(&[50.0, 50.2, 49.9, 50.1, 50.0]));
        assert_eq!(stable.verdict, "stable");
    }

    #[test]
    fn retained_rn_objects_confirm_an_existing_growth_trend() {
        let plan = AndroidLeakTestPlan {
            cycles: 6,
            checkpoint_every: 2,
            warmup_cycles: 2,
            stabilization_ms: 0,
            cooldown_ms: 0,
            threshold_mb_per_cycle: 0.25,
        };
        let checkpoints = vec![
            AndroidMemoryCheckpoint {
                kind: "cycle".to_owned(),
                cycle: 2,
                elapsed_ms: 0,
                cpu_pct: None,
                pss_mb: Some(60.0),
                rss_mb: None,
                java_heap_mb: None,
                native_heap_mb: None,
            },
            AndroidMemoryCheckpoint {
                kind: "cycle".to_owned(),
                cycle: 4,
                elapsed_ms: 100,
                cpu_pct: None,
                pss_mb: Some(62.0),
                rss_mb: None,
                java_heap_mb: None,
                native_heap_mb: None,
            },
            AndroidMemoryCheckpoint {
                kind: "cycle".to_owned(),
                cycle: 6,
                elapsed_ms: 200,
                cpu_pct: None,
                pss_mb: Some(64.0),
                rss_mb: None,
                java_heap_mb: None,
                native_heap_mb: None,
            },
        ];
        let mut report = analyze_memory_leak(&plan, checkpoints);
        assert_eq!(report.verdict, "insufficient_evidence");
        let diagnostics = ReactNativeDiagnosticsSummary {
            schema_version: 1,
            collector: "reactor-rn-sdk-v1".to_owned(),
            benchmark_mode: Some("memory-retention-fault".to_owned()),
            event_file: "rn-diagnostics.ndjson".to_owned(),
            event_count: 12,
            component_names: vec![],
            component_render_count: 0,
            component_tree_commit_count: 0,
            profile_commit_count: 0,
            console_event_count: 0,
            network_event_count: 0,
            hermes_heap_sample_count: 0,
            allocated_object_count: 6,
            retained_object_count: 6,
            retained_bytes: 6 * 1024 * 1024,
            profile_file: None,
            hermes_heap_stats_file: None,
            hermes_heap_snapshot_file: None,
            java_heap_dump_file: None,
            recent_events: vec![],
            warnings: vec![],
        };
        reconcile_memory_leak_with_rn_diagnostics(&mut report, Some(&diagnostics));
        assert_eq!(report.verdict, "confirmed_leak");
        assert_eq!(report.confidence, "medium");
        assert_eq!(report.managed_retained_object_count, Some(6));
        assert_eq!(report.managed_retained_bytes, Some(6 * 1024 * 1024));
    }

    #[test]
    fn enriches_devtools_profile_with_managed_component_source_locations() {
        let profile = serde_json::json!({
            "version": 5,
            "dataForRoots": [{
                "rootID": 1,
                "snapshots": [[42, {
                    "id": 42,
                    "displayName": "MemoryScenario",
                    "children": []
                }]],
                "commitData": []
            }]
        });
        let events = vec![ReactNativeDiagnosticEvent {
            timestamp_ms: 1,
            kind: "component_render".to_owned(),
            payload: serde_json::json!({
                "name": "MemoryScenario",
                "sourceFile": "demos/react-native/App.tsx",
                "sourceLine": 170,
                "sourceColumn": 1
            }),
        }];

        let enriched = enrich_react_profile_source_locations(profile, &events);

        assert_eq!(
            enriched.pointer("/sourceLocations/42"),
            Some(&serde_json::json!({
                "file": "demos/react-native/App.tsx",
                "line": 170,
                "column": 1
            }))
        );
    }

    #[test]
    fn inspector_screenshot_requires_a_bounded_png() {
        let valid = b"\x89PNG\r\n\x1a\nsmall".to_vec();
        assert_eq!(
            validate_png_bytes(valid.clone(), "fixture", 32).unwrap(),
            valid
        );
        assert!(validate_png_bytes(b"not-png".to_vec(), "fixture", 32).is_err());
        assert!(validate_png_bytes(b"\x89PNG\r\n\x1a\nlarge".to_vec(), "fixture", 8).is_err());
    }

    #[test]
    fn rejects_insufficient_trace_space_with_exact_evidence() {
        let error = require_available_space("emulator-5554:/trace", 1024, 2048).unwrap_err();
        assert!(matches!(
            &error,
            RunnerError::InsufficientSpace {
                available_bytes: 1024,
                required_bytes: 2048,
                ..
            }
        ));
        assert!(error.to_string().contains("emulator-5554:/trace"));
        require_available_space("fixture", 2048, 2048).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reports_android_disconnect_as_a_command_failure() {
        let error = android_shell_text(
            Path::new("/usr/bin/false"),
            "disconnected-device",
            &["dumpsys", "meminfo"],
            "meminfo",
        )
        .await
        .unwrap_err();
        assert!(matches!(&error, RunnerError::CommandFailed { .. }));
        assert!(error.to_string().contains("meminfo"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_failures_redact_resolved_flow_inputs() {
        let secret = Zeroizing::new("reactor-private-fixture".to_owned());
        let inputs = vec![("MAESTRO_REACTOR_INPUT_SETUP_0_".to_owned(), secret)];
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "printf '%s' \"$MAESTRO_REACTOR_INPUT_SETUP_0_\" >&2; exit 1",
            ])
            .env("MAESTRO_REACTOR_INPUT_SETUP_0_", inputs[0].1.as_str());
        let error = run_command_with_timeout_redacted(
            command,
            "redaction-fixture",
            Duration::from_secs(2),
            &inputs,
        )
        .await
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("[REDACTED_INPUT]"));
        assert!(!message.contains("reactor-private-fixture"));
    }

    #[test]
    fn perfetto_config_enables_frame_timeline_for_target_app() {
        let config = perfetto_config("com.reactor.fixture", 12_345);
        assert!(config.contains("android.surfaceflinger.frametimeline"));
        assert!(config.contains("atrace_apps: \"com.reactor.fixture\""));
        assert!(config.contains("duration_ms: 12345"));
    }

    #[test]
    fn heapprofd_config_and_retention_csv_are_versioned_and_parseable() {
        let config = heapprofd_config("com.reactor.fixture", 45_000);
        assert!(config.contains("android.heapprofd"));
        assert!(config.contains("process_cmdline: \"com.reactor.fixture\""));
        assert!(config.contains("duration_ms: 45000"));
        assert_eq!(
            parse_heapprofd_csv("retained_bytes,retained_allocation_count\n1048576,12\n").unwrap(),
            (1_048_576, 12)
        );
    }

    #[tokio::test]
    async fn measurement_audit_proves_zero_model_calls() {
        let directory =
            std::env::temp_dir().join(format!("reactor-ai-audit-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).await.unwrap();
        let path = write_measurement_ai_audit(&directory, "job-fixture")
            .await
            .unwrap();
        let payload: Value = serde_json::from_slice(&fs::read(path).await.unwrap()).unwrap();
        assert_eq!(
            payload
                .get("measurementWindowModelCalls")
                .and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            payload.get("aiProviderLinked").and_then(Value::as_bool),
            Some(false)
        );
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_command_stops_a_hung_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60"]);
        let started = std::time::Instant::now();
        let error = run_command_with_timeout(command, "hung-fixture", Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn marker_foundation_is_truthful_about_opaque_step_execution() {
        let flow = Flow {
            schema_version: 1,
            id: "markers".to_owned(),
            name: "Markers".to_owned(),
            app_id: "com.example".to_owned(),
            platform: reactor_protocol::Platform::Android,
            intent: None,
            setup: vec![Step::LaunchApp],
            measured: vec![Step::Pause { duration_ms: 1 }],
            teardown: vec![],
        };
        let foundation = flow_marker_foundation(&flow);
        assert_eq!(foundation.clock, "host_monotonic");
        assert_eq!(foundation.iteration_boundaries, "unavailable");
        assert_eq!(foundation.step_boundaries, "unavailable");
        assert!(foundation.uncertainty_ms.is_none());
        assert!(!foundation.steps.is_empty());
        assert!(foundation.steps[0].id.starts_with("flow-step:"));
    }

    #[test]
    fn validates_android_package_ids_before_using_components_or_paths() {
        for valid in ["com.example.app", "io_reactor.fixture2"] {
            validate_android_package_id(valid).unwrap();
        }
        for invalid in [
            "com",
            ".com.example",
            "com.example/Receiver",
            "com.example-app",
            "1com.example",
            "com..example",
        ] {
            assert!(validate_android_package_id(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn diagnostic_ingress_enforces_cumulative_duration_and_plan_consistency() {
        let mut request = AndroidRunRequest {
            workspace: PathBuf::from("fixture"),
            flow_lock: PathBuf::from("flow.lock.json"),
            framework: "react-native".to_owned(),
            scenario: "list".to_owned(),
            device_id: "emulator-5554".to_owned(),
            duration_ms: 18_000,
            iteration_count: 3,
            run_mode: RunMode::Diagnose,
            diagnostic_plan: Some(DiagnosticPlanV1 {
                schema_version: 1,
                mode: reactor_protocol::DiagnosticCaptureMode::InBand,
                collectors: vec![reactor_protocol::DiagnosticCollectorPlanV1 {
                    collector: "hermes-cpu".to_owned(),
                    required: true,
                }],
                resource_limits: reactor_protocol::DiagnosticResourceLimitsV1 {
                    max_duration_ms: 54_000,
                    max_artifact_bytes: 1024,
                    max_events: 100,
                    max_samples: 100,
                },
            }),
            leak_test: None,
            manual_session: false,
        };
        validate_android_request(&request).unwrap();
        request.iteration_count = 4;
        assert!(
            validate_android_request(&request)
                .unwrap_err()
                .to_string()
                .contains("cumulative duration")
        );
        request.run_mode = RunMode::Benchmark;
        assert!(validate_android_request(&request).is_err());
    }

    #[test]
    fn artifact_refs_are_normalized_and_cumulative_bytes_are_bounded() {
        let directory =
            std::env::temp_dir().join(format!("reactor-artifact-ref-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.json");
        let second = directory.join("nested/second.json");
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, b"1234").unwrap();
        std::fs::write(&second, b"5678").unwrap();
        assert_eq!(
            result_relative_artifact_path(&directory, &second).unwrap(),
            "nested/second.json"
        );
        assert!(
            enforce_diagnostic_artifacts(&directory, [first.clone(), second.clone()], 8).is_ok()
        );
        assert!(enforce_diagnostic_artifacts(&directory, [first, second], 7).is_err());
        assert!(result_relative_artifact_path(&directory, directory.parent().unwrap()).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn available_hermes_trace_samples_obey_sample_limit() {
        let path = std::env::temp_dir().join(format!(
            "reactor-hermes-samples-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, br#"{"traceEvents":[{},{}]}"#).unwrap();
        enforce_json_sample_limit(&path, 2).unwrap();
        assert!(enforce_json_sample_limit(&path, 1).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn flashlight_timeout_includes_per_iteration_automation_overhead() {
        let request = AndroidRunRequest {
            workspace: PathBuf::from("fixture"),
            flow_lock: PathBuf::from("flow.lock.json"),
            framework: "react-native".to_owned(),
            scenario: "list".to_owned(),
            device_id: "emulator-5554".to_owned(),
            duration_ms: 18_000,
            iteration_count: 10,
            run_mode: RunMode::Benchmark,
            diagnostic_plan: None,
            leak_test: None,
            manual_session: false,
        };
        assert_eq!(flashlight_timeout(&request), Duration::from_secs(900));
    }

    #[test]
    fn parses_adb_device_rows() {
        let devices = parse_adb_devices(
            "List of devices attached\nABC device product:foo model:Pixel_8 device:husky\n",
        );
        assert_eq!(devices[0].id, "ABC");
        assert_eq!(devices[0].name.as_deref(), Some("Pixel_8"));
        assert!(devices[0].physical);
    }

    #[test]
    fn parses_only_a_resolved_android_launcher_component() {
        assert_eq!(
            parse_android_launcher_component(
                "priority=0 preferredOrder=0 match=0x108000 specificIndex=-1 isDefault=true\ncom.example/.MainActivity\n"
            ),
            Some("com.example/.MainActivity".to_owned())
        );
        assert_eq!(
            parse_android_launcher_component("No activity found\n"),
            None
        );
        assert_eq!(
            parse_android_launcher_component("com.example / .MainActivity\n"),
            None
        );
    }

    #[test]
    fn marks_emulator_targets() {
        let devices = parse_adb_devices(
            "List of devices attached\nemulator-5554 device product:sdk_gphone model:sdk_gphone64_arm64 device:emu64a\n",
        );
        assert!(!devices[0].physical);
    }

    #[test]
    fn parses_only_booted_ios_simulators() {
        let value = serde_json::json!({
            "devices": {
                "com.apple.CoreSimulator.SimRuntime.iOS-26-2": [
                    {"udid": "BOOTED", "name": "iPhone 17", "state": "Booted", "deviceTypeIdentifier": "iphone"},
                    {"udid": "OFF", "name": "iPhone 16", "state": "Shutdown"}
                ]
            }
        });
        let devices = parse_ios_simulators(&value).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].platform, "ios");
        assert!(!devices[0].physical);
        assert_eq!(
            devices[0].metadata.get("osVersion").map(String::as_str),
            Some("26.2")
        );
    }

    #[test]
    fn recovers_orphaned_job_and_preserves_valid_artifact() {
        let workspace =
            std::env::temp_dir().join(format!("reactor-runner-{}", uuid::Uuid::new_v4()));
        let store = open_store(&workspace).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "mode": "recovery-test" }))
            .unwrap();
        let artifact = workspace.join("partial.json");
        std::fs::write(&artifact, b"partial evidence").unwrap();
        store
            .register_artifact(&job.id, "partial", &artifact)
            .unwrap();

        let recovered = recover_orphaned_jobs(&workspace).unwrap();

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, JobState::Failed);
        assert!(
            verify_job_artifacts(&workspace, &job.id)
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn cancellation_is_idempotent() {
        let workspace =
            std::env::temp_dir().join(format!("reactor-cancel-{}", uuid::Uuid::new_v4()));
        let job = open_store(&workspace)
            .unwrap()
            .create_job(&serde_json::json!({ "mode": "cancel-test" }))
            .unwrap();
        let first = cancel_persisted_job(&workspace, &job.id).unwrap();
        let second = cancel_persisted_job(&workspace, &job.id).unwrap();
        assert_eq!(first.state, JobState::Cancelled);
        assert_eq!(second.state, JobState::Cancelled);
        assert_eq!(
            get_job(&workspace, &job.id, 0).unwrap().1.len(),
            2,
            "a repeated cancel must not append another terminal event"
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn manual_stop_is_graceful_idempotent_and_rejects_regular_jobs() {
        let workspace =
            std::env::temp_dir().join(format!("reactor-manual-stop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let store = open_store(&workspace).unwrap();
        let manual = store
            .create_job(&serde_json::json!({ "manualSession": true }))
            .unwrap();
        let first = request_android_manual_stop(&workspace, &manual.id).unwrap();
        let event_count_after_first = store.events_after(&manual.id, 0).unwrap().len();
        let second = request_android_manual_stop(&workspace, &manual.id).unwrap();
        assert_eq!(first.state, JobState::Queued);
        assert_eq!(second.state, JobState::Queued);
        assert!(
            workspace
                .join("results/runs")
                .join(&manual.id)
                .join("manual-stop.request")
                .is_file()
        );
        assert_eq!(
            store.events_after(&manual.id, 0).unwrap().len(),
            event_count_after_first
        );

        let regular = store
            .create_job(&serde_json::json!({ "manualSession": false }))
            .unwrap();
        assert!(request_android_manual_stop(&workspace, &regular.id).is_err());
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a Reactor-managed Android target and explicit local workspace/Flow lock"]
    async fn manual_android_recording_stops_and_persists_evidence() {
        let workspace = PathBuf::from(
            std::env::var("REACTOR_MANUAL_WORKSPACE")
                .expect("set REACTOR_MANUAL_WORKSPACE for the managed runtime"),
        );
        let flow_lock = PathBuf::from(
            std::env::var("REACTOR_MANUAL_FLOW_LOCK")
                .expect("set REACTOR_MANUAL_FLOW_LOCK for a verified Android Flow"),
        );
        let request = AndroidRunRequest {
            workspace: workspace.clone(),
            flow_lock,
            framework: "react-native".to_owned(),
            scenario: "manual-diagnose".to_owned(),
            device_id: "emulator-5554".to_owned(),
            duration_ms: 30_000,
            iteration_count: 1,
            run_mode: RunMode::Diagnose,
            diagnostic_plan: Some(DiagnosticPlanV1 {
                schema_version: 1,
                mode: reactor_protocol::DiagnosticCaptureMode::InBand,
                collectors: vec![reactor_protocol::DiagnosticCollectorPlanV1 {
                    collector: "hermes-cpu".to_owned(),
                    required: false,
                }],
                resource_limits: reactor_protocol::DiagnosticResourceLimitsV1 {
                    max_duration_ms: 30_000,
                    max_artifact_bytes: 256 * 1024 * 1024,
                    max_events: 500_000,
                    max_samples: 2_000_000,
                },
            }),
            leak_test: None,
            manual_session: true,
        };
        let job = enqueue_android(&request).unwrap();
        let execute_request = request.clone();
        let job_id = job.id.clone();
        let execute_job_id = job_id.clone();
        let execution =
            tokio::spawn(
                async move { execute_android_job(&execute_request, &execute_job_id).await },
            );
        for _ in 0..120 {
            let (current, _) = get_job(&workspace, &job_id, 0).unwrap();
            if current.state == JobState::Measuring {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        request_android_manual_stop(&workspace, &job_id).unwrap();
        let (completed, result) = execution.await.unwrap().unwrap();
        assert_eq!(completed.state, JobState::Completed);
        assert_eq!(result.scenario, "manual-diagnose");
        let artifacts = workspace.join("results/runs").join(&job_id);
        assert!(artifacts.join("flashlight.json").is_file());
        assert!(artifacts.join("perfetto.pftrace").is_file());
        assert!(artifacts.join("result.json").is_file());
    }
}
