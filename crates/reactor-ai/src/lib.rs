//! AI drafts and repairs flows before measurement; it never executes measured steps.

use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

use async_trait::async_trait;
use reactor_analysis::{AnalysisReport, AnalysisVerdict};
use reactor_protocol::{Flow, Platform, validate_flow};
#[cfg(test)]
use reactor_protocol::{Selector, Step, SwipeDirection};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

/// Maximum number of model-assisted repairs allowed for one failed trial.
pub const MAX_FLOW_REPAIR_ATTEMPTS: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowGenerationRequest {
    pub intent: String,
    pub app_id: String,
    pub platform: Platform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_tree: Option<String>,
    #[serde(default)]
    pub screenshot_artifact_ids: Vec<String>,
    /// User-approved, credential-redacted source hints. These never prove an on-device state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowProbeRequest {
    pub goal: String,
    pub app_id: String,
    pub platform: Platform,
    pub ui_tree: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowRepairRequest {
    pub flow: Flow,
    pub failure: DryRunFailure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_tree: Option<String>,
    #[serde(default)]
    pub screenshot_artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowModificationRequest {
    pub flow: Flow,
    pub instruction: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowQuestionRequest {
    pub flow: Flow,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_tree: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowQuestionAnswer {
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowAssistantRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<Flow>,
    pub message: String,
    pub app_id: String,
    pub platform: Platform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowAssistantDecision {
    pub kind: String,
    pub answer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DryRunFailure {
    pub step_path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPreview {
    pub original_chars: usize,
    pub included_chars: usize,
    pub element_count: usize,
    pub redaction_count: usize,
    pub screenshot_count: usize,
    pub screenshot_bytes_uploaded: u64,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactedUiContext {
    pub ui_tree: String,
    pub preview: ContextPreview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowChange {
    pub path: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedFlow {
    pub flow: Flow,
    pub provider: String,
    pub model: String,
    pub prompt_template_version: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisExplanationRequest {
    pub report: AnalysisReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitedInsight {
    pub title: String,
    pub text: String,
    pub fact: bool,
    #[serde(default)]
    pub metric_refs: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisNextStep {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisExplanation {
    pub schema_version: u32,
    pub verdict: AnalysisVerdict,
    pub provider: String,
    pub model: String,
    pub prompt_template_version: String,
    pub summary: String,
    pub facts: Vec<CitedInsight>,
    pub hypotheses: Vec<CitedInsight>,
    pub next_steps: Vec<AnalysisNextStep>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelAnalysisOutput {
    summary: String,
    #[serde(default)]
    hypotheses: Vec<ModelHypothesis>,
    #[serde(default)]
    next_steps: Vec<AnalysisNextStep>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelHypothesis {
    title: String,
    text: String,
    metric_refs: Vec<String>,
    evidence_refs: Vec<String>,
}

#[derive(Debug, Error)]
pub enum AiProviderError {
    #[error("AI provider is unavailable: {0}")]
    Unavailable(String),
    #[error("AI provider rejected the request: {0}")]
    Rejected(String),
    #[error("AI response did not match Reactor Flow schema: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("system credential store is unavailable: {0}")]
    Unavailable(String),
}

pub trait CredentialStore: Send + Sync {
    /// Saves one provider credential outside Reactor's config, logs, and database.
    ///
    /// # Errors
    ///
    /// Returns an error when the system credential store rejects the operation.
    fn save(&self, provider: &str, value: &str) -> Result<(), CredentialError>;

    /// Loads one provider credential.
    ///
    /// # Errors
    ///
    /// Returns an error when the system credential store is unavailable.
    fn load(&self, provider: &str) -> Result<Option<String>, CredentialError>;

    /// Deletes one provider credential. Missing credentials are treated as success.
    ///
    /// # Errors
    ///
    /// Returns an error when the system credential store rejects the operation.
    fn delete(&self, provider: &str) -> Result<(), CredentialError>;
}

#[derive(Debug, Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    fn entry(provider: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new("com.reactor.performance.ai", provider)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }
}

impl CredentialStore for SystemCredentialStore {
    fn save(&self, provider: &str, value: &str) -> Result<(), CredentialError> {
        Self::entry(provider)?
            .set_password(value)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    fn load(&self, provider: &str) -> Result<Option<String>, CredentialError> {
        match Self::entry(provider)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }

    fn delete(&self, provider: &str) -> Result<(), CredentialError> {
        match Self::entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }
}

/// Produces a bounded upload preview and removes common credentials/contact values from an Android
/// or iOS accessibility tree. Screenshots remain local unless a separate future upload action is
/// explicitly approved; consequently their uploaded byte count is always zero here.
///
/// # Panics
///
/// Panics only if Reactor's compile-time constant regular expressions are edited into an invalid
/// form.
#[must_use]
pub fn redact_ui_tree(ui_tree: &str, screenshot_count: usize) -> RedactedUiContext {
    let attribute = Regex::new(r#"(?i)(text|content-desc|label|value)="([^"]*)""#)
        .expect("static attribute regex is valid");
    let compact_attribute = Regex::new(r#"(?i)\b(accessibilityText|text|value)=([^;,\r\n"]+)"#)
        .expect("static compact attribute regex is valid");
    let email = Regex::new(r"(?i)[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}")
        .expect("static email regex is valid");
    let long_number = Regex::new(r"\b\d{8,}\b").expect("static number regex is valid");
    let token = Regex::new(r"\b[A-Za-z0-9_-]{32,}\b").expect("static token regex is valid");
    let mut redaction_count = 0;
    let mut output = String::with_capacity(ui_tree.len());
    // UIAutomator commonly emits the entire hierarchy on one line. Split at each node so a
    // password field cannot cause unrelated sibling selectors to be treated as password values.
    let node_scoped_tree = ui_tree.replace("<node", "\n<node");
    for line in node_scoped_tree.lines() {
        let password = line.contains("password=\"true\"")
            || line.contains("secure=\"true\"")
            || line.contains("password=true")
            || line.contains("secure=true");
        let redacted = attribute.replace_all(line, |captures: &regex::Captures<'_>| {
            let name = captures.get(1).map_or("text", |value| value.as_str());
            let original = captures.get(2).map_or("", |value| value.as_str());
            let value = redact_sensitive_value(
                original,
                password
                    && !name.eq_ignore_ascii_case("content-desc")
                    && !name.eq_ignore_ascii_case("label"),
                &email,
                &long_number,
                &token,
                &mut redaction_count,
            );
            format!(r#"{name}="{value}""#)
        });
        let redacted =
            compact_attribute.replace_all(&redacted, |captures: &regex::Captures<'_>| {
                let name = captures
                    .get(1)
                    .map_or("accessibilityText", |value| value.as_str());
                let original = captures.get(2).map_or("", |value| value.as_str());
                let value = redact_sensitive_value(
                    original,
                    password && !name.eq_ignore_ascii_case("accessibilityText"),
                    &email,
                    &long_number,
                    &token,
                    &mut redaction_count,
                );
                format!("{name}={value}")
            });
        output.push_str(&redacted);
        output.push('\n');
    }
    let original_chars = ui_tree.chars().count();
    let ui_tree = truncate(&output, 20_000);
    let xml_elements = ui_tree.matches("<node").count();
    let element_count = if xml_elements > 0 {
        xml_elements
    } else if ui_tree.starts_with("element_num,") {
        ui_tree
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .count()
    } else {
        0
    };
    RedactedUiContext {
        preview: ContextPreview {
            original_chars,
            included_chars: ui_tree.chars().count(),
            element_count,
            redaction_count,
            screenshot_count,
            screenshot_bytes_uploaded: 0,
            fields: vec![
                "element class/resource-id".to_owned(),
                "redacted text/content description".to_owned(),
                "bounds and visibility".to_owned(),
            ],
        },
        ui_tree,
    }
}

fn redact_sensitive_value(
    original: &str,
    password: bool,
    email: &Regex,
    long_number: &Regex,
    token: &Regex,
    redaction_count: &mut usize,
) -> String {
    if password && !original.is_empty() {
        *redaction_count += 1;
        return "[REDACTED_PASSWORD]".to_owned();
    }
    let mut value = original.to_owned();
    for (pattern, replacement) in [
        (email, "[REDACTED_EMAIL]"),
        (long_number, "[REDACTED_NUMBER]"),
        (token, "[REDACTED_TOKEN]"),
    ] {
        let matches = pattern.find_iter(&value).count();
        if matches > 0 {
            *redaction_count += matches;
            value = pattern.replace_all(&value, replacement).into_owned();
        }
    }
    value
}

/// Returns a stable, field-level diff for explicit human confirmation before locking a repaired
/// Flow.
///
/// # Errors
///
/// Returns an error only if either schema-owned Flow cannot be serialized.
pub fn diff_flows(before: &Flow, after: &Flow) -> Result<Vec<FlowChange>, serde_json::Error> {
    let before = serde_json::to_value(before)?;
    let after = serde_json::to_value(after)?;
    let mut changes = Vec::new();
    diff_values("$", Some(&before), Some(&after), &mut changes);
    Ok(changes)
}

fn diff_values(
    path: &str,
    before: Option<&Value>,
    after: Option<&Value>,
    changes: &mut Vec<FlowChange>,
) {
    if before == after || changes.len() >= 200 {
        return;
    }
    match (before, after) {
        (Some(Value::Object(left)), Some(Value::Object(right))) => {
            let keys = left
                .keys()
                .chain(right.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                diff_values(
                    &format!("{path}.{key}"),
                    left.get(key),
                    right.get(key),
                    changes,
                );
            }
        }
        (Some(Value::Array(left)), Some(Value::Array(right))) => {
            for index in 0..left.len().max(right.len()) {
                diff_values(
                    &format!("{path}[{index}]"),
                    left.get(index),
                    right.get(index),
                    changes,
                );
            }
        }
        _ => changes.push(FlowChange {
            path: path.to_owned(),
            before: before.cloned(),
            after: after.cloned(),
        }),
    }
}

#[async_trait]
pub trait FlowAiProvider: Send + Sync {
    fn id(&self) -> &'static str;
    async fn generate(
        &self,
        request: FlowGenerationRequest,
    ) -> Result<GeneratedFlow, AiProviderError>;
    async fn probe(&self, _request: FlowProbeRequest) -> Result<GeneratedFlow, AiProviderError> {
        Err(AiProviderError::Unavailable(
            "this provider does not support interactive exploration probes".to_owned(),
        ))
    }
    async fn repair(&self, request: FlowRepairRequest) -> Result<GeneratedFlow, AiProviderError>;
    async fn modify(
        &self,
        _request: FlowModificationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        Err(AiProviderError::Unavailable(
            "this provider does not support natural-language Flow modification".to_owned(),
        ))
    }
    async fn answer_flow_question(
        &self,
        _request: FlowQuestionRequest,
    ) -> Result<FlowQuestionAnswer, AiProviderError> {
        Err(AiProviderError::Unavailable(
            "this provider does not support Flow questions".to_owned(),
        ))
    }
    async fn classify_flow_request(
        &self,
        _request: FlowAssistantRequest,
    ) -> Result<FlowAssistantDecision, AiProviderError> {
        Err(AiProviderError::Unavailable(
            "this provider does not support unified Flow assistance".to_owned(),
        ))
    }
}

#[async_trait]
pub trait AnalysisAiProvider: Send + Sync {
    async fn explain(
        &self,
        request: AnalysisExplanationRequest,
    ) -> Result<AnalysisExplanation, AiProviderError>;
}

/// A locally installed coding-agent CLI that Reactor may use before measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliProviderKind {
    Codex,
    ClaudeCode,
}

impl CliProviderKind {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex-cli",
            Self::ClaudeCode => "claude-code-cli",
        }
    }

    const fn command_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI",
            Self::ClaudeCode => "Claude Code CLI",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliProviderStatus {
    pub kind: CliProviderKind,
    pub label: String,
    pub available: bool,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub authenticated: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelStatus {
    pub available: bool,
    pub endpoint: String,
    pub models: Vec<String>,
    pub detail: String,
}

/// Checks an OpenAI-compatible local model server without sending prompts or credentials.
pub async fn doctor_local_model(endpoint: &str) -> LocalModelStatus {
    let endpoint_label = safe_provider_url(endpoint);
    let urls = match local_model_discovery_endpoints(endpoint) {
        Ok(urls) => urls,
        Err(error) => {
            return LocalModelStatus {
                available: false,
                endpoint: endpoint_label,
                models: vec![],
                detail: error.to_string(),
            };
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return LocalModelStatus {
                available: false,
                endpoint: endpoint_label,
                models: vec![],
                detail: error.to_string(),
            };
        }
    };
    let mut last_error = "本地模型服务没有响应".to_owned();
    for url in urls {
        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                let payload = match response.json::<Value>().await {
                    Ok(payload) => payload,
                    Err(error) => {
                        last_error = format!("模型列表不是有效 JSON: {error}");
                        continue;
                    }
                };
                let mut models = local_model_names(&payload);
                models.sort();
                models.dedup();
                return LocalModelStatus {
                    available: true,
                    endpoint: endpoint_label,
                    detail: if models.is_empty() {
                        "服务已连接，但尚未发现已加载模型".to_owned()
                    } else {
                        format!("服务已连接，发现 {} 个模型", models.len())
                    },
                    models,
                };
            }
            Ok(response) => {
                last_error = format!("模型列表返回 HTTP {}", response.status().as_u16());
            }
            Err(error) => last_error = error.to_string(),
        }
    }
    LocalModelStatus {
        available: false,
        endpoint: endpoint_label,
        models: vec![],
        detail: format!("未连接本地模型服务：{last_error}"),
    }
}

fn local_model_discovery_endpoints(endpoint: &str) -> Result<Vec<String>, AiProviderError> {
    let mut url = reqwest::Url::parse(endpoint.trim()).map_err(|error| {
        AiProviderError::Unavailable(format!("invalid local model URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AiProviderError::Unavailable(
            "local model URL must use http or https".to_owned(),
        ));
    }
    url.set_fragment(None);
    url.set_query(None);
    let path = url.path().trim_end_matches('/').to_owned();
    let base_path = path
        .strip_suffix("/responses")
        .or_else(|| path.strip_suffix("/chat/completions"))
        .or_else(|| path.strip_suffix("/models"))
        .unwrap_or(&path)
        .trim_end_matches("/v1");
    let mut openai = url.clone();
    openai.set_path(&format!("{base_path}/v1/models"));
    let mut ollama = url;
    ollama.set_path(&format!("{base_path}/api/tags"));
    Ok(vec![openai.to_string(), ollama.to_string()])
}

fn local_model_names(payload: &Value) -> Vec<String> {
    payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("id")
                .or_else(|| item.get("name"))
                .or_else(|| item.get("model"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// Finds and checks a local CLI without reading or returning any credential material.
pub async fn doctor_cli_provider(
    kind: CliProviderKind,
    executable_override: Option<&str>,
) -> CliProviderStatus {
    let Some(executable) = resolve_cli_executable(kind, executable_override) else {
        return CliProviderStatus {
            kind,
            label: kind.label().to_owned(),
            available: false,
            executable: None,
            version: None,
            authenticated: false,
            detail: "未检测到本机安装；Reactor 不会自动下载安装".to_owned(),
        };
    };
    let version = probe_cli(&executable, &["--version"]).await;
    let authenticated = match kind {
        CliProviderKind::Codex => probe_cli(&executable, &["login", "status"]).await.is_some(),
        CliProviderKind::ClaudeCode => probe_cli(&executable, &["auth", "status"])
            .await
            .and_then(|value| serde_json::from_str::<Value>(&value).ok())
            .and_then(|value| value.get("loggedIn").and_then(Value::as_bool))
            .unwrap_or(false),
    };
    let available = version.is_some();
    CliProviderStatus {
        kind,
        label: kind.label().to_owned(),
        available,
        executable: Some(executable.display().to_string()),
        version: version.map(|value| sanitize_diagnostic(&value)),
        authenticated,
        detail: if !available {
            "可执行文件无法启动".to_owned()
        } else if authenticated {
            "已就绪；复用本机登录状态，不读取凭据".to_owned()
        } else {
            "已安装，但尚未登录；请先在终端完成该 CLI 的登录".to_owned()
        },
    }
}

async fn probe_cli(executable: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        Command::new(executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn sanitize_diagnostic(value: &str) -> String {
    value
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect()
}

fn resolve_cli_executable(
    kind: CliProviderKind,
    executable_override: Option<&str>,
) -> Option<PathBuf> {
    if let Some(value) = executable_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let path = PathBuf::from(value);
        return path.is_file().then_some(path);
    }
    if let Some(path) = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(kind.command_name()))
            .find(|candidate| candidate.is_file())
    }) {
        return Some(path);
    }
    let mut candidates = match kind {
        CliProviderKind::Codex => vec![
            PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
        ],
        CliProviderKind::ClaudeCode => vec![
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ],
    };
    if let Some(home) = env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".local/bin")
                .join(kind.command_name()),
        );
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

const CLI_STDOUT_LIMIT: u64 = 1_048_576;
const CLI_STDERR_LIMIT: u64 = 262_144;
static CLI_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Safe, non-interactive adapter for an already-installed Codex or Claude Code CLI.
pub struct CliFlowProvider {
    kind: CliProviderKind,
    executable: PathBuf,
    model: Option<String>,
    timeout: Duration,
}

impl CliFlowProvider {
    /// Resolves an existing executable. Reactor never downloads a CLI.
    ///
    /// # Errors
    ///
    /// Returns unavailable when the selected CLI cannot be found.
    pub fn new(
        kind: CliProviderKind,
        executable_override: Option<&str>,
        model: Option<String>,
    ) -> Result<Self, AiProviderError> {
        let executable = resolve_cli_executable(kind, executable_override).ok_or_else(|| {
            AiProviderError::Unavailable(format!(
                "{} 未安装或路径无效；Reactor 不会自动下载安装",
                kind.label()
            ))
        })?;
        Ok(Self {
            kind,
            executable,
            model: model.filter(|value| !value.trim().is_empty()),
            timeout: Duration::from_secs(120),
        })
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn complete_with_system(
        &self,
        system: &str,
        instruction: String,
        prompt_template_version: &str,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let flow_value = self
            .structured_json(
                format!("{system}\n\n{instruction}"),
                flow_output_schema(),
                "flow",
            )
            .await?;
        let flow: Flow = serde_json::from_value(flow_value)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        validate_flow(&flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        Ok(GeneratedFlow {
            flow,
            provider: self.kind.id().to_owned(),
            model: self.model_name(),
            prompt_template_version: prompt_template_version.to_owned(),
            notes: vec![format!(
                "{} 本机非交互输出已通过 Reactor Flow 校验",
                self.kind.label()
            )],
        })
    }

    async fn complete(&self, instruction: String) -> Result<GeneratedFlow, AiProviderError> {
        self.complete_with_system(SYSTEM_PROMPT, instruction, "reactor-flow-v1")
            .await
    }

    fn model_name(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| "CLI 默认模型".to_owned())
    }

    async fn structured_json(
        &self,
        prompt: String,
        schema_value: Value,
        artifact_name: &str,
    ) -> Result<Value, AiProviderError> {
        let temp = cli_temp_directory(self.kind)?;
        tokio::fs::create_dir_all(&temp)
            .await
            .map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        let result = self
            .structured_json_in(&temp, prompt, schema_value, artifact_name)
            .await;
        let _ = tokio::fs::remove_dir_all(&temp).await;
        result
    }

    async fn structured_json_in(
        &self,
        temp: &Path,
        prompt: String,
        schema_value: Value,
        artifact_name: &str,
    ) -> Result<Value, AiProviderError> {
        let schema = serde_json::to_string(&schema_value)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let schema_path = temp.join(format!("reactor-{artifact_name}.schema.json"));
        tokio::fs::write(&schema_path, &schema)
            .await
            .map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        let output_path = temp.join(format!("reactor-{artifact_name}.output.json"));
        let command = self.build_command(temp, &schema, &schema_path, &output_path);
        let stdout = self.execute_command(command, &prompt).await?;
        match self.kind {
            CliProviderKind::Codex => {
                let text = tokio::fs::read_to_string(&output_path)
                    .await
                    .map_err(|error| {
                        AiProviderError::InvalidResponse(format!(
                            "Codex 未生成最终结构化结果: {error}"
                        ))
                    })?;
                serde_json::from_str::<Value>(strip_markdown_fence(&text))
                    .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))
            }
            CliProviderKind::ClaudeCode => extract_claude_structured_output(&stdout),
        }
    }

    fn build_command(
        &self,
        temp: &Path,
        schema: &str,
        schema_path: &Path,
        output_path: &Path,
    ) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .current_dir(temp)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.as_std_mut().process_group(0);
        match self.kind {
            CliProviderKind::Codex => {
                command.args([
                    "exec",
                    "--sandbox",
                    "read-only",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "--ignore-rules",
                    "--disable",
                    "plugins",
                    "--disable",
                    "apps",
                    "--disable",
                    "remote_plugin",
                    "--color",
                    "never",
                    "--output-schema",
                ]);
                command
                    .arg(schema_path)
                    .arg("--output-last-message")
                    .arg(output_path);
                if let Some(model) = &self.model {
                    command.arg("--model").arg(model);
                }
                command.arg("-");
            }
            CliProviderKind::ClaudeCode => {
                command.args([
                    "-p",
                    "--output-format",
                    "json",
                    "--json-schema",
                    schema,
                    "--tools",
                    "",
                    "--disallowedTools",
                    "mcp__*",
                    "--permission-mode",
                    "dontAsk",
                    "--no-session-persistence",
                    "--disable-slash-commands",
                ]);
                if let Some(model) = &self.model {
                    command.arg("--model").arg(model);
                }
            }
        }
        command
    }

    async fn execute_command(
        &self,
        mut command: Command,
        prompt: &str,
    ) -> Result<Vec<u8>, AiProviderError> {
        let mut child = command.spawn().map_err(|error| {
            AiProviderError::Unavailable(format!("无法启动 {}: {error}", self.kind.label()))
        })?;
        let pid = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AiProviderError::Unavailable("无法打开 CLI 输入".to_owned()))?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        drop(stdin);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AiProviderError::Unavailable("无法读取 CLI 输出".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AiProviderError::Unavailable("无法读取 CLI 错误输出".to_owned()))?;
        let execution = async {
            let status = child.wait();
            let read_stdout = async {
                let mut bytes = Vec::new();
                stdout
                    .take(CLI_STDOUT_LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                Ok::<_, std::io::Error>(bytes)
            };
            let read_stderr = async {
                let mut bytes = Vec::new();
                stderr
                    .take(CLI_STDERR_LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                Ok::<_, std::io::Error>(bytes)
            };
            tokio::join!(status, read_stdout, read_stderr)
        };
        let Ok((status, stdout, stderr)) = tokio::time::timeout(self.timeout, execution).await
        else {
            terminate_cli_process(&mut child, pid).await;
            return Err(AiProviderError::Unavailable(format!(
                "{} 调用超时，进程已终止",
                self.kind.label()
            )));
        };
        let status = status.map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        let stdout = stdout.map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        let stderr = stderr.map_err(|error| AiProviderError::Unavailable(error.to_string()))?;
        if stdout.len() as u64 > CLI_STDOUT_LIMIT || stderr.len() as u64 > CLI_STDERR_LIMIT {
            return Err(AiProviderError::InvalidResponse(
                "CLI 输出超过 Reactor 安全上限".to_owned(),
            ));
        }
        if !status.success() {
            let detail = sanitize_cli_error(&stderr);
            return Err(AiProviderError::Rejected(if detail.is_empty() {
                format!("{} 返回失败状态；请确认已登录", self.kind.label())
            } else {
                format!("{}: {detail}", self.kind.label())
            }));
        }
        Ok(stdout)
    }
}

fn extract_claude_structured_output(stdout: &[u8]) -> Result<Value, AiProviderError> {
    let payload: Value = serde_json::from_slice(stdout)
        .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
    if let Some(value) = payload.get("structured_output") {
        return Ok(value.clone());
    }
    if let Some(result) = payload.get("result").and_then(Value::as_str) {
        return serde_json::from_str(strip_markdown_fence(result))
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()));
    }
    if payload.get("schemaVersion").is_some() {
        return Ok(payload);
    }
    Err(AiProviderError::InvalidResponse(
        "Claude Code 输出中缺少 structured_output".to_owned(),
    ))
}

fn sanitize_cli_error(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).replace('\r', "\n");
    let lines = stderr
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            !lower.contains("api key")
                && !lower.contains("authorization")
                && !lower.contains("bearer ")
                && !lower.contains(" warn ")
                && !lower.contains("codex_core::skills::loader")
                && !lower.contains("codex_core::plugins::loader")
                && !lower.contains("unknown feature key in config")
        })
        .collect::<Vec<_>>();
    lines
        .into_iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

async fn terminate_cli_process(child: &mut tokio::process::Child, pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = tokio::time::timeout(Duration::from_millis(400), child.wait()).await;
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn cli_temp_directory(kind: CliProviderKind) -> Result<PathBuf, AiProviderError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| AiProviderError::Unavailable(error.to_string()))?
        .as_nanos();
    let sequence = CLI_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(env::temp_dir().join(format!(
        "reactor-{}-{}-{nonce}-{sequence}",
        kind.id(),
        std::process::id()
    )))
}

fn flow_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["schemaVersion", "id", "name", "appId", "platform", "intent", "setup", "measured", "teardown"],
        "properties": {
            "schemaVersion": { "type": "integer", "const": 1 },
            "id": { "type": "string", "minLength": 1 },
            "name": { "type": "string", "minLength": 1 },
            "appId": { "type": "string", "minLength": 1 },
            "platform": { "type": "string", "enum": ["android", "ios"] },
            "intent": { "type": ["string", "null"] },
            "setup": { "type": "array", "items": { "$ref": "#/$defs/step" } },
            "measured": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/step" } },
            "teardown": { "type": "array", "items": { "$ref": "#/$defs/step" } }
        },
        "$defs": {
            "selector": {
                "type": "object",
                "additionalProperties": false,
                "required": ["semanticId", "accessibilityId", "text", "index", "coordinate"],
                "properties": {
                    "semanticId": { "type": ["string", "null"] },
                    "accessibilityId": { "type": ["string", "null"] },
                    "text": { "type": ["string", "null"] },
                    "index": { "type": ["integer", "null"], "minimum": 0 },
                    "coordinate": {
                        "anyOf": [
                            { "type": "null" },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["x", "y"],
                                "properties": {
                                    "x": { "type": "number" },
                                    "y": { "type": "number" }
                                }
                            }
                        ]
                    }
                }
            },
            "inputValue": {
                "anyOf": [
                    { "type": "string", "minLength": 1, "maxLength": 4096 },
                    { "type": "object", "additionalProperties": false, "required": ["variableRef"], "properties": { "variableRef": { "type": "string", "minLength": 1, "maxLength": 128 } } },
                    { "type": "object", "additionalProperties": false, "required": ["secretRef"], "properties": { "secretRef": { "type": "string", "minLength": 1, "maxLength": 128 } } },
                    { "type": "object", "additionalProperties": false, "required": ["promptRef"], "properties": { "promptRef": { "type": "string", "minLength": 1, "maxLength": 128 } } },
                    { "type": "object", "additionalProperties": false, "required": ["totpRef"], "properties": { "totpRef": { "type": "string", "minLength": 1, "maxLength": 128 } } }
                ]
            },
            "step": {
                "anyOf": [
                    { "type": "object", "additionalProperties": false, "required": ["action"], "properties": { "action": { "type": "string", "const": "reset_app_state" } } },
                    { "type": "object", "additionalProperties": false, "required": ["action"], "properties": { "action": { "type": "string", "const": "launch_app" } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "target"], "properties": { "action": { "type": "string", "const": "tap" }, "target": { "$ref": "#/$defs/selector" } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "target", "value", "clearBefore"], "properties": { "action": { "type": "string", "const": "input_text" }, "target": { "$ref": "#/$defs/selector" }, "value": { "$ref": "#/$defs/inputValue" }, "clearBefore": { "type": "boolean" } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "direction", "duration_ms"], "properties": { "action": { "type": "string", "const": "swipe" }, "direction": { "type": "string", "enum": ["up", "down", "left", "right"] }, "duration_ms": { "type": "integer", "minimum": 0 } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "target", "timeout_ms"], "properties": { "action": { "type": "string", "const": "wait_for" }, "target": { "$ref": "#/$defs/selector" }, "timeout_ms": { "type": "integer", "minimum": 0 } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "target"], "properties": { "action": { "type": "string", "const": "assert_visible" }, "target": { "$ref": "#/$defs/selector" } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "duration_ms"], "properties": { "action": { "type": "string", "const": "pause" }, "duration_ms": { "type": "integer", "minimum": 0 } } },
                    { "type": "object", "additionalProperties": false, "required": ["action", "times", "steps"], "properties": { "action": { "type": "string", "const": "repeat" }, "times": { "type": "integer", "minimum": 1, "maximum": 100 }, "steps": { "type": "array", "items": { "$ref": "#/$defs/step" } } } }
                ]
            }
        }
    })
}

#[async_trait]
impl FlowAiProvider for CliFlowProvider {
    fn id(&self) -> &'static str {
        self.kind.id()
    }

    async fn generate(
        &self,
        request: FlowGenerationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let tree = truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000);
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        self.complete(format!(
            "Generate a Flow.\nIntent: {}\nApp id: {}\nPlatform: {:?}\nUI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}",
            request.intent, request.app_id, request.platform, tree, source_context
        ))
        .await
    }

    async fn probe(&self, request: FlowProbeRequest) -> Result<GeneratedFlow, AiProviderError> {
        let tree = truncate(&request.ui_tree, 20_000);
        self.complete_with_system(
            PROBE_SYSTEM_PROMPT,
            format!(
                "Choose the single safe entry action for this goal.\nGoal: {}\nApp id: {}\nPlatform: {:?}\nCurrent UI tree:\n{}",
                request.goal, request.app_id, request.platform, tree
            ),
            "reactor-probe-v1",
        )
        .await
    }

    async fn repair(&self, request: FlowRepairRequest) -> Result<GeneratedFlow, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let tree = truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000);
        self.complete(format!(
            "Repair this Flow without changing its performance intent.\nFailure at {}: {} ({})\nFlow:\n{}\nCurrent UI tree:\n{}",
            request.failure.step_path,
            request.failure.message,
            request.failure.code,
            flow,
            tree
        ))
        .await
    }

    async fn modify(
        &self,
        request: FlowModificationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let observed_selectors = observed_selector_inventory(request.ui_tree.as_deref());
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        self.complete_with_system(
            MODIFY_SYSTEM_PROMPT,
            format!(
                "Modify this Reactor Flow exactly as requested.\nUser instruction: {}\nTrial failure: {}\nObserved selector values (exact; never translate or change case): {}\nCurrent redacted UI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}\nCurrent Flow:\n{}",
                request.instruction,
                truncate(request.failure_context.as_deref().unwrap_or("not provided"), 4_000),
                observed_selectors,
                truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
                source_context,
                flow
            ),
            "reactor-flow-modify-v1",
        )
        .await
    }

    async fn answer_flow_question(
        &self,
        request: FlowQuestionRequest,
    ) -> Result<FlowQuestionAnswer, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let output = self
            .structured_json(
                format!(
                    "{}\n\nQuestion: {}\nCurrent redacted UI tree:\n{}\nCurrent Flow:\n{}",
                    FLOW_QUESTION_SYSTEM_PROMPT,
                    request.question,
                    truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
                    flow
                ),
                flow_question_output_schema(),
                "flow-question",
            )
            .await?;
        serde_json::from_value(output)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))
    }

    async fn classify_flow_request(
        &self,
        request: FlowAssistantRequest,
    ) -> Result<FlowAssistantDecision, AiProviderError> {
        let flow = request.flow.as_ref().map_or_else(
            || "not created".to_owned(),
            |flow| serde_json::to_string_pretty(flow).unwrap_or_else(|_| "invalid".to_owned()),
        );
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        let output = self
            .structured_json(
                format!(
                    "{}\n\nMessage: {}\nApp id: {}\nPlatform: {:?}\nCurrent redacted UI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}\nCurrent Flow:\n{}",
                    FLOW_ASSISTANT_SYSTEM_PROMPT,
                    request.message,
                    request.app_id,
                    request.platform,
                    truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
                    source_context,
                    flow
                ),
                flow_assistant_output_schema(),
                "flow-assistant",
            )
            .await?;
        serde_json::from_value(output)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))
    }
}

