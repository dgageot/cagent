//! CLI interface using clap

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::debug;

use crate::agent::{AgentBuilder, Team};
use crate::config::{parse_model_ref, Config};
use crate::model::{create_provider_from_parts, ProviderOptions};
use crate::runtime::{Event, LocalRuntime, ResumeType, RuntimeConfig};
use crate::session::Session;
use crate::tools::builtin::*;
use crate::tools::mcp::McpToolset;
use crate::tools::mcp::{RemoteMcpToolset, TransportType};
use crate::tools::{DescriptionToolSet, FilteredToolSet, ToolSet, ToolSetWithInstruction};
use crate::user_config::{default_config_path, expand_tilde, UserConfigStore};

use crate::logging::{init_tracing, shutdown_tracing};

#[derive(Parser, Debug)]
#[command(name = "cagent", version, about = "AI agent runner")]
pub struct Cli {
    /// Enable debug logging (writes to ~/.cagent/cagent.debug.log by default)
    #[arg(short, long, global = true)]
    pub debug: bool,

    /// Custom debug log file path (implies --debug)
    #[arg(long, global = true)]
    pub log_file: Option<PathBuf>,

    /// Enable OpenTelemetry tracing (export via OTEL_* env vars)
    #[arg(short = 'o', long, global = true)]
    pub otel: bool,

    /// Set the models gateway address (overrides CAGENT_MODELS_GATEWAY env var)
    #[arg(long, global = true, env = "CAGENT_MODELS_GATEWAY")]
    pub models_gateway: Option<String>,

    /// Set environment variables from file(s)
    #[arg(long = "env-from-file", global = true)]
    pub env_files: Vec<PathBuf>,

    /// Provide a single tool to call other tools via code execution
    #[arg(long, global = true)]
    pub code_mode_tools: bool,

    /// Set the working directory for the session (applies to tools and relative paths)
    #[arg(long, global = true)]
    pub working_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run an agent interactively
    Run {
        #[arg(default_value = "")]
        config: String,
        #[arg(default_value = "")]
        message: String,
        #[arg(short, long, default_value = "root")]
        agent: String,
        #[arg(long)]
        yolo: bool,
        #[arg(long)]
        model: Option<String>,
        /// Path to a cassette file for fake/replay mode
        #[arg(long)]
        fake: Option<PathBuf>,
        /// Delay in milliseconds between SSE chunks when using --fake
        #[arg(long, default_value = "15")]
        stream_delay: u64,
        #[cfg(feature = "tui")]
        #[arg(long)]
        no_tui: bool,
    },
    /// Execute an agent non-interactively
    Exec {
        config: String,
        message: String,
        #[arg(short, long, default_value = "root")]
        agent: String,
        #[arg(long)]
        yolo: bool,
    },
    /// Create a new agent configuration
    New {
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "openai/gpt-4o")]
        model: String,
    },
    /// Push an agent to an OCI registry
    Push {
        /// Path to the agent configuration file
        agent_file: PathBuf,
        /// OCI registry reference (e.g., docker.io/namespace/agent:tag)
        registry_ref: String,
    },
    /// Pull an agent from an OCI registry
    Pull {
        /// OCI registry reference (e.g., docker.io/namespace/agent:tag)
        registry_ref: String,
        /// Output filename (defaults to agent name + .yaml)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Force pull even if the configuration already exists locally
        #[arg(short, long)]
        force: bool,
    },
    /// Start an agent as an MCP (Model Context Protocol) server
    Mcp {
        /// Path to the agent configuration file
        config: String,
        /// Name of the agent to run (all agents if not specified)
        #[arg(short, long)]
        agent: Option<String>,
        /// Use streaming HTTP transport instead of stdio
        #[arg(long)]
        http: bool,
        /// Port to listen on when using HTTP transport (default: random available port)
        #[arg(short, long, default_value = "0")]
        port: u16,
    },
    /// Start an agent as an A2A (Agent-to-Agent) server
    A2a {
        /// Path to the agent configuration file
        config: String,
        /// Name of the agent to run
        #[arg(short, long, default_value = "root")]
        agent: String,
        /// Port to listen on (default: random available port)
        #[arg(short, long, default_value = "0")]
        port: u16,
    },
    /// Start the cagent API server
    Api {
        /// Path to the agent configuration file or directory
        config: String,
        /// Address to listen on
        #[arg(short, long, default_value = ":8080")]
        listen: String,
        /// Path to the session database
        #[arg(short, long, default_value = "session.db")]
        session_db: PathBuf,
    },
    /// Run evaluations for an agent
    Eval {
        /// Path to the agent configuration file
        config: String,
        /// Directory containing evaluation tests (default: ./evals)
        #[arg(default_value = "./evals")]
        evals_dir: PathBuf,
        /// Number of concurrent evaluation runs
        #[arg(short, long, default_value = "4")]
        concurrency: usize,
        /// Model to use for relevance checking
        #[arg(long, default_value = "anthropic/claude-opus-4-5-20251101")]
        judge_model: String,
        /// Directory for results and logs
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Manage the agent catalog
    Catalog {
        #[command(subcommand)]
        action: CatalogAction,
    },
    /// Generate shell completions
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Manage user configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Manage agent aliases
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },
    /// Build an agent image for deployment
    Build {
        /// Path to the agent configuration file
        config: String,
        /// Output image tag (e.g., my-agent:latest)
        #[arg(short, long)]
        tag: Option<String>,
        /// Push to registry after building
        #[arg(long)]
        push: bool,
        /// Base image to use (default: docker/cagent:latest)
        #[arg(long, default_value = "docker/cagent:latest")]
        base: String,
    },
    /// Record AI API interactions for testing/replay
    Record {
        /// Path to the agent configuration file
        config: String,
        /// Output directory for recordings (cassettes)
        #[arg(short, long, default_value = "./cassettes")]
        output: PathBuf,
        /// Name for this recording session
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Send feedback to the cagent team
    Feedback {
        /// Feedback message (or - to read from stdin)
        message: String,
        /// Include debug logs with feedback
        #[arg(long)]
        include_logs: bool,
    },
    /// Start an Agent Client Protocol (ACP) server
    Acp {
        /// Path to the agent configuration file
        config: String,
        /// Port to listen on (default: random available port)
        #[arg(short, long, default_value = "0")]
        port: u16,
    },
    /// Show version
    Version,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Show the path to the config file
    Path,
    /// Show the current configuration
    Show,
}

