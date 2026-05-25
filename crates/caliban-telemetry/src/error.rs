//! Telemetry-layer error types.

use std::path::PathBuf;

use thiserror::Error;

/// Errors surfaced by the telemetry init / exporter / rate-card layers.
#[derive(Debug, Error)]
pub enum TelemetryError {
    /// OTLP exporter construction failed.
    #[error("otlp exporter setup failed: {0}")]
    OtlpExporter(String),
    /// `rates.yaml` parse error.
    #[error("invalid rate card at {path}: {source}")]
    InvalidRates {
        /// Path to the file we tried to parse.
        path: PathBuf,
        /// Underlying serde error.
        #[source]
        source: serde_yaml::Error,
    },
    /// `rates.yaml` I/O error.
    #[error("could not read rate card at {path}: {source}")]
    RatesIo {
        /// Path to the file we tried to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The supplied OTLP endpoint URL is not parseable.
    #[error("invalid OTLP endpoint: {0}")]
    InvalidEndpoint(String),
    /// Headers-helper script invocation failed.
    #[error("headers helper {path} failed: {source}")]
    HeadersHelper {
        /// Path to the helper script.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}
