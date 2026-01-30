//! Evaluation framework for testing agents
//!
//! This module provides functionality to run evaluations against agents:
//! - Load and execute evaluation test cases
//! - Run agents in Docker containers
//! - Judge response relevance using LLM-as-a-judge
//! - Compute and report scoring metrics

mod judge;
mod progress;
mod runner;
mod save;
mod scoring;
mod types;

pub use judge::Judge;
pub use progress::{is_tty, ProgressBar};
pub use runner::{run_evaluation, Runner};
pub use save::save_eval;
pub use scoring::{compute_summary, tool_call_f1_score};
pub use types::{Config, EvalCriteria, EvalRun, EvalSession, Result, Summary};