#[async_trait]
impl AnalysisAiProvider for CliFlowProvider {
    async fn explain(
        &self,
        request: AnalysisExplanationRequest,
    ) -> Result<AnalysisExplanation, AiProviderError> {
        let prompt = analysis_prompt(&request.report)?;
        let output = self
            .structured_json(prompt, analysis_output_schema(), "analysis")
            .await?;
        let model_output: ModelAnalysisOutput = serde_json::from_value(output)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        build_analysis_explanation(
            &request.report,
            model_output,
            self.kind.id().to_owned(),
            self.model_name(),
        )
    }
}

#[derive(Debug, Default)]
pub struct OfflineAnalysisExplainer;

#[async_trait]
impl AnalysisAiProvider for OfflineAnalysisExplainer {
    async fn explain(
        &self,
        request: AnalysisExplanationRequest,
    ) -> Result<AnalysisExplanation, AiProviderError> {
        let report = request.report;
        let facts = deterministic_facts(&report);
        let summary = match report.verdict {
            AnalysisVerdict::Regressed => "规则层检测到性能回归；以下事实均来自可追溯指标。",
            AnalysisVerdict::Improved => "规则层检测到性能改善；以下事实均来自可追溯指标。",
            AnalysisVerdict::Stable => "规则层未检测到超过阈值的性能回归。",
            AnalysisVerdict::Incompatible => "基线不兼容，Reactor 已拒绝给出性能回归结论。",
        }
        .to_owned();
        let next_steps = if report.verdict == AnalysisVerdict::Incompatible {
            vec![AnalysisNextStep {
                title: "重新建立兼容基线".to_owned(),
                text: "使用同一锁定 Flow、平台、设备配置、构建模式和指标定义重新运行。".to_owned(),
            }]
        } else {
            vec![AnalysisNextStep {
                title: "复测并定位".to_owned(),
                text: "先重复正式基准确认分布，再结合组件 Profile 和原始 trace 定位原因。"
                    .to_owned(),
            }]
        };
        Ok(AnalysisExplanation {
            schema_version: 1,
            verdict: report.verdict,
            provider: "reactor-rules".to_owned(),
            model: "deterministic-analysis-v1".to_owned(),
            prompt_template_version: "reactor-analysis-v1".to_owned(),
            summary,
            facts,
            hypotheses: vec![],
            next_steps,
        })
    }
}

