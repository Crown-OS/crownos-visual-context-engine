//! `context-eval generate`: a synthetic day as JSON, for replay and for tests.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use context_eval::{Scenario, generate};

#[derive(Debug, Parser)]
#[command(name = "context-eval", about, version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a synthetic trace.
    Generate {
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, value_enum, default_value_t = ScenarioArg::Workday)]
        scenario: ScenarioArg,
        /// Where to write; stdout when absent.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScenarioArg {
    Workday,
    Short,
}

impl From<ScenarioArg> for Scenario {
    fn from(scenario: ScenarioArg) -> Self {
        match scenario {
            ScenarioArg::Workday => Self::Workday,
            ScenarioArg::Short => Self::Short,
        }
    }
}

fn main() -> Result<()> {
    let Command::Generate {
        seed,
        scenario,
        out,
    } = Cli::parse().command;
    let trace = generate(scenario.into(), seed);
    let json = trace.to_json().context("rendering the trace")?;
    match out {
        Some(path) => {
            std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
        }
        None => {
            println!("{json}");
            Ok(())
        }
    }
}
