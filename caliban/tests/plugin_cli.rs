//! Integration tests for the `caliban plugin <verb>` proxy (`plugin_cli.rs`).
//!
//! These run the compiled binary via `Command::new(env!("CARGO_BIN_EXE_caliban"))`.
//! Every invocation is made hermetic by pointing `HOME` (and the XDG dirs, for
//! Linux) at a fresh `tempfile` directory, so the trust store, user install
//! root, and config file all live under the temp dir — no developer state is
//! read or mutated, and nothing hits the network.

use std::path::Path;
use std::process::{Command, Output};

/// Build a hermetic `caliban` command rooted at `dir`.
fn caliban(dir: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_caliban"));
    c.current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("cfg"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env_remove("CALIBAN_ENABLED_PLUGINS")
        .env_remove("CALIBAN_STRICT_KNOWN_MARKETPLACES")
        .env_remove("CALIBAN_BLOCKED_MARKETPLACES");
    c
}

/// Run `caliban plugin <args...>` and return the captured output.
fn run(dir: &Path, args: &[&str]) -> Output {
    let mut c = caliban(dir);
    c.arg("plugin");
    c.args(args);
    c.output().expect("failed to invoke caliban plugin")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("process terminated by signal")
}

#[test]
fn bare_plugin_prints_help_exit_zero() {
    // `plugin` (no verb) and `plugin help` reach the proxy's own
    // `print_help` (stderr). `--help`/`-h` are intercepted by clap on
    // the `Plugin` subcommand itself and print clap's usage to stdout,
    // so they never reach this code path.
    let dir = tempfile::tempdir().unwrap();
    for verb in [&[][..], &["help"]] {
        let out = run(dir.path(), verb);
        assert_eq!(code(&out), 0, "`plugin {verb:?}` should exit 0");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("manage plugin packages"),
            "help text expected on stderr, got: {stderr}"
        );
    }
}

#[test]
fn unknown_subcommand_exits_two_and_prints_help() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["frobnicate"]);
    assert_eq!(code(&out), 2);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown subcommand 'frobnicate'"),
        "{stderr}"
    );
}

#[test]
fn list_on_empty_workspace_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["list"]);
    assert_eq!(
        code(&out),
        0,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn info_without_name_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["info"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing <name>"));
}

#[test]
fn info_unknown_plugin_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["info", "does-not-exist"]);
    assert_eq!(code(&out), 1);
}

#[test]
fn remove_without_name_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["remove"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing <name>"));
}

#[test]
fn remove_unknown_plugin_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["remove", "does-not-exist"]);
    assert_eq!(code(&out), 1);
}

#[test]
fn install_without_spec_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["install"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing"));
}

#[test]
fn install_dir_without_path_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["install", "--dir"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("--dir requires a path"));
}

#[test]
fn install_spec_without_at_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["install", "justaname"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("<name>@<marketplace>"));
}

#[test]
fn install_unexpected_flag_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["install", "--bogus"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected arg"));
}

#[test]
fn install_dir_missing_manifest_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("empty-src");
    std::fs::create_dir_all(&src).unwrap();
    let out = run(dir.path(), &["install", "--dir", src.to_str().unwrap()]);
    assert_eq!(code(&out), 1);
}

#[test]
fn install_dir_with_valid_manifest_sideloads_and_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    // A minimal valid plugin source: just plugin.json with name + version.
    let src = dir.path().join("my-plugin-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("plugin.json"),
        r#"{"name":"demo-plugin","version":"0.1.0"}"#,
    )
    .unwrap();
    // A nested file to exercise the recursive directory copy.
    std::fs::create_dir_all(src.join("commands")).unwrap();
    std::fs::write(src.join("commands/hello.md"), "# hello\n").unwrap();

    let out = run(dir.path(), &["install", "--dir", src.to_str().unwrap()]);
    assert_eq!(
        code(&out),
        0,
        "sideload should succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("installed plugin 'demo-plugin'"));

    // Sideloading again overwrites the existing install dir (clear-then-copy path).
    let out2 = run(dir.path(), &["install", "--dir", src.to_str().unwrap()]);
    assert_eq!(code(&out2), 0, "re-sideload should overwrite cleanly");
}

#[test]
fn install_then_list_and_info_report_the_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("widget-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("plugin.json"),
        r#"{"name":"widget","version":"2.3.4","description":"a test widget"}"#,
    )
    .unwrap();

    let installed = run(dir.path(), &["install", "--dir", src.to_str().unwrap()]);
    assert_eq!(code(&installed), 0, "sideload should succeed");

    // `list` now scans the user install root and should surface the plugin
    // (exercises cli.list + render_overlay with a non-empty row set).
    let listed = run(dir.path(), &["list"]);
    assert_eq!(code(&listed), 0);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("widget"),
        "list should mention the installed plugin: {}",
        String::from_utf8_lossy(&listed.stdout)
    );

    // `info <name>` resolves the installed plugin (cli.info success path,
    // pretty-printed JSON).
    let info = run(dir.path(), &["info", "widget"]);
    assert_eq!(
        code(&info),
        0,
        "info on an installed plugin should succeed; stderr: {}",
        String::from_utf8_lossy(&info.stderr)
    );
    assert!(String::from_utf8_lossy(&info.stdout).contains("widget"));

    // `remove <name>` tears it back down (cli.remove success path).
    let removed = run(dir.path(), &["remove", "widget"]);
    assert_eq!(
        code(&removed),
        0,
        "remove of an installed plugin should succeed"
    );
    assert!(String::from_utf8_lossy(&removed.stdout).contains("removed plugin 'widget'"));
}

#[test]
fn update_without_name_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["update"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing <name>"));
}

#[test]
fn update_unexpected_flag_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["update", "--bogus"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected arg"));
}

#[test]
fn enable_without_name_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["enable"]);
    assert_eq!(code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing <name>"));
}

#[test]
fn enable_then_disable_roundtrips_user_settings() {
    let dir = tempfile::tempdir().unwrap();

    let out = run(dir.path(), &["enable", "demo-plugin"]);
    assert_eq!(
        code(&out),
        0,
        "enable should succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("'demo-plugin' enabled"));

    // Enabling again is idempotent (no duplicate entry, still exit 0).
    let again = run(dir.path(), &["enable", "demo-plugin"]);
    assert_eq!(code(&again), 0);

    let out = run(dir.path(), &["disable", "demo-plugin"]);
    assert_eq!(code(&out), 0, "disable should succeed");
    assert!(String::from_utf8_lossy(&out.stdout).contains("'demo-plugin' disabled"));
}
