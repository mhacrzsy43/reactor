//! Deterministic performance comparison. AI may explain this output, but never changes its facts.

use reactor_protocol::NormalizedResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod ci;
mod profile;

pub use ci::{CiReport, CiStatus, render_ci_html, render_ci_junit};

pub use profile::{
    ComponentChangeEvidence, ComponentProfileDiff, ComponentProfileStat, DiagnosticFinding,
    DiagnosticProfileReport, DiagnosticProfileType, FunctionProfileStat, ProfileCommit,
    ProfileDiffReport, ProfileError, SourceLocation, analyze_profile_json, apply_source_map_json,
    diff_profile_reports,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegressionPolicy {
    pub frame_time_p95_pct: f64,
    pub jank_pct: f64,
    pub startup_pct: f64,
    pub memory_pct: f64,
    pub cpu_pct: f64,
    pub fps_pct: f64,
}

impl Default for RegressionPolicy {
    fn default() -> Self {
        Self {
            frame_time_p95_pct: 10.0,
            jank_pct: 20.0,
            startup_pct: 15.0,
            memory_pct: 10.0,
            cpu_pct: 15.0,
            fps_pct: 10.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    LowerIsBetter,
    HigherIsBetter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricVerdict {
    Improved,
    Stable,
    Regressed,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisVerdict {
    Improved,
    Stable,
    Regressed,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricComparison {
    pub id: String,
    pub label: String,
    pub unit: String,
    pub direction: MetricDirection,
    pub baseline: Option<f64>,
    pub current: Option<f64>,
    pub absolute_delta: Option<f64>,
    pub percent_delta: Option<f64>,
    pub threshold_pct: f64,
    pub verdict: MetricVerdict,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisFinding {
    pub id: String,
    pub severity: FindingSeverity,
    pub title: String,
    pub summary: String,
    pub fact: bool,
    pub metric_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceBundle {
    pub schema_version: u32,
    pub baseline_run_id: String,
    pub current_run_id: String,
    pub flow_hash: String,
    pub framework: String,
    pub platform: String,
    pub scenario: String,
    pub device_class: String,
    pub metric_definitions: Vec<String>,
    pub raw_evidence: Vec<String>,
    pub normalized_facts: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisReport {
    pub schema_version: u32,
    pub verdict: AnalysisVerdict,
    pub compatibility: CompatibilityReport,
    pub metrics: Vec<MetricComparison>,
    pub findings: Vec<AnalysisFinding>,
    pub evidence: EvidenceBundle,
}

#[derive(Debug, Clone)]
struct MetricSpec {
    id: &'static str,
    label: &'static str,
    unit: &'static str,
    direction: MetricDirection,
    baseline: Option<f64>,
    current: Option<f64>,
    threshold_pct: f64,
    baseline_ref: String,
    current_ref: String,
}

#[must_use]
pub fn analyze_pair(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
    policy: &RegressionPolicy,
) -> AnalysisReport {
    let compatibility = check_compatibility(baseline, current);
    let mut metrics = metric_specs(baseline, current, policy)
        .into_iter()
        .map(compare_metric)
        .collect::<Vec<_>>();
    if !compatibility.compatible {
        for metric in &mut metrics {
            metric.verdict = MetricVerdict::Unavailable;
        }
    }
    let verdict = if !compatibility.compatible {
        AnalysisVerdict::Incompatible
    } else if metrics
        .iter()
        .any(|metric| metric.verdict == MetricVerdict::Regressed)
    {
        AnalysisVerdict::Regressed
    } else if metrics
        .iter()
        .any(|metric| metric.verdict == MetricVerdict::Improved)
    {
        AnalysisVerdict::Improved
    } else {
        AnalysisVerdict::Stable
    };
    let findings = build_findings(verdict, &compatibility, &metrics);
    AnalysisReport {
        schema_version: 1,
        verdict,
        compatibility,
        metrics,
        findings,
        evidence: build_evidence(baseline, current),
    }
}

#[must_use]
pub fn check_compatibility(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
) -> CompatibilityReport {
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();
    exact_match(
        "框架",
        &baseline.framework,
        &current.framework,
        &mut reasons,
    );
    exact_match("平台", &baseline.platform, &current.platform, &mut reasons);
    exact_match("场景", &baseline.scenario, &current.scenario, &mut reasons);
    exact_match(
        "构建模式",
        &baseline.build_mode,
        &current.build_mode,
        &mut reasons,
    );
    exact_match(
        "采集适配器",
        &baseline.adapter,
        &current.adapter,
        &mut reasons,
    );
    exact_match(
        "Flow SHA-256",
        &baseline.flow_hash,
        &current.flow_hash,
        &mut reasons,
    );
    if baseline.source.synthetic || current.source.synthetic {
        reasons.push("模拟导览数据不能作为真实性能回归基线".to_owned());
    }
    if baseline.device.physical != current.device.physical {
        reasons.push("模拟器与物理设备结果不能混合比较".to_owned());
    }
    if baseline.device.id != current.device.id {
        reasons.push("设备标识不同".to_owned());
    }
    if baseline.device.os_version != current.device.os_version {
        reasons.push("操作系统版本不同".to_owned());
    }
    if (baseline.device.refresh_rate - current.device.refresh_rate).abs() > 0.01 {
        reasons.push("屏幕刷新率不同".to_owned());
    }
    match (&baseline.android_native, &current.android_native) {
        (Some(left), Some(right)) => {
            exact_match(
                "Android 指标定义",
                &left.definitions_version,
                &right.definitions_version,
                &mut reasons,
            );
            exact_match(
                "Android 采集器",
                &left.collector,
                &right.collector,
                &mut reasons,
            );
            if left.trace_processor_version != right.trace_processor_version {
                warnings.push("Trace Processor 版本不同；规则仍可运行，但建议复测".to_owned());
            }
        }
        (None, None) => {}
        _ => reasons.push("Android 原生指标可用性不同".to_owned()),
    }
    match (&baseline.ios_native, &current.ios_native) {
        (Some(left), Some(right)) => {
            exact_match(
                "iOS 指标定义",
                &left.definitions_version,
                &right.definitions_version,
                &mut reasons,
            );
            exact_match(
                "iOS 采集器",
                &left.collector,
                &right.collector,
                &mut reasons,
            );
            exact_match(
                "xctrace 模板",
                &left.template,
                &right.template,
                &mut reasons,
            );
        }
        (None, None) => {}
        _ => reasons.push("iOS 原生指标可用性不同".to_owned()),
    }
    CompatibilityReport {
        compatible: reasons.is_empty(),
        reasons,
        warnings,
    }
}

fn exact_match(label: &str, baseline: &str, current: &str, reasons: &mut Vec<String>) {
    if baseline != current {
        reasons.push(format!("{label}不同：{baseline} vs {current}"));
    }
}

fn metric_specs(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
    policy: &RegressionPolicy,
) -> Vec<MetricSpec> {
    let mut metrics = android_metric_specs(baseline, current, policy);
    metrics.extend(ios_metric_specs(baseline, current, policy));
    metrics.extend(summary_metric_specs(baseline, current, policy));
    metrics
}

fn android_metric_specs(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
    policy: &RegressionPolicy,
) -> Vec<MetricSpec> {
    let (Some(left), Some(right)) = (&baseline.android_native, &current.android_native) else {
        return vec![];
    };
    vec![
        spec(
            "frame_time_p95_ms",
            "P95 帧耗时",
            "ms",
            MetricDirection::LowerIsBetter,
            left.frame_time_p95_ms,
            right.frame_time_p95_ms,
            policy.frame_time_p95_pct,
        ),
        spec(
            "jank_frame_pct",
            "Jank 帧占比",
            "%",
            MetricDirection::LowerIsBetter,
            left.jank_frame_pct,
            right.jank_frame_pct,
            policy.jank_pct,
        ),
        spec(
            "over_budget_frame_pct",
            "超帧预算占比",
            "%",
            MetricDirection::LowerIsBetter,
            left.over_budget_frame_pct,
            right.over_budget_frame_pct,
            policy.jank_pct,
        ),
        spec(
            "startup_time_ms",
            "冷启动",
            "ms",
            MetricDirection::LowerIsBetter,
            left.startup_time_ms,
            right.startup_time_ms,
            policy.startup_pct,
        ),
        spec(
            "memory_pss_mb",
            "原生 PSS",
            "MB",
            MetricDirection::LowerIsBetter,
            left.memory_pss_mb,
            right.memory_pss_mb,
            policy.memory_pct,
        ),
    ]
}

fn ios_metric_specs(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
    policy: &RegressionPolicy,
) -> Vec<MetricSpec> {
    let (Some(left), Some(right)) = (&baseline.ios_native, &current.ios_native) else {
        return vec![];
    };
    vec![
        spec(
            "ios_cpu_mean_pct",
            "Time Profiler CPU",
            "%",
            MetricDirection::LowerIsBetter,
            left.cpu_mean_pct,
            right.cpu_mean_pct,
            policy.cpu_pct,
        ),
        spec(
            "ios_frame_time_p95_ms",
            "iOS P95 帧耗时",
            "ms",
            MetricDirection::LowerIsBetter,
            left.frame_time_p95_ms,
            right.frame_time_p95_ms,
            policy.frame_time_p95_pct,
        ),
        spec(
            "ios_startup_time_ms",
            "iOS 冷启动",
            "ms",
            MetricDirection::LowerIsBetter,
            left.startup_time_ms,
            right.startup_time_ms,
            policy.startup_pct,
        ),
        spec(
            "ios_memory_peak_mb",
            "iOS 峰值内存",
            "MB",
            MetricDirection::LowerIsBetter,
            left.memory_peak_mb,
            right.memory_peak_mb,
            policy.memory_pct,
        ),
    ]
}

fn summary_metric_specs(
    baseline: &NormalizedResult,
    current: &NormalizedResult,
    policy: &RegressionPolicy,
) -> [MetricSpec; 4] {
    [
        spec(
            "fps_mean",
            "平均 FPS",
            "fps",
            MetricDirection::HigherIsBetter,
            baseline.summary.fps_mean,
            current.summary.fps_mean,
            policy.fps_pct,
        ),
        spec(
            "fps_p10",
            "P10 FPS",
            "fps",
            MetricDirection::HigherIsBetter,
            baseline.summary.fps_p10,
            current.summary.fps_p10,
            policy.fps_pct,
        ),
        spec(
            "cpu_mean_pct",
            "平均 CPU",
            "%",
            MetricDirection::LowerIsBetter,
            baseline.summary.cpu_mean_pct,
            current.summary.cpu_mean_pct,
            policy.cpu_pct,
        ),
        spec(
            "ram_peak_mb",
            "峰值内存",
            "MB",
            MetricDirection::LowerIsBetter,
            baseline.summary.ram_peak_mb,
            current.summary.ram_peak_mb,
            policy.memory_pct,
        ),
    ]
}

fn spec(
    id: &'static str,
    label: &'static str,
    unit: &'static str,
    direction: MetricDirection,
    baseline: Option<f64>,
    current: Option<f64>,
    threshold_pct: f64,
) -> MetricSpec {
    MetricSpec {
        id,
        label,
        unit,
        direction,
        baseline,
        current,
        threshold_pct,
        baseline_ref: format!("baseline.metrics.{id}"),
        current_ref: format!("current.metrics.{id}"),
    }
}

fn compare_metric(spec: MetricSpec) -> MetricComparison {
    let (absolute_delta, percent_delta, verdict) = match (spec.baseline, spec.current) {
        (Some(baseline), Some(current)) if baseline.is_finite() && current.is_finite() => {
            let delta = current - baseline;
            let percent = if baseline.abs() > f64::EPSILON {
                Some(delta / baseline.abs() * 100.0)
            } else {
                None
            };
            let signed_worsening = match spec.direction {
                MetricDirection::LowerIsBetter => percent.unwrap_or_else(|| delta.signum() * 100.0),
                MetricDirection::HigherIsBetter => {
                    -percent.unwrap_or_else(|| -delta.signum() * 100.0)
                }
            };
            let verdict = if signed_worsening > spec.threshold_pct {
                MetricVerdict::Regressed
            } else if signed_worsening < -spec.threshold_pct {
                MetricVerdict::Improved
            } else {
                MetricVerdict::Stable
            };
            (Some(delta), percent, verdict)
        }
        _ => (None, None, MetricVerdict::Unavailable),
    };
    MetricComparison {
        id: spec.id.to_owned(),
        label: spec.label.to_owned(),
        unit: spec.unit.to_owned(),
        direction: spec.direction,
        baseline: spec.baseline,
        current: spec.current,
        absolute_delta,
        percent_delta,
        threshold_pct: spec.threshold_pct,
        verdict,
        evidence_refs: vec![spec.baseline_ref, spec.current_ref],
    }
}

fn build_findings(
    verdict: AnalysisVerdict,
    compatibility: &CompatibilityReport,
    metrics: &[MetricComparison],
) -> Vec<AnalysisFinding> {
    if !compatibility.compatible {
        return vec![AnalysisFinding {
            id: "incompatible-baseline".to_owned(),
            severity: FindingSeverity::Critical,
            title: "基线不兼容".to_owned(),
            summary: compatibility.reasons.join("；"),
            fact: true,
            metric_refs: vec![],
            evidence_refs: vec![
                "baseline.metadata".to_owned(),
                "current.metadata".to_owned(),
            ],
        }];
    }
    let regressions = metrics
        .iter()
        .filter(|metric| metric.verdict == MetricVerdict::Regressed)
        .collect::<Vec<_>>();
    if regressions.is_empty() {
        return vec![AnalysisFinding {
            id: "no-regression".to_owned(),
            severity: FindingSeverity::Info,
            title: if verdict == AnalysisVerdict::Improved {
                "检测到性能改善".to_owned()
            } else {
                "未检测到性能回归".to_owned()
            },
            summary: "所有可比较指标均在配置阈值内，规则层未发现回归。".to_owned(),
            fact: true,
            metric_refs: metrics.iter().map(|metric| metric.id.clone()).collect(),
            evidence_refs: metrics
                .iter()
                .flat_map(|metric| metric.evidence_refs.clone())
                .collect(),
        }];
    }
    regressions
        .into_iter()
        .map(|metric| AnalysisFinding {
            id: format!("regression-{}", metric.id),
            severity: FindingSeverity::Warning,
            title: format!("{}发生回归", metric.label),
            summary: format!(
                "基线 {} {}，当前 {} {}，变化 {:+.1}%，超过 {:.1}% 阈值。",
                display(metric.baseline),
                metric.unit,
                display(metric.current),
                metric.unit,
                metric.percent_delta.unwrap_or_default(),
                metric.threshold_pct
            ),
            fact: true,
            metric_refs: vec![metric.id.clone()],
            evidence_refs: metric.evidence_refs.clone(),
        })
        .collect()
}

fn display(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value:.2}"))
}

fn build_evidence(baseline: &NormalizedResult, current: &NormalizedResult) -> EvidenceBundle {
    let mut definitions = Vec::new();
    let mut raw = Vec::new();
    for result in [baseline, current] {
        if let Some(native) = &result.android_native {
            definitions.push(native.definitions_version.clone());
            raw.push(native.perfetto_trace_file.clone());
        }
        if let Some(native) = &result.ios_native {
            definitions.push(native.definitions_version.clone());
            raw.push(native.trace_archive_file.clone());
            raw.push(native.profile_export_file.clone());
        }
        if let Some(path) = &result.source.raw_file {
            raw.push(path.clone());
        }
    }
    definitions.sort();
    definitions.dedup();
    raw.sort();
    raw.dedup();
    EvidenceBundle {
        schema_version: 1,
        baseline_run_id: baseline.run_id.clone(),
        current_run_id: current.run_id.clone(),
        flow_hash: current.flow_hash.clone(),
        framework: current.framework.clone(),
        platform: current.platform.clone(),
        scenario: current.scenario.clone(),
        device_class: if current.device.physical == Some(true) {
            "physical".to_owned()
        } else {
            "simulator".to_owned()
        },
        metric_definitions: definitions,
        raw_evidence: raw,
        normalized_facts: json!({ "baseline": baseline, "current": current }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use reactor_protocol::{
        AndroidNativeMetrics, DeviceMetadata, MetricSummary, NormalizedResult, ResultSource,
    };

    use super::*;

    fn fixture(run: &str, p95: f64, jank: f64, physical: bool) -> NormalizedResult {
        NormalizedResult {
            schema_version: 1,
            run_id: run.to_owned(),
            created_at: Utc::now(),
            framework: "react-native".to_owned(),
            platform: "android".to_owned(),
            scenario: "list".to_owned(),
            adapter: "perfetto".to_owned(),
            build_mode: "release".to_owned(),
            flow_hash: "same-flow".to_owned(),
            app_id: Some("com.reactor.fixture".to_owned()),
            app_version: Some("1.0 (1)".to_owned()),
            device: DeviceMetadata {
                id: Some("emulator-5554".to_owned()),
                name: Some("Pixel".to_owned()),
                os_version: Some("35".to_owned()),
                refresh_rate: 60.0,
                physical: Some(physical),
            },
            source: ResultSource {
                name: Some("perfetto".to_owned()),
                status: Some("measured".to_owned()),
                raw_file: Some(format!("results/{run}/result.json")),
                synthetic: false,
            },
            android_native: Some(AndroidNativeMetrics {
                schema_version: 1,
                definitions_version: "android-native-v1".to_owned(),
                collector: "perfetto-frametimeline-v1".to_owned(),
                trace_processor_version: "57.2".to_owned(),
                perfetto_trace_file: format!("results/{run}/trace.perfetto"),
                frame_count: 300,
                frame_time_mean_ms: Some(p95 * 0.7),
                frame_time_p50_ms: Some(p95 * 0.6),
                frame_time_p95_ms: Some(p95),
                frame_time_p99_ms: Some(p95 * 1.4),
                jank_frame_count: 10,
                jank_frame_pct: Some(jank),
                over_budget_frame_pct: Some(jank * 2.0),
                startup_time_ms: Some(220.0),
                memory_pss_mb: Some(70.0),
                thermal_status_before: Some(0),
                thermal_status_after: Some(0),
                memory_leak: None,
                warnings: vec![],
            }),
            ios_native: None,
            iterations: vec![],
            summary: MetricSummary {
                iteration_count: 10,
                successful_iteration_count: 10,
                fps_mean: None,
                fps_p10: None,
                low_fps_sample_pct: None,
                ram_mean_mb: None,
                ram_peak_mb: None,
                cpu_mean_pct: Some(12.0),
                ui_cpu_mean_pct: None,
                js_cpu_mean_pct: None,
            },
            warnings: vec![],
        }
    }

    #[test]
    fn injected_frame_regression_is_found_without_ai() {
        let baseline = fixture("baseline", 18.0, 2.0, false);
        let current = fixture("current", 24.0, 3.0, false);
        let report = analyze_pair(&baseline, &current, &RegressionPolicy::default());
        assert_eq!(report.verdict, AnalysisVerdict::Regressed);
        assert!(report.compatibility.compatible);
        assert!(report.findings.iter().all(|finding| finding.fact));
        assert!(report.findings.iter().any(|finding| {
            finding.metric_refs == ["frame_time_p95_ms"]
                && finding
                    .evidence_refs
                    .contains(&"baseline.metrics.frame_time_p95_ms".to_owned())
        }));
    }

    #[test]
    fn simulator_and_physical_results_are_rejected() {
        let baseline = fixture("baseline", 18.0, 2.0, false);
        let mut current = fixture("current", 18.0, 2.0, true);
        current.device.id = Some("physical-device".to_owned());
        let report = analyze_pair(&baseline, &current, &RegressionPolicy::default());
        assert_eq!(report.verdict, AnalysisVerdict::Incompatible);
        assert!(
            report
                .metrics
                .iter()
                .all(|metric| metric.verdict == MetricVerdict::Unavailable)
        );
        assert!(
            report
                .compatibility
                .reasons
                .iter()
                .any(|reason| reason.contains("模拟器与物理设备"))
        );
    }

    #[test]
    fn synthetic_results_cannot_become_a_real_baseline() {
        let mut baseline = fixture("baseline", 18.0, 2.0, false);
        baseline.source.synthetic = true;
        let current = fixture("current", 18.0, 2.0, false);
        let report = analyze_pair(&baseline, &current, &RegressionPolicy::default());
        assert_eq!(report.verdict, AnalysisVerdict::Incompatible);
        assert!(
            report
                .compatibility
                .reasons
                .iter()
                .any(|reason| reason.contains("模拟导览"))
        );
    }
}