#[derive(Subcommand, Debug)]
pub enum AliasAction {
    /// Add a new alias: alias add <name> <agent-path>
    Add {
        name: String,
        path: String,
        /// Automatically approve all tool calls without prompting
        #[arg(long)]
        yolo: bool,
        /// Override agent model (format: [agent=]provider/model)
        #[arg(long)]
        model: Option<String>,
        /// Hide tool call results in the TUI
        #[arg(long)]
        hide_tool_results: bool,
    },
    /// List all aliases
    List,
    /// Remove an alias
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum CatalogAction {
    /// List catalog entries
    List {
        /// Organization/namespace to list (default: agentcatalog)
        #[arg(default_value = "agentcatalog")]
        org: String,
    },
}

impl Cli {
    pub async fn run(self) -> anyhow::Result<()> {
        init_tracing(
            self.debug || self.log_file.is_some(),
            self.log_file.as_deref(),
            self.otel,
        )?;

        // Apply runtime config flags
        apply_runtime_config(
            self.models_gateway.as_deref(),
            &self.env_files,
            self.working_dir.as_deref(),
        )?;

        let result = match self.command {
            #[cfg(feature = "tui")]
            Commands::Run {
                config,
                message,
                agent,
                yolo,
                model,
                fake,
                stream_delay,
                no_tui,
            } => {
                let (config, yolo, model, hide_tool_results) =
                    resolve_run_target(&config, yolo, model.as_deref())?;

                // Set up fake proxy if specified
                let _proxy = if let Some(cassette_path) = &fake {
                    let options = crate::fake::ProxyOptionsBuilder::new()
                        .simulate_stream(true)
                        .stream_chunk_delay(std::time::Duration::from_millis(stream_delay))
                        .build();
                    let proxy = crate::fake::start_replay_proxy(cassette_path, options).await?;
                    println!("\n🎬 Replay mode: using cassette {}", cassette_path.display());
                    println!("   Proxy: {}\n", proxy.gateway_url());
                    
                    // Update environment with proxy URL
                    std::env::set_var("CAGENT_MODELS_GATEWAY", proxy.gateway_url());
                    Some(proxy)
                } else {
                    None
                };

                if !no_tui && message.is_empty() {
                    return run_tui(&config, yolo, model.as_deref(), hide_tool_results).await;
                }
                run_cli(
                    &config,
                    &message,
                    &agent,
                    yolo,
                    model.as_deref(),
                    hide_tool_results,
                )
                .await
            }
            #[cfg(not(feature = "tui"))]
            Commands::Run {
                config,
                message,
                agent,
                yolo,
                model,
                fake,
                stream_delay,
            } => {
                let (config, yolo, model, hide_tool_results) =
                    resolve_run_target(&config, yolo, model.as_deref())?;
                
                // Set up fake proxy if specified
                let _proxy = if let Some(cassette_path) = &fake {
                    let options = crate::fake::ProxyOptionsBuilder::new()
                        .simulate_stream(true)
                        .stream_chunk_delay(std::time::Duration::from_millis(stream_delay))
                        .build();
                    let proxy = crate::fake::start_replay_proxy(cassette_path, options).await?;
                    std::env::set_var("CAGENT_MODELS_GATEWAY", proxy.gateway_url());
                    Some(proxy)
                } else {
                    None
                };

                run_cli(
                    &config,
                    &message,
                    &agent,
                    yolo,
                    model.as_deref(),
                    hide_tool_results,
                )
                .await
            }
            Commands::Exec {
                config,
                message,
                agent,
                yolo,
            } => {
                let (config, yolo, model, hide_tool_results) =
                    resolve_run_target(&config, yolo, None)?;
                exec(
                    &config,
                    &message,
                    &agent,
                    yolo,
                    model.as_deref(),
                    hide_tool_results,
                )
                .await
            }
            Commands::New { output, model } => new_config(output, &model),
            Commands::Push {
                agent_file,
                registry_ref,
            } => push_agent(&agent_file, &registry_ref).await,
            Commands::Pull {
                registry_ref,
                output,
                force,
            } => pull_agent(&registry_ref, output, force).await,
            Commands::Mcp {
                config,
                agent,
                http,
                port,
            } => mcp_server(&config, agent.as_deref(), http, port).await,
            Commands::A2a {
                config,
                agent,
                port,
            } => a2a_server(&config, &agent, port).await,
            Commands::Api {
                config,
                listen,
                session_db,
            } => api_server(&config, &listen, &session_db).await,
            Commands::Eval {
                config,
                evals_dir,
                concurrency,
                judge_model,
                output,
            } => eval_agent(&config, &evals_dir, concurrency, &judge_model, output).await,
            Commands::Catalog { action } => catalog_command(action).await,
            Commands::Completion { shell } => {
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "cagent", &mut std::io::stdout());
                Ok(())
            }
            Commands::Config { action } => config_command(action),
            Commands::Alias { action } => alias_command(action),
            Commands::Build {
                config,
                tag,
                push,
                base,
            } => build_agent(&config, tag.as_deref(), push, &base).await,
            Commands::Record {
                config,
                output,
                name,
            } => record_interactions(&config, &output, name.as_deref()).await,
            Commands::Feedback {
                message,
                include_logs,
            } => send_feedback(&message, include_logs).await,
            Commands::Acp { config, port } => acp_server(&config, port).await,
            Commands::Version => {
                println!("cagent {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        };

        if self.otel {
            // Ensure spans are flushed.
            shutdown_tracing();
        }

        result
    }
}

#[cfg(feature = "tui")]
async fn run_tui(
    config_path: &str,
    yolo: bool,
    model_override: Option<&str>,
    hide_tool_results: bool,
) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let team = build_team(&config, model_override)?;
    crate::tui::run_tui(team, yolo, hide_tool_results).await
}

async fn run_cli(
    config_path: &str,
    initial_message: &str,
    agent_name: &str,
    yolo: bool,
    model_override: Option<&str>,
    hide_tool_results: bool,
) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let team = build_team(&config, model_override)?;
    let runtime = LocalRuntime::new(team, RuntimeConfig::default())?;
    runtime.set_current_agent(agent_name).await?;

