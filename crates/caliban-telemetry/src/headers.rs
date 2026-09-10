//! `otel_headers_helper` integration.
//!
//! Operators point a setting at a script; we run it **once at startup**
//! (`Telemetry::init_from_env`) and parse its stdout as `k=v\nk=v\n…`. The
//! parsed headers merge with `OTEL_EXPORTER_OTLP_HEADERS` (helper wins on key
//! collision). This lets short-lived bearer tokens (GCP IAM, AWS SigV4, etc.)
//! flow into OTLP exports without checking secrets into env files. There is no
//! background refresh thread; periodic re-invocation is future work (#426 T3).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::TelemetryError;

/// Configuration for the headers-helper script.
#[derive(Debug, Clone)]
pub struct HeadersHelperConfig {
    /// Path to the script. Must be executable.
    pub path: PathBuf,
}

impl HeadersHelperConfig {
    /// Construct from the script path.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

/// Parse stdout from a helper script: each non-empty line is `key=value`.
/// Lines without `=` or starting with `#` are skipped.
#[must_use]
pub fn parse_helper_output(stdout: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// Parse the `OTEL_EXPORTER_OTLP_HEADERS=k1=v1,k2=v2` env-var format.
#[must_use]
pub fn parse_otlp_headers_env(raw: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for piece in raw.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if let Some((k, v)) = piece.split_once('=') {
            out.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// Merge env-supplied headers with helper-output headers. Helper wins on key
/// collision per ADR 0033.
#[must_use]
pub fn merge_headers(
    env_headers: &BTreeMap<String, String>,
    helper_headers: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut merged = env_headers.clone();
    for (k, v) in helper_headers {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// Invoke the helper script synchronously and parse its stdout.
///
/// # Errors
/// Returns `TelemetryError::HeadersHelper` when the script fails to spawn
/// or exits non-zero.
pub fn invoke_helper(path: &Path) -> Result<BTreeMap<String, String>, TelemetryError> {
    let output = Command::new(path)
        .output()
        .map_err(|source| TelemetryError::HeadersHelper {
            path: path.to_path_buf(),
            source,
        })?;
    if !output.status.success() {
        return Err(TelemetryError::HeadersHelper {
            path: path.to_path_buf(),
            source: std::io::Error::other(format!(
                "helper exited with status {:?}; stderr: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim(),
            )),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_helper_output(&stdout))
}

/// True iff `err` is `ETXTBSY` ("text file busy", errno 26).
///
/// Test-only hardening (#580, follows #441): a freshly written + `chmod +x`
/// script can transiently fail `execve` with `ETXTBSY` under parallel CI load
/// while any process still holds a write handle to it. The production helper
/// execs a *user-provided* binary it never wrote, so it deliberately carries no
/// retry (retrying a startup helper would add undesirable latency); only the
/// tests that fabricate an executable and immediately run it need this guard.
/// Shared with `init.rs`'s `refresh_dynamic_headers` test.
#[cfg(all(test, unix))]
pub(crate) fn is_etxtbsy(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(26)
}

/// Number of times a test may retry a freshly-written script's exec before
/// giving up on the `ETXTBSY` window. Ample: the busy window is sub-millisecond.
#[cfg(all(test, unix))]
pub(crate) const ETXTBSY_TEST_RETRIES: u32 = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_helper_output_extracts_kv_pairs() {
        let s = "Authorization=Bearer abc\nX-Tenant=acme\n# a comment\n\n";
        let h = parse_helper_output(s);
        assert_eq!(
            h.get("Authorization").map(String::as_str),
            Some("Bearer abc")
        );
        assert_eq!(h.get("X-Tenant").map(String::as_str), Some("acme"));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn parse_otlp_env_extracts_kv_pairs() {
        let s = "Authorization=Bearer xyz,X-Other=42";
        let h = parse_otlp_headers_env(s);
        assert_eq!(
            h.get("Authorization").map(String::as_str),
            Some("Bearer xyz")
        );
        assert_eq!(h.get("X-Other").map(String::as_str), Some("42"));
    }

    #[test]
    fn helper_wins_on_key_collision() {
        let env_headers = parse_otlp_headers_env("Authorization=Bearer ENV,X-Tenant=env-acme");
        let helper = parse_helper_output("Authorization=Bearer HELPER");
        let merged = merge_headers(&env_headers, &helper);
        assert_eq!(
            merged.get("Authorization").map(String::as_str),
            Some("Bearer HELPER"),
            "helper output wins per ADR 0033",
        );
        assert_eq!(
            merged.get("X-Tenant").map(String::as_str),
            Some("env-acme"),
            "env-only keys preserved",
        );
    }

    #[cfg(unix)]
    #[test]
    fn is_etxtbsy_matches_errno_26_only() {
        assert!(is_etxtbsy(&std::io::Error::from_raw_os_error(26)));
        // EACCES (13) and a kind-only error must not be mistaken for ETXTBSY.
        assert!(!is_etxtbsy(&std::io::Error::from_raw_os_error(13)));
        assert!(!is_etxtbsy(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
    }

    #[test]
    fn invoke_helper_runs_real_script_on_unix() {
        // POSIX shell echo via /bin/sh to make this portable. On non-Unix
        // we'd need a different helper, but caliban only ships on Unix for now.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("helper.sh");
            std::fs::write(
                &script,
                "#!/bin/sh\nprintf 'Authorization=Bearer 12345\\nX-Tenant=acme\\n'\n",
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            // #580: this test execs the file it just wrote, which can transiently
            // ETXTBSY under parallel CI load. Retry only that class of failure.
            let out = {
                let mut attempt = 0;
                loop {
                    match invoke_helper(&script) {
                        Ok(out) => break out,
                        Err(TelemetryError::HeadersHelper { source, .. })
                            if is_etxtbsy(&source) && attempt < ETXTBSY_TEST_RETRIES =>
                        {
                            attempt += 1;
                            std::thread::yield_now();
                        }
                        Err(e) => panic!("helper must run: {e}"),
                    }
                }
            };
            assert_eq!(
                out.get("Authorization").map(String::as_str),
                Some("Bearer 12345"),
            );
            assert_eq!(out.get("X-Tenant").map(String::as_str), Some("acme"));
        }
    }
}
