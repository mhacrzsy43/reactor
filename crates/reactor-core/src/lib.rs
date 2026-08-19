//! Deterministic use cases shared by the CLI, runner, and desktop app.

mod compiler;
mod plan;
mod report;
mod stats;

pub use compiler::{CompileError, CompiledFlow, compile_maestro};
pub use plan::{PlanError, RunPlan, RunPlanInput, RunTask, build_run_plan};
pub use report::render_html_report;
pub use stats::{aggregate_iterations, mean, percentile};
