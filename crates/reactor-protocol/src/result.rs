use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    Benchmark,
    Diagnose,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCaptureMode {
    #[default]
    Disabled,
    InBand,
    Companion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCollectorPlanV1 {
    pub collector: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResourceLimitsV1 {
    pub max_duration_ms: u64,
    pub max_artifact_bytes: u64,
    pub max_events: u64,
    pub max_samples: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPlanV1 {
    pub schema_version: u32,
    pub mode: DiagnosticCaptureMode,
    #[serde(default)]
    pub collectors: Vec<DiagnosticCollectorPlanV1>,
    pub resource_limits: DiagnosticResourceLimitsV1,
}

impl DiagnosticPlanV1 {
    /// Validates bounded diagnostic resources before a runner accepts work.
    ///
    /// # Errors
    ///
    /// Returns a field-specific message for unsupported plans or limits outside hard bounds.
    pub fn validate(&self) -> Result<(), String> {
        const MAX_DURATION_MS: u64 = 30 * 60 * 1_000;
        const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;
        const MAX_EVENTS: u64 = 5_000_000;
        const MAX_SAMPLES: u64 = 20_000_000;

        if self.schema_version != 1 {
            return Err("diagnostic plan schemaVersion must be 1".to_owned());
        }
        if self.mode == DiagnosticCaptureMode::Disabled {
            return Err("diagnostic capture mode must not be disabled".to_owned());
        }
        if self.collectors.is_empty() {
            return Err("diagnostic plan must request at least one collector".to_owned());
        }
        let mut names = std::collections::BTreeSet::new();
        for collector in &self.collectors {
            if collector.collector != "hermes-cpu" {
                return Err(format!(
                    "unsupported diagnostic collector: {}",
                    collector.collector
                ));
            }
            if !names.insert(&collector.collector) {
                return Err(format!(
                    "duplicate diagnostic collector: {}",
                    collector.collector
                ));
            }
        }
        let limits = &self.resource_limits;
        for (name, value, maximum) in [
            ("maxDurationMs", limits.max_duration_ms, MAX_DURATION_MS),
            (
                "maxArtifactBytes",
                limits.max_artifact_bytes,
                MAX_ARTIFACT_BYTES,
            ),
            ("maxEvents", limits.max_events, MAX_EVENTS),
            ("maxSamples", limits.max_samples, MAX_SAMPLES),
        ] {
            if value == 0 || value > maximum {
                return Err(format!("{name} must be between 1 and {maximum}"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildIdentityV1 {
    pub schema_version: u32,
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_native_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reactor_sdk_version: Option<String>,
    pub variant: String,
    pub optimization_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_sha256: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactIntegrity {
    Complete,
    Partial,
    Truncated,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactTimeRangeV1 {
    pub start_ns: u64,
    pub end_ns: u64,
    pub clock: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    pub path: String,
    pub format: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub producer: String,
    pub producer_version: String,
    pub capture_method: String,
    pub integrity: ArtifactIntegrity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_range: Option<ArtifactTimeRangeV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    Collected,
    Unavailable,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorDiagnosticV1 {
    pub status: CollectorStatus,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactNativeFrameworkDiagnosticsV1 {
    #[serde(default)]
    pub collectors: BTreeMap<String, CollectorDiagnosticV1>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameworkDiagnosticsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub react_native: Option<ReactNativeFrameworkDiagnosticsV1>,
}

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
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_plan: Option<DiagnosticPlanV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_identity: Option<BuildIdentityV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework_diagnostics: Option<FrameworkDiagnosticsV1>,
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

impl NormalizedResult {
    /// Returns React Native diagnostics from the additive v1 field, falling back to the legacy
    /// Android-native nesting for historical results.
    #[must_use]
    pub fn react_native_diagnostics(&self) -> Option<ReactNativeDiagnosticsView<'_>> {
        if let Some(diagnostics) = self
            .framework_diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.react_native.as_ref())
        {
            return Some(ReactNativeDiagnosticsView::V1(diagnostics));
        }
        self.android_native
            .as_ref()
            .and_then(|native| native.rn_diagnostics.as_ref())
            .map(ReactNativeDiagnosticsView::Legacy)
    }

    /// Adds a collector-status view converted from legacy `androidNative.rnDiagnostics` when the
    /// new top-level field is absent. Historical JSON remains untouched on disk.
    pub fn populate_framework_diagnostics_fallback(&mut self) {
        if self.framework_diagnostics.is_some() {
            return;
        }
        let Some(legacy) = self
            .android_native
            .as_ref()
            .and_then(|native| native.rn_diagnostics.as_ref())
        else {
            return;
        };
        let mut collectors = BTreeMap::new();
        let mut artifacts = Vec::new();
        if let Some(path) = &legacy.profile_file {
            artifacts.push(legacy_artifact(path, "react-devtools-profile", legacy));
        }
        artifacts.push(legacy_artifact(
            &legacy.event_file,
            "reactor-rn-events",
            legacy,
        ));
        collectors.insert(
            legacy.collector.clone(),
            CollectorDiagnosticV1 {
                status: CollectorStatus::Collected,
                artifacts,
                reason: Some("converted from androidNative.rnDiagnostics".to_owned()),
            },
        );
        self.framework_diagnostics = Some(FrameworkDiagnosticsV1 {
            react_native: Some(ReactNativeFrameworkDiagnosticsV1 { collectors }),
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReactNativeDiagnosticsView<'a> {
    V1(&'a ReactNativeFrameworkDiagnosticsV1),
    Legacy(&'a ReactNativeDiagnosticsSummary),
}

fn legacy_artifact(
    path: &str,
    format: &str,
    legacy: &ReactNativeDiagnosticsSummary,
) -> ArtifactRef {
    ArtifactRef {
        path: path.to_owned(),
        format: format.to_owned(),
        size_bytes: 0,
        sha256: String::new(),
        producer: legacy.collector.clone(),
        producer_version: format!("legacy-schema-{}", legacy.schema_version),
        capture_method: "legacy_android_native_rn_diagnostics".to_owned(),
        integrity: ArtifactIntegrity::Partial,
        time_range: None,
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rn_diagnostics: Option<ReactNativeDiagnosticsSummary>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactNativeDiagnosticsSummary {
    pub schema_version: u32,
    pub collector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_mode: Option<String>,
    pub event_file: String,
    pub event_count: u64,
    pub component_names: Vec<String>,
    pub component_render_count: u64,
    pub component_tree_commit_count: u64,
    pub profile_commit_count: u64,
    pub console_event_count: u64,
    pub network_event_count: u64,
    pub hermes_heap_sample_count: u64,
    pub allocated_object_count: u64,
    pub retained_object_count: u64,
    pub retained_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes_heap_stats_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hermes_heap_snapshot_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_heap_dump_file: Option<String>,
    #[serde(default)]
    pub recent_events: Vec<ReactNativeDiagnosticEvent>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactNativeDiagnosticEvent {
    pub timestamp_ms: u64,
    pub kind: String,
    pub payload: serde_json::Value,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_heap_trace_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_retained_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_retained_allocation_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_retained_object_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_retained_bytes: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_result_defaults_to_benchmark_and_converts_legacy_rn_diagnostics() {
        let mut value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/result-v1-diagnostics.json"
        ))
        .unwrap();
        value.as_object_mut().unwrap().remove("runMode");
        value.as_object_mut().unwrap().remove("diagnosticPlan");
        value.as_object_mut().unwrap().remove("buildIdentity");
        value.as_object_mut().unwrap().remove("artifacts");
        value
            .as_object_mut()
            .unwrap()
            .remove("frameworkDiagnostics");
        value["androidNative"] = serde_json::json!({
            "schemaVersion": 1, "definitionsVersion": "android-native-v1",
            "collector": "perfetto-v1", "traceProcessorVersion": "57.2",
            "perfettoTraceFile": "trace.pftrace", "frameCount": 0,
            "frameTimeMeanMs": null, "frameTimeP50Ms": null, "frameTimeP95Ms": null,
            "frameTimeP99Ms": null, "jankFrameCount": 0, "jankFramePct": null,
            "overBudgetFramePct": null, "startupTimeMs": null, "memoryPssMb": null,
            "thermalStatusBefore": null, "thermalStatusAfter": null,
            "rnDiagnostics": {
                "schemaVersion": 1, "collector": "rn-hook-v1", "eventFile": "events.json",
                "eventCount": 0, "componentNames": [], "componentRenderCount": 0,
                "componentTreeCommitCount": 0, "profileCommitCount": 0,
                "consoleEventCount": 0, "networkEventCount": 0,
                "hermesHeapSampleCount": 0, "allocatedObjectCount": 0,
                "retainedObjectCount": 0, "retainedBytes": 0
            }, "warnings": []
        });
        let mut result: NormalizedResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.run_mode, RunMode::Benchmark);
        assert!(matches!(
            result.react_native_diagnostics(),
            Some(ReactNativeDiagnosticsView::Legacy(_))
        ));
        result.populate_framework_diagnostics_fallback();
        let collector = &result
            .framework_diagnostics
            .unwrap()
            .react_native
            .unwrap()
            .collectors["rn-hook-v1"];
        assert_eq!(collector.status, CollectorStatus::Collected);
        assert_eq!(collector.artifacts[0].integrity, ArtifactIntegrity::Partial);
    }

    #[test]
    fn diagnostics_schema_fixture_deserializes() {
        let fixture = include_str!("../../../tests/fixtures/result-v1-diagnostics.json");
        let result: NormalizedResult = serde_json::from_str(fixture).unwrap();
        assert_eq!(result.run_mode, RunMode::Diagnose);
        assert_eq!(
            result
                .build_identity
                .as_ref()
                .map(|identity| identity.fingerprint.as_str()),
            Some("fixture-build-fingerprint")
        );
        assert!(matches!(
            result.react_native_diagnostics(),
            Some(ReactNativeDiagnosticsView::V1(_))
        ));
    }

    #[test]
    fn diagnostic_plan_validation_rejects_unbounded_or_unenforceable_inputs() {
        let mut plan = DiagnosticPlanV1 {
            schema_version: 1,
            mode: DiagnosticCaptureMode::InBand,
            collectors: vec![DiagnosticCollectorPlanV1 {
                collector: "hermes-cpu".to_owned(),
                required: true,
            }],
            resource_limits: DiagnosticResourceLimitsV1 {
                max_duration_ms: 60_000,
                max_artifact_bytes: 1024,
                max_events: 100,
                max_samples: 100,
            },
        };
        assert!(plan.validate().is_ok());
        plan.resource_limits.max_duration_ms = 0;
        assert!(plan.validate().unwrap_err().contains("maxDurationMs"));
        plan.resource_limits.max_duration_ms = 60_000;
        plan.collectors[0].collector = "unknown".to_owned();
        assert!(plan.validate().unwrap_err().contains("unsupported"));
    }
}
