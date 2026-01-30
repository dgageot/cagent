use std::path::{Path, PathBuf};

use once_cell::sync::OnceCell;
use tracing_subscriber::filter::EnvFilter;

static OTEL_PROVIDER: OnceCell<opentelemetry_sdk::trace::SdkTracerProvider> = OnceCell::new();

/// Initialize tracing/logging.
///
/// If `debug` is false, we log at info level to stderr.
/// If `debug` is true, we log at debug level and also write to a file.
///
/// If `log_file` is provided, it is used as the file destination.
/// Otherwise defaults to `~/.cagent/cagent.debug.log`.
///
/// If `otel` is enabled, an OpenTelemetry tracing pipeline is installed.
pub fn init_tracing(debug: bool, log_file: Option<&Path>, otel: bool) -> anyhow::Result<()> {
    init_tracing_with_home(debug, log_file, otel, None)
}

pub fn shutdown_tracing() {
    if let Some(provider) = OTEL_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            tracing::warn!(error = %e, "failed to shutdown otel tracer provider");
        }
    }
}

pub fn init_tracing_with_home(
    debug: bool,
    log_file: Option<&Path>,
    otel: bool,
    home: Option<&Path>,
) -> anyhow::Result<()> {
    let level = if debug { "debug" } else { "info" };

    // Always enable RUST_LOG overrides if set, otherwise use our default.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    let (file_layer, log_path) = if debug {
        let path = match log_file {
            Some(p) => p.to_path_buf(),
            None => default_debug_log_path(home),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        let layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(file);

        (Some(layer), Some(path))
    } else {
        (None, None)
    };

    let otel_layer = if otel {
        let tracer = init_otel_tracer()?;
        Some(tracing_opentelemetry::layer().with_tracer(tracer))
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .with(otel_layer)
        .init();

    if let Some(path) = log_path {
        tracing::debug!(log_file = %path.display(), "debug logging enabled");
    }

    Ok(())
}

fn init_otel_tracer() -> anyhow::Result<opentelemetry_sdk::trace::Tracer> {
    use opentelemetry::global;
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        // Keep defaults so it can be configured via OTEL_* env vars.
        .with_endpoint(
            std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4317".to_string()),
        )
        .build()?;

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name("cagent")
                .build(),
        )
        .build();

    // Safe because init_tracing is expected to be called once.
    let _ = OTEL_PROVIDER.set(provider.clone());

    global::set_tracer_provider(provider.clone());

    Ok(provider.tracer("cagent"))
}

fn default_debug_log_path(home: Option<&Path>) -> PathBuf {
    let base = home
        .map(Path::to_path_buf)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".cagent").join("cagent.debug.log")
}

// Needed for `.with(...)`
use tracing_subscriber::prelude::*;
