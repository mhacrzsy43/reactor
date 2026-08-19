use std::path::PathBuf;

use clap::{Parser, Subcommand};
use reactor_protocol::FlowLock;
use reactor_runner::{
    AndroidRunRequest, IosRunRequest, doctor, run_android, run_demo_suite, run_ios,
};

#[derive(Debug, Parser)]
#[command(name = "reactor-runner", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Doctor {
        workspace: PathBuf,
    },
    Demo {
        workspace: PathBuf,
        flow_lock: PathBuf,
    },
    Android {
        workspace: PathBuf,
        flow_lock: PathBuf,
        framework: String,
        scenario: String,
        device: String,
        #[arg(long, default_value_t = 18_000)]
        duration_ms: u64,
        #[arg(long, default_value_t = 10)]
        iterations: u32,
    },
    Ios {
        workspace: PathBuf,
        flow_lock: PathBuf,
        framework: String,
        scenario: String,
        device: String,
        #[arg(long, default_value_t = 5_000)]
        duration_ms: u64,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Doctor { workspace } => {
            println!("{}", serde_json::to_string_pretty(&doctor(&workspace))?);
        }
        Command::Demo {
            workspace,
            flow_lock,
        } => {
            let flow: FlowLock = serde_json::from_slice(&tokio::fs::read(flow_lock).await?)?;
            let output = run_demo_suite(&workspace, &flow).await?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::Android {
            workspace,
            flow_lock,
            framework,
            scenario,
            device,
            duration_ms,
            iterations,
        } => {
            let output = run_android(&AndroidRunRequest {
                workspace,
                flow_lock,
                framework,
                scenario,
                device_id: device,
                duration_ms,
                iteration_count: iterations,
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        Command::Ios {
            workspace,
            flow_lock,
            framework,
            scenario,
            device,
            duration_ms,
        } => {
            let output = run_ios(&IosRunRequest {
                workspace,
                flow_lock,
                framework,
                scenario,
                device_id: device,
                duration_ms,
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
    }
    Ok(())
}
