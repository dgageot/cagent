//! OCI Registry support for cagent
//!
//! This module provides functionality to:
//! - Push agents to OCI registries
//! - Pull agents from OCI registries
//! - Store and cache artifacts locally
//! - Auto-pull for periodic registry updates

pub mod auto_pull;

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use oci_client::client::{ClientConfig, Config as OciConfig, ImageLayer};
use oci_client::manifest::OciImageManifest;
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_stream::StreamExt;
use tracing::{debug, info, warn};

use crate::paths;

/// Media types for cagent OCI artifacts
pub mod media_types {
    /// Media type for cagent config
    pub const CONFIG: &str = "application/vnd.docker.cagent.config.v1+json";
    /// Media type for agent YAML layer
    pub const AGENT_YAML: &str = "application/yaml";
}

/// OCI annotations used by cagent
pub mod annotations {
    /// cagent version that created the artifact
    pub const CAGENT_VERSION: &str = "io.docker.cagent.version";
    /// Creation timestamp
    pub const CREATED: &str = "org.opencontainers.image.created";
    /// Description
    pub const DESCRIPTION: &str = "org.opencontainers.image.description";
    /// Author
    pub const AUTHORS: &str = "org.opencontainers.image.authors";
    /// License
    pub const LICENSES: &str = "org.opencontainers.image.licenses";
    /// Version/revision
    pub const REVISION: &str = "org.opencontainers.image.revision";
}

/// Error type for corrupted local store
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("local artifact store corrupted - artifact may need to be re-fetched")]
    Corrupted,
    #[error("artifact not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Metadata about a stored artifact
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    /// Content digest (sha256:...)
    pub digest: String,
    /// OCI reference (e.g., namespace/repo:tag)
    pub reference: String,
    /// Size in bytes
    pub size: u64,
    /// When the artifact was stored
    pub stored_at: chrono::DateTime<chrono::Utc>,
    /// Labels (from OCI config)
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Annotations (from OCI manifest)
    #[serde(default)]
    pub annotations: HashMap<String, String>,
}

/// Local content store for OCI artifacts
pub struct ContentStore {
    base_dir: PathBuf,
}

impl ContentStore {
    /// Create a new content store with the default base directory (~/.cagent/store)
    pub fn new() -> Result<Self> {
        Self::with_base_dir(paths::get_store_dir())
    }