    let working_dir = std::env::current_dir()?.to_string_lossy().to_string();
    let mut session = Session::new()
        .with_working_dir(&working_dir)
        .with_tools_approved(yolo);

    session.hide_tool_results = hide_tool_results;

    if !initial_message.is_empty() {
        session = session.with_user_message(initial_message);
    }

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        // Prompt if needed
        if needs_input(&session) {
            print!("\n> ");
            std::io::Write::flush(&mut std::io::stdout())?;

            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                break;
            }

            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            if let Some(handled) = handle_command(input, &mut session, &working_dir, yolo)? {
                if handled {
                    continue;
                } else {
                    break;
                }
            }

            session = session.with_user_message(input);
        }

        // Run agent
        let mut events = runtime.run_stream(&mut session).await;
        while let Some(event) = events.recv().await {
            print_event(&event);

            if let Event::ToolCallConfirmation { tool, .. } = &event {
                print!("\nApprove '{}' (y/n/a)? ", tool.name);
                std::io::Write::flush(&mut std::io::stdout())?;

                line.clear();
                reader.read_line(&mut line).await?;

                let resume = match line.trim().to_lowercase().as_str() {
                    "y" | "yes" => ResumeType::Approve,
                    "a" | "all" => ResumeType::ApproveSession,
                    _ => ResumeType::Reject {
                        reason: Some("Declined".into()),
                    },
                };
                runtime.resume(resume).await;
            }
        }
    }

    println!("\nGoodbye!");
    Ok(())
}

async fn exec(
    config_path: &str,
    message: &str,
    agent_name: &str,
    yolo: bool,
    model_override: Option<&str>,
    hide_tool_results: bool,
) -> anyhow::Result<()> {
    let config = load_config(config_path)?;
    let team = build_team(&config, model_override)?;
    let runtime = LocalRuntime::new(team, RuntimeConfig::default())?;
    runtime.set_current_agent(agent_name).await?;

    let mut session = Session::new()
        .with_working_dir(std::env::current_dir()?.to_string_lossy())
        .with_tools_approved(yolo)
        .with_user_message(message);

    session.hide_tool_results = hide_tool_results;

    for event in runtime.run(&mut session).await? {
        if let Event::AgentChoice { content, .. } = event {
            print!("{}", content);
        }
    }
    println!();
    Ok(())
}

fn new_config(output: Option<PathBuf>, model: &str) -> anyhow::Result<()> {
    let yaml = format!(
        r#"version: "3"

agents:
  root:
    model: {model}
    description: A helpful AI assistant
    instruction: |
      You are a helpful AI assistant.
    add_date: true
    add_environment_info: true
    toolsets:
      - type: filesystem
      - type: shell
      - type: think
"#
    );

    if let Some(path) = output {
        std::fs::write(&path, &yaml)?;
        println!("Created: {}", path.display());
    } else {
        println!("{}", yaml);
    }
    Ok(())
}

/// Push an agent configuration to an OCI registry
async fn push_agent(agent_file: &Path, registry_ref: &str) -> anyhow::Result<()> {
    if !agent_file.exists() {
        anyhow::bail!("Agent file not found: {}", agent_file.display());
    }

    println!("Pushing agent {} to {}", agent_file.display(), registry_ref);

    // Read and validate the agent file
    let content = std::fs::read_to_string(agent_file)?;
    let _config: crate::config::Config = serde_yaml::from_str(&content)?;

    // Push to OCI registry
    let digest = crate::oci::push_agent(agent_file, registry_ref).await?;
    
    println!("\n✅ Successfully pushed agent to {}", registry_ref);
    println!("   Digest: {}", digest);

    Ok(())
}

/// Pull an agent configuration from an OCI registry
async fn pull_agent(
    registry_ref: &str,
    output: Option<PathBuf>,
    force: bool,
) -> anyhow::Result<()> {
    let output_file = output.unwrap_or_else(|| {
        // Generate filename from reference (namespace_repo_tag.yaml)
        let name = registry_ref
            .replace(['/', ':'], "_")
            .trim_matches('_')
            .to_string();
        PathBuf::from(format!("{}.yaml", name))
    });

    if output_file.exists() && !force {
        anyhow::bail!(
            "Output file already exists: {}\nUse --force to overwrite",
            output_file.display()
        );
    }

    println!("Pulling agent from {}", registry_ref);

    // Pull from OCI registry
    let content = crate::oci::pull_agent(registry_ref).await?;

    // Validate it's a valid agent config
    let _config: crate::config::Config = serde_yaml::from_str(&content)?;

    // Write to output file
    std::fs::write(&output_file, &content)?;

    println!("\n✅ Successfully pulled agent to {}", output_file.display());

    Ok(())
}

