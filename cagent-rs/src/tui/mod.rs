//! TUI (Terminal User Interface) for cagent
//!
//! A full-featured terminal interface with:
//! - Message display with markdown rendering
//! - Multi-line input editor
//! - Status bar with token usage
//! - Tool call visualization
//! - Keyboard shortcuts
//! - Multiple color themes

#[cfg(feature = "tui")]
pub mod app;

#[cfg(feature = "tui")]
pub mod markdown;

#[cfg(feature = "tui")]
pub mod theme;

#[cfg(feature = "tui")]
pub use app::run_tui;

#[cfg(feature = "tui")]
pub use theme::Theme;
