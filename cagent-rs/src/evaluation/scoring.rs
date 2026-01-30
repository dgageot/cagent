//! Scoring functions for evaluation results

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use super::types::{Result, Summary};

/// Size thresholds for response classification (character counts)
const SIZE_RANGES: &[(&str, (usize, usize))] = &[
    ("S", (0, 500)),
    ("M", (500, 1500)),
    ("L", (1500, 5000)),
    ("XL", (5000, usize::MAX)),
];

/// Classify a response by character count
pub fn get_response_size(response: &str) -> String {
    let length = response.len();
    for (label, (min, max)) in SIZE_RANGES {
        if length >= *min && length < *max {
            return label.to_string();
        }
    }
    "XL".to_string()
}

/// Calculate the F1 score between expected and actual tool calls
pub fn tool_call_f1_score(expected: &[String], actual: &[String]) -> f64 {
    if expected.is_empty() && actual.is_empty() {
        return 1.0;
    }
    if expected.is_empty() || actual.is_empty() {
        return 0.0;
    }

    let expected_counts = count_strings(expected);
    let actual_counts = count_strings(actual);

    let mut true_positives = 0;
    for (name, expected_count) in &expected_counts {
        if let Some(&actual_count) = actual_counts.get(name) {
            true_positives += (*expected_count).min(actual_count);
        }
    }

    let precision = true_positives as f64 / actual.len() as f64;
    let recall = true_positives as f64 / expected.len() as f64;

    if precision + recall == 0.0 {
        return 0.0;
    }

    2.0 * (precision * recall) / (precision + recall)
}

fn count_strings(strs: &[String]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for s in strs {
        *counts.entry(s.as_str()).or_insert(0) += 1;
    }
    counts
}

/// Count handoff tool calls
pub fn count_handoffs(tool_calls: &[String]) -> usize {
    tool_calls.iter().filter(|name| *name == "handoff").count()
}

/// Compute aggregate summary from evaluation results
pub fn compute_summary(results: &[Result]) -> Summary {
    let mut summary = Summary {
        total_evals: results.len(),
        ..Default::default()
    };

    for r in results {
        summary.total_cost += r.cost;

        if r.error.is_some() {
            summary.failed_evals += 1;
            continue;
        }

        if !r.size_expected.is_empty() {
            summary.sizes_total += 1;
            if r.size_expected == r.size {
                summary.sizes_passed += 1;
            }
        }

        summary.tools_total += r.tool_calls_score_expected;
        summary.tools_passed += r.tool_calls_score * r.tool_calls_score_expected;

        summary.handoffs_total += 1;
        if r.handoffs {
            summary.handoffs_passed += 1;
        }

        summary.relevance_total += r.relevance_expected;
        summary.relevance_passed += r.relevance;
    }

    summary
}

/// Print the evaluation summary
pub fn print_summary<W: Write>(out: &mut W, summary: &Summary, duration: Duration) {
    writeln!(out).ok();

    if summary.failed_evals > 0 {
        writeln!(
            out,
            "❌         Errors: {}/{} evaluations failed",
            summary.failed_evals, summary.total_evals
        )
        .ok();
    }

    print_metric(out, "Sizes", summary.sizes_passed as f64, summary.sizes_total as f64);
    print_metric(out, "Tool Calls", summary.tools_passed, summary.tools_total);
    print_metric(
        out,
        "Handoffs",
        summary.handoffs_passed as f64,
        summary.handoffs_total as f64,
    );
    print_metric(out, "Relevance", summary.relevance_passed, summary.relevance_total);

    writeln!(out).ok();
    writeln!(out, "Total Cost: ${:.6}", summary.total_cost).ok();
    writeln!(out, "Total Time: {:.0}s", duration.as_secs_f64()).ok();
}

