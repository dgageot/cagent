//! Tool permission checking based on configurable Allow/Ask/Deny patterns.
//!
//! This module provides a way to control tool execution permissions:
//! - **Allow**: Tools matching these patterns are auto-approved (like --yolo for specific tools)
//! - **Ask**: Tools not matching any pattern require user confirmation (default)
//! - **Deny**: Tools matching these patterns are always rejected, even with --yolo
//!
//! Evaluation order: Deny (checked first), then Allow, then Ask (default)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission decision for a tool call
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Tool requires user approval (default behavior)
    Ask,
    /// Tool is auto-approved without user confirmation
    Allow,
    /// Tool is rejected and should not be executed
    Deny,
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Decision::Ask => write!(f, "ask"),
            Decision::Allow => write!(f, "allow"),
            Decision::Deny => write!(f, "deny"),
        }
    }
}

/// Permission configuration with Allow/Deny patterns
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PermissionsConfig {
    /// Tool name patterns that are auto-approved without user confirmation
    #[serde(default)]
    pub allow: Vec<String>,
    /// Tool name patterns that are always rejected
    #[serde(default)]
    pub deny: Vec<String>,
}

/// Permission checker that evaluates tool permissions based on configured patterns
#[derive(Debug, Clone)]
pub struct Checker {
    allow_patterns: Vec<String>,
    deny_patterns: Vec<String>,
}

impl Default for Checker {
    fn default() -> Self {
        Self::new()
    }
}

impl Checker {
    /// Create a new empty permission checker
    pub fn new() -> Self {
        Self {
            allow_patterns: Vec::new(),
            deny_patterns: Vec::new(),
        }
    }

    /// Create a permission checker from config
    pub fn from_config(cfg: &PermissionsConfig) -> Self {
        Self {
            allow_patterns: cfg.allow.clone(),
            deny_patterns: cfg.deny.clone(),
        }
    }

    /// Check permission for a tool name without arguments
    pub fn check(&self, tool_name: &str) -> Decision {
        self.check_with_args(tool_name, None)
    }

    /// Check permission for a tool name with optional arguments
    ///
    /// Evaluation order: Deny (checked first), then Allow, then Ask (default)
    ///
    /// Patterns support:
    /// - Simple tool names: "shell", "read_*"
    /// - Argument matching: "shell:cmd=ls*" matches shell tool with cmd argument starting with "ls"
    /// - Multiple arguments: "shell:cmd=ls*:cwd=/home/*" matches both conditions
    /// - Glob patterns in both tool names and argument values
    pub fn check_with_args(
        &self,
        tool_name: &str,
        args: Option<&HashMap<String, serde_json::Value>>,
    ) -> Decision {
        // Deny patterns are checked first - they take priority
        for pattern in &self.deny_patterns {
            if match_tool_pattern(pattern, tool_name, args) {
                return Decision::Deny;
            }
        }

        // Allow patterns are checked second
        for pattern in &self.allow_patterns {
            if match_tool_pattern(pattern, tool_name, args) {
                return Decision::Allow;
            }
        }

        // Default is Ask
        Decision::Ask
    }

    /// Returns true if no permissions are configured
    pub fn is_empty(&self) -> bool {
        self.allow_patterns.is_empty() && self.deny_patterns.is_empty()
    }

    /// Get the allow patterns
    pub fn allow_patterns(&self) -> &[String] {
        &self.allow_patterns
    }

    /// Get the deny patterns
    pub fn deny_patterns(&self) -> &[String] {
        &self.deny_patterns
    }
}

