//! Build-time version reporting.
//!
//! The crate semver (`CARGO_PKG_VERSION`) alone can't distinguish two
//! builds cut from different commits on the same `main` — every commit
//! after a release still reports the released version. `build.rs` captures
//! the git short SHA, a dirty-worktree marker, and the commit date at
//! compile time and threads them here so `caliban --version` can pin the
//! exact point in history a binary was built from (#303). Builds with no
//! git metadata (crates.io, source tarballs) fall back to the bare semver.

use std::sync::OnceLock;

/// Compose the `--version` string clap prints after the binary name.
///
/// `sha` empty ⇒ no git metadata was available at build time, so we report
/// just the semver. `dirty` is either `"-dirty"` or empty and is appended
/// directly to the SHA. `date` empty ⇒ omit the date clause.
fn compose_long_version(semver: &str, sha: &str, dirty: &str, date: &str) -> String {
    if sha.is_empty() {
        return semver.to_string();
    }
    if date.is_empty() {
        format!("{semver} ({sha}{dirty})")
    } else {
        format!("{semver} ({sha}{dirty}, {date})")
    }
}

/// The full version string for clap's `--version` / `-V` output.
///
/// Cached in a `OnceLock` because clap's derive `version = ...` attribute
/// wants a `&'static str`.
pub(crate) fn long_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        compose_long_version(
            env!("CARGO_PKG_VERSION"),
            option_env!("CALIBAN_GIT_SHA").unwrap_or(""),
            option_env!("CALIBAN_GIT_DIRTY").unwrap_or(""),
            option_env!("CALIBAN_COMMIT_DATE").unwrap_or(""),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::compose_long_version;

    #[test]
    fn full_metadata_shows_sha_dirty_and_date() {
        assert_eq!(
            compose_long_version("0.4.0", "a2f19c3", "-dirty", "2026-07-03"),
            "0.4.0 (a2f19c3-dirty, 2026-07-03)"
        );
    }

    #[test]
    fn clean_tree_omits_dirty_marker() {
        assert_eq!(
            compose_long_version("0.4.0", "a2f19c3", "", "2026-07-03"),
            "0.4.0 (a2f19c3, 2026-07-03)"
        );
    }

    #[test]
    fn no_git_falls_back_to_bare_semver() {
        assert_eq!(compose_long_version("0.4.0", "", "", ""), "0.4.0");
    }

    #[test]
    fn sha_without_date_omits_date_clause() {
        assert_eq!(
            compose_long_version("0.4.0", "a2f19c3", "", ""),
            "0.4.0 (a2f19c3)"
        );
    }

    #[test]
    fn empty_sha_ignores_stray_dirty_or_date() {
        // Defensive: if the SHA is missing we report only the semver even
        // when other fields somehow leaked through.
        assert_eq!(
            compose_long_version("0.4.0", "", "-dirty", "2026-07-03"),
            "0.4.0"
        );
    }
}
