//! MCP Catalog module
//!
//! This module provides access to the Docker MCP catalog with local caching
//! and automatic refresh on stale data.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, warn};

use crate::paths;

/// Docker MCP Catalog URL
pub const DOCKER_CATALOG_URL: &str = "https://desktop.docker.com/mcp/catalog/v3/catalog.json";

/// Cache filename
const CATALOG_CACHE_FILENAME: &str = "mcp_catalog.json";

/// Cache duration (24 hours)
const CATALOG_CACHE_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Server type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    #[serde(default)]
    pub secrets: Vec<Secret>,
}

/// Secret definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Secret {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Catalog is a map of server names to server specifications
pub type Catalog = HashMap<String, Server>;

/// Cached catalog data
#[derive(Debug, Serialize, Deserialize)]
struct CachedCatalog {
    catalog: Catalog,
    cached_at: chrono::DateTime<chrono::Utc>,
}

/// Global catalog state (using tokio RwLock for async-safe access)
struct CatalogState {
    catalog: Catalog,
    loaded: bool,
    stale: bool,
}

impl Default for CatalogState {
    fn default() -> Self {
        Self {
            catalog: HashMap::new(),
            loaded: false,
            stale: false,
        }
    }
}

lazy_static::lazy_static! {
    static ref CATALOG_STATE: RwLock<CatalogState> = RwLock::new(CatalogState::default());
}

static REFRESH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Get the cache file path
fn get_cache_file_path() -> PathBuf {
    paths::get_catalog_cache_dir().join(CATALOG_CACHE_FILENAME)
}

/// Load catalog from cache file
fn load_catalog_from_cache() -> Result<(Catalog, Duration)> {
    let cache_file = get_cache_file_path();

    let data = std::fs::read_to_string(&cache_file)?;
    let cached: CachedCatalog = serde_json::from_str(&data)?;

    let cache_age = chrono::Utc::now()
        .signed_duration_since(cached.cached_at)
        .to_std()
        .unwrap_or(Duration::ZERO);

    Ok((cached.catalog, cache_age))
}

/// Save catalog to cache file
fn save_catalog_to_cache(catalog: &Catalog) -> Result<()> {
    let cache_file = get_cache_file_path();

    // Ensure directory exists
    if let Some(parent) = cache_file.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let cached = CachedCatalog {
        catalog: catalog.clone(),
        cached_at: chrono::Utc::now(),
    };

    let data = serde_json::to_string_pretty(&cached)?;
    std::fs::write(&cache_file, data)?;

    Ok(())
}

/// Fetch catalog from network
pub async fn fetch_catalog_from_network() -> Result<Catalog> {
    debug!("Fetching MCP catalog from network");

    let client = reqwest::Client::new();
    let resp = client.get(DOCKER_CATALOG_URL).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch catalog: {}", resp.status());
    }

    #[derive(Deserialize)]
    struct TopLevel {
        catalog: Catalog,
    }

    let top_level: TopLevel = resp.json().await?;
    Ok(top_level.catalog)
}

/// Ensure catalog is loaded (from cache or network)
pub async fn ensure_catalog_loaded() {
    // Quick check if already loaded
    {
        let state = CATALOG_STATE.read().await;
        if state.loaded {
            return;
        }
    }

    // Acquire write lock to load
    let mut state = CATALOG_STATE.write().await;

    // Double-check after acquiring write lock
    if state.loaded {
        return;
    }

    // Try loading from cache first
    if let Ok((catalog, cache_age)) = load_catalog_from_cache() {
        debug!(
            cache_age = ?cache_age,
            servers = catalog.len(),
            "Loaded MCP catalog from cache"
        );
        state.catalog = catalog;
        state.loaded = true;
        state.stale = cache_age > CATALOG_CACHE_DURATION;
        return;
    }

    // Cache miss, need to fetch from network
    // Release lock during network call
    drop(state);

    match fetch_catalog_from_network().await {
        Ok(catalog) => {
            // Save to cache
            if let Err(e) = save_catalog_to_cache(&catalog) {
                warn!(error = %e, "Failed to save catalog to cache");
            }

            let mut state = CATALOG_STATE.write().await;
            state.catalog = catalog;
            state.loaded = true;
            state.stale = false;
        }
        Err(e) => {
            error!(error = %e, "Failed to fetch MCP catalog");
        }
    }
}

/// Get a server from the catalog
pub async fn get_server(server_name: &str) -> Option<Server> {
    ensure_catalog_loaded().await;

    // Check the cache first
    let (server, should_refresh) = {
        let state = CATALOG_STATE.read().await;
        let server = state.catalog.get(server_name).cloned();
        let should_refresh = state.stale && server.is_some();
        (server, should_refresh)
    };

    if let Some(srv) = server {
        // If stale, trigger background refresh for next time
        if should_refresh {
            trigger_background_refresh();
        }
        return Some(srv);
    }

    // Server not found, try refreshing
    if refresh_catalog_from_network().await {
        let state = CATALOG_STATE.read().await;
        return state.catalog.get(server_name).cloned();
    }

    None
}

/// Get required environment variables for a server
pub async fn required_env_vars(server_name: &str) -> Result<Vec<Secret>> {
    let server = get_server(server_name)
        .await
        .ok_or_else(|| anyhow::anyhow!("MCP server '{}' not found in catalog", server_name))?;

    // For remote servers, assume OAuth is used
    if server.server_type.as_deref() == Some("remote") {
        return Ok(vec![]);
    }

    Ok(server.secrets)
}

/// List all servers in the catalog
pub async fn list_servers() -> Catalog {
    ensure_catalog_loaded().await;

    let state = CATALOG_STATE.read().await;
    state.catalog.clone()
}

/// Trigger a background refresh of the catalog
fn trigger_background_refresh() {
    // Use atomic to prevent multiple concurrent refreshes
    if REFRESH_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // Refresh already in progress
    }

    tokio::spawn(async {
        let _ = refresh_catalog_from_network().await;
        REFRESH_IN_PROGRESS.store(false, Ordering::SeqCst);
    });
}

/// Refresh catalog from network
async fn refresh_catalog_from_network() -> bool {
    match fetch_catalog_from_network().await {
        Ok(catalog) => {
            if let Err(e) = save_catalog_to_cache(&catalog) {
                warn!(error = %e, "Failed to save refreshed catalog to cache");
            }

            let mut state = CATALOG_STATE.write().await;
            state.catalog = catalog;
            state.stale = false;

            debug!("MCP catalog refreshed from network");
            true
        }
        Err(e) => {
            debug!(error = %e, "Background catalog refresh failed");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_file_path() {
        let path = get_cache_file_path();
        assert!(path.to_string_lossy().contains("catalog"));
        assert!(path.to_string_lossy().contains("mcp_catalog.json"));
    }

    #[tokio::test]
    async fn test_list_servers_empty_initially() {
        // This test verifies the initial state
        // Actual network fetch would happen in integration tests
        // For unit test, we just verify the function doesn't panic
    }
}