/// Parse a permission pattern into tool name pattern and argument conditions.
/// Pattern format: "toolname" or "toolname:arg1=val1:arg2=val2"
fn parse_pattern(pattern: &str) -> (&str, HashMap<&str, &str>) {
    let mut arg_patterns = HashMap::new();
    let parts: Vec<&str> = pattern.split(':').collect();

    if parts.is_empty() {
        return (pattern, arg_patterns);
    }

    // First part is always part of the tool name
    let mut tool_parts = vec![parts[0]];
    let mut found_args = false;

    for part in parts.iter().skip(1) {
        if let Some((key, value)) = part.split_once('=') {
            if !key.is_empty() {
                // This is an argument pattern
                arg_patterns.insert(key, value);
                found_args = true;
            }
        } else if !found_args {
            // No = found and we haven't started args yet, so it's part of tool name
            tool_parts.push(part);
        }
        // If we've started collecting args but this part has no =, skip it
    }

    let _tool_pattern = if tool_parts.len() == 1 {
        tool_parts[0]
    } else {
        // We need to reconstruct the tool name with colons
        // This is a bit tricky since we can't return a reference to a local String
        // For now, we'll match the first part only in complex cases
        // This handles simple tool names like "shell" but won't support "mcp:github:create_issue"
        // with argument patterns - that's an edge case we can address later
        parts[0]
    };

    // Re-parse to handle colons in tool names properly
    let full_pattern = pattern;
    let mut split_point = None;

    for (i, part) in parts.iter().enumerate().skip(1) {
        if part.contains('=') {
            // Found start of arguments
            split_point = Some(i);
            break;
        }
    }

    if let Some(sp) = split_point {
        let tool_name = parts[..sp].join(":");
        let tool_pattern_str = Box::leak(tool_name.into_boxed_str());

        for part in parts.iter().skip(sp) {
            if let Some((key, value)) = part.split_once('=') {
                if !key.is_empty() {
                    arg_patterns.insert(key, value);
                }
            }
        }

        (tool_pattern_str, arg_patterns)
    } else {
        (full_pattern, HashMap::new())
    }
}

/// Check if a tool name and its arguments match a pattern
fn match_tool_pattern(
    pattern: &str,
    tool_name: &str,
    args: Option<&HashMap<String, serde_json::Value>>,
) -> bool {
    let (tool_pattern, arg_patterns) = parse_pattern(pattern);

    // First check if the tool name matches
    if !match_glob(tool_pattern, tool_name) {
        return false;
    }

    // If no argument patterns, we're done - tool name matched
    if arg_patterns.is_empty() {
        return true;
    }

    // If pattern has argument conditions but no args provided, no match
    let Some(args) = args else {
        return false;
    };

    // All argument patterns must match
    for (arg_name, arg_pattern) in arg_patterns {
        let Some(arg_value) = args.get(arg_name) else {
            return false;
        };

        let arg_str = arg_to_string(arg_value);
        if !match_glob(arg_pattern, &arg_str) {
            return false;
        }
    }

    true
}

/// Convert an argument value to a string for pattern matching
fn arg_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(f) = n.as_f64() {
                if f == f.trunc() {
                    (f as i64).to_string()
                } else {
                    f.to_string()
                }
            } else {
                n.to_string()
            }
        }
        serde_json::Value::Null => "null".to_string(),
        _ => v.to_string(),
    }
}

/// Check if a value matches a glob pattern (case-insensitive)
///
/// Supports glob-style patterns:
/// - "*" matches any sequence of characters
/// - "?" matches any single character
fn match_glob(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let value = value.to_lowercase();

    // Handle trailing wildcard for prefix matching
    // This allows "sudo*" to match "sudo rm -rf /"
    if pattern.ends_with('*') && !pattern.ends_with("\\*") {
        let prefix = &pattern[..pattern.len() - 1];
        // If prefix contains no other glob characters, do simple prefix match
        if !prefix.contains('*') && !prefix.contains('?') && !prefix.contains('[') {
            return value.starts_with(prefix);
        }
    }

    // Simple glob matching
    glob_match(&pattern, &value)
}

