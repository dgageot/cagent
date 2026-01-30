//! Environment variable providers
//!
//! This module provides abstraction over environment variable sources, allowing
//! cagent to get variables from different sources (OS environment, files, etc.)

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use tracing::debug;

use crate::desktop;

/// Provider trait for accessing environment variables
pub trait EnvProvider: Send + Sync {
    /// Get an environment variable by name
    /// Returns (value, found) where found indicates if the variable was present
    fn get(&self, name: &str) -> (String, bool);
}

/// Provider that reads from OS environment variables
#[derive(Debug, Default, Clone)]
pub struct OsEnvProvider;

impl OsEnvProvider {
    pub fn new() -> Self {
        Self
    }
}

impl EnvProvider for OsEnvProvider {
    fn get(&self, name: &str) -> (String, bool) {
        match std::env::var(name) {
            Ok(value) => (value, true),
            Err(_) => (String::new(), false),
        }
    }
}

/// Provider that reads from a list of KEY=VALUE strings
#[derive(Debug, Clone)]
pub struct EnvListProvider {
    env: Vec<String>,
}

impl EnvListProvider {
    pub fn new(env: Vec<String>) -> Self {
        Self { env }
    }
}

impl EnvProvider for EnvListProvider {
    fn get(&self, name: &str) -> (String, bool) {
        for entry in &self.env {
            if let Some((key, value)) = entry.split_once('=') {
                if key == name {
                    return (value.to_string(), true);
                }
            }
        }
        (String::new(), false)
    }
}

/// Provider that reads from .env files
#[derive(Debug, Clone)]
pub struct EnvFilesProvider {
    values: HashMap<String, String>,
}

impl EnvFilesProvider {
    /// Create a new provider from a list of .env file paths
    pub fn new(env_files: &[impl AsRef<Path>]) -> Result<Self> {
        let mut values = HashMap::new();
        
        for file_path in env_files {
            let contents = std::fs::read_to_string(file_path)?;
            for line in contents.lines() {
                let line = line.trim();
                
                // Skip empty lines and comments
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                
                // Parse KEY=VALUE, handling quoted values
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    
                    // Remove surrounding quotes if present
                    let value = if (value.starts_with('"') && value.ends_with('"'))
                        || (value.starts_with('\'') && value.ends_with('\''))
                    {
                        &value[1..value.len() - 1]
                    } else {
                        value
                    };
                    
                    values.insert(key.to_string(), value.to_string());
                }
            }
        }
        
        Ok(Self { values })
    }
}

impl EnvProvider for EnvFilesProvider {
    fn get(&self, name: &str) -> (String, bool) {
        match self.values.get(name) {
            Some(value) => (value.clone(), true),
            None => (String::new(), false),
        }
    }
}

/// Provider that chains multiple providers, returning the first match
#[derive(Default)]
pub struct MultiProvider {
    providers: Vec<Box<dyn EnvProvider>>,
}

impl MultiProvider {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }
    
    /// Add a provider to the chain
    pub fn add(mut self, provider: impl EnvProvider + 'static) -> Self {
        self.providers.push(Box::new(provider));
        self
    }
    
    /// Create a default multi-provider with OS environment as first provider
    pub fn with_os_env(self) -> Self {
        self.add(OsEnvProvider::new())
    }
}

impl EnvProvider for MultiProvider {
    fn get(&self, name: &str) -> (String, bool) {
        for provider in &self.providers {
            let (value, found) = provider.get(name);
            if found {
                return (value, true);
            }
        }
        (String::new(), false)
    }
}

/// Provider backed by a HashMap (useful for testing)
#[derive(Debug, Clone, Default)]
pub struct MapEnvProvider {
    values: HashMap<String, String>,
}

impl MapEnvProvider {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }
    
    pub fn with_value(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }
    
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }
}

impl EnvProvider for MapEnvProvider {
    fn get(&self, name: &str) -> (String, bool) {
        match self.values.get(name) {
            Some(value) => (value.clone(), true),
            None => (String::new(), false),
        }
    }
}

// Docker Desktop environment variable names
/// Environment variable for Docker user email
pub const DOCKER_EMAIL: &str = "DOCKER_EMAIL";
/// Environment variable for Docker username
pub const DOCKER_USERNAME: &str = "DOCKER_USERNAME";
/// Environment variable for Docker token
pub const DOCKER_TOKEN: &str = "DOCKER_TOKEN";