    /// Create a new content store with a custom base directory
    pub fn with_base_dir(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir)?;
        Ok(Self { base_dir })
    }

    /// Get the path to the refs directory
    fn refs_dir(&self) -> PathBuf {
        self.base_dir.join("refs")
    }

    /// Get the path for an artifact tar file
    fn tar_path(&self, digest: &str) -> PathBuf {
        self.base_dir.join(format!("{}.tar", digest))
    }

    /// Get the path for artifact metadata
    fn metadata_path(&self, digest: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", digest))
    }

    /// Hash a reference to get a refs filename
    fn hash_reference(reference: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(reference.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Store an artifact (YAML content) and return its digest
    pub fn store_artifact(
        &self,
        content: &[u8],
        reference: &str,
        annotations: HashMap<String, String>,
    ) -> Result<String> {
        // Calculate digest of content
        let mut hasher = Sha256::new();
        hasher.update(content);
        let digest = format!("sha256:{:x}", hasher.finalize());

        debug!(
            digest = %digest,
            reference = %reference,
            size = content.len(),
            "Storing artifact"
        );

        // Create a simple tar archive with the content
        let tar_path = self.tar_path(&digest);
        {
            let file = std::fs::File::create(&tar_path)?;
            let mut tar = tar::Builder::new(file);

            // Add the agent.yaml file to the tar
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(Utc::now().timestamp() as u64);
            header.set_cksum();

            tar.append_data(&mut header, "agent.yaml", content)?;
            tar.finish()?;
        }

        // Get the tar file size
        let tar_size = std::fs::metadata(&tar_path)?.len();

        // Create metadata
        let metadata = ArtifactMetadata {
            digest: digest.clone(),
            reference: reference.to_string(),
            size: tar_size,
            stored_at: Utc::now(),
            labels: HashMap::new(),
            annotations,
        };

        // Save metadata
        let metadata_json = serde_json::to_string_pretty(&metadata)?;
        std::fs::write(self.metadata_path(&digest), metadata_json)?;

        // Create reference link
        self.create_reference_link(reference, &digest)?;

        info!(
            digest = %digest,
            reference = %reference,
            "Artifact stored"
        );

        Ok(digest)
    }

    /// Get artifact content by identifier (digest or reference)
    pub fn get_artifact(&self, identifier: &str) -> Result<String, StoreError> {
        let digest = self.resolve_identifier(identifier)?;
        let tar_path = self.tar_path(&digest);

        if !tar_path.exists() {
            return Err(StoreError::Corrupted);
        }

        // Open and read the tar
        let file = std::fs::File::open(&tar_path)?;
        let mut archive = tar::Archive::new(file);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?;

            // Look for agent.yaml
            if path.to_string_lossy().ends_with("agent.yaml")
                || path.to_string_lossy().ends_with("agent.yml")
            {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                return Ok(content);
            }
        }

        // No agent.yaml found
        Err(StoreError::Corrupted)
    }

    /// Get artifact metadata
    pub fn get_artifact_metadata(&self, identifier: &str) -> Result<ArtifactMetadata, StoreError> {
        let digest = self.resolve_identifier(identifier)?;
        let metadata_path = self.metadata_path(&digest);

        if !metadata_path.exists() {
            return Err(StoreError::Corrupted);
        }

        let content = std::fs::read_to_string(&metadata_path)?;
        let metadata: ArtifactMetadata = serde_json::from_str(&content)?;
        Ok(metadata)
    }

    /// List all stored artifacts
    pub fn list_artifacts(&self) -> Result<Vec<ArtifactMetadata>> {
        let mut artifacts = Vec::new();

        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) == Some("tar") {
                let digest = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default();

                if let Ok(metadata) = self.get_artifact_metadata(digest) {
                    artifacts.push(metadata);
                }
            }
        }

        // Sort by stored_at descending
        artifacts.sort_by(|a, b| b.stored_at.cmp(&a.stored_at));

        Ok(artifacts)
    }

    /// Delete an artifact
    pub fn delete_artifact(&self, identifier: &str) -> Result<(), StoreError> {
        let digest = self.resolve_identifier(identifier)?;

        // Remove tar
        let tar_path = self.tar_path(&digest);
        if tar_path.exists() {
            std::fs::remove_file(&tar_path)?;
        }

        // Remove metadata
        let metadata_path = self.metadata_path(&digest);
        if metadata_path.exists() {
            std::fs::remove_file(&metadata_path)?;
        }

        // Remove reference links
        self.remove_reference_links(&digest)?;

        Ok(())
    }

    /// Resolve identifier (digest or reference) to a digest
    fn resolve_identifier(&self, identifier: &str) -> Result<String, StoreError> {
        // If it's already a digest, return it
        if identifier.starts_with("sha256:") {
            return Ok(identifier.to_string());
        }

        // Normalize reference (add :latest if no tag)
        let reference = if identifier.contains(':') {
            identifier.to_string()
        } else {
            format!("{}:latest", identifier)
        };

        // Look up in refs
        self.resolve_reference(&reference)
    }

    /// Resolve a reference to a digest
    fn resolve_reference(&self, reference: &str) -> Result<String, StoreError> {
        let refs_dir = self.refs_dir();
        let ref_hash = Self::hash_reference(reference);
        let ref_file = refs_dir.join(&ref_hash);

        if !ref_file.exists() {
            return Err(StoreError::NotFound(reference.to_string()));
        }

        let digest = std::fs::read_to_string(&ref_file)?;
        Ok(digest.trim().to_string())
    }

    /// Create a reference link
    fn create_reference_link(&self, reference: &str, digest: &str) -> Result<()> {
        let refs_dir = self.refs_dir();
        std::fs::create_dir_all(&refs_dir)?;

        let ref_hash = Self::hash_reference(reference);
        let ref_file = refs_dir.join(&ref_hash);

        std::fs::write(&ref_file, digest)?;
        Ok(())
    }

    /// Remove all reference links pointing to a digest
    fn remove_reference_links(&self, digest: &str) -> Result<(), StoreError> {
        let refs_dir = self.refs_dir();

        if !refs_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&refs_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                let content = std::fs::read_to_string(&path)?;
                if content.trim() == digest {
                    std::fs::remove_file(&path)?;
                }
            }
        }

        Ok(())
    }
}

