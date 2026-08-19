use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use reactor_protocol::Platform;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct RunPlanInput {
    pub platform: Platform,
    pub device_id: Option<String>,
    pub frameworks: Vec<String>,
    pub scenarios: Vec<String>,
    pub known_frameworks: BTreeSet<String>,
    pub scenario_durations_ms: BTreeMap<String, u64>,
    pub measured_iterations: u32,
    pub seed: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTask {
    pub framework: String,
    pub scenario: String,
    pub platform: Platform,
    pub device_id: Option<String>,
    pub duration_ms: u64,
    pub warmup_iterations: u32,
    pub measured_iterations: u32,
    pub order: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPlan {
    pub schema_version: u32,
    pub platform: Platform,
    pub device_id: Option<String>,
    pub seed: u32,
    pub tasks: Vec<RunTask>,
    pub id: String,
    pub hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanCore<'a> {
    schema_version: u32,
    platform: Platform,
    device_id: &'a Option<String>,
    seed: u32,
    tasks: &'a [RunTask],
}

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("unknown framework: {0}")]
    UnknownFramework(String),
    #[error("unknown scenario: {0}")]
    UnknownScenario(String),
    #[error("failed to serialize run plan: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Builds a seeded, shuffled and content-hashed execution plan.
///
/// # Errors
///
/// Returns an error for unknown frameworks/scenarios or failed serialization.
pub fn build_run_plan(input: RunPlanInput) -> Result<RunPlan, PlanError> {
    let mut tasks = Vec::new();
    for framework in &input.frameworks {
        if !input.known_frameworks.contains(framework) {
            return Err(PlanError::UnknownFramework(framework.clone()));
        }
        for scenario in &input.scenarios {
            let duration_ms = input
                .scenario_durations_ms
                .get(scenario)
                .copied()
                .ok_or_else(|| PlanError::UnknownScenario(scenario.clone()))?;
            tasks.push(RunTask {
                framework: framework.clone(),
                scenario: scenario.clone(),
                platform: input.platform,
                device_id: input.device_id.clone(),
                duration_ms,
                warmup_iterations: 1,
                measured_iterations: input.measured_iterations,
                order: 0,
            });
        }
    }

    shuffle(&mut tasks, input.seed);
    for (index, task) in tasks.iter_mut().enumerate() {
        task.order = u32::try_from(index + 1).unwrap_or(u32::MAX);
    }

    let core = PlanCore {
        schema_version: 1,
        platform: input.platform,
        device_id: &input.device_id,
        seed: input.seed,
        tasks: &tasks,
    };
    let hash = hex::encode(Sha256::digest(serde_json::to_vec(&core)?));
    let timestamp = input
        .created_at
        .to_rfc3339_opts(SecondsFormat::Millis, true)
        .replace(':', "-");
    let id = format!("{timestamp}_{}", &hash[..10]);

    Ok(RunPlan {
        schema_version: 1,
        platform: input.platform,
        device_id: input.device_id,
        seed: input.seed,
        tasks,
        id,
        hash,
    })
}

/// Mulberry32 + Fisher-Yates, matching the Node reference implementation.
fn shuffle<T>(items: &mut [T], seed: u32) {
    let mut random = Mulberry32(seed);
    for index in (1..items.len()).rev() {
        let width = u64::try_from(index + 1).expect("slice length fits in u64");
        let scaled = (u64::from(random.next_u32()) * width) >> 32;
        let swap = usize::try_from(scaled).expect("scaled index fits in usize");
        items.swap(index, swap);
    }
}

struct Mulberry32(u32);

impl Mulberry32 {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut result = self.0;
        result = (result ^ (result >> 15)).wrapping_mul(result | 1);
        result ^= result.wrapping_add((result ^ (result >> 7)).wrapping_mul(result | 61));
        result ^ (result >> 14)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(seed: u32) -> RunPlanInput {
        RunPlanInput {
            platform: Platform::Android,
            device_id: Some("device-1".to_owned()),
            frameworks: vec!["rn".to_owned(), "flutter".to_owned(), "lynx".to_owned()],
            scenarios: vec!["list".to_owned(), "update".to_owned()],
            known_frameworks: ["rn", "flutter", "lynx"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            scenario_durations_ms: [("list".to_owned(), 100), ("update".to_owned(), 200)]
                .into_iter()
                .collect(),
            measured_iterations: 10,
            seed,
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn deterministic_for_same_seed() {
        let first = build_run_plan(input(42)).unwrap();
        let second = build_run_plan(input(42)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.tasks.len(), 6);
    }

    #[test]
    fn different_seed_changes_order() {
        let first = build_run_plan(input(1)).unwrap();
        let second = build_run_plan(input(2)).unwrap();
        assert_ne!(first.tasks, second.tasks);
    }
}
