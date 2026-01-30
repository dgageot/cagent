use std::path::Path;

use crate::logging::init_tracing_with_home;

#[test]
fn init_tracing_creates_default_log_dir_under_home() {
    // Note: tracing subscriber can only be initialized once per process.
    // This test must not run in parallel with other tests that initialize tracing.
    // For now, the workspace doesn't init tracing anywhere in tests.

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();

    init_tracing_with_home(true, None, false, Some(home)).unwrap();

    let log_path = home.join(".cagent").join("cagent.debug.log");
    assert!(log_path.exists(), "expected {:?} to exist", log_path);
}

#[test]
fn cli_parses_log_file_flag() {
    use clap::Parser;

    let cli =
        crate::cli::Cli::parse_from(["cagent", "--log-file", "/tmp/cagent.test.log", "version"]);

    assert_eq!(
        cli.log_file.as_deref(),
        Some(Path::new("/tmp/cagent.test.log"))
    );
}

#[test]
fn cli_parses_otel_flag() {
    use clap::Parser;

    let cli = crate::cli::Cli::parse_from(["cagent", "--otel", "version"]);

    assert!(cli.otel);
}

#[test]
fn cli_parses_completion_subcommand() {
    use clap::Parser;

    let cli = crate::cli::Cli::parse_from(["cagent", "completion", "bash"]);

    match cli.command {
        crate::cli::Commands::Completion { shell } => {
            assert_eq!(shell, clap_complete::Shell::Bash);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