/// Parse an OCI reference string into a Reference
fn parse_reference(reference: &str) -> Result<Reference> {
    // Add default registry if not present
    let full_ref = if reference.contains('.') || reference.starts_with("localhost") {
        reference.to_string()
    } else {
        // Default to Docker Hub for short references like "namespace/repo:tag"
        format!("docker.io/{}", reference)
    };

    // Add :latest if no tag
    let with_tag = if full_ref.contains(':') {
        full_ref
    } else {
        format!("{}:latest", full_ref)
    };

    with_tag
        .parse()
        .with_context(|| format!("Invalid OCI reference: {}", reference))
}

/// Get registry authentication from environment or Docker config
fn get_auth(registry: &str) -> RegistryAuth {
    // Try environment variables first
    let username_var = format!(
        "{}_USERNAME",
        registry.to_uppercase().replace(['.', '-'], "_")
    );
    let password_var = format!(
        "{}_PASSWORD",
        registry.to_uppercase().replace(['.', '-'], "_")
    );

    if let (Ok(username), Ok(password)) = (std::env::var(&username_var), std::env::var(&password_var))
    {
        debug!(registry = %registry, "Using auth from environment variables");
        return RegistryAuth::Basic(username, password);
    }

    // Try Docker Hub specific env vars
    if registry == "docker.io" || registry == "registry-1.docker.io" {
        if let (Ok(username), Ok(password)) = (
            std::env::var("DOCKER_USERNAME"),
            std::env::var("DOCKER_PASSWORD"),
        ) {
            debug!(registry = %registry, "Using Docker Hub auth from environment");
            return RegistryAuth::Basic(username, password);
        }
    }

    // Try to read from Docker config file
    if let Some(auth) = try_docker_config_auth(registry) {
        debug!(registry = %registry, "Using auth from Docker config");
        return auth;
    }

    debug!(registry = %registry, "No auth found, using anonymous");
    RegistryAuth::Anonymous
}

/// Try to read authentication from Docker config.json
fn try_docker_config_auth(registry: &str) -> Option<RegistryAuth> {
    let home = dirs::home_dir()?;
    let config_path = home.join(".docker").join("config.json");

    if !config_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&config_path).ok()?;

    #[derive(Deserialize)]
    struct DockerConfig {
        auths: Option<HashMap<String, AuthEntry>>,
    }

    #[derive(Deserialize)]
    struct AuthEntry {
        auth: Option<String>,
    }

    let config: DockerConfig = serde_json::from_str(&content).ok()?;
    let auths = config.auths?;

    // Try different registry URL formats
    let registries_to_try = [
        registry.to_string(),
        format!("https://{}", registry),
        format!("https://{}/v2/", registry),
        format!("https://{}/v1/", registry),
    ];

    // Special case for Docker Hub
    let docker_hub_variants = if registry == "docker.io" || registry == "registry-1.docker.io" {
        vec![
            "https://index.docker.io/v1/".to_string(),
            "https://index.docker.io/v2/".to_string(),
            "docker.io".to_string(),
            "index.docker.io".to_string(),
        ]
    } else {
        vec![]
    };

    for reg in registries_to_try
        .iter()
        .chain(docker_hub_variants.iter())
    {
        if let Some(entry) = auths.get(reg) {
            if let Some(auth) = &entry.auth {
                // Decode base64 auth (username:password)
                if let Ok(decoded) = base64_decode(auth) {
                    if let Some((username, password)) = decoded.split_once(':') {
                        return Some(RegistryAuth::Basic(
                            username.to_string(),
                            password.to_string(),
                        ));
                    }
                }
            }
        }
    }

    None
}