/// Start an agent as an MCP server
async fn mcp_server(
    config_path: &str,
    agent: Option<&str>,
    http: bool,
    port: u16,
) -> anyhow::Result<()> {
    use crate::mcp_server::{start_mcp_server_http, start_mcp_server_stdio};

    let config = load_config(config_path)?;
    let team = Arc::new(build_team(&config, None)?);
    let agent_name = agent.map(|s| s.to_string());

    if http {
        // Use HTTP transport
        let actual_port = if port == 0 { 8080 } else { port };
        println!("Starting MCP server on http://127.0.0.1:{}", actual_port);
        start_mcp_server_http(team, agent_name, "127.0.0.1", actual_port)
            .await
            .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;
    } else {
        // Use stdio transport (default)
        start_mcp_server_stdio(team, agent_name)
            .await
            .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;
    }

    Ok(())
}

/// Start an agent as an A2A server
async fn a2a_server(config_path: &str, agent: &str, port: u16) -> anyhow::Result<()> {
    use crate::a2a_server::start_a2a_server;

    let config = load_config(config_path)?;
    let team = Arc::new(build_team(&config, None)?);
    let agent_name = agent.to_string();

    start_a2a_server(team, agent_name, port).await
}

/// Start the cagent API server
async fn api_server(config_path: &str, listen: &str, session_db: &Path) -> anyhow::Result<()> {
    use crate::api::ApiServerBuilder;
    use std::collections::HashMap;

    let config = load_config(config_path)?;

    // Create a map of configs with the filename as key
    let mut configs = HashMap::new();
    let config_name = std::path::Path::new(config_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("default")
        .to_string();
    configs.insert(config_name, config);

    // Normalize listen address
    let addr = if listen.starts_with(':') {
        format!("0.0.0.0{}", listen)
    } else {
        listen.to_string()
    };

    println!("Starting API server...");

    ApiServerBuilder::new()
        .with_sqlite_store(session_db)?
        .with_configs(configs)
        .with_runtime_config(crate::runtime::RuntimeConfig::default())
        .with_addr(&addr)
        .serve()
        .await
}

/// Run evaluations for an agent
async fn eval_agent(
    config_path: &str,
    evals_dir: &Path,
    concurrency: usize,
    judge_model: &str,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    use crate::evaluation::{run_evaluation, Config as EvalConfig};

    // Validate config exists
    let _config = load_config(config_path)?;

    // Create the judge model provider if specified
    let judge_provider: Option<Arc<dyn crate::model::Provider + Send + Sync>> = if !judge_model.is_empty() {
        let (provider_name, model_name) = parse_model_ref(judge_model)
            .ok_or_else(|| anyhow::anyhow!("Invalid judge model format: {}", judge_model))?;
        let provider = create_provider_from_parts(provider_name, model_name, Default::default(), None)?;
        Some(provider)
    } else {
        None
    };

    // Generate a run name with timestamp
    let run_name = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();

    let eval_config = EvalConfig {
        agent_filename: config_path.to_string(),
        evals_dir: evals_dir.to_string_lossy().to_string(),
        judge_model: if judge_model.is_empty() {
            None
        } else {
            Some(judge_model.to_string())
        },
        concurrency,
        only: vec![],
        base_image: None,
        keep_containers: false,
    };

    let mut stdout = std::io::stdout();
    let run = run_evaluation(&mut stdout, &run_name, eval_config, judge_provider).await?;

    // Save results if output directory specified
    if let Some(output_dir) = output {
        std::fs::create_dir_all(&output_dir)?;
        let results_file = output_dir.join(format!("{}.json", run_name));
        let json = serde_json::to_string_pretty(&run)?;
        std::fs::write(&results_file, json)?;
        println!("\nResults saved to: {}", results_file.display());
    }

    // Exit with error code if any evaluations failed
    if run.summary.failed_evals > 0 {
        std::process::exit(1);
    }

    Ok(())
}

/// Build an agent image for deployment
async fn build_agent(
    config_path: &str,
    tag: Option<&str>,
    push: bool,
    base: &str,
) -> anyhow::Result<()> {
    // Validate the agent config exists and is valid
    let _config = load_config(config_path)?;
    let agent_path = std::path::Path::new(config_path);
    let agent_dir = agent_path.parent().unwrap_or(std::path::Path::new("."));
    let agent_file = agent_path.file_name().unwrap_or_default().to_string_lossy();

    let image_tag = tag.unwrap_or("cagent-agent:latest");

    println!("Building agent image from {}...", config_path);

    // Create Dockerfile content
    let dockerfile = format!(
        r#"# syntax=docker/dockerfile:1
FROM {base}
COPY {agent_file} /agent.yaml
ENTRYPOINT ["/cagent", "exec", "/agent.yaml"]
"#,
        base = base,
        agent_file = agent_file
    );

    // Build using docker build
    let mut cmd = tokio::process::Command::new("docker");
    cmd.args(["build", "-t", image_tag, "-f", "-", "."]);
    cmd.current_dir(agent_dir);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

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

    println!("\n✅ Built image: {}", image_tag);

    // Push if requested
    if push {
        println!("Pushing image to registry...");

        let push_output = tokio::process::Command::new("docker")
            .args(["push", image_tag])
            .output()
            .await?;

        if !push_output.status.success() {
            let stderr = String::from_utf8_lossy(&push_output.stderr);
            anyhow::bail!("Failed to push image: {}", stderr);
        }

        println!("✅ Pushed image: {}", image_tag);
    }

    Ok(())
}

/// Record AI API interactions for testing/replay
async fn record_interactions(
    config_path: &str,
    output: &Path,
    name: Option<&str>,
) -> anyhow::Result<()> {
    use crate::fake;

    // Ensure output directory exists
    std::fs::create_dir_all(output)?;

    // Generate cassette filename
    let session_name = name.unwrap_or("recording");
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let cassette_filename = format!("{}_{}.yaml", session_name, timestamp);
    let cassette_path = output.join(&cassette_filename);

    println!("🎬 Starting recording mode");
    println!("   Agent config: {}", config_path);
    println!("   Cassette: {}\n", cassette_path.display());

    // Start the recording proxy
    let proxy = fake::start_recording_proxy(&cassette_path).await?;
    let gateway_url = proxy.gateway_url().to_string();

    println!("📡 Recording proxy started at {}", gateway_url);
    println!("   Press Ctrl+C to stop recording and save the cassette.\n");

    // Set the gateway URL as environment variable so the model providers use it
    std::env::set_var("CAGENT_MODELS_GATEWAY", &gateway_url);

    // Run the TUI with the proxy active
    let config = load_config(config_path)?;
    let team = build_team(&config, None)?;
    let result = crate::tui::run_tui(team, false, false).await;

    // Stop the proxy and save the cassette
    println!("\n💾 Saving cassette...");
    proxy.stop().await?;
    println!("\n✅ Recording saved to: {}", cassette_path.display());
    println!("\nTo replay this recording, use:");
    println!("   cagent run {} --fake {}", config_path, cassette_path.display());

    result
}

/// Send feedback to the cagent team
async fn send_feedback(message: &str, include_logs: bool) -> anyhow::Result<()> {
    // Read message from stdin if "-" is provided
    let feedback_message = if message == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        message.to_string()
    };

    if feedback_message.trim().is_empty() {
        anyhow::bail!("Feedback message cannot be empty");
    }

    let mut feedback_content = feedback_message.clone();

    // Include debug logs if requested
    if include_logs {
        let log_path = crate::paths::get_debug_log_path();
        if log_path.exists() {
            if let Ok(logs) = std::fs::read_to_string(&log_path) {
                // Only include last 1000 lines
                let lines: Vec<&str> = logs.lines().collect();
                let start = lines.len().saturating_sub(1000);
                let recent_logs: String = lines[start..].join("\n");
                feedback_content.push_str("\n\n--- Debug Logs ---\n");
                feedback_content.push_str(&recent_logs);
            }
        }
    }

    // Create a GitHub issue URL with the feedback pre-filled
    let title_str = "User Feedback";
    let body_str = format!(
        "## Feedback\n\n{}\n\n---\n*cagent version: {}*",
        feedback_message,
        env!("CARGO_PKG_VERSION")
    );
    let encoded_title = urlencoding::encode(title_str);
    let encoded_body = urlencoding::encode(&body_str);

    let url = format!(
        "https://github.com/docker/cagent/issues/new?title={}&body={}",
        encoded_title, encoded_body
    );

    println!("Thank you for your feedback!\n");
    println!("Please submit your feedback at:\n{}", url);

    // Try to open the URL in the default browser
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();

    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(&url).spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", &url])
        .spawn();

    Ok(())
}

