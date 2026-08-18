use std::fmt::Write as _;

use reactor_protocol::{Flow, InputValue, Selector, Step, SwipeDirection, validate_flow};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledFlow {
    pub setup: String,
    pub measured: String,
    pub teardown: String,
    pub input_bindings: Vec<CompiledInputBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledInputBinding {
    pub path: String,
    pub environment_key: String,
    pub value: InputValue,
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error(transparent)]
    InvalidFlow(#[from] reactor_protocol::FlowValidationError),
    #[error("{path}: Maestro cannot compile a selector containing only an index")]
    UnsupportedSelector { path: String },
}

/// Compiles each execution phase into a separate Maestro YAML document.
///
/// # Errors
///
/// Returns an error when the Flow is invalid or uses a selector Maestro cannot represent safely.
pub fn compile_maestro(flow: &Flow) -> Result<CompiledFlow, CompileError> {
    validate_flow(flow)?;
    let (setup, mut input_bindings) = compile_section(&flow.app_id, &flow.setup, "setup")?;
    let (measured, measured_bindings) = compile_section(&flow.app_id, &flow.measured, "measured")?;
    let (teardown, teardown_bindings) = compile_section(&flow.app_id, &flow.teardown, "teardown")?;
    input_bindings.extend(measured_bindings);
    input_bindings.extend(teardown_bindings);
    Ok(CompiledFlow {
        setup,
        measured,
        teardown,
        input_bindings,
    })
}

fn compile_section(
    app_id: &str,
    steps: &[Step],
    prefix: &str,
) -> Result<(String, Vec<CompiledInputBinding>), CompileError> {
    let mut output = format!("appId: {}\n---\n", quote(app_id));
    let mut input_bindings = Vec::new();
    compile_steps(steps, prefix, 0, &mut output, &mut input_bindings)?;
    Ok((output, input_bindings))
}

fn compile_steps(
    steps: &[Step],
    prefix: &str,
    indent: usize,
    output: &mut String,
    input_bindings: &mut Vec<CompiledInputBinding>,
) -> Result<(), CompileError> {
    let pad = " ".repeat(indent);
    for (index, step) in steps.iter().enumerate() {
        let path = format!("{prefix}[{index}]");
        match step {
            Step::ResetAppState => writeln!(output, "{pad}- clearState").unwrap(),
            Step::LaunchApp => writeln!(output, "{pad}- launchApp").unwrap(),
            Step::Tap { target } => {
                writeln!(output, "{pad}- tapOn: {}", selector(target, &path)?).unwrap();
            }
            Step::InputText {
                target,
                value,
                clear_before,
            } => {
                writeln!(output, "{pad}- tapOn: {}", selector(target, &path)?).unwrap();
                if *clear_before {
                    writeln!(output, "{pad}- eraseText").unwrap();
                }
                let input = if let InputValue::Literal(value) = value {
                    value.clone()
                } else {
                    let environment_key = input_environment_key(&path);
                    input_bindings.push(CompiledInputBinding {
                        path: path.clone(),
                        environment_key: environment_key.clone(),
                        value: value.clone(),
                    });
                    format!("${{{environment_key}}}")
                };
                writeln!(output, "{pad}- inputText: {}", quote(&input)).unwrap();
            }
            Step::Swipe {
                direction,
                duration_ms,
            } => {
                let direction = match direction {
                    SwipeDirection::Up => "UP",
                    SwipeDirection::Down => "DOWN",
                    SwipeDirection::Left => "LEFT",
                    SwipeDirection::Right => "RIGHT",
                };
                write!(
                    output,
                    "{pad}- swipe:\n{pad}    direction: {direction}\n{pad}    duration: {duration_ms}\n"
                )
                .unwrap();
            }
            Step::WaitFor { target, timeout_ms } => write!(
                output,
                "{pad}- extendedWaitUntil:\n{pad}    visible: {}\n{pad}    timeout: {timeout_ms}\n",
                selector(target, &path)?
            )
            .unwrap(),
            Step::AssertVisible { target } => {
                writeln!(output, "{pad}- assertVisible: {}", selector(target, &path)?).unwrap();
            }
            Step::Pause { duration_ms } => write!(
                output,
                "{pad}- waitForAnimationToEnd:\n{pad}    timeout: {duration_ms}\n"
            )
            .unwrap(),
            Step::Repeat { times, steps } => {
                write!(
                    output,
                    "{pad}- repeat:\n{pad}    times: {times}\n{pad}    commands:\n"
                )
                .unwrap();
                compile_steps(
                    steps,
                    &format!("{path}.steps"),
                    indent + 6,
                    output,
                    input_bindings,
                )?;
            }
        }
    }
    Ok(())
}

fn input_environment_key(path: &str) -> String {
    let path = path
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("MAESTRO_REACTOR_INPUT_{path}")
}

fn selector(selector: &Selector, path: &str) -> Result<String, CompileError> {
    if let Some(id) = selector
        .accessibility_id
        .as_deref()
        .or(selector.semantic_id.as_deref())
    {
        return Ok(match selector.enabled {
            Some(enabled) => format!("{{ id: {}, enabled: {enabled} }}", quote(id)),
            None => format!("{{ id: {} }}", quote(id)),
        });
    }
    if let Some(text) = selector.text.as_deref() {
        return Ok(match selector.enabled {
            Some(enabled) => format!("{{ text: {}, enabled: {enabled} }}", quote(text)),
            None => quote(text),
        });
    }
    if let Some(coordinate) = selector.coordinate {
        return Ok(format!(
            "{{ point: {} }}",
            quote(&format!("{},{}", coordinate.x, coordinate.y))
        ));
    }
    Err(CompileError::UnsupportedSelector {
        path: path.to_owned(),
    })
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use reactor_protocol::{InputValue, Platform, SecretInputReference, Selector};

    use super::*;

    #[test]
    fn compiles_measured_flow() {
        let flow = Flow {
            schema_version: 1,
            id: "list".to_owned(),
            name: "List".to_owned(),
            app_id: "com.reactor.demo".to_owned(),
            platform: Platform::Android,
            intent: None,
            setup: vec![Step::ResetAppState],
            measured: vec![
                Step::LaunchApp,
                Step::Tap {
                    target: Selector {
                        text: Some("List scenario".to_owned()),
                        ..Selector::default()
                    },
                },
            ],
            teardown: vec![],
        };
        let output = compile_maestro(&flow).unwrap();
        assert!(output.setup.contains("clearState"));
        assert!(output.measured.contains("tapOn: \"List scenario\""));
    }

    #[test]
    fn coordinate_selector_compiles_as_a_valid_inline_maestro_mapping() {
        let flow = Flow {
            schema_version: 1,
            id: "canvas".to_owned(),
            name: "Canvas".to_owned(),
            app_id: "com.reactor.demo".to_owned(),
            platform: Platform::Android,
            intent: None,
            setup: vec![],
            measured: vec![Step::Tap {
                target: Selector {
                    coordinate: Some(reactor_protocol::Coordinate { x: 720.0, y: 522.0 }),
                    ..Selector::default()
                },
            }],
            teardown: vec![],
        };
        let output = compile_maestro(&flow).unwrap();
        assert!(output.measured.contains("tapOn: { point: \"720,522\" }"));
    }

    #[test]
    fn enabled_state_assertion_compiles_into_the_maestro_selector() {
        let flow = Flow {
            schema_version: 1,
            id: "enabled-state".to_owned(),
            name: "Enabled state".to_owned(),
            app_id: "com.reactor.demo".to_owned(),
            platform: Platform::Android,
            intent: None,
            setup: vec![],
            measured: vec![Step::AssertVisible {
                target: Selector {
                    text: Some("Continue".to_owned()),
                    enabled: Some(true),
                    ..Selector::default()
                },
            }],
            teardown: vec![],
        };
        let output = compile_maestro(&flow).unwrap();
        assert!(
            output
                .measured
                .contains("assertVisible: { text: \"Continue\", enabled: true }")
        );
    }

    #[test]
    fn referenced_input_compiles_to_environment_placeholder_without_plaintext() {
        let flow = Flow {
            schema_version: 1,
            id: "login".to_owned(),
            name: "Login".to_owned(),
            app_id: "com.reactor.demo".to_owned(),
            platform: Platform::Android,
            intent: None,
            setup: vec![Step::InputText {
                target: Selector {
                    accessibility_id: Some("password".to_owned()),
                    ..Selector::default()
                },
                value: InputValue::SecretRef(SecretInputReference {
                    secret_ref: "test-account.password".to_owned(),
                }),
                clear_before: true,
            }],
            measured: vec![Step::LaunchApp],
            teardown: vec![],
        };
        let output = compile_maestro(&flow).unwrap();
        assert!(
            output
                .setup
                .contains("inputText: \"${MAESTRO_REACTOR_INPUT_SETUP_0_}\"")
        );
        assert!(!output.setup.contains("test-account.password"));
        assert_eq!(output.input_bindings.len(), 1);
        assert_eq!(
            output.input_bindings[0].environment_key,
            "MAESTRO_REACTOR_INPUT_SETUP_0_"
        );
    }
}