fn analysis_prompt(report: &AnalysisReport) -> Result<String, AiProviderError> {
    let bounded = json!({
        "verdict": report.verdict,
        "compatibility": report.compatibility,
        "metrics": report.metrics,
        "ruleFindings": report.findings,
        "evidence": {
            "baselineRunId": report.evidence.baseline_run_id,
            "currentRunId": report.evidence.current_run_id,
            "flowHash": report.evidence.flow_hash,
            "framework": report.evidence.framework,
            "platform": report.evidence.platform,
            "scenario": report.evidence.scenario,
            "deviceClass": report.evidence.device_class,
            "metricDefinitions": report.evidence.metric_definitions,
            "rawEvidence": report.evidence.raw_evidence,
        }
    });
    let evidence = serde_json::to_string_pretty(&bounded)
        .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
    Ok(format!(
        "{ANALYSIS_SYSTEM_PROMPT}\n\nExplain this immutable Reactor rule report.\n{evidence}"
    ))
}

fn analysis_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "hypotheses", "nextSteps"],
        "properties": {
            "summary": { "type": "string", "minLength": 1 },
            "hypotheses": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "text", "metricRefs", "evidenceRefs"],
                    "properties": {
                        "title": { "type": "string", "minLength": 1 },
                        "text": { "type": "string", "minLength": 1 },
                        "metricRefs": { "type": "array", "minItems": 1, "items": { "type": "string" } },
                        "evidenceRefs": { "type": "array", "minItems": 1, "items": { "type": "string" } }
                    }
                }
            },
            "nextSteps": {
                "type": "array",
                "maxItems": 6,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "text"],
                    "properties": {
                        "title": { "type": "string", "minLength": 1 },
                        "text": { "type": "string", "minLength": 1 }
                    }
                }
            }
        }
    })
}