/// Start an Agent Client Protocol (ACP) server
async fn acp_server(config_path: &str, port: u16) -> anyhow::Result<()> {
    // TODO: Implement ACP server
    // This requires:
    // 1. Loading the agent configuration
    // 2. Starting an ACP server
    // 3. Exposing the agent via Agent Client Protocol

    let _config = load_config(config_path)?;

    let port_display = if port == 0 {
        "random available port".to_string()
    } else {
        format!("port {}", port)
    };

    println!(
        "\n🚧 ACP server not yet implemented.\n\n\
         This command will:\n\
         1. Start an ACP server on {}\n\
         2. Expose the agent via Agent Client Protocol\n\n\
         To use this feature, the ACP server module needs to be implemented.",
        port_display
    );

    Ok(())
}

/// Manage the agent catalog
async fn catalog_command(action: CatalogAction) -> anyhow::Result<()> {
    match action {
        CatalogAction::List { org } => {
            println!("Fetching catalog from {}...", org);

            // Fetch from Docker Hub
            let client = reqwest::Client::new();
            let url = format!(
                "https://hub.docker.com/v2/repositories/{}/?page_size=100",
                org
            );

            let response = client
                .get(&url)
                .header("Accept", "application/json")
                .send()
                .await?;

            if !response.status().is_success() {
                anyhow::bail!("Failed to fetch catalog: {}", response.status());
            }

            #[derive(serde::Deserialize)]
            struct HubRepo {
                name: String,
                #[serde(default)]
                description: String,
            }

            #[derive(serde::Deserialize)]
            struct HubRepoList {
                results: Vec<HubRepo>,
            }

            let list: HubRepoList = response.json().await?;

            println!("\n{:<30} DESCRIPTION", "NAME");
            println!("{}", "-".repeat(80));

            for repo in list.results {
                let desc = repo.description.replace(['\n', '\t'], " ");
                let desc = if desc.len() > 50 {
                    format!("{}...", &desc[..47])
                } else {
                    desc
                };
                println!("{:<30} {}", format!("{}/{}", org, repo.name), desc);
            }

            Ok(())
        }
    }
}

fn config_command(action: Option<ConfigAction>) -> anyhow::Result<()> {
    let config_path = default_config_path()?;

    match action {
        None | Some(ConfigAction::Show) => {
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                println!("{}", content);
            } else {
                println!("# No user configuration found at {}", config_path.display());
                println!("# Default configuration:");
                println!(
                    r#"version: v1
# models_gateway: https://models.example.com
# aliases:
#   my-agent:
#     path: ./my-agent.yaml
#     yolo: false
# settings:
#   theme: dark
#   hide_tool_results: false
"#
                );
            }
        }
        Some(ConfigAction::Path) => {
            println!("{}", config_path.display());
        }
    }
    Ok(())
}

