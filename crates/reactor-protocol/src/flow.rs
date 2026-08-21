use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const FLOW_SCHEMA_VERSION: u32 = 1;
const MAX_EXPANDED_STEPS: usize = 1_000;
const MAX_REPEAT_COUNT: u32 = 100;
const MAX_TIMEOUT_MS: u64 = 120_000;
const MAX_GESTURE_MS: u64 = 60_000;
const MAX_INPUT_LITERAL_CHARS: usize = 4_096;
const MAX_INPUT_REFERENCE_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Android,
    Ios,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flow {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub app_id: String,
    pub platform: Platform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    #[serde(default)]
    pub setup: Vec<Step>,
    pub measured: Vec<Step>,
    #[serde(default)]
    pub teardown: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Step {
    ResetAppState,
    LaunchApp,
    Tap {
        target: Selector,
    },
    InputText {
        target: Selector,
        #[serde(alias = "text")]
        value: InputValue,
        #[serde(default = "default_true")]
        clear_before: bool,
    },
    Swipe {
        direction: SwipeDirection,
        duration_ms: u64,
    },
    WaitFor {
        target: Selector,
        timeout_ms: u64,
    },
    AssertVisible {
        target: Selector,
    },
    Pause {
        duration_ms: u64,
    },
    Repeat {
        times: u32,
        steps: Vec<Self>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandedFlowStep {
    pub id: String,
    pub path: String,
    pub section: String,
    pub action: String,
}

impl Flow {
    #[must_use]
    pub fn expanded_steps(&self) -> Vec<ExpandedFlowStep> {
        let mut expanded = Vec::new();
        expand_steps(&self.setup, "setup", "setup", &mut expanded);
        expand_steps(&self.measured, "measured", "measured", &mut expanded);
        expand_steps(&self.teardown, "teardown", "teardown", &mut expanded);
        expanded
    }
}

fn expand_steps(steps: &[Step], section: &str, prefix: &str, expanded: &mut Vec<ExpandedFlowStep>) {
    for (index, step) in steps.iter().enumerate() {
        let path = format!("{prefix}[{index}]");
        if let Step::Repeat { times, steps } = step {
            for iteration in 0..*times {
                expand_steps(
                    steps,
                    section,
                    &format!("{path}.repeat[{iteration}].steps"),
                    expanded,
                );
            }
        } else {
            expanded.push(ExpandedFlowStep {
                id: format!("flow-step:{path}"),
                path,
                section: section.to_owned(),
                action: step.action_name().to_owned(),
            });
        }
    }
}

impl Step {
    #[must_use]
    pub const fn action_name(&self) -> &'static str {
        match self {
            Self::ResetAppState => "reset_app_state",
            Self::LaunchApp => "launch_app",
            Self::Tap { .. } => "tap",
            Self::InputText { .. } => "input_text",
            Self::Swipe { .. } => "swipe",
            Self::WaitFor { .. } => "wait_for",
            Self::AssertVisible { .. } => "assert_visible",
            Self::Pause { .. } => "pause",
            Self::Repeat { .. } => "repeat",
        }
    }
}

const fn default_true() -> bool {
    true
}

/// A Flow-owned input value. Plain strings remain valid for backwards compatibility and represent
/// non-sensitive literals. All sensitive or runtime-dependent values are stored only as named
/// references and resolved immediately before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    Literal(String),
    VariableRef(VariableInputReference),
    SecretRef(SecretInputReference),
    PromptRef(PromptInputReference),
    TotpRef(TotpInputReference),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VariableInputReference {
    pub variable_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretInputReference {
    pub secret_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptInputReference {
    pub prompt_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TotpInputReference {
    pub totp_ref: String,
}

impl InputValue {
    #[must_use]
    pub fn reference(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Literal(_) => None,
            Self::VariableRef(value) => Some(("variableRef", &value.variable_ref)),
            Self::SecretRef(value) => Some(("secretRef", &value.secret_ref)),
            Self::PromptRef(value) => Some(("promptRef", &value.prompt_ref)),
            Self::TotpRef(value) => Some(("totpRef", &value.totp_ref)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Selector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessibility_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate: Option<Coordinate>,
}

impl Selector {
    fn is_empty(&self) -> bool {
        self.semantic_id.as_deref().is_none_or(str::is_empty)
            && self.accessibility_id.as_deref().is_none_or(str::is_empty)
            && self.text.as_deref().is_none_or(str::is_empty)
            && self.coordinate.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Coordinate {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationProvenance {
    pub provider: String,
    pub model: String,
    pub prompt_template_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialMode {
    AndroidTarget,
    IosSimulator,
    ProductTourValidation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowTrialEvidence {
    pub schema_version: u32,
    pub mode: TrialMode,
    pub passed: bool,
    pub flow_hash: String,
    pub executed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_dir: Option<String>,
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowLock {
    pub schema_version: u32,
    pub flow_hash: String,
    pub locked_at: DateTime<Utc>,
    pub compiler_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<GenerationProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial: Option<FlowTrialEvidence>,
    pub flow: Flow,
}

impl FlowLock {
    /// Creates the immutable input consumed by measured runs.
    ///
    /// # Errors
    ///
    /// Returns a validation or serialization error when the Flow cannot be safely locked.
    pub fn new(
        flow: Flow,
        generation: Option<GenerationProvenance>,
    ) -> Result<Self, FlowValidationError> {
        Self::new_with_trial(flow, generation, None)
    }

    /// Creates a lock and binds optional pre-measurement trial evidence to the Flow hash.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails or the trial does not prove this exact Flow.
    pub fn new_with_trial(
        flow: Flow,
        generation: Option<GenerationProvenance>,
        trial: Option<FlowTrialEvidence>,
    ) -> Result<Self, FlowValidationError> {
        validate_flow(&flow)?;
        let flow_hash = canonical_flow_hash(&flow)?;
        if let Some(evidence) = &trial {
            if !evidence.passed {
                return Err(FlowValidationError::TrialDidNotPass);
            }
            if evidence.flow_hash != flow_hash {
                return Err(FlowValidationError::TrialHashMismatch);
            }
        }
        Ok(Self {
            schema_version: 1,
            flow_hash,
            locked_at: Utc::now(),
            compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
            generation,
            trial,
            flow,
        })
    }

    /// Recomputes the immutable Flow hash before execution.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock or attached trial evidence was modified.
    pub fn verify(&self) -> Result<(), FlowValidationError> {
        validate_flow(&self.flow)?;
        let actual = canonical_flow_hash(&self.flow)?;
        let legacy_matches = if actual == self.flow_hash {
            false
        } else {
            legacy_flow_hash(&self.flow)?.as_deref() == Some(self.flow_hash.as_str())
        };
        if actual != self.flow_hash && !legacy_matches {
            return Err(FlowValidationError::LockHashMismatch);
        }
        if let Some(evidence) = &self.trial {
            if !evidence.passed {
                return Err(FlowValidationError::TrialDidNotPass);
            }
            if evidence.flow_hash != self.flow_hash {
                return Err(FlowValidationError::TrialHashMismatch);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn has_android_trial(&self, device_id: &str) -> bool {
        self.trial.as_ref().is_some_and(|trial| {
            trial.passed
                && !trial.synthetic
                && trial.mode == TrialMode::AndroidTarget
                && trial.device_id.as_deref() == Some(device_id)
                && trial.flow_hash == self.flow_hash
        })
    }

    #[must_use]
    pub fn has_ios_simulator_trial(&self, device_id: &str) -> bool {
        self.trial.as_ref().is_some_and(|trial| {
            trial.passed
                && !trial.synthetic
                && trial.mode == TrialMode::IosSimulator
                && trial.device_id.as_deref() == Some(device_id)
                && trial.flow_hash == self.flow_hash
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowWarning {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FlowValidationReport {
    pub warnings: Vec<FlowWarning>,
}

#[derive(Debug, Error)]
pub enum FlowValidationError {
    #[error("unsupported Flow schema version {actual}; expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("{field} must not be empty")]
    EmptyField { field: &'static str },
    #[error("measured must contain at least one step")]
    EmptyMeasuredSteps,
    #[error("intent requires navigation, but Flow has no tap/input navigation step")]
    MissingIntentNavigation,
    #[error(
        "intent requires navigation, but Flow does not verify the destination after navigation"
    )]
    MissingIntentVerification,
    #[error("{path}: selector must contain a semantic id, accessibility id, text, or coordinate")]
    EmptySelector { path: String },
    #[error(
        "{path}: sensitive action target requires an explicit non-automated workflow: {target}"
    )]
    SensitiveActionTarget { path: String, target: String },
    #[error(
        "{path}: {action} is a preparation action and is forbidden inside measured; move it to setup"
    )]
    PreparationInsideMeasuredWindow { path: String, action: &'static str },
    #[error("{path}: repeat count must be between 1 and {MAX_REPEAT_COUNT}")]
    InvalidRepeat { path: String },
    #[error("{path}: duration exceeds the limit")]
    DurationTooLong { path: String },
    #[error("{path}: input value must not be empty")]
    EmptyInputValue { path: String },
    #[error("{path}: {kind} must contain a non-empty reference name")]
    EmptyInputReference { path: String, kind: &'static str },
    #[error("{path}: input value exceeds its maximum length")]
    InputValueTooLong { path: String },
    #[error("{path}: sensitive input target cannot store a literal value in Flow")]
    SensitiveInputLiteral { path: String },
    #[error("expanded Flow has more than {MAX_EXPANDED_STEPS} steps")]
    TooManySteps,
    #[error("Flow lock hash does not match its canonical Flow")]
    LockHashMismatch,
    #[error("trial evidence does not match the Flow hash")]
    TrialHashMismatch,
    #[error("trial evidence is not successful")]
    TrialDidNotPass,
    #[error("failed to serialize Flow for hashing: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Validates structural limits and the deterministic measurement boundary.
///
/// # Errors
///
/// Returns the first schema or safety violation with its step path.
pub fn validate_flow(flow: &Flow) -> Result<FlowValidationReport, FlowValidationError> {
    if flow.schema_version != FLOW_SCHEMA_VERSION {
        return Err(FlowValidationError::UnsupportedSchema {
            actual: flow.schema_version,
            expected: FLOW_SCHEMA_VERSION,
        });
    }
    for (field, value) in [
        ("id", flow.id.as_str()),
        ("name", flow.name.as_str()),
        ("appId", flow.app_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(FlowValidationError::EmptyField { field });
        }
    }
    if flow.measured.is_empty() {
        return Err(FlowValidationError::EmptyMeasuredSteps);
    }
    validate_intent_alignment(flow)?;

    let mut report = FlowValidationReport::default();
    let mut expanded_steps = 0;
    validate_steps(
        &flow.setup,
        "setup",
        false,
        &mut expanded_steps,
        &mut report,
    )?;
    validate_steps(
        &flow.measured,
        "measured",
        true,
        &mut expanded_steps,
        &mut report,
    )?;
    validate_steps(
        &flow.teardown,
        "teardown",
        false,
        &mut expanded_steps,
        &mut report,
    )?;
    Ok(report)
}

fn validate_intent_alignment(flow: &Flow) -> Result<(), FlowValidationError> {
    if !requires_navigation_intent(flow) {
        return Ok(());
    }

    let mut saw_navigation = false;
    let mut verified_after_navigation = false;
    let mut last_navigation_target = None;
    let mut destination_marker = None;
    // Teardown returns the app to a neutral state and is deliberately excluded from goal proof.
    // Every navigation in setup/measured invalidates an earlier proof, so the final navigation
    // must have its own destination marker.
    for steps in [&flow.setup, &flow.measured] {
        inspect_intent_steps(
            steps,
            &mut saw_navigation,
            &mut verified_after_navigation,
            &mut last_navigation_target,
            &mut destination_marker,
        );
    }
    if !saw_navigation {
        return Err(FlowValidationError::MissingIntentNavigation);
    }
    if !verified_after_navigation {
        return Err(FlowValidationError::MissingIntentVerification);
    }
    Ok(())
}

/// Returns whether the user intent explicitly asks the Flow to enter another screen/state.
#[must_use]
pub fn requires_navigation_intent(flow: &Flow) -> bool {
    let Some(intent) = flow.intent.as_deref() else {
        return false;
    };
    let intent = intent.to_lowercase();
    [
        "进入",
        "打开详情",
        "打开第",
        "打开列表",
        "open the list",
        "open list",
        "open detail",
        "navigate",
        "go to",
        "enter the",
    ]
    .iter()
    .any(|needle| intent.contains(needle))
}

/// Returns the stable marker that proves the destination after the final navigation.
///
/// Validation must be run first; an invalid or non-navigation Flow returns `None`.
#[must_use]
pub fn navigation_destination_marker(flow: &Flow) -> Option<Selector> {
    if !requires_navigation_intent(flow) {
        return None;
    }
    let mut saw_navigation = false;
    let mut verified_after_navigation = false;
    let mut last_navigation_target = None;
    let mut destination_marker = None;
    for steps in [&flow.setup, &flow.measured] {
        inspect_intent_steps(
            steps,
            &mut saw_navigation,
            &mut verified_after_navigation,
            &mut last_navigation_target,
            &mut destination_marker,
        );
    }
    verified_after_navigation
        .then_some(destination_marker)
        .flatten()
}

fn inspect_intent_steps(
    steps: &[Step],
    saw_navigation: &mut bool,
    verified_after_navigation: &mut bool,
    last_navigation_target: &mut Option<Selector>,
    destination_marker: &mut Option<Selector>,
) {
    for step in steps {
        match step {
            Step::Tap { target } | Step::InputText { target, .. } => {
                *saw_navigation = true;
                *verified_after_navigation = false;
                *last_navigation_target = Some(target.clone());
                *destination_marker = None;
            }
            Step::WaitFor { target, .. } | Step::AssertVisible { target }
                if *saw_navigation
                    && is_stable_destination_selector(target)
                    && last_navigation_target
                        .as_ref()
                        .is_none_or(|navigation| !selectors_share_identity(navigation, target)) =>
            {
                *verified_after_navigation = true;
                *destination_marker = Some(target.clone());
            }
            Step::Repeat { steps, .. } => {
                inspect_intent_steps(
                    steps,
                    saw_navigation,
                    verified_after_navigation,
                    last_navigation_target,
                    destination_marker,
                );
            }
            _ => {}
        }
    }
}

fn is_stable_destination_selector(selector: &Selector) -> bool {
    selector
        .semantic_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || selector
            .accessibility_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || selector
            .text
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn selectors_share_identity(left: &Selector, right: &Selector) -> bool {
    [
        (left.semantic_id.as_deref(), right.semantic_id.as_deref()),
        (
            left.accessibility_id.as_deref(),
            right.accessibility_id.as_deref(),
        ),
        (left.text.as_deref(), right.text.as_deref()),
    ]
    .into_iter()
    .any(|(left, right)| {
        left.zip(right)
            .is_some_and(|(left, right)| !left.is_empty() && left == right)
    })
}

fn validate_steps(
    steps: &[Step],
    prefix: &str,
    measured: bool,
    expanded_steps: &mut usize,
    report: &mut FlowValidationReport,
) -> Result<(), FlowValidationError> {
    for (index, step) in steps.iter().enumerate() {
        let path = format!("{prefix}[{index}]");
        *expanded_steps += 1;
        if *expanded_steps > MAX_EXPANDED_STEPS {
            return Err(FlowValidationError::TooManySteps);
        }
        match step {
            Step::ResetAppState | Step::LaunchApp if measured => {
                return Err(FlowValidationError::PreparationInsideMeasuredWindow {
                    path,
                    action: step.action_name(),
                });
            }
            Step::Tap { target } => {
                if measured && selector_is_authentication_preparation(target) {
                    return Err(FlowValidationError::PreparationInsideMeasuredWindow {
                        path,
                        action: "authentication tap",
                    });
                }
                validate_selector(target, &path, report)?;
                validate_safe_action_target(target, &path)?;
            }
            Step::InputText { target, value, .. } => {
                if measured && input_is_authentication_preparation(target, value) {
                    return Err(FlowValidationError::PreparationInsideMeasuredWindow {
                        path,
                        action: "authentication input",
                    });
                }
                validate_selector(target, &path, report)?;
                validate_safe_action_target(target, &path)?;
                validate_input_value(value, &path)?;
                validate_sensitive_input_value(target, value, &path)?;
            }
            Step::AssertVisible { target } => {
                if measured {
                    return Err(FlowValidationError::PreparationInsideMeasuredWindow {
                        path,
                        action: step.action_name(),
                    });
                }
                validate_selector(target, &path, report)?;
            }
            Step::WaitFor { target, timeout_ms } => {
                if measured {
                    return Err(FlowValidationError::PreparationInsideMeasuredWindow {
                        path,
                        action: step.action_name(),
                    });
                }
                validate_selector(target, &path, report)?;
                if *timeout_ms > MAX_TIMEOUT_MS {
                    return Err(FlowValidationError::DurationTooLong { path });
                }
            }
            Step::Swipe { duration_ms, .. } | Step::Pause { duration_ms }
                if *duration_ms > MAX_GESTURE_MS =>
            {
                return Err(FlowValidationError::DurationTooLong { path });
            }
            Step::Repeat { times, steps } => {
                if *times == 0 || *times > MAX_REPEAT_COUNT {
                    return Err(FlowValidationError::InvalidRepeat { path });
                }
                let before_children = *expanded_steps;
                validate_steps(
                    steps,
                    &format!("{path}.steps"),
                    measured,
                    expanded_steps,
                    report,
                )?;
                let child_count = expanded_steps.saturating_sub(before_children);
                *expanded_steps = expanded_steps.saturating_add(
                    child_count.saturating_mul((*times as usize).saturating_sub(1)),
                );
                if *expanded_steps > MAX_EXPANDED_STEPS {
                    return Err(FlowValidationError::TooManySteps);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn input_is_authentication_preparation(target: &Selector, value: &InputValue) -> bool {
    selector_is_authentication_preparation(target)
        || matches!(value, InputValue::SecretRef(_) | InputValue::TotpRef(_))
        || value
            .reference()
            .is_some_and(|(_, reference)| authentication_identity(reference))
}

fn selector_is_authentication_preparation(selector: &Selector) -> bool {
    [
        selector.semantic_id.as_deref(),
        selector.accessibility_id.as_deref(),
        selector.text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(authentication_identity)
}

fn authentication_identity(value: &str) -> bool {
    let value = value.to_lowercase();
    [
        "username",
        "user name",
        "email",
        "password",
        "passcode",
        "sign in",
        "signin",
        "log in",
        "login",
        "auth-submit",
        "one-time code",
        "verification code",
        "用户名",
        "账号",
        "邮箱",
        "密码",
        "登录",
        "验证码",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn validate_sensitive_input_value(
    target: &Selector,
    value: &InputValue,
    path: &str,
) -> Result<(), FlowValidationError> {
    if !matches!(value, InputValue::Literal(_)) {
        return Ok(());
    }
    let identity = [
        target.semantic_id.as_deref(),
        target.accessibility_id.as_deref(),
        target.text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    if [
        "password", "passwd", "passcode", "secret", "token", "密码", "口令",
    ]
    .iter()
    .any(|keyword| identity.contains(keyword))
    {
        return Err(FlowValidationError::SensitiveInputLiteral {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn validate_input_value(value: &InputValue, path: &str) -> Result<(), FlowValidationError> {
    if let InputValue::Literal(value) = value {
        if value.is_empty() {
            return Err(FlowValidationError::EmptyInputValue {
                path: path.to_owned(),
            });
        }
        if value.chars().count() > MAX_INPUT_LITERAL_CHARS {
            return Err(FlowValidationError::InputValueTooLong {
                path: path.to_owned(),
            });
        }
        return Ok(());
    }
    let (kind, reference) = value.reference().expect("reference variant checked above");
    if reference.trim().is_empty() {
        return Err(FlowValidationError::EmptyInputReference {
            path: path.to_owned(),
            kind,
        });
    }
    if reference.chars().count() > MAX_INPUT_REFERENCE_CHARS {
        return Err(FlowValidationError::InputValueTooLong {
            path: path.to_owned(),
        });
    }
    Ok(())
}

fn validate_safe_action_target(target: &Selector, path: &str) -> Result<(), FlowValidationError> {
    let value = [
        target.semantic_id.as_deref(),
        target.accessibility_id.as_deref(),
        target.text.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    let sensitive = [
        "delete",
        "remove account",
        "pay now",
        "purchase",
        "buy now",
        "checkout",
        "transfer",
        "unsubscribe",
        "uninstall",
        "sign out",
        "log out",
        "删除",
        "支付",
        "购买",
        "立即下单",
        "确认订单",
        "转账",
        "注销账号",
        "退出登录",
    ]
    .iter()
    .find(|needle| value.contains(**needle));
    if let Some(target) = sensitive {
        return Err(FlowValidationError::SensitiveActionTarget {
            path: path.to_owned(),
            target: (*target).to_owned(),
        });
    }
    Ok(())
}

fn validate_selector(
    selector: &Selector,
    path: &str,
    report: &mut FlowValidationReport,
) -> Result<(), FlowValidationError> {
    if selector.is_empty() {
        return Err(FlowValidationError::EmptySelector {
            path: path.to_owned(),
        });
    }
    if selector.coordinate.is_some() {
        report.warnings.push(FlowWarning {
            path: path.to_owned(),
            code: "coordinate_selector".to_owned(),
            message: "coordinate selectors are fragile and require explicit review".to_owned(),
        });
    }
    Ok(())
}

/// Hashes the schema-owned struct representation. Flow v1 deliberately contains no map fields,
/// making `serde_json` field ordering deterministic across Reactor components.
///
/// # Errors
///
/// Returns an error if the Flow cannot be serialized.
pub fn canonical_flow_hash(flow: &Flow) -> Result<String, FlowValidationError> {
    let bytes = serde_json::to_vec(flow)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacyFlow<'a> {
    schema_version: u32,
    id: &'a str,
    name: &'a str,
    app_id: &'a str,
    platform: Platform,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent: Option<&'a str>,
    setup: Vec<LegacyStep<'a>>,
    measured: Vec<LegacyStep<'a>>,
    teardown: Vec<LegacyStep<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum LegacyStep<'a> {
    ResetAppState,
    LaunchApp,
    Tap {
        target: &'a Selector,
    },
    InputText {
        target: &'a Selector,
        text: &'a str,
    },
    Swipe {
        direction: SwipeDirection,
        duration_ms: u64,
    },
    WaitFor {
        target: &'a Selector,
        timeout_ms: u64,
    },
    AssertVisible {
        target: &'a Selector,
    },
    Pause {
        duration_ms: u64,
    },
    Repeat {
        times: u32,
        steps: Vec<LegacyStep<'a>>,
    },
}

fn legacy_steps(steps: &[Step]) -> Option<Vec<LegacyStep<'_>>> {
    steps
        .iter()
        .map(|step| {
            Some(match step {
                Step::ResetAppState => LegacyStep::ResetAppState,
                Step::LaunchApp => LegacyStep::LaunchApp,
                Step::Tap { target } => LegacyStep::Tap { target },
                Step::InputText {
                    target,
                    value: InputValue::Literal(text),
                    clear_before: true,
                } => LegacyStep::InputText { target, text },
                Step::InputText { .. } => return None,
                Step::Swipe {
                    direction,
                    duration_ms,
                } => LegacyStep::Swipe {
                    direction: *direction,
                    duration_ms: *duration_ms,
                },
                Step::WaitFor { target, timeout_ms } => LegacyStep::WaitFor {
                    target,
                    timeout_ms: *timeout_ms,
                },
                Step::AssertVisible { target } => LegacyStep::AssertVisible { target },
                Step::Pause { duration_ms } => LegacyStep::Pause {
                    duration_ms: *duration_ms,
                },
                Step::Repeat { times, steps } => LegacyStep::Repeat {
                    times: *times,
                    steps: legacy_steps(steps)?,
                },
            })
        })
        .collect()
}

fn legacy_flow_hash(flow: &Flow) -> Result<Option<String>, FlowValidationError> {
    let Some(setup) = legacy_steps(&flow.setup) else {
        return Ok(None);
    };
    let Some(measured) = legacy_steps(&flow.measured) else {
        return Ok(None);
    };
    let Some(teardown) = legacy_steps(&flow.teardown) else {
        return Ok(None);
    };
    let legacy = LegacyFlow {
        schema_version: flow.schema_version,
        id: &flow.id,
        name: &flow.name,
        app_id: &flow.app_id,
        platform: flow.platform,
        intent: flow.intent.as_deref(),
        setup,
        measured,
        teardown,
    };
    Ok(Some(hex::encode(Sha256::digest(serde_json::to_vec(
        &legacy,
    )?))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_flow() -> Flow {
        Flow {
            schema_version: 1,
            id: "list-scroll".to_owned(),
            name: "List scroll".to_owned(),
            app_id: "com.reactor.demo".to_owned(),
            platform: Platform::Android,
            intent: Some("measure list scrolling".to_owned()),
            setup: vec![Step::ResetAppState, Step::LaunchApp],
            measured: vec![Step::Repeat {
                times: 8,
                steps: vec![Step::Swipe {
                    direction: SwipeDirection::Up,
                    duration_ms: 800,
                }],
            }],
            teardown: vec![],
        }
    }

    #[test]
    fn expanded_steps_have_stable_repeat_paths() {
        let flow = valid_flow();
        let steps = flow.expanded_steps();
        assert_eq!(steps[0].id, "flow-step:setup[0]");
        assert_eq!(steps[1].id, "flow-step:setup[1]");
        assert_eq!(steps[2].id, "flow-step:measured[0].repeat[0].steps[0]");
        assert_eq!(steps[9].id, "flow-step:measured[0].repeat[7].steps[0]");
        assert_eq!(steps[2].action, "swipe");
        assert_eq!(steps, flow.expanded_steps());
    }

    #[test]
    fn stable_hash_for_identical_flow() {
        let flow = valid_flow();
        assert_eq!(
            canonical_flow_hash(&flow).unwrap(),
            canonical_flow_hash(&flow).unwrap()
        );
    }

    #[test]
    fn verifies_legacy_input_lock_after_text_field_migration() {
        let legacy_json = r#"{"schemaVersion":1,"id":"legacy-input","name":"Legacy input","appId":"com.reactor.demo","platform":"android","setup":[],"measured":[{"action":"input_text","target":{"text":"Search"},"text":"hello"}],"teardown":[]}"#;
        let flow: Flow = serde_json::from_str(legacy_json).unwrap();
        let legacy_hash = hex::encode(Sha256::digest(legacy_json.as_bytes()));
        assert_ne!(canonical_flow_hash(&flow).unwrap(), legacy_hash);
        let lock = FlowLock {
            schema_version: 1,
            flow_hash: legacy_hash,
            locked_at: Utc::now(),
            compiler_version: "0.1.0".to_owned(),
            generation: None,
            trial: None,
            flow,
        };
        assert!(lock.verify().is_ok());
    }

    #[test]
    fn rejects_reset_inside_measurement() {
        let mut flow = valid_flow();
        flow.measured = vec![Step::ResetAppState];
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::PreparationInsideMeasuredWindow {
                action: "reset_app_state",
                ..
            })
        ));
    }

    #[test]
    fn rejects_launch_readiness_and_authentication_inside_measurement() {
        let mut flow = valid_flow();
        for step in [
            Step::LaunchApp,
            Step::WaitFor {
                target: Selector {
                    text: Some("Ready".to_owned()),
                    ..Selector::default()
                },
                timeout_ms: 1_000,
            },
            Step::InputText {
                target: Selector {
                    text: Some("Username".to_owned()),
                    ..Selector::default()
                },
                value: InputValue::PromptRef(PromptInputReference {
                    prompt_ref: "auth.username".to_owned(),
                }),
                clear_before: true,
            },
            Step::Tap {
                target: Selector {
                    semantic_id: Some("auth-submit-signin".to_owned()),
                    ..Selector::default()
                },
            },
        ] {
            flow.measured = vec![step];
            assert!(matches!(
                validate_flow(&flow),
                Err(FlowValidationError::PreparationInsideMeasuredWindow { .. })
            ));
        }
    }

    #[test]
    fn warns_about_coordinate_selector() {
        let mut flow = valid_flow();
        flow.measured = vec![Step::Tap {
            target: Selector {
                coordinate: Some(Coordinate { x: 10.0, y: 20.0 }),
                ..Selector::default()
            },
        }];
        assert_eq!(validate_flow(&flow).unwrap().warnings.len(), 1);
    }

    #[test]
    fn rejects_navigation_intent_without_navigation_or_destination_proof() {
        let mut flow = valid_flow();
        flow.intent = Some("启动应用，进入列表页面并滚动".to_owned());
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::MissingIntentNavigation)
        ));

        flow.setup.push(Step::Tap {
            target: Selector {
                text: Some("List scenario".to_owned()),
                ..Selector::default()
            },
        });
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::MissingIntentVerification)
        ));

        flow.setup.push(Step::WaitFor {
            target: Selector {
                text: Some("List ready".to_owned()),
                ..Selector::default()
            },
            timeout_ms: 10_000,
        });
        assert!(validate_flow(&flow).is_ok());
    }

    #[test]
    fn destination_proof_cannot_reuse_the_navigation_target() {
        let mut flow = valid_flow();
        flow.intent = Some("进入列表页面并滚动".to_owned());
        let list_button = Selector {
            text: Some("List scenario".to_owned()),
            ..Selector::default()
        };
        flow.setup.extend([
            Step::Tap {
                target: list_button.clone(),
            },
            Step::AssertVisible {
                target: list_button,
            },
        ]);

        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::MissingIntentVerification)
        ));
    }

    #[test]
    fn final_navigation_requires_its_own_destination_proof() {
        let mut flow = valid_flow();
        flow.intent = Some("进入列表页面，再打开详情".to_owned());
        flow.setup.extend([
            Step::Tap {
                target: Selector {
                    text: Some("List scenario".to_owned()),
                    ..Selector::default()
                },
            },
            Step::WaitFor {
                target: Selector {
                    text: Some("List ready".to_owned()),
                    ..Selector::default()
                },
                timeout_ms: 10_000,
            },
            Step::Tap {
                target: Selector {
                    text: Some("Item 1".to_owned()),
                    ..Selector::default()
                },
            },
        ]);

        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::MissingIntentVerification)
        ));

        flow.setup.push(Step::AssertVisible {
            target: Selector {
                accessibility_id: Some("detail-screen".to_owned()),
                ..Selector::default()
            },
        });
        assert!(validate_flow(&flow).is_ok());
    }

    #[test]
    fn coordinate_only_check_cannot_prove_a_destination() {
        let mut flow = valid_flow();
        flow.intent = Some("进入列表页面并滚动".to_owned());
        flow.setup.extend([
            Step::Tap {
                target: Selector {
                    text: Some("List scenario".to_owned()),
                    ..Selector::default()
                },
            },
            Step::AssertVisible {
                target: Selector {
                    coordinate: Some(Coordinate { x: 20.0, y: 30.0 }),
                    ..Selector::default()
                },
            },
        ]);

        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::MissingIntentVerification)
        ));
    }

    #[test]
    fn rejects_sensitive_automatic_action_targets() {
        let mut flow = valid_flow();
        flow.measured = vec![Step::Tap {
            target: Selector {
                text: Some("Delete account".to_owned()),
                ..Selector::default()
            },
        }];
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::SensitiveActionTarget { .. })
        ));
    }

    #[test]
    fn input_values_keep_legacy_literals_and_serialize_references_without_secrets() {
        let legacy: Step = serde_json::from_value(serde_json::json!({
            "action": "input_text",
            "target": { "text": "Search" },
            "text": "visible test value"
        }))
        .unwrap();
        assert!(matches!(
            legacy,
            Step::InputText {
                value: InputValue::Literal(ref value),
                ..
            } if value == "visible test value"
        ));

        let referenced = Step::InputText {
            target: Selector {
                accessibility_id: Some("password".to_owned()),
                ..Selector::default()
            },
            value: InputValue::SecretRef(SecretInputReference {
                secret_ref: "test-account.password".to_owned(),
            }),
            clear_before: true,
        };
        let json = serde_json::to_string(&referenced).unwrap();
        assert!(json.contains("\"value\":{\"secretRef\":\"test-account.password\"}"));
        assert!(!json.contains("password-value"));
    }

    #[test]
    fn rejects_empty_or_oversized_input_references() {
        let mut flow = valid_flow();
        flow.setup.push(Step::InputText {
            target: Selector {
                accessibility_id: Some("username".to_owned()),
                ..Selector::default()
            },
            value: InputValue::VariableRef(VariableInputReference {
                variable_ref: "  ".to_owned(),
            }),
            clear_before: true,
        });
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::EmptyInputReference { .. })
        ));

        if let Some(Step::InputText { value, .. }) = flow.setup.last_mut() {
            *value = InputValue::SecretRef(SecretInputReference {
                secret_ref: "x".repeat(MAX_INPUT_REFERENCE_CHARS + 1),
            });
        }
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::InputValueTooLong { .. })
        ));
    }

    #[test]
    fn rejects_literal_password_values_in_flow() {
        let mut flow = valid_flow();
        flow.setup.push(Step::InputText {
            target: Selector {
                accessibility_id: Some("login-password".to_owned()),
                ..Selector::default()
            },
            value: InputValue::Literal("must-not-be-stored".to_owned()),
            clear_before: true,
        });
        assert!(matches!(
            validate_flow(&flow),
            Err(FlowValidationError::SensitiveInputLiteral { .. })
        ));
    }

    #[test]
    fn detects_tampered_lock() {
        let mut lock = FlowLock::new(valid_flow(), None).unwrap();
        lock.flow.name = "Tampered".to_owned();
        assert!(matches!(
            lock.verify(),
            Err(FlowValidationError::LockHashMismatch)
        ));
    }
}
