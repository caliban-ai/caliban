//! `caliban plugin` subcommand entry point.
//!
//! The dispatcher is intentionally tiny — it parses the verb + args from
//! a raw `argv` slice (no `clap` subcommand restructuring) and delegates
//! to [`caliban_plugins::Cli`].

use std::path::PathBuf;

use caliban_plugins::{Cli, MarketplaceClient, MarketplaceSettings, PluginSettings, TrustStore};

/// Entry point. `args` is `argv[2..]` (i.e. the slice *after* `plugin`).
///
/// Returns the desired process exit code.
pub(crate) async fn run(args: &[String]) -> i32 {
    let verb = args.first().map_or("", String::as_str);
    let rest = &args[args.len().min(1)..];
    match verb {
        "" | "help" | "--help" | "-h" => {
            print_help();
            0
        }
        "list" => cmd_list(),
        "info" => cmd_info(rest),
        "install" => cmd_install(rest).await,
        "remove" => cmd_remove(rest),
        "update" => cmd_update(rest).await,
        "enable" => cmd_enable(rest, true),
        "disable" => cmd_enable(rest, false),
        other => {
            eprintln!("caliban plugin: unknown subcommand '{other}'");
            print_help();
            2
        }
    }
}

fn print_help() {
    eprintln!(
        "caliban plugin — manage plugin packages (ADR 0030)

Usage:
  caliban plugin list
  caliban plugin info <name>
  caliban plugin install <name>@<marketplace> [--yes]
  caliban plugin install --dir <path>
  caliban plugin update <name> [--yes]
  caliban plugin remove <name>
  caliban plugin enable <name>
  caliban plugin disable <name>

Env:
  CALIBAN_ENABLED_PLUGINS             Comma-separated list of enabled plugins.
  CALIBAN_STRICT_KNOWN_MARKETPLACES   Comma-separated marketplace allowlist.
  CALIBAN_BLOCKED_MARKETPLACES        Comma-separated marketplace block list.
  CALIBAN_STRICT_PLUGIN_ONLY_CUSTOMIZATION=1
                                      Reject non-managed plugins."
    );
}

fn make_cli() -> Result<Cli, String> {
    let workspace_root = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let user_install_dir = dirs::data_local_dir()
        .ok_or_else(|| "no $XDG_DATA_HOME".to_string())?
        .join("caliban")
        .join("plugins");
    let trust = TrustStore::open_default().map_err(|e| format!("trust store: {e}"))?;
    let marketplace =
        MarketplaceClient::new(reqwest::Client::new(), MarketplaceSettings::from_env());
    let settings = PluginSettings::from_env();
    Ok(Cli {
        workspace_root,
        user_install_dir,
        trust,
        marketplace,
        settings,
    })
}

fn cmd_list() -> i32 {
    let cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin list: {e}");
            return 1;
        }
    };
    match cli.list() {
        Ok(rows) => {
            for line in caliban_plugins::render_overlay(&rows) {
                println!("{line}");
            }
            0
        }
        Err(e) => {
            eprintln!("caliban plugin list: {e}");
            1
        }
    }
}

fn cmd_info(args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("caliban plugin info: missing <name>");
        return 2;
    };
    let cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin info: {e}");
            return 1;
        }
    };
    match cli.info(name) {
        Ok(v) => {
            match serde_json::to_string_pretty(&v) {
                Ok(s) => println!("{s}"),
                Err(_) => println!("{v}"),
            }
            0
        }
        Err(e) => {
            eprintln!("caliban plugin info: {e}");
            1
        }
    }
}

fn cmd_remove(args: &[String]) -> i32 {
    let Some(name) = args.first() else {
        eprintln!("caliban plugin remove: missing <name>");
        return 2;
    };
    let mut cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin remove: {e}");
            return 1;
        }
    };
    match cli.remove(name) {
        Ok(()) => {
            println!("removed plugin '{name}'");
            0
        }
        Err(e) => {
            eprintln!("caliban plugin remove: {e}");
            1
        }
    }
}

async fn cmd_install(args: &[String]) -> i32 {
    // Two forms:
    //   caliban plugin install <name>@<marketplace> [--yes]
    //   caliban plugin install --dir <path>
    let mut approve = false;
    let mut spec: Option<String> = None;
    let mut dir: Option<PathBuf> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "--yes" => approve = true,
            "--dir" => {
                idx += 1;
                if let Some(p) = args.get(idx) {
                    dir = Some(PathBuf::from(p));
                } else {
                    eprintln!("caliban plugin install: --dir requires a path");
                    return 2;
                }
            }
            s if !s.starts_with("--") && spec.is_none() => spec = Some(s.to_string()),
            other => {
                eprintln!("caliban plugin install: unexpected arg '{other}'");
                return 2;
            }
        }
        idx += 1;
    }

    if let Some(dir) = dir {
        return cmd_install_dir(&dir);
    }
    let Some(spec) = spec else {
        eprintln!("caliban plugin install: missing <name>@<marketplace> or --dir <path>");
        return 2;
    };
    let Some((name, market)) = spec.split_once('@') else {
        eprintln!("caliban plugin install: expected <name>@<marketplace>, got '{spec}'");
        return 2;
    };
    let mut cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin install: {e}");
            return 1;
        }
    };
    match cli.install(name, market, None, approve).await {
        Ok(path) => {
            println!("installed plugin '{name}' to {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("caliban plugin install: {e}");
            1
        }
    }
}

