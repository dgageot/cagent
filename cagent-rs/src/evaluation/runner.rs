//! Evaluation runner for executing test cases

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Utc;
use futures::stream::{self, StreamExt};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::judge::Judge;
use super::scoring::{compute_summary, count_handoffs, get_response_size, print_summary, tool_call_f1_score};
use super::types::{Config, EvalRun, EvalSession, Result as EvalResult, Summary};
use crate::model::Provider;

/// Evaluation runner
pub struct Runner {
    config: Config,
    agent_dir: String,
    agent_file: String,
    judge: Option<Judge>,
    image_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl Runner {
    /// Create a new evaluation runner
    pub fn new(
        config: Config,
        judge_model: Option<Arc<dyn Provider + Send + Sync>>,
    ) -> Result<Self> {
        let agent_path = Path::new(&config.agent_filename);
        let agent_dir = agent_path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        let agent_file = agent_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .ok_or_else(|| anyhow::anyhow!("Invalid agent filename"))?;

        let judge = judge_model.map(|m| Judge::new(m, config.concurrency.max(1)));

        Ok(Self {
            config,
            agent_dir,
            agent_file,
            judge,
            image_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Run all evaluations and return results
    pub async fn run<W: Write>(&self, out: &mut W) -> Result<Vec<EvalResult>> {
        writeln!(out, "Loading evaluation sessions...")?;
        let mut evals = self.load_eval_sessions().await?;

        // Sort by estimated duration (longest first) to avoid long tail
        evals.sort_by(|a, b| b.estimated_duration().cmp(&a.estimated_duration()));

        // Pre-build images
        self.pre_build_images(out, &evals).await?;

        let concurrency = if self.config.concurrency == 0 {
            num_cpus::get()
        } else {
            self.config.concurrency
        };

        writeln!(
            out,
            "Running {} evaluations with concurrency {}\n",
            evals.len(),
            concurrency
        )?;

        // Run evaluations concurrently
        let results: Vec<_> = stream::iter(evals.into_iter().enumerate())
            .map(|(idx, eval)| {
                let runner = self;
                async move {
                    let result = runner.run_single_eval(&eval).await;
                    (idx, result)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        // Sort results by original index
        let mut sorted_results: Vec<_> = results.into_iter().collect();
        sorted_results.sort_by_key(|(idx, _)| *idx);

        let final_results: Vec<EvalResult> = sorted_results
            .into_iter()
            .map(|(_, result)| match result {
                Ok(r) => r,
                Err(e) => {
                    error!("Evaluation failed: {}", e);
                    EvalResult {
                        error: Some(e.to_string()),
                        ..Default::default()
                    }
                }
            })
            .collect();

        // Print results
        for result in &final_results {
            let (successes, failures) = result.check_results();
            let status = if failures.is_empty() { "✅" } else { "❌" };
            writeln!(
                out,
                "{} {} - successes: {:?}, failures: {:?}",
                status, result.title, successes, failures
            )?;
        }

        Ok(final_results)
    }

    /// Load evaluation sessions from the evals directory
    async fn load_eval_sessions(&self) -> Result<Vec<EvalSession>> {
        let evals_dir = Path::new(&self.config.evals_dir);
        let mut sessions = Vec::new();

        let mut entries = tokio::fs::read_dir(evals_dir)
            .await
            .with_context(|| format!("Reading evals directory: {}", self.config.evals_dir))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let file_name = path.file_name().map(|f| f.to_string_lossy().to_string());

            if let Some(name) = file_name {
                // Filter by --only patterns
                if !self.config.only.is_empty()
                    && !matches_any_pattern(&name, &self.config.only)
                {
                    continue;
                }

                if !name.ends_with(".json") {
                    continue;
                }

                let content = tokio::fs::read_to_string(&path)
                    .await
                    .with_context(|| format!("Reading eval file: {}", path.display()))?;

                let mut session: EvalSession = serde_json::from_str(&content)
                    .with_context(|| format!("Parsing eval file: {}", path.display()))?;

                session.source_path = path.to_string_lossy().to_string();

                if session.title.is_empty() {
                    session.title = name.trim_end_matches(".json").to_string();
                }

                sessions.push(session);
            }
        }

        Ok(sessions)
    }

    /// Pre-build Docker images for all unique working directories
    async fn pre_build_images<W: Write>(&self, out: &mut W, evals: &[EvalSession]) -> Result<()> {
        let mut working_dirs = std::collections::HashSet::new();
        for eval in evals {
            if let Some(wd) = &eval.evals.working_dir {
                working_dirs.insert(wd.clone());
            } else {
                working_dirs.insert(String::new());
            }
        }

        if working_dirs.is_empty() {
            return Ok(());
        }

        writeln!(out, "Pre-building {} Docker image(s)...", working_dirs.len())?;

        for wd in working_dirs {
            self.get_or_build_image(&wd).await?;
        }

        Ok(())
    }

    /// Get or build a Docker image for the given working directory
    async fn get_or_build_image(&self, working_dir: &str) -> Result<String> {
        let mut cache = self.image_cache.lock().await;

        if let Some(image_id) = cache.get(working_dir) {
            return Ok(image_id.clone());
        }

        // Build the image
        let image_id = self.build_eval_image(working_dir).await?;
        cache.insert(working_dir.to_string(), image_id.clone());
        Ok(image_id)
    }

    /// Build a Docker image for evaluation
    async fn build_eval_image(&self, working_dir: &str) -> Result<String> {
        let image_name = format!("cagent-eval-{}", uuid::Uuid::new_v4());

        // Create Dockerfile content
        let base_image = self
            .config
            .base_image
            .as_deref()
            .unwrap_or("docker/cagent:latest");

        let dockerfile = if working_dir.is_empty() {
            format!("FROM {}\n", base_image)
        } else {
            format!(
                "FROM {}\nCOPY {} /workspace\nWORKDIR /workspace\n",
                base_image, working_dir
            )
        };

        debug!(image = %image_name, working_dir = %working_dir, "Building eval image");

        // Build the image using docker build
        let mut cmd = Command::new("docker");
        cmd.args(["build", "-t", &image_name, "-f", "-", "."]);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if !working_dir.is_empty() {
            let working_dir_path = Path::new(&self.config.evals_dir)
                .join("working_dirs")
                .join(working_dir);
            if working_dir_path.exists() {
                cmd.current_dir(working_dir_path);
            }
        }

        let mut child = cmd.spawn()?;

        // Write Dockerfile to stdin
        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = stdin;
            stdin.write_all(dockerfile.as_bytes()).await?;
        }

        let output = child.wait_with_output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to build Docker image: {}", stderr);
        }

        info!(image = %image_name, "Built eval image");
        Ok(image_name)
    }

    /// Run a single evaluation
    async fn run_single_eval(&self, eval: &EvalSession) -> Result<EvalResult> {
        let start_time = Instant::now();
        debug!(title = %eval.title, "Starting evaluation");

        let question = eval
            .get_first_user_message()
            .ok_or_else(|| anyhow::anyhow!("No user message found in evaluation"))?;

        let mut result = EvalResult {
            input_path: eval.source_path.clone(),
            title: eval.title.clone(),
            question: question.clone(),
            size_expected: eval.evals.size.clone().unwrap_or_default(),
            relevance_expected: eval.evals.relevance.len() as f64,
            ..Default::default()
        };

        let expected_tool_calls = eval.extract_tool_calls();
        if !expected_tool_calls.is_empty() {
            result.tool_calls_score_expected = 1.0;
        }

        // Get or build image
        let working_dir = eval.evals.working_dir.as_deref().unwrap_or("");
        let image_id = self.get_or_build_image(working_dir).await?;

        // Run cagent in container
        let events = self.run_cagent_in_container(&image_id, &question).await?;

        // Parse events
        let (response, cost, output_tokens, actual_tool_calls) = parse_container_events(&events);

        result.response = response;
        result.cost = cost;
        result.output_tokens = output_tokens;
        result.raw_output = events;
        result.size = get_response_size(&result.response);

        // Calculate tool call score
        if !expected_tool_calls.is_empty() || !actual_tool_calls.is_empty() {
            result.tool_calls_score = tool_call_f1_score(&expected_tool_calls, &actual_tool_calls);
        }

        // Check handoffs
        result.handoffs = count_handoffs(&expected_tool_calls) == count_handoffs(&actual_tool_calls);

        // Run relevance checks
        if let Some(judge) = &self.judge {
            if !eval.evals.relevance.is_empty() {
                let (passed, failed, errs) = judge
                    .check_relevance(&result.response, &eval.evals.relevance)
                    .await;

                result.relevance = passed as f64;
                result.failed_relevance = failed;

                for e in errs {
                    warn!(title = %eval.title, error = %e, "Relevance check error");
                }
            }
        }

        debug!(
            title = %eval.title,
            duration = ?start_time.elapsed(),
            "Evaluation complete"
        );

        Ok(result)
    }

    /// Run cagent in a Docker container and collect events
    async fn run_cagent_in_container(
        &self,
        image_id: &str,
        question: &str,
    ) -> Result<Vec<HashMap<String, serde_json::Value>>> {
        let container_name = format!("cagent-eval-{}", uuid::Uuid::new_v4());

        let mut args = vec![
            "run".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "--privileged".to_string(),
            "--init".to_string(),
        ];

        if !self.config.keep_containers {
            args.push("--rm".to_string());
        }

        args.extend(["-i".to_string()]);
        args.extend([
            "-v".to_string(),
            format!("{}:/configs:ro", self.agent_dir),
        ]);

        // Add API key environment variables
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GOOGLE_API_KEY",
            "MISTRAL_API_KEY",
            "XAI_API_KEY",
            "NEBIUS_API_KEY",
        ] {
            if std::env::var(name).is_ok() {
                args.extend(["-e".to_string(), name.to_string()]);
            }
        }

        // Add models gateway if set
        if let Ok(gateway) = std::env::var("CAGENT_MODELS_GATEWAY") {
            args.extend(["-e".to_string(), "CAGENT_MODELS_GATEWAY".to_string()]);
            if !gateway.is_empty() {
                if std::env::var("DOCKER_DESKTOP_TOKEN").is_ok() {
                    args.extend(["-e".to_string(), "DOCKER_DESKTOP_TOKEN".to_string()]);
                }
            }
        }

        args.push(image_id.to_string());
        args.push(format!("/configs/{}", self.agent_file));
        args.push(question.to_string());

        debug!(container = %container_name, "Running cagent in container");

        let mut cmd = Command::new("docker");
        cmd.args(&args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");

        // Read stderr in background
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = String::new();
            while reader.read_line(&mut buf).await.is_ok() && !buf.is_empty() {
                // Just collect, don't print
            }
            buf
        });

        // Read and parse stdout events
        let mut events = Vec::new();
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<HashMap<String, serde_json::Value>>(&line) {
                Ok(event) => events.push(event),
                Err(e) => {
                    debug!(line = %line, error = %e, "Failed to parse JSON event");
                }
            }
        }

        let status = child.wait().await?;
        let stderr_output = stderr_handle.await.unwrap_or_default();

        if events.is_empty() {
            let stderr_str = stderr_output.trim();
            if !status.success() {
                anyhow::bail!(
                    "Container failed with status {}: {}",
                    status,
                    stderr_str
                );
            }
            if !stderr_str.is_empty() {
                anyhow::bail!("No events received from container (stderr: {})", stderr_str);
            }
            anyhow::bail!("No events received from container");
        }

        Ok(events)
    }
}

/// Parse events from container output
fn parse_container_events(
    events: &[HashMap<String, serde_json::Value>],
) -> (String, f64, i64, Vec<String>) {
    let mut response = String::new();
    let mut cost = 0.0;
    let mut output_tokens = 0i64;
    let mut tool_calls = Vec::new();

    for event in events {
        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        match event_type {
            "agent_choice" => {
                if let Some(content) = event.get("content").and_then(|v| v.as_str()) {
                    response.push_str(content);
                }
            }
            "tool_call" => {
                if let Some(tc) = event.get("tool_call") {
                    if let Some(func) = tc.get("function") {
                        if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                            tool_calls.push(name.to_string());
                        }
                    }
                }
            }
            "token_usage" => {
                if let Some(usage) = event.get("usage") {
                    if let Some(c) = usage.get("cost").and_then(|v| v.as_f64()) {
                        cost = c;
                    }
                    if let Some(tokens) = usage.get("output_tokens").and_then(|v| v.as_f64()) {
                        output_tokens += tokens as i64;
                    }
                }
            }
            _ => {}
        }
    }

