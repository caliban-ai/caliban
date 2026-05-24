//! CLI types for headless (`-p` / `--print`) mode.

use clap::ValueEnum;

/// Output format for headless mode. Selected by `--output-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum OutputFormat {
    /// Plain assistant text on stdout. Default.
    #[default]
    Text,
    /// Single JSON result object on stdout (the final `result` frame).
    Json,
    /// NDJSON stream of events on stdout.
    StreamJson,
}

impl OutputFormat {
    /// Parse from a CLI-style string. Used in tests and for explicit
    /// programmatic conversion outside clap.
    ///
    /// # Errors
    /// Returns `Err(s.to_string())` on an unrecognized value.
    pub(crate) fn parse_str(s: &str) -> Result<Self, String> {
        match s {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "stream-json" => Ok(Self::StreamJson),
            other => Err(other.to_string()),
        }
    }
}

/// Input format for headless mode. Selected by `--input-format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum InputFormat {
    /// Plain user prompt from positional arg or stdin. Default.
    #[default]
    Text,
    /// NDJSON user events from stdin (each line is a `user` or
    /// `control/interrupt` frame).
    StreamJson,
}

impl InputFormat {
    /// Parse from a CLI-style string.
    ///
    /// # Errors
    /// Returns `Err(s.to_string())` on an unrecognized value.
    pub(crate) fn parse_str(s: &str) -> Result<Self, String> {
        match s {
            "text" => Ok(Self::Text),
            "stream-json" => Ok(Self::StreamJson),
            other => Err(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_parse_text() {
        assert_eq!(OutputFormat::parse_str("text").unwrap(), OutputFormat::Text);
    }

    #[test]
    fn output_format_parse_json() {
        assert_eq!(OutputFormat::parse_str("json").unwrap(), OutputFormat::Json);
    }

    #[test]
    fn output_format_parse_stream_json() {
        assert_eq!(
            OutputFormat::parse_str("stream-json").unwrap(),
            OutputFormat::StreamJson
        );
    }

    #[test]
    fn output_format_parse_unknown_errors() {
        assert!(OutputFormat::parse_str("yaml").is_err());
    }

    #[test]
    fn input_format_parse_text() {
        assert_eq!(InputFormat::parse_str("text").unwrap(), InputFormat::Text);
    }

    #[test]
    fn input_format_parse_stream_json() {
        assert_eq!(
            InputFormat::parse_str("stream-json").unwrap(),
            InputFormat::StreamJson
        );
    }

    #[test]
    fn input_format_parse_unknown_errors() {
        assert!(InputFormat::parse_str("xml").is_err());
    }

    #[test]
    fn default_output_format_is_text() {
        assert_eq!(OutputFormat::default(), OutputFormat::Text);
    }

    #[test]
    fn default_input_format_is_text() {
        assert_eq!(InputFormat::default(), InputFormat::Text);
    }
}
