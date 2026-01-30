//! Source loader with caching and periodic refresh
//!
//! This module provides a caching layer for configuration sources that
//! periodically refreshes the config in the background. Useful for:
//! - Hot-reloading agent configurations
//! - Caching remote configs (OCI, HTTP)
//! - Reducing I/O overhead for frequently accessed configs

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// A source that can be read
pub trait Source: Send + Sync {
    /// Get the name of the source (for logging)
    fn name(&self) -> &str;

    /// Get the parent directory (for resolving relative paths)
    fn parent_dir(&self) -> Option<&str>;

    /// Read the source content
    fn read(&self) -> Result<Vec<u8>>;
}

/// File-based config source
#[derive(Debug)]
pub struct FileSource {
    path: PathBuf,
    name: String,
}

impl FileSource {
    /// Create a new file source
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path.display().to_string();
        Self { path, name }
    }
}

impl Source for FileSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn parent_dir(&self) -> Option<&str> {
        self.path.parent().and_then(|p| p.to_str())
    }

    fn read(&self) -> Result<Vec<u8>> {
        std::fs::read(&self.path).map_err(|e| anyhow::anyhow!("Failed to read {}: {}", self.path.display(), e))
    }
}

/// In-memory source for testing
#[derive(Debug)]
pub struct MemorySource {
    name: String,
    content: Vec<u8>,
}

impl MemorySource {
    /// Create a new in-memory source
    pub fn new(name: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
        }
    }
}

impl Source for MemorySource {
    fn name(&self) -> &str {
        &self.name
    }

    fn parent_dir(&self) -> Option<&str> {
        None
    }

    fn read(&self) -> Result<Vec<u8>> {
        Ok(self.content.clone())
    }
}

/// Cached data with potential error
struct CachedData {
    data: Option<Vec<u8>>,
    error: Option<String>,
}

