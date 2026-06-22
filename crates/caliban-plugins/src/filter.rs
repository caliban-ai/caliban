//! Platform / version / strict / enable-list gating applied to a discovered
//! plugin candidate. Produces a [`LoadedPlugin`] when the candidate survives
//! all gates, `None` when it is filtered out, or a [`PluginError`] for a
//! per-plugin hard failure (malformed manifest, name mismatch, strict reject).

use std::path::Path;

use crate::aggregate::pad_version;
use crate::error::PluginError;
use crate::loaded::{LoadedPlugin, PluginSource};
use crate::manager::PluginSettings;
use crate::manifest::PluginManifest;

/// Validate + filter a single discovered candidate.
///
/// # Errors
///
/// Returns [`PluginError`] for per-plugin hard failures: a malformed manifest,
/// a name/dir mismatch, or a strict-plugin-only rejection.
pub fn try_load_one(
    plug_dir: &Path,
    manifest_path: &Path,
    source: PluginSource,
    settings: &PluginSettings,
) -> Result<Option<LoadedPlugin>, PluginError> {
    let manifest = PluginManifest::from_path(manifest_path)?;
    manifest.check_name_matches_dir(manifest_path)?;
    // Platform gating.
    if !manifest.platform_matches() {
        tracing::info!(
            target: caliban_common::tracing_targets::TARGET_PLUGINS,
            name = %manifest.name,
            "skipping plugin: platform mismatch",
        );
        return Ok(None);
    }
    // Min-version gating.
    if let (Some(min), Some(cur)) = (
        manifest.caliban.min_version.as_deref(),
        settings.caliban_version.as_deref(),
    ) && let (Ok(min_v), Ok(cur_v)) = (
        semver::Version::parse(&pad_version(min)),
        semver::Version::parse(&pad_version(cur)),
    ) && cur_v < min_v
    {
        tracing::info!(
            target: caliban_common::tracing_targets::TARGET_PLUGINS,
            name = %manifest.name,
            min = %min,
            current = %cur,
            "skipping plugin: caliban version too old",
        );
        return Ok(None);
    }

    // Strict-plugin-only-customization: reject non-managed plugins.
    if settings.strict_plugin_only_customization && source != PluginSource::Managed {
        return Err(PluginError::StrictPluginOnly {
            name: manifest.name.clone(),
        });
    }

    // Enable list filter (managed plugins ignore it).
    if source != PluginSource::Managed
        && let Some(enabled) = settings.enabled.as_ref()
        && !enabled.iter().any(|n| n == &manifest.name)
    {
        tracing::debug!(
            target: caliban_common::tracing_targets::TARGET_PLUGINS,
            name = %manifest.name,
            "skipping plugin: not in CALIBAN_ENABLED_PLUGINS",
        );
        return Ok(None);
    }

    let components = manifest.resolved_components(plug_dir);
    Ok(Some(LoadedPlugin {
        namespace: manifest.name.clone(),
        manifest,
        root_dir: plug_dir.to_path_buf(),
        source,
        components,
    }))
}
