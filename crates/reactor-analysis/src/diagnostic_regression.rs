use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    CompatibilityReport, JsHotspotDiffPolicy, JsHotspotStat, SlowFrameCluster,
    SlowFrameClusterPolicy, SlowFrameObservation, SourceLocation, compare_slow_frame_clusters,
    diff_js_hotspots,
};

pub const DIAGNOSTIC_REGRESSION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticRegressionEvidence {
    pub schema_version: u32,
    pub run_id: String,
    pub js_hotspots: Vec<JsHotspotStat>,
    pub slow_frames: Vec<SlowFrameObservation>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticRegressionPolicy {
    pub js_hotspots: JsHotspotDiffPolicy,
    pub slow_frames: SlowFrameClusterPolicy,
}

#[derive(Debug, Error, PartialEq)]
pub enum DiagnosticRegressionError {
    #[error("unsupported baseline diagnostic evidence schema {actual}; expected {expected}")]
    BaselineSchema { actual: u32, expected: u32 },
    #[error("unsupported current diagnostic evidence schema {actual}; expected {expected}")]
    CurrentSchema { actual: u32, expected: u32 },
    #[error("diagnostic regression threshold `{0}` must be finite and non-negative")]
    InvalidThreshold(&'static str),
    #[error("diagnostic regression minimum `{0}` must be at least one")]
    InvalidMinimum(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRegressionVerdict {
    Stable,
    Regressed,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRegressionReport {
    pub schema_version: u32,
    pub baseline_run_id: String,
    pub current_run_id: String,
    pub verdict: DiagnosticRegressionVerdict,
    pub compatibility: CompatibilityReport,
    pub policy: DiagnosticRegressionPolicy,
    pub facts: DiagnosticRegressionFacts,
    pub rule_hits: Vec<DiagnosticRuleHit>,
    pub temporal_candidates: Vec<TemporalCandidate>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRegressionFacts {
    pub js_hotspot_deltas: Vec<JsHotspotDeltaFact>,
    pub slow_frame_cluster_deltas: Vec<SlowFrameClusterDeltaFact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHotspotDeltaFact {
    pub identity: String,
    pub name: String,
    pub source: Option<SourceLocation>,
    pub baseline_sample_count: u64,
    pub current_sample_count: u64,
    pub sample_count_delta: i64,
    pub baseline_self_time_ms: f64,
    pub current_self_time_ms: f64,
    pub self_time_delta_ms: f64,
    pub self_time_delta_pct: Option<f64>,
    pub baseline_inclusive_time_ms: f64,
    pub current_inclusive_time_ms: f64,
    pub inclusive_time_delta_ms: f64,
    pub baseline_selection_share_pct: Option<f64>,
    pub current_selection_share_pct: Option<f64>,
    pub selection_share_delta_pct_points: Option<f64>,
    pub baseline_slow_frame_window_count: u64,
    pub current_slow_frame_window_count: u64,
    pub slow_frame_window_count_delta: i64,
    pub added_caller_paths: Vec<Vec<String>>,
    pub removed_caller_paths: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameClusterDeltaFact {
    pub signature: String,
    pub baseline_count: u64,
    pub current_count: u64,
    pub count_delta: i64,
    pub count_delta_pct: Option<f64>,
    pub baseline_total_duration_ms: f64,
    pub current_total_duration_ms: f64,
    pub total_duration_delta_ms: f64,
    pub total_duration_delta_pct: Option<f64>,
    pub new_cluster: bool,
    pub removed_cluster: bool,
    pub common_components: Vec<String>,
    pub common_bottom_up_hotspots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRuleKind {
    JsHotspotGrowth,
    SlowFrameClusterGrowth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRuleHit {
    pub rule_id: String,
    pub kind: DiagnosticRuleKind,
    pub fact_identity: String,
    pub gates: Vec<DiagnosticRuleGate>,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRuleGate {
    pub metric: String,
    pub observed: Option<f64>,
    pub comparison: String,
    pub threshold: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCaptureSide {
    Baseline,
    Current,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporalCandidate {
    pub capture: DiagnosticCaptureSide,
    pub signature: String,
    pub step_id: Option<String>,
    pub jank_type: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub frame_count: u64,
    pub total_duration_ms: f64,
    pub frame_ids: Vec<String>,
    pub component_candidates: Vec<String>,
    pub bottom_up_hotspot_candidates: Vec<String>,
}

/// Produces deterministic diagnostic deltas from already-derived evidence.
/// Compatibility is an explicit caller input; incompatible captures retain facts
/// but cannot emit rule hits. Temporal overlap is reported only as a candidate.
///
/// # Errors
///
/// Rejects unsupported evidence versions and invalid thresholds.
#[allow(clippy::too_many_lines)]
pub fn analyze_diagnostic_regression(
    baseline: &DiagnosticRegressionEvidence,
    current: &DiagnosticRegressionEvidence,
    compatibility: CompatibilityReport,
    policy: DiagnosticRegressionPolicy,
) -> Result<DiagnosticRegressionReport, DiagnosticRegressionError> {
    validate_inputs(baseline, current, policy)?;
    let js = diff_js_hotspots(
        &baseline.js_hotspots,
        &current.js_hotspots,
        compatibility.compatible,
        policy.js_hotspots,
    );
    let slow = compare_slow_frame_clusters(
        &baseline.slow_frames,
        &current.slow_frames,
        policy.slow_frames,
    );

    let js_hotspot_deltas = js
        .hotspots
        .iter()
        .map(|diff| JsHotspotDeltaFact {
            identity: diff.identity.clone(),
            name: diff.name.clone(),
            source: diff.source.clone(),
            baseline_sample_count: diff.baseline_sample_count,
            current_sample_count: diff.current_sample_count,
            sample_count_delta: diff.sample_count_delta,
            baseline_self_time_ms: diff.baseline_self_time_ms,
            current_self_time_ms: diff.current_self_time_ms,
            self_time_delta_ms: diff.self_time_delta_ms,
            self_time_delta_pct: diff.self_time_delta_pct,
            baseline_inclusive_time_ms: diff.baseline_inclusive_time_ms,
            current_inclusive_time_ms: diff.current_inclusive_time_ms,
            inclusive_time_delta_ms: diff.inclusive_time_delta_ms,
            baseline_selection_share_pct: diff.baseline_selection_share_pct,
            current_selection_share_pct: diff.current_selection_share_pct,
            selection_share_delta_pct_points: diff.selection_share_delta_pct_points,
            baseline_slow_frame_window_count: diff.baseline_slow_frame_window_count,
            current_slow_frame_window_count: diff.current_slow_frame_window_count,
            slow_frame_window_count_delta: diff.slow_frame_window_count_delta,
            added_caller_paths: diff.added_caller_paths.clone(),
            removed_caller_paths: diff.removed_caller_paths.clone(),
        })
        .collect();
    let slow_frame_cluster_deltas = slow
        .diffs
        .iter()
        .map(|diff| SlowFrameClusterDeltaFact {
            signature: diff.signature.clone(),
            baseline_count: diff.baseline_count,
            current_count: diff.current_count,
            count_delta: diff.count_delta,
            count_delta_pct: diff.count_delta_pct,
            baseline_total_duration_ms: diff.baseline_total_duration_ms,
            current_total_duration_ms: diff.current_total_duration_ms,
            total_duration_delta_ms: diff.total_duration_delta_ms,
            total_duration_delta_pct: diff.total_duration_delta_pct,
            new_cluster: diff.new_cluster,
            removed_cluster: diff.removed_cluster,
            common_components: diff.common_components.clone(),
            common_bottom_up_hotspots: diff.common_bottom_up_hotspots.clone(),
        })
        .collect();

    let mut rule_hits = Vec::new();
    if compatibility.compatible {
        rule_hits.extend(
            js.hotspots
                .iter()
                .filter(|diff| diff.regressed)
                .map(|diff| DiagnosticRuleHit {
                    rule_id: "js-hotspot-growth-v1".to_owned(),
                    kind: DiagnosticRuleKind::JsHotspotGrowth,
                    fact_identity: diff.identity.clone(),
                    gates: vec![
                        gate(
                            "selfTimeDeltaPct",
                            diff.self_time_delta_pct,
                            ">=",
                            policy.js_hotspots.relative_threshold_pct,
                        ),
                        gate(
                            "selfTimeDeltaMs",
                            Some(diff.self_time_delta_ms),
                            ">=",
                            policy.js_hotspots.absolute_self_time_threshold_ms,
                        ),
                        gate(
                            "currentSampleCount",
                            Some(count_as_f64(diff.current_sample_count)),
                            ">=",
                            count_as_f64(policy.js_hotspots.min_current_samples),
                        ),
                    ],
                    evidence_refs: vec![format!(
                        "facts.jsHotspotDeltas[identity={}]",
                        diff.identity
                    )],
                }),
        );
        rule_hits.extend(slow.diffs.iter().filter(|diff| diff.regressed).map(|diff| {
            DiagnosticRuleHit {
                rule_id: "slow-frame-cluster-growth-v1".to_owned(),
                kind: DiagnosticRuleKind::SlowFrameClusterGrowth,
                fact_identity: diff.signature.clone(),
                gates: vec![
                    gate(
                        "countDeltaPct",
                        diff.count_delta_pct,
                        ">=",
                        policy.slow_frames.count_relative_threshold_pct,
                    ),
                    gate(
                        "totalDurationDeltaPct",
                        diff.total_duration_delta_pct,
                        ">=",
                        policy.slow_frames.duration_relative_threshold_pct,
                    ),
                    gate(
                        "totalDurationDeltaMs",
                        Some(diff.total_duration_delta_ms),
                        ">=",
                        policy.slow_frames.duration_absolute_threshold_ms,
                    ),
                    gate(
                        "currentFrameCount",
                        Some(count_as_f64(diff.current_count)),
                        ">=",
                        count_as_f64(policy.slow_frames.min_current_frames),
                    ),
                ],
                evidence_refs: vec![format!(
                    "facts.slowFrameClusterDeltas[signature={}]",
                    diff.signature
                )],
            }
        }));
    }

    let temporal_candidates = temporal_candidates(&slow.baseline_clusters, &slow.current_clusters);
    let verdict = if !compatibility.compatible {
        DiagnosticRegressionVerdict::Incompatible
    } else if rule_hits.is_empty() {
        DiagnosticRegressionVerdict::Stable
    } else {
        DiagnosticRegressionVerdict::Regressed
    };
    Ok(DiagnosticRegressionReport {
        schema_version: DIAGNOSTIC_REGRESSION_SCHEMA_VERSION,
        baseline_run_id: baseline.run_id.clone(),
        current_run_id: current.run_id.clone(),
        verdict,
        compatibility,
        policy,
        facts: DiagnosticRegressionFacts {
            js_hotspot_deltas,
            slow_frame_cluster_deltas,
        },
        rule_hits,
        temporal_candidates,
        limitations: vec![
            "Rule hits identify threshold crossings, not causes.".to_owned(),
            "Temporal candidates describe clustered observations and do not imply causal relationships between frames, components, or JS functions.".to_owned(),
            "Results depend on the completeness and clock alignment of the supplied managed artifacts.".to_owned(),
        ],
    })
}

fn validate_inputs(
    baseline: &DiagnosticRegressionEvidence,
    current: &DiagnosticRegressionEvidence,
    policy: DiagnosticRegressionPolicy,
) -> Result<(), DiagnosticRegressionError> {
    if baseline.schema_version != DIAGNOSTIC_REGRESSION_SCHEMA_VERSION {
        return Err(DiagnosticRegressionError::BaselineSchema {
            actual: baseline.schema_version,
            expected: DIAGNOSTIC_REGRESSION_SCHEMA_VERSION,
        });
    }
    if current.schema_version != DIAGNOSTIC_REGRESSION_SCHEMA_VERSION {
        return Err(DiagnosticRegressionError::CurrentSchema {
            actual: current.schema_version,
            expected: DIAGNOSTIC_REGRESSION_SCHEMA_VERSION,
        });
    }
    for (name, value) in [
        (
            "jsHotspots.relativeThresholdPct",
            policy.js_hotspots.relative_threshold_pct,
        ),
        (
            "jsHotspots.absoluteSelfTimeThresholdMs",
            policy.js_hotspots.absolute_self_time_threshold_ms,
        ),
        ("slowFrames.proximityMs", policy.slow_frames.proximity_ms),
        (
            "slowFrames.countRelativeThresholdPct",
            policy.slow_frames.count_relative_threshold_pct,
        ),
        (
            "slowFrames.durationRelativeThresholdPct",
            policy.slow_frames.duration_relative_threshold_pct,
        ),
        (
            "slowFrames.durationAbsoluteThresholdMs",
            policy.slow_frames.duration_absolute_threshold_ms,
        ),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(DiagnosticRegressionError::InvalidThreshold(name));
        }
    }
    for (name, value) in [
        (
            "jsHotspots.minCurrentSamples",
            policy.js_hotspots.min_current_samples,
        ),
        (
            "slowFrames.minCurrentFrames",
            policy.slow_frames.min_current_frames,
        ),
    ] {
        if value == 0 {
            return Err(DiagnosticRegressionError::InvalidMinimum(name));
        }
    }
    Ok(())
}

fn gate(
    metric: &str,
    observed: Option<f64>,
    comparison: &str,
    threshold: f64,
) -> DiagnosticRuleGate {
    DiagnosticRuleGate {
        metric: metric.to_owned(),
        observed,
        comparison: comparison.to_owned(),
        threshold,
    }
}

fn temporal_candidates(
    baseline: &[SlowFrameCluster],
    current: &[SlowFrameCluster],
) -> Vec<TemporalCandidate> {
    baseline
        .iter()
        .map(|cluster| candidate(DiagnosticCaptureSide::Baseline, cluster))
        .chain(
            current
                .iter()
                .map(|cluster| candidate(DiagnosticCaptureSide::Current, cluster)),
        )
        .collect()
}

fn candidate(capture: DiagnosticCaptureSide, cluster: &SlowFrameCluster) -> TemporalCandidate {
    TemporalCandidate {
        capture,
        signature: cluster.signature.clone(),
        step_id: cluster.step_id.clone(),
        jank_type: cluster.jank_type.clone(),
        start_ms: cluster.start_ms,
        end_ms: cluster.end_ms,
        frame_count: cluster.frame_count,
        total_duration_ms: cluster.total_duration_ms,
        frame_ids: cluster.frame_ids.clone(),
        component_candidates: cluster.common_components.clone(),
        bottom_up_hotspot_candidates: cluster.common_bottom_up_hotspots.clone(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn count_as_f64(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotspot(samples: u64, self_ms: f64, path: &str) -> JsHotspotStat {
        JsHotspotStat {
            name: "renderList".to_owned(),
            source: Some(SourceLocation {
                file: "src/List.tsx".to_owned(),
                line: Some(42),
                column: Some(3),
            }),
            sample_count: samples,
            self_time_ms: self_ms,
            inclusive_time_ms: self_ms * 2.0,
            selected_time_ms: self_ms,
            slow_frame_window_count: samples / 2,
            caller_paths: vec![vec![path.to_owned(), "renderList".to_owned()]],
        }
    }

    fn frame(id: &str, duration_ms: f64) -> SlowFrameObservation {
        SlowFrameObservation {
            id: id.to_owned(),
            step_id: Some("scroll".to_owned()),
            timestamp_ms: 0.0,
            duration_ms,
            jank_type: "deadline_missed".to_owned(),
            components: vec!["List".to_owned()],
            bottom_up_hotspots: vec!["renderList".to_owned()],
        }
    }

    fn evidence(
        run_id: &str,
        hotspot: JsHotspotStat,
        frames: Vec<SlowFrameObservation>,
    ) -> DiagnosticRegressionEvidence {
        DiagnosticRegressionEvidence {
            schema_version: 1,
            run_id: run_id.to_owned(),
            js_hotspots: vec![hotspot],
            slow_frames: frames,
        }
    }

    fn compatibility(compatible: bool) -> CompatibilityReport {
        CompatibilityReport {
            compatible,
            reasons: if compatible {
                vec![]
            } else {
                vec!["capture mismatch".to_owned()]
            },
            warnings: vec![],
        }
    }

    #[test]
    fn report_separates_facts_hits_and_temporal_candidates() {
        let baseline = evidence("base", hotspot(10, 20.0, "root"), vec![frame("b", 20.0)]);
        let current = evidence(
            "current",
            hotspot(20, 40.0, "screen"),
            vec![frame("c1", 40.0), frame("c2", 40.0)],
        );
        let report = analyze_diagnostic_regression(
            &baseline,
            &current,
            compatibility(true),
            DiagnosticRegressionPolicy::default(),
        )
        .unwrap();

        assert_eq!(report.verdict, DiagnosticRegressionVerdict::Regressed);
        assert_eq!(report.rule_hits.len(), 2);
        assert_eq!(report.temporal_candidates.len(), 2);
        let js = &report.facts.js_hotspot_deltas[0];
        assert_eq!(js.sample_count_delta, 10);
        assert!((js.inclusive_time_delta_ms - 40.0).abs() < f64::EPSILON);
        assert_eq!(js.added_caller_paths[0][0], "screen");
        assert_eq!(js.removed_caller_paths[0][0], "root");
    }

    #[test]
    fn incompatibility_preserves_facts_but_suppresses_rule_hits() {
        let baseline = evidence("base", hotspot(10, 20.0, "root"), vec![frame("b", 20.0)]);
        let current = evidence(
            "current",
            hotspot(20, 40.0, "screen"),
            vec![frame("c", 50.0)],
        );
        let report = analyze_diagnostic_regression(
            &baseline,
            &current,
            compatibility(false),
            DiagnosticRegressionPolicy::default(),
        )
        .unwrap();

        assert_eq!(report.verdict, DiagnosticRegressionVerdict::Incompatible);
        assert!(report.rule_hits.is_empty());
        assert!(!report.facts.js_hotspot_deltas.is_empty());
    }

    #[test]
    fn invalid_threshold_is_rejected() {
        let evidence = evidence("run", hotspot(1, 1.0, "root"), vec![]);
        let mut policy = DiagnosticRegressionPolicy::default();
        policy.js_hotspots.relative_threshold_pct = f64::NAN;
        assert!(matches!(
            analyze_diagnostic_regression(&evidence, &evidence, compatibility(true), policy),
            Err(DiagnosticRegressionError::InvalidThreshold(_))
        ));
    }
}
