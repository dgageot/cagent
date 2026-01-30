//! OAuth Token Store for MCP servers
//!
//! This module provides storage for OAuth tokens used by MCP servers
//! that require authentication. Supports both in-memory and file-based
//! persistent storage.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// OAuth token with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    /// The access token string
    pub access_token: String,
    /// Token type (usually "Bearer")
    pub token_type: String,
    /// Time-to-live in seconds (optional)
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// Refresh token for getting new access tokens (optional)
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// OAuth scope (optional)
    #[serde(default)]
    pub scope: Option<String>,
    /// Unix timestamp when the token expires (computed from expires_in)
    #[serde(default)]
    pub expires_at: Option<u64>,
}

impl OAuthToken {
    /// Create a new OAuth token
    pub fn new(access_token: impl Into<String>, token_type: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            token_type: token_type.into(),
            expires_in: None,
            refresh_token: None,
            scope: None,
            expires_at: None,
        }
    }

    /// Set the expiration time in seconds
    pub fn with_expires_in(mut self, expires_in: u64) -> Self {
        self.expires_in = Some(expires_in);
        // Compute expires_at from current time
        if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
            self.expires_at = Some(now.as_secs() + expires_in);
        }
        self
    }

    /// Set the refresh token
    pub fn with_refresh_token(mut self, refresh_token: impl Into<String>) -> Self {
        self.refresh_token = Some(refresh_token.into());
        self
    }

    /// Set the scope
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Check if the token is expired (with 30 second buffer)
    pub fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false; // No expiry means never expires
        };

        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return false; // Can't determine, assume not expired
        };

        // Consider expired 30 seconds before actual expiry for safety
        now.as_secs() + 30 >= expires_at
    }

    /// Get the remaining time until expiration
    pub fn time_until_expiry(&self) -> Option<Duration> {
        let expires_at = self.expires_at?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;

        if now.as_secs() >= expires_at {
            return Some(Duration::ZERO);
        }

        Some(Duration::from_secs(expires_at - now.as_secs()))
    }
}

/// Trait for OAuth token storage
pub trait TokenStore: Send + Sync {
    /// Get a token for the given resource URL
    fn get_token(&self, resource_url: &str) -> Option<OAuthToken>;

    /// Store a token for the given resource URL
    fn store_token(&self, resource_url: &str, token: OAuthToken) -> Result<()>;

    /// Remove a token for the given resource URL
    fn remove_token(&self, resource_url: &str) -> Result<()>;

    /// Check if a token exists for the given resource URL
    fn has_token(&self, resource_url: &str) -> bool {
        self.get_token(resource_url).is_some()
    }
}

/// In-memory token store (tokens lost on restart)
#[derive(Debug, Default)]
pub struct InMemoryTokenStore {
    tokens: RwLock<HashMap<String, OAuthToken>>,
}

impl InMemoryTokenStore {
    /// Create a new in-memory token store
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }
}

impl TokenStore for InMemoryTokenStore {
    fn get_token(&self, resource_url: &str) -> Option<OAuthToken> {
        let tokens = self.tokens.read().ok()?;
        let token = tokens.get(resource_url)?;

        // Don't return expired tokens
        if token.is_expired() {
            debug!(resource_url, "Token is expired");
            return None;
        }

        Some(token.clone())
    }

    fn store_token(&self, resource_url: &str, token: OAuthToken) -> Result<()> {
        let mut tokens = self
            .tokens
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;
        tokens.insert(resource_url.to_string(), token);
        debug!(resource_url, "Stored OAuth token");
        Ok(())
    }

    fn remove_token(&self, resource_url: &str) -> Result<()> {
        let mut tokens = self
            .tokens
            .write()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;
        tokens.remove(resource_url);
        debug!(resource_url, "Removed OAuth token");
        Ok(())
    }
}

/// File-based persistent token store
#[derive(Debug)]
pub struct FileTokenStore {
    path: PathBuf,
    tokens: RwLock<HashMap<String, OAuthToken>>,
}

/// Serialized format for the token store file
#[derive(Debug, Serialize, Deserialize)]
struct TokenStoreFile {
    tokens: HashMap<String, OAuthToken>,
}

impl FileTokenStore {
    /// Create a new file-based token store
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Try to load existing tokens
        let tokens = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let store: TokenStoreFile = serde_json::from_str(&content)?;
            store.tokens
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            tokens: RwLock::new(tokens),
        })
    }

    /// Create a token store in the default location (~/.cagent/tokens/)
    pub fn default_store(resource_name: &str) -> Result<Self> {
        let tokens_dir = crate::paths::get_tokens_dir();
        std::fs::create_dir_all(&tokens_dir)?;

        // Sanitize resource name for filename
        let safe_name: String = resource_name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();

        let path = tokens_dir.join(format!("{}.json", safe_name));
        Self::new(path)
    }

    /// Save tokens to disk
    fn save(&self) -> Result<()> {
        let tokens = self
            .tokens
            .read()
            .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;

        let store = TokenStoreFile {
            tokens: tokens.clone(),
        };

        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(&store)?;
        std::fs::write(&self.path, content)?;

        debug!(path = ?self.path, "Saved token store to disk");
        Ok(())
    }
}

impl TokenStore for FileTokenStore {
    fn get_token(&self, resource_url: &str) -> Option<OAuthToken> {
        let tokens = self.tokens.read().ok()?;
        let token = tokens.get(resource_url)?;

        // Don't return expired tokens
        if token.is_expired() {
            debug!(resource_url, "Token is expired");
            return None;
        }

        Some(token.clone())
    }