/// Provider that retrieves Docker credentials from Docker Desktop
///
/// This provider communicates with the Docker Desktop backend API to retrieve:
/// - DOCKER_TOKEN: Authentication token for Docker Hub
/// - DOCKER_EMAIL: User's Docker Hub email
/// - DOCKER_USERNAME: User's Docker Hub username
#[derive(Debug, Default, Clone)]
pub struct DockerDesktopProvider;

impl DockerDesktopProvider {
    pub fn new() -> Self {
        Self
    }
}

impl EnvProvider for DockerDesktopProvider {
    fn get(&self, name: &str) -> (String, bool) {
        match name {
            DOCKER_EMAIL => {
                if let Some(info) = desktop::get_user_info_blocking() {
                    debug!("Retrieved Docker email from Docker Desktop");
                    return (info.email, true);
                }
                (String::new(), false)
            }
            DOCKER_USERNAME => {
                if let Some(info) = desktop::get_user_info_blocking() {
                    debug!("Retrieved Docker username from Docker Desktop");
                    return (info.username, true);
                }
                (String::new(), false)
            }
            DOCKER_TOKEN => {
                if let Some(token) = desktop::get_token_blocking() {
                    debug!("Retrieved Docker token from Docker Desktop");
                    return (token, true);
                }
                (String::new(), false)
            }
            _ => (String::new(), false),
        }
    }
}

/// Create a default environment provider chain
///
/// The chain includes (in order of priority):
/// 1. OS environment variables
/// 2. Docker Desktop provider (for DOCKER_TOKEN, DOCKER_EMAIL, DOCKER_USERNAME)
pub fn new_default_provider() -> MultiProvider {
    MultiProvider::new()
        .add(OsEnvProvider::new())
        .add(DockerDesktopProvider::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_env_provider() {
        std::env::set_var("TEST_VAR_12345", "test_value");
        let provider = OsEnvProvider::new();
        
        let (value, found) = provider.get("TEST_VAR_12345");
        assert!(found);
        assert_eq!(value, "test_value");
        
        let (_, found) = provider.get("NONEXISTENT_VAR_12345");
        assert!(!found);
        
        std::env::remove_var("TEST_VAR_12345");
    }

    #[test]
    fn test_env_list_provider() {
        let env = vec![
            "KEY1=value1".to_string(),
            "KEY2=value2".to_string(),
            "KEY3=".to_string(),
        ];
        let provider = EnvListProvider::new(env);
        
        let (value, found) = provider.get("KEY1");
        assert!(found);
        assert_eq!(value, "value1");
        
        let (value, found) = provider.get("KEY3");
        assert!(found);
        assert_eq!(value, "");
        
        let (_, found) = provider.get("NONEXISTENT");
        assert!(!found);
    }

    #[test]
    fn test_env_files_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join(".env");
        std::fs::write(&env_file, r#"
# Comment line
KEY1=value1
KEY2="quoted value"
KEY3='single quoted'
KEY4=
"#).unwrap();
        
        let provider = EnvFilesProvider::new(&[&env_file]).unwrap();
        
        let (value, found) = provider.get("KEY1");
        assert!(found);
        assert_eq!(value, "value1");
        
        let (value, found) = provider.get("KEY2");
        assert!(found);
        assert_eq!(value, "quoted value");
        
        let (value, found) = provider.get("KEY3");
        assert!(found);
        assert_eq!(value, "single quoted");
        
        let (value, found) = provider.get("KEY4");
        assert!(found);
        assert_eq!(value, "");
    }

    #[test]
    fn test_multi_provider() {
        let list_provider = EnvListProvider::new(vec!["KEY1=list_value".to_string()]);
        let map_provider = MapEnvProvider::new()
            .with_value("KEY1", "map_value")
            .with_value("KEY2", "only_in_map");
        
        // list_provider is checked first, so KEY1 should come from it
        let provider = MultiProvider::new()
            .add(list_provider)
            .add(map_provider);
        
        let (value, found) = provider.get("KEY1");
        assert!(found);
        assert_eq!(value, "list_value");
        
        let (value, found) = provider.get("KEY2");
        assert!(found);
        assert_eq!(value, "only_in_map");
    }

    #[test]
    fn test_map_env_provider() {
        let provider = MapEnvProvider::new()
            .with_value("KEY1", "value1")
            .with_value("KEY2", "value2");
        
        let (value, found) = provider.get("KEY1");
        assert!(found);
        assert_eq!(value, "value1");
        
        let (_, found) = provider.get("NONEXISTENT");
        assert!(!found);
    }
}