fn build_analysis_explanation(
    report: &AnalysisReport,
    output: ModelAnalysisOutput,
    provider: String,
    model: String,
) -> Result<AnalysisExplanation, AiProviderError> {
    let valid_metrics = report
        .metrics
        .iter()
        .map(|metric| metric.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let valid_evidence = report
        .metrics
        .iter()
        .flat_map(|metric| metric.evidence_refs.iter().map(String::as_str))
        .chain(
            report
                .findings
                .iter()
                .flat_map(|finding| finding.evidence_refs.iter().map(String::as_str)),
        )
        .collect::<std::collections::BTreeSet<_>>();
    let hypotheses = output
        .hypotheses
        .into_iter()
        .map(|hypothesis| {
            if hypothesis.metric_refs.is_empty()
                || hypothesis.evidence_refs.is_empty()
                || hypothesis
                    .metric_refs
                    .iter()
                    .any(|reference| !valid_metrics.contains(reference.as_str()))
                || hypothesis
                    .evidence_refs
                    .iter()
                    .any(|reference| !valid_evidence.contains(reference.as_str()))
            {
                return Err(AiProviderError::InvalidResponse(
                    "AI hypothesis contains a missing or unknown evidence reference".to_owned(),
                ));
            }
            Ok(CitedInsight {
                title: hypothesis.title,
                text: hypothesis.text,
                fact: false,
                metric_refs: hypothesis.metric_refs,
                evidence_refs: hypothesis.evidence_refs,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AnalysisExplanation {
        schema_version: 1,
        verdict: report.verdict,
        provider,
        model,
        prompt_template_version: "reactor-analysis-v1".to_owned(),
        summary: output.summary,
        facts: deterministic_facts(report),
        hypotheses,
        next_steps: output.next_steps,
    })
}

fn deterministic_facts(report: &AnalysisReport) -> Vec<CitedInsight> {
    report
        .findings
        .iter()
        .map(|finding| CitedInsight {
            title: finding.title.clone(),
            text: finding.summary.clone(),
            fact: true,
            metric_refs: finding.metric_refs.clone(),
            evidence_refs: finding.evidence_refs.clone(),
        })
        .collect()
}

/// Network provider supporting both the Responses and Chat Completions contracts.
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    model: String,
    provider_id: String,
    provider_label: String,
}

impl OpenAiCompatibleProvider {
    #[must_use]
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key: Some(api_key),
            model,
            provider_id: "openai-compatible".to_owned(),
            provider_label: "Cloud AI".to_owned(),
        }
    }

    #[must_use]
    pub fn new_local(endpoint: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key: None,
            model,
            provider_id: "local-model".to_owned(),
            provider_label: "Local Model".to_owned(),
        }
    }

    async fn complete_with_system(
        &self,
        system: &str,
        instruction: String,
        prompt_template_version: &str,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let (api, content) = self.complete_text(system, &instruction).await?;
        let flow: Flow = serde_json::from_str(strip_markdown_fence(&content))
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        validate_flow(&flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        Ok(GeneratedFlow {
            flow,
            provider: format!("{}-{}", self.provider_id, api.provider_id()),
            model: self.model.clone(),
            prompt_template_version: prompt_template_version.to_owned(),
            notes: vec![format!(
                "{} output passed Reactor Flow validation via {}",
                self.provider_label,
                api.label()
            )],
        })
    }

    async fn complete(&self, instruction: String) -> Result<GeneratedFlow, AiProviderError> {
        self.complete_with_system(SYSTEM_PROMPT, instruction, "reactor-flow-v1")
            .await
    }

    async fn complete_text(
        &self,
        system_prompt: &str,
        instruction: &str,
    ) -> Result<(OpenAiApi, String), AiProviderError> {
        let endpoints = provider_endpoints(&self.endpoint)?;
        for (index, endpoint) in endpoints.iter().enumerate() {
            match self.request(endpoint, system_prompt, instruction).await {
                Ok(payload) => {
                    let content = extract_model_text(endpoint.api, &payload)?;
                    return Ok((endpoint.api, content.to_owned()));
                }
                Err(EndpointAttemptError::Network(message)) => {
                    return Err(AiProviderError::Unavailable(format!(
                        "POST {}: {message}",
                        endpoint.safe_url
                    )));
                }
                Err(EndpointAttemptError::Rejected { status, message }) => {
                    let can_fallback = index + 1 < endpoints.len()
                        && should_try_compatible_endpoint(status, &message);
                    if !can_fallback {
                        return Err(AiProviderError::Rejected(format!(
                            "POST {} returned HTTP {status}: {message}",
                            endpoint.safe_url
                        )));
                    }
                }
            }
        }
        Err(AiProviderError::Unavailable(
            "no compatible provider endpoint was available".to_owned(),
        ))
    }

    async fn request(
        &self,
        endpoint: &ProviderEndpoint,
        system_prompt: &str,
        instruction: &str,
    ) -> Result<Value, EndpointAttemptError> {
        let body = match endpoint.api {
            OpenAiApi::Responses => json!({
                "model": self.model,
                "instructions": system_prompt,
                "input": instruction
            }),
            OpenAiApi::ChatCompletions => json!({
                "model": self.model,
                "messages": [
                    { "role": "system", "content": system_prompt },
                    { "role": "user", "content": instruction }
                ]
            }),
        };
        let mut request = self.client.post(&endpoint.url);
        if let Some(api_key) = &self.api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| EndpointAttemptError::Network(error.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| EndpointAttemptError::Network(error.to_string()))?;
        let payload = serde_json::from_str::<Value>(&text)
            .unwrap_or_else(|_| json!({ "error": { "message": truncate(&text, 500) } }));
        if !status.is_success() {
            return Err(EndpointAttemptError::Rejected {
                status: status.as_u16(),
                message: provider_error_message(&payload),
            });
        }
        Ok(payload)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiApi {
    Responses,
    ChatCompletions,
}

impl OpenAiApi {
    const fn label(self) -> &'static str {
        match self {
            Self::Responses => "Responses API",
            Self::ChatCompletions => "Chat Completions API",
        }
    }

    const fn provider_id(self) -> &'static str {
        match self {
            Self::Responses => "openai-compatible-responses",
            Self::ChatCompletions => "openai-compatible-chat-completions",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderEndpoint {
    url: String,
    safe_url: String,
    api: OpenAiApi,
}

#[derive(Debug)]
enum EndpointAttemptError {
    Network(String),
    Rejected { status: u16, message: String },
}

fn provider_endpoints(input: &str) -> Result<Vec<ProviderEndpoint>, AiProviderError> {
    let mut url = reqwest::Url::parse(input.trim()).map_err(|error| {
        AiProviderError::Unavailable(format!("invalid provider Base URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AiProviderError::Unavailable(
            "provider Base URL must use http or https".to_owned(),
        ));
    }
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    if path.ends_with("/responses") {
        return Ok(vec![provider_endpoint(&url, OpenAiApi::Responses)]);
    }
    if path.ends_with("/chat/completions") {
        return Ok(vec![provider_endpoint(&url, OpenAiApi::ChatCompletions)]);
    }

    let base = if path.is_empty() {
        "/v1".to_owned()
    } else if path.ends_with("/v1") {
        path.to_owned()
    } else {
        format!("{path}/v1")
    };
    let mut responses = url.clone();
    responses.set_path(&format!("{base}/responses"));
    let mut chat = url;
    chat.set_path(&format!("{base}/chat/completions"));
    Ok(vec![
        provider_endpoint(&responses, OpenAiApi::Responses),
        provider_endpoint(&chat, OpenAiApi::ChatCompletions),
    ])
}

fn provider_endpoint(url: &reqwest::Url, api: OpenAiApi) -> ProviderEndpoint {
    let mut safe = url.clone();
    safe.set_query(None);
    ProviderEndpoint {
        url: url.to_string(),
        safe_url: safe.to_string(),
        api,
    }
}

fn safe_provider_url(input: &str) -> String {
    reqwest::Url::parse(input.trim()).map_or_else(
        |_| input.trim().chars().take(160).collect(),
        |mut url| {
            url.set_fragment(None);
            url.set_query(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        },
    )
}

fn extract_model_text(api: OpenAiApi, payload: &Value) -> Result<&str, AiProviderError> {
    let content = match api {
        OpenAiApi::Responses => payload
            .get("output_text")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|output| output.get("content").and_then(Value::as_array))
                    .flatten()
                    .find_map(|content| content.get("text").and_then(Value::as_str))
            }),
        OpenAiApi::ChatCompletions => payload
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .or_else(|| payload.pointer("/choices/0/text").and_then(Value::as_str)),
    };
    content.ok_or_else(|| {
        AiProviderError::InvalidResponse(format!("missing model text in {} response", api.label()))
    })
}

fn provider_error_message(payload: &Value) -> String {
    payload
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .map_or_else(
            || "provider returned an error".to_owned(),
            |message| truncate(message, 500),
        )
}

fn should_try_compatible_endpoint(status: u16, message: &str) -> bool {
    if matches!(status, 404 | 405 | 501) {
        return true;
    }
    let message = message.to_ascii_lowercase();
    status == 400
        && [
            "invalid url",
            "unknown route",
            "unknown endpoint",
            "not supported",
        ]
        .iter()
        .any(|needle| message.contains(needle))
}

fn strip_markdown_fence(content: &str) -> &str {
    content
        .trim()
        .strip_prefix("```json")
        .or_else(|| content.trim().strip_prefix("```"))
        .and_then(|content| content.strip_suffix("```"))
        .map_or_else(|| content.trim(), str::trim)
}

#[async_trait]
impl FlowAiProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &'static str {
        "openai-compatible"
    }

    async fn generate(
        &self,
        request: FlowGenerationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let tree = truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000);
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        self.complete(format!(
            "Generate a Flow.\nIntent: {}\nApp id: {}\nPlatform: {:?}\nUI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}",
            request.intent, request.app_id, request.platform, tree, source_context
        ))
        .await
    }

    async fn probe(&self, request: FlowProbeRequest) -> Result<GeneratedFlow, AiProviderError> {
        let tree = truncate(&request.ui_tree, 20_000);
        self.complete_with_system(
            PROBE_SYSTEM_PROMPT,
            format!(
                "Choose the single safe entry action for this goal.\nGoal: {}\nApp id: {}\nPlatform: {:?}\nCurrent UI tree:\n{}",
                request.goal, request.app_id, request.platform, tree
            ),
            "reactor-probe-v1",
        )
        .await
    }

    async fn repair(&self, request: FlowRepairRequest) -> Result<GeneratedFlow, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let tree = truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000);
        self.complete(format!(
            "Repair this Flow without changing its performance intent.\nFailure at {}: {} ({})\nFlow:\n{}\nCurrent UI tree:\n{}",
            request.failure.step_path,
            request.failure.message,
            request.failure.code,
            flow,
            tree
        ))
        .await
    }

    async fn modify(
        &self,
        request: FlowModificationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let observed_selectors = observed_selector_inventory(request.ui_tree.as_deref());
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        self.complete_with_system(
            MODIFY_SYSTEM_PROMPT,
            format!(
                "Modify this Reactor Flow exactly as requested.\nUser instruction: {}\nTrial failure: {}\nObserved selector values (exact; never translate or change case): {}\nCurrent redacted UI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}\nCurrent Flow:\n{}",
                request.instruction,
                truncate(request.failure_context.as_deref().unwrap_or("not provided"), 4_000),
                observed_selectors,
                truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
                source_context,
                flow
            ),
            "reactor-flow-modify-v1",
        )
        .await
    }

    async fn answer_flow_question(
        &self,
        request: FlowQuestionRequest,
    ) -> Result<FlowQuestionAnswer, AiProviderError> {
        let flow = serde_json::to_string_pretty(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        let prompt = format!(
            "Question: {}\nCurrent redacted UI tree:\n{}\nCurrent Flow:\n{}",
            request.question,
            truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
            flow
        );
        let (_, content) = self
            .complete_text(FLOW_QUESTION_SYSTEM_PROMPT, &prompt)
            .await?;
        serde_json::from_str(strip_markdown_fence(&content))
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))
    }

    async fn classify_flow_request(
        &self,
        request: FlowAssistantRequest,
    ) -> Result<FlowAssistantDecision, AiProviderError> {
        let flow = request.flow.as_ref().map_or_else(
            || "not created".to_owned(),
            |flow| serde_json::to_string_pretty(flow).unwrap_or_else(|_| "invalid".to_owned()),
        );
        let source_context = truncate(
            request.source_context.as_deref().unwrap_or("not provided"),
            48_000,
        );
        let prompt = format!(
            "Message: {}\nApp id: {}\nPlatform: {:?}\nCurrent redacted UI tree:\n{}\nProject source context (hints only; never credentials or UI proof):\n{}\nCurrent Flow:\n{}",
            request.message,
            request.app_id,
            request.platform,
            truncate(request.ui_tree.as_deref().unwrap_or("not provided"), 20_000),
            source_context,
            flow
        );
        let (_, content) = self
            .complete_text(FLOW_ASSISTANT_SYSTEM_PROMPT, &prompt)
            .await?;
        serde_json::from_str(strip_markdown_fence(&content))
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))
    }
}

