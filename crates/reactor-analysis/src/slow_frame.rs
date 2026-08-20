use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameObservation {
    pub id: String,
    pub step_id: Option<String>,
    pub timestamp_ms: f64,
    pub duration_ms: f64,
    pub jank_type: String,
    pub components: Vec<String>,
    pub bottom_up_hotspots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameCluster {
    pub signature: String,
    pub step_id: Option<String>,
    pub jank_type: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub frame_count: u64,
    pub total_duration_ms: f64,
    pub max_duration_ms: f64,
    pub common_components: Vec<String>,
    pub common_bottom_up_hotspots: Vec<String>,
    pub frame_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameClusterPolicy {
    pub proximity_ms: f64,
    pub count_relative_threshold_pct: f64,
    pub duration_relative_threshold_pct: f64,
    pub duration_absolute_threshold_ms: f64,
    pub min_current_frames: u64,
}

impl Default for SlowFrameClusterPolicy {
    fn default() -> Self {
        Self {
            proximity_ms: 250.0,
            count_relative_threshold_pct: 20.0,
            duration_relative_threshold_pct: 20.0,
            duration_absolute_threshold_ms: 16.0,
            min_current_frames: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameClusterDiff {
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
    pub regressed: bool,
    pub common_components: Vec<String>,
    pub common_bottom_up_hotspots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlowFrameClusterComparison {
    pub baseline_clusters: Vec<SlowFrameCluster>,
    pub current_clusters: Vec<SlowFrameCluster>,
    pub diffs: Vec<SlowFrameClusterDiff>,
    pub regression_count: u64,
}

/// Clusters slow frames by step, jank type, candidate signature, and temporal
/// proximity. Candidate signatures are set-based so input ordering is irrelevant.
#[must_use]
pub fn cluster_slow_frames(
    frames: &[SlowFrameObservation],
    proximity_ms: f64,
) -> Vec<SlowFrameCluster> {
    let mut sorted = frames
        .iter()
        .filter(|frame| {
            frame.timestamp_ms.is_finite()
                && frame.duration_ms.is_finite()
                && frame.duration_ms >= 0.0
        })
        .cloned()
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        frame_group_key(left)
            .cmp(&frame_group_key(right))
            .then_with(|| left.timestamp_ms.total_cmp(&right.timestamp_ms))
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut clusters = Vec::new();
    let mut pending = VecDeque::from(sorted);
    while let Some(first) = pending.pop_front() {
        let key = frame_group_key(&first);
        let mut group = vec![first];
        while pending
            .front()
            .is_some_and(|candidate| frame_group_key(candidate) == key)
        {
            if let Some(candidate) = pending.pop_front() {
                group.push(candidate);
            }
        }
        let mut current = Vec::new();
        for frame in group {
            let split = current
                .last()
                .is_some_and(|previous: &SlowFrameObservation| {
                    frame.timestamp_ms - (previous.timestamp_ms + previous.duration_ms)
                        > proximity_ms.max(0.0)
                });
            if split {
                clusters.push(finish_cluster(&current));
                current.clear();
            }
            current.push(frame);
        }
        if !current.is_empty() {
            clusters.push(finish_cluster(&current));
        }
    }
    clusters.sort_by(|left, right| {
        left.start_ms
            .total_cmp(&right.start_ms)
            .then_with(|| left.signature.cmp(&right.signature))
    });
    clusters
}

/// Builds and compares baseline/current cluster summaries without storage or UI dependencies.
#[must_use]
pub fn compare_slow_frame_clusters(
    baseline: &[SlowFrameObservation],
    current: &[SlowFrameObservation],
    policy: SlowFrameClusterPolicy,
) -> SlowFrameClusterComparison {
    let baseline_clusters = cluster_slow_frames(baseline, policy.proximity_ms);
    let current_clusters = cluster_slow_frames(current, policy.proximity_ms);
    let baseline_summary = summarize(&baseline_clusters);
    let current_summary = summarize(&current_clusters);
    let signatures = baseline_summary
        .keys()
        .chain(current_summary.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut diffs = signatures
        .into_iter()
        .map(|signature| {
            let baseline = baseline_summary.get(&signature);
            let current = current_summary.get(&signature);
            let baseline_count = baseline.map_or(0, |summary| summary.frame_count);
            let current_count = current.map_or(0, |summary| summary.frame_count);
            let baseline_duration = baseline.map_or(0.0, |summary| summary.total_duration_ms);
            let current_duration = current.map_or(0.0, |summary| summary.total_duration_ms);
            let count_delta_pct =
                percent_delta(count_as_f64(baseline_count), count_as_f64(current_count));
            let duration_delta_ms = current_duration - baseline_duration;
            let duration_delta_pct = percent_delta(baseline_duration, current_duration);
            let count_regressed = if baseline_count == 0 {
                current_count > 0
            } else {
                count_delta_pct.is_some_and(|delta| delta >= policy.count_relative_threshold_pct)
            };
            let duration_regressed = duration_delta_ms >= policy.duration_absolute_threshold_ms
                && (baseline_duration == 0.0
                    || duration_delta_pct
                        .is_some_and(|delta| delta >= policy.duration_relative_threshold_pct));
            let representative = current.or(baseline);
            SlowFrameClusterDiff {
                signature,
                baseline_count,
                current_count,
                count_delta: signed_delta(current_count, baseline_count),
                count_delta_pct,
                baseline_total_duration_ms: baseline_duration,
                current_total_duration_ms: current_duration,
                total_duration_delta_ms: duration_delta_ms,
                total_duration_delta_pct: duration_delta_pct,
                new_cluster: baseline.is_none(),
                removed_cluster: current.is_none(),
                regressed: count_regressed
                    && duration_regressed
                    && current_count >= policy.min_current_frames,
                common_components: representative
                    .map_or_else(Vec::new, |summary| summary.common_components.clone()),
                common_bottom_up_hotspots: representative.map_or_else(Vec::new, |summary| {
                    summary.common_bottom_up_hotspots.clone()
                }),
            }
        })
        .collect::<Vec<_>>();
    diffs.sort_by(|left, right| {
        right
            .regressed
            .cmp(&left.regressed)
            .then_with(|| right.count_delta.cmp(&left.count_delta))
            .then_with(|| left.signature.cmp(&right.signature))
    });
    SlowFrameClusterComparison {
        regression_count: usize_as_u64(diffs.iter().filter(|diff| diff.regressed).count()),
        baseline_clusters,
        current_clusters,
        diffs,
    }
}

#[derive(Debug, Clone)]
struct ClusterSummary {
    frame_count: u64,
    total_duration_ms: f64,
    common_components: Vec<String>,
    common_bottom_up_hotspots: Vec<String>,
}

fn summarize(clusters: &[SlowFrameCluster]) -> BTreeMap<String, ClusterSummary> {
    let mut summaries = BTreeMap::new();
    for cluster in clusters {
        let summary = summaries
            .entry(cluster.signature.clone())
            .or_insert_with(|| ClusterSummary {
                frame_count: 0,
                total_duration_ms: 0.0,
                common_components: cluster.common_components.clone(),
                common_bottom_up_hotspots: cluster.common_bottom_up_hotspots.clone(),
            });
        summary.frame_count = summary.frame_count.saturating_add(cluster.frame_count);
        summary.total_duration_ms += cluster.total_duration_ms;
        summary.common_components =
            intersection(&summary.common_components, &cluster.common_components);
        summary.common_bottom_up_hotspots = intersection(
            &summary.common_bottom_up_hotspots,
            &cluster.common_bottom_up_hotspots,
        );
    }
    summaries
}

fn finish_cluster(frames: &[SlowFrameObservation]) -> SlowFrameCluster {
    let first = &frames[0];
    let end_ms = frames
        .iter()
        .map(|frame| frame.timestamp_ms + frame.duration_ms)
        .fold(first.timestamp_ms, f64::max);
    SlowFrameCluster {
        signature: frame_group_key(first),
        step_id: first.step_id.clone(),
        jank_type: first.jank_type.clone(),
        start_ms: first.timestamp_ms,
        end_ms,
        frame_count: usize_as_u64(frames.len()),
        total_duration_ms: frames.iter().map(|frame| frame.duration_ms).sum(),
        max_duration_ms: frames
            .iter()
            .map(|frame| frame.duration_ms)
            .fold(0.0, f64::max),
        common_components: common_values(frames, |frame| &frame.components),
        common_bottom_up_hotspots: common_values(frames, |frame| &frame.bottom_up_hotspots),
        frame_ids: frames.iter().map(|frame| frame.id.clone()).collect(),
    }
}

fn frame_group_key(frame: &SlowFrameObservation) -> String {
    format!(
        "step={}|jank={}|components={}|hotspots={}",
        frame.step_id.as_deref().unwrap_or("<none>"),
        frame.jank_type,
        canonical(&frame.components).join(","),
        canonical(&frame.bottom_up_hotspots).join(",")
    )
}

fn common_values<F>(frames: &[SlowFrameObservation], values: F) -> Vec<String>
where
    F: Fn(&SlowFrameObservation) -> &[String],
{
    let Some(first) = frames.first() else {
        return vec![];
    };
    frames
        .iter()
        .skip(1)
        .fold(canonical(values(first)), |common, frame| {
            intersection(&common, &canonical(values(frame)))
        })
}

fn canonical(values: &[String]) -> Vec<String> {
    values
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn intersection(left: &[String], right: &[String]) -> Vec<String> {
    let right = right.iter().collect::<BTreeSet<_>>();
    left.iter()
        .filter(|value| right.contains(value))
        .cloned()
        .collect()
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

#[allow(clippy::cast_precision_loss)]
fn count_as_f64(value: u64) -> f64 {
    value as f64
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: &str, timestamp_ms: f64, duration_ms: f64) -> SlowFrameObservation {
        SlowFrameObservation {
            id: id.to_owned(),
            step_id: Some("scroll".to_owned()),
            timestamp_ms,
            duration_ms,
            jank_type: "deadline_missed".to_owned(),
            components: vec!["List".to_owned(), "Row".to_owned()],
            bottom_up_hotspots: vec!["renderRow".to_owned()],
        }
    }

    #[test]
    fn clustering_is_order_independent_and_splits_on_proximity() {
        let frames = vec![
            frame("late", 1_000.0, 30.0),
            frame("two", 40.0, 20.0),
            frame("one", 0.0, 20.0),
        ];
        let clusters = cluster_slow_frames(&frames, 50.0);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].frame_count, 2);
        assert_eq!(clusters[0].frame_ids, ["one", "two"]);
        assert_eq!(clusters[0].common_components, ["List", "Row"]);
    }

    #[test]
    fn comparison_detects_material_new_cluster_and_growth() {
        let baseline = vec![frame("b1", 0.0, 20.0), frame("b2", 30.0, 20.0)];
        let mut current = vec![
            frame("c1", 0.0, 30.0),
            frame("c2", 40.0, 30.0),
            frame("c3", 80.0, 30.0),
        ];
        let mut new_kind = frame("network", 500.0, 40.0);
        new_kind.jank_type = "buffer_stuffing".to_owned();
        current.push(new_kind);
        let comparison =
            compare_slow_frame_clusters(&baseline, &current, SlowFrameClusterPolicy::default());
        assert_eq!(comparison.regression_count, 1);
        assert!(
            comparison
                .diffs
                .iter()
                .any(|diff| !diff.new_cluster && diff.count_delta == 1 && diff.regressed)
        );
        assert!(
            comparison
                .diffs
                .iter()
                .any(|diff| diff.new_cluster && !diff.regressed)
        );
    }
}
