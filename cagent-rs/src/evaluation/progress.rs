//! Progress bar for evaluation runs

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

use super::types::Result as EvalResult;

/// Progress bar for evaluation runs
pub struct ProgressBar<W: Write + Send + 'static> {
    out: Arc<Mutex<W>>,
    total: usize,
    completed: AtomicUsize,
    passed: AtomicUsize,
    failed: AtomicUsize,
    running: Arc<Mutex<Vec<String>>>,
    stop_tx: Option<watch::Sender<bool>>,
    is_tty: bool,
}

impl<W: Write + Send + 'static> ProgressBar<W> {
    /// Create a new progress bar
    pub fn new(out: W, total: usize, is_tty: bool) -> Self {
        Self {
            out: Arc::new(Mutex::new(out)),
            total,
            completed: AtomicUsize::new(0),
            passed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            running: Arc::new(Mutex::new(Vec::new())),
            stop_tx: None,
            is_tty,
        }
    }

    /// Start the progress bar update loop
    pub fn start(&mut self) {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        self.stop_tx = Some(stop_tx);

        let out = Arc::clone(&self.out);
        let total = self.total;
        let completed = &self.completed as *const AtomicUsize;
        let passed = &self.passed as *const AtomicUsize;
        let failed = &self.failed as *const AtomicUsize;
        let running = Arc::clone(&self.running);
        let is_tty = self.is_tty;

        // Safety: We ensure the ProgressBar outlives the spawned task
        // by stopping it in drop()
        let completed = unsafe { &*completed };
        let passed = unsafe { &*passed };
        let failed = unsafe { &*failed };

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let c = completed.load(Ordering::Relaxed);
                        let p = passed.load(Ordering::Relaxed);
                        let f = failed.load(Ordering::Relaxed);
                        let r = running.lock().clone();
                        
                        render_progress(&out, total, c, p, f, &r, is_tty, false);
                    }
                    _ = stop_rx.changed() => {
                        if *stop_rx.borrow() {
                            let c = completed.load(Ordering::Relaxed);
                            let p = passed.load(Ordering::Relaxed);
                            let f = failed.load(Ordering::Relaxed);
                            let r = running.lock().clone();
                            
                            render_progress(&out, total, c, p, f, &r, is_tty, true);
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Stop the progress bar
    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
    }

    /// Mark an evaluation as started
    pub fn set_running(&self, title: &str) {
        self.running.lock().push(title.to_string());
    }

    /// Mark an evaluation as completed
    pub fn complete(&self, title: &str, success: bool) {
        {
            let mut running = self.running.lock();
            running.retain(|t| t != title);
        }
        
        self.completed.fetch_add(1, Ordering::Relaxed);
        if success {
            self.passed.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Print a result (clears progress bar, prints result, re-renders progress)
    pub fn print_result(&self, result: &EvalResult) {
        let mut out = self.out.lock();
        
        // Clear current line on TTY
        if self.is_tty {
            write!(out, "\r\x1b[K").ok();
        }
        
        let (successes, failures) = result.check_results();
        let success = failures.is_empty();
        
        // Print title with icon
        if success {
            writeln!(out, "\x1b[32m✓\x1b[0m {} (${:.6})", result.title, result.cost).ok();
        } else {
            writeln!(out, "\x1b[31m✗\x1b[0m {} (${:.6})", result.title, result.cost).ok();
        }
        
        // Print successes and failures
        for s in &successes {
            writeln!(out, "  \x1b[32m✓\x1b[0m {}", s).ok();
        }
        for f in &failures {
            writeln!(out, "  \x1b[31m✗\x1b[0m {}", f).ok();
        }
    }

    /// Get the writer (for summary output)
    pub fn writer(&self) -> Arc<Mutex<W>> {
        Arc::clone(&self.out)
    }
}

impl<W: Write + Send + 'static> Drop for ProgressBar<W> {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(true);
        }
    }
}

/// Render the progress bar
fn render_progress<W: Write>(
    out: &Arc<Mutex<W>>,
    total: usize,
    completed: usize,
    passed: usize,
    failed: usize,
    running: &[String],
    is_tty: bool,
    is_final: bool,
) {
    let mut out = out.lock();
    
    // Get terminal width
    let term_width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);
    
    // Calculate bar width
    let bar_width = (term_width.saturating_sub(60)).clamp(10, 50);
    
    // Calculate progress
    let filled_width = if total > 0 {
        (completed * bar_width) / total
    } else {
        0
    };
    
    let bar = format!(
        "{}{}",
        "█".repeat(filled_width),
        "░".repeat(bar_width - filled_width)
    );
    
    let percent = if total > 0 {
        (completed * 100) / total
    } else {
        0
    };
    
    // Build status line
    let counts = format!(
        "\x1b[32m✓{}\x1b[0m \x1b[31m✗{}\x1b[0m",
        passed, failed
    );
    
    let mut status = format!(
        "[{}] {:3}% ({}/{}) {}",
        bar, percent, completed, total, counts
    );
    
    // Add running info
    if !running.is_empty() {
        let available_for_name = term_width.saturating_sub(status.len() + 15).max(5);
        let name = if running[0].len() > available_for_name {
            format!("{}…", &running[0][..available_for_name - 1])
        } else {
            running[0].clone()
        };
        
        if running.len() == 1 {
            status = format!("{} | {}", status, name);
        } else {
            status = format!("{} | {} +{} more", status, name, running.len() - 1);
        }
    }
    
    if is_tty {
        // Clear line and write status
        write!(out, "\r\x1b[K{}", status).ok();
        if is_final {
            writeln!(out).ok();
        }
    } else if is_final {
        writeln!(out, "{}", status).ok();
    }
    
    out.flush().ok();
}

/// Check if stdout is a TTY
pub fn is_tty() -> bool {
    atty::is(atty::Stream::Stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_progress_bar_counts() {
        let buf = Vec::new();
        let pb = ProgressBar::new(buf, 10, false);
        
        pb.set_running("test1");
        pb.complete("test1", true);
        
        assert_eq!(pb.completed.load(Ordering::Relaxed), 1);
        assert_eq!(pb.passed.load(Ordering::Relaxed), 1);
        assert_eq!(pb.failed.load(Ordering::Relaxed), 0);
        
        pb.set_running("test2");
        pb.complete("test2", false);
        
        assert_eq!(pb.completed.load(Ordering::Relaxed), 2);
        assert_eq!(pb.passed.load(Ordering::Relaxed), 1);
        assert_eq!(pb.failed.load(Ordering::Relaxed), 1);
    }
}