#[async_trait]
impl AnalysisAiProvider for OpenAiCompatibleProvider {
    async fn explain(
        &self,
        request: AnalysisExplanationRequest,
    ) -> Result<AnalysisExplanation, AiProviderError> {
        let prompt = analysis_prompt(&request.report)?;
        let (_, content) = self.complete_text(ANALYSIS_SYSTEM_PROMPT, &prompt).await?;
        let model_output: ModelAnalysisOutput =
            serde_json::from_str(strip_markdown_fence(&content))
                .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        build_analysis_explanation(
            &request.report,
            model_output,
            self.provider_id.clone(),
            self.model.clone(),
        )
    }
}

/// Test-only deterministic composer for schema and repair fixtures.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct OfflineFlowComposer;

#[cfg(test)]
#[async_trait]
impl FlowAiProvider for OfflineFlowComposer {
    fn id(&self) -> &'static str {
        "offline-composer"
    }

    async fn generate(
        &self,
        request: FlowGenerationRequest,
    ) -> Result<GeneratedFlow, AiProviderError> {
        let intent = request.intent.to_lowercase();
        let mut measured = vec![Step::LaunchApp];
        let (id, entry, ready, complete) =
            if contains_any(&intent, &["list", "scroll", "列表", "滚动"]) {
                ("list", Some("List scenario"), Some("List ready"), None)
            } else if contains_any(&intent, &["update", "刷新", "更新"]) {
                (
                    "update",
                    Some("Update scenario"),
                    Some("Update ready"),
                    Some("Update complete"),
                )
            } else if contains_any(&intent, &["animation", "动画"]) {
                (
                    "animation",
                    Some("Animation scenario"),
                    Some("Animation ready"),
                    Some("Animation complete"),
                )
            } else {
                ("startup", None, Some("Reactor ready"), None)
            };

        if let Some(entry) = entry {
            measured.push(wait_for("Reactor ready", 10_000));
            measured.push(Step::Tap {
                target: text_selector(entry),
            });
        }
        if let Some(ready) = ready {
            measured.push(wait_for(ready, 10_000));
        }
        if id == "list" {
            measured.push(Step::Repeat {
                times: extract_repeat(&intent).unwrap_or(8).clamp(1, 30),
                steps: vec![Step::Swipe {
                    direction: SwipeDirection::Up,
                    duration_ms: 800,
                }],
            });
        }
        if let Some(complete) = complete {
            measured.push(wait_for(complete, 12_000));
        }
        let flow = Flow {
            schema_version: 1,
            id: format!("{id}-generated"),
            name: format!("{} performance flow", title(id)),
            app_id: request.app_id,
            platform: request.platform,
            intent: Some(request.intent),
            setup: vec![Step::ResetAppState],
            measured,
            teardown: vec![],
        };
        validate_flow(&flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        Ok(GeneratedFlow {
            flow,
            provider: "reactor".to_owned(),
            model: "offline-intent-composer-v1".to_owned(),
            prompt_template_version: "reactor-flow-v1".to_owned(),
            notes: vec![
                "Offline composer used; configure an AI provider for arbitrary application flows"
                    .to_owned(),
            ],
        })
    }

    async fn repair(&self, request: FlowRepairRequest) -> Result<GeneratedFlow, AiProviderError> {
        validate_flow(&request.flow)
            .map_err(|error| AiProviderError::InvalidResponse(error.to_string()))?;
        Ok(GeneratedFlow {
            flow: request.flow,
            provider: "reactor".to_owned(),
            model: "offline-intent-composer-v1".to_owned(),
            prompt_template_version: "reactor-flow-v1".to_owned(),
            notes: vec![format!(
                "Offline composer cannot repair {}; configure an AI provider",
                request.failure.step_path
            )],
        })
    }
}