fn cmd_install_dir(src: &std::path::Path) -> i32 {
    // Sideload: just copy the directory into the user install root and
    // write a trust record with marketplace == "sideload".
    let manifest = match caliban_plugins::PluginManifest::from_path(&src.join("plugin.json")) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("caliban plugin install --dir: {e}");
            return 1;
        }
    };
    let mut cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin install --dir: {e}");
            return 1;
        }
    };
    let dest = cli.user_install_dir.join(&manifest.name);
    if dest.exists()
        && let Err(e) = std::fs::remove_dir_all(&dest)
    {
        eprintln!(
            "caliban plugin install --dir: could not clear {}: {e}",
            dest.display()
        );
        return 1;
    }
    if let Err(e) = copy_dir_recursive(src, &dest) {
        eprintln!("caliban plugin install --dir: {e}");
        return 1;
    }
    cli.trust.record(
        &manifest.name,
        caliban_plugins::PluginTrustRecord {
            version: manifest.version.clone(),
            marketplace: "sideload".into(),
            manifest_sha256: String::new(),
            installed_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    if let Err(e) = cli.trust.save() {
        eprintln!("caliban plugin install --dir: trust save failed: {e}");
        return 1;
    }
    println!("installed plugin '{}' to {}", manifest.name, dest.display());
    0
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

async fn cmd_update(args: &[String]) -> i32 {
    let mut approve = false;
    let mut name: Option<String> = None;
    for a in args {
        match a.as_str() {
            "--yes" => approve = true,
            s if !s.starts_with("--") && name.is_none() => name = Some(s.to_string()),
            other => {
                eprintln!("caliban plugin update: unexpected arg '{other}'");
                return 2;
            }
        }
    }
    let Some(name) = name else {
        eprintln!("caliban plugin update: missing <name>");
        return 2;
    };
    let mut cli = match make_cli() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("caliban plugin update: {e}");
            return 1;
        }
    };
    match cli.update(&name, approve).await {
        Ok(Some(path)) => {
            println!("updated plugin '{name}' to {}", path.display());
            0
        }
        Ok(None) => {
            println!("plugin '{name}' is already up-to-date");
            0
        }
        Err(e) => {
            eprintln!("caliban plugin update: {e}");
            1
        }
    }
}

fn cmd_enable(args: &[String], enable: bool) -> i32 {
    let verb = if enable { "enable" } else { "disable" };
    let Some(name) = args.first() else {
        eprintln!("caliban plugin {verb}: missing <name>");
        return 2;
    };
    match update_user_settings_plugins_enabled(name, enable) {
        Ok(path) => {
            println!("plugin '{name}' {verb}d in {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("caliban plugin {verb}: {e}");
            1
        }
    }
}

/// Toggle `plugins.enabled` for `name` inside the user-scope
/// `settings.json`. Creates the file if missing, preserves any other
/// keys, and writes pretty-printed JSON. Returns the path written.
fn update_user_settings_plugins_enabled(name: &str, enable: bool) -> Result<PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "no user config dir".to_string())?
        .join("caliban");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("settings.json");
    let mut root: serde_json::Value = if path.exists() {
        let raw =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if raw.trim().is_empty() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?
        }
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;
    let plugins_entry = obj
        .entry("plugins")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let plugins_obj = plugins_entry
        .as_object_mut()
        .ok_or_else(|| format!("plugins must be a JSON object in {}", path.display()))?;
    let enabled_entry = plugins_obj
        .entry("enabled")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let arr = enabled_entry
        .as_array_mut()
        .ok_or_else(|| format!("plugins.enabled must be an array in {}", path.display()))?;
    let already = arr.iter().any(|v| v.as_str() == Some(name));
    if enable {
        if !already {
            arr.push(serde_json::Value::String(name.to_string()));
        }
    } else {
        arr.retain(|v| v.as_str() != Some(name));
    }
    let serialized =
        serde_json::to_string_pretty(&root).map_err(|e| format!("serialize settings: {e}"))?;
    std::fs::write(&path, serialized).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}