/// Simple base64 decode helper
fn base64_decode(input: &str) -> Result<String, std::string::FromUtf8Error> {
    // Simple base64 decoding without additional dependencies
    let input = input.trim();
    let mut result = Vec::with_capacity(input.len() * 3 / 4);

    let table: Vec<u8> = (b'A'..=b'Z')
        .chain(b'a'..=b'z')
        .chain(b'0'..=b'9')
        .chain([b'+', b'/'])
        .collect();

    let decode_table: [u8; 256] = {
        let mut t = [255u8; 256];
        for (i, &c) in table.iter().enumerate() {
            t[c as usize] = i as u8;
        }
        t
    };

    let input_bytes: Vec<u8> = input.bytes().filter(|&b| b != b'=').collect();

    for chunk in input_bytes.chunks(4) {
        let mut buf = 0u32;
        let mut bits = 0;

        for &c in chunk {
            let val = decode_table[c as usize];
            if val == 255 {
                continue;
            }
            buf = (buf << 6) | val as u32;
            bits += 6;
        }

        while bits >= 8 {
            bits -= 8;
            result.push((buf >> bits) as u8);
        }
    }

    String::from_utf8(result)
}

/// Create an OCI client
fn create_client() -> Client {
    let config = ClientConfig {
        protocol: oci_client::client::ClientProtocol::Https,
        ..Default::default()
    };
    Client::new(config)
}

/// Convert HashMap to BTreeMap for OCI layer annotations
#[allow(dead_code)]
fn to_btree_map(map: HashMap<String, String>) -> BTreeMap<String, String> {
    map.into_iter().collect()
}

/// Convert BTreeMap to HashMap
fn from_btree_map(map: BTreeMap<String, String>) -> HashMap<String, String> {
    map.into_iter().collect()
}