fn print_metric<W: Write>(out: &mut W, label: &str, passed: f64, total: f64) {
    let ratio = if total > 0.0 { passed / total } else { 0.0 };
    let icon = status_icon(ratio);
    writeln!(
        out,
        "{} {:>14}: {:.0}/{:.0} passed ({:.1}%)",
        icon,
        label,
        passed,
        total,
        ratio * 100.0
    )
    .ok();
}

fn status_icon(ratio: f64) -> &'static str {
    if ratio > 0.75 {
        "✅"
    } else if ratio > 0.50 {
        "⚠️"
    } else {
        "❌"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_response_size() {
        assert_eq!(get_response_size(""), "S");
        assert_eq!(get_response_size(&"x".repeat(100)), "S");
        assert_eq!(get_response_size(&"x".repeat(500)), "M");
        assert_eq!(get_response_size(&"x".repeat(1000)), "M");
        assert_eq!(get_response_size(&"x".repeat(1500)), "L");
        assert_eq!(get_response_size(&"x".repeat(3000)), "L");
        assert_eq!(get_response_size(&"x".repeat(5000)), "XL");
        assert_eq!(get_response_size(&"x".repeat(10000)), "XL");
    }

    #[test]
    fn test_tool_call_f1_score() {
        // Both empty
        assert_eq!(tool_call_f1_score(&[], &[]), 1.0);

        // One empty
        assert_eq!(
            tool_call_f1_score(&["a".to_string()], &[]),
            0.0
        );
        assert_eq!(
            tool_call_f1_score(&[], &["a".to_string()]),
            0.0
        );

        // Perfect match
        assert_eq!(
            tool_call_f1_score(
                &["a".to_string(), "b".to_string()],
                &["a".to_string(), "b".to_string()]
            ),
            1.0
        );

        // Partial match
        let score = tool_call_f1_score(
            &["a".to_string(), "b".to_string()],
            &["a".to_string(), "c".to_string()],
        );
        assert!(score > 0.0 && score < 1.0);

        // No match
        assert_eq!(
            tool_call_f1_score(
                &["a".to_string(), "b".to_string()],
                &["c".to_string(), "d".to_string()]
            ),
            0.0
        );
    }

    #[test]
    fn test_count_handoffs() {
        assert_eq!(count_handoffs(&[]), 0);
        assert_eq!(
            count_handoffs(&["handoff".to_string(), "other".to_string()]),
            1
        );
        assert_eq!(
            count_handoffs(&["handoff".to_string(), "handoff".to_string()]),
            2
        );
    }

    #[test]
    fn test_compute_summary() {
        let results = vec![
            Result {
                cost: 0.01,
                size_expected: "M".to_string(),
                size: "M".to_string(),
                tool_calls_score_expected: 1.0,
                tool_calls_score: 1.0,
                handoffs: true,
                relevance_expected: 2.0,
                relevance: 2.0,
                ..Default::default()
            },
            Result {
                cost: 0.02,
                size_expected: "L".to_string(),
                size: "M".to_string(), // Wrong size
                tool_calls_score_expected: 1.0,
                tool_calls_score: 0.5, // Partial tool match
                handoffs: false,       // Handoff mismatch
                relevance_expected: 1.0,
                relevance: 0.0, // Relevance failed
                ..Default::default()
            },
        ];

        let summary = compute_summary(&results);
        assert_eq!(summary.total_evals, 2);
        assert_eq!(summary.failed_evals, 0);
        assert!((summary.total_cost - 0.03).abs() < f64::EPSILON);
        assert_eq!(summary.sizes_passed, 1);
        assert_eq!(summary.sizes_total, 2);
        assert!((summary.tools_passed - 1.5).abs() < f64::EPSILON);
        assert!((summary.tools_total - 2.0).abs() < f64::EPSILON);
        assert_eq!(summary.handoffs_passed, 1);
        assert_eq!(summary.handoffs_total, 2);
        assert!((summary.relevance_passed - 2.0).abs() < f64::EPSILON);
        assert!((summary.relevance_total - 3.0).abs() < f64::EPSILON);
    }
}
