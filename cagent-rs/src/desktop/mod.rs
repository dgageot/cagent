//! Docker Desktop integration
//!
//! This module provides communication with Docker Desktop's backend API
//! to retrieve user information and authentication tokens.

use std::io::{BufRead, BufReader, Write};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;
use tracing::debug;

/// Docker Hub user information
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DockerHubInfo {
    #[serde(rename = "id", default)]
    pub username: String,
    #[serde(default)]
    pub email: String,
}

/// Paths to Docker Desktop sockets
#[derive(Debug, Clone)]
pub struct DockerDesktopPaths {
    pub backend_socket: String,
}

/// Get Docker Desktop paths (cached)
pub fn get_paths() -> &'static DockerDesktopPaths {
    static PATHS: OnceLock<DockerDesktopPaths> = OnceLock::new();
    PATHS.get_or_init(|| {
        get_docker_desktop_paths().unwrap_or_else(|_| DockerDesktopPaths {
            backend_socket: String::new(),
        })
    })
}

#[cfg(target_os = "macos")]
fn get_docker_desktop_paths() -> Result<DockerDesktopPaths> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Unable to get home directory"))?;
    let data = home
        .join("Library")
        .join("Containers")
        .join("com.docker.docker")
        .join("Data");

    Ok(DockerDesktopPaths {
        backend_socket: data.join("backend.sock").to_string_lossy().to_string(),
    })
}

#[cfg(target_os = "linux")]
fn get_docker_desktop_paths() -> Result<DockerDesktopPaths> {
    use std::path::Path;

    // Inside LinuxKit
    let linuxkit_path = "/run/host-services/backend.sock";
    if Path::new(linuxkit_path).exists() {
        return Ok(DockerDesktopPaths {
            backend_socket: linuxkit_path.to_string(),
        });
    }

    // Inside WSL2
    let wsl_path = "/mnt/wsl/docker-desktop/shared-sockets/host-services/backend.sock";
    if Path::new(wsl_path).exists() {
        return Ok(DockerDesktopPaths {
            backend_socket: wsl_path.to_string(),
        });
    }

    // On native Linux
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Unable to get home directory"))?;
    Ok(DockerDesktopPaths {
        backend_socket: home
            .join(".docker")
            .join("desktop")
            .join("backend.sock")
            .to_string_lossy()
            .to_string(),
    })
}

#[cfg(target_os = "windows")]
fn get_docker_desktop_paths() -> Result<DockerDesktopPaths> {
    Ok(DockerDesktopPaths {
        backend_socket: r"\\.\pipe\dockerBackendApiServer".to_string(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn get_docker_desktop_paths() -> Result<DockerDesktopPaths> {
    anyhow::bail!("Unsupported platform for Docker Desktop")
}

/// Client for communicating with Docker Desktop backend
pub struct DesktopClient {
    timeout: Duration,
}

impl Default for DesktopClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopClient {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }

    /// Make a GET request to the Docker Desktop backend API (Unix socket)
    #[cfg(unix)]
    fn get_blocking<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T> {
        use std::os::unix::net::UnixStream;

        let socket_path = &get_paths().backend_socket;
        if socket_path.is_empty() {
            anyhow::bail!("Docker Desktop socket path not available");
        }

        debug!("Connecting to Docker Desktop backend at {}", socket_path);

        let mut stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        // Send HTTP request
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            endpoint
        );
        stream.write_all(request.as_bytes())?;

        // Read response
        let mut reader = BufReader::new(stream);

        // Read status line
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;

        // Read headers until empty line
        let mut content_length: Option<usize> = None;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some(len_str) = header.strip_prefix("Content-Length: ") {
                content_length = len_str.trim().parse().ok();
            }
        }

        // Read body
        let body = if let Some(len) = content_length {
            let mut buf = vec![0u8; len];
            std::io::Read::read_exact(&mut reader, &mut buf)?;
            buf
        } else {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut buf)?;
            buf
        };

        let result: T = serde_json::from_slice(&body)?;
        Ok(result)
    }

    /// Make a GET request to the Docker Desktop backend API (Windows named pipe)
    #[cfg(windows)]
    fn get_blocking<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T> {
        use std::fs::OpenOptions;
        use std::io::Read;

        let pipe_path = &get_paths().backend_socket;
        if pipe_path.is_empty() {
            anyhow::bail!("Docker Desktop pipe path not available");
        }

        debug!("Connecting to Docker Desktop backend at {}", pipe_path);

        let mut file = OpenOptions::new().read(true).write(true).open(pipe_path)?;

        // Send HTTP request
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            endpoint
        );
        file.write_all(request.as_bytes())?;

        // Read response
        let mut reader = BufReader::new(file);

        // Read status line
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;

        // Read headers until empty line
        let mut content_length: Option<usize> = None;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header)?;
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some(len_str) = header.strip_prefix("Content-Length: ") {
                content_length = len_str.trim().parse().ok();
            }
        }

        // Read body
        let body = if let Some(len) = content_length {
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            buf
        } else {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf)?;
            buf
        };

        let result: T = serde_json::from_slice(&body)?;
        Ok(result)
    }

    /// Get the Docker registry token
    pub fn get_token(&self) -> Result<String> {
        let token: String = self.get_blocking("/registry/token")?;
        Ok(token)
    }

    /// Get Docker Hub user information
    pub fn get_user_info(&self) -> Result<DockerHubInfo> {
        self.get_blocking("/registry/username")
    }

    /// Check if Docker Desktop is running
    pub fn is_running(&self) -> bool {
        self.get_blocking::<serde_json::Value>("/ping").is_ok()
    }
}

/// Get the Docker token from Docker Desktop (blocking)
pub fn get_token_blocking() -> Option<String> {
    DesktopClient::new().get_token().ok()
}

/// Get Docker Hub user info from Docker Desktop (blocking)
pub fn get_user_info_blocking() -> Option<DockerHubInfo> {
    DesktopClient::new().get_user_info().ok()
}

/// Check if Docker Desktop is running (blocking)
pub fn is_docker_desktop_running_blocking() -> bool {
    DesktopClient::new().is_running()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_paths() {
        let paths = get_paths();
        // Just verify it doesn't panic and returns something
        // On supported platforms, backend_socket should be non-empty
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        assert!(
            !paths.backend_socket.is_empty(),
            "backend_socket should be set on supported platforms"
        );

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        assert!(
            paths.backend_socket.is_empty(),
            "backend_socket should be empty on unsupported platforms"
        );
    }

    #[test]
    fn test_desktop_client_creation() {
        let client = DesktopClient::new();
        assert_eq!(client.timeout, Duration::from_secs(10));
    }
}
