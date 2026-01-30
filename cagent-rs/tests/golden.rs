//! Golden file testing utilities for cagent
//!
//! Provides snapshot testing capabilities - comparing actual output
//! against expected "golden" files.
//!
//! # Usage
//!
//! ```ignore
//! use cagent::tests::golden::assert_golden;
//!
//! #[test]
//! fn test_output() {
//!     let output = generate_some_output();
//!     assert_golden("my_test.golden", &output);
//! }
//! ```
//!
//! To update golden files, run tests with UPDATE_GOLDEN=1:
//!
//! ```sh
//! UPDATE_GOLDEN=1 cargo test
//! ```

use std::fs;
use std::path::PathBuf;

/// Default directory for golden files relative to the crate root
pub const GOLDEN_DIR: &str = "tests/golden";

/// Assert that the actual content matches the golden file.
///
/// If the golden file doesn't exist and UPDATE_GOLDEN is set, creates it.
/// If UPDATE_GOLDEN is set, updates the golden file with actual content.
///
/// # Panics
///
/// Panics if the content doesn't match and UPDATE_GOLDEN is not set.
pub fn assert_golden(name: &str, actual: &str) {
    let golden_path = golden_file_path(name);
    let update = std::env::var("UPDATE_GOLDEN").is_ok();

    if update {
        // Create directory if needed
        if let Some(parent) = golden_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create golden directory");
        }
        fs::write(&golden_path, actual).expect("Failed to write golden file");
        println!("Updated golden file: {}", golden_path.display());
        return;
    }

    if !golden_path.exists() {
        panic!(
            "Golden file not found: {}\n\
             Run with UPDATE_GOLDEN=1 to create it:\n\
             UPDATE_GOLDEN=1 cargo test",
            golden_path.display()
        );
    }

    let expected = fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("Failed to read golden file {}: {}", golden_path.display(), e));

    if actual != expected {
        // Generate diff for better error messages
        let diff = diff_strings(&expected, actual);
        panic!(
            "Output doesn't match golden file: {}\n\n\
             {} DIFF:\n{}\n\n\
             Run with UPDATE_GOLDEN=1 to update:\n\
             UPDATE_GOLDEN=1 cargo test",
            golden_path.display(),
            "=".repeat(40),
            diff
        );
    }
}

/// Get the path for a golden file
pub fn golden_file_path(name: &str) -> PathBuf {
    // Try to find the crate root
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));

    manifest_dir.join(GOLDEN_DIR).join(name)
}

/// Create a simple text diff between two strings
fn diff_strings(expected: &str, actual: &str) -> String {
    let mut diff = String::new();
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();

    let max_lines = expected_lines.len().max(actual_lines.len());

    for i in 0..max_lines {
        let expected_line = expected_lines.get(i).map(|s| *s).unwrap_or("");
        let actual_line = actual_lines.get(i).map(|s| *s).unwrap_or("");

        if expected_line != actual_line {
            diff.push_str(&format!("Line {}:\n", i + 1));
            diff.push_str(&format!("  - {}\n", expected_line));
            diff.push_str(&format!("  + {}\n", actual_line));
        }
    }

    if diff.is_empty() {
        if expected_lines.len() != actual_lines.len() {
            diff.push_str(&format!(
                "Line count differs: expected {}, got {}",
                expected_lines.len(),
                actual_lines.len()
            ));
        } else {
            diff.push_str("(no visible differences - check whitespace?)");
        }
    }

    diff
}

/// Assert that the actual JSON matches the golden file.
///
/// Normalizes JSON before comparison (removes formatting differences).
#[allow(dead_code)]
pub fn assert_golden_json(name: &str, actual: &serde_json::Value) {
    let actual_str = serde_json::to_string_pretty(actual).expect("Failed to serialize actual JSON");
    assert_golden(name, &actual_str);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_strings_same() {
        let diff = diff_strings("hello\nworld", "hello\nworld");
        assert!(diff.contains("no visible differences"));
    }

    #[test]
    fn test_diff_strings_different() {
        let diff = diff_strings("hello\nworld", "hello\nearth");
        assert!(diff.contains("Line 2"));
        assert!(diff.contains("- world"));
        assert!(diff.contains("+ earth"));
    }

    #[test]
    fn test_golden_file_path() {
        let path = golden_file_path("test.golden");
        assert!(path.to_string_lossy().contains("tests/golden/test.golden"));
    }
}