/// Push an agent YAML to a remote OCI registry
///
/// This function packages the agent as an OCI artifact and pushes it to the registry.
pub async fn push_agent(yaml_path: impl AsRef<Path>, reference: &str) -> Result<String> {
    let yaml_path = yaml_path.as_ref();
    let content = std::fs::read(yaml_path)
        .with_context(|| format!("Reading agent file: {}", yaml_path.display()))?;

    // Parse the YAML to get metadata
    #[derive(Default, Deserialize)]
    struct Metadata {
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        license: Option<String>,
        #[serde(default)]
        version: Option<String>,
    }

    #[derive(Default, Deserialize)]
    struct Config {
        #[serde(default)]
        metadata: Metadata,
    }

    let config: Config = serde_yaml::from_slice(&content).unwrap_or(Config {
        metadata: Metadata {
            author: None,
            license: None,
            version: None,
        },
    });

    // Build manifest annotations
    let mut manifest_annotations = BTreeMap::new();
    manifest_annotations.insert(
        annotations::CAGENT_VERSION.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    manifest_annotations.insert(annotations::CREATED.to_string(), Utc::now().to_rfc3339());
    manifest_annotations.insert(
        annotations::DESCRIPTION.to_string(),
        format!(
            "cagent agent: {}",
            yaml_path.file_name().unwrap_or_default().to_string_lossy()
        ),
    );

    if let Some(author) = &config.metadata.author {
        manifest_annotations.insert(annotations::AUTHORS.to_string(), author.clone());
    }
    if let Some(license) = &config.metadata.license {
        manifest_annotations.insert(annotations::LICENSES.to_string(), license.clone());
    }
    if let Some(version) = &config.metadata.version {
        manifest_annotations.insert(annotations::REVISION.to_string(), version.clone());
    }

    // Parse the reference
    let oci_ref = parse_reference(reference)?;

    info!(reference = %oci_ref, "Pushing agent to OCI registry");

    // Create OCI client
    let client = create_client();
    let auth = get_auth(oci_ref.registry());

    // Create config blob (empty JSON object for artifacts)
    let config_data = OciConfig {
        data: b"{}".to_vec().into(),
        media_type: media_types::CONFIG.to_string(),
        annotations: None,
    };

    // Create layer annotations
    let mut layer_annotations = BTreeMap::new();
    layer_annotations.insert(
        "org.opencontainers.image.title".to_string(),
        yaml_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );

    // Create the image layers (just the YAML content)
    let layers = vec![ImageLayer::new(
        content.clone(),
        media_types::AGENT_YAML.to_string(),
        Some(layer_annotations),
    )];

    // Create the manifest with annotations
    let mut manifest = OciImageManifest::build(&layers, &config_data, None);
    manifest.annotations = Some(manifest_annotations);

    // Push the image
    let push_response = client
        .push(&oci_ref, &layers, config_data, &auth, Some(manifest))
        .await
        .with_context(|| format!("Pushing to registry: {}", oci_ref))?;

    let manifest_digest = push_response.manifest_url;

    info!(
        reference = %oci_ref,
        digest = %manifest_digest,
        "Agent pushed to OCI registry"
    );

    // Also store locally for caching
    let store = ContentStore::new()?;
    let local_annotations = HashMap::new();
    let _ = store.store_artifact(&content, reference, local_annotations);

    Ok(manifest_digest)
}

/// Pull an agent from an OCI registry
///
/// This function pulls the agent artifact and extracts the YAML content.
pub async fn pull_agent(reference: &str) -> Result<String> {
    let store = ContentStore::new()?;

    // Normalize reference (add :latest if no tag)
    let reference = if reference.contains(':') {
        reference.to_string()
    } else {
        format!("{}:latest", reference)
    };

    info!(reference = %reference, "Pulling agent from OCI registry");

    // Try to get from local store first
    match store.get_artifact(&reference) {
        Ok(content) => {
            debug!(reference = %reference, "Found agent in local store");
            return Ok(content);
        }
        Err(StoreError::NotFound(_)) => {
            debug!(reference = %reference, "Agent not in local store, fetching from registry");
        }
        Err(StoreError::Corrupted) => {
            warn!(reference = %reference, "Local store corrupted, re-fetching from registry");
        }
        Err(e) => {
            return Err(e.into());
        }
    }

    // Parse the reference
    let oci_ref = parse_reference(&reference)?;

    // Create OCI client
    let client = create_client();
    let auth = get_auth(oci_ref.registry());

    // Pull the manifest first to get layer info
    let (manifest, _digest) = client
        .pull_manifest(&oci_ref, &auth)
        .await
        .with_context(|| format!("Pulling manifest from registry: {}", oci_ref))?;

    // Find the YAML layer
    let image_manifest = match manifest {
        oci_client::manifest::OciManifest::Image(m) => m,
        oci_client::manifest::OciManifest::ImageIndex(idx) => {
            // For image index, we need to pick the first manifest
            // (agent artifacts should only have one anyway)
            if let Some(first) = idx.manifests.first() {
                let manifest_ref: Reference = format!(
                    "{}/{}@{}",
                    oci_ref.registry(),
                    oci_ref.repository(),
                    first.digest
                )
                .parse()?;
                let (m, _) = client.pull_manifest(&manifest_ref, &auth).await?;
                match m {
                    oci_client::manifest::OciManifest::Image(im) => im,
                    _ => anyhow::bail!("Expected image manifest, got index"),
                }
            } else {
                anyhow::bail!("Empty image index");
            }
        }
    };

    // Find the YAML layer
    let yaml_layer = image_manifest
        .layers
        .iter()
        .find(|l| l.media_type == media_types::AGENT_YAML || l.media_type.contains("yaml"))
        .or_else(|| image_manifest.layers.first())
        .ok_or_else(|| anyhow::anyhow!("No layers found in manifest"))?;

    debug!(
        digest = %yaml_layer.digest,
        media_type = %yaml_layer.media_type,
        "Found YAML layer"
    );

    // Pull the layer content using streaming
    let mut stream = client
        .pull_blob_stream(&oci_ref, yaml_layer)
        .await
        .with_context(|| format!("Pulling layer: {}", yaml_layer.digest))?;

    // Collect all the bytes
    let mut content = Vec::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Reading blob stream")?;
        content.extend_from_slice(&bytes);
    }

    let content_str =
        String::from_utf8(content.clone()).context("Layer content is not valid UTF-8")?;

    // Store locally for caching
    let annotations = from_btree_map(image_manifest.annotations.unwrap_or_default());
    store.store_artifact(&content, &reference, annotations)?;

    info!(reference = %reference, "Agent pulled from OCI registry");

    Ok(content_str)
}