fn alias_command(action: AliasAction) -> anyhow::Result<()> {
    let store = UserConfigStore::new_default()?;

    match action {
        AliasAction::Add {
            name,
            path,
            yolo,
            model,
            hide_tool_results,
        } => {
            let mut cfg = store.load()?;
            let path = expand_tilde(&path)?;
            cfg.aliases.insert(
                name.clone(),
                crate::user_config::Alias {
                    path: path.clone(),
                    yolo,
                    model,
                    hide_tool_results,
                },
            );
            store.save(&cfg)?;

            println!("Alias '{}' created successfully", name);
            println!("  Alias: {}", name);
            println!("  Agent: {}", path);
            Ok(())
        }
        AliasAction::List => {
            let cfg = store.load()?;
            if cfg.aliases.is_empty() {
                println!("No aliases registered.");
                println!("\nCreate an alias with: cagent alias add <name> <agent-path>");
                return Ok(());
            }

            let mut names: Vec<_> = cfg.aliases.keys().cloned().collect();
            names.sort();
            println!("Registered aliases ({}):\n", names.len());
            for name in names {
                let a = &cfg.aliases[&name];
                let mut opts = Vec::new();
                if a.yolo {
                    opts.push("yolo".to_string());
                }
                if let Some(m) = a.model.as_deref().filter(|s| !s.is_empty()) {
                    opts.push(format!("model={}", m));
                }
                if a.hide_tool_results {
                    opts.push("hide-tool-results".to_string());
                }

                if opts.is_empty() {
                    println!("  {} → {}", name, a.path);
                } else {
                    println!("  {} → {} [{}]", name, a.path, opts.join(", "));
                }
            }
            println!("\nRun an alias with: cagent run <alias>");
            Ok(())
        }
        AliasAction::Remove { name } => {
            let mut cfg = store.load()?;
            if cfg.aliases.remove(&name).is_none() {
                anyhow::bail!("alias '{}' not found", name);
            }
            store.save(&cfg)?;
            println!("Alias '{}' removed successfully", name);
            Ok(())
        }
    }
}

/// Resolve a run target (config path or alias).
///
/// Returns: (resolved_config_path, yolo, model_override, hide_tool_results)
fn resolve_run_target(
    config_or_alias: &str,
    cli_yolo: bool,
    cli_model: Option<&str>,
) -> anyhow::Result<(String, bool, Option<String>, bool)> {
    // If a path exists, treat it as a config file/dir.
    if !config_or_alias.is_empty() {
        let p = Path::new(config_or_alias);
        if p.exists() {
            return Ok((
                config_or_alias.to_string(),
                cli_yolo,
                cli_model.map(|s| s.to_string()),
                false,
            ));
        }
    }

    // Otherwise, try aliases. Empty means "default" alias if present.
    let alias_name = if config_or_alias.is_empty() {
        "default"
    } else {
        config_or_alias
    };

    let store = UserConfigStore::new_default()?;
    let cfg = store.load()?;
    if let Some(alias) = cfg.aliases.get(alias_name) {
        let yolo = cli_yolo || alias.yolo;
        let model = cli_model
            .map(|s| s.to_string())
            .or_else(|| alias.model.clone());
        let hide_tool_results = alias.hide_tool_results || cfg.settings.hide_tool_results;
        return Ok((alias.path.clone(), yolo, model, hide_tool_results));
    }

    // Fallback: empty config => default agent.
    if config_or_alias.is_empty() {
        return Ok((
            String::new(),
            cli_yolo,
            cli_model.map(|s| s.to_string()),
            false,
        ));
    }

    Ok((
        config_or_alias.to_string(),
        cli_yolo,
        cli_model.map(|s| s.to_string()),
        false,
    ))
}

/// Apply runtime configuration from CLI flags.
/// Handles models gateway, environment files, and working directory.
fn apply_runtime_config(
    models_gateway: Option<&str>,
    env_files: &[PathBuf],
    working_dir: Option<&Path>,
) -> anyhow::Result<()> {
    // Set models gateway environment variable if provided via CLI
    // (the gateway module reads from CAGENT_MODELS_GATEWAY)
    if let Some(gateway) = models_gateway {
        let gateway = gateway.trim().trim_end_matches('/');
        if !gateway.is_empty() {
            std::env::set_var(crate::gateway::CAGENT_MODELS_GATEWAY_ENV, gateway);
            debug!("Models gateway set to: {}", gateway);
        }
    }

    // Load environment variables from files
    for env_file in env_files {
        if !env_file.exists() {
            anyhow::bail!("Environment file not found: {}", env_file.display());
        }
        let content = std::fs::read_to_string(env_file)?;
        for line in content.lines() {
            let line = line.trim();
            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Parse KEY=VALUE
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"').trim_matches('\'');
                std::env::set_var(key, value);
                debug!("Set env var from file: {}={}", key, value);
            }
        }
    }

    // Change working directory if specified
    if let Some(wd) = working_dir {
        let abs_wd = std::fs::canonicalize(wd)
            .map_err(|_| anyhow::anyhow!("Invalid working directory: {}", wd.display()))?;
        if !abs_wd.is_dir() {
            anyhow::bail!(
                "Working directory does not exist or is not a directory: {}",
                abs_wd.display()
            );
        }
        std::env::set_current_dir(&abs_wd)?;
        std::env::set_var("PWD", &abs_wd);
        debug!("Working directory set to: {}", abs_wd.display());
    }

    Ok(())
}

/// Check if a string looks like an OCI reference (e.g., agentcatalog/pirate, docker.io/foo/bar:tag)
/// This mimics the Go behavior using container registry reference parsing rules.
fn is_oci_reference(input: &str) -> bool {
    // Skip if it looks like a local file (has .yaml/.yml extension)
    let ext = std::path::Path::new(input)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext == "yaml" || ext == "yml" {
        return false;
    }

    // Skip /dev/fd/ file descriptors
    if input.starts_with("/dev/fd/") {
        return false;
    }

    // An OCI reference typically looks like:
    // - namespace/repo (e.g., agentcatalog/pirate)
    // - registry/namespace/repo (e.g., docker.io/agentcatalog/pirate)
    // - With optional tag (e.g., namespace/repo:tag)
    // - With optional digest (e.g., namespace/repo@sha256:...)

    // Must contain a '/' to be a valid reference
    if !input.contains('/') {
        return false;
    }

    // Simple validation: doesn't start with . or / (those are paths)
    if input.starts_with('.') || input.starts_with('/') {
        return false;
    }

    // Looks like an OCI reference
    true
}

