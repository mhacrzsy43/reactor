use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::SourceLocation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHotspotStat {
    pub name: String,
    pub source: Option<SourceLocation>,
    pub sample_count: u64,
    pub self_time_ms: f64,
    pub inclusive_time_ms: f64,
    pub selected_time_ms: f64,
    pub slow_frame_window_count: u64,
    pub caller_paths: Vec<Vec<String>>,
}

impl JsHotspotStat {
    #[must_use]
    pub fn stable_identity(&self) -> String {
        self.source.as_ref().map_or_else(
            || format!("name:{}", self.name),
            |source| {
                format!(
                    "{}:{}:{}:{}",
                    source.file,
                    source.line.unwrap_or_default(),
                    source.column.unwrap_or_default(),
                    self.name
                )
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHotspotDiffPolicy {
    pub relative_threshold_pct: f64,
    pub absolute_self_time_threshold_ms: f64,
    pub min_current_samples: u64,
}

impl Default for JsHotspotDiffPolicy {
    fn default() -> Self {
        Self {
            relative_threshold_pct: 20.0,
            absolute_self_time_threshold_ms: 5.0,
            min_current_samples: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHotspotDiff {
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
    pub regressed: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsHotspotDiffReport {
    pub schema_version: u32,
    pub compatible: bool,
    pub reasons: Vec<String>,
    pub regression_count: u64,
    pub hotspots: Vec<JsHotspotDiff>,
}

/// Compares stable JS hotspot identities. A regression requires all three
/// gates: relative growth, absolute self-time growth, and minimum samples.
#[must_use]
pub fn diff_js_hotspots(
    baseline: &[JsHotspotStat],
    current: &[JsHotspotStat],
    compatible_capture: bool,
    policy: JsHotspotDiffPolicy,
) -> JsHotspotDiffReport {
    let mut reasons = Vec::new();
    if !compatible_capture {
        reasons.push("JS Profile 采集定义不兼容".to_owned());
    }
    let baseline_total = baseline
        .iter()
        .map(|hotspot| hotspot.selected_time_ms)
        .sum::<f64>();
    let current_total = current
        .iter()
        .map(|hotspot| hotspot.selected_time_ms)
        .sum::<f64>();
    let baseline_map = baseline
        .iter()
        .map(|hotspot| (hotspot.stable_identity(), hotspot))
        .collect::<BTreeMap<_, _>>();
    let current_map = current
        .iter()
        .map(|hotspot| (hotspot.stable_identity(), hotspot))
        .collect::<BTreeMap<_, _>>();
    let identities = baseline_map
        .keys()
        .chain(current_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut hotspots = identities
        .into_iter()
        .map(|identity| {
            let left = baseline_map.get(&identity).copied();
            let right = current_map.get(&identity).copied();
            build_diff(
                identity,
                left,
                right,
                baseline_total,
                current_total,
                compatible_capture,
                policy,
            )
        })
        .collect::<Vec<_>>();
    hotspots.sort_by(|left, right| {
        right
            .regressed
            .cmp(&left.regressed)
            .then_with(|| right.self_time_delta_ms.total_cmp(&left.self_time_delta_ms))
            .then_with(|| left.identity.cmp(&right.identity))
    });
    JsHotspotDiffReport {
        schema_version: 1,
        compatible: compatible_capture,
        reasons,
        regression_count: usize_as_u64(hotspots.iter().filter(|hotspot| hotspot.regressed).count()),
        hotspots,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_diff(
    identity: String,
    baseline: Option<&JsHotspotStat>,
    current: Option<&JsHotspotStat>,
    baseline_total: f64,
    current_total: f64,
    compatible_capture: bool,
    policy: JsHotspotDiffPolicy,
) -> JsHotspotDiff {
    let representative = current.or(baseline);
    let baseline_sample_count = baseline.map_or(0, |hotspot| hotspot.sample_count);
    let current_sample_count = current.map_or(0, |hotspot| hotspot.sample_count);
    let baseline_self_time_ms = baseline.map_or(0.0, |hotspot| hotspot.self_time_ms);
    let current_self_time_ms = current.map_or(0.0, |hotspot| hotspot.self_time_ms);
    let baseline_inclusive_time_ms = baseline.map_or(0.0, |hotspot| hotspot.inclusive_time_ms);
    let current_inclusive_time_ms = current.map_or(0.0, |hotspot| hotspot.inclusive_time_ms);
    let self_time_delta_ms = current_self_time_ms - baseline_self_time_ms;
    let self_time_delta_pct = percent_delta(baseline_self_time_ms, current_self_time_ms);
    let baseline_share = share(
        baseline.map_or(0.0, |hotspot| hotspot.selected_time_ms),
        baseline_total,
    );
    let current_share = share(
        current.map_or(0.0, |hotspot| hotspot.selected_time_ms),
        current_total,
    );
    let relative_gate = if baseline_self_time_ms > f64::EPSILON {
        self_time_delta_pct.is_some_and(|delta| delta >= policy.relative_threshold_pct)
    } else {
        current_self_time_ms >= policy.absolute_self_time_threshold_ms
    };
    let absolute_gate = self_time_delta_ms >= policy.absolute_self_time_threshold_ms;
    let sample_gate = current_sample_count >= policy.min_current_samples;
    let regressed = compatible_capture && relative_gate && absolute_gate && sample_gate;
    let mut gate_reasons = Vec::new();
    gate_reasons.push(format!(
        "相对门槛 {}：{}",
        policy.relative_threshold_pct,
        if relative_gate { "命中" } else { "未命中" }
    ));
    gate_reasons.push(format!(
        "绝对门槛 {:.3}ms：{}",
        policy.absolute_self_time_threshold_ms,
        if absolute_gate { "命中" } else { "未命中" }
    ));
    gate_reasons.push(format!(
        "最小样本 {}：{}",
        policy.min_current_samples,
        if sample_gate { "命中" } else { "未命中" }
    ));
    if !compatible_capture {
        gate_reasons.push("采集定义不兼容，禁止回归判定".to_owned());
    }

    let baseline_paths = baseline
        .into_iter()
        .flat_map(|hotspot| hotspot.caller_paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let current_paths = current
        .into_iter()
        .flat_map(|hotspot| hotspot.caller_paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let added_caller_paths = current_paths.difference(&baseline_paths).cloned().collect();
    let removed_caller_paths = baseline_paths.difference(&current_paths).cloned().collect();
    JsHotspotDiff {
        identity,
        name: representative.map_or_else(|| "(unknown)".to_owned(), |value| value.name.clone()),
        source: representative.and_then(|value| value.source.clone()),
        baseline_sample_count,
        current_sample_count,
        sample_count_delta: signed_delta(current_sample_count, baseline_sample_count),
        baseline_self_time_ms,
        current_self_time_ms,
        self_time_delta_ms,
        self_time_delta_pct,
        baseline_inclusive_time_ms,
        current_inclusive_time_ms,
        inclusive_time_delta_ms: current_inclusive_time_ms - baseline_inclusive_time_ms,
        baseline_selection_share_pct: baseline_share,
        current_selection_share_pct: current_share,
        selection_share_delta_pct_points: baseline_share
            .zip(current_share)
            .map(|(baseline, current)| current - baseline),
        baseline_slow_frame_window_count: baseline
            .map_or(0, |hotspot| hotspot.slow_frame_window_count),
        current_slow_frame_window_count: current
            .map_or(0, |hotspot| hotspot.slow_frame_window_count),
        slow_frame_window_count_delta: signed_delta(
            current.map_or(0, |hotspot| hotspot.slow_frame_window_count),
            baseline.map_or(0, |hotspot| hotspot.slow_frame_window_count),
        ),
        added_caller_paths,
        removed_caller_paths,
        regressed,
        reasons: gate_reasons,
    }
}

fn share(value: f64, total: f64) -> Option<f64> {
    (total > f64::EPSILON).then(|| value / total * 100.0)
}

fn percent_delta(baseline: f64, current: f64) -> Option<f64> {
    (baseline.abs() > f64::EPSILON).then(|| (current - baseline) / baseline.abs() * 100.0)
}

fn signed_delta(current: u64, baseline: u64) -> i64 {
    if current >= baseline {
        i64::try_from(current - baseline).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(baseline - current).unwrap_or(i64::MAX)
    }
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hotspot(samples: u64, self_time_ms: f64) -> JsHotspotStat {
        JsHotspotStat {
            name: "renderList".to_owned(),
            source: Some(SourceLocation {
                file: "src/List.tsx".to_owned(),
                line: Some(42),
                column: Some(3),
            }),
            sample_count: samples,
            self_time_ms,
            inclusive_time_ms: self_time_ms * 2.0,
            selected_time_ms: self_time_ms,
            slow_frame_window_count: samples / 2,
            caller_paths: vec![vec!["root".to_owned(), "renderList".to_owned()]],
        }
    }

    #[test]
    fn regression_requires_relative_absolute_and_sample_gates() {
        let policy = JsHotspotDiffPolicy::default();
        let baseline = hotspot(10, 20.0);

        let relative_only = hotspot(10, 24.5);
        assert_eq!(
            diff_js_hotspots(
                std::slice::from_ref(&baseline),
                &[relative_only],
                true,
                policy
            )
            .regression_count,
            0
        );

        let too_few_samples = hotspot(4, 30.0);
        assert_eq!(
            diff_js_hotspots(
                std::slice::from_ref(&baseline),
                &[too_few_samples],
                true,
                policy,
            )
            .regression_count,
            0
        );

        let regression = hotspot(12, 30.0);
        let report = diff_js_hotspots(&[baseline], &[regression], true, policy);
        assert_eq!(report.regression_count, 1);
        assert!(report.hotspots[0].regressed);
    }

    #[test]
    fn incompatible_capture_never_emits_regression() {
        let report = diff_js_hotspots(
            &[hotspot(10, 10.0)],
            &[hotspot(20, 30.0)],
            false,
            JsHotspotDiffPolicy::default(),
        );
        assert!(!report.compatible);
        assert_eq!(report.regression_count, 0);
    }
}
