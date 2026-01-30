//! Testing utilities for cagent
//!
//! This module provides VCR (Video Cassette Recorder) style testing utilities
//! for recording and replaying AI API interactions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

/// VCR mode for recording/replaying
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcrMode {
    /// Only replay from cassette, fail if not found
    ReplayOnly,
    /// Record new interactions, replay existing ones
    RecordOnce,
    /// Always record, overwriting existing cassettes
    Record,
}

/// Cassette file for storing recorded interactions
#[derive(Debug)]
pub struct Cassette {
    path: PathBuf,
    interactions: Vec<Interaction>,
    mode: VcrMode,
}

/// A recorded HTTP interaction
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Interaction {
    pub request: RecordedRequest,
    pub response: RecordedResponse,
}

/// Recorded HTTP request
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    pub body: String,
}

/// Recorded HTTP response
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordedResponse {
    pub status: u16,
    pub body: String,
}

impl Cassette {
    /// Create a new cassette at the given path
    pub fn new(path: impl AsRef<Path>, mode: VcrMode) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        
        // Try to load existing cassette
        let interactions = if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            serde_json::from_str(&contents)?
        } else {
            Vec::new()
        };

        Ok(Self {
            path,
            interactions,
            mode,
        })
    }

    /// Get path to cassettes directory (relative to test file)
    pub fn cassette_path(test_name: &str) -> PathBuf {
        PathBuf::from("tests")
            .join("cassettes")
            .join(format!("{}.json", test_name))
    }

    /// Find a matching interaction
    pub fn find_match(&self, method: &str, url: &str, body: &str) -> Option<&Interaction> {
        // Normalize dynamic fields for matching
        let normalized_body = normalize_request_body(body);
        
        self.interactions.iter().find(|i| {
            i.request.method == method
                && i.request.url == url
                && normalize_request_body(&i.request.body) == normalized_body
        })
    }

    /// Add a new interaction
    pub fn add_interaction(&mut self, interaction: Interaction) {
        self.interactions.push(interaction);
    }

    /// Save the cassette to disk
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(&self.interactions)?;
        std::fs::write(&self.path, contents)?;
        Ok(())
    }

    /// Get the mode
    pub fn mode(&self) -> VcrMode {
        self.mode
    }
}

/// Normalize request body for matching (removes dynamic fields like tool call IDs)
fn normalize_request_body(body: &str) -> String {
    // Remove tool call IDs which are dynamic
    let re = regex::Regex::new(r#"call_[a-z0-9\-]+"#).unwrap();
    re.replace_all(body, "call_ID").to_string()
}

/// VCR test context for a single test
#[allow(dead_code)]
pub struct VcrTestContext {
    cassette: Arc<std::sync::Mutex<Cassette>>,
    base_url: String,
}

impl VcrTestContext {
    /// Create a new VCR test context
    pub fn new(test_name: &str, mode: VcrMode) -> Result<Self> {
        let path = Cassette::cassette_path(test_name);
        let cassette = Cassette::new(path, mode)?;
        
        Ok(Self {
            cassette: Arc::new(std::sync::Mutex::new(cassette)),
            base_url: String::new(),
        })
    }

    /// Get the cassette
    pub fn cassette(&self) -> Arc<std::sync::Mutex<Cassette>> {
        Arc::clone(&self.cassette)
    }

    /// Save the cassette when test completes
    pub fn save(&self) -> Result<()> {
        self.cassette.lock().unwrap().save()
    }
}

impl Drop for VcrTestContext {
    fn drop(&mut self) {
        if let Err(e) = self.save() {
            eprintln!("Failed to save cassette: {}", e);
        }
    }
}

/// Macro to create a VCR test
#[macro_export]
macro_rules! vcr_test {
    ($test_name:ident, $mode:expr, $body:expr) => {
        #[tokio::test]
        async fn $test_name() {
            let ctx = $crate::testing::VcrTestContext::new(
                stringify!($test_name),
                $mode,
            ).expect("Failed to create VCR context");
            
            $body(ctx).await;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_request_body() {
        let body = r#"{"tool_calls":[{"id":"call_abc123def456"}]}"#;
        let normalized = normalize_request_body(body);
        assert!(normalized.contains("call_ID"));
        assert!(!normalized.contains("call_abc123def456"));
    }

    #[test]
    fn test_cassette_roundtrip() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let cassette_path = tmp_dir.path().join("test_cassette.json");
        
        // Create new cassette (file doesn't exist yet)
        let mut cassette = Cassette::new(&cassette_path, VcrMode::RecordOnce).unwrap();
        
        cassette.add_interaction(Interaction {
            request: RecordedRequest {
                method: "POST".to_string(),
                url: "https://api.openai.com/v1/chat/completions".to_string(),
                body: r#"{"model":"gpt-4"}"#.to_string(),
            },
            response: RecordedResponse {
                status: 200,
                body: r#"{"choices":[]}"#.to_string(),
            },
        });
        
        cassette.save().unwrap();
        
        // Reload and verify
        let loaded = Cassette::new(&cassette_path, VcrMode::ReplayOnly).unwrap();
        assert_eq!(loaded.interactions.len(), 1);
        
        // Test matching
        let matched = loaded.find_match(
            "POST",
            "https://api.openai.com/v1/chat/completions",
            r#"{"model":"gpt-4"}"#,
        );
        assert!(matched.is_some());
    }
}