/// A source loader that caches content and optionally refreshes periodically
///
/// The loader will:
/// 1. Load content immediately on creation
/// 2. Optionally start a background task to refresh content periodically
/// 3. Cache successful reads, only log errors if data is available
pub struct SourceLoader {
    inner: Arc<dyn Source>,
    refresh_interval: Duration,
    cached: Arc<RwLock<CachedData>>,
    /// Channel to signal shutdown
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl SourceLoader {
    /// Create a new source loader without auto-refresh
    pub fn new(source: impl Source + 'static) -> Self {
        let inner = Arc::new(source);
        let loader = Self {
            inner: inner.clone(),
            refresh_interval: Duration::ZERO,
            cached: Arc::new(RwLock::new(CachedData {
                data: None,
                error: None,
            })),
            shutdown_tx: None,
        };

        // Load initial content
        loader.load();

        loader
    }

    /// Create a new source loader with periodic refresh
    pub fn with_refresh(source: impl Source + 'static, refresh_interval: Duration) -> Self {
        let inner = Arc::new(source);
        let cached = Arc::new(RwLock::new(CachedData {
            data: None,
            error: None,
        }));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let loader = Self {
            inner: inner.clone(),
            refresh_interval,
            cached: cached.clone(),
            shutdown_tx: Some(shutdown_tx),
        };

        // Load initial content
        loader.load();

        // Start background refresh if interval > 0
        if refresh_interval > Duration::ZERO {
            let name = inner.name().to_string();
            let inner_clone = inner.clone();
            let cached_clone = cached;

            tokio::spawn(async move {
                refresh_loop(inner_clone, cached_clone, refresh_interval, shutdown_rx, name).await;
            });

            info!(
                source = loader.inner.name(),
                interval_secs = refresh_interval.as_secs(),
                "Started source loader with auto-refresh"
            );
        }

        loader
    }

    /// Load/refresh the content
    fn load(&self) {
        match self.inner.read() {
            Ok(data) => {
                let mut cached = self.cached.write().unwrap();
                cached.data = Some(data);
                cached.error = None;
                debug!(source = self.inner.name(), "Source loaded successfully");
            }
            Err(e) => {
                let mut cached = self.cached.write().unwrap();
                if cached.data.is_some() {
                    // Keep previous data, just log the error
                    warn!(
                        source = self.inner.name(),
                        error = %e,
                        "Failed to refresh source, keeping previous data"
                    );
                } else {
                    // No previous data, store the error
                    cached.error = Some(e.to_string());
                    warn!(
                        source = self.inner.name(),
                        error = %e,
                        "Failed to load source"
                    );
                }
            }
        }
    }

    /// Get the source name
    pub fn name(&self) -> &str {
        self.inner.name()
    }

    /// Get the parent directory
    pub fn parent_dir(&self) -> Option<&str> {
        self.inner.parent_dir()
    }

    /// Read the cached content
    pub fn read(&self) -> Result<Vec<u8>> {
        let cached = self.cached.read().unwrap();
        
        if let Some(ref data) = cached.data {
            Ok(data.clone())
        } else if let Some(ref error) = cached.error {
            Err(anyhow::anyhow!("{}", error))
        } else {
            Err(anyhow::anyhow!("Source not loaded"))
        }
    }

    /// Force a refresh of the content
    pub fn refresh(&self) {
        self.load();
    }
}

impl Drop for SourceLoader {
    fn drop(&mut self) {
        // Signal shutdown to the background task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }
}

/// Background refresh loop
async fn refresh_loop(
    inner: Arc<dyn Source>,
    cached: Arc<RwLock<CachedData>>,
    interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
    source_name: String,
) {
    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // Skip first tick (already loaded on creation)

    loop {
        tokio::select! {
            _ = timer.tick() => {
                debug!(source = %source_name, "Refreshing source");
                match inner.read() {
                    Ok(data) => {
                        let mut cached = cached.write().unwrap();
                        cached.data = Some(data);
                        cached.error = None;
                        debug!(source = %source_name, "Source refreshed successfully");
                    }
                    Err(e) => {
                        let mut cached = cached.write().unwrap();
                        if cached.data.is_some() {
                            warn!(
                                source = %source_name,
                                error = %e,
                                "Failed to refresh source, keeping previous data"
                            );
                        } else {
                            cached.error = Some(e.to_string());
                        }
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    debug!(source = %source_name, "Source loader shutting down");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_file_source() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "test content").unwrap();

        let source = FileSource::new(tmp.path());
        assert!(source.name().contains("tmp"));
        assert!(source.parent_dir().is_some());

        let content = source.read().unwrap();
        assert_eq!(content, b"test content");
    }

    #[test]
    fn test_memory_source() {
        let source = MemorySource::new("test", "hello world");
        assert_eq!(source.name(), "test");
        assert!(source.parent_dir().is_none());

        let content = source.read().unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn test_source_loader_caches() {
        let source = MemorySource::new("test", "cached content");
        let loader = SourceLoader::new(source);

        // First read
        let content1 = loader.read().unwrap();
        assert_eq!(content1, b"cached content");

        // Second read should return cached
        let content2 = loader.read().unwrap();
        assert_eq!(content2, b"cached content");
    }

    #[test]
    fn test_source_loader_error() {
        struct FailingSource;
        impl Source for FailingSource {
            fn name(&self) -> &str { "failing" }
            fn parent_dir(&self) -> Option<&str> { None }
            fn read(&self) -> Result<Vec<u8>> {
                Err(anyhow::anyhow!("Simulated failure"))
            }
        }

        let loader = SourceLoader::new(FailingSource);
        let result = loader.read();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Simulated failure"));
    }

    #[test]
    fn test_source_loader_keeps_data_on_error() {
        static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct FlakeySource;
        impl Source for FlakeySource {
            fn name(&self) -> &str { "flakey" }
            fn parent_dir(&self) -> Option<&str> { None }
            fn read(&self) -> Result<Vec<u8>> {
                let count = CALL_COUNT.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Ok(b"initial data".to_vec())
                } else {
                    Err(anyhow::anyhow!("Simulated failure"))
                }
            }
        }

        let loader = SourceLoader::new(FlakeySource);

        // First read succeeds
        let content = loader.read().unwrap();
        assert_eq!(content, b"initial data");

        // Force refresh (will fail)
        loader.refresh();

        // Should still have the old data
        let content = loader.read().unwrap();
        assert_eq!(content, b"initial data");
    }

    #[tokio::test]
    async fn test_source_loader_with_refresh() {
        let source = MemorySource::new("test", "content");
        let loader = SourceLoader::with_refresh(source, Duration::from_millis(100));

        // Verify initial load
        let content = loader.read().unwrap();
        assert_eq!(content, b"content");

        // Give time for at least one refresh
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should still work
        let content = loader.read().unwrap();
        assert_eq!(content, b"content");

        // Drop loader to stop background task
        drop(loader);
    }
}
