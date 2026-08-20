use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::SourceLocation;

#[derive(Debug, Error)]
pub enum StackProfileError {
    #[error("profile is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("profile must contain nodes, samples, and timeDeltas arrays")]
    Unsupported,
    #[error("samples and timeDeltas have different lengths ({samples} vs {time_deltas})")]
    SampleDeltaLength { samples: usize, time_deltas: usize },
    #[error("node {0} is missing a usable id")]
    MissingNodeId(usize),
    #[error("duplicate node id {0}")]
    DuplicateNode(String),
    #[error("node {child} has more than one parent ({first} and {second})")]
    MultipleParents {
        child: String,
        first: String,
        second: String,
    },
    #[error("node {node} references unknown parent {parent}")]
    UnknownParent { node: String, parent: String },
    #[error("sample {sample_index} references unknown node {node}")]
    UnknownSampleNode { sample_index: usize, node: String },
    #[error("cycle detected while reconstructing stack for node {0}")]
    Cycle(String),
    #[error("time delta at sample {0} is negative or not finite")]
    InvalidTimeDelta(usize),
    #[error("profile does not contain any samples")]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackFrame {
    /// Profile-local node id. It distinguishes recursive frames and equal names.
    pub node_id: String,
    /// Stable aggregation identity. Source-mapped locations should replace this
    /// identity at integration time without changing samples or durations.
    pub identity: String,
    pub name: String,
    pub source: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedSample {
    pub index: u64,
    /// Start of the interval represented by this sample, in profile microseconds.
    pub timestamp_us: f64,
    pub duration_us: f64,
    pub leaf_node_id: String,
    /// Root-to-leaf stack. Recursive calls remain separate entries.
    pub stack: Vec<StackFrame>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackAggregateNode {
    pub frame: StackFrame,
    pub sample_count: u64,
    pub self_sample_count: u64,
    pub inclusive_time_us: f64,
    pub self_time_us: f64,
    pub children: Vec<StackAggregateNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlameAggregate {
    pub path: Vec<String>,
    pub frame: StackFrame,
    pub depth: u64,
    pub sample_count: u64,
    pub self_sample_count: u64,
    pub inclusive_time_us: f64,
    pub self_time_us: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedFrame {
    pub identity: String,
    pub name: String,
    pub source: Option<SourceLocation>,
    pub self_sample_count: u64,
    pub inclusive_sample_count: u64,
    pub self_time_us: f64,
    pub inclusive_time_us: f64,
    pub self_time_pct: f64,
    pub inclusive_time_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StackConservation {
    pub total_sample_count: u64,
    pub total_time_us: f64,
    pub ranked_self_sample_count: u64,
    pub ranked_self_time_us: f64,
    pub call_tree_root_time_us: f64,
    pub bottom_up_root_time_us: f64,
    pub flame_root_time_us: f64,
    pub conserved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStackAnalysis {
    pub start_time_us: f64,
    pub end_time_us: f64,
    pub samples: Vec<TimedSample>,
    pub call_tree: Vec<StackAggregateNode>,
    pub bottom_up: Vec<StackAggregateNode>,
    pub flame: Vec<FlameAggregate>,
    pub ranked: Vec<RankedFrame>,
    pub conservation: StackConservation,
}

#[derive(Debug, Clone)]
struct NodeRecord {
    frame: StackFrame,
    parent: Option<String>,
}

#[derive(Debug, Clone)]
struct MutableAggregate {
    frame: StackFrame,
    sample_count: u64,
    self_sample_count: u64,
    inclusive_time_us: f64,
    self_time_us: f64,
    children: BTreeMap<String, MutableAggregate>,
}

impl MutableAggregate {
    fn new(frame: StackFrame) -> Self {
        Self {
            frame,
            sample_count: 0,
            self_sample_count: 0,
            inclusive_time_us: 0.0,
            self_time_us: 0.0,
            children: BTreeMap::new(),
        }
    }

    fn insert_call_path(&mut self, path: &[StackFrame], duration_us: f64) {
        self.sample_count = self.sample_count.saturating_add(1);
        self.inclusive_time_us += duration_us;
        if path.is_empty() {
            self.self_sample_count = self.self_sample_count.saturating_add(1);
            self.self_time_us += duration_us;
            return;
        }
        let frame = &path[0];
        self.children
            .entry(frame.node_id.clone())
            .or_insert_with(|| Self::new(frame.clone()))
            .insert_call_path(&path[1..], duration_us);
    }

    fn insert_bottom_path(&mut self, callers: &[StackFrame], duration_us: f64, leaf: bool) {
        self.sample_count = self.sample_count.saturating_add(1);
        self.inclusive_time_us += duration_us;
        if leaf {
            self.self_sample_count = self.self_sample_count.saturating_add(1);
            self.self_time_us += duration_us;
        }
        if let Some((caller, rest)) = callers.split_first() {
            self.children
                .entry(caller.node_id.clone())
                .or_insert_with(|| Self::new(caller.clone()))
                .insert_bottom_path(rest, duration_us, false);
        }
    }

    fn finish(self) -> StackAggregateNode {
        StackAggregateNode {
            frame: self.frame,
            sample_count: self.sample_count,
            self_sample_count: self.self_sample_count,
            inclusive_time_us: self.inclusive_time_us,
            self_time_us: self.self_time_us,
            children: self.children.into_values().map(Self::finish).collect(),
        }
    }
}

#[derive(Debug, Clone)]
struct MutableRanked {
    frame: StackFrame,
    self_sample_count: u64,
    inclusive_sample_count: u64,
    self_time_us: f64,
    inclusive_time_us: f64,
}

/// Parses a Hermes/Chrome `nodes` + `samples` + `timeDeltas` profile and
/// reconstructs every root-to-leaf stack before deriving all aggregate views.
///
/// # Errors
///
/// Returns an error for malformed JSON, invalid topology, unknown sample nodes,
/// mismatched sample arrays, invalid deltas, or an empty profile.
pub fn analyze_hermes_stack_json(json: &str) -> Result<HermesStackAnalysis, StackProfileError> {
    let value: Value = serde_json::from_str(json)?;
    reconstruct_hermes_stacks(&value)
}

/// Reconstructs and aggregates a decoded Hermes/Chrome CPU profile.
///
/// # Errors
///
/// See [`analyze_hermes_stack_json`].
pub fn reconstruct_hermes_stacks(value: &Value) -> Result<HermesStackAnalysis, StackProfileError> {
    let nodes = value
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or(StackProfileError::Unsupported)?;
    let samples = value
        .get("samples")
        .and_then(Value::as_array)
        .ok_or(StackProfileError::Unsupported)?;
    let deltas = value
        .get("timeDeltas")
        .and_then(Value::as_array)
        .ok_or(StackProfileError::Unsupported)?;
    if samples.is_empty() {
        return Err(StackProfileError::Empty);
    }
    if samples.len() != deltas.len() {
        return Err(StackProfileError::SampleDeltaLength {
            samples: samples.len(),
            time_deltas: deltas.len(),
        });
    }

    let records = parse_nodes(nodes)?;
    let start_time_us = value
        .get("startTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let mut cursor_us = start_time_us;
    let mut timed_samples = Vec::with_capacity(samples.len());
    for (index, (sample, delta)) in samples.iter().zip(deltas).enumerate() {
        let node_id = value_id(sample).ok_or_else(|| StackProfileError::UnknownSampleNode {
            sample_index: index,
            node: "<invalid>".to_owned(),
        })?;
        let duration_us = delta
            .as_f64()
            .filter(|duration| duration.is_finite() && *duration >= 0.0)
            .ok_or(StackProfileError::InvalidTimeDelta(index))?;
        let stack = stack_for(&node_id, &records)?;
        timed_samples.push(TimedSample {
            index: usize_as_u64(index),
            timestamp_us: cursor_us,
            duration_us,
            leaf_node_id: node_id,
            stack,
        });
        cursor_us += duration_us;
    }

    Ok(aggregate_samples(start_time_us, cursor_us, timed_samples))
}

fn parse_nodes(nodes: &[Value]) -> Result<HashMap<String, NodeRecord>, StackProfileError> {
    let mut records = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let id = node
            .get("id")
            .and_then(value_id)
            .ok_or(StackProfileError::MissingNodeId(index))?;
        let frame_value = node.get("callFrame").unwrap_or(node);
        let name = frame_value
            .get("functionName")
            .or_else(|| frame_value.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("(anonymous)")
            .to_owned();
        let source = source_from_call_frame(frame_value);
        let identity = frame_identity(&id, &name, source.as_ref());
        let parent = node.get("parent").and_then(value_id);
        let record = NodeRecord {
            frame: StackFrame {
                node_id: id.clone(),
                identity,
                name,
                source,
            },
            parent,
        };
        if records.insert(id.clone(), record).is_some() {
            return Err(StackProfileError::DuplicateNode(id));
        }
    }

    for node in nodes {
        let Some(parent_id) = node.get("id").and_then(value_id) else {
            continue;
        };
        for child_id in node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(value_id)
        {
            let Some(child) = records.get_mut(&child_id) else {
                return Err(StackProfileError::UnknownParent {
                    node: child_id,
                    parent: parent_id.clone(),
                });
            };
            if let Some(existing) = &child.parent
                && existing != &parent_id
            {
                return Err(StackProfileError::MultipleParents {
                    child: child_id,
                    first: existing.clone(),
                    second: parent_id.clone(),
                });
            }
            child.parent = Some(parent_id.clone());
        }
    }
    for (id, record) in &records {
        if let Some(parent) = &record.parent
            && !records.contains_key(parent)
        {
            return Err(StackProfileError::UnknownParent {
                node: id.clone(),
                parent: parent.clone(),
            });
        }
    }
    Ok(records)
}

fn stack_for(
    leaf_id: &str,
    records: &HashMap<String, NodeRecord>,
) -> Result<Vec<StackFrame>, StackProfileError> {
    let mut reversed = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(leaf_id);
    while let Some(id) = current {
        if !seen.insert(id.to_owned()) {
            return Err(StackProfileError::Cycle(id.to_owned()));
        }
        let record = records
            .get(id)
            .ok_or_else(|| StackProfileError::UnknownSampleNode {
                sample_index: 0,
                node: id.to_owned(),
            })?;
        reversed.push(record.frame.clone());
        current = record.parent.as_deref();
    }
    reversed.reverse();
    Ok(reversed)
}

fn aggregate_samples(
    start_time_us: f64,
    end_time_us: f64,
    samples: Vec<TimedSample>,
) -> HermesStackAnalysis {
    let mut call_roots = BTreeMap::<String, MutableAggregate>::new();
    let mut bottom_roots = BTreeMap::<String, MutableAggregate>::new();
    let mut ranked = BTreeMap::<String, MutableRanked>::new();

    for sample in &samples {
        if let Some((root, rest)) = sample.stack.split_first() {
            call_roots
                .entry(root.node_id.clone())
                .or_insert_with(|| MutableAggregate::new(root.clone()))
                .insert_call_path(rest, sample.duration_us);
        }
        let reversed = sample.stack.iter().rev().cloned().collect::<Vec<_>>();
        if let Some((leaf, callers)) = reversed.split_first() {
            bottom_roots
                .entry(leaf.node_id.clone())
                .or_insert_with(|| MutableAggregate::new(leaf.clone()))
                .insert_bottom_path(callers, sample.duration_us, true);
        }

        if let Some(leaf) = sample.stack.last() {
            let entry = ranked_entry(&mut ranked, leaf);
            entry.self_sample_count = entry.self_sample_count.saturating_add(1);
            entry.self_time_us += sample.duration_us;
        }
        let mut identities = BTreeSet::new();
        for frame in &sample.stack {
            if identities.insert(frame.identity.clone()) {
                let entry = ranked_entry(&mut ranked, frame);
                entry.inclusive_sample_count = entry.inclusive_sample_count.saturating_add(1);
                entry.inclusive_time_us += sample.duration_us;
            }
        }
    }

    let call_tree = call_roots
        .into_values()
        .map(MutableAggregate::finish)
        .collect::<Vec<_>>();
    let bottom_up = bottom_roots
        .into_values()
        .map(MutableAggregate::finish)
        .collect::<Vec<_>>();
    let total_time_us = samples.iter().map(|sample| sample.duration_us).sum::<f64>();
    let mut ranked = ranked
        .into_values()
        .map(|entry| RankedFrame {
            identity: entry.frame.identity,
            name: entry.frame.name,
            source: entry.frame.source,
            self_sample_count: entry.self_sample_count,
            inclusive_sample_count: entry.inclusive_sample_count,
            self_time_us: entry.self_time_us,
            inclusive_time_us: entry.inclusive_time_us,
            self_time_pct: percentage(entry.self_time_us, total_time_us),
            inclusive_time_pct: percentage(entry.inclusive_time_us, total_time_us),
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .self_time_us
            .total_cmp(&left.self_time_us)
            .then_with(|| right.inclusive_time_us.total_cmp(&left.inclusive_time_us))
            .then_with(|| left.identity.cmp(&right.identity))
    });

    let mut flame = Vec::new();
    for root in &call_tree {
        flatten_flame(root, &mut Vec::new(), &mut flame);
    }
    let ranked_self_sample_count = ranked.iter().map(|frame| frame.self_sample_count).sum();
    let ranked_self_time_us = ranked.iter().map(|frame| frame.self_time_us).sum::<f64>();
    let call_tree_root_time_us = call_tree.iter().map(|root| root.inclusive_time_us).sum();
    let bottom_up_root_time_us = bottom_up.iter().map(|root| root.inclusive_time_us).sum();
    let flame_root_time_us = flame
        .iter()
        .filter(|entry| entry.depth == 0)
        .map(|entry| entry.inclusive_time_us)
        .sum();
    let total_sample_count = usize_as_u64(samples.len());
    let conserved = ranked_self_sample_count == total_sample_count
        && approximately_equal(ranked_self_time_us, total_time_us)
        && approximately_equal(call_tree_root_time_us, total_time_us)
        && approximately_equal(bottom_up_root_time_us, total_time_us)
        && approximately_equal(flame_root_time_us, total_time_us);

    HermesStackAnalysis {
        start_time_us,
        end_time_us,
        samples,
        call_tree,
        bottom_up,
        flame,
        ranked,
        conservation: StackConservation {
            total_sample_count,
            total_time_us,
            ranked_self_sample_count,
            ranked_self_time_us,
            call_tree_root_time_us,
            bottom_up_root_time_us,
            flame_root_time_us,
            conserved,
        },
    }
}

fn ranked_entry<'a>(
    ranked: &'a mut BTreeMap<String, MutableRanked>,
    frame: &StackFrame,
) -> &'a mut MutableRanked {
    ranked
        .entry(frame.identity.clone())
        .or_insert_with(|| MutableRanked {
            frame: frame.clone(),
            self_sample_count: 0,
            inclusive_sample_count: 0,
            self_time_us: 0.0,
            inclusive_time_us: 0.0,
        })
}

fn flatten_flame(
    node: &StackAggregateNode,
    path: &mut Vec<String>,
    output: &mut Vec<FlameAggregate>,
) {
    path.push(node.frame.identity.clone());
    output.push(FlameAggregate {
        path: path.clone(),
        frame: node.frame.clone(),
        depth: usize_as_u64(path.len().saturating_sub(1)),
        sample_count: node.sample_count,
        self_sample_count: node.self_sample_count,
        inclusive_time_us: node.inclusive_time_us,
        self_time_us: node.self_time_us,
    });
    for child in &node.children {
        flatten_flame(child, path, output);
    }
    path.pop();
}

fn source_from_call_frame(frame: &Value) -> Option<SourceLocation> {
    let file = frame
        .get("url")
        .or_else(|| frame.get("file"))
        .and_then(Value::as_str)
        .filter(|file| !file.is_empty())?
        .to_owned();
    Some(SourceLocation {
        file,
        line: frame
            .get("lineNumber")
            .or_else(|| frame.get("line"))
            .and_then(Value::as_u64)
            .map(|line| line.saturating_add(1)),
        column: frame
            .get("columnNumber")
            .or_else(|| frame.get("column"))
            .and_then(Value::as_u64)
            .map(|column| column.saturating_add(1)),
    })
}

fn frame_identity(id: &str, name: &str, source: Option<&SourceLocation>) -> String {
    source.map_or_else(
        || format!("node:{id}:{name}"),
        |source| {
            format!(
                "{}:{}:{}:{name}",
                source.file,
                source.line.unwrap_or_default(),
                source.column.unwrap_or_default()
            )
        },
    )
}

fn value_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|id| id.to_string()))
}

fn percentage(value: f64, total: f64) -> f64 {
    if total > 0.0 {
        value / total * 100.0
    } else {
        0.0
    }
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let tolerance = right.abs().max(1.0) * 1.0e-9;
    (left - right).abs() <= tolerance
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reconstructs_recursive_stacks_and_conserves_every_view() {
        let profile = json!({
            "startTime": 1_000,
            "nodes": [
                {"id": 1, "callFrame": {"functionName": "(root)"}, "children": [2]},
                {"id": 2, "callFrame": {"functionName": "recurse", "url": "app.js", "lineNumber": 9, "columnNumber": 0}, "children": [3, 4]},
                {"id": 3, "callFrame": {"functionName": "recurse", "url": "app.js", "lineNumber": 9, "columnNumber": 0}, "children": [5]},
                {"id": 4, "callFrame": {"functionName": "sameName", "url": "left.js", "lineNumber": 0, "columnNumber": 0}},
                {"id": 5, "callFrame": {"functionName": "sameName", "url": "right.js", "lineNumber": 0, "columnNumber": 0}}
            ],
            "samples": [3, 4, 5],
            "timeDeltas": [1_000, 2_000, 3_000]
        });

        let analysis = reconstruct_hermes_stacks(&profile).unwrap();
        assert_eq!(analysis.samples[0].stack.len(), 3);
        assert_eq!(analysis.samples[0].stack[1].name, "recurse");
        assert_eq!(analysis.samples[0].stack[2].name, "recurse");
        assert!((analysis.samples[1].timestamp_us - 2_000.0).abs() < f64::EPSILON);
        assert!((analysis.end_time_us - 7_000.0).abs() < f64::EPSILON);
        assert!(analysis.conservation.conserved);
        assert_eq!(analysis.conservation.total_sample_count, 3);
        assert!((analysis.conservation.total_time_us - 6_000.0).abs() < f64::EPSILON);

        let same_name = analysis
            .ranked
            .iter()
            .filter(|frame| frame.name == "sameName")
            .collect::<Vec<_>>();
        assert_eq!(
            same_name.len(),
            2,
            "locations must disambiguate equal names"
        );
        let recurse = analysis
            .ranked
            .iter()
            .find(|frame| frame.name == "recurse")
            .unwrap();
        assert_eq!(recurse.inclusive_sample_count, 3);
        assert!((recurse.inclusive_time_us - 6_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn rejects_cycles_and_mismatched_sample_deltas() {
        let mismatch = json!({
            "nodes": [{"id": 1}],
            "samples": [1],
            "timeDeltas": []
        });
        assert!(matches!(
            reconstruct_hermes_stacks(&mismatch),
            Err(StackProfileError::SampleDeltaLength { .. })
        ));

        let cycle = json!({
            "nodes": [
                {"id": 1, "parent": 2},
                {"id": 2, "parent": 1}
            ],
            "samples": [1],
            "timeDeltas": [1]
        });
        assert!(matches!(
            reconstruct_hermes_stacks(&cycle),
            Err(StackProfileError::Cycle(_))
        ));
    }
}
