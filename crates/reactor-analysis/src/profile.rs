use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "unsupported profile: expected React DevTools dataForRoots or a Hermes/Chrome CPU profile"
    )]
    Unsupported,
    #[error("profile does not contain any samples or component durations")]
    Empty,
    #[error("source map is not valid: {0}")]
    SourceMap(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticProfileType {
    ReactProfiler,
    HermesCpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceLocation {
    pub file: String,
    pub line: Option<u64>,
    pub column: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentChangeEvidence {
    pub component_id: String,
    pub props: Vec<String>,
    pub state: Vec<String>,
    pub context: Vec<String>,
    pub hooks: Vec<u64>,
    pub did_hooks_change: bool,
    pub is_first_mount: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCommit {
    pub id: String,
    pub root_id: String,
    pub index: u64,
    pub timestamp_ms: Option<f64>,
    pub duration_ms: Option<f64>,
    pub rendered_component_ids: Vec<String>,
    pub changed_component_ids: Vec<String>,
    pub updater_component_ids: Vec<String>,
    pub changes: Vec<ComponentChangeEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProfileStat {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub parent_name: Option<String>,
    pub source: Option<SourceLocation>,
    pub render_count: u64,
    pub commit_count: u64,
    pub changed_render_count: u64,
    pub unchanged_render_count: u64,
    pub updater_count: u64,
    pub total_time_ms: f64,
    pub self_time_ms: f64,
    pub average_time_ms: f64,
    pub p50_time_ms: f64,
    pub p95_time_ms: f64,
    pub max_time_ms: f64,
    pub commit_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionProfileStat {
    pub id: String,
    pub name: String,
    pub source: Option<SourceLocation>,
    pub sample_count: u64,
    pub self_time_ms: f64,
    pub self_time_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFinding {
    pub rule_id: String,
    pub severity: DiagnosticSeverity,
    pub title: String,
    pub summary: String,
    pub component_id: Option<String>,
    pub component_name: Option<String>,
    pub commit_ids: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub source: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticProfileReport {
    pub schema_version: u32,
    pub profile_type: DiagnosticProfileType,
    pub source_format: String,
    pub profile_id: String,
    pub root_count: u64,
    pub commit_count: u64,
    pub total_duration_ms: f64,
    pub components: Vec<ComponentProfileStat>,
    pub functions: Vec<FunctionProfileStat>,
    pub commits: Vec<ProfileCommit>,
    pub findings: Vec<DiagnosticFinding>,
    pub warnings: Vec<String>,
    pub source_map_applied: bool,
    pub source_map_mapped_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProfileDiff {
    pub key: String,
    pub name: String,
    pub source: Option<SourceLocation>,
    pub baseline_render_count: u64,
    pub current_render_count: u64,
    pub render_count_delta: i64,
    pub render_count_delta_pct: Option<f64>,
    pub baseline_total_time_ms: f64,
    pub current_total_time_ms: f64,
    pub total_time_delta_ms: f64,
    pub total_time_delta_pct: Option<f64>,
    pub regressed: bool,
    pub new_component: bool,
    pub removed_component: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDiffReport {
    pub schema_version: u32,
    pub compatible: bool,
    pub reasons: Vec<String>,
    pub components: Vec<ComponentProfileDiff>,
    pub regression_count: u64,
}

#[derive(Debug, Default)]
struct MutableComponent {
    id: String,
    name: String,
    parent_id: Option<String>,
    source: Option<SourceLocation>,
    actual_durations: Vec<f64>,
    self_time_ms: f64,
    changed_count: u64,
    updater_count: u64,
    commit_ids: Vec<String>,
}

/// Parses a React `DevTools` Profiler export or Hermes/Chrome CPU profile.
///
/// # Errors
///
/// Returns [`ProfileError::Json`] for malformed JSON, [`ProfileError::Unsupported`]
/// for an unknown profile format, and [`ProfileError::Empty`] when the profile
/// contains no usable duration samples.
pub fn analyze_profile_json(json: &str) -> Result<DiagnosticProfileReport, ProfileError> {
    let value: Value = serde_json::from_str(json)?;
    if value
        .get("dataForRoots")
        .and_then(Value::as_array)
        .is_some()
    {
        return parse_react_profile(&value);
    }
    if value.get("nodes").and_then(Value::as_array).is_some()
        || value.get("traceEvents").and_then(Value::as_array).is_some()
    {
        return parse_hermes_profile(&value);
    }
    Err(ProfileError::Unsupported)
}

fn parse_react_profile(value: &Value) -> Result<DiagnosticProfileReport, ProfileError> {
    let roots = value
        .get("dataForRoots")
        .and_then(Value::as_array)
        .ok_or(ProfileError::Unsupported)?;
    let source_locations = parse_source_locations(value.get("sourceLocations"));
    let (components, commits, total_duration_ms) = collect_react_data(roots, &source_locations);
    if components
        .values()
        .all(|component| component.actual_durations.is_empty())
    {
        return Err(ProfileError::Empty);
    }
    let stats = component_stats(components);
    let findings = react_findings(&stats);
    Ok(DiagnosticProfileReport {
        schema_version: 1,
        profile_type: DiagnosticProfileType::ReactProfiler,
        source_format: format!(
            "react-devtools-profiler-v{}",
            value.get("version").and_then(Value::as_u64).unwrap_or(0)
        ),
        profile_id: profile_id(value, "react"),
        root_count: usize_as_u64(roots.len()),
        commit_count: usize_as_u64(commits.len()),
        total_duration_ms,
        components: stats,
        functions: vec![],
        commits,
        findings,
        warnings: vec![
            "React DevTools 导出通常不含完整 Props/State；无变化判定以 changeDescriptions 可用性为准。"
                .to_owned(),
        ],
        source_map_applied: false,
        source_map_mapped_count: 0,
    })
}

fn collect_react_data(
    roots: &[Value],
    source_locations: &HashMap<String, SourceLocation>,
) -> (BTreeMap<String, MutableComponent>, Vec<ProfileCommit>, f64) {
    let mut components = BTreeMap::<String, MutableComponent>::new();
    let mut commits = Vec::new();
    let mut total_duration_ms = 0.0;
    for root in roots {
        let root_id = id_string(root.get("rootID")).unwrap_or_else(|| "root".to_owned());
        let snapshots = root
            .get("snapshots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut parent_ids = HashMap::<String, String>::new();
        for snapshot in &snapshots {
            let Some((id, node)) = snapshot_entry(snapshot) else {
                continue;
            };
            for child in node
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|child| id_string(Some(child)))
            {
                parent_ids.insert(child, id.clone());
            }
            let name = node
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map_or_else(|| format!("Component #{id}"), str::to_owned);
            components
                .entry(id.clone())
                .or_insert_with(|| MutableComponent {
                    id: id.clone(),
                    name,
                    source: source_locations.get(&id).cloned(),
                    ..MutableComponent::default()
                });
        }
        for (child, parent) in parent_ids {
            components
                .entry(child.clone())
                .or_insert_with(|| MutableComponent {
                    id: child.clone(),
                    name: format!("Component #{child}"),
                    source: source_locations.get(&child).cloned(),
                    ..MutableComponent::default()
                })
                .parent_id = Some(parent);
        }
        for (index, commit) in root
            .get("commitData")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let profile_commit =
                collect_react_commit(&root_id, index, commit, source_locations, &mut components);
            total_duration_ms += profile_commit.duration_ms.unwrap_or_default();
            commits.push(profile_commit);
        }
    }
    (components, commits, total_duration_ms)
}

fn collect_react_commit(
    root_id: &str,
    index: usize,
    commit: &Value,
    source_locations: &HashMap<String, SourceLocation>,
    components: &mut BTreeMap<String, MutableComponent>,
) -> ProfileCommit {
    let commit_id = format!("{root_id}:{index}");
    let self_durations = duration_pairs(commit.get("fiberSelfDurations"))
        .into_iter()
        .collect::<HashMap<_, _>>();
    let change_evidence = component_changes(commit.get("changeDescriptions"));
    let changed_ids = change_evidence
        .iter()
        .map(|change| change.component_id.clone())
        .collect::<BTreeSet<_>>();
    let updater_ids = updater_component_ids(commit.get("updaters"));
    let mut rendered_ids = Vec::new();
    for (id, duration) in duration_pairs(commit.get("fiberActualDurations")) {
        let component = components
            .entry(id.clone())
            .or_insert_with(|| MutableComponent {
                id: id.clone(),
                name: format!("Component #{id}"),
                source: source_locations.get(&id).cloned(),
                ..MutableComponent::default()
            });
        component.actual_durations.push(duration);
        component.self_time_ms += self_durations.get(&id).copied().unwrap_or_default();
        component.changed_count += u64::from(changed_ids.contains(&id));
        component.commit_ids.push(commit_id.clone());
        rendered_ids.push(id);
    }
    for updater_id in &updater_ids {
        if let Some(component) = components.get_mut(updater_id) {
            component.updater_count += 1;
        }
    }
    ProfileCommit {
        id: commit_id,
        root_id: root_id.to_owned(),
        index: usize_as_u64(index),
        timestamp_ms: commit.get("timestamp").and_then(Value::as_f64),
        duration_ms: commit.get("duration").and_then(Value::as_f64),
        rendered_component_ids: rendered_ids,
        changed_component_ids: changed_ids.into_iter().collect(),
        updater_component_ids: updater_ids,
        changes: change_evidence,
    }
}

fn component_stats(components: BTreeMap<String, MutableComponent>) -> Vec<ComponentProfileStat> {
    let names = components
        .iter()
        .map(|(id, component)| (id.clone(), component.name.clone()))
        .collect::<HashMap<_, _>>();
    let mut stats = components
        .into_values()
        .map(|mut component| {
            component.actual_durations.sort_by(f64::total_cmp);
            let render_count = usize_as_u64(component.actual_durations.len());
            let total_time_ms = component.actual_durations.iter().sum::<f64>();
            ComponentProfileStat {
                id: component.id,
                name: component.name,
                parent_name: component
                    .parent_id
                    .as_ref()
                    .and_then(|id| names.get(id).cloned()),
                parent_id: component.parent_id,
                source: component.source,
                render_count,
                commit_count: usize_as_u64(component.commit_ids.len()),
                changed_render_count: component.changed_count,
                unchanged_render_count: render_count.saturating_sub(component.changed_count),
                updater_count: component.updater_count,
                total_time_ms,
                self_time_ms: component.self_time_ms,
                average_time_ms: divide(total_time_ms, render_count),
                p50_time_ms: percentile(&component.actual_durations, 50, 100),
                p95_time_ms: percentile(&component.actual_durations, 95, 100),
                max_time_ms: component
                    .actual_durations
                    .last()
                    .copied()
                    .unwrap_or_default(),
                commit_ids: component.commit_ids,
            }
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        right
            .render_count
            .cmp(&left.render_count)
            .then_with(|| right.total_time_ms.total_cmp(&left.total_time_ms))
    });
    stats
}

fn parse_hermes_profile(value: &Value) -> Result<DiagnosticProfileReport, ProfileError> {
    let (functions, total_duration_ms) = hermes_functions(value);
    if functions.is_empty() || total_duration_ms <= 0.0 {
        return Err(ProfileError::Empty);
    }
    let mut functions = functions.into_values().collect::<Vec<_>>();
    for function in &mut functions {
        function.self_time_pct = function.self_time_ms / total_duration_ms * 100.0;
    }
    functions.sort_by(|left, right| right.self_time_ms.total_cmp(&left.self_time_ms));
    let findings = functions
        .iter()
        .filter(|function| function.self_time_pct >= 15.0)
        .take(10)
        .map(|function| DiagnosticFinding {
            rule_id: "hot-js-function".to_owned(),
            severity: DiagnosticSeverity::Warning,
            title: format!("JS 热点函数：{}", function.name),
            summary: format!("该函数占采样 Self Time 的 {:.1}%。", function.self_time_pct),
            component_id: None,
            component_name: None,
            commit_ids: vec![],
            evidence_refs: vec![format!("functions.{}.selfTimeMs", function.id)],
            source: function.source.clone(),
        })
        .collect();
    Ok(DiagnosticProfileReport {
        schema_version: 1,
        profile_type: DiagnosticProfileType::HermesCpu,
        source_format: "hermes-chrome-cpu-profile-v1".to_owned(),
        profile_id: profile_id(value, "hermes"),
        root_count: 0,
        commit_count: 0,
        total_duration_ms,
        components: vec![],
        functions,
        commits: vec![],
        findings,
        warnings: vec![
            "Hermes CPU Profile 用于 JS 调用热点，不提供 React 组件 Render 次数。".to_owned(),
        ],
        source_map_applied: false,
        source_map_mapped_count: 0,
    })
}

/// Maps generated profile locations to their original source locations using a
/// Source Map v3 document.
///
/// # Errors
///
/// Returns [`ProfileError::SourceMap`] when the Source Map cannot be decoded.
pub fn apply_source_map_json(
    report: &mut DiagnosticProfileReport,
    source_map_json: &str,
) -> Result<u64, ProfileError> {
    let source_map = sourcemap::SourceMap::from_slice(source_map_json.as_bytes())
        .map_err(|error| ProfileError::SourceMap(error.to_string()))?;
    let mut mapped_count = 0_u64;
    for component in &mut report.components {
        mapped_count += u64::from(map_source_location(&source_map, &mut component.source));
    }
    for function in &mut report.functions {
        mapped_count += u64::from(map_source_location(&source_map, &mut function.source));
    }
    for finding in &mut report.findings {
        if let Some(component_id) = &finding.component_id {
            finding.source = report
                .components
                .iter()
                .find(|component| &component.id == component_id)
                .and_then(|component| component.source.clone());
        } else if let Some(function_id) = finding
            .evidence_refs
            .first()
            .and_then(|reference| reference.strip_prefix("functions."))
            .and_then(|reference| reference.strip_suffix(".selfTimeMs"))
        {
            finding.source = report
                .functions
                .iter()
                .find(|function| function.id == function_id)
                .and_then(|function| function.source.clone());
        }
    }
    report.source_map_applied = true;
    report.source_map_mapped_count = mapped_count;
    if mapped_count == 0 {
        report
            .warnings
            .push("Source Map 已读取，但没有找到与 Profile 位置匹配的映射。".to_owned());
    }
    Ok(mapped_count)
}

fn map_source_location(
    source_map: &sourcemap::SourceMap,
    location: &mut Option<SourceLocation>,
) -> bool {
    let Some(generated) = location else {
        return false;
    };
    let Some(line) = generated
        .line
        .and_then(|line| u32::try_from(line.saturating_sub(1)).ok())
    else {
        return false;
    };
    let column = generated
        .column
        .and_then(|column| u32::try_from(column.saturating_sub(1)).ok())
        .unwrap_or(0);
    let Some(token) = source_map.lookup_token(line, column) else {
        return false;
    };
    let Some(file) = token.get_source() else {
        return false;
    };
    *generated = SourceLocation {
        file: file.to_owned(),
        line: Some(u64::from(token.get_src_line()) + 1),
        column: Some(u64::from(token.get_src_col()) + 1),
    };
    true
}

fn hermes_functions(value: &Value) -> (BTreeMap<String, FunctionProfileStat>, f64) {
    if let Some(nodes) = value.get("nodes").and_then(Value::as_array) {
        return chrome_cpu_functions(value, nodes);
    }
    value
        .get("traceEvents")
        .and_then(Value::as_array)
        .map_or_else(
            || (BTreeMap::new(), 0.0),
            |events| trace_event_functions(events),
        )
}

fn chrome_cpu_functions(
    value: &Value,
    nodes: &[Value],
) -> (BTreeMap<String, FunctionProfileStat>, f64) {
    let mut functions = BTreeMap::<String, FunctionProfileStat>::new();
    for node in nodes {
        let Some(id) = id_string(node.get("id")) else {
            continue;
        };
        let frame = node.get("callFrame").unwrap_or(node);
        let name = frame
            .get("functionName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("(anonymous)")
            .to_owned();
        functions.insert(
            id.clone(),
            FunctionProfileStat {
                id,
                name,
                source: source_from_frame(frame),
                sample_count: 0,
                self_time_ms: 0.0,
                self_time_pct: 0.0,
            },
        );
    }
    let samples = value
        .get("samples")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let deltas = value
        .get("timeDeltas")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut total_duration_ms = 0.0;
    for (index, sample) in samples.iter().enumerate() {
        let Some(id) = id_string(Some(sample)) else {
            continue;
        };
        let duration_ms = deltas
            .get(index)
            .and_then(Value::as_f64)
            .unwrap_or_default()
            / 1_000.0;
        if let Some(function) = functions.get_mut(&id) {
            function.sample_count += 1;
            function.self_time_ms += duration_ms;
        }
        total_duration_ms += duration_ms;
    }
    (functions, total_duration_ms)
}

fn trace_event_functions(events: &[Value]) -> (BTreeMap<String, FunctionProfileStat>, f64) {
    let mut functions = BTreeMap::<String, FunctionProfileStat>::new();
    let mut total_duration_ms = 0.0;
    for (index, event) in events.iter().enumerate() {
        if event.get("ph").and_then(Value::as_str) != Some("X") {
            continue;
        }
        let duration_ms = event.get("dur").and_then(Value::as_f64).unwrap_or_default() / 1_000.0;
        let name = event
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(anonymous)")
            .to_owned();
        let id = format!("trace:{index}");
        functions.insert(
            id.clone(),
            FunctionProfileStat {
                id,
                name,
                source: None,
                sample_count: 1,
                self_time_ms: duration_ms,
                self_time_pct: 0.0,
            },
        );
        total_duration_ms += duration_ms;
    }
    (functions, total_duration_ms)
}

#[must_use]
pub fn diff_profile_reports(
    baseline: &DiagnosticProfileReport,
    current: &DiagnosticProfileReport,
) -> ProfileDiffReport {
    const MIN_TOTAL_TIME_REGRESSION_MS: f64 = 5.0;
    let mut reasons = Vec::new();
    if baseline.profile_type != current.profile_type {
        reasons.push("Profile 类型不同".to_owned());
    }
    if baseline.profile_type != DiagnosticProfileType::ReactProfiler {
        reasons.push("组件 Render Diff 仅适用于 React Profiler".to_owned());
    }
    let baseline_map = baseline
        .components
        .iter()
        .map(|component| (component_key(component), component))
        .collect::<BTreeMap<_, _>>();
    let current_map = current
        .components
        .iter()
        .map(|component| (component_key(component), component))
        .collect::<BTreeMap<_, _>>();
    let keys = baseline_map
        .keys()
        .chain(current_map.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut components = keys
        .into_iter()
        .map(|key| {
            let left = baseline_map.get(&key).copied();
            let right = current_map.get(&key).copied();
            let baseline_render_count = left.map_or(0, |value| value.render_count);
            let current_render_count = right.map_or(0, |value| value.render_count);
            let baseline_total_time_ms = left.map_or(0.0, |value| value.total_time_ms);
            let current_total_time_ms = right.map_or(0.0, |value| value.total_time_ms);
            let render_count_delta = signed_delta(current_render_count, baseline_render_count);
            let total_time_delta_ms = current_total_time_ms - baseline_total_time_ms;
            let render_count_delta_pct = percent_delta(
                count_as_f64(baseline_render_count),
                count_as_f64(current_render_count),
            );
            let total_time_delta_pct = percent_delta(baseline_total_time_ms, current_total_time_ms);
            let regressed = render_count_delta_pct.is_some_and(|value| value > 20.0)
                || (total_time_delta_ms >= MIN_TOTAL_TIME_REGRESSION_MS
                    && total_time_delta_pct.is_some_and(|value| value > 20.0));
            let Some(component) = right.or(left) else {
                return ComponentProfileDiff {
                    key,
                    name: "Unknown component".to_owned(),
                    source: None,
                    baseline_render_count: 0,
                    current_render_count: 0,
                    render_count_delta: 0,
                    render_count_delta_pct: None,
                    baseline_total_time_ms: 0.0,
                    current_total_time_ms: 0.0,
                    total_time_delta_ms: 0.0,
                    total_time_delta_pct: None,
                    regressed: false,
                    new_component: false,
                    removed_component: false,
                };
            };
            ComponentProfileDiff {
                key,
                name: component.name.clone(),
                source: component.source.clone(),
                baseline_render_count,
                current_render_count,
                render_count_delta,
                render_count_delta_pct,
                baseline_total_time_ms,
                current_total_time_ms,
                total_time_delta_ms,
                total_time_delta_pct,
                regressed,
                new_component: left.is_none(),
                removed_component: right.is_none(),
            }
        })
        .collect::<Vec<_>>();
    components.sort_by(|left, right| {
        right
            .regressed
            .cmp(&left.regressed)
            .then_with(|| right.render_count_delta.cmp(&left.render_count_delta))
    });
    ProfileDiffReport {
        schema_version: 1,
        compatible: reasons.is_empty(),
        reasons,
        regression_count: usize_as_u64(
            components
                .iter()
                .filter(|component| component.regressed)
                .count(),
        ),
        components,
    }
}

fn react_findings(components: &[ComponentProfileStat]) -> Vec<DiagnosticFinding> {
    let by_id = components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<HashMap<_, _>>();
    let mut findings = Vec::new();
    for component in components {
        if component.render_count >= 3 && component.unchanged_render_count >= 3 {
            findings.push(DiagnosticFinding {
                rule_id: "repeated-render-without-change".to_owned(),
                severity: if component.render_count >= 10 {
                    DiagnosticSeverity::Critical
                } else {
                    DiagnosticSeverity::Warning
                },
                title: format!("{} 可能存在重复 Render", component.name),
                summary: format!(
                    "记录到 {} 次 Render，其中 {} 次没有 changeDescriptions 证据。",
                    component.render_count, component.unchanged_render_count
                ),
                component_id: Some(component.id.clone()),
                component_name: Some(component.name.clone()),
                commit_ids: component.commit_ids.clone(),
                evidence_refs: vec![
                    format!("components.{}.renderCount", component.id),
                    format!("components.{}.unchangedRenderCount", component.id),
                ],
                source: component.source.clone(),
            });
        }
        if let Some(parent) = component.parent_id.as_deref().and_then(|id| by_id.get(id))
            && component.render_count >= 3
            && parent.render_count >= 3
            && component.render_count * 10 >= parent.render_count * 8
        {
            findings.push(DiagnosticFinding {
                rule_id: "parent-cascade-render".to_owned(),
                severity: DiagnosticSeverity::Warning,
                title: format!("{} 疑似受父组件级联更新", component.name),
                summary: format!(
                    "父组件 {} Render {} 次，子组件同步 Render {} 次。",
                    parent.name, parent.render_count, component.render_count
                ),
                component_id: Some(component.id.clone()),
                component_name: Some(component.name.clone()),
                commit_ids: component.commit_ids.clone(),
                evidence_refs: vec![
                    format!("components.{}.renderCount", parent.id),
                    format!("components.{}.renderCount", component.id),
                ],
                source: component.source.clone(),
            });
        }
    }
    findings
}

fn duration_pairs(value: Option<&Value>) -> Vec<(String, f64)> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            Some((id_string(pair.first())?, pair.get(1)?.as_f64()?))
        })
        .collect()
}

fn snapshot_entry(value: &Value) -> Option<(String, &Value)> {
    if let Some(pair) = value.as_array() {
        return Some((id_string(pair.first())?, pair.get(1)?));
    }
    Some((id_string(value.get("id"))?, value))
}

fn component_changes(value: Option<&Value>) -> Vec<ComponentChangeEvidence> {
    let entries = match value {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(id, description)| (id.clone(), description))
            .collect::<Vec<_>>(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let pair = item.as_array()?;
                Some((id_string(pair.first())?, pair.get(1)?))
            })
            .collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .map(|(component_id, description)| ComponentChangeEvidence {
            component_id,
            props: string_array(description.get("props")),
            state: string_array(description.get("state")),
            context: context_changes(description.get("context")),
            hooks: description
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect(),
            did_hooks_change: description
                .get("didHooksChange")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            is_first_mount: description
                .get("isFirstMount")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

fn updater_component_ids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|updater| id_string(updater.get("id")))
        .collect()
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn context_changes(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Bool(true)) => vec!["Context value".to_owned()],
        other => string_array(other),
    }
}

fn parse_source_locations(value: Option<&Value>) -> HashMap<String, SourceLocation> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.iter())
        .filter_map(|(id, location)| {
            Some((
                id.clone(),
                SourceLocation {
                    file: location
                        .get("file")
                        .or_else(|| location.get("url"))
                        .and_then(Value::as_str)?
                        .to_owned(),
                    line: location
                        .get("line")
                        .or_else(|| location.get("lineNumber"))
                        .and_then(Value::as_u64),
                    column: location
                        .get("column")
                        .or_else(|| location.get("columnNumber"))
                        .and_then(Value::as_u64),
                },
            ))
        })
        .collect()
}

fn source_from_frame(frame: &Value) -> Option<SourceLocation> {
    let file = frame
        .get("url")
        .and_then(Value::as_str)
        .filter(|file| !file.is_empty())?
        .to_owned();
    Some(SourceLocation {
        file,
        line: frame
            .get("lineNumber")
            .and_then(Value::as_u64)
            .map(|line| line + 1),
        column: frame
            .get("columnNumber")
            .and_then(Value::as_u64)
            .map(|column| column + 1),
    })
}

fn component_key(component: &ComponentProfileStat) -> String {
    component.source.as_ref().map_or_else(
        || component.name.clone(),
        |source| {
            format!(
                "{}:{}:{}",
                component.name,
                source.file,
                source.line.unwrap_or_default()
            )
        },
    )
}

fn id_string(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|id| id.to_string()))
    })
}

fn divide(total: f64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        total / count_as_f64(count)
    }
}