#[cfg(test)]
fn wait_for(text: &str, timeout_ms: u64) -> Step {
    Step::WaitFor {
        target: text_selector(text),
        timeout_ms,
    }
}

#[cfg(test)]
fn text_selector(text: &str) -> Selector {
    Selector {
        text: Some(text.to_owned()),
        ..Selector::default()
    }
}

#[cfg(test)]
fn contains_any(value: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| value.contains(candidate))
}

#[cfg(test)]
fn extract_repeat(value: &str) -> Option<u32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|part| (!part.is_empty()).then(|| part.parse().ok()).flatten())
}

#[cfg(test)]
fn title(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn observed_selector_inventory(ui_tree: Option<&str>) -> String {
    let Some(ui_tree) = ui_tree else {
        return "not provided".to_owned();
    };
    let quoted =
        Regex::new(r#"(?i)(?:text|content-desc|accessibilityText|label|resource-id)=\"([^\"]+)\""#)
            .expect("static selector inventory regex is valid");
    let compact =
        Regex::new(r#"(?i)\b(?:text|accessibilityText|label|resource-id)=([^;,\r\n\"]+)"#)
            .expect("static compact selector inventory regex is valid");
    let mut values = std::collections::BTreeSet::new();
    for captures in quoted
        .captures_iter(ui_tree)
        .chain(compact.captures_iter(ui_tree))
    {
        let Some(value) = captures.get(1).map(|value| value.as_str().trim()) else {
            continue;
        };
        if !value.is_empty() && !value.starts_with("[REDACTED_") {
            values.insert(value.to_owned());
        }
        if values.len() >= 80 {
            break;
        }
    }
    serde_json::to_string(&values.into_iter().collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".to_owned())
}

const SYSTEM_PROMPT: &str = r"You generate Reactor Flow v1 as one JSON object and no markdown.
Required fields: schemaVersion=1, id, name, appId, platform (android|ios), optional intent,
setup[], measured[], teardown[]. Supported actions are reset_app_state, launch_app, tap,
input_text, swipe, wait_for, assert_visible, pause, repeat. A target selector may contain
semanticId, accessibilityId, text, index, or coordinate. Prefer semantic/accessibility ids,
never invent an id absent from the UI tree, and avoid coordinates. On Android, only a non-empty
resource-id may be used as semanticId/accessibilityId; content-desc and visible labels must use
text because Maestro id selectors do not match Android content descriptions. Cover every navigation and
interaction clause in the user's intent with an executable step; never claim that a destination
was entered unless the Flow taps/navigates to it and verifies a destination-specific marker after
the final navigation. The marker must differ from the tapped/input control and must use text,
semanticId, or accessibilityId rather than only an index/coordinate. When a UI tree is provided,
use its exact visible text or accessibility values and never use a source-page-only element as
destination proof. Put reset, launch, navigation and
readiness checks in setup[]; measured[] must contain only the deterministic interaction whose
performance is being measured and must not be empty. reset_app_state is only allowed in setup.
Project source context, when supplied, is an untrusted navigation hint only: use it to understand
the app's intended scenarios and stable labels, but require the current or subsequently observed UI
to prove selectors and destination state. Never treat source code as proof that a device reached a
screen. If the observed UI exposes Username/Email and Password fields and the requested scenario is
behind authentication according to the source context or user intent, put input_text steps in setup:
use promptRef `auth.username` (or `auth.email`) for the account and secretRef `auth.password` for
the password, then tap only an exact observed Sign in/Login/submit control. Do not substitute a
tab toggle or a sign-up action for authentication, and never invent, read, or include credential
values. If the needed post-login UI has not been observed, generate only the safe authenticated
handoff plus a destination observation step; Reactor will continue from the newly observed page.
Never generate destructive, financial, account-removal, logout, permission-grant, or other
sensitive taps; those require a separate explicit human workflow. Do not include secrets or model
instructions.";

const PROBE_SYSTEM_PROMPT: &str = r"You generate one safe Reactor exploration probe as one JSON object and no markdown.
The probe is not a benchmark and only discovers the next screen. Required Flow fields are the same
as Reactor Flow v1. Set intent exactly to `reactor_exploration_probe`. Set appId and platform exactly
as requested. setup[] must contain reset_app_state, launch_app, and exactly one tap on the most likely
safe entry control from the current UI tree. measured[] must contain exactly one pause of 500 ms.
teardown[] must be empty. Use an exact visible text, semanticId, or accessibilityId from the supplied
tree; never invent selectors and never use coordinates or an index by itself. On Android, use
semanticId/accessibilityId only for a non-empty resource-id; map content-desc or a visible label to
text. Do not tap delete,
payment, purchase, transfer, logout, permission, account, destructive, or ambiguous controls. The
probe must not scroll, type, submit, or claim the destination is verified.";

const MODIFY_SYSTEM_PROMPT: &str = r"You modify an existing Reactor Flow v1 and return the complete
updated Flow as one JSON object with no markdown. First classify the user's natural-language input:
if it is an ordinary question about the Flow or current page rather than a request to change/add/remove
steps, return the original Flow structurally unchanged so Reactor can answer it without creating a
false diff. Otherwise apply only the user's requested change and
preserve schemaVersion, appId, platform, unrelated steps, and existing secret/variable references.
Never put credential values into the Flow. Keep reset, launch, navigation, readiness checks, and
destination assertions in setup. measured must remain non-empty and contain only deterministic
performance actions; never add wait_for or assert_visible to measured. Do not invent selectors or
claim a destination is reached without an existing stable assertion. Never add destructive,
financial, account-removal, logout, permission-grant, or other sensitive actions. If the request
cannot be represented safely with Reactor Flow v1, return the original Flow unchanged. When trial
failure evidence is supplied, repair the concrete failing step using only selectors present in the
redacted UI tree. Selector text is case-sensitive and language-sensitive: copy an exact observed
value and never translate it from the user's instruction. When a failed wait/assert selector is
absent from the observed selector inventory, replace only that failing step's target with the
closest exact observed value for the same control; do not delete, insert, or reorder other steps,
and do not modify selectors for pages that have not been observed yet. A missing promptRef runtime value is not a Flow defect: preserve the promptRef so
Reactor can request its one-time value before replay.";

const FLOW_QUESTION_SYSTEM_PROMPT: &str = r"You answer a question about the supplied Reactor Flow and
current redacted UI context. Return one JSON object with exactly one string field named answer and no
markdown. Explain what the current Flow does, why a step or selector exists, or what a safe next action
would be. Do not claim to have changed the Flow. Do not reveal or infer secrets, credentials, editable
field values, or information absent from the supplied context. Keep the answer concise and concrete.";

fn flow_question_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["answer"],
        "properties": {
            "answer": { "type": "string", "minLength": 1, "maxLength": 4000 }
        }
    })
}

const FLOW_ASSISTANT_SYSTEM_PROMPT: &str = r"Classify one natural-language message in Reactor Flow
Explorer. Return JSON only. kind must be question when the user is asking for an explanation,
recommendation, capability, or information without requesting Flow steps to be created, changed,
added, removed, or reordered. For question, answer directly and do not claim any Flow change. kind
must be change when the user asks to create a Flow or change/add/remove/reorder its steps. For change,
answer must be a short description of the requested change. Never reveal or infer secrets.";

fn flow_assistant_output_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["kind", "answer"],
        "properties": {
            "kind": { "type": "string", "enum": ["question", "change"] },
            "answer": { "type": "string", "minLength": 1, "maxLength": 4000 }
        }
    })
}

