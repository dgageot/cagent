//! Agent abstraction for AI assistants.
//!
//! An [`Agent`] represents an AI assistant configured with:
//! - A model provider (OpenAI, Anthropic, etc.)
//! - A set of tools it can use
//! - Optional sub-agents for task delegation
//! - Configuration options (date, environment info, history limits)
//!
//! # Multi-Agent Support
//!
//! Agents can be organized into hierarchies where a parent agent can
//! delegate tasks to sub-agents using the `transfer_task` tool.
//! This enables building teams of specialized agents.
//!
//! # Example Configuration
//!
//! ```yaml
//! agents:
//!   root:
//!     model: openai/gpt-4o
//!     description: "Project coordinator"
//!     sub_agents:
//!       - researcher
//!       - developer
//!     toolsets:
//!       - type: transfer_task
//!
//!   researcher:
//!     model: anthropic/claude-sonnet-4-0
//!     description: "Research specialist"
//!     toolsets:
//!       - type: fetch
//!       - type: filesystem
//! ```

use std::fmt;
use std::sync::Arc;

use crate::config::{AgentConfig, HooksConfig};
use crate::model::Provider;
use crate::tools::{Tool, ToolSet};

/// An AI agent with tools and configuration
#[derive(Clone)]
pub struct Agent {
    pub name: String,
    pub description: Option<String>,
    pub welcome_message: Option<String>,
    pub instruction: Option<String>,
    pub model: Option<Arc<dyn Provider>>,
    pub toolsets: Vec<Arc<dyn ToolSet>>,
    pub sub_agents: Vec<Agent>,
    pub handoffs: Vec<Agent>,
    pub parents: Vec<String>,
    pub add_date: bool,
    pub add_environment_info: bool,
    pub max_iterations: usize,
    pub num_history_items: usize,
    /// Additional prompt files to include as system messages
    pub add_prompt_files: Vec<String>,
    /// Add a "description" parameter to all tools
    pub add_description_parameter: bool,
    /// Hooks configuration for lifecycle events
    pub hooks: Option<HooksConfig>,
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("toolsets_count", &self.toolsets.len())
            .field(
                "sub_agents",
                &self.sub_agents.iter().map(|a| &a.name).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Agent {
    /// Create a new agent from configuration
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            welcome_message: None,
            instruction: None,
            model: None,
            toolsets: Vec::new(),
            sub_agents: Vec::new(),
            handoffs: Vec::new(),
            parents: Vec::new(),
            add_date: false,
            add_environment_info: false,
            max_iterations: 0,
            num_history_items: 0,
            add_prompt_files: Vec::new(),
            add_description_parameter: false,
            hooks: None,
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instruction = Some(instruction.into());
        self
    }

    pub fn with_model(mut self, model: Arc<dyn Provider>) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_toolset(mut self, toolset: Arc<dyn ToolSet>) -> Self {
        self.toolsets.push(toolset);
        self
    }

    pub fn with_sub_agent(mut self, agent: Agent) -> Self {
        self.sub_agents.push(agent);
        self
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    pub fn with_add_date(mut self, add: bool) -> Self {
        self.add_date = add;
        self
    }

    pub fn with_add_environment_info(mut self, add: bool) -> Self {
        self.add_environment_info = add;
        self
    }

    /// Check if this agent has sub-agents
    pub fn has_sub_agents(&self) -> bool {
        !self.sub_agents.is_empty()
    }

    /// Get all available tools from all toolsets
    pub async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        let mut all_tools = Vec::new();
        for toolset in &self.toolsets {
            let tools = toolset.tools().await?;
            all_tools.extend(tools);
        }
        Ok(all_tools)
    }

    /// Get a model provider for this agent
    pub fn get_model(&self) -> Option<Arc<dyn Provider>> {
        self.model.clone()
    }
}

/// A team of agents
#[derive(Clone)]
pub struct Team {
    agents: Vec<Agent>,
    default_agent: String,
}

impl fmt::Debug for Team {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Team")
            .field(
                "agents",
                &self.agents.iter().map(|a| &a.name).collect::<Vec<_>>(),
            )
            .field("default_agent", &self.default_agent)
            .finish()
    }
}