    (response, cost, output_tokens, tool_calls)
}

/// Check if a name matches any of the patterns (case-insensitive)
fn matches_any_pattern(name: &str, patterns: &[String]) -> bool {
    let name_lower = name.to_lowercase();
    patterns
        .iter()
        .any(|p| name_lower.contains(&p.to_lowercase()))
}

/// Run an evaluation and return the results
pub async fn run_evaluation<W: Write>(
    out: &mut W,
    run_name: &str,
    config: Config,
    judge_model: Option<Arc<dyn Provider + Send + Sync>>,
) -> Result<EvalRun> {
    let runner = Runner::new(config, judge_model)?;

    writeln!(out, "Evaluation run: {}", run_name)?;

    let start_time = Instant::now();
    let start_timestamp = Utc::now();

    let results = runner.run(out).await?;
    let duration = start_time.elapsed();

    let summary = compute_summary(&results);
    print_summary(out, &summary, duration);

    Ok(EvalRun {
        name: run_name.to_string(),
        timestamp: start_timestamp,
        duration,
        results,
        summary,
    })
}

/// Get the number of CPUs (for default concurrency)
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_any_pattern() {
        assert!(matches_any_pattern("test_file.json", &["test".to_string()]));
        assert!(matches_any_pattern("TEST_FILE.json", &["test".to_string()]));
        assert!(!matches_any_pattern("other_file.json", &["test".to_string()]));
        assert!(matches_any_pattern(
            "test_file.json",
            &["foo".to_string(), "test".to_string()]
        ));
    }

    #[test]
    fn test_parse_container_events() {
        let events = vec![
            {
                let mut m = HashMap::new();
                m.insert(
                    "type".to_string(),
                    serde_json::Value::String("agent_choice".to_string()),
                );
                m.insert(
                    "content".to_string(),
                    serde_json::Value::String("Hello".to_string()),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "type".to_string(),
                    serde_json::Value::String("agent_choice".to_string()),
                );
                m.insert(
                    "content".to_string(),
                    serde_json::Value::String(" World".to_string()),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "type".to_string(),
                    serde_json::Value::String("tool_call".to_string()),
                );
                m.insert(
                    "tool_call".to_string(),
                    serde_json::json!({
                        "function": {
                            "name": "search"
                        }
                    }),
                );
                m
            },
            {
                let mut m = HashMap::new();
                m.insert(
                    "type".to_string(),
                    serde_json::Value::String("token_usage".to_string()),
                );
                m.insert(
                    "usage".to_string(),
                    serde_json::json!({
                        "cost": 0.01,
                        "output_tokens": 100
                    }),
                );
                m
            },
        ];

        let (response, cost, tokens, tool_calls) = parse_container_events(&events);
        assert_eq!(response, "Hello World");
        assert!((cost - 0.01).abs() < f64::EPSILON);
        assert_eq!(tokens, 100);
        assert_eq!(tool_calls, vec!["search"]);
    }
}