fn percentile(sorted: &[f64], numerator: usize, denominator: usize) -> f64 {
    if sorted.is_empty() || denominator == 0 {
        return 0.0;
    }
    let last = sorted.len() - 1;
    let index = last
        .saturating_mul(numerator)
        .saturating_add(denominator / 2)
        / denominator;
    sorted[index.min(sorted.len() - 1)]
}

fn usize_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
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
    // Profile counts are bounded by in-memory vectors and cannot approach 2^53
    // in a practical Reactor run. Keep the report schema at u64 for portability.
    value as f64
}

fn percent_delta(baseline: f64, current: f64) -> Option<f64> {
    (baseline.abs() > f64::EPSILON).then(|| (current - baseline) / baseline.abs() * 100.0)
}

fn profile_id(value: &Value, prefix: &str) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{prefix}-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn react_profile(render_multiplier: usize) -> String {
        let commits = (0..render_multiplier)
            .map(|index| {
                json!({
                    "timestamp": f64::from(u32::try_from(index).unwrap_or(u32::MAX)) * 16.0,
                    "duration": 8.0,
                    "fiberActualDurations": [[1, 6.0], [2, 4.0]],
                    "fiberSelfDurations": [[1, 2.0], [2, 3.0]],
                    "changeDescriptions": {}
                })
            })
            .collect::<Vec<_>>();
        json!({
            "version": 5,
            "sourceLocations": { "2": { "file": "src/ListItem.tsx", "line": 42, "column": 3 } },
            "dataForRoots": [{
                "rootID": 10,
                "snapshots": [
                    { "id": 1, "displayName": "List", "children": [2] },
                    { "id": 2, "displayName": "ListItem", "children": [] }
                ],
                "commitData": commits
            }]
        })
        .to_string()
    }

    #[test]
    fn react_profile_reports_component_render_counts_and_cascade() {
        let report = analyze_profile_json(&react_profile(4)).unwrap();
        assert_eq!(report.profile_type, DiagnosticProfileType::ReactProfiler);
        assert_eq!(report.commit_count, 4);
        let item = report
            .components
            .iter()
            .find(|component| component.name == "ListItem")
            .unwrap();
        assert_eq!(item.render_count, 4);
        assert_eq!(item.unchanged_render_count, 4);
        assert_eq!(item.source.as_ref().unwrap().line, Some(42));
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.rule_id == "repeated-render-without-change")
        );
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.rule_id == "parent-cascade-render")
        );
    }

    #[test]
    fn profile_diff_detects_render_count_regression() {
        let baseline = analyze_profile_json(&react_profile(4)).unwrap();
        let current = analyze_profile_json(&react_profile(8)).unwrap();
        let diff = diff_profile_reports(&baseline, &current);
        assert!(diff.compatible);
        assert_eq!(diff.regression_count, 2);
        assert!(
            diff.components
                .iter()
                .all(|component| component.render_count_delta == 4)
        );
    }

    #[test]
    fn profile_diff_ignores_small_absolute_timing_noise() {
        let mut baseline = analyze_profile_json(&react_profile(2)).unwrap();
        let mut current = baseline.clone();
        for component in &mut baseline.components {
            component.total_time_ms = 1.0;
        }
        for component in &mut current.components {
            component.total_time_ms = 1.5;
        }

        let noisy_diff = diff_profile_reports(&baseline, &current);
        assert_eq!(noisy_diff.regression_count, 0);

        current.components[0].total_time_ms = 7.0;
        let material_diff = diff_profile_reports(&baseline, &current);
        assert_eq!(material_diff.regression_count, 1);
    }

    #[test]
    fn hermes_cpu_profile_reports_hot_functions_and_sources() {
        let profile = json!({
            "nodes": [
                { "id": 1, "callFrame": { "functionName": "renderList", "url": "src/list.ts", "lineNumber": 9, "columnNumber": 2 } },
                { "id": 2, "callFrame": { "functionName": "other", "url": "src/other.ts", "lineNumber": 1, "columnNumber": 0 } }
            ],
            "samples": [1, 1, 1, 2],
            "timeDeltas": [1000, 1000, 1000, 1000]
        }).to_string();
        let report = analyze_profile_json(&profile).unwrap();
        assert_eq!(report.profile_type, DiagnosticProfileType::HermesCpu);
        assert_eq!(report.functions[0].name, "renderList");
        assert!((report.functions[0].self_time_pct - 75.0).abs() < f64::EPSILON);
        assert_eq!(report.functions[0].source.as_ref().unwrap().line, Some(10));
    }

    #[test]
    fn shipped_diagnostic_fixtures_round_trip_and_detect_regression() {
        let baseline = analyze_profile_json(include_str!(
            "../../../tests/fixtures/react-profiler-baseline.json"
        ))
        .unwrap();
        let current = analyze_profile_json(include_str!(
            "../../../tests/fixtures/react-profiler-regressed.json"
        ))
        .unwrap();
        let hermes = analyze_profile_json(include_str!(
            "../../../tests/fixtures/hermes-cpu-profile.json"
        ))
        .unwrap();

        assert_eq!(baseline.commit_count, 3);
        assert_eq!(baseline.commits[1].changes[0].props, ["items"]);
        assert_eq!(current.commit_count, 8);
        assert_eq!(current.components[0].render_count, 8);
        assert!(current.findings.iter().any(|finding| {
            finding.rule_id == "repeated-render-without-change"
                && finding
                    .source
                    .as_ref()
                    .is_some_and(|source| source.line == Some(18))
        }));
        let diff = diff_profile_reports(&baseline, &current);
        assert!(diff.compatible);
        assert_eq!(diff.regression_count, 3);
        assert_eq!(hermes.profile_type, DiagnosticProfileType::HermesCpu);
        assert_eq!(hermes.functions[0].name, "renderProductList");
    }

    #[test]
    fn source_map_maps_hermes_bundle_location_to_original_source() {
        let profile = json!({
            "nodes": [{
                "id": 1,
                "callFrame": {
                    "functionName": "renderList",
                    "url": "index.bundle",
                    "lineNumber": 0,
                    "columnNumber": 0
                }
            }],
            "samples": [1],
            "timeDeltas": [1_000]
        })
        .to_string();
        let source_map = json!({
            "version": 3,
            "file": "index.bundle",
            "sources": ["src/List.tsx"],
            "names": [],
            "mappings": "AAAA"
        })
        .to_string();
        let mut report = analyze_profile_json(&profile).unwrap();
        let mapped = apply_source_map_json(&mut report, &source_map).unwrap();

        assert_eq!(mapped, 1);
        assert!(report.source_map_applied);
        assert_eq!(report.source_map_mapped_count, 1);
        assert_eq!(
            report.functions[0].source.as_ref().unwrap().file,
            "src/List.tsx"
        );
        assert_eq!(report.functions[0].source.as_ref().unwrap().line, Some(1));
        assert_eq!(report.findings[0].source, report.functions[0].source);
    }
}
