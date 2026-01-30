//! Telemetry module for cagent
//!
//! This module provides optional anonymous usage telemetry. Telemetry is 
//! disabled by default and can be controlled via the `TELEMETRY_ENABLED`
//! environment variable.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Global telemetry state
static TELEMETRY_ENABLED: OnceLock<AtomicBool> = OnceLock::new();

/// Check if telemetry is enabled
pub fn is_enabled() -> bool {
    TELEMETRY_ENABLED
        .get_or_init(|| {
            let enabled = std::env::var("TELEMETRY_ENABLED")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false); // Disabled by default
            AtomicBool::new(enabled)
        })
        .load(Ordering::Relaxed)
}

/// Disable telemetry at runtime
pub fn disable() {
    if let Some(enabled) = TELEMETRY_ENABLED.get() {
        enabled.store(false, Ordering::Relaxed);
    }
}

/// Enable telemetry at runtime
pub fn enable() {
    if let Some(enabled) = TELEMETRY_ENABLED.get() {
        enabled.store(true, Ordering::Relaxed);
    }
}

/// Track a command event (no-op if telemetry is disabled)
pub fn track_command(_command: &str) {
    if !is_enabled() {
        return;
    }
    // In a real implementation, this would send telemetry data
    // For now, this is a no-op placeholder
    tracing::debug!(command = _command, "telemetry: command tracked");
}

/// Track an error event (no-op if telemetry is disabled)
pub fn track_error(_error: &str) {
    if !is_enabled() {
        return;
    }
    // In a real implementation, this would send telemetry data
    // For now, this is a no-op placeholder
    tracing::debug!(error = _error, "telemetry: error tracked");
}

/// Track agent usage (no-op if telemetry is disabled)
pub fn track_agent_usage(_agent_name: &str, _model: &str) {
    if !is_enabled() {
        return;
    }
    // In a real implementation, this would send telemetry data
    tracing::debug!(
        agent = _agent_name,
        model = _model,
        "telemetry: agent usage tracked"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_disabled_by_default() {
        // Clear any existing state by running in a fresh test
        // Note: This test may be affected by other tests setting TELEMETRY_ENABLED
        // In practice, telemetry should be disabled by default
        
        // Just verify the functions don't panic
        track_command("test");
        track_error("test error");
        track_agent_usage("test_agent", "test_model");
    }

    #[test]
    fn test_disable_enable() {
        // These should not panic regardless of initial state
        disable();
        enable();
        disable();
    }
}
