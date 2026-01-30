//! Path utilities for cagent
//!
//! This module provides utilities for resolving cagent configuration and data
//! directories following XDG-like conventions.

use std::path::PathBuf;

/// Get the user's home directory
pub fn get_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Get the cagent config directory
///
/// Returns `~/.config/cagent` on Unix-like systems, or falls back to
/// a temporary directory if the home directory cannot be determined.
pub fn get_config_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".config").join("cagent"),
        None => {
            // Fallback to temp directory
            std::env::temp_dir().join(".cagent-config")
        }
    }
}

/// Get the cagent data directory (caches, content, logs, sessions)
///
/// Returns `~/.cagent` on Unix-like systems, or falls back to
/// a temporary directory if the home directory cannot be determined.
pub fn get_data_dir() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".cagent"),
        None => {
            // Fallback to temp directory
            std::env::temp_dir().join(".cagent")
        }
    }
}

/// Get the default sessions directory
pub fn get_sessions_dir() -> PathBuf {
    get_data_dir().join("sessions")
}

/// Get the default sessions database path
pub fn get_sessions_db_path() -> PathBuf {
    get_data_dir().join("sessions.db")
}

/// Get the debug log file path
pub fn get_debug_log_path() -> PathBuf {
    get_data_dir().join("cagent.debug.log")
}

/// Get the MCP content store directory
pub fn get_content_store_dir() -> PathBuf {
    get_data_dir().join("content")
}

/// Get the OCI artifact store directory
pub fn get_store_dir() -> PathBuf {
    get_data_dir().join("store")
}

/// Get the OAuth tokens directory
pub fn get_tokens_dir() -> PathBuf {
    get_data_dir().join("tokens")
}

/// Get the MCP catalog cache directory
pub fn get_catalog_cache_dir() -> PathBuf {
    get_data_dir().join("catalog")
}

/// Ensure a directory exists, creating it if necessary
pub fn ensure_dir(path: &std::path::Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

/// Expand tilde in path (~)
pub fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            return if path == "~" {
                home
            } else {
                home.join(&path[2..])
            };
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_home_dir() {
        // Should return a non-empty path on most systems
        let home = get_home_dir();
        // Can't assert much since this depends on the system
        // Just verify it doesn't panic
        drop(home);
    }

    #[test]
    fn test_get_config_dir() {
        let config_dir = get_config_dir();
        assert!(config_dir.to_string_lossy().contains("cagent"));
    }

    #[test]
    fn test_get_data_dir() {
        let data_dir = get_data_dir();
        assert!(data_dir.to_string_lossy().contains("cagent"));
    }

    #[test]
    fn test_get_sessions_dir() {
        let sessions_dir = get_sessions_dir();
        assert!(sessions_dir.to_string_lossy().contains("sessions"));
    }

    #[test]
    fn test_get_debug_log_path() {
        let log_path = get_debug_log_path();
        assert!(log_path.to_string_lossy().contains("cagent.debug.log"));
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/test/path");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expanded, home.join("test/path"));
        }

        // Non-tilde path should be unchanged
        let non_tilde = expand_tilde("/absolute/path");
        assert_eq!(non_tilde, PathBuf::from("/absolute/path"));

        // Just tilde
        let just_tilde = expand_tilde("~");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(just_tilde, home);
        }
    }

    #[test]
    fn test_ensure_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("test_dir");
        
        assert!(!new_dir.exists());
        ensure_dir(&new_dir).unwrap();
        assert!(new_dir.exists());
        
        // Should be idempotent
        ensure_dir(&new_dir).unwrap();
        assert!(new_dir.exists());
    }
}