    fn store_token(&self, resource_url: &str, token: OAuthToken) -> Result<()> {
        {
            let mut tokens = self
                .tokens
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;
            tokens.insert(resource_url.to_string(), token);
        }

        // Persist to disk
        self.save()?;
        debug!(resource_url, path = ?self.path, "Stored and persisted OAuth token");
        Ok(())
    }

    fn remove_token(&self, resource_url: &str) -> Result<()> {
        {
            let mut tokens = self
                .tokens
                .write()
                .map_err(|e| anyhow::anyhow!("Failed to acquire lock: {}", e))?;
            tokens.remove(resource_url);
        }

        // Persist to disk
        self.save()?;
        debug!(resource_url, "Removed OAuth token");
        Ok(())
    }
}

/// Composite token store that uses in-memory with file backup
#[derive(Debug)]
pub struct CachedFileTokenStore {
    memory: InMemoryTokenStore,
    file: FileTokenStore,
}

impl CachedFileTokenStore {
    /// Create a new cached file token store
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let file = FileTokenStore::new(&path)?;

        // Pre-populate memory cache from file
        let memory = InMemoryTokenStore::new();
        {
            let file_tokens = file.tokens.read().map_err(|e| {
                anyhow::anyhow!("Failed to read file tokens: {}", e)
            })?;
            let mut mem_tokens = memory.tokens.write().map_err(|e| {
                anyhow::anyhow!("Failed to write memory tokens: {}", e)
            })?;
            for (url, token) in file_tokens.iter() {
                if !token.is_expired() {
                    mem_tokens.insert(url.clone(), token.clone());
                }
            }
        }

        Ok(Self { memory, file })
    }
}

impl TokenStore for CachedFileTokenStore {
    fn get_token(&self, resource_url: &str) -> Option<OAuthToken> {
        // Try memory first
        if let Some(token) = self.memory.get_token(resource_url) {
            return Some(token);
        }

        // Fall back to file
        self.file.get_token(resource_url)
    }

    fn store_token(&self, resource_url: &str, token: OAuthToken) -> Result<()> {
        // Store in memory
        self.memory.store_token(resource_url, token.clone())?;

        // Persist to file (with error handling)
        if let Err(e) = self.file.store_token(resource_url, token) {
            warn!(error = %e, "Failed to persist token to file");
        }

        Ok(())
    }

    fn remove_token(&self, resource_url: &str) -> Result<()> {
        // Remove from memory
        self.memory.remove_token(resource_url)?;

        // Remove from file (with error handling)
        if let Err(e) = self.file.remove_token(resource_url) {
            warn!(error = %e, "Failed to remove token from file");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oauth_token_creation() {
        let token = OAuthToken::new("access123", "Bearer");
        assert_eq!(token.access_token, "access123");
        assert_eq!(token.token_type, "Bearer");
        assert!(!token.is_expired());
    }

    #[test]
    fn test_oauth_token_with_expiry() {
        let token = OAuthToken::new("access123", "Bearer").with_expires_in(3600);

        assert!(token.expires_at.is_some());
        assert!(!token.is_expired());

        let remaining = token.time_until_expiry();
        assert!(remaining.is_some());
        assert!(remaining.unwrap() > Duration::from_secs(3500));
    }

    #[test]
    fn test_oauth_token_expired() {
        let mut token = OAuthToken::new("access123", "Bearer");
        // Set expiry to past
        token.expires_at = Some(0);

        assert!(token.is_expired());
        assert_eq!(token.time_until_expiry(), Some(Duration::ZERO));
    }

    #[test]
    fn test_in_memory_store() {
        let store = InMemoryTokenStore::new();
        let token = OAuthToken::new("access123", "Bearer").with_expires_in(3600);

        // Store token
        store
            .store_token("https://api.example.com", token.clone())
            .unwrap();

        // Retrieve token
        let retrieved = store.get_token("https://api.example.com");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().access_token, "access123");

        // Remove token
        store.remove_token("https://api.example.com").unwrap();
        assert!(store.get_token("https://api.example.com").is_none());
    }

    #[test]
    fn test_in_memory_store_expired_tokens() {
        let store = InMemoryTokenStore::new();
        let mut token = OAuthToken::new("expired", "Bearer");
        token.expires_at = Some(0); // Already expired

        store
            .store_token("https://api.example.com", token)
            .unwrap();

        // Should not return expired tokens
        assert!(store.get_token("https://api.example.com").is_none());
    }

    #[test]
    fn test_file_store() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tokens.json");

        let store = FileTokenStore::new(&path).unwrap();
        let token = OAuthToken::new("access123", "Bearer")
            .with_expires_in(3600)
            .with_refresh_token("refresh456");

        // Store token
        store
            .store_token("https://api.example.com", token)
            .unwrap();

        // Verify file was created
        assert!(path.exists());

        // Create new store from same file (simulating restart)
        let store2 = FileTokenStore::new(&path).unwrap();

        // Should still have the token
        let retrieved = store2.get_token("https://api.example.com");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().access_token, "access123");
    }

    #[test]
    fn test_has_token() {
        let store = InMemoryTokenStore::new();
        let token = OAuthToken::new("access123", "Bearer").with_expires_in(3600);

        assert!(!store.has_token("https://api.example.com"));

        store
            .store_token("https://api.example.com", token)
            .unwrap();

        assert!(store.has_token("https://api.example.com"));
    }
}
