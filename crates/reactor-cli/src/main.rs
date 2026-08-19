use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use reactor_ai::{FlowAiProvider, FlowGenerationRequest, OpenAiCompatibleProvider};
use reactor_analysis::{
    CiReport, RegressionPolicy, analyze_pair, analyze_profile_json, apply_source_map_json,
    diff_profile_reports, render_ci_html, render_ci_junit,
};
use reactor_core::{compile_maestro, render_html_report};
use reactor_protocol::{
    Flow, FlowLock, FlowTrialEvidence, GenerationProvenance, NormalizedResult, Platform,
    validate_flow,
};
use reactor_runner::{
    AndroidLeakTestPlan, AndroidRunRequest, IosRunRequest, cancel_persisted_job,
    discover_android_devices, discover_ios_simulators, doctor, get_job, list_jobs, run_android,
    run_demo_suite, run_ios, trial_android, trial_ios_simulator, validate_product_tour_flow,
    verify_job_artifacts,
};
use reactor_toolchain::{ManagedToolsManifest, SetupOptions, setup};

#[derive(Debug, Parser)]
#[command(
    name = "reactor",
    version,
    about = "AI-driven mobile performance testing"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliPlatform {
    Android,
    Ios,
}

impl From<CliPlatform> for Platform {
    fn from(value: CliPlatform) -> Self {
        match value {
            CliPlatform::Android => Self::Android,
            CliPlatform::Ios => Self::Ios,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Prepare Reactor's pinned private Java, Maestro, ADB and collector toolchain.
    Setup {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value = "tools/managed-tools-v1.json")]
        manifest: PathBuf,
        #[arg(long)]
        offline: bool,
        #[arg(long)]
        proxy: Option<String>,
        #[arg(long, env = "REACTOR_MAESTRO_OVERRIDE")]
        maestro_override: Option<PathBuf>,
    },
    /// Check Reactor's private managed toolchain.
    Doctor {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// List Android targets and booted iOS Simulators visible to Reactor.
    Devices {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// List persisted Runner jobs for reconnecting from another process.
    Jobs {
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Read a job and only the events after the supplied stable cursor.
    Job {
        id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 0)]
        cursor: i64,
    },
    /// Idempotently cancel a detached Runner job and its child process group.
    CancelJob {
        id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Verify indexed artifact sizes and SHA-256 hashes for one job.
    VerifyArtifacts {
        id: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Generate a Flow from natural language with a configured AI provider.
    GenerateFlow {
        #[arg(long)]
        intent: String,
        #[arg(long)]
        app_id: String,
        #[arg(long, value_enum, default_value_t = CliPlatform::Android)]
        platform: CliPlatform,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value = "https://api.openai.com/v1/chat/completions")]
        endpoint: String,
        #[arg(long, default_value = "gpt-5-mini")]
        model: String,
    },
    /// Validate a Reactor Flow without executing it.
    ValidateFlow { input: PathBuf },
    /// Execute a Flow once on its target simulator/device and save locking evidence.
    TrialFlow {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Validate and freeze a Flow for deterministic measured execution.
    LockFlow {
        input: PathBuf,
        output: PathBuf,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        trial_report: Option<PathBuf>,
    },
    /// Compile a Flow lock to separate Maestro phase files.
    CompileFlow { input: PathBuf, output_dir: PathBuf },
    /// Run the no-device product tour with clearly marked synthetic results.
    Demo {
        flow_lock: PathBuf,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Execute a locked Flow on Android with managed Maestro and Flashlight.
    RunAndroid {
        flow_lock: PathBuf,
        #[arg(long)]
        framework: String,
        #[arg(long)]
        scenario: String,
        #[arg(long)]
        device: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 18_000)]
        duration_ms: u64,
        #[arg(long, default_value_t = 10)]
        iterations: u32,
        /// Also run the measured Flow repeatedly in one process and capture memory checkpoints.
        #[arg(long)]
        leak_cycles: Option<u32>,
    },
    /// Execute a locked Flow on an iOS Simulator with xctrace Time Profiler evidence.
    RunIos {
        flow_lock: PathBuf,
        #[arg(long)]
        framework: String,
        #[arg(long)]
        scenario: String,
        #[arg(long)]
        device: String,
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        #[arg(long, default_value_t = 5_000)]
        duration_ms: u64,
    },
    /// Build one offline evidence report from compatible normalized result files.
    Report {
        /// One or more `NormalizedResult` JSON files (each file may also contain an array).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Directory for results.json and report.html.
        #[arg(long)]
        output_dir: PathBuf,
        #[arg(long, default_value = "Reactor 跨框架性能对比")]
        title: String,
    },
    /// Compare normalized results and optional component profiles for CI.
    Ci {
        /// Baseline `NormalizedResult` JSON (one result or an array).
        #[arg(long)]
        baseline: PathBuf,
        /// Current `NormalizedResult` JSON (one result or an array).
        #[arg(long)]
        current: PathBuf,
        /// Directory for analysis.json, junit.xml and report.html.
        #[arg(long)]
        output_dir: PathBuf,
        /// Optional `RegressionPolicy` JSON; defaults to Reactor's versioned policy.
        #[arg(long)]
        policy: Option<PathBuf>,
        /// Optional baseline React Profiler JSON.
        #[arg(long, requires = "current_profile")]
        baseline_profile: Option<PathBuf>,
        /// Optional current React Profiler JSON.
        #[arg(long, requires = "baseline_profile")]
        current_profile: Option<PathBuf>,
        /// Optional Source Map v3 applied locally to both profiles.
        #[arg(long)]
        source_map: Option<PathBuf>,
    },
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Setup {
            workspace,
            manifest,
            offline,
            proxy,
            maestro_override,
        } => {
            let manifest: ManagedToolsManifest =
                serde_json::from_slice(&fs::read(workspace.join(manifest))?)?;
            let installed = setup(
                &workspace,
                &manifest,
                &SetupOptions {
                    offline,
                    proxy,
                    maestro_override,
                },
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&installed)?);
        }
        Command::Doctor { workspace } => {
            println!("{}", serde_json::to_string_pretty(&doctor(&workspace))?);
        }
        Command::Devices { workspace } => {
            let (android, ios) = tokio::join!(
                discover_android_devices(&workspace),
                discover_ios_simulators()
            );
            let mut devices = android?;
            devices.extend(ios?);
            println!("{}", serde_json::to_string_pretty(&devices)?);
        }
        Command::Jobs { workspace, limit } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&list_jobs(&workspace, limit)?)?
            );
        }
        Command::Job {
            id,
            workspace,
            cursor,
        } => {
            let (job, events) = get_job(&workspace, &id, cursor)?;
            let next_cursor = events.last().map_or(cursor, |event| event.id);
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "job": job,
                    "events": events,
                    "nextCursor": next_cursor,
                }))?
            );
        }
        Command::CancelJob { id, workspace } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&cancel_persisted_job(&workspace, &id)?)?
            );
        }
        Command::VerifyArtifacts { id, workspace } => {
            let issues = verify_job_artifacts(&workspace, &id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": issues.is_empty(),
                    "issues": issues,
                }))?
            );
        }
        Command::GenerateFlow {
            intent,
            app_id,
            platform,
            output,
            api_key,
            endpoint,
            model,
        } => {
            let request = FlowGenerationRequest {
                intent,
                app_id,
                platform: platform.into(),
                ui_tree: None,
                screenshot_artifact_ids: vec![],
            };
            let key = api_key
                .or_else(|| std::env::var("REACTOR_AI_API_KEY").ok())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Flow generation requires --api-key or REACTOR_AI_API_KEY; offline rule generation was removed",
                    )
                })?;
            let generated = OpenAiCompatibleProvider::new(endpoint, key, model)
                .generate(request)
                .await?;
            write_json(&output, &generated.flow)?;
            println!("{}", output.display());
        }
        Command::ValidateFlow { input } => {
            let flow: Flow = serde_json::from_slice(&fs::read(input)?)?;
            println!("{}", serde_json::to_string_pretty(&validate_flow(&flow)?)?);
        }
        Command::TrialFlow {
            input,
            output,
            device,
            workspace,
        } => {
            let flow: Flow = serde_json::from_slice(&fs::read(input)?)?;
            let evidence = if let Some(device) = device {
                match flow.platform {
                    Platform::Android => trial_android(&workspace, &flow, &device, None).await?,
                    Platform::Ios => trial_ios_simulator(&workspace, &flow, &device, None).await?,
                }
            } else {
                validate_product_tour_flow(&workspace, &flow).await?
            };
            write_json(&output, &evidence)?;
            println!("{}", output.display());
        }
        Command::LockFlow {
            input,
            output,
            provider,
            model,
            trial_report,
        } => {
            let flow: Flow = serde_json::from_slice(&fs::read(input)?)?;
            let generation = provider
                .zip(model)
                .map(|(provider, model)| GenerationProvenance {
                    provider,
                    model,
                    prompt_template_version: "reactor-flow-v1".to_owned(),
                });
            let trial = trial_report
                .map(fs::read)
                .transpose()?
                .map(|bytes| serde_json::from_slice::<FlowTrialEvidence>(&bytes))
                .transpose()?;
            let locked = FlowLock::new_with_trial(flow, generation, trial)?;
            write_json(&output, &locked)?;
            println!("{}", output.display());
        }
        Command::CompileFlow { input, output_dir } => {
            let lock: FlowLock = serde_json::from_slice(&fs::read(input)?)?;
            let compiled = compile_maestro(&lock.flow)?;
            fs::create_dir_all(&output_dir)?;
            fs::write(output_dir.join("setup.yaml"), compiled.setup)?;
            fs::write(output_dir.join("measured.yaml"), compiled.measured)?;
            fs::write(output_dir.join("teardown.yaml"), compiled.teardown)?;
            println!("{}", output_dir.display());
        }
        Command::Demo {
            flow_lock,
            workspace,
        } => {
            let flow: FlowLock = serde_json::from_slice(&fs::read(flow_lock)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&run_demo_suite(&workspace, &flow).await?)?
            );
        }
        Command::RunAndroid {
            flow_lock,
            framework,
            scenario,
            device,
            workspace,
            duration_ms,
            iterations,
            leak_cycles,
        } => {
            let result = run_android(&AndroidRunRequest {
                workspace,
                flow_lock,
                framework,
                scenario,
                device_id: device,
                duration_ms,
                iteration_count: iterations,
                leak_test: leak_cycles.map(|cycles| AndroidLeakTestPlan {
                    cycles,
                    checkpoint_every: 2,
                    warmup_cycles: 2,
                    stabilization_ms: 750,
                    cooldown_ms: 5_000,
                    threshold_mb_per_cycle: 0.25,
                }),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::RunIos {
            flow_lock,
            framework,
            scenario,
            device,
            workspace,
            duration_ms,
        } => {
            let result = run_ios(&IosRunRequest {
                workspace,
                flow_lock,
                framework,
                scenario,
                device_id: device,
                duration_ms,
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Report {
            inputs,
            output_dir,
            title,
        } => {
            let mut results = inputs
                .iter()
                .map(read_normalized_results)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            validate_report_inputs(&results)?;
            results.sort_by(|left, right| {
                left.scenario
                    .cmp(&right.scenario)
                    .then_with(|| left.framework.cmp(&right.framework))
            });
            fs::create_dir_all(&output_dir)?;
            write_json(&output_dir.join("results.json"), &results)?;
            fs::write(
                output_dir.join("report.html"),
                render_html_report(&title, &results),
            )?;
            println!("{}", output_dir.display());
        }
        Command::Ci {
            baseline,
            current,
            output_dir,
            policy,
            baseline_profile,
            current_profile,
            source_map,
        } => {
            let report = run_ci(
                &baseline,
                &current,
                &output_dir,
                policy.as_ref(),
                baseline_profile.as_ref(),
                current_profile.as_ref(),
                source_map.as_ref(),
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            std::process::exit(i32::from(report.exit_code));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_ci(
    baseline_path: &PathBuf,
    current_path: &PathBuf,
    output_dir: &PathBuf,
    policy_path: Option<&PathBuf>,
    baseline_profile_path: Option<&PathBuf>,
    current_profile_path: Option<&PathBuf>,
    source_map_path: Option<&PathBuf>,
) -> Result<CiReport, Box<dyn std::error::Error>> {
    let baseline_results = read_normalized_results(baseline_path)?;
    let current_results = read_normalized_results(current_path)?;
    if baseline_results.is_empty() || current_results.is_empty() {
        return Err("CI inputs must contain at least one normalized result".into());
    }
    let policy = policy_path.map_or_else(
        || Ok(RegressionPolicy::default()),
        |path| {
            serde_json::from_slice::<RegressionPolicy>(&fs::read(path)?)
                .map_err(Box::<dyn std::error::Error>::from)
        },
    )?;
    let analyses = current_results
        .iter()
        .map(|current| {
            let baseline = baseline_results
                .iter()
                .find(|candidate| candidate.framework == current.framework)
                .unwrap_or(&baseline_results[0]);
            analyze_pair(baseline, current, &policy)
        })
        .collect::<Vec<_>>();
    let profile_diff =
        read_profile_diff(baseline_profile_path, current_profile_path, source_map_path)?;
    let report = CiReport::new(analyses, profile_diff);
    fs::create_dir_all(output_dir)?;
    write_json(&output_dir.join("analysis.json"), &report)?;
    fs::write(output_dir.join("junit.xml"), render_ci_junit(&report))?;
    fs::write(output_dir.join("report.html"), render_ci_html(&report))?;
    Ok(report)
}

fn read_normalized_results(
    path: &PathBuf,
) -> Result<Vec<reactor_protocol::NormalizedResult>, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    if value.is_array() {
        Ok(serde_json::from_value(value)?)
    } else {
        Ok(vec![serde_json::from_value(value)?])
    }
}

fn validate_report_inputs(results: &[NormalizedResult]) -> Result<(), Box<dyn std::error::Error>> {
    let Some(first) = results.first() else {
        return Err("report requires at least one normalized result".into());
    };
    if first.device.id.is_none() {
        return Err("report inputs must identify the device target".into());
    }
    if first.device.physical.is_none() {
        return Err("report inputs must identify whether the target is physical".into());
    }
    if first.device.os_version.is_none() {
        return Err("report inputs must record the target OS version".into());
    }
    if first.app_id.is_none() || first.app_version.is_none() {
        return Err("report inputs must record the application id and version".into());
    }
    if !first.device.refresh_rate.is_finite() || first.device.refresh_rate <= 0.0 {
        return Err("report inputs must include a valid refresh rate".into());
    }
    for result in results.iter().skip(1) {
        if result.schema_version != first.schema_version {
            return Err("report inputs must use the same result schema version".into());
        }
        if result.platform != first.platform {
            return Err("report cannot mix Android and iOS results".into());
        }
        if result.device.physical != first.device.physical {
            return Err("report cannot mix simulator/emulator and physical-device results".into());
        }
        if result.device.id != first.device.id {
            return Err("report inputs must come from the same device target".into());
        }
        if result.device.os_version != first.device.os_version {
            return Err("report inputs must come from the same OS version".into());
        }
        if (result.device.refresh_rate - first.device.refresh_rate).abs() > f64::EPSILON {
            return Err("report inputs must use the same refresh rate".into());
        }
        if result.build_mode != first.build_mode {
            return Err("report inputs must use the same build mode".into());
        }
        if result.source.synthetic != first.source.synthetic {
            return Err("report cannot mix measured and synthetic results".into());
        }
        if result.app_id.is_none() || result.app_version.is_none() {
            return Err("report inputs must record the application id and version".into());
        }
        if result.adapter != first.adapter {
            return Err("report inputs must use the same measurement adapter".into());
        }
        validate_native_metric_compatibility(first, result)?;
    }
    Ok(())
}

fn validate_native_metric_compatibility(
    first: &NormalizedResult,
    result: &NormalizedResult,
) -> Result<(), Box<dyn std::error::Error>> {
    match (&first.android_native, &result.android_native) {
        (Some(left), Some(right))
            if left.definitions_version != right.definitions_version
                || left.collector != right.collector
                || left.trace_processor_version != right.trace_processor_version =>
        {
            return Err("report inputs use incompatible Android metric definitions".into());
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err("report cannot mix Android native evidence availability".into());
        }
        _ => {}
    }
    match (&first.ios_native, &result.ios_native) {
        (Some(left), Some(right))
            if left.definitions_version != right.definitions_version
                || left.collector != right.collector
                || left.xctrace_version != right.xctrace_version
                || left.template != right.template =>
        {
            return Err("report inputs use incompatible iOS metric definitions".into());
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err("report cannot mix iOS native evidence availability".into());
        }
        _ => {}
    }
    Ok(())
}

fn read_profile_diff(
    baseline_path: Option<&PathBuf>,
    current_path: Option<&PathBuf>,
    source_map_path: Option<&PathBuf>,
) -> Result<Option<reactor_analysis::ProfileDiffReport>, Box<dyn std::error::Error>> {
    let (Some(baseline_path), Some(current_path)) = (baseline_path, current_path) else {
        return Ok(None);
    };
    let mut baseline = analyze_profile_json(&fs::read_to_string(baseline_path)?)?;
    let mut current = analyze_profile_json(&fs::read_to_string(current_path)?)?;
    if let Some(source_map_path) = source_map_path {
        let source_map = fs::read_to_string(source_map_path)?;
        apply_source_map_json(&mut baseline, &source_map)?;
        apply_source_map_json(&mut current, &source_map)?;
    }
    Ok(Some(diff_profile_reports(&baseline, &current)))
}

fn write_json(
    path: &PathBuf,
    value: &impl serde::Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result() -> NormalizedResult {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1,
            "runId": "run-a",
            "createdAt": "2026-08-18T00:00:00Z",
            "framework": "react-native",
            "platform": "android",
            "scenario": "list",
            "adapter": "flashlight-android",
            "buildMode": "release",
            "flowHash": "flow-a",
            "appId": "com.reactor.fixture",
            "appVersion": "1.0 (1)",
            "device": {
                "id": "emulator-5554",
                "name": "Pixel",
                "osVersion": "16",
                "refreshRate": 60.0,
                "physical": false
            },
            "source": {
                "name": "fixture",
                "status": "SUCCESS",
                "rawFile": "flashlight.json",
                "synthetic": false
            },
            "androidNative": {
                "schemaVersion": 1,
                "definitionsVersion": "android-native-v1",
                "collector": "perfetto-frametimeline-v1",
                "traceProcessorVersion": "57.2",
                "perfettoTraceFile": "perfetto.pftrace",
                "frameCount": 1,
                "frameTimeMeanMs": 16.0,
                "frameTimeP50Ms": 16.0,
                "frameTimeP95Ms": 16.0,
                "frameTimeP99Ms": 16.0,
                "jankFrameCount": 0,
                "jankFramePct": 0.0,
                "overBudgetFramePct": 0.0,
                "startupTimeMs": 500.0,
                "memoryPssMb": 100.0,
                "thermalStatusBefore": 0,
                "thermalStatusAfter": 0,
                "warnings": []
            },
            "iterations": [],
            "summary": {
                "iterationCount": 0,
                "successfulIterationCount": 0,
                "fpsMean": null,
                "fpsP10": null,
                "lowFpsSamplePct": null,
                "ramMeanMb": null,
                "ramPeakMb": null,
                "cpuMeanPct": null,
                "uiCpuMeanPct": null,
                "jsCpuMeanPct": null
            },
            "warnings": []
        }))
        .expect("valid fixture")
    }

    fn validation_error(results: &[NormalizedResult]) -> String {
        validate_report_inputs(results)
            .expect_err("inputs should be incompatible")
            .to_string()
    }

    #[test]
    fn report_accepts_compatible_cross_framework_results() {
        let first = result();
        let mut second = first.clone();
        second.run_id = "run-b".into();
        second.framework = "flutter".into();
        second.flow_hash = "flow-b".into();
        assert!(validate_report_inputs(&[first, second]).is_ok());
    }

    #[test]
    fn report_rejects_mixed_platform_device_and_build_mode() {
        let first = result();
        let mut different = first.clone();
        different.platform = "ios".into();
        assert!(validation_error(&[first.clone(), different]).contains("Android and iOS"));

        let mut different = first.clone();
        different.device.physical = Some(true);
        assert!(validation_error(&[first.clone(), different]).contains("physical-device"));

        let mut different = first.clone();
        different.device.id = Some("emulator-5556".into());
        assert!(validation_error(&[first.clone(), different]).contains("same device"));

        let mut different = first.clone();
        different.build_mode = "debug".into();
        assert!(validation_error(&[first, different]).contains("same build mode"));
    }

    #[test]
    fn report_rejects_unknown_device_class_and_invalid_refresh_rate() {
        let mut unknown_class = result();
        unknown_class.device.physical = None;
        assert!(validation_error(&[unknown_class]).contains("whether the target is physical"));

        let mut invalid_rate = result();
        invalid_rate.device.refresh_rate = 0.0;
        assert!(validation_error(&[invalid_rate]).contains("valid refresh rate"));

        let mut missing_os = result();
        missing_os.device.os_version = None;
        assert!(validation_error(&[missing_os]).contains("OS version"));

        let mut missing_app = result();
        missing_app.app_version = None;
        assert!(validation_error(&[missing_app]).contains("application id and version"));
    }

    #[test]
    fn report_rejects_incompatible_metric_evidence() {
        let first = result();
        let mut different = first.clone();
        different.adapter = "other-adapter".into();
        assert!(validation_error(&[first.clone(), different]).contains("measurement adapter"));

        let mut different = first.clone();
        different
            .android_native
            .as_mut()
            .expect("Android metrics")
            .definitions_version = "android-native-v2".into();
        assert!(validation_error(&[first.clone(), different]).contains("metric definitions"));

        let mut missing = first.clone();
        missing.android_native = None;
        assert!(validation_error(&[first, missing]).contains("evidence availability"));
    }
}
