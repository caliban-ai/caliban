//! `caliban settings` — settings import / print CLI (Phase 6).

use crate::args::SettingsCommand;

/// Top-level dispatcher for `caliban settings <verb>`.
pub(crate) fn run(cmd: &SettingsCommand) -> i32 {
    match cmd {
        SettingsCommand::Import {
            from,
            scope,
            dry_run,
        } => cmd_import(from, scope.as_deref(), *dry_run),
        SettingsCommand::Print { scope } => cmd_print(scope.as_deref()),
    }
}

/// Import a full settings JSON into canonical caliban TOML for the chosen scope.
fn cmd_import(from: &std::path::Path, scope: Option<&str>, dry_run: bool) -> i32 {
    let s = match scope.unwrap_or("project") {
        "project" => caliban_settings::Scope::Project,
        "user" => caliban_settings::Scope::User,
        "local" => caliban_settings::Scope::Local,
        other => {
            eprintln!(
                "[caliban settings] unknown scope {other:?}; \
                 expected one of project/user/local"
            );
            return 2;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let Some(dst) = caliban_settings::scope_path(s, caliban_settings::FileKind::Settings, &cwd)
    else {
        eprintln!("[caliban settings] no writable destination for scope {s:?}");
        return 1;
    };
    if dry_run {
        println!("would import {} -> {}", from.display(), dst.display());
        return 0;
    }
    match caliban_settings::import::import_settings_to_toml(from, &dst) {
        Ok(()) => {
            println!("imported settings to {}", dst.display());
            0
        }
        Err(e) => {
            eprintln!("[caliban settings] import failed: {e}");
            1
        }
    }
}

/// Print the settings for a single scope as JSON.
fn cmd_print(scope: Option<&str>) -> i32 {
    let s = match scope.unwrap_or("project") {
        "managed" => caliban_settings::Scope::Managed,
        "user" => caliban_settings::Scope::User,
        "project" => caliban_settings::Scope::Project,
        "local" => caliban_settings::Scope::Local,
        other => {
            eprintln!(
                "[caliban settings] unknown scope {other:?}; \
                 expected one of managed/user/project/local"
            );
            return 2;
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut opts = caliban_settings::LoadOptions::new(cwd);
    opts.scope_filter = Some(vec![s]);
    opts.schema_validate = false;
    let Ok(loaded) = caliban_settings::load_settings(&opts) else {
        eprintln!("[caliban settings] failed to load settings");
        return 1;
    };
    match serde_json::to_string_pretty(&loaded.settings) {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("[caliban settings] serialization error: {e}");
            1
        }
    }
}