const ANALYSIS_SYSTEM_PROMPT: &str = r"You explain an immutable Reactor performance report as JSON only.
The verdict, compatibility result, metric values, thresholds, and rule findings are facts and must
never be changed or contradicted. Put possible causes only in hypotheses. Every hypothesis must cite
existing metricRefs and evidenceRefs exactly as provided. Do not invent measurements, component names,
source locations, traces, or causal claims. Keep facts and hypotheses clearly separate. Suggest concrete
verification steps. Do not include secrets, markdown, or a replacement verdict.";

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read as _, Write as _};

    fn one_shot_http_server(body: String) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![0_u8; 32 * 1024];
            let size = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..size]).into_owned())
                .unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    #[cfg(unix)]
    fn fake_cli(name: &str, body: &str) -> PathBuf {
        let directory = cli_temp_directory(CliProviderKind::Codex).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn valid_flow_json() -> &'static str {
        r#"{"schemaVersion":1,"id":"cli-flow","name":"CLI flow","appId":"com.example.app","platform":"android","intent":"launch","setup":[{"action":"reset_app_state"}],"measured":[{"action":"launch_app"}],"teardown":[]}"#
    }

    fn generation_request() -> FlowGenerationRequest {
        FlowGenerationRequest {
            intent: "launch".to_owned(),
            app_id: "com.example.app".to_owned(),
            platform: Platform::Android,
            ui_tree: None,
            screenshot_artifact_ids: vec![],
            source_context: None,
        }
    }

    #[test]
    fn source_context_is_optional_and_the_system_prompt_keeps_it_as_a_hint() {
        let request = FlowGenerationRequest {
            source_context: Some("--- App.tsx ---\nMemory scenario".to_owned()),
            ..generation_request()
        };
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(serialized.contains("sourceContext"));
        assert!(serialized.contains("Memory scenario"));
        assert!(SYSTEM_PROMPT.contains("untrusted navigation hint only"));
        assert!(SYSTEM_PROMPT.contains("secretRef `auth.password`"));
        assert!(SYSTEM_PROMPT.contains("never invent, read, or include credential\nvalues"));
    }

    fn analysis_report() -> AnalysisReport {
        serde_json::from_value(json!({
            "schemaVersion": 1,
            "verdict": "regressed",
            "compatibility": { "compatible": true, "reasons": [], "warnings": [] },
            "metrics": [{
                "id": "frame_time_p95_ms",
                "label": "P95 帧耗时",
                "unit": "ms",
                "direction": "lower_is_better",
                "baseline": 18.0,
                "current": 24.0,
                "absoluteDelta": 6.0,
                "percentDelta": 33.333,
                "thresholdPct": 10.0,
                "verdict": "regressed",
                "evidenceRefs": ["baseline.metrics.frame_time_p95_ms", "current.metrics.frame_time_p95_ms"]
            }],
            "findings": [{
                "id": "regression-frame_time_p95_ms",
                "severity": "warning",
                "title": "P95 帧耗时发生回归",
                "summary": "P95 increased beyond threshold.",
                "fact": true,
                "metricRefs": ["frame_time_p95_ms"],
                "evidenceRefs": ["baseline.metrics.frame_time_p95_ms", "current.metrics.frame_time_p95_ms"]
            }],
            "evidence": {
                "schemaVersion": 1,
                "baselineRunId": "baseline",
                "currentRunId": "current",
                "flowHash": "same-flow",
                "framework": "react-native",
                "platform": "android",
                "scenario": "list",
                "deviceClass": "simulator",
                "metricDefinitions": ["android-native-v1"],
                "rawEvidence": ["trace.perfetto"],
                "normalizedFacts": {}
            }
        }))
        .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_cli_provider_reads_validated_last_message() {
        let body = format!(
            r#"
if [ "${{1:-}}" = "--version" ]; then echo "codex-cli test"; exit 0; fi
if [ "${{1:-}}" = "login" ]; then exit 0; fi
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output-last-message" ]; then shift; output="$1"; fi
  shift || true
done
cat >/dev/null
printf '%s' '{}' > "$output""#,
            valid_flow_json()
        );
        let path = fake_cli("codex", &body);
        let generated = CliFlowProvider::new(
            CliProviderKind::Codex,
            path.to_str(),
            Some("test-model".to_owned()),
        )
        .unwrap()
        .generate(generation_request())
        .await
        .unwrap();
        assert_eq!(generated.provider, "codex-cli");
        assert_eq!(generated.flow.id, "cli-flow");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_cli_provider_reads_structured_output() {
        let payload = json!({ "structured_output": serde_json::from_str::<Value>(valid_flow_json()).unwrap() });
        let body = format!(
            r#"
if [ "${{1:-}}" = "--version" ]; then echo "claude test"; exit 0; fi
if [ "${{1:-}}" = "auth" ]; then echo '{{"loggedIn":true}}'; exit 0; fi
cat >/dev/null
printf '%s' '{payload}'"#
        );
        let path = fake_cli("claude", &body);
        let generated = CliFlowProvider::new(CliProviderKind::ClaudeCode, path.to_str(), None)
            .unwrap()
            .generate(generation_request())
            .await
            .unwrap();
        assert_eq!(generated.provider, "claude-code-cli");
        assert_eq!(generated.flow.id, "cli-flow");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_doctor_reports_installation_and_auth_without_credentials() {
        let path = fake_cli(
            "claude",
            r#"
if [ "${1:-}" = "--version" ]; then echo "claude 9.9"; exit 0; fi
if [ "${1:-}" = "auth" ]; then echo '{"loggedIn":true,"token":"must-not-leak"}'; exit 0; fi
exit 1"#,
        );
        let status = doctor_cli_provider(CliProviderKind::ClaudeCode, path.to_str()).await;
        assert!(status.available);
        assert!(status.authenticated);
        assert!(!status.detail.contains("must-not-leak"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_provider_rejects_invalid_json_and_times_out() {
        let invalid = fake_cli(
            "claude-invalid",
            "cat >/dev/null\nprintf '%s' '{\"result\":\"not-json\"}'",
        );
        let error = CliFlowProvider::new(CliProviderKind::ClaudeCode, invalid.to_str(), None)
            .unwrap()
            .generate(generation_request())
            .await
            .unwrap_err();
        assert!(
            matches!(error, AiProviderError::InvalidResponse(_)),
            "unexpected error: {error:?}"
        );
        let _ = std::fs::remove_dir_all(invalid.parent().unwrap());

        let slow = fake_cli("claude-slow", "cat >/dev/null\nsleep 5");
        let error = CliFlowProvider::new(CliProviderKind::ClaudeCode, slow.to_str(), None)
            .unwrap()
            .with_timeout(Duration::from_millis(50))
            .generate(generation_request())
            .await
            .unwrap_err();
        assert!(matches!(error, AiProviderError::Unavailable(_)));
        let _ = std::fs::remove_dir_all(slow.parent().unwrap());
    }

    #[test]
    fn explicit_missing_cli_path_does_not_fall_back_to_another_installation() {
        assert!(
            CliFlowProvider::new(
                CliProviderKind::Codex,
                Some("/definitely/missing/reactor-codex"),
                None,
            )
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires an installed and authenticated Codex CLI"]
    async fn real_codex_cli_generates_a_valid_flow() {
        let path = env::var("REACTOR_CODEX_E2E_PATH").expect("set REACTOR_CODEX_E2E_PATH");
        let generated = CliFlowProvider::new(CliProviderKind::Codex, Some(&path), None)
            .unwrap()
            .generate(generation_request())
            .await
            .unwrap();
        assert_eq!(generated.provider, "codex-cli");
        validate_flow(&generated.flow).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an installed and authenticated Codex CLI"]
    async fn real_codex_cli_modifies_a_valid_flow() {
        let path = env::var("REACTOR_CODEX_E2E_PATH").expect("set REACTOR_CODEX_E2E_PATH");
        let flow = serde_json::from_str(valid_flow_json()).unwrap();
        let generated = CliFlowProvider::new(CliProviderKind::Codex, Some(&path), None)
            .unwrap()
            .modify(FlowModificationRequest {
                flow,
                instruction: "Keep the launch measurement and add a 500 ms pause after it"
                    .to_owned(),
                failure_context: None,
                ui_tree: None,
                source_context: None,
            })
            .await
            .unwrap();
        assert_eq!(generated.provider, "codex-cli");
        validate_flow(&generated.flow).unwrap();
        assert!(generated.flow.measured.len() >= 2);
    }

    #[tokio::test]
    #[ignore = "requires an installed and authenticated Codex CLI"]
    async fn real_codex_cli_explains_an_immutable_analysis_report() {
        let path = env::var("REACTOR_CODEX_E2E_PATH").expect("set REACTOR_CODEX_E2E_PATH");
        let explanation = CliFlowProvider::new(CliProviderKind::Codex, Some(&path), None)
            .unwrap()
            .explain(AnalysisExplanationRequest {
                report: analysis_report(),
            })
            .await
            .unwrap();
        assert_eq!(explanation.verdict, AnalysisVerdict::Regressed);
        assert!(explanation.facts.iter().all(|insight| insight.fact));
        assert!(explanation.hypotheses.iter().all(|insight| {
            !insight.fact && !insight.metric_refs.is_empty() && !insight.evidence_refs.is_empty()
        }));
    }

    #[tokio::test]
    #[ignore = "requires an installed and authenticated Claude Code CLI"]
    async fn real_claude_code_cli_generates_a_valid_flow() {
        let path = env::var("REACTOR_CLAUDE_E2E_PATH").expect("set REACTOR_CLAUDE_E2E_PATH");
        let generated = CliFlowProvider::new(CliProviderKind::ClaudeCode, Some(&path), None)
            .unwrap()
            .generate(generation_request())
            .await
            .unwrap();
        assert_eq!(generated.provider, "claude-code-cli");
        validate_flow(&generated.flow).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an installed and authenticated Claude Code CLI"]
    async fn real_claude_code_cli_explains_an_immutable_analysis_report() {
        let path = env::var("REACTOR_CLAUDE_E2E_PATH").expect("set REACTOR_CLAUDE_E2E_PATH");
        let explanation = CliFlowProvider::new(CliProviderKind::ClaudeCode, Some(&path), None)
            .unwrap()
            .explain(AnalysisExplanationRequest {
                report: analysis_report(),
            })
            .await
            .unwrap();
        assert_eq!(explanation.verdict, AnalysisVerdict::Regressed);
        assert!(explanation.facts.iter().all(|insight| insight.fact));
        assert!(explanation.hypotheses.iter().all(|insight| {
            !insight.fact && !insight.metric_refs.is_empty() && !insight.evidence_refs.is_empty()
        }));
    }

    #[test]
    fn expands_provider_base_url_to_responses_and_chat_endpoints() {
        let endpoints = provider_endpoints("https://open.example.test/v1").unwrap();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].api, OpenAiApi::Responses);
        assert_eq!(endpoints[0].url, "https://open.example.test/v1/responses");
        assert_eq!(endpoints[1].api, OpenAiApi::ChatCompletions);
        assert_eq!(
            endpoints[1].url,
            "https://open.example.test/v1/chat/completions"
        );
    }

    #[test]
    fn preserves_an_explicit_provider_endpoint_and_hides_query_from_errors() {
        let endpoints = provider_endpoints(
            "https://open.example.test/v1/chat/completions?api-version=2026-01-01",
        )
        .unwrap();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].api, OpenAiApi::ChatCompletions);
        assert!(endpoints[0].url.contains("api-version=2026-01-01"));
        assert_eq!(
            endpoints[0].safe_url,
            "https://open.example.test/v1/chat/completions"
        );
    }

    #[test]
    fn extracts_text_from_both_openai_response_contracts() {
        let responses = json!({
            "output": [{ "content": [{ "type": "output_text", "text": "{\"schemaVersion\":1}" }] }]
        });
        let chat = json!({ "choices": [{ "message": { "content": "{\"schemaVersion\":1}" } }] });
        assert_eq!(
            extract_model_text(OpenAiApi::Responses, &responses).unwrap(),
            "{\"schemaVersion\":1}"
        );
        assert_eq!(
            extract_model_text(OpenAiApi::ChatCompletions, &chat).unwrap(),
            "{\"schemaVersion\":1}"
        );
    }

    #[tokio::test]
    async fn local_model_doctor_discovers_models_without_prompting() {
        let (endpoint, request) = one_shot_http_server(
            json!({ "data": [{ "id": "qwen2.5:7b" }, { "id": "llama3.2" }] }).to_string(),
        );
        let status = doctor_local_model(&endpoint).await;
        assert!(status.available);
        assert_eq!(status.models, ["llama3.2", "qwen2.5:7b"]);
        assert!(request.recv().unwrap().starts_with("GET /v1/models "));
    }

    #[tokio::test]
    async fn local_model_provider_sends_no_authorization_header() {
        let payload = json!({ "output_text": valid_flow_json() }).to_string();
        let (endpoint, request) = one_shot_http_server(payload);
        let generated = OpenAiCompatibleProvider::new_local(endpoint, "local-test".to_owned())
            .generate(generation_request())
            .await
            .unwrap();
        assert!(generated.provider.starts_with("local-model-"));
        let request = request.recv().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("post /v1/responses "));
        assert!(!request.contains("authorization:"));
    }

    #[tokio::test]
    async fn offline_analysis_keeps_rule_verdict_and_facts() {
        let explanation = OfflineAnalysisExplainer
            .explain(AnalysisExplanationRequest {
                report: analysis_report(),
            })
            .await
            .unwrap();
        assert_eq!(explanation.verdict, AnalysisVerdict::Regressed);
        assert_eq!(explanation.provider, "reactor-rules");
        assert!(explanation.facts.iter().all(|insight| insight.fact));
        assert!(explanation.hypotheses.is_empty());
    }

    #[tokio::test]
    async fn local_model_analysis_requires_real_evidence_references() {
        let model_output = json!({
            "summary": "The immutable report shows a regression.",
            "hypotheses": [{
                "title": "Possible JS work",
                "text": "Verify with a component profile.",
                "metricRefs": ["frame_time_p95_ms"],
                "evidenceRefs": ["current.metrics.frame_time_p95_ms"]
            }],
            "nextSteps": [{ "title": "Capture profile", "text": "Record the same interaction." }]
        });
        let response = json!({ "output_text": model_output.to_string() }).to_string();
        let (endpoint, request) = one_shot_http_server(response);
        let explanation = OpenAiCompatibleProvider::new_local(endpoint, "local-test".to_owned())
            .explain(AnalysisExplanationRequest {
                report: analysis_report(),
            })
            .await
            .unwrap();
        assert_eq!(explanation.verdict, AnalysisVerdict::Regressed);
        assert!(!explanation.hypotheses[0].fact);
        assert_eq!(
            explanation.hypotheses[0].evidence_refs,
            ["current.metrics.frame_time_p95_ms"]
        );
        assert!(request.recv().unwrap().starts_with("POST /v1/responses "));

        let invalid = ModelAnalysisOutput {
            summary: "invalid".to_owned(),
            hypotheses: vec![ModelHypothesis {
                title: "Invented".to_owned(),
                text: "No evidence".to_owned(),
                metric_refs: vec!["invented_metric".to_owned()],
                evidence_refs: vec!["invented.evidence".to_owned()],
            }],
            next_steps: vec![],
        };
        let error = build_analysis_explanation(
            &analysis_report(),
            invalid,
            "test".to_owned(),
            "test".to_owned(),
        )
        .unwrap_err();
        assert!(matches!(error, AiProviderError::InvalidResponse(_)));
    }

    #[test]
    fn strips_common_markdown_json_fences() {
        assert_eq!(
            strip_markdown_fence("```json\n{\"ok\":true}\n```"),
            "{\"ok\":true}"
        );
        assert_eq!(strip_markdown_fence(" {\"ok\":true} "), "{\"ok\":true}");
    }

    #[test]
    fn selector_inventory_preserves_exact_language_and_case() {
        let inventory = observed_selector_inventory(Some(
            r#"<node text="Sign in" content-desc=""/><node text="Username"/><node accessibilityText=Password;password=true/>"#,
        ));
        let values: Vec<String> = serde_json::from_str(&inventory).unwrap();

        assert!(values.contains(&"Sign in".to_owned()));
        assert!(values.contains(&"Username".to_owned()));
        assert!(values.contains(&"Password".to_owned()));
        assert!(!values.contains(&"登录".to_owned()));
    }

    #[tokio::test]
    async fn offline_composer_creates_list_flow() {
        let result = OfflineFlowComposer
            .generate(FlowGenerationRequest {
                intent: "打开列表并滚动 10 次".to_owned(),
                app_id: "com.reactor.demo".to_owned(),
                platform: Platform::Android,
                ui_tree: None,
                screenshot_artifact_ids: vec![],
                source_context: None,
            })
            .await
            .unwrap();
        assert_eq!(result.flow.id, "list-generated");
        assert!(result.flow.measured.len() >= 4);
    }

    #[tokio::test]
    async fn ten_representative_intents_produce_schema_valid_flows() {
        let intents = [
            "启动应用并测量首屏",
            "Launch the app and benchmark startup",
            "进入列表并滚动 10 次",
            "Open the list and scroll 6 times",
            "刷新数据并等待更新完成",
            "Run the update scenario",
            "播放动画并等待动画完成",
            "Measure the animation scenario",
            "打开列表并滚动 1 次",
            "打开列表并滚动 100 次",
        ];
        for (index, intent) in intents.into_iter().enumerate() {
            let generated = OfflineFlowComposer
                .generate(FlowGenerationRequest {
                    intent: intent.to_owned(),
                    app_id: format!("com.reactor.fixture{index}"),
                    platform: if index.is_multiple_of(2) {
                        Platform::Android
                    } else {
                        Platform::Ios
                    },
                    ui_tree: None,
                    screenshot_artifact_ids: vec![],
                    source_context: None,
                })
                .await
                .unwrap();
            assert!(
                validate_flow(&generated.flow).is_ok(),
                "intent {index} produced an invalid Flow"
            );
        }
    }

    #[test]
    fn redacts_sensitive_ui_values_and_never_uploads_screenshot_bytes() {
        let tree = r#"<hierarchy>
<node text="person@example.com" password="false" bounds="[0,0][10,10]" />
<node text="secret-value" password="true" bounds="[0,10][10,20]" />
</hierarchy>"#;
        let context = redact_ui_tree(tree, 1);
        assert!(context.ui_tree.contains("[REDACTED_EMAIL]"));
        assert!(context.ui_tree.contains("[REDACTED_PASSWORD]"));
        assert_eq!(context.preview.redaction_count, 2);
        assert_eq!(context.preview.screenshot_count, 1);
        assert_eq!(context.preview.screenshot_bytes_uploaded, 0);
    }

    #[test]
    fn minified_password_node_does_not_redact_sibling_selectors() {
        let tree = r#"<hierarchy><node text="Username" content-desc="Username" password="false"/><node text="hunter2" content-desc="Password" password="true"/></hierarchy>"#;
        let context = redact_ui_tree(tree, 0);
        let inventory = observed_selector_inventory(Some(&context.ui_tree));
        let values: Vec<String> = serde_json::from_str(&inventory).unwrap();

        assert!(context.ui_tree.contains(r#"text="Username""#));
        assert!(context.ui_tree.contains(r#"text="[REDACTED_PASSWORD]""#));
        assert!(context.ui_tree.contains(r#"content-desc="Password""#));
        assert!(values.contains(&"Username".to_owned()));
        assert!(values.contains(&"Password".to_owned()));
        assert!(!context.ui_tree.contains("hunter2"));
    }

    #[test]
    fn redacts_sensitive_values_from_ios_compact_hierarchy() {
        let hierarchy = "element_num,depth,attributes,parent_num\n1,1,\"accessibilityText=person@example.com; value=123456789; bounds=[0,0][10,10]\",0";
        let context = redact_ui_tree(hierarchy, 1);
        assert!(context.ui_tree.contains("[REDACTED_EMAIL]"));
        assert!(context.ui_tree.contains("[REDACTED_NUMBER]"));
        assert_eq!(context.preview.redaction_count, 2);
        assert_eq!(context.preview.element_count, 1);
        assert_eq!(context.preview.screenshot_bytes_uploaded, 0);
    }

    #[tokio::test]
    async fn flow_diff_pinpoints_repaired_step() {
        let generated = OfflineFlowComposer
            .generate(FlowGenerationRequest {
                intent: "打开列表并滚动 10 次".to_owned(),
                app_id: "com.reactor.demo".to_owned(),
                platform: Platform::Android,
                ui_tree: None,
                screenshot_artifact_ids: vec![],
                source_context: None,
            })
            .await
            .unwrap();
        let mut repaired = generated.flow.clone();
        let Step::Repeat { times, .. } = repaired.measured.last_mut().unwrap() else {
            panic!("list flow must end in repeat");
        };
        *times = 12;
        let changes = diff_flows(&generated.flow, &repaired).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path.rsplit('.').next(), Some("times"));
    }
}
