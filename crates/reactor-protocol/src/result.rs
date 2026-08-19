use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedResult {
    pub schema_version: u32,
    pub run_id: String,
    pub created_at: DateTime<Utc>,
    pub framework: String,
    pub platform: String,
    pub scenario: String,
    pub adapter: String,
    pub build_mode: String,
    pub flow_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub device: DeviceMetadata,
    pub source: ResultSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub android_native: Option<AndroidNativeMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ios_native: Option<IosNativeMetrics>,
    pub iterations: Vec<IterationMetrics>,
    pub summary: MetricSummary,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidNativeMetrics {
    pub schema_version: u32,
    pub definitions_version: String,
    pub collector: String,
    pub trace_processor_version: String,
    pub perfetto_trace_file: String,
    pub frame_count: u64,
    pub frame_time_mean_ms: Option<f64>,
    pub frame_time_p50_ms: Option<f64>,
    pub frame_time_p95_ms: Option<f64>,
    pub frame_time_p99_ms: Option<f64>,
    pub jank_frame_count: u64,
    pub jank_frame_pct: Option<f64>,
    pub over_budget_frame_pct: Option<f64>,
    pub startup_time_ms: Option<f64>,
    pub memory_pss_mb: Option<f64>,
    pub thermal_status_before: Option<u32>,
    pub thermal_status_after: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_leak: Option<AndroidMemoryLeakReport>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidMemoryCheckpoint {
    pub kind: String,
    pub cycle: u32,
    pub elapsed_ms: u64,
    pub cpu_pct: Option<f64>,
    pub pss_mb: Option<f64>,
    pub rss_mb: Option<f64>,
    pub java_heap_mb: Option<f64>,
    pub native_heap_mb: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidMemoryLeakReport {
    pub schema_version: u32,
    pub definitions_version: String,
    pub collector: String,
    pub cycles: u32,
    pub checkpoint_every: u32,
    pub warmup_cycles: u32,
    pub stabilization_ms: u64,
    pub cooldown_ms: u64,
    pub slope_mb_per_cycle: Option<f64>,
    pub end_delta_mb: Option<f64>,
    pub monotonic_growth_pct: Option<f64>,
    pub cooldown_recovery_mb: Option<f64>,
    pub threshold_mb_per_cycle: f64,
    pub verdict: String,
    pub confidence: String,
    pub checkpoints: Vec<AndroidMemoryCheckpoint>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosNativeMetrics {
    pub schema_version: u32,
    pub definitions_version: String,
    pub collector: String,
    pub xctrace_version: String,
    pub template: String,
    pub trace_file: String,
    pub trace_archive_file: String,
    pub toc_export_file: String,
    pub profile_export_file: String,
    pub recording_duration_ms: f64,
    pub cpu_sample_count: u64,
    pub cpu_mean_pct: Option<f64>,
    pub frame_time_p95_ms: Option<f64>,
    pub startup_time_ms: Option<f64>,
    pub memory_peak_mb: Option<f64>,
    pub energy_impact: Option<f64>,
    pub availability: IosMetricAvailability,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IosMetricAvailability {
    pub cpu: String,
    pub frames: String,
    pub startup: String,
    pub memory: String,
    pub energy: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceMetadata {
    pub id: Option<String>,
    pub name: Option<String>,
    pub os_version: Option<String>,
    pub refresh_rate: f64,
    pub physical: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSource {
    pub name: Option<String>,
    pub status: Option<String>,
    pub raw_file: Option<String>,
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IterationMetrics {
    pub status: String,
    pub duration_ms: f64,
    pub sample_count: u64,
    pub fps_mean: Option<f64>,
    pub fps_p10: Option<f64>,
    pub low_fps_sample_pct: Option<f64>,
    pub ram_mean_mb: Option<f64>,
    pub ram_peak_mb: Option<f64>,
    pub cpu_mean_pct: Option<f64>,
    pub ui_cpu_mean_pct: Option<f64>,
    pub js_cpu_mean_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSummary {
    pub iteration_count: u64,
    pub successful_iteration_count: u64,
    pub fps_mean: Option<f64>,
    pub fps_p10: Option<f64>,
    pub low_fps_sample_pct: Option<f64>,
    pub ram_mean_mb: Option<f64>,
    pub ram_peak_mb: Option<f64>,
    pub cpu_mean_pct: Option<f64>,
    pub ui_cpu_mean_pct: Option<f64>,
    pub js_cpu_mean_pct: Option<f64>,
}
