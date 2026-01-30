//! Auto-pull for OCI registry updates
//!
//! This module provides automatic periodic pulling of agent configs from
//! OCI registries. Useful for keeping agents up-to-date in server deployments.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::oci::pull_agent;

/// Configuration for auto-pull behavior
#[derive(Debug, Clone)]
pub struct AutoPullConfig {
    /// OCI reference to pull (e.g., "docker.io/namespace/agent:latest")
    pub reference: String,
    /// Interval between pull attempts
    pub interval: Duration,
    /// Local path to store the pulled config
    pub target_path: PathBuf,
    /// Whether to continue on errors
    pub ignore_errors: bool,
}

impl AutoPullConfig {
    /// Create a new auto-pull config
    pub fn new(reference: impl Into<String>, target_path: impl Into<PathBuf>) -> Self {
        Self {
            reference: reference.into(),
            interval: Duration::from_secs(300), // Default: 5 minutes
            target_path: target_path.into(),
            ignore_errors: true,
        }
    }

    /// Set the pull interval
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Set error handling behavior
    pub fn with_ignore_errors(mut self, ignore: bool) -> Self {
        self.ignore_errors = ignore;
        self
    }
}

/// Handle for controlling the auto-pull task
pub struct AutoPullHandle {
    shutdown_tx: watch::Sender<bool>,
}

impl AutoPullHandle {
    /// Signal the auto-pull task to stop
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for AutoPullHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Callback type for when a new version is pulled
pub type OnUpdateCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Start an auto-pull task that periodically pulls from an OCI registry
///
/// Returns a handle that can be used to stop the task.
pub fn start_auto_pull(config: AutoPullConfig, on_update: Option<OnUpdateCallback>) -> AutoPullHandle {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tokio::spawn(auto_pull_loop(config, shutdown_rx, on_update));

    AutoPullHandle { shutdown_tx }
}

async fn auto_pull_loop(
    config: AutoPullConfig,
    mut shutdown_rx: watch::Receiver<bool>,
    on_update: Option<OnUpdateCallback>,
) {
    info!(
        reference = %config.reference,
        interval_secs = config.interval.as_secs(),
        target = ?config.target_path,
        "Starting auto-pull for OCI reference"
    );

    // Do an initial pull
    do_pull(&config, &on_update).await;

    let mut timer = tokio::time::interval(config.interval);
    timer.tick().await; // Skip first tick (already pulled)

    loop {
        tokio::select! {
            _ = timer.tick() => {
                do_pull(&config, &on_update).await;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!(reference = %config.reference, "Auto-pull shutting down");
                    return;
                }
            }
        }
    }
}

async fn do_pull(config: &AutoPullConfig, on_update: &Option<OnUpdateCallback>) {
    debug!(reference = %config.reference, "Auto-pulling from OCI registry");

    match pull_agent(&config.reference).await {
        Ok(content) => {
            // Compare with existing content
            let content_changed = match std::fs::read_to_string(&config.target_path) {
                Ok(existing) => existing != content,
                Err(_) => true, // File doesn't exist, so it's new
            };

            if content_changed {
                // Write the new content
                if let Some(parent) = config.target_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        error!(
                            reference = %config.reference,
                            error = %e,
                            "Failed to create parent directory for pulled config"
                        );
                        if !config.ignore_errors {
                            return;
                        }
                    }
                }

                match std::fs::write(&config.target_path, &content) {
                    Ok(_) => {
                        info!(
                            reference = %config.reference,
                            target = ?config.target_path,
                            "Pulled new version from OCI registry"
                        );

                        // Notify callback
                        if let Some(callback) = on_update {
                            callback(&config.reference);
                        }
                    }
                    Err(e) => {
                        error!(
                            reference = %config.reference,
                            error = %e,
                            "Failed to write pulled config"
                        );
                    }
                }
            } else {
                debug!(
                    reference = %config.reference,
                    "No changes detected in OCI registry"
                );
            }
        }
        Err(e) => {
            if config.ignore_errors {
                warn!(
                    reference = %config.reference,
                    error = %e,
                    "Failed to pull from OCI registry (ignoring)"
                );
            } else {
                error!(
                    reference = %config.reference,
                    error = %e,
                    "Failed to pull from OCI registry"
                );
            }
        }
    }
}

/// Builder for auto-pull configuration
#[derive(Debug, Default)]
pub struct AutoPullBuilder {
    configs: Vec<AutoPullConfig>,
    default_interval: Duration,
}

impl AutoPullBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            configs: Vec::new(),
            default_interval: Duration::from_secs(300),
        }
    }

    /// Set the default interval for all configs
    pub fn default_interval(mut self, interval: Duration) -> Self {
        self.default_interval = interval;
        self
    }

    /// Add an OCI reference to auto-pull
    pub fn add(mut self, reference: impl Into<String>, target_path: impl Into<PathBuf>) -> Self {
        let config = AutoPullConfig::new(reference, target_path)
            .with_interval(self.default_interval);
        self.configs.push(config);
        self
    }

    /// Add a config with custom settings
    pub fn add_config(mut self, config: AutoPullConfig) -> Self {
        self.configs.push(config);
        self
    }

    /// Start all auto-pull tasks
    ///
    /// Returns handles for all started tasks
    pub fn start(self, on_update: Option<OnUpdateCallback>) -> Vec<AutoPullHandle> {
        self.configs
            .into_iter()
            .map(|config| start_auto_pull(config, on_update.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_pull_config() {
        let config = AutoPullConfig::new("docker.io/test/agent:latest", "/tmp/agent.yaml")
            .with_interval(Duration::from_secs(60))
            .with_ignore_errors(false);

        assert_eq!(config.reference, "docker.io/test/agent:latest");
        assert_eq!(config.interval, Duration::from_secs(60));
        assert!(!config.ignore_errors);
    }

    #[test]
    fn test_auto_pull_builder() {
        let builder = AutoPullBuilder::new()
            .default_interval(Duration::from_secs(120))
            .add("docker.io/test/agent1:latest", "/tmp/agent1.yaml")
            .add("docker.io/test/agent2:latest", "/tmp/agent2.yaml");

        assert_eq!(builder.configs.len(), 2);
        assert_eq!(builder.configs[0].interval, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn test_auto_pull_handle_stop() {
        // This is a basic test that the handle can be created and stopped
        let config = AutoPullConfig::new("docker.io/test/agent:latest", "/tmp/test.yaml")
            .with_interval(Duration::from_secs(3600)); // Long interval to prevent actual pulls

        let handle = start_auto_pull(config, None);

        // Give task time to start
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Stop should not panic
        handle.stop();

        // Drop should also work
        drop(handle);
    }
}
