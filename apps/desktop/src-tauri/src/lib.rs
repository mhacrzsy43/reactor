use std::{
    env,
    fs::{File, OpenOptions},
    future::Future,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[cfg(unix)]
use std::os::unix::{fs::OpenOptionsExt as _, process::CommandExt as _};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use reactor_ai::{
    AiProviderError, AnalysisAiProvider, AnalysisExplanation, AnalysisExplanationRequest,
    CliFlowProvider, CliProviderKind, CliProviderStatus, CredentialStore, DryRunFailure,
    FlowAiProvider, FlowAssistantDecision, FlowAssistantRequest, FlowChange, FlowGenerationRequest,
    FlowModificationRequest, FlowProbeRequest, FlowQuestionRequest, FlowRepairRequest,
    GeneratedFlow, LocalModelStatus, MAX_FLOW_REPAIR_ATTEMPTS, OfflineAnalysisExplainer,
    OpenAiCompatibleProvider, RedactedUiContext, SystemCredentialStore, diff_flows,
    doctor_cli_provider, doctor_local_model as check_local_model, redact_ui_tree,
};
use reactor_analysis::{
    AnalysisReport, DiagnosticIndex, DiagnosticManifest as IndexDiagnosticManifest,
    DiagnosticProfileReport, FrameDrilldown as IndexFrameDrilldown, ProfileDiffReport,
    RegressionPolicy, SelectionAnalysis as IndexSelectionAnalysis,
    TimelineItem as IndexTimelineItem, TimelineOverview as IndexTimelineOverview,
    TimelineWindow as IndexTimelineWindow, analyze_pair,
    analyze_profile_json as analyze_diagnostic_profile,
    apply_source_map_json as apply_diagnostic_source_map,
    diff_profile_reports as diff_diagnostic_profiles,
};
use reactor_core::{CompiledFlow, compile_maestro};
use reactor_inspector::{InspectorElement, inspect_hierarchy};
use reactor_protocol::{
    ArtifactIntegrity, DiagnosticPlanV1, Flow, FlowLock, FlowTrialEvidence, GenerationProvenance,
    InputValue, Platform, RunMode, Selector, Step, canonical_flow_hash,
    navigation_destination_marker, requires_navigation_intent,
};
use reactor_runner::{
    AndroidLeakTestPlan, AndroidRunRequest, DiscoveredDevice, DoctorReport, IosRunRequest,
    TrialFailureEvidence, capture_android_current_ui_tree, capture_android_screenshot,
    capture_android_trial_failure, capture_android_ui_tree, capture_ios_current_ui_tree,
    capture_ios_screenshot, capture_ios_trial_failure, capture_ios_ui_tree,
    delete_all_flow_secrets, delete_flow_secret, discover_android_devices, discover_ios_simulators,
    doctor, enqueue_android, enqueue_demo, enqueue_ios, execute_android_job, execute_demo_job,
    execute_explorer_step, execute_ios_job, has_flow_secret, recover_orphaned_jobs,
    replay_explorer_flow_with_progress, sample_android_live_performance, save_flow_secret,
    trial_android, trial_ios_simulator,
};
use reactor_store::{DiagnosticRunCatalogEntry, DiagnosticRunFilter, Job, JobEvent, Store};
use reactor_toolchain::{InstalledManifest, ManagedToolsManifest, SetupOptions};
use serde::{Deserialize, Serialize};
use tauri::Manager as _;
use tauri::ipc::Channel;
use zeroize::Zeroizing;

mod updater;

// Tauri embeds the current production frontend into release and direct Cargo builds.

const MANAGED_TOOLS_MANIFEST: &str = include_str!("../../../../tools/managed-tools-v1.json");
static BUNDLED_TOOL_ARCHIVES: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    doctor: DoctorReport,
    devices: Vec<DiscoveredDevice>,
    workspace: String,
    active_job: Option<Job>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureDeviceInspectorInput {
    platform: Platform,
    device_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrialLivePerformanceInput {
    device_id: String,
    app_id: String,
    elapsed_ms: u64,
}

#[tauri::command]
async fn sample_trial_live_performance(
    input: TrialLivePerformanceInput,
) -> Result<serde_json::Value, String> {
    sample_android_live_performance(
        &workspace(),
        &input.device_id,
        &input.app_id,
        input.elapsed_ms,
    )
    .await
    .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInspectorSnapshot {
    platform: Platform,
    device_id: String,
    screenshot_data_url: String,
    screenshot_width: u32,
    screenshot_height: u32,
    viewport_width: f64,
    viewport_height: f64,
    captured_at: DateTime<Utc>,
    elements: Vec<InspectorElement>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceReplayFrame {
    platform: Platform,
    device_id: String,
    screenshot_data_url: String,
    screenshot_width: u32,
    screenshot_height: u32,
    captured_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PerformExplorerStepInput {
    platform: Platform,
    device_id: String,
    app_id: String,
    step: Step,
    #[serde(default)]
    execution_point: Option<reactor_protocol::Coordinate>,
    #[serde(default)]
    viewport_width: Option<f64>,
    #[serde(default)]
    viewport_height: Option<f64>,
    #[serde(default)]
    runtime_input: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowSecretInput {
    reference: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlowSecretStatus {
    reference: String,
    stored: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayExplorerFlowInput {
    platform: Platform,
    device_id: String,
    flow: Flow,
    #[serde(default)]
    prompt_values: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayProgress {
    completed_step_index: usize,
}

const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;
const PLUGIN_CONTRACT_VERSION: u32 = 1;
const MAX_PROFILE_JSON_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_MAP_BYTES: u64 = 128 * 1024 * 1024;
const AI_CLI_TIMEOUT_SECONDS: u64 = 120;
const AI_CLI_STDOUT_BYTES: u64 = 1024 * 1024;
const AI_CLI_STDERR_BYTES: u64 = 256 * 1024;
const LOCAL_TRACE_MIN_FREE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_WORKER_LOG_FILES: usize = 20;
const UPDATE_MANIFEST_SCHEMA_VERSION: u32 = 1;
const STABLE_UPDATE_ENDPOINT: &str =
    "https://github.com/mhacrzsy43/reactor/releases/latest/download/stable.json";
const BETA_UPDATE_ENDPOINT: &str =
    "https://github.com/mhacrzsy43/reactor/releases/download/beta/beta.json";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResourcePolicyView {
    plugin_contract_version: u32,
    external_plugins_enabled: bool,
    trusted_built_in_adapters: Vec<&'static str>,
    ai_cli_timeout_seconds: u64,
    ai_cli_stdout_bytes: u64,
    ai_cli_stderr_bytes: u64,
    max_profile_json_bytes: u64,
    max_source_map_bytes: u64,
    local_trace_min_free_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MaintenanceStatus {
    schema_version: i64,
    history_count: u64,
    workspace_bytes: u64,
    available_disk_bytes: u64,
    sensitive_artifact_count: u64,
    policy: ResourcePolicyView,
    update: UpdatePolicyView,
    last_update: Option<updater::UpdateTransactionView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
struct UpdatePolicyView {
    current_version: &'static str,
    default_channel: &'static str,
    stable_endpoint: &'static str,
    beta_endpoint: &'static str,
    manifest_schema_version: u32,
    signature_algorithm: &'static str,
    signature_required: bool,
    production_key_configured: bool,
    staged_install: bool,
    rollback_on_failed_health_check: bool,
    compatibility_line: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateManifestV1 {
    schema_version: u32,
    channel: String,
    version: String,
    published_at: String,
    compatibility: UpdateCompatibility,
    artifacts: Vec<UpdateArtifact>,
    signature: UpdateSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCompatibility {
    minimum_app_version: String,
    database_schema: i64,
    flow_schemas: Vec<u32>,
    result_schemas: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateArtifact {
    platform: String,
    arch: String,
    url: String,
    sha256: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSignature {
    algorithm: String,
    key_id: String,
    value: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SignedUpdatePayload<'a> {
    schema_version: u32,
    channel: &'a str,
    version: &'a str,
    published_at: &'a str,
    compatibility: &'a UpdateCompatibility,
    artifacts: &'a [UpdateArtifact],
    signature_algorithm: &'a str,
    signature_key_id: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedUpdateManifest {
    channel: String,
    version: String,
    artifact_count: usize,
    key_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallStagedUpdateInput {
    transaction_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StageUpdateInput {
    channel: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticBundleResult {
    path: String,
    credential_values_included: bool,
    screenshots_included: bool,
    ui_trees_included: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrivacyEraseInput {
    mode: String,
    confirmation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrivacyEraseResult {
    removed_files: u64,
    removed_bytes: u64,
    credentials_removed: bool,
    full_reset: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateInput {
    intent: String,
    app_id: String,
    platform: Platform,
    ui_tree: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    save_api_key: bool,
    #[serde(default)]
    use_saved_api_key: bool,
    model: Option<String>,
    provider: Option<String>,
    cli_executable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModifyFlowInput {
    flow: Flow,
    instruction: String,
    failure_context: Option<String>,
    ui_tree: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    save_api_key: bool,
    #[serde(default)]
    use_saved_api_key: bool,
    model: Option<String>,
    provider: String,
    cli_executable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowAssistantInput {
    flow: Option<Flow>,
    instruction: String,
    app_id: String,
    platform: Platform,
    ui_tree: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    save_api_key: bool,
    #[serde(default)]
    use_saved_api_key: bool,
    model: Option<String>,
    provider: String,
    cli_executable: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlowModificationProposal {
    generated: GeneratedFlow,
    changes: Vec<FlowChange>,
    answer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewGenerationContextInput {
    app_id: String,
    platform: Platform,
    device_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliDoctorInput {
    codex_executable: Option<String>,
    claude_executable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalModelDoctorInput {
    endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FailureEvidenceView {
    artifact_dir: String,
    error_path: String,
    ui_tree_path: Option<String>,
    screenshot_path: Option<String>,
}

impl From<&TrialFailureEvidence> for FailureEvidenceView {
    fn from(value: &TrialFailureEvidence) -> Self {
        Self {
            artifact_dir: value.artifact_dir.clone(),
            error_path: value.error_path.clone(),
            ui_tree_path: value.ui_tree_path.clone(),
            screenshot_path: value.screenshot_path.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrialPreparation {
    generated: GeneratedFlow,
    trial: Option<FlowTrialEvidence>,
    failure: Option<DryRunFailure>,
    evidence: Option<FailureEvidenceView>,
    context: Option<RedactedUiContext>,
    source_context: Option<RedactedUiContext>,
    goal_evidence: Option<GoalEvidenceView>,
    changes: Vec<FlowChange>,
    repair_attempts: u32,
    model_calls: u32,
    audit_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalEvidenceView {
    marker: String,
    source_contains_marker: bool,
    destination_contains_marker: bool,
    source_elements: usize,
    destination_elements: usize,
    verified: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrialGeneratedInput {
    generated: GeneratedFlow,
    device_id: Option<String>,
    source_context: Option<RedactedUiContext>,
    #[serde(default)]
    prompt_values: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepairFlowInput {
    preparation: TrialPreparation,
    device_id: String,
    endpoint: String,
    api_key: Option<String>,
    #[serde(default)]
    save_api_key: bool,
    #[serde(default)]
    use_saved_api_key: bool,
    model: String,
    allow_model_context: bool,
    provider: Option<String>,
    cli_executable: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedJob {
    job_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobSnapshot {
    job: Job,
    events: Vec<JobEvent>,
    has_more_events: bool,
    results: Vec<reactor_protocol::NormalizedResult>,
    report_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobPage {
    jobs: Vec<Job>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeJobPairInput {
    baseline_job_id: String,
    current_job_id: String,
    policy: Option<RegressionPolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeProfileInput {
    json: String,
    source_map: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeManagedProfileInput {
    job_id: String,
    run_id: String,
    artifact: ManagedProfileArtifactInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagedProfileArtifactInput {
    path: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffProfileInput {
    baseline: DiagnosticProfileReport,
    current: DiagnosticProfileReport,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRunInput {
    job_id: String,
    run_id: String,
    flow_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticOverviewInput {
    job_id: String,
    run_id: String,
    start_ms: f64,
    end_ms: f64,
    pixel_width: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticManifestDto {
    schema_version: u32,
    run_id: String,
    range: Option<DiagnosticRangeDto>,
    tracks: Vec<DiagnosticTrackAvailabilityDto>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRangeDto {
    start_ms: f64,
    end_ms: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticTrackAvailabilityDto {
    kind: String,
    track_id: Option<i64>,
    state: String,
    label: String,
    reason: Option<String>,
    count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineOverviewDto {
    range: DiagnosticRangeDto,
    tracks: Vec<TimelineOverviewTrackDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineOverviewTrackDto {
    kind: String,
    buckets: Vec<TimelineOverviewBucketDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineOverviewBucketDto {
    start_ms: f64,
    end_ms: f64,
    count: u64,
    max_duration_ms: Option<f64>,
    slow_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineItemDto {
    id: i64,
    track_id: i64,
    track: String,
    item_type: String,
    start_ms: f64,
    end_ms: f64,
    label: String,
    severity: Option<String>,
    data: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineWindowDto {
    range: DiagnosticRangeDto,
    items: Vec<TimelineItemDto>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionAnalysisDto {
    range: DiagnosticRangeDto,
    summary: String,
    event_count: u64,
    frame_count: u64,
    slow_frame_count: u64,
    react_commit_count: u64,
    cpu_sample_count: u64,
    top_functions: Vec<reactor_analysis::RankedValue>,
    top_components: Vec<reactor_analysis::RankedValue>,
    availability: std::collections::BTreeMap<String, DiagnosticTrackAvailabilityDto>,
    correlations: Vec<serde_json::Value>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameDrilldownDto {
    available: bool,
    reason: Option<String>,
    frame_id: Option<i64>,
    start_ms: f64,
    end_ms: f64,
    duration_ms: Option<f64>,
    budget_ms: Option<f64>,
    classification: Option<String>,
    details: Vec<FrameDetailDto>,
    correlations: Vec<serde_json::Value>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FrameDetailDto {
    label: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticWindowInput {
    job_id: String,
    run_id: String,
    start_ms: f64,
    end_ms: f64,
    #[serde(default)]
    track_ids: Vec<i64>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticSelectionInput {
    job_id: String,
    run_id: String,
    start_ms: f64,
    end_ms: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
struct FrameDrilldownInput {
    job_id: String,
    run_id: String,
    frame_id: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobAnalysis {
    baseline_job: Job,
    current_job: Job,
    reports: Vec<AnalysisReport>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplainAnalysisInput {
    report: AnalysisReport,
    provider: Option<String>,
    endpoint: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    save_api_key: bool,
    #[serde(default)]
    use_saved_api_key: bool,
    model: Option<String>,
    cli_executable: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RealRunInput {
    flow_lock: FlowLock,
    framework: String,
    scenario: String,
    device_id: String,
    duration_ms: u64,
    iterations: u32,
    #[serde(default)]
    run_mode: RunMode,
    #[serde(default)]
    diagnostic_plan: Option<DiagnosticPlanV1>,
    leak_test: Option<AndroidLeakTestPlan>,
    #[serde(default)]
    manual_session: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRunsInput {
    limit: Option<u32>,
    offset: Option<u32>,
    flow_hash: Option<String>,
    framework: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRunSummary {
    job_id: String,
    run_id: String,
    created_at: DateTime<Utc>,
    framework: String,
    platform: String,
    scenario: String,
    flow_hash: String,
    app_id: Option<String>,
    app_version: Option<String>,
    adapter: String,
    device_name: Option<String>,
    device_physical: Option<bool>,
    successful_iteration_count: u64,
    iteration_count: u64,
    synthetic: bool,
    lock_available: bool,
    result: reactor_protocol::NormalizedResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRunPage {
    runs: Vec<DiagnosticRunSummary>,
    total: u64,
    offset: u32,
    limit: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticRerunEligibility {
    job_id: String,
    run_id: String,
    lock_available: bool,
    platform: String,
    diagnose_available: bool,
    eligible: bool,
    reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionManifestV1 {
    schema_version: u32,
    job_id: String,
    flow_hash: String,
    flow_lock_sha256: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupToolsInput {
    offline: bool,
    proxy: Option<String>,
    maestro_override: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkerRequest {
    Demo {
        workspace: PathBuf,
        flow_lock: Box<FlowLock>,
    },
    Android {
        request: AndroidRunRequest,
    },
    Ios {
        request: IosRunRequest,
    },
}

/// Returns the self-contained desktop runtime directory.
///
/// A packaged macOS app must not put its `SQLite` database and managed tools beside the source
/// checkout: Finder-launched applications can be denied or stalled by macOS Documents access,
/// and an installed DMG would otherwise depend on the build machine's checkout path. Developers
/// can still point the desktop shell at an isolated fixture with `REACTOR_WORKSPACE`.
fn workspace() -> PathBuf {
    if let Some(path) = env::var_os("REACTOR_WORKSPACE").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "macos")]
    {
        return home_directory()
            .join("Library/Application Support")
            .join("com.reactor.performance");
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_directory)
            .join("Reactor");
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory().join(".local/share"))
            .join("reactor");
    }
    #[allow(unreachable_code)]
    home_directory().join(".reactor")
}

fn home_directory() -> PathBuf {
    env::var_os("HOME").map_or_else(|| std::env::temp_dir().join("reactor-user"), PathBuf::from)
}

fn bundled_tool_archives() -> Option<PathBuf> {
    BUNDLED_TOOL_ARCHIVES.get().cloned()
}

fn resource_policy() -> ResourcePolicyView {
    ResourcePolicyView {
        plugin_contract_version: PLUGIN_CONTRACT_VERSION,
        external_plugins_enabled: false,
        trusted_built_in_adapters: vec![
            "maestro",
            "android-perfetto",
            "android-flashlight",
            "ios-xctrace",
        ],
        ai_cli_timeout_seconds: AI_CLI_TIMEOUT_SECONDS,
        ai_cli_stdout_bytes: AI_CLI_STDOUT_BYTES,
        ai_cli_stderr_bytes: AI_CLI_STDERR_BYTES,
        max_profile_json_bytes: MAX_PROFILE_JSON_BYTES,
        max_source_map_bytes: MAX_SOURCE_MAP_BYTES,
        local_trace_min_free_bytes: LOCAL_TRACE_MIN_FREE_BYTES,
    }
}

fn update_policy() -> UpdatePolicyView {
    UpdatePolicyView {
        current_version: env!("CARGO_PKG_VERSION"),
        default_channel: "stable",
        stable_endpoint: STABLE_UPDATE_ENDPOINT,
        beta_endpoint: BETA_UPDATE_ENDPOINT,
        manifest_schema_version: UPDATE_MANIFEST_SCHEMA_VERSION,
        signature_algorithm: "Ed25519",
        signature_required: true,
        production_key_configured: option_env!("REACTOR_UPDATE_PUBLIC_KEY")
            .is_some_and(|value| !value.trim().is_empty()),
        staged_install: true,
        rollback_on_failed_health_check: true,
        compatibility_line: "1.x keeps Flow v1, Result v1 and transactional database upgrades readable",
    }
}

fn signed_update_payload(manifest: &UpdateManifestV1) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&SignedUpdatePayload {
        schema_version: manifest.schema_version,
        channel: &manifest.channel,
        version: &manifest.version,
        published_at: &manifest.published_at,
        compatibility: &manifest.compatibility,
        artifacts: &manifest.artifacts,
        signature_algorithm: &manifest.signature.algorithm,
        signature_key_id: &manifest.signature.key_id,
    })
    .map_err(|error| error.to_string())
}

fn validate_signed_update_manifest(
    manifest: &UpdateManifestV1,
    public_key_base64: &str,
    supported_database_schema: i64,
) -> Result<(), String> {
    if manifest.schema_version != UPDATE_MANIFEST_SCHEMA_VERSION {
        return Err("不支持的更新 manifest 版本".to_owned());
    }
    if !matches!(manifest.channel.as_str(), "stable" | "beta") {
        return Err("更新通道必须是 stable 或 beta".to_owned());
    }
    if manifest.version.trim().is_empty()
        || manifest.published_at.trim().is_empty()
        || manifest.compatibility.minimum_app_version.trim().is_empty()
    {
        return Err("更新版本或发布时间缺失".to_owned());
    }
    if manifest.compatibility.database_schema > supported_database_schema
        || !manifest.compatibility.flow_schemas.contains(&1)
        || !manifest.compatibility.result_schemas.contains(&1)
    {
        return Err("更新与当前数据库、Flow 或 Result 协议不兼容".to_owned());
    }
    if manifest.artifacts.is_empty()
        || manifest.artifacts.iter().any(|artifact| {
            !artifact.url.starts_with("https://")
                || artifact.size == 0
                || artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err("更新 artifact 元数据不完整或不安全".to_owned());
    }
    if manifest.signature.algorithm != "Ed25519" || manifest.signature.key_id.len() < 8 {
        return Err("更新签名元数据无效".to_owned());
    }
    let public_key = BASE64_STANDARD
        .decode(public_key_base64.trim())
        .map_err(|_| "发布公钥不是有效 Base64".to_owned())?;
    let signature = BASE64_STANDARD
        .decode(manifest.signature.value.trim())
        .map_err(|_| "manifest 签名不是有效 Base64".to_owned())?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(&signed_update_payload(manifest)?, &signature)
        .map_err(|_| "manifest Ed25519 签名验证失败".to_owned())
}

fn is_sensitive_artifact(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains("screenshot")
        || name.contains("ui-tree")
        || name.contains("ui_hierarchy")
        || name.contains("ui-hierarchy")
}

fn is_sensitive_registered_kind(kind: &str) -> bool {
    matches!(
        kind,
        "perfetto_trace"
            | "react_native_hermes_cpu"
            | "react_native_diagnostics"
            | "react_native_profile"
            | "react_native_hermes_heap_stats"
            | "react_native_hermes_heap_snapshot"
            | "react_native_java_heap_dump"
            | "android_native_heap_trace"
            | "xctrace_archive"
            | "xctrace_profile"
    )
}

fn is_sensitive_diagnostic_provenance(artifact: &reactor_protocol::ArtifactRef) -> bool {
    let provenance = format!(
        "{} {} {} {}",
        artifact.format, artifact.producer, artifact.producer_version, artifact.capture_method
    )
    .to_ascii_lowercase();
    ["trace", "profile", "heap", "reactor-rn-events"]
        .iter()
        .any(|marker| provenance.contains(marker))
}

fn managed_existing_file_from(root: &Path, base: &Path, path: &Path) -> Option<PathBuf> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let canonical = std::fs::canonicalize(candidate).ok()?;
    let metadata = std::fs::symlink_metadata(&canonical).ok()?;
    (metadata.is_file() && canonical.starts_with(canonical_root)).then_some(canonical)
}

fn managed_existing_file(root: &Path, path: &Path) -> Option<PathBuf> {
    managed_existing_file_from(root, root, path)
}

fn collect_result_diagnostic_paths(
    root: &Path,
    result_base: &Path,
    result: &reactor_protocol::NormalizedResult,
) -> Vec<PathBuf> {
    let mut paths = result
        .artifacts
        .iter()
        .filter(|artifact| is_sensitive_diagnostic_provenance(artifact))
        .filter_map(|artifact| {
            managed_existing_file_from(root, result_base, Path::new(&artifact.path))
        })
        .collect::<Vec<_>>();
    if let Some(diagnostics) = result.react_native_diagnostics() {
        match diagnostics {
            reactor_protocol::ReactNativeDiagnosticsView::V1(diagnostics) => {
                paths.extend(
                    diagnostics
                        .collectors
                        .values()
                        .flat_map(|collector| &collector.artifacts)
                        .filter(|artifact| is_sensitive_diagnostic_provenance(artifact))
                        .filter_map(|artifact| {
                            managed_existing_file_from(root, result_base, Path::new(&artifact.path))
                        }),
                );
            }
            reactor_protocol::ReactNativeDiagnosticsView::Legacy(diagnostics) => {
                paths.extend(
                    std::iter::once(Some(diagnostics.event_file.as_str()))
                        .chain(std::iter::once(diagnostics.profile_file.as_deref()))
                        .flatten()
                        .filter_map(|path| {
                            managed_existing_file_from(root, result_base, Path::new(path))
                        }),
                );
            }
        }
    }
    paths
}

fn scan_files(root: &Path) -> Result<(u64, u64, Vec<PathBuf>), String> {
    if !root.exists() {
        return Ok((0, 0, Vec::new()));
    }
    let mut stack = vec![root.to_path_buf()];
    let mut file_count = 0_u64;
    let mut total_bytes = 0_u64;
    let mut sensitive = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                file_count += 1;
                total_bytes = total_bytes.saturating_add(metadata.len());
                if is_sensitive_artifact(&path) {
                    sensitive.push(path);
                }
            }
        }
    }
    Ok((file_count, total_bytes, sensitive))
}

fn maintenance_status_for(root: &Path) -> Result<MaintenanceStatus, String> {
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let (_, workspace_bytes, sensitive) = scan_files(root)?;
    let available_disk_bytes = fs2::available_space(root).map_err(|error| error.to_string())?;
    Ok(MaintenanceStatus {
        schema_version: store.schema_version().map_err(|error| error.to_string())?,
        history_count: store.job_count().map_err(|error| error.to_string())?,
        workspace_bytes,
        available_disk_bytes,
        sensitive_artifact_count: sensitive.len() as u64,
        policy: resource_policy(),
        update: update_policy(),
        last_update: updater::latest_transaction(root),
    })
}

fn create_diagnostic_bundle_for(root: &Path) -> Result<DiagnosticBundleResult, String> {
    let status = maintenance_status_for(root)?;
    let safe_checks = doctor(root)
        .checks
        .into_iter()
        .map(|check| {
            serde_json::json!({
                "id": check.id,
                "available": check.available,
                "managed": check.managed,
            })
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "schemaVersion": DIAGNOSTIC_SCHEMA_VERSION,
        "createdAt": chrono::Utc::now(),
        "reactorVersion": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "databaseSchemaVersion": status.schema_version,
        "historyCount": status.history_count,
        "workspaceBytes": status.workspace_bytes,
        "availableDiskBytes": status.available_disk_bytes,
        "sensitiveArtifactCount": status.sensitive_artifact_count,
        "resourcePolicy": status.policy,
        "updatePolicy": status.update,
        "toolChecks": safe_checks,
        "privacy": {
            "credentialValuesIncluded": false,
            "screenshotsIncluded": false,
            "uiTreesIncluded": false,
            "jobRequestsIncluded": false,
            "jobErrorsIncluded": false,
            "absolutePathsIncluded": false,
        }
    });
    let directory = root.join(".reactor/diagnostics");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("reactor-diagnostic-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())?;
    Ok(DiagnosticBundleResult {
        path: path.display().to_string(),
        credential_values_included: false,
        screenshots_included: false,
        ui_trees_included: false,
    })
}

fn erase_sensitive_files(root: &Path) -> Result<PrivacyEraseResult, String> {
    let results_root = root.join("results");
    let (_, _, named_sensitive) = scan_files(&results_root)?;
    let mut sensitive = named_sensitive
        .into_iter()
        .filter_map(|path| managed_existing_file(&results_root, &path))
        .collect::<std::collections::BTreeSet<_>>();

    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let total = store.job_count().map_err(|error| error.to_string())?;
    let mut offset = 0_u32;
    while u64::from(offset) < total {
        let jobs = store
            .list_jobs_page(100, offset)
            .map_err(|error| error.to_string())?;
        if jobs.is_empty() {
            break;
        }
        offset = offset.saturating_add(u32::try_from(jobs.len()).unwrap_or(100));
        for job in jobs {
            for artifact in store
                .list_artifacts(&job.id)
                .map_err(|error| error.to_string())?
            {
                if is_sensitive_registered_kind(&artifact.kind)
                    && let Some(path) = managed_existing_file(root, Path::new(&artifact.path))
                {
                    sensitive.insert(path);
                }
            }
            if let Some(result_path) = job.result_path {
                let result_base = Path::new(&result_path).parent().unwrap_or(root);
                for result in read_results(&result_path).unwrap_or_default() {
                    sensitive.extend(collect_result_diagnostic_paths(root, result_base, &result));
                }
            }
        }
    }
    drop(store);

    let worker_dir = root.join(".reactor/runtime/workers");
    if worker_dir.is_dir() {
        for entry in std::fs::read_dir(&worker_dir).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if matches!(
                path.extension().and_then(std::ffi::OsStr::to_str),
                Some("json" | "log")
            ) && let Some(path) = managed_existing_file(root, &path)
            {
                sensitive.insert(path);
            }
        }
    }

    let mut removed_files = 0_u64;
    let mut removed_bytes = 0_u64;
    for path in sensitive {
        let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
        std::fs::remove_file(&path).map_err(|error| error.to_string())?;
        removed_files += 1;
        removed_bytes = removed_bytes.saturating_add(metadata.len());
    }
    Ok(PrivacyEraseResult {
        removed_files,
        removed_bytes,
        credentials_removed: false,
        full_reset: false,
    })
}

fn remove_data_directory(path: &Path) -> Result<(u64, u64), String> {
    let (files, bytes, _) = scan_files(path)?;
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    }
    Ok((files, bytes))
}

fn erase_all_local_data(root: &Path) -> Result<PrivacyEraseResult, String> {
    SystemCredentialStore
        .delete("openai-compatible")
        .map_err(|error| error.to_string())?;
    delete_all_flow_secrets().map_err(|error| error.to_string())?;
    let targets = [
        root.join("results"),
        root.join(".reactor/runtime"),
        root.join(".reactor/diagnostics"),
    ];
    let mut removed_files = 0_u64;
    let mut removed_bytes = 0_u64;
    for target in targets {
        let (files, bytes) = remove_data_directory(&target)?;
        removed_files = removed_files.saturating_add(files);
        removed_bytes = removed_bytes.saturating_add(bytes);
    }
    Ok(PrivacyEraseResult {
        removed_files,
        removed_bytes,
        credentials_removed: true,
        full_reset: true,
    })
}

fn seed_bundled_tool_archives(source: &Path, workspace: &Path) -> Result<(), String> {
    let destination = workspace.join(".reactor/downloads");
    std::fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        if !source_path.is_file() {
            continue;
        }
        let target = destination.join(entry.file_name());
        if !target.is_file() {
            std::fs::copy(&source_path, target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn bootstrap() -> Result<Bootstrap, String> {
    let workspace = workspace();
    recover_orphaned_jobs(&workspace).map_err(|error| error.to_string())?;
    let (android, ios) = tokio::join!(
        discover_android_devices(&workspace),
        discover_ios_simulators()
    );
    let mut devices = android.unwrap_or_default();
    devices.extend(ios.unwrap_or_default());
    let active_job = Store::open(&workspace.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?
        .list_jobs(100)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|job| !job.state.is_terminal() && job.worker_pid.is_some());
    Ok(Bootstrap {
        doctor: doctor(&workspace),
        devices,
        workspace: workspace.display().to_string(),
        active_job,
    })
}

#[tauri::command]
fn maintenance_status() -> Result<MaintenanceStatus, String> {
    maintenance_status_for(&workspace())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn verify_update_manifest(manifest_json: String) -> Result<VerifiedUpdateManifest, String> {
    let public_key = option_env!("REACTOR_UPDATE_PUBLIC_KEY")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "当前构建未配置正式发布公钥；Reactor 拒绝验证或安装未签名更新".to_owned())?;
    let manifest: UpdateManifestV1 =
        serde_json::from_str(&manifest_json).map_err(|error| error.to_string())?;
    let supported_schema = Store::open(&workspace().join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?
        .schema_version()
        .map_err(|error| error.to_string())?;
    validate_signed_update_manifest(&manifest, public_key, supported_schema)?;
    Ok(VerifiedUpdateManifest {
        channel: manifest.channel,
        version: manifest.version,
        artifact_count: manifest.artifacts.len(),
        key_id: manifest.signature.key_id,
    })
}

fn current_install_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    {
        executable
            .ancestors()
            .find(|path| path.extension().and_then(std::ffi::OsStr::to_str) == Some("app"))
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                "自动安装只能从已打包的 Reactor.app 启动；开发二进制只允许验证更新".to_owned()
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        executable
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "无法定位当前 Reactor 安装目录".to_owned())
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn stage_update(input: StageUpdateInput) -> Result<updater::StagedUpdate, String> {
    let root = workspace();
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    if store.has_active_jobs().map_err(|error| error.to_string())? {
        return Err("存在运行中的任务；正式测量结束后才能检查或安装更新".to_owned());
    }
    let supported_schema = store.schema_version().map_err(|error| error.to_string())?;
    drop(store);
    let public_key = option_env!("REACTOR_UPDATE_PUBLIC_KEY")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "当前构建未配置正式发布公钥；Reactor 拒绝下载或安装更新".to_owned())?;
    let endpoint = match input.channel.as_str() {
        "stable" => STABLE_UPDATE_ENDPOINT,
        "beta" => BETA_UPDATE_ENDPOINT,
        _ => return Err("更新通道必须是 stable 或 beta".to_owned()),
    };
    updater::fetch_and_stage(
        &root,
        endpoint,
        &input.channel,
        public_key,
        supported_schema,
        &current_install_path()?,
    )
    .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn install_staged_update(
    input: InstallStagedUpdateInput,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = workspace();
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    if store.has_active_jobs().map_err(|error| error.to_string())? {
        return Err("存在运行中的任务；正式测量结束后才能安装更新".to_owned());
    }
    let transaction_path = PathBuf::from(input.transaction_path);
    let allowed_root = root.join(".reactor/updates/transactions");
    let canonical_transaction =
        std::fs::canonicalize(&transaction_path).map_err(|error| error.to_string())?;
    let canonical_root = std::fs::canonicalize(&allowed_root).map_err(|error| error.to_string())?;
    if !canonical_transaction.starts_with(&canonical_root)
        || canonical_transaction.file_name() != Some(std::ffi::OsStr::new("transaction.json"))
    {
        return Err("更新事务不属于 Reactor 的受管目录".to_owned());
    }
    updater::spawn_install_helper(&canonical_transaction)?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn create_diagnostic_bundle() -> Result<DiagnosticBundleResult, String> {
    create_diagnostic_bundle_for(&workspace())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn erase_private_data(input: PrivacyEraseInput) -> Result<PrivacyEraseResult, String> {
    let root = workspace();
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    if store.has_active_jobs().map_err(|error| error.to_string())? {
        return Err("存在运行中的任务；结束或取消任务后才能擦除数据".to_owned());
    }
    drop(store);
    match input.mode.as_str() {
        "sensitive_artifacts" if input.confirmation == "ERASE SENSITIVE" => {
            erase_sensitive_files(&root)
        }
        "all_local_data" if input.confirmation == "ERASE ALL" => erase_all_local_data(&root),
        "sensitive_artifacts" | "all_local_data" => Err("擦除确认文字不匹配".to_owned()),
        other => Err(format!("未知擦除模式: {other}")),
    }
}

#[tauri::command]
async fn generate_flow(input: GenerateInput) -> Result<GeneratedFlow, String> {
    let request = FlowGenerationRequest {
        intent: input.intent,
        app_id: input.app_id,
        platform: input.platform,
        ui_tree: input.ui_tree.map(|tree| redact_ui_tree(&tree, 0).ui_tree),
        screenshot_artifact_ids: vec![],
    };
    let provider = input
        .provider
        .as_deref()
        .ok_or_else(|| "请选择 Local Model、Codex CLI、Claude Code 或 Cloud AI".to_owned())?;
    ensure_model_calls_allowed()?;
    match provider {
        "offline" => Err("Reactor Offline Flow 生成已移除，请选择真实 AI Provider".to_owned()),
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| "Cloud AI 需要 API Key，密钥可选保存到系统钥匙串".to_owned())?;
            OpenAiCompatibleProvider::new(
                input
                    .endpoint
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                api_key,
                input.model.unwrap_or_else(|| "gpt-5-mini".to_owned()),
            )
            .generate(request)
            .await
            .map_err(|error| error.to_string())
        }
        "local" => OpenAiCompatibleProvider::new_local(
            input
                .endpoint
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_owned()),
            input.model.unwrap_or_else(|| "qwen2.5:7b".to_owned()),
        )
        .generate(request)
        .await
        .map_err(|error| error.to_string()),
        "codex" | "claude" => {
            let kind = if provider == "codex" {
                CliProviderKind::Codex
            } else {
                CliProviderKind::ClaudeCode
            };
            CliFlowProvider::new(kind, input.cli_executable.as_deref(), input.model)
                .map_err(|error| error.to_string())?
                .generate(request)
                .await
                .map_err(|error| error.to_string())
        }
        other => Err(format!("未知 Flow Provider: {other}")),
    }
}

#[tauri::command]
async fn classify_flow_request(input: FlowAssistantInput) -> Result<FlowAssistantDecision, String> {
    let instruction = input.instruction.trim();
    if instruction.is_empty() || instruction.chars().count() > 4_000 {
        return Err("Flow AI 输入必须为 1–4000 个字符".to_owned());
    }
    if input.provider == "local" {
        return Err("Local Model 暂不用于 Flow Explorer AI".to_owned());
    }
    ensure_model_calls_allowed()?;
    let provider: Box<dyn FlowAiProvider> = match input.provider.as_str() {
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| "Cloud AI 需要 API Key，密钥可选保存到系统钥匙串".to_owned())?;
            Box::new(OpenAiCompatibleProvider::new(
                input
                    .endpoint
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                api_key,
                input.model.unwrap_or_else(|| "gpt-5-mini".to_owned()),
            ))
        }
        "codex" | "claude" => {
            let kind = if input.provider == "codex" {
                CliProviderKind::Codex
            } else {
                CliProviderKind::ClaudeCode
            };
            Box::new(
                CliFlowProvider::new(kind, input.cli_executable.as_deref(), input.model)
                    .map_err(|error| error.to_string())?,
            )
        }
        other => return Err(format!("未知 Flow Provider: {other}")),
    };
    provider
        .classify_flow_request(FlowAssistantRequest {
            flow: input.flow,
            message: instruction.to_owned(),
            app_id: input.app_id,
            platform: input.platform,
            ui_tree: input.ui_tree.map(|tree| redact_ui_tree(&tree, 0).ui_tree),
        })
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn modify_flow(input: ModifyFlowInput) -> Result<FlowModificationProposal, String> {
    let instruction = input.instruction.trim();
    if instruction.is_empty() {
        return Err("请输入希望如何修改 Flow".to_owned());
    }
    if instruction.chars().count() > 4_000 {
        return Err("Flow 修改要求不能超过 4000 个字符".to_owned());
    }
    if input.provider == "offline" {
        return Err(
            "Reactor Offline 不是大模型；请选择 Local Model、Codex CLI、Claude Code 或 Cloud AI"
                .to_owned(),
        );
    }
    compile_maestro(&input.flow).map_err(|error| format!("当前 Flow 无法修改：{error}"))?;
    ensure_model_calls_allowed()?;
    let request = FlowModificationRequest {
        flow: input.flow.clone(),
        instruction: instruction.to_owned(),
        failure_context: input.failure_context,
        ui_tree: input.ui_tree.map(|tree| redact_ui_tree(&tree, 0).ui_tree),
    };
    let provider: Box<dyn FlowAiProvider> = match input.provider.as_str() {
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| "Cloud AI 需要 API Key，密钥可选保存到系统钥匙串".to_owned())?;
            Box::new(OpenAiCompatibleProvider::new(
                input
                    .endpoint
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                api_key,
                input.model.unwrap_or_else(|| "gpt-5-mini".to_owned()),
            ))
        }
        "local" => Box::new(OpenAiCompatibleProvider::new_local(
            input
                .endpoint
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_owned()),
            input.model.unwrap_or_else(|| "qwen2.5:7b".to_owned()),
        )),
        "codex" | "claude" => {
            let kind = if input.provider == "codex" {
                CliProviderKind::Codex
            } else {
                CliProviderKind::ClaudeCode
            };
            Box::new(
                CliFlowProvider::new(kind, input.cli_executable.as_deref(), input.model)
                    .map_err(|error| error.to_string())?,
            )
        }
        other => return Err(format!("未知 Flow Provider: {other}")),
    };
    let mut generated = modify_with_schema_retry(provider.as_ref(), request.clone()).await?;
    if generated.flow.app_id != input.flow.app_id {
        return Err("AI 修改试图改变 appId，Reactor 已拒绝该提案".to_owned());
    }
    if generated.flow.platform != input.flow.platform {
        return Err("AI 修改试图改变平台，Reactor 已拒绝该提案".to_owned());
    }
    compile_maestro(&generated.flow)
        .map_err(|error| format!("AI 修改未通过 Rust 校验：{error}"))?;
    let changes = diff_flows(&input.flow, &generated.flow).map_err(|error| error.to_string())?;
    let answer = if changes.is_empty() && request.failure_context.is_none() {
        Some(
            provider
                .answer_flow_question(FlowQuestionRequest {
                    flow: input.flow.clone(),
                    question: instruction.to_owned(),
                    ui_tree: request.ui_tree.clone(),
                })
                .await
                .map_err(|error| error.to_string())?
                .answer,
        )
    } else {
        None
    };
    generated
        .notes
        .push("Natural-language Flow modification proposed and validated".to_owned());
    Ok(FlowModificationProposal {
        generated,
        changes,
        answer,
    })
}

async fn modify_with_schema_retry(
    provider: &dyn FlowAiProvider,
    request: FlowModificationRequest,
) -> Result<GeneratedFlow, String> {
    match provider.modify(request.clone()).await {
        Ok(generated) => Ok(generated),
        Err(AiProviderError::InvalidResponse(validation_error)) => {
            let mut retry = request;
            retry.instruction = format!(
                "{}\n\nYour previous proposal was rejected by Reactor validation: {}. Return the complete corrected Flow. Fix only that validation defect, preserve the requested behavior and all unrelated steps, and use only non-empty selectors observed in the supplied redacted UI tree.",
                retry.instruction,
                validation_error.chars().take(2_000).collect::<String>()
            );
            provider.modify(retry).await.map_err(|retry_error| {
                format!(
                    "AI 提案两次未通过 Reactor 校验；当前 Flow 未改变。首次错误：{validation_error}；纠错错误：{retry_error}"
                )
            })
        }
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
async fn probe_flow(input: GenerateInput) -> Result<GeneratedFlow, String> {
    let request = FlowProbeRequest {
        goal: input.intent,
        app_id: input.app_id,
        platform: input.platform,
        ui_tree: redact_ui_tree(
            input
                .ui_tree
                .as_deref()
                .ok_or_else(|| "逐步探索需要当前页面 UI 树".to_owned())?,
            0,
        )
        .ui_tree,
    };
    let provider = input
        .provider
        .as_deref()
        .ok_or_else(|| "请选择用于逐步探索的 AI Provider".to_owned())?;
    ensure_model_calls_allowed()?;
    match provider {
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| "Cloud AI 需要 API Key，密钥可选保存到系统钥匙串".to_owned())?;
            OpenAiCompatibleProvider::new(
                input
                    .endpoint
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                api_key,
                input.model.unwrap_or_else(|| "gpt-5-mini".to_owned()),
            )
            .probe(request)
            .await
            .map_err(|error| error.to_string())
        }
        "local" => OpenAiCompatibleProvider::new_local(
            input
                .endpoint
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_owned()),
            input.model.unwrap_or_else(|| "qwen2.5:7b".to_owned()),
        )
        .probe(request)
        .await
        .map_err(|error| error.to_string()),
        "codex" | "claude" => {
            let kind = if provider == "codex" {
                CliProviderKind::Codex
            } else {
                CliProviderKind::ClaudeCode
            };
            CliFlowProvider::new(kind, input.cli_executable.as_deref(), input.model)
                .map_err(|error| error.to_string())?
                .probe(request)
                .await
                .map_err(|error| error.to_string())
        }
        other => Err(format!("未知 Flow Provider: {other}")),
    }
}

#[tauri::command]
async fn preview_generation_context(
    input: PreviewGenerationContextInput,
) -> Result<RedactedUiContext, String> {
    let tree = match input.platform {
        Platform::Android => {
            capture_android_ui_tree(&workspace(), &input.device_id, &input.app_id).await
        }
        Platform::Ios => capture_ios_ui_tree(&workspace(), &input.device_id, &input.app_id).await,
    }
    .map_err(|error| error.to_string())?;
    Ok(redact_ui_tree(&tree, 0))
}

#[tauri::command]
async fn capture_device_inspector(
    input: CaptureDeviceInspectorInput,
) -> Result<DeviceInspectorSnapshot, String> {
    capture_device_inspector_for(input).await
}

#[tauri::command]
async fn capture_device_replay_frame(
    input: CaptureDeviceInspectorInput,
) -> Result<DeviceReplayFrame, String> {
    let root = workspace();
    ensure_inspector_capture_allowed(&root)?;
    let screenshot = match input.platform {
        Platform::Android => capture_android_screenshot(&root, &input.device_id).await,
        Platform::Ios => capture_ios_screenshot(&root, &input.device_id).await,
    }
    .map_err(|error| error.to_string())?;
    ensure_inspector_capture_allowed(&root)?;
    let (screenshot_width, screenshot_height) = png_dimensions(&screenshot)?;
    Ok(DeviceReplayFrame {
        platform: input.platform,
        device_id: input.device_id,
        screenshot_data_url: format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(screenshot)
        ),
        screenshot_width,
        screenshot_height,
        captured_at: Utc::now(),
    })
}

async fn capture_device_inspector_for(
    input: CaptureDeviceInspectorInput,
) -> Result<DeviceInspectorSnapshot, String> {
    let root = workspace();
    ensure_inspector_capture_allowed(&root)?;
    let (screenshot, hierarchy) =
        capture_synchronized_inspector_pair(&root, input.platform, &input.device_id).await?;
    // A worker may have been started by another Reactor process while capture was in flight. In
    // that case discard the snapshot so inspection can never be mistaken for measurement data.
    ensure_inspector_capture_allowed(&root)?;

    let (screenshot_width, screenshot_height) = png_dimensions(&screenshot)?;
    let elements =
        inspect_hierarchy(input.platform, &hierarchy).map_err(|error| error.to_string())?;
    let (viewport_width, viewport_height) = inspector_viewport(
        input.platform,
        screenshot_width,
        screenshot_height,
        &elements,
    );
    let warnings = inspector_warnings(&elements, viewport_width, viewport_height);
    Ok(DeviceInspectorSnapshot {
        platform: input.platform,
        device_id: input.device_id,
        screenshot_data_url: format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(screenshot)
        ),
        screenshot_width,
        screenshot_height,
        viewport_width,
        viewport_height,
        captured_at: Utc::now(),
        elements,
        warnings,
    })
}

async fn capture_synchronized_inspector_pair(
    root: &Path,
    platform: Platform,
    device_id: &str,
) -> Result<(Vec<u8>, String), String> {
    for attempt in 0..3 {
        ensure_inspector_capture_allowed(root)?;
        let before = match platform {
            Platform::Android => capture_android_current_ui_tree(root, device_id).await,
            Platform::Ios => capture_ios_current_ui_tree(root, device_id).await,
        }
        .map_err(|error| error.to_string())?;
        let screenshot = match platform {
            Platform::Android => capture_android_screenshot(root, device_id).await,
            Platform::Ios => capture_ios_screenshot(root, device_id).await,
        }
        .map_err(|error| error.to_string())?;
        let after = match platform {
            Platform::Android => capture_android_current_ui_tree(root, device_id).await,
            Platform::Ios => capture_ios_current_ui_tree(root, device_id).await,
        }
        .map_err(|error| error.to_string())?;
        if before == after {
            return Ok((screenshot, after));
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
    }
    Err("页面或键盘仍在变化，无法把截图与 Selector 对齐；请等待画面稳定后重新同步。".to_owned())
}

#[tauri::command]
async fn perform_explorer_step(
    input: PerformExplorerStepInput,
) -> Result<DeviceInspectorSnapshot, String> {
    let root = workspace();
    ensure_inspector_capture_allowed(&root)?;
    let mut viewport_size = input.viewport_width.zip(input.viewport_height);
    if input.platform == Platform::Android
        && matches!(&input.step, Step::Swipe { .. })
        && viewport_size.is_none()
    {
        // A WebView wheel callback can arrive while React is replacing the previous snapshot.
        // Recover the authoritative device dimensions instead of turning a visible mirror into a
        // failed recording. This capture stays outside every performance measurement window.
        let screenshot = capture_android_screenshot(&root, &input.device_id)
            .await
            .map_err(|error| error.to_string())?;
        let (width, height) = png_dimensions(&screenshot)?;
        viewport_size = Some((f64::from(width), f64::from(height)));
    }
    let mut prompt_values = std::collections::BTreeMap::new();
    if let Step::InputText {
        value: InputValue::PromptRef(reference),
        ..
    } = &input.step
    {
        let value = input
            .runtime_input
            .ok_or_else(|| format!("promptRef {} 需要本次交互输入值", reference.prompt_ref))?;
        if value.is_empty() {
            return Err("交互输入值不能为空".to_owned());
        }
        prompt_values.insert(reference.prompt_ref.clone(), Zeroizing::new(value));
    } else if input.runtime_input.is_some() {
        return Err("runtimeInput 只允许用于 promptRef，避免绕过 Flow 引用策略".to_owned());
    }
    execute_explorer_step(
        &root,
        input.platform,
        &input.device_id,
        &input.app_id,
        input.step,
        input.execution_point,
        viewport_size,
        (!prompt_values.is_empty()).then_some(prompt_values),
    )
    .await
    .map_err(|error| error.to_string())?;
    ensure_inspector_capture_allowed(&root)?;
    if input.platform == Platform::Android {
        // Return the changed pixels immediately, then let the desktop refresh the expensive
        // accessibility hierarchy in the background. This keeps the recorder responsive without
        // weakening final Maestro replay and destination-proof gates.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let screenshot = capture_android_screenshot(&root, &input.device_id)
            .await
            .map_err(|error| error.to_string())?;
        ensure_inspector_capture_allowed(&root)?;
        let (screenshot_width, screenshot_height) = png_dimensions(&screenshot)?;
        return Ok(DeviceInspectorSnapshot {
            platform: input.platform,
            device_id: input.device_id,
            screenshot_data_url: format!(
                "data:image/png;base64,{}",
                BASE64_STANDARD.encode(screenshot)
            ),
            screenshot_width,
            screenshot_height,
            viewport_width: f64::from(screenshot_width),
            viewport_height: f64::from(screenshot_height),
            captured_at: Utc::now(),
            elements: vec![],
            warnings: vec!["设备动作已完成；Selector 索引正在后台刷新".to_owned()],
        });
    }
    let stable = wait_for_explorer_stability(&root, input.platform, &input.device_id).await;
    let mut snapshot = capture_device_inspector_for(CaptureDeviceInspectorInput {
        platform: input.platform,
        device_id: input.device_id,
    })
    .await?;
    if !stable {
        snapshot
            .warnings
            .push("页面在 4 秒内未形成连续一致的 UI 树；请检查动画、键盘或动态内容".to_owned());
    }
    Ok(snapshot)
}

#[tauri::command]
async fn replay_recorded_flow(
    input: ReplayExplorerFlowInput,
    on_progress: Channel<ReplayProgress>,
) -> Result<DeviceInspectorSnapshot, String> {
    let root = workspace();
    ensure_inspector_capture_allowed(&root)?;
    let prompt_values = input
        .prompt_values
        .into_iter()
        .map(|(reference, value)| (reference, Zeroizing::new(value)))
        .collect::<std::collections::BTreeMap<_, _>>();
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
    let progress_forwarder = tokio::spawn(async move {
        while let Some(completed_step_index) = progress_rx.recv().await {
            let _ = on_progress.send(ReplayProgress {
                completed_step_index,
            });
        }
    });
    let replay_result = replay_explorer_flow_with_progress(
        &root,
        input.platform,
        &input.device_id,
        &input.flow,
        (!prompt_values.is_empty()).then_some(prompt_values),
        Some(progress_tx),
    )
    .await;
    progress_forwarder
        .await
        .map_err(|error| error.to_string())?;
    replay_result.map_err(|error| error.to_string())?;
    capture_device_inspector_for(CaptureDeviceInspectorInput {
        platform: input.platform,
        device_id: input.device_id,
    })
    .await
}

async fn wait_for_explorer_stability(root: &Path, platform: Platform, device_id: &str) -> bool {
    let mut previous = None;
    for _ in 0..8 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let tree = match platform {
            Platform::Android => capture_android_current_ui_tree(root, device_id).await,
            Platform::Ios => capture_ios_current_ui_tree(root, device_id).await,
        };
        let Ok(tree) = tree else {
            continue;
        };
        if previous.as_ref() == Some(&tree) {
            return true;
        }
        previous = Some(tree);
    }
    false
}

fn ensure_inspector_capture_allowed(root: &Path) -> Result<(), String> {
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    if store.has_active_jobs().map_err(|error| error.to_string())? {
        return Err(
            "Flow Explorer 已暂停：存在运行中的测试任务，截图和 UI 树同步不会进入测量窗口"
                .to_owned(),
        );
    }
    Ok(())
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err("设备截图不是有效的 PNG IHDR 图像".to_owned());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| "PNG 宽度缺失")?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| "PNG 高度缺失")?);
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return Err(format!("设备截图尺寸不安全或无效：{width} × {height}"));
    }
    Ok((width, height))
}

fn inspector_viewport(
    platform: Platform,
    screenshot_width: u32,
    screenshot_height: u32,
    elements: &[InspectorElement],
) -> (f64, f64) {
    let screenshot = (f64::from(screenshot_width), f64::from(screenshot_height));
    if platform == Platform::Android {
        return screenshot;
    }
    let max_x = elements
        .iter()
        .map(|element| element.bounds.x + element.bounds.width)
        .fold(0.0_f64, f64::max);
    let max_y = elements
        .iter()
        .map(|element| element.bounds.y + element.bounds.height)
        .fold(0.0_f64, f64::max);
    for scale in [3.0, 2.0] {
        let logical_width = screenshot.0 / scale;
        let logical_height = screenshot.1 / scale;
        if max_x >= logical_width * 0.85
            && max_x <= logical_width * 1.02
            && max_y >= logical_height * 0.85
            && max_y <= logical_height * 1.02
        {
            return (logical_width, logical_height);
        }
    }
    screenshot
}

fn inspector_warnings(
    elements: &[InspectorElement],
    viewport_width: f64,
    viewport_height: f64,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if elements.is_empty() {
        warnings
            .push("当前 UI 树没有可审查元素；WebView、Canvas 或自绘界面可能不可访问".to_owned());
        return warnings;
    }
    if !elements
        .iter()
        .flat_map(|element| &element.candidates)
        .any(|candidate| candidate.score >= 80)
    {
        warnings
            .push("当前页面没有高稳定性 Selector，建议补充 accessibility/resource id".to_owned());
    }
    let outside = elements
        .iter()
        .filter(|element| {
            element.bounds.x < 0.0
                || element.bounds.y < 0.0
                || element.bounds.x + element.bounds.width > viewport_width + 1.0
                || element.bounds.y + element.bounds.height > viewport_height + 1.0
        })
        .count();
    if outside > 0 {
        warnings.push(format!("{outside} 个元素部分位于当前画面之外"));
    }
    warnings
}

#[tauri::command]
async fn doctor_cli_providers(input: CliDoctorInput) -> Vec<CliProviderStatus> {
    let (codex, claude) = tokio::join!(
        doctor_cli_provider(CliProviderKind::Codex, input.codex_executable.as_deref()),
        doctor_cli_provider(
            CliProviderKind::ClaudeCode,
            input.claude_executable.as_deref()
        )
    );
    vec![codex, claude]
}

#[tauri::command]
async fn doctor_local_model(input: LocalModelDoctorInput) -> LocalModelStatus {
    check_local_model(&input.endpoint).await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn compile_flow_preview(flow: Flow) -> Result<CompiledFlow, String> {
    compile_maestro(&flow).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_flow_secret_value(input: FlowSecretInput) -> Result<FlowSecretStatus, String> {
    let value = input
        .value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Secret 值不能为空".to_owned())?;
    save_flow_secret(&input.reference, &value).map_err(|error| error.to_string())?;
    Ok(FlowSecretStatus {
        reference: input.reference,
        stored: true,
    })
}

#[tauri::command]
fn get_flow_secret_status(input: FlowSecretInput) -> Result<FlowSecretStatus, String> {
    let stored = has_flow_secret(&input.reference).map_err(|error| error.to_string())?;
    Ok(FlowSecretStatus {
        reference: input.reference,
        stored,
    })
}

#[tauri::command]
fn delete_flow_secret_value(input: FlowSecretInput) -> Result<FlowSecretStatus, String> {
    delete_flow_secret(&input.reference).map_err(|error| error.to_string())?;
    Ok(FlowSecretStatus {
        reference: input.reference,
        stored: false,
    })
}

fn resolve_api_key(
    provided: Option<String>,
    save: bool,
    use_saved: bool,
) -> Result<Option<String>, String> {
    let provided = provided.filter(|value| !value.trim().is_empty());
    if let Some(value) = provided {
        if save {
            SystemCredentialStore
                .save("openai-compatible", &value)
                .map_err(|error| error.to_string())?;
        }
        return Ok(Some(value));
    }
    if use_saved {
        return SystemCredentialStore
            .load("openai-compatible")
            .map_err(|error| error.to_string());
    }
    Ok(None)
}

#[tauri::command]
async fn trial_generated_flow(input: TrialGeneratedInput) -> Result<TrialPreparation, String> {
    let prompt_values = input
        .prompt_values
        .into_iter()
        .map(|(reference, value)| (reference, Zeroizing::new(value)))
        .collect();
    run_preparation(
        input.generated,
        input.device_id.as_deref(),
        input.source_context,
        prompt_values,
    )
    .await
}

async fn run_preparation(
    generated: GeneratedFlow,
    device_id: Option<&str>,
    source_context: Option<RedactedUiContext>,
    prompt_values: std::collections::BTreeMap<String, Zeroizing<String>>,
) -> Result<TrialPreparation, String> {
    let Some(device_id) = device_id else {
        let platform = match generated.flow.platform {
            Platform::Android => "Android Emulator/设备",
            Platform::Ios => "iOS Simulator",
        };
        return Ok(TrialPreparation {
            generated,
            trial: None,
            failure: Some(DryRunFailure {
                step_path: "target".to_owned(),
                code: "target_unavailable".to_owned(),
                message: format!(
                    "未检测到 {platform}。请先准备 Reactor 内置工具并启动目标；静态编译不能替代上机试跑。"
                ),
            }),
            evidence: None,
            context: None,
            source_context,
            goal_evidence: None,
            changes: vec![],
            repair_attempts: 0,
            model_calls: 0,
            audit_path: None,
        });
    };
    let source_context = if source_context.is_none() && requires_navigation_intent(&generated.flow)
    {
        capture_navigation_source_context(&generated.flow, device_id, prompt_values.clone()).await
    } else {
        source_context
    };
    let trial = match generated.flow.platform {
        Platform::Android => {
            trial_android(
                &workspace(),
                &generated.flow,
                device_id,
                Some(prompt_values),
            )
            .await
        }
        Platform::Ios => {
            trial_ios_simulator(
                &workspace(),
                &generated.flow,
                device_id,
                Some(prompt_values),
            )
            .await
        }
    };
    match trial {
        Ok(trial) => finish_successful_trial(generated, trial, source_context).await,
        Err(error) => {
            finish_failed_trial(generated, device_id, source_context, error.to_string()).await
        }
    }
}

async fn capture_navigation_source_context(
    flow: &Flow,
    device_id: &str,
    prompt_values: std::collections::BTreeMap<String, Zeroizing<String>>,
) -> Option<RedactedUiContext> {
    let source_flow = navigation_source_flow(flow)?;
    let trial = match source_flow.platform {
        Platform::Android => {
            trial_android(&workspace(), &source_flow, device_id, Some(prompt_values))
                .await
                .ok()?
        }
        Platform::Ios => {
            trial_ios_simulator(&workspace(), &source_flow, device_id, Some(prompt_values))
                .await
                .ok()?
        }
    };
    read_trial_destination_context(&trial).await
}

fn navigation_source_flow(flow: &Flow) -> Option<Flow> {
    let marker = navigation_destination_marker(flow)?;
    let verification = flow
        .setup
        .iter()
        .enumerate()
        .map(|(index, step)| (false, index, step))
        .chain(
            flow.measured
                .iter()
                .enumerate()
                .map(|(index, step)| (true, index, step)),
        )
        .rfind(|(_, _, step)| {
            matches!(
                step,
                Step::WaitFor { target, .. } | Step::AssertVisible { target }
                    if target == &marker
            )
        })?;
    let navigation = flow
        .setup
        .iter()
        .enumerate()
        .map(|(index, step)| (false, index, step))
        .chain(
            flow.measured
                .iter()
                .enumerate()
                .map(|(index, step)| (true, index, step)),
        )
        .take_while(|(measured, index, _)| (*measured, *index) != (verification.0, verification.1))
        .filter(|(_, _, step)| matches!(step, Step::Tap { .. } | Step::InputText { .. }))
        .last()?;

    let mut setup = if navigation.0 {
        let mut steps = flow.setup.clone();
        steps.extend_from_slice(&flow.measured[..navigation.1]);
        steps
    } else {
        flow.setup[..navigation.1].to_vec()
    };
    while matches!(setup.last(), Some(Step::Pause { .. })) {
        setup.pop();
    }
    Some(Flow {
        schema_version: flow.schema_version,
        id: format!("{}-source-evidence", flow.id),
        name: format!("{} source evidence", flow.name),
        app_id: flow.app_id.clone(),
        platform: flow.platform,
        intent: None,
        setup,
        measured: vec![Step::Pause { duration_ms: 1 }],
        teardown: vec![],
    })
}

async fn finish_successful_trial(
    generated: GeneratedFlow,
    trial: FlowTrialEvidence,
    source_context: Option<RedactedUiContext>,
) -> Result<TrialPreparation, String> {
    let destination_context = read_trial_destination_context(&trial).await;
    let goal_evidence = verify_navigation_goal(
        &generated.flow,
        source_context.as_ref(),
        destination_context.as_ref(),
    );
    if requires_navigation_intent(&generated.flow)
        && !goal_evidence
            .as_ref()
            .is_some_and(|evidence| evidence.verified)
    {
        let (code, message) = goal_failure(source_context.as_ref(), goal_evidence.as_ref());
        return Ok(TrialPreparation {
            generated,
            trial: None,
            failure: Some(DryRunFailure {
                step_path: "goal.destination".to_owned(),
                code: code.to_owned(),
                message: message.to_owned(),
            }),
            evidence: None,
            context: destination_context,
            source_context,
            goal_evidence,
            changes: vec![],
            repair_attempts: 0,
            model_calls: 0,
            audit_path: None,
        });
    }
    Ok(TrialPreparation {
        generated,
        trial: Some(trial),
        failure: None,
        evidence: None,
        context: destination_context,
        source_context,
        goal_evidence,
        changes: vec![],
        repair_attempts: 0,
        model_calls: 0,
        audit_path: None,
    })
}

fn goal_failure(
    source_context: Option<&RedactedUiContext>,
    evidence: Option<&GoalEvidenceView>,
) -> (&'static str, &'static str) {
    match evidence {
        None if source_context.is_none() => (
            "source_context_required",
            "导航 Flow 需要先读取起始页面，才能证明目标页不是起始页上的元素",
        ),
        None => (
            "destination_evidence_unavailable",
            "Flow 已执行，但未能读取目标页 UI 证据，因此不能锁定",
        ),
        Some(evidence) if evidence.source_contains_marker => (
            "destination_marker_not_unique",
            "目标页验证标记在起始页已经存在，不能证明页面发生了正确转换",
        ),
        Some(_) => (
            "destination_marker_missing",
            "目标页验证标记未出现在试跑后的 UI 树中，不能证明已到达目标页",
        ),
    }
}

async fn finish_failed_trial(
    generated: GeneratedFlow,
    device_id: &str,
    source_context: Option<RedactedUiContext>,
    message: String,
) -> Result<TrialPreparation, String> {
    let captured = match generated.flow.platform {
        Platform::Android => capture_android_trial_failure(&workspace(), device_id, &message)
            .await
            .ok(),
        Platform::Ios => capture_ios_trial_failure(&workspace(), device_id, &message)
            .await
            .ok(),
    };
    let context = captured.as_ref().and_then(|evidence| {
        evidence
            .ui_tree
            .as_deref()
            .map(|tree| redact_ui_tree(tree, usize::from(evidence.screenshot_path.is_some())))
    });
    let rejected_marker = captured
        .as_ref()
        .and_then(|evidence| evidence.ui_tree.as_deref())
        .and_then(|tree| runtime_input_rejection_marker(&generated.flow, &message, tree));
    let (code, failure_message) = if let Some(marker) = rejected_marker {
        (
            "runtime_input_rejected",
            format!(
                "应用仍显示“{marker}”，本次有效账号或 Secret 被应用拒绝。请更新运行数据后重试，不要修改 Flow Selector。"
            ),
        )
    } else {
        ("automation_trial_failed", message.as_str().to_owned())
    };
    Ok(TrialPreparation {
        generated,
        trial: None,
        failure: Some(DryRunFailure {
            step_path: "trial".to_owned(),
            code: code.to_owned(),
            message: failure_message,
        }),
        evidence: captured.as_ref().map(FailureEvidenceView::from),
        context,
        source_context,
        goal_evidence: None,
        changes: vec![],
        repair_attempts: 0,
        model_calls: 0,
        audit_path: None,
    })
}

fn runtime_input_rejection_marker<'a>(
    flow: &'a Flow,
    failure_message: &str,
    ui_tree: &str,
) -> Option<&'a str> {
    let failed_index = flow.setup.iter().rposition(|step| {
        assertion_target(step)
            .and_then(selector_primary_value)
            .is_some_and(|value| failure_message.contains(value))
    })?;
    flow.setup[..failed_index]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(marker_index, step)| {
            let marker = assertion_target(step).and_then(selector_primary_value)?;
            let later_steps = &flow.setup[marker_index + 1..failed_index];
            let has_input = later_steps
                .iter()
                .any(|step| matches!(step, Step::InputText { .. }));
            let has_submit = later_steps
                .iter()
                .any(|step| matches!(step, Step::Tap { .. }));
            (has_input && has_submit && ui_tree.contains(marker)).then_some(marker)
        })
}

fn assertion_target(step: &Step) -> Option<&Selector> {
    match step {
        Step::WaitFor { target, .. } | Step::AssertVisible { target } => Some(target),
        _ => None,
    }
}

fn selector_primary_value(selector: &Selector) -> Option<&str> {
    selector
        .accessibility_id
        .as_deref()
        .or(selector.semantic_id.as_deref())
        .or(selector.text.as_deref())
}

async fn read_trial_destination_context(trial: &FlowTrialEvidence) -> Option<RedactedUiContext> {
    let directory = Path::new(trial.artifact_dir.as_deref()?);
    let filename = match trial.mode {
        reactor_protocol::TrialMode::AndroidTarget => "destination-ui-tree.xml",
        reactor_protocol::TrialMode::IosSimulator => "destination-ui-tree.csv",
        reactor_protocol::TrialMode::ProductTourValidation => return None,
    };
    let tree = tokio::fs::read_to_string(directory.join(filename))
        .await
        .ok()?;
    Some(redact_ui_tree(&tree, 0))
}

fn verify_navigation_goal(
    flow: &Flow,
    source: Option<&RedactedUiContext>,
    destination: Option<&RedactedUiContext>,
) -> Option<GoalEvidenceView> {
    let marker = navigation_destination_marker(flow)?;
    let marker = primary_selector_value(&marker)?;
    let source = source?;
    let destination = destination?;
    let source_contains_marker = tree_contains_marker(&source.ui_tree, &marker);
    let destination_contains_marker = tree_contains_marker(&destination.ui_tree, &marker);
    Some(GoalEvidenceView {
        marker,
        source_contains_marker,
        destination_contains_marker,
        source_elements: source.preview.element_count,
        destination_elements: destination.preview.element_count,
        verified: !source_contains_marker && destination_contains_marker,
    })
}

fn primary_selector_value(selector: &Selector) -> Option<String> {
    selector
        .accessibility_id
        .as_ref()
        .or(selector.semantic_id.as_ref())
        .or(selector.text.as_ref())
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn tree_contains_marker(tree: &str, marker: &str) -> bool {
    if tree.contains(marker) {
        return true;
    }
    let escaped = marker
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    tree.contains(&escaped)
}

#[tauri::command]
async fn repair_flow(input: RepairFlowInput) -> Result<TrialPreparation, String> {
    if !input.allow_model_context {
        return Err("未获得脱敏 UI 证据上传确认".to_owned());
    }
    let device_id = input.device_id;
    let provider_mode = input.provider.as_deref().unwrap_or("cloud");
    ensure_model_calls_allowed()?;
    let provider: Box<dyn FlowAiProvider> = match provider_mode {
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| {
                        "AI 修复需要 Provider API Key，密钥可保存到系统钥匙串".to_owned()
                    })?;
            Box::new(OpenAiCompatibleProvider::new(
                input.endpoint,
                api_key,
                input.model,
            ))
        }
        "codex" | "claude" => Box::new(
            CliFlowProvider::new(
                if provider_mode == "codex" {
                    CliProviderKind::Codex
                } else {
                    CliProviderKind::ClaudeCode
                },
                input.cli_executable.as_deref(),
                Some(input.model),
            )
            .map_err(|error| error.to_string())?,
        ),
        "local" => Box::new(OpenAiCompatibleProvider::new_local(
            input.endpoint,
            input.model,
        )),
        "offline" => return Err("Reactor Offline 不支持试跑自愈，请选择 AI Provider".to_owned()),
        other => return Err(format!("未知 Flow Provider: {other}")),
    };
    let (mut current, audit_attempts) = execute_repair_loop(
        input.preparation,
        provider.as_ref(),
        |repaired, source_context| {
            let device_id = device_id.clone();
            async move {
                run_preparation(
                    repaired,
                    Some(&device_id),
                    source_context,
                    std::collections::BTreeMap::default(),
                )
                .await
            }
        },
    )
    .await?;
    let audit_path = write_flow_audit(&current, &audit_attempts).await?;
    current.audit_path = Some(audit_path);
    Ok(current)
}

async fn execute_repair_loop<P, F, Fut>(
    mut current: TrialPreparation,
    provider: &P,
    mut trial: F,
) -> Result<(TrialPreparation, Vec<serde_json::Value>), String>
where
    P: FlowAiProvider + ?Sized,
    F: FnMut(GeneratedFlow, Option<RedactedUiContext>) -> Fut,
    Fut: Future<Output = Result<TrialPreparation, String>>,
{
    let original = current.generated.flow.clone();
    let mut audit_attempts = Vec::new();
    for attempt in 1..=MAX_FLOW_REPAIR_ATTEMPTS {
        let failure = current
            .failure
            .clone()
            .ok_or_else(|| "当前 Flow 没有可修复的试跑失败".to_owned())?;
        if failure.code == "runtime_input_rejected" {
            return Err("应用拒绝了本次运行数据；请更新有效账号或 Secret 后重试，AI 不会修改 Flow 来绕过登录失败。".to_owned());
        }
        if matches!(
            failure.code.as_str(),
            "source_context_required" | "destination_evidence_unavailable"
        ) {
            return Err(
                "Reactor 缺少起始页或目标页验收证据；请重新采集证据并试跑，Flow Copilot 不会修改正确的 Flow。"
                    .to_owned(),
            );
        }
        let context = current
            .context
            .as_ref()
            .map(|context| context.ui_tree.clone());
        let repaired = provider
            .repair(FlowRepairRequest {
                flow: current.generated.flow.clone(),
                failure,
                ui_tree: context,
                screenshot_artifact_ids: vec![],
            })
            .await
            .map_err(|error| error.to_string())?;
        let changes = diff_flows(&original, &repaired.flow).map_err(|error| error.to_string())?;
        if changes.is_empty() {
            return Err("AI 未产生任何 Flow 修改，已停止自愈".to_owned());
        }
        let before_hash =
            canonical_flow_hash(&current.generated.flow).map_err(|error| error.to_string())?;
        let after_hash = canonical_flow_hash(&repaired.flow).map_err(|error| error.to_string())?;
        let mut next = trial(repaired, current.source_context.clone()).await?;
        audit_attempts.push(serde_json::json!({
            "attempt": attempt,
            "beforeFlowHash": before_hash,
            "afterFlowHash": after_hash,
            "changes": changes,
            "contextPreview": current.context.as_ref().map(|context| &context.preview),
            "trialPassed": next.trial.is_some(),
        }));
        next.changes =
            diff_flows(&original, &next.generated.flow).map_err(|error| error.to_string())?;
        next.repair_attempts = attempt;
        next.model_calls = attempt;
        current = next;
        if current.trial.is_some() {
            break;
        }
    }
    Ok((current, audit_attempts))
}

async fn write_flow_audit(
    preparation: &TrialPreparation,
    attempts: &[serde_json::Value],
) -> Result<String, String> {
    let directory = workspace().join("results/audit");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|error| error.to_string())?;
    let path = directory.join(format!("flow-{}.json", uuid::Uuid::new_v4()));
    let payload = serde_json::json!({
        "schemaVersion": 1,
        "createdAt": chrono::Utc::now(),
        "operation": "flow_repair",
        "provider": preparation.generated.provider,
        "model": preparation.generated.model,
        "modelCalls": preparation.model_calls,
        "measurementWindowModelCalls": 0,
        "repairAttempts": preparation.repair_attempts,
        "attempts": attempts,
        "finalTrialPassed": preparation.trial.is_some(),
        "screenshotBytesUploaded": 0,
    });
    tokio::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())?
        ),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn confirm_flow(preparation: TrialPreparation) -> Result<FlowLock, String> {
    if preparation.failure.is_some() || preparation.trial.is_none() {
        return Err("只能锁定已通过试跑的 Flow".to_owned());
    }
    if preparation
        .trial
        .as_ref()
        .is_some_and(|trial| trial.synthetic)
    {
        return Err("静态校验不能锁定用于真实测量的 Flow；请连接目标并完成上机试跑".to_owned());
    }
    let provenance = GenerationProvenance {
        provider: preparation.generated.provider.clone(),
        model: preparation.generated.model.clone(),
        prompt_template_version: preparation.generated.prompt_template_version.clone(),
    };
    FlowLock::new_with_trial(
        preparation.generated.flow,
        Some(provenance),
        preparation.trial,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn setup_tools(input: SetupToolsInput) -> Result<InstalledManifest, String> {
    let workspace = workspace();
    if let Some(source) = bundled_tool_archives() {
        let seed_workspace = workspace.clone();
        tokio::task::spawn_blocking(move || seed_bundled_tool_archives(&source, &seed_workspace))
            .await
            .map_err(|error| error.to_string())??;
    }
    let manifest: ManagedToolsManifest =
        serde_json::from_str(MANAGED_TOOLS_MANIFEST).map_err(|error| error.to_string())?;
    reactor_toolchain::setup(
        &workspace,
        &manifest,
        &SetupOptions {
            offline: input.offline,
            proxy: input.proxy,
            maestro_override: input.maestro_override,
        },
    )
    .await
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn start_demo(flow_lock: FlowLock) -> Result<StartedJob, String> {
    let workspace = workspace();
    let job = enqueue_demo(&workspace, &flow_lock).map_err(|error| error.to_string())?;
    spawn_worker(
        &workspace,
        &job.id,
        &WorkerRequest::Demo {
            workspace: workspace.clone(),
            flow_lock: Box::new(flow_lock),
        },
    )?;
    Ok(StartedJob { job_id: job.id })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn write_new_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn persist_job_provenance(
    root: &Path,
    job_id: &str,
    flow_lock: &FlowLock,
) -> Result<PathBuf, String> {
    flow_lock.verify().map_err(|error| error.to_string())?;
    let directory = root.join("results/runs").join(job_id);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let lock_path = directory.join("flow.lock.json");
    let lock_bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(flow_lock).map_err(|error| error.to_string())?
    )
    .into_bytes();
    write_new_private_file(&lock_path, &lock_bytes)?;
    let manifest = ExecutionManifestV1 {
        schema_version: 1,
        job_id: job_id.to_owned(),
        flow_hash: flow_lock.flow_hash.clone(),
        flow_lock_sha256: sha256_bytes(&lock_bytes),
        created_at: Utc::now(),
    };
    let manifest_bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?
    )
    .into_bytes();
    let manifest_path = directory.join("execution-manifest.json");
    write_new_private_file(&manifest_path, &manifest_bytes)?;
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    store
        .register_artifact(job_id, "flow_lock", &lock_path)
        .map_err(|error| error.to_string())?;
    store
        .register_artifact(job_id, "execution_manifest", &manifest_path)
        .map_err(|error| error.to_string())?;
    Ok(lock_path)
}

fn start_android_for(root: &Path, input: RealRunInput) -> Result<(Job, AndroidRunRequest), String> {
    let staging_dir = root.join(".reactor/runtime/desktop");
    std::fs::create_dir_all(&staging_dir).map_err(|error| error.to_string())?;
    let staging_path = staging_dir.join(format!("{}.lock.json", uuid::Uuid::new_v4()));
    let staging_bytes = serde_json::to_vec(&input.flow_lock).map_err(|error| error.to_string())?;
    write_new_private_file(&staging_path, &staging_bytes)?;
    let mut request = AndroidRunRequest {
        workspace: root.to_path_buf(),
        flow_lock: staging_path.clone(),
        framework: input.framework,
        scenario: input.scenario,
        device_id: input.device_id,
        duration_ms: input.duration_ms,
        iteration_count: input.iterations,
        run_mode: input.run_mode,
        diagnostic_plan: input.diagnostic_plan,
        leak_test: input.leak_test,
        manual_session: input.manual_session,
    };
    let job = enqueue_android(&request).map_err(|error| error.to_string())?;
    let provenance = persist_job_provenance(root, &job.id, &input.flow_lock);
    let _ = std::fs::remove_file(staging_path);
    request.flow_lock = provenance?;
    Ok((job, request))
}

#[tauri::command]
fn start_android(input: RealRunInput) -> Result<StartedJob, String> {
    let workspace = workspace();
    let (job, request) = start_android_for(&workspace, input)?;
    let worker_workspace = request.workspace.clone();
    spawn_worker(
        &worker_workspace,
        &job.id,
        &WorkerRequest::Android { request },
    )?;
    Ok(StartedJob { job_id: job.id })
}

#[tauri::command]
fn start_ios(input: RealRunInput) -> Result<StartedJob, String> {
    if input.run_mode == RunMode::Diagnose || input.diagnostic_plan.is_some() {
        return Err("iOS Diagnose is not supported; use Android Diagnose".to_owned());
    }
    let workspace = workspace();
    let staging_dir = workspace.join(".reactor/runtime/desktop");
    std::fs::create_dir_all(&staging_dir).map_err(|error| error.to_string())?;
    let staging_path = staging_dir.join(format!("{}.lock.json", uuid::Uuid::new_v4()));
    let staging_bytes = serde_json::to_vec(&input.flow_lock).map_err(|error| error.to_string())?;
    write_new_private_file(&staging_path, &staging_bytes)?;
    let mut request = IosRunRequest {
        workspace: workspace.clone(),
        flow_lock: staging_path.clone(),
        framework: input.framework,
        scenario: input.scenario,
        device_id: input.device_id,
        duration_ms: input.duration_ms,
    };
    let job = enqueue_ios(&request).map_err(|error| error.to_string())?;
    let provenance = persist_job_provenance(&workspace, &job.id, &input.flow_lock);
    let _ = std::fs::remove_file(staging_path);
    request.flow_lock = provenance?;
    let worker_workspace = request.workspace.clone();
    spawn_worker(&worker_workspace, &job.id, &WorkerRequest::Ios { request })?;
    Ok(StartedJob { job_id: job.id })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_job(
    job_id: String,
    cursor: Option<i64>,
    before: Option<i64>,
    event_limit: Option<u32>,
) -> Result<JobSnapshot, String> {
    let store = Store::open(&workspace().join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let job = store
        .get_job(&job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到任务 {job_id}"))?;
    let event_limit = event_limit.unwrap_or(100).clamp(1, 500);
    let mut events = if let Some(before) = before {
        store.events_before_page(&job_id, before, event_limit + 1)
    } else {
        store.events_after_page(&job_id, cursor.unwrap_or(0), event_limit + 1)
    }
    .map_err(|error| error.to_string())?;
    let has_more_events = events.len() > event_limit as usize;
    if has_more_events {
        if before.is_some() {
            events.remove(0);
        } else {
            events.truncate(event_limit as usize);
        }
    }
    let results = job
        .result_path
        .as_deref()
        .map(read_results)
        .transpose()?
        .unwrap_or_default();
    let report_path = job.result_path.as_deref().and_then(|path| {
        Path::new(path)
            .parent()
            .map(|parent| parent.join("report.html"))
            .filter(|path| path.is_file())
            .map(|path| path.display().to_string())
    });
    Ok(JobSnapshot {
        job,
        events,
        has_more_events,
        results,
        report_path,
    })
}

#[tauri::command]
fn list_jobs(limit: Option<u32>, offset: Option<u32>) -> Result<JobPage, String> {
    let limit = limit.unwrap_or(25).clamp(1, 100);
    let offset = offset.unwrap_or(0);
    let store = Store::open(&workspace().join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    Ok(JobPage {
        jobs: store
            .list_jobs_page(limit, offset)
            .map_err(|error| error.to_string())?,
        total: store.job_count().map_err(|error| error.to_string())?,
        offset,
        limit,
    })
}

fn load_job_result(
    root: &Path,
    job_id: &str,
    run_id: &str,
) -> Result<reactor_protocol::NormalizedResult, String> {
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    store
        .get_diagnostic_result(job_id, run_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("任务 {job_id} 中找不到运行 {run_id}"))
}

fn load_historical_flow_lock_for(
    root: &Path,
    job_id: &str,
    run_id: &str,
    expected_flow_hash: Option<&str>,
) -> Result<FlowLock, String> {
    let result = load_job_result(root, job_id, run_id)?;
    if expected_flow_hash.is_some_and(|flow_hash| flow_hash != result.flow_hash) {
        return Err("请求的 flowHash 与所选 jobId/runId 不匹配".to_owned());
    }
    let run_root = root.join("results/runs");
    let job_directory = run_root.join(job_id);
    let canonical_run_root = std::fs::canonicalize(&run_root).map_err(|error| error.to_string())?;
    let canonical_job_directory = std::fs::canonicalize(&job_directory)
        .map_err(|_| "该历史运行没有可验证的执行 provenance".to_owned())?;
    if !canonical_job_directory.starts_with(&canonical_run_root)
        || canonical_job_directory.parent() != Some(canonical_run_root.as_path())
    {
        return Err("历史运行目录不属于 Reactor 的受管 job 目录".to_owned());
    }
    let lock_path = managed_existing_file(&canonical_job_directory, Path::new("flow.lock.json"))
        .ok_or_else(|| "该历史运行缺少 flow.lock.json；仍可分析，但不能加载或重跑".to_owned())?;
    let manifest_path = managed_existing_file(
        &canonical_job_directory,
        Path::new("execution-manifest.json"),
    )
    .ok_or_else(|| {
        "该历史运行缺少 execution-manifest.json；仍可分析，但不能加载或重跑".to_owned()
    })?;
    let lock_bytes = std::fs::read(&lock_path).map_err(|error| error.to_string())?;
    let manifest: ExecutionManifestV1 =
        serde_json::from_slice(&std::fs::read(&manifest_path).map_err(|error| error.to_string())?)
            .map_err(|_| "执行 manifest 无效".to_owned())?;
    if manifest.schema_version != 1
        || manifest.job_id != job_id
        || manifest.flow_hash != result.flow_hash
        || manifest.flow_lock_sha256 != sha256_bytes(&lock_bytes)
    {
        return Err("执行 provenance 完整性校验失败".to_owned());
    }
    let lock: FlowLock =
        serde_json::from_slice(&lock_bytes).map_err(|_| "历史 flow.lock.json 无效".to_owned())?;
    lock.verify().map_err(|error| error.to_string())?;
    if lock.flow_hash != result.flow_hash {
        return Err("flow.lock.json 与所选 runId 的 flowHash 不匹配".to_owned());
    }
    Ok(lock)
}

fn historical_rerun_blocking_references(flow: &Flow) -> Vec<&'static str> {
    fn inspect_steps(steps: &[Step], references: &mut std::collections::BTreeSet<&'static str>) {
        for step in steps {
            match step {
                Step::InputText { value, .. } => match value {
                    InputValue::PromptRef(_) => {
                        references.insert("promptRef");
                    }
                    InputValue::SecretRef(_) => {
                        references.insert("secretRef");
                    }
                    InputValue::TotpRef(_) => {
                        references.insert("totpRef");
                    }
                    InputValue::Literal(_) | InputValue::VariableRef(_) => {}
                },
                Step::Repeat { steps, .. } => inspect_steps(steps, references),
                _ => {}
            }
        }
    }

    let mut references = std::collections::BTreeSet::new();
    inspect_steps(&flow.setup, &mut references);
    inspect_steps(&flow.measured, &mut references);
    inspect_steps(&flow.teardown, &mut references);
    references.into_iter().collect()
}

fn diagnostic_rerun_eligibility_for(
    root: &Path,
    input: &DiagnosticRunInput,
) -> DiagnosticRerunEligibility {
    match load_historical_flow_lock_for(
        root,
        &input.job_id,
        &input.run_id,
        input.flow_hash.as_deref(),
    ) {
        Ok(lock) => {
            let platform = match lock.flow.platform {
                Platform::Android => "android",
                Platform::Ios => "ios",
            }
            .to_owned();
            let blocked_references = historical_rerun_blocking_references(&lock.flow);
            let eligible = blocked_references.is_empty();
            let reason = (!eligible).then(|| {
                format!(
                    "历史 Flow 包含需要本次重新确认的输入引用（{}）；当前历史重跑入口不接收新输入，已禁用 Benchmark/Diagnose，且不会读取旧 Prompt 或系统凭据库 Secret/TOTP",
                    blocked_references.join("、")
                )
            });
            DiagnosticRerunEligibility {
                job_id: input.job_id.clone(),
                run_id: input.run_id.clone(),
                lock_available: true,
                diagnose_available: eligible && lock.flow.platform == Platform::Android,
                platform,
                eligible,
                reason,
            }
        }
        Err(reason) => DiagnosticRerunEligibility {
            job_id: input.job_id.clone(),
            run_id: input.run_id.clone(),
            lock_available: false,
            platform: "unknown".to_owned(),
            diagnose_available: false,
            eligible: false,
            reason: Some(reason),
        },
    }
}

fn diagnostic_run_summary(
    root: &Path,
    store: &Store,
    entry: DiagnosticRunCatalogEntry,
) -> Result<DiagnosticRunSummary, String> {
    let result = store
        .get_diagnostic_result(&entry.job_id, &entry.run_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("任务 {} 中找不到运行 {}", entry.job_id, entry.run_id))?;
    let binding = DiagnosticRunInput {
        job_id: entry.job_id.clone(),
        run_id: entry.run_id.clone(),
        flow_hash: Some(entry.flow_hash.clone()),
    };
    let lock_available = diagnostic_rerun_eligibility_for(root, &binding).lock_available;
    Ok(DiagnosticRunSummary {
        job_id: entry.job_id,
        run_id: entry.run_id,
        created_at: entry.created_at,
        framework: entry.framework,
        platform: entry.platform,
        scenario: entry.scenario,
        flow_hash: entry.flow_hash,
        app_id: entry.app_id,
        app_version: entry.app_version,
        adapter: entry.adapter,
        device_name: entry.device_name,
        device_physical: entry.device_physical,
        successful_iteration_count: entry.successful_iteration_count,
        iteration_count: entry.iteration_count,
        synthetic: entry.synthetic,
        lock_available,
        result,
    })
}

fn list_diagnostic_runs_for(
    root: &Path,
    input: &DiagnosticRunsInput,
) -> Result<DiagnosticRunPage, String> {
    let limit = input.limit.unwrap_or(25).clamp(1, 100);
    let offset = input.offset.unwrap_or(0);
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let filter = DiagnosticRunFilter {
        flow_hash: input.flow_hash.as_deref(),
        framework: input.framework.as_deref(),
        ..DiagnosticRunFilter::default()
    };
    let entries = store
        .list_diagnostic_runs_page(&filter, limit, offset)
        .map_err(|error| error.to_string())?;
    let runs = entries
        .into_iter()
        .map(|entry| diagnostic_run_summary(root, &store, entry))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DiagnosticRunPage {
        runs,
        total: store
            .count_diagnostic_runs(&filter)
            .map_err(|error| error.to_string())?,
        offset,
        limit,
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn list_diagnostic_runs(input: DiagnosticRunsInput) -> Result<DiagnosticRunPage, String> {
    list_diagnostic_runs_for(&workspace(), &input)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_diagnostic_rerun_eligibility(input: DiagnosticRunInput) -> DiagnosticRerunEligibility {
    diagnostic_rerun_eligibility_for(&workspace(), &input)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn load_historical_flow_lock(input: DiagnosticRunInput) -> Result<FlowLock, String> {
    load_historical_flow_lock_for(
        &workspace(),
        &input.job_id,
        &input.run_id,
        input.flow_hash.as_deref(),
    )
}

#[tauri::command]
fn analyze_job_pair(input: AnalyzeJobPairInput) -> Result<JobAnalysis, String> {
    if input.baseline_job_id == input.current_job_id {
        return Err("基线和当前运行不能是同一个任务".to_owned());
    }
    let store = Store::open(&workspace().join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let baseline_job = store
        .get_job(&input.baseline_job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到基线任务 {}", input.baseline_job_id))?;
    let current_job = store
        .get_job(&input.current_job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到当前任务 {}", input.current_job_id))?;
    if !baseline_job.state.is_terminal() || !current_job.state.is_terminal() {
        return Err("只能分析已经结束的任务".to_owned());
    }
    let baseline_path = baseline_job
        .result_path
        .as_deref()
        .ok_or_else(|| "基线任务没有性能结果".to_owned())?;
    let current_path = current_job
        .result_path
        .as_deref()
        .ok_or_else(|| "当前任务没有性能结果".to_owned())?;
    let baseline_results = read_results(baseline_path)?;
    let current_results = read_results(current_path)?;
    if baseline_results.is_empty() || current_results.is_empty() {
        return Err("所选任务的结果为空".to_owned());
    }
    let policy = input.policy.unwrap_or_default();
    let reports = current_results
        .iter()
        .map(|current| {
            let baseline = baseline_results
                .iter()
                .find(|candidate| candidate.framework == current.framework)
                .unwrap_or(&baseline_results[0]);
            analyze_pair(baseline, current, &policy)
        })
        .collect();
    Ok(JobAnalysis {
        baseline_job,
        current_job,
        reports,
    })
}

#[tauri::command]
fn analyze_profile_json(input: AnalyzeProfileInput) -> Result<DiagnosticProfileReport, String> {
    if input.json.len() as u64 > MAX_PROFILE_JSON_BYTES {
        return Err(format!(
            "Profile 超过 {} MiB 安全上限",
            MAX_PROFILE_JSON_BYTES / 1024 / 1024
        ));
    }
    if input
        .source_map
        .as_ref()
        .is_some_and(|source_map| source_map.len() as u64 > MAX_SOURCE_MAP_BYTES)
    {
        return Err(format!(
            "Source Map 超过 {} MiB 安全上限",
            MAX_SOURCE_MAP_BYTES / 1024 / 1024
        ));
    }
    let mut report = analyze_diagnostic_profile(&input.json).map_err(|error| error.to_string())?;
    if let Some(source_map) = input.source_map {
        apply_diagnostic_source_map(&mut report, &source_map).map_err(|error| error.to_string())?;
    }
    Ok(report)
}

fn analyze_managed_profile_for(
    root: &Path,
    input: &AnalyzeManagedProfileInput,
) -> Result<DiagnosticProfileReport, String> {
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let job = store
        .get_job(&input.job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到任务 {}", input.job_id))?;
    if !job.state.is_terminal() {
        return Err("只能分析已经结束任务的受管 Profile".to_owned());
    }
    let result = load_job_result(root, &input.job_id, &input.run_id)?;
    let result_path = job
        .result_path
        .as_deref()
        .ok_or_else(|| "任务没有受管结果路径".to_owned())?;
    let managed_result_path = managed_existing_file(&root.join("results"), Path::new(result_path))
        .ok_or_else(|| "任务结果不属于 Reactor 受管目录".to_owned())?;
    let result_directory = managed_result_path
        .parent()
        .ok_or_else(|| "任务结果路径没有父目录".to_owned())?;
    let declared_profile = result
        .android_native
        .as_ref()
        .and_then(|native| native.rn_diagnostics.as_ref())
        .and_then(|diagnostics| diagnostics.profile_file.as_deref())
        .ok_or_else(|| "所选 jobId/runId 未声明 React Profile".to_owned())?;
    let declared_artifact = result
        .framework_diagnostics
        .as_ref()
        .and_then(|diagnostics| diagnostics.react_native.as_ref())
        .into_iter()
        .flat_map(|diagnostics| diagnostics.collectors.values())
        .flat_map(|collector| &collector.artifacts)
        .find(|artifact| {
            artifact.format == "react-devtools-profile-json"
                && artifact.integrity == ArtifactIntegrity::Complete
                && artifact.path == input.artifact.path
                && artifact.size_bytes == input.artifact.size_bytes
                && artifact.sha256 == input.artifact.sha256
        })
        .ok_or_else(|| {
            "请求的 Profile identity 未在所选 Run 的完整 artifact 声明中注册".to_owned()
        })?;
    let declared_name = Path::new(declared_profile).file_name();
    if declared_name.is_none() || Path::new(&declared_artifact.path).file_name() != declared_name {
        return Err("artifact 与所选 Run 声明的 profileFile 不匹配".to_owned());
    }
    let path = managed_existing_file_from(
        result_directory,
        result_directory,
        Path::new(&declared_artifact.path),
    )
    .ok_or_else(|| "Profile 不在所选任务结果目录内".to_owned())?;
    let registered = store
        .list_artifacts(&input.job_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|artifact| {
            artifact.kind == "react_native_profile"
                && Path::new(&artifact.path).canonicalize().ok().as_deref() == Some(path.as_path())
                && artifact.size_bytes == declared_artifact.size_bytes
                && artifact.sha256 == declared_artifact.sha256
        })
        .ok_or_else(|| "Profile 未以匹配 path/size/SHA-256 注册到所选 Job".to_owned())?;
    let bytes = std::fs::read(&path).map_err(|error| format!("无法读取受管 Profile：{error}"))?;
    if bytes.len() as u64 != registered.size_bytes || sha256_bytes(&bytes) != registered.sha256 {
        return Err("受管 Profile 完整性复验失败".to_owned());
    }
    if bytes.len() as u64 > MAX_PROFILE_JSON_BYTES {
        return Err(format!(
            "Profile 超过 {} MiB 安全上限",
            MAX_PROFILE_JSON_BYTES / 1024 / 1024
        ));
    }
    let json =
        String::from_utf8(bytes).map_err(|_| "受管 Profile 不是有效 UTF-8 JSON".to_owned())?;
    analyze_diagnostic_profile(&json).map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn analyze_managed_profile(
    input: AnalyzeManagedProfileInput,
) -> Result<DiagnosticProfileReport, String> {
    analyze_managed_profile_for(&workspace(), &input)
}

#[tauri::command]
fn diff_profile_reports(input: DiffProfileInput) -> ProfileDiffReport {
    let DiffProfileInput { baseline, current } = input;
    let report = diff_diagnostic_profiles(&baseline, &current);
    drop((baseline, current));
    report
}

#[tauri::command]
async fn explain_analysis(input: ExplainAnalysisInput) -> Result<AnalysisExplanation, String> {
    let provider_mode = input.provider.as_deref().unwrap_or("offline");
    if provider_mode != "offline" {
        ensure_model_calls_allowed()?;
    }
    let provider: Box<dyn AnalysisAiProvider> = match provider_mode {
        "offline" => Box::new(OfflineAnalysisExplainer),
        "local" => Box::new(OpenAiCompatibleProvider::new_local(
            input
                .endpoint
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_owned()),
            input.model.unwrap_or_else(|| "qwen2.5:7b".to_owned()),
        )),
        "cloud" => {
            let api_key =
                resolve_api_key(input.api_key, input.save_api_key, input.use_saved_api_key)?
                    .ok_or_else(|| "Cloud AI 结果解读需要 API Key".to_owned())?;
            Box::new(OpenAiCompatibleProvider::new(
                input
                    .endpoint
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_owned()),
                api_key,
                input.model.unwrap_or_else(|| "gpt-5-mini".to_owned()),
            ))
        }
        "codex" | "claude" => Box::new(
            CliFlowProvider::new(
                if provider_mode == "codex" {
                    CliProviderKind::Codex
                } else {
                    CliProviderKind::ClaudeCode
                },
                input.cli_executable.as_deref(),
                input.model,
            )
            .map_err(|error| error.to_string())?,
        ),
        other => return Err(format!("未知结果解读 Provider: {other}")),
    };
    provider
        .explain(AnalysisExplanationRequest {
            report: input.report,
        })
        .await
        .map_err(|error| error.to_string())
}

fn ensure_model_calls_allowed() -> Result<(), String> {
    let store = Store::open(&workspace().join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let active = store
        .list_jobs(100)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|job| !job.state.is_terminal());
    if let Some(job) = active {
        return Err(format!(
            "任务 {} 正在运行；Reactor 在测量任务结束前禁止调用模型",
            job.id.get(..8).unwrap_or(&job.id)
        ));
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn cancel_job(job_id: String) -> Result<Job, String> {
    reactor_runner::cancel_persisted_job(&workspace(), &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn stop_manual_diagnose(job_id: String) -> Result<Job, String> {
    reactor_runner::request_android_manual_stop(&workspace(), &job_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn open_report(path: String) -> Result<(), String> {
    let allowed_root =
        std::fs::canonicalize(workspace().join("results")).map_err(|error| error.to_string())?;
    let report = std::fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if !report.starts_with(&allowed_root)
        || report.extension().and_then(std::ffi::OsStr::to_str) != Some("html")
    {
        return Err("只允许打开 Reactor results 目录内的 HTML 报告".to_owned());
    }
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(report)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn read_results(path: &str) -> Result<Vec<reactor_protocol::NormalizedResult>, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice::<Vec<reactor_protocol::NormalizedResult>>(&bytes)
        .or_else(|_| {
            serde_json::from_slice::<reactor_protocol::NormalizedResult>(&bytes)
                .map(|result| vec![result])
        })
        .map_err(|error| error.to_string())
}

fn diagnostic_index_path(
    managed_results_root: &Path,
    result_base: &Path,
    run_id: &str,
) -> Result<PathBuf, String> {
    use sha2::{Digest as _, Sha256};

    let canonical_managed_root =
        std::fs::canonicalize(managed_results_root).map_err(|error| error.to_string())?;
    let canonical_result_base =
        std::fs::canonicalize(result_base).map_err(|error| error.to_string())?;
    if !canonical_result_base.starts_with(&canonical_managed_root) {
        return Err("诊断结果目录不属于当前工作区".to_owned());
    }
    let index_root = canonical_result_base.join("diagnostic-index");
    std::fs::create_dir_all(&index_root).map_err(|error| error.to_string())?;
    let canonical_index_root =
        std::fs::canonicalize(&index_root).map_err(|error| error.to_string())?;
    if !canonical_index_root.starts_with(&canonical_result_base) {
        return Err("诊断索引目录不属于结果目录".to_owned());
    }
    let opaque_id = hex::encode(Sha256::digest(run_id.as_bytes()));
    let run_directory = canonical_index_root.join(opaque_id);
    std::fs::create_dir_all(&run_directory).map_err(|error| error.to_string())?;
    let canonical_run_directory =
        std::fs::canonicalize(&run_directory).map_err(|error| error.to_string())?;
    if !canonical_run_directory.starts_with(&canonical_index_root) {
        return Err("诊断运行索引逃逸受管目录".to_owned());
    }
    Ok(canonical_run_directory.join("diagnostic-index.sqlite"))
}

fn diagnostic_index(job_id: &str, run_id: &str) -> Result<DiagnosticIndex, String> {
    let root = workspace();
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let job = store
        .get_job(job_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("找不到任务 {job_id}"))?;
    if !job.state.is_terminal() {
        return Err("诊断索引只能为已经结束的任务构建".to_owned());
    }
    let result = load_job_result(&root, job_id, run_id)?;
    let result_path = job
        .result_path
        .as_deref()
        .ok_or_else(|| "任务没有可索引的结果".to_owned())?;
    let managed_result_path = managed_existing_file(&root.join("results"), Path::new(result_path))
        .ok_or_else(|| "任务结果不属于 Reactor 受管目录".to_owned())?;
    let result_base = managed_result_path
        .parent()
        .ok_or_else(|| "结果路径没有父目录".to_owned())?;
    let index_path = diagnostic_index_path(&root.join("results"), result_base, &result.run_id)?;
    DiagnosticIndex::open_or_build(&index_path, result_base, &result)
        .map_err(|error| error.to_string())
}

fn diagnostic_track_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "events" => Some("runtime_events"),
        "react" => Some("react_commits"),
        "frames" => Some("frames"),
        "cpu" => Some("js_samples"),
        _ => None,
    }
}

fn diagnostic_ui_tracks() -> [(&'static str, &'static str); 5] {
    [
        ("iterations", "Iterations"),
        ("frames", "Frames"),
        ("react_commits", "React commits"),
        ("js_samples", "CPU samples"),
        ("runtime_events", "Diagnostic events"),
    ]
}

fn diagnostic_state(state: &str) -> String {
    match state {
        "available" => "available",
        "unsupported" => "unsupported",
        "failed" => "failed",
        "not_collected" => "not_collected",
        _ => "unavailable",
    }
    .to_owned()
}

fn manifest_dto(
    manifest: IndexDiagnosticManifest,
    overview: &IndexTimelineOverview,
) -> DiagnosticManifestDto {
    let tracks_by_kind = overview
        .tracks
        .iter()
        .filter_map(|track| diagnostic_track_kind(&track.kind).map(|kind| (kind, track)))
        .collect::<std::collections::BTreeMap<_, _>>();
    let availability_by_kind = manifest
        .availability
        .iter()
        .filter_map(|(kind, availability)| {
            diagnostic_track_kind(kind).map(|ui_kind| (ui_kind, availability))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let tracks = diagnostic_ui_tracks()
        .into_iter()
        .map(|(kind, default_label)| {
            let track = tracks_by_kind.get(kind).copied();
            let availability = availability_by_kind.get(kind).copied();
            DiagnosticTrackAvailabilityDto {
                kind: kind.to_owned(),
                track_id: track.map(|value| value.id),
                state: availability.map_or_else(
                    || "not_collected".to_owned(),
                    |value| diagnostic_state(&value.state),
                ),
                label: track
                    .map_or(default_label, |value| value.name.as_str())
                    .to_owned(),
                reason: availability
                    .and_then(|value| value.reason.clone())
                    .or_else(|| {
                        (kind == "iterations").then(|| {
                            "the diagnostic index does not expose an iteration track".to_owned()
                        })
                    }),
                count: availability.map_or(0, |value| value.item_count),
            }
        })
        .collect();
    DiagnosticManifestDto {
        schema_version: manifest.schema_version,
        run_id: manifest.run_id,
        range: manifest
            .start_ms
            .zip(manifest.end_ms)
            .map(|(start_ms, end_ms)| DiagnosticRangeDto { start_ms, end_ms }),
        tracks,
        warnings: manifest.warnings,
    }
}

fn timeline_item_dto(
    item: IndexTimelineItem,
    track_kinds: &std::collections::BTreeMap<i64, String>,
) -> Option<TimelineItemDto> {
    let track = track_kinds.get(&item.track_id)?.clone();
    Some(TimelineItemDto {
        id: item.id,
        track_id: item.track_id,
        track,
        item_type: item.item_type,
        start_ms: item.start_ms,
        end_ms: item.end_ms,
        label: item.label,
        severity: item.severity,
        data: item.data,
    })
}

fn diagnostic_track_map(
    overview: &IndexTimelineOverview,
) -> std::collections::BTreeMap<i64, String> {
    overview
        .tracks
        .iter()
        .filter_map(|track| {
            diagnostic_track_kind(&track.kind).map(|kind| (track.id, kind.to_owned()))
        })
        .collect()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_diagnostic_manifest(input: DiagnosticRunInput) -> Result<DiagnosticManifestDto, String> {
    let index = diagnostic_index(&input.job_id, &input.run_id)?;
    let overview = index.overview().map_err(|error| error.to_string())?;
    Ok(manifest_dto(overview.manifest.clone(), &overview))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_timeline_overview(input: DiagnosticOverviewInput) -> Result<TimelineOverviewDto, String> {
    let index = diagnostic_index(&input.job_id, &input.run_id)?;
    let overview = index.overview().map_err(|error| error.to_string())?;
    let track_kinds = diagnostic_track_map(&overview);
    let track_ids = track_kinds.keys().copied().collect::<Vec<_>>();
    let window = index
        .timeline_window(input.start_ms, input.end_ms, &track_ids, Some(20_000))
        .map_err(|error| error.to_string())?;
    let bucket_count = input.pixel_width.clamp(1, 2_000).div_ceil(4).max(1);
    let bucket_count_usize = usize::try_from(bucket_count).expect("bounded timeline bucket count");
    let bucket_width = (input.end_ms - input.start_ms) / f64::from(bucket_count);
    let mut buckets = track_kinds
        .values()
        .map(|kind| {
            (
                kind.clone(),
                vec![(0_u64, None::<f64>, 0_u64); bucket_count_usize],
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    for item in &window.items {
        let Some(kind) = track_kinds.get(&item.track_id) else {
            continue;
        };
        let index = (0..bucket_count)
            .position(|candidate| {
                item.start_ms < input.start_ms + f64::from(candidate + 1) * bucket_width
            })
            .unwrap_or(bucket_count_usize - 1);
        let bucket = &mut buckets.get_mut(kind).expect("known diagnostic track")[index];
        bucket.0 += 1;
        let duration = (item.end_ms - item.start_ms).max(0.0);
        bucket.1 = Some(bucket.1.map_or(duration, |current| current.max(duration)));
        if item
            .severity
            .as_deref()
            .is_some_and(|value| matches!(value, "slow" | "warning" | "error"))
        {
            bucket.2 += 1;
        }
    }
    Ok(TimelineOverviewDto {
        range: DiagnosticRangeDto {
            start_ms: input.start_ms,
            end_ms: input.end_ms,
        },
        tracks: buckets
            .into_iter()
            .map(|(kind, values)| TimelineOverviewTrackDto {
                kind,
                buckets: values
                    .into_iter()
                    .enumerate()
                    .map(|(index, (count, max_duration_ms, slow_count))| {
                        TimelineOverviewBucketDto {
                            start_ms: input.start_ms
                                + f64::from(u32::try_from(index).expect("bounded bucket index"))
                                    * bucket_width,
                            end_ms: if index + 1 == bucket_count_usize {
                                input.end_ms
                            } else {
                                input.start_ms
                                    + f64::from(
                                        u32::try_from(index + 1).expect("bounded bucket index"),
                                    ) * bucket_width
                            },
                            count,
                            max_duration_ms,
                            slow_count,
                        }
                    })
                    .collect(),
            })
            .collect(),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_timeline_window(input: DiagnosticWindowInput) -> Result<TimelineWindowDto, String> {
    let index = diagnostic_index(&input.job_id, &input.run_id)?;
    let overview = index.overview().map_err(|error| error.to_string())?;
    let track_kinds = diagnostic_track_map(&overview);
    let allowed_ids = input
        .track_ids
        .into_iter()
        .filter(|id| track_kinds.contains_key(id))
        .collect::<Vec<_>>();
    if allowed_ids.is_empty() {
        return Ok(TimelineWindowDto {
            range: DiagnosticRangeDto {
                start_ms: input.start_ms,
                end_ms: input.end_ms,
            },
            items: Vec::new(),
            truncated: false,
        });
    }
    let window: IndexTimelineWindow = index
        .timeline_window(input.start_ms, input.end_ms, &allowed_ids, input.limit)
        .map_err(|error| error.to_string())?;
    Ok(TimelineWindowDto {
        range: DiagnosticRangeDto {
            start_ms: window.start_ms,
            end_ms: window.end_ms,
        },
        items: window
            .items
            .into_iter()
            .filter_map(|item| timeline_item_dto(item, &track_kinds))
            .collect(),
        truncated: window.clipped,
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn analyze_diagnostic_selection(
    input: DiagnosticSelectionInput,
) -> Result<SelectionAnalysisDto, String> {
    let index = diagnostic_index(&input.job_id, &input.run_id)?;
    let analysis: IndexSelectionAnalysis = index
        .analyze_selection(input.start_ms, input.end_ms)
        .map_err(|error| error.to_string())?;
    let availability = analysis
        .availability
        .iter()
        .filter_map(|(kind, value)| {
            diagnostic_track_kind(kind).map(|ui_kind| {
                (
                    ui_kind.to_owned(),
                    DiagnosticTrackAvailabilityDto {
                        kind: ui_kind.to_owned(),
                        track_id: None,
                        state: diagnostic_state(&value.state),
                        label: ui_kind.to_owned(),
                        reason: value.reason.clone(),
                        count: value.item_count,
                    },
                )
            })
        })
        .collect();
    Ok(SelectionAnalysisDto {
        range: DiagnosticRangeDto { start_ms: analysis.start_ms, end_ms: analysis.end_ms },
        summary: "Counts and hotspots are limited to the selected time range; temporal overlap is not causal evidence.".to_owned(),
        event_count: analysis.event_count,
        frame_count: analysis.frame_count,
        slow_frame_count: analysis.slow_frame_count,
        react_commit_count: analysis.react_commit_count,
        cpu_sample_count: analysis.cpu_sample_count,
        top_functions: analysis.top_functions,
        top_components: analysis.top_components,
        availability,
        correlations: Vec::new(),
        warnings: vec!["No causal relationship is inferred from this selection.".to_owned()],
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn get_frame_drilldown(input: FrameDrilldownInput) -> Result<FrameDrilldownDto, String> {
    let drilldown: IndexFrameDrilldown = diagnostic_index(&input.job_id, &input.run_id)?
        .frame_drilldown(input.frame_id)
        .map_err(|error| error.to_string())?;
    let frame = drilldown.frame.as_ref();
    let duration_ms = frame.map(|value| (value.end_ms - value.start_ms).max(0.0));
    Ok(FrameDrilldownDto {
        available: drilldown.available,
        reason: drilldown.reason.clone(),
        frame_id: frame.map(|_| input.frame_id),
        start_ms: frame.map_or(0.0, |value| value.start_ms),
        end_ms: frame.map_or(0.0, |value| value.end_ms),
        duration_ms,
        budget_ms: None,
        classification: frame.and_then(|value| value.severity.clone()),
        details: vec![
            FrameDetailDto {
                label: "Overlapping events".to_owned(),
                value: drilldown.overlapping_events.len().to_string(),
            },
            FrameDetailDto {
                label: "React commits".to_owned(),
                value: drilldown.react_commits.len().to_string(),
            },
            FrameDetailDto {
                label: "CPU hotspots".to_owned(),
                value: drilldown.cpu_samples.len().to_string(),
            },
        ],
        correlations: Vec::new(),
        warnings: if drilldown.correlations.is_empty() {
            vec!["No indexed temporal correlation candidates are available; no causal relationship is inferred.".to_owned()]
        } else {
            vec!["Indexed correlation records are temporal candidates only and are not causal evidence.".to_owned()]
        },
    })
}

fn write_worker_request(path: &Path, request: &WorkerRequest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(request).map_err(|error| error.to_string())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn take_worker_request(path: &Path) -> Result<WorkerRequest, String> {
    let request = std::fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))?;
    std::fs::remove_file(path).map_err(|error| error.to_string())?;
    Ok(request)
}

fn prune_worker_logs(worker_dir: &Path, keep: usize) -> Result<(), String> {
    let mut logs = std::fs::read_dir(worker_dir)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(std::ffi::OsStr::to_str) == Some("log"))
                .then(|| {
                    entry
                        .metadata()
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .map(|modified| (modified, path))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    logs.sort_by_key(|(modified, path)| (*modified, path.clone()));
    let remove_count = logs.len().saturating_sub(keep);
    for (_, path) in logs.into_iter().take(remove_count) {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn spawn_worker(workspace: &Path, job_id: &str, request: &WorkerRequest) -> Result<(), String> {
    let worker_dir = workspace.join(".reactor/runtime/workers");
    std::fs::create_dir_all(&worker_dir).map_err(|error| error.to_string())?;
    prune_worker_logs(&worker_dir, MAX_WORKER_LOG_FILES.saturating_sub(1))?;
    let request_path = worker_dir.join(format!("{job_id}.json"));
    write_worker_request(&request_path, request)?;
    let stdout = File::create(worker_dir.join(format!("{job_id}.log")))
        .map_err(|error| error.to_string())?;
    let stderr = stdout.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg("--reactor-worker")
        .arg(&request_path)
        .arg(job_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Err(error) = Store::open(&workspace.join(".reactor/runtime/reactor.sqlite3"))
        .and_then(|store| store.set_worker_pid(job_id, child.id()))
    {
        let _ = child.kill();
        return Err(error.to_string());
    }
    Ok(())
}

/// Runs the hidden worker mode before Tauri initializes. The worker is the same signed binary,
/// but does not create a window and survives after the desktop UI exits.
#[must_use]
pub fn run_worker_from_args() -> bool {
    let mut args = std::env::args_os().skip(1);
    let Some(mode) = args.next() else {
        return false;
    };
    if mode == std::ffi::OsStr::new("--reactor-update-helper") {
        let result = args
            .next()
            .ok_or_else(|| "missing update transaction path".to_owned())
            .and_then(|path| {
                let parent_pid = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<u32>().ok())
                    .ok_or_else(|| "missing update parent pid".to_owned())?;
                updater::run_helper(Path::new(&path), parent_pid)
            });
        if let Err(error) = result {
            eprintln!("{error}");
        }
        return true;
    }
    if mode == std::ffi::OsStr::new("--reactor-update-health-probe") {
        let result = args
            .next()
            .ok_or_else(|| "missing health probe workspace".to_owned())
            .and_then(|root| {
                let expected_version = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| "missing expected update version".to_owned())?;
                run_update_health_probe(Path::new(&root), &expected_version)
            });
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return true;
    }
    if mode != std::ffi::OsStr::new("--reactor-worker") {
        return false;
    }
    let Some(request_path) = args.next() else {
        eprintln!("missing worker request path");
        return true;
    };
    let Some(job_id) = args.next().and_then(|value| value.into_string().ok()) else {
        eprintln!("missing worker job id");
        return true;
    };
    let request_path = PathBuf::from(request_path);
    let result = (|| -> Result<(), String> {
        let request = take_worker_request(&request_path)?;
        tokio::runtime::Runtime::new()
            .map_err(|error| error.to_string())?
            .block_on(async move {
                match request {
                    WorkerRequest::Demo {
                        workspace,
                        flow_lock,
                    } => execute_demo_job(&workspace, &flow_lock, &job_id)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    WorkerRequest::Android { request } => execute_android_job(&request, &job_id)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    WorkerRequest::Ios { request } => execute_ios_job(&request, &job_id)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                }
            })
    })();
    if let Err(error) = result {
        eprintln!("{error}");
    }
    true
}

fn run_update_health_probe(root: &Path, expected_version: &str) -> Result<(), String> {
    if expected_version != env!("CARGO_PKG_VERSION") {
        return Err(format!(
            "候选版本不匹配：期望 {expected_version}，实际 {}",
            env!("CARGO_PKG_VERSION")
        ));
    }
    let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3"))
        .map_err(|error| error.to_string())?;
    let _ = store.schema_version().map_err(|error| error.to_string())?;
    let _ = store.list_jobs(1).map_err(|error| error.to_string())?;
    let manifest: ManagedToolsManifest =
        serde_json::from_str(MANAGED_TOOLS_MANIFEST).map_err(|error| error.to_string())?;
    for required in ["maestro", "adb", "flashlight", "trace_processor"] {
        if !manifest.tools.iter().any(|tool| tool.id == required) {
            return Err(format!("候选版本缺少内置适配器声明：{required}"));
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Reactor desktop shell.
///
/// # Panics
///
/// Panics if Tauri cannot initialize the native application runtime.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                let archives = resource_dir.join("managed-tool-archives");
                if archives.is_dir() {
                    let _ = BUNDLED_TOOL_ARCHIVES.set(archives);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            maintenance_status,
            verify_update_manifest,
            stage_update,
            install_staged_update,
            create_diagnostic_bundle,
            erase_private_data,
            setup_tools,
            generate_flow,
            classify_flow_request,
            modify_flow,
            probe_flow,
            preview_generation_context,
            capture_device_inspector,
            capture_device_replay_frame,
            perform_explorer_step,
            doctor_cli_providers,
            doctor_local_model,
            compile_flow_preview,
            replay_recorded_flow,
            save_flow_secret_value,
            get_flow_secret_status,
            delete_flow_secret_value,
            trial_generated_flow,
            sample_trial_live_performance,
            repair_flow,
            confirm_flow,
            start_demo,
            start_android,
            start_ios,
            get_job,
            list_jobs,
            list_diagnostic_runs,
            get_diagnostic_rerun_eligibility,
            load_historical_flow_lock,
            analyze_job_pair,
            analyze_profile_json,
            analyze_managed_profile,
            diff_profile_reports,
            get_diagnostic_manifest,
            get_timeline_overview,
            get_timeline_window,
            analyze_diagnostic_selection,
            get_frame_drilldown,
            explain_analysis,
            cancel_job,
            stop_manual_diagnose,
            open_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running Reactor");
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use super::*;
    use reactor_protocol::Platform;

    fn temporary_workspace(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("reactor-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    struct MockRepairProvider {
        calls: AtomicU32,
    }

    #[async_trait::async_trait]
    impl FlowAiProvider for MockRepairProvider {
        fn id(&self) -> &'static str {
            "mock-repair"
        }

        async fn generate(
            &self,
            _request: FlowGenerationRequest,
        ) -> Result<GeneratedFlow, reactor_ai::AiProviderError> {
            Err(reactor_ai::AiProviderError::Unavailable(
                "generation is not used by this test".to_owned(),
            ))
        }

        async fn repair(
            &self,
            request: FlowRepairRequest,
        ) -> Result<GeneratedFlow, reactor_ai::AiProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let mut flow = request.flow;
            flow.name = format!("Mock repair {call}");
            Ok(GeneratedFlow {
                flow,
                provider: self.id().to_owned(),
                model: "deterministic-test-double".to_owned(),
                prompt_template_version: "reactor-flow-v1".to_owned(),
                notes: vec![],
            })
        }

        async fn modify(
            &self,
            request: FlowModificationRequest,
        ) -> Result<GeneratedFlow, reactor_ai::AiProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                return Err(reactor_ai::AiProviderError::InvalidResponse(
                    "setup[2]: selector must not be empty".to_owned(),
                ));
            }
            let mut flow = request.flow;
            flow.name = "Schema-corrected modification".to_owned();
            Ok(GeneratedFlow {
                flow,
                provider: self.id().to_owned(),
                model: "deterministic-test-double".to_owned(),
                prompt_template_version: "reactor-flow-v1".to_owned(),
                notes: vec![],
            })
        }
    }

    fn failed_preparation(generated: GeneratedFlow) -> TrialPreparation {
        TrialPreparation {
            generated,
            trial: None,
            failure: Some(DryRunFailure {
                step_path: "$.measured[1]".to_owned(),
                code: "selector_not_found".to_owned(),
                message: "target is absent".to_owned(),
            }),
            evidence: None,
            context: None,
            source_context: None,
            goal_evidence: None,
            changes: vec![],
            repair_attempts: 0,
            model_calls: 0,
            audit_path: None,
        }
    }

    fn navigation_flow() -> Flow {
        Flow {
            schema_version: 1,
            id: "generic-list".to_owned(),
            name: "Generic list".to_owned(),
            app_id: "com.example.anyapp".to_owned(),
            platform: Platform::Android,
            intent: Some("进入列表页面并滚动".to_owned()),
            setup: vec![
                reactor_protocol::Step::Tap {
                    target: Selector {
                        text: Some("Open catalog".to_owned()),
                        ..Selector::default()
                    },
                },
                reactor_protocol::Step::AssertVisible {
                    target: Selector {
                        accessibility_id: Some("catalog-screen".to_owned()),
                        ..Selector::default()
                    },
                },
            ],
            measured: vec![reactor_protocol::Step::Swipe {
                direction: reactor_protocol::SwipeDirection::Up,
                duration_ms: 500,
            }],
            teardown: vec![],
        }
    }

    fn login_flow() -> Flow {
        Flow {
            schema_version: 1,
            id: "login".to_owned(),
            name: "Login".to_owned(),
            app_id: "com.example.login".to_owned(),
            platform: Platform::Android,
            intent: Some("先验证失败登录，再使用有效账号登录".to_owned()),
            setup: vec![
                Step::WaitFor {
                    target: Selector {
                        text: Some("Username".to_owned()),
                        ..Selector::default()
                    },
                    timeout_ms: 10_000,
                },
                Step::InputText {
                    target: Selector {
                        text: Some("Username".to_owned()),
                        ..Selector::default()
                    },
                    value: InputValue::PromptRef(reactor_protocol::PromptInputReference {
                        prompt_ref: "invalid_username".to_owned(),
                    }),
                    clear_before: true,
                },
                Step::Tap {
                    target: Selector {
                        text: Some("Sign in".to_owned()),
                        ..Selector::default()
                    },
                },
                Step::WaitFor {
                    target: Selector {
                        text: Some("Invalid username or password".to_owned()),
                        ..Selector::default()
                    },
                    timeout_ms: 10_000,
                },
                Step::InputText {
                    target: Selector {
                        text: Some("Username".to_owned()),
                        ..Selector::default()
                    },
                    value: InputValue::PromptRef(reactor_protocol::PromptInputReference {
                        prompt_ref: "valid_username".to_owned(),
                    }),
                    clear_before: true,
                },
                Step::Tap {
                    target: Selector {
                        text: Some("Sign in".to_owned()),
                        ..Selector::default()
                    },
                },
                Step::WaitFor {
                    target: Selector {
                        text: Some("Home ready".to_owned()),
                        ..Selector::default()
                    },
                    timeout_ms: 10_000,
                },
            ],
            measured: vec![Step::Pause { duration_ms: 1 }],
            teardown: vec![],
        }
    }

    #[test]
    fn retained_rejection_marker_is_classified_as_runtime_input_failure() {
        let flow = login_flow();
        let marker = runtime_input_rejection_marker(
            &flow,
            "Assertion is false: Home ready is visible",
            r#"<node text="Invalid username or password" /><node text="Username" />"#,
        );
        assert_eq!(marker, Some("Invalid username or password"));
    }

    #[test]
    fn missing_rejection_marker_keeps_automation_failure_repairable() {
        let flow = login_flow();
        let marker = runtime_input_rejection_marker(
            &flow,
            "Assertion is false: Home ready is visible",
            r#"<node text="Unexpected screen" />"#,
        );
        assert_eq!(marker, None);
    }

    #[test]
    fn runtime_goal_proof_requires_a_marker_unique_to_the_destination() {
        let source = redact_ui_tree(
            r#"<hierarchy><node text="Open catalog" resource-id="home" /></hierarchy>"#,
            0,
        );
        let destination = redact_ui_tree(
            r#"<hierarchy><node text="Catalog" resource-id="catalog-screen" /></hierarchy>"#,
            0,
        );
        let proof = verify_navigation_goal(&navigation_flow(), Some(&source), Some(&destination))
            .expect("navigation flow has a marker");
        assert!(proof.verified);
        assert!(!proof.source_contains_marker);
        assert!(proof.destination_contains_marker);

        let ambiguous_source = redact_ui_tree(
            r#"<hierarchy><node text="Hidden catalog" resource-id="catalog-screen" /></hierarchy>"#,
            0,
        );
        let proof = verify_navigation_goal(
            &navigation_flow(),
            Some(&ambiguous_source),
            Some(&destination),
        )
        .expect("navigation flow has a marker");
        assert!(!proof.verified);
        assert!(proof.source_contains_marker);
    }

    #[test]
    fn navigation_source_flow_stops_immediately_before_the_final_navigation() {
        let mut flow = navigation_flow();
        flow.setup.insert(0, Step::LaunchApp);
        flow.setup.insert(
            1,
            Step::WaitFor {
                target: Selector {
                    text: Some("Home ready".to_owned()),
                    ..Selector::default()
                },
                timeout_ms: 10_000,
            },
        );

        let source = navigation_source_flow(&flow).expect("navigation source flow");
        assert_eq!(source.setup.len(), 2);
        assert!(matches!(source.setup[0], Step::LaunchApp));
        assert!(matches!(source.setup[1], Step::WaitFor { .. }));
        assert_eq!(source.measured, vec![Step::Pause { duration_ms: 1 }]);
        assert!(source.intent.is_none());
        assert!(
            !source
                .setup
                .iter()
                .any(|step| matches!(step, Step::Tap { .. }))
        );
    }

    #[test]
    fn stable_update_policy_requires_signed_staged_updates_and_rollback() {
        let policy = update_policy();
        assert_eq!(policy.default_channel, "stable");
        assert_eq!(policy.manifest_schema_version, 1);
        assert_eq!(policy.signature_algorithm, "Ed25519");
        assert!(policy.signature_required);
        assert!(policy.staged_install);
        assert!(policy.rollback_on_failed_health_check);
        assert!(policy.stable_endpoint.starts_with("https://"));
        assert!(policy.compatibility_line.contains("Flow v1"));
    }

    #[test]
    fn candidate_health_probe_checks_version_database_history_and_tools() {
        let root = temporary_workspace("update-health");
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        store
            .create_job(&serde_json::json!({ "mode": "demo" }))
            .unwrap();
        drop(store);
        run_update_health_probe(&root, env!("CARGO_PKG_VERSION")).unwrap();
        assert!(run_update_health_probe(&root, "99.0.0").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_manifest_rejects_tampering_and_incompatible_protocols() {
        use ring::signature::KeyPair as _;

        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let mut manifest = UpdateManifestV1 {
            schema_version: 1,
            channel: "stable".to_owned(),
            version: "1.0.1".to_owned(),
            published_at: "2026-08-19T12:00:00Z".to_owned(),
            compatibility: UpdateCompatibility {
                minimum_app_version: "1.0.0".to_owned(),
                database_schema: 2,
                flow_schemas: vec![1],
                result_schemas: vec![1],
            },
            artifacts: vec![UpdateArtifact {
                platform: "darwin".to_owned(),
                arch: "aarch64".to_owned(),
                url: "https://example.test/Reactor.app.tar.gz".to_owned(),
                sha256: "a".repeat(64),
                size: 1024,
            }],
            signature: UpdateSignature {
                algorithm: "Ed25519".to_owned(),
                key_id: "release-2026".to_owned(),
                value: String::new(),
            },
        };
        manifest.signature.value = BASE64_STANDARD.encode(
            key_pair
                .sign(&signed_update_payload(&manifest).unwrap())
                .as_ref(),
        );
        let public_key = BASE64_STANDARD.encode(key_pair.public_key().as_ref());
        validate_signed_update_manifest(&manifest, &public_key, 2).unwrap();

        manifest.version = "1.0.2".to_owned();
        assert!(validate_signed_update_manifest(&manifest, &public_key, 2).is_err());
        manifest.compatibility.flow_schemas = vec![2];
        assert!(validate_signed_update_manifest(&manifest, &public_key, 2).is_err());
    }

    #[test]
    fn diagnostic_bundle_contains_only_bounded_non_sensitive_metadata() {
        let root = temporary_workspace("diagnostic");
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        store
            .create_job(&serde_json::json!({
                "apiKey": "super-secret-token",
                "email": "private@example.com"
            }))
            .unwrap();
        let evidence = root.join("results/trials/one/failure");
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::write(evidence.join("screenshot.png"), b"private pixels").unwrap();
        std::fs::write(
            evidence.join("ui-tree.xml"),
            b"<node text='private@example.com'/>",
        )
        .unwrap();

        let bundle = create_diagnostic_bundle_for(&root).unwrap();
        let json = std::fs::read_to_string(&bundle.path).unwrap();
        assert!(!json.contains("super-secret-token"));
        assert!(!json.contains("private@example.com"));
        assert!(!json.contains(root.to_string_lossy().as_ref()));
        assert!(!bundle.credential_values_included);
        assert!(!bundle.screenshots_included);
        assert!(!bundle.ui_trees_included);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["historyCount"], 1);
        assert_eq!(value["sensitiveArtifactCount"], 2);
        assert_eq!(value["resourcePolicy"]["externalPluginsEnabled"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sensitive_erase_uses_registered_kind_and_worker_provenance_without_broad_deletion() {
        let root = temporary_workspace("privacy");
        let evidence = root.join("results/runs/one");
        let workers = root.join(".reactor/runtime/workers");
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::create_dir_all(&workers).unwrap();
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "kind": "android" }))
            .unwrap();
        let screenshot = evidence.join("screenshot.png");
        let unrelated_trace = evidence.join("user-trace-not-registered.pftrace");
        std::fs::write(&screenshot, b"pixels").unwrap();
        std::fs::write(&unrelated_trace, b"unrelated").unwrap();

        let registered = [
            ("perfetto_trace", "performance.pftrace"),
            ("react_native_hermes_cpu", "cpu.cpuprofile"),
            ("react_native_profile", "react-profile.json"),
            ("react_native_diagnostics", "rn-events.ndjson"),
            ("react_native_hermes_heap_snapshot", "heap.heapsnapshot"),
        ];
        for (kind, name) in registered {
            let path = evidence.join(name);
            std::fs::write(&path, kind).unwrap();
            store.register_artifact(&job.id, kind, &path).unwrap();
        }
        let request = workers.join("stale.json");
        let log = workers.join("stale.log");
        let unrelated_worker_file = workers.join("README.txt");
        std::fs::write(&request, b"secret request").unwrap();
        std::fs::write(&log, b"secret log").unwrap();
        std::fs::write(&unrelated_worker_file, b"keep").unwrap();
        drop(store);

        let result = erase_sensitive_files(&root).unwrap();
        assert_eq!(result.removed_files, 8);
        assert!(!screenshot.exists());
        for (_, name) in registered {
            assert!(!evidence.join(name).exists());
        }
        assert!(!request.exists());
        assert!(!log.exists());
        assert!(unrelated_trace.exists());
        assert!(unrelated_worker_file.exists());
        assert!(!result.full_reset);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_index_hashes_traversal_run_ids_and_stays_canonically_contained() {
        let root = temporary_workspace("diagnostic-index-traversal");
        let managed_results = root.join("results");
        let results = managed_results.join("runs/job");
        std::fs::create_dir_all(&results).unwrap();
        let path =
            diagnostic_index_path(&managed_results, &results, "../../outside/../persisted-run")
                .unwrap();
        let canonical_parent = path.parent().unwrap().canonicalize().unwrap();
        let canonical_index_root = results.join("diagnostic-index").canonicalize().unwrap();
        assert!(canonical_parent.starts_with(&canonical_index_root));
        assert_eq!(
            canonical_parent.parent(),
            Some(canonical_index_root.as_path())
        );
        assert_eq!(
            canonical_parent
                .file_name()
                .unwrap()
                .to_string_lossy()
                .len(),
            64
        );
        assert!(!root.join("outside").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_request_is_private_consumed_after_deserialization_and_logs_are_bounded() {
        let root = temporary_workspace("worker-request");
        let workers = root.join(".reactor/runtime/workers");
        std::fs::create_dir_all(&workers).unwrap();
        let request_path = workers.join("job.json");
        let request = WorkerRequest::Ios {
            request: IosRunRequest {
                workspace: root.clone(),
                flow_lock: root.join("flow.lock.json"),
                framework: "react-native".to_owned(),
                scenario: "list".to_owned(),
                device_id: "simulator".to_owned(),
                duration_ms: 1_000,
            },
        };
        write_worker_request(&request_path, &request).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&request_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let taken = take_worker_request(&request_path).unwrap();
        assert!(matches!(taken, WorkerRequest::Ios { .. }));
        assert!(!request_path.exists());

        for index in 0..25 {
            std::fs::write(workers.join(format!("{index:02}.log")), b"log").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        prune_worker_logs(&workers, MAX_WORKER_LOG_FILES).unwrap();
        let log_count = std::fs::read_dir(&workers)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(std::ffi::OsStr::to_str) == Some("log")
            })
            .count();
        assert_eq!(log_count, MAX_WORKER_LOG_FILES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_import_rejects_content_above_the_published_limit() {
        let input = AnalyzeProfileInput {
            json: "x".repeat(usize::try_from(MAX_PROFILE_JSON_BYTES + 1).unwrap()),
            source_map: None,
        };
        assert!(
            analyze_profile_json(input)
                .unwrap_err()
                .contains("安全上限")
        );
    }

    #[test]
    fn inspector_png_dimensions_and_ios_scale_are_deterministic() {
        let mut png = Vec::from(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice());
        png.extend_from_slice(&1179_u32.to_be_bytes());
        png.extend_from_slice(&2556_u32.to_be_bytes());
        assert_eq!(png_dimensions(&png).unwrap(), (1179, 2556));

        let hierarchy = "element_num,depth,attributes,parent_num\n1,0,\"accessibilityText=App; bounds=[0,0][393,852]; enabled=true\",\n";
        let elements = inspect_hierarchy(Platform::Ios, hierarchy).unwrap();
        assert_eq!(
            inspector_viewport(Platform::Ios, 1179, 2556, &elements),
            (393.0, 852.0)
        );
        assert!(png_dimensions(b"not a png").is_err());
    }

    #[test]
    fn inspector_capture_is_rejected_while_a_job_is_active() {
        let root = temporary_workspace("inspector-active-job");
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        store
            .create_job(&serde_json::json!({ "kind": "android" }))
            .unwrap();
        let error = ensure_inspector_capture_allowed(&root).unwrap_err();
        assert!(error.contains("运行中的测试任务"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn injected_provider_cannot_exceed_two_repair_attempts() {
        let generated = GeneratedFlow {
            flow: navigation_flow(),
            provider: "test-double".to_owned(),
            model: "deterministic-test-double".to_owned(),
            prompt_template_version: "test-v1".to_owned(),
            notes: vec![],
        };
        let provider = MockRepairProvider {
            calls: AtomicU32::new(0),
        };
        let trial_calls = Arc::new(AtomicU32::new(0));
        let counted_trials = Arc::clone(&trial_calls);

        let (result, audit) = execute_repair_loop(
            failed_preparation(generated),
            &provider,
            move |generated, _source_context| {
                counted_trials.fetch_add(1, Ordering::SeqCst);
                async move { Ok(failed_preparation(generated)) }
            },
        )
        .await
        .unwrap();

        assert_eq!(
            provider.calls.load(Ordering::SeqCst),
            MAX_FLOW_REPAIR_ATTEMPTS
        );
        assert_eq!(trial_calls.load(Ordering::SeqCst), MAX_FLOW_REPAIR_ATTEMPTS);
        assert_eq!(result.repair_attempts, MAX_FLOW_REPAIR_ATTEMPTS);
        assert_eq!(result.model_calls, MAX_FLOW_REPAIR_ATTEMPTS);
        assert!(result.failure.is_some());
        assert!(result.trial.is_none());
        assert_eq!(audit.len(), MAX_FLOW_REPAIR_ATTEMPTS as usize);
    }

    #[test]
    fn diagnostic_track_mapping_keeps_numeric_ids_separate_from_ui_kinds() {
        assert_eq!(diagnostic_track_kind("events"), Some("runtime_events"));
        assert_eq!(diagnostic_track_kind("react"), Some("react_commits"));
        assert_eq!(diagnostic_track_kind("cpu"), Some("js_samples"));
        assert_eq!(diagnostic_track_kind("correlations"), None);
        assert_eq!(diagnostic_state("unavailable"), "unavailable");
        assert_eq!(diagnostic_ui_tracks()[0].0, "iterations");
    }

    fn completed_diagnostic_job(root: &Path, with_provenance: bool) -> (Job, FlowLock, String) {
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let job = store
            .create_job(&serde_json::json!({ "runMode": "diagnose" }))
            .unwrap();
        let lock = FlowLock::new(navigation_flow(), None).unwrap();
        let result_dir = root.join("results/runs").join(&job.id);
        std::fs::create_dir_all(&result_dir).unwrap();
        let mut result: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/result-v1-diagnostics.json"
        ))
        .unwrap();
        result["runId"] = serde_json::Value::String(format!("{}-diagnose", job.id));
        result["flowHash"] = serde_json::Value::String(lock.flow_hash.clone());
        let run_id = result["runId"].as_str().unwrap().to_owned();
        let result_path = result_dir.join("result.json");
        std::fs::write(&result_path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
        store.index_result(&job.id, &run_id, None, &result).unwrap();
        if with_provenance {
            persist_job_provenance(root, &job.id, &lock).unwrap();
        }
        for state in [
            reactor_store::JobState::Preflight,
            reactor_store::JobState::Warmup,
            reactor_store::JobState::Measuring,
            reactor_store::JobState::Normalizing,
            reactor_store::JobState::Completed,
        ] {
            store
                .transition(
                    &job.id,
                    state,
                    "test",
                    (state == reactor_store::JobState::Completed)
                        .then(|| result_path.display().to_string())
                        .as_deref(),
                    None,
                )
                .unwrap();
        }
        (store.get_job(&job.id).unwrap().unwrap(), lock, run_id)
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn managed_profile_requires_exact_run_declaration_registration_and_integrity() {
        let root = temporary_workspace("managed-profile-integrity");
        let (job, _, run_id) = completed_diagnostic_job(&root, true);
        let result_dir = root.join("results/runs").join(&job.id);
        let profile_path = result_dir.join("react-profile.json");
        let profile_bytes =
            include_bytes!("../../../../tests/fixtures/react-profiler-baseline.json");
        std::fs::write(&profile_path, profile_bytes).unwrap();
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let registered = store
            .register_artifact(&job.id, "react_native_profile", &profile_path)
            .unwrap();
        let mut result = store
            .get_diagnostic_result(&job.id, &run_id)
            .unwrap()
            .unwrap();
        let relative_path = "react-profile.json";
        let artifact = reactor_protocol::ArtifactRef {
            path: relative_path.to_owned(),
            format: "react-devtools-profile-json".to_owned(),
            size_bytes: registered.size_bytes,
            sha256: registered.sha256.clone(),
            producer: "test-react-runtime".to_owned(),
            producer_version: "schema-1".to_owned(),
            capture_method: "test-managed-export".to_owned(),
            integrity: ArtifactIntegrity::Complete,
            time_range: None,
        };
        result.android_native = Some(reactor_protocol::AndroidNativeMetrics {
            schema_version: 1,
            definitions_version: "test".to_owned(),
            collector: "test".to_owned(),
            trace_processor_version: "test".to_owned(),
            perfetto_trace_file: "trace".to_owned(),
            frame_count: 0,
            frame_time_mean_ms: None,
            frame_time_p50_ms: None,
            frame_time_p95_ms: None,
            frame_time_p99_ms: None,
            jank_frame_count: 0,
            jank_frame_pct: None,
            over_budget_frame_pct: None,
            startup_time_ms: None,
            memory_pss_mb: None,
            thermal_status_before: None,
            thermal_status_after: None,
            memory_leak: None,
            rn_diagnostics: Some(reactor_protocol::ReactNativeDiagnosticsSummary {
                schema_version: 1,
                collector: "test-react-runtime".to_owned(),
                benchmark_mode: Some("diagnose".to_owned()),
                event_file: "events.ndjson".to_owned(),
                event_count: 0,
                component_names: Vec::new(),
                component_render_count: 0,
                component_tree_commit_count: 0,
                profile_commit_count: 1,
                console_event_count: 0,
                network_event_count: 0,
                hermes_heap_sample_count: 0,
                allocated_object_count: 0,
                retained_object_count: 0,
                retained_bytes: 0,
                profile_file: Some(relative_path.to_owned()),
                hermes_heap_stats_file: None,
                hermes_heap_snapshot_file: None,
                java_heap_dump_file: None,
                recent_events: Vec::new(),
                warnings: Vec::new(),
            }),
            warnings: Vec::new(),
        });
        result.framework_diagnostics = Some(reactor_protocol::FrameworkDiagnosticsV1 {
            react_native: Some(reactor_protocol::ReactNativeFrameworkDiagnosticsV1 {
                collectors: std::collections::BTreeMap::from([(
                    "react-runtime".to_owned(),
                    reactor_protocol::CollectorDiagnosticV1 {
                        status: reactor_protocol::CollectorStatus::Collected,
                        artifacts: vec![artifact.clone()],
                        reason: None,
                    },
                )]),
            }),
        });
        store
            .index_result(
                &job.id,
                &run_id,
                result.device.id.as_deref(),
                &serde_json::to_value(&result).unwrap(),
            )
            .unwrap();
        let input = AnalyzeManagedProfileInput {
            job_id: job.id.clone(),
            run_id: run_id.clone(),
            artifact: ManagedProfileArtifactInput {
                path: artifact.path,
                size_bytes: artifact.size_bytes,
                sha256: artifact.sha256,
            },
        };
        assert_eq!(
            analyze_managed_profile_for(&root, &input)
                .unwrap()
                .profile_type,
            reactor_analysis::DiagnosticProfileType::ReactProfiler
        );
        std::fs::write(&profile_path, b"{}").unwrap();
        assert!(
            analyze_managed_profile_for(&root, &input)
                .unwrap_err()
                .contains("完整性")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_flow_lock_requires_exact_job_run_binding_and_valid_hashes() {
        let root = temporary_workspace("historical-flow-lock");
        let (job, lock, run_id) = completed_diagnostic_job(&root, true);
        assert_eq!(
            load_historical_flow_lock_for(&root, &job.id, &run_id, None).unwrap(),
            lock
        );
        assert!(
            load_historical_flow_lock_for(&root, &job.id, "another-run", None)
                .unwrap_err()
                .contains("找不到运行")
        );
        assert!(
            load_historical_flow_lock_for(&root, &job.id, &run_id, Some("wrong-flow"))
                .unwrap_err()
                .contains("flowHash")
        );
        std::fs::write(
            root.join("results/runs")
                .join(&job.id)
                .join("flow.lock.json"),
            b"{}",
        )
        .unwrap();
        assert!(
            load_historical_flow_lock_for(&root, &job.id, &run_id, None)
                .unwrap_err()
                .contains("完整性")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_diagnostic_results_remain_listable_but_cannot_load_or_rerun() {
        let root = temporary_workspace("old-diagnostic-run");
        let (job, _, run_id) = completed_diagnostic_job(&root, false);
        let page = list_diagnostic_runs_for(
            &root,
            &DiagnosticRunsInput {
                limit: Some(10),
                offset: Some(0),
                flow_hash: None,
                framework: None,
            },
        )
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.runs[0].job_id, job.id);
        assert_eq!(page.runs[0].run_id, run_id);
        assert_eq!(page.runs[0].result.run_id, page.runs[0].run_id);
        assert_eq!(page.runs[0].result.flow_hash, page.runs[0].flow_hash);
        assert!(!page.runs[0].lock_available);
        let eligibility = diagnostic_rerun_eligibility_for(
            &root,
            &DiagnosticRunInput {
                job_id: job.id.clone(),
                run_id: run_id.clone(),
                flow_hash: None,
            },
        );
        assert!(!eligibility.eligible);
        assert!(load_historical_flow_lock_for(&root, &job.id, &run_id, None).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_sensitive_or_prompt_inputs_are_loadable_but_not_rerunnable() {
        let root = temporary_workspace("historical-input-fail-closed");
        let (job, _, run_id) = completed_diagnostic_job(&root, true);
        let mut flow = navigation_flow();
        flow.setup.insert(
            0,
            Step::InputText {
                target: Selector {
                    text: Some("Username".to_owned()),
                    ..Selector::default()
                },
                value: InputValue::PromptRef(reactor_protocol::PromptInputReference {
                    prompt_ref: "account.username".to_owned(),
                }),
                clear_before: true,
            },
        );
        let lock = FlowLock::new(flow, None).unwrap();
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let mut result = store
            .get_diagnostic_result(&job.id, &run_id)
            .unwrap()
            .unwrap();
        result.flow_hash = lock.flow_hash.clone();
        store
            .index_result(
                &job.id,
                &run_id,
                result.device.id.as_deref(),
                &serde_json::to_value(&result).unwrap(),
            )
            .unwrap();
        let directory = root.join("results/runs").join(&job.id);
        std::fs::remove_file(directory.join("flow.lock.json")).unwrap();
        std::fs::remove_file(directory.join("execution-manifest.json")).unwrap();
        persist_job_provenance(&root, &job.id, &lock).unwrap();

        assert!(load_historical_flow_lock_for(&root, &job.id, &run_id, None).is_ok());
        let eligibility = diagnostic_rerun_eligibility_for(
            &root,
            &DiagnosticRunInput {
                job_id: job.id,
                run_id,
                flow_hash: Some(lock.flow_hash),
            },
        );
        assert!(eligibility.lock_available);
        assert!(!eligibility.eligible);
        assert!(!eligibility.diagnose_available);
        assert!(eligibility.reason.unwrap().contains("promptRef"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_ios_flow_is_loadable_and_benchmark_eligible_but_not_diagnosable() {
        let root = temporary_workspace("ios-historical-eligibility");
        let (job, mut lock, run_id) = completed_diagnostic_job(&root, true);
        lock.flow.platform = Platform::Ios;
        lock = FlowLock::new(lock.flow, lock.generation).unwrap();

        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let mut result = store
            .get_diagnostic_result(&job.id, &run_id)
            .unwrap()
            .unwrap();
        result.platform = "ios".to_owned();
        result.flow_hash = lock.flow_hash.clone();
        store
            .index_result(
                &job.id,
                &run_id,
                result.device.id.as_deref(),
                &serde_json::to_value(&result).unwrap(),
            )
            .unwrap();
        let directory = root.join("results/runs").join(&job.id);
        std::fs::remove_file(directory.join("flow.lock.json")).unwrap();
        std::fs::remove_file(directory.join("execution-manifest.json")).unwrap();
        persist_job_provenance(&root, &job.id, &lock).unwrap();

        let eligibility = diagnostic_rerun_eligibility_for(
            &root,
            &DiagnosticRunInput {
                job_id: job.id.clone(),
                run_id,
                flow_hash: Some(lock.flow_hash.clone()),
            },
        );
        assert!(eligibility.lock_available);
        assert!(eligibility.eligible);
        assert_eq!(eligibility.platform, "ios");
        assert!(!eligibility.diagnose_available);
        assert!(eligibility.reason.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provenance_files_are_job_scoped_immutable_and_contain_no_resolved_values() {
        let root = temporary_workspace("provenance-security");
        let store = Store::open(&root.join(".reactor/runtime/reactor.sqlite3")).unwrap();
        let job = store.create_job(&serde_json::json!({})).unwrap();
        let lock = FlowLock::new(login_flow(), None).unwrap();
        let path = persist_job_provenance(&root, &job.id, &lock).unwrap();
        assert_eq!(
            path,
            root.join("results/runs")
                .join(&job.id)
                .join("flow.lock.json")
        );
        let lock_text = std::fs::read_to_string(&path).unwrap();
        let manifest_text = std::fs::read_to_string(
            root.join("results/runs")
                .join(&job.id)
                .join("execution-manifest.json"),
        )
        .unwrap();
        assert!(lock_text.contains("promptRef"));
        assert!(!lock_text.contains("resolved-password"));
        assert!(!manifest_text.contains("promptRef"));
        assert!(persist_job_provenance(&root, &job.id, &lock).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_modification_gets_one_bounded_schema_correction() {
        let provider = MockRepairProvider {
            calls: AtomicU32::new(0),
        };
        let generated = modify_with_schema_retry(
            &provider,
            FlowModificationRequest {
                flow: navigation_flow(),
                instruction: "repair the observed failure".to_owned(),
                failure_context: Some("selector_not_found".to_owned()),
                ui_tree: Some("text=List scenario".to_owned()),
            },
        )
        .await
        .unwrap();

        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert_eq!(generated.flow.name, "Schema-corrected modification");
    }
}
