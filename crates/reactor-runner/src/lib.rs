#![allow(clippy::cast_precision_loss)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use chrono::Utc;
use reactor_core::{aggregate_iterations, compile_maestro, mean, percentile, render_html_report};
use reactor_protocol::{
    AndroidNativeMetrics, DeviceMetadata, Flow, FlowLock, FlowTrialEvidence, FlowValidationError,
    IosMetricAvailability, IosNativeMetrics, IterationMetrics, NormalizedResult, ResultSource,
    Step, TrialMode, canonical_flow_hash,
};
use reactor_store::{ArtifactIssue, Job, JobEvent, JobState, Store};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt, process::Command};
use walkdir::WalkDir;

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

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
            let issues = store.verify_artifacts(&job.id)?;
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
    Ok(open_store(workspace)?.verify_artifacts(job_id)?)
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

/// Persists an Android job before a detached worker starts it.
///
/// # Errors
///
/// Returns an error when the queue database cannot be updated.
pub fn enqueue_android(request: &AndroidRunRequest) -> Result<Job, RunnerError> {
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
    let adb = adb.to_string_lossy().into_owned();
    command_text(
        &adb,
        &[
            "-s",
            device_id,
            "shell",
            "monkey",
            "-p",
            app_id,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
        "launch Android app for UI context",
        Duration::from_secs(15),
    )
    .await?;
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

/// Executes one reviewed Flow Explorer step against the current screen without launching or
/// resetting the application. The ephemeral Maestro document is removed after execution and is
/// never indexed as benchmark evidence.
///
/// # Errors
///
/// Returns an error when the step is not an interactive recorder action, validation fails, the
/// managed automation runtime is unavailable, or the device command fails.
pub async fn execute_explorer_step(
    workspace: &Path,
    platform: reactor_protocol::Platform,
    device_id: &str,
    app_id: &str,
    step: Step,
) -> Result<(), RunnerError> {
    if !matches!(
        &step,
        Step::Tap { .. } | Step::InputText { .. } | Step::Swipe { .. } | Step::Pause { .. }
    ) {
        return Err(RunnerError::CommandFailed {
            command: "Flow Explorer interaction".to_owned(),
            output: "only tap, input_text, swipe, and pause are interactive recorder actions"
                .to_owned(),
        });
    }
    let flow = Flow {
        schema_version: 1,
        id: "flow-explorer-step".to_owned(),
        name: "Flow Explorer reviewed step".to_owned(),
        app_id: app_id.to_owned(),
        platform,
        intent: None,
        setup: vec![],
        measured: vec![step],
        teardown: vec![],
    };
    let compiled = compile_maestro(&flow)?;
    let tools = resolve_tools(workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let directory = workspace
        .join(".reactor/runtime/explorer")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory).await?;
    let path = directory.join("step.yaml");
    fs::write(&path, compiled.measured).await?;
    let result = match platform {
        reactor_protocol::Platform::Android => {
            let adb = tools.adb.ok_or(RunnerError::MissingTool("adb"))?;
            run_maestro(&maestro, &java, &adb, &path, Some(device_id)).await
        }
        reactor_protocol::Platform::Ios => run_maestro_ios(&maestro, &java, &path, device_id).await,
    };
    let _ = fs::remove_dir_all(&directory).await;
    result
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
    let remote_trace = format!("/data/misc/perfetto-traces/reactor-{job_id}.pftrace");
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
    stdin
        .write_all(perfetto_config(app_id, duration_ms).as_bytes())
        .await?;
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
    let resolved = android_shell_text(
        adb,
        device_id,
        &["cmd", "package", "resolve-activity", "--brief", app_id],
        "resolve-activity",
    )
    .await?;
    let Some(component) = resolved
        .lines()
        .rev()
        .find(|line| line.contains('/') && !line.contains(' '))
    else {
        return Ok(None);
    };
    android_shell_text(adb, device_id, &["am", "force-stop", app_id], "force-stop").await?;
    let output = android_shell_text(
        adb,
        device_id,
        &["am", "start", "-W", "-n", component],
        "am-start",
    )
    .await?;
    Ok(parse_startup_total_time(&output))
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
    if let Err(error) = &result
        && let Ok(store) = open_store(&request.workspace)
    {
        let _ = store.fail(job_id, &error.to_string());
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
    if !lock.has_android_trial(&request.device_id) {
        return Err(RunnerError::MissingAndroidTrial(request.device_id.clone()));
    }
    let compiled = compile_maestro(&lock.flow)?;
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
        "执行非计分预热",
        None,
    )?;
    if !lock.flow.setup.is_empty() {
        run_maestro(&maestro, &java, &adb, &setup_path, Some(&request.device_id)).await?;
    }
    run_maestro(
        &maestro,
        &java,
        &adb,
        &measured_path,
        Some(&request.device_id),
    )
    .await?;
    if !lock.flow.teardown.is_empty() {
        run_maestro(
            &maestro,
            &java,
            &adb,
            &teardown_path,
            Some(&request.device_id),
        )
        .await?;
    }

    transition_job(
        &request.workspace,
        job_id,
        JobState::Measuring,
        "执行锁定 Flow 并采集原生指标",
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
    let flashlight_result = run_flashlight(
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
    )
    .await;
    let perfetto_result = stop_perfetto(&adb, &request.device_id, &perfetto, &perfetto_path).await;
    if perfetto_path.is_file() {
        register_artifact(&request.workspace, job_id, "perfetto_trace", &perfetto_path)?;
    }
    if let Err(error) = flashlight_result {
        let _ = perfetto_result;
        return Err(error);
    }
    perfetto_result?;
    register_artifact(&request.workspace, job_id, "flashlight_raw", &raw_path)?;
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
    let tools = resolve_tools(&request.workspace);
    let maestro = tools.maestro.ok_or(RunnerError::MissingTool("maestro"))?;
    let java = tools.java.ok_or(RunnerError::MissingTool("java"))?;
    let executable = ios_app_executable_name(&request.device_id, &lock.flow.app_id).await?;

    let artifact_dir = request.workspace.join("results/runs").join(job_id);
    fs::create_dir_all(&artifact_dir).await?;
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
        run_maestro_ios(&maestro, &java, &setup_path, &request.device_id).await?;
    }
    run_maestro_ios(&maestro, &java, &measured_path, &request.device_id).await?;
    if !lock.flow.teardown.is_empty() {
        run_maestro_ios(&maestro, &java, &teardown_path, &request.device_id).await?;
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
    let measured_result =
        run_maestro_ios(&maestro, &java, &measured_path, &request.device_id).await;
    let trace_result = stop_xctrace(&mut xctrace, &trace_path).await;
    measured_result?;
    trace_result?;
    if !lock.flow.teardown.is_empty() {
        run_maestro_ios(&maestro, &java, &teardown_path, &request.device_id).await?;
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
) -> Result<FlowTrialEvidence, RunnerError> {
    let flow_hash = canonical_flow_hash(flow)?;
    let compiled = compile_maestro(flow)?;
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
        run_maestro(&maestro, &java, &adb, &setup_path, Some(device_id)).await?;
    }
    let measured = run_maestro(&maestro, &java, &adb, &measured_path, Some(device_id)).await;
    measured?;
    if let Ok(tree) = capture_android_current_ui_tree(workspace, device_id).await {
        fs::write(artifact_dir.join("destination-ui-tree.xml"), tree).await?;
    }
    let teardown = if flow.teardown.is_empty() {
        Ok(())
    } else {
        run_maestro(&maestro, &java, &adb, &teardown_path, Some(device_id)).await
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
) -> Result<FlowTrialEvidence, RunnerError> {
    let flow_hash = canonical_flow_hash(flow)?;
    let compiled = compile_maestro(flow)?;
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
        run_maestro_ios(&maestro, &java, &setup_path, simulator_id).await?;
    }
    let measured = run_maestro_ios(&maestro, &java, &measured_path, simulator_id).await;
    measured?;
    if let Ok(tree) = capture_ios_current_ui_tree(workspace, simulator_id).await {
        fs::write(artifact_dir.join("destination-ui-tree.csv"), tree).await?;
    }
    let teardown = if flow.teardown.is_empty() {
        Ok(())
    } else {
        run_maestro_ios(&maestro, &java, &teardown_path, simulator_id).await
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

async fn run_maestro(
    maestro: &Path,
    java: &Path,
    adb: &Path,
    flow: &Path,
    device_id: Option<&str>,
) -> Result<(), RunnerError> {
    let mut command = Command::new(maestro);
    command.args(["test", &flow.display().to_string(), "--no-ansi"]);
    if let Some(device_id) = device_id {
        command.args(["--udid", device_id]);
    }
    configure_environment(&mut command, java, adb, device_id);
    run_command_with_timeout(command, "maestro", Duration::from_secs(120)).await
}

async fn run_maestro_ios(
    maestro: &Path,
    java: &Path,
    flow: &Path,
    simulator_id: &str,
) -> Result<(), RunnerError> {
    let mut command = Command::new(maestro);
    command.args([
        "test",
        &flow.display().to_string(),
        "--no-ansi",
        "--udid",
        simulator_id,
    ]);
    configure_java_environment(&mut command, java);
    run_command_with_timeout(command, "maestro-ios-simulator", Duration::from_secs(120)).await
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
    run_command_with_timeout(command, "flashlight", flashlight_timeout(request)).await
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
    mut command: Command,
    label: &str,
    timeout: Duration,
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
        Err(RunnerError::CommandFailed {
            command: label.to_owned(),
            output: format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
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

    #[test]
    fn perfetto_config_enables_frame_timeline_for_target_app() {
        let config = perfetto_config("com.reactor.fixture", 12_345);
        assert!(config.contains("android.surfaceflinger.frametimeline"));
        assert!(config.contains("atrace_apps: \"com.reactor.fixture\""));
        assert!(config.contains("duration_ms: 12345"));
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
    fn flashlight_timeout_includes_per_iteration_automation_overhead() {
        let request = AndroidRunRequest {
            workspace: PathBuf::from("fixture"),
            flow_lock: PathBuf::from("flow.lock.json"),
            framework: "react-native".to_owned(),
            scenario: "list".to_owned(),
            device_id: "emulator-5554".to_owned(),
            duration_ms: 18_000,
            iteration_count: 10,
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
}