impl Team {
    pub fn new(agents: Vec<Agent>, default_agent: impl Into<String>) -> Self {
        Self {
            agents,
            default_agent: default_agent.into(),
        }
    }

    /// Get the default agent
    pub fn default_agent(&self) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == self.default_agent)
    }

    /// Get the name of the default agent
    pub fn default_agent_name(&self) -> &str {
        &self.default_agent
    }

    /// Get an agent by name
    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Get all agents
    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    /// Get all agent names
    pub fn agent_names(&self) -> Vec<String> {
        self.agents.iter().map(|a| a.name.clone()).collect()
    }

    /// Number of agents
    pub fn size(&self) -> usize {
        self.agents.len()
    }

    /// Start all toolsets for all agents
    pub async fn start_toolsets(&self) -> anyhow::Result<()> {
        for agent in &self.agents {
            for toolset in &agent.toolsets {
                toolset.start().await?;
            }
        }
        Ok(())
    }

    /// Stop all toolsets for all agents
    pub async fn stop_toolsets(&self) -> anyhow::Result<()> {
        for agent in &self.agents {
            for toolset in &agent.toolsets {
                toolset.stop().await?;
            }
        }
        Ok(())
    }
}

/// Builder for creating agents from config
pub struct AgentBuilder;

/// Inner builder state for agents
pub struct AgentBuilderInner {
    name: String,
    description: Option<String>,
    instruction: Option<String>,
    model: Option<Arc<dyn Provider>>,
    toolsets: Vec<Arc<dyn ToolSet>>,
}

impl AgentBuilderInner {
    /// Create a new builder inner
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            instruction: None,
            model: None,
            toolsets: Vec::new(),
        }
    }

    /// Set the agent description
    pub fn with_description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    /// Set the agent instruction
    pub fn with_instruction(mut self, instruction: &str) -> Self {
        self.instruction = Some(instruction.to_string());
        self
    }

    /// Set the agent model
    pub fn with_model(mut self, model: Arc<dyn Provider>) -> Self {
        self.model = Some(model);
        self
    }

    /// Add a toolset
    pub fn with_toolset(mut self, toolset: Arc<dyn ToolSet>) -> Self {
        self.toolsets.push(toolset);
        self
    }

    /// Build the agent
    pub fn build(self) -> Agent {
        Agent {
            name: self.name,
            description: self.description,
            welcome_message: None,
            instruction: self.instruction,
            model: self.model,
            toolsets: self.toolsets,
            sub_agents: Vec::new(),
            handoffs: Vec::new(),
            parents: Vec::new(),
            add_date: false,
            add_environment_info: false,
            max_iterations: 0,
            num_history_items: 0,
            add_prompt_files: Vec::new(),
            add_description_parameter: false,
            hooks: None,
        }
    }
}

impl AgentBuilder {
    /// Create a new agent builder with the given name
    pub fn new(name: &str) -> AgentBuilderInner {
        AgentBuilderInner::new(name)
    }

    /// Build an agent from configuration
    pub fn from_config(
        config: &AgentConfig,
        model: Option<Arc<dyn Provider>>,
        toolsets: Vec<Arc<dyn ToolSet>>,
    ) -> Agent {
        Agent {
            name: config.name.clone(),
            description: config.description.clone(),
            welcome_message: config.welcome_message.clone(),
            instruction: config.instruction.clone(),
            model,
            toolsets,
            sub_agents: Vec::new(), // Will be populated later
            handoffs: Vec::new(),
            parents: Vec::new(),
            add_date: config.add_date,
            add_environment_info: config.add_environment_info,
            max_iterations: config.max_iterations,
            num_history_items: config.num_history_items,
            add_prompt_files: config.add_prompt_files.clone(),
            add_description_parameter: config.add_description_parameter,
            hooks: config.hooks.clone(),
        }
    }
}
