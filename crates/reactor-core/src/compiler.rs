use std::fmt::Write as _;

use reactor_protocol::{Flow, Selector, Step, SwipeDirection, validate_flow};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledFlow {
    pub setup: String,
    pub measured: String,
    pub teardown: String,
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
    Ok(CompiledFlow {
        setup: compile_section(&flow.app_id, &flow.setup, "setup")?,
        measured: compile_section(&flow.app_id, &flow.measured, "measured")?,
        teardown: compile_section(&flow.app_id, &flow.teardown, "teardown")?,
    })
}

fn compile_section(app_id: &str, steps: &[Step], prefix: &str) -> Result<String, CompileError> {
    let mut output = format!("appId: {}\n---\n", quote(app_id));
    compile_steps(steps, prefix, 0, &mut output)?;
    Ok(output)
}

fn compile_steps(
    steps: &[Step],
    prefix: &str,
    indent: usize,
    output: &mut String,
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
            Step::InputText { target, text } => {
                writeln!(output, "{pad}- tapOn: {}", selector(target, &path)?).unwrap();
                writeln!(output, "{pad}- inputText: {}", quote(text)).unwrap();
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
                compile_steps(steps, &format!("{path}.steps"), indent + 6, output)?;
            }
        }
    }
    Ok(())
}

fn selector(selector: &Selector, path: &str) -> Result<String, CompileError> {
    if let Some(id) = selector
        .accessibility_id
        .as_deref()
        .or(selector.semantic_id.as_deref())
    {
        return Ok(format!("{{ id: {} }}", quote(id)));
    }
    if let Some(text) = selector.text.as_deref() {
        return Ok(quote(text));
    }
    if let Some(coordinate) = selector.coordinate {
        return Ok(format!("point: {},{}", coordinate.x, coordinate.y));
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
    use reactor_protocol::{Platform, Selector};

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
}