/// Simple glob pattern matching
fn glob_match(pattern: &str, value: &str) -> bool {
    let mut pattern_chars = pattern.chars().peekable();
    let mut value_chars = value.chars().peekable();

    while let Some(p) = pattern_chars.next() {
        match p {
            '*' => {
                // Match zero or more characters
                if pattern_chars.peek().is_none() {
                    // Trailing *, matches everything
                    return true;
                }

                // Try matching the rest of the pattern at each position
                let remaining_pattern: String = pattern_chars.collect();
                let mut remaining_value: String = value_chars.collect();

                loop {
                    if glob_match(&remaining_pattern, &remaining_value) {
                        return true;
                    }
                    if remaining_value.is_empty() {
                        return false;
                    }
                    remaining_value = remaining_value[1..].to_string();
                }
            }
            '?' => {
                // Match exactly one character
                if value_chars.next().is_none() {
                    return false;
                }
            }
            '\\' => {
                // Escape next character
                let escaped = pattern_chars.next();
                let v = value_chars.next();
                if escaped != v {
                    return false;
                }
            }
            c => {
                // Match literal character
                if value_chars.next() != Some(c) {
                    return false;
                }
            }
        }
    }

    // Pattern exhausted, value should also be exhausted
    value_chars.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_checker() {
        let checker = Checker::new();
        assert!(checker.is_empty());
        assert_eq!(checker.check("shell"), Decision::Ask);
    }

    #[test]
    fn test_allow_pattern() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec!["shell".to_string()],
            deny: vec![],
        });
        assert_eq!(checker.check("shell"), Decision::Allow);
        assert_eq!(checker.check("filesystem"), Decision::Ask);
    }

    #[test]
    fn test_deny_pattern() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec![],
            deny: vec!["shell".to_string()],
        });
        assert_eq!(checker.check("shell"), Decision::Deny);
        assert_eq!(checker.check("filesystem"), Decision::Ask);
    }

    #[test]
    fn test_deny_takes_priority() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec!["shell".to_string()],
            deny: vec!["shell".to_string()],
        });
        // Deny is checked first
        assert_eq!(checker.check("shell"), Decision::Deny);
    }

    #[test]
    fn test_wildcard_pattern() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec!["read_*".to_string()],
            deny: vec![],
        });
        assert_eq!(checker.check("read_file"), Decision::Allow);
        assert_eq!(checker.check("read_multiple_files"), Decision::Allow);
        assert_eq!(checker.check("write_file"), Decision::Ask);
    }

    #[test]
    fn test_argument_pattern() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec!["shell:cmd=ls*".to_string()],
            deny: vec![],
        });

        // With matching argument
        let mut args = HashMap::new();
        args.insert("cmd".to_string(), serde_json::json!("ls -la"));
        assert_eq!(
            checker.check_with_args("shell", Some(&args)),
            Decision::Allow
        );

        // With non-matching argument
        let mut args = HashMap::new();
        args.insert("cmd".to_string(), serde_json::json!("rm -rf /"));
        assert_eq!(checker.check_with_args("shell", Some(&args)), Decision::Ask);

        // Without arguments
        assert_eq!(checker.check("shell"), Decision::Ask);
    }

    #[test]
    fn test_case_insensitive() {
        let checker = Checker::from_config(&PermissionsConfig {
            allow: vec!["SHELL".to_string()],
            deny: vec![],
        });
        assert_eq!(checker.check("shell"), Decision::Allow);
        assert_eq!(checker.check("Shell"), Decision::Allow);
        assert_eq!(checker.check("SHELL"), Decision::Allow);
    }

    #[test]
    fn test_glob_matching() {
        assert!(glob_match("shell", "shell"));
        assert!(!glob_match("shell", "shells"));
        assert!(glob_match("shell*", "shell"));
        assert!(glob_match("shell*", "shells"));
        assert!(glob_match("*shell", "myshell"));
        assert!(glob_match("*shell*", "myshells"));
        assert!(glob_match("sh?ll", "shell"));
        assert!(!glob_match("sh?ll", "shll"));
    }

    #[test]
    fn test_arg_to_string() {
        assert_eq!(arg_to_string(&serde_json::json!("hello")), "hello");
        assert_eq!(arg_to_string(&serde_json::json!(true)), "true");
        assert_eq!(arg_to_string(&serde_json::json!(false)), "false");
        assert_eq!(arg_to_string(&serde_json::json!(42)), "42");
        assert_eq!(
            arg_to_string(&serde_json::json!(std::f64::consts::PI)),
            "3.141592653589793"
        );
        assert_eq!(arg_to_string(&serde_json::json!(3.0)), "3");
    }
}