/// Load configuration from an OCI registry reference.
fn load_oci_config(reference: &str) -> anyhow::Result<Config> {
    // Pull from OCI registry synchronously
    let rt = tokio::runtime::Handle::try_current()
        .map(|h| h.block_on(crate::oci::pull_agent(reference)))
        .unwrap_or_else(|_| {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(crate::oci::pull_agent(reference))
        })?;

    // Parse the YAML
    let config: Config = serde_yaml::from_str(&rt)?;
    Ok(config)
}

fn load_config(path: &str) -> anyhow::Result<Config> {
    if path.is_empty() {
        return Ok(Config::default_agent());
    }

    let p = std::path::Path::new(path);

    // Check if it's a local file or directory first
    if p.exists() {
        // Allow pointing to a directory (like Go's "agents directory" workflow).
        // If a directory is passed, load agent.yaml/agent.yml from inside it.
        if p.is_dir() {
            let candidate_yaml = p.join("agent.yaml");
            let candidate_yml = p.join("agent.yml");

            if candidate_yaml.is_file() {
                return Ok(Config::load(candidate_yaml)?);
            }
            if candidate_yml.is_file() {
                return Ok(Config::load(candidate_yml)?);
            }

            anyhow::bail!(
                "{} is a directory but contains no agent.yaml or agent.yml",
                p.display()
            );
        }

        return Ok(Config::load(p)?);
    }

    // Check if it's an OCI reference (e.g., agentcatalog/pirate)
        if is_oci_reference(path) {
        return load_oci_config(path);
    }

    // Not a local file and not an OCI reference - error
    anyhow::bail!(
        "Config file not found: {}\n\nIf this is an OCI reference, OCI support is not yet implemented.",
        path
    )
}

fn needs_input(session: &Session) -> bool {
    let msgs = session.get_all_messages();
    msgs.is_empty() || msgs.last().map(|m| m.message.role) != Some(crate::chat::MessageRole::User)
}

fn handle_command(
    input: &str,
    session: &mut Session,
    working_dir: &str,
    yolo: bool,
) -> anyhow::Result<Option<bool>> {
    if !input.starts_with('/') {
        return Ok(None);
    }

    match input {
        "/exit" | "/quit" => Ok(Some(false)),
        "/new" | "/reset" => {
            *session = Session::new()
                .with_working_dir(working_dir)
                .with_tools_approved(yolo);
            println!("✓ Session reset.");
            Ok(Some(true))
        }
        "/usage" => {
            println!(
                "📊 Input: {} | Output: {} | Total: {} | Cost: ${:.4}",
                session.input_tokens,
                session.output_tokens,
                session.input_tokens + session.output_tokens,
                session.cost
            );
            Ok(Some(true))
        }
        "/help" => {
            println!("/new - Reset session\n/usage - Token usage\n/exit - Quit");
            Ok(Some(true))
        }
        _ => {
            println!("Unknown command. Try /help");
            Ok(Some(true))
        }
    }
}

