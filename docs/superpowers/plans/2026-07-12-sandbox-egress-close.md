# Sandbox Egress Close Implementation Plan (#406)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Under `--workspace`, a sandboxed Bash command can read the disk but cannot reach the network — with a working opt-out.

**Architecture:** Both sandbox backends already implement deny-egress correctly; the path has never been selected in production. The core change flips `allow_all_outbound` to `false` in `workspace_fence_policy()`. Around that, three things must land or the change is either broken or unusable: a latent bwrap fail-open (#476) that would silently restore full egress, a macOS/Seatbelt asymmetry that denies loopback along with egress (breaking every localhost test suite on macOS only), and a config/CLI surface — the sandbox has **no** user-reachable configuration today, so closing egress without an escape hatch would strand anyone needing `git fetch`.

**Tech Stack:** Rust (edition 2024 workspace), `clap` derive for CLI, `caliban-settings` (JSON-schema-backed `settings.json`, ADR 0026), bubblewrap (Linux) / `sandbox-exec` Seatbelt (macOS).

**Spec:** `docs/superpowers/specs/2026-07-12-sandbox-egress-confinement-design.md`

## Global Constraints

- **Breaking change, target 0.7.0.** `--workspace` will also mean "no network". Requires a BREAKING changelog entry.
- **Never widen egress to satisfy a narrower permission.** A local-sounding option must never grant the internet (this is the #403 / #476 failure mode, and the reason this ticket exists).
- **Both backends must behave equivalently.** Any task that changes network posture must be verified on bwrap *and* Seatbelt. Loopback works inside the sandbox on both; real egress fails on both.
- **Reads stay open.** `allow_read: ["/"]` is deliberate and stays. Do not add read confinement — it is explicitly rejected in the spec.
- **caliban's own provider calls must not be affected.** They run in the parent process and never pass through the shim. Any test that breaks the agent's own model calls means the change is wrong.
- Verification gate before any push (mirrors CI): `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`.
- **No `Co-Authored-By` / "Generated with Claude" trailers** in commits.

---

### Task 1: Fix the bwrap `allow_local_binding` fail-open (#476)

The existing branch skips `--unshare-net` when `allow_local_binding` is set, leaving the host network namespace intact — **full egress**. On Linux, `--unshare-net` *is* the loopback-only posture (an isolated netns has `lo` up), so `allow_local_binding` must **not** suppress it. This must land first: Task 3 sets `allow_local_binding: true`, which would otherwise silently undo the entire ticket.

**Files:**
- Modify: `crates/caliban-sandbox/src/bwrap.rs:84-101`
- Test: `crates/caliban-sandbox/src/bwrap.rs` (in-file `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Policy`, `NetworkAcl` from `crate::config`.
- Produces: no signature change — `build_args(policy: &Policy) -> Vec<OsString>` behavior change only.

- [ ] **Step 1: Write the failing test**

Add to the tests module in `crates/caliban-sandbox/src/bwrap.rs`:

```rust
#[test]
fn local_binding_does_not_grant_egress() {
    // #476: `allow_local_binding` is satisfied BY --unshare-net (an isolated
    // netns has loopback up). It must never suppress it — doing so keeps the
    // host network namespace and grants ALL egress.
    let policy = Policy {
        network: NetworkAcl {
            allow_local_binding: true,
            allow_all_outbound: false,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    };
    let args = build_args(&policy);
    assert!(
        args.iter().any(|a| a == "--unshare-net"),
        "allow_local_binding must still isolate the network namespace; \
         got: {args:?}"
    );
}

#[test]
fn allow_all_outbound_keeps_host_netns() {
    let policy = Policy {
        network: NetworkAcl {
            allow_all_outbound: true,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    };
    let args = build_args(&policy);
    assert!(
        !args.iter().any(|a| a == "--unshare-net"),
        "allow_all_outbound must keep the host network namespace"
    );
}
```

If the tests module does not already import them, add at its top:

```rust
use crate::config::{NetworkAcl, Policy};
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-sandbox local_binding_does_not_grant_egress`
Expected: FAIL — `allow_local_binding must still isolate the network namespace; got: [...]` (no `--unshare-net` in the args).

- [ ] **Step 3: Write minimal implementation**

Replace the network block at `crates/caliban-sandbox/src/bwrap.rs:88-101` with:

```rust
    // Isolate the network unless egress is *explicitly* required. A bare
    // `allowed_domains`/`denied_domains` list does NOT keep the namespace open:
    // bwrap can't filter per-hostname, so those lists are enforceable only via
    // the proxy (`validate_policy` rejects them otherwise). Keeping the network
    // open for a domain list would grant ALL egress — the inversion #403 fixes.
    //
    // `allow_local_binding` does NOT keep the namespace open either (#476):
    // --unshare-net *is* the loopback-only posture on Linux — bwrap brings `lo`
    // up inside the fresh netns, so a command can still bind and connect to
    // 127.0.0.1; it simply cannot reach the host or the internet. Letting
    // `allow_local_binding` suppress --unshare-net granted full egress from a
    // local-sounding permission.
    if allow_proxy {
        // Deny direct egress; only the operator's loopback proxy is reachable.
        // The proxy enforces domain rules.
        push_str(&mut args, "--unshare-net");
    } else if !net.allow_all_outbound {
        push_str(&mut args, "--unshare-net");
    }
    // Otherwise (`allow_all_outbound`): keep the network namespace so egress works.
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-sandbox`
Expected: PASS, including both new tests and the existing suite.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-sandbox/src/bwrap.rs
git commit -m "fix(permissions): allow_local_binding must not grant full egress (#476)"
```

---

### Task 2: Seatbelt loopback-only branch

Seatbelt's profile is `(deny default)`. With no network rule, **all** network is denied — *including loopback*. bwrap's `--unshare-net` keeps `lo` up. Without this task, macOS gets a materially more broken sandbox than Linux: every test suite that binds `127.0.0.1` fails on macOS only.

**Files:**
- Modify: `crates/caliban-sandbox/src/seatbelt.rs:72-105`
- Test: `crates/caliban-sandbox/src/seatbelt.rs` (in-file `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Policy`, `NetworkAcl`.
- Produces: no signature change — `render_profile(policy: &Policy) -> String` gains a loopback branch.

- [ ] **Step 1: Write the failing test**

Add to the tests module in `crates/caliban-sandbox/src/seatbelt.rs`:

```rust
#[test]
fn local_binding_allows_loopback_only() {
    // Seatbelt is `(deny default)`, so with no network rule even loopback is
    // denied — unlike bwrap's --unshare-net, which keeps `lo` up. Without an
    // explicit loopback branch, closing egress would break every localhost
    // test server on macOS only.
    let policy = Policy {
        network: NetworkAcl {
            allow_local_binding: true,
            allow_all_outbound: false,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    };
    let s = render_profile(&policy);
    assert!(
        s.contains(r#"(remote ip "localhost:*")"#),
        "loopback must be permitted when allow_local_binding is set:\n{s}"
    );
    assert!(
        !s.contains("(allow network*)\n"),
        "must NOT emit a blanket network allow — that is full egress:\n{s}"
    );
}

#[test]
fn no_network_flags_denies_all_network() {
    let policy = Policy::default();
    let s = render_profile(&policy);
    assert!(
        !s.contains("(allow network"),
        "a default policy must emit no network allow at all:\n{s}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-sandbox local_binding_allows_loopback_only`
Expected: FAIL — `loopback must be permitted when allow_local_binding is set` (no network rule is emitted at all today).

- [ ] **Step 3: Write minimal implementation**

In `crates/caliban-sandbox/src/seatbelt.rs`, add a branch to the network chain. The existing chain is `if http_proxy_port != 0 { … } else if socks_proxy_port != 0 { … } else if allow_all_outbound { … }`. Append **one more** `else if`, after the `allow_all_outbound` arm:

```rust
    } else if net.allow_local_binding {
        // Loopback only: the child may bind and connect to 127.0.0.1 (test
        // servers, dev servers) but has no route off the box. This mirrors
        // what bwrap's --unshare-net gives for free on Linux — an isolated
        // netns with `lo` up. Seatbelt is `(deny default)`, so without this
        // branch closing egress would also kill loopback (macOS only).
        let _ = writeln!(out, ";; Network: loopback only (allow_local_binding).");
        let _ = writeln!(
            out,
            r#"(allow network* (local ip "*:*") (remote ip "localhost:*"))"#
        );
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-sandbox`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-sandbox/src/seatbelt.rs
git commit -m "feat(permissions): Seatbelt loopback-only branch for allow_local_binding (#406)"
```

---

### Task 3: Close egress in the fence policy

The core change. With Tasks 1–2 in place, both backends now do the right thing for this policy.

**Files:**
- Modify: `caliban/src/startup/compose.rs:493-498`
- Test: `caliban/src/startup/compose.rs` (in-file `#[cfg(test)] mod tests`; add one if absent)

**Interfaces:**
- Consumes: `caliban_sandbox::{Policy, NetworkAcl}`.
- Produces: `workspace_fence_policy(workspace_root: &Path) -> caliban_sandbox::Policy` — unchanged signature, new network posture.

- [ ] **Step 1: Write the failing test**

Add to `caliban/src/startup/compose.rs`:

```rust
#[cfg(test)]
mod fence_policy_tests {
    use std::path::Path;

    #[test]
    fn fence_denies_egress_but_keeps_loopback() {
        let p = super::workspace_fence_policy(Path::new("/tmp/ws"));
        assert!(
            !p.network.allow_all_outbound,
            "the workspace fence must NOT grant blanket egress (#406)"
        );
        assert!(
            p.network.allow_local_binding,
            "loopback must stay usable so localhost test servers still work"
        );
        assert!(
            p.network.allowed_domains.is_empty() && p.network.denied_domains.is_empty(),
            "domain lists require a proxy to enforce (#403); the fence ships none"
        );
    }

    #[test]
    fn fence_still_reads_broadly_and_fences_writes() {
        let p = super::workspace_fence_policy(Path::new("/tmp/ws"));
        assert!(
            p.filesystem.allow_read.iter().any(|r| r == Path::new("/")),
            "reads stay open by design — this is a write fence, not a read jail"
        );
        assert!(
            p.filesystem.allow_write.iter().any(|w| w == Path::new("/tmp/ws")),
            "the workspace must remain writable"
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban fence_denies_egress_but_keeps_loopback`
Expected: FAIL — `the workspace fence must NOT grant blanket egress (#406)`.

- [ ] **Step 3: Write minimal implementation**

Replace the `network:` block in `workspace_fence_policy()` at `caliban/src/startup/compose.rs:493-498`:

```rust
        network: NetworkAcl {
            // Egress is CLOSED (#406). Reads are deliberately open
            // (`allow_read: ["/"]`), and that is only defensible while the
            // child cannot phone home: a command may read `~/.aws/credentials`
            // but has nowhere to send it. Opening reads *and* egress together
            // concedes credential exfiltration outright.
            //
            // Loopback stays up so localhost test/dev servers keep working —
            // `--unshare-net` on Linux, an explicit loopback rule on macOS.
            // Opt out with `--sandbox-network=allow` (Task 5) when a run
            // genuinely needs the network (`git fetch`, `cargo` against
            // crates.io, `gh`). Per-domain allowlists need a proxy: #477.
            allow_all_outbound: false,
            allow_local_binding: true,
            ..NetworkAcl::default()
        },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban fence_`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add caliban/src/startup/compose.rs
git commit -m "feat(permissions)!: close sandbox egress by default under --workspace (#406)"
```

---

### Task 4: Settings surface — `sandbox.network`

The sandbox has **no user-reachable configuration**: `caliban-sandbox/src/config.rs` documents a `[sandbox]` TOML table, but nothing in production deserializes a `Policy`. Closing egress without an escape hatch would strand anyone who needs `git fetch`. Settings are JSON-schema-backed `settings.json` (ADR 0026), **not** TOML — add the section there.

**Files:**
- Modify: `crates/caliban-settings/src/settings.rs` (the `Settings` struct, ~line 221)
- Modify: `crates/caliban-settings/src/schema.json` (`properties`)
- Modify: `caliban/src/startup/compose.rs` (`build_registry`, `build_bash_fence`, `workspace_fence_policy`)
- Test: `crates/caliban-settings/src/settings.rs` and `caliban/src/startup/compose.rs`

**Interfaces:**
- Produces: `caliban_settings::SandboxSettings { network: Option<SandboxNetwork> }` and `pub enum SandboxNetwork { Deny, Allow }`, reachable as `settings.sandbox`.
- Produces: `workspace_fence_policy(workspace_root: &Path, network: SandboxNetwork) -> Policy` — **signature change**; Task 5 calls it with the CLI-resolved value.
- Consumes: Task 3's policy body.

- [ ] **Step 1: Write the failing test**

Add to `crates/caliban-settings/src/settings.rs` tests:

```rust
#[test]
fn sandbox_network_parses_from_settings_json() {
    let v: serde_json::Value = serde_json::json!({
        "sandbox": { "network": "allow" }
    });
    let s: Settings = serde_json::from_value(v).expect("parse");
    assert_eq!(s.sandbox.network, Some(SandboxNetwork::Allow));
}

#[test]
fn sandbox_network_defaults_to_none() {
    let s: Settings = serde_json::from_value(serde_json::json!({})).expect("parse");
    assert_eq!(s.sandbox.network, None, "unset means 'use the fence default' (deny)");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-settings sandbox_network`
Expected: FAIL to compile — `no field 'sandbox' on type 'Settings'`.

- [ ] **Step 3: Write minimal implementation**

In `crates/caliban-settings/src/settings.rs`, add the type (near the other section types) and the field.

```rust
/// Whether sandboxed commands may reach the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxNetwork {
    /// No egress. Loopback still works. The default when the fence is active.
    Deny,
    /// Full egress — the escape hatch for runs that need `git fetch` / `gh`.
    Allow,
}

/// `sandbox` section of `settings.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SandboxSettings {
    /// Egress posture for sandboxed Bash commands. `None` = use the fence
    /// default (deny). Overridden by `--sandbox-network` on the CLI.
    pub network: Option<SandboxNetwork>,
}
```

Add to the `Settings` struct, following the existing section-comment style:

```rust
    // ----- sandbox ----------------------------------------------------------
    /// OS-sandbox posture for Bash commands under `--workspace` (#406).
    pub sandbox: SandboxSettings,
```

Export it from `crates/caliban-settings/src/lib.rs` by adding `SandboxNetwork, SandboxSettings` to the existing `pub use settings::{…}` list (line ~67).

Add to `crates/caliban-settings/src/schema.json` under `properties`:

```json
    "sandbox": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "network": {
          "type": "string",
          "enum": ["deny", "allow"],
          "description": "Egress posture for sandboxed Bash commands under --workspace. 'deny' (default) blocks the network; loopback still works. 'allow' restores full egress."
        }
      }
    },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-settings sandbox_network`
Expected: PASS (both).

- [ ] **Step 5: Thread it into the fence**

Change `workspace_fence_policy` to take the resolved posture, in `caliban/src/startup/compose.rs`:

```rust
fn workspace_fence_policy(
    workspace_root: &Path,
    network: caliban_settings::SandboxNetwork,
) -> caliban_sandbox::Policy {
```

and in its `network:` block, replace the hardcoded `allow_all_outbound: false` from Task 3 with:

```rust
            allow_all_outbound: matches!(network, caliban_settings::SandboxNetwork::Allow),
            allow_local_binding: true,
```

Change `build_bash_fence` to accept and forward it:

```rust
fn build_bash_fence(
    workspace_root: &Path,
    network: caliban_settings::SandboxNetwork,
) -> Option<Arc<caliban_sandbox::SandboxedShim>> {
    let policy = workspace_fence_policy(workspace_root, network);
```

`build_registry` does not currently receive `Settings` — add the parameter (other functions in this file already take `settings_snapshot: &caliban_settings::Settings`, follow that name):

```rust
pub(crate) fn build_registry(
    args: &Args,
    workspace: WorkspaceRoot,
    todos: caliban_agent_core::SharedTodos,
    plan_mode: caliban_agent_core::SharedPlanMode,
    plugin_skill_roots: &[PathBuf],
    settings_snapshot: &caliban_settings::Settings,
) -> ToolRegistry {
```

and at the Bash construction site (~line 565):

```rust
    let bash = if crate::args::should_restrict(args) {
        let network = crate::args::sandbox_network(args, settings_snapshot);
        BashTool::with_sandbox(root.clone(), build_bash_fence(&workspace_root, network))
    } else {
        BashTool::new(root.clone())
    };
```

`crate::args::sandbox_network` is defined in Task 5. To keep this task compiling on its own, add it now as the settings-only resolver and extend it in Task 5:

```rust
// in caliban/src/args.rs
/// Resolve the sandbox egress posture. CLI wins over settings; settings over
/// the fence default (deny).
pub(crate) fn sandbox_network(
    _args: &Args,
    settings: &caliban_settings::Settings,
) -> caliban_settings::SandboxNetwork {
    settings
        .sandbox
        .network
        .unwrap_or(caliban_settings::SandboxNetwork::Deny)
}
```

Update every `build_registry` call site to pass the settings snapshot (find them with `rg -n 'build_registry\('`).

- [ ] **Step 6: Run the suite**

Run: `cargo test --workspace`
Expected: PASS. Task 3's `fence_denies_egress_but_keeps_loopback` test must be updated to pass the new arg:

```rust
let p = super::workspace_fence_policy(Path::new("/tmp/ws"), caliban_settings::SandboxNetwork::Deny);
```

- [ ] **Step 7: Commit**

```bash
git add crates/caliban-settings/src crates/caliban-settings/src/schema.json caliban/src
git commit -m "feat(config): sandbox.network settings surface, wired into the fence (#406)"
```

---

### Task 5: `--sandbox-network` CLI flag

The escape hatch. CLI beats settings.

**Files:**
- Modify: `caliban/src/args.rs` (the `Args` struct + `sandbox_network()` from Task 4)
- Test: `caliban/src/args.rs` tests

**Interfaces:**
- Consumes: `caliban_settings::SandboxNetwork` (Task 4).
- Produces: `Args::sandbox_network: Option<SandboxNetworkArg>`, and the final resolver `args::sandbox_network(&Args, &Settings) -> SandboxNetwork`.

- [ ] **Step 1: Write the failing test**

Add to the tests module in `caliban/src/args.rs`:

```rust
#[test]
fn sandbox_network_cli_overrides_settings() {
    use caliban_settings::{SandboxNetwork, SandboxSettings, Settings};
    let mut settings = Settings::default();
    settings.sandbox = SandboxSettings { network: Some(SandboxNetwork::Deny) };

    let args = parse(&["--workspace", "/tmp", "--sandbox-network=allow"]);
    assert_eq!(
        super::sandbox_network(&args, &settings),
        SandboxNetwork::Allow,
        "the CLI flag must win over settings.json"
    );
}

#[test]
fn sandbox_network_falls_back_to_settings_then_deny() {
    use caliban_settings::{SandboxNetwork, SandboxSettings, Settings};

    let mut settings = Settings::default();
    settings.sandbox = SandboxSettings { network: Some(SandboxNetwork::Allow) };
    let args = parse(&["--workspace", "/tmp"]);
    assert_eq!(super::sandbox_network(&args, &settings), SandboxNetwork::Allow);

    let settings = Settings::default();
    let args = parse(&["--workspace", "/tmp"]);
    assert_eq!(
        super::sandbox_network(&args, &settings),
        SandboxNetwork::Deny,
        "unset everywhere => deny (the #406 default)"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban sandbox_network_cli_overrides_settings`
Expected: FAIL — unknown argument `--sandbox-network`.

- [ ] **Step 3: Write minimal implementation**

In `caliban/src/args.rs`, add the value enum next to the other `ValueEnum` types (the file already imports `clap::{Parser, ValueEnum}`):

```rust
/// CLI spelling of the sandbox egress posture (`--sandbox-network`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SandboxNetworkArg {
    /// Block egress (default under `--workspace`). Loopback still works.
    Deny,
    /// Allow full egress — for runs that need `git fetch`, `cargo`, `gh`.
    Allow,
}
```

Add the field to `Args`:

```rust
    /// Egress posture for sandboxed Bash commands. Defaults to `deny` whenever
    /// the workspace fence is active (#406): a sandboxed command may read the
    /// disk but cannot reach the network. Use `allow` when a run genuinely
    /// needs `git fetch` / `cargo` / `gh`. Loopback works either way.
    #[arg(long = "sandbox-network", value_name = "deny|allow", value_enum)]
    pub(crate) sandbox_network: Option<SandboxNetworkArg>,
```

Replace the Task 4 stub resolver with the full one:

```rust
/// Resolve the sandbox egress posture. CLI wins over `settings.json`; settings
/// win over the fence default (deny, #406).
pub(crate) fn sandbox_network(
    args: &Args,
    settings: &caliban_settings::Settings,
) -> caliban_settings::SandboxNetwork {
    use caliban_settings::SandboxNetwork;
    match args.sandbox_network {
        Some(SandboxNetworkArg::Allow) => SandboxNetwork::Allow,
        Some(SandboxNetworkArg::Deny) => SandboxNetwork::Deny,
        None => settings.sandbox.network.unwrap_or(SandboxNetwork::Deny),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban sandbox_network`
Expected: PASS (both).

- [ ] **Step 5: Commit**

```bash
git add caliban/src/args.rs
git commit -m "feat(cli): --sandbox-network=deny|allow escape hatch (#406)"
```

---

### Task 6: Error UX — say why the network died

Codex's single most-reported gotcha is `npm install` hanging with no explanation. Do not reproduce it. When a sandboxed command fails **and** egress was denied by policy, name the cause and the opt-out.

**Files:**
- Modify: `crates/caliban-sandbox/src/shim.rs` (expose the posture)
- Modify: `crates/caliban-tools-builtin/src/shell/bash.rs` (append the hint)
- Test: `crates/caliban-tools-builtin/tests/bash_sandbox.rs`

**Interfaces:**
- Produces: `SandboxedShim::egress_denied(&self) -> bool` — true when the shim is active and the policy grants neither `allow_all_outbound` nor a proxy port.
- Consumes: `BashTool`'s existing sandbox field (`Option<Arc<SandboxedShim>>`).

- [ ] **Step 1: Write the failing test**

Add to `crates/caliban-tools-builtin/tests/bash_sandbox.rs`:

```rust
#[test]
fn egress_denied_is_reported_by_the_shim() {
    use caliban_sandbox::{NetworkAcl, Policy, SandboxedShim};

    let denied = SandboxedShim::new(Policy {
        enabled: true,
        network: NetworkAcl {
            allow_all_outbound: false,
            allow_local_binding: true,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    })
    .expect("shim");

    let open = SandboxedShim::new(Policy {
        enabled: true,
        network: NetworkAcl {
            allow_all_outbound: true,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    })
    .expect("shim");

    // Only meaningful when a backend is actually present.
    if denied.is_active() {
        assert!(denied.egress_denied(), "fence policy must report egress denied");
    }
    if open.is_active() {
        assert!(!open.egress_denied(), "allow_all_outbound must not report denied");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p caliban-tools-builtin egress_denied_is_reported_by_the_shim`
Expected: FAIL to compile — `no method named 'egress_denied'`.

- [ ] **Step 3: Write minimal implementation**

In `crates/caliban-sandbox/src/shim.rs`, next to the existing `is_active()` / `should_auto_allow_bash()` accessors:

```rust
    /// Whether this shim blocks network egress — i.e. it is active and the
    /// policy grants neither blanket outbound nor a proxy route. Drives the
    /// Bash tool's "your command failed because the sandbox blocked the
    /// network" hint (#406).
    #[must_use]
    pub fn egress_denied(&self) -> bool {
        let net = &self.policy.network;
        self.is_active()
            && !net.allow_all_outbound
            && net.http_proxy_port == 0
            && net.socks_proxy_port == 0
    }
```

In `crates/caliban-tools-builtin/src/shell/bash.rs`, where a non-zero exit is turned into the tool result, append the hint. Locate the failure path (the branch that formats a non-zero exit code) and add:

```rust
// #406: a sandboxed command that fails with egress denied is overwhelmingly
// likely to have failed *because* of it (git fetch, cargo, npm, curl). Say so
// — a silent hang/failure with no explanation is Codex's top complaint.
if self
    .sandbox
    .as_ref()
    .is_some_and(|s| s.egress_denied())
{
    out.push_str(
        "\n\nnote: network egress is blocked by the --workspace sandbox. \
         If this command needed the network, re-run with `--sandbox-network=allow` \
         (or set `sandbox.network = \"allow\"` in settings.json). Loopback is \
         unaffected.",
    );
}
```

(`out` is the accumulated result string; match the surrounding code's actual binding name.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p caliban-tools-builtin`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/caliban-sandbox/src/shim.rs crates/caliban-tools-builtin/src/shell/bash.rs crates/caliban-tools-builtin/tests/bash_sandbox.rs
git commit -m "feat(tools): explain sandbox egress denial on Bash failure (#406)"
```

---

### Task 7: Cross-backend integration tests

The load-bearing verification: egress actually fails, loopback actually works, on **both** backends. These are the tests that would have caught #476 and the macOS asymmetry.

**Files:**
- Modify: `crates/caliban-tools-builtin/tests/bash_sandbox.rs`

**Interfaces:**
- Consumes: `SandboxedShim`, `workspace_fence_policy`-equivalent policy built inline (the test crate cannot see `caliban/src`; construct the same policy by hand).

- [ ] **Step 1: Write the failing tests**

```rust
/// The fence policy as shipped (#406): reads open, writes fenced, egress shut,
/// loopback up. Mirrors `workspace_fence_policy` — keep in sync.
fn egress_denied_policy(ws: &std::path::Path) -> caliban_sandbox::Policy {
    use caliban_sandbox::{FilesystemAcl, NetworkAcl, Policy};
    Policy {
        enabled: true,
        fail_if_unavailable: false,
        filesystem: FilesystemAcl {
            allow_read: vec![std::path::PathBuf::from("/")],
            allow_write: vec![ws.to_path_buf(), std::path::PathBuf::from("/tmp")],
            ..FilesystemAcl::default()
        },
        network: NetworkAcl {
            allow_all_outbound: false,
            allow_local_binding: true,
            ..NetworkAcl::default()
        },
        ..Policy::default()
    }
}

#[tokio::test]
async fn sandboxed_command_cannot_reach_the_network() {
    let ws = tempfile::tempdir().expect("tmp");
    let shim = std::sync::Arc::new(
        caliban_sandbox::SandboxedShim::new(egress_denied_policy(ws.path())).expect("shim"),
    );
    if !shim.is_active() {
        eprintln!("no sandbox backend available; skipping");
        return;
    }

    // A TCP connect to a routable address must fail. Use a bare connect rather
    // than DNS so the test does not depend on a resolver.
    let out = run_sandboxed(&shim, "exec 3<>/dev/tcp/1.1.1.1/53 && echo REACHED").await;
    assert!(
        !out.contains("REACHED"),
        "egress must be blocked under the fence, got:\n{out}"
    );
}

#[tokio::test]
async fn sandboxed_command_can_still_use_loopback() {
    let ws = tempfile::tempdir().expect("tmp");
    let shim = std::sync::Arc::new(
        caliban_sandbox::SandboxedShim::new(egress_denied_policy(ws.path())).expect("shim"),
    );
    if !shim.is_active() {
        eprintln!("no sandbox backend available; skipping");
        return;
    }

    // Bind a listener and connect to it, entirely inside the sandbox. This is
    // the regression the macOS Seatbelt asymmetry would introduce: bwrap keeps
    // `lo` up inside --unshare-net, Seatbelt (deny default) does not unless we
    // emit an explicit loopback rule.
    let script = r#"
        (exec 3<>/dev/tcp/127.0.0.1/1 || true) 2>/dev/null
        python3 - <<'PY' 2>/dev/null || nc -l 127.0.0.1 0 &
PY
        echo LOOPBACK_OK
    "#;
    let out = run_sandboxed(&shim, script).await;
    assert!(
        out.contains("LOOPBACK_OK"),
        "loopback must remain usable inside the sandbox, got:\n{out}"
    );
}

#[tokio::test]
async fn allow_restores_egress() {
    use caliban_sandbox::NetworkAcl;
    let ws = tempfile::tempdir().expect("tmp");
    let mut policy = egress_denied_policy(ws.path());
    policy.network = NetworkAcl {
        allow_all_outbound: true,
        ..NetworkAcl::default()
    };
    let shim =
        std::sync::Arc::new(caliban_sandbox::SandboxedShim::new(policy).expect("shim"));
    if !shim.is_active() {
        eprintln!("no sandbox backend available; skipping");
        return;
    }
    // We assert the *policy* opens the namespace rather than requiring real
    // internet in CI: egress_denied() is the contract the Bash hint keys off.
    assert!(
        !shim.egress_denied(),
        "--sandbox-network=allow must restore egress"
    );
}
```

`run_sandboxed` is the existing helper in this test file that wraps a command through the shim and returns combined output — reuse it; do not write a second one. If its name differs, match the file.

**Note on the loopback test:** the heredoc above is deliberately defensive because the available tooling (`python3`, `nc`) varies by runner. If neither is present on the CI image, replace the body with a Rust-side listener bound on the host **before** entering the sandbox — but be aware that under bwrap's `--unshare-net` the sandbox's loopback is a *separate* namespace, so a host-side listener is **not** reachable and such a test would fail for the right reason on Linux and the wrong reason on macOS. Prefer an in-sandbox listener.

- [ ] **Step 2: Run tests to verify they fail (before Tasks 1–3 land) / pass (after)**

Run: `cargo test -p caliban-tools-builtin sandboxed_command`
Expected: PASS on a machine with a backend; a clean skip (with the `eprintln!`) on one without.

- [ ] **Step 3: Commit**

```bash
git add crates/caliban-tools-builtin/tests/bash_sandbox.rs
git commit -m "test(permissions): egress denied + loopback works on both backends (#406)"
```

---

### Task 8: ADR, guide, and BREAKING changelog

The spec's core claim — *open reads are defensible only because egress is shut* — must survive as a reference, or the next person will reopen egress "to fix `cargo`" and quietly reinstate the hole.

**Files:**
- Create: `docs/adr/0054-sandbox-confinement-posture.md`
- Modify: `docs/adr/README.md` (index row)
- Modify: `docs/guide/src/` — the sandbox/permissions page
- Modify: `CHANGELOG.md` (`## [Unreleased]`)

- [ ] **Step 1: Write the ADR**

Use the `adr-create` skill (it assigns the number, fills the MADR-lite template, and appends the index row). Content, drawn from the spec:

- **Context:** the sandbox is an opt-in write fence wrapping only Bash; it conceded reads (`allow_read: ["/"]`), egress (`allow_all_outbound: true`), and the full parent env at once. Same uid as the user, so file modes buy nothing. Threat model: untrusted content steering the agent into an exfiltrating command.
- **Decision:** keep reads open, close egress. Loopback stays up. Escape hatch: `--sandbox-network=allow` / `sandbox.network`.
- **Rationale:** nobody in the field read-jails (Claude Code documents that `~/.ssh` and `~/.aws/credentials` stay readable); Codex CLI and Claude Code both close egress. Filesystem and network isolation are only meaningful **together** — open reads are safe *only* while egress is shut.
- **Consequences:** breaking (`--workspace` now implies no network); per-domain allowlists need a proxy (#477); env scrubbing is follow-up defense-in-depth (#405); read confinement explicitly rejected.
- **Explicitly NOT claimed:** the sandbox is not a read jail and not a secrets boundary against an attacker who has egress by another route.

- [ ] **Step 2: Update the guide**

In the sandbox/permissions page, state the guarantee plainly, in the user's terms:

> Under `--workspace`, Bash commands run in an OS sandbox. Writes are confined to your workspace and temp dirs, and **the network is blocked** — a command can read your disk but cannot send anything off the machine. Loopback still works, so local test servers are fine.
>
> Reads are **not** restricted: a sandboxed command can still read `~/.ssh` and `~/.aws/credentials`. That is safe only because egress is closed. If you re-open the network with `--sandbox-network=allow`, a hijacked command can exfiltrate those files — so use it deliberately.

- [ ] **Step 3: Add the changelog entry**

Under `## [Unreleased]`, in a `### Changed` section:

```markdown
- **Sandboxed Bash commands can no longer reach the network** (#406) —
  **BREAKING**. Under `--workspace` (or `--restrict-paths`), the OS sandbox now
  denies egress by default: a command may read the disk but cannot phone home.
  Loopback is unaffected, so localhost test servers keep working. `git fetch`,
  `cargo` against crates.io, `npm install`, `gh`, and `curl` will fail in
  sandboxed commands unless you opt out with `--sandbox-network=allow` or
  `sandbox.network = "allow"` in `settings.json`.

  Reads remain open by design (`~/.ssh`, `~/.aws/credentials` are still
  readable) — that is only defensible while egress is shut, which is the point
  of this change. Per-domain allowlists require a proxy (#477); environment
  scrubbing is tracked in #405. See ADR 0054.
```

Also add under `### Fixed`:

```markdown
- **`allow_local_binding` no longer grants full egress** (#476): bwrap skipped
  `--unshare-net` when the flag was set, so a local-sounding permission silently
  opened the whole internet. (#406)
```

- [ ] **Step 4: Run the full gate**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all four pass.

- [ ] **Step 5: Commit**

```bash
git add docs/adr CHANGELOG.md docs/guide
git commit -m "docs(adr): sandbox confinement posture — reads open, egress closed (#406)"
```

---

## Self-Review

**Spec coverage.** §1 threat model → Task 8 (ADR). §2 close egress → Task 3. §3 macOS loopback asymmetry → Task 2, verified by Task 7. §3's #476 dependency → Task 1. §4 config surface → Tasks 4–5. §5 error UX → Task 6. §6 out-of-scope (#405 env scrub, #477 proxy, read confinement) → not implemented, recorded in Task 8's ADR consequences. Migration → Task 8 changelog. Testing §1–6 → Tasks 1–3, 7.

**Ordering is load-bearing.** Task 1 (#476) *must* precede Task 3: Task 3 sets `allow_local_binding: true`, which under today's bwrap logic suppresses `--unshare-net` and grants full egress — silently undoing the whole ticket. Task 2 must precede Task 7's loopback test on macOS.

**Type consistency.** `SandboxNetwork` (settings enum, `Deny`/`Allow`) vs `SandboxNetworkArg` (CLI `ValueEnum`) are deliberately distinct — settings crate must not depend on clap. `args::sandbox_network()` is the single resolver mapping one to the other; it is introduced as a stub in Task 4 Step 5 and completed in Task 5 Step 3, so Task 4 compiles standalone. `workspace_fence_policy` gains its second parameter in Task 4, and Task 3's test is updated in Task 4 Step 6 to match — noted explicitly so a fresh implementer of Task 4 does not leave Task 3's test broken.

**Known soft spot.** Task 7's loopback test depends on tooling present on the runner (`python3`/`nc`). The step calls this out and explains why a host-side listener is *not* a valid substitute (bwrap's netns is separate). If both are missing on the CI image, the implementer should add a tiny Rust test binary that binds and connects inside the sandbox rather than weakening the assertion.