/// Check if a reference looks like an OCI registry reference
pub fn is_oci_reference(s: &str) -> bool {
    // A reference looks like: namespace/repo or namespace/repo:tag
    // It should contain a slash and not be a file path
    if s.is_empty() || s.starts_with('/') || s.starts_with('.') {
        return false;
    }

    // Check if it looks like a path with common extensions
    if s.ends_with(".yaml") || s.ends_with(".yml") || s.ends_with(".json") {
        return false;
    }

    // Must contain at least one slash (namespace/repo)
    if !s.contains('/') {
        return false;
    }

    // Should not look like a file path
    if std::path::Path::new(s).exists() {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_content_store_roundtrip() {
        let dir = tempdir().unwrap();
        let store = ContentStore::with_base_dir(dir.path()).unwrap();

        let content = b"agents:\n  root:\n    model: test\n";
        let reference = "test/agent:v1";
        let annotations = HashMap::new();

        // Store
        let digest = store
            .store_artifact(content, reference, annotations)
            .unwrap();
        assert!(digest.starts_with("sha256:"));

        // Get by reference
        let retrieved = store.get_artifact(reference).unwrap();
        assert_eq!(retrieved.as_bytes(), content);

        // Get by digest
        let retrieved2 = store.get_artifact(&digest).unwrap();
        assert_eq!(retrieved2.as_bytes(), content);

        // Get metadata
        let metadata = store.get_artifact_metadata(reference).unwrap();
        assert_eq!(metadata.digest, digest);
        assert_eq!(metadata.reference, reference);

        // List
        let artifacts = store.list_artifacts().unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].digest, digest);

        // Delete
        store.delete_artifact(reference).unwrap();
        assert!(store.get_artifact(reference).is_err());
    }

    #[test]
    fn test_is_oci_reference() {
        // Valid OCI references
        assert!(is_oci_reference("namespace/repo"));
        assert!(is_oci_reference("namespace/repo:tag"));
        assert!(is_oci_reference("docker.io/library/nginx"));
        assert!(is_oci_reference("ghcr.io/owner/agent:v1.0"));
        assert!(is_oci_reference("agentcatalog/pirate"));

        // Invalid - file paths
        assert!(!is_oci_reference("/etc/agent.yaml"));
        assert!(!is_oci_reference("./agent.yaml"));
        assert!(!is_oci_reference("agent.yaml"));
        assert!(!is_oci_reference("path/to/agent.yaml"));

        // Invalid - no namespace
        assert!(!is_oci_reference("repo"));
        assert!(!is_oci_reference("repo:tag"));

        // Invalid - empty
        assert!(!is_oci_reference(""));
    }

    #[test]
    fn test_hash_reference() {
        let hash1 = ContentStore::hash_reference("test/repo:v1");
        let hash2 = ContentStore::hash_reference("test/repo:v1");
        let hash3 = ContentStore::hash_reference("test/repo:v2");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 hex
    }

    #[test]
    fn test_parse_reference() {
        // Short reference (Docker Hub)
        let r = parse_reference("namespace/repo").unwrap();
        assert_eq!(r.registry(), "docker.io");
        assert_eq!(r.repository(), "namespace/repo");
        assert_eq!(r.tag(), Some("latest"));

        // With tag
        let r = parse_reference("namespace/repo:v1").unwrap();
        assert_eq!(r.tag(), Some("v1"));

        // With explicit registry
        let r = parse_reference("ghcr.io/owner/repo:tag").unwrap();
        assert_eq!(r.registry(), "ghcr.io");
        assert_eq!(r.repository(), "owner/repo");
        assert_eq!(r.tag(), Some("tag"));
    }

    #[test]
    fn test_base64_decode() {
        // "user:password" in base64
        let decoded = base64_decode("dXNlcjpwYXNzd29yZA==").unwrap();
        assert_eq!(decoded, "user:password");

        // Empty string
        let decoded = base64_decode("").unwrap();
        assert_eq!(decoded, "");

        // Just username
        let decoded = base64_decode("dXNlcg==").unwrap();
        assert_eq!(decoded, "user");
    }

    #[test]
    fn test_btree_hashmap_conversion() {
        let mut hashmap = HashMap::new();
        hashmap.insert("key1".to_string(), "value1".to_string());
        hashmap.insert("key2".to_string(), "value2".to_string());

        let btree = to_btree_map(hashmap.clone());
        let back = from_btree_map(btree);

        assert_eq!(hashmap, back);
    }
}