fn print_event(event: &Event) {
    match event {
        Event::AgentChoice { content, .. } => {
            print!("{}", content);
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        Event::AgentReasoning { content, .. } => {
            print!("\x1b[90m{}\x1b[0m", content);
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        Event::ToolCall { tool, agent, .. } => {
            println!("\n\x1b[36m▶ [{}] {}\x1b[0m", agent, tool.name);
        }
        Event::ToolCallResponse { result, .. } if result.is_error => {
            println!("\x1b[31m✗ {}\x1b[0m", result.output);
        }
        Event::Error { message } => println!("\x1b[31m✗ {}\x1b[0m", message),
        Event::Warning { message, .. } => println!("\x1b[33m⚠ {}\x1b[0m", message),
        Event::AgentSwitching {
            active,
            to_agent,
            from_agent,
        } => {
            if *active {
                println!("\n\x1b[35m→ {}\x1b[0m", to_agent);
            } else {
                println!("\n\x1b[35m← {}\x1b[0m", from_agent);
            }
        }
        Event::StreamStopped { .. } => println!(),
        _ => {}
    }
}

pub fn build_team(config: &Config, model_override: Option<&str>) -> anyhow::Result<Team> {
    let working_dir = std::env::current_dir()?;
    let mut agents = Vec::new();

    for agent_config in config.agents.values() {
        let model_ref = model_override
            .map(String::from)
            .or_else(|| agent_config.model.clone())
            .unwrap_or_else(|| "openai/gpt-4o".to_string());

        let (provider_name, model_name) = config
            .models
            .get(&model_ref)
            .map(|m| (m.provider.as_str(), m.model.as_str()))
            .or_else(|| parse_model_ref(&model_ref))
            .ok_or_else(|| anyhow::anyhow!("Invalid model: {}", model_ref))?;

        let model_config = config.models.get(&model_ref);
        let options = ProviderOptions {
            temperature: model_config.and_then(|m| m.temperature),
            max_tokens: model_config.and_then(|m| m.max_tokens),
            parallel_tool_calls: model_config.and_then(|m| m.parallel_tool_calls),
            top_p: model_config.and_then(|m| m.top_p),
            frequency_penalty: model_config.and_then(|m| m.frequency_penalty),
            presence_penalty: model_config.and_then(|m| m.presence_penalty),
            thinking_budget: model_config.and_then(|m| m.thinking_budget),
            base_url: model_config.and_then(|m| m.base_url.clone()),
            token_key: model_config.and_then(|m| m.token_key.clone()),
            provider_opts: model_config
                .map(|m| m.provider_opts.clone())
                .unwrap_or_default(),
        };

        let provider = create_provider_from_parts(
            provider_name,
            model_name,
            options,
            config.providers.get(provider_name),
        )?;

        let toolsets: Vec<Arc<dyn ToolSet>> = agent_config
            .toolsets
            .iter()
            .filter_map(|ts| -> Option<Arc<dyn ToolSet>> {
                let mut base: Arc<dyn ToolSet> = match ts.toolset_type.as_str() {
                    "filesystem" => {
                        let fs = FilesystemToolset::new(&working_dir)
                            .with_post_edit(ts.post_edit.clone())
                            .with_ignore_vcs(ts.ignore_vcs);
                        Arc::new(fs)
                    }
                    "shell" => Arc::new(ShellToolset::new(&working_dir)),
                    "think" => Arc::new(ThinkToolset),
                    "transfer_task" => Arc::new(TransferTaskToolset),
                    "handoff" => Arc::new(HandoffToolset),
                    "todo" => {
                        if ts.shared {
                            Arc::new(TodoToolset::shared())
                        } else {
                            Arc::new(TodoToolset::new())
                        }
                    }
                    "fetch" => Arc::new(FetchToolset::new()),
                    "background_jobs" => Arc::new(BackgroundJobsToolset::new(&working_dir)),
                    "memory" => {
                        let path = ts.path.clone().unwrap_or_else(|| {
                            dirs::home_dir()
                                .unwrap_or_default()
                                .join(".cagent/memory.db")
                                .to_string_lossy()
                                .to_string()
                        });
                        match MemoryToolset::new(&path) {
                            Ok(m) => Arc::new(m) as Arc<dyn ToolSet>,
                            Err(_) => return None,
                        }
                    }
                    "script" => {
                        if ts.shell.is_empty() {
                            debug!("Script toolset has no 'shell' tools defined");
                            return None;
                        }
                        match ScriptToolset::new(&working_dir, ts.shell.clone(), ts.env.clone()) {
                            Ok(s) => Arc::new(s) as Arc<dyn ToolSet>,
                            Err(e) => {
                                debug!("Failed to create script toolset: {}", e);
                                return None;
                            }
                        }
                    }
                    "api" => {
                        let config = ts.api_config.clone().or_else(|| {
                            // For backward compatibility, construct from toolset fields
                            // if api_config is not provided
                            debug!("API toolset requires api_config");
                            None
                        })?;
                        match ApiToolset::new(config) {
                            Ok(a) => Arc::new(a) as Arc<dyn ToolSet>,
                            Err(e) => {
                                debug!("Failed to create api toolset: {}", e);
                                return None;
                            }
                        }
                    }
                    "mcp" => {
                        // Check if this is a remote MCP toolset
                        if let Some(ref remote) = ts.remote {
                            let name = ts.reference.clone().unwrap_or_default();
                            let transport_type = remote
                                .transport_type
                                .as_deref()
                                .and_then(TransportType::from_str)
                                .unwrap_or(TransportType::Sse);
                            Arc::new(RemoteMcpToolset::new(
                                name,
                                remote.url.clone(),
                                transport_type,
                                remote.headers.clone(),
                            )) as Arc<dyn ToolSet>
                        } else {
                            // MCP toolset requires command to be set for stdio transport
                            let command = match ts.command.as_ref() {
                                Some(cmd) if !cmd.is_empty() => cmd.clone(),
                                _ => {
                                    debug!("MCP toolset requires 'command' to be set for stdio transport");
                                    return None;
                                }
                            };
                            // Use 'ref' field for toolset name prefix, or empty string
                            let name = ts.reference.clone().unwrap_or_default();
                            Arc::new(McpToolset::new(
                                name,
                                command,
                                ts.args.clone(),
                                ts.env.clone(),
                                Some(working_dir.clone()),
                            )) as Arc<dyn ToolSet>
                        }
                    }
                    "user_prompt" => {
                        // User prompt toolset - handler will be set by runtime
                        Arc::new(UserPromptToolset::new()) as Arc<dyn ToolSet>
                    }
                    other => {
                        debug!("Unknown toolset: {}", other);
                        return None;
                    }
                };

                // Apply per-toolset instruction override if configured.
                if let Some(instr) = ts.instruction.as_ref().filter(|s| !s.trim().is_empty()) {
                    base = Arc::new(ToolSetWithInstruction::new(base, instr.clone()));
                }

                // Apply tool filtering if configured.
                if !ts.tools.is_empty() {
                    base = Arc::new(FilteredToolSet::new(base, ts.tools.clone()));
                }

                Some(base)
            })
            .collect();

        // Auto-add transfer_task if sub-agents exist
        let mut toolsets = toolsets;
        if !agent_config.sub_agents.is_empty() {
            let has_transfer = toolsets.iter().any(|ts| {
                futures::executor::block_on(ts.tools())
                    .map(|t| t.iter().any(|tool| tool.name == "transfer_task"))
                    .unwrap_or(false)
            });
            if !has_transfer {
                toolsets.push(Arc::new(TransferTaskToolset));
            }
        }

        // Auto-add handoff if handoffs exist
        if !agent_config.handoffs.is_empty() {
            let has_handoff = toolsets.iter().any(|ts| {
                futures::executor::block_on(ts.tools())
                    .map(|t| t.iter().any(|tool| tool.name == "handoff"))
                    .unwrap_or(false)
            });
            if !has_handoff {
                toolsets.push(Arc::new(HandoffToolset));
            }
        }

        // Wrap all toolsets with DescriptionToolSet if configured
        if agent_config.add_description_parameter {
            toolsets = toolsets
                .into_iter()
                .map(|ts| Arc::new(DescriptionToolSet::new(ts)) as Arc<dyn ToolSet>)
                .collect();
        }

        agents.push(AgentBuilder::from_config(
            agent_config,
            Some(provider),
            toolsets,
        ));
    }

    let default = config
        .agents
        .keys()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No agents defined"))?;
    Ok(Team::new(agents, default))
}
